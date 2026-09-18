//! Golden tests of `dstore-wire` messages, frames and payloads (owner wire): `wire/frames.json`,
//! `wire/decode.json`, `wire/frame_errors.json`, `admin/requests.json`, `admin/replies.json`,
//! `status/status.json`. Schemas: VECTORS.md, families `wire` and `admin`.
//!
//! Vector fields that other crates' functions produce are asserted by those crates' tests: the client
//! extras of `frames.json` replies (`cas_mismatch`, `incomplete.error`, `short_ids`, `holders_ids`) and the
//! CLI outputs (`argv`, `printed`, `printed_token_create`, `cluster_ticket`, `node`, `unreachable`).

use std::error::Error;
use std::fmt;
use std::sync::LazyLock;
use std::time::Duration;

use amber_store_core::key::{Key, Type};
use dstore_codec::{DecodeError, Struct, marshal, unmarshal};
use dstore_testkit::golden::{self, Payload, decimal_i64, decimal_u64};
use dstore_ticket::{Member, Ticket};
use dstore_wire::{
    AdminReply, AdminRequest, KeyFailure, KeyHolders, KeyReject, MAX_FRAME, Msg,
    ProtocolDataEndpointRec, ProtocolFrameError, ProtocolMsg, ProtocolRefInfo, ProtocolRemoteError,
    RefInfo, RemoteError, ScanRow, Status, T_ADMIN, T_ADMIN_REPLY, VoterStat, WireError, as_remote,
    decode_admin_reply, decode_status, encode_frame, error_from_msg, expect, is_code, keys32,
    read_msg, read_protocol_msg, write_msg,
};
use serde::{Deserialize, Deserializer};

// ---------------------------------------------------------------------------------------------------------
// Helpers.

fn hx(s: &str) -> Vec<u8> {
    golden::hex(s)
}

fn hx_opt(s: &Option<String>) -> Option<Vec<u8>> {
    s.as_deref().map(golden::hex)
}

/// A `[][]byte` list: a `null` element is a Go nil element, which `Vec<Vec<u8>>` holds as empty (R6).
fn hx_list(l: &[Option<String>]) -> Vec<Vec<u8>> {
    l.iter().map(|e| hx_opt(e).unwrap_or_default()).collect()
}

fn to_hex(b: &[u8]) -> String {
    dstore_gocompat::hex::encode(b)
}

/// serde `deserialize_with`: `null` reads as the default value (Go nil lists).
fn nullable<'de, D: Deserializer<'de>, T: Deserialize<'de> + Default>(d: D) -> Result<T, D::Error> {
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// The first 30 bytes of BLAKE3-256(data). blake3 is not a dev-dependency of this package, but
/// `amber_store_core::key::Key::new` stores the leading digest bytes after the key's header and length
/// bytes; with length 0 the length takes one byte, so key bytes 2..32 are digest bytes 0..30.
fn blake3_prefix(data: &[u8]) -> Vec<u8> {
    Key::new(Type::Blob, 0, data).0[2..].to_vec()
}

fn same_blake3(data: &[u8], want_hex: &str) -> bool {
    hx(want_hex).get(..30) == Some(&blake3_prefix(data)[..])
}

/// Collects failures so one run reports every mismatching case.
#[derive(Default)]
struct Failures {
    list: Vec<String>,
    checked: usize,
}

impl Failures {
    fn check(&mut self, ok: bool, msg: impl FnOnce() -> String) {
        self.checked += 1;
        if !ok {
            self.list.push(msg());
        }
    }

    fn fail(&mut self, msg: String) {
        self.checked += 1;
        self.list.push(msg);
    }

    fn finish(self, what: &str) {
        assert!(self.checked > 0, "{what}: no case checked");
        if !self.list.is_empty() {
            let shown: Vec<&str> = self.list.iter().take(40).map(String::as_str).collect();
            panic!(
                "{what}: {} of {} checks failed:\n{}",
                self.list.len(),
                self.checked,
                shown.join("\n")
            );
        }
    }
}

/// `fmt.Errorf("<prefix>: %w", inner)`.
#[derive(Debug)]
struct Wrapped {
    prefix: &'static str,
    inner: Box<dyn Error + Send + Sync + 'static>,
}

impl fmt::Display for Wrapped {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.prefix, self.inner)
    }
}

impl Error for Wrapped {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&*self.inner)
    }
}

