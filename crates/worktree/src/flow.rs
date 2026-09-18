//! `worktree/flow.go`: flows over a connected cluster (async over the client).
//!
//! Go's ctx is observed only by the client calls (`ref_get`, `pull_tree`, `push`). The scan, the tree diff,
//! the merge, the applier and ingest run on the blocking pool holding `Arc<packstore::Store>` and are not
//! abortable, as in Go (PORTING §5.1, §5.4). The small JSON writes of `.dstore/config` and `.dstore/state` run
//! inline, where Go runs them.

use std::ffi::OsString;
use std::sync::Arc;

use amber_store_core::key::Key;
use amber_store_core::{ingest, packstore};
use dstore_client::{Cluster, Cond, Progress, PullStats, PushStats};
use dstore_gocompat::ctx::Ctx;
use dstore_gocompat::errno::PathError;
use dstore_gocompat::os;
use dstore_gocompat::path::to_path;
use dstore_gocompat::time::GoTime;

use crate::error::lossy;
use crate::{Change, Config, Conflict, DIR, Error, Tree};

/// `worktree.FetchResult`.
#[derive(Clone, Debug)]
pub struct FetchResult {
    /// The reference exists on the cluster.
    pub exists: bool,
    /// Its tree was already the stored remote.
    pub up_to_date: bool,
    pub key: Key,
    pub stats: PullStats,
}

/// The zero `FetchResult` (key all zero).
impl Default for FetchResult {
    fn default() -> FetchResult {
        FetchResult {
            exists: false,
            up_to_date: false,
            key: Key([0; 32]),
            stats: PullStats::default(),
        }
    }
}

/// `worktree.PullResult`.
#[derive(Clone, Debug, Default)]
pub struct PullResult {
    pub fetch: FetchResult,
    pub up_to_date: bool,
    pub applied: Vec<Change>,
    pub conflicts: Vec<Conflict>,
}

/// `worktree.PushResult`.
#[derive(Clone, Debug)]
pub struct PushResult {
    pub root: Key,
    /// The tree equals base and base is what the cluster holds.
    pub nothing: bool,
    /// The cluster already held this tree from an interrupted push.
    pub recovered: bool,
    pub built: packstore::WriteStats,
    pub stats: PushStats,
}

/// The zero `PushResult` (root all zero).
impl Default for PushResult {
    fn default() -> PushResult {
        PushResult {
            root: Key([0; 32]),
            nothing: false,
            recovered: false,
            built: packstore::WriteStats::default(),
            stats: PushStats::default(),
        }
    }
}

/// Runs blocking work on the blocking pool (PORTING §5.1). A panic there is a bug and resumes here.
async fn blocking<T, F>(f: F) -> Result<T, Error>
where
    F: FnOnce() -> Result<T, Error> + Send + 'static,
    T: Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(r) => r,
        Err(e) if e.is_panic() => std::panic::resume_unwind(e.into_panic()),
        // Only when the runtime shuts down under the task; Go has no such case.
        Err(e) => Err(Error::Msg(format!("worktree: blocking task: {e}"))),
    }
}

/// `t.Get` as an owned getter for the blocking pool.
fn getter(
    st: &Arc<packstore::Store>,
) -> impl Fn(Key) -> Result<Vec<u8>, packstore::Error> + Send + Sync + 'static {
    let st = Arc::clone(st);
    move |k| st.get(k)
}

/// Go `os.Stat`: follows symlinks; `stat <path>: <errno>`, with Go's EINVAL for a NUL byte.
fn os_stat(p: &[u8]) -> Result<std::fs::Metadata, PathError> {
    let err = |e| PathError {
        op: "stat",
        path: p.to_vec(),
        err: e,
    };
    if p.contains(&0) {
        return Err(err(std::io::Error::from_raw_os_error(libc::EINVAL)));
    }
    std::fs::metadata(to_path(p)).map_err(err)
}

