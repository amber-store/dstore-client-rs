//! `wire.Keys32` and `wire.RawKeys` (also Go `RawIDs`), `wire/wire.go:364-386`.

use crate::frame::WireError;

/// `wire.Keys32`: `KeyLen` for the first bad entry.
///
/// Go callers ignore the error (`lacking, _ := wire.Keys32(resp.Keys)`, `client/objects.go:108`), so a
/// malformed reply reads as an empty list there; mirror that at the call site.
pub fn keys32(raw: &[Vec<u8>]) -> Result<Vec<[u8; 32]>, WireError> {
    let mut out = Vec::with_capacity(raw.len());
    for (index, b) in raw.iter().enumerate() {
        let key = <[u8; 32]>::try_from(b.as_slice()).map_err(|_| WireError::KeyLen {
            index,
            len: b.len(),
        })?;
        out.push(key);
    }
    Ok(out)
}

/// `wire.RawKeys` (also Go `RawIDs`).
pub fn raw_keys(keys: &[[u8; 32]]) -> Vec<Vec<u8>> {
    keys.iter().map(|k| k.to_vec()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys32_accepts_32_byte_entries() {
        assert_eq!(keys32(&[]).expect("empty"), Vec::<[u8; 32]>::new());
        let raw = vec![vec![1u8; 32], vec![2u8; 32]];
        assert_eq!(keys32(&raw).expect("two"), [[1u8; 32], [2u8; 32]]);
    }

    #[test]
    fn keys32_reports_the_first_bad_entry() {
        let cases: &[(Vec<Vec<u8>>, &str)] = &[
            (vec![vec![1; 32], vec![2; 31]], "wire: key 1 has 31 bytes"),
            (vec![vec![1; 33]], "wire: key 0 has 33 bytes"),
            (vec![vec![]], "wire: key 0 has 0 bytes"),
            (
                vec![vec![1; 32], vec![], vec![3; 31]],
                "wire: key 1 has 0 bytes",
            ),
        ];
        for (raw, want) in cases {
            match keys32(raw) {
                Ok(keys) => panic!("{want}: accepted {keys:?}"),
                Err(e) => assert_eq!(e.to_string(), *want),
            }
        }
    }

    #[test]
    fn raw_keys_round_trips() {
        let keys = [[7u8; 32], [9u8; 32]];
        let raw = raw_keys(&keys);
        assert_eq!(raw, [vec![7u8; 32], vec![9u8; 32]]);
        assert_eq!(keys32(&raw).expect("round trip"), keys);
        assert!(raw_keys(&[]).is_empty());
    }
}
