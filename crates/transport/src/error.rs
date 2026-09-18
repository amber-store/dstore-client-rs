//! Transport errors with Go's wrapper texts, and `Pool::call` errors.

use crate::CtxError;

/// Transport errors. The wrapper texts are Go's; inner QUIC texts differ (DD-4).
#[derive(Debug, Clone, thiserror::Error)]
pub enum TransportError {
    #[error("transport: closed")]
    Closed,
    #[error("transport: peer recently unreachable")]
    RecentlyUnreachable,
    /// 10 hex chars (go-iroh `Short`).
    #[error("transport: no candidate addresses for {0}")]
    NoCandidates(String),
    #[error("transport: bind: {0}")]
    Bind(String),
    #[error("transport: pkarr publisher: {0}")]
    PkarrPublisher(String),
    #[error("dial {addr}: {source}")]
    DialAddr {
        addr: String,
        source: Box<TransportError>,
    },
    #[error("discovery: {0}")]
    Discovery(#[source] Box<TransportError>),
    /// `errors.Join`: "\n"-separated.
    #[error("{}", joined(.0))]
    Joined(Vec<TransportError>),
    #[error("data is not a valid public key")]
    InvalidKey,
    #[error("iroh: no reachable address for endpoint")]
    NoAddress,
    #[error("iroh: cannot connect to self")]
    SelfConnect,
    #[error("{0}")]
    Ctx(#[from] CtxError),
    #[error("mem: local endpoint is down")]
    MemLocalDown,
    #[error("mem: {0} unreachable")]
    MemUnreachable(String),
    #[error("mem: {0} not bound")]
    MemNotBound(String),
    #[error("mem: {0} does not speak {1}")]
    MemAlpn(String, String),
    /// iroh/noq inner text (DD-4).
    #[error("{0}")]
    Quic(String),
}

/// The `errors.Join` layout of the joined errors.
fn joined(errs: &[TransportError]) -> String {
    todo!()
}

/// `Pool::call` errors.
#[derive(Debug, thiserror::Error)]
pub enum CallError {
    #[error("{0}")]
    Transport(#[source] TransportError),
    #[error("{0}")]
    Wire(#[source] dstore_wire::WireError),
    #[error("{0}")]
    Ctx(#[source] CtxError),
    #[error("{0}")]
    Remote(#[source] dstore_wire::RemoteError),
}
