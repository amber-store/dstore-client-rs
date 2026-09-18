//! `view/view.go`: the cluster view types, decoding, membership helpers and `Placement`.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::sync::Arc;

use dstore_codec::cbor_struct;

use crate::placement::{Member, NodeId, Set, Table};

pub const VOTER_SYNC_DONE: i64 = 0;
pub const VOTER_SYNC_PENDING: i64 = 1;

cbor_struct! {
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct Voter = "view.Voter" {
        0 => id: Option<Vec<u8>> = "[]uint8",
        1 => since: u64 = "uint64",
    }
}

cbor_struct! {
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct Former = "view.Former" {
        0 => id: Option<Vec<u8>> = "[]uint8",
        1 => until: i64 = "int64",
    }
}

cbor_struct! {
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct DataEndpoint = "view.DataEndpoint" {
        0 => id: Option<Vec<u8>> = "[]uint8",
        1 => addrs: Vec<String> = "[]string" [omitempty],
    }
}

cbor_struct! {
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct Node = "view.Node" {
        0 => id: Option<Vec<u8>> = "[]uint8",
        1 => weight: u32 = "uint32",
        2 => addrs: Vec<String> = "[]string" [omitempty],
        3 => data: Vec<DataEndpoint> = "[]view.DataEndpoint" [omitempty],
        4 => token: Vec<u8> = "[]uint8" [omitempty],
        5 => zone: String = "string" [omitempty],
        6 => incarnation: u64 = "uint64" [omitempty],
        7 => writable: bool = "bool",
    }
}

cbor_struct! {
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct Ramp = "view.Ramp" {
        0 => node: Option<Vec<u8>> = "[]uint8",
        1 => target: u32 = "uint32",
        2 => step: i64 = "int",
    }
}

cbor_struct! {
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct Acl = "view.ACL" {
        0 => allowed: Vec<Option<Vec<u8>>> = "[][]uint8" [omitempty],
        1 => admins: Vec<Option<Vec<u8>>> = "[][]uint8" [omitempty],
    }
}

cbor_struct! {
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct Pending = "view.Pending" {
        0 => nodes: Option<Vec<Node>> = "[]view.Node",
        1 => replicas: u8 = "uint8",
        2 => id: u64 = "uint64",
        3 => participants_ack: Vec<Option<Vec<u8>>> = "[][]uint8" [omitempty],
        4 => participants: Vec<Option<Vec<u8>>> = "[][]uint8" [omitempty],
        5 => frozen: bool = "bool" [omitempty],
        6 => round: u32 = "uint32" [omitempty],
        7 => primary_done: Vec<Option<Vec<u8>>> = "[][]uint8" [omitempty],
        8 => done: Vec<Option<Vec<u8>>> = "[][]uint8" [omitempty],
        9 => frozen_at: i64 = "int64" [omitempty],
        10 => reason: String = "string" [omitempty],
        11 => ramp: Option<Box<Ramp>> = "view.Ramp" [omitempty],
        12 => since: i64 = "int64" [omitempty],
    }
}

cbor_struct! {
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct View = "view.View" {
        0 => cluster_id: Option<Vec<u8>> = "[]uint8",
        1 => incarnation: u64 = "uint64",
        2 => epoch: u64 = "uint64",
        3 => version: u64 = "uint64",
        4 => placement_epoch: u64 = "uint64",
        5 => replicas: u8 = "uint8",
        6 => min_replicas: u8 = "uint8",
        7 => voters: Option<Vec<Voter>> = "[]view.Voter",
        8 => voter_sync: i64 = "int",
        9 => voter_sync_cursor: Vec<u8> = "[]uint8" [omitempty],
        10 => nodes: Option<Vec<Node>> = "[]view.Node",
        11 => pending: Option<Box<Pending>> = "view.Pending" [omitempty],
        12 => former: Vec<Former> = "[]view.Former" [omitempty],
        13 => fenced: Vec<Option<Vec<u8>>> = "[][]uint8" [omitempty],
        14 => recovered_inc: u64 = "uint64" [omitempty],
        15 => recovered_epoch: u64 = "uint64" [omitempty],
        16 => rebalance_pause: bool = "bool" [omitempty],
        17 => rate_cap: u64 = "uint64" [omitempty],
        18 => voter_sync_target: Vec<u8> = "[]uint8" [omitempty],
        19 => voter_sync_add: bool = "bool" [omitempty],
        20 => deferred_voters: Vec<Option<Vec<u8>>> = "[][]uint8" [omitempty],
        21 => acl: Option<Box<Acl>> = "view.ACL" [omitempty],
        22 => ramps: Vec<Ramp> = "[]view.Ramp" [omitempty],
        23 => remove_voters: Vec<Option<Vec<u8>>> = "[][]uint8" [omitempty],
    }
}

/// `view` errors with Go's texts.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ViewError {
    #[error("view: decode: {0}")]
    Decode(#[source] dstore_codec::DecodeError),
    #[error("view: bad node id {}", dstore_gocompat::quote::quote(.0))]
    BadNodeId(Vec<u8>),
    #[error(
        "view: change drops {dropped} nodes at once with R={replicas}; every key owned only by them would be lost (use --force)"
    )]
    ChangeDropsTooMany { dropped: usize, replicas: i64 },
    #[error("view: not a member")]
    NotMember,
}

