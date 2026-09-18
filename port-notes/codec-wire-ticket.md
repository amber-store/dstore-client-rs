# Port spec: deterministic CBOR codec, wire protocol, ticket

Normative Go: `github.com/amber-store/dstore` v0.1.9 (368f2c7), plus
`github.com/amber-store/transport-iroh@v0.4.0/protocol`,
`github.com/fxamacker/cbor/v2@v2.9.3`, `github.com/tmc/go-iroh@v0.2.0/key`,
and the Go standard library of the toolchain dstore pins (`go 1.26.5`,
dstore `go.mod:3`). The `encoding/base32` decode loop was diffed between the
local go1.23.12 and the go1.26.5 toolchain in the module cache: identical.

Items marked **(probe)** were confirmed by running a throwaway Go program
(go1.26.5, `replace` onto the dstore checkout, in a scratch directory under
`/private/tmp/claude-502`, deleted afterwards). Output is quoted verbatim.

---

## 1. Scope

### 1.1 Go files covered

| File | Lines | What it does |
|---|---:|---|
| dstore `codec/codec.go` | 38 | `encMode = cbor.CanonicalEncOptions().EncMode()`, `decMode = cbor.DecOptions{}.DecMode()`; `Marshal`, `Unmarshal`, `MustMarshal` |
| dstore `wire/wire.go` | 395 | ALPNs, limits, frame numbers, error codes, `Msg` and sub-structs, `WriteMsg`/`ReadMsg`, `Error`, `Expect`, pack re-exports, `Keys32`/`RawKeys`/`RawIDs`, `CloseStream` |
| dstore `wire/wire_test.go` | 90 | frame round trip, error frames, pack interop |
| dstore `ticket/ticket.go` | 96 | `Ticket`/`Member`, `Encode`, `IDs`, `Parse`, `parseIDs` |
| dstore `ticket/ticket_test.go` | 80 | `TestRoundTrip`, `TestParseIDs` |
| transport-iroh `protocol/protocol.go` | 180 | its own `Msg` (the decoder `packReader` uses), `WriteMsg`/`ReadMsg`, `RemoteError`, constants |
| transport-iroh `protocol/pack.go` | 141 | `chunkWriter`, `SendPack`, `SendPackRecords`, `packReader` |
| fxamacker/cbor v2.9.3 `encode.go` 2308, `decode.go` 3300, `valid.go` 394, `cache.go` 413, `structfields.go` 303, `common.go` 191, `decode_map_utils.go` 98 | | the behaviour dstore relies on, §2.1 |
| go-iroh v0.2.0 `key/key.go` 524, `key/key_core.go` 37 | | `ParseEndpointID` |
| go-iroh v0.2.0 `iroh/conn.go` (Stream wrapper, lines 55-95) | | `Close`/`CloseWrite` = FIN, `CancelRead` = STOP_SENDING |
| Go stdlib `encoding/base32/base32.go` (`decode`, `DecodeString`, `stripNewlines`), `strings.TrimSpace/ToLower/ToUpper/FieldsFunc`, `%q` | | ticket parsing |

Consumers read to pin the field sets and stream conventions:
`client/client.go` (403), `client/refs.go` (150), `client/watch.go` (245),
`client/objects.go` (395), `client/fetch.go` (309), `client/progress.go` (181);
`transport/transport.go` (`Pool.Call`/`Open`, 189-243); `node/server.go` (264),
`node/data.go` (623), `node/put.go` (360), `node/refs.go` (422),
`node/watch.go` (310), `node/status.go` (124), `node/admin.go` (302),
`node/join.go` (TJoin); `cmd/dstore/client.go` (574; `admin`, `printStatus`),
`cmd/dstore/main.go` (ticket commands 295-410, seed parse 489),
`cmd/dstore/wc.go:93`; `worktree/flow.go:306-331` (`TicketFromView`,
`RefreshTicket`); `paxos/acceptor.go`, `paxos/proposer.go` (cluster frames).

### 1.2 Who uses what on the client side

- **codec**: every frame; ticket bytes; `node.AdminRequest` into `Msg.Params`
  (`client/client.go:375`); `node.AdminReply` decode
  (`cmd/dstore/client.go:98-108`); `node.Status` decode (`node/status.go:112-116`,
  used by `cmd/dstore/client.go:158`); `view.Encode`/`view.Decode` (view spec).
- **wire**: `client` package, `transport.Pool`, `cmd/dstore/main.go:171` (ALPN list).
- **protocol**: `SendPackRecords`/`NewPackReader` (through `wire`), and the
  constants `TData`/`TDataEnd`/`TErr`/`ChunkSize`.
- **ticket**: `client.Config.Ticket` (`client/client.go:25`, `86-111`),
  `cmd/dstore/client.go:60-73`, `cmd/dstore/wc.go:93`,
  `cmd/dstore/main.go:311,340-363,489`, `worktree/flow.go:306-331`.

---

## 2. API used client-side

### 2.1 codec: fxamacker/cbor v2.9.3 as dstore configures it

Every dstore record is a Go struct whose fields carry
`cbor:"N,keyasint"` or `cbor:"N,keyasint,omitempty"` tags. There are no
embedded structs, maps, pointers (except the top-level `*Msg`), `time.Time`,
`big.Int` or marshaler types. Field types in use: `int`, `int64`, `uint64`,
`uint32`, `uint8`, `bool`, `string`, `float64` (only `AdminRequest.Garbage`),
`[]byte`, `[][]byte`, `[]string`, `[]struct`.

#### 2.1.1 Encoding (`codec.Marshal`, `codec.MustMarshal`)

Options: `CanonicalEncOptions()` (`encode.go:631-639`): `Sort: SortCanonical`
(= `SortLengthFirst`, `encode.go:181,196`), `ShortestFloat16`,
`NaNConvert7e00`, `InfConvertFloat16`, `IndefLengthForbidden`. Everything else
is the zero value: `NilContainers = NilContainerAsNull` (`encode.go:397`),
`OmitEmpty = OmitEmptyCBORValue` (`encode.go:419`),
`String = StringToTextString` (`encode.go:216`).

- **E1** A top-level `*Msg` is dereferenced (`getEncodeIndirectValueFunc`,
  `encode.go:2096-2114`). A struct value (`Ticket`, `AdminRequest`) encodes the same way.
- **E2** A struct becomes a definite CBOR map (major 5). Its head is the
  shortest form of the number of fields actually written: the head is
  reserved for all fields, then rewritten and the body shifted when fields
  were omitted (`encode.go:1539-1622`).
- **E3** Field order is ascending by the bytes of each field's encoded key.
  With only unsigned integer keys, length-first and bytewise orders coincide
  (`cache.go:347-357`, `214-223`). **Rule: emit present fields in ascending numeric key order.**
- **E4** A key is an unsigned-integer head (`cache.go:309-315`).
- **E5** `omitempty` (`encode.go:1561-1569`, `isEmpty*` at `2120-2201`) omits:
  bool `false`; any int/uint `== 0`; float `== 0.0` (so `-0.0` too); string
  of length 0; slice of length 0, **nil or empty**; nil pointer/interface; a
  struct only when every field is `omitempty` and empty (unused here).
- **E6** Without `omitempty` the field is always written:
  - nil `[]byte` or nil `[]T` → `f6` (null) (`encode.go:1287-1290`, `1330-1333`);
  - empty non-nil `[]byte` → `40`, empty non-nil `[]T` → `80` (`encode.go:1297-1300`, `1337-1340`);
  - `0` → `00`, `false` → `f4`, `""` → `60`.
- **E7** Integers: Go `int`…`int64` → major 0 when `>= 0`, else major 1 with argument `-1-v`
  (`encode.go:1089-1101`); `uint*` → major 0 (`1103-1109`). The head is
  always shortest (`encodeHead`, `encode.go:1926-1967`): n ≤ 23 in one byte,
  ≤ 0xff as `x|24, n`, ≤ 0xffff as `x|25` + 2 BE bytes, ≤ 0xffffffff as `x|26` + 4, else `x|27` + 8.
  This is exactly core-rs `cbor::append_head` (`src/cbor.rs:125-142`).
- **E8** bool → `f4` / `f5` (`encode.go:1077-1087`).
- **E9** `[]byte` → major 2 (`encode.go:1285-1310`); `string` → major 3,
  raw bytes, **no UTF-8 check on encode** (`1312-1320`).
- **E10** `[]string`, `[][]byte`, `[]struct` → definite major 4 of the
  element encodings. A nil `[]byte` element inside `[][]byte` → `f6`
  (probe: `Unreachable: [][]byte{nil, {}}` → `181b 82 f6 40`).
- **E11** float64: NaN → `f9 7e00`; +Inf → `f9 7c00`; -Inf → `f9 fc00`; if
  `float64(float32(f)) != f` → `fb` + 8 BE bytes; else if the float32 is exact
  as float16 → `f9` + 2; else `fa` + 4 (`encode.go:1111-1172`). Probe:
  `0.5` → `f9 3800`; `0.1` → `fb 3fb999999999999a`; `3.4e38` → `fb 47eff933c78cdfad`.
- **E12** Encoding cannot fail for these types, so `MustMarshal` never panics.

#### 2.1.2 Decoding (`codec.Unmarshal` = `DecOptions{}.DecMode()`)

Defaults that matter (`decode.go:772-1100`): `DupMapKeyQuiet`;
`IndefLengthAllowed`; `TagsAllowed`; `MaxNestedLevels = 32`;
`MaxArrayElements = 131072`; `MaxMapPairs = 131072` (`decode.go:988-1055`);
`UTF8RejectInvalid`; `FieldNameMatchingPreferCaseSensitive` (it never matches
keyasint fields, `decode_map_utils.go:52-63`); `ByteStringToStringForbidden`;
`FieldNameByteStringForbidden`; no `ExtraReturnErrors`, so unknown keys are
ignored; NaN/Inf allowed; every well-formed simple value allowed.

`Unmarshal` runs two passes (`decode.go:1291-1303`). **Pass 1 checks the whole
input before any field is decoded, so its errors take precedence over type errors.**

**Pass 1: well-formedness** (`valid.go:88-213`). Verbatim errors:

- **W1** Empty input → `io.EOF`, text `EOF`.
- **W2** Any truncation (head argument, string body, missing items or tag
  content) → `io.ErrUnexpectedEOF`, text `unexpected EOF`.
- **W3** Additional info 28/29/30 on any major →
  `cbor: invalid additional information <ai> for type <type>` (`valid.go:376-377`).
  ai 31 on major 0, 1 or 6 → same message (`valid.go:366-369`). A `ff` byte
  outside an indefinite container → `cbor: unexpected "break" code` (`valid.go:370-371`).
- **W4** `f8 xx` with xx < 32 → `cbor: invalid simple value <xx> for type primitives` (`valid.go:315-317`).
- **W5** Definite string length not fitting Go `int` →
  `cbor: byte string length <n> is too large, causing integer overflow`
  (`UTF-8 text string` for text) (`valid.go:116-120`). A length longer than the input → W2.
- **W6** Array/map: depth is incremented, then `> 32` →
  `cbor: exceeded max nested level 32`. A length not fitting `int` →
  `cbor: array length <n> is too large, it would cause integer overflow`
  (`map length` for maps) (`valid.go:139-143`). A count `> 131072` →
  `cbor: exceeded max number of elements 131072 for CBOR array` /
  `cbor: exceeded max number of key-value pairs 131072 for CBOR map`, checked
  **right after the head, before any element** (`valid.go:145-153`; probe
  `a1 04 9a00020001` and `ba00020001`).
- **W7** Indefinite strings (major 2/3, ai 31): chunks until `ff`. A chunk of
  another major → `cbor: wrong element type <chunk type> for indefinite-length <type>`;
  an indefinite chunk → `cbor: indefinite-length <type> chunk is not definite-length` (`valid.go:216-239`).
- **W8** Indefinite arrays/maps: items until `ff`, limits checked while
  counting (array `i > 131072`; map `i/2 > 131072` at even i); a map with an
  odd item count → `cbor: unexpected "break" code` (`valid.go:242-276`).
- **W9** Tags: a tag chain is scanned iteratively and every tag after the
  first adds one nesting level (`> 32` → nested-level error); a tag without
  content → W2 (`valid.go:173-209`). Built-in tag content types are **not**
  checked in pass 1 (`Unmarshal` passes `checkBuiltinTags=false`, `decode.go:1296`).
- **W10** Floats of every width, NaN and Inf are accepted.
- **W11** Bytes left after the item →
  `cbor: <n> bytes of extraneous data starting at index <i>` (`valid.go:74-82`, `94-96`).

CBOR type names inside messages (`common.go:25-46`): `positive integer`,
`negative integer`, `byte string`, `UTF-8 text string`, `array`, `map`, `tag`, `primitives`.

**Pass 2: value decoding** (`parseToValue`, `decode.go:1404-1704`). Decoding
continues after a type error and returns the **first** error in document
order (`parseMapToStruct` `2756-2897`, `parseArrayToSlice` `2372-2380`).
Pass 1 has already guaranteed structure, so stopping at the first error in
document order gives the same result (probe `continue-after-error`: later
fields are still filled, but `ReadMsg` and `ticket.Parse` discard the value on any error).

- **D1** null `f6` / undefined `f7` as a value: slices become nil;
  int/uint/bool/string stay zero; **no error** (`fillNil`, `decode.go:3061-3068`).
  A top-level `f6` or `f7` gives a zero `Msg` with no error (probe).
- **D2** Tag preamble for every decoded value (`decode.go:1456-1475`): strip
  leading 55799 tags (probe `d9d9f7 a1 00 1820` → Type 32). Then check every
  tag in the chain against the head of its immediate content: tag 0 needs a
  text string; tag 1 needs uint, negint or a float head `f9..fb`; tags 2/3
  need a byte string (`common.go:137-183`). Messages:
  `cbor: tag number 0 must be followed by text string, got <type>`,
  `cbor: tag number 1 must be followed by integer or floating-point number, got <type>`,
  `cbor: tag number 2 or 3 must be followed by byte string, got <type>`.
- **D3** Tagged values (`decode.go:1619-1677`):
  - tag 2 into an integer field: the magnitude if it fits, else
    `cbor: cannot unmarshal tag into Go struct field wire.Msg.2 of type uint64 (18446744073709551616 overflows uint64)` (probe);
  - tag 2 or 3 into `[]byte`: the raw content bytes;
  - tag 3 into a uint field, or any bignum into string/bool: type error
    `cbor: cannot unmarshal tag into Go struct field wire.Msg.6 of type string` (probe);
  - any other tag number: its content is decoded into the same target
    (probe `a1 00 d82a 1820` → Type 32; `a1 04 81 d82a 4101` → Keys `[01]`).
