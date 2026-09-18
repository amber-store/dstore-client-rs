# impl-worktree-offline: layer L2 notes

Owner: worktree-offline. Files: `crates/worktree/src/{lib,tree,change,scan,merge,apply,diff,sys,error}.rs` and
`tests/golden_tests/worktree.rs`. `flow.rs` belongs to worktree-flow (L5) and is untouched.

PORTING.md §4.10 is implemented except `flow.rs`. No public signature changed.

## Additions beyond PORTING.md §4.10 (nothing changed)

1. **`dstore_worktree::sys::mkfifo(path: &Path, mode: u32) -> io::Result<()>`** (x/sys/unix `Mkfifo`). The
   applier needs it, and the root golden tests need it for the `fifo` disk op: rustix 1.1.4 has no
   `mkfifoat` on Apple, and the root package has no `libc` dev-dependency.
2. **`#[derive(Debug)]` on `SourceError`.**
3. **`Error::Walk` Display goes through `dstore_client::corefmt::walk_error_text`.** §4.10 shows
   `#[error("{0}")]`; the variant's type is unchanged. PORTING §5.2 asks for fstree names to be re-quoted with
   Go `%q` where the error is produced, and the variant has no other place to do it. xattr decode errors are
   rendered with `corefmt::cbor_error_text` (`cborx:` prefix) into `Error::Wrapped`.
   - **Until client-a lands `corefmt`, formatting a walk error or hitting a corrupt inline xattr map panics
     (`todo!()`).** Nothing else calls `corefmt`. The golden checks that need it are in the `*_corefmt` tests,
     marked `#[ignore = "needs dstore_client::corefmt"]`: `diff_trees_corefmt`, `scan_corefmt`,
     `unified_tree_to_tree_corefmt`, `apply_error_texts_corefmt`. Un-ignore them when corefmt lands.
4. **Crate-internal seams for worktree-flow** (`pub(crate)`):
   - `Tree::open_raw`: Go `openRaw`, with `state.base` the empty tree and `synced_at` Go's zero time
     (`tree::GO_ZERO_TIME`, 0001-01-01).
   - `tree::open_store(dir)`: Go `packstore.Open(dir, WithSync(true))` with Go's texts (below).
   - `error::{wrap, io_error, prefixed, lossy, cbor_error}`: `fmt.Errorf` shapes over typed causes.
   - `change::{collect, expand, expand_children, join_path}`, and the `sys` helpers (`rename`, `symlink`,
     `mkdir`, `readlink`, `lstat`/`os_lstat`, `read_dir`, `read_file_partial`, `chmod`, `lchown`, `mknod`,
     `utimes_nano_at`, `close`, `octal_alt`).
5. **`lib.rs` puts `#[allow(dead_code, unused_variables)]` on `mod flow;`**, so the L0 stubs do not break
   `clippy -D warnings`. Remove it with the stubs.

## Decisions (Go behaviour kept, or where Rust differs)

- **Packstore open.** core-rs `Store::open_with` creates directories 0777 & ~umask, returns bare `io::Error`s
  and renders Rust errno texts. `open_store` therefore runs Go's first steps itself:
  - `gocompat::os::mkdir_all(dir, 0o755)`, wrapped `packstore: creating <dir>: <PathError>` (vector
    `tree/open-packstore-is-a-file`);
  - `File::open(dir)`, returned as `open <dir>: <errno>` (core-rs-gaps G6);
  - then core-rs. A `packstore::Error::Other` (the flock conflict) becomes `Error::Msg` rewritten by
    `gocompat::errno::rewrite_os_errors`. The Display is then Go's
    `packstore: <dir> is already open: resource temporarily unavailable`, and the typed source is dropped.
    Other packstore errors stay `Error::Packstore`.
- **`.amberignore` load errors.** core-rs `Matcher::root`/`descend` return a bare `io::Error`. On failure, the
  scan reads the file again with `gocompat::os::read_file` to get Go's `open …`/`read …` PathError
  (`read <root>/.amberignore: is a directory`). If that second read succeeds (a race), the error is reported
  as `open`.
- **Bare errnos** (`chmod: %w`, `set mtime: %w`, `%s: mkfifo: %w`, `%s: mknod: %w`, `xattr %q: %w`, the
  restore chmod, and the xattr read in the scan) are `Error::Wrapped` with `gocompat::errno::io_error_text`.
  `os.Symlink`/`os.Rename` failures render Go's `*LinkError` (`rename <old> <new>: <errno>`). `sys::rename`
  ports `os/file_unix.go` `rename`, including the EEXIST refusal for an existing directory.