/// Go `bytes.Equal(b, id[:])`: a nil or short id never equals a 32-byte id.
fn id_equal(b: Option<&[u8]>, id: &NodeId) -> bool {
    b == Some(&id.0[..])
}

/// Go `bytes.Equal(a, b)`: nil equals empty.
fn bytes_equal(a: Option<&[u8]>, b: Option<&[u8]>) -> bool {
    a.unwrap_or_default() == b.unwrap_or_default()
}

impl View {
    /// `View.Encode` (canonical CBOR).
    pub fn encode(&self) -> Vec<u8> {
        dstore_codec::marshal(self)
    }

    /// `view.Decode`: every codec error is wrapped as `view: decode: …`.
    pub fn decode(b: &[u8]) -> Result<View, ViewError> {
        dstore_codec::unmarshal::<View>(b).map_err(ViewError::Decode)
    }

    /// `View.Compare`: Greater = this view is newer. Incarnation first, then epoch; `version` is not
    /// part of it.
    pub fn compare(&self, incarnation: u64, epoch: u64) -> Ordering {
        self.incarnation
            .cmp(&incarnation)
            .then(self.epoch.cmp(&epoch))
    }

    pub fn nodes(&self) -> &[Node] {
        self.nodes.as_deref().unwrap_or_default()
    }

    /// `pending.nodes`, empty without a pending transition.
    fn pending_nodes(&self) -> &[Node] {
        match &self.pending {
            Some(p) => p.nodes.as_deref().unwrap_or_default(),
            None => &[],
        }
    }

    fn voter_list(&self) -> &[Voter] {
        self.voters.as_deref().unwrap_or_default()
    }

    /// `nodes`, then `pending.nodes`; exact 32-byte match.
    pub fn node(&self, id: &NodeId) -> Option<&Node> {
        self.nodes()
            .iter()
            .chain(self.pending_nodes())
            .find(|n| id_equal(n.id.as_deref(), id))
    }

    /// `View.IsMember`: a node under `nodes` or `pending.nodes`, or one of their data endpoints.
    pub fn is_member(&self, id: &NodeId) -> bool {
        self.node(id).is_some() || self.data_endpoint_owner(id).is_some()
    }

    /// `View.DataEndpointOwner`: the `nid()` of the first node under `nodes`, then `pending.nodes`, with
    /// a data endpoint `id`.
    pub fn data_endpoint_owner(&self, id: &NodeId) -> Option<NodeId> {
        fn check(nodes: &[Node], id: &NodeId) -> Option<NodeId> {
            nodes
                .iter()
                .find(|n| n.data.iter().any(|d| id_equal(d.id.as_deref(), id)))
                .map(Node::nid)
        }
        check(self.nodes(), id).or_else(|| check(self.pending_nodes(), id))
    }

    pub fn is_former(&self, id: &NodeId) -> bool {
        self.former.iter().any(|f| id_equal(f.id.as_deref(), id))
    }

    pub fn is_voter(&self, id: &NodeId) -> bool {
        self.voter_list()
            .iter()
            .any(|vo| id_equal(vo.id.as_deref(), id))
    }

    /// `View.VoterIDs`: zero-padded or truncated copies.
    pub fn voter_ids(&self) -> Vec<NodeId> {
        self.voter_list()
            .iter()
            .map(|vo| NodeId::from_slice_lossy(vo.id.as_deref().unwrap_or_default()))
            .collect()
    }

    /// `View.Quorum`: ⌊|voters|/2⌋ + 1.
    pub fn quorum(&self) -> usize {
        self.voter_list().len() / 2 + 1
    }

    /// `View.AllMembers`: the `nid()` of every node under `nodes`, then `pending.nodes`, deduplicated in
    /// first-seen order.
    pub fn all_members(&self) -> Vec<NodeId> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for n in self.nodes().iter().chain(self.pending_nodes()) {
            let id = n.nid();
            if seen.insert(id) {
                out.push(id);
            }
        }
        out
    }
}

impl Node {
    /// `Node.NID`: Go `copy()` of the id into 32 bytes.
    pub fn nid(&self) -> NodeId {
        NodeId::from_slice_lossy(self.id.as_deref().unwrap_or_default())
    }

    /// The zone bytes, else the raw id bytes (any length).
    pub fn zone_or_id(&self) -> Vec<u8> {
        if self.zone.is_empty() {
            self.id.clone().unwrap_or_default()
        } else {
            self.zone.as_bytes().to_vec()
        }
    }
}

/// `view.ParseNodeID`: exactly 64 hex chars, any case.
pub fn parse_node_id(s: &[u8]) -> Result<NodeId, ViewError> {
    match dstore_gocompat::hex::decode_string(s) {
        Ok(b) if b.len() == 32 => Ok(NodeId::from_slice_lossy(&b)),
        _ => Err(ViewError::BadNodeId(s.to_vec())),
    }
}

/// `view.IDString`.
pub fn id_string(id: &NodeId) -> String {
    id.to_hex()
}

/// `view.ShortID`.
pub fn short_id(id: &NodeId) -> String {
    id.short()
}

/// `node.ShortID`: "?" unless len == 32.
pub fn node_short_id(b: &[u8]) -> String {
    if b.len() == 32 {
        NodeId::from_slice_lossy(b).short()
    } else {
        "?".to_owned()
    }
}

