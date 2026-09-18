//! Working-copy errors with Go's texts (worktree §2).

use amber_store_core::{fstree, ingest, key, packstore};
use dstore_gocompat::errno::PathError;

/// Display = the Go texts of worktree §2 (sentinels verbatim; wraps "%s: %w", "working copy %s: …",
/// "bad state file: base: …", "refusing unsafe path %q", "xattr %q: %w", "%s: mknod: %w", "chmod: %w", …).
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not a dstore working copy (no .dstore in this or any parent directory)")]
    NotWorkingCopy,
    #[error("incomplete clone: delete the directory and clone again")]
    Incomplete,
    #[error("the reference does not exist on the cluster: nothing to pull")]
    NoRemote,
    #[error("the cluster's tree moved since your last sync: pull first, or --force")]
    RemoteMoved,
    #[error("the reference was deleted on the cluster: --force to recreate it")]
    RemoteDeleted,
    #[error("conflicting changes: resolve them, or --force to take the cluster's side")]
    Conflict,
    #[error("reference changed on the cluster since your last fetch: pull first, or --force ({0})")]
    RefChanged(#[source] dstore_client::Error),
    #[error("client: unknown reference: {}", String::from_utf8_lossy(.0))]
    UnknownRefName(Vec<u8>),
    #[error("too large to diff")]
    TooLarge,
    /// Fully formatted Go texts without a typed cause.
    #[error("{0}")]
    Msg(String),
    #[error("{text}")]
    Wrapped {
        text: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },
    #[error("{0}")]
    Client(#[source] dstore_client::Error),
    #[error("{0}")]
    Path(#[source] PathError),
    #[error("{0}")]
    Packstore(#[source] packstore::Error),
    #[error("{0}")]
    Ingest(#[source] ingest::Error),
    #[error("{0}")]
    Walk(#[source] fstree::WalkError<packstore::Error>),
    #[error("{0}")]
    Key(#[source] key::Error),
}

impl Error {
    pub fn is_conflict(&self) -> bool {
        todo!()
    }

    pub fn is_no_remote(&self) -> bool {
        todo!()
    }
}
