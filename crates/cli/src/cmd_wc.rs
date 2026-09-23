//! Working-copy actions `clone`, `init`, `fetch`, `pull`, `push`, `status`, `diff`, and `resolve_ticket`,
//! `wc_config`, `push_user`, `describe_change`, `filter_paths` (`cmd/dstore/wc.go`).
//!
//! Spec: port-notes/worktree.md §2.10 and §2.12, port-notes/cli.md §2.5 and §2.8.11, PORTING.md §1.4, §5.1,
//! §5.4.
//!
//! - The connection commands (clone, init, fetch, pull, push) call `common::signal_ctx()` and run their
//!   transfer under `progress::run_transfer`, as Go's `signalCtx` and `runTransfer` do. The steps before
//!   `signalCtx` (arguments, `wcConfig`, opening the working copy) register nothing.
//! - `status` and `diff` install no signal handler, so SIGINT kills them. They are blocking end to end
//!   (open, scan, diff, rendering) and run in `spawn_blocking`.
//! - Go strings are bytes here: reference names, directories, paths and config fields are printed raw.
//!   Only error texts are `String`s (lossy for non-UTF-8 arguments, PORTING.md DD-8).

use std::ffi::OsString;
use std::future::Future;
use std::io::Write;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use amber_store_core::key::{Key, Type};
use dstore_client::{Cluster, Progress};
use dstore_gocli::{CliError, Context};
use dstore_gocompat::ctx::Ctx;
use dstore_gocompat::slog::Logger;
use dstore_gocompat::time::GoTime;
use dstore_worktree::{
    Change, Config, Conflict, DiskSource, FetchResult, Kind, PullResult, PushResult, RemoteState,
    Source, Status, Tree, TreeSource,
};

use crate::common::{self, NetOpts, Session};
use crate::progress::run_transfer;

const NO_CLUSTER: &str = "no cluster: set --ticket or $DSTORE_TICKET";
const CLONE_USAGE: &str = "clone NAME [DIR]";
const INIT_USAGE: &str = "init NAME";
const REMOTE_AND_INCOMING: &str = "--remote and --incoming exclude each other";

// ---- helpers (wc.go:50-125) ----

/// `resolveTicket`: the first non-empty of the flag, the stored ticket and the environment.
pub(crate) fn resolve_ticket(flag: &[u8], stored: &[u8], env: &[u8]) -> Result<Vec<u8>, String> {
    [flag, stored, env]
        .into_iter()
        .find(|s| !s.is_empty())
        .map(<[u8]>::to_vec)
        .ok_or_else(|| NO_CLUSTER.to_string())
}

/// `wcConfig`: the stored config (`None` for clone and init), overridden by the flags given on the command
/// line, with `$DSTORE_TICKET` as the last resort for the ticket. `$DSTORE_NO_DISCOVERY` counts only
/// without a stored config, and a value `strconv.ParseBool` rejects is ignored (PORTING.md §1.4).
pub(crate) fn wc_config(c: &Context, stored: Option<&Config>) -> Result<Config, CliError> {
    wc_config_env(c, stored, &|name| std::env::var_os(name))
}

/// `wcConfig` over `getenv`; an unset variable reads as "" (`os.Getenv`).
fn wc_config_env(
    c: &Context,
    stored: Option<&Config>,
    getenv: &dyn Fn(&str) -> Option<OsString>,
) -> Result<Config, CliError> {
    let env = |name: &str| getenv(name).map(OsString::into_vec).unwrap_or_default();
    let mut cfg = stored.cloned().unwrap_or_default();
    cfg.ticket = resolve_ticket(
        c.os_string("ticket").as_bytes(),
        &cfg.ticket,
        &env("DSTORE_TICKET"),
    )
    .map_err(CliError::Msg)?;
    if c.is_set("relay") {
        cfg.relay = c.os_string("relay").into_vec();
    }
    if c.is_set("no-relay") {
        cfg.no_relay = c.bool("no-relay");
    }
    if c.is_set("no-discovery") {
        cfg.no_discovery = c.bool("no-discovery");
    } else if stored.is_none() {
        let v = env("DSTORE_NO_DISCOVERY");
        // A non-UTF-8 value fails ParseBool in Go too.
        if let Some(b) = std::str::from_utf8(&v)
            .ok()
            .and_then(|s| dstore_gocompat::strconv::parse_bool(s).ok())
        {
            cfg.no_discovery = b;
        }
    }
    if c.is_set("user") {
        cfg.user = c.os_string("user").into_vec();
    }
    Ok(cfg)
}

/// `dialConfig`: `ticket.Parse(cfg.Ticket)`, then `dialTicket` with the config's connection options.
async fn dial_config(ctx: &Ctx, cfg: &Config, log: &Logger) -> Result<Session, CliError> {
    let t = dstore_ticket::parse(&cfg.ticket).map_err(CliError::msg)?;
    let n = NetOpts {
        relay: lossy(&cfg.relay),
        no_relay: cfg.no_relay,
        no_discovery: cfg.no_discovery,
    };
    common::dial_ticket(ctx, t, &n, log).await
}

/// `openWC`: the working copy holding the current directory (`os.Getwd`, with its `$PWD` rule). Blocking.
fn open_wc() -> Result<Tree, CliError> {
    let wd = dstore_gocompat::os::getwd().map_err(CliError::msg)?;
    Tree::open(&wd).map_err(CliError::msg)
}

/// `pushUser`: `--user`, else the stored user, else the OS user (a failed lookup leaves ""), checked with
/// `reference.ValidateUser` (`user: <err>`). `--user ""` counts as no flag.
fn push_user(flag: &[u8], stored: &Config) -> Result<String, CliError> {
    push_user_with(flag, stored, || {
        dstore_gocompat::os::current_username().ok()
    })
}

fn push_user_with(
    flag: &[u8],
    stored: &Config,
    os_user: impl FnOnce() -> Option<String>,
) -> Result<String, CliError> {
    let mut u = flag.to_vec();
    if u.is_empty() {
        u.clone_from(&stored.user);
    }
    if u.is_empty()
        && let Some(name) = os_user()
    {
        u = name.into_bytes();
    }
    dstore_client::validate_user_bytes(&u).map_err(|e| CliError::Msg(format!("user: {e}")))?;
    // ValidateUser accepted it, so it is UTF-8.
    String::from_utf8(u).map_err(|_| CliError::Msg("user: user must be valid UTF-8".to_string()))
}

/// `worktree.TicketFromView(cl.View()).Encode()`: the stored ticket is the one derived from the view, also
/// when a short ids ticket was given. `Dial` always learns a view; without one the given ticket is kept
/// (Go would dereference nil there).
fn derived_ticket(cl: &Cluster, given: Vec<u8>) -> Vec<u8> {
    match cl.view() {
        Some(v) => dstore_worktree::ticket_from_view(&v).encode().into_bytes(),
        None => given,
    }
}

/// `c.Int("jobs")` for the worktree: Go's `<= 0` (the number of cores) is 0 here (PORTING.md §5.6).
fn jobs_of(c: &Context) -> usize {
    usize::try_from(c.int("jobs")).unwrap_or(0)
}

/// `k.String()[:16]`.
fn key16(k: &Key) -> String {
    let mut s = k.to_string();
    s.truncate(16);
    s
}

fn lossy(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Go's unbuffered `os.Stdout`: one write, flushed; errors are ignored as `fmt.Printf`'s are.
fn stdout_write(b: &[u8]) {
    let mut o = std::io::stdout().lock();
    let _ = o.write_all(b);
    let _ = o.flush();
}

fn stderr_write(b: &[u8]) {
    let mut e = std::io::stderr().lock();
    let _ = e.write_all(b);
    let _ = e.flush();
}

/// Blocking working-copy work off the async workers (PORTING.md §5.1). A panic is re-raised.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, CliError> + Send + 'static,
) -> Result<T, CliError> {
    match tokio::task::spawn_blocking(f).await {
        Ok(r) => r,
        Err(e) if e.is_panic() => std::panic::resume_unwind(e.into_panic()),
        Err(e) => Err(CliError::Msg(e.to_string())),
    }
}

// ---- output lines (the Printf/Println formats of wc.go) ----

/// Go `k.Type() == key.Commit`, read from the type nibble (the zero key of a result is a Blob key).
fn is_commit(k: &Key) -> bool {
    Type::from_u8(k.0[0] >> 4) == Some(Type::Commit)
}

