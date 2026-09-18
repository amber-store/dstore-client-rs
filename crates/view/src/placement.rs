//! `placement/placement.go`: slots, salts, the rational rank order, owners and cached tables.
//!
//! Every computation is integer-only, as in Go, so rankings are bit-exact on every CPU. Go's `Ranked`
//! type is declared but never used, so it is not ported.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

pub const SLOT_BITS: u32 = 20;
pub const SLOTS: u32 = 1 << SLOT_BITS;

/// The BLAKE3 domain prefix of the per-node salt (Go `saltDomain`). It is hashed as its 24 bytes, with
/// no NUL and no length prefix.
pub const SALT_DOMAIN: &str = "amber-dstore/placement/1";

/// A node id: the node's ed25519 public key bytes (`placement.NodeID`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(pub [u8; 32]);

impl NodeId {
    /// Go `copy()`: min(len, 32) bytes, zero-padded.
    pub fn from_slice_lossy(b: &[u8]) -> NodeId {
        let mut id = [0u8; 32];
        for (dst, src) in id.iter_mut().zip(b) {
            *dst = *src;
        }
        NodeId(id)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// `view.IDString`.
    pub fn to_hex(&self) -> String {
        dstore_gocompat::hex::encode(&self.0)
    }

    /// `view.ShortID`: hex of bytes 0..4.
    pub fn short(&self) -> String {
        dstore_gocompat::hex::encode(&self.0[..4])
    }
}

impl From<[u8; 32]> for NodeId {
    fn from(b: [u8; 32]) -> NodeId {
        NodeId(b)
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

/// `placement.Slot`: the top 20 bits of the big-endian u64 at `key[24..32]`.
pub fn slot(key: &[u8; 32]) -> u32 {
    let tail = u64::from_be_bytes([
        key[24], key[25], key[26], key[27], key[28], key[29], key[30], key[31],
    ]);
    (tail >> (64 - SLOT_BITS)) as u32
}

/// `placement.Salt`: the first 8 bytes of `BLAKE3(SALT_DOMAIN ‖ id)`, big-endian. Go reads them from the
/// XOF, whose first bytes are the 256-bit digest's.
pub fn salt(id: &NodeId) -> u64 {
    let mut h = blake3::Hasher::new();
    h.update(SALT_DOMAIN.as_bytes());
    h.update(&id.0);
    let digest = h.finalize();
    let b = digest.as_bytes();
    u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
}

/// `placement.Fmix64`: the murmur3 64-bit finalizer.
pub fn fmix64(mut x: u64) -> u64 {
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
    x ^= x >> 33;
    x = x.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    x ^= x >> 33;
    x
}

/// `placement.Log2Fix`: ⌊log2(x)·2^32⌋ by bit-by-bit squaring. Panics "placement: Log2Fix(0)", as Go
/// does; [`l`] never passes 0.
pub fn log2fix(x: u64) -> u64 {
    if x == 0 {
        panic!("placement: Log2Fix(0)");
    }
    let i = u64::from(63 - x.leading_zeros());
    let mut m = x << (63 - i);
    let mut f = 0u64;
    for j in 1..=32u32 {
        let p = u128::from(m) * u128::from(m);
        let hi = (p >> 64) as u64;
        let lo = p as u64;
        if hi & (1 << 63) != 0 {
            m = hi;
            f |= 1 << (32 - j);
        } else {
            m = (hi << 1) | (lo >> 63);
        }
    }
    (i << 32) | f
}

/// `placement.L`: `64·2^32 − log2fix(h+1)` with `h = fmix64(slot ⊕ salt)`; `h = 2^64−1` gives 0.
pub fn l(slot: u32, salt: u64) -> u64 {
    let h = fmix64(u64::from(slot) ^ salt);
    if h == u64::MAX {
        return 0;
    }
    (64u64 << 32) - log2fix(h + 1)
}

/// Go `less(a, b, la, lb)` as an ordering: `Less` when `a` ranks above `b`. The full 128-bit products
/// `a.weight·lb` and `b.weight·la` compare (the larger ranks above), then the smaller id. This is the
/// exact rational order of `w/L` with `L = 0` as +∞, a strict weak ordering.
fn rank_order(a: &Member, la: u64, b: &Member, lb: u64) -> Ordering {
    let pa = u128::from(a.weight) * u128::from(lb);
    let pb = u128::from(b.weight) * u128::from(la);
    pb.cmp(&pa).then_with(|| a.id.cmp(&b.id))
}

/// `placement.Set`.
pub struct Set {
    members: Vec<Member>,
    salts: Vec<u64>,
}

impl Set {
    /// `placement.NewSet`: a salt for every member, weight-0 members included.
    pub fn new(members: Vec<Member>) -> Set {
        let salts = members.iter().map(|m| salt(&m.id)).collect();
        Set { members, salts }
    }

    pub fn members(&self) -> &[Member] {
        &self.members
    }

    /// `Set.Rank`: weighted members by descending score, then weight-0 members by id.
    ///
    /// Go sorts the weighted members with `sort.SliceStable`; [`rank_order`] is a strict weak ordering, so
    /// every stable sort gives the same permutation. Go sorts the weight-0 members with the unstable
    /// `sort.Slice`, whose order among members sharing an id shows in the indexes, so they go through the
    /// port of Go's pdqsort.
    pub fn rank(&self, slot: u32) -> Vec<u32> {
        let mut weighted: Vec<(usize, &Member, u64)> = Vec::with_capacity(self.members.len());
        let mut zero: Vec<(usize, &Member)> = Vec::new();
        for (i, (m, &s)) in self.members.iter().zip(&self.salts).enumerate() {
            if m.weight == 0 {
                zero.push((i, m));
            } else {
                weighted.push((i, m, l(slot, s)));
            }
        }
        weighted.sort_by(|a, b| rank_order(a.1, a.2, b.1, b.2));
        crate::gosort::slice(&mut zero, |a, b| a.1.id < b.1.id);
        weighted
            .iter()
            .map(|e| e.0)
            .chain(zero.iter().map(|e| e.0))
            .map(|i| i as u32)
            .collect()
    }

    /// `Set.Owners`: the first `r` ranked members whose zone key is not taken yet, stopping at the first
    /// weight-0 member. The zone key is the zone, or the 32 id bytes when the zone is empty, so an
    /// explicit zone equal to another member's id bytes collides, as in Go.
    pub fn owners(&self, slot: u32, r: usize) -> Vec<u32> {
        let rank = self.rank(slot);
        let mut out = Vec::with_capacity(r.min(rank.len()));
        let mut zones: HashSet<&[u8]> = HashSet::new();
        for i in rank {
            if out.len() >= r {
                break;
            }
            let Some(m) = self.members.get(i as usize) else {
                break;
            };
            if m.weight == 0 {
                break;
            }
            let zone: &[u8] = if m.zone.is_empty() { &m.id.0 } else { &m.zone };
            if !zones.insert(zone) {
                continue;
            }
            out.push(i);
        }
        out
    }
}

/// `placement.Table`: a set with lazily cached owners and ranks per slot.
///
/// Go preallocates two 2^20-entry slices per table and indexes them by slot (a slot ≥ `SLOTS` panics
/// there); this cache fills a map on first use instead. Concurrent callers may compute a slot twice, as in
/// Go; the first stored result wins and is identical.
pub struct Table {
    set: Set,
    r: usize,
    owners: RwLock<HashMap<u32, Arc<[u32]>>>,
    rank: RwLock<HashMap<u32, Arc<[u32]>>>,
}

/// The cached value of `slot`, computed outside the lock on a miss. A poisoned lock still serves its map:
/// entries are only ever inserted whole.
fn cached(
    cache: &RwLock<HashMap<u32, Arc<[u32]>>>,
    slot: u32,
    compute: impl FnOnce() -> Vec<u32>,
) -> Arc<[u32]> {
    let hit = match cache.read() {
        Ok(g) => g.get(&slot).cloned(),
        Err(poisoned) => poisoned.into_inner().get(&slot).cloned(),
    };
    if let Some(v) = hit {
        return v;
    }
    let v: Arc<[u32]> = Arc::from(compute());
    let mut g = match cache.write() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    g.entry(slot).or_insert(v).clone()
}

impl Table {
    /// `placement.NewTable`.
    pub fn new(set: Set, r: usize) -> Table {
        Table {
            set,
            r,
            owners: RwLock::new(HashMap::new()),
            rank: RwLock::new(HashMap::new()),
        }
    }

    pub fn set(&self) -> &Set {
        &self.set
    }

    pub fn replicas(&self) -> usize {
        self.r
    }

    /// `Table.Owners`, cached.
    pub fn owners(&self, slot: u32) -> Arc<[u32]> {
        cached(&self.owners, slot, || self.set.owners(slot, self.r))
    }

    /// `Table.Rank`, cached.
    pub fn rank(&self, slot: u32) -> Arc<[u32]> {
        cached(&self.rank, slot, || self.set.rank(slot))
    }

    /// `Table.OwnerIDs`.
    pub fn owner_ids(&self, key: &[u8; 32]) -> Vec<NodeId> {
        self.ids(&self.owners(slot(key)))
    }

    /// `Table.RankIDs`: the full read order under this set, weight-0 members last.
    pub fn rank_ids(&self, key: &[u8; 32]) -> Vec<NodeId> {
        self.ids(&self.rank(slot(key)))
    }

    /// `Table.IsOwner`.
    pub fn is_owner(&self, key: &[u8; 32], id: &NodeId) -> bool {
        self.owners(slot(key)).iter().any(|&j| {
            self.set
                .members
                .get(j as usize)
                .is_some_and(|m| m.id == *id)
        })
    }

    fn ids(&self, idx: &[u32]) -> Vec<NodeId> {
        idx.iter()
            .filter_map(|&j| self.set.members.get(j as usize).map(|m| m.id))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id_from(b: u8) -> NodeId {
        NodeId([b; 32])
    }

    fn member(b: u8, weight: u32, zone: &str) -> Member {
        Member {
            id: id_from(b),
            weight,
            zone: zone.as_bytes().to_vec(),
        }
    }

    /// splitmix64 (core-rs VECTORS.md), standing in for Go's `rand.New(rand.NewSource(1))`.
    fn splitmix(state: &mut u64) -> u64 {
        *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = *state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    // placement_test.go TestLog2FixExact.
    #[test]
    fn log2fix_exact() {
        for i in 0..64u32 {
            assert_eq!(log2fix(1u64 << i), u64::from(i) << 32, "Log2Fix(2^{i})");
        }
        // The fixed-point result must be within one ulp of 2^-32 below the true value; float64 has about
        // 53 bits, so allow its slack near 2^64.
        let mut state = 1u64;
        for _ in 0..10_000 {
            let x = splitmix(&mut state);
            if x == 0 {
                continue;
            }
            let want = (x as f64).log2() * 4_294_967_296.0;
            let got = log2fix(x) as f64;
            assert!(
                got <= want + 1.0 && got >= want - 4096.0,
                "Log2Fix({x}) = {got}, float says {want}"
            );
        }
    }

    #[test]
    #[should_panic(expected = "placement: Log2Fix(0)")]
    fn log2fix_zero_panics() {
        log2fix(0);
    }

    // placement_test.go TestFmix64Vectors.
    #[test]
    fn fmix64_vectors() {
        let cases: [(u64, u64); 4] = [
            (0, 0),
            (1, 0xb456_bcfc_34c2_cb2c),
            (0xdead_beef, 0xd24b_d59f_862a_1dac),
            (0xffff_ffff_ffff_ffff, 0x64b5_720b_4b82_5f21),
        ];
        for (input, want) in cases {
            assert_eq!(fmix64(input), want, "Fmix64({input:#x})");
        }
    }

    // placement_test.go TestGoldenVectors: the frozen ranks, the salt of idFrom(1) and L(12345, salt1).
    #[test]
    fn golden_vectors() {
        let set = Set::new(vec![
            member(0x01, 1000, ""),
            member(0x02, 2000, ""),
            member(0x03, 500, ""),
            member(0x04, 4000, ""),
            member(0x05, 1000, ""),
        ]);
        let frozen: [(u32, [u32; 5]); 6] = [
            (0, [3, 4, 2, 1, 0]),
            (1, [3, 0, 4, 1, 2]),
            (12345, [1, 3, 4, 2, 0]),
            (0xfffff, [3, 1, 4, 0, 2]),
            (524288, [3, 0, 1, 4, 2]),
            (777777, [4, 1, 2, 3, 0]),
        ];
        for (slot, want) in frozen {
            assert_eq!(set.rank(slot), want, "slot {slot}");
        }
        assert_eq!(salt(&id_from(0x01)), 0x91d3_f42d_715b_dc8a);
        assert_eq!(l(12345, salt(&id_from(0x01))), 0x2_2356_754f);
    }

    // placement_test.go TestWeightedDistribution.
    #[test]
    fn weighted_distribution() {
        const N: u32 = 200_000;
        let members = vec![
            member(1, 100, ""),
            member(2, 200, ""),
            member(3, 300, ""),
            member(4, 400, ""),
        ];
        let total: u32 = members.iter().map(|m| m.weight).sum();
        let set = Set::new(members.clone());
        let mut counts = [0u32; 4];
        for slot in 0..N {
            if let Some(&first) = set.rank(slot).first() {
                counts[first as usize] += 1;
            }
        }
        for (i, m) in members.iter().enumerate() {
            let want = f64::from(N) * f64::from(m.weight) / f64::from(total);
            let got = f64::from(counts[i]);
            assert!(
                (got - want).abs() / want <= 0.03,
                "member {i}: {got} wins, want ~{want}"
            );
        }
    }

    // placement_test.go TestAddingNodeMovesOnlyToIt.
    #[test]
    fn adding_node_moves_only_to_it() {
        let base = vec![
            member(1, 100, ""),
            member(2, 100, ""),
            member(3, 100, ""),
            member(4, 100, ""),
        ];
        let mut grown = base.clone();
        grown.push(member(5, 100, ""));
        let a = Set::new(base.clone());
        let b = Set::new(grown.clone());
        let (mut moved, mut total) = (0u32, 0u32);
        for slot in 0..50_000u32 {
            let oa = a.owners(slot, 3);
            let ob = b.owners(slot, 3);
            total += 1;
            // Every owner in the old set either stays an owner or was displaced by 5.
            for &i in &oa {
                let found = ob
                    .iter()
                    .any(|&j| base[i as usize].id == grown[j as usize].id);
                if !found {
                    moved += 1;
                    let has5 = ob.iter().any(|&j| grown[j as usize].id == id_from(5));
                    assert!(has5, "slot {slot}: owner moved but not to the new node");
                }
            }
        }
        assert!(moved != 0 && moved <= total, "moved {moved} of {total}");
    }

    // placement_test.go TestZoneRule.
    #[test]
    fn zone_rule() {
        let members = vec![
            member(1, 100, "a"),
            member(2, 100, "a"),
            member(3, 100, "b"),
            member(4, 100, "b"),
            member(5, 0, "c"),
        ];
        let set = Set::new(members.clone());
        for slot in 0..2000u32 {
            let o = set.owners(slot, 3);
            assert_eq!(o.len(), 2, "slot {slot}: {} owners with two zones", o.len());
            assert_ne!(
                members[o[0] as usize].zone, members[o[1] as usize].zone,
                "slot {slot}: same zone twice"
            );
            let r = set.rank(slot);
            assert_eq!(r.last(), Some(&4), "weight-0 member must rank last");
        }
    }

    // placement_test.go TestSlot.
    #[test]
    fn slot_of_tails() {
        let mut k = [0u8; 32];
        k[24..].copy_from_slice(&u64::MAX.to_be_bytes());
        assert_eq!(slot(&k), SLOTS - 1, "slot of all-ones tail");
        k[24..].copy_from_slice(&(1u64 << 44).to_be_bytes());
        assert_eq!(slot(&k), 1, "slot of 2^44");
    }

    // Go orders weight-0 members that share an id with the unstable sort.Slice: indexes from a scratch
    // go1.26.5 program printing dstore v0.1.9 placement.NewSet(members).Rank(slot) (deleted afterwards).
    // Both sets have more than 12 weight-0 members, where a stable sort gives other indexes.
    #[test]
    fn weight_zero_members_in_go_sort_slice_order() {
        let cases: [(u32, &str, &str); 2] = [
            (
                718459,
                "02:0,00:0,00:0,00:0,00:0,00:2,00:3,02:0,01:0,01:0,01:0,00:0,02:2,02:0,00:3,01:3,02:0,00:0,01:2,02:0,02:0,00:0,02:0,00:0",
                "12,6,14,15,5,18,17,21,2,3,4,23,1,11,8,9,10,13,0,19,20,16,22,7",
            ),
            (
                908465,
                "01:1,00:0,01:0,00:0,00:0,01:0,00:0,00:2,00:3,00:0,01:0,00:0,01:1,01:0,00:0,00:0,00:0,01:0,00:0,01:0,00:0,00:0",
                "8,7,0,12,16,14,3,4,21,6,9,20,18,11,1,15,13,17,2,19,10,5",
            ),
        ];
        for (slot, members, want) in cases {
            let members: Vec<Member> = members
                .split(',')
                .map(|m| {
                    let (b, w) = m.split_once(':').unwrap_or_default();
                    member(
                        u8::from_str_radix(b, 16).unwrap_or_default(),
                        w.parse().unwrap_or_default(),
                        "",
                    )
                })
                .collect();
            let got: Vec<String> = Set::new(members)
                .rank(slot)
                .iter()
                .map(u32::to_string)
                .collect();
            assert_eq!(got.join(","), want, "slot {slot}");
        }
    }

    #[test]
    fn node_id_forms() {
        let long: Vec<u8> = (1..=40).collect();
        let id = NodeId::from_slice_lossy(&long);
        assert_eq!(id.0.to_vec(), long[..32].to_vec());
        let short = NodeId::from_slice_lossy(&[0xab, 0xcd]);
        assert_eq!(short.0[..2], [0xab, 0xcd]);
        assert!(short.0[2..].iter().all(|&b| b == 0));
        assert_eq!(NodeId::from_slice_lossy(&[]), NodeId::default());
        assert_eq!(short.short(), "abcd0000");
        assert_eq!(id.to_hex().len(), 64);
        assert!(id.to_hex().starts_with("0102030405"));
        assert_eq!(NodeId::from([7u8; 32]).as_bytes(), &[7u8; 32]);
    }

    #[test]
    fn empty_set_and_zero_replicas() {
        let empty = Set::new(Vec::new());
        assert!(empty.rank(0).is_empty());
        assert!(empty.owners(0, 3).is_empty());
        let set = Set::new(vec![member(1, 1, ""), member(2, 1, "")]);
        assert!(set.owners(0, 0).is_empty());
        assert_eq!(set.owners(0, usize::MAX).len(), 2);
    }

    #[test]
    fn explicit_zone_collides_with_raw_id_bytes() {
        // Member 2's explicit zone equals member 1's 32 id bytes: only one of them can own.
        let set = Set::new(vec![member(0x61, 1, ""), member(2, 1, &"a".repeat(32))]);
        for slot in 0..64 {
            assert_eq!(set.owners(slot, 2).len(), 1, "slot {slot}");
        }
    }

    #[test]
    fn table_caches_set_results() {
        let set = Set::new(vec![
            member(1, 1000, "x"),
            member(2, 2000, ""),
            member(3, 0, ""),
            member(4, 500, "x"),
        ]);
        let expect_rank: Vec<Vec<u32>> = (0..32).map(|s| set.rank(s)).collect();
        let expect_owners: Vec<Vec<u32>> = (0..32).map(|s| set.owners(s, 2)).collect();
        let table = Table::new(set, 2);
        assert_eq!(table.replicas(), 2);
        assert_eq!(table.set().members().len(), 4);
        for s in 0..32u32 {
            let first = table.owners(s);
            assert_eq!(first.to_vec(), expect_owners[s as usize]);
            assert!(Arc::ptr_eq(&first, &table.owners(s)));
            assert_eq!(table.rank(s).to_vec(), expect_rank[s as usize]);
        }
        let mut key = [0u8; 32];
        key[24..].copy_from_slice(&(5u64 << 44).to_be_bytes());
        let owners = table.owner_ids(&key);
        let want: Vec<NodeId> = expect_owners[5]
            .iter()
            .map(|&j| table.set().members()[j as usize].id)
            .collect();
        assert_eq!(owners, want);
        assert_eq!(table.rank_ids(&key).len(), 4);
        assert_eq!(table.rank_ids(&key).last(), Some(&id_from(3)));
        for m in table.set().members() {
            assert_eq!(table.is_owner(&key, &m.id), want.contains(&m.id));
        }
    }
}
