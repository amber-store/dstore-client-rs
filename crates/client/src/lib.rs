//! dstore `client` (`client.go`, `rank.go`, `batch.go`, `progress.go`, `refs.go`, `watch.go`,
//! `objects.go`, `fetch.go`, `tree.go`, `commit.go`).
//!
//! `Cluster` requires a multi-thread tokio runtime.
//!
//! Module ownership: part A `cluster`, `rank`, `batch`, `progress`, `refs`, `watch`, `error`, `corefmt`;
//! part B `objects`, `fetch`, `tree`, coding against the `pub(crate)` seams of part A.
//!
//! Spec: PORTING.md §4.8; port-notes/client-core.md, port-notes/client-transfer.md.
#![deny(unsafe_op_in_unsafe_fn)]

pub use dstore_gocompat::ctx::{Ctx, CtxError};
pub use dstore_gocompat::slog::Logger;
pub use dstore_view::NodeId;

mod batch;
mod cluster;
mod commit;
pub mod corefmt;
mod error;
mod fetch;
mod objects;
mod progress;
mod rank;
mod refs;
mod tree;
mod watch;

pub use batch::*;
pub use cluster::*;
pub use commit::*;
pub use error::*;
pub use objects::*;
pub use progress::*;
pub use refs::*;
pub use tree::*;
pub use watch::*;

pub const DEFAULT_BATCH_BYTES: usize = 16 << 20;
pub const BATCH_KEYS: usize = 8192;
pub const GET_BATCH_KEYS: usize = 2048;
pub const GET_BATCH_BYTES: usize = 8 << 20;
pub const GET_EST_MAX: u64 = 64 << 10;
pub const PULL_WRITE_BYTES: usize = 16 << 20;
pub const DIAL_MEMBER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
pub const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);
