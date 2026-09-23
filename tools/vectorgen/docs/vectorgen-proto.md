# vectorgen-proto: wire, pack, ticket, admin and status vectors

Owner: vectorgen-proto (L1). Every value comes from the Go libraries pinned in PORTING.md §0: dstore v0.1.10
(`wire`, `codec`, `ticket`, `client` error types, `node` payload types, `view.IDsOf`), transport-iroh v0.4.0
`protocol`, core v0.0.9 `amberpack`, `key`, `reference`, go-iroh v0.2.0 `key`, urfave/cli v2.27.7.

| Family | Files | Generator |
|---|---|---|
| `wire` | `wire/frames.json`, `wire/decode.json`, `wire/frame_errors.json` | `family_wire.go` |
| `wire-pack` | `wire/pack_frames.json`, `wire/pack_reader.json` | `family_wire.go` |
| `ticket` | `ticket/encode.json`, `ticket/parse.json`, `ticket/curve.json` | `family_ticket.go` |
| `admin` | `admin/requests.json`, `admin/replies.json`, `status/status.json` | `family_admin.go` |

```sh
nix develop -c go -C tools/vectorgen run . ../../tests/golden wire wire-pack ticket admin
```

**Self-checks** (generation fails when one does not hold):

- `frames.json` reproduces every verified frame of client-core §3.2-§3.3, client-transfer §3.1, the codec-wire-ticket
  §3.3 "other probe payloads" and verification §3.1, plus the client-core record and view and the client-transfer
  view.
- Every `wire_msg` decode case gives the same decision through `wire.ReadMsg` (error prefixed
  `wire: decode frame: `), every `protocol_msg` case through `protocol.ReadMsg` (`protocol: decode frame: `).
- `pack_frames.json` matches the client-transfer §3.2 SHA-256 values, the codec-wire-ticket §2.3.4 empty stream and
  the Go zstd record of client-transfer §3.3; every complete pack reads back through `NewPackReader` + amberpack
  `Records` with the stream positioned after TDataEnd.
- `ticket/encode.json` matches the codec-wire-ticket §2.4.1-§2.4.2 and cli §5.4 encodings.
- `frames.json` keeps every case name the generator adds, exactly once, as a `name` or an alias.
- `admin` matches every cli §3.8 request row and every reply row (hex and printed output), client-core §3.4 and
  verification §3.2 hex, and checks the module's `cmd/dstore/main.go` and `client.go`
  (found with `go list -m -f {{.Dir}}`) still hold the 17 `node.AdminRequest` literals, the flag definitions,
  `adminAction`'s print block and `printStatus`'s node lines that the generator copies.

## Conventions used by all these files

Per VECTORS.md: bytes are lowercase hex; Go `int`, `int64` and `uint64` values are decimal strings (read them with
`dstore_testkit::golden::decimal_i64` / `decimal_u64`); `uint8`/`uint16`/`uint32`, lengths and counts are JSON numbers;
payloads are `Payload` objects. Texts are exact Go output.

JSON field names are the Rust field names of PORTING.md §4.3-§4.4 (`cluster_id`, `has_expected`, `retry_after`, …).

### `MsgJSON` (`wire.Msg`)

Sparse: `typ` (decimal string) is always present; every other field is present only when non-zero, as Go's
omitempty encoding has it. A missing field is the zero value (`0`, `false`, `""`, empty bytes, empty list).

| Kind | Fields | JSON |
|---|---|---|
| `[]byte` | `cluster_id record data key view version expected_version expected_old prefix after next current reg ballot value accepted promised token nonce params status node` | hex |
| `uint64` | `incarnation epoch since g seq sent received marked` | decimal string |
| `int`/`int64` | `typ limit retry_after shortfall not_after` | decimal string |
| `uint32` | `weight` | number |
| `bool` | `pin force has_expected has_current has_value more no_vote expand idle` | boolean |
| `string` | `name code text zone error pattern` | string |
| `[][]byte` | `keys unreachable missing` | array of hex; a `null` element is a Go nil element (Rust `Vec<Vec<u8>>` holds it as empty, codec-wire-ticket R6) |
| `[]string` | `addrs deleted` | array of strings |
| `[]RefInfo` | `refs` | `{name, key: hex\|null, version: hex\|null, created_at: i64s, user}` |
| `[]KeyHolders` | `holders short` | `{key: hex\|null, holders: [hex\|null]}` |
| `[]KeyFailure` | `failed` | `{key: hex\|null, node: hex\|null, reason, retry_after: i64s}` |
| `[]KeyReject` | `rejected` | `{key: hex\|null, reason}` |
| `[]ScanRow` | `rows` | `{reg: hex\|null, promised: hex, accepted: hex, value: hex, has_value}` |

