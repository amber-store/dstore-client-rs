//! transport-iroh v0.4.0 `protocol`: `protocol.Msg`, frame reading, `SendPackRecords` and
//! `NewPackReader`, plus the amberpack record stream over a pack reader.
//!
//! - [`read_protocol_msg`] is `protocol.ReadMsg` (`protocol/protocol.go:143-164`). A pack reader decodes
//!   every frame with transport-iroh's own `protocol.Msg` typing, so a dstore frame arriving during a
//!   pack is accepted or refused exactly as in Go (codec-wire-ticket §2.3.5, R3).
//! - [`PackSender`] is `SendPackRecords` (`protocol/pack.go:76-91`): core `amberpack.Writer`, whose
//!   4096-byte `bufio.Writer` sits over transport-iroh's `chunkWriter` (`pack.go:33-69`).
//! - [`PackReader`] is `packReader` (`pack.go:101-141`).
//! - [`PackRecords`] is core v0.0.8 `amberpack.Reader.Records` (`amberpack/pack.go:140-195`) over a
//!   [`PackReader`], validating each record with core-rs `amberpack::parse_record`.
//!
//! Spec: PORTING.md §4.3; port-notes/codec-wire-ticket.md §2.3; core-rs-gaps.md §2.2, §2.10.

use std::io;

use amber_store_core::amberpack::{self, MAX_PAYLOAD, REC_HEADER_SIZE, RawRecord};
use dstore_codec::{Enc, cbor_struct};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::consts::{CHUNK_SIZE, MAX_FRAME, T_DATA, T_DATA_END, T_ERR};
use crate::frame::{ShortCause, WireError};

pub const PACK_MAGIC: &[u8; 8] = b"AMBERPK\x03";

/// core `amberpack` `tagEnd`: the end marker of a record stream.
const TAG_END: u8 = 0x00;
/// core `amberpack` `tagChunk`: the first byte of every record.
const TAG_CHUNK: u8 = 0x01;
/// `bufio.defaultBufSize`, the buffer of core `amberpack.Writer`.
const BUFIO_SIZE: usize = 4096;
/// `WriteMsg(Msg{Type: TDataEnd})`.
const TDATA_END_FRAME: [u8; 7] = [0x00, 0x00, 0x00, 0x03, 0xa1, 0x00, 0x08];

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

