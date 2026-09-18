# impl-vectorgen-worktree: notes

Owner: vectorgen-worktree (layer L1, Go only). Files: `tools/vectorgen/family_worktree.go` (family
`worktree`), `tools/vectorgen/cmd/mktree/main.go`, `tools/vectorgen/cmd/treekey/main.go`,
`tools/vectorgen/docs/vectorgen-worktree.md` (every schema), and the generated `tests/golden/worktree/`
and `tests/golden/errors/worktree_text.json`.

## Decisions and deviations

1. **File names.**
   - `describe_change`, `type_name`, `Kind.String()`, `resolve_ticket`, `filter_paths`, the working-copy printf
     texts and `ticket_from_view` go to `worktree/cli.json`, not `text/formats.json`, which another family
     owns.
   - The working-copy error texts go to `errors/worktree_text.json`, not `errors/text.json` (PORTING §7 lists
     both under one file).
   - The Scan decisions sit in `worktree/diff_trees.json` under `scan`, next to `diff_trees`.
2. **Verbatim copies without `mainpkg_copy.go`.** `describeChange`, `resolveTicket` and `filterPaths` are
   copied into `family_worktree.go` under `wt` names, so they cannot collide with another family's copies.
   The self-check embeds the file (`//go:embed`), prints each copy renamed to the original with
   `go/printer`, and compares it with `cmd/dstore/wc.go` from the module cache. It also requires every format
   string used for printf and error vectors to be a string literal of `wc.go` or `worktree/flow.go`.
3. **`{ROOT}` placeholder** in texts that embed a temporary directory (Open/Create/Clone/Apply errors, Scan
   and DiskSource errors, `filter_paths`).
4. **Scan vectors are disk scripts** (`setup`, `edit` ops). They record only path, kind and modes, with
   symlink permission bits masked to `S_IFLNK`: ownership, mtimes and base keys depend on the machine and
   the clock, and symlink modes differ between macOS (0755) and Linux (0777). Rust tests ingest the base
   with core-rs `ingest::dir`.
5. **Item 12** (apply reproduces trees) is not a vector: the re-ingested key depends on the runner's uid and
   gid. It remains a Go-test port. **Item 16** is interop. **Items 5-7** (go-udiff, pdqsort, lcs) are
   outside my owned paths (`udiff/`), so they are not generated here.
6. **Spec text against code** (the vectors follow the code):
   - verification.md §5 writes `a.txt (file → directory)`; `describeChange` gives `a.txt/ (file → directory)`,
     because the new side is a directory.
   - verification.md §5 describes a 5 MiB splitmix `big` changed at 4 MiB as oversized. A 5 MiB file is not
     over `MaxDiffBytes`; the `big-a`/`big-b` fixtures use 16777216- and 16777217-byte files instead.
   - Go's `time.Parse(RFC3339Nano)` rejects lower-case `t`/`z`, accepts a `+24:00` offset and accepts a comma
     fraction (`state.json` decode cases).
7. **objects.bin** is about 2.2 MiB: the 16-MiB fixtures repeat a 64-byte line, so only a few 1-MiB chunks
   are unique, and they compress to kilobytes in git.
8. **Determinism.** Two runs give identical bytes (verified on macOS; not yet run on Linux). The generator never
   outputs Go maps and never reads the clock or the user. It chdirs only inside `filter_paths`, and restores
   the working directory.
9. **`Tree.Get` semantics in the tree getter.** Absent objects return `packstore.ErrNotFound`, so missing
   object texts read `… packstore: object not found`, as over a real local packstore.
10. **Generated from an isolated copy.** While I worked, the shared `tools/vectorgen` package did not compile
    because sibling families were mid-edit (`family_wire.go`: undefined `tkTicket`; earlier `family_client.go`,
    `client`). I ran `go vet`, `go test` (the registry tests) and the generator from a scratch module holding
    byte-identical copies of `go.mod`, `go.sum`, `main.go`, `util.go`, `deps.go`, `main_test.go` and
    `family_worktree.go`, and wrote `tests/golden/worktree` and `tests/golden/errors/worktree_text.json` from
    there. Regenerate from the real module once it builds, and check that nothing changes.

## Rust consumers (for the owners)

- worktree-offline: `config.json`, `state.json`, `trees/`, `diff_trees.json`, `merge.json`, `unified.json`,
  `cli.json` `kind_string`/`type_name`, and the `tree/`, `apply/` and `worktree/` cases of
  `errors/worktree_text.json`.
- worktree-flow: the `flow/` error cases.
- cli-wc: `cli.json` `describe_change`, `status_row`, `resolve_ticket`, `filter_paths`, `printf`, and the `cmd/` and
  `cli-line/` error cases.
- view: `cli.json` `ticket_from_view` (also covered by `view/view_placement.json`).

`tests/golden_tests/worktree.rs` names `text/formats.json` `type_name` and `errors/text.json`. The owners
should read `worktree/cli.json` and `errors/worktree_text.json` instead; that file is not mine to edit.

## Review

Reviewer: review-vectorgen-worktree. I assumed the work was wrong and checked it against the sources and by
running it.

### What was checked

- **Specs.**
  - Read: the task; PORTING §0-3, §4.10, §5-7; worktree.md in full, including the addenda; verification.md
    §4.2-4.3, §5 (working-copy part) and the addenda.
- **Go sources, line by line.**
  - dstore v0.1.9: `worktree/{tree,change,scan,merge,apply,diff,flow}.go`, `cmd/dstore/wc.go`, the `main.go`
    error line, `client/refs.go` and `client/tree.go`.
  - core v0.0.8: `ingest/driver.go` and `parallel.go` (the in-memory builder matches `buildFile`, `buildEntry`
    and `buildDir`: the same default item chunker, nil byte options, one empty Blob for an empty file, and
    the 256-byte xattr inline limit), and `packstore.Get` (a bare `ErrNotFound`, as the fixture getter
    returns).
