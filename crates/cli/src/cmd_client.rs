//! Actions of `store push/pull`, `refs`, `watch`, `ref get/delete`, `ls` and `cat` (`cmd/dstore/client.go`).
//!
//! Each action keeps Go's order: argument checks, `signalCtx`, the local store, the dial, then the output.
//! The work after the dial lives in functions over a given [`Cluster`] (`*_with`, the transfer bodies), which
//! the unit tests drive over a `dstore_testkit::fake` cluster on the in-memory transport.
//!
//! Spec: port-notes/cli.md §2.5, §2.8.10, §2.8.12; PORTING.md §2.3, §5.1, §5.9.

use std::collections::HashMap;
use std::io::{self, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use amber_store_core::fstree::{self, Entry, WalkError};
use amber_store_core::key::Key;
use amber_store_core::{ingest, packstore, reference, refstore};
use dstore_client::{Cluster, Cond, Progress, PullStats, PushStats, Ref, corefmt};
use dstore_gocli::{CliError, Context};
use dstore_gocompat::ctx::Ctx;
use dstore_gocompat::errno::PathError;
use dstore_gocompat::fmt::hex_lower;
use dstore_gocompat::quote::quote;
use dstore_gocompat::slog::Logger;
use dstore_gocompat::time::{GoTime, SystemZone, Zone, format_rfc3339};
use futures::StreamExt;

use crate::common::{self, NetOpts, Session};
use crate::{nodeside, progress};

/// The first line of Go's panic in `cat NAME /`: `ResolveEntry` returns a nil entry, which
/// `len(e.ContentKey)` dereferences (DD-7).
const NIL_DEREF: &str = "runtime error: invalid memory address or nil pointer dereference";

/// `unix.S_IFMT`, `unix.S_IFDIR` (the same on Linux and macOS).
const S_IFMT: u64 = 0o170000;
const S_IFDIR: u64 = 0o040000;

// ---- actions ----

/// `store push PATH NAME` (`client.go:244-299`).
pub(crate) async fn store_push(c: &Context) -> Result<(), CliError> {
    if c.narg() != 2 {
        return Err(CliError::Msg("push PATH NAME".to_owned()));
    }
    let path = PathBuf::from(c.arg(0));
    let name_raw = c.arg(1).as_bytes().to_vec();
    dstore_client::validate_name_bytes(&name_raw).map_err(CliError::Msg)?;
    // A valid name is valid UTF-8.
    let name = lossy(&name_raw);
    let ctx = common::signal_ctx();
    let (st, refs) = common::open_local(c)?;
    let (root, stats) = ingest_tree(&st, path, jobs_of(c.int("jobs"))).await?;
    write_stderr(&built_line(&root, stats.stored));
    let cond = push_cond(c.bool("force"), c.os_string("expected-version").as_bytes())?;
    let user = c.os_string("user").into_vec();
    let dial = DialArgs::of(c);
    let cas = CasSeen::default();
    let body = PushBody {
        st: Arc::clone(&st),
        root,
        name: name.clone(),
        user: user.clone(),
        cond,
        cas: cas.clone(),
    };
    let res = progress::run_transfer(c, &ctx, format!("push {name}"), move |ctx, log, prog| {
        Box::pin(async move {
            let s = dial.dial(&ctx, &log).await?;
            let r = body.run(&ctx, &s.cluster, Some(prog)).await;
            s.close().await;
            r
        })
    })
    .await;
    let ps = res.map_err(|e| cas.hint(e))?;
    record_local(&refs, &name, &root, &user, GoTime::now().unix_nano());
    emit(&mut GoStdout::new(), &pushed_line(&name, &ps));
    Ok(())
}

/// `store pull NAME` (`client.go:309-340`).
pub(crate) async fn store_pull(c: &Context) -> Result<(), CliError> {
    let name_raw = c.first().as_bytes().to_vec();
    if name_raw.is_empty() {
        return Err(CliError::Msg("pull NAME".to_owned()));
    }
    let name = lossy(&name_raw);
    let ctx = common::signal_ctx();
    let (st, refs) = common::open_local(c)?;
    let dial = DialArgs::of(c);
    let local = Arc::clone(&st);
    let pull_name = name.clone();
    let ps = progress::run_transfer(c, &ctx, format!("pull {name}"), move |ctx, log, prog| {
        Box::pin(async move {
            let s = dial.dial(&ctx, &log).await?;
            let r = pull_body(&ctx, &s.cluster, local, &pull_name, Some(prog)).await;
            s.close().await;
            r
        })
    })
    .await?;
    let line = finish_pull(&refs, &name, &name_raw, &ps)?;
    emit(&mut GoStdout::new(), &line);
    Ok(())
}

/// `refs [PREFIX]` (`client.go:344-367`).
pub(crate) async fn refs(c: &Context) -> Result<(), CliError> {
    let ctx = common::signal_ctx();
    let s = common::dial_cluster(&ctx, c, &common::logger(c)).await?;
    let res = refs_with(
        &ctx,
        &s.cluster,
        c.first().as_bytes(),
        &mut GoStdout::new(),
        &SystemZone,
    )
    .await;
    s.close().await;
    res
}

/// `watch PATTERN` (`client.go:378-403`).
pub(crate) async fn watch(c: &Context) -> Result<(), CliError> {
    let pattern = c.first().as_bytes().to_vec();
    if pattern.is_empty() {
        return Err(CliError::Msg("watch PATTERN".to_owned()));
    }
    let ctx = common::signal_ctx();
    let s = common::dial_cluster(&ctx, c, &common::logger(c)).await?;
    let res = watch_with(
        &ctx,
        &s.cluster,
        lossy(&pattern),
        &mut GoStdout::new(),
        &SystemZone,
    )
    .await;
    s.close().await;
    res
}

/// `ref get NAME` (`client.go:412-427`).
pub(crate) async fn ref_get(c: &Context) -> Result<(), CliError> {
    let ctx = common::signal_ctx();
    let s = common::dial_cluster(&ctx, c, &common::logger(c)).await?;
    let res = ref_get_with(
        &ctx,
        &s.cluster,
        &lossy(c.first().as_bytes()),
        &mut GoStdout::new(),
        &SystemZone,
    )
    .await;
    s.close().await;
    res
}

/// `ref delete NAME` (`client.go:429-448`).
pub(crate) async fn ref_delete(c: &Context) -> Result<(), CliError> {
    let ctx = common::signal_ctx();
    let s = common::dial_cluster(&ctx, c, &common::logger(c)).await?;
    let res = ref_delete_with(
        &ctx,
        &s.cluster,
        &lossy(c.first().as_bytes()),
        c.bool("force"),
        c.os_string("expected-version").as_bytes(),
    )
    .await;
    s.close().await;
    res
}

/// `ls NAME [PATH]` (`client.go:476-508`).
pub(crate) async fn ls(c: &Context) -> Result<(), CliError> {
    let ctx = common::signal_ctx();
    let s = common::dial_cluster(&ctx, c, &common::logger(c)).await?;
    let res = ls_with(
        &ctx,
        &s.cluster,
        &lossy(c.first().as_bytes()),
        c.arg(1).as_bytes(),
        GoStdout::new(),
    )
    .await;
    s.close().await;
    res.map(drop)
}

/// `cat NAME PATH` (`client.go:518-550`).
pub(crate) async fn cat(c: &Context) -> Result<(), CliError> {
    if c.narg() != 2 {
        return Err(CliError::Msg("cat NAME PATH".to_owned()));
    }
    let ctx = common::signal_ctx();
    let s = common::dial_cluster(&ctx, c, &common::logger(c)).await?;
    let res = cat_with(
        &ctx,
        &s.cluster,
        &lossy(c.first().as_bytes()),
        c.arg(1).as_bytes(),
        GoStdout::new(),
    )
    .await;
    // Go's deferred cl.Close() also runs while the nil-entry panic unwinds.
    s.close().await;
    match res {
        Ok(Some(_)) => Ok(()),
        Ok(None) => crate::go_panic_exit(NIL_DEREF),
        Err(e) => Err(e),
    }
}

// ---- dialing inside a transfer ----

/// Go `dialClusterLog(ctx, c, log)` for a transfer body, which gets its logger from `run_transfer`. The flag
/// values are read before the transfer starts, because the body outlives the borrowed `Context`; the reads
/// have no side effects, so the order of the checks is Go's.
struct DialArgs {
    ticket: Vec<u8>,
    store: Vec<u8>,
    net: NetOpts,
}

impl DialArgs {
    fn of(c: &Context) -> DialArgs {
        DialArgs {
            ticket: c.os_string("ticket").into_vec(),
            store: c.os_string("store").into_vec(),
            net: common::net_opts_of(c),
        }
    }

    /// The ticket (`--ticket`, else `localTicket(--store)`, else [`common::NO_CLUSTER`]), then
    /// `dialTicket`.
    async fn dial(&self, ctx: &Ctx, log: &Logger) -> Result<Session, CliError> {
        let t = self.ticket()?;
        common::dial_ticket(ctx, t, &self.net, log).await
    }

    fn ticket(&self) -> Result<dstore_ticket::Ticket, CliError> {
        if !self.ticket.is_empty() {
            dstore_ticket::parse(&self.ticket).map_err(CliError::msg)
        } else if !self.store.is_empty() {
            nodeside::local_ticket(&self.store)
        } else {
            Err(CliError::Msg(common::NO_CLUSTER.to_owned()))
        }
    }
}

// ---- store push / store pull ----

/// `ingest.Dir(st, path, ingest.Opts{Jobs: jobs})` on a blocking thread.
async fn ingest_tree(
    st: &Arc<packstore::Store>,
    path: PathBuf,
    jobs: usize,
) -> Result<(Key, packstore::WriteStats), CliError> {
    let st = Arc::clone(st);
    let (stats, res) = blocking(move || {
        ingest::dir(
            &st,
            &path,
            ingest::Opts {
                jobs,
                ..ingest::Opts::default()
            },
        )
    })
    .await?;
    let root = res.map_err(CliError::msg)?;
    Ok((root, stats))
}

/// `--jobs` as core-rs takes it: Go's `Jobs < 1` (GOMAXPROCS) is 0 (core-rs-gaps G13).
fn jobs_of(n: i64) -> usize {
    usize::try_from(n).unwrap_or(0)
}

/// The push condition: `--force`, else must-be-new or `--expected-version` (hex).
fn push_cond(force: bool, expected_version: &[u8]) -> Result<Cond, CliError> {
    let mut cond = Cond {
        force,
        ..Cond::default()
    };
    if !force {
        cond.versioned = true;
        if !expected_version.is_empty() {
            cond.expected_version = common::hex_decode(expected_version).map_err(CliError::Msg)?;
        }
    }
    Ok(cond)
}

/// The delete condition: `--expected-version` (hex), else unconditional (with or without `--force`).
fn delete_cond(force: bool, expected_version: &[u8]) -> Result<Cond, CliError> {
    let mut cond = Cond {
        force,
        ..Cond::default()
    };
    if !expected_version.is_empty() {
        cond.expected_version = common::hex_decode(expected_version).map_err(CliError::Msg)?;
        cond.versioned = true;
    } else if !cond.force {
        cond.force = true;
    }
    Ok(cond)
}

/// Go's `errors.As(err, &cm)` on what `runTransfer` returns. The transfer body records the text of a CAS
/// mismatch; the hint is added only when `run_transfer` returns that same error, so a TUI run error, which
/// Go returns instead of the transfer's, gets none. The TUI's `failed:` line shows the error without the
/// hint, as Go's does.
#[derive(Clone, Default)]
struct CasSeen(Arc<Mutex<Option<String>>>);

impl CasSeen {
    fn note(&self, e: &dstore_client::Error) -> CliError {
        let text = e.to_string();
        if e.cas_mismatch().is_some() {
            *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(text.clone());
        }
        CliError::Msg(text)
    }

    fn hint(&self, e: CliError) -> CliError {
        let seen = self
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        match e {
            CliError::Msg(m) if seen.as_deref() == Some(m.as_str()) => {
                CliError::Msg(format!("{m} (pull first, or --force)"))
            }
            e => e,
        }
    }
}

/// What the `store push` transfer does after dialing.
struct PushBody {
    st: Arc<packstore::Store>,
    root: Key,
    name: String,
    /// `--user` as given (Go string bytes).
    user: Vec<u8>,
    cond: Cond,
    cas: CasSeen,
}

impl PushBody {
    async fn run(
        self,
        ctx: &Ctx,
        cl: &Cluster,
        prog: Option<Progress>,
    ) -> Result<PushStats, CliError> {
        // Go uploads the tree, then Push fails at `rec.Encode()` with the ValidateUser text. `Cluster::push`
        // takes `&str`, and a lossy user would write a reference. So a non-UTF-8 user is replaced by one
        // that core-rs rejects at the same `encode()`, after the upload, and that error gets Go's text.
        let (user, not_utf8) = match std::str::from_utf8(&self.user) {
            Ok(u) => (u, false),
            Err(_) => (REJECTED_USER, true),
        };
        cl.push(
            ctx,
            Arc::clone(&self.st),
            self.root,
            &self.name,
            user,
            self.cond.clone(),
            prog,
        )
        .await
        .map_err(|e| match e {
            // The only `Reference` error of a push is the record's `encode()`.
            dstore_client::Error::Reference(_) if not_utf8 => {
                let why = match dstore_client::validate_user_bytes(&self.user) {
                    Err(why) => why,
                    Ok(()) => "user must be valid UTF-8".to_owned(),
                };
                CliError::Msg(format!("reference user: {why}"))
            }
            e => self.cas.note(&e),
        })
    }
}

/// The user a non-UTF-8 `--user` is pushed as: core-rs `validate_user` rejects it (a control character)
/// when the record is encoded, after the upload, where Go's `Encode` rejects the real one.
const REJECTED_USER: &str = "\u{1}";

/// The local record of a push: this run's time, not the cluster record's; encoding and put errors are
/// ignored (an invalid `--user` just skips it).
fn record_local(refs: &refstore::Store, name: &str, root: &Key, user: &[u8], now_ns: i64) {
    let Ok(user) = std::str::from_utf8(user) else {
        // Go's `Encode` fails on it (ValidateUser).
        return;
    };
    let rec = reference::Reference {
        name: name.to_owned(),
        key: root.0.to_vec(),
        user: user.to_owned(),
        created_at: now_ns,
        ..reference::Reference::default()
    };
    if let Ok(enc) = rec.encode() {
        let _ = refs.put(name, &enc);
    }
}

/// What the `store pull` transfer does after dialing.
async fn pull_body(
    ctx: &Ctx,
    cl: &Cluster,
    st: Arc<packstore::Store>,
    name: &str,
    prog: Option<Progress>,
) -> Result<PullStats, CliError> {
    cl.pull(ctx, st, name, prog).await.map_err(CliError::msg)
}

/// The local reference of a pull (its error is returned), then the output line.
fn finish_pull(
    refs: &refstore::Store,
    name: &str,
    name_raw: &[u8],
    ps: &PullStats,
) -> Result<Vec<u8>, CliError> {
    refs.put(name, &ps.record).map_err(CliError::msg)?;
    Ok(pulled_line(name_raw, ps))
}

// ---- references ----

async fn refs_with(
    ctx: &Ctx,
    cl: &Cluster,
    prefix: &[u8],
    out: &mut (dyn Write + Send),
    zone: &dyn Zone,
) -> Result<(), CliError> {
    let refs = cl.ref_list(ctx, prefix).await.map_err(CliError::msg)?;
    for r in &refs {
        emit(
            out,
            &ref_line(
                r.name.as_bytes(),
                r.key.as_deref().unwrap_or_default(),
                r.created_at,
                r.user.as_bytes(),
                zone,
            ),
        );
    }
    Ok(())
}

/// Prints every change until the stream ends; the stream ends without an item when ctx is cancelled, so a
/// signal returns `Ok` (exit 0).
async fn watch_with(
    ctx: &Ctx,
    cl: &Cluster,
    pattern: String,
    out: &mut (dyn Write + Send),
    zone: &dyn Zone,
) -> Result<(), CliError> {
    let mut changes = cl.watch_refs(ctx.clone(), pattern, HashMap::new());
    while let Some(item) = changes.next().await {
        let ch = item.map_err(CliError::msg)?;
        if ch.synced {
            continue;
        }
        if ch.deleted {
            emit(out, &deleted_line(ch.name.as_bytes()));
            continue;
        }
        emit(
            out,
            &ref_line(
                ch.name.as_bytes(),
                ch.key.as_deref().unwrap_or_default(),
                ch.created_at,
                ch.user.as_bytes(),
                zone,
            ),
        );
    }
    Ok(())
}

async fn ref_get_with(
    ctx: &Ctx,
    cl: &Cluster,
    name: &str,
    out: &mut (dyn Write + Send),
    zone: &dyn Zone,
) -> Result<(), CliError> {
    let r = cl.ref_get(ctx, name).await.map_err(CliError::msg)?;
    emit(out, &ref_get_text(&r, zone));
    Ok(())
}

async fn ref_delete_with(
    ctx: &Ctx,
    cl: &Cluster,
    name: &str,
    force: bool,
    expected_version: &[u8],
) -> Result<(), CliError> {
    // After dialing, as Go decodes the hex.
    let cond = delete_cond(force, expected_version)?;
    cl.ref_delete(ctx, name, &cond).await.map_err(CliError::msg)
}

// ---- trees: ls, cat and the cluster getter ----

/// Go's `get := clusterGet(ctx, cl)` as an fstree getter for a blocking thread (PORTING.md §5.1): each
/// call blocks on [`common::cluster_get`] through the runtime handle.
fn cluster_get(
    rt: tokio::runtime::Handle,
    ctx: &Ctx,
    cl: &Cluster,
) -> impl FnMut(Key) -> Result<Vec<u8>, dstore_client::Error> + Send + 'static {
    let (ctx, cl) = (ctx.clone(), cl.clone());
    move |k| rt.block_on(common::cluster_get(&ctx, &cl, k))
}

