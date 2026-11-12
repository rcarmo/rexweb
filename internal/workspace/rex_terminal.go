package workspace

import (
	"bytes"
	"crypto/rand"
	_ "embed"
	"encoding/base64"
	"encoding/binary"
	"encoding/json"
	"fmt"
	"net/http"
	"path/filepath"
	"strings"
	"time"

	"github.com/creack/pty"
	"github.com/gorilla/websocket"
)

const rexDataProtocol = "rex.data.terminal.v1"
const rexTicketTTL = 60 * time.Second

// A blank 80x24 Ghostty snapshot captured after resetting an idle /bin/cat block in
// Rex 0.1.0-preview.14. It contains no program output or filesystem paths.
//
//go:embed rex_blank.snapshot
var rexBlankSnapshot []byte

type rexTicket struct {
	pane    *pane
	client  *rexClient
	session string
	expires time.Time
}

func (s *rexServer) issueTicket(c *rexClient, session string, p *pane) (fields, error) {
	s.ticketMu.Lock()
	defer s.ticketMu.Unlock()
	now := time.Now()
	for id, ticket := range s.tickets {
		if now.After(ticket.expires) {
			delete(s.tickets, id)
			continue
		}
		select {
		case <-ticket.client.done:
			delete(s.tickets, id)
		default:
		}
	}
	if len(s.tickets) >= 256 {
		return nil, &rexError{"conflict", "too many outstanding data tickets", 409}
	}
	var id [16]byte
	rand.Read(id[:])
	s.tickets[id] = rexTicket{p, c, session, now.Add(rexTicketTTL)}
	return fields{"ticket": base64.RawURLEncoding.EncodeToString(id[:]), "expires_in_ms": rexTicketTTL.Milliseconds()}, nil
}

func (s *rexServer) consumeTicket(id [16]byte) (rexTicket, bool) {
	s.ticketMu.Lock()
	defer s.ticketMu.Unlock()
	ticket, ok := s.tickets[id]
	delete(s.tickets, id)
	if !ok || time.Now().After(ticket.expires) {
		return rexTicket{}, false
	}
	select {
	case <-ticket.client.done:
		return rexTicket{}, false
	default:
		return ticket, true
	}
}

func (s *rexServer) data(w http.ResponseWriter, r *http.Request) {
	select {
	case s.dataStreams <- struct{}{}:
		defer func() { <-s.dataStreams }()
	default:
		http.Error(w, "too many data connections", http.StatusServiceUnavailable)
		return
	}
	ws, err := upgradeRex(w, r, rexDataProtocol)
	if err != nil {
		return
	}
	defer ws.Close()
	ws.SetReadLimit(4096)
	ws.SetReadDeadline(time.Now().Add(5 * time.Second))
	kind, hello, err := ws.ReadMessage()
	if err != nil {
		return
	}
	if kind != websocket.BinaryMessage || len(hello) == 0 {
		rejectRexData(ws, 5)
		return
	}
	if hello[0] == 2 {
		rejectRexData(ws, 1) // The client must open a new stream with a new ticket.
		return
	}
	if hello[0] != 1 || len(hello) < 17 {
		rejectRexData(ws, 5)
		return
	}
	ticket, ok := s.consumeTicket([16]byte(hello[1:17]))
	if !ok || s.store.lookupPane(ticket.session, ticket.pane.id) != ticket.pane {
		rejectRexData(ws, 1)
		return
	}
	p := ticket.pane
	a, err := p.attachFor(-1, ticket.client.id)
	if err != nil {
		rejectRexData(ws, 2)
		return
	}
	defer p.detach(a)

	var welcome [32]byte
	welcome[0] = 0x81
	rand.Read(welcome[1:17])
	rand.Read(welcome[24:])
	nonce := binary.LittleEndian.Uint64(welcome[24:])
	if nonce == 0 {
		nonce = 1
		binary.LittleEndian.PutUint64(welcome[24:], nonce)
	}
	if writeBinary(ws, welcome[:]) != nil || writeBinary(ws, rexBlankSnapshot) != nil ||
		writeRexResize(ws, nonce, a.size, ticket.client.id) != nil {
		return
	}
	for start := 0; start < len(a.replay); start += 64 << 10 {
		if writeBinary(ws, a.replay[start:min(start+(64<<10), len(a.replay))]) != nil {
			return
		}
	}

	done := make(chan struct{})
	ws.SetReadDeadline(time.Now().Add(45 * time.Second))
	ws.SetPongHandler(func(string) error { return ws.SetReadDeadline(time.Now().Add(45 * time.Second)) })
	go func() {
		defer close(done)
		for {
			if _, _, err := ws.ReadMessage(); err != nil {
				return
			}
		}
	}()
	ping := time.NewTicker(15 * time.Second)
	defer ping.Stop()
	updates := a.live
	if a.exited && writeRexAPC(ws, nonce, fields{"type": "child_exited", "exit_code": p.exitStatus()}) != nil {
		return
	}
	for {
		select {
		case <-ticket.client.done:
			return
		case <-done:
			return
		case <-ping.C:
			if ws.WriteControl(websocket.PingMessage, nil, time.Now().Add(time.Second)) != nil {
				return
			}
		case update, ok := <-updates:
			if !ok {
				if !p.ended() || writeRexAPC(ws, nonce, fields{"type": "child_exited", "exit_code": p.exitStatus()}) != nil {
					return
				}
				updates = nil
				continue
			}
			if update.size != nil {
				if writeRexResize(ws, nonce, *update.size, ticket.client.id) != nil {
					return
				}
			} else if writeBinary(ws, update.output) != nil {
				return
			}
		}
	}
}

