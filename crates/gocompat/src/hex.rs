//! `encoding/hex`.

/// `hex.EncodeToString`.
pub fn encode(b: &[u8]) -> String {
    todo!()
}

/// `encoding/hex` errors with Go's texts.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HexError {
    #[error("encoding/hex: odd length hex string")]
    OddLength,
    /// `%#U` of the byte as a rune: `U+007A 'z'`.
    #[error("encoding/hex: invalid byte: {}", fmt_u(*.0))]
    InvalidByte(u8),
}

/// `fmt.Sprintf("%#U", rune(b))`.
pub fn fmt_u(b: u8) -> String {
    todo!()
}

/// `hex.DecodeString`: an invalid byte is reported before an odd length.
pub fn decode_string(s: &[u8]) -> Result<Vec<u8>, HexError> {
    todo!()
}