Inside the sub-structs every field is present. `null` is a nil non-omitempty `[]byte` (Rust `None`); `""` is an
empty non-nil one (`Some(vec![])`); omitempty byte fields are always strings.

### `ProtocolMsgJSON` (`protocol.Msg`)

Sparse like `MsgJSON`: `typ`, `name`, `root`, `cas`, `expected_old`, `record`,
`refs: [{name, key: hex|null, created_at: i64s, user}]`, `keys`, `data`, `key`, `code`, `text`, `current`, `token`,
`data_conns` (i64s), `data_ports` (numbers), `data_endpoints: [{id: hex|null, addrs: [string]}]`, `names`.

### `TicketJSON` (`ticket.Ticket`)

`{cluster_id: hex|null, incarnation: u64s, members: [{id: hex|null, addrs: [string]|null}]|null}`. `members: null`
is Rust `None`, `[]` is `Some(vec![])`; `addrs` null and `[]` both mean an empty `Vec<String>`.

### `AdminRequestJSON`, `AdminReplyJSON`, `StatusJSON`

Every field present. Omitempty byte fields are hex strings (`""` when empty), lists are never null.

- AdminRequest: `op, node, weight (number), zone, replicas (number), dead, allow_unsafe, force, key, garbage,
  garbage_bits, tolerate, forwarded, pause, rate (u64s), names`. `garbage` is Go's shortest `'g'` text
  (`"0.5"`, `"NaN"`, `"+Inf"`), `garbage_bits` the 16-hex-digit IEEE bits; tests use
  `f64::from_bits(u64::from_str_radix(garbage_bits, 16))`.
- AdminReply: `text, token, view, names, key, ticket, gc`.
- Status: `id (hex|null), epoch, incarnation, packs, records, bytes, pins, unreachable ([hex|null]), pending_packs,
  transition, gc, lease_holder, voters ([{id: hex|null, calls, failures, p99ms}]), writable, free_bytes,
  total_bytes, puts, gets, ref_puts, bytes_in, bytes_out, amnesiac, retired, scrub_age_sec, last_live, corrupt,
  is_holder, unaudited_keys, watchers`. The integers are decimal strings.

### `RemoteErrorJSON` (`wire.ErrorFromMsg`)

`{code, text, view: hex, retry_after_ns: i64s, retry_after: Duration.String(), error: Error()}`.
`retry_after_ns` is `RetryAfter * time.Millisecond`; a negative value stays negative in Go (Rust clamps to ZERO:
compare `retry_after_ns` only when it is ≥ 0).

### Fixtures

- `cid16` = `00 01 … 0f`; codec-wire-ticket `k1` = 11×32, `k2` = 22×32.
- client-core: record `R` = `reference.Reference{Name: "trees/a", Key: 01..20, User: "alice", CreatedAt:
  1700000000123456789}.Encode()`; view `V` of client-core §3.3.
- client-transfer: `cid` = 11×16, `k(s, n)` = `key.New(Blob, n, data(s, n))`, `idN` = 32 zero bytes with
  `[0] = [31] = N`, the minimal view of §3.1.
- verification: `id(s)` = `ed25519.NewKeyFromSeed(data(s, 32))` public key; `k(s)` = `k(s, 64)`.

## `wire/frames.json`

```
{ "requests": [FrameCase], "replies": [FrameCase], "other": [FrameCase],
  "bulk": [BulkCase], "keys32": [Keys32Case], "error_is": [ErrorIsCase], "is_code": [IsCodeCase] }
```

