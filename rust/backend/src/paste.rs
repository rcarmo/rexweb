/// Normalize terminal paste text and optionally apply bracketed-paste markers.
///
/// Callers are responsible for validating the event payload and enforcing the
/// maximum paste length.
pub fn encode(text: &str, bracketed: bool) -> String {
    let normalized = text.replace("\r\n", "\n");
    let mut encoded = String::with_capacity(normalized.len() + 12);

    for ch in normalized.chars() {
        encoded.push(match ch {
            '\n' => '\r',
            '\t' | '\r' => ch,
            '\u{0}'..='\u{1f}' | '\u{7f}' => ' ',
            _ => ch,
        });
    }

    if bracketed && !encoded.is_empty() {
        let mut wrapped = String::with_capacity(encoded.len() + 12);
        wrapped.push_str("\x1b[200~");
        wrapped.push_str(&encoded);
        wrapped.push_str("\x1b[201~");
        wrapped
    } else {
        encoded
    }
}

#[cfg(test)]
mod tests {
    use super::encode;

    #[test]
    fn matches_rex_paste_normalization() {
        let cases = [
            ("plain Unicode", "hello λ😀", false, "hello λ😀"),
            (
                "line endings",
                "one\r\ntwo\nthree\rfour",
                false,
                "one\rtwo\rthree\rfour",
            ),
            (
                "controls",
                "a\u{3}\x1b[201~\0\u{7f}b\tc",
                false,
                "a  [201~  b\tc",
            ),
            (
                "bracketed",
                "hello\nworld",
                true,
                "\x1b[200~hello\rworld\x1b[201~",
            ),
            ("empty", "", true, ""),
        ];

        for (name, input, bracketed, expected) in cases {
            assert_eq!(encode(input, bracketed), expected, "{name}");
        }
    }

    #[test]
    fn preserves_only_the_go_whitelisted_controls() {
        let all_ascii_controls: String = (0_u8..=31).map(char::from).collect();
        let mut expected = " ".repeat(9);
        expected.push('\t');
        expected.push('\r');
        expected.push_str("  ");
        expected.push('\r');
        expected.push_str(&" ".repeat(18));

        assert_eq!(encode(&all_ascii_controls, false), expected);
        assert_eq!(encode("\t\r\n\u{85}", false), "\t\r\u{85}");
        assert_eq!(encode("\u{7f}\u{80}\u{9f}", false), " \u{80}\u{9f}");
    }

    #[test]
    fn normalizes_each_line_ending_before_wrapping() {
        assert_eq!(encode("\r\n\n\r\r\n", false), "\r\r\r\r");
        assert_eq!(encode("x\r\ny", true), "\x1b[200~x\ry\x1b[201~");
    }

    #[test]
    fn only_wraps_nonempty_normalized_text() {
        assert_eq!(encode("", false), "");
        assert_eq!(encode("", true), "");
        assert_eq!(encode("\0", true), "\x1b[200~ \x1b[201~");
    }
}
