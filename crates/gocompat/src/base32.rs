//! `encoding/base32` with `NoPadding`: the Go decode loop (go-iroh node ids, tickets).

/// RFC 4648 standard alphabet.
pub const STD_ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
/// z-base-32 alphabet.
pub const ZBASE32_ALPHABET: &[u8; 32] = b"ybndrfg8ejkmcpqxot1uwisza345h769";

/// `base32.CorruptInputError`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("illegal base32 data at input byte {0}")]
pub struct CorruptInputError(pub usize);

/// `Encoding.WithPadding(NoPadding).DecodeString`: strips `\r\n`, drops 1/3/6-symbol tails, no
/// trailing-bit check.
pub fn decode_nopad(alphabet: &[u8; 32], s: &[u8]) -> Result<Vec<u8>, CorruptInputError> {
    todo!()
}

/// `Encoding.WithPadding(NoPadding).EncodeToString`.
pub fn encode_nopad(alphabet: &[u8; 32], b: &[u8]) -> String {
    todo!()
}
