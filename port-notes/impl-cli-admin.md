# impl-cli-admin: the admin actions (layer L5)

Owner: cli-admin. Files:
- `crates/cli/src/cmd_admin.rs`: the actions of `cluster`, `serve`, `token`, `node`, `voter`, `transition`,
  `gc` and `catalog`, and a unit test over the fake cluster;
- `crates/cli/src/cmd_admin/args.rs` (new, a submodule of `cmd_admin`): the network-free part;
- `tests/cli_admin.rs` (new).

No public signature changed; PORTING.md §4.12 has no `cmd_admin` items. Nothing outside these files was edited.

## What landed

Every action of `cmd/dstore/main.go` for these commands, with no `todo!()`, `unwrap` or `expect` outside tests:

- **`cluster status`**: `signal_ctx`, `dial_cluster` with `logger(c)`, `common::print_status` on stdout, then
  the session close (DD-11). Short cluster ids (DD-7/DD-15) are `print_status`'s.
- **`cluster ticket [--ids]`**: `signal_ctx`. Without `--ticket` but with `--store` it calls
  `nodeside::local_ticket` (§2.2 B, no network). Otherwise it dials and sends `admin {cluster-ticket}`, then
  `ticket::parse(r.ticket)` (its error verbatim), and prints `IDs()` or `Encode()` plus `\n` before closing.
- **`cluster replicas R [--yes]`**: `ParseUint(First(), 10, 8)`, else `replicas R`. Without `--yes`, the
  question goes to stdout without a newline, then comes `fmt.Scanln` (below) and the `y` prefix check
  (`aborted`). Only then `admin_action` runs, which is where `signal_ctx` starts, as in Go.
- **`token create [--weight]`**: `uint32(c.Uint("weight"))` (the low 32 bits), dial, `admin`,
  `r.text + "\n"` (a newline even for an empty text).
- **`node remove|drain|weight|zone|repair`, `voter add|remove`, `transition *`, `gc *`,
  `catalog backup|backups`**: the request is built first, so the argument checks come before
  `admin_action` (`adminAction`: `signal_ctx`, dial, admin, print, close). The ids go through
  `view::parse_node_id` on the raw argv bytes, so `view: bad node id "<%q>"` echoes them. `node weight` uses
  `ParseUint(Get(1), 10, 32)` → `weight ID GiB`. `gc why` needs strict hex of 32 bytes →
  `why KEY (64 hex chars)`. `node zone` sends `Get(1)` lossily (DD-8), and `""` is allowed. `voter add`
  reads the undefined `--allow-unsafe` as false.
- **`catalog restore KEY|FILE`** (§2.2 C): `signal_ctx`, then the argument: `os.ReadFile(arg)`, else a
  32-byte key, else `restore KEY|FILE`.
  - For a key: dial, then `Cluster::get`. An error item is returned; each record's
    `common::record_payload` replaces the data; no record gives `backup object not found in the cluster`.
  - Then, for both sources: `--store == ""` → `restore runs on a voter: give --store`, else
    `CATALOG_RESTORE_UNSUPPORTED`. The session closes last, as Go's deferred `cl.Close()`.
- **Node-side `serve`, `cluster init`, `node join`** (§2.2 A):
  - `signal_ctx` first, as Go;
  - `node join` only: `ticket::parse(--seed)`, then a 32-byte strict-hex token (`token must be 32 bytes of
    hex`);
  - then `--store == ""` (`no store directory: …`), then `size::pack_size`, then
    `nodeside::node_side_error`. Nothing is created.

## Decisions

1. **The pure part is its own file, `cmd_admin/args.rs`, which `tests/cli_admin.rs` compiles with `#[path]`.**
   - `cmd_admin` is a private module (`mod cmd_admin;` in `lib.rs`, which cli-admin does not own).
   - The vector files need serde, which only the root package has as a dev-dependency (cli-common wrote a JSON
     reader for its unit tests instead).
   - So `args.rs` names external crates only (no `crate::`). The test binary then runs the very
     `AdminCommand::request`, `scanln_word`, `confirmed`, `replicas_prompt`, `join_seed_and_token`,
     `restore_source`, `token_line`, `reply_ticket` and `ticket_line` the actions call.
   - The dial/print/close glue stays in `cmd_admin.rs`; it needs `futures` (the `GetStream`), which the root
     package lacks.
   - Proposal, if an integration test should import these directly: `#[doc(hidden)] pub mod cmd_admin;` in
     `lib.rs`.
