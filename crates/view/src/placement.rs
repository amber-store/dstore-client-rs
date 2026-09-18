//! `placement/placement.go`: slots, salts, the rational rank order, owners and cached tables.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

pub const SLOT_BITS: u32 = 20;
pub const SLOTS: u32 = 1 << SLOT_BITS;

/// A node id: the node's ed25519 public key bytes (`placement.NodeID`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(pub [u8; 32]);

impl NodeId {
    /// Go `copy()`: min(len, 32) bytes, zero-padded.
    pub fn from_slice_lossy(b: &[u8]) -> NodeId {
        todo!()
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        todo!()
    }

    /// `view.IDString`.
    pub fn to_hex(&self) -> String {
        todo!()
    }

    /// `view.ShortID`: hex of bytes 0..4.
    pub fn short(&self) -> String {
        todo!()
    }
}

impl From<[u8; 32]> for NodeId {
    fn from(b: [u8; 32]) -> NodeId {
        todo!()
    }
}

/// `placement.Member`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub id: NodeId,
    pub weight: u32,
    /// An empty zone uses the 32 id bytes as the zone key.
    pub zone: Vec<u8>,
}

/// `placement.Slot`.
pub fn slot(key: &[u8; 32]) -> u32 {
    todo!()
}

/// `placement.Salt`.
pub fn salt(id: &NodeId) -> u64 {
    todo!()
}

/// `placement.Fmix64`.
pub fn fmix64(x: u64) -> u64 {
    todo!()
}

/// `placement.Log2Fix`: panics "placement: Log2Fix(0)".
pub fn log2fix(x: u64) -> u64 {
    todo!()
}

/// `placement.L`.
pub fn l(slot: u32, salt: u64) -> u64 {
    todo!()
}

/// `placement.Set`.
pub struct Set {
    members: Vec<Member>,
    salts: Vec<u64>,
}

impl Set {
    pub fn new(members: Vec<Member>) -> Set {
        todo!()
    }

    pub fn members(&self) -> &[Member] {
        todo!()
    }

    /// Stable 128-bit rational order; weight-0 members last by id.
    pub fn rank(&self, slot: u32) -> Vec<u32> {
        todo!()
    }

    pub fn owners(&self, slot: u32, r: usize) -> Vec<u32> {
        todo!()
    }
}

/// `placement.Table`: a set with lazily cached owners and ranks per slot.
pub struct Table {
    set: Set,
    r: usize,
    owners: RwLock<HashMap<u32, Arc<[u32]>>>,
    rank: RwLock<HashMap<u32, Arc<[u32]>>>,
}

impl Table {
    pub fn new(set: Set, r: usize) -> Table {
        todo!()
    }

    pub fn set(&self) -> &Set {
        todo!()
    }

    pub fn replicas(&self) -> usize {
        todo!()
    }

    pub fn owners(&self, slot: u32) -> Arc<[u32]> {
        todo!()
    }

    pub fn rank(&self, slot: u32) -> Arc<[u32]> {
        todo!()
    }

    pub fn owner_ids(&self, key: &[u8; 32]) -> Vec<NodeId> {
        todo!()
    }

    pub fn rank_ids(&self, key: &[u8; 32]) -> Vec<NodeId> {
        todo!()
    }

    pub fn is_owner(&self, key: &[u8; 32], id: &NodeId) -> bool {
        todo!()
    }
}
