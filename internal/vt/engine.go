// Package vt owns isolated, bounded Ghostty WebAssembly terminals in the Go service.
package vt

import (
	"context"
	_ "embed"
	"encoding/json"
	"errors"
	"fmt"
	"sort"
	"strings"
	"sync"
	"time"

	"github.com/tetratelabs/wazero"
	"github.com/tetratelabs/wazero/api"
)

const MaxCapture = 4 << 20

var ErrCaptureLimit = errors.New("capture exceeds 4 MiB")

//go:embed callbacks.wasm
var callbacks []byte

type Engine struct {
	cache  wazero.CompilationCache
	wasm   []byte
	config wazero.RuntimeConfig
	warm   wazero.Runtime
}

func NewEngine(ctx context.Context, wasm []byte) (*Engine, error) {
	cache := wazero.NewCompilationCache()
	config := wazero.NewRuntimeConfig().WithCompilationCache(cache).WithMemoryLimitPages(1024).WithCloseOnContextDone(true)
	warm := wazero.NewRuntimeWithConfig(ctx, config)
	if _, err := warm.CompileModule(ctx, wasm); err != nil {
		warm.Close(ctx)
		cache.Close(ctx)
		return nil, fmt.Errorf("compile Ghostty VT: %w", err)
	}
	if _, err := warm.CompileModule(ctx, callbacks); err != nil {
		warm.Close(ctx)
		cache.Close(ctx)
		return nil, err
	}
	return &Engine{cache: cache, wasm: append([]byte(nil), wasm...), config: config, warm: warm}, nil
}
func (e *Engine) Close() { e.warm.Close(context.Background()); e.cache.Close(context.Background()) }

type abiField struct{ Offset, Size uint32 }
type abiType struct {
	Size   uint32
	Fields map[string]abiField
	Values map[string]int64
}
type manifest struct {
	ABI struct {
		PointerSize uint32 `json:"pointer_size"`
		UsizeSize   uint32 `json:"usize_size"`
		Endian      string
	}
	Types map[string]abiType
}

func (a manifest) value(name, key string) (uint64, error) {
	t, ok := a.Types[name]
	if !ok {
		return 0, fmt.Errorf("missing VT ABI type %s", name)
	}
	v, ok := t.Values[key]
	if !ok {
		return 0, fmt.Errorf("missing VT ABI selector %s.%s", name, key)
	}
	return uint64(uint32(v)), nil
}
func (a manifest) field(name, key string) (abiField, error) {
	t, ok := a.Types[name]
	if !ok {
		return abiField{}, fmt.Errorf("missing VT ABI type %s", name)
	}
	f, ok := t.Fields[key]
	if !ok || f.Size == 0 || uint64(f.Offset)+uint64(f.Size) > uint64(t.Size) {
		return abiField{}, fmt.Errorf("invalid VT ABI field %s.%s", name, key)
	}
	return f, nil
}

type Status struct {
	ID       string `json:"id"`
	App      string `json:"app"`
	Title    string `json:"title"`
	Message  string `json:"message"`
	State    string `json:"state"`
	Kind     string `json:"kind"`
	Progress int    `json:"progress"`
}
type Prompt struct {
	Line     uint32 `json:"line"`
	Kind     string `json:"kind"`
	Command  string `json:"command,omitempty"`
	ExitCode *int32 `json:"exit_code,omitempty"`
}
type Info struct {
	Title    string   `json:"title"`
	PWD      string   `json:"pwd"`
	Revision uint64   `json:"revision"`
	Records  []Status `json:"records"`
	Prompts  []Prompt `json:"prompts"`
}

type State struct {
	mu                                sync.Mutex
	runtime                           wazero.Runtime
	module                            api.Module
	shim                              api.Module
	abi                               manifest
	terminal                          uint32
	scratch                           uint32
	cols, rows, cellWidth, cellHeight uint32
	replies                           []byte
	callbackErr                       error
	dirtyTitle, dirtyPWD              bool
	info                              Info
	records                           map[string]Status
	functions                         map[string]api.Function
	closed                            bool
}

