//! Working-copy flows over `dstore_transport::mem` and `dstore_testkit::fake` (owner worktree-flow).
//!
//! - Ports of `node/worktree_test.go`: `TestWorktreeInitPushCloneEditPull`, `TestWorktreeConflict` and
//!   `TestWorktreePushRecoversAfterLostState`.
//! - Port of `node/branch_test.go` `TestWorktreeBranch` (dstore v0.1.10), without its garbage-collection
//!   part (the fake nodes have no GC), and the branch scenarios beside it: a message on a plain reference,
//!   a message that is not UTF-8, and the up-to-date fetch of a branch.
//! - The flow scenarios of port-notes/worktree.md §2.9, §5 and §6:
//!   - a fetch of an unknown name, and the up-to-date fetch that does not re-pull;
//!   - the pull refusals, with the fetch state saved and base kept;
//!   - the push refusals, `force`, and both `ErrRefChanged` texts from real CAS mismatches;
//!   - the root-only `.dstore` exclusion;
//!   - clone and init clean-up;
//!   - a failed apply in pull (base kept) and in clone (applied files left in an existing directory),
//!     the working-copy quirks of PORTING §1.4;
//!   - `refresh_ticket`.
//! - The `flow/`, `cmd/`, `client/` and `ErrRefChanged` cases of `errors/worktree_text.json`.
//!
//! No sockets. Working copies live in real temporary directories under `$TMPDIR`; like Go's, the tests
//! assume no `.dstore` above it.

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use amber_store_core::commit::Commit;
use amber_store_core::fstree;
use amber_store_core::key::{Key, Type};
use dstore_client::{CasMismatch, Cluster, Cond, Config as ClientConfig, Ctx, Logger, NodeId};
use dstore_gocompat::slog::{Attr, Handler, Level, Record};
use dstore_testkit::fake::{FakeCluster, FakeClusterConfig};
use dstore_testkit::golden::{hex, load_json};
use dstore_testkit::splitmix;
use dstore_transport::Endpoint;
use dstore_transport::mem::Network;
use dstore_wire::{T_GET, T_MISSING};
use dstore_worktree::{self as wt, Change, Config, Kind, RemoteState, Tree};
use serde::Deserialize;

const REF_CHANGED: &str =
    "reference changed on the cluster since your last fetch: pull first, or --force";

// ---- harness ----

/// A log handler that drops everything.
struct Discard;

impl Handler for Discard {
    fn enabled(&self, _level: Level) -> bool {
        false
    }
    fn handle(&self, _handler_attrs: &[Attr], _r: &Record) {}
}

fn ok<T, E: std::fmt::Display>(r: Result<T, E>, what: &str) -> T {
    match r {
        Ok(v) => v,
        Err(e) => panic!("{what}: {e}"),
    }
}

fn fails<T, E>(r: Result<T, E>, what: &str) -> E {
    match r {
        Ok(_) => panic!("{what}: succeeded"),
        Err(e) => e,
    }
}

/// Go `cluster3`: three fake nodes, R = 3, and clients dialled into them.
struct Harness {
    net: Arc<Network>,
    fc: Arc<FakeCluster>,
}

impl Harness {
    async fn cluster3() -> Harness {
        let net = Network::new();
        let fc = FakeCluster::start(&net, FakeClusterConfig::default()).await;
        Harness { net, fc }
    }

    /// Go `h.client(t, n)`: a client on its own endpoint.
    async fn client(&self, n: u8) -> Cluster {
        let mut id = [0xc0u8; 32];
        id[31] = n;
        let endpoint: Arc<dyn Endpoint> = self.net.bind(NodeId(id), &[]);
        let cfg = ClientConfig {
            endpoint: Some(endpoint),
            ticket: self.fc.ticket(),
            logger: Some(Logger::new(Arc::new(Discard))),
            request_timeout: Duration::from_secs(20),
            ..ClientConfig::default()
        };
        ok(Cluster::dial(&Ctx::background(), cfg).await, "dial")
    }

    /// Client-ALPN requests of type `typ` read by the nodes so far.
    fn count(&self, typ: i64) -> usize {
        self.fc
            .ids()
            .into_iter()
            .map(|id| self.fc.requests(id).iter().filter(|m| m.typ == typ).count())
            .sum()
    }

    async fn close(self) {
        self.fc.close().await;
    }
}

fn tempdir() -> tempfile::TempDir {
    ok(
        tempfile::Builder::new().prefix("dstore-wt-flow-").tempdir(),
        "tempdir",
    )
}

fn bytes(p: &Path) -> Vec<u8> {
    p.as_os_str().as_bytes().to_vec()
}

fn root_of(t: &Tree) -> PathBuf {
    PathBuf::from(OsStr::from_bytes(&t.root))
}

/// `worktree.Config{Name: name}`.
fn cfg(name: &str) -> Config {
    Config {
        name: name.as_bytes().to_vec(),
        ..Config::default()
    }
}

fn empty_key() -> Key {
    wt::empty_tree().0
}

/// Go `writeWC`.
fn write_wc(root: &Path, rel: &str, content: &str) {
    let p = root.join(rel);
    if let Some(parent) = p.parent() {
        ok(std::fs::create_dir_all(parent), "mkdir");
    }
    ok(std::fs::write(&p, content), "write");
}

/// Go `readWC`.
fn read_wc(root: &Path, rel: &str) -> String {
    String::from_utf8_lossy(&ok(std::fs::read(root.join(rel)), rel)).into_owned()
}

/// Go `wcKinds`.
fn kinds(cs: &[Change]) -> BTreeMap<String, Kind> {
    cs.iter()
        .map(|c| (String::from_utf8_lossy(&c.path).into_owned(), c.kind))
        .collect()
}

fn kind_of(k: &BTreeMap<String, Kind>, p: &str) -> Option<Kind> {
    k.get(p).copied()
}

fn state_json(root: &Path) -> serde_json::Value {
    let b = ok(std::fs::read(root.join(".dstore/state")), "read state");
    ok(serde_json::from_slice(&b), "parse state")
}

fn is_empty_dir(p: &Path) -> bool {
    ok(std::fs::read_dir(p), "read dir").next().is_none()
}

