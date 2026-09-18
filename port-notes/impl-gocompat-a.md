# impl-gocompat-a: layer L1 notes

Owner: gocompat-a. Files: `crates/gocompat/src/quote.rs`, `strings.rs`, `tables.rs` (generated),
`tools/vectorgen/cmd/gotables/main.go`, `tools/vectorgen/family_text.go` (families `quote` and `case`),
`tests/golden/text/quote.json`, `tests/golden/text/case.json`, `tests/golden_tests/gocompat_text.rs`,
`tools/vectorgen/docs/gocompat-a.md`.

## Public API

As in PORTING.md §4.1, unchanged: `quote::{quote, is_print, is_space}` and
`strings::{to_lower, to_upper, trim_space, fields_func}`.

## Crate-internal helpers (for other gocompat modules)

- `strings::decode_rune(&[u8]) -> (char, usize)`: `utf8.DecodeRuneInString`, with `(U+FFFD, 1)` for an
  invalid encoding and `(U+FFFD, 0)` for empty input. A valid U+FFFD comes back with width 3.
- `strings::decode_last_rune(&[u8]) -> (char, usize)`: `utf8.DecodeLastRuneInString`.
- `strings::runes(&[u8])`: Go's `for i, r := range s`, yielding `(offset, rune, width)` with
  `(U+FFFD, 1)` for each invalid byte.
- `strings::RUNE_ERROR`.
- `strings::unicode_to_lower(char)` and `unicode_to_upper(char)`: `unicode.ToLower` and `unicode.ToUpper`
  (simple mapping).

These are `pub(crate)`. `slog::needs_quoting` (`unicode.IsSpace`, `!unicode.IsPrint`, `RuneError`) can
build on `runes`, `is_space` and `is_print`. `strconv.IsPrint` and `unicode.IsPrint` agree on every rune in
go1.26.5, and `gotables` checks that.

**Note for `json` (gocompat-c).** encoding/json v1 folds names with `unicode.SimpleFold` (`foldRune`).
`tables.rs` holds only `ToLower`/`ToUpper`. In go1.26.5, for 83 runes the set `{r, ToLower(r), ToUpper(r)}`
is not the `SimpleFold` orbit of `r`. All 83 are keys of `unicode/tables.go` `caseOrbit`; examples:

- K, k, U+212A;
- S, s, U+017F;
- U+00B5, U+039C, U+03BC;
- U+0345, U+0399, U+03B9, U+1FBE;
- U+01C4 to U+01C6.

U+0130 and U+0131 fold only to themselves, even though `ToLower(U+0130)` is `i`. A complete `SimpleFold`
needs the `caseOrbit` table. U+0390 and U+1FD3 are not related in go1.26.5: `SimpleFold(U+0390)` is U+0390
(corrected in review; an earlier version of this note gave them as an example). How `json.rs` folds keys is
recorded in impl-gocompat-c.md.

## Generated table

- `tables.rs` is exactly the output of `go run ./cmd/gotables` (go1.26.5, Unicode 15.0.0), checked with
  `cmp`.
- The strconv tables are value for value those of go1.26.5's `$GOROOT/src/strconv/isprint.go`.
- `IS_GRAPHIC` carries `#[allow(dead_code)]`: `Quote` does not use `isGraphic`, but PORTING.md §4.1
  names the table. Only the unit tests read it.

## Spec discrepancy

view-placement.md §3.5 (row U+200B, `view: bad node id "<U+200B>"`) and verification.md §2.4 (`%q` probe list)
show a **literal** U+200B between the quotes. go1.26.5 `strconv.Quote("\U0000200b")` gives
`"\u200b"`: U+200B is category Cf, so it is not printable. `text/quote.json` case
`U+200B zero width space` asserts Go's output, and so do the unit tests.

## How this was verified

While this work landed, sibling files still in progress kept two builds from compiling for a while:
`tools/vectorgen/family_wire.go` and `family_client.go` broke the vectorgen package (still broken at hand-off),
and `crates/gocompat/src/json.rs` broke `dstore-gocompat` (fixed later). So:

- **Vectors.** `quote` and `case` were generated from a scratch copy of the module (`main.go`, `util.go`,
  `family_text.go`, `go.mod`, `go.sum`) into the real `tests/golden`. There, `gofmt -l` and `go vet` were
  clean, two runs gave identical bytes, and the output equals the committed files. Once the package
  compiles, rerun `go run . ../../tests/golden quote case` in the real module; the output must not
  change.
- **Tables.** `gotables` is its own package and runs in the real module. Its output equals the committed
  `tables.rs` (`cmp`).
- **Rust.** The tests ran in a scratch workspace:
  - its `dstore-gocompat` compiles only `quote.rs`, `strings.rs` and `tables.rs` (through `#[path]`,
    without the L0 `allow(dead_code)`);
  - it uses the real testkit `golden.rs` and the committed vectors.

  There the 19 unit tests and the 5 `gocompat_text` golden tests pass, and
  `cargo clippy --all-targets -- -D warnings` is clean. `rustfmt --check --edition 2024` is clean on all
  four files in the repo.
- **Real workspace.** Once `dstore-gocompat` compiled again, `cargo test -p dstore-gocompat --lib`
  (the 19 `quote`/`strings` tests) and `cargo test -p dstore-client-rs --test golden gocompat_text` (5 tests)
  passed. `cargo clippy -p dstore-gocompat --all-targets` and `cargo clippy --test golden` reported nothing
  on these files.