/// `protocol.RemoteFromMsg`.
fn remote_from_msg(m: ProtocolMsg) -> ProtocolRemoteError {
    ProtocolRemoteError {
        code: m.code,
        text: m.text,
        current: m.current,
    }
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

impl ProtocolFrameError {
    /// A copy with the same variant and text (the pack reader's errors are sticky).
    fn dup(&self) -> ProtocolFrameError {
        match self {
            ProtocolFrameError::Eof => ProtocolFrameError::Eof,
            ProtocolFrameError::UnexpectedEof => ProtocolFrameError::UnexpectedEof,
            ProtocolFrameError::TooLarge(n) => ProtocolFrameError::TooLarge(*n),
            ProtocolFrameError::Short(cause) => ProtocolFrameError::Short(match cause {
                ShortCause::Eof => ShortCause::Eof,
                ShortCause::UnexpectedEof => ShortCause::UnexpectedEof,
                ShortCause::Io(e) => ShortCause::Io(dup_io(e)),
            }),
            ProtocolFrameError::Decode(e) => ProtocolFrameError::Decode(e.clone()),
            ProtocolFrameError::Io(e) => ProtocolFrameError::Io(dup_io(e)),
        }
    }
}

/// A copy of an I/O error that renders the same Go text (`errno::io_error_text`): the raw errno when there
/// is one, else the kind and the message.
fn dup_io(e: &io::Error) -> io::Error {
    match e.raw_os_error() {
        Some(code) => io::Error::from_raw_os_error(code),
        None => io::Error::new(e.kind(), e.to_string()),
    }
}

/// `protocol.ReadMsg`: the counting header loop; not cancel-safe.
///
/// - 0 header bytes, then EOF → `Eof`; 1-3 bytes, then EOF → `UnexpectedEof`; another error → `Io` (one of
///   kind `UnexpectedEof` → `UnexpectedEof`, as Go's `errors.Is(err, io.ErrUnexpectedEOF)`).
/// - The length is checked against `MAX_FRAME` before the payload is allocated.
/// - A short payload → `Short(Eof)` when no payload byte came, `Short(UnexpectedEof)` when some did.
/// - The payload decodes as `protocol.Msg` (`protocol: decode frame: <cbor error>`).
pub async fn read_protocol_msg<R: tokio::io::AsyncRead + Unpin + ?Sized>(
    r: &mut R,
) -> Result<ProtocolMsg, ProtocolFrameError> {
    let mut hdr = [0u8; 4];
    if let Err(cause) = read_full(r, &mut hdr).await {
        return Err(match cause {
            ShortCause::Eof => ProtocolFrameError::Eof,
            ShortCause::UnexpectedEof => ProtocolFrameError::UnexpectedEof,
            ShortCause::Io(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                ProtocolFrameError::UnexpectedEof
            }
            ShortCause::Io(e) => ProtocolFrameError::Io(e),
        });
    }
    let n = u32::from_be_bytes(hdr);
    let len = match usize::try_from(n) {
        Ok(len) if len <= MAX_FRAME => len,
        _ => return Err(ProtocolFrameError::TooLarge(u64::from(n))),
    };
    let mut payload = vec![0u8; len];
    read_full(r, &mut payload)
        .await
        .map_err(ProtocolFrameError::Short)?;
    dstore_codec::unmarshal::<ProtocolMsg>(&payload).map_err(ProtocolFrameError::Decode)
}

/// Go `io.ReadFull` over an async reader: counts the bytes, so a clean end (`Eof`) is told from a cut one
/// (`UnexpectedEof`; codec-wire-ticket K9).
async fn read_full<R: AsyncRead + Unpin + ?Sized>(
    r: &mut R,
    buf: &mut [u8],
) -> Result<(), ShortCause> {
    let mut got = 0usize;
    while let Some(rest) = buf.get_mut(got..).filter(|rest| !rest.is_empty()) {
        match r.read(rest).await {
            Ok(0) => {
                return Err(if got == 0 {
                    ShortCause::Eof
                } else {
                    ShortCause::UnexpectedEof
                });
            }
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(ShortCause::Io(e)),
        }
    }
    Ok(())
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

impl PackReadError {
    /// A copy with the same variant and text (errors are sticky).
    fn dup(&self) -> PackReadError {
        match self {
            PackReadError::Frame(e) => PackReadError::Frame(e.dup()),
            PackReadError::Remote(e) => PackReadError::Remote(e.clone()),
            PackReadError::Unexpected(t) => PackReadError::Unexpected(*t),
        }
    }

    /// The sticky "error" of a clean end of stream at a frame boundary, which reads as EOF.
    fn is_eof(&self) -> bool {
        matches!(self, PackReadError::Frame(ProtocolFrameError::Eof))
    }
}

/// SendPackRecords' chunkWriter + amberpack.Writer: exact 1 MiB TData frames ({0:7, 8:chunk}); dropping
/// without finish() writes nothing more (Go: a source error aborts without the terminator or the partial
/// chunk).
///
/// Bytes pass through a 4096-byte buffer before the chunk writer, as they pass through core
/// `amberpack.Writer`'s `bufio.Writer`. The frames of a finished pack do not depend on it, but it decides
/// which completed chunks were already written when a sender is dropped: Go writes a chunk only once the
/// bytes that complete it have left that buffer. After a write error every call returns that error again
/// (bufio's sticky error). Not cancel-safe.
pub struct PackSender<'a, W: tokio::io::AsyncWrite + Unpin + ?Sized> {
    w: &'a mut W,
    /// `chunkWriter.buf`: the pack bytes of the frame being filled (fewer than `CHUNK_SIZE` between calls).
    buf: Vec<u8>,
    wrote_magic: bool,
    /// The `bufio.Writer` buffer of `amberpack.Writer` (at most `BUFIO_SIZE` bytes).
    bw: Vec<u8>,
    /// `bufio.Writer.err`.
    err: Option<io::Error>,
}

impl<'a, W: tokio::io::AsyncWrite + Unpin + ?Sized> PackSender<'a, W> {
    pub fn new(w: &'a mut W) -> PackSender<'a, W> {
        PackSender {
            w,
            buf: Vec::new(),
            wrote_magic: false,
            bw: Vec::with_capacity(BUFIO_SIZE),
            err: None,
        }
    }

    /// Writes the magic before the first record.
    ///
    /// `amberpack.Writer.AddRecord`: the record is written as given; the receiver validates it.
    pub async fn add_record(&mut self, rec: &[u8]) -> Result<(), WireError> {
        self.ensure_magic().await?;
        self.buffered_write(rec).await
    }

    /// The magic if none was written, 0x00, the remainder as TData, then TDataEnd {0:8}.
    ///
    /// `amberpack.Writer.Close` (magic, end marker, flush), then `chunkWriter.finish`. The writer is
    /// flushed at the end (a no-op for QUIC streams).
    pub async fn finish(mut self) -> Result<(), WireError> {
        self.ensure_magic().await?;
        // bufio.Writer.WriteByte(tagEnd).
        self.sticky()?;
        if self.bw.len() >= BUFIO_SIZE {
            self.buffered_flush().await?;
        }
        self.bw.push(TAG_END);
        self.buffered_flush().await?;
        // chunkWriter.finish.
        self.chunk_flush().await.map_err(WireError::Io)?;
        self.w
            .write_all(&TDATA_END_FRAME)
            .await
            .map_err(WireError::Io)?;
        self.w.flush().await.map_err(WireError::Io)
    }

    /// `amberpack.Writer.ensureHeader`.
    async fn ensure_magic(&mut self) -> Result<(), WireError> {
        if self.wrote_magic {
            return Ok(());
        }
        self.buffered_write(PACK_MAGIC).await?;
        self.wrote_magic = true;
        Ok(())
    }

    /// `bufio.Writer.Write`.
    async fn buffered_write(&mut self, mut p: &[u8]) -> Result<(), WireError> {
        self.sticky()?;
        while p.len() > BUFIO_SIZE - self.bw.len() {
            if self.bw.is_empty() {
                // A large write into an empty buffer goes straight to the chunk writer.
                let res = self.chunk_write(p).await;
                return self.keep_error(res);
            }
            let (head, tail) = p.split_at(BUFIO_SIZE - self.bw.len());
            self.bw.extend_from_slice(head);
            p = tail;
            self.buffered_flush().await?;
        }
        self.bw.extend_from_slice(p);
        Ok(())
    }

    /// `bufio.Writer.Flush`.
    async fn buffered_flush(&mut self) -> Result<(), WireError> {
        self.sticky()?;
        if self.bw.is_empty() {
            return Ok(());
        }
        let bw = std::mem::take(&mut self.bw);
        let res = self.chunk_write(&bw).await;
        self.bw = bw;
        self.bw.clear();
        self.keep_error(res)
    }

    /// `chunkWriter.Write`: a TData frame whenever exactly `CHUNK_SIZE` bytes are buffered.
    async fn chunk_write(&mut self, mut p: &[u8]) -> io::Result<()> {
        while !p.is_empty() {
            if self.buf.is_empty() && p.len() >= CHUNK_SIZE {
                // A whole chunk: the frame Go writes, without copying it into the buffer first.
                let (chunk, tail) = p.split_at(CHUNK_SIZE);
                write_tdata(&mut *self.w, chunk).await?;
                p = tail;
                continue;
            }
            let (head, tail) = p.split_at((CHUNK_SIZE - self.buf.len()).min(p.len()));
            if self.buf.capacity() == 0 {
                self.buf.reserve_exact(CHUNK_SIZE);
            }
            self.buf.extend_from_slice(head);
            p = tail;
            if self.buf.len() == CHUNK_SIZE {
                self.chunk_flush().await?;
            }
        }
        Ok(())
    }

    /// `chunkWriter.flush`: a non-empty buffer as one TData frame; an empty one writes nothing.
    async fn chunk_flush(&mut self) -> io::Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        let res = write_tdata(&mut *self.w, &self.buf).await;
        self.buf.clear();
        res
    }

    fn sticky(&self) -> Result<(), WireError> {
        match &self.err {
            Some(e) => Err(WireError::Io(dup_io(e))),
            None => Ok(()),
        }
    }

    fn keep_error(&mut self, res: io::Result<()>) -> Result<(), WireError> {
        res.map_err(|e| {
            self.err = Some(dup_io(&e));
            WireError::Io(e)
        })
    }
}

/// `WriteMsg(Msg{Type: TData, Data: data})`: `u32be len ‖ a2 00 07 08 <bstr head> ‖ data`, only keys 0
/// and 8 (codec-wire-ticket §2.3.4). Go writes the length header, then the whole CBOR payload; this writes
/// the length with the CBOR head, then the data, so the chunk is not copied. The bytes are the same.
async fn write_tdata<W: AsyncWrite + Unpin + ?Sized>(w: &mut W, data: &[u8]) -> io::Result<()> {
    let mut e = Enc::new();
    e.head(5, 2);
    e.uint(0);
    e.int(T_DATA);
    e.uint(8);
    e.head(2, data.len() as u64);
    let head = e.into_bytes();
    // At most CHUNK_SIZE + 9 bytes, far below MAX_FRAME.
    let len = (head.len() + data.len()) as u32;
    let mut prefix = Vec::with_capacity(4 + head.len());
    prefix.extend_from_slice(&len.to_be_bytes());
    prefix.extend_from_slice(&head);
    w.write_all(&prefix).await?;
    w.write_all(data).await
}

/// `protocol.NewPackReader`: TData → bytes, TDataEnd → EOF, TErr → Remote, other → Unexpected; errors
/// are sticky. A clean EOF at a frame boundary also reads as EOF (Ok(0)), as in Go.
///
/// Not cancel-safe: a frame read that is abandoned leaves the stream mid-frame.
pub struct PackReader<R> {
    r: R,
    cur: Vec<u8>,
    pos: usize,
    done: bool,
    err: Option<PackReadError>,
}

impl<R: tokio::io::AsyncRead + Unpin> PackReader<R> {
    pub fn new(r: R) -> PackReader<R> {
        PackReader {
            r,
            cur: Vec::new(),
            pos: 0,
            done: false,
            err: None,
        }
    }

    /// `packReader.Read`: reads frames until the current chunk has bytes, then copies what fits. Ok(0) is
    /// EOF, except for an empty `buf`, which still waits for data first, as Go's `Read(nil)` does.
    pub async fn read(&mut self, buf: &mut [u8]) -> Result<usize, PackReadError> {
        if !self.fill().await? {
            return Ok(0);
        }
        let avail = self.cur.get(self.pos..).unwrap_or_default();
        let n = avail.len().min(buf.len());
        if let (Some(dst), Some(src)) = (buf.get_mut(..n), avail.get(..n)) {
            dst.copy_from_slice(src);
        }
        self.pos += n;
        Ok(n)
    }

    /// `io.Copy(io.Discard, pr)`: the number of pack bytes discarded. EOF (TDataEnd, or a clean end at a
    /// frame boundary) is success.
    pub async fn drain(&mut self) -> Result<u64, PackReadError> {
        let mut n = 0u64;
        while self.fill().await? {
            n += self.cur.len().saturating_sub(self.pos) as u64;
            self.pos = self.cur.len();
        }
        Ok(n)
    }

    pub fn into_inner(self) -> R {
        self.r
    }

    /// The loop of `packReader.Read`: true once unread chunk bytes are available, false at EOF.
    async fn fill(&mut self) -> Result<bool, PackReadError> {
        while self.pos >= self.cur.len() {
            if let Some(e) = &self.err {
                return if e.is_eof() { Ok(false) } else { Err(e.dup()) };
            }
            if self.done {
                return Ok(false);
            }
            let m = match read_protocol_msg(&mut self.r).await {
                Ok(m) => m,
                Err(e) => return self.stick(PackReadError::Frame(e)),
            };
            match m.typ {
                T_DATA => {
                    // An empty Data yields no bytes and the loop reads on.
                    self.cur = m.data;
                    self.pos = 0;
                }
                T_DATA_END => self.done = true,
                T_ERR => return self.stick(PackReadError::Remote(remote_from_msg(m))),
                typ => return self.stick(PackReadError::Unexpected(typ)),
            }
        }
        Ok(true)
    }

    /// Keeps `e` as the sticky error and returns it; EOF reads as false.
    fn stick(&mut self, e: PackReadError) -> Result<bool, PackReadError> {
        self.err = Some(e.dup());
        if e.is_eof() { Ok(false) } else { Err(e) }
    }
}

/// `amberpack.NewReader(pr).Records()` over a [`PackReader`], reading from the reader's own buffer (no
/// BufReader), so `drain()` consumes TDataEnd.
///
/// Go reads through a 4096-byte `bufio.Reader`, but `packReader.Read` never returns bytes of more than
/// one frame, so both read the same frames and give the same records and errors. Only `drain()`'s count
/// can differ: bytes after the end marker that Go's buffer swallowed are counted here.
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

/// Why a read of the record stream stopped short: Go `io.ReadFull` / `bufio.Reader.ReadByte`.
enum ReadShort {
    Eof,
    UnexpectedEof,
    Pack(PackReadError),
}

impl std::fmt::Display for ReadShort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadShort::Eof => f.write_str("EOF"),
            ReadShort::UnexpectedEof => f.write_str("unexpected EOF"),
            ReadShort::Pack(e) => write!(f, "{e}"),
        }
    }
}