// ---- node/worktree_test.go ----

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn worktree_init_push_clone_edit_pull() {
    let h = Harness::cluster3().await;
    let ctx = Ctx::background();
    let ca = h.client(100).await;

    // A: an existing directory becomes the working copy of a new name.
    let a = tempdir();
    write_wc(a.path(), "hello.txt", "hello\n");
    write_wc(a.path(), "sub/deep.txt", "deep\n");
    let (mut ta, fr) = ok(
        wt::init(&ctx, &ca, &bytes(a.path()), cfg("trees/wc"), None).await,
        "init",
    );
    assert!(!fr.exists, "a new name must not exist");
    let st = ok(ta.status(2), "status");
    let k = kinds(&st.changes);
    assert_eq!(kind_of(&k, "hello.txt"), Some(Kind::Added), "{k:?}");
    assert_eq!(kind_of(&k, "sub"), Some(Kind::Added), "{k:?}");
    assert_eq!(kind_of(&k, "sub/deep.txt"), Some(Kind::Added), "{k:?}");
    assert_eq!(st.remote, RemoteState::Absent);
    let pr = ok(
        ta.push(&ctx, &ca, "tester", b"", false, 2, None).await,
        "push",
    );
    assert!(!pr.nothing, "{pr:?}");
    let st = ok(ta.status(2), "status after push");
    assert!(st.changes.is_empty(), "{:?}", kinds(&st.changes));
    assert_eq!(st.meta_only, 0);
    assert_eq!(st.remote, RemoteState::UpToDate);
    let pr = ok(
        ta.push(&ctx, &ca, "tester", b"", false, 2, None).await,
        "second push",
    );
    assert!(pr.nothing, "{pr:?}");

    // B: a clone sees the push.
    let cb = h.client(101).await;
    let bt = tempdir();
    let b = bt.path().join("wc");
    let (mut tb, _) = ok(
        wt::clone(&ctx, &cb, &bytes(&b), cfg("trees/wc"), None).await,
        "clone",
    );
    assert_eq!(read_wc(&b, "hello.txt"), "hello\n");
    assert_eq!(read_wc(&b, "sub/deep.txt"), "deep\n");
    let st = ok(tb.status(2), "status after clone");
    assert!(st.changes.is_empty(), "{:?}", kinds(&st.changes));
    assert_eq!(st.remote, RemoteState::UpToDate);

    // B edits and pushes.
    write_wc(&b, "hello.txt", "hello world\n");
    write_wc(&b, "new.txt", "n\n");
    ok(std::fs::remove_file(b.join("sub/deep.txt")), "remove");
    let st = ok(tb.status(2), "status after edits");
    let k = kinds(&st.changes);
    assert_eq!(kind_of(&k, "hello.txt"), Some(Kind::Modified), "{k:?}");
    assert_eq!(kind_of(&k, "new.txt"), Some(Kind::Added), "{k:?}");
    assert_eq!(kind_of(&k, "sub/deep.txt"), Some(Kind::Deleted), "{k:?}");
    assert_eq!(st.changes.len(), 3, "{k:?}");
    ok(
        tb.push(&ctx, &cb, "tester", b"", false, 2, None).await,
        "push from B",
    );

    // A fetches, sees the move, pulls.
    let fr = ok(ta.fetch(&ctx, &ca, None).await, "fetch");
    assert!(!fr.up_to_date, "{fr:?}");
    let st = ok(ta.status(2), "status after fetch");
    let k = kinds(&st.incoming);
    assert_eq!(st.remote, RemoteState::Moved);
    assert_eq!(kind_of(&k, "hello.txt"), Some(Kind::Modified), "{k:?}");
    assert_eq!(kind_of(&k, "new.txt"), Some(Kind::Added), "{k:?}");
    assert_eq!(kind_of(&k, "sub/deep.txt"), Some(Kind::Deleted), "{k:?}");
    let (plr, res) = ta.pull(&ctx, &ca, false, 2, None).await;
    ok(res, "pull");
    assert!(plr.conflicts.is_empty(), "{plr:?}");
    assert_eq!(read_wc(a.path(), "hello.txt"), "hello world\n");
    assert_eq!(read_wc(a.path(), "new.txt"), "n\n");
    assert!(
        !a.path().join("sub/deep.txt").exists(),
        "sub/deep.txt should be gone"
    );
    let st = ok(ta.status(2), "status after pull");
    assert!(st.changes.is_empty(), "{:?}", kinds(&st.changes));
    assert_eq!(st.remote, RemoteState::UpToDate);
    let (plr, res) = ta.pull(&ctx, &ca, false, 2, None).await;
    ok(res, "second pull");
    assert!(plr.up_to_date, "{plr:?}");

    ok(ta.close(), "close A");
    ok(tb.close(), "close B");
    ca.close();
    cb.close();
    h.close().await;
}

/// Go `cloneTwo`: pushes a source tree under `name` and clones it twice.
struct Two {
    ta: Tree,
    tb: Tree,
    ca: Cluster,
    cb: Cluster,
    _dirs: Vec<tempfile::TempDir>,
}

