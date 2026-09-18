//! Golden-vector loaders (PORTING.md §7). Vectors live under `<workspace>/tests/golden`, are generated
//! by `tools/vectorgen` from the Go implementation, and follow the conventions of VECTORS.md.

use std::path::{Path, PathBuf};

use serde::de::{DeserializeOwned, Error as _};
use serde::{Deserialize, Deserializer};

/// `<workspace>/tests/golden`.
pub fn golden_dir() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    // This crate lives in <workspace>/crates/testkit.
    match manifest.parent().and_then(Path::parent) {
        Some(root) => root.join("tests").join("golden"),
        None => manifest.join("../../tests/golden"),
    }
}

/// Loads and parses `<golden_dir>/<rel>`. Panics, failing the test, when the file is missing or does not
/// parse: vectors are committed, so a missing file is a failure, never a skip.
pub fn load_json<T: DeserializeOwned>(rel: &str) -> T {
    let path = golden_dir().join(rel);
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => panic!(
            "golden vector {} is missing: {e} (regenerate with tools/vectorgen, see VECTORS.md)",
            path.display()
        ),
    };
    match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(e) => panic!("golden vector {} does not parse: {e}", path.display()),
    }
}

/// Decodes a hex string of a vector. Panics on invalid hex.
pub fn hex(s: &str) -> Vec<u8> {
    match ::hex::decode(s) {
        Ok(b) => b,
        Err(e) => panic!("golden vector: invalid hex {s:?}: {e}"),
    }
}

/// A payload description: `{"hex": "…"}` for inline bytes, or `{"seed": S, "len": N}` for
/// `splitmix::data(S, N)`. The seed may be a decimal string (the vectors' form for 64-bit integers) or a
/// JSON number.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(untagged)]
pub enum Payload {
    Inline {
        hex: String,
    },
    Splitmix {
        #[serde(deserialize_with = "decimal_u64")]
        seed: u64,
        len: usize,
    },
}

impl Payload {
    /// The payload's bytes. Panics on invalid inline hex.
    pub fn bytes(&self) -> Vec<u8> {
        match self {
            Payload::Inline { hex: h } => hex(h),
            Payload::Splitmix { seed, len } => crate::splitmix::data(*seed, *len),
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum DecimalU64 {
    Num(u64),
    Str(String),
}

#[derive(Deserialize)]
#[serde(untagged)]
enum DecimalI64 {
    Num(i64),
    Str(String),
}

/// serde `deserialize_with` for a `u64` written as a decimal string (or a JSON number).
pub fn decimal_u64<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    match DecimalU64::deserialize(d)? {
        DecimalU64::Num(n) => Ok(n),
        DecimalU64::Str(s) => s.parse().map_err(D::Error::custom),
    }
}

/// serde `deserialize_with` for an `i64` written as a decimal string (or a JSON number).
pub fn decimal_i64<'de, D: Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
    match DecimalI64::deserialize(d)? {
        DecimalI64::Num(n) => Ok(n),
        DecimalI64::Str(s) => s.parse().map_err(D::Error::custom),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn golden_dir_is_the_workspace_vector_dir() {
        let dir = golden_dir();
        assert!(dir.ends_with("tests/golden"), "{}", dir.display());
        assert!(dir.is_dir(), "{} is not a directory", dir.display());
    }

    #[test]
    #[should_panic(expected = "is missing")]
    fn load_json_fails_on_a_missing_file() {
        let _: serde_json::Value = load_json("no-such-family/missing.json");
    }

    #[test]
    fn hex_decodes_lowercase() {
        assert_eq!(hex("00ff7a"), [0x00, 0xff, 0x7a]);
        assert!(hex("").is_empty());
    }

    #[test]
    #[should_panic(expected = "invalid hex")]
    fn hex_fails_on_invalid_input() {
        hex("0g");
    }

    #[test]
    fn payload_forms() {
        let json = r#"[
            {"hex": "00ff"},
            {"seed": "1", "len": 20},
            {"seed": 7, "len": 3},
            {"seed": "18446744073709551615", "len": 0}
        ]"#;
        let payloads: Vec<Payload> = match serde_json::from_str(json) {
            Ok(p) => p,
            Err(e) => panic!("payloads do not parse: {e}"),
        };
        assert_eq!(payloads[0].bytes(), [0x00, 0xff]);
        assert_eq!(
            ::hex::encode(payloads[1].bytes()),
            "c15c0289ec2d0a9167ec8e65a18debbe5e5532fb"
        );
        assert_eq!(::hex::encode(payloads[2].bytes()), "d70d32");
        assert!(matches!(
            payloads[3],
            Payload::Splitmix {
                seed: u64::MAX,
                len: 0
            }
        ));
    }

    #[test]
    fn payload_rejects_a_bad_seed() {
        assert!(serde_json::from_str::<Payload>(r#"{"seed": "-1", "len": 1}"#).is_err());
        assert!(serde_json::from_str::<Payload>(r#"{"len": 1}"#).is_err());
    }

    #[test]
    fn decimal_helpers() {
        #[derive(Deserialize)]
        struct Row {
            #[serde(deserialize_with = "decimal_u64")]
            u: u64,
            #[serde(deserialize_with = "decimal_i64")]
            i: i64,
        }
        let row: Row = match serde_json::from_str(
            r#"{"u": "18446744073709551615", "i": "-9223372036854775808"}"#,
        ) {
            Ok(r) => r,
            Err(e) => panic!("row does not parse: {e}"),
        };
        assert_eq!((row.u, row.i), (u64::MAX, i64::MIN));
        let row: Row = match serde_json::from_str(r#"{"u": 5, "i": -5}"#) {
            Ok(r) => r,
            Err(e) => panic!("row does not parse: {e}"),
        };
        assert_eq!((row.u, row.i), (5, -5));
    }
}
