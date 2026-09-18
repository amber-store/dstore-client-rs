# impl-cli-app: command table, process entry, node-side texts, size (layer L3)

Owner: cli-app. Files: `crates/cli/src/{lib.rs, app.rs, size.rs, nodeside.rs}`, `tests/cli_snapshots.rs`,
`tests/golden_tests/cli.rs`, and the action stubs of `crates/cli/src/{cmd_admin,cmd_client,cmd_wc}.rs` (decision 1).

## What landed

- **`lib.rs`** (PORTING.md §4.12, §5.2, §5.9):
  - `VERSION`;
  - `main_entry`: SIGPIPE to `SIG_DFL`, a multi-thread runtime, `block_on(run(argv))`, flush, the runtime
    forgotten (not dropped), the exit code;
  - `run`: `dstore: ` + `rewrite_os_errors(msg)` and exit 1 for `CliError::Msg`; `msg` and `code` for
    `CliError::Exit`, printing nothing when `msg` is empty as urfave `HandleExitCoder` does;
  - `go_panic_exit`: flush stdout, `panic: <text>\n` on stderr, exit 2 (DD-7).
- **`app.rs`**: every definition of cli.md §2.8 in Go order, taken from `main.go`, `client.go`, `tui.go` and
  `wc.go` at HEAD: names, usage, args usage, the `watch` description, flag kinds, defaults, usage texts, env
  vars and required flags. The shared flag groups are functions named after Go's (`client_flags`,
  `node_flags`, `wc_flags`, …). Actions are `act::<name>` wrappers that box `cmd_*::<name>(c)`.
- **`size.rs`**: `parse_size`, `pack_size`, `DEFAULT_PACK_SIZE` (`pub(crate)`).
- **`nodeside.rs`**: `node_side_error`, `STORE_TICKET_UNSUPPORTED`, `CATALOG_RESTORE_UNSUPPORTED`,
  `local_ticket`.
- **Tests**: unit tests in `app.rs` (command paths, order, flag names per command, actions, env vars, required
  flags, kinds and defaults, texts), `size.rs` (the `TestParseSize` tables, cli.md §5.4 texts, invalid UTF-8,
  Unicode case), `nodeside.rs` (texts, missing identity, cleaned path, directory identity, readable identity);
  `tests/golden_tests/cli.rs` (`parse_size`, `pack_size`); `tests/cli_snapshots.rs` (below).

## Decisions and deviations

1. **Action stubs added to files cli-app does not own.** `cmd_admin.rs`, `cmd_client.rs` and `cmd_wc.rs` held
   only their doc comments, and `app()` must call into them. I added one
   `pub(crate) async fn <name>(c: &Context) -> Result<(), CliError> { todo!() }` per action. This only adds
   items: git showed the files unchanged since L0, and no L5 owner was working. Layer L5 fills the bodies, and
   `app.rs` never needs editing. The futures must be `Send` (`dstore_gocli::Action`).
   - `cmd_admin`: `cluster_init`, `cluster_status`, `cluster_ticket`, `cluster_replicas`, `serve`,
     `token_create`, `node_join`, `node_remove`, `node_drain`, `node_weight`, `node_zone`, `node_repair`,
     `voter_add`, `voter_remove`, `transition_{status,abort,refreeze,pause,resume}`,
     `gc_{run,status,hold,release,why}`, `catalog_{backup,backups,restore}`.
   - `cmd_client`: `store_push`, `store_pull`, `refs`, `watch`, `ref_get`, `ref_delete`, `ls`, `cat`.
   - `cmd_wc`: `clone`, `init`, `fetch`, `pull`, `push`, `status`, `diff`.
2. **What the framework appends.** `app()` defines neither `--help`/`--version` flags nor the `help` command.
   `dstore_gocli::run` appends them, as urfave does (cli.md §2.2.1-§2.2.2): the root `--help, -h` then
   `--version, -v`, every command's `--help, -h`, and the shared `help, h` command.
3. **argv.** `dstore_cli::run(args)` passes the whole command line, program name first, to `dstore_gocli::run`,
   as `app.Run(os.Args)` does. `HelpName` comes from `AppDef.name` (`dstore`), not from `args[0]`.
4. **`unsafe` in `crates/cli/src/lib.rs`.** `restore_sigpipe` calls `libc::signal(SIGPIPE, SIG_DFL)`, which
   PORTING.md §5.9 requires. PORTING.md §3.4 allows `unsafe` only in `gocompat::os`/`time`, `worktree::sys` and
   the transport-iroh socket modules. std, nix and rustix have no safe call that sets a disposition, and gocompat
   has no helper (I do not own it). Proposal: move it to `dstore_gocompat::os::restore_sigpipe()`.