impl Tree {
    /// Go `fetch`: reads the reference and pulls its tree into the packstore, updating the state in memory
    /// only. The result is filled as far as the fetch got, as Go's is.
    async fn fetch_state(
        &mut self,
        ctx: &Ctx,
        cl: &Cluster,
        prog: Option<Progress>,
    ) -> (FetchResult, Result<(), Error>) {
        let mut r = FetchResult::default();
        let name = lossy(&self.config.name).into_owned();
        let rf = match cl.ref_get(ctx, &name).await {
            Ok(rf) => rf,
            Err(e) if e.is_unknown_ref() => {
                self.state.has_remote = false;
                self.state.remote = Key([0; 32]);
                self.state.remote_version = None;
                return (r, Ok(()));
            }
            Err(e) => return (r, Err(Error::Client(e))),
        };
        let k = match Key::parse(&rf.reference.key) {
            Ok(k) => k,
            Err(e) => return (r, Err(Error::Key(e))),
        };
        r.exists = true;
        r.key = k;
        if self.state.has_remote && self.state.remote == k {
            // No PullTree, even when the local packstore lost objects (Go v0.1.9).
            r.up_to_date = true;
        } else if let Err(e) = cl
            .pull_tree(ctx, Arc::clone(&self.store), k, &mut r.stats, prog)
            .await
        {
            return (r, Err(Error::Client(e)));
        }
        self.state.has_remote = true;
        self.state.remote = k;
        self.state.remote_version = Some(rf.version);
        (r, Ok(()))
    }

    /// Go `Fetch`, keeping the result on error for `Pull`.
    async fn fetch_saved(
        &mut self,
        ctx: &Ctx,
        cl: &Cluster,
        prog: Option<Progress>,
    ) -> (FetchResult, Result<(), Error>) {
        let (r, res) = self.fetch_state(ctx, cl, prog).await;
        let res = res.and_then(|()| self.save_state());
        (r, res)
    }

    /// Go `Fetch`: records the reference's current tree as remote, then saves the state (only when the fetch
    /// succeeded).
    pub async fn fetch(
        &mut self,
        ctx: &Ctx,
        cl: &Cluster,
        prog: Option<Progress>,
    ) -> Result<FetchResult, Error> {
        let (r, res) = self.fetch_saved(ctx, cl, prog).await;
        res.map(|()| r)
    }

    /// Go `Pull`: fetches, then applies the remote's changes since base over the working directory, keeping
    /// local changes. With `force`, conflicts take the remote's side (appended after the merged changes);
    /// otherwise they abort the pull before anything is written. The result is filled even on error (the CLI
    /// prints `conflicts` on `Error::Conflict`). A failed apply does not move base.
    pub async fn pull(
        &mut self,
        ctx: &Ctx,
        cl: &Cluster,
        force: bool,
        jobs: usize,
        prog: Option<Progress>,
    ) -> (PullResult, Result<(), Error>) {
        let (fetch, res) = self.fetch_saved(ctx, cl, prog).await;
        let mut r = PullResult {
            fetch,
            ..PullResult::default()
        };
        if let Err(e) = res {
            return (r, Err(e));
        }
        if !self.state.has_remote {
            return (r, Err(Error::NoRemote));
        }
        if self.state.remote == self.state.base {
            r.up_to_date = true;
            return (r, Ok(()));
        }
        let (base, remote, synced_at) = (self.state.base, self.state.remote, self.state.synced_at);
        let root = self.root.clone();
        let get = getter(&self.store);
        let merged = blocking(move || {
            let local = crate::scan(&root, base, &get, synced_at, jobs)?;
            let incoming = crate::diff_trees(&get, base, remote)?;
            Ok(crate::merge(&local, &incoming))
        })
        .await;
        let (mut apply, conflicts) = match merged {
            Ok(v) => v,
            Err(e) => return (r, Err(e)),
        };
        r.conflicts = conflicts;
        if !r.conflicts.is_empty() {
            if !force {
                return (r, Err(Error::Conflict));
            }
            apply.extend(r.conflicts.iter().map(|c| c.incoming.clone()));
        }
        let root = self.root.clone();
        let get = getter(&self.store);
        match blocking(move || crate::apply(&root, &apply, &get).map(|()| apply)).await {
            Ok(applied) => r.applied = applied,
            Err(e) => return (r, Err(e)),
        }
        self.state.base = self.state.remote;
        self.state.synced_at = GoTime::now();
        let res = self.save_state();
        (r, res)
    }

