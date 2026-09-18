# impl-wire: layer L2 notes

Owner: wire. Files: `crates/wire/src/lib.rs`, `consts.rs`, `msg.rs`, `error.rs`, `frame.rs`, `keys.rs`,
`admin.rs`, and `tests/golden_tests/wire.rs`. `pack.rs` belongs to wire-pack.

## What is implemented

- **consts, msg, admin declarations.** Checked field by field (key, Go type, omitempty) against
  `wire/wire.go`, `node/admin.go` and `node/status.go`. They are unchanged from L0.
- **error.rs.**
  - `RemoteError::message` / `is`, `error_from_msg`, `err_msg`.
  - `as_remote` walks `err`, then `source()`, with `downcast_ref`.
  - `is_code`.
- **frame.rs.**
  - `encode_frame`: `TooLarge` over 16 MiB, nothing written.
  - `write_msg`: one `write_all`.
  - `read_msg`: a counting `read` loop for the header and for the payload. The length is checked before
    the payload is allocated.
  - `expect`, `write_err`.
- **keys.rs.** `keys32` (`KeyLen` for the first bad entry), `raw_keys`.
- **admin.rs.** `decode_status`, `decode_admin_reply` (`codec.Unmarshal`).
- **lib.rs.** The crate-level L0 `#![allow(dead_code, unused_variables)]` is removed. While pack.rs was still
  stubbed it was scoped to `mod pack`; wire-pack has since landed, and clippy is clean without it.

Nothing in PORTING.md §4.3 was changed and nothing was added.

## Decisions

1. **`error_from_msg` computes Go's product.** Go computes `time.Duration(m.RetryAfter) * time.Millisecond`,
   a wrapping `int64` product. Rust uses `wrapping_mul(1_000_000)`, so every non-negative Go duration is
   reproduced, including wrapped ones (18446744073710 ms is 448384 ns in Go too). A negative result is ZERO
   (PORTING §4.3). Every caller tests `RetryAfter > 0`, so this matches Go.
2. **`read_msg` error mapping.**
   - Header phase: an I/O error of kind `UnexpectedEof` becomes `WireError::UnexpectedEof`, as Go's
     `errors.Is(err, io.ErrUnexpectedEOF)` does. Other errors are `Io`.
   - Payload phase: errors are wrapped as they are (`Short(Io)`), as Go's `%w` does.
   - `Interrupted` reads are retried.
3. **`expect` does not hand back the frame with an error.** The §4.3 signature is
   `Result<Msg, WireError>`. No Go caller reads the frame that `Expect` returns alongside an error
   (`client/fetch.go:288` discards it, `client/objects.go:292` returns on error). The TErr fields travel in
   `RemoteError`; a type mismatch keeps `got`/`want`.
4. **`encode_frame` copies the payload.** It marshals, then copies into the length-prefixed buffer, as Go's
   `WriteMsg` does, so a 16 MiB frame peaks at twice its size.

## Tests

### Unit tests (27, in the owned modules)

- dstore `wire_test.go`: `TestFrameRoundTrip`, `TestErrorFrames`, `TestPackFramesInterop`.
- **Header loop:** byte-at-a-time reader, 0-3 header bytes, I/O errors passed through, kind-`UnexpectedEof`
  mapping.
- **Payload:** too-large refused before the payload is read (a reader that fails on a second read); short
  frames (`EOF`, `unexpected EOF`, I/O); decode errors (length 0, array top level).
- **Writes:** one `poll_write` per frame; nothing written when too large; the max-frame head; a write error
  rendered through `io_error_text`.
- **Errors:** texts, `is`, retry_after edges, `as_remote` through wrapper chains, `ProtocolRemoteError` never
  matching.
- **Probes:** `keys32`/`raw_keys`; the cli §3.8 admin and status hex; the codec-wire-ticket §2.2.2 and §3.3
  probes.

### Golden tests (`tests/golden_tests/wire.rs`, 20 tests)

- **`wire/frames.json`.**
  - `requests`/`replies`/`other`: `encode_frame == frame_hex` unless `decode_only`/`null_element`;
    `read_msg == msg`, frame consumed exactly; `remote_error` through `error_from_msg`; the `keys32`
    extra; `incomplete.sample` (the `keys32` result, empty on error).
  - `bulk`: `frame_len`, `head_hex`, BLAKE3, read back.
  - `keys32`, `error_is`, `is_code`.
