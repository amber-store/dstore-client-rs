# impl-udiff: layer L1 notes

Owner: udiff. Files: `crates/udiff/` (all), `tools/vectorgen/family_udiff.go` (family `udiff`: `udiff/udiff.json`,
`udiff/lcs.json`, `udiff/pdqsort.json`), `tools/vectorgen/docs/udiff.md`, `tests/golden_tests/udiff.rs`, and the
generated `tests/golden/udiff/`.

PORTING.md §4.9 is implemented without changing any public signature or adding a public item. The crate still has
no dependencies.

## What is ported

- **go-udiff v0.4.1, line by line.**
  - `unified.go`: `Unified`, `ToUnified`, `toUnified`, `splitLines`, `addEqualLines`, `String`.
  - `diff.go`: `validate`, `SortEdits`, `lineEdits`, `expandEdit`.
  - `ndiff.go`: `Lines`.
  - `lcs/old.go`: `diff`, `compute`, `editGraph`, `toDiffs`, `forward`, `forwardlcs`, `lookForward`,
    `set`/`getForward`, `backwardlcs`, `lookBackward`, `set`/`getBackward`, `twosided`, `twoDone`, `twolcs`.
  - `lcs/common.go`, `lcs/labels.go` and `lcs/sequence.go`.
  - `backward`, `bdone`, `valid` and `Apply` exist only under `cfg(test)`, to port the Go tests that use them.
- **Go 1.26.5 `sort`.** `zsortfunc.go` (pdqsort and the stable path), `sort.go:59-80`, `slice.go`, and `IsSorted`.
  - The helpers are generic over a crate-private `LessSwap` trait (index-based `less`/`swap`, like Go's
    `lessSwap`).
  - `gosort::slice`/`slice_stable` wrap a slice and a closure, which lets the unit tests port Go's index-based
    testing types (swap-bounded `testingData`, the antiquicksort adversary, non-deterministic `Less`).
- **Go `int` is `isize` inside the ports**, so negative diagonals, `LastIndex == -1` and loop bounds behave as in
  Go. The public API keeps `usize`.

## Decisions and deviations

1. **`pdqsort.json` is read by root `tests/golden_tests/udiff.rs`, not by crate unit tests** (PORTING §7 lists
   `gosort` under crate unit tests). `crates/udiff/Cargo.toml` has no `dstore-testkit`/`serde_json`
   dev-dependencies, and adding them would change `Cargo.lock`. `gosort` is `pub`, so the root test reaches it.
   The crate unit tests port the Go sort tests as properties.
2. **`unified` on an internal error.** Go calls `log.Fatalf("internal error in diff.Unified: %v", err)`. Rust
   writes `internal error in diff.Unified: <err>` to stderr (without the log timestamp) and exits with status 1.
   It is unreachable: `Lines` always yields sorted, non-overlapping edits.
3. **Go panics kept as panics.** `twosided`'s `no forward paths` / `no backward paths` and out-of-range `label`
   reads are unreachable algorithm invariants (Go panics too). Every other data path is panic-free.
4. **`x+1 <= e.ux`** in `twolcs` is written `x < self.ux` (clippy `int_plus_one`), and the same for `y`. For
   integers the two are equivalent.
5. **Go tests that need unported code are adapted.**
   - `TestNEdits`, `TestNRandom` and `diff_test.go` `TestRegressionOld001`/`002` use go-udiff's `Strings`; they
     run as round-trip properties of `lines`.
   - `TestToUnified` shells out to `patch`; it uses an in-process strict unified-diff applier that checks every
     context line, deleted line and hunk count.
   - `TestLcsFix` also asserts the exact `fix()` output, which Go's test does not check. The expected values were
     taken from a scratch run of go-udiff's `lcs` package.
   - `math/rand` streams became splitmix64.
6. **Vectors of large inputs are recipes.**
   - Random cases store a splitmix64 recipe plus FNV-1a digests of the generated texts and of the output, and the
     verbatim output when it is at most 1024 bytes.
   - pdqsort cases store the permutation verbatim up to 256 elements, and always as a digest.
   - Counts: `udiff.json` has 84 cases, 60 `to_unified` cases and 3393 random cases (1234 with verbatim output);
     `lcs.json` has 213 string and 490 random cases; `pdqsort.json` has 1702 cases.
   - The three files total about 5 MB. docs/udiff.md specifies the generators.
