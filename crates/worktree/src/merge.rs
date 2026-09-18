//! `worktree/merge.go` (blocking).

use crate::Change;

/// `worktree.Conflict`.
#[derive(Clone, Debug)]
pub struct Conflict {
    pub path: Vec<u8>,
    pub local: Change,
    pub incoming: Change,
}

/// `worktree.Merge`: (changes to apply, conflicts).
pub fn merge(local: &[Change], incoming: &[Change]) -> (Vec<Change>, Vec<Conflict>) {
    todo!()
}
