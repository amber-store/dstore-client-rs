# impl-gocompat-b: `strconv`, `time`, `fmt` of `dstore-gocompat`

Owner: gocompat-b (layer L1). Files: `crates/gocompat/src/{strconv,time,fmt}.rs`,
`tools/vectorgen/family_gocompat_numtime.go` (family `gocompat-numtime`, `tests/golden/gocompat/numtime.json`),
`tools/vectorgen/docs/gocompat-b.md`, `tests/golden_tests/gocompat_numtime.rs`. No manifest, lock file or
module list was changed, and every PORTING.md §4.1 signature is kept.

## Decisions

1. **`strconv` is a line-by-line port of go1.26.5 `internal/strconv`.**
   - Parsing: `readFloat`, `atof64exact`, Eisel-Lemire, the 800-digit `decimal` slow path, and `atofHex`.
   - Formatting: Dragonbox (`ftoadbox.go`) and `formatDigits` for `'g'`.
   - The `pow10Tab` table (696 entries) is transcribed mechanically from `pow10tab.go`.
   - Rust's float formatting and `f64::from_str` were rejected:
     - A measured 1047 of 202k probes differ: Go's Dragonbox breaks exact ties between two shortest
       candidates to even, while Rust rounds up (e.g. `1125899906842624.25` prints `…242e+15` in Go).
     - Go's parser keeps two quirks that `from_str` does not reproduce: exponent accumulation stops once it
       reaches 10000, and the slow path takes the decimal point from at most 800 stored digits, so a
       905-digit integer with `e-904` parses to `1.0000000000000002e-105`. Both are pinned by vectors.
2. **Invalid base or bit size panics** (`strconv: invalid base N` / `invalid bit size N`). Go returns a
   `NumError` whose `Err` reads `invalid base N` / `invalid bit size N`, which neither `NumErrorKind` nor
   `text() -> &'static str` can represent. Every dstore call site passes a constant base and bit size.
   Documented under `# Panics`.
3. **`errors_join` keeps empty messages.** PORTING.md §4.1's comment says "between non-empty messages". Go's
   `errors.Join` drops only nil errors, and an error with empty text still adds its line
   (`errors.Join(errors.New(""), errors.New("b")).Error() == "\nb"`). The Go behaviour is implemented (Go is
   normative), and callers pass only the texts of non-nil errors.
4. **`parse_rfc3339nano` ports Go's general `parse`**, walking the RFC3339Nano layout chunk by chunk with Go's
   error constructors and the time package's own `quote`. Go's `parseRFC3339` fast path is not ported: it
   accepts a subset of these inputs (4-digit fields, `.` fraction, offset hour ≤ 23) with the same instants,
   so it cannot change a result or an error.
5. **`SystemZone`** asks libc (`localtime_r` + `tm_gmtoff`), except where Go's `initLocal` falls back to UTC:
   - `TZ` set but empty, `:`, `UTC` or `:UTC`;
   - an absolute path that is not a TZif file;
   - a name with no TZif file under Go's standard `platformZoneSources` (`/usr/share/zoneinfo/`,
     `/usr/share/lib/zoneinfo/`, `/usr/lib/locale/TZ/`, `/etc/zoneinfo`).
   The decision is cached once per process, like Go's `localLoc`. Go's embedded-tzdata and
   `$GOROOT/lib/time/zoneinfo.zip` sources are not consulted; they depend on how the Go binary was built.
6. **`format_slog_time`** follows slog's `appendRFC3339Millis`: truncate to ms, add 100µs, format
   RFC3339Nano, remove the byte at offset 23. Its odd results for years outside 0..9999 are pinned by vectors.
7. **`format_log_std`** ports `log.itoa` including its byte arithmetic for negative years, without the
   trailing space that `formatHeader` appends.
8. **`GoTime::add_ns`** saturates internal seconds (since year 1) as `Time.addSec` does. Formatting uses
   wrapping arithmetic like Go's `absSeconds`, so extreme instants (`math.MaxInt64` seconds) format as Go
   does.

## Verification

