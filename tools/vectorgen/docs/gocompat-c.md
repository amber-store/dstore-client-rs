# gocompat-c: `gocompat/json.json`, `gocompat/hex.json`, `gocompat/base32.json`, `gocompat/path.json`

Family `gocompat_io` (`tools/vectorgen/family_gocompat_io.go`), plus the helper program `cmd/goerrno`. They
lock the Go standard-library behaviour behind `dstore-gocompat` `json`, `hex`, `base32`, `path` and `errno`
(PORTING.md §4.1), with go1.26.5.

Regenerate:

```sh
nix develop -c go -C tools/vectorgen run . ../../tests/golden gocompat_io
nix develop -c sh -c 'cd tools/vectorgen && go run ./cmd/goerrno > ../../crates/gocompat/src/errno_tables.rs'
```

Conventions are the root `VECTORS.md` ones. Every byte string (inputs, paths, field values) is lowercase hex
in a field ending in `_hex`, because the cases include invalid UTF-8. `out_hex` is `null` exactly when Go
returned an error, and `error` is `""` exactly when it did not.

The Rust tests are `tests/golden_tests/gocompat_io.rs` (root `tests/golden.rs`). Each test runs every case and
fails with the first ten mismatches.

## `gocompat/json.json`

`encoding/json` v1 (`json.MarshalIndent(v, "", "  ")` and `json.Unmarshal`) over two structs:
`worktree.Config` from dstore v0.1.9 and `main.stateJSON`, a copy of the unexported `worktree.stateJSON`.

```json
{
  "structs": [
    {"name": "config", "go_type": "worktree.Config",
     "fields": [{"name": "ticket", "kind": "string", "omitempty": false}, …]}
  ],
  "marshal": [
    {"name": "verified_sample", "struct": "config",
     "values": [{"name": "ticket", "str_hex": "…", "bool": false}, …],
     "out": "{\n  \"ticket\": …\n}"}
  ],
  "unmarshal": [
    {"name": "config_12", "struct": "config", "in_hex": "…",
     "slots": [{"name": "ticket", "set": true, "str_hex": "74", "bool": false}, …],
     "error": "json: cannot unmarshal number into Go struct field Config.ticket of type string"}
  ]
}
```

- `structs[].fields` are in declaration order, from the struct tags. `kind` is `string` or `bool`.
- `marshal[].values` hold every field in declaration order: `str_hex` for string fields (`""` for bool
  fields), `bool` for bool fields. `out` is Go's output without a trailing newline.
  - Cases: the worktree §5 item 2 samples (zero config, all fields, `&<>` and control escapes, invalid UTF-8,
    quotes), every ASCII byte, five states, and 120 random configs built from escape-heavy tokens
    (splitmix64 seed `0x6a736f6e`).
  - Rust drops `omitempty` fields that are `""` or `false`, the caller's job in `marshal_indent_object`, then
    compares bytes.
- `unmarshal[].slots`: `set` is false when the input never assigned the field (absent, unknown key, `null`, a
  type error, or any syntax error). `str_hex`/`bool` hold the value when `set`.
  - Go decodes each input twice, into structs prefilled with different sentinels, and a field counts as set
    when either run changed it.
  - Cases: hand-written inputs (empty and top-level non-objects, case folding including U+212A and U+017F,
    duplicates, nulls, nested unknown keys, type errors, invalid UTF-8, surrogate escapes, the Go scanner
    error texts, depth 10000/10001), and 300 byte mutations per struct of well-formed inputs.
  - Rust calls `unmarshal_object(in, go_type, fields)`. It compares `JsonError`'s `Display` with `error` and
    each slot with `set`/value.

## `gocompat/hex.json`

```json
{"encode": [{"in_hex": "…", "out": "…"}],
 "decode": [{"in_hex": "7a64", "out_hex": null, "error": "encoding/hex: invalid byte: U+007A 'z'"}]}
```

