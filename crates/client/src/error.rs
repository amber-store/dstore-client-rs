//! Client errors with Go's texts (client-core §2.14, client-transfer §3.4) (part A).

use std::error::Error as StdError;
use std::sync::Arc;

use amber_store_core::{amberpack, fstree, key, packstore, reference};

use crate::{CasMismatch, CtxError, Incomplete};

/// The client's error type. `Display` is Go's `Error()` text; wrapped errors are also `source()`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("client: no endpoint")]
    NoEndpoint,
    #[error("client: ticket names no nodes")]
    TicketNamesNoNodes,
    #[error("client: no bootstrap node answered: {0}")]
    NoBootstrap(#[source] Box<Error>),
    #[error("client: unexpected reply {0}")]
    UnexpectedReply(i64),
    #[error("client: no nodes")]
    NoNodes,
    #[error("client: unknown reference")]
    UnknownRef,
    #[error("{0}")]
    CasMismatch(#[source] CasMismatch),
    #[error("{0}")]
    Incomplete(#[source] Incomplete),
    #[error("client: watch stream idle")]
    WatchIdle,
    /// "%w: type %d" of `wire.ErrProtocol`.
    #[error("wire: unexpected frame: type {0}")]
    UnexpectedFrame(i64),
    #[error("{0}")]
    Remote(#[source] dstore_wire::RemoteError),
    #[error("{0}")]
    Transport(#[source] dstore_transport::TransportError),
    #[error("{0}")]
    Wire(#[source] dstore_wire::WireError),
    #[error("{0}")]
    PackRecords(#[source] dstore_wire::PackRecordsError),
    #[error("{0}")]
    Ctx(#[source] CtxError),
    #[error("{0}")]
    View(#[source] dstore_view::ViewError),
    #[error("{0}")]
    Reference(#[source] reference::Error),
    #[error("{0}")]
    Key(#[source] key::Error),
    #[error("{0}")]
    Amberpack(#[source] amberpack::Error),
    #[error("{0}")]
    Packstore(#[source] packstore::Error),
    #[error("{0}")]
    ChildKeys(#[source] fstree::ChildKeysError),
    #[error("no owners")]
    NoOwners,
    #[error("payload hashes to {want}, not {key}")]
    PayloadHash { want: String, key: String },
    #[error("walk local tree: {0}")]
    WalkLocalTree(#[source] fstree::WalkError<packstore::Error>),
    #[error("negotiate {} at its primary: {source}", dstore_gocompat::fmt::hex_lower(.key8))]
    Negotiate {
        key8: [u8; 8],
        source: std::sync::Arc<Error>,
    },
    #[error("upload to {node}: {source}")]
    UploadTo {
        node: String,
        source: std::sync::Arc<Error>,
    },
    #[error("record {} rejected: {reason}", dstore_gocompat::fmt::hex_lower(.key8))]
    RecordRejected { key8: [u8; 8], reason: String },
    /// `names` = `%v` of a `[]string`.
    #[error("push: {count} keys could not be placed; owners not confirming: {names}")]
    NotPlaced { count: usize, names: String },
    /// Go `fmt.Errorf("%w; last upload error: %v", shortError, lastErr)`: only `placed` is wrapped.
    #[error("{placed}; last upload error: {last}")]
    NotPlacedLastError {
        #[source]
        placed: Box<Error>,
        last: std::sync::Arc<Error>,
    },
    #[error("pull: fetch ended early")]
    FetchEndedEarly,
    #[error("pull: object {} not found in the cluster", dstore_gocompat::fmt::hex_lower(.key8))]
    PullObjectNotFound { key8: [u8; 8] },
    #[error("pull: tree incomplete after fetch: {0}")]
    PullIncomplete(#[source] fstree::WalkError<packstore::Error>),
    /// RecordSource failures from callers.
    #[error("{0}")]
    Other(String),
}

/// A client error seen through the `source()` chain: the error itself, or one boxed or shared inside a
/// wrapper (a `Box<Error>` or `Arc<Error>` source is its own node of the chain, and its `source()` skips
/// to the inner error's source).
fn as_client<'a>(e: &'a (dyn StdError + 'static)) -> Option<&'a Error> {
    if let Some(c) = e.downcast_ref::<Error>() {
        return Some(c);
    }
    if let Some(b) = e.downcast_ref::<Box<Error>>() {
        return Some(b);
    }
    e.downcast_ref::<Arc<Error>>().map(|a| &**a)
}

impl Error {
    /// The error and its `source()` chain, as Go's `errors.As` walks `%w` wrapping.
    fn chain(&self) -> impl Iterator<Item = &(dyn StdError + 'static)> {
        let mut next: Option<&(dyn StdError + 'static)> = Some(self);
        std::iter::from_fn(move || {
            let cur = next?;
            next = cur.source();
            Some(cur)
        })
    }

    /// `errors.As` through the source chain.
    pub fn remote(&self) -> Option<&dstore_wire::RemoteError> {
        dstore_wire::as_remote(self)
    }

    /// `wire.IsCode`.
    pub fn is_code(&self, code: &str) -> bool {
        self.remote().is_some_and(|e| e.code == code)
    }

    /// `errors.As(err, **client.CASMismatch)`.
    pub fn cas_mismatch(&self) -> Option<&CasMismatch> {
        self.chain().find_map(|e| e.downcast_ref::<CasMismatch>())
    }

    /// `errors.As(err, **client.Incomplete)`.
    pub fn incomplete(&self) -> Option<&Incomplete> {
        self.chain().find_map(|e| e.downcast_ref::<Incomplete>())
    }

    /// `errors.Is(err, client.ErrUnknownRef)`.
    pub fn is_unknown_ref(&self) -> bool {
        self.chain()
            .any(|e| matches!(as_client(e), Some(Error::UnknownRef)))
    }

    /// `errors.Is(err, context.Canceled)` / `context.DeadlineExceeded`: the context error in the chain.
    pub fn ctx_error(&self) -> Option<CtxError> {
        self.chain()
            .find_map(|e| e.downcast_ref::<CtxError>().copied())
    }
}

/// Remote/Transport/Wire/Ctx 1:1.
impl From<dstore_transport::CallError> for Error {
    fn from(e: dstore_transport::CallError) -> Error {
        use dstore_transport::CallError;
        match e {
            CallError::Remote(r) => Error::Remote(r),
            CallError::Transport(t) => Error::Transport(t),
            CallError::Wire(w) => Error::Wire(w),
            CallError::Ctx(c) => Error::Ctx(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use dstore_transport::{CallError, TransportError};
    use dstore_wire::{RemoteError, WireError};

    use super::*;

    fn remote(code: &str, text: &str, view: &[u8]) -> RemoteError {
        RemoteError {
            code: code.to_owned(),
            text: text.to_owned(),
            view: view.to_vec(),
            retry_after: Duration::ZERO,
        }
    }

    #[test]
    fn call_errors_map_one_to_one() {
        let e = Error::from(CallError::Remote(remote("busy", "slow", &[])));
        assert!(matches!(e, Error::Remote(_)));
        assert_eq!(e.to_string(), "remote: busy: slow");
        let e = Error::from(CallError::Transport(TransportError::RecentlyUnreachable));
        assert!(matches!(
            e,
            Error::Transport(TransportError::RecentlyUnreachable)
        ));
        let e = Error::from(CallError::Wire(WireError::Eof));
        assert_eq!(e.to_string(), "EOF");
        let e = Error::from(CallError::Ctx(CtxError::DeadlineExceeded));
        assert_eq!(e.to_string(), "context deadline exceeded");
        assert_eq!(e.ctx_error(), Some(CtxError::DeadlineExceeded));
    }

    // wire_test.go TestErrorFrames: the error carries the view; IsCode and AsError see it.
    #[test]
    fn remote_errors_are_found_through_wrappers() {
        let e = Error::Remote(remote("stale-view", "request epoch is behind", &[1, 2, 3]));
        assert!(e.is_code("stale-view"));
        assert!(!e.is_code("busy"));
        assert_eq!(e.remote().map(|r| r.view.clone()), Some(vec![1, 2, 3]));

        let wrapped = Error::NoBootstrap(Box::new(Error::Wire(WireError::Remote(remote(
            "unavailable",
            "no view",
            &[],
        )))));
        assert_eq!(
            wrapped.to_string(),
            "client: no bootstrap node answered: remote: unavailable: no view"
        );
        assert!(wrapped.is_code("unavailable"));

        let upload = Error::UploadTo {
            node: "01000000".into(),
            source: Arc::new(Error::Remote(remote("busy", "x", &[]))),
        };
        assert_eq!(upload.to_string(), "upload to 01000000: remote: busy: x");
        assert!(upload.is_code("busy"));

        assert!(!Error::NoNodes.is_code("busy"));
        assert!(Error::Transport(TransportError::Closed).remote().is_none());
    }

    #[test]
    fn predicates_see_through_boxes_and_arcs() {
        assert!(Error::UnknownRef.is_unknown_ref());
        assert!(Error::NoBootstrap(Box::new(Error::UnknownRef)).is_unknown_ref());
        let arc = Error::Negotiate {
            key8: [0; 8],
            source: Arc::new(Error::UnknownRef),
        };
        assert!(arc.is_unknown_ref());
        assert!(!Error::Remote(remote("unknown-ref", "no such reference", &[])).is_unknown_ref());

        let cm = CasMismatch {
            current: vec![1, 2, 3],
            record: Vec::new(),
            version: Vec::new(),
            has_current: true,
        };
        let e = Error::NoBootstrap(Box::new(Error::CasMismatch(cm.clone())));
        assert_eq!(e.cas_mismatch(), Some(&cm));
        assert!(e.incomplete().is_none());

        let inc = Incomplete {
            sample: vec![[7; 32]],
            shortfall: 3,
        };
        let e = Error::Incomplete(inc.clone());
        assert_eq!(e.incomplete(), Some(&inc));
        assert_eq!(e.to_string(), "incomplete: 3 keys short");

        let ctx = Error::UploadTo {
            node: "02000000".into(),
            source: Arc::new(Error::Transport(TransportError::Ctx(CtxError::Canceled))),
        };
        assert_eq!(ctx.ctx_error(), Some(CtxError::Canceled));
        assert_eq!(Error::NoNodes.ctx_error(), None);
    }

    #[test]
    fn not_placed_last_error_wraps_only_the_short_error() {
        let e = Error::NotPlacedLastError {
            placed: Box::new(Error::NotPlaced {
                count: 5,
                names: "[03000000 (5 keys)]".into(),
            }),
            last: Arc::new(Error::Remote(remote("busy", "", &[]))),
        };
        assert_eq!(
            e.to_string(),
            "push: 5 keys could not be placed; owners not confirming: [03000000 (5 keys)]; last upload error: remote: busy"
        );
        // %v does not wrap: the last error is not part of the chain.
        assert!(!e.is_code("busy"));
    }
}
