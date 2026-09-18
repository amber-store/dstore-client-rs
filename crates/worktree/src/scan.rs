//! `worktree/scan.go`: the working directory against a tree (blocking).

use amber_store_core::key::Key;
use dstore_gocompat::time::GoTime;

use crate::{Change, Error, Getter};

/// `worktree.Scan`.
pub fn scan(
    root: &[u8],
    base: Key,
    get: Getter<'_>,
    synced_at: GoTime,
    jobs: usize,
) -> Result<Vec<Change>, Error> {
    todo!()
}