- `encode`: `hex.EncodeToString` of splitmix64 data, lengths 0 to 40.
- `decode`: `hex.DecodeString`.
  - Cases: Go's `errTests`, core-rs-gaps §3.7, every byte after a `0` and alone (all `%#U` texts), and 200
    random strings.
- Rust: `hex::encode`, `hex::decode_string`.

## `gocompat/base32.json`

```json
{"encode": [{"alphabet": "std", "in_hex": "…", "out": "…"}],
 "decode": [{"alphabet": "zbase32", "in_hex": "…", "out_hex": null, "error": "illegal base32 data at input byte 3"}]}
```

- `alphabet`: `std` is `base32.StdEncoding.WithPadding(base32.NoPadding)`; `zbase32` is
  `base32.NewEncoding("ybndrfg8ejkmcpqxot1uwisza345h769").WithPadding(base32.NoPadding)`.
- `encode`: `EncodeToString` of splitmix64 data, lengths 0 to 40, per alphabet.
- `decode`: `DecodeString`, per alphabet.
  - Cases: RFC 4648 pairs, Go's `TestDecodeCorrupt` inputs, newline stripping, the codec-wire-ticket §2.4.7
    probes, the 0xFF padding branch, the codec-wire-ticket §5 G8 grid (`A`, `a`, `y`, `Y` repeated 0 to 17
    times; `=` at every position of 8-symbol inputs; mixed newlines), and 300 random strings.
  - Every fixed input is decoded with both alphabets.
- Rust: `base32::encode_nopad`, `base32::decode_nopad`.

## `gocompat/path.json`

```json
{"clean": [{"in_hex": "…", "out_hex": "…"}], "dir": […], "base": […], "abs": […],
 "join": [{"elems_hex": ["61", "62"], "out_hex": "612f62"}],
 "rel": [{"base_hex": "61", "target_hex": "2f61", "out_hex": null, "error_hex": "52656c3a2063616e2774206d616b65202f612072656c617469766520746f2061"}]}
```

- `clean`, `dir`, `base`: `filepath.Clean`, `filepath.Dir`, `path.Base` (the generator checks that
  `filepath.Base` agrees).
  - Cases: Go's `cleantests`/`dirtests`/`basetests` inputs and 600 random paths over `a b . .. / \xff é`.
- `abs`: `filepath.Abs` of the absolute inputs only; relative results depend on the working directory.
- `join`: `filepath.Join`: Go's `jointests` and 200 random element lists.
- `rel`: `filepath.Rel`: Go's `reltests`, working-copy shapes, and 400 random pairs.
  - `error_hex` holds Go's error text, `""` when there is none. The text echoes both paths, which can be
    invalid UTF-8, so it is hex.
- Rust: `path::{clean, dir, base, abs, join, rel}`. `rel` returns `None` where Go errors. The text is not
  part of the Rust API: the test checks that `Rel: can't make <target> relative to <base>`, built from the
  inputs, equals `error_hex`.

## `crates/gocompat/src/errno_tables.rs` (`cmd/goerrno`)

- `go run ./cmd/goerrno` parses the `errors` array of `$GOROOT/src/syscall/zerrors_{darwin,linux}_{amd64,arm64}.go`
  with `go/parser` and prints one `ERRORS: &[(i32, &str)]` static per target.
  - It refuses a toolchain other than go1.26.5.
  - When the host is one of the four targets, it checks the parsed table against `syscall.Errno(n).Error()`.
- The tables differ by architecture: darwin/arm64 adds 106 (`interface output queue is full`), and
  linux/arm64 adds 133 (`memory page has hardware error`).
- Other architectures of the same OS fall back to the arm64 table on macOS and the amd64 table on Linux.
- Rust: `errno::errno_text` / `errno_string` (`errno <n>` when the table has no text, as `Errno.Error()`),
  unit-tested in `crates/gocompat/src/errno.rs`. CI can regenerate the file and diff it.