2. **`AdminCommand`** lists the 22 subcommands whose request comes from the arguments alone. `request(c)`
   runs the checks in Go's order (the id before the weight, and so on) and evaluates the flags as Go does.
   `cluster replicas` reads R from the built request for the prompt (`uint8(r) == r` for R ≤ 255).
3. **Delegation.** The actions call cli-common's `signal_ctx`, `logger`, `dial_cluster`, `admin`,
   `admin_action`, `print_status` and `record_payload`, exactly where Go calls `signalCtx`, `logger`,
   `dialCluster`, `admin`, `adminAction`, `printStatus` and `recordPayload`. `adminAction`'s print block is
   common's (`write_admin_reply`).
4. **The prompt reads like Go's `fmt.Scanln(&ans)` on `os.Stdin`** (PORTING.md §5.9):
   - The read is unbuffered, one byte per `read(2)`, through `std::io::stdin().as_fd().try_clone_to_owned()`:
     a dup of fd 0 that shares the offset and bypasses std's `BufReader`. This needs no `unsafe`.
   - It runs in `spawn_blocking`, so a reader that never answers blocks only that thread. Until
     `admin_action`, no signal handler is installed, so SIGINT kills the process, as Go's.
   - `Scanner` ports `ss`/`readRune`:
     - `SkipSpace`, with a newline (also `\r\n`) failing as "unexpected newline";
     - `notEOF`, then the token up to fmt's `space` table;
     - the trailing `nlIsEnd` check, which runs after the word is stored;
     - `atEOF` after a newline;
     - `utf8.FullRune`/`DecodeRune` with the pending bytes after an ill-formed start.
   - A read error aborts the scan with `ans` unchanged (""), whereas EOF keeps a complete word.
   - A stdin that cannot be duplicated reads as a failed scan, so the answer is `aborted`.
5. **Go truth for the scanner.** A throwaway Go 1.26.5 program ran `fmt.Fscanln(r, &ans)` over 51 inputs.
   The reader hid `bytes.Reader`'s `RuneScanner`, as `*os.File` does, and a second reader fails after its
   data. The program recorded `ans`, the bytes consumed and the `y` check. The inputs:
   - the snapshot answers;
   - `\r`, `\r\n` and `\r\r\n`;
   - Unicode spaces (U+00A0, U+0085 as `c2 85`, U+2028, U+3000);
   - ill-formed UTF-8 (`ff`, `e2 28`, surrogates, `f4 90 …`, a truncated `e2 82` at EOF);
   - NUL, `Ÿes` and `Kyes` (Kelvin sign), which are not confirmed;
   - trailing spaces, extra words, and read errors mid-word and after the word.

   The table is `SCANLN_GO` in `tests/cli_admin.rs`. The program ran with `go run` and a scratch `GOCACHE`,
   and was deleted with its cache.
6. **`catalog restore FILE --store DIR` does not read `DIR/identity`.** Go's `node.OpenOffline` would fail
   first with `node: no identity in …`. The snapshot vectors' node-side kind C substitute
   (`files catalog restore --store nostore backup.bin`) expects `CATALOG_RESTORE_UNSUPPORTED`, which is also
   PORTING.md §2.2 C.5, so the fixed text comes right after the `--store` check. The key path's identity
   read happens anyway through `dial_cluster` → `local_ticket` when only `--store` is given, as in Go.
7. **`token create` builds its request before dialing.** Go reads `c.Uint("weight")` after `dialCluster`.
   Nothing can fail there, and flag values do not change, so the order is unobservable.
8. **Stdout.** Each Go `fmt.Print*` is one `write_all` plus `flush` on a locked stdout, and write errors are
   ignored. `cluster status` passes `std::io::stdout()` to `print_status` and flushes before closing.

## Tests

