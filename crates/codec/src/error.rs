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

/// "positive integer", …, "UTF-8 text string", "primitives" (fxamacker `common.go`).
impl std::fmt::Display for CborType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        todo!()
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
    /// The D11 format.
    pub fn message(&self) -> String {
        todo!()
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
    #[error(
        "cbor: cannot unmarshal {key_type} into Go value of type string (map key is of type {key_type} and cannot be used to match struct field name)"
    )]
    MapKey { key_type: CborType },
    #[error(
        "cbor: cannot unmarshal {cbor_type} into Go value of type int64 ({detail} overflows Go's int64)"
    )]
    MapKeyOverflow { cbor_type: CborType, detail: String },
}
