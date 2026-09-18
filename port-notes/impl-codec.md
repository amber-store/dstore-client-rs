# impl-codec: layer L1 notes

Owner: codec. `crates/codec/` (enc, dec, wellformed, error, field, macros and their tests),
`tools/vectorgen/family_codec.go` (families `codec/encode.json`, `codec/decode.json`),
`tools/vectorgen/docs/codec.md`, `tests/golden_tests/codec.rs`.

## What is implemented

- **Pass 1** (`wellformed.rs`): a port of fxamacker v2.9.3 `valid.go` as `Unmarshal` calls it
  (`wellformed(false, false)`).
  - Limits: max nesting 32 (tag chains add a level per additional tag, scanned iteratively), 131072 array
    elements and map pairs checked right after a definite head and while counting indefinite items.
  - Errors: int64 length overflow texts; the ai 28-31, break, two-byte simple value and chunk texts; `EOF`,
    `unexpected EOF` and extraneous data.
- **Pass 2** (`dec.rs`): one internal `value(kind, go_type)` ports `parseToValue` for the Go kinds dstore
  uses (uint widths, int, bool, float64, string, `[]uint8`, other slices, struct).
  - Tag preamble: strip 55799 tags, then check built-in tags 0-3 against their content heads.
  - Fill rules: `fill*` exactly, including simple values into integers, arrays into `[]byte`, bignums (raw
    bytes into slices, magnitude into integers, overflow details with `big.Int` decimal text), null as a
    no-op or nil, and a tagged null into `*T` allocating the pointer.
  - `read_map_struct`: definite or indefinite maps; integer keys beyond int64 are errors; text keys are
    UTF-8 checked, then never match; other key types are errors; the first duplicate of a matched key
    wins and later ones are skipped unexamined; unknown keys are skipped.
  - Field rewrite: an error leaving a field becomes `"<GO_NAME>.<key>"`, so the outermost field wins.
  - Failing fast returns the same first error in document order (§2.1.2 argument); callers discard
    values on error.
- **Encoding** (`enc.rs`): shortest heads via `amber_store_core::cbor::append_head`; `f64_canonical` ports
  `encodeFloat` with `ShortestFloat16`, `NaNConvert7e00`, `InfConvertFloat16` and x448/float16 v0.8.4
  `PrecisionFromfloat32`, `f32bitsToF16bits` and `f16bitsToF32bits` (a unit test round-trips all 65536
  half-precision bit patterns).
- **Fields** (`field.rs`): all 17 impls of §4.2 with fxamacker `isEmpty*` semantics (`-0.0` empty, NaN not).
- **`cbor_struct!`**: the scaffold's syntax and expansion are kept as they were. The field rewrite lives in
  `Dec::read_map_struct`, which gets `GO_NAME` from the expansion.

## Additions beyond PORTING.md §4.2 (nothing in §4 was changed)

1. `Dec::read_bytes_opt(go_type) -> Result<Option<Vec<u8>>, DecodeError>` and
   `Dec::read_array_opt(go_type, elem) -> Result<Option<Vec<T>>, DecodeError>`: the nil-keeping forms that
   `Option<Vec<u8>>`, `Vec<Option<Vec<u8>>>` and `Option<Vec<T>>` need. `read_bytes`/`read_array` are
   these with `unwrap_or_default()`.
2. `CborType::of_initial_byte(u8) -> CborType` and `CborType::as_str(self) -> &'static str`.
3. `DecodeError` gained doc comments only; its variants and texts are unchanged.

## Findings that refine the spec

- **Map-key errors inside struct fields are rewritten.** codec-wire-ticket D11 says map-key errors are
  not rewritten. They are `*UnmarshalTypeError`s, so `decodeToStructField` sets their field like any
  other type error. Verified in `codec/decode.json` ("key bstr in leaf", "key negint overflow in ptr"):
  `cbor: cannot unmarshal byte string into Go struct field main.codecOmit.14 of type string (map key is of
  type byte string and cannot be used to match struct field name)`. At the top level the §4.2 variants
  `MapKey` / `MapKeyOverflow` are returned. Once inside a field they become
  `DecodeError::Type(UnmarshalTypeError { go_type: "string" | "int64", detail: "map key is of type …" |
  "<n> overflows Go's int64", struct_field })`. The other unrewritten errors are as D11 says: UTF-8,
  built-in tag content and pass-1 errors.