/// `"root " + key16`: a fetch that found a plain tree.
fn root_desc(root16: &str) -> String {
    format!("root {root16}")
}

/// `fmt.Sprintf("commit %s, root %s", …)`: a fetch that found a branch.
fn commit_desc(commit16: &str, root16: &str) -> String {
    format!("commit {commit16}, root {root16}")
}

/// `fetchedDesc`: names what a fetch found: the tree, or the commit and its tree on a branch.
fn fetched_desc(fr: &FetchResult) -> String {
    if is_commit(&fr.key) {
        commit_desc(&key16(&fr.key), &key16(&fr.tree))
    } else {
        root_desc(&key16(&fr.key))
    }
}

/// `pushedKey`: what the reference names after a push: the commit on a branch, else the tree.
fn pushed_key(r: &PushResult) -> Key {
    if is_commit(&r.commit) {
        r.commit
    } else {
        r.root
    }
}

/// `desc` is [`fetched_desc`]'s text.
fn cloned(name: &[u8], dir: &[u8], desc: &str, fetched: i64, bytes: i64) -> Vec<u8> {
    let tail = format!(": {desc}, {fetched} objects fetched ({bytes} bytes)\n");
    [b"cloned ", name, b" into ", dir, tail.as_bytes()].concat()
}

fn init_exists(name: &[u8], desc: &str) -> Vec<u8> {
    let tail =
        format!("; the reference exists ({desc}): status shows everything as new, pull merges\n");
    [b"initialised working copy of ", name, tail.as_bytes()].concat()
}

fn init_absent(name: &[u8]) -> Vec<u8> {
    [
        b"initialised working copy of ",
        name,
        b"; the reference does not exist yet: push creates it\n",
    ]
    .concat()
}

fn fetch_absent(name: &[u8]) -> Vec<u8> {
    [name, b" does not exist on the cluster\n"].concat()
}

fn fetch_up_to_date(name: &[u8], root16: &str) -> Vec<u8> {
    let tail = format!(": up to date ({root16})\n");
    [name, tail.as_bytes()].concat()
}

fn fetched(name: &[u8], desc: &str, fetched: i64, bytes: i64) -> Vec<u8> {
    let tail = format!(": {desc}, {fetched} objects fetched ({bytes} bytes)\n");
    [b"fetched ", name, tail.as_bytes()].concat()
}

const CONFLICTS_HEADER: &[u8] = b"conflicts:\n";

fn conflict_row(path: &[u8], local: Kind, incoming: Kind) -> Vec<u8> {
    let tail = format!(" (local: {local}, cluster: {incoming})\n");
    [b"  ", path, tail.as_bytes()].concat()
}

/// `if errors.Is(err, worktree.ErrConflict)`: the stderr list of the conflicts the pull kept, only when the
/// command's error is the pull's `ErrConflict`. The pull may have hit it while the command fails with
/// another error: a TUI that cannot run returns its own error (Go `runTUI`), and then nothing is listed.
fn conflict_listing(err: Option<&CliError>, conflicts: Option<Vec<Conflict>>) -> Option<Vec<u8>> {
    let list = conflicts?;
    let is_conflict =
        matches!(err, Some(CliError::Msg(m)) if *m == dstore_worktree::Error::Conflict.to_string());
    if !is_conflict {
        return None;
    }
    let mut b = CONFLICTS_HEADER.to_vec();
    for cf in &list {
        b.extend_from_slice(&conflict_row(&cf.path, cf.local.kind, cf.incoming.kind));
    }
    Some(b)
}

const PULL_UP_TO_DATE: &[u8] = b"already up to date\n";

/// `fmt.Printf("pulled: %d paths updated", …)`.
fn pulled_prefix(applied: usize) -> String {
    format!("pulled: {applied} paths updated")
}

/// `fmt.Printf(", %d conflicts taken from the cluster", …)`.
fn conflicts_taken(conflicts: usize) -> String {
    format!(", {conflicts} conflicts taken from the cluster")
}

/// The `pulled:` line: its two Printf calls, then `fmt.Println()`.
fn pulled(applied: usize, conflicts: usize) -> Vec<u8> {
    let mut s = pulled_prefix(applied);
    if conflicts > 0 {
        s.push_str(&conflicts_taken(conflicts));
    }
    s.push('\n');
    s.into_bytes()
}

/// `push` with nothing to push, and `status` without changes.
const NOTHING_TO_PUSH: &[u8] = b"nothing to push\n";

fn push_recovered(name: &[u8], root16: &str) -> Vec<u8> {
    let tail = format!(" already holds {root16} (an earlier push completed); state updated\n");
    [name, tail.as_bytes()].concat()
}

fn pushed(name: &[u8], root16: &str, keys: i64, uploaded: i64, version: &[u8]) -> Vec<u8> {
    let tail = format!(
        ": root {root16}, {keys} objects, {uploaded} uploaded, version {}\n",
        dstore_gocompat::fmt::hex_lower(version)
    );
    [b"pushed ", name, tail.as_bytes()].concat()
}

/// The `pushed` line of a push that made a commit.
fn pushed_commit(
    name: &[u8],
    commit16: &str,
    root16: &str,
    keys: i64,
    uploaded: i64,
    version: &[u8],
) -> Vec<u8> {
    let tail = format!(
        ": commit {commit16}, root {root16}, {keys} objects, {uploaded} uploaded, version {}\n",
        dstore_gocompat::fmt::hex_lower(version)
    );
    [b"pushed ", name, tail.as_bytes()].concat()
}

fn status_header(name: &[u8], base16: &str) -> Vec<u8> {
    let tail = format!(", synced to {base16}\n");
    [b"reference ", name, tail.as_bytes()].concat()
}

const REMOTE_UP_TO_DATE: &[u8] = b"remote: up to date\n";
const REMOTE_ABSENT: &[u8] = b"remote: the reference does not exist on the cluster\n";

fn remote_moved(added: usize, modified: usize, deleted: usize) -> Vec<u8> {
    format!("remote: moved since your last fetch (+{added} ~{modified} -{deleted}; run pull)\n")
        .into_bytes()
}

const CHANGES: &[u8] = b"changes:\n";

/// `fmt.Printf("  %-9s %s\n", kind, described)`. `%-9s` pads to 9 runes; the kind texts are ASCII.
fn status_row_of(kind: Kind, described: &[u8]) -> Vec<u8> {
    let kind = kind.to_string();
    let head = format!("  {kind:<9} ");
    [head.as_bytes(), described, b"\n"].concat()
}

fn meta_only(n: usize) -> Vec<u8> {
    format!("{n} paths differ only in mtime, ownership or xattrs\n").into_bytes()
}

/// `describeChange`: the path, a trailing slash for a directory (the new entry, or the old one of a
/// deletion), and the old and new type names of a type change or permission bits of a mode change.
pub(crate) fn describe_change(ch: &Change) -> Vec<u8> {
    let (old, new) = (ch.old.as_deref(), ch.new.as_deref());
    let mut p = ch.path.clone();
    if dstore_worktree::is_dir(new) || (new.is_none() && dstore_worktree::is_dir(old)) {
        p.push(b'/');
    }
    // Scan and DiffTrees give type and mode changes both entries (Go dereferences them unchecked).
    match (ch.kind, old, new) {
        (Kind::TypeChanged, Some(o), Some(n)) => {
            let detail = format!(
                " ({} \u{2192} {})",
                dstore_worktree::type_name(o.mode),
                dstore_worktree::type_name(n.mode)
            );
            p.extend_from_slice(detail.as_bytes());
        }
        (Kind::ModeChanged, Some(o), Some(n)) => {
            let detail = format!(
                " ({:04o} \u{2192} {:04o})",
                o.mode & 0o7777,
                n.mode & 0o7777
            );
            p.extend_from_slice(detail.as_bytes());
        }
        _ => {}
    }
    p
}

/// A row of `dstore status`.
fn status_row(ch: &Change) -> Vec<u8> {
    status_row_of(ch.kind, &describe_change(ch))
}