/// `view.Contains` (`bytes.Equal`).
pub fn contains(ids: &[Option<Vec<u8>>], id: &NodeId) -> bool {
    ids.iter().any(|b| id_equal(b.as_deref(), id))
}

/// Appends `id` unless present (`view.AddID`).
pub fn add_id(ids: &mut Vec<Option<Vec<u8>>>, id: &NodeId) {
    if !contains(ids, id) {
        ids.push(Some(id.0.to_vec()));
    }
}

/// Zero-padded or truncated copies.
pub fn ids_of(raw: &[Vec<u8>]) -> Vec<NodeId> {
    raw.iter()
        .map(|b| NodeId::from_slice_lossy(b.as_slice()))
        .collect()
}

/// `view.SortNodes`: by id bytes (`bytes.Compare`, nil and empty comparing equal), through the port of
/// Go's unstable `sort.Slice`, so nodes that share an id end up in Go's order.
pub fn sort_nodes(nodes: &mut [Node]) {
    crate::gosort::slice(nodes, |a, b| {
        a.id.as_deref().unwrap_or_default() < b.id.as_deref().unwrap_or_default()
    });
}

/// `view.DefaultMinReplicas`: max(R−1, 2) capped at R.
pub fn default_min_replicas(r: u8) -> u8 {
    let r = i64::from(r);
    let mut m = r - 1;
    if m < 2 {
        m = 2;
    }
    if m > r {
        m = r;
    }
    m as u8
}

/// `view.ValidateChange`: refuses a target set that drops R or more weighted current nodes at once,
/// unless forced. A current node counts as dropped when it has weight and no target entry with the same
/// id (`bytes.Equal`) has weight.
pub fn validate_change(
    cur: &[Node],
    target: &[Node],
    replicas: i64,
    force: bool,
) -> Result<(), ViewError> {
    if force {
        return Ok(());
    }
    let mut dropped = 0usize;
    for n in cur {
        let found = target
            .iter()
            .any(|t| bytes_equal(n.id.as_deref(), t.id.as_deref()) && t.weight > 0);
        if !found && n.weight > 0 {
            dropped += 1;
        }
    }
    if dropped as i64 >= replicas && replicas > 0 {
        return Err(ViewError::ChangeDropsTooMany { dropped, replicas });
    }
    Ok(())
}

/// `view.Placement`: the current table and the pending one.
///
/// Go serialises lookups with a mutex; the tables here synchronise their caches themselves.
pub struct Placement {
    view: Arc<View>,
    cur: Table,
    pending: Option<Table>,
}

/// Go `members`: node order, duplicates kept.
fn members(nodes: &[Node]) -> Vec<Member> {
    nodes
        .iter()
        .map(|n| Member {
            id: n.nid(),
            weight: n.weight,
            zone: n.zone_or_id(),
        })
        .collect()
}

impl Placement {
    /// `view.NewPlacement`.
    pub fn new(view: Arc<View>) -> Placement {
        let cur = Table::new(Set::new(members(view.nodes())), usize::from(view.replicas));
        let pending = view.pending.as_ref().map(|p| {
            Table::new(
                Set::new(members(p.nodes.as_deref().unwrap_or_default())),
                usize::from(p.replicas),
            )
        });
        Placement { view, cur, pending }
    }

    pub fn view(&self) -> &Arc<View> {
        &self.view
    }

    /// `Placement.Owners`: owners under `nodes`.
    pub fn owners(&self, key: &[u8; 32]) -> Vec<NodeId> {
        self.cur.owner_ids(key)
    }

    /// None without pending; Some([]) with an empty pending.
    pub fn pending_owners(&self, key: &[u8; 32]) -> Option<Vec<NodeId>> {
        self.pending.as_ref().map(|t| t.owner_ids(key))
    }

    /// `Placement.WriteSet`: the owners, then the pending owners absent from the owners. As in Go, the
    /// seen set holds only the owners, so an id repeated among the pending owners is appended twice.
    pub fn write_set(&self, key: &[u8; 32]) -> Vec<NodeId> {
        let mut out = self.owners(key);
        let Some(pending) = self.pending_owners(key) else {
            return out;
        };
        let seen: HashSet<NodeId> = out.iter().copied().collect();
        out.extend(pending.into_iter().filter(|id| !seen.contains(id)));
        out
    }

    /// No dedup within the current table.
    ///
    /// `Placement.ReadOrder`: the rank under `nodes`, then the pending rank entries absent from it (the
    /// seen set holds only the current rank, as in Go).
    pub fn read_order(&self, key: &[u8; 32]) -> Vec<NodeId> {
        let mut out = self.cur.rank_ids(key);
        let Some(pending) = &self.pending else {
            return out;
        };
        let seen: HashSet<NodeId> = out.iter().copied().collect();
        out.extend(
            pending
                .rank_ids(key)
                .into_iter()
                .filter(|id| !seen.contains(id)),
        );
        out
    }

    pub fn is_owner(&self, key: &[u8; 32], id: &NodeId) -> bool {
        self.cur.is_owner(key, id)
    }

    /// False without a pending transition.
    pub fn is_pending_owner(&self, key: &[u8; 32], id: &NodeId) -> bool {
        self.pending.as_ref().is_some_and(|t| t.is_owner(key, id))
    }

