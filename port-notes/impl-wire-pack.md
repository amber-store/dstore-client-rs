# impl-wire-pack: layer L2 notes

Owner: wire-pack. Files: `crates/wire/src/pack.rs` (implementation and unit tests),
`tests/golden_tests/wire_pack.rs` (`wire/pack_frames.json`, `wire/pack_reader.json`).

## What is implemented

- **`read_protocol_msg`** (`protocol.ReadMsg`): the counting header loop, then the `MAX_FRAME` check before
  the payload is allocated, the short-payload causes and `protocol.Msg` decoding. It mirrors the wire
  owner's `read_msg`, including one choice: an underlying io error of kind `UnexpectedEof` in the header
  maps to `UnexpectedEof` (Go `errors.Is(err, io.ErrUnexpectedEOF)`). `ProtocolMsg` and its sub-structs
  are the L0 declarations, unchanged.
- **`PackSender`** (`SendPackRecords`): `amberpack.Writer` (4096-byte `bufio.Writer`) over `chunkWriter`.
  - It writes TData frames of exactly 1 MiB carrying only keys 0 and 8, then the remainder and
    `00000003a10008`.
  - `add_record` writes the magic before the first record. `finish` writes the magic if none was
    written, `00`, flushes the buffer, writes the remainder and TDataEnd, then calls `flush()` on the
    writer (a no-op on noq streams).
  - Dropping the sender without `finish` writes nothing more.
- **`PackReader`** (`packReader`): TData gives bytes (an empty Data reads on), TDataEnd gives EOF, TErr
  gives `Remote`, any other type gives `Unexpected`. Errors are sticky, and a clean end at a frame boundary
  is a sticky EOF (`Ok(0)`). `drain` is `io.Copy(io.Discard, pr)`.
- **`PackRecords`** (`amberpack.Reader.Records`): magic, tag, the 45 header bytes, the `slen ≤ MAX_PAYLOAD`
  check, the payload, then `amberpack::parse_record`. Exactly one error, then `None`. It reads straight
  from the `PackReader`'s chunk, so `drain()` consumes TDataEnd.

Nothing in PORTING.md §4.3 was changed or added. Only doc comments of `PackRecordsError` were extended.

## Conflict with PORTING.md §4.3 (Go wins, as the L1 note said)

`PackRecordsError::Read(#[source] PackReadError)` is documented as displaying the read error alone. Go's
`Records` wraps every pack-reader error with `%v` as `amberpack: malformed pack stream: <step>: <err>`.
The steps are `reading magic`, `truncated before end marker`, `truncated record header` and
`truncated record payload`. Examples from `wire/pack_reader.json`:
`… truncated record payload: remote: internal: sender died` and `… reading magic: protocol: short frame:
unexpected EOF`.

