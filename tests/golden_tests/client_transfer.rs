//! Golden tests of `dstore-client` part B public APIs (owner client-b): `client/verify_record.json`,
//! `client/placement_decisions.json`, and the part B texts of `errors/client_text.json` (`payload hashes …`,
//! `no owners`, `negotiate … at its primary: …`, `walk local tree: …`, `upload to …`, `record … rejected: …`,
//! `push: … keys could not be placed …`, `pull: …`). `client/transcripts/` is not generated yet (VECTORS.md
//! "Not generated yet"). The crate-private `est_size`/`pick_batch` vectors (`client/fetch.json`) are unit
//! tests of `dstore_client::fetch`.

use std::sync::Arc;
use std::time::Duration;

use amber_store_core::amberpack::{self, RawRecord, Record};
use amber_store_core::fstree::{MissingObjectError, WalkError};
use amber_store_core::ingest;
use amber_store_core::key::{Key, Type};
use amber_store_core::packstore;
use dstore_client::{Cluster, Cond, Config, Ctx, CtxError, Error, PullStats};
use dstore_gocompat::fmt::v_strings;
use dstore_testkit::{golden, splitmix};
use dstore_ticket::{Member, Ticket};
use dstore_transport::mem::{MemEndpoint, Network};
use dstore_transport::{Endpoint, TransportError};
use dstore_view::{NodeId, View};
use dstore_wire::{ALPN_CLIENT, Msg, RemoteError, T_VIEW, T_VIEW_REPLY};
use serde::Deserialize;

fn arr32(b: &[u8]) -> [u8; 32] {
    match <[u8; 32]>::try_from(b) {
        Ok(a) => a,
        Err(_) => panic!("expected 32 bytes, got {}", b.len()),
    }
}

fn arr8(hex: &str) -> [u8; 8] {
    let b = golden::hex(hex);
    match <[u8; 8]>::try_from(b.as_slice()) {
        Ok(a) => a,
        Err(_) => panic!("expected 8 bytes, got {}", b.len()),
    }
}

// ---- client/verify_record.json ----

#[derive(Deserialize)]
struct VerifyVectors {
    cases: Vec<VerifyCase>,
}

#[derive(Deserialize)]
struct VerifyCase {
    name: String,
    raw: RawJson,
    parses: bool,
    ok: bool,
    #[serde(default)]
    out_key: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    error_portable: Option<bool>,
}

#[derive(Deserialize)]
struct RawJson {
    key: String,
    flags: u8,
    ulen: u32,
    slen: u32,
    bytes: String,
}

#[test]
fn verify_record_vectors() {
    let v: VerifyVectors = golden::load_json("client/verify_record.json");
    assert_eq!(v.cases.len(), 23, "verify_record cases");
    for c in &v.cases {
        let bytes = golden::hex(&c.raw.bytes);
        let raw = RawRecord {
            record: Record {
                key: Key(arr32(&golden::hex(&c.raw.key))),
                flags: c.raw.flags,
                ulen: c.raw.ulen,
                slen: c.raw.slen,
            },
            bytes: bytes.clone(),
        };
        let parses = amberpack::parse_record(&bytes).is_ok_and(|r| r == raw.record);
        assert_eq!(parses, c.parses, "{}: parse_record", c.name);
        match dstore_client::verify_record(&raw) {
            Ok((k, out)) => {
                assert!(c.ok, "{}: accepted, Go refuses with {:?}", c.name, c.error);
                let want = c.out_key.as_deref().map(|h| arr32(&golden::hex(h)));
                assert_eq!(Some(k), want, "{}: key", c.name);
                assert_eq!(out, bytes, "{}: the record is returned verbatim", c.name);
            }
            Err(e) => {
                assert!(!c.ok, "{}: refused with {e}, Go accepts", c.name);
                if c.error_portable != Some(false) {
                    assert_eq!(Some(e.to_string()), c.error, "{}: error text", c.name);
                }
            }
        }
    }
}

