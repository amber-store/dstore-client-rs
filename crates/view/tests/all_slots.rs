//! Golden digests of `placement/all_slots.json` (verification.md §4.3 item 9; VECTORS.md family `placement`).
//!
//! For each set: BLAKE3-256, for slot = 0 … 2^20−1 in order, over `u8 len(rank) ‖ rank indexes as u8 ‖
//! u8 len(owners) ‖ owners as u8`, with `rank = Set::rank(slot)` and `owners = Set::owners(slot, r)`.
//!
//! These digests live here rather than in root `tests/golden_tests/view.rs` because they need BLAKE3, which
//! only this crate depends on. The root module checks the same file's spot ranks. Slots are ranked on
//! several threads and hashed in slot order.

use std::ops::Range;

use dstore_testkit::golden;
use dstore_view::NodeId;
use dstore_view::placement::{Member, SLOTS, Set};
use serde::Deserialize;

#[derive(Deserialize)]
struct AllSlots {
    slots: u32,
    sets: Vec<SetCase>,
}

#[derive(Deserialize)]
struct SetCase {
    name: String,
    members: Option<Vec<MemberJson>>,
    r: usize,
    all_slots_blake3: String,
}

#[derive(Deserialize)]
struct MemberJson {
    id: String,
    weight: u32,
    zone: String,
}

/// The hashed byte stream of `slots`.
fn stream(set: &Set, r: usize, slots: Range<u32>) -> Vec<u8> {
    let mut out = Vec::new();
    for slot in slots {
        let rank = set.rank(slot);
        let owners = set.owners(slot, r);
        out.push(rank.len() as u8);
        out.extend(rank.iter().map(|&i| i as u8));
        out.push(owners.len() as u8);
        out.extend(owners.iter().map(|&i| i as u8));
    }
    out
}

fn all_slots_digest(set: &Set, r: usize) -> String {
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get().min(16)) as u32;
    let chunk = SLOTS.div_ceil(threads);
    let parts: Vec<Vec<u8>> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|t| {
                let range = (t * chunk).min(SLOTS)..((t + 1) * chunk).min(SLOTS);
                scope.spawn(move || stream(set, r, range))
            })
            .collect();
        workers
            .into_iter()
            .map(|w| match w.join() {
                Ok(part) => part,
                Err(_) => panic!("a ranking thread panicked"),
            })
            .collect()
    });
    let mut hasher = blake3::Hasher::new();
    for part in &parts {
        hasher.update(part);
    }
    hasher.finalize().to_hex().to_string()
}

#[test]
fn all_slots_blake3() {
    let a: AllSlots = golden::load_json("placement/all_slots.json");
    assert_eq!(a.slots, SLOTS);
    assert!(!a.sets.is_empty());
    for s in &a.sets {
        let members: Vec<Member> = s
            .members
            .iter()
            .flatten()
            .map(|m| {
                let id = golden::hex(&m.id);
                assert_eq!(id.len(), 32, "{}: id {}", s.name, m.id);
                Member {
                    id: NodeId::from_slice_lossy(&id),
                    weight: m.weight,
                    zone: m.zone.as_bytes().to_vec(),
                }
            })
            .collect();
        assert!(members.len() <= 255, "{}: too many members to hash", s.name);
        let set = Set::new(members);
        assert_eq!(
            all_slots_digest(&set, s.r),
            s.all_slots_blake3,
            "{}",
            s.name
        );
    }
}