- **Go test expectations in the vectors.**
  - All 13 `TestMerge` rows give the test's apply and conflict paths.
  - `prunes-equal-subtrees` makes 2 getter calls.
  - The `every-kind` DiffTrees and Scan kinds satisfy `TestDiffTrees_EveryKind` and `TestScan_EveryKind`.
  - The racy window at exactly `synced_at - 2s` gives no change; 1 ns later gives `modified a.txt`.
- **Texts the generator builds itself** instead of taking them from a Go call. All three are correct:
  - `%w (%v)` over `*client.CASMismatch`: `RefPut` returns it unwrapped and `Push` returns that error as is.
  - `dstore: <err>\n`: `main.go` uses `fmt.Fprintln(os.Stderr, "dstore:", err)`.
  - `user: %w`: `pushUser`.
- **Self-check mutations.** An altered `wtResolveTicket` copy and a format literal that `wc.go` lacks each
  stop generation with a clear message.
- **Docs.** Every JSON key of the 8 files appears in `docs/vectorgen-worktree.md`.
- **Determinism.**
  - The real `tools/vectorgen` package still does not build: `family_wire.go` has an undefined `admRequest`,
    a sibling file mid-edit.
  - I used a scratch module with byte-identical `go.mod`, `go.sum`, `main.go`, `util.go`, `deps.go`,
    `main_test.go` and `family_worktree.go`.
  - Two runs gave identical output, identical to the committed `tests/golden` files (before and after my
    changes).
- **Gates.**
  - `gofmt -l`: clean.
  - `go vet`: passes for darwin and for `GOOS=linux` (the family, `cmd/mktree`, `cmd/treekey`).
  - `go test -count=1 .`: the registry tests pass.
- **Helpers.**
  - `mktree -seed 7 -small 25` twice gave trees with the same `treekey` key; `-no-ignore` changes the key.
  - A non-empty DIR exits 1 and a missing argument exits 2 with the usage.
  - `treekey` accepts a single file and reports a missing path.

### Findings, fixed

1. **Silent UTF-8 coercion.** Only fields routed through `wtText` had a `_hex` fallback. Error texts
   (`wtErrText`), the disk-case `changes`, `filter_paths`, printf arguments and docs were plain strings, which
   `encoding/json` would silently turn into U+FFFD, leaving a vector that no longer holds Go's bytes.
   - Added `wtCheckUTF8`, which walks each value before `writeJSON` (and `trees.json`); it also rejects maps.
   - No output byte changed. A mutation that puts `\xff` into a tree doc now stops generation.
2. **Environment dependency.** `Find` walks every ancestor of `$TMPDIR`. If a `.dstore` existed above the
   temporary directory, `tree/find-not-a-working-copy`, `tree/open-not-a-working-copy` and
   `tree/open-dstore-is-a-file` would have recorded another text without failing.
   - The generator now requires `ErrNotWorkingCopy` for those three cases and `ErrIncomplete` for
     `tree/open-incomplete`.
   - A mutation test fails as expected.
3. **Misnamed case.** The printf case `status-row-long-path` printed `  deleted   old/`, not a long path. It is
   now `status-row-deleted-directory`, the only byte change in `tests/golden/worktree/cli.json`.
4. **Docs.**
   - The DiffTrees list claimed "the diff, reverse and clone pairs of every fixture"; only `diff` and `special`
     have them. The list now names every case.
   - "Not generated here" now names family `udiff` (`family_udiff.go`) for worktree.md §5 items 5-7, and
     explains why `TestScan_Xattr` stays a Go-test port (tmpfs and APFS xattr support varies).
   - The UTF-8 and sentinel guards are documented.
5. **Notes order.** Items 9 and 10 above were out of order; they are reordered.

`tests/golden/worktree` and `tests/golden/errors/worktree_text.json` were regenerated through `main.go`, which
replaces only the family's owned paths. Two fresh runs match them byte for byte.

### Deviations accepted as reasonable

- **Files.**
  - `worktree/cli.json` replaces `text/formats.json` `describe_change`/`type_name`.
  - `errors/worktree_text.json` replaces `errors/text.json`; the client family now writes
    `errors/client_text.json`, so nobody writes `errors/text.json`.
  - PORTING §7 and VECTORS.md should name the new files when the docs are merged.
- **Schemas.** `diff_trees.json` has top-level `diff_trees` and `scan` instead of `cases`, and full entries
  instead of modes only. `merge.json` has `{path, kind}` objects instead of bare paths. Both are supersets of
  verification.md §4.3 items 16-17, and both are documented.
- **Fixture corrections.** The 16777216- and 16777217-byte fixtures replace verification.md's "5 MiB
  oversized" file, which is not over `MaxDiffBytes`. `a.txt/ (file → directory)` is the code's output.
- **Not generated.**
  - Item 12 (re-ingest keys): as a non-root runner, the key depends on its uid and gid.
  - `TestScan_Xattr`: see above.

### Not verified

- **The real module.** Regeneration from the real `tools/vectorgen` module waits for `family_wire.go` to
  build.
- **Linux byte identity.** It was not run; only cross-vetted. By analysis, the Linux errno texts used are the
  same (EISDIR, ENOTDIR, ENOENT, EWOULDBLOCK = EAGAIN), and the disk scripts set every mode explicitly and
  mask symlink bits. Filesystems that add default xattrs (SELinux labels) could show up as `meta` changes.
  The CI `vectors` job settles it.