// ---- errors/client_text.json (part B kinds) ----

#[derive(Deserialize)]
struct TextVectors {
    cases: Vec<TextCase>,
}

#[derive(Deserialize)]
struct TextCase {
    name: String,
    kind: String,
    out: String,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    want: Option<String>,
    #[serde(default)]
    inner: Option<String>,
    #[serde(default)]
    node: Option<String>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    count: Option<usize>,
    #[serde(default)]
    names: Option<Vec<String>>,
    #[serde(default)]
    last: Option<String>,
}

fn field<'a>(c: &'a TextCase, v: &'a Option<String>, what: &str) -> &'a str {
    match v.as_deref() {
        Some(s) => s,
        None => panic!("{}: no {what}", c.name),
    }
}

/// The inner errors the vectors wrap, rebuilt as the client produces them.
fn inner_error(text: &str) -> Error {
    match text {
        "no owners" => Error::NoOwners,
        "transport: closed" => Error::Transport(TransportError::Closed),
        "context deadline exceeded" => Error::Ctx(CtxError::DeadlineExceeded),
        _ => match text
            .strip_prefix("remote: ")
            .and_then(|r| r.split_once(": "))
        {
            Some((code, msg)) => Error::Remote(RemoteError {
                code: code.to_owned(),
                text: msg.to_owned(),
                view: Vec::new(),
                retry_after: Duration::ZERO,
            }),
            None => panic!("no rebuild for inner error {text:?}"),
        },
    }
}

/// The 64-hex key inside an fstree text.
fn key_in(text: &str) -> Key {
    let hex = text
        .split([' ', ':'])
        .find(|w| w.len() == 64 && w.bytes().all(|b| b.is_ascii_hexdigit()));
    match hex {
        Some(h) => Key(arr32(&golden::hex(h))),
        None => panic!("no key in {text:?}"),
    }
}

/// `push: <count> keys could not be placed; owners not confirming: <names>`, rebuilt from its text.
fn not_placed_from(text: &str) -> Error {
    let rest = text.strip_prefix("push: ").unwrap_or_default();
    let (count, names) = match rest.split_once(" keys could not be placed; owners not confirming: ")
    {
        Some(p) => p,
        None => panic!("not a shortError text: {text:?}"),
    };
    match count.parse() {
        Ok(count) => Error::NotPlaced {
            count,
            names: names.to_owned(),
        },
        Err(e) => panic!("count in {text:?}: {e}"),
    }
}