    pub fn in_write_set(&self, key: &[u8; 32], id: &NodeId) -> bool {
        self.is_owner(key, id) || self.is_pending_owner(key, id)
    }
}

/// Go's `cap(v.ClusterID)` for the view that `view.Decode(b)` returns, or the decode error.
///
/// `cluster status` prints `%x` of `v.ClusterID[:4]` (`cmd/dstore/client.go:136`), and Go slices up to the
/// capacity, not the length ([`status_cluster_prefix`]). fxamacker v2.9.3 gives:
/// - nil (absent, null, undefined): 0;
/// - a definite-length byte string, also inside tags 2/3: its length (`make` + `copy`);
/// - an array of integers: its element count (`reflect.MakeSlice(count, count)`);
/// - an indefinite-length byte string: the capacity of `append`-ing its chunks to `[]byte{}`, through
///   go1.26.5 `runtime.growslice` and the malloc size classes.
///
/// Only the first key 0 counts (a later duplicate is skipped), and tags before the map or the value are
/// stripped, as `View::decode` does. [`View`] itself keeps no capacity, so this needs the view bytes.
pub fn cluster_id_cap(b: &[u8]) -> Result<usize, ViewError> {
    View::decode(b)?;
    Ok(Scan { data: b, off: 0 }.cluster_id_cap())
}

/// `cmd/dstore` printStatus `%x` of `v.ClusterID[:4]`: 8 lowercase hex characters, where bytes past the
/// length (within the capacity) read as zero, as Go's zeroed append buffers do. With `cap < 4`, the Go
/// runtime panic text, for `dstore_cli::go_panic_exit` (PORTING DD-7):
/// `runtime error: slice bounds out of range [:4] with capacity N`.
pub fn status_cluster_prefix(cluster_id: Option<&[u8]>, cap: usize) -> Result<String, String> {
    if cap < 4 {
        return Err(format!(
            "runtime error: slice bounds out of range [:4] with capacity {cap}"
        ));
    }
    let mut prefix = [0u8; 4];
    for (dst, src) in prefix.iter_mut().zip(cluster_id.unwrap_or_default()) {
        *dst = *src;
    }
    Ok(dstore_gocompat::fmt::hex_lower(&prefix))
}

/// A cursor over input that [`View::decode`] accepted: well-formed, with every item of a known shape.
struct Scan<'a> {
    data: &'a [u8],
    off: usize,
}

impl Scan<'_> {
    fn peek(&self) -> Option<u8> {
        self.data.get(self.off).copied()
    }

    /// fxamacker `getHead`: (major, additional information, argument).
    fn head(&mut self) -> (u8, u8, u64) {
        let first = self.peek().unwrap_or(0);
        self.off = self.off.saturating_add(1);
        let ai = first & 0x1f;
        let size = match ai {
            24 => 1,
            25 => 2,
            26 => 4,
            27 => 8,
            _ => return (first >> 5, ai, u64::from(ai)),
        };
        let mut val = 0u64;
        for _ in 0..size {
            val = (val << 8) | u64::from(self.peek().unwrap_or(0));
            self.off = self.off.saturating_add(1);
        }
        (first >> 5, ai, val)
    }

    fn advance(&mut self, n: u64) {
        self.off = self
            .off
            .saturating_add(usize::try_from(n).unwrap_or(usize::MAX));
    }

    /// fxamacker `foundBreak`; the end of the input also ends a container.
    fn found_break(&mut self) -> bool {
        match self.peek() {
            None => true,
            Some(0xff) => {
                self.off += 1;
                true
            }
            Some(_) => false,
        }
    }

    /// fxamacker `skip`: one item, unexamined.
    fn skip(&mut self) {
        let (major, ai, val) = self.head();
        if ai == 31 && (2..=5).contains(&major) {
            while !self.found_break() {
                self.skip();
            }
            return;
        }
        match major {
            2 | 3 => self.advance(val),
            4 | 5 => {
                let items = if major == 5 {
                    val.saturating_mul(2)
                } else {
                    val
                };
                for _ in 0..items {
                    if self.peek().is_none() {
                        break;
                    }
                    self.skip();
                }
            }
            6 => self.skip(),
            _ => {}
        }
    }

    /// `parseToValue` of the top-level item into `View`, then `parseMapToStruct` up to the first key 0.
    fn cluster_id_cap(&mut self) -> usize {
        // Tags around the struct are stripped; null or undefined is the zero view.
        while self.peek().is_some_and(|b| b >> 5 == 6) {
            self.head();
        }
        if self.peek().is_none_or(|b| b >> 5 != 5) {
            return 0;
        }
        let (_, ai, count) = self.head();
        let mut left = count;
        loop {
            if ai == 31 {
                if self.found_break() {
                    return 0;
                }
            } else if left == 0 || self.peek().is_none() {
                return 0;
            } else {
                left -= 1;
            }
            let save = self.off;
            let (major, _, key) = self.head();
            if major == 0 && key == 0 {
                return self.byte_slice_cap();
            }
            self.off = save;
            self.skip();
            self.skip();
        }
    }

    /// `parseToValue` into `[]byte`: the capacity of the resulting slice.
    fn byte_slice_cap(&mut self) -> usize {
        loop {
            match self.peek().map(|b| b >> 5) {
                Some(2) => return self.byte_string_cap(),
                Some(4) => return self.array_len(),
                Some(6) => {
                    let (_, _, num) = self.head();
                    if num == 2 || num == 3 {
                        return self.byte_string_cap();
                    }
                    // Any other tag: its content decodes into the same slice.
                }
                // null or undefined: nil.
                _ => return 0,
            }
        }
    }

    /// fxamacker `parseByteString` + `fillByteString`.
    fn byte_string_cap(&mut self) -> usize {
        let (_, ai, val) = self.head();
        if ai != 31 {
            return usize::try_from(val).unwrap_or(usize::MAX);
        }
        let (mut len, mut cap) = (0usize, 0usize);
        while !self.found_break() {
            let (_, _, n) = self.head();
            self.advance(n);
            len = len.saturating_add(usize::try_from(n).unwrap_or(usize::MAX));
            if len > cap {
                cap = go_grow_byte_slice(len, cap);
            }
        }
        cap
    }

    /// fxamacker `parseArrayToSlice`: definite count, or `numOfItemsUntilBreak`.
    fn array_len(&mut self) -> usize {
        let (_, ai, val) = self.head();
        if ai != 31 {
            return usize::try_from(val).unwrap_or(usize::MAX);
        }
        let mut n = 0usize;
        while !self.found_break() {
            self.skip();
            n += 1;
        }
        n
    }
}