- **D4** Positive integer (`fillPositiveInt`, `decode.go:3070-3113`): into
  `int`/`int64`, values `> MaxInt64` fail with detail `(<n> overflows int)` or
  `int64`; into `uint64`/`uint32`/`uint8` a per-width check, e.g.
  `cbor: cannot unmarshal positive integer into Go struct field wire.Msg.43 of type uint32 (4294967296 overflows uint32)`
  (probe); into bool/string/slices a type error without detail.
- **D5** Negative integer (`decode.go:1544-1564`, `fillNegativeInt`
  `3115-3139`): an argument `> MaxInt64` fails with
  `cbor: cannot unmarshal negative integer into Go struct field wire.Msg.0 of type int (-18446744073709551616 overflows Go's int64)`
  (probe); into signed fields the value `-1-n` (per-width check); into any uint field
  `cbor: cannot unmarshal negative integer into Go struct field wire.Msg.2 of type uint64` (probe).
- **D6** Byte string (`fillByteString`, `decode.go:3165-3224`): into `[]byte`
  a copy, **non-nil even when empty**; into `string` →
  `cbor: cannot unmarshal byte string into Go struct field wire.Msg.6 of type string` (probe).
  Indefinite chunks are concatenated (probe `07 5f 4161 4162 ff` → `6162`).
- **D7** Text string (`decode.go:2312-2337`, `3226-3249`): content must be
  valid UTF-8, else `cbor: invalid UTF-8 string`, per chunk for indefinite
  strings (probe). Into `string`, or into `[]byte` →
  `cbor: cannot unmarshal UTF-8 text string into Go struct field wire.Msg.7 of type []uint8` (probe).
- **D8** Primitives (`decode.go:1582-1617`): a float, or false/true, into a
  non-matching field → `cbor: cannot unmarshal primitives into Go struct field <F> of type <T>`.
  Other simple values (0-19 in one byte, 32-255 in two bytes) into
  integer fields give their **numeric value** (probe `a1 00 f0` → Type 16);
  into string/[]byte/bool they are a type error.
- **D9** Array (`decode.go:1679-1690`, `2361-2381`):
  - into a slice, element-wise. A new slice is allocated when the target is
    nil or the count is 0, so `80` yields an **empty non-nil** slice (probe `04 80` → `keysnil=false`);
  - into `[]byte`, element-wise uint8 per D4/D8; a null element becomes 0 (probe `07 81 f6` → Record `00`);
  - into a struct at top level → `cbor: cannot unmarshal array into Go value of type wire.Msg (cannot decode CBOR array to struct without toarray option)` (probe);
  - into a scalar field → `cbor: cannot unmarshal array into Go struct field wire.Msg.6 of type string` (probe).
- **D10** Map into struct (`parseMapToStruct`, `decode.go:2714-2898`), per key type:
  - **unsigned or negative int**: an argument `> MaxInt64` records
    `cbor: cannot unmarshal positive integer into Go value of type int64 (18446744073709551615 overflows Go's int64)`
    or `cbor: cannot unmarshal negative integer into Go value of type int64 (-1-18446744073709551615 overflows Go's int64)`
    (probe) and skips the value. Otherwise the key is matched numerically;
    an unmatched value is skipped unexamined.
  - **text string**: parsed with the UTF-8 check (`cbor: invalid UTF-8 string`, probe
    `a1 61ff 00`), then matched against non-keyasint names. There are none,
    so the value is skipped.
  - **byte string or any other type** →
    `cbor: cannot unmarshal byte string into Go value of type string (map key is of type byte string and cannot be used to match struct field name)`
    (`primitives` for a float key); key and value are skipped (`decode.go:2862-2893`, probe).
  - **duplicate matched key**: the second value is skipped **without a type
    check**; the first value wins (`checkDupField`, `decode_map_utils.go:21-30`;
    probe `a2 00 1820 00 6161` → Type 32, no error).
  - Skipped values are never checked for UTF-8 or type (`skip`,
    `decode.go:2925-2958`; probe `a1 1863 61ff` → no error).
  - A map into a non-struct field → `cbor: cannot unmarshal map into Go struct field wire.Msg.6 of type string` (probe).
- **D11** Error text (`decodeToStructField`, `decode.go:2703-2710`;
  `UnmarshalTypeError.Error`, `decode.go:175-188`):
  `cbor: cannot unmarshal <cbor type> into Go struct field <Struct>.<key> of type <go type>`
  plus ` (<detail>)` when present; with no field:
  `cbor: cannot unmarshal <cbor type> into Go value of type <go type>`.
  - `<go type>` is the type of the innermost value that failed (element type
    for slice elements: `[]uint8` for a `[][]byte` element, `uint8` for a
    `[]byte` element, `string` inside a `RefInfo`).
  - `<Struct>.<key>` is **rewritten at each enclosing struct field on the way
    out**, so it names the **outermost** struct field (probe:
    `cbor: cannot unmarshal byte string into Go struct field wire.Msg.21 of type string`
    for a bad `RefInfo.Name`; `... ticket.Ticket.2 of type []uint8` for a bad
    `Member.ID`; `... wire.Msg.24 of type int64` for a bad `KeyFailure.RetryAfter`).
  - Errors that carry no struct field (map-key errors, D2 tag-content errors,
    UTF-8 errors, pass-1 errors) are not rewritten.
- **D12** A top-level value that is not a map, null or tag is a type error
  without a field: `cbor: cannot unmarshal positive integer into Go value of type wire.Msg` (probe).

**Limits** (my analysis of producers):

- Arrays are capped at 131072 elements and maps at 131072 pairs; any larger
  listing fails to decode on either side.
- Go producers stay within: missing/get ≤ 8192 keys (`wire.MaxKeys`,
  `node/data.go:28`, `195`); client put batches ≤ 8192 keys
  (`client/objects.go:22`, `165`); ref-list pages ≤ 20000 entries and ~4 MiB
  (`node/refs.go:80`, `93`); ref-changes frames flushed at 4 MiB (`node/watch.go:278-292`);
  incomplete samples ≤ 64 (`node/refs.go:162-165`).
- A Rust client must keep put batches at ≤ 8192 keys, as Go does: a
  `TPutResult.Holders` over 131072 entries would not decode.
- The `TRefWatch` known list is a single frame: ≤ 16 MiB **and** ≤ 131072 `RefInfo` entries.
  The design note's "about 200k references" (ref-watch design line 79) is
  wrong; the element cap is tighter.
- Nesting in use: `Msg` 1 → `Refs` 2 → `RefInfo` 3; `Ticket` 1 → `Members` 2 → `Member` 3 → `Addrs` 4.

### 2.2 wire (`wire/wire.go`)

#### 2.2.1 Constants (verbatim, `wire.go:21-129`)

```go
ALPNClient  = "amber-dstore/1"
ALPNCluster = "amber-dstore-cluster/1"
ALPNGateway = "amber-store-iroh/1"

MaxFrame     = 16 << 20   // 16777216
MaxKeys      = 8192
MaxPutBatch  = 64 << 20   // 67108864
MaxPageBytes = 4 << 20    // 4194304
ChunkSize    = protocol.ChunkSize // 1 << 20
```

Frame types:

| Name | # | Name | # | Name | # |
|---|---:|---|---:|---|---:|
| TData | 7 | TViewReply | 48 | TPrepare | 64 |
| TDataEnd | 8 | TMissingReply | 49 | TAccept | 65 |
| TErr | 10 | TAbsent | 50 | TRead | 66 |
| TView | 32 | TPutResult | 51 | TScan | 67 |
| TMissing | 33 | TRef | 52 | TInstall | 68 |
| TGet | 34 | TOK | 53 | TPurge | 69 |
| TPut | 35 | TCASMismatch | 54 | TMarker | 70 |
| TRefGet | 36 | TIncomplete | 55 | TSeed | 71 |
| TRefPut | 37 | TRefs | 56 | TPromise | 80 |
| TRefDelete | 38 | TStatusReply | 57 | TConflict | 81 |
| TRefList | 39 | TAdminReply | 58 | TAccepted | 82 |
| TStatus | 40 | TRefChanges | 59 | TReadReply | 83 |
| TAdmin | 41 | TRefSynced | 60 | TScanReply | 84 |
| TRefWatch | 42 | | | TInstalled | 85 |
| TJoin | 96 | TGCBarrier | 97 | TGCMark | 98 |
| TGCKeys | 99 | TGCStatus | 100 | TAck | 101 |
| TViewChanged | 102 | TGCStatusRep | 103 | TGCAbort | 104 |
| TPing | 105 | TPong | 106 | TBackupNote | 107 |
| TRefChanged | 108 | | | | |

Error codes (verbatim strings): `stale-view`, `not-owner`, `no-space`, `busy`,
`bad-request`, `unauthorized`, `unknown-ref`, `cas-mismatch`, `incomplete`,
`unavailable`, `timeout`, `internal`, `not-member`, `need-view`, `expired`,
`amnesiac`, `mark-frozen`, `retired`, `too-soon`, `conflict`, `no-mark`
(Go names `CodeStaleView` … `CodeNoMark`, `wire.go:107-129`).

`ErrProtocol = errors.New("wire: unexpected frame")` (`wire.go:244`).

#### 2.2.2 Sub-structs (`wire.go:131-167`)

"Go type" is the string fxamacker prints in type errors (D11).

| Struct (error name) | Key | Field | Go type | omitempty |
|---|---:|---|---|---|
| `wire.KeyHolders` | 0 | Key | `[]uint8` | no |
| | 1 | Holders | `[][]uint8` | yes |
| `wire.KeyFailure` | 0 | Key | `[]uint8` | no |
| | 1 | Node | `[]uint8` | no |
| | 2 | Reason | `string` | no |
| | 3 | RetryAfter (ms) | `int64` | yes |
| `wire.KeyReject` | 0 | Key | `[]uint8` | no |
| | 1 | Reason | `string` | no |
| `wire.RefInfo` | 0 | Name | `string` | no |
| | 1 | Key | `[]uint8` | no |
| | 2 | Version | `[]uint8` | no |
| | 3 | CreatedAt (ns) | `int64` | no |
| | 4 | User | `string` | yes |
| `wire.ScanRow` | 0 | Reg | `[]uint8` | no |
| | 1 | Promised | `[]uint8` | yes |
| | 2 | Accepted | `[]uint8` | yes |
| | 3 | Value | `[]uint8` | yes |
| | 4 | HasValue | `bool` | yes |

Probe: `KeyHolders{}` → `a1 00 f6`; `ScanRow{}` → `a1 00 f6`;
`KeyFailure{Key: {1}}` → `a3 00 4101 01 f6 02 60`;
`RefInfo{Name:"n", Key:{}, Version:{9}, CreatedAt:-2, User:"u"}` → `a5 00 616e 01 40 02 4109 03 21 04 6175`.

#### 2.2.3 `Msg` (`wire.go:169-240`, error name `wire.Msg`)

Every field is `omitempty` except key 0. Keys 0-23 encode as one byte
`00`-`17`; 24-61 as `18 18`-`18 3d`.

| Key | Field | Go type | Key | Field | Go type |
|---:|---|---|---:|---|---|
| 0 | Type (**not omitempty**) | `int` | 31 | HasCurrent | `bool` |
| 1 | ClusterID | `[]uint8` | 32 | Reg | `[]uint8` |
| 2 | Incarnation | `uint64` | 33 | Ballot | `[]uint8` |
| 3 | Epoch | `uint64` | 34 | Value | `[]uint8` |
| 4 | Keys | `[][]uint8` | 35 | HasValue | `bool` |
| 5 | Pin | `bool` | 36 | Accepted | `[]uint8` |
| 6 | Name | `string` | 37 | Promised | `[]uint8` |
| 7 | Record | `[]uint8` | 38 | NotAfter | `int64` |
| 8 | Data | `[]uint8` | 39 | Rows | `[]wire.ScanRow` |
| 9 | Key | `[]uint8` | 40 | More | `bool` |
| 10 | Code | `string` | 41 | Since | `uint64` |
| 11 | Text | `string` | 42 | Token | `[]uint8` |
| 12 | View | `[]uint8` | 43 | Weight | `uint32` |
| 13 | Version | `[]uint8` | 44 | Zone | `string` |
| 14 | ExpectedVersion | `[]uint8` | 45 | Addrs | `[]string` |
| 15 | ExpectedOld | `[]uint8` | 46 | NoVote | `bool` |
| 16 | Force | `bool` | 47 | G | `uint64` |
| 17 | HasExpected | `bool` | 48 | Nonce | `[]uint8` |
| 18 | Prefix | `[]uint8` | 49 | Seq | `uint64` |
| 19 | After | `[]uint8` | 50 | Expand | `bool` |
| 20 | Limit | `int` | 51 | Params | `[]uint8` |
| 21 | Refs | `[]wire.RefInfo` | 52 | Sent | `uint64` |
| 22 | Next | `[]uint8` | 53 | Received | `uint64` |
| 23 | Holders | `[]wire.KeyHolders` | 54 | Idle | `bool` |
| 24 | Failed | `[]wire.KeyFailure` | 55 | Marked | `uint64` |
| 25 | Rejected | `[]wire.KeyReject` | 56 | Missing | `[][]uint8` |
| 26 | Short | `[]wire.KeyHolders` | 57 | Status | `[]uint8` |
| 27 | Unreachable | `[][]uint8` | 58 | Node | `[]uint8` |
| 28 | RetryAfter (ms) | `int64` | 59 | Error | `string` |
| 29 | Current | `[]uint8` | 60 | Pattern | `string` |
| 30 | Shortfall | `int` | 61 | Deleted | `[]string` |

Because all fields but `Type` are `omitempty`, the nil/empty distinction
never shows in a `Msg` encoding. It does show inside the sub-structs and the
ticket (E6), which is why decoding must keep it (§4.3).

#### 2.2.4 Functions

- **`WriteMsg(w io.Writer, m *Msg) error`** (`wire.go:246-260`):
  1. `payload := codec.Marshal(m)`; the error wrap `wire: encode frame: %w` is unreachable.
  2. If `len(payload) > MaxFrame`: return
     `wire: frame of <len> bytes exceeds limit 16777216`, **writing nothing**.
  3. **One** `Write` of `u32be(len) ‖ payload`; its error is returned unwrapped.
