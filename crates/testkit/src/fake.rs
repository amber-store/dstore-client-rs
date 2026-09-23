//! A fake dstore cluster over `dstore_transport::mem`: the client-ALPN handlers of dstore v0.1.10
//! `node/server.go`, `data.go`, `put.go`, `refs.go`, `watch.go`, `admin.go` and `status.go`
//! (verification §4.6), with fault injection and request transcripts.
//!
//! The nodes share one state: the view, a record store per node, the catalog of references and the
//! registry of watch streams. Where a Go node talks to another member over the cluster ALPN (forwarding
//! a put, negotiating holders for `missing`, the completeness walk of a ref-put, broadcasting a
//! ref-changed hint), the fake first checks that the member is reachable over the in-memory network (a
//! cached cluster-ALPN connection that is still open, else a fresh dial), so `Network::set_down` and
//! `Network::partition` affect it as they affect Go nodes, and then reads or writes that member's state
//! directly.
//!
//! Differences from a Go node that a client can observe:
//! - There is no catalog Paxos, maintenance, GC, scrub or reconcile. Admin replies are canned
//!   ([`FakeCluster`] docs), and keys a forward failed to deliver stay missing until a client re-sends
//!   them.
//! - Reference versions are 40-byte ballots (a big-endian counter shared by the cluster ‖ the
//!   coordinating node's id), the same shape as Go's `paxos.Ballot`.
//! - Where Go iterates a map (holder lists, forward failures, the watch's known list), the fake uses
//!   write-set, first-use or name order (PORTING DD-10).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use amber_store_core::amberpack::{self, REC_HEADER_SIZE, RawRecord};
use amber_store_core::fstree;
use amber_store_core::key::{Key, Type};
use amber_store_core::reference::{self, Reference};
use dstore_gocompat::ctx::Ctx;
use dstore_ticket::{Member, Ticket};
use dstore_transport::mem::{MemEndpoint, Network};
use dstore_transport::{Conn, Endpoint, RecvStream, SendStream, Stream, TransportError};
use dstore_view::{Node, NodeId, Placement, View, Voter};
use dstore_wire::{
    self as wire, AdminReply, AdminRequest, KeyFailure, KeyHolders, KeyReject, Msg, PackReader,
    PackRecords, PackSender, RefInfo, Status, WireError,
};
use tokio::io::{AsyncReadExt, AsyncWrite};
use tokio::sync::mpsc;

use crate::refglob::{self, Glob};
use crate::splitmix::SplitMix64;

/// The limit `handleRefList` and the watch reconcile pass to `Catalog.RefList` (refs.go, watch.go).
pub const DEFAULT_REF_PAGE_LIMIT: usize = 20000;
/// `node.Config.WatchReconcile`'s default (node.go:122).
pub const DEFAULT_WATCH_RECONCILE: Duration = Duration::from_secs(30);

/// `paxos.BallotSize`.
const BALLOT_SIZE: usize = 40;
/// The acceptor's scan limit clamp (paxos/acceptor.go `scan`).
const SCAN_LIMIT_MAX: usize = 100_000;
/// `watchers.subscribe`'s hint queue.
const HINT_QUEUE: usize = 1024;
/// `Node.Unreachable` forgets a failure after two minutes.
const UNREACHABLE_TTL: Duration = Duration::from_secs(120);
/// The Go test harness's `ForwardTimeout`, used for member dials.
const LINK_DIAL_TIMEOUT: Duration = Duration::from_secs(10);
/// `walkComplete` keeps at most this many missing keys; `handleRefPut` sends at most 64 of them.
const MISSING_CAP: usize = 1024;
const INCOMPLETE_SAMPLE: usize = 64;
/// The disk size the canned status reports.
const TOTAL_BYTES: i64 = 1 << 40;
/// The seed of the cluster's pseudo-random stream (tokens, the cluster id, forward retry hints).
const RNG_SEED: u64 = 0x6473_746f_7265;
/// `node.BackupName`: the reserved reference of the catalog backup object.
const BACKUP_NAME: &str = "dstore/catalog-backup";
/// `noteBackup` keeps the last 24 backup keys.
const MAX_BACKUPS: usize = 24;

/// The shape of a fake cluster. [`Default`] is the Go harness's `cluster3`: three nodes, R = 3,
/// min_replicas = 2.
#[derive(Clone, Debug)]
pub struct FakeClusterConfig {
    /// The number of nodes. Node `i` (0-based) has the Go harness's id `nid(i+1)`: bytes 0 and 31 are
    /// `i+1`.
    pub nodes: usize,
    /// 0 → 3, as `InitCluster`.
    pub replicas: u8,
    /// 0 → `view.DefaultMinReplicas(replicas)`.
    pub min_replicas: u8,
    /// Per node; missing entries weigh 100.
    pub weights: Vec<u32>,
    /// Per node; missing entries have no zone.
    pub zones: Vec<String>,
}

impl Default for FakeClusterConfig {
    fn default() -> Self {
        FakeClusterConfig {
            nodes: 3,
            replicas: 3,
            min_replicas: 2,
            weights: Vec::new(),
            zones: Vec::new(),
        }
    }
}

/// A fault injected into the nth request of a type at a node ([`FakeCluster::inject`]).
///
/// Every injection matching a request applies: all delays first, then `CloseBeforeReply` (which wins
/// over `Err`), else the first `Err`; `CorruptRecord` and `DropHints` shape the handling itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Injection {
    /// Sleep before handling the request.
    Delay(Duration),
    /// Handle the request with its reply discarded (side effects happen), then end the stream (FIN and
    /// STOP_SENDING) and close the connection. A watch request is not handled.
    CloseBeforeReply,
    /// Answer `TErr{code, text, retry_after}` instead of handling the request; `with_view` adds the
    /// encoded view and stamps the frame, as `writeStale` does.
    Err {
        code: String,
        text: String,
        with_view: bool,
        retry_after_ms: i64,
    },
    /// On a `get`: the record of this key is served tampered (a payload bit flipped, the CRC valid), so
    /// that it parses but fails verification.
    CorruptRecord([u8; 32]),
    /// On a `ref-put` or `ref-delete`: the commit's hint reaches only the coordinator's own watchers
    /// (a lost ref-changed hint). On a `ref-watch`: that stream ignores every hint and learns of changes
    /// only by its periodic reconcile.
    DropHints,
}

/// A running fake cluster.
///
/// Canned admin replies (`node/admin.go` shapes and texts, errors as `unavailable`):
/// - `token-create`: a 32-byte token and its hex;
/// - `cluster-ticket`: this node first, then the first three nodes;
/// - `node-*`, `voter-*`: the 32-byte id check; `node-repair` text; for the other `node-*` ops the
///   `is not a member` and transition-in-progress checks, then the view and
///   `transition proposed at epoch N` (no transition is proposed); `voters now N`;
/// - `replicas` (its checks, then `transition proposed`), `transition-status|abort|pause|resume`,
///   `rate-cap`; `transition-refreeze` (`no frozen transition` unless one is frozen);
/// - `gc-run|status`: the zero `GCState` and `epoch 0 idle` (`, ON HOLD` after `gc-hold`); `gc-hold`;
/// - `gc-why`: the names whose trees reach the key;
/// - `catalog-backup`: Go's backup blob and `dstore/catalog-backup` reference; `catalog-backups`;
/// - `keep`; any other op is `bad-request "unknown admin op X"`.
pub struct FakeCluster {
    endpoints: Vec<Arc<MemEndpoint>>,
    shared: Arc<Shared>,
}

/// State reachable from the serving tasks.
struct Shared {
    ids: Vec<NodeId>,
    endpoints: Vec<Arc<MemEndpoint>>,
    /// Ends every loop and stream when the cluster closes.
    ctx: Ctx,
    state: Mutex<FakeState>,
    watchers: Mutex<Vec<Arc<WatchSub>>>,
    /// Cluster-ALPN connections between members, the reachability probe.
    links: Mutex<HashMap<(NodeId, NodeId), Arc<dyn Conn>>>,
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

struct FakeState {
    view: View,
    /// Built lazily from `view`; dropped whenever the view changes.
    placement: Option<Arc<Placement>>,
    /// Aligned with `Shared::ids`.
    nodes: Vec<NodeState>,
    catalog: BTreeMap<String, RefValue>,
    ballot: u64,
    injections: Vec<Planned>,
    ref_page_limit: usize,
    watch_reconcile: Duration,
    rng: SplitMix64,
    gc_hold: bool,
    backups: Vec<[u8; 32]>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RefValue {
    record: Vec<u8>,
    version: Vec<u8>,
}

#[derive(Default)]
struct NodeState {
    writable: bool,
    store: BTreeMap<[u8; 32], Stored>,
    seq: u64,
    pins: BTreeSet<[u8; 32]>,
    requests: Vec<Msg>,
    counts: HashMap<i64, usize>,
    unreachable: BTreeMap<NodeId, tokio::time::Instant>,
    puts: u64,
    gets: u64,
    ref_puts: u64,
    bytes_in: u64,
    bytes_out: u64,
}

struct Stored {
    /// Append order, standing in for the pack location `SortByLocation` orders by.
    seq: u64,
    record: Vec<u8>,
}

struct Planned {
    node: NodeId,
    op: i64,
    nth: usize,
    inj: Injection,
}

/// What the injections matching one request ask for.
#[derive(Default)]
struct Plan {
    delays: Vec<Duration>,
    close_before_reply: bool,
    err: Option<InjectedErr>,
    corrupt: Vec<[u8; 32]>,
    drop_hints: bool,
}

struct InjectedErr {
    code: String,
    text: String,
    with_view: bool,
    retry_after_ms: i64,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The Go harness's `nid(i+1)`.
fn node_id(i: usize) -> NodeId {
    let n = i.wrapping_add(1);
    let mut id = [0u8; 32];
    id[0] = n as u8;
    id[1] = (n >> 8) as u8;
    id[31] = n as u8;
    NodeId(id)
}

/// Go `copy(id[:], b)`.
fn nid_of(b: &[u8]) -> NodeId {
    let mut id = [0u8; 32];
    let n = b.len().min(32);
    id[..n].copy_from_slice(&b[..n]);
    NodeId(id)
}

/// `view.ShortID`.
fn short_id(id: &NodeId) -> String {
    hex::encode(&id.0[..4])
}

/// `MemEndpoint.Addrs()`.
fn mem_addr(id: &NodeId) -> String {
    format!("mem:{}", short_id(id))
}

/// `view.Contains`.
fn contains(ids: &[Option<Vec<u8>>], id: &NodeId) -> bool {
    ids.iter().any(|b| b.as_deref() == Some(&id.0[..]))
}

/// `View.AllMembers`: nodes, then pending nodes, first occurrence.
fn all_members(v: &View) -> Vec<NodeId> {
    let mut out: Vec<NodeId> = Vec::new();
    let pending = v
        .pending
        .as_ref()
        .and_then(|p| p.nodes.as_deref())
        .unwrap_or(&[]);
    for n in v.nodes.as_deref().unwrap_or(&[]).iter().chain(pending) {
        let id = nid_of(n.id.as_deref().unwrap_or(&[]));
        if !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

fn stamp(v: &View, m: &mut Msg) {
    m.incarnation = v.incarnation;
    m.epoch = v.epoch;
}

/// `wire.CloseStream`.
fn close_stream(s: &mut Stream) {
    s.send.finish();
    s.recv.cancel_read(0);
}

fn is_leaf(k: &[u8; 32]) -> bool {
    matches!(
        Type::from_u8(k[0] >> 4),
        Some(Type::Blob) | Some(Type::XattrSet)
    )
}

/// The decoded payload of a stored record.
fn decode_record(rec: &[u8]) -> Option<Vec<u8>> {
    let r = amberpack::parse_record(rec).ok()?;
    let stored = rec.get(REC_HEADER_SIZE..REC_HEADER_SIZE + r.slen as usize)?;
    amberpack::decode_payload(r.flags, r.ulen, stored).ok()
}

/// `keyOfRecord`.
fn key_of_record(rec: &[u8]) -> Option<Vec<u8>> {
    Reference::decode(rec).ok().map(|r| r.key)
}

/// node `verifyRecord` (data.go): canonical key, payload decodes, hashes to the key, and a Blob,
/// XattrSet or Commit length field equals the payload length (these carry their own serialized length;
/// directory and file nodes carry their subtree's).
fn verify_record(raw: &RawRecord) -> Result<([u8; 32], Vec<u8>), String> {
    let k = raw.record.key;
    k.validate().map_err(|e| e.to_string())?;
    let stored = raw.bytes.get(REC_HEADER_SIZE..).unwrap_or(&[]);
    let payload = amberpack::decode_payload(raw.record.flags, raw.record.ulen, stored)
        .map_err(|e| e.to_string())?;
    let Some(t) = Type::from_u8(k.0[0] >> 4) else {
        return Err(format!("key: reserved object type: {}", k.0[0] >> 4));
    };
    let want = Key::new(t, k.length(), &payload);
    if want != k {
        return Err(format!("payload hashes to {want}"));
    }
    if matches!(t, Type::Blob | Type::XattrSet | Type::Commit) && k.length() != payload.len() as u64
    {
        return Err("length field mismatch".to_string());
    }
    Ok((k.0, raw.bytes.clone()))
}

/// A copy of a record whose payload has one bit flipped and whose CRC is valid, so that it parses and
/// then fails verification.
fn tamper(rec: &[u8]) -> Vec<u8> {
    let Ok(r) = amberpack::parse_record(rec) else {
        let mut b = rec.to_vec();
        if let Some(last) = b.last_mut() {
            *last ^= 1;
        }
        return b;
    };
    let mut payload = decode_record(rec).unwrap_or_default();
    match payload.last_mut() {
        Some(b) => *b ^= 1,
        None => payload.push(0),
    }
    amberpack::encode_record(r.key, &payload).unwrap_or_else(|_| rec.to_vec())
}

/// `paxos.Ballot`: ordered by counter, then proposer, which is also the order of its bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
struct Ballot {
    counter: u64,
    proposer: [u8; 32],
}

impl Ballot {
    /// `paxos.ParseBallot`: empty is the zero ballot; any length but 40 is an error.
    fn parse(b: &[u8]) -> Option<Ballot> {
        match b.len() {
            0 => Some(Ballot::default()),
            BALLOT_SIZE => {
                let mut counter = [0u8; 8];
                counter.copy_from_slice(&b[..8]);
                let mut proposer = [0u8; 32];
                proposer.copy_from_slice(&b[8..]);
                Some(Ballot {
                    counter: u64::from_be_bytes(counter),
                    proposer,
                })
            }
            _ => None,
        }
    }

    fn is_zero(&self) -> bool {
        self.counter == 0 && self.proposer == [0u8; 32]
    }

    fn bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(BALLOT_SIZE);
        out.extend_from_slice(&self.counter.to_be_bytes());
        out.extend_from_slice(&self.proposer);
        out
    }
}

/// `catalog.Cond`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Cond {
    expected_version: Option<Vec<u8>>,
    versioned: bool,
    expected_old: Option<Vec<u8>>,
    keyed: bool,
    force: bool,
}

/// `catalog.CASMismatch`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct CasMismatch {
    current: Option<RefValue>,
    version: Vec<u8>,
    has_current: bool,
}

/// An omitempty `[]byte` of a frame: empty is Go's nil.
fn nonempty(b: &[u8]) -> Option<Vec<u8>> {
    (!b.is_empty()).then(|| b.to_vec())
}

/// node `condOf` (refs.go).
fn cond_of(m: &Msg) -> Cond {
    let mut c = Cond {
        force: m.force,
        ..Cond::default()
    };
    if m.has_expected {
        let expected_version = nonempty(&m.expected_version);
        let expected_old = nonempty(&m.expected_old);
        if expected_version.is_some() || (expected_old.is_none() && m.version.is_empty()) {
            c.versioned = true;
            c.expected_version = expected_version;
        }
        if expected_old.is_some() {
            c.keyed = true;
            c.expected_old = expected_old;
            c.versioned = false;
        }
    }
    c
}

impl Cond {
    /// `Cond.check`.
    fn check(&self, cur: Option<&RefValue>, cur_key: Option<&[u8]>) -> Result<(), CasMismatch> {
        let mismatch = || CasMismatch {
            current: cur.cloned(),
            version: cur.map(|c| c.version.clone()).unwrap_or_default(),
            has_current: cur.is_some(),
        };
        if self.force {
            return Ok(());
        }
        if self.versioned {
            return match (&self.expected_version, cur) {
                (None, Some(_)) => Err(mismatch()),
                (None, None) => Ok(()),
                (Some(ev), Some(c)) if *ev == c.version => Ok(()),
                (Some(_), _) => Err(mismatch()),
            };
        }
        if self.keyed {
            return match (&self.expected_old, cur) {
                (None, Some(_)) => Err(mismatch()),
                (None, None) => Ok(()),
                (Some(eo), Some(_)) if cur_key == Some(eo.as_slice()) => Ok(()),
                (Some(_), _) => Err(mismatch()),
            };
        }
        Ok(())
    }
}

/// The length of a CBOR head for argument `n`.
fn cbor_head_len(n: usize) -> usize {
    match n {
        0..=23 => 1,
        24..=0xff => 2,
        0x100..=0xffff => 3,
        0x1_0000..=0xffff_ffff => 5,
        _ => 9,
    }
}

/// The encoded size of `catalog.RefValue{Record, Version}`, the acceptor's row value.
fn ref_value_len(rv: &RefValue) -> usize {
    let mut n = 1;
    if !rv.record.is_empty() {
        n += 1 + cbor_head_len(rv.record.len()) + rv.record.len();
    }
    n + 1 + cbor_head_len(rv.version.len()) + rv.version.len()
}

/// `stats`-free `storeBatch` for one record: a dedup hit is pinned, a fresh key appended.
fn store_record(node: &mut NodeState, k: [u8; 32], rec: &[u8]) {
    if node.store.contains_key(&k) {
        node.pins.insert(k);
        return;
    }
    insert_record(node, k, rec);
}

/// `packstore.Put` of one record: appended when absent, nothing (no pin) when present.
fn insert_record(node: &mut NodeState, k: [u8; 32], rec: &[u8]) {
    if node.store.contains_key(&k) {
        return;
    }
    node.seq += 1;
    let seq = node.seq;
    node.store.insert(
        k,
        Stored {
            seq,
            record: rec.to_vec(),
        },
    );
}

/// `localMissing`: which keys the node lacks and holds; held keys are pinned when asked.
fn local_missing(
    node: &mut NodeState,
    keys: &[[u8; 32]],
    pin: bool,
) -> (Vec<[u8; 32]>, Vec<[u8; 32]>) {
    let mut lacking = Vec::new();
    let mut present = Vec::new();
    for k in keys {
        if node.store.contains_key(k) {
            present.push(*k);
        } else {
            lacking.push(*k);
        }
    }
    if pin {
        node.pins.extend(present.iter().copied());
    }
    (lacking, present)
}

fn unreachable_ids(node: &mut NodeState) -> Vec<Vec<u8>> {
    let now = tokio::time::Instant::now();
    node.unreachable
        .retain(|_, t| now.saturating_duration_since(*t) <= UNREACHABLE_TTL);
    node.unreachable.keys().map(|id| id.0.to_vec()).collect()
}

/// `maintenance.transitionText`.
fn transition_text(v: &View) -> String {
    let Some(p) = v.pending.as_ref() else {
        if !v.ramps.is_empty() {
            return format!("idle ({} ramp(s) pending)", v.ramps.len());
        }
        return "idle".to_string();
    };
    if !p.frozen {
        return format!(
            "id {} ({}): adopting, {}/{} acked",
            p.id,
            p.reason,
            p.participants_ack.len(),
            all_members(v).len()
        );
    }
    let waiting: Vec<String> = p
        .participants
        .iter()
        .map(|id| nid_of(id.as_deref().unwrap_or(&[])))
        .filter(|id| !contains(&p.done, id))
        .map(|id| short_id(&id))
        .collect();
    format!(
        "id {} round {} ({}): {}/{} done, waiting for {}",
        p.id,
        p.round,
        p.reason,
        p.done.len(),
        p.participants.len(),
        dstore_gocompat::fmt::v_strings(&waiting)
    )
}

/// `gcState.statusText` of an idle, never-run collector.
fn gc_text(hold: bool) -> String {
    let mut s = "epoch 0 idle".to_string();
    if hold {
        s.push_str(", ON HOLD");
    }
    s
}

/// `codec.MustMarshal(catalog.GCState{Hold: hold})`: `Epoch` (0) and `Phase` (1) are not omitempty,
/// `Hold` (9) is.
fn gc_state_cbor(hold: bool) -> Vec<u8> {
    if hold {
        vec![0xa3, 0x00, 0x00, 0x01, 0x00, 0x09, 0xf5]
    } else {
        vec![0xa2, 0x00, 0x00, 0x01, 0x00]
    }
}

/// `codec.MustMarshal([]backupEntry)` (maintenance.go): an array of `{0: Name, 1: Record}`; no
/// references is a nil slice, CBOR null.
fn backup_cbor(entries: &[(&String, &RefValue)]) -> Vec<u8> {
    let mut e = dstore_codec::Enc::new();
    if entries.is_empty() {
        e.null();
        return e.into_bytes();
    }
    e.head(4, entries.len() as u64);
    for (name, rv) in entries {
        e.head(5, 2);
        e.uint(0);
        e.text(name);
        e.uint(1);
        e.bytes(&rv.record);
    }
    e.into_bytes()
}

/// `time.Now().UnixNano()`.
fn unix_nanos() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX))
}