- **FrameCase** `{name, aliases?: [name], source, call?, decode_only: bool, null_element?: true, msg: MsgJSON, frame_hex,
  remote_error?: RemoteErrorJSON, keys32?: {ok, count, error?}, cas_mismatch?: string, incomplete?: {error, sample: [hex]},
  short_ids?: [[hex]], holders_ids?: [[hex]]}`.
  - `requests`: what the client sends (stamps, cond flags, the ref-watch known list with `version` null and
    `created_at` 0). `call` names the client call that builds it.
  - `replies`: node replies. Extras, computed from the decoded message: `remote_error` for TErr; `keys32` =
    `wire.Keys32(keys)` for missing-reply, absent and incomplete; `cas_mismatch` = `client.CASMismatch.Error()`;
    `incomplete` = `client.Incomplete.Error()` and its sample (`Keys32` result, empty on error);
    `short_ids`/`holders_ids` = `view.IDsOf` per entry (zero-padded or truncated to 32 bytes).
  - `other`: generic `Msg` encodings (integer boundaries, negative ints, nil elements, every field set).
  - Identical frames from several specs appear once: `aliases` names the other cases, `source` joins the sources.
    Every case name the generator adds is exactly one `name` or alias (self-checked).
  - `null_element: true` when a `[][]byte` list of the message (`keys`, `unreachable`, `missing`, or the `holders`
    of a `holders`/`short` entry) holds a nil element. Rust `Vec<Vec<u8>>` decodes it as empty and encodes `40` where
    Go writes `f6` (codec-wire-ticket R6), so such a frame cannot be re-encoded byte for byte (`other`
    `ref-changes-nil-elements`).
  - Tests: `encode_frame(msg) == frame_hex` unless `decode_only` or `null_element`; `read_msg(frame_hex) == msg`
    always (a `null` element compares as empty).
- **BulkCase** `{name, source, call, msg: MsgJSON (without keys), keys: Payload, key_count, frame_len, frame_blake3,
  head_hex}`: `keys` is `key_count` 32-byte keys concatenated; `head_hex` is the first 64 frame bytes.
- **Keys32Case** `{name, keys: [hex|null], ok, count, error?}`: `wire.Keys32`.
- **ErrorIsCase** `{name, err: {code, text}, wrapped, target: {code, text}, is}`: `errors.Is(err, &wire.Error{target})`,
  with `err` wrapped once by `fmt.Errorf("upload to 01000000: %w")` when `wrapped`.
- **IsCodeCase** `{name, kind: "wire"|"protocol", wrapped, code, text, query, is_code, error}`: `wire.IsCode` over a
  `*wire.Error` or `*protocol.RemoteError` (never matches), optionally wrapped with `fmt.Errorf("get: %w")`.

Rust: `tests/golden_tests/wire.rs` (encode/decode, keys32, error_is, is_code); the reply extras also serve client-a
and client-b tests.

## `wire/decode.json`

```
{ "wire_msg": [Case<MsgJSON>], "protocol_msg": [Case<ProtocolMsgJSON>], "ticket": [Case<TicketJSON>],
  "admin_request": [Case<AdminRequestJSON>], "admin_reply": [Case<AdminReplyJSON>], "status": [Case<StatusJSON>] }
Case = { name, field?, shape_hex?, payload_hex? | payload_parts?, ok, go_error?, value?, canonical_hex? |
         (canonical_len, canonical_blake3), null_element? }
```

- The decode is `codec.Unmarshal(payload, &T)` (`node.DecodeStatus` for status): the entry point of `ReadMsg`,
  `ticket.Parse`, the CLI's `admin` and `printStatus`. `go_error` is the exact text (no frame prefix).
- `payload_parts: [{hex, repeat}]` replaces `payload_hex` for payloads over 4096 bytes: concatenate `hex` repeated
  `repeat` times, in order.
- `value` (when `ok`): the decoded value in the target's JSON schema. `canonical_hex`: Go's re-encoding of it
  (`canonical_len` + `canonical_blake3` when over 4096 bytes). `null_element: true` when a `[][]byte` list of the
  value holds a nil element: Rust decodes it as empty and re-encodes `40` where Go writes `f6`, so skip the
  re-encoding comparison for those.