- **`tests/cli_admin.rs`** (11 tests, no sockets). Each context comes from `dstore_gocli::dispatch` over
  `dstore_cli::app()` with an empty environment, and the tests assert that nothing is printed before the action.
  - **`argv_builds_the_go_request`**, over all 60 argv cases of `admin/requests.json` (22 commands):
    - the request field by field (`garbage` by its bits; any NaN equals any NaN);
    - `marshal` against `params_hex`;
    - the TAdmin frame stamped `(cid16, 1, 7)` against `frame_hex`.
  - **`argv_requests_reach_a_fake_cluster`** sends the same 60 requests through `common::admin` to a
    `FakeCluster`. Every TAdmin frame a node reads carries the vector's params and the view's stamp, and
    restamped it equals `frame_hex`, so the client adds no field.
  - **`replies_print_as_token_create_and_cluster_ticket_do`**, over all 19 `admin/replies.json` cases:
    - the decoding;
    - `printed_token_create`;
    - the 4 `cluster_ticket` outcomes: `Encode()`, `IDs()`, and `ticket: empty` and the base32 error.
  - **`token_create_and_cluster_ticket_over_a_fake_cluster`**:
    - `token create --weight 7` returns a 32-byte token with its hex text;
    - `cluster ticket`: the answering node first, then the view's first three; `--ids` deduplicated; the
      re-encoding equals the node's string.
  - **`argument_errors_have_go_texts`** (32 command lines) and **`accepted_argument_forms`**:
    - the forms `ParseUint` base 10 accepts or rejects (`010`, `+5`, ` 5`, `1_0`, `0x10`, 2³²);
    - upper-case ids;
    - an empty zone with extra arguments;
    - `voter add` without `--allow-unsafe`;
    - the 32-bit truncation of `--weight`.
  - **`non_utf8_arguments`**: `%q` of `ab\xffcd`, a lossy zone, and a non-UTF-8 weight.
  - **`node_join_seed_and_token`**: `ticket: empty`, `bogus`, an off-curve seed, then the token forms.
  - **`catalog_restore_argument`**: a file wins, even one named like a key; then keys; then `""`, a
    directory, a missing file, 31 bytes and odd hex.
  - **`replicas_prompt_text`**, and **`scanln_matches_go`** (the 51 Go cases: answer, bytes consumed,
    confirmation).
- **`cmd_admin::tests::fetch_backup_over_a_fake_cluster`**: `catalog backup`, then `fetch_backup` of its key.
  The payload hashes to the key (`Key::new(Blob, len, data)`). A key no node holds gives
  `backup object not found in the cluster`.
