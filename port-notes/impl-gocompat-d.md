# impl-gocompat-d: `gocompat::ctx`, `gocompat::slog`, family `gocompat-slog`

Owner gocompat-d (layer L1). Files:

- `crates/gocompat/src/ctx.rs` and `crates/gocompat/src/slog.rs`, each with unit tests.
- `tools/vectorgen/family_gocompat_slog.go`, which writes `tests/golden/gocompat/slog.json`, documented in
  `tools/vectorgen/docs/gocompat-d.md`.
- `tests/golden_tests/gocompat_slog.rs`.

## Items added to PORTING.md §4.1 (no signature changed)

- `pub fn slog::format_default_record(handler_attrs: &[Attr], r: &Record, now: GoTime, zone: &dyn Zone) -> Vec<u8>`
  - Returns the exact line `slog.Default()` writes: the `log.LstdFlags` header at `now`, the level, the raw
    message, the attributes, and "\n" unless the text already ends with one.
  - `Logger::default_logger()` calls it with `GoTime::now()` and `SystemZone`.
  - It exists so the default logger's layout can be tested with fixed times. §4.1 has no function that
    exposes that line.
- `#[derive(Clone, Debug, PartialEq)]` on `slog::Record`, for handlers that forward records and for tests.
- `impl Debug for slog::Logger`, which prints the handler attributes, and `impl Debug for ctx::Ctx`, which
  prints the deadline and err. Structs that hold them can then derive `Debug`.