// ---------------------------------------------------------------------------------------------------------
// Shared JSON types (VECTORS.md "Shared JSON types of the protocol families").

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RefInfoJson {
    name: String,
    key: Option<String>,
    version: Option<String>,
    #[serde(deserialize_with = "decimal_i64")]
    created_at: i64,
    user: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyHoldersJson {
    key: Option<String>,
    #[serde(deserialize_with = "nullable")]
    holders: Vec<Option<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyFailureJson {
    key: Option<String>,
    node: Option<String>,
    reason: String,
    #[serde(deserialize_with = "decimal_i64")]
    retry_after: i64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyRejectJson {
    key: Option<String>,
    reason: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScanRowJson {
    reg: Option<String>,
    promised: String,
    accepted: String,
    value: String,
    has_value: bool,
}

/// `MsgJSON`: sparse; a missing field is the zero value.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MsgJson {
    #[serde(deserialize_with = "decimal_i64")]
    typ: i64,
    #[serde(default)]
    cluster_id: String,
    #[serde(default, deserialize_with = "decimal_u64")]
    incarnation: u64,
    #[serde(default, deserialize_with = "decimal_u64")]
    epoch: u64,
    #[serde(default, deserialize_with = "nullable")]
    keys: Vec<Option<String>>,
    #[serde(default)]
    pin: bool,
    #[serde(default)]
    name: String,
    #[serde(default)]
    record: String,
    #[serde(default)]
    data: String,
    #[serde(default)]
    key: String,
    #[serde(default)]
    code: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    view: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    expected_version: String,
    #[serde(default)]
    expected_old: String,
    #[serde(default)]
    force: bool,
    #[serde(default)]
    has_expected: bool,
    #[serde(default)]
    prefix: String,
    #[serde(default)]
    after: String,
    #[serde(default, deserialize_with = "decimal_i64")]
    limit: i64,
    #[serde(default, deserialize_with = "nullable")]
    refs: Vec<RefInfoJson>,
    #[serde(default)]
    next: String,
    #[serde(default, deserialize_with = "nullable")]
    holders: Vec<KeyHoldersJson>,
    #[serde(default, deserialize_with = "nullable")]
    failed: Vec<KeyFailureJson>,
    #[serde(default, deserialize_with = "nullable")]
    rejected: Vec<KeyRejectJson>,
    #[serde(default, deserialize_with = "nullable")]
    short: Vec<KeyHoldersJson>,
    #[serde(default, deserialize_with = "nullable")]
    unreachable: Vec<Option<String>>,
    #[serde(default, deserialize_with = "decimal_i64")]
    retry_after: i64,
    #[serde(default)]
    current: String,
    #[serde(default, deserialize_with = "decimal_i64")]
    shortfall: i64,
    #[serde(default)]
    has_current: bool,
    #[serde(default)]
    reg: String,
    #[serde(default)]
    ballot: String,
    #[serde(default)]
    value: String,
    #[serde(default)]
    has_value: bool,
    #[serde(default)]
    accepted: String,
    #[serde(default)]
    promised: String,
    #[serde(default, deserialize_with = "decimal_i64")]
    not_after: i64,
    #[serde(default, deserialize_with = "nullable")]
    rows: Vec<ScanRowJson>,
    #[serde(default)]
    more: bool,
    #[serde(default, deserialize_with = "decimal_u64")]
    since: u64,
    #[serde(default)]
    token: String,
    #[serde(default)]
    weight: u32,
    #[serde(default)]
    zone: String,
    #[serde(default, deserialize_with = "nullable")]
    addrs: Vec<String>,
    #[serde(default)]
    no_vote: bool,
    #[serde(default, deserialize_with = "decimal_u64")]
    g: u64,
    #[serde(default)]
    nonce: String,
    #[serde(default, deserialize_with = "decimal_u64")]
    seq: u64,
    #[serde(default)]
    expand: bool,
    #[serde(default)]
    params: String,
    #[serde(default, deserialize_with = "decimal_u64")]
    sent: u64,
    #[serde(default, deserialize_with = "decimal_u64")]
    received: u64,
    #[serde(default)]
    idle: bool,
    #[serde(default, deserialize_with = "decimal_u64")]
    marked: u64,
    #[serde(default, deserialize_with = "nullable")]
    missing: Vec<Option<String>>,
    #[serde(default)]
    status: String,
    #[serde(default)]
    node: String,
    #[serde(default)]
    error: String,
    #[serde(default)]
    pattern: String,
    #[serde(default, deserialize_with = "nullable")]
    deleted: Vec<String>,
}

fn key_holders(l: &[KeyHoldersJson]) -> Vec<KeyHolders> {
    l.iter()
        .map(|h| KeyHolders {
            key: hx_opt(&h.key),
            holders: hx_list(&h.holders),
        })
        .collect()
}

impl MsgJson {
    fn to_msg(&self) -> Msg {
        Msg {
            typ: self.typ,
            cluster_id: hx(&self.cluster_id),
            incarnation: self.incarnation,
            epoch: self.epoch,
            keys: hx_list(&self.keys),
            pin: self.pin,
            name: self.name.clone(),
            record: hx(&self.record),
            data: hx(&self.data),
            key: hx(&self.key),
            code: self.code.clone(),
            text: self.text.clone(),
            view: hx(&self.view),
            version: hx(&self.version),
            expected_version: hx(&self.expected_version),
            expected_old: hx(&self.expected_old),
            force: self.force,
            has_expected: self.has_expected,
            prefix: hx(&self.prefix),
            after: hx(&self.after),
            limit: self.limit,
            refs: self
                .refs
                .iter()
                .map(|r| RefInfo {
                    name: r.name.clone(),
                    key: hx_opt(&r.key),
                    version: hx_opt(&r.version),
                    created_at: r.created_at,
                    user: r.user.clone(),
                })
                .collect(),
            next: hx(&self.next),
            holders: key_holders(&self.holders),
            failed: self
                .failed
                .iter()
                .map(|f| KeyFailure {
                    key: hx_opt(&f.key),
                    node: hx_opt(&f.node),
                    reason: f.reason.clone(),
                    retry_after: f.retry_after,
                })
                .collect(),
            rejected: self
                .rejected
                .iter()
                .map(|r| KeyReject {
                    key: hx_opt(&r.key),
                    reason: r.reason.clone(),
                })
                .collect(),
            short: key_holders(&self.short),
            unreachable: hx_list(&self.unreachable),
            retry_after: self.retry_after,
            current: hx(&self.current),
            shortfall: self.shortfall,
            has_current: self.has_current,
            reg: hx(&self.reg),
            ballot: hx(&self.ballot),
            value: hx(&self.value),
            has_value: self.has_value,
            accepted: hx(&self.accepted),
            promised: hx(&self.promised),
            not_after: self.not_after,
            rows: self
                .rows
                .iter()
                .map(|r| ScanRow {
                    reg: hx_opt(&r.reg),
                    promised: hx(&r.promised),
                    accepted: hx(&r.accepted),
                    value: hx(&r.value),
                    has_value: r.has_value,
                })
                .collect(),
            more: self.more,
            since: self.since,
            token: hx(&self.token),
            weight: self.weight,
            zone: self.zone.clone(),
            addrs: self.addrs.clone(),
            no_vote: self.no_vote,
            g: self.g,
            nonce: hx(&self.nonce),
            seq: self.seq,
            expand: self.expand,
            params: hx(&self.params),
            sent: self.sent,
            received: self.received,
            idle: self.idle,
            marked: self.marked,
            missing: hx_list(&self.missing),
            status: hx(&self.status),
            node: hx(&self.node),
            error: self.error.clone(),
            pattern: self.pattern.clone(),
            deleted: self.deleted.clone(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProtocolRefInfoJson {
    name: String,
    key: Option<String>,
    #[serde(deserialize_with = "decimal_i64")]
    created_at: i64,
    user: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProtocolEndpointJson {
    id: Option<String>,
    #[serde(deserialize_with = "nullable")]
    addrs: Vec<String>,
}

/// `ProtocolMsgJSON`: sparse like `MsgJSON`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProtocolMsgJson {
    #[serde(deserialize_with = "decimal_i64")]
    typ: i64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    root: String,
    #[serde(default)]
    cas: bool,
    #[serde(default)]
    expected_old: String,
    #[serde(default)]
    record: String,
    #[serde(default, deserialize_with = "nullable")]
    refs: Vec<ProtocolRefInfoJson>,
    #[serde(default, deserialize_with = "nullable")]
    keys: Vec<Option<String>>,
    #[serde(default)]
    data: String,
    #[serde(default)]
    key: String,
    #[serde(default)]
    code: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    current: String,
    #[serde(default)]
    token: String,
    #[serde(default, deserialize_with = "decimal_i64")]
    data_conns: i64,
    #[serde(default, deserialize_with = "nullable")]
    data_ports: Vec<u16>,
    #[serde(default, deserialize_with = "nullable")]
    data_endpoints: Vec<ProtocolEndpointJson>,
    #[serde(default, deserialize_with = "nullable")]
    names: Vec<String>,
}

impl ProtocolMsgJson {
    fn to_msg(&self) -> ProtocolMsg {
        ProtocolMsg {
            typ: self.typ,
            name: self.name.clone(),
            root: hx(&self.root),
            cas: self.cas,
            expected_old: hx(&self.expected_old),
            record: hx(&self.record),
            refs: self
                .refs
                .iter()
                .map(|r| ProtocolRefInfo {
                    name: r.name.clone(),
                    key: hx_opt(&r.key),
                    created_at: r.created_at,
                    user: r.user.clone(),
                })
                .collect(),
            keys: hx_list(&self.keys),
            data: hx(&self.data),
            key: hx(&self.key),
            code: self.code.clone(),
            text: self.text.clone(),
            current: hx(&self.current),
            token: hx(&self.token),
            data_conns: self.data_conns,
            data_ports: self.data_ports.clone(),
            data_endpoints: self
                .data_endpoints
                .iter()
                .map(|e| ProtocolDataEndpointRec {
                    id: hx_opt(&e.id),
                    addrs: e.addrs.clone(),
                })
                .collect(),
            names: self.names.clone(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MemberJson {
    id: Option<String>,
    #[serde(deserialize_with = "nullable")]
    addrs: Vec<String>,
}

/// `TicketJSON`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TicketJson {
    cluster_id: Option<String>,
    #[serde(deserialize_with = "decimal_u64")]
    incarnation: u64,
    members: Option<Vec<MemberJson>>,
}

impl TicketJson {
    fn to_ticket(&self) -> Ticket {
        Ticket {
            cluster_id: hx_opt(&self.cluster_id),
            incarnation: self.incarnation,
            members: self.members.as_ref().map(|l| {
                l.iter()
                    .map(|m| Member {
                        id: hx_opt(&m.id),
                        addrs: m.addrs.clone(),
                    })
                    .collect()
            }),
        }
    }
}

/// `AdminRequestJSON`: every field present.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdminRequestJson {
    op: String,
    node: String,
    weight: u32,
    zone: String,
    replicas: u8,
    dead: bool,
    allow_unsafe: bool,
    force: bool,
    key: String,
    /// Go's shortest `'g'` text, for failure messages; the value comes from `garbage_bits`.
    garbage: String,
    garbage_bits: String,
    tolerate: bool,
    forwarded: bool,
    pause: bool,
    #[serde(deserialize_with = "decimal_u64")]
    rate: u64,
    #[serde(deserialize_with = "nullable")]
    names: Vec<String>,
}

impl AdminRequestJson {
    fn to_request(&self) -> AdminRequest {
        let bits = match u64::from_str_radix(&self.garbage_bits, 16) {
            Ok(b) => b,
            Err(e) => panic!(
                "garbage_bits {:?} ({}): {e}",
                self.garbage_bits, self.garbage
            ),
        };
        AdminRequest {
            op: self.op.clone(),
            node: hx(&self.node),
            weight: self.weight,
            zone: self.zone.clone(),
            replicas: self.replicas,
            dead: self.dead,
            allow_unsafe: self.allow_unsafe,
            force: self.force,
            key: hx(&self.key),
            garbage: f64::from_bits(bits),
            tolerate: self.tolerate,
            forwarded: self.forwarded,
            pause: self.pause,
            rate: self.rate,
            names: self.names.clone(),
        }
    }
}

/// Equality with `garbage` compared by bits (NaN, -0.0).
fn same_admin_request(a: &AdminRequest, b: &AdminRequest) -> bool {
    let zeroed = |r: &AdminRequest| AdminRequest {
        garbage: 0.0,
        ..r.clone()
    };
    a.garbage.to_bits() == b.garbage.to_bits() && zeroed(a) == zeroed(b)
}

/// `AdminReplyJSON`: every field present.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdminReplyJson {
    text: String,
    token: String,
    view: String,
    #[serde(deserialize_with = "nullable")]
    names: Vec<String>,
    key: String,
    ticket: String,
    gc: String,
}

impl AdminReplyJson {
    fn to_reply(&self) -> AdminReply {
        AdminReply {
            text: self.text.clone(),
            token: hx(&self.token),
            view: hx(&self.view),
            names: self.names.clone(),
            key: hx(&self.key),
            ticket: self.ticket.clone(),
            gc: hx(&self.gc),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct VoterStatJson {
    id: Option<String>,
    #[serde(deserialize_with = "decimal_u64")]
    calls: u64,
    #[serde(deserialize_with = "decimal_u64")]
    failures: u64,
    #[serde(deserialize_with = "decimal_i64")]
    p99ms: i64,
}

/// `StatusJSON`: every field present.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StatusJson {
    id: Option<String>,
    #[serde(deserialize_with = "decimal_u64")]
    epoch: u64,
    #[serde(deserialize_with = "decimal_u64")]
    incarnation: u64,
    #[serde(deserialize_with = "decimal_i64")]
    packs: i64,
    #[serde(deserialize_with = "decimal_u64")]
    records: u64,
    #[serde(deserialize_with = "decimal_i64")]
    bytes: i64,
    #[serde(deserialize_with = "decimal_i64")]
    pins: i64,
    #[serde(deserialize_with = "nullable")]
    unreachable: Vec<Option<String>>,
    #[serde(deserialize_with = "decimal_i64")]
    pending_packs: i64,
    transition: String,
    gc: String,
    lease_holder: String,
    #[serde(deserialize_with = "nullable")]
    voters: Vec<VoterStatJson>,
    writable: bool,
    #[serde(deserialize_with = "decimal_i64")]
    free_bytes: i64,
    #[serde(deserialize_with = "decimal_i64")]
    total_bytes: i64,
    #[serde(deserialize_with = "decimal_u64")]
    puts: u64,
    #[serde(deserialize_with = "decimal_u64")]
    gets: u64,
    #[serde(deserialize_with = "decimal_u64")]
    ref_puts: u64,
    #[serde(deserialize_with = "decimal_u64")]
    bytes_in: u64,
    #[serde(deserialize_with = "decimal_u64")]
    bytes_out: u64,
    amnesiac: bool,
    retired: bool,
    #[serde(deserialize_with = "decimal_i64")]
    scrub_age_sec: i64,
    #[serde(deserialize_with = "decimal_u64")]
    last_live: u64,
    #[serde(deserialize_with = "decimal_i64")]
    corrupt: i64,
    is_holder: bool,
    #[serde(deserialize_with = "decimal_i64")]
    unaudited_keys: i64,
    #[serde(deserialize_with = "decimal_i64")]
    watchers: i64,
}

impl StatusJson {
    fn to_status(&self) -> Status {
        Status {
            id: hx_opt(&self.id),
            epoch: self.epoch,
            incarnation: self.incarnation,
            packs: self.packs,
            records: self.records,
            bytes: self.bytes,
            pins: self.pins,
            unreachable: hx_list(&self.unreachable),
            pending_packs: self.pending_packs,
            transition: self.transition.clone(),
            gc: self.gc.clone(),
            lease_holder: hx(&self.lease_holder),
            voters: self
                .voters
                .iter()
                .map(|v| VoterStat {
                    id: hx_opt(&v.id),
                    calls: v.calls,
                    failures: v.failures,
                    p99ms: v.p99ms,
                })
                .collect(),
            writable: self.writable,
            free_bytes: self.free_bytes,
            total_bytes: self.total_bytes,
            puts: self.puts,
            gets: self.gets,
            ref_puts: self.ref_puts,
            bytes_in: self.bytes_in,
            bytes_out: self.bytes_out,
            amnesiac: self.amnesiac,
            retired: self.retired,
            scrub_age_sec: self.scrub_age_sec,
            last_live: self.last_live,
            corrupt: self.corrupt,
            is_holder: self.is_holder,
            unaudited_keys: self.unaudited_keys,
            watchers: self.watchers,
        }
    }
}

/// `RemoteErrorJSON` (`wire.ErrorFromMsg`).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RemoteErrorJson {
    code: String,
    text: String,
    view: String,
    #[serde(deserialize_with = "decimal_i64")]
    retry_after_ns: i64,
    retry_after: String,
    error: String,
}

/// Rust clamps a negative Go duration to ZERO (PORTING §4.3), so the duration is compared only when
/// Go's is non-negative.
fn check_remote(label: &str, e: &RemoteError, j: &RemoteErrorJson, f: &mut Failures) {
    f.check(
        e.code == j.code && e.text == j.text && e.view == hx(&j.view),
        || format!("{label}: remote error {e:?}, want {j:?}"),
    );
    f.check(e.to_string() == j.error, || {
        format!(
            "{label}: remote error text {:?}, want {:?}",
            e.to_string(),
            j.error
        )
    });
    match u64::try_from(j.retry_after_ns) {
        Ok(ns) => {
            let text = dstore_gocompat::time::duration_string(
                dstore_gocompat::time::duration_to_ns(e.retry_after),
            );
            f.check(
                e.retry_after == Duration::from_nanos(ns) && text == j.retry_after,
                || {
                    format!(
                        "{label}: retry_after {:?} ({text}), want {ns}ns ({})",
                        e.retry_after, j.retry_after
                    )
                },
            );
        }
        Err(_) => f.check(e.retry_after == Duration::ZERO, || {
            format!("{label}: negative retry_after gave {:?}", e.retry_after)
        }),
    }
}

// ---------------------------------------------------------------------------------------------------------
// wire/frames.json

#[derive(Deserialize)]
struct FramesFile {
    requests: Vec<FrameCase>,
    replies: Vec<FrameCase>,
    other: Vec<FrameCase>,
    bulk: Vec<BulkCase>,
    keys32: Vec<Keys32Case>,
    error_is: Vec<ErrorIsCase>,
    is_code: Vec<IsCodeCase>,
}

#[derive(Deserialize)]
struct FrameCase {
    name: String,
    source: String,
    decode_only: bool,
    #[serde(default)]
    null_element: bool,
    msg: MsgJson,
    frame_hex: String,
    #[serde(default)]
    remote_error: Option<RemoteErrorJson>,
    #[serde(default)]
    keys32: Option<Keys32Json>,
    #[serde(default)]
    incomplete: Option<IncompleteJson>,
}

#[derive(Debug, Deserialize)]
struct Keys32Json {
    ok: bool,
    count: usize,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Deserialize)]
struct IncompleteJson {
    sample: Vec<String>,
}

#[derive(Deserialize)]
struct BulkCase {
    name: String,
    msg: MsgJson,
    keys: Payload,
    key_count: usize,
    frame_len: usize,
    frame_blake3: String,
    head_hex: String,
}

#[derive(Deserialize)]
struct Keys32Case {
    name: String,
    keys: Vec<Option<String>>,
    ok: bool,
    count: usize,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Deserialize)]
struct ErrSpec {
    code: String,
    text: String,
}

impl ErrSpec {
    fn remote(&self) -> RemoteError {
        RemoteError {
            code: self.code.clone(),
            text: self.text.clone(),
            view: Vec::new(),
            retry_after: Duration::ZERO,
        }
    }
}

#[derive(Deserialize)]
struct ErrorIsCase {
    name: String,
    err: ErrSpec,
    wrapped: bool,
    target: ErrSpec,
    is: bool,
}

#[derive(Deserialize)]
struct IsCodeCase {
    name: String,
    kind: String,
    wrapped: bool,
    code: String,
    text: String,
    query: String,
    is_code: bool,
    error: String,
}

/// `keys32 = wire.Keys32(keys)` as the vectors record it.
fn check_keys32(label: &str, raw: &[Vec<u8>], want: &Keys32Json, f: &mut Failures) {
    match keys32(raw) {
        Ok(keys) => f.check(
            want.ok && keys.len() == want.count && want.error.is_none(),
            || format!("{label}: keys32 gave {} keys, want {want:?}", keys.len()),
        ),
        Err(e) => f.check(
            !want.ok && want.count == 0 && want.error.as_deref() == Some(e.to_string().as_str()),
            || format!("{label}: keys32 error {e}, want {want:?}"),
        ),
    }
}

#[tokio::test]
async fn frames_encode_and_read_back() {
    let file: FramesFile = golden::load_json("wire/frames.json");
    let mut f = Failures::default();
    let groups = [
        ("requests", &file.requests),
        ("replies", &file.replies),
        ("other", &file.other),
    ];
    for (group, cases) in groups {
        assert!(!cases.is_empty(), "frames.json {group} is empty");
        for c in cases {
            let label = format!("{group}/{} ({})", c.name, c.source);
            let frame = hx(&c.frame_hex);
            let want = c.msg.to_msg();
            if !c.decode_only && !c.null_element {
                match encode_frame(&want) {
                    Ok(got) => f.check(got == frame, || {
                        format!(
                            "{label}: encode_frame {}, want {}",
                            to_hex(&got),
                            c.frame_hex
                        )
                    }),
                    Err(e) => f.fail(format!("{label}: encode_frame failed: {e}")),
                }
            }
            let mut r = &frame[..];
            let got = match read_msg(&mut r).await {
                Ok(m) => m,
                Err(e) => {
                    f.fail(format!("{label}: read_msg failed: {e}"));
                    continue;
                }
            };
            f.check(got == want, || {
                format!("{label}: read_msg gave {got:?}, want {want:?}")
            });
            f.check(
                matches!(read_msg(&mut r).await, Err(WireError::Eof)),
                || format!("{label}: the frame was not consumed exactly"),
            );
            if let Some(re) = &c.remote_error {
                check_remote(&label, &error_from_msg(&got), re, &mut f);
            }
            if let Some(k) = &c.keys32 {
                check_keys32(&label, &got.keys, k, &mut f);
            }
            if let Some(inc) = &c.incomplete {
                // client `Incomplete.Sample`: the Keys32 result, empty on error.
                let sample: Vec<String> = keys32(&got.keys)
                    .unwrap_or_default()
                    .iter()
                    .map(|k| to_hex(k))
                    .collect();
                f.check(sample == inc.sample, || {
                    format!(
                        "{label}: incomplete sample {sample:?}, want {:?}",
                        inc.sample
                    )
                });
            }
        }
    }
    f.finish("wire/frames.json frames");
}

#[tokio::test]
async fn frames_bulk() {
    let file: FramesFile = golden::load_json("wire/frames.json");
    let mut f = Failures::default();
    for c in &file.bulk {
        let label = &c.name;
        let keys = c.keys.bytes();
        let chunks = keys.chunks_exact(32);
        f.check(chunks.remainder().is_empty(), || {
            format!("{label}: key payload of {} bytes", keys.len())
        });
        let mut m = c.msg.to_msg();
        m.keys = chunks.map(<[u8]>::to_vec).collect();
        f.check(m.keys.len() == c.key_count, || {
            format!("{label}: {} keys, want {}", m.keys.len(), c.key_count)
        });
        let frame = match encode_frame(&m) {
            Ok(fr) => fr,
            Err(e) => {
                f.fail(format!("{label}: encode_frame failed: {e}"));
                continue;
            }
        };
        f.check(frame.len() == c.frame_len, || {
            format!(
                "{label}: frame of {} bytes, want {}",
                frame.len(),
                c.frame_len
            )
        });
        f.check(to_hex(&frame[..frame.len().min(64)]) == c.head_hex, || {
            format!(
                "{label}: frame head {}",
                to_hex(&frame[..frame.len().min(64)])
            )
        });
        f.check(same_blake3(&frame, &c.frame_blake3), || {
            format!(
                "{label}: frame blake3 prefix {}, want {}",
                to_hex(&blake3_prefix(&frame)),
                c.frame_blake3
            )
        });
        match read_msg(&mut &frame[..]).await {
            Ok(got) => f.check(got == m, || format!("{label}: read back differs")),
            Err(e) => f.fail(format!("{label}: read_msg failed: {e}")),
        }
    }
    f.finish("wire/frames.json bulk");
}

#[test]
fn frames_keys32() {
    let file: FramesFile = golden::load_json("wire/frames.json");
    let mut f = Failures::default();
    for c in &file.keys32 {
        let raw = hx_list(&c.keys);
        let want = Keys32Json {
            ok: c.ok,
            count: c.count,
            error: c.error.clone(),
        };
        check_keys32(&c.name, &raw, &want, &mut f);
        if let Ok(keys) = keys32(&raw) {
            let back = dstore_wire::raw_keys(&keys);
            f.check(back == raw, || {
                format!("{}: raw_keys does not round-trip", c.name)
            });
        }
    }
    f.finish("wire/frames.json keys32");
}

#[test]
fn frames_error_is() {
    let file: FramesFile = golden::load_json("wire/frames.json");
    let mut f = Failures::default();
    for c in &file.error_is {
        let target = c.target.remote();
        let err: Box<dyn Error + Send + Sync> = if c.wrapped {
            Box::new(Wrapped {
                prefix: "upload to 01000000",
                inner: Box::new(c.err.remote()),
            })
        } else {
            Box::new(c.err.remote())
        };
        let got = as_remote(&*err).is_some_and(|e| e.is(&target));
        f.check(got == c.is, || {
            format!("{}: errors.Is gave {got}, want {}", c.name, c.is)
        });
    }
    f.finish("wire/frames.json error_is");
}

#[test]
fn frames_is_code() {
    let file: FramesFile = golden::load_json("wire/frames.json");
    let mut f = Failures::default();
    for c in &file.is_code {
        let inner: Box<dyn Error + Send + Sync> = match c.kind.as_str() {
            "wire" => Box::new(RemoteError {
                code: c.code.clone(),
                text: c.text.clone(),
                view: Vec::new(),
                retry_after: Duration::ZERO,
            }),
            "protocol" => Box::new(ProtocolRemoteError {
                code: c.code.clone(),
                text: c.text.clone(),
                current: Vec::new(),
            }),
            other => panic!("{}: unknown kind {other}", c.name),
        };
        let err: Box<dyn Error + Send + Sync> = if c.wrapped {
            Box::new(Wrapped {
                prefix: "get",
                inner,
            })
        } else {
            inner
        };
        let got = is_code(&*err, &c.query);
        f.check(got == c.is_code, || {
            format!("{}: is_code gave {got}, want {}", c.name, c.is_code)
        });
        f.check(err.to_string() == c.error, || {
            format!(
                "{}: error text {:?}, want {:?}",
                c.name,
                err.to_string(),
                c.error
            )
        });
    }
    f.finish("wire/frames.json is_code");
}

// ---------------------------------------------------------------------------------------------------------
// wire/decode.json

#[derive(Deserialize)]
struct DecodeFile {
    wire_msg: Vec<DecodeCase<MsgJson>>,
    protocol_msg: Vec<DecodeCase<ProtocolMsgJson>>,
    ticket: Vec<DecodeCase<TicketJson>>,
    admin_request: Vec<DecodeCase<AdminRequestJson>>,
    admin_reply: Vec<DecodeCase<AdminReplyJson>>,
    status: Vec<DecodeCase<StatusJson>>,
}

#[derive(Deserialize)]
struct Part {
    hex: String,
    repeat: usize,
}

#[derive(Deserialize)]
struct DecodeCase<J> {
    name: String,
    #[serde(default)]
    payload_hex: Option<String>,
    #[serde(default)]
    payload_parts: Option<Vec<Part>>,
    ok: bool,
    #[serde(default)]
    go_error: Option<String>,
    /// No `#[serde(default)]`: on a generic field it would add a `J: Default` bound; an `Option` field
    /// is optional anyway.
    value: Option<J>,
    #[serde(default)]
    canonical_hex: Option<String>,
    #[serde(default)]
    canonical_len: Option<usize>,
    #[serde(default)]
    canonical_blake3: Option<String>,
    #[serde(default)]
    null_element: bool,
}

impl<J> DecodeCase<J> {
    fn payload(&self) -> Vec<u8> {
        match (&self.payload_hex, &self.payload_parts) {
            (Some(h), _) => hx(h),
            (None, Some(parts)) => parts
                .iter()
                .flat_map(|p| hx(&p.hex).repeat(p.repeat))
                .collect(),
            (None, None) => panic!("{}: no payload", self.name),
        }
    }

    fn check_canonical(&self, label: &str, got: &[u8], f: &mut Failures) {
        match (
            &self.canonical_hex,
            self.canonical_len,
            &self.canonical_blake3,
        ) {
            (Some(h), _, _) => f.check(to_hex(got) == *h, || {
                format!("{label}: re-encoding {}, want {h}", to_hex(got))
            }),
            (None, Some(len), Some(b3)) => {
                f.check(got.len() == len && same_blake3(got, b3), || {
                    format!(
                        "{label}: re-encoding of {} bytes does not match ({len}, {b3})",
                        got.len()
                    )
                })
            }
            _ => f.fail(format!("{label}: case has no canonical encoding")),
        }
    }
}

static DECODE: LazyLock<DecodeFile> = LazyLock::new(|| golden::load_json("wire/decode.json"));

/// `codec.Unmarshal(payload, &T)`: the decision, the verbatim error, the value and the re-encoding.
fn run_decode<J, T: Struct + fmt::Debug>(
    what: &str,
    cases: &[DecodeCase<J>],
    decode: impl Fn(&[u8]) -> Result<T, DecodeError>,
    expected: impl Fn(&J) -> T,
    same: impl Fn(&T, &T) -> bool,
) {
    assert!(!cases.is_empty(), "{what}: no cases");
    let mut f = Failures::default();
    for c in cases {
        let label = format!("{what}/{}", c.name);
        match (decode(&c.payload()), c.ok) {
            (Ok(got), true) => {
                match &c.value {
                    Some(j) => {
                        let want = expected(j);
                        f.check(same(&got, &want), || {
                            format!("{label}: decoded {got:?}, want {want:?}")
                        });
                    }
                    None => f.fail(format!("{label}: ok case without a value")),
                }
                if !c.null_element {
                    c.check_canonical(&label, &marshal(&got), &mut f);
                }
            }
            (Err(e), false) => f.check(
                c.go_error.as_deref() == Some(e.to_string().as_str()),
                || format!("{label}: error {:?}, want {:?}", e.to_string(), c.go_error),
            ),
            (Ok(got), false) => f.fail(format!(
                "{label}: decoded {got:?}, want error {:?}",
                c.go_error
            )),
            (Err(e), true) => f.fail(format!("{label}: error {e}, want success")),
        }
    }
    f.finish(what);
}

fn framed(payload: &[u8]) -> Vec<u8> {
    let len = match u32::try_from(payload.len()) {
        Ok(n) if payload.len() <= MAX_FRAME => n,
        _ => panic!("payload of {} bytes does not fit a frame", payload.len()),
    };
    let mut frame = len.to_be_bytes().to_vec();
    frame.extend_from_slice(payload);
    frame
}

#[test]
fn decode_wire_msg() {
    run_decode(
        "wire/decode.json wire_msg",
        &DECODE.wire_msg,
        unmarshal::<Msg>,
        MsgJson::to_msg,
        |a, b| a == b,
    );
}

/// The same decisions through `read_msg` (errors prefixed `wire: decode frame: `).
#[tokio::test]
async fn decode_wire_msg_through_read_msg() {
    let mut f = Failures::default();
    for c in &DECODE.wire_msg {
        let label = format!("wire_msg/{}", c.name);
        let frame = framed(&c.payload());
        match (read_msg(&mut &frame[..]).await, c.ok) {
            (Ok(got), true) => {
                if let Some(j) = &c.value {
                    let want = j.to_msg();
                    f.check(got == want, || {
                        format!("{label}: read {got:?}, want {want:?}")
                    });
                }
            }
            (Err(e), false) => {
                let want = c
                    .go_error
                    .as_ref()
                    .map(|g| format!("wire: decode frame: {g}"));
                f.check(want.as_deref() == Some(e.to_string().as_str()), || {
                    format!("{label}: error {:?}, want {want:?}", e.to_string())
                });
            }
            (got, _) => f.fail(format!("{label}: read_msg gave {got:?}, want ok={}", c.ok)),
        }
    }
    f.finish("wire/decode.json wire_msg through read_msg");
}

#[test]
fn decode_protocol_msg() {
    run_decode(
        "wire/decode.json protocol_msg",
        &DECODE.protocol_msg,
        unmarshal::<ProtocolMsg>,
        ProtocolMsgJson::to_msg,
        |a, b| a == b,
    );
}

/// The same decisions through `read_protocol_msg` (errors prefixed `protocol: decode frame: `).
#[tokio::test]
async fn decode_protocol_msg_through_read_protocol_msg() {
    let mut f = Failures::default();
    for c in &DECODE.protocol_msg {
        let label = format!("protocol_msg/{}", c.name);
        let frame = framed(&c.payload());
        match (read_protocol_msg(&mut &frame[..]).await, c.ok) {
            (Ok(got), true) => {
                if let Some(j) = &c.value {
                    let want = j.to_msg();
                    f.check(got == want, || {
                        format!("{label}: read {got:?}, want {want:?}")
                    });
                }
            }
            (Err(e), false) => {
                let want = c
                    .go_error
                    .as_ref()
                    .map(|g| format!("protocol: decode frame: {g}"));
                f.check(want.as_deref() == Some(e.to_string().as_str()), || {
                    format!("{label}: error {:?}, want {want:?}", e.to_string())
                });
            }
            (got, _) => f.fail(format!(
                "{label}: read_protocol_msg gave {got:?}, want ok={}",
                c.ok
            )),
        }
    }
    f.finish("wire/decode.json protocol_msg through read_protocol_msg");
}

#[test]
fn decode_ticket() {
    run_decode(
        "wire/decode.json ticket",
        &DECODE.ticket,
        unmarshal::<Ticket>,
        TicketJson::to_ticket,
        |a, b| a == b,
    );
}

#[test]
fn decode_admin_request() {
    run_decode(
        "wire/decode.json admin_request",
        &DECODE.admin_request,
        unmarshal::<AdminRequest>,
        AdminRequestJson::to_request,
        same_admin_request,
    );
}

#[test]
fn decode_admin_reply_matrix() {
    run_decode(
        "wire/decode.json admin_reply",
        &DECODE.admin_reply,
        decode_admin_reply,
        AdminReplyJson::to_reply,
        |a, b| a == b,
    );
}

#[test]
fn decode_status_matrix() {
    run_decode(
        "wire/decode.json status",
        &DECODE.status,
        decode_status,
        StatusJson::to_status,
        |a, b| a == b,
    );
}

// ---------------------------------------------------------------------------------------------------------
// wire/frame_errors.json

#[derive(Deserialize)]
struct FrameErrorsFile {
    reads: Vec<ReadsCase>,
    writes: Vec<WriteCase>,
    expect: Vec<ExpectCase>,
}

#[derive(Deserialize)]
struct ReadsCase {
    name: String,
    source: String,
    stream_hex: String,
    reads: Vec<ReadJson>,
    protocol_reads: Vec<ProtocolReadJson>,
}

#[derive(Deserialize)]
struct ReadJson {
    result: String,
    #[serde(default)]
    msg: Option<MsgJson>,
    #[serde(default)]
    go_error: Option<String>,
}

#[derive(Deserialize)]
struct ProtocolReadJson {
    result: String,
    #[serde(default)]
    msg: Option<ProtocolMsgJson>,
    #[serde(default)]
    go_error: Option<String>,
}

#[derive(Deserialize)]
struct WriteCase {
    name: String,
    msg: MsgJson,
    data: Payload,
    ok: bool,
    #[serde(default)]
    go_error: Option<String>,
    bytes_written: usize,
    #[serde(default)]
    frame_blake3: Option<String>,
    #[serde(default)]
    head_hex: Option<String>,
}

#[derive(Deserialize)]
struct ExpectCase {
    name: String,
    stream_hex: String,
    #[serde(deserialize_with = "decimal_i64")]
    want: i64,
    ok: bool,
    #[serde(default)]
    msg: Option<MsgJson>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    go_error: Option<String>,
    #[serde(default)]
    remote: Option<RemoteErrorJson>,
}

/// The vectors' read result kinds.
fn wire_kind(e: &WireError) -> &'static str {
    match e {
        WireError::Eof => "eof",
        WireError::UnexpectedEof => "unexpected_eof",
        WireError::TooLarge(_) => "too_large",
        WireError::Short(_) => "short",
        WireError::Decode(_) => "decode",
        WireError::Io(_) => "io",
        WireError::Remote(_) => "remote",
        WireError::Protocol { .. } => "protocol",
        WireError::KeyLen { .. } => "key_len",
    }
}

fn protocol_kind(e: &ProtocolFrameError) -> &'static str {
    match e {
        ProtocolFrameError::Eof => "eof",
        ProtocolFrameError::UnexpectedEof => "unexpected_eof",
        ProtocolFrameError::TooLarge(_) => "too_large",
        ProtocolFrameError::Short(_) => "short",
        ProtocolFrameError::Decode(_) => "decode",
        ProtocolFrameError::Io(_) => "io",
    }
}

/// An error result: its kind and, when the vector has one, its text (`eof` carries none).
fn check_error_result(
    label: &str,
    kind: &str,
    text: String,
    want_kind: &str,
    want_text: Option<&str>,
    f: &mut Failures,
) {
    f.check(kind == want_kind, || {
        format!("{label}: result {kind} ({text}), want {want_kind}")
    });
    match want_text {
        Some(t) => f.check(text == t, || format!("{label}: error {text:?}, want {t:?}")),
        None => f.check(want_kind == "eof" && text == "EOF", || {
            format!("{label}: error {text:?} without a vector text")
        }),
    }
}

#[tokio::test]
async fn frame_errors_reads() {
    let file: FrameErrorsFile = golden::load_json("wire/frame_errors.json");
    let mut f = Failures::default();
    for c in &file.reads {
        let stream = hx(&c.stream_hex);
        let mut r = &stream[..];
        for (i, want) in c.reads.iter().enumerate() {
            let label = format!("{} ({}) read {i}", c.name, c.source);
            match (read_msg(&mut r).await, &want.msg) {
                (Ok(got), Some(j)) if want.result == "ok" => {
                    let m = j.to_msg();
                    f.check(got == m, || format!("{label}: read {got:?}, want {m:?}"));
                }
                (Ok(got), _) => f.fail(format!("{label}: read {got:?}, want {}", want.result)),
                (Err(e), _) => check_error_result(
                    &label,
                    wire_kind(&e),
                    e.to_string(),
                    &want.result,
                    want.go_error.as_deref(),
                    &mut f,
                ),
            }
        }
    }
    f.finish("wire/frame_errors.json reads");
}

#[tokio::test]
async fn frame_errors_protocol_reads() {
    let file: FrameErrorsFile = golden::load_json("wire/frame_errors.json");
    let mut f = Failures::default();
    for c in &file.reads {
        let stream = hx(&c.stream_hex);
        let mut r = &stream[..];
        for (i, want) in c.protocol_reads.iter().enumerate() {
            let label = format!("{} ({}) protocol read {i}", c.name, c.source);
            match (read_protocol_msg(&mut r).await, &want.msg) {
                (Ok(got), Some(j)) if want.result == "ok" => {
                    let m = j.to_msg();
                    f.check(got == m, || format!("{label}: read {got:?}, want {m:?}"));
                }
                (Ok(got), _) => f.fail(format!("{label}: read {got:?}, want {}", want.result)),
                (Err(e), _) => check_error_result(
                    &label,
                    protocol_kind(&e),
                    e.to_string(),
                    &want.result,
                    want.go_error.as_deref(),
                    &mut f,
                ),
            }
        }
    }
    f.finish("wire/frame_errors.json protocol_reads");
}

#[tokio::test]
async fn frame_errors_writes() {
    let file: FrameErrorsFile = golden::load_json("wire/frame_errors.json");
    let mut f = Failures::default();
    for c in &file.writes {
        let label = &c.name;
        let mut m = c.msg.to_msg();
        m.data = c.data.bytes();
        let mut out = Vec::new();
        match (write_msg(&mut out, &m).await, c.ok) {
            (Ok(()), true) => {
                f.check(out.len() == c.bytes_written, || {
                    format!(
                        "{label}: wrote {} bytes, want {}",
                        out.len(),
                        c.bytes_written
                    )
                });
                if let Some(h) = &c.head_hex {
                    f.check(to_hex(&out[..out.len().min(13)]) == *h, || {
                        format!(
                            "{label}: head {}, want {h}",
                            to_hex(&out[..out.len().min(13)])
                        )
                    });
                }
                match &c.frame_blake3 {
                    Some(b3) => f.check(same_blake3(&out, b3), || {
                        format!("{label}: frame blake3 differs")
                    }),
                    None => f.fail(format!("{label}: ok write without frame_blake3")),
                }
                match read_msg(&mut &out[..]).await {
                    Ok(got) => f.check(got == m, || format!("{label}: read back differs")),
                    Err(e) => f.fail(format!("{label}: read back failed: {e}")),
                }
            }
            (Err(e), false) => {
                f.check(
                    c.go_error.as_deref() == Some(e.to_string().as_str()),
                    || format!("{label}: error {:?}, want {:?}", e.to_string(), c.go_error),
                );
                f.check(out.len() == c.bytes_written && out.is_empty(), || {
                    format!("{label}: wrote {} bytes on error", out.len())
                });
                f.check(
                    encode_frame(&m).map_err(|e| e.to_string()).err() == c.go_error,
                    || format!("{label}: encode_frame disagrees with write_msg"),
                );
            }
            (got, _) => f.fail(format!("{label}: write_msg gave {got:?}, want ok={}", c.ok)),
        }
    }
    f.finish("wire/frame_errors.json writes");
}

#[tokio::test]
async fn frame_errors_expect() {
    let file: FrameErrorsFile = golden::load_json("wire/frame_errors.json");
    let mut f = Failures::default();
    for c in &file.expect {
        let label = &c.name;
        let stream = hx(&c.stream_hex);
        let msg = c.msg.as_ref().map(MsgJson::to_msg);
        match (expect(&mut &stream[..], c.want).await, c.ok) {
            (Ok(got), true) => f.check(msg.as_ref() == Some(&got), || {
                format!("{label}: expect gave {got:?}, want {msg:?}")
            }),
            (Err(e), false) => {
                let kind = c.kind.as_deref().unwrap_or("");
                check_error_result(
                    label,
                    wire_kind(&e),
                    e.to_string(),
                    kind,
                    c.go_error.as_deref(),
                    &mut f,
                );
                match (&e, &msg) {
                    (WireError::Remote(re), Some(m)) => {
                        f.check(*re == error_from_msg(m), || {
                            format!(
                                "{label}: remote error {re:?} is not error_from_msg of the frame"
                            )
                        });
                        f.check(as_remote(&e).is_some(), || {
                            format!("{label}: as_remote found nothing")
                        });
                        match &c.remote {
                            Some(j) => check_remote(label, re, j, &mut f),
                            None => f.fail(format!("{label}: remote case without remote")),
                        }
                    }
                    (WireError::Protocol { got, want }, Some(m)) => {
                        f.check(*got == m.typ && *want == c.want, || {
                            format!(
                                "{label}: protocol error type {got}, want {}; want {want}",
                                m.typ
                            )
                        });
                    }
                    (WireError::Remote(_) | WireError::Protocol { .. }, None) => {
                        f.fail(format!("{label}: {kind} case without the frame"));
                    }
                    _ => {}
                }
            }
            (got, _) => f.fail(format!("{label}: expect gave {got:?}, want ok={}", c.ok)),
        }
    }
    f.finish("wire/frame_errors.json expect");
}

// ---------------------------------------------------------------------------------------------------------
// admin/requests.json, admin/replies.json, status/status.json

#[derive(Deserialize)]
struct AdminRequestsFile {
    cases: Vec<AdminRequestCase>,
}

#[derive(Deserialize)]
struct AdminRequestCase {
    name: String,
    source: String,
    request: AdminRequestJson,
    params_hex: String,
    frame_hex: String,
}

#[derive(Deserialize)]
struct AdminRepliesFile {
    cases: Vec<AdminReplyCase>,
    decode: Vec<AdminReplyDecodeCase>,
}

#[derive(Deserialize)]
struct AdminReplyCase {
    name: String,
    source: String,
    reply: AdminReplyJson,
    status_hex: String,
    frame_hex: String,
}

#[derive(Deserialize)]
struct AdminReplyDecodeCase {
    name: String,
    status_hex: String,
    ok: bool,
    #[serde(default)]
    go_error: Option<String>,
    #[serde(default)]
    reply: Option<AdminReplyJson>,
}

#[derive(Deserialize)]
struct StatusFile {
    cases: Vec<StatusCase>,
    decode: Vec<StatusDecodeCase>,
}

#[derive(Deserialize)]
struct StatusCase {
    name: String,
    source: String,
    status: StatusJson,
    hex: String,
    #[serde(default)]
    null_element: bool,
}

#[derive(Deserialize)]
struct StatusDecodeCase {
    name: String,
    hex: String,
    ok: bool,
    #[serde(default)]
    go_error: Option<String>,
    #[serde(default)]
    status: Option<StatusJson>,
}

/// The stamp of the admin vectors: cid16 = 00..0f, incarnation 1, epoch 7.
fn cid16() -> Vec<u8> {
    (0u8..16).collect()
}

async fn check_frame(label: &str, m: &Msg, frame_hex: &str, f: &mut Failures) {
    let frame = hx(frame_hex);
    match encode_frame(m) {
        Ok(got) => f.check(got == frame, || {
            format!("{label}: frame {}, want {frame_hex}", to_hex(&got))
        }),
        Err(e) => f.fail(format!("{label}: encode_frame failed: {e}")),
    }
    match read_msg(&mut &frame[..]).await {
        Ok(got) => f.check(got == *m, || format!("{label}: read {got:?}, want {m:?}")),
        Err(e) => f.fail(format!("{label}: read_msg failed: {e}")),
    }
}

#[tokio::test]
async fn admin_requests() {
    let file: AdminRequestsFile = golden::load_json("admin/requests.json");
    let mut f = Failures::default();
    for c in &file.cases {
        let label = format!("{} ({})", c.name, c.source);
        let req = c.request.to_request();
        let params = hx(&c.params_hex);
        f.check(marshal(&req) == params, || {
            format!(
                "{label}: params {}, want {}",
                to_hex(&marshal(&req)),
                c.params_hex
            )
        });
        // The request as the wire carries it, which is what Go decodes: an empty (±0) omitempty garbage
        // is dropped and reads as +0, and every NaN is written `f9 7e00` and reads as 7ff8000000000000.
        let carried = AdminRequest {
            garbage: match req.garbage {
                0.0 => 0.0,
                g if g.is_nan() => f64::from_bits(0x7ff8_0000_0000_0000),
                g => g,
            },
            ..req.clone()
        };
        match unmarshal::<AdminRequest>(&params) {
            Ok(back) => f.check(same_admin_request(&back, &carried), || {
                format!("{label}: decoded {back:?}, want {carried:?}")
            }),
            Err(e) => f.fail(format!("{label}: decode failed: {e}")),
        }
        let m = Msg {
            typ: T_ADMIN,
            cluster_id: cid16(),
            incarnation: 1,
            epoch: 7,
            params,
            ..Msg::default()
        };
        check_frame(&label, &m, &c.frame_hex, &mut f).await;
    }
    f.finish("admin/requests.json");
}

#[tokio::test]
async fn admin_replies() {
    let file: AdminRepliesFile = golden::load_json("admin/replies.json");
    let mut f = Failures::default();
    for c in &file.cases {
        let label = format!("{} ({})", c.name, c.source);
        let reply = c.reply.to_reply();
        let status = hx(&c.status_hex);
        f.check(marshal(&reply) == status, || {
            format!(
                "{label}: payload {}, want {}",
                to_hex(&marshal(&reply)),
                c.status_hex
            )
        });
        match decode_admin_reply(&status) {
            Ok(back) => f.check(back == reply, || {
                format!("{label}: decoded {back:?}, want {reply:?}")
            }),
            Err(e) => f.fail(format!("{label}: decode failed: {e}")),
        }
        let m = Msg {
            typ: T_ADMIN_REPLY,
            incarnation: 1,
            epoch: 7,
            status,
            ..Msg::default()
        };
        check_frame(&label, &m, &c.frame_hex, &mut f).await;
    }
    for c in &file.decode {
        let label = format!("decode/{}", c.name);
        match (decode_admin_reply(&hx(&c.status_hex)), c.ok, &c.reply) {
            (Ok(got), true, Some(j)) => {
                let want = j.to_reply();
                f.check(got == want, || {
                    format!("{label}: decoded {got:?}, want {want:?}")
                });
            }
            (Err(e), false, _) => f.check(
                c.go_error.as_deref() == Some(e.to_string().as_str()),
                || format!("{label}: error {:?}, want {:?}", e.to_string(), c.go_error),
            ),
            (got, _, _) => f.fail(format!("{label}: decode gave {got:?}, want ok={}", c.ok)),
        }
    }
    f.finish("admin/replies.json");
}

#[test]
fn status_cases() {
    let file: StatusFile = golden::load_json("status/status.json");
    let mut f = Failures::default();
    for c in &file.cases {
        let label = format!("{} ({})", c.name, c.source);
        let st = c.status.to_status();
        let hex = hx(&c.hex);
        if !c.null_element {
            f.check(marshal(&st) == hex, || {
                format!(
                    "{label}: encoding {}, want {}",
                    to_hex(&marshal(&st)),
                    c.hex
                )
            });
        }
        match decode_status(&hex) {
            Ok(back) => f.check(back == st, || {
                format!("{label}: decoded {back:?}, want {st:?}")
            }),
            Err(e) => f.fail(format!("{label}: decode failed: {e}")),
        }
    }
    for c in &file.decode {
        let label = format!("decode/{}", c.name);
        match (decode_status(&hx(&c.hex)), c.ok, &c.status) {
            (Ok(got), true, Some(j)) => {
                let want = j.to_status();
                f.check(got == want, || {
                    format!("{label}: decoded {got:?}, want {want:?}")
                });
            }
            (Err(e), false, _) => f.check(
                c.go_error.as_deref() == Some(e.to_string().as_str()),
                || format!("{label}: error {:?}, want {:?}", e.to_string(), c.go_error),
            ),
            (got, _, _) => f.fail(format!("{label}: decode gave {got:?}, want ok={}", c.ok)),
        }
    }
    f.finish("status/status.json");
}
