//! `client/client.go`: `Config`, `Dial`, the view cache, failure bookkeeping, request helpers,
//! `Status` and `Admin` (part A).

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Duration;

use dstore_gocompat::slog::Attr;
use dstore_ticket::{Member, Ticket};
use dstore_transport::{AddrsFn, Endpoint, Pool};
use dstore_view::{Node, Placement, View, short_id};
use dstore_wire::{
    ALPN_CLIENT, AdminRequest, CODE_STALE_VIEW, MAX_PUT_BATCH, Msg, T_ADMIN, T_ADMIN_REPLY,
    T_STATUS, T_VIEW, T_VIEW_REPLY,
};

use crate::progress::path_attrs_of;
use crate::rank::rank_owners;
use crate::{Ctx, DEFAULT_BATCH_BYTES, DIAL_MEMBER_TIMEOUT, Error, Logger, NodeId, PROBE_TIMEOUT};

const DEFAULT_CONNS: usize = 4;
const DEFAULT_JOBS: usize = 8;
const DEFAULT_GC_INTERVAL: Duration = Duration::from_secs(4 * 60 * 60);
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(2 * 60);
const DEFAULT_WATCH_IDLE: Duration = Duration::from_secs(2 * 60);
/// `handleErr`: `5s << min(failures-1, 4)`, capped at 60 s.
const BACKOFF_BASE: Duration = Duration::from_secs(5);
const BACKOFF_MAX: Duration = Duration::from_secs(60);
/// `callRetry` attempts on `stale-view`.
const CALL_RETRY_ATTEMPTS: usize = 4;
/// `anyNode` retries per node on `stale-view`, after the first call.
const ANY_NODE_STALE_RETRIES: usize = 4;

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
        Config {
            endpoint: None,
            ticket: Ticket::default(),
            conns: 0,
            jobs: 0,
            logger: None,
            gc_interval: Duration::ZERO,
            request_timeout: Duration::ZERO,
            batch_bytes: 0,
            watch_idle: Duration::ZERO,
        }
    }
}

/// `Dial`'s defaults (client.go:63-85); the logger is filled in too.
fn with_defaults(mut cfg: Config) -> (Config, Logger) {
    if cfg.conns == 0 {
        cfg.conns = DEFAULT_CONNS;
    }
    if cfg.jobs == 0 {
        cfg.jobs = DEFAULT_JOBS;
    }
    let log = cfg.logger.clone().unwrap_or_else(Logger::default_logger);
    cfg.logger = Some(log.clone());
    if cfg.gc_interval.is_zero() {
        cfg.gc_interval = DEFAULT_GC_INTERVAL;
    }
    if cfg.request_timeout.is_zero() {
        cfg.request_timeout = DEFAULT_REQUEST_TIMEOUT;
    }
    if cfg.batch_bytes == 0 {
        cfg.batch_bytes = DEFAULT_BATCH_BYTES;
    }
    cfg.batch_bytes = cfg.batch_bytes.min(MAX_PUT_BATCH);
    if cfg.watch_idle.is_zero() {
        cfg.watch_idle = DEFAULT_WATCH_IDLE;
    }
    (cfg, log)
}

/// The backoff after a node's `failures`-th consecutive non-remote failure.
pub(crate) fn backoff_after(failures: u64) -> Duration {
    let shift = failures.saturating_sub(1).min(4) as u32;
    (BACKOFF_BASE * (1u32 << shift)).min(BACKOFF_MAX)
}

/// A ticket member's id when it is 32 bytes long.
fn member_id(m: &Member) -> Option<NodeId> {
    let id = m.id.as_deref()?;
    <[u8; 32]>::try_from(id).ok().map(NodeId)
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
    /// Shared with the pool's address lookup.
    state: Arc<RwLock<ViewState>>,
}

/// What `c.mu` protects in Go.
#[derive(Default)]
struct ViewState {
    view: Option<Arc<View>>,
    placement: Option<Arc<Placement>>,
    boot_addrs: HashMap<NodeId, Vec<String>>,
    /// The ticket's 32-byte member ids in first-occurrence order: Go ranges over the `bootAddrs` map, in
    /// random order (DD-10).
    boot_order: Vec<NodeId>,
    /// Until when a node is penalised.
    backoff: HashMap<NodeId, tokio::time::Instant>,
    /// Consecutive non-remote failures.
    failures: HashMap<NodeId, u64>,
    /// The last view reply's unreachable list.
    unreach: HashSet<NodeId>,
}

fn read_state(state: &RwLock<ViewState>) -> RwLockReadGuard<'_, ViewState> {
    state.read().unwrap_or_else(PoisonError::into_inner)
}

fn write_state(state: &RwLock<ViewState>) -> RwLockWriteGuard<'_, ViewState> {
    state.write().unwrap_or_else(PoisonError::into_inner)
}

/// `addrsOf`: the view's addresses of the node when it has any, else the ticket's.
fn addrs_of(state: &RwLock<ViewState>, id: NodeId) -> Vec<String> {
    let st = read_state(state);
    if let Some(v) = &st.view
        && let Some(nd) = v.node(&id)
        && !nd.addrs.is_empty()
    {
        return nd.addrs.clone();
    }
    st.boot_addrs.get(&id).cloned().unwrap_or_default()
}

impl Cluster {
    /// Go `Dial`, client-core §2.3.
    pub async fn dial(ctx: &Ctx, cfg: Config) -> Result<Cluster, Error> {
        let Some(ep) = cfg.endpoint.clone() else {
            return Err(Error::NoEndpoint);
        };
        let (cfg, log) = with_defaults(cfg);
        let mut st = ViewState::default();
        let mut members = Vec::new();
        for m in cfg.ticket.members() {
            let Some(id) = member_id(m) else {
                continue;
            };
            if st.boot_addrs.insert(id, m.addrs.clone()).is_none() {
                st.boot_order.push(id);
            }
            members.push(id);
        }
        let state = Arc::new(RwLock::new(st));
        let addrs: AddrsFn = {
            let state = Arc::clone(&state);
            Arc::new(move |id| addrs_of(&state, id))
        };
        let pool = Pool::new(Arc::clone(&ep), addrs, cfg.conns);
        let c = Cluster {
            inner: Arc::new(Inner {
                cfg,
                log,
                ep,
                pool,
                state,
            }),
        };

        let mut last_err = None;
        for id in members {
            let dctx = ctx.with_timeout(DIAL_MEMBER_TIMEOUT);
            let req = Msg {
                typ: T_VIEW,
                ..Msg::default()
            };
            let res = c.inner.pool.call(&dctx, id, ALPN_CLIENT, &req).await;
            dctx.cancel();
            let resp = match res {
                Ok(resp) => resp,
                Err(e) => {
                    last_err = Some(Error::from(e));
                    continue;
                }
            };
            if let Err(e) = c.adopt_reply(&resp) {
                last_err = Some(e);
                continue;
            }
            let nodes = c.view().map_or(0, |v| v.nodes().len());
            let mut attrs = vec![
                Attr::string("node", short_id(&id)),
                Attr::int64("nodes", nodes as i64),
            ];
            attrs.extend(c.path_attrs(id));
            c.inner.log.info("connected", attrs);
            return Ok(c);
        }
        let last_err = last_err.unwrap_or(Error::TicketNamesNoNodes);
        c.inner.pool.close();
        Err(Error::NoBootstrap(Box::new(last_err)))
    }