- **Differential run.** A throwaway Go program compared 200000 random byte strings with the Rust port:
  Go's `strconv.Quote`, `strings.ToLower`, `ToUpper`, `TrimSpace` and `FieldsFunc(unicode.IsSpace)`
  against `quote`, `to_lower`, `to_upper`, `trim_space` and `fields_func`. The inputs mixed arbitrary
  bytes, truncated runes, surrogate encodings and white space, and every output matched. Nothing from
  this run is committed.

## Review

By review-gocompat-a, an adversarial review of gocompat-a, owning the same files. It checked the code
against the go1.26.5 sources: `strconv/quote.go`, `isprint.go`, `strings/strings.go`, `unicode/letter.go`,
`graphic.go` and `unicode/utf8/utf8.go`.

### Line-by-line check

No defect found in `quote.rs`, `strings.rs`, `tables.rs`, `cmd/gotables` or `family_text.go`:

- **`quote`** follows `appendQuotedWith`: `\xNN` only for `(RuneError, 1)`, so a valid U+FFFD is kept.
  Then `appendEscapedRune` in order: the quote and backslash, `IsPrint`, the C escapes, `\x` below U+0020
  and for U+007F, `\u`, `\U`, with lower-case hex digits.
- **`is_print`**: the Latin-1 fast path, `bsearch` with the `i&^1` / `i|1` range test, the U+20000
  shortcut, and `isNotPrint32` offset by 0x10000.
- **`is_space`**: the Latin-1 switch, then White_Space.
- **`to_lower` / `to_upper`**: the ASCII fast path, then output equal to `strings.Map`. An invalid byte
  becomes `mapping(U+FFFD)`, which is U+FFFD; a valid U+FFFD is unchanged.
- **`trim_space`**: both ASCII fast-path exits, then `TrimFunc` / `TrimRightFunc` through `indexFunc` and
  `lastIndexFunc`, with the `lim` guard of `DecodeLastRuneInString`.
- **`fields_func`**: the span loop of `FieldsFunc`.
- **`decode_rune`**: the `first` and `acceptRanges` tables.

### Verified in this review

- **Tables.**
  - `tables.rs` is byte-identical to `go run ./cmd/gotables` in the real module (`cmp`).
  - The five strconv tables equal `$GOROOT/src/strconv/isprint.go` value for value: 424, 133, 508, 112
    and 16 values.
  - `WHITE_SPACE` has 25 runes, `TO_LOWER` 1433 entries and `TO_UPPER` 1450.
- **Real `tools/vectorgen` module.** The sibling compile errors are fixed.
  - `go vet ./...` is clean and `go test ./...` passes.
  - Two runs of `go run . <tmp> quote case` into separate temporary directories give identical output,
    equal to the committed `text/quote.json` and `text/case.json`.
  - `gofmt -l` is clean on `family_text.go` and `cmd/gotables`.
- **Vector files.**
  - Two-space indentation and a trailing newline.
  - No duplicate case names among the 944 and 365 cases.
  - The first, middle and last rune of all ten `IsPrint` lookup classes.
  - Every `to_lower` / `to_upper` run is well formed.
- **U+200B discrepancy confirmed.** `strconv.Quote` and `%q` give `"​"`, while view-placement.md §3.5
  (row U+200B) and verification.md §2.4 hold a literal U+200B.
- **Independent differential fuzz** (not committed; removed afterwards).
  - A PCG-seeded Go generator produced 300000 inputs, weighted towards truncated prefixes and stray
    suffixes of multi-byte runes; overlong, surrogate and above-U+10FFFF forms; every White_Space rune;
    every rune with a case mapping; and the lookalikes.
  - A scratch binary compiled these three files through `#[path]`.
  - `quote`, `to_lower`, `to_upper`, `trim_space` and `fields_func(_, is_space)` gave 0 mismatches.
- **Go tests.**
  - Ported completely: strconv `quotetests` (the `out` column); strings `upperTests`, `lowerTests`,
    `trimSpaceTests` (16), `fieldstests` (15, with the invalid-UTF-8 row checked separately) and
    `FieldsFuncTests`.
  - Covered by the every-rune golden tests: strconv `TestIsPrint` / `TestIsGraphic`, unicode `TestIsSpace`
    and `TestToUpperCase` / `TestToLowerCase`.
  - No test is ignored or skipped, and `load_json` panics when a file is missing.

### Fixed

- **`tests/golden_tests/gocompat_text.rs` `expand`.** A run whose `hi - lo` is not a multiple of `stride`
  now fails. Before, `step_by` silently dropped `hi` from the expansion.
- **The note for gocompat-c above.** The U+0390 / U+1FD3 example was wrong. Rewritten from a go1.26.5 probe
  that compared every rune's `SimpleFold` orbit with `{r, ToLower(r), ToUpper(r)}`.

### Gates run in this review

- `cargo test -p dstore-gocompat --lib -- quote:: strings::`: 19 passed.
- `cargo test -p dstore-client-rs --test golden gocompat_text`: 5 passed, run again after the fix.
- `rustfmt --check --edition 2024` is clean on `quote.rs`, `strings.rs`, `tables.rs` and
  `gocompat_text.rs`.
- `cargo clippy -p dstore-gocompat --all-targets -- -D warnings` and
  `cargo clippy -p dstore-client-rs --test golden -- -D warnings` report nothing in these files. Both still
  exit 101 on siblings' files: `strconv.rs`, `time.rs` and `tests/golden_tests/udiff.rs`.

### Still open

- The CI `vectors` job has not run on GitHub.
- `tools/vectorgen/docs/gocompat-a.md` is not yet assembled into VECTORS.md, which is not owned here.
