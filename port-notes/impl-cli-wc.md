# impl-cli-wc: layer L5 notes

Owner: cli-wc. Files:
- `crates/cli/src/cmd_wc.rs`: the working-copy commands of dstore v0.1.9 `cmd/dstore/wc.go` (HEAD);
- `tests/cli_wc.rs`: a new root integration target;
- this file.

## What landed

- **Actions** `clone`, `init`, `fetch`, `pull`, `push`, `status`, `diff`. They are called through the `act::*`
  wrappers of `app.rs`, which is unchanged.
- **Helpers**, all crate-private, one per Go function of wc.go:
  - `resolve_ticket`;
  - `wc_config`, with `wc_config_env` taking `getenv`;
  - `dial_config`, `open_wc`;
  - `push_user`, with `push_user_with` taking the OS-user lookup;
  - `describe_change`, `status_row`;
  - `filter_paths`, with `filter_paths_with` taking `filepath.Abs`.
- **Output.** One byte builder per `Printf`/`Println` format of wc.go (`cloned`, `init_exists`, …,
  `status_row_of`, `meta_only`), and `status_lines` for the whole `status` output.
- **`withCluster`** is `with_cluster` over a private `WcOp` trait (`FetchOp`, `PullOp`, `PushOp`). The trait
  returns `impl Future + Send`, which `run_transfer`'s spawned task requires.
- **No public signature changed.** PORTING.md §4.12 names the contents of `cmd_wc` only. Nothing was added to
  another module.

## Decisions

1. **Order of steps, as wc.go:**
   - **clone:**
     1. `First() == ""` → `clone NAME [DIR]`;
     2. `validate_name_bytes`;
     3. `dir` = `Get(1)`, else `path.Base(NAME)`;
     4. `wcConfig(nil)`;
     5. `signal_ctx`;
     6. `run_transfer("clone NAME")`: `ticket::parse`, `dial_ticket`, the view-derived ticket (always, also
        for an ids ticket), `worktree::clone`, then the session close (DD-11);
     7. print, then close the tree.
   - **init:** the same, with `init NAME`, and `os.Getwd()` after `wcConfig` and before `signal_ctx`. Extra
     arguments are ignored.
   - **fetch, pull, push (`withCluster`):**
     1. `openWC` (Getwd with the `$PWD` rule, then `Tree::open`, whose lock comes first);
     2. `wcConfig(&stored)`;
     3. `signal_ctx`;
     4. `run_transfer("<cmd> <stored name>")`: dial, the op, then `refresh_ticket` on success only;
     5. the session close.
   - **push** checks the user inside the op, after dialing (PORTING.md §1.4). The lookup order is `--user`
     (`c.String`, so `--user ""` is no flag), the **stored** config's user, then the OS user. The value of
     `--user` is read before the transfer; reading a flag has no side effect.
   - **status:** `openWC`, `Tree::status`, print.
   - **diff:**
     1. `--remote --incoming` fails before the working copy is opened;
     2. `openWC`;
     3. without a remote, `--incoming` and `--remote` fail with `ErrNoRemote`;
     4. `--incoming` diffs the trees; `--remote` scans with `GoTime::now()`; the default scans with
        `synced_at`;
     5. `filter_paths` when `NArg > 0`;
     6. `stat` or `unified` to stdout.
2. **Signals (PORTING.md §5.4).**
   - `status` and `diff` register nothing, so SIGINT kills them. `tests/cli_wc.rs` checks this.
   - The connection commands call `common::signal_ctx()` exactly where Go calls `signalCtx()`. Every earlier
     failure registers nothing: usage, name validation, no ticket, no working copy, a locked copy, Getwd.
3. **Blocking work (PORTING.md §5.1).**
   - `status` and `diff` run their whole body in one `spawn_blocking` task: open, scan or tree diff, filter,
     render.
   - `withCluster`'s `openWC` also runs in `spawn_blocking`.
   - `Tree::close`, `refresh_ticket` and the OS-user lookup run inline. They are tiny, and Go runs them inline.
