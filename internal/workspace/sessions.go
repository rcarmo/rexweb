// Package workspace runs the local PTY sessions and their browser API.
package workspace

import (
	"crypto/rand"
	"encoding/hex"
	"net/http"
	"slices"
	"strings"
	"sync"
)

type fields map[string]any

const (
	maxSessions = 32
	maxPanes    = 64
)

type Store struct {
	mu       sync.Mutex
	shell    string
	sandbox  Sandbox
	sessions []*session
	hostID   string
	watchers map[chan workspaceEvent]struct{}
}

type session struct {
	ID     string `json:"session_id"`
	Label  string `json:"label"`
	active string
	rev    uint64
	tabs   []*window
	panes  map[string]*pane
}

type window struct {
	id      string
	layerID string
	label   string
	focused string
	root    *node
}

type node struct {
	block, split, direction string
	ratio                   float64
	before, after           *node
}

func New(shell string, sandbox Sandbox) *Store {
	return &Store{shell: shell, sandbox: sandbox, hostID: uuid(), watchers: make(map[chan workspaceEvent]struct{})}
}

func newID(kind string) string { return kind + ":" + uuid() }

func uuid() string {
	var raw [16]byte
	rand.Read(raw[:])
	raw[6] = raw[6]&0x0f | 0x40
	raw[8] = raw[8]&0x3f | 0x80
	h := hex.EncodeToString(raw[:])
	return h[:8] + "-" + h[8:12] + "-" + h[12:16] + "-" + h[16:20] + "-" + h[20:]
}

func (s *Store) Close() {
	s.mu.Lock()
	defer s.mu.Unlock()
	for _, session := range s.sessions {
		for _, pane := range session.panes {
			pane.close()
		}
	}
	s.sessions = nil
}

func (s *Store) find(id string) *session {
	for _, item := range s.sessions {
		if item.ID == id {
			return item
		}
	}
	return nil
}

func (s *Store) newPane() (*pane, error) {
	count := 0
	for _, session := range s.sessions {
		count += len(session.panes)
	}
	if count >= maxPanes {
		return nil, &apiError{http.StatusConflict, "pane limit reached"}
	}
	return startPane(s.sandbox, s.shell)
}

func (s *session) addWindow(p *pane, name string) *window {
	if name == "" {
		name = "Terminal"
	}
	w := &window{id: newID("window"), layerID: newID("layer"), label: name, focused: p.id, root: &node{block: p.id}}
	s.tabs = append(s.tabs, w)
	s.active = w.id
	s.panes[p.id] = p
	return w
}

type action struct {
	Op          string  `json:"op"`
	Session     string  `json:"session"`
	Window      string  `json:"window"`
	Block       string  `json:"block"`
	Label       string  `json:"label"`
	Direction   string  `json:"direction"`
	Before      string  `json:"before"`
	After       string  `json:"after"`
	Ratio       float64 `json:"ratio"`
	Side        string  `json:"side"`
	Focus       *bool   `json:"focus"`
	WindowLabel string  `json:"window_label"`
}

func (s *Store) mutate(a action) (fields, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if a.Op == "ensure-default" {
		if len(s.sessions) > 0 {
			return s.sessions[0].summary(), nil
		}
		a.Op = "create"
	}
	result, err := s.mutateLocked(a)
	if err == nil {
		s.notifyMutation(a, result)
	}
	return result, err
}

func (s *Store) EnsureDefault() error {
	_, err := s.mutate(action{Op: "ensure-default"})
	return err
}

func (s *Store) mutateLocked(a action) (fields, error) {
	if a.Op == "create" {
		if len(s.sessions) >= maxSessions {
			return nil, &apiError{http.StatusConflict, "session limit reached"}
		}
		p, err := s.newPane()
		if err != nil {
			return nil, err
		}
		item := &session{ID: newID("session"), Label: label(a.Label), rev: 1, panes: make(map[string]*pane)}
		tab := item.addWindow(p, a.WindowLabel)
		s.sessions = append(s.sessions, item)
		return fields{"session_id": item.ID, "initial_windows": []fields{createdWindow(tab)}}, nil
	}
	item := s.find(a.Session)
	if item == nil {
		return nil, missing("session")
	}

	var result fields
	switch a.Op {
	case "rename":
		item.Label = label(a.Label)
	case "destroy":
		for index, current := range s.sessions {
			if current == item {
				s.sessions = slices.Delete(s.sessions, index, index+1)
				break
			}
		}
		for _, p := range item.panes {
			p.close()
		}
	case "new-window":
		p, err := s.newPane()
		if err != nil {
			return nil, err
		}
		active := item.active
		result = createdWindow(item.addWindow(p, a.WindowLabel))
		if active != "" && a.Focus != nil && !*a.Focus {
			item.active = active
		}
	case "focus-window", "close-window", "rename-window":
		index := slices.IndexFunc(item.tabs, func(w *window) bool { return w.id == a.Window })
		if index < 0 {
			return nil, missing("window")
		}
		w := item.tabs[index]
		switch a.Op {
		case "focus-window":
			item.active = w.id
		case "rename-window":
			w.label = label(a.Label)
		case "close-window":
			w.root.eachBlock(func(id string) {
				item.panes[id].close()
				delete(item.panes, id)
			})
			item.removeWindow(index)
		}
	case "focus-block", "close-block", "split":
		index, leaf := item.findPane(a.Block)
		if leaf == nil {
			return nil, missing("pane")
		}
		w := item.tabs[index]
		switch a.Op {
		case "focus-block":
			w.focused, item.active = a.Block, w.id
		case "close-block":
			item.panes[a.Block].close()
			delete(item.panes, a.Block)
			item.removePane(index, a.Block)
		case "split":
			ratio := a.Ratio
			if ratio == 0 {
				ratio = .5
			}
			if !validDirection(a.Direction) || (a.Side != "" && a.Side != "before" && a.Side != "after") || ratio <= 0 || ratio >= 1 {
				return nil, invalid("split direction, side or ratio")
			}
			p, err := s.newPane()
			if err != nil {
				return nil, err
			}
			item.panes[p.id] = p
			before, after := *leaf, &node{block: p.id}
			if a.Side == "before" {
				before, after = after, before
			}
			split := newID("split")
			*leaf = &node{split: split, direction: a.Direction, ratio: ratio, before: before, after: after}
			if a.Focus == nil || *a.Focus {
				w.focused, item.active = p.id, w.id
			}
			result = fields{"block_ids": []string{p.id}, "anchor_id": a.Block, "split_ids": []string{split}}
		}
	case "resize-split":
		if !validDirection(a.Direction) || a.Ratio <= 0 || a.Ratio >= 1 {
			return nil, invalid("split direction or ratio")
		}
		for _, w := range item.tabs {
			if w.root.resize(a) {
				item.rev++
				return fields{"revision": item.rev}, nil
			}
		}
		return nil, missing("split")
	default:
		return nil, invalid("operation")
	}
	item.rev++
	if result == nil {
		result = fields{}
	}
	result["revision"] = item.rev
	return result, nil
}