/// The root key of the tree under `name`.
async fn tree_root(ctx: &Ctx, cl: &Cluster, name: &str) -> Result<Key, CliError> {
    let r = cl.ref_get(ctx, name).await.map_err(CliError::msg)?;
    Key::parse(&r.reference.key).map_err(CliError::msg)
}

/// Lists the directory at `path` (`""` and `"/"` are the root; otherwise `strings.Trim(path, "/")` is
/// resolved) of the tree under `name`, one entry name per line, raw bytes.
async fn ls_with<W: Write + Send + 'static>(
    ctx: &Ctx,
    cl: &Cluster,
    name: &str,
    path: &[u8],
    mut out: W,
) -> Result<W, CliError> {
    let root = tree_root(ctx, cl, name).await?;
    let mut get = cluster_get(tokio::runtime::Handle::current(), ctx, cl);
    let path = path.to_vec();
    let entries = blocking(move || {
        // A branch is listed through its commit's tree.
        let root = dstore_client::tree_of(root, &mut get).map_err(CliError::msg)?;
        let mut dir = root;
        if !path.is_empty() && path != b"/" {
            dir = resolve_path(root, trim_slashes(&path), &mut get)?;
        }
        fstree::collect_entries(dir, &mut get).map_err(walk_msg)
    })
    .await??;
    for e in &entries {
        let mut line = e.name.clone();
        line.push(b'\n');
        emit(&mut out, &line);
    }
    Ok(out)
}