- **A tagged null into `*T`** (`c6 f6`, `d9d9f7 f6`) gives `Some(T::default())`: `parseToValue` checks
  for a bare null before allocating the pointer. A tagged null into a slice is nil.
- **Bignum tags into `bool`/`string`/struct** fitting `uint64` give a type error without detail. Beyond
  `uint64`, the detail is `(<decimal> overflows <type>)`, e.g. `(18446744073709551616 overflows bool)`.
- **Go 1.26 `reflect.Value.IsZero` treats -0.0 as zero.** The vector generator uses its own
  `codecIsZero` so rendered values keep a negative zero.

## Known limits

- A bignum overflow detail holds the decimal text of the whole content. Rust computes it about 8 times
  more slowly than Go's `big.Int.String`, with the same subquadratic growth (see the review below).
  - 1 MiB: 3.2 s in Rust (release), 0.40 s in Go.
  - 4 MiB: 30 s in Rust, 3.8 s in Go.

  Only a peer that sends megabytes of bignum into an integer field reaches it.
- `Vec<Vec<u8>>` fields cannot hold a nil element (R6). The golden test skips only the re-encoding
  comparison for decoded values holding one.

## Vectors

`tools/vectorgen/docs/codec.md` has the schema. Regenerate with
`go -C tools/vectorgen run . ../../tests/golden codec`. Two runs produce identical bytes. `decode.json`
(1.28 MB, 4083 cases) crosses 124 item shapes with every field kind, slice elements, nested struct fields
and the top level, plus about 546 structure cases. `encode.json` (113 KB) holds 524 scalar cases and 151
struct cases.

## Toolchain note

During this work sibling modules of `dstore-gocompat` did not compile (mid-edit), and `dstore-codec`
depends on `dstore-gocompat` in its manifest, although no codec code uses it. So the codec tests ran in a
scratch Cargo workspace outside the repo:
- the real `crates/codec/src/lib.rs` by path, with only `amber-store-core` and `thiserror`;
- a copy of testkit's `golden`/`splitmix` pointed at the repo's `tests/golden`;
- a runner including `tests/golden_tests/codec.rs` by path.

The `dstore-gocompat` dependency of `dstore-codec` is unused. PORTING.md §3.2 lists it, so it stays.

## Review

Adversarial review by review-codec (same file ownership). The review found one real defect, a
performance divergence on hostile input, and fixed it. It found no correctness divergence from Go.

### What was checked

- **Pass 1**, line by line against fxamacker v2.9.3 `valid.go`:
  - head errors, and the order of the depth, indefinite-length, int-overflow and count checks;
  - tag chains and the extraneous-data check.
- **Pass 2** against:
  - `parseToValue`: the pointer null check before allocation, the self-described tag strip, the built-in
    tag check, bignums, and simple values switched on ai;
  - the `fill*` functions, `parseArrayToSlice`, `parseArrayToStruct` (the text without toarray),
    `parseMapToStruct` (key kinds, first-error recording, `checkDupField`) and `decodeToStructField`
    (the rewrite of every `*UnmarshalTypeError`, map-key errors included);
  - `skip`, `getHead` and `validBuiltinTag`.

  Text keys never match keyasint fields: `getDecodingStructType` indexes those fields only by integer.
  The default simple-value registry rejects nothing.

  Failing fast is equivalent to Go. Every Go error site records only the first error, in document order,
  and well-formed input cannot panic while Go continues after that error.
- **Encoder** against:
  - `encodeFloat`, and x448/float16 v0.8.4 `PrecisionFromfloat32`, `f32bitsToF16bits` and
    `f16bitsToF32bits`;
  - `isEmpty*`.

  A Go `float64` → `float32` conversion that overflows gives float64 encoding in both implementations.
- **API.** Compared with the L0 scaffold, only items were added (`read_bytes_opt`, `read_array_opt`,
  `CborType::of_initial_byte`, `as_str`). No §4.2 signature changed. The lib.rs module list is unchanged,
  and no data path uses `unwrap`/`expect`.
