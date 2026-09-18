# impl-cli-client: the client command actions (layer L5)

Owner: cli-client. Files: `crates/cli/src/cmd_client.rs` (the actions and their unit tests), `tests/cli_client.rs`
(new), and this file. No manifest, no PORTING.md §4 signature and no file of another owner was changed.

A port of dstore v0.1.9 `cmd/dstore/client.go`: `store push PATH NAME`, `store pull NAME`, `refs [PREFIX]`,
`watch PATTERN`, `ref get NAME`, `ref delete NAME`, `ls NAME [PATH]` and `cat NAME PATH`.

## What landed

- **Order of every action, as Go's.** Argument checks; `common::signal_ctx()`; the local store
  (`common::open_local`); the build; the dial; the command; the output. Every path that dialed closes its
  `Session` (`cluster.close()`, then the endpoint bounded to 3 s, DD-11), including the error paths.
- **`store push`.**
  - The steps, in order:
    1. `NArg != 2` → `push PATH NAME`; then `dstore_client::validate_name_bytes` over the raw argument.
    2. `signal_ctx`, then `open_local`.
    3. `ingest::dir` in `spawn_blocking`. `--jobs` values below 1 become 0 (= every core, core-rs-gaps G13).
    4. stderr `built <16 hex>: <stored> new objects`.
    5. The condition: `--force`, else must-be-new or `hexDecode(--expected-version)`, whose error comes here,
       before the dial.
    6. `progress::run_transfer("push NAME")`, whose body dials, pushes and closes.
  - A CAS mismatch gets ` (pull first, or --force)` (see decision 3). Then comes the local record
    `Reference{NAME, root, --user, now}`, whose encoding and put errors are ignored, and stdout
    `pushed %s: %d objects, %d uploaded, version %x`.
- **`store pull`.**
  - The steps: an empty NAME → `pull NAME`; `signal_ctx`; `open_local`; `run_transfer("pull NAME")`, which dials,
    runs `Cluster::pull` and closes.
  - Then `refs.put(NAME, record)`, whose error is returned, and stdout
    `pulled %s: root <64 hex>, %d objects fetched (%d bytes)`.
- **`refs`, `watch`, `ref get`, `ref delete`.**
  - Each is `common::dial_cluster(ctx, c, &common::logger(c))`, then the `*_with` function over the cluster.
  - The line formats are Go's, with times from `format_rfc3339` in `SystemZone`. `watch` checks
    `PATTERN != ""` before `signal_ctx`, skips `synced` events, prints `NAME\tdeleted` for deletions, and returns
    `Ok` when the stream ends on cancellation (exit 0).
  - `ref delete`: `--expected-version` is decoded after dialing, as in Go; without it the delete is
    unconditional. A CAS failure prints the raw `cas mismatch: …`.
- **`ls` and `cat`.**
  - `RefGet`, then `key.Parse` of the reference key. The fstree walk runs in `spawn_blocking` (PORTING.md §5.1).
    Its getter blocks on `common::cluster_get` through `Handle::block_on`, so core-rs error texts are kept.
    Errors are rendered by `corefmt::walk_error_text`.
  - `ls`: the root for `""` and `"/"`, else `ResolvePath(strings.Trim(p, "/"))`; one raw entry name per line.
  - `cat`:
    - `NArg != 2` → `cat NAME PATH` before `signal_ctx`;
    - then `ResolveEntry(strings.Trim(PATH, "/"))`; a content key that is not 32 bytes long gives
      `not a regular file with content`;
    - `fstree::write_content` streams to stdout, which is flushed at the end or on error.
    - A nil entry (the path names the root: `/`, `""`, `.`, `//`) closes the session, then calls
      `go_panic_exit("runtime error: invalid memory address or nil pointer dereference")` (DD-7, exit 2).
- **Output.**
  - Every Go `Printf` is one write plus a flush (PORTING.md §5.9); stdout errors are ignored, as Go's are.
  - `cat` writes through `GoStdout`: unbuffered like Go's `os.Stdout`, with its write errors rendered as the
    `*PathError` Go returns, `write /dev/stdout: <errno>` (EPIPE never gets that far: SIGPIPE is `SIG_DFL`).

## Decisions and deviations

