# ci-nix-docs (layer L6): flake checks, CI, README, VECTORS.md

Owned files: `flake.nix`, `flake.lock`, `.envrc`, `.github/workflows/ci.yml`, `README.md`, `VECTORS.md`, the root
`Cargo.toml` and `Cargo.lock`, `tools/vectorgen/cmd/clisnap/`, `tests/golden/cli/snapshots.json`,
`tests/cli_snapshots.rs`, `tests/golden_tests/wire.rs`, `tests/golden_tests/wire_pack.rs`.

## Changes

1. **`flake.nix` `checks.tests`** runs every test target that opens no socket: `--workspace --lib`, the root
   targets `golden`, `cli_snapshots`, `cli_admin`, `cli_client`, `cli_wc`, `fake_cluster`,
   `fake_cluster_transfer` and `fake_cluster_worktree`, and the crate targets `all_slots` (dstore-view) and
   `structs` (dstore-codec).
   - `structs` was not on the task's list. It opens no socket, and the task asks for every such target.
   - With `--workspace`, `--test all_slots` finds the dstore-view target, which is the same as
     `-p dstore-view --test all_slots`. A local `cargo test` with exactly these flags ran every target.
   - `iroh_loopback` stays out of the check; the CI `rust` job runs it.
2. **Root `[dev-dependencies]`** gain `blake3` and `futures` (both already in `[workspace.dependencies]`).
   `Cargo.lock` changes by two lines, the root package's dependency list; nothing new is vendored.
   - `tests/golden_tests/wire.rs` and `wire_pack.rs` compare full BLAKE3-256 digests with `blake3::hash`,
     where they used to compare the first 30 bytes through core-rs `Key::new`. `wire.rs` no longer imports
     `amber_store_core::key`.
   - `futures` is only declared. The Get ports that impl-client-b.md suggests moving into the root suites
     belong to the fake-cluster owners.
