//! splitmix64 streams as defined in core-rs VECTORS.md ("Deterministic data streams"), used for vector
//! payloads (VECTORS.md).

/// The splitmix64 generator; the field is the state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SplitMix64(pub u64);

impl SplitMix64 {
    /// `next(state)`.
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// `data(seed, n)`: `next` outputs appended as 8 little-endian bytes each, truncated to `n` bytes.
pub fn data(seed: u64, n: usize) -> Vec<u8> {
    let mut rng = SplitMix64(seed);
    let mut out = Vec::with_capacity(n.saturating_add(8));
    while out.len() < n {
        out.extend_from_slice(&rng.next_u64().to_le_bytes());
    }
    out.truncate(n);
    out
}

/// `u64s(seed, n)`: the first `n` outputs of `next`.
pub fn u64s(seed: u64, n: usize) -> Vec<u64> {
    let mut rng = SplitMix64(seed);
    (0..n).map(|_| rng.next_u64()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Reference values from the Go algorithm of tools/vectorgen/util.go, run with go1.26.5.
    #[test]
    fn outputs_match_go() {
        assert_eq!(
            u64s(0, 3),
            [
                0xe220_a839_7b1d_cdaf,
                0x6e78_9e6a_a1b9_65f4,
                0x06c4_5d18_8009_454f
            ]
        );
        assert_eq!(
            u64s(42, 3),
            [
                0xbdd7_3226_2feb_6e95,
                0x28ef_e333_b266_f103,
                0x4752_6757_130f_9f52
            ]
        );
        assert_eq!(
            u64s(u64::MAX, 3),
            [
                0xe4d9_7177_1b65_2c20,
                0xe99f_f867_dbf6_82c9,
                0x382f_f84c_b272_81e9
            ]
        );
    }

    #[test]
    fn data_is_little_endian_and_truncated() {
        assert_eq!(
            ::hex::encode(data(1, 20)),
            "c15c0289ec2d0a9167ec8e65a18debbe5e5532fb"
        );
        assert_eq!(::hex::encode(data(7, 3)), "d70d32");
        assert!(data(9, 0).is_empty());
        let first = SplitMix64(5).next_u64().to_le_bytes();
        assert_eq!(data(5, 8), first);
    }

    #[test]
    fn generator_state_advances() {
        let mut rng = SplitMix64(0);
        let a = rng.next_u64();
        assert_eq!(rng.0, 0x9E37_79B9_7F4A_7C15);
        assert_eq!(a, u64s(0, 1)[0]);
    }
}
