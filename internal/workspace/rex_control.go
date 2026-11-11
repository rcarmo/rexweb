package workspace

import (
	"encoding/json"
	"errors"
	"net/http"
	"os"
	"path/filepath"
	"runtime"
	"slices"
	"strings"
	"sync"
	"time"

	"github.com/gorilla/websocket"
)

const (
	rexRevision        = 12
	rexControlProtocol = "rex.control.v1"
	rexTerminalCreator = "com.superlogical.terminal"
	rexShellFlavor     = "com.superlogical.terminal.shell"
)

type rexServer struct {
	store       *Store
	instance    string
	started     time.Time
	ticketMu    sync.Mutex
	tickets     map[[16]byte]rexTicket
	controls    chan struct{}
	dataStreams chan struct{}
}

type rexClient struct {
	id       string
	done     chan struct{}
	all      bool
	attached map[string]bool
}

type workspaceEvent struct {
	session string
	body    fields
}

func (s *Store) RexHandler() http.Handler {
	server := &rexServer{
		store: s, instance: uuid(), started: time.Now().UTC(),
		tickets: make(map[[16]byte]rexTicket), controls: make(chan struct{}, 32), dataStreams: make(chan struct{}, 96),
	}
	mux := http.NewServeMux()
	mux.HandleFunc("GET /api/control", server.control)
	mux.HandleFunc("GET /api/data", server.data)
	return mux
}

func eventBody(name string, data any) fields {
	return fields{"type": name, "payload": data}
}

func (s *Store) observe() (chan workspaceEvent, func()) {
	s.mu.Lock()
	defer s.mu.Unlock()
	ch := make(chan workspaceEvent, 128)
	s.watchers[ch] = struct{}{}
	return ch, func() {
		s.mu.Lock()
		defer s.mu.Unlock()
		delete(s.watchers, ch)
	}
}

// notify is called with Store.mu held.
func (s *Store) notify(e workspaceEvent) {
	for ch := range s.watchers {
		select {
		case ch <- e:
		default:
			close(ch)
			delete(s.watchers, ch)
		}
	}
}

func (s *Store) sessionEvent(id, kind string, payload fields) {
	s.notify(workspaceEvent{id, eventBody("session_event", fields{"session_id": id, "event": eventBody(kind, payload)})})
}

func (s *Store) notifyMutation(a action, reply fields) {
	switch a.Op {
	case "create":
		id := reply["session_id"].(string)
		s.notify(workspaceEvent{id, eventBody("session_created", fields{"session": s.find(id).summary()})})
	case "destroy":
		s.notify(workspaceEvent{a.Session, eventBody("session_destroyed", fields{"session_id": a.Session})})
	default:
		if a.Op == "rename" {
			s.sessionEvent(a.Session, "session_label_changed", fields{"label": s.find(a.Session).Label})
		}
		s.sessionEvent(a.Session, "session_view_changed", fields{"revision": reply["revision"]})
	}
}

func rexUpgrader(protocol string) websocket.Upgrader {
	return websocket.Upgrader{
		Subprotocols: []string{protocol}, HandshakeTimeout: 5 * time.Second,
		CheckOrigin: func(r *http.Request) bool { return r.Header.Get("Origin") == "" },
	}
}

func upgradeRex(w http.ResponseWriter, r *http.Request, protocol string) (*websocket.Conn, error) {
	if !slices.Contains(websocket.Subprotocols(r), protocol) {
		http.Error(w, "required Rex subprotocol missing", http.StatusBadRequest)
		return nil, errors.New("required Rex subprotocol missing")
	}
	upgrader := rexUpgrader(protocol)
	return upgrader.Upgrade(w, r, nil)
}

type rexRequest struct {
	Type    string          `json:"type"`
	ID      string          `json:"id"`
	Method  string          `json:"method"`
	Payload json.RawMessage `json:"payload"`
}

