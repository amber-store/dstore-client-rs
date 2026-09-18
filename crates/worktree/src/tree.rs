//! `worktree/tree.go`: the working copy on disk (blocking).

use std::sync::Arc;

use amber_store_core::key::Key;
use amber_store_core::packstore;
use dstore_gocompat::time::GoTime;

use crate::{Change, Error};

/// `worktree.Config` (`.dstore/config`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Config {
    pub ticket: Vec<u8>,
    pub name: Vec<u8>,
    pub relay: Vec<u8>,
    pub no_relay: bool,
    pub no_discovery: bool,
    pub user: Vec<u8>,
}

/// `worktree.State` (`.dstore/state`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub base: Key,
    pub remote: Key,
    pub has_remote: bool,
    pub remote_version: Option<Vec<u8>>,
    pub synced_at: GoTime,
}

/// `*worktree.Tree`.
pub struct Tree {
    pub root: Vec<u8>,
    pub config: Config,
    pub state: State,
    pub store: Arc<packstore::Store>,
}

/// Where the reference stands on the cluster.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteState {
    UpToDate,
    Moved,
    Absent,
}

/// `worktree.Status`.
pub struct Status {
    pub changes: Vec<Change>,
    pub meta_only: usize,
    pub remote: RemoteState,
    pub incoming: Vec<Change>,
}

/// Reads an object by key.
pub type Getter<'a> = &'a (dyn Fn(Key) -> Result<Vec<u8>, packstore::Error> + Sync);

/// The empty tree: key 2001bbe6…, bytes 80.
pub fn empty_tree() -> (Key, Vec<u8>) {
    todo!()
}

/// `worktree.Find`.
pub fn find(dir: &[u8]) -> Result<Vec<u8>, Error> {
    todo!()
}

/// `worktree.Remove`.
pub fn remove(dir: &[u8]) -> Result<(), Error> {
    todo!()
}

impl Tree {
    /// `worktree.Open`: lock before state (a locked copy reports the lock error).
    pub fn open(dir: &[u8]) -> Result<Tree, Error> {
        todo!()
    }

    /// `worktree.Create`: no state file.
    pub fn create(dir: &[u8], cfg: Config) -> Result<Tree, Error> {
        todo!()
    }

    pub fn close(self) -> Result<(), Error> {
        todo!()
    }

    pub fn get(&self, k: Key) -> Result<Vec<u8>, packstore::Error> {
        todo!()
    }

    pub fn save_config(&self) -> Result<(), Error> {
        todo!()
    }

    pub fn save_state(&self) -> Result<(), Error> {
        todo!()
    }

    pub fn status(&self, jobs: usize) -> Result<Status, Error> {
        todo!()
    }
}
