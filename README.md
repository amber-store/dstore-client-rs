# dstore-client-rs

A Rust port of the client side of [dstore](https://github.com/amber-store/dstore) v0.1.9: the client
library (the Go packages `client`, `codec`, `wire`, `ticket`, `view`, `placement`, `transport` and
`worktree`) and the `dstore` command-line interface, built to be 100% compatible with Go dstore. A Rust
client talks to Go dstore nodes over `amber-dstore/1`, shares working copies and local stores with Go
processes, and prints the same texts with the same exit statuses.

It builds on [core-rs](https://github.com/amber-store/core-rs) (`amber-store-core` 0.3.0) for trees,
records and local stores, and on [Rust iroh](https://github.com/n0-computer/iroh) 1.2.0 for QUIC.

The contract every change follows is [`PORTING.md`](PORTING.md). The per-area specs are in
[`port-notes/`](port-notes/).

**Status: scaffold.** Every crate and public API of PORTING.md §4 exists, with `todo!()` bodies. The layers
of PORTING.md §6 implement them.

## Compatibility contract

The full contract is PORTING.md §1. In short:

**Byte-identical** with Go. Golden vectors from the Go libraries and snapshots of the Go binary lock these:

- client-produced CBOR request frames, and pack framing cut at exact 1 MiB offsets;
- decoding decisions and error texts for `wire.Msg`, `protocol.Msg`, `view.View`, `ticket.Ticket`,
  `node.AdminReply` and `node.Status`;
- tickets (`Encode`, `IDs`, what `Parse` accepts and its error texts);
- placement (slots, salts, rank, owners, write set, read order);
- the working-copy files `.dstore/config` and `.dstore/state`;
- `dstore diff` output, go-udiff's quirks included;
- the CLI surface: commands, aliases, flags, defaults, environment variables, help output, usage errors,
  prompts, stdout texts, `dstore: <err>` lines and exit statuses;
- slog lines, progress lines and the TUI model's frame;
- Go number, duration and time formats;
- raw records.

**Interoperable** but not compared byte for byte:

- a Rust client against Go nodes, over direct paths, relays, number0 DNS discovery and id-only tickets
  over mDNS;
- packstores used by Go and Rust processes in turn;
- working copies created by one implementation and used by the other;
- records pushed by either side, and zstd records decoded by both.

**Different by design** (PORTING.md §1.3, DD-1 to DD-14). The main differences:

- Newly compressed records differ (libzstd against klauspost). This shifts byte counts for trees that Rust
  ingests locally.
- `store push/pull --local` keeps references in redb (see below).
- Node-side commands fail with a fixed message (see below).
- Transport error texts below dstore's wrappers, and connect timing, follow Rust iroh.
- The TUI's terminal bytes differ; only the model's frame is identical.
- Go runtime panics are reproduced as their first line and exit status 2.
- Where Go iterates a map in random order, Rust uses a deterministic order.

**Go quirks reproduced on purpose** (PORTING.md §1.4). urfave/cli's parsing and help quirks, validation
orders, dstore v0.1.9 client behaviours and CLI text details stay as Go has them. Do not "fix" them.

## Node-side commands

dstore-client-rs does not implement the dstore node, which needs the Pebble meta store and the paxos
acceptor. The node-side commands keep Go's definitions, flags, help and pre-store validation. Then they
fail with `dstore: <text>` and exit status 1, without touching the filesystem:

- `serve`, `cluster init` and `node join`:
  `<cmd> is a node-side command and dstore-client-rs does not implement the dstore node; use the Go dstore binary (github.com/amber-store/dstore v0.1.9)`
- `cluster status`, `cluster ticket` and `catalog restore` given `--store` without `--ticket`, after reading
  `<store>/identity`:
  `deriving a ticket from --store needs the node's Pebble meta store, which dstore-client-rs does not implement; pass --ticket or $DSTORE_TICKET, or use the Go dstore binary`
- `catalog restore`, after fetching the backup:
  `catalog restore writes through the node's paxos acceptor, which dstore-client-rs does not implement; use the Go dstore binary`

Use the Go `dstore` binary for these commands.

## Local references for `store push/pull --local DIR`

Go and Rust share `DIR/packstore`. The local references differ:

- Go keeps them in a Pebble database under `DIR/refs`.
- Rust keeps them in redb, in `DIR/refs/refs.redb`, because there is no Rust Pebble.

When `DIR/refs` holds a Pebble database written by Go, Rust refuses to open it:

`refstore: <DIR>/refs holds a Pebble database written by Go dstore; dstore-client-rs keeps local references in redb and cannot open it (use another --local directory)`

The reverse does not fail loudly. Go dstore run on a directory Rust wrote does not see Rust's references: it
silently creates a second database, a Pebble one, next to `refs.redb`. Use separate `--local` directories
for Go and Rust.

## Workspace

| Crate | Content |
|---|---|
| `dstore-gocompat` | Go standard-library behaviour: strconv, strings, time, json v1, hex, base32, errno texts, paths, `Ctx`, slog |
| `dstore-codec` | fxamacker/cbor v2.9.3-compatible CBOR and `cbor_struct!` |
| `dstore-wire` | `wire.Msg`, frames, remote errors, pack framing, admin and status payloads |
| `dstore-ticket` | `dstore1` tickets, go-iroh endpoint-id parsing |
| `dstore-view` | `NodeId`, placement, views, `Placement`, `ticket_from_view` |
| `dstore-transport` | `Endpoint`/`Conn`/`Stream` traits, `Pool`, Go address strings, in-memory network |
| `dstore-transport-iroh` | the Rust iroh endpoint, dial phases, discovery, a port of go-iroh's mDNS resolver |
| `dstore-client` | the client: dial, view cache, references, watch, put and get, push and pull |
| `dstore-udiff` | go-udiff v0.4.1 unified diffs with Go's pdqsort |
| `dstore-worktree` | working copies |
| `dstore-gocli` | a urfave/cli v2.27.7-compatible CLI framework |
| `dstore-cli` | `cmd/dstore`: the command table, actions, progress and TUI |
| `dstore-testkit` | test support: splitmix64, golden loaders, refglob, fake cluster (not published) |

The root package `dstore-client-rs` provides the `dstore` library facade (`dstore::client`,
`dstore::worktree`, …) and the `dstore` binary.

## Building and developing

The Nix flake provides the toolchain (rustc 1.95, Go 1.26.5):

```sh
nix develop                 # or direnv, through .envrc
cargo build --bin dstore
cargo test --workspace      # TZ=UTC for the CLI snapshots
cargo clippy --workspace --all-targets -- -D warnings
nix build .#dstore          # the CLI through rustPlatform.buildRustPackage
nix flake check             # fmt, clippy and the socket-free test suites
nix fmt                     # cargo fmt --all
```

`tools/vectorgen` is a Go module that generates the golden vectors under `tests/golden/` from the Go
implementation. See [`VECTORS.md`](VECTORS.md):

```sh
nix develop -c go -C tools/vectorgen run . ../../tests/golden              # every family
nix develop -c go -C tools/vectorgen run . ../../tests/golden FAMILY...    # only these families
```

A missing vector file fails its test. The live Go↔Rust interop harness is `interop/check.sh`
(PORTING.md §7).

`dstore --version` prints `dev` unless the build sets `DSTORE_VERSION`.

## License

LGPL-3.0-only. See [`LICENSE`](LICENSE) for the LGPL terms and [`COPYING`](COPYING) for the GPL terms the
LGPL incorporates. core-rs, which this port links and copies helpers from, has the same license.
