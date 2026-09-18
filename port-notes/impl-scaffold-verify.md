# impl-scaffold-verify: L0 scaffold verification

Owner: scaffold-verify. An adversarial check of the L0 scaffold (`impl-scaffold.md`) against PORTING.md §3,
§4, §5.10, §5.11, §7 and §8, and against the Go struct tags. Every deviation found is fixed; the fixes are at
the end, together with what was reviewed and kept.

## Checklist

### §3.1 tree and file names

- [x] Root `Cargo.toml`: `[workspace]` members `crates/*`, resolver 3; root package `dstore-client-rs` with
  `[lib] name = "dstore"` (`src/lib.rs`) and `[[bin]] name = "dstore"` (`src/bin/dstore.rs`, `doc = false`).
- [x] `Cargo.lock` committed; `src/lib.rs` facade; `src/bin/dstore.rs` is
  `std::process::exit(dstore_cli::main_entry())`.
- [x] 13 crates under `crates/` with package names `dstore-gocompat`, `-codec`, `-wire`, `-ticket`, `-view`,
  `-transport`, `-transport-iroh`, `-client`, `-udiff`, `-worktree`, `-gocli`, `-cli`, `-testkit`
  (`publish = false`).
- [x] Module files as §4 names them: gocompat `quote strings strconv time json hex base32 errno path os fmt
  ctx slog` plus `tables.rs` (and a private `errno_tables.rs`); wire `consts msg error frame keys pack admin`;
  client `cluster rank batch progress refs watch objects fetch tree corefmt error`; worktree `tree change scan
  merge apply diff flow sys`; cli `app nodeside common cmd_admin cmd_client cmd_wc size progress`; gocli
  `goflag help`.
- [x] `tests/golden.rs`, `tests/golden/` (`.gitkeep`), `tests/cli_snapshots.rs`, `tests/fake_cluster.rs`,
  `tests/iroh_loopback.rs`, `examples/holdlock.rs`.
- [x] `tools/vectorgen` (`go.mod`, `go.sum`, `main.go`, `util.go`, `deps.go`, `docs/`, `cmd/README.md`);
  `flake.nix`, `flake.lock`, `.envrc`, `.gitignore`, `.github/workflows/ci.yml`; `README.md`, `VECTORS.md`,
  `LICENSE` (LGPL-3.0 text), `COPYING` (GPL-3.0 text).
- [ ] `interop/check.sh`, `interop/lib.sh`: absent by design (layer L6). CI's `interop` job fails until they
  land.

### §3.2 dependency edges

- [x] Each crate's `[dependencies]` against the table: gocompat, codec, wire, ticket, view, transport,
  transport-iroh, client, udiff (none), worktree, gocli, cli (every workspace crate except testkit) and the root
  (every crate; testkit and the listed crates as dev-dependencies) match. Reviewed and kept: testkit →
  `dstore-ticket` (`FakeCluster::ticket()` returns `dstore_ticket::Ticket`); root → `amber-store-core` (the
  `core` facade re-export).
- [x] `cargo tree -e normal` of `dstore-client`, `dstore-worktree`, `dstore-testkit` (and `dstore-transport`,
  `dstore-view`, `dstore-ticket`): no `iroh`, `noq` or `dstore-transport-iroh`; only `iroh-base 1.2.0`, through
  `dstore-ticket`, as §3.2 intends.

### §5.10 versions and features, §5.11 core-rs pin

- [x] `[workspace.dependencies]` lists every crate and version of §5.10: `iroh =1.2.0` with default features
  off and `tls-ring` + `fast-apple-datapath`; `iroh-base =1.2.0`; tokio `rt-multi-thread macros sync time
  io-util net signal fs` (root dev-dependency adds `test-util`); socket2 `all`; nix `net`; rustix `fs`; serde
  `derive`.