- **Matrix cases** (`name` starts with `matrix `, codec-wire-ticket G4): `field` is `<struct>.<key>` or
  `<struct>.<key>[].<key>` for an element of a struct list, `shape_hex` one of the 34 shapes
  `00 1bffffffffffffffff 20 3bffffffffffffffff 40 4161 60 6161 61ff 80 8101 81f6 814161 a0 f6 f7 f4 f5 f93e00
  fb3fb999999999999a f0 f8ff c24105 c249010000000000000000 c34101 c06161 c001 c16161 d82a4161 d9d9f74161
  5f41614162ff 7f6161ff 9f01ff bfff`. The payload is `a1 <key> <shape>` for a top-level field, or
  `a1 <list key> 81 a1 <key> <shape>` for a list element. Fields: `wire.Msg` 0-61, `wire.Msg.21[]` (RefInfo),
  `.23[]`/`.26[]` (KeyHolders), `.24[]` (KeyFailure), `.25[]` (KeyReject), `.39[]` (ScanRow); `protocol.Msg` 0-17,
  `.6[]` (RefInfo), `.16[]` (DataEndpointRec); `ticket.Ticket` 0-2, `.2[]` (Member); `node.AdminRequest` 0-14;
  `node.AdminReply` 0-6; `node.Status` 0-28, `.12[]` (VoterStat).
- **Structure cases** (G5 and verification §2.1/§5): the 14 probes, null/undefined, tag preambles, bignums,
  integer overflow per width, strings and chunks, simple values, arrays into bytes, every map-key type, duplicates,
  top-level types, truncations, invalid additional information on every major, `f8 00`-`f8 1f`, length overflow,
  nesting 29-33 (arrays, maps, tags), the 131072/131073 caps (definite and indefinite), pass-1-before-pass-2.

Rust: `tests/golden_tests/codec.rs`: `dstore_codec::unmarshal::<T>` must match `ok` and `go_error` exactly, give
`value`, and re-encode to `canonical_hex`.

## `wire/frame_errors.json`

```
{ "reads": [ {name, source, stream_hex, reads: [Read], protocol_reads: [ProtocolRead]} ],
  "writes": [ {name, msg: MsgJSON, data: Payload, ok, go_error?, bytes_written, frame_blake3?, head_hex?} ],
  "expect": [ {name, stream_hex, want: i64s, ok, msg?: MsgJSON, kind?, go_error?, remote?: RemoteErrorJSON} ] }
Read = {result, msg?: MsgJSON, go_error?}      ProtocolRead = {result, msg?: ProtocolMsgJSON, go_error?}
```

- `reads`: `wire.ReadMsg` (and `protocol.ReadMsg` for `protocol_reads`) called repeatedly on the stream until the
  first error, which is the last entry. `result`: `ok`, `eof` (io.EOF, no `go_error`), `unexpected_eof`
  (1-3 header bytes), `too_large`, `short` (`… short frame: EOF|unexpected EOF`), `decode`, `io`.
- `writes`: `wire.WriteMsg` of `{typ: 7, data}`; over `MaxFrame` it fails with `go_error` and `bytes_written` 0.
  `head_hex` is the first 13 frame bytes.
- `expect`: `wire.Expect(stream, want)`. `msg` is what Go returns alongside the error (TErr and a type mismatch
  return the frame). `kind`: `remote` (a `*wire.Error`, see `remote`), `protocol` (`wire: unexpected frame: type
  N, want M`), or a read result kind.

Rust: `tests/golden_tests/wire.rs` (`read_msg`, `read_protocol_msg`, `encode_frame`/`write_msg`, `expect`).

## `wire/pack_frames.json`

```
{ "cases": [ { name, source, records: [Record], record_seq?: {count, first_seed: u64s, len}, records_blake3,
               source_error_after?, go_error?, frames: [Frame], stream_len, stream_blake3, stream_hex?,
               read_back: {records, error?, next} } ] }
Record = {key, flags, ulen, slen, header_hex, payload: Payload, record_hex?}
Frame  = {type: 7|8, frame_len, head_hex, data_len, data_blake3, data_hex?}
```

- The stream is `wire.SendPackRecords` over the records in order. A record is `header_hex ‖ payload.bytes()`
  (`record_hex` too when ≤ 4096 bytes). Raw records (flags 0) hold `data(seed, n)` under `k(seed, n)`, so both
  implementations produce them identically (DD-1); `zstd-and-raw` carries Go's zstd record verbatim.