/// The stdout of `dstore status`, line by line (wc.go:367-397).
fn status_lines(name: &[u8], base: &Key, st: &Status) -> Vec<Vec<u8>> {
    let mut lines = vec![status_header(name, &key16(base))];
    lines.push(match st.remote {
        RemoteState::UpToDate => REMOTE_UP_TO_DATE.to_vec(),
        RemoteState::Absent => REMOTE_ABSENT.to_vec(),
        RemoteState::Moved => {
            let (mut a, mut m, mut d) = (0, 0, 0);
            for ch in &st.incoming {
                match ch.kind {
                    Kind::Added => a += 1,
                    Kind::Deleted => d += 1,
                    _ => m += 1,
                }
            }
            remote_moved(a, m, d)
        }
    });
    if !st.changes.is_empty() {
        lines.push(CHANGES.to_vec());
        lines.extend(st.changes.iter().map(status_row));
    }
    if st.meta_only > 0 {
        lines.push(meta_only(st.meta_only));
    }
    if st.changes.is_empty() && st.meta_only == 0 {
        lines.push(NOTHING_TO_PUSH.to_vec());
    }
    lines
}

/// `filterPaths`: the changes at or below the arguments, which are relative to the current directory
/// (`filepath.Abs`, then `filepath.Rel` to the root, lexically; symlinks are not resolved). Every argument
/// is checked before any change is filtered, and the change order is kept.
pub(crate) fn filter_paths(
    root: &[u8],
    changes: Vec<Change>,
    args: &[Vec<u8>],
) -> Result<Vec<Change>, CliError> {
    filter_paths_with(root, changes, args, &dstore_gocompat::path::abs)
}

/// `filterPaths` over `abs` (`filepath.Abs`).
fn filter_paths_with(
    root: &[u8],
    changes: Vec<Change>,
    args: &[Vec<u8>],
    abs: &dyn Fn(&[u8]) -> std::io::Result<Vec<u8>>,
) -> Result<Vec<Change>, CliError> {
    let mut prefixes = Vec::with_capacity(args.len());
    for a in args {
        let abs = abs(a).map_err(CliError::msg)?;
        match dstore_gocompat::path::rel(root, &abs) {
            Some(rel) if rel != b".." && !rel.starts_with(b"../") => prefixes.push(rel),
            _ => {
                return Err(CliError::Msg(format!(
                    "{} is outside the working copy",
                    lossy(a)
                )));
            }
        }
    }
    Ok(changes
        .into_iter()
        .filter(|ch| prefixes.iter().any(|p| at_or_below(&ch.path, p)))
        .collect())
}

/// `p == "." || path == p || strings.HasPrefix(path, p+"/")`.
fn at_or_below(path: &[u8], p: &[u8]) -> bool {
    p == b"."
        || path == p
        || path
            .strip_prefix(p)
            .is_some_and(|rest| rest.first() == Some(&b'/'))
}

// ---- withCluster (wc.go:224-250) ----

/// What `withCluster` runs over the dialled cluster (Go's `fn`).
trait WcOp: Send + 'static {
    type Out: Send + 'static;

    fn run<'a>(
        self,
        ctx: &'a Ctx,
        tree: &'a mut Tree,
        cl: &'a Cluster,
        prog: Progress,
    ) -> impl Future<Output = Result<Self::Out, CliError>> + Send + 'a;
}

struct FetchOp;

impl WcOp for FetchOp {
    type Out = FetchResult;

    async fn run(
        self,
        ctx: &Ctx,
        tree: &mut Tree,
        cl: &Cluster,
        prog: Progress,
    ) -> Result<FetchResult, CliError> {
        tree.fetch(ctx, cl, Some(prog)).await.map_err(CliError::msg)
    }
}

struct PullOp {
    force: bool,
    jobs: usize,
    /// Go's `r.Conflicts`, kept when the pull fails with `ErrConflict` (the CLI prints them then).
    conflicts: Arc<Mutex<Option<Vec<Conflict>>>>,
}

impl WcOp for PullOp {
    type Out = PullResult;

    async fn run(
        self,
        ctx: &Ctx,
        tree: &mut Tree,
        cl: &Cluster,
        prog: Progress,
    ) -> Result<PullResult, CliError> {
        let (r, res) = tree.pull(ctx, cl, self.force, self.jobs, Some(prog)).await;
        match res {
            Ok(()) => Ok(r),
            Err(e) => {
                if e.is_conflict() {
                    *lock(&self.conflicts) = Some(r.conflicts);
                }
                Err(CliError::msg(e))
            }
        }
    }
}

struct PushOp {
    /// `c.String("user")`.
    user: Vec<u8>,
    /// `c.String("message")`.
    message: Vec<u8>,
    force: bool,
    jobs: usize,
}

impl WcOp for PushOp {
    type Out = PushResult;

    async fn run(
        self,
        ctx: &Ctx,
        tree: &mut Tree,
        cl: &Cluster,
        prog: Progress,
    ) -> Result<PushResult, CliError> {
        // The user is checked after dialing (PORTING.md §1.4), against the stored config.
        let user = push_user(&self.user, &tree.config)?;
        tree.push(
            ctx,
            cl,
            &user,
            &self.message,
            self.force,
            self.jobs,
            Some(prog),
        )
        .await
        .map_err(CliError::msg)
    }
}

/// `withCluster`: open the working copy, build the connection config from its stored config and the flags,
/// install the signal context, then under the progress display dial, run `op` and, when it succeeds,
/// refresh the stored ticket. The cluster is closed at the end of the transfer. Returns the reference name,
/// `op`'s result and the open working copy, which the caller closes after printing.
async fn with_cluster<O: WcOp>(
    c: &Context,
    title: &str,
    op: O,
) -> Result<(Vec<u8>, O::Out, Tree), CliError> {
    let tree = blocking(open_wc).await?;
    let cfg = match wc_config(c, Some(&tree.config)) {
        Ok(cfg) => cfg,
        Err(e) => {
            let _ = tree.close();
            return Err(e);
        }
    };
    let ctx = common::signal_ctx();
    let name = tree.config.name.clone();
    let title = format!("{title} {}", lossy(&name));
    let (out, tree) = run_transfer(c, &ctx, title, move |ctx, log, prog| {
        Box::pin(async move {
            let mut tree = tree;
            let res: Result<O::Out, CliError> = async {
                let session = dial_config(&ctx, &cfg, &log).await?;
                let res: Result<O::Out, CliError> = async {
                    let out = op.run(&ctx, &mut tree, &session.cluster, prog).await?;
                    tree.refresh_ticket(&session.cluster)
                        .map_err(CliError::msg)?;
                    Ok(out)
                }
                .await;
                session.close().await;
                res
            }
            .await;
            match res {
                Ok(out) => Ok((out, tree)),
                Err(e) => {
                    let _ = tree.close();
                    Err(e)
                }
            }
        })
    })
    .await?;
    Ok((name, out, tree))
}

// ---- actions ----

/// `clone NAME [DIR]`.
pub(crate) async fn clone(c: &Context) -> Result<(), CliError> {
    let name = c.first().as_bytes().to_vec();
    if name.is_empty() {
        return Err(CliError::Msg(CLONE_USAGE.to_string()));
    }
    dstore_client::validate_name_bytes(&name).map_err(CliError::Msg)?;
    let mut dir = c.arg(1).as_bytes().to_vec();
    if dir.is_empty() {
        dir = dstore_gocompat::path::base(&name);
    }
    let mut cfg = wc_config(c, None)?;
    cfg.name.clone_from(&name);
    let ctx = common::signal_ctx();
    let target = dir.clone();
    let (tree, fr) = run_transfer(
        c,
        &ctx,
        format!("clone {}", lossy(&name)),
        move |ctx, log, prog| {
            Box::pin(async move {
                let session = dial_config(&ctx, &cfg, &log).await?;
                let mut cfg = cfg;
                cfg.ticket = derived_ticket(&session.cluster, cfg.ticket);
                let res = dstore_worktree::clone(&ctx, &session.cluster, &target, cfg, Some(prog))
                    .await
                    .map_err(CliError::msg);
                session.close().await;
                res
            })
        },
    )
    .await?;
    stdout_write(&cloned(
        &name,
        &dir,
        &fetched_desc(&fr),
        fr.stats.fetched,
        fr.stats.bytes,
    ));
    let _ = tree.close();
    Ok(())
}