- **`wire/decode.json`, all six sections.**
  - Decoding: `unmarshal::<Msg | ProtocolMsg | dstore_ticket::Ticket | AdminRequest>`, `decode_admin_reply`,
    `decode_status`. Decision, verbatim `go_error`, `value`, re-encoding (`canonical_hex`, or length plus
    BLAKE3).
  - `wire_msg` also through `read_msg`, `protocol_msg` through `read_protocol_msg` (`… decode frame: `
    prefixes).
  - VECTORS.md names `codec.rs` for this file, but `codec.rs` reads only `codec/*.json`, so it is asserted
    here.
- **`wire/frame_errors.json`.** `reads` via `read_msg` and `read_protocol_msg` (result kind, text, message);
  `writes` (bytes written, head, BLAKE3, `encode_frame` agreeing); `expect` (kind, text, `RemoteError` equal
  to `error_from_msg` of the frame, `remote` JSON, protocol `got`/`want`).
- **`admin/requests.json`.** `marshal == params_hex`, decode back, stamped TAdmin frame (cid16, 1, 7) encode
  and read.
- **`admin/replies.json`.** `marshal == status_hex`, `decode_admin_reply`, TAdminReply frame, `decode` cases.
- **`status/status.json`.** `marshal == hex` unless `null_element`, `decode_status`, `decode` cases.
- **Left to the owners of the producing functions:** the client extras (`cas_mismatch`, `incomplete.error`,
  `short_ids`, `holders_ids`) and the CLI outputs (`argv`, `printed`, `printed_token_create`,
  `cluster_ticket`, `node`, `unreachable`).
- **BLAKE3 digests.** The root package has no `blake3` dev-dependency, and this task may not edit its
  manifest. `amber_store_core::key::Key::new(Blob, 0, data)` stores digest bytes 0..30 in key bytes 2..32,
  so the tests compare that 30-byte prefix. Adding `blake3` to the root `[dev-dependencies]` would allow the
  full digest.

## Findings

1. **`admin/requests.json` `request` is the Go value, not what the wire carries.** A decoded request can
   differ from it:
   - `gc-run-garbage--0`: -0.0 is omitted (omitempty), so it decodes as +0.0;
   - `gc-run-garbage-NaN`: bits `7ff8000000000001` are written `f9 7e00` (NaNConvert7e00), so the value
     decodes as `7ff8000000000000`, as `decode.json` `gc-run-nan` confirms.

   The golden test compares decoded requests against the carried value. cli-admin tests that compare
   decoded requests need the same rule; tests that compare `marshal` output are unaffected.
2. **`TestErrorFrames`** checks `m != nil` alongside the error. The Rust port asserts the carried view
   instead (decision 3).

## Gates

- `cargo test -p dstore-wire`: 58 passed (27 owned, 31 of wire-pack), 0 ignored.
- `cargo clippy -p dstore-wire --all-targets -- -D warnings`: clean.
- `rustfmt --check --edition 2024` over the owned files: clean.
- **Golden `wire::`:** 20 passed, 0 ignored, and clippy over the module is clean. Both were run through a
  scratch Cargo package outside the repo, deleted afterwards. It had path dependencies on codec, gocompat,
  testkit, ticket and wire, and included `tests/golden_tests/wire.rs` with `#[path]`.
- **Root golden run blocked.** `cargo test -p dstore-client-rs --test golden wire::` could not build while
  sibling crates were mid-edit:
  - dstore-worktree `sys.rs` (`OsString::into_vec` without `OsStringExt`, plus a lifetime error);
  - earlier, dstore-transport (`Unreachable` not found).

  The review below ran the real root binary.

## Review

Reviewer: review-wire. Every owned file was checked line by line against:
- dstore `wire/wire.go` and `wire/wire_test.go`;
- `node/admin.go:18-46` and `node/status.go:13-116`;
- transport-iroh v0.4.0 `protocol/protocol.go:124-164`;
- the generator (`tools/vectorgen/family_wire.go`, `genWireFrameErrors`).

### Verified, no change

1. **Declarations.** Checked for every field: the key, the Go type name the codec prints, and omitempty. This
   covers every `T*` constant, the 21 codes, the limits, `Msg` and its five sub-structs, `AdminRequest`,
   `AdminReply`, `VoterStat` and `Status`.
