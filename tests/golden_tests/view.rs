//! Golden tests of `dstore-view` (`view/view_placement.json`, `placement/all_slots.json`).
//!
//! The schemas are in VECTORS.md (families `view` and `placement`). The 2^20-slot BLAKE3 digests of
//! `placement/all_slots.json` need BLAKE3, which only `dstore-view` depends on, so
//! `crates/view/tests/all_slots.rs` checks them; this module checks that file's spot ranks and owners.

use std::cmp::Ordering;
use std::sync::{Arc, OnceLock};

use dstore_testkit::golden::{self, decimal_u64};
use dstore_view::placement::{self, Member, Set, Table};
use dstore_view::{Acl, DataEndpoint, Former, Node, NodeId, Pending, Placement, Ramp, View, Voter};
use serde::{Deserialize, Deserializer};

/// A JSON list that Go writes as `null` when its slice is nil, where nil and empty mean the same.
fn null_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

fn hex(b: &[u8]) -> String {
    dstore_gocompat::hex::encode(b)
}

fn hex_id(s: &str) -> NodeId {
    let b = golden::hex(s);
    assert_eq!(b.len(), 32, "vector id {s} is not 32 bytes");
    NodeId::from_slice_lossy(&b)
}

fn key32(s: &str) -> [u8; 32] {
    hex_id(s).0
}

fn hexes(ids: &[NodeId]) -> Vec<String> {
    ids.iter().map(NodeId::to_hex).collect()
}

fn opt_bytes(s: Option<&str>) -> Option<Vec<u8>> {
    s.map(golden::hex)
}

fn decode(name: &str, cbor: &str) -> View {
    match View::decode(&golden::hex(cbor)) {
        Ok(v) => v,
        Err(e) => panic!("{name}: {e}"),
    }
}

#[derive(Deserialize)]
struct Vectors {
    constants: Constants,
    fmix64: Vec<U64Pair>,
    log2fix: Vec<U64Pair>,
    slot: Vec<SlotCase>,
    salt: Vec<SaltCase>,
    l: Vec<LCase>,
    sets: Vec<SetCase>,
    views: Vec<ViewCase>,
    view_cbor: Vec<CborCase>,
    view_decode: Vec<DecodeCase>,
    parse_node_id: Vec<ParseCase>,
    short_id: Vec<ShortCase>,
    ticket_from_view: Vec<TicketCase>,
    helpers: Vec<HelpersCase>,
    node_helpers: Vec<NodeHelperCase>,
    id_lists: Vec<IdListCase>,
    ids_of: Vec<IdsOfCase>,
    compare: Vec<CompareCase>,
    default_min_replicas: Vec<MinReplicasCase>,
    validate_change: Vec<ValidateCase>,
    sort_nodes: SortNodesCase,
    status_cluster_prefix: Vec<PrefixCase>,
}

fn vectors() -> &'static Vectors {
    static VECTORS: OnceLock<Vectors> = OnceLock::new();
    VECTORS.get_or_init(|| golden::load_json("view/view_placement.json"))
}

#[derive(Deserialize)]
struct Constants {
    slot_bits: u32,
    slots: u32,
    salt_domain: String,
    voter_sync_done: i64,
    voter_sync_pending: i64,
    log2fix_zero_panic: String,
}

#[derive(Deserialize)]
struct U64Pair {
    #[serde(rename = "in", deserialize_with = "decimal_u64")]
    input: u64,
    #[serde(deserialize_with = "decimal_u64")]
    out: u64,
}

#[derive(Deserialize)]
struct SlotCase {
    key: String,
    slot: u32,
}

#[derive(Deserialize)]
struct SaltCase {
    id: String,
    #[serde(deserialize_with = "decimal_u64")]
    salt: u64,
}

#[derive(Deserialize)]
struct LCase {
    slot: u32,
    #[serde(deserialize_with = "decimal_u64")]
    salt: u64,
    #[serde(deserialize_with = "decimal_u64")]
    l: u64,
}

#[derive(Deserialize, Debug)]
struct MemberJson {
    id: Option<String>,
    weight: u32,
    zone: String,
}

fn placement_members(members: &[MemberJson]) -> Vec<Member> {
    members
        .iter()
        .map(|m| Member {
            id: hex_id(m.id.as_deref().unwrap_or_default()),
            weight: m.weight,
            zone: m.zone.as_bytes().to_vec(),
        })
        .collect()
}

