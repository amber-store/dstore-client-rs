# dstore-client-rs: porting contract and architecture

This is the contract every implementer of dstore-client-rs follows. It fixes what "100% compatible
with Go dstore" means, which parts are ported, the Cargo workspace and crate boundaries, the public
Rust API of every crate (precise enough to code against before it exists), the cross-cutting
decisions, the build order, and how compatibility is proven.

## 0. Normative references and precedence

| What | Pin |
|---|---|
| Go dstore (normative behaviour) | `github.com/amber-store/dstore` tag `v0.1.9`, HEAD `368f2c7`, checkout `/Users/dragan/amber-store/dstore`. Ignore the uncommitted formatting-only change to `cmd/dstore/wc.go`; read files with `git show HEAD:<path>`. |
| Go dependencies | `github.com/amber-store/core` v0.0.8; `github.com/amber-store/transport-iroh` v0.4.0 (`protocol`); `github.com/tmc/go-iroh` v0.2.0; `github.com/fxamacker/cbor/v2` v2.9.3; `github.com/aymanbagabas/go-udiff` v0.4.1; `github.com/urfave/cli/v2` v2.27.7; `charm.land/bubbletea/v2` v2.0.9, `lipgloss/v2` v2.0.6, `bubbles/v2` v2.2.1; Go toolchain go1.26.5 (stdlib behaviour: `encoding/base32`, `encoding/json` v1, `strconv`, `unicode`, `sort`, `log/slog`, `flag`, `time`). |
| core-rs | crate `amber-store-core` 0.3.0, git rev `a85ffa1eb5ed363b9072ab224de179196cd0a046` (= public tag v0.3.0) |
| Rust iroh | `iroh = "=1.2.0"` (iroh-base/iroh-relay 1.2.0, noq 1.3.0): the latest release, by user decision (2026-09-18). go-iroh v0.2.0's matrix verifies 1.0.3; compatibility with 1.2.0 is proven by the live interop suite (§7), not assumed. |
| Toolchain | nixpkgs `nixos-26.05`: rustc/cargo/clippy/rustfmt 1.95.0, go 1.26.5. Workspace `rust-version = "1.91"`, edition 2024. |

**Override (user decision, 2026-09-18): Rust iroh is the latest release, `iroh = "=1.2.0"`.** Area specs
cite iroh 1.0.3 / noq 1.1.1 sources (`iroh-1.0.3/…`) in about 57 places. Treat those as pointers only
and re-verify every API and default against `iroh-1.2.0`, `iroh-base-1.2.0`, `iroh-relay-1.2.0`,
`noq-1.3.0` and `noq-proto-1.3.0` in the offline registry. Already confirmed in 1.2.0: features `tls-ring`
and `fast-apple-datapath`, `endpoint::presets::Minimal`, `Endpoint::builder(preset)`,
`PortmapperConfig::Disabled`, `address_lookup::DnsAddressLookup::builder(origin)`, noq default
`initial_rtt` 333 ms.

Area specs (read the one for your crate completely before coding):

| Spec | Covers |
|---|---|
| `port-notes/codec-wire-ticket.md` | fxamacker CBOR as dstore configures it, `wire`, transport-iroh pack framing, tickets, go-iroh endpoint-id parsing, admin/status payload types |
| `port-notes/view-placement.md` | placement, view types and decoding, `Placement`, node ids, `TicketFromView`, status view lines, Appendix A vector generator |
| `port-notes/transport.md` | `transport` package, go-iroh semantics, Rust iroh mapping, mDNS port, in-memory transport |
| `port-notes/client-core.md` | client part A: Config, Dial, view cache, ranking, pool, call/anyNode, refs, watch, progress, slog format |
| `port-notes/client-transfer.md` | client part B: Missing, Put, Placed, Get/fetcher, VerifyRecord, Push, Pull/PullTree |
| `port-notes/worktree.md` | working copies, go-udiff port, Go stdlib behaviour the working-copy CLI observes |
| `port-notes/cli.md` | `cmd/dstore`: urfave/cli behaviour, every command, help texts, slog, TUI, signals, node-side needs |
| `port-notes/core-rs-gaps.md` | every core v0.0.8 API used client-side and its core-rs equivalent, gaps G1-G22 |
| `port-notes/verification.md` | golden-vector generator, live interop harness, fake node, Nix flake, CI |

Precedence when sources disagree:

1. The Go source at the pins above decides behaviour. Where `architecture/dstore.md` or a design note
   disagrees with the code, port the code.
2. This document decides architecture, crate boundaries, public APIs, cross-cutting decisions (§5)
   and every conflict between specs (§9).
3. The area specs decide the details; each ends with a `## Addenda (synthesis)` section added by the
   completeness check, which is part of the spec.

Line numbers: `worktree.md` and `cli.md` cite `cmd/dstore/wc.go` from the working tree, which is
HEAD + 12 lines after line 46; `verification.md` cites HEAD. `scripts/e2e-loopback.sh` stale lines
are 42 and 46 at HEAD.

---

## 1. Compatibility contract

### 1.1 Byte-identical

Locked by golden vectors from the Go libraries (§7) and by CLI snapshots from the Go binary.

- **Client-produced CBOR.** Every client-ALPN request frame (`u32be len ‖ canonical CBOR`), including
  stamps, cond flags, the ref-watch known list (`RefInfo.Version` null, `CreatedAt` 0), and
  `node.AdminRequest` params including shortest-float `Garbage`.
- **Pack framing.** TData frames cut at exact 1 MiB offsets of the pack stream, TData carrying only
  keys 0 and 8, TDataEnd `00000003a10008`, for identical record bytes.
- **Decoding decisions.** The accept/reject set, the resulting values (nil vs empty kept), first
  duplicate wins, unknown keys ignored, tags stripped, and the error texts for every path pinned by
  vectors, for `wire.Msg`, `protocol.Msg`, `view.View`, `ticket.Ticket`, `node.AdminReply`,
  `node.Status`.
- **Tickets.** `Encode()` strings, `IDs()`, the `Parse` accept set (Go base32 quirks, Unicode
  lookalikes) and its error texts.
- **Placement.** `slot`, `salt`, `fmix64`, `log2fix`, `L`, rank, owners, write set, read order.
- **Working-copy files.** `.dstore/config` and `.dstore/state` (Go `encoding/json` v1 `MarshalIndent`
  plus `\n`, written via `<path>.tmp` then rename), the empty-tree key, and the derived stored ticket.
- **Diff output.** `dstore diff` unified hunks and `--stat` (go-udiff v0.4.1 with Go pdqsort,
  including the `--`/`++` miscount).
- **CLI surface.** Command and subcommand names, aliases, flag names, types, defaults and usage
  texts, env vars, positional argument handling, help output byte for byte (templates, tabwriter
  widths, trailing spaces, truncated `help <parent>`), `Incorrect Usage` output, required-flag
  messages, exit codes (0, 1, 3, and 2 for the two Go runtime panics, DD-7), every stdout text,
  prompts, and the `dstore: <err>` line for errors produced by dstore or core code (after the errno
  rewrite of §5.2).
- **Logging and progress.** The slog `TextHandler` line format, attribute kinds and order of every
  log call, the plain progress status lines, and the TUI model's `view()` string before terminal
  rendering.
- **Text formats.** `HumanBytes`, `Rate`, `Duration.String` and `Round`, RFC3339 in the local zone,
  RFC3339Nano UTC, `%q`, `%x`, Go errno texts, `encoding/hex` errors.
- **Records.** Raw (uncompressed) records built by core-rs.

### 1.2 Interoperable

These work together with Go, but their bytes are not compared:

