//! Client errors with Go's texts (client-core §2.14, client-transfer §3.4) (part A).

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
    #[error("{placed}; last upload error: {last}")]
    NotPlacedLastError {
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

impl Error {
    /// `errors.As` through the source chain.
    pub fn remote(&self) -> Option<&dstore_wire::RemoteError> {
        todo!()
    }

    pub fn is_code(&self, code: &str) -> bool {
        todo!()
    }

    pub fn cas_mismatch(&self) -> Option<&CasMismatch> {
        todo!()
    }

    pub fn incomplete(&self) -> Option<&Incomplete> {
        todo!()
    }

    pub fn is_unknown_ref(&self) -> bool {
        todo!()
    }

    pub fn ctx_error(&self) -> Option<CtxError> {
        todo!()
    }
}

/// Remote/Transport/Wire/Ctx 1:1.
impl From<dstore_transport::CallError> for Error {
    fn from(e: dstore_transport::CallError) -> Error {
        todo!()
    }
}