/// `init NAME`.
pub(crate) async fn init(c: &Context) -> Result<(), CliError> {
    let name = c.first().as_bytes().to_vec();
    if name.is_empty() {
        return Err(CliError::Msg(INIT_USAGE.to_string()));
    }
    dstore_client::validate_name_bytes(&name).map_err(CliError::Msg)?;
    let mut cfg = wc_config(c, None)?;
    cfg.name.clone_from(&name);
    let wd = dstore_gocompat::os::getwd().map_err(CliError::msg)?;
    let ctx = common::signal_ctx();
    let (tree, fr) = run_transfer(
        c,
        &ctx,
        format!("init {}", lossy(&name)),
        move |ctx, log, prog| {
            Box::pin(async move {
                let session = dial_config(&ctx, &cfg, &log).await?;
                let mut cfg = cfg;
                cfg.ticket = derived_ticket(&session.cluster, cfg.ticket);
                let res = dstore_worktree::init(&ctx, &session.cluster, &wd, cfg, Some(prog))
                    .await
                    .map_err(CliError::msg);
                session.close().await;
                res
            })
        },
    )
    .await?;
    let line = if fr.exists {
        init_exists(&name, &fetched_desc(&fr))
    } else {
        init_absent(&name)
    };
    stdout_write(&line);
    let _ = tree.close();
    Ok(())
}

/// `fetch`.
pub(crate) async fn fetch(c: &Context) -> Result<(), CliError> {
    let (name, fr, tree) = with_cluster(c, "fetch", FetchOp).await?;
    let line = if !fr.exists {
        fetch_absent(&name)
    } else if fr.up_to_date {
        fetch_up_to_date(&name, &key16(&fr.key))
    } else {
        fetched(&name, &fetched_desc(&fr), fr.stats.fetched, fr.stats.bytes)
    };
    stdout_write(&line);
    let _ = tree.close();
    Ok(())
}

/// `pull [--force]`.
pub(crate) async fn pull(c: &Context) -> Result<(), CliError> {
    let conflicts = Arc::new(Mutex::new(None));
    let op = PullOp {
        force: c.bool("force"),
        jobs: jobs_of(c),
        conflicts: Arc::clone(&conflicts),
    };
    let res = with_cluster(c, "pull", op).await;
    // errors.Is(err, worktree.ErrConflict): the conflicts go to stderr before the error.
    let taken = lock(&conflicts).take();
    if let Some(b) = conflict_listing(res.as_ref().err(), taken) {
        stderr_write(&b);
    }
    let (_, r, tree) = res?;
    let line = if r.up_to_date {
        PULL_UP_TO_DATE.to_vec()
    } else {
        pulled(r.applied.len(), r.conflicts.len())
    };
    stdout_write(&line);
    let _ = tree.close();
    Ok(())
}

/// `push [--force] [--user U] [-m MSG]`.
pub(crate) async fn push(c: &Context) -> Result<(), CliError> {
    let op = PushOp {
        user: c.os_string("user").into_vec(),
        message: c.os_string("message").into_vec(),
        force: c.bool("force"),
        jobs: jobs_of(c),
    };
    let (name, r, tree) = with_cluster(c, "push", op).await?;
    let line = if r.nothing {
        NOTHING_TO_PUSH.to_vec()
    } else if r.recovered {
        push_recovered(&name, &key16(&pushed_key(&r)))
    } else if is_commit(&r.commit) {
        pushed_commit(
            &name,
            &key16(&r.commit),
            &key16(&r.root),
            r.stats.keys,
            r.stats.uploaded,
            &r.stats.version,
        )
    } else {
        pushed(
            &name,
            &key16(&r.root),
            r.stats.keys,
            r.stats.uploaded,
            &r.stats.version,
        )
    };
    stdout_write(&line);
    let _ = tree.close();
    Ok(())
}

/// `status`: offline, no signal handler.
pub(crate) async fn status(c: &Context) -> Result<(), CliError> {
    let jobs = jobs_of(c);
    blocking(move || {
        let tree = open_wc()?;
        let res = tree.status(jobs).map_err(CliError::msg).map(|st| {
            for line in status_lines(&tree.config.name, &tree.state.base, &st) {
                stdout_write(&line);
            }
        });
        let _ = tree.close();
        res
    })
    .await
}

/// What `diff` compares.
#[derive(Clone, Copy)]
enum Against {
    /// The last synced tree against the working directory.
    Base,
    /// `--remote`: the fetched tree against the working directory.
    Remote,
    /// `--incoming`: the last synced tree against the fetched one.
    Incoming,
}

/// `diff [--remote|--incoming] [--stat] [PATH...]`: offline, no signal handler.
pub(crate) async fn diff(c: &Context) -> Result<(), CliError> {
    let (remote, incoming) = (c.bool("remote"), c.bool("incoming"));
    // Before the working copy is opened (PORTING.md §1.4).
    if remote && incoming {
        return Err(CliError::Msg(REMOTE_AND_INCOMING.to_string()));
    }
    let against = if incoming {
        Against::Incoming
    } else if remote {
        Against::Remote
    } else {
        Against::Base
    };
    let stat = c.bool("stat");
    let jobs = jobs_of(c);
    let args: Vec<Vec<u8>> = c.args().iter().map(|a| a.as_bytes().to_vec()).collect();
    blocking(move || {
        let tree = open_wc()?;
        let res = diff_tree(&tree, against, stat, jobs, &args);
        let _ = tree.close();
        res
    })
    .await
}

/// The body of `diff` after `openWC`. Output streams to stdout change by change, so partial output can
/// precede an error, as in Go.
fn diff_tree(
    tree: &Tree,
    against: Against,
    stat: bool,
    jobs: usize,
    args: &[Vec<u8>],
) -> Result<(), CliError> {
    let get = |k: Key| tree.get(k);
    let tree_src = TreeSource { get: &get };
    let disk = DiskSource {
        root: tree.root.clone(),
    };
    let st = &tree.state;
    let no_remote = || CliError::msg(dstore_worktree::Error::NoRemote);
    let (changes, new): (Vec<Change>, &dyn Source) = match against {
        Against::Incoming => {
            if !st.has_remote {
                return Err(no_remote());
            }
            let changes =
                dstore_worktree::diff_trees(&get, st.base, st.remote).map_err(CliError::msg)?;
            (changes, &tree_src)
        }
        Against::Remote => {
            if !st.has_remote {
                return Err(no_remote());
            }
            let changes = dstore_worktree::scan(&tree.root, st.remote, &get, GoTime::now(), jobs)
                .map_err(CliError::msg)?;
            (changes, &disk)
        }
        Against::Base => {
            let changes = dstore_worktree::scan(&tree.root, st.base, &get, st.synced_at, jobs)
                .map_err(CliError::msg)?;
            (changes, &disk)
        }
    };
    let changes = if args.is_empty() {
        changes
    } else {
        filter_paths(&tree.root, changes, args)?
    };
    let mut out = std::io::stdout().lock();
    let res = if stat {
        dstore_worktree::stat(&mut out, &changes, &tree_src, new)
    } else {
        dstore_worktree::unified(&mut out, &changes, &tree_src, new)
    };
    let _ = out.flush();
    res.map_err(CliError::msg)
}

#[cfg(test)]
mod tests {
    //! Vectors: `worktree/cli.json` (`describe_change`, `status_row`, `resolve_ticket`, `filter_paths`,
    //! `printf`), `cli/text.json` (`describe_change`, `resolve_ticket`, `filter_paths`) and the `cmd/` texts
    //! of `errors/worktree_text.json`. A missing vector file fails the test.

    use amber_store_core::fstree::Entry;

    use super::json::{self, J};
    use super::*;

