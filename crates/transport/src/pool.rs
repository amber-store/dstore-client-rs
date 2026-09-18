//! `transport.Pool` (`transport/transport.go:63-243`): grow to `per_peer`, round robin, the 2 s
//! failed-dial window only with no live conn, only `open_stream` failures drop, never shrink. The dial
//! lock is a `tokio::sync::Mutex<()>` awaited outside `ctx.run` (not ctx-aware, as in Go).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::{AddrsFn, CallError, Conn, Ctx, Endpoint, NodeId, PathInfo, Stream, TransportError};

/// `transport.Pool`.
pub struct Pool {
    ep: Arc<dyn Endpoint>,
    addrs: AddrsFn,
    per_peer: usize,
    state: Mutex<PoolState>,
}

/// (peer, ALPN).
type PeerKey = (NodeId, String);

struct PoolState {
    conns: HashMap<PeerKey, Vec<Arc<dyn Conn>>>,
    next: HashMap<PeerKey, usize>,
    dialing: HashMap<PeerKey, Arc<tokio::sync::Mutex<()>>>,
    failed: HashMap<PeerKey, tokio::time::Instant>,
}

impl Pool {
    /// `transport.NewPool`; `per_peer` 0 → 1.
    pub fn new(ep: Arc<dyn Endpoint>, addrs: AddrsFn, per_peer: usize) -> Pool {
        todo!()
    }

    pub fn endpoint(&self) -> &Arc<dyn Endpoint> {
        todo!()
    }

    /// `Pool.Get`.
    pub async fn get(
        &self,
        ctx: &Ctx,
        id: NodeId,
        alpn: &str,
    ) -> Result<Arc<dyn Conn>, TransportError> {
        todo!()
    }

    /// Go `Drop`.
    pub fn drop_peer(&self, id: NodeId, alpn: &str) {
        todo!()
    }

    /// The path of the first live conn.
    pub fn path(&self, id: NodeId, alpn: &str) -> Option<PathInfo> {
        todo!()
    }

    pub fn close(&self) {
        todo!()
    }

    /// Go `Pool.Call`: get; open_stream (error → drop_peer); write_msg; close_write; read one frame under
    /// ctx (ctx end → cancel_read(0) + finish, CtxError); TErr → Remote; close_stream on every exit.
    pub async fn call(
        &self,
        ctx: &Ctx,
        id: NodeId,
        alpn: &str,
        req: &dstore_wire::Msg,
    ) -> Result<dstore_wire::Msg, CallError> {
        todo!()
    }

    /// `Pool.Open`.
    pub async fn open(&self, ctx: &Ctx, id: NodeId, alpn: &str) -> Result<Stream, TransportError> {
        todo!()
    }
}