func label(value string) string {
	if name := strings.TrimSpace(value); name != "" {
		return name
	}
	return "Shell"
}

func validDirection(direction string) bool {
	return direction == "horizontal" || direction == "vertical"
}

func createdWindow(w *window) fields {
	return fields{"window_id": w.id, "block_ids": []string{w.focused}}
}

func (s *session) findPane(id string) (int, **node) {
	for i, w := range s.tabs {
		if leaf := findLeaf(&w.root, id); leaf != nil {
			return i, leaf
		}
	}
	return -1, nil
}

func findLeaf(root **node, id string) **node {
	if (*root).block != "" {
		if (*root).block == id {
			return root
		}
		return nil
	}
	if leaf := findLeaf(&(*root).before, id); leaf != nil {
		return leaf
	}
	return findLeaf(&(*root).after, id)
}

func (n *node) resize(a action) bool {
	if n.block != "" {
		return false
	}
	if n.direction == a.Direction && findLeaf(&n.before, a.Before) != nil && findLeaf(&n.after, a.After) != nil {
		n.ratio = a.Ratio
		return true
	}
	return n.before.resize(a) || n.after.resize(a)
}

func removeLeaf(n *node, id string) *node {
	if n.block == id {
		return nil
	}
	if n.block != "" {
		return n
	}
	n.before = removeLeaf(n.before, id)
	if n.before == nil {
		return n.after
	}
	n.after = removeLeaf(n.after, id)
	if n.after == nil {
		return n.before
	}
	return n
}

func (s *session) removePane(index int, id string) {
	w := s.tabs[index]
	w.root = removeLeaf(w.root, id)
	if w.root != nil {
		if w.focused == id {
			first := w.root
			for first.before != nil {
				first = first.before
			}
			w.focused = first.block
		}
		return
	}
	s.removeWindow(index)
}

func (s *session) removeWindow(index int) {
	w := s.tabs[index]
	s.tabs = slices.Delete(s.tabs, index, index+1)
	if s.active == w.id {
		s.active = ""
		if len(s.tabs) > 0 {
			s.active = s.tabs[min(index, len(s.tabs)-1)].id
		}
	}
}

func (n *node) eachBlock(fn func(string)) {
	if n.block != "" {
		fn(n.block)
		return
	}
	n.before.eachBlock(fn)
	n.after.eachBlock(fn)
}

type rect [4]float64

func (n *node) layout(r rect, blocks *[]fields) fields {
	if n.block != "" {
		*blocks = append(*blocks, fields{"block_id": n.block, "label": "shell"})
		return fields{"rect": r, "block_id": n.block}
	}
	before, after := r, r
	axis := 0
	if n.direction == "vertical" {
		axis = 1
	}
	before[axis+2] = r[axis+2] * n.ratio
	after[axis] += before[axis+2]
	after[axis+2] -= before[axis+2]
	return fields{"rect": r, "split": fields{
		"split_id": n.split, "direction": n.direction, "ratio": n.ratio,
		"before": n.before.layout(before, blocks), "after": n.after.layout(after, blocks),
	}}
}

func (s *session) view(host string) fields {
	tabs := make([]fields, 0, len(s.tabs))
	for _, w := range s.tabs {
		var blocks []fields
		layout := w.root.layout(rect{0, 0, 1, 1}, &blocks)
		for _, block := range blocks {
			p := s.panes[block["block_id"].(string)]
			block["creator_name"], block["flavor"], block["host_id"] = rexTerminalCreator, rexShellFlavor, host
			if p.owner != "" {
				block["owner"] = p.owner
			}
		}
		tabs = append(tabs, fields{"window_id": w.id, "label": w.label, "focused_block_id": w.focused,
			"layers": []fields{{"layer_id": w.layerID, "kind": "tiled", "bounds": rect{0, 0, 1, 1}, "layout": layout, "blocks": blocks}}})
	}
	return fields{"session_id": s.ID, "label": s.Label, "revision": s.rev, "active_window_id": s.active, "windows": tabs}
}