/// `fmt.Errorf("%w: <step>: %v", ErrMalformed, err)`: the read error is kept as text only, as `%v` does.
fn malformed(step: &str, e: ReadShort) -> PackRecordsError {
    PackRecordsError::Stream(format!("{step}: {e}"))
}

impl<R: tokio::io::AsyncRead + Unpin> PackRecords<R> {
    pub fn new(pr: PackReader<R>) -> PackRecords<R> {
        PackRecords {
            pr,
            state: RecordsState::Magic,
        }
    }

    /// One error, then None.
    ///
    /// The next validated record, undecoded (framing, the size bound, CRC and key canonicality are
    /// checked; the payload hash is not). `None` after the end marker, which is not followed by a read of
    /// TDataEnd: call [`PackRecords::drain`] before reading the next frame. Not cancel-safe.
    pub async fn next(
        &mut self,
    ) -> Option<Result<amber_store_core::amberpack::RawRecord, PackRecordsError>> {
        if matches!(self.state, RecordsState::Done) {
            return None;
        }
        match self.next_record().await {
            Ok(Some(rec)) => Some(Ok(rec)),
            Ok(None) => {
                self.state = RecordsState::Done;
                None
            }
            Err(e) => {
                self.state = RecordsState::Done;
                Some(Err(e))
            }
        }
    }

    pub async fn drain(&mut self) -> Result<u64, PackReadError> {
        self.pr.drain().await
    }

    pub fn into_pack_reader(self) -> PackReader<R> {
        self.pr
    }

    /// One step of `Records`: Ok(None) on the end marker.
    async fn next_record(&mut self) -> Result<Option<RawRecord>, PackRecordsError> {
        if matches!(self.state, RecordsState::Magic) {
            let mut magic = [0u8; PACK_MAGIC.len()];
            self.read_full(&mut magic)
                .await
                .map_err(|e| malformed("reading magic", e))?;
            if &magic != PACK_MAGIC {
                return Err(PackRecordsError::Stream("bad magic".to_string()));
            }
            self.state = RecordsState::Records;
        }
        let tag = self
            .read_byte()
            .await
            .map_err(|e| malformed("truncated before end marker", e))?;
        match tag {
            TAG_END => Ok(None),
            TAG_CHUNK => {
                // The full record: the tag, the other 45 header bytes, then slen payload bytes.
                let mut hdr = [0u8; REC_HEADER_SIZE];
                hdr[0] = tag;
                self.read_full(&mut hdr[1..])
                    .await
                    .map_err(|e| malformed("truncated record header", e))?;
                let slen = u32::from_be_bytes([hdr[38], hdr[39], hdr[40], hdr[41]]);
                if slen > MAX_PAYLOAD {
                    return Err(PackRecordsError::Stream(format!(
                        "record payload {slen} exceeds limit {MAX_PAYLOAD}"
                    )));
                }
                let slen = slen as usize;
                let mut full = Vec::with_capacity(REC_HEADER_SIZE + slen.min(CHUNK_SIZE));
                full.extend_from_slice(&hdr);
                self.read_append(&mut full, slen)
                    .await
                    .map_err(|e| malformed("truncated record payload", e))?;
                // Go: fmt.Errorf("%w: %v", ErrMalformed, err).
                let record = amberpack::parse_record(&full).map_err(|e| {
                    PackRecordsError::Record(amberpack::Error::Malformed(e.to_string()))
                })?;
                Ok(Some(RawRecord {
                    record,
                    bytes: full,
                }))
            }
            tag => Err(PackRecordsError::Stream(format!("bad record tag {tag:#x}"))),
        }
    }

    /// `io.ReadFull`.
    async fn read_full(&mut self, buf: &mut [u8]) -> Result<(), ReadShort> {
        let mut got = 0usize;
        while let Some(rest) = buf.get_mut(got..).filter(|rest| !rest.is_empty()) {
            match self.pr.read(rest).await {
                Ok(0) => {
                    return Err(if got == 0 {
                        ReadShort::Eof
                    } else {
                        ReadShort::UnexpectedEof
                    });
                }
                Ok(n) => got += n,
                Err(e) => return Err(ReadShort::Pack(e)),
            }
        }
        Ok(())
    }

    /// `bufio.Reader.ReadByte`.
    async fn read_byte(&mut self) -> Result<u8, ReadShort> {
        let mut b = [0u8; 1];
        match self.pr.read(&mut b).await {
            Ok(0) => Err(ReadShort::Eof),
            Ok(_) => Ok(b[0]),
            Err(e) => Err(ReadShort::Pack(e)),
        }
    }

