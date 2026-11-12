package workspace

import (
	"encoding/json"
	"strings"
	"testing"
)

func TestRexPaste(t *testing.T) {
	for _, test := range []struct {
		name, input, want string
		bracketed         bool
	}{
		{"plain Unicode", "hello λ😀", "hello λ😀", false},
		{"line endings", "one\r\ntwo\nthree\rfour", "one\rtwo\rthree\rfour", false},
		{"controls", "a\x03\x1b[201~\x00\x7fb\tc", "a  [201~  b\tc", false},
		{"bracketed", "hello\nworld", "\x1b[200~hello\rworld\x1b[201~", true},
		{"empty", "", "", true},
	} {
		t.Run(test.name, func(t *testing.T) {
			p := &pane{}
			p.vt.paste = test.bracketed
			var a rexArgs
			a.Event.Type = "paste"
			a.Event.Payload, _ = json.Marshal(fields{"text": test.input})
			got := ""
			calls := 0
			err := (&rexServer{}).blockEvent(p, a, func(text string) error { got = text; calls++; return nil })
			if err != nil || got != test.want || calls != 1 {
				t.Fatalf("paste = %q, calls=%d, err=%v; want %q", got, calls, err, test.want)
			}
		})
	}
	for _, payload := range []json.RawMessage{json.RawMessage(`{"text":2}`), json.RawMessage(`{`), func() json.RawMessage {
		b, _ := json.Marshal(fields{"text": strings.Repeat("x", (64<<10)+1)})
		return b
	}()} {
		var a rexArgs
		a.Event.Type = "paste"
		a.Event.Payload = payload
		if err := (&rexServer{}).blockEvent(&pane{}, a, func(string) error { t.Fatal("invalid paste was sent"); return nil }); err == nil {
			t.Fatal("invalid paste accepted")
		}
	}
}
