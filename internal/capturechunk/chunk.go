package capturechunk

import (
	"encoding/json"
	"unicode/utf8"
)

// Native Rex paginates capture by encoded JSON size rather than raw text bytes.
func JSONPrefix(data string, budget int) int {
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
		encoded, _ := json.Marshal(data[:end])
		if len(encoded) <= budget {
			best = end
			low = middle + 1
		} else {
			high = middle - 1
		}
	}
	return best
}
