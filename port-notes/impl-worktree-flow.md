# impl-worktree-flow: layer L5 notes

Owner: worktree-flow. Files:
- `crates/worktree/src/flow.rs`: the flows of dstore v0.1.9 `worktree/flow.go`.
- `tests/fake_cluster_worktree.rs`: a new root integration target.
- `crates/worktree/src/lib.rs`: only the hand-off line. The L0 `#[allow(dead_code, unused_variables)]` on
  `mod flow` is removed.

PORTING.md §4.10 `flow.rs` is implemented: `Tree::fetch`, `Tree::pull`, `Tree::push`, `Tree::refresh_ticket`,
`clone`, `init`. No public signature changed.

## Additions beyond PORTING.md §4.10 (nothing changed)

- `#[derive(Clone, Debug)]` on `FetchResult` and `PushResult`, and `#[derive(Clone, Debug, Default)]` on
  `PullResult`. `Default` for `FetchResult` and `PushResult` is written by hand, because core-rs `Key` has no
  `Default`; it gives Go's zero value (key all zero).
- Crate-private helpers in `flow.rs`:
  - `Tree::fetch_state`, Go's unexported `fetch`: it updates the state in memory only and returns the result
    filled as far as it got.
  - `Tree::fetch_saved`, Go's `Fetch`, keeping the partial result on error for `Pull`'s `r.Fetch`.
  - `os_stat`: Go `os.Stat`, giving `stat <path>: <errno>` with EINVAL for a NUL byte.
  - `prepare_clone`, `clone_fill`, `blocking`, `getter`.

## Decisions

- **Blocking work.**
  - These run in `tokio::task::spawn_blocking` holding `Arc<packstore::Store>` (PORTING §5.1):
    - the scan, the tree diff and the merge (one task);
    - apply;
    - `ingest::dir`;
    - clone's stat/mkdir/readdir/`Tree::create`;
    - init's `Tree::create`;
    - the clean-up after a failed clone or init.
  - The `.dstore/config` and `.dstore/state` writes run inline, as Go does. They are two small files, and
    `&mut self` cannot move into a blocking task.
  - None of this observes ctx, so Ctrl+C never aborts apply (PORTING §5.4).
  - A panic on the blocking pool is resumed in the caller: it is a bug. A `JoinError` that is not a panic
    only happens while the runtime shuts down. It becomes `Error::Msg("worktree: blocking task: …")`, a
    Rust-only text for a case Go does not have.
- **Names.** `Config.name` goes to `ref_get` and `push` through `String::from_utf8_lossy`: CBOR text fields are
  `String` (PORTING DD-8). `reference.ValidateName` has already rejected invalid UTF-8 in the CLI.
- **`ExpectedVersion`.** `state.remote_version` `None` (Go nil) is sent as an empty `Cond.expected_version` with
  `versioned`. That is what Go's omitempty wire field carries for nil, so "the name must be new".
- **`RemoteVersion`.** It is set to `Some(…)` from `ref_get`, `PushStats.version` or `CasMismatch.version`.
  Go's nil vs empty there only shows in memory: `save_state` writes the hex of either.
- **The `ErrRefChanged` test is Go's `errors.As(err, &cm)`.** It is `dstore_client::Error::cas_mismatch()`,
  which walks the source chain. The wrapped text is `Error::RefChanged(err)` (`"%w (%v)"`) over the whole
  client error.
- **Clone clean-up** follows Go exactly:
  - `Create` failing after clone created `dir` → `os.RemoveAll(dir)`;
  - any later failure → close, then `RemoveAll(dir)` when created (only the leaf of a `MkdirAll` chain), else
    `Remove(dir)` (only `.dstore`, so applied files stay: PORTING §1.4);
  - clean-up errors are ignored.
  - The unknown-name error is `Error::UnknownRefName(cfg.name)` (`client: unknown reference: <name>`).
- **Init** returns `Tree::create`'s error untouched. After a failed fetch or state write it closes the tree and
  removes `dir/.dstore`.
- **`refresh_ticket`** compares `ticket_from_view(view).encode()` with the stored config bytes and writes
  `save_config` only when they differ. Without a view it does nothing.

## Tests (`tests/fake_cluster_worktree.rs`, over `dstore_testkit::fake`, no sockets)

- The three `node/worktree_test.go` tests, with Go's assertions plus a few more:
  - `worktree_init_push_clone_edit_pull`;
  - `worktree_conflict`, which also checks the exact `ErrRefChanged` text with the current key, that a
    refused push leaves the state alone, that a refused pull saved the fetch (on-disk `remote` moved, `base`
    kept), and that forced conflicts are appended after the merged changes;
  - `worktree_push_recovers_after_lost_state`, which also checks that the cluster's version is adopted in
    memory and on disk.