    fn vectors(rel: &str) -> J {
        let p = format!("{}/../../tests/golden/{rel}", env!("CARGO_MANIFEST_DIR"));
        let text = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("vector file {p}: {e}"));
        json::parse(&text)
    }

    fn unhex(s: &str) -> Vec<u8> {
        assert!(s.len().is_multiple_of(2), "odd hex {s:?}");
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
            .collect()
    }

    /// A text field in its `x` or `x_hex` form.
    fn bytes_field(j: &J, name: &str) -> Vec<u8> {
        match j.get(name) {
            Some(v) => v.str().as_bytes().to_vec(),
            None => unhex(j.at(&format!("{name}_hex")).str()),
        }
    }

    fn kind_named(s: &str) -> Kind {
        match s {
            "new" => Kind::Added,
            "deleted" => Kind::Deleted,
            "modified" => Kind::Modified,
            "type" => Kind::TypeChanged,
            "mode" => Kind::ModeChanged,
            "meta" => Kind::MetaChanged,
            other => panic!("kind {other:?}"),
        }
    }

    /// Go's `worktree.Kind` numbers; `None` for values without a Rust variant.
    fn kind_numbered(n: i64) -> Option<Kind> {
        Some(match n {
            0 => Kind::Added,
            1 => Kind::Deleted,
            2 => Kind::Modified,
            3 => Kind::TypeChanged,
            4 => Kind::ModeChanged,
            5 => Kind::MetaChanged,
            _ => return None,
        })
    }

    fn entry_of_mode(mode: u64) -> Arc<Entry> {
        Arc::new(Entry {
            mode,
            ..Default::default()
        })
    }

    /// A VECTORS.md `Change`; only the entries' modes matter here.
    fn change_of(j: &J) -> Change {
        let entry = |side: &J| -> Option<Arc<Entry>> {
            if side.is_null() {
                None
            } else {
                Some(entry_of_mode(side.at("mode").str().parse().expect("mode")))
            }
        };
        Change {
            path: bytes_field(j, "path"),
            kind: kind_named(j.at("kind").str()),
            old: entry(j.at("old")),
            new: entry(j.at("new")),
        }
    }

    fn added(path: &[u8]) -> Change {
        Change {
            path: path.to_vec(),
            kind: Kind::Added,
            old: None,
            new: Some(entry_of_mode(0o100644)),
        }
    }

    fn msg(e: CliError) -> String {
        match e {
            CliError::Msg(m) => m,
            CliError::Exit { msg, code } => panic!("exit {code}: {msg}"),
        }
    }

    #[test]
    fn describe_change_and_status_row_vectors() {
        let v = vectors("worktree/cli.json");
        let cases = v.at("describe_change").arr();
        assert!(cases.len() >= 20);
        for c in cases {
            let ch = change_of(c.at("change"));
            assert_eq!(
                describe_change(&ch),
                bytes_field(c, "out"),
                "{}",
                c.at("name").str()
            );
        }
        let rows = v.at("status_row").arr();
        assert_eq!(rows.len(), cases.len());
        for c in rows {
            let ch = change_of(c.at("change"));
            assert_eq!(
                status_row(&ch),
                bytes_field(c, "out"),
                "{}",
                c.at("name").str()
            );
        }
    }

    #[test]
    fn describe_change_text_vectors() {
        let v = vectors("cli/text.json");
        let (mut checked, mut skipped) = (0, 0);
        for c in v.at("describe_change").arr() {
            // Kind 9 has no Rust variant (VECTORS.md); no other case may be skipped.
            let Some(kind) = kind_numbered(c.at("kind").num()) else {
                assert_eq!(c.at("kind").num(), 9, "{c:?}");
                skipped += 1;
                continue;
            };
            assert_eq!(kind.to_string(), c.at("kind_string").str());
            let side = |name: &str| match c.at(name) {
                J::Null => None,
                m => Some(entry_of_mode(u64::try_from(m.num()).expect("mode"))),
            };
            let ch = Change {
                path: c.at("path").str().as_bytes().to_vec(),
                kind,
                old: side("old_mode"),
                new: side("new_mode"),
            };
            let what = c.at("path").str();
            assert_eq!(
                describe_change(&ch),
                c.at("describe").str().as_bytes(),
                "{what}"
            );
            let line = format!("{}\n", c.at("line").str());
            assert_eq!(status_row(&ch), line.as_bytes(), "{what}");
            checked += 1;
        }
        assert!(
            checked >= 18 && skipped == 1,
            "{checked} checked, {skipped} skipped"
        );
    }

    #[test]
    fn resolve_ticket_vectors() {
        for file in ["worktree/cli.json", "cli/text.json"] {
            let v = vectors(file);
            let cases = v.at("resolve_ticket").arr();
            assert!(cases.len() >= 6, "{file}");
            for c in cases {
                let got = resolve_ticket(
                    c.at("flag").str().as_bytes(),
                    c.at("stored").str().as_bytes(),
                    c.at("env").str().as_bytes(),
                );
                if c.at("ok").bool() {
                    assert_eq!(
                        got,
                        Ok(c.at("out").str().as_bytes().to_vec()),
                        "{file} {c:?}"
                    );
                } else {
                    assert_eq!(got, Err(c.at("error").str().to_string()), "{file} {c:?}");
                }
            }
        }
    }

    /// Go `filepath.Abs` with the working directory `wd`.
    fn abs_in(wd: &[u8]) -> impl Fn(&[u8]) -> std::io::Result<Vec<u8>> + '_ {
        move |p: &[u8]| {
            Ok(if p.first() == Some(&b'/') {
                dstore_gocompat::path::clean(p)
            } else {
                dstore_gocompat::path::join(&[wd, p])
            })
        }
    }

    fn run_filter(
        root: &[u8],
        wd: &[u8],
        changes: &[String],
        args: &[String],
    ) -> Result<Vec<String>, String> {
        let changes: Vec<Change> = changes.iter().map(|p| added(p.as_bytes())).collect();
        let args: Vec<Vec<u8>> = args.iter().map(|a| a.as_bytes().to_vec()).collect();
        filter_paths_with(root, changes, &args, &abs_in(wd))
            .map(|kept| kept.iter().map(|c| lossy(&c.path)).collect())
            .map_err(msg)
    }

    fn strings(j: &J, root: &str) -> Vec<String> {
        j.arr()
            .iter()
            .map(|s| s.str().replace("{ROOT}", root))
            .collect()
    }

    #[test]
    fn filter_paths_vectors() {
        let root = "/wc/root";
        let v = vectors("worktree/cli.json");
        let cases = v.at("filter_paths").arr();
        assert!(cases.len() >= 18);
        for c in cases {
            let name = c.at("name").str();
            let wd = dstore_gocompat::path::join(&[root.as_bytes(), c.at("cwd").str().as_bytes()]);
            let got = run_filter(
                root.as_bytes(),
                &wd,
                &strings(c.at("changes"), root),
                &strings(c.at("args"), root),
            );
            let want = match c.get("error") {
                Some(e) => Err(e.str().replace("{ROOT}", root)),
                None if c.at("kept").is_null() => Ok(Vec::new()),
                None => Ok(strings(c.at("kept"), root)),
            };
            assert_eq!(got, want, "{name}");
        }
    }

    #[test]
    fn filter_paths_text_vectors() {
        let root = "/wc/root";
        let v = vectors("cli/text.json");
        let cases = v.at("filter_paths").arr();
        assert!(cases.len() >= 20);
        for c in cases {
            let got = run_filter(
                root.as_bytes(),
                root.as_bytes(),
                &strings(c.at("changes"), root),
                &strings(c.at("args"), root),
            );
            let want = if c.at("ok").bool() {
                Ok(strings(c.at("out"), root))
            } else {
                Err(c.at("error").str().replace("{ROOT}", root))
            };
            assert_eq!(got, want, "{c:?}");
        }
    }

    /// A symlinked working-copy root stays as given: Rel is lexical.
    #[test]
    fn filter_paths_is_lexical() {
        let changes = vec![added(b"a.txt"), added(b"sub/b.txt")];
        let args = vec![b"/real/wc/a.txt".to_vec()];
        let e = filter_paths_with(b"/link/wc", changes, &args, &abs_in(b"/link/wc"))
            .map(|_| ())
            .map_err(msg);
        assert_eq!(
            e,
            Err("/real/wc/a.txt is outside the working copy".to_string())
        );
        let err = filter_paths_with(
            b"/wc",
            vec![added(b"a")],
            &[b"x".to_vec()],
            &|_: &[u8]| Err(std::io::Error::other("getwd: no such file or directory")),
        )
        .map(|_| ())
        .map_err(msg);
        assert_eq!(err, Err("getwd: no such file or directory".to_string()));
    }

    /// `fetchedDesc` and `pushedKey` over Go's results.
    #[test]
    fn fetched_desc_and_pushed_key_vectors() {
        let v = vectors("worktree/cli.json");
        let key = |h: &str| Key(unhex(h).try_into().expect("a 32-byte key"));
        let cases = v.at("fetched_desc").arr();
        assert!(cases.len() >= 4);
        for c in cases {
            let fr = FetchResult {
                key: key(c.at("key").str()),
                tree: key(c.at("tree").str()),
                ..FetchResult::default()
            };
            assert_eq!(
                fetched_desc(&fr),
                c.at("out").str(),
                "{}",
                c.at("name").str()
            );
        }
        let cases = v.at("pushed_key").arr();
        assert!(cases.len() >= 3);
        for c in cases {
            let r = PushResult {
                root: key(c.at("root").str()),
                commit: key(c.at("commit").str()),
                ..PushResult::default()
            };
            assert_eq!(
                pushed_key(&r),
                key(c.at("out").str()),
                "{}",
                c.at("name").str()
            );
        }
    }

    /// Every stdout/stderr format of wc.go with the vectors' fixed arguments.
    #[test]
    fn printf_vectors() {
        let v = vectors("worktree/cli.json");
        let cases = v.at("printf").arr();
        assert!(cases.len() >= 34);
        for c in cases {
            let name = c.at("name").str();
            let args = c.at("args").arr();
            let s = |i: usize| args[i].at("value").str().as_bytes().to_vec();
            let text = |i: usize| args[i].at("value").str().to_string();
            let int = |i: usize| -> i64 { args[i].at("value").str().parse().expect("int") };
            let count = |i: usize| usize::try_from(int(i)).expect("count");
            let kind = |i: usize| kind_numbered(int(i)).expect("kind");
            let got: Vec<u8> = match c.at("format").str() {
                "cloned %s into %s: %s, %d objects fetched (%d bytes)\n" => {
                    cloned(&s(0), &s(1), &text(2), int(3), int(4))
                }
                "initialised working copy of %s; the reference exists (%s): status shows everything as new, pull merges\n" => {
                    init_exists(&s(0), &text(1))
                }
                "initialised working copy of %s; the reference does not exist yet: push creates it\n" => {
                    init_absent(&s(0))
                }
                "%s does not exist on the cluster\n" => fetch_absent(&s(0)),
                "%s: up to date (%s)\n" => fetch_up_to_date(&s(0), &text(1)),
                "fetched %s: %s, %d objects fetched (%d bytes)\n" => {
                    fetched(&s(0), &text(1), int(2), int(3))
                }
                "conflicts:" => CONFLICTS_HEADER.to_vec(),
                "  %s (local: %s, cluster: %s)\n" => conflict_row(&s(0), kind(1), kind(2)),
                "already up to date" => PULL_UP_TO_DATE.to_vec(),
                "pulled: %d paths updated" => pulled_prefix(count(0)).into_bytes(),
                ", %d conflicts taken from the cluster" => conflicts_taken(count(0)).into_bytes(),
                // The bare fmt.Println() that ends the pulled: line.
                "" => b"\n".to_vec(),
                "nothing to push" => NOTHING_TO_PUSH.to_vec(),
                "%s already holds %s (an earlier push completed); state updated\n" => {
                    push_recovered(&s(0), &text(1))
                }
                "commit %s, root %s" => commit_desc(&text(0), &text(1)).into_bytes(),
                "pushed %s: commit %s, root %s, %d objects, %d uploaded, version %x\n" => {
                    pushed_commit(
                        &s(0),
                        &text(1),
                        &text(2),
                        int(3),
                        int(4),
                        &unhex(args[5].at("value").str()),
                    )
                }
                "pushed %s: root %s, %d objects, %d uploaded, version %x\n" => pushed(
                    &s(0),
                    &text(1),
                    int(2),
                    int(3),
                    &unhex(args[4].at("value").str()),
                ),
                "reference %s, synced to %s\n" => status_header(&s(0), &text(1)),
                "remote: up to date" => REMOTE_UP_TO_DATE.to_vec(),
                "remote: the reference does not exist on the cluster" => REMOTE_ABSENT.to_vec(),
                "remote: moved since your last fetch (+%d ~%d -%d; run pull)\n" => {
                    remote_moved(count(0), count(1), count(2))
                }
                "changes:" => CHANGES.to_vec(),
                "  %-9s %s\n" => status_row_of(kind(0), &s(1)),
                "%d paths differ only in mtime, ownership or xattrs\n" => meta_only(count(0)),
                other => panic!("{name}: format {other:?} has no Rust counterpart"),
            };
            assert_eq!(lossy(&got), c.at("out").str(), "{name}");
        }
        // The composed pulled: line.
        assert_eq!(pulled(3, 0), b"pulled: 3 paths updated\n");
        assert_eq!(
            pulled(3, 2),
            b"pulled: 3 paths updated, 2 conflicts taken from the cluster\n"
        );
    }

    /// The `cmd/` texts of errors/worktree_text.json (wc.go usage errors, `resolveTicket`, `pushUser`, and
    /// the `reference.ValidateName` texts clone and init return).
    #[test]
    fn cmd_error_texts() {
        let v = vectors("errors/worktree_text.json");
        let cfg = Config::default();
        let user = |flag: &[u8]| push_user_with(flag, &cfg, || None).map_err(msg);
        let name = |n: &[u8]| dstore_client::validate_name_bytes(n);
        let long = vec![b'a'; 1025];
        let mut checked = 0;
        for c in v.at("cases").arr() {
            let case = c.at("name").str();
            let Some(which) = case.strip_prefix("cmd/") else {
                continue;
            };
            let got: String = match which {
                "clone-usage" => CLONE_USAGE.to_string(),
                "init-usage" => INIT_USAGE.to_string(),
                "remote-and-incoming" => REMOTE_AND_INCOMING.to_string(),
                "resolve-ticket" => resolve_ticket(b"", b"", b"").expect_err("no ticket"),
                "push-user-empty" => user(b"").expect_err("empty"),
                "push-user-invalid-utf8" => user(b"\xff").expect_err("utf-8"),
                "push-user-control" => user(b"a\x01b").expect_err("control"),
                "push-user-too-long" => user(&long).expect_err("long"),
                "reference-name-empty" => name(b"").expect_err("empty"),
                "reference-name-too-long" => name(&long).expect_err("long"),
                "reference-name-invalid-utf8" => name(b"\xff").expect_err("utf-8"),
                "reference-name-at" => name(b"a@b").expect_err("at"),
                "reference-name-control" => name(b"a\x01b").expect_err("control"),
                other => panic!("unknown case cmd/{other}"),
            };
            assert_eq!(got, c.at("out").str(), "{case}");
            checked += 1;
        }
        assert_eq!(checked, 13);
    }

    /// `pushUser`: the flag, then the stored user, then the OS user; `--user ""` is no flag.
    #[test]
    fn push_user_precedence() {
        let stored = Config {
            user: b"stored".to_vec(),
            ..Default::default()
        };
        let none = Config::default();
        let os = || Some("osuser".to_string());
        assert_eq!(
            push_user_with(b"flag", &stored, os).map_err(msg),
            Ok("flag".into())
        );
        assert_eq!(
            push_user_with(b"", &stored, os).map_err(msg),
            Ok("stored".into())
        );
        assert_eq!(
            push_user_with(b"", &none, os).map_err(msg),
            Ok("osuser".into())
        );
        assert_eq!(
            push_user_with(b"", &none, || None).map_err(msg),
            Err("user: user must not be empty".into())
        );
        // A bad stored user is not replaced by the OS user.
        let bad = Config {
            user: b"a\x7fb".to_vec(),
            ..Default::default()
        };
        assert_eq!(
            push_user_with(b"", &bad, os).map_err(msg),
            Err("user: user must not contain control characters".into())
        );
    }

    /// The context `dstore_gocli::dispatch` hands the action of `args` (after the program name), with
    /// `env` as the environment.
    fn ctx_of(args: &[&[u8]], env: &[(&str, &[u8])]) -> Context {
        let argv: Vec<OsString> = std::iter::once(&b"dstore"[..])
            .chain(args.iter().copied())
            .map(|a| OsString::from_vec(a.to_vec()))
            .collect();
        let getenv = |n: &str| {
            env.iter()
                .find(|(k, _)| *k == n)
                .map(|(_, v)| OsString::from_vec(v.to_vec()))
        };
        let mut out = Vec::new();
        match dstore_gocli::dispatch(&crate::app(), argv, &mut out, &getenv) {
            Ok(Some((_, c))) => c,
            other => panic!(
                "{args:?}: no action reached ({:?}), stdout {:?}",
                other.map(|f| f.is_some()),
                String::from_utf8_lossy(&out)
            ),
        }
    }

    fn config_of(
        args: &[&[u8]],
        env: &[(&str, &[u8])],
        stored: Option<&Config>,
    ) -> Result<Config, String> {
        let c = ctx_of(args, env);
        let getenv = |n: &str| {
            env.iter()
                .find(|(k, _)| *k == n)
                .map(|(_, v)| OsString::from_vec(v.to_vec()))
        };
        wc_config_env(&c, stored, &getenv).map_err(msg)
    }

    #[test]
    fn wc_config_without_a_stored_config() {
        let clone: &[&[u8]] = &[b"clone", b"trees/x"];
        assert_eq!(config_of(clone, &[], None), Err(NO_CLUSTER.to_string()));
        let cfg = config_of(clone, &[("DSTORE_TICKET", b"e")], None).expect("config");
        assert_eq!(
            cfg,
            Config {
                ticket: b"e".to_vec(),
                ..Default::default()
            }
        );
        // $DSTORE_NO_DISCOVERY: strconv.ParseBool, anything else is ignored.
        let cases: [(&[u8], bool); 9] = [
            (b"true", true),
            (b"1", true),
            (b"T", true),
            (b"TRUE", true),
            (b"false", false),
            (b"maybe", false),
            (b"", false),
            (b"yes", false),
            (b"\xff", false),
        ];
        for (value, want) in cases {
            let env: &[(&str, &[u8])] = &[("DSTORE_TICKET", b"e"), ("DSTORE_NO_DISCOVERY", value)];
            let cfg = config_of(clone, env, None).expect("config");
            assert_eq!(cfg.no_discovery, want, "{value:?}");
        }
        // The flag wins over the environment.
        let env: &[(&str, &[u8])] = &[("DSTORE_TICKET", b"e"), ("DSTORE_NO_DISCOVERY", b"true")];
        let cfg =
            config_of(&[b"clone", b"--no-discovery=false", b"trees/x"], env, None).expect("cfg");
        assert!(!cfg.no_discovery);
        // init takes the same path (no --user given, so the user stays empty).
        let cfg = config_of(&[b"init", b"--relay", b"r", b"trees/x"], env, None).expect("cfg");
        assert_eq!(
            cfg,
            Config {
                ticket: b"e".to_vec(),
                relay: b"r".to_vec(),
                no_discovery: true,
                ..Default::default()
            }
        );
        // --ticket beats the environment; --user is stored.
        let cfg = config_of(
            &[
                b"clone",
                b"--ticket",
                b"f",
                b"--user",
                b"u",
                b"--no-relay",
                b"trees/x",
            ],
            env,
            None,
        )
        .expect("cfg");
        assert_eq!(
            cfg,
            Config {
                ticket: b"f".to_vec(),
                no_relay: true,
                no_discovery: true,
                user: b"u".to_vec(),
                ..Default::default()
            }
        );
    }

    #[test]
    fn wc_config_with_a_stored_config() {
        let stored = Config {
            ticket: b"s".to_vec(),
            name: b"trees/demo".to_vec(),
            relay: b"r0".to_vec(),
            no_relay: true,
            no_discovery: false,
            user: b"u0".to_vec(),
        };
        let env: &[(&str, &[u8])] = &[("DSTORE_TICKET", b"e"), ("DSTORE_NO_DISCOVERY", b"true")];
        // The stored ticket beats $DSTORE_TICKET; $DSTORE_NO_DISCOVERY is ignored.
        let cmds: [&[u8]; 3] = [b"fetch", b"pull", b"push"];
        for cmd in cmds {
            let cfg = config_of(&[cmd], env, Some(&stored)).expect("cfg");
            assert_eq!(cfg, stored, "{cmd:?}");
        }
        // An explicit empty --ticket falls through to the stored one.
        let cfg = config_of(&[b"fetch", b"--ticket="], env, Some(&stored)).expect("cfg");
        assert_eq!(cfg.ticket, b"s");
        // Flags given on the command line override for this run, also to "" and false.
        let cfg = config_of(
            &[
                b"pull",
                b"--ticket",
                b"f",
                b"--relay",
                b"",
                b"--no-relay=false",
                b"--no-discovery",
            ],
            env,
            Some(&stored),
        )
        .expect("cfg");
        assert_eq!(
            cfg,
            Config {
                ticket: b"f".to_vec(),
                relay: Vec::new(),
                no_relay: false,
                no_discovery: true,
                ..stored.clone()
            }
        );
        let cfg = config_of(&[b"push", b"--user", b""], env, Some(&stored)).expect("cfg");
        assert_eq!(cfg.user, b"");
        let cfg = config_of(&[b"push", b"--user", b"u1"], env, Some(&stored)).expect("cfg");
        assert_eq!(cfg.user, b"u1");
        // Without a ticket anywhere.
        let bare = Config::default();
        assert_eq!(
            config_of(&[b"fetch"], &[], Some(&bare)),
            Err(NO_CLUSTER.to_string())
        );
    }

    #[test]
    fn jobs_flag() {
        assert_eq!(jobs_of(&ctx_of(&[b"status"], &[])), 0);
        assert_eq!(jobs_of(&ctx_of(&[b"status", b"--jobs", b"5"], &[])), 5);
        assert_eq!(jobs_of(&ctx_of(&[b"diff", b"--jobs=-3"], &[])), 0);
        assert_eq!(jobs_of(&ctx_of(&[b"pull", b"--jobs", b"2"], &[])), 2);
    }

    fn change(path: &str, kind: Kind, old: Option<u64>, new: Option<u64>) -> Change {
        Change {
            path: path.as_bytes().to_vec(),
            kind,
            old: old.map(entry_of_mode),
            new: new.map(entry_of_mode),
        }
    }

    fn joined(lines: Vec<Vec<u8>>) -> String {
        lossy(&lines.concat())
    }

    #[test]
    fn status_output() {
        let base = Key([0x20; 32]);
        let file = 0o100644;
        let dir = 0o40755;
        // The wc1 snapshot: changes, one metadata-only path, no remote.
        let st = Status {
            changes: vec![
                change("a.txt", Kind::Modified, Some(file), Some(file)),
                change("link", Kind::TypeChanged, Some(0o120777), Some(dir)),
                change("link/inner.txt", Kind::Added, None, Some(file)),
                change("run.sh", Kind::ModeChanged, Some(file), Some(0o100755)),
                change("sub/b.txt", Kind::Deleted, Some(file), None),
            ],
            meta_only: 1,
            remote: RemoteState::Absent,
            incoming: Vec::new(),
        };
        assert_eq!(
            joined(status_lines(b"trees/demo", &base, &st)),
            "reference trees/demo, synced to 2020202020202020\n\
             remote: the reference does not exist on the cluster\n\
             changes:\n  modified  a.txt\n  type      link/ (symlink \u{2192} directory)\n  \
             new       link/inner.txt\n  mode      run.sh (0644 \u{2192} 0755)\n  deleted   sub/b.txt\n\
             1 paths differ only in mtime, ownership or xattrs\n"
        );
        // Moved: Added and Deleted counted apart, every other kind (MetaChanged too) as modified; "nothing
        // to push" even though the remote moved.
        let st = Status {
            changes: Vec::new(),
            meta_only: 0,
            remote: RemoteState::Moved,
            incoming: vec![
                change("a", Kind::Added, None, Some(file)),
                change("b", Kind::Deleted, Some(file), None),
                change("c", Kind::Modified, Some(file), Some(file)),
                change("d", Kind::MetaChanged, Some(file), Some(file)),
                change("e", Kind::TypeChanged, Some(file), Some(dir)),
                change("f", Kind::Added, None, Some(dir)),
            ],
        };
        assert_eq!(
            joined(status_lines(b"trees/demo2", &base, &st)),
            "reference trees/demo2, synced to 2020202020202020\n\
             remote: moved since your last fetch (+2 ~3 -1; run pull)\n\
             nothing to push\n"
        );
        let st = Status {
            changes: Vec::new(),
            meta_only: 2,
            remote: RemoteState::UpToDate,
            incoming: Vec::new(),
        };
        assert_eq!(
            joined(status_lines(b"n", &base, &st)),
            "reference n, synced to 2020202020202020\nremote: up to date\n\
             2 paths differ only in mtime, ownership or xattrs\n"
        );
        // Raw bytes of a non-UTF-8 name and path.
        let st = Status {
            changes: vec![change("x", Kind::Added, None, Some(file))],
            meta_only: 0,
            remote: RemoteState::UpToDate,
            incoming: Vec::new(),
        };
        let mut raw = st;
        raw.changes[0].path = b"caf\xe9".to_vec();
        assert_eq!(
            status_lines(b"n\xff", &base, &raw).concat(),
            b"reference n\xff, synced to 2020202020202020\nremote: up to date\nchanges:\n  new       caf\xe9\n"
        );
    }

    /// Go dereferences both entries of type and mode changes; Scan and DiffTrees always give them. Without
    /// them only the path is printed.
    #[test]
    fn describe_change_without_entries() {
        let ch = change("x", Kind::TypeChanged, None, None);
        assert_eq!(describe_change(&ch), b"x");
        let ch = change("d", Kind::ModeChanged, Some(0o40755), None);
        assert_eq!(describe_change(&ch), b"d/");
    }

    #[test]
    fn key16_is_the_hex_prefix() {
        let mut k = [0u8; 32];
        k[0] = 0x20;
        k[1] = 0x01;
        k[7] = 0xff;
        k[8] = 0xee;
        assert_eq!(key16(&Key(k)), "20010000000000ff");
    }

    /// `errors.Is(err, worktree.ErrConflict)`: the conflicts are listed only with the pull's own conflict
    /// error, not when the command fails with another one after the pull hit a conflict.
    #[test]
    fn conflicts_are_listed_only_with_the_conflict_error() {
        let file = 0o100644;
        let conflicts = || {
            Some(vec![
                Conflict {
                    path: b"d/y".to_vec(),
                    local: change("d/y", Kind::Deleted, Some(file), None),
                    incoming: change("d/y", Kind::Added, None, Some(file)),
                },
                Conflict {
                    path: b"a b".to_vec(),
                    local: change("a b", Kind::MetaChanged, Some(file), Some(file)),
                    incoming: change("a b", Kind::TypeChanged, Some(file), Some(0o120777)),
                },
            ])
        };
        let conflict = CliError::msg(dstore_worktree::Error::Conflict);
        assert_eq!(
            conflict_listing(Some(&conflict), conflicts()).map(|b| lossy(&b)),
            Some(
                "conflicts:\n  d/y (local: deleted, cluster: new)\n  a b (local: meta, cluster: type)\n"
                    .to_string()
            )
        );
        // runTUI's own error (the program could not run) replaces the transfer's: nothing is listed.
        let tui = CliError::Msg("could not open a new TTY: device not configured".to_string());
        assert_eq!(conflict_listing(Some(&tui), conflicts()), None);
        assert_eq!(conflict_listing(Some(&conflict), None), None);
        assert_eq!(conflict_listing(None, None), None);
    }
}