- [x] `amber-store-core = { git = "https://github.com/amber-store/core-rs", rev = "a85ffa1eb5ed363b9072ab224de179196cd0a046" }`.
- [x] `Cargo.lock` resolves amber-store-core 0.3.0 at that rev, iroh / iroh-base / iroh-relay 1.2.0, noq /
  noq-proto 1.3.0, tokio 1.53.1, tokio-util 0.7.19, futures / futures-core 0.3.34, async-trait 0.1.92,
  async-stream 0.3.6, async-channel 2.5.0, pin-project-lite 0.2.17, thiserror 2.0.20, blake3 1.8.6,
  data-encoding 2.11.1, rand 0.9.2, socket2 0.6.5, nix 0.30.1, libc 0.2.189, xattr 1.6.1, url 2.5.8, crossterm
  0.29.0, hex 0.4.3, serde 1.0.229, serde_json 1.0.151, tempfile 3.27.0, walkdir 2.5.0, rustix 1.1.4, similar
  2.7.0, ring 0.17.14. rustls is 0.23.45: the lock resolved offline, so the §5.10 fallback pin (0.23.43) does
  not apply.
- [x] Edition 2024, `rust-version = "1.91"`, license `LGPL-3.0-only`.
- Reviewed and kept (scaffold notes): iroh-base feature `key` (`PublicKey::from_bytes` is behind it),
  crossterm feature `event-stream` (async key events for the TUI).

### §4 public API, item by item

Every type, field, derive, variant and error text, constant value, trait, function and method signature was
compared with the §4 code blocks.

- [x] §4.1 gocompat: `quote`, `strings`, `strconv` (`NumErrorKind`, `NumError` text), `time` (constants,
  `GoTime` derives and methods, `Zone`, `SystemZone`, `FixedZone`, the formatters and the parser), `json`,
  `hex`, `base32` (alphabets, `CorruptInputError`), `errno` (`PathError` text and `#[source]`), `path`, `os`,
  `fmt`, `ctx` (`Ctx`, `CtxError` texts, methods), `slog` (`Level` constants and `Display`, `Value`, `Attr`,
  `Record`, `Handler`, `Logger`, `TextHandler`, `needs_quoting`, `format_text_record`).
- [x] §4.2 codec: `Enc` (plus `Default`), `Encode`, `marshal`, `CborType` (+ `Display`), `UnmarshalTypeError`,
  `DecodeError` (18 variants, texts verbatim), `Dec` methods, `Field` with the 17 listed impls, `Struct`,
  `unmarshal`, `well_formed`, and the `cbor_struct!` syntax `key => field: Type = "go type" [omitempty]`.