/// go1.26.5 `runtime.growslice` for a `[]byte` (element size 1, no pointers): the new capacity.
fn go_grow_byte_slice(new_len: usize, old_cap: usize) -> usize {
    go_roundupsize_noscan(go_nextslicecap(new_len, old_cap))
}

/// go1.26.5 `runtime.nextslicecap`.
fn go_nextslicecap(new_len: usize, old_cap: usize) -> usize {
    const THRESHOLD: usize = 256;
    let mut newcap = old_cap;
    let doublecap = newcap.saturating_add(newcap);
    if new_len > doublecap {
        return new_len;
    }
    if old_cap < THRESHOLD {
        return doublecap;
    }
    loop {
        // Transition from growing 2x for small slices to growing 1.25x for large slices.
        newcap = newcap.saturating_add(newcap.saturating_add(3 * THRESHOLD) >> 2);
        if newcap >= new_len {
            return newcap;
        }
    }
}

/// go1.26.5 `runtime.roundupsize(size, noscan = true)`.
fn go_roundupsize_noscan(size: usize) -> usize {
    const MAX_SMALL_SIZE: usize = 32768;
    const MALLOC_HEADER_SIZE: usize = 8;
    const SMALL_SIZE_DIV: usize = 8;
    const SMALL_SIZE_MAX: usize = 1024;
    const LARGE_SIZE_DIV: usize = 128;
    const PAGE_SIZE: usize = 8192;
    if size <= MAX_SMALL_SIZE - MALLOC_HEADER_SIZE {
        let class = if size <= SMALL_SIZE_MAX - 8 {
            GO_SIZE_TO_SIZE_CLASS8.get(size.div_ceil(SMALL_SIZE_DIV))
        } else {
            // Go's uintptr `divRoundUp(reqSize-smallSizeMax, largeSizeDiv)` wraps for 1017..=1023 and
            // lands on index 0.
            let n = size.wrapping_sub(SMALL_SIZE_MAX);
            GO_SIZE_TO_SIZE_CLASS128.get(n.wrapping_add(LARGE_SIZE_DIV - 1) / LARGE_SIZE_DIV)
        };
        return class
            .and_then(|&c| GO_SIZE_CLASS_TO_SIZE.get(usize::from(c)))
            .map_or(size, |&s| usize::from(s));
    }
    match size.checked_add(PAGE_SIZE - 1) {
        Some(req) => req & !(PAGE_SIZE - 1),
        None => size,
    }
}

/// go1.26.5 `internal/runtime/gc.SizeClassToSize`.
const GO_SIZE_CLASS_TO_SIZE: [u16; 68] = [
    0, 8, 16, 24, 32, 48, 64, 80, 96, 112, 128, 144, 160, 176, 192, 208, 224, 240, 256, 288, 320,
    352, 384, 416, 448, 480, 512, 576, 640, 704, 768, 896, 1024, 1152, 1280, 1408, 1536, 1792,
    2048, 2304, 2688, 3072, 3200, 3456, 4096, 4864, 5376, 6144, 6528, 6784, 6912, 8192, 9472, 9728,
    10240, 10880, 12288, 13568, 14336, 16384, 18432, 19072, 20480, 21760, 24576, 27264, 28672,
    32768,
];

/// go1.26.5 `internal/runtime/gc.SizeToSizeClass8`.
const GO_SIZE_TO_SIZE_CLASS8: [u8; 129] = [
    0, 1, 2, 3, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13, 14, 14, 15, 15,
    16, 16, 17, 17, 18, 18, 19, 19, 19, 19, 20, 20, 20, 20, 21, 21, 21, 21, 22, 22, 22, 22, 23, 23,
    23, 23, 24, 24, 24, 24, 25, 25, 25, 25, 26, 26, 26, 26, 27, 27, 27, 27, 27, 27, 27, 27, 28, 28,
    28, 28, 28, 28, 28, 28, 29, 29, 29, 29, 29, 29, 29, 29, 30, 30, 30, 30, 30, 30, 30, 30, 31, 31,
    31, 31, 31, 31, 31, 31, 31, 31, 31, 31, 31, 31, 31, 31, 32, 32, 32, 32, 32, 32, 32, 32, 32, 32,
    32, 32, 32, 32, 32, 32,
];

