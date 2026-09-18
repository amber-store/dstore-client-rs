//! fxamacker decode errors with their verbatim texts.

/// fxamacker `cborType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CborType {
    PositiveInteger,
    NegativeInteger,
    ByteString,
    TextString,
    Array,
    Map,
    Tag,
    Primitives,
}

impl CborType {
    /// The type of an initial byte: its high three bits (fxamacker `getType`).
    pub fn of_initial_byte(b: u8) -> CborType {
        match b >> 5 {
            0 => CborType::PositiveInteger,
            1 => CborType::NegativeInteger,
            2 => CborType::ByteString,
            3 => CborType::TextString,
            4 => CborType::Array,
            5 => CborType::Map,
            6 => CborType::Tag,
            _ => CborType::Primitives,
        }
    }

    /// fxamacker `cborType.String`.
    pub fn as_str(self) -> &'static str {
        match self {
            CborType::PositiveInteger => "positive integer",
            CborType::NegativeInteger => "negative integer",
            CborType::ByteString => "byte string",
            CborType::TextString => "UTF-8 text string",
            CborType::Array => "array",
            CborType::Map => "map",
            CborType::Tag => "tag",
            CborType::Primitives => "primitives",
        }
    }
}

/// "positive integer", …, "UTF-8 text string", "primitives" (fxamacker `common.go`).
impl std::fmt::Display for CborType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// fxamacker `*UnmarshalTypeError`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnmarshalTypeError {
    pub cbor_type: CborType,
    /// Innermost failing Go type, e.g. "uint32", "[]uint8", "view.Pending".
    pub go_type: String,
    /// "wire.Msg.21": the outermost struct field, rewritten on the way out.
    pub struct_field: Option<String>,
    /// "4294967296 overflows uint32", "cannot decode CBOR array to struct without toarray option".
    pub detail: Option<String>,
}

impl UnmarshalTypeError {
    /// The D11 format (fxamacker `UnmarshalTypeError.Error`).
    pub fn message(&self) -> String {
        let mut s = String::from("cbor: cannot unmarshal ");
        s.push_str(self.cbor_type.as_str());
        match self.struct_field.as_deref() {
            Some(field) if !field.is_empty() => {
                s.push_str(" into Go struct field ");
                s.push_str(field);
                s.push_str(" of type ");
            }
            _ => s.push_str(" into Go value of type "),
        }
        s.push_str(&self.go_type);
        if let Some(detail) = self.detail.as_deref()
            && !detail.is_empty()
        {
            s.push_str(" (");
            s.push_str(detail);
            s.push(')');
        }
        s
    }
}

/// `codec.Unmarshal` errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    #[error("EOF")]
    Eof,
    #[error("unexpected EOF")]
    UnexpectedEof,
    #[error("cbor: invalid additional information {ai} for type {ty}")]
    InvalidAi { ai: u8, ty: CborType },
    #[error("cbor: unexpected \"break\" code")]
    UnexpectedBreak,
    #[error("cbor: invalid simple value {0} for type primitives")]
    InvalidSimple(u8),
    #[error("cbor: {ty} length {n} is too large, causing integer overflow")]
    StrLenOverflow { ty: CborType, n: u64 },
    #[error("cbor: {ty} length {n} is too large, it would cause integer overflow")]
    LenOverflow { ty: CborType, n: u64 },
    #[error("cbor: exceeded max nested level 32")]
    MaxNested,
    #[error("cbor: exceeded max number of elements 131072 for CBOR array")]
    MaxArray,
    #[error("cbor: exceeded max number of key-value pairs 131072 for CBOR map")]
    MaxMap,
    #[error("cbor: wrong element type {chunk} for indefinite-length {ty}")]
    ChunkType { chunk: CborType, ty: CborType },
    #[error("cbor: indefinite-length {0} chunk is not definite-length")]
    ChunkIndef(CborType),
    #[error("cbor: {n} bytes of extraneous data starting at index {index}")]
    Extraneous { n: usize, index: usize },
    #[error("cbor: invalid UTF-8 string")]
    InvalidUtf8,
    /// The three D2 messages.
    #[error("{0}")]
    BadTag(String),
    #[error("{}", .0.message())]
    Type(UnmarshalTypeError),
    /// A map key of a type that cannot name a struct field, in a top-level struct. Inside a struct
    /// field the error becomes [`DecodeError::Type`] with the field set, as fxamacker rewrites it.
    #[error(
        "cbor: cannot unmarshal {key_type} into Go value of type string (map key is of type {key_type} and cannot be used to match struct field name)"
    )]
    MapKey { key_type: CborType },
    /// An integer map key beyond int64, in a top-level struct: `detail` is "18446744073709551615" or
    /// "-1-18446744073709551615". Inside a struct field it becomes [`DecodeError::Type`].
    #[error(
        "cbor: cannot unmarshal {cbor_type} into Go value of type int64 ({detail} overflows Go's int64)"
    )]
    MapKeyOverflow { cbor_type: CborType, detail: String },
}