func rejectRexData(ws *websocket.Conn, reason byte) {
	writeBinary(ws, []byte{0x82, reason})
	ws.WriteControl(websocket.CloseMessage, websocket.FormatCloseMessage(1008, "data handshake rejected"), time.Now().Add(time.Second))
}

func writeRexAPC(ws *websocket.Conn, nonce uint64, message fields) error {
	body, err := json.Marshal(message)
	if err != nil {
		return err
	}
	return writeBinary(ws, fmt.Appendf(nil, "\x1b_rex1;%d;%s\x1b\\", nonce, body))
}

func writeRexResize(ws *websocket.Conn, nonce uint64, size pty.Winsize, owner string) error {
	return writeRexAPC(ws, nonce, fields{"type": "resize", "cols": size.Cols, "rows": size.Rows,
		"cell_width_px": size.X / size.Cols, "cell_height_px": size.Y / size.Rows, "owner": owner})
}

type rexKey struct {
	Key       string `json:"key"`
	Mods      string `json:"mods"`
	Consumed  string `json:"consumed_mods"`
	Text      string `json:"text"`
	Unshifted int    `json:"unshifted_codepoint"`
	Action    string `json:"action"`
	Composing bool   `json:"composing"`
}

func modifier(value, name string) bool {
	for _, mod := range strings.FieldsFunc(value, func(r rune) bool { return r == '+' || r == ',' || r == ' ' || r == '|' }) {
		if mod == name {
			return true
		}
	}
	return false
}

func (k rexKey) encode(cursorApplication bool) string {
	if k.Action == "release" || k.Composing || modifier(k.Mods, "super") {
		return ""
	}
	text := k.Text
	switch k.Key {
	case "enter", "numpad_enter":
		text = "\r"
	case "tab":
		text = "\t"
		if modifier(k.Mods, "shift") {
			text = "\x1b[Z"
		}
	case "backspace":
		text = "\x7f"
	case "escape":
		text = "\x1b"
	case "arrow_up", "arrow_down", "arrow_right", "arrow_left", "up", "down", "right", "left":
		final := map[string]string{"up": "A", "down": "B", "right": "C", "left": "D"}[strings.TrimPrefix(k.Key, "arrow_")]
		prefix := "\x1b["
		if cursorApplication {
			prefix = "\x1bO"
		}
		text = prefix + final
	case "home":
		text = "\x1b[H"
	case "end":
		text = "\x1b[F"
	case "insert":
		text = "\x1b[2~"
	case "delete":
		text = "\x1b[3~"
	case "page_up":
		text = "\x1b[5~"
	case "page_down":
		text = "\x1b[6~"
	}

	if modifier(k.Mods, "ctrl") && !modifier(k.Consumed, "ctrl") {
		code := k.Unshifted
		if code == 0 && len(k.Key) == 5 && strings.HasPrefix(k.Key, "key_") {
			code = int(k.Key[4])
		}
		if code >= 'a' && code <= 'z' {
			code -= 'a' - 'A'
		}
		switch {
		case code >= '@' && code <= '_':
			text = string(rune(code & 31))
		case code == ' ':
			text = "\x00"
		}
	}
	if text != "" && modifier(k.Mods, "alt") && !modifier(k.Consumed, "alt") {
		text = "\x1b" + text
	}
	return text
}

type terminalModes struct {
	cursor, paste, focus bool
	tail                 []byte
}

func (m *terminalModes) update(data []byte) {
	combined := append(m.tail, data...)
	check := func(mode string, value *bool) {
		on := bytes.LastIndex(combined, []byte("\x1b[?"+mode+"h"))
		off := bytes.LastIndex(combined, []byte("\x1b[?"+mode+"l"))
		if on >= 0 || off >= 0 {
			*value = on > off
		}
	}
	check("1", &m.cursor)
	check("2004", &m.paste)
	check("1004", &m.focus)
	m.tail = append(m.tail[:0], combined[max(0, len(combined)-7):]...)
}