    /// Go `Push`: builds the working directory's tree (`.dstore` excluded at the root), uploads it and writes
    /// the reference under compare-and-swap on the stored remote version. It refuses when base and remote
    /// differ unless `force`, which replaces the reference unconditionally. Push does not fetch first.
    pub async fn push(
        &mut self,
        ctx: &Ctx,
        cl: &Cluster,
        user: &str,
        force: bool,
        jobs: usize,
        prog: Option<Progress>,
    ) -> Result<PushResult, Error> {
        let (empty, _) = crate::empty_tree();
        let s = &self.state;
        let synced = (s.has_remote && s.remote == s.base) || (!s.has_remote && s.base == empty);
        if !synced && !force {
            return Err(if s.has_remote {
                Error::RemoteMoved
            } else {
                Error::RemoteDeleted
            });
        }
        let store = Arc::clone(&self.store);
        let dir = self.root.clone();
        let (built, root) = blocking(move || {
            let opts = ingest::Opts {
                jobs,
                exclude: vec![OsString::from(DIR)],
                ..Default::default()
            };
            let (stats, res) = ingest::dir(&store, to_path(&dir), opts);
            res.map(|k| (stats, k)).map_err(Error::Ingest)
        })
        .await?;
        let mut r = PushResult {
            root,
            built,
            ..PushResult::default()
        };
        if root == self.state.base && synced {
            r.nothing = true;
            return Ok(r);
        }
        let mut cond = Cond {
            force,
            ..Cond::default()
        };
        if !force {
            cond.versioned = true;
            // None (Go nil): the name must be new.
            cond.expected_version = self.state.remote_version.clone().unwrap_or_default();
        }
        let name = lossy(&self.config.name).into_owned();
        match cl
            .push(ctx, Arc::clone(&self.store), root, &name, user, cond, prog)
            .await
        {
            Ok(ps) => {
                self.state.remote_version = Some(ps.version.clone());
                r.stats = ps;
            }
            Err(e) => {
                // errors.As(err, &cm): a mismatch whose current key is this root is the trace of an
                // interrupted push; any other mismatch means the reference moved.
                let mismatch = e.cas_mismatch().map(|cm| {
                    let same =
                        cm.has_current && Key::parse(&cm.current).is_ok_and(|cur| cur == root);
                    (same, cm.version.clone())
                });
                match mismatch {
                    None => return Err(Error::Client(e)),
                    Some((false, _)) => return Err(Error::RefChanged(e)),
                    Some((true, version)) => {
                        r.recovered = true;
                        self.state.remote_version = Some(version);
                    }
                }
            }
        }
        self.state.base = root;
        self.state.remote = root;
        self.state.has_remote = true;
        self.state.synced_at = GoTime::now();
        self.save_state()?;
        Ok(r)
    }

    /// Go `RefreshTicket`: stores the ticket derived from the connected cluster's view when it differs from
    /// the stored one. It saves the stored config, never a flag-overridden copy.
    pub fn refresh_ticket(&mut self, cl: &Cluster) -> Result<(), Error> {
        let Some(v) = cl.view() else {
            return Ok(());
        };
        let s = crate::ticket_from_view(&v).encode();
        if s.as_bytes() == self.config.ticket.as_slice() {
            return Ok(());
        }
        self.config.ticket = s.into_bytes();
        self.save_config()
    }
}

