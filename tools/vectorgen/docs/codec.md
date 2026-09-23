# Family `codec`

Files: `tests/golden/codec/encode.json`, `tests/golden/codec/decode.json`. Generator:
`tools/vectorgen/family_codec.go` (`go run . ../../tests/golden codec`). Rust tests:
`tests/golden_tests/codec.rs` (`encode_scalars`, `encode_structs`, `decode_cases`).

The values come from dstore v0.1.10 `codec.Marshal` and `codec.Unmarshal`, which are fxamacker/cbor v2.9.3
`CanonicalEncOptions().EncMode()` and `DecOptions{}.DecMode()`. The inputs are Go test structs that mirror
every field kind and struct-tag combination dstore declares. The Rust tests declare the same structs with
`cbor_struct!`, with the same Go names (`main.codecOmit`, …), so error texts compare verbatim.

## Test structs

| Vector type | Go type | Fields (key: Go type → Rust type) |
|---|---|---|
| `Leaf` | `main.codecLeaf` | 0 `Name string` → `String`; 1 `Key []byte` → `Option<Vec<u8>>`; 2 `N int64` omitempty → `i64` |
| `Omit` | `main.codecOmit` | all omitempty: 0 `Int int` → `i64`; 1 `I64 int64`; 2 `U8 uint8`; 3 `U16 uint16`; 4 `U32 uint32`; 5 `U64 uint64`; 6 `Bool bool`; 7 `F64 float64`; 8 `Str string`; 9 `Bytes []byte` → `Vec<u8>`; 10 `List [][]byte` → `Vec<Vec<u8>>`; 11 `ListN [][]byte` → `Vec<Option<Vec<u8>>>`; 12 `Strs []string`; 13 `U16s []uint16`; 14 `Leaves []codecLeaf` → `Vec<Leaf>`; 15 `Ptr *codecLeaf` → `Option<Box<Leaf>>`; 24 `Far24 uint64`; 256 `Far256 bool`; 65536 `Far65536 string` |
| `NoOmit` | `main.codecNoOmit` | no omitempty: 0-8 as `Omit`; 9 `Bytes []byte` → `Option<Vec<u8>>`; 14 `Leaves []codecLeaf` → `Option<Vec<Leaf>>`; 15 `Ptr *codecLeaf` → `Option<Box<Leaf>>` |
| `Mid` | `main.codecMid` | 0 `Leaves []codecLeaf` omitempty; 1 `Leaf *codecLeaf` omitempty |
| `Top` | `main.codecTop` | 0 `Mids []codecMid` omitempty; 1 `Mid *codecMid` omitempty; 2 `Name string` omitempty |
| `Wide` | `main.codecWide` | `F0`…`F24 uint8`, omitempty, keys 0-24 (map heads `b7`, `b818`, `b819`) |

## Value rendering

A struct value (`value` in both files) is a JSON object of the struct's **non-zero** fields in declaration
order, keyed by Go field name. A field is zero when `reflect.Value.IsZero` says so, except that a
negative-zero float is not zero. So a nil slice or pointer is left out, and an empty non-nil slice is kept.

| Go kind | JSON |
|---|---|
| `int`, `int64`, `uint64` | decimal string |
| `uint8`, `uint16`, `uint32` | number |
| `bool` | boolean |
| `float64` | decimal string of `math.Float64bits` |
| `string` | string |
| `[]byte` | lowercase hex, `null` for nil |
| other slices | array, `null` for nil (elements rendered the same way, a nil `[]byte` element as `null`) |
| `*T` | the struct object, `null` for nil |
| struct | object of non-zero fields |

The Rust test reads a rendering into the Rust struct (`Mirror::from_json`, unknown field names panic) and
compares `to_json(rust value)` with `to_json(from_json(go value))`, where `to_json` renders every field.
That comparison checks everything the Rust types can hold. It deliberately ignores what they cannot hold:
nil versus empty for omitempty collections (`Vec`), and a nil element of `Omit.List`, which Rust decodes
as an empty vector (codec-wire-ticket R6).

## `codec/encode.json`

```json
{
  "scalars": [ScalarCase, ...],
  "structs": [StructCase, ...]
}
```

**ScalarCase:** `codec.Marshal` of one Go scalar.

| Field | Meaning |
|---|---|
| `name` | unique case name |
| `kind` | `uint` (a `uint64`), `int` (an `int64`), `float64`, `bool`, `null` (a nil `[]byte`), `bytes` (a `[]byte`), `text` (a `string`) |
| `u64`, `i64` | decimal string, for `uint` and `int` |
| `bits` | decimal string of the float64 bits, for `float64` |
| `bool` | for `bool` |
| `data` | a Payload (`{"hex"}` or `{"seed","len"}`), for `bytes` and `text` (text data is valid UTF-8) |
| `output_hex` | the encoding, for every kind except `bytes` and `text` |
| `output_head_hex` | for `bytes` and `text`: the encoding is this head followed by `data` |

