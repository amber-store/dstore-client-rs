//! `wire.Msg` and its sub-structs (`wire/wire.go:131-240`). Every `Msg` field is omitempty except
//! key 0 (codec-wire-ticket §2.2.3).

use dstore_codec::cbor_struct;

cbor_struct! {
    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct Msg = "wire.Msg" {
        0 => typ: i64 = "int",
        1 => cluster_id: Vec<u8> = "[]uint8" [omitempty],
        2 => incarnation: u64 = "uint64" [omitempty],
        3 => epoch: u64 = "uint64" [omitempty],
        4 => keys: Vec<Vec<u8>> = "[][]uint8" [omitempty],
        5 => pin: bool = "bool" [omitempty],
        6 => name: String = "string" [omitempty],
        7 => record: Vec<u8> = "[]uint8" [omitempty],
        8 => data: Vec<u8> = "[]uint8" [omitempty],
        9 => key: Vec<u8> = "[]uint8" [omitempty],
        10 => code: String = "string" [omitempty],
        11 => text: String = "string" [omitempty],
        12 => view: Vec<u8> = "[]uint8" [omitempty],
        13 => version: Vec<u8> = "[]uint8" [omitempty],
        14 => expected_version: Vec<u8> = "[]uint8" [omitempty],
        15 => expected_old: Vec<u8> = "[]uint8" [omitempty],
        16 => force: bool = "bool" [omitempty],
        17 => has_expected: bool = "bool" [omitempty],
        18 => prefix: Vec<u8> = "[]uint8" [omitempty],
        19 => after: Vec<u8> = "[]uint8" [omitempty],
        20 => limit: i64 = "int" [omitempty],
        21 => refs: Vec<RefInfo> = "[]wire.RefInfo" [omitempty],
        22 => next: Vec<u8> = "[]uint8" [omitempty],
        23 => holders: Vec<KeyHolders> = "[]wire.KeyHolders" [omitempty],
        24 => failed: Vec<KeyFailure> = "[]wire.KeyFailure" [omitempty],
        25 => rejected: Vec<KeyReject> = "[]wire.KeyReject" [omitempty],
        26 => short: Vec<KeyHolders> = "[]wire.KeyHolders" [omitempty],
        27 => unreachable: Vec<Vec<u8>> = "[][]uint8" [omitempty],
        28 => retry_after: i64 = "int64" [omitempty],
        29 => current: Vec<u8> = "[]uint8" [omitempty],
        30 => shortfall: i64 = "int" [omitempty],
        31 => has_current: bool = "bool" [omitempty],
        32 => reg: Vec<u8> = "[]uint8" [omitempty],
        33 => ballot: Vec<u8> = "[]uint8" [omitempty],
        34 => value: Vec<u8> = "[]uint8" [omitempty],
        35 => has_value: bool = "bool" [omitempty],
        36 => accepted: Vec<u8> = "[]uint8" [omitempty],
        37 => promised: Vec<u8> = "[]uint8" [omitempty],
        38 => not_after: i64 = "int64" [omitempty],
        39 => rows: Vec<ScanRow> = "[]wire.ScanRow" [omitempty],
        40 => more: bool = "bool" [omitempty],
        41 => since: u64 = "uint64" [omitempty],
        42 => token: Vec<u8> = "[]uint8" [omitempty],
        43 => weight: u32 = "uint32" [omitempty],
        44 => zone: String = "string" [omitempty],
        45 => addrs: Vec<String> = "[]string" [omitempty],
        46 => no_vote: bool = "bool" [omitempty],
        47 => g: u64 = "uint64" [omitempty],
        48 => nonce: Vec<u8> = "[]uint8" [omitempty],
        49 => seq: u64 = "uint64" [omitempty],
        50 => expand: bool = "bool" [omitempty],
        51 => params: Vec<u8> = "[]uint8" [omitempty],
        52 => sent: u64 = "uint64" [omitempty],
        53 => received: u64 = "uint64" [omitempty],
        54 => idle: bool = "bool" [omitempty],
        55 => marked: u64 = "uint64" [omitempty],
        56 => missing: Vec<Vec<u8>> = "[][]uint8" [omitempty],
        57 => status: Vec<u8> = "[]uint8" [omitempty],
        58 => node: Vec<u8> = "[]uint8" [omitempty],
        59 => error: String = "string" [omitempty],
        60 => pattern: String = "string" [omitempty],
        61 => deleted: Vec<String> = "[]string" [omitempty],
    }
}

