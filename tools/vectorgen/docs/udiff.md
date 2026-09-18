# Family `udiff`

Owner: udiff (layer L1). Generator: `tools/vectorgen/family_udiff.go`, family `udiff`, owning
`udiff/udiff.json`, `udiff/lcs.json` and `udiff/pdqsort.json`. Producers: go-udiff v0.4.1 (`Unified`,
`ToUnified`, `Lines`, `lcs.DiffLines`, and the inputs of `difftest.TestCases`) and go1.26.5
`sort.Slice`. Spec: PORTING.md §4.9, port-notes/worktree.md §3.9 and §5 items 5-7.

Regenerate with `go run . ../../tests/golden udiff` in `tools/vectorgen`. The family takes about two
seconds and writes about 5 MB.

Rust tests: `tests/golden_tests/udiff.rs` (the `udiff` module of root `tests/golden.rs`). All three files
are read there, including `pdqsort.json`: `dstore_udiff::gosort` is public, and the crate has no
dev-dependencies. A missing file fails the test.

## Conventions of this family

On top of the root `VECTORS.md` conventions:

- **Text fields.** A byte string that is valid UTF-8 is a JSON string in its field (`old`, `out`, …).
  Otherwise the field is absent and its `_hex` twin (`old_hex`, `out_hex`, …) holds lowercase hex.
  Exactly one of the two is present.
- **Digests.** `*_fnv1a64` is FNV-1a 64 (Go `hash/fnv.New64a`) as a decimal string. A permutation digest
  (`order_fnv1a64`) is taken over the ids written as 8-byte little-endian integers.
- **Recipes.** Large inputs are described as splitmix64 recipes (below) that the Rust test replays. Every
  recipe case also carries the lengths and digests of the texts it generates. When those differ, the
  test reports a generator mismatch, not a udiff bug.

## `udiff/udiff.json`

```json
{
  "alphabets": [ { "name": "letters", "lines_hex": ["610a", …] } ],
  "cases": [ { "name", "old_label"|"old_label_hex", "new_label"|"new_label_hex",
               "old"|"old_hex", "new"|"new_hex",
               "edits": [ { "start": n, "end": n, "new"|"new_hex" } ],
               "out"|"out_hex" } ],
  "to_unified": [ { "name", "old_label", "new_label", "content"|"content_hex",
                    "edits": [ … ], "context_lines": n, "out"|"out_hex" | "error" } ],
  "random": [ { "name", "old_label", "new_label", "recipe": { … },
                "old_len": n, "old_fnv1a64": "…", "new_len": n, "new_fnv1a64": "…",
                "edits": n, "out_len": n, "out_fnv1a64": "…", "out"|"out_hex"? } ]
}
```

- **`alphabets`**: six tables of 41 distinct one-line strings, each ending in exactly one `\n`:
  - `letters`: `a\n` … `O\n`;
  - `words`: Go-like source lines;
  - `crlf`: letters ending in `\r\n`;
  - `edge`: invalid UTF-8 (`\xff`, truncated and overlong sequences, surrogates), NUL, lone `\r`, BOM,
    U+2028, lines that look like diff markers (`--x`, `++y`, `--- a`, `@@ -1 +1 @@`,
    `\ No newline at end of file`);
  - `prefix` and `suffix`: 280-byte lines that share a long common prefix or suffix.
- **`cases`**: hand-written inputs. `edits` is `udiff.Lines(old, new)` and `out` is
  `udiff.Unified(old_label, new_label, old, new)`. They include:
  - the probes of verification.md §3.4 and the samples of worktree.md §3.7;
  - every `difftest.TestCases` input as `Unified("from", "to", In, Out)`;
  - identical, empty-old and empty-new pairs;
  - a missing final newline on either or both sides, and insertions at EOF;
  - two changes 4-8 unchanged lines apart (the 6-line joining gap);
  - changes at the first and last lines;
  - CRLF and bare `\r`, and tie-breaking in repeated lines;
  - Unicode, invalid UTF-8 content and labels, diff-marker lines;
  - long common prefixes and suffixes, whole-file replacement;
  - two cases past the search limit of 50 (unrelated files, every other line changed);
  - many small hunks, empty labels, NUL bytes.