/// The checks `proposeTransition` makes before the operation's own.
fn transition_admissible(v: &View) -> Result<(), AdminErr> {
    if v.pending.is_some() {
        return Err(AdminErr::Other(
            "a transition is already in progress".to_string(),
        ));
    }
    if v.voter_sync == dstore_view::VOTER_SYNC_PENDING {
        return Err(AdminErr::Other("a voter change is in progress".to_string()));
    }
    Ok(())
}

/// `View.Node`: under nodes, or pending nodes.
fn view_node<'a>(v: &'a View, id: &NodeId) -> Option<&'a Node> {
    let pending = v
        .pending
        .as_ref()
        .and_then(|p| p.nodes.as_deref())
        .unwrap_or(&[]);
    v.nodes
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .chain(pending)
        .find(|n| n.id.as_deref() == Some(&id.0[..]))
}

enum EpochErr {
    Stale,
    Other(&'static str),
}

/// node `checkEpoch` against the shared view (a refresh cannot find a newer one).
fn check_epoch(v: &View, m: &Msg) -> Result<(), EpochErr> {
    if !m.cluster_id.is_empty() && v.cluster_id.as_deref().unwrap_or(&[]) != m.cluster_id.as_slice()
    {
        return Err(EpochErr::Other("wrong cluster"));
    }
    match (v.incarnation, v.epoch).cmp(&(m.incarnation, m.epoch)) {
        std::cmp::Ordering::Greater => Err(EpochErr::Stale),
        std::cmp::Ordering::Less => Err(EpochErr::Other("epoch above the catalog's")),
        std::cmp::Ordering::Equal => Ok(()),
    }
}

impl FakeState {
    fn new(view: View, nodes: usize, rng_seed: u64) -> FakeState {
        FakeState {
            view,
            placement: None,
            nodes: (0..nodes)
                .map(|_| NodeState {
                    writable: true,
                    ..NodeState::default()
                })
                .collect(),
            catalog: BTreeMap::new(),
            ballot: 0,
            injections: Vec::new(),
            ref_page_limit: DEFAULT_REF_PAGE_LIMIT,
            watch_reconcile: DEFAULT_WATCH_RECONCILE,
            rng: SplitMix64(rng_seed),
            gc_hold: false,
            backups: Vec::new(),
        }
    }

    fn placement(&mut self) -> Arc<Placement> {
        if let Some(p) = &self.placement {
            return p.clone();
        }
        let p = Arc::new(Placement::new(Arc::new(self.view.clone())));
        self.placement = Some(p.clone());
        p
    }

    fn view_changed(&mut self) {
        self.placement = None;
    }

    fn next_ballot(&mut self, proposer: NodeId) -> Vec<u8> {
        self.ballot += 1;
        Ballot {
            counter: self.ballot,
            proposer: proposer.0,
        }
        .bytes()
    }

    fn random_bytes(&mut self, n: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(n + 8);
        while out.len() < n {
            out.extend_from_slice(&self.rng.next_u64().to_le_bytes());
        }
        out.truncate(n);
        out
    }

    /// `Catalog.RefPut` (catalog.go): the condition, with a retry after a lost reply reporting success.
    fn catalog_put(
        &mut self,
        proposer: NodeId,
        name: &str,
        record: &[u8],
        cond: &Cond,
    ) -> Result<Vec<u8>, CasMismatch> {
        let cur = self.catalog.get(name).cloned();
        let cur_key = cur.as_ref().and_then(|c| key_of_record(&c.record));
        if let Err(mm) = cond.check(cur.as_ref(), cur_key.as_deref()) {
            // A retry after a lost reply: our record is already there.
            if let Some(c) = &cur
                && c.record == record
            {
                return Ok(c.version.clone());
            }
            return Err(mm);
        }
        let version = self.next_ballot(proposer);
        self.catalog.insert(
            name.to_string(),
            RefValue {
                record: record.to_vec(),
                version: version.clone(),
            },
        );
        Ok(version)
    }

    /// `Catalog.RefDelete`: the version of the deletion, or `None` when nothing was deleted.
    fn catalog_delete(
        &mut self,
        proposer: NodeId,
        name: &str,
        cond: &Cond,
    ) -> Result<Option<Vec<u8>>, CasMismatch> {
        let cur = self.catalog.get(name).cloned();
        if cur.is_none()
            && (cond.force
                || (cond.versioned && cond.expected_version.is_some())
                || (cond.keyed && cond.expected_old.is_some()))
        {
            return Ok(None);
        }
        let cur_key = cur.as_ref().and_then(|c| key_of_record(&c.record));
        cond.check(cur.as_ref(), cur_key.as_deref())?;
        if cur.is_none() {
            return Ok(None);
        }
        self.catalog.remove(name);
        Ok(Some(self.next_ballot(proposer)))
    }

    /// `Catalog.RefList` over the acceptor's scan: names from `prefix` (or strictly after `after`) up to
    /// `prefix‖ff ff ff ff`, at most `limit` rows and the 4 MiB accounting; `next` is the last name
    /// returned when more rows remain.
    fn catalog_list(
        &self,
        prefix: &[u8],
        after: &[u8],
        limit: usize,
    ) -> (Vec<(String, RefValue)>, String) {
        let limit = if limit == 0 || limit > SCAN_LIMIT_MAX {
            SCAN_LIMIT_MAX
        } else {
            limit
        };
        // The scan's prefix is `ref/‖prefix`, never empty, so its bound is always `‖ff ff ff ff`.
        let mut upper = prefix.to_vec();
        upper.extend_from_slice(&[0xff; 4]);
        let mut out = Vec::new();
        let mut size = 0usize;
        let mut more = false;
        for (name, rv) in &self.catalog {
            let nb = name.as_bytes();
            let above = if after.is_empty() {
                nb >= prefix
            } else {
                nb > after
            };
            if !above {
                continue;
            }
            if nb >= upper.as_slice() {
                break;
            }
            if out.len() >= limit || size >= wire::MAX_PAGE_BYTES {
                more = true;
                break;
            }
            size += "ref/".len() + nb.len() + ref_value_len(rv) + 2 * BALLOT_SIZE;
            out.push((name.clone(), rv.clone()));
        }
        let next = match out.last() {
            Some((name, _)) if more => name.clone(),
            _ => String::new(),
        };
        (out, next)
    }

    /// `markReachable` / `markUnreachable` at node `idx` for a member it called.
    fn note_reach(&mut self, idx: usize, id: NodeId, ok: bool) {
        if let Some(n) = self.nodes.get_mut(idx) {
            if ok {
                n.unreachable.remove(&id);
            } else {
                n.unreachable.insert(id, tokio::time::Instant::now());
            }
        }
    }

    /// node `getData` from the shared stores: this node's copy, else the first owner in read order that
    /// holds it (`fetchRecord`; each owner `getFrom` asks is marked reachable or unreachable).
    fn get_data(
        &mut self,
        ids: &[NodeId],
        idx: usize,
        reach: &HashMap<NodeId, bool>,
        pl: &Placement,
        k: &[u8; 32],
    ) -> Option<Vec<u8>> {
        if let Some(s) = self.nodes.get(idx).and_then(|n| n.store.get(k)) {
            return decode_record(&s.record);
        }
        for o in pl.read_order(k) {
            if o == ids[idx] {
                continue;
            }
            let ok = reach.get(&o).copied().unwrap_or(false);
            self.note_reach(idx, o, ok);
            let Some(o_idx) = ids.iter().position(|x| *x == o).filter(|_| ok) else {
                continue;
            };
            if let Some(s) = self.nodes[o_idx].store.get(k) {
                return decode_record(&s.record);
            }
        }
        None
    }

    /// `negotiateComplete`: has-and-pin at every owner, then the keys held by fewer than
    /// `min(minR, owners)` owners of nodes (or of pending nodes).
    fn negotiate_complete(
        &mut self,
        ids: &[NodeId],
        idx: usize,
        reach: &HashMap<NodeId, bool>,
        pl: &Placement,
        keys: &[[u8; 32]],
        min_r: usize,
    ) -> Vec<[u8; 32]> {
        let me = ids[idx];
        let mut by_owner: Vec<(NodeId, Vec<[u8; 32]>)> = Vec::new();
        for k in keys {
            for o in pl.write_set(k) {
                match by_owner.iter_mut().find(|(id, _)| *id == o) {
                    Some((_, ks)) => ks.push(*k),
                    None => by_owner.push((o, vec![*k])),
                }
            }
        }
        // None: the owner could not be asked.
        let mut results: HashMap<NodeId, Option<HashSet<[u8; 32]>>> = HashMap::new();
        for (o, ks) in &by_owner {
            let asked = *o == me || reach.get(o).copied().unwrap_or(false);
            if *o != me {
                self.note_reach(idx, *o, asked);
            }
            let lacking = match ids.iter().position(|x| x == o) {
                Some(o_idx) if asked => {
                    let (lacking, _) = local_missing(&mut self.nodes[o_idx], ks, true);
                    Some(lacking.into_iter().collect())
                }
                _ => None,
            };
            results.insert(*o, lacking);
        }
        let count = |owners: &[NodeId], k: &[u8; 32]| {
            owners
                .iter()
                .filter(|o| matches!(results.get(*o), Some(Some(lack)) if !lack.contains(k)))
                .count()
        };
        let mut short = Vec::new();
        for k in keys {
            let owners = pl.owners(k);
            if count(&owners, k) < min_r.min(owners.len()) {
                short.push(*k);
                continue;
            }
            if let Some(po) = pl.pending_owners(k)
                && count(&po, k) < min_r.min(po.len())
            {
                short.push(*k);
            }
        }
        short
    }

    /// The payload of `k` from any node's store (the admin paths, which Go runs cluster-wide).
    fn any_data(&self, k: &[u8; 32]) -> Option<Vec<u8>> {
        self.nodes
            .iter()
            .find_map(|n| n.store.get(k))
            .and_then(|s| decode_record(&s.record))
    }

    /// `fstree.ReachableKeys` over the shared stores; `None` when an interior object is missing.
    fn reachable_keys(&self, root: [u8; 32]) -> Option<Vec<[u8; 32]>> {
        let mut out = vec![root];
        let mut seen: HashSet<[u8; 32]> = HashSet::from([root]);
        let mut frontier = vec![root];
        while !frontier.is_empty() {
            let mut next = Vec::new();
            for k in &frontier {
                if is_leaf(k) {
                    continue;
                }
                let data = self.any_data(k)?;
                let kids = fstree::child_keys(Key(*k), &data).ok()?;
                for c in kids {
                    if seen.insert(c.0) {
                        out.push(c.0);
                        next.push(c.0);
                    }
                }
            }
            frontier = next;
        }
        Some(out)
    }

    /// node `Why` over the catalog.
    fn why(&self, target: &[u8; 32]) -> Vec<String> {
        let mut out = Vec::new();
        for (name, rv) in &self.catalog {
            let Ok(r) = Reference::decode(&rv.record) else {
                continue;
            };
            let Ok(root) = Key::parse(&r.key) else {
                continue;
            };
            if let Some(keys) = self.reachable_keys(root.0)
                && keys.contains(target)
            {
                out.push(name.clone());
            }
        }
        out
    }
}

enum AdminErr {
    Remote(&'static str, String),
    Other(String),
}

fn node_id_of(b: &[u8]) -> Result<NodeId, AdminErr> {
    if b.len() != 32 {
        return Err(AdminErr::Other("node id must be 32 bytes".to_string()));
    }
    Ok(nid_of(b))
}

/// One ref-watch stream's subscription to hints (`watchSub`).
struct WatchSub {
    node: NodeId,
    glob: Glob,
    tx: mpsc::Sender<Hint>,
    rescan: AtomicBool,
    drop_hints: bool,
}

/// `refHint`.
#[derive(Clone, Debug)]
struct Hint {
    name: String,
    /// `None` for a deletion.
    record: Option<Vec<u8>>,
    version: Vec<u8>,
}

impl FakeCluster {
    /// Binds `cfg.nodes` endpoints speaking `amber-dstore/1` and `amber-dstore-cluster/1` on `net`, builds
    /// the view (incarnation 1, epoch 1, every node a voter) and starts serving.
    pub async fn start(net: &Arc<Network>, cfg: FakeClusterConfig) -> Arc<FakeCluster> {
        let ids: Vec<NodeId> = (0..cfg.nodes).map(node_id).collect();
        let endpoints: Vec<Arc<MemEndpoint>> = ids
            .iter()
            .map(|id| net.bind(*id, &[wire::ALPN_CLIENT, wire::ALPN_CLUSTER]))
            .collect();
        // InitCluster's defaults.
        let replicas = if cfg.replicas == 0 { 3 } else { cfg.replicas };
        let min_replicas = if cfg.min_replicas == 0 {
            default_min_replicas(replicas)
        } else {
            cfg.min_replicas
        };
        let nodes: Vec<Node> = ids
            .iter()
            .enumerate()
            .map(|(i, id)| Node {
                id: Some(id.0.to_vec()),
                weight: cfg.weights.get(i).copied().unwrap_or(100),
                addrs: vec![mem_addr(id)],
                zone: cfg.zones.get(i).cloned().unwrap_or_default(),
                writable: true,
                // InitCluster and join admit every node at incarnation 1.
                incarnation: 1,
                ..Node::default()
            })
            .collect();
        let voters: Vec<Voter> = ids
            .iter()
            .map(|id| Voter {
                id: Some(id.0.to_vec()),
                since: 1,
            })
            .collect();
        let view = View {
            cluster_id: Some(crate::splitmix::data(RNG_SEED, 16)),
            incarnation: 1,
            epoch: 1,
            version: 1,
            placement_epoch: 1,
            replicas,
            min_replicas,
            voters: Some(voters),
            voter_sync: dstore_view::VOTER_SYNC_DONE,
            nodes: Some(nodes),
            ..View::default()
        };
        let shared = Arc::new(Shared {
            ids: ids.clone(),
            endpoints: endpoints.clone(),
            ctx: Ctx::background().with_cancel(),
            state: Mutex::new(FakeState::new(view, ids.len(), RNG_SEED)),
            watchers: Mutex::new(Vec::new()),
            links: Mutex::new(HashMap::new()),
            tasks: Mutex::new(Vec::new()),
        });
        let mut tasks = Vec::with_capacity(ids.len());
        for idx in 0..ids.len() {
            tasks.push(tokio::spawn(shared.clone().accept_loop(idx)));
        }
        lock(&shared.tasks).extend(tasks);
        Arc::new(FakeCluster { endpoints, shared })
    }

