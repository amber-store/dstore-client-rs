# impl-ticket: `dstore-ticket` (layer L2)

Owner: ticket. Files: `crates/ticket/` (`Cargo.toml`, `src/lib.rs` and its unit tests), `tests/golden_tests/ticket.rs`.

## What is implemented

PORTING.md §4.4 as written. No public item was added or changed.

- `Member`, `Ticket` through `cbor_struct!`.
- `Ticket::encode`: `"dstore1"` plus the lower-cased `gocompat::base32::encode_nopad(STD_ALPHABET, marshal(t))`.
- `Ticket::ids`, `members`, and `Display` (= `encode`).
- `parse(&[u8])`, in Go's order:
  1. `strings::trim_space`, then `ticket: empty`.
  2. The literal prefix test, `strings::to_lower(s).starts_with("dstore1")`.
  3. `s[7..]` of the original bytes, upper-cased, through `gocompat::base32::decode_nopad`.
  4. `codec::unmarshal`.
  5. `ticket: no members`.

  Id lists split on `, SP \t \n \r` only (`strings::fields_func`). Each field is lower-cased before
  `parse_endpoint_id`, and the error quotes the original field with `quote::quote`.
- `parse_endpoint_id(&[u8])`: go-iroh `ParseEndpointID` on the bytes as given, with no lower-casing.
  - Exactly 64 bytes go to `gocompat::hex::decode_string` (either case, as Go's `hex.DecodeString`).
  - Anything else goes to `decode_nopad(STD_ALPHABET, to_upper(s))`, whose result must be 32 bytes.
  - Then comes the curve check.
  - On any error, the z-base-32 hint is added when `decode_nopad(ZBASE32_ALPHABET, s)` gives 32 bytes.
- `is_valid_public_key`: `iroh_base::PublicKey::from_bytes(b).is_ok()`.

## Curve check: confirmed, no filippo port needed

`iroh_base::PublicKey::from_bytes` → ed25519-dalek 3.0.0 `VerifyingKey::from_bytes` → curve25519-dalek 5.0.0
`CompressedEdwardsY::decompress` accepts exactly what filippo `edwards25519.Point.SetBytes` (v1.2.0, go-iroh's
pin) accepts. The code paths are the same:
- y is loaded into 51-bit limbs with bit 255 masked and not reduced, so y ≥ p works as y mod p;
- `u = y² − 1`, `v = d·y² + 1`, `r = (u·v³)(u·v⁷)^((p−5)/8)`, square test `correct | flipped`, compared on
  canonical bytes;
- the sign bit only selects ±x and never rejects, so x = 0 with the sign bit set is accepted;
- all-0xff (y = 2^255 − 1 ≡ 18 mod p, sign set) is valid.

Evidence:
- `ticket/curve.json`: all 146 cases (93 valid).
- A one-off differential run, not committed: 200000 Go `key.NewPublicKey` results over random arrays, small
  y, y from p − 64 to 2^255 − 1, and valid keys, each with a random sign flip. 0 mismatches.

The same run also compared 100000 `ticket.Parse` inputs and 50000 raw `ParseEndpointID` inputs. The inputs
mixed:
- id lists with every separator;
- lookalikes, case flips and newlines;
- mutated tickets and random CBOR bodies;
- White_Space and non-space wrappers, and invalid UTF-8.

It compared error texts, `IDs()`, `Encode()` and the CBOR bytes: 0 mismatches.

## Findings that refine the spec

1. **codec-wire-ticket §2.4.2 probe text has a typo.** The printed `dstore1umaf…` string is 183 characters,
   with one `a` too many, while the text says 182. Go's string (`ticket/encode.json` case `probe`) is 182
   characters, and the port produces it.
2. verification.md §2.2's claim that go-iroh rejects 52-character ids with non-zero trailing bits is wrong,
   as codec-wire-ticket Addenda 7 already says. `ticket/parse.json` `b32id1-last-symbol-*` accepts all 32
   variants. Symbols `q`…`7` give a different last byte (`…cd25`), because the last symbol carries one data
   bit and four trailing bits.
3. In a `dstore1` body only U+0131 and U+017F are accepted lookalikes. U+0130 and U+212A stay non-ASCII
   under `ToUpper` and give `illegal base32 data at input byte <i>`. In an id, all four are accepted
   (`ToLower` first).

## Tests

- **Unit** (`cargo test -p dstore-ticket`): 19 tests (18 by the implementer, 1 added in review).
  - dstore `TestRoundTrip` and `TestParseIDs`. The keys come from `iroh_base::SecretKey::from_bytes` seeds
    instead of random ones.
  - go-iroh `TestPublicKeyFromStringHex`, `TestPublicKeyAllZeroIsValid`, `TestParseEndpointIDRejectsGarbage`,
    `TestPublicKeyInvalidCurvePoint`, `TestParseEndpointIDRejectsZ32`, `TestParseBase32UpperAndLower`.
  - `TestEndpointIDEncoding`, text form and z-base-32 decode only: the port has no `ParseEndpointIDZ32`, and
    the binary marshalling does not apply.
  - The probes of codec-wire-ticket §2.4.1-§2.4.9, curve rows, error `source()` chains, and a cross-check
    that data-encoding `BASE32_NOPAD` gives the same text. That cross-check is the only use of the
    `data-encoding` dependency.
- **Golden** (`cargo test -p dstore-client-rs --test golden ticket::`): 4 tests.
  - `encode`: `ticket/encode.json`, 15 cases. Checks CBOR, the decode round trip, `encode`, `Display`, `ids`,
    `members().len()` and `parse(encoded)`.
  - `parse_cases`: `ticket/parse.json`, 208 cases. Checks the ticket value, re-encoding and `ids`, or the exact
    error text.
  - `curve`: `ticket/curve.json`, 146 cases. Checks `is_valid_public_key`, `parse_endpoint_id` of the hex and
    base32 forms, and `parse` of the hex form.
  - `decode`: the 181 `ticket` cases of `wire/decode.json` (field matrix and structure cases). Checks decisions,
    values, verbatim errors and canonical re-encodings. No other module tested this section against the real
    `Ticket` type.
  - The 4 tests passed in the root `golden` binary. They were run again after the last changes to the crate's test
    code, while siblings `dstore-view` and `dstore-worktree` did not compile. That run used a scratch runner outside
    the repo: copies of testkit's `golden`/`splitmix` modules and this module included by `#[path]`. All 4 passed,
    and `cargo clippy --tests -- -D warnings` was clean.
- **Checks:** `rustfmt --check --edition 2024` over both files. `cargo clippy -p dstore-ticket --all-targets -- -D
  warnings` is clean.

## Notes for dependents

- `view::ticket_from_view` builds `Ticket { cluster_id: Some(..), incarnation, members: Some(..) }`, with
  `Member.addrs` from the view.
- `transport/ids.json` `endpoint_ids` can use `dstore_ticket::is_valid_public_key`: it is the same check.
- `KeyError::Z32`'s `source()` is the boxed field itself. Downcast it with `downcast_ref::<Box<KeyError>>()`, not
  `downcast_ref::<KeyError>()`.

## Review

Reviewer: review-ticket. It re-read PORTING §0-3, §4.4, §5-7, codec-wire-ticket §2.4, §3.4, §4.4, §5 G7-G11, §6,
§7 K5-K7 and the Addenda, and VECTORS.md family `ticket`, `wire/decode.json` and `gocompat/base32.json`.

### Checked

- **Go line by line.**
  - dstore `ticket/ticket.go`: `Encode`, `IDs`, `Parse` and `parseIDs`.
  - go-iroh v0.2.0 `key.go`: `ParseEndpointID`, `ParsePublicKey`, `decodeBase32OrHex` and `NewPublicKey`.
  - go-iroh v0.2.0 `key_core.go`: the two base32 decoders.

  The port matches Go on:
  - the validation order;
  - every error text, and the `%q` of the original field;
  - lower-casing the field before `ParseEndpointID`, and the z-base-32 hint applied to those same bytes, not upper-cased;
  - hex routing on the lowered length, where `hex.DecodeString` accepts either case;
  - `copy` into 32 bytes;
  - nil `Addrs`, nil `ClusterID` and incarnation 0 for id lists.

  Nothing on a data path can panic.
- **Curve check, by reading both sides.** The Go side:
  - filippo edwards25519 v1.2.0 `Point.SetBytes`, `field.Element.SetBytes` and `SqrtRatio`.

  The Rust side:
  - iroh-base 1.2.0 `PublicKey::from_bytes` only calls ed25519-dalek 3.0.0 `VerifyingKey::from_bytes`;
  - that only decompresses, through curve25519-dalek 5.0.0 `decompress::step_1`/`step_2`, `FieldElement51::from_bytes`
    and `sqrt_ratio_i`.

  The two are the same:
  - the 51-bit limbs are loaded with bit 255 masked and not reduced;
  - `u = y² − 1`, `v = d·y² + 1`, with the same `r` formula;
  - both return the square flag `correct | flipped`, compared on canonical bytes;
  - the sign bit only negates.

  On x86_64 the simd backend covers point arithmetic only, and decompression uses the serial u64 field, so Linux accepts
  the same set.
- **Vectors.** Every case is asserted, and none is skipped or weakened:
  - 15 in `ticket/encode.json`;
  - 208 in `ticket/parse.json` (146 ok, 62 errors; 4 with invalid UTF-8 input);
  - 146 in `ticket/curve.json` (93 valid);
  - 181 in the `ticket` section of `wire/decode.json`.

  No ticket decode case uses `payload_parts`, `canonical_blake3` or `null_element`. The test handles only `payload_hex`
  and `canonical_hex`, so any such case would fail rather than be skipped.
- **Go tests** of codec-wire-ticket §6 are all ported:
  - `TestRoundTrip` and `TestParseIDs`;
  - the 7 go-iroh key tests (only the text and z-base-32 parts of `TestEndpointIDEncoding`).
- **Spec typo claim confirmed.** The §2.4.2 string is 183 characters, and Go's string is 182.
- **Independent Go-vs-Rust differential.** A scratch Go module used the pinned dstore v0.1.9 and go-iroh v0.2.0 from the
  module cache. It had its own generator and seed, and it and its output have been deleted. 0 mismatches:
  - 80000 `ticket.Parse` inputs, compared on error text, `Encode`, `IDs` and CBOR. The inputs mixed:
    - id lists with every separator and non-separator;
    - lookalikes and runes whose case mapping changes the byte length (U+0130, U+0131, U+017F, U+212A, U+212B, U+2126,
      U+1E9E, U+00DF, U+01C5, U+0345, U+023A, U+2C65, U+0149);
    - invalid UTF-8;
    - White_Space and non-space wrappers;
    - mutated and case-flipped tickets with newlines and extra symbols;
    - random and corrupted CBOR bodies, and prefix variants.
  - 40000 raw `ParseEndpointID` inputs: hex, base32, z-base-32 and random, mutated and case-flipped.
  - 100000 `NewPublicKey` arrays: random, valid keys with sign flips, y near 2^255 (including y ≥ p) and small y.

### Fixed

1. **`KeyError::Z32` had no `source()`.**
   - Go wraps with `%w`, so `errors.Is(err, ErrDecodeBase32)` holds, and PORTING §3.4/§5.2 require a wrapped error to
     be the `source()`.
   - Fix: `#[source]` on the field. No public signature changed and `Display` is unchanged.
   - Because the field is a `Box<KeyError>`, thiserror returns the box as the source.
   - No dstore code calls `errors.Is` on go-iroh key errors, so no output changes.
2. **Tests.**
   - New `non_ascii_runes_mapping_to_ascii` checks every rune. `parse` slices the original bytes at `PREFIX.len()`,
     relying on this, and §2.4.8 claims hex routing sees only ASCII hex digits:
     - only U+0130 and U+212A lower-case to ASCII;
     - only U+0131 and U+017F upper-case to ASCII.
   - `error_sources` also checks the Z32 source chain and the CBOR source.
   - `endpoint_id_encoding`: the z-base-32 round trip now passes the curve check, as `ParseEndpointIDZ32` does.
   - `parse_id_lists` runs the `TestParseIDs` body over 16 deterministic key pairs, where Go draws two random keys per
     run.

No other defect was found in the implementation.

### Gates (review)

- `cargo test -p dstore-ticket`: 19 pass, 0 ignored.
- `cargo clippy -p dstore-ticket --all-targets -- -D warnings`: clean.
- `rustfmt --check --edition 2024` over both owned files: clean.
- `cargo test -p dstore-client-rs --test golden ticket::`:
  - Early in the review, before any change, the root golden binary built and the 4 tests passed.
  - After the changes it could not build in the workspace. Sibling crates were mid-edit and 12 retries a minute apart
    failed: `crates/client/src/error.rs` lifetime errors, a duplicate key in `crates/transport-iroh/Cargo.toml`, and
    `crates/gocli`.
  - The same unchanged module was therefore run on the final code through a scratch runner outside the repo: a package
    with path dependencies on `dstore-ticket`, `dstore-gocompat`, `dstore-codec` and `dstore-testkit`, including
    `tests/golden_tests/ticket.rs` by `#[path]`. testkit's `golden_dir` is fixed at compile time, so the runner reads
    the committed `tests/golden`.
  - All 4 pass (encode 15, parse 208, curve 146, decode 181), and `cargo clippy --tests -- -D warnings` is clean. The
    runner is deleted.
  - The review changed no code that the golden module calls, except for `KeyError::Z32`'s `source()`.
- Rerun `cargo test -p dstore-client-rs --test golden ticket::` in the workspace once the siblings compile.
