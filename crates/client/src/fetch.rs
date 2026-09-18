//! `client/fetch.go`: the fetcher behind `Cluster::get` (part B, internal).

/// `estSize`: 46 + min(length, 64 KiB).
pub(crate) fn est_size(k: &[u8; 32]) -> usize {
    todo!()
}
