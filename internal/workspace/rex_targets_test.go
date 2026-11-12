package workspace

import (
	"encoding/json"
	"testing"
)

func targetFixture() *rexServer {
	store := New("/bin/sh", Sandbox{})
	for _, id := range []string{"session:aaaa-1111", "session:bbbb-2222"} {
		p := &pane{id: "block:" + id}
		item := &session{ID: id, Label: "duplicate", panes: map[string]*pane{p.id: p}}
		item.addWindow(p, "terminal")
		store.sessions = append(store.sessions, item)
	}
	return &rexServer{store: store}
}
func TestResolveTargets(t *testing.T) {
	s := targetFixture()
	cases := []struct{ query, status string }{
		{"session:aaaa-1111", "resolved"}, {"1111", "resolved"}, {"duplicate", "ambiguous"}, {"missing", "not_found"}, {"", "resolved"},
	}
	for _, test := range cases {
		raw, _ := json.Marshal(fields{"kinds": []string{"session"}, "queries": []string{test.query}})
		result, err := s.resolveTargets(raw)
		if err != nil {
			t.Fatal(err)
		}
		got := result["results"].([]fields)[0]
		if got["status"] != test.status {
			t.Fatalf("query %q: %+v", test.query, got)
		}
	}
	raw := json.RawMessage(`{"kinds":["window"],"queries":["1"],"scope":{"session":"1111"}}`)
	r, err := s.resolveTargets(raw)
	if err != nil {
		t.Fatal(err)
	}
	target := r["results"].([]fields)[0]["target"].(fields)
	if target["session_id"] != "session:aaaa-1111" {
		t.Fatal(target)
	}
	if _, err = s.resolveTargets(json.RawMessage(`{"kinds":["block"],"queries":[""],"scope":{"session":"duplicate"}}`)); err == nil {
		t.Fatal("ambiguous scope accepted")
	}
	if _, err = s.resolveTargets(json.RawMessage(`{"kinds":["client"],"queries":[""]}`)); err == nil {
		t.Fatal("unsupported client kind accepted")
	}
}
func TestDescribeBlock(t *testing.T) {
	s := targetFixture()
	item := s.store.sessions[0]
	id := item.tabs[0].focused
	result, err := s.describeBlocks(rexArgs{Session: item.ID, Block: id}, true)
	if err != nil {
		t.Fatal(err)
	}
	if result["block"].(fields)["block_id"] != id {
		t.Fatal(result)
	}
	if _, err := s.describeBlocks(rexArgs{Session: item.ID, Block: "missing"}, true); err == nil {
		t.Fatal("missing block accepted")
	}
}
