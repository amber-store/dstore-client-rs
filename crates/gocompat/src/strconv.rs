//! `strconv.ParseBool/ParseInt/ParseUint/ParseFloat` and `FormatFloat(f, 'g', -1, 64)`, go1.26.5.

/// The two `strconv` error classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumErrorKind {
    /// `strconv.ErrSyntax`: "invalid syntax".
    Syntax,
    /// `strconv.ErrRange`: "value out of range".
    Range,
}

impl NumErrorKind {
    /// The Go text of the class.
    pub fn text(&self) -> &'static str {
        todo!()
    }
}

/// `*strconv.NumError`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("strconv.{func}: parsing {}: {}", crate::quote::quote(.num.as_bytes()), .kind.text())]
pub struct NumError {
    pub func: &'static str,
    pub num: String,
    pub kind: NumErrorKind,
}

/// `strconv.ParseBool`.
pub fn parse_bool(s: &str) -> Result<bool, NumError> {
    todo!()
}

/// `strconv.ParseInt`; base 0 follows the `0x`/`0o`/`0b`/`0` prefixes and the `_` rules.
pub fn parse_int(s: &str, base: u32, bit_size: u32) -> Result<i64, NumError> {
    todo!()
}

/// `strconv.ParseUint`.
pub fn parse_uint(s: &str, base: u32, bit_size: u32) -> Result<u64, NumError> {
    todo!()
}

/// `strconv.ParseFloat(s, 64)`: hex floats, inf/nan, `_` rules.
pub fn parse_float(s: &str) -> Result<f64, NumError> {
    todo!()
}

/// `strconv.FormatFloat(f, 'g', -1, 64)`.
pub fn format_float_g(f: f64) -> String {
    todo!()
}