- **`ReadMsg(r io.Reader) (*Msg, error)`** (`wire.go:264-285`):
  1. `io.ReadFull` of 4 header bytes:
     - 0 bytes, then EOF → `io.EOF` (clean end of stream);
     - 1-3 bytes, then EOF → `io.ErrUnexpectedEOF`;
     - any other error (reset, stop) → returned as is.
  2. `n > MaxFrame` → `wire: frame of <n> bytes exceeds limit 16777216`.
  3. `io.ReadFull` of n payload bytes; failure → `wire: short frame: <err>`:
     `unexpected EOF` when partially read, `EOF` when no payload byte came.
  4. Decode failure → `wire: decode frame: <cbor error>`. `n == 0` → `wire: decode frame: EOF` (probe).
  - **Rust note:** tokio `read_exact` reports `UnexpectedEof` in both header
    cases, so the header must be read with a counting loop.
  - Probe texts: `EOF`; `unexpected EOF`; `wire: frame of 16777217 bytes exceeds limit 16777216`;
    `wire: short frame: unexpected EOF`;
    `wire: decode frame: cbor: cannot unmarshal array into Go value of type wire.Msg (cannot decode CBOR array to struct without toarray option)`.
- **`type Error struct{ Code, Text string; View []byte; RetryAfter time.Duration }`** (`wire.go:287-300`):
  `Error()` is `remote: <code>` when `Text == ""`, else `remote: <code>: <text>`
  (probe `remote: busy` | `remote: busy: t`).
- **`(*Error).Is(target)`** (`wire.go:303-306`): target is `*Error` with the same
  Code, and target.Text is empty or equal.
- **`ErrorFromMsg(m)`** (`wire.go:309-311`): Code, Text, View, `RetryAfter = m.RetryAfter * time.Millisecond`.
- **`ErrMsg(code, text)` / `WriteErr(w, code, text)`** (`wire.go:314-321`): frame
  `{0:10, 10:code, 11:text}`, **never stamped** (probe `rep-err`).
- **`IsCode(err, code)` / `AsError(err)`** (`wire.go:324-334`): `errors.As`
  through `%w` chains. Only `*wire.Error` matches; a `*protocol.RemoteError`
  from a pack read does **not** (probe: `iswirecode=false`).
- **`Expect(r, want int) (*Msg, error)`** (`wire.go:338-350`):
  - a `ReadMsg` error → `(nil, err)`;
  - `Type == TErr` → `(m, *Error)`;
  - `want != 0` and the type differs → `(m, fmt.Errorf("%w: type %d, want %d", ErrProtocol, got, want))`,
    text `wire: unexpected frame: type 53, want 52` (probe).
- **`SendPackRecords(w, recs)` / `NewPackReader(r)`** (`wire.go:354-362`): delegate to `protocol` (§2.3).
- **`Keys32(raw [][]byte) ([][32]byte, error)`** (`wire.go:365-374`): every
  entry must be 32 bytes, else `wire: key <i> has <n> bytes` and a nil
  result. On success the result has `len(raw)` entries (non-nil for 0).
  - Callers **ignore** this error: `lacking, _ := wire.Keys32(resp.Keys)`
    (`client/objects.go:108`) means a malformed reply counts every key as held;
    `sample, _ :=` (`client/refs.go:108`) gives an empty sample. Mirror this.
- **`RawKeys(keys [][32]byte) [][]byte`** (`wire.go:377-383`): `make([][]byte, len(keys))`,
  so a key-less reply omits key 4. `RawIDs` = `RawKeys` (`wire.go:386`).
- **`CloseStream(s io.Closer)`** (`wire.go:390-395`): `s.Close()`, then `CancelRead(0)`
  if available; errors ignored. In go-iroh:
  - `Stream.Close()` → `s.s.Close()`, "closes the send side" (FIN) (`iroh/conn.go:69`; qng `stream.go:204-206`);
  - `CloseWrite()` does exactly the same (`iroh/conn.go:75`);
  - `CancelRead(code)` → STOP_SENDING with that code (`iroh/conn.go:87`; qng `stream.go:184-186`).

#### 2.2.5 Stream conventions

Sources: architecture §10; `transport/transport.go:189-243`;
`client/objects.go:244-301`; `client/fetch.go:275-309`;
`client/watch.go:127-245`; `node/server.go:83-101`.

- One bidirectional stream per operation; the initiator writes first.
- The node reads one request frame, runs the handler, and on return
  `defer wire.CloseStream(s)` sends FIN and STOP_SENDING(0) (`node/server.go:84`).
- **`Pool.Call(ctx, id, alpn, req)`** (`transport.go:192-229`):
  1. `Get` a connection; `OpenStream` (an error drops the connection);
     `defer CloseStream`.
  2. `WriteMsg(req)` (error returned); `CloseWrite()` (error ignored).
  3. Read one frame in a goroutine. If ctx ends first: `CancelRead(0)`,
     `Close()`, wait for the reader, return `ctx.Err()`.
  4. A read error is returned. `Type == TErr` → `(reply, *wire.Error)`.
     No other type check; callers check `Type`.
- **get**: `WriteMsg(TGet)`; `CloseWrite`; `Expect(TAbsent)`; `NewPackReader`;
  amberpack `Records()`; **drain** with `io.Copy(io.Discard, pr)` (`fetch.go:306`); then `c.ok(id)`.
- **put**: `WriteMsg(TPut)` without FIN; `SendPackRecords`; `CloseWrite`;
  `Expect(TPutResult)`. The node reads the pack through `NewPackReader`
  (`node/data.go:311`).
- **ref-watch**: `Open`; `WriteMsg(TRefWatch)` **without** `CloseWrite`.
  - The node ends the watch when the client FINs: it runs
    `io.Copy(io.Discard, s)` then cancels (`node/watch.go:143-147`).
  - The client reads frames until done; to abandon it sends `CancelRead(0)`,
    then `Close()` (`client/watch.go:162-165`).

### 2.3 transport-iroh `protocol` package (v0.4.0)

dstore reuses only `SendPackRecords`, `NewPackReader`, `RemoteError` and the
constants. But `packReader` decodes frames with **transport-iroh's own
`protocol.Msg`**, so its layout decides what a pack transfer accepts.

#### 2.3.1 Constants (`protocol.go:15-51`)

```go
ALPN      = "amber-store-iroh/1"
MaxFrame  = 16 << 20
ChunkSize = 1 << 20
TPush = 1; TPull = 2; TRefList = 3; TRef = 4; TRefs = 5; TWants = 6
TData = 7; TDataEnd = 8; TOK = 9; TErr = 10; TAttach = 11; TAccept = 12; TPin = 13
CodeCASMismatch = "cas-mismatch"; CodeUnknownRef = "unknown-ref"
CodeBadRequest = "bad-request"; CodeInternal = "internal"
ErrProtocol = errors.New("protocol: unexpected frame")
```

#### 2.3.2 `protocol.Msg` (`protocol.go:79-103`, error name `protocol.Msg`)

All fields `omitempty` except 0.

| Key | Field | Go type | Key | Field | Go type |
|---:|---|---|---:|---|---|
| 0 | Type | `int` | 9 | Key | `[]uint8` |
| 1 | Name | `string` | 10 | Code | `string` |
| 2 | Root | `[]uint8` | 11 | Text | `string` |
| 3 | CAS | `bool` | 12 | Current | `[]uint8` |
| 4 | ExpectedOld | `[]uint8` | 13 | Token | `[]uint8` |
| 5 | Record | `[]uint8` | 14 | DataConns | `int` |
| 6 | Refs | `[]protocol.RefInfo` | 15 | DataPorts | `[]uint16` |
| 7 | Keys | `[][]uint8` | 16 | DataEndpoints | `[]protocol.DataEndpointRec` |
| 8 | Data | `[]uint8` | 17 | Names | `[]string` |

`protocol.RefInfo` = {0 Name `string`, 1 Key `[]uint8`, 2 CreatedAt `int64`, 3 User `string` omitempty}.
`protocol.DataEndpointRec` = {0 ID `[]uint8`, 1 Addrs `[]string` omitempty}.

#### 2.3.3 Frame I/O and `RemoteError`

- **`WriteMsg(w, Msg)`** (`protocol.go:124-139`): the same bytes as `wire.WriteMsg`,
  written in two `Write` calls (header, then payload). Errors:
  `protocol: encode frame: %w`, `protocol: frame of %d bytes exceeds limit %d`.
- **`ReadMsg(r)`** (`protocol.go:143-164`): same header/EOF rules as `wire.ReadMsg`; texts
  `protocol: frame of %d bytes exceeds limit %d`, `protocol: short frame: %w`, `protocol: decode frame: %w`.
- **`RemoteError{Code, Text string; Current []byte}`** (`protocol.go:166-180`):
  `Error()` is always `remote: <code>: <text>`, keeping the trailing `": "`
  when Text is empty (probe: `remote: busy: `). `RemoteFromMsg` fills Current
  from key 12, which is dstore's `View`.

#### 2.3.4 Sending a pack: `chunkWriter` + `SendPackRecords` (`pack.go:33-91`)

1. `cw := &chunkWriter{w}`; `pw := amberpack.NewWriter(cw)`.
2. For each `(rec, err)` from the iterator: on err, **return err immediately**:
   no terminator, and the partial chunk buffer is *not* flushed.
   Otherwise `pw.AddRecord(rec)` (writes the pack magic before the first record).
3. `pw.Close()`: writes the magic if nothing was added, then the end marker `00`.
4. `cw.finish()`: flush a non-empty buffer as a TData frame, then write `TDataEnd`.

`chunkWriter.Write` appends to a buffer and, **whenever the buffer reaches
exactly `ChunkSize` (1 MiB)**, writes `{0: 7, 8: <1 MiB>}` and resets
(`pack.go:39-61`). So frame boundaries fall at fixed 1 MiB offsets of the pack
byte stream, independent of write sizes. A flush of an empty buffer writes nothing.

- TData frames carry **only** keys 0 and 8; TDataEnd is `{0: 8}` = `a1 00 08`.
- **The pack payload** (core-rs `src/amberpack.rs:1-30`, `229-283`): magic
  `"AMBERPK\x03"` (8 bytes), then each record verbatim (46-byte header
  `01 ‖ key[32] ‖ flags ‖ ulen u32be ‖ slen u32be ‖ crc32c u32be`, then slen
  payload bytes), then the end marker `00`.

Probe frame streams (record payloads incompressible):

| Pack bytes | Frames `(payload length, head)` |
|---|---|
| 9 (no records) | full stream `0000000e a2000708 49 414d424552504b03 00` · `00000003 a10008` |
| 1048576 | `1048585: a2 00 07 08 5a00100000 …` · `3: a10008` |
| 1048577 | `1048585: a2000708 5a00100000 …` · `6: a2000708 41 00` · `3: a10008` |
| 2621541 | `1048585` · `1048585` · `524398: a2000708 5a00080065 …` · `3` |

#### 2.3.5 Reading a pack: `packReader` (`pack.go:101-141`)

`Read(b)` loops while its current chunk is empty:

1. A sticky error is returned again. After TDataEnd → `io.EOF`.
2. `m, err := protocol.ReadMsg(p.r)` (**protocol.Msg decoding**). An error
   becomes sticky and is returned **as is**. A stream that ends cleanly at a frame boundary
   before TDataEnd therefore gives `io.EOF`; the amberpack reader then reports
   truncation ("truncated before end marker" in core-rs `src/amberpack.rs:340`).
3. By type:
   - `TData` → `cur = m.Data`; an empty Data yields no bytes and the loop continues;
   - `TDataEnd` → done;
   - `TErr` → sticky `*RemoteError`;
   - anything else → sticky `protocol: unexpected frame: type <n> during pack transfer`.

Consumers must drain to EOF so TDataEnd is consumed before the next frame is read.

Probe outcomes that follow from decoding dstore frames as `protocol.Msg`:

| Frame arriving during a pack | Result |
|---|---|
| dstore `TErr{Code:"busy", View:{1,2}, RetryAfter:7}` | `remote: busy: `, `Current = 0102`, `wire.IsCode` false |
| dstore `TAbsent{Keys:[[1]]}` | `protocol: decode frame: cbor: cannot unmarshal byte string into Go struct field protocol.Msg.4 of type uint8` |
| dstore `TRefs{Refs:[{Name:"a"}]}` | `protocol: unexpected frame: type 56 during pack transfer` (key 21 is unknown, ignored) |
| `TData` that also carries dstore `Epoch` (key 3) | `protocol: decode frame: cbor: cannot unmarshal positive integer into Go struct field protocol.Msg.3 of type bool` |
| empty stream | `io.EOF` |

**Consequences for Rust:**

- (a) Never stamp or add fields to TData/TDataEnd: Go readers would fail.
- (b) The Rust pack reader must decode frames with `protocol.Msg` typing to
  reproduce the same accept/reject decisions and texts.
- (c) A TErr during a pack surfaces as a `RemoteError` with the
  `remote: <code>: <text>` format, not as a `wire.Error`.

### 2.4 ticket (`ticket/ticket.go`)

#### 2.4.1 Types (`ticket.go:18-32`)

| Struct (error name) | Key | Field | Go type | omitempty |
|---|---:|---|---|---|
| `ticket.Member` | 0 | ID | `[]uint8` | no |
| | 1 | Addrs | `[]string` | yes |
| `ticket.Ticket` | 0 | ClusterID | `[]uint8` | no |
| | 1 | Incarnation | `uint64` | no |
| | 2 | Members | `[]ticket.Member` | no |

`const Prefix = "dstore1"`;
`enc = base32.StdEncoding.WithPadding(base32.NoPadding)` (`ticket.go:34`).

Probe encodings:

- `Ticket{}` → `a3 00 f6 01 00 02 f6`
- `Ticket{Members: []Member{{ID: nil, Addrs: []string{}}}}` → `a3 00 f6 01 00 02 81 a1 00 f6`
- `Ticket{ClusterID: []byte{}, Members: []Member{}}` → `a3 00 40 01 00 02 80`

Decoding keeps nil and empty apart, and re-encoding reproduces the input (probe):
`a3004001000281a1004140` → same bytes; `a300f601000281a100f6` → same bytes.

#### 2.4.2 `Encode()` / `String()` (`ticket.go:37-41`)

`"dstore1" + strings.ToLower(enc.EncodeToString(codec.MustMarshal(t)))`,
using the RFC 4648 alphabet `ABCDEFGHIJKLMNOPQRSTUVWXYZ234567`, no padding,
then lower-cased. `String()` returns `Encode()`.

Probe vector (the `TestRoundTrip` ticket): ClusterID `"0123456789abcdef"`,
Incarnation 3, one member with ID `07 00…00` (32 bytes) and Addrs
`["ip:127.0.0.1:4433", "relay:https://relay.example/"]`.

CBOR:

```
a3 00 50 30313233343536373839616263646566 01 03 02 81 a2 00 58 20 07 (00 ×31)
01 82 71 69703a3132372e302e302e313a34343333 78 1c 72656c61793a68747470733a2f2f72656c61792e6578616d706c652f
```

