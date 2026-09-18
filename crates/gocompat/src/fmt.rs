//! `fmt` verbs dstore prints and the `errors.Join` layout.

/// `%x` of bytes (empty input prints nothing).
pub fn hex_lower(b: &[u8]) -> String {
    todo!()
}

/// `%v` of a `[]string`: "[a b]", "[]".
pub fn v_strings(items: &[String]) -> String {
    todo!()
}

/// `errors.Join(...).Error()`: "\n" between the non-empty messages.
pub fn errors_join(msgs: &[String]) -> String {
    todo!()
}
