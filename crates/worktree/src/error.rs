//! Working-copy errors with Go's texts (worktree §2).

use std::borrow::Cow;

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
    /// Go `ErrInUse` as `Open` and `Create` return it, wrapped `working copy <root>: %w`: another dstore
    /// command, of either implementation, has the working copy open (`.dstore/lock`, dstore v0.1.11).
    #[error("working copy {}: in use by another dstore command", String::from_utf8_lossy(.0))]
    InUse(Vec<u8>),
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
    /// fstree walk errors, with entry names re-quoted as Go `%q` (PORTING §5.2, core-rs-gaps G4).
    #[error("{}", dstore_client::corefmt::walk_error_text(.0))]
    Walk(#[source] fstree::WalkError<packstore::Error>),
    #[error("{0}")]
    Key(#[source] key::Error),
}

impl Error {
    /// Go `errors.Is(err, ErrConflict)`.
    pub fn is_conflict(&self) -> bool {
        self.chain_any(|e| matches!(e, Error::Conflict))
    }

    /// Go `errors.Is(err, ErrInUse)`.
    pub fn is_in_use(&self) -> bool {
        self.chain_any(|e| matches!(e, Error::InUse(_)))
    }

    /// Go `errors.Is(err, ErrNoRemote)`.
    pub fn is_no_remote(&self) -> bool {
        self.chain_any(|e| matches!(e, Error::NoRemote))
    }

    /// Whether `pred` holds for this error or a worktree error it wraps.
    fn chain_any(&self, pred: impl Fn(&Error) -> bool) -> bool {
        let mut cur = Some(self);
        while let Some(e) = cur {
            if pred(e) {
                return true;
            }
            cur = match e {
                Error::Wrapped { source, .. } => source.downcast_ref::<Error>(),
                _ => None,
            };
        }
        false
    }
}

/// Go `%s` of a Go string held as bytes (lossy for invalid UTF-8, PORTING DD-8).
pub(crate) fn lossy(b: &[u8]) -> Cow<'_, str> {
    String::from_utf8_lossy(b)
}

/// A formatted Go text over a typed cause (`fmt.Errorf("…%w", err)`).
pub(crate) fn wrap(text: String, source: impl std::error::Error + Send + Sync + 'static) -> Error {
    Error::Wrapped {
        text,
        source: Box::new(source),
    }
}

/// A bare errno or other `io::Error` returned raw, rendered with Go's text.
pub(crate) fn io_error(e: std::io::Error) -> Error {
    let text = dstore_gocompat::errno::io_error_text(&e);
    wrap(text, e)
}

/// `fmt.Errorf("%s: %w", prefix, err)`.
pub(crate) fn prefixed(prefix: &[u8], err: Error) -> Error {
    let text = format!("{}: {}", lossy(prefix), err);
    wrap(text, err)
}

/// A `cborx` decode error with Go's text (`cborx:` prefix).
pub(crate) fn cbor_error(e: amber_store_core::cbor::Error) -> Error {
    let text = dstore_client::corefmt::cbor_error_text(&e);
    wrap(text, e)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sentinel_texts() {
        assert_eq!(
            Error::NotWorkingCopy.to_string(),
            "not a dstore working copy (no .dstore in this or any parent directory)"
        );
        assert_eq!(
            Error::Incomplete.to_string(),
            "incomplete clone: delete the directory and clone again"
        );
        assert_eq!(Error::TooLarge.to_string(), "too large to diff");
        assert_eq!(
            Error::UnknownRefName(b"trees/x".to_vec()).to_string(),
            "client: unknown reference: trees/x"
        );
    }

    #[test]
    fn predicates_see_through_wraps() {
        assert!(Error::Conflict.is_conflict());
        assert!(!Error::NoRemote.is_conflict());
        assert!(Error::NoRemote.is_no_remote());
        let wrapped = prefixed(b"p", Error::Conflict);
        assert_eq!(
            wrapped.to_string(),
            "p: conflicting changes: resolve them, or --force to take the cluster's side"
        );
        assert!(wrapped.is_conflict());
        assert!(!wrapped.is_no_remote());
    }

    #[test]
    fn io_errors_use_go_texts() {
        let e = io_error(std::io::Error::from_raw_os_error(libc::EACCES));
        assert_eq!(e.to_string(), "permission denied");
        let e = prefixed(
            b"sub/x",
            Error::Path(PathError {
                op: "remove",
                path: b"/abs/sub/x".to_vec(),
                err: std::io::Error::from_raw_os_error(libc::EACCES),
            }),
        );
        assert_eq!(e.to_string(), "sub/x: remove /abs/sub/x: permission denied");
    }
}
