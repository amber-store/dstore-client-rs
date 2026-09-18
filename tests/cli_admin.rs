//! cli-admin (layer L5): the admin actions of `cmd/dstore/main.go` (`dstore_cli` `cmd_admin`) over
//! `tests/golden/admin/requests.json` and `admin/replies.json`, Go's `fmt.Scanln`, and a fake cluster on the
//! in-memory transport. No sockets.
//!
//! `cmd_admin` is a private module of dstore-cli, so this binary compiles its network-free part,
//! `crates/cli/src/cmd_admin/args.rs`, as a module of its own: the argv → `AdminRequest` mapping, the argument
//! checks, the prompt's scanner and the output texts are the code the actions run. The actions' glue
//! (`signalCtx`, dialing, printing, closing) is covered by `tests/cli_snapshots.rs` `admin_cases` (every
//! pre-dial outcome of the Go binary) and by `cmd_admin`'s unit test over the fake cluster; `adminAction`'s
//! print block (replies.json `printed`) by `common`'s unit tests.

#[allow(dead_code)]
#[path = "../crates/cli/src/cmd_admin/args.rs"]
mod args;

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::io::{self, Read};
use std::os::unix::ffi::OsStringExt;
use std::sync::Arc;

use args::{AdminCommand, RestoreSource};
use dstore_client::{Cluster, Config, Ctx, Logger, NodeId};
use dstore_gocli::{CliError, Context};
use dstore_gocompat::slog::{Attr, Handler, Level, Record};
use dstore_testkit::fake::{FakeCluster, FakeClusterConfig};
use dstore_testkit::golden::{self, load_json};
use dstore_transport::Endpoint;
use dstore_transport::mem::Network;
use dstore_wire::{AdminReply, AdminRequest, Msg, T_ADMIN};
use serde::Deserialize;

/// A node id that is a valid curve point (`verification §5`, used by the CLI snapshots).
const ID: &str = "4bb675de4f6376ab61737033d701560e4434be1d6736562ae29b8835b763cd24";

// ---- vectors ----

#[derive(Debug, Deserialize)]
struct Requests {
    cases: Vec<RequestCase>,
}

#[derive(Debug, Deserialize)]
struct RequestCase {
    name: String,
    argv: Option<Vec<String>>,
    request: RequestJson,
    params_hex: String,
    frame_hex: String,
}

/// VECTORS.md `AdminRequestJSON`: every field present, byte fields hex.
#[derive(Debug, Deserialize)]
struct RequestJson {
    op: String,
    node: String,
    weight: u32,
    zone: String,
    replicas: u8,
    dead: bool,
    allow_unsafe: bool,
    force: bool,
    key: String,
    garbage_bits: String,
    tolerate: bool,
    forwarded: bool,
    pause: bool,
    #[serde(deserialize_with = "golden::decimal_u64")]
    rate: u64,
    names: Vec<String>,
}

