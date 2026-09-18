# impl-cli-common: shared command helpers (layer L5)

Owner: cli-common. Files:
- `crates/cli/src/common.rs` and its unit tests;
- `[dev-dependencies]` `dstore-testkit` in `crates/cli/Cargo.toml`, which adds one line to `Cargo.lock`
  (`dstore-testkit` in dstore-cli's dependency list; no new package);
- the hand-offs: `dstore_gocompat::os::restore_sigpipe` (`crates/gocompat/src/os.rs`), its call in
  `crates/cli/src/lib.rs`, the dead-code allows in `lib.rs`, and DD-15 in PORTING.md §1.3.

## What landed

PORTING.md §4.12 `common` in full, with no `todo!()` left and no `unwrap`/`expect` outside tests.

- `NetOpts`, `net_opts_of`: `--relay` (lossy text, as the URL parser needs it), `--no-relay`, `--no-discovery`.
- `logger`: `TextHandler` on stderr at `log_level(--log-level)` with `SystemZone`, a new handler per call.
- `signal_ctx`: SIGINT and SIGTERM are registered with tokio before it returns; a spawned task cancels the
  ctx on the first one and keeps both streams alive, so later signals are swallowed until exit (§5.4).
- `Session::close`: `cluster.close()`, then `endpoint.close_bounded(3 s)` (DD-11).
- `dial_cluster`: `--ticket` (`$DSTORE_TICKET`) through `ticket::parse` on the raw bytes, else `--store`
  (`$DSTORE_STORE`) through `nodeside::local_ticket`, else `NO_CLUSTER`; then `dial_ticket`.
- `dial_ticket`: `generate_secret_key`, `relay_mode_of` (its error verbatim), `bind_iroh` (no ALPNs,
  `discover = !no_discovery`, no announce, the logger), `Cluster::dial` with the logger and `gc_interval` 4 h;
  a failed dial closes the endpoint.
- `admin`: `Cluster::admin(ctx, None, req)` (Go's zero id: any node), then `decode_admin_reply`.
- `admin_action`: `signal_ctx`, dial, `admin`, print (`Text`, each name, `key %x` for a 32-byte key), close.
- `print_status`: `client.go:134-193` byte for byte (below).
- `record_payload`, `open_local` (§2.3), `cluster_get`, `hex_decode`.

## Items added (nothing in PORTING.md §4.12 was changed)

1. **`pub async fn cluster_get(ctx: &Ctx, cl: &Cluster, k: Key) -> Result<Vec<u8>, dstore_client::Error>`.**
   §4.12 names `cluster_get` in the module list but not in the API block. The error type is the client's, so
   the function can serve as the `E` of the fstree getters (`WalkError<E>` needs `Display` and
   `std::error::Error`): stream errors pass through, a payload error is `Error::Amberpack`, and the two Go
   texts are `Error::Other` (`object <64 hex> not found`, `no data`). cli-client's getter runs it with
   `Handle::block_on` on a blocking thread (§5.1).
2. **`pub const NO_CLUSTER`**: `no cluster: set --ticket or $DSTORE_TICKET`, also the text of cli-wc's
   `resolveTicket`.
3. **`NetOpts` derives** `Clone, Debug, Default, PartialEq, Eq`.

## Decisions

1. **DD-15 (orchestrator decision).** `print_status` takes the cluster id's length as its capacity:
   `dstore_view::status_cluster_prefix(v.cluster_id, len)`. A shorter-than-4-byte id calls
   `go_panic_exit("runtime error: slice bounds out of range [:4] with capacity N")` before anything is
   printed (DD-7). Go agrees for every definite-length encoding; a 1..3-byte indefinite-length id prints
   zero-padded in Go. Recorded in PORTING.md §1.3 and asserted over `view_placement.json`
   `status_cluster_prefix` (3 cases differ in capacity; the 9-byte one still agrees with Go).
2. **No view.** `cl.view()` is always `Some` after a successful dial. Should it be `None`, `print_status`
   exits as Go's nil dereference would (`runtime error: invalid memory address or nil pointer dereference`).
3. **Output.** Each Go `fmt.Print*` call group is one `write_all` plus `flush` on `out`, and write errors are
   ignored as `fmt` ignores them. `out` is flushed before `go_panic_exit`.
4. **Per-node status.** `ctx.with_timeout(5 s)` around `Cluster::status`, cancelled after the call.
   `— unreachable: <err>` uses the client error's Display; any `decode_status` error gives `— bad status`.
   `FreeBytes >> 30` is an arithmetic shift (`-1 GiB` for a negative value, as Go).
5. **Failed dial.** Go calls `ep.Close()`, the library close (5 s bound), so Rust calls `Endpoint::close`
   (`close_bounded(CLOSE_TIMEOUT)`), not the 3 s of DD-11, which applies at CLI exit.
6. **`signal_ctx` edge cases.** Outside a tokio runtime nothing is registered and the ctx ends only when
   cancelled. If one registration fails (it cannot with `enable_all()`), that signal is not watched; Go's
   `Notify` cannot fail.
7. **`open_local`.**
   - The packstore opens as `worktree::open_store` does (core-rs-gaps G3, G5, G6): `mkdir_all(dir, 0o755)`
     wrapped as `packstore: creating <dir>: …`, the raw `open <dir>: <errno>`, then core-rs (its flock text
     reaches Go's form through the top-level `rewrite_os_errors`).
   - The Pebble check lists `filepath.Join(DIR, "refs")` and matches the §2.3 names. A missing or unreadable
     directory holds none. The refusal names the joined (cleaned) path, which is `P/refs` for the snapshot
     cases. The packstore is dropped (its flock released) before the error returns.
   - `<local>/refs` is pre-created 0755 as Pebble's `MkdirAll` does (G5); a failure there is left for
     `refstore::Store::open` to report (G20 keeps core-rs's `refstore: opening redb: …` texts).
8. **`admin_action` order**, as Go's deferred close: print, then close; on an admin error, close, then return
   the error.

## Tests (`common::tests`, 21)

- **Vectors, read from `tests/golden`** by a small JSON reader in the test module. serde is not a dependency
  of dstore-cli, and this task may add only workspace crates as dev-dependencies. A missing file fails.
  - `admin/replies.json`: every `printed` of `cases` and `decode` (20+) through `write_admin_reply`.
  - `status/status.json`: all 18 `cases`, `decode` and `unreachable` entries: the node line and the node's
    printed output (`[lease holder]`/`[AMNESIAC]`/`[retired]`, `cannot reach` with `?` ids, `gc`, hidden
    `idle` transition, voter lines, `— bad status`, `— unreachable: …`).
  - `view/view_placement.json` `status_cluster_prefix`: Go's prefix or panic text for the definite cases,
    DD-15 for the indefinite ones.
- **Over `dstore_testkit::fake`** (mem transport, no sockets):
  - `print_status_over_a_fake_cluster`: the whole output for three nodes with weights and zones, where node 2
    sleeps past the timeout (`— unreachable: context deadline exceeded`, the run takes 5 s) and node 3 answers
    `unavailable` (`— unreachable: remote: unavailable: no view`);
  - `admin_over_a_fake_cluster`: `transition-status`, `catalog-backup` (text and `key <hex>`),
    `catalog-backups` (names), an unknown op's remote error;
  - `cluster_get_over_a_fake_cluster`: a stored blob's payload, and `object <hex> not found`.
- **Formatting units:** `status_header_lines` (the fault-tolerance suffix, the transition and voter-change
  lines, a short voter-sync target `?`), `node_lines_quote_zones_and_match_voters_exactly`,
  `admin_reply_key_needs_32_bytes`, `hex_decode_like_go`, `record_payload_of_raw_records`, `pebble_names`.
- **Up to the bind, no sockets:** `dial_cluster_ticket_sources` (`NO_CLUSTER`, `--ticket=`, flag and env
  tickets, `--ticket` over `--store`, `$DSTORE_STORE` only where `--store` is defined, the identity-read error),
  `dial_ticket_relay_errors_come_before_the_bind`, `net_opts_from_flags`, `logger_levels`. The contexts come
  from `dstore_gocli::dispatch` over `crate::app()`.
- **Signals:** `signal_ctx_cancels_and_swallows_later_signals` sends SIGTERM to the test process with the
  `kill` binary, then SIGTERM and SIGINT again, which are swallowed; `signal_ctx_outside_a_runtime_registers_nothing`.
- **Local stores:** `open_local_creates_both_stores` (plus the second-open flock text),
  `open_local_refuses_pebble_refs_and_releases_the_packstore`, `open_local_packstore_errors_have_go_texts`
  (a file in the way, and a `DIR/./` cleaned as `filepath.Join` does). Scratch directories are removed on drop.

## Hand-offs from the L2-L4 reviews

- **(1) DD-15** is in PORTING.md §1.3 (decision 1 above).
- **(2) `restore_sigpipe`** is now `dstore_gocompat::os::restore_sigpipe()`, inside the §3.4 unsafe allowlist.
  `main_entry` calls it first; `crates/cli/src/lib.rs` has no `unsafe` left.
- **(2) Dead-code allows.** All four `#[allow(dead_code, unused_variables)]` in `lib.rs` (`common`,
  `cmd_admin`, `cmd_client`, `cmd_wc`) are removed, with their stale comment. The last three were removed once
  cli-admin, cli-client and cli-wc had no `todo!()` left: with their files as they were at that moment,
  `cargo check -p dstore-cli --lib --tests` gave no warnings, first in a scratch copy, then in the checkout.
- `tests/cli_snapshots.rs` `client_cases` is ignored for "dstore_cli::cmd_client and dstore_cli::common".
  `common` has landed, so cli-client can un-ignore it once `cmd_client` has.

## Gates

All in the flake dev shell with `CARGO_TARGET_DIR=scratchpad/targets/cli-common` (deleted afterwards), macOS
arm64.

While the siblings' `cmd_client.rs` and `cmd_wc.rs` were mid-edit, first with compile errors in their test
code, then with clippy lints and a failing test in their own modules, some gates ran in a scratch copy of the
checkout where those files were reset to their HEAD stubs. The copy is deleted.

- In the checkout, after the allows were removed:
  - `cargo check -p dstore-cli --lib --tests`: no warnings;
  - `cargo test -p dstore-cli --lib common::`: 21 passed;
  - `cargo clippy -p dstore-cli --lib --tests --no-deps -- -D warnings`: nothing in `common.rs` or `lib.rs`.
    The 4 lints it reports are in the siblings' in-progress `cmd_wc.rs` (`manual_async_fn` at 435, 456 and 488,
    `manual_is_multiple_of` at 834).
  - `rustfmt --edition 2024 --check` on `common.rs`, `lib.rs` and `gocompat/src/os.rs`: clean.
- In the scratch copy (sibling files at HEAD):
  - `cargo clippy -p dstore-cli --lib --tests --no-deps -- -D warnings`: clean;
  - `cargo test -p dstore-client-rs --test cli_snapshots`: 6 passed, 3 ignored (the L5 groups). `framework_cases`
    still matches all 170 cases after the `lib.rs` change.
- With the siblings' in-progress `cmd_wc.rs`: `cargo test -p dstore-cli --lib` gave 76 passed; the one failure
  is `cmd_wc::tests::cmd_error_texts`, a sibling's test.
- `cargo clippy -p dstore-gocompat --all-targets --no-deps -- -D warnings`: clean. `cargo test -p
  dstore-gocompat --lib os::`: 8 passed.
- SIGPIPE after the move: `dstore --help` into a pipe with a closed reader, run with SIGPIPE inherited as
  ignored, dies by signal 13 (return code -13). `dstore --version` prints `dstore version dev`.

## Review

Reviewer: review-cli-common, with the same file ownership. Files changed: `crates/cli/src/common.rs` and this
file. `lib.rs`, `gocompat/src/os.rs`, `crates/cli/Cargo.toml`, `Cargo.lock` and the DD-15 entry of PORTING.md
are unchanged: the review found no defect in them.

### What was checked against the Go source

Specs read: PORTING.md §0-3, §4.12, §5-7 (and DD-7, DD-11, DD-15); cli.md §2.3-§2.5, §2.7, §3.3, §3.4, §3.8,
§5.4 and its Addenda; view-placement §3.4, §3.5 and Addenda; client-core §2.9, §3.8 and Addenda; VECTORS.md
families `admin` and `cli`; impl-cli-app, impl-view. Go: `cmd/dstore/client.go:39-221, 454-574`,
`main.go:54-68, 258-260, 319-333`, `client/client.go` `Close`/`View`, `transport/iroh.go` `Close`, `view/view.go`
(`Pending`, `VoterSync`, `IsVoter`), core v0.0.8 `packstore.Open`.

- **`dial_cluster`/`dial_ticket`**: the ticket source order (`--ticket` bytes through `ticket::parse`, then
  `--store` through `local_ticket`, then `no cluster: …`), then `GenerateSecretKey`, `relayModeOf` (its error
  verbatim, before anything is bound), `BindIroh` (no ALPNs, `Discover = !no-discovery`, no announce, the
  logger), `client.Dial` with `GCInterval` 4 h, and `ep.Close()` (the library close, 5 s) on a failed dial.
- **`admin`/`admin_action`**: the zero id (any node), `AdminReply` decoding, the print block (`Text` when
  non-empty, each name, `key %x` only for 32 bytes), the deferred close after printing, and no close when the
  dial fails.
- **`print_status`**, format string by format string: the `ClusterID[:4]` panic before anything is printed
  (DD-7, DD-15 as decided), `len(Nodes)`/`len(Voters)`, the fault-tolerance suffix, the `Pending` and
  `VoterSync == 1` lines (`ShortID` → `?`), the node line (`IDString(NID)`, `%q` zone, exact 32-byte
  `IsVoter`), the per-node 5 s timeout cancelled after the call, `Println(line, "— unreachable:", err)`,
  `— bad status`, `FreeBytes>>30` (arithmetic), the tags in Go's order, `cannot reach`, `gc`, `transition`
  (not `""`/`idle`), the voter lines. Output is written per Go print group, after each status call, as Go's
  unbuffered stdout does.
- **`record_payload`, `cluster_get`** (first result only, `missing()` after the loop, the two texts),
  **`hex_decode`** (`len(s)/2` pairs, `%q` of the whole input), **`logger`** (`strings.ToLower` equivalence,
  a new handler per call), **`signal_ctx`** (registered before it returns; later signals swallowed).
- **`open_local`** against `packstore.Open` (`MkdirAll` wrapped as `packstore: creating`, the raw `os.Open`
  error, then the flock text) and PORTING §2.3 (packstore first, the Pebble listing before redb, the packstore
  released on a refusal).
- **Hand-offs**: DD-15 in §1.3; `restore_sigpipe` in `gocompat::os` (the only `unsafe`, inside the §3.4
  allowlist; `crates/cli/src` has none); the four dead-code allows are gone and `cargo clippy -p dstore-cli
  --all-targets -D warnings` is clean with the siblings' current files. The SIGPIPE probe was repeated on the
  review's build: `--help` and `--version` into a pipe without a reader, SIGPIPE inherited as ignored, both
  die by signal 13.

### Found and fixed

1. **Two vector sections were not asserted.** VECTORS.md family `cli` gives `cli/text.json` `hex_decode`
   (16 cases, "Rust `common::hex_decode(in.as_bytes())`") and `log_level` (17 cases, `logLevel(c)` with
   `--log-level value`) to the dstore-cli unit tests, and `tests/golden_tests/cli.rs`/`cli_progress.rs` defer
   them there, but no test read them (the existing tests used hand-written cases). Added `hex_decode_vectors`
   and `log_level_vectors`; the latter parses `--log-level <value> refs` over `crate::app()` and checks that
   `logger(c)` enables exactly the vector's level and above. Both pass.
2. **The DD-7 path skipped Go's deferred close.** In Go the panic of `v.ClusterID[:4]` unwinds through the
   `cluster status` action, whose deferred `cl.Close()` closes the pool connections before the runtime prints
   the panic and exits 2; the nodes see CONNECTION_CLOSE. `print_status` exited at once, so the nodes saw an
   idle timeout. It now calls `close_before_panic` first: flush `out`, `cl.close()`, then the endpoint close
   bounded to 3 s (DD-11). Only the peers can observe the difference; stderr and the exit status are unchanged.
   A unit test runs `close_before_panic` over the fake cluster (the exit itself cannot run in a test).

### Checked, not changed

- **`--relay` is read lossily** (`NetOpts.relay: String`, §4.12, and `relay_mode_of(&str)`). Go keeps the
  bytes, so a non-UTF-8 `--relay` that Go's `url.Parse` rejects prints `\xNN` in Go and U+FFFD here. DD-8's
  scope (pathological input in an error text); a UTF-8 value is unaffected.
- **Signal handlers stay installed after the action** (Go's `stop()` unregisters them at return). The process
  exits right after the action, so only a signal in that window differs (PORTING §5.4).
- **A `<local>/refs` that is a file** gets core-rs's `refstore: opening redb: …` text, not Pebble's (G20).
- **DD-15**: the capacity is the length; the 3 indefinite-length vector cases are asserted as DD-15 says.
- **Un-ignoring.** `tests/cli_snapshots.rs` is not this label's file, and `client_cases`/`admin_cases`/
  `wc_cases` wait on `cmd_client`/`cmd_admin`/`cmd_wc`, which have not been reviewed. With `--include-ignored`
  all 9 snapshot tests pass here (admin 75, client 57 and wc 62 cases included), so nothing on common's side
  blocks them.

### Gates (review)

All in the flake dev shell on macOS arm64, `CARGO_TARGET_DIR=scratchpad/targets/review-cli-common` (deleted
afterwards).

- `cargo test -p dstore-cli --lib`: 99 passed, 0 failed (`common::` 24: the 21 above plus
  `hex_decode_vectors`, `log_level_vectors`, `close_before_panic_closes_the_cluster`).
- `cargo test -p dstore-gocompat --lib`: 117 passed.
- `cargo test -p dstore-client-rs --test cli_snapshots -- --include-ignored`: 9 passed.
- `TZ=UTC cargo test --workspace --locked` (before the last unit test was added): 964 passed, 0 failed,
  4 ignored (the three L5 snapshot groups and `live_go_node_view_call`).
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: clean; after the last edit,
  `cargo clippy -p dstore-cli -p dstore-gocompat --all-targets --no-deps -- -D warnings`: clean.
- `rustfmt --edition 2024 --check` on `common.rs`, `lib.rs`, `gocompat/src/os.rs`: clean.
- The signal test runs the `kill` binary; the dev shell's `kill` is coreutils 9.11's, which stdenv also puts
  on the PATH of the flake's sandboxed `checks.tests` (`--workspace --lib`).