func (s *rexServer) control(w http.ResponseWriter, r *http.Request) {
	select {
	case s.controls <- struct{}{}:
		defer func() { <-s.controls }()
	default:
		http.Error(w, "too many control connections", http.StatusServiceUnavailable)
		return
	}
	ws, err := upgradeRex(w, r, rexControlProtocol)
	if err != nil {
		return
	}
	defer ws.Close()
	ws.SetReadLimit(1 << 20)
	ws.SetReadDeadline(time.Now().Add(5 * time.Second))
	var hello struct {
		Type string `json:"type"`
		Max  int    `json:"api_revision"`
		Min  int    `json:"api_min_revision"`
	}
	if ws.ReadJSON(&hello) != nil || hello.Type != "hello" {
		return
	}
	if hello.Max < rexRevision || hello.Min > rexRevision {
		ws.WriteControl(websocket.CloseMessage, websocket.FormatCloseMessage(1008, "API revision 12 required"), time.Now().Add(time.Second))
		return
	}

	client := &rexClient{id: newID("client"), done: make(chan struct{}), attached: make(map[string]bool)}
	defer s.release(client, "")
	defer close(client.done)
	events, unsubscribe := s.store.observe()
	defer unsubscribe()
	if writeRex(ws, fields{
		"type": "welcome", "protocol": rexControlProtocol, "limits": fields{"max_message_bytes": 1 << 20},
		"api_revision": rexRevision, "api_server_revision": rexRevision, "api_min_revision": rexRevision,
		"client": fields{"id": client.id, "reconnect_token": uuid(), "resumed": false},
	}) != nil {
		return
	}

	requests := make(chan rexRequest, 16)
	readDone := make(chan struct{})
	ws.SetReadDeadline(time.Now().Add(45 * time.Second))
	ws.SetPongHandler(func(string) error { return ws.SetReadDeadline(time.Now().Add(45 * time.Second)) })
	go func() {
		defer close(readDone)
		for {
			var request rexRequest
			if ws.ReadJSON(&request) != nil {
				return
			}
			select {
			case requests <- request:
			case <-client.done:
				return
			}
		}
	}()

	ping := time.NewTicker(15 * time.Second)
	defer ping.Stop()
	var seq uint64
	for {
		select {
		case <-readDone:
			return
		case <-ping.C:
			if ws.WriteControl(websocket.PingMessage, nil, time.Now().Add(time.Second)) != nil {
				return
			}
		case event, ok := <-events:
			if !ok {
				return
			}
			if client.all || client.attached[event.session] {
				seq++
				if writeRex(ws, fields{"type": "event", "seq": seq, "payload": event.body}) != nil {
					return
				}
			}
		case request := <-requests:
			if request.Type != "request" || request.ID == "" {
				return
			}
			result, err := s.call(client, request.Method, request.Payload)
			reply := fields{"type": "response", "id": request.ID, "seq": seq}
			if err == nil {
				reply["payload"] = result
			} else {
				reply["error"] = rexErrorFields(err)
			}
			if writeRex(ws, reply) != nil {
				return
			}
		}
	}
}

func writeRex(ws *websocket.Conn, payload any) error {
	ws.SetWriteDeadline(time.Now().Add(5 * time.Second))
	return ws.WriteJSON(payload)
}

type rexError struct {
	code, message string
	status        int
}

func (e *rexError) Error() string { return e.message }

func rexErrorFields(err error) fields {
	var wire *rexError
	if errors.As(err, &wire) {
		return fields{"code": wire.code, "message": wire.message, "status": wire.status}
	}
	var api *apiError
	if errors.As(err, &api) {
		code := map[int]string{400: "bad_request", 404: "not_found", 409: "conflict"}[api.status]
		if api.msg == "session not found" {
			code = "session_not_found"
		}
		return fields{"code": code, "message": api.msg, "status": api.status}
	}
	return fields{"code": "internal", "message": "workspace operation failed", "status": 500}
}

func (s *session) summary() fields { return fields{"session_id": s.ID, "label": s.Label} }

func (s *Store) summaries() []fields {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.summariesLocked()
}

func (s *Store) summariesLocked() []fields {
	list := make([]fields, 0, len(s.sessions))
	for _, item := range s.sessions {
		list = append(list, item.summary())
	}
	return list
}

func (s *rexServer) status(count int) fields {
	return fields{
		"pid": os.Getpid(), "instance_id": s.instance, "host_id": s.store.hostID, "started_at": s.started,
		"exit_empty": false, "session_count": count, "api_revision": rexRevision, "api_min_revision": rexRevision,
	}
}

