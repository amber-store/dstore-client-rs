//! Frames: `u32be len ‖ canonical CBOR` (`wire.ReadMsg`, `WriteMsg`, `Expect`, `WriteErr`).

use crate::error::RemoteError;
use crate::msg::Msg;

/// Why a frame body was short.
#[derive(Debug, thiserror::Error)]
pub enum ShortCause {
    #[error("EOF")]
    Eof,
    #[error("unexpected EOF")]
    UnexpectedEof,
    #[error("{}", dstore_gocompat::errno::io_error_text(.0))]
    Io(std::io::Error),
}

/// Frame I/O errors with Go's texts.
#[derive(Debug, thiserror::Error)]
pub enum WireError {
    /// A clean end before a header.
    #[error("EOF")]
    Eof,
    /// 1-3 header bytes.
    #[error("unexpected EOF")]
    UnexpectedEof,
    #[error("wire: frame of {0} bytes exceeds limit 16777216")]
    TooLarge(u64),
    #[error("wire: short frame: {0}")]
    Short(ShortCause),
    #[error("wire: decode frame: {0}")]
    Decode(#[source] dstore_codec::DecodeError),
    #[error("{}", dstore_gocompat::errno::io_error_text(.0))]
    Io(#[source] std::io::Error),
    /// `Expect` on TErr.
    #[error("{0}")]
    Remote(#[source] RemoteError),
    #[error("wire: unexpected frame: type {got}, want {want}")]
    Protocol { got: i64, want: i64 },
    #[error("wire: key {index} has {len} bytes")]
    KeyLen { index: usize, len: usize },
}

/// `u32be len ‖ payload`; TooLarge writes nothing.
pub fn encode_frame(m: &Msg) -> Result<Vec<u8>, WireError> {
    todo!()
}

/// `wire.WriteMsg`: one `write_all`.
pub async fn write_msg<W: tokio::io::AsyncWrite + Unpin + ?Sized>(
    w: &mut W,
    m: &Msg,
) -> Result<(), WireError> {
    todo!()
}

/// `wire.ReadMsg`: the counting header loop; not cancel-safe.
pub async fn read_msg<R: tokio::io::AsyncRead + Unpin + ?Sized>(
    r: &mut R,
) -> Result<Msg, WireError> {
    todo!()
}

/// `wire.Expect`.
pub async fn expect<R: tokio::io::AsyncRead + Unpin + ?Sized>(
    r: &mut R,
    want: i64,
) -> Result<Msg, WireError> {
    todo!()
}

/// `wire.WriteErr`.
pub async fn write_err<W: tokio::io::AsyncWrite + Unpin + ?Sized>(
    w: &mut W,
    code: &str,
    text: &str,
) -> Result<(), WireError> {
    todo!()
}