- **Temp files.** `.dstore-tmp-*` comes from `gocompat::os::create_temp`. Write errors go through an adapter
  and read `write <tmp>: <errno>` inside `WalkError::Io`. Close errors read `close <tmp>: <errno>`.
- **`DiskSource`** keeps the bytes read before a read error (`sys::read_file_partial`, Go `os.ReadFile`), so
  the binary heuristic sees the same data Go does.
- **`os.ReadDir`**: `open <dir>` for the open, `readdir` (darwin) or `readdirent` (linux) for iteration, an
  entry that vanishes before its lstat fallback skipped, bytewise sort. A failing DT_UNKNOWN fallback reads
  `fstatat <dir>: <errno>` (Go `File.lstatat`). `IsDir` is `DirEntry::file_type`, so a symlink to a directory
  is not a directory.
- **Map iteration (PORTING DD-10).** The applier's directory-mode restore runs deepest path first
  (`BTreeMap` in reverse), so a read-only parent cannot block restoring its child. xattrs are set in sorted
  name order.
- **Inputs Go would crash on, which Scan/DiffTrees never produce:**
  - `merge`: a same-path row where either side has no `new` entry counts as a conflict (Go dereferences nil).
  - `apply`: a non-deletion without `new` fails with `<path>: <kind> change has no new entry`
    (Go panics).