- **Tests.**
  - dstore has no `codec` tests.
  - The core-rs templates of codec-wire-ticket §6 are ported in `crates/codec/tests/structs.rs`. Two have
    other names: `fxamacker_verbatim_messages` is `error::tests::verbatim_texts` plus
    `probe_error_texts`, and `decode_simple_values_fill_created_at` is
    `decode_simple_values_fill_integer_fields`. `decode_rejects_indefinite_length_map` is inverted.
  - Nothing is `#[ignore]`d.
  - The golden runner asserts `wellformed_error`, `go_error` verbatim, the decoded value and the
    re-encoding. The only exception is R6, documented.
- **Vectors.**
  - Regenerated twice into temporary directories: identical, and equal to the committed files.
  - JSON conventions of PORTING §7: struct/ordered-slice JSON, decimal-string u64/i64, hex bytes, `null`
    for nil.
  - `docs/codec.md` documents all three case schemas.
  - Every G4 shape and G5 structure item of codec-wire-ticket §5 is present.
- **Differential fuzz.** A scratch Go program generated 600000 structure-aware random inputs (two seeds),
  deleted afterwards. It mixed:
  - random tags and bignums, simple values and floats, non-canonical heads, indefinite items with wrong
    chunks;
  - duplicate, unknown and wrongly typed keys, nested structs;
  - truncations and byte flips.

  fxamacker decoded each input into the codec test structs. A temporary Rust test compared
  well-formedness texts, error texts and re-encodings: 0 mismatches over 411 distinct error shapes, with
  44% of inputs decoding successfully.

### Findings

1. **Fixed: quadratic bignum overflow text.** `big_decimal` divided repeatedly by 10^9, so it took 96 s
   for a 1 MiB bignum into an integer field, where Go takes 0.40 s. A 16 MiB frame would have taken hours
   on a tokio worker, and a ticket of a few hundred KiB on argv several seconds.

   It now splits the binary limbs recursively at powers of two and recombines the halves with Karatsuba
   multiplication in base 10^9. Repeated division remains for leaves of at most 64 limbs, and schoolbook
   multiplication below 32 decimal limbs.

   Measured in release:

   | Content | Before | After | Go |
   |---|---|---|---|
   | 64 KiB | 400 ms | 73 ms | 22 ms |
   | 256 KiB | 5.9 s | 350 ms | 59 ms |
   | 1 MiB | 96 s | 3.2 s | 0.40 s |
   | 4 MiB | — | 30 s | 3.8 s |

   Larger thresholds (64/128) were not faster. The full error texts are identical to Go's: FNV-1a hashes
   compared for tags 2 and 3 at every size in the table.

   New unit tests check the conversion against repeated division (0-140 limbs and up to 3000 limbs,
   random, all-ones, power-of-two and sparse patterns) and Karatsuba against schoolbook (balanced,
   unbalanced and all-(10^9-1) operands).

   `codec/decode.json` gains 6 structure cases, pinned against `big.Int.String`:
   - 256, 257 and 260 bytes: both sides of the leaf boundary, and the plus-one carry of tag 3;
   - 4096 bytes;
   - 16384 bytes three structs deep.

   No existing case changed.
2. **Remaining, not a defect:** the constant factor of about 8 against Go for megabyte bignums (Known
   limits).

### Gates

- `cargo test -p dstore-codec`: 24 unit tests, 16 integration tests, 2 doctests.
- `cargo test -p dstore-client-rs --test golden codec::`: 3 tests over 675 encode cases and 4083 decode
  cases.
- `rustfmt --check --edition 2024` over the owned files; `gofmt` and `go vet main.go util.go
  family_codec.go`.
- In-repo `cargo clippy -p dstore-codec --all-targets --no-deps -- -D warnings`. It needs `--no-deps`
  while sibling `dstore-gocompat` has lints (`strconv.rs`, `time.rs`).
- `go vet .` over the whole vectorgen package fails in a sibling file: `family_view.go` imports
  `crypto/ed25519` without using it.
