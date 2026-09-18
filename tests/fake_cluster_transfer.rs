//! Transfer scenarios over `dstore_transport::mem` (owner client-b): push and pull round trips, dedup,
//! `incomplete` renegotiation, `busy` and `stale-view` retries, corrupt copies, rejected records, a node down,
//! negotiation failover, and interrupted pulls resuming. No sockets.
//!
//! The nodes are scripted in this file rather than taken from `dstore_testkit::fake`, so each scenario
//! controls exactly what the cluster answers. They follow `node/data.go` and `node/refs.go` where the client
//! depends on it: `missing` lists the lacking keys and the short holders of present keys, `get` answers
//! `absent` then one pack, `put` verifies each record, refuses keys outside the write set, stores and
//! replicates to the other owners that are up, and `ref-put` checks the placement of every reachable key and
//! the CAS condition. Two harness choices differ from a Go node: a put stream's pack is read before the reply
//! is chosen, so an early error never resets the client's writes, and only `view` replies are stamped, so no
//! background view refresh races a scenario.
//!
//! `Get` itself (early drops, the missing accessor) is tested in `dstore_client::objects`, because iterating a
//! `futures::Stream` needs the `futures` crate, which the root package does not depend on.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use amber_store_core::amberpack::{self, REC_HEADER_SIZE};
use amber_store_core::fstree;
use amber_store_core::ingest;
use amber_store_core::key::{Key, Type};
use amber_store_core::packstore;
use amber_store_core::reference::Reference;
use dstore_client::{
    Cluster, Cond, Config, Ctx, NodeId, Progress, ProgressReport, PullStats, PushStats,
    PutObserver, RecordSizer, RecordSource,
};
use dstore_gocompat::slog::{Attr, Handler, Level, Logger, Record, Value};
use dstore_testkit::fake::{FakeCluster, FakeClusterConfig};
use dstore_testkit::splitmix;
use dstore_ticket::{Member, Ticket};
use dstore_transport::mem::{MemEndpoint, Network};
use dstore_transport::{Endpoint, Stream};
use dstore_view::{Node, Placement, View, Voter};
use dstore_wire::{
    ALPN_CLIENT, CODE_BAD_REQUEST, CODE_STALE_VIEW, CODE_UNKNOWN_REF, KeyHolders, KeyReject, Msg,
    PackReader, PackRecords, PackSender, T_ABSENT, T_CAS_MISMATCH, T_ERR, T_GET, T_INCOMPLETE,
    T_MISSING, T_MISSING_REPLY, T_OK, T_PUT, T_PUT_RESULT, T_REF, T_REF_GET, T_REF_PUT, T_VIEW,
    T_VIEW_REPLY,
};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn ok<T, E: std::fmt::Display>(r: Result<T, E>) -> T {
    match r {
        Ok(v) => v,
        Err(e) => panic!("{e}"),
    }
}

/// Go's test node ids: `id[0] = id[31] = i`.
fn nid(i: u8) -> NodeId {
    let mut id = [0u8; 32];
    id[0] = i;
    id[31] = i;
    NodeId(id)
}