impl DecodeError {
    /// A type error without struct field or detail.
    pub(crate) fn type_error(cbor_type: CborType, go_type: &str) -> DecodeError {
        DecodeError::Type(UnmarshalTypeError {
            cbor_type,
            go_type: go_type.to_owned(),
            struct_field: None,
            detail: None,
        })
    }

    /// A type error with a detail message.
    pub(crate) fn type_detail(cbor_type: CborType, go_type: &str, detail: String) -> DecodeError {
        DecodeError::Type(UnmarshalTypeError {
            cbor_type,
            go_type: go_type.to_owned(),
            struct_field: None,
            detail: Some(detail),
        })
    }

    /// fxamacker `decodeToStructField`: an `*UnmarshalTypeError` leaving a struct field gets the
    /// field's name, overwriting the name an inner struct set. The map-key errors are
    /// `*UnmarshalTypeError`s too, so they are rewritten into the D11 form.
    pub(crate) fn in_struct_field(self, go_struct: &str, key: u64) -> DecodeError {
        let field = format!("{go_struct}.{key}");
        match self {
            DecodeError::Type(mut e) => {
                e.struct_field = Some(field);
                DecodeError::Type(e)
            }
            DecodeError::MapKey { key_type } => DecodeError::Type(UnmarshalTypeError {
                cbor_type: key_type,
                go_type: "string".to_owned(),
                struct_field: Some(field),
                detail: Some(format!(
                    "map key is of type {key_type} and cannot be used to match struct field name"
                )),
            }),
            DecodeError::MapKeyOverflow { cbor_type, detail } => {
                DecodeError::Type(UnmarshalTypeError {
                    cbor_type,
                    go_type: "int64".to_owned(),
                    struct_field: Some(field),
                    detail: Some(format!("{detail} overflows Go's int64")),
                })
            }
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cbor_type_names() {
        let names: Vec<String> = (0u8..8)
            .map(|m| CborType::of_initial_byte(m << 5).to_string())
            .collect();
        assert_eq!(
            names,
            [
                "positive integer",
                "negative integer",
                "byte string",
                "UTF-8 text string",
                "array",
                "map",
                "tag",
                "primitives"
            ]
        );
    }

    #[test]
    fn unmarshal_type_error_formats() {
        let mut e = UnmarshalTypeError {
            cbor_type: CborType::PositiveInteger,
            go_type: "wire.Msg".to_owned(),
            struct_field: None,
            detail: None,
        };
        assert_eq!(
            e.message(),
            "cbor: cannot unmarshal positive integer into Go value of type wire.Msg"
        );
        e.go_type = "uint32".to_owned();
        e.struct_field = Some("wire.Msg.43".to_owned());
        e.detail = Some("4294967296 overflows uint32".to_owned());
        assert_eq!(
            DecodeError::Type(e.clone()).to_string(),
            "cbor: cannot unmarshal positive integer into Go struct field wire.Msg.43 of type uint32 (4294967296 overflows uint32)"
        );
        // Go tests the strings for emptiness, not presence.
        e.struct_field = Some(String::new());
        e.detail = Some(String::new());
        assert_eq!(
            e.message(),
            "cbor: cannot unmarshal positive integer into Go value of type uint32"
        );
    }

    #[test]
    fn verbatim_texts() {
        let cases: Vec<(DecodeError, &str)> = vec![
            (DecodeError::Eof, "EOF"),
            (DecodeError::UnexpectedEof, "unexpected EOF"),
            (
                DecodeError::InvalidAi {
                    ai: 28,
                    ty: CborType::TextString,
                },
                "cbor: invalid additional information 28 for type UTF-8 text string",
            ),
            (
                DecodeError::UnexpectedBreak,
                "cbor: unexpected \"break\" code",
            ),
            (
                DecodeError::InvalidSimple(16),
                "cbor: invalid simple value 16 for type primitives",
            ),
            (
                DecodeError::StrLenOverflow {
                    ty: CborType::ByteString,
                    n: 1 << 63,
                },
                "cbor: byte string length 9223372036854775808 is too large, causing integer overflow",
            ),
            (
                DecodeError::LenOverflow {
                    ty: CborType::Map,
                    n: u64::MAX,
                },
                "cbor: map length 18446744073709551615 is too large, it would cause integer overflow",
            ),
            (DecodeError::MaxNested, "cbor: exceeded max nested level 32"),
            (
                DecodeError::MaxArray,
                "cbor: exceeded max number of elements 131072 for CBOR array",
            ),
            (
                DecodeError::MaxMap,
                "cbor: exceeded max number of key-value pairs 131072 for CBOR map",
            ),
            (
                DecodeError::ChunkType {
                    chunk: CborType::TextString,
                    ty: CborType::ByteString,
                },
                "cbor: wrong element type UTF-8 text string for indefinite-length byte string",
            ),
            (
                DecodeError::ChunkIndef(CborType::ByteString),
                "cbor: indefinite-length byte string chunk is not definite-length",
            ),
            (
                DecodeError::Extraneous { n: 2, index: 5 },
                "cbor: 2 bytes of extraneous data starting at index 5",
            ),
            (DecodeError::InvalidUtf8, "cbor: invalid UTF-8 string"),
            (
                DecodeError::MapKey {
                    key_type: CborType::ByteString,
                },
                "cbor: cannot unmarshal byte string into Go value of type string (map key is of type byte string and cannot be used to match struct field name)",
            ),
            (
                DecodeError::MapKeyOverflow {
                    cbor_type: CborType::NegativeInteger,
                    detail: "-1-18446744073709551615".to_owned(),
                },
                "cbor: cannot unmarshal negative integer into Go value of type int64 (-1-18446744073709551615 overflows Go's int64)",
            ),
        ];
        for (e, want) in cases {
            assert_eq!(e.to_string(), want);
        }
    }

    #[test]
    fn map_key_errors_are_rewritten_in_struct_fields() {
        let e = DecodeError::MapKey {
            key_type: CborType::ByteString,
        }
        .in_struct_field("wire.Msg", 21);
        assert_eq!(
            e.to_string(),
            "cbor: cannot unmarshal byte string into Go struct field wire.Msg.21 of type string (map key is of type byte string and cannot be used to match struct field name)"
        );
        let e = DecodeError::MapKeyOverflow {
            cbor_type: CborType::PositiveInteger,
            detail: "18446744073709551615".to_owned(),
        }
        .in_struct_field("wire.Msg", 26);
        assert_eq!(
            e.to_string(),
            "cbor: cannot unmarshal positive integer into Go struct field wire.Msg.26 of type int64 (18446744073709551615 overflows Go's int64)"
        );
        // Only type errors carry a field.
        assert_eq!(
            DecodeError::InvalidUtf8.in_struct_field("wire.Msg", 6),
            DecodeError::InvalidUtf8
        );
        // The outermost field wins.
        let inner = DecodeError::type_error(CborType::ByteString, "string")
            .in_struct_field("wire.RefInfo", 0)
            .in_struct_field("wire.Msg", 21);
        assert_eq!(
            inner.to_string(),
            "cbor: cannot unmarshal byte string into Go struct field wire.Msg.21 of type string"
        );
    }
}