1. **`DialArgs` restates `dialClusterLog`'s ticket source for the transfer bodies.**
   - The `run_transfer` body is `'static`, and `common::dial_cluster` takes the borrowed `Context`.
     `common::ticket_of` is private.
   - So `store push`/`pull` read `--ticket`, `--store` and `net_opts_of(c)` before `run_transfer`. Inside the
     body they resolve the ticket in Go's order (`ticket::parse`, else `nodeside::local_ticket`, else
     `common::NO_CLUSTER`) and call `common::dial_ticket` with the transfer's logger.
   - Reading the flags has no side effects, so the observable order is Go's. In particular the ticket is parsed
     inside the transfer, so in TUI mode its error is drawn as `failed: …`, as in Go.
2. **Byte-exact `ResolvePath`/`ResolveEntry`.**
   - core-rs `fstree::resolve_path`/`resolve_entry` take `&str`, so a non-UTF-8 path component could never match
     its entry.
   - `cmd_client` ports both Go functions line by line over the path's bytes, using the public
     `fstree::lookup_entry` (same variants and texts).
   - Their errors are `CliError`s with Go's texts: `corefmt::walk_error_text` for the fstree errors, and the
     `".." is not supported` error formatted locally with `gocompat::quote` over the raw path, because core-rs
     `WalkError::DotDot` holds a `String` (review finding 2).
3. **The CAS hint.**
   - Go adds ` (pull first, or --force)` after `runTransfer` returns, when `errors.As(err, &CASMismatch)`. The
     body returns a `CliError`, whose type is lost, so the body records the mismatch text in a `CasSeen`.
   - The hint is added only when `run_transfer` returns that same text. A TUI run error, which Go returns instead
     of the transfer's error, therefore gets none. The TUI's `failed:` line shows the error without the hint, as
     Go's does.
4. **A non-UTF-8 `--user` in `store push` (Go's order kept; changed by the review, finding 1).**
   - Go uploads the tree, then `Push` fails at `rec.Encode()` with `reference user: <ValidateUser text>`.
   - The Rust `Cluster::push` takes `&str`, and a lossy user would write a reference with U+FFFD instead.
   - The body therefore pushes with the stand-in user `"\u{1}"` (`REJECTED_USER`). core-rs `validate_user`
     rejects it when the record is encoded, after the upload, at the point where Go's `Encode` fails. That
     `Error::Reference` is the only one `Cluster::push` can return. It is given Go's text:
     `reference user: ` + `validate_user_bytes` over the raw bytes, so a user over 1024 bytes reads
     `user exceeds 1024 bytes`.
   - The upload, its log and progress lines, stdout, stderr and the exit status are all Go's.
   - A valid UTF-8 user that `ValidateUser` rejects (control characters) goes through `Cluster::push` as it
     is. It fails at the same point as Go, with the same text.
   - `record_local` skips a non-UTF-8 user, as Go's `Encode` fails on it. It is never reached with an invalid
     user, because the push fails first.
5. **Lossy names (DD-8).** `ref get/delete NAME`, `ls/cat NAME`, `watch PATTERN` and `store pull NAME` reach
   the client as `String`. `refs PREFIX` is passed as bytes. Stdout prints names, users and entry names as
   raw bytes.
6. **Reuse of cli-common's helpers:** `NO_CLUSTER`, `cluster_get`, `record_payload` (through `cluster_get`),
   `hex_decode`, `open_local`, `signal_ctx`, `logger`, `dial_cluster`, `dial_ticket` and `Session`.
   `cmd_client` defines no second version of any of them. PORTING.md §6 lists `open_local` and `cluster_get`
   under cli-client, but cli-common owns `common.rs` in L5 and landed both.

## Tests

- **Unit tests** (`cargo test -p dstore-cli --lib -- cmd_client`), 18 in total.
  - Output helpers:
    - `ref_line` and `deleted_line`: RFC3339 in three zones, the flooring of negative times, raw bytes;
    - `ref_get_text`, including an empty version (`version ` with its trailing space);
    - `built_line`, `pushed_line` and `pulled_line` (the full 64-hex root);
    - `trim_slashes` (`strings.Trim`), `jobs_of`, `push_cond` and `delete_cond` (bad hex, the odd nibble,
      `--force` over a bad `--expected-version`);
    - `CasSeen` (the hint only for the noted error), the stdout `PathError` texts;
    - `DialArgs` resolving the ticket in Go's order (`no cluster`, `ticket: …`, `--ticket` over `--store`,
      the `local_ticket` text);
    - `record_local` (the record, and its skip for invalid users), and `ingest_tree` error texts after the
      errno rewrite.
  - Over `dstore_testkit::fake` (three nodes on the in-memory transport; no sockets):
    - `store push` then `store pull`: counts, the local reference, the CAS hint on a second must-be-new
      push, `--expected-version` with the printed version, `--force`, and a pull into a fresh store whose
      local reference equals the cluster record;
    - a non-UTF-8 or rejected `--user`;
    - `refs` with prefixes, `ref get` (including an unknown name), and `ref delete` (a wrong version, bad hex,
      the right version, the unconditional default);
    - `ls`: the root spellings, `strings.Trim` paths, not-a-directory, not-found including non-UTF-8 names,
      and `..`;
    - `cat`: files, the root as a nil entry, a symlink, a directory's `… is not a file-content object (type
      DirLeaf)`, and `..`;
    - the cluster getter on objects no node holds: `object <k> not found` and
      `fstree: reading <k>: object <k> not found`;
    - `watch`: existing references, then a new one, then a deletion, an unmatched name not printed, and
      `Ok` on cancellation.
- **`tests/cli_client.rs`** (binary level, no network), 7 tests.
  - Every expectation comes from the Go binary: `go build ./cmd/dstore` of `/Users/dragan/amber-store/dstore`
    (HEAD `368f2c7`, v0.1.9, plus the formatting-only `wc.go` change), built into the scratchpad and deleted
    afterwards.
  - The cases:
    - `store push` builds the tree before a bad `--expected-version`, a missing ticket, a bad ticket and an
      unchecked invalid `--user`. The tree is then in `L/packstore` and no local reference exists;
    - `--force` ignores a bad `--expected-version`; `abc` and a one-byte `\xff` decode to nothing, while
      `\xff\xfe` prints `bad hex "\xff\xfe"`;
    - `--jobs -3`; `src/`; a single file (`1 new objects`); a second build (`0 new objects`);
      `$AMBER_STORE`;
    - `stat missing: no such file or directory` and `fifo is neither a regular file nor a directory`, after
      the local store has been created;
    - a control character or invalid UTF-8 in NAME, and flags after the positionals: each fails before the
      local store exists;
    - `store pull` opens (creates) the local store before the ticket error or `no cluster`, and an empty NAME
      fails before.
- **Snapshots.** `cargo test -p dstore-client-rs --test cli_snapshots -- --include-ignored client_cases` passes.
  That group holds the 57 `client` cases of `cli/snapshots.json`, including the DD-2 substitutes. The review
  removed its `#[ignore]`, so it now runs by default.

