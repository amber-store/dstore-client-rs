# impl-cli-snapshots (layer L5 gate)

Owner of `tests/cli_snapshots.rs`. The label could also fix bugs the snapshots revealed in
`crates/cli/src/` and `crates/gocli/src/`. None were revealed, so the CLI code is unchanged. The one fix is in
the harness's fixture builder (umask, below).

## Result

Every one of the 364 cases of `tests/golden/cli/snapshots.json` passes byte for byte (exit code, stdout,
stderr). Each case runs the real `dstore` binary (`env!("CARGO_BIN_EXE_dstore")`) with the clean
environment of VECTORS.md "Running a case". The 17 `node_side` cases compare with `node_side.rust`
(A 8, B 5, C 2, DD-2 2).

| Test | Cases |
|---|---|
| `framework_cases` | 170 (help 113, unknown 9, usage 37, required 11 of 13) |
| `admin_cases` | 75 |
| `client_cases` | 57 |
| `wc_cases` | 62 |

The file has 10 tests, and all pass: the four groups above; classification, dispatch agreement,
`local_ticket`, `pack_size` and the fixture builder; and the new `symlink_step_has_the_generation_mode`. The
owners' reviews had already removed the `#[ignore]` of the three L5 groups, so none is ignored.

## Found and fixed: the fixture builder depended on the test process's umask

- **Symptom.** Under `umask 077`, `wc_cases` failed three cases: `wc/wc1 diff`, `wc/wc1/sub diff` and
  `wc/wc1/sub diff ..`. Rust printed `old mode 0700` / `new mode 0755` after `diff a/link b/link`.
- **Cause.**
  - Fixture `wc1` creates `link` as a symlink, ingests it into the base, then replaces it with a 0755
    directory.
  - On macOS a new symlink gets `0777 &^ umask`, and the ingested entry keeps those bits. The fixture's
    `symlink` step has no mode.
  - For a type change, worktree `modeLines` (`worktree/diff.go:101-104`, Rust `crates/worktree/src/diff.rs`)
    prints the mode lines when the bits differ. Under 022 the link is 0755, the same as the directory, so
    nothing is printed. Under 077 it is 0700.
- **Not a CLI bug.** clisnap rerun under `umask 077` gives Go output that differs from the committed file in
  exactly those three cases, with the same two added lines that Rust printed. Under 022, clisnap reproduces
  the committed file byte for byte. So the vectors assume umask 022 while the fixture is built.
- **Fix** (`tests/cli_snapshots.rs`):
  - The `symlink` step now calls `symlink()`: `os.Symlink`, then on macOS
    `fchmodat(AT_SYMLINK_NOFOLLOW)` to `0777 &^ GENERATION_UMASK` (0o022). This goes through rustix's `fs`
    feature, with no `unsafe` and no manifest change.
  - Linux links are always 0777 and cannot be changed, so nothing is done there.
  - The regression test `symlink_step_has_the_generation_mode` checks the bits: 0755 on macOS, 0777 on Linux.
- **After the fix.** The whole file passes under `umask 077` with a hostile parent environment. That
  environment had `DSTORE_TICKET`, `DSTORE_STORE`, `AMBER_STORE`, `DSTORE_LOG_LEVEL=debug`, `DSTORE_NO_TUI`,
  `DSTORE_NO_DISCOVERY`, `DSTORE_PACK_SIZE` and `TZ=Asia/Kolkata`. It also passes under `umask 022`. With the
  umask fixed, only the parent environment was left to test, and it never mattered: `env_clear` isolates
  the binary.

### Hand-off to vectorgen-cli: these three vectors cannot pass on Linux

- On Linux a symlink's bits are always 0777. So Go (and Rust) print `old mode 0777` / `new mode 0755` in
  `wc/wc1 diff` and its two `sub` variants, which the macOS-generated vector lacks.
- The CI `vectors` job (regenerate and diff on ubuntu) and the ubuntu `rust` job (`wc_cases`) will both fail
  on these three cases.
