//! `transport.Endpoint`, `transport.Conn` and QUIC stream halves (`transport/transport.go:18-61`).

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

/// A bidirectional stream.
pub struct Stream {
    pub send: Box<dyn SendStream>,
    pub recv: Box<dyn RecvStream>,
}

impl Stream {
    pub fn new(send: Box<dyn SendStream>, recv: Box<dyn RecvStream>) -> Stream {
        todo!()
    }

    pub fn close_write(&mut self) {
        todo!()
    }

    pub fn cancel_read(&mut self, code: u64) {
        todo!()
    }

    /// `wire.CloseStream`: finish, then `cancel_read(0)`.
    pub fn close_stream(&mut self) {
        todo!()
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