Text (182 chars):
`dstore1umafambrgiztinjwg44dsylcmnsgkzqbambidiqalaqaoaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaabqjyws4b2gezdolrqfyyc4mj2gq2dgm3ydrzgk3dbpe5gq5duobztulzpojswyylzfzsxqylnobwgkly`

#### 2.4.3 `IDs()` (`ticket.go:45-55`)

In member order, keep members whose `len(ID) == 32` and whose lowercase hex
has not been seen yet; join the hex strings with `","`. No qualifying member → `""`.

#### 2.4.4 `Parse(s)` (`ticket.go:60-80`), in validation order

1. `s = strings.TrimSpace(s)`: Unicode White_Space, the same set as Rust
   `char::is_whitespace` (probe: a leading U+00A0 and a trailing U+2003 are trimmed).
2. `s == ""` → `ticket: empty`.
3. If `!strings.HasPrefix(strings.ToLower(s), "dstore1")` → `parseIDs(s)`.
   Only ASCII case variants of the prefix can match: the probe scan found no
   non-ASCII rune that lower-cases to one of `d s t o r e 1`. Rust:
   `s.len() >= 7 && s.as_bytes()[..7].eq_ignore_ascii_case(b"dstore1")`.
4. `b, err := enc.DecodeString(strings.ToUpper(s[7:]))` with the Go decoder
   (§2.4.7); on error → `ticket: <err>`, e.g. `ticket: illegal base32 data at input byte 3`.
5. `codec.Unmarshal(b, &t)` fails → `ticket: <cbor error>` (empty body → `ticket: EOF`).
6. `len(t.Members) == 0` (null or empty) → `ticket: no members`.

Nothing else is validated: member ID lengths, ClusterID and curve points
are left to `client.Dial`.

#### 2.4.5 `parseIDs(s)` (`ticket.go:82-96`)

- Fields: `strings.FieldsFunc(s, r == ',' || ' ' || '\t' || '\n' || '\r')`.
  `\v`, `\f` and Unicode spaces are **not** separators (probe: hex`\v`hex and
  hex`U+00A0`hex fail as one field).
- For each field `f`: `id, err := irohkey.ParseEndpointID(strings.ToLower(f))`.
  On error → `ticket: %q is neither a dstore1 ticket nor a node id: %w`.
  `%q` quotes the **original** field with Go `strconv.Quote`: printable runes
  as is, `\"` and `\\`, `\a\b\f\n\r\t\v`, other bytes below 0x20, 0x7f and
  invalid UTF-8 as `\xNN`, non-printable runes as `\uNNNN` / `\UNNNNNNNN`
  (probe: `"zz\x01\"é"`, `"… …"`).
- Each success appends `Member{ID: <32 bytes>}` with nil Addrs. Duplicates are kept (probe: 2 members).
- No fields → `ticket: no members`.
- The result has ClusterID nil and Incarnation 0.

#### 2.4.6 go-iroh `ParseEndpointID(s)` (`key.go:229-238` → `212-218` → `505-524` → `61-66`)

1. `decodeBase32OrHex(s)`:
   - if `len(s) == 64` (bytes): `hex.DecodeString`; failure → `failed to decode hex string`;
   - else `stdBase32NoPad.DecodeString(strings.ToUpper(s))` (`key_core.go:25-29`);
     failure → `failed to decode base32 string`; decoded length ≠ 32 → `invalid length`.
2. `NewPublicKey`: `edwards25519.Point.SetBytes`, which accepts non-canonical
   encodings (`filippo.io/edwards25519@v1.2.0/edwards25519.go:138-150`);
   failure → `data is not a valid public key`.
3. On any error, if `decodeZBase32(s)` (alphabet
   `ybndrfg8ejkmcpqxot1uwisza345h769`, same Go decoder) yields 32 bytes, the
   error becomes `<err>: input is z-base-32, use ParseEndpointIDZ32`.

What succeeds:

- 64 hex characters;
- **exactly 52** base32 symbols. 51, 53 and 54 give `invalid length`, because
  final partial quanta of 1/3/6 symbols are dropped (§2.4.7);
- 52 symbols with **any trailing bits** in the last symbol: all 16 variants
  decode to the same id (probe);
- the Unicode lookalikes U+0130 `İ`, U+0131 `ı`, U+017F `ſ`, U+212A `K` inside
  a base32 id (probe: accepted), because `ToLower` then `ToUpper` maps them to `I`, `I`, `S`, `K`.

`ToLower` changes byte lengths (`İ` 2→1 byte, `K` 3→1 byte), and the 64-byte hex routing uses the lowered length.

Rust iroh-base cannot stand in: `PublicKey::from_str`
(`iroh-base-1.0.3/src/key.rs:249-257`, `475-496`) uses `data_encoding::BASE32_NOPAD`,
which rejects non-zero trailing bits (`check_trailing_bits: true`,
data-encoding-2.11.1 `src/lib.rs:1902`) and does no Unicode case mapping.
Its error texts do match go-iroh's (`key.rs:231-244`), and
`PublicKey::from_bytes(&[u8; 32])` (`key.rs:122-127` → ed25519-dalek 2.2.0
`src/verifying.rs:165-173`, dalek decompression) accepts the same
non-canonical encodings as filippo `SetBytes`.

#### 2.4.7 Go base32 decoding (`encoding/base32/base32.go`: `decode`, `DecodeString`, `stripNewlines`)

- `DecodeString` first removes every `\r` and `\n`, wherever they are
  (probe: `"A\nA"` → `00`; a ticket with `"\n\r"` inside the body parses).
- A symbol is valid only if it is an exact byte of the alphabet. StdEncoding
  is upper-case only (probe `"a"` → error), so callers upper-case first.
- Quanta of 8 symbols give 5 bytes. A final partial quantum of j symbols
  gives 1, 2, 3, 4 bytes for j = 2, 4, 5, 7, and **zero bytes and no error for
  j = 1, 3, 6** (probe: `"A"`, `"AAA"`, `"AAAAAA"` → empty; `"AAAAAAAAA"` → 5 bytes).
- Trailing bits are not checked (probe: `"ME"` and `"MF"` → `61`).
- An invalid symbol, including `=`, → `illegal base32 data at input byte <i>`,
  where i is its index in the newline-stripped, upper-cased string
  (probe `"A="` → 1; `"0A"` → 0).
- The NoPadding padding branch compares against `byte(-1) = 0xFF`, which never
  occurs in valid UTF-8, so it is unreachable from `Parse`.
- **Encoding** is canonical: data-encoding `BASE32_NOPAD.encode` (then
  lower-case) gives identical output.

#### 2.4.8 Go case mapping (probe scan over every rune, go1.26.5)

- `strings.ToUpper` turns a non-ASCII rune into a base32 symbol only for U+0131 → `I` and U+017F → `S`.
- `ToLower` then `ToUpper` also maps U+0130 → `I` and U+212A → `K`.
- No non-ASCII rune lower-cases to `[0-9a-f]`.
- For the error index: every symbol before the first invalid one is ASCII
  or `ı`/`ſ` (1 byte after `ToUpper`), and newlines are stripped, so
  i = the number of non-newline symbols before the offender.
- The only other place the Unicode mapping is observable is the 64-byte hex routing in §2.4.6.

#### 2.4.9 Parse outcomes (probe, verbatim)

| Input | Result |
|---|---|
| `""`, `"  "` | `ticket: empty` |
| `"dstore1"` | `ticket: EOF` |
| `"nope"` | `ticket: "nope" is neither a dstore1 ticket nor a node id: invalid length` |
| `" , ,"` | `ticket: no members` |
| 63 hex chars | `ticket: "<63 hex>" is neither a dstore1 ticket nor a node id: failed to decode base32 string` |
| `"ab"×32` | `ticket: "abab…ab" is neither a dstore1 ticket nor a node id: data is not a valid public key` |
| hex `,zz` | `ticket: "zz" is neither a dstore1 ticket nor a node id: invalid length` |
| `zz\x01"é` | `ticket: "zz\x01\"é" is neither a dstore1 ticket nor a node id: failed to decode base32 string` |
| 52-char z-base-32 id | `ticket: "<z32>" is neither a dstore1 ticket nor a node id: failed to decode base32 string: input is z-base-32, use ParseEndpointIDZ32` |
| hex U+00A0 hex | `ticket: "<hex> <hex>" is neither a dstore1 ticket nor a node id: failed to decode base32 string` |
| hex `\v` hex | `ticket: "<hex>\v<hex>" is neither a dstore1 ticket nor a node id: failed to decode base32 string` |
| test ticket + `a` (body length ≡ 7 mod 8) | `ticket: cbor: 1 bytes of extraneous data starting at index 109` |
| test ticket + `aaa` / `aaaaaa` / `aaaaaaaa` | `… 2 bytes …` / `… 4 bytes …` / `… 5 bytes of extraneous data starting at index 109` |
| `!` at body index 3 | `ticket: illegal base32 data at input byte 3` |
| `8` at body index 8 | `ticket: illegal base32 data at input byte 8` |
| upper-case ticket; `DsToRe1` + mixed-case body; `\n\r` inside the body; last symbol with other trailing bits; `ı` for `i` in the body; NBSP / U+2003 around it | accepted (members=1, inc=3) |
| hex id, 52-char base32 id (lower or upper), with `İ`/`ı`/`ſ`/`K` lookalikes | accepted |

#### 2.4.10 Where tickets are produced and consumed

- **`dstore cluster init`** (`cmd/dstore/main.go:311-314`): builds
  `Ticket{v.ClusterID, v.Incarnation, [{node id, n.Endpoint().Addrs()}]}` and prints
  `node id: <id>`, then `cluster ticket: <Encode()>`.
- **Admin op `cluster-ticket`** (`node/admin.go:85-96`): this node first, with
  its endpoint addrs, followed by up to the first 3 view nodes (this node can
  appear twice); returned as `AdminReply.Ticket`.
- **`dstore cluster ticket [--ids]`** (`main.go:337-363`): with `--store` and no
  `--ticket`, uses `localTicket` (node offline open + `worktree.TicketFromView`);
  otherwise dials and runs `cluster-ticket`, then **`Parse` and re-`Encode`**.
  Prints `t.IDs()` with `--ids`, else `t.Encode()`.
- **Worktree** `TicketFromView` (`worktree/flow.go:306-315`): up to 4 members
  from `v.Nodes`. `RefreshTicket` (`flow.go:320-331`) saves the config only when the
  `Encode()` string differs. Rust encoding must be byte-identical, or Go and
  Rust clients sharing a working copy will keep rewriting it.
- **`client.Dial`** (`client/client.go:86-116`):
  - members with `len(ID) == 32` become bootstrap entries (others skipped);
  - dial them in ticket order, each with a 15 s timeout, sending the **unstamped** `TView`; the first reply adopted wins;
  - when all fail → `client: no bootstrap node answered: <lastErr>`;
  - with no usable member → `client: no bootstrap node answered: client: ticket names no nodes`.
- **Ticket parse call sites**: `--ticket` / `$DSTORE_TICKET` (`cmd/dstore/client.go:60-64`),
  working-copy config (`cmd/dstore/wc.go:93`), `node join --seed` (`main.go:489`).

### 2.5 Admin and status payloads (CBOR inside `Msg.Params` / `Msg.Status`)

These structs live in the `node` package, but the client CLI encodes and
decodes them, so they belong to the client port.

#### 2.5.1 `node.AdminRequest` (`node/admin.go:18-35`)

| Key | Field | Go type | omitempty |
|---:|---|---|---|
| 0 | Op | `string` | **no** |
| 1 | Node | `[]uint8` | yes |
| 2 | Weight | `uint32` | yes |
| 3 | Zone | `string` | yes |
| 4 | Replicas | `uint8` | yes |
| 5 | Dead | `bool` | yes |
| 6 | AllowUnsafe | `bool` | yes |
| 7 | Force | `bool` | yes |
| 8 | Key | `[]uint8` | yes |
| 9 | Garbage | `float64` | yes (E11 float rules) |
| 10 | Tolerate | `bool` | yes |
| 11 | Forwarded | `bool` | yes |
| 12 | Pause | `bool` | yes |
| 13 | Rate | `uint64` | yes |
| 14 | Names | `[]string` | yes |

Probe:

- `{Op:"cluster-ticket"}` → `a1 00 6e 636c75737465722d7469636b6574`
- `{Op:"gc-run", Garbage:0.5}` → `a2 00 66 67632d72756e 09 f93800`
- `{Op:"replicas", Replicas:3, Weight:100, Names:["a"]}` → `a4 00 68 7265706c69636173 02 1864 04 03 0e 81 6161`
- `{Op:"gc-status"}` → `a1 00 69 67632d737461747573`

#### 2.5.2 `node.AdminReply` (`node/admin.go:37-46`)

All fields `omitempty`: 0 Text `string`, 1 Token `[]uint8`, 2 View `[]uint8`,
3 Names `[]string`, 4 Key `[]uint8`, 5 Ticket `string`, 6 GC `[]uint8`
(CBOR of `catalog.GCState`; the CLI never decodes it). Probe:
`{Text:"ok"}` → `a1 00 62 6f6b`.

The CLI decodes it with `codec.Unmarshal` and returns the error unwrapped
(`cmd/dstore/client.go:98-108`). `adminAction` (`client.go:110-132`) prints:

1. `r.Text` + `\n` if non-empty;
2. each Name + `\n`;
3. `key <hex>\n` when `len(Key) == 32`.

#### 2.5.3 CLI operations (`cmd/dstore/main.go`)

| Command | AdminRequest |
|---|---|
| `cluster ticket` (dial path) | `{Op:"cluster-ticket"}` (`main.go:352`) |
| `cluster replicas R` | `{Op:"replicas", Replicas:uint8(R)}` (`main.go:386`) |
| `token create [--weight]` | `{Op:"token-create", Weight:uint32}`; prints `r.Text` (`main.go:452-456`) |
| `node remove ID [--dead] [--allow-unsafe]` | `{Op:"node-remove", Node, Dead, AllowUnsafe}` |
| `node drain ID` | `{Op:"node-drain", Node}` |
| `node weight ID GiB` | `{Op:"node-weight", Node, Weight}` |
| `node zone ID ZONE` | `{Op:"node-zone", Node, Zone}` |
| `node repair ID` | `{Op:"node-repair", Node}` |
| `voter add ID` / `voter remove ID [--allow-unsafe]` | `{Op:"voter-add"|"voter-remove", Node, AllowUnsafe}` |
| `transition status|abort|refreeze|pause|resume` | `{Op:"transition-<sub>"}` |
| `gc run [--tolerate-missing] [--garbage F]` | `{Op:"gc-run", Tolerate, Garbage}` |
| `gc status` / `gc hold` / `gc release` | `{Op:"gc-status"}` / `{Op:"gc-hold", Pause:true}` / `{Op:"gc-hold", Pause:false}` |
| `gc why KEY` | `{Op:"gc-why", Key}` |
| `catalog backup` / `catalog backups` | `{Op:"catalog-backup"}` / `{Op:"catalog-backups"}` |

