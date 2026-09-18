# impl-vectorgen-client: layer L1 notes

Owner: vectorgen-client. Go generator only: `tools/vectorgen/family_client.go` (family `client`),
`tools/vectorgen/family_refglob.go` (family `refglob`), `tools/vectorgen/docs/vectorgen-client.md` (every
schema), and the generated `tests/golden/client/{rank,batches,fetch,verify_record,progress,backoff,placement_decisions}.json`,
`tests/golden/errors/client_text.json`, `tests/golden/refglob/refglob.json`. `client/transcripts/` is left to
the later transcripts family (no overlap: the family owns files, not the `client` directory).

## How the values are produced

- Unexported Go code (`rankOwners`, `rttClass`, `batches`, `estSize`, `pickBatch` and its types, `tracker` and its
  methods, `countKeys`, `pathAttrs`) is copied verbatim. `clientSelfCheck` compares every copy with the dstore
  v0.1.9 source (AST, comments dropped, a fixed rename map for types the copies take from package `client` or
  stub), checks the statement copies of `handleErr` (backoff) and `WatchRefs` (reconnect delay), checks that every
  format string of the error vectors is a string literal of the named function or variable, and that the timing
  expressions occur in the named functions. Negative tests were run: a drifted `rttClass` and a drifted format
  literal both fail the run.
- `placement_decisions.json` and most of `errors/client_text.json` come from the real client over
  `transport.Network` with a scripted fake node (`clientServe`, `clientViewReplier`): `client.Dial`, `Owners`,
  `WriteSet`, `ReadOrder`, `Primary`, `Placed`, `RefGet/Put/Delete/List`, `Admin`, `Missing`, `Push`, `PullTree`.
  Push and PullTree run against temporary packstores (removed afterwards).
- The Go tests' expectations (`rank_test.go`, `batch_test.go`, `refglob_test.go`) are asserted before writing.
- Two runs give identical bytes (checked with `diff -r`).

## Findings for the Rust owners

1. **`est_size` can be negative in Go** (client-b). `estSize` is `46 + min(int(k.Length()), 64 KiB)`, and the
   `uint64 → int` conversion wraps for a length field of 2^63 or more: length 2^63 gives
   `-9223372036854775762`, 2^64-1 gives `45`. PORTING.md §4.8 declares `est_size(k) -> usize`, which cannot hold
   these. Such keys reach the fetcher only from user-supplied hex keys (`catalog restore KEY`), where they change
   the accumulator byte counts and so the batch composition. `client/fetch.json` `est_size` carries these cases
   (`noncanonical_0`, `_1`, `_2`, `_7`) with `est` as a decimal string. Suggested: add an `i64`-returning variant
   for the fetcher rather than change the §4 signature.
2. **zstd error texts** (client-b). `verify_record.json` marks the two zstd-library texts
   (`zstd: decompressed size exceeds configured limit`, `zstd: invalid input: magic number mismatch`) with
   `error_portable: false`; libzstd texts differ. The length-mismatch text (`decompressed to 4096 bytes, header says
   4097`) is portable.
3. **`VerifyRecord` ignores the flag bits other than zstd and the raw `ulen`** (flags 2 and 3 accepted, raw
   `ulen != slen` accepted). These inputs never come from a pack stream (`ParseRecord` refuses them); the cases
   have `parses: false`.
4. **`Rate` with a negative duration** (client-a). Go gives `-` for `took <= 0`; the Rust `rate(bytes, Duration)`
   cannot take a negative duration, so tests skip or map the `took_ns < 0` cases.
5. **Tracker paths** (client-a). The tracker scenarios script `pool.Path` per node. `live_mem_paths` matches what
   `dstore_transport::mem` reports (direct, 1 ms); `relayed_path` needs a test hook for the pool path.
6. **refglob invalid UTF-8** (testkit owner). `refglob.json` `invalid` has patterns that are not UTF-8
   (`pattern` absent, `pattern_hex` only); `dstore_testkit::refglob::compile(&str)` cannot receive them.