func (e *Engine) NewTerminal(ctx context.Context, cols, rows uint32) (_ *State, err error) {
	s := &State{runtime: wazero.NewRuntimeWithConfig(ctx, e.config), cols: cols, rows: rows, records: make(map[string]Status), functions: make(map[string]api.Function)}
	defer func() {
		if err != nil {
			s.runtime.Close(context.Background())
		}
	}()
	_, err = s.runtime.NewHostModuleBuilder("env").NewFunctionBuilder().WithFunc(func(uint32, uint32) {}).Export("log").Instantiate(ctx)
	if err != nil {
		return nil, err
	}
	host := s.runtime.NewHostModuleBuilder("rexweb_host")
	host.NewFunctionBuilder().WithFunc(s.writePTY).Export("write_pty")
	host.NewFunctionBuilder().WithFunc(s.programStatus).Export("program_status")
	host.NewFunctionBuilder().WithFunc(s.semanticPrompt).Export("semantic_prompt")
	host.NewFunctionBuilder().WithFunc(func(context.Context, api.Module, uint32, uint32) { s.dirtyTitle = true }).Export("title_changed")
	host.NewFunctionBuilder().WithFunc(func(context.Context, api.Module, uint32, uint32) { s.dirtyPWD = true }).Export("pwd_changed")
	host.NewFunctionBuilder().WithFunc(s.size).Export("size")
	if _, err = host.Instantiate(ctx); err != nil {
		return nil, err
	}
	compiled, err := s.runtime.CompileModule(ctx, e.wasm)
	if err != nil {
		return nil, err
	}
	s.module, err = s.runtime.InstantiateModule(ctx, compiled, wazero.NewModuleConfig().WithName("vt").WithStartFunctions())
	if err != nil {
		return nil, err
	}
	for _, name := range []string{"ghostty_type_json", "ghostty_wasm_alloc", "ghostty_wasm_free", "ghostty_wasm_alloc_opaque", "ghostty_wasm_free_opaque", "ghostty_terminal_new", "ghostty_terminal_free", "ghostty_terminal_set", "ghostty_terminal_get", "ghostty_terminal_vt_write", "ghostty_terminal_resize", "ghostty_formatter_terminal_new", "ghostty_formatter_set", "ghostty_formatter_format_buf", "ghostty_formatter_free"} {
		fn := s.module.ExportedFunction(name)
		if fn == nil {
			return nil, fmt.Errorf("missing Ghostty export %s", name)
		}
		s.functions[name] = fn
	}
	pointer, err := s.call(ctx, "ghostty_type_json")
	if err != nil {
		return nil, err
	}
	raw, err := s.cstring(uint32(pointer), 4<<20)
	if err != nil {
		return nil, err
	}
	if err = json.Unmarshal([]byte(raw), &s.abi); err != nil {
		return nil, err
	}
	if s.abi.ABI.PointerSize != 4 || s.abi.ABI.UsizeSize != 4 || s.abi.ABI.Endian != "little" {
		return nil, errors.New("unsupported Ghostty VT ABI")
	}
	s.scratch, err = s.alloc(ctx, 8)
	if err != nil {
		return nil, err
	}
	if err = s.result(ctx, "ghostty_terminal_new", 0, uint64(s.scratch), uint64(cols), uint64(rows)); err != nil {
		return nil, err
	}
	s.terminal, err = s.read32(s.scratch)
	if err != nil || s.terminal == 0 {
		return nil, errors.New("Ghostty did not create a terminal")
	}
	for name, value := range map[string]uint64{"SCROLLBACK_MAX_BYTES": 16 << 20, "SCROLLBACK_MAX_LINES": 2000, "KITTY_IMAGE_STORAGE_LIMIT": 0} {
		key, err := s.abi.value("GhosttyTerminalOption", name)
		if err != nil {
			return nil, err
		}
		s.put32(s.scratch, uint32(value))
		s.put32(s.scratch+4, 0)
		if err = s.result(ctx, "ghostty_terminal_set", uint64(s.terminal), key, uint64(s.scratch)); err != nil {
			return nil, err
		}
	}
	compiledShim, err := s.runtime.CompileModule(ctx, callbacks)
	if err != nil {
		return nil, err
	}
	s.shim, err = s.runtime.InstantiateModule(ctx, compiledShim, wazero.NewModuleConfig().WithName("callbacks").WithStartFunctions())
	if err != nil {
		return nil, err
	}
	for export, option := range map[string]string{"install_write": "WRITE_PTY", "install_status": "PROGRAM_STATUS", "install_prompt": "SEMANTIC_PROMPT", "install_title": "TITLE_CHANGED", "install_pwd": "PWD_CHANGED", "install_size": "SIZE"} {
		values, err := s.shim.ExportedFunction(export).Call(ctx)
		if err != nil || len(values) != 1 || uint32(values[0]) == ^uint32(0) {
			return nil, fmt.Errorf("install VT callback %s: %v", export, err)
		}
		key, err := s.abi.value("GhosttyTerminalOption", option)
		if err != nil {
			return nil, err
		}
		if err = s.result(ctx, "ghostty_terminal_set", uint64(s.terminal), key, values[0]); err != nil {
			return nil, err
		}
	}
	return s, nil
}

