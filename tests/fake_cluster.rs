//! Client and working-copy scenarios over `dstore_transport::mem` and `dstore_testkit::fake`: ports of
//! `node/cluster_test.go`, `node/watch_test.go` and `worktree_test.go` (verification §4.6). No sockets.
//!
//! Part A (owner client-a): Dial, the stale-view refresh, backoff and node preference, reference operations
//! and paging, and the ports of `TestClusterWatchRefs`, `TestClusterWatchBadPattern`,
//! `TestClusterWatchLostHint` and `TestClusterWatchReconnect`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use amber_store_core::{amberpack, fstree, reference::Reference};
use dstore_client::{Cluster, Cond, Config, Ctx, Error, Logger, NodeId, PutObserver, RefChange};
use dstore_gocompat::slog::{Attr, Handler, Level, Record};
use dstore_testkit::fake::{FakeCluster, FakeClusterConfig};
use dstore_transport::Endpoint;
use dstore_transport::mem::Network;
use dstore_wire::{CODE_BAD_REQUEST, T_REF_LIST, T_REF_PUT, T_STATUS, T_VIEW};
use tokio::sync::mpsc;

/// A log handler that drops everything.
struct Discard;

impl Handler for Discard {
    fn enabled(&self, _level: Level) -> bool {
        false
    }
    fn handle(&self, _handler_attrs: &[Attr], _r: &Record) {}
}

fn client_config(net: &Arc<Network>, fc: &FakeCluster) -> Config {
    let endpoint: Arc<dyn Endpoint> = net.bind(NodeId([0xc1; 32]), &[]);
    Config {
        endpoint: Some(endpoint),
        ticket: fc.ticket(),
        logger: Some(Logger::new(Arc::new(Discard))),
        request_timeout: Duration::from_secs(20),
        ..Config::default()
    }
}

async fn dial_with(cfg: Config) -> Cluster {
    match Cluster::dial(&Ctx::background(), cfg).await {
        Ok(c) => c,
        Err(e) => panic!("dial: {e}"),
    }
}

async fn cluster3(net: &Arc<Network>) -> Arc<FakeCluster> {
    FakeCluster::start(net, FakeClusterConfig::default()).await
}

/// Stores a blob at its owners through `Cluster::put`, returning its key.
async fn store_blob(c: &Cluster, data: &[u8]) -> [u8; 32] {
    let obj = fstree::encode_blob(data);
    let key = obj.key.0;
    let rec = match amberpack::encode_record(obj.key, &obj.bytes) {
        Ok(r) => r,
        Err(e) => panic!("encode record: {e}"),
    };
    let Some(primary) = c.primary(&key) else {
        panic!("no primary");
    };
    let size = rec.len();
    let res = c
        .put(
            &Ctx::background(),
            HashMap::from([(primary, vec![key])]),
            Arc::new(move |_| Ok(rec.clone())),
            Arc::new(move |_| size),
            PutObserver::default(),
        )
        .await;
    assert!(res.errors.is_empty(), "put errors: {:?}", res.errors.keys());
    assert!(
        res.holders.get(&key).is_some_and(|h| h.len() >= 2),
        "holders {:?}",
        res.holders.get(&key)
    );
    key
}

fn record(name: &str, root: &[u8; 32], user: &str) -> Vec<u8> {
    let r = Reference {
        name: name.to_owned(),
        key: root.to_vec(),
        user: user.to_owned(),
        created_at: dstore_gocompat::time::GoTime::now().unix_nano(),
        ..Reference::default()
    };
    match r.encode() {
        Ok(b) => b,
        Err(e) => panic!("encode reference {name}: {e}"),
    }
}

fn err<T>(r: Result<T, Error>, what: &str) -> Error {
    match r {
        Ok(_) => panic!("{what}: succeeded"),
        Err(e) => e,
    }
}

