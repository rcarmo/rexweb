//! Native Rex capture pagination with Go's encoding/json string byte costs.
#![forbid(unsafe_code)]

/// Return the capture prefix that fits inside `budget` encoded JSON bytes.
/// The service may paginate raw bytes, so malformed UTF-8 is handled too.
#[must_use]
pub fn prefix_len(data: &[u8], budget: usize) -> usize {
    if budget < 8 {
        return 0;
    }
    let (mut low, mut high, mut best) = (0, data.len().min(budget), 0);
    while low <= high {
        let middle = low + (high - low) / 2;
        let mut end = middle;
        while end > 0 && data.get(end).is_some_and(|byte| byte & 0xc0 == 0x80) {
            end -= 1;
        }
        if fits(&data[..end], budget - 2) {
            best = end;
            low = middle + 1;
        } else {
            // The empty prefix always fits once the string quotes are reserved.
            high = middle - 1;
        }
    }
    best
}

fn fits(data: &[u8], mut remaining: usize) -> bool {
    for chunk in data.utf8_chunks() {
        for ch in chunk.valid().chars() {
            let cost = match ch {
                '\u{08}' | '\t' | '\n' | '\u{0c}' | '\r' | '"' | '\\' => 2,
                '\0'..='\u{1f}' | '<' | '>' | '&' | '\u{2028}' | '\u{2029}' => 6,
                _ => ch.len_utf8(),
            };
            let Some(rest) = remaining.checked_sub(cost) else {
                return false;
            };
            remaining = rest;
        }
        // Go's encoder emits an escaped replacement for each malformed byte.
        let Some(rest) = remaining.checked_sub(6 * chunk.invalid().len()) else {
            return false;
        };
        remaining = rest;
    }
    true
}

/// Rust Anything's Go JSON bridge carries valid Unicode strings.
pub fn json_prefix(data: String, budget: i64) -> Result<i64, String> {
    let size = prefix_len(data.as_bytes(), usize::try_from(budget).unwrap_or(0));
    i64::try_from(size).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::prefix_len;

    #[test]
    fn accounts_for_go_json_escapes() {
        assert_eq!(prefix_len(b"a<&", 8), 1);
        assert_eq!(prefix_len(b"\n\t\x08\x0c\r", 10), 4);
        assert_eq!(prefix_len("a\u{2028}b".as_bytes(), 9), 4);
    }

    #[test]
    fn counts_bytes_without_splitting_a_rune() {
        assert_eq!(prefix_len("é🙂中".as_bytes(), 8), 6);
        assert_eq!(prefix_len(b"abcd", 7), 0);
    }

    #[test]
    fn malformed_utf8_costs_six_bytes_per_invalid_byte() {
        assert_eq!(prefix_len(b"\xf0\x9f", 8), 0);
        assert_eq!(prefix_len(b"\xf0\x9f", 14), 2);
        assert_eq!(prefix_len(b"x\xff", 9), 2);
    }
}
