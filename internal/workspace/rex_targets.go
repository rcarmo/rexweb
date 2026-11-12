package workspace

import (
	"encoding/json"
	"fmt"
	"slices"
	"strings"
)

// resolveTargets implements the public target.resolve shape, including ambiguity
// rather than silently selecting the first identically labelled session.
func (s *rexServer) resolveTargets(payload json.RawMessage) (fields, error) {
	var a struct {
		Kinds, Queries []string
		Context        struct {
			Session string `json:"session_id"`
			Block   string `json:"block_id"`
		}
		Scope struct{ Session, Window string }
	}
	if err := json.Unmarshal(payload, &a); err != nil {
		return nil, invalid("target query")
	}
	if len(a.Kinds) == 0 || len(a.Queries) > 128 {
		return nil, invalid("target kinds or query count")
	}
	for _, kind := range a.Kinds {
		if kind != "session" && kind != "window" && kind != "block" {
			return nil, unsupported("only session, window and block targets are supported")
		}
	}
	s.store.mu.Lock()
	defer s.store.mu.Unlock()
	match := func(candidates []fields, query string) []fields {
		// Match precedence: full ID, label, ordinal, ID suffix.
		for pass := 0; pass < 4; pass++ {
			out := []fields{}
			for _, c := range candidates {
				id := fmt.Sprint(c[c["kind"].(string)+"_id"])
				yes := false
				switch pass {
				case 0:
					yes = id == query
				case 1:
					yes = query != "" && strings.EqualFold(fmt.Sprint(c["label"]), query)
				case 2:
					yes = c["kind"] == "window" && fmt.Sprint(c["ordinal"]) == query
				case 3:
					yes = len(query) >= 4 && strings.HasSuffix(id, query)
				}
				if yes {
					out = append(out, c)
				}
			}
			if len(out) > 0 {
				return out
			}
		}
		return nil
	}
	sessions := []fields{}
	for _, item := range s.store.sessions {
		sessions = append(sessions, fields{"kind": "session", "session_id": item.ID, "label": item.Label})
	}
	selected := a.Scope.Session
	if selected != "" {
		found := match(sessions, selected)
		if len(found) != 1 {
			return nil, invalid("session scope (not found or ambiguous)")
		}
		selected = found[0]["session_id"].(string)
	} else {
		selected = a.Context.Session
	}
	if selected == "" && len(s.store.sessions) > 0 {
		selected = s.store.sessions[0].ID
	}
	candidates := []fields{}
	if slices.Contains(a.Kinds, "session") {
		candidates = append(candidates, sessions...)
	}
	for _, item := range s.store.sessions {
		if item.ID != selected {
			continue
		}
		windows := []fields{}
		for i, w := range item.tabs {
			windows = append(windows, fields{"kind": "window", "session_id": item.ID, "window_id": w.id, "label": w.label, "ordinal": i + 1})
		}
		windowID := ""
		if a.Scope.Window != "" {
			found := match(windows, a.Scope.Window)
			if len(found) != 1 {
				return nil, invalid("window scope")
			}
			windowID = found[0]["window_id"].(string)
		}
		if slices.Contains(a.Kinds, "window") {
			candidates = append(candidates, windows...)
		}
		if slices.Contains(a.Kinds, "block") {
			for _, w := range item.tabs {
				if windowID != "" && w.id != windowID {
					continue
				}
				w.root.eachBlock(func(id string) {
					candidates = append(candidates, fields{"kind": "block", "session_id": item.ID, "window_id": w.id, "block_id": id, "label": "shell"})
				})
			}
		}
	}
	results := []fields{}
	for _, q := range a.Queries {
		query := q
		if query == "" {
			if len(a.Kinds) == 1 && a.Kinds[0] == "session" {
				query = selected
			} else {
				query = a.Context.Block
				if query == "" || len(a.Kinds) == 1 && a.Kinds[0] == "window" {
					if item := s.store.find(selected); item != nil {
						for _, w := range item.tabs {
							if w.id == item.active {
								if len(a.Kinds) == 1 && a.Kinds[0] == "window" {
									query = w.id
								} else {
									query = w.focused
								}
								break
							}
						}
					}
				}
			}
		}
		found := match(candidates, query)
		r := fields{"query": q, "status": "not_found"}
		if len(found) == 1 {
			r["status"], r["target"] = "resolved", found[0]
		} else if len(found) > 1 {
			r["status"], r["candidates"] = "ambiguous", found
		}
		results = append(results, r)
	}
	return fields{"results": results}, nil
}

func terminalAPI() fields {
	methods := fields{}
	for _, name := range []string{"write", "resize", "size", "input_mode", "title", "pwd", "process", "program_status", "set_theme"} {
		methods[name] = fields{"description": "Terminal " + name}
	}
	methods["write"] = fields{"description": "Type text into the PTY.", "input_schema": fields{"type": "object", "properties": fields{"data": fields{"type": "string"}}, "required": []string{"data"}}}
	return fields{"methods": methods, "events": fields{"paste": fields{"type": "object", "properties": fields{"text": fields{"type": "string"}}}, "key": fields{"type": "object"}, "focus": fields{"type": "object"}}}
}
func (s *rexServer) describeBlocks(a rexArgs, single bool) (fields, error) {
	s.store.mu.Lock()
	defer s.store.mu.Unlock()
	item := s.store.find(a.Session)
	if item == nil {
		return nil, missing("session")
	}
	blocks := []fields{}
	for _, w := range item.tabs {
		w.root.eachBlock(func(id string) {
			if single && a.Block != id {
				return
			}
			block := fields{"block_id": id, "label": "shell", "creator_name": rexTerminalCreator, "flavor": rexShellFlavor, "host_id": s.store.hostID, "placement": "placed", "window_id": w.id}
			if owner := item.panes[id].owner; owner != "" {
				block["owner"] = owner
			}
			blocks = append(blocks, block)
		})
	}
	if single {
		if len(blocks) == 0 {
			return nil, missing("pane")
		}
		return fields{"block": blocks[0], "api": terminalAPI()}, nil
	}
	return fields{"blocks": blocks}, nil
}