    /// Closes the pool only.
    pub fn close(&self) {
        self.inner.pool.close();
    }

    pub fn endpoint(&self) -> &Arc<dyn Endpoint> {
        &self.inner.ep
    }

    fn read(&self) -> RwLockReadGuard<'_, ViewState> {
        read_state(&self.inner.state)
    }

    fn write(&self) -> RwLockWriteGuard<'_, ViewState> {
        write_state(&self.inner.state)
    }

    /// The cached view.
    pub fn view(&self) -> Option<Arc<View>> {
        self.read().view.clone()
    }

    /// The cached placement tables.
    pub fn placement(&self) -> Option<Arc<Placement>> {
        self.read().placement.clone()
    }

    /// `adoptReply`: adopt the reply's view, then replace the hints, even when the view was older.
    fn adopt_reply(&self, m: &Msg) -> Result<(), Error> {
        if m.typ != T_VIEW_REPLY {
            return Err(Error::UnexpectedReply(m.typ));
        }
        let v = View::decode(&m.view).map_err(Error::View)?;
        self.adopt(v);
        let unreach = m
            .unreachable
            .iter()
            .filter_map(|u| <[u8; 32]>::try_from(u.as_slice()).ok())
            .map(NodeId)
            .collect();
        self.write().unreach = unreach;
        Ok(())
    }

    /// `adopt`: views order by (incarnation, epoch, version); an equal triple is not re-adopted.
    fn adopt(&self, v: View) {
        let mut st = self.write();
        if let Some(cur) = &st.view {
            match cur.compare(v.incarnation, v.epoch) {
                Ordering::Greater => return,
                Ordering::Equal if v.version <= cur.version => return,
                _ => {}
            }
        }
        let v = Arc::new(v);
        st.placement = Some(Arc::new(Placement::new(Arc::clone(&v))));
        st.view = Some(v);
    }

    /// `RefreshView`: the view from any node.
    pub async fn refresh_view(&self, ctx: &Ctx) -> Result<(), Error> {
        let mut m = Msg {
            typ: T_VIEW,
            ..Msg::default()
        };
        let resp = self.any_node(ctx, &mut m).await?;
        self.adopt_reply(&resp)
    }

    /// `Primary`: the preferred owner under `nodes`.
    pub fn primary(&self, key: &[u8; 32]) -> Option<NodeId> {
        let owners = self.placement()?.owners(key);
        self.preferred(&owners).first().copied()
    }

    /// `Owners`: owners(key, nodes).
    pub fn owners(&self, key: &[u8; 32]) -> Vec<NodeId> {
        self.placement().map(|p| p.owners(key)).unwrap_or_default()
    }

    /// `WriteSet`: owners under nodes ∪ pending.nodes.
    pub fn write_set(&self, key: &[u8; 32]) -> Vec<NodeId> {
        self.placement()
            .map(|p| p.write_set(key))
            .unwrap_or_default()
    }

    /// `ReadOrder`: the placement's read order with only the first R re-ranked by preference. One
    /// (view, placement) snapshot.
    pub fn read_order(&self, key: &[u8; 32]) -> Vec<NodeId> {
        let Some(pl) = self.placement() else {
            return Vec::new();
        };
        let order = pl.read_order(key);
        let r = usize::from(pl.view().replicas);
        if order.len() <= r {
            return self.preferred(&order);
        }
        let mut out = self.preferred(&order[..r]);
        out.extend_from_slice(&order[r..]);
        out
    }

    /// `Nodes`: the ids under `nodes`, in view order.
    pub fn nodes(&self) -> Vec<NodeId> {
        self.view()
            .map(|v| v.nodes().iter().map(Node::nid).collect())
            .unwrap_or_default()
    }

    /// `allNodes`: the view's nodes, or the bootstrap nodes before there are any.
    pub(crate) fn all_nodes(&self) -> Vec<NodeId> {
        let ids = self.nodes();
        if ids.is_empty() {
            return self.read().boot_order.clone();
        }
        ids
    }

    /// `Status`: no reply type check.
    pub async fn status(&self, ctx: &Ctx, id: NodeId) -> Result<Vec<u8>, Error> {
        let mut m = Msg {
            typ: T_STATUS,
            ..Msg::default()
        };
        let resp = self.call(ctx, id, &mut m).await?;
        Ok(resp.status)
    }

    /// `Admin`: None (or the zero id, as in Go) → anyNode.
    pub async fn admin(
        &self,
        ctx: &Ctx,
        id: Option<NodeId>,
        req: &AdminRequest,
    ) -> Result<Vec<u8>, Error> {
        let mut m = Msg {
            typ: T_ADMIN,
            params: dstore_codec::marshal(req),
            ..Msg::default()
        };
        let resp = match id {
            Some(id) if id != NodeId::default() => self.call(ctx, id, &mut m).await?,
            _ => self.any_node(ctx, &mut m).await?,
        };
        if resp.typ != T_ADMIN_REPLY {
            return Err(Error::UnexpectedReply(resp.typ));
        }
        Ok(resp.status)
    }

    // ---- crate-internal seams (part A → part B) ----

    /// The config with defaults applied.
    pub(crate) fn cfg(&self) -> &Config {
        &self.inner.cfg
    }

    pub(crate) fn log(&self) -> &Logger {
        &self.inner.log
    }

    pub(crate) fn pool(&self) -> &Pool {
        &self.inner.pool
    }

    /// `stamp`: cluster id, incarnation and epoch of the cached view, in place.
    pub(crate) fn stamp(&self, m: &mut Msg) {
        if let Some(v) = self.view() {
            m.cluster_id = v.cluster_id.clone().unwrap_or_default();
            m.incarnation = v.incarnation;
            m.epoch = v.epoch;
        }
    }

    /// `call`: one request to a node under `RequestTimeout`, with failure bookkeeping and an async view
    /// refresh when the reply carries a newer epoch.
    pub(crate) async fn call(&self, ctx: &Ctx, id: NodeId, m: &mut Msg) -> Result<Msg, Error> {
        let cctx = ctx.with_timeout(self.inner.cfg.request_timeout);
        self.stamp(m);
        let res = self.inner.pool.call(&cctx, id, ALPN_CLIENT, m).await;
        cctx.cancel();
        let resp = match res {
            Ok(resp) => resp,
            Err(e) => {
                let err = Error::from(e);
                self.handle_err(id, &err);
                return Err(err);
            }
        };
        self.ok(id);
        if (resp.epoch > 0 || resp.incarnation > 0)
            && self
                .view()
                .is_some_and(|v| v.compare(resp.incarnation, resp.epoch) == Ordering::Less)
        {
            self.spawn_refresh();
        }
        Ok(resp)
    }

    /// `go c.RefreshView(context.Background())`: fire and forget, one task per call.
    fn spawn_refresh(&self) {
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let c = self.clone();
        let fut: futures::future::BoxFuture<'static, ()> = Box::pin(async move {
            let _ = c.refresh_view(&Ctx::background()).await;
        });
        rt.spawn(fut);
    }

    /// `callRetry`: ≤ 4 calls on stale-view.
    pub(crate) async fn call_retry(
        &self,
        ctx: &Ctx,
        id: NodeId,
        m: &mut Msg,
    ) -> Result<Msg, Error> {
        let mut res = self.call(ctx, id, m).await;
        for _ in 1..CALL_RETRY_ATTEMPTS {
            match &res {
                Err(e) if e.is_code(CODE_STALE_VIEW) => res = self.call(ctx, id, m).await,
                _ => break,
            }
        }
        res
    }

    /// `anyNode`: nodes in preference order until one answers; ≤ 5 calls per node on stale-view. A
    /// remote answer stands; a cancelled ctx does not stop the loop.
    pub(crate) async fn any_node(&self, ctx: &Ctx, m: &mut Msg) -> Result<Msg, Error> {
        let mut ids: Vec<NodeId> = self
            .view()
            .map(|v| v.nodes().iter().map(Node::nid).collect())
            .unwrap_or_default();
        if ids.is_empty() {
            ids = self.read().boot_order.clone();
        }
        let mut last_err = None;
        for id in self.preferred(&ids) {
            let mut res = self.call(ctx, id, m).await;
            let mut attempt = 0;
            while attempt < ANY_NODE_STALE_RETRIES
                && matches!(&res, Err(e) if e.is_code(CODE_STALE_VIEW))
            {
                // The node was ahead of this client's view; handle_err adopted the view it sent, so the
                // retry carries the new epoch.
                res = self.call(ctx, id, m).await;
                attempt += 1;
            }
            match res {
                Ok(resp) => return Ok(resp),
                Err(e) if e.remote().is_some() => return Err(e),
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err.unwrap_or(Error::NoNodes))
    }

    /// `handleErr`: a remote answer adopts its carried view and is never penalised; any other failure
    /// adds backoff.
    pub(crate) fn handle_err(&self, id: NodeId, err: &Error) {
        if let Some(we) = err.remote() {
            if !we.view.is_empty()
                && let Ok(v) = View::decode(&we.view)
            {
                self.adopt(v);
            }
            return;
        }
        let mut st = self.write();
        let failures = st.failures.entry(id).or_insert(0);
        *failures = failures.saturating_add(1);
        let d = backoff_after(*failures);
        let now = tokio::time::Instant::now();
        st.backoff.insert(id, now.checked_add(d).unwrap_or(now));
    }

    /// `ok`: clears the node's backoff, failures and unreachable hint.
    pub(crate) fn ok(&self, id: NodeId) {
        let mut st = self.write();
        st.backoff.remove(&id);
        st.failures.remove(&id);
        st.unreach.remove(&id);
    }

    /// `penalty`: +2 while a backoff runs, +1 for an unreachable hint.
    pub(crate) fn penalty(&self, id: NodeId) -> i64 {
        let st = self.read();
        let mut p = 0;
        if st
            .backoff
            .get(&id)
            .is_some_and(|until| tokio::time::Instant::now() < *until)
        {
            p += 2;
        }
        if st.unreach.contains(&id) {
            p += 1;
        }
        p
    }

    /// `preferred`: `rankOwners` with the live connection's current path.
    pub(crate) fn preferred(&self, ids: &[NodeId]) -> Vec<NodeId> {
        rank_owners(
            ids,
            |id| self.penalty(id),
            |id| self.inner.pool.path(id, ALPN_CLIENT),
        )
    }

    /// `probeHinted`: a 3 s view request to every hinted node, concurrently; results ignored.
    pub(crate) async fn probe_hinted(&self, ctx: &Ctx) {
        let mut ids: Vec<NodeId> = self.read().unreach.iter().copied().collect();
        ids.sort();
        let mut tasks = tokio::task::JoinSet::new();
        for id in ids {
            let c = self.clone();
            let pctx = ctx.with_timeout(PROBE_TIMEOUT);
            tasks.spawn(async move {
                let mut m = Msg {
                    typ: T_VIEW,
                    ..Msg::default()
                };
                let _ = c.call(&pctx, id, &mut m).await;
                pctx.cancel();
            });
        }
        while tasks.join_next().await.is_some() {}
    }

    /// `pathAttrs`: the client's connection to id for a log line.
    pub(crate) fn path_attrs(&self, id: NodeId) -> Vec<Attr> {
        path_attrs_of(self.inner.pool.path(id, ALPN_CLIENT))
    }
}