`rate-cap` (Rate), `keep` (Names) and `Force`/`Forwarded` have no CLI
producer: `Forwarded` is node-internal and `keep` comes from elsewhere. An
unknown op → `TErr bad-request "unknown admin op <op>"` (`node/admin.go:260`).

#### 2.5.4 `node.Status` / `node.VoterStat` (`node/status.go:13-52`)

| Key | Field | Go type | omitempty | Key | Field | Go type | omitempty |
|---:|---|---|---|---:|---|---|---|
| 0 | ID | `[]uint8` | no | 15 | TotalBytes | `int64` | no |
| 1 | Epoch | `uint64` | no | 16 | Puts | `uint64` | no |
| 2 | Incarnation | `uint64` | no | 17 | Gets | `uint64` | no |
| 3 | Packs | `int` | no | 18 | RefPuts | `uint64` | no |
| 4 | Records | `uint64` | no | 19 | BytesIn | `uint64` | no |
| 5 | Bytes | `int64` | no | 20 | BytesOut | `uint64` | no |
| 6 | Pins | `int` | no | 21 | Amnesiac | `bool` | yes |
| 7 | Unreachable | `[][]uint8` | yes | 22 | Retired | `bool` | yes |
| 8 | PendingPacks | `int` | no | 23 | ScrubAgeSec | `int64` | yes |
| 9 | Transition | `string` | yes | 24 | LastLive | `uint64` | yes |
| 10 | GC | `string` | yes | 25 | Corrupt | `int` | yes |
| 11 | LeaseHolder | `[]uint8` | yes | 26 | IsHolder | `bool` | yes |
| 12 | Voters | `[]node.VoterStat` | yes | 27 | UnauditedKeys | `int` | yes |
| 13 | Writable | `bool` | no | 28 | Watchers | `int` | yes |
| 14 | FreeBytes | `int64` | no | | | | |

`node.VoterStat`: 0 ID `[]uint8`, 1 Calls `uint64`, 2 Failures `uint64`,
3 P99ms `int64`, none omitempty.

`printStatus` (`cmd/dstore/client.go:134-193`) uses `DecodeStatus`; a decode
failure prints `<line> — bad status`, and a status call error prints
`<line> — unreachable: <err>`.

### 2.6 Client ALPN: request and reply field sets (cross-checked client ↔ node)

**Stamps.** "stamp" means keys 1 ClusterID, 2 Incarnation and 3 Epoch from the
client's cached view (`client.stamp`, `client/client.go:189-197`), all
omitted while no view is cached. "rstamp" means keys 2 Incarnation and 3
Epoch from the node's view (`stampReply`, `node/server.go:198-204`; nothing
while the node has no view).

**Before dispatch** (`node/server.go:103-139`): when the view's ACL allowlist is non-empty:

- a peer in neither `Allowed` nor `Admins` → `TErr unauthorized "not on the allowlist"`;
- `TRefDelete` from a non-admin → `TErr unauthorized "ref-delete needs an admin peer"`.

An unknown type → `TErr bad-request "unknown operation"`. `TPing` → rstamp `TPong`.
`WriteErr` frames are never stamped.

**Client-side handling.**

- **`call`** (`client/client.go:266-281`):
  1. `RequestTimeout` (default 2 min) wraps `Pool.Call`.
  2. On error → `handleErr`: a `*wire.Error` whose `len(View) > 0` → `view.Decode` and adopt, no backoff.
     Any other error → failures++, backoff `5 s << min(failures-1, 4)` capped at 60 s.
  3. On success → `ok(id)`. If `resp.Epoch > 0 || resp.Incarnation > 0` and the
     cached view compares older → `go RefreshView(context.Background())`.
- **`callRetry`**: up to 4 calls while the error is `stale-view`.
- **`anyNode`** (`client/client.go:329-362`): nodes in preference order; each node
  is retried up to 4 more times on `stale-view`. The first success, or any
  `*wire.Error` answer, is final. Otherwise the last transport error, or `client: no nodes`.
- Unexpected reply types → `client: unexpected reply <n>` (`client/refs.go:84,111,128,141`; `client/client.go:138,387`).

| Op | Request frame (client) | Reply frames (node) |
|---|---|---|
| view | Dial: `{0:32}` **unstamped** (`client.go:99`, via `pool.Call` directly). RefreshView / probeHinted: `{0:32, stamp}` (`client.go:182`, `259`) | no view → `TErr unavailable "no view"`; else rstamp `{0:48, 12 View, 27 Unreachable?}` (`server.go:254-264`). The epoch is not checked |
| missing | `{0:33, stamp, 4 Keys, 5 Pin?}` via `callRetry` (`objects.go:89`) | `len(Keys) > 8192` → `bad-request "too many keys"`; a bad key → `bad-request "wire: key i has n bytes"`; store error → `internal <err>`; else rstamp `{0:49, 4 Keys (lacking)?, 26 Short?}`. Short lists present keys held by fewer than `len(WriteSet)` owners, `{0 Key, 1 Holders?}` (`data.go:26-44`, `136-148`). The epoch is not checked on the client ALPN |
| get | open stream; `{0:34, stamp, 4 Keys}`; FIN (`fetch.go:284-287`) | the same key-count/key-length errors; store error → `internal`; else rstamp `{0:50, 4 Keys (absent)?}`, then TData…TDataEnd with the present records in store location order (`data.go:193-244`). Epoch not checked |
| put | open stream; `{0:35, stamp}`; TData…TDataEnd; FIN (`objects.go:266-286`) | epoch older → rstamp `{0:10, 10 "stale-view", 11 "request epoch is behind", 12 View}` (`server.go:248-252`); "wrong cluster" / "epoch above the catalog's" → `bad-request <text>`; not writable → `no-space "node below its free-space reserve"`; pack error → `bad-request "pack: <err>"`; over 64 MiB → `bad-request "batch over 64 MiB"`; store error → `no-space <err>` / `internal <err>`; else rstamp `{0:51, 23 Holders?, 24 Failed?, 25 Rejected?}` (`data.go:284-356`) |
| ref-get | `{0:36, stamp, 6 Name}` (`refs.go:79`) | invalid name → `bad-request <err>`; catalog errors per `catalogErr` below; else rstamp `{0:52, 7 Record, 13 Version}` (`node/refs.go:68-77`) |
| ref-put | `{0:37, stamp, 7 Record, 16 Force?, 17 HasExpected?, 14 ExpectedVersion?, 15 ExpectedOld?}` (`client/refs.go:96-97`, `58-68`) | in order: decode fails → `bad-request "record: <err>"`; key not 32 bytes → `bad-request "record key"`; invalid name → `bad-request "name: <err>"`; `m.Name != "" && m.Name != rec.Name` → `bad-request "frame name differs from the record's"`; key parse fails → `bad-request "root key: <err>"`; epoch older → stale frame (other epoch errors ignored); walk deadline → `timeout "completeness walk exceeded put_ttl"`; other walk error → `unavailable "completeness walk: <err>"`; missing keys → rstamp `{0:55, 4 Keys (≤ 64 sample), 30 Shortfall}`; catalog error → `catalogErr`; else rstamp `{0:53, 9 Key (record key), 13 Version}` (`node/refs.go:125-175`) |
| ref-delete | `{0:38, stamp, 6 Name, cond}` (`client/refs.go:116-117`) | ACL admin check first; invalid name → `bad-request`; `catalogErr` (includes TCASMismatch); else rstamp `{0:53}`, no Key or Version (`node/refs.go:104-122`) |
| ref-list | `{0:39, stamp, 18 Prefix?, 19 After?}`. Prefix `[]byte("")` is omitted; After is nil on the first page (`client/refs.go:136`) | `catalogErr`; else rstamp `{0:56, 21 Refs?, 22 Next?}` with `RefInfo{0 Name, 1 Key, 2 Version, 3 CreatedAt (ns), 4 User?}` (`node/refs.go:79-102`). Next is set when the catalog has more, or when the running estimate `len(name)+32+40+len(user)+16` exceeds 4 MiB, in which case the crossing entry is included and Next = its name. The client loops until Next or Refs is empty (`client/refs.go:144`) |
| ref-watch | open stream; `{0:42, stamp, 60 Pattern, 21 Refs?}`, known entries `RefInfo{0 Name, 1 Key, 2 Version = **null**, 3 CreatedAt = **0**}`, key 21 omitted when the known map is empty; no FIN (`client/watch.go:136-140`) | bad glob → `bad-request <err>`; no view → `unavailable "no view"`. Known entries not matching the pattern or with empty Key are ignored. Then a stream of rstamp `{0:59, 21 Refs?, 61 Deleted?}` and rstamp `{0:60}`: after the initial difference and after every reconcile scan (heartbeat). A failing initial scan → `catalogErr` frame and the handler returns (`node/watch.go:120-310`) |
| status | `{0:40, stamp}` (`client.go:366`) | rstamp `{0:57, 57 Status = CBOR(node.Status)}` (`node/status.go:106-109`) |
| admin | `{0:41, stamp, 51 Params = CBOR(node.AdminRequest)}`: `anyNode` when id is zero, else `call` (`client.go:374-390`) | Params decode fails → `bad-request <cbor error>`; `Admin` returns a `*wire.Error` → the same code and text; other errors → `unavailable <err>`; else rstamp `{0:58, 57 Status = CBOR(node.AdminReply)}` (`node/admin.go:49-62`) |

**`cond`** (`client/refs.go:58-68`): always `Force`. With `Versioned`:
`HasExpected = true` plus ExpectedVersion. With `Keyed`: `HasExpected = true`
plus ExpectedOld. Omitempty erases nil and empty conditions, so
`Versioned` with an empty version and `Keyed` with a nil old key both encode
as `17 f5` alone, which the node reads as "must not exist": `condOf`,
`node/refs.go:27-41`, sets Versioned when ExpectedVersion is non-nil, or when
ExpectedOld and Version are both nil; a non-nil ExpectedOld selects Keyed.

**`catalogErr`** (`node/refs.go:52-66`):

| Catalog outcome | Reply |
|---|---|
| CAS mismatch | rstamp `{0:54, 13 Version?, 31 HasCurrent?, 7 Record?, 29 Current?}`; Record and Current only when a current value exists (`node/refs.go:43-50`) |
| unknown reference | `TErr unknown-ref "no such reference"` |
| deadline exceeded | `TErr timeout <err>` |
| `expired` code | `TErr timeout "reference commit expired"` |
| anything else | `TErr unavailable <err>` |

**Client mapping.** `RefGet`/`RefPut`/`RefDelete` map `unknown-ref` to
`client: unknown reference` (`client/refs.go:21`, `70-75`).
`CASMismatch.Error()` is `cas mismatch: reference is absent` when HasCurrent
is false, else `cas mismatch: current key <hex>`. `Incomplete.Error()` is
`incomplete: <n> keys short` (`client/refs.go:31-47`).

**ref-watch client** (`client/watch.go:199-243`):

- `TErr stale-view` → retry the same node (up to 3 more);
- `bad-request` / `unauthorized` → yield the `*wire.Error` and stop;
- other codes → next node;
- any other frame type → treated like a stream failure;
- no frame for `WatchIdle` (default 2 min) → abandon and reconnect.

### 2.7 Cluster ALPN frames (node side; field sets from `node/` and `paxos/`)

For the node-side decisions: a client-only port never sends these, except
that `node join` sends TJoin. Every cluster request is stamped with
`stampReq` (keys 1, 2, 3; `node/data.go:182-189`).

| Frame | Fields set (source) |
|---|---|
| TJoin 96 | `42 Token, 43 Weight, 44 Zone, 45 Addrs, 46 NoVote, 16 Force` (`node/join.go:191`); forwarded with `58 Node` added (`join.go:64`). Reply `TViewReply{12 View}` (`join.go:89`) |
| TMissing 33 / TGet 34 / TPut 35 | same as the client ALPN (`node/data.go:156,584,506`; `node/put.go:258`) |
| TPrepare 64 | `32 Reg, 33 Ballot` (`paxos/proposer.go:250`) → TPromise `{36 Accepted, 34 Value, 35 HasValue}` / TConflict `{33 Ballot}` (`paxos/acceptor.go:353,347`) |
| TAccept 65 | `32 Reg, 33 Ballot, 34 Value, 35 HasValue, 38 NotAfter` (`proposer.go:289`) → TAccepted `{}` / TConflict `{33 Ballot}` |
| TRead 66 | `32 Reg` → TReadReply `{36 Accepted, 37 Promised, 34 Value, 35 HasValue}` |
| TScan 67 | `18 Prefix, 19 After, 20 Limit` → TScanReply `{39 Rows, 40 More?, 22 Next?}` |
| TInstall 68 | `12 View` → TInstalled `{}` |
| TPurge 69 | `32 Reg, 33 Ballot` |
| TMarker 70 | `41 Since`; TSeed 71: `39 Rows` (`node/maintenance.go:681,677`) |
| TGCBarrier 97 | `47 G` → TOK; TGCMark 98: `47 G, 48 Nonce, 51 Params, 34 Value`; TGCKeys 99: `47 G, 48 Nonce, 49 Seq, 4 Keys, 50 Expand, 58 Node` → TAck; TGCStatus 100: `47 G, 48 Nonce, 16 Force?` → TGCStatusRep 103 `{52 Sent, 53 Received, 54 Idle, 55 Marked, 56 Missing}`; TGCAbort 104: `47 G` (`node/gc.go`) |
| TViewChanged 102 | `3 Epoch` (`maintenance.go:294` …) or `58 Node` (repair, `admin.go:104`) → TAck |
| TPing 105 → TPong 106 | rstamp only |
| TBackupNote 107 | `9 Key` → TAck |
| TRefChanged 108 | `6 Name, 7 Record?, 13 Version` → TAck (`node/watch.go:96`) |
| stale acceptor reply | `TErr{10 "stale-view", 12 View}`, no Text (`paxos/acceptor.go:295`) |

---

## 3. Byte formats and text formats (verbatim)

### 3.1 Frame

```
frame   = length payload
length  = uint32 big-endian, 0..16777216 (a larger value is refused before reading)
payload = one CBOR data item (a map in practice), exactly `length` bytes
```