/// Streams the file at `strings.Trim(path, "/")` of the tree under `name` to `out`, which is flushed at the
/// end or on error. `Ok(None)` is Go's nil entry (the path names the root): the caller panics (DD-7).
async fn cat_with<W: Write + Send + 'static>(
    ctx: &Ctx,
    cl: &Cluster,
    name: &str,
    path: &[u8],
    out: W,
) -> Result<Option<W>, CliError> {
    let root = tree_root(ctx, cl, name).await?;
    let mut get = cluster_get(tokio::runtime::Handle::current(), ctx, cl);
    let path = trim_slashes(path).to_vec();
    blocking(move || {
        let root = dstore_client::tree_of(root, &mut get).map_err(CliError::msg)?;
        let Some(e) = resolve_entry(root, &path, &mut get)? else {
            return Ok(None);
        };
        if e.content_key.len() != 32 {
            return Err(CliError::Msg("not a regular file with content".to_owned()));
        }
        let k = Key::parse(&e.content_key).map_err(CliError::msg)?;
        let mut out = out;
        let res = fstree::write_content(&mut out, k, &mut get);
        let _ = out.flush();
        res.map_err(walk_msg)?;
        Ok(Some(out))
    })
    .await?
}

/// Go `fstree.ResolvePath` over the path's bytes (core-rs takes `&str`; a non-UTF-8 component must still
/// find its entry). Errors carry Go's texts (`walk_error_text`, and [`dot_dot`] over the raw path).
fn resolve_path<G, E>(root: Key, path: &[u8], get: &mut G) -> Result<Key, CliError>
where
    G: FnMut(Key) -> Result<Vec<u8>, E>,
    E: std::fmt::Display,
{
    let mut k = root;
    for comp in path.split(|&b| b == b'/') {
        if comp.is_empty() || comp == b"." {
            continue;
        }
        if comp == b".." {
            return Err(dot_dot(path));
        }
        let found = fstree::lookup_entry(k, comp, &mut *get).map_err(walk_msg)?;
        if found.mode & S_IFMT != S_IFDIR {
            return Err(walk_msg(WalkError::<E>::NotDir {
                name: comp.to_vec(),
            }));
        }
        k = Key::parse(&found.content_key).map_err(|source| {
            walk_msg(WalkError::<E>::ContentKey {
                name: comp.to_vec(),
                source,
            })
        })?;
    }
    Ok(k)
}