- `record_seq` replaces `records` for `10000-records-of-100`: record i is the raw record of
  `data(first_seed + i, len)` with key `k(first_seed + i, len)`:
  `01 ‖ key ‖ 00 ‖ u32be(len) ‖ u32be(len) ‖ crc32c ‖ data`, the CRC-32C taken over the record with the CRC field
  zero. `records_blake3` hashes all records concatenated.
- `source_error_after: i`: the source yields an error in place of record `i`; `go_error` is its text, and the frames
  are what reached the writer (no TDataEnd). Go's amberpack writer buffers up to 4096 bytes below the chunk writer,
  so only completed 1 MiB chunks are written before the error; the cases keep the chunk boundary away from that
  window, where a Rust `PackSender` dropped without `finish()` writes the same frames.
- `frames`: `frame_len` is the u32 length header, `head_hex` the frame bytes before the data (header, CBOR map head,
  keys, bstr head), `data_hex` when `data_len` ≤ 256.
- `read_back`: `NewPackReader` + amberpack `Records` (count, or the error text), `io.Copy(io.Discard, pr)` (an error
  is `drain: <text>`), then `wire.ReadMsg` on the stream (`next`, a Read result kind).

Rust: `tests/golden_tests/wire_pack.rs`: `PackSender` produces `stream_blake3` (and `stream_hex`); `PackRecords`
reads the records back.

## `wire/pack_reader.json`

```
{ "cases": [ { name, source, stream_hex,
               read: {read_size, data_hex, end: End, again: End},
               records: {records: [{key, flags, ulen, slen, bytes_hex}], error?, drain_error?, next: Read} } ] }
End = {kind: "eof"|"frame"|"remote"|"unexpected", frame_kind?, error?, remote?: {code, text, current}, unexpected_type?: i64s}
```

- `read`: `wire.NewPackReader` read with a `read_size` buffer until an error; `data_hex` is every byte read, `end`
  the error, `again` one more `Read` (errors are sticky). `eof` covers TDataEnd and a clean end of stream at a frame
  boundary. `frame` is a `protocol.ReadMsg` error (`frame_kind` as in frame_errors). `remote` is a TErr
  (`protocol.RemoteError`, text always `remote: <code>: <text>`). `unexpected` is
  `protocol: unexpected frame: type N during pack transfer`.
- `records`: a fresh `NewPackReader` under amberpack `Records`: the records yielded, then `error` (the one error
  text). Records wraps read errors with `%v`, e.g. `amberpack: malformed pack stream: truncated before end marker:
  remote: busy: late`. Then `drain_error` = `io.Copy(io.Discard, pr)` (nil on EOF), then `next` = `wire.ReadMsg`.
- Cases: TData without data, EOF at a boundary and mid-frame, oversize frames, dstore TErr with and without text and
  with View and RetryAfter (unstamped, and stamped: key 2 is `protocol.Msg.Root`), dstore TAbsent/TRefs, TData
  carrying Epoch, type 51 and TWants mid-pack, bad and short magic, oversized, truncated, bad-tag, CRC, ulen and
  reserved-key records, missing end marker, end marker in its own frame, data and TErr after the end marker, TErr
  and EOF mid-record, a zstd record.

Rust: `tests/golden_tests/wire_pack.rs` (`PackReader::read`, `PackRecords::next`, `drain`).

## `ticket/encode.json`

`{ "cases": [ {name, source, ticket: TicketJSON, cbor_hex, encoded, ids, parse: ParseResult} ] }` with
`ParseResult = {ok, error?, ticket?: TicketJSON, encoded?, ids}` = `ticket.Parse(encoded)` (and the re-encoding).
Tests: `marshal(ticket) == cbor_hex`, `ticket.encode() == encoded`, `ticket.ids() == ids`, `parse(encoded)`.

## `ticket/parse.json`

