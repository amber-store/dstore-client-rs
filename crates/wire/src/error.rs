//! `wire.Error` (remote errors), `wire.ErrorFrom`, `wire.ErrMsg`, and the `errors.As` walk.

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
        todo!()
    }

    /// `(*wire.Error).Is`: same code; target text empty or equal.
    pub fn is(&self, target: &RemoteError) -> bool {
        todo!()
    }
}

/// `wire.ErrorFrom`: retry_after = `m.retry_after` ms (negative → ZERO).
pub fn error_from_msg(m: &Msg) -> RemoteError {
    todo!()
}

/// `wire.ErrMsg`: `{0:10, 10:code, 11:text}`, never stamped.
pub fn err_msg(code: &str, text: &str) -> Msg {
    todo!()
}

/// Go `errors.As(err, **wire.Error)`: walks `err`, then its `source()` chain.
pub fn as_remote<'a>(err: &'a (dyn std::error::Error + 'static)) -> Option<&'a RemoteError> {
    todo!()
}

/// `wire.IsCode`.
pub fn is_code(err: &(dyn std::error::Error + 'static), code: &str) -> bool {
    todo!()
}