/// Polls until `cond` holds, for at most `d` (real time).
async fn eventually(what: &str, d: Duration, mut cond: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + d;
    while !cond() {
        if tokio::time::Instant::now() >= deadline {
            panic!("timeout waiting for {what}");
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

// ---- Dial, view refresh, backoff ----

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn client_dial_adopts_the_cluster_view() {
    let net = Network::new();
    let fc = cluster3(&net).await;
    let c = dial_with(client_config(&net, &fc)).await;
    assert_eq!(c.nodes(), fc.ids());
    assert_eq!(c.view().map(|v| v.encode()), Some(fc.view().encode()));
    let views = fc.requests(fc.ids()[0]);
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].typ, T_VIEW);
    assert_eq!(views[0].epoch, 0, "the dial request is unstamped");

    // The first member down: the next one answers.
    net.set_down(fc.ids()[0], true);
    let c2 = dial_with(client_config(&net, &fc)).await;
    assert_eq!(c2.nodes(), fc.ids());
    assert_eq!(fc.requests(fc.ids()[1]).len(), 1);
    // Every member down.
    for id in fc.ids() {
        net.set_down(id, true);
    }
    let e = err(
        Cluster::dial(&Ctx::background(), client_config(&net, &fc)).await,
        "dial",
    );
    let short = dstore_view::short_id(&fc.ids()[2]);
    assert_eq!(
        e.to_string(),
        format!("client: no bootstrap node answered: mem: {short} unreachable")
    );
    fc.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn client_newer_epoch_refreshes_the_view_and_stale_puts_retry() {
    let net = Network::new();
    let fc = cluster3(&net).await;
    let c = dial_with(client_config(&net, &fc)).await;
    let root = store_blob(&c, b"stale view").await;

    fc.bump_epoch();
    // A reply stamped with the newer epoch starts an async refresh.
    c.status(&Ctx::background(), fc.ids()[1])
        .await
        .expect("status");
    eventually("the refreshed view", Duration::from_secs(10), || {
        c.view().is_some_and(|v| v.epoch == 2)
    })
    .await;

    fc.bump_epoch();
    let rec = record("trees/stale", &root, "tester");
    let version = c
        .ref_put(&Ctx::background(), &rec, &Cond::default())
        .await
        .expect("ref put after stale-view");
    assert!(!version.is_empty());
    let puts: Vec<u64> = fc
        .ids()
        .iter()
        .flat_map(|id| fc.requests(*id))
        .filter(|m| m.typ == T_REF_PUT)
        .map(|m| m.epoch)
        .collect();
    assert_eq!(puts, [2, 3], "retried with the adopted epoch");
    assert_eq!(c.view().map(|v| v.epoch), Some(3));
    fc.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn client_backoff_moves_requests_to_live_nodes() {
    let net = Network::new();
    let fc = cluster3(&net).await;
    let c = dial_with(client_config(&net, &fc)).await;
    let ids = fc.ids();
    let ctx = Ctx::background();

    net.set_down(ids[0], true);
    let e = err(c.status(&ctx, ids[0]).await, "status of a down node");
    assert_eq!(
        e.to_string(),
        format!("mem: {} unreachable", dstore_view::short_id(&ids[0]))
    );
    let e = err(c.status(&ctx, ids[0]).await, "status again");
    assert_eq!(e.to_string(), "transport: peer recently unreachable");

    // The penalised node ranks last: reference listings go to the next node.
    let before = fc.requests(ids[0]).len();
    c.ref_list(&ctx, b"").await.expect("ref list");
    assert_eq!(fc.requests(ids[0]).len(), before);
    assert!(fc.requests(ids[1]).iter().any(|m| m.typ == T_REF_LIST));
    assert_eq!(
        c.read_order(&[0u8; 32]).last().copied(),
        Some(ids[0]),
        "read order puts the penalised owner last"
    );
    assert_eq!(
        fc.requests(ids[1])
            .iter()
            .filter(|m| m.typ == T_STATUS)
            .count(),
        0
    );
    fc.close().await;
}

// ---- References ----

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn client_reference_operations() {
    let net = Network::new();
    let fc = cluster3(&net).await;
    let c = dial_with(client_config(&net, &fc)).await;
    let ctx = Ctx::background();
    let root = store_blob(&c, b"refs").await;
    let rec = record("trees/a", &root, "alice");

    let must_not_exist = Cond {
        versioned: true,
        ..Cond::default()
    };
    let v1 = c
        .ref_put(&ctx, &rec, &must_not_exist)
        .await
        .expect("create");
    // Go catalog.RefPut: the same record again is a retry after a lost reply, and succeeds.
    let again = c
        .ref_put(&ctx, &rec, &must_not_exist)
        .await
        .expect("retry of the same record");
    assert_eq!(again, v1);
    let other = record("trees/a", &root, "bob");
    let e = err(
        c.ref_put(&ctx, &other, &must_not_exist).await,
        "create again",
    );
    let cm = e.cas_mismatch().expect("cas mismatch");
    assert!(cm.has_current);
    assert_eq!(cm.current, root);
    assert_eq!(cm.version, v1);
    assert_eq!(
        e.to_string(),
        format!("cas mismatch: current key {}", hex::encode(root))
    );

    let r = c.ref_get(&ctx, "trees/a").await.expect("ref get");
    assert_eq!(r.name, "trees/a");
    assert_eq!(r.record, rec);
    assert_eq!(r.version, v1);
    assert_eq!(r.reference.key, root);
    assert_eq!(r.reference.user, "alice");

    // Keyed on the current key: a new version.
    let keyed = Cond {
        keyed: true,
        expected_old: root.to_vec(),
        ..Cond::default()
    };
    let v2 = c.ref_put(&ctx, &rec, &keyed).await.expect("keyed put");
    assert_ne!(v2, v1);

    let stale = Cond {
        versioned: true,
        expected_version: v1.clone(),
        ..Cond::default()
    };
    let e = err(c.ref_delete(&ctx, "trees/a", &stale).await, "stale delete");
    assert!(e.cas_mismatch().is_some(), "{e}");
    let force = Cond {
        force: true,
        ..Cond::default()
    };
    c.ref_delete(&ctx, "trees/a", &force).await.expect("delete");
    let e = err(c.ref_get(&ctx, "trees/a").await, "get deleted");
    assert!(e.is_unknown_ref());
    assert_eq!(e.to_string(), "client: unknown reference");

    // A root the cluster does not hold.
    let absent = [
        0x00, 0x05, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22,
        23, 24, 25, 26, 27, 28, 29, 30,
    ];
    let e = err(
        c.ref_put(&ctx, &record("trees/x", &absent, "alice"), &force)
            .await,
        "incomplete",
    );
    let inc = e.incomplete().expect("incomplete");
    assert_eq!(inc.shortfall, 1);
    assert_eq!(inc.sample, [absent]);
    assert_eq!(e.to_string(), "incomplete: 1 keys short");

    // Server-side validation answers bad-request.
    let e = err(c.ref_put(&ctx, &[0xa0], &force).await, "bad record");
    assert!(e.is_code(CODE_BAD_REQUEST), "{e}");
    fc.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn client_ref_list_follows_pages() {
    let net = Network::new();
    let fc = cluster3(&net).await;
    let c = dial_with(client_config(&net, &fc)).await;
    let ctx = Ctx::background();
    let root = store_blob(&c, b"paging").await;
    for name in [
        "trees/0", "trees/1", "trees/2", "trees/3", "trees/4", "other/x",
    ] {
        fc.ref_put_local(fc.ids()[0], &record(name, &root, "tester"))
            .await
            .expect("put local");
    }
    fc.set_ref_page_limit(2);
    let got = c.ref_list(&ctx, b"trees/").await.expect("ref list");
    let names: Vec<&str> = got.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(
        names,
        ["trees/0", "trees/1", "trees/2", "trees/3", "trees/4"]
    );
    assert!(
        got.iter()
            .all(|r| r.key.as_deref() == Some(&root[..]) && r.user == "tester")
    );
    let pages = fc
        .ids()
        .iter()
        .flat_map(|id| fc.requests(*id))
        .filter(|m| m.typ == T_REF_LIST)
        .count();
    assert_eq!(pages, 3);
    let all = c.ref_list(&ctx, b"").await.expect("list all");
    assert_eq!(all.len(), 6);
    fc.close().await;
}

// ---- node/watch_test.go ----

/// `startWatch`: the watch runs on its own task and forwards events, as the Go test's goroutine does.
fn start_watch(
    c: &Cluster,
    ctx: &Ctx,
    pattern: &str,
    known: HashMap<String, Vec<u8>>,
) -> (
    mpsc::Receiver<RefChange>,
    tokio::task::JoinHandle<Option<Error>>,
) {
    let (tx, rx) = mpsc::channel(1024);
    let mut s = c.watch_refs(ctx.clone(), pattern.to_owned(), known);
    let done = tokio::spawn(async move {
        while let Some(item) = std::future::poll_fn(|cx| s.as_mut().poll_next(cx)).await {
            match item {
                Ok(ev) => {
                    if tx.send(ev).await.is_err() {
                        return None;
                    }
                }
                Err(e) => return Some(e),
            }
        }
        None
    });
    (rx, done)
}

/// Reads events until `pred` accepts one, returning every event read.
async fn until(
    rx: &mut mpsc::Receiver<RefChange>,
    d: Duration,
    what: &str,
    pred: impl Fn(&RefChange) -> bool,
) -> Vec<RefChange> {
    let mut out = Vec::new();
    let deadline = tokio::time::Instant::now() + d;
    loop {
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Some(ev)) => {
                let hit = pred(&ev);
                out.push(ev);
                if hit {
                    return out;
                }
            }
            Ok(None) => panic!("watch ended waiting for {what}; got {out:?}"),
            Err(_) => panic!("timeout waiting for {what}; got {out:?}"),
        }
    }
}

async fn change(rx: &mut mpsc::Receiver<RefChange>, d: Duration, name: &str) -> RefChange {
    let evs = until(rx, d, name, |ev| ev.name == name && !ev.deleted).await;
    evs[evs.len() - 1].clone()
}

fn other_node(fc: &FakeCluster, not: NodeId) -> NodeId {
    match fc.ids().into_iter().find(|id| *id != not) {
        Some(id) => id,
        None => panic!("no other node"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cluster_watch_refs() {
    let net = Network::new();
    let fc = cluster3(&net).await;
    fc.set_watch_reconcile(Duration::from_secs(60));
    let c = dial_with(client_config(&net, &fc)).await;
    let ctx = Ctx::background();
    let root_a = store_blob(&c, b"tree a").await;
    let root_b = store_blob(&c, b"tree b").await;
    let force = Cond {
        force: true,
        ..Cond::default()
    };
    for (name, root) in [
        ("trees/a", &root_a),
        ("trees/b", &root_b),
        ("other/x", &root_a),
    ] {
        c.ref_put(&ctx, &record(name, root, "tester"), &force)
            .await
            .expect("ref put");
    }

    let known = HashMap::from([
        ("trees/a".to_owned(), vec![0x5a; 32]),
        ("trees/gone".to_owned(), vec![0x6b; 32]),
    ]);
    let (mut rx, done) = start_watch(&c, &ctx, "trees/**", known);
    let initial = until(&mut rx, Duration::from_secs(20), "synced", |ev| ev.synced).await;
    let got: HashMap<String, RefChange> = initial
        .iter()
        .filter(|ev| !ev.synced)
        .map(|ev| (ev.name.clone(), ev.clone()))
        .collect();
    assert_eq!(got.len(), 3, "initial difference: {initial:?}");
    let a = &got["trees/a"];
    assert!(
        !a.deleted
            && a.key.as_deref() == Some(&root_a[..])
            && a.user == "tester"
            && !a.version.is_empty()
    );
    assert_eq!(got["trees/b"].key.as_deref(), Some(&root_b[..]));
    assert!(got["trees/gone"].deleted);
    assert_ne!(initial[initial.len() - 1].node, NodeId::default());

    // A write coordinated by any node reaches the watcher by hint.
    for (i, id) in fc.ids().into_iter().enumerate() {
        let name = format!("trees/c{i}");
        fc.ref_put_local(id, &record(&name, &root_a, "tester2"))
            .await
            .expect("put local");
        let ev = change(&mut rx, Duration::from_secs(5), &name).await;
        assert_eq!(ev.key.as_deref(), Some(&root_a[..]));
        assert_eq!(ev.user, "tester2");
    }
    // A deletion.
    c.ref_delete(&ctx, "trees/b", &force).await.expect("delete");
    until(
        &mut rx,
        Duration::from_secs(5),
        "deletion of trees/b",
        |ev| ev.name == "trees/b" && ev.deleted,
    )
    .await;
    // The same key again is not a change.
    let ids = fc.ids();
    fc.ref_put_local(ids[1], &record("trees/a", &root_a, "tester3"))
        .await
        .expect("put local");
    fc.ref_put_local(ids[2], &record("trees/d", &root_b, "tester2"))
        .await
        .expect("put local");
    let evs = until(&mut rx, Duration::from_secs(5), "trees/d", |ev| {
        ev.name == "trees/d"
    })
    .await;
    assert_eq!(evs.len(), 1, "unexpected events before trees/d: {evs:?}");
    // A name outside the pattern is silent.
    fc.ref_put_local(ids[0], &record("other/y", &root_b, "tester2"))
        .await
        .expect("put local");
    fc.ref_put_local(ids[0], &record("trees/e", &root_b, "tester2"))
        .await
        .expect("put local");
    let evs = until(&mut rx, Duration::from_secs(5), "trees/e", |ev| {
        ev.name == "trees/e"
    })
    .await;
    assert_eq!(evs.len(), 1, "unexpected events before trees/e: {evs:?}");

    ctx.cancel();
    match tokio::time::timeout(Duration::from_secs(10), done).await {
        Ok(Ok(None)) => {}
        other => panic!("watch did not end cleanly on cancel: {other:?}"),
    }
    fc.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cluster_watch_bad_pattern() {
    let net = Network::new();
    let fc = cluster3(&net).await;
    let c = dial_with(client_config(&net, &fc)).await;
    let (_rx, done) = start_watch(&c, &Ctx::background(), "trees/[", HashMap::new());
    match tokio::time::timeout(Duration::from_secs(20), done).await {
        Ok(Ok(Some(e))) => {
            assert!(e.is_code(CODE_BAD_REQUEST), "got {e}, want bad-request");
        }
        other => panic!("watch did not end with bad-request: {other:?}"),
    }
    fc.close().await;
}

/// The link between the coordinator of a write and the serving node is cut: the hint is lost and the
/// periodic reconcile delivers the change.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cluster_watch_lost_hint() {
    let net = Network::new();
    let fc = cluster3(&net).await;
    fc.set_watch_reconcile(Duration::from_millis(500));
    let c = dial_with(client_config(&net, &fc)).await;
    let ctx = Ctx::background();
    let root = store_blob(&c, b"lost hint").await;
    c.ref_put(
        &ctx,
        &record("trees/a", &root, "tester"),
        &Cond {
            force: true,
            ..Cond::default()
        },
    )
    .await
    .expect("ref put");
    let (mut rx, _done) = start_watch(&c, &ctx, "trees/*", HashMap::new());
    let initial = until(&mut rx, Duration::from_secs(20), "synced", |ev| ev.synced).await;
    let serving = initial[initial.len() - 1].node;
    let coord = other_node(&fc, serving);
    net.partition(coord, serving, true);
    fc.ref_put_local(coord, &record("trees/b", &root, "tester2"))
        .await
        .expect("put local");
    let ev = change(&mut rx, Duration::from_secs(15), "trees/b").await;
    assert_eq!(ev.key.as_deref(), Some(&root[..]));
    net.partition(coord, serving, false);
    ctx.cancel();
    fc.close().await;
}

/// The serving node goes down: the client moves the watch to another node and receives what changed
/// meanwhile.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cluster_watch_reconnect() {
    let net = Network::new();
    let fc = cluster3(&net).await;
    fc.set_watch_reconcile(Duration::from_secs(60));
    let cfg = Config {
        watch_idle: Duration::from_secs(5),
        ..client_config(&net, &fc)
    };
    let c = dial_with(cfg).await;
    let ctx = Ctx::background();
    let root = store_blob(&c, b"reconnect").await;
    c.ref_put(
        &ctx,
        &record("trees/a", &root, "tester"),
        &Cond {
            force: true,
            ..Cond::default()
        },
    )
    .await
    .expect("ref put");
    let (mut rx, _done) = start_watch(&c, &ctx, "trees/*", HashMap::new());
    let initial = until(&mut rx, Duration::from_secs(20), "synced", |ev| ev.synced).await;
    let serving = initial[initial.len() - 1].node;
    net.set_down(serving, true);
    let writer = other_node(&fc, serving);
    fc.ref_put_local(writer, &record("trees/b", &root, "tester2"))
        .await
        .expect("put local");

    let evs = until(&mut rx, Duration::from_secs(30), "synced again", |ev| {
        ev.synced
    })
    .await;
    let again = &evs[evs.len() - 1];
    assert!(
        again.node != serving && again.node != NodeId::default(),
        "resynced via {:?}, the node taken down",
        again.node
    );
    let mut seen = false;
    for ev in &evs {
        if ev.name == "trees/b" && !ev.deleted {
            seen = true;
        }
        assert_ne!(
            ev.name, "trees/a",
            "trees/a re-sent after reconnect: {ev:?}"
        );
    }
    if !seen {
        // The write may have landed after the new stream synced.
        change(&mut rx, Duration::from_secs(10), "trees/b").await;
    }
    // The new stream is live: a further change arrives.
    fc.ref_put_local(writer, &record("trees/c", &root, "tester2"))
        .await
        .expect("put local");
    change(&mut rx, Duration::from_secs(10), "trees/c").await;
    net.set_down(serving, false);
    ctx.cancel();
    fc.close().await;
}