    /// The node ids, in view order.
    pub fn ids(&self) -> Vec<NodeId> {
        self.shared.ids.clone()
    }

    /// A ticket naming every node with its `mem:` address, and the cluster id and incarnation.
    pub fn ticket(&self) -> Ticket {
        let st = self.shared.lock();
        Ticket {
            cluster_id: st.view.cluster_id.clone(),
            incarnation: st.view.incarnation,
            members: Some(
                self.shared
                    .ids
                    .iter()
                    .map(|id| Member {
                        id: Some(id.0.to_vec()),
                        addrs: vec![mem_addr(id)],
                    })
                    .collect(),
            ),
        }
    }

    /// The current view.
    pub fn view(&self) -> View {
        self.shared.lock().view.clone()
    }

    /// Bumps the epoch (and the version): requests stamped with the old epoch become stale.
    pub fn bump_epoch(&self) {
        let mut st = self.shared.lock();
        st.view.epoch += 1;
        st.view.version += 1;
        st.view_changed();
    }

    /// Sets a node's free-space flag: a non-writable node refuses puts with `no-space`, and its view
    /// entry's `Writable` follows, as `selfEntryLoop` would publish it.
    pub fn set_writable(&self, id: NodeId, writable: bool) {
        let Some(idx) = self.shared.index_of(&id) else {
            return;
        };
        let mut st = self.shared.lock();
        st.nodes[idx].writable = writable;
        if let Some(n) = st
            .view
            .nodes
            .as_mut()
            .and_then(|ns| ns.iter_mut().find(|n| n.id.as_deref() == Some(&id.0[..])))
        {
            n.writable = writable;
        }
        st.view.version += 1;
        st.view_changed();
    }

    /// Injects a fault into the `nth` client-ALPN request of type `op` (a `wire::T_*` value) at node
    /// `id`. `nth` counts from 1 over every request of that type the node receives; `nth == 0` applies
    /// to every such request.
    pub fn inject(&self, id: NodeId, op: i64, nth: usize, inj: Injection) {
        self.shared.lock().injections.push(Planned {
            node: id,
            op,
            nth,
            inj,
        });
    }

    /// The records a node stores, by key.
    pub fn stored(&self, id: NodeId) -> BTreeMap<[u8; 32], Vec<u8>> {
        let Some(idx) = self.shared.index_of(&id) else {
            return BTreeMap::new();
        };
        let st = self.shared.lock();
        st.nodes[idx]
            .store
            .iter()
            .map(|(k, s)| (*k, s.record.clone()))
            .collect()
    }

    /// The request transcript of a node: every client-ALPN request frame it read, in arrival order.
    pub fn requests(&self, id: NodeId) -> Vec<Msg> {
        let Some(idx) = self.shared.index_of(&id) else {
            return Vec::new();
        };
        self.shared.lock().nodes[idx].requests.clone()
    }

    /// The limit a ref-list page and each watch reconcile page ask the catalog for (Go: 20000). The
    /// acceptor clamps 0 and values over 100000 to 100000.
    pub fn set_ref_page_limit(&self, n: usize) {
        self.shared.lock().ref_page_limit = n;
    }

    /// The reconcile interval of watch streams started afterwards (Go `WatchReconcile`, 30 s).
    pub fn set_watch_reconcile(&self, d: Duration) {
        self.shared.lock().watch_reconcile = d;
    }

    /// Go `node.RefPutLocal` with `catalog.Cond{Force: true}`, the watch tests' `putLocal`: a reference
    /// write coordinated by node `id` in process, completeness-checked, committed and hinted.
    pub async fn ref_put_local(&self, id: NodeId, record: &[u8]) -> Result<Vec<u8>, String> {
        let Some(idx) = self.shared.index_of(&id) else {
            return Err(format!("fake: {} is not a node", short_id(&id)));
        };
        self.shared.local_ref_put(idx, record).await
    }

    /// Stops serving: every loop and watch stream ends and the endpoints close.
    pub async fn close(&self) {
        self.shared.ctx.cancel();
        let links: Vec<Arc<dyn Conn>> = lock(&self.shared.links).drain().map(|(_, c)| c).collect();
        for c in links {
            c.close();
        }
        for ep in &self.endpoints {
            ep.close().await;
        }
        let tasks: Vec<_> = lock(&self.shared.tasks).drain(..).collect();
        for t in tasks {
            t.abort();
        }
    }
}

impl Drop for FakeCluster {
    fn drop(&mut self) {
        self.shared.ctx.cancel();
        for t in lock(&self.shared.tasks).drain(..) {
            t.abort();
        }
    }
}

/// `view.DefaultMinReplicas`.
fn default_min_replicas(r: u8) -> u8 {
    let m = i64::from(r) - 1;
    m.max(2).min(i64::from(r)) as u8
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, FakeState> {
        lock(&self.state)
    }

    fn index_of(&self, id: &NodeId) -> Option<usize> {
        self.ids.iter().position(|x| x == id)
    }

    fn stamp(&self, mut m: Msg) -> Msg {
        stamp(&self.lock().view, &mut m);
        m
    }

    fn mark_reachable(&self, idx: usize, id: NodeId) {
        self.lock().note_reach(idx, id, true);
    }

    /// Whether node `from` reaches member `to` over the network now: a cached cluster-ALPN connection
    /// that is still open, else a fresh dial. It marks nothing: the callers mark the members a Go node
    /// would have called (`remoteMissing`, a forward, `getFrom`), and a broadcast marks nobody.
    async fn probe(&self, from: usize, to: NodeId) -> bool {
        let me = self.ids[from];
        if to == me {
            return true;
        }
        let cached = lock(&self.links).get(&(me, to)).cloned();
        if let Some(c) = cached
            && !c.is_closed()
        {
            return true;
        }
        if self.index_of(&to).is_none() {
            return false;
        }
        let ctx = self.ctx.with_timeout(LINK_DIAL_TIMEOUT);
        match self.endpoints[from]
            .dial(&ctx, to, vec![mem_addr(&to)], wire::ALPN_CLUSTER)
            .await
        {
            Ok(c) => {
                let old = lock(&self.links).insert((me, to), c);
                if let Some(old) = old {
                    old.close();
                }
                true
            }
            Err(_) => false,
        }
    }

    async fn probe_map(&self, from: usize, ids: &[NodeId]) -> HashMap<NodeId, bool> {
        let mut out = HashMap::new();
        for id in ids {
            if !out.contains_key(id) {
                let ok = self.probe(from, *id).await;
                out.insert(*id, ok);
            }
        }
        out
    }

    /// `acceptLoop`.
    async fn accept_loop(self: Arc<Self>, idx: usize) {
        let ep = self.endpoints[idx].clone();
        loop {
            match ep.accept(&self.ctx).await {
                Ok(conn) => {
                    tokio::spawn(self.clone().serve_conn(idx, conn));
                }
                Err(e) => {
                    if self.ctx.err().is_some() || matches!(e, TransportError::Closed) {
                        return;
                    }
                    if self.ctx.sleep(Duration::from_millis(50)).await.is_err() {
                        return;
                    }
                }
            }
        }
    }

    /// `serveConn`.
    async fn serve_conn(self: Arc<Self>, idx: usize, conn: Arc<dyn Conn>) {
        let alpn = conn.alpn();
        let client = alpn == wire::ALPN_CLIENT;
        if !client && alpn != wire::ALPN_CLUSTER {
            conn.close();
            return;
        }
        self.mark_reachable(idx, conn.remote_id());
        while let Ok(s) = conn.accept_stream(&self.ctx).await {
            tokio::spawn(self.clone().serve_stream(idx, conn.clone(), client, s));
        }
        conn.close();
    }

    /// `serveStream`: one operation per stream, with the transcript and injections of the fake.
    async fn serve_stream(
        self: Arc<Self>,
        idx: usize,
        conn: Arc<dyn Conn>,
        client: bool,
        mut s: Stream,
    ) {
        let m = match self.ctx.run(wire::read_msg(&mut s.recv)).await {
            Ok(Ok(m)) => m,
            _ => {
                close_stream(&mut s);
                return;
            }
        };
        let remote = conn.remote_id();
        if !client {
            let _ = self.serve_cluster(idx, remote, &m, &mut s.send).await;
            close_stream(&mut s);
            return;
        }
        let plan = self.on_request(idx, &m);
        for d in &plan.delays {
            if self.ctx.sleep(*d).await.is_err() {
                close_stream(&mut s);
                return;
            }
        }
        if plan.close_before_reply {
            if plan.err.is_none() && m.typ != wire::T_REF_WATCH {
                let mut sink = tokio::io::sink();
                let _ = self
                    .serve_client(idx, remote, &m, &mut s.recv, &mut sink, &plan)
                    .await;
            }
            close_stream(&mut s);
            conn.close();
            return;
        }
        if let Some(e) = &plan.err {
            let _ = self.write_injected_err(e, &mut s.send).await;
            close_stream(&mut s);
            return;
        }
        if m.typ == wire::T_REF_WATCH && self.acl_refusal(remote, m.typ).is_none() {
            self.handle_ref_watch(idx, m, s, &plan).await;
            return;
        }
        let Stream { send, recv } = &mut s;
        let _ = self.serve_client(idx, remote, &m, recv, send, &plan).await;
        close_stream(&mut s);
    }

    /// Records the request and collects the injections that match it.
    fn on_request(&self, idx: usize, m: &Msg) -> Plan {
        let me = self.ids[idx];
        let mut guard = self.lock();
        let st = &mut *guard;
        let node = &mut st.nodes[idx];
        node.requests.push(m.clone());
        let count = node.counts.entry(m.typ).or_insert(0);
        *count += 1;
        let nth = *count;
        let mut plan = Plan::default();
        for p in st
            .injections
            .iter()
            .filter(|p| p.node == me && p.op == m.typ && (p.nth == 0 || p.nth == nth))
        {
            match &p.inj {
                Injection::Delay(d) => plan.delays.push(*d),
                Injection::CloseBeforeReply => plan.close_before_reply = true,
                Injection::Err {
                    code,
                    text,
                    with_view,
                    retry_after_ms,
                } => {
                    if plan.err.is_none() {
                        plan.err = Some(InjectedErr {
                            code: code.clone(),
                            text: text.clone(),
                            with_view: *with_view,
                            retry_after_ms: *retry_after_ms,
                        });
                    }
                }
                Injection::CorruptRecord(k) => plan.corrupt.push(*k),
                Injection::DropHints => plan.drop_hints = true,
            }
        }
        plan
    }

    async fn write_injected_err<W>(&self, e: &InjectedErr, out: &mut W) -> Result<(), WireError>
    where
        W: AsyncWrite + Unpin + Send + ?Sized,
    {
        let m = {
            let st = self.lock();
            let mut m = Msg {
                typ: wire::T_ERR,
                code: e.code.clone(),
                text: e.text.clone(),
                retry_after: e.retry_after_ms,
                ..Msg::default()
            };
            if e.with_view {
                m.view = dstore_codec::marshal(&st.view);
                stamp(&st.view, &mut m);
            }
            m
        };
        wire::write_msg(out, &m).await
    }

    /// The allowlist check of `serveClient`.
    fn acl_refusal(&self, remote: NodeId, typ: i64) -> Option<&'static str> {
        let st = self.lock();
        let acl = st.view.acl.as_ref()?;
        if acl.allowed.is_empty() {
            return None;
        }
        if !contains(&acl.allowed, &remote) && !contains(&acl.admins, &remote) {
            return Some("not on the allowlist");
        }
        if typ == wire::T_REF_DELETE && !contains(&acl.admins, &remote) {
            return Some("ref-delete needs an admin peer");
        }
        None
    }

    /// `serveCluster`, as far as members need it: view and ping.
    async fn serve_cluster<W>(
        &self,
        idx: usize,
        remote: NodeId,
        m: &Msg,
        out: &mut W,
    ) -> Result<(), WireError>
    where
        W: AsyncWrite + Unpin + Send + ?Sized,
    {
        let member = {
            let st = self.lock();
            all_members(&st.view).contains(&remote)
                || st
                    .view
                    .voters
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .any(|v| v.id.as_deref() == Some(&remote.0[..]))
        };
        if !member {
            return wire::write_err(out, wire::CODE_NOT_MEMBER, "not a member of the view").await;
        }
        match m.typ {
            wire::T_VIEW => self.handle_view(idx, out).await,
            wire::T_PING => {
                let pong = self.stamp(Msg {
                    typ: wire::T_PONG,
                    ..Msg::default()
                });
                wire::write_msg(out, &pong).await
            }
            _ => wire::write_err(out, wire::CODE_BAD_REQUEST, "unknown cluster operation").await,
        }
    }

    /// `serveClient` for every operation but `ref-watch`, which needs the whole stream.
    async fn serve_client<W>(
        self: &Arc<Self>,
        idx: usize,
        remote: NodeId,
        m: &Msg,
        recv: &mut Box<dyn RecvStream>,
        out: &mut W,
        plan: &Plan,
    ) -> Result<(), WireError>
    where
        W: AsyncWrite + Unpin + Send + ?Sized,
    {
        if let Some(text) = self.acl_refusal(remote, m.typ) {
            return wire::write_err(out, wire::CODE_UNAUTHORIZED, text).await;
        }
        match m.typ {
            wire::T_VIEW => self.handle_view(idx, out).await,
            wire::T_MISSING => self.handle_missing(idx, m, out).await,
            wire::T_GET => self.handle_get(idx, m, out, plan).await,
            wire::T_PUT => self.handle_put(idx, m, recv, out).await,
            wire::T_REF_GET => self.handle_ref_get(m, out).await,
            wire::T_REF_PUT => self.handle_ref_put(idx, m, out, plan).await,
            wire::T_REF_DELETE => self.handle_ref_delete(idx, m, out, plan).await,
            wire::T_REF_LIST => self.handle_ref_list(m, out).await,
            wire::T_REF_WATCH => Ok(()),
            wire::T_STATUS => self.handle_status(idx, out).await,
            wire::T_ADMIN => self.handle_admin(idx, m, out).await,
            wire::T_PING => {
                let pong = self.stamp(Msg {
                    typ: wire::T_PONG,
                    ..Msg::default()
                });
                wire::write_msg(out, &pong).await
            }
            _ => wire::write_err(out, wire::CODE_BAD_REQUEST, "unknown operation").await,
        }
    }

    /// `writeStale`.
    async fn write_stale<W>(&self, out: &mut W) -> Result<(), WireError>
    where
        W: AsyncWrite + Unpin + Send + ?Sized,
    {
        let m = {
            let st = self.lock();
            let mut m = Msg {
                typ: wire::T_ERR,
                code: wire::CODE_STALE_VIEW.to_string(),
                text: "request epoch is behind".to_string(),
                view: dstore_codec::marshal(&st.view),
                ..Msg::default()
            };
            stamp(&st.view, &mut m);
            m
        };
        wire::write_msg(out, &m).await
    }

    /// `handleView`.
    async fn handle_view<W>(&self, idx: usize, out: &mut W) -> Result<(), WireError>
    where
        W: AsyncWrite + Unpin + Send + ?Sized,
    {
        let m = {
            let mut st = self.lock();
            let unreachable = unreachable_ids(&mut st.nodes[idx]);
            let mut m = Msg {
                typ: wire::T_VIEW_REPLY,
                view: dstore_codec::marshal(&st.view),
                unreachable,
                ..Msg::default()
            };
            stamp(&st.view, &mut m);
            m
        };
        wire::write_msg(out, &m).await
    }

    /// `handleMissing` on the client ALPN.
    async fn handle_missing<W>(&self, idx: usize, m: &Msg, out: &mut W) -> Result<(), WireError>
    where
        W: AsyncWrite + Unpin + Send + ?Sized,
    {
        if m.keys.len() > wire::MAX_KEYS {
            return wire::write_err(out, wire::CODE_BAD_REQUEST, "too many keys").await;
        }
        let keys = match wire::keys32(&m.keys) {
            Ok(k) => k,
            Err(e) => return wire::write_err(out, wire::CODE_BAD_REQUEST, &e.to_string()).await,
        };
        let (lacking, present, pl) = {
            let mut st = self.lock();
            let (lacking, present) = local_missing(&mut st.nodes[idx], &keys, m.pin);
            let pl = (!present.is_empty()).then(|| st.placement());
            (lacking, present, pl)
        };
        let mut reply = self.stamp(Msg {
            typ: wire::T_MISSING_REPLY,
            keys: wire::raw_keys(&lacking),
            ..Msg::default()
        });
        if let Some(pl) = pl {
            reply.short = self.negotiate_holders(idx, &pl, &present, m.pin).await;
        }
        wire::write_msg(out, &reply).await
    }

