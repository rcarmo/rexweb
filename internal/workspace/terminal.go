package workspace

import (
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"runtime"
	"strings"
	"sync"
	"syscall"
	"time"

	"github.com/creack/pty"
	"rexweb/internal/vt"
)

const historyBytes = 512 << 10

type pane struct {
	id      string
	file    *os.File
	process *os.Process
	owner   string // Protected by Store.mu.
	inputMu sync.Mutex

	mu       sync.Mutex
	history  []byte
	end      int64
	updates  chan paneUpdate
	consumer string
	vt       terminalModes
	state    *vt.State
	stateErr error
	blobs    map[uint32]captureBlob
	nextBlob uint32
	exitCode int
	size     pty.Winsize
	lagging  bool
	finished bool
}

type attachment struct {
	Offset int64 `json:"offset"`
	Reset  bool  `json:"reset"`
	replay []byte
	live   chan paneUpdate
	size   pty.Winsize
	exited bool
}

type paneUpdate struct {
	output []byte
	size   *pty.Winsize
}

func startPane(sandbox Sandbox, shell string, engine *vt.Engine) (*pane, error) {
	var state *vt.State
	var err error
	if engine != nil {
		state, err = engine.NewTerminal(context.Background(), 80, 24)
		if err != nil {
			return nil, err
		}
	}
	success := false
	defer func() {
		if !success && state != nil {
			state.Close()
		}
	}()
	master, slave, err := pty.Open()
	if err != nil {
		return nil, err
	}
	defer slave.Close()
	if err := pty.Setsize(master, &pty.Winsize{Cols: 80, Rows: 24}); err != nil {
		master.Close()
		return nil, err
	}
	cmd := sandbox.command(slave.Name(), shell, "-i")
	cmd.Stdin, cmd.Stdout, cmd.Stderr = slave, slave, slave
	if cmd.SysProcAttr == nil {
		cmd.SysProcAttr = &syscall.SysProcAttr{}
	}
	cmd.SysProcAttr.Setsid, cmd.SysProcAttr.Setctty = true, true
	var readyRead, readyWrite *os.File
	if runtime.GOOS == "linux" && !sandbox.host {
		readyRead, readyWrite, err = os.Pipe()
		if err != nil {
			master.Close()
			return nil, err
		}
		defer readyRead.Close()
		defer readyWrite.Close()
		cmd.ExtraFiles = []*os.File{readyWrite}
	}
	if err := cmd.Start(); err != nil {
		master.Close()
		return nil, err
	}
	if readyRead != nil {
		readyWrite.Close()
		readyRead.SetReadDeadline(time.Now().Add(5 * time.Second))
		// The helper marks fd 3 close-on-exec. EOF means successful exec; an
		// error message or deadline means no pane is admitted into the Store.
		message, setupErr := io.ReadAll(io.LimitReader(readyRead, 4096))
		if setupErr != nil || len(message) != 0 {
			cmd.Process.Kill()
			cmd.Wait()
			master.Close()
			return nil, fmt.Errorf("start sandboxed shell: %s (%v)", strings.TrimSpace(string(message)), setupErr)
		}
	}
	file, err := pollableMaster(master)
	master.Close()
	if err != nil {
		cmd.Process.Kill()
		cmd.Wait()
		return nil, err
	}

	p := &pane{id: newID("block"), file: file, process: cmd.Process, history: make([]byte, historyBytes), state: state, size: pty.Winsize{Cols: 80, Rows: 24}}
	wait := make(chan int, 1)
	go func() {
		cmd.Wait()
		code := 1
		if cmd.ProcessState != nil {
			code = cmd.ProcessState.ExitCode()
		}
		wait <- code
	}()
	go func() {
		buf := make([]byte, 16<<10)
		for {
			n, err := p.file.Read(buf)
			if n > 0 {
				p.publish(buf[:n])
			}
			if err != nil {
				break
			}
		}
		p.mu.Lock()
		p.file.Close()
		p.mu.Unlock()
		code := <-wait
		p.mu.Lock()
		p.exitCode = code
		p.finished = true
		if p.updates != nil && !p.lagging {
			close(p.updates)
		}
		p.mu.Unlock()
	}()
	success = true
	return p, nil
}

// Reopen the descriptor with Go's poller so input deadlines and Close work on the PTY.
func pollableMaster(master *os.File) (*os.File, error) {
	raw, err := master.SyscallConn()
	if err != nil {
		return nil, err
	}
	var fd int
	var dupErr error
	if err := raw.Control(func(original uintptr) {
		syscall.ForkLock.RLock()
		defer syscall.ForkLock.RUnlock()
		fd, dupErr = syscall.Dup(int(original))
		if dupErr == nil {
			syscall.CloseOnExec(fd)
		}
	}); err != nil {
		return nil, err
	}
	if dupErr != nil {
		return nil, dupErr
	}
	if err := syscall.SetNonblock(fd, true); err != nil {
		syscall.Close(fd)
		return nil, err
	}
	file := os.NewFile(uintptr(fd), master.Name())
	if err := file.SetWriteDeadline(time.Time{}); err != nil {
		file.Close()
		return nil, fmt.Errorf("configure PTY polling: %w", err)
	}
	return file, nil
}

