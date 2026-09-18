//! dstore `worktree` (`tree.go`, `change.go`, `scan.go`, `merge.go`, `apply.go`, `diff.go`, `flow.go`,
//! `xattr*.go`). `TicketFromView` lives in `dstore-view` and is re-exported here.
//!
//! Paths and `Config` fields are bytes. Scan, `diff_trees`, apply, ingest and `hash_file` are blocking;
//! callers wrap them in `spawn_blocking`.
//!
//! Spec: PORTING.md §4.10; port-notes/worktree.md.
#![allow(dead_code, unused_variables)] // L0 stubs: remove once the modules are implemented
#![deny(unsafe_op_in_unsafe_fn)]

pub use dstore_view::ticket_from_view;

/// The working-copy metadata directory.
pub const DIR: &str = ".dstore";
pub const RACY_WINDOW_NS: i64 = 2_000_000_000;
pub const MAX_DIFF_BYTES: u64 = 16 << 20;

mod apply;
mod change;
mod diff;
mod error;
mod flow;
mod merge;
mod scan;
pub mod sys;
mod tree;

pub use apply::*;
pub use change::*;
pub use diff::*;
pub use error::*;
pub use flow::*;
pub use merge::*;
pub use scan::*;
pub use tree::*;