- **Scratch harness.** While sibling files were mid-edit (`json.rs` lexer error, `family_wire.go` undefined
  symbols), the in-repo build could not compile. My modules were compiled alone in a scratch workspace: the
  three files via `#[path]`, a `quote` stand-in, `dstore-testkit` `golden.rs`/`splitmix.rs` via `#[path]`,
  and `tests/golden_tests/gocompat_numtime.rs` via `#[path]`. The vectors were generated from a scratch copy
  of `go.mod`, `go.sum`, `main.go`, `util.go` and my family file.
- **Golden vectors.** All 15 golden tests pass, including the error texts, and generation is deterministic
  (two runs, identical bytes).
- **In-repo runs**, once the sibling files compiled again:
  - `cargo test -p dstore-gocompat --lib -- strconv:: time:: fmt::`: 31 passed.
  - `cargo test -p dstore-client-rs --test golden gocompat_numtime`: 15 passed, with the real `quote`.
  - `cargo clippy -p dstore-gocompat --all-targets -- -D warnings`: nothing reported in my files.
  - `rustfmt --check --edition 2024` over my files: clean.
- **In-repo generation:** `go run . <tmp> gocompat-numtime` from `tools/vectorgen`, once the other
  families compiled, gives bytes identical to the committed `tests/golden/gocompat/numtime.json`.
- **Bulk cross-check (not committed).** About 1.2M cases from a scratch Go program with 0 mismatches:
  FormatFloat, ParseFloat, ParseInt/ParseUint in bases 0/2/8/10/16/36 and bit sizes 1..64, Duration
  String/Round, ParseDuration with mutations, RFC3339/slog/clock/RFC3339Nano over wide instant and offset
  ranges, and Parse(RFC3339Nano) with mutations.

## For other owners

- `NumError` `Display` uses `crate::quote::quote` (gocompat-a).
- `dstore_gocompat::time` has no `Duration`-typed API: slog `Duration` attributes pass `i64` nanoseconds to
  `duration_string`, and `std::time::Duration` goes through `duration_to_ns`.
- gocli's flag parsing should call `parse_int(s, 0, 64)` (Go `IntSize` is 64 on supported targets),
  `parse_uint(s, 0, 64)`, `parse_float` and `parse_bool`, then map `NumErrorKind` to the `flag` package texts
  (`parse error` / `value out of range`).

## Review

Reviewer: review-gocompat-b. Checked against the go1.26.5 sources:
- `internal/strconv`: `atoi.go`, `atob.go`, `atof.go`, `atofeisel.go`, `decimal.go`, `ftoa.go`, `ftoadbox.go`,
  `math.go`; plus `strconv/number.go`;
- `time`: `time.go`, `format.go`, `format_rfc3339.go`, `zoneinfo_unix.go`, `zoneinfo_read.go`;
- `log/slog/handler.go` (`appendRFC3339Millis`) and `errors/join.go`.

1. **Line by line, no defect found:**
   - ParseUint/ParseInt validation order: empty, then base, then bit size; a range error wins over a later
     syntax error. Also `underscoreOK`.
   - `readFloat`, `special`, `atof64exact`, Eisel-Lemire, the 800-digit `decimal` slow path and `atofHex`.
   - Dragonbox and `formatDigits` `'g'`.
   - `Duration.String`/`Round` and `ParseDuration`, including the wrapping `d += v`.
   - The civil date and clock arithmetic, `appendInt`, `appendNano` and `appendFormatRFC3339`.
   - The general `parse` walk of the RFC3339Nano layout:
     - chunk texts, and `hold` rather than the advanced value;
     - range-error precedence in the zone offset;
     - `extra text`, the day check, and `addSec` saturation.
   - `time.quote`; slog's truncate, add 100µs, drop byte 23; `log.itoa`.
   - `initLocal`'s UTC fallbacks, and `errors.Join`: `joinError.Error` keeps a line for an empty text.
