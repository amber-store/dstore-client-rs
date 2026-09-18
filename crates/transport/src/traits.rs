//! `transport.Endpoint`, `transport.Conn` and QUIC stream halves (`transport/transport.go:18-61`).

use std::fmt;
use std::sync::Arc;

use crate::{Ctx, NodeId, TransportError};

/// The selected network path of a connection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PathInfo {
    pub direct: bool,
    /// ZERO = not measured.
    pub rtt: std::time::Duration,
}

/// The send half of a bidirectional stream.
pub trait SendStream: tokio::io::AsyncWrite + Send + Unpin + 'static {
    /// Go `Close()`/`CloseWrite()`: FIN; errors ignored.
    fn finish(&mut self);
}

/// The receive half of a bidirectional stream.
pub trait RecvStream: tokio::io::AsyncRead + Send + Unpin + 'static {
    /// Go `CancelRead(code)`: STOP_SENDING.
    fn cancel_read(&mut self, code: u64);
}

/// A bidirectional stream (Go `transport.Stream`). Read from `recv` and write to `send`; the halves can be
/// used concurrently.
pub struct Stream {
    pub send: Box<dyn SendStream>,
    pub recv: Box<dyn RecvStream>,
}

impl Stream {
    pub fn new(send: Box<dyn SendStream>, recv: Box<dyn RecvStream>) -> Stream {
        Stream { send, recv }
    }

    /// Go `CloseWrite()`: finishes the send side.
    pub fn close_write(&mut self) {
        self.send.finish();
    }

    /// Go `CancelRead(code)`: abandons the receive side.
    pub fn cancel_read(&mut self, code: u64) {
        self.recv.cancel_read(code);
    }

    /// `wire.CloseStream`: finish, then `cancel_read(0)`.
    ///
    /// Go calls `Close()` (which equals `CloseWrite()` for both iroh and mem streams) and then
    /// `CancelRead(0)`, ignoring every error.
    pub fn close_stream(&mut self) {
        self.send.finish();
        self.recv.cancel_read(0);
    }
}

impl fmt::Debug for Stream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Stream").finish_non_exhaustive()
    }
}

/// `transport.Conn`.
#[async_trait::async_trait]
pub trait Conn: Send + Sync + 'static {
    fn remote_id(&self) -> NodeId;
    fn alpn(&self) -> String;
    async fn open_stream(&self, ctx: &Ctx) -> Result<Stream, TransportError>;
    async fn accept_stream(&self, ctx: &Ctx) -> Result<Stream, TransportError>;
    /// CONNECTION_CLOSE with app code 0 and an empty reason.
    fn close(&self);
    fn path(&self) -> PathInfo;
    /// Go: `<-Done()` would not block.
    fn is_closed(&self) -> bool;
    async fn closed(&self);
}

/// `transport.Endpoint`.
#[async_trait::async_trait]
pub trait Endpoint: Send + Sync + 'static {
    fn id(&self) -> NodeId;
    async fn dial(
        &self,
        ctx: &Ctx,
        id: NodeId,
        addrs: Vec<String>,
        alpn: &str,
    ) -> Result<Arc<dyn Conn>, TransportError>;
    async fn accept(&self, ctx: &Ctx) -> Result<Arc<dyn Conn>, TransportError>;
    fn addrs(&self) -> Vec<String>;
    async fn close(&self);
}

/// The dial addresses of a node (`Pool`'s address lookup).
pub type AddrsFn = Arc<dyn Fn(NodeId) -> Vec<String> + Send + Sync>;

#[cfg(test)]
mod tests {
    use super::*;

    use std::io;
    use std::pin::Pin;
    use std::sync::{Mutex, MutexGuard, PoisonError};
    use std::task::{Context, Poll};

    type Log = Arc<Mutex<Vec<String>>>;

    fn lock(log: &Log) -> MutexGuard<'_, Vec<String>> {
        log.lock().unwrap_or_else(PoisonError::into_inner)
    }

    struct Send_(Log);
    impl tokio::io::AsyncWrite for Send_ {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            Poll::Ready(Ok(buf.len()))
        }
        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }
    impl SendStream for Send_ {
        fn finish(&mut self) {
            lock(&self.0).push("finish".to_string());
        }
    }

    struct Recv(Log);
    impl tokio::io::AsyncRead for Recv {
        fn poll_read(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            _buf: &mut tokio::io::ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }
    impl RecvStream for Recv {
        fn cancel_read(&mut self, code: u64) {
            lock(&self.0).push(format!("cancel_read {code}"));
        }
    }

    fn stream() -> (Stream, Log) {
        let log = Log::default();
        (
            Stream::new(Box::new(Send_(log.clone())), Box::new(Recv(log.clone()))),
            log,
        )
    }

    #[test]
    fn stream_helpers_map_to_the_halves() {
        let (mut s, log) = stream();
        s.close_write();
        s.cancel_read(7);
        assert_eq!(*lock(&log), ["finish", "cancel_read 7"]);
    }

    // wire.CloseStream: Close (FIN) first, then CancelRead(0).
    #[test]
    fn close_stream_finishes_then_cancels_the_read_side() {
        let (mut s, log) = stream();
        s.close_stream();
        assert_eq!(*lock(&log), ["finish", "cancel_read 0"]);
        assert_eq!(format!("{s:?}"), "Stream { .. }");
    }

    #[test]
    fn path_info_default_is_not_measured() {
        let p = PathInfo::default();
        assert!(!p.direct);
        assert_eq!(p.rtt, std::time::Duration::ZERO);
    }
}