    /// `negotiateHolders`: the keys held by fewer owners than their write set, with their holders.
    async fn negotiate_holders(
        &self,
        idx: usize,
        pl: &Placement,
        present: &[[u8; 32]],
        pin: bool,
    ) -> Vec<KeyHolders> {
        let me = self.ids[idx];
        let write_sets: Vec<Vec<NodeId>> = present.iter().map(|k| pl.write_set(k)).collect();
        let mut by_owner: Vec<(NodeId, Vec<[u8; 32]>)> = Vec::new();
        for (k, ws) in present.iter().zip(&write_sets) {
            for o in ws.iter().filter(|o| **o != me) {
                match by_owner.iter_mut().find(|(id, _)| id == o) {
                    Some((_, ks)) => ks.push(*k),
                    None => by_owner.push((*o, vec![*k])),
                }
            }
        }
        let owners: Vec<NodeId> = by_owner.iter().map(|(o, _)| *o).collect();
        let reach = self.probe_map(idx, &owners).await;
        let mut holders: HashMap<[u8; 32], Vec<NodeId>> =
            present.iter().map(|k| (*k, vec![me])).collect();
        {
            let mut st = self.lock();
            for (o, ks) in &by_owner {
                let ok = reach.get(o).copied().unwrap_or(false);
                st.note_reach(idx, *o, ok);
                let o_idx = self.index_of(o).filter(|_| ok);
                let Some(o_idx) = o_idx else {
                    for k in ks {
                        if let Some(h) = holders.get_mut(k) {
                            h.push(NodeId([0u8; 32]));
                        }
                    }
                    continue;
                };
                let (lacking, _) = local_missing(&mut st.nodes[o_idx], ks, pin);
                let lack: HashSet<[u8; 32]> = lacking.into_iter().collect();
                for k in ks.iter().filter(|k| !lack.contains(*k)) {
                    if let Some(h) = holders.get_mut(k) {
                        h.push(*o);
                    }
                }
            }
        }
        let mut out = Vec::new();
        for (k, ws) in present.iter().zip(&write_sets) {
            let ids: Vec<Vec<u8>> = holders
                .get(k)
                .map(|hs| {
                    hs.iter()
                        .filter(|h| h.0 != [0u8; 32])
                        .map(|h| h.0.to_vec())
                        .collect()
                })
                .unwrap_or_default();
            if ids.len() < ws.len() {
                out.push(KeyHolders {
                    key: Some(k.to_vec()),
                    holders: ids,
                });
            }
        }
        out
    }

    /// `handleGet`.
    async fn handle_get<W>(
        &self,
        idx: usize,
        m: &Msg,
        out: &mut W,
        plan: &Plan,
    ) -> Result<(), WireError>
    where
        W: AsyncWrite + Unpin + Send + ?Sized,
    {
        self.lock().nodes[idx].gets += 1;
        if m.keys.len() > wire::MAX_KEYS {
            return wire::write_err(out, wire::CODE_BAD_REQUEST, "too many keys").await;
        }
        let keys = match wire::keys32(&m.keys) {
            Ok(k) => k,
            Err(e) => return wire::write_err(out, wire::CODE_BAD_REQUEST, &e.to_string()).await,
        };
        let (absent, mut present) = {
            let st = self.lock();
            let node = &st.nodes[idx];
            let mut absent = Vec::new();
            let mut present = Vec::new();
            for k in &keys {
                match node.store.get(k) {
                    Some(s) => present.push((s.seq, *k, s.record.clone())),
                    None => absent.push(k.to_vec()),
                }
            }
            (absent, present)
        };
        present.sort_by_key(|(seq, _, _)| *seq);
        let reply = self.stamp(Msg {
            typ: wire::T_ABSENT,
            keys: absent,
            ..Msg::default()
        });
        wire::write_msg(out, &reply).await?;
        let mut sent = 0u64;
        let mut res = Ok(());
        let mut sender = PackSender::new(&mut *out);
        for (_, k, rec) in &present {
            let bytes = if plan.corrupt.contains(k) {
                tamper(rec)
            } else {
                rec.clone()
            };
            sent += bytes.len() as u64;
            if let Err(e) = sender.add_record(&bytes).await {
                res = Err(e);
                break;
            }
        }
        if res.is_ok() {
            res = sender.finish().await;
        }
        self.lock().nodes[idx].bytes_out += sent;
        res
    }

    /// `handlePut` on the client ALPN, with the forwards of `putTx` against the shared stores.
    async fn handle_put<W>(
        &self,
        idx: usize,
        m: &Msg,
        recv: &mut Box<dyn RecvStream>,
        out: &mut W,
    ) -> Result<(), WireError>
    where
        W: AsyncWrite + Unpin + Send + ?Sized,
    {
        let me = self.ids[idx];
        let (epoch, writable) = {
            let mut st = self.lock();
            st.nodes[idx].puts += 1;
            (check_epoch(&st.view, m), st.nodes[idx].writable)
        };
        match epoch {
            Err(EpochErr::Stale) => return self.write_stale(out).await,
            Err(EpochErr::Other(text)) => {
                return wire::write_err(out, wire::CODE_BAD_REQUEST, text).await;
            }
            Ok(()) => {}
        }
        if !writable {
            return wire::write_err(
                out,
                wire::CODE_NO_SPACE,
                "node below its free-space reserve",
            )
            .await;
        }
        let pl = self.lock().placement();
        let mut rejected = Vec::new();
        let mut total = 0usize;
        let mut seen = HashSet::new();
        let mut tx_keys: Vec<[u8; 32]> = Vec::new();
        let mut tx_records: HashMap<[u8; 32], Vec<u8>> = HashMap::new();
        let mut failure = None;
        {
            let mut records = PackRecords::new(PackReader::new(&mut *recv));
            while let Some(item) = records.next().await {
                let raw = match item {
                    Ok(raw) => raw,
                    Err(e) => {
                        failure = Some(format!("pack: {e}"));
                        break;
                    }
                };
                total += raw.bytes.len();
                if total > wire::MAX_PUT_BATCH {
                    failure = Some("batch over 64 MiB".to_string());
                    break;
                }
                let (k, rec) = match verify_record(&raw) {
                    Ok(v) => v,
                    Err(e) => {
                        rejected.push(KeyReject {
                            key: Some(raw.record.key.0.to_vec()),
                            reason: format!("verify: {e}"),
                        });
                        continue;
                    }
                };
                if !pl.in_write_set(&k, &me) {
                    rejected.push(KeyReject {
                        key: Some(k.to_vec()),
                        reason: wire::CODE_NOT_OWNER.to_string(),
                    });
                    continue;
                }
                if !seen.insert(k) {
                    continue;
                }
                store_record(&mut self.lock().nodes[idx], k, &rec);
                tx_keys.push(k);
                tx_records.insert(k, rec);
            }
        }
        if let Some(text) = failure {
            return wire::write_err(out, wire::CODE_BAD_REQUEST, &text).await;
        }
        self.lock().nodes[idx].bytes_in += total as u64;

        // Forwards to each key's other owners.
        let mut by_owner: Vec<(NodeId, Vec<[u8; 32]>)> = Vec::new();
        for k in &tx_keys {
            for o in pl.write_set(k).into_iter().filter(|o| *o != me) {
                match by_owner.iter_mut().find(|(id, _)| *id == o) {
                    Some((_, ks)) => ks.push(*k),
                    None => by_owner.push((o, vec![*k])),
                }
            }
        }
        let owners: Vec<NodeId> = by_owner.iter().map(|(o, _)| *o).collect();
        let reach = self.probe_map(idx, &owners).await;
        let mut holders: HashMap<[u8; 32], Vec<Vec<u8>>> =
            tx_keys.iter().map(|k| (*k, vec![me.0.to_vec()])).collect();
        let mut failed = Vec::new();
        {
            let mut st = self.lock();
            for (o, ks) in &by_owner {
                let reason = match self.index_of(o) {
                    Some(o_idx) if reach.get(o).copied().unwrap_or(false) => {
                        // The forward's reply marks the owner reachable; a refusal marks nothing.
                        if st.nodes[o_idx].writable {
                            st.note_reach(idx, *o, true);
                            for k in ks {
                                if let Some(rec) = tx_records.get(k) {
                                    store_record(&mut st.nodes[o_idx], *k, rec);
                                }
                                if let Some(h) = holders.get_mut(k) {
                                    h.push(o.0.to_vec());
                                }
                            }
                            None
                        } else {
                            Some(wire::CODE_NO_SPACE)
                        }
                    }
                    _ => {
                        st.note_reach(idx, *o, false);
                        Some("unreachable")
                    }
                };
                if let Some(reason) = reason {
                    for k in ks {
                        let retry_after = 500 + (st.rng.next_u64() % 1500) as i64;
                        failed.push(KeyFailure {
                            key: Some(k.to_vec()),
                            node: Some(o.0.to_vec()),
                            reason: reason.to_string(),
                            retry_after,
                        });
                    }
                }
            }
        }
        let mut reply = Msg {
            typ: wire::T_PUT_RESULT,
            rejected,
            failed,
            ..Msg::default()
        };
        for k in &tx_keys {
            reply.holders.push(KeyHolders {
                key: Some(k.to_vec()),
                holders: holders.remove(k).unwrap_or_default(),
            });
        }
        let reply = self.stamp(reply);
        wire::write_msg(out, &reply).await
    }

    /// `handleRefGet`.
    async fn handle_ref_get<W>(&self, m: &Msg, out: &mut W) -> Result<(), WireError>
    where
        W: AsyncWrite + Unpin + Send + ?Sized,
    {
        if let Err(e) = reference::validate_name(&m.name) {
            return wire::write_err(out, wire::CODE_BAD_REQUEST, &e.to_string()).await;
        }
        let reply = {
            let st = self.lock();
            st.catalog.get(&m.name).map(|rv| {
                let mut r = Msg {
                    typ: wire::T_REF,
                    record: rv.record.clone(),
                    version: rv.version.clone(),
                    ..Msg::default()
                };
                stamp(&st.view, &mut r);
                r
            })
        };
        match reply {
            Some(r) => wire::write_msg(out, &r).await,
            None => wire::write_err(out, wire::CODE_UNKNOWN_REF, "no such reference").await,
        }
    }

    /// `writeCASMismatch`.
    async fn write_cas_mismatch<W>(&self, mm: &CasMismatch, out: &mut W) -> Result<(), WireError>
    where
        W: AsyncWrite + Unpin + Send + ?Sized,
    {
        let mut reply = Msg {
            typ: wire::T_CAS_MISMATCH,
            version: mm.version.clone(),
            has_current: mm.has_current,
            ..Msg::default()
        };
        if let Some(cur) = &mm.current {
            reply.record = cur.record.clone();
            reply.current = key_of_record(&cur.record).unwrap_or_default();
        }
        let reply = self.stamp(reply);
        wire::write_msg(out, &reply).await
    }

    /// `handleRefPut`.
    async fn handle_ref_put<W>(
        self: &Arc<Self>,
        idx: usize,
        m: &Msg,
        out: &mut W,
        plan: &Plan,
    ) -> Result<(), WireError>
    where
        W: AsyncWrite + Unpin + Send + ?Sized,
    {
        let me = self.ids[idx];
        self.lock().nodes[idx].ref_puts += 1;
        let rec = match Reference::decode(&m.record) {
            Ok(r) => r,
            Err(e) => {
                return wire::write_err(out, wire::CODE_BAD_REQUEST, &format!("record: {e}")).await;
            }
        };
        if rec.key.len() != 32 {
            return wire::write_err(out, wire::CODE_BAD_REQUEST, "record key").await;
        }
        if let Err(e) = reference::validate_name(&rec.name) {
            return wire::write_err(out, wire::CODE_BAD_REQUEST, &format!("name: {e}")).await;
        }
        if !m.name.is_empty() && m.name != rec.name {
            return wire::write_err(
                out,
                wire::CODE_BAD_REQUEST,
                "frame name differs from the record's",
            )
            .await;
        }
        let root = match Key::parse(&rec.key) {
            Ok(k) => k,
            Err(e) => {
                return wire::write_err(out, wire::CODE_BAD_REQUEST, &format!("root key: {e}"))
                    .await;
            }
        };
        let stale = matches!(check_epoch(&self.lock().view, m), Err(EpochErr::Stale));
        if stale {
            return self.write_stale(out).await;
        }
        let (missing, shortfall) = self.walk_complete(idx, root).await;
        if !missing.is_empty() {
            let sample = &missing[..missing.len().min(INCOMPLETE_SAMPLE)];
            let reply = self.stamp(Msg {
                typ: wire::T_INCOMPLETE,
                keys: wire::raw_keys(sample),
                shortfall,
                ..Msg::default()
            });
            return wire::write_msg(out, &reply).await;
        }
        let result = self
            .lock()
            .catalog_put(me, &rec.name, &m.record, &cond_of(m));
        match result {
            Err(mm) => self.write_cas_mismatch(&mm, out).await,
            Ok(version) => {
                self.ref_changed(
                    idx,
                    &rec.name,
                    Some(m.record.clone()),
                    version.clone(),
                    plan.drop_hints,
                );
                let reply = self.stamp(Msg {
                    typ: wire::T_OK,
                    key: rec.key.clone(),
                    version,
                    ..Msg::default()
                });
                wire::write_msg(out, &reply).await
            }
        }
    }

    /// `walkComplete`: the keys under `root` held by fewer than `min(minR, owners)` owners (at most
    /// 1024 of them) and their total.
    async fn walk_complete(&self, idx: usize, root: Key) -> (Vec<[u8; 32]>, i64) {
        let me = self.ids[idx];
        let members: Vec<NodeId> = {
            let st = self.lock();
            all_members(&st.view)
                .into_iter()
                .filter(|id| *id != me)
                .collect()
        };
        let reach = self.probe_map(idx, &members).await;
        let mut st = self.lock();
        let pl = st.placement();
        let min_r = usize::from(st.view.min_replicas);
        let mut missing = Vec::new();
        let mut shortfall = 0i64;
        let mut seen: HashSet<[u8; 32]> = HashSet::new();
        let mut to_check: Vec<[u8; 32]> = Vec::new();
        let mut frontier = vec![root.0];
        let mut flush = |st: &mut FakeState, to_check: &mut Vec<[u8; 32]>| {
            if to_check.is_empty() {
                return;
            }
            for k in st.negotiate_complete(&self.ids, idx, &reach, &pl, to_check, min_r) {
                if missing.len() < MISSING_CAP {
                    missing.push(k);
                }
                shortfall += 1;
            }
            to_check.clear();
        };
        while !frontier.is_empty() {
            let mut interior = Vec::new();
            for k in frontier {
                if !seen.insert(k) {
                    continue;
                }
                to_check.push(k);
                if !is_leaf(&k) {
                    interior.push(k);
                }
            }
            let mut next = Vec::new();
            for k in &interior {
                let Some(data) = st.get_data(&self.ids, idx, &reach, &pl, k) else {
                    continue;
                };
                if let Ok(kids) = fstree::child_keys(Key(*k), &data) {
                    next.extend(kids.into_iter().map(|c| c.0));
                }
            }
            if to_check.len() >= wire::MAX_KEYS {
                flush(&mut st, &mut to_check);
            }
            frontier = next;
        }
        flush(&mut st, &mut to_check);
        drop(st);
        (missing, shortfall)
    }

    /// `handleRefDelete`.
    async fn handle_ref_delete<W>(
        self: &Arc<Self>,
        idx: usize,
        m: &Msg,
        out: &mut W,
        plan: &Plan,
    ) -> Result<(), WireError>
    where
        W: AsyncWrite + Unpin + Send + ?Sized,
    {
        if let Err(e) = reference::validate_name(&m.name) {
            return wire::write_err(out, wire::CODE_BAD_REQUEST, &e.to_string()).await;
        }
        let me = self.ids[idx];
        let result = self.lock().catalog_delete(me, &m.name, &cond_of(m));
        match result {
            Err(mm) => self.write_cas_mismatch(&mm, out).await,
            Ok(deleted) => {
                if let Some(version) = deleted {
                    self.ref_changed(idx, &m.name, None, version, plan.drop_hints);
                }
                let reply = self.stamp(Msg {
                    typ: wire::T_OK,
                    ..Msg::default()
                });
                wire::write_msg(out, &reply).await
            }
        }
    }

    /// `handleRefList`.
    async fn handle_ref_list<W>(&self, m: &Msg, out: &mut W) -> Result<(), WireError>
    where
        W: AsyncWrite + Unpin + Send + ?Sized,
    {
        let reply = {
            let st = self.lock();
            let (entries, mut next) = st.catalog_list(&m.prefix, &m.after, st.ref_page_limit);
            let mut reply = Msg {
                typ: wire::T_REFS,
                ..Msg::default()
            };
            stamp(&st.view, &mut reply);
            let mut size = 0usize;
            for (name, rv) in entries {
                let Ok(r) = Reference::decode(&rv.record) else {
                    continue;
                };
                size += name.len() + 32 + 40 + r.user.len() + 16;
                reply.refs.push(RefInfo {
                    name: name.clone(),
                    key: Some(r.key),
                    version: Some(rv.version),
                    created_at: r.created_at,
                    user: r.user,
                });
                if size > wire::MAX_PAGE_BYTES {
                    next = name;
                    break;
                }
            }
            if !next.is_empty() {
                reply.next = next.into_bytes();
            }
            reply
        };
        wire::write_msg(out, &reply).await
    }