- A Rust client against Go dstore v0.1.9 nodes on `amber-dstore/1`: direct paths, relay, number0 DNS
  discovery, and id-only tickets over mDNS (through a port of go-iroh's mDNS resolver, §5.12).
- Packstores (`.dstore/packstore`, `<local>/packstore`) used by Go and Rust processes one after the
  other; the directory flock excludes both.
- Working copies created or updated by one implementation and used by the other.
- Records pushed by Rust accepted by Go nodes (CRC and payload hash verified), and the reverse.
- zstd records: each side decodes the other's frames; compressed bytes differ (DD-1).

### 1.3 Different by design

| # | Difference | Reason |
|---|---|---|
| DD-1 | Newly compressed records differ (libzstd vs klauspost: no content checksum, different ratio). This shifts `PushStats.bytes`, progress `TotalBytes`, the `bytes=` log attributes and put-batch boundaries (`storedSizer` = 46 + slen) for trees ingested locally by Rust. | core-rs contract; no client-side dependence on compressed bytes (core-rs-gaps G8). Golden vectors use incompressible payloads. |
| DD-2 | `store push/pull --local DIR` stores references in redb (`DIR/refs/refs.redb`), not Pebble. A Pebble directory is refused with a fixed error (§2.3). | No Rust Pebble; core-rs refstore is redb. The `packstore/` half interoperates. |
| DD-3 | Node-side commands (`serve`, `cluster init`, `node join`), the restore step of `catalog restore`, and ticket derivation from `--store` fail with a fixed error after Go's validation steps (§2.2). | They need the Pebble meta store, the paxos acceptor and the whole node. |
| DD-4 | Transport error texts and timing below the dstore wrappers. The inner QUIC text (noq vs qng) differs. A connect phase fails at `min(ctx deadline, 10 s)`, where qng uses a 5 s handshake idle / 10 s total timeout. The `dial <addr>: …` lines of one phase share one inner error and are joined in candidate order, not completion order. The wrapper texts are identical: `client: no bootstrap node answered: `, `dial %s: `, `discovery: `, `transport: …`. | Rust iroh sends handshakes on all known paths and has no separate handshake idle timeout (transport §7). |
| DD-5 | "RTT not measured" is detected when the selected path's RTT equals noq's initial RTT (333 ms), so a real 333.000 ms sample reads as unmeasured. | noq exposes no has-sample flag. |
| DD-6 | TUI terminal bytes (renderer control sequences, colour downsampling). Only `UiModel::view()` is byte-identical. | Bubble Tea has no Rust port; the renderer is a hand-rolled crossterm inline renderer. |
| DD-7 | The Go runtime panics in `dstore cat NAME /` (nil entry) and `cluster status` with a cluster id shorter than 4 bytes: Rust writes only the panic's first line to stderr and exits 2, without the goroutine dump. | Exit status parity without imitating a Go crash dump. |
| DD-8 | A non-UTF-8 command-line argument placed into a CBOR text field (`ref get/delete NAME`, `ls/cat NAME`, `watch PATTERN`, `node zone ID ZONE`) is sent lossily (U+FFFD). Go sends invalid UTF-8, and the node fails to decode the frame. Likewise, non-UTF-8 file paths inside error messages (`PathError`, `… is outside the working copy`) are rendered lossily on stderr. Stdout texts that print names and paths (`ls`, `status`, `diff`, `refs`) stay byte-exact. | `wire::Msg` text fields are `String`; error `Display` produces a `String`. Only pathological inputs are affected. |
| DD-9 | Packstore segment files under a non-022 umask get `0666 & ~umask` (Go: `0644 & ~umask`) until core-rs is patched. Rust pre-creates store directories with 0755. | core-rs `create_active` mode (core-rs-gaps G5); upstream patch proposed. |
| DD-10 | Where Go iterates a map, Rust uses a deterministic order: anyNode's bootstrap fallback, the ref-watch known list, `shortError` names, `negotiate %x`/`record %x rejected` picks, pickBatch ties, primary goroutine order, xattr set order, directory chmod restore order. | Go's order is random, so any order is compatible. |
| DD-11 | At CLI exit Rust awaits `Endpoint::close()` (bounded to 3 s) after `Cluster::close()`. | Go's `CloseWithError` blocks until CONNECTION_CLOSE is sent; noq only queues it. This restores parity for the peer. |
| DD-12 | Rare core-rs error texts are not re-rendered: `decode_payload` over-long frames, and `readdirent` vs `open` op names in walk errors. | Unobservable in practice (core-rs-gaps G17, G19, R3). Common paths are rewritten (§5.2). |
| DD-13 | A `--relay` string that Go's `url.Parse` accepts but `url::Url` rejects (e.g. `foo`) is accepted. Relays count as enabled, bind waits its 10 s for a home relay, and no relay path is dialled. | Same observable effect as Go, whose relay never connects. |
| DD-14 | Unicode tables (`IsPrint`, `IsSpace`, simple case mapping) are those of go1.26.5. | Regenerate the tables when dstore's `go.mod` `go` line changes. |
| DD-15 | `cluster status` takes the cluster id's length as its capacity. An id shorter than 4 bytes takes the DD-7 path (`panic: runtime error: slice bounds out of range [:4] with capacity N`, N = the length, exit 2). Go slices `v.ClusterID[:4]` up to the capacity: a 1..3-byte id sent as an indefinite-length CBOR byte string has an append-grown capacity of at least 8, so Go prints it zero-padded (`01020000`) and exits 0. | `View` and `Cluster` keep neither the view bytes nor Go's slice capacity (`dstore_view::cluster_id_cap` needs the bytes). Nodes encode views canonically, with definite lengths, so only a hand-crafted reply differs. Decided by the orchestrator at the L2-L4 review. |

### 1.4 Go quirks reproduced on purpose

Do not "fix" these; each is observable:

- **urfave/cli** (cli §2.2):
  - Flag parsing stops at the first positional argument, and `-name` ≡ `--name`.
  - `<leaf> help|h` shows help instead of running; `help <parent>` prints truncated help; `help help` has its own output; the shared help command keeps its HelpName.
  - `Incorrect Usage` help lists `COMMANDS: help, h`; unknown commands exit 3 with `No help topic for '…'`.
  - An empty env var counts as set (`AMBER_STORE=` satisfies `--local`).
  - Invalid bool env values are usage errors for flags that bind the env var, but are ignored by `wcConfig` for `DSTORE_NO_DISCOVERY`.
- **Terminals and signals.** `isTerminal` is a character-device check (`/dev/null` counts). While a command that installed `signalCtx` runs, later SIGINT/SIGTERM are swallowed; `status` and `diff` install nothing and die by the signal.
- **Validation order.**
  - `store push` builds the tree before failing on a missing ticket.
  - `cluster replicas` prompts before dialing.
  - Working-copy `push` checks the user after dialing.
  - `catalog restore` fetches before requiring `--store`.
  - `diff --remote --incoming` fails before opening the working copy.
- **Client v0.1.9 behaviour** (client-transfer §8.1, client-core §8.3):
  - A malformed `Keys32` list in a missing reply is ignored (every key counts as held), and unreadable records are skipped during upload.
  - `busy` sleeps without watching ctx; the stale-view retry goes to the same primary.
  - Unrequested and duplicate get records are emitted; the read order is re-ranked per retry; the view is refreshed at most once per fetcher; a Blob's length field is not checked.
  - The incomplete-retry path ignores its put result and direct-fills every key it just uploaded.
  - `Get`'s `missing()` accumulates across iterations.
  - `anyNode` keeps calling the remaining nodes after cancellation.
  - `Status` returns bytes without checking the reply type.
  - One `RefreshView` task is spawned per reply that carries a newer epoch.
  - The watch idle timer keeps running while the consumer handles an event.
  - The pool dials up to `Conns` connections, never shrinks, and its dial lock is not ctx-aware.
- **Working copies** (worktree §8):
  - Xattrs are never removed by apply, and leftover `.dstore-tmp-*` files are pushed.
  - A failed clone into a pre-existing empty directory leaves files behind; a failed apply in `pull` does not move `base`.
  - Change lists come out in walk order, not bytewise order.
  - `fetch` does not re-pull when `Remote == k` even if the local packstore lost objects.
- **CLI text details.**
  - `hexDecode` silently drops an odd trailing nibble.
  - `token create` prints a newline even for empty text.
  - `%x` of empty bytes prints nothing (`version ` keeps its trailing space).
  - `store pull` prints the full 64-hex root while the other commands print 16.

---

## 2. Scope decision table

### 2.1 Every command

"client" means implemented in Rust, with behaviour identical to Go. "node-side" means the
definition, flags, help and pre-store validation are identical, then the action fails as in §2.2.

| Command | Go | Side | Rust handling |
|---|---|---|---|
| `dstore` (root: `--log-level`, `--help/-h`, `--version/-v`) | main.go:34-70 | client | implemented (gocli) |
| `help, h` | urfave help.go | client | implemented, including the quirks |
| `cluster` (parent) | main.go:279 | client | help only |
| `cluster init` | main.go:285-318 | node-side | stub (§2.2 A) |
| `cluster status` | main.go:319-333, client.go:134-193 | client; `--store` without `--ticket` is node-side | implemented; the store path is §2.2 B |
| `cluster ticket [--ids]` | main.go:334-368, 394-405 | client; `--store` without `--ticket` is node-side | implemented; the store path is §2.2 B |
| `cluster replicas R [--yes]` | main.go:369-392 | client | implemented (prompt before dialing) |
| `serve` | main.go:409-434 | node-side | stub (§2.2 A) |
| `token` / `token create [--weight]` | main.go:436-461 | client | implemented |
| `node` (parent) | main.go:463 | client | help only |
| `node join` | main.go:476-534 | node-side | stub (§2.2 A) after `--seed` parse and token check |
| `node remove ID [--dead] [--allow-unsafe]`, `node drain ID`, `node weight ID GiB`, `node zone ID ZONE`, `node repair ID` | main.go:535-578 | client | implemented (admin frames) |
| `voter add ID`, `voter remove ID [--allow-unsafe]` | main.go:583-601 | client | implemented |
| `transition status\|abort\|refreeze\|pause\|resume` | main.go:603-618 | client | implemented |
| `gc run [--tolerate-missing] [--garbage F]`, `gc status`, `gc hold`, `gc release`, `gc why KEY` | main.go:620-642 | client | implemented |
| `catalog backup`, `catalog backups` | main.go:644-650 | client | implemented |
| `catalog restore KEY\|FILE` | main.go:651-695 | mixed | argument check, dial and fetch implemented; restore step is §2.2 C |
| `store` (parent), `store push PATH NAME`, `store pull NAME` | client.go:205-342 | client + local store | implemented (DD-2 policy §2.3) |
| `clone NAME [DIR]`, `init NAME`, `fetch`, `pull`, `push` | wc.go:115-338 (HEAD) | client + working copy | implemented |
| `status`, `diff [PATH...]` | wc.go:339-488 (HEAD) | offline working copy | implemented |
| `refs [PREFIX]`, `watch PATTERN`, `ref` (parent), `ref get NAME`, `ref delete NAME`, `ls NAME [PATH]`, `cat NAME PATH` | client.go:344-574 | client | implemented (`cat NAME /` → DD-7) |

That is 21 top-level commands, 32 subcommands and the `help` command. Library scope: everything in Go
packages `client`, `codec`, `wire`, `ticket`, `view`, `placement`, `transport`, `worktree`, the
payload types of `node/admin.go` and `node/status.go`, and transport-iroh `protocol` pack framing.
Not ported: `node`, `paxos`, `catalog`, `meta`; `refglob` only as a test helper (testkit).

### 2.2 Node-side handling (exact)

All node-side failures return a `CliError::Msg`, printed `dstore: <text>` with exit status 1. No
filesystem side effect happens before the failure: Go's `MkdirAll`, identity creation and port
writes are never performed.

**A. `serve`, `cluster init`, `node join`.** Replicate Go's steps up to `openNode`'s filesystem
work, in order:

1. Parsing, env application and required-flag checks, done by the framework.
2. `node join` only: `ticket::parse(--seed)`, whose errors are returned verbatim. Then the token must
   decode with strict `encoding/hex` to 32 bytes, else `token must be 32 bytes of hex`.
3. `--store == ""` → `no store directory: set --store or $DSTORE_STORE`.
4. `pack_size(c)` → `--pack-size: …` errors (cli §2.9).
5. Fail with
   `<cmd> is a node-side command and dstore-client-rs does not implement the dstore node; use the Go dstore binary (github.com/amber-store/dstore v0.1.9)`,
   where `<cmd>` is `serve`, `cluster init` or `node join`.

**B. `--store` ticket derivation** (`cluster status`, `cluster ticket`, and `catalog restore` without
`--ticket`). Replicate `node.OpenOffline`'s first step:

1. Read `<store>/identity` (`std::fs::read`). A failure → `node: no identity in <dir>: open <dir>/identity: <go errno text>` (verified vector).
2. Otherwise fail with
   `deriving a ticket from --store needs the node's Pebble meta store, which dstore-client-rs does not implement; pass --ticket or $DSTORE_TICKET, or use the Go dstore binary`.

**C. `catalog restore`.**
1. Read the argument as a file, else strict hex of 32 bytes, else `restore KEY|FILE`.
2. Dial (B applies when only `--store` is given).
3. Fetch with `Cluster::get`: an error is returned; with no record, `backup object not found in the cluster`.
4. `--store == ""` → `restore runs on a voter: give --store`.
5. Fail with
   `catalog restore writes through the node's paxos acceptor, which dstore-client-rs does not implement; use the Go dstore binary`.

Constants live in `dstore_cli::nodeside` (§4.12).

### 2.3 Local refs policy for `store push/pull --local DIR` (DD-2)

`open_local` mirrors Go `openLocal` (client.go:209-221): open `DIR/packstore` with sync, then open
the refs store; if the refs store fails, close the packstore and return the error. Before calling
`refstore::Store::open(DIR/refs, true)`, list `DIR/refs`. If it contains any of `CURRENT`, `LOCK`,
`MANIFEST-*`, `OPTIONS-*`, `marker.format-version.*`, `marker.manifest.*`, `*.sst`, `*.log`, fail with
`refstore: <DIR>/refs holds a Pebble database written by Go dstore; dstore-client-rs keeps local references in redb and cannot open it (use another --local directory)`.
Go dstore run on a Rust-written directory silently creates a second (Pebble) database next to
`refs.redb`; document this in the README.

---

## 3. Workspace and crate layout

### 3.1 Tree

```text
dstore-client-rs/
  Cargo.toml              [workspace] (members crates/*) + root package `dstore-client-rs`
                          ([lib] name = "dstore" facade; [[bin]] name = "dstore", doc = false)
  Cargo.lock              committed
  src/lib.rs              facade re-exports (§4.14)
  src/bin/dstore.rs       fn main() { std::process::exit(dstore_cli::main_entry()) }
  crates/gocompat/        dstore-gocompat        Go stdlib behaviour, Ctx, slog
  crates/codec/           dstore-codec           fxamacker/cbor v2.9.3-compatible CBOR
  crates/wire/            dstore-wire            wire.Msg, frames, remote errors, pack framing, node admin/status payloads
  crates/ticket/          dstore-ticket          dstore1 tickets, go-iroh endpoint-id parsing
  crates/view/            dstore-view            NodeId, placement, view, Placement, TicketFromView
  crates/transport/       dstore-transport       Endpoint/Conn/Stream traits, Pool, Go address strings, in-memory network
  crates/transport-iroh/  dstore-transport-iroh  Rust iroh 1.2.0 endpoint, dial phases, discovery, go-iroh mDNS port
  crates/client/          dstore-client          client package (parts A and B)
  crates/udiff/           dstore-udiff           go-udiff v0.4.1 Unified path + Go pdqsort
  crates/worktree/        dstore-worktree        worktree package
  crates/gocli/           dstore-gocli           urfave/cli v2.27.7-compatible framework (Go flag parser, help templates, tabwriter)
  crates/cli/             dstore-cli             cmd/dstore: command table, actions, progress/TUI
  crates/testkit/         dstore-testkit         publish = false: splitmix, golden loaders, refglob, FakeNode
  tests/golden.rs         one integration binary, a `mod` per vector family
  tests/golden/           committed vectors (§7)
  tests/cli_snapshots.rs  spawns env!("CARGO_BIN_EXE_dstore") over tests/golden/cli/snapshots.json
  tests/fake_cluster.rs   client + worktree scenarios over transport::mem + FakeNode
  tests/iroh_loopback.rs  real Rust iroh on 127.0.0.1 (never inside the Nix sandbox)
  examples/holdlock.rs    lock-interop helper for interop D12
  tools/vectorgen/        Go module: vector generator, gotables, clisnap, mktree, treekey, storecmp, holdlock
  interop/check.sh, interop/lib.sh
  flake.nix, flake.lock, .envrc, .gitignore, .github/workflows/ci.yml
  README.md, VECTORS.md, PORTING.md, port-notes/
```

### 3.2 Crate dependencies

| Crate | Depends on (workspace) | External (versions in §5.10) |
|---|---|---|
| `dstore-gocompat` | — | tokio, tokio-util, libc, thiserror |
| `dstore-codec` | gocompat | amber-store-core (`cbor::append_head`), thiserror |
| `dstore-wire` | gocompat, codec | amber-store-core (`amberpack`), tokio (io-util), futures-core, thiserror |
| `dstore-ticket` | gocompat, codec | iroh-base (curve check), data-encoding, thiserror |
| `dstore-view` | gocompat, codec, ticket | blake3, thiserror |
| `dstore-transport` | gocompat, wire, view | tokio, tokio-util, async-trait, futures, thiserror |
| `dstore-transport-iroh` | gocompat, wire, view, transport | iroh, iroh-base, tokio, tokio-util, futures, async-trait, socket2, nix, data-encoding, url, rand, thiserror |
| `dstore-client` | gocompat, codec, wire, ticket, view, transport | amber-store-core, tokio, tokio-util, futures, async-stream, async-channel, pin-project-lite, rand, thiserror |
| `dstore-udiff` | — | — |
| `dstore-worktree` | gocompat, ticket, view, client, udiff | amber-store-core, xattr, libc, tokio, thiserror |
| `dstore-gocli` | gocompat | tokio (rt), thiserror |
| `dstore-cli` | all of the above except testkit | amber-store-core, iroh, crossterm, tokio, tokio-util, futures, libc, thiserror |
| `dstore-testkit` | gocompat, codec, wire, view, transport | amber-store-core, tokio, serde, serde_json, hex |
| root `dstore-client-rs` | every crate (testkit as dev-dependency) | dev: serde, serde_json, hex, tempfile, walkdir, xattr, rustix, similar, tokio (test-util) |

Only `dstore-cli` and the root package depend on `dstore-transport-iroh`, so `client`, `worktree`,
`testkit` and their tests build without compiling iroh.

### 3.3 Parallel-development contract

1. **Scaffold first (layer L0).** One agent creates the whole workspace. Every crate and module of §4
   exists with every public item: types with all fields, enums with all variants, constants with
   their real values, and functions with their final signatures and `todo!()` bodies.
   `cargo check --workspace --all-targets` passes at the end of L0 and after every merge.
2. **One owner per module** (§6). An agent edits only its modules and their tests. Changing a public
   signature listed in §4 also means updating PORTING.md in the same change; add rather than change.
3. **Crate-internal seams** used by sibling modules (for example client part A's `call`,
   `handle_err`, `stamp` for part B) are listed in §4 as `pub(crate)` and stubbed in L0.
4. **Tests against unfinished siblings** are marked `#[ignore = "needs <crate>::<module>"]` and
   un-ignored by whoever lands the sibling. `main` stays green.
5. **Vectors before tests.** The Go generator (tools/vectorgen) is owned separately. A golden test
   whose vector file is missing fails; it does not skip.
6. **No sockets in `--lib` tests** of any crate. Real UDP/mDNS tests live only in root
   `tests/iroh_loopback.rs`.
7. **Blocking work never runs on async worker threads** (§5.1). Every `std::sync` lock is released
   before an `.await`.

### 3.4 Conventions for every crate

- **Go numbers.** Go `int`/`int64` → `i64`; `uint*` → the same width; use `usize` only for lengths
  and indexes. Go `time.Duration` → `std::time::Duration` for non-negative values, `i64` nanoseconds
  where a negative value is observable (slog Duration attributes, `duration_string`).
- **Keys and ids.** Content keys → `[u8; 32]` (or `amber_store_core::key::Key` where core-rs APIs are
  called); node ids → `dstore_view::NodeId`.
- **Go bytes and strings.**
  - A non-omitempty CBOR `[]byte` → `Option<Vec<u8>>` (`None` = nil = `f6`).
  - An omitempty `[]byte` → `Vec<u8>`; a CBOR text field → `String`.
  - Go strings from argv, the filesystem or JSON → bytes (`Vec<u8>`/`&[u8]`/`OsString`).
  - Functions that echo their input with `%q` take `&[u8]` (`ticket::parse`, `view::parse_node_id`).
- **Errors** follow §5.2. `Display` is exactly Go's `Error()` text; a wrapped error is also returned
  by `source()`; nothing ever prints the source chain.
- **I/O.** Async operations take `&Ctx` as their first argument. Sync core-rs calls follow §5.1.
- **Clippy.** No `unsafe` outside `gocompat::os`, `gocompat::time` (libc), `worktree::sys` and
  `transport-iroh::ifaces`/`mdns` (socket options). `#![deny(unsafe_op_in_unsafe_fn)]`.

---

## 4. Crates

### 4.1 `dstore-gocompat`

**Ports** go1.26.5 stdlib behaviour:
- `strconv` (`quote.go`, `isprint.go`, `ParseBool/Int/Uint/Float`, `FormatFloat('g', -1, 64)`) and `unicode` (`IsPrint`, `IsSpace`, simple case mapping);
- `strings.ToLower/ToUpper/TrimSpace/FieldsFunc`;
- `time` (`Duration.String/Round`, `ParseDuration`, `Format` for RFC3339, RFC3339Nano, the slog layout, `15:04:05` and the log package layout, `Parse(RFC3339Nano)`, the local zone);
- `encoding/json` v1 (structs of string and bool fields only), `encoding/hex`, the `encoding/base32` decode loop;
- `os.Getwd/MkdirAll/Remove/RemoveAll/CreateTemp/WriteFile/ReadFile`, `path.Base`, `filepath.Abs/Clean/Rel/Join/Dir`, cgo-less `os/user.Current`;
- `syscall` errno strings (darwin, linux), `errors.Join` layout;
- `context` as `Ctx`, and `log/slog` `TextHandler`.

It also copies the duration and RFC3339 helpers from core-rs `examples/amber-store.rs` (keep the
LGPL-3.0-only notice).

**Specs:**
- codec-wire-ticket §2.4.7-2.4.8, §4.4 (`gostr`);
- view-placement §3.5;
- client-core §3.5-3.6, §4.3 (`ctx`, `slog`), §7.3;
- cli §2.3, §4.3, §4.6;
- worktree §2.12;
- core-rs-gaps §4.2 (`goerr`, `gohex`), §4.3.

**Generated tables.** `src/tables.rs` is written by `tools/vectorgen/cmd/gotables` (a Go program
printing Rust source): the `strconv` isPrint/isNotPrint/isGraphic tables, `unicode.White_Space`, and
every rune whose `unicode.ToLower`/`ToUpper` differs from itself. The file is committed; CI
regenerates it and diffs.

```rust
// quote.rs
pub fn quote(s: &[u8]) -> String;                  // strconv.Quote over Go string bytes (\xNN for invalid UTF-8)
pub fn is_print(r: char) -> bool;                  // strconv.IsPrint
pub fn is_space(r: char) -> bool;                  // unicode.IsSpace

// strings.rs
pub fn to_lower(s: &[u8]) -> Vec<u8>;              // strings.ToLower: ASCII fast path, else strings.Map(unicode.ToLower):
pub fn to_upper(s: &[u8]) -> Vec<u8>;              //   each invalid UTF-8 byte becomes U+FFFD; simple (not full) mapping
pub fn trim_space(s: &[u8]) -> &[u8];              // strings.TrimSpace (Unicode White_Space)
pub fn fields_func<'a>(s: &'a [u8], is_sep: impl Fn(char) -> bool) -> Vec<&'a [u8]>;

// strconv.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumErrorKind { Syntax, Range }            // "invalid syntax" / "value out of range"
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("strconv.{func}: parsing {}: {}", crate::quote::quote(.num.as_bytes()), .kind.text())]
pub struct NumError { pub func: &'static str, pub num: String, pub kind: NumErrorKind }
impl NumErrorKind { pub fn text(&self) -> &'static str; }
pub fn parse_bool(s: &str) -> Result<bool, NumError>;
pub fn parse_int(s: &str, base: u32, bit_size: u32) -> Result<i64, NumError>;   // base 0: 0x/0o/0b/0 prefixes and '_' rules
pub fn parse_uint(s: &str, base: u32, bit_size: u32) -> Result<u64, NumError>;
pub fn parse_float(s: &str) -> Result<f64, NumError>;                           // bitSize 64, hex floats, inf/nan, '_' rules
pub fn format_float_g(f: f64) -> String;                                         // FormatFloat(f, 'g', -1, 64)

// time.rs
pub const MILLISECOND: i64 = 1_000_000;
pub const SECOND: i64 = 1_000_000_000;
pub fn duration_string(ns: i64) -> String;                     // time.Duration.String ("0s", "1.5µs", "2m0s", "-1s")
pub fn duration_round(ns: i64, m: i64) -> i64;                 // Duration.Round: half away from zero, saturating
pub fn parse_duration(s: &str) -> Result<i64, String>;         // time.ParseDuration with Go error texts
pub fn duration_to_ns(d: std::time::Duration) -> i64;          // saturating
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct GoTime { pub unix_secs: i64, pub nanos: u32 }       // instant, UTC based
impl GoTime {
    pub fn now() -> GoTime;
    pub fn from_unix_nano(ns: i64) -> GoTime;                  // floor division for negative ns
    pub fn unix_nano(&self) -> i64;                            // wrapping, as Go
    pub fn add_ns(&self, ns: i64) -> GoTime;
}
pub trait Zone: Send + Sync { fn offset_at(&self, unix_secs: i64) -> i32; }
pub struct SystemZone;                                         // libc localtime_r + tm_gmtoff (TZ honoured as Go initLocal does where tzdata exists)
pub struct FixedZone(pub i32);
impl Zone for SystemZone { .. }
impl Zone for FixedZone { .. }
pub fn format_rfc3339(t: GoTime, zone: &dyn Zone) -> String;          // time.RFC3339, "Z" for offset 0
pub fn format_rfc3339nano_utc(t: GoTime) -> String;                   // trailing zeros trimmed, "Z"
pub fn format_slog_time(t: GoTime, zone: &dyn Zone) -> String;        // "2006-01-02T15:04:05.000Z07:00", truncated to ms
pub fn format_clock(t: GoTime, zone: &dyn Zone) -> String;            // "15:04:05"
pub fn format_log_std(t: GoTime, zone: &dyn Zone) -> String;          // "2006/01/02 15:04:05" (log package, slog.Default)
pub fn parse_rfc3339nano(s: &str) -> Result<GoTime, String>;          // accepts ±hh:mm; Go texts e.g.
                                                                      // `parsing time "" as "2006-01-02T15:04:05.999999999Z07:00": cannot parse "" as "2006"`

// json.rs (encoding/json v1; escapeHTML = true)
pub enum JsonField<'a> { Str(&'a [u8]), Bool(bool) }
pub fn marshal_indent_object(fields: &[(&str, JsonField<'_>)]) -> Vec<u8>;    // MarshalIndent(v, "", "  "), no trailing "\n"; caller drops omitempty fields
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum JsonKind { String, Bool }
pub struct JsonFieldSpec { pub name: &'static str, pub kind: JsonKind }
#[derive(Clone, Debug, PartialEq, Eq)] pub enum JsonValue { String(Vec<u8>), Bool(bool) }
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JsonError {
    #[error("unexpected end of JSON input")] UnexpectedEnd,
    #[error("{0}")] Syntax(String),        // `invalid character 'x' looking for beginning of value`, … (Go scanner texts)
    #[error("json: cannot unmarshal {value} into Go struct field {go_struct}.{field} of type {go_type}")]
    Type { value: &'static str, go_struct: &'static str, field: String, go_type: &'static str },
}
/// Go json.Unmarshal into a struct: exact key match, then case-folding match (incl. K/U+212A, S/U+017F);
/// the last duplicate wins; null leaves a field unset; unknown keys are ignored; the first type error is
/// recorded and decoding continues; invalid UTF-8 inside strings → U+FFFD per byte.
/// Returns one slot per spec (None = not set) and the first error.
pub fn unmarshal_object(data: &[u8], go_struct: &'static str, fields: &[JsonFieldSpec]) -> (Vec<Option<JsonValue>>, Result<(), JsonError>);

// hex.rs (encoding/hex)
pub fn encode(b: &[u8]) -> String;
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HexError {
    #[error("encoding/hex: odd length hex string")] OddLength,
    #[error("encoding/hex: invalid byte: {}", fmt_u(*.0))] InvalidByte(u8),   // %#U of the byte as a rune: U+007A 'z'
}
pub fn fmt_u(b: u8) -> String;
pub fn decode_string(s: &[u8]) -> Result<Vec<u8>, HexError>;                  // invalid byte reported before odd length

// base32.rs (encoding/base32, NoPadding)
pub const STD_ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
pub const ZBASE32_ALPHABET: &[u8; 32] = b"ybndrfg8ejkmcpqxot1uwisza345h769";
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("illegal base32 data at input byte {0}")]
pub struct CorruptInputError(pub usize);
pub fn decode_nopad(alphabet: &[u8; 32], s: &[u8]) -> Result<Vec<u8>, CorruptInputError>;  // strips \r\n; drops 1/3/6-symbol tails; no trailing-bit check
pub fn encode_nopad(alphabet: &[u8; 32], b: &[u8]) -> String;

// errno.rs
pub fn errno_text(code: i32) -> Option<&'static str>;          // Go syscall table for cfg(target_os)
pub fn io_error_text(e: &std::io::Error) -> String;            // raw OS error → Go text; UnexpectedEof → "unexpected EOF"; else e.to_string()
pub fn rewrite_os_errors(msg: &str) -> String;                 // "<Rust strerror> (os error N)" → Go text (§5.2)
#[derive(Debug, thiserror::Error)]
#[error("{op} {}: {}", String::from_utf8_lossy(.path), io_error_text(.err))]
pub struct PathError { pub op: &'static str, pub path: Vec<u8>, #[source] pub err: std::io::Error }

// path.rs (Unix)
pub fn clean(p: &[u8]) -> Vec<u8>;
pub fn join(elems: &[&[u8]]) -> Vec<u8>;
pub fn dir(p: &[u8]) -> Vec<u8>;
pub fn base(p: &[u8]) -> Vec<u8>;                               // path.Base: "" → ".", all slashes → "/"
pub fn abs(p: &[u8]) -> std::io::Result<Vec<u8>>;               // Clean(Join(getwd(), p)) when relative
pub fn rel(base: &[u8], target: &[u8]) -> Option<Vec<u8>>;      // filepath.Rel, lexical; None = Go error
pub fn to_path(b: &[u8]) -> std::path::PathBuf;
pub fn from_path(p: &std::path::Path) -> Vec<u8>;

// os.rs
pub fn getwd() -> std::io::Result<Vec<u8>>;                     // $PWD if absolute and same (dev, ino) as "."
pub fn mkdir_all(path: &[u8], mode: u32) -> Result<(), PathError>;
pub fn remove(path: &[u8]) -> Result<(), PathError>;            // unlink, then rmdir; error selection as os.Remove
pub fn remove_all(path: &[u8]) -> Result<(), PathError>;
pub fn create_temp(dir: &[u8], pattern: &str) -> Result<(std::fs::File, Vec<u8>), PathError>;  // ".dstore-tmp-*": decimal u32 names, O_EXCL 0600, 10000 tries
pub fn write_file(path: &[u8], data: &[u8], mode: u32) -> Result<(), PathError>;               // O_WRONLY|O_CREATE|O_TRUNC
pub fn read_file(path: &[u8]) -> Result<Vec<u8>, PathError>;
pub fn current_username() -> Result<String, String>;           // getpwuid_r(getuid()), else $USER when $USER and $HOME set
pub fn is_char_device(fd: std::os::fd::RawFd) -> bool;         // cmd/dstore isTerminal
pub fn geteuid() -> u32;

// fmt.rs
pub fn hex_lower(b: &[u8]) -> String;                           // %x
pub fn v_strings(items: &[String]) -> String;                   // %v of []string: "[a b]", "[]"
pub fn errors_join(msgs: &[String]) -> String;                  // errors.Join layout: "\n" between non-empty messages

// ctx.rs (context.Context)
#[derive(Clone)]
pub struct Ctx { /* token: tokio_util::sync::CancellationToken, deadline: Option<tokio::time::Instant>, cause: Arc<OnceLock<CtxError>>, parent: Option<Box<Ctx>> */ }
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CtxError { #[error("context canceled")] Canceled, #[error("context deadline exceeded")] DeadlineExceeded }
impl Ctx {
    pub fn background() -> Ctx;
    pub fn with_cancel(&self) -> Ctx;                          // child; parent cancellation propagates
    pub fn with_timeout(&self, d: std::time::Duration) -> Ctx; // child; deadline = min(parent deadline, now + d)
    pub fn cancel(&self);                                      // this ctx and its descendants only
    pub fn deadline(&self) -> Option<tokio::time::Instant>;
    pub fn err(&self) -> Option<CtxError>;                     // cause of the first event: own cancel, own deadline, or the parent's err
    pub async fn done(&self);
    pub async fn run<F: std::future::Future>(&self, f: F) -> Result<F::Output, CtxError>;   // select!{done, f}
    pub async fn sleep(&self, d: std::time::Duration) -> Result<(), CtxError>;
}

// slog.rs (log/slog, dstore's use of it)
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Level(pub i32);
impl Level { pub const DEBUG: Level = Level(-4); pub const INFO: Level = Level(0); pub const WARN: Level = Level(4); pub const ERROR: Level = Level(8); }
impl std::fmt::Display for Level { .. }                        // "INFO", "WARN+2", "DEBUG-1"
pub fn log_level(s: &str) -> Level;                            // cmd/dstore logLevel: ToLower; debug|warn|error, else INFO
#[derive(Clone, Debug, PartialEq)]
pub enum Value { String(String), Int64(i64), Uint64(u64), Float64(f64), Bool(bool), Duration(i64), Time(GoTime), Bytes(Vec<u8>), Any(String) }
#[derive(Clone, Debug, PartialEq)]
pub struct Attr { pub key: String, pub value: Value }
impl Attr {
    pub fn string(k: &str, v: impl Into<String>) -> Attr;
    pub fn int64(k: &str, v: i64) -> Attr;
    pub fn uint64(k: &str, v: u64) -> Attr;
    pub fn bool(k: &str, v: bool) -> Attr;
    pub fn duration(k: &str, ns: i64) -> Attr;
    pub fn any(k: &str, v: impl std::fmt::Display) -> Attr;     // errors and %+v values
}
pub struct Record { pub time: Option<GoTime>, pub level: Level, pub message: String, pub attrs: Vec<Attr> }
pub trait Handler: Send + Sync + 'static {
    fn enabled(&self, level: Level) -> bool;
    fn handle(&self, handler_attrs: &[Attr], r: &Record);      // handler_attrs = Logger::with attributes, first
}
#[derive(Clone)]
pub struct Logger { /* handler: Arc<dyn Handler>, attrs: Arc<Vec<Attr>> */ }
impl Logger {
    pub fn new(h: std::sync::Arc<dyn Handler>) -> Logger;
    pub fn with(&self, attrs: Vec<Attr>) -> Logger;
    pub fn enabled(&self, level: Level) -> bool;
    pub fn log(&self, level: Level, msg: &str, attrs: Vec<Attr>);  // checks enabled first; time = GoTime::now()
    pub fn debug(&self, msg: &str, attrs: Vec<Attr>);
    pub fn info(&self, msg: &str, attrs: Vec<Attr>);
    pub fn warn(&self, msg: &str, attrs: Vec<Attr>);
    pub fn error(&self, msg: &str, attrs: Vec<Attr>);
    pub fn default_logger() -> Logger;                         // slog.Default(): log-package format on stderr, level INFO
}
pub struct TextHandler { /* level, zone, out: Mutex<Box<dyn Write + Send>> */ }
impl TextHandler {
    pub fn new(out: Box<dyn std::io::Write + Send>, level: Level, zone: std::sync::Arc<dyn Zone>) -> TextHandler;
}
impl Handler for TextHandler { .. }                            // one write_all per record
pub fn needs_quoting(s: &str) -> bool;                         // slog text_handler needsQuoting
pub fn format_text_record(handler_attrs: &[Attr], r: &Record, zone: &dyn Zone) -> Vec<u8>;  // the exact line incl. "\n"
```

### 4.2 `dstore-codec`

**Ports** `dstore/codec/codec.go`, plus the fxamacker/cbor v2.9.3 behaviour behind
`CanonicalEncOptions().EncMode()` and `DecOptions{}.DecMode()`. **Spec:** codec-wire-ticket §2.1,
§4.2-§4.4, §7 K1-K2, §8 R1-R2, R6, R9, R11-R12. Model the decoder on core-rs
`src/reference.rs:440-890`, generalised to indefinite lengths and the 131072 caps.

Decisions:
- Hand-rolled. Pass 1 is a well-formedness check with fxamacker's limits and texts; pass 2 is
  per-field decoding.
- Every error text is verbatim fxamacker, including the rewrite to the outermost struct field.
- Lax decoding is replicated: tags stripped, bignums into integers, simple values into integers,
  arrays into bytes, first duplicate wins, null into a scalar is a no-op.

```rust
pub struct Enc { buf: Vec<u8> }
impl Enc {
    pub fn new() -> Enc;
    pub fn into_bytes(self) -> Vec<u8>;
    pub fn head(&mut self, major: u8, n: u64);        // shortest head = amber_store_core::cbor::append_head
    pub fn uint(&mut self, v: u64);
    pub fn int(&mut self, v: i64);                    // major 1 with -1-v when v < 0
    pub fn bool(&mut self, v: bool);
    pub fn null(&mut self);                           // 0xf6
    pub fn bytes(&mut self, v: &[u8]);
    pub fn text(&mut self, v: &str);
    pub fn f64_canonical(&mut self, v: f64);          // E11: NaN f97e00, ±Inf f97c00/f9fc00, then float16/32/64 shortest exact
}
pub trait Encode { fn encode(&self, e: &mut Enc); }
pub fn marshal<T: Encode + ?Sized>(v: &T) -> Vec<u8>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CborType { PositiveInteger, NegativeInteger, ByteString, TextString, Array, Map, Tag, Primitives }
impl std::fmt::Display for CborType { .. }            // "positive integer", …, "UTF-8 text string", "primitives"

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnmarshalTypeError {
    pub cbor_type: CborType,
    pub go_type: String,                 // innermost failing Go type, e.g. "uint32", "[]uint8", "view.Pending"
    pub struct_field: Option<String>,    // "wire.Msg.21": outermost struct field, rewritten on the way out
    pub detail: Option<String>,          // "4294967296 overflows uint32", "cannot decode CBOR array to struct without toarray option"
}
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    #[error("EOF")] Eof,
    #[error("unexpected EOF")] UnexpectedEof,
    #[error("cbor: invalid additional information {ai} for type {ty}")] InvalidAi { ai: u8, ty: CborType },
    #[error("cbor: unexpected \"break\" code")] UnexpectedBreak,
    #[error("cbor: invalid simple value {0} for type primitives")] InvalidSimple(u8),
    #[error("cbor: {ty} length {n} is too large, causing integer overflow")] StrLenOverflow { ty: CborType, n: u64 },
    #[error("cbor: {ty} length {n} is too large, it would cause integer overflow")] LenOverflow { ty: CborType, n: u64 },
    #[error("cbor: exceeded max nested level 32")] MaxNested,
    #[error("cbor: exceeded max number of elements 131072 for CBOR array")] MaxArray,
    #[error("cbor: exceeded max number of key-value pairs 131072 for CBOR map")] MaxMap,
    #[error("cbor: wrong element type {chunk} for indefinite-length {ty}")] ChunkType { chunk: CborType, ty: CborType },
    #[error("cbor: indefinite-length {0} chunk is not definite-length")] ChunkIndef(CborType),
    #[error("cbor: {n} bytes of extraneous data starting at index {index}")] Extraneous { n: usize, index: usize },
    #[error("cbor: invalid UTF-8 string")] InvalidUtf8,
    #[error("{0}")] BadTag(String),                   // the three D2 messages
    #[error("{}", .0.message())] Type(UnmarshalTypeError),
    #[error("cbor: cannot unmarshal {key_type} into Go value of type string (map key is of type {key_type} and cannot be used to match struct field name)")]
    MapKey { key_type: CborType },
    #[error("cbor: cannot unmarshal {cbor_type} into Go value of type int64 ({detail} overflows Go's int64)")]
    MapKeyOverflow { cbor_type: CborType, detail: String },
}
impl UnmarshalTypeError { pub fn message(&self) -> String; }   // D11 format

/// Pass-2 cursor over input that pass 1 already validated.
pub struct Dec<'a> { /* data: &'a [u8], off: usize */ }
impl<'a> Dec<'a> {
    pub fn peek_major(&self) -> u8;
    pub fn take_null(&mut self) -> bool;                                    // f6 / f7 consumed → true
    pub fn tag_preamble(&mut self) -> Result<(), DecodeError>;              // D2
    pub fn read_uint(&mut self, go_type: &'static str, max: u64) -> Result<u64, DecodeError>;          // D3/D4/D8
    pub fn read_int(&mut self, go_type: &'static str, min: i64, max: i64) -> Result<i64, DecodeError>; // D3/D4/D5/D8
    pub fn read_bool(&mut self, go_type: &'static str) -> Result<bool, DecodeError>;
    pub fn read_f64(&mut self, go_type: &'static str) -> Result<f64, DecodeError>;
    pub fn read_text(&mut self, go_type: &'static str) -> Result<String, DecodeError>;                // D7, chunks, UTF-8
    pub fn read_bytes(&mut self, go_type: &'static str) -> Result<Vec<u8>, DecodeError>;              // D6, D9 (array of uint8), D3 bignum
    pub fn read_array<T>(&mut self, go_type: &'static str, elem: impl FnMut(&mut Dec<'a>) -> Result<T, DecodeError>) -> Result<Vec<T>, DecodeError>;
    pub fn read_map_struct(&mut self, go_type: &'static str, field: impl FnMut(&mut Dec<'a>, u64) -> Result<bool, DecodeError>) -> Result<(), DecodeError>;
                                                                            // D10: key dispatch; field returns false for unknown keys (value skipped);
                                                                            // duplicates of a matched key are skipped unexamined
    pub fn skip(&mut self);
}

/// Implemented for the field types dstore uses: bool, u8, u16, u32, u64, i64, f64, String, Vec<u8>,
/// Option<Vec<u8>>, Vec<Vec<u8>>, Vec<Option<Vec<u8>>>, Vec<String>, Vec<u16>, Vec<T: Struct>,
/// Option<Vec<T: Struct>>, Option<Box<T: Struct>>. Vec<Vec<u8>> decodes a null element as an empty vector
/// (codec-wire-ticket R6); Vec<Option<Vec<u8>>> keeps it as None (views, which golden tests re-encode).
pub trait Field: Sized + Default {
    fn encode_field(&self, e: &mut Enc);
    fn is_empty_field(&self) -> bool;                                           // fxamacker omitempty emptiness
    fn decode_field(d: &mut Dec<'_>, go_type: &'static str) -> Result<Self, DecodeError>;   // element Go type = go_type without a leading "[]"
}
/// A keyasint CBOR struct (generated by cbor_struct!).
pub trait Struct: Encode + Sized + Default {
    const GO_NAME: &'static str;                                                // "wire.Msg"
    fn decode_struct(d: &mut Dec<'_>) -> Result<Self, DecodeError>;
}
pub fn unmarshal<T: Struct>(b: &[u8]) -> Result<T, DecodeError>;               // pass 1, then pass 2; top-level null → T::default()
pub fn well_formed(b: &[u8]) -> Result<(), DecodeError>;

/// Syntax used by every struct in wire, ticket, view and admin (fields in ascending key order):
/// cbor_struct! {
///     #[derive(Clone, Debug, Default, PartialEq)]
///     pub struct RefInfo = "wire.RefInfo" {
///         0 => name: String = "string",
///         1 => key: Option<Vec<u8>> = "[]uint8",
///         3 => created_at: i64 = "int64",
///         4 => user: String = "string" [omitempty],
///     }
/// }
/// Generates the struct (all fields `pub`), `Encode` (count present fields, shortest map head, ascending keys),
/// and `Struct::decode_struct` (definite or indefinite map, numeric key dispatch, first duplicate wins,
/// errors rewritten to "<GO_NAME>.<key>").
#[macro_export]
macro_rules! cbor_struct { .. }
```

### 4.3 `dstore-wire`

**Ports:**
- dstore `wire/wire.go` (395);
- transport-iroh v0.4.0 `protocol/protocol.go` (180) and `protocol/pack.go` (141);
- the payload types of `node/admin.go` (`AdminRequest`, `AdminReply`) and `node/status.go` (`Status`, `VoterStat`, `DecodeStatus`).

**Specs:**
- codec-wire-ticket §2.2, §2.3, §2.5, §2.6, §3, §4.4, §7 K3-K4, K9-K10, §8 R3, R5, R7-R9, R14;
- core-rs-gaps §2.2, §2.10, §4.3;
- client-transfer §3.1-§3.3, §4.4;
- cli §2.7, §3.8.

Go `wire.CloseStream` is `dstore_transport::Stream::close_stream` (§4.6).

Names: Go `TFooBar` → `T_FOO_BAR` (`TOK` → `T_OK`, `TCASMismatch` → `T_CAS_MISMATCH`,
`TGCStatusRep` → `T_GC_STATUS_REP`); Go `CodeFooBar` → `CODE_FOO_BAR`.

```rust
// consts.rs
pub const ALPN_CLIENT: &str = "amber-dstore/1";
pub const ALPN_CLUSTER: &str = "amber-dstore-cluster/1";
pub const ALPN_GATEWAY: &str = "amber-store-iroh/1";
pub const MAX_FRAME: usize = 16 << 20;
pub const MAX_KEYS: usize = 8192;
pub const MAX_PUT_BATCH: usize = 64 << 20;
pub const MAX_PAGE_BYTES: usize = 4 << 20;
pub const CHUNK_SIZE: usize = 1 << 20;
pub const T_DATA: i64 = 7;          pub const T_DATA_END: i64 = 8;       pub const T_ERR: i64 = 10;
pub const T_VIEW: i64 = 32;         pub const T_MISSING: i64 = 33;       pub const T_GET: i64 = 34;
pub const T_PUT: i64 = 35;          pub const T_REF_GET: i64 = 36;       pub const T_REF_PUT: i64 = 37;
pub const T_REF_DELETE: i64 = 38;   pub const T_REF_LIST: i64 = 39;      pub const T_STATUS: i64 = 40;
pub const T_ADMIN: i64 = 41;        pub const T_REF_WATCH: i64 = 42;     pub const T_VIEW_REPLY: i64 = 48;
pub const T_MISSING_REPLY: i64 = 49; pub const T_ABSENT: i64 = 50;       pub const T_PUT_RESULT: i64 = 51;
pub const T_REF: i64 = 52;          pub const T_OK: i64 = 53;            pub const T_CAS_MISMATCH: i64 = 54;
pub const T_INCOMPLETE: i64 = 55;   pub const T_REFS: i64 = 56;          pub const T_STATUS_REPLY: i64 = 57;
pub const T_ADMIN_REPLY: i64 = 58;  pub const T_REF_CHANGES: i64 = 59;   pub const T_REF_SYNCED: i64 = 60;
pub const T_PREPARE: i64 = 64;      pub const T_ACCEPT: i64 = 65;        pub const T_READ: i64 = 66;
pub const T_SCAN: i64 = 67;         pub const T_INSTALL: i64 = 68;       pub const T_PURGE: i64 = 69;
pub const T_MARKER: i64 = 70;       pub const T_SEED: i64 = 71;          pub const T_PROMISE: i64 = 80;
pub const T_CONFLICT: i64 = 81;     pub const T_ACCEPTED: i64 = 82;      pub const T_READ_REPLY: i64 = 83;
pub const T_SCAN_REPLY: i64 = 84;   pub const T_INSTALLED: i64 = 85;     pub const T_JOIN: i64 = 96;
pub const T_GC_BARRIER: i64 = 97;   pub const T_GC_MARK: i64 = 98;       pub const T_GC_KEYS: i64 = 99;
pub const T_GC_STATUS: i64 = 100;   pub const T_ACK: i64 = 101;          pub const T_VIEW_CHANGED: i64 = 102;
pub const T_GC_STATUS_REP: i64 = 103; pub const T_GC_ABORT: i64 = 104;   pub const T_PING: i64 = 105;
pub const T_PONG: i64 = 106;        pub const T_BACKUP_NOTE: i64 = 107;  pub const T_REF_CHANGED: i64 = 108;
pub const CODE_STALE_VIEW: &str = "stale-view";   pub const CODE_NOT_OWNER: &str = "not-owner";
pub const CODE_NO_SPACE: &str = "no-space";       pub const CODE_BUSY: &str = "busy";
pub const CODE_BAD_REQUEST: &str = "bad-request"; pub const CODE_UNAUTHORIZED: &str = "unauthorized";
pub const CODE_UNKNOWN_REF: &str = "unknown-ref"; pub const CODE_CAS_MISMATCH: &str = "cas-mismatch";
pub const CODE_INCOMPLETE: &str = "incomplete";   pub const CODE_UNAVAILABLE: &str = "unavailable";
pub const CODE_TIMEOUT: &str = "timeout";         pub const CODE_INTERNAL: &str = "internal";
pub const CODE_NOT_MEMBER: &str = "not-member";   pub const CODE_NEED_VIEW: &str = "need-view";
pub const CODE_EXPIRED: &str = "expired";         pub const CODE_AMNESIAC: &str = "amnesiac";
pub const CODE_MARK_FROZEN: &str = "mark-frozen"; pub const CODE_RETIRED: &str = "retired";
pub const CODE_TOO_SOON: &str = "too-soon";       pub const CODE_CONFLICT: &str = "conflict";
pub const CODE_NO_MARK: &str = "no-mark";

// msg.rs: every Msg field is omitempty except key 0 (codec-wire-ticket §2.2.3)
cbor_struct! {
    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct Msg = "wire.Msg" {
        0 => typ: i64 = "int",
        1 => cluster_id: Vec<u8> = "[]uint8" [omitempty],
        2 => incarnation: u64 = "uint64" [omitempty],
        3 => epoch: u64 = "uint64" [omitempty],
        4 => keys: Vec<Vec<u8>> = "[][]uint8" [omitempty],
        5 => pin: bool = "bool" [omitempty],
        6 => name: String = "string" [omitempty],
        7 => record: Vec<u8> = "[]uint8" [omitempty],
        8 => data: Vec<u8> = "[]uint8" [omitempty],
        9 => key: Vec<u8> = "[]uint8" [omitempty],
        10 => code: String = "string" [omitempty],
        11 => text: String = "string" [omitempty],
        12 => view: Vec<u8> = "[]uint8" [omitempty],
        13 => version: Vec<u8> = "[]uint8" [omitempty],
        14 => expected_version: Vec<u8> = "[]uint8" [omitempty],
        15 => expected_old: Vec<u8> = "[]uint8" [omitempty],
        16 => force: bool = "bool" [omitempty],
        17 => has_expected: bool = "bool" [omitempty],
        18 => prefix: Vec<u8> = "[]uint8" [omitempty],
        19 => after: Vec<u8> = "[]uint8" [omitempty],
        20 => limit: i64 = "int" [omitempty],
        21 => refs: Vec<RefInfo> = "[]wire.RefInfo" [omitempty],
        22 => next: Vec<u8> = "[]uint8" [omitempty],
        23 => holders: Vec<KeyHolders> = "[]wire.KeyHolders" [omitempty],
        24 => failed: Vec<KeyFailure> = "[]wire.KeyFailure" [omitempty],
        25 => rejected: Vec<KeyReject> = "[]wire.KeyReject" [omitempty],
        26 => short: Vec<KeyHolders> = "[]wire.KeyHolders" [omitempty],
        27 => unreachable: Vec<Vec<u8>> = "[][]uint8" [omitempty],
        28 => retry_after: i64 = "int64" [omitempty],
        29 => current: Vec<u8> = "[]uint8" [omitempty],
        30 => shortfall: i64 = "int" [omitempty],
        31 => has_current: bool = "bool" [omitempty],
        32 => reg: Vec<u8> = "[]uint8" [omitempty],
        33 => ballot: Vec<u8> = "[]uint8" [omitempty],
        34 => value: Vec<u8> = "[]uint8" [omitempty],
        35 => has_value: bool = "bool" [omitempty],
        36 => accepted: Vec<u8> = "[]uint8" [omitempty],
        37 => promised: Vec<u8> = "[]uint8" [omitempty],
        38 => not_after: i64 = "int64" [omitempty],
        39 => rows: Vec<ScanRow> = "[]wire.ScanRow" [omitempty],
        40 => more: bool = "bool" [omitempty],
        41 => since: u64 = "uint64" [omitempty],
        42 => token: Vec<u8> = "[]uint8" [omitempty],
        43 => weight: u32 = "uint32" [omitempty],
        44 => zone: String = "string" [omitempty],
        45 => addrs: Vec<String> = "[]string" [omitempty],
        46 => no_vote: bool = "bool" [omitempty],
        47 => g: u64 = "uint64" [omitempty],
        48 => nonce: Vec<u8> = "[]uint8" [omitempty],
        49 => seq: u64 = "uint64" [omitempty],
        50 => expand: bool = "bool" [omitempty],
        51 => params: Vec<u8> = "[]uint8" [omitempty],
        52 => sent: u64 = "uint64" [omitempty],
        53 => received: u64 = "uint64" [omitempty],
        54 => idle: bool = "bool" [omitempty],
        55 => marked: u64 = "uint64" [omitempty],
        56 => missing: Vec<Vec<u8>> = "[][]uint8" [omitempty],
        57 => status: Vec<u8> = "[]uint8" [omitempty],
        58 => node: Vec<u8> = "[]uint8" [omitempty],
        59 => error: String = "string" [omitempty],
        60 => pattern: String = "string" [omitempty],
        61 => deleted: Vec<String> = "[]string" [omitempty],
    }
}
cbor_struct! { #[derive(Clone, Debug, Default, PartialEq, Eq)] pub struct KeyHolders = "wire.KeyHolders" {
    0 => key: Option<Vec<u8>> = "[]uint8", 1 => holders: Vec<Vec<u8>> = "[][]uint8" [omitempty], } }
cbor_struct! { #[derive(Clone, Debug, Default, PartialEq, Eq)] pub struct KeyFailure = "wire.KeyFailure" {
    0 => key: Option<Vec<u8>> = "[]uint8", 1 => node: Option<Vec<u8>> = "[]uint8",
    2 => reason: String = "string", 3 => retry_after: i64 = "int64" [omitempty], } }
cbor_struct! { #[derive(Clone, Debug, Default, PartialEq, Eq)] pub struct KeyReject = "wire.KeyReject" {
    0 => key: Option<Vec<u8>> = "[]uint8", 1 => reason: String = "string", } }
cbor_struct! { #[derive(Clone, Debug, Default, PartialEq, Eq)] pub struct RefInfo = "wire.RefInfo" {
    0 => name: String = "string", 1 => key: Option<Vec<u8>> = "[]uint8", 2 => version: Option<Vec<u8>> = "[]uint8",
    3 => created_at: i64 = "int64", 4 => user: String = "string" [omitempty], } }
cbor_struct! { #[derive(Clone, Debug, Default, PartialEq, Eq)] pub struct ScanRow = "wire.ScanRow" {
    0 => reg: Option<Vec<u8>> = "[]uint8", 1 => promised: Vec<u8> = "[]uint8" [omitempty],
    2 => accepted: Vec<u8> = "[]uint8" [omitempty], 3 => value: Vec<u8> = "[]uint8" [omitempty],
    4 => has_value: bool = "bool" [omitempty], } }

// error.rs
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{}", self.message())]
pub struct RemoteError { pub code: String, pub text: String, pub view: Vec<u8>, pub retry_after: std::time::Duration }
impl RemoteError {
    pub fn message(&self) -> String;                        // "remote: <code>" or "remote: <code>: <text>"
    pub fn is(&self, target: &RemoteError) -> bool;         // same code; target text empty or equal
}
pub fn error_from_msg(m: &Msg) -> RemoteError;              // retry_after = m.retry_after ms (negative → ZERO)
pub fn err_msg(code: &str, text: &str) -> Msg;              // {0:10, 10:code, 11:text}, never stamped
pub fn as_remote<'a>(err: &'a (dyn std::error::Error + 'static)) -> Option<&'a RemoteError>;  // walks err, then source() chain (Go errors.As)
pub fn is_code(err: &(dyn std::error::Error + 'static), code: &str) -> bool;

// frame.rs
#[derive(Debug, thiserror::Error)]
pub enum ShortCause {
    #[error("EOF")] Eof,
    #[error("unexpected EOF")] UnexpectedEof,
    #[error("{}", dstore_gocompat::errno::io_error_text(.0))] Io(std::io::Error),
}
#[derive(Debug, thiserror::Error)]
pub enum WireError {
    #[error("EOF")] Eof,                                                     // clean end before a header
    #[error("unexpected EOF")] UnexpectedEof,                                // 1-3 header bytes
    #[error("wire: frame of {0} bytes exceeds limit 16777216")] TooLarge(u64),
    #[error("wire: short frame: {0}")] Short(ShortCause),
    #[error("wire: decode frame: {0}")] Decode(#[source] dstore_codec::DecodeError),
    #[error("{}", dstore_gocompat::errno::io_error_text(.0))] Io(#[source] std::io::Error),
    #[error("{0}")] Remote(#[source] RemoteError),                          // Expect on TErr
    #[error("wire: unexpected frame: type {got}, want {want}")] Protocol { got: i64, want: i64 },
    #[error("wire: key {index} has {len} bytes")] KeyLen { index: usize, len: usize },
}
pub fn encode_frame(m: &Msg) -> Result<Vec<u8>, WireError>;                  // u32be len ‖ payload; TooLarge writes nothing
pub async fn write_msg<W: tokio::io::AsyncWrite + Unpin + ?Sized>(w: &mut W, m: &Msg) -> Result<(), WireError>;  // one write_all
pub async fn read_msg<R: tokio::io::AsyncRead + Unpin + ?Sized>(r: &mut R) -> Result<Msg, WireError>;          // counting header loop; not cancel-safe
pub async fn expect<R: tokio::io::AsyncRead + Unpin + ?Sized>(r: &mut R, want: i64) -> Result<Msg, WireError>;
pub async fn write_err<W: tokio::io::AsyncWrite + Unpin + ?Sized>(w: &mut W, code: &str, text: &str) -> Result<(), WireError>;

// keys.rs
pub fn keys32(raw: &[Vec<u8>]) -> Result<Vec<[u8; 32]>, WireError>;           // KeyLen for the first bad entry
pub fn raw_keys(keys: &[[u8; 32]]) -> Vec<Vec<u8>>;                           // also Go RawIDs

// pack.rs (transport-iroh protocol)
pub const PACK_MAGIC: &[u8; 8] = b"AMBERPK\x03";
cbor_struct! { #[derive(Clone, Debug, Default, PartialEq)] pub struct ProtocolMsg = "protocol.Msg" {
    0 => typ: i64 = "int", 1 => name: String = "string" [omitempty], 2 => root: Vec<u8> = "[]uint8" [omitempty],
    3 => cas: bool = "bool" [omitempty], 4 => expected_old: Vec<u8> = "[]uint8" [omitempty],
    5 => record: Vec<u8> = "[]uint8" [omitempty], 6 => refs: Vec<ProtocolRefInfo> = "[]protocol.RefInfo" [omitempty],
    7 => keys: Vec<Vec<u8>> = "[][]uint8" [omitempty], 8 => data: Vec<u8> = "[]uint8" [omitempty],
    9 => key: Vec<u8> = "[]uint8" [omitempty], 10 => code: String = "string" [omitempty],
    11 => text: String = "string" [omitempty], 12 => current: Vec<u8> = "[]uint8" [omitempty],
    13 => token: Vec<u8> = "[]uint8" [omitempty], 14 => data_conns: i64 = "int" [omitempty],
    15 => data_ports: Vec<u16> = "[]uint16" [omitempty],
    16 => data_endpoints: Vec<ProtocolDataEndpointRec> = "[]protocol.DataEndpointRec" [omitempty],
    17 => names: Vec<String> = "[]string" [omitempty], } }
cbor_struct! { #[derive(Clone, Debug, Default, PartialEq, Eq)] pub struct ProtocolRefInfo = "protocol.RefInfo" {
    0 => name: String = "string", 1 => key: Option<Vec<u8>> = "[]uint8", 2 => created_at: i64 = "int64",
    3 => user: String = "string" [omitempty], } }
cbor_struct! { #[derive(Clone, Debug, Default, PartialEq, Eq)] pub struct ProtocolDataEndpointRec = "protocol.DataEndpointRec" {
    0 => id: Option<Vec<u8>> = "[]uint8", 1 => addrs: Vec<String> = "[]string" [omitempty], } }
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("remote: {code}: {text}")]                                           // keeps ": " when text is empty
pub struct ProtocolRemoteError { pub code: String, pub text: String, pub current: Vec<u8> }
#[derive(Debug, thiserror::Error)]
pub enum ProtocolFrameError {
    #[error("EOF")] Eof,
    #[error("unexpected EOF")] UnexpectedEof,
    #[error("protocol: frame of {0} bytes exceeds limit 16777216")] TooLarge(u64),
    #[error("protocol: short frame: {0}")] Short(ShortCause),
    #[error("protocol: decode frame: {0}")] Decode(#[source] dstore_codec::DecodeError),
    #[error("{}", dstore_gocompat::errno::io_error_text(.0))] Io(#[source] std::io::Error),
}
pub async fn read_protocol_msg<R: tokio::io::AsyncRead + Unpin + ?Sized>(r: &mut R) -> Result<ProtocolMsg, ProtocolFrameError>;
#[derive(Debug, thiserror::Error)]
pub enum PackReadError {
    #[error("{0}")] Frame(#[source] ProtocolFrameError),
    #[error("{0}")] Remote(#[source] ProtocolRemoteError),                  // TErr during a pack
    #[error("protocol: unexpected frame: type {0} during pack transfer")] Unexpected(i64),
}
/// SendPackRecords' chunkWriter + amberpack.Writer: exact 1 MiB TData frames ({0:7, 8:chunk}); dropping without
/// finish() writes nothing more (Go: a source error aborts without the terminator or the partial chunk).
pub struct PackSender<'a, W: tokio::io::AsyncWrite + Unpin + ?Sized> { /* w, buf, wrote_magic */ }
impl<'a, W: tokio::io::AsyncWrite + Unpin + ?Sized> PackSender<'a, W> {
    pub fn new(w: &'a mut W) -> PackSender<'a, W>;
    pub async fn add_record(&mut self, rec: &[u8]) -> Result<(), WireError>;   // magic before the first record
    pub async fn finish(self) -> Result<(), WireError>;                         // magic if none, 0x00, remainder TData, TDataEnd {0:8}
}
/// protocol.NewPackReader: TData → bytes, TDataEnd → EOF, TErr → Remote, other → Unexpected; errors sticky.
/// A clean EOF at a frame boundary also reads as EOF (Ok(0)), as in Go.
pub struct PackReader<R> { /* r, cur, pos, done, err */ }
impl<R: tokio::io::AsyncRead + Unpin> PackReader<R> {
    pub fn new(r: R) -> PackReader<R>;
    pub async fn read(&mut self, buf: &mut [u8]) -> Result<usize, PackReadError>;
    pub async fn drain(&mut self) -> Result<u64, PackReadError>;               // io.Copy(io.Discard, pr)
    pub fn into_inner(self) -> R;
}
/// amberpack.NewReader(pr).Records() over a PackReader, reading from the reader's own buffer (no BufReader),
/// so drain() consumes TDataEnd.
pub struct PackRecords<R> { /* pr: PackReader<R>, state */ }
impl<R: tokio::io::AsyncRead + Unpin> PackRecords<R> {
    pub fn new(pr: PackReader<R>) -> PackRecords<R>;
    pub async fn next(&mut self) -> Option<Result<amber_store_core::amberpack::RawRecord, PackRecordsError>>;  // one error, then None
    pub async fn drain(&mut self) -> Result<u64, PackReadError>;
    pub fn into_pack_reader(self) -> PackReader<R>;
}
#[derive(Debug, thiserror::Error)]
pub enum PackRecordsError {
    #[error("{0}")] Read(#[source] PackReadError),
    #[error("amberpack: malformed pack stream: {0}")] Stream(String),        // "reading magic: unexpected EOF", "bad magic", "bad record tag 0x02", "record payload N exceeds limit M", …
    #[error("{0}")] Record(#[source] amber_store_core::amberpack::Error),     // parse_record failures (same texts as Go)
}

// admin.rs (node payloads carried in Msg.params / Msg.status)
cbor_struct! { #[derive(Clone, Debug, Default, PartialEq)] pub struct AdminRequest = "node.AdminRequest" {
    0 => op: String = "string", 1 => node: Vec<u8> = "[]uint8" [omitempty], 2 => weight: u32 = "uint32" [omitempty],
    3 => zone: String = "string" [omitempty], 4 => replicas: u8 = "uint8" [omitempty], 5 => dead: bool = "bool" [omitempty],
    6 => allow_unsafe: bool = "bool" [omitempty], 7 => force: bool = "bool" [omitempty], 8 => key: Vec<u8> = "[]uint8" [omitempty],
    9 => garbage: f64 = "float64" [omitempty], 10 => tolerate: bool = "bool" [omitempty], 11 => forwarded: bool = "bool" [omitempty],
    12 => pause: bool = "bool" [omitempty], 13 => rate: u64 = "uint64" [omitempty], 14 => names: Vec<String> = "[]string" [omitempty], } }
cbor_struct! { #[derive(Clone, Debug, Default, PartialEq, Eq)] pub struct AdminReply = "node.AdminReply" {
    0 => text: String = "string" [omitempty], 1 => token: Vec<u8> = "[]uint8" [omitempty], 2 => view: Vec<u8> = "[]uint8" [omitempty],
    3 => names: Vec<String> = "[]string" [omitempty], 4 => key: Vec<u8> = "[]uint8" [omitempty],
    5 => ticket: String = "string" [omitempty], 6 => gc: Vec<u8> = "[]uint8" [omitempty], } }
cbor_struct! { #[derive(Clone, Debug, Default, PartialEq, Eq)] pub struct VoterStat = "node.VoterStat" {
    0 => id: Option<Vec<u8>> = "[]uint8", 1 => calls: u64 = "uint64", 2 => failures: u64 = "uint64", 3 => p99ms: i64 = "int64", } }
cbor_struct! { #[derive(Clone, Debug, Default, PartialEq, Eq)] pub struct Status = "node.Status" {
    0 => id: Option<Vec<u8>> = "[]uint8", 1 => epoch: u64 = "uint64", 2 => incarnation: u64 = "uint64",
    3 => packs: i64 = "int", 4 => records: u64 = "uint64", 5 => bytes: i64 = "int64", 6 => pins: i64 = "int",
    7 => unreachable: Vec<Vec<u8>> = "[][]uint8" [omitempty], 8 => pending_packs: i64 = "int",
    9 => transition: String = "string" [omitempty], 10 => gc: String = "string" [omitempty],
    11 => lease_holder: Vec<u8> = "[]uint8" [omitempty], 12 => voters: Vec<VoterStat> = "[]node.VoterStat" [omitempty],
    13 => writable: bool = "bool", 14 => free_bytes: i64 = "int64", 15 => total_bytes: i64 = "int64",
    16 => puts: u64 = "uint64", 17 => gets: u64 = "uint64", 18 => ref_puts: u64 = "uint64",
    19 => bytes_in: u64 = "uint64", 20 => bytes_out: u64 = "uint64", 21 => amnesiac: bool = "bool" [omitempty],
    22 => retired: bool = "bool" [omitempty], 23 => scrub_age_sec: i64 = "int64" [omitempty],
    24 => last_live: u64 = "uint64" [omitempty], 25 => corrupt: i64 = "int" [omitempty],
    26 => is_holder: bool = "bool" [omitempty], 27 => unaudited_keys: i64 = "int" [omitempty],
    28 => watchers: i64 = "int" [omitempty], } }
pub fn decode_status(b: &[u8]) -> Result<Status, dstore_codec::DecodeError>;
pub fn decode_admin_reply(b: &[u8]) -> Result<AdminReply, dstore_codec::DecodeError>;
```

### 4.4 `dstore-ticket`

**Ports:**
- dstore `ticket/ticket.go` (96);
- go-iroh v0.2.0 `key/key.go` `ParseEndpointID`, `decodeBase32OrHex`, `NewPublicKey`, z-base-32 hint, and `key_core.go`.

**Spec:** codec-wire-ticket §2.4, §3.4, §4.4 (ticket), §5 G7-G11, §7 K5-K7.

The curve check uses `iroh_base::PublicKey::from_bytes` (ed25519-dalek). A golden vector must confirm
it accepts exactly what filippo `edwards25519.Point.SetBytes` accepts (all-0xff is valid in Go).

```rust
pub const PREFIX: &str = "dstore1";
cbor_struct! { #[derive(Clone, Debug, Default, PartialEq, Eq)] pub struct Member = "ticket.Member" {
    0 => id: Option<Vec<u8>> = "[]uint8", 1 => addrs: Vec<String> = "[]string" [omitempty], } }
cbor_struct! { #[derive(Clone, Debug, Default, PartialEq, Eq)] pub struct Ticket = "ticket.Ticket" {
    0 => cluster_id: Option<Vec<u8>> = "[]uint8", 1 => incarnation: u64 = "uint64",
    2 => members: Option<Vec<Member>> = "[]ticket.Member", } }
impl Ticket {
    pub fn encode(&self) -> String;                    // "dstore1" + lower(base32 std nopad (CBOR))
    pub fn ids(&self) -> String;                       // IDs(): hex of 32-byte member ids, first occurrence, ","-joined
    pub fn members(&self) -> &[Member];                // None → &[]
}
impl std::fmt::Display for Ticket { .. }              // encode()
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeyError {
    #[error("failed to decode hex string")] Hex,
    #[error("failed to decode base32 string")] Base32,
    #[error("invalid length")] Length,
    #[error("data is not a valid public key")] KeyData,
    #[error("{0}: input is z-base-32, use ParseEndpointIDZ32")] Z32(Box<KeyError>),
}
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TicketError {
    #[error("ticket: empty")] Empty,
    #[error("ticket: {0}")] Base32(#[source] dstore_gocompat::base32::CorruptInputError),
    #[error("ticket: {0}")] Cbor(#[source] dstore_codec::DecodeError),
    #[error("ticket: no members")] NoMembers,
    #[error("ticket: {} is neither a dstore1 ticket nor a node id: {source}", dstore_gocompat::quote::quote(.field))]
    NotId { field: Vec<u8>, #[source] source: KeyError },
}
pub fn parse(s: &[u8]) -> Result<Ticket, TicketError>;                        // Go ticket.Parse, validation order §2.4.4
pub fn parse_endpoint_id(s: &[u8]) -> Result<[u8; 32], KeyError>;             // go-iroh ParseEndpointID on the given bytes
pub fn is_valid_public_key(b: &[u8; 32]) -> bool;
```

### 4.5 `dstore-view`

**Ports:**
- dstore `placement/placement.go` (272) and `view/view.go` (469);
- `worktree.TicketFromView` (`worktree/flow.go:306-315`);
- `node.ShortID` (`node/status.go:118-124`).

**Spec:** view-placement (all), Appendix A generator.

`NodeId` is defined here and re-exported by transport, client and the facade. It is never validated
as a curve point; validation happens only at dial time (transport-iroh).

```rust
pub mod placement {
    pub const SLOT_BITS: u32 = 20;
    pub const SLOTS: u32 = 1 << SLOT_BITS;
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct NodeId(pub [u8; 32]);
    impl NodeId {
        pub fn from_slice_lossy(b: &[u8]) -> NodeId;    // Go copy(): min(len, 32) bytes, zero-padded
        pub fn as_bytes(&self) -> &[u8; 32];
        pub fn to_hex(&self) -> String;                 // view.IDString
        pub fn short(&self) -> String;                  // view.ShortID: hex of [0..4]
    }
    impl From<[u8; 32]> for NodeId { .. }
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Member { pub id: NodeId, pub weight: u32, pub zone: Vec<u8> }   // empty zone → the 32 id bytes as zone key
    pub fn slot(key: &[u8; 32]) -> u32;
    pub fn salt(id: &NodeId) -> u64;
    pub fn fmix64(x: u64) -> u64;
    pub fn log2fix(x: u64) -> u64;                      // panics "placement: Log2Fix(0)"
    pub fn l(slot: u32, salt: u64) -> u64;
    pub struct Set { /* members, salts */ }
    impl Set {
        pub fn new(members: Vec<Member>) -> Set;
        pub fn members(&self) -> &[Member];
        pub fn rank(&self, slot: u32) -> Vec<u32>;       // stable 128-bit rational order, weight-0 members last by id
        pub fn owners(&self, slot: u32, r: usize) -> Vec<u32>;
    }
    pub struct Table { /* set, r, lazy RwLock<HashMap<u32, Arc<[u32]>>> for owners and rank */ }
    impl Table {
        pub fn new(set: Set, r: usize) -> Table;
        pub fn set(&self) -> &Set;
        pub fn replicas(&self) -> usize;
        pub fn owners(&self, slot: u32) -> std::sync::Arc<[u32]>;
        pub fn rank(&self, slot: u32) -> std::sync::Arc<[u32]>;
        pub fn owner_ids(&self, key: &[u8; 32]) -> Vec<NodeId>;
        pub fn rank_ids(&self, key: &[u8; 32]) -> Vec<NodeId>;
        pub fn is_owner(&self, key: &[u8; 32], id: &NodeId) -> bool;
    }
}

pub mod view {
    use super::placement::NodeId;
    pub const VOTER_SYNC_DONE: i64 = 0;
    pub const VOTER_SYNC_PENDING: i64 = 1;
    cbor_struct! { #[derive(Clone, Debug, Default, PartialEq, Eq)] pub struct Voter = "view.Voter" {
        0 => id: Option<Vec<u8>> = "[]uint8", 1 => since: u64 = "uint64", } }
    cbor_struct! { #[derive(Clone, Debug, Default, PartialEq, Eq)] pub struct Former = "view.Former" {
        0 => id: Option<Vec<u8>> = "[]uint8", 1 => until: i64 = "int64", } }
    cbor_struct! { #[derive(Clone, Debug, Default, PartialEq, Eq)] pub struct DataEndpoint = "view.DataEndpoint" {
        0 => id: Option<Vec<u8>> = "[]uint8", 1 => addrs: Vec<String> = "[]string" [omitempty], } }
    cbor_struct! { #[derive(Clone, Debug, Default, PartialEq, Eq)] pub struct Node = "view.Node" {
        0 => id: Option<Vec<u8>> = "[]uint8", 1 => weight: u32 = "uint32", 2 => addrs: Vec<String> = "[]string" [omitempty],
        3 => data: Vec<DataEndpoint> = "[]view.DataEndpoint" [omitempty], 4 => token: Vec<u8> = "[]uint8" [omitempty],
        5 => zone: String = "string" [omitempty], 6 => incarnation: u64 = "uint64" [omitempty], 7 => writable: bool = "bool", } }
    cbor_struct! { #[derive(Clone, Debug, Default, PartialEq, Eq)] pub struct Ramp = "view.Ramp" {
        0 => node: Option<Vec<u8>> = "[]uint8", 1 => target: u32 = "uint32", 2 => step: i64 = "int", } }
    cbor_struct! { #[derive(Clone, Debug, Default, PartialEq, Eq)] pub struct Acl = "view.ACL" {
        0 => allowed: Vec<Option<Vec<u8>>> = "[][]uint8" [omitempty], 1 => admins: Vec<Option<Vec<u8>>> = "[][]uint8" [omitempty], } }
    cbor_struct! { #[derive(Clone, Debug, Default, PartialEq, Eq)] pub struct Pending = "view.Pending" {
        0 => nodes: Option<Vec<Node>> = "[]view.Node", 1 => replicas: u8 = "uint8", 2 => id: u64 = "uint64",
        3 => participants_ack: Vec<Option<Vec<u8>>> = "[][]uint8" [omitempty], 4 => participants: Vec<Option<Vec<u8>>> = "[][]uint8" [omitempty],
        5 => frozen: bool = "bool" [omitempty], 6 => round: u32 = "uint32" [omitempty],
        7 => primary_done: Vec<Option<Vec<u8>>> = "[][]uint8" [omitempty], 8 => done: Vec<Option<Vec<u8>>> = "[][]uint8" [omitempty],
        9 => frozen_at: i64 = "int64" [omitempty], 10 => reason: String = "string" [omitempty],
        11 => ramp: Option<Box<Ramp>> = "view.Ramp" [omitempty], 12 => since: i64 = "int64" [omitempty], } }
    cbor_struct! { #[derive(Clone, Debug, Default, PartialEq, Eq)] pub struct View = "view.View" {
        0 => cluster_id: Option<Vec<u8>> = "[]uint8", 1 => incarnation: u64 = "uint64", 2 => epoch: u64 = "uint64",
        3 => version: u64 = "uint64", 4 => placement_epoch: u64 = "uint64", 5 => replicas: u8 = "uint8",
        6 => min_replicas: u8 = "uint8", 7 => voters: Option<Vec<Voter>> = "[]view.Voter", 8 => voter_sync: i64 = "int",
        9 => voter_sync_cursor: Vec<u8> = "[]uint8" [omitempty], 10 => nodes: Option<Vec<Node>> = "[]view.Node",
        11 => pending: Option<Box<Pending>> = "view.Pending" [omitempty], 12 => former: Vec<Former> = "[]view.Former" [omitempty],
        13 => fenced: Vec<Option<Vec<u8>>> = "[][]uint8" [omitempty], 14 => recovered_inc: u64 = "uint64" [omitempty],
        15 => recovered_epoch: u64 = "uint64" [omitempty], 16 => rebalance_pause: bool = "bool" [omitempty],
        17 => rate_cap: u64 = "uint64" [omitempty], 18 => voter_sync_target: Vec<u8> = "[]uint8" [omitempty],
        19 => voter_sync_add: bool = "bool" [omitempty], 20 => deferred_voters: Vec<Option<Vec<u8>>> = "[][]uint8" [omitempty],
        21 => acl: Option<Box<Acl>> = "view.ACL" [omitempty], 22 => ramps: Vec<Ramp> = "[]view.Ramp" [omitempty],
        23 => remove_voters: Vec<Option<Vec<u8>>> = "[][]uint8" [omitempty], } }
    #[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
    pub enum ViewError {
        #[error("view: decode: {0}")] Decode(#[source] dstore_codec::DecodeError),
        #[error("view: bad node id {}", dstore_gocompat::quote::quote(.0))] BadNodeId(Vec<u8>),
        #[error("view: change drops {dropped} nodes at once with R={replicas}; every key owned only by them would be lost (use --force)")]
        ChangeDropsTooMany { dropped: usize, replicas: i64 },
        #[error("view: not a member")] NotMember,
    }
    impl View {
        pub fn encode(&self) -> Vec<u8>;
        pub fn decode(b: &[u8]) -> Result<View, ViewError>;
        pub fn compare(&self, incarnation: u64, epoch: u64) -> std::cmp::Ordering;   // Greater = this view is newer
        pub fn nodes(&self) -> &[Node];
        pub fn node(&self, id: &NodeId) -> Option<&Node>;           // nodes, then pending.nodes; exact 32-byte match
        pub fn is_member(&self, id: &NodeId) -> bool;
        pub fn data_endpoint_owner(&self, id: &NodeId) -> Option<NodeId>;
        pub fn is_former(&self, id: &NodeId) -> bool;
        pub fn is_voter(&self, id: &NodeId) -> bool;
        pub fn voter_ids(&self) -> Vec<NodeId>;
        pub fn quorum(&self) -> usize;
        pub fn all_members(&self) -> Vec<NodeId>;
    }
    impl Node {
        pub fn nid(&self) -> NodeId;
        pub fn zone_or_id(&self) -> Vec<u8>;                         // zone bytes, else raw id bytes (any length)
    }
    pub fn parse_node_id(s: &[u8]) -> Result<NodeId, ViewError>;    // exactly 64 hex chars, any case
    pub fn id_string(id: &NodeId) -> String;
    pub fn short_id(id: &NodeId) -> String;
    pub fn node_short_id(b: &[u8]) -> String;                        // "?" unless len == 32
    pub fn contains(ids: &[Option<Vec<u8>>], id: &NodeId) -> bool;
    pub fn add_id(ids: &mut Vec<Option<Vec<u8>>>, id: &NodeId);
    pub fn ids_of(raw: &[Vec<u8>]) -> Vec<NodeId>;                   // zero-padded or truncated copies
    pub fn sort_nodes(nodes: &mut [Node]);
    pub fn default_min_replicas(r: u8) -> u8;
    pub fn validate_change(cur: &[Node], target: &[Node], replicas: i64, force: bool) -> Result<(), ViewError>;
    pub struct Placement { /* view: Arc<View>, cur: Table, pending: Option<Table> */ }
    impl Placement {
        pub fn new(view: std::sync::Arc<View>) -> Placement;
        pub fn view(&self) -> &std::sync::Arc<View>;
        pub fn owners(&self, key: &[u8; 32]) -> Vec<NodeId>;
        pub fn pending_owners(&self, key: &[u8; 32]) -> Option<Vec<NodeId>>;   // None without pending; Some([]) with empty pending
        pub fn write_set(&self, key: &[u8; 32]) -> Vec<NodeId>;
        pub fn read_order(&self, key: &[u8; 32]) -> Vec<NodeId>;            // no dedup within the current table
        pub fn is_owner(&self, key: &[u8; 32], id: &NodeId) -> bool;
        pub fn is_pending_owner(&self, key: &[u8; 32], id: &NodeId) -> bool;
        pub fn in_write_set(&self, key: &[u8; 32], id: &NodeId) -> bool;
    }
}
pub use placement::NodeId;
pub use view::*;
/// worktree.TicketFromView: first ≤ 4 of v.nodes in view order; no nodes → members None (f6).
pub fn ticket_from_view(v: &view::View) -> dstore_ticket::Ticket;
```

### 4.6 `dstore-transport`

**Ports:**
- dstore `transport/transport.go` (243) and `transport/mem.go` (365);
- `ParseAddrs` from `transport/iroh.go` (247-261), plus go-iroh `netaddr` parse/format rules (`endpointaddr.go`, `relayurl.go`).

**Specs:**
- transport §2.1-2.2, §2.4, §2.9, §3.1, §4.3-4.4, §4.7, §4.9, §5.1-5.4, §5.7, §5.9;
- client-core §2.8;
- verification §4.6 (mem transport).

Decisions:
- Port the Pool exactly: grow to `per_peer`, round robin, the 2 s failed-dial window only with no
  live conn, the second check against the unfiltered list, only `open_stream` failures drop, never shrink.
- The dial lock is a `tokio::sync::Mutex<()>`, awaited outside `ctx.run` (not ctx-aware, as in Go).
- The mem transport keeps Go's quirk: a blocked read survives connection close.

```rust
pub use dstore_view::NodeId;
pub use dstore_gocompat::ctx::{Ctx, CtxError};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PathInfo { pub direct: bool, pub rtt: std::time::Duration }       // rtt ZERO = not measured

pub trait SendStream: tokio::io::AsyncWrite + Send + Unpin + 'static {
    fn finish(&mut self);                                                     // Go Close()/CloseWrite(): FIN; errors ignored
}
pub trait RecvStream: tokio::io::AsyncRead + Send + Unpin + 'static {
    fn cancel_read(&mut self, code: u64);                                     // Go CancelRead(code): STOP_SENDING
}
pub struct Stream { pub send: Box<dyn SendStream>, pub recv: Box<dyn RecvStream> }
impl Stream {
    pub fn new(send: Box<dyn SendStream>, recv: Box<dyn RecvStream>) -> Stream;
    pub fn close_write(&mut self);
    pub fn cancel_read(&mut self, code: u64);
    pub fn close_stream(&mut self);                                            // wire.CloseStream: finish, then cancel_read(0)
}

#[async_trait::async_trait]
pub trait Conn: Send + Sync + 'static {
    fn remote_id(&self) -> NodeId;
    fn alpn(&self) -> String;
    async fn open_stream(&self, ctx: &Ctx) -> Result<Stream, TransportError>;
    async fn accept_stream(&self, ctx: &Ctx) -> Result<Stream, TransportError>;
    fn close(&self);                                                          // CONNECTION_CLOSE app code 0, empty reason
    fn path(&self) -> PathInfo;
    fn is_closed(&self) -> bool;                                              // Go: <-Done() would not block
    async fn closed(&self);
}
#[async_trait::async_trait]
pub trait Endpoint: Send + Sync + 'static {
    fn id(&self) -> NodeId;
    async fn dial(&self, ctx: &Ctx, id: NodeId, addrs: Vec<String>, alpn: &str) -> Result<std::sync::Arc<dyn Conn>, TransportError>;
    async fn accept(&self, ctx: &Ctx) -> Result<std::sync::Arc<dyn Conn>, TransportError>;
    fn addrs(&self) -> Vec<String>;
    async fn close(&self);
}
pub type AddrsFn = std::sync::Arc<dyn Fn(NodeId) -> Vec<String> + Send + Sync>;

#[derive(Debug, Clone, thiserror::Error)]
pub enum TransportError {
    #[error("transport: closed")] Closed,
    #[error("transport: peer recently unreachable")] RecentlyUnreachable,
    #[error("transport: no candidate addresses for {0}")] NoCandidates(String),     // 10 hex chars (go-iroh Short)
    #[error("transport: bind: {0}")] Bind(String),
    #[error("transport: pkarr publisher: {0}")] PkarrPublisher(String),
    #[error("dial {addr}: {source}")] DialAddr { addr: String, source: Box<TransportError> },
    #[error("discovery: {0}")] Discovery(#[source] Box<TransportError>),
    #[error("{}", joined(.0))] Joined(Vec<TransportError>),                          // errors.Join: "\n"-separated
    #[error("data is not a valid public key")] InvalidKey,
    #[error("iroh: no reachable address for endpoint")] NoAddress,
    #[error("iroh: cannot connect to self")] SelfConnect,
    #[error("{0}")] Ctx(#[from] CtxError),
    #[error("mem: local endpoint is down")] MemLocalDown,
    #[error("mem: {0} unreachable")] MemUnreachable(String),
    #[error("mem: {0} not bound")] MemNotBound(String),
    #[error("mem: {0} does not speak {1}")] MemAlpn(String, String),
    #[error("{0}")] Quic(String),                                                    // iroh/noq inner text (DD-4)
}
fn joined(errs: &[TransportError]) -> String;

pub struct Pool { /* ep, addrs, per_peer, std::sync::Mutex<State{conns, next, dialing, failed}> */ }
impl Pool {
    pub fn new(ep: std::sync::Arc<dyn Endpoint>, addrs: AddrsFn, per_peer: usize) -> Pool;   // 0 → 1
    pub fn endpoint(&self) -> &std::sync::Arc<dyn Endpoint>;
    pub async fn get(&self, ctx: &Ctx, id: NodeId, alpn: &str) -> Result<std::sync::Arc<dyn Conn>, TransportError>;
    pub fn drop_peer(&self, id: NodeId, alpn: &str);                                          // Go Drop
    pub fn path(&self, id: NodeId, alpn: &str) -> Option<PathInfo>;                           // first live conn
    pub fn close(&self);
    /// Go Pool.Call: get; open_stream (error → drop_peer); write_msg; close_write; read one frame under ctx
    /// (ctx end → cancel_read(0) + finish, CtxError); TErr → Remote; close_stream on every exit.
    pub async fn call(&self, ctx: &Ctx, id: NodeId, alpn: &str, req: &dstore_wire::Msg) -> Result<dstore_wire::Msg, CallError>;
    pub async fn open(&self, ctx: &Ctx, id: NodeId, alpn: &str) -> Result<Stream, TransportError>;
}
#[derive(Debug, thiserror::Error)]
pub enum CallError {
    #[error("{0}")] Transport(#[source] TransportError),
    #[error("{0}")] Wire(#[source] dstore_wire::WireError),
    #[error("{0}")] Ctx(#[source] CtxError),
    #[error("{0}")] Remote(#[source] dstore_wire::RemoteError),
}

pub mod addr {
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct GoRelayUrl(pub String);                                   // normalised Go url.String()
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum GoTransportAddr {
        Relay(GoRelayUrl),
        Ip { ip: std::net::IpAddr, zone: Option<String>, port: u16 },    // IPv4-mapped IPv6 kept as IPv6
        Custom { id: u64, data: Vec<u8> },
    }
    impl std::fmt::Display for GoTransportAddr { .. }                   // "relay:<url>", "ip:1.2.3.4:5", "ip:[fe80::1%en0]:7", "<id:x>_<hex>" (no "custom:")
    impl GoTransportAddr { pub fn is_relay(&self) -> bool; }
    pub fn parse_relay_url(s: &str) -> Result<GoRelayUrl, String>;      // "failed to parse relay URL: parse \"…\": …"
    pub fn parse_addr_port(s: &str) -> Result<(std::net::IpAddr, Option<String>, u16), String>;   // netip.ParseAddrPort texts
    pub fn parse_transport_addr(s: &str) -> Result<GoTransportAddr, String>;                        // netaddr.ParseTransportAddr texts
    pub fn parse_addrs(addrs: &[String]) -> Vec<GoTransportAddr>;       // Go ParseAddrs: bare ip:port fallback, silent skip
}

pub mod mem {
    pub const PIPE_LIMIT: usize = 4 << 20;
    pub struct Network { /* std::sync::Mutex<{endpoints, down, cut, delay}> */ }
    impl Network {
        pub fn new() -> std::sync::Arc<Network>;
        pub fn bind(self: &std::sync::Arc<Self>, id: super::NodeId, alpns: &[&str]) -> std::sync::Arc<MemEndpoint>;
        pub fn set_down(&self, id: super::NodeId, down: bool);
        pub fn partition(&self, a: super::NodeId, b: super::NodeId, cut: bool);
        pub fn set_delay(&self, d: std::time::Duration);
    }
    pub struct MemEndpoint { .. }                                        // impl Endpoint; addrs() = ["mem:<shortid>"]; path {direct: true, rtt: 1ms}
}
```

### 4.7 `dstore-transport-iroh`

**Ports:**
- dstore `transport/iroh.go` (457) and `transport/ifaces.go` (55);
- `relayModeOf` (`cmd/dstore/main.go:123-137`);
- the go-iroh v0.2.0 default relay map (`relay/relay.go:23-30,249-256`);
- the lookup semantics of `iroh/endpoint.go` (`lookupAddr`, `connectEarly`);
- the go-iroh `iroh/mdns` resolver (`mdns.go`, `dnsmsg.go`).

**Specs:**
- transport §2.3, §2.5-§2.8, §2.10, §3.2-§3.4, §4.2, §4.5-§4.8, §4.10, §5.5-§5.8, §6-§8;
- client-core §4.5 is superseded where §9 says so.

Decisions (§5.12 has the reasons):
- **Rust iroh setup.** Pin `iroh = "=1.2.0"` with default features off and `tls-ring` +
  `fast-apple-datapath` on; `PortmapperConfig::Disabled`; build from `presets::Minimal`.
- **Relays.**
  - The default relay map is go-iroh's canary hosts, with QUIC port 7842.
  - A `--relay` URL that `url::Url` cannot parse gives `RelayMode::Custom(RelayMap::empty())` (DD-13).
  - With relays enabled, bind waits up to 10 s for a home relay (`online()`).
- **Discovery.** Nothing is attached to the endpoint; discovery runs only in `discover_dial`.
  - mDNS uses the ported go-iroh resolver, **not** iroh-mdns-address-lookup.
  - DNS uses `iroh::address_lookup::DnsAddressLookup` with origin `dns.iroh.link.`, never the
    `n0_dns()` helpers.
- **Transport config.** keepalive 5 s, max idle 60 s, 1024 concurrent bidi streams, `initial_rtt`
  explicitly 333 ms (the RTT sentinel), stream receive window 16 MiB, send window 64 MiB.
- **Dial phases.** One `Endpoint::connect` per phase. Each phase is capped at
  `min(ctx deadline, CONNECT_PHASE_CAP = 10 s)`, and the direct phase at `direct_timeout` (2 s).
- **Node-side announcing.** `announce = true` (mDNS responder, pkarr publisher) is not implemented in
  v1: `bind_iroh` returns `TransportError::Bind("announce is node-side and not implemented")`.

```rust
pub const GO_DEFAULT_RELAYS: [&str; 4] = [
    "https://use1-1.relay.n0.iroh-canary.iroh.link.",
    "https://usw1-1.relay.n0.iroh-canary.iroh.link.",
    "https://euc1-1.relay.n0.iroh-canary.iroh.link.",
    "https://aps1-1.relay.n0.iroh-canary.iroh.link.",
];
pub const RELAY_QUIC_PORT: u16 = 7842;
pub const KEEP_ALIVE: std::time::Duration = std::time::Duration::from_secs(5);
pub const MAX_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
pub const MAX_INCOMING_BIDI_STREAMS: u32 = 1024;
pub const NOQ_INITIAL_RTT: std::time::Duration = std::time::Duration::from_millis(333);
pub const STREAM_RECEIVE_WINDOW: u32 = 16 << 20;
pub const SEND_WINDOW: u64 = 64 << 20;
pub const DIRECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
pub const ONLINE_WAIT: std::time::Duration = std::time::Duration::from_secs(10);
pub const MDNS_LOOKUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);
pub const CONNECT_PHASE_CAP: std::time::Duration = std::time::Duration::from_secs(10);
pub const CLOSE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RelayChoice { Default, Custom(dstore_transport::addr::GoRelayUrl) }
/// cmd/dstore relayModeOf: no_relay → None; url → Custom (parse error returned verbatim); else Default.
pub fn relay_mode_of(url: &str, no_relay: bool) -> Result<Option<RelayChoice>, String>;

pub struct IrohConfig {
    pub secret_key: iroh::SecretKey,
    pub alpns: Vec<String>,                               // client: empty
    pub relay: Option<RelayChoice>,                       // None = relays disabled
    pub advertise: Option<Vec<std::net::SocketAddr>>,     // node-side
    pub bind_addr: Option<std::net::SocketAddr>,          // tests / node
    pub loopback: bool,
    pub direct_timeout: Option<std::time::Duration>,      // None → DIRECT_TIMEOUT
    pub discover: bool,
    pub announce: bool,                                   // must be false in v1
    pub logger: Option<dstore_gocompat::slog::Logger>,    // None → Logger::default_logger()
}
pub struct IrohEndpoint { /* ep: iroh::Endpoint, id, cfg, addrs: Mutex<Vec<String>>, discovery, bg: CancellationToken */ }
pub async fn bind_iroh(ctx: &dstore_gocompat::ctx::Ctx, cfg: IrohConfig) -> Result<std::sync::Arc<IrohEndpoint>, dstore_transport::TransportError>;
#[async_trait::async_trait]
impl dstore_transport::Endpoint for IrohEndpoint { .. }   // dial = direct phase, relay+direct phase, discover_dial (transport §4.5)
impl IrohEndpoint {
    pub fn raw(&self) -> &iroh::Endpoint;                  // Go IrohEndpoint.Raw
    pub async fn close_bounded(&self, d: std::time::Duration);
}
pub struct IrohConn { /* conn: iroh::endpoint::Connection */ }   // impl Conn; path() maps rtt == NOQ_INITIAL_RTT → ZERO
pub fn generate_secret_key() -> iroh::SecretKey;
/// GoTransportAddr → iroh candidate; None when it cannot be converted (unparseable relay URL, unknown zone name).
pub fn to_iroh_addr(a: &dstore_transport::addr::GoTransportAddr) -> Option<iroh::TransportAddr>;
/// Go form of a relay URL as nodes publish it ("relay:" prefix added by the caller).
pub fn go_relay_string(u: &iroh::RelayUrl) -> String;

pub mod mdns {
    pub const SERVICE_NAME: &str = "irohv1";
    pub fn endpoint_label(id: &[u8; 32]) -> String;                        // RFC 4648 base32 lowercase, no padding
    pub fn build_query(id: &[u8; 32]) -> Vec<u8>;                          // 2 PTR questions, no compression
    pub struct Announcement { pub id: [u8; 32], pub addrs: Vec<std::net::SocketAddr>, pub relay: Option<String>, pub user_data: Option<String> }
    pub fn parse_announcement(packet: &[u8]) -> Option<Announcement>;       // records from any section, compression pointers ≤ 32
    pub struct MdnsResolver { .. }
    impl MdnsResolver {
        /// Joins 224.0.0.251:5353 (required) and [ff02::fb]:5353 (best effort) with SO_REUSEADDR+SO_REUSEPORT on every
        /// up multicast interface; failure → Err(text), the caller logs WARN "transport: mdns discovery unavailable" error=<text>.
        pub async fn start(logger: dstore_gocompat::slog::Logger) -> Result<std::sync::Arc<MdnsResolver>, String>;
        /// Cache hit → at once; else send the query and poll the cache every 25 ms until `timeout` or ctx end.
        pub async fn resolve(&self, ctx: &dstore_gocompat::ctx::Ctx, id: [u8; 32], timeout: std::time::Duration) -> Option<Announcement>;
    }
}
pub mod ifaces {
    pub const BRIDGE_PREFIXES: [&str; 10] = ["docker", "br-", "cni", "flannel", "veth", "virbr", "lxc", "utun", "awdl", "llw"];
    pub fn interface_ips() -> Vec<std::net::IpAddr>;                       // transport/ifaces.go filters; node-side advertise
}
```

### 4.8 `dstore-client`

**Ports:** dstore `client/client.go` (403), `rank.go` (71), `batch.go` (29), `progress.go` (181),
`refs.go` (150), `watch.go` (245), `objects.go` (395), `fetch.go` (309), `tree.go` (479).

**Specs:**
- client-core (part A, all);
- client-transfer (part B, all);
- view-placement §2.3;
- codec-wire-ticket §2.6;
- core-rs-gaps §2.2-§2.5, §4.3 (`verify_record`, `stored_size_of`).

The code is normative wherever these disagree with `architecture/dstore.md` (client-core §8.1,
client-transfer §2.10).

**Module ownership:**
- part A: `cluster`, `rank`, `batch`, `progress`, `refs`, `watch`, `error`;
- part B: `objects`, `fetch`, `tree`.

Part B codes against the `pub(crate)` seams below, stubbed in L0.

**Runtime.** `Cluster` requires a multi-thread tokio runtime:
- single-object local-store reads (`has`, `get`, `get_record`, `stored_size`) run inline, as Go does;
- `reachable_keys`, `check_complete` and `write_parallel` run in `spawn_blocking` with
  `Arc<packstore::Store>`;
- the recursive `want` of `PullTree` is iterative, keeping Go's pre-order queue order;
- the per-held-subtree completeness test is a sequential local check (same boolean as
  `fstree::check_complete`, core-rs-gaps G12);
- the final gate uses core-rs `check_complete` for its error text.

```rust
pub use dstore_view::NodeId;
pub use dstore_gocompat::ctx::{Ctx, CtxError};
pub use dstore_gocompat::slog::Logger;
use amber_store_core::{amberpack, fstree, key, packstore, reference};

pub const DEFAULT_BATCH_BYTES: usize = 16 << 20;
pub const BATCH_KEYS: usize = 8192;
pub const GET_BATCH_KEYS: usize = 2048;
pub const GET_BATCH_BYTES: usize = 8 << 20;
pub const GET_EST_MAX: u64 = 64 << 10;
pub const PULL_WRITE_BYTES: usize = 16 << 20;
pub const DIAL_MEMBER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
pub const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

// ---- cluster.rs (client.go) ----
#[derive(Clone)]
pub struct Config {
    pub endpoint: Option<std::sync::Arc<dyn dstore_transport::Endpoint>>,  // None → Error::NoEndpoint (checked first)
    pub ticket: dstore_ticket::Ticket,
    pub conns: usize,                        // 0 → 4
    pub jobs: usize,                         // 0 → 8
    pub logger: Option<Logger>,              // None → Logger::default_logger()
    pub gc_interval: std::time::Duration,    // ZERO → 4 h
    pub request_timeout: std::time::Duration,// ZERO → 2 min
    pub batch_bytes: usize,                  // 0 → 16 MiB, then min(64 MiB)
    pub watch_idle: std::time::Duration,     // ZERO → 2 min
}
impl Default for Config { .. }                // all zero / None

#[derive(Clone)]
pub struct Cluster { /* Arc<Inner { cfg, log, ep, pool, state: RwLock<{view, placement, boot_addrs, backoff, failures, unreach}> }> */ }
impl Cluster {
    pub async fn dial(ctx: &Ctx, cfg: Config) -> Result<Cluster, Error>;   // Go Dial, client-core §2.3
    pub fn close(&self);                                                    // closes the pool only
    pub fn endpoint(&self) -> &std::sync::Arc<dyn dstore_transport::Endpoint>;
    pub fn view(&self) -> Option<std::sync::Arc<dstore_view::View>>;
    pub fn placement(&self) -> Option<std::sync::Arc<dstore_view::Placement>>;
    pub async fn refresh_view(&self, ctx: &Ctx) -> Result<(), Error>;
    pub fn primary(&self, key: &[u8; 32]) -> Option<NodeId>;
    pub fn owners(&self, key: &[u8; 32]) -> Vec<NodeId>;
    pub fn write_set(&self, key: &[u8; 32]) -> Vec<NodeId>;
    pub fn read_order(&self, key: &[u8; 32]) -> Vec<NodeId>;               // one (view, placement) snapshot
    pub fn nodes(&self) -> Vec<NodeId>;
    pub async fn status(&self, ctx: &Ctx, id: NodeId) -> Result<Vec<u8>, Error>;                         // no reply type check
    pub async fn admin(&self, ctx: &Ctx, id: Option<NodeId>, req: &dstore_wire::AdminRequest) -> Result<Vec<u8>, Error>;  // None → anyNode

    // crate-internal seams (part A → part B)
    pub(crate) fn cfg(&self) -> &Config;                                   // defaults applied
    pub(crate) fn log(&self) -> &Logger;
    pub(crate) fn pool(&self) -> &dstore_transport::Pool;
    pub(crate) fn stamp(&self, m: &mut dstore_wire::Msg);
    pub(crate) async fn call(&self, ctx: &Ctx, id: NodeId, m: &mut dstore_wire::Msg) -> Result<dstore_wire::Msg, Error>;
    pub(crate) async fn call_retry(&self, ctx: &Ctx, id: NodeId, m: &mut dstore_wire::Msg) -> Result<dstore_wire::Msg, Error>;  // ≤ 4 calls on stale-view
    pub(crate) async fn any_node(&self, ctx: &Ctx, m: &mut dstore_wire::Msg) -> Result<dstore_wire::Msg, Error>;   // ≤ 5 calls per node on stale-view
    pub(crate) fn handle_err(&self, id: NodeId, err: &Error);
    pub(crate) fn ok(&self, id: NodeId);
    pub(crate) fn penalty(&self, id: NodeId) -> i64;
    pub(crate) fn preferred(&self, ids: &[NodeId]) -> Vec<NodeId>;
    pub(crate) async fn probe_hinted(&self, ctx: &Ctx);
    pub(crate) fn path_attrs(&self, id: NodeId) -> Vec<dstore_gocompat::slog::Attr>;
}

// ---- rank.rs ----
pub(crate) fn rtt_class(rtt: std::time::Duration) -> i64;                   // <5ms 0, <25ms 1, <100ms 2, else 3
pub(crate) fn rank_owners(ids: &[NodeId], penalty: impl Fn(NodeId) -> i64, path: impl Fn(NodeId) -> Option<dstore_transport::PathInfo>) -> Vec<NodeId>;

// ---- batch.rs ----
pub type RecordSizer = std::sync::Arc<dyn Fn(&[u8; 32]) -> usize + Send + Sync>;
pub(crate) fn batches(keys: &[[u8; 32]], size: &dyn Fn(&[u8; 32]) -> usize, max_bytes: usize, max_keys: usize) -> Vec<Vec<[u8; 32]>>;

// ---- progress.rs ----
pub type Progress = std::sync::Arc<dyn Fn(&ProgressReport) + Send + Sync>;   // called under the tracker mutex; must not re-enter the client
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProgressReport { pub objects: i64, pub total_objects: i64, pub bytes: i64, pub total_bytes: i64, pub nodes: Vec<NodeProgress> }
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NodeProgress { pub id: NodeId, pub direct: bool, pub rtt: std::time::Duration, pub in_flight: i64, pub awaiting: i64, pub bytes: i64 }
#[derive(Clone, Default)]
pub struct PutObserver {
    pub start: Option<std::sync::Arc<dyn Fn(NodeId) + Send + Sync>>,
    pub sent: Option<std::sync::Arc<dyn Fn(NodeId, usize) + Send + Sync>>,
    pub flushed: Option<std::sync::Arc<dyn Fn(NodeId) + Send + Sync>>,
    pub done: Option<std::sync::Arc<dyn Fn(NodeId, bool) + Send + Sync>>,
}
pub(crate) struct Tracker { .. }
impl Tracker {
    pub(crate) fn new(c: &Cluster, prog: Option<Progress>) -> std::sync::Arc<Tracker>;
    pub(crate) fn observer(self: &std::sync::Arc<Self>) -> PutObserver;
    pub(crate) fn totals(&self, objects: i64, done: i64, bytes: i64);
    pub(crate) fn more(&self, bytes: i64);
    pub(crate) fn objects(&self, n: i64);
    pub(crate) fn bytes(&self) -> i64;
}
pub(crate) fn count_keys(m: &std::collections::HashMap<NodeId, Vec<[u8; 32]>>, size: &dyn Fn(&[u8; 32]) -> usize) -> (i64, i64);
pub fn human_bytes(n: i64) -> String;                                         // "1024.0 KiB" for 1048575
pub fn rate(bytes: i64, took: std::time::Duration) -> String;                // ZERO → "-"

// ---- refs.rs ----
pub struct Ref { pub name: String, pub record: Vec<u8>, pub version: Vec<u8>, pub reference: reference::Reference }
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Cond { pub expected_version: Vec<u8>, pub versioned: bool, pub expected_old: Vec<u8>, pub keyed: bool, pub force: bool }
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{}", self.message())]
pub struct CasMismatch { pub current: Vec<u8>, pub record: Vec<u8>, pub version: Vec<u8>, pub has_current: bool }
impl CasMismatch { pub fn message(&self) -> String; }                         // "cas mismatch: reference is absent" | "cas mismatch: current key <%x>"
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("incomplete: {shortfall} keys short")]
pub struct Incomplete { pub sample: Vec<[u8; 32]>, pub shortfall: i64 }
impl Cluster {
    pub async fn ref_get(&self, ctx: &Ctx, name: &str) -> Result<Ref, Error>;
    pub async fn ref_put(&self, ctx: &Ctx, record: &[u8], cond: &Cond) -> Result<Vec<u8>, Error>;
    pub async fn ref_delete(&self, ctx: &Ctx, name: &str, cond: &Cond) -> Result<(), Error>;
    pub async fn ref_list(&self, ctx: &Ctx, prefix: &[u8]) -> Result<Vec<dstore_wire::RefInfo>, Error>;   // errors not mapped through refErr
}
/// Go reference.ValidateName over raw argv bytes: empty, then > 1024 bytes, then "must be valid UTF-8", then core-rs validate_name.
pub fn validate_name_bytes(name: &[u8]) -> Result<(), String>;
/// Same for reference.ValidateUser ("user must …").
pub fn validate_user_bytes(user: &[u8]) -> Result<(), String>;

// ---- watch.rs ----
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RefChange { pub name: String, pub key: Option<Vec<u8>>, pub version: Vec<u8>, pub created_at: i64, pub user: String, pub deleted: bool, pub synced: bool, pub node: NodeId }
pub type WatchStream = std::pin::Pin<Box<dyn futures::Stream<Item = Result<RefChange, Error>> + Send + 'static>>;
impl Cluster {
    /// async-stream generator (pull semantics). Ends without an item when ctx ends; yields Err for bad-request/unauthorized, then ends.
    pub fn watch_refs(&self, ctx: Ctx, pattern: String, known: std::collections::HashMap<String, Vec<u8>>) -> WatchStream;
}

// ---- objects.rs (part B) ----
pub type RecordSource = std::sync::Arc<dyn Fn(&[u8; 32]) -> Result<Vec<u8>, Error> + Send + Sync>;
pub struct MissingResult {
    pub lacking: std::collections::HashMap<NodeId, Vec<[u8; 32]>>,
    pub holders: std::collections::HashMap<[u8; 32], Vec<NodeId>>,
    pub failed: std::collections::HashMap<[u8; 32], std::sync::Arc<Error>>,
}
pub struct PutResult {
    pub holders: std::collections::HashMap<[u8; 32], Vec<NodeId>>,
    pub failed: std::collections::HashMap<[u8; 32], Vec<dstore_wire::KeyFailure>>,
    pub rejected: std::collections::HashMap<[u8; 32], String>,
    pub errors: std::collections::HashMap<NodeId, std::sync::Arc<Error>>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GetResult { pub key: [u8; 32], pub record: Vec<u8> }
/// Lazy: nothing starts until first polled; dropping it cancels the fetcher. Records arrive in arrival order.
/// A ctx error after the last record is yielded as Err (Go yields ctx.Err()).
pub struct GetStream { .. }
impl futures::Stream for GetStream { type Item = Result<GetResult, Error>; .. }
impl GetStream { pub fn missing(&self) -> Vec<[u8; 32]>; }                  // accumulates across polls, as Go
impl Cluster {
    pub async fn missing(&self, ctx: &Ctx, keys: &[[u8; 32]], pin: bool) -> Result<MissingResult, Error>;   // never Err in v0.1.9
    pub async fn put(&self, ctx: &Ctx, by_primary: std::collections::HashMap<NodeId, Vec<[u8; 32]>>, src: RecordSource, size: RecordSizer, obs: PutObserver) -> PutResult;
    pub fn placed(&self, key: &[u8; 32], holders: &[NodeId]) -> bool;
    pub fn get(&self, ctx: &Ctx, keys: Vec<[u8; 32]>) -> GetStream;
}
pub fn verify_record(raw: &amberpack::RawRecord) -> Result<([u8; 32], Vec<u8>), Error>;

// ---- fetch.rs (part B, internal) ----
pub(crate) fn est_size(k: &[u8; 32]) -> usize;                                // 46 + min(length, 64 KiB)

// ---- tree.rs (part B) ----
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PushStats { pub keys: i64, pub uploaded: i64, pub bytes: i64, pub version: Vec<u8> }
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullStats { pub keys: i64, pub fetched: i64, pub bytes: i64, pub root: key::Key, pub record: Vec<u8>, pub version: Vec<u8> }
impl Default for PullStats { .. }                                             // root = Key([0; 32])
impl Cluster {
    pub async fn push(&self, ctx: &Ctx, local: std::sync::Arc<packstore::Store>, root: key::Key, name: &str, user: &str, cond: Cond, prog: Option<Progress>) -> Result<PushStats, Error>;
    pub async fn pull(&self, ctx: &Ctx, local: std::sync::Arc<packstore::Store>, name: &str, prog: Option<Progress>) -> Result<PullStats, Error>;
    pub async fn pull_tree(&self, ctx: &Ctx, local: std::sync::Arc<packstore::Store>, root: key::Key, st: &mut PullStats, prog: Option<Progress>) -> Result<(), Error>;
}
pub(crate) fn stored_size_of(st: &packstore::Store, k: &[u8; 32]) -> usize;   // 46 + slen when stored, else key length

// ---- corefmt.rs (part A): Go texts for core-rs errors that reach users (core-rs-gaps G4, §2.7) ----
pub mod corefmt {
    /// fstree WalkError Display with names re-quoted by gocompat::quote (Go %q), else identical to core-rs.
    pub fn walk_error_text<E: std::fmt::Display>(e: &amber_store_core::fstree::WalkError<E>) -> String;
    /// cbor::Error with Go cborx texts: "cborx: …" prefix, plain "unexpected EOF" for truncation.
    pub fn cbor_error_text(e: &amber_store_core::cbor::Error) -> String;
}

// ---- error.rs ----
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("client: no endpoint")] NoEndpoint,
    #[error("client: ticket names no nodes")] TicketNamesNoNodes,
    #[error("client: no bootstrap node answered: {0}")] NoBootstrap(#[source] Box<Error>),
    #[error("client: unexpected reply {0}")] UnexpectedReply(i64),
    #[error("client: no nodes")] NoNodes,
    #[error("client: unknown reference")] UnknownRef,
    #[error("{0}")] CasMismatch(#[source] CasMismatch),
    #[error("{0}")] Incomplete(#[source] Incomplete),
    #[error("client: watch stream idle")] WatchIdle,
    #[error("wire: unexpected frame: type {0}")] UnexpectedFrame(i64),          // "%w: type %d" of wire.ErrProtocol
    #[error("{0}")] Remote(#[source] dstore_wire::RemoteError),
    #[error("{0}")] Transport(#[source] dstore_transport::TransportError),
    #[error("{0}")] Wire(#[source] dstore_wire::WireError),
    #[error("{0}")] PackRecords(#[source] dstore_wire::PackRecordsError),
    #[error("{0}")] Ctx(#[source] CtxError),
    #[error("{0}")] View(#[source] dstore_view::ViewError),
    #[error("{0}")] Reference(#[source] reference::Error),
    #[error("{0}")] Key(#[source] key::Error),
    #[error("{0}")] Amberpack(#[source] amberpack::Error),
    #[error("{0}")] Packstore(#[source] packstore::Error),
    #[error("{0}")] ChildKeys(#[source] fstree::ChildKeysError),
    #[error("no owners")] NoOwners,
    #[error("payload hashes to {want}, not {key}")] PayloadHash { want: String, key: String },
    #[error("walk local tree: {0}")] WalkLocalTree(#[source] fstree::WalkError<packstore::Error>),
    #[error("negotiate {} at its primary: {source}", dstore_gocompat::fmt::hex_lower(.key8))] Negotiate { key8: [u8; 8], source: std::sync::Arc<Error> },
    #[error("upload to {node}: {source}")] UploadTo { node: String, source: std::sync::Arc<Error> },
    #[error("record {} rejected: {reason}", dstore_gocompat::fmt::hex_lower(.key8))] RecordRejected { key8: [u8; 8], reason: String },
    #[error("push: {count} keys could not be placed; owners not confirming: {names}")] NotPlaced { count: usize, names: String },  // names = %v of []string
    #[error("{placed}; last upload error: {last}")] NotPlacedLastError { placed: Box<Error>, last: std::sync::Arc<Error> },
    #[error("pull: fetch ended early")] FetchEndedEarly,
    #[error("pull: object {} not found in the cluster", dstore_gocompat::fmt::hex_lower(.key8))] PullObjectNotFound { key8: [u8; 8] },
    #[error("pull: tree incomplete after fetch: {0}")] PullIncomplete(#[source] fstree::WalkError<packstore::Error>),
    #[error("{0}")] Other(String),                                              // RecordSource failures from callers
}
impl Error {
    pub fn remote(&self) -> Option<&dstore_wire::RemoteError>;                  // errors.As through the source chain
    pub fn is_code(&self, code: &str) -> bool;
    pub fn cas_mismatch(&self) -> Option<&CasMismatch>;
    pub fn incomplete(&self) -> Option<&Incomplete>;
    pub fn is_unknown_ref(&self) -> bool;
    pub fn ctx_error(&self) -> Option<CtxError>;
}
impl From<dstore_transport::CallError> for Error { .. }                        // Remote/Transport/Wire/Ctx 1:1
```

Seams used across crates: the worktree calls `ref_get`, `pull_tree`, `push`, `view`; the CLI calls
everything public.

### 4.9 `dstore-udiff`

**Ports:**
- go-udiff v0.4.1, the Unified path only:
  - `unified.go` (`Unified`, `toUnified`, `splitLines`, `addEqualLines`, `unified.String`);
  - `diff.go` (`Edit`, `validate`, `SortEdits`, `lineEdits`, `expandEdit`);
  - `ndiff.go` `Lines`;
  - `lcs/old.go` (`twosided` with limit 50; `backward` is skipped), `lcs/common.go`, `lcs/labels.go`, `lcs/sequence.go`;
- Go 1.26.5 `sort/zsortfunc.go` (`pdqsort_func` and helpers), `sort/sort.go:59-80`, `sort/slice.go`, and `sort.Stable` for `validate`.

**Spec:** worktree §3.9 (points 1-14), §5 items 5-7, §6 go-udiff tests.

**Rule.** Port line by line. No diff crate, no std sort inside `lcs`: Go's unstable pdqsort order
changes 5% of heavy random edits. No external dependencies.

```rust
pub fn unified(old_label: &[u8], new_label: &[u8], old: &[u8], new: &[u8]) -> Vec<u8>;   // udiff.Unified (3 context lines)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit { pub start: usize, pub end: usize, pub new: Vec<u8> }
pub fn lines(before: &[u8], after: &[u8]) -> Vec<Edit>;
pub fn to_unified(old_label: &[u8], new_label: &[u8], content: &[u8], edits: &[Edit], context_lines: usize) -> Result<Vec<u8>, String>;
pub mod lcs {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Diff { pub start: usize, pub end: usize, pub repl_start: usize, pub repl_end: usize }
    pub fn diff_lines(a: &[&[u8]], b: &[&[u8]]) -> Vec<Diff>;
}
pub mod gosort {
    pub fn slice<T>(v: &mut [T], less: impl FnMut(&T, &T) -> bool);          // sort.Slice = pdqsort_func(data, 0, n, bits.Len(n))
    pub fn slice_stable<T>(v: &mut [T], less: impl FnMut(&T, &T) -> bool);   // sort.SliceStable / sort.Stable (insertion + symMerge)
    pub fn is_sorted<T>(v: &[T], less: impl FnMut(&T, &T) -> bool) -> bool;
}
```

### 4.10 `dstore-worktree`

**Ports:** dstore `worktree/tree.go` (245), `change.go` (238), `scan.go` (284), `merge.go` (80),
`apply.go` (310), `diff.go` (206), `flow.go` (331), `xattr.go` (55), `xattr_darwin.go`,
`xattr_linux.go`. `TicketFromView` lives in `dstore-view` and is re-exported here.

**Specs:**
- worktree (all);
- core-rs-gaps §2.3-§2.8, §4.3 (`read_xattrs`, `set_xattr`, meta), G5, G9, G11;
- verification §3.3.

Decisions:
- Paths and `Config` fields are bytes.
- JSON goes through `gocompat::json`; filesystem operations with Go error texts through
  `gocompat::os`/`gocompat::path`; errors are rendered through `gocompat::errno`.
- Change lists keep walk order.
- Scan, `diff_trees`, apply, ingest and `hash_file` are blocking; callers wrap them in `spawn_blocking`.
- Go's ctx is observed only by client calls, so Ctrl+C never aborts `apply`.
- Crate-private core-rs helpers (xattr reader, device numbers, mtime, `read_dir_sorted`) are
  duplicated in `sys`, and `mkdev` is added.

```rust
pub use dstore_view::ticket_from_view;
use amber_store_core::{fstree::Entry, key::Key, packstore};
pub const DIR: &str = ".dstore";
pub const RACY_WINDOW_NS: i64 = 2_000_000_000;
pub const MAX_DIFF_BYTES: u64 = 16 << 20;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Config { pub ticket: Vec<u8>, pub name: Vec<u8>, pub relay: Vec<u8>, pub no_relay: bool, pub no_discovery: bool, pub user: Vec<u8> }
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State { pub base: Key, pub remote: Key, pub has_remote: bool, pub remote_version: Option<Vec<u8>>, pub synced_at: dstore_gocompat::time::GoTime }
pub struct Tree { pub root: Vec<u8>, pub config: Config, pub state: State, pub store: std::sync::Arc<packstore::Store> }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind { Added, Deleted, Modified, TypeChanged, ModeChanged, MetaChanged }
impl std::fmt::Display for Kind { .. }                       // "new" "deleted" "modified" "type" "mode" "meta"
#[derive(Clone, Debug)]
pub struct Change { pub path: Vec<u8>, pub kind: Kind, pub old: Option<std::sync::Arc<Entry>>, pub new: Option<std::sync::Arc<Entry>> }
#[derive(Clone, Debug)]
pub struct Conflict { pub path: Vec<u8>, pub local: Change, pub incoming: Change }
pub struct FetchResult { pub exists: bool, pub up_to_date: bool, pub key: Key, pub stats: dstore_client::PullStats }
pub struct PullResult { pub fetch: FetchResult, pub up_to_date: bool, pub applied: Vec<Change>, pub conflicts: Vec<Conflict> }
pub struct PushResult { pub root: Key, pub nothing: bool, pub recovered: bool, pub built: packstore::WriteStats, pub stats: dstore_client::PushStats }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteState { UpToDate, Moved, Absent }
pub struct Status { pub changes: Vec<Change>, pub meta_only: usize, pub remote: RemoteState, pub incoming: Vec<Change> }
pub type Getter<'a> = &'a (dyn Fn(Key) -> Result<Vec<u8>, packstore::Error> + Sync);

/// Display = the Go texts of worktree §2 (sentinels verbatim; wraps "%s: %w", "working copy %s: …",
/// "bad state file: base: …", "refusing unsafe path %q", "xattr %q: %w", "%s: mknod: %w", "chmod: %w", …).
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not a dstore working copy (no .dstore in this or any parent directory)")] NotWorkingCopy,
    #[error("incomplete clone: delete the directory and clone again")] Incomplete,
    #[error("the reference does not exist on the cluster: nothing to pull")] NoRemote,
    #[error("the cluster's tree moved since your last sync: pull first, or --force")] RemoteMoved,
    #[error("the reference was deleted on the cluster: --force to recreate it")] RemoteDeleted,
    #[error("conflicting changes: resolve them, or --force to take the cluster's side")] Conflict,
    #[error("reference changed on the cluster since your last fetch: pull first, or --force ({0})")] RefChanged(#[source] dstore_client::Error),
    #[error("client: unknown reference: {}", String::from_utf8_lossy(.0))] UnknownRefName(Vec<u8>),
    #[error("too large to diff")] TooLarge,
    #[error("{0}")] Msg(String),                               // fully formatted Go texts without a typed cause
    #[error("{text}")] Wrapped { text: String, #[source] source: Box<dyn std::error::Error + Send + Sync + 'static> },
    #[error("{0}")] Client(#[source] dstore_client::Error),
    #[error("{0}")] Path(#[source] dstore_gocompat::errno::PathError),
    #[error("{0}")] Packstore(#[source] packstore::Error),
    #[error("{0}")] Ingest(#[source] amber_store_core::ingest::Error),
    #[error("{0}")] Walk(#[source] amber_store_core::fstree::WalkError<packstore::Error>),
    #[error("{0}")] Key(#[source] amber_store_core::key::Error),
}
impl Error { pub fn is_conflict(&self) -> bool; pub fn is_no_remote(&self) -> bool; }

// tree.rs (blocking)
pub fn empty_tree() -> (Key, Vec<u8>);                         // 2001bbe6…, bytes 80
pub fn find(dir: &[u8]) -> Result<Vec<u8>, Error>;
pub fn remove(dir: &[u8]) -> Result<(), Error>;
impl Tree {
    pub fn open(dir: &[u8]) -> Result<Tree, Error>;           // lock before state (a locked copy reports the lock error)
    pub fn create(dir: &[u8], cfg: Config) -> Result<Tree, Error>;   // no state file
    pub fn close(self) -> Result<(), Error>;
    pub fn get(&self, k: Key) -> Result<Vec<u8>, packstore::Error>;
    pub fn save_config(&self) -> Result<(), Error>;
    pub fn save_state(&self) -> Result<(), Error>;
    pub fn status(&self, jobs: usize) -> Result<Status, Error>;
}
// change.rs, scan.rs, merge.rs, apply.rs (blocking)
pub fn type_name(mode: u64) -> String;
pub fn is_dir(e: Option<&Entry>) -> bool;
pub fn same_content(a: &Entry, b: &Entry) -> bool;
pub fn equivalent(a: &Entry, b: &Entry) -> bool;
pub fn compare(old: &Entry, new: &Entry) -> Option<Kind>;
pub fn diff_trees(get: Getter<'_>, a: Key, b: Key) -> Result<Vec<Change>, Error>;
pub fn scan(root: &[u8], base: Key, get: Getter<'_>, synced_at: dstore_gocompat::time::GoTime, jobs: usize) -> Result<Vec<Change>, Error>;
pub fn merge(local: &[Change], incoming: &[Change]) -> (Vec<Change>, Vec<Conflict>);
pub fn apply(root: &[u8], changes: &[Change], get: Getter<'_>) -> Result<(), Error>;
// diff.rs (blocking; writes as it goes, partial output may precede an error)
pub enum SourceError { TooLarge, Other(Error) }
pub trait Source { fn content(&self, path: &[u8], e: &Entry) -> (Option<Vec<u8>>, Option<SourceError>); }  // data may come with an error
pub struct TreeSource<'a> { pub get: Getter<'a> }
pub struct DiskSource { pub root: Vec<u8> }
impl Source for TreeSource<'_> { .. }
impl Source for DiskSource { .. }
pub fn unified(w: &mut dyn std::io::Write, changes: &[Change], old: &dyn Source, new: &dyn Source) -> Result<(), Error>;
pub fn stat(w: &mut dyn std::io::Write, changes: &[Change], old: &dyn Source, new: &dyn Source) -> Result<(), Error>;
// flow.rs (async over the client)
impl Tree {
    pub async fn fetch(&mut self, ctx: &dstore_gocompat::ctx::Ctx, cl: &dstore_client::Cluster, prog: Option<dstore_client::Progress>) -> Result<FetchResult, Error>;  // Go Fetch (saves state)
    pub async fn pull(&mut self, ctx: &dstore_gocompat::ctx::Ctx, cl: &dstore_client::Cluster, force: bool, jobs: usize, prog: Option<dstore_client::Progress>) -> (PullResult, Result<(), Error>);
    pub async fn push(&mut self, ctx: &dstore_gocompat::ctx::Ctx, cl: &dstore_client::Cluster, user: &str, force: bool, jobs: usize, prog: Option<dstore_client::Progress>) -> Result<PushResult, Error>;
    pub fn refresh_ticket(&mut self, cl: &dstore_client::Cluster) -> Result<(), Error>;
}
pub async fn clone(ctx: &dstore_gocompat::ctx::Ctx, cl: &dstore_client::Cluster, dir: &[u8], cfg: Config, prog: Option<dstore_client::Progress>) -> Result<(Tree, FetchResult), Error>;
pub async fn init(ctx: &dstore_gocompat::ctx::Ctx, cl: &dstore_client::Cluster, dir: &[u8], cfg: Config, prog: Option<dstore_client::Progress>) -> Result<(Tree, FetchResult), Error>;

pub mod sys {                                                  // x/sys/unix equivalents (core-rs-gaps §4.3)
    pub fn read_xattrs(path: &std::path::Path) -> std::io::Result<std::collections::BTreeMap<Vec<u8>, Vec<u8>>>;   // macOS follows symlinks, Linux l*
    pub fn set_xattr(path: &std::path::Path, name: &[u8], value: &[u8]) -> std::io::Result<()>;
    pub fn major(dev: u64) -> u32;
    pub fn minor(dev: u64) -> u32;
    pub fn mkdev(major: u32, minor: u32) -> u64;
    pub fn unix_nano(md: &std::fs::Metadata) -> i64;               // wrapping mtime*1e9 + nsec
}
```

### 4.11 `dstore-gocli`

**Ports:**
- the subset of urfave/cli v2.27.7 dstore uses: `app.go`, `command.go`, `help.go`, `template.go`, `flag*.go`, `context.go`, `errors.go`;
- Go `flag.(*FlagSet).parseOne` and its value parsers;
- `text/tabwriter` with minwidth 1, tabwidth 8, padding 2, pad `' '`, flags 0.

**Spec:** cli §2.1-§2.2, §3.1-§3.2, §4.2, §6 (urfave tests).

Decisions:
- No clap.
- Arguments and string flag values are `OsString`, and env values are read as `OsString`.
- Help is written to stdout, which the caller passes in.
- Every quirk of §1.4 is reproduced and locked by snapshots.

```rust
pub enum FlagKind {
    String { default: &'static str },
    Bool { default: bool },
    Int { default: i64 },
    Int64 { default: i64 },
    Uint { default: u64 },
    Float64 { default: f64 },
    Duration { default_ns: i64 },
    StringSlice,
}
pub struct FlagDef {
    pub name: &'static str,
    pub aliases: &'static [&'static str],          // only help/h, version/v
    pub kind: FlagKind,
    pub usage: &'static str,
    pub env: &'static [&'static str],
    pub required: bool,
    pub disable_default_text: bool,                // help and version flags
}
pub type Action = for<'a> fn(&'a Context) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), CliError>> + Send + 'a>>;
pub struct CommandDef {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub usage: &'static str,
    pub args_usage: &'static str,
    pub description: &'static str,
    pub flags: Vec<FlagDef>,
    pub subcommands: Vec<CommandDef>,
    pub action: Option<Action>,                    // None → help action
}
pub struct AppDef { pub name: &'static str, pub usage: &'static str, pub version: String, pub flags: Vec<FlagDef>, pub commands: Vec<CommandDef> }
#[derive(Clone, Debug, PartialEq)]
pub enum FlagValue { Str(std::ffi::OsString), Bool(bool), I64(i64), U64(u64), F64(f64), DurationNs(i64), Slice(Vec<String>) }
pub struct Context { /* levels root..current: (command path, values, set-on-cli, set-from-env), args */ }
impl Context {
    pub fn string(&self, name: &str) -> String;            // lossy; searches this level then ancestors; "" when undefined
    pub fn os_string(&self, name: &str) -> std::ffi::OsString;
    pub fn bool(&self, name: &str) -> bool;
    pub fn int(&self, name: &str) -> i64;
    pub fn int64(&self, name: &str) -> i64;
    pub fn uint(&self, name: &str) -> u64;
    pub fn float64(&self, name: &str) -> f64;
    pub fn duration_ns(&self, name: &str) -> i64;
    pub fn string_slice(&self, name: &str) -> Vec<String>;
    pub fn is_set(&self, name: &str) -> bool;              // on the command line, or found in the environment (even empty)
    pub fn args(&self) -> &[std::ffi::OsString];
    pub fn narg(&self) -> usize;
    pub fn arg(&self, n: usize) -> &std::ffi::OsStr;       // "" when missing (Go Args().Get)
    pub fn first(&self) -> &std::ffi::OsStr;
}
#[derive(Debug)]
pub enum CliError { Msg(String), Exit { msg: String, code: i32 } }   // Msg → "dstore: <msg>", exit 1; Exit → "<msg>", code
impl CliError { pub fn msg(e: impl std::fmt::Display) -> CliError; }
/// §2.2.2 of cli.md: setup, env application, Go flag parse, Incorrect Usage + help, help/version flags,
/// required flags, subcommand dispatch, action or help action.
pub async fn run(app: &AppDef, args: Vec<std::ffi::OsString>, stdout: &mut (dyn std::io::Write + Send)) -> Result<(), CliError>;
pub mod goflag { .. }                                       // parse_one port; value parsers via gocompat::strconv/time
pub mod help { .. }                                         // templates, row stringification, TabWriter
```

### 4.12 `dstore-cli`

**Ports:** `cmd/dstore/main.go` (698), `client.go` (574), `wc.go` (489 at HEAD), `size.go` (46),
`tui.go` (374).

**Specs:**
- cli (all);
- worktree §2.10;
- client-core §2.9, §3.8;
- view-placement §3.4 (status view lines);
- core-rs-gaps G3, G11, G13, G16.

**Modules and owners:**
- `app`: command table only (every definition of cli §2.8, in Go order);
- `nodeside` (§2.2);
- `common`: dial, admin, print_status, record_payload, open_local, cluster_get, hex_decode, signals;
- `cmd_admin`: cluster, token, node, voter, transition, gc, catalog actions;
- `cmd_client`: store push/pull, refs, watch, ref, ls, cat;
- `cmd_wc`: clone, init, fetch, pull, push, status, diff, resolve_ticket, wc_config, push_user, describe_change, filter_paths;
- `size`;
- `progress`: latest, rate meter, status line, UiModel, renderer, TeaHandler, colours.

```rust
pub const VERSION: &str = match option_env!("DSTORE_VERSION") { Some(v) => v, None => "dev" };
/// Process entry used by the root bin: restore SIGPIPE to SIG_DFL, build a multi-thread tokio runtime, run(),
/// flush stdout and stderr, return the exit code. The caller exits with std::process::exit (runtime not dropped).
pub fn main_entry() -> i32;
pub async fn run(args: Vec<std::ffi::OsString>) -> i32;      // prints "dstore: <err>" (after errno rewrite) or "<msg>"; returns 0/1/3
pub fn app() -> dstore_gocli::AppDef;
/// Go runtime panic parity (DD-7): writes "panic: <text>\n" to stderr and exits 2.
pub fn go_panic_exit(text: &str) -> !;

pub mod nodeside {
    pub fn node_side_error(cmd: &str) -> dstore_gocli::CliError;
    pub const STORE_TICKET_UNSUPPORTED: &str = "deriving a ticket from --store needs the node's Pebble meta store, which dstore-client-rs does not implement; pass --ticket or $DSTORE_TICKET, or use the Go dstore binary";
    pub const CATALOG_RESTORE_UNSUPPORTED: &str = "catalog restore writes through the node's paxos acceptor, which dstore-client-rs does not implement; use the Go dstore binary";
    pub fn local_ticket(dir: &[u8]) -> Result<dstore_ticket::Ticket, dstore_gocli::CliError>;   // §2.2 B: identity read, then STORE_TICKET_UNSUPPORTED
}
pub mod size {
    pub fn parse_size(s: &str) -> Result<i64, String>;
    pub fn pack_size(c: &dstore_gocli::Context) -> Result<i64, String>;
}
pub mod common {
    pub struct NetOpts { pub relay: String, pub no_relay: bool, pub no_discovery: bool }
    pub fn net_opts_of(c: &dstore_gocli::Context) -> NetOpts;
    pub fn logger(c: &dstore_gocli::Context) -> dstore_gocompat::slog::Logger;               // TextHandler on stderr
    pub fn signal_ctx() -> dstore_gocompat::ctx::Ctx;                                        // SIGINT+SIGTERM → cancel; handlers stay installed
    pub struct Session { pub cluster: dstore_client::Cluster, pub endpoint: std::sync::Arc<dstore_transport_iroh::IrohEndpoint> }
    impl Session { pub async fn close(self); }                                                // cluster.close(); endpoint.close_bounded(3 s)
    pub async fn dial_cluster(ctx: &dstore_gocompat::ctx::Ctx, c: &dstore_gocli::Context, log: &dstore_gocompat::slog::Logger) -> Result<Session, dstore_gocli::CliError>;
    pub async fn dial_ticket(ctx: &dstore_gocompat::ctx::Ctx, t: dstore_ticket::Ticket, n: &NetOpts, log: &dstore_gocompat::slog::Logger) -> Result<Session, dstore_gocli::CliError>;
    pub async fn admin(ctx: &dstore_gocompat::ctx::Ctx, cl: &dstore_client::Cluster, req: &dstore_wire::AdminRequest) -> Result<dstore_wire::AdminReply, dstore_gocli::CliError>;
    pub async fn admin_action(c: &dstore_gocli::Context, req: dstore_wire::AdminRequest) -> Result<(), dstore_gocli::CliError>;
    pub async fn print_status(ctx: &dstore_gocompat::ctx::Ctx, cl: &dstore_client::Cluster, out: &mut (dyn std::io::Write + Send)) -> Result<(), dstore_gocli::CliError>;
    pub fn record_payload(rec: &[u8]) -> Result<Vec<u8>, amber_store_core::amberpack::Error>;
    pub fn open_local(c: &dstore_gocli::Context) -> Result<(std::sync::Arc<amber_store_core::packstore::Store>, amber_store_core::refstore::Store), dstore_gocli::CliError>;  // §2.3
    pub fn hex_decode(s: &[u8]) -> Result<Vec<u8>, String>;                                  // "bad hex %q"; odd nibble dropped
}
pub mod progress {
    pub struct Latest { .. }
    impl Latest {
        pub fn new() -> std::sync::Arc<Latest>;
        pub fn get(&self) -> dstore_client::ProgressReport;
        pub fn progress(self: &std::sync::Arc<Self>) -> dstore_client::Progress;
    }
    pub struct RateMeter { .. }
    impl RateMeter { pub fn new(window: std::time::Duration) -> RateMeter; pub fn add(&mut self, t: std::time::Instant, n: i64) -> f64; }
    pub fn status_line(r: &dstore_client::ProgressReport, rate: f64) -> String;
    pub fn fraction(r: &dstore_client::ProgressReport) -> f64;
    pub enum UiMsg { Tick(std::time::Instant), Resize(u16), CtrlC, Event { at: dstore_gocompat::time::GoTime, level: dstore_gocompat::slog::Level, text: String }, Done(Option<String>) }
    pub struct UiModel { .. }
    impl UiModel {
        pub fn new(title: String, latest: std::sync::Arc<Latest>, cancel: dstore_gocompat::ctx::Ctx, zone: std::sync::Arc<dyn dstore_gocompat::time::Zone>) -> UiModel;
        pub fn update(&mut self, msg: UiMsg) -> bool;                                          // true = quit
        pub fn view(&self) -> String;                                                          // byte-identical to Go View() content
    }
    pub struct TeaHandler { .. }                                                               // impl slog::Handler: "msg key=value…", bytes humanised for Int64
    pub fn blend1d(steps: usize, a: [u8; 3], b: [u8; 3]) -> Vec<[u8; 3]>;                     // lipgloss Blend1D / go-colorful BlendLab
    /// plain mode unless a character-device stderr and no --no-tui; runs f with the logger and progress callback.
    pub async fn run_transfer<T: Send + 'static>(
        c: &dstore_gocli::Context, ctx: &dstore_gocompat::ctx::Ctx, title: String,
        f: impl FnOnce(dstore_gocompat::ctx::Ctx, dstore_gocompat::slog::Logger, dstore_client::Progress) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, dstore_gocli::CliError>> + Send>> + Send + 'static,
    ) -> Result<T, dstore_gocli::CliError>;
}
```

### 4.13 `dstore-testkit` (publish = false)

**Ports:**
- Go test helpers (`node/cluster_test.go` `makeTree`/`pushTree`/`waitFor`, `node/watch_test.go` helpers);
- `refglob/refglob.go`;
- node handler semantics for a FakeNode (`node/server.go`, `data.go`, `refs.go`, `watch.go`, `admin.go`, `status.go`);
- core-rs `tests/common/mod.rs`.

**Specs:** verification §4.3-§4.6; client-transfer §5.2 item 9; client-core §4.7.

```rust
pub mod splitmix {
    pub struct SplitMix64(pub u64);
    impl SplitMix64 { pub fn next_u64(&mut self) -> u64; }
    pub fn data(seed: u64, n: usize) -> Vec<u8>;                   // core-rs VECTORS.md stream
    pub fn u64s(seed: u64, n: usize) -> Vec<u64>;
}
pub mod golden {
    pub fn golden_dir() -> std::path::PathBuf;                      // <workspace>/tests/golden
    pub fn load_json<T: serde::de::DeserializeOwned>(rel: &str) -> T;   // panics (test fails) when missing
    pub fn hex(s: &str) -> Vec<u8>;
    #[derive(Debug, Clone, serde::Deserialize)]
    #[serde(untagged)]
    pub enum Payload { Inline { hex: String }, Splitmix { seed: u64, len: usize } }
    impl Payload { pub fn bytes(&self) -> Vec<u8>; }
}
pub mod refglob {
    pub struct Glob { .. }
    pub fn compile(pattern: &str) -> Result<Glob, String>;          // Go refglob texts
    impl Glob { pub fn matches(&self, name: &str) -> bool; pub fn prefix(&self) -> String; }
}
pub mod fake {
    pub struct FakeClusterConfig { pub nodes: usize, pub replicas: u8, pub min_replicas: u8, pub weights: Vec<u32>, pub zones: Vec<String> }
    pub enum Injection {
        Delay(std::time::Duration),
        CloseBeforeReply,
        Err { code: String, text: String, with_view: bool, retry_after_ms: i64 },
        CorruptRecord([u8; 32]),
        DropHints,
    }
    pub struct FakeCluster { .. }
    impl FakeCluster {
        pub async fn start(net: &std::sync::Arc<dstore_transport::mem::Network>, cfg: FakeClusterConfig) -> std::sync::Arc<FakeCluster>;
        pub fn ids(&self) -> Vec<dstore_view::NodeId>;
        pub fn ticket(&self) -> dstore_ticket::Ticket;
        pub fn view(&self) -> dstore_view::View;
        pub fn bump_epoch(&self);
        pub fn set_writable(&self, id: dstore_view::NodeId, writable: bool);
        pub fn inject(&self, id: dstore_view::NodeId, op: i64, nth: usize, inj: Injection);
        pub fn stored(&self, id: dstore_view::NodeId) -> std::collections::BTreeMap<[u8; 32], Vec<u8>>;
        pub fn requests(&self, id: dstore_view::NodeId) -> Vec<dstore_wire::Msg>;   // transcript
        pub fn set_ref_page_limit(&self, n: usize);
        pub fn set_watch_reconcile(&self, d: std::time::Duration);
        pub async fn close(&self);
    }
}
```

### 4.14 Root package `dstore-client-rs`

```rust
// src/lib.rs, [lib] name = "dstore"
pub use amber_store_core as core;
pub use dstore_client as client;
pub use dstore_codec as codec;
pub use dstore_gocompat as gocompat;
pub use dstore_ticket as ticket;
pub use dstore_transport as transport;
pub use dstore_transport_iroh as transport_iroh;
pub use dstore_udiff as udiff;
pub use dstore_view as view;
pub use dstore_wire as wire;
pub use dstore_worktree as worktree;

// src/bin/dstore.rs
fn main() { std::process::exit(dstore_cli::main_entry()) }
```

Integration tests (§7): `tests/golden.rs`, `tests/cli_snapshots.rs`, `tests/fake_cluster.rs`,
`tests/iroh_loopback.rs`. Examples: `examples/holdlock.rs` (opens a `.dstore/packstore` and sleeps).

---

## 5. Cross-cutting decisions

### 5.1 Async runtime and blocking I/O

- **Runtime.** tokio multi-thread everywhere. `dstore_cli::main_entry` builds it. Library users
  must call `Cluster` from a multi-thread runtime.
- **`spawn_blocking` (holding `Arc<packstore::Store>`)** for:
  - `ingest::dir`, and draining `ingest::objects` (`hash_file`);
  - `fstree::reachable_keys`, `check_complete`, `packstore::Store::write_parallel`;
  - the working-copy scan, `diff_trees`, `apply`, `status`, `unified`, `stat`;
  - fstree walks over the cluster (`ls`, `cat`): the getter calls
    `tokio::runtime::Handle::block_on(cluster_get(k))`, which is allowed on a blocking thread, so
    core-rs error texts are kept.
- **Inline, as Go does:** single-object local reads (`has`, `get`, `get_record`, `stored_size`),
  `parse_record`/`decode_payload` of one record, placement math.
- **Locks.** `std::sync` locks are never held across `.await`. The per-peer dial lock is a
  `tokio::sync::Mutex<()>`.
- **`wire::read_msg` is not cancel-safe.**
  - `Pool::call` wraps it in `ctx.run(...)` and abandons the stream when ctx ends (`cancel_read(0)`
    + `finish`).
  - The watch stream reads frames in a spawned task feeding `mpsc::channel(1)`; abandoning aborts
    that task, which drops `RecvStream` (STOP_SENDING 0).
- **Go concurrency mapping.**
  - goroutines + `WaitGroup` → `JoinSet`; `chan struct{}` semaphores → `tokio::sync::Semaphore`.
  - buffered channels → `tokio::sync::mpsc` with Go's capacities; the fetcher's unbuffered MPMC job
    channel → `async_channel::bounded(1)`.
  - `select` → unbiased `tokio::select!`; `select { … default: }` drains → `try_recv` loops.

### 5.2 Errors

- **Display is Go's text.**
  - A wrapped error is `#[error("prefix: {0}")] X(#[source] Inner)`: the text includes the inner text
    and `source()` returns it.
  - Never use `#[error(transparent)]` for errors that `as_remote`, `is_code` or the CAS checks must see.
  - Nothing prints `source()` chains.
- **`errors.As`/`errors.Is`** become `dstore_wire::as_remote` / `is_code`, which walk `self` and the
  `source()` chain with `downcast_ref`. Sentinels are enum variants; each crate provides predicates
  (`Error::is_unknown_ref`, `is_conflict`, `cas_mismatch`, …).
- **Remote errors.**
  - A node's answer is `dstore_wire::RemoteError { code, text, view, retry_after }`; match codes with
    `err.is_code(dstore_wire::CODE_STALE_VIEW)`.
  - A `TErr` read during a pack is `ProtocolRemoteError` and never matches `is_code`, as in Go.
- **Cloning.** Errors kept in maps or shared between tasks are `Arc<dstore_client::Error>`.
- **Top-level rendering.** `dstore-cli` prints
  `"dstore: " + gocompat::errno::rewrite_os_errors(&err.to_string()) + "\n"` to stderr and exits 1.
  `CliError::Exit { msg, code }` prints `msg + "\n"` and exits with `code` (3 for "No help topic").
- **core-rs texts** are fixed where they are produced (core-rs-gaps G3, G4, G6, G16):
  - wrap `amberignore::Matcher::root` failures as `open <dir>/.amberignore: <errno>`;
  - map a `packstore` directory open failure to `open <dir>: <errno>`;
  - re-render fstree names with `gocompat::quote` via
    `dstore_client::corefmt::walk_error_text(&WalkError<E>) -> String`;
  - render xattr decode errors with a `cborx:` prefix via `corefmt::cbor_error_text(&cbor::Error) -> String`;
  - use `gocompat::hex` for every `encoding/hex` decode.
- **Go runtime panics** (`cat NAME /`, `cluster status` with a short cluster id) call
  `dstore_cli::go_panic_exit` (DD-7). Rust panics are bugs and are never used for parity.

### 5.3 Logging

- Every crate logs through `dstore_gocompat::slog::Logger`, with the attribute kinds and order of
  client-core §2.13: `node` String (ShortID); counts Int64; `bytes` Int64; `took`, `rtt`, `wait`,
  `in` Duration; `err`/`error` Any.
- **CLI.** Plain mode uses `TextHandler(stderr, log_level(--log-level), SystemZone)`; TUI mode uses
  `TeaHandler`. `logger(c)` builds a new handler per call, as Go does.
- **Library.** `Config.logger = None` → `Logger::default_logger()`, which reproduces Go's
  `slog.Default()` log-package format.
- **No tracing subscriber** is installed, so iroh internals print nothing (go-iroh prints nothing
  through dstore's slog either).

### 5.4 Cancellation and signals

- **Ctx.** Every Go `context.Context` parameter becomes `&Ctx`. Errors read `context canceled` /
  `context deadline exceeded`. Timeouts nest (a child never outlives its parent).
- **Signals.** Actions that call `signalCtx` in Go call `common::signal_ctx()`: tokio unix signal
  streams for SIGINT and SIGTERM cancel the ctx and stay registered until exit, so later signals are
  swallowed. `status`, `diff`, and every path that fails validation before Go creates `signalCtx`
  register nothing and die by the signal.
- **TUI.** With stdin a character device, raw mode delivers Ctrl+C as a key: `UiMsg::CtrlC` cancels
  once and adds the WARN event `cancelling`.
- **Watch.** `watch` returns `Ok` on cancellation (exit 0). A cancelled transfer returns its error
  (exit 1).
- Blocking worktree steps (apply, scan) are not abortable, as in Go.

### 5.5 Timing constants (normative)

| Constant | Value | Source |
|---|---|---|
| Dial per bootstrap member | 15 s | client.go:98 |
| `RequestTimeout` default; put/get streams | 2 min; 10 × RequestTimeout | client.go:75-77; objects.go:245, fetch.go:276 |
| Probe of hinted-unreachable nodes | 3 s each, concurrent | client.go:257 |
| Node backoff | `5s << min(failures-1, 4)`, cap 60 s; remote errors never back off | client.go:201-218 |
| Stale-view retries | `callRetry` 4 calls; `anyNode` 5 calls per node; watch 4 attempts per node; `putBatch` 4 attempts | client.go:284-362, watch.go:50-91, objects.go:216-242 |
| Busy wait | `retry_after` if > 0, else 1 s (not ctx-aware) | objects.go:229-236 |
| Push rounds / ref-put attempts | 3 / 3 | tree.go:51,131 |
| Re-pin | between rounds when > `gc_interval/2` since the last pin (CLI passes 4 h) | tree.go:116 |
| Watch idle; served pause; reconnect backoff | 2 min; 200 ms; refresh (15 s), then `delay + rand[0, delay/2]`, 1 s doubling to 30 s | watch.go |
| Pool failed-dial window | 2 s, only while no live connection | transport.go:113-116 |
| Direct phase; connect phase cap; relay online wait at bind; mDNS lookup | 2 s; 10 s (DD-4); 10 s; 3 s | iroh.go |
| QUIC | keepalive 5 s, idle 60 s, 1024 incoming bidi streams | iroh.go:100-104 |
| Endpoint close | 5 s (library `close`), 3 s at CLI exit | iroh.go:401-406, DD-11 |
| `cluster status` per-node status | 5 s | cmd client.go:151 |
| Plain progress line; TUI tick | every 5 s (first after 5 s); 100 ms | tui.go |

Tests use `tokio::time::pause()` for backoff, idle and delay behaviour.

### 5.6 Configuration defaults

- **`dstore_client::Config`.** Zero values become Go's defaults: conns 4, jobs 8, gc_interval 4 h,
  request_timeout 2 min, batch_bytes 16 MiB capped at 64 MiB, watch_idle 2 min.
- **The CLI** sets only `endpoint`, `ticket`, `logger` and `gc_interval = 4h`. `--jobs` feeds ingest
  and scan only; a negative `--jobs` maps to 0 (= cores), as Go's `<= 0` does.
- **Client endpoint.** `IrohConfig { secret_key: generate_secret_key(), alpns: [],
  relay: relay_mode_of(--relay, --no-relay)?, discover: !--no-discovery, announce: false, .. }`,
  with `direct_timeout` 2 s.
- **Version.** `dstore version <VERSION>` where `VERSION = option_env!("DSTORE_VERSION")` or `dev`.

### 5.7 CBOR codec approach

Hand-rolled, two passes (§4.2). Not ciborium or minicbor: both differ in limits, laxness and error
texts. Rules:
- emit present fields in ascending key order;
- keep nil vs empty through `Option`;
- use the shortest float form for `AdminRequest.garbage`;
- decode as leniently as fxamacker `DecOptions{}`, with verbatim error texts;
- cap arrays and maps at 131072 before allocating.

Put batches stay at 8192 keys and the ref-watch known list is capped by the node at 131072 entries
(client-side behaviour unchanged).

### 5.8 Go strings and bytes

- **argv** is `OsString`; Go-string arguments reach library code as bytes.
- **Filesystem paths**, working-copy `Config` fields and `Change.path` are bytes.
- **CBOR text fields** are `String` (DD-8).
- **Stdout** carries raw bytes where Go prints raw strings: `ls` entry names, `status`/`diff` paths,
  `cat` content, `refs`/`watch` names.

### 5.9 Process exit and stdio

- **SIGPIPE.** `main_entry` sets `SIGPIPE` to `SIG_DFL` before anything else. A write to a closed
  stdout or stderr then kills the process by signal, as Go does for fds 1 and 2. Rust's default
  (ignore, then panic in `println!`) would differ. Sockets are unaffected (MSG_NOSIGNAL/SO_NOSIGPIPE).
- **Stdout flushing.** Output is written through a locked stdout and flushed after every line for
  streaming commands (`watch`, `refs`, `status`, `diff` per change, progress lines on stderr), and
  before exit. `cat` streams through `fstree::write_content` and flushes at the end or on error.
- **Exit.** Flush, then `std::process::exit(code)` without dropping the runtime. Background tasks
  (async view refreshes, discovery) are abandoned, as Go's exit abandons goroutines.
- **Prompt.** `cluster replicas` reads stdin unbuffered, byte by byte up to `\n` or EOF, and takes
  the first whitespace-separated token (Go `fmt.Scanln`).

### 5.10 Dependency versions

All in the offline registry `/Users/dragan/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f`.
Declare them in `[workspace.dependencies]`.

| Crate | Version | Features / notes |
|---|---|---|
| `amber-store-core` | git rev `a85ffa1eb5ed363b9072ab224de179196cd0a046` | §5.11 |
| `iroh` | `=1.2.0` | `default-features = false`, `features = ["tls-ring", "fast-apple-datapath"]` |
| `iroh-base` | `=1.2.0` | ticket curve check |
| `tokio` | `1.53.1` | `rt-multi-thread`, `macros`, `sync`, `time`, `io-util`, `net`, `signal`, `fs` (dev: `test-util`) |
| `tokio-util` | `0.7.19` | `CancellationToken` |
| `futures` / `futures-core` | `0.3.34` | |
| `async-trait` | `0.1.92` | transport traits |
| `async-stream` | `0.3.6` | `watch_refs` |
| `async-channel` | `2.5.0` | fetcher job channel |
| `pin-project-lite` | `0.2.17` | `GetStream`, pack reader |
| `thiserror` | `2.0.20` | = core-rs lock |
| `blake3` | `1.8.6` | = core-rs lock; placement salt |
| `data-encoding` | `2.11.1` | base32 encode only, mDNS label |
| `rand` | `0.9.2` | watch jitter |
| `socket2` | `0.6.5` | `all`; mDNS sockets |
| `nix` | `0.30.1` | `net`; getifaddrs, if_nametoindex |
| `libc` | `0.2.189` | = core-rs lock |
| `xattr` | `1.6.1` | = core-rs lock |
| `url` | `2.5.8` | relay URL conversion for dialing |
| `crossterm` | `0.29.0` | TUI renderer, raw mode |
| `hex` | `0.4.3` | testkit only |
| `serde` / `serde_json` | `1.0.229` / `1.0.151` | testkit and tests only, never for working-copy files |
| `tempfile`, `walkdir`, `rustix`, `similar` | `3.27.0`, `2.5.0`, `1.1.4` (`fs`), `2.7.0` | dev-dependencies; `similar` only for assertion diffs |

**Not used:**
- `clap`, `ratatui` (CLI and TUI parity);
- `ciborium`, `minicbor` (§5.7);
- `iroh-mdns-address-lookup` (cannot read go-iroh announcements);
- `chrono`, `jiff` (libc local time; FixedZone in tests);
- `tracing-subscriber`, `similar`/`imara-diff` for output, `tempfile` in non-test code (Go temp names).

**Lockfile.** Seed `Cargo.lock` with `cargo generate-lockfile --offline`. If resolution needs
uncached versions, pin the iroh subtree to the known-good lock of
`/Users/dragan/fables-for-robots/secret-bunker-iroh/Cargo.lock` for the non-iroh crates (tokio 1.53.1,
rustls 0.23.43, ring 0.17.14). Commit `Cargo.lock`.

### 5.11 core-rs pin

- **Manifest entry:**
  `amber-store-core = { git = "https://github.com/amber-store/core-rs", rev = "a85ffa1eb5ed363b9072ab224de179196cd0a046" }`
  (public repo; tag v0.3.0 is the same commit).
- **Nix `outputHashes."amber-store-core-0.3.0"`** = `sha256-ZnXXnztVqGXq5ILG29kkJIIRo9/TTW3XqULx1chbLXA=`
  (computed from `git archive`; confirm on the first `nix build`, bump with every rev).
- **Effective MSRV** is 1.88 for core-rs and 1.91 for iroh; the workspace declares
  `rust-version = "1.91"`.
- **Upstream proposals, none required:**
  - public `PACK_MAGIC`/`TAG_END` and an indefinite-length `read_head`;
  - public xattr, device-number and mtime helpers;
  - segment files created 0644 and directories 0755;
  - a non-panicking `Key::type_`;
  - an async-friendly amberpack reader.

### 5.12 Rust iroh

- **Version.** Pin 1.2.0, the latest release (user decision). It replaces 1.0.3, the column go-iroh v0.2.0 verifies; the interop suite is the gate. Earlier text: moving between 1.x releases is a Cargo-only change,
  gated on the interop suite passing.
- **Builder.** `Endpoint::builder(presets::Minimal)` with `.secret_key`, `.alpns`,
  `.transport_config(...)`, `.portmapper_config(PortmapperConfig::Disabled)`, `.relay_mode(...)`.
  No `.address_lookup(...)`. Never use `presets::N0`, `RelayMode::Default`, `DnsAddressLookup::n0_dns`
  or `PkarrResolver::n0_dns`, which honour `IROH_FORCE_STAGING_RELAYS` and use other relay hosts.
- **Relay map.** Default: `RelayMap::from_iter(GO_DEFAULT_RELAYS)` (QUIC port 7842 through
  `RelayConfig::from(RelayUrl)`). `--relay`: that URL, or `RelayMap::empty()` when `url::Url` rejects
  it (DD-13).
- **Discovery.**
  - With `discover`, the go-iroh mDNS resolver port starts at bind.
  - With `discover` and relays enabled, `DnsAddressLookup::builder("dns.iroh.link.")` also uses the
    endpoint's resolver.
  - Both run only inside `discover_dial`; the first answer carrying the right id and a non-empty
    address set wins.
- **Paths and RTT.** `Conn::path` takes the selected path: `direct = !is_relay()`, and `rtt` is
  ZERO when it equals `NOQ_INITIAL_RTT`. `initial_rtt` is set explicitly so the sentinel cannot drift.
- **Streams.** `SendStream::finish()` = FIN, `RecvStream::stop(0)` = STOP_SENDING. Dropping an
  unfinished stream finishes it or stops it; call these explicitly anyway.
- **Shutdown.** The CLI awaits `Endpoint::close()` bounded to 3 s after `Cluster::close()` (DD-11).

### 5.13 Platform

- Unix only: macOS and Linux, on x86_64 and aarch64.
- `gocompat::errno` carries Go's `syscall` tables per `cfg(target_os)`.
- CI runs on `ubuntu-latest` and `macos-latest`.
- Windows is out of scope; dstore and core-rs are Unix-only.

---

## 6. Implementation order

Each layer starts when the layers it depends on have merged. Modules within a layer proceed in
parallel. Everything codes against the L0 stubs.

| Layer | Modules (one owner each) | Parallel | Depends on |
|---|---|---|---|
| L0 scaffold | workspace manifest with `[workspace.dependencies]`; every crate with the full §4 API stubbed (`todo!()`); `cbor_struct!` macro expanding to the struct plus trait calls; root facade and bin; flake.nix, flake.lock (copied from core-rs), .envrc, .gitignore; CI skeleton; `tools/vectorgen` go.mod skeleton; README and VECTORS.md stubs | no (1 agent) | — |
| L1 foundations | gocompat-a: `quote`, `strings`, `tables.rs` + `tools/vectorgen/cmd/gotables`; gocompat-b: `strconv`, `time`, `fmt`; gocompat-c: `json`, `hex`, `base32`, `errno`, `path`, `os`; gocompat-d: `ctx`, `slog`; codec; udiff (+`gosort`); vectorgen-proto (wire, decode, pack, ticket, view, placement, admin, status, text families); vectorgen-worktree (config, state, trees, diff, merge, udiff families); vectorgen-cli (`cmd/clisnap`, cases, size/format unit vectors) | yes (9) | L0 |
| L2 protocol and offline | wire (msg, frame, error, keys, admin); wire-pack; ticket; view (placement + view + `ticket_from_view`); gocli; worktree-offline (tree, change, scan, merge, apply, diff, sys) | yes (6) | codec, gocompat; worktree also needs udiff |
| L3 transport and harness | transport (traits, Pool, addr); transport-mem; testkit (splitmix, golden, refglob, FakeNode); cli-progress (Latest, RateMeter, status_line, UiModel, renderer, TeaHandler, colours); cli-app (command table, help and usage-error snapshot tests; actions stubbed) | yes (5) | wire, view, gocli |
| L4 network and client | transport-iroh (bind, dial phases, conn, path, discovery wiring); transport-iroh-mdns (packet builder/parser, resolver); client-a (cluster, rank, batch, progress, refs, watch, error, corefmt); client-b (objects, fetch, tree) | yes (4) | transport; client-b codes against client-a seams |
| L5 flows and commands | worktree-flow (fetch, clone, init, pull, push, refresh_ticket); cli-client (store push/pull, refs, watch, ref get/delete, ls, cat, open_local, cluster_get); cli-admin (cluster, token, node, voter, transition, gc, catalog, print_status, nodeside); cli-wc (clone, init, fetch, pull, push, status, diff, wc_config, push_user, describe_change, filter_paths) | yes (4) | client, worktree-offline, transport-iroh, cli-app |
| L6 integration | `tests/fake_cluster.rs` ports; `tests/iroh_loopback.rs`; `interop/check.sh` + helpers (`treekey`, `mktree`, `storecmp`, `holdlock`, `examples/holdlock.rs`); CI jobs green; `nix flake check`; README (compatibility contract, DD list, node-side note) | yes (4) | L5 |

Notes:
- `ticket_from_view` makes `view` depend on `ticket`. Implement ticket first within L2, or stub it.
- `gotables` output (unicode tables) is needed by `quote`/`strings` tests. gocompat-a owns both.
- Golden-vector families land with, or before, the Rust tests that read them.

---

## 7. Golden vectors and tests

**Generator.** One Go module, `tools/vectorgen`:
- **Requires:** dstore v0.1.9, core v0.0.8, go-udiff v0.4.1, fxamacker/cbor v2.9.3, go-iroh v0.2.0,
  zeebo/blake3 v0.2.4; built with go1.26.5, `GOTOOLCHAIN=local`, `CGO_ENABLED=0`.
- **Output.** `vectorgen <out-dir> [family…]` deletes exactly what it owns and regenerates. JSON is
  built from structs only, `u64`/`i64` are decimal strings, bytes are lowercase hex, payloads use
  splitmix64 as in core-rs VECTORS.md.
- **Package `main` functions** of `cmd/dstore` are verbatim copies checked by an AST self-check.
- **go.sum** is committed from an online `go mod tidy`.

**Families** (`tests/golden/<family>/`, schemas in VECTORS.md), the union of the specs:

| Family | Content | Spec |
|---|---|---|
| `wire/frames.json` | every client-ALPN request and reply of the pinned probes | codec §3.2-3.3, client-core §3.2-3.3, client-transfer §3.1, verification §5 |
| `wire/decode.json` | field decode matrix and structure cases; **`go_error` is asserted** | codec G4-G5, verification §2.1 |
| `wire/frame_errors.json`, `wire/pack_frames.json`, `wire/pack_reader.json` | ReadMsg edge cases; SendPackRecords chunking; packReader negatives | codec G2-G3, G6; client-transfer §5.2 item 2 |
| `ticket/encode.json`, `ticket/parse.json`, `ticket/base32.json`, `ticket/curve.json` | encode and IDs; Parse texts; Go base32 decoder; curve-point acceptance | codec G7-G8, G11 |
| `gocompat/tables` (Rust source) + `text/quote.json`, `text/case.json` | isprint, White_Space, simple case mapping; `strconv.Quote` | codec G9-G10, view §3.5 |
| `view/view_placement.json` | Appendix A generator output: fmix64, log2fix, slot, salt, L, sets, views, view_cbor, view_decode, parse_node_id, short_id, ticket_from_view, helpers, compare, validate_change, sort_nodes | view-placement §5, Appendix A |
| `placement/all_slots.json` | `all_slots_blake3` per set | verification §4.3 item 9 |
| `client/rank.json`, `client/batches.json`, `client/fetch.json`, `client/verify_record.json`, `client/progress.json`, `client/backoff.json` | rank scenarios; batches; est_size and pick_batch; VerifyRecord outcomes; tracker reports; backoff sequence | client-core §5, client-transfer §5.2 items 3-5 |
| `client/placement_decisions.json`, `client/transcripts/*.json` | Owners/WriteSet/ReadOrder/Placed through Dial; scripted fake-node conversations | client-transfer §5.2 items 8-9 |
| `admin/requests.json`, `admin/replies.json`, `status/status.json` | AdminRequest per CLI subcommand; AdminReply and printed output; Status variants | codec §2.5, cli §3.8, verification §5 |
| `transport/addrs.json`, `transport/relay_urls.json`, `transport/ids.json`, `transport/mdns.json`, `transport/pool_scripts.json` | ParseTransportAddr/ParseAddrs; ParseRelayURL; endpoint-id validation and identity strings; mDNS query, announcement and parse cases; Pool behaviour scripts | transport §5 |
| `text/formats.json` | HumanBytes, Rate, Duration String/Round, RFC3339 local (**each case carries `offset_secs`**; tests use `FixedZone`), RFC3339Nano, slog lines, `status_line`, `describe_change`, `type_name` | client-core §3.5-3.7, cli §5.4, verification §5 |
| `worktree/config.json`, `worktree/state.json`, `worktree/trees/`, `worktree/diff_trees.json`, `worktree/merge.json`, `worktree/unified.json`, `udiff/udiff.json`, `udiff/pdqsort.json`, `udiff/lcs.json` | working-copy files; tree fixtures; DiffTrees/Scan; merge table; Unified/Stat; go-udiff random cases (≥ 3000); pdqsort permutations; DiffLines | worktree §5 items 1-16 |
| `cli/size.json`, `cli/snapshots.json` | parseSize and pack_size; stdout, stderr and exit of every CLI case | cli §5.2-5.4, verification §4.3 item 24 |
| `errors/text.json`, `refglob/refglob.json` | client and worktree error texts; refglob for the fake node | verification §4.3 items 22-23 |

**Placement of tests:**
- vectors of crate-private functions (`batches`, `rank_owners`, `est_size`, `gosort`) → crate unit
  tests via `dstore_testkit::golden`;
- public APIs → root `tests/golden.rs`;
- CLI → `tests/cli_snapshots.rs`, with a clean env (`PATH`, temp `HOME`, `TZ=UTC`) and `{CWD}`
  normalised with `pwd -P`.

**Rules:**
- a missing vector file fails the test;
- no sockets outside `tests/iroh_loopback.rs`;
- `tokio::time::pause()` for timing tests;
- never assert an order that Go randomises (DD-10).

**Go tests to port, by crate:**
- codec/wire/ticket: codec-wire-ticket §6.
- view: `placement_test.go` (7 tests).
- client: `batch_test.go` (4), `rank_test.go` (7), `wire_test.go` `TestErrorFrames`.
- fake cluster: `node/cluster_test.go` (7), `watch_test.go` (4), `worktree_test.go` (3).
- transport: `iroh_test.go` (4), the go-iroh mdns/netaddr tests (transport §6).
- worktree/udiff: `worktree/*_test.go`, go-udiff tests.
- cli: `size_test.go`, `wc_test.go`, `tui_test.go` (`TestRateMeter`, `TestStatusLine`, `TestUIModel`
  against `UiModel::view`, `TestTeaHandler`), plus the urfave tests listed in cli §6.

**Live interop** (`interop/check.sh`, verification §4.5): a 3-node Go v0.1.9 cluster on loopback,
built with `CGO_ENABLED=0` into `mktemp -d` and removed on exit, using `store push/pull`, not the
stale script lines. It runs checks A1-A13, B1-B14, C1-C3, D1-D13, E1-E3, and G1-G2 (G1 asserts the
§2.3 refusal, G2 the §2.2 texts), with exact, stdout+exit, normalized, format and root comparison
modes, a TZ matrix (UTC, Asia/Kolkata, America/St_Johns), and `INTEROP_MDNS=auto`.

---

## 8. Nix flake and CI

**Flake** (verification §4.8, adapted to the workspace):
- **Inputs.** `nixpkgs` `nixos-26.05` and `systems`; `flake.lock` copied from core-rs (rev
  `445d861c6d31b4af0c79d8d4be2331f762a361d7`).
- **`packages.dstore`.**
  - `rustPlatform.buildRustPackage` over `lib.fileset.unions [ ./Cargo.toml ./Cargo.lock ./src ./crates ./tests ./examples ]`;
  - `cargoLock.outputHashes."amber-store-core-0.3.0"` per §5.11;
  - `cargoBuildFlags = [ "-p" "dstore-client-rs" "--bin" "dstore" ]`, `doCheck = false`
    (the Darwin sandbox refuses UDP binds).
- **`checks`:**
  - `dstore`;
  - `fmt`: `rustfmt --check --edition 2024` over `src crates tests examples`;
  - `clippy`: `cargo clippy --workspace --all-targets --offline -- -D warnings`;
  - `tests`: `cargoTestFlags = [ "--workspace" "--lib" ]`, plus root `--test golden --test cli_snapshots --test fake_cluster`, `TZ = "UTC"`.
- **`devShells.default`.** cargo, rustc, rustfmt, clippy, rust-analyzer, go, gopls;
  `hardeningDisable = [ "all" ]`; `GOTOOLCHAIN = "local"`; `CGO_ENABLED = "0"`;
  `RUST_SRC_PATH` = `rustPlatform.rustLibSrc`. The C compiler for zstd-sys, ring and blake3 comes from
  stdenv.
- **`formatter`:** `cargo fmt --all`. `.envrc`: `use flake`.

**CI** (`.github/workflows/ci.yml`, verification §4.9):

| Job | Runs on | Steps |
|---|---|---|
| `rust` | ubuntu-latest, macos-latest | toolchain 1.95.0; `cargo fmt --all --check`; `cargo clippy --workspace --all-targets --locked -- -D warnings`; `cargo test --workspace --locked` (includes iroh loopback), `TZ=UTC` |
| `vectors` | ubuntu-latest | `go vet ./...`; regenerate twice and diff; diff against `tests/golden`; regenerate `crates/gocompat/src/tables.rs` and diff; `clisnap` and diff |
| `interop` | ubuntu-latest, 45 min timeout | check out dstore at `v0.1.9`; `cargo build --release --locked --bin dstore --examples`; `bash interop/check.sh`; upload logs on failure; heavy and chaos groups on `workflow_dispatch` |
| `nix` | ubuntu-latest, macos-latest | `nix flake check -L`; `nix build .#dstore -L && ./result/bin/dstore --version` |

---

## 9. Conflicts between specs, resolved

| # | Topic | Decision | Overrides |
|---|---|---|---|
| C1 | mDNS | Port go-iroh's resolver, which parses records in any section. `swarm-discovery-0.6.3/src/receiver.rs:77-160` reads A/AAAA only from additionals, while go-iroh sends them as answers. | client-core §4.1/§4.5, cli §4.8, verification §4.7 (`iroh-mdns-address-lookup`) |
| C2 | Default relays | go-iroh canary hosts (`relay/relay.go:23-30`, despite its comment saying "production"). | cli §2.5 ("number0 production map") and §4.8 (`RelayMode::Default`) |
| C3 | Discovery wiring | Explicit, only in `discover_dial`; no endpoint address lookup, no `n0_dns()`. | client-core §4.5 |
| C4 | 52-char base32 ids with trailing bits | Accepted: go-iroh decodes with Go `encoding/base32`, which ignores trailing bits (`key/key.go:505-524`, `key_core.go`). The verification probe flipped a data bit. | verification §2.2 |
| C5 | Base32 decoding | Go decode loop in `gocompat::base32`. | verification §7 #11 (data-encoding `Specification`) |
| C6 | CBOR | Hand-rolled codec. | verification §7 #6 (ciborium `Value`) |
| C7 | Decode error texts | Asserted verbatim for every generated vector. | verification §2.1 ("informational"), view-placement open decision 1 |
| C8 | CLI framework | urfave-compatible `dstore-gocli`. | worktree §4.1 ("clap 4.6") |
| C9 | TUI | Hand-rolled crossterm inline renderer over a pure `UiModel`. | verification §6 and §8 open decision 11 (ratatui) |
| C10 | "RTT not measured" | `rtt == NOQ_INITIAL_RTT`, with `initial_rtt` set explicitly. | client-transfer D2 alternative (`frame_rx.acks == 0`) |
| C11 | Stale-view attempts | `callRetry` 4, `anyNode` 5 per node (`client.go:284-362`), watch 4 per node. | verification §2.6 ("4 attempts (callRetry, anyNode)") |
| C12 | Local time zone | libc `localtime_r` `SystemZone`; tests use `FixedZone` from vector offsets. | client-core §4.1 (chrono), verification §4.4 (jiff `tzdb-bundle-always`) |
| C13 | `Ctx` API | §4.1 (`with_cancel` returns `Ctx`; `cancel()` method). | client-core `(Ctx, CancelHandle)`, client-transfer `child()` |
| C14 | Transport traits | Boxed `SendStream`/`RecvStream` traits + `async-trait` (§4.6). | transport §4.3 (enum halves), client-core §4.3 (`BoxFuture` traits) |
| C15 | `Pool::new` `per_peer` | `usize`, 0 → 1. | transport §4.4 (`i64`) |
| C16 | `stamp`/`call` | `&mut Msg` (retries re-stamp in place). | client-transfer §4.1 (by value) |
| C17 | Progress | `Arc<dyn Fn(&ProgressReport)>`; counts `i64`. | client-core (by value), client-transfer (`usize` counts) |
| C18 | Client error name | `dstore_client::Error`. | client-transfer `ClientError` |
| C19 | Input-echoing parsers | `ticket::parse(&[u8])`, `view::parse_node_id(&[u8])`. | codec §4.4 (`&str`), view-placement §4.3 (`&str` variant) |
| C20 | Address strings | Hand-written Go parser in `dstore_transport::addr`. | view-placement §4.4 (`view::addrs` over `iroh_base::TransportAddr`) |
| C21 | Admin/status types | `dstore_wire::admin`. | cli §4.1 (`src/bin/dstore/admin_types.rs`) |
| C22 | `NodeId` | Newtype in `dstore-view`. | transport and client-transfer (`type NodeId = [u8; 32]`) |
| C23 | Fake node | `dstore-testkit` crate. | verification §4.1 (`src/testing` behind `test-support`) |
| C24 | Go runtime panics | Emulated (first line, exit 2). | cli §8.2 item 3 and core-rs-gaps D4 (exit 1); agrees with view-placement open decision 2 |
| C25 | Pebble refs | Refuse at open for push and pull. | core-rs-gaps D1 ("skip on push") |
| C26 | Undialable `--relay` | Accepted (DD-13). | cli §7 ("fail early") |
| C27 | Node-side messages | The exact texts of §2.2. | cli §8.2 item 2 and verification §8 suggestions |
| C28 | Layout | Workspace of §3. | verification §4.1 (single package with `src/testing`) and the single-crate module paths in every area spec (`src/codec`, `src/client`, …); module contents are unchanged |
| C29 | Stale e2e lines | Lines 42 and 46 at HEAD. | cli §1.2 ("52,56") |
| C30 | mem transport quirk | Faithful: blocked reads survive connection close. | transport open decision 11 |

---

## 10. Open decisions for the user

1. **Node-side commands.** v1 keeps `serve`, `cluster init`, `node join`, the restore step of
   `catalog restore`, and `--store` ticket derivation as identical definitions that fail with the
   fixed messages of §2.2. Should a later phase port the node (Pebble-compatible meta store, paxos
   acceptor, full node), or delegate these commands to a Go `dstore` binary on `PATH`?
2. **Local refs for `store push/pull --local`.** v1 stores refs in redb and refuses Pebble
   directories (DD-2). Is sharing `--local` directories with Go dstore required, i.e. a
   Pebble-compatible refs writer?
3. **License.** dstore has no LICENSE file. The port links core-rs (LGPL-3.0-only) and copies LGPL
   helpers from core-rs examples. Recommended: LGPL-3.0-only for dstore-client-rs.

---

## 11. Completeness check

- **Go symbols.** A script at synthesis time enumerated the exported Go symbols (376) of `client`,
  `codec`, `wire`, `ticket`, `view`, `placement`, `transport`, `worktree`, `node/admin.go`,
  `node/status.go` and transport-iroh `protocol`, and checked each against the port-notes. The only
  names no spec mentioned were 14 wire error-code constant names (`CodeBusy`, `CodeNoSpace`, …),
  whose string values were covered; all 21 are now in §4.3. Every other symbol is covered by a spec
  and by the §4 API.
- **CLI surface.** All urfave `Name:` literals of `cmd/dstore` (78 command and flag names) and all 7
  env vars (`AMBER_STORE`, `DSTORE_LOG_LEVEL`, `DSTORE_NO_DISCOVERY`, `DSTORE_NO_TUI`,
  `DSTORE_PACK_SIZE`, `DSTORE_STORE`, `DSTORE_TICKET`) appear in cli.md. §2.1 lists every command.
- **Addenda.** Behaviours no spec covered, and the decisions above that change a spec, are appended
  to each spec as `## Addenda (synthesis)`.
