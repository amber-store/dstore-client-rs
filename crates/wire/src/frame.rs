//! Frames: `u32be len ‖ canonical CBOR` (`wire.ReadMsg`, `WriteMsg`, `Expect`, `WriteErr`,
//! `wire/wire.go:246-350`).

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::consts::{MAX_FRAME, T_ERR};
use crate::error::{RemoteError, err_msg, error_from_msg};
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
    let payload = dstore_codec::marshal(m);
    let len = match u32::try_from(payload.len()) {
        Ok(n) if payload.len() <= MAX_FRAME => n,
        _ => return Err(WireError::TooLarge(payload.len() as u64)),
    };
    let mut frame = Vec::with_capacity(4 + payload.len());
    frame.extend_from_slice(&len.to_be_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

/// `wire.WriteMsg`: one `write_all`.
pub async fn write_msg<W: AsyncWrite + Unpin + ?Sized>(
    w: &mut W,
    m: &Msg,
) -> Result<(), WireError> {
    let frame = encode_frame(m)?;
    w.write_all(&frame).await.map_err(WireError::Io)
}

/// `wire.ReadMsg`: the counting header loop; not cancel-safe.
///
/// - 0 header bytes, then EOF → `Eof`; 1-3 bytes, then EOF → `UnexpectedEof`; another error → `Io`
///   (an error of kind `UnexpectedEof` → `UnexpectedEof`, as Go's `errors.Is(err, io.ErrUnexpectedEOF)`).
/// - The length is checked against `MAX_FRAME` before the payload is allocated.
/// - A short payload → `Short(Eof)` when no payload byte came, `Short(UnexpectedEof)` when some did.
pub async fn read_msg<R: AsyncRead + Unpin + ?Sized>(r: &mut R) -> Result<Msg, WireError> {
    let mut hdr = [0u8; 4];
    match read_full(r, &mut hdr).await {
        Ok(()) => {}
        Err(Partial::Eof) => return Err(WireError::Eof),
        Err(Partial::UnexpectedEof) => return Err(WireError::UnexpectedEof),
        Err(Partial::Io(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
            return Err(WireError::UnexpectedEof);
        }
        Err(Partial::Io(e)) => return Err(WireError::Io(e)),
    }
    let n = u32::from_be_bytes(hdr);
    let len = match usize::try_from(n) {
        Ok(len) if len <= MAX_FRAME => len,
        _ => return Err(WireError::TooLarge(u64::from(n))),
    };
    let mut payload = vec![0u8; len];
    if let Err(p) = read_full(r, &mut payload).await {
        return Err(WireError::Short(match p {
            Partial::Eof => ShortCause::Eof,
            Partial::UnexpectedEof => ShortCause::UnexpectedEof,
            Partial::Io(e) => ShortCause::Io(e),
        }));
    }
    dstore_codec::unmarshal::<Msg>(&payload).map_err(WireError::Decode)
}

/// `wire.Expect`.
///
/// Go also returns the frame alongside a TErr or type-mismatch error. No Go caller reads it
/// (`client/fetch.go:288`, `client/objects.go:292`), and the TErr fields are kept in `RemoteError`.
pub async fn expect<R: AsyncRead + Unpin + ?Sized>(r: &mut R, want: i64) -> Result<Msg, WireError> {
    let m = read_msg(r).await?;
    if m.typ == T_ERR {
        return Err(WireError::Remote(error_from_msg(&m)));
    }
    if want != 0 && m.typ != want {
        return Err(WireError::Protocol { got: m.typ, want });
    }
    Ok(m)
}

/// `wire.WriteErr`.
pub async fn write_err<W: AsyncWrite + Unpin + ?Sized>(
    w: &mut W,
    code: &str,
    text: &str,
) -> Result<(), WireError> {
    write_msg(w, &err_msg(code, text)).await
}

/// Why [`read_full`] stopped before filling its buffer (Go `io.ReadFull`).
enum Partial {
    /// No byte came before the end of the stream.
    Eof,
    /// Some bytes came, then the end of the stream.
    UnexpectedEof,
    Io(std::io::Error),
}

/// Go `io.ReadFull` over an async reader: counts the bytes, so a clean end is told from a cut one
/// (tokio `read_exact` reports `UnexpectedEof` for both, codec-wire-ticket K9).
async fn read_full<R: AsyncRead + Unpin + ?Sized>(
    r: &mut R,
    buf: &mut [u8],
) -> Result<(), Partial> {
    let mut got = 0usize;
    while let Some(rest) = buf.get_mut(got..).filter(|rest| !rest.is_empty()) {
        match r.read(rest).await {
            Ok(0) => {
                return Err(if got == 0 {
                    Partial::Eof
                } else {
                    Partial::UnexpectedEof
                });
            }
            Ok(n) => got += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(Partial::Io(e)),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::io;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    use tokio::io::ReadBuf;

    use super::*;
    use crate::consts::{CODE_STALE_VIEW, T_DATA, T_MISSING, T_OK, T_REF};
    use crate::error::{as_remote, is_code};
    use crate::msg::KeyHolders;

    /// A reader that delivers scripted chunks (at most `buf.remaining()` bytes each), then EOF.
    struct Script(VecDeque<io::Result<Vec<u8>>>);

    impl Script {
        fn new(items: Vec<io::Result<Vec<u8>>>) -> Script {
            Script(items.into())
        }

        /// One byte per read.
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

    /// A writer that records every `poll_write` call, accepts at most `limit` bytes per call, or fails with an OS
    /// error.
    #[derive(Default)]
    struct Recorder {
        writes: Vec<Vec<u8>>,
        fail: Option<i32>,
        limit: Option<usize>,
    }

    impl AsyncWrite for Recorder {
        fn poll_write(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            if let Some(code) = self.fail {
                return Poll::Ready(Err(io::Error::from_raw_os_error(code)));
            }
            let n = self.limit.map_or(buf.len(), |l| l.min(buf.len()));
            self.writes.push(buf[..n].to_vec());
            Poll::Ready(Ok(n))
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    fn frame_of(payload: &[u8]) -> Vec<u8> {
        let mut f = (payload.len() as u32).to_be_bytes().to_vec();
        f.extend_from_slice(payload);
        f
    }

    /// dstore `wire/wire_test.go` `TestFrameRoundTrip`.
    #[tokio::test]
    async fn frame_round_trip() {
        let mut buf = Vec::new();
        let input = Msg {
            typ: T_MISSING,
            epoch: 9,
            keys: vec![vec![0; 32]],
            pin: true,
            holders: vec![KeyHolders {
                key: Some(vec![1]),
                holders: vec![vec![2]],
            }],
            ..Msg::default()
        };
        write_msg(&mut buf, &input).await.expect("write");
        let mut r = &buf[..];
        let out = read_msg(&mut r).await.expect("read");
        assert_eq!(out, input);
        assert!(matches!(read_msg(&mut r).await, Err(WireError::Eof)));
    }

    /// dstore `wire/wire_test.go` `TestErrorFrames`. Rust's `expect` does not hand back the frame with
    /// the error (Go's `m != nil`); the frame's fields travel in the `RemoteError`.
    #[tokio::test]
    async fn error_frames() {
        let mut buf = Vec::new();
        let m = Msg {
            typ: T_ERR,
            code: CODE_STALE_VIEW.into(),
            text: "behind".into(),
            view: vec![1, 2],
            ..Msg::default()
        };
        write_msg(&mut buf, &m).await.expect("write");
        let err = match expect(&mut &buf[..], T_OK).await {
            Ok(m) => panic!("expected stale-view, got {m:?}"),
            Err(e) => e,
        };
        assert!(is_code(&err, CODE_STALE_VIEW), "{err}");
        let we = as_remote(&err).expect("remote error");
        assert_eq!(we.view, b"\x01\x02");
        assert_eq!(err.to_string(), "remote: stale-view: behind");
    }

    #[tokio::test]
    async fn header_loop_tells_clean_eof_from_partial_header() {
        assert!(matches!(
            read_msg(&mut Script::new(vec![])).await,
            Err(WireError::Eof)
        ));
        for n in 1..4 {
            let hdr = [0u8, 0, 0, 4];
            let err = read_msg(&mut Script::bytewise(&hdr[..n])).await;
            assert!(matches!(err, Err(WireError::UnexpectedEof)), "{n} bytes");
        }
        // A frame delivered one byte at a time.
        let frame = frame_of(&[0xa1, 0x00, 0x18, 0x35]);
        let m = read_msg(&mut Script::bytewise(&frame)).await.expect("read");
        assert_eq!(m.typ, T_OK);
    }

    #[tokio::test]
    async fn header_io_errors_pass_through() {
        let mut r = Script::new(vec![
            Ok(vec![0, 0]),
            Err(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "stream reset",
            )),
        ]);
        let err = read_msg(&mut r).await.expect_err("reset");
        assert!(matches!(err, WireError::Io(_)), "{err:?}");
        assert_eq!(err.to_string(), "stream reset");

        let mut r = Script::new(vec![Err(io::Error::from_raw_os_error(54))]);
        let err = read_msg(&mut r).await.expect_err("errno");
        assert_eq!(
            err.to_string(),
            dstore_gocompat::errno::io_error_text(&io::Error::from_raw_os_error(54))
        );

        let mut r = Script::new(vec![Err(io::Error::from(io::ErrorKind::UnexpectedEof))]);
        assert!(matches!(
            read_msg(&mut r).await,
            Err(WireError::UnexpectedEof)
        ));
    }

    #[tokio::test]
    async fn too_large_is_refused_before_the_payload_is_read() {
        let mut r = Script::new(vec![
            Ok(vec![0x01, 0x00, 0x00, 0x01]),
            Err(io::Error::other("payload must not be read")),
        ]);
        let err = read_msg(&mut r).await.expect_err("too large");
        assert!(matches!(err, WireError::TooLarge(16_777_217)), "{err:?}");
        assert_eq!(
            err.to_string(),
            "wire: frame of 16777217 bytes exceeds limit 16777216"
        );
        let mut r = &[0xff, 0xff, 0xff, 0xff][..];
        let err = read_msg(&mut r).await.expect_err("too large");
        assert_eq!(
            err.to_string(),
            "wire: frame of 4294967295 bytes exceeds limit 16777216"
        );
    }

    /// Go's `io.ReadFull` never sees EINTR (the runtime retries it); `read_full` retries `Interrupted` in both
    /// phases.
    #[tokio::test]
    async fn interrupted_reads_are_retried() {
        let frame = frame_of(&[0xa1, 0x00, 0x18, 0x35]);
        let intr = || Err(io::Error::from(io::ErrorKind::Interrupted));
        let mut r = Script::new(vec![
            intr(),
            Ok(frame[..2].to_vec()),
            intr(),
            Ok(frame[2..6].to_vec()),
            intr(),
            Ok(frame[6..].to_vec()),
        ]);
        assert_eq!(read_msg(&mut r).await.expect("read").typ, T_OK);
        assert!(matches!(read_msg(&mut r).await, Err(WireError::Eof)));
    }

    /// Payload phase: an io error of kind `UnexpectedEof` stays wrapped, as Go's `%w` keeps the reader's
    /// error, and reads "unexpected EOF".
    #[tokio::test]
    async fn payload_unexpected_eof_error_is_a_short_frame() {
        let mut r = Script::new(vec![
            Ok(vec![0, 0, 0, 5, 0xa1]),
            Err(io::Error::from(io::ErrorKind::UnexpectedEof)),
        ]);
        let err = read_msg(&mut r).await.expect_err("short");
        assert!(
            matches!(err, WireError::Short(ShortCause::Io(_))),
            "{err:?}"
        );
        assert_eq!(err.to_string(), "wire: short frame: unexpected EOF");
    }

    #[tokio::test]
    async fn short_frames() {
        let err = read_msg(&mut &[0, 0, 0, 5][..]).await.expect_err("short");
        assert_eq!(err.to_string(), "wire: short frame: EOF");
        let err = read_msg(&mut &[0, 0, 0, 5, 0xa1][..])
            .await
            .expect_err("short");
        assert_eq!(err.to_string(), "wire: short frame: unexpected EOF");
        let mut r = Script::new(vec![
            Ok(vec![0, 0, 0, 5, 0xa1]),
            Err(io::Error::other("stopped")),
        ]);
        let err = read_msg(&mut r).await.expect_err("short");
        assert_eq!(err.to_string(), "wire: short frame: stopped");
    }

    #[tokio::test]
    async fn decode_errors() {
        let err = read_msg(&mut &[0, 0, 0, 0][..]).await.expect_err("empty");
        assert_eq!(err.to_string(), "wire: decode frame: EOF");
        let err = read_msg(&mut &frame_of(&[0x81, 0x00])[..])
            .await
            .expect_err("array");
        assert_eq!(
            err.to_string(),
            "wire: decode frame: cbor: cannot unmarshal array into Go value of type wire.Msg (cannot decode CBOR array to struct without toarray option)"
        );
    }

    #[tokio::test]
    async fn write_msg_is_one_write_and_nothing_when_too_large() {
        let mut w = Recorder::default();
        let m = Msg {
            typ: T_OK,
            ..Msg::default()
        };
        write_msg(&mut w, &m).await.expect("write");
        assert_eq!(w.writes, [vec![0, 0, 0, 4, 0xa1, 0x00, 0x18, 0x35]]);

        // {0: 7, 8: bstr(16777208)}: 4 bytes of map head, keys and type, a 5-byte bstr head, the data.
        let big = Msg {
            typ: T_DATA,
            data: vec![0x5a; MAX_FRAME - 8],
            ..Msg::default()
        };
        let mut w = Recorder::default();
        let err = write_msg(&mut w, &big).await.expect_err("too large");
        assert_eq!(
            err.to_string(),
            "wire: frame of 16777217 bytes exceeds limit 16777216"
        );
        assert!(w.writes.is_empty());
        assert!(matches!(
            encode_frame(&big),
            Err(WireError::TooLarge(16_777_217))
        ));

        let max = Msg {
            typ: T_DATA,
            data: vec![0x5a; MAX_FRAME - 9],
            ..Msg::default()
        };
        let frame = encode_frame(&max).expect("max frame");
        assert_eq!(frame.len(), 4 + MAX_FRAME);
        assert_eq!(
            frame[..13],
            [1, 0, 0, 0, 0xa2, 0, 7, 8, 0x5a, 0, 0xff, 0xff, 0xf7]
        );
    }

    /// Go's one `w.Write(buf)` writes the whole frame or fails; a writer taking a few bytes per call gets the
    /// rest in later calls.
    #[tokio::test]
    async fn write_msg_completes_short_writes() {
        let m = err_msg("busy", "slow down");
        let want = encode_frame(&m).expect("frame");
        let mut w = Recorder {
            limit: Some(3),
            ..Recorder::default()
        };
        write_msg(&mut w, &m).await.expect("write");
        assert!(w.writes.len() > 1);
        assert_eq!(w.writes.concat(), want);
    }

    #[tokio::test]
    async fn write_errors_are_returned_unwrapped() {
        let mut w = Recorder {
            fail: Some(32),
            ..Recorder::default()
        };
        let err = write_err(&mut w, "busy", "").await.expect_err("EPIPE");
        assert!(matches!(err, WireError::Io(_)));
        assert_eq!(
            err.to_string(),
            dstore_gocompat::errno::io_error_text(&io::Error::from_raw_os_error(32))
        );
    }

    #[tokio::test]
    async fn write_err_frame() {
        let mut buf = Vec::new();
        write_err(&mut buf, "busy", "t").await.expect("write");
        assert_eq!(
            buf,
            [
                0, 0, 0, 12, 0xa3, 0x00, 0x0a, 0x0a, 0x64, b'b', b'u', b's', b'y', 0x0b, 0x61, b't'
            ]
        );
        let err = expect(&mut &buf[..], 0).await.expect_err("remote");
        assert_eq!(err.to_string(), "remote: busy: t");
    }

    #[tokio::test]
    async fn expect_checks_the_type() {
        let ok = frame_of(&[0xa1, 0x00, 0x18, 0x35]);
        let err = expect(&mut &ok[..], T_REF).await.expect_err("mismatch");
        assert!(matches!(err, WireError::Protocol { got: 53, want: 52 }));
        assert_eq!(err.to_string(), "wire: unexpected frame: type 53, want 52");
        assert!(as_remote(&err).is_none());
        assert_eq!(expect(&mut &ok[..], T_OK).await.expect("ok").typ, T_OK);
        assert_eq!(expect(&mut &ok[..], 0).await.expect("any").typ, T_OK);
        assert!(matches!(
            expect(&mut &[][..], T_REF).await,
            Err(WireError::Eof)
        ));
    }

    /// dstore `wire/wire_test.go` `TestPackFramesInterop`.
    #[tokio::test]
    async fn pack_frames_interop() {
        use amber_store_core::amberpack;
        use amber_store_core::key::{Key, Type};

        use crate::pack::{PackReader, PackRecords, PackSender};

        let data = b"some blob bytes";
        let k = Key::new(Type::Blob, data.len() as u64, data);
        let rec = amberpack::encode_record(k, data).expect("record");
        let mut buf = Vec::new();
        let mut sender = PackSender::new(&mut buf);
        sender.add_record(&rec).await.expect("add");
        sender.finish().await.expect("finish");

        let first = read_msg(&mut &buf[..]).await.expect("first frame");
        assert_eq!(first.typ, T_DATA);
        assert!(!first.data.is_empty());

        let mut records = PackRecords::new(PackReader::new(&buf[..]));
        let mut n = 0;
        while let Some(raw) = records.next().await {
            let raw = raw.expect("record");
            assert_eq!(raw.record.key, k);
            n += 1;
        }
        assert_eq!(n, 1);
        records.drain().await.expect("drain");
        let mut rest = records.into_pack_reader().into_inner();
        assert!(matches!(read_msg(&mut rest).await, Err(WireError::Eof)));
    }
}