fn view_nodes(members: &[MemberJson]) -> Vec<Node> {
    members
        .iter()
        .map(|m| Node {
            id: opt_bytes(m.id.as_deref()),
            weight: m.weight,
            zone: m.zone.clone(),
            ..Node::default()
        })
        .collect()
}

#[derive(Deserialize)]
struct SetCase {
    name: String,
    #[serde(default, deserialize_with = "null_default")]
    members: Vec<MemberJson>,
    replicas: Vec<usize>,
    slots: Vec<SetSlot>,
}

#[derive(Deserialize)]
struct SetSlot {
    slot: u32,
    #[serde(default, deserialize_with = "null_default")]
    l: Vec<Option<String>>,
    #[serde(default, deserialize_with = "null_default")]
    rank: Vec<u32>,
    owners: Vec<OwnersCase>,
}

#[derive(Deserialize)]
struct OwnersCase {
    r: usize,
    #[serde(default, deserialize_with = "null_default")]
    owners: Vec<u32>,
}

#[derive(Deserialize)]
struct ViewCase {
    name: String,
    cbor: String,
    keys: Vec<KeyCase>,
}

#[derive(Deserialize)]
struct KeyCase {
    key: String,
    slot: u32,
    #[serde(default, deserialize_with = "null_default")]
    owners: Vec<String>,
    pending_owners: Option<Vec<String>>,
    #[serde(default, deserialize_with = "null_default")]
    write_set: Vec<String>,
    #[serde(default, deserialize_with = "null_default")]
    read_order: Vec<String>,
    #[serde(default, deserialize_with = "null_default")]
    flags: Vec<FlagCase>,
}

#[derive(Deserialize)]
struct FlagCase {
    id: String,
    is_owner: bool,
    is_pending_owner: bool,
    in_write_set: bool,
}

#[derive(Deserialize)]
struct CborCase {
    name: String,
    cbor: String,
}

