//! `worktree/flow.go`: flows over a connected cluster (async over the client).

use amber_store_core::key::Key;
use amber_store_core::packstore;
use dstore_client::{Cluster, Progress, PullStats, PushStats};
use dstore_gocompat::ctx::Ctx;

use crate::{Change, Config, Conflict, Error, Tree};

/// `worktree.FetchResult`.
pub struct FetchResult {
    pub exists: bool,
    pub up_to_date: bool,
    pub key: Key,
    pub stats: PullStats,
}

/// `worktree.PullResult`.
pub struct PullResult {
    pub fetch: FetchResult,
    pub up_to_date: bool,
    pub applied: Vec<Change>,
    pub conflicts: Vec<Conflict>,
}

/// `worktree.PushResult`.
pub struct PushResult {
    pub root: Key,
    pub nothing: bool,
    pub recovered: bool,
    pub built: packstore::WriteStats,
    pub stats: PushStats,
}

impl Tree {
    /// Go `Fetch` (saves state).
    pub async fn fetch(
        &mut self,
        ctx: &Ctx,
        cl: &Cluster,
        prog: Option<Progress>,
    ) -> Result<FetchResult, Error> {
        todo!()
    }

    pub async fn pull(
        &mut self,
        ctx: &Ctx,
        cl: &Cluster,
        force: bool,
        jobs: usize,
        prog: Option<Progress>,
    ) -> (PullResult, Result<(), Error>) {
        todo!()
    }

    pub async fn push(
        &mut self,
        ctx: &Ctx,
        cl: &Cluster,
        user: &str,
        force: bool,
        jobs: usize,
        prog: Option<Progress>,
    ) -> Result<PushResult, Error> {
        todo!()
    }

    pub fn refresh_ticket(&mut self, cl: &Cluster) -> Result<(), Error> {
        todo!()
    }
}

/// `worktree.Clone`.
pub async fn clone(
    ctx: &Ctx,
    cl: &Cluster,
    dir: &[u8],
    cfg: Config,
    prog: Option<Progress>,
) -> Result<(Tree, FetchResult), Error> {
    todo!()
}

/// `worktree.Init`.
pub async fn init(
    ctx: &Ctx,
    cl: &Cluster,
    dir: &[u8],
    cfg: Config,
    prog: Option<Progress>,
) -> Result<(Tree, FetchResult), Error> {
    todo!()
}