2. **Independent differential probe** (scratch, deleted afterwards). A Go program and a Rust binary linked
   against the real `dstore-gocompat` compared 3.42M cases:
   - ParseBool: 20k.
   - ParseInt and ParseUint: 400k each, over bases 0 and 2..36 and bit sizes 0..64, with prefixes, signs,
     underscores and mutations.
   - ParseFloat: 400k, including exact float64 midpoints (some perturbed), 700-950-digit mantissas, hex floats
     and specials.
   - FormatFloat `'g'` -1: 600k, including subnormals, ties and powers of ten ±1 ulp.
   - `Duration.String`, `Round` and `ParseDuration`: 300k each.
   - RFC3339, RFC3339Nano, slog, clock and log layouts: 300k over the full i64 seconds range with i32 offsets.
   - `Parse(RFC3339Nano)`: 400k.

   The only mismatch is finding 3.
3. **`format_log_std` for years at or below −49000 (documented).**
   - `log.itoa`'s byte arithmetic writes a first byte that is not valid UTF-8: Go gives
     `b"\x90'*+/10/12 15:35:41"` for year −93088965.
   - The fixed `-> String` signature cannot hold that byte, so the result has U+FFFD there.
   - This is never observed: `slog.Default()` logs the current time. It is now stated on the function and pinned
     in `log_itoa_matches_go_byte_arithmetic`.
   - The committed vectors never reach such years: no output section contains U+FFFD.
4. **Generator hardening (fixed).** `json.Marshal` turns invalid UTF-8 into U+FFFD, which would hide finding
   3 if a vector ever reached it. `ntUTF8` now also checks error texts and layout outputs and panics. The
   generated bytes are unchanged.
5. **Docs error (fixed).** `tools/vectorgen/docs/gocompat-b.md` said `parse_int` covers bases "plus 2..36 via
   the Go test inputs". The generator only uses 0, 10 and 16. Corrected.
6. **`SystemZone` against Go `time.Local` (verified on macOS).** Offsets are identical for 26 `TZ` values × 9
   instants from 1653 to 2096. The values:
   - unset, `""`, `:`, `UTC`, `:UTC`, `Etc/UTC`, `UTC0`, `GMT`, `localtime`;
   - `Asia/Kolkata`, `:Asia/Kolkata`, `::Asia/Kolkata`, `America/St_Johns`, `Europe/Berlin`, `Asia/Kathmandu`,
     `America/Sao_Paulo`, `Pacific/Chatham`;
   - absolute paths, `:/usr/share/zoneinfo/Australia/Lord_Howe`, `/etc/localtime`, a directory, `/nonexistent`;
   - `XYZ-3`, `EST5EDT`, `Etc/GMT+5`, `Nowhere/Zone`.

   The nixpkgs Go 1.26.5 patches `platformZoneSources` to put the nixpkgs tzdata first; standard toolchains do
   not. dstore does not import `time/tzdata`.
7. **Deviations reviewed and accepted:**
   - panics on an invalid base or bit size: every urfave flag parser and dstore call site passes constants, so
     no data reaches it;
   - `errors_join` keeps empty texts, as `joinError.Error` does;
   - no `parseRFC3339` fast path: it accepts a subset of what `parse` accepts, with the same instants, and
     `time.Parse` falls back to `parse` for everything else.
8. **Coverage.**
   - Golden tests assert the value and the full error text of every section; nothing is ignored.
   - The lib tables are subsets of Go's; the complete `atoftests`, `atoi_test.go` inputs and
     `parseDurationTests` are in the vectors.
   - Go test inputs that are not valid UTF-8 (`"\x85\x85"`, `"\xffff"`) cannot reach the `&str` API.

Gates run by the reviewer (flake dev shell, own target directory):
- `cargo test -p dstore-gocompat --lib -- strconv:: time:: fmt::`: 31 passed.
- `cargo test -p dstore-client-rs --test golden gocompat_numtime`: 15 passed.
- `cargo clippy -p dstore-gocompat --all-targets -- -D warnings`: clean.
  - It is also clean for these files with `--force-warn dead_code --force-warn unused_variables`, which
    overrides the crate's L0 allow.
  - The remaining force-warn hits are in `ctx.rs`, `os.rs` and `tables.rs`.
- `rustfmt --check --edition 2024` on the four Rust files: clean.
- `go run . <tmp> gocompat-numtime` twice from `tools/vectorgen`: both runs are identical to each other and to
  the committed file.
- `go vet ./...` and `gofmt -l`: clean.