### 3.2 Probe frames (client requests, stamp = ClusterID 00..0f, Incarnation 1, Epoch 7; k1 = 11×32, k2 = 22×32)

| Case | Frame (hex) |
|---|---|
| view (Dial, unstamped) | `00000004 a1 00 1820` |
| view stamped | `0000001a a4 00 1820 01 50 000102030405060708090a0b0c0d0e0f 02 01 03 07` |
| missing, pin | `00000040 a6 00 1821 01 50 00…0f 02 01 03 07 04 81 5820 11…11 05 f5` |
| missing, no pin | `0000003e a5 00 1821 01 50 00…0f 02 01 03 07 04 81 5820 11…11` |
| get [k1,k2] | `00000060 a5 00 1822 01 50 00…0f 02 01 03 07 04 82 5820 11…11 5820 22…22` |
| put | `0000001a a4 00 1823 01 50 00…0f 02 01 03 07` |
| ref-get "main" | `00000020 a5 00 1824 01 50 00…0f 02 01 03 07 06 64 6d61696e` |
| ref-put versioned 0a0b | `00000024 a7 00 1825 01 50 00…0f 02 01 03 07 07 42 0203 0e 42 0a0b 11 f5` |
| ref-put must-not-exist | `00000020 a6 00 1825 01 50 00…0f 02 01 03 07 07 42 0203 11 f5` |
| ref-put force | `00000020 a6 00 1825 01 50 00…0f 02 01 03 07 07 42 0203 10 f5` |
| ref-delete force | `00000022 a6 00 1826 01 50 00…0f 02 01 03 07 06 64 6d61696e 10 f5` |
| ref-list "a/" after "a/b" | `00000023 a6 00 1827 01 50 00…0f 02 01 03 07 12 42 612f 13 43 612f62` |
| ref-list "" (first page) | `0000001a a4 00 1827 01 50 00…0f 02 01 03 07` |
| ref-watch "**" known {x: k1} | `0000004c a6 00 182a 01 50 00…0f 02 01 03 07 15 81 a4 00 6178 01 5820 11…11 02 f6 03 00 183c 62 2a2a` |
| ref-watch, empty known (unstamped example) | payload `a2 00 182a 183c 61 2a` |
| status | `0000001a a4 00 1828 01 50 00…0f 02 01 03 07` |
| admin gc-status | `00000029 a5 00 1829 01 50 00…0f 02 01 03 07 1833 4c a1 00 69 67632d737461747573` |

### 3.3 Probe frames (node replies, rstamp = Incarnation 1, Epoch 7)

| Case | Frame (hex) |
|---|---|
| view-reply View=01, Unreachable [k2] | `00000030 a5 00 1830 02 01 03 07 0c 41 01 181b 81 5820 22…22` |
| missing-reply, none lacking, Short [{k1,[k2]}] | `00000053 a4 00 1831 02 01 03 07 181a 81 a2 00 5820 11…11 01 81 5820 22…22` |
| absent, none | `00000008 a3 00 1832 02 01 03 07` |
| put-result | `000000df a6 00 1833 02 01 03 07 17 81 a2 00 5820 22…22 01 81 5820 11…11 1818 81 a4 00 5820 11…11 01 5820 22…22 02 6b 756e726561636861626c65 03 1904d2 1819 81 a2 00 5820 11…11 01 69 6e6f742d6f776e6572` |
| ref Record=05 Version=06 | `0000000e a5 00 1834 02 01 03 07 07 41 05 0d 41 06` |
| ok (ref-put) Key=k1 Version=06 | `0000002e a5 00 1835 02 01 03 07 09 5820 11…11 0d 41 06` |
| ok (ref-delete) | `00000008 a3 00 1835 02 01 03 07` |
| cas-mismatch | `00000035 a7 00 1836 02 01 03 07 07 41 05 0d 41 06 181d 5820 11…11 181f f5` |
| incomplete [k1] shortfall 3 | `0000002f a5 00 1837 02 01 03 07 04 81 5820 11…11 181e 03` |
| refs [{a,k1,06,1700000000000000000,"u"}] next "a" | `00000044 a5 00 1838 02 01 03 07 15 81 a5 00 6161 01 5820 11…11 02 41 06 03 1b 17979cfe362a0000 04 6175 16 41 61` |
| ref-changes [{a,k1,06,1}] deleted ["gone"] | `0000003e a5 00 183b 02 01 03 07 15 81 a4 00 6161 01 5820 11…11 02 41 06 03 01 183d 81 64 676f6e65` |
| ref-synced | `00000008 a3 00 183c 02 01 03 07` |
| status-reply Status=a0 | `0000000c a4 00 1839 02 01 03 07 1839 41 a0` |
| admin-reply {Text:"ok"} | `00000010 a4 00 183a 02 01 03 07 1839 45 a1 00 62 6f6b` |
| err unauthorized (WriteErr, unstamped) | `00000027 a3 00 0a 0a 6c 756e617574686f72697a6564 0b 74 6e6f74206f6e2074686520616c6c6f776c697374` |
| stale-view (writeStale) View=01 | `0000002f a6 00 0a 02 01 03 07 0a 6a 7374616c652d76696577 0b 77 72657175657374…626568696e64 0c 41 01` |
| pong | `00000008 a3 00 186a 02 01 03 07` |
| data-end | `00000003 a1 00 08` |

Other probe payloads:

| Case | Payload (hex) |
|---|---|
| `Msg{Type:TErr, Code:"busy", RetryAfter:1500}` | `a3 00 0a 0a 64 62757379 181c 1905dc` |
| `Msg{Type:TRefList, Limit:-1, Shortfall:300}` | `a3 00 1827 14 20 181e 19012c` |
| `Msg{Type:TJoin, Weight:70000, Since:1<<40, G:1<<32-1}` | `a4 00 1860 1829 1b 0000010000000000 182b 1a 00011170 182f 1a ffffffff` |
| `Msg{Type:TRefChanges, Deleted:["x",""], Unreachable:[nil,{}]}` | `a3 00 183b 181b 82 f6 40 183d 82 6178 60` |

### 3.4 Text formats

| Where | Format |
|---|---|
| ticket | `dstore1` + lowercase unpadded RFC 4648 base32 of the Ticket CBOR |
| short form | comma-joined lowercase hex ids (`IDs()`) |
| node id input | 64 hex, or 52 base32 symbols, case-insensitive; separators `, SP \t \n \r` |
| `wire.Error` | `remote: <code>` / `remote: <code>: <text>` |
| `protocol.RemoteError` | `remote: <code>: <text>` (trailing `": "` when text empty) |
| frame too large | `wire: frame of <n> bytes exceeds limit 16777216` (`protocol: …` in pack reads) |
| short payload | `wire: short frame: <err>` |
| undecodable payload | `wire: decode frame: <cbor err>` |
| wrong frame type | `wire: unexpected frame: type <got>, want <want>` |
| unexpected frame during a pack | `protocol: unexpected frame: type <n> during pack transfer` |
| bad key length | `wire: key <i> has <n> bytes` |
| client | `client: no endpoint`, `client: no bootstrap node answered: <err>`, `client: ticket names no nodes`, `client: unexpected reply <n>`, `client: no nodes`, `client: unknown reference`, `client: watch stream idle` |

---

## 4. Rust design

### 4.1 Module layout

| Module | Go counterpart | Content |
|---|---|---|
| `src/codec/mod.rs` | `codec` | `Enc`, `Encode`, `Decode`, `marshal`, `unmarshal`, `DecodeError` |
| `src/codec/wellformed.rs` | fxamacker `valid.go` | pass 1 |
| `src/codec/dec.rs` | fxamacker `decode.go` subset | pass 2 primitives |
| `src/codec/macros.rs` | struct tags | `cbor_struct!` |
| `src/gostr.rs` | `strconv.Quote`, `strings.ToLower/ToUpper/TrimSpace` | shared with the CLI spec |
| `src/wire/mod.rs` | `wire` | constants, `Msg` + sub-structs, `RemoteError`, `read_msg`/`write_msg`/`expect`, `keys32`/`raw_keys`, `close_stream` |
| `src/wire/pack.rs` | `protocol` (pack.go + Msg decoding) | `ProtocolMsg`, `send_pack_records`, `PackReader`, async amberpack record reader |
| `src/wire/admin.rs` | `node.AdminRequest/AdminReply/Status/VoterStat` | payload structs |
| `src/ticket.rs` | `ticket` + go-iroh `ParseEndpointID` + Go base32 | `Ticket`, `Member`, `parse`, `parse_endpoint_id`, `gobase32` |

### 4.2 Codec choice (evaluated)

- **ciborium 0.2.2** (offline registry): serde struct identifiers are strings,
  so integer keys need a hand-written `Serialize`/`Deserialize` per struct.
  The nil / empty / omitempty triple needs per-field `Option` plus
  `skip_serializing_if`. Decoding (`ciborium-0.2.2/src/de/mod.rs`) has no
  well-formedness-first pass, no 131072 caps, a recursion limit of 256 (Go: 32),
  and rejects simple values into integers; its error texts differ from
  fxamacker's. It would still need a pass-1 wrapper and custom visitors: no gain.
- **minicbor**: not in the offline registry. Its `#[n(i)]` derive fits integer
  keys, but it decodes strictly (no tag stripping, first-duplicate-wins,
  null-into-scalar or simple-into-int), with different errors.
- **Hand-rolled (chosen)**: roughly 900 lines plus a macro. core-rs has done
  this already for `Reference`: `src/reference.rs:440-890` (`skip_item`,
  `strip_and_check_tags`, `read_bstr_field`, `read_byte_elem`,
  `read_int64_field`, `unmarshal`), verified against fxamacker with a
  21197-case differential oracle (`port-notes/reference.md:128-157`). Those
  functions are private, so mirror and generalise them: indefinite lengths,
  131072 caps, per-type targets, the D11 field-name rewrite.
  - Encoding reuses `amber_store_core::cbor::append_head` (`src/cbor.rs:125-142`),
    which equals `encodeHead`.
  - Decoding needs its own head reader: `cbor::read_head` rejects ai 31
    (`src/cbor.rs:190`), but fxamacker accepts indefinite items.

### 4.3 Go → Rust field representation

| Go field | omitempty | Rust type | Encode | Decode |
|---|---|---|---|---|
| `int`, `int64` | any | `i64` (macro carries `"int"` or `"int64"`) | E7 | D4/D5, detail uses the Go type name |
| `uint64` / `uint32` / `uint8` | any | `u64` / `u32` / `u8` | E7 | D4, per-width overflow |
| `bool` | any | `bool` | E8 | D8 |
| `string` | any | `String` | E9 | D7 |
| `float64` | yes | `f64` | E11 | pass 2 into float (unused on receive) |
| `[]byte` | yes | `Vec<u8>` | omit if empty | null → empty; bstr/indef/array/bignum per D6/D9/D3 |
| `[]byte` | **no** | `Option<Vec<u8>>` (`None` = nil → `f6`) | `f6` / bstr | null → `None`; bstr → `Some` (even empty) |
| `[][]byte` | yes | `Vec<Vec<u8>>` | array of bstr | null element → empty (risk R6) |
| `[]string` | yes | `Vec<String>` | array of tstr | |
| `[]T` | yes | `Vec<T>` | array of maps | null element → `T::default()` |
| `[]T` | **no** | `Option<Vec<T>>` (`Ticket.members`) | `f6` / array | null → `None`; `80` → `Some(vec![])` |

Non-omitempty nullable fields: `KeyHolders.key`, `KeyFailure.{key,node}`,
`KeyReject.key`, `RefInfo.{key,version}`, `ScanRow.reg`,
`Ticket.{cluster_id,members}`, `Member.id`, `Status.id`, `VoterStat.id`.

### 4.4 Signatures

```rust
// src/codec/mod.rs
pub struct Enc { buf: Vec<u8> }
impl Enc {
    pub fn head(&mut self, major: u8, n: u64);        // cbor::append_head
    pub fn uint(&mut self, v: u64);
    pub fn int(&mut self, v: i64);                    // major 1 with -1-v when v < 0
    pub fn bool(&mut self, v: bool);
    pub fn null(&mut self);                           // 0xf6
    pub fn bytes(&mut self, v: &[u8]);
    pub fn text(&mut self, v: &str);
    pub fn f64_canonical(&mut self, v: f64);          // E11
}
pub trait Encode { fn encode(&self, e: &mut Enc); }
pub trait Decode: Sized + Default {
    const GO_NAME: &'static str;                      // "wire.Msg", "ticket.Ticket", ...
    fn decode_map(d: &mut Dec<'_>) -> Result<Self, DecodeError>;
}
pub fn marshal<T: Encode + ?Sized>(v: &T) -> Vec<u8>;
pub fn unmarshal<T: Decode>(b: &[u8]) -> Result<T, DecodeError>; // pass 1, then pass 2

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum DecodeError {
    #[error("EOF")] Eof,
    #[error("unexpected EOF")] UnexpectedEof,
    #[error("cbor: invalid additional information {ai} for type {ty}")] InvalidAi { ai: u8, ty: CborType },
    #[error("cbor: unexpected \"break\" code")] UnexpectedBreak,
    #[error("cbor: invalid simple value {0} for type primitives")] InvalidSimple(u8),
    #[error("cbor: {ty} length {n} is too large, causing integer overflow")] StrLenOverflow { ty: CborType, n: u64 },
    #[error("cbor: {ty} length {n} is too large, it would cause integer overflow")] LenOverflow { ty: CborType, n: u64 },
    #[error("cbor: exceeded max nested level 32")] MaxNested,
    #[error("cbor: exceeded max number of elements 131072 for CBOR array")] MaxArray,
    #[error("cbor: exceeded max number of key-value pairs 131072 for CBOR map")] MaxMap,
    #[error("cbor: wrong element type {chunk} for indefinite-length {ty}")] ChunkType { chunk: CborType, ty: CborType },
    #[error("cbor: indefinite-length {0} chunk is not definite-length")] ChunkIndef(CborType),
    #[error("cbor: {n} bytes of extraneous data starting at index {index}")] Extraneous { n: usize, index: usize },
    #[error("cbor: invalid UTF-8 string")] InvalidUtf8,
    #[error("{0}")] BadTag(String),                   // the three D2 messages
    #[error("{0}")] Type(UnmarshalTypeError),         // D11 format, field rewritten on the way out
}
```

`CborType` displays as the fxamacker names (`common.go:25-46`). On a 64-bit
target, a Go `int` overflow detail reads `(<n> overflows int)`.

**`cbor_struct!` sketch:**

