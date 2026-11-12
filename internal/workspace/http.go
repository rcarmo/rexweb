package workspace

import (
	"encoding/json"
	"errors"
	"io"
	"log"
	"net/http"
	"strconv"
	"time"

	"github.com/gorilla/websocket"
)

type apiError struct {
	status int
	msg    string
}

func (e *apiError) Error() string { return e.msg }

func missing(resource string) *apiError {
	return &apiError{http.StatusNotFound, resource + " not found"}
}

func invalid(field string) *apiError { return &apiError{http.StatusBadRequest, "invalid " + field} }

func respond(w http.ResponseWriter, result any, err error) {
	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("Cache-Control", "no-store")
	if err != nil {
		var api *apiError
		if !errors.As(err, &api) {
			log.Printf("workspace operation failed: %v", err)
			api = &apiError{http.StatusInternalServerError, "workspace operation failed"}
		}
		w.WriteHeader(api.status)
		result = fields{"message": api.msg}
	}
	json.NewEncoder(w).Encode(result)
}

// Handler supplies browser routes; the service applies authentication and Origin checks.
func (s *Store) Handler() http.Handler {
	mux := http.NewServeMux()
	mux.HandleFunc("GET /api/sessions", s.list)
	mux.HandleFunc("GET /api/session", s.view)
	mux.HandleFunc("POST /api/action", s.action)
	mux.HandleFunc("GET /api/terminal", s.terminal)
	return mux
}

func (s *Store) list(w http.ResponseWriter, _ *http.Request) {
	s.mu.Lock()
	list := make([]fields, 0, len(s.sessions))
	for _, item := range s.sessions {
		list = append(list, fields{"session_id": item.ID, "label": item.Label})
	}
	s.mu.Unlock()
	respond(w, fields{"sessions": list}, nil)
}

func (s *Store) view(w http.ResponseWriter, r *http.Request) {
	s.mu.Lock()
	item := s.find(r.URL.Query().Get("session"))
	var result fields
	if item != nil {
		result = item.view(s.hostID)
	}
	s.mu.Unlock()
	if result == nil {
		respond(w, nil, missing("session"))
		return
	}
	respond(w, result, nil)
}

func (s *Store) action(w http.ResponseWriter, r *http.Request) {
	var a action
	decoder := json.NewDecoder(http.MaxBytesReader(w, r.Body, 8<<10))
	decoder.DisallowUnknownFields()
	if err := decoder.Decode(&a); err != nil || decoder.Decode(new(any)) != io.EOF {
		respond(w, nil, invalid("action"))
		return
	}
	result, err := s.mutate(a)
	respond(w, result, err)
}

func (s *Store) lookupPane(sessionID, blockID string) *pane {
	s.mu.Lock()
	defer s.mu.Unlock()
	if item := s.find(sessionID); item != nil {
		return item.panes[blockID]
	}
	return nil
}

var upgrader = websocket.Upgrader{
	HandshakeTimeout: 5 * time.Second,
	CheckOrigin:      func(*http.Request) bool { return true }, // The gateway checks the browser origin.
}

func (s *Store) terminal(w http.ResponseWriter, r *http.Request) {
	p := s.lookupPane(r.URL.Query().Get("session"), r.URL.Query().Get("block"))
	if p == nil {
		respond(w, nil, missing("pane"))
		return
	}
	offset := int64(-1)
	if value := r.URL.Query().Get("offset"); value != "" {
		var err error
		offset, err = strconv.ParseInt(value, 10, 64)
		if err != nil || offset < 0 {
			respond(w, nil, invalid("stream offset"))
			return
		}
	}
	a, err := p.attach(offset)
	if err != nil {
		respond(w, nil, &apiError{http.StatusConflict, err.Error()})
		return
	}
	defer p.detach(a)

	ws, err := upgrader.Upgrade(w, r, nil)
	if err != nil {
		return
	}
	var done chan struct{}
	defer func() {
		ws.Close()
		if done != nil {
			<-done
		}
	}()
	ws.SetReadLimit(64 << 10)

	if writeJSON(ws, "stream", a) != nil {
		return
	}
	if len(a.replay) != 0 && writeBinary(ws, a.replay) != nil {
		return
	}
	if a.exited {
		sendExit(ws)
		return
	}
	if writeJSON(ws, "ready", nil) != nil {
		return
	}

	done = make(chan struct{})
	pongs := make(chan json.RawMessage, 1)
	go func() {
		defer close(done)
		p.readInput(ws, pongs)
	}()

	for {
		select {
		case <-done:
			return
		case pong := <-pongs:
			if writeJSON(ws, "pong", pong) != nil {
				return
			}
		case data, ok := <-a.live:
			if !ok {
				if p.ended() {
					sendExit(ws)
				}
				return
			}
			if len(data.output) != 0 && writeBinary(ws, data.output) != nil {
				return
			}
		}
	}
}

func (p *pane) readInput(ws *websocket.Conn, pongs chan<- json.RawMessage) {
	for {
		var message []json.RawMessage
		if ws.ReadJSON(&message) != nil || len(message) != 2 {
			return
		}
		var kind string
		if json.Unmarshal(message[0], &kind) != nil {
			return
		}
		switch kind {
		case "stdin":
			var input string
			if json.Unmarshal(message[1], &input) != nil || p.write(input) != nil {
				return
			}
		case "resize":
			var size struct{ Width, Height int }
			if json.Unmarshal(message[1], &size) != nil || size.Width < 2 || size.Width > 500 ||
				size.Height < 1 || size.Height > 500 || p.resize(size.Width, size.Height) != nil {
				return
			}
		case "ping":
			select {
			case pongs <- message[1]:
			default:
			}
		default:
			return
		}
	}
}

func writeJSON(ws *websocket.Conn, kind string, data any) error {
	ws.SetWriteDeadline(time.Now().Add(5 * time.Second))
	return ws.WriteJSON([]any{kind, data})
}

func writeBinary(ws *websocket.Conn, data []byte) error {
	ws.SetWriteDeadline(time.Now().Add(5 * time.Second))
	return ws.WriteMessage(websocket.BinaryMessage, data)
}

func sendExit(ws *websocket.Conn) {
	writeJSON(ws, "exit", nil)
	ws.WriteControl(websocket.CloseMessage, websocket.FormatCloseMessage(websocket.CloseNormalClosure, ""), time.Now().Add(time.Second))
}