    /// `handleStatus`, over canned figures.
    async fn handle_status<W>(&self, idx: usize, out: &mut W) -> Result<(), WireError>
    where
        W: AsyncWrite + Unpin + Send + ?Sized,
    {
        let me = self.ids[idx];
        let watchers = lock(&self.watchers).iter().filter(|s| s.node == me).count() as i64;
        let reply = {
            let mut st = self.lock();
            let unreachable = unreachable_ids(&mut st.nodes[idx]);
            let node = &st.nodes[idx];
            let bytes: i64 = node.store.values().map(|s| s.record.len() as i64).sum();
            let status = Status {
                id: Some(me.0.to_vec()),
                epoch: st.view.epoch,
                incarnation: st.view.incarnation,
                packs: i64::from(!node.store.is_empty()),
                records: node.store.len() as u64,
                bytes,
                pins: node.pins.len() as i64,
                unreachable,
                transition: transition_text(&st.view),
                gc: gc_text(st.gc_hold),
                lease_holder: self.ids.first().map(|id| id.0.to_vec()).unwrap_or_default(),
                writable: node.writable,
                free_bytes: TOTAL_BYTES - bytes,
                total_bytes: TOTAL_BYTES,
                puts: node.puts,
                gets: node.gets,
                ref_puts: node.ref_puts,
                bytes_in: node.bytes_in,
                bytes_out: node.bytes_out,
                is_holder: idx == 0,
                watchers,
                ..Status::default()
            };
            let mut m = Msg {
                typ: wire::T_STATUS_REPLY,
                status: dstore_codec::marshal(&status),
                ..Msg::default()
            };
            stamp(&st.view, &mut m);
            m
        };
        wire::write_msg(out, &reply).await
    }

    /// `handleAdmin`.
    async fn handle_admin<W>(
        self: &Arc<Self>,
        idx: usize,
        m: &Msg,
        out: &mut W,
    ) -> Result<(), WireError>
    where
        W: AsyncWrite + Unpin + Send + ?Sized,
    {
        let req = match dstore_codec::unmarshal::<AdminRequest>(&m.params) {
            Ok(r) => r,
            Err(e) => return wire::write_err(out, wire::CODE_BAD_REQUEST, &e.to_string()).await,
        };
        let result = if req.op == "catalog-backup" {
            self.catalog_backup(idx).await
        } else {
            self.admin(idx, &req)
        };
        match result {
            Ok(reply) => {
                let reply = self.stamp(Msg {
                    typ: wire::T_ADMIN_REPLY,
                    status: dstore_codec::marshal(&reply),
                    ..Msg::default()
                });
                wire::write_msg(out, &reply).await
            }
            Err(AdminErr::Remote(code, text)) => wire::write_err(out, code, &text).await,
            Err(AdminErr::Other(text)) => wire::write_err(out, wire::CODE_UNAVAILABLE, &text).await,
        }
    }

    /// node `Admin` with canned results.
    fn admin(&self, idx: usize, req: &AdminRequest) -> Result<AdminReply, AdminErr> {
        let me = self.ids[idx];
        let mut st = self.lock();
        let op = req.op.as_str();
        match op {
            "token-create" => {
                let token = st.random_bytes(32);
                Ok(AdminReply {
                    text: hex::encode(&token),
                    token,
                    ..AdminReply::default()
                })
            }
            "cluster-ticket" => {
                let nodes = st.view.nodes.as_deref().unwrap_or(&[]);
                let me_id = nodes
                    .iter()
                    .chain(
                        st.view
                            .pending
                            .as_ref()
                            .and_then(|p| p.nodes.as_deref())
                            .unwrap_or(&[]),
                    )
                    .find(|n| n.id.as_deref() == Some(&me.0[..]))
                    .and_then(|n| n.id.clone());
                let mut members = vec![Member {
                    id: me_id,
                    addrs: vec![mem_addr(&me)],
                }];
                members.extend(nodes.iter().take(3).map(|n| Member {
                    id: n.id.clone(),
                    addrs: n.addrs.clone(),
                }));
                let t = Ticket {
                    cluster_id: st.view.cluster_id.clone(),
                    incarnation: st.view.incarnation,
                    members: Some(members),
                };
                Ok(AdminReply {
                    ticket: t.encode(),
                    ..AdminReply::default()
                })
            }
            "node-remove" | "node-drain" | "node-weight" | "node-zone" | "node-repair" => {
                let id = node_id_of(&req.node)?;
                if op == "node-repair" {
                    return Ok(AdminReply {
                        text: format!("repair scheduled: holders will refill {}", short_id(&id)),
                        ..AdminReply::default()
                    });
                }
                // nodeChange, then proposeTransition's checks; the transition itself is not proposed.
                if view_node(&st.view, &id).is_none() {
                    return Err(AdminErr::Other(format!(
                        "{} is not a member",
                        short_id(&id)
                    )));
                }
                transition_admissible(&st.view)?;
                Ok(AdminReply {
                    view: dstore_codec::marshal(&st.view),
                    text: format!("transition proposed at epoch {}", st.view.epoch),
                    ..AdminReply::default()
                })
            }
            "voter-add" | "voter-remove" => {
                node_id_of(&req.node)?;
                Ok(AdminReply {
                    view: dstore_codec::marshal(&st.view),
                    text: format!(
                        "voters now {}",
                        st.view.voters.as_deref().map_or(0, <[Voter]>::len)
                    ),
                    ..AdminReply::default()
                })
            }
            "replicas" => {
                transition_admissible(&st.view)?;
                if req.replicas == 0 {
                    return Err(AdminErr::Other("replicas must be ≥ 1".to_string()));
                }
                Ok(AdminReply {
                    view: dstore_codec::marshal(&st.view),
                    text: "transition proposed".to_string(),
                    ..AdminReply::default()
                })
            }
            "transition-status" => Ok(AdminReply {
                view: dstore_codec::marshal(&st.view),
                text: transition_text(&st.view),
                ..AdminReply::default()
            }),
            "transition-abort" => {
                if st.view.pending.take().is_some() {
                    st.view.epoch += 1;
                    st.view.version += 1;
                    st.view_changed();
                }
                Ok(AdminReply {
                    text: "transition aborted".to_string(),
                    ..AdminReply::default()
                })
            }
            "transition-refreeze" => {
                // maintenance.refreeze with force: keep the participants that are done, next round.
                let Some(p) = st.view.pending.as_mut().filter(|p| p.frozen) else {
                    return Err(AdminErr::Other("no frozen transition".to_string()));
                };
                let done = std::mem::take(&mut p.done);
                p.participants
                    .retain(|id| contains(&done, &nid_of(id.as_deref().unwrap_or(&[]))));
                p.round += 1;
                p.primary_done.clear();
                p.frozen_at = unix_nanos();
                st.view.epoch += 1;
                st.view.version += 1;
                st.view_changed();
                Ok(AdminReply {
                    view: dstore_codec::marshal(&st.view),
                    text: "participants re-frozen".to_string(),
                    ..AdminReply::default()
                })
            }
            "transition-pause" | "transition-resume" | "rate-cap" => {
                match op {
                    "transition-pause" => st.view.rebalance_pause = true,
                    "transition-resume" => st.view.rebalance_pause = false,
                    _ => st.view.rate_cap = req.rate,
                }
                st.view.version += 1;
                st.view_changed();
                Ok(AdminReply {
                    text: "ok".to_string(),
                    ..AdminReply::default()
                })
            }
            "gc-run" | "gc-status" => Ok(AdminReply {
                gc: gc_state_cbor(st.gc_hold),
                text: gc_text(st.gc_hold),
                ..AdminReply::default()
            }),
            "gc-hold" => {
                st.gc_hold = req.pause;
                Ok(AdminReply {
                    text: "ok".to_string(),
                    ..AdminReply::default()
                })
            }
            "gc-why" => {
                let Ok(k) = <[u8; 32]>::try_from(req.key.as_slice()) else {
                    return Err(AdminErr::Other("key must be 32 bytes".to_string()));
                };
                Ok(AdminReply {
                    names: st.why(&k),
                    ..AdminReply::default()
                })
            }
            "catalog-backups" => Ok(AdminReply {
                names: st.backups.iter().map(hex::encode).collect(),
                ..AdminReply::default()
            }),
            "keep" => Ok(AdminReply {
                text: "ok".to_string(),
                ..AdminReply::default()
            }),
            _ => Err(AdminErr::Remote(
                wire::CODE_BAD_REQUEST,
                format!("unknown admin op {}", req.op),
            )),
        }
    }

    /// `refChanged`: this node's watchers learn of the commit now; every other member reachable from
    /// it gets the hint unless `drop_broadcast`.
    fn ref_changed(
        self: &Arc<Self>,
        idx: usize,
        name: &str,
        record: Option<Vec<u8>>,
        version: Vec<u8>,
        drop_broadcast: bool,
    ) {
        let me = self.ids[idx];
        let hint = Hint {
            name: name.to_string(),
            record,
            version,
        };
        self.hint_node(me, &hint);
        if drop_broadcast {
            return;
        }
        let members = all_members(&self.lock().view);
        for id in members.into_iter().filter(|id| *id != me) {
            let shared = self.clone();
            let hint = hint.clone();
            tokio::spawn(async move {
                if shared.probe(idx, id).await {
                    shared.hint_node(id, &hint);
                }
            });
        }
    }

    /// node `RefPutLocal` with `catalog.Cond{Force: true}`: completeness-checked, committed, hinted.
    async fn local_ref_put(self: &Arc<Self>, idx: usize, record: &[u8]) -> Result<Vec<u8>, String> {
        let rec = Reference::decode(record).map_err(|e| e.to_string())?;
        let root = Key::parse(&rec.key).map_err(|e| e.to_string())?;
        let (missing, shortfall) = self.walk_complete(idx, root).await;
        if shortfall > 0 {
            let sample = missing
                .first()
                .map(|k| hex::encode(&k[..8]))
                .unwrap_or_default();
            return Err(format!(
                "incomplete: {shortfall} keys short (e.g. {sample})"
            ));
        }
        let cond = Cond {
            force: true,
            ..Cond::default()
        };
        let version = self
            .lock()
            .catalog_put(self.ids[idx], &rec.name, record, &cond)
            .map_err(|_| "catalog: cas mismatch".to_string())?;
        self.ref_changed(
            idx,
            &rec.name,
            Some(record.to_vec()),
            version.clone(),
            false,
        );
        Ok(version)
    }

    /// `maintenance.backupCatalog`: the references but the backup's own as `[]backupEntry`, stored as a
    /// Blob here and forwarded to its other owners that lack it, committed as the reference
    /// `dstore/catalog-backup` (user `dstore`) through `RefPutLocal`, then noted (the last 24 are kept).
    async fn catalog_backup(self: &Arc<Self>, idx: usize) -> Result<AdminReply, AdminErr> {
        let me = self.ids[idx];
        let (k, rec, owners) = {
            let mut st = self.lock();
            let data = {
                let entries: Vec<(&String, &RefValue)> = st
                    .catalog
                    .iter()
                    .filter(|(name, _)| name.as_str() != BACKUP_NAME)
                    .collect();
                backup_cbor(&entries)
            };
            let k = Key::new(Type::Blob, data.len() as u64, &data);
            let rec =
                amberpack::encode_record(k, &data).map_err(|e| AdminErr::Other(e.to_string()))?;
            insert_record(&mut st.nodes[idx], k.0, &rec);
            let owners: Vec<NodeId> = st
                .placement()
                .write_set(&k.0)
                .into_iter()
                .filter(|o| *o != me)
                .collect();
            (k, rec, owners)
        };
        let reach = self.probe_map(idx, &owners).await;
        {
            let mut st = self.lock();
            for o in &owners {
                let ok = reach.get(o).copied().unwrap_or(false);
                // forwardTo's remoteMissing marks the owner; a non-writable owner refuses the put.
                st.note_reach(idx, *o, ok);
                if let Some(o_idx) = self.index_of(o).filter(|_| ok)
                    && st.nodes[o_idx].writable
                {
                    insert_record(&mut st.nodes[o_idx], k.0, &rec);
                }
            }
        }
        let reference = Reference {
            name: BACKUP_NAME.to_string(),
            key: k.0.to_vec(),
            user: "dstore".to_string(),
            created_at: unix_nanos(),
            ..Reference::default()
        };
        let enc = reference
            .encode()
            .map_err(|e| AdminErr::Other(e.to_string()))?;
        self.local_ref_put(idx, &enc)
            .await
            .map_err(AdminErr::Other)?;
        let mut st = self.lock();
        st.backups.push(k.0);
        if st.backups.len() > MAX_BACKUPS {
            let excess = st.backups.len() - MAX_BACKUPS;
            st.backups.drain(..excess);
        }
        Ok(AdminReply {
            key: k.0.to_vec(),
            text: "backup written".to_string(),
            ..AdminReply::default()
        })
    }

    /// `watchers.hint` at one node.
    fn hint_node(&self, node: NodeId, h: &Hint) {
        let subs = lock(&self.watchers);
        for sub in subs.iter().filter(|s| s.node == node) {
            if sub.drop_hints || !sub.glob.matches(&h.name) {
                continue;
            }
            if sub.tx.try_send(h.clone()).is_err() {
                sub.rescan.store(true, Ordering::SeqCst);
            }
        }
    }