/// Clone's checks and `Create`: `dir` must not exist (it is then created) or must be an empty directory.
/// Returns the tree and whether `dir` was created.
fn prepare_clone(dir: &[u8], cfg: Config) -> Result<(Tree, bool), Error> {
    let mut created = false;
    match os_stat(dir) {
        // errors.Is(err, fs.ErrNotExist): ENOENT only.
        Err(e) if e.err.raw_os_error() == Some(libc::ENOENT) => {
            os::mkdir_all(dir, 0o755).map_err(Error::Path)?;
            created = true;
        }
        Err(e) => return Err(Error::Path(e)),
        Ok(md) if !md.is_dir() => {
            return Err(Error::Msg(format!("{} is not a directory", lossy(dir))));
        }
        Ok(_) => {
            let ents = crate::sys::read_dir(dir).map_err(Error::Path)?;
            if !ents.is_empty() {
                return Err(Error::Msg(format!("{} is not empty", lossy(dir))));
            }
        }
    }
    match Tree::create(dir, cfg) {
        Ok(t) => Ok((t, created)),
        Err(e) => {
            if created {
                let _ = os::remove_all(dir);
            }
            Err(e)
        }
    }
}

/// Clone after `Create`: fetch, apply empty → remote, then the state file (written last).
async fn clone_fill(
    t: &mut Tree,
    ctx: &Ctx,
    cl: &Cluster,
    prog: Option<Progress>,
) -> Result<FetchResult, Error> {
    let (r, res) = t.fetch_state(ctx, cl, prog).await;
    res?;
    if !r.exists {
        return Err(Error::UnknownRefName(t.config.name.clone()));
    }
    let (base, remote) = (t.state.base, t.state.remote);
    let root = t.root.clone();
    let get = getter(&t.store);
    blocking(move || {
        let changes = crate::diff_trees(&get, base, remote)?;
        crate::apply(&root, &changes, &get)
    })
    .await?;
    t.state.base = t.state.remote;
    t.state.synced_at = GoTime::now();
    t.save_state()?;
    Ok(r)
}

/// `worktree.Clone`: a working copy of the reference in `dir`, which must not exist or must be empty. The
/// state file is written last, so an interrupted clone is recognisable. On an error the tree is closed and
/// what clone created is removed: all of `dir` when clone created it, otherwise only `dir/.dstore` (files
/// already applied stay behind, as in Go).
pub async fn clone(
    ctx: &Ctx,
    cl: &Cluster,
    dir: &[u8],
    cfg: Config,
    prog: Option<Progress>,
) -> Result<(Tree, FetchResult), Error> {
    let d = dir.to_vec();
    let (mut t, created) = blocking(move || prepare_clone(&d, cfg)).await?;
    match clone_fill(&mut t, ctx, cl, prog).await {
        Ok(r) => Ok((t, r)),
        Err(e) => {
            let d = dir.to_vec();
            let _ = blocking(move || {
                let _ = t.close();
                let _ = if created {
                    os::remove_all(&d).map_err(Error::Path)
                } else {
                    crate::remove(&d)
                };
                Ok(())
            })
            .await;
            Err(e)
        }
    }
}

/// `worktree.Init`: makes the existing directory `dir` a working copy of the reference, with the empty tree
/// as base, and fetches so that remote records whether the name exists. On an error `.dstore` is removed.
pub async fn init(
    ctx: &Ctx,
    cl: &Cluster,
    dir: &[u8],
    cfg: Config,
    prog: Option<Progress>,
) -> Result<(Tree, FetchResult), Error> {
    let d = dir.to_vec();
    let mut t = blocking(move || Tree::create(&d, cfg)).await?;
    let (r, mut res) = t.fetch_state(ctx, cl, prog).await;
    if res.is_ok() {
        t.state.synced_at = GoTime::now();
        res = t.save_state();
    }
    if let Err(e) = res {
        let d = dir.to_vec();
        let _ = blocking(move || {
            let _ = t.close();
            let _ = crate::remove(&d);
            Ok(())
        })
        .await;
        return Err(e);
    }
    Ok((t, r))
}