3. **Linux-portable CLI snapshots.**
   - clisnap's `symlink` step takes an optional `mode`. With a mode it calls
     `unix.Fchmodat(AT_SYMLINK_NOFOLLOW)` (clisnap's `lchmod`), then fails unless `Lstat` shows the bits.
     macOS applies the bits; Linux refuses with `EOPNOTSUPP`, and its links are always 0777 anyway.
   - Fixture `wc1` gives `link` mode 0777.
   - `tests/cli_snapshots.rs`: `Step::Symlink` has `mode: Option<u32>`, and `symlink()` does the same (the
     `rustix::fs::chmodat` call is compiled on macOS only; every OS checks the bits afterwards).
     `GENERATION_UMASK` is gone. `symlink_step_sets_the_link_mode` replaces `symlink_step_has_the_generation_mode`.
   - New Go test `TestSymlinkStepMode`: 0777 everywhere; 0700 fails on Linux and is applied on macOS.
   - Regenerated with `go run ./cmd/clisnap -o …` from `tools/vectorgen` (umask 022, `GOPROXY=off`, scratch
     `GOCACHE` and `TMPDIR`, all deleted). Against the committed file, exactly four things changed: the wc1
     `symlink` step gained `"mode": 511`, and `wc/wc1 diff`, `wc/wc1/sub diff` and `wc/wc1/sub diff ..` now
     print `old mode 0777` / `new mode 0755` after `diff a/link b/link`. The other 361 cases and the other
     fixtures are byte-identical. Linux gives the same output for these three cases, by symlink(7).
4. **`.github/workflows/ci.yml`**: jobs `rust`, `vectors`, `interop` and `nix` as in PORTING.md §8.
   - They use `actions/checkout@v4`, `dtolnay/rust-toolchain@master` with toolchain 1.95.0, and
     `actions/setup-go@v5` with go-version 1.26.5 (the same value as the `go` lines of `tools/vectorgen/go.mod`
     and dstore's `go.mod`).
   - The `nix` job uses `cachix/install-nix-action@v31`. `Swatinem/rust-cache@v2` and
     `actions/upload-artifact@v4` are also used.
   - The `rust` job runs `cargo test --workspace --all-targets --locked` with `TZ=UTC`.
   - The `interop` job checks this repository out into `dstore-client-rs/` and Go dstore v0.1.9 into its
     sibling `dstore/`, builds `--release --bin dstore --examples`, and runs `bash interop/check.sh`.
   - Checked with Ruby's YAML parser and with actionlint 1.7.12 (with shellcheck over the `run:` scripts):
     no findings.
5. **`README.md`**:
   - what the project is;
   - build, install and run with Nix (`nix run .#dstore -- --help`, `nix build`, `nix profile install`,
     `nix develop`) and with cargo;
   - a usage overview in the style of dstore's README (a cluster, the client commands, working copies);
   - the compatibility contract with the DD-1 to DD-15 table, the node-side commands note and the `--local`
     refs note;
   - Rust iroh 1.2.0 and the interop evidence;
   - development: tests, golden vectors, the interop harness, CI;
   - the license.

   Flags and outputs were checked against the built binary's `--help` and `cmd_client::ref_get_text`
   (`ref get` prints `name`, `key`, `version`, `user`, `created`). `cluster replicas` is written
   `[--yes] R`, because urfave stops flag parsing at the first positional argument.
6. **`VECTORS.md`**:
   - The status is final.
   - The "Rust tests" column matches the files that load each vector. The wire family is read by `wire.rs`
     and `ticket.rs`, not `codec.rs`. Also listed: `tests/cli_admin.rs`, the dstore-cli, dstore-view and
     dstore-transport-iroh readers, and `tests/fake_cluster_worktree.rs`.
   - The layer-L1 `#[ignore]` wording is gone; no golden test is ignored.
   - New: the flake and CI coverage, the `symlink` `mode` field and the wc1 rationale, and sections for
     `storecmp` and `holdlock`.
   - "Not generated" records that `client/transcripts/*.json` was never generated, and that the fake-cluster
     suites cover those scenarios: `FakeCluster::requests` in `fake_cluster.rs` and
     `fake_cluster_worktree.rs`, and the scripted request checks of `fake_cluster_transfer.rs`.

## Hand-offs (files this task does not own)

- `tools/vectorgen/docs/vectorgen-cli.md` line 256, the `symlink` row of "Fixture steps": add `mode`? as in
  VECTORS.md, and the wc1 paragraph after the table. VECTORS.md is assembled from these docs, so the two now
  differ in this row.
- `tools/vectorgen/cmd/README.md`: `storecmp` and `holdlock` still say "not written yet, layer L6".
- `tests/fake_cluster_transfer.rs` module doc: "iterating a `futures::Stream` needs the `futures` crate,
  which the root package does not depend on" no longer holds.

## Product bugs found by the gates

None. Every gate run here passed on the product code as it stood (see "Verification").

## Verification (macOS arm64, nix develop: rustc 1.95.0, go 1.26.5)

- `cargo test --locked --workspace --lib --test golden --test cli_snapshots --test cli_admin --test cli_client
  --test cli_wc --test fake_cluster --test fake_cluster_transfer --test fake_cluster_worktree --test all_slots
  --test structs` with `TZ=UTC`: every target passed. It ran again for `cli_snapshots` on the regenerated
  file: 10 passed, including `wc_cases` and `symlink_step_sets_the_link_mode`.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: clean. `rustfmt --edition 2024 --check`
  on the three Rust files changed here: clean. `gofmt -l` on clisnap: clean.
- `go vet ./cmd/clisnap/...` and `go test ./cmd/clisnap/...`: ok.
- `nix flake check -L` (aarch64-darwin, sandbox `relaxed`): exit 0. It built `checks.fmt`, `checks.clippy`,
  `checks.dstore` and `checks.tests`. `checks.tests` ran every listed target in the release profile, and 24
  test binaries reported `ok`, none failing. The other three systems are only evaluated.
- `nix build .#dstore --no-link --print-out-paths` gave `/nix/store/…-dstore-0.1.0`. `bin/dstore --version`
  prints `dstore version dev`, and `nix run .#dstore -- --help` prints the root help. The check and package
  outputs, and the actionlint/ShellCheck paths, were deleted afterwards (`nix store delete`).

## Not verified here

- Linux. There is no Linux builder on this machine. The CI `vectors` job (clisnap on ubuntu) and the
  `rust`/`nix` ubuntu jobs are the proof, in particular for the three wc1 cases and for xattrs inside the
  Linux Nix sandbox.
- **The interop job's inputs, unresolved.** `interop/check.sh` did not exist when this task finished. The
  job and the README's harness paragraph follow port-notes/verification.md §4.5, not the script. They assume:
  - `DSTORE_GO_REPO` (the sibling checkout `$GITHUB_WORKSPACE/dstore` at tag v0.1.9) and `DSTORE_RS_BIN`
    (`dstore-client-rs/target/release/dstore`, built by the job with `--examples`);
  - `INTEROP_MDNS=auto`, `INTEROP_LOG_DIR`, and `INTEROP_HEAVY`/`INTEROP_CHAOS` on `workflow_dispatch`;
  - `GOTOOLCHAIN=local` and `CGO_ENABLED=0`;
  - a tag checkout that `git describe --tags --exact-match` can read.

  Whoever lands the script must check these names, the working directory (`dstore-client-rs/`) and whether
  the script builds the Rust binary itself, and fix `.github/workflows/ci.yml` and the README if they differ.
- The CI workflow has not run on GitHub.
- Newer majors of the actions exist (checkout v7, setup-go v7, install-nix-action v31.11). The workflow keeps
  the pinned majors the task named.