/// go1.26.5 `internal/runtime/gc.SizeToSizeClass128`.
const GO_SIZE_TO_SIZE_CLASS128: [u8; 249] = [
    32, 33, 34, 35, 36, 37, 37, 38, 38, 39, 39, 40, 40, 40, 41, 41, 41, 42, 43, 43, 44, 44, 44, 44,
    44, 45, 45, 45, 45, 45, 45, 46, 46, 46, 46, 47, 47, 47, 47, 47, 47, 48, 48, 48, 49, 49, 50, 51,
    51, 51, 51, 51, 51, 51, 51, 51, 51, 52, 52, 52, 52, 52, 52, 52, 52, 52, 52, 53, 53, 54, 54, 54,
    54, 55, 55, 55, 55, 55, 56, 56, 56, 56, 56, 56, 56, 56, 56, 56, 56, 57, 57, 57, 57, 57, 57, 57,
    57, 57, 57, 58, 58, 58, 58, 58, 58, 59, 59, 59, 59, 59, 59, 59, 59, 59, 59, 59, 59, 59, 59, 59,
    59, 60, 60, 60, 60, 60, 60, 60, 60, 60, 60, 60, 60, 60, 60, 60, 60, 61, 61, 61, 61, 61, 62, 62,
    62, 62, 62, 62, 62, 62, 62, 62, 62, 63, 63, 63, 63, 63, 63, 63, 63, 63, 63, 64, 64, 64, 64, 64,
    64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 65, 65, 65, 65, 65, 65, 65,
    65, 65, 65, 65, 65, 65, 65, 65, 65, 65, 65, 65, 65, 65, 66, 66, 66, 66, 66, 66, 66, 66, 66, 66,
    66, 67, 67, 67, 67, 67, 67, 67, 67, 67, 67, 67, 67, 67, 67, 67, 67, 67, 67, 67, 67, 67, 67, 67,
    67, 67, 67, 67, 67, 67, 67, 67, 67,
];

#[cfg(test)]
mod tests {
    use super::*;

    fn h(s: &str) -> Vec<u8> {
        match dstore_gocompat::hex::decode_string(s.as_bytes()) {
            Ok(b) => b,
            Err(e) => panic!("bad test hex {s:?}: {e}"),
        }
    }

    fn cat(parts: &[&[u8]]) -> Vec<u8> {
        parts.concat()
    }

    /// A definite-length byte string of `n` bytes 01, 02, … (wrapping), as the scratch Go program built.
    fn bs(n: usize) -> Vec<u8> {
        let mut out = match n {
            0..=23 => vec![0x40 + n as u8],
            24..=255 => vec![0x58, n as u8],
            256..=65535 => cat(&[&[0x59], &(n as u16).to_be_bytes()]),
            _ => cat(&[&[0x5a], &(n as u32).to_be_bytes()]),
        };
        out.extend((0..n).map(|i| (i + 1) as u8));
        out
    }

    fn indef(chunks: &[usize]) -> Vec<u8> {
        let mut out = vec![0x5f];
        for &c in chunks {
            out.extend(bs(c));
        }
        out.push(0xff);
        out
    }

    fn with_key0(value: &[u8]) -> Vec<u8> {
        cat(&[&[0xa1, 0x00], value])
    }