2. **`read_msg`.**
   - The header follows Go's `io.ReadFull`: EOF after 0 bytes, `unexpected EOF` after 1-3 bytes, and any other
     error returned as is.
   - An io error of kind `UnexpectedEof` becomes `UnexpectedEof`, as `errors.Is(err, io.ErrUnexpectedEOF)`
     would. noq 1.3.0 never produces that kind: `ReadError` maps to `ConnectionReset` or `NotConnected`
     (`recv_stream.rs:675-684`). So this branch is reachable only through other readers, and the text is the
     same either way.
   - `n > MaxFrame` is refused before allocating. The payload phase wraps the reader's error as Go's `%w`
     does, and `n == 0` gives `wire: decode frame: EOF`.
3. **`encode_frame` / `write_msg`.** An oversized payload is refused with nothing written, and a write error is
   returned unwrapped. The single write of Go's `io.Writer` is a `write_all` loop here.
4. **`expect`.** The order matches Go: the read error, then TErr (so `want == T_ERR` still errors), then the
   type check (`want == 0` accepts any type). Dropping the frame that Go returns alongside the error is safe:
   both Go callers return on error (`client/objects.go:292-296`, `client/fetch.go:288-291`).
5. **`error_from_msg`.** It takes Go's wrapping product and turns a negative result into ZERO. The only Go
   reader of `RetryAfter` (`client/objects.go:231`, `we.RetryAfter > 0`) behaves the same.
6. **`is_code`.** It tests the first remote error in the chain, as `errors.As` does, even when a deeper one
   carries the code.
7. **Vector audit.** Every field produced by a wire function is asserted, across all six files. Fields left to
   others:
   - `frames.json`:
     - `aliases` and `call` are names only;
     - `cas_mismatch` and `incomplete.error` belong to client-a;
     - `short_ids` and `holders_ids` are `view::ids_of`, for the client tests.
   - `admin/requests.json` `argv`; `admin/replies.json` `printed`, `printed_token_create` and `cluster_ticket`;
     `status/status.json` `node`, `printed` and `unreachable`: all cli-admin.
   - `decode.json` `field` and `shape_hex` are labels only.
8. **`frame_errors.json` has no `io` read results.** Go reads over a `bytes.Reader`, so no vector reaches
   those paths; the unit tests cover them.
9. **The 30-byte BLAKE3 prefix is kept.** For length 0, core-rs `Key::new_from_hash` copies `full_hash[..30]`
   into key bytes 2..32 (`src/key.rs:131-139`), so the comparison covers 240 digest bits.
10. **The golden `admin_requests` rule for the carried value is right.**
    - fxamacker omitempty treats -0.0 as empty (`v.Float() == 0`).
    - `CanonicalEncOptions` writes every NaN as `f97e00`, which decodes to `7ff8000000000000`.
11. **`TestFrameRoundTrip`, `TestErrorFrames` and `TestPackFramesInterop` are ported.** No test was ignored,
    so there was nothing to un-ignore.

### Changed

1. **`as_remote` also matches a `Box<RemoteError>` or `Arc<RemoteError>` in the chain.**
   - std's `impl Error for Box<T>` and `Arc<T>` forward `source()` to `T::source()`. A remote error held
     directly behind one of these pointers was therefore invisible to the walk. In Go a `*wire.Error`
     pointer matches.
   - Chains through `Arc<dstore_client::Error>` already worked, because the `Arc` forwards to the client
     error's own source.
   - Test: `error::tests::as_remote_sees_through_box_and_arc`.
2. **Unit tests for paths no vector reaches.**
   - `interrupted_reads_are_retried`: both phases.
   - `payload_unexpected_eof_error_is_a_short_frame`: `Short(Io)`, text `wire: short frame: unexpected EOF`.
   - `write_msg_completes_short_writes`: a writer that takes 3 bytes per call.

### Review gates

- `cargo test -p dstore-wire`: 62 passed (31 owned, 31 wire-pack), 0 ignored.
- `cargo clippy -p dstore-wire --all-targets -- -D warnings`: clean.
- `rustfmt --check --edition 2024` over the owned files: clean.
- **Before the review edits:** `cargo test -p dstore-client-rs --test golden wire::` on the real root binary gave
  20 passed, 0 ignored.
- **After the edits the root binary did not build.** Two siblings were mid-edit:
  - `dstore-client` (lib): `E0106 missing lifetime specifier`;
  - then the `crates/cli` manifest: `duplicate key`.
- **Post-edit run instead.** The golden `wire` module ran through a scratch package outside the repo, with
  path dependencies on codec, gocompat, testkit, ticket and wire, the repo's `Cargo.lock`, and `#[path]` to
  `tests/golden_tests/wire.rs`. Deleted afterwards.
  - 20 passed, 0 ignored.
  - `cargo clippy --tests -- -D warnings` over that package, which lints `wire.rs`: clean.