4. **Output.**
   - Every stdout and stderr text is built as bytes. Names, directories and paths are raw.
   - Each line is one locked write and a flush. Write errors are ignored, as `fmt.Printf`'s are; a closed pipe
     kills by SIGPIPE (§5.9).
   - The `pulled:` line (three Go calls) and the conflict list (header plus rows) are one write each, with the
     same bytes.
   - `%-9s` pads a `String` (`Kind`'s `Display` writes with `write_str`, which ignores width).
5. **pull's conflicts.** Go's closure assigns `r`, and the action prints `r.Conflicts` when
   `errors.Is(err, ErrConflict)`.
   - Here `PullOp` fills an `Arc<Mutex<Option<Vec<Conflict>>>>` only when the pull's error `is_conflict()`.
   - The transfer closure still returns the error, so the TUI shows the failure as Go's `runTUI` does.
   - The conflicts are printed after `run_transfer` returns and before the error line. Only the pull itself can
     produce `ErrConflict`; `refresh_ticket` does not run after an error.
6. **The open working copy.**
   - It moves into the transfer closure, since `run_transfer` needs `'static + Send`.
   - On success it comes back and is closed after the output is printed, as Go's `defer tr.Close()`.
   - On error it is closed inside the closure. Go closes it after printing the conflicts; that ordering is
     unobservable.
7. **`--jobs`:** a negative value maps to 0 (= cores), which is Go's `<= 0` (PORTING.md §5.6).
8. **Relay.** The config's `relay` bytes reach `NetOpts.relay: String` lossily. Go hands the bytes to
   `url.Parse`. This only differs for a non-UTF-8 `--relay` or stored relay (pathological; the DD-8 kind).
9. **`$DSTORE_NO_DISCOVERY`.**
   - It is read only when there is no stored config (clone, init) and `--no-discovery` is not given.
   - A non-UTF-8 value fails `ParseBool` and is ignored, as in Go. So are `maybe`, `yes` and `""`.
10. **Inputs Go would crash on**, which the flows never produce:
    - `describe_change` of a type or mode change without both entries prints the path (with its slash). Go
      dereferences nil there; Scan and DiffTrees always give both entries.
    - `derived_ticket` without a view keeps the given ticket, where Go dereferences nil. `Cluster::dial` always
      sets the view.
11. **Error texts.** `… is outside the working copy` renders a non-UTF-8 argument lossily; PORTING.md DD-8
    lists this case. Every other error is the worktree, ticket, client or common error's `Display`, and the
    top-level errno rewrite happens in `dstore_cli::run`.
12. **Vector tests read JSON with a small test-only reader** (`cmd_wc::json`). The crate has no serde
    dependency, and a new external dev-dependency would have been a manifest change beyond this task.
    `CARGO_MANIFEST_DIR/../../tests/golden/<file>` is read with `read_to_string`, so a missing file fails
    the test.

## Tests

- **Unit tests** (`cargo test -p dstore-cli --lib -- cmd_wc`, 15):
  - `describe_change_and_status_row_vectors`: `worktree/cli.json` `describe_change` (20) and `status_row` (20),
    including `out_hex`.
  - `describe_change_text_vectors`: `cli/text.json` `describe_change`. Kind 9 is skipped, as VECTORS.md says.
  - `resolve_ticket_vectors`: both files, including the error.
  - `filter_paths_vectors`: `worktree/cli.json`, 18 cases, with `cwd` and `{ROOT}`.
  - `filter_paths_text_vectors`: `cli/text.json`, 20 cases.
  - `filter_paths_is_lexical`: a symlinked root, and the Getwd error returned raw.
  - `printf_vectors`: all 27 formats, matched by format string, so an unknown format fails the test; plus the
    composed `pulled:` line.
  - `cmd_error_texts`: the 13 `cmd/` cases of `errors/worktree_text.json`.
  - `push_user_precedence`.
  - `wc_config_without_a_stored_config`: `$DSTORE_TICKET`, the `ParseBool` table, the flag winning over the
    env, init, `--ticket`/`--user`/`--no-relay`.
  - `wc_config_with_a_stored_config`: the stored ticket beating the env, the env's `NO_DISCOVERY` ignored,
    `--ticket=` falling through, overrides to `""` and `false`, `--user ""`.
  - `jobs_flag`, `status_output` (the wc1 layout, moved counts, raw bytes), `describe_change_without_entries`,
    `key16_is_the_hex_prefix`.
  - The contexts come from `dstore_gocli::dispatch` over `crate::app()`, with the environment injected.
- **`tests/cli_wc.rs`** (6), through `CARGO_BIN_EXE_dstore`. No test opens a socket.
  - `stored_ticket_beats_the_environment`:
    - the stored ticket beats `$DSTORE_TICKET` for fetch, pull and push;
    - `--ticket` overrides for one run, and `--ticket=` falls through;
    - a subdirectory finds the same copy;
    - `.dstore/config` is byte-identical afterwards;
    - without a stored ticket the env counts, then `no cluster`.
  - `clone_and_init_take_the_environment`: the env ticket (also with `--ticket ""`), `DSTORE_NO_DISCOVERY=maybe`,
    and nothing created after a failed ticket.
  - `status_and_diff_die_by_sigint`: `status`, `diff` and `diff --stat` with 3000 new files, stdout unread;
    `kill -INT` ends each by signal 2.
  - `pwd_decides_the_working_copy_root`:
    - with `$PWD` naming the symlinked cwd, `diff <link>/a.txt` is inside;
    - without `$PWD`, with a `$PWD` naming another directory, or with a relative `$PWD`, it is outside.
  - `a_locked_working_copy_is_refused`: status, diff, fetch, pull and push while the test holds the packstore
    lock all give `packstore: … is already open: resource temporarily unavailable`.
  - `paths_are_printed_raw`: status and `diff --stat` over a `caf\xe9.txt`. Where the file system refuses
    non-UTF-8 names (APFS: EILSEQ), only the plain part runs.
- **`tests/cli_snapshots.rs` `wc_cases`** (62 Go cases; the test is not owned here): passes with `--ignored`.

## Not covered here

- **"push checks the user after dialing" end to end.** The test would have to dial, and only
  `tests/iroh_loopback.rs` may open sockets. The order is structural in `PushOp::run`: the user check runs
  after `dial_config`. The interop suite covers it.
- **The success lines of clone, init, fetch, pull and push against a cluster.** The printf vectors lock their
  bytes, and worktree-flow's fake-cluster tests cover the flows. The CLI dials iroh only, so the lines
  themselves are left to the interop suite.
- **The TUI path**: only a smoke probe under `script(1)` (below). Its terminal bytes are DD-6.
- **Linux** runs are left to CI. On Linux, `paths_are_printed_raw` also runs its non-UTF-8 part.

## For other owners

- **cli-app, or whoever owns `tests/cli_snapshots.rs`:** remove the `#[ignore]` on `wc_cases`. All 62 cases
  pass against this `cmd_wc`.
- **cli-common:** the `#[allow(dead_code, unused_variables)]` on `mod cmd_wc` in `crates/cli/src/lib.rs` can
  go. `cargo clippy -p dstore-cli --no-deps --all-targets -- --force-warn dead_code --force-warn
  unused_variables` reports nothing in `cmd_wc.rs`.
- **ci-nix-docs:** new root test target `--test cli_wc` for the flake's `checks.tests`. It spawns the binary
  and `kill`, which comes from coreutils on PATH.

## Verification (macOS arm64, `nix develop`, own target dir, `--locked`)

- `cargo test --workspace --locked` (TZ=UTC, siblings' L5 work in the checkout): 949 passed, 0 failed, 4 ignored
  (`admin_cases`, `client_cases`, `wc_cases`, `live_go_node_view_call`).
- `cargo test -p dstore-cli --lib`: 95 passed (15 in `cmd_wc`).
- `cargo test -p dstore-client-rs --test cli_wc`: 6 passed.
- `cargo test -p dstore-client-rs --test cli_snapshots -- --include-ignored wc_cases framework_cases`: 2 passed.
- `cargo test -p dstore-client-rs --test fake_cluster_worktree -- --include-ignored`: 14 passed, including
  worktree-flow's `cmd_error_texts_through_the_cli` against this binary.
- `cargo clippy -p dstore-cli --all-targets -- -D warnings`: clean.
- `cargo clippy -p dstore-client-rs --no-deps --test cli_wc -- -D warnings`: clean.
- `rustfmt --check --edition 2024` over the owned files: clean.
- **TUI smoke probe.** `script -q /dev/null dstore clone --ticket bogus trees/x` in a scratch directory,
  removed afterwards:
  - the inline renderer draws the `clone trees/x` frame, then `failed: ticket: "bogus" …`;
  - then `dstore: ticket: …`, with exit 1.
  
  With `--no-tui`, only the error line appears.

## Review

Reviewer: review-cli-wc. `cmd_wc.rs` was checked against `cmd/dstore/wc.go` at HEAD, one function at a time:
`resolveTicket`, `wcConfig`, `dialConfig`, `openWC`, `pushUser`, the seven actions, `withCluster`,
`describeChange` and `filterPaths`, plus `runTransfer`/`runTUI` in tui.go. For each I compared the step order,
where the signal ctx is created, every stdout and stderr byte, and which errors come back unchanged.

### Findings

1. **Fixed: pull listed conflicts after a non-conflict error.** Go prints the list only when
   `errors.Is(err, worktree.ErrConflict)` holds for the error that `withCluster` returns. When the
   Bubble Tea program cannot run, `runTUI` returns the program's own error instead of the transfer's.
   `PullOp` kept the conflicts whenever the pull itself hit `ErrConflict`, so Rust printed the list
   before a different `dstore:` line. The new `conflict_listing` prints only when the command's error is
   `ErrConflict`'s text: `CliError` carries no source chain, and only the pull produces that text. The test
   `conflicts_are_listed_only_with_the_conflict_error` covers it.
2. **Fixed: a vector case could be skipped silently.** `describe_change_text_vectors` skipped every
   `cli/text.json` case whose kind has no Rust variant. It now asserts that the only skip is the one
   kind-9 case that VECTORS.md names.
3. **Fixed, outside the owned files:** removed the `#[ignore]` on `wc_cases` in `tests/cli_snapshots.rs`, a
   one-line change like the siblings' change for `admin_cases` and `client_cases`. All 62 Go `wc` cases
   pass, along with the `validation` cases of clone, init and diff that the same test runs.
4. **Checked, no change needed:**
   - The validation order: clone and init check the name before `wcConfig`, and init calls Getwd before
     `signalCtx`. `withCluster` opens the working copy (lock first) before `wcConfig`. push checks the user
     after dialing, against the stored config. `diff --remote --incoming` fails before the working copy
     opens.
   - Output: every format string, `%-9s` padding, `%04o`, `%x` of an empty version, the three-call
     `pulled:` line, and conflicts written to stderr before the `dstore:` line.
   - `RefreshTicket` runs only on success and saves the stored config, not the flag overrides.
   - status and diff register no signal handler and flush per line. diff flushes per change, so partial
     output can precede an error.
   - Negative `--jobs` gives 0, which means all cores (Go `Jobs < 1` → GOMAXPROCS). clone does not read
     `--jobs`.
   - `$DSTORE_NO_DISCOVERY` is read only without a stored config, and ParseBool failures are ignored.
   - Go's `Unified` returns the error of the hunk write and ignores the errors of the header writes.
     `dstore_worktree::unified` does the same (not owned here).
5. **Accepted deviations** (from the implementer's list, re-checked): the lossy conversion of relay bytes;
   `describe_change`/`derived_ticket` on inputs Go would crash on, which the flows never produce; the working
   copy closed inside the transfer closure on error. Go's deferred `tr.Close()` inside `withCluster` also runs
   before the conflicts are printed, so the order matches.
6. **Already done by others:** cli-common removed the dead-code allows on `mod cmd_wc` in
   `crates/cli/src/lib.rs`.

### Still open

- **ci-nix-docs:** `--test cli_wc` is not yet in the flake's `checks.tests`.
- **End-to-end runs against a cluster are still untested:** the success lines of clone, init, fetch, pull
  and push, and "push checks the user after dialing". The CLI dials iroh only, so these need
  `tests/iroh_loopback.rs` (interop owns it) or the live interop suite.

### Verification (review; macOS arm64, `nix develop`, own target dir, `--locked`, TZ=UTC)

- `cargo test --workspace`: 41 targets, 970 passed, 0 failed, 1 ignored (`live_go_node_view_call`). `wc_cases`,
  `admin_cases` and `client_cases` now run by default.
- `cargo test -p dstore-cli --lib -- cmd_wc`: 16 passed, one of them new.
- `cargo clippy -p dstore-cli --all-targets -- -D warnings` and
  `cargo clippy -p dstore-client-rs --no-deps --test cli_wc --test cli_snapshots -- -D warnings`: clean.
- `rustfmt --check --edition 2024` over `cmd_wc.rs`, `tests/cli_wc.rs` and `tests/cli_snapshots.rs`: clean.
- **Possible flake in `tests/cli_wc.rs` `a_locked_working_copy_is_refused`, seen once.** One run panicked at
  its setup `Tree::open(...).expect("hold the working copy")`, while other tests of the binary ran in
  parallel threads. The panic text was not captured, and 55 later runs of the binary passed. core-rs opens
  the lock directory with `File::open` (O_CLOEXEC), so exec cannot pass the flock on. If it happens again,
  record the message first.
