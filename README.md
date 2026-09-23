# dstore-client-rs

A Rust port of the client side of [dstore](https://github.com/amber-store/dstore) v0.1.11: the client
library (the Go packages `client`, `codec`, `wire`, `ticket`, `view`, `placement`, `transport` and
`worktree`) and the `dstore` command-line interface, built to be 100% compatible with Go dstore. A Rust
client talks to Go dstore nodes over `amber-dstore/1`, shares working copies and local stores with Go
processes, and prints the same texts with the same exit statuses.

It builds on [core-rs](https://github.com/amber-store/core-rs) (`amber-store-core` 0.7.0) for trees,
records and local stores, and on [Rust iroh](https://github.com/n0-computer/iroh) 1.2.0 for QUIC. The nodes
stay Go: dstore-client-rs has no node (see "Node-side commands").

The contract every change follows is [`PORTING.md`](PORTING.md). The per-area specs, and the notes of every
implementer and reviewer, are in [`port-notes/`](port-notes/).

## Build, install and run

With [Nix](https://nixos.org) (flakes enabled):

```sh
nix run .#dstore -- --help            # build and run without installing
nix build .#dstore                    # the CLI in ./result/bin/dstore
nix profile install .#dstore          # install it into your profile
nix develop                           # rustc 1.95, cargo, clippy, rustfmt, rust-analyzer, go 1.26.5, gopls
```

`.envrc` holds `use flake`, so [direnv](https://direnv.net) enters the dev shell by itself. From outside a
checkout, use `github:amber-store/dstore-client-rs#dstore` instead of `.#dstore`.

Without Nix, you need Rust 1.91 or later and a C compiler (for zstd, ring and blake3):

```sh
cargo build --release --locked --bin dstore     # target/release/dstore
cargo install --locked --path .
```

`dstore --version` prints `dstore version dev` unless the build sets `DSTORE_VERSION`
(`DSTORE_VERSION=v0.1.0 cargo build …`).

## Usage

The commands, flags and environment variables are those of Go dstore v0.1.11. `dstore --help` and
`dstore <command> --help` list them.

### A cluster

The nodes are Go dstore processes. Create and join them with the Go binary
([github.com/amber-store/dstore](https://github.com/amber-store/dstore) v0.1.11); every other command works
with either binary:

```
# first node (Go): creates the cluster and prints its ticket
dstore cluster init --store /srv/n1 --replicas 3 --weight auto
dstore serve --store /srv/n1

# more nodes (Go): a single-use token per node, then join
dstore token create --ticket dstore1…
dstore node join --store /srv/n2 --seed dstore1… --token <hex> --weight auto

# operations (either binary)
dstore cluster status --ticket dstore1…
dstore cluster ticket --ticket dstore1… [--ids]
dstore cluster replicas --ticket dstore1… [--yes] R   # flags go before the arguments
dstore node remove ID | drain ID | weight ID GiB | zone ID Z | repair ID
dstore voter add ID | remove ID
dstore transition status | abort | refreeze | pause | resume
dstore gc run | status | why KEY | hold | release
dstore catalog backup | backups                 # catalog restore runs on a Go voter
```

### Client commands

```
# --ticket takes the ticket or the ids `cluster ticket --ids` prints; $DSTORE_TICKET stands in for it
dstore store push --ticket dstore1… --local ~/.amber ./tree trees/demo   # from a standalone local store
dstore store pull --ticket dstore1… --local ~/.amber trees/demo
dstore refs --ticket dstore1… [PREFIX]
dstore watch --ticket dstore1… 'trees/**'    # prints each change until Ctrl+C
dstore ref get --ticket dstore1… trees/demo  # name, key, version, user, created
dstore ref delete --ticket dstore1… trees/demo
dstore ls  --ticket dstore1… trees/demo sub
dstore cat --ticket dstore1… trees/demo hello.txt
dstore refs --ticket 3f9a…,b71c…             # member ids, found by discovery
```

- `--local` (`$AMBER_STORE`) is a directory holding `packstore/` and `refs/`; see "Local references" below.
- `store push --expected-version HEX` writes the reference under CAS against the version `ref get` printed.
  Without it the name must be new; `--force` replaces it unconditionally.
- A client that has only member ids finds their addresses by discovery: mDNS on the local link and, when
  relays are enabled, number0's DNS service. `--no-discovery` (`$DSTORE_NO_DISCOVERY`) turns that off,
  `--no-relay` uses direct addresses only, and `--relay URL` selects a relay.
- `push` and `pull` show a progress display when stderr is a terminal; `--no-tui` (`$DSTORE_NO_TUI`) prints
  plain log lines and a status line every five seconds instead. `--log-level` (`$DSTORE_LOG_LEVEL`) is a
  global flag and goes before the command.

### Working copies

A reference can be worked on like a git branch:

```
dstore clone --ticket dstore1… trees/demo [DIR]   # DIR defaults to "demo"
cd demo
dstore status                    # new, modified, deleted, type and mode changes since the last sync
dstore diff [--stat] [PATH…]     # unified diffs against the last synced tree
dstore fetch                     # learn the cluster's current tree
dstore diff --incoming           # what pull would apply; --remote: against the fetched tree
dstore pull [--force]            # apply the cluster's changes, keeping local ones
dstore push [--force] [--user U] [-m MSG]  # build, upload, write the reference under CAS
dstore init --ticket dstore1… trees/new   # make an existing directory a working copy; push creates the reference
```

`.dstore/` holds a packstore, the config (ticket, name, connection flags, user), the state (the tree last
synced and the tree last fetched, with its version) and the lock file. `status` and `diff` work offline. A stored ticket
takes precedence over `$DSTORE_TICKET`, and `--ticket` overrides it for one run. `push` refuses when the
reference moved since the last fetch; `pull` refuses a path changed on both sides unless `--force`. One
command at a time runs in a working copy: `.dstore/lock` is held while one runs, and a second fails with
`working copy <root>: in use by another dstore command`. Go and Rust use the same working copies: the files
in `.dstore/` are byte-identical, and both take that lock, so the two keep each other out as well.

A reference that names a commit (core object type 5, made by `amber-store commit create` or by `push -m`)
is a branch. Clone, fetch and pull take the commit's tree and bring its history along. Each `push` on a
branch records a new commit with the fetched one as parent, the user as author and `-m` as the message, so
the cluster keeps the whole history alive. A message on a plain reference turns it into a branch. `ls` and
`cat` accept a branch too. The state file then also holds `remote_commit`.

A directory entry may hold a commit (core v0.0.10) and reads as the commit's tree. A working copy shows it
as a plain directory, and a push writes it back as one, without the commit.

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
- packstores used by Go and Rust processes, in turn or at the same time (since core v0.0.10 any number of
  processes share one store);
- the local references of `store push/pull --local`, one SQLite file that both read and write;
- working copies created by one implementation and used by the other;
- records pushed by either side, and zstd records decoded by both.

**Different by design** (PORTING.md §1.3):

| # | Difference |
|---|---|
| DD-1 | Newly compressed records differ (libzstd against klauspost). This shifts byte counts, progress totals and put-batch boundaries for trees that Rust ingests locally. |
| DD-2 | A `--local` directory whose references are still the Pebble database of Go dstore v0.1.10 or earlier is refused until Go has imported it (see "Local references"). |
| DD-3 | The node-side commands fail with a fixed message after Go's validation (see "Node-side commands"). |
| DD-4 | Transport error texts below dstore's wrappers, and connect timing, follow Rust iroh. The wrapper texts are identical. |
| DD-5 | An RTT sample of exactly 333 ms (noq's initial RTT) reads as "not measured". |
| DD-6 | The TUI's terminal bytes differ; only the model's frame is identical. |
| DD-7 | Go runtime panics (`cat NAME /`, `cluster status` with a cluster id shorter than 4 bytes) print the panic's first line and exit with status 2, without a goroutine dump. |
| DD-8 | A non-UTF-8 argument placed in a CBOR text field is sent lossily (U+FFFD), and non-UTF-8 arguments and paths in error messages print lossily. Stdout stays byte-exact. |
| DD-9 | Under a umask other than 022, packstore segment files get `0666 &^ umask` (Go: `0644 &^ umask`), until core-rs is patched. |
| DD-10 | Where Go iterates a map in random order, Rust uses a deterministic order. |
| DD-11 | At exit the CLI waits up to 3 s for the endpoint to close. |
| DD-12 | Rare core-rs error texts are not re-rendered. |
| DD-13 | A `--relay` value that Go's `url.Parse` accepts but Rust's `url` rejects is accepted, and no relay is dialled, which is what Go's unusable relay amounts to. |
| DD-14 | The Unicode tables are those of go1.26.5. |
| DD-15 | `cluster status` treats a cluster id shorter than 4 bytes as DD-7, where Go, for a hand-crafted indefinite-length id, prints it zero-padded. |

**Go quirks reproduced on purpose** (PORTING.md §1.4). urfave/cli's parsing and help quirks, validation
orders, dstore v0.1.11 client behaviours and CLI text details stay as Go has them. Do not "fix" them.

### Node-side commands

dstore-client-rs does not implement the dstore node, which needs the Pebble meta store and the paxos
acceptor. The node-side commands keep Go's definitions, flags, help and pre-store validation. Then they
fail with `dstore: <text>` and exit status 1, without touching the filesystem:

- `serve`, `cluster init` and `node join`:
  `<cmd> is a node-side command and dstore-client-rs does not implement the dstore node; use the Go dstore binary (github.com/amber-store/dstore v0.1.11)`
- `cluster status`, `cluster ticket` and `catalog restore` given `--store` without `--ticket`, after reading
  `<store>/identity`:
  `deriving a ticket from --store needs the node's Pebble meta store, which dstore-client-rs does not implement; pass --ticket or $DSTORE_TICKET, or use the Go dstore binary`
- `catalog restore`, after fetching the backup:
  `catalog restore writes through the node's paxos acceptor, which dstore-client-rs does not implement; use the Go dstore binary`

Use the Go `dstore` binary for these commands.

### Local references for `store push/pull --local DIR`

Go and Rust share all of `DIR`: the objects in `DIR/packstore`, and since Go dstore v0.1.11 (core v0.0.10,
core-rs 0.7.0) the local references too, in `DIR/refs/refs.sqlite`. Any number of processes of either
implementation may have a directory open at once.

Earlier releases kept the references elsewhere, and each side imports only its own:

- A `DIR/refs/refs.redb` of an earlier dstore-client-rs is imported on the first open. The old file is kept in
  `DIR/refs/redb-migrated/`, and a poison file takes its place so that an older build cannot start an empty
  store there. Open such a directory with dstore-client-rs before Go touches it: Go knows nothing of
  `refs.redb`. If Go was first, the import merges the old references into Go's database and overwrites
  nothing.
- A Pebble database of Go dstore v0.1.10 or earlier is imported by Go dstore v0.1.11 on its first open. Rust
  cannot import Pebble and refuses the directory until Go has:

  `refstore: <DIR>/refs holds a Pebble database written by Go dstore v0.1.10 or earlier; dstore-client-rs cannot import it: open the --local directory once with Go dstore v0.1.11 or later, which does`

### Commits made by dstore v0.1.10

Commit keys follow core v0.0.10: the length field is the commit's footprint, its own bytes plus the length
field of every tree it records. Commits made by dstore v0.1.10, Go or Rust (core v0.0.9, core-rs 0.4.0), are
keyed by their own length alone, and this release refuses them, as Go dstore v0.1.11 does: nodes neither
store them nor accept a reference on them, clients do not read through them
(`commit <key>: length field N is not the commit's footprint M …; a commit keyed by an older rule has to be
created again`), and while a reference names one a GC epoch aborts, naming the missing keys, with nothing
swept.

**Before upgrading** a cluster that holds such branches, clone each into a working copy; afterwards nothing
reads them. Upgrade every node and every client before pushing to a branch again: a v0.1.10 node refuses the
new commits as a v0.1.11 node refuses the old. Then start each branch again: `dstore ref delete NAME`, and
in its working copy `dstore fetch` followed by `dstore push --force -m MESSAGE`. History recorded by v0.1.10
does not carry over, and a reference deleted without a working copy leaves its tree to the next GC.

## Rust iroh 1.2.0 and the interop evidence

The QUIC stack is Rust iroh `=1.2.0` (noq 1.3.0), the latest release, by user decision. go-iroh v0.2.0, which
Go dstore uses, verifies its compatibility matrix against iroh 1.0.3, so compatibility with 1.2.0 is proven
by running against Go nodes, not assumed:

- [`port-notes/impl-interop-probe.md`](port-notes/impl-interop-probe.md): the probe run before any crate
  existed. A Rust iroh 1.2.0 endpoint connected to a Go dstore v0.1.9 node and exchanged a `view` request
  and reply. That covered the handshake, raw-public-key TLS, ALPN, bidirectional streams, FIN and framing
  over direct UDP.
- [`interop/`](interop/): the live interop suite, `interop/check.sh`. It builds a 3-node Go dstore v0.1.11
  cluster on loopback into a temporary directory and deletes it on exit, then runs the Rust CLI against it.
  The checks compare Go and Rust outputs (exactly, or stdout and exit status only, or after normalisation)
  and cross-read trees, references, working copies and local stores between the two implementations
  (PORTING.md §7, port-notes/verification.md §4.5). The CI `interop` job runs it.
- `tests/iroh_loopback.rs`: the Rust endpoint against itself on 127.0.0.1 (the CI `rust` job).

The endpoint follows go-iroh's choices: the same default relays, mDNS through a port of go-iroh's
resolver (which reads go-iroh's announcements), number0 DNS discovery only when dialling an id, and no
tracing output (PORTING.md §5.12).

iroh itself carries two changes. `third_party/iroh-1.2.0` is the published 1.2.0 plus two small patches, used
through `[patch.crates-io]`. The second names a bootstrap home relay at bind, as go-iroh does, so a dial does not
wait for the first net_report. The first stops a NAT traversal round from starting while a direct path is selected. go-iroh follows
the same rule. Stock 1.2.0 starts a round on every new connection. That round triggers two go-iroh v0.2.0 node
bugs, a key-update error and a stateless reset after a retired connection ID is reused. Together they killed
about 1 Rust connection in 180 in the interop suite. [`third_party/README.md`](third_party/README.md) has the
diff and the evidence.

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

## Development

```sh
nix develop                                            # or direnv
cargo build --bin dstore
TZ=UTC cargo test --workspace --all-targets            # TZ=UTC for the CLI snapshots
cargo clippy --workspace --all-targets -- -D warnings
nix fmt                                                # cargo fmt --all
nix flake check -L                                     # fmt, clippy, the package and every socket-free test target
```

Tests:

- unit tests in every crate, and root `tests/golden.rs` over the golden vectors;
- `tests/cli_snapshots.rs`, which runs the binary over the snapshots of the Go binary;
- `tests/cli_admin.rs`, `tests/cli_client.rs` and `tests/cli_wc.rs` for the CLI actions;
- `tests/fake_cluster*.rs`, which run the client and working copies against fake nodes on an in-memory
  network;
- `tests/iroh_loopback.rs`, which opens real sockets. It is left out of the Nix check because the Darwin
  sandbox refuses UDP binds.

### Golden vectors

`tools/vectorgen` is a Go module that generates the golden vectors under `tests/golden/` from the Go
implementation, with dstore v0.1.11 and its dependencies as they are pinned. [`VECTORS.md`](VECTORS.md)
documents every family and schema:

```sh
nix develop -c go -C tools/vectorgen run . ../../tests/golden              # every family
nix develop -c go -C tools/vectorgen run . ../../tests/golden FAMILY...    # only these families
nix develop -c go -C tools/vectorgen run ./cmd/clisnap -o ../../tests/golden/cli     # CLI snapshots
nix develop -c go -C tools/vectorgen run ./cmd/gotables > crates/gocompat/src/tables.rs
nix develop -c go -C tools/vectorgen run ./cmd/goerrno > crates/gocompat/src/errno_tables.rs
```

A missing vector file fails its test. `clisnap` builds the Go `dstore` binary into a temporary directory,
runs every offline CLI case and deletes the binary.

### The interop harness

`interop/check.sh` needs Go dstore v0.1.11. It takes, in this order:
- a prebuilt binary (`DSTORE_GO_BIN`);
- a checkout whose HEAD is tag v0.1.11 (`DSTORE_GO_REPO`, default `../dstore`);
- otherwise `go install …@v0.1.11`.

The Rust CLI comes from `DSTORE_RS_BIN`, with the `holdlock` example next to it in `examples/`. Without it, the
script runs `cargo build --release --locked --bin dstore --examples` itself:

```sh
git clone --branch v0.1.11 https://github.com/amber-store/dstore ../dstore
nix develop -c cargo build --release --locked --bin dstore --examples
nix develop -c env DSTORE_GO_REPO=../dstore DSTORE_RS_BIN=target/release/dstore bash interop/check.sh
```

Every binary goes into a temporary directory. On exit the script removes that directory, the stores and every
process it started. [`interop/README.md`](interop/README.md) lists the checks and all inputs. `INTEROP_MDNS=auto|require|skip` controls the id-only-ticket checks. `INTEROP_HEAVY=1` and
`INTEROP_CHAOS=1` add the transition and node-restart groups.

### CI

`.github/workflows/ci.yml` runs four jobs:

| Job | Runs on | Steps |
|---|---|---|
| `rust` | ubuntu-latest, macos-latest | `cargo fmt --check`, clippy with `-D warnings`, `cargo test --workspace --all-targets --locked` with `TZ=UTC` |
| `vectors` | ubuntu-latest | `go vet`, `go test`; every vector family regenerated twice and diffed against `tests/golden`; `tables.rs`, `errno_tables.rs` and `cli/snapshots.json` regenerated and diffed |
| `interop` | ubuntu-latest | Go dstore v0.1.11 checked out next to this repository, then `bash interop/check.sh` |
| `nix` | ubuntu-latest, macos-latest | `nix flake check -L`, `nix build .#dstore` |

## License

LGPL-3.0-only. See [`LICENSE`](LICENSE) for the LGPL terms and [`COPYING`](COPYING) for the GPL terms the
LGPL incorporates. core-rs, which this port links and copies helpers from, has the same license.