## Results (macOS arm64, flake dev shell, own target dir)

- `cargo test -p dstore-cli --lib -- cmd_client`: 18 passed.
- `cargo test -p dstore-client-rs --test cli_client`: 7 passed.
- `cargo test -p dstore-client-rs --test cli_snapshots -- --include-ignored client_cases`: passed.
- `rustfmt --check --edition 2024` on both owned files: clean.
- clippy: at implementation time, `-D warnings` stopped on the sibling's `crates/cli/src/cmd_wc.rs`. At review
  time `cargo clippy -p dstore-cli -p dstore-client-rs --all-targets -- -D warnings` is clean.

## Remaining (for other owners)

- The flake's `checks.tests` should run `--test cli_client` (ci-nix-docs).
- PORTING.md DD-8 (orchestrator): its list of lossy arguments should also name `store pull NAME`. Go sends the
  invalid UTF-8 in the ref-get frame, and the node fails to decode it. Rust sends U+FFFD and, when that name
  exists, writes it into the local refstore.
- The interop suite is the only place that runs these end to end:
  - the DD-7 exit of `cat NAME /`, `watch` exiting 0 on SIGINT, and the SIGPIPE cases of cli.md Addenda 5;
  - the network paths and stdout formats of every command (cli.md §5.3).
  The snapshots stop before dialing, and the unit tests cover the nil-entry outcome.
- Done since the implementation: the `#[allow(dead_code, unused_variables)]` on `mod cmd_client` in
  `crates/cli/src/lib.rs` is gone (cli-common), and `client_cases` is un-ignored (this review).

## Review (review-cli-client)