func (s *State) call(ctx context.Context, name string, args ...uint64) (uint64, error) {
	result, err := s.functions[name].Call(ctx, args...)
	if err != nil {
		return 0, err
	}
	if len(result) == 0 {
		return 0, nil
	}
	return result[0], nil
}
func (s *State) result(ctx context.Context, name string, args ...uint64) error {
	value, err := s.call(ctx, name, args...)
	if err != nil {
		return err
	}
	if int32(value) != 0 {
		return fmt.Errorf("%s: Ghostty status %d", name, int32(value))
	}
	return nil
}
func (s *State) alloc(ctx context.Context, size uint32) (uint32, error) {
	value, err := s.call(ctx, "ghostty_wasm_alloc", uint64(size))
	if err != nil {
		return 0, err
	}
	if value == 0 {
		return 0, errors.New("Ghostty allocation failed")
	}
	return uint32(value), nil
}
func (s *State) free(ctx context.Context, ptr, size uint32) {
	s.call(ctx, "ghostty_wasm_free", uint64(ptr), uint64(size))
}
func (s *State) read32(ptr uint32) (uint32, error) {
	v, ok := s.module.Memory().ReadUint32Le(ptr)
	if !ok {
		return 0, errors.New("Ghostty memory read out of bounds")
	}
	return v, nil
}
func (s *State) put32(ptr, value uint32) {
	if !s.module.Memory().WriteUint32Le(ptr, value) {
		s.callbackErr = errors.New("Ghostty memory write out of bounds")
	}
}
func (s *State) cstring(ptr, max uint32) (string, error) {
	limit := s.module.Memory().Size()
	if ptr == 0 || ptr >= limit {
		return "", errors.New("invalid Ghostty string pointer")
	}
	end := ptr
	for end-ptr < max && end < limit {
		value, ok := s.module.Memory().ReadByte(end)
		if !ok {
			return "", errors.New("Ghostty string read failed")
		}
		if value == 0 {
			raw, _ := s.module.Memory().Read(ptr, end-ptr)
			return string(raw), nil
		}
		end++
	}
	return "", errors.New("unterminated Ghostty string")
}
func (s *State) stringAt(ptr uint32, max uint32) string {
	pf, err := s.abi.field("GhosttyString", "ptr")
	if err != nil {
		s.callbackErr = err
		return ""
	}
	lf, err := s.abi.field("GhosttyString", "len")
	if err != nil {
		s.callbackErr = err
		return ""
	}
	p, err := s.read32(ptr + pf.Offset)
	if err != nil {
		s.callbackErr = err
		return ""
	}
	length, err := s.read32(ptr + lf.Offset)
	if err != nil {
		s.callbackErr = err
		return ""
	}
	if length > max {
		length = max
	}
	raw, ok := s.module.Memory().Read(p, length)
	if !ok {
		s.callbackErr = errors.New("invalid Ghostty metadata string")
		return ""
	}
	return string(raw)
}
func (s *State) member(ptr uint32, name, key string) uint32 {
	f, err := s.abi.field(name, key)
	if err != nil {
		s.callbackErr = err
		return ptr
	}
	return ptr + f.Offset
}
func (s *State) enumName(name string, value uint32) string {
	for key, v := range s.abi.Types[name].Values {
		if uint32(v) == value && key != "MAX_VALUE" {
			return strings.ToLower(key)
		}
	}
	return "unknown"
}
func (s *State) writePTY(_ context.Context, _ api.Module, _ uint32, _ uint32, ptr, length uint32) {
	if length == 0 {
		return
	}
	if uint64(len(s.replies))+uint64(length) > 64<<10 {
		s.callbackErr = errors.New("Ghostty response limit exceeded")
		return
	}
	raw, ok := s.module.Memory().Read(ptr, length)
	if !ok {
		s.callbackErr = errors.New("invalid Ghostty response")
		return
	}
	s.replies = append(s.replies, raw...)
}
func (s *State) programStatus(_ context.Context, _ api.Module, _ uint32, _ uint32, ptr uint32) {
	t := "GhosttyTerminalProgramStatus"
	state, _ := s.read32(s.member(ptr, t, "state"))
	kind, _ := s.read32(s.member(ptr, t, "kind"))
	p, ok := s.module.Memory().ReadByte(s.member(ptr, t, "progress"))
	if !ok {
		s.callbackErr = errors.New("invalid program status")
		return
	}
	record := Status{State: s.enumName("GhosttyProgramStatusState", state), Kind: s.enumName("GhosttyProgramStatusKind", kind), Progress: int(int8(p)), ID: s.stringAt(s.member(ptr, t, "id"), 256), App: s.stringAt(s.member(ptr, t, "app"), 256), Title: s.stringAt(s.member(ptr, t, "title"), 1024), Message: s.stringAt(s.member(ptr, t, "message"), 4096)}
	if record.State == "clear" {
		delete(s.records, record.ID)
	} else if _, exists := s.records[record.ID]; exists || len(s.records) < 32 {
		s.records[record.ID] = record
	}
	s.info.Revision++
}
func (s *State) get32(ctx context.Context, key string) (uint32, error) {
	value, err := s.abi.value("GhosttyTerminalData", key)
	if err != nil {
		return 0, err
	}
	if err = s.result(ctx, "ghostty_terminal_get", uint64(s.terminal), value, uint64(s.scratch)); err != nil {
		return 0, err
	}
	return s.read32(s.scratch)
}
func (s *State) semanticPrompt(ctx context.Context, _ api.Module, _ uint32, _ uint32, ptr uint32) {
	t := "GhosttyTerminalSemanticPrompt"
	kind, _ := s.read32(s.member(ptr, t, "kind"))
	mark := Prompt{Kind: s.enumName("GhosttySemanticPromptKind", kind), Command: s.stringAt(s.member(ptr, t, "command"), 4096)}
	total, err := s.get32(ctx, "TOTAL_ROWS")
	if err != nil {
		s.callbackErr = err
		return
	}
	row, err := s.get32(ctx, "CURSOR_Y")
	if err != nil {
		s.callbackErr = err
		return
	}
	mark.Line = uint32(max(0, int64(total)-int64(s.rows))) + row
	has, _ := s.module.Memory().ReadByte(s.member(ptr, t, "has_exit_code"))
	if has != 0 {
		code, _ := s.read32(s.member(ptr, t, "exit_code"))
		value := int32(code)
		mark.ExitCode = &value
	}
	s.info.Prompts = append(s.info.Prompts, mark)
	if len(s.info.Prompts) > 256 {
		s.info.Prompts = append([]Prompt(nil), s.info.Prompts[len(s.info.Prompts)-256:]...)
	}
	s.info.Revision++
}
func (s *State) size(_ context.Context, _ api.Module, _ uint32, _ uint32, ptr uint32) uint32 {
	t := "GhosttySizeReportSize"
	s.module.Memory().WriteUint16Le(s.member(ptr, t, "rows"), uint16(s.rows))
	s.module.Memory().WriteUint16Le(s.member(ptr, t, "columns"), uint16(s.cols))
	s.put32(s.member(ptr, t, "cell_width"), s.cellWidth)
	s.put32(s.member(ptr, t, "cell_height"), s.cellHeight)
	return 1
}
func (s *State) pullString(ctx context.Context, key string) (string, error) {
	value, err := s.abi.value("GhosttyTerminalData", key)
	if err != nil {
		return "", err
	}
	if err = s.result(ctx, "ghostty_terminal_get", uint64(s.terminal), value, uint64(s.scratch)); err != nil {
		return "", err
	}
	return s.stringAt(s.scratch, 4096), s.callbackErr
}

