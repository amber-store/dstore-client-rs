//! transport-iroh v0.4.0 `protocol`: `protocol.Msg`, frame reading, `SendPackRecords` and
//! `NewPackReader`, plus the amberpack record stream over a pack reader.

use dstore_codec::cbor_struct;

use crate::frame::{ShortCause, WireError};

pub const PACK_MAGIC: &[u8; 8] = b"AMBERPK\x03";

cbor_struct! {
    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct ProtocolMsg = "protocol.Msg" {
        0 => typ: i64 = "int",
        1 => name: String = "string" [omitempty],
        2 => root: Vec<u8> = "[]uint8" [omitempty],
        3 => cas: bool = "bool" [omitempty],
        4 => expected_old: Vec<u8> = "[]uint8" [omitempty],
        5 => record: Vec<u8> = "[]uint8" [omitempty],
        6 => refs: Vec<ProtocolRefInfo> = "[]protocol.RefInfo" [omitempty],
        7 => keys: Vec<Vec<u8>> = "[][]uint8" [omitempty],
        8 => data: Vec<u8> = "[]uint8" [omitempty],
        9 => key: Vec<u8> = "[]uint8" [omitempty],
        10 => code: String = "string" [omitempty],
        11 => text: String = "string" [omitempty],
        12 => current: Vec<u8> = "[]uint8" [omitempty],
        13 => token: Vec<u8> = "[]uint8" [omitempty],
        14 => data_conns: i64 = "int" [omitempty],
        15 => data_ports: Vec<u16> = "[]uint16" [omitempty],
        16 => data_endpoints: Vec<ProtocolDataEndpointRec> = "[]protocol.DataEndpointRec" [omitempty],
        17 => names: Vec<String> = "[]string" [omitempty],
    }
}

cbor_struct! {
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct ProtocolRefInfo = "protocol.RefInfo" {
        0 => name: String = "string",
        1 => key: Option<Vec<u8>> = "[]uint8",
        2 => created_at: i64 = "int64",
        3 => user: String = "string" [omitempty],
    }
}

cbor_struct! {
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct ProtocolDataEndpointRec = "protocol.DataEndpointRec" {
        0 => id: Option<Vec<u8>> = "[]uint8",
        1 => addrs: Vec<String> = "[]string" [omitempty],
    }
}

/// `*protocol.RemoteError`: keeps ": " when the text is empty.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("remote: {code}: {text}")]
pub struct ProtocolRemoteError {
    pub code: String,
    pub text: String,
    pub current: Vec<u8>,
}

/// `protocol.ReadMsg` errors.
#[derive(Debug, thiserror::Error)]
pub enum ProtocolFrameError {
    #[error("EOF")]
    Eof,
    #[error("unexpected EOF")]
    UnexpectedEof,
    #[error("protocol: frame of {0} bytes exceeds limit 16777216")]
    TooLarge(u64),
    #[error("protocol: short frame: {0}")]
    Short(ShortCause),
    #[error("protocol: decode frame: {0}")]
    Decode(#[source] dstore_codec::DecodeError),
    #[error("{}", dstore_gocompat::errno::io_error_text(.0))]
    Io(#[source] std::io::Error),
}

/// `protocol.ReadMsg`.
pub async fn read_protocol_msg<R: tokio::io::AsyncRead + Unpin + ?Sized>(
    r: &mut R,
) -> Result<ProtocolMsg, ProtocolFrameError> {
    todo!()
}

/// Errors of a pack reader.
#[derive(Debug, thiserror::Error)]
pub enum PackReadError {
    #[error("{0}")]
    Frame(#[source] ProtocolFrameError),
    /// TErr during a pack.
    #[error("{0}")]
    Remote(#[source] ProtocolRemoteError),
    #[error("protocol: unexpected frame: type {0} during pack transfer")]
    Unexpected(i64),
}

/// SendPackRecords' chunkWriter + amberpack.Writer: exact 1 MiB TData frames ({0:7, 8:chunk}); dropping
/// without finish() writes nothing more (Go: a source error aborts without the terminator or the partial
/// chunk).
pub struct PackSender<'a, W: tokio::io::AsyncWrite + Unpin + ?Sized> {
    w: &'a mut W,
    buf: Vec<u8>,
    wrote_magic: bool,
}

impl<'a, W: tokio::io::AsyncWrite + Unpin + ?Sized> PackSender<'a, W> {
    pub fn new(w: &'a mut W) -> PackSender<'a, W> {
        todo!()
    }

    /// Writes the magic before the first record.
    pub async fn add_record(&mut self, rec: &[u8]) -> Result<(), WireError> {
        todo!()
    }

    /// The magic if none was written, 0x00, the remainder as TData, then TDataEnd {0:8}.
    pub async fn finish(self) -> Result<(), WireError> {
        todo!()
    }
}

/// `protocol.NewPackReader`: TData → bytes, TDataEnd → EOF, TErr → Remote, other → Unexpected; errors
/// are sticky. A clean EOF at a frame boundary also reads as EOF (Ok(0)), as in Go.
pub struct PackReader<R> {
    r: R,
    cur: Vec<u8>,
    pos: usize,
    done: bool,
    err: Option<PackReadError>,
}

impl<R: tokio::io::AsyncRead + Unpin> PackReader<R> {
    pub fn new(r: R) -> PackReader<R> {
        todo!()
    }

    pub async fn read(&mut self, buf: &mut [u8]) -> Result<usize, PackReadError> {
        todo!()
    }

    /// `io.Copy(io.Discard, pr)`.
    pub async fn drain(&mut self) -> Result<u64, PackReadError> {
        todo!()
    }

    pub fn into_inner(self) -> R {
        todo!()
    }
}

/// `amberpack.NewReader(pr).Records()` over a [`PackReader`], reading from the reader's own buffer (no
/// BufReader), so `drain()` consumes TDataEnd.
pub struct PackRecords<R> {
    pr: PackReader<R>,
    state: RecordsState,
}

/// Where the record stream is.
enum RecordsState {
    Magic,
    Records,
    Done,
}

impl<R: tokio::io::AsyncRead + Unpin> PackRecords<R> {
    pub fn new(pr: PackReader<R>) -> PackRecords<R> {
        todo!()
    }

    /// One error, then None.
    pub async fn next(
        &mut self,
    ) -> Option<Result<amber_store_core::amberpack::RawRecord, PackRecordsError>> {
        todo!()
    }

    pub async fn drain(&mut self) -> Result<u64, PackReadError> {
        todo!()
    }

    pub fn into_pack_reader(self) -> PackReader<R> {
        todo!()
    }
}

/// Errors of the record stream.
#[derive(Debug, thiserror::Error)]
pub enum PackRecordsError {
    #[error("{0}")]
    Read(#[source] PackReadError),
    /// "reading magic: unexpected EOF", "bad magic", "bad record tag 0x02",
    /// "record payload N exceeds limit M", …
    #[error("amberpack: malformed pack stream: {0}")]
    Stream(String),
    /// `parse_record` failures (same texts as Go).
    #[error("{0}")]
    Record(#[source] amber_store_core::amberpack::Error),
}
