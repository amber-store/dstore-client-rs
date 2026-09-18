//! `client/progress.go`: progress reports, the put observer, the tracker, `HumanBytes` and `Rate`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::{Cluster, NodeId};

/// `client.Progress`: called under the tracker mutex; must not re-enter the client.
pub type Progress = Arc<dyn Fn(&ProgressReport) + Send + Sync>;

/// `client.ProgressReport`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProgressReport {
    pub objects: i64,
    pub total_objects: i64,
    pub bytes: i64,
    pub total_bytes: i64,
    pub nodes: Vec<NodeProgress>,
}

/// `client.NodeProgress`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NodeProgress {
    pub id: NodeId,
    pub direct: bool,
    pub rtt: Duration,
    pub in_flight: i64,
    pub awaiting: i64,
    pub bytes: i64,
}

/// `client.PutObserver`.
#[derive(Clone, Default)]
pub struct PutObserver {
    pub start: Option<Arc<dyn Fn(NodeId) + Send + Sync>>,
    pub sent: Option<Arc<dyn Fn(NodeId, usize) + Send + Sync>>,
    pub flushed: Option<Arc<dyn Fn(NodeId) + Send + Sync>>,
    pub done: Option<Arc<dyn Fn(NodeId, bool) + Send + Sync>>,
}

/// `tracker`.
pub(crate) struct Tracker {
    cluster: Cluster,
    prog: Option<Progress>,
    state: Mutex<TrackerState>,
}

struct TrackerState {
    report: ProgressReport,
    per_node: HashMap<NodeId, NodeProgress>,
}

impl Tracker {
    pub(crate) fn new(c: &Cluster, prog: Option<Progress>) -> Arc<Tracker> {
        todo!()
    }

    pub(crate) fn observer(self: &Arc<Self>) -> PutObserver {
        todo!()
    }

    pub(crate) fn totals(&self, objects: i64, done: i64, bytes: i64) {
        todo!()
    }

    pub(crate) fn more(&self, bytes: i64) {
        todo!()
    }

    pub(crate) fn objects(&self, n: i64) {
        todo!()
    }

    pub(crate) fn bytes(&self) -> i64 {
        todo!()
    }
}

/// `countKeys`: (objects, bytes).
pub(crate) fn count_keys(
    m: &HashMap<NodeId, Vec<[u8; 32]>>,
    size: &dyn Fn(&[u8; 32]) -> usize,
) -> (i64, i64) {
    todo!()
}

/// `client.HumanBytes`: "1024.0 KiB" for 1048575.
pub fn human_bytes(n: i64) -> String {
    todo!()
}

/// `client.Rate`: ZERO → "-".
pub fn rate(bytes: i64, took: Duration) -> String {
    todo!()
}