fn len_i64(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

fn is_leaf(k: &Key) -> bool {
    matches!(
        Type::from_u8(k.0[0] >> 4),
        Some(Type::Blob | Type::XattrSet)
    )
}

// ---- the scripted cluster ----

/// What a node answers besides the protocol.
#[derive(Default)]
struct Script {
    /// TErr replies (code, text, retry_after ms) for the next put streams.
    put_errors: VecDeque<(String, String, i64)>,
    /// A TErr reply (code, text) for every `missing`.
    missing_error: Option<(String, String)>,
    /// Keys whose records this node serves over a flipped payload byte (with a valid CRC).
    corrupt: HashSet<[u8; 32]>,
    /// A reason every record of a put is rejected with.
    reject: Option<String>,
    /// A pause before a put is answered (store and replicate time).
    put_delay: Option<Duration>,
    /// Truncate the first lacking key of every `missing` reply to 31 bytes.
    missing_bad_lacking: bool,
}

/// The catalog: name → (reference record, version).
type RefTable = BTreeMap<String, (Vec<u8>, Vec<u8>)>;

struct TestNode {
    id: NodeId,
    store: Mutex<BTreeMap<[u8; 32], Vec<u8>>>,
    requests: Mutex<Vec<Msg>>,
    script: Mutex<Script>,
}

impl TestNode {
    fn has(&self, k: &[u8; 32]) -> bool {
        lock(&self.store).contains_key(k)
    }
}

struct Shared {
    view: Mutex<View>,
    nodes: Vec<Arc<TestNode>>,
    down: Mutex<HashSet<NodeId>>,
    refs: Mutex<RefTable>,
    versions: Mutex<u64>,
    /// `incomplete` answers left for ref-puts, whichever node gets them.
    incomplete: Mutex<usize>,
}

impl Shared {
    fn placement(&self) -> Placement {
        Placement::new(Arc::new(lock(&self.view).clone()))
    }

    fn node(&self, id: &NodeId) -> Option<&Arc<TestNode>> {
        self.nodes.iter().find(|n| n.id == *id)
    }
}

fn make_view(ids: &[NodeId], replicas: u8, min_replicas: u8, epoch: u64) -> View {
    View {
        cluster_id: Some(vec![0x11; 16]),
        incarnation: 1,
        epoch,
        version: epoch,
        placement_epoch: 1,
        replicas,
        min_replicas,
        voters: Some(vec![Voter {
            id: Some(ids[0].0.to_vec()),
            since: 1,
        }]),
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

struct TestCluster {
    net: Arc<Network>,
    shared: Arc<Shared>,
    ctx: Ctx,
}

impl Drop for TestCluster {
    fn drop(&mut self) {
        self.ctx.cancel();
    }
}

impl TestCluster {
    async fn start(n: u8, replicas: u8, min_replicas: u8) -> TestCluster {
        let net = Network::new();
        let ids: Vec<NodeId> = (1..=n).map(nid).collect();
        let nodes = ids
            .iter()
            .map(|id| {
                Arc::new(TestNode {
                    id: *id,
                    store: Mutex::default(),
                    requests: Mutex::default(),
                    script: Mutex::default(),
                })
            })
            .collect();
        let shared = Arc::new(Shared {
            view: Mutex::new(make_view(&ids, replicas, min_replicas, 7)),
            nodes,
            down: Mutex::default(),
            refs: Mutex::default(),
            versions: Mutex::new(0),
            incomplete: Mutex::new(0),
        });
        let ctx = Ctx::background().with_cancel();
        for (i, id) in ids.iter().enumerate() {
            let ep = net.bind(*id, &[ALPN_CLIENT]);
            tokio::spawn(serve(ep, Arc::clone(&shared), i, ctx.clone()));
        }
        TestCluster { net, shared, ctx }
    }

    fn node(&self, i: usize) -> &Arc<TestNode> {
        &self.shared.nodes[i]
    }

    fn set_down(&self, i: usize, down: bool) {
        let id = self.node(i).id;
        self.net.set_down(id, down);
        if down {
            lock(&self.shared.down).insert(id);
        } else {
            lock(&self.shared.down).remove(&id);
        }
    }

    fn bump_epoch(&self, epoch: u64) {
        let mut v = lock(&self.shared.view);
        v.epoch = epoch;
        v.version = epoch;
    }

    /// Every request of a type, with the index of the node that received it, in arrival order per node.
    fn requests(&self, typ: i64) -> Vec<(usize, Msg)> {
        let mut out = Vec::new();
        for (i, n) in self.shared.nodes.iter().enumerate() {
            for m in lock(&n.requests).iter() {
                if m.typ == typ {
                    out.push((i, m.clone()));
                }
            }
        }
        out
    }

    fn holders(&self, k: &[u8; 32]) -> usize {
        self.shared.nodes.iter().filter(|n| n.has(k)).count()
    }

    async fn dial(&self) -> (Cluster, Arc<Capture>) {
        self.dial_with(|_| {}).await
    }

    async fn dial_with(&self, tweak: impl FnOnce(&mut Config)) -> (Cluster, Arc<Capture>) {
        let cap = Arc::new(Capture::default());
        let ep: Arc<dyn Endpoint> = self.net.bind(nid(100), &[ALPN_CLIENT]);
        let handler: Arc<dyn Handler> = Arc::clone(&cap) as Arc<dyn Handler>;
        let mut cfg = Config {
            endpoint: Some(ep),
            ticket: Ticket {
                cluster_id: Some(vec![0x11; 16]),
                incarnation: 1,
                members: Some(vec![Member {
                    id: Some(self.node(0).id.0.to_vec()),
                    addrs: Vec::new(),
                }]),
            },
            logger: Some(Logger::new(handler)),
            request_timeout: Duration::from_secs(20),
            ..Config::default()
        };
        tweak(&mut cfg);
        match Cluster::dial(&Ctx::background(), cfg).await {
            Ok(c) => (c, cap),
            Err(e) => panic!("dial: {e}"),
        }
    }
}

async fn serve(ep: Arc<MemEndpoint>, sh: Arc<Shared>, me: usize, ctx: Ctx) {
    while let Ok(conn) = ep.accept(&ctx).await {
        let (sh, ctx) = (Arc::clone(&sh), ctx.clone());
        tokio::spawn(async move {
            while let Ok(s) = conn.accept_stream(&ctx).await {
                let sh = Arc::clone(&sh);
                tokio::spawn(async move {
                    let _ = handle(&sh, me, s).await;
                });
            }
        });
    }
}

async fn handle(sh: &Shared, me: usize, mut s: Stream) -> Result<(), dstore_wire::WireError> {
    let m = dstore_wire::read_msg(&mut s.recv).await?;
    lock(&sh.nodes[me].requests).push(m.clone());
    let reply = match m.typ {
        T_VIEW => {
            let v = lock(&sh.view).clone();
            Msg {
                typ: T_VIEW_REPLY,
                incarnation: v.incarnation,
                epoch: v.epoch,
                view: v.encode(),
                ..Msg::default()
            }
        }
        T_MISSING => missing(sh, me, &m),
        T_GET => {
            get(sh, me, &m, &mut s).await?;
            s.close_write();
            return Ok(());
        }
        T_PUT => put(sh, me, &m, &mut s).await,
        T_REF_GET => ref_get(sh, &m),
        T_REF_PUT => ref_put(sh, &m),
        _ => dstore_wire::err_msg(CODE_BAD_REQUEST, "unknown operation"),
    };
    dstore_wire::write_msg(&mut s.send, &reply).await?;
    s.close_write();
    Ok(())
}

fn stale(sh: &Shared) -> Msg {
    Msg {
        typ: T_ERR,
        code: CODE_STALE_VIEW.to_owned(),
        text: "request epoch is behind".to_owned(),
        view: lock(&sh.view).encode(),
        ..Msg::default()
    }
}

fn missing(sh: &Shared, me: usize, m: &Msg) -> Msg {
    let node = &sh.nodes[me];
    if let Some((code, text)) = lock(&node.script).missing_error.clone() {
        return dstore_wire::err_msg(&code, &text);
    }
    let keys = match dstore_wire::keys32(&m.keys) {
        Ok(k) => k,
        Err(e) => return dstore_wire::err_msg(CODE_BAD_REQUEST, &e.to_string()),
    };
    let pl = sh.placement();
    let down = lock(&sh.down).clone();
    let (present, lacking): (Vec<[u8; 32]>, Vec<[u8; 32]>) =
        keys.into_iter().partition(|k| node.has(k));
    let mut short = Vec::new();
    for k in &present {
        let ws = pl.write_set(k);
        let mut holders = vec![node.id.0.to_vec()];
        for o in &ws {
            if *o == node.id || down.contains(o) {
                continue;
            }
            if sh.node(o).is_some_and(|n| n.has(k)) {
                holders.push(o.0.to_vec());
            }
        }
        if holders.len() < ws.len() {
            short.push(KeyHolders {
                key: Some(k.to_vec()),
                holders,
            });
        }
    }
    let mut raw_lacking = dstore_wire::raw_keys(&lacking);
    if lock(&node.script).missing_bad_lacking
        && let Some(first) = raw_lacking.first_mut()
    {
        first.truncate(31);
    }
    Msg {
        typ: T_MISSING_REPLY,
        keys: raw_lacking,
        short,
        ..Msg::default()
    }
}

/// A record over the same key whose payload has its last byte flipped, with a valid CRC.
fn corrupted(rec: &[u8]) -> Vec<u8> {
    let Ok(h) = amberpack::parse_record(rec) else {
        return rec.to_vec();
    };
    let Ok(mut data) = amberpack::decode_payload(h.flags, h.ulen, &rec[REC_HEADER_SIZE..]) else {
        return rec.to_vec();
    };
    if let Some(b) = data.last_mut() {
        *b ^= 0xff;
    }
    amberpack::encode_record(h.key, &data).unwrap_or_else(|_| rec.to_vec())
}

async fn get(
    sh: &Shared,
    me: usize,
    m: &Msg,
    s: &mut Stream,
) -> Result<(), dstore_wire::WireError> {
    let node = &sh.nodes[me];
    let keys = match dstore_wire::keys32(&m.keys) {
        Ok(k) => k,
        Err(e) => {
            let reply = dstore_wire::err_msg(CODE_BAD_REQUEST, &e.to_string());
            return dstore_wire::write_msg(&mut s.send, &reply).await;
        }
    };
    let corrupt = lock(&node.script).corrupt.clone();
    let mut absent = Vec::new();
    let mut recs = Vec::new();
    {
        let store = lock(&node.store);
        for k in &keys {
            match store.get(k) {
                Some(rec) if corrupt.contains(k) => recs.push(corrupted(rec)),
                Some(rec) => recs.push(rec.clone()),
                None => absent.push(*k),
            }
        }
    }
    let reply = Msg {
        typ: T_ABSENT,
        keys: dstore_wire::raw_keys(&absent),
        ..Msg::default()
    };
    dstore_wire::write_msg(&mut s.send, &reply).await?;
    let mut sender = PackSender::new(&mut s.send);
    for rec in &recs {
        sender.add_record(rec).await?;
    }
    sender.finish().await
}

async fn put(sh: &Shared, me: usize, m: &Msg, s: &mut Stream) -> Msg {
    let mut raws = Vec::new();
    let mut pack_err = None;
    {
        let mut records = PackRecords::new(PackReader::new(&mut s.recv));
        while let Some(r) = records.next().await {
            match r {
                Ok(raw) => raws.push(raw),
                Err(e) => {
                    pack_err = Some(e.to_string());
                    break;
                }
            }
        }
        let _ = records.drain().await;
    }
    let v = lock(&sh.view).clone();
    if m.epoch < v.epoch {
        return stale(sh);
    }
    let node = &sh.nodes[me];
    let (scripted, reject, delay) = {
        let mut sc = lock(&node.script);
        (sc.put_errors.pop_front(), sc.reject.clone(), sc.put_delay)
    };
    if let Some(d) = delay {
        tokio::time::sleep(d).await;
    }
    if let Some((code, text, retry_after)) = scripted {
        return Msg {
            typ: T_ERR,
            code,
            text,
            retry_after,
            ..Msg::default()
        };
    }
    if let Some(e) = pack_err {
        return dstore_wire::err_msg(CODE_BAD_REQUEST, &format!("pack: {e}"));
    }
    let pl = Placement::new(Arc::new(v));
    let down = lock(&sh.down).clone();
    let mut holders = Vec::new();
    let mut rejected = Vec::new();
    let mut seen = HashSet::new();
    for raw in raws {
        let k = raw.record.key.0;
        if let Some(reason) = &reject {
            rejected.push(KeyReject {
                key: Some(k.to_vec()),
                reason: reason.clone(),
            });
            continue;
        }
        if let Err(e) = dstore_client::verify_record(&raw) {
            rejected.push(KeyReject {
                key: Some(k.to_vec()),
                reason: format!("verify: {e}"),
            });
            continue;
        }
        if !pl.in_write_set(&k, &node.id) {
            rejected.push(KeyReject {
                key: Some(k.to_vec()),
                reason: "not-owner".to_owned(),
            });
            continue;
        }
        if !seen.insert(k) {
            continue;
        }
        lock(&node.store).insert(k, raw.bytes.clone());
        let mut ids = vec![node.id.0.to_vec()];
        for o in pl.write_set(&k) {
            if o == node.id || down.contains(&o) {
                continue;
            }
            if let Some(n) = sh.node(&o) {
                lock(&n.store).insert(k, raw.bytes.clone());
                ids.push(o.0.to_vec());
            }
        }
        holders.push(KeyHolders {
            key: Some(k.to_vec()),
            holders: ids,
        });
    }
    Msg {
        typ: T_PUT_RESULT,
        holders,
        rejected,
        ..Msg::default()
    }
}

fn ref_get(sh: &Shared, m: &Msg) -> Msg {
    match lock(&sh.refs).get(&m.name) {
        Some((record, version)) => Msg {
            typ: T_REF,
            name: m.name.clone(),
            record: record.clone(),
            version: version.clone(),
            ..Msg::default()
        },
        None => dstore_wire::err_msg(CODE_UNKNOWN_REF, "no such reference"),
    }
}

/// The reachable keys held by fewer than `min(min_replicas, owners)` owners.
fn short_keys(sh: &Shared, root: Key) -> Vec<[u8; 32]> {
    let pl = sh.placement();
    let min_r = usize::from(pl.view().min_replicas);
    let mut short = Vec::new();
    let mut seen = HashSet::from([root]);
    let mut stack = vec![root];
    while let Some(k) = stack.pop() {
        let owners = pl.owners(&k.0);
        let mut record = None;
        let mut count = 0;
        for n in &sh.nodes {
            if let Some(rec) = lock(&n.store).get(&k.0) {
                if owners.contains(&n.id) {
                    count += 1;
                }
                record.get_or_insert_with(|| rec.clone());
            }
        }
        if count < min_r.min(owners.len()) {
            short.push(k.0);
        }
        if is_leaf(&k) {
            continue;
        }
        if let Some(rec) = record
            && let Ok(h) = amberpack::parse_record(&rec)
            && let Ok(data) = amberpack::decode_payload(h.flags, h.ulen, &rec[REC_HEADER_SIZE..])
            && let Ok(kids) = fstree::child_keys(k, &data)
        {
            for kid in kids {
                if seen.insert(kid) {
                    stack.push(kid);
                }
            }
        }
    }
    short
}

fn ref_put(sh: &Shared, m: &Msg) -> Msg {
    if m.epoch < lock(&sh.view).epoch {
        return stale(sh);
    }
    let r = match Reference::decode(&m.record) {
        Ok(r) => r,
        Err(e) => return dstore_wire::err_msg(CODE_BAD_REQUEST, &format!("record: {e}")),
    };
    let root = match Key::parse(&r.key) {
        Ok(k) => k,
        Err(e) => return dstore_wire::err_msg(CODE_BAD_REQUEST, &format!("root key: {e}")),
    };
    {
        let mut inc = lock(&sh.incomplete);
        if *inc > 0 {
            *inc -= 1;
            return Msg {
                typ: T_INCOMPLETE,
                keys: vec![root.0.to_vec()],
                shortfall: 1,
                ..Msg::default()
            };
        }
    }
    let short = short_keys(sh, root);
    if !short.is_empty() {
        return Msg {
            typ: T_INCOMPLETE,
            keys: dstore_wire::raw_keys(&short[..short.len().min(64)]),
            shortfall: len_i64(short.len()),
            ..Msg::default()
        };
    }
    let mut refs = lock(&sh.refs);
    let current = refs.get(&r.name).cloned();
    if !m.force && m.has_expected {
        let matches = match &current {
            None => m.expected_version.is_empty(),
            Some((_, version)) => *version == m.expected_version,
        };
        if !matches {
            return match current {
                None => Msg {
                    typ: T_CAS_MISMATCH,
                    ..Msg::default()
                },
                Some((record, version)) => Msg {
                    typ: T_CAS_MISMATCH,
                    current: Reference::decode(&record)
                        .map(|c| c.key)
                        .unwrap_or_default(),
                    record,
                    version,
                    has_current: true,
                    ..Msg::default()
                },
            };
        }
    }
    let version = {
        let mut n = lock(&sh.versions);
        *n += 1;
        n.to_be_bytes().to_vec()
    };
    refs.insert(r.name.clone(), (m.record.clone(), version.clone()));
    Msg {
        typ: T_OK,
        key: root.0.to_vec(),
        version,
        ..Msg::default()
    }
}

// ---- client-side helpers ----

/// Records every log call of the client.
#[derive(Default)]
struct Capture {
    records: Mutex<Vec<Record>>,
}

impl Handler for Capture {
    fn enabled(&self, _level: Level) -> bool {
        true
    }

    fn handle(&self, _handler_attrs: &[Attr], r: &Record) {
        lock(&self.records).push(r.clone());
    }
}

impl Capture {
    fn find(&self, msg: &str) -> Vec<Record> {
        lock(&self.records)
            .iter()
            .filter(|r| r.message == msg)
            .cloned()
            .collect()
    }
}

fn attr<'a>(r: &'a Record, key: &str) -> Option<&'a Value> {
    r.attrs.iter().find(|a| a.key == key).map(|a| &a.value)
}

fn open_store(dir: &Path) -> Arc<packstore::Store> {
    Arc::new(ok(packstore::Store::open_with(
        dir,
        packstore::Options::new(),
    )))
}

/// A local tree: files `f<i>` (every third under `sub/`) of `size + 37 i` splitmix bytes.
struct LocalTree {
    store: Arc<packstore::Store>,
    root: Key,
    keys: Vec<Key>,
    _dir: tempfile::TempDir,
}

fn make_tree(files: usize, size: usize, seed: u64) -> LocalTree {
    let dir = ok(tempfile::tempdir());
    let src = dir.path().join("src");
    ok(std::fs::create_dir_all(src.join("sub")));
    for i in 0..files {
        let data = splitmix::data(seed + i as u64, size + i * 37);
        let p = if i % 3 == 0 {
            src.join("sub").join(format!("g{i:03}"))
        } else {
            src.join(format!("f{i:03}"))
        };
        ok(std::fs::write(p, data));
    }
    let store = open_store(&dir.path().join("packstore"));
    let opts = ingest::Opts {
        jobs: 2,
        ..ingest::Opts::default()
    };
    let (_, root) = ingest::dir(&store, &src, opts);
    let root = ok(root);
    let keys = ok(fstree::reachable_keys(root, |k| store.get(k)));
    LocalTree {
        store,
        root,
        keys,
        _dir: dir,
    }
}

fn fresh_store() -> (Arc<packstore::Store>, tempfile::TempDir) {
    let dir = ok(tempfile::tempdir());
    (open_store(&dir.path().join("packstore")), dir)
}

fn assert_same_objects(a: &packstore::Store, b: &packstore::Store, keys: &[Key]) {
    for k in keys {
        assert_eq!(ok(b.get(*k)), ok(a.get(*k)), "object {k} differs");
    }
}

fn versioned(version: &[u8]) -> Cond {
    Cond {
        versioned: true,
        expected_version: version.to_vec(),
        ..Cond::default()
    }
}

fn force() -> Cond {
    Cond {
        force: true,
        ..Cond::default()
    }
}

// ---- scenarios ----

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn push_pull_round_trip_dedup_and_cas() {
    let tc = TestCluster::start(3, 3, 2).await;
    let (cl, log) = tc.dial().await;
    let ctx = Ctx::background();
    let local = make_tree(30, 20_000, 1);

    let st = ok(cl
        .push(
            &ctx,
            Arc::clone(&local.store),
            local.root,
            "trees/a",
            "tester",
            versioned(&[]),
            None,
        )
        .await);
    assert_eq!(st.keys, len_i64(local.keys.len()));
    assert_eq!(st.uploaded, st.keys, "a fresh cluster lacks every key");
    assert!(!st.version.is_empty());
    let wire: i64 = local
        .keys
        .iter()
        .map(|k| len_i64(ok(local.store.get_record(*k)).len()))
        .sum();
    assert_eq!(st.bytes, wire, "bytes count the records sent");
    for k in &local.keys {
        assert!(tc.holders(&k.0) >= 2, "{k} on {} nodes", tc.holders(&k.0));
    }
    let negotiated = log.find("negotiated");
    assert_eq!(negotiated.len(), 1);
    assert_eq!(attr(&negotiated[0], "upload"), Some(&Value::Int64(st.keys)));
    assert_eq!(log.find("reference written").len(), 1);

    // A second push of the same tree uploads nothing.
    let st2 = ok(cl
        .push(
            &ctx,
            Arc::clone(&local.store),
            local.root,
            "trees/a",
            "tester",
            versioned(&st.version),
            None,
        )
        .await);
    assert_eq!((st2.uploaded, st2.bytes), (0, 0));
    assert_ne!(st2.version, st.version);

    // A CAS against the old version fails with the typed error.
    match cl
        .push(
            &ctx,
            Arc::clone(&local.store),
            local.root,
            "trees/a",
            "tester",
            versioned(&st.version),
            None,
        )
        .await
    {
        Err(e) => {
            let cm = e.cas_mismatch();
            assert!(
                cm.is_some_and(|c| c.has_current && c.current == local.root.0),
                "{e}"
            );
        }
        Ok(s) => panic!("stale CAS accepted: {s:?}"),
    }

    // Pull into a fresh store.
    let (pulled, _d) = fresh_store();
    let ps = ok(cl.pull(&ctx, Arc::clone(&pulled), "trees/a", None).await);
    assert_eq!(ps.root, local.root);
    assert_eq!(ps.version, st2.version);
    assert_eq!((ps.keys, ps.fetched), (st.keys, st.keys));
    assert_eq!(ps.bytes, wire);
    assert_same_objects(&local.store, &pulled, &local.keys);
    cl.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn push_progress_reports() {
    // TestClusterPushProgress.
    let tc = TestCluster::start(3, 3, 2).await;
    let (cl, _log) = tc.dial().await;
    let ctx = Ctx::background();
    let local = make_tree(30, 20_000, 2);
    let reports: Arc<Mutex<Vec<ProgressReport>>> = Arc::default();
    let sink = Arc::clone(&reports);
    let prog: Progress = Arc::new(move |r: &ProgressReport| lock(&sink).push(r.clone()));
    let st = ok(cl
        .push(
            &ctx,
            Arc::clone(&local.store),
            local.root,
            "trees/p",
            "tester",
            force(),
            Some(prog),
        )
        .await);
    let reports = lock(&reports).clone();
    assert!(
        reports.len() >= 10,
        "only {} reports for {} objects",
        reports.len(),
        st.keys
    );
    let mut last_bytes = 0;
    for (i, r) in reports.iter().enumerate() {
        assert!(
            r.bytes >= last_bytes,
            "report {i}: bytes went from {last_bytes} to {}",
            r.bytes
        );
        last_bytes = r.bytes;
        assert_eq!(r.total_objects, st.keys, "report {i}");
        let mut ids: Vec<NodeId> = r.nodes.iter().map(|n| n.id).collect();
        ids.sort();
        assert_eq!(
            ids,
            r.nodes.iter().map(|n| n.id).collect::<Vec<_>>(),
            "nodes ordered by id"
        );
    }
    let Some(last) = reports.last() else {
        panic!("no report");
    };
    assert_eq!(last.objects, last.total_objects);
    assert_eq!(last.bytes, last.total_bytes);
    assert_eq!(last.total_bytes, st.bytes);
    let wire: i64 = local
        .keys
        .iter()
        .map(|k| len_i64(ok(local.store.get_record(*k)).len()))
        .sum();
    assert_eq!(st.bytes, wire);
    assert_eq!(last.nodes.len(), 3, "the push spread over every owner");
    let mut sum = 0;
    for n in &last.nodes {
        assert_eq!((n.in_flight, n.awaiting), (0, 0), "{}", n.id.short());
        assert!(n.bytes > 0, "node {} received nothing", n.id.short());
        assert!(n.direct);
        sum += n.bytes;
    }
    assert_eq!(sum, last.bytes);

    // A push that needs no upload reports the totals and nothing else.
    let reports: Arc<Mutex<Vec<ProgressReport>>> = Arc::default();
    let sink = Arc::clone(&reports);
    let prog: Progress = Arc::new(move |r: &ProgressReport| lock(&sink).push(r.clone()));
    ok(cl
        .push(
            &ctx,
            Arc::clone(&local.store),
            local.root,
            "trees/p",
            "tester",
            force(),
            Some(prog),
        )
        .await);
    let reports = lock(&reports).clone();
    assert_eq!(reports.len(), 1);
    assert_eq!(
        (
            reports[0].objects,
            reports[0].total_objects,
            reports[0].total_bytes
        ),
        (st.keys, st.keys, 0)
    );
    cl.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn push_pipelines_batches_per_primary() {
    // TestClusterPushPipelines: with small batches several are in flight per node.
    let tc = TestCluster::start(3, 3, 2).await;
    for i in 0..3 {
        lock(&tc.node(i).script).put_delay = Some(Duration::from_millis(50));
    }
    let (cl, _log) = tc.dial_with(|cfg| cfg.batch_bytes = 64 << 10).await;
    let ctx = Ctx::background();
    let local = make_tree(30, 20_000, 13);
    let max_in_flight: Arc<Mutex<i64>> = Arc::default();
    let sink = Arc::clone(&max_in_flight);
    let prog: Progress = Arc::new(move |r: &ProgressReport| {
        let mut m = lock(&sink);
        for n in &r.nodes {
            *m = (*m).max(n.in_flight);
        }
    });
    ok(cl
        .push(
            &ctx,
            Arc::clone(&local.store),
            local.root,
            "trees/pipe",
            "tester",
            force(),
            Some(prog),
        )
        .await);
    let max = *lock(&max_in_flight);
    assert!(
        max >= 2,
        "at most {max} batch in flight per node: batches are not pipelined"
    );
    assert!(max <= 4, "{max} batches in flight, Conns is 4");
    cl.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn push_uploads_only_what_the_cluster_lacks() {
    let tc = TestCluster::start(3, 3, 2).await;
    let (cl, _log) = tc.dial().await;
    let ctx = Ctx::background();
    let a = make_tree(12, 10_000, 7);
    ok(cl
        .push(
            &ctx,
            Arc::clone(&a.store),
            a.root,
            "t/a",
            "tester",
            force(),
            None,
        )
        .await);
    // The same twelve files plus three more: new blobs and new directory objects only.
    let b = make_tree(15, 10_000, 7);
    let known: HashSet<Key> = a.keys.iter().copied().collect();
    let new = b.keys.iter().filter(|k| !known.contains(k)).count();
    assert!(new > 3 && new < b.keys.len());
    let st = ok(cl
        .push(
            &ctx,
            Arc::clone(&b.store),
            b.root,
            "t/b",
            "tester",
            force(),
            None,
        )
        .await);
    assert_eq!(st.keys, len_i64(b.keys.len()));
    assert_eq!(st.uploaded, len_i64(new));
    cl.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn incomplete_reference_writes_renegotiate() {
    let tc = TestCluster::start(3, 3, 2).await;
    let (cl, log) = tc.dial().await;
    let ctx = Ctx::background();
    let local = make_tree(9, 5_000, 3);

    *lock(&tc.shared.incomplete) = 2;
    let st = ok(cl
        .push(
            &ctx,
            Arc::clone(&local.store),
            local.root,
            "t/inc",
            "tester",
            force(),
            None,
        )
        .await);
    assert!(!st.version.is_empty());
    assert_eq!(tc.requests(T_REF_PUT).len(), 3);
    let warns = log.find("reference write incomplete, renegotiating");
    assert_eq!(warns.len(), 2);
    assert_eq!(attr(&warns[1], "attempt"), Some(&Value::Int64(2)));
    assert_eq!(
        attr(&warns[0], "err"),
        Some(&Value::Any("incomplete: 1 keys short".to_owned()))
    );
    // Every renegotiation pins the whole set again.
    let pinned: usize = tc
        .requests(T_MISSING)
        .iter()
        .filter(|(_, m)| m.pin)
        .map(|(_, m)| m.keys.len())
        .sum();
    assert!(pinned >= 3 * local.keys.len(), "{pinned} pinned keys");

    // A third incomplete answer is returned.
    *lock(&tc.shared.incomplete) = 3;
    match cl
        .push(
            &ctx,
            Arc::clone(&local.store),
            local.root,
            "t/inc",
            "tester",
            force(),
            None,
        )
        .await
    {
        Err(e) => {
            assert_eq!(e.to_string(), "incomplete: 1 keys short");
            assert!(e.incomplete().is_some());
        }
        Ok(s) => panic!("push succeeded: {s:?}"),
    }
    assert_eq!(tc.requests(T_REF_PUT).len(), 6);
    cl.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn busy_batches_are_retried_after_retry_after() {
    let tc = TestCluster::start(3, 3, 2).await;
    let (cl, log) = tc.dial().await;
    let ctx = Ctx::background();
    let local = make_tree(9, 5_000, 4);
    for i in 0..3 {
        lock(&tc.node(i).script).put_errors.push_back((
            "busy".to_owned(),
            "slow down".to_owned(),
            3,
        ));
    }
    ok(cl
        .push(
            &ctx,
            Arc::clone(&local.store),
            local.root,
            "t/busy",
            "tester",
            force(),
            None,
        )
        .await);
    let retries = log.find("upload retry");
    assert!(!retries.is_empty());
    for r in &retries {
        assert_eq!(attr(r, "reason"), Some(&Value::String("busy".to_owned())));
        assert_eq!(attr(r, "wait"), Some(&Value::Duration(3_000_000)));
        assert_eq!(attr(r, "attempt"), Some(&Value::Int64(1)));
    }
    assert!(log.find("upload failed").is_empty());
    cl.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn busy_forever_fails_with_the_last_upload_error() {
    let tc = TestCluster::start(1, 1, 1).await;
    let (cl, log) = tc.dial().await;
    let ctx = Ctx::background();
    let local = make_tree(4, 2_000, 5);
    for _ in 0..64 {
        lock(&tc.node(0).script).put_errors.push_back((
            "busy".to_owned(),
            "slow down".to_owned(),
            1,
        ));
    }
    match cl
        .push(
            &ctx,
            Arc::clone(&local.store),
            local.root,
            "t/busy",
            "tester",
            force(),
            None,
        )
        .await
    {
        Err(e) => {
            let n = local.keys.len();
            assert_eq!(
                e.to_string(),
                format!(
                    "push: {n} keys could not be placed; owners not confirming: [01000000 ({n} keys)]; \
                     last upload error: upload to 01000000: remote: busy: slow down"
                )
            );
        }
        Ok(s) => panic!("push succeeded: {s:?}"),
    }
    // Round 1 renegotiates, round 2 sends the short keys to their owners directly, round 3 gives up:
    // four put streams per batch each time, every busy answer logged (the fourth too, as in Go).
    assert_eq!(tc.requests(T_PUT).len(), 16);
    assert_eq!(log.find("upload retry").len(), 16);
    assert!(log.find("upload failed").is_empty());
    assert_eq!(
        log.find("sending short objects to their owners directly")
            .len(),
        1
    );
    assert!(tc.requests(T_REF_PUT).is_empty());
    cl.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stale_view_retry_goes_to_the_same_primary_with_the_new_epoch() {
    let tc = TestCluster::start(1, 1, 1).await;
    let (cl, log) = tc.dial().await;
    let ctx = Ctx::background();
    let local = make_tree(4, 2_000, 6);
    tc.bump_epoch(9);
    ok(cl
        .push(
            &ctx,
            Arc::clone(&local.store),
            local.root,
            "t/stale",
            "tester",
            force(),
            None,
        )
        .await);
    let epochs: Vec<u64> = tc.requests(T_PUT).iter().map(|(_, m)| m.epoch).collect();
    assert_eq!(epochs, vec![7, 9]);
    let retries = log.find("upload retry");
    assert_eq!(retries.len(), 1);
    assert_eq!(
        attr(&retries[0], "reason"),
        Some(&Value::String("stale view".to_owned()))
    );
    assert_eq!(cl.view().map(|v| v.epoch), Some(9));
    cl.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rejected_records_fail_the_push() {
    let tc = TestCluster::start(1, 1, 1).await;
    let (cl, _log) = tc.dial().await;
    let ctx = Ctx::background();
    let local = make_tree(3, 1_000, 8);
    lock(&tc.node(0).script).reject = Some("not-owner".to_owned());
    match cl
        .push(
            &ctx,
            Arc::clone(&local.store),
            local.root,
            "t/rej",
            "tester",
            force(),
            None,
        )
        .await
    {
        Err(e) => assert_eq!(
            e.to_string(),
            format!(
                "record {} rejected: not-owner",
                &local.root.to_string()[..16]
            )
        ),
        Ok(s) => panic!("push succeeded: {s:?}"),
    }
    cl.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn corrupt_copies_are_skipped_and_the_next_owner_asked() {
    let tc = TestCluster::start(3, 3, 2).await;
    let (cl, _log) = tc.dial().await;
    let ctx = Ctx::background();
    let local = make_tree(15, 8_000, 9);
    ok(cl
        .push(
            &ctx,
            Arc::clone(&local.store),
            local.root,
            "t/c",
            "tester",
            force(),
            None,
        )
        .await);
    let all: HashSet<[u8; 32]> = local.keys.iter().map(|k| k.0).collect();
    for i in 0..2 {
        lock(&tc.node(i).script).corrupt = all.clone();
    }
    let (pulled, _d) = fresh_store();
    let ps = ok(cl.pull(&ctx, Arc::clone(&pulled), "t/c", None).await);
    assert_eq!(ps.fetched, len_i64(local.keys.len()));
    assert_same_objects(&local.store, &pulled, &local.keys);
    cl.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_copy_corrupt_everywhere_is_not_found_after_one_view_refresh() {
    let tc = TestCluster::start(3, 3, 2).await;
    let (cl, _log) = tc.dial().await;
    let ctx = Ctx::background();
    let local = make_tree(6, 4_000, 10);
    ok(cl
        .push(
            &ctx,
            Arc::clone(&local.store),
            local.root,
            "t/bad",
            "tester",
            force(),
            None,
        )
        .await);
    let bad = local.keys.iter().copied().find(is_leaf).map(|k| k.0);
    let Some(bad) = bad else {
        panic!("no blob in the tree");
    };
    for i in 0..3 {
        lock(&tc.node(i).script).corrupt.insert(bad);
    }
    let views = tc.requests(T_VIEW).len();
    let (pulled, _d) = fresh_store();
    match cl.pull(&ctx, Arc::clone(&pulled), "t/bad", None).await {
        Err(e) => assert_eq!(
            e.to_string(),
            format!(
                "pull: object {} not found in the cluster",
                hex::encode(&bad[..8])
            )
        ),
        Ok(s) => panic!("pull succeeded: {s:?}"),
    }
    assert_eq!(
        tc.requests(T_VIEW).len(),
        views + 1,
        "one view refresh per fetcher"
    );
    let asked = tc
        .requests(T_GET)
        .iter()
        .filter(|(_, m)| m.keys.iter().any(|k| k.as_slice() == bad))
        .count();
    assert_eq!(
        asked, 6,
        "every owner is asked before and after the refresh"
    );
    cl.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_interrupted_pull_fetches_only_what_is_missing() {
    let tc = TestCluster::start(3, 3, 2).await;
    let (cl, _log) = tc.dial().await;
    let ctx = Ctx::background();
    let local = make_tree(30, 6_000, 11);
    ok(cl
        .push(
            &ctx,
            Arc::clone(&local.store),
            local.root,
            "t/r",
            "tester",
            force(),
            None,
        )
        .await);

    // Everything but every other blob, as a pull cut short leaves the store.
    let (partial, _d) = fresh_store();
    let mut missing = HashSet::new();
    let mut objs = Vec::new();
    for (i, k) in local.keys.iter().enumerate() {
        if is_leaf(k) && i % 2 == 0 {
            missing.insert(k.0);
            continue;
        }
        objs.push(Ok::<_, std::convert::Infallible>(packstore::Object {
            key: *k,
            data: Vec::new(),
            record: Some(ok(local.store.get_record(*k))),
        }));
    }
    assert!(!missing.is_empty());
    ok(partial
        .write_parallel(objs, packstore::WriteOpts::default())
        .1);

    assert!(tc.requests(T_GET).is_empty(), "a push sends no get");
    let mut st = PullStats::default();
    ok(cl
        .pull_tree(&ctx, Arc::clone(&partial), local.root, &mut st, None)
        .await);
    assert_eq!(st.keys, len_i64(missing.len()));
    assert_eq!(st.fetched, len_i64(missing.len()));
    let asked: HashSet<[u8; 32]> = tc
        .requests(T_GET)
        .iter()
        .flat_map(|(_, m)| {
            m.keys
                .iter()
                .filter_map(|k| <[u8; 32]>::try_from(k.as_slice()).ok())
        })
        .collect();
    assert_eq!(asked, missing, "only the missing blobs are asked for");
    assert_same_objects(&local.store, &partial, &local.keys);

    // A complete store fetches nothing at all.
    let gets_before = tc.requests(T_GET).len();
    let mut st = PullStats::default();
    ok(cl
        .pull_tree(&ctx, Arc::clone(&partial), local.root, &mut st, None)
        .await);
    assert_eq!((st.keys, st.fetched), (0, 0));
    assert_eq!(tc.requests(T_GET).len(), gets_before);
    cl.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn push_and_pull_with_an_owner_down() {
    let tc = TestCluster::start(3, 3, 2).await;
    let (cl, log) = tc.dial().await;
    let ctx = Ctx::background();
    let local = make_tree(20, 5_000, 12);
    tc.set_down(2, true);
    let down = tc.node(2).id;
    let primary_was_down = local
        .keys
        .iter()
        .any(|k| cl.owners(&k.0).first() == Some(&down));
    ok(cl
        .push(
            &ctx,
            Arc::clone(&local.store),
            local.root,
            "t/down",
            "tester",
            force(),
            None,
        )
        .await);
    for k in &local.keys {
        assert!(tc.holders(&k.0) >= 2);
        assert!(!tc.node(2).has(&k.0));
    }
    if primary_was_down {
        assert!(
            !log.find("negotiation failed at a primary, asking the next owner")
                .is_empty()
        );
    }
    let (pulled, _d) = fresh_store();
    let ps = ok(cl.pull(&ctx, Arc::clone(&pulled), "t/down", None).await);
    assert_eq!(ps.fetched, len_i64(local.keys.len()));
    assert_same_objects(&local.store, &pulled, &local.keys);
    cl.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_malformed_lacking_list_counts_every_key_as_held() {
    // v0.1.9: askPrimaries ignores the Keys32 error of the reply.
    let tc = TestCluster::start(1, 1, 1).await;
    let (cl, _log) = tc.dial().await;
    let keys: Vec<[u8; 32]> = (0..3u64)
        .map(|i| Key::new(Type::Blob, 32, &splitmix::data(2000 + i, 32)).0)
        .collect();
    let mr = ok(cl.missing(&Ctx::background(), &keys, true).await);
    assert_eq!(mr.lacking.values().map(Vec::len).sum::<usize>(), 3);

    lock(&tc.node(0).script).missing_bad_lacking = true;
    let mr = ok(cl.missing(&Ctx::background(), &keys, true).await);
    assert!(mr.lacking.is_empty());
    assert!(mr.failed.is_empty());
    for k in &keys {
        assert_eq!(
            mr.holders.get(k),
            Some(&vec![tc.node(0).id]),
            "held everywhere"
        );
    }
    cl.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unreadable_record_is_skipped_during_upload() {
    // v0.1.9: putOnce skips a key whose record cannot be read; the key stays short.
    let tc = TestCluster::start(1, 1, 1).await;
    let (cl, _log) = tc.dial().await;
    let local = make_tree(6, 3_000, 14);
    let gone = local.keys[local.keys.len() - 1];
    let store = Arc::clone(&local.store);
    let src: RecordSource = Arc::new(move |k: &[u8; 32]| {
        if *k == gone.0 {
            return Err(dstore_client::Error::Other("gone".to_owned()));
        }
        store
            .get_record(Key(*k))
            .map_err(dstore_client::Error::Packstore)
    });
    let size: RecordSizer = Arc::new(|_: &[u8; 32]| 1000);
    let sent: Arc<Mutex<Vec<usize>>> = Arc::default();
    let sink = Arc::clone(&sent);
    let obs = PutObserver {
        sent: Some(Arc::new(move |_: NodeId, n: usize| lock(&sink).push(n))),
        ..PutObserver::default()
    };
    let mut by_primary = HashMap::new();
    by_primary.insert(
        tc.node(0).id,
        local.keys.iter().map(|k| k.0).collect::<Vec<_>>(),
    );
    let pr = cl.put(&Ctx::background(), by_primary, src, size, obs).await;
    assert!(pr.errors.is_empty());
    assert!(pr.rejected.is_empty());
    assert_eq!(pr.holders.len(), local.keys.len() - 1);
    assert!(!pr.holders.contains_key(&gone.0));
    assert!(!tc.node(0).has(&gone.0));
    assert_eq!(lock(&sent).len(), local.keys.len() - 1);
    cl.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn missing_fails_keys_on_a_remote_error_and_retries_on_a_transport_error() {
    let tc = TestCluster::start(3, 3, 2).await;
    let (cl, _log) = tc.dial().await;
    let ctx = Ctx::background();
    let keys: Vec<[u8; 32]> = (0..40u64)
        .map(|i| Key::new(Type::Blob, 32, &splitmix::data(1000 + i, 32)).0)
        .collect();

    // A node down: its keys are asked at their next owner; nothing fails.
    tc.set_down(1, true);
    let mr = ok(cl.missing(&ctx, &keys, false).await);
    assert!(mr.failed.is_empty());
    assert_eq!(mr.lacking.values().map(Vec::len).sum::<usize>(), keys.len());
    assert!(!mr.lacking.contains_key(&tc.node(1).id));
    tc.set_down(1, false);

    // A node that answers with an error fails its keys, which are not asked elsewhere.
    lock(&tc.node(2).script).missing_error = Some(("internal".to_owned(), "boom".to_owned()));
    let before = tc.requests(T_MISSING).len();
    let mr = ok(cl.missing(&ctx, &keys, true).await);
    let failed_at_2: Vec<&[u8; 32]> = keys.iter().filter(|k| mr.failed.contains_key(*k)).collect();
    for k in &failed_at_2 {
        assert_eq!(mr.failed[*k].to_string(), "remote: internal: boom");
        assert!(!mr.holders.contains_key(*k));
    }
    // A remote error neither penalises nor clears a node, so the primaries are those of the call.
    let at_2: Vec<&[u8; 32]> = keys
        .iter()
        .filter(|k| cl.primary(k) == Some(tc.node(2).id))
        .collect();
    assert!(!at_2.is_empty(), "no key has node 3 as its primary");
    assert_eq!(
        failed_at_2, at_2,
        "exactly the keys of the node that answered with an error fail"
    );
    let asked = tc.requests(T_MISSING).len() - before;
    let primaries: HashSet<NodeId> = keys.iter().filter_map(|k| cl.primary(k)).collect();
    assert_eq!(asked, primaries.len(), "one request per primary, no retry");
    cl.close();
}

// ---- node/cluster_test.go over dstore_testkit::fake ----
//
// The Go cluster tests again, over the testkit's `FakeCluster` (the port of the node's request handling:
// negotiated holders, forwards, stamped replies, the ref-put completeness walk) instead of the scripted
// nodes above. The client dials as `h.clientWith(t, 100, tweak)` does, with a ticket naming node 1 only.

/// Go `cluster3(t)` (3 nodes, R = 3, min_replicas = 2) and `h.clientWith(t, 100, tweak)`.
async fn cluster3_with(
    tweak: impl FnOnce(&mut Config),
) -> (Arc<Network>, Arc<FakeCluster>, Cluster) {
    let net = Network::new();
    let fc = FakeCluster::start(&net, FakeClusterConfig::default()).await;
    let first = fc.ticket().members.unwrap_or_default().into_iter().next();
    let ep: Arc<dyn Endpoint> = net.bind(nid(100), &[ALPN_CLIENT]);
    let mut cfg = Config {
        endpoint: Some(ep),
        ticket: Ticket {
            cluster_id: None,
            incarnation: 0,
            members: first.map(|m| vec![m]),
        },
        logger: Some(Logger::new(Arc::new(Capture::default()))),
        request_timeout: Duration::from_secs(20),
        ..Config::default()
    };
    tweak(&mut cfg);
    match Cluster::dial(&Ctx::background(), cfg).await {
        Ok(c) => (net, fc, c),
        Err(e) => panic!("dial: {e}"),
    }
}

/// How many nodes of the fake cluster store each key.
fn fake_holders(fc: &FakeCluster, keys: &[Key]) -> Vec<(Key, usize)> {
    let stores: Vec<BTreeMap<[u8; 32], Vec<u8>>> =
        fc.ids().into_iter().map(|id| fc.stored(id)).collect();
    keys.iter()
        .map(|k| (*k, stores.iter().filter(|s| s.contains_key(&k.0)).count()))
        .collect()
}

async fn push_named(
    cl: &Cluster,
    local: &LocalTree,
    name: &str,
    cond: Cond,
    prog: Option<Progress>,
) -> Result<PushStats, dstore_client::Error> {
    cl.push(
        &Ctx::background(),
        Arc::clone(&local.store),
        local.root,
        name,
        "tester",
        cond,
        prog,
    )
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cluster_push_pull() {
    // TestClusterPushPull up to the pull (its RefList/RefGet/RefDelete steps are client-a's).
    let (_net, fc, cl) = cluster3_with(|_| {}).await;
    let local = make_tree(30, 20_000, 31);
    let st = ok(push_named(&cl, &local, "trees/a", versioned(&[]), None).await);
    assert!(
        st.uploaded > 0 && !st.version.is_empty(),
        "push stats {st:?}"
    );
    // Every key placed on min_replicas owners, checked with the nodes' stores.
    let min_r = cl.view().map_or(0, |v| usize::from(v.min_replicas));
    assert_eq!(min_r, 2);
    for (k, n) in fake_holders(&fc, &local.keys) {
        assert!(n >= min_r, "key {} on {n} nodes", &k.to_string()[..16]);
    }
    // A second push of the same tree uploads nothing.
    let st2 = ok(push_named(&cl, &local, "trees/a", versioned(&st.version), None).await);
    assert_eq!(st2.uploaded, 0, "second push uploaded {}", st2.uploaded);
    // A CAS with the wrong version fails.
    match push_named(&cl, &local, "trees/a", versioned(&st.version), None).await {
        Err(e) => assert!(e.cas_mismatch().is_some(), "want a cas mismatch, got {e}"),
        Ok(s) => panic!("expected cas mismatch, got {s:?}"),
    }
    // Pull into a fresh store and compare.
    let (pulled, _d) = fresh_store();
    let ps = ok(cl
        .pull(&Ctx::background(), Arc::clone(&pulled), "trees/a", None)
        .await);
    assert_eq!(ps.root, local.root);
    assert_same_objects(&local.store, &pulled, &local.keys);
    cl.close();
    fc.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cluster_node_down_during_write() {
    // TestClusterNodeDownDuringWrite up to the push: with node 3 down, writes proceed at min_replicas = 2.
    // The healing that follows is node behaviour.
    let (net, fc, cl) = cluster3_with(|_| {}).await;
    net.set_down(nid(3), true);
    let local = make_tree(12, 20_000, 32);
    if let Err(e) = push_named(&cl, &local, "t", force(), None).await {
        panic!("push with a node down: {e}");
    }
    let down = fc.stored(nid(3));
    for (k, n) in fake_holders(&fc, &local.keys) {
        assert!(n >= 2, "key {} on {n} nodes", &k.to_string()[..16]);
        assert!(!down.contains_key(&k.0), "node 3 was down but stored {k}");
    }
    net.set_down(nid(3), false);
    cl.close();
    fc.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cluster_push_progress() {
    // TestClusterPushProgress.
    let (_net, fc, cl) = cluster3_with(|_| {}).await;
    let local = make_tree(30, 20_000, 33);
    let reports: Arc<Mutex<Vec<ProgressReport>>> = Arc::default();
    let sink = Arc::clone(&reports);
    let prog: Progress = Arc::new(move |r: &ProgressReport| lock(&sink).push(r.clone()));
    let st = ok(push_named(&cl, &local, "trees/p", force(), Some(prog)).await);
    let reports = lock(&reports).clone();
    assert!(
        reports.len() >= 10,
        "only {} progress reports for {} objects",
        reports.len(),
        st.keys
    );
    let mut last_bytes = 0;
    for (i, r) in reports.iter().enumerate() {
        assert!(
            r.bytes >= last_bytes,
            "report {i}: bytes went from {last_bytes} to {}",
            r.bytes
        );
        last_bytes = r.bytes;
        assert_eq!(r.total_objects, st.keys, "report {i}: total objects");
    }
    let Some(last) = reports.last() else {
        panic!("no report");
    };
    assert!(
        last.objects == last.total_objects
            && last.bytes == last.total_bytes
            && last.total_bytes == st.bytes
            && st.bytes != 0,
        "final report {last:?}, stats {st:?}"
    );
    // Bytes count what went over the wire: the records, not the logical lengths in the keys.
    let wire: i64 = local
        .keys
        .iter()
        .map(|k| len_i64(ok(local.store.get_record(*k)).len()))
        .sum();
    assert_eq!(st.bytes, wire, "push stats count the records");
    let mut sum = 0;
    for n in &last.nodes {
        sum += n.bytes;
        assert_eq!(
            (n.in_flight, n.awaiting),
            (0, 0),
            "node {} still has batches in flight",
            n.id.short()
        );
    }
    assert!(
        !last.nodes.is_empty() && sum == last.bytes,
        "node bytes {sum} over {} nodes, want {}",
        last.nodes.len(),
        last.bytes
    );
    // The push spread over every owner.
    assert_eq!(
        last.nodes.len(),
        fc.ids().len(),
        "push talked to every node"
    );
    for n in &last.nodes {
        assert!(n.bytes > 0, "node {} received nothing", n.id.short());
    }
    cl.close();
    fc.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cluster_push_pipelines() {
    // TestClusterPushPipelines: several batches in flight per primary.
    let (_net, fc, cl) = cluster3_with(|cfg| cfg.batch_bytes = 64 << 10).await;
    let local = make_tree(30, 20_000, 34);
    let max_in_flight: Arc<Mutex<i64>> = Arc::default();
    let sink = Arc::clone(&max_in_flight);
    let prog: Progress = Arc::new(move |r: &ProgressReport| {
        let mut m = lock(&sink);
        for n in &r.nodes {
            *m = (*m).max(n.in_flight);
        }
    });
    if let Err(e) = push_named(&cl, &local, "trees/pipe", force(), Some(prog)).await {
        panic!("push: {e}");
    }
    let max = *lock(&max_in_flight);
    assert!(
        max >= 2,
        "at most {max} batch in flight per node: batches are not pipelined"
    );
    cl.close();
    fc.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cluster_pull_with_node_down() {
    // TestClusterPullWithNodeDown: every key is served by its next owner in the read order.
    let (net, fc, cl) = cluster3_with(|_| {}).await;
    let local = make_tree(30, 20_000, 35);
    if let Err(e) = push_named(&cl, &local, "trees/down", force(), None).await {
        panic!("push: {e}");
    }
    net.set_down(nid(2), true);
    let (pulled, _d) = fresh_store();
    let ps = ok(cl
        .pull(&Ctx::background(), Arc::clone(&pulled), "trees/down", None)
        .await);
    assert_eq!(ps.root, local.root);
    assert_eq!(ps.fetched, len_i64(local.keys.len()));
    assert_same_objects(&local.store, &pulled, &local.keys);
    net.set_down(nid(2), false);
    cl.close();
    fc.close().await;
}

// ---- the client-transfer §5.2 item 9 scenarios as request assertions ----
//
// `client/transcripts/` is not generated yet (VECTORS.md), so these check the requests the scripted nodes
// read instead of replaying recorded conversations.

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn missing_asks_8193_keys_in_chunks_of_8192() {
    // missing_8193: R = 1, one node, 8193 keys with pin → two `missing` frames of 8192 and 1 keys.
    let tc = TestCluster::start(1, 1, 1).await;
    let (cl, _log) = tc.dial().await;
    let keys: Vec<[u8; 32]> = (0..8193u64)
        .map(|i| Key::new(Type::Blob, 8, &i.to_le_bytes()).0)
        .collect();
    let mr = ok(cl.missing(&Ctx::background(), &keys, true).await);
    assert!(mr.failed.is_empty());
    let mut lacking = mr.lacking.get(&tc.node(0).id).cloned().unwrap_or_default();
    lacking.sort_unstable();
    let mut want = keys.clone();
    want.sort_unstable();
    assert_eq!(lacking, want, "every key lacking at the one node");
    let reqs = tc.requests(T_MISSING);
    assert!(reqs.iter().all(|(_, m)| m.pin), "pin on every chunk");
    let mut sizes: Vec<usize> = reqs.iter().map(|(_, m)| m.keys.len()).collect();
    sizes.sort_unstable();
    assert_eq!(sizes, vec![1, 8192]);
    // Chunks follow the input order.
    let asked: Vec<Vec<u8>> = {
        let mut r = reqs.clone();
        r.sort_by_key(|(_, m)| std::cmp::Reverse(m.keys.len()));
        r.into_iter().flat_map(|(_, m)| m.keys).collect()
    };
    let input: Vec<Vec<u8>> = keys.iter().map(|k| k.to_vec()).collect();
    assert_eq!(asked, input);
    cl.close();
}

/// Records of `n` splitmix payloads of `len` bytes, keyed, with a source and a sizer over them.
fn records(n: u64, len: usize, seed: u64) -> (Vec<[u8; 32]>, RecordSource, RecordSizer) {
    let mut keys = Vec::new();
    let mut recs = HashMap::new();
    for i in 0..n {
        let data = splitmix::data(seed + i, len);
        let k = Key::new(Type::Blob, len as u64, &data);
        keys.push(k.0);
        recs.insert(k.0, ok(amberpack::encode_record(k, &data)));
    }
    let recs = Arc::new(recs);
    let (a, b) = (Arc::clone(&recs), recs);
    let src: RecordSource = Arc::new(move |k: &[u8; 32]| {
        a.get(k)
            .cloned()
            .ok_or_else(|| dstore_client::Error::Other("no record".to_owned()))
    });
    let size: RecordSizer = Arc::new(move |k: &[u8; 32]| b.get(k).map_or(0, Vec::len));
    (keys, src, size)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn put_splits_batches_by_bytes_with_conns_streams_at_most() {
    // put_split: BatchBytes = 64 KiB, 30 records of 20 KiB → batches of 3 (46 + 20480 bytes each), so 10
    // put streams, at most Conns = 4 of them in flight.
    let tc = TestCluster::start(1, 1, 1).await;
    lock(&tc.node(0).script).put_delay = Some(Duration::from_millis(100));
    let (cl, _log) = tc.dial_with(|cfg| cfg.batch_bytes = 64 << 10).await;
    let (keys, src, size) = records(30, 20 << 10, 0x5350_4c49);
    let flight: Arc<Mutex<(i64, i64)>> = Arc::default();
    let (s1, s2) = (Arc::clone(&flight), Arc::clone(&flight));
    let obs = PutObserver {
        start: Some(Arc::new(move |_: NodeId| {
            let mut g = lock(&s1);
            g.0 += 1;
            g.1 = g.1.max(g.0);
        })),
        done: Some(Arc::new(move |_: NodeId, _: bool| lock(&s2).0 -= 1)),
        ..PutObserver::default()
    };
    let by_primary = HashMap::from([(tc.node(0).id, keys.clone())]);
    let pr = cl.put(&Ctx::background(), by_primary, src, size, obs).await;
    assert!(
        pr.errors.is_empty(),
        "{:?}",
        pr.errors
            .values()
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
    );
    assert!(pr.rejected.is_empty());
    assert_eq!(pr.holders.len(), 30);
    assert_eq!(
        tc.requests(T_PUT).len(),
        10,
        "30 records of 20 KiB in 64 KiB batches"
    );
    let (now, max) = *lock(&flight);
    assert_eq!(now, 0, "every started batch is done");
    assert_eq!(max, 4, "Conns batches of one primary in flight");
    cl.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn put_busy_waits_retry_after_then_gives_up_after_four_attempts() {
    // put_busy: `busy` with retry_after 50 ms → the second stream after at least 50 ms; four busy answers →
    // the batch fails with the node's error.
    let tc = TestCluster::start(1, 1, 1).await;
    let (cl, log) = tc.dial().await;
    let (keys, src, size) = records(1, 300, 0x4255_5359);
    let p = tc.node(0).id;
    lock(&tc.node(0).script)
        .put_errors
        .push_back(("busy".to_owned(), "slow down".to_owned(), 50));
    let began = std::time::Instant::now();
    let pr = cl
        .put(
            &Ctx::background(),
            HashMap::from([(p, keys.clone())]),
            Arc::clone(&src),
            Arc::clone(&size),
            PutObserver::default(),
        )
        .await;
    let took = began.elapsed();
    assert!(pr.errors.is_empty());
    assert!(pr.holders.contains_key(&keys[0]));
    assert_eq!(tc.requests(T_PUT).len(), 2);
    assert!(took >= Duration::from_millis(50), "retried after {took:?}");
    let retries = log.find("upload retry");
    assert_eq!(retries.len(), 1);
    assert_eq!(
        attr(&retries[0], "wait"),
        Some(&Value::Duration(50_000_000))
    );

    for _ in 0..4 {
        lock(&tc.node(0).script).put_errors.push_back((
            "busy".to_owned(),
            "slow down".to_owned(),
            1,
        ));
    }
    let pr = cl
        .put(
            &Ctx::background(),
            HashMap::from([(p, keys.clone())]),
            src,
            size,
            PutObserver::default(),
        )
        .await;
    match pr.errors.get(&p) {
        Some(e) => assert_eq!(e.to_string(), "remote: busy: slow down"),
        None => panic!("four busy answers did not fail the batch"),
    }
    assert!(pr.holders.is_empty());
    assert_eq!(tc.requests(T_PUT).len(), 6, "four attempts");
    // Every busy answer is logged, the fourth too; "upload failed" is only for other errors.
    assert_eq!(log.find("upload retry").len(), 5);
    assert!(log.find("upload failed").is_empty());
    cl.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn push_happy_request_sequence() {
    // push_happy: 1 node, R = 1, a tree of 3 files → `missing` with pin over every key, one `put`, then a
    // `ref-put` with HasExpected and an empty expected version (must not exist). A second push negotiates,
    // uploads nothing and expects the first version.
    let tc = TestCluster::start(1, 1, 1).await;
    let (cl, _log) = tc.dial().await;
    let local = make_tree(3, 1_000, 36);
    let st = ok(cl
        .push(
            &Ctx::background(),
            Arc::clone(&local.store),
            local.root,
            "t/happy",
            "tester",
            versioned(&[]),
            None,
        )
        .await);
    let reqs: Vec<Msg> = lock(&tc.node(0).requests).clone();
    let types: Vec<i64> = reqs.iter().map(|m| m.typ).collect();
    assert_eq!(types, vec![T_VIEW, T_MISSING, T_PUT, T_REF_PUT]);
    let all: Vec<Vec<u8>> = local.keys.iter().map(|k| k.0.to_vec()).collect();
    assert!(reqs[1].pin);
    assert_eq!(reqs[1].keys, all, "every reachable key, root first");
    for m in &reqs[1..] {
        assert_eq!(
            (m.cluster_id.as_slice(), m.incarnation, m.epoch),
            (&[0x11u8; 16][..], 1, 7),
            "stamped"
        );
    }
    let rp = &reqs[3];
    assert!(rp.has_expected && !rp.force);
    assert!(rp.expected_version.is_empty());
    let r = ok(Reference::decode(&rp.record));
    assert_eq!((r.name.as_str(), r.user.as_str()), ("t/happy", "tester"));
    assert_eq!(r.key, local.root.0.to_vec());

    let before = lock(&tc.node(0).requests).len();
    let st2 = ok(cl
        .push(
            &Ctx::background(),
            Arc::clone(&local.store),
            local.root,
            "t/happy",
            "tester",
            versioned(&st.version),
            None,
        )
        .await);
    assert_eq!(st2.uploaded, 0);
    let reqs: Vec<Msg> = lock(&tc.node(0).requests)[before..].to_vec();
    let types: Vec<i64> = reqs.iter().map(|m| m.typ).collect();
    assert_eq!(types, vec![T_MISSING, T_REF_PUT]);
    assert_eq!(reqs[1].expected_version, st.version);
    cl.close();
}