    /// `handleRefWatch` and `refWatch.run`: the stream ends when the client closes its side, a write
    /// fails, or the cluster closes.
    async fn handle_ref_watch(self: &Arc<Self>, idx: usize, m: Msg, s: Stream, plan: &Plan) {
        let Stream { mut send, recv } = s;
        let glob = match refglob::compile(&m.pattern) {
            Ok(g) => g,
            Err(e) => {
                let _ = wire::write_err(&mut send, wire::CODE_BAD_REQUEST, &e).await;
                send.finish();
                let mut recv = recv;
                recv.cancel_read(0);
                return;
            }
        };
        let me = self.ids[idx];
        let mut known = BTreeMap::new();
        for r in &m.refs {
            if let Some(k) = r.key.as_ref().filter(|k| !k.is_empty())
                && glob.matches(&r.name)
            {
                known.insert(
                    r.name.clone(),
                    RefState {
                        key: Some(k.clone()),
                        version: Ballot::default(),
                    },
                );
            }
        }
        let wctx = self.ctx.with_cancel();
        let (tx, mut rx) = mpsc::channel(HINT_QUEUE);
        let sub = Arc::new(WatchSub {
            node: me,
            glob: glob.clone(),
            tx,
            rescan: AtomicBool::new(false),
            drop_hints: plan.drop_hints,
        });
        lock(&self.watchers).push(sub.clone());
        // The client ends the watch by closing its side of the stream.
        let drain_ctx = wctx.clone();
        let drain = tokio::spawn(async move {
            let mut recv = recv;
            let mut buf = [0u8; 1024];
            loop {
                tokio::select! {
                    r = recv.read(&mut buf) => match r {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    },
                    _ = drain_ctx.done() => break,
                }
            }
            drain_ctx.cancel();
            recv
        });
        let interval = self.lock().watch_reconcile;
        let mut w = Watch {
            shared: self.clone(),
            glob,
            known,
            batch: Msg::default(),
            size: 0,
        };
        w.run(&wctx, &mut send, &mut rx, &sub, interval).await;
        lock(&self.watchers).retain(|s| !Arc::ptr_eq(s, &sub));
        wctx.cancel();
        let recv = drain.await.ok();
        send.finish();
        if let Some(mut recv) = recv {
            recv.cancel_read(0);
        }
    }
}

/// What a watch stream believes its client holds for a name (`refState`).
#[derive(Clone, Debug)]
struct RefState {
    /// `None` once the client was told the name is gone.
    key: Option<Vec<u8>>,
    version: Ballot,
}

/// One watch stream (`refWatch`).
struct Watch {
    shared: Arc<Shared>,
    glob: Glob,
    known: BTreeMap<String, RefState>,
    batch: Msg,
    size: usize,
}

/// A write under the watch's ctx; the ctx ending reads as a failed write.
async fn write_ctx(ctx: &Ctx, send: &mut Box<dyn SendStream>, m: &Msg) -> Result<(), WireError> {
    match ctx.run(wire::write_msg(send, m)).await {
        Ok(r) => r,
        Err(e) => Err(WireError::Io(std::io::Error::other(e.to_string()))),
    }
}

impl Watch {
    async fn run(
        &mut self,
        ctx: &Ctx,
        send: &mut Box<dyn SendStream>,
        rx: &mut mpsc::Receiver<Hint>,
        sub: &WatchSub,
        interval: Duration,
    ) {
        if let Err(e) = self.reconcile(ctx, send).await {
            if ctx.err().is_none() {
                let _ = wire::write_err(send, wire::CODE_UNAVAILABLE, &e.to_string()).await;
            }
            return;
        }
        if self.synced(ctx, send).await.is_err() {
            return;
        }
        let period = interval.max(Duration::from_millis(1));
        let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = ctx.done() => return,
                h = rx.recv() => {
                    let Some(h) = h else { return };
                    self.apply_hint(ctx, send, h).await;
                    while let Ok(h) = rx.try_recv() {
                        self.apply_hint(ctx, send, h).await;
                    }
                    if self.flush(ctx, send).await.is_err() {
                        return;
                    }
                    if sub.rescan.swap(false, Ordering::SeqCst) && self.rescan(ctx, send).await.is_err() {
                        return;
                    }
                }
                _ = ticker.tick() => {
                    if self.rescan(ctx, send).await.is_err() {
                        return;
                    }
                }
            }
        }
    }

    /// `rescan`: a reconcile that fails is retried at the next tick, without the heartbeat.
    async fn rescan(&mut self, ctx: &Ctx, send: &mut Box<dyn SendStream>) -> Result<(), WireError> {
        if self.reconcile(ctx, send).await.is_err() {
            return Ok(());
        }
        self.synced(ctx, send).await
    }

    /// `reconcile`: the difference between the catalog and what the client holds.
    async fn reconcile(
        &mut self,
        ctx: &Ctx,
        send: &mut Box<dyn SendStream>,
    ) -> Result<(), WireError> {
        let prefix = self.glob.prefix();
        let mut seen: HashSet<String> = HashSet::new();
        let mut after = String::new();
        loop {
            let (entries, next) = {
                let st = self.shared.lock();
                st.catalog_list(prefix.as_bytes(), after.as_bytes(), st.ref_page_limit)
            };
            let empty = entries.is_empty();
            for (name, rv) in entries {
                if !self.glob.matches(&name) {
                    continue;
                }
                let Ok(r) = Reference::decode(&rv.record) else {
                    continue;
                };
                seen.insert(name.clone());
                let version = Ballot::parse(&rv.version).unwrap_or_default();
                let st = self.known.get(&name).cloned();
                if let Some(s) = &st
                    && !s.version.is_zero()
                    && version < s.version
                {
                    continue; // a hint got ahead of a lagging row
                }
                self.known.insert(
                    name.clone(),
                    RefState {
                        key: Some(r.key.clone()),
                        version,
                    },
                );
                if st
                    .as_ref()
                    .is_some_and(|s| s.key.as_deref() == Some(r.key.as_slice()))
                {
                    continue;
                }
                let info = RefInfo {
                    name,
                    key: Some(r.key),
                    version: Some(rv.version),
                    created_at: r.created_at,
                    user: r.user,
                };
                self.queue(ctx, send, info).await;
            }
            if next.is_empty() || empty {
                break;
            }
            after = next;
        }
        let gone: Vec<(String, RefState)> = self
            .known
            .iter()
            .filter(|(name, _)| !seen.contains(*name))
            .map(|(name, st)| (name.clone(), st.clone()))
            .collect();
        for (name, st) in gone {
            self.known.remove(&name);
            if st.key.is_some() {
                self.queue_deleted(ctx, send, name).await;
            }
        }
        self.flush(ctx, send).await
    }

    /// `applyHint`: a hint at or below the known version is stale or a duplicate.
    async fn apply_hint(&mut self, ctx: &Ctx, send: &mut Box<dyn SendStream>, h: Hint) {
        let Some(version) = Ballot::parse(&h.version) else {
            return;
        };
        if version.is_zero() {
            return;
        }
        let st = self.known.get(&h.name).cloned();
        if let Some(s) = &st
            && !s.version.is_zero()
            && version <= s.version
        {
            return;
        }
        let Some(record) = h.record else {
            // Remember the deletion so that a delayed older put is ignored.
            self.known
                .insert(h.name.clone(), RefState { key: None, version });
            if st.is_some_and(|s| s.key.is_some()) {
                self.queue_deleted(ctx, send, h.name).await;
            }
            return;
        };
        let Ok(r) = Reference::decode(&record) else {
            return;
        };
        self.known.insert(
            h.name.clone(),
            RefState {
                key: Some(r.key.clone()),
                version,
            },
        );
        if st.is_some_and(|s| s.key.as_deref() == Some(r.key.as_slice())) {
            return;
        }
        let info = RefInfo {
            name: h.name,
            key: Some(r.key),
            version: Some(h.version),
            created_at: r.created_at,
            user: r.user,
        };
        self.queue(ctx, send, info).await;
    }

    async fn queue(&mut self, ctx: &Ctx, send: &mut Box<dyn SendStream>, r: RefInfo) {
        self.size += r.name.len()
            + r.key.as_ref().map_or(0, Vec::len)
            + r.version.as_ref().map_or(0, Vec::len)
            + r.user.len()
            + 16;
        self.batch.refs.push(r);
        if self.size > wire::MAX_PAGE_BYTES {
            let _ = self.flush(ctx, send).await;
        }
    }

    async fn queue_deleted(&mut self, ctx: &Ctx, send: &mut Box<dyn SendStream>, name: String) {
        self.size += name.len() + 8;
        self.batch.deleted.push(name);
        if self.size > wire::MAX_PAGE_BYTES {
            let _ = self.flush(ctx, send).await;
        }
    }

    /// `flush`: the queued changes, if any, as one `ref-changes` frame.
    async fn flush(&mut self, ctx: &Ctx, send: &mut Box<dyn SendStream>) -> Result<(), WireError> {
        if self.batch.refs.is_empty() && self.batch.deleted.is_empty() {
            return Ok(());
        }
        let mut m = std::mem::take(&mut self.batch);
        m.typ = wire::T_REF_CHANGES;
        self.size = 0;
        let m = self.shared.stamp(m);
        write_ctx(ctx, send, &m).await
    }

    /// `synced`: the client holds the current state; doubles as the heartbeat.
    async fn synced(&mut self, ctx: &Ctx, send: &mut Box<dyn SendStream>) -> Result<(), WireError> {
        let m = self.shared.stamp(Msg {
            typ: wire::T_REF_SYNCED,
            ..Msg::default()
        });
        write_ctx(ctx, send, &m).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn must<T, E: std::fmt::Display>(r: Result<T, E>, what: &str) -> T {
        match r {
            Ok(v) => v,
            Err(e) => panic!("{what}: {e}"),
        }
    }

    /// A raw (incompressible) blob record.
    fn blob(seed: u64, len: usize) -> ([u8; 32], Vec<u8>) {
        let data = crate::splitmix::data(seed, len);
        let k = Key::new(Type::Blob, len as u64, &data);
        (
            k.0,
            must(amberpack::encode_record(k, &data), "encode_record"),
        )
    }

    fn raw_record(rec: &[u8]) -> RawRecord {
        RawRecord {
            record: must(amberpack::parse_record(rec), "parse_record"),
            bytes: rec.to_vec(),
        }
    }

    fn reference_record(name: &str, root: &[u8; 32], user: &str, created_at: i64) -> Vec<u8> {
        let r = Reference {
            name: name.to_string(),
            key: root.to_vec(),
            user: user.to_string(),
            created_at,
            ..Reference::default()
        };
        must(r.encode(), "Reference::encode")
    }

    fn state() -> FakeState {
        FakeState::new(View::default(), 3, RNG_SEED)
    }

    #[test]
    fn node_ids_are_the_go_harness_ids() {
        let id = node_id(0);
        assert_eq!(id.0[0], 1);
        assert_eq!(id.0[31], 1);
        assert!(id.0[1..31].iter().all(|b| *b == 0));
        assert_eq!(node_id(2).0[0], 3);
        assert_eq!(mem_addr(&node_id(0)), "mem:01000000");
        assert_eq!(nid_of(&[7; 16]).0[..16], [7; 16]);
        assert_eq!(nid_of(&[7; 40]).0, [7; 32]);
        assert_eq!(default_min_replicas(1), 1);
        assert_eq!(default_min_replicas(2), 2);
        assert_eq!(default_min_replicas(3), 2);
        assert_eq!(default_min_replicas(5), 4);
    }

    #[test]
    fn verify_record_accepts_and_rejects_as_the_node() {
        let (k, rec) = blob(1, 100);
        let (got, bytes) = must(verify_record(&raw_record(&rec)), "verify");
        assert_eq!(got, k);
        assert_eq!(bytes, rec);

        let bad = tamper(&rec);
        let err = match verify_record(&raw_record(&bad)) {
            Ok(_) => panic!("tampered record verified"),
            Err(e) => e,
        };
        assert!(err.starts_with("payload hashes to "), "{err}");
        assert_eq!(err.len(), "payload hashes to ".len() + 64);

        // A Blob key whose length field disagrees with the payload.
        let data = crate::splitmix::data(2, 100);
        let k = Key::new(Type::Blob, 999, &data);
        let rec = must(amberpack::encode_record(k, &data), "encode_record");
        assert_eq!(
            verify_record(&raw_record(&rec)).err().as_deref(),
            Some("length field mismatch")
        );

        // A Commit's length field is its own serialized length, checked like a Blob's (node
        // TestVerifyRecordCommitLength).
        let dir = must(fstree::encode_dir_leaf(&[]), "empty tree");
        let id = amber_store_core::commit::Identity {
            name: "tester".into(),
            when: 1,
            ..Default::default()
        };
        let commit = amber_store_core::commit::Commit {
            tree: dir.key,
            parents: Vec::new(),
            author: id.clone(),
            committer: id,
            message: "m".into(),
            signature: Vec::new(),
            public_key: Vec::new(),
        };
        let (ck, cdata) = must(commit.object(), "commit");
        let rec = must(amberpack::encode_record(ck, &cdata), "encode_record");
        assert!(verify_record(&raw_record(&rec)).is_ok());
        let bad = Key::new(Type::Commit, cdata.len() as u64 + 1, &cdata);
        let rec = must(amberpack::encode_record(bad, &cdata), "encode_record");
        assert_eq!(
            verify_record(&raw_record(&rec)).err().as_deref(),
            Some("length field mismatch")
        );

        // A DirNode key over any payload: no length check, the hash decides.
        let k = Key::new(Type::DirNode, 5, &data);
        let rec = must(amberpack::encode_record(k, &data), "encode_record");
        assert!(verify_record(&raw_record(&rec)).is_ok());
    }

    #[test]
    fn tamper_keeps_the_record_parseable() {
        let (_, rec) = blob(3, 64);
        let bad = tamper(&rec);
        assert_ne!(bad, rec);
        assert!(amberpack::parse_record(&bad).is_ok());
        let (_, empty) = blob(4, 0);
        let bad = tamper(&empty);
        assert!(amberpack::parse_record(&bad).is_ok());
        assert!(verify_record(&raw_record(&bad)).is_err());
    }

    #[test]
    fn cond_of_follows_refs_go() {
        let m = Msg {
            force: true,
            ..Msg::default()
        };
        assert_eq!(
            cond_of(&m),
            Cond {
                force: true,
                ..Cond::default()
            }
        );
        // HasExpected without anything: the name must not exist.
        let m = Msg {
            has_expected: true,
            ..Msg::default()
        };
        assert_eq!(
            cond_of(&m),
            Cond {
                versioned: true,
                ..Cond::default()
            }
        );
        // A record version without an expected version: no condition.
        let m = Msg {
            has_expected: true,
            version: vec![1],
            ..Msg::default()
        };
        assert_eq!(cond_of(&m), Cond::default());
        let m = Msg {
            has_expected: true,
            expected_version: vec![9],
            ..Msg::default()
        };
        assert_eq!(
            cond_of(&m),
            Cond {
                versioned: true,
                expected_version: Some(vec![9]),
                ..Cond::default()
            }
        );
        // ExpectedOld wins over ExpectedVersion.
        let m = Msg {
            has_expected: true,
            expected_version: vec![9],
            expected_old: vec![8],
            ..Msg::default()
        };
        assert_eq!(
            cond_of(&m),
            Cond {
                keyed: true,
                expected_version: Some(vec![9]),
                expected_old: Some(vec![8]),
                ..Cond::default()
            }
        );
        // Without HasExpected the expectations are ignored.
        let m = Msg {
            expected_version: vec![9],
            ..Msg::default()
        };
        assert_eq!(cond_of(&m), Cond::default());
    }

    #[test]
    fn catalog_put_conditions_and_retries() {
        let mut st = state();
        let me = node_id(0);
        let root = blob(5, 10).0;
        let rec_a = reference_record("trees/a", &root, "u", 1);
        let rec_b = reference_record("trees/a", &root, "u", 2);
        let must_not_exist = Cond {
            versioned: true,
            ..Cond::default()
        };
        let v1 = must(
            st.catalog_put(me, "trees/a", &rec_a, &must_not_exist)
                .map_err(|m| format!("{m:?}")),
            "first put",
        );
        assert_eq!(v1.len(), BALLOT_SIZE);
        assert_eq!(Ballot::parse(&v1).map(|b| b.counter), Some(1));
        // A retry of the same record reports the committed version.
        assert_eq!(
            st.catalog_put(me, "trees/a", &rec_a, &must_not_exist),
            Ok(v1.clone())
        );
        // Another record must not replace it.
        let mm = match st.catalog_put(me, "trees/a", &rec_b, &must_not_exist) {
            Ok(_) => panic!("replaced an existing name"),
            Err(mm) => mm,
        };
        assert!(mm.has_current);
        assert_eq!(mm.version, v1);
        assert_eq!(
            mm.current.as_ref().map(|c| c.record.clone()),
            Some(rec_a.clone())
        );
        // The right expected version replaces it.
        let by_version = Cond {
            versioned: true,
            expected_version: Some(v1.clone()),
            ..Cond::default()
        };
        let v2 = must(
            st.catalog_put(me, "trees/a", &rec_b, &by_version)
                .map_err(|m| format!("{m:?}")),
            "versioned put",
        );
        assert!(Ballot::parse(&v2) > Ballot::parse(&v1));
        // Keyed by the current key.
        let keyed = Cond {
            keyed: true,
            expected_old: Some(root.to_vec()),
            ..Cond::default()
        };
        let rec_c = reference_record("trees/a", &root, "u", 3);
        assert!(st.catalog_put(me, "trees/a", &rec_c, &keyed).is_ok());
        let wrong_key = Cond {
            keyed: true,
            expected_old: Some(vec![1; 32]),
            ..Cond::default()
        };
        let rec_d = reference_record("trees/a", &root, "u", 4);
        assert!(st.catalog_put(me, "trees/a", &rec_d, &wrong_key).is_err());
        // Absent with an expected version: mismatch without a current value.
        let mm = match st.catalog_put(me, "trees/new", &rec_d, &by_version) {
            Ok(_) => panic!("versioned put of an absent name"),
            Err(mm) => mm,
        };
        assert!(!mm.has_current);
        assert!(mm.current.is_none());
        assert!(mm.version.is_empty());
    }

    #[test]
    fn catalog_delete_follows_catalog_go() {
        let mut st = state();
        let me = node_id(1);
        let root = blob(6, 10).0;
        let rec = reference_record("trees/x", &root, "", 1);
        let force = Cond {
            force: true,
            ..Cond::default()
        };
        // Absent with force or an expectation: nothing to do, no deletion to announce.
        assert_eq!(st.catalog_delete(me, "trees/x", &force), Ok(None));
        let by_version = Cond {
            versioned: true,
            expected_version: Some(vec![1; 40]),
            ..Cond::default()
        };
        assert_eq!(st.catalog_delete(me, "trees/x", &by_version), Ok(None));
        // Absent with no condition: also nothing.
        assert_eq!(st.catalog_delete(me, "trees/x", &Cond::default()), Ok(None));
        let v = must(
            st.catalog_put(me, "trees/x", &rec, &force)
                .map_err(|m| format!("{m:?}")),
            "put",
        );
        // Present, the wrong version: mismatch.
        assert!(st.catalog_delete(me, "trees/x", &by_version).is_err());
        // Present, must-not-exist: mismatch.
        let must_not_exist = Cond {
            versioned: true,
            ..Cond::default()
        };
        assert!(st.catalog_delete(me, "trees/x", &must_not_exist).is_err());
        let right = Cond {
            versioned: true,
            expected_version: Some(v),
            ..Cond::default()
        };
        let deleted = must(
            st.catalog_delete(me, "trees/x", &right)
                .map_err(|m| format!("{m:?}")),
            "delete",
        );
        assert!(deleted.is_some());
        assert!(!st.catalog.contains_key("trees/x"));
    }

    #[test]
    fn catalog_list_pages_like_the_acceptor_scan() {
        let mut st = state();
        let me = node_id(0);
        let root = blob(7, 10).0;
        let force = Cond {
            force: true,
            ..Cond::default()
        };
        for name in ["other/x", "trees/a", "trees/b", "trees/c", "trees0", "u"] {
            let rec = reference_record(name, &root, "", 1);
            assert!(st.catalog_put(me, name, &rec, &force).is_ok());
        }
        let names = |entries: &[(String, RefValue)]| -> Vec<String> {
            entries.iter().map(|(n, _)| n.clone()).collect()
        };
        let (e, next) = st.catalog_list(b"trees/", b"", 2);
        assert_eq!(names(&e), ["trees/a", "trees/b"]);
        assert_eq!(next, "trees/b");
        let (e, next) = st.catalog_list(b"trees/", next.as_bytes(), 2);
        assert_eq!(names(&e), ["trees/c"]);
        assert_eq!(next, "");
        // Exactly a full page with nothing after it: no next.
        let (e, next) = st.catalog_list(b"trees/", b"trees/a", 2);
        assert_eq!(names(&e), ["trees/b", "trees/c"]);
        assert_eq!(next, "");
        // The empty prefix lists everything; limit 0 is the acceptor's 100000.
        let (e, next) = st.catalog_list(b"", b"", 0);
        assert_eq!(e.len(), 6);
        assert_eq!(next, "");
        // An `after` below the prefix starts the scan there (the acceptor's lower bound).
        let (e, _) = st.catalog_list(b"trees/", b"other", 10);
        assert_eq!(names(&e), ["other/x", "trees/a", "trees/b", "trees/c"]);
    }

    #[test]
    fn ballots_order_by_counter_then_proposer() {
        let a = Ballot {
            counter: 1,
            proposer: [9; 32],
        };
        let b = Ballot {
            counter: 2,
            proposer: [0; 32],
        };
        assert!(a < b);
        assert!(a.bytes() < b.bytes());
        assert_eq!(Ballot::parse(&a.bytes()), Some(a));
        assert_eq!(Ballot::parse(&[]), Some(Ballot::default()));
        assert!(Ballot::parse(&[]).is_some_and(|b| b.is_zero()));
        assert_eq!(Ballot::parse(&[1; 8]), None);
    }

    #[test]
    fn ref_value_len_is_the_cbor_size() {
        let rv = RefValue {
            record: vec![0; 30],
            version: vec![0; 40],
        };
        // a2 00 58 1e <30> 02 58 28 <40>
        assert_eq!(ref_value_len(&rv), 1 + 1 + 2 + 30 + 1 + 2 + 40);
        assert_eq!(cbor_head_len(23), 1);
        assert_eq!(cbor_head_len(24), 2);
        assert_eq!(cbor_head_len(65536), 5);
    }

    #[test]
    fn texts_follow_the_go_node() {
        let mut v = View::default();
        assert_eq!(transition_text(&v), "idle");
        v.ramps = vec![dstore_view::Ramp::default(); 2];
        assert_eq!(transition_text(&v), "idle (2 ramp(s) pending)");
        assert_eq!(gc_text(false), "epoch 0 idle");
        assert_eq!(gc_text(true), "epoch 0 idle, ON HOLD");
    }

    #[test]
    fn check_epoch_orders_incarnation_then_epoch() {
        let v = View {
            cluster_id: Some(vec![1, 2]),
            incarnation: 2,
            epoch: 5,
            ..View::default()
        };
        let req = |cid: &[u8], inc, epoch| Msg {
            cluster_id: cid.to_vec(),
            incarnation: inc,
            epoch,
            ..Msg::default()
        };
        assert!(check_epoch(&v, &req(&[1, 2], 2, 5)).is_ok());
        assert!(check_epoch(&v, &req(&[], 2, 5)).is_ok());
        assert!(matches!(
            check_epoch(&v, &req(&[1, 2], 2, 4)),
            Err(EpochErr::Stale)
        ));
        assert!(matches!(
            check_epoch(&v, &req(&[1, 2], 1, 9)),
            Err(EpochErr::Stale)
        ));
        assert!(matches!(
            check_epoch(&v, &req(&[1, 2], 2, 6)),
            Err(EpochErr::Other("epoch above the catalog's"))
        ));
        assert!(matches!(
            check_epoch(&v, &req(&[3], 2, 5)),
            Err(EpochErr::Other("wrong cluster"))
        ));
    }

    #[test]
    fn local_missing_pins_present_keys() {
        let mut node = NodeState::default();
        let (k1, r1) = blob(8, 10);
        let (k2, _) = blob(9, 10);
        store_record(&mut node, k1, &r1);
        let (lacking, present) = local_missing(&mut node, &[k2, k1, k1], false);
        assert_eq!(lacking, [k2]);
        assert_eq!(present, [k1, k1]);
        assert!(node.pins.is_empty());
        let _ = local_missing(&mut node, &[k1], true);
        assert!(node.pins.contains(&k1));
        // A dedup hit pins as well.
        let mut node = NodeState::default();
        store_record(&mut node, k1, &r1);
        store_record(&mut node, k1, &r1);
        assert_eq!(node.store.len(), 1);
        assert!(node.pins.contains(&k1));
    }

    /// Over the in-memory network: these need the sibling modules the handlers call.
    mod over_mem {
        use super::*;
        use dstore_view::Placement;

        const CLIENT: u8 = 100;

        struct Client {
            ep: Arc<MemEndpoint>,
            ctx: Ctx,
        }

        fn client(net: &Arc<Network>) -> Client {
            let mut id = [0u8; 32];
            id[0] = CLIENT;
            id[31] = CLIENT;
            Client {
                ep: net.bind(NodeId(id), &[wire::ALPN_CLIENT]),
                ctx: Ctx::background().with_timeout(Duration::from_secs(20)),
            }
        }

        impl Client {
            async fn open(&self, to: NodeId) -> (Arc<dyn Conn>, Stream) {
                let conn = must(
                    self.ep
                        .dial(&self.ctx, to, vec![mem_addr(&to)], wire::ALPN_CLIENT)
                        .await,
                    "dial",
                );
                let s = must(conn.open_stream(&self.ctx).await, "open_stream");
                (conn, s)
            }

            async fn call(&self, to: NodeId, req: &Msg) -> Msg {
                let (_conn, mut s) = self.open(to).await;
                must(wire::write_msg(&mut s.send, req).await, "write_msg");
                s.send.finish();
                must(wire::read_msg(&mut s.recv).await, "read_msg")
            }

            /// A put as `putBatch` sends it. A node that refuses the batch (stale view, no space) answers
            /// before reading the pack and resets the stream, so writing the pack may fail; the reply is
            /// read either way.
            async fn put(&self, to: NodeId, req: &Msg, records: &[Vec<u8>]) -> Msg {
                let (_conn, mut s) = self.open(to).await;
                must(wire::write_msg(&mut s.send, req).await, "write_msg");
                let mut sender = PackSender::new(&mut s.send);
                let mut sent = true;
                for r in records {
                    if sender.add_record(r).await.is_err() {
                        sent = false;
                        break;
                    }
                }
                if sent {
                    let _ = sender.finish().await;
                }
                s.send.finish();
                must(wire::read_msg(&mut s.recv).await, "read_msg")
            }

            async fn get(&self, to: NodeId, keys: &[[u8; 32]]) -> (Msg, Vec<RawRecord>) {
                let (_conn, mut s) = self.open(to).await;
                let req = Msg {
                    typ: wire::T_GET,
                    keys: wire::raw_keys(keys),
                    ..Msg::default()
                };
                must(wire::write_msg(&mut s.send, &req).await, "write_msg");
                s.send.finish();
                let absent = must(wire::read_msg(&mut s.recv).await, "read absent");
                let mut records = PackRecords::new(PackReader::new(&mut s.recv));
                let mut out = Vec::new();
                while let Some(r) = records.next().await {
                    out.push(must(r, "pack record"));
                }
                (absent, out)
            }
        }

        fn stamped(v: &View, typ: i64) -> Msg {
            Msg {
                typ,
                cluster_id: v.cluster_id.clone().unwrap_or_default(),
                incarnation: v.incarnation,
                epoch: v.epoch,
                ..Msg::default()
            }
        }

        async fn cluster(nodes: usize, replicas: u8) -> (Arc<Network>, Arc<FakeCluster>, Client) {
            let net = Network::new();
            let fc = FakeCluster::start(
                &net,
                FakeClusterConfig {
                    nodes,
                    replicas,
                    ..FakeClusterConfig::default()
                },
            )
            .await;
            let c = client(&net);
            (net, fc, c)
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn view_ping_unknown_and_transcript() {
            let (_net, fc, c) = cluster(3, 3).await;
            let n1 = fc.ids()[0];
            let v = fc.view();
            let reply = c
                .call(
                    n1,
                    &Msg {
                        typ: wire::T_VIEW,
                        ..Msg::default()
                    },
                )
                .await;
            assert_eq!(reply.typ, wire::T_VIEW_REPLY);
            assert_eq!(reply.view, dstore_codec::marshal(&v));
            assert_eq!((reply.incarnation, reply.epoch), (1, 1));
            let pong = c
                .call(
                    n1,
                    &Msg {
                        typ: wire::T_PING,
                        ..Msg::default()
                    },
                )
                .await;
            assert_eq!(pong.typ, wire::T_PONG);
            let bad = c
                .call(
                    n1,
                    &Msg {
                        typ: 99,
                        ..Msg::default()
                    },
                )
                .await;
            assert_eq!(
                (bad.typ, bad.code.as_str(), bad.text.as_str()),
                (wire::T_ERR, "bad-request", "unknown operation")
            );
            let transcript: Vec<i64> = fc.requests(n1).iter().map(|m| m.typ).collect();
            assert_eq!(transcript, [wire::T_VIEW, wire::T_PING, 99]);
            assert!(fc.requests(fc.ids()[1]).is_empty());
            let t = fc.ticket();
            assert_eq!(t.members().len(), 3);
            fc.close().await;
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn put_replicates_and_missing_negotiates() {
            let (net, fc, c) = cluster(3, 3).await;
            let ids = fc.ids();
            let v = fc.view();
            let blobs: Vec<([u8; 32], Vec<u8>)> = (0..5).map(|i| blob(100 + i, 300)).collect();
            let recs: Vec<Vec<u8>> = blobs.iter().map(|(_, r)| r.clone()).collect();
            let reply = c.put(ids[0], &stamped(&v, wire::T_PUT), &recs).await;
            assert_eq!(reply.typ, wire::T_PUT_RESULT, "{reply:?}");
            assert!(reply.failed.is_empty() && reply.rejected.is_empty());
            assert_eq!(reply.holders.len(), 5);
            for h in &reply.holders {
                assert_eq!(h.holders.len(), 3);
                assert_eq!(h.holders[0], ids[0].0.to_vec());
            }
            for id in &ids {
                assert_eq!(fc.stored(*id).len(), 5);
            }
            // Missing at node 2: the absent key is lacking; the held keys are fully placed.
            let (absent, _) = blob(200, 10);
            let mut req = stamped(&v, wire::T_MISSING);
            req.keys = wire::raw_keys(&[blobs[0].0, absent]);
            req.pin = true;
            let reply = c.call(ids[1], &req).await;
            assert_eq!(reply.typ, wire::T_MISSING_REPLY);
            assert_eq!(reply.keys, [absent.to_vec()]);
            assert!(reply.short.is_empty(), "{:?}", reply.short);

            // Node 3 down: its forward fails, and missing reports the key short.
            net.set_down(ids[2], true);
            let (k, rec) = blob(300, 300);
            let reply = c.put(ids[0], &stamped(&v, wire::T_PUT), &[rec]).await;
            assert_eq!(reply.holders.len(), 1);
            assert_eq!(
                reply.holders[0].holders,
                [ids[0].0.to_vec(), ids[1].0.to_vec()]
            );
            assert_eq!(reply.failed.len(), 1);
            assert_eq!(reply.failed[0].reason, "unreachable");
            assert_eq!(reply.failed[0].node, Some(ids[2].0.to_vec()));
            assert!((500..2000).contains(&reply.failed[0].retry_after));
            let mut req = stamped(&v, wire::T_MISSING);
            req.keys = wire::raw_keys(&[k]);
            let reply = c.call(ids[0], &req).await;
            assert!(reply.keys.is_empty());
            assert_eq!(reply.short.len(), 1);
            assert_eq!(
                reply.short[0].holders,
                [ids[0].0.to_vec(), ids[1].0.to_vec()]
            );
            // The failed member is reported unreachable in the view reply.
            let view = c
                .call(
                    ids[0],
                    &Msg {
                        typ: wire::T_VIEW,
                        ..Msg::default()
                    },
                )
                .await;
            assert_eq!(view.unreachable, [ids[2].0.to_vec()]);
            net.set_down(ids[2], false);
            fc.close().await;
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn put_refusals() {
            let (_net, fc, c) = cluster(3, 1).await;
            let ids = fc.ids();
            let v = fc.view();
            // Stale epoch.
            fc.bump_epoch();
            let (_, rec) = blob(400, 100);
            let reply = c
                .put(
                    ids[0],
                    &stamped(&v, wire::T_PUT),
                    std::slice::from_ref(&rec),
                )
                .await;
            assert_eq!(
                (reply.code.as_str(), reply.text.as_str()),
                ("stale-view", "request epoch is behind")
            );
            assert_eq!(reply.epoch, 2);
            assert_eq!(reply.view, dstore_codec::marshal(&fc.view()));
            let v = fc.view();
            // Not writable.
            fc.set_writable(ids[0], false);
            let reply = c
                .put(
                    ids[0],
                    &stamped(&v, wire::T_PUT),
                    std::slice::from_ref(&rec),
                )
                .await;
            assert_eq!(
                (reply.code.as_str(), reply.text.as_str()),
                ("no-space", "node below its free-space reserve")
            );
            fc.set_writable(ids[0], true);
            // R = 1: find a key another node owns, plus a tampered record.
            let pl = Placement::new(Arc::new(fc.view()));
            let foreign = (0..1000u64)
                .map(|i| blob(1000 + i, 50))
                .find(|(k, _)| !pl.in_write_set(k, &ids[0]));
            let Some((fk, frec)) = foreign else {
                panic!("no foreign key")
            };
            let own = (0..1000u64)
                .map(|i| blob(5000 + i, 50))
                .find(|(k, _)| pl.in_write_set(k, &ids[0]));
            let Some((ok, orec)) = own else {
                panic!("no own key")
            };
            let reply = c
                .put(
                    ids[0],
                    &stamped(&v, wire::T_PUT),
                    &[frec, tamper(&orec), orec.clone(), orec],
                )
                .await;
            assert_eq!(reply.typ, wire::T_PUT_RESULT);
            assert_eq!(reply.rejected.len(), 2);
            assert_eq!(reply.rejected[0].key, Some(fk.to_vec()));
            assert_eq!(reply.rejected[0].reason, "not-owner");
            assert!(
                reply.rejected[1]
                    .reason
                    .starts_with("verify: payload hashes to ")
            );
            assert_eq!(reply.holders.len(), 1);
            assert_eq!(reply.holders[0].key, Some(ok.to_vec()));
            fc.close().await;
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn get_absent_and_corrupt() {
            let (_net, fc, c) = cluster(1, 1).await;
            let n1 = fc.ids()[0];
            let v = fc.view();
            let (k, rec) = blob(600, 200);
            let (absent, _) = blob(601, 10);
            let reply = c
                .put(n1, &stamped(&v, wire::T_PUT), std::slice::from_ref(&rec))
                .await;
            assert_eq!(reply.typ, wire::T_PUT_RESULT);
            let (abs, records) = c.get(n1, &[absent, k]).await;
            assert_eq!(abs.typ, wire::T_ABSENT);
            assert_eq!(abs.keys, [absent.to_vec()]);
            assert_eq!(records.len(), 1);
            assert_eq!(records[0].bytes, rec);
            fc.inject(n1, wire::T_GET, 2, Injection::CorruptRecord(k));
            let (_, records) = c.get(n1, &[k]).await;
            assert_eq!(records.len(), 1);
            assert!(verify_record(&records[0]).is_err());
            let (_, records) = c.get(n1, &[k]).await;
            assert!(verify_record(&records[0]).is_ok());
            fc.close().await;
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn references() {
            let (_net, fc, c) = cluster(3, 3).await;
            let ids = fc.ids();
            let v = fc.view();
            let (root, rec) = blob(700, 100);
            // Incomplete: the root is not stored.
            let mut req = stamped(&v, wire::T_REF_PUT);
            req.name = "trees/a".to_string();
            req.record = reference_record("trees/a", &root, "tester", 1);
            let reply = c.call(ids[0], &req).await;
            assert_eq!(reply.typ, wire::T_INCOMPLETE);
            assert_eq!(reply.keys, [root.to_vec()]);
            assert_eq!(reply.shortfall, 1);
            // Store it; the ref-put commits.
            let _ = c.put(ids[0], &stamped(&v, wire::T_PUT), &[rec]).await;
            let reply = c.call(ids[0], &req).await;
            assert_eq!(reply.typ, wire::T_OK, "{reply:?}");
            assert_eq!(reply.key, root.to_vec());
            let version = reply.version.clone();
            // Must-not-exist: mismatch with the current record.
            let mut again = req.clone();
            again.record = reference_record("trees/a", &root, "tester", 2);
            again.has_expected = true;
            let reply = c.call(ids[1], &again).await;
            assert_eq!(reply.typ, wire::T_CAS_MISMATCH);
            assert!(reply.has_current);
            assert_eq!(reply.version, version);
            assert_eq!(reply.current, root.to_vec());
            // A frame name that differs.
            let mut other = req.clone();
            other.name = "trees/b".to_string();
            let reply = c.call(ids[0], &other).await;
            assert_eq!(reply.text, "frame name differs from the record's");
            // ref-get.
            let get = Msg {
                typ: wire::T_REF_GET,
                name: "trees/a".to_string(),
                ..Msg::default()
            };
            let reply = c.call(ids[2], &get).await;
            assert_eq!((reply.typ, reply.version.clone()), (wire::T_REF, version));
            let bad = Msg {
                typ: wire::T_REF_GET,
                name: "bad@@name".to_string(),
                ..Msg::default()
            };
            let reply = c.call(ids[2], &bad).await;
            assert_eq!(
                (reply.code.as_str(), reply.text.as_str()),
                ("bad-request", "reference name must not contain '@'")
            );
            // ref-list pages.
            for name in ["trees/b", "trees/c", "trees/d", "other/x"] {
                let mut r = stamped(&v, wire::T_REF_PUT);
                r.record = reference_record(name, &root, "", 1);
                assert_eq!(c.call(ids[0], &r).await.typ, wire::T_OK);
            }
            fc.set_ref_page_limit(2);
            let mut after = Vec::new();
            let mut names = Vec::new();
            loop {
                let list = Msg {
                    typ: wire::T_REF_LIST,
                    prefix: b"trees/".to_vec(),
                    after: after.clone(),
                    ..Msg::default()
                };
                let reply = c.call(ids[0], &list).await;
                assert_eq!(reply.typ, wire::T_REFS);
                names.extend(reply.refs.iter().map(|r| r.name.clone()));
                if reply.next.is_empty() || reply.refs.is_empty() {
                    break;
                }
                after = reply.next;
            }
            assert_eq!(names, ["trees/a", "trees/b", "trees/c", "trees/d"]);
            // ref-delete, then the name is unknown.
            let del = Msg {
                typ: wire::T_REF_DELETE,
                name: "trees/a".to_string(),
                force: true,
                ..Msg::default()
            };
            assert_eq!(c.call(ids[1], &del).await.typ, wire::T_OK);
            let reply = c.call(ids[2], &get).await;
            assert_eq!(
                (reply.code.as_str(), reply.text.as_str()),
                ("unknown-ref", "no such reference")
            );
            fc.close().await;
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn watch() {
            let (_net, fc, c) = cluster(3, 3).await;
            let ids = fc.ids();
            let v = fc.view();
            let (root, rec) = blob(800, 100);
            let _ = c.put(ids[0], &stamped(&v, wire::T_PUT), &[rec]).await;
            for name in ["trees/a", "trees/b", "other/x"] {
                assert!(
                    fc.ref_put_local(ids[0], &reference_record(name, &root, "tester", 1))
                        .await
                        .is_ok()
                );
            }
            // A bad pattern.
            let bad = Msg {
                typ: wire::T_REF_WATCH,
                pattern: "trees/[".to_string(),
                ..Msg::default()
            };
            let reply = c.call(ids[1], &bad).await;
            assert_eq!(
                (reply.code.as_str(), reply.text.as_str()),
                ("bad-request", "refglob: unterminated character class")
            );
            // The initial difference, then synced.
            let (_conn, mut s) = c.open(ids[1]).await;
            let mut req = stamped(&v, wire::T_REF_WATCH);
            req.pattern = "trees/**".to_string();
            req.refs = vec![
                RefInfo {
                    name: "trees/a".to_string(),
                    key: Some(vec![1; 32]),
                    ..RefInfo::default()
                },
                RefInfo {
                    name: "trees/gone".to_string(),
                    key: Some(vec![2; 32]),
                    ..RefInfo::default()
                },
            ];
            must(wire::write_msg(&mut s.send, &req).await, "write watch");
            let changes = must(wire::read_msg(&mut s.recv).await, "changes");
            assert_eq!(changes.typ, wire::T_REF_CHANGES);
            let names: Vec<&str> = changes.refs.iter().map(|r| r.name.as_str()).collect();
            assert_eq!(names, ["trees/a", "trees/b"]);
            assert_eq!(changes.deleted, ["trees/gone"]);
            let synced = must(wire::read_msg(&mut s.recv).await, "synced");
            assert_eq!(synced.typ, wire::T_REF_SYNCED);
            // A write coordinated elsewhere arrives by hint.
            assert!(
                fc.ref_put_local(ids[2], &reference_record("trees/c", &root, "tester2", 2))
                    .await
                    .is_ok()
            );
            let changes = must(wire::read_msg(&mut s.recv).await, "hinted change");
            assert_eq!(changes.refs.len(), 1);
            assert_eq!(changes.refs[0].name, "trees/c");
            assert_eq!(changes.refs[0].user, "tester2");
            s.send.finish();
            // A stream that drops hints learns by its reconcile.
            fc.set_watch_reconcile(Duration::from_millis(200));
            fc.inject(ids[1], wire::T_REF_WATCH, 0, Injection::DropHints);
            let (_conn, mut s) = c.open(ids[1]).await;
            let mut req = stamped(&v, wire::T_REF_WATCH);
            req.pattern = "trees/*".to_string();
            must(wire::write_msg(&mut s.send, &req).await, "write watch");
            let first = must(wire::read_msg(&mut s.recv).await, "initial");
            assert_eq!(first.refs.len(), 3);
            let synced = must(wire::read_msg(&mut s.recv).await, "synced");
            assert_eq!(synced.typ, wire::T_REF_SYNCED);
            assert!(
                fc.ref_put_local(ids[0], &reference_record("trees/d", &root, "tester2", 3))
                    .await
                    .is_ok()
            );
            let changes = must(wire::read_msg(&mut s.recv).await, "reconciled change");
            assert_eq!(changes.typ, wire::T_REF_CHANGES);
            assert_eq!(changes.refs[0].name, "trees/d");
            let synced = must(wire::read_msg(&mut s.recv).await, "heartbeat");
            assert_eq!(synced.typ, wire::T_REF_SYNCED);
            s.send.finish();
            fc.close().await;
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn injections() {
            let (_net, fc, c) = cluster(1, 1).await;
            let n1 = fc.ids()[0];
            let ping = Msg {
                typ: wire::T_PING,
                ..Msg::default()
            };
            fc.inject(
                n1,
                wire::T_PING,
                2,
                Injection::Err {
                    code: "busy".to_string(),
                    text: "try later".to_string(),
                    with_view: true,
                    retry_after_ms: 50,
                },
            );
            assert_eq!(c.call(n1, &ping).await.typ, wire::T_PONG);
            let reply = c.call(n1, &ping).await;
            assert_eq!(
                (
                    reply.typ,
                    reply.code.as_str(),
                    reply.text.as_str(),
                    reply.retry_after
                ),
                (wire::T_ERR, "busy", "try later", 50)
            );
            assert_eq!(reply.view, dstore_codec::marshal(&fc.view()));
            assert_eq!(c.call(n1, &ping).await.typ, wire::T_PONG);
            fc.inject(
                n1,
                wire::T_PING,
                0,
                Injection::Delay(Duration::from_millis(100)),
            );
            let t = tokio::time::Instant::now();
            assert_eq!(c.call(n1, &ping).await.typ, wire::T_PONG);
            assert!(t.elapsed() >= Duration::from_millis(100));
            fc.inject(n1, wire::T_VIEW, 1, Injection::CloseBeforeReply);
            let (conn, mut s) = c.open(n1).await;
            must(
                wire::write_msg(
                    &mut s.send,
                    &Msg {
                        typ: wire::T_VIEW,
                        ..Msg::default()
                    },
                )
                .await,
                "write",
            );
            s.send.finish();
            assert!(wire::read_msg(&mut s.recv).await.is_err());
            conn.closed().await;
            fc.close().await;
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn admin_and_status() {
            let (_net, fc, c) = cluster(3, 3).await;
            let n1 = fc.ids()[0];
            let admin = |op: &str| Msg {
                typ: wire::T_ADMIN,
                params: dstore_codec::marshal(&AdminRequest {
                    op: op.to_string(),
                    ..AdminRequest::default()
                }),
                ..Msg::default()
            };
            let reply = c.call(n1, &admin("nope")).await;
            assert_eq!(
                (reply.code.as_str(), reply.text.as_str()),
                ("bad-request", "unknown admin op nope")
            );
            let reply = c.call(n1, &admin("token-create")).await;
            assert_eq!(reply.typ, wire::T_ADMIN_REPLY);
            let ar = must(
                wire::decode_admin_reply(&reply.status),
                "decode admin reply",
            );
            assert_eq!(ar.token.len(), 32);
            assert_eq!(ar.text, hex::encode(&ar.token));
            let reply = c.call(n1, &admin("node-drain")).await;
            assert_eq!(
                (reply.code.as_str(), reply.text.as_str()),
                ("unavailable", "node id must be 32 bytes")
            );
            let bad = Msg {
                typ: wire::T_ADMIN,
                params: vec![0xff],
                ..Msg::default()
            };
            assert_eq!(c.call(n1, &bad).await.code, "bad-request");
            let reply = c
                .call(
                    n1,
                    &Msg {
                        typ: wire::T_STATUS,
                        ..Msg::default()
                    },
                )
                .await;
            assert_eq!(reply.typ, wire::T_STATUS_REPLY);
            let st = must(wire::decode_status(&reply.status), "decode status");
            assert_eq!(st.id, Some(n1.0.to_vec()));
            assert!(st.writable);
            assert_eq!(st.transition, "idle");
            fc.close().await;
        }

        fn admin_msg(req: AdminRequest) -> Msg {
            Msg {
                typ: wire::T_ADMIN,
                params: dstore_codec::marshal(&req),
                ..Msg::default()
            }
        }

        fn op(name: &str) -> AdminRequest {
            AdminRequest {
                op: name.to_string(),
                ..AdminRequest::default()
            }
        }

        /// CBOR head of a byte string of `n` bytes, 24 ≤ n < 256.
        fn bstr_head(n: usize) -> Vec<u8> {
            assert!((24..256).contains(&n), "{n}");
            vec![0x58, n as u8]
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn admin_checks_gc_state_and_catalog_backup() {
            let (_net, fc, c) = cluster(3, 3).await;
            let ids = fc.ids();
            let v = fc.view();
            let reply_of = |m: &Msg| must(wire::decode_admin_reply(&m.status), "admin reply");

            // gc-status: the zero GCState, then ON HOLD.
            let reply = c.call(ids[0], &admin_msg(op("gc-status"))).await;
            let ar = reply_of(&reply);
            assert_eq!(ar.gc, [0xa2, 0x00, 0x00, 0x01, 0x00]);
            assert_eq!(ar.text, "epoch 0 idle");
            let hold = AdminRequest {
                pause: true,
                ..op("gc-hold")
            };
            assert_eq!(reply_of(&c.call(ids[0], &admin_msg(hold)).await).text, "ok");
            let ar = reply_of(&c.call(ids[1], &admin_msg(op("gc-run"))).await);
            assert_eq!(ar.gc, [0xa3, 0x00, 0x00, 0x01, 0x00, 0x09, 0xf5]);
            assert_eq!(ar.text, "epoch 0 idle, ON HOLD");

            // The admin checks a client can reach.
            let stranger = AdminRequest {
                node: vec![9; 32],
                ..op("node-drain")
            };
            let reply = c.call(ids[0], &admin_msg(stranger)).await;
            assert_eq!(
                (reply.code.as_str(), reply.text.as_str()),
                ("unavailable", "09090909 is not a member")
            );
            let member = AdminRequest {
                node: ids[2].0.to_vec(),
                weight: 50,
                ..op("node-weight")
            };
            let ar = reply_of(&c.call(ids[0], &admin_msg(member)).await);
            assert_eq!(ar.text, "transition proposed at epoch 1");
            let reply = c.call(ids[0], &admin_msg(op("replicas"))).await;
            assert_eq!(
                (reply.code.as_str(), reply.text.as_str()),
                ("unavailable", "replicas must be ≥ 1")
            );
            let reply = c.call(ids[0], &admin_msg(op("transition-refreeze"))).await;
            assert_eq!(
                (reply.code.as_str(), reply.text.as_str()),
                ("unavailable", "no frozen transition")
            );

            // catalog-backup: Go's blob of every reference, committed as dstore/catalog-backup.
            let (root, rec) = blob(900, 100);
            let _ = c.put(ids[0], &stamped(&v, wire::T_PUT), &[rec]).await;
            let trees_a = reference_record("trees/a", &root, "tester", 7);
            assert!(fc.ref_put_local(ids[0], &trees_a).await.is_ok());
            let reply = c.call(ids[1], &admin_msg(op("catalog-backup"))).await;
            assert_eq!(reply.typ, wire::T_ADMIN_REPLY, "{reply:?}");
            let ar = reply_of(&reply);
            assert_eq!(ar.text, "backup written");
            let Ok(key) = <[u8; 32]>::try_from(ar.key.as_slice()) else {
                panic!("backup key of {} bytes", ar.key.len())
            };
            let mut want = vec![0x81, 0xa2, 0x00, 0x67];
            want.extend_from_slice(b"trees/a");
            want.push(0x01);
            want.extend_from_slice(&bstr_head(trees_a.len()));
            want.extend_from_slice(&trees_a);
            for id in &ids {
                let stored = fc.stored(*id);
                let Some(r) = stored.get(&key) else {
                    panic!("backup not stored at {}", short_id(id))
                };
                assert_eq!(decode_record(r), Some(want.clone()));
            }
            let get = Msg {
                typ: wire::T_REF_GET,
                name: BACKUP_NAME.to_string(),
                ..Msg::default()
            };
            let reply = c.call(ids[2], &get).await;
            assert_eq!(reply.typ, wire::T_REF, "{reply:?}");
            let r = must(Reference::decode(&reply.record), "backup reference");
            assert_eq!((r.key.as_slice(), r.user.as_str()), (&key[..], "dstore"));
            let ar = reply_of(&c.call(ids[0], &admin_msg(op("catalog-backups"))).await);
            assert_eq!(ar.names, [hex::encode(key)]);
            // A second backup leaves the backup reference out of the blob.
            let ar = reply_of(&c.call(ids[0], &admin_msg(op("catalog-backup"))).await);
            assert_eq!(ar.key, key.to_vec());
            fc.close().await;
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn close_before_reply_keeps_the_commit_and_the_retry_succeeds() {
            let (_net, fc, c) = cluster(1, 1).await;
            let n1 = fc.ids()[0];
            let v = fc.view();
            let (root, rec) = blob(910, 100);
            let _ = c.put(n1, &stamped(&v, wire::T_PUT), &[rec]).await;
            let mut req = stamped(&v, wire::T_REF_PUT);
            req.record = reference_record("trees/lost", &root, "tester", 1);
            req.has_expected = true;
            fc.inject(n1, wire::T_REF_PUT, 1, Injection::CloseBeforeReply);
            let (conn, mut s) = c.open(n1).await;
            must(wire::write_msg(&mut s.send, &req).await, "write ref-put");
            s.send.finish();
            assert!(wire::read_msg(&mut s.recv).await.is_err());
            conn.closed().await;
            let get = Msg {
                typ: wire::T_REF_GET,
                name: "trees/lost".to_string(),
                ..Msg::default()
            };
            let committed = c.call(n1, &get).await;
            assert_eq!(committed.typ, wire::T_REF);
            // The retry finds its own record: success at the committed version.
            let retry = c.call(n1, &req).await;
            assert_eq!(retry.typ, wire::T_OK, "{retry:?}");
            assert_eq!(retry.version, committed.version);
            assert_eq!(fc.requests(n1).len(), 4);
            fc.close().await;
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn unreachable_lists_only_the_members_a_node_called() {
            let (net, fc, c) = cluster(5, 2).await;
            let ids = fc.ids();
            let v = fc.view();
            let pl = Placement::new(Arc::new(v.clone()));
            let down = ids[4];
            let pick = |seed: u64, with_down: bool| {
                (0..2000u64).map(|i| blob(seed + i, 64)).find(|(k, _)| {
                    pl.in_write_set(k, &ids[0]) && pl.in_write_set(k, &down) == with_down
                })
            };
            let Some((k1, r1)) = pick(20_000, false) else {
                panic!("no key owned by n1 without n5")
            };
            let Some((_, r2)) = pick(30_000, true) else {
                panic!("no key owned by n1 and n5")
            };
            net.set_down(down, true);
            let view_req = Msg {
                typ: wire::T_VIEW,
                ..Msg::default()
            };
            // A put and a reference commit that never call n5: no unreachable member, although the
            // completeness walk probes and the ref-changed broadcast dials every member.
            let reply = c.put(ids[0], &stamped(&v, wire::T_PUT), &[r1]).await;
            assert!(reply.failed.is_empty(), "{reply:?}");
            let mut req = stamped(&v, wire::T_REF_PUT);
            req.record = reference_record("trees/a", &k1, "", 1);
            assert_eq!(c.call(ids[0], &req).await.typ, wire::T_OK);
            tokio::time::sleep(Duration::from_millis(50)).await;
            assert!(c.call(ids[0], &view_req).await.unreachable.is_empty());
            // A forward to n5 fails: n5 is listed.
            let reply = c.put(ids[0], &stamped(&v, wire::T_PUT), &[r2]).await;
            assert_eq!(reply.failed.len(), 1);
            assert_eq!(
                c.call(ids[0], &view_req).await.unreachable,
                [down.0.to_vec()]
            );
            net.set_down(down, false);
            fc.close().await;
        }

        /// node/watch_test.go TestClusterWatchLostHint at the frame level: the coordinator cannot reach
        /// the serving node, so the hint is lost and the reconcile delivers the change.
        #[tokio::test(flavor = "multi_thread")]
        async fn watch_lost_hint_by_partition() {
            let (net, fc, c) = cluster(3, 3).await;
            let ids = fc.ids();
            let v = fc.view();
            let (root, rec) = blob(920, 100);
            let _ = c.put(ids[0], &stamped(&v, wire::T_PUT), &[rec]).await;
            assert!(
                fc.ref_put_local(ids[0], &reference_record("trees/a", &root, "tester", 1))
                    .await
                    .is_ok()
            );
            fc.set_watch_reconcile(Duration::from_millis(300));
            let serving = ids[1];
            let coord = ids[2];
            let (_conn, mut s) = c.open(serving).await;
            let mut req = stamped(&v, wire::T_REF_WATCH);
            req.pattern = "trees/*".to_string();
            must(wire::write_msg(&mut s.send, &req).await, "write watch");
            let first = must(wire::read_msg(&mut s.recv).await, "initial");
            assert_eq!(first.refs.len(), 1);
            let synced = must(wire::read_msg(&mut s.recv).await, "synced");
            assert_eq!(synced.typ, wire::T_REF_SYNCED);
            net.partition(coord, serving, true);
            assert!(
                fc.ref_put_local(coord, &reference_record("trees/b", &root, "tester2", 2))
                    .await
                    .is_ok()
            );
            // The walk asked the serving node (an owner) and failed: the coordinator lists it.
            let view_req = Msg {
                typ: wire::T_VIEW,
                ..Msg::default()
            };
            assert_eq!(
                c.call(coord, &view_req).await.unreachable,
                [serving.0.to_vec()]
            );
            let changes = loop {
                let m = must(wire::read_msg(&mut s.recv).await, "reconciled change");
                if m.typ == wire::T_REF_CHANGES {
                    break m;
                }
                assert_eq!(m.typ, wire::T_REF_SYNCED);
            };
            assert_eq!(changes.refs.len(), 1);
            assert_eq!(changes.refs[0].name, "trees/b");
            assert_eq!(changes.refs[0].user, "tester2");
            net.partition(coord, serving, false);
            s.send.finish();
            fc.close().await;
        }
    }
}