```rust
cbor_struct! {
    #[go = "wire.RefInfo"]
    pub struct RefInfo {
        0 name:       String          = text("string"),
        1 key:        Option<Vec<u8>> = bytes_nullable("[]uint8"),
        2 version:    Option<Vec<u8>> = bytes_nullable("[]uint8"),
        3 created_at: i64             = int("int64"),
        4 user:       String          = text("string") omitempty,
    }
}
```

The macro generates `Default`, `Encode` (count the present fields, write the
head, then fields in ascending key order), and `Decode::decode_map`:
definite or indefinite map; numeric-key dispatch; a `seen` bitset so the first
value wins and duplicates are skipped unexamined; `UnmarshalTypeError.field`
rewritten to `"<GO_NAME>.<key>"` whenever an error leaves a field.

**Pass 2 primitives** (`Dec { data: &[u8], off: usize }`, input already
well-formed, so no bounds errors are possible):

- `is_null()` for the one-byte heads `f6` and `f7`;
- `tag_preamble()` (D2);
- `read_int(go: &str, min, max)`, `read_uint(go: &str, max)`;
- `read_bool()`, `read_text()` (definite + chunks, UTF-8 check), `read_bytes()`;
- `read_array(|d| …)`, `skip()`.

On the first error, fail fast; §2.1.2 argues this is observably identical.

```rust
// src/wire/mod.rs
pub const ALPN_CLIENT: &[u8] = b"amber-dstore/1";
pub const ALPN_CLUSTER: &[u8] = b"amber-dstore-cluster/1";
pub const ALPN_GATEWAY: &[u8] = b"amber-store-iroh/1";
pub const MAX_FRAME: u32 = 16 << 20; pub const MAX_KEYS: usize = 8192;
pub const MAX_PUT_BATCH: usize = 64 << 20; pub const MAX_PAGE_BYTES: usize = 4 << 20;
pub const CHUNK_SIZE: usize = 1 << 20;
pub const T_DATA: i64 = 7; /* … every T* of §2.2.1 … */ pub const T_REF_CHANGED: i64 = 108;
pub const CODE_STALE_VIEW: &str = "stale-view"; /* … all 21 codes … */

pub struct Msg { pub typ: i64, pub cluster_id: Vec<u8>, pub incarnation: u64, pub epoch: u64,
                 pub keys: Vec<Vec<u8>>, /* … keys 5..61 per §2.2.3 … */ pub deleted: Vec<String> }

#[derive(Debug, Clone, thiserror::Error)]
#[error("{}", if self.text.is_empty() { format!("remote: {}", self.code) } else { format!("remote: {}: {}", self.code, self.text) })]
pub struct RemoteError { pub code: String, pub text: String, pub view: Vec<u8>, pub retry_after: std::time::Duration }

#[derive(Debug, thiserror::Error)]
pub enum WireError {
    #[error("EOF")] Eof,                              // clean end before a header
    #[error("unexpected EOF")] UnexpectedEof,         // 1-3 header bytes
    #[error("wire: frame of {0} bytes exceeds limit 16777216")] TooLarge(u64),
    #[error("wire: short frame: {0}")] Short(ShortCause), // "unexpected EOF" | "EOF" | io
    #[error("wire: decode frame: {0}")] Decode(codec::DecodeError),
    #[error(transparent)] Io(std::io::Error),
    #[error(transparent)] Remote(RemoteError),        // Expect / Call on TErr
    #[error("wire: unexpected frame: type {got}, want {want}")] Protocol { got: i64, want: i64 },
    #[error("wire: key {index} has {len} bytes")] KeyLen { index: usize, len: usize },
}

pub async fn write_msg<W: tokio::io::AsyncWrite + Unpin>(w: &mut W, m: &Msg) -> Result<(), WireError>; // one write_all
pub async fn read_msg<R: tokio::io::AsyncRead + Unpin>(r: &mut R) -> Result<Msg, WireError>;          // counting header loop
pub async fn expect<R: tokio::io::AsyncRead + Unpin>(r: &mut R, want: i64) -> Result<Msg, WireError>;
pub fn error_from_msg(m: &Msg) -> RemoteError;
pub fn err_msg(code: &str, text: &str) -> Msg;
pub fn is_code(err: &(dyn std::error::Error + 'static), code: &str) -> bool; // walk source(); RemoteError only
pub fn keys32(raw: &[Vec<u8>]) -> Result<Vec<[u8; 32]>, WireError>;
pub fn raw_keys(keys: &[[u8; 32]]) -> Vec<Vec<u8>>;
pub fn close_stream(send: &mut iroh::endpoint::SendStream, recv: &mut iroh::endpoint::RecvStream) {
    let _ = send.finish();                            // Go Close / CloseWrite: FIN
    let _ = recv.stop(0u32.into());                   // Go CancelRead(0): STOP_SENDING
}
```

```rust
// src/wire/pack.rs
pub struct ProtocolMsg { /* keys 0..17 of §2.3.2, GO_NAME "protocol.Msg" */ }
#[derive(Debug, thiserror::Error)]
pub enum PackReadError {
    #[error("EOF")] Eof,
    #[error(transparent)] Frame(ProtocolFrameError),  // "protocol: ..." prefixes
    #[error("remote: {code}: {text}")] Remote { code: String, text: String, current: Vec<u8> },
    #[error("protocol: unexpected frame: type {0} during pack transfer")] Unexpected(i64),
}
pub async fn send_pack_records<W, S, E>(w: &mut W, recs: S) -> Result<(), E>
where W: tokio::io::AsyncWrite + Unpin, S: futures_core::Stream<Item = Result<Vec<u8>, E>>, E: From<std::io::Error>;
pub struct PackReader<R> { r: R, cur: Vec<u8>, pos: usize, done: bool, err: Option<PackReadError> }
impl<R: tokio::io::AsyncRead + Unpin> PackReader<R> {
    pub async fn read(&mut self, buf: &mut [u8]) -> Result<usize, PackReadError>;
    pub async fn drain(&mut self) -> Result<(), PackReadError>;
}
pub async fn next_record<R>(pr: &mut PackReader<R>, st: &mut PackState) -> Result<Option<amber_store_core::amberpack::RawRecord>, amber_store_core::amberpack::Error>;
```

`send_pack_records` writes the pack stream itself, so the TData boundaries stay exact:

- the magic `b"AMBERPK\x03"` before the first record or at close, each record verbatim, then `0x00`;
- a 1 MiB chunk buffer, emitting a frame at exactly 1 MiB, flushing the remainder at finish, then `{0:8}`;
- a source error returns at once, with no flush and no terminator.

`next_record` mirrors core-rs `Reader::next_record` (`src/amberpack.rs:330-372`)
over async reads: magic check; tag byte; 45 header bytes; `slen <= MAX_PAYLOAD`;
payload; `amberpack::parse_record` (`157-201`); errors wrapped as `Malformed`.

```rust
// src/ticket.rs
pub const PREFIX: &str = "dstore1";
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Member { pub id: Option<Vec<u8>>, pub addrs: Vec<String> }
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ticket { pub cluster_id: Option<Vec<u8>>, pub incarnation: u64, pub members: Option<Vec<Member>> }
impl Ticket { pub fn encode(&self) -> String; pub fn ids(&self) -> String; }
impl std::fmt::Display for Ticket { /* encode() */ }

#[derive(Debug, thiserror::Error)]
pub enum TicketError {
    #[error("ticket: empty")] Empty,
    #[error("ticket: {0}")] Base32(gobase32::CorruptInputError),
    #[error("ticket: {0}")] Cbor(codec::DecodeError),
    #[error("ticket: no members")] NoMembers,
    #[error("ticket: {} is neither a dstore1 ticket nor a node id: {source}", gostr::quote(.field))] NotId { field: String, source: KeyError },
}
pub fn parse(s: &str) -> Result<Ticket, TicketError>;

#[derive(Debug, thiserror::Error)]
pub enum KeyError {
    #[error("failed to decode hex string")] Hex,
    #[error("failed to decode base32 string")] Base32,
    #[error("invalid length")] Length,
    #[error("data is not a valid public key")] KeyData,
    #[error("{0}: input is z-base-32, use ParseEndpointIDZ32")] Z32(Box<KeyError>),
}
pub fn parse_endpoint_id(s: &str) -> Result<iroh_base::PublicKey, KeyError>; // §2.4.6; curve check via PublicKey::from_bytes

pub mod gobase32 {
    pub const STD: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    pub const ZBASE32: &[u8; 32] = b"ybndrfg8ejkmcpqxot1uwisza345h769";
    #[derive(Debug, thiserror::Error)] #[error("illegal base32 data at input byte {0}")]
    pub struct CorruptInputError(pub usize);
    pub fn decode_nopad(alphabet: &[u8; 32], s: &str) -> Result<Vec<u8>, CorruptInputError>; // strip \r\n; drop 1/3/6 tails; no trailing-bit check
    pub fn encode_std_nopad_lower(b: &[u8]) -> String; // data_encoding::BASE32_NOPAD then to_ascii_lowercase
}
```

`gostr` provides:

- `quote(&str)` (`strconv.Quote`);
- `to_lower_go` / `to_upper_go`, Go **simple** case mapping (risk R4).
  Rust `char::to_lowercase`/`to_uppercase` apply full mappings (`'İ'` → `"i\u{307}"`,
  `'ß'` → `"SS"`), so they cannot be used directly;
- `trim_space` = `str::trim` (the same White_Space set).

### 4.5 Crate dependencies (all available offline)

| Crate | Version | Use |
|---|---|---|
| `tokio` | 1 (`io-util`, `rt`, `time`, `sync`, `macros`) | async framing |
| `iroh` | `=1.0.3` (go-iroh v0.2.0 is verified wire-compatible with it) | `endpoint::{SendStream, RecvStream}` = noq 1.1.x types: `write_all` (`noq-1.1.1/src/send_stream.rs:74`), `finish` (`:188`), tokio `AsyncWrite` (`:332`); `read` (`recv_stream.rs:76`), `read_exact` (`:89`), `stop` (`:275`), tokio `AsyncRead` (`:594`); re-exported in `iroh-1.0.3/src/endpoint.rs:96-113` |
| `iroh-base` | 1.0.3 | `PublicKey` (`EndpointId` alias, `src/key.rs:70`), `from_bytes` (`:122-127`) |
| `data-encoding` | 2.11 | encode only: `BASE32_NOPAD`, `HEXLOWER` |
| `thiserror` | 2 | error types |
| `futures-core` | 0.3 | `Stream` for record sources (optional; an async closure works too) |
| `amber-store-core` | 0.3.0 (path `../core-rs` or git rev a85ffa1) | `cbor::append_head`; `amberpack::{parse_record, decode_payload, RawRecord, Record, REC_HEADER_SIZE, MAX_PAYLOAD, Error}`; `key::Key` |

No ciborium or minicbor.

---

## 5. Golden vectors a Go generator should emit

The generator is a Go module that requires dstore v0.1.9 (or `replace`s onto
the checkout) and is built with go1.26.5. It writes JSON lines
`{"case": …, "input_hex": …, "output_hex": …, "error": …}`. Every expected
value in §2 and §3 marked (probe) should appear as a case.

- **G1 Struct encodings.**
  - Every client request and node reply of §3.2/§3.3 (same stamps, same k1/k2).
  - `KeyHolders`, `KeyFailure`, `KeyReject`, `RefInfo`, `ScanRow`, `Ticket`,
    `Member`, `VoterStat` with each nullable field nil, empty and non-empty.
  - A fully populated `node.Status`.
  - `AdminRequest` for every CLI op of §2.5.3.
  - `Garbage` ∈ {0, -0.0 (omitted), 0.5, 1.5, 0.1, 65504, 65520, 1e-40,
    3.4e38, +Inf, -Inf, NaN, math.MaxFloat64, math.SmallestNonzeroFloat64}.
  - Every integer field at 0, 23, 24, 255, 256, 65535, 65536, 2^32-1, 2^32,
    max of its type, and −1 / MinInt64 for signed fields.
- **G2 `WriteMsg`.** Frames for G1; a `Msg{Type:TData, Data: 16 MiB}` whose payload exceeds `MaxFrame` (error text, zero bytes written).
- **G3 `ReadMsg`.** Inputs: empty; 1, 2, 3 header bytes; length 16777217;
  length 5 with 1 payload byte; length 5 with 0 payload bytes (`wire: short frame: EOF`);
  length 0; each G5 payload framed.
- **G4 Field decode matrix.** For every `Msg` key 0-61, every `RefInfo` key
  inside key 21, every `KeyHolders` key inside 26, every `Ticket`/`Member` key,
  and every `AdminReply`/`Status` key, crossed with these value shapes:
  `00`, `1bffffffffffffffff`, `20`, `3bffffffffffffffff`, `40`, `4161`, `60`,
  `6161`, `61ff`, `80`, `8101`, `81f6`, `8141 61`, `a0`, `f6`, `f7`, `f4`, `f5`,
  `f93e00`, `fb3fb999999999999a`, `f0`, `f8ff`, `c24105`, `c249010000000000000000`,
  `c34101`, `c06161`, `c001`, `c16161`, `d82a4161`, `d9d9f74161`, `5f41614162ff`,
  `7f6161ff`, `9f01ff`, `bfff`.
  Expected: the decoded field (with a nil/empty marker) and the error string.
- **G5 Structure.**
  - Trailing bytes; 31 vs 32 nested arrays under key 21; 32 vs 33 nested tags.
  - Claimed counts 131072 / 131073 for arrays and maps, definite and indefinite.
  - Indefinite map with an odd item count; stray `ff`; ai 28/29/30 on every major;
    ai 31 on majors 0/1/6; `f8 00`-`f8 1f`.
  - Wrong chunk type and nested indefinite chunk in strings.
  - Key types: bstr, float, array, map, tag, negint, text, invalid-UTF-8 text, uint overflow, negint overflow.
  - Duplicate matched key with the same and with a different type; duplicate unmatched keys.
  - Top level: null, undefined, tagged map, array, integer, bstr, empty input.
- **G6 Pack streams.**
  - `SendPackRecords` over deterministic incompressible records (math/rand seed 42, as the probe):
    0 records; pack size exactly 1 MiB; 1 MiB+1; 2.5 MiB across two records;
    a source that fails after 2 records (bytes up to the failure, no TDataEnd).
    For big streams emit the frame lengths plus SHA-256 of the stream.
  - `packReader` over: TData with and without Data; TDataEnd; dstore TErr with
    Text, without Text, with View and RetryAfter; dstore TAbsent; dstore TRefs;
    TData carrying Epoch; EOF at a frame boundary before TDataEnd; EOF mid-frame.
    Expected: bytes read plus the sticky error text.
