# gocompat-a: `text/quote.json`, `text/case.json` and `gotables`

Owner: gocompat-a (PORTING.md §6, layer L1). The Rust side is `crates/gocompat/src/quote.rs`, `strings.rs`
and the generated `tables.rs` (PORTING.md §4.1, DD-14). The Go side is the go1.26.5 standard library
(Unicode 15.0.0): `strconv/quote.go`, `strconv/isprint.go`, `unicode/graphic.go`, `unicode/letter.go`,
`unicode/tables.go`, `unicode/utf8/utf8.go` and `strings/strings.go`.

## Families

| Family | File | Go code |
|---|---|---|
| `quote` | `text/quote.json` | `strconv.Quote`, `strconv.IsPrint`, `strconv.IsGraphic` |
| `case` | `text/case.json` | `strings.ToLower`, `strings.ToUpper`, `strings.TrimSpace`, `strings.FieldsFunc(s, unicode.IsSpace)`, `unicode.ToLower`, `unicode.ToUpper`, `unicode.IsSpace` |

Both are registered in `tools/vectorgen/family_text.go`:

```sh
nix develop -c go -C tools/vectorgen run . ../../tests/golden quote case
```

The files record `go_version` (`runtime.Version()`) and `unicode_version` (`unicode.Version`). The Rust
tests require `go1.26.5`.

### `text/quote.json`

```json
{
  "go_version": "go1.26.5",
  "unicode_version": "15.0.0",
  "cases": [
    { "name": "ticket probe", "in_hex": "7a7a0122c3a9", "out": "\"zz\\x01\\\"é\"" }
  ],
  "is_print_flips": [32, 127, 161, 173, 174]
}
```

- `cases[]`:
  - `name`: what the input exercises;
  - `in_hex`: the input bytes, often invalid UTF-8;
  - `out`: `strconv.Quote(string(in))`, always valid UTF-8.
- `is_print_flips`: every rune `r`, ascending, where `strconv.IsPrint(r) != strconv.IsPrint(r-1)`. Rune 0
  is not printable, and the answer is constant between two flips, so the list gives `IsPrint` for all
  0x110000 runes (surrogates included, which are not printable).

The cases, in file order:

1. Every byte 0x00-0x7f alone, every rune U+0080-U+00FF, every byte 0x80-0xff alone (invalid), and all
   ASCII bytes in one string.
2. `quoteSpecials`:
   - the probes of the port notes (codec-wire-ticket §2.4.9, view-placement §3.5, client-core §3.5) and
     strconv `quotetests`;
   - the escapes;
   - non-printable runes of every kind (format characters, separators, unassigned, private use,
     noncharacters, tags);
   - surrogate encodings, a CESU-8 pair, overlong forms, runes above U+10FFFF, five- and six-byte forms,
     truncated runes, stray continuation bytes.
3. The first, middle and last rune of each lookup path of `strconv.IsPrint`:
   - Latin-1 fast path, printable and not;
   - inside an `isPrint16` range, an `isNotPrint16` exception, between `isPrint16` ranges;
   - the same three for `isPrint32` below U+20000;
   - printable and not from U+20000, where no exception list exists.
4. Every graphic rune that is not printable (the `isGraphic` list).
5. Both sides (`r-1`, `r`) of every `is_print_flips` boundary, eight boundaries per case.
6. 256 random inputs, `random 0` to `random 255`.

Random inputs use splitmix64 (VECTORS.md) from seed `0x71756f7465` ("quote"), one state for the whole
sample. Each input has `next % 24` units. For each unit, `x = next` and `y = x >> 8`, and `x % 8` selects:

