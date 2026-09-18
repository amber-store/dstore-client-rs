# impl-vectorgen-proto: layer L1 notes

Owner: vectorgen-proto. Files: `tools/vectorgen/family_wire.go` (families `wire`, `wire-pack`),
`family_ticket.go` (`ticket`), `family_admin.go` (`admin`), `tools/vectorgen/docs/vectorgen-proto.md`, and the
generated `tests/golden/{wire,ticket,admin,status}/`. Schemas and self-checks are in the docs file.

## Findings for the Rust owners

1. **amberpack `Records` wraps read errors as text** (core v0.0.8 `amberpack/pack.go:140-195`): every error from the
   pack reader becomes `amberpack: malformed pack stream: <step>: <err>` with `%v`, where step is `reading magic`,
   `truncated before end marker`, `truncated record header` or `truncated record payload`; for example
   `amberpack: malformed pack stream: truncated before end marker: remote: busy: ` (`terr-busy-no-text`) and
   `amberpack: malformed pack stream: truncated record payload: remote: internal: sender died` (`terr-mid-record`)
   in pack_reader.json. This is
   what `client.getStream` returns. PORTING §4.3 comments `PackRecordsError::Read(#[source] PackReadError)` as
   displaying the read error alone. The wire-pack owner must render Go's wrapped text (for instance through
   `Stream(String)`, or by making `Read` display `"amberpack: malformed pack stream: {step}: {0}"`). In Go the
   wrapped error no longer matches `*protocol.RemoteError`.
2. **Stamped dstore frames during a pack fail to decode.** Key 2 (`Incarnation`) is `protocol.Msg.Root` (`[]byte`),
   so a stamped TErr or TPutResult arriving mid-pack gives `protocol: decode frame: cbor: cannot unmarshal positive
   integer into Go struct field protocol.Msg.2 of type []uint8`, not a remote error. The specs' probes used
   unstamped frames; pack_reader.json has both.
3. **urfave stops flag parsing at the first positional argument.** verification.md §5's argv lists
   (`cluster replicas 2 --yes`, `node remove <id> --dead --allow-unsafe`) would prompt or drop the flags in Go.
   admin/requests.json puts flags first and adds `node-remove-flags-after-id-ignored`.
4. **`token create --weight 4294967296`** gives `Weight: 0` (Go `uint32(c.Uint(...))` truncates), so the key is
   omitted.
5. **Curve acceptance** (ticket/curve.json): filippo accepts non-canonical y ≥ p whenever y mod p decodes, and x = 0
   with the sign bit set (y = 1 and y = p-1, both signs; all-0xff). The ticket owner must confirm that
   `iroh_base::PublicKey::from_bytes` (dalek) gives the same set; if it rejects some, PORTING §4.4's
   `is_valid_public_key` needs its own decompression check.
6. **`wire.ErrorFromMsg` keeps a negative RetryAfter** as a negative Duration (`ver-err-busy-negative-retry`:
   `-5ms`). PORTING §4.3 clamps to ZERO in Rust; tests should compare `retry_after_ns` only when non-negative.
7. **go-iroh accepts 52-symbol base32 ids with any trailing bits** (16 of the 32 last-symbol variants give the
   same id; the others flip a data bit), confirming PORTING C4 against verification.md §2.2.
8. **A z-base-32 id without `8`, `1` or `9`** decodes under RFC 4648 as a different valid id and is accepted
   (`z32-rfc-decodable-id33`).

## Deviations from the specs' schemas (verification.md §4.3)

- `wire/frames.json` splits cases into `requests`, `replies`, `other` and `bulk`, adds client-derived extras, and
  merges identical frames (`aliases`). `MsgJSON` uses the Rust snake_case field names, not the Go names.
- `wire/decode.json` is grouped by target type (`wire_msg`, `protocol_msg`, `ticket`, `admin_request`,
  `admin_reply`, `status`) and holds the G4 matrix and G5 structure cases; each case carries `value` and
  `canonical_hex`; `go_error` is asserted (PORTING C7).
- `wire/frame_errors.json` records every read until the first error (`reads`) for wire and protocol, plus
  `writes` and `expect`.
- `wire/pack_frames.json` describes records by `header_hex` + `payload` (or `record_seq`), frames with `frame_len`
  and `head_hex`, and a Go read-back.
- `admin/replies.json` and `status/status.json` add `frame_hex`, printed outputs and decode cases.

## Not generated here

- `ticket/base32.json` belongs to another owner (not in this task).
- No live capture of requests through `client.Dial` over `transport.NewNetwork()`: request frames are built with the
  exact field assignments of the client code (`stamp`, `Cond.apply`, `RefList`, `watchOnce`, `Admin`) and checked
  against the verified hex of the specs.

## Review

