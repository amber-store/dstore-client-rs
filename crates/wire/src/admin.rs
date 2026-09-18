//! Node payloads carried in `Msg.params` / `Msg.status`: `node.AdminRequest`, `node.AdminReply`
//! (`node/admin.go:18-46`), `node.Status`, `node.VoterStat`, `node.DecodeStatus` (`node/status.go`).

use dstore_codec::cbor_struct;

cbor_struct! {
    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct AdminRequest = "node.AdminRequest" {
        0 => op: String = "string",
        1 => node: Vec<u8> = "[]uint8" [omitempty],
        2 => weight: u32 = "uint32" [omitempty],
        3 => zone: String = "string" [omitempty],
        4 => replicas: u8 = "uint8" [omitempty],
        5 => dead: bool = "bool" [omitempty],
        6 => allow_unsafe: bool = "bool" [omitempty],
        7 => force: bool = "bool" [omitempty],
        8 => key: Vec<u8> = "[]uint8" [omitempty],
        9 => garbage: f64 = "float64" [omitempty],
        10 => tolerate: bool = "bool" [omitempty],
        11 => forwarded: bool = "bool" [omitempty],
        12 => pause: bool = "bool" [omitempty],
        13 => rate: u64 = "uint64" [omitempty],
        14 => names: Vec<String> = "[]string" [omitempty],
    }
}

cbor_struct! {
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct AdminReply = "node.AdminReply" {
        0 => text: String = "string" [omitempty],
        1 => token: Vec<u8> = "[]uint8" [omitempty],
        2 => view: Vec<u8> = "[]uint8" [omitempty],
        3 => names: Vec<String> = "[]string" [omitempty],
        4 => key: Vec<u8> = "[]uint8" [omitempty],
        5 => ticket: String = "string" [omitempty],
        6 => gc: Vec<u8> = "[]uint8" [omitempty],
    }
}

cbor_struct! {
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct VoterStat = "node.VoterStat" {
        0 => id: Option<Vec<u8>> = "[]uint8",
        1 => calls: u64 = "uint64",
        2 => failures: u64 = "uint64",
        3 => p99ms: i64 = "int64",
    }
}

cbor_struct! {
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct Status = "node.Status" {
        0 => id: Option<Vec<u8>> = "[]uint8",
        1 => epoch: u64 = "uint64",
        2 => incarnation: u64 = "uint64",
        3 => packs: i64 = "int",
        4 => records: u64 = "uint64",
        5 => bytes: i64 = "int64",
        6 => pins: i64 = "int",
        7 => unreachable: Vec<Vec<u8>> = "[][]uint8" [omitempty],
        8 => pending_packs: i64 = "int",
        9 => transition: String = "string" [omitempty],
        10 => gc: String = "string" [omitempty],
        11 => lease_holder: Vec<u8> = "[]uint8" [omitempty],
        12 => voters: Vec<VoterStat> = "[]node.VoterStat" [omitempty],
        13 => writable: bool = "bool",
        14 => free_bytes: i64 = "int64",
        15 => total_bytes: i64 = "int64",
        16 => puts: u64 = "uint64",
        17 => gets: u64 = "uint64",
        18 => ref_puts: u64 = "uint64",
        19 => bytes_in: u64 = "uint64",
        20 => bytes_out: u64 = "uint64",
        21 => amnesiac: bool = "bool" [omitempty],
        22 => retired: bool = "bool" [omitempty],
        23 => scrub_age_sec: i64 = "int64" [omitempty],
        24 => last_live: u64 = "uint64" [omitempty],
        25 => corrupt: i64 = "int" [omitempty],
        26 => is_holder: bool = "bool" [omitempty],
        27 => unaudited_keys: i64 = "int" [omitempty],
        28 => watchers: i64 = "int" [omitempty],
    }
}

/// `node.DecodeStatus`: `codec.Unmarshal` into a `node.Status`. Go also returns the partly filled value
/// with an error; its only caller (`printStatus`) discards it.
pub fn decode_status(b: &[u8]) -> Result<Status, dstore_codec::DecodeError> {
    dstore_codec::unmarshal::<Status>(b)
}

