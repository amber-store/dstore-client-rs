//! A fake dstore cluster over `dstore_transport::mem`: node handler semantics of `node/server.go`,
//! `data.go`, `refs.go`, `watch.go`, `admin.go`, `status.go` (verification §4.6), with fault injection
//! and request transcripts.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dstore_ticket::Ticket;
use dstore_transport::mem::{MemEndpoint, Network};
use dstore_view::{NodeId, View};
use dstore_wire::Msg;

/// The shape of a fake cluster.
pub struct FakeClusterConfig {
    pub nodes: usize,
    pub replicas: u8,
    pub min_replicas: u8,
    pub weights: Vec<u32>,
    pub zones: Vec<String>,
}

/// A fault injected into the nth request of a type at a node.
pub enum Injection {
    Delay(Duration),
    CloseBeforeReply,
    Err {
        code: String,
        text: String,
        with_view: bool,
        retry_after_ms: i64,
    },
    CorruptRecord([u8; 32]),
    DropHints,
}

/// A running fake cluster.
pub struct FakeCluster {
    net: Arc<Network>,
    endpoints: Vec<Arc<MemEndpoint>>,
    state: Mutex<FakeState>,
}

struct FakeState {
    view: View,
    stored: BTreeMap<NodeId, BTreeMap<[u8; 32], Vec<u8>>>,
    requests: BTreeMap<NodeId, Vec<Msg>>,
    injections: Vec<(NodeId, i64, usize, Injection)>,
    ref_page_limit: usize,
    watch_reconcile: Duration,
}

impl FakeCluster {
    pub async fn start(net: &Arc<Network>, cfg: FakeClusterConfig) -> Arc<FakeCluster> {
        todo!()
    }

    pub fn ids(&self) -> Vec<NodeId> {
        todo!()
    }

    pub fn ticket(&self) -> Ticket {
        todo!()
    }

    pub fn view(&self) -> View {
        todo!()
    }

    pub fn bump_epoch(&self) {
        todo!()
    }

    pub fn set_writable(&self, id: NodeId, writable: bool) {
        todo!()
    }

    pub fn inject(&self, id: NodeId, op: i64, nth: usize, inj: Injection) {
        todo!()
    }

    pub fn stored(&self, id: NodeId) -> BTreeMap<[u8; 32], Vec<u8>> {
        todo!()
    }

    /// The request transcript of a node.
    pub fn requests(&self, id: NodeId) -> Vec<Msg> {
        todo!()
    }

    pub fn set_ref_page_limit(&self, n: usize) {
        todo!()
    }

    pub fn set_watch_reconcile(&self, d: Duration) {
        todo!()
    }

    pub async fn close(&self) {
        todo!()
    }
}