/// A scripted in-memory node set for the crate's tests: each node serves one request per stream over
/// `dstore_transport::mem`, as `node/server.go` does, from a handler.
#[cfg(test)]
pub(crate) mod test_node {
    use std::sync::{Mutex, MutexGuard};

    use dstore_gocompat::slog::{Handler as LogHandler, Level, Record};
    use dstore_transport::Stream;
    use dstore_transport::mem::{MemEndpoint, Network};
    use tokio::io::AsyncReadExt;

    use super::*;

    /// What a node does with a request.
    pub(crate) enum Reply {
        /// Write the frames, then finish.
        Frames(Vec<Msg>),
        /// Finish without a frame (the client reads `EOF`).
        Close,
        /// Forward frames until the channel closes, then finish.
        Stream(tokio::sync::mpsc::Receiver<Msg>),
    }

    /// A node's request handler: (initial view, node id, request).
    pub(crate) type Handler = Arc<dyn Fn(&View, NodeId, &Msg) -> Reply + Send + Sync>;

    pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
        m.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Node i: `[i+1; 32]`, so ids sort by index.
    pub(crate) fn node_id(i: usize) -> NodeId {
        NodeId([i as u8 + 1; 32])
    }

    pub(crate) const CLIENT_ID: NodeId = NodeId([0xcc; 32]);

    /// The error of a call that must fail.
    pub(crate) trait Fail {
        fn fail(self, what: &str) -> Error;
    }

    impl<T> Fail for Result<T, Error> {
        fn fail(self, what: &str) -> Error {
            match self {
                Ok(_) => panic!("{what}: the call succeeded"),
                Err(e) => e,
            }
        }
    }

