# impl-scaffold: layer L0 notes

Owner: scaffold (L0). The workspace, every crate and module of PORTING.md §4 with `todo!()` bodies, the
`cbor_struct!` macro, the Nix flake, CI, the vectorgen Go module, README.md and VECTORS.md. Only
`dstore-testkit` `splitmix` and `golden` are implemented (with unit tests).

## Additions beyond PORTING.md (nothing in §4 was changed)

1. **`dstore-testkit` depends on `dstore-ticket`.** §3.2 omits it, but `FakeCluster::ticket()` returns
   `dstore_ticket::Ticket`.
2. **`crossterm` has the `event-stream` feature.** §5.10 lists none; cli.md §4.9 needs async key events
   (`EventStream`). Drop it if the progress owner reads events synchronously.
3. **`iroh-base` has the `key` feature.** `PublicKey::from_bytes` (ticket curve check) is behind `key`; the
   crate's default features only enable `relay`, and `dstore-ticket` does not depend on `iroh`.
4. **Derives added:** `mdns::Announcement` (Clone, Debug, PartialEq, Eq), `splitmix::SplitMix64` (Clone,
   Debug, PartialEq, Eq). **Impl added:** `impl Default for dstore_codec::Enc` (calls `Enc::new`; clippy
   `new_without_default`).
5. **`dstore_testkit::golden` helpers:** `decimal_u64` and `decimal_i64` (serde `deserialize_with` for 64-bit
   integers written as decimal strings, numbers also accepted). `Payload` reads `{"hex": "…"}` or
   `{"seed": S, "len": N}` with `S` a decimal string or a number. The Go side is `util.go` `Inline`, `SM`,
   `U64`, `I64`, `Hex`.
6. **Lint allows on fixed signatures:** `Cluster::push` has `#[allow(clippy::too_many_arguments)]`;
   `view::add_id` has `#[allow(clippy::ptr_arg)]` until its body appends. `gocompat::strings::fields_func`
   is written with elided lifetimes (the same signature; clippy `needless_lifetimes`).
7. **`transport::mem::MemConn`** is a `pub(crate)` type implementing `Conn` (§4 lists only `MemEndpoint`).

## Layout decisions owners code against

- **Glob re-exports, no lib.rs edits needed.** `codec`, `transport`, `transport-iroh`, `client`, `udiff`,
  `worktree` and `gocli` have private modules re-exported with `pub use module::*`. A new `pub` item in
  your module is exported at the crate root automatically. `wire` has `pub mod`s plus the same globs, so
  `dstore_wire::admin::AdminRequest` and `dstore_wire::AdminRequest` both work. `gocompat`, `view`, `cli`
  and `testkit` expose `pub mod`s as §4 shows.
- **Where items live.**
  - transport: `traits.rs` (PathInfo, SendStream, RecvStream, Stream, Conn, Endpoint, AddrsFn), `error.rs`
    (TransportError, `joined`, CallError), `pool.rs`, `addr.rs`, `mem.rs`.
  - transport-iroh: constants in `lib.rs`; `endpoint.rs` (RelayChoice, relay_mode_of, IrohConfig,
    IrohEndpoint, bind_iroh, generate_secret_key, to_iroh_addr, go_relay_string); `conn.rs` (IrohConn).
  - client: constants in `lib.rs`; `corefmt` is a `pub mod`.
  - worktree: constants in `lib.rs`; `tree.rs` (Config, State, Tree, RemoteState, Status, Getter,
    empty_tree, find, remove, Tree::open…status); `change.rs` (Kind, Change, type_name, is_dir,
    same_content, equivalent, compare, diff_trees); `merge.rs` (Conflict, merge); `scan.rs`; `apply.rs`;
    `diff.rs` (SourceError, Source, TreeSource, DiskSource, unified, stat); `flow.rs` (FetchResult,
    PullResult, PushResult, Tree::fetch/pull/push/refresh_ticket, clone, init).
  - gocli: `flag.rs` (FlagKind, FlagDef, FlagValue), `context.rs` (Context), `app.rs` (Action, CommandDef,
    AppDef, CliError, run); `goflag` and `help` are empty `pub mod`s.
  - cli: VERSION, main_entry, run, go_panic_exit in `lib.rs`; `app()` in `app.rs`, re-exported.
- **Private fields** (`Ctx`, `Logger`, `TextHandler`, `Pool`, `Network`, `MemEndpoint`, `IrohEndpoint`,
  `MdnsResolver`, `Cluster`, `Tracker`, `GetStream`, `Context`, `UiModel`, `FakeCluster`, `PackReader`,
  `PackRecords`, `Set`, `Table`, `Placement`, …) are guesses at the described state. Change them freely.

## `cbor_struct!` internals (codec owner)

The syntax is fixed (§4.2). The expansion is only a first cut:

- `Encode::encode` counts present fields (non-omitempty always; omitempty unless
  `Field::is_empty_field`), writes `e.head(5, n)`, then `e.uint(key)` and `Field::encode_field` per present
  field.
- `Struct::decode_struct` starts from `Default` and calls
  `d.read_map_struct(GO_NAME, |d, key| match key { K => field = <Ty as Field>::decode_field(d, "go type")?; Ok(true), _ => Ok(false) })`.
- The "<GO_NAME>.<key>" error rewrite is not in the expansion; put it in `read_map_struct` or change the
  internals.
- `Ticket::encode()` and `View::encode()` are inherent methods, so method-call syntax on those concrete
  types picks them over `Encode::encode`. Concrete code should call `Encode::encode(v, e)`.
- The rustdoc example in `macros.rs` is a doctest that compiles the macro from outside the crate.

## Toolchain and lockfile

- Cargo.lock came from `cargo generate-lockfile --offline`, then `cargo update --offline --precise` for
  blake3 1.8.6 and rustix 1.1.4 to match §5.10. tokio 1.53.1, iroh 1.2.0, noq 1.3.0.
- The user's git config rewrites `https://github.com/` to ssh, so cargo's built-in git fetch of core-rs fails
  with an ssh auth error. The repo is now in `~/.cargo/git`. If cargo ever refetches it, set
  `CARGO_NET_GIT_FETCH_WITH_CLI=true`.
- `tools/vectorgen/go.sum` came from an online `go mod tidy` (default proxy).

## Nix

- `nix build .#dstore` passes. The core-rs `outputHashes."amber-store-core-0.3.0"` of §5.11
  (`sha256-ZnXXnztVqGXq5ILG29kkJIIRo9/TTW3XqULx1chbLXA=`) is confirmed by the vendoring step. No extra
  darwin inputs were needed.
- `nix flake check` (the `fmt`, `clippy` and `tests` checks) has not been run in L0.
- The flake reads the version from `cargoToml.workspace.package.version`.

## vectorgen command semantics

Superseded by scaffold-verify (see `impl-scaffold-verify.md`): families register with
`register(name, owns, gen)`, and `vectorgen` deletes exactly the paths a family owns before copying its new
files in, as core-rs vectorgen and PORTING.md §7 require. VECTORS.md "The generator" has the rules.

The golden test modules were also split per PORTING.md §6 owner (`wire_pack`, `client_transfer`,
`transport_iroh`, `transport_mdns`, `cli_progress` added); `tests/golden.rs` lists them.
