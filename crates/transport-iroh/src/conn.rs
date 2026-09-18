//! `irohConn` (`transport/iroh.go:408-445`) over an iroh connection, and the halves of a
//! `transport.Stream` over noq streams.
//!
//! Stream semantics (transport §2.6, §4.3; PORTING.md §5.12):
//! - `finish()` sends FIN. Go's `Close`/`CloseWrite` return an error after the peer's STOP_SENDING and nil
//!   for a second close; every dstore caller ignores both, and noq reports neither.
//! - `cancel_read(code)` sends STOP_SENDING(code) unless the stream was already read to its end or
//!   stopped, as Go's `CancelRead`.
//! - Dropping an unfinished send half finishes it (or resets it with the peer's stop code), and dropping a
//!   receive half that has not reached its end stops it with code 0 (noq `Drop`). dstore code calls
//!   `finish`/`cancel_read` explicitly anyway.
//! - Inner error texts are noq's, not qng's (DD-4).

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use dstore_gocompat::ctx::Ctx;
use dstore_transport::{Conn, NodeId, PathInfo, RecvStream, SendStream, Stream, TransportError};
use iroh::endpoint::{Connection, VarInt};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::NOQ_INITIAL_RTT;

/// A `transport.Conn` over Rust iroh. `path()` maps rtt == `NOQ_INITIAL_RTT` → ZERO.
pub struct IrohConn {
    conn: Connection,
}

impl IrohConn {
    pub(crate) fn new(conn: Connection) -> IrohConn {
        IrohConn { conn }
    }
}

#[async_trait::async_trait]
impl Conn for IrohConn {
    /// `RemoteID`: the peer identity verified by the RFC 7250 handshake.
    fn remote_id(&self) -> NodeId {
        NodeId(*self.conn.remote_id().as_bytes())
    }

    /// `ALPN`: the negotiated protocol.
    fn alpn(&self) -> String {
        String::from_utf8_lossy(self.conn.alpn()).into_owned()
    }

    /// `OpenStream` (go-iroh `OpenStreamSync`): waits until the peer's MAX_STREAMS credit allows another
    /// stream, or the ctx ends. Nothing reaches the peer before the first write.
    async fn open_stream(&self, ctx: &Ctx) -> Result<Stream, TransportError> {
        let (send, recv) = ctx.run(self.conn.open_bi()).await?.map_err(quic_error)?;
        Ok(stream(send, recv))
    }

    /// `AcceptStream` (node side).
    async fn accept_stream(&self, ctx: &Ctx) -> Result<Stream, TransportError> {
        let (send, recv) = ctx.run(self.conn.accept_bi()).await?.map_err(quic_error)?;
        Ok(stream(send, recv))
    }

    /// `Close` (`CloseWithError(0, "")`): CONNECTION_CLOSE with application code 0 and an empty reason.
    /// noq only queues the frame; `IrohEndpoint::close` waits for it to be sent (DD-11).
    fn close(&self) {
        self.conn.close(VarInt::from_u32(0), b"");
    }

    /// `Path`: the selected path. `direct` is true unless it is a relay path, and `rtt` stays ZERO until the
    /// path has a sample (noq reports the configured initial RTT until then, DD-5). With no selected path:
    /// `{direct: true, rtt: 0}`.
    fn path(&self) -> PathInfo {
        let mut info = PathInfo {
            direct: true,
            rtt: Duration::ZERO,
        };
        let paths = self.conn.paths();
        for p in paths.iter() {
            if p.is_selected() {
                select_path(&mut info, p.is_relay(), p.rtt());
            }
        }
        info
    }

    /// `<-Done()` would not block: the connection is closed, for any reason.
    fn is_closed(&self) -> bool {
        self.conn.close_reason().is_some()
    }

    /// `<-Done()`: local close, remote close, idle timeout.
    async fn closed(&self) {
        let _ = self.conn.closed().await;
    }
}

/// One selected path in Go's `irohConn.Path` loop: `Direct = !Relayed`, and `RTT = p.RTT` only when the path
/// has a measurement, so a later selected path without one keeps the earlier RTT.
fn select_path(info: &mut PathInfo, relay: bool, rtt: Duration) {
    info.direct = !relay;
    if rtt != NOQ_INITIAL_RTT {
        info.rtt = rtt;
    }
}

