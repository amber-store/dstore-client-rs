//! `client/batch.go`.

use std::sync::Arc;

/// The stored size of a record, for batching.
pub type RecordSizer = Arc<dyn Fn(&[u8; 32]) -> usize + Send + Sync>;

/// `batches`: cut `keys` into batches of at most `max_bytes` and `max_keys`.
pub(crate) fn batches(
    keys: &[[u8; 32]],
    size: &dyn Fn(&[u8; 32]) -> usize,
    max_bytes: usize,
    max_keys: usize,
) -> Vec<Vec<[u8; 32]>> {
    todo!()
}