Content: unsigned and signed integers at every head boundary (0, 23, 24, 255, 256, 65535, 65536,
2^32-1, 2^32, max, and the negative mirrors down to `MinInt64`). Floats: the spec's list
(0, -0, 0.5, 1.5, 0.1, 65504, 65520, 1e-40, 3.4e38, ±Inf, NaN, `MaxFloat64`,
`SmallestNonzeroFloat64`, …), float16 and float32 boundaries, NaN payloads, a sweep of `m×2^e` around
the float16 subnormal range (x448/float16 `PrecisionUnknown`, decided by round trip), and 48 random
float64, float32 and float16 bit patterns each. Also `false`, `true`, a nil `[]byte`, byte strings of
length 0-65536, and text strings.

Rust: `Enc::uint`, `Enc::int`, `Enc::f64_canonical`, `Enc::bool`, `Enc::null`, `Enc::bytes`, `Enc::text`.
The output must equal the expected hex.

**StructCase:** `codec.Marshal` of a test struct value.

| Field | Meaning |
|---|---|
| `name` | unique case name |
| `type` | vector type (table above) |
| `value` | the Go value, rendered as above |
| `output_hex` | the encoding |

Content:
- zero values;
- each `Omit` field alone at the integer boundaries of its type, floats (-0.0 omitted), strings and byte
  strings of 1/23/24/255/256 bytes, empty collections (omitted), nil elements in `ListN`, 24-element lists;
- far keys;
- `NoOmit` nil versus empty (`f6` versus `40`/`80`), -0.0 and NaN;
- nested `Top`;
- `Wide` with 1, 23, 24 and 25 fields.

Encode values never put a nil element in `Omit.List`, which Rust cannot represent. Rust:
`marshal(&Mirror::from_json(value))` must equal `output_hex`.

## `codec/decode.json`

```json
{ "cases": [DecodeCase, ...] }
```

**DecodeCase:** `codec.Unmarshal` of the input into a new value of `type`.

| Field | Meaning |
|---|---|
| `name` | unique case name |
| `type` | vector type |
| `input_hex` | input prefix |
| `repeat_hex`, `repeat_count` | optional: `repeat_hex` appended `repeat_count` times (large counts) |
| `suffix_hex` | optional: appended last |
| `wellformed_error` | `DecOptions{}.DecMode().Wellformed(input)` (pass 1 alone): error text or `null` |
| `go_error` | `codec.Unmarshal` error text, or `null` |
| `value` | with `go_error` null: the decoded value, rendered as above; otherwise `null` |
| `value_cbor_hex` | with `go_error` null: `codec.Marshal` of the decoded value; otherwise `null` |

Content:
- **Field matrix.** 124 single-item shapes (`codecShapes`): canonical and non-canonical heads, every
  integer width boundary and overflow, definite and indefinite strings, invalid UTF-8 (including a rune
  split across chunks), arrays, maps, simple values, floats, null, undefined, and tags. The tags are
  self-described, unknown, bignums in and out of range, and built-in tags 0-3 with bad content. They are
  crossed with:
  - every `Omit` field 0-15 (`a1 <key> <shape>`);
  - `NoOmit` 9, 14 and 15;
  - the first element of `Omit` 9-14 (`a1 <key> 81 <shape>`);
  - each field of a `Leaf` inside `Omit.Leaves` (the rewrite to `main.codecOmit.14`);
  - the top level.
- **Structure.**
  - Empty input, trailing data, stray breaks.
  - Additional information 28-30 on every major, 31 on majors 0/1/6.
  - Two-byte simple values 0-33.
  - Indefinite strings with wrong or nested chunks; indefinite maps with odd item counts; unterminated items.
  - Map keys of every type, including integer keys beyond int64, invalid UTF-8 text keys, and the same
    inside nested structs.
  - Duplicate keys: first wins, unchecked, after null.
  - Error ordering: the first error in document order wins; pass 1 precedes pass 2.
  - Skipped values never examined.
  - Nil versus empty and pointers (a tagged null allocates `*T`).
  - Errors naming the outermost field three levels deep.
  - Nesting at 30/31/32 arrays, maps and tag chains, 32/33/34 tags, up to 100000 tags.
  - Claimed and real counts at 131072/131073 for definite and indefinite arrays and maps.
  - Large bignums (256, 257, 260, 4096 and 16384 bytes) into integer fields. The overflow text holds the
    whole decimal, so Rust's recursive conversion is checked against `big.Int.String`.
  - Lengths beyond int64.
  - Far keys; `Wide` maps.
  - Full canonical encodings and every truncation of them.

Rust:
- `well_formed(input)` must match `wellformed_error`;
- `unmarshal::<T>(input)` must fail with exactly `go_error`, or succeed with the rendered value;
- `marshal` of the decoded value must equal `value_cbor_hex`, except when Go's value has a nil
  `Omit.List` element (R6).