cbor_struct! {
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct KeyHolders = "wire.KeyHolders" {
        0 => key: Option<Vec<u8>> = "[]uint8",
        1 => holders: Vec<Vec<u8>> = "[][]uint8" [omitempty],
    }
}

cbor_struct! {
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct KeyFailure = "wire.KeyFailure" {
        0 => key: Option<Vec<u8>> = "[]uint8",
        1 => node: Option<Vec<u8>> = "[]uint8",
        2 => reason: String = "string",
        3 => retry_after: i64 = "int64" [omitempty],
    }
}

cbor_struct! {
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct KeyReject = "wire.KeyReject" {
        0 => key: Option<Vec<u8>> = "[]uint8",
        1 => reason: String = "string",
    }
}

cbor_struct! {
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct RefInfo = "wire.RefInfo" {
        0 => name: String = "string",
        1 => key: Option<Vec<u8>> = "[]uint8",
        2 => version: Option<Vec<u8>> = "[]uint8",
        3 => created_at: i64 = "int64",
        4 => user: String = "string" [omitempty],
    }
}

cbor_struct! {
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct ScanRow = "wire.ScanRow" {
        0 => reg: Option<Vec<u8>> = "[]uint8",
        1 => promised: Vec<u8> = "[]uint8" [omitempty],
        2 => accepted: Vec<u8> = "[]uint8" [omitempty],
        3 => value: Vec<u8> = "[]uint8" [omitempty],
        4 => has_value: bool = "bool" [omitempty],
    }
}

#[cfg(test)]
mod tests {
    use dstore_codec::{marshal, unmarshal};

    use super::*;
    use crate::consts::{T_ERR, T_JOIN, T_REF_CHANGES, T_REF_LIST};

    fn hex(s: &str) -> Vec<u8> {
        dstore_gocompat::hex::decode_string(s.as_bytes()).expect("test hex")
    }

    /// codec-wire-ticket §2.2.2 probes: nil non-omitempty `[]byte` fields encode as null.
    #[test]
    fn sub_struct_probes() {
        assert_eq!(marshal(&KeyHolders::default()), hex("a100f6"));
        assert_eq!(marshal(&ScanRow::default()), hex("a100f6"));
        let failure = KeyFailure {
            key: Some(vec![1]),
            ..KeyFailure::default()
        };
        assert_eq!(marshal(&failure), hex("a300410101f60260"));
        let info = RefInfo {
            name: "n".into(),
            key: Some(vec![]),
            version: Some(vec![9]),
            created_at: -2,
            user: "u".into(),
        };
        let b = hex("a500616e01400241090321046175");
        assert_eq!(marshal(&info), b);
        assert_eq!(unmarshal::<RefInfo>(&b), Ok(info));
        assert_eq!(unmarshal::<RefInfo>(&hex("a100f6")), Ok(RefInfo::default()));
    }

    /// codec-wire-ticket §3.3 "other probe payloads".
    #[test]
    fn msg_probes() {
        let cases = [
            (
                Msg {
                    typ: T_ERR,
                    code: "busy".into(),
                    retry_after: 1500,
                    ..Msg::default()
                },
                "a3000a0a6462757379181c1905dc",
            ),
            (
                Msg {
                    typ: T_REF_LIST,
                    limit: -1,
                    shortfall: 300,
                    ..Msg::default()
                },
                "a30018271420181e19012c",
            ),
            (
                Msg {
                    typ: T_JOIN,
                    weight: 70000,
                    since: 1 << 40,
                    g: (1 << 32) - 1,
                    ..Msg::default()
                },
                "a400186018291b0000010000000000182b1a00011170182f1affffffff",
            ),
        ];
        for (m, want) in cases {
            assert_eq!(marshal(&m), hex(want), "{m:?}");
            assert_eq!(unmarshal::<Msg>(&hex(want)), Ok(m));
        }
        // A nil element of a [][]byte list decodes as empty (codec-wire-ticket R6).
        let m = unmarshal::<Msg>(&hex("a300183b181b82f640183d82617860")).expect("decode");
        assert_eq!(m.typ, T_REF_CHANGES);
        assert_eq!(m.unreachable, [Vec::<u8>::new(), Vec::new()]);
        assert_eq!(m.deleted, ["x", ""]);
    }
}
