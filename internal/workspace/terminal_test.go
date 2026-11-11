package workspace

import (
	"bytes"
	"testing"
)

func TestReplayAfterHistoryWrap(t *testing.T) {
	p := &pane{history: make([]byte, historyBytes)}
	for i := range 33 {
		p.publish(bytes.Repeat([]byte{byte(i)}, 16<<10))
	}

	a, err := p.attach(0)
	if err != nil {
		t.Fatal(err)
	}
	if !a.Reset || a.Offset != 16<<10 || len(a.replay) != historyBytes {
		t.Fatalf("unexpected replay bounds: reset=%t offset=%d length=%d", a.Reset, a.Offset, len(a.replay))
	}
	for i, value := range a.replay {
		if want := byte(i/(16<<10) + 1); value != want {
			t.Fatalf("replay byte %d = %d, want %d", i, value, want)
		}
	}
}

func TestSlowViewerCanResume(t *testing.T) {
	p := &pane{history: make([]byte, historyBytes)}
	p.publish([]byte("before"))
	first, err := p.attach(-1)
	if err != nil {
		t.Fatal(err)
	}
	for range cap(first.live) + 2 {
		p.publish([]byte("x"))
	}
	if _, err := p.attach(6); err != errPaneAttached {
		t.Fatalf("second viewer: %v", err)
	}
	count := 0
	for data := range first.live {
		count += len(data.output)
	}
	p.detach(first)
	second, err := p.attach(6 + int64(count))
	if err != nil {
		t.Fatal(err)
	}
	if second.Reset || string(second.replay) != "xx" {
		t.Fatalf("unexpected resume: reset=%t replay=%q", second.Reset, second.replay)
	}
	p.publish([]byte("after"))
	select {
	case data := <-second.live:
		if string(data.output) != "after" {
			t.Fatalf("live output = %q", data.output)
		}
	default:
		t.Fatal("missing live output")
	}
}