5. **`local_ticket` follows the snapshots, not PORTING.md §2.2 B.1.** It renders `gocompat::os::read_file`'s
   `PathError`, whose op is `open` for an open failure and `read` for a read failure. Go's snapshot
   `node-side/files cluster ticket --store dirident` prints
   `node: no identity in dirident: read dirident/identity: is a directory`; §2.2 B.1's fixed `open` would
   differ. The path is `filepath.Join(dir, "identity")` (cleaned); the message keeps `dir` as given.
6. **`pack_size` reads bytes.** It takes `--pack-size` through `Context::os_string`, and a private
   `parse_size_bytes` backs `parse_size`, so `%q` of a non-UTF-8 value prints `\xNN` as Go does.
7. **Runtime.** `main_entry` runs `run` with `Runtime::block_on` on the main thread, so the future of
   `dstore_gocli::run` need not be `Send`. The runtime is `mem::forget`-ten: dropping it would wait for blocking
   tasks, while Go's exit abandons goroutines.
8. **`pack_size` golden test in a child process.** `std::env::set_var` is `unsafe` in edition 2024 and races the
   other tests of the `golden` binary. The test re-runs itself once per case
   (`--exact cli::pack_size --include-ignored`), with `DSTORE_PACK_SIZE` set or removed in the child's
   environment and `DSTORE_CLI_GOLDEN_PACK_SIZE_CASE` naming the case. The child parses the `serve` definition of
   `dstore_cli::app()` with a recording action. *Superseded in the review (finding 3): the test now calls
   `dstore_gocli::dispatch` with the case's environment, in process.*
9. **Crate-level `#![allow(dead_code, unused_variables)]` kept.** `common.rs`, `progress.rs` and the `cmd_*`
   stubs still need it.
10. **PORTING.md §2.1 miscounts subcommands.** It says "21 top-level commands, 32 subcommands", but Go v0.1.9
    defines 30: cluster 4, token 1, node 6, voter 2, transition 5, gc 5, catalog 3, store 2, ref 2. The `app`
    unit test asserts 30, and every path is compared with the Go definitions.

## `tests/cli_snapshots.rs`

- **Procedure.** VECTORS.md "Running a case", step by step: a canonicalised temp ROOT, a separate HOME and
  scratch dir, `env_clear` + `PATH`, `HOME`, `TZ=UTC` and the case env, `{CWD}` in args and env values, piped
  stdin/stdout/stderr, a 60 s limit, a normal exit required, then the normalisation of step 7 in order. Mismatches
  print a unified diff (`similar`) and the first differing line escaped.
- **Fixture ops, all implemented:**
  - `mkdir`, `write`, `symlink`, `remove`, `chmod`;
  - `mtime`: rustix `utimensat` with `SYMLINK_NOFOLLOW`;
  - `wc_create`: `Tree::create` + `close`;
  - `ingest`: core-rs `ingest::dir` with jobs 1 and `exclude`; `dir` `.` ingests ROOT itself;
  - `wc_state`: a `Tree` literal over `ROOT/.dstore/packstore` with the state, then `save_state` and `close`. This
    relies on `Tree`'s fields staying `pub` (PORTING.md §4.10) and keeps worktree's own serialisation;
  - `pebble_refs`: empty files.
- **Classification.** Groups `help`, `unknown`, `usage`, and `required` cases whose stderr is a required-flag
  error, are framework cases. Every other case is filed under its top-level command's action module (the first
  argument after the global `--log-level VALUE`).
- **No case needs the network in Rust.** Go binds only in the kind-A node-side cases, and Rust substitutes the
  §2.2 A text; every other case ends before dialing.