func (s *State) Feed(data []byte) ([]byte, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.closed {
		return nil, errors.New("VT terminal closed")
	}
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	if len(data) == 0 {
		return nil, nil
	}
	if len(data) > 64<<10 {
		return nil, errors.New("VT feed exceeds 64 KiB")
	}
	ptr, err := s.alloc(ctx, uint32(len(data)))
	if err != nil {
		return nil, err
	}
	defer s.free(ctx, ptr, uint32(len(data)))
	if !s.module.Memory().Write(ptr, data) {
		return nil, errors.New("VT feed memory overflow")
	}
	s.replies = nil
	s.callbackErr = nil
	if _, err = s.call(ctx, "ghostty_terminal_vt_write", uint64(s.terminal), uint64(ptr), uint64(len(data))); err != nil {
		return nil, err
	}
	if s.dirtyTitle {
		s.info.Title, _ = s.pullString(ctx, "TITLE")
		s.dirtyTitle = false
		s.info.Revision++
	}
	if s.dirtyPWD {
		s.info.PWD, _ = s.pullString(ctx, "PWD")
		s.dirtyPWD = false
		s.info.Revision++
	}
	return append([]byte(nil), s.replies...), s.callbackErr
}
func (s *State) Resize(cols, rows, cellWidth, cellHeight uint32) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.closed {
		return errors.New("VT terminal closed")
	}
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	if err := s.result(ctx, "ghostty_terminal_resize", uint64(s.terminal), uint64(cols), uint64(rows), uint64(cellWidth), uint64(cellHeight)); err != nil {
		return err
	}
	s.cols, s.rows, s.cellWidth, s.cellHeight = cols, rows, cellWidth, cellHeight
	return nil
}
func (s *State) Info() Info {
	s.mu.Lock()
	defer s.mu.Unlock()
	result := s.info
	result.Prompts = append([]Prompt(nil), s.info.Prompts...)
	result.Records = make([]Status, 0, len(s.records))
	for _, record := range s.records {
		result.Records = append(result.Records, record)
	}
	sort.Slice(result.Records, func(i, j int) bool { return result.Records[i].ID < result.Records[j].ID })
	return result
}
func (s *State) Capture(format string, trim, unwrap bool) ([]byte, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.closed {
		return nil, errors.New("VT terminal closed")
	}
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	formats := map[string]string{"text": "PLAIN", "vt": "VT", "html": "HTML"}
	kind, ok := formats[format]
	if !ok {
		return nil, errors.New("unsupported capture format")
	}
	emit, err := s.abi.value("GhosttyFormatterFormat", kind)
	if err != nil {
		return nil, err
	}
	if err = s.result(ctx, "ghostty_formatter_terminal_new", 0, uint64(s.scratch), uint64(s.terminal)); err != nil {
		return nil, err
	}
	formatter, err := s.read32(s.scratch)
	if err != nil {
		return nil, err
	}
	defer s.call(ctx, "ghostty_formatter_free", uint64(formatter))
	for key, value := range map[string]uint32{"EMIT": uint32(emit), "TRIM": boolInt(trim), "UNWRAP": boolInt(unwrap)} {
		option, err := s.abi.value("GhosttyFormatterOption", key)
		if err != nil {
			return nil, err
		}
		s.put32(s.scratch, value)
		if err = s.result(ctx, "ghostty_formatter_set", uint64(formatter), option, uint64(s.scratch)); err != nil {
			return nil, err
		}
	}
	if format == "vt" {
		key, _ := s.abi.value("GhosttyFormatterOption", "EXTRAS")
		s.put32(s.scratch, 4095)
		if err = s.result(ctx, "ghostty_formatter_set", uint64(formatter), key, uint64(s.scratch)); err != nil {
			return nil, err
		}
	}
	status, err := s.call(ctx, "ghostty_formatter_format_buf", uint64(formatter), 0, 0, uint64(s.scratch))
	if err != nil {
		return nil, err
	}
	if int32(status) != 0 && int32(status) != -3 {
		return nil, fmt.Errorf("measure capture: Ghostty status %d", int32(status))
	}
	required, err := s.read32(s.scratch)
	if err != nil {
		return nil, err
	}
	if required > MaxCapture {
		return nil, ErrCaptureLimit
	}
	if required == 0 {
		return []byte{}, nil
	}
	ptr, err := s.alloc(ctx, required)
	if err != nil {
		return nil, err
	}
	defer s.free(ctx, ptr, required)
	if err = s.result(ctx, "ghostty_formatter_format_buf", uint64(formatter), uint64(ptr), uint64(required), uint64(s.scratch)); err != nil {
		return nil, err
	}
	length, err := s.read32(s.scratch)
	if err != nil || length > required {
		return nil, errors.New("invalid capture length")
	}
	raw, ok := s.module.Memory().Read(ptr, length)
	if !ok {
		return nil, errors.New("invalid capture memory")
	}
	result := append([]byte(nil), raw...)
	if format == "html" {
		sanitized, err := sanitizeHTML(result)
		if err != nil {
			return nil, err
		}
		if len(sanitized) > MaxCapture {
			return nil, ErrCaptureLimit
		}
		return sanitized, nil
	}
	return result, nil
}
func boolInt(value bool) uint32 {
	if value {
		return 1
	}
	return 0
}
func (s *State) Close() {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.closed {
		return
	}
	s.closed = true
	s.runtime.Close(context.Background())
}