- **`to_unified`**: `udiff.ToUnified(old_label, new_label, content, edits, context_lines)` over
  explicit edits: `out` on success, `error` with Go's text (`diff has out-of-bounds edits`,
  `diff has overlapping edits`) on failure. They include:
  - the character-level `Edits` and the `LineEdits` of `difftest.TestCases`, exercising `lineEdits` and
    `expandEdit`;
  - context lines 0, 1, 2, 3, 4, 5, 10 and 100;
  - unsorted edits and insertions at one point (the stable `SortEdits`);
  - out-of-bounds and overlapping edits;
  - no edits;
  - merges of edits on one line, partial and EOF insertions, invalid UTF-8.
- **`random`**: at least 3000 recipe cases (3393). `edits` is `len(udiff.Lines(old, new))` and `out` is
  `udiff.Unified(old_label, new_label, old, new)` with `old_label = "a/" + name` and
  `new_label = "b/" + name`. The output is always given as `out_len` and `out_fnv1a64`, and also
  verbatim when it is at most 1024 bytes.

### Random recipes

```json
{ "seed": "…", "alphabet": i, "distinct": k, "lines": n, "mode": "independent"|"edits",
  "new_lines": n, "edits": e, "max_run": r, "strip_eol": "none"|"old"|"new"|"both" }
```

`generate(recipe)`, with `next()` the splitmix64 stream of VECTORS.md from `state = seed`:

1. `value()`: `v = next()`. When `alphabet >= 0`, `v = v % distinct` and the line is
   `alphabets[alphabet].lines_hex[v]`. When `alphabet == -1`, the line is `%016x\n` of `v` (lowercase).
2. The old values are `lines` calls of `value()`.
3. `independent`: the new values are `new_lines` calls of `value()`.
4. `edits`: start the new values as a copy of the old ones, then repeat `edits` times:
   1. `op = next() % 3` (0 insert, 1 delete, 2 replace);
   2. `pos = next() % (len + 1)`;
   3. `run = 1 + next() % max_run`;
   4. `del = min(run, len - pos)` for delete and replace, else 0;
   5. for insert and replace, `ins` is `run` calls of `value()`, else empty;
   6. replace `values[pos : pos+del]` with `ins`.
5. Each text is the concatenation of its lines. `strip_eol` `old`, `new` or `both` removes one final
   `\n` byte from that text, if present.

Classes (the name prefix before `/`):

| Class | Cases | Shape |
|---|---|---|
| `heavy` | 1500 | `letters`, 2-41 distinct, 50-450 independent lines each side: the evidence run of worktree.md §3.9 (search limit, `lcs.fix`) |
| `heavy_words` | 150 | the same over `words` |
| `edits` | 600 | any alphabet, 0-450 lines, 1-10 edit runs of 1-6 lines |
| `heavy_edits` | 500 | any alphabet, 50-450 lines, 20-150 runs of 1-12 lines |
| `small` | 600 | any alphabet, 1-6 distinct, 0-15 lines, independent (0-15 new lines) or 0-4 runs of 1-3 lines |
| `hex` | 40 | unique hex lines, 100-2000 lines, 1-40 runs of 1-5 lines |
| `huge` | 3 | 986895 hex lines (16777215 bytes) with 0, 3 and 8 runs of 1-3 lines |

`strip_eol` is not `none` in 10% of the `heavy`, `heavy_words`, `heavy_edits` and `hex` cases, in 20% of
the `edits` cases and in 40% of the `small` cases; it is then `old`, `new` or `both` with equal odds. The
three `huge` cases use `none`, `new` and `old`. The recipe always carries the value, so the replay needs no
percentages.

## `udiff/lcs.json`

```json
{
  "alphabets": [ … as in udiff.json … ],
  "strings": [ { "name", "a", "b", "diffs": [[start, end, repl_start, repl_end], …] } ],
  "random": [ { "name", "recipe": { … }, "old_lines": n, "new_lines": n, "diffs": [[…], …] } ]
}
```

- **`strings`**: `lcs.DiffLines` over the bytes of `a` and `b`, each byte one element. They include:
  - the `Btests` of `lcs/common_test.go` in both directions, with and without `TestIntOld`'s fills;
  - the strings of `TestSpecialOld`, `TestRegressionOld001`-`003` and `TestDiffAPI`;
  - empty sides;
  - two cases past the search limit.
