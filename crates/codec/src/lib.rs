//! CBOR as dstore configures fxamacker/cbor v2.9.3 (dstore `codec/codec.go`):
//! `CanonicalEncOptions().EncMode()` for encoding and `DecOptions{}.DecMode()` for decoding, with
//! fxamacker's limits, laxness and verbatim error texts.
//!
//! Hand-rolled in two passes: a well-formedness check, then per-field decoding. Keyasint structs are
//! declared with [`cbor_struct!`].
//!
//! Spec: PORTING.md §4.2, §5.7; port-notes/codec-wire-ticket.md §2.1, §4.2-§4.4.
#![deny(unsafe_op_in_unsafe_fn)]

mod dec;
mod enc;
mod error;
mod field;
mod macros;
mod wellformed;

pub use dec::*;
pub use enc::*;
pub use error::*;
pub use field::*;
pub use wellformed::*;