7. **The pdqsort heapsort fallback.**
   - No regular pattern reaches it. McIlroy's antiquicksort adversary doesn't either: in go1.26.5 neither the
     adaptive adversary run through `sort.Sort` nor its static replay reaches `heapSort`. The replay repeats the
     adversary's comparison counts exactly at every size. Go's `TestAdversary` only bounds comparisons.
   - The `adversary` cases stay as pathological inputs.
   - What reaches the fallback are seeded "coin" comparators (`coin_1_2`, `coin_1_16`, `coin_15_16`), which answer
     `less` from their own splitmix64 stream. `coin_15_16` unbalances every partition. Coin cases also require the
     port to make exactly Go's sequence of `less` calls.
   - A Go coverage run (`-coverpkg=sort` over `pdqCases`) puts `pdqsort_func`, `heapSort_func` and `siftDown_func`
     at 100%.
8. **Generating from a scratch module.** While a sibling's `family_wire.go` was mid-edit, the vectorgen package
   did not compile, so early runs used a scratch module holding only `go.mod`, `go.sum`, `main.go`, `util.go` and
   `family_udiff.go`. The final vectors come from the real module: `go run . ../../tests/golden udiff`, run twice
   with byte-identical results. `go vet .` and `go test .` pass.
9. **Code the vectors cannot reach.** A scratch `go test -coverpkg` over the family's generators reached every
   block of go-udiff's Unified path except the following:
   - `forward`'s `fdone(0, 0)` early return and its "D is too large" tail. `forward` only runs inside `twolcs`, on
     a rectangle that is reachable within the limit.
   - `compute`'s `limit <= 0`.
   - `lcs.sort`'s equal-X tie. lcs never produces it, but `pdqsort.json` `x_asc_len_desc` covers the comparator.
   - `lineEdits`' unreachable `len(edits) == 0` after the fast path.
   - `log.Fatalf`, the debug checks and the panics.

   `twolcs`, `twoDone`, `fix`, `overlap`, `validate`, `expandEdit`, `toUnified` and `String` are at 100%.

## Verification

- `cargo test -p dstore-udiff`: 39 unit tests pass.
- `cargo test -p dstore-client-rs --test golden -- udiff::`: 5 golden tests pass (`unified_cases`,
  `to_unified_cases`, `unified_random`, `lcs_diff_lines`, `pdqsort_slice`).
- `cargo clippy -p dstore-udiff --all-targets -- -D warnings` and rustfmt are clean.

## Review

Adversarial review (label review-udiff). The port was assumed wrong until checked. No correctness defect
was found in the Rust code; two small fixes went into the generator and its docs.

### Checked line by line against the Go sources

- **go-udiff v0.4.1.**
  - `unified.go`: `Unified`, `ToUnified`, `toUnified`, `splitLines`, `addEqualLines`, `String`.
  - `diff.go`: `validate`, `SortEdits`, `editsSort.Less`, `lineEdits` fast and slow paths, `expandEdit`.
  - `ndiff.go` `Lines`.
  - `lcs/old.go`: `compute`, `toDiffs`, `fdone`/`bdone`, `forward`, `backward`, `forwardlcs`,
    `backwardlcs`, `lookForward`/`lookBackward`, `twosided` (both kmax scans and panics), `twoDone`, and
    `twolcs` with every special case in order.
  - `lcs/common.go`: `sort`, `valid`, `fix`, `overlap`, `prepend`, `append`, `ok`.
  - `lcs/labels.go` (a nil row is an empty `Vec`) and `lcs/sequence.go`.
- **go1.26.5 `sort`.** All 17 functions of `zsortfunc.go`, plus `xorshift`, `nextPowerOfTwo`, `IsSorted`,
  `Slice`, `SliceStable` and `SliceIsSorted`.
  - This includes `partialInsertionSort`'s `j >= 1` bound (not `j > a`), `breakPatterns`' window and
    modulus, the ninther swap count and `symMerge`'s `uint` halving.

Every arithmetic step, bound, short-circuit order and verbatim text (`diff has out-of-bounds edits`,
`diff has overlapping edits`, the hunk header rules, `\ No newline at end of file`) matches. Non-test `as
usize` index conversions only see non-negative values. The panics that remain are the unreachable ones Go
panics on too.