    /// The test view of client-core §3.3 over the given nodes: cluster id 00..0f, incarnation 1, epoch 7,
    /// version 9, R 3, min 2.
    pub(crate) fn test_view(ids: &[NodeId]) -> View {
        View {
            cluster_id: Some((0u8..16).collect()),
            incarnation: 1,
            epoch: 7,
            version: 9,
            placement_epoch: 7,
            replicas: 3,
            min_replicas: 2,
            voters: Some(Vec::new()),
            nodes: Some(
                ids.iter()
                    .map(|id| Node {
                        id: Some(id.0.to_vec()),
                        weight: 100,
                        writable: true,
                        ..Node::default()
                    })
                    .collect(),
            ),
            ..View::default()
        }
    }

    /// `node.handleView`'s reply, stamped with the view's incarnation and epoch.
    pub(crate) fn view_reply(v: &View, unreachable: &[NodeId]) -> Msg {
        Msg {
            typ: T_VIEW_REPLY,
            incarnation: v.incarnation,
            epoch: v.epoch,
            view: v.encode(),
            unreachable: unreachable.iter().map(|id| id.0.to_vec()).collect(),
            ..Msg::default()
        }
    }

    /// Views answer with the initial view; every other request takes the next scripted reply (none left:
    /// close the stream).
    pub(crate) fn scripted(replies: Arc<Mutex<Vec<Msg>>>) -> Handler {
        Arc::new(move |v, _id, req| {
            if req.typ == T_VIEW {
                return Reply::Frames(vec![view_reply(v, &[])]);
            }
            let mut r = lock(&replies);
            if r.is_empty() {
                Reply::Close
            } else {
                Reply::Frames(vec![r.remove(0)])
            }
        })
    }

    /// A log handler that keeps every record.
    #[derive(Default)]
    pub(crate) struct Capture(Mutex<Vec<Record>>);

    impl LogHandler for Capture {
        fn enabled(&self, _level: Level) -> bool {
            true
        }
        fn handle(&self, _handler_attrs: &[Attr], r: &Record) {
            lock(&self.0).push(r.clone());
        }
    }

    type Transcript = Arc<Mutex<Vec<(NodeId, Msg)>>>;

    pub(crate) struct TestNet {
        /// Keeps the network alive: endpoints hold it weakly.
        #[allow(dead_code)]
        pub(crate) net: Arc<Network>,
        pub(crate) ids: Vec<NodeId>,
        pub(crate) view: View,
        client: Arc<MemEndpoint>,
        transcript: Transcript,
        capture: Arc<Capture>,
    }

    impl TestNet {
        /// n nodes under the test view.
        pub(crate) async fn start(n: usize, handler: Handler) -> TestNet {
            let ids: Vec<NodeId> = (0..n).map(node_id).collect();
            let view = test_view(&ids);
            TestNet::start_with_view(ids, view, handler).await
        }

        pub(crate) async fn start_with_view(
            ids: Vec<NodeId>,
            view: View,
            handler: Handler,
        ) -> TestNet {
            let net = Network::new();
            let transcript: Transcript = Arc::default();
            for &id in &ids {
                let ep = net.bind(id, &[ALPN_CLIENT]);
                spawn_node(
                    ep,
                    id,
                    Arc::new(view.clone()),
                    handler.clone(),
                    transcript.clone(),
                );
            }
            let client = net.bind(CLIENT_ID, &[]);
            TestNet {
                net,
                ids,
                view,
                client,
                transcript,
                capture: Arc::default(),
            }
        }

        /// A config naming every node, in order, with the capturing logger.
        pub(crate) fn config(&self) -> Config {
            let endpoint: Arc<dyn Endpoint> = self.client.clone();
            Config {
                endpoint: Some(endpoint),
                ticket: Ticket {
                    cluster_id: self.view.cluster_id.clone(),
                    incarnation: 1,
                    members: Some(
                        self.ids
                            .iter()
                            .map(|id| Member {
                                id: Some(id.0.to_vec()),
                                addrs: Vec::new(),
                            })
                            .collect(),
                    ),
                },
                logger: Some(Logger::new(self.capture.clone())),
                ..Config::default()
            }
        }

        pub(crate) async fn dial(&self) -> Cluster {
            match Cluster::dial(&Ctx::background(), self.config()).await {
                Ok(c) => c,
                Err(e) => panic!("dial: {e}"),
            }
        }

        /// Every request the nodes read, in arrival order.
        pub(crate) fn requests(&self) -> Vec<(NodeId, Msg)> {
            lock(&self.transcript).clone()
        }

        /// The requests of one type.
        pub(crate) fn requests_of(&self, typ: i64) -> Vec<(NodeId, Msg)> {
            self.requests()
                .into_iter()
                .filter(|(_, m)| m.typ == typ)
                .collect()
        }

        pub(crate) fn records(&self) -> Vec<Record> {
            lock(&self.capture.0).clone()
        }
    }

    fn spawn_node(
        ep: Arc<MemEndpoint>,
        id: NodeId,
        view: Arc<View>,
        handler: Handler,
        transcript: Transcript,
    ) {
        tokio::spawn(async move {
            let ctx = Ctx::background();
            while let Ok(conn) = ep.accept(&ctx).await {
                let (view, handler, transcript) =
                    (view.clone(), handler.clone(), transcript.clone());
                tokio::spawn(async move {
                    let ctx = Ctx::background();
                    while let Ok(s) = conn.accept_stream(&ctx).await {
                        tokio::spawn(serve_stream(
                            s,
                            id,
                            view.clone(),
                            handler.clone(),
                            transcript.clone(),
                        ));
                    }
                });
            }
        });
    }