- The private fields of `Ctx` are replaced (the scaffold's were guesses): `Ctx { node: Arc<Node> }`.

## Decisions

**`Ctx`** (go1.26.5 `context/context.go`):

- **Cancellation.** It travels down a `tokio_util` `CancellationToken` tree. `cancel()` records its instant
  in a `OnceLock` before cancelling the token, so an observer that sees the token cancelled always finds
  the instant.
- **Deadlines.**
  - Each `Ctx` stores its effective deadline, `min(parent, now + d)`, and it is checked lazily against
    `tokio::time::Instant::now()`.
  - No timer task is spawned, so a `Ctx` can be created and inspected outside a runtime.
  - `done()` selects on the token and on `sleep_until(deadline)`.
  - A `with_timeout` beyond the clock's range keeps the parent's deadline.
- **`err()`.**
  - It is the earliest event among own cancel, own deadline and the parent's event, latched per node the
    first time it is observed. Parent and child then agree even when a cancel races a deadline.
  - At the same instant a deadline beats a cancel: the ctx is done at its deadline.
  - A child created from an ended parent latches the parent's event at creation, which is Go's
    `propagateCancel` and wins over the child's own zero timeout.
  - A zero timeout ends the child at creation with `DeadlineExceeded`.
- **`cancel()`** works on a background `Ctx` as well (the shape of `signal.NotifyContext(context.Background())`).
  It affects only that ctx, its clones and its descendants.
- **`run(f)`** on an ended ctx returns the error without polling `f`. Otherwise it uses an unbiased
  `select!` of `done()` and `f`. Go's `select` between a closed `Done` and a ready result is random, so
  either outcome is compatible.

**`slog`:**

- **`log_level(s)`** compares ignoring ASCII case, which equals Go's `strings.ToLower(s) == word`. A Go probe
  over every rune showed that only U+0130 (to `i`) and U+212A (to `k`) lower-case to ASCII, and neither
  letter occurs in `debug`, `warn` or `error`. The vectors include both runes.
- **`needs_quoting`** is Go's rule with `safeSet` (json_handler.go): ASCII space, `=`, `"` and controls
  force quoting; `\` and DEL do not. Non-ASCII runes force quoting when they are U+FFFD, `unicode.IsSpace`
  or not `unicode.IsPrint` (`gocompat::quote`).
- **`TextHandler::handle`** formats first, then does one `write_all` plus `flush` under the lock. A poisoned
  lock is recovered, and write errors are ignored (`Logger` discards handler errors in Go).
- **The default handler.**
  - Enabled from INFO: `logLoggerLevel`'s zero value. `slog.SetDefault` and `SetLogLoggerLevel` are not
    modelled.
  - It writes each line with one `write_all` to `std::io::stderr().lock()`. A new handler per
    `default_logger()` call is equivalent to Go's shared one, because stderr serialises writes.
- **Time attributes** are formatted in the handler's zone. `GoTime` carries no location, and dstore logs no
  time attributes.
- **No attribute is elided.** Go elides only `Attr{}` with a nil `any`, which a rendered `Value` cannot
  represent.
- **Invalid UTF-8** reaches a handler only through `Value::Bytes`, because keys, messages and strings are
  `String` (DD-8).

## Tests

- **No test is ignored.** `gocompat::time` (gocompat-b) landed during this task, and the 8 tests marked
  `#[ignore = "needs gocompat::time"]` were un-ignored:
  - slog unit tests: `go_text_handler_cases`, `go_text_handler_preformatted`,
    `text_record_values_and_zones`, `logger_with_and_levels`, `default_record_layout`.
  - golden tests: `text_handler_records`, `logger_with_matches_go_with`, `default_logger_lines`.
- **Real repo.**
  - `cargo test -p dstore-gocompat --lib -- ctx:: slog::` runs 32 tests: all pass, none ignored.
  - `cargo test --test golden -- gocompat_slog` runs 10 tests: all pass, none ignored.
  - rustfmt (edition 2024) is clean for the three Rust files.
  - `cargo clippy -p dstore-gocompat --all-targets -- -D warnings` currently stops on sibling lints in
    `strconv.rs` and `time.rs`. None of the reported lints are in `ctx.rs` or `slog.rs`.
- **Scratch harness** (not committed, deleted afterwards).
  - It compiled the real `ctx.rs`, `slog.rs`, `quote.rs`, `strings.rs`, `strconv.rs`, `tables.rs` and
    `time.rs` together with the root test `tests/golden_tests/gocompat_slog.rs` reading `tests/golden`, while
    the real crate did not compile (sibling `json.rs` mid-edit).
  - 32 unit tests and 10 golden tests pass: 82 records, 17 default-logger lines, 182 `needs_quoting`
    cases, levels, `log_level` and the paused-clock `ctx` tests.
  - `clippy --all-targets -- -D warnings` is clean for `ctx.rs`, `slog.rs` and `gocompat_slog.rs`, with the
    sibling modules' lints allowed in the harness only.
  - Before `gocompat::time` landed, the harness also passed with a stand-in time formatter.
- **Paused clock.** tokio `test-util` is not enabled for the gocompat crate's own tests, and its manifest is
  not mine. So the paused-clock `ctx` tests live in `tests/golden_tests/gocompat_slog.rs`, where the root
  dev-dependency enables it. The unit tests use real time with 30 ms deadlines.

## Vectors

- **Family placement.**
  - PORTING.md §7 lists "slog lines" under `text/formats.json`. This task put the slog family at
    `gocompat/slog.json`, which owns no other path.
  - The doc comment of `tests/golden_tests/gocompat_slog.rs` names it.
  - Nothing conflicts if the `text/formats.json` owner also emits slog lines.
- **Self-checks.** The generator compares its copy of cmd/dstore `logLevel` with the dstore v0.1.9 module
  source (`go list -m`, `go/printer`). It also checks the `LstdFlags` header composition against the real
  log package at the current time.
- **Generation.** The vectors were generated from a scratch copy of the vectorgen module (go.mod, go.sum,
  `main.go`, `util.go` and this family). At the time, sibling families (`family_wire.go`) did not compile. A
  second run was byte-identical.
- **Escapes.** Special characters in the generator's string literals are Go escapes (`\u00a0`, `\ufeff`, …).
  A literal U+FEFF in Go source is a compile error.

## Review

Reviewer review-gocompat-d. It re-read PORTING.md §0-3, §4.1 and §5-7, client-core §2.13, §3.5-3.6, §4.3 and
the Addenda, and cli.md §2.3, §4.3, §4.6 and the Addenda. It then compared the port line by line with the
go1.26.5 sources:

- `log/slog`: `text_handler.go` (`needsQuoting`, `appendTextValue`); `handler.go` (`commonHandler.handle`,
  `withAttrs`, `appendNonBuiltIns`, `appendKey`, `appendString`, `appendRFC3339Millis`, `defaultHandler`);
  `level.go`; `logger.go` (`log`, `With`); `record.go` (`argsToAttr`); `value.go` (`append`); `attr.go`
  (`isEmpty`).
- `log/log.go` (`output`, `formatHeader`).
- `context/context.go` (`WithDeadlineCause`, `propagateCancel`).
- cmd/dstore `logLevel`.

### Findings

No correctness defect was found in `slog.rs`, `ctx.rs`, the golden tests or the generator.

- **`needs_quoting`, every rune.** A Go probe observed `needsQuoting("a"+r+"b")` through `TextHandler` for
  every rune. A scratch crate compared the result with the Rust function: 1,112,064 runes, 0 mismatches.
  - A second probe showed `unicode.IsPrint`, which slog calls, equals `strconv.IsPrint`, which
    `quote::is_print` ports, for every rune.
  - go1.26.5's `strconv` `TestIsPrint` keeps them equal.
  - `text/quote.json` already checks `is_print` and `is_space` for every rune, so exhaustive ranges were
    not added to `gocompat/slog.json`.
- **`log_level`.** The probe confirmed that only U+0130 and U+212A lower-case to ASCII.
- **Verified spec lines.** 19 of the 20 lines of client-core §3.5 and cli.md §2.3 equal the vector lines.
  - The exception is client-core §3.5's `quoting` line. It shows U+00A0 (NBSP) and U+200B (ZWSP) as literal
    characters, but Go's `strconv.Quote` writes both as backslash-u escapes, because neither rune is
    printable.
  - The vector, which is Go's output, is right, and Rust matches it. The spec is not this owner's to edit.
- **cli.md §2.3 on Duration.** It says Duration values follow "then quoting rule". Go writes them raw
  (`Value.append`), and so does Rust.
- **Accepted differences.**
  - `Attr{}` elision is not modelled, and time attributes use the handler's zone. Both are documented
    above.
  - `with_timeout` beyond the clock's range gives `deadline() == None`, where Go keeps a far deadline. The
    dstore v0.1.9 client never calls `ctx.Deadline()`: `WithDeadline` and `WithoutCancel` appear only in
    node and paxos code.
- **Fixed: timing-dependent unit tests.** `ctx::tests::deadline_ends_the_ctx` and
  `cancel_before_the_deadline_stays_canceled` asserted outcomes that hold only if the test thread does
  not stall for 30 ms between two statements, which can happen on a loaded CI runner.
  - They now assert the exact cause only when `start.elapsed() < SHORT` proves the event order. Otherwise
    they assert only that the ctx ended.
  - Test time is unchanged. The exact paths stay covered deterministically on the paused clock in
    `tests/golden_tests/gocompat_slog.rs`.

### Gates

- **Vectors.**
  - gofmt is clean. `go vet ./...` is clean over the whole vectorgen module, whose sibling families compile
    now.
  - `gocompat-slog` was regenerated twice through the real registry
    (`go -C tools/vectorgen run . <tmp> gocompat-slog`). Both runs are byte-identical and equal the
    committed `tests/golden/gocompat/slog.json`: 47 levels, 32 log levels, 182 `needs_quoting` cases, 82
    records and 17 default-logger lines.
  - This resolves the earlier remaining item that the registry run had not been exercised.
  - `go.mod` and `go.sum` are unchanged.
- **JSON conventions.**
  - The JSON is built from structs only. `unix_secs` and `num` are decimal strings, and small integers are
    numbers.
  - Invalid UTF-8 appears only in `in_hex` and `hex`.
  - `docs/gocompat-d.md` documents every field.
- **Tests.**
  - `cargo test -p dstore-gocompat --lib -- ctx:: slog::`: 32 passed. `cargo test --test golden --
    gocompat_slog`: 10 passed. No test is ignored.
  - 24 concurrent runs of the ctx unit tests passed both before and after the fix.
- **rustfmt** (edition 2024) is clean for the three Rust files.
- **Clippy.**
  - `cargo clippy -p dstore-gocompat --all-targets -- -D warnings` is still blocked by sibling lints:
    `manual_is_multiple_of`, `manual_range_contains` and a manual ASCII range check in `strconv.rs`, and
    `manual_range_contains` in `time.rs`.
  - In warn mode, no lint is reported in `ctx.rs` or `slog.rs`, nor in `tests/golden_tests/gocompat_slog.rs`
    (root `--test golden`).

### Remaining

- `text/formats.json` (PORTING.md §7) does not exist yet. If its owner adds slog lines, the module that
  reads them (`client.rs`, owner client-a) tests `format_text_record` too.
- Real-crate clippy with `-D warnings` waits on the sibling lints above.