func (p *pane) modes() terminalModes {
	p.mu.Lock()
	defer p.mu.Unlock()
	return terminalModes{cursor: p.vt.cursor, paste: p.vt.paste, focus: p.vt.focus}
}

func (s *rexServer) terminalCall(c *rexClient, method string, a rexArgs) (fields, error) {
	p := s.store.lookupRexPane(a.Session, a.Block)
	if p == nil {
		return nil, missing("pane")
	}
	send := func(input string) error {
		if input == "" {
			return nil
		}
		if !p.canControl(c.id) {
			return &rexError{"conflict", "pane is controlled by another terminal connection", 409}
		}
		return p.write(input)
	}
	switch method {
	case "client.key":
		input := a.Key.encode(p.modes().cursor)
		return fields{"handled": false}, send(input)
	case "block.event":
		return fields{}, s.blockEvent(p, a, send)
	}

	var args struct {
		Data   string `json:"data"`
		Cols   int    `json:"columns"`
		Rows   int    `json:"rows"`
		Width  int    `json:"cell_width_px"`
		Height int    `json:"cell_height_px"`
	}
	if len(a.Args) != 0 && json.Unmarshal(a.Args, &args) != nil {
		return nil, invalid("terminal method arguments")
	}
	switch a.Method {
	case "write":
		if len(args.Data) > 64<<10 {
			return nil, invalid("terminal input length")
		}
		return fields{}, send(args.Data)
	case "resize":
		if args.Cols < 2 || args.Cols > 500 || args.Rows < 1 || args.Rows > 500 ||
			args.Width < 0 || args.Width > 1024 || args.Height < 0 || args.Height > 1024 {
			return nil, invalid("terminal dimensions")
		}
		if !p.canControl(c.id) {
			return nil, &rexError{"conflict", "pane is controlled by another terminal connection", 409}
		}
		return fields{}, p.resizeCells(args.Cols, args.Rows, args.Width, args.Height)
	case "size":
		size := p.currentSize()
		return fields{"columns": size.Cols, "rows": size.Rows, "cell_width_px": size.X / size.Cols,
			"cell_height_px": size.Y / size.Rows, "desired": []fields{}}, nil
	case "input_mode":
		return p.inputMode()
	case "title":
		return fields{}, nil
	case "pwd":
		return fields{"pwd": s.store.sandbox.Root()}, nil
	case "process":
		child := fields{"pid": p.process.Pid, "name": filepath.Base(s.store.shell), "path": s.store.shell,
			"cwd": s.store.sandbox.Root()}
		return fields{"child": child, "foreground": child, "ancestors": []fields{}, "home": filepath.Join(s.store.sandbox.Root(), ".home")}, nil
	case "program_status":
		return fields{"records": []fields{}}, nil
	case "set_theme":
		return fields{}, nil // The native client draws with its selected theme.
	default:
		return nil, &rexError{"method_not_found", "unknown terminal method", 404}
	}
}

func (s *Store) lookupRexPane(sessionID, blockID string) *pane {
	s.mu.Lock()
	defer s.mu.Unlock()
	item := s.find(sessionID)
	if item == nil {
		return nil
	}
	if blockID == "" {
		for _, w := range item.tabs {
			if w.id == item.active {
				blockID = w.focused
				break
			}
		}
	}
	return item.panes[blockID]
}

func (s *rexServer) blockEvent(p *pane, a rexArgs, send func(string) error) error {
	switch a.Event.Type {
	case "key":
		var key rexKey
		if json.Unmarshal(a.Event.Payload, &key) != nil {
			return invalid("key event")
		}
		return send(key.encode(p.modes().cursor))
	case "paste":
		var paste struct{ Text string }
		if json.Unmarshal(a.Event.Payload, &paste) != nil || len(paste.Text) > 64<<10 {
			return invalid("paste event")
		}
		text := strings.Map(func(r rune) rune {
			if r == '\n' {
				return '\r'
			}
			if (r < 32 && r != '\t' && r != '\r') || r == 127 {
				return ' '
			}
			return r
		}, strings.ReplaceAll(paste.Text, "\r\n", "\n"))
		if text != "" && p.modes().paste {
			text = "\x1b[200~" + text + "\x1b[201~"
		}
		return send(text)
	case "focus":
		var focus struct{ Action string }
		if json.Unmarshal(a.Event.Payload, &focus) != nil || (focus.Action != "in" && focus.Action != "out") {
			return invalid("focus event")
		}
		if p.modes().focus {
			if focus.Action == "in" {
				return send("\x1b[I")
			}
			return send("\x1b[O")
		}
		return nil
	default:
		return unsupported("unsupported terminal event")
	}
}
