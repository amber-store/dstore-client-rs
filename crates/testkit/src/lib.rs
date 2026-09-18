//! Test support for dstore-client-rs (not published): splitmix64 streams, golden-vector loaders,
//! `refglob` for the fake node, and a fake dstore cluster over `dstore_transport::mem`.
//!
//! Spec: PORTING.md §4.13, §7; port-notes/verification.md §4.3-§4.6.
#![deny(unsafe_op_in_unsafe_fn)]

pub mod fake;
pub mod golden;
pub mod refglob;
pub mod splitmix;