Reviewer: review-vectorgen-proto. Re-read PORTING §0-3, §4.3-4.4, §5-7, codec-wire-ticket §2.2-2.6, §3, §5, the
client-core §3.1-3.4 and §5, client-transfer §3.1-3.3 and §5, cli §2.7-2.8, §3.4, §3.8 and §5, and verification §2,
§3.1-3.2, §4.3 and §5 with their addenda. Checked the generator against the Go sources: transport-iroh
`protocol/pack.go` and `protocol.go`, core `amberpack/pack.go` `Records`, dstore `wire/wire.go`, `cmd/dstore/main.go`
(the 17 `node.AdminRequest` literals, the flag definitions, urfave ordering), `cmd/dstore/client.go` (`adminAction`,
`printStatus`), `view.IDsOf`, `client/watch.go` and `client/fetch.go`, and go-iroh `key.go`. Before fixing anything,
two regenerations were byte-identical to each other and to the committed files.

### Fixed

1. **frames.json silently dropped cases.** `addFrame` stored `&(*dst)[len(*dst)-1]` in `seen`. Once `append` grew
   the list, a later duplicate frame added its alias to a stale copy. That lost the verification §5 case
   `ver-ref-put-keyed-new` (`keyed-new`) and `uint-epoch-0`. `seen` now stores the list and index, and `wvCheckNames`
   fails unless every added name appears exactly once as a case or alias. The two aliases are back under
   `ref-put-versioned-nil` and `view-unstamped`.
2. **The Kelvin-sign lookalike cases held ASCII `K` (0x4b), not U+212A.** This hit `body-kelvin-k`,
   `b32id-kelvin-k`, `b32id-all-lookalikes` and `61-hex-then-kelvin`, so codec-wire-ticket G7's U+212A was never
   tested. Every non-ASCII rune in `family_ticket.go` is now a `\u` escape. With the real rune, Go rejects U+212A in a
   body (`ticket: illegal base32 data at input byte 28`, because `ToUpper` keeps it) and accepts it in an id (`ToLower`
   gives `k`).
3. **Invalid UTF-8 in a JSON string.** The status case `free-bytes-negative` had zone `"\xff"`. Go's JSON encoder
   wrote U+FFFD, so `node.zone` disagreed with `node.line` (`zone "\xff"`). This broke VECTORS.md, which requires
   non-UTF-8 text to go in a `_hex` field. A view zone is a CBOR text field: fxamacker rejects invalid UTF-8 and Rust
   holds a `String`, so such a zone is unreachable. The case now uses U+00AD (`zone "­"`). The generator refuses
   invalid UTF-8 zones.
4. **Nil `[][]byte` elements had no marker** (codec-wire-ticket R6). `other/ref-changes-nil-elements` and the status
   case `unreachable-odd-ids` encode `f6` elements, which Rust `Vec<Vec<u8>>` writes as `40`. Under the documented rule
   "encode == frame_hex unless decode_only", those tests could never pass. FrameCase and status cases now carry
   `null_element: true`, and the docs say to skip the re-encoding comparison.
5. **The status decode case `unknown-key` was a type error.** Its payload was `a2 00 01 18 63 01` (uint into
   `node.Status.0`), in both status.json `decode` and decode.json `status`. It is now `a2 01 05 18 63 01` (ok, epoch 5).
   The duplicate `status-reply-a0` (same bytes as `a0`) is replaced by `id-null` (`a1 00 f6`).
6. **Self-checks were narrower than reported.** Only 22 of cli §3.8's 33 request rows were checked. The check now
   covers all 33, plus verification §3.2 and codec §2.5.1 `gc-run` 0.5 and 0.3, and every cli §3.8 reply row's hex and
   printed output (including `token`).
7. **Finding 1 quoted a text that is not in the vectors** (`… truncated before end marker: remote: busy: late`). In
   `terr-after-end-marker`, `Records` ends cleanly and the TErr shows only as `drain_error`. The quote is corrected.

### Verified, unchanged

- The pack framing and reader outcomes match `pack.go` and `Records`: sticky errors, TDataEnd, a clean EOF at a
  frame boundary, and `%v` wrapping. The source-error cases keep the chunk boundary clear of bufio's 4096-byte window,
  so a Rust `PackSender` dropped without `finish()` writes the same frames.
- frame_errors matches `wire.ReadMsg`, `protocol.ReadMsg`, `WriteMsg` and `Expect`.
- The argv mapping matches main.go, including urfave's stop at the first positional argument, `uint32` truncation
  of `--weight`, and `-0` garbage being omitted.
- The adminAction and printStatus copies match (Println spacing, `FreeBytes>>30` on negative values).
- The curve set matches, and all-0xff is valid.
- The JSON conventions hold: 64-bit integers as strings, lowercase hex, `null` vs `""`, structs only.
- Every case listed in verification §5, codec G1-G7 and G11, client-core §5 items 1, 2 and 4, client-transfer §5.2
  items 1 and 2, and cli §3.8 is present.

### Deviations and remaining

- The admin source check is a whitespace-normalised substring check over `cmd/dstore`, not the go/printer AST
  comparison of PORTING §7. It still fails on drift, so it is kept.
- For the L2 owners: finding 1 (`PackRecordsError::Read` display) and finding 5 (dalek curve parity) are still open.
  The per-family docs are not yet folded into VECTORS.md (not an owned file).
- Gates after the fixes: `gofmt -l` clean, `go vet .` ok, `go test -count=1 .` ok. Two regenerations into scratch dirs
  were identical, and the regenerated tests/golden files equal them.