- **`unified`/`stat` writes.** Header and stat lines ignore write errors (Go's `Fprintf`), and the writer is
  flushed after each change (worktree addenda 6). The udiff body's write error is returned with Go's errno
  text. The writer is anonymous, so Go's `write /dev/stdout: …` path prefix is not rendered.
- **`hash_file`.** It drains `ingest::objects`. A clean drain without a root is unreachable; it is reported as
  `ingest::Error::Stopped` instead of a panic.
- **Recursion.** `diff_trees` and the scan recurse per directory level, as Go does. Frames are small (the
  entry lists are on the heap), so a `spawn_blocking` thread's 2 MiB stack covers PATH_MAX-deep trees.
- **`Kind` Display**: the vectors' `Kind(6)`/`Kind(-1)` have no Rust `Kind` and are not tested.

## Tests

- **Unit tests (44, `cargo test -p dstore-worktree --lib`).** These port every `worktree/*_test.go` test except
  the flow tests:
  - `TestCreateOpenRoundTrip`, `TestFindOutsideWorkingCopy`, `TestRemove`;
  - `TestDiffTrees_EveryKind`, `TestDiffTrees_PrunesEqualSubtrees`, `TestCompare`;
  - all six `TestScan_*` (`TestScan_Xattr` returns early on ENOTSUP, as Go skips);
  - `TestMerge` (13 rows);
  - the four `TestApply_*`;
  - `TestUnified_TreeToTree`, `TestUnified_TreeToDisk`, `TestStat`;
  - plus the lock-before-state order, state decode texts, `status`, device and mode writes, and the `sys`
    formulas.
- **Golden tests (`tests/golden_tests/worktree.rs`).**
  - Config encode (`Tree::create` and `save_config` bytes, no `.tmp` left) and decode (through `Tree::open`:
    `ok` configs, and `working copy {ROOT}: bad config: …` texts).
  - State encode (`save_state` bytes) and decode (`Tree::open`, including `missing` and `directory` setups).
  - `empty_tree`.
  - DiffTrees with a counting getter over `objects.bin` (full entries, walk order, `get_calls`).
  - Scan disk scripts after `ingest::dir`.
  - Merge.
  - Unified/Stat tree→tree and tree→disk renders, including partial output before errors.
  - `Kind` Display and `type_name`.
  - `errors/worktree_text.json`: the `worktree/` sentinels with their `cli-line/` forms, and every `tree/`
    and `apply/` case.
  - Not covered here: `worktree/ErrRefChanged` (the variant carries a client error; worktree-flow), and the
    `flow/`, `cmd/`, `client/` cases (worktree-flow, cli-wc).
- The ported Go tests and the scan vectors run on real temporary directories under `$TMPDIR`. Like Go's,
  they assume no `.dstore` above `$TMPDIR`.

## Verification (macOS arm64, 2026-09-18)

- `cargo test -p dstore-worktree --lib`: 44 passed.
- `cargo test -p dstore-client-rs --test golden -- worktree::`: 14 passed, 4 ignored (the `*_corefmt` tests).
  While `corefmt` is still a stub, the non-ignored `diff_trees` and `scan` tests check walk errors through
  core-rs's own Display, which equals Go's text for these name-free errors.
- `cargo clippy -p dstore-worktree --all-targets -- -D warnings`: clean.
- `cargo clippy -p dstore-client-rs --test golden`: no finding in `tests/golden_tests/worktree.rs`. With
  `-D warnings` the run stops early on lint errors in `dstore-gocli`, a sibling crate still being edited.
- `rustfmt --check --edition 2024` on the owned files: clean.
- **Not verified:**
  - The `cfg(target_os = "linux")` code (xattr l* calls, `Major`/`Minor`/`Mkdev`, `readdirent`, ENODATA) was
    not compiled: the dev shell has no Linux std.
  - Linux byte identity of the scan and disk vectors is left to CI.

## Review

Reviewer: review-worktree-offline (adversarial review of worktree-offline's work, 2026-09-18).

Method:
- **Specs.** Re-read PORTING §0-3, §4.8 (`corefmt`), §4.10 and §5-7; worktree.md in full, including the addenda;
  core-rs-gaps §2.3-2.8, §4.3, G5, G9 and G11; verification §3.3; impl-gocompat-c, impl-udiff and
  impl-vectorgen-worktree; VECTORS.md family `worktree`.
- **Go and core-rs sources, line by line.**
  - dstore v0.1.9: `worktree/{tree,change,scan,merge,apply,diff,xattr,xattr_darwin,xattr_linux}.go`, and
    `flow.go` `Status`.
  - core v0.0.8: `packstore.Open`, `amberignore.load`, `ingest.statPath`.
  - go1.26.5 `os`: `rename`, `Mkdir`, `ReadDir`, `dir_darwin.go`, `dir_unix.go`, `statat_unix.go`,
    `File.wrapErr`.
  - core-rs: `amberignore`, `packstore::Store::open_with`, `fstree::{collect_entries, write_content, WalkError}`,
    `ingest::{objects, Error}`; xattr 1.6.1 `list_path`/`get_path`/`set_path`; std `sys/fs/unix.rs` readdir.
- **Tests against the generator.** The golden test's setups were compared with
  `tools/vectorgen/family_worktree.go`: `wtRunOps`, `wtApplyErrors` and the `tree/` cases.

### Confirmed correct (no change)

- **tree.**
  - Validation order of `openRaw` (Find → config read → Unmarshal → packstore open) and `Create` (Abs → Find →
    MkdirAll 0755 → config → packstore → Put, no state file).
  - `Open` takes the lock before the state.
  - `loadState`: only ENOENT is `ErrIncomplete`; hex before `key.Parse`; `remote_version` ignored without a
    remote; `synced_at` last.
  - `writeJSON` writes `<path>.tmp`, then Go's rename with its EEXIST refusal of a directory.
  - The `packstore.Open` texts: `packstore: creating …`, a raw `open <dir>`, the flock text.
  - `status` equals `flow.go` `Status`.
- **change.** `%#o` type names, SameContent by `a`'s type, the Compare order, the DiffTrees merge-join with
  `kx` parsed before `ky`, pruning.
- **scan.**
  - listDir before CollectEntries; the root `.dstore` by string equality; Descend before the base key is parsed.
  - The racy rule; the `entry` fields, including the sign-extended Darwin rdev; the xattr inline limit;
    `hashFile` through `ingest::objects`.
  - core-rs ingest errors carry Go's `op path:` prefix with Rust errno texts, which the CLI's top-level
    `rewrite_os_errors` renders (PORTING §5.2).
- **merge.** The `goneAbove` bound `i > 0`, the `SearchStrings` lower bound, the last duplicate winning, the
  rule order.
- **apply.**
  - Every path is checked first; pdqsort orders.
  - `rejectSymlinkComponents`: an ENOTDIR Lstat error further up is returned raw, not skipped, as in Go.
  - `writable` records the mode before chmod. The ignored errno sets are ENOENT/ENOTEMPTY/EEXIST for Remove and
    EEXIST|ENOTEMPTY for Mkdir.
  - Mkdev and mknod truncation per platform; chown → chmod → xattrs → utimensat; deferred directory metadata in
    descending path order; the `write <tmp>` and `close <tmp>` texts.
- **diff.** `sides`; mode lines for every kind; the header before any content read; the binary heuristic; the
  old side's error first; header only for equal bytes; Stat's `--`/`++` miscount; the totals line.
- **sys.**
  - Linux and Darwin Major/Minor/Mkdev. `linux_split_formulas` was re-derived by hand: `0x0001_2678_9ab3_45cd`.
  - NsecToTimespec.
  - The xattr calls use the right deref variants. A NUL in an xattr name gives EINVAL through rustix, as in Go.
- **Tests.**
  - Every `worktree/*_test.go` test is ported and asserts at least what Go asserts.
  - The golden setups equal the generator's: the ops, the 13 `tree/` cases and the 20 `apply/` cases.
  - The vectors' `mtime` ops only touch readable files and directories, so the test's `File::set_times` is
    equivalent to `os.Chtimes`.
- **Deviations.** Those listed at the top of this file were reviewed and accepted.

### Found and fixed

1. **`sys::read_dir` DT_UNKNOWN fallback text.** Go's `File.ReadDir` falls back to `File.lstatat`, which reports
   `fstatat <dir>: <errno>`: `wrapErr` names the directory. The port reported `lstat <dir>/<name>: …`. Now
   `fstatat <dir>`.
2. **Zero-inode directory entries.** Go's darwin and linux `readdir` skip `d_ino == 0` (deleted, not yet
   removed); std's `ReadDir` yields them. `sys::read_dir` now skips them. gocompat-c's `remove_all` got the
   same fix.
3. **The root's `.amberignore` path.** Go reads `filepath.Join(root, ".amberignore")`, which is cleaned
   lexically. core-rs `Matcher::root` joins without cleaning, so a root such as `a/..` went through the file
   system. `scan` now hands core-rs `path::clean(root)`; the `.dstore` string comparison keeps the raw root.
4. **Vectors not asserted.** Nothing checked `worktree/trees/trees.json` `objects`, `omitted`, `trees` or
   `xattrs` (worktree §5 item 13). Two golden tests were added:
   - `trees_inventory`: objects.bin holds exactly the listed objects with their sizes and none of the omitted
     ones, and every tree root matches;
   - `xattr_encodings`: `cbor::encode_xattrs`, the `<= ingest::DEFAULT_XATTR_INLINE_MAX` decision and the
     `fstree::encode_xattr_set` key, for all 7 cases.
5. **No disk test crossed the inline limit.** Unit test `scan::tests::xattr_inline_limit_matches_ingest` sizes
   `user.x` so that the file's whole xattr map encodes to exactly 256 and 257 bytes. The map includes attributes
   the file system adds itself (here `com.apple.provenance`, present on every fresh file). The test then:
   - checks a clean scan against ingest;
   - checks the entries' `xattrs_in` and `xattrs_key`.

### Still open

- **`dstore_client::corefmt` is still `todo!()`** (client-a has not landed it).
  - The four `*_corefmt` golden tests stay ignored.
  - Formatting any `Error::Walk` or cborx decode error panics until then.
- **Linux.** The Linux cfg code was reviewed by hand, not compiled (no Linux std in the dev shell). Linux byte
  identity of the disk vectors is also unchecked. Both are left to CI.
- **Texts left as they are (rare).**
  - An `fdopendir` failure reads `open` (Go on darwin: `fdopendir`).
  - core-rs packstore `load` and ingest errors keep Rust errno texts until the CLI's top-level rewrite.

### Gates (this review)

- **Real checkout.**
  - `cargo test -p dstore-worktree --lib`: 45 passed.
  - `cargo test -p dstore-client-rs --test golden -- worktree::`: 16 passed, 4 ignored (corefmt).
- **Clippy.** `cargo clippy -p dstore-worktree --all-targets -- -D warnings` and
  `cargo clippy -p dstore-client-rs --test golden -- -D warnings` are clean on a scratch snapshot of HEAD
  (fb12c3e) with the owned files overlaid. In the real checkout, both stop on a sibling's lint error in
  `crates/client/src/progress.rs`, which is not owned here.
- **Formatting.** `rustfmt --check --edition 2024` over the owned files is clean.
