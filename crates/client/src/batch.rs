//! `client/batch.go`.

use std::sync::Arc;

/// The stored size of a record, for batching.
pub type RecordSizer = Arc<dyn Fn(&[u8; 32]) -> usize + Send + Sync>;

/// `batches`: splits `keys`, in order, into batches of at most `max_bytes` by the sizer and at most
/// `max_keys` keys. A record larger than `max_bytes` is a batch of its own.
pub(crate) fn batches(
    keys: &[[u8; 32]],
    size: &dyn Fn(&[u8; 32]) -> usize,
    max_bytes: usize,
    max_keys: usize,
) -> Vec<Vec<[u8; 32]>> {
    let mut out = Vec::new();
    let mut cur: Vec<[u8; 32]> = Vec::new();
    let mut bytes = 0usize;
    for k in keys {
        let n = size(k);
        if !cur.is_empty() && (bytes.saturating_add(n) > max_bytes || cur.len() >= max_keys) {
            out.push(std::mem::take(&mut cur));
            bytes = 0;
        }
        cur.push(*k);
        bytes = bytes.saturating_add(n);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use dstore_testkit::golden;
    use serde::Deserialize;

    use super::*;

    fn keys_n(n: usize) -> Vec<[u8; 32]> {
        (0..n)
            .map(|i| {
                let mut k = [0u8; 32];
                k[0] = (i + 1) as u8;
                k
            })
            .collect()
    }

    fn lens(b: &[Vec<[u8; 32]>]) -> Vec<usize> {
        b.iter().map(Vec::len).collect()
    }

    #[test]
    fn batches_balances_by_sizer_not_by_key_length() {
        let got = batches(&keys_n(5), &|_| 10, 25, 100);
        assert_eq!(lens(&got), [2, 2, 1]);
    }

    #[test]
    fn batches_caps_keys_per_batch() {
        let got = batches(&keys_n(7), &|_| 1, 1 << 20, 3);
        assert_eq!(lens(&got), [3, 3, 1]);
    }

    #[test]
    fn batches_sends_an_oversized_record_alone() {
        let sizes = |k: &[u8; 32]| match k[0] {
            1 => 5,
            2 => 100,
            3 => 5,
            _ => 0,
        };
        let got = batches(&keys_n(3), &sizes, 20, 100);
        assert_eq!(got.len(), 3, "got batch sizes {:?}", lens(&got));
    }

    #[test]
    fn batches_keeps_order() {
        let keys = keys_n(4);
        let got = batches(&keys, &|_| 1, 2, 100);
        assert_eq!(got, vec![vec![keys[0], keys[1]], vec![keys[2], keys[3]]]);
    }

    #[derive(Deserialize)]
    struct SizeRun {
        size: usize,
        count: usize,
    }

    #[derive(Deserialize)]
    struct Case {
        name: String,
        sizes: Vec<SizeRun>,
        max_bytes: usize,
        max_keys: usize,
        want_lens: Vec<usize>,
    }

    #[derive(Deserialize)]
    struct File {
        default_batch_bytes: usize,
        batch_keys: usize,
        max_put_batch: usize,
        cases: Vec<Case>,
    }

    #[test]
    fn golden_batches() {
        let f: File = golden::load_json("client/batches.json");
        assert_eq!(f.default_batch_bytes, crate::DEFAULT_BATCH_BYTES);
        assert_eq!(f.batch_keys, crate::BATCH_KEYS);
        assert_eq!(f.max_put_batch, dstore_wire::MAX_PUT_BATCH);
        assert!(f.cases.len() >= 4);
        for c in &f.cases {
            // Key i carries its position, so the sizer can look its size up.
            let sizes: Vec<usize> = c
                .sizes
                .iter()
                .flat_map(|r| std::iter::repeat_n(r.size, r.count))
                .collect();
            let keys: Vec<[u8; 32]> = (0..sizes.len())
                .map(|i| {
                    let mut k = [0u8; 32];
                    k[..8].copy_from_slice(&(i as u64).to_be_bytes());
                    k
                })
                .collect();
            let sizer = |k: &[u8; 32]| {
                let mut b = [0u8; 8];
                b.copy_from_slice(&k[..8]);
                sizes[u64::from_be_bytes(b) as usize]
            };
            let got = batches(&keys, &sizer, c.max_bytes, c.max_keys);
            assert_eq!(lens(&got), c.want_lens, "{}", c.name);
            let flat: Vec<[u8; 32]> = got.into_iter().flatten().collect();
            assert_eq!(flat, keys, "{}: order", c.name);
        }
    }
}
