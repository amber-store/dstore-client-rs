//! Go `context.Context` as [`Ctx`]: cancellation, deadlines that nest, and Go's error texts.

use std::sync::{Arc, OnceLock};

use tokio_util::sync::CancellationToken;

/// A Go `context.Context`.
#[derive(Clone)]
pub struct Ctx {
    token: CancellationToken,
    deadline: Option<tokio::time::Instant>,
    cause: Arc<OnceLock<CtxError>>,
    parent: Option<Box<Ctx>>,
}

/// `context.Canceled` / `context.DeadlineExceeded`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CtxError {
    #[error("context canceled")]
    Canceled,
    #[error("context deadline exceeded")]
    DeadlineExceeded,
}

impl Ctx {
    /// `context.Background()`.
    pub fn background() -> Ctx {
        todo!()
    }

    /// `context.WithCancel`: a child; parent cancellation propagates.
    pub fn with_cancel(&self) -> Ctx {
        todo!()
    }

    /// `context.WithTimeout`: a child whose deadline is min(parent deadline, now + d).
    pub fn with_timeout(&self, d: std::time::Duration) -> Ctx {
        todo!()
    }

    /// Cancels this ctx and its descendants only.
    pub fn cancel(&self) {
        todo!()
    }

    /// `Context.Deadline`.
    pub fn deadline(&self) -> Option<tokio::time::Instant> {
        todo!()
    }

    /// `Context.Err`: the cause of the first event (own cancel, own deadline, or the parent's err).
    pub fn err(&self) -> Option<CtxError> {
        todo!()
    }

    /// `<-Context.Done()`.
    pub async fn done(&self) {
        todo!()
    }

    /// Runs `f` until it completes or the ctx ends (`select { done, f }`).
    pub async fn run<F: std::future::Future>(&self, f: F) -> Result<F::Output, CtxError> {
        todo!()
    }

    /// Sleeps for `d` unless the ctx ends first.
    pub async fn sleep(&self, d: std::time::Duration) -> Result<(), CtxError> {
        todo!()
    }
}