impl RequestJson {
    fn request(&self) -> AdminRequest {
        let bits = match u64::from_str_radix(&self.garbage_bits, 16) {
            Ok(b) => b,
            Err(e) => panic!("garbage_bits {:?}: {e}", self.garbage_bits),
        };
        AdminRequest {
            op: self.op.clone(),
            node: golden::hex(&self.node),
            weight: self.weight,
            zone: self.zone.clone(),
            replicas: self.replicas,
            dead: self.dead,
            allow_unsafe: self.allow_unsafe,
            force: self.force,
            key: golden::hex(&self.key),
            garbage: f64::from_bits(bits),
            tolerate: self.tolerate,
            forwarded: self.forwarded,
            pause: self.pause,
            rate: self.rate,
            names: self.names.clone(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct Replies {
    cases: Vec<ReplyCase>,
}

#[derive(Debug, Deserialize)]
struct ReplyCase {
    name: String,
    reply: ReplyJson,
    status_hex: String,
    printed_token_create: String,
    cluster_ticket: Option<TicketOutcome>,
}

/// VECTORS.md `AdminReplyJSON`.
#[derive(Debug, Deserialize)]
struct ReplyJson {
    text: String,
    token: String,
    view: String,
    names: Vec<String>,
    key: String,
    ticket: String,
    gc: String,
}

impl ReplyJson {
    fn reply(&self) -> AdminReply {
        AdminReply {
            text: self.text.clone(),
            token: golden::hex(&self.token),
            view: golden::hex(&self.view),
            names: self.names.clone(),
            key: golden::hex(&self.key),
            ticket: self.ticket.clone(),
            gc: golden::hex(&self.gc),
        }
    }
}

/// `cluster ticket`'s outcome for a reply: `ticket.Parse(r.Ticket)`, then `Encode()` / `IDs()`.
#[derive(Debug, Deserialize)]
struct TicketOutcome {
    ok: bool,
    error: Option<String>,
    printed: Option<String>,
    printed_ids: Option<String>,
}

// ---- helpers ----

/// The `Context` the framework hands the action of `dstore <argv…>`, with an empty environment.
fn context_os(argv: Vec<OsString>) -> Context {
    let what = format!("{argv:?}");
    let args: Vec<OsString> = std::iter::once(OsString::from("dstore"))
        .chain(argv)
        .collect();
    let mut out = Vec::new();
    match dstore_gocli::dispatch(&dstore_cli::app(), args, &mut out, &|_: &str| None) {
        Ok(Some((_, c))) => {
            assert!(out.is_empty(), "{what}: the framework printed {out:?}");
            c
        }
        Ok(None) => panic!("{what}: no action reached"),
        Err(e) => panic!("{what}: {e}"),
    }
}

fn context(argv: &[&str]) -> Context {
    context_os(argv.iter().map(OsString::from).collect())
}

fn text(e: CliError) -> String {
    match e {
        CliError::Msg(m) => m,
        CliError::Exit { msg, code } => panic!("unexpected exit error {code}: {msg}"),
    }
}

/// The command whose action `argv` reaches.
fn command_of(argv: &[String]) -> AdminCommand {
    use AdminCommand as A;
    let sub = argv.get(1).map_or("", String::as_str);
    match (argv[0].as_str(), sub) {
        ("cluster", "ticket") => A::ClusterTicket,
        ("cluster", "replicas") => A::ClusterReplicas,
        ("token", "create") => A::TokenCreate,
        ("node", "remove") => A::NodeRemove,
        ("node", "drain") => A::NodeDrain,
        ("node", "weight") => A::NodeWeight,
        ("node", "zone") => A::NodeZone,
        ("node", "repair") => A::NodeRepair,
        ("voter", "add") => A::VoterAdd,
        ("voter", "remove") => A::VoterRemove,
        ("transition", "status") => A::TransitionStatus,
        ("transition", "abort") => A::TransitionAbort,
        ("transition", "refreeze") => A::TransitionRefreeze,
        ("transition", "pause") => A::TransitionPause,
        ("transition", "resume") => A::TransitionResume,
        ("gc", "run") => A::GcRun,
        ("gc", "status") => A::GcStatus,
        ("gc", "hold") => A::GcHold,
        ("gc", "release") => A::GcRelease,
        ("gc", "why") => A::GcWhy,
        ("catalog", "backup") => A::CatalogBackup,
        ("catalog", "backups") => A::CatalogBackups,
        _ => panic!("{argv:?}: not an admin request command"),
    }
}

fn request(argv: &[&str], cmd: AdminCommand) -> AdminRequest {
    match cmd.request(&context(argv)) {
        Ok(r) => r,
        Err(e) => panic!("{argv:?}: {}", text(e)),
    }
}

/// Field by field; `garbage` by its bits, except that any NaN equals any NaN (Go's ParseFloat NaN carries a
/// payload bit; the canonical encoding `f97e00` drops it, which `params_hex` checks).
fn assert_request(name: &str, got: &AdminRequest, want: &AdminRequest) {
    let (g, w) = (got.garbage, want.garbage);
    assert!(
        (g.is_nan() && w.is_nan()) || g.to_bits() == w.to_bits(),
        "{name}: garbage {g:?} ({:016x}), want {w:?} ({:016x})",
        g.to_bits(),
        w.to_bits()
    );
    let strip = |r: &AdminRequest| AdminRequest {
        garbage: 0.0,
        ..r.clone()
    };
    assert_eq!(strip(got), strip(want), "{name}");
}

/// The TAdmin frame of `params` stamped as the vectors are: cluster id 00..0f, incarnation 1, epoch 7.
fn vector_frame(m: &Msg) -> String {
    let m = Msg {
        cluster_id: (0u8..16).collect(),
        incarnation: 1,
        epoch: 7,
        ..m.clone()
    };
    match dstore_wire::encode_frame(&m) {
        Ok(f) => hex::encode(f),
        Err(e) => panic!("encode frame: {e}"),
    }
}

/// A log handler that drops everything.
struct Discard;

impl Handler for Discard {
    fn enabled(&self, _level: Level) -> bool {
        false
    }
    fn handle(&self, _handler_attrs: &[Attr], _r: &Record) {}
}

async fn dial(net: &Arc<Network>, fc: &FakeCluster) -> Cluster {
    let endpoint: Arc<dyn Endpoint> = net.bind(NodeId([0xc1; 32]), &[]);
    let cfg = Config {
        endpoint: Some(endpoint),
        ticket: fc.ticket(),
        logger: Some(Logger::new(Arc::new(Discard))),
        ..Config::default()
    };
    match Cluster::dial(&Ctx::background(), cfg).await {
        Ok(c) => c,
        Err(e) => panic!("dial: {e}"),
    }
}

/// The TAdmin requests a fake node has read, in order.
fn admin_frames(fc: &FakeCluster, id: NodeId) -> Vec<Msg> {
    fc.requests(id)
        .into_iter()
        .filter(|m| m.typ == T_ADMIN)
        .collect()
}

// ---- admin/requests.json ----

/// Every case with an argv: the framework's context of that command line, through the command's request
/// builder, gives Go's `node.AdminRequest`, its canonical CBOR and its TAdmin frame.
#[test]
fn argv_builds_the_go_request() {
    let file: Requests = load_json("admin/requests.json");
    let mut commands = BTreeSet::new();
    let mut n = 0;
    for case in &file.cases {
        let Some(argv) = &case.argv else {
            continue;
        };
        let cmd = command_of(argv);
        let argv_str: Vec<&str> = argv.iter().map(String::as_str).collect();
        let got = request(&argv_str, cmd);
        assert_request(&case.name, &got, &case.request.request());
        let params = dstore_codec::marshal(&got);
        assert_eq!(
            hex::encode(&params),
            case.params_hex,
            "{}: params",
            case.name
        );
        let m = Msg {
            typ: T_ADMIN,
            params,
            ..Msg::default()
        };
        assert_eq!(vector_frame(&m), case.frame_hex, "{}: frame", case.name);
        commands.insert(format!("{cmd:?}"));
        n += 1;
    }
    assert_eq!(
        commands.len(),
        22,
        "every request command has vectors: {commands:?}"
    );
    assert!(n >= 60, "only {n} argv cases");
}

/// The same requests sent as the actions send them (`common::admin`, any node) to a fake cluster: every
/// TAdmin frame a node reads carries the vector's params, and restamped as the vector it is the vector's
/// frame (the client adds no other field).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn argv_requests_reach_a_fake_cluster() {
    let net = Network::new();
    let fc = FakeCluster::start(&net, FakeClusterConfig::default()).await;
    let cl = dial(&net, &fc).await;
    let ctx = Ctx::background();
    let ids = fc.ids();
    let file: Requests = load_json("admin/requests.json");
    let mut n = 0;
    for case in &file.cases {
        let Some(argv) = &case.argv else {
            continue;
        };
        let argv_str: Vec<&str> = argv.iter().map(String::as_str).collect();
        let req = request(&argv_str, command_of(argv));
        let before: Vec<usize> = ids.iter().map(|id| admin_frames(&fc, *id).len()).collect();
        // The fake's answer may be an error (a non-member id, no frozen transition, R = 0): only the
        // request matters here.
        let _ = dstore_cli::common::admin(&ctx, &cl, &req).await;
        let mut sent = Vec::new();
        for (id, skip) in ids.iter().zip(before) {
            sent.extend(admin_frames(&fc, *id).into_iter().skip(skip));
        }
        assert!(
            !sent.is_empty(),
            "{}: no TAdmin frame reached a node",
            case.name
        );
        let view = fc.view();
        for m in &sent {
            assert_eq!(
                hex::encode(&m.params),
                case.params_hex,
                "{}: params",
                case.name
            );
            assert_eq!(
                m.cluster_id,
                view.cluster_id.clone().unwrap_or_default(),
                "{}: stamp",
                case.name
            );
            assert_eq!(vector_frame(m), case.frame_hex, "{}: frame", case.name);
        }
        n += 1;
    }
    assert!(n >= 60, "only {n} argv cases");
    cl.close();
    fc.close().await;
}

// ---- admin/replies.json ----

/// Every reply: its decoding, `token create`'s line (a newline even for an empty text), and `cluster
/// ticket`'s parse and printed forms or its error.
#[test]
fn replies_print_as_token_create_and_cluster_ticket_do() {
    let file: Replies = load_json("admin/replies.json");
    let mut tickets = 0;
    for case in &file.cases {
        let r = match dstore_wire::decode_admin_reply(&golden::hex(&case.status_hex)) {
            Ok(r) => r,
            Err(e) => panic!("{}: decode: {e}", case.name),
        };
        assert_eq!(r, case.reply.reply(), "{}: reply", case.name);
        assert_eq!(
            args::token_line(&r),
            case.printed_token_create,
            "{}: token create",
            case.name
        );
        let Some(want) = &case.cluster_ticket else {
            continue;
        };
        tickets += 1;
        match args::reply_ticket(&r) {
            Ok(t) => {
                assert!(want.ok, "{}: parsed, want {:?}", case.name, want.error);
                assert_eq!(
                    Some(args::ticket_line(&t, false)),
                    want.printed,
                    "{}: cluster ticket",
                    case.name
                );
                assert_eq!(
                    Some(args::ticket_line(&t, true)),
                    want.printed_ids,
                    "{}: cluster ticket --ids",
                    case.name
                );
            }
            Err(e) => {
                assert!(!want.ok, "{}: failed, want success", case.name);
                assert_eq!(Some(text(e)), want.error, "{}: error", case.name);
            }
        }
    }
    assert_eq!(tickets, 4, "cluster_ticket outcomes");
}

/// `token create --weight 7` and `cluster ticket [--ids]` over a fake cluster, through `common::admin` as
/// the actions call it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn token_create_and_cluster_ticket_over_a_fake_cluster() {
    let net = Network::new();
    let fc = FakeCluster::start(&net, FakeClusterConfig::default()).await;
    let cl = dial(&net, &fc).await;
    let ctx = Ctx::background();
    let admin = |req: AdminRequest| {
        let (ctx, cl) = (&ctx, &cl);
        async move {
            match dstore_cli::common::admin(ctx, cl, &req).await {
                Ok(r) => r,
                Err(e) => panic!("{}: {}", req.op, text(e)),
            }
        }
    };

    let req = request(
        &["token", "create", "--weight", "7"],
        AdminCommand::TokenCreate,
    );
    assert_eq!(req.weight, 7);
    let r = admin(req).await;
    assert_eq!(r.token.len(), 32);
    assert_eq!(r.text, hex::encode(&r.token));
    assert_eq!(args::token_line(&r), format!("{}\n", r.text));

    let r = admin(request(&["cluster", "ticket"], AdminCommand::ClusterTicket)).await;
    let t = match args::reply_ticket(&r) {
        Ok(t) => t,
        Err(e) => panic!("cluster ticket: {}", text(e)),
    };
    let view = fc.view();
    assert_eq!(t.cluster_id, view.cluster_id);
    assert_eq!(t.incarnation, view.incarnation);
    // The answering node first, then the first three nodes of the view (which repeat it).
    let hex_of = |m: &dstore_ticket::Member| hex::encode(m.id.clone().unwrap_or_default());
    let ids: Vec<String> = fc.ids().iter().map(|id| hex::encode(id.0)).collect();
    let members = t.members();
    assert_eq!(members.len(), 4, "members {members:?}");
    let first = hex_of(&members[0]);
    assert!(ids.contains(&first), "first member {first}");
    let rest: Vec<String> = members[1..].iter().map(hex_of).collect();
    assert_eq!(rest, ids);
    let mut want_ids = vec![first.clone()];
    want_ids.extend(ids.iter().filter(|id| **id != first).cloned());
    assert_eq!(
        args::ticket_line(&t, true),
        format!("{}\n", want_ids.join(","))
    );
    // Parsed and re-encoded, the ticket is the node's own string.
    assert_eq!(args::ticket_line(&t, false), format!("{}\n", r.ticket));

    cl.close();
    fc.close().await;
}

// ---- argument checks before dialing ----

#[test]
fn argument_errors_have_go_texts() {
    use AdminCommand as A;
    let key31 = "5a".repeat(31);
    let key33 = "5a".repeat(33);
    let odd = format!("{}5", "5a".repeat(31));
    let base32 = "aebagbafaydqqcikbmga2dqpcaireeyuculbogazdinryhi6d4qa";
    let bad_id = |s: &str| format!("view: bad node id {s:?}");
    let cases: Vec<(Vec<&str>, A, String)> = vec![
        (vec!["node", "remove", "zz"], A::NodeRemove, bad_id("zz")),
        (vec!["node", "remove"], A::NodeRemove, bad_id("")),
        (vec!["node", "drain"], A::NodeDrain, bad_id("")),
        (vec!["node", "repair", "zz"], A::NodeRepair, bad_id("zz")),
        (vec!["node", "zone"], A::NodeZone, bad_id("")),
        (
            vec!["node", "remove", &ID[..62]],
            A::NodeRemove,
            bad_id(&ID[..62]),
        ),
        (vec!["voter", "add"], A::VoterAdd, bad_id("")),
        (vec!["voter", "add", base32], A::VoterAdd, bad_id(base32)),
        (vec!["voter", "remove", "zz"], A::VoterRemove, bad_id("zz")),
        // The id is checked before the weight.
        (
            vec!["node", "weight", "zz", "x"],
            A::NodeWeight,
            bad_id("zz"),
        ),
        (
            vec!["node", "weight", ID, "x"],
            A::NodeWeight,
            "weight ID GiB".into(),
        ),
        (
            vec!["node", "weight", ID],
            A::NodeWeight,
            "weight ID GiB".into(),
        ),
        (
            vec!["node", "weight", ID, "4294967296"],
            A::NodeWeight,
            "weight ID GiB".into(),
        ),
        (
            vec!["node", "weight", ID, "-1"],
            A::NodeWeight,
            "weight ID GiB".into(),
        ),
        (
            vec!["node", "weight", ID, "+5"],
            A::NodeWeight,
            "weight ID GiB".into(),
        ),
        (
            vec!["node", "weight", ID, " 5"],
            A::NodeWeight,
            "weight ID GiB".into(),
        ),
        (
            vec!["node", "weight", ID, "1_0"],
            A::NodeWeight,
            "weight ID GiB".into(),
        ),
        (
            vec!["node", "weight", ID, "0x10"],
            A::NodeWeight,
            "weight ID GiB".into(),
        ),
        (
            vec!["gc", "why", "zz"],
            A::GcWhy,
            "why KEY (64 hex chars)".into(),
        ),
        (vec!["gc", "why"], A::GcWhy, "why KEY (64 hex chars)".into()),
        (
            vec!["gc", "why", &key31],
            A::GcWhy,
            "why KEY (64 hex chars)".into(),
        ),
        (
            vec!["gc", "why", &key33],
            A::GcWhy,
            "why KEY (64 hex chars)".into(),
        ),
        (
            vec!["gc", "why", &odd],
            A::GcWhy,
            "why KEY (64 hex chars)".into(),
        ),
    ];
    let mut cases = cases;
    // ("-1" as R is a flag to the framework, as in Go: `flag provided but not defined: -1`.)
    for r in ["x", "", "300", "256", "+3", "0x3", " 3", "1_0", "3.0"] {
        cases.push((
            vec!["cluster", "replicas", r],
            A::ClusterReplicas,
            "replicas R".into(),
        ));
    }
    for (argv, cmd, want) in cases {
        match cmd.request(&context(&argv)) {
            Ok(r) => panic!("{argv:?}: accepted as {r:?}"),
            Err(e) => assert_eq!(text(e), want, "{argv:?}"),
        }
    }
}

#[test]
fn accepted_argument_forms() {
    use AdminCommand as A;
    // Leading zeros are decimal digits (base 10, not 0).
    assert_eq!(
        request(&["node", "weight", ID, "010"], A::NodeWeight).weight,
        10
    );
    assert_eq!(
        request(&["cluster", "replicas", "003"], A::ClusterReplicas).replicas,
        3
    );
    // Upper-case ids and keys.
    let upper = ID.to_uppercase();
    assert_eq!(
        request(&["node", "remove", &upper], A::NodeRemove).node,
        golden::hex(ID)
    );
    // An empty zone is sent (omitted from the CBOR), and extra arguments are ignored.
    let r = request(&["node", "zone", ID, "", "extra"], A::NodeZone);
    assert_eq!((r.zone.as_str(), r.node.len()), ("", 32));
    // `voter add` has no --allow-unsafe; `voter remove` reads it.
    assert!(!request(&["voter", "add", ID], A::VoterAdd).allow_unsafe);
    assert!(request(&["voter", "remove", "--allow-unsafe", ID], A::VoterRemove).allow_unsafe);
    // uint32(c.Uint("weight")) keeps the low 32 bits.
    let w = request(
        &["token", "create", "--weight", "4294967303"],
        A::TokenCreate,
    )
    .weight;
    assert_eq!(w, 7);
}

/// Go strings from argv that are not UTF-8: `%q` echoes the bytes, and a zone reaches the CBOR text field
/// lossily (PORTING.md DD-8).
#[test]
fn non_utf8_arguments() {
    let os = |b: &[u8]| OsString::from_vec(b.to_vec());
    let argv = vec![os(b"node"), os(b"remove"), os(b"ab\xffcd")];
    match AdminCommand::NodeRemove.request(&context_os(argv)) {
        Ok(r) => panic!("accepted {r:?}"),
        Err(e) => assert_eq!(text(e), r#"view: bad node id "ab\xffcd""#),
    }
    let argv = vec![os(b"node"), os(b"zone"), os(ID.as_bytes()), os(b"z\xffne")];
    match AdminCommand::NodeZone.request(&context_os(argv)) {
        Ok(r) => assert_eq!(r.zone, "z\u{fffd}ne"),
        Err(e) => panic!("{}", text(e)),
    }
    let argv = vec![os(b"node"), os(b"weight"), os(ID.as_bytes()), os(b"1\xff")];
    match AdminCommand::NodeWeight.request(&context_os(argv)) {
        Ok(r) => panic!("accepted {r:?}"),
        Err(e) => assert_eq!(text(e), "weight ID GiB"),
    }
}

/// `node join`'s checks before `openNode`: the seed first (its `ticket.Parse` error verbatim), then the
/// token.
#[test]
fn node_join_seed_and_token() {
    let token = "01".repeat(32);
    let check = |seed: &str, tok: &str| {
        args::join_seed_and_token(&context(&["node", "join", "--seed", seed, "--token", tok]))
            .map_err(text)
    };
    assert_eq!(check("", ""), Err("ticket: empty".to_string()));
    assert_eq!(
        check("bogus", "x"),
        Err(
            r#"ticket: "bogus" is neither a dstore1 ticket nor a node id: invalid length"#
                .to_string()
        )
    );
    let off_curve = "07".repeat(32);
    assert_eq!(
        check(&off_curve, &token),
        Err(format!(
            "ticket: \"{off_curve}\" is neither a dstore1 ticket nor a node id: data is not a valid public key"
        ))
    );
    let bad_token = Err("token must be 32 bytes of hex".to_string());
    assert_eq!(check(ID, "zz"), bad_token);
    assert_eq!(check(ID, &"01".repeat(31)), bad_token);
    assert_eq!(check(ID, &"01".repeat(33)), bad_token);
    assert_eq!(check(ID, &format!("{token}0")), bad_token);
    assert_eq!(check(ID, ""), bad_token);
    assert_eq!(check(ID, &token), Ok(()));
    assert_eq!(check(ID, &"AB".repeat(32)), Ok(()));
}

/// `catalog restore`'s argument: a readable file wins, else a 32-byte key, else `restore KEY|FILE`.
#[test]
fn catalog_restore_argument() {
    let dir = match tempfile::Builder::new()
        .prefix("dstore-cli-admin-")
        .tempdir()
    {
        Ok(d) => d,
        Err(e) => panic!("temp dir: {e}"),
    };
    let root = dstore_gocompat::path::from_path(dir.path());
    let path = |name: &str| [&root[..], b"/", name.as_bytes()].concat();
    let key = "5a".repeat(32);
    let write = |name: &str, data: &[u8]| {
        if let Err(e) = std::fs::write(dir.path().join(name), data) {
            panic!("write {name}: {e}");
        }
    };
    write("backup.bin", b"\x00\x01backup");
    // A file named like a key is read as a file.
    write(&key, b"file");
    let wrong = |arg: &[u8]| match args::restore_source(arg) {
        Ok(s) => panic!("{arg:?}: {s:?}"),
        Err(e) => assert_eq!(text(e), "restore KEY|FILE", "{arg:?}"),
    };
    let ok = |arg: &[u8]| match args::restore_source(arg) {
        Ok(s) => s,
        Err(e) => panic!("{arg:?}: {}", text(e)),
    };
    assert_eq!(
        ok(&path("backup.bin")),
        RestoreSource::File(b"\x00\x01backup".to_vec())
    );
    assert_eq!(ok(&path(&key)), RestoreSource::File(b"file".to_vec()));
    assert_eq!(ok(key.as_bytes()), RestoreSource::Key([0x5a; 32]));
    assert_eq!(
        ok(key.to_uppercase().as_bytes()),
        RestoreSource::Key([0x5a; 32])
    );
    wrong(b"");
    wrong(&root);
    wrong(&path("missing"));
    wrong("5a".repeat(31).as_bytes());
    wrong(format!("{key}5").as_bytes());
}

// ---- the cluster replicas prompt ----

#[test]
fn replicas_prompt_text() {
    assert_eq!(
        args::replicas_prompt(3),
        "changing R to 3 moves about 1/3 of every node's data; continue? [y/N] "
    );
    assert_eq!(
        args::replicas_prompt(0),
        "changing R to 0 moves about 1/0 of every node's data; continue? [y/N] "
    );
    assert_eq!(
        args::replicas_prompt(255),
        "changing R to 255 moves about 1/255 of every node's data; continue? [y/N] "
    );
}

/// Go 1.26.5 `var ans string; fmt.Fscanln(r, &ans)` over a reader that is not an `io.RuneScanner` (as
/// `os.Stdin`), recorded from a throwaway Go program (port-notes/impl-cli-admin.md): input, whether the reader
/// fails after the input instead of reaching EOF, `ans`, the bytes consumed, and
/// `strings.HasPrefix(strings.ToLower(ans), "y")`.
const SCANLN_GO: &[(&str, bool, &str, usize, bool)] = &[
    ("6e0a", false, "6e", 2, false),
    ("", false, "", 0, false),
    ("79657320706c656173650a", false, "796573", 5, true),
    ("59", false, "59", 1, true),
    ("4e0a", false, "4e", 2, false),
    ("0a", false, "", 1, false),
    ("2020790a", false, "79", 4, true),
    ("6e6f0a", false, "6e6f", 3, false),
    ("792065787472610a", false, "79", 3, true),
    ("7965730a", false, "796573", 4, true),
    ("0d0a", false, "", 2, false),
    ("0d790a", false, "79", 3, true),
    ("0d0d0a790a", false, "", 3, false),
    ("09790a", false, "79", 3, true),
    ("c2a0790a", false, "79", 4, true),
    ("e380807965730a", false, "796573", 7, true),
    ("79c2a0780a", false, "79", 4, true),
    ("ff790a", false, "efbfbd79", 3, false),
    ("79ff0a", false, "79efbfbd", 3, true),
    ("e228790a", false, "efbfbd2879", 4, false),
    ("c3a90a", false, "c3a9", 3, false),
    ("79c3a9206f6b0a", false, "79c3a9", 5, true),
    ("200a20790a", false, "", 2, false),
    ("5965730d0a", false, "596573", 5, true),
    ("79202020", false, "79", 4, true),
    ("79850a", false, "79efbfbd", 3, true),
    ("8579", false, "efbfbd79", 2, false),
    ("c285790a", false, "79", 4, true),
    ("e280a8790a", false, "79", 5, true),
    ("f09f9880790a", false, "f09f988079", 6, false),
    ("e282", false, "efbfbdefbfbd", 2, false),
    ("79e282", false, "79efbfbdefbfbd", 3, true),
    ("eda080790a", false, "efbfbdefbfbdefbfbd79", 5, false),
    (
        "f4908080790a",
        false,
        "efbfbdefbfbdefbfbdefbfbd79",
        6,
        false,
    ),
    ("e08079", false, "efbfbdefbfbd79", 3, false),
    ("790d", false, "79", 2, true),
    ("790d7a0a", false, "79", 3, true),
    ("2020092020", false, "", 5, false),
    ("7979007a0a", false, "7979007a", 5, true),
    ("00790a", false, "0079", 3, false),
    ("c5b865730a", false, "c5b86573", 5, false),
    ("e284aa790a", false, "e284aa79", 5, false),
    ("790a0a6d6f72650a", false, "79", 2, true),
    ("79200a", false, "79", 3, true),
    ("7920092078", false, "79", 5, true),
    ("", true, "", 0, false),
    ("79", true, "", 1, false),
    ("7920", true, "79", 2, true),
    ("202079", true, "", 3, false),
    ("e2", true, "", 1, false),
    ("7965730a", true, "796573", 4, true),
    // Added by review-cli-admin from a second throwaway Go 1.26.5 probe (the 51 rows above re-verified by
    // it too): a lone `\r`, VT and FS, U+1680 and U+200B, `YES`, read errors after a newline or `\r`,
    // truncated and complete 4-byte runes, U+2028/U+0085 after the word, and more ill-formed UTF-8.
    ("0d", false, "", 1, false),
    ("0d", true, "", 1, false),
    ("0d0d", false, "", 2, false),
    ("790d0d0a", false, "79", 4, true),
    ("0b790a", false, "79", 3, true),
    ("1c790a", false, "1c79", 3, false),
    ("e19a80790a", false, "79", 5, true),
    ("e2808b790a", false, "e2808b79", 5, false),
    ("594553", false, "594553", 3, true),
    ("790a", true, "79", 2, true),
    ("0d0a", true, "", 2, false),
    ("f09f98", false, "efbfbdefbfbdefbfbd", 3, false),
    ("f09f9880", false, "f09f9880", 4, false),
    ("79e280a8", false, "79", 4, true),
    ("79c2850a", false, "79", 4, true),
    ("c285", false, "", 2, false),
    ("79", false, "79", 1, true),
    ("7920", false, "79", 2, true),
    ("2079", false, "79", 2, true),
    ("0a790a", false, "", 1, false),
    ("790d", true, "79", 2, true),
    ("e282ac790a", false, "e282ac79", 5, false),
    ("f0e282ac0a", false, "efbfbde282ac", 5, false),
    ("f09041790a", false, "efbfbdefbfbd4179", 5, false),
    ("ed9fbf790a", false, "ed9fbf79", 5, false),
    ("f48fbfbf0a", false, "f48fbfbf", 5, false),
    ("c0af790a", false, "efbfbdefbfbd79", 4, false),
];

/// A reader that yields `data`, then fails with a non-EOF error.
struct FailAfter<'a> {
    data: &'a [u8],
    n: usize,
}

impl Read for FailAfter<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let Some(slot) = buf.first_mut() else {
            return Ok(0);
        };
        match self.data.get(self.n) {
            Some(&b) => {
                *slot = b;
                self.n += 1;
                Ok(1)
            }
            None => Err(io::Error::other("boom")),
        }
    }
}

#[test]
fn scanln_matches_go() {
    for &(input_hex, fail_after, ans_hex, consumed, confirmed) in SCANLN_GO {
        let input = golden::hex(input_hex);
        let (ans, used) = if fail_after {
            let mut r = FailAfter { data: &input, n: 0 };
            let ans = args::scanln_word(&mut r);
            (ans, r.n)
        } else {
            let mut r: &[u8] = &input;
            let ans = args::scanln_word(&mut r);
            (ans, input.len() - r.len())
        };
        let case = format!("input {input_hex:?} (fail after: {fail_after})");
        assert_eq!(hex::encode(ans.as_bytes()), ans_hex, "{case}: ans");
        assert_eq!(used, consumed, "{case}: bytes consumed");
        assert_eq!(args::confirmed(&ans), confirmed, "{case}: confirmed");
    }
}