/// A minimal JSON reader for the vector files (the crate has no serde); test-only.
#[cfg(test)]
mod json {
    #[derive(Clone, Debug, PartialEq)]
    pub enum J {
        Null,
        Bool(bool),
        /// The number's text.
        Num(String),
        Str(String),
        Arr(Vec<J>),
        Obj(Vec<(String, J)>),
    }

    impl J {
        pub fn get(&self, k: &str) -> Option<&J> {
            match self {
                J::Obj(m) => m.iter().find(|(n, _)| n == k).map(|(_, v)| v),
                _ => None,
            }
        }

        pub fn at(&self, k: &str) -> &J {
            self.get(k)
                .unwrap_or_else(|| panic!("no key {k:?} in {self:?}"))
        }

        pub fn str(&self) -> &str {
            match self {
                J::Str(s) => s,
                other => panic!("not a string: {other:?}"),
            }
        }

        pub fn arr(&self) -> &[J] {
            match self {
                J::Arr(a) => a,
                other => panic!("not an array: {other:?}"),
            }
        }

        pub fn num(&self) -> i64 {
            match self {
                J::Num(n) => n.parse().expect("integer"),
                other => panic!("not a number: {other:?}"),
            }
        }

        pub fn bool(&self) -> bool {
            match self {
                J::Bool(b) => *b,
                other => panic!("not a bool: {other:?}"),
            }
        }