#[test]
fn part_b_error_texts() {
    let v: TextVectors = golden::load_json("errors/client_text.json");
    let mut seen = std::collections::BTreeSet::new();
    let mut built = 0;
    for c in &v.cases {
        let err = match c.kind.as_str() {
            "payload_hash" => Error::PayloadHash {
                want: field(c, &c.want, "want").to_owned(),
                key: field(c, &c.key, "key").to_owned(),
            },
            "no_owners" => Error::NoOwners,
            "negotiate" => Error::Negotiate {
                key8: arr8(field(c, &c.key, "key")),
                source: Arc::new(inner_error(field(c, &c.inner, "inner"))),
            },
            "walk_local_tree" => {
                let inner = field(c, &c.inner, "inner");
                assert!(inner.ends_with(": packstore: object not found"), "{inner}");
                Error::WalkLocalTree(WalkError::Read {
                    key: key_in(inner),
                    source: packstore::Error::NotFound,
                })
            }
            "pull_object_not_found" => Error::PullObjectNotFound {
                key8: arr8(field(c, &c.key, "key")),
            },
            "upload_to" => Error::UploadTo {
                node: NodeId(arr32(&golden::hex(field(c, &c.node, "node")))).short(),
                source: Arc::new(inner_error(field(c, &c.inner, "inner"))),
            },
            "record_rejected" => Error::RecordRejected {
                key8: arr8(field(c, &c.key, "key")),
                reason: field(c, &c.reason, "reason").to_owned(),
            },
            "not_placed" => Error::NotPlaced {
                count: c.count.unwrap_or_default(),
                names: v_strings(c.names.as_deref().unwrap_or_default()),
            },
            "not_placed_last_error" => {
                let last = field(c, &c.last, "last");
                let (node, inner) = match last
                    .strip_prefix("upload to ")
                    .and_then(|l| l.split_once(": "))
                {
                    Some(p) => p,
                    None => panic!("{}: last {last:?}", c.name),
                };
                Error::NotPlacedLastError {
                    placed: Box::new(not_placed_from(field(c, &c.inner, "inner"))),
                    last: Arc::new(Error::UploadTo {
                        node: node.to_owned(),
                        source: Arc::new(inner_error(inner)),
                    }),
                }
            }
            "fetch_ended_early" => Error::FetchEndedEarly,
            "pull_incomplete" => Error::PullIncomplete(WalkError::Missing(MissingObjectError {
                key: key_in(field(c, &c.inner, "inner")),
            })),
            _ => continue,
        };
        assert_eq!(err.to_string(), c.out, "{}", c.name);
        seen.insert(c.kind.clone());
        built += 1;
    }
    assert_eq!(built, 14, "part B cases in errors/client_text.json");
    for kind in [
        "payload_hash",
        "no_owners",
        "negotiate",
        "walk_local_tree",
        "pull_object_not_found",
        "upload_to",
        "record_rejected",
        "not_placed",
        "not_placed_last_error",
        "fetch_ended_early",
        "pull_incomplete",
    ] {
        assert!(
            seen.contains(kind),
            "no {kind} case in errors/client_text.json"
        );
    }
}

// ---- client/placement_decisions.json ----

#[derive(Deserialize)]
struct PlacementVectors {
    scenarios: Vec<Scenario>,
}

#[derive(Deserialize)]
struct Scenario {
    name: String,
    view: String,
    unreachable: Vec<usize>,
    members: Vec<String>,
    bootstrap: usize,
    nodes: Vec<usize>,
    replicas: u8,
    min_replicas: u8,
    keys: Vec<KeyCase>,
}

#[derive(Deserialize)]
struct KeyCase {
    key: String,
    primary: Option<usize>,
    owners: Vec<usize>,
    pending_owners: Option<Vec<usize>>,
    write_set: Vec<usize>,
    read_order: Vec<usize>,
    placed: Vec<PlacedCase>,
}

#[derive(Deserialize)]
struct PlacedCase {
    name: String,
    holders: Vec<usize>,
    placed: bool,
}

/// Discards the client's log lines.
struct Quiet;

impl dstore_gocompat::slog::Handler for Quiet {
    fn enabled(&self, _level: dstore_gocompat::slog::Level) -> bool {
        false
    }

    fn handle(
        &self,
        _handler_attrs: &[dstore_gocompat::slog::Attr],
        _r: &dstore_gocompat::slog::Record,
    ) {
    }
}

