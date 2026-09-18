//! `client/client.go`: `Config`, `Dial`, the view cache, failure bookkeeping, request helpers,
//! `Status` and `Admin` (part A).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use dstore_gocompat::slog::Attr;
use dstore_ticket::Ticket;
use dstore_transport::{Endpoint, Pool};
use dstore_view::{Placement, View};
use dstore_wire::{AdminRequest, Msg};

use crate::{Ctx, Error, Logger, NodeId};

/// `client.Config`.
#[derive(Clone)]
pub struct Config {
    /// None → `Error::NoEndpoint` (checked first).
    pub endpoint: Option<Arc<dyn Endpoint>>,
    pub ticket: Ticket,
    /// 0 → 4.
    pub conns: usize,
    /// 0 → 8.
    pub jobs: usize,
    /// None → `Logger::default_logger()`.
    pub logger: Option<Logger>,
    /// ZERO → 4 h.
    pub gc_interval: Duration,
    /// ZERO → 2 min.
    pub request_timeout: Duration,
    /// 0 → 16 MiB, then min(64 MiB).
    pub batch_bytes: usize,
    /// ZERO → 2 min.
    pub watch_idle: Duration,
}

/// All zero / None.
impl Default for Config {
    fn default() -> Config {
        todo!()
    }
}

/// `*client.Cluster`.
#[derive(Clone)]
pub struct Cluster {
    inner: Arc<Inner>,
}

struct Inner {
    cfg: Config,
    log: Logger,
    ep: Arc<dyn Endpoint>,
    pool: Pool,
    state: RwLock<ViewState>,
}

struct ViewState {
    view: Option<Arc<View>>,
    placement: Option<Arc<Placement>>,
    boot_addrs: HashMap<NodeId, Vec<String>>,
    backoff: HashMap<NodeId, tokio::time::Instant>,
    failures: HashMap<NodeId, u32>,
    unreach: HashSet<NodeId>,
}

impl Cluster {
    /// Go `Dial`, client-core §2.3.
    pub async fn dial(ctx: &Ctx, cfg: Config) -> Result<Cluster, Error> {
        todo!()
    }

    /// Closes the pool only.
    pub fn close(&self) {
        todo!()
    }

    pub fn endpoint(&self) -> &Arc<dyn Endpoint> {
        todo!()
    }

    pub fn view(&self) -> Option<Arc<View>> {
        todo!()
    }

    pub fn placement(&self) -> Option<Arc<Placement>> {
        todo!()
    }

    pub async fn refresh_view(&self, ctx: &Ctx) -> Result<(), Error> {
        todo!()
    }

    pub fn primary(&self, key: &[u8; 32]) -> Option<NodeId> {
        todo!()
    }

    pub fn owners(&self, key: &[u8; 32]) -> Vec<NodeId> {
        todo!()
    }

    pub fn write_set(&self, key: &[u8; 32]) -> Vec<NodeId> {
        todo!()
    }

    /// One (view, placement) snapshot.
    pub fn read_order(&self, key: &[u8; 32]) -> Vec<NodeId> {
        todo!()
    }

    pub fn nodes(&self) -> Vec<NodeId> {
        todo!()
    }

    /// No reply type check.
    pub async fn status(&self, ctx: &Ctx, id: NodeId) -> Result<Vec<u8>, Error> {
        todo!()
    }

    /// None → anyNode.
    pub async fn admin(
        &self,
        ctx: &Ctx,
        id: Option<NodeId>,
        req: &AdminRequest,
    ) -> Result<Vec<u8>, Error> {
        todo!()
    }

    // ---- crate-internal seams (part A → part B) ----

    /// The config with defaults applied.
    pub(crate) fn cfg(&self) -> &Config {
        todo!()
    }

    pub(crate) fn log(&self) -> &Logger {
        todo!()
    }

    pub(crate) fn pool(&self) -> &Pool {
        todo!()
    }

    pub(crate) fn stamp(&self, m: &mut Msg) {
        todo!()
    }

    pub(crate) async fn call(&self, ctx: &Ctx, id: NodeId, m: &mut Msg) -> Result<Msg, Error> {
        todo!()
    }

    /// ≤ 4 calls on stale-view.
    pub(crate) async fn call_retry(
        &self,
        ctx: &Ctx,
        id: NodeId,
        m: &mut Msg,
    ) -> Result<Msg, Error> {
        todo!()
    }

    /// ≤ 5 calls per node on stale-view.
    pub(crate) async fn any_node(&self, ctx: &Ctx, m: &mut Msg) -> Result<Msg, Error> {
        todo!()
    }

    pub(crate) fn handle_err(&self, id: NodeId, err: &Error) {
        todo!()
    }

    pub(crate) fn ok(&self, id: NodeId) {
        todo!()
    }

    pub(crate) fn penalty(&self, id: NodeId) -> i64 {
        todo!()
    }

    pub(crate) fn preferred(&self, ids: &[NodeId]) -> Vec<NodeId> {
        todo!()
    }

    pub(crate) async fn probe_hinted(&self, ctx: &Ctx) {
        todo!()
    }

    pub(crate) fn path_attrs(&self, id: NodeId) -> Vec<Attr> {
        todo!()
    }
}