#[derive(Deserialize)]
struct DecodeCase {
    name: String,
    input: String,
    ok: bool,
    reencode: Option<String>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct ParseCase {
    input_hex: String,
    ok: bool,
    id: Option<String>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct ShortCase {
    bytes: Option<String>,
    node_short: String,
    view_short: Option<String>,
    id_string: Option<String>,
}

#[derive(Deserialize)]
struct TicketCase {
    name: String,
    view: String,
    ticket: String,
    ticket_cbor: String,
}

#[derive(Deserialize)]
struct HelpersCase {
    name: String,
    view: String,
    #[serde(default, deserialize_with = "null_default")]
    all_members: Vec<String>,
    #[serde(default, deserialize_with = "null_default")]
    voter_ids: Vec<String>,
    quorum: usize,
    #[serde(default, deserialize_with = "null_default")]
    lookups: Vec<LookupCase>,
}

#[derive(Deserialize)]
struct LookupCase {
    id: String,
    node_found: bool,
    node_weight: u32,
    #[serde(default, deserialize_with = "null_default")]
    node_addrs: Vec<String>,
    is_member: bool,
    data_endpoint_owner: Option<String>,
    is_former: bool,
    is_voter: bool,
}

#[derive(Deserialize)]
struct NodeHelperCase {
    id: Option<String>,
    zone: String,
    nid: String,
    zone_or_id: String,
}

#[derive(Deserialize)]
struct IdListCase {
    #[serde(default, deserialize_with = "null_default")]
    ids: Vec<Option<String>>,
    id: String,
    contains: bool,
    #[serde(default, deserialize_with = "null_default")]
    add_id: Vec<Option<String>>,
}

#[derive(Deserialize)]
struct IdsOfCase {
    #[serde(default, deserialize_with = "null_default")]
    raw: Vec<String>,
    #[serde(default, deserialize_with = "null_default")]
    ids: Vec<String>,
}

#[derive(Deserialize)]
struct CompareCase {
    #[serde(deserialize_with = "decimal_u64")]
    view_incarnation: u64,
    #[serde(deserialize_with = "decimal_u64")]
    view_epoch: u64,
    #[serde(deserialize_with = "decimal_u64")]
    incarnation: u64,
    #[serde(deserialize_with = "decimal_u64")]
    epoch: u64,
    result: i32,
}

#[derive(Deserialize)]
struct MinReplicasCase {
    r: u8,
    min_replicas: u8,
}

#[derive(Deserialize)]
struct ValidateCase {
    name: String,
    #[serde(default, deserialize_with = "null_default")]
    cur: Vec<MemberJson>,
    #[serde(default, deserialize_with = "null_default")]
    target: Vec<MemberJson>,
    replicas: i64,
    force: bool,
    error: Option<String>,
}

#[derive(Deserialize)]
struct SortNodesCase {
    input: Vec<Option<String>>,
    output: Vec<Option<String>>,
}

#[derive(Deserialize)]
struct PrefixCase {
    name: String,
    view: String,
    cluster_id: Option<String>,
    cap: usize,
    prefix: Option<String>,
    panic: Option<String>,
}

#[derive(Deserialize)]
struct AllSlots {
    slots: u32,
    sets: Vec<AllSlotsSet>,
}

#[derive(Deserialize)]
struct AllSlotsSet {
    name: String,
    #[serde(default, deserialize_with = "null_default")]
    members: Vec<MemberJson>,
    r: usize,
    spots: Vec<Spot>,
    all_slots_blake3: String,
}

#[derive(Deserialize)]
struct Spot {
    slot: u32,
    #[serde(default, deserialize_with = "null_default")]
    rank: Vec<u32>,
    #[serde(default, deserialize_with = "null_default")]
    owners: Vec<u32>,
}

#[test]
fn constants() {
    let c = &vectors().constants;
    assert_eq!(placement::SLOT_BITS, c.slot_bits);
    assert_eq!(placement::SLOTS, c.slots);
    assert_eq!(placement::SALT_DOMAIN, c.salt_domain);
    assert_eq!(dstore_view::VOTER_SYNC_DONE, c.voter_sync_done);
    assert_eq!(dstore_view::VOTER_SYNC_PENDING, c.voter_sync_pending);
    let payload = match std::panic::catch_unwind(|| placement::log2fix(0)) {
        Ok(v) => panic!("log2fix(0) returned {v}"),
        Err(payload) => payload,
    };
    let text = payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned());
    assert_eq!(text.as_deref(), Some(c.log2fix_zero_panic.as_str()));
}

#[test]
fn fmix64() {
    let cases = &vectors().fmix64;
    assert!(!cases.is_empty());
    for c in cases {
        assert_eq!(placement::fmix64(c.input), c.out, "fmix64({})", c.input);
    }
}

#[test]
fn log2fix() {
    let cases = &vectors().log2fix;
    assert!(cases.len() >= 1000);
    for c in cases {
        assert_eq!(placement::log2fix(c.input), c.out, "log2fix({})", c.input);
    }
}

#[test]
fn slot() {
    let cases = &vectors().slot;
    assert!(!cases.is_empty());
    for c in cases {
        assert_eq!(placement::slot(&key32(&c.key)), c.slot, "slot({})", c.key);
    }
}

#[test]
fn salt() {
    let cases = &vectors().salt;
    assert!(!cases.is_empty());
    for c in cases {
        assert_eq!(placement::salt(&hex_id(&c.id)), c.salt, "salt({})", c.id);
    }
}

#[test]
fn l() {
    let cases = &vectors().l;
    assert!(!cases.is_empty());
    for c in cases {
        assert_eq!(
            placement::l(c.slot, c.salt),
            c.l,
            "l({}, {})",
            c.slot,
            c.salt
        );
    }
}

#[test]
fn sets() {
    let sets = &vectors().sets;
    assert!(!sets.is_empty());
    for s in sets {
        let members = placement_members(&s.members);
        let set = Set::new(members.clone());
        assert_eq!(set.members(), members.as_slice(), "{}", s.name);
        let tables: Vec<Table> = s
            .replicas
            .iter()
            .map(|&r| Table::new(Set::new(members.clone()), r))
            .collect();
        for sl in &s.slots {
            let at = format!("{} slot {}", s.name, sl.slot);
            let ls: Vec<Option<String>> = members
                .iter()
                .map(|m| {
                    (m.weight > 0)
                        .then(|| placement::l(sl.slot, placement::salt(&m.id)).to_string())
                })
                .collect();
            assert_eq!(ls, sl.l, "{at}: l");
            assert_eq!(set.rank(sl.slot), sl.rank, "{at}: rank");
            assert_eq!(sl.owners.len(), s.replicas.len(), "{at}: owner lists");
            for (o, table) in sl.owners.iter().zip(&tables) {
                assert_eq!(table.replicas(), o.r, "{at}: r");
                assert_eq!(set.owners(sl.slot, o.r), o.owners, "{at}: owners r={}", o.r);
                assert_eq!(
                    table.owners(sl.slot).to_vec(),
                    o.owners,
                    "{at}: table owners"
                );
                assert_eq!(table.rank(sl.slot).to_vec(), sl.rank, "{at}: table rank");
            }
        }
    }
}

#[test]
fn views() {
    let views = &vectors().views;
    assert!(!views.is_empty());
    for vc in views {
        let v = Arc::new(decode(&vc.name, &vc.cbor));
        assert_eq!(hex(&v.encode()), vc.cbor, "{}: re-encoding", vc.name);
        let p = Placement::new(Arc::clone(&v));
        assert!(Arc::ptr_eq(p.view(), &v));
        for k in &vc.keys {
            let key = key32(&k.key);
            let at = format!("{} key {}", vc.name, k.key);
            assert_eq!(placement::slot(&key), k.slot, "{at}: slot");
            assert_eq!(hexes(&p.owners(&key)), k.owners, "{at}: owners");
            assert_eq!(
                p.pending_owners(&key).map(|o| hexes(&o)),
                k.pending_owners,
                "{at}: pending_owners"
            );
            assert_eq!(hexes(&p.write_set(&key)), k.write_set, "{at}: write_set");
            assert_eq!(hexes(&p.read_order(&key)), k.read_order, "{at}: read_order");
            assert!(!k.flags.is_empty(), "{at}: flags");
            for f in &k.flags {
                let id = hex_id(&f.id);
                assert_eq!(p.is_owner(&key, &id), f.is_owner, "{at} {}: is_owner", f.id);
                assert_eq!(
                    p.is_pending_owner(&key, &id),
                    f.is_pending_owner,
                    "{at} {}: is_pending_owner",
                    f.id
                );
                assert_eq!(
                    p.in_write_set(&key, &id),
                    f.in_write_set,
                    "{at} {}: in_write_set",
                    f.id
                );
            }
        }
    }
}

#[test]
fn view_cbor_round_trips() {
    let cases = &vectors().view_cbor;
    assert!(!cases.is_empty());
    for c in cases {
        let v = decode(&c.name, &c.cbor);
        assert_eq!(hex(&v.encode()), c.cbor, "{}", c.name);
        assert_eq!(
            hex(&dstore_codec::marshal(&v)),
            c.cbor,
            "{}: marshal",
            c.name
        );
    }
}

/// `ids[i]` of the generator (`tools/vectorgen/family_view.go`): splitmix `data(300 + i, 32)`.
fn gen_id(i: u64) -> Vec<u8> {
    dstore_testkit::splitmix::data(300 + i, 32)
}

/// The generator's `initView`.
fn init_view() -> View {
    View {
        cluster_id: Some(dstore_testkit::splitmix::data(1000, 16)),
        incarnation: 1,
        epoch: 1,
        version: 1,
        placement_epoch: 1,
        replicas: 3,
        min_replicas: 2,
        voters: Some(vec![Voter {
            id: Some(gen_id(0)),
            since: 1,
        }]),
        nodes: Some(vec![Node {
            id: Some(gen_id(0)),
            weight: 931,
            addrs: vec![
                "ip:192.168.1.10:51820".to_owned(),
                "ip:[fe80::1]:51820".to_owned(),
                "relay:https://euw1-1.relay.n0.iroh-canary.iroh.link./".to_owned(),
            ],
            writable: true,
            incarnation: 1,
            ..Node::default()
        }]),
        ..View::default()
    }
}

/// The generator's `full` view: every field populated.
fn full_view() -> View {
    let node = |i: u64, weight: u32| Node {
        id: Some(gen_id(i)),
        weight,
        ..Node::default()
    };
    View {
        cluster_id: Some(dstore_testkit::splitmix::data(1000, 16)),
        incarnation: 2,
        epoch: 57,
        version: 311,
        placement_epoch: 54,
        replicas: 3,
        min_replicas: 2,
        voters: Some(vec![
            Voter {
                id: Some(gen_id(0)),
                since: 1,
            },
            Voter {
                id: Some(gen_id(1)),
                since: 12,
            },
            Voter {
                id: Some(gen_id(2)),
                since: 30,
            },
        ]),
        voter_sync: dstore_view::VOTER_SYNC_PENDING,
        voter_sync_cursor: b"join/\x00\x01".to_vec(),
        nodes: Some(vec![
            Node {
                addrs: vec!["ip:10.0.0.1:4433".to_owned()],
                data: vec![
                    DataEndpoint {
                        id: Some(gen_id(9)),
                        addrs: vec!["ip:10.0.0.1:4434".to_owned()],
                    },
                    DataEndpoint {
                        id: Some(gen_id(10)),
                        addrs: Vec::new(),
                    },
                ],
                token: dstore_testkit::splitmix::data(77, 32),
                zone: "rack-1".to_owned(),
                incarnation: 3,
                writable: true,
                ..node(0, 4096)
            },
            Node {
                zone: "rack-2".to_owned(),
                incarnation: 1,
                writable: false,
                ..node(1, 2048)
            },
            Node {
                addrs: vec!["relay:https://relay.example/".to_owned()],
                writable: true,
                incarnation: 1,
                ..node(2, 0)
            },
        ]),
        pending: Some(Box::new(Pending {
            nodes: Some(vec![
                Node {
                    writable: true,
                    ..node(0, 4096)
                },
                Node {
                    writable: true,
                    incarnation: 1,
                    ..node(5, 512)
                },
            ]),
            replicas: 2,
            id: 57,
            participants_ack: vec![Some(gen_id(0)), Some(gen_id(5))],
            participants: vec![Some(gen_id(0))],
            frozen: true,
            round: 2,
            primary_done: vec![Some(gen_id(0))],
            done: vec![Some(gen_id(0))],
            frozen_at: 1_757_999_999_123_456_789,
            reason: format!("join {}", NodeId::from_slice_lossy(&gen_id(5)).short()),
            ramp: Some(Box::new(Ramp {
                node: Some(gen_id(5)),
                target: 4096,
                step: 1,
            })),
            since: 1_757_999_990_000_000_000,
        })),
        former: vec![Former {
            id: Some(gen_id(7)),
            until: 1_760_000_000_000_000_000,
        }],
        fenced: vec![Some(gen_id(8))],
        recovered_inc: 1,
        recovered_epoch: 40,
        rebalance_pause: true,
        rate_cap: 104_857_600,
        voter_sync_target: gen_id(5),
        voter_sync_add: true,
        deferred_voters: vec![Some(gen_id(11))],
        acl: Some(Box::new(Acl {
            allowed: vec![Some(gen_id(9)), Some(gen_id(10))],
            admins: vec![Some(gen_id(9))],
        })),
        ramps: vec![Ramp {
            node: Some(gen_id(5)),
            target: 4096,
            step: 1,
        }],
        remove_voters: vec![Some(gen_id(7))],
    }
}

/// Every `view_cbor` case built field by field, as the generator built it (view-placement §5: "the Rust
/// struct built by hand == decode(hex)"), compared with both the encoding and the decoded value.
#[test]
fn view_cbor_hand_built() {
    let cases: Vec<(&str, View)> = vec![
        ("zero", View::default()),
        ("init", init_view()),
        ("full", full_view()),
        (
            "negatives",
            View {
                voter_sync: -1,
                former: vec![Former {
                    id: Some(gen_id(0)),
                    until: -1,
                }],
                pending: Some(Box::new(Pending {
                    frozen_at: -5,
                    since: i64::MIN,
                    ramp: Some(Box::new(Ramp {
                        step: -2,
                        ..Ramp::default()
                    })),
                    ..Pending::default()
                })),
                ramps: vec![Ramp {
                    step: -300,
                    ..Ramp::default()
                }],
                ..View::default()
            },
        ),
        (
            "maxes",
            View {
                incarnation: u64::MAX,
                epoch: 1 << 32,
                version: (1 << 32) - 1,
                placement_epoch: 65536,
                replicas: 255,
                min_replicas: 24,
                voter_sync: 1 << 40,
                rate_cap: u64::MAX,
                nodes: Some(vec![Node {
                    id: Some(gen_id(0)),
                    weight: u32::MAX,
                    incarnation: u64::MAX,
                    writable: true,
                    ..Node::default()
                }]),
                pending: Some(Box::new(Pending {
                    round: u32::MAX,
                    id: 23,
                    replicas: 23,
                    ..Pending::default()
                })),
                ..View::default()
            },
        ),
        (
            "utf8",
            View {
                nodes: Some(vec![Node {
                    id: Some(gen_id(0)),
                    weight: 1,
                    zone: "zürich-\u{1F3D4}".to_owned(),
                    ..Node::default()
                }]),
                pending: Some(Box::new(Pending {
                    reason: "ramp ✓".to_owned(),
                    ..Pending::default()
                })),
                ..View::default()
            },
        ),
        (
            "writable_false_and_data_no_addrs",
            View {
                nodes: Some(vec![Node {
                    id: Some(gen_id(0)),
                    data: vec![
                        DataEndpoint {
                            id: Some(gen_id(1)),
                            addrs: Vec::new(),
                        },
                        DataEndpoint::default(),
                    ],
                    ..Node::default()
                }]),
                ..View::default()
            },
        ),
        (
            "long_addrs",
            View {
                nodes: Some(vec![Node {
                    id: Some(gen_id(0)),
                    weight: 1,
                    addrs: vec![
                        "a".repeat(23),
                        "b".repeat(24),
                        "c".repeat(255),
                        "d".repeat(256),
                    ],
                    ..Node::default()
                }]),
                ..View::default()
            },
        ),
        (
            "acl_nil_and_empty_elements",
            View {
                acl: Some(Box::new(Acl {
                    allowed: vec![None, Some(Vec::new())],
                    admins: vec![Some(gen_id(3))],
                })),
                fenced: vec![None],
                deferred_voters: vec![Some(Vec::new())],
                remove_voters: vec![Some(vec![1, 2])],
                ..View::default()
            },
        ),
        (
            "pending_lists",
            View {
                pending: Some(Box::new(Pending {
                    nodes: None,
                    participants_ack: vec![None],
                    participants: vec![Some(Vec::new())],
                    primary_done: vec![Some(gen_id(4))],
                    done: vec![Some(gen_id(4)), None],
                    ..Pending::default()
                })),
                ..View::default()
            },
        ),
        (
            "empty_non_nil",
            View {
                cluster_id: Some(Vec::new()),
                voters: Some(Vec::new()),
                nodes: Some(Vec::new()),
                ..View::default()
            },
        ),
        (
            "zero_pointers",
            View {
                pending: Some(Box::new(Pending::default())),
                acl: Some(Box::new(Acl::default())),
                ..View::default()
            },
        ),
        (
            "pending_zero_ramp",
            View {
                pending: Some(Box::new(Pending {
                    ramp: Some(Box::new(Ramp::default())),
                    nodes: Some(Vec::new()),
                    ..Pending::default()
                })),
                ..View::default()
            },
        ),
        (
            "node_nil_and_empty_id",
            View {
                nodes: Some(vec![
                    Node {
                        id: None,
                        weight: 1,
                        ..Node::default()
                    },
                    Node {
                        id: Some(Vec::new()),
                        weight: 2,
                        ..Node::default()
                    },
                ]),
                ..View::default()
            },
        ),
    ];
    let names: Vec<&str> = cases.iter().map(|(name, _)| *name).collect();
    for (name, built) in cases {
        let Some(c) = vectors().view_cbor.iter().find(|c| c.name == name) else {
            panic!("view_cbor case {name} is missing");
        };
        assert_eq!(hex(&built.encode()), c.cbor, "{name}: encoding");
        assert_eq!(decode(name, &c.cbor), built, "{name}: decoded value");
    }
    for c in &vectors().view_cbor {
        assert!(
            names.contains(&c.name.as_str()),
            "view_cbor case {} is not built by hand",
            c.name
        );
    }
}

#[test]
fn view_decode() {
    let cases = &vectors().view_decode;
    assert!(!cases.is_empty());
    for c in cases {
        match View::decode(&golden::hex(&c.input)) {
            Ok(v) => {
                assert!(c.ok, "{}: decoded, Go says {:?}", c.name, c.error);
                assert_eq!(
                    Some(hex(&v.encode())),
                    c.reencode,
                    "{}: re-encoding",
                    c.name
                );
            }
            Err(e) => {
                assert!(!c.ok, "{}: {e}, Go decodes", c.name);
                assert_eq!(Some(e.to_string()), c.error, "{}: error", c.name);
            }
        }
    }
}

#[test]
fn parse_node_id() {
    let cases = &vectors().parse_node_id;
    assert!(!cases.is_empty());
    for c in cases {
        let input = golden::hex(&c.input_hex);
        match dstore_view::parse_node_id(&input) {
            Ok(id) => {
                assert!(c.ok, "{}: parsed, Go says {:?}", c.input_hex, c.error);
                assert_eq!(Some(id.to_hex()), c.id, "{}", c.input_hex);
            }
            Err(e) => {
                assert!(!c.ok, "{}: {e}, Go parses", c.input_hex);
                assert_eq!(Some(e.to_string()), c.error, "{}", c.input_hex);
            }
        }
    }
}

#[test]
fn short_id() {
    let cases = &vectors().short_id;
    assert!(!cases.is_empty());
    for c in cases {
        let b = opt_bytes(c.bytes.as_deref()).unwrap_or_default();
        assert_eq!(
            dstore_view::node_short_id(&b),
            c.node_short,
            "{:?}",
            c.bytes
        );
        if b.len() == 32 {
            let id = NodeId::from_slice_lossy(&b);
            assert_eq!(Some(dstore_view::short_id(&id)), c.view_short);
            assert_eq!(Some(dstore_view::id_string(&id)), c.id_string);
            assert_eq!(Some(id.short()), c.view_short);
        } else {
            assert!(c.view_short.is_none() && c.id_string.is_none());
        }
    }
}

#[test]
fn ticket_from_view_cbor() {
    let cases = &vectors().ticket_from_view;
    assert!(!cases.is_empty());
    for c in cases {
        let t = dstore_view::ticket_from_view(&decode(&c.name, &c.view));
        assert_eq!(hex(&dstore_codec::marshal(&t)), c.ticket_cbor, "{}", c.name);
    }
}

#[test]
fn ticket_from_view_encode() {
    for c in &vectors().ticket_from_view {
        let t = dstore_view::ticket_from_view(&decode(&c.name, &c.view));
        assert_eq!(t.encode(), c.ticket, "{}", c.name);
    }
}

#[test]
fn helpers() {
    let cases = &vectors().helpers;
    assert!(!cases.is_empty());
    for c in cases {
        let v = decode(&c.name, &c.view);
        assert_eq!(
            hexes(&v.all_members()),
            c.all_members,
            "{}: all_members",
            c.name
        );
        assert_eq!(hexes(&v.voter_ids()), c.voter_ids, "{}: voter_ids", c.name);
        assert_eq!(v.quorum(), c.quorum, "{}: quorum", c.name);
        for l in &c.lookups {
            let id = hex_id(&l.id);
            let at = format!("{} {}", c.name, l.id);
            let node = v.node(&id);
            assert_eq!(node.is_some(), l.node_found, "{at}: node found");
            assert_eq!(node.map_or(0, |n| n.weight), l.node_weight, "{at}: weight");
            assert_eq!(
                node.map(|n| n.addrs.clone()).unwrap_or_default(),
                l.node_addrs,
                "{at}: addrs"
            );
            assert_eq!(v.is_member(&id), l.is_member, "{at}: is_member");
            assert_eq!(
                v.data_endpoint_owner(&id).map(|o| o.to_hex()),
                l.data_endpoint_owner,
                "{at}: data_endpoint_owner"
            );
            assert_eq!(v.is_former(&id), l.is_former, "{at}: is_former");
            assert_eq!(v.is_voter(&id), l.is_voter, "{at}: is_voter");
        }
    }
}

#[test]
fn node_helpers() {
    let cases = &vectors().node_helpers;
    assert!(!cases.is_empty());
    for c in cases {
        let n = Node {
            id: opt_bytes(c.id.as_deref()),
            zone: c.zone.clone(),
            ..Node::default()
        };
        assert_eq!(n.nid().to_hex(), c.nid, "{:?} {:?}: nid", c.id, c.zone);
        assert_eq!(
            hex(&n.zone_or_id()),
            c.zone_or_id,
            "{:?} {:?}: zone_or_id",
            c.id,
            c.zone
        );
    }
}

#[test]
fn id_lists() {
    let cases = &vectors().id_lists;
    assert!(!cases.is_empty());
    for c in cases {
        let id = hex_id(&c.id);
        let mut ids: Vec<Option<Vec<u8>>> = c.ids.iter().map(|b| opt_bytes(b.as_deref())).collect();
        assert_eq!(dstore_view::contains(&ids, &id), c.contains, "{:?}", c.ids);
        dstore_view::add_id(&mut ids, &id);
        let want: Vec<Option<Vec<u8>>> = c.add_id.iter().map(|b| opt_bytes(b.as_deref())).collect();
        assert_eq!(ids, want, "{:?}: add_id", c.ids);
    }
}

#[test]
fn ids_of() {
    let cases = &vectors().ids_of;
    assert!(!cases.is_empty());
    for c in cases {
        let raw: Vec<Vec<u8>> = c.raw.iter().map(|b| golden::hex(b)).collect();
        assert_eq!(hexes(&dstore_view::ids_of(&raw)), c.ids, "{:?}", c.raw);
    }
}

#[test]
fn compare() {
    let cases = &vectors().compare;
    assert!(!cases.is_empty());
    for c in cases {
        let v = View {
            incarnation: c.view_incarnation,
            epoch: c.view_epoch,
            ..View::default()
        };
        let want = match c.result {
            -1 => Ordering::Less,
            0 => Ordering::Equal,
            1 => Ordering::Greater,
            r => panic!("bad compare result {r}"),
        };
        assert_eq!(
            v.compare(c.incarnation, c.epoch),
            want,
            "({}, {}) vs ({}, {})",
            c.view_incarnation,
            c.view_epoch,
            c.incarnation,
            c.epoch
        );
    }
}

#[test]
fn default_min_replicas() {
    let cases = &vectors().default_min_replicas;
    assert_eq!(cases.len(), 256);
    for c in cases {
        assert_eq!(
            dstore_view::default_min_replicas(c.r),
            c.min_replicas,
            "r={}",
            c.r
        );
    }
}

#[test]
fn validate_change() {
    let cases = &vectors().validate_change;
    assert!(!cases.is_empty());
    for c in cases {
        let got = dstore_view::validate_change(
            &view_nodes(&c.cur),
            &view_nodes(&c.target),
            c.replicas,
            c.force,
        );
        assert_eq!(got.err().map(|e| e.to_string()), c.error, "{}", c.name);
    }
}

#[test]
fn sort_nodes() {
    let c = &vectors().sort_nodes;
    let mut nodes: Vec<Node> = c
        .input
        .iter()
        .map(|id| Node {
            id: opt_bytes(id.as_deref()),
            ..Node::default()
        })
        .collect();
    dstore_view::sort_nodes(&mut nodes);
    let got: Vec<Option<String>> = nodes.iter().map(|n| n.id.as_deref().map(hex)).collect();
    assert_eq!(got, c.output);
}

#[test]
fn status_cluster_prefix() {
    let cases = &vectors().status_cluster_prefix;
    assert!(!cases.is_empty());
    for c in cases {
        let v = decode(&c.name, &c.view);
        assert_eq!(
            v.cluster_id.as_deref().map(hex),
            c.cluster_id,
            "{}: cluster_id",
            c.name
        );
        let cap = match dstore_view::cluster_id_cap(&golden::hex(&c.view)) {
            Ok(cap) => cap,
            Err(e) => panic!("{}: {e}", c.name),
        };
        assert_eq!(cap, c.cap, "{}: cap", c.name);
        let got = dstore_view::status_cluster_prefix(v.cluster_id.as_deref(), cap);
        match (&c.prefix, &c.panic) {
            (Some(prefix), None) => assert_eq!(got, Ok(prefix.clone()), "{}", c.name),
            (None, Some(text)) => assert_eq!(got, Err(text.clone()), "{}", c.name),
            other => panic!("{}: bad vector {other:?}", c.name),
        }
    }
}

#[test]
fn all_slots_spots() {
    let a: AllSlots = golden::load_json("placement/all_slots.json");
    assert_eq!(a.slots, placement::SLOTS);
    assert!(!a.sets.is_empty());
    for s in &a.sets {
        let members = placement_members(&s.members);
        let set = Set::new(members.clone());
        let table = Table::new(Set::new(members), s.r);
        assert_eq!(s.all_slots_blake3.len(), 64, "{}: digest", s.name);
        assert!(!s.spots.is_empty(), "{}: spots", s.name);
        for sp in &s.spots {
            let at = format!("{} slot {}", s.name, sp.slot);
            assert_eq!(set.rank(sp.slot), sp.rank, "{at}: rank");
            assert_eq!(set.owners(sp.slot, s.r), sp.owners, "{at}: owners");
            assert_eq!(table.rank(sp.slot).to_vec(), sp.rank, "{at}: table rank");
            assert_eq!(
                table.owners(sp.slot).to_vec(),
                sp.owners,
                "{at}: table owners"
            );
        }
    }
}