- **Suggested portable fixture:** give the `symlink` step a mode that clisnap applies with `lchmod` where the
  OS allows it and that must be 0777 (Linux's only value). Then every platform prints the same mode lines.
  This harness would then set that mode instead of `GENERATION_UMASK`.
- **Alternative:** end `wc1`'s type change on a 0777 directory, which prints no mode lines on Linux or on a
  macOS link made with that `lchmod`.
- Not verified on Linux here (no Linux host). The Linux behaviour follows from symlink(7).

## What was verified

1. **Harness against clisnap.** `run_case` and `apply_step` were compared line by line with clisnap
   `runCase`, `normalise` and `applyStep` (`tools/vectorgen/cmd/clisnap/main.go`). They agree on:
   - the environment: exactly `PATH`, `HOME`, `TZ=UTC`, then the case env with later entries winning, and
     no `PWD`;
   - `HOME` inside a resolved directory, the cwd resolved to a real path, and `{CWD}` in args and env;
   - stdin as a pipe that gets the text and then EOF, and stdout/stderr as pipes;
   - the 60 s limit and a normal exit;
   - `{ROOT}` replaced only when cwd differs from ROOT (the Go code's condition; VECTORS.md says "when
     `subdir` is not empty", which is the same for every case);
   - `{key:…}` then `{key16:…}`, in sorted variable order.

   There are three harmless deviations:
   - scratch ingests share one throwaway store (Go makes one per step), and only the root key is used;
   - `pebble_refs` creates empty files (as documented);
   - `wc_state` opens `.dstore/packstore` to build a `Tree`. Every `wc_state` fixture ran `wc_create`
     first, so the directory already exists.
2. **The vectors are current.** clisnap was rebuilt and rerun into scratch. It used go1.26.5 from the dev
   shell, `GOPROXY=off`, a scratch `GOCACHE` and `TMPDIR`, and umask 022, and the binaries and cache were
   deleted afterwards. Its `snapshots.json` is byte-identical to the committed file.
3. **No network.** Running the test binary under `sandbox-exec -p '(version 1)(allow default)(deny
   network*)'` gives 10 passed. The same profile refuses a UDP bind and send (`EPERM`) that succeeds
   outside it. So no case binds an endpoint, and none needs a cluster.
4. **Stability.** Ten consecutive runs of the test binary passed (about 1.9 s each), before the umask fix.
   After it, the runs under umask 022, umask 077 with the hostile environment, and the sandbox all passed.
5. **Gates.** `rustfmt --check --edition 2024 tests/cli_snapshots.rs` is clean. So is `cargo clippy --locked
   -p dstore-client-rs --no-deps --test cli_snapshots -- -D warnings`, rechecked after a touch.

## Changes

- `tests/cli_snapshots.rs`:
  - the `symlink` step, `GENERATION_UMASK` and the new test (above);
  - the module doc, which no longer calls the three action groups future (L5) work, records the sandbox
    check, and lists the live-only outputs not covered here.

## Cases that need a live cluster (interop suite)

None of the captured cases needs one. These outputs are not in `snapshots.json`, because Go cannot
produce them without nodes. They move to the interop suite (verification.md §4.5, `interop/check.sh`):

| Output | Why it needs a cluster |
|---|---|
| `cat NAME /`: DD-7 panic line, exit 2 | The nil entry appears only after the ref and the root tree are fetched. |
| `cluster status` with a cluster id shorter than 4 bytes: DD-7/DD-15, exit 2, preceded by the session close | The id comes from a node's view reply (only a hand-crafted reply has one). |
| SIGPIPE: `cat NAME big \| head -c1`, `watch 'trees/**' \| head -1` (cli.md Addenda 5) | Both write only after a dial succeeds. |
| `watch` ending with exit 0 on SIGINT | Needs an established watch stream. |
| TUI into `/dev/null` (no output), colour downsampling through a pty | The TUI starts only for a transfer, after dialing. |
| `pull` conflict output | Needs a remote that moved. |
| cli.md §3.4 stdout of `cluster status`/`ticket`/`replicas`, `token create`, `node`/`voter`/`transition`/`gc`/`catalog` ops, `refs`, `watch`, `ref get/delete`, `ls`, `cat`, `store push/pull` | Printed from node replies. |
| Working-copy success lines of `clone`, `init`, `fetch`, `pull`, `push`, and `push` checking the user after dialing | These come after the dial (their bytes are fixed by the 27 `worktree/cli.json` printf vectors and the flows by the fake-cluster tests). |

## Not done here

- **Linux.** Only macOS arm64 was run. See the hand-off above for the three `wc1` diff vectors. The CI
  `vectors` job (ubuntu) is the Linux proof for everything else.
- **cli.md §5.2 says "all 32 subcommands".** The count is 30 (PORTING.md §2.1). This is the
  completeness owner's correction, and cli.md is not this label's file.
- **flake `checks.tests`** already names `--test cli_snapshots` (flake.nix line 83). The other new L5
  targets are ci-nix-docs' hand-off.