async fn clone_two(h: &Harness, name: &str) -> Two {
    let ctx = Ctx::background();
    let c0 = h.client(102).await;
    let src = tempdir();
    write_wc(src.path(), "f.txt", "base\n");
    write_wc(src.path(), "other.txt", "other\n");
    let (mut t0, _) = ok(
        wt::init(&ctx, &c0, &bytes(src.path()), cfg(name), None).await,
        "init",
    );
    ok(
        t0.push(&ctx, &c0, "tester", b"", false, 2, None).await,
        "push",
    );
    ok(t0.close(), "close");
    c0.close();
    let (ca, cb) = (h.client(103).await, h.client(104).await);
    let (da, db) = (tempdir(), tempdir());
    let (ta, _) = ok(
        wt::clone(&ctx, &ca, &bytes(&da.path().join("a")), cfg(name), None).await,
        "clone a",
    );
    let (tb, _) = ok(
        wt::clone(&ctx, &cb, &bytes(&db.path().join("b")), cfg(name), None).await,
        "clone b",
    );
    Two {
        ta,
        tb,
        ca,
        cb,
        _dirs: vec![src, da, db],
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn worktree_conflict() {
    let h = Harness::cluster3().await;
    let ctx = Ctx::background();
    let Two {
        mut ta,
        mut tb,
        ca,
        cb,
        _dirs,
    } = clone_two(&h, "trees/conflict").await;
    let (ra, rb) = (root_of(&ta), root_of(&tb));

    write_wc(&ra, "f.txt", "A\n");
    write_wc(&rb, "f.txt", "B\n");
    write_wc(&rb, "b.txt", "b\n");
    let pa = ok(ta.push(&ctx, &ca, "a", b"", false, 2, None).await, "push A");
    // B has not fetched: the cluster refuses its push.
    let before = tb.state.clone();
    let e = fails(tb.push(&ctx, &cb, "b", b"", false, 2, None).await, "push B");
    assert!(matches!(e, wt::Error::RefChanged(_)), "push B: {e}");
    assert_eq!(
        e.to_string(),
        format!("{REF_CHANGED} (cas mismatch: current key {})", pa.root)
    );
    assert_eq!(tb.state, before, "a refused push leaves the state alone");

    let (plr, res) = tb.pull(&ctx, &cb, false, 2, None).await;
    let e = fails(res, "pull B");
    assert!(e.is_conflict(), "pull B: {e}");
    assert_eq!(plr.conflicts.len(), 1, "{plr:?}");
    assert_eq!(plr.conflicts[0].path, b"f.txt");
    assert!(plr.applied.is_empty(), "{plr:?}");
    assert_eq!(
        read_wc(&rb, "f.txt"),
        "B\n",
        "a refused pull must not touch the directory"
    );
    // The fetch was saved; base did not move.
    let st = state_json(&rb);
    assert_eq!(st["base"], before.base.to_string());
    assert_eq!(st["remote"], pa.root.to_string());

    let (plr, res) = tb.pull(&ctx, &cb, true, 2, None).await;
    ok(res, "forced pull");
    assert_eq!(
        read_wc(&rb, "f.txt"),
        "A\n",
        "forced pull: remote side on the conflict"
    );
    assert_eq!(
        read_wc(&rb, "b.txt"),
        "b\n",
        "forced pull: local addition kept"
    );
    // The conflicts' incoming changes come after the merged ones.
    assert_eq!(plr.conflicts.len(), 1, "{plr:?}");
    assert_eq!(
        plr.applied.last().map(|c| c.path.as_slice()),
        Some(b"f.txt".as_slice())
    );
    ok(
        tb.push(&ctx, &cb, "b", b"", false, 2, None).await,
        "push B after pull",
    );
    let (_, res) = ta.pull(&ctx, &ca, false, 2, None).await;
    ok(res, "pull A");
    assert_eq!(read_wc(&ra, "f.txt"), "A\n", "A after pull");
    assert_eq!(read_wc(&ra, "b.txt"), "b\n", "A after pull");

    ok(ta.close(), "close A");
    ok(tb.close(), "close B");
    ca.close();
    cb.close();
    h.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn worktree_push_recovers_after_lost_state() {
    let h = Harness::cluster3().await;
    let ctx = Ctx::background();
    let c = h.client(105).await;
    let dir = tempdir();
    write_wc(dir.path(), "f.txt", "1\n");
    let (mut tr, _) = ok(
        wt::init(&ctx, &c, &bytes(dir.path()), cfg("trees/recover"), None).await,
        "init",
    );
    ok(
        tr.push(&ctx, &c, "tester", b"", false, 2, None).await,
        "push",
    );
    write_wc(dir.path(), "f.txt", "2\n");
    let before = tr.state.clone();
    let pr = ok(
        tr.push(&ctx, &c, "tester", b"", false, 2, None).await,
        "push",
    );
    assert!(!pr.recovered, "{pr:?}");
    // The reference write landed but the state write was lost.
    tr.state = before;
    ok(tr.save_state(), "save state");
    let pr = ok(
        tr.push(&ctx, &c, "tester", b"", false, 2, None).await,
        "retried push",
    );
    assert!(pr.recovered, "{pr:?}");
    // The cluster's version is adopted.
    let r = ok(c.ref_get(&ctx, "trees/recover").await, "ref get");
    assert_eq!(
        tr.state.remote_version.as_deref(),
        Some(r.version.as_slice())
    );
    assert_eq!(
        state_json(dir.path())["remote_version"],
        hex::encode(&r.version)
    );
    let st = ok(tr.status(2), "status after recovery");
    assert!(st.changes.is_empty(), "{:?}", kinds(&st.changes));
    assert_eq!(st.remote, RemoteState::UpToDate);

    ok(tr.close(), "close");
    c.close();
    h.close().await;
}

// ---- branches: a reference naming a commit (node/branch_test.go) ----

/// Go `readCommit`: the commit `k` from the working copy's store.
fn read_commit(t: &Tree, k: Key) -> Commit {
    let data = ok(t.get(k), "commit object");
    ok(Commit::decode(&data), "decode commit")
}

fn is_commit(k: Key) -> bool {
    Type::from_u8(k.0[0] >> 4) == Some(Type::Commit)
}

/// A reference naming a commit is a branch: working copies clone and pull its tree, every push adds a
/// commit on top, and transfers carry the whole history.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn worktree_branch() {
    let h = Harness::cluster3().await;
    let ctx = Ctx::background();
    let ca = h.client(110).await;

    // A message turns a new reference into a branch.
    let a = tempdir();
    write_wc(a.path(), "f.txt", "one\n");
    let (mut ta, _) = ok(
        wt::init(&ctx, &ca, &bytes(a.path()), cfg("trees/branch"), None).await,
        "init",
    );
    let pr = ok(
        ta.push(&ctx, &ca, "alice", b"first", false, 2, None).await,
        "push",
    );
    assert!(is_commit(pr.commit), "{pr:?}");
    let (first, tree1) = (pr.commit, pr.root);
    let c = read_commit(&ta, first);
    assert_eq!(c.tree, tree1);
    assert!(c.parents.is_empty(), "{c:?}");
    assert_eq!(c.message, "first");
    assert_eq!(c.author.name, "alice");
    assert_eq!(c.committer, c.author);
    assert!(
        c.author.when > 0 && (-1439..=1439).contains(&c.author.tz_offset),
        "{c:?}"
    );
    let r = ok(ca.ref_get(&ctx, "trees/branch").await, "ref get");
    assert_eq!(r.reference.key.as_slice(), &first.0[..]);
    let st = ok(ta.status(2), "status after push");
    assert!(st.changes.is_empty(), "{:?}", kinds(&st.changes));
    assert_eq!(st.remote, RemoteState::UpToDate);
    assert!(ta.state.is_branch());
    assert_eq!((ta.state.base, ta.state.remote), (tree1, tree1));
    assert_eq!(ta.state.remote_key(), first);
    assert_eq!(state_json(a.path())["remote"], tree1.to_string());
    assert_eq!(state_json(a.path())["remote_commit"], first.to_string());
    let pr = ok(
        ta.push(&ctx, &ca, "alice", b"", false, 2, None).await,
        "second push",
    );
    assert!(pr.nothing, "{pr:?}");

    // A clone checks out the commit's tree.
    let cb = h.client(111).await;
    let b = tempdir();
    let (mut tb, fr) = ok(
        wt::clone(
            &ctx,
            &cb,
            &bytes(&b.path().join("b")),
            cfg("trees/branch"),
            None,
        )
        .await,
        "clone",
    );
    assert_eq!((fr.key, fr.tree), (first, tree1));
    assert_eq!((tb.state.base, tb.state.remote_commit), (tree1, first));
    assert_eq!(read_wc(&root_of(&tb), "f.txt"), "one\n");

    // An up-to-date fetch of a branch compares the commit, and pulls nothing.
    let gets = h.count(T_GET);
    let fr = ok(tb.fetch(&ctx, &cb, None).await, "fetch");
    assert!(fr.exists && fr.up_to_date, "{fr:?}");
    assert_eq!((fr.key, fr.tree), (first, tree1));
    assert_eq!(h.count(T_GET), gets, "an up-to-date fetch pulled");

    // B's push, without a message, still commits on top of first.
    write_wc(&root_of(&tb), "f.txt", "two\n");
    let pr = ok(
        tb.push(&ctx, &cb, "bob", b"", false, 2, None).await,
        "push from B",
    );
    assert!(is_commit(pr.commit), "{pr:?}");
    let second = pr.commit;
    let c = read_commit(&tb, second);
    assert_eq!((c.tree, c.parents.as_slice()), (pr.root, &[first][..]));
    assert_eq!((c.author.name.as_str(), c.message.as_str()), ("bob", ""));

    // A pulls B's commit.
    let (plr, res) = ta.pull(&ctx, &ca, false, 2, None).await;
    ok(res, "pull");
    assert!(plr.conflicts.is_empty());
    assert_eq!(read_wc(a.path(), "f.txt"), "two\n");
    assert_eq!(ta.state.remote_commit, second);

    // An interrupted push (reference written, state lost) is recognised on retry although the retry's
    // commit carries a new timestamp.
    write_wc(a.path(), "f.txt", "three\n");
    let before = ta.state.clone();
    let pr = ok(
        ta.push(&ctx, &ca, "alice", b"", false, 2, None).await,
        "push",
    );
    assert!(!pr.recovered, "{pr:?}");
    let third = pr.commit;
    tokio::time::sleep(Duration::from_millis(2)).await; // a distinct timestamp for the retry
    ta.state = before.clone();
    let pr = ok(
        ta.push(&ctx, &ca, "alice", b"", false, 2, None).await,
        "retried push",
    );
    assert!(pr.recovered, "{pr:?}");
    assert_eq!(pr.commit, third);
    assert_eq!(ta.state.remote_commit, third);
    assert_eq!(state_json(a.path())["remote_commit"], third.to_string());
    assert_eq!(read_commit(&ta, third).parents, vec![second]);
    // The same lost state, but over another parent: the cluster's commit records the same tree onto
    // other parents, so it is not this push, and the reference changed.
    let mut stale = before;
    stale.remote_commit = first;
    let kept = std::mem::replace(&mut ta.state, stale);
    let e = fails(
        ta.push(&ctx, &ca, "alice", b"", false, 2, None).await,
        "a retry onto another parent",
    );
    assert!(e.to_string().starts_with(REF_CHANGED), "{e}");
    ta.state = kept;

    // History is carried: the first commit's tree is reachable only through the branch's ancestry, and
    // the nodes hold it; a fresh clone has the whole history locally.
    let hist = ok(fstree::reachable_keys(third, |k| ta.get(k)), "reachable");
    assert!(
        hist.contains(&tree1),
        "the first commit's tree is not reachable from the tip"
    );
    assert!(hist.contains(&first) && hist.contains(&second));
    let mut held: std::collections::HashSet<[u8; 32]> = std::collections::HashSet::new();
    for id in h.fc.ids() {
        held.extend(h.fc.stored(id).into_keys());
    }
    for k in &hist {
        assert!(
            held.contains(&k.0),
            "history key {k} ({}) is on no node",
            k.type_()
        );
    }
    let cc = h.client(112).await;
    let c_dir = tempdir();
    let (tc, fr) = ok(
        wt::clone(
            &ctx,
            &cc,
            &bytes(&c_dir.path().join("c")),
            cfg("trees/branch"),
            None,
        )
        .await,
        "clone of the history",
    );
    assert_eq!(fr.key, third);
    assert_eq!(read_wc(&root_of(&tc), "f.txt"), "three\n");
    let cloned = ok(
        fstree::reachable_keys(third, |k| tc.get(k)),
        "reachable in the clone",
    );
    assert_eq!(cloned.len(), hist.len());

    ok(ta.close(), "close");
    ok(tb.close(), "close");
    ok(tc.close(), "close");
    ca.close();
    cb.close();
    cc.close();
    h.close().await;
}

/// A message on a plain reference makes a root commit and so starts a branch; a message that is not
/// UTF-8 fails as Go's commit validation fails it, after the tree is built and before the reference moves.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_message_turns_a_plain_reference_into_a_branch() {
    let h = Harness::cluster3().await;
    let ctx = Ctx::background();
    let c = h.client(113).await;
    let dir = tempdir();
    write_wc(dir.path(), "f.txt", "1\n");
    let (mut tr, _) = ok(
        wt::init(&ctx, &c, &bytes(dir.path()), cfg("trees/plain"), None).await,
        "init",
    );
    let pr = ok(
        tr.push(&ctx, &c, "tester", b"", false, 2, None).await,
        "push",
    );
    assert_eq!(pr.commit, Key([0; 32]), "a plain push made a commit");
    assert!(!tr.state.is_branch());
    assert_eq!(tr.state.remote_key(), pr.root);
    assert!(state_json(dir.path()).get("remote_commit").is_none());
    let plain = ok(c.ref_get(&ctx, "trees/plain").await, "ref get");
    assert_eq!(plain.reference.key.as_slice(), &pr.root.0[..]);

    write_wc(dir.path(), "f.txt", "2\n");
    let e = fails(
        tr.push(&ctx, &c, "tester", b"caf\xe9", false, 2, None)
            .await,
        "a message that is not UTF-8",
    );
    assert_eq!(e.to_string(), "commit: commit message must be valid UTF-8");
    let r = ok(c.ref_get(&ctx, "trees/plain").await, "ref get");
    assert_eq!(r.version, plain.version, "the reference moved");

    let pr = ok(
        tr.push(&ctx, &c, "tester", "née".as_bytes(), false, 2, None)
            .await,
        "push with a message",
    );
    assert!(is_commit(pr.commit), "{pr:?}");
    let cm = read_commit(&tr, pr.commit);
    assert!(cm.parents.is_empty(), "{cm:?}");
    assert_eq!((cm.tree, cm.message.as_str()), (pr.root, "née"));
    assert!(tr.state.is_branch());
    let r = ok(c.ref_get(&ctx, "trees/plain").await, "ref get");
    assert_eq!(r.reference.key.as_slice(), &pr.commit.0[..]);

    ok(tr.close(), "close");
    c.close();
    h.close().await;
}

// ---- flow scenarios (worktree.md §2.9) ----

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fetch_of_an_unknown_name_and_the_up_to_date_fetch() {
    let h = Harness::cluster3().await;
    let ctx = Ctx::background();
    let c = h.client(106).await;
    let dir = tempdir();

    // An empty directory, an absent name.
    let (mut t, fr) = ok(
        wt::init(&ctx, &c, &bytes(dir.path()), cfg("trees/new"), None).await,
        "init",
    );
    assert!(!fr.exists && !fr.up_to_date, "{fr:?}");
    assert!(!t.state.has_remote);
    assert_eq!(t.state.remote_version, None);
    let st = state_json(dir.path());
    assert_eq!(st["base"], empty_key().to_string());
    assert_eq!(st["remote"], "");
    assert_eq!(st["remote_version"], "");

    // Nothing to pull.
    let (plr, res) = t.pull(&ctx, &c, false, 2, None).await;
    let e = fails(res, "pull");
    assert!(e.is_no_remote(), "{e}");
    assert!(!plr.fetch.exists && !plr.up_to_date, "{plr:?}");

    // An empty directory against an absent name: nothing to push, and the name stays absent.
    let pr = ok(
        t.push(&ctx, &c, "tester", b"", false, 2, None).await,
        "push",
    );
    assert!(pr.nothing, "{pr:?}");
    assert_eq!(pr.root, empty_key());
    assert!(fails(c.ref_get(&ctx, "trees/new").await, "ref get").is_unknown_ref());

    // A file: the push creates the name. A fetch is then up to date.
    write_wc(dir.path(), "x.txt", "x\n");
    let pr = ok(
        t.push(&ctx, &c, "tester", b"", false, 2, None).await,
        "push",
    );
    assert!(!pr.nothing && !pr.recovered, "{pr:?}");
    assert!(pr.stats.keys > 0, "{pr:?}");
    let fr = ok(t.fetch(&ctx, &c, None).await, "fetch");
    assert!(fr.exists && fr.up_to_date, "{fr:?}");
    assert_eq!(fr.key, pr.root);
    ok(t.close(), "close");

    // Go v0.1.9: a fetch does not re-pull when the remote is unchanged, even when the local packstore lost
    // its objects.
    ok(
        std::fs::remove_dir_all(dir.path().join(".dstore/packstore")),
        "remove packstore",
    );
    let mut t = ok(Tree::open(&bytes(dir.path())), "open");
    let (gets, missing) = (h.count(T_GET), h.count(T_MISSING));
    let fr = ok(t.fetch(&ctx, &c, None).await, "fetch");
    assert!(fr.exists && fr.up_to_date, "{fr:?}");
    assert_eq!(fr.stats, dstore_client::PullStats::default());
    assert_eq!((h.count(T_GET), h.count(T_MISSING)), (gets, missing));
    assert!(
        t.status(2).is_err(),
        "the base tree is gone from the packstore"
    );

    ok(t.close(), "close");
    c.close();
    h.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn push_refusals_and_force() {
    let h = Harness::cluster3().await;
    let ctx = Ctx::background();
    let name = "trees/refuse";
    let Two {
        mut ta,
        mut tb,
        ca,
        cb,
        _dirs,
    } = clone_two(&h, name).await;
    let (ra, rb) = (root_of(&ta), root_of(&tb));

    // The cluster moves; A fetches it.
    write_wc(&rb, "f.txt", "B\n");
    ok(tb.push(&ctx, &cb, "b", b"", false, 2, None).await, "push B");
    write_wc(&ra, "f.txt", "A\n");
    let fr = ok(ta.fetch(&ctx, &ca, None).await, "fetch");
    assert!(fr.exists && !fr.up_to_date, "{fr:?}");
    let before = ta.state.clone();
    let e = fails(
        ta.push(&ctx, &ca, "a", b"", false, 2, None).await,
        "push after the move",
    );
    assert!(matches!(e, wt::Error::RemoteMoved), "{e}");
    assert_eq!(ta.state, before);
    // --force replaces the reference unconditionally.
    let pr = ok(
        ta.push(&ctx, &ca, "a", b"", true, 2, None).await,
        "forced push",
    );
    assert!(!pr.nothing && !pr.recovered, "{pr:?}");
    let r = ok(ca.ref_get(&ctx, name).await, "ref get");
    assert_eq!(r.reference.key, pr.root.0.to_vec());
    assert_eq!(r.reference.user, "a");
    assert_eq!(
        ta.state.remote_version.as_deref(),
        Some(r.version.as_slice())
    );
    assert_eq!((ta.state.base, ta.state.remote), (pr.root, pr.root));

    // The reference is deleted on the cluster.
    ok(
        cb.ref_delete(
            &ctx,
            name,
            &Cond {
                force: true,
                ..Cond::default()
            },
        )
        .await,
        "ref delete",
    );
    let fr = ok(ta.fetch(&ctx, &ca, None).await, "fetch");
    assert!(!fr.exists, "{fr:?}");
    assert!(!ta.state.has_remote);
    let e = fails(
        ta.push(&ctx, &ca, "a", b"", false, 2, None).await,
        "push after the deletion",
    );
    assert!(matches!(e, wt::Error::RemoteDeleted), "{e}");
    // --force recreates it, even though the tree equals base.
    let pr2 = ok(
        ta.push(&ctx, &ca, "a", b"", true, 2, None).await,
        "forced push",
    );
    assert!(!pr2.nothing, "{pr2:?}");
    assert_eq!(pr2.root, pr.root);
    let r = ok(ca.ref_get(&ctx, name).await, "ref get");
    assert_eq!(r.reference.key, pr.root.0.to_vec());
    assert!(ta.state.has_remote);

    ok(ta.close(), "close A");
    ok(tb.close(), "close B");
    ca.close();
    cb.close();
    h.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn push_ref_changed_texts() {
    let v = error_vectors();
    let h = Harness::cluster3().await;
    let ctx = Ctx::background();
    let (c1, c2) = (h.client(110).await, h.client(111).await);
    let (d1, d2) = (tempdir(), tempdir());
    write_wc(d1.path(), "a.txt", "a\n");
    write_wc(d2.path(), "b.txt", "b\n");
    let (mut t1, fr1) = ok(
        wt::init(&ctx, &c1, &bytes(d1.path()), cfg("trees/race"), None).await,
        "init 1",
    );
    let (mut t2, fr2) = ok(
        wt::init(&ctx, &c2, &bytes(d2.path()), cfg("trees/race"), None).await,
        "init 2",
    );
    assert!(!fr1.exists && !fr2.exists);
    let p1 = ok(
        t1.push(&ctx, &c1, "one", b"", false, 2, None).await,
        "push 1",
    );

    // The name must be new, but the cluster holds the other copy's tree.
    let before = t2.state.clone();
    let e = fails(
        t2.push(&ctx, &c2, "two", b"", false, 2, None).await,
        "push 2",
    );
    assert!(matches!(e, wt::Error::RefChanged(_)), "{e}");
    assert_eq!(
        e.to_string(),
        format!("{REF_CHANGED} (cas mismatch: current key {})", p1.root)
    );
    assert_eq!(t2.state, before, "a refused push leaves the state alone");

    // The reference was deleted after the last sync: "reference is absent".
    ok(
        c2.ref_delete(
            &ctx,
            "trees/race",
            &Cond {
                force: true,
                ..Cond::default()
            },
        )
        .await,
        "ref delete",
    );
    write_wc(d1.path(), "a.txt", "a2\n");
    let e = fails(
        t1.push(&ctx, &c1, "one", b"", false, 2, None).await,
        "push 1",
    );
    assert!(matches!(e, wt::Error::RefChanged(_)), "{e}");
    assert_eq!(e.to_string(), want(&v, "flow/push-ref-changed-absent"));
    assert_eq!(
        format!("dstore: {e}\n"),
        want(&v, "cli-line/flow/push-ref-changed-absent")
    );

    ok(t1.close(), "close 1");
    ok(t2.close(), "close 2");
    c1.close();
    c2.close();
    h.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn push_excludes_only_the_root_dstore() {
    let h = Harness::cluster3().await;
    let ctx = Ctx::background();
    let c = h.client(112).await;
    let dir = tempdir();
    write_wc(dir.path(), "a.txt", "a\n");
    write_wc(dir.path(), "sub/.dstore", "nested\n");
    // A leftover applier temp file is ordinary data (Go v0.1.9).
    write_wc(dir.path(), ".dstore-tmp-123", "left\n");
    let (mut t, _) = ok(
        wt::init(&ctx, &c, &bytes(dir.path()), cfg("trees/excl"), None).await,
        "init",
    );
    let pr = ok(
        t.push(&ctx, &c, "tester", b"", false, 2, None).await,
        "push",
    );
    let store = Arc::clone(&t.store);
    let get = move |k: Key| store.get(k);
    let paths = kinds(&ok(wt::diff_trees(&get, empty_key(), pr.root), "diff"));
    for p in ["a.txt", "sub", "sub/.dstore", ".dstore-tmp-123"] {
        assert_eq!(kind_of(&paths, p), Some(Kind::Added), "{p}: {paths:?}");
    }
    assert!(
        !paths
            .keys()
            .any(|p| p == ".dstore" || p.starts_with(".dstore/")),
        "{paths:?}"
    );

    // A clone reproduces the nested .dstore file.
    let c2 = h.client(113).await;
    let other = tempdir();
    let (t2, _) = ok(
        wt::clone(
            &ctx,
            &c2,
            &bytes(&other.path().join("c")),
            cfg("trees/excl"),
            None,
        )
        .await,
        "clone",
    );
    assert_eq!(read_wc(&root_of(&t2), "sub/.dstore"), "nested\n");
    assert_eq!(read_wc(&root_of(&t2), ".dstore-tmp-123"), "left\n");

    ok(t.close(), "close");
    ok(t2.close(), "close clone");
    c.close();
    c2.close();
    h.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn clone_cleans_up_what_it_created() {
    let h = Harness::cluster3().await;
    let ctx = Ctx::background();
    let c = h.client(120).await;
    let base = tempdir();

    // An unknown name into a new directory: the directory is removed again.
    let target = base.path().join("new");
    let e = fails(
        wt::clone(&ctx, &c, &bytes(&target), cfg("trees/missing"), None).await,
        "clone",
    );
    assert!(matches!(e, wt::Error::UnknownRefName(_)), "{e}");
    assert_eq!(e.to_string(), "client: unknown reference: trees/missing");
    assert!(!target.exists());
    // MkdirAll created both levels; RemoveAll(dir) removes only the leaf, as in Go.
    let nested = base.path().join("x/y");
    fails(
        wt::clone(&ctx, &c, &bytes(&nested), cfg("trees/missing"), None).await,
        "clone",
    );
    assert!(!nested.exists());
    assert!(base.path().join("x").is_dir());
    // Into an existing empty directory: only .dstore goes.
    let empty = base.path().join("empty");
    ok(std::fs::create_dir(&empty), "mkdir");
    fails(
        wt::clone(&ctx, &c, &bytes(&empty), cfg("trees/missing"), None).await,
        "clone",
    );
    assert!(empty.is_dir() && is_empty_dir(&empty));

    // A successful clone: config and state on disk.
    let src = tempdir();
    write_wc(src.path(), "f.txt", "f\n");
    let (mut t0, _) = ok(
        wt::init(&ctx, &c, &bytes(src.path()), cfg("trees/cl"), None).await,
        "init",
    );
    let p0 = ok(
        t0.push(&ctx, &c, "tester", b"", false, 2, None).await,
        "push",
    );
    ok(t0.close(), "close");
    let dir = base.path().join("ok");
    let config = Config {
        ticket: b"t1".to_vec(),
        name: b"trees/cl".to_vec(),
        user: b"me".to_vec(),
        ..Config::default()
    };
    let (t, fr) = ok(
        wt::clone(&ctx, &c, &bytes(&dir), config.clone(), None).await,
        "clone",
    );
    assert!(fr.exists && !fr.up_to_date, "{fr:?}");
    assert_eq!(fr.key, p0.root);
    assert!(fr.stats.fetched > 0, "{fr:?}");
    assert_eq!((t.state.base, t.state.remote), (p0.root, p0.root));
    let r = ok(c.ref_get(&ctx, "trees/cl").await, "ref get");
    let st = state_json(&dir);
    assert_eq!(st["base"], p0.root.to_string());
    assert_eq!(st["remote"], p0.root.to_string());
    assert_eq!(st["remote_version"], hex::encode(&r.version));
    assert_eq!(
        String::from_utf8_lossy(&ok(std::fs::read(dir.join(".dstore/config")), "config")),
        "{\n  \"ticket\": \"t1\",\n  \"name\": \"trees/cl\",\n  \"user\": \"me\"\n}\n"
    );
    assert_eq!(read_wc(&dir, "f.txt"), "f\n");
    ok(t.close(), "close");
    let t = ok(Tree::open(&bytes(&dir)), "open the clone");
    assert_eq!(t.config, config);
    let st = ok(t.status(2), "status");
    assert!(st.changes.is_empty(), "{:?}", kinds(&st.changes));
    assert_eq!(st.remote, RemoteState::UpToDate);

    ok(t.close(), "close");
    c.close();
    h.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn init_of_an_existing_name_then_pull() {
    let h = Harness::cluster3().await;
    let ctx = Ctx::background();
    let c = h.client(121).await;
    let src = tempdir();
    write_wc(src.path(), "f.txt", "same\n");
    write_wc(src.path(), "g.txt", "remote\n");
    let (mut t0, _) = ok(
        wt::init(&ctx, &c, &bytes(src.path()), cfg("trees/init"), None).await,
        "init",
    );
    let p0 = ok(
        t0.push(&ctx, &c, "tester", b"", false, 2, None).await,
        "push",
    );
    ok(t0.close(), "close");

    let dir = tempdir();
    write_wc(dir.path(), "f.txt", "same\n");
    write_wc(dir.path(), "h.txt", "local\n");
    let (mut t, fr) = ok(
        wt::init(&ctx, &c, &bytes(dir.path()), cfg("trees/init"), None).await,
        "init",
    );
    assert!(fr.exists && !fr.up_to_date, "{fr:?}");
    assert_eq!(fr.key, p0.root);
    assert_eq!((t.state.base, t.state.remote), (empty_key(), p0.root));
    // status shows everything as new; the remote moved from the empty base.
    let st = ok(t.status(2), "status");
    assert_eq!(st.remote, RemoteState::Moved);
    let k = kinds(&st.changes);
    assert_eq!(kind_of(&k, "f.txt"), Some(Kind::Added), "{k:?}");
    assert_eq!(kind_of(&k, "h.txt"), Some(Kind::Added), "{k:?}");
    let k = kinds(&st.incoming);
    assert_eq!(kind_of(&k, "f.txt"), Some(Kind::Added), "{k:?}");
    assert_eq!(kind_of(&k, "g.txt"), Some(Kind::Added), "{k:?}");
    // Base and remote differ: push refuses.
    let e = fails(
        t.push(&ctx, &c, "tester", b"", false, 2, None).await,
        "push",
    );
    assert!(matches!(e, wt::Error::RemoteMoved), "{e}");
    // pull merges: the equal addition is no conflict, the local addition stays.
    let (plr, res) = t.pull(&ctx, &c, false, 2, None).await;
    ok(res, "pull");
    assert!(plr.conflicts.is_empty(), "{plr:?}");
    let applied = kinds(&plr.applied);
    assert!(
        applied.contains_key("f.txt") && applied.contains_key("g.txt"),
        "{applied:?}"
    );
    assert_eq!(read_wc(dir.path(), "g.txt"), "remote\n");
    assert_eq!(read_wc(dir.path(), "h.txt"), "local\n");
    let st = ok(t.status(2), "status after pull");
    assert_eq!(st.remote, RemoteState::UpToDate);
    let k = kinds(&st.changes);
    assert_eq!(k.len(), 1, "{k:?}");
    assert_eq!(kind_of(&k, "h.txt"), Some(Kind::Added), "{k:?}");

    ok(t.close(), "close");
    c.close();
    h.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn init_with_a_failed_fetch_leaves_nothing_behind() {
    let h = Harness::cluster3().await;
    let ctx = Ctx::background();
    let c = h.client(122).await;
    for id in h.fc.ids() {
        h.net.set_down(id, true);
    }
    let dir = tempdir();
    write_wc(dir.path(), "k.txt", "k\n");
    let e = fails(
        wt::init(&ctx, &c, &bytes(dir.path()), cfg("trees/down"), None).await,
        "init",
    );
    assert!(matches!(e, wt::Error::Client(_)), "{e}");
    assert!(!dir.path().join(".dstore").exists());
    assert_eq!(read_wc(dir.path(), "k.txt"), "k\n");

    c.close();
    h.close().await;
}

/// PORTING §1.4: a failed apply in `pull` does not move base; the fetch is still saved.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pull_with_a_failed_apply_keeps_base() {
    let h = Harness::cluster3().await;
    let ctx = Ctx::background();
    let name = "trees/applyfail";
    let c0 = h.client(140).await;
    let src = tempdir();
    write_wc(src.path(), "f.txt", "f\n");
    write_wc(src.path(), "sub/deep.txt", "deep\n");
    let (mut t0, _) = ok(
        wt::init(&ctx, &c0, &bytes(src.path()), cfg(name), None).await,
        "init",
    );
    ok(
        t0.push(&ctx, &c0, "tester", b"", false, 2, None).await,
        "push",
    );
    ok(t0.close(), "close");
    c0.close();
    let (ca, cb) = (h.client(141).await, h.client(142).await);
    let (da, db) = (tempdir(), tempdir());
    let (mut ta, _) = ok(
        wt::clone(&ctx, &ca, &bytes(&da.path().join("a")), cfg(name), None).await,
        "clone a",
    );
    let (mut tb, _) = ok(
        wt::clone(&ctx, &cb, &bytes(&db.path().join("b")), cfg(name), None).await,
        "clone b",
    );
    let (ra, rb) = (root_of(&ta), root_of(&tb));

    // A edits below sub and pushes; B turns sub into a file.
    write_wc(&ra, "sub/deep.txt", "deep A\n");
    let pa = ok(ta.push(&ctx, &ca, "a", b"", false, 2, None).await, "push A");
    ok(std::fs::remove_dir_all(rb.join("sub")), "remove sub");
    write_wc(&rb, "sub", "file\n");
    let before = tb.state.clone();

    // Forced, the conflict takes the cluster's side, which apply cannot write below a file.
    for round in 0..2 {
        let (plr, res) = tb.pull(&ctx, &cb, true, 2, None).await;
        let e = fails(res, "forced pull");
        assert_eq!(
            e.to_string(),
            format!("{}/sub: not a directory", rb.display()),
            "round {round}"
        );
        assert_eq!(plr.conflicts.len(), 1, "{plr:?}");
        let cf = &plr.conflicts[0];
        assert_eq!(cf.path, b"sub/deep.txt");
        assert_eq!(
            (cf.local.kind, cf.incoming.kind),
            (Kind::Deleted, Kind::Modified)
        );
        assert!(plr.applied.is_empty(), "{plr:?}");
        // The first pull fetched A's tree; the retry finds it already fetched.
        assert!(plr.fetch.exists, "{plr:?}");
        assert_eq!(plr.fetch.up_to_date, round == 1, "{plr:?}");
        assert_eq!(plr.fetch.key, pa.root);
        // Base and synced_at did not move, in memory or on disk; remote did.
        assert_eq!(tb.state.base, before.base);
        assert_eq!(tb.state.synced_at, before.synced_at);
        assert_eq!(tb.state.remote, pa.root);
        let st = state_json(&rb);
        assert_eq!(st["base"], before.base.to_string());
        assert_eq!(st["remote"], pa.root.to_string());
        assert_eq!(read_wc(&rb, "sub"), "file\n");
    }

    ok(ta.close(), "close A");
    ok(tb.close(), "close B");
    ca.close();
    cb.close();
    h.close().await;
}

/// PORTING §1.4: a clone whose apply fails part-way leaves the applied files in a directory that already
/// existed (only `.dstore` is removed), and removes a directory it created.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_apply_in_clone_leaves_files_in_an_existing_directory() {
    let h = Harness::cluster3().await;
    let ctx = Ctx::background();
    let c = h.client(143).await;
    let name = "trees/odd";

    // A tree built by hand: a file, then an entry of a type apply refuses (S_IFMT 0160000).
    let ps = tempdir();
    let store = Arc::new(ok(
        amber_store_core::packstore::Store::open_with(
            ps.path().join("packstore"),
            amber_store_core::packstore::Options::new().sync(false),
        ),
        "open packstore",
    ));
    let blob = fstree::encode_blob(b"a\n");
    let mtime = 1_700_000_000_000_000_000;
    let entries = [
        fstree::Entry {
            name: b"a.txt".to_vec(),
            mode: 0o100644,
            mtime,
            content_key: blob.key.0.to_vec(),
            ..fstree::Entry::default()
        },
        fstree::Entry {
            name: b"z".to_vec(),
            mode: 0o160644,
            mtime,
            ..fstree::Entry::default()
        },
    ];
    let leaf = ok(fstree::encode_dir_leaf(&entries), "encode dir leaf");
    ok(store.put(blob.key, &blob.bytes), "put blob");
    ok(store.put(leaf.key, &leaf.bytes), "put dir leaf");
    ok(
        c.push(
            &ctx,
            Arc::clone(&store),
            leaf.key,
            name,
            "tester",
            Cond::default(),
            None,
        )
        .await,
        "push",
    );
    ok(store.close(), "close packstore");

    let base = tempdir();
    let existing = base.path().join("existing");
    ok(std::fs::create_dir(&existing), "mkdir");
    let e = fails(
        wt::clone(&ctx, &c, &bytes(&existing), cfg(name), None).await,
        "clone into an existing directory",
    );
    assert_eq!(e.to_string(), "z: unsupported type 0160000");
    let left: Vec<_> = ok(std::fs::read_dir(&existing), "read dir")
        .map(|de| ok(de, "dir entry").file_name())
        .collect();
    assert_eq!(left, ["a.txt"], "only .dstore is removed");
    assert_eq!(read_wc(&existing, "a.txt"), "a\n");

    let fresh = base.path().join("fresh");
    let e = fails(
        wt::clone(&ctx, &c, &bytes(&fresh), cfg(name), None).await,
        "clone into a new directory",
    );
    assert_eq!(e.to_string(), "z: unsupported type 0160000");
    assert!(!fresh.exists(), "the directory clone created is removed");

    c.close();
    h.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn refresh_ticket_stores_the_view_ticket() {
    let h = Harness::cluster3().await;
    let ctx = Ctx::background();
    let c = h.client(123).await;
    let src = tempdir();
    write_wc(src.path(), "f.txt", "f\n");
    let (mut t0, _) = ok(
        wt::init(&ctx, &c, &bytes(src.path()), cfg("trees/tk"), None).await,
        "init",
    );
    ok(
        t0.push(&ctx, &c, "tester", b"", false, 2, None).await,
        "push",
    );
    ok(t0.close(), "close");

    let base = tempdir();
    let dir = base.path().join("c");
    let config = Config {
        ticket: b"stale".to_vec(),
        name: b"trees/tk".to_vec(),
        ..Config::default()
    };
    let (mut t, _) = ok(
        wt::clone(&ctx, &c, &bytes(&dir), config, None).await,
        "clone",
    );
    assert_eq!(t.config.ticket, b"stale");
    ok(t.refresh_ticket(&c), "refresh");
    let Some(view) = c.view() else {
        panic!("no view after dial");
    };
    let want_ticket = wt::ticket_from_view(&view).encode();
    assert_eq!(t.config.ticket, want_ticket.as_bytes());
    // Up to four members, in view order: the fake view has three.
    assert_eq!(
        ok(dstore_ticket::parse(want_ticket.as_bytes()), "parse ticket")
            .members
            .map(|m| m.len()),
        Some(3)
    );
    let config_file = dir.join(".dstore/config");
    let want_file = format!("{{\n  \"ticket\": \"{want_ticket}\",\n  \"name\": \"trees/tk\"\n}}\n");
    assert_eq!(
        String::from_utf8_lossy(&ok(std::fs::read(&config_file), "config")),
        want_file
    );
    // An unchanged ticket: the stored config is not rewritten.
    let marker = format!(
        "{{\n  \"ticket\": \"{want_ticket}\",\n  \"name\": \"trees/tk\",\n  \"user\": \"marker\"\n}}\n"
    );
    ok(std::fs::write(&config_file, &marker), "write marker");
    ok(t.refresh_ticket(&c), "refresh again");
    assert_eq!(
        String::from_utf8_lossy(&ok(std::fs::read(&config_file), "config")),
        marker
    );

    ok(t.close(), "close");
    c.close();
    h.close().await;
}

// ---- errors/worktree_text.json: flow/, cmd/, client/ and ErrRefChanged ----

#[derive(Deserialize)]
struct ErrorCase {
    name: String,
    out: Option<String>,
    out_hex: Option<String>,
}

#[derive(Deserialize)]
struct ErrorVectors {
    cases: Vec<ErrorCase>,
}

fn error_vectors() -> HashMap<String, String> {
    let v: ErrorVectors = load_json("errors/worktree_text.json");
    v.cases
        .into_iter()
        .map(|c| {
            let out = match (c.out, c.out_hex) {
                (Some(s), _) => s,
                (None, Some(h)) => String::from_utf8_lossy(&hex(&h)).into_owned(),
                (None, None) => String::new(),
            };
            (c.name, out)
        })
        .collect()
}

fn want<'a>(v: &'a HashMap<String, String>, name: &str) -> &'a str {
    v.get(name)
        .unwrap_or_else(|| panic!("errors/worktree_text.json has no case {name}"))
}

fn ref_changed(cm: CasMismatch) -> wt::Error {
    wt::Error::RefChanged(dstore_client::Error::CasMismatch(cm))
}

#[test]
fn ref_changed_client_and_cmd_texts() {
    let v = error_vectors();
    // vectorgen: k1 = wtBlobKey(smData(1, 100)).
    let k1 = fstree::encode_blob(&splitmix::data(1, 100)).key;
    let cases = [
        (
            "flow/push-ref-changed-absent",
            ref_changed(CasMismatch {
                current: Vec::new(),
                record: Vec::new(),
                version: vec![1, 2],
                has_current: false,
            }),
        ),
        (
            "flow/push-ref-changed-current",
            ref_changed(CasMismatch {
                current: k1.0.to_vec(),
                record: Vec::new(),
                version: vec![3],
                has_current: true,
            }),
        ),
        (
            "flow/push-ref-changed-current-short-key",
            ref_changed(CasMismatch {
                current: vec![0xab, 0xcd],
                record: Vec::new(),
                version: Vec::new(),
                has_current: true,
            }),
        ),
        (
            "flow/clone-unknown-ref",
            wt::Error::UnknownRefName(b"trees/x".to_vec()),
        ),
    ];
    for (name, e) in &cases {
        assert_eq!(e.to_string(), want(&v, name), "{name}");
    }
    let absent = ref_changed(CasMismatch {
        current: Vec::new(),
        record: Vec::new(),
        version: Vec::new(),
        has_current: false,
    });
    assert_eq!(
        format!("dstore: {absent}\n"),
        want(&v, "cli-line/flow/push-ref-changed-absent")
    );

    // ErrRefChanged is only ever returned wrapped: the sentinel's text, then " (<the client error>)", which
    // is also the source.
    let sentinel = want(&v, "worktree/ErrRefChanged");
    assert_eq!(sentinel, REF_CHANGED);
    assert_eq!(
        want(&v, "cli-line/worktree/ErrRefChanged"),
        format!("dstore: {sentinel}\n")
    );
    assert_eq!(
        absent.to_string(),
        format!("{sentinel} (cas mismatch: reference is absent)")
    );
    assert_eq!(
        std::error::Error::source(&absent).map(|s| s.to_string()),
        Some("cas mismatch: reference is absent".to_owned())
    );

    // client/ErrUnknownRef.
    assert_eq!(
        dstore_client::Error::UnknownRef.to_string(),
        want(&v, "client/ErrUnknownRef")
    );
    assert_eq!(
        format!("dstore: {}\n", dstore_client::Error::UnknownRef),
        want(&v, "cli-line/client/ErrUnknownRef")
    );

    // cmd/: the reference-name and push-user texts come from the library validators (wc.go wraps the
    // user's as "user: %w").
    let names: [(&str, Vec<u8>); 5] = [
        ("empty", Vec::new()),
        ("too-long", vec![b'n'; 1025]),
        ("invalid-utf8", b"trees/\xff".to_vec()),
        ("at", b"trees/a@b".to_vec()),
        ("control", b"trees/a\tb".to_vec()),
    ];
    for (name, input) in &names {
        let got = fails(dstore_client::validate_name_bytes(input), name);
        assert_eq!(
            got,
            want(&v, &format!("cmd/reference-name-{name}")),
            "{name}"
        );
    }
    let users: [(&str, Vec<u8>); 4] = [
        ("empty", Vec::new()),
        ("invalid-utf8", b"\xff".to_vec()),
        ("control", b"a\x01".to_vec()),
        ("too-long", vec![b'u'; 1025]),
    ];
    for (name, input) in &users {
        let got = format!(
            "user: {}",
            fails(dstore_client::validate_user_bytes(input), name)
        );
        assert_eq!(got, want(&v, &format!("cmd/push-user-{name}")), "{name}");
    }
}

/// vectorgen `wtTreeErrors`, the `flow/` cases: Init and Clone refusals on real temporary directories.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn flow_error_texts() {
    let v = error_vectors();
    let h = Harness::cluster3().await;
    let ctx = Ctx::background();
    let c = h.client(130).await;
    // vectorgen: cfg = worktree.Config{Ticket: "t", Name: "trees/x"}.
    let small = || Config {
        ticket: b"t".to_vec(),
        name: b"trees/x".to_vec(),
        ..Config::default()
    };
    let create = |root: &Path| {
        ok(
            ok(Tree::create(&bytes(root), small()), "create").close(),
            "close",
        )
    };
    let text =
        |e: &wt::Error, root: &Path| e.to_string().replace(&root.display().to_string(), "{ROOT}");
    let mut failures = Vec::new();
    let mut check = |name: &str, got: String| {
        if got != want(&v, name) {
            failures.push(format!("{name}: {got:?}, want {:?}", want(&v, name)));
        }
    };

    let root = tempdir();
    create(root.path());
    let e = fails(
        wt::init(
            &ctx,
            &c,
            &bytes(&root.path().join("sub/deeper")),
            small(),
            None,
        )
        .await,
        "init inside",
    );
    check("flow/init-inside", text(&e, root.path()));

    let root = tempdir();
    write_wc(root.path(), "file", "x");
    let e = fails(
        wt::clone(&ctx, &c, &bytes(&root.path().join("file")), small(), None).await,
        "clone onto a file",
    );
    check("flow/clone-not-a-directory", text(&e, root.path()));

    let root = tempdir();
    write_wc(root.path(), "x", "x");
    let e = fails(
        wt::clone(&ctx, &c, &bytes(root.path()), small(), None).await,
        "clone into a non-empty directory",
    );
    check("flow/clone-not-empty", text(&e, root.path()));

    let root = tempdir();
    write_wc(root.path(), "file", "x");
    let e = fails(
        wt::clone(
            &ctx,
            &c,
            &bytes(&root.path().join("file/sub")),
            small(),
            None,
        )
        .await,
        "clone below a file",
    );
    check("flow/clone-parent-is-a-file", text(&e, root.path()));

    let root = tempdir();
    create(root.path());
    ok(std::fs::create_dir(root.path().join("empty")), "mkdir");
    let e = fails(
        wt::clone(&ctx, &c, &bytes(&root.path().join("empty")), small(), None).await,
        "clone inside a working copy",
    );
    check("flow/clone-empty-directory-inside", text(&e, root.path()));
    // Clone did not create the directory, so it stays.
    assert!(root.path().join("empty").is_dir());

    assert!(failures.is_empty(), "{}", failures.join("\n"));
    c.close();
    h.close().await;
}

/// The `cmd/` literals of wc.go (`dstore_cli::cmd_wc`), through the CLI: each fails before any dial or
/// file-system change.
#[test]
fn cmd_error_texts_through_the_cli() {
    let v = error_vectors();
    let cwd = tempdir();
    let home = tempdir();
    let cases: [(&[&str], &str); 5] = [
        (&["clone"], "cmd/clone-usage"),
        (&["init"], "cmd/init-usage"),
        (
            &["diff", "--remote", "--incoming"],
            "cmd/remote-and-incoming",
        ),
        (&["clone", "trees/x"], "cmd/resolve-ticket"),
        (&["clone", "trees/a@b"], "cmd/reference-name-at"),
    ];
    for (args, name) in cases {
        let out = ok(
            std::process::Command::new(env!("CARGO_BIN_EXE_dstore"))
                .args(args)
                .current_dir(cwd.path())
                .env_clear()
                .env("PATH", std::env::var_os("PATH").unwrap_or_default())
                .env("HOME", home.path())
                .env("TZ", "UTC")
                .output(),
            "spawn dstore",
        );
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        assert_eq!(String::from_utf8_lossy(&out.stdout), "", "{args:?}");
        assert_eq!(
            String::from_utf8_lossy(&out.stderr),
            format!("dstore: {}\n", want(&v, name)),
            "{args:?}"
        );
    }
    assert!(
        is_empty_dir(cwd.path()),
        "the refused commands created files"
    );
}