/// The generator's fake node: answers `view` as `node.handleView` does, with the scenario's hints.
async fn serve_view(ep: Arc<MemEndpoint>, ctx: Ctx, view: Vec<u8>, unreachable: Vec<Vec<u8>>) {
    while let Ok(conn) = ep.accept(&ctx).await {
        let (ctx, view, unreachable) = (ctx.clone(), view.clone(), unreachable.clone());
        tokio::spawn(async move {
            while let Ok(mut s) = conn.accept_stream(&ctx).await {
                let Ok(m) = dstore_wire::read_msg(&mut s.recv).await else {
                    continue;
                };
                let reply = if m.typ == T_VIEW {
                    Msg {
                        typ: T_VIEW_REPLY,
                        incarnation: 1,
                        epoch: 7,
                        view: view.clone(),
                        unreachable: unreachable.clone(),
                        ..Msg::default()
                    }
                } else {
                    dstore_wire::err_msg(dstore_wire::CODE_BAD_REQUEST, "unknown operation")
                };
                let _ = dstore_wire::write_msg(&mut s.send, &reply).await;
                s.close_write();
            }
        });
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn placement_decisions_through_dial() {
    let v: PlacementVectors = golden::load_json("client/placement_decisions.json");
    assert_eq!(v.scenarios.len(), 6);
    for s in &v.scenarios {
        let members: Vec<NodeId> = s
            .members
            .iter()
            .map(|h| NodeId(arr32(&golden::hex(h))))
            .collect();
        let net = Network::new();
        let boot = members[s.bootstrap];
        let ctx = Ctx::background().with_cancel();
        let unreachable: Vec<Vec<u8>> = s
            .unreachable
            .iter()
            .map(|i| members[*i].0.to_vec())
            .collect();
        let server = tokio::spawn(serve_view(
            net.bind(boot, &[ALPN_CLIENT]),
            ctx.clone(),
            golden::hex(&s.view),
            unreachable,
        ));
        let client_ep: Arc<dyn Endpoint> = net.bind(NodeId([0xfe; 32]), &[ALPN_CLIENT]);
        let cfg = Config {
            endpoint: Some(client_ep),
            ticket: Ticket {
                cluster_id: None,
                incarnation: 0,
                members: Some(vec![Member {
                    id: Some(boot.0.to_vec()),
                    addrs: Vec::new(),
                }]),
            },
            logger: Some(dstore_gocompat::slog::Logger::new(Arc::new(Quiet))),
            request_timeout: Duration::from_secs(20),
            ..Config::default()
        };
        let cl = match Cluster::dial(&Ctx::background(), cfg).await {
            Ok(c) => c,
            Err(e) => panic!("{}: dial: {e}", s.name),
        };
        let idx = |id: &NodeId| match members.iter().position(|m| m == id) {
            Some(i) => i,
            None => panic!("{}: {} is not a member", s.name, id.to_hex()),
        };
        let idxs = |ids: &[NodeId]| ids.iter().map(idx).collect::<Vec<usize>>();
        assert_eq!(idxs(&cl.nodes()), s.nodes, "{}: nodes", s.name);
        let view = match cl.view() {
            Some(v) => v,
            None => panic!("{}: no view adopted", s.name),
        };
        assert_eq!(
            (view.replicas, view.min_replicas),
            (s.replicas, s.min_replicas),
            "{}",
            s.name
        );
        for kc in &s.keys {
            let k = arr32(&golden::hex(&kc.key));
            let what = format!("{} {}", s.name, &kc.key[..16]);
            assert_eq!(
                cl.primary(&k).map(|id| idx(&id)),
                kc.primary,
                "{what}: primary"
            );
            assert_eq!(idxs(&cl.owners(&k)), kc.owners, "{what}: owners");
            let pending = cl.placement().and_then(|pl| pl.pending_owners(&k));
            assert_eq!(
                pending.map(|p| idxs(&p)),
                kc.pending_owners,
                "{what}: pending owners"
            );
            assert_eq!(idxs(&cl.write_set(&k)), kc.write_set, "{what}: write set");
            assert_eq!(
                idxs(&cl.read_order(&k)),
                kc.read_order,
                "{what}: read order"
            );
            for pc in &kc.placed {
                let holders: Vec<NodeId> = pc.holders.iter().map(|i| members[*i]).collect();
                assert_eq!(
                    cl.placed(&k, &holders),
                    pc.placed,
                    "{what}: placed {}",
                    pc.name
                );
            }
        }
        cl.close();
        ctx.cancel();
        let _ = server.await;
    }
}

// ---- errors/client_text.json: the part B cases Go produced through client calls ----

fn open_store(dir: &std::path::Path) -> Arc<packstore::Store> {
    match packstore::Store::open_with(dir, packstore::Options::new()) {
        Ok(s) => Arc::new(s),
        Err(e) => panic!("open packstore {}: {e}", dir.display()),
    }
}

/// The generator's `genClientText` scenario: a fake node answering `view` with a view without nodes, the
/// empty tree ingested from an empty directory, and an empty packstore. `Missing`, `Push` (twice) and
/// `PullTree` run through a dialed client and must fail with the generated texts.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn part_b_error_texts_through_client_calls() {
    let v: TextVectors = golden::load_json("errors/client_text.json");
    let out = |name: &str| match v.cases.iter().find(|c| c.name == name) {
        Some(c) => c.out.clone(),
        None => panic!("no case {name} in errors/client_text.json"),
    };

    let tmp = match tempfile::tempdir() {
        Ok(d) => d,
        Err(e) => panic!("tempdir: {e}"),
    };
    let src = tmp.path().join("src");
    if let Err(e) = std::fs::create_dir_all(&src) {
        panic!("mkdir {}: {e}", src.display());
    }
    let tree = open_store(&tmp.path().join("tree"));
    let empty = open_store(&tmp.path().join("empty"));
    let opts = ingest::Opts {
        jobs: 1,
        ..ingest::Opts::default()
    };
    let root = match ingest::dir(&tree, &src, opts).1 {
        Ok(k) => k,
        Err(e) => panic!("ingest the empty tree: {e}"),
    };

    let view = View {
        cluster_id: Some(splitmix::data(0x43_4c55, 16)),
        incarnation: 1,
        epoch: 7,
        version: 9,
        placement_epoch: 7,
        replicas: 3,
        min_replicas: 2,
        ..View::default()
    };
    let net = Network::new();
    let boot = NodeId([0x42; 32]);
    let ctx = Ctx::background().with_cancel();
    let server = tokio::spawn(serve_view(
        net.bind(boot, &[ALPN_CLIENT]),
        ctx.clone(),
        view.encode(),
        Vec::new(),
    ));
    let client_ep: Arc<dyn Endpoint> = net.bind(NodeId([0xfe; 32]), &[]);
    let cfg = Config {
        endpoint: Some(client_ep),
        ticket: Ticket {
            cluster_id: None,
            incarnation: 0,
            members: Some(vec![Member {
                id: Some(boot.0.to_vec()),
                addrs: Vec::new(),
            }]),
        },
        logger: Some(dstore_gocompat::slog::Logger::new(Arc::new(Quiet))),
        ..Config::default()
    };
    let bg = Ctx::background();
    let cl = match Cluster::dial(&bg, cfg).await {
        Ok(c) => c,
        Err(e) => panic!("dial: {e}"),
    };
    let force = Cond {
        force: true,
        ..Cond::default()
    };

    let k1 = Key::new(Type::Blob, 100, &splitmix::data(1, 100)).0;
    match cl.missing(&bg, &[k1], false).await {
        Ok(mr) => match mr.failed.get(&k1) {
            Some(e) => assert_eq!(e.to_string(), out("missing_no_owners")),
            None => panic!("Missing over a view without nodes: no failure"),
        },
        Err(e) => panic!("Missing: {e}"),
    }
    match cl
        .push(
            &bg,
            Arc::clone(&tree),
            root,
            "trees/a",
            "alice",
            force.clone(),
            None,
        )
        .await
    {
        Err(e) => assert_eq!(e.to_string(), out("push_negotiate_no_owners")),
        Ok(st) => panic!("Push over a view without nodes succeeded: {st:?}"),
    }
    match cl
        .push(
            &bg,
            Arc::clone(&empty),
            root,
            "trees/a",
            "alice",
            force,
            None,
        )
        .await
    {
        Err(e) => assert_eq!(e.to_string(), out("push_walk_local_tree")),
        Ok(st) => panic!("Push from an empty packstore succeeded: {st:?}"),
    }
    let mut st = PullStats::default();
    match cl
        .pull_tree(&bg, Arc::clone(&empty), root, &mut st, None)
        .await
    {
        Err(e) => assert_eq!(e.to_string(), out("pull_tree_object_not_found")),
        Ok(()) => panic!("PullTree over a view without nodes succeeded"),
    }

    cl.close();
    ctx.cancel();
    let _ = server.await;
}