- Flow scenarios (worktree.md §2.9, §5, §6):
  - `fetch_of_an_unknown_name_and_the_up_to_date_fetch`:
    - `init` of an absent name, with the state file's empty `remote`/`remote_version`;
    - `pull` → `ErrNoRemote`, with `fetch` filled;
    - an empty directory → `nothing`, and the name stays absent;
    - a push creates it, and the next fetch is up to date;
    - after the packstore is deleted, a fetch still reports up to date and sends no `get`/`missing` (the
      v0.1.9 quirk), and `status` then fails.
  - `push_refusals_and_force`:
    - `ErrRemoteMoved` after a fetch, with the state unchanged;
    - `--force` replaces the reference (key, user and version checked);
    - after a forced `ref_delete` and a fetch, `ErrRemoteDeleted`;
    - `--force` recreates the reference although the tree equals base.
  - `push_ref_changed_texts`:
    - two copies init the same new name, and the second push gets `… (cas mismatch: current key <root>)`;
    - after the name is deleted, a synced copy's push gets exactly the golden
      `flow/push-ref-changed-absent`, and its `cli-line/` form.
  - `push_excludes_only_the_root_dstore`: `sub/.dstore` and a `.dstore-tmp-123` leftover are pushed, the root
    `.dstore` is not, and a clone reproduces them.
  - `clone_cleans_up_what_it_created`:
    - an unknown name into a new directory, a new nested `x/y` (only `y` removed) and an existing empty
      directory (only `.dstore` removed);
    - a successful clone, with the `config` bytes and a `state` whose base = remote = key and whose version
      is the cluster's, reopened with `Tree::open` and a clean status.
  - `init_of_an_existing_name_then_pull`:
    - base stays the empty tree, status shows everything new and the remote moved, push refuses;
    - pull merges an equal addition without conflict and keeps the local addition.
  - `init_with_a_failed_fetch_leaves_nothing_behind` (every node down).
  - `refresh_ticket_stores_the_view_ticket`: a stale ticket is replaced by the view's (three members, exact
    config bytes). An equal ticket leaves the file untouched, checked with a marker file.
- `errors/worktree_text.json`:
  - `ref_changed_client_and_cmd_texts`: every `flow/` text case, `cli-line/flow/push-ref-changed-absent`,
    `worktree/ErrRefChanged` and its `cli-line/` form (Rust only returns it wrapped; its source is the
    client error), `client/ErrUnknownRef` and its `cli-line/` form, and the `cmd/reference-name-*` and
    `cmd/push-user-*` cases through `dstore_client::validate_{name,user}_bytes`.
  - `flow_error_texts`: `flow/init-inside`, `flow/clone-not-a-directory`, `flow/clone-not-empty`,
    `flow/clone-parent-is-a-file`, `flow/clone-empty-directory-inside`, run through `init`/`clone` on real
    directories, with `{ROOT}` substituted as vectorgen does.
  - `cmd_error_texts_through_the_cli`: `cmd/clone-usage`, `cmd/init-usage`, `cmd/remote-and-incoming`,
    `cmd/resolve-ticket` and `cmd/reference-name-at` through `CARGO_BIN_EXE_dstore`, with exit 1 and no file
    created. These literals live in cli-wc's `dstore_cli::cmd_wc`. The test passes against the `cmd_wc` that
    cli-wc landed, so it is not ignored.

## For other owners

- **ci-nix-docs:** the new root test target is `--test fake_cluster_worktree`. Add it to the flake's
  `checks.tests`.
- **cli-wc:**
  - The flows take `jobs: usize`; map a negative `--jobs` to 0.
  - `pull` returns `(PullResult, Result)`: print `conflicts` on `err.is_conflict()`.
  - `clone`/`init` need `cfg.ticket` already replaced by `ticket_from_view(cl.view()).encode()`, as wc.go does.
- The header of `tests/fake_cluster.rs` mentions `worktree_test.go`. Those ports live in
  `tests/fake_cluster_worktree.rs`.

## Verification (macOS arm64, 2026-09-18)

- **Snapshot.** The checkout was copied to the scratchpad with the siblings' mid-edit files (`crates/cli/*`,
  `gocompat/src/os.rs`) and `Cargo.lock` restored to HEAD, so this code built against committed siblings.
  With `--locked`:
  - `cargo test --test fake_cluster_worktree`: 13 passed, 1 ignored (the CLI test, whose sibling was
    unfinished then). The binary was rerun 8 more times: 13 passed each time.
  - `cargo test -p dstore-worktree --lib`: 45 passed.
  - `cargo test --test golden -- worktree::`: 20 passed.
  - `cargo clippy -p dstore-worktree -p dstore-client-rs --all-targets -- -D warnings`: clean.
- **Real checkout**, after the siblings updated `Cargo.lock`, with `--locked`:
  - `cargo test --test fake_cluster_worktree -- --include-ignored`: 14 passed, including the CLI test
    against the landed `cmd_wc`.
  - `cargo clippy -p dstore-worktree --all-targets -- -D warnings`: clean.
  - Clippy over the root package stops on cli-wc's `manual_async_fn` lints in `crates/cli/src/cmd_wc.rs`.
    That file is not owned here.
- **Formatting.** `rustfmt --check --edition 2024` over the owned files: clean.
- **Not verified:** Linux. Nothing here is Linux-specific beyond the worktree-offline code, which is left to CI.

