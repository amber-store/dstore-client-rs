# Live interop suite

`interop/check.sh` runs the Go dstore v0.1.10 CLI and the Rust `dstore` side by side against a real
3-node Go v0.1.10 cluster on loopback, and compares what they print, their exit codes and their effects
(port-notes/verification.md §4.5, PORTING.md §7). `interop/lib.sh` holds the helpers.

```sh
nix develop -c bash interop/check.sh            # everything (about 10-15 minutes)
nix develop -c bash interop/check.sh B3 D1      # two checks, plus the checks they build on
nix develop -c bash interop/check.sh D          # a group
bash interop/check.sh --list                    # the checks
```

It prints one `PASS`, `FAIL` or `SKIP` line per check (a failure lists the command and a diff), then a
summary. The exit status is 0 when nothing failed, 1 when a check failed and 2 when the setup failed.

## What it does

1. **Work directory.** `mktemp -d "${TMPDIR:-/tmp}/dstore-interop.XXXXXX"`. Every binary, store, working
   copy and log lives there, and the exit trap stops every process the suite started (nodes,
   watchers, lock holders) and removes the directory.
2. **Binaries**, all into the work directory:
   - Go dstore v0.1.10 with `CGO_ENABLED=0`: from `$DSTORE_GO_BIN` (copied); else from a checkout
     (`$DSTORE_GO_REPO`, or `../dstore` next to this repository) whose HEAD must be tag `v0.1.10`, built
     from `git archive HEAD` (uncommitted changes are ignored); else `go install …/cmd/dstore@v0.1.10`.
   - The vectorgen helpers `mktree`, `treekey`, `storecmp` and `holdlock` (tools/vectorgen/cmd).
   - The Rust CLI and `examples/holdlock.rs`: from `$DSTORE_RS_BIN` (the holdlock example next to it in
     `examples/`, or `$DSTORE_RS_HOLDLOCK`); else `cargo build --release --locked --bin dstore --examples`
     into `$INTEROP_CARGO_TARGET_DIR`, by default the work directory's `target/`.
3. **Cluster**, as `dstore/scripts/e2e-loopback.sh` builds it but with `store push/pull` instead of its
   stale `push/pull --local` lines: `cluster init --store n1 --replicas 3 --weight 100 --no-relay
   --loopback`, `serve --store n1 … --gc-interval 1m`, a Go token for n2's `node join`, a **Rust** token
   for n3's (check A3), then a poll until `cluster status` shows `nodes 3 voters 3` with no transition and
   no voter change (up to 300 s).
4. **Checks.** Every client command runs with `--no-relay` and `DSTORE_LOG_LEVEL=warn`, `DSTORE_TICKET`
   set to the init ticket, `DSTORE_NO_TUI=true` (stderr is a file anyway), `TZ=UTC` unless a check sets
   it. For each comparison the Go client runs first, then the Rust client with the same arguments; both
   runs are recorded as `checks/<step>.{go,rs}.{cmd,out,err,exit}`. The caller's `DSTORE_*`,
   `AMBER_STORE`, `NO_COLOR` and `COLORTERM` are unset first, so nothing reaches another cluster.

## Comparison modes

| Mode | Meaning |
|---|---|
| exact | stdout and exit status identical; stderr identical up to the `time=` field of slog lines |
| stdout+exit | stdout and exit status identical, and the last `dstore: ` line of stderr |
| normalized | `norm_status` of stdout identical (digit runs of the `      epoch ` and `      voter ` lines masked, each run of voter lines sorted), plus exit status and the `dstore: ` line |
| panic | DD-7: both exit 2, stdout identical, the Rust stderr is the first line of Go's panic |
| format | one extended regex per line |
| root | the printed or computed root keys are equal (`treekey`), and `storecmp` finds every reachable record in both stores |

Outputs the cluster can change between two runs (`cluster status`, `gc status`, `refs`) are compared up
to 3-6 times, 3 s apart, and must agree once.

## Checks

`--list` prints them. Groups: **A** cluster and admin, **B** standalone local stores (`store
push/pull`, refs, ls, cat), **C** watch, **D** working copies created by one implementation and used by
the other, **E** CLI behaviour, **G** documented exceptions (DD-2, DD-3), **H** the live cases handed over
by the CLI snapshot gate and the L5 reviews:

| id | case |
|---|---|
| H1 | `cat NAME /`: the DD-7 panic line and exit 2, after the ref and the root tree are fetched |
| H2 | `cat NAME big.bin \| head -c1`: both die by SIGPIPE (status 141) |
| H3 | `watch 'trees/**' \| head -1`: both die by SIGPIPE on the write after `head` exits |
| H4 | the TUI with stderr on `/dev/null` (a character device, so the TUI path runs): `store pull`, `clone`. Go is the reference. On macOS both succeed. On Linux, Bubble Tea's epoll input reader refuses the harness's `/dev/null` stdin, and both exit 1 before transferring anything |
| H5 | the TUI through a pty (`script`), with truecolor, 256-colour, `NO_COLOR` and `TERM=dumb`: both draw frames, print the same `pulled` line, and use the same kinds of styling (24-bit colours, 256 colours, basic colours, bold; DD-6) |
| H6 | the working-copy `push` checks `--user` after dialing |
| H7 | `catalog restore KEY` fetches the backup object, then requires `--store` |
| H8 | `cluster status` with a cluster id shorter than 4 bytes: always SKIP (real nodes cannot send one; unit tests cover it) |

