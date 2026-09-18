//! `encoding/json` v1 for structs of string and bool fields only (escapeHTML = true).

/// A field value to marshal.
pub enum JsonField<'a> {
    Str(&'a [u8]),
    Bool(bool),
}

/// `json.MarshalIndent(v, "", "  ")` of a struct, without a trailing "\n". The caller drops omitempty
/// fields.
pub fn marshal_indent_object(fields: &[(&str, JsonField<'_>)]) -> Vec<u8> {
    todo!()
}

/// The Go type of a struct field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JsonKind {
    String,
    Bool,
}

/// One struct field to unmarshal into.
pub struct JsonFieldSpec {
    pub name: &'static str,
    pub kind: JsonKind,
}

/// A decoded field value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JsonValue {
    String(Vec<u8>),
    Bool(bool),
}

/// `json.Unmarshal` errors with Go's texts.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JsonError {
    #[error("unexpected end of JSON input")]
    UnexpectedEnd,
    /// `invalid character 'x' looking for beginning of value`, … (Go scanner texts).
    #[error("{0}")]
    Syntax(String),
    #[error(
        "json: cannot unmarshal {value} into Go struct field {go_struct}.{field} of type {go_type}"
    )]
    Type {
        value: &'static str,
        go_struct: &'static str,
        field: String,
        go_type: &'static str,
    },
}

/// Go `json.Unmarshal` into a struct: exact key match, then case-folding match (incl. K/U+212A,
/// S/U+017F); the last duplicate wins; null leaves a field unset; unknown keys are ignored; the first
/// type error is recorded and decoding continues; invalid UTF-8 inside strings becomes U+FFFD per byte.
/// Returns one slot per spec (None = not set) and the first error.
pub fn unmarshal_object(
    data: &[u8],
    go_struct: &'static str,
    fields: &[JsonFieldSpec],
) -> (Vec<Option<JsonValue>>, Result<(), JsonError>) {
    todo!()
}
