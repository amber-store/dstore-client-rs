//! `irohConn` (`transport/iroh.go:408-445`) over an iroh connection.

use dstore_gocompat::ctx::Ctx;
use dstore_transport::{Conn, NodeId, PathInfo, Stream, TransportError};

/// A `transport.Conn` over Rust iroh. `path()` maps rtt == `NOQ_INITIAL_RTT` → ZERO.
pub struct IrohConn {
    conn: iroh::endpoint::Connection,
}

#[async_trait::async_trait]
impl Conn for IrohConn {
    fn remote_id(&self) -> NodeId {
        todo!()
    }

    fn alpn(&self) -> String {
        todo!()
    }

    async fn open_stream(&self, ctx: &Ctx) -> Result<Stream, TransportError> {
        todo!()
    }

    async fn accept_stream(&self, ctx: &Ctx) -> Result<Stream, TransportError> {
        todo!()
    }

    fn close(&self) {
        todo!()
    }

    fn path(&self) -> PathInfo {
        todo!()
    }

    fn is_closed(&self) -> bool {
        todo!()
    }

    async fn closed(&self) {
        todo!()
    }
}
