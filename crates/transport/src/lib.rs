//! dstore `transport/transport.go` (traits, `Pool`) and `transport/mem.go` (in-memory network), plus
//! `ParseAddrs` (`transport/iroh.go:247-261`) and go-iroh `netaddr` parse/format rules.
//!
//! Spec: PORTING.md §4.6; port-notes/transport.md §2.1-2.2, §2.4, §2.9, §4.3-4.4, §4.7, §4.9.
#![allow(dead_code, unused_variables)] // L0 stubs: remove once the modules are implemented
#![deny(unsafe_op_in_unsafe_fn)]

pub use dstore_gocompat::ctx::{Ctx, CtxError};
pub use dstore_view::NodeId;

pub mod addr;
mod error;
pub mod mem;
mod pool;
mod traits;

pub use error::*;
pub use pool::*;
pub use traits::*;
