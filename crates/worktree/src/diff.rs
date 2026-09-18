//! `worktree/diff.go`: unified diffs and `--stat` (blocking; writes as it goes, partial output may
//! precede an error).

use amber_store_core::fstree::Entry;

use crate::{Change, Error, Getter};

/// Why content is unavailable.
pub enum SourceError {
    TooLarge,
    Other(Error),
}

/// Content of one side of a diff. Data may come with an error.
pub trait Source {
    fn content(&self, path: &[u8], e: &Entry) -> (Option<Vec<u8>>, Option<SourceError>);
}

/// Content from the store.
pub struct TreeSource<'a> {
    pub get: Getter<'a>,
}

/// Content from the working directory.
pub struct DiskSource {
    pub root: Vec<u8>,
}

impl Source for TreeSource<'_> {
    fn content(&self, path: &[u8], e: &Entry) -> (Option<Vec<u8>>, Option<SourceError>) {
        todo!()
    }
}

impl Source for DiskSource {
    fn content(&self, path: &[u8], e: &Entry) -> (Option<Vec<u8>>, Option<SourceError>) {
        todo!()
    }
}

/// `worktree.Unified`.
pub fn unified(
    w: &mut dyn std::io::Write,
    changes: &[Change],
    old: &dyn Source,
    new: &dyn Source,
) -> Result<(), Error> {
    todo!()
}

/// `worktree.Stat`.
pub fn stat(
    w: &mut dyn std::io::Write,
    changes: &[Change],
    old: &dyn Source,
    new: &dyn Source,
) -> Result<(), Error> {
    todo!()
}