| `x % 8` | Unit |
|---|---|
| 0, 1 | ASCII byte `y % 0x80` |
| 2 | rune `0x80 + y % 0x80` |
| 3 | rune `y % 0x10000`; a surrogate as its invalid three-byte form `E0\|r>>12, 80\|(r>>6)&3F, 80\|r&3F` |
| 4 | rune `0x10000 + y % 0x100000` |
| 5 | invalid byte `0x80 + y % 0x80` |
| 6, 7 | `quoteRandomRunes[y % 13]`: `"`, `\`, U+007F, U+0085, U+00A0, U+00AD, U+200B, U+2028, U+FEFF, U+FFFD, U+1F600, U+E0001, U+10FFFF |

### `text/case.json`

```json
{
  "go_version": "go1.26.5",
  "unicode_version": "15.0.0",
  "cases": [
    {
      "name": "invalid bytes at the edges",
      "in_hex": "20ff206120fe20",
      "to_lower_hex": "20efbfbd206120efbfbd20",
      "to_upper_hex": "20efbfbd204120efbfbd20",
      "trim_space_hex": "ff206120fe",
      "fields_hex": ["ff", "61", "fe"]
    }
  ],
  "to_lower": [{ "lo": 65, "hi": 90, "stride": 1, "delta": 32 }],
  "to_upper": [{ "lo": 97, "hi": 122, "stride": 1, "delta": -32 }],
  "is_space_flips": [9, 14, 32, 33, 133, 134, 160, 161]
}
```

- `cases[]`:
  - `name`, `in_hex`: as in `text/quote.json`;
  - `to_lower_hex`, `to_upper_hex`: `strings.ToLower`, `strings.ToUpper`;
  - `trim_space_hex`: `strings.TrimSpace`;
  - `fields_hex`: `strings.FieldsFunc(s, unicode.IsSpace)`, `[]` when there are no fields.
- `to_lower`, `to_upper`: the complete `unicode.ToLower` / `unicode.ToUpper` mapping as runs. For
  `r = lo, lo+stride, …, hi` (stride 1 or 2), the mapping gives `r + delta`. A rune in no run maps to
  itself. Runs never overlap.
- `is_space_flips`: as `is_print_flips`, for `unicode.IsSpace`.

The cases, in file order:

1. All ASCII bytes in one string.
2. `caseSpecials`:
   - strings `upperTests`, `lowerTests` and `trimSpaceTests` cases;
   - the lookalikes U+0130, U+0131, U+017F and U+212A of the ticket parser (codec-wire-ticket
     §2.4.6-2.4.8);
   - sharp s, titlecase digraphs, final sigma, runes without a simple mapping, and mappings that change
     the byte length;
   - astral letters, and a valid U+FFFD;
   - invalid UTF-8 of each kind;
   - spaces at the edges, both `TrimSpace` fast-path exits, and format characters that are not space
     (U+180E, U+200B, U+2060, U+FEFF).
3. For every White_Space rune `W`: `W x W W y W`.
4. 256 random inputs.

Random inputs use seed `0x63617365` ("case"). Each input has `next % 16` units:

| `x % 8` | Unit |
|---|---|
| 0 | printable ASCII `0x20 + y % 0x5f` |
| 1 | `" \t\n\v\f\r"[y % 6]` |
| 2 | the `y % 25`th White_Space rune |
| 3 | the `y % n`th of the `n` runes whose `unicode.ToLower` differs, ascending |
| 4 | the same for `unicode.ToUpper` |
| 5 | invalid byte `0x80 + y % 0x80` |
| 6 | `caseRandomRunes[y % 18]`: U+0130, U+0131, U+017F, U+212A, U+00DF, U+1E9E, U+01C5, U+03C2, U+00B5, U+00FF, U+FFFD, U+180E, U+200B, U+FEFF, U+2C65, U+023A, U+10400, U+1E900 |
| 7 | rune `y % 0x110000`; a surrogate as its invalid three-byte form |

## Rust tests

`tests/golden_tests/gocompat_text.rs` (in the root `tests/golden.rs` binary) uses only the public API. It
collects every failure before failing.

- `quote_matches_go`: `quote(in) == out` for every case.
- `is_print_matches_go_for_every_rune`, `is_space_matches_go_for_every_rune`: `is_print` and `is_space`
  of every `char` against the flip lists.
- `strings_functions_match_go`: `to_lower`, `to_upper`, `trim_space` and `fields_func(_, is_space)` for
  every case.
- `case_mapping_matches_go_for_every_rune`: `to_lower` and `to_upper` of every one-rune string against the
  expanded runs.

Unit tests in `quote.rs` and `strings.rs` port strconv `quotetests` and strings `upperTests`, `lowerTests`,
`trimSpaceTests`, `fieldstests` and `FieldsFuncTests`. They also check the probes of the port notes, Go's
UTF-8 decoding edge cases, and the table invariants.

## `gotables`

`tools/vectorgen/cmd/gotables` prints `crates/gocompat/src/tables.rs`:

```sh
nix develop -c go -C tools/vectorgen run ./cmd/gotables > crates/gocompat/src/tables.rs
```

- It refuses to run on a toolchain other than go1.26.5 (`goVersion`).
- `IS_PRINT16`, `IS_NOT_PRINT16`, `IS_PRINT32`, `IS_NOT_PRINT32` and `IS_GRAPHIC` come from the scan of
  `strconv/makeisprint.go` over `strconv.IsPrint` and `strconv.IsGraphic`. Their values equal those of
  go1.26.5's `strconv/isprint.go` (424, 133, 508, 112 and 16 values).
- `WHITE_SPACE` lists the 25 runes of `unicode.White_Space`.
- `TO_LOWER` and `TO_UPPER` hold `(r, mapped)` for every rune whose simple mapping differs from itself.
- Before printing, it checks the tables over every rune against `strconv.IsPrint`, `unicode.IsPrint`,
  `strconv.IsGraphic`, `unicode.IsGraphic`, `unicode.IsSpace`, `unicode.ToLower` and `unicode.ToUpper`.
  The checks use the lookup algorithms of the Rust code.
- Every static carries `#[rustfmt::skip]`, so `cargo fmt` keeps the one-entry-per-line layout.

CI's `vectors` job regenerates the file and diffs it against the committed one.

## When Go changes

When the `go` line of dstore's `go.mod` changes (PORTING.md DD-14):

1. update `goVersion` in `cmd/gotables` and `GO_VERSION` in `tests/golden_tests/gocompat_text.rs`;
2. regenerate `tables.rs` and the `quote` and `case` families;
3. run the golden tests.
