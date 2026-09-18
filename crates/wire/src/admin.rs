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

/// `node.DecodeStatus`.
pub fn decode_status(b: &[u8]) -> Result<Status, dstore_codec::DecodeError> {
    todo!()
}

/// `codec.Unmarshal` into a `node.AdminReply`.
pub fn decode_admin_reply(b: &[u8]) -> Result<AdminReply, dstore_codec::DecodeError> {
    todo!()
}