        pub fn is_null(&self) -> bool {
            matches!(self, J::Null)
        }
    }

    pub fn parse(text: &str) -> J {
        let mut p = Parser {
            b: text.as_bytes(),
            i: 0,
        };
        let v = p.value();
        p.ws();
        assert_eq!(p.i, p.b.len(), "trailing data at {}", p.i);
        v
    }

    struct Parser<'a> {
        b: &'a [u8],
        i: usize,
    }

    impl Parser<'_> {
        fn peek(&self) -> Option<u8> {
            self.b.get(self.i).copied()
        }

        fn next(&mut self) -> u8 {
            let c = self.peek().expect("unexpected end of JSON");
            self.i += 1;
            c
        }

        fn ws(&mut self) {
            while matches!(self.peek(), Some(b' ' | b'\n' | b'\r' | b'\t')) {
                self.i += 1;
            }
        }

        fn expect(&mut self, c: u8) {
            self.ws();
            assert_eq!(self.next(), c, "at {}", self.i);
        }

        fn literal(&mut self, word: &str, v: J) -> J {
            assert!(
                self.b[self.i..].starts_with(word.as_bytes()),
                "at {}",
                self.i
            );
            self.i += word.len();
            v
        }

        fn value(&mut self) -> J {
            self.ws();
            match self.peek().expect("unexpected end of JSON") {
                b'{' => {
                    self.i += 1;
                    let mut m = Vec::new();
                    self.ws();
                    if self.peek() == Some(b'}') {
                        self.i += 1;
                        return J::Obj(m);
                    }
                    loop {
                        self.ws();
                        let k = self.string();
                        self.expect(b':');
                        m.push((k, self.value()));
                        self.ws();
                        match self.next() {
                            b',' => {}
                            b'}' => return J::Obj(m),
                            c => panic!("unexpected {:?} at {}", c as char, self.i),
                        }
                    }
                }
                b'[' => {
                    self.i += 1;
                    let mut a = Vec::new();
                    self.ws();
                    if self.peek() == Some(b']') {
                        self.i += 1;
                        return J::Arr(a);
                    }
                    loop {
                        a.push(self.value());
                        self.ws();
                        match self.next() {
                            b',' => {}
                            b']' => return J::Arr(a),
                            c => panic!("unexpected {:?} at {}", c as char, self.i),
                        }
                    }
                }
                b'"' => J::Str(self.string()),
                b't' => self.literal("true", J::Bool(true)),
                b'f' => self.literal("false", J::Bool(false)),
                b'n' => self.literal("null", J::Null),
                _ => {
                    let start = self.i;
                    while matches!(
                        self.peek(),
                        Some(b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')
                    ) {
                        self.i += 1;
                    }
                    assert!(self.i > start, "bad value at {start}");
                    J::Num(String::from_utf8(self.b[start..self.i].to_vec()).expect("number"))
                }
            }
        }

        fn hex4(&mut self) -> u32 {
            let s = std::str::from_utf8(&self.b[self.i..self.i + 4]).expect("\\u escape");
            self.i += 4;
            u32::from_str_radix(s, 16).expect("\\u escape")
        }

        fn string(&mut self) -> String {
            self.expect(b'"');
            let mut out = Vec::new();
            loop {
                match self.next() {
                    b'"' => break,
                    b'\\' => match self.next() {
                        b'"' => out.push(b'"'),
                        b'\\' => out.push(b'\\'),
                        b'/' => out.push(b'/'),
                        b'b' => out.push(0x08),
                        b'f' => out.push(0x0c),
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'u' => {
                            let mut cp = self.hex4();
                            if (0xd800..0xdc00).contains(&cp)
                                && self.b[self.i..].starts_with(b"\\u")
                            {
                                self.i += 2;
                                let lo = self.hex4();
                                cp = 0x10000 + ((cp - 0xd800) << 10) + (lo - 0xdc00);
                            }
                            let ch = char::from_u32(cp).unwrap_or('\u{fffd}');
                            let mut buf = [0u8; 4];
                            out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                        }
                        c => panic!("bad escape {:?} at {}", c as char, self.i),
                    },
                    c => out.push(c),
                }
            }
            String::from_utf8(out).expect("UTF-8 string")
        }
    }
}