7. **Vector file name.** PORTING.md §7 and the doc comments of `tests/golden_tests/client.rs` and
   `client_transfer.rs` say `errors/text.json`; this family writes `errors/client_text.json` (the task's name; the
   worktree texts live in another family's file). Those modules should read `errors/client_text.json`.
8. **Crate unit tests.** `rank.json`, `batches.json`, `fetch.json`, `backoff.json` and the tracker part of
   `progress.json` test `pub(crate)` functions, so they belong in `dstore-client` unit tests (PORTING §7). The
   crate does not depend on `dstore-testkit`; its manifest needs a dev-dependency (testkit or serde_json) for
   those tests. I did not edit manifests.

## Deviations from the area specs (recorded, not changes to §4)

- client-transfer §5.1 proposed `tools/xfervectors` and `tests/golden/transfer/`; PORTING.md §7 supersedes it
  (one vectorgen module, `client/` files).
- client-transfer §5.2 item 5's `est_size.json`/`pick_batch.json` are one file, `client/fetch.json`; client-core
  §5 item 5's HumanBytes/Rate are in `client/progress.json` (with `path_attrs` and `count_keys`).
- verification §4.3 item 10's `client/rank.json` schema also held `batches` and `placed`; those are in
  `client/batches.json` and `client/placement_decisions.json`, and `rank_owners` cases list per-owner objects.
- client-transfer §5.2 item 8 asks for 64 keys per view. The two main scenarios now have 64 keys (all-zero and
  all-`ff` keys plus 62 splitmix64 keys); the other four have 16 or 4. They had 24 before the review.
- verification §4.3 item 22: worktree sentinels, `%w (%v)` of `ErrRefChanged` and the `ReadMsg` errors are not in
  `errors/client_text.json` (worktree and wire families own them). `reference.Decode` pass-through texts are core
  texts and are not included.

## State of the shared module during this session

While I worked, `go vet` of `tools/vectorgen` failed on sibling files mid-edit (`family_text.go` byte order
marks, `family_wire.go` undefined identifiers), so development runs used a scratch copy of the module (`main.go`,
`util.go`, `deps.go`, `go.mod`, `go.sum`, `main_test.go` and my two files): `gofmt -l` clean, `go vet` clean,
`go test` passes. The vectors in `tests/golden` were written by that copy (it touches only the owned paths). At the
end the shared module built: `go vet` passed and `go run . <tmp> client refglob` produced byte-identical output
(`diff -r`).

## Review

Reviewer: review-vectorgen-client. Checked against dstore v0.1.9, core v0.0.8 and core-rs rev a85ffa1.

### What was verified

- **Gates.** `gofmt -l`, `go vet .` and `go test -count=1 .` pass. Two runs into temporary directories are
  byte-identical and equal `tests/golden`, both before and after the fixes below.
- **Self-check.** Negative tests ran in a scratch copy of the whole module, deleted afterwards. Each of these fails
  the run with a `self-check:` message:
  - a drifted `rttClass`, `pickBatch`, `(*tracker).observer` or `getEstMax`;
  - a drifted `handleErr` statement;
  - a drifted `Push` format literal;
  - a drifted `WatchRefs` timing expression.
- **Values against the Go code.**
  - Backoff is `5s << min(f-1, 4)`, capped at 60 s. The watch delays double from 1 s up to 30 s.
  - `est_size` wraps: a length field of 2^63 gives a negative value, and 2^64-1 gives 45 (`int(uint64)` is -1).
  - Every `pick_batch` step has a unique pick. The 8 MiB boundary uses `<=`.
  - `Placed` returns true over a view without nodes.
  - A 31-byte member id gives `client: ticket names no nodes`. With several members, the last error wins.
  - `RefList` does not map `unknown-ref`.
  - `mem: <shortid> not bound` and the path {direct, 1 ms} match `transport/mem.go:91,210` and
    `crates/transport/src/{error,mem}.rs`.
- **verify_record cases against core-rs.**
  - Like Go's `DecodePayload`, `decode_payload` ignores flag bits other than zstd and the raw `ulen`. A
    `verify_record` built as validate, `decode_payload`, key compare therefore reproduces every case, including the
    `parses: false` ones.
  - `Error::Corrupt` renders `amberpack: corrupt pack data: {0}`. So `decompressed to 4096 bytes, header says 4097`
    is portable, and the over-long frame is correctly `error_portable: false` (DD-12).
  - `Key` is `Key(pub [u8; 32])`, so tests can build the reserved-bit keys.
- **Determinism across architectures.** Every `rate` case has |bytes/took| < 2^63 (at most 4.6e18), so the
  float-to-int64 conversion is exact on arm64 and amd64. Neither `HumanBytes` nor `Rate` does a multiply-add that
  could be fused. A Linux regeneration in CI should give the same bytes.
- **Texts this family leaves out.** They exist elsewhere: `errors/worktree_text.json` has `ErrRefChanged` and its
  `(%v)` wrapping, and `wire/frame_errors.json` has the `ReadMsg` texts.
- **Hygiene.** The generator's temporary packstores are removed. No `vectorgen-*` directory was left in `$TMPDIR`.

### Fixed

1. **`client/placement_decisions.json`.** The two main scenarios now have 64 keys, as client-transfer §5.2 item 8
   asks. They had 24, and the file grows from 240 KB to 442 KB. That deviation is withdrawn.
2. **`client/rank.json` `rtt_class`.** Added the 4.999 ms boundary of client-core §5 item 7. Only 5 ms − 1 ns was
   there.
3. **`refglob/refglob.json`.**
   - Validation-order cases:
     - 4096 bytes plus `ff` gives the length text;
     - a control character plus 4096 bytes gives the length text;
     - `01 ff` gives the UTF-8 text;
     - a control character inside a class, after a backslash, or after a bad range gives the control-character text.
   - Newline names:
     - `**/x` matches `a\n/x`;
     - `a/**` does not match `a/\n`.
   - Patterns with U+0085 and U+2028, which compile. Go refuses only runes below 0x20 and 0x7f; a Rust port using
     `char::is_control` would wrongly refuse U+0085.
   - The file now has 144 match, 59 prefix and 32 invalid cases.
4. **`family_client.go`.**
   - Added a compile-time assertion that the `clientRecordSizer` alias keeps `client.RecordSizer`'s signature. The
     AST check does not compare it, and the copies of `batches` and `countKeys` take the alias.
   - Removed the unused `clientCountKeysCase.Sizes` field.
5. **`family_refglob.go`.** The U+0085 and U+2028 runes are built with `string(rune(…))`, so the source holds no
   invisible characters.
6. **`docs/vectorgen-client.md`.**
   - Added the report node schema, which was missing (`id`, `direct`, `rtt_ns`, `in_flight`, `awaiting`, `bytes`).
   - Stated the `pick_batch` seed rule per case.
   - Added the placement key counts and the `rtt_class` boundary list.
   - Documented the refglob validation order and C1 controls.

### Left open (other owners)

- **`errors/text.json` is named but no longer written.** PORTING.md §7, VECTORS.md "Families", and the doc
  comments of `tests/golden_tests/client.rs` and `client_transfer.rs` still name it. The client texts are in
  `errors/client_text.json` and the worktree texts in `errors/worktree_text.json`.
- **`est_size -> usize`** (PORTING §4.8) cannot hold Go's negative estimates (finding 1 stands).
- **Working directory.** `clientModuleDir` runs `go list -m` in the current directory, so the generator must run
  from `tools/vectorgen`, as `go -C tools/vectorgen run .` does.
- **Large JSON numbers.** `Incomplete.shortfall` 2^40 is a JSON number. It is below 2^53, so serde_json reads it
  exactly.
- **No Rust tests yet.** The tests that read these files do not exist yet: client-a, client-b, and the testkit owner
  for refglob.