- **`random`**: the first cases of the udiff random classes (150 `heavy`, 20 `heavy_words`, 80
  `edits`, 80 `heavy_edits`, 150 `small`, 10 `hex`), with the same names and recipes. `diffs` is
  `lcs.DiffLines(splitLines(old), splitLines(new))`. `splitLines` is go-udiff's, copied verbatim as
  `udSplitLines`: it splits after each `\n` and keeps a final partial line.

## `udiff/pdqsort.json`

```json
{ "cases": [ { "name", "n": n, "pattern", "modulus": m, "swaps": s, "seed": "…",
               "less": "len_desc"|"x_asc_len_desc"|"coin_1_2"|"coin_1_16"|"coin_15_16",
               "values": [v, …]?, "order": [id, …] | null, "order_fnv1a64": "…" } ] }
```

Each case is `sort.Slice` over `n` elements `{X, Len, ID}` with one of these comparators:

| `less` | `less(a, b)` |
|---|---|
| `len_desc` | `a.Len > b.Len` (go-udiff `lcs.fix`) |
| `x_asc_len_desc` | `a.X < b.X`, ties by `a.Len > b.Len` (go-udiff `lcs.sort`) |
| `coin_1_2`, `coin_1_16`, `coin_15_16` | `c = next()` from a separate splitmix64 stream starting at `seed`; true when `c % 2 == 0`, `c % 16 == 0` and `c % 16 != 0` respectively |

`order` lists the `ID`s in sorted order. Duplicate keys make the unstable order observable. Under a coin
comparator the order depends on the exact sequence of `less` calls. `coin_15_16` unbalances every
partition, which leads to `breakPatterns` and the heapsort fallback. `order` is `null` when `n > 256`;
`order_fnv1a64` is always given.

### Sort recipes

With `next()` from `state = seed`, values `v[i]` for `i` in `0..n`:

| `pattern` | `v[i]` |
|---|---|
| `random` | `next() % modulus` |
| `asc`, `asc_swaps` | `i / modulus` |
| `desc`, `desc_swaps` | `(n-1-i) / modulus` |
| `equal` | `0` |
| `saw` | `i % modulus` |
| `organ` | `min(i, n-1-i) / modulus` |
| `adversary` | `values[i]` (`modulus` and `swaps` are 0) |

For `*_swaps` with `n > 0`, repeat `swaps` times: `p = next() % n`, `q = next() % n`, swap `v[p]` and
`v[q]`. Then for `i` in `0..n`:

- `x_asc_len_desc`: element `{X: v[i], Len: next() % 3, ID: i}`;
- every other comparator: element `{X: 0, Len: v[i], ID: i}`.

Sizes run over 0-17, 20, 24, 31-33, 40, 48-52, 63-65, 99-101, 127-129, 150, 200, 255-257, 400, 500,
511-513, 1000, 1023-1025, 2048 and 4096. This covers the insertion-sort bound (12), median of three
(8), Tukey's ninther and the partial insertion sort (50), and power-of-two boundaries for
`breakPatterns`. Every size gets 10 shapes, and sizes up to 64 get 9 more, each under `len_desc` and
`x_asc_len_desc`.

Sizes 64, 129, 500, 1000 and 2048 also get the `adversary` shape. Its `values` are the values that
M. Douglas McIlroy's antiquicksort adversary (go1.26.5 `sort_test.go` `adversaryTestingData`) freezes
while `sort.Sort` runs, as `n-1-value` for `len_desc`. Sorting them repeats the adversary's comparisons
exactly. go1.26.5's pdqsort never reaches heapsort on this input.

Finally, sizes 13, 49, 50, 64, 100, 257, 500, 1000 and 4096 get the pattern `equal` under each coin
comparator, with two seeds each.

## What the Rust tests assert

- `unified_cases`: `dstore_udiff::lines(old, new) == edits` and `dstore_udiff::unified(...) == out`.
- `to_unified_cases`: `dstore_udiff::to_unified(...)` equals `Ok(out)` or `Err(error)`.
- `unified_random`: there are at least 3000 cases, and the generated texts match `old_len`/`old_fnv1a64`
  and `new_len`/`new_fnv1a64`. Then `lines` gives `edits` edits, and `unified` matches
  `out_len`/`out_fnv1a64` (and `out` when present).
- `lcs_diff_lines`: `dstore_udiff::lcs::diff_lines` equals `diffs`; for recipes the line counts match
  first.
- `pdqsort_slice`: `dstore_udiff::gosort::slice` with the case's comparator gives `order` (when present)
  and `order_fnv1a64`.
