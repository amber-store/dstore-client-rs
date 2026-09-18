//! dstore `wire` (messages, frames, remote errors, key lists), transport-iroh v0.4.0 `protocol` pack
//! framing, and the payload types of `node/admin.go` and `node/status.go`.
//!
//! Go `wire.CloseStream` is `dstore_transport::Stream::close_stream`.
//!
//! Spec: PORTING.md §4.3; port-notes/codec-wire-ticket.md §2.2, §2.3, §2.5, §2.6, §3.
#![allow(dead_code, unused_variables)] // L0 stubs: remove once the modules are implemented
#![deny(unsafe_op_in_unsafe_fn)]

pub mod admin;
pub mod consts;
pub mod error;
pub mod frame;
pub mod keys;
pub mod msg;
pub mod pack;

pub use admin::*;
pub use consts::*;
pub use error::*;
pub use frame::*;
pub use keys::*;
pub use msg::*;
pub use pack::*;
