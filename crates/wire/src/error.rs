//! `wire.Error` (remote errors), `wire.ErrorFromMsg`, `wire.ErrMsg`, and the `errors.As` walk
//! (`wire/wire.go:287-334`).

use crate::consts::T_ERR;
use crate::msg::Msg;

/// `*wire.Error`: a node's answer.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{}", self.message())]
pub struct RemoteError {
    pub code: String,
    pub text: String,
    pub view: Vec<u8>,
    pub retry_after: std::time::Duration,
}

impl RemoteError {
    /// "remote: <code>" or "remote: <code>: <text>".
    pub fn message(&self) -> String {
        if self.text.is_empty() {
            format!("remote: {}", self.code)
        } else {
            format!("remote: {}: {}", self.code, self.text)
        }
    }

    /// `(*wire.Error).Is`: same code; target text empty or equal.
    pub fn is(&self, target: &RemoteError) -> bool {
        target.code == self.code && (target.text.is_empty() || target.text == self.text)
    }
}

/// `wire.ErrorFromMsg`: retry_after = `m.retry_after` ms (negative → ZERO).
///
/// Go computes `time.Duration(m.RetryAfter) * time.Millisecond`, a wrapping `int64` product. The same
/// wrapping product is taken here, so every non-negative Go duration is reproduced exactly; a negative
/// one becomes ZERO, which every caller treats as Go treats a negative duration (`RetryAfter > 0` fails).
pub fn error_from_msg(m: &Msg) -> RemoteError {
    let ns = m.retry_after.wrapping_mul(1_000_000);
    let retry_after = u64::try_from(ns)
        .map(std::time::Duration::from_nanos)
        .unwrap_or_default();
    RemoteError {
        code: m.code.clone(),
        text: m.text.clone(),
        view: m.view.clone(),
        retry_after,
    }
}

/// `wire.ErrMsg`: `{0:10, 10:code, 11:text}`, never stamped.
pub fn err_msg(code: &str, text: &str) -> Msg {
    Msg {
        typ: T_ERR,
        code: code.to_owned(),
        text: text.to_owned(),
        ..Msg::default()
    }
}

