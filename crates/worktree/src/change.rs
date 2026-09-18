//! `worktree/change.go`: change classification and the tree diff (blocking).

use std::sync::Arc;

use amber_store_core::fstree::Entry;
use amber_store_core::key::Key;

use crate::{Error, Getter};

/// `worktree.Kind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Added,
    Deleted,
    Modified,
    TypeChanged,
    ModeChanged,
    MetaChanged,
}

/// "new" "deleted" "modified" "type" "mode" "meta".
impl std::fmt::Display for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        todo!()
    }
}

/// `worktree.Change`.
#[derive(Clone, Debug)]
pub struct Change {
    pub path: Vec<u8>,
    pub kind: Kind,
    pub old: Option<Arc<Entry>>,
    pub new: Option<Arc<Entry>>,
}

pub fn type_name(mode: u64) -> String {
    todo!()
}

pub fn is_dir(e: Option<&Entry>) -> bool {
    todo!()
}

pub fn same_content(a: &Entry, b: &Entry) -> bool {
    todo!()
}

pub fn equivalent(a: &Entry, b: &Entry) -> bool {
    todo!()
}

pub fn compare(old: &Entry, new: &Entry) -> Option<Kind> {
    todo!()
}

/// `worktree.DiffTrees`: changes in walk order.
pub fn diff_trees(get: Getter<'_>, a: Key, b: Key) -> Result<Vec<Change>, Error> {
    todo!()
}