    /// (name, input, len, cap, nil) from `view.Decode` in a scratch go1.26.5 program (deleted afterwards)
    /// printing `len(v.ClusterID)`, `cap(v.ClusterID)` and `v.ClusterID == nil`.
    fn go_cases() -> Vec<(&'static str, Vec<u8>, usize, usize, bool)> {
        vec![
            ("top_null", h("f6"), 0, 0, true),
            ("top_undefined", h("f7"), 0, 0, true),
            ("empty_map", h("a0"), 0, 0, true),
            ("null", h("a100f6"), 0, 0, true),
            ("undefined", h("a100f7"), 0, 0, true),
            ("def_0", with_key0(&bs(0)), 0, 0, false),
            ("def_1", with_key0(&bs(1)), 1, 1, false),
            ("def_3", with_key0(&bs(3)), 3, 3, false),
            ("def_4", with_key0(&bs(4)), 4, 4, false),
            ("def_24", with_key0(&bs(24)), 24, 24, false),
            ("def_300", with_key0(&bs(300)), 300, 300, false),
            ("indef_none", with_key0(&indef(&[])), 0, 0, false),
            ("indef_0", with_key0(&indef(&[0])), 0, 0, false),
            ("indef_0_0", with_key0(&indef(&[0, 0])), 0, 0, false),
            ("indef_1", with_key0(&indef(&[1])), 1, 8, false),
            ("indef_0_0_3", with_key0(&indef(&[0, 0, 3])), 3, 8, false),
            ("indef_8", with_key0(&indef(&[8])), 8, 8, false),
            ("indef_8_1", with_key0(&indef(&[8, 1])), 9, 16, false),
            ("indef_9", with_key0(&indef(&[9])), 9, 16, false),
            ("indef_16_1", with_key0(&indef(&[16, 1])), 17, 32, false),
            ("indef_17", with_key0(&indef(&[17])), 17, 24, false),
            ("indef_5_5", with_key0(&indef(&[5, 5])), 10, 16, false),
            ("indef_100", with_key0(&indef(&[100])), 100, 112, false),
            ("indef_255_1", with_key0(&indef(&[255, 1])), 256, 256, false),
            ("indef_256_1", with_key0(&indef(&[256, 1])), 257, 512, false),
            (
                "indef_300_300",
                with_key0(&indef(&[300, 300])),
                600,
                1024,
                false,
            ),
            ("indef_1017", with_key0(&indef(&[1017])), 1017, 1024, false),
            ("indef_1024", with_key0(&indef(&[1024])), 1024, 1024, false),
            ("indef_1025", with_key0(&indef(&[1025])), 1025, 1152, false),
            ("indef_3000", with_key0(&indef(&[3000])), 3000, 3072, false),
            (
                "indef_32760",
                with_key0(&indef(&[32760])),
                32760,
                32768,
                false,
            ),
            (
                "indef_32761",
                with_key0(&indef(&[32761])),
                32761,
                32768,
                false,
            ),
            (
                "indef_40000",
                with_key0(&indef(&[40000])),
                40000,
                40960,
                false,
            ),
            ("indef_1x300", with_key0(&indef(&[1; 300])), 300, 512, false),
            (
                "indef_7x1000",
                with_key0(&indef(&[7; 1000])),
                7000,
                9472,
                false,
            ),
            (
                "indef_2048_x3",
                with_key0(&indef(&[2048, 2048, 2048])),
                6144,
                6528,
                false,
            ),
            ("array_0", h("a10080"), 0, 0, false),
            ("array_3", h("a10083010203"), 3, 3, false),
            ("array_5", h("a100850102030405"), 5, 5, false),
            ("array_indef_2", h("a1009f0102ff"), 2, 2, false),
            ("array_indef_0", h("a1009fff"), 0, 0, false),
            (
                "bignum_def_2",
                with_key0(&cat(&[&[0xc2], &bs(2)])),
                2,
                2,
                false,
            ),
            (
                "bignum_indef_2",
                with_key0(&cat(&[&[0xc2], &indef(&[2])])),
                2,
                8,
                false,
            ),
            (
                "neg_bignum_def_3",
                with_key0(&cat(&[&[0xc3], &bs(3)])),
                3,
                3,
                false,
            ),
            (
                "tag100_def_2",
                with_key0(&cat(&[&[0xd8, 0x64], &bs(2)])),
                2,
                2,
                false,
            ),
            (
                "tag100_indef_2",
                with_key0(&cat(&[&[0xd8, 0x64], &indef(&[2])])),
                2,
                8,
                false,
            ),
            (
                "selfdesc_def_2",
                with_key0(&cat(&[&[0xd9, 0xd9, 0xf7], &bs(2)])),
                2,
                2,
                false,
            ),
            (
                "tag100_bignum_indef_1",
                with_key0(&cat(&[&[0xd8, 0x64, 0xc2], &indef(&[1])])),
                1,
                8,
                false,
            ),
            (
                "dup_first_indef_wins",
                cat(&[&[0xa2, 0x00], &indef(&[1]), &[0x00], &bs(4)]),
                1,
                8,
                false,
            ),
            (
                "dup_first_def_wins",
                cat(&[&[0xa2, 0x00], &bs(1), &[0x00], &indef(&[4])]),
                1,
                1,
                false,
            ),
            (
                "dup_first_null_wins",
                cat(&[&[0xa2, 0x00, 0xf6, 0x00], &indef(&[4])]),
                0,
                0,
                true,
            ),
            (
                "indef_map",
                cat(&[&[0xbf, 0x00], &indef(&[1]), &[0xff]]),
                1,
                8,
                false,
            ),
            (
                "top_selfdesc",
                cat(&[&h("d9d9f7a100"), &indef(&[2])]),
                2,
                8,
                false,
            ),
            ("top_tag100", cat(&[&h("d864a100"), &bs(2)]), 2, 2, false),
            ("text_key_first", h("a26130410900420102"), 2, 2, false),
            (
                "neg_key_first",
                cat(&[&h("a220410900"), &indef(&[3])]),
                3,
                8,
                false,
            ),
            (
                "unknown_key_indef",
                cat(&[&h("a218630000"), &bs(2)]),
                2,
                2,
                false,
            ),
            (
                "unknown_key_nested",
                cat(&[&h("a218635f4101ff00"), &bs(2)]),
                2,
                2,
                false,
            ),
            (
                "non_shortest_key",
                cat(&[&h("a11800"), &indef(&[2])]),
                2,
                8,
                false,
            ),
            ("key_1_only", h("a10101"), 0, 0, true),
        ]
    }

    #[test]
    fn cluster_id_cap_matches_go() {
        for (name, input, len, cap, nil) in go_cases() {
            let v = match View::decode(&input) {
                Ok(v) => v,
                Err(e) => panic!("{name}: {e}"),
            };
            assert_eq!(v.cluster_id.is_none(), nil, "{name}: nil");
            assert_eq!(
                v.cluster_id.as_ref().map_or(0, Vec::len),
                len,
                "{name}: len"
            );
            assert_eq!(cluster_id_cap(&input), Ok(cap), "{name}: cap");
        }
    }

