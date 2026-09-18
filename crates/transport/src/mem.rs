//! `transport/mem.go`: an in-memory network for tests. Keeps Go's quirk: a blocked read survives
//! connection close.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::{Conn, Ctx, Endpoint, NodeId, PathInfo, Stream, TransportError};

/// Per-direction pipe buffer limit.
pub const PIPE_LIMIT: usize = 4 << 20;

/// `transport.Network`.
pub struct Network {
    state: Mutex<NetState>,
}

struct NetState {
    endpoints: HashMap<NodeId, Weak<MemEndpoint>>,
    down: HashSet<NodeId>,
    cut: HashSet<(NodeId, NodeId)>,
    delay: Duration,
}

impl Network {
    pub fn new() -> Arc<Network> {
        todo!()
    }

    pub fn bind(self: &Arc<Self>, id: NodeId, alpns: &[&str]) -> Arc<MemEndpoint> {
        todo!()
    }

    pub fn set_down(&self, id: NodeId, down: bool) {
        todo!()
    }

    pub fn partition(&self, a: NodeId, b: NodeId, cut: bool) {
        todo!()
    }

    pub fn set_delay(&self, d: Duration) {
        todo!()
    }
}

/// An endpoint on a [`Network`]: `addrs()` = ["mem:<shortid>"]; path {direct: true, rtt: 1ms}.
pub struct MemEndpoint {
    net: Weak<Network>,
    id: NodeId,
    alpns: Vec<String>,
    incoming_tx: tokio::sync::mpsc::Sender<Arc<dyn Conn>>,
    incoming_rx: tokio::sync::Mutex<tokio::sync::mpsc::Receiver<Arc<dyn Conn>>>,
    closed: CancellationToken,
}

#[async_trait::async_trait]
impl Endpoint for MemEndpoint {
    fn id(&self) -> NodeId {
        todo!()
    }

    async fn dial(
        &self,
        ctx: &Ctx,
        id: NodeId,
        addrs: Vec<String>,
        alpn: &str,
    ) -> Result<Arc<dyn Conn>, TransportError> {
        todo!()
    }

    async fn accept(&self, ctx: &Ctx) -> Result<Arc<dyn Conn>, TransportError> {
        todo!()
    }

    fn addrs(&self) -> Vec<String> {
        todo!()
    }

    async fn close(&self) {
        todo!()
    }
}

/// One side of an in-memory connection.
pub(crate) struct MemConn {
    local: NodeId,
    remote: NodeId,
    alpn: String,
    streams_tx: tokio::sync::mpsc::Sender<Stream>,
    streams_rx: tokio::sync::Mutex<tokio::sync::mpsc::Receiver<Stream>>,
    closed: CancellationToken,
}

#[async_trait::async_trait]
impl Conn for MemConn {
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