/// Go `fstree.ResolveEntry` over the path's bytes: `None` for the root (the empty path, or only `""` and
/// `"."` components). Errors as [`resolve_path`]'s.
fn resolve_entry<G, E>(root: Key, path: &[u8], get: &mut G) -> Result<Option<Entry>, CliError>
where
    G: FnMut(Key) -> Result<Vec<u8>, E>,
    E: std::fmt::Display,
{
    let mut dir = root;
    let mut cur: Option<Entry> = None;
    for comp in path.split(|&b| b == b'/') {
        if comp.is_empty() || comp == b"." {
            continue;
        }
        if comp == b".." {
            return Err(dot_dot(path));
        }
        if let Some(c) = &cur {
            if c.mode & S_IFMT != S_IFDIR {
                return Err(walk_msg(WalkError::<E>::NotDir {
                    name: c.name.clone(),
                }));
            }
            dir = Key::parse(&c.content_key).map_err(|source| {
                walk_msg(WalkError::<E>::ContentKey {
                    name: c.name.clone(),
                    source,
                })
            })?;
        }
        cur = Some(fstree::lookup_entry(dir, comp, &mut *get).map_err(walk_msg)?);
    }
    Ok(cur)
}

/// `fmt.Errorf("fstree: %q: \"..\" is not supported", path)` over the path's raw bytes (core-rs's
/// `WalkError::DotDot` holds a `String`, which would render invalid UTF-8 as U+FFFD).
fn dot_dot(path: &[u8]) -> CliError {
    CliError::Msg(format!("fstree: {}: \"..\" is not supported", quote(path)))
}

/// `strings.Trim(p, "/")`.
fn trim_slashes(p: &[u8]) -> &[u8] {
    let start = p.iter().position(|&b| b != b'/').unwrap_or(p.len());
    let end = p.iter().rposition(|&b| b != b'/').map_or(start, |i| i + 1);
    p.get(start..end).unwrap_or_default()
}

fn walk_msg<E: std::fmt::Display>(e: WalkError<E>) -> CliError {
    CliError::Msg(corefmt::walk_error_text(&e))
}

// ---- output ----

/// `time.Unix(0, ns).Format(time.RFC3339)`.
fn rfc3339(ns: i64, zone: &dyn Zone) -> String {
    format_rfc3339(GoTime::from_unix_nano(ns), zone)
}

/// `"%s\t%x\t%s\t%s\n"`: name, key, RFC3339 creation time, user (`refs`, `watch`).
fn ref_line(name: &[u8], key: &[u8], created_at: i64, user: &[u8], zone: &dyn Zone) -> Vec<u8> {
    let mut b = Vec::with_capacity(name.len() + user.len() + 96);
    b.extend_from_slice(name);
    b.push(b'\t');
    b.extend_from_slice(hex_lower(key).as_bytes());
    b.push(b'\t');
    b.extend_from_slice(rfc3339(created_at, zone).as_bytes());
    b.push(b'\t');
    b.extend_from_slice(user);
    b.push(b'\n');
    b
}

/// `"%s\tdeleted\n"` (`watch`).
fn deleted_line(name: &[u8]) -> Vec<u8> {
    let mut b = name.to_vec();
    b.extend_from_slice(b"\tdeleted\n");
    b
}

/// `ref get`: `"name %s\nkey %x\nversion %x\nuser %s\ncreated %s\n"`.
fn ref_get_text(r: &Ref, zone: &dyn Zone) -> Vec<u8> {
    format!(
        "name {}\nkey {}\nversion {}\nuser {}\ncreated {}\n",
        r.name,
        hex_lower(&r.reference.key),
        hex_lower(&r.version),
        r.reference.user,
        rfc3339(r.reference.created_at, zone)
    )
    .into_bytes()
}

/// `store push`, stderr: `"built %s: %d new objects\n"` with the root's first 16 hex characters.
fn built_line(root: &Key, stored: usize) -> Vec<u8> {
    format!(
        "built {}: {stored} new objects\n",
        hex_lower(root.0.get(..8).unwrap_or_default())
    )
    .into_bytes()
}

/// `store push`: `"pushed %s: %d objects, %d uploaded, version %x\n"`.
fn pushed_line(name: &str, ps: &PushStats) -> Vec<u8> {
    format!(
        "pushed {name}: {} objects, {} uploaded, version {}\n",
        ps.keys,
        ps.uploaded,
        hex_lower(&ps.version)
    )
    .into_bytes()
}

/// `store pull`: `"pulled %s: root %s, %d objects fetched (%d bytes)\n"`, the root in full (64 hex).
fn pulled_line(name: &[u8], ps: &PullStats) -> Vec<u8> {
    let mut b = b"pulled ".to_vec();
    b.extend_from_slice(name);
    b.extend_from_slice(
        format!(
            ": root {}, {} objects fetched ({} bytes)\n",
            ps.root, ps.fetched, ps.bytes
        )
        .as_bytes(),
    );
    b
}

/// One Go `Printf` to stdout: one write, flushed (PORTING.md §5.9); errors are ignored as Go's are.
fn emit(out: &mut (dyn Write + Send), b: &[u8]) {
    let _ = out.write_all(b);
    let _ = out.flush();
}

/// `fmt.Fprintf(os.Stderr, …)`.
fn write_stderr(b: &[u8]) {
    let mut e = io::stderr();
    let _ = e.write_all(b);
    let _ = e.flush();
}

/// Go's `os.Stdout`: unbuffered (every write reaches fd 1 before it returns), and its write errors are
/// `*PathError`s, `write /dev/stdout: <errno>`.
struct GoStdout(io::Stdout);

impl GoStdout {
    fn new() -> GoStdout {
        GoStdout(io::stdout())
    }
}

impl Write for GoStdout {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.0
            .write_all(b)
            .and_then(|()| self.0.flush())
            .map_err(stdout_error)?;
        Ok(b.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush().map_err(stdout_error)
    }
}

fn stdout_error(err: io::Error) -> io::Error {
    io::Error::new(
        err.kind(),
        PathError {
            op: "write",
            path: b"/dev/stdout".to_vec(),
            err,
        },
    )
}

// ---- helpers ----