type rexArgs struct {
	Session     string          `json:"session_id"`
	Block       string          `json:"block_id"`
	Blocks      []string        `json:"block_ids"`
	Window      string          `json:"window_id"`
	Label       string          `json:"label"`
	WindowLabel string          `json:"window_label"`
	Anchor      string          `json:"anchor_id"`
	Direction   string          `json:"direction"`
	Side        string          `json:"side"`
	Before      string          `json:"before_block_id"`
	After       string          `json:"after_block_id"`
	Ratio       float64         `json:"ratio"`
	Focus       *bool           `json:"focus"`
	Mode        string          `json:"mode"`
	Method      string          `json:"method"`
	Args        json.RawMessage `json:"args"`
	Event       struct {
		Type    string          `json:"type"`
		Payload json.RawMessage `json:"payload"`
	} `json:"event"`
	Key         rexKey          `json:"key"`
	EventScope  string          `json:"event_scope"`
	Preferred   string          `json:"preferred_session_id"`
	AttachIDs   *[]string       `json:"attach_session_ids"`
	DataTickets []rexDataSource `json:"data_tickets"`
	DataRef     string          `json:"data_ref_id"`
	Subprotocol string          `json:"subprotocol"`
	Layout      rexLayout       `json:"layout"`
	Initial     []struct {
		WindowLabel string    `json:"window_label"`
		Layout      rexLayout `json:"layout"`
	} `json:"initial_windows"`
}

type rexLayout struct {
	Block *struct {
		Flavor  string `json:"flavor"`
		Options struct {
			Command []string `json:"command"`
			Input   string   `json:"initial_input"`
		} `json:"options"`
	} `json:"block"`
	Split json.RawMessage `json:"split"`
}

func (l rexLayout) supported() bool {
	return len(l.Split) == 0 && (l.Block == nil ||
		(l.Block.Flavor == rexShellFlavor && len(l.Block.Options.Command) == 0 && l.Block.Options.Input == ""))
}

type rexDataSource struct {
	Creator     string `json:"creator_name"`
	Ref         string `json:"data_ref_id"`
	Subprotocol string `json:"subprotocol"`
}

func (s *rexServer) call(c *rexClient, method string, payload json.RawMessage) (fields, error) {
	var a rexArgs
	if len(payload) != 0 && json.Unmarshal(payload, &a) != nil {
		return nil, invalid("arguments")
	}
	switch method {
	case "server.version":
		return fields{"rex_version": "0.1.0-rexweb.1", "platform": runtime.GOOS + "_" + runtime.GOARCH,
			"api_revision": rexRevision, "api_min_revision": rexRevision, "dirty": false}, nil
	case "server.status":
		return s.status(len(s.store.summaries())), nil
	case "server.whoami":
		return fields{"principal": fields{"kind": "anonymous", "transport": "tcp"}}, nil
	case "hosts.list":
		return fields{"hosts": []fields{}}, nil
	case "client.keymap":
		return s.keymap(payload)
	case "block_creator.list":
		return fields{"creators": []fields{s.creator()}}, nil
	case "session.list":
		return fields{"sessions": s.store.summaries()}, nil
	case "session.attach_all", "session.bootstrap":
		if a.EventScope != "session" && a.EventScope != "all" {
			return nil, invalid("event scope")
		}
		c.all = true
		if method == "session.bootstrap" {
			return s.bootstrap(c, a), nil
		}
		return fields{"sessions": s.store.summaries()}, nil
	case "session.attach", "session.view":
		return s.view(c, a.Session, method == "session.attach")
	case "session.detach":
		delete(c.attached, a.Session)
		s.release(c, a.Session)
		return fields{}, nil
	case "session.create":
		if len(a.Initial) > 1 || (len(a.Initial) == 1 && !a.Initial[0].Layout.supported()) {
			return nil, unsupported("only one initial shell tab is supported")
		}
		if len(a.Initial) == 1 {
			a.WindowLabel = a.Initial[0].WindowLabel
		}
		return s.store.mutate(action{Op: "create", Label: a.Label, WindowLabel: a.WindowLabel})
	case "client.set_label":
		return fields{}, nil
	}
	if !c.all && !c.attached[a.Session] && a.Session != "" {
		return nil, &rexError{"conflict", "attach to the session first", 409}
	}

	switch method {
	case "session.destroy", "session.set_label", "session.set_window_label", "session.new_window",
		"session.focus_window", "session.focus_block", "session.close_window", "block.close",
		"session.new_split", "session.resize_pane":
		return s.mutate(method, a)
	case "data.connect":
		if !c.attached[a.Session] {
			return nil, &rexError{"conflict", "attach to the session first", 409}
		}
		if a.DataRef != "terminal" || (a.Subprotocol != "" && a.Subprotocol != rexDataProtocol) {
			return nil, invalid("terminal data source or subprotocol")
		}
		p := s.store.lookupPane(a.Session, a.Block)
		if p == nil {
			return nil, missing("pane")
		}
		return s.issueTicket(c, a.Session, p)
	case "session.own_block", "session.disown_block":
		if !c.attached[a.Session] {
			return nil, &rexError{"conflict", "attach to the session first", 409}
		}
		return s.ownership(c, a, method == "session.own_block")
	case "client.key", "block.event", "block.method":
		return s.terminalCall(c, method, a)
	default:
		if strings.HasPrefix(method, rexTerminalCreator+".") {
			a.Method = strings.TrimPrefix(method, rexTerminalCreator+".")
			return s.terminalCall(c, "block.method", a)
		}
		return nil, &rexError{"method_not_found", "unknown Rex method", 404}
	}
}