### Evidence beyond the committed vectors

1. **Differential run on fresh inputs.** A scratch Go program (not the vectorgen seeds) wrote 189,500
   cases, which a scratch Rust binary replayed through `dstore-udiff`. There were 0 failures.
   - 19,500 `Unified` + `Lines` cases:
     - byte-level texts with CR, invalid UTF-8 and partial lines;
     - small line alphabets;
     - edit runs around the search limit;
     - numbered lines with every m-th line changed.
   - 100,000 `ToUnified` calls over explicit edits, often unsorted, overlapping or out of bounds. 21,788
     of them returned Go's error texts, and no Go call panicked.
   - 50,000 per-byte `lcs.DiffLines` cases over `ab`/`abc`, up to 160 elements.
   - 20,000 `sort.Slice`/`SliceStable`/`SliceIsSorted` calls (n up to 5000) under seeded coin comparators
     and pair-hash comparators. Both the result and the exact number of `less` calls matched.
2. **`TestLcsFix` expectations.** The exact `fix()` results hardcoded in `lcs::tests::lcs_fix` were re-run
   against go-udiff's own `fix` (scratch internal test). All 8 match.
3. **Vector power (mutation replay).** A scratch Go program regenerated every committed recipe and
   replayed `udiff.json` and `lcs.json` against a scratch go-udiff copy with switchable mutations. The
   baseline matches every committed digest.

   | Mutation | Cases no longer matching |
   |---|---|
   | `fix` with `sort.SliceStable` | 64/1500 `heavy`, 5/150 `heavy_words`, 3/500 `heavy_edits`, 9/490 lcs random |
   | search limit 49 | 835 `heavy`, 237 `heavy_edits`, 84 `heavy_words`, 116 lcs random, 2 `hex`, 1 hand-written |
   | joining gap 5 instead of 6 | 238 `heavy_edits`, 74 `edits`, 55 `heavy`, 11 `hex`, 5 `heavy_words`, 2 `small`, 2 hand-written |
   | `lcs.sort` with `sort.SliceStable` | none |

   The last row is expected. After `fix`, and in `twolcs`' concatenations, the diagonals have distinct
   X, so every algorithm leaves them in the same order. `pdqsort.json` `x_asc_len_desc` locks that
   comparator's permutation anyway.
4. **Regeneration.** `go run . <tmp> udiff`, run twice into fresh directories, gave identical bytes,
   equal to the committed `tests/golden/udiff`. The same held after the fix below.

### Fixes

- **`tools/vectorgen/family_udiff.go`.** `pdqCase.Values` was `[]uint64` written as JSON numbers, which
  goes against VECTORS.md ("64-bit integers … are decimal strings").
  - The values are ranks below `n`, so the field is now `[]int`, the convention for small integers.
  - The output bytes are unchanged (regenerated and diffed).
  - The `Less` field comment now also names the coin comparators.
- **`tools/vectorgen/docs/udiff.md`.** The `small` row said "40% without a final newline", but the
  parameter is the odds that `strip_eol` is not `none`, and the other classes gave no odds at all. The
  table now lists each class's run lengths and alphabet choice, plus the exact `strip_eol` odds of every
  class.

### Deviations reviewed and accepted

- `pdqsort.json` is read by the root golden test: the crate has no dev-dependencies, and `gosort` is
  public.
- `unified`'s unreachable `log.Fatalf` is emulated without Go's log timestamp.
- The unreachable panics stay panics.
- The Go tests that rely on `Strings`, `patch` or `math/rand` are adapted; their properties are kept or
  strengthened.
- Large inputs are described by recipes plus digests.

### Gates (review run)

- `cargo test -p dstore-udiff`: 39 passed.
- `cargo test -p dstore-client-rs --test golden -- udiff::`: 5 passed.
- `cargo clippy -p dstore-udiff --all-targets -- -D warnings`: clean.
- `cargo clippy -p dstore-client-rs --test golden -- -D warnings`: no warnings.
- `rustfmt --check --edition 2024` over the crate and `tests/golden_tests/udiff.rs`: clean.
- In `tools/vectorgen`: `go vet .` is clean, and `gofmt -l family_udiff.go` prints nothing.

### Still open (not owned here)

- Merge `tools/vectorgen/docs/udiff.md` into the Families section of `VECTORS.md` (its owner).