**E3** runs `cargo test --release --test iroh_loopback -- --ignored live_go_node` against n1, the four
tests at once:
- the stamped ping;
- the view call (skipped when mDNS is unavailable, B13);
- the two cases of D9, which dial through a UDP relay that slows the dialled path. The vendored iroh patch
  (`third_party/README.md`) starts no NAT traversal round while a direct path is selected. So both tests
  must stay on the slow path, and every request must be answered. One test uses a single connection from
  the CLI's every-interface endpoint, the other the CLI pool's 4 connections.

`live_go_node_pool_stays_direct_behind_a_slow_path` binds its client to 127.0.0.1. Before the patch, NAT
traversal from the CLI's every-interface endpoint could select the host's own address on an interface with
a 1280-byte MTU (a Tailscale utun). A Go node cannot answer a handshake over that route, for a Go client or
a Rust one (port-notes/impl-interop-fixes.md, interop-fix-3).

The stdout of `cluster status`, `cluster ticket` (and `--ids`), `cluster replicas`, `token create`, the
node, voter, transition, gc and catalog operations, `refs`, `watch`, `ref get/delete`, `ls`, `cat`, `store
push/pull` and the working-copy success lines are compared in A, B, C and D.

Deviations from verification.md §4.5, each explained in the check's code:

- **A11** uses `voter add <an existing voter>` (`already a voter`). With a non-member id the node adds a
  phantom voter (a majority of the new set answers the ping) and waits a minute for its marker, which
  leaves the cluster with 4 voters for the rest of the suite.
- **B12** sends SIGINT after the first `msg=uploaded` line (a stored batch) rather than the first
  `uploading` line, so the re-run must upload fewer objects.
- **B11** trees are `mktree -seed 3|4` plus 60 MiB of random files.
- **D7** conflicts on `run.sh` (modified on both sides) instead of a new `f.txt`.
- **D13** plants the n1-only init ticket in `.dstore/config`; one fetch rewrites it from the view. With
  `INTEROP_HEAVY=1`, A13 repeats it after the replica count changed.
- **D14** (dstore v0.1.10) works on a branch, a reference naming a commit. Go starts it with `push -m`, Rust
  clones it and pushes a commit on top without a message, Go fetches and pulls that commit, Go commits again
  and Rust pulls. The `.dstore/state` files must agree but for `synced_at` (they hold `remote_commit`), `ls`
  and `cat` of the branch are compared exactly, and after a lost state both clients, in twin copies, report
  the earlier push by the commit it stored. Commit keys differ between runs (they hold a timestamp), so the
  lines that print one are checked by format and against the state file.
- **G1** follows PORTING.md §2.3: Go on a Rust-written `--local` directory succeeds and adds Pebble files
  next to `refs.redb`, after which the Rust client refuses the directory.

## Inputs

| Variable | Default | Meaning |
|---|---|---|
| `DSTORE_GO_BIN` | | a prebuilt Go dstore v0.1.10 (copied into the work directory) |
| `DSTORE_GO_REPO` | `../dstore` | a dstore checkout whose HEAD is tag v0.1.10 |
| `DSTORE_RS_BIN`, `DSTORE_RS_HOLDLOCK` | | a prebuilt Rust CLI and holdlock example |
| `INTEROP_CARGO_TARGET_DIR` | work dir `target/` | where `cargo build` and E3's `cargo test` build |
| `INTEROP_MDNS` | `auto` | `auto`: B13 is skipped when the Go client cannot resolve ids over mDNS either; `require`: it fails; `skip` |
| `INTEROP_HEAVY` | 0 | 1 runs A12 and A13 (transitions) |
| `INTEROP_CHAOS` | 0 | 1 runs C3 (a node restart) |
| `INTEROP_BASELINE` | 0 | 1 uses the Go binary as both clients, to tell harness bugs from Rust bugs (G, E3 skip) |
| `INTEROP_KEEP` | 0 | 1 keeps the work directory's logs and stores (binaries are still removed) |
| `INTEROP_FAILFAST` | 0 | 1 stops at the first failure |
| `INTEROP_LOG_DIR` | | on failure, `checks/` and `logs/` are copied there (CI uploads them) |
| `INTEROP_TZS` | `UTC Asia/Kolkata America/St_Johns` | the TZ matrix of B6, B7 and C1 |
| `INTEROP_CMD_TIMEOUT` | 180 | seconds one client command may run |

Portability: bash 3.2 or later on macOS and Linux, POSIX tools only (no `timeout`, no GNU-only flags);
waits are poll loops. E3 needs `cargo`; H5 needs `script(1)` (BSD or util-linux).