    #[test]
    fn cluster_id_cap_keeps_the_decode_error() {
        assert_eq!(
            cluster_id_cap(&[]).map_err(|e| e.to_string()),
            Err("view: decode: EOF".to_owned())
        );
        assert_eq!(
            cluster_id_cap(&h("a1006461626364")).map_err(|e| e.to_string()),
            Err("view: decode: cbor: cannot unmarshal UTF-8 text string into Go struct field view.View.0 of type []uint8".to_owned())
        );
    }

    #[test]
    fn go_growth_rules() {
        // Appending n bytes to an empty []byte gives roundupsize(n).
        let singles: [(usize, usize); 10] = [
            (1, 8),
            (8, 8),
            (9, 16),
            (17, 24),
            (1016, 1024),
            (1017, 1024),
            (1025, 1152),
            (32760, 32768),
            (32761, 32768),
            (40000, 40960),
        ];
        for (n, want) in singles {
            assert_eq!(go_grow_byte_slice(n, 0), want, "append {n} to empty");
        }
        assert_eq!(go_nextslicecap(300, 256), 512);
        assert_eq!(go_nextslicecap(4096, 2048), 4732);
        assert_eq!(go_roundupsize_noscan(0), 0);
    }

    #[test]
    fn status_cluster_prefix_texts() {
        assert_eq!(
            status_cluster_prefix(None, 0),
            Err("runtime error: slice bounds out of range [:4] with capacity 0".to_owned())
        );
        assert_eq!(
            status_cluster_prefix(Some(&[1, 2, 3]), 3),
            Err("runtime error: slice bounds out of range [:4] with capacity 3".to_owned())
        );
        assert_eq!(
            status_cluster_prefix(Some(&[1, 2]), 8),
            Ok("01020000".to_owned())
        );
        assert_eq!(
            status_cluster_prefix(Some(&h("48c1cc4d8bba1e3cafbbf4d4829d7ad0")), 16),
            Ok("48c1cc4d".to_owned())
        );
    }

    fn node(id: &[u8], weight: u32) -> Node {
        Node {
            id: Some(id.to_vec()),
            weight,
            ..Node::default()
        }
    }

    // A 31-byte id and the same bytes followed by 00 share a NodeId but not a zone key, so both own under
    // the pending set. Go appends that id twice to WriteSet and ReadOrder (checked with a scratch go1.26.5
    // program against dstore v0.1.9, deleted afterwards).
    #[test]
    fn write_set_and_read_order_dedup_only_against_the_current_list() {
        let mut x: Vec<u8> = (1..=32).collect();
        x[31] = 0;
        let v = View {
            replicas: 1,
            nodes: Some(vec![node(&[0xb0; 32], 1)]),
            pending: Some(Box::new(Pending {
                replicas: 2,
                nodes: Some(vec![node(&x[..31], 1), node(&x, 1)]),
                ..Pending::default()
            })),
            ..View::default()
        };
        let p = Placement::new(Arc::new(v));
        let k = [0u8; 32];
        let xid = NodeId::from_slice_lossy(&x);
        let bid = NodeId([0xb0; 32]);
        assert_eq!(p.owners(&k), [bid]);
        assert_eq!(p.pending_owners(&k), Some(vec![xid, xid]));
        assert_eq!(p.write_set(&k), [bid, xid, xid]);
        assert_eq!(p.read_order(&k), [bid, xid, xid]);
        assert!(p.in_write_set(&k, &xid));
        assert!(!p.is_owner(&k, &xid));
        assert!(p.is_pending_owner(&k, &xid));
    }

    // Go's sort.Slice order among nodes that share an id (nil and empty compare equal): from a scratch
    // go1.26.5 program printing the input positions after dstore v0.1.9 view.SortNodes (deleted afterwards).
    // A stable sort gives another order for both lists.
    #[test]
    fn sort_nodes_in_go_sort_slice_order() {
        let cases: [(&str, &str); 2] = [
            (
                ",0201,0202,0001,,02,,,02,02,00,0101,01,0202,0202,0200,00",
                "7,0,4,6,16,10,3,12,11,8,9,5,15,1,2,13,14",
            ),
            (
                "0102,,01,00,nil,0102,0102,0102,0002,01,0000,0002,nil,nil,,0202,0100",
                "12,14,4,13,1,3,10,11,8,9,2,16,7,6,0,5,15",
            ),
        ];
        for (ids, want) in cases {
            let mut nodes: Vec<Node> = ids
                .split(',')
                .enumerate()
                .map(|(i, d)| Node {
                    id: (d != "nil").then(|| h(d)),
                    weight: i as u32,
                    ..Node::default()
                })
                .collect();
            sort_nodes(&mut nodes);
            let got: Vec<String> = nodes.iter().map(|n| n.weight.to_string()).collect();
            assert_eq!(got.join(","), want, "{ids}");
        }
    }

    #[test]
    fn lookups_need_exact_32_byte_ids() {
        let short = node(&[7; 31], 1);
        let v = View {
            nodes: Some(vec![short]),
            ..View::default()
        };
        let padded = NodeId::from_slice_lossy(&[7; 31]);
        assert!(v.node(&padded).is_none());
        assert!(!v.is_member(&padded));
        assert_eq!(v.all_members(), [padded]);
        assert_eq!(v.quorum(), 1);
        assert!(v.voter_ids().is_empty());
    }
}