- **G7 Tickets.**
  - `Encode` of the test ticket and of nil/empty variants.
  - `IDs()` with duplicates, a 31-byte ID, no members.
  - `Parse` of every §2.4.9 row; base32 node ids of length 48-60; all 32 last-symbol variants.
  - Lookalike runes U+0130/U+0131/U+017F/U+212A in node ids and in dstore1 bodies.
  - Every Unicode White_Space rune leading/trailing; U+180E and U+200B (not trimmed).
  - Separators `\v`, `\f`, U+00A0 inside; prefix `DSTORE1`/`dStOrE1`; body with `\r\n` every 8 symbols.
  - A ticket body with trailing 1/3/6 extra symbols after a full quantum.
  - 64-byte fields containing non-ASCII runes (hex routing).
- **G8 base32.** `DecodeString` with `StdEncoding.WithPadding(NoPadding)` over
  `"A"×n` and `"a"×n` for n = 0..17, `=` at every position of an 8-symbol input,
  and mixed newlines: output hex and error text. The same with the z-base-32 alphabet.
- **G9 Case-mapping table.** Every rune where `len(strings.ToLower(string(r)))
  != len(string(r))` or `len(strings.ToUpper(string(r))) != len(string(r))`,
  plus every non-ASCII rune whose lower or upper form is ASCII. Embed it in
  `gostr` so Rust reproduces Go's simple case mapping byte-for-byte for
  routing and error offsets.
- **G10 `strconv.Quote`.** Runes 0x00-0x7f, 0x80-0xa0, U+00AD, U+200B, U+2028,
  U+FEFF, U+E0001, U+10FFFF, combining marks, `"` and `\`.
- **G11 Curve points** (`ParseEndpointID` of hex): all-zero; y ≥ p
  (non-canonical); x = 0 with the sign bit set; a few invalid points; random
  valid keys. Expected: accept/reject, for parity with `iroh_base::PublicKey::from_bytes`.

---

## 6. Go tests worth porting (by name)

- **dstore `wire/wire_test.go`:** `TestFrameRoundTrip`, `TestErrorFrames`,
  `TestPackFramesInterop` (needs core-rs `amberpack::encode_record` and the async pack reader).
- **dstore `ticket/ticket_test.go`:** `TestRoundTrip`, `TestParseIDs` (keys via `iroh_base::SecretKey::generate`).
- **transport-iroh `protocol/protocol_test.go`:** `TestMsgRoundTrip`,
  `TestReadMsgRejectsOversizeFrame`, `TestReadMsgTruncatedFrame`, `TestRemoteError`.
  Port `TestMsgDataEndpointsRoundTrip`, `TestMsgDataEndpointsCompat` and
  `TestMsgPinNamesRoundTrip` only for the `ProtocolMsg` decoder.
- **transport-iroh `protocol/pack_test.go`:** `TestSendPackRecordsRoundTrip`,
  `TestSendPackRecordsPropagatesSourceError`, `TestPackReaderSurfacesRemoteError`,
  `TestPackReaderRejectsUnexpectedFrame`. `TestPackRoundTrip` and
  `TestSendPackPropagatesSourceError` cover `SendPack`, which the client never uses.
- **go-iroh `key/key_test.go`:** `TestPublicKeyFromStringHex`,
  `TestPublicKeyAllZeroIsValid`, `TestParseEndpointIDRejectsGarbage`,
  `TestPublicKeyInvalidCurvePoint`, `TestParseEndpointIDRejectsZ32`,
  `TestEndpointIDEncoding`, `TestParseBase32UpperAndLower`.
- **core-rs `src/reference.rs` tests, as templates for the generic decoder:**
  `decode_duplicate_key_first_wins_and_skips_unchecked`, `decode_map_key_types`,
  `decode_rejects_low_two_byte_simple_values`, `decode_simple_values_fill_created_at`,
  `decode_int_key_overflow`, `decode_text_key_utf8_checked`, `decode_tag_handling`,
  `decode_arrays_fill_byte_fields`, `decode_nesting_boundaries`,
  `decode_well_formedness_precedes_field_decoding`, `fxamacker_verbatim_messages`.
  Invert `decode_rejects_indefinite_length_map`: here indefinite maps must be **accepted**.

---

## 7. Gaps in core-rs and Rust iroh, with workarounds

| # | Gap | Evidence | Workaround |
|---|---|---|---|
| K1 | `cbor::read_head` rejects additional info 31, but fxamacker accepts indefinite-length strings, arrays and maps | core-rs `src/cbor.rs:157-192`; fxamacker `valid.go:108-137` | Own head reader in `codec::dec`; optionally propose `read_head_indef` upstream |
| K2 | The fxamacker-faithful lax decoder exists only privately inside `reference.rs` (no indefinite lengths, no 131072 caps) | `src/reference.rs:440-890`, `port-notes/reference.md:89-105` | Re-implement generically (§4.4). Later: extract a shared `cbor::lax` module in core-rs |
| K3 | `amberpack::Writer`/`Reader` are sync `std::io`; `PACK_MAGIC` and `TAG_END` are private | `src/amberpack.rs:48,53,229-283,309-449` | Write the framing directly (magic + records + `00`) and parse records asynchronously with `amberpack::parse_record`; or propose `pub const`s plus a push-based parser upstream |
| K4 | core-rs `Reader` read errors embed Rust `io::Error` text (e.g. "failed to fill whole buffer"), not Go's "unexpected EOF" | `src/amberpack.rs:323-327` | In the async reader, format truncation as `unexpected EOF` to match Go's `amberpack: malformed pack stream: <what>: unexpected EOF` |
| K5 | `iroh_base::PublicKey::from_str` rejects non-zero trailing bits, has no Unicode case mapping, no z-base-32 hint | `iroh-base-1.0.3/src/key.rs:249-257,475-496`; data-encoding `src/lib.rs:1902` | `ticket::parse_endpoint_id` (§2.4.6) + `PublicKey::from_bytes` for the curve check |
| K6 | data-encoding decoding rejects the lengths and trailing bits Go accepts | data-encoding `check_trailing_bits` | `gobase32::decode_nopad` mirroring Go's `decode` loop; data-encoding for encoding only |
| K7 | Rust std case mapping is full mapping; Go uses simple mapping | Go `unicode.ToLower/ToUpper` vs Rust `char::to_lowercase` | Embedded table from G9 |
| K8 | No `strconv.Quote` in Rust | — | `gostr::quote` pinned by G10 |
| K9 | tokio `read_exact` cannot tell a clean EOF from a partial header | tokio `AsyncReadExt::read_exact` | Counting loop over `read` in `read_msg` |
| K10 | noq `finish()`/`stop()` return `ClosedStream` when already closed; writes after a peer STOP_SENDING fail with `WriteError::Stopped` | `noq-1.1.1/src/send_stream.rs:188-212`, `recv_stream.rs:275` | Ignore close errors (as Go does); surface write errors as `WireError::Io` |

---

## 8. Risks and open decisions

- **R1 (decision) Verbatim fxamacker error texts.** They reach users only
  through malformed peers (`wire: decode frame: …`) and malformed tickets
  (`ticket: cbor: …`). **Recommend verbatim**: tickets are user input, and one
  error framework serves both.
- **R2 (decision) Lax decoding.** Replicate tag stripping, bignums, simple
  values into integers, arrays into bytes and first-duplicate-wins. Rejecting
  them instead only changes outcomes for non-Go peers. **Recommend replicating**; G4/G5 lock it.
- **R3 (decision) Pack-reader frame typing.** Either a full `ProtocolMsg`
  decoder (§2.3.2), which reproduces Go's rejections and texts (e.g. a TData
  carrying key 3), or decode only keys 0/8/10/11/12. **Recommend full.**
- **R4 (decision) Go case mapping.** A full table (G9), or ASCII plus the 4
  lookalikes. The accept set is identical either way; they differ only in
  hex/base32 routing for 64-byte fields containing length-changing runes, i.e.
  the error text. **Recommend the table** for 100 %.
- **R5 (decision) Async pack reader.** Re-implement the amberpack framing
  asynchronously (recommended; ~80 lines, exact error control), or run the core-rs sync
  `Reader` under `spawn_blocking` with a sync bridge (`tokio-util`
  `SyncIoBridge`; offline availability not checked).
- **R6 (decision) `[][]byte` nil elements.** Model them as `Vec<Vec<u8>>`: a
  null element decodes as empty and would re-encode as `40`, not `f6`. Clients
  never re-encode decoded key lists, so **recommend `Vec<Vec<u8>>`**. Use
  `Option` only for the non-omitempty `[]byte` fields of §4.3, where
  re-encoding does happen (`cluster ticket` re-encodes a parsed ticket).
- **R7 Put vs stale-view timing.** `handlePut` checks the epoch *before*
  reading the pack, replies `stale-view` and closes (FIN + STOP_SENDING)
  while the client may still be sending. Go's `putOnce` then sometimes gets a
  write error (`SendPackRecords` fails, not retried as stale) and sometimes the
  stale frame from `Expect`. The Rust client must keep the same ordering
  (send the whole pack, then read) so its behaviour matches Go's, races included.
- **R8 Size caps.** A ref-watch known list is capped at 131072 entries (not the
  design note's ~200k); put replies stay decodable only if batches keep ≤ 8192
  keys. Keep Go's batching constants.
- **R9 Memory safety.** Check the length against `MAX_FRAME` before
  allocating the payload. Pass 1 must reject claimed counts over 131072 before
  allocating, and never size allocations from untrusted counts (core-rs
  `port-notes/cbor.md:51-64` records the Go presize DoS).
- **R10 Toolchain drift.** The stdlib behaviour here is go1.26.5's. A Go bump
  can change Unicode tables (G9/G10) and, in principle, `encoding/base32`.
  Regenerate the vectors when dstore's `go.mod` `go` line changes.
- **R11 Float encoding.** Only `AdminRequest.Garbage` is a float.
  `ShortestFloat16` uses x448/float16 `PrecisionFromfloat32` plus a round-trip
  test (`encode.go:1135-1160`); pin it with the G1 float list.
- **R12 Go `int` width.** dstore targets are 64-bit, so `Type`, `Limit`,
  `Shortfall` and the `node.Status` ints are `i64`. Overflow details print `int`, not `int64`.
- **R13 Invalid UTF-8.** Go strings may carry invalid UTF-8 (e.g. names or
  patterns from non-UTF-8 CLI args), which Go encodes as-is and every decoder
  rejects. Rust `String` cannot carry them; the CLI spec must decide on
  `OsString` handling.
- **R14 Record bytes differ.** Newly compressed records differ between Go
  (klauspost zstd) and core-rs (libzstd) (`src/amberpack.rs:27-30`), so pack
  and TData bytes are only byte-identical for identical record inputs. Golden
  pack vectors must use incompressible or pre-encoded records.
- **R15 Node-side commands** need these, beyond this area:
  - **`cluster init`**: the node (Pebble meta, paxos catalog `InitCluster`) and
    the endpoint's addrs; from this area only `Ticket::encode`.
  - **`cluster ticket` via `--store`**: `node.OpenOffline` (Pebble meta holding
    the persisted view) plus `worktree.TicketFromView`; from this area `Ticket::encode`/`ids`.
  - **`node join --seed`**: `ticket::parse`, TJoin on the cluster ALPN
    (§2.7), then running a node.
  - **`serve`, `catalog restore`**: none of this area beyond the codec.
  - **Recommendation:** implement ticket and TJoin encoding in the library
    now, and leave the store-opening commands to a separate decision (a later
    port, or an explicit "unsupported" error).


---

## Addenda (synthesis)

Added by the architecture synthesis. `PORTING.md` is normative where it differs from this spec.

1. **Crates.**
   - The codec is `dstore-codec`.
   - `wire`, the pack framing and the admin/status payloads are `dstore-wire`.
   - The ticket is `dstore-ticket`.
   - The `gostr` helpers are `dstore-gocompat` (`quote`, `strings`, `base32`).

   The public signatures of PORTING.md §4.2-§4.4 replace §4.4 here, including the `cbor_struct!`
   syntax `0 => field: Type = "go type" [omitempty]`.
2. **Go constant names** map to Rust as `TFooBar` → `T_FOO_BAR` (`TOK` → `T_OK`,
   `TCASMismatch` → `T_CAS_MISMATCH`, `TGCStatusRep` → `T_GC_STATUS_REP`) and `CodeFooBar` →
   `CODE_FOO_BAR`. The Go names, in the order of §2.2.1's strings: `CodeStaleView`, `CodeNotOwner`,
   `CodeNoSpace`, `CodeBusy`, `CodeBadRequest`, `CodeUnauthorized`, `CodeUnknownRef`, `CodeCASMismatch`,
   `CodeIncomplete`, `CodeUnavailable`, `CodeTimeout`, `CodeInternal`, `CodeNotMember`, `CodeNeedView`,
   `CodeExpired`, `CodeAmnesiac`, `CodeMarkFrozen`, `CodeRetired`, `CodeTooSoon`, `CodeConflict`,
   `CodeNoMark` (`wire/wire.go:107-129`).
3. **`ticket::parse(&[u8])`** and `parse_endpoint_id(&[u8])` take bytes. Go strings can carry
   invalid UTF-8, and `%q` in `ticket: %q is neither …` echoes the original bytes.
4. **`protocol.Msg` key 15 (`DataPorts []uint16`)** needs codec support for `Vec<u16>`.
5. **Decisions on §8.**
   - R1: verbatim fxamacker texts, **asserted** by every vector (verification.md §2.1 called them
     informational; overridden).
   - R2: replicate the lax decoding.
   - R3: full `ProtocolMsg` typing.
   - R4: the full case-mapping table, generated into `crates/gocompat/src/tables.rs` by
     `tools/vectorgen/cmd/gotables`.
   - R5: async re-implementation (`PackSender`, `PackReader`, `PackRecords`).
   - R6: `Vec<Vec<u8>>` in `wire`; views use `Vec<Option<Vec<u8>>>`.
   - R7: send the whole pack, then read.
   - R13: non-UTF-8 argv placed into CBOR text fields is sent lossily (PORTING DD-8).
   - R15: node-side commands per PORTING §2.2.
6. **Error chains.** Wrapper variants (`WireError::Remote`, `PackReadError::Remote`, client wrappers)
   use `#[error("{0}")] X(#[source] Inner)`, never `#[error(transparent)]`, so `wire::as_remote`
   finds the inner error by walking `source()`.
7. **Base32 node ids.** verification.md §2.2 claims go-iroh rejects 52-character ids with non-zero
   trailing bits. That is wrong: `decodeStdBase32NoPad` is Go's `encoding/base32`, which ignores
   trailing bits (`key/key.go:505-524`, `key_core.go`). §2.4.6 here is correct (PORTING C4).
