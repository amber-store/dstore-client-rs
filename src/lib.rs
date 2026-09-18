//! The dstore client library: a compatible Rust port of the client side of Go dstore
//! (`github.com/amber-store/dstore` v0.1.9). This facade re-exports the workspace crates; PORTING.md holds
//! the compatibility contract.
#![deny(unsafe_op_in_unsafe_fn)]

pub use amber_store_core as core;
pub use dstore_client as client;
pub use dstore_codec as codec;
pub use dstore_gocompat as gocompat;
pub use dstore_ticket as ticket;
pub use dstore_transport as transport;
pub use dstore_transport_iroh as transport_iroh;
pub use dstore_udiff as udiff;
pub use dstore_view as view;
pub use dstore_wire as wire;
pub use dstore_worktree as worktree;