    async fn serve_stream(
        mut s: Stream,
        id: NodeId,
        view: Arc<View>,
        handler: Handler,
        transcript: Transcript,
    ) {
        let Ok(req) = dstore_wire::read_msg(&mut *s.recv).await else {
            return;
        };
        lock(&transcript).push((id, req.clone()));
        match handler(&view, id, &req) {
            Reply::Frames(frames) => {
                for f in &frames {
                    if dstore_wire::write_msg(&mut *s.send, f).await.is_err() {
                        break;
                    }
                }
                s.send.finish();
            }
            Reply::Close => s.send.finish(),
            Reply::Stream(mut rx) => {
                while let Some(f) = rx.recv().await {
                    if dstore_wire::write_msg(&mut *s.send, &f).await.is_err() {
                        break;
                    }
                }
                s.send.finish();
            }
        }
        // Read to the client's FIN, so the receive half is never reset under the client.
        let mut buf = [0u8; 256];
        while let Ok(n) = s.recv.read(&mut buf).await {
            if n == 0 {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use dstore_gocompat::slog::Value;
    use dstore_testkit::golden::{self, decimal_i64};
    use dstore_transport::TransportError;
    use dstore_wire::{
        CODE_BAD_REQUEST, CODE_UNAVAILABLE, RemoteError, T_OK, T_REF_GET, T_STATUS_REPLY, err_msg,
    };
    use serde::Deserialize;

    use super::test_node::{self, CLIENT_ID, Fail, Handler, Reply, TestNet, node_id, view_reply};
    use super::*;

    fn frame_hex(m: &Msg) -> String {
        match dstore_wire::encode_frame(m) {
            Ok(b) => dstore_gocompat::hex::encode(&b),
            Err(e) => panic!("encode: {e}"),
        }
    }

    fn view_with(epoch: u64, version: u64, ids: &[NodeId]) -> View {
        View {
            epoch,
            version,
            ..test_node::test_view(ids)
        }
    }

    async fn dial_err(cfg: Config) -> String {
        match Cluster::dial(&Ctx::background(), cfg).await {
            Ok(_) => panic!("dial succeeded"),
            Err(e) => e.to_string(),
        }
    }

    #[test]
    fn config_default_is_all_zero() {
        let c = Config::default();
        assert!(c.endpoint.is_none());
        assert!(c.logger.is_none());
        assert_eq!(c.ticket, Ticket::default());
        assert_eq!((c.conns, c.jobs, c.batch_bytes), (0, 0, 0));
        assert!(c.gc_interval.is_zero() && c.request_timeout.is_zero() && c.watch_idle.is_zero());
    }

    #[tokio::test]
    async fn dial_applies_the_defaults() {
        let tn = TestNet::start(1, test_node::scripted(Arc::default())).await;
        let c = tn.dial().await;
        let cfg = c.cfg();
        assert_eq!(cfg.conns, 4);
        assert_eq!(cfg.jobs, 8);
        assert_eq!(cfg.gc_interval, Duration::from_secs(4 * 3600));
        assert_eq!(cfg.request_timeout, Duration::from_secs(120));
        assert_eq!(cfg.batch_bytes, 16 << 20);
        assert_eq!(cfg.watch_idle, Duration::from_secs(120));
        assert!(cfg.logger.is_some());

        let custom = Config {
            conns: 2,
            jobs: 3,
            gc_interval: Duration::from_secs(1),
            request_timeout: Duration::from_secs(20),
            batch_bytes: 100 << 20,
            watch_idle: Duration::from_secs(5),
            ..tn.config()
        };
        let c = match Cluster::dial(&Ctx::background(), custom).await {
            Ok(c) => c,
            Err(e) => panic!("dial: {e}"),
        };
        let cfg = c.cfg();
        assert_eq!((cfg.conns, cfg.jobs), (2, 3));
        assert_eq!(cfg.batch_bytes, 64 << 20);
        assert_eq!(cfg.request_timeout, Duration::from_secs(20));
        assert_eq!(cfg.watch_idle, Duration::from_secs(5));
        assert_eq!(c.nodes(), tn.ids);
        let ep: &Arc<dyn Endpoint> = c.endpoint();
        assert_eq!(ep.id(), CLIENT_ID);
    }

    #[tokio::test]
    async fn dial_errors() {
        // A node that answers view with ok, an error, a bad view or nothing, by id.
        let handler: Handler = Arc::new(|_v, id, _req| match id.0[0] {
            1 => Reply::Frames(vec![Msg {
                typ: T_OK,
                ..Msg::default()
            }]),
            2 => Reply::Frames(vec![err_msg(CODE_UNAVAILABLE, "no view")]),
            3 => Reply::Frames(vec![Msg {
                typ: T_VIEW_REPLY,
                view: vec![0x80],
                ..Msg::default()
            }]),
            _ => Reply::Close,
        });
        let tn = TestNet::start(4, handler).await;
        let only = |i: usize| Config {
            ticket: Ticket {
                members: Some(vec![Member {
                    id: Some(node_id(i).0.to_vec()),
                    addrs: Vec::new(),
                }]),
                ..Ticket::default()
            },
            ..tn.config()
        };

        let no_ep = Config {
            endpoint: None,
            ..tn.config()
        };
        assert_eq!(dial_err(no_ep).await, "client: no endpoint");
        let no_members = Config {
            ticket: Ticket::default(),
            ..tn.config()
        };
        assert_eq!(
            dial_err(no_members).await,
            "client: no bootstrap node answered: client: ticket names no nodes"
        );
        let short = Config {
            ticket: Ticket {
                members: Some(vec![Member {
                    id: Some(vec![1; 31]),
                    addrs: Vec::new(),
                }]),
                ..Ticket::default()
            },
            ..tn.config()
        };
        assert_eq!(
            dial_err(short).await,
            "client: no bootstrap node answered: client: ticket names no nodes"
        );
        assert_eq!(
            dial_err(only(0)).await,
            "client: no bootstrap node answered: client: unexpected reply 53"
        );
        assert_eq!(
            dial_err(only(1)).await,
            "client: no bootstrap node answered: remote: unavailable: no view"
        );
        assert_eq!(
            dial_err(only(2)).await,
            "client: no bootstrap node answered: view: decode: cbor: cannot unmarshal array into Go value of type view.View (cannot decode CBOR array to struct without toarray option)"
        );
        assert_eq!(
            dial_err(only(3)).await,
            "client: no bootstrap node answered: EOF"
        );
        let unbound = NodeId([0x77; 32]);
        let last_wins = Config {
            ticket: Ticket {
                members: Some(vec![
                    Member {
                        id: Some(node_id(0).0.to_vec()),
                        addrs: Vec::new(),
                    },
                    Member {
                        id: Some(unbound.0.to_vec()),
                        addrs: Vec::new(),
                    },
                ]),
                ..Ticket::default()
            },
            ..tn.config()
        };
        assert_eq!(
            dial_err(last_wins).await,
            "client: no bootstrap node answered: mem: 77777777 not bound"
        );
        // The view request is unstamped (client-core §3.2).
        let views = tn.requests();
        assert!(!views.is_empty());
        for (_, m) in views {
            assert_eq!(frame_hex(&m), "00000004a1001820");
        }
    }

    #[tokio::test]
    async fn dial_tries_members_in_order_and_logs_connected() {
        let handler: Handler = Arc::new(|v, id, req| {
            if id == node_id(0) {
                Reply::Close
            } else if req.typ == T_VIEW {
                Reply::Frames(vec![view_reply(v, &[node_id(2)])])
            } else {
                Reply::Close
            }
        });
        let tn = TestNet::start(3, handler).await;
        let c = tn.dial().await;
        let order: Vec<NodeId> = tn.requests().into_iter().map(|(id, _)| id).collect();
        assert_eq!(order, [node_id(0), node_id(1)]);
        assert_eq!(c.penalty(node_id(2)), 1, "the reply's unreachable hint");
        // Dial does not go through call: the failed member is not penalised.
        assert_eq!(c.penalty(node_id(0)), 0);

        let recs = tn.records();
        assert_eq!(recs.len(), 1);
        let r = &recs[0];
        assert_eq!(r.level, dstore_gocompat::slog::Level::INFO);
        assert_eq!(r.message, "connected");
        assert_eq!(
            r.attrs,
            vec![
                Attr::string("node", "02020202"),
                Attr::int64("nodes", 3),
                Attr::string("path", "direct"),
                Attr::duration("rtt", 1_000_000),
            ]
        );
    }

    #[tokio::test]
    async fn adopt_orders_views_by_incarnation_epoch_version() {
        let tn = TestNet::start(3, test_node::scripted(Arc::default())).await;
        let c = tn.dial().await;
        let first = c.view().expect("view");
        assert_eq!((first.incarnation, first.epoch, first.version), (1, 7, 9));

        c.adopt(view_with(7, 9, &tn.ids));
        assert!(
            Arc::ptr_eq(&first, &c.view().expect("view")),
            "equal triple not re-adopted"
        );
        c.adopt(view_with(7, 8, &tn.ids));
        assert_eq!(c.view().map(|v| v.version), Some(9));
        c.adopt(view_with(7, 10, &tn.ids));
        assert_eq!(c.view().map(|v| v.version), Some(10));
        c.adopt(view_with(6, 100, &tn.ids));
        assert_eq!(c.view().map(|v| (v.epoch, v.version)), Some((7, 10)));
        c.adopt(View {
            incarnation: 2,
            ..view_with(0, 0, &tn.ids[..1])
        });
        assert_eq!(c.view().map(|v| (v.incarnation, v.epoch)), Some((2, 0)));
        assert_eq!(c.nodes(), [node_id(0)]);
        let pl = c.placement().expect("placement");
        assert!(Arc::ptr_eq(pl.view(), &c.view().expect("view")));

        // adoptReply replaces the hints even when it keeps the newer view; 31-byte entries are dropped.
        let mut reply = view_reply(&view_with(1, 1, &tn.ids), &[node_id(1)]);
        reply.unreachable.push(vec![9; 31]);
        c.adopt_reply(&reply).expect("adopt reply");
        assert_eq!(c.view().map(|v| v.incarnation), Some(2));
        assert_eq!(c.penalty(node_id(1)), 1);
        let reply = view_reply(&view_with(1, 1, &tn.ids), &[]);
        c.adopt_reply(&reply).expect("adopt reply");
        assert_eq!(c.penalty(node_id(1)), 0);
        let err = c
            .adopt_reply(&Msg {
                typ: T_OK,
                ..Msg::default()
            })
            .fail("unexpected");
        assert_eq!(err.to_string(), "client: unexpected reply 53");
    }

    #[derive(Deserialize)]
    struct BackoffCase {
        failures: u64,
        #[serde(deserialize_with = "decimal_i64")]
        backoff_ns: i64,
    }

    #[derive(Deserialize)]
    struct BackoffFile {
        handle_err: Vec<BackoffCase>,
        #[serde(deserialize_with = "decimal_i64")]
        dial_member_timeout_ns: i64,
        #[serde(deserialize_with = "decimal_i64")]
        probe_timeout_ns: i64,
    }

    #[tokio::test(start_paused = true)]
    async fn golden_backoff() {
        let f: BackoffFile = golden::load_json("client/backoff.json");
        assert_eq!(
            dstore_gocompat::time::duration_to_ns(DIAL_MEMBER_TIMEOUT),
            f.dial_member_timeout_ns
        );
        assert_eq!(
            dstore_gocompat::time::duration_to_ns(PROBE_TIMEOUT),
            f.probe_timeout_ns
        );
        let tn = TestNet::start(1, test_node::scripted(Arc::default())).await;
        let c = tn.dial().await;
        let id = NodeId([0x42; 32]);
        for case in &f.handle_err {
            assert_eq!(
                dstore_gocompat::time::duration_to_ns(backoff_after(case.failures)),
                case.backoff_ns,
                "failures {}",
                case.failures
            );
            c.write().failures.insert(id, case.failures - 1);
            c.handle_err(id, &Error::Transport(TransportError::Closed));
            let st = c.read();
            assert_eq!(st.failures.get(&id), Some(&case.failures));
            let until = st.backoff.get(&id).copied().expect("backoff");
            let d = until - tokio::time::Instant::now();
            assert_eq!(
                dstore_gocompat::time::duration_to_ns(d),
                case.backoff_ns,
                "failures {}",
                case.failures
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn penalty_backoff_hints_and_remote_errors() {
        let tn = TestNet::start(3, test_node::scripted(Arc::default())).await;
        let c = tn.dial().await;
        let id = node_id(1);
        let transport = Error::Transport(TransportError::RecentlyUnreachable);

        c.handle_err(id, &transport);
        assert_eq!(c.penalty(id), 2);
        tokio::time::advance(Duration::from_millis(4999)).await;
        assert_eq!(c.penalty(id), 2);
        tokio::time::advance(Duration::from_millis(1)).await;
        assert_eq!(c.penalty(id), 0, "expired at its instant");
        // The expired entry still counts failures: the next backoff is 10 s.
        c.handle_err(id, &transport);
        tokio::time::advance(Duration::from_secs(9)).await;
        assert_eq!(c.penalty(id), 2);
        c.adopt_reply(&view_reply(&tn.view, &[id])).expect("hint");
        assert_eq!(c.penalty(id), 3);
        c.ok(id);
        assert_eq!(c.penalty(id), 0);
        assert!(c.read().failures.is_empty());

        // A remote error is never penalised, and its view is adopted.
        let newer = view_with(8, 1, &tn.ids);
        let remote = Error::Remote(RemoteError {
            code: CODE_STALE_VIEW.into(),
            text: "request epoch is behind".into(),
            view: newer.encode(),
            retry_after: Duration::ZERO,
        });
        c.handle_err(id, &remote);
        assert_eq!(c.penalty(id), 0);
        assert_eq!(c.view().map(|v| v.epoch), Some(8));
        // An undecodable carried view is ignored.
        let bad = Error::Remote(RemoteError {
            code: CODE_STALE_VIEW.into(),
            text: String::new(),
            view: vec![0x80],
            retry_after: Duration::ZERO,
        });
        c.handle_err(id, &bad);
        assert_eq!(c.view().map(|v| v.epoch), Some(8));
        assert_eq!(c.penalty(id), 0);
    }

    #[tokio::test]
    async fn call_stamps_and_refreshes_a_newer_view() {
        let ids: Vec<NodeId> = (0..3).map(node_id).collect();
        let newer = view_with(8, 1, &ids);
        let newer_bytes = newer.clone();
        let handler: Handler = Arc::new(move |v, _id, req| match req.typ {
            T_VIEW if req.epoch == 7 => Reply::Frames(vec![view_reply(&newer_bytes, &[])]),
            T_VIEW => Reply::Frames(vec![view_reply(v, &[])]),
            _ => Reply::Frames(vec![Msg {
                typ: T_STATUS_REPLY,
                incarnation: 1,
                epoch: 8,
                status: vec![0xa0],
                ..Msg::default()
            }]),
        });
        let tn = TestNet::start(3, handler).await;
        let c = tn.dial().await;
        let status = c
            .status(&Ctx::background(), node_id(1))
            .await
            .expect("status");
        assert_eq!(status, [0xa0]);
        let reqs = tn.requests_of(T_STATUS);
        assert_eq!(
            frame_hex(&reqs[0].1),
            "0000001aa40018280150000102030405060708090a0b0c0d0e0f02010307"
        );
        for _ in 0..2000 {
            if c.view().is_some_and(|v| v.epoch == 8) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        assert_eq!(c.view().map(|v| v.epoch), Some(8));
        let refresh: Vec<Msg> = tn
            .requests_of(T_VIEW)
            .into_iter()
            .map(|(_, m)| m)
            .filter(|m| m.epoch == 7)
            .collect();
        assert!(!refresh.is_empty());
        assert_eq!(
            frame_hex(&refresh[0]),
            "0000001aa40018200150000102030405060708090a0b0c0d0e0f02010307"
        );
        // Now stamped with the adopted epoch.
        c.status(&Ctx::background(), node_id(1))
            .await
            .expect("status");
        assert_eq!(tn.requests_of(T_STATUS)[1].1.epoch, 8);
    }

    #[tokio::test]
    async fn status_does_not_check_the_reply_type() {
        let replies = Arc::new(Mutex::new(vec![Msg {
            typ: T_OK,
            ..Msg::default()
        }]));
        let tn = TestNet::start(1, test_node::scripted(replies)).await;
        let c = tn.dial().await;
        let got = c
            .status(&Ctx::background(), node_id(0))
            .await
            .expect("status");
        assert!(got.is_empty());
    }

    #[tokio::test]
    async fn call_retry_and_any_node_stale_view_attempts() {
        let handler: Handler = Arc::new(|v, _id, req| {
            if req.typ == T_VIEW {
                Reply::Frames(vec![view_reply(v, &[])])
            } else {
                Reply::Frames(vec![err_msg(CODE_STALE_VIEW, "request epoch is behind")])
            }
        });
        let tn = TestNet::start(2, handler).await;
        let c = tn.dial().await;
        let ctx = Ctx::background();
        let mut m = Msg {
            typ: T_REF_GET,
            name: "x".into(),
            ..Msg::default()
        };
        let err = c.call_retry(&ctx, node_id(1), &mut m).await.fail("stale");
        assert!(err.is_code(CODE_STALE_VIEW));
        assert_eq!(tn.requests_of(T_REF_GET).len(), 4);

        let err = c.any_node(&ctx, &mut m).await.fail("stale");
        assert!(err.is_code(CODE_STALE_VIEW));
        let after: Vec<NodeId> = tn
            .requests_of(T_REF_GET)
            .into_iter()
            .skip(4)
            .map(|(id, _)| id)
            .collect();
        assert_eq!(
            after,
            [node_id(0); 5],
            "5 calls on the first node, none on the next"
        );
        assert_eq!(c.penalty(node_id(0)), 0);
    }

    // client.go:287-292: callRetry retries only stale-view; any other outcome is returned at once.
    #[tokio::test]
    async fn call_retry_stops_on_any_other_outcome() {
        let replies = Arc::new(Mutex::new(vec![
            err_msg(CODE_BAD_REQUEST, "nope"),
            Msg {
                typ: T_OK,
                ..Msg::default()
            },
        ]));
        let tn = TestNet::start(1, test_node::scripted(replies)).await;
        let c = tn.dial().await;
        let ctx = Ctx::background();
        let mut m = Msg {
            typ: T_REF_GET,
            ..Msg::default()
        };
        let err = c.call_retry(&ctx, node_id(0), &mut m).await.fail("remote");
        assert!(err.is_code(CODE_BAD_REQUEST));
        assert_eq!(tn.requests_of(T_REF_GET).len(), 1);
        c.call_retry(&ctx, node_id(0), &mut m).await.expect("ok");
        assert_eq!(tn.requests_of(T_REF_GET).len(), 2);
        // No reply left: the node finishes without a frame, a non-remote failure, also not retried.
        let err = c.call_retry(&ctx, node_id(0), &mut m).await.fail("eof");
        assert_eq!(err.to_string(), "EOF");
        assert_eq!(tn.requests_of(T_REF_GET).len(), 3);
        assert_eq!(c.penalty(node_id(0)), 2);
    }

    // PORTING.md §1.4: one RefreshView task per reply that carries a newer epoch, with no single-flight.
    // The nodes keep answering views with the older view, so nothing is adopted and each reply spawns one.
    #[tokio::test]
    async fn call_spawns_one_refresh_per_newer_reply() {
        let handler: Handler = Arc::new(|v, _id, req| match req.typ {
            T_VIEW => Reply::Frames(vec![view_reply(v, &[])]),
            _ => Reply::Frames(vec![Msg {
                typ: T_STATUS_REPLY,
                incarnation: 1,
                epoch: 8,
                ..Msg::default()
            }]),
        });
        let tn = TestNet::start(2, handler).await;
        let c = tn.dial().await;
        let ctx = Ctx::background();
        for _ in 0..3 {
            c.status(&ctx, node_id(1)).await.expect("status");
        }
        // Dial's view request is unstamped; the refreshes carry the cached epoch 7.
        let refreshes = || {
            tn.requests_of(T_VIEW)
                .into_iter()
                .filter(|(_, m)| m.epoch == 7)
                .count()
        };
        for _ in 0..2000 {
            if refreshes() >= 3 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(refreshes(), 3);
        assert_eq!(c.view().map(|v| v.epoch), Some(7));
    }

    #[tokio::test]
    async fn any_node_moves_on_after_transport_errors_only() {
        let handler: Handler = Arc::new(|v, id, req| {
            if req.typ == T_VIEW {
                return Reply::Frames(vec![view_reply(v, &[])]);
            }
            match id.0[0] {
                1 => Reply::Close,
                2 => Reply::Frames(vec![err_msg(CODE_BAD_REQUEST, "nope")]),
                _ => Reply::Frames(vec![Msg {
                    typ: T_OK,
                    ..Msg::default()
                }]),
            }
        });
        let tn = TestNet::start(3, handler).await;
        let c = tn.dial().await;
        let ctx = Ctx::background();
        let mut m = Msg {
            typ: T_REF_GET,
            ..Msg::default()
        };
        let err = c.any_node(&ctx, &mut m).await.fail("remote");
        assert_eq!(err.to_string(), "remote: bad-request: nope");
        let order: Vec<NodeId> = tn
            .requests_of(T_REF_GET)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(order, [node_id(0), node_id(1)]);
        assert_eq!(c.penalty(node_id(0)), 2);
        assert_eq!(c.penalty(node_id(1)), 0);

        // The penalised node now ranks last.
        let resp = c.any_node(&ctx, &mut m).await;
        assert!(resp.is_err());
        assert_eq!(c.preferred(&tn.ids), [node_id(1), node_id(2), node_id(0)]);

        // No view nodes: the bootstrap nodes; neither: client: no nodes.
        c.adopt(View {
            nodes: Some(Vec::new()),
            ..view_with(9, 0, &[])
        });
        assert!(c.nodes().is_empty());
        assert_eq!(c.all_nodes(), tn.ids);
        c.write().boot_order.clear();
        let err = c.any_node(&ctx, &mut m).await.fail("no nodes");
        assert_eq!(err.to_string(), "client: no nodes");
    }

    #[tokio::test]
    async fn any_node_keeps_calling_after_cancellation() {
        let tn = TestNet::start(3, test_node::scripted(Arc::default())).await;
        let c = tn.dial().await;
        let ctx = Ctx::background();
        ctx.cancel();
        let mut m = Msg {
            typ: T_REF_GET,
            ..Msg::default()
        };
        let err = c.any_node(&ctx, &mut m).await.fail("canceled");
        assert_eq!(err.to_string(), "context canceled");
        assert_eq!(
            err.ctx_error(),
            Some(dstore_gocompat::ctx::CtxError::Canceled)
        );
        for id in &tn.ids {
            assert_eq!(c.penalty(*id), 2, "{id:?} penalised");
        }
    }

    #[tokio::test]
    async fn admin_frames_and_replies() {
        let replies = Arc::new(Mutex::new(vec![
            Msg {
                typ: T_ADMIN_REPLY,
                incarnation: 1,
                epoch: 7,
                status: vec![0xa1, 0x00, 0x62, b'o', b'k'],
                ..Msg::default()
            },
            Msg {
                typ: T_OK,
                ..Msg::default()
            },
            err_msg(CODE_BAD_REQUEST, "unknown admin op nope"),
            Msg {
                typ: T_ADMIN_REPLY,
                ..Msg::default()
            },
        ]));
        let tn = TestNet::start(2, test_node::scripted(replies)).await;
        let c = tn.dial().await;
        let ctx = Ctx::background();
        let req = AdminRequest {
            op: "gc-status".into(),
            ..AdminRequest::default()
        };
        let got = c.admin(&ctx, None, &req).await.expect("admin");
        assert_eq!(got, [0xa1, 0x00, 0x62, b'o', b'k']);
        let admin = tn.requests_of(T_ADMIN);
        assert_eq!(
            frame_hex(&admin[0].1),
            "00000029a50018290150000102030405060708090a0b0c0d0e0f0201030718334ca1006967632d737461747573"
        );
        let err = c
            .admin(&ctx, Some(NodeId::default()), &req)
            .await
            .fail("ok reply");
        assert_eq!(err.to_string(), "client: unexpected reply 53");
        let err = c.admin(&ctx, None, &req).await.fail("remote");
        assert_eq!(
            err.to_string(),
            "remote: bad-request: unknown admin op nope"
        );
        c.admin(&ctx, Some(node_id(1)), &req)
            .await
            .expect("admin to a node");
        let targets: Vec<NodeId> = tn
            .requests_of(T_ADMIN)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(targets, [node_id(0), node_id(0), node_id(0), node_id(1)]);
    }

    #[tokio::test]
    async fn placement_queries_rank_only_the_first_r() {
        let tn = TestNet::start(5, test_node::scripted(Arc::default())).await;
        let c = tn.dial().await;
        let pl = c.placement().expect("placement");
        let mut keys = vec![[0u8; 32], [0xff; 32]];
        keys.extend((0..8u64).map(|i| {
            let mut k = [0u8; 32];
            k.copy_from_slice(&dstore_testkit::splitmix::data(0x4b45_5953 + i, 32));
            k
        }));
        for key in &keys {
            let owners = pl.owners(key);
            assert_eq!(c.owners(key), owners);
            assert_eq!(c.write_set(key), pl.write_set(key));
            // Only the bootstrap node has a (direct, 1 ms) connection: preference is rank order.
            assert_eq!(c.primary(key), owners.first().copied());
            assert_eq!(c.read_order(key), pl.read_order(key));
        }
        let key = keys[0];
        let order = pl.read_order(&key);
        assert_eq!(order.len(), 5);
        c.adopt_reply(&view_reply(&tn.view, &[order[0]]))
            .expect("hint");
        let got = c.read_order(&key);
        assert_eq!(got[..3], [order[1], order[2], order[0]]);
        assert_eq!(got[3..], order[3..]);
        assert_eq!(c.primary(&key), Some(pl.owners(&key)[1]));
    }

    #[tokio::test]
    async fn probe_hinted_clears_answering_hints() {
        let handler: Handler = Arc::new(|v, id, req| {
            if id == node_id(2) && req.epoch != 0 {
                Reply::Close
            } else {
                Reply::Frames(vec![view_reply(v, &[])])
            }
        });
        let tn = TestNet::start(3, handler).await;
        let c = tn.dial().await;
        c.adopt_reply(&view_reply(&tn.view, &[node_id(1), node_id(2)]))
            .expect("hints");
        assert_eq!((c.penalty(node_id(1)), c.penalty(node_id(2))), (1, 1));
        c.probe_hinted(&Ctx::background()).await;
        assert_eq!(c.penalty(node_id(1)), 0);
        assert_eq!(c.penalty(node_id(2)), 3);
        let probes: Vec<NodeId> = tn
            .requests_of(T_VIEW)
            .into_iter()
            .filter(|(_, m)| m.epoch == 7)
            .map(|(id, _)| id)
            .collect();
        assert_eq!(probes.len(), 2);
    }

    #[tokio::test]
    async fn path_attrs_of_a_live_connection() {
        let tn = TestNet::start(2, test_node::scripted(Arc::default())).await;
        let c = tn.dial().await;
        let attrs = c.path_attrs(node_id(0));
        assert!(matches!(&attrs[0].value, Value::String(s) if s == "direct"));
        assert!(matches!(attrs[1].value, Value::Duration(1_000_000)));
        assert_eq!(c.path_attrs(node_id(1)), [Attr::string("path", "none")]);
        c.close();
        assert_eq!(c.path_attrs(node_id(0)), [Attr::string("path", "none")]);
        assert!(c.read().unreach.is_empty());
    }
}
