package workspace

import (
	"encoding/json"
	"errors"
	"time"
	"unicode/utf8"

	"rexweb/internal/vt"
)

const nativeJSONBudget = 960 << 10
const nativeBlobBudget = 4 << 20

type captureBlob struct {
	data    []byte
	expires time.Time
}

// Native Rex paginates capture by encoded JSON size rather than raw text bytes.
func jsonPrefix(data []byte, budget int) int {
	if budget < 8 {
		return 0
	}
	low, high, best := 0, min(len(data), budget), 0
	for low <= high {
		middle := low + (high-low)/2
		end := middle
		for end > 0 && end < len(data) && !utf8.RuneStart(data[end]) {
			end--
		}
		encoded, _ := json.Marshal(string(data[:end]))
		if len(encoded) <= budget {
			best = end
			low = middle + 1
		} else {
			high = middle - 1
		}
	}
	return best
}
func (p *pane) nativeCapture(format string, trim, unwrap bool, maxBytes int) (fields, error) {
	data, err := p.capture(format, trim, unwrap)
	if errors.Is(err, vt.ErrCaptureLimit) {
		return nil, &rexError{"too_large", "capture exceeds 4 MiB", 413}
	}
	if err != nil {
		return nil, err
	}
	encoded, _ := json.Marshal(string(data))
	if maxBytes == 0 {
		if len(encoded) > nativeJSONBudget {
			return nil, &rexError{"too_large", "set max_bytes to read this capture in chunks", 413}
		}
		return fields{"content": string(data)}, nil
	}
	limit := min(maxBytes, nativeJSONBudget)
	end := jsonPrefix(data, limit)
	if end == 0 && len(data) > 0 {
		return nil, invalid("max_bytes (too small)")
	}
	reply := fields{"content": string(data[:end]), "total": len(data), "eof": end == len(data)}
	if end < len(data) {
		p.mu.Lock()
		defer p.mu.Unlock()
		if p.blobs == nil {
			p.blobs = make(map[uint32]captureBlob)
		}
		// A pane owns at most 4 MiB of short-lived export data.
		retained := 0
		for id, blob := range p.blobs {
			if time.Now().After(blob.expires) {
				delete(p.blobs, id)
			} else {
				retained += len(blob.data)
			}
		}
		for retained+len(data) > nativeBlobBudget {
			for id, blob := range p.blobs {
				delete(p.blobs, id)
				retained -= len(blob.data)
				break
			}
		}
		p.nextBlob++
		p.blobs[p.nextBlob] = captureBlob{data, time.Now().Add(5 * time.Minute)}
		reply["id"] = p.nextBlob
	}
	return reply, nil
}
func (p *pane) nativeBlob(id uint32, offset, maxBytes int) (fields, error) {
	p.mu.Lock()
	defer p.mu.Unlock()
	blob, ok := p.blobs[id]
	if !ok || time.Now().After(blob.expires) {
		delete(p.blobs, id)
		return nil, missing("capture blob")
	}
	if offset < 0 || offset > len(blob.data) || (offset < len(blob.data) && !utf8.RuneStart(blob.data[offset])) {
		return nil, invalid("blob offset")
	}
	limit := nativeJSONBudget
	if maxBytes > 0 {
		limit = min(maxBytes, limit)
	}
	end := offset + jsonPrefix(blob.data[offset:], limit)
	if end == offset && offset < len(blob.data) {
		return nil, invalid("max_bytes (too small)")
	}
	return fields{"content": string(blob.data[offset:end]), "total": len(blob.data), "eof": end == len(blob.data)}, nil
}
