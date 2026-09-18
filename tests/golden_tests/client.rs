//! Golden tests of `dstore-client` part A public APIs (owner client-a): `client/progress.json`
//! `human_bytes`/`rate`, and the part A texts of `errors/client_text.json` (`wire.Error`, `CASMismatch`, `Incomplete`,
//! `ErrUnknownRef`, `client: no bootstrap node answered: …`, `client: unexpected reply …`). The texts Go produced
//! through real client calls are reproduced through real `Cluster` calls over `dstore_transport::mem`. Part B kinds
//! (`payload_hash`, `negotiate`, `walk_local_tree`, `upload_to`, `record_rejected`, `not_placed*`, `pull_*`) are
//! asserted in `client_transfer.rs`. Schemas: VECTORS.md "Family `client`".

use std::sync::Arc;
use std::time::Duration;

use dstore_client::{CasMismatch, Cluster, Cond, Config, Ctx, Error, Incomplete, Logger, NodeId};
use dstore_gocompat::slog::{Attr, Handler, Level, Record};
use dstore_testkit::golden::{self, decimal_i64};
use dstore_ticket::{Member, Ticket};
use dstore_transport::mem::Network;
use dstore_transport::{Endpoint, Stream};
use dstore_view::{Node, View};
use dstore_wire::{
    ALPN_CLIENT, AdminRequest, Msg, ProtocolRemoteError, RemoteError, T_CAS_MISMATCH, T_INCOMPLETE,
    T_OK, T_REF, T_REFS, T_VIEW, T_VIEW_REPLY, err_msg,
};
use serde::Deserialize;
use tokio::io::AsyncReadExt;

// ---- client/progress.json: human_bytes, rate ----

#[derive(Deserialize)]
struct HumanBytesCase {
    #[serde(deserialize_with = "decimal_i64")]
    n: i64,
    out: String,
}

#[derive(Deserialize)]
struct RateCase {
    #[serde(deserialize_with = "decimal_i64")]
    bytes: i64,
    #[serde(deserialize_with = "decimal_i64")]
    took_ns: i64,
    out: String,
}

#[derive(Deserialize)]
struct ProgressFile {
    human_bytes: Vec<HumanBytesCase>,
    rate: Vec<RateCase>,
}

#[test]
fn human_bytes() {
    let f: ProgressFile = golden::load_json("client/progress.json");
    assert!(f.human_bytes.len() > 20);
    for c in &f.human_bytes {
        assert_eq!(
            dstore_client::human_bytes(c.n),
            c.out,
            "HumanBytes({})",
            c.n
        );
    }
}

#[test]
fn rate() {
    let f: ProgressFile = golden::load_json("client/progress.json");
    assert!(f.rate.len() > 10);
    for c in &f.rate {
        assert_eq!(
            dstore_client::rate_ns(c.bytes, c.took_ns),
            c.out,
            "Rate({}, {} ns)",
            c.bytes,
            c.took_ns
        );
        // A std Duration cannot be negative: those cases only go through rate_ns.
        if let Ok(ns) = u64::try_from(c.took_ns) {
            assert_eq!(
                dstore_client::rate(c.bytes, Duration::from_nanos(ns)),
                c.out,
                "Rate({}, {} ns)",
                c.bytes,
                c.took_ns
            );
        }
    }
}

// ---- errors/client_text.json ----

#[derive(Deserialize)]
struct TextCase {
    name: String,
    kind: String,
    out: String,
    inner: Option<String>,
    has_current: Option<bool>,
    current: Option<String>,
    shortfall: Option<i64>,
    code: Option<String>,
    text: Option<String>,
    #[serde(rename = "type")]
    typ: Option<i64>,
}

#[derive(Deserialize)]
struct TextFile {
    cases: Vec<TextCase>,
}