func (p *pane) publish(data []byte) {
	p.mu.Lock()
	var replies []byte
	if p.state != nil && p.stateErr == nil {
		replies, p.stateErr = p.state.Feed(data)
	}
	index := int(p.end % historyBytes)
	n := copy(p.history[index:], data)
	copy(p.history, data[n:])
	p.end += int64(len(data))
	p.vt.update(data)
	if p.updates != nil && !p.lagging {
		p.sendUpdate(paneUpdate{output: append([]byte(nil), data...)})
	}
	// Preserve the connected client's reply ownership. Headless terminals
	// still answer queries when no live viewer can do so.
	replyHeadless := p.updates == nil || p.lagging
	failed := p.stateErr != nil
	p.mu.Unlock()
	if failed && p.process != nil {
		p.process.Kill()
		return
	}
	if replyHeadless && len(replies) > 0 {
		p.write(string(replies))
	}
}

func (p *pane) sendUpdate(update paneUpdate) {
	if p.updates == nil || p.lagging {
		return
	}
	select {
	case p.updates <- update:
	default:
		close(p.updates)
		p.lagging = true
		// Keep ownership until the old connection closes.
	}
}

var errPaneAttached = errors.New("pane already has an active terminal connection")

func (p *pane) attach(offset int64) (attachment, error) {
	return p.attachFor(offset, "")
}

func (p *pane) attachFor(offset int64, consumer string) (attachment, error) {
	p.mu.Lock()
	defer p.mu.Unlock()
	if p.updates != nil {
		return attachment{}, errPaneAttached
	}
	oldest := max(0, p.end-historyBytes)
	reset := offset < oldest || offset > p.end
	if reset {
		offset = oldest
	}
	replay := make([]byte, p.end-offset)
	n := copy(replay, p.history[int(offset%historyBytes):])
	copy(replay[n:], p.history)
	a := attachment{Offset: offset, Reset: reset, replay: replay, exited: p.finished, size: p.size}
	if !p.finished {
		a.live = make(chan paneUpdate, 32)
		p.updates, p.consumer = a.live, consumer
	}
	return a, nil
}

func (p *pane) detach(a attachment) {
	p.mu.Lock()
	defer p.mu.Unlock()
	if p.updates == a.live {
		p.updates, p.consumer = nil, ""
		p.lagging = false
	}
}

func (p *pane) ended() bool {
	p.mu.Lock()
	defer p.mu.Unlock()
	return p.finished && !p.lagging
}

func (p *pane) write(input string) error {
	p.inputMu.Lock()
	defer p.inputMu.Unlock()
	if err := p.file.SetWriteDeadline(time.Now().Add(2 * time.Second)); err != nil {
		return err
	}
	_, err := p.file.WriteString(input)
	return err
}

func (p *pane) resize(cols, rows int) error {
	return p.resizeCells(cols, rows, 0, 0)
}

func (p *pane) resizeCells(cols, rows, width, height int) error {
	p.mu.Lock()
	defer p.mu.Unlock()
	if width == 0 && height == 0 {
		width, height = int(p.size.X)/int(p.size.Cols), int(p.size.Y)/int(p.size.Rows)
	}
	size := pty.Winsize{Cols: uint16(cols), Rows: uint16(rows), X: uint16(min(cols*width, 65535)), Y: uint16(min(rows*height, 65535))}
	if p.size == size {
		return nil
	}
	if err := pty.Setsize(p.file, &size); err != nil {
		return err
	}
	if p.state != nil {
		if err := p.state.Resize(uint32(cols), uint32(rows), uint32(width), uint32(height)); err != nil {
			return err
		}
	}
	p.size = size
	p.sendUpdate(paneUpdate{size: &size})
	return nil
}

func (p *pane) currentSize() pty.Winsize {
	p.mu.Lock()
	defer p.mu.Unlock()
	return p.size
}

func (p *pane) canControl(id string) bool {
	p.mu.Lock()
	defer p.mu.Unlock()
	return p.updates == nil || p.consumer == id
}

func (p *pane) close() {
	p.mu.Lock()
	p.file.Close()
	if p.state != nil {
		p.state.Close()
	}
	p.blobs = nil
	p.mu.Unlock()
	p.process.Kill()
}

func (p *pane) exitStatus() int {
	p.mu.Lock()
	defer p.mu.Unlock()
	return p.exitCode
}

func (p *pane) capture(format string, trim, unwrap bool) ([]byte, error) {
	p.mu.Lock()
	defer p.mu.Unlock()
	if p.state == nil {
		return nil, &apiError{503, "terminal capture unavailable"}
	}
	if p.stateErr != nil {
		return nil, &apiError{503, "terminal state unavailable"}
	}
	return p.state.Capture(format, trim, unwrap)
}
func (p *pane) metadata() vt.Info {
	p.mu.Lock()
	defer p.mu.Unlock()
	if p.state == nil {
		return vt.Info{Records: []vt.Status{}, Prompts: []vt.Prompt{}}
	}
	return p.state.Info()
}