/// `codec.Unmarshal` into a `node.AdminReply` (`cmd/dstore/client.go:98-108` returns the error as is).
pub fn decode_admin_reply(b: &[u8]) -> Result<AdminReply, dstore_codec::DecodeError> {
    dstore_codec::unmarshal::<AdminReply>(b)
}

#[cfg(test)]
mod tests {
    use dstore_codec::marshal;

    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        dstore_gocompat::hex::decode_string(s.as_bytes()).expect("test hex")
    }

    #[test]
    fn admin_request_probes() {
        let cases: &[(AdminRequest, &str)] = &[
            (
                AdminRequest {
                    op: "cluster-ticket".into(),
                    ..AdminRequest::default()
                },
                "a1006e636c75737465722d7469636b6574",
            ),
            (
                AdminRequest {
                    op: "gc-run".into(),
                    garbage: 0.5,
                    ..AdminRequest::default()
                },
                "a2006667632d72756e09f93800",
            ),
            (
                AdminRequest {
                    op: "gc-run".into(),
                    garbage: -0.0,
                    ..AdminRequest::default()
                },
                "a1006667632d72756e",
            ),
            (
                AdminRequest {
                    op: "replicas".into(),
                    replicas: 3,
                    weight: 100,
                    names: vec!["a".into()],
                    ..AdminRequest::default()
                },
                "a400687265706c6963617302186404030e816161",
            ),
            (
                AdminRequest {
                    op: "node-remove".into(),
                    node: vec![0xab; 32],
                    dead: true,
                    allow_unsafe: true,
                    ..AdminRequest::default()
                },
                "a4006b6e6f64652d72656d6f7665015820abababababababababababababababababababababababababababababababab05f506f5",
            ),
        ];
        for (req, want) in cases {
            assert_eq!(marshal(req), hex(want), "{req:?}");
            let back: AdminRequest = dstore_codec::unmarshal(&hex(want)).expect("decode");
            assert_eq!(back.op, req.op);
            assert_eq!(back.garbage.to_bits(), (req.garbage + 0.0).to_bits());
        }
    }

    #[test]
    fn admin_reply_probes() {
        let reply = AdminReply {
            text: "ok".into(),
            ..AdminReply::default()
        };
        assert_eq!(marshal(&reply), hex("a100626f6b"));
        assert_eq!(
            decode_admin_reply(&hex("a200626f6b186301")).map(|r| r.text),
            Ok("ok".to_owned())
        );
        assert_eq!(decode_admin_reply(&hex("a0")), Ok(AdminReply::default()));
        assert_eq!(
            decode_admin_reply(&[0xff]).map_err(|e| e.to_string()),
            Err("cbor: unexpected \"break\" code".into())
        );
    }

    #[test]
    fn status_probes() {
        let zero = "b000f601000200030004000500060008000df40e000f0010001100120013001400";
        assert_eq!(marshal(&Status::default()), hex(zero));
        assert_eq!(decode_status(&hex(zero)), Ok(Status::default()));

        // cli §3.8.
        let st = Status {
            id: Some(vec![0xab; 32]),
            epoch: 5,
            incarnation: 1,
            packs: 2,
            records: 10,
            bytes: 1234,
            pins: 1,
            unreachable: vec![vec![0x01; 32]],
            transition: "idle".into(),
            gc: "epoch 3 idle".into(),
            voters: vec![VoterStat {
                id: Some(vec![0x01; 32]),
                calls: 10,
                failures: 1,
                p99ms: 12,
            }],
            writable: true,
            free_bytes: (5 << 30) + 123,
            total_bytes: 10 << 30,
            is_holder: true,
            ..Status::default()
        };
        let want = hex(
            "b5005820abababababababababababababababababababababababababababababababab010502010302040a051904d206010781582001010101010101010101010101010101010101010101010101010101010101010800096469646c650a6c65706f636820332069646c650c81a40058200101010101010101010101010101010101010101010101010101010101010101010a0201030c0df50e1b000000014000007b0f1b000000028000000010001100120013001400181af5",
        );
        assert_eq!(marshal(&st), want);
        assert_eq!(decode_status(&want), Ok(st));
        assert_eq!(
            decode_status(&[0xff]).map_err(|e| e.to_string()),
            Err("cbor: unexpected \"break\" code".into())
        );
        assert_eq!(
            decode_status(&[]).map_err(|e| e.to_string()),
            Err("EOF".into())
        );
    }
}