/// Part B kinds, asserted by `client_transfer.rs`.
const PART_B_KINDS: &[&str] = &[
    "payload_hash",
    "negotiate",
    "walk_local_tree",
    "upload_to",
    "record_rejected",
    "not_placed",
    "not_placed_last_error",
    "pull_object_not_found",
    "pull_incomplete",
];

/// The error a case describes, built from its fields.
fn built(c: &TextCase) -> Option<String> {
    let s = |o: &Option<String>| o.clone().unwrap_or_default();
    let e = match c.kind.as_str() {
        "no_endpoint" => Error::NoEndpoint,
        "no_nodes" => Error::NoNodes,
        "watch_idle" => Error::WatchIdle,
        "fetch_ended_early" => Error::FetchEndedEarly,
        "no_owners" => Error::NoOwners,
        "unknown_ref" => Error::UnknownRef,
        "unexpected_reply" => Error::UnexpectedReply(c.typ.unwrap_or_default()),
        "unexpected_frame" => Error::UnexpectedFrame(c.typ.unwrap_or_default()),
        "cas_mismatch" => Error::CasMismatch(CasMismatch {
            current: c.current.as_deref().map(golden::hex).unwrap_or_default(),
            record: Vec::new(),
            version: Vec::new(),
            has_current: c.has_current.unwrap_or_default(),
        }),
        "incomplete" => Error::Incomplete(Incomplete {
            sample: Vec::new(),
            shortfall: c.shortfall.unwrap_or_default(),
        }),
        "remote" => Error::Remote(RemoteError {
            code: s(&c.code),
            text: s(&c.text),
            view: Vec::new(),
            retry_after: Duration::ZERO,
        }),
        "protocol_remote" => {
            let pe = ProtocolRemoteError {
                code: s(&c.code),
                text: s(&c.text),
                current: Vec::new(),
            };
            return Some(pe.to_string());
        }
        // The inner error of a Dial comes from a real dial (below).
        "no_bootstrap" => Error::NoBootstrap(Box::new(Error::Other(s(&c.inner)))),
        _ => return None,
    };
    Some(e.to_string())
}

/// A log handler that drops everything (Dial logs `connected`).
struct Discard;

impl Handler for Discard {
    fn enabled(&self, _level: Level) -> bool {
        false
    }
    fn handle(&self, _handler_attrs: &[Attr], _r: &Record) {}
}

const NODE: NodeId = NodeId([0x11; 32]);
/// Its short id is `a38d2241`, the member the Go generator left unbound.
const UNBOUND: NodeId = NodeId([
    0xa3, 0x8d, 0x22, 0x41, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 1,
]);

type Reply = Arc<dyn Fn(&Msg) -> Option<Msg> + Send + Sync>;

