//! `fmt` verbs dstore prints and the `errors.Join` layout.

/// `%x` of bytes (empty input prints nothing).
pub fn hex_lower(b: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(b.len() * 2);
    for &x in b {
        out.push(char::from(DIGITS[usize::from(x >> 4)]));
        out.push(char::from(DIGITS[usize::from(x & 0x0f)]));
    }
    out
}

/// `%v` of a `[]string`: "[a b]", "[]".
pub fn v_strings(items: &[String]) -> String {
    let mut out = String::from("[");
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(item);
    }
    out.push(']');
    out
}

/// `errors.Join(...).Error()`: "\n" between the messages.
///
/// Pass the texts of the non-nil errors only: `errors.Join` drops nil errors, but it keeps an error
/// whose text is empty, so an empty message still contributes its line (go1.26.5 `errors/join.go`).
/// No messages give "" (Go returns a nil error).
pub fn errors_join(msgs: &[String]) -> String {
    msgs.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_lower_matches_percent_x() {
        assert_eq!(hex_lower(&[]), "");
        assert_eq!(hex_lower(&[0x00, 0x0f, 0xab, 0xff]), "000fabff");
        assert_eq!(
            hex_lower(&[0xde, 0xad, 0xbe, 0xef, 0x01, 0x23, 0x45, 0x67]),
            "deadbeef01234567"
        );
    }

    #[test]
    fn v_strings_matches_percent_v() {
        assert_eq!(v_strings(&[]), "[]");
        assert_eq!(v_strings(&["a".to_owned()]), "[a]");
        assert_eq!(
            v_strings(&["1.2.3.4:5".to_owned(), "[::1]:6".to_owned()]),
            "[1.2.3.4:5 [::1]:6]"
        );
        // %v prints strings raw, including empty ones and spaces.
        assert_eq!(
            v_strings(&[String::new(), "a b".to_owned(), String::new()]),
            "[ a b ]"
        );
    }

    #[test]
    fn errors_join_layout() {
        assert_eq!(errors_join(&[]), "");
        assert_eq!(errors_join(&["x".to_owned()]), "x");
        assert_eq!(
            errors_join(&[
                "dial ip:127.0.0.1:9: x".to_owned(),
                "discovery: y".to_owned()
            ]),
            "dial ip:127.0.0.1:9: x\ndiscovery: y"
        );
        // errors.Join(errors.New(""), errors.New("b")).Error() == "\nb"
        assert_eq!(errors_join(&[String::new(), "b".to_owned()]), "\nb");
    }
}