    /// `io.ReadFull` of `n` more bytes onto `out`, straight from the pack reader's chunks. `out` grows with
    /// the bytes received rather than with the untrusted length.
    async fn read_append(&mut self, out: &mut Vec<u8>, n: usize) -> Result<(), ReadShort> {
        let mut got = 0usize;
        while got < n {
            match self.pr.fill().await {
                Ok(true) => {}
                Ok(false) => {
                    return Err(if got == 0 {
                        ReadShort::Eof
                    } else {
                        ReadShort::UnexpectedEof
                    });
                }
                Err(e) => return Err(ReadShort::Pack(e)),
            }
            let avail = self.pr.cur.get(self.pr.pos..).unwrap_or_default();
            let take = avail.len().min(n - got);
            out.extend_from_slice(avail.get(..take).unwrap_or_default());
            self.pr.pos += take;
            got += take;
        }
        Ok(())
    }
}

/// Errors of the record stream.
///
/// Every error of Go's `Records` wraps `amberpack.ErrMalformed` and renders
/// `amberpack: malformed pack stream: …`; so do these.
#[derive(Debug, thiserror::Error)]
pub enum PackRecordsError {
    /// Not produced by [`PackRecords::next`]: Go's `Records` formats a pack-reader error into its own text
    /// with `%v` (`amberpack: malformed pack stream: truncated before end marker: remote: busy: late`),
    /// which is [`PackRecordsError::Stream`], and the remote error can no longer be matched.
    #[error("{0}")]
    Read(#[source] PackReadError),
    /// "reading magic: unexpected EOF", "bad magic", "bad record tag 0x2",
    /// "record payload N exceeds limit M", "truncated record payload: remote: internal: sender died", …
    #[error("amberpack: malformed pack stream: {0}")]
    Stream(String),
    /// `parse_record` failures (same texts as Go): `amberpack::Error::Malformed` holding the
    /// `amberpack: corrupt pack data: …` text, as Go's `fmt.Errorf("%w: %v", ErrMalformed, err)` renders it.
    #[error("{0}")]
    Record(#[source] amber_store_core::amberpack::Error),
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    use amber_store_core::amberpack::{decode_payload, encode_record};
    use amber_store_core::fstree::{Object, encode_blob};
    use amber_store_core::key::{Key, Type};
    use tokio::io::ReadBuf;

    use super::*;
    use crate::consts::{CODE_CAS_MISMATCH, CODE_INTERNAL, CODE_UNKNOWN_REF};
    use crate::frame::read_msg;

    // transport-iroh protocol frame types that dstore does not name.
    const T_PUSH: i64 = 1;
    const T_PULL: i64 = 2;
    const T_REF_LIST: i64 = 3;
    const T_REF: i64 = 4;
    const T_REFS: i64 = 5;
    const T_WANTS: i64 = 6;
    const T_OK: i64 = 9;
    const T_ATTACH: i64 = 11;
    const T_ACCEPT: i64 = 12;
    const T_PIN: i64 = 13;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    fn msg(typ: i64) -> ProtocolMsg {
        ProtocolMsg {
            typ,
            ..Default::default()
        }
    }

    /// `protocol.WriteMsg`.
    fn frame(m: &ProtocolMsg) -> Vec<u8> {
        let payload = dstore_codec::marshal(m);
        let mut out = (payload.len() as u32).to_be_bytes().to_vec();
        out.extend_from_slice(&payload);
        out
    }

    fn tdata(b: &[u8]) -> Vec<u8> {
        frame(&ProtocolMsg {
            typ: T_DATA,
            data: b.to_vec(),
            ..Default::default()
        })
    }

    fn tdata_end() -> Vec<u8> {
        frame(&msg(T_DATA_END))
    }

    /// A wire pack of the magic then `body`, in one TData frame, then TDataEnd.
    fn framed_pack(body: &[u8]) -> Vec<u8> {
        let mut pack = PACK_MAGIC.to_vec();
        pack.extend_from_slice(body);
        let mut out = tdata(&pack);
        out.extend_from_slice(&tdata_end());
        out
    }

    async fn send(recs: &[Vec<u8>]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut s = PackSender::new(&mut out);
        for r in recs {
            s.add_record(r)
                .await
                .unwrap_or_else(|e| panic!("add_record: {e}"));
        }
        s.finish().await.unwrap_or_else(|e| panic!("finish: {e}"));
        out
    }

    /// The records up to the first error; asserts that the error is the last item.
    async fn collect_records<R: AsyncRead + Unpin>(
        recs: &mut PackRecords<R>,
    ) -> (Vec<RawRecord>, Option<PackRecordsError>) {
        let mut out = Vec::new();
        while let Some(r) = recs.next().await {
            match r {
                Ok(rec) => out.push(rec),
                Err(e) => {
                    assert!(recs.next().await.is_none(), "one error, then None");
                    return (out, Some(e));
                }
            }
        }
        assert!(recs.next().await.is_none(), "None stays None");
        (out, None)
    }

    async fn records_of(stream: &[u8]) -> (Vec<RawRecord>, Option<PackRecordsError>) {
        collect_records(&mut PackRecords::new(PackReader::new(stream))).await
    }

    /// The frame lengths (u32 headers) of a stream.
    fn frame_lens(mut b: &[u8]) -> Vec<usize> {
        let mut out = Vec::new();
        while b.len() >= 4 {
            let n = u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize;
            out.push(n);
            b = &b[4 + n..];
        }
        assert!(b.is_empty(), "trailing bytes");
        out
    }

    /// splitmix64 bytes: incompressible, so records stay raw.
    fn incompressible(n: usize) -> Vec<u8> {
        let mut state = 0x1234_5678_9abc_def0u64;
        let mut out = Vec::with_capacity(n + 8);
        while out.len() < n {
            state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            out.extend_from_slice(&(z ^ (z >> 31)).to_le_bytes());
        }
        out.truncate(n);
        out
    }

    /// CRC-32C (Castagnoli), bitwise.
    fn crc32c(data: &[u8]) -> u32 {
        let mut crc = !0u32;
        for &b in data {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0x82f6_3b78
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    /// Recomputes a record's CRC field.
    fn recrc(rec: &mut [u8]) {
        rec[42..46].fill(0);
        let c = crc32c(rec);
        rec[42..46].copy_from_slice(&c.to_be_bytes());
    }

    fn blob(data: &[u8]) -> Object {
        encode_blob(data)
    }

    fn record(o: &Object) -> Vec<u8> {
        encode_record(o.key, &o.bytes).unwrap_or_else(|e| panic!("encode_record: {e}"))
    }

    /// A reader that delivers scripted chunks (at most what fits per read), then EOF.
    struct Script(VecDeque<io::Result<Vec<u8>>>);

    impl Script {
        fn bytewise(b: &[u8]) -> Script {
            Script(b.iter().map(|&x| Ok(vec![x])).collect())
        }
    }

    impl AsyncRead for Script {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            match self.0.pop_front() {
                None => Poll::Ready(Ok(())),
                Some(Err(e)) => Poll::Ready(Err(e)),
                Some(Ok(mut chunk)) => {
                    let n = chunk.len().min(buf.remaining());
                    buf.put_slice(&chunk[..n]);
                    if n < chunk.len() {
                        chunk.drain(..n);
                        self.0.push_front(Ok(chunk));
                    }
                    Poll::Ready(Ok(()))
                }
            }
        }
    }

    /// A writer that accepts `limit` bytes, then fails with EPIPE.
    struct FailAfter {
        limit: usize,
        written: Vec<u8>,
    }

    impl AsyncWrite for FailAfter {
        fn poll_write(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            let room = self.limit - self.written.len();
            if room == 0 {
                return Poll::Ready(Err(io::Error::from_raw_os_error(32)));
            }
            let n = buf.len().min(room);
            self.written.extend_from_slice(&buf[..n]);
            Poll::Ready(Ok(n))
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    // ---- transport-iroh protocol/protocol_test.go ----

    /// `TestMsgRoundTrip`.
    #[tokio::test]
    async fn msg_round_trip() {
        let rep = |b: u8| vec![b; 32];
        let msgs = vec![
            ProtocolMsg {
                typ: T_PUSH,
                name: "backups/home".into(),
                root: rep(7),
                cas: true,
                expected_old: rep(9),
                ..Default::default()
            },
            ProtocolMsg {
                typ: T_PUSH,
                name: "n".into(),
                root: rep(7),
                cas: true,
                ..Default::default()
            },
            ProtocolMsg {
                typ: T_PULL,
                name: "backups/home".into(),
                ..Default::default()
            },
            msg(T_REF_LIST),
            ProtocolMsg {
                typ: T_REF,
                record: vec![0xa1, 0x00, 0x01],
                ..Default::default()
            },
            ProtocolMsg {
                typ: T_REFS,
                refs: vec![ProtocolRefInfo {
                    name: "a".into(),
                    key: Some(rep(1)),
                    created_at: 42,
                    user: "u".into(),
                }],
                ..Default::default()
            },
            ProtocolMsg {
                typ: T_WANTS,
                keys: vec![rep(2), rep(3)],
                ..Default::default()
            },
            msg(T_WANTS),
            ProtocolMsg {
                typ: T_DATA,
                data: b"payload".to_vec(),
                ..Default::default()
            },
            msg(T_DATA_END),
            ProtocolMsg {
                typ: T_OK,
                key: rep(4),
                ..Default::default()
            },
            ProtocolMsg {
                typ: T_ERR,
                code: CODE_CAS_MISMATCH.into(),
                text: "remote ref changed".into(),
                current: rep(5),
                ..Default::default()
            },
            ProtocolMsg {
                typ: T_PUSH,
                name: "n".into(),
                root: rep(7),
                data_conns: 3,
                ..Default::default()
            },
            ProtocolMsg {
                typ: T_ACCEPT,
                token: b"tok-1234".to_vec(),
                data_ports: vec![4242, 4243],
                ..Default::default()
            },
            ProtocolMsg {
                typ: T_ATTACH,
                token: b"tok-1234".to_vec(),
                ..Default::default()
            },
            ProtocolMsg {
                typ: T_REF,
                record: vec![0xa1],
                token: b"tok-9".to_vec(),
                ..Default::default()
            },
        ];
        let stream: Vec<u8> = msgs.iter().flat_map(frame).collect();
        let mut r: &[u8] = &stream;
        for want in &msgs {
            let got = read_protocol_msg(&mut r)
                .await
                .unwrap_or_else(|e| panic!("read (want {want:?}): {e}"));
            assert_eq!(&got, want);
        }
        assert!(matches!(
            read_protocol_msg(&mut r).await,
            Err(ProtocolFrameError::Eof)
        ));
    }

    /// `TestReadMsgRejectsOversizeFrame`.
    #[tokio::test]
    async fn read_msg_rejects_oversize_frame() {
        let hdr = ((MAX_FRAME + 1) as u32).to_be_bytes();
        let err = read_protocol_msg(&mut &hdr[..]).await.err();
        assert!(matches!(err, Some(ProtocolFrameError::TooLarge(16777217))));
        assert_eq!(
            err.map(|e| e.to_string()).as_deref(),
            Some("protocol: frame of 16777217 bytes exceeds limit 16777216")
        );
    }

    /// `TestReadMsgTruncatedFrame`.
    #[tokio::test]
    async fn read_msg_truncated_frame() {
        let f = frame(&msg(T_DATA_END));
        let err = read_protocol_msg(&mut &f[..f.len() - 1]).await.err();
        assert_eq!(
            err.map(|e| e.to_string()).as_deref(),
            Some("protocol: short frame: unexpected EOF")
        );
    }

    /// `TestRemoteError`.
    #[test]
    fn remote_error() {
        let m = ProtocolMsg {
            typ: T_ERR,
            code: CODE_UNKNOWN_REF.into(),
            text: "ref \"x\" not found".into(),
            ..Default::default()
        };
        let re = remote_from_msg(m.clone());
        assert_eq!(
            (re.code.as_str(), re.text.as_str()),
            (CODE_UNKNOWN_REF, m.text.as_str())
        );
        assert_eq!(re.to_string(), "remote: unknown-ref: ref \"x\" not found");
        let busy = remote_from_msg(ProtocolMsg {
            code: "busy".into(),
            current: vec![1, 2],
            ..Default::default()
        });
        assert_eq!(busy.to_string(), "remote: busy: ");
        assert_eq!(busy.current, [1, 2]);
    }

    /// `TestMsgDataEndpointsRoundTrip`.
    #[tokio::test]
    async fn msg_data_endpoints_round_trip() {
        let m = ProtocolMsg {
            typ: T_ACCEPT,
            token: vec![1],
            data_ports: vec![4001, 4002],
            data_endpoints: vec![
                ProtocolDataEndpointRec {
                    id: Some(vec![7; 32]),
                    addrs: vec![
                        "ip:192.168.1.5:4001".into(),
                        "relay:https://euc1-1.relay.example./".into(),
                    ],
                },
                ProtocolDataEndpointRec {
                    id: Some(vec![8; 32]),
                    addrs: vec!["ip:192.168.1.5:4002".into()],
                },
            ],
            ..Default::default()
        };
        let f = frame(&m);
        let got = read_protocol_msg(&mut &f[..])
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(got, m);
    }

    cbor_struct! {
        #[derive(Clone, Debug, Default, PartialEq)]
        struct OldMsg = "protocol.oldMsg" {
            0 => typ: i64 = "int",
            13 => token: Vec<u8> = "[]uint8" [omitempty],
            15 => data_ports: Vec<u16> = "[]uint16" [omitempty],
        }
    }

    /// `TestMsgDataEndpointsCompat`: old peers ignore key 16; its absence decodes as empty.
    #[test]
    fn msg_data_endpoints_compat() {
        let m = ProtocolMsg {
            typ: T_ACCEPT,
            token: vec![1],
            data_ports: vec![4001],
            data_endpoints: vec![ProtocolDataEndpointRec {
                id: Some(vec![7; 32]),
                addrs: vec!["ip:127.0.0.1:4001".into()],
            }],
            ..Default::default()
        };
        let old: OldMsg = dstore_codec::unmarshal(&dstore_codec::marshal(&m))
            .unwrap_or_else(|e| panic!("old decoder rejects new frame: {e}"));
        assert_eq!(
            old,
            OldMsg {
                typ: T_ACCEPT,
                token: vec![1],
                data_ports: vec![4001],
            }
        );
        let new: ProtocolMsg =
            dstore_codec::unmarshal(&dstore_codec::marshal(&old)).unwrap_or_else(|e| panic!("{e}"));
        assert!(new.data_endpoints.is_empty());
    }

    /// `TestMsgPinNamesRoundTrip`.
    #[tokio::test]
    async fn msg_pin_names_round_trip() {
        let m = ProtocolMsg {
            typ: T_PIN,
            names: vec!["a/b".into(), "c".into()],
            ..Default::default()
        };
        let f = frame(&m);
        let got = read_protocol_msg(&mut &f[..])
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!((got.typ, got.names), (13, m.names));
    }

    // ---- transport-iroh protocol/pack_test.go ----

    /// `TestSendPackRecordsRoundTrip`: pre-encoded records pass through untouched.
    #[tokio::test]
    async fn send_pack_records_round_trip() {
        let objs: Vec<Object> = (0..4u8)
            .map(|i| blob(&vec![i + 1; 100 + usize::from(i)]))
            .collect();
        let recs: Vec<Vec<u8>> = objs.iter().map(record).collect();
        let stream = send(&recs).await;
        let mut rest: &[u8] = &stream;
        let mut pr = PackRecords::new(PackReader::new(&mut rest));
        let (got, err) = collect_records(&mut pr).await;
        assert!(err.is_none(), "{err:?}");
        assert_eq!(got.len(), objs.len());
        for (raw, o) in got.iter().zip(&objs) {
            let payload = decode_payload(
                raw.record.flags,
                raw.record.ulen,
                &raw.bytes[REC_HEADER_SIZE..],
            )
            .unwrap_or_else(|e| panic!("{e}"));
            assert_eq!((raw.record.key, payload), (o.key, o.bytes.clone()));
        }
        pr.drain().await.unwrap_or_else(|e| panic!("drain: {e}"));
        drop(pr);
        assert!(rest.is_empty(), "the stream is positioned after TDataEnd");
    }

    /// `TestSendPackRecordsPropagatesSourceError`: the caller returns its source error; a sender dropped
    /// before anything filled a chunk writes nothing (no magic, no terminator).
    #[tokio::test]
    async fn send_pack_records_propagates_source_error() {
        let mut out = Vec::new();
        drop(PackSender::new(&mut out));
        assert!(out.is_empty());
        {
            let mut s = PackSender::new(&mut out);
            for o in [blob(b"a"), blob(&incompressible(5000))] {
                s.add_record(&record(&o))
                    .await
                    .unwrap_or_else(|e| panic!("{e}"));
            }
        }
        assert!(out.is_empty(), "{} bytes written", out.len());
    }

    /// `TestPackReaderSurfacesRemoteError`.
    #[tokio::test]
    async fn pack_reader_surfaces_remote_error() {
        let mut stream = tdata(b"junk");
        stream.extend_from_slice(&frame(&ProtocolMsg {
            typ: T_ERR,
            code: CODE_INTERNAL.into(),
            text: "sender died".into(),
            ..Default::default()
        }));
        let mut pr = PackReader::new(&stream[..]);
        let mut buf = [0u8; 16];
        let mut got = Vec::new();
        let err = loop {
            match pr.read(&mut buf).await {
                Ok(0) => panic!("EOF before the remote error"),
                Ok(n) => got.extend_from_slice(&buf[..n]),
                Err(e) => break e,
            }
        };
        assert_eq!(got, b"junk");
        assert!(
            matches!(&err, PackReadError::Remote(re) if re.code == CODE_INTERNAL),
            "{err:?}"
        );
        assert_eq!(err.to_string(), "remote: internal: sender died");
    }

    /// `TestPackReaderRejectsUnexpectedFrame`.
    #[tokio::test]
    async fn pack_reader_rejects_unexpected_frame() {
        let stream = frame(&msg(T_WANTS));
        let mut pr = PackReader::new(&stream[..]);
        let err = pr.read(&mut [0u8; 8]).await.err();
        assert!(matches!(err, Some(PackReadError::Unexpected(6))));
        assert_eq!(
            err.map(|e| e.to_string()).as_deref(),
            Some("protocol: unexpected frame: type 6 during pack transfer")
        );
    }

    // ---- dstore wire/wire_test.go ----

    /// `TestPackFramesInterop`: the pack frames decode as `wire.Msg` too, and the stream is positioned after
    /// TDataEnd once the records are read and the reader drained.
    #[tokio::test]
    async fn pack_frames_interop() {
        let data = b"some blob bytes";
        let k = Key::new(Type::Blob, data.len() as u64, data);
        let rec = encode_record(k, data).unwrap_or_else(|e| panic!("{e}"));
        let stream = send(&[rec]).await;
        let first = read_msg(&mut &stream[..])
            .await
            .unwrap_or_else(|e| panic!("first frame: {e}"));
        assert!(first.typ == T_DATA && !first.data.is_empty(), "{first:?}");
        let mut rest: &[u8] = &stream;
        let mut pr = PackRecords::new(PackReader::new(&mut rest));
        let (got, err) = collect_records(&mut pr).await;
        assert!(err.is_none(), "{err:?}");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].record.key, k);
        pr.drain().await.unwrap_or_else(|e| panic!("drain: {e}"));
        drop(pr);
        assert!(matches!(read_msg(&mut rest).await, Err(WireError::Eof)));
    }

    // ---- core amberpack/pack_test.go (Records, over TData frames) ----

    /// `TestWriter_AddRecord_RoundTrip`.
    #[tokio::test]
    async fn records_add_record_round_trip() {
        let objs = [blob(b"alpha"), blob(&b"amber".repeat(50_000))];
        let recs: Vec<Vec<u8>> = objs.iter().map(record).collect();
        assert_eq!(recs[1][33], 1, "the amber record is compressed");
        let (got, err) = records_of(&send(&recs).await).await;
        assert!(err.is_none(), "{err:?}");
        assert_eq!(got.len(), objs.len());
        for (raw, o) in got.iter().zip(&objs) {
            let payload = decode_payload(
                raw.record.flags,
                raw.record.ulen,
                &raw.bytes[REC_HEADER_SIZE..],
            )
            .unwrap_or_else(|e| panic!("{e}"));
            assert_eq!((raw.record.key, payload), (o.key, o.bytes.clone()));
        }
    }

    /// `TestReader_Records_RoundTrip`.
    #[tokio::test]
    async fn records_round_trip() {
        let objs = [
            blob(b"alpha"),
            blob(b""),
            blob(&b"amber".repeat(50_000)),
            blob(&incompressible(4000)),
        ];
        let want: Vec<Vec<u8>> = objs.iter().map(record).collect();
        let (got, err) = records_of(&send(&want).await).await;
        assert!(err.is_none(), "{err:?}");
        assert_eq!(got.len(), objs.len());
        for ((raw, o), w) in got.iter().zip(&objs).zip(&want) {
            assert_eq!(&raw.bytes, w);
            assert_eq!(raw.record.key, o.key);
            assert_eq!(raw.record.ulen as usize, o.bytes.len());
            assert_eq!(raw.bytes.len(), REC_HEADER_SIZE + raw.record.slen as usize);
        }
        let again: Vec<Vec<u8>> = got.into_iter().map(|r| r.bytes).collect();
        let (decoded, err) = records_of(&send(&again).await).await;
        assert!(err.is_none(), "{err:?}");
        for (raw, o) in decoded.iter().zip(&objs) {
            let payload = decode_payload(
                raw.record.flags,
                raw.record.ulen,
                &raw.bytes[REC_HEADER_SIZE..],
            )
            .unwrap_or_else(|e| panic!("{e}"));
            assert_eq!(payload, o.bytes);
        }
    }

    fn assert_malformed(err: Option<PackRecordsError>, want: &str) {
        let text = err.map(|e| e.to_string());
        assert_eq!(text.as_deref(), Some(want));
    }

    /// `TestReader_RejectsLegacyVersions`.
    #[tokio::test]
    async fn records_reject_legacy_versions() {
        for magic in [b"AMBERPK\x01", b"AMBERPK\x02"] {
            let mut pack = magic.to_vec();
            pack.push(TAG_END);
            let mut stream = tdata(&pack);
            stream.extend_from_slice(&tdata_end());
            let (_, err) = records_of(&stream).await;
            assert_malformed(err, "amberpack: malformed pack stream: bad magic");
        }
    }

    /// `TestWriterReader_EmptyStreamIsValid`.
    #[tokio::test]
    async fn records_empty_stream_is_valid() {
        let stream = send(&[]).await;
        assert_eq!(
            hex(&stream),
            "0000000ea200070849414d424552504b030000000003a10008"
        );
        let (got, err) = records_of(&stream).await;
        assert!(got.is_empty() && err.is_none(), "{err:?}");
    }

    /// `TestReader_BadMagic`.
    #[tokio::test]
    async fn records_bad_magic() {
        let mut stream = tdata(b"NOTAMBER...");
        stream.extend_from_slice(&tdata_end());
        let (_, err) = records_of(&stream).await;
        assert_malformed(err, "amberpack: malformed pack stream: bad magic");
    }

    /// `TestReader_TruncatedMissingEndMarker` and `TestReader_Records_Truncated`.
    #[tokio::test]
    async fn records_truncated_missing_end_marker() {
        let rec = record(&blob(b"data"));
        let (got, err) = records_of(&framed_pack(&rec)).await;
        assert_eq!(got.len(), 1, "the record before the truncation is yielded");
        assert_malformed(
            err,
            "amberpack: malformed pack stream: truncated before end marker: EOF",
        );
    }

    /// `TestReader_NonCanonicalKeyRejected`.
    #[tokio::test]
    async fn records_non_canonical_key_rejected() {
        let mut rec = record(&blob(b"payload"));
        rec[1] = 0xf0;
        recrc(&mut rec);
        rec.push(TAG_END);
        let (_, err) = records_of(&framed_pack(&rec)).await;
        assert!(
            matches!(&err, Some(PackRecordsError::Record(e)) if e.is_malformed()),
            "{err:?}"
        );
        assert_malformed(
            err,
            "amberpack: malformed pack stream: amberpack: corrupt pack data: record key: key: reserved object type: 15",
        );
    }

    /// `TestReader_BadRecordTag`.
    #[tokio::test]
    async fn records_bad_record_tag() {
        let (_, err) = records_of(&framed_pack(&[0x42])).await;
        assert_malformed(err, "amberpack: malformed pack stream: bad record tag 0x42");
    }

    /// `TestReader_TruncatedPayload`.
    #[tokio::test]
    async fn records_truncated_payload() {
        let rec = record(&blob(&incompressible(100)));
        let (_, err) = records_of(&framed_pack(&rec[..REC_HEADER_SIZE + 5])).await;
        assert_malformed(
            err,
            "amberpack: malformed pack stream: truncated record payload: unexpected EOF",
        );
    }

    /// `TestReader_RecordCRCMismatch` and `TestReader_Records_CRCMismatch`.
    #[tokio::test]
    async fn records_crc_mismatch() {
        let mut rec = record(&blob(&incompressible(64)));
        if let Some(last) = rec.last_mut() {
            *last ^= 0x01;
        }
        rec.push(TAG_END);
        let (_, err) = records_of(&framed_pack(&rec)).await;
        assert_malformed(
            err,
            "amberpack: malformed pack stream: amberpack: corrupt pack data: record CRC mismatch",
        );
    }

    /// `TestReader_OversizedPayloadRejected`: refused before the payload is read.
    #[tokio::test]
    async fn records_oversized_payload_rejected() {
        let rec = record(&blob(b"x"));
        let mut hdr = rec[..REC_HEADER_SIZE].to_vec();
        hdr[38..42].copy_from_slice(&(MAX_PAYLOAD + 1).to_be_bytes());
        let (_, err) = records_of(&framed_pack(&hdr)).await;
        assert_malformed(
            err,
            "amberpack: malformed pack stream: record payload 268435457 exceeds limit 268435456",
        );
    }

    // ---- behaviour pinned against Go (scratch programs over transport-iroh v0.4.0) ----

    /// TData frames are canonical `protocol.Msg` frames holding keys 0 and 8 only.
    #[tokio::test]
    async fn tdata_frames_are_protocol_msg_frames() {
        for n in [1, 23, 24, 255, 256, 65535, 65536, CHUNK_SIZE] {
            let data = incompressible(n);
            let mut out = Vec::new();
            write_tdata(&mut out, &data)
                .await
                .unwrap_or_else(|e| panic!("{e}"));
            assert_eq!(out, tdata(&data), "len {n}");
        }
        assert_eq!(TDATA_END_FRAME.to_vec(), tdata_end());
    }

    /// Go's `SendPackRecords` over records of these sizes (record i is `i + 1` repeated; AddRecord does not
    /// validate): the frames of the finished pack, and the frames already written when the source fails
    /// after the last record.
    ///
    /// All rows were measured with scratch Go programs over transport-iroh v0.4.0. First the wire-pack
    /// author's four cases. Then review-wire-pack's rows: a grid that puts the 1 MiB boundary 1, 4087,
    /// 4095, 4096, 4097, 8191 or 8192 bytes after the first big record, with and without a small first
    /// record, and a few rows of a random sweep. Rows marked `window` are those where a sender without
    /// bufio's 4096-byte buffer would already have written a frame. The review replayed all 248 Go rows
    /// (110 window rows) through `PackSender` with no difference; these are a subset.
    #[tokio::test]
    async fn bufio_window_matches_go() {
        const C: usize = CHUNK_SIZE;
        let cases: &[(&[usize], &[usize], &[usize])] = &[
            // wire-pack
            (&[C - 108, 200], &[1048585, 107, 3], &[]), // window
            (&[C - 108, 5000], &[1048585, 4908, 3], &[1048585]),
            (
                &[C - 3008, 1000, 1000, 1000, 1000],
                &[1048585, 1008, 3],
                &[],
            ), // window
            (&[4088, C - 4146, 100], &[1048585, 57, 3], &[]), // window
            // review-wire-pack grid
            (&[1048567, 0], &[1048585, 3], &[]),
            (&[1048567, 1], &[1048585, 6, 3], &[]), // window
            (&[1048567, 4096], &[1048585, 4103, 3], &[]), // window
            (&[1048567, 4097], &[1048585, 4104, 3], &[1048585]),
            (&[1048567, 1, 1, 1], &[1048585, 8, 3], &[]), // window
            (&[4088, 1044479, 1], &[1048585, 6, 3], &[]), // window
            (&[4088, 1044479, 4096], &[1048585, 4103, 3], &[]), // window
            (&[100, 1048467, 4096], &[1048585, 4103, 3], &[]), // window
            (&[1044481, 4086], &[1048585, 3], &[]),
            (&[1044481, 4087], &[1048585, 6, 3], &[]), // window
            (&[1044481, 2043, 2045], &[1048585, 7, 3], &[]), // window
            (&[4088, 1040393, 4096], &[1048585, 15, 3], &[]), // window
            (&[1044473, 2047, 2049], &[1048585, 7, 3], &[]), // window
            (&[1044473, 1, 1, 4095], &[1048585, 8, 3], &[1048585]),
            (&[1044472, 4096], &[1048585, 6, 3], &[]), // window
            (&[1044472, 4097], &[1048585, 7, 3], &[1048585]),
            (&[1044472, 2048, 2049], &[1048585, 7, 3], &[1048585]),
            (&[4088, 1040384, 4096], &[1048585, 6, 3], &[]), // window
            (&[4088, 1040384, 4097], &[1048585, 7, 3], &[1048585]),
            (&[1044471, 2048, 2050], &[1048585, 7, 3], &[]), // window
            (&[1044471, 1, 1, 4097], &[1048585, 8, 3], &[]), // window
            (&[4088, 1040383, 4096], &[1048585, 3], &[]),
            (&[1040377, 4095, 4097], &[1048585, 7, 3], &[]), // window
            (&[1040377, 1, 1, 8191], &[1048585, 8, 3], &[1048585]),
            (&[1040376, 4096, 4097], &[1048585, 7, 3], &[1048585]),
            // review-wire-pack sweep
            (&[1046184, 3876], &[1048585, 1500, 3], &[]), // window
            (
                &[2094381, 4370, 242],
                &[1048585, 1048585, 1857, 3],
                &[1048585, 1048585],
            ),
            (&[2096560], &[1048585, 1048002, 3], &[1048585]),
            (
                &[4097, 1040794, 4097, 0, 245],
                &[1048585, 673, 3],
                &[1048585],
            ),
            (
                &[0, 1047073, 123, 4097, 4097, 3741, 4096],
                &[1048585, 14667, 3],
                &[1048585],
            ),
        ];
        for &(sizes, finished, dropped) in cases {
            let recs: Vec<Vec<u8>> = sizes
                .iter()
                .enumerate()
                .map(|(i, &n)| vec![i as u8 + 1; n])
                .collect();
            let stream = send(&recs).await;
            assert_eq!(frame_lens(&stream), finished, "finished {sizes:?}");
            let mut out = Vec::new();
            {
                let mut s = PackSender::new(&mut out);
                for r in &recs {
                    s.add_record(r).await.unwrap_or_else(|e| panic!("{e}"));
                }
            }
            assert_eq!(frame_lens(&out), dropped, "dropped {sizes:?}");
            assert!(stream.starts_with(&out));
        }
    }

    /// Go: `Read(nil)` reads frames until one carries data (an empty TData and a TData "xy" here), leaving
    /// TDataEnd unread.
    #[tokio::test]
    async fn read_with_empty_buffer_waits_for_data() {
        let mut stream = tdata(b"");
        stream.extend_from_slice(&tdata(b"xy"));
        stream.extend_from_slice(&tdata_end());
        let mut rest: &[u8] = &stream;
        let mut pr = PackReader::new(&mut rest);
        assert_eq!(pr.read(&mut []).await.ok(), Some(0));
        let mut buf = [0u8; 8];
        assert_eq!(pr.read(&mut buf).await.ok(), Some(2));
        assert_eq!(&buf[..2], b"xy");
        assert_eq!(pr.read(&mut buf).await.ok(), Some(0));
        assert_eq!(pr.read(&mut buf).await.ok(), Some(0));
        drop(pr);
        assert!(rest.is_empty());
    }

    #[tokio::test]
    async fn drain_counts_the_discarded_bytes() {
        let mut stream = tdata(b"abc");
        stream.extend_from_slice(&tdata(b"de"));
        stream.extend_from_slice(&tdata_end());
        stream.extend_from_slice(&tdata(b"next"));
        let mut rest: &[u8] = &stream;
        let mut pr = PackReader::new(&mut rest);
        let mut one = [0u8; 1];
        assert_eq!(pr.read(&mut one).await.ok(), Some(1));
        assert_eq!(pr.drain().await.ok(), Some(4));
        assert_eq!(pr.drain().await.ok(), Some(0));
        assert!(pr.into_inner().starts_with(&tdata(b"next")));
    }

    /// A clean end at a frame boundary is EOF, and stays EOF.
    #[tokio::test]
    async fn eof_at_a_frame_boundary_is_eof() {
        let stream = tdata(b"abc");
        let mut pr = PackReader::new(&stream[..]);
        let mut buf = [0u8; 8];
        assert_eq!(pr.read(&mut buf).await.ok(), Some(3));
        assert_eq!(pr.read(&mut buf).await.ok(), Some(0));
        assert_eq!(pr.read(&mut buf).await.ok(), Some(0));
        assert_eq!(pr.drain().await.ok(), Some(0));
    }

    /// Sticky errors keep their Go text, including I/O errors, which are copied.
    #[tokio::test]
    async fn sticky_errors_keep_their_text() {
        let mut items: VecDeque<io::Result<Vec<u8>>> = VecDeque::new();
        items.push_back(Ok(tdata(b"ab")));
        items.push_back(Err(io::Error::from_raw_os_error(54)));
        let mut pr = PackReader::new(Script(items));
        let mut buf = [0u8; 8];
        assert_eq!(pr.read(&mut buf).await.ok(), Some(2));
        let first = pr.read(&mut buf).await.err().map(|e| e.to_string());
        let again = pr.read(&mut buf).await.err();
        assert!(
            matches!(&again, Some(PackReadError::Frame(ProtocolFrameError::Io(e))) if e.raw_os_error() == Some(54)),
            "{again:?}"
        );
        assert_eq!(again.map(|e| e.to_string()), first);
        let drained = pr.drain().await.err().map(|e| e.to_string());
        assert_eq!(drained, first);

        let mut items: VecDeque<io::Result<Vec<u8>>> = VecDeque::new();
        items.push_back(Ok(vec![0, 0, 0, 9, 0xa2]));
        items.push_back(Err(io::Error::new(
            io::ErrorKind::ConnectionReset,
            "stream reset by peer: error 0",
        )));
        let mut pr = PackReader::new(Script(items));
        let first = pr.read(&mut buf).await.err().map(|e| e.to_string());
        assert_eq!(
            first.as_deref(),
            Some("protocol: short frame: stream reset by peer: error 0")
        );
        assert_eq!(pr.read(&mut buf).await.err().map(|e| e.to_string()), first);
    }

    /// One byte per underlying read: the counting loops see every partial read.
    #[tokio::test]
    async fn bytewise_reads_give_the_same_records() {
        let recs: Vec<Vec<u8>> = [blob(b"alpha"), blob(&incompressible(3000))]
            .iter()
            .map(record)
            .collect();
        let stream = send(&recs).await;
        let (got, err) = records_of(&stream).await;
        let mut slow = PackRecords::new(PackReader::new(Script::bytewise(&stream)));
        let (slow_got, slow_err) = collect_records(&mut slow).await;
        assert!(err.is_none() && slow_err.is_none());
        assert_eq!(got, slow_got);
        assert_eq!(slow.drain().await.ok(), Some(0));

        let truncated = &stream[..stream.len() - 20];
        let (_, err) = records_of(truncated).await;
        let (_, slow_err) = collect_records(&mut PackRecords::new(PackReader::new(
            Script::bytewise(truncated),
        )))
        .await;
        assert_eq!(err.map(|e| e.to_string()), slow_err.map(|e| e.to_string()));
    }

    /// A write error is returned, then returned again by every later call (bufio's sticky error).
    #[tokio::test]
    async fn write_errors_are_sticky() {
        let mut w = FailAfter {
            limit: 100,
            written: Vec::new(),
        };
        let mut s = PackSender::new(&mut w);
        let big = incompressible(CHUNK_SIZE + 10);
        let first = s.add_record(&big).await.err().map(|e| e.to_string());
        assert_eq!(first.as_deref(), Some("broken pipe"));
        let again = s.add_record(b"x").await.err().map(|e| e.to_string());
        assert_eq!(again, first);
        let fin = s.finish().await.err().map(|e| e.to_string());
        assert_eq!(fin, first);
        assert_eq!(w.written.len(), 100);
    }
}