`{ "cases": [ {name, source, input?, input_hex, ok, error?, ticket?, encoded?, ids} ] }`: `ticket.Parse(input)`.
`input_hex` always holds the bytes; `input` is present only when they are valid UTF-8. The error text is exact (the
CLI prints it). Cases: codec-wire-ticket §2.4.9 and G7, verification §5, the cli §5.2 ticket inputs, all 32
last-symbol variants of a ticket body and of a base32 id, base32 ids of 48-60 symbols, the lookalikes U+0130, U+0131,
U+017F and U+212A in ids and bodies (written as `\u` escapes in the generator; Go accepts all four in an id, and only
U+0131/U+017F in a body, where `ToUpper` keeps U+0130 and U+212A), every `unicode.IsSpace` rune around an id and a ticket, U+180E/U+200B/U+FEFF (not trimmed), separators,
newlines in bodies, extra symbols after a full quantum, 64-byte fields with non-ASCII runes, crafted CBOR bodies and
invalid UTF-8.

## `ticket/curve.json`

`{ "cases": [ {name, bytes, valid, hex_parse_error?, base32_parse_error?} ] }`: `valid` is go-iroh
`key.NewPublicKey(bytes)` (filippo edwards25519 `SetBytes`); the errors are `key.ParseEndpointID` of the 64-hex and
52-symbol base32 forms. Cases: all-zero, all-0xff, small y with both signs, y = p-1, the non-canonical y = p..p+18
with both signs, 07×32, ab×32, id(1..8) and their sign flips, 64 splitmix encodings. Tests: `is_valid_public_key`
and `parse_endpoint_id` agree (PORTING §4.4 requires `iroh_base::PublicKey::from_bytes` to accept exactly this set).

## `admin/requests.json`

`{ "cases": [ {name, source, argv: [string]|null, request: AdminRequestJSON, params_hex, frame_hex} ] }`

- `argv` is run through a urfave/cli v2.27.7 app holding the admin subcommands with cmd/dstore's flag definitions and
  argument handling; the action captures `node.AdminRequest` instead of dialing. Flags must precede positional
  arguments (urfave stops parsing at the first one: `node-remove-flags-after-id-ignored`).
- `argv: null`: requests with no CLI producer (`rate-cap`, `keep`, `Force`/`Forwarded`, integer boundaries).
- `params_hex` = `codec.Marshal(request)`; `frame_hex` = the TAdmin frame stamped with (cid16, 1, 7).

Rust: `tests/golden_tests/wire.rs` (`marshal(AdminRequest)`, frame); cli-admin tests map `argv` to requests.

## `admin/replies.json`

```
{ "cases": [ {name, source, reply: AdminReplyJSON, status_hex, frame_hex, printed, printed_token_create,
              cluster_ticket?: {ok, error?, printed?, printed_ids?}} ],
  "decode": [ {name, status_hex, ok, go_error?, reply?, printed?} ] }
```

- `frame_hex`: TAdminReply `{57: status_hex}` stamped (1, 7). `printed`: `adminAction`'s stdout (Text line, each
  name, `key %x` for a 32-byte key). `printed_token_create`: `fmt.Println(r.Text)`. `cluster_ticket`: `cluster
  ticket`'s `ticket.Parse(r.Ticket)` then `Encode()` / `IDs()` plus `\n`, or the error.
- `decode`: `codec.Unmarshal` of hand-made payloads (unknown keys, break, null, type errors, duplicates).

## `status/status.json`

```
{ "cases": [ {name, source, status: StatusJSON, hex, null_element?: true, node: Node, printed} ],
  "decode": [ {name, hex, ok, go_error?, status?, node: Node, printed} ],
  "unreachable": [ {name, node: Node, error, printed} ] }
Node = {id, weight, zone, voter, writable, line}
```

- `hex` = `codec.Marshal(status)`. `null_element: true` when `unreachable` holds a nil element (`f6` in `hex`;
  Rust encodes `40`, R6): compare `decode_status(hex)` but skip `marshal(status) == hex` for those
  (`unreachable-odd-ids`). `node.zone` is always valid UTF-8 (the Rust view zone is a `String`).
- `line` = `printStatus`'s node line
  (`  %s weight %d zone %q voter=%v writable=%v`); `printed` = its per-node output for this status blob: the line,
  `      epoch …` with `[lease holder]`/`[AMNESIAC]`/`[retired]`, `cannot reach`, `gc`, `transition` (not `""`/`idle`)
  and `voter` lines, or `<line> — bad status` when `node.DecodeStatus` fails, or `<line> — unreachable: <error>`.

Rust: `tests/golden_tests/wire.rs` (`decode_status`, `marshal(Status)`); cli-admin tests (`print_status`).