- These errors are produced as `PackRecordsError::Stream(format!("{step}: {err}"))`, and `Read` is never
  produced. Its doc comment now says so; no signature changed. As with Go's `%v`, the remote error can no
  longer be reached (`source()` is `None`, and Go's `errors.As(err, *RemoteError)` fails).
- `parse_record` failures are `PackRecordsError::Record(amberpack::Error::Malformed(e.to_string()))`. A
  `Record(Corrupt)` would display `amberpack: corrupt pack data: …` without Go's
  `amberpack: malformed pack stream: ` prefix. The `Malformed` wrapper renders Go's
  `fmt.Errorf("%w: %v", ErrMalformed, err)` exactly, and `is_malformed()` holds, as `errors.Is(err,
  ErrMalformed)` does in Go.
- `bad record tag` uses Go's `%#x`, `0x2`, not the `0x02` of the §4.3 comment.

## Findings and choices beyond the spec sketches

1. **bufio window when a sender is dropped.** Go's `amberpack.Writer` buffers up to 4096 bytes above the
   chunk writer, so a 1 MiB boundary inside that buffered tail is not crossed until bufio flushes.
   `PackSender` emulates `bufio.Writer.Write`/`WriteByte`/`Flush` exactly: the large-write bypass when the
   buffer is empty, and filling the buffer before flushing. With this, the frames already written when a
   sender is dropped are Go's, inside the window too. The VECTORS.md cases avoid that window, so a scratch
   Go program over transport-iroh v0.4.0 (deleted afterwards) measured four cases that hit it. They are
   pinned in the unit test `bufio_window_matches_go`:
   - `[C-108, 200]` gives no frame;
   - `[C-108, 5000]` gives 1;
   - `[C-3008, 1000×4]` gives none;
   - `[4088, C-4146, 100]` gives none.

   A sender without the buffer would have written a frame in the first case. The frames of a finished
   pack do not depend on the buffer. Private fields `bw` and `err` were added; they are not public.
2. **Sticky write errors.** After a write error, `add_record` and `finish` return that error again, as
   bufio's `b.err` does (a copy: `io::Error` is not `Clone`).
3. **Sticky read errors are copies** (`raw_os_error`, else kind + message) and render the same Go text
   through `errno::io_error_text`. The first return is the original error.
4. **`read(&mut [])` waits for data.** Go's `Read(nil)` keeps reading frames until a chunk has bytes,
   verified with the same scratch program. `read` does the same, then returns `Ok(0)`.
5. **No `BufReader` below `PackRecords`.** Go's `Records` reads through a 4096-byte `bufio.Reader`, but
   `packReader.Read` never returns bytes of more than one frame. Both read the same frames and yield the
   same records and errors, and the stream ends at the same position after `drain`. Only `drain()`'s count
   can differ: bytes after the end marker that Go's bufio swallowed are counted here. dstore never uses
   the count.
6. **Memory.** A record payload buffer grows with the bytes received (initial capacity at most 1 MiB).
   Go allocates `46 + slen` up front, up to 256 MiB. Frame payloads are allocated after the `MAX_FRAME`
   check, as in Go.
7. **Not cancel-safe:** `read_protocol_msg`, `PackReader::read`/`drain`, `PackRecords::next`/`drain`, and
   `PackSender::add_record`/`finish`. Callers wrap them the way PORTING §5.1 wraps `read_msg`.

## For other owners

- **Root manifest (not owned):** the golden tests compare BLAKE3 digests on their first 30 bytes, through
  core-rs `Key::new(Type::Blob, 0, data)`, which embeds `digest[..30]` after two header bytes. The root
  package has no `blake3` dev-dependency. Adding one would allow full 32-byte comparisons. The wire owner
  uses the same workaround.
- **client-b:** `getStream` ignores the `Records` error text and never matches it; `putOnce`'s source
  never errors (unreadable keys are skipped). Call `PackRecords::drain` after the records, as Go does.

## Tests

- **Unit tests** (`cargo test -p dstore-wire --lib pack::`, 31 tests, no sockets):
  - transport-iroh `protocol_test.go`: `TestMsgRoundTrip`, `TestReadMsgRejectsOversizeFrame`,
    `TestReadMsgTruncatedFrame`, `TestRemoteError`, `TestMsgDataEndpointsRoundTrip`,
    `TestMsgDataEndpointsCompat`, `TestMsgPinNamesRoundTrip`.
  - `pack_test.go`: `TestSendPackRecordsRoundTrip`, `TestSendPackRecordsPropagatesSourceError` (as "a sender
    dropped before a chunk fills writes nothing"), `TestPackReaderSurfacesRemoteError`,
    `TestPackReaderRejectsUnexpectedFrame`.
  - dstore `wire_test.go`: `TestPackFramesInterop`, using the wire owner's `read_msg`.
  - core `amberpack/pack_test.go`, over TData framing: `TestWriter_AddRecord_RoundTrip`,
    `TestReader_Records_RoundTrip`, `TestReader_RejectsLegacyVersions`, `TestWriterReader_EmptyStreamIsValid`,
    `TestReader_BadMagic`, `TestReader_TruncatedMissingEndMarker` + `TestReader_Records_Truncated`,
    `TestReader_NonCanonicalKeyRejected`, `TestReader_BadRecordTag`, `TestReader_TruncatedPayload`,
    `TestReader_RecordCRCMismatch` + `TestReader_Records_CRCMismatch`, `TestReader_OversizedPayloadRejected`.
  - Extras:
    - TData frames equal canonical `protocol.Msg` frames;
    - the bufio window cases and `Read(nil)`;
    - drain counts, EOF at a boundary, sticky io texts;
    - bytewise underlying reads, sticky write errors.
- **Golden tests** (`tests/golden_tests/wire_pack.rs`):
  - `pack_frames`, all 14 cases: record bytes and `records_blake3`, the `PackSender` stream (length, hex,
    BLAKE3), every frame (length, head, data, data BLAKE3), and the read back through `PackRecords` +
    `drain` + `read_msg`.
  - `pack_reader`, all 34 cases: `PackReader::read` with a 5-byte buffer (data, end, again), then
    `PackRecords` (records, error, one error then `None`), `drain_error`, and the next frame through
    `read_msg`.
  - `blake3_prefix_of_the_empty_input`.
  - The wire owner's `wire::` golden tests cover `read_protocol_msg` against `decode.json` `protocol_msg`
    and `frame_errors.json` `protocol_reads`.
- Nothing is `#[ignore]`d.

## Gates

- `cargo test -p dstore-wire`: 58 passed (27 of wire, 31 of wire-pack), 0 ignored.
- `cargo test -p dstore-client-rs --test golden -- wire_pack:: wire::`: 23 passed (3 `wire_pack::`, 20
  `wire::`), 0 ignored. The first two attempts could not build because sibling crates were mid-edit
  (dstore-transport `mem.rs`, then dstore-worktree `sys.rs`).
- `cargo clippy -p dstore-wire --all-targets -- -D warnings`: clean, run both before and after `lib.rs`
  dropped its `allow` on `mod pack`.
- `cargo rustc -p dstore-wire --lib -- --force-warn dead_code --force-warn unused_variables --force-warn
  unused_imports --force-warn unused_mut`: no warnings.
- `rustfmt --check --edition 2024` over both owned files: clean.
- **Clippy over `tests/golden_tests/wire_pack.rs`:** clean with `-D warnings`, run through a scratch package
  outside the repo (deleted afterwards), where the 3 tests also passed. The package was a shim
  `dstore-testkit` holding copies of `golden`/`splitmix`, with a path dependency on `crates/wire` and a
  `#[path]` include of the module. The root package's clippy was not run in the repo: sibling crates
  still carry warnings, e.g. dstore-transport `addr.rs` `PathSegment`.

## Review

Reviewer: review-wire-pack. I re-read PORTING §0-3, §4.3 and §5-7; codec-wire-ticket §2.2.5, §2.3, §4.4,
§5-8 and its addenda; client-transfer §3.2-3.3, §4.4, §5.2 and its addenda; core-rs-gaps §2.2, §2.10, §3.3
and §4.3; VECTORS.md family `wire-pack`; impl-wire.md; and impl-vectorgen-proto.md. I compared the code line
by line against:

- transport-iroh v0.4.0 `protocol/pack.go` and `protocol.go` (`ReadMsg`, `WriteMsg`, `chunkWriter`,
  `SendPackRecords`, `packReader.Read`);
- core v0.0.8 `amberpack/pack.go` (`Writer`, `Records`);
- the go1.26.5 `bufio.Writer` (`Write`, `WriteString`, `WriteByte`, `Flush`), `bufio.Reader` (`Read`,
  `ReadByte`, `fill`), `io.ReadFull` and `io.Copy(io.Discard, …)`, where `discard.ReadFrom` turns `io.EOF`
  into nil;
- the generator `tools/vectorgen/family_wire.go` (`genPackFrames`, `genPackReader`).

Nothing was ignored and nothing needed un-ignoring.

### Checked by differential runs against Go

Scratch Go programs over transport-iroh v0.4.0, core v0.0.8 and dstore v0.1.9 wrote the cases. Scratch Rust
packages replayed them through `dstore-wire`. All of it lives under the session scratchpad and was deleted
afterwards.

1. **Reader, 2,500 random cases.** The pack bodies were valid, truncated, byte-flipped, legacy-magic,
   missing the end marker, carrying a huge `slen`, or followed by junk after the end marker. Each body was
   split into random TData frames, including empty ones. The stream then ended in one of these ways:
   - TDataEnd, with or without a following TOK frame;
   - a clean EOF;
   - a dstore TErr at a random frame;
   - a cut within the last 12 bytes;
   - TWants, TPutResult or TRefs;
   - an oversize header;
   - a stamped TErr;
   - a TData carrying Epoch.

   Read sizes were 1, 2, 5, 7, 64 and 4096. The run compared the `PackReader::read` data, the end error and
   the next read after it; the `PackRecords` records (all bytes) and their error; the `drain` error; and the
   next `read_msg`. **0 differences.** This also confirms finding 5 above: there is no `BufReader`, and the
   frames read, the errors and the stream position still equal Go's `bufio.Reader` path.
2. **Sender, 248 rows.** These are the author's four rows, a random sweep of 64 rows, and a 180-row grid
   that puts the 1 MiB boundary 1-8192 bytes after a big record, with and without a small first record and
   with 1-3 tail records. Each row compares the frame lengths of the finished pack and of a source that
   fails after the last record. In 110 rows the boundary falls inside bufio's buffered tail. **0
   differences.** `bufio_window_matches_go` now pins 34 of these rows; it pinned 4 before.
3. **`Read(nil)`.** Go read the empty TData and TData "xy" and left TDataEnd unread (18 of 25 bytes), as
   finding 4 says.

### Fixed

1. **The `write_tdata` doc comment was wrong.** It said this code splits a frame into two writes as Go
   does. Go writes the length header, then the whole CBOR payload. This code writes the length together
   with the CBOR head, then the data. The bytes are the same and the split is not observable. The comment
   now says so.
2. **Too few measured sender rows.** Only four Go-measured rows were pinned; now there are 34 (item 2
   above).

### Confirmed, no change

- The PORTING §4.3 `PackRecordsError::Read` conflict. Go's `%v` text wins (`pack_reader.json`), so `Stream`
  is right and `Read` is never produced. `dstore-client` wraps `PackRecordsError` whole, and Go's
  `getStream` caller drops the error (`fetch.go:252`), so nothing needs to match the remote error inside.
- `parse_record` failures render as `amberpack: malformed pack stream: amberpack: corrupt pack data: …`;
  a bad tag renders as `0x2`, not `0x02`.
- `read_protocol_msg`. Header EOF maps as Go's `errors.Is(err, io.ErrUnexpectedEOF)` does; `MAX_FRAME` is
  checked before the payload is allocated; the short-frame causes and the decode-error prefix are Go's.
- `PackReader`. The sticky error is checked before `done`, as Go does; an empty TData reads on; a clean EOF
  at a frame boundary is a sticky EOF.
- `PackSender`. The magic goes into bufio before the first record. A large write into an empty buffer goes
  straight to the chunk writer. `WriteByte` flushes a full buffer first. A chunk flush clears the buffer
  even on error, and errors are sticky.
- Golden coverage. Every field of both vector files is asserted, except:
  - `source`, which is documentation;
  - `pack_frames.json` `go_error` (`boom`), the text of Go's failing source, which `PackSender` has no
    counterpart of.

  All of these are covered: the codec-wire-ticket G6 stream and reader cases, the client-transfer §5.2
  item 2 cases, and the Go tests named by codec-wire-ticket §6 and core-rs-gaps §6.

### Gates (review)

- **`cargo test -p dstore-wire`:** 62 passed, 0 ignored. This includes the extended
  `bufio_window_matches_go`.
- **`cargo clippy -p dstore-wire --all-targets -- -D warnings`:** clean.
- **`rustfmt --check --edition 2024` over both owned files:** clean.
- **Golden `wire_pack::` (3 tests: `pack_frames`, `pack_reader`, `blake3_prefix_of_the_empty_input`):**
  passed, and clippy with `-D warnings` over the module is clean. Both ran through a scratch package outside
  the repo (deleted afterwards). It had path dependencies on `crates/wire` and `crates/testkit`, whose
  `golden_dir()` resolves to this repo's `tests/golden`, and it included the module with `#[path]`.
- **The root run is still blocked.** `cargo test -p dstore-client-rs --test golden -- wire_pack::` could
  not build on three tries over about 20 minutes: dstore-client `src/error.rs` (`as_client` missing a
  lifetime, `chain`) was mid-edit by its owner.
- **Still open:** the BLAKE3 comparisons use 30 of 32 bytes, because the root manifest has no `blake3`
  dev-dependency. Linux byte identity is left to CI.
