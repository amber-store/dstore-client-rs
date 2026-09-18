//! `view/view.go`: the cluster view types, decoding, membership helpers and `Placement`.

use std::sync::Arc;

use dstore_codec::cbor_struct;

use crate::placement::{NodeId, Table};

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

impl View {
    /// `View.Encode`.
    pub fn encode(&self) -> Vec<u8> {
        todo!()
    }

    /// `view.Decode`.
    pub fn decode(b: &[u8]) -> Result<View, ViewError> {
        todo!()
    }

    /// `View.Compare`: Greater = this view is newer.
    pub fn compare(&self, incarnation: u64, epoch: u64) -> std::cmp::Ordering {
        todo!()
    }

    pub fn nodes(&self) -> &[Node] {
        todo!()
    }

    /// `nodes`, then `pending.nodes`; exact 32-byte match.
    pub fn node(&self, id: &NodeId) -> Option<&Node> {
        todo!()
    }

    pub fn is_member(&self, id: &NodeId) -> bool {
        todo!()
    }

    pub fn data_endpoint_owner(&self, id: &NodeId) -> Option<NodeId> {
        todo!()
    }

    pub fn is_former(&self, id: &NodeId) -> bool {
        todo!()
    }

    pub fn is_voter(&self, id: &NodeId) -> bool {
        todo!()
    }

    pub fn voter_ids(&self) -> Vec<NodeId> {
        todo!()
    }

    pub fn quorum(&self) -> usize {
        todo!()
    }

    pub fn all_members(&self) -> Vec<NodeId> {
        todo!()
    }
}

impl Node {
    pub fn nid(&self) -> NodeId {
        todo!()
    }

    /// The zone bytes, else the raw id bytes (any length).
    pub fn zone_or_id(&self) -> Vec<u8> {
        todo!()
    }
}

/// `view.ParseNodeID`: exactly 64 hex chars, any case.
pub fn parse_node_id(s: &[u8]) -> Result<NodeId, ViewError> {
    todo!()
}

/// `view.IDString`.
pub fn id_string(id: &NodeId) -> String {
    todo!()
}

/// `view.ShortID`.
pub fn short_id(id: &NodeId) -> String {
    todo!()
}

/// `node.ShortID`: "?" unless len == 32.
pub fn node_short_id(b: &[u8]) -> String {
    todo!()
}

pub fn contains(ids: &[Option<Vec<u8>>], id: &NodeId) -> bool {
    todo!()
}

/// Appends `id` unless present (`view.AddID`).
#[allow(clippy::ptr_arg)] // L0: the stub body does not append yet; remove with the implementation
pub fn add_id(ids: &mut Vec<Option<Vec<u8>>>, id: &NodeId) {
    todo!()
}

/// Zero-padded or truncated copies.
pub fn ids_of(raw: &[Vec<u8>]) -> Vec<NodeId> {
    todo!()
}

pub fn sort_nodes(nodes: &mut [Node]) {
    todo!()
}

pub fn default_min_replicas(r: u8) -> u8 {
    todo!()
}

pub fn validate_change(
    cur: &[Node],
    target: &[Node],
    replicas: i64,
    force: bool,
) -> Result<(), ViewError> {
    todo!()
}

/// `view.Placement`: the current table and the pending one.
pub struct Placement {
    view: Arc<View>,
    cur: Table,
    pending: Option<Table>,
}

impl Placement {
    pub fn new(view: Arc<View>) -> Placement {
        todo!()
    }

    pub fn view(&self) -> &Arc<View> {
        todo!()
    }

    pub fn owners(&self, key: &[u8; 32]) -> Vec<NodeId> {
        todo!()
    }

    /// None without pending; Some([]) with an empty pending.
    pub fn pending_owners(&self, key: &[u8; 32]) -> Option<Vec<NodeId>> {
        todo!()
    }

    pub fn write_set(&self, key: &[u8; 32]) -> Vec<NodeId> {
        todo!()
    }

    /// No dedup within the current table.
    pub fn read_order(&self, key: &[u8; 32]) -> Vec<NodeId> {
        todo!()
    }

    pub fn is_owner(&self, key: &[u8; 32], id: &NodeId) -> bool {
        todo!()
    }

    pub fn is_pending_owner(&self, key: &[u8; 32], id: &NodeId) -> bool {
        todo!()
    }

    pub fn in_write_set(&self, key: &[u8; 32], id: &NodeId) -> bool {
        todo!()
    }
}
