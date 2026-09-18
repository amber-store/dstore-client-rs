//! `worktree/apply.go`: the applier (blocking; not abortable, as in Go).

use crate::{Change, Error, Getter};

/// `worktree.Apply`.
pub fn apply(root: &[u8], changes: &[Change], get: Getter<'_>) -> Result<(), Error> {
    todo!()
}