Every action of `cmd_client.rs` was checked line by line against these Go sources:
- `cmd/dstore/client.go` at HEAD `368f2c7`;
- core v0.0.8 `fstree/collect.go`, `lookup.go`, `content.go` and `reference/reference.go`;
- dstore `client/tree.go` and `client/watch.go`;
- core-rs `fstree/read.rs` (`lookup_entry`, `collect_entries`, `write_content`) and `reference.rs`.

The check covered texts, trailing spaces, exit codes and validation order.

**Findings, fixed.**
1. **A non-UTF-8 `--user` failed before the upload** (the implementer's decision 4). Go uploads the tree, logs,
   and reports progress, and only then fails at `rec.Encode()`.
   - The push now runs with a stand-in user that core-rs rejects at the same `encode()`, and that error gets
     Go's text (decision 4 above).
   - The unit test `store_push_with_a_non_utf8_user_writes_no_reference` now also asserts that the root is
     stored on the fake cluster after the failure, and that no reference was written.
2. **The `".." is not supported` error of `ls` and `cat` rendered the path lossily.** It went through core-rs
   `WalkError::DotDot { path: String }`, so U+FFFD appeared where Go prints `\xff`.
   - `resolve_path`/`resolve_entry` now return `CliError` with Go's texts, and format this one with
     `gocompat::quote` over the raw path.
   - New assertions: `ls ../\xff` and `cat /../\xfe/`.
3. **`tests/cli_snapshots.rs` `client_cases` was still `#[ignore]`d.** Its dependencies (`cmd_client`,
   `common`) have landed. It is un-ignored and passes (57 cases).

**Checked, no change needed.**
- **`store push`.** The steps come in Go's order:
  - `NArg != 2`, then `ValidateName`, which core-rs checks in Go's order (empty, length, UTF-8, then `@` or a
    control character per rune);
  - `signalCtx`, `openLocal`, `ingest.Dir` (`--jobs` ≤ 0 → 0), and the `built <16 hex>: N new objects`
    stderr line;
  - the condition: `--force` skips the hex decode;
  - the dial inside `run_transfer`, where `DialArgs` resolves the ticket as `ticket_of` does and `--store` is
    undefined on these commands, as in Go.
  - The CAS hint is applied only to the transfer's own CAS text. The local record is written with a new
    `CreatedAt`, and its errors are ignored. Then `pushed %s: %d objects, %d uploaded, version %x`.
- **`store pull`.** The empty NAME is checked before `signalCtx`, then `openLocal`, `Cluster::pull`, and
  `refs.put`, whose error is returned. Then `pulled`, with the 64-hex root (core-rs `Key` `Display` is
  lowercase hex).
- **`refs`, `watch`, `ref get`, `ref delete`.**
  - The line formats and RFC3339 times in `SystemZone` are Go's.
  - `watch` skips `synced` events and returns `Ok` on cancellation: Go's `WatchRefs` returns without yielding
    once ctx ends.
  - `ref delete` decodes `--expected-version` after dialing. Without it, the delete is unconditional, and
    `--force` together with a version keeps both.
- **`ls` and `cat`.**
  - `strings.Trim(p, "/")`, and `ResolvePath`/`ResolveEntry` walk component by component over raw bytes.
    `CollectEntries` prints raw names.
  - `cat` checks `NArg` before `signalCtx`. A content key that is not 32 bytes long is
    `not a regular file with content`.
  - `WriteContent`'s `reading <k>: …` has no `fstree:` prefix (`WalkError::ContentRead`).
  - `cat NAME /` closes the session, then exits 2 (DD-7).
- **Output.** Go's unbuffered stdout is emulated by `GoStdout`, and `write /dev/stdout: <errno>` covers write
  errors.
- **Specs.** cli.md §6 and client-transfer §6 list no Go test for these actions. The cli.md §5.3 cases need a
  live cluster (interop).

**Gates** (macOS arm64, flake dev shell, own target dir, deleted afterwards):
- `cargo test -p dstore-cli --lib -- cmd_client`: 18 passed.
- `cargo test -p dstore-client-rs --test cli_client`: 7 passed.
- `TZ=UTC cargo test -p dstore-client-rs --test cli_snapshots`: 9 passed, none ignored (`client_cases`, and the
  siblings' `admin_cases` and `wc_cases`, all run by default).
- `cargo clippy -p dstore-cli -p dstore-client-rs --all-targets -- -D warnings`: clean.
- `rustfmt --check --edition 2024` on `cmd_client.rs`, `tests/cli_client.rs` and `tests/cli_snapshots.rs`: clean.