- **`tests/cli_snapshots.rs` `admin_cases`** (cli-app's, still `#[ignore]`d) passes with `--ignored`. That
  is all 75 admin cases of the Go binary, byte for byte:
  - validation, `no cluster`, the prompt with 10 stdin variants;
  - node-side kinds A, B and C;
  - the identity-read errors.

## Gates

macOS arm64, `nix develop`, `CARGO_TARGET_DIR=scratchpad/targets/cli-admin`:

- `cargo test --locked -p dstore-client-rs --test cli_admin`: 11 passed.
- `cargo test --locked -p dstore-cli --lib cmd_admin::`: 1 passed.
- `cargo test --locked -p dstore-client-rs --test cli_snapshots -- --ignored admin_cases`: passed (75 cases).
- `cargo clippy --locked -p dstore-cli --all-targets -- -D warnings`: clean. `lib.rs` no longer allows
  dead code on `mod cmd_admin`, and the crate stays clean.
- `cargo clippy --locked -p dstore-client-rs --no-deps --test cli_admin --test cli_snapshots -- -D warnings`:
  clean.
- `rustfmt --edition 2024 --check crates/cli/src/cmd_admin.rs tests/cli_admin.rs` (which follows `args.rs`):
  clean.

## Hand-offs

- **cli-app** (owner of `tests/cli_snapshots.rs`): drop the `#[ignore]` of `admin_cases`, which passes now.
- **ci-nix-docs**: add `--test cli_admin` to the flake's `checks.tests` (a new L5 test target).
- **Not testable without sockets**: the dial glue itself (dialing, then printing and closing), over a real
  cluster. That is the interop suite's job.

## Review

Reviewer: review-cli-admin. Files touched: `crates/cli/src/cmd_admin.rs`, `tests/cli_admin.rs`, this file, and
one line of `tests/cli_snapshots.rs` (the `#[ignore]` of `admin_cases`, PORTING.md §3.3 item 4).

**Checked against the Go source line by line** (`main.go:229-260`, `279-405`, `414-458`, `463-698`,
`client.go:41-132`): every action's steps, validation order, texts, stdout and exit status.
- `signalCtx` placement:
  - first in `cluster status|ticket`, `token create`, `catalog restore` and the three node-side commands;
  - inside `admin_action` for the adminAction commands and `cluster replicas`, after the argument checks and
    after the prompt. No other signal registration runs before the prompt (only the TUI registers SIGWINCH),
    so SIGINT at the prompt kills the process, as in Go.
- `idArg` / voter `act`: `parse_node_id` on the raw argv bytes. `voter add` reads the undefined
  `--allow-unsafe` as false.
- `ParseUint(First(), 10, 8)` and `ParseUint(Get(1), 10, 32)`, and the strict 32-byte hex of `gc why`.
- The stdout texts: `%d` of R twice, no newline after the prompt; `fmt.Println(r.Text)` for `token create`;
  `IDs()`/`Encode()` plus `\n` for `cluster ticket`.
- Print before close (Go's deferred `cl.Close()`), and close before returning an error.
- `catalog restore`: file first, then key, fetch, then `--store`, then the §2.2 C.5 text. Not reading
  `DIR/identity` there (decision 6) is what snapshots.json's kind C substitute for
  `files catalog restore --store nostore backup.bin` requires.
- `data == nil`: core v0.0.8 `amberpack.DecodePayload` always returns a non-nil slice
  (`make([]byte, len)`, or `DecodeAll` into `make([]byte, 0, ulen)`). A record with an empty payload therefore
  does not read as "not found" in Go, and `Some(vec![])` matches.

**The `fmt.Scanln` port**, checked against go1.26.5 `fmt/scan.go`:
- `readRune`: pending bytes, the `FullRune` loop, EOF inside a sequence;
- `ss.ReadRune`/`getRune`: `atEOF` on a newline;
- `SkipSpace`, including `\r` + peek;
- `notEOF`, `token`, `convertString`;
- `doScan`'s `nlIsEnd` check, which runs after `*v` is assigned;
- `errorHandler`;
- the `space` table.

A second throwaway Go probe (`go run` with a scratch `GOCACHE`, both deleted) re-ran `fmt.Fscanln` over a
reader that is not a `RuneScanner`. It reproduced all 51 rows of `SCANLN_GO`. I added 27 more of its rows to
the table:
- a lone `\r` (EOF and read error);
- VT, FS, U+1680 and U+200B;
- `YES`;
- read errors after `\n` and after `\r`;
- truncated and complete 4-byte runes;
- U+2028 and U+0085 after the word;
- `f0 e2 82 ac`, `f0 90 41`, `c0 af`, U+D7FF and U+10FFFF.

**Finding (fixed): a non-blocking stdin aborted the prompt.**
- Go: `os.NewFile(0)` marks a non-blocking fd pollable (`os/file_unix.go` `newFile`), so `fmt.Scanln` waits for
  input on a pipe or terminal that another process left in O_NONBLOCK.
- Rust: `read(2)` returned EAGAIN. The scanner took it as a read error, so the answer was empty and the command
  failed with `aborted` even when `y` followed.
- Fix: `read_answer` reads through `WaitReadable`, which retries `WouldBlock` after a 10 ms pause on the blocking
  thread. Every other error still fails the scan.
- Test: `cmd_admin::tests::a_non_blocking_stdin_is_waited_for`.

**Vectors:**
- `admin/requests.json`: all 60 argv cases, for the request, the params and the frame, both built and received
  by a FakeCluster. The 15 cases without argv are `tests/golden_tests/wire.rs`'s.
- `admin/replies.json`: `printed_token_create` and all 4 `cluster_ticket` outcomes here. `printed` is tested in
  common's unit tests.
- `status/status.json`: `print_status` output, tested in common's unit tests.
- No admin-specific Go test is listed in cli §6.

**No other defects found.** No `todo!()`, and no `unwrap`/`expect` outside tests.

**Gates** (macOS arm64, `nix develop`, `CARGO_TARGET_DIR=scratchpad/targets/review-cli-admin`):
- `cargo test --locked -p dstore-client-rs --test cli_admin`: 11 passed (`SCANLN_GO` now has 78 rows).
- `cargo test --locked -p dstore-cli --lib cmd_admin`: 2 passed.
- `cargo test --locked -p dstore-client-rs --test cli_snapshots`: `admin_cases` passes and is no longer
  ignored (75 cases).
- `cargo clippy --locked -p dstore-cli --all-targets -- -D warnings` and
  `cargo clippy --locked -p dstore-client-rs --no-deps --test cli_admin --test cli_snapshots -- -D warnings`:
  clean.
- `rustfmt --edition 2024 --check` on `cmd_admin.rs`, `cmd_admin/args.rs` and `tests/cli_admin.rs`: clean.
- `cargo test --workspace --locked`: exit 0, 970 passed, 0 failed. The only ignored test is `iroh_loopback`
  `live_go_node_view_call`, which needs `DSTORE_LIVE_STORE`.

**Hand-offs still open:**
- ci-nix-docs: the flake's `checks.tests` needs `--test cli_admin`.
- The live interop suite covers the dial/print/close glue of `cluster status`, `cluster ticket`, `token create`
  and `catalog restore KEY` against Go nodes.