func unsupported(message string) *rexError { return &rexError{"not_supported", message, 400} }

func (s *rexServer) mutate(method string, a rexArgs) (fields, error) {
	ops := map[string]string{
		"session.destroy": "destroy", "session.set_label": "rename", "session.set_window_label": "rename-window",
		"session.new_window": "new-window", "session.focus_window": "focus-window", "session.focus_block": "focus-block",
		"session.close_window": "close-window", "block.close": "close-block", "session.new_split": "split", "session.resize_pane": "resize-split",
	}
	if (method == "session.new_window" || method == "session.new_split") && !a.Layout.supported() {
		return nil, unsupported("only a single interactive shell can be opened")
	}
	s.store.mu.Lock()
	item := s.store.find(a.Session)
	if item == nil {
		s.store.mu.Unlock()
		return nil, missing("session")
	}
	windowID := a.Window
	if windowID == "" {
		windowID = item.active
	}
	blockID := a.Block
	if method == "session.new_split" {
		blockID = a.Anchor
	}
	if blockID == "" {
		for _, w := range item.tabs {
			if w.id == windowID {
				blockID = w.focused
				break
			}
		}
	}
	s.store.mu.Unlock()
	return s.store.mutate(action{
		Op: ops[method], Session: a.Session, Window: windowID, Block: blockID,
		Label: a.Label, WindowLabel: a.WindowLabel, Side: a.Side, Focus: a.Focus,
		Direction: a.Direction, Before: a.Before, After: a.After, Ratio: a.Ratio,
	})
}

func (s *rexServer) view(c *rexClient, id string, attach bool) (fields, error) {
	s.store.mu.Lock()
	defer s.store.mu.Unlock()
	item := s.store.find(id)
	if item == nil {
		return nil, missing("session")
	}
	view := item.view(s.store.hostID)
	if attach {
		c.attached[id] = true
		return fields{"client_id": c.id, "view": view}, nil
	}
	if !c.all && !c.attached[id] {
		return nil, &rexError{"conflict", "attach to the session first", 409}
	}
	return view, nil
}

func (s *rexServer) bootstrap(c *rexClient, a rexArgs) fields {
	s.store.mu.Lock()
	defer s.store.mu.Unlock()
	list := s.store.summariesLocked()
	result := fields{
		"client_id": c.id, "server_status": s.status(len(list)), "attach_all": fields{"sessions": list},
		"selected_session": nil, "attached_sessions": []fields{}, "data_tickets": []fields{},
	}
	var attached, tickets []fields
	attach := func(item *session) fields {
		c.attached[item.ID] = true
		for _, source := range a.DataTickets {
			if source.Creator != rexTerminalCreator || source.Ref != "terminal" ||
				(source.Subprotocol != "" && source.Subprotocol != rexDataProtocol) {
				continue
			}
			for _, p := range item.panes {
				if ticket, err := s.issueTicket(c, item.ID, p); err == nil {
					ticket["session_id"], ticket["block_id"], ticket["data_ref_id"] = item.ID, p.id, source.Ref
					tickets = append(tickets, ticket)
				}
			}
			break
		}
		return fields{"session": item.summary(), "view": item.view(s.store.hostID)}
	}
	if a.AttachIDs != nil {
		for _, id := range *a.AttachIDs {
			if item := s.store.find(id); item != nil && !c.attached[id] {
				attached = append(attached, attach(item))
			}
		}
		if len(attached) != 0 {
			result["attached_sessions"] = attached
		}
	} else if len(s.store.sessions) != 0 {
		item := s.store.find(a.Preferred)
		if item == nil {
			item = s.store.sessions[0]
		}
		result["selected_session"] = attach(item)
	}
	if len(tickets) != 0 {
		result["data_tickets"] = tickets
	}
	return result
}