/// noq/iroh error text (DD-4).
fn quic_error(e: impl std::fmt::Display) -> TransportError {
    TransportError::Quic(e.to_string())
}

fn stream(send: iroh::endpoint::SendStream, recv: iroh::endpoint::RecvStream) -> Stream {
    Stream::new(
        Box::new(IrohSendStream(send)),
        Box::new(IrohRecvStream(recv)),
    )
}

/// The send half of an iroh bidirectional stream.
struct IrohSendStream(iroh::endpoint::SendStream);

impl AsyncWrite for IrohSendStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        AsyncWrite::poll_write(Pin::new(&mut self.get_mut().0), cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        AsyncWrite::poll_flush(Pin::new(&mut self.get_mut().0), cx)
    }

    /// noq finishes the stream.
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        AsyncWrite::poll_shutdown(Pin::new(&mut self.get_mut().0), cx)
    }
}

impl SendStream for IrohSendStream {
    fn finish(&mut self) {
        let _ = self.0.finish();
    }
}

/// The receive half of an iroh bidirectional stream.
struct IrohRecvStream(iroh::endpoint::RecvStream);

impl AsyncRead for IrohRecvStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        AsyncRead::poll_read(Pin::new(&mut self.get_mut().0), cx, buf)
    }
}

impl RecvStream for IrohRecvStream {
    fn cancel_read(&mut self, code: u64) {
        // Codes above 2^62 do not fit a QUIC varint; dstore only ever sends 0.
        let code = VarInt::from_u64(code).unwrap_or(VarInt::MAX);
        let _ = self.0.stop(code);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(paths: &[(bool, bool, Duration)]) -> PathInfo {
        let mut info = PathInfo {
            direct: true,
            rtt: Duration::ZERO,
        };
        for &(selected, relay, rtt) in paths {
            if selected {
                select_path(&mut info, relay, rtt);
            }
        }
        info
    }

    fn info(direct: bool, rtt_ms: u64) -> PathInfo {
        PathInfo {
            direct,
            rtt: Duration::from_millis(rtt_ms),
        }
    }

    // transport §2.6: no selected path → {Direct: true, RTT: 0}.
    #[test]
    fn no_selected_path_is_direct_and_unmeasured() {
        assert_eq!(run(&[]), info(true, 0));
        assert_eq!(
            run(&[(false, true, Duration::from_millis(5))]),
            info(true, 0)
        );
    }

    #[test]
    fn selected_path_gives_direct_and_rtt() {
        assert_eq!(
            run(&[(true, false, Duration::from_millis(12))]),
            info(true, 12)
        );
        assert_eq!(
            run(&[(true, true, Duration::from_millis(80))]),
            info(false, 80)
        );
        // Unselected paths are ignored.
        assert_eq!(
            run(&[
                (false, false, Duration::from_millis(1)),
                (true, true, Duration::from_millis(40)),
            ]),
            info(false, 40)
        );
    }

    // DD-5: the unsampled estimator reports exactly the configured initial RTT.
    #[test]
    fn initial_rtt_reads_as_not_measured() {
        assert_eq!(run(&[(true, false, NOQ_INITIAL_RTT)]), info(true, 0));
        assert_eq!(
            run(&[(true, false, NOQ_INITIAL_RTT + Duration::from_nanos(1))]),
            PathInfo {
                direct: true,
                rtt: NOQ_INITIAL_RTT + Duration::from_nanos(1)
            }
        );
    }

    // Go's loop: a later selected path overwrites Direct, and its RTT only when measured.
    #[test]
    fn later_selected_path_overwrites_only_measured_rtt() {
        assert_eq!(
            run(&[
                (true, false, Duration::from_millis(7)),
                (true, true, NOQ_INITIAL_RTT),
            ]),
            info(false, 7)
        );
        assert_eq!(
            run(&[
                (true, false, Duration::from_millis(7)),
                (true, true, Duration::from_millis(90)),
            ]),
            info(false, 90)
        );
    }

    #[test]
    fn quic_errors_keep_the_inner_text() {
        assert_eq!(quic_error("timed out").to_string(), "timed out");
    }
}
