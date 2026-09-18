//! `strings.ToLower/ToUpper/TrimSpace/FieldsFunc`, go1.26.5.

/// `strings.ToLower`: ASCII fast path, else `strings.Map(unicode.ToLower)`. Each invalid UTF-8 byte
/// becomes U+FFFD; the mapping is simple (not full) case mapping.
pub fn to_lower(s: &[u8]) -> Vec<u8> {
    todo!()
}

/// `strings.ToUpper`, with the same rules as [`to_lower`].
pub fn to_upper(s: &[u8]) -> Vec<u8> {
    todo!()
}

/// `strings.TrimSpace` (Unicode White_Space).
pub fn trim_space(s: &[u8]) -> &[u8] {
    todo!()
}

/// `strings.FieldsFunc`.
pub fn fields_func(s: &[u8], is_sep: impl Fn(char) -> bool) -> Vec<&[u8]> {
    todo!()
}