func (s *rexServer) ownership(c *rexClient, a rexArgs, own bool) (fields, error) {
	s.store.mu.Lock()
	defer s.store.mu.Unlock()
	item := s.store.find(a.Session)
	if item == nil {
		return nil, missing("session")
	}
	ids := a.Blocks
	if len(ids) == 0 {
		for id := range item.panes {
			ids = append(ids, id)
		}
		slices.Sort(ids)
	}
	for _, id := range ids {
		if item.panes[id] == nil {
			return nil, missing("pane")
		}
	}
	blocks := make([]fields, 0, len(ids))
	changed := false
	for _, id := range ids {
		p := item.panes[id]
		previous := p.owner
		if own && (a.Mode != "request" || p.owner == "") {
			p.owner = c.id
		}
		if !own && p.owner == c.id {
			p.owner = ""
		}
		changed = changed || previous != p.owner
		block := fields{"block_id": id, "owner": nil}
		if p.owner != "" {
			block["owner"] = p.owner
		}
		blocks = append(blocks, block)
	}
	if changed {
		item.rev++
		s.store.sessionEvent(item.ID, "session_view_changed", fields{"revision": item.rev})
	}
	return fields{"blocks": blocks}, nil
}

func (s *rexServer) release(c *rexClient, id string) {
	s.store.mu.Lock()
	defer s.store.mu.Unlock()
	for _, item := range s.store.sessions {
		if id != "" && id != item.ID {
			continue
		}
		changed := false
		for _, p := range item.panes {
			if p.owner == c.id {
				p.owner, changed = "", true
			}
		}
		if changed {
			item.rev++
			s.store.sessionEvent(item.ID, "session_view_changed", fields{"revision": item.rev})
		}
	}
}

func (s *rexServer) creator() fields {
	return fields{
		"name": rexTerminalCreator, "title": "Terminal", "description": "An interactive shell in the workspace", "host_id": s.store.hostID,
		"flavors": []fields{{"name": "shell", "flavor": rexShellFlavor, "title": "Shell",
			"description": "An interactive shell", "options_schema": fields{"type": "object"}}},
		"api": fields{"methods": fields{}, "events": fields{}},
	}
}

type keyBinding struct {
	Keys   []string        `json:"keys"`
	Action string          `json:"action"`
	Args   json.RawMessage `json:"args,omitempty"`
	Mode   string          `json:"mode,omitempty"`
	Source string          `json:"source,omitempty"`
}

func (s *rexServer) keymap(payload json.RawMessage) (fields, error) {
	type layer struct {
		Name     string       `json:"name"`
		Bindings []keyBinding `json:"bindings"`
		Removals []keyBinding `json:"removals"`
	}
	var request struct {
		Defaults  []layer `json:"defaults"`
		Overrides []layer `json:"overrides"`
	}
	if len(payload) != 0 && json.Unmarshal(payload, &request) != nil {
		return nil, invalid("keymap")
	}
	bindings := []keyBinding{}
	names := make(map[string]bool)
	for _, layer := range append(request.Defaults, request.Overrides...) {
		if layer.Name == "" || strings.Contains(layer.Name, ":") || names[layer.Name] {
			return nil, invalid("keymap layer name")
		}
		names[layer.Name] = true
		for _, removal := range layer.Removals {
			bindings = slices.DeleteFunc(bindings, func(binding keyBinding) bool {
				return binding.Mode == removal.Mode &&
					(removal.Action == "" || removal.Action == binding.Action) &&
					(slices.Equal(binding.Keys, removal.Keys) || (removal.Mode != "" && len(removal.Keys) == 0))
			})
		}
		for _, binding := range layer.Bindings {
			binding.Source = layer.Name
			bindings = append(bindings, binding)
		}
	}
	slices.Reverse(bindings)
	return fields{
		"bindings": bindings, "diagnostics": []fields{},
		"config": fields{"path": filepath.Join(s.store.sandbox.Root(), ".home", "init.lua"), "exists": false,
			"generation": 1, "instance_id": s.instance, "hash": ""},
	}, nil
}