fn lossy(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// `spawn_blocking`; a panic on the blocking thread is re-raised here (a core-rs bug, as a panicking
/// goroutine would crash Go).
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<T, CliError> {
    match tokio::task::spawn_blocking(f).await {
        Ok(v) => Ok(v),
        Err(e) if e.is_panic() => std::panic::resume_unwind(e.into_panic()),
        Err(e) => Err(CliError::Msg(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use dstore_client::{CasMismatch, Config};
    use dstore_gocompat::errno::rewrite_os_errors;
    use dstore_gocompat::slog::{Attr, Handler, Level, Record};
    use dstore_gocompat::time::FixedZone;
    use dstore_testkit::fake::{FakeCluster, FakeClusterConfig};
    use dstore_transport::Endpoint;
    use dstore_transport::mem::Network;
    use dstore_view::NodeId;

    use super::*;

    // ---- harness ----

    struct Discard;

    impl Handler for Discard {
        fn enabled(&self, _level: Level) -> bool {
            false
        }
        fn handle(&self, _handler_attrs: &[Attr], _r: &Record) {}
    }

    fn ok<T, E: std::fmt::Display>(r: Result<T, E>) -> T {
        match r {
            Ok(v) => v,
            Err(e) => panic!("{e}"),
        }
    }

    fn err_text<T>(r: Result<T, CliError>) -> String {
        match r {
            Ok(_) => panic!("succeeded"),
            Err(e) => e.to_string(),
        }
    }

    /// A scratch directory under the system temp dir, removed on drop.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Scratch {
            static N: AtomicUsize = AtomicUsize::new(0);
            let p = std::env::temp_dir().join(format!(
                "dstore-cmd-client-{tag}-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&p);
            ok(std::fs::create_dir_all(&p));
            Scratch(p)
        }

        fn path(&self, rel: &str) -> PathBuf {
            self.0.join(rel)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A writer the test and a spawned task share.
    #[derive(Clone, Default)]
    struct SharedBuf(Arc<Mutex<Vec<u8>>>);

    impl SharedBuf {
        fn bytes(&self) -> Vec<u8> {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    impl Write for SharedBuf {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// Polls until `cond` holds, for at most 10 s (real time).
    async fn eventually(what: &str, mut cond: impl FnMut() -> bool) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !cond() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "timeout waiting for {what}"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// A three-node fake cluster (R = 3) and a client dialled over the in-memory network.
    struct Env {
        fc: Arc<FakeCluster>,
        cl: Cluster,
        ctx: Ctx,
        _net: Arc<Network>,
    }

    impl Env {
        async fn new() -> Env {
            let net = Network::new();
            let fc = FakeCluster::start(&net, FakeClusterConfig::default()).await;
            let endpoint: Arc<dyn Endpoint> = net.bind(NodeId([0xc1; 32]), &[]);
            let cfg = Config {
                endpoint: Some(endpoint),
                ticket: fc.ticket(),
                logger: Some(Logger::new(Arc::new(Discard))),
                request_timeout: Duration::from_secs(20),
                ..Config::default()
            };
            let cl = ok(Cluster::dial(&Ctx::background(), cfg).await);
            Env {
                fc,
                cl,
                ctx: Ctx::background(),
                _net: net,
            }
        }

        async fn close(self) {
            self.cl.close();
            self.fc.close().await;
        }

        /// Pushes the tree ingested from `src` under `name` (must be new) as `store push` does.
        async fn push(
            &self,
            st: &Arc<packstore::Store>,
            src: &Path,
            name: &str,
        ) -> (Key, PushStats) {
            let (root, _) = ok(ingest_tree(st, src.to_path_buf(), 0).await);
            let body = PushBody {
                st: Arc::clone(st),
                root,
                name: name.to_owned(),
                user: b"alice".to_vec(),
                cond: ok(push_cond(false, b"")),
                cas: CasSeen::default(),
            };
            let ps = ok(body.run(&self.ctx, &self.cl, None).await);
            (root, ps)
        }
    }

    fn stores(dir: &Scratch, sub: &str) -> (Arc<packstore::Store>, refstore::Store) {
        let st = ok(packstore::Store::open_with(
            dir.path(sub).join("packstore"),
            packstore::Options::new().sync(false),
        ));
        let refs = ok(refstore::Store::open(dir.path(sub).join("refs"), false));
        (Arc::new(st), refs)
    }

    /// a.txt, e/ (empty), link → a.txt, sub/b.txt, sub/deeper/c.txt.
    fn write_tree(dir: &Path) {
        ok(std::fs::create_dir_all(dir.join("e")));
        ok(std::fs::create_dir_all(dir.join("sub/deeper")));
        ok(std::fs::write(dir.join("a.txt"), b"alpha\n"));
        ok(std::fs::write(dir.join("sub/b.txt"), b"beta\n"));
        ok(std::fs::write(dir.join("sub/deeper/c.txt"), b"gamma"));
        ok(std::os::unix::fs::symlink("a.txt", dir.join("link")));
    }

    fn reference_record(name: &str, root: &Key, user: &str, created_at: i64) -> Vec<u8> {
        let r = reference::Reference {
            name: name.to_owned(),
            key: root.0.to_vec(),
            user: user.to_owned(),
            created_at,
            ..reference::Reference::default()
        };
        ok(r.encode())
    }

    const T0: i64 = 1_758_198_896_123_456_789;

    // ---- output helpers ----

    #[test]
    fn ref_lines_are_gos() {
        let utc = FixedZone(0);
        assert_eq!(
            ref_line(b"trees/a", &[0xab, 0x01], T0, b"alice", &utc),
            b"trees/a\tab01\t2025-09-18T12:34:56Z\talice\n"
        );
        assert_eq!(
            ref_line(b"trees/a", &[], T0, b"", &FixedZone(7200)),
            b"trees/a\t\t2025-09-18T14:34:56+02:00\t\n"
        );
        assert_eq!(
            ref_line(b"x", &[0xff], -1, b"u", &FixedZone(-5 * 3600)),
            b"x\tff\t1969-12-31T18:59:59-05:00\tu\n"
        );
        // Names and users go out as raw bytes.
        assert_eq!(
            ref_line(b"n\xff", &[], 0, b"\xfe", &utc),
            b"n\xff\t\t1970-01-01T00:00:00Z\t\xfe\n"
        );
        assert_eq!(deleted_line(b"trees/\xffx"), b"trees/\xffx\tdeleted\n");
    }

    #[test]
    fn ref_get_text_is_gos() {
        let r = Ref {
            name: "trees/a".to_owned(),
            record: Vec::new(),
            version: vec![0x00, 0x2a],
            reference: reference::Reference {
                name: "trees/a".to_owned(),
                key: vec![0x5a; 32],
                user: "alice".to_owned(),
                created_at: T0,
                ..reference::Reference::default()
            },
        };
        let want = format!(
            "name trees/a\nkey {}\nversion 002a\nuser alice\ncreated 2025-09-18T14:34:56+02:00\n",
            "5a".repeat(32)
        );
        assert_eq!(ref_get_text(&r, &FixedZone(7200)), want.as_bytes());
        // %x of an empty version prints nothing: "version " keeps its trailing space.
        let empty = Ref {
            version: Vec::new(),
            reference: reference::Reference {
                user: String::new(),
                created_at: 0,
                ..r.reference.clone()
            },
            ..r
        };
        let want = format!(
            "name trees/a\nkey {}\nversion \nuser \ncreated 1970-01-01T00:00:00Z\n",
            "5a".repeat(32)
        );
        assert_eq!(ref_get_text(&empty, &FixedZone(0)), want.as_bytes());
    }

    #[test]
    fn transfer_lines_are_gos() {
        let mut k = [0x11; 32];
        k[..8].copy_from_slice(&[0x20, 0x6d, 0x9e, 0xa2, 0x93, 0x59, 0xa7, 0x1c]);
        assert_eq!(
            built_line(&Key(k), 2),
            b"built 206d9ea29359a71c: 2 new objects\n"
        );
        assert_eq!(
            built_line(&Key(k), 0),
            b"built 206d9ea29359a71c: 0 new objects\n"
        );
        let ps = PushStats {
            keys: 5,
            uploaded: 3,
            bytes: 99,
            version: vec![0x01, 0xfe],
        };
        assert_eq!(
            pushed_line("trees/x", &ps),
            b"pushed trees/x: 5 objects, 3 uploaded, version 01fe\n"
        );
        let none = PushStats {
            version: Vec::new(),
            ..ps
        };
        assert_eq!(
            pushed_line("trees/x", &none),
            b"pushed trees/x: 5 objects, 3 uploaded, version \n"
        );
        let pull = PullStats {
            keys: 7,
            fetched: 4,
            bytes: 100,
            root: Key(k),
            ..PullStats::default()
        };
        // store pull prints all 64 hex characters of the root.
        let want = format!(
            "pulled trees/é: root {}, 4 objects fetched (100 bytes)\n",
            Key(k)
        );
        assert_eq!(pulled_line("trees/é".as_bytes(), &pull), want.as_bytes());
        assert_eq!(Key(k).to_string().len(), 64);
    }

    #[test]
    fn trim_slashes_is_strings_trim() {
        for (p, want) in [
            (&b""[..], &b""[..]),
            (b"/", b""),
            (b"//", b""),
            (b"a", b"a"),
            (b"/a/b/", b"a/b"),
            (b"//a//b//", b"a//b"),
            (b"./x", b"./x"),
            (b"x/.", b"x/."),
        ] {
            assert_eq!(trim_slashes(p), want, "{:?}", String::from_utf8_lossy(p));
        }
    }

    #[test]
    fn jobs_below_one_mean_all_cores() {
        assert_eq!(jobs_of(-3), 0);
        assert_eq!(jobs_of(0), 0);
        assert_eq!(jobs_of(5), 5);
    }

    #[test]
    fn push_and_delete_conditions() {
        let force = ok(push_cond(true, b"zz"));
        assert_eq!(
            force,
            Cond {
                force: true,
                ..Cond::default()
            }
        );
        assert_eq!(
            ok(push_cond(false, b"")),
            Cond {
                versioned: true,
                ..Cond::default()
            }
        );
        assert_eq!(
            ok(push_cond(false, b"0aBc")),
            Cond {
                versioned: true,
                expected_version: vec![0x0a, 0xbc],
                ..Cond::default()
            }
        );
        // hexDecode drops an odd trailing nibble.
        assert_eq!(ok(push_cond(false, b"abc")).expected_version, vec![0xab]);
        assert_eq!(err_text(push_cond(false, b"zz")), "bad hex \"zz\"");

        let unconditional = Cond {
            force: true,
            ..Cond::default()
        };
        assert_eq!(ok(delete_cond(false, b"")), unconditional);
        assert_eq!(ok(delete_cond(true, b"")), unconditional);
        assert_eq!(
            ok(delete_cond(false, b"01")),
            Cond {
                versioned: true,
                expected_version: vec![1],
                ..Cond::default()
            }
        );
        assert_eq!(
            ok(delete_cond(true, b"01")),
            Cond {
                versioned: true,
                expected_version: vec![1],
                force: true,
                ..Cond::default()
            }
        );
        assert_eq!(err_text(delete_cond(true, b"0g")), "bad hex \"0g\"");
    }

    #[test]
    fn cas_hint_only_for_the_noted_mismatch() {
        let cas = CasSeen::default();
        assert_eq!(
            cas.hint(CliError::Msg("cas mismatch: reference is absent".into()))
                .to_string(),
            "cas mismatch: reference is absent"
        );
        let other = cas.note(&dstore_client::Error::UnknownRef);
        assert_eq!(other.to_string(), "client: unknown reference");
        assert_eq!(cas.hint(other).to_string(), "client: unknown reference");
        let mismatch = dstore_client::Error::CasMismatch(CasMismatch {
            current: vec![0xab; 2],
            record: Vec::new(),
            version: Vec::new(),
            has_current: true,
        });
        let noted = cas.note(&mismatch);
        assert_eq!(noted.to_string(), "cas mismatch: current key abab");
        assert_eq!(
            cas.hint(noted).to_string(),
            "cas mismatch: current key abab (pull first, or --force)"
        );
        // A different error returned by run_transfer (a TUI run error) gets no hint.
        assert_eq!(
            cas.hint(CliError::Msg("error entering raw mode: x".into()))
                .to_string(),
            "error entering raw mode: x"
        );
        assert_eq!(
            cas.hint(CliError::Exit {
                msg: "cas mismatch: current key abab".into(),
                code: 1
            })
            .to_string(),
            "cas mismatch: current key abab"
        );
    }

    #[test]
    fn stdout_errors_are_path_errors() {
        assert_eq!(
            stdout_error(io::Error::from_raw_os_error(libc::EBADF)).to_string(),
            "write /dev/stdout: bad file descriptor"
        );
        let io_walk: WalkError<dstore_client::Error> =
            WalkError::Io(stdout_error(io::Error::from_raw_os_error(libc::EIO)));
        assert_eq!(
            walk_msg(io_walk).to_string(),
            "write /dev/stdout: input/output error"
        );
    }

    #[test]
    fn dial_args_resolve_the_ticket_in_gos_order() {
        let args = |ticket: &[u8], store: &[u8]| DialArgs {
            ticket: ticket.to_vec(),
            store: store.to_vec(),
            net: NetOpts {
                relay: String::new(),
                no_relay: false,
                no_discovery: false,
            },
        };
        assert_eq!(err_text(args(b"", b"").ticket()), common::NO_CLUSTER);
        let bogus = "ticket: \"bogus\" is neither a dstore1 ticket nor a node id: invalid length";
        assert_eq!(err_text(args(b"bogus", b"").ticket()), bogus);
        // --ticket wins over --store.
        assert_eq!(
            err_text(args(b"bogus", b"/nonexistent-dstore").ticket()),
            bogus
        );
        assert_eq!(
            err_text(args(b"", b"/nonexistent-dstore").ticket()),
            "node: no identity in /nonexistent-dstore: open /nonexistent-dstore/identity: no such file or directory"
        );
        assert_eq!(err_text(args(b"   ", b"").ticket()), "ticket: empty");
    }

    #[test]
    fn local_record_of_a_push() {
        let dir = Scratch::new("record");
        let (_st, refs) = stores(&dir, "l");
        let root = Key([0x21; 32]);
        record_local(&refs, "trees/x", &root, b"alice", 1234);
        let got = ok(reference::Reference::decode(&ok(refs.get("trees/x"))));
        assert_eq!(
            (
                got.name.as_str(),
                got.key,
                got.user.as_str(),
                got.created_at
            ),
            ("trees/x", root.0.to_vec(), "alice", 1234)
        );
        // An empty user is allowed.
        record_local(&refs, "trees/y", &root, b"", 5);
        assert_eq!(
            ok(reference::Reference::decode(&ok(refs.get("trees/y")))).user,
            ""
        );
        // Go's Encode rejects these users; nothing is written.
        record_local(&refs, "trees/z", &root, b"\xff", 5);
        record_local(&refs, "trees/w", &root, b"a\x01", 5);
        assert!(refs.get("trees/z").is_err());
        assert!(refs.get("trees/w").is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn ingest_errors_read_as_gos() {
        let dir = Scratch::new("ingest");
        let (st, _refs) = stores(&dir, "l");
        let missing = dir.path("missing");
        let text = rewrite_os_errors(&err_text(ingest_tree(&st, missing.clone(), 0).await));
        assert_eq!(
            text,
            format!("stat {}: no such file or directory", missing.display())
        );
        write_tree(&dir.path("src"));
        let (root, stats) = ok(ingest_tree(&st, dir.path("src"), 0).await);
        assert!(stats.stored > 0);
        // A second build stores nothing new and gives the same root.
        let (again, stats) = ok(ingest_tree(&st, dir.path("src"), 0).await);
        assert_eq!((again, stats.stored), (root, 0));
    }

    // ---- over a fake cluster ----

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn store_push_then_pull() {
        let e = Env::new().await;
        let dir = Scratch::new("push-pull");
        write_tree(&dir.path("src"));
        let (st, refs) = stores(&dir, "local");
        let (root, ps) = e.push(&st, &dir.path("src"), "trees/t").await;
        assert!(ps.keys >= 6, "{ps:?}");
        assert_eq!(ps.uploaded, ps.keys);
        assert!(!ps.version.is_empty());
        let line = pushed_line("trees/t", &ps);
        assert_eq!(
            line,
            format!(
                "pushed trees/t: {} objects, {} uploaded, version {}\n",
                ps.keys,
                ps.uploaded,
                hex_lower(&ps.version)
            )
            .as_bytes()
        );
        record_local(&refs, "trees/t", &root, b"alice", 7);
        assert_eq!(
            ok(reference::Reference::decode(&ok(refs.get("trees/t")))).key,
            root.0.to_vec()
        );
        let r = ok(e.cl.ref_get(&e.ctx, "trees/t").await);
        assert_eq!(
            (r.reference.key.clone(), r.reference.user.as_str()),
            (root.0.to_vec(), "alice")
        );

        // Must be new: the second push fails with the hint.
        let cas = CasSeen::default();
        let again = PushBody {
            st: Arc::clone(&st),
            root,
            name: "trees/t".to_owned(),
            user: b"alice".to_vec(),
            cond: ok(push_cond(false, b"")),
            cas: cas.clone(),
        };
        let res = again
            .run(&e.ctx, &e.cl, None)
            .await
            .map_err(|x| cas.hint(x));
        assert_eq!(
            err_text(res),
            format!("cas mismatch: current key {root} (pull first, or --force)")
        );
        // --expected-version with the version push printed, then --force.
        let versioned = PushBody {
            st: Arc::clone(&st),
            root,
            name: "trees/t".to_owned(),
            user: b"bob".to_vec(),
            cond: ok(push_cond(false, hex_lower(&ps.version).as_bytes())),
            cas: CasSeen::default(),
        };
        let v2 = ok(versioned.run(&e.ctx, &e.cl, None).await);
        assert_eq!(v2.uploaded, 0);
        assert_ne!(v2.version, ps.version);
        let forced = PushBody {
            st: Arc::clone(&st),
            root,
            name: "trees/t".to_owned(),
            user: Vec::new(),
            cond: ok(push_cond(true, b"")),
            cas: CasSeen::default(),
        };
        let v3 = ok(forced.run(&e.ctx, &e.cl, None).await);
        assert_ne!(v3.version, v2.version);

        // Pull into a fresh store: every object is fetched and the local reference is the cluster's record.
        let (st2, refs2) = stores(&dir, "other");
        let pull = ok(pull_body(&e.ctx, &e.cl, Arc::clone(&st2), "trees/t", None).await);
        assert_eq!(pull.root, root);
        assert_eq!(pull.fetched, ps.keys);
        assert_eq!(pull.version, v3.version);
        let line = ok(finish_pull(&refs2, "trees/t", b"trees/t", &pull));
        assert_eq!(line, pulled_line(b"trees/t", &pull));
        assert!(
            String::from_utf8_lossy(&line).contains(&format!(": root {root}, ")),
            "{}",
            String::from_utf8_lossy(&line)
        );
        assert_eq!(ok(refs2.get("trees/t")), pull.record);
        let r = ok(e.cl.ref_get(&e.ctx, "trees/t").await);
        assert_eq!(pull.record, r.record);
        assert!(st2.get(root).is_ok());

        assert_eq!(
            err_text(pull_body(&e.ctx, &e.cl, st2, "trees/none", None).await),
            "client: unknown reference"
        );
        e.close().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn store_push_with_a_non_utf8_user_writes_no_reference() {
        let e = Env::new().await;
        let dir = Scratch::new("user");
        write_tree(&dir.path("src"));
        let (st, _refs) = stores(&dir, "local");
        let (root, _) = ok(ingest_tree(&st, dir.path("src"), 0).await);
        let body = |user: Vec<u8>| PushBody {
            st: Arc::clone(&st),
            root,
            name: "trees/u".to_owned(),
            user,
            cond: ok(push_cond(false, b"")),
            cas: CasSeen::default(),
        };
        assert_eq!(
            err_text(body(b"al\xffce".to_vec()).run(&e.ctx, &e.cl, None).await),
            "reference user: user must be valid UTF-8"
        );
        // As in Go, the tree was uploaded before the record's encoding failed.
        assert!(
            e.fc.ids()
                .into_iter()
                .any(|id| e.fc.stored(id).contains_key(&root.0)),
            "the root was not uploaded"
        );
        // ValidateUser checks the length first.
        assert_eq!(
            err_text(body(vec![0xff; 1025]).run(&e.ctx, &e.cl, None).await),
            "reference user: user exceeds 1024 bytes"
        );
        // A valid UTF-8 user that ValidateUser rejects fails in Push itself, with the same text.
        assert_eq!(
            err_text(body(b"a\x01".to_vec()).run(&e.ctx, &e.cl, None).await),
            "reference user: user must not contain control characters"
        );
        assert_eq!(
            err_text(e.cl.ref_get(&e.ctx, "trees/u").await.map_err(CliError::msg)),
            "client: unknown reference"
        );
        e.close().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn refs_ref_get_and_ref_delete() {
        let e = Env::new().await;
        let dir = Scratch::new("refs");
        write_tree(&dir.path("src"));
        let (st, _refs) = stores(&dir, "local");
        let (root, _) = e.push(&st, &dir.path("src"), "trees/t").await;
        ok(e.cl
            .ref_put(
                &e.ctx,
                &reference_record("trees/u", &root, "bob", T0),
                &Cond {
                    versioned: true,
                    ..Cond::default()
                },
            )
            .await);
        ok(e.cl
            .ref_put(
                &e.ctx,
                &reference_record("other/x", &root, "", 0),
                &Cond {
                    versioned: true,
                    ..Cond::default()
                },
            )
            .await);
        let zone = FixedZone(3600);
        let t = ok(e.cl.ref_get(&e.ctx, "trees/t").await);
        let t_line = ref_line(b"trees/t", &root.0, t.reference.created_at, b"alice", &zone);
        let u_line = format!("trees/u\t{root}\t2025-09-18T13:34:56+01:00\tbob\n").into_bytes();

        let mut out = Vec::new();
        ok(refs_with(&e.ctx, &e.cl, b"trees/", &mut out, &zone).await);
        assert_eq!(out, [t_line.clone(), u_line.clone()].concat());
        let mut out = Vec::new();
        ok(refs_with(&e.ctx, &e.cl, b"", &mut out, &zone).await);
        assert_eq!(out.split(|&b| b == b'\n').count(), 4, "{out:?}");
        let mut out = Vec::new();
        ok(refs_with(&e.ctx, &e.cl, b"trees/u", &mut out, &zone).await);
        assert_eq!(out, u_line);
        let mut out = Vec::new();
        ok(refs_with(&e.ctx, &e.cl, b"none/", &mut out, &zone).await);
        assert!(out.is_empty());

        let mut out = Vec::new();
        ok(ref_get_with(&e.ctx, &e.cl, "trees/u", &mut out, &zone).await);
        let u = ok(e.cl.ref_get(&e.ctx, "trees/u").await);
        assert_eq!(
            out,
            format!(
                "name trees/u\nkey {root}\nversion {}\nuser bob\ncreated 2025-09-18T13:34:56+01:00\n",
                hex_lower(&u.version)
            )
            .as_bytes()
        );
        let mut out = Vec::new();
        assert_eq!(
            err_text(ref_get_with(&e.ctx, &e.cl, "trees/none", &mut out, &zone).await),
            "client: unknown reference"
        );
        assert!(out.is_empty());

        // ref delete: a wrong version is a CAS mismatch (no hint); bad hex; the right version; the
        // unconditional default.
        assert_eq!(
            err_text(ref_delete_with(&e.ctx, &e.cl, "trees/u", false, b"00").await),
            format!("cas mismatch: current key {root}")
        );
        assert_eq!(
            err_text(ref_delete_with(&e.ctx, &e.cl, "trees/u", false, b"zz").await),
            "bad hex \"zz\""
        );
        ok(ref_delete_with(
            &e.ctx,
            &e.cl,
            "trees/u",
            false,
            hex_lower(&u.version).as_bytes(),
        )
        .await);
        assert!(e.cl.ref_get(&e.ctx, "trees/u").await.is_err());
        ok(ref_delete_with(&e.ctx, &e.cl, "other/x", false, b"").await);
        assert!(e.cl.ref_get(&e.ctx, "other/x").await.is_err());
        e.close().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn ls_lists_directories() {
        let e = Env::new().await;
        let dir = Scratch::new("ls");
        write_tree(&dir.path("src"));
        let (st, _refs) = stores(&dir, "local");
        e.push(&st, &dir.path("src"), "trees/t").await;
        let ls = |path: &'static [u8]| {
            let (ctx, cl) = (e.ctx.clone(), e.cl.clone());
            async move { ls_with(&ctx, &cl, "trees/t", path, Vec::new()).await }
        };
        for p in [&b""[..], b"/", b"//", b"./", b"."] {
            assert_eq!(ok(ls(p).await), b"a.txt\ne\nlink\nsub\n", "{p:?}");
        }
        for p in [&b"sub"[..], b"/sub/", b"sub//", b"./sub", b"//sub/./"] {
            assert_eq!(ok(ls(p).await), b"b.txt\ndeeper\n", "{p:?}");
        }
        assert_eq!(ok(ls(b"sub/deeper").await), b"c.txt\n");
        assert_eq!(ok(ls(b"e").await), b"");
        assert_eq!(
            err_text(ls(b"a.txt").await),
            "fstree: \"a.txt\": not a directory"
        );
        assert_eq!(
            err_text(ls(b"link").await),
            "fstree: \"link\": not a directory"
        );
        assert_eq!(
            err_text(ls(b"nope").await),
            "fstree: \"nope\": entry not found"
        );
        assert_eq!(
            err_text(ls(b"sub/n\xffpe").await),
            "fstree: \"n\\xffpe\": entry not found"
        );
        // The path in the ".." error is the trimmed one.
        assert_eq!(
            err_text(ls(b"/sub/../x/").await),
            "fstree: \"sub/../x\": \"..\" is not supported"
        );
        assert_eq!(
            err_text(ls(b"../x").await),
            "fstree: \"../x\": \"..\" is not supported"
        );
        // Go quotes the raw path: an invalid byte is `\xff`, not U+FFFD.
        assert_eq!(
            err_text(ls(b"../\xff").await),
            "fstree: \"../\\xff\": \"..\" is not supported"
        );
        assert_eq!(
            err_text(ls_with(&e.ctx, &e.cl, "trees/none", b"", Vec::new()).await),
            "client: unknown reference"
        );
        e.close().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cat_streams_files() {
        let e = Env::new().await;
        let dir = Scratch::new("cat");
        write_tree(&dir.path("src"));
        let (st, _refs) = stores(&dir, "local");
        let (root, _) = e.push(&st, &dir.path("src"), "trees/t").await;
        let cat = |path: &'static [u8]| {
            let (ctx, cl) = (e.ctx.clone(), e.cl.clone());
            async move { cat_with(&ctx, &cl, "trees/t", path, Vec::new()).await }
        };
        assert_eq!(ok(cat(b"a.txt").await), Some(b"alpha\n".to_vec()));
        assert_eq!(
            ok(cat(b"/sub/deeper/c.txt/").await),
            Some(b"gamma".to_vec())
        );
        assert_eq!(ok(cat(b"./sub/b.txt").await), Some(b"beta\n".to_vec()));
        // The root is Go's nil entry: the action panics (DD-7).
        for p in [&b"/"[..], b"", b".", b"//", b"./."] {
            assert_eq!(ok(cat(p).await), None, "{p:?}");
        }
        assert_eq!(
            err_text(cat(b"link").await),
            "not a regular file with content"
        );
        let sub = ok(fstree::resolve_path(root, "sub", |k| st.get(k)));
        assert_eq!(
            err_text(cat(b"sub").await),
            format!("{sub} is not a file-content object (type DirLeaf)")
        );
        assert_eq!(
            err_text(cat(b"a.txt/x").await),
            "fstree: \"a.txt\": not a directory"
        );
        assert_eq!(
            err_text(cat(b"nope").await),
            "fstree: \"nope\": entry not found"
        );
        assert_eq!(
            err_text(cat(b"sub/../a.txt").await),
            "fstree: \"sub/../a.txt\": \"..\" is not supported"
        );
        assert_eq!(
            err_text(cat(b"/../\xfe/").await),
            "fstree: \"../\\xfe\": \"..\" is not supported"
        );
        e.close().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cluster_getter_reports_missing_objects() {
        let e = Env::new().await;
        let dir = Scratch::new("getter");
        ok(std::fs::create_dir_all(dir.path("never")));
        ok(std::fs::write(dir.path("never/z.txt"), b"never pushed"));
        let (st, _refs) = stores(&dir, "local");
        // Built locally, never pushed.
        let (never, _) = ok(ingest_tree(&st, dir.path("never"), 0).await);
        let get = cluster_get(tokio::runtime::Handle::current(), &e.ctx, &e.cl);
        let res = ok(blocking(move || {
            let mut get = get;
            let one = get(never).map(drop).map_err(|x| x.to_string());
            let walk = fstree::collect_entries(never, &mut get).map(drop);
            (one, walk.map_err(|x| corefmt::walk_error_text(&x)))
        })
        .await);
        assert_eq!(res.0, Err(format!("object {never} not found")));
        assert_eq!(
            res.1,
            Err(format!("fstree: reading {never}: object {never} not found"))
        );
        e.close().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn watch_prints_changes_until_cancelled() {
        let e = Env::new().await;
        let dir = Scratch::new("watch");
        write_tree(&dir.path("src"));
        let (st, _refs) = stores(&dir, "local");
        let (root, _) = e.push(&st, &dir.path("src"), "trees/t").await;
        let t = ok(e.cl.ref_get(&e.ctx, "trees/t").await);
        let zone = FixedZone(0);
        let t_line = ref_line(b"trees/t", &root.0, t.reference.created_at, b"alice", &zone);

        let ctx = e.ctx.with_cancel();
        let buf = SharedBuf::default();
        let task = {
            let (ctx, cl, mut out) = (ctx.clone(), e.cl.clone(), buf.clone());
            tokio::spawn(async move {
                watch_with(&ctx, &cl, "trees/**".to_owned(), &mut out, &FixedZone(0)).await
            })
        };
        eventually("the existing reference", || buf.bytes() == t_line).await;

        let versioned = Cond {
            versioned: true,
            ..Cond::default()
        };
        ok(e.cl
            .ref_put(
                &e.ctx,
                &reference_record("other/x", &root, "", 0),
                &versioned,
            )
            .await);
        ok(e.cl
            .ref_put(
                &e.ctx,
                &reference_record("trees/u", &root, "bob", T0),
                &versioned,
            )
            .await);
        let u_line = format!("trees/u\t{root}\t2025-09-18T12:34:56Z\tbob\n").into_bytes();
        let want = [t_line.clone(), u_line.clone()].concat();
        eventually("the new reference", || buf.bytes() == want).await;

        ok(ref_delete_with(&e.ctx, &e.cl, "trees/u", false, b"").await);
        let want = [t_line, u_line, b"trees/u\tdeleted\n".to_vec()].concat();
        eventually("the deletion", || buf.bytes() == want).await;

        ctx.cancel();
        let res = ok(tokio::time::timeout(Duration::from_secs(10), task).await);
        assert!(ok(res).is_ok());
        assert_eq!(buf.bytes(), want);
        e.close().await;
    }
}
