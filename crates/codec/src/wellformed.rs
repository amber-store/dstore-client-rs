//! Pass 1: well-formedness with fxamacker's limits and texts (`valid.go`).

use crate::DecodeError;

/// fxamacker `Valid` as `DecOptions{}` configures it: max nesting 32, 131072 array elements and map
/// pairs, extraneous-data check.
pub fn well_formed(b: &[u8]) -> Result<(), DecodeError> {
    todo!()
}