## Review

Reviewer: review-worktree-flow (2026-09-18). `flow.rs` was checked line by line against Go v0.1.9
`worktree/flow.go`, worktree.md §2.9 and its Addenda, the working-copy design note, and PORTING §1.4,
§4.10 and §5.1-§5.4.

**Behaviour: no defect found.** Checked in particular:
- **fetch:**
  - `ErrUnknownRef` is tested before other errors.
  - The key is parsed before `exists` is set.
  - No `PullTree` runs when `remote == k`, but the version is still refreshed.
  - `Fetch` saves only after a successful fetch.
- **pull:**
  - `r.fetch` is kept on a fetch or save error.
  - `NoRemote` is checked before up-to-date.
  - The scan runs before the tree diff.
  - With `force`, the conflicts' incoming changes are appended after the merged ones.
  - On a failed apply, `applied` stays empty and neither base nor `synced_at` moves.
- **push:**
  - The `synced` rule, and the `RemoteDeleted`/`RemoteMoved` choice.
  - `nothing` only when `root == base && synced`, force included.
  - `Cond` is `force` alone, or `versioned` with the stored version. A nil and an empty version are the same on the wire: `wire.Msg` field 14 is omitempty.
  - The recovery test is exactly `!cm.HasCurrent || perr != nil || cur != root`.
  - The `"%w (%v)"` text.
  - State is untouched on a refusal and on any client error; a recovered push keeps zero `stats`.
- **clone:**
  - Check order: `Stat` follows symlinks; only ENOENT leads to `MkdirAll`; then "not a directory", `ReadDir` open errors, "not empty".
  - A `Create` failure removes the directory only when clone created it.
  - `fail` closes the tree, then runs `RemoveAll(dir)` or `Remove(dir)`.
  - The unknown-name text.
- **init:** `Create`'s error is returned without clean-up; `.dstore` is removed after a failed fetch or state write.
- **refresh_ticket:** compares the encoded view ticket with the stored bytes, and saves the stored config.

**Fixed:**
- A dead `#[allow(clippy::too_many_arguments)]` on `Tree::push` was removed. The method has 7 inputs including `self`, which does not exceed clippy's threshold of 7. Clippy stays clean.
- **Test gap.** The two failed-apply quirks of PORTING §1.4 were claimed but not tested. Added to `tests/fake_cluster_worktree.rs`:
  - `pull_with_a_failed_apply_keeps_base`:
    - B turns `sub` into a file while A modifies `sub/deep.txt` and pushes.
    - B's forced pull conflicts (local Deleted, incoming Modified), and apply fails with `<root>/sub: not a directory`.
    - `applied` is empty; base and `synced_at` stay in memory and on disk; remote moves and is saved.
    - A retry fails the same way with an up-to-date fetch.
  - `a_failed_apply_in_clone_leaves_files_in_an_existing_directory`:
    - A hand-built tree (file `a.txt`, then `z` of type 0160000) is pushed with `Cluster::push`.
    - Clone into an existing empty directory fails with `z: unsupported type 0160000` and leaves `a.txt`; only `.dstore` is removed.
    - Clone into a new directory removes that directory.

**Accepted as documented (no change):**
- The `.dstore/config` and `.dstore/state` writes run inline. `save_state`/`save_config` take `&self`, and `refresh_ticket` is sync by §4.10; `block_in_place` would panic on a current-thread runtime.
- The Rust-only `JoinError` text.
- `clone` and `init` drop Go's partial `FetchResult` on error. The signature is fixed by §4.10, and wc.go prints nothing from it on error.

**Hand-off (3):** verified.
- `mod flow` carries no allow.
- All 27 `flow/`, `cmd/`, `client/` and `ErrRefChanged` cases of `errors/worktree_text.json` are asserted, including the `cli-line/` forms.
- No test in the target is ignored.
- `node/worktree_test.go`'s three tests are ported with all of Go's assertions.

**Verification** (macOS arm64, real checkout with the siblings' uncommitted L5 files, all with `--locked`):
- `cargo test --test fake_cluster_worktree -- --include-ignored`, `TZ=UTC`: 16 passed, 0 ignored. That is the 14 existing tests plus the 2 added here.
- `cargo test --workspace`, `TZ=UTC`: 41 test binaries; 962 passed, 0 failed, 4 ignored. The 4 ignored are the L5 CLI snapshot groups owned by others and `live_go_node_view_call`.
- `cargo clippy -p dstore-worktree --all-targets -- -D warnings`: clean.
- `cargo clippy -p dstore-client-rs --test fake_cluster_worktree -- -D warnings`: clean.
- `rustfmt --check --edition 2024` over `flow.rs`, `lib.rs` and `fake_cluster_worktree.rs`: clean.
- The reviewer's target dir and logs are deleted; no temp dirs or processes remain.

**Still open (not owned here):**
- ci-nix-docs must add `--test fake_cluster_worktree` to the flake's `checks.tests`; `flake.nix` has no entry yet.
- Linux is unverified.
