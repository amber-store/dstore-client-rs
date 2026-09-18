//! `wire.Keys32` and `wire.RawKeys` (also Go `RawIDs`).

use crate::frame::WireError;

/// `wire.Keys32`: `KeyLen` for the first bad entry.
pub fn keys32(raw: &[Vec<u8>]) -> Result<Vec<[u8; 32]>, WireError> {
    todo!()
}

/// `wire.RawKeys` (also Go `RawIDs`).
pub fn raw_keys(keys: &[[u8; 32]]) -> Vec<Vec<u8>> {
    todo!()
}