/// Go `errors.As(err, **wire.Error)`: walks `err`, then its `source()` chain.
///
/// A `Box<RemoteError>` or `Arc<RemoteError>` in the chain matches too, as a `*wire.Error` pointer does in Go:
/// std's `Error` impls for `Box<T>` and `Arc<T>` forward `source()` to `T::source()` and so hide `T` itself
/// from a plain `source()` walk.
pub fn as_remote<'a>(err: &'a (dyn std::error::Error + 'static)) -> Option<&'a RemoteError> {
    let mut cur = Some(err);
    while let Some(e) = cur {
        if let Some(remote) = remote_of(e) {
            return Some(remote);
        }
        cur = e.source();
    }
    None
}

/// `e` itself as a `RemoteError`, held directly or behind a `Box` or `Arc`.
fn remote_of<'a>(e: &'a (dyn std::error::Error + 'static)) -> Option<&'a RemoteError> {
    if let Some(remote) = e.downcast_ref::<RemoteError>() {
        return Some(remote);
    }
    if let Some(remote) = e.downcast_ref::<Box<RemoteError>>() {
        return Some(&**remote);
    }
    e.downcast_ref::<std::sync::Arc<RemoteError>>()
        .map(|remote| &**remote)
}

/// `wire.IsCode`.
pub fn is_code(err: &(dyn std::error::Error + 'static), code: &str) -> bool {
    as_remote(err).is_some_and(|e| e.code == code)
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::fmt;
    use std::time::Duration;

    use super::*;
    use crate::frame::WireError;
    use crate::pack::ProtocolRemoteError;

    fn remote(code: &str, text: &str) -> RemoteError {
        RemoteError {
            code: code.to_owned(),
            text: text.to_owned(),
            view: Vec::new(),
            retry_after: Duration::ZERO,
        }
    }

    /// `fmt.Errorf("<prefix>: %w", inner)`.
    #[derive(Debug)]
    struct Wrapped {
        prefix: &'static str,
        inner: Box<dyn Error + Send + Sync + 'static>,
    }

    impl fmt::Display for Wrapped {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{}: {}", self.prefix, self.inner)
        }
    }

    impl Error for Wrapped {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            Some(&*self.inner)
        }
    }

    #[test]
    fn message_probes() {
        assert_eq!(remote("busy", "").to_string(), "remote: busy");
        assert_eq!(remote("busy", "t").to_string(), "remote: busy: t");
        assert_eq!(remote("busy", "t").message(), "remote: busy: t");
    }

    #[test]
    fn is_matches_code_and_optional_text() {
        let e = remote("busy", "slow");
        assert!(e.is(&remote("busy", "")));
        assert!(e.is(&remote("busy", "slow")));
        assert!(!e.is(&remote("busy", "fast")));
        assert!(!e.is(&remote("no-space", "")));
        assert!(!remote("busy", "").is(&remote("busy", "slow")));
    }

    #[test]
    fn error_from_msg_copies_the_frame() {
        let m = Msg {
            typ: T_ERR,
            code: "busy".into(),
            text: "slow down".into(),
            view: vec![1, 2],
            retry_after: 1500,
            ..Msg::default()
        };
        let e = error_from_msg(&m);
        assert_eq!(e.code, "busy");
        assert_eq!(e.text, "slow down");
        assert_eq!(e.view, [1, 2]);
        assert_eq!(e.retry_after, Duration::from_millis(1500));
    }

    #[test]
    fn error_from_msg_retry_after_edges() {
        let with = |ms: i64| {
            error_from_msg(&Msg {
                typ: T_ERR,
                retry_after: ms,
                ..Msg::default()
            })
            .retry_after
        };
        assert_eq!(with(0), Duration::ZERO);
        assert_eq!(with(-5), Duration::ZERO);
        assert_eq!(with(i64::MIN), Duration::ZERO);
        // The largest product without wrapping.
        assert_eq!(
            with(9_223_372_036_854),
            Duration::from_nanos(9_223_372_036_854_000_000)
        );
        // 9223372036855000000 wraps to a negative Go duration.
        assert_eq!(with(9_223_372_036_855), Duration::ZERO);
        // 18446744073710000000 - 2^64 = 448384: Go's product wraps to a positive 448.384µs.
        assert_eq!(with(18_446_744_073_710), Duration::from_nanos(448_384));
    }

    #[test]
    fn err_msg_frame_is_unstamped() {
        let m = err_msg(crate::CODE_UNAUTHORIZED, "not on the allowlist");
        assert_eq!(m.typ, T_ERR);
        let payload = dstore_codec::marshal(&m);
        // codec-wire-ticket §3.3 "err unauthorized (WriteErr, unstamped)".
        let mut want = vec![0xa3, 0x00, 0x0a, 0x0a, 0x6c];
        want.extend_from_slice(b"unauthorized");
        want.extend_from_slice(&[0x0b, 0x74]);
        want.extend_from_slice(b"not on the allowlist");
        assert_eq!(payload, want);
    }

    #[test]
    fn as_remote_walks_the_source_chain() {
        let direct = remote("stale-view", "behind");
        assert_eq!(as_remote(&direct), Some(&direct));

        let wire = WireError::Remote(remote("busy", ""));
        assert_eq!(as_remote(&wire).map(|e| e.code.as_str()), Some("busy"));

        let wrapped = Wrapped {
            prefix: "upload to 01000000",
            inner: Box::new(Wrapped {
                prefix: "get",
                inner: Box::new(WireError::Remote(remote(
                    "unknown-ref",
                    "no such reference",
                ))),
            }),
        };
        assert_eq!(
            wrapped.to_string(),
            "upload to 01000000: get: remote: unknown-ref: no such reference"
        );
        assert!(is_code(&wrapped, crate::CODE_UNKNOWN_REF));
        assert!(!is_code(&wrapped, crate::CODE_BUSY));

        let protocol = WireError::Protocol { got: 53, want: 52 };
        assert!(as_remote(&protocol).is_none());
    }

    /// `#[source] Arc<T>` / `Box<T>` fields (client errors keep `Arc<Error>`): std forwards `source()` through the
    /// pointer, so a remote error held behind one is still found.
    #[test]
    fn as_remote_sees_through_box_and_arc() {
        #[derive(Debug)]
        struct Held<P: Error + 'static>(P);

        impl<P: Error + 'static> fmt::Display for Held<P> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "held: {}", self.0)
            }
        }

        impl<P: Error + 'static> Error for Held<P> {
            fn source(&self) -> Option<&(dyn Error + 'static)> {
                Some(&self.0)
            }
        }

        let boxed = Held(Box::new(remote("busy", "b")));
        assert_eq!(as_remote(&boxed).map(|e| e.text.as_str()), Some("b"));
        assert!(is_code(&boxed, crate::CODE_BUSY));

        let arced = Held(std::sync::Arc::new(remote("stale-view", "a")));
        assert_eq!(as_remote(&arced).map(|e| e.text.as_str()), Some("a"));
        assert!(is_code(&arced, crate::CODE_STALE_VIEW));

        // An Arc'd wrapper whose own source is the remote error (dstore_client::Error::UploadTo).
        let shared = Held(std::sync::Arc::new(WireError::Remote(remote(
            "no-space", "",
        ))));
        assert!(is_code(&shared, crate::CODE_NO_SPACE));

        let unrelated = Held(std::sync::Arc::new(WireError::Protocol { got: 1, want: 2 }));
        assert!(as_remote(&unrelated).is_none());
    }

    #[test]
    fn a_protocol_remote_error_never_matches() {
        let pe = ProtocolRemoteError {
            code: "busy".into(),
            text: String::new(),
            current: vec![1, 2],
        };
        assert_eq!(pe.to_string(), "remote: busy: ");
        assert!(!is_code(&pe, "busy"));
        let wrapped = Wrapped {
            prefix: "get",
            inner: Box::new(pe),
        };
        assert!(as_remote(&wrapped).is_none());
    }
}
