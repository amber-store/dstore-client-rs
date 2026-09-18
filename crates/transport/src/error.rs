//! Transport errors with Go's wrapper texts, and `Pool::call` errors.

use std::fmt::Write as _;

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

/// The `errors.Join` layout of the joined errors: each error's text, separated by "\n". Go's `Join` drops
/// nil errors when it builds the list, so every element here is printed.
fn joined(errs: &[TransportError]) -> String {
    let mut out = String::new();
    for (i, e) in errs.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let _ = write!(out, "{e}");
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    use std::error::Error as _;

    fn dial(addr: &str, inner: &str) -> TransportError {
        TransportError::DialAddr {
            addr: addr.to_string(),
            source: Box::new(TransportError::Quic(inner.to_string())),
        }
    }

    // transport §3.4 and §5.7.
    #[test]
    fn go_texts() {
        let cases = [
            (TransportError::Closed, "transport: closed"),
            (
                TransportError::RecentlyUnreachable,
                "transport: peer recently unreachable",
            ),
            (
                TransportError::NoCandidates("79b5562e8f".to_string()),
                "transport: no candidate addresses for 79b5562e8f",
            ),
            (TransportError::Bind("x".to_string()), "transport: bind: x"),
            (
                TransportError::PkarrPublisher("x".to_string()),
                "transport: pkarr publisher: x",
            ),
            (dial("ip:127.0.0.1:9", "x"), "dial ip:127.0.0.1:9: x"),
            (
                TransportError::Discovery(Box::new(TransportError::NoAddress)),
                "discovery: iroh: no reachable address for endpoint",
            ),
            (
                TransportError::Joined(vec![
                    dial("ip:127.0.0.1:9", "x"),
                    TransportError::Discovery(Box::new(TransportError::Quic("y".to_string()))),
                ]),
                "dial ip:127.0.0.1:9: x\ndiscovery: y",
            ),
            (
                TransportError::Joined(vec![dial("ip:127.0.0.1:9", "x")]),
                "dial ip:127.0.0.1:9: x",
            ),
            (TransportError::InvalidKey, "data is not a valid public key"),
            (
                TransportError::NoAddress,
                "iroh: no reachable address for endpoint",
            ),
            (TransportError::SelfConnect, "iroh: cannot connect to self"),
            (
                TransportError::Ctx(CtxError::DeadlineExceeded),
                "context deadline exceeded",
            ),
            (TransportError::Ctx(CtxError::Canceled), "context canceled"),
            (TransportError::MemLocalDown, "mem: local endpoint is down"),
            (
                TransportError::MemUnreachable("62297a9a".to_string()),
                "mem: 62297a9a unreachable",
            ),
            (
                TransportError::MemNotBound("5cb5f57e".to_string()),
                "mem: 5cb5f57e not bound",
            ),
            (
                TransportError::MemAlpn("b44b49be".to_string(), "amber-dstore/1".to_string()),
                "mem: b44b49be does not speak amber-dstore/1",
            ),
            (TransportError::Quic("boom".to_string()), "boom"),
        ];
        for (err, want) in cases {
            assert_eq!(err.to_string(), want);
        }
    }

    #[test]
    fn wrappers_expose_their_inner_error() {
        let d = dial("ip:127.0.0.1:9", "x");
        assert_eq!(d.source().map(|s| s.to_string()), Some("x".to_string()));
        let disc = TransportError::Discovery(Box::new(TransportError::NoAddress));
        assert_eq!(
            disc.source().map(|s| s.to_string()),
            Some("iroh: no reachable address for endpoint".to_string())
        );
        let e: TransportError = CtxError::Canceled.into();
        assert!(matches!(e, TransportError::Ctx(CtxError::Canceled)));
        let c = CallError::Transport(TransportError::Closed);
        assert_eq!(c.to_string(), "transport: closed");
        assert_eq!(
            c.source().map(|s| s.to_string()),
            Some("transport: closed".to_string())
        );
        assert_eq!(
            CallError::Ctx(CtxError::DeadlineExceeded).to_string(),
            "context deadline exceeded"
        );
    }
}