fn view_of(ids: &[NodeId]) -> View {
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

/// A node answering one request per stream with `reply` (None: close without a frame).
fn serve(net: &Arc<Network>, id: NodeId, reply: Reply) {
    let ep = net.bind(id, &[ALPN_CLIENT]);
    tokio::spawn(async move {
        let ctx = Ctx::background();
        while let Ok(conn) = ep.accept(&ctx).await {
            let reply = reply.clone();
            tokio::spawn(async move {
                let ctx = Ctx::background();
                while let Ok(s) = conn.accept_stream(&ctx).await {
                    tokio::spawn(answer(s, reply.clone()));
                }
            });
        }
    });
}

async fn answer(mut s: Stream, reply: Reply) {
    let Ok(req) = dstore_wire::read_msg(&mut *s.recv).await else {
        return;
    };
    if let Some(m) = reply(&req) {
        let _ = dstore_wire::write_msg(&mut *s.send, &m).await;
    }
    s.send.finish();
    let mut buf = [0u8; 64];
    while let Ok(n) = s.recv.read(&mut buf).await {
        if n == 0 {
            break;
        }
    }
}

/// A node that answers views with a one-node view and everything else with `other`.
fn answering(other: Msg) -> Reply {
    Arc::new(move |req| {
        if req.typ == T_VIEW {
            Some(Msg {
                typ: T_VIEW_REPLY,
                incarnation: 1,
                epoch: 7,
                view: view_of(&[NODE]).encode(),
                ..Msg::default()
            })
        } else {
            Some(other.clone())
        }
    })
}

fn config(net: &Arc<Network>, members: Option<Vec<Member>>) -> Config {
    let endpoint: Arc<dyn Endpoint> = net.bind(NodeId([0xcc; 32]), &[]);
    Config {
        endpoint: Some(endpoint),
        ticket: Ticket {
            members,
            ..Ticket::default()
        },
        logger: Some(Logger::new(Arc::new(Discard))),
        ..Config::default()
    }
}

fn member(id: &[u8]) -> Member {
    Member {
        id: Some(id.to_vec()),
        addrs: Vec::new(),
    }
}

async fn dial_text(cfg: Config) -> String {
    match Cluster::dial(&Ctx::background(), cfg).await {
        Ok(_) => panic!("dial succeeded"),
        Err(e) => e.to_string(),
    }
}

async fn dialed(reply: Msg) -> (Arc<Network>, Cluster) {
    let net = Network::new();
    serve(&net, NODE, answering(reply));
    let cfg = config(&net, Some(vec![member(&NODE.0)]));
    match Cluster::dial(&Ctx::background(), cfg).await {
        Ok(c) => (net, c),
        Err(e) => panic!("dial: {e}"),
    }
}

fn text<T>(r: Result<T, Error>) -> String {
    match r {
        Ok(_) => panic!("the call succeeded"),
        Err(e) => e.to_string(),
    }
}

/// The text of a case Go produced through a real client call, from the same call here.
async fn called(name: &str, k1: &[u8]) -> Option<String> {
    let ctx = Ctx::background();
    let ok = Msg {
        typ: T_OK,
        ..Msg::default()
    };
    let force = Cond {
        force: true,
        ..Cond::default()
    };
    let admin = AdminRequest {
        op: "nope".into(),
        ..AdminRequest::default()
    };
    Some(match name {
        "dial_no_endpoint" => {
            let net = Network::new();
            dial_text(Config {
                endpoint: None,
                ..config(&net, Some(vec![member(&NODE.0)]))
            })
            .await
        }
        "dial_ticket_names_no_nodes" => dial_text(config(&Network::new(), None)).await,
        "dial_member_id_31_bytes" => {
            dial_text(config(&Network::new(), Some(vec![member(&[0x11; 31])]))).await
        }
        "dial_member_not_bound" => {
            dial_text(config(&Network::new(), Some(vec![member(&UNBOUND.0)]))).await
        }
        "dial_unexpected_reply" | "dial_last_member_error_wins" => {
            let net = Network::new();
            serve(
                &net,
                NODE,
                Arc::new(|_: &Msg| {
                    Some(Msg {
                        typ: T_OK,
                        ..Msg::default()
                    })
                }),
            );
            let members = if name == "dial_unexpected_reply" {
                vec![member(&NODE.0)]
            } else {
                vec![member(&NODE.0), member(&UNBOUND.0)]
            };
            dial_text(config(&net, Some(members))).await
        }
        "dial_remote_error" => {
            let net = Network::new();
            serve(
                &net,
                NODE,
                Arc::new(|_: &Msg| Some(err_msg("unavailable", "no view"))),
            );
            dial_text(config(&net, Some(vec![member(&NODE.0)]))).await
        }
        "dial_bad_view" => {
            let net = Network::new();
            serve(
                &net,
                NODE,
                Arc::new(|_: &Msg| {
                    Some(Msg {
                        typ: T_VIEW_REPLY,
                        view: vec![0x80],
                        ..Msg::default()
                    })
                }),
            );
            dial_text(config(&net, Some(vec![member(&NODE.0)]))).await
        }
        "dial_no_reply" => {
            let net = Network::new();
            serve(&net, NODE, Arc::new(|_: &Msg| None));
            dial_text(config(&net, Some(vec![member(&NODE.0)]))).await
        }
        "ref_get_unknown_ref" => {
            let (_net, c) = dialed(err_msg("unknown-ref", "no such reference")).await;
            let err = match c.ref_get(&ctx, "trees/a").await {
                Ok(_) => panic!("ref get succeeded"),
                Err(e) => e,
            };
            assert!(err.is_unknown_ref());
            err.to_string()
        }
        "ref_get_unexpected_reply" => {
            let (_net, c) = dialed(ok).await;
            text(c.ref_get(&ctx, "trees/a").await)
        }
        "ref_put_unexpected_reply" => {
            let (_net, c) = dialed(Msg {
                typ: T_REFS,
                ..Msg::default()
            })
            .await;
            text(c.ref_put(&ctx, &[0xa0], &force).await)
        }
        "ref_delete_unexpected_reply" => {
            let (_net, c) = dialed(Msg {
                typ: T_REF,
                ..Msg::default()
            })
            .await;
            text(c.ref_delete(&ctx, "trees/a", &force).await)
        }
        "ref_list_unexpected_reply" => {
            let (_net, c) = dialed(ok).await;
            text(c.ref_list(&ctx, b"").await)
        }
        "admin_unexpected_reply" => {
            let (_net, c) = dialed(ok).await;
            text(c.admin(&ctx, None, &admin).await)
        }
        "ref_list_remote_error_not_mapped" => {
            let (_net, c) = dialed(err_msg("unknown-ref", "no such reference")).await;
            text(c.ref_list(&ctx, b"").await)
        }
        "admin_remote_error" => {
            let (_net, c) = dialed(err_msg("bad-request", "unknown admin op nope")).await;
            text(c.admin(&ctx, None, &admin).await)
        }
        "ref_put_cas_mismatch" => {
            let (_net, c) = dialed(Msg {
                typ: T_CAS_MISMATCH,
                current: k1.to_vec(),
                has_current: true,
                ..Msg::default()
            })
            .await;
            text(c.ref_put(&ctx, &[0xa0], &force).await)
        }
        "ref_delete_cas_mismatch_absent" => {
            let (_net, c) = dialed(Msg {
                typ: T_CAS_MISMATCH,
                ..Msg::default()
            })
            .await;
            text(c.ref_delete(&ctx, "trees/a", &force).await)
        }
        "ref_put_incomplete" => {
            let (_net, c) = dialed(Msg {
                typ: T_INCOMPLETE,
                keys: vec![k1.to_vec()],
                shortfall: 7,
                ..Msg::default()
            })
            .await;
            text(c.ref_put(&ctx, &[0xa0], &force).await)
        }
        _ => return None,
    })
}

#[tokio::test]
async fn client_error_texts() {
    let f: TextFile = golden::load_json("errors/client_text.json");
    let k1 = golden::hex("00644b75a8842dc16ded120ee1f96d229ac409028f4d5d429499a4ad548aae85");
    let (mut built_n, mut called_n) = (0, 0);
    for c in &f.cases {
        if PART_B_KINDS.contains(&c.kind.as_str()) {
            continue;
        }
        let Some(want) = built(c) else {
            panic!("{}: unknown kind {}", c.name, c.kind);
        };
        assert_eq!(want, c.out, "{}: built", c.name);
        built_n += 1;
        if let Some(got) = called(&c.name, &k1).await {
            assert_eq!(got, c.out, "{}: called", c.name);
            called_n += 1;
        }
    }
    // Every part A case of the file (58 cases, 12 of them part B kinds) is built, and every scenario Go
    // produced through a real part A call is reproduced.
    assert_eq!(built_n, 46, "{built_n} cases built");
    assert_eq!(called_n, 20, "{called_n} cases called");
}