- **Tests that run now:**
  - `cases_are_classified_and_substitutes_match`: every case is classified; every `node_side.rust` equals what
    `nodeside::node_side_error`, `STORE_TICKET_UNSUPPORTED`, `CATALOG_RESTORE_UNSUPPORTED` or the §2.3 text
    gives;
  - `local_ticket_matches_the_identity_read_cases`: `local_ticket` against Go's `cluster ticket --store
    nostore|dirident|node` outcomes, in fixture `files`;
  - `fixture_builder_builds_the_plain_fixtures`: fixtures `files` and `pebble-refs`, including the ingest key.
- **Case counts** (364): framework 170 (help 113, unknown 9, usage 37, required 11), admin 75, client 57,
  wc 62. The two `required` cases with `AMBER_STORE` set reach `store pull`'s action (`dstore: pull NAME`).
- **Results (macOS arm64, `nix develop`, once gocli landed):**
  - `cargo test -p dstore-cli --lib -- app:: size:: nodeside::`: 16 passed;
  - `cargo test -p dstore-client-rs --test golden -- cli::`: `parse_size` and `pack_size` (17 cases, each in a
    child process) passed;
  - `cargo test -p dstore-client-rs --test cli_snapshots`: 4 passed, including `framework_cases`, where all
    170 help, unknown, usage and required cases match Go byte for byte; 3 ignored;
  - `cargo clippy -p dstore-cli --no-deps --all-targets -- -D warnings`: clean;
  - `cargo clippy -p dstore-client-rs --no-deps --test cli_snapshots -- -D warnings`: clean;
  - for `--test golden`, clippy reported lints only in the sibling-owned `tests/golden_tests/worktree.rs`;
    `tests/golden_tests/cli.rs` has none;
  - rustfmt `--check` on the owned files: clean.
  - Without `--no-deps`, clippy stopped at lints in dstore-gocli (a sibling mid-edit).
- **Ignored, with the module that un-ignores them:**
  - `admin_cases` (75 cases): `dstore_cli::cmd_admin` (layer L5);
  - `client_cases` (57 cases): `dstore_cli::cmd_client` and `common` (layer L5);
  - `wc_cases` (62 cases): `dstore_cli::cmd_wc` (layer L5). (The review dropped `dstore_worktree` from the reason:
    its offline part has landed, and every fixture now builds, finding 2.)
- **Linux** byte identity is not proven here: vectors and runs are macOS; the CI `vectors`/`rust` jobs settle it.

## Review

Reviewer: review-cli-app. Files changed: `tests/cli_snapshots.rs`, `tests/golden_tests/cli.rs`, `crates/cli/src/size.rs`
(one new unit test), and this file. `lib.rs`, `app.rs`, `nodeside.rs`, the library code of `size.rs` and the `cmd_*`
stubs are unchanged: the review found no defect in them.

### What was checked against the Go source

- **`app.rs`**, line by line against `main.go`, `client.go`, `wc.go` and `tui.go` at HEAD:
  - command order, names, `Usage`, `ArgsUsage`, the `watch` description, and parents without an action;
  - flag kinds, which help does not show but value parsing does: `rate`/`min-free` Int64, `jobs` Int,
    `replicas`/`min-replicas`/`token create --weight` Uint, `garbage` Float64, `gc-interval`/`put-ttl` Duration;
  - defaults (`info`, `2Gi`, `auto`, `3`, `4h`, `1h`), usage texts (U+2212, U+2026), the 7 env vars and the 4
    required flags.
  - Go defines 30 subcommands; PORTING.md §2.1's 32 is wrong (deviation 10 confirmed).
- **`size.rs`** against `size.go` and `main.go` `packSize`:
  - `TrimSpace`, leading ASCII digits, the `ParseInt` error, `ToLower` then `TrimSuffix("b")`;
  - the messages quote the trimmed input and the original unit; the overflow check;
  - `--pack-size: %w` and `%d is not a positive size`.
- **`nodeside.rs`** against `node.OpenOffline`: `os.ReadFile(filepath.Join(dir, "identity"))` wrapped as
  `node: no identity in %s: %w`. The op is `open` or `read` as Go's `*PathError` gives it (deviation 5 confirmed by
  the `dirident` snapshot).
- **`lib.rs`** against `main.go` and urfave `HandleExitCoder`:
  - `dstore: ` + errno rewrite, then exit 1; an `ExitCoder` text is printed only when non-empty;
  - stdout is flushed before stderr is written;
  - SIGPIPE goes to `SIG_DFL` first, and the runtime is forgotten, not dropped.
- **End-to-end probe of the built binary** (throwaway python; the child inherits SIGPIPE ignored, so only dstore's own
  reset can kill it):
  - `dstore --help` into a pipe whose reader is closed dies by signal 13;
  - `nosuch` exits 3 with `No help topic for 'nosuch'`, and `refs --help extra` exits 1 with
    `dstore: No help topic for 'extra'`;
  - `--version` prints `dstore version dev`.
- **The harness** against clisnap `runCase`, `applyStep` and `normalise`: the environment, cwd resolution, `{CWD}`
  substitution, the 60 s limit, the normal-exit requirement and the normalisation order.

### Findings and fixes

1. **194 of the 364 cases were not asserted at all** (the three ignored tests). That included outcomes cli-app decides
   itself: how the command table routes every action case, `pack_size` in the node-side cases, and `local_ticket` in 7
   of the 10 identity-read cases. Added over the same vectors; these run now. Each calls `dstore_gocli::dispatch`
   over `dstore_cli::app()` with the case's arguments and environment:
   - `classification_agrees_with_dispatch`: the framework reaches an action for exactly the 194 admin, client and wc
     cases, and prints nothing before it. It reaches none for the 170 framework cases.
   - `pack_size_matches_the_node_side_cases`: the 5 Go `--pack-size: …` errors, from the flag and from
     `$DSTORE_PACK_SIZE`, byte for byte, and a valid size for the 8 kind-A cases.
   - `local_ticket_matches_the_identity_read_cases` now covers all 10 identity-read cases:
     - `--store nostore` for `cluster ticket`, `cluster status` and `catalog restore KEY`;
     - `$DSTORE_STORE=nostore`, and `--store dirident`;
     - the 5 kind-B cases.

     The `--store` value comes from the parsed flags, so the env path is exercised too.
2. **The fixture-builder test built only 2 of the 10 fixtures.** `wc_create`, `ingest into wc` and `wc_state` were
   untested, although the offline part of dstore-worktree has landed. `fixture_builder_builds_every_fixture` builds all
   10:
   - it checks every ingest variable, and `.dstore/state` (`base`, `remote`, `remote_version`) against the step;
   - it opens every fixture that has a state as a working copy.
   - The `wc_cases` ignore reason no longer names `dstore_worktree`.
3. **The `pack_size` golden test re-ran its own test binary** once per case to set `$DSTORE_PACK_SIZE`. It passed
   libtest flags (`--exact cli::pack_size`) and matched `1 passed` in the child's stdout. That is fragile: a renamed
   module or another test runner changes what the child runs. It is also unnecessary, because
   `dstore_gocli::dispatch` takes `getenv`. The test now uses dispatch in process and runs each case over both `serve`
   and `cluster init`. Decision 8 is superseded.
4. **`TestPackSizeFlag` was not ported as a test**; only the vectors covered it. Added `size::tests::pack_size_flag`.
5. **Harness fidelity to clisnap.** No case changes outcome today:
   - `{ROOT}` is replaced when the resolved cwd differs from ROOT (clisnap `normalise`), instead of when `subdir` is
     non-empty;
   - `remove` has `os.RemoveAll` semantics: a missing path is not an error;
   - `HOME` is resolved, as clisnap's is.

### Confirmed, not changed

- **Deviation 4:** `unsafe` `libc::signal` in `crates/cli/src/lib.rs`, outside the PORTING.md §3.4 allowlist.
  gocompat still has no SIGPIPE helper (checked by grep over `crates/`).
- **`wc_state`:** the step opens and closes the working copy's packstore, because the Rust `Tree` holds its store.
  clisnap builds a `Tree` without a store. Nothing observable changes.
- **Still ignored:** `admin_cases`, `client_cases` and `wc_cases`, because `cmd_admin`, `cmd_client`, `cmd_wc` and
  `common` are still `todo!()` stubs. No sibling they need has landed.

### Gates

All runs on macOS arm64 under `nix develop`, with `CARGO_TARGET_DIR=scratchpad/targets/review-cli-app`.

- `cargo test -p dstore-cli --lib -- app:: size:: nodeside::`: 17 passed.
- `cargo test -p dstore-client-rs --test golden -- cli::`: 2 passed.
- `cargo test -p dstore-client-rs --test cli_snapshots`: 6 passed, 3 ignored. `framework_cases` matches all 170 cases
  byte for byte.
- `cargo clippy -p dstore-cli --all-targets -- -D warnings`: clean.
- `cargo clippy -p dstore-client-rs --no-deps --test cli_snapshots --test golden -- -D warnings`: clean.
- `rustfmt --edition 2024 --check` over the owned files: clean.

### Still open

- **Layer L5** fills `cmd_admin`, `cmd_client`, `cmd_wc` and `common`, then un-ignores `admin_cases`, `client_cases`
  and `wc_cases`.
- **`restore_sigpipe`** should move into `dstore_gocompat::os` (PORTING.md §3.4).
- **PORTING.md** needs two corrections: §2.1 (30 subcommands) and the §2.2 B.1 wording (`open` or `read`).
- **`go_panic_exit`** has no test. Its L5 callers (`cat NAME /`, a short cluster id) are not captured by the snapshots
  (VECTORS.md "Not captured"); the interop suite covers them.
- **The crate-level `allow(dead_code, unused_variables)`** stays until L5 lands.
- **Linux byte identity** is left to CI.