- [x] §4.3 wire: the three ALPNs, the limits, all 54 `T_*` values against `wire/wire.go` (7/8/10 are
  protocol's `TData`/`TDataEnd`/`TErr`), all 21 `CODE_*` strings; `RemoteError`, `error_from_msg`, `err_msg`,
  `as_remote`, `is_code`; `ShortCause`, `WireError` (texts, `#[source]` placement), the frame functions;
  `keys32`, `raw_keys`; `PACK_MAGIC`, `ProtocolRemoteError`, `ProtocolFrameError`, `read_protocol_msg`,
  `PackReadError`, `PackSender`, `PackReader`, `PackRecords`, `PackRecordsError`; `decode_status`,
  `decode_admin_reply`. Everything is also exported at the crate root.
- [x] §4.4 ticket: `PREFIX`, `Ticket::encode/ids/members`, `Display`, `KeyError`, `TicketError` (`NotId` with its
  `source`), `parse`, `parse_endpoint_id`, `is_valid_public_key`.
- [x] §4.5 view: `placement` (`SLOT_BITS` 20 = Go `SlotBits`, `SLOTS`, `NodeId` derives, methods and `From`,
  `Member`, `slot`, `salt`, `fmix64`, `log2fix`, `l`, `Set`, `Table`); `view` (`VOTER_SYNC_*`, `ViewError`
  texts, `View` and `Node` methods, the helper functions, `Placement`); `pub use placement::NodeId`,
  `pub use view::*`; `ticket_from_view`.
- [x] §4.6 transport: the re-exports, `PathInfo`, `SendStream`, `RecvStream`, `Stream`, `Conn`, `Endpoint`,
  `AddrsFn`, `TransportError` (17 variants and texts), the private `joined`, `Pool`, `CallError`, `addr`
  (`GoRelayUrl`, `GoTransportAddr` + `Display`, `is_relay`, the parsers), `mem` (`PIPE_LIMIT`, `Network`,
  `MemEndpoint` impl `Endpoint`).
- [x] §4.7 transport-iroh: `GO_DEFAULT_RELAYS` equal to go-iroh `relay/relay.go` `DefaultMap` ("https://" +
  the canary hostnames with the trailing dot), `RELAY_QUIC_PORT` 7842 = `DefaultQUICPort`, the other 11 timing
  and window constants, `RelayChoice`, `relay_mode_of`, `IrohConfig` fields, `IrohEndpoint` impl `Endpoint`,
  `raw`, `close_bounded`, `IrohConn` impl `Conn`, `generate_secret_key`, `to_iroh_addr`, `go_relay_string`,
  `mdns` (`SERVICE_NAME`, `endpoint_label`, `build_query`, `Announcement`, `parse_announcement`,
  `MdnsResolver::start/resolve`), `ifaces` (`BRIDGE_PREFIXES` equal to `transport/ifaces.go` `bridgePrefixes`,
  `interface_ips`).
- [x] §4.8 client: the re-exports, the 8 constants, `Config` (+ `Default`), the public `Cluster` methods and the
  13 `pub(crate)` seams (`cfg log pool stamp call call_retry any_node handle_err ok penalty preferred
  probe_hinted path_attrs`), `rtt_class`, `rank_owners`, `RecordSizer`, `batches`, `Progress`, `ProgressReport`,
  `NodeProgress`, `PutObserver`, `pub(crate) Tracker` (`new observer totals more objects bytes`), `count_keys`,
  `human_bytes`, `rate`, `Ref`, `Cond`, `CasMismatch`, `Incomplete`, `ref_get/put/delete/list`,
  `validate_name_bytes`, `validate_user_bytes`, `RefChange`, `WatchStream`, `watch_refs`, `RecordSource`,
  `MissingResult`, `PutResult`, `GetResult`, `GetStream` (`Stream` impl, `missing`), `missing/put/placed/get`,
  `verify_record`, `est_size`, `PushStats`, `PullStats` (+ `Default`), `push/pull/pull_tree`, `stored_size_of`,
  `corefmt`, `Error` (33 variants and texts), its predicates, `From<CallError>`.
- [x] §4.9 udiff: `unified`, `Edit`, `lines`, `to_unified`, `lcs::Diff`, `lcs::diff_lines`,
  `gosort::slice/slice_stable/is_sorted`.
- [x] §4.10 worktree: the `ticket_from_view` re-export, `DIR`, `RACY_WINDOW_NS`, `MAX_DIFF_BYTES`, `Config`,
  `State`, `Tree`, `Kind` (+ `Display`), `Change`, `Conflict`, `FetchResult`, `PullResult`, `PushResult`,
  `RemoteState`, `Status`, `Getter`, `Error` (17 variants and texts) and its predicates, the tree, change, scan,
  merge, apply, diff and flow functions and methods, `sys`.
- [x] §4.11 gocli: `FlagKind`, `FlagDef`, `Action`, `CommandDef`, `AppDef`, `FlagValue`, the `Context`
  methods, `CliError` (+ `msg`), `run`, `goflag`, `help`.
- [x] §4.12 cli: `VERSION`, `main_entry`, `run`, `app`, `go_panic_exit`, `nodeside` (`node_side_error`,
  `STORE_TICKET_UNSUPPORTED` and `CATALOG_RESTORE_UNSUPPORTED` verbatim per §2.2, `local_ticket`), `size`,
  `common`, `progress`.
- [x] §4.13 testkit: `splitmix`, `golden` (`Payload` untagged, the seed read as a decimal string or a number, as
  util.go writes it), `refglob`, `fake`.
- [x] §4.14 facade: the 11 re-exports verbatim.

### CBOR structs against the Go tags

Keys, Rust field types (§3.4: a non-omitempty `[]byte` → `Option<Vec<u8>>`, a non-omitempty `[]T` →
`Option<Vec<T>>`, `*T` → `Option<Box<T>>`), Go type strings (reflect names such as `"int"`, `"[]uint8"`,
`"[][]uint8"`, `"[]wire.RefInfo"`, and `"view.Pending"` for `*Pending`, which view-placement's vector
`view.View.11 of type view.Pending` confirms) and omitempty flags, field by field:

- [x] `wire/wire.go`: `Msg` (keys 0-61), `KeyHolders`, `KeyFailure`, `KeyReject`, `RefInfo`, `ScanRow`.
- [x] transport-iroh v0.4.0 `protocol/protocol.go`: `Msg` (0-17), `RefInfo`, `DataEndpointRec`.
- [x] `node/admin.go`: `AdminRequest` (0-14), `AdminReply` (0-6).
- [x] `node/status.go`: `VoterStat` (0-3), `Status` (0-28).
- [x] `ticket/ticket.go`: `Member`, `Ticket`.
- [x] `view/view.go`: `Voter`, `Former`, `DataEndpoint`, `Node`, `Ramp`, `ACL` (Rust `Acl`, `GO_NAME`
  `"view.ACL"`), `Pending`, `View`; `VoterSyncDone`/`VoterSyncPending`.

No disagreement between the Go tags and PORTING.md §4 was found, so nothing changed here.

### §8 flake and CI

- [x] `flake.lock` is byte-identical to core-rs's (nixpkgs `445d861c…`, systems).
- [x] `packages.dstore` / `default`: `buildRustPackage` over `lib.fileset.unions [ ./Cargo.toml ./Cargo.lock
  ./src ./crates ./tests ./examples ]`, `outputHashes."amber-store-core-0.3.0"` =
  `sha256-ZnXXnztVqGXq5ILG29kkJIIRo9/TTW3XqULx1chbLXA=`, `cargoBuildFlags = [ "-p" "dstore-client-rs" "--bin"
  "dstore" ]`, `doCheck = false`.
- [x] `checks`: `dstore`; `fmt` (`rustfmt --check --edition 2024` over `src crates tests examples`); `clippy`
  (`--workspace --all-targets --offline -- -D warnings`); `tests` (`--workspace --lib --test golden --test
  cli_snapshots --test fake_cluster`, `TZ = "UTC"`).
- [x] `devShells.default`: cargo, rustc, rustfmt, clippy, rust-analyzer, go, gopls; `hardeningDisable = [ "all"
  ]`; `GOTOOLCHAIN = "local"`; `CGO_ENABLED = "0"`; `RUST_SRC_PATH`. In the shell: cargo and rustc 1.95.0, clippy
  0.1.95, go1.26.5, rust-analyzer and gopls on `PATH`, both variables set.
- [x] `formatter` (`cargo fmt --all`), `.envrc` (`use flake`), `.gitignore` (`target/`, `result`, `result-*`,
  `.direnv/`).
- [x] `ci.yml`: jobs `rust` (ubuntu and macos, toolchain 1.95.0, fmt, clippy `--workspace --all-targets
  --locked -D warnings`, `cargo test --workspace --locked` with `TZ=UTC`), `vectors` (`go vet`, regenerate twice
  and diff, diff against `tests/golden`, `gotables` diff, `clisnap` diff), `interop` (dstore v0.1.9 checkout,
  release build with examples, `interop/check.sh`, logs on failure, heavy and chaos on `workflow_dispatch`),
  `nix` (`nix flake check -L`, `nix build .#dstore -L && ./result/bin/dstore --version`).

### tools/vectorgen

- [x] `go.mod`: module path, `go 1.26.5`, requires dstore v0.1.9, core v0.0.8, transport-iroh v0.4.0, go-iroh
  v0.2.0, cbor v2.9.3, go-udiff v0.4.1, urfave/cli v2.27.7, bubbletea v2.0.9, lipgloss v2.0.6, bubbles v2.2.1,
  zeebo/blake3 v0.2.4; `go.sum` from an online tidy.
- [x] `util.go`: splitmix64 as VECTORS.md defines it, `U64`/`I64` decimal strings, `Hex` (nil → `null`),
  `Payload` `{hex}` | `{seed, len}`, `writeJSON` (`MarshalIndent`, two spaces, trailing `\n`).
- [x] Registry semantics: fixed (deviation 1).

### tests and testkit

- [x] testkit `splitmix` and `golden` are implemented, with unit tests. The splitmix64 reference values were
  re-derived with an independent Go program: `u64s(0|42|2^64-1, 3)`, `data(1, 20)`, `data(7, 3)` all match.
- [x] `golden::load_json` panics when a file is missing (unit test `load_json_fails_on_a_missing_file`).
- [x] `tests/golden.rs` module list: fixed (deviation 2).

## Deviations fixed

1. **vectorgen did not delete what a family owns.** PORTING.md §7 ("deletes exactly what it owns and
   regenerates") and verification.md §4.2, following core-rs `tools/vectorgen/main.go`, require stale outputs to
   disappear on regeneration. The scaffold only replaced the files a run produced. Now:
   - `register(name, owns, gen)` names the owned paths (clean relative slash paths, files or directories).
   - Registration refuses bad paths, paths overlapping another family's (or the family's own), and paths
     covering `cli/snapshots.json` (from `cmd/clisnap`) or `.gitkeep`.
   - A run checks the staged output first (at least one file under every owned path, nothing outside), so a
     failing generator leaves `<out-dir>` untouched; then it deletes the owned paths and copies the new files.
   - `tools/vectorgen/main_test.go` covers stale-file removal, untouched unowned files, output checks and the
     registration refusals. VECTORS.md "The generator" and impl-scaffold.md are updated.
2. **The golden module layout deviated from §3.1 without a record, and parallel owners shared modules.**
   §3.1 says "a `mod` per vector family". The scaffold made one module per owner group and did not record it,
   and four groups were shared by owners that §6 runs in parallel: `wire` (wire, wire-pack in L2), `client`
   (client-a, client-b in L4), `transport` (transport-iroh, transport-iroh-mdns in L4) and `cli` (cli-app,
   cli-progress in L3). A literal module per family would be worse for parallel work: `text/formats.json` alone
   is read by seven owners. Kept the owner groups, added `wire_pack`, `client_transfer`, `transport_iroh`,
   `transport_mdns` and `cli_progress`, and recorded the rule in `tests/golden.rs` and VECTORS.md ("Where tests
   live"). Each module's doc comment names the vector files it reads.

## Other fixes

3. **`cbor_struct!` did not enforce ascending keys.** §4.2 requires fields in ascending key order, and the
   generated encoder writes fields in declaration order, so out-of-order keys would silently produce
   non-canonical CBOR. The expansion now carries a const assertion; a `compile_fail` doctest locks it.
4. **The root crate lacked `#![deny(unsafe_op_in_unsafe_fn)]`** (§3.4 conventions for every crate). Added to
   `src/lib.rs`.

## Reviewed and kept

The scaffold's recorded additions: testkit → ticket; iroh-base `key` and crossterm `event-stream` features;
derives on `mdns::Announcement` and `splitmix::SplitMix64`; `impl Default for Enc`; `pub(crate) MemConn`;
`golden::decimal_u64`/`decimal_i64`; the lint allows on `Cluster::push` and `view::add_id`; crate-level
`#![allow(dead_code, unused_variables)]` for the stubs; the precise lock pins of blake3 and rustix.

## Gates

Run inside `nix develop /Users/dragan/amber-store/dstore-client-rs` with a private `CARGO_TARGET_DIR`, on the
final tree:

- `cargo fmt --all --check`: clean.
- `cargo check --workspace --all-targets --locked`: passes, no warnings.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: clean.
- `cargo test -p dstore-testkit --locked`: 10 passed.
- `cargo test --workspace --locked`: passes. testkit has 10 unit tests; codec has 2 doctests, one of them the new
  `compile_fail` test; every other test binary and doctest has 0 tests.
- The `compile_fail` doctest fails for the intended reason. Compiling the unordered struct against the built
  `libdstore_codec` gives `error[E0080]: evaluation panicked: cbor_struct!: field keys must be strictly
  ascending`, and the same struct with ascending keys compiles.
- `go -C tools/vectorgen build -o /dev/null .`, `go vet ./...`, `go test -count=1 ./...` (3 tests),
  `gofmt -l tools/vectorgen`: pass, clean.
- `nix flake show`: `packages` (`dstore`, `default`), `checks` (`dstore`, `fmt`, `clippy`, `tests`),
  `devShells.default` and `formatter` evaluate.
- `nix develop -c cargo --version`: cargo 1.95.0. `nix develop -c go version`: go1.26.5 darwin/arm64.

Not run in this pass: `nix flake check` and `nix build .#dstore`. The scaffold agent built `.#dstore`; since then
only Rust sources, tests and docs changed, and `cargo check` covers them.
