# Verification strategy, Nix flake, CI

Area: how dstore-client-rs proves it is 100% compatible with Go dstore v0.1.9
(github.com/amber-store/dstore, HEAD 368f2c7; the uncommitted formatting-only
change to `cmd/dstore/wc.go` is ignored, and every line number below for
`wc.go` is from `git show HEAD:cmd/dstore/wc.go`). The strategy has five parts:

- (a) a Go golden-vector generator, `tools/vectorgen`, that drives the real Go
  libraries and writes `tests/golden/`;
- (b) a live interop harness, `interop/check.sh`, that runs a 3-node Go cluster
  on loopback and cross-checks the Rust and Go CLIs against it;
- (c) in-process Rust tests over an in-memory transport with a fake node;
- (d) a Nix flake (devShell, package, checks, formatter);
- (e) a GitHub Actions workflow.

Everything marked "probe" was run against the Go libraries in a scratch
module and the output is quoted verbatim. The scratch directory, its module
cache and every binary built there were deleted afterwards.

---

## 1. Scope

### 1.1 Go tests surveyed (dstore v0.1.9)

| file | lines | what it covers | port? |
|---|---:|---|---|
| `client/batch_test.go` | 51 | `batches` byte/key splitting | yes, pure |
| `client/rank_test.go` | 110 | `rankOwners` path preference | yes, pure |
| `cmd/dstore/size_test.go` | 68 | `parseSize`, `--pack-size`/`DSTORE_PACK_SIZE` | yes, pure |
| `cmd/dstore/tui_test.go` | 106 | TUI model, `rateMeter`, `teaHandler`, `statusLine`/`fraction` | partly (plain-mode pieces) |
| `cmd/dstore/wc_test.go` | 20 | `resolveTicket` precedence | yes, pure |
| `ticket/ticket_test.go` | 80 | ticket round trip, id-list parsing, `IDs()` | yes + vectors |
| `wire/wire_test.go` | 90 | frame round trip, error frames, pack-frame interop | yes + vectors |
| `placement/placement_test.go` | 234 | log2fix, fmix64, frozen rankings, distribution, zones | yes + vectors |
| `refglob/refglob_test.go` | 92 | glob Match/Prefix/Invalid | only for the fake node |
| `transport/iroh_test.go` | 357 | real iroh loopback, handshake wait, RTT, mDNS by id | yes, adapted to Rust iroh |
| `worktree/tree_test.go` | 88 | Create/Open/SaveState/Find/Remove | yes |
| `worktree/change_test.go` | 171 | `DiffTrees`, pruning, `Compare` | yes + vectors |
| `worktree/scan_test.go` | 156 | `Scan`: kinds, ignore, racy mtime, retype, xattr | yes |
| `worktree/merge_test.go` | 91 | `Merge` rule table | yes + vectors |
| `worktree/diff_test.go` | 103 | `Unified`, `Stat` | yes + vectors |
| `worktree/apply_test.go` | 179 | `Apply`: clone/update, non-empty dirs, unsafe paths, symlinked ancestor | yes |
| `node/cluster_test.go` | 831 | 3-node in-memory cluster: push/pull, GC, removal, node down, progress, pipelining, streaming, get | client-side ones over the fake node; the rest live |
| `node/watch_test.go` | 260 | `WatchRefs` initial diff, hints, lost hint, reconnect, bad pattern | yes, over the fake node |
| `node/worktree_test.go` | 240 | init/push/clone/edit/pull, conflict, lost-state recovery | yes, over the fake node |
| `node/config_test.go`, `node/corrupt_test.go`, `paxos/paxos_test.go` | 19/68/448 | node internals | no |
| `scripts/e2e-loopback.sh` | 58 | 3 real nodes over iroh loopback, push/pull/watch/ids/GC | template for (b) |
| `.github/workflows/test.yml` | 11 | `go vet ./...`, `go test ./... -timeout 900s` on ubuntu-latest | reference |
| `Dockerfile` | 20 | `CGO_ENABLED=0 go build -trimpath -ldflags "-s -w -X main.version=${VERSION}"` | build recipe for (b) |

### 1.2 Go sources read for byte and text formats

`wire/wire.go` (395), `codec/codec.go` (38), `ticket/ticket.go` (96),
`view/view.go` (469), `placement/placement.go` (272), `client/client.go` (403),
`client/objects.go` (395), `client/refs.go` (150), `client/tree.go` (479),
`client/fetch.go` (309), `client/watch.go` (245), `client/progress.go` (181),
`client/rank.go` (71), `client/batch.go` (29), `transport/transport.go` (243),
`transport/mem.go` (365), `transport/iroh.go` (457), `transport/ifaces.go` (55),
`worktree/{tree 245, flow 331, change 238, scan 284, diff 206, merge 80, apply 310, xattr 55}.go`,
`cmd/dstore/{main 698, client 574, wc 501, tui 374, size 46}.go`,
`node/{server 264, data 623, refs 422, watch 310, admin 302, status 124}.go`
(handlers only, to specify the fake node), and
`github.com/amber-store/transport-iroh@v0.4.0/protocol/{protocol.go 180, pack.go 141}`.

### 1.3 Approaches borrowed

- core-rs: `VECTORS.md` (splitmix64 payloads, one JSON file per family,
  deterministic output, generator deletes what it owns), `tools/vectorgen`
  (Go, 1729 lines, `writeJSON` from structs only), `tests/common/mod.rs`
  (splitmix, `Payload`, `golden_dir`), `tests/cli_e2e.rs` (spawns the example
  binary), `interop/check.sh` (builds both CLIs into `mktemp -d`, cross-reads,
  `cmp`), `.github/workflows/ci.yml` (fmt/clippy/build/test on ubuntu+macOS,
  interop on ubuntu with `actions/checkout` of the Go repo at a pinned commit),
  `flake.nix` (devShell only, `hardeningDisable = [ "all" ]`, go for vectorgen).
- `/Users/dragan/fables-for-robots/secret-bunker-iroh/flake.nix`: known-good
  iroh 1.0.3 build with `rustPlatform.buildRustPackage`, no extra system
  inputs, `doCheck = false` because the Darwin sandbox refuses UDP binds.

---

## 2. API used client-side, as the verification surface

This section lists what each verification layer has to pin down, with the
exact semantics, edge cases, validation order and timing constants that
tests must accommodate.

### 2.1 Deterministic CBOR (`codec`), the base of every byte vector

`codec/codec.go:13-23`: the encoder is `cbor.CanonicalEncOptions()`
(fxamacker v2.9.3, `encode.go:631-639`): `Sort: SortCanonical`,
`ShortestFloat: ShortestFloat16`, `NaNConvert: NaNConvert7e00`,
`InfConvert: InfConvertFloat16`, `IndefLength: IndefLengthForbidden`.
`NilContainers` keeps its default, so a nil slice without `omitempty`
encodes as CBOR null `f6` (for example `RefInfo.Version` at key 2 in a
`ref-watch` request built by `client/watch.go:136-139`, and nil `View.Voters`
or `Ticket.Members`). vectorgen must include such cases, because a naive Rust
encoder writes `80` or omits the key. The decoder is `cbor.DecOptions{}`, the
defaults.

Probe results for `wire.ReadMsg` over hand-made payloads. Each case gives the
input payload, then the result, and for accepted input the canonical
re-encoding:

| case | payload hex | Go result |
|---|---|---|
| keys out of order | `a2030700 1835` | ok, canonical `a20018350307` |
| non-shortest int | `a100190035` | ok, `a1001835` |
| indefinite-length map | `bf001835ff` | ok, `a1001835` |
| unknown key 99 | `a2001835186301` | ok, `a1001835` (unknown keys dropped) |
| duplicate key | `a2001835001820` | ok, `a1001835` (first value kept) |
| text where bytes expected (key 4 elem) | `a200182104816361 6263` | ERR `wire: decode frame: cbor: cannot unmarshal UTF-8 text string into Go struct field wire.Msg.4 of type []uint8` |
| bytes where string expected (key 6) | `a2001824064178` | ERR `... cannot unmarshal byte string into Go struct field wire.Msg.6 of type string` |
| trailing byte after the map | `a100183500` | ERR `wire: decode frame: cbor: 1 bytes of extraneous data starting at index 4` |
| negative int into uint64 (key 3) | `a200183503 20` | ERR `... cannot unmarshal negative integer into Go struct field wire.Msg.3 of type uint64` |
| tag 0 around a string (key 11) | `a20018350bc06174` | ok, `a20018350b6174` (tag ignored) |
| float into int (key 0) | `a100f93800` | ERR `... cannot unmarshal primitives into Go struct field wire.Msg.0 of type int` |
| int into bool (key 5) | `a20018350501` | ERR `... cannot unmarshal positive integer into Go struct field wire.Msg.5 of type bool` |
| empty payload | `` | ERR `wire: decode frame: EOF` |
| array at top level | `8100` | ERR `... cannot unmarshal array into Go value of type wire.Msg (cannot decode CBOR array to struct without toarray option)` |

The Rust decoder must accept exactly what Go accepts. Duplicate keys keep the
first value. The error texts are informational: they only reach users when a
peer is broken, so vectors record accept/reject plus the canonical
re-encoding, and not the text.

Frame layer (`wire/wire.go:247-285`): a 4-byte big-endian length, then the
payload. `MaxFrame = 16 << 20` is checked on write
(`wire: frame of %d bytes exceeds limit %d`) and on read before allocating.
A clean EOF before the header gives `io.EOF`. A cut inside the header gives
`io.ErrUnexpectedEOF`. A cut inside the payload gives
`wire: short frame: unexpected EOF`. A decode failure gives
`wire: decode frame: %w`.

### 2.2 Tickets (`ticket/ticket.go`)

- `Encode` (37-39): `"dstore1" + lower(base32 StdEncoding NoPadding (CBOR{0: ClusterID, 1: Incarnation, 2: Members}))`.
  `Member{0: ID, 1: Addrs omitempty}`. `Incarnation` and `Members` have no
  `omitempty`.
- `IDs()` (45-55): the hex of each 32-byte member id, first occurrence only,
  joined with `,`. Ids that are not 32 bytes are skipped.
- `Parse` (60-80) order: `TrimSpace`, then empty gives `ticket: empty`. If
  `ToLower(s)` lacks the `dstore1` prefix, parse as ids. Otherwise decode
  `ToUpper(s[7:])` as base32, with error `ticket: %w`, then CBOR, with error
  `ticket: %w`. Zero members gives `ticket: no members`.
- `parseIDs` (82-96) splits only on `,`, space, `\t`, `\n`, `\r`, and calls
  go-iroh `key.ParseEndpointID(lower(field))`. On error it reports
  `ticket: %q is neither a dstore1 ticket nor a node id: %w`.

Probe facts that a Rust port gets wrong by default:

- Non-canonical base32 trailing bits are **accepted** in the dstore1 form.
  Flipping the low 1 or 2 bits of the last symbol of a 127-symbol ticket
  (79 CBOR bytes) parsed fine and re-encoded to the original.
  `data_encoding::BASE32_NOPAD` has `check_trailing_bits: true`
  (data-encoding-2.11.1 `src/lib.rs:1902`, `:2264`). Rust needs a custom
  `Specification` with `check_trailing_bits = false`, lowercase translated
  to uppercase, and `ignore = "\r\n"`. Go's `encoding/base32` ignores `\r`
  and `\n` inside the input; vectorgen must confirm this with an
  embedded-newline case.
- For the 52-character base32 id form, go-iroh **rejects** non-zero trailing
  bits with a z-base-32 hint. The probe error was
  `ticket: "a4dq…a4dr" is neither a dstore1 ticket nor a node id: data is not a valid public key: input is z-base-32, use ParseEndpointIDZ32`
  (go-iroh `key/key.go:229-238`).
- go-iroh error texts reach stderr verbatim, so the Rust CLI must reproduce
  them. From `key/key.go:34-40`: `data is not a valid public key` (hex that
  is not an ed25519 point, for example 32×0x07), `invalid length`,
  `failed to decode hex string`, `failed to decode base32 string` (this one
  also covers `hex;hex` and `hex<NBSP>hex`).
- A leading or trailing NBSP is trimmed by `TrimSpace`; an inner NBSP is not
  a separator. `DSTORE1…` in upper case and `"  dstore1…\n"` both parse.

### 2.3 View, placement, ranking, batching

- `view.View` (`view/view.go:114-139`) is 24 keys. Non-omitempty keys:
  0-8 and 10. `Node.Writable` (key 7) has no omitempty and encodes `f4`/`f5`.
  `Compare` (177-191) orders by incarnation, then epoch.
  `client.adopt` (`client/client.go:156-167`) replaces the view only when
  it is newer, or at equal (incarnation, epoch) with a higher `Version`.
- `placement` (`placement/placement.go`): `SlotBits = 20`;
  `Slot = BE u64(key[24:32]) >> 44` (37-39);
  `Salt = BE u64(BLAKE3("amber-dstore/placement/1" ‖ id)[0:8])` (43-50);
  `Fmix64` (53-60); `Log2Fix` (64-81, panics on 0); `L` (85-91);
  `less` compares `a.weight·L_b` against `b.weight·L_a` as 128-bit products,
  ties by smaller id (101-111). `Rank` sorts weighted members stably, then
  appends weight-0 members in id order (145-166). `Owners` takes the first r
  members of distinct zones (zone "" means the id bytes) and stops at weight 0
  (171-194). Frozen expectations (`placement_test.go:84-91`) for five members
  with ids 0x01..0x05 repeated and weights 1000/2000/500/4000/1000:
  slot 0 `{3,4,2,1,0}`, 1 `{3,0,4,1,2}`, 12345 `{1,3,4,2,0}`,
  0xfffff `{3,1,4,0,2}`, 524288 `{3,0,1,4,2}`, 777777 `{4,1,2,3,0}`;
  `Salt(0x01…) = 0x91d3f42d715bdc8a` (probe confirmed);
  `L(12345, salt1) = 0x22356754f`.
- `rankOwners` (`client/rank.go:34-71`): a stable sort by penalty ascending,
  then relay (0 direct, 1 relayed), then RTT class, then input position.
  Classes (17-28): `<5ms`→0, `<25ms`→1, `<100ms`→2, else 3. An owner with no
  measured path counts as direct, class 0. `penalty` (`client.go:228-239`)
  adds +2 while backoff is active and +1 if the last view reply listed the
  owner as unreachable.
- `batches` (`client/batch.go:12-29`) cuts before k when
  `len(cur)>0 && (bytes+n > maxBytes || len(cur) >= maxKeys)`. Put batches
  use `defaultBatchBytes = 16<<20` and `batchKeys = 8192`
  (`objects.go:20-23`), with `Config.BatchBytes` capped at
  `wire.MaxPutBatch = 64<<20` (`client.go:78-81`). Get batches use
  `getBatchKeys 2048`, `getBatchBytes 8<<20`, and
  `estSize = 46 + min(key length, 64<<10)` (`fetch.go:17-25`).
- `Placed` (`objects.go:306-329`): at least `min(MinReplicas, |owners|)`
  holders among owners, and the same under `pending.nodes` while a
  transition runs.

### 2.4 Text formatting that must match byte for byte (probe)

- `client.HumanBytes` (`progress.go:170-181`): `%d B` below 1024, else
  `%.1f %ciB` over `"KMGTPE"`. Go `%.1f` rounds the exact binary value half
  to even. Probe: 0→`0 B`, 1023→`1023 B`, 1024→`1.0 KiB`, 1280→`1.2 KiB`,
  1331→`1.3 KiB`, 1152→`1.1 KiB`, 1126→`1.1 KiB`, 1536→`1.5 KiB`,
  3584→`3.5 KiB`, 1048575→`1024.0 KiB`, 1101004→`1.0 MiB`, 5<<30→`5.0 GiB`,
  MaxInt64→`8.0 EiB`, -5→`-5 B`.
- `client.Rate` (162-167): `-` when took ≤ 0, else `HumanBytes(int64(bytes/took.Seconds()))+"/s"`.
  Probe: (3 MiB, 2 s)→`1.5 MiB/s`.
- `time.Duration.String`: 0→`0s`, 1→`1ns`, 1.5s→`1.5s`, 90s→`1m30s`,
  3661s→`1h1m1s`, 999µs→`999µs`. `Round(time.Second)` rounds half away from
  zero: 2.5s→`3s`, 1.5s→`2s`.
- `time.Unix(0,ns).Format(time.RFC3339)` renders in the **local** zone,
  truncates fractions and floors negative ns, using `Z` for zero offset.
  Used by `refs`, `watch`, `ref get` (`cmd/dstore/client.go:363,399,425`).
  Probe: UTC -1→`1969-12-31T23:59:59Z`; Asia/Kolkata 1700000000123456789→`2023-11-15T03:43:20+05:30`;
  America/St_Johns 0→`1969-12-31T20:30:00-03:30`, 1719792000000000000→`2024-06-30T21:30:00-02:30`;
  Europe/Zurich 1719792000000000000→`2024-07-01T02:00:00+02:00`.
- `SyncedAt.UTC().Format(time.RFC3339Nano)` (`worktree/tree.go:187`) trims
  trailing zeros: `2023-11-14T22:13:20Z`, `…20.000000005Z`, `…20.12Z`,
  -1ns→`1969-12-31T23:59:59.999999999Z`.
- `%q` (status zone, `cmd/dstore/client.go:150`) uses `strconv.Quote`:
  `"zone-a"`, `"é"` (printable, kept), `"\x00"`, `"tab\t"`, `"​"`,
  `"\xff"`.
- `encoding/json.MarshalIndent(v, "", "  ")` plus `\n`
  (`worktree/tree.go:235-245`) escapes `&` `<` `>` as `&` `<`
  `>`, escapes U+2028 as ` `, and replaces invalid UTF-8 with
  `�`. Probe for `Name: "a&b<c> "`, `User: "\xffé"`:
  `"name": "a&b<c> "`, `"user": "�é"`.
  `json.Unmarshal` matches keys case-insensitively and ignores unknown keys;
  vectorgen must lock both (serde does neither by default).
- go-udiff v0.4.1 `udiff.Unified(oldLabel,newLabel,old,new)` uses
  `DefaultContextLines = 3` (`unified.go:17,22-24`) and writes
  `\ No newline at end of file` (`unified.go:261`). Probe outputs are in §3.4.
  Its LCS algorithm (`lcs/`) determines hunk shape, so the Rust port must
  follow it, not `similar`.

### 2.5 CLI surface: validation order, exit codes (probe on the v0.1.9 binary)

- `main` (`cmd/dstore/main.go:48-51`): any error prints `dstore: <err>` on
  stderr and exits 1. urfave/cli v2.27.7 adds:
  - an unknown command prints `No help topic for 'bogus'` on stderr and
    exits **3**;
  - an unknown flag prints `Incorrect Usage: flag provided but not defined: -bogus`,
    a blank line and the subcommand help (which, unlike `--help`, lists
    `COMMANDS: help, h`) on **stdout**, then `dstore: flag provided but not defined: -bogus`
    on stderr, exit 1;
  - required flags are checked before the action:
    `dstore: Required flag "local" not set`;
  - `dstore` with no arguments prints the app help, exit 0;
  - `dstore --version` prints `dstore version dev`, exit 0.
- Validation happens before any network use. Probe outputs:
  `refs` with no ticket → `no cluster: set --ticket or $DSTORE_TICKET`;
  `watch` → `watch PATTERN`; `diff --remote --incoming` →
  `--remote and --incoming exclude each other` (checked before `openWC`);
  `status` outside a working copy →
  `not a dstore working copy (no .dstore in this or any parent directory)`;
  `refs --ticket nope` → `ticket: "nope" is neither a dstore1 ticket nor a node id: invalid length`;
  `refs --ticket dstore1!!!` → `ticket: illegal base32 data at input byte 0`;
  `gc why zz` → `why KEY (64 hex chars)`; `node remove zz` → `view: bad node id "zz"`;
  `clone` → `clone NAME [DIR]`; `cluster replicas x` → `replicas R`.
  The ticket is parsed before `--relay`: `--relay ::bad` with an invalid id
  reports the id error.
- `dialClusterLog` (`cmd/dstore/client.go:57-74`) takes `--ticket` first.
  With `--ticket` empty and `--store` set, it derives a ticket through
  `node.OpenOffline` (Pebble). `c.String("store")` only finds a flag that
  the command defines, and only `cluster status`, `cluster ticket` and
  `catalog restore` define `storeFlag()` (main.go:321,336,651), so no other
  client command can take this path.
- Working-copy commands (`wc.go:27-34`) bind no env vars. Ticket precedence is
  flag, then stored, then `$DSTORE_TICKET` (`resolveTicket`, 40-47).
  `DSTORE_NO_DISCOVERY` is consulted only by `clone`/`init`
  (`stored == nil`, 69-73), parsed with `strconv.ParseBool`.
- Signals: `signalCtx` handles SIGINT and SIGTERM (main.go:258-260). `watch`
  returns nil on cancel and exits 0. A cancelled transfer returns its error
  and exits 1.

### 2.6 Timing constants the tests must accommodate

| constant | value | source |
|---|---|---|
| Dial per bootstrap member | 15 s | `client/client.go:98` |
| `RequestTimeout` default | 2 min | `client.go:75-77` |
| put/get stream timeout | 10 × RequestTimeout | `objects.go:245`, `fetch.go:276` |
| probe of hinted-unreachable members | 3 s each, in parallel | `client.go:257` |
| node backoff | `5s << min(failures-1, 4)`, capped 60 s; remote errors never back off | `client.go:201-218` |
| stale-view retries | 4 attempts (`callRetry`, `anyNode`) | `client.go:284-294,345` |
| put batch retries | 4 attempts; busy waits `RetryAfter` or 1 s | `objects.go:216-242` |
| push rounds / ref-put attempts | 3 / 3 | `tree.go:51,131` |
| re-pin | when a push outlives `GCInterval/2` (CLI passes 4 h) | `tree.go:116`, `cmd/dstore/client.go:90` |
| watch idle | 2 min default | `client.go:82-84`, `watch.go:174` |
| watch reconnect | 200 ms after a served stream; otherwise refresh view (15 s) then `delay + rand(delay/2+1)`, delay 1 s doubling to 30 s | `watch.go:50-91` |
| pool failed-dial memory | 2 s ("transport: peer recently unreachable") | `transport/transport.go:113-116` |
| direct dial timeout | 2 s, then relays, then discovery | `transport/iroh.go:279-301` |
| mDNS lookup | 3 s | `iroh.go:56` |
| QUIC | keep-alive 5 s, idle 60 s, 1024 incoming streams | `iroh.go:100-104` |
| relay online wait at bind | 10 s | `iroh.go:127` |
| `cluster status` per-node status | 5 s | `cmd/dstore/client.go:151` |
| plain progress line | every 5 s | `tui.go:70` |
| `node join` | 10 min per member | `main.go:516` |

Rust tests use `tokio::time::pause()` (tokio 1.53 `test-util`) for backoff,
idle and delay tests, not real sleeps.

### 2.7 Node-side commands: what they need (the harness always runs them from Go)

- `serve` (main.go:409-434) needs `openNode` (229-256): `--store` or
  `DSTORE_STORE`, otherwise `no store directory: set --store or $DSTORE_STORE`.
  It also needs `packSize` (`--pack-size` default `2Gi`, `DSTORE_PACK_SIZE`),
  `MkdirAll`, and `bindNodeEndpoint` (161-207): the `<store>/identity` file
  (hex 32-byte seed + `\n`, mode 0600), the `<store>/port` file (the UDP port
  reused across restarts, overridden by `--bind`), `--advertise-addr`,
  `--loopback`, relay mode, and discovery that announces. Then
  `node.Open(Config{StoreDir, PaxosDir, Endpoint, Logger, Jobs, Rate, MinFree, Gateway, GCInterval, PutTTL, SegmentSize})`,
  which covers the Pebble meta DB, paxos acceptor (Pebble), packstore and both
  ALPNs, followed by `n.Start` and blocking until a signal. A store with no
  view fails with
  `this store is not a member of a cluster: run cluster init or node join`.
- `cluster init` (285-317): `openNode`, `parseWeight` (`auto` =
  `statfs.Blocks*Bsize >> 30`, error `--weight auto: cannot stat the store filesystem`),
  and `n.InitCluster(ctx, R, minR, weight, zone, allowUnsafe)`. It prints
  `node id: <64hex>`, `cluster ticket: <dstore1…>` (members = this node with
  `Endpoint().Addrs()`) and `now run: dstore serve --store <dir>`.
- `node join` (476-534): `ticket.Parse(--seed)`, a token that must decode to
  32 hex bytes (`token must be 32 bytes of hex`), `openNode`, `parseWeight`,
  `Start`, then `n.Join` for each 32-byte member (10 min each), wrapping
  failures as `join: %w`. It prints `node id:` and serves until a signal.
- `catalog restore KEY|FILE` (651-695): a readable FILE wins; otherwise KEY
  (64 hex) is fetched through `client.Get` with `recordPayload`. It needs
  `--store`, `node.OpenOffline` (Pebble) and `RestoreCatalog`, and prints
  `%d references written`.
- A ticket derived from `--store` (`localTicket`, `cmd/dstore/main.go:394-405`)
  uses `node.OpenOffline(dir).View()` with `worktree.TicketFromView`, or fails
  with `this store is not a member of a cluster`.

None of these is reachable without Pebble and the paxos acceptor, so the Rust
CLI either omits them or prints a fixed error (open decision in §8). The
harness uses the Go binary for every node role.

---

## 3. Byte formats and text formats, verbatim

All values in 3.1-3.4 come from the probe program against dstore v0.1.9,
core v0.0.8, transport-iroh v0.4.0, fxamacker v2.9.3 and go-udiff v0.4.1.
They are the first vectors the Rust tests should pass, and vectorgen
reproduces them.

Fixtures: `cid = 00 01 … 0f` (16 bytes), `id = 07×32`, `k1 = 11×32`.

### 3.1 Frames (4-byte BE length + canonical CBOR)

```
ok           (Msg{Type:TOK})                                         00000004a1001835
view-req     (TView, cid, inc 1, epoch 7)                            0000001aa40018200150000102030405060708090a0b0c0d0e0f02010307
missing-req  (TMissing, cid, 1, 7, Keys [k1], Pin)                   00000040a60018210150000102030405060708090a0b0c0d0e0f0201030704815820111111111111111111111111111111111111111111111111111111111111111105f5
ref-put-new  (TRefPut, cid, 1, 7, Record a0, HasExpected, no Expected*)  0000001fa60018250150000102030405060708090a0b0c0d0e0f020103070741a011f5
err-busy     (TErr, Code "busy", Text "slow down", RetryAfter 1500)  00000019a4000a0a64627573790b69736c6f7720646f776e181c1905dc
```

`Dial`'s first request is unstamped (`client.go:99`, through `pool.Call`, not
`c.call`): `00000004a1001820`. Every later request goes through `c.stamp`
(189-197) and carries keys 1-3.

`SendPackRecords` of one raw record, `key.New(Blob, 15, "some blob bytes")`
(flag 0, so the bytes are deterministic):

```
0000004ca20007085846414d424552504b0301000f1d515e4e7f0a5ad3c42f8a9fdd51eb7d32ab303d1f5945944c7b59c48d7f000000000f0000000f59643432736f6d6520626c6f622062797465730000000003a10008
```

That is one `TData` frame (key 8 = `AMBERPK\x03` + 46-byte header + payload
+ end marker `00`, 70 bytes) followed by `TDataEnd` `00000003a10008`. The
chunk writer (`transport-iroh protocol/pack.go:34-68`) emits frames of exactly
`ChunkSize = 1<<20` data bytes plus a final shorter one.
`NewPackReader` (101-141) turns a `TErr` mid-pack into
`protocol.RemoteError`, whose text is always `remote: %s: %s` (an empty
text still gives a trailing `": "`, protocol.go:173-175). `wire.Error` is
different: `remote: <code>` when the text is empty, else
`remote: <code>: <text>` (`wire/wire.go:295-300`).

### 3.2 Ticket, view, admin, status

```
ticket (cid, inc 1, [{id, ["ip:127.0.0.1:4433"]}])
  dstore1umafaaabaibqibiga4eascqlbqgq4dybaebidiqalaqaobyha4dqobyha4dqobyha4dqobyha4dqobyha4dqobyha4dqobybqfyws4b2gezdolrqfyyc4mj2gq2dgmy
ticket.IDs()       0707070707070707070707070707070707070707070707070707070707070707
view (cid, inc 1, epoch 2, version 3, placement_epoch 2, R 3, minR 2, voters [{id,1}], nodes [{id, 100, ["ip:127.0.0.1:4433"], writable}])
  aa0050000102030405060708090a0b0c0d0e0f0101020203030402050306020781a20058200707070707070707070707070707070707070707070707070707070707070707010108000a81a4005820070707070707070707070707070707070707070707070707070707070707070701186402817169703a3132372e302e302e313a3434333307f5
worktree.TicketFromView(view).Encode() == the ticket above
AdminRequest{Op:"gc-run", Garbage:0.5}   a2006667632d72756e09f93800            (float16)
AdminRequest{Op:"gc-run", Garbage:0.3}   a2006667632d72756e09fb3fd3333333333333 (float64)
node.Status{ID:id, Epoch:5, Packs:2, Records:10, Bytes:12345, FreeBytes:1<<40, Writable:true}
  b00058200707070707070707070707070707070707070707070707070707070707070707010502000302040a05193039060008000df50e1b00000100000000000f0010001100120013001400
empty tree (fstree.EncodeDirLeaf(nil))   key 2001bbe6a9f5a0146a1f4d0381e9b0ed1ac2f1a979ce9d5ad84e46ff0b58f36b, bytes 80
```

In the Status encoding, keys 2, 6, 8 and 15-20 are present as `00` because
those fields have no `omitempty` (`node/status.go:22-52`).

### 3.3 Working-copy files

`.dstore/config` from `worktree.Create(dir, Config{Ticket:"dstore1abc", Name:"trees/demo", NoRelay:true, User:"me"})`:

```
{
  "ticket": "dstore1abc",
  "name": "trees/demo",
  "no_relay": true,
  "user": "me"
}
```

`.dstore/state` with `HasRemote`, `Remote = empty`, `RemoteVersion 010203`,
`SyncedAt = time.Unix(1_700_000_000, 5).UTC()`:

```
{
  "base": "2001bbe6a9f5a0146a1f4d0381e9b0ed1ac2f1a979ce9d5ad84e46ff0b58f36b",
  "remote": "2001bbe6a9f5a0146a1f4d0381e9b0ed1ac2f1a979ce9d5ad84e46ff0b58f36b",
  "remote_version": "010203",
  "synced_at": "2023-11-14T22:13:20.000000005Z"
}
```

Both files end with `\n`, are written to `path + ".tmp"`, then renamed
(`tree.go:235-245`). Without a remote, `remote` and `remote_version` are
written as `""` (`stateJSON` has no omitempty, `tree.go:59-64`). The layout is
`.dstore/{config,state,packstore/}` (`tree.go:22-28`).

### 3.4 Unified diffs (go-udiff)

```
udiff.Unified("a/edit.txt","b/edit.txt","one\ntwo\nthree\n","one\n2\nthree\n")
--- a/edit.txt
+++ b/edit.txt
@@ -1,3 +1,3 @@
 one
-two
+2
 three

udiff.Unified("a/link","b/link","t1","t2")
--- a/link
+++ b/link
@@ -1 +1 @@
-t1
\ No newline at end of file
+t2
\ No newline at end of file

udiff.Unified("/dev/null","b/new.txt","","hi\n")
--- /dev/null
+++ b/new.txt
@@ -0,0 +1 @@
+hi
```

Wrapping from `worktree/diff.go`:

- each content change starts with `diff a/P b/P\n`;
- a mode change adds `old mode %04o\nnew mode %04o\n`;
- a directory's own mode change shows only the header and mode lines;
- binary or oversized content gives `Binary files a/P and b/P differ\n`, with
  `/dev/null` for an absent side;
- `--stat` lines are ` P | +A -D\n` or ` P | binary\n`, followed by
  ` %d files changed, %d insertions(+), %d deletions(-)\n`
  (diff.go:163-205);
- binary means a NUL within the first 8192 bytes (154-159); oversized means
  `length > 16<<20` (`MaxDiffBytes`, 19).

### 3.5 CLI output formats the harness parses or compares

| command | stdout / stderr format | source |
|---|---|---|
| `cluster init` | `node id: %s` / `cluster ticket: %s` / `now run: dstore serve --store %s` | main.go:312-314 |
| `token create` | the hex token (`AdminReply.Text`) | main.go:456 |
| admin actions | `Text` line if non-empty; each `Names` entry; `key %x` if `len(Key)==32` | cmd/dstore/client.go:122-131 |
| `cluster replicas R` | prompt `changing R to %d moves about 1/%d of every node's data; continue? [y/N] `; a non-`y` answer gives `aborted` | main.go:379-384 |
| `store push` | stderr `built %s: %d new objects` (root[:16]); stdout `pushed %s: %d objects, %d uploaded, version %x`; CAS error gets ` (pull first, or --force)` appended | client.go:264,289,297 |
| `store pull` | `pulled %s: root %s, %d objects fetched (%d bytes)` (full 64-hex root) | client.go:338 |
| `refs` | `%s\t%x\t%s\t%s` (name, key, RFC3339 local, user) | client.go:363 |
| `watch` | `%s\tdeleted` or `%s\t%x\t%s\t%s`; Synced prints nothing | client.go:394-400 |
| `ref get` | `name %s\nkey %x\nversion %x\nuser %s\ncreated %s` | client.go:425 |
| `ls` / `cat` | one name per line / raw bytes; `not a regular file with content` | client.go:505,543 |
| `cluster status` | `cluster %x incarnation %d epoch %d version %d`; `replicas %d min_replicas %d nodes %d voters %d` + ` (no catalog fault tolerance)` when voters<3; `transition %d (%s): frozen=%v acked=%d participants=%d done=%d`; `voter change in progress (target %s)`; per node `  %s weight %d zone %q voter=%v writable=%v` [+ ` — unreachable: %v` or ` — bad status`], `      epoch %d packs %d records %d bytes %d pins %d pending-packs %d free %d GiB` + ` [lease holder]`/` [AMNESIAC]`/` [retired]`, `      cannot reach: %s`, `      gc: %s`, `      transition: %s`, `      voter %s: %d calls, %d failures, p99 %d ms` | client.go:134-193 |
| `clone` | `cloned %s into %s: root %s, %d objects fetched (%d bytes)` (root[:16]) | wc.go:156 |
| `init` | `initialised working copy of %s; the reference exists (root %s): status shows everything as new, pull merges` / `initialised working copy of %s; the reference does not exist yet: push creates it` | wc.go:204,206 |
| `fetch` | `%s does not exist on the cluster` / `%s: up to date (%s)` / `fetched %s: root %s, %d objects fetched (%d bytes)` | wc.go:258-262 |
| `pull` | stderr `conflicts:` + `  %s (local: %s, cluster: %s)`; stdout `already up to date` or `pulled: %d paths updated` [+ `, %d conflicts taken from the cluster`] | wc.go:281-297 |
| `push` (wc) | `nothing to push` / `%s already holds %s (an earlier push completed); state updated` / `pushed %s: root %s, %d objects, %d uploaded, version %x` | wc.go:327-332 |
| `status` | `reference %s, synced to %s`; `remote: up to date` / `remote: the reference does not exist on the cluster` / `remote: moved since your last fetch (+%d ~%d -%d; run pull)`; `changes:` + `  %-9s %s` (Kind name `new` `deleted` `modified` `type` `mode` `meta`, path + `/` for dirs, ` (%s → %s)` type names, ` (%04o → %04o)`); `%d paths differ only in mtime, ownership or xattrs`; `nothing to push` | wc.go:354-405, worktree/change.go:25-73 |
| plain progress | `%d/%d objects` [+ `  %s / %s` or `  %s`] + `  %s/s` [+ `  eta %s`] | tui.go:117-130 |

`TypeName` (`worktree/change.go:55-73`): `file`, `directory`, `symlink`,
`fifo`, `socket`, `char device`, `block device`, else `type %#o`.

### 3.6 App help (the urfave template reproduced byte for byte)

`dstore` with no arguments, exit 0 (the CLI snapshot file holds the rest):

```
NAME:
   dstore - a distributed amber store: cluster nodes and the client

USAGE:
   dstore [global options] command [command options]

VERSION:
   dev

COMMANDS:
   cluster     init, status, ticket, replicas
   serve       run a node
   token       join tokens
   node        join, remove, drain, weight, zone, repair
   voter       change a node's vote after join
   transition  status, abort, refreeze, pause, resume
   gc          run, status, why, hold, release
   catalog     backup, restore, backups
   store       push and pull between a standalone local store and the cluster
   clone       clone the tree under NAME into DIR (default: the last segment of NAME) as a working copy
   init        make the current directory a working copy of NAME, with nothing synced yet
   fetch       record the reference's current tree as the remote and fetch its objects
   pull        fetch and apply the cluster's changes over the working directory
   push        build the working directory's tree, upload it and write the reference
   status      list the working directory's changes since the last sync, and whether the cluster moved
   diff        unified diffs of the working directory against the last synced tree
   refs        list references
   watch       watch references matching a glob and print each change until interrupted
   ref         get or delete a reference
   ls          list a directory of a pushed tree
   cat         write a file of a pushed tree to stdout
   help, h     Shows a list of commands or help for one command

GLOBAL OPTIONS:
   --log-level value  debug|info|warn|error (a global flag: give it before the command) (default: "info") [$DSTORE_LOG_LEVEL]
   --help, -h         show help
   --version, -v      print the version
```

`dstore refs --help`:

```
NAME:
   dstore refs - list references

USAGE:
   dstore refs [command options] [PREFIX]

OPTIONS:
   --ticket value  cluster ticket (dstore1…) or comma-separated node ids, found by discovery [$DSTORE_TICKET]
   --relay value   relay URL for the fallback path (default: the built-in relay map)
   --no-relay      direct addresses only, no relay (default: false)
   --no-discovery  neither announce this endpoint nor resolve node ids by discovery (mDNS and, with relays, number0's DNS) (default: false) [$DSTORE_NO_DISCOVERY]
   --help, -h      show help
```

---

## 4. Rust design

### 4.1 Repository layout (verification parts)

```
Cargo.toml                 package dstore-client-rs, [[bin]] dstore, rust-version = "1.91"
Cargo.lock                 committed (binary crate; the Nix build needs it)
VECTORS.md                 schema of tests/golden (as in core-rs VECTORS.md)
tests/common/mod.rs        splitmix64, Payload, golden_dir(), load_json(), objects.bin reader (copied from core-rs tests/common/mod.rs)
tests/golden.rs            ONE integration-test binary; `mod` per family below (simplifies cargoTestFlags)
tests/golden/              committed vectors (§4.3)
tests/cli_snapshots.rs     runs env!("CARGO_BIN_EXE_dstore") over tests/golden/cli/snapshots.json
tests/fake_cluster.rs      client + worktree tests over transport::mem + FakeNode (§4.6)
tests/iroh_loopback.rs     real Rust-iroh endpoints on 127.0.0.1 (§4.7); excluded from the Nix sandbox
src/testing/               FakeNode, behind cargo feature `test-support` (or tests/common/fake_node.rs, §8)
tools/vectorgen/           Go module: vector generator + helpers (§4.2)
interop/check.sh           live Go<->Rust harness (§4.5)
interop/lib.sh             run/compare/normalize helpers
flake.nix, flake.lock, .envrc ("use flake"), .gitignore (target/, .direnv/, result, result-*)
.github/workflows/ci.yml
```

### 4.2 `tools/vectorgen` (Go)

`tools/vectorgen/go.mod`:

```
module github.com/amber-store/dstore-client-rs/tools/vectorgen

go 1.26.5

require (
	github.com/amber-store/core v0.0.8
	github.com/amber-store/dstore v0.1.9
	github.com/aymanbagabas/go-udiff v0.4.1
	github.com/fxamacker/cbor/v2 v2.9.3
	github.com/tmc/go-iroh v0.2.0
	github.com/zeebo/blake3 v0.2.4
)
```

(`go mod tidy` adds the indirect block.)

**Module-graph check, done for this spec.** A scratch module that required
only `github.com/amber-store/core v0.0.8` and `github.com/amber-store/dstore v0.1.9`
and imported client, codec, node, placement, refglob, ticket, view, wire,
worktree, core amberpack/key and go-udiff was tested with nixpkgs Go 1.26.5
(`GOTOOLCHAIN=local`):

1. `GOFLAGS=-mod=mod GOPROXY=file:///Users/dragan/go/pkg/mod/cache/download GOSUMDB=off`
   with a scratch `GOMODCACHE`: `go mod tidy` succeeded, `go list -m all`
   gave 201 modules, and `go run .` produced §3. MVS resolved
   transport-iroh v0.4.0, go-iroh v0.2.0, cbor v2.9.3 and udiff v0.4.1.
2. The literal form `GOFLAGS=-mod=mod GOPROXY=off` with the default
   `GOMODCACHE=/Users/dragan/go/pkg/mod`: `go mod tidy` and
   `go build -o /dev/null .` succeeded, and
   `find /Users/dragan/go/pkg/mod -newer <marker>` found nothing, so the cache
   was not written. All 69 module trees the build needs were already
   extracted there.
3. The offline `tidy` wrote a smaller `go.sum` (173 lines, against 297 from
   the file-proxy run; it omits `/go.mod` hash lines for pruned modules).
   Generate the committed `go.sum` online with the default proxy; the offline
   runs only prove the graph resolves.
4. `CGO_ENABLED=0 GOBIN=<tmp> go install github.com/amber-store/dstore/cmd/dstore@v0.1.9`
   built the CLI offline (file proxy), which is how §3.6 and §2.5 were
   captured. The binary was deleted afterwards. `CGO_ENABLED=0` matches the
   Dockerfile and avoids the macOS Xcode-license cgo problem recorded in
   `docs/superpowers/plans/2026-09-16-working-copy.md:15-17`.

**Rules** (as core-rs `tools/vectorgen/main.go`, `util.go`):

- `vectorgen <out-dir>`: deletes exactly the files and directories it owns,
  then regenerates. Two runs must give identical bytes.
- JSON only through `writeJSON` (`MarshalIndent`, 2 spaces, trailing `\n`),
  from structs and ordered slices, never maps. Bytes are lowercase hex
  strings. `u64`/`i64` values are decimal strings (they exceed 2^53).
  `bool` and small ints are JSON numbers and booleans.
- Payloads use splitmix64 `data(seed, n)` and the `Payload` JSON forms
  exactly as in core-rs `VECTORS.md` ("Deterministic data streams").
- ed25519 node ids come from `crypto/ed25519.NewKeyFromSeed(data(seed,32))`,
  so they are valid points and deterministic.
- Never iterate a Go map into output. Several Go code paths are
  order-random: `watchOnce` builds `Refs` from a map (`client/watch.go:136-139`);
  `shortError` names (`tree.go:233-237`); `pickBatch` (`fetch.go:206-234`);
  `node.status` `Voters` (`node/status.go:97-99`). Vectors either avoid
  these or sort.
- Functions in `package main` (`cmd/dstore`) cannot be imported. vectorgen
  holds **verbatim copies** in `mainpkg_copy.go` of `parseSize`
  (size.go:14-46), `statusLine` and `fraction` (tui.go:117-143),
  `resolveTicket` (wc.go:40-47), `describeChange` (wc.go:393-405),
  `hexDecode` (client.go:554-574) and `rateMeter` (tui.go:88-114). A
  self-check runs before generating: it finds the module directory
  (`go list -m -f {{.Dir}} github.com/amber-store/dstore`), parses
  `cmd/dstore/*.go` with `go/parser`, prints each `FuncDecl` with
  `go/printer`, and compares against the copy. Any difference fails.

**Helper commands** in the same module (their output lives in temp dirs and
nothing is committed):

| command | purpose |
|---|---|
| `go run ./cmd/treekey [-exclude .dstore] DIR` | root key of a directory computed by core `ingest` without storing (drain `ingest.ScanWith` / `Objects` with `Opts{Exclude}`), printed as 64 hex |
| `go run ./cmd/mktree -seed S DIR` | deterministic source tree for interop: splitmix files incl. 0 B, 1 B, 5 MiB multi-chunk, 300 KiB constant (compressible), unicode names, nested depth 4, 2000 small files, symlink, fifo, 0o755 script, fixed mtimes, `.amberignore` + one ignored file, best-effort `user.*` xattr |
| `go run ./cmd/storecmp A B ROOT` | opens two packstores with Go core and checks every object reachable from ROOT is present and byte-equal in both |
| `go run ./cmd/holdlock DIR SECONDS` | opens `DIR/.dstore/packstore` (flock) and sleeps, for the lock-interop check |
| `go run ./cmd/clisnap -o OUT` | builds the Go CLI (`CGO_ENABLED=0 go install github.com/amber-store/dstore/cmd/dstore@v0.1.9` into a temp `GOBIN`, deleted on exit), runs every case of `tools/vectorgen/clisnap/cases.json`, writes `OUT/snapshots.json` |

### 4.3 Vector files and schemas (`tests/golden/`)

Common JSON types: `hex` string; `u64s` decimal string; `Payload` per core-rs.

1. **`wire/frames.json`**: every client-ALPN message.
   `{ "cases": [ { "name": "...", "msg": MsgJSON, "frame_hex": "..." } ] }`.
   `MsgJSON` uses the Go field names of `wire.Msg`
   (`Type`, `ClusterID`, `Incarnation`, … `Deleted`); omitted means zero.
   Nested `RefInfo` / `KeyHolders` / `KeyFailure` / `KeyReject` use Go
   field names too. Rust asserts `encode(msg) == frame_hex` and
   `decode(frame_hex) == msg`.
2. **`wire/decode.json`**: laxness.
   `{ "cases": [ { "name", "payload_hex", "ok": bool, "canonical_hex"?: hex, "go_error"?: string } ] }`.
   Rust must match `ok` and `canonical_hex`; `go_error` is informational.
3. **`wire/frame_errors.json`**:
   `{ "cases": [ { "name", "stream_hex", "result": "ok"|"eof"|"unexpected_eof"|"too_large"|"short"|"decode" } ] }`.
4. **`wire/pack_frames.json`**: `SendPackRecords` chunking.
   `{ "cases": [ { "name", "records": [ {"key": hex, "payload": Payload} ], "frames": [ {"type": 7|8, "data_len": n, "data_blake3": hex, "data_hex"?: hex} ], "stream_blake3": hex } ] }`.
   Payloads are incompressible (flag 0), so records are
   implementation-independent. Include `stream_hex` when ≤ 4096 bytes.
5. **`ticket/encode.json`**:
   `{ "cases": [ { "name", "ticket": {"cluster_id": hex, "incarnation": u64s, "members": [ {"id": hex, "addrs": [..]|null} ]|null}, "cbor_hex", "encoded", "ids" } ] }`.
6. **`ticket/parse.json`**:
   `{ "cases": [ { "name", "input", "ok": bool, "ticket"?: ..., "error"?: "exact text" } ] }`.
   Error text **must** match: the CLI prints it.
7. **`view/views.json`**:
   `{ "cases": [ { "name", "view": ViewJSON, "hex", "decode_only": bool } ] }`.
   `ViewJSON` uses Go field names.
8. **`view/placement_ops.json`**:
   `{ "cases": [ { "name", "view_hex", "keys": [ { "key": hex, "slot": n, "owners": [hex], "write_set": [hex], "read_order": [hex], "pending_owners": [hex]|null } ], "ticket_from_view": "dstore1…", "all_members": [hex] } ] }`.
9. **`placement/placement.json`**:
   `{ "fmix64": [ {"in": u64s, "out": u64s} ], "log2fix": [ {"in": u64s, "out": u64s} ], "salts": [ {"id": hex, "salt": u64s} ], "l": [ {"slot": n, "salt": u64s, "l": u64s} ], "sets": [ { "name", "members": [ {"id": hex, "weight": n, "zone": ""} ], "r": n, "slots": [ {"slot": n, "rank": [idx], "owners": [idx]} ], "all_slots_blake3": hex } ] }`.
   `all_slots_blake3` = BLAKE3 over, for slot 0..2^20-1, `u8 len(rank) ‖ rank indexes as u8 ‖ u8 len(owners) ‖ owners as u8`.
10. **`client/rank.json`**:
    `{ "rank_owners": [ { "name", "n": count, "penalties": [n], "paths": [ {"present": bool, "direct": bool, "rtt_ns": u64s} ], "want": [idx] } ], "rtt_class": [ {"rtt_ns": u64s, "class": n} ], "batches": [ { "sizes": [n], "max_bytes": n, "max_keys": n, "want_lens": [n] } ], "placed": [ { "view_hex", "key": hex, "holders": [hex], "placed": bool } ] }`.
11. **`admin/requests.json`**:
    `{ "cases": [ { "name", "argv": ["gc","run","--garbage","0.5"], "request": AdminRequestJSON, "params_hex", "frame_hex" } ] }`.
    One case per CLI subcommand that builds an `AdminRequest`
    (main.go:352,386,452,541,549,561,569,577,590,605,627-638,649-650).
    Also **`admin/replies.json`**:
    `{ "cases": [ { "name", "reply": AdminReplyJSON, "status_hex", "printed": "exact stdout of adminAction" } ] }`.
12. **`status/status.json`**:
    `{ "cases": [ { "name", "status": StatusJSON, "hex" } ] }`,
    covering all omitempty variants and `VoterStat`.
13. **`worktree/config.json`**:
    `{ "encode": [ { "config": {...}, "file": "exact bytes" } ], "decode": [ { "file", "ok", "config"?, "error"? } ] }`.
    Decode cases: case-insensitive keys, unknown keys, duplicate keys,
    invalid UTF-8 input.
14. **`worktree/state.json`**:
    `{ "encode": [ { "base": hex, "has_remote": bool, "remote": hex, "remote_version": hex, "synced_at_ns": u64s, "file" } ], "decode": [ { "file", "ok", "state"?, "error"?: "exact text of loadState" } ] }`.
    Error texts reach stderr, e.g.
    `bad state file: remote_version: encoding/hex: odd length hex string`.
15. **`worktree/trees/`**: `objects.bin` (`key 32 ‖ BE u64 len ‖ bytes`, as in
    core-rs) plus `trees.json` = `{ "trees": [ {"name", "root": hex} ] }`,
    built through `fstree` builders with fixed metadata (no filesystem).
16. **`worktree/diff_trees.json`**:
    `{ "cases": [ { "name", "a": treeName, "b": treeName, "changes": [ {"path", "kind": "new"|…, "old_mode": n|null, "new_mode": n|null} ], "get_calls": n } ] }`.
17. **`worktree/merge.json`**:
    `{ "cases": [ { "name", "local": [ChangeJSON], "incoming": [ChangeJSON], "apply": [path], "conflicts": [path] } ] }`.
    `ChangeJSON` carries full entries: `name`, `mode`, `uid`, `gid`,
    `mtime_ns`, `content_key`, `link_target`, `rdev`, `xattrs_in`,
    `xattrs_key`.
18. **`worktree/unified.json`**:
    `{ "cases": [ { "name", "a", "b", "unified": "exact text", "stat": "exact text" } ] }`
    over DiffTrees of the trees in item 15 (TreeSource on both sides).
19. **`udiff/udiff.json`**:
    `{ "cases": [ { "name", "old_label", "new_label", "old": Payload|string, "new": Payload|string, "out": "exact text" } ] }`.
20. **`text/formats.json`**:
    `{ "human_bytes": [ {"n": i64s, "out"} ], "rate": [ {"bytes": i64s, "took_ns": i64s, "out"} ], "duration": [ {"ns": i64s, "string", "round_second"} ], "rfc3339": [ {"ns": i64s, "zone", "out"} ], "rfc3339nano_utc": [ {"ns": i64s, "out"} ], "go_quote": [ {"in_hex", "out"} ], "status_line": [ {"report": {...}, "rate": f64-as-string, "out", "fraction": "…"} ], "describe_change": [ {"change": ChangeJSON, "out"} ], "type_name": [ {"mode": n, "out"} ] }`.
21. **`cli/size.json`**:
    `{ "parse": [ {"in", "ok", "out"?: i64s, "error"?: "exact"} ], "pack_size": [ {"flag"?, "env"?, "ok", "out"?, "error"?} ] }`.
22. **`errors/text.json`**:
    `{ "cases": [ { "name", "out": "exact" } ] }`. Covers every client or
    worktree error string reachable from the CLI with fixed inputs:
    `wire.Error` with and without text, `protocol.RemoteError`,
    `CASMismatch` (absent / current), `Incomplete`, `ErrUnknownRef`, the
    `worktree` sentinels (flow.go:20-24, tree.go:34-35),
    `%w (%v)` wrapping for `ErrRefChanged` (flow.go:249),
    `client: no bootstrap node answered: %w`, `client: unexpected reply %d`,
    `negotiate %x at its primary: %w`, `record %x rejected: %s`,
    `pull: object %x not found in the cluster`.
23. **`refglob/refglob.json`** (for the fake node):
    `{ "match": [ {"pattern", "name", "match"} ], "prefix": [ {"pattern", "prefix"} ], "invalid": [ {"pattern", "error": "exact"} ] }`.
    The error reaches stderr as `dstore: remote: bad-request: <error>`.
24. **`cli/snapshots.json`** (from `cmd/clisnap`, §4.2):
    `{ "cases": [ { "name", "args": [..], "env": {..}, "cwd": "empty"|"wc-no-config"|"wc-no-state"|"wc-bad-state-key", "stdin": "", "exit": n, "stdout", "stderr" } ] }`.
    Outputs normalize the temp cwd to `{CWD}` (`pwd -P`).

A Rust golden test **fails** when a file is missing (core-rs PORTING.md rule).

### 4.4 Rust consumption and dev-dependencies

All of these are in the offline registry
(`/Users/dragan/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f`):

```toml
[dev-dependencies]
serde = { version = "1.0.229", features = ["derive"] }
serde_json = "1.0.151"
hex = "0.4.3"
tempfile = "3.27.0"
jiff = { version = "0.2.35", features = ["tzdb-bundle-always"] }  # TZ vectors without a system tzdb (Nix sandbox)
tokio = { version = "1.53.1", features = ["macros", "rt-multi-thread", "test-util", "time"] }
walkdir = "2.5.0"
xattr = "1.6.1"
rustix = { version = "1.1.4", features = ["fs"] }   # mkfifo/chmod/utimensat in worktree tests
similar = "2.7.0"                                   # only for readable assertion diffs, never for output
```

`assert_cmd` is not available offline, so CLI tests spawn the binary with
`std::process::Command`, as core-rs `tests/cli_e2e.rs` does, using
`env!("CARGO_BIN_EXE_dstore")`. The environment is cleared except `PATH`,
`HOME` (a temp dir) and `TZ=UTC`. Golden loaders reuse core-rs
`tests/common/mod.rs` (`SplitMix64`, `data`, `u64s`, `Payload`,
`golden_dir`). One test module per §4.3 family lives inside
`tests/golden.rs`.

Mapping of verification needs onto core-rs 0.3.0 (rev a85ffa1), with the
functions checked:

- objects and records: `amberpack::encode_record` (`src/amberpack.rs:126`),
  `parse_record` (:157), `decode_payload` (:205), `Writer::add_record`
  (:268), `Reader::records` (:424).
- keys and local stores: `key::Key::validate` (`src/key.rs:158`);
  `packstore::Store::open` (`src/packstore/mod.rs:359`, flock at 369-370
  matching Go `packstore.go:164`), `get` (:772), `get_record` (:807),
  `stored_size` (:832), `has` (:914), `write_parallel`
  (`src/packstore/parallel.rs:109`).
- trees: `fstree::reachable_keys` (`src/fstree/read.rs:694`),
  `check_complete` (:742), `child_keys` (:368), `collect_entries` (:536),
  `resolve_path` (:463), `resolve_entry` (:498), `write_content` (:653),
  `encode_dir_leaf` (`src/fstree/encode.rs:130`).
- references: `reference::validate_name` (`src/reference.rs:265`),
  `validate_user` (:292), `Reference::encode` (:384), `decode` (:394);
  `refstore::Store::open` (`src/refstore.rs:114`, redb).
- ingest and xattrs: `ingest::dir` (`src/ingest/mod.rs:379`), `objects`
  (:311); `cbor::encode_xattrs` / `decode_xattrs` (`src/cbor.rs:220,245`).

### 4.5 Live interop harness (`interop/check.sh`)

**Inputs** (environment variables):

- `DSTORE_GO_BIN`: a prebuilt Go dstore, copied into the work dir.
- `DSTORE_GO_REPO`: a checkout at tag v0.1.9. The harness verifies
  `git -C "$DSTORE_GO_REPO" describe --tags --exact-match` is `v0.1.9`, then
  runs `CGO_ENABLED=0 go build -trimpath -o "$W/dstore-go" ./cmd/dstore`.
- Neither set: `GOBIN="$W/gobin" CGO_ENABLED=0 go install github.com/amber-store/dstore/cmd/dstore@v0.1.9`.
- `DSTORE_RS_BIN`: the Rust binary. Default:
  `cargo build --release --locked --bin dstore` with
  `target/release/dstore`. With `INTEROP_RS_TARGET=tmp`, the build goes to
  `CARGO_TARGET_DIR="$W/target"` and is deleted with the work dir.
- `INTEROP_MDNS=auto|require|skip` (default `auto`); `INTEROP_HEAVY=1`
  (transitions); `INTEROP_CHAOS=1` (node restarts); `INTEROP_KEEP=1` (keep
  logs, still delete binaries); `INTEROP_FAILFAST=1`;
  `INTEROP_LOG_DIR` (logs copied there on failure).

**Portability.** bash only, macOS and Linux. No GNU-only flags (`find -printf`,
`sed -i`) and no `timeout` (absent on macOS); waits are poll loops. The work
dir is `W=$(mktemp -d "${TMPDIR:-/tmp}/dstore-interop.XXXXXX")`. The trap
kills every recorded PID (nodes, watchers), waits for them, then
`rm -rf "$W"`. The Go binary always lives under `$W`, so it is always
removed.

**Cluster.** This follows `scripts/e2e-loopback.sh`, with one fix. At v0.1.9
lines 42 and 46 still say `dstore push --local …` / `dstore pull --local …`,
but those commands moved under `store` and `push`/`pull` are now the
working-copy commands without `--local`, so the script as committed fails
with `flag provided but not defined: -local`. Use `store push` / `store pull`.

```
export DSTORE_LOG_LEVEL=warn
go/dstore cluster init --store n1 --replicas 3 --weight 100 --no-relay --loopback > init.out
T=$(grep 'cluster ticket:' init.out | awk '{print $3}'); export DSTORE_TICKET=$T
go/dstore serve --store n1 --no-relay --loopback --gc-interval 1m > n1.log 2>&1 &     # record PID
TOK2=$(go/dstore token create --no-relay)                                                  # Go token
go/dstore node join --store n2 --seed "$T" --token "$TOK2" --weight 100 --no-ramp --no-relay --loopback > n2.log 2>&1 &
TOK3=$(rs/dstore token create --no-relay)                                                  # Rust token (check A3)
go/dstore node join --store n3 --seed "$T" --token "$TOK3" --weight 100 --no-ramp --no-relay --loopback > n3.log 2>&1 &
poll (≤ 150 × 2 s): go/dstore cluster status --no-relay contains 'nodes 3 voters 3' and no line '^transition '
```

Every client command runs with `--no-relay` and `--no-tui` (stderr is a file
anyway). For each check the harness runs the Go client first, which is the
expected output, and then the Rust client. Both are recorded in
`$W/checks/<id>.{go,rs}.{out,err,exit}`, and mismatches are printed as
`diff -u`.

**Comparison modes.**

- `exact`: stdout, stderr and exit code are all identical.
- `stdout+exit`: stderr may differ in slog lines, but its last line starting
  with `dstore: ` must be identical.
- `normalized`: both outputs go through `interop/lib.sh:norm_status`, which
  replaces digit runs in lines starting with `      epoch ` and
  `      voter ` by `N` and sorts consecutive `      voter ` lines (node-side
  map order).
- `format`: a regex per line.
- `root`: the root keys printed or computed are equal.

**TZ matrix.** Checks marked `tz` run three times, with `TZ=UTC`,
`TZ=Asia/Kolkata` and `TZ=America/St_Johns`.

**Checks.** "go→rs" means the Go client acts first and the Rust client
verifies, and vice versa. Every check also passes when both clients are Go:
the harness runs a Go-only pass once with `INTEROP_BASELINE=1`, to separate
Rust bugs from harness bugs.

| id | check | mode |
|---|---|---|
| **A: cluster and admin** | | |
| A1 | `cluster status` go vs rs, run back to back | normalized |
| A2 | `cluster ticket` and `cluster ticket --ids` go vs rs; each printed ticket, used as `--ticket`, lets the other implementation run `refs` | exact |
| A3 | `rs token create` prints `^[0-9a-f]{64}$`; node n3 joins with it (proves the `AdminRequest` CBOR); cluster reaches 3/3 | format + effect |
| A4 | `transition status`, `gc status` (after `gc hold`), `catalog backups` go vs rs | exact |
| A5 | `gc hold` then `gc release`: both print `ok` | exact |
| A6 | `rs gc run` exits 0; its text equals the immediately following `go gc status` (after the cycle settles, hold on) | exact |
| A7 | `gc why <root of trees/rs>` and `gc why <64 zeros>` go vs rs (names list; empty prints nothing) | exact |
| A8 | `rs catalog backup` prints `backup written` and `key <64hex>`; the key appears in `go catalog backups` | format + effect |
| A9 | `node repair <n2 id>` go vs rs: `repair scheduled: holders will refill <8hex>` | exact |
| A10 | `transition pause` / `transition resume` go vs rs: `ok` | exact |
| A11 | node-reported errors go vs rs: `ref get trees/none` (`dstore: client: unknown reference`), `watch 'trees/['` (`dstore: remote: bad-request: refglob: …`), `ref get 'bad@@name'` (bad-request text from `reference.ValidateName`), `voter add <64hex non-member>` | exact stderr+exit |
| A12 (heavy) | `rs node weight <n3> 50` prints `transition proposed at epoch N`; wait steady; `go cluster status` shows `weight 50`; restore to 100 with Go | format + effect |
| A13 (heavy) | `rs cluster replicas 2 --yes`, wait steady, `go cluster replicas 3 --yes` | format + effect |
| **B: standalone local stores** | | |
| B1 | `mktree -seed 1 src1`; `rs store push --local rsL --user interop src1 trees/rs`; stderr has `built <16hex>: N new objects`; stdout has `pushed trees/rs: K objects, U uploaded, version <hex>` | format |
| B2 | `go store pull --local goL trees/rs`: root = `treekey src1`; `storecmp rsL/packstore goL/packstore ROOT` (Go reads the Rust-written packstore) | root + effect |
| B3 | reverse: `mktree -seed 2 src2`, `go store push` trees/go, `rs store pull --local rsL2 trees/go`; root equal; `storecmp` | root + effect |
| B4 | pull the same ref into fresh stores with both clients: `pulled …` lines identical (records are served verbatim, so counts and bytes match) | exact stdout |
| B5 | re-push the same name without `--expected-version`: both print `dstore: cas mismatch: current key <hex> (pull first, or --force)` exit 1; with `--expected-version <from ref get>` both succeed; `--force` succeeds | exact stderr+exit |
| B6 (tz) | `refs`, `refs trees/` go vs rs | exact |
| B7 (tz) | `ref get trees/rs` go vs rs | exact |
| B8 | `ls trees/rs`, `ls trees/rs sub`, `ls trees/rs /`, `cat trees/rs hello.txt`, `cat trees/rs big.bin` (5 MiB, `cmp`), `cat trees/rs sub` (`not a regular file with content`), `cat trees/rs nope` | exact (bytes for cat) |
| B9 | `ref delete trees/tmp` (default force): `refs` lacks it; `ref delete --expected-version 00 trees/rs2` CAS error go vs rs | exact |
| B10 | packstore cross-write: copy `rsL/packstore` to `x/packstore` (no `refs/`); `go store push --local x src1 trees/x` reports `built <root>: 0 new objects` (Go dedups against Rust segments); the mirror image with Rust on a Go-written copy | format |
| B11 | large tree `mktree -seed 3 -files 2000 -bytes 64Mi`; `rs store push --jobs 2`; `go store pull`; root equal; and the reverse | root |
| B12 | interrupt: SIGINT to `rs store push` after the first `uploading` log line; exit 1 with last stderr line `dstore: context canceled` (compare with the Go run of the same); re-run succeeds with `U` < first-run total | stdout+exit |
| B13 | id-only tickets (`INTEROP_MDNS`): `IDS=$(go cluster ticket --ids)`; `rs refs --ticket "$IDS"` and `go refs --ticket "$IDS"` identical and include `trees/rs`. `auto` mode: if the Go run fails too, report SKIP (no mDNS on this host) | exact / skip |
| B14 | `DSTORE_TICKET=garbage rs refs --ticket "$T"` works (flag wins); `rs refs` with `DSTORE_TICKET=garbage` gives the same parse error as Go | exact |
| **C: watch** | | |
| C1 (tz) | before B1, start `go watch 'trees/**'` and `rs watch 'trees/**'` in the background; after B/D, SIGINT both; exit 0; same multiset of lines, same per-name order, lines match `^[^\t]+\t(deleted|[0-9a-f]{64}\t\S+\t.*)$` | normalized |
| C2 | `rs watch 'trees/**'` started after refs exist prints every existing ref first (initial difference) identically to Go | exact (sorted) |
| C3 (chaos) | kill n2's `node join`/`serve` process; the Rust watch reconnects; a push made while n2 is down appears; restart n2 with `go serve --store n2 --no-relay --loopback` | effect |
| **D: working copies across implementations** | | |
| D1 | `go clone trees/rs wcA` and `rs clone trees/rs wcB`: stdout identical after substituting the dir; `treekey -exclude .dstore wcA` = `… wcB` = root; `diff -r` equal (excluding `.dstore`) | exact + root |
| D2 | `.dstore/config` byte-identical wcA vs wcB; `.dstore/state` identical after normalizing `synced_at` | exact |
| D3 | `rs status` in wcA (Go-created) = `go status` in wcA = `reference trees/rs, synced to <16hex>` / `remote: up to date` / `nothing to push` | exact |
| D4 | in wcA: modify, chmod +x, delete, add file, add dir with file, retarget symlink, file→dir, dir→file, touch only, `user.wc` xattr (best effort), add `.amberignore` hiding a base path. Then `status`, `diff`, `diff --stat`, `diff sub`, `diff ../outside` (`… is outside the working copy`) go vs rs | exact |
| D5 | `rs push --user interop` in wcA: `pushed trees/rs: root …`; then `go status` clean; `go push` and `rs push` both print `nothing to push` | exact |
| D6 | `go fetch` in wcB: `fetched trees/rs: root …`; `rs status`, `rs diff --incoming`, `rs diff --remote` vs Go in wcB | exact |
| D7 | conflict: edit `f.txt` in wcB (Rust side) and push a different `f.txt` from wcA (Go). `rs push` in wcB (not fetched) prints `dstore: reference changed on the cluster since your last fetch: pull first, or --force (cas mismatch: current key <hex>)`. `rs pull` exits 1 with stderr `conflicts:` / `  f.txt (local: modified, cluster: modified)` / `dstore: conflicting changes: resolve them, or --force to take the cluster's side`. `rs pull --force`: `pulled: N paths updated, 1 conflicts taken from the cluster`. Each step is compared with Go on a twin copy made by `cp -Rp wcB wcB-go` beforehand | exact |
| D8 | `rs init trees/new` in an existing dir: `initialised working copy of trees/new; the reference does not exist yet: push creates it`; `go status` = `rs status` (all `new`, remote absent); `go push`; `rs fetch` prints `trees/new: up to date (<16hex>)` | exact |
| D9 | lost state: save `.dstore/state`, `rs push` a change, restore the saved state; `go push` prints `trees/new already holds <16hex> (an earlier push completed); state updated`, and the mirror image with Rust | exact |
| D10 | remove `.dstore/state`: both `status` print `dstore: incomplete clone: delete the directory and clone again` | exact |
| D11 | `clone` into a non-empty dir (`dstore: <dir> is not empty`), onto a file (`… is not a directory`), unknown ref (`dstore: client: unknown reference: trees/none`, dir removed afterwards); `init` inside wcA (`<abs> is inside the working copy at <root>`) | exact (paths normalized) |
| D12 | lock interop: `holdlock wcA 10 &` then `rs status` in wcA fails (exit 1); the mirror image with a Rust holder (`examples/holdlock.rs`) and `go status` | exit only (texts from core vs core-rs, §7) |
| D13 | stored-ticket refresh: after A12/A13 changes the view, `rs fetch` rewrites `.dstore/config` exactly as `go fetch` does in a twin copy | exact |
| **E: CLI behaviour** | | |
| E1 | SIGINT and SIGTERM to `watch` exit 0 (both clients) | exit |
| E2 | `DSTORE_LOG_LEVEL=warn`: no `level=INFO` lines on stderr for push (both) | effect |
| E3 | Go-iroh ping over the client ALPN: a Rust test-only example sends `TPing` to n1 and gets `TPong` stamped with the view's (incarnation, epoch) | effect |
| **G: documented exceptions (assert the failure, don't skip)** | | |
| G1 | `rs store pull --local goL …` on a Go-written `goL/refs` (Pebble) fails; `go store pull --local rsL …` on Rust `refs` (redb) fails | exit only |
| G2 | `rs cluster ticket --store n1`, `rs cluster status --store n1`, `rs catalog restore --store n1 FILE`: the fixed Rust error chosen in §8, exit 1 | exact (Rust text) |

### 4.6 In-process Rust tests over a fake node (c)

**Transport.** Port `transport/mem.go` as
`dstore_client::transport::mem::{Network, MemEndpoint}` (public, used by
tests).

- `Network::{bind(id, alpns), set_down(id, bool), partition(a, b, bool), set_delay(d)}`.
- A dial checks down/cut/bound/ALPN, with the exact texts
  `mem: local endpoint is down`, `mem: %s unreachable`, `mem: %s not bound`,
  `mem: %s does not speak %s` (mem.go:80-120).
- `path()` returns `{direct: true, rtt: 1ms}` (mem.go:210).
- Each stream is a pair of pipes with a 4 MiB backpressure limit (`pipeLimit`,
  mem.go:284). `close_write` is a FIN; `cancel_read` makes the peer's write
  fail with `mem: stream reset by peer` and the local read fail with
  `mem: read canceled`; a connection close propagates to the peer.
- Implement it with tokio (`tokio::sync::Notify` + `Mutex<VecDeque<u8>>`, or
  `tokio::io::duplex` extended with independent half-close and cancel).

**FakeNode** (`src/testing/fake_node.rs`, feature `test-support`). N nodes
share one `FakeCluster` state: the view (cluster id, incarnation, epoch,
version, nodes, R=3, min_replicas=2), per-node `BTreeMap<Key, record bytes>`,
a catalog `BTreeMap<name, (record, version)>` with the version an 8-byte BE
counter (opaque to the client), pins per node, and an injection table. Each
node binds `wire::ALPN_CLIENT` on the mem network and serves one request per
stream, as `node/server.go:103-139`:

| op | minimal behaviour (Go reference) |
|---|---|
| `TView` | `TViewReply{View: encode(view), Unreachable}`, stamped (server.go:254-264, 198-204) |
| `TMissing` | >8192 keys → `TErr bad-request "too many keys"`; a key not 32 bytes → `wire: key %d has %d bytes`. Reply `TMissingReply{Keys: lacking}`; with present keys, `Short` = entries `{Key, Holders}` for keys held by fewer owners than `WriteSet` (data.go:26-44,87-149); record pins when `Pin` |
| `TGet` | `TAbsent{Keys: absent}` first, then `SendPackRecords` of the present records (data.go:193-244); injectable corrupt copy (client must skip it and ask the next owner) |
| `TPut` | epoch older than the view → `TErr stale-view "request epoch is behind"` with `View` (server.go:249-253); not writable → `no-space "node below its free-space reserve"`. Read the pack: bad pack → `bad-request "pack: …"`, >64 MiB → `bad-request "batch over 64 MiB"`. Verify each record (data.go:255-278): failure → `Rejected{Key, "verify: …"}`; not in WriteSet → `Rejected{Key, "not-owner"}`. Store locally, replicate to the other owners in shared state (skip nodes marked down), reply `TPutResult{Holders[{Key, Holders}], Failed, Rejected}`. Injection: `TErr busy` with `RetryAfter` ms; `Failed{Key, Node, Reason, RetryAfter}` |
| `TRefGet` | `validate_name` failure → `bad-request <text>`; absent → `TErr unknown-ref "no such reference"` (refs.go:52-77); else `TRef{Record, Version}` |
| `TRefPut` | decode the record (`bad-request "record: …"`), `len(key)!=32` → `"record key"`, name check (`"name: …"`), frame name mismatch (`"frame name differs from the record's"`), `key.Parse` (`"root key: …"`), stale epoch → stale-view. Completeness: every key reachable from root held by ≥ `min(minR, owners)` owners, else `TIncomplete{Keys: ≤64 sample, Shortfall}` (refs.go:125-175). Condition per `condOf` (refs.go:27-41); mismatch → `TCASMismatch{Version, HasCurrent, Record, Current key}` (43-50); else `TOK{Key, Version}` and notify watchers |
| `TRefDelete` | name check; `condOf`; absent with a condition → CAS mismatch; absent with force → `TOK` (confirm against `catalog/catalog.go` `RefDelete` when implementing); notify watchers with a deletion |
| `TRefList` | prefix/after, a configurable page limit (tests use 2) plus the 4 MiB accounting (refs.go:79-102), `Next` = last name when truncated |
| `TRefWatch` | refglob compile error → `bad-request <text>`; initial reconcile against `Refs` known (refs.go/watch.go:120-135, 201-246): `TRefChanges{Refs, Deleted}` when non-empty, then `TRefSynced`; later hints produce `TRefChanges`; a configurable rescan interval re-sends `TRefSynced`; ends when the client closes its side |
| `TStatus` | `TStatusReply{Status: encode(canned Status)}` |
| `TAdmin` | decode `AdminRequest` (`bad-request` on failure); canned `AdminReply` per op; unknown op → `bad-request "unknown admin op X"` (admin.go:260) |
| `TPing` | `TPong`, stamped |
| other | `TErr bad-request "unknown operation"` |

Injection hooks, per (node id, op, nth call): delay, close the connection
before the reply, a `TErr` with any code/text/view/retry_after, bump the
epoch (stale view), drop hints (lost-hint test).

**Tests that need the fake node** (port by name into `tests/fake_cluster.rs`):

- `node/cluster_test.go`: `TestClusterPushPull` (210), `TestClusterNodeDownDuringWrite`
  (423, client part: a push succeeds with n3 down), `TestClusterPushProgress`
  (455), `TestClusterPushPipelines` (533), `TestClusterGetYieldsBeforeEveryBatchIsFetched`
  (660), `TestClusterGetStopsEarlyCleanly` (688), `TestClusterPullWithNodeDown` (733).
- `node/watch_test.go`: `TestClusterWatchRefs` (98), `TestClusterWatchBadPattern`
  (177), `TestClusterWatchLostHint` (198), `TestClusterWatchReconnect` (222).
- `node/worktree_test.go`: `TestWorktreeInitPushCloneEditPull` (42),
  `TestWorktreeConflict` (162), `TestWorktreePushRecoversAfterLostState` (206).
- New tests for behaviour Go only exercises indirectly:
  - `Dial` tries members in order, each for 15 s, and fails with
    `client: no bootstrap node answered: …`;
  - `callRetry` makes at most 4 attempts and `anyNode` stops on the first
    remote error;
  - the backoff sequence 5, 10, 20, 40, 60, 60 s (paused clock);
  - `probeHinted` clears unreachable hints;
  - `Missing` retries the next owner after a transport failure but not
    after a remote error;
  - `putBatch` sleeps for busy `RetryAfter`, makes 4 attempts and stops the
    primary's remaining batches after an error;
  - `Push` fails with `record %x rejected: %s`, uses `directFill` in round 2,
    re-negotiates on `Incomplete` at most 3 times, then fails with
    `push: %d keys could not be placed; owners not confirming: …`;
  - `RefList` paging ends on an empty `Next` or empty `Refs`;
  - the fetcher refreshes the view once when a read order is exhausted, then
    reports the key missing;
  - `WatchRefs` handles idle reconnect, `stale-view` retry, and ends on
    `bad-request` / `unauthorized` by yielding the error;
  - `adopt` ignores older views.
- Pure tests with no fake node: `client/batch_test.go` (4 tests),
  `client/rank_test.go` (7), `cmd/dstore/size_test.go` (`TestParseSize`,
  `TestPackSizeFlag`), `wc_test.go` (`TestResolveTicket`), `tui_test.go`
  (`TestRateMeter`, `TestStatusLine`), `ticket_test.go` (2), `wire_test.go`
  (3), `placement_test.go` (7), all `worktree/*_test.go`.

**Client and node tests left to the live harness:** `TestClusterGC`,
`TestClusterRemoveNode`, `TestClusterPutStreamsWhileReceiving`,
`TestClusterPutGivesUpASlowForward`, which test node behaviour.

### 4.7 Real-iroh Rust tests (`tests/iroh_loopback.rs`)

Port `transport/iroh_test.go` onto `dstore_client::transport::iroh::IrohEndpoint`,
built on Rust iroh 1.0.3:

- `Endpoint::builder(presets::Minimal)` (`src/endpoint.rs:952`,
  `src/endpoint/presets.rs:59`);
- `.relay_mode(RelayMode::Disabled)` (:557), `.alpns(Vec<Vec<u8>>)` (:535),
  `.bind_addr(..)` (:363), `.transport_config(QuicTransportConfig)` (:669,
  builder `src/endpoint/quic.rs:134`), `.address_lookup(..)` (:605);
- `Endpoint::connect` (:1052), `Endpoint::addr` (:1199);
- path info from `Connection::paths()` (`src/endpoint/connection.rs:1144`),
  `rtt(path_id)` (:1016), `is_ip()` / `is_relay()`
  (`src/socket/transports.rs:1081,1086`);
- mDNS through `iroh-mdns-address-lookup` 0.4.0, service name `irohv1`
  (`src/lib.rs:83`). go-iroh v0.2.0 documents `DefaultServiceName = "irohv1"`
  as "the Rust iroh-mdns-address-lookup service name"
  (`iroh/mdns/mdns_js.go:18-19`).

| Go test | Rust port |
|---|---|
| `TestIrohLoopback` (17) | ping/pong between two endpoints on 127.0.0.1, ALPN and remote id checked, path direct |
| `TestIrohDialWaitsForHandshake` (97) | a dead address fails within about `DirectTimeout` (500 ms, bound 3 s); dead+live candidates → live wins. The 0-RTT loop is Go-specific; keep only the invariants unless the Rust transport enables 0-RTT |
| `TestIrohPathRTTIsUnknownUntilSampled` (213) | the loopback RTT reported is 0 or < 50 ms, before and after one exchange |
| `TestIrohDiscoverByID` (277) | server announces over mDNS with loopback addrs; client dials by id alone; a bad address falls back to discovery; without discovery a dial by id fails within 3 s. Skip when no mDNS listener can open |

These bind UDP sockets, so they run in CI (cargo native) but not inside the
Nix sandbox.

### 4.8 Nix flake (d)

**Toolchain, confirmed.** Running
`nix eval --raw --inputs-from /Users/dragan/amber-store/core-rs nixpkgs#<attr>.version`
gives `rustc` 1.95.0, `cargo` 1.95.0, `clippy` 1.95.0, `rustfmt` 1.95.0,
`rust-analyzer` 2026-06-01 and `go` 1.26.5. That is core-rs's lock of
`nixos-26.05`: rev `445d861c6d31b4af0c79d8d4be2331f762a361d7`, narHash
`sha256-HFQhkQcl5D1hUNoen3SGHCSFCt2Bg6uP+HgbrnA3InQ=`. Start `flake.lock`
as a verbatim copy of `/Users/dragan/amber-store/core-rs/flake.lock` (same
inputs: `nixpkgs` and `systems`), so both repos pin the same nixpkgs; update
it later with `nix flake update`. iroh 1.0.3 needs `rust-version = "1.91"`,
and the local non-Nix rustc 1.86 is too old.

**core-rs git dependency.**

- `Cargo.toml`:
  `amber-store-core = { git = "https://github.com/amber-store/core-rs", rev = "a85ffa1eb5ed363b9072ab224de179196cd0a046" }`.
  The repo is PUBLIC (`gh repo view`), so cargo and CI fetch it without
  credentials.
- nixpkgs `importCargoLock` fetches git crates with
  `fetchgit { url; rev = gitParts.sha; sha256 = outputHashes."<name>-<version>"; }`
  (`pkgs/build-support/rust/import-cargo-lock.nix:173-193`). fetchgit
  defaults to `fetchSubmodules ? true` and `leaveDotGit ? null`
  (`pkgs/build-support/fetchgit/default.nix:69-73`).
- core-rs has no submodules and no `.gitattributes`, so the output equals
  `git archive a85ffa1`. Its NAR hash, computed for this spec with
  `git archive HEAD | tar -x` and `nix hash path --sri --type sha256`, is
  **`sha256-ZnXXnztVqGXq5ILG29kkJIIRo9/TTW3XqULx1chbLXA=`**. Confirm it on the
  first `nix build`: a wrong hash makes Nix print the right one. Update it
  with every rev bump.

**System dependencies.** None beyond stdenv:

- ring, zstd-sys (via core-rs `zstd`) and blake3 compile C with the stdenv cc;
- rustls uses ring, so there is no OpenSSL or pkg-config;
- on Linux, netwatch 0.19 uses the pure-Rust netlink crates;
- on macOS, netwatch pulls `objc2-core-foundation` and
  `objc2-system-configuration`, which link frameworks from the default
  apple-sdk of the darwin stdenv.

`secret-bunker-iroh/flake.nix` builds iroh 1.0.3 with no extra inputs. If
Darwin linking ever fails, add
`buildInputs = lib.optionals pkgs.stdenv.hostPlatform.isDarwin [ pkgs.libiconv ];`.

**`flake.nix`:**

```nix
{
  description = "github.com/amber-store/dstore-client-rs";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";
    systems.url = "github:nix-systems/default";
  };

  outputs = { self, nixpkgs, systems, ... }:
    let
      lib = nixpkgs.lib;
      eachSystem = f:
        lib.genAttrs (import systems) (system: f system nixpkgs.legacyPackages.${system});

      cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);

      src = lib.fileset.toSource {
        root = ./.;
        fileset = lib.fileset.unions [ ./Cargo.toml ./Cargo.lock ./src ./tests ];
      };

      rustCommon = {
        inherit src;
        version = cargoToml.package.version;
        cargoLock = {
          lockFile = ./Cargo.lock;
          # github.com/amber-store/core-rs a85ffa1eb5ed363b9072ab224de179196cd0a046 (v0.3.0)
          outputHashes."amber-store-core-0.3.0" = "sha256-ZnXXnztVqGXq5ILG29kkJIIRo9/TTW3XqULx1chbLXA=";
        };
      };
    in {
      formatter = eachSystem (system: pkgs: pkgs.writeShellApplication {
        name = "fmt";
        runtimeInputs = [ pkgs.cargo pkgs.rustfmt ];
        text = ''exec cargo fmt --all "$@"'';
      });

      packages = eachSystem (system: pkgs: rec {
        dstore = pkgs.rustPlatform.buildRustPackage (rustCommon // {
          pname = "dstore";
          cargoBuildFlags = [ "--bin" "dstore" ];
          # iroh tests bind UDP sockets, which the Darwin sandbox refuses (EPERM);
          # the socket-free suites run in checks.tests, everything runs in CI.
          doCheck = false;
          meta = {
            description = "dstore client library and CLI (Rust port of github.com/amber-store/dstore)";
            mainProgram = "dstore";
          };
        });
        default = dstore;
      });

      checks = eachSystem (system: pkgs: {
        dstore = self.packages.${system}.dstore;

        # cargo fmt would run cargo metadata and try to fetch the git dependency;
        # rustfmt on the files needs nothing.
        fmt = pkgs.runCommand "dstore-fmt-check" { nativeBuildInputs = [ pkgs.rustfmt ]; } ''
          cd ${src}
          find src tests -name '*.rs' -print0 | xargs -0 rustfmt --check --edition 2024
          touch $out
        '';

        clippy = pkgs.rustPlatform.buildRustPackage (rustCommon // {
          pname = "dstore-clippy";
          nativeBuildInputs = [ pkgs.clippy ];
          buildPhase = ''
            runHook preBuild
            cargo clippy --all-targets --all-features --offline -- -D warnings
            runHook postBuild
          '';
          doCheck = false;
          installPhase = "touch $out";
        });

        tests = pkgs.rustPlatform.buildRustPackage (rustCommon // {
          pname = "dstore-tests";
          doCheck = true;
          checkFeatures = [ "test-support" ];
          # socket-free suites only: unit, golden vectors, CLI snapshots, fake cluster
          cargoTestFlags = [ "--lib" "--test" "golden" "--test" "cli_snapshots" "--test" "fake_cluster" ];
          TZ = "UTC";
          installPhase = "touch $out";
        });
      });

      devShells = eachSystem (system: pkgs: {
        default = pkgs.mkShell {
          hardeningDisable = [ "all" ];
          # go: tools/vectorgen, the CLI snapshot generator, the interop build of Go dstore
          packages = with pkgs; [ cargo rustc rustfmt clippy rust-analyzer go gopls ];
          RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
          GOTOOLCHAIN = "local";   # go.mod says 1.26.5 = nixpkgs go; never download a toolchain
          CGO_ENABLED = "0";       # as the dstore Dockerfile; avoids the macOS Xcode cgo trap
        };
      });
    };
}
```

Notes:

- `nix flake check` builds only the host system's checks; the other systems
  are evaluated.
- The `tests` check needs `jiff` with `tzdb-bundle-always`, because the
  Linux sandbox has no `/usr/share/zoneinfo`.
- `examples/` (for instance `holdlock.rs` for D12) must be added to the
  fileset when it exists.
- `.envrc`: `use flake`.

### 4.9 GitHub Actions (e) — `.github/workflows/ci.yml`

```yaml
name: CI

on:
  push:
    branches: [main]
  pull_request:
  workflow_dispatch:

permissions:
  contents: read

concurrency:
  group: ci-${{ github.ref }}
  cancel-in-progress: true

env:
  CARGO_TERM_COLOR: always
  RUST_TOOLCHAIN: "1.95.0"   # = nixpkgs nixos-26.05 rustc
  DSTORE_GO_REF: v0.1.9

jobs:
  rust:
    strategy:
      fail-fast: false
      matrix:
        os: [ubuntu-latest, macos-latest]
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v5
      - uses: dtolnay/rust-toolchain@master
        with:
          toolchain: ${{ env.RUST_TOOLCHAIN }}
          components: rustfmt, clippy
      - uses: Swatinem/rust-cache@v2
      - run: cargo fmt --all --check
      - run: cargo clippy --all-targets --all-features --locked -- -D warnings
      - run: cargo build --all-targets --all-features --locked
      - name: Tests (unit, golden, CLI snapshots, fake cluster, real iroh loopback)
        run: cargo test --all-features --locked
        env:
          TZ: UTC

  vectors:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v5
      - uses: actions/setup-go@v5
        with:
          go-version-file: tools/vectorgen/go.mod
          cache-dependency-path: tools/vectorgen/go.sum
      - name: go vet
        working-directory: tools/vectorgen
        run: go vet ./...
      - name: Regenerate twice; must be deterministic and match the committed vectors
        working-directory: tools/vectorgen
        run: |
          go run . "$RUNNER_TEMP/golden-a"
          go run . "$RUNNER_TEMP/golden-b"
          diff -r "$RUNNER_TEMP/golden-a" "$RUNNER_TEMP/golden-b"
          diff -r --exclude=cli "$RUNNER_TEMP/golden-a" ../../tests/golden
      - name: CLI snapshots from the Go binary (built into a temp GOBIN and removed)
        working-directory: tools/vectorgen
        env:
          CGO_ENABLED: "0"
        run: |
          go run ./cmd/clisnap -o "$RUNNER_TEMP/cli"
          diff -r "$RUNNER_TEMP/cli" ../../tests/golden/cli

  interop:
    runs-on: ubuntu-latest
    timeout-minutes: 45
    steps:
      - uses: actions/checkout@v5
      - uses: actions/checkout@v5
        with:
          repository: amber-store/dstore
          ref: ${{ env.DSTORE_GO_REF }}
          path: dstore-go
      - uses: dtolnay/rust-toolchain@master
        with:
          toolchain: ${{ env.RUST_TOOLCHAIN }}
      - uses: Swatinem/rust-cache@v2
      - uses: actions/setup-go@v5
        with:
          go-version-file: dstore-go/go.mod
          cache-dependency-path: |
            dstore-go/go.sum
            tools/vectorgen/go.sum
      - run: cargo build --release --locked --all-features --bin dstore --examples
      - name: Live Go <-> Rust interop over a 3-node loopback cluster
        run: bash interop/check.sh
        env:
          DSTORE_GO_REPO: ${{ github.workspace }}/dstore-go
          DSTORE_RS_BIN: ${{ github.workspace }}/target/release/dstore
          INTEROP_MDNS: auto
          INTEROP_LOG_DIR: ${{ runner.temp }}/interop-logs
      - if: failure()
        uses: actions/upload-artifact@v4
        with:
          name: interop-logs
          path: ${{ runner.temp }}/interop-logs

  nix:
    strategy:
      fail-fast: false
      matrix:
        os: [ubuntu-latest, macos-latest]
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v5
      - uses: DeterminateSystems/nix-installer-action@main
      - run: nix flake check -L
      - run: nix build .#dstore -L && ./result/bin/dstore --version
```

The `vectors` job needs network access for Go modules; the committed
`go.sum` must be the online one (§4.2 item 3). The interop job builds the Go
binary inside `interop/check.sh` into `mktemp -d` and deletes it. Heavy and
chaos groups run on `workflow_dispatch`, or with a label, by setting
`INTEROP_HEAVY=1 INTEROP_CHAOS=1`.

---

## 5. Golden vectors a Go generator must emit (concrete cases)

Fixtures used below: `cid16 = 00..0f`; `id(s)` = ed25519 public key from
`NewKeyFromSeed(data(s,32))`; `k(s)` = `key.New(Blob, n, data(s,n))`
(canonical keys); `rec(s,n)` = raw record of `data(s,n)` (flag 0).

**`wire/frames.json`**: one case per line; every request stamped with
(cid16, inc 1, epoch 7) unless noted.

- Requests:
  - `view-unstamped` (`a1001820`), `view-stamped`;
  - `missing` with 1 key, pin true; `missing-8192` (8192 keys, pin false;
    only `frame_blake3` + length);
  - `get` with 3 keys; `put-header`;
  - `ref-get` name `trees/demo`;
  - `ref-put` variants: `new` (HasExpected, no Expected*), `versioned`
    (ExpectedVersion 8 bytes), `keyed-new` (Keyed, ExpectedOld nil: the Go
    client never sends this, the node's `condOf` treats it as versioned),
    `keyed` (ExpectedOld 32 bytes), `force`;
  - `ref-delete` variants: `force`, `expected-version`;
  - `ref-list` variants: `prefix`, `prefix+after`, `empty-prefix`
    (`Prefix []byte("")` is omitted);
  - `ref-watch` variants: `no-known` (Refs nil), `one-known`
    (`Refs[{Name, Key}]`, Version nil encodes `f6`, CreatedAt `00`),
    `unicode-pattern`;
  - `status`; `admin` (Params = `AdminRequest{Op:"gc-status"}`); `ping`.
- Replies, stamped with (inc 1, epoch 7):
  - `view-reply` (View + Unreachable 2 ids);
  - `missing-reply` (lacking 2 + Short 1 with 2 holders; Short with holders
    nil);
  - `absent` (Keys nil → key 4 omitted; with 2 keys);
  - `data` (9 bytes) and `data-end`;
  - `put-result` (Rejected verify/not-owner, Holders 2 keys × 3 ids, Failed
    with RetryAfter 250 and without);
  - `ref`; `ok-refput` (Key + Version); `ok-bare`;
  - `cas-mismatch` variants: `current` (HasCurrent, Current 32, Record,
    Version) and `absent` (Version only);
  - `incomplete` (Keys 64, Shortfall 1000);
  - `refs` (3 RefInfo, one with User "", Next "trees/c"; final page without
    Next);
  - `status-reply`; `admin-reply`;
  - `ref-changes` (Refs 2 + Deleted 1); `ref-synced`; `pong`;
  - `err` for every code of `wire.go:107-129`, with and without Text,
    stale-view with View, busy with RetryAfter 1500.

**`wire/decode.json`**: the 14 probe cases of §2.1, plus:

- `null-into-bytes` (`{0:52, 7:null}` ok);
- `null-into-string`;
- `nested-indefinite-array` (`Keys` as `9f 5820… ff`);
- `big-uint-epoch` (`1b ffffffffffffffff`);
- `epoch-overflow-int` (Type as `1b 0000000100000000` into Go `int`, 64-bit:
  ok);
- `unknown-nested-key-in-RefInfo`;
- `float16-garbage-in-admin` (`AdminRequest` 0.5);
- `text-key` (`{"a":1, 0:53}`: record ok or ERR, whichever Go does);
- `map-key-uint-nonshortest` (`18 00` as key 0).

**`wire/frame_errors.json`**: `00000000` (empty payload, decode EOF),
`01000001…` (too large), `000000`
(unexpected EOF), `00000004a100` (short), clean EOF (empty stream), two
frames back to back.

**`wire/pack_frames.json`**:

- `empty-pack` (one TData of 9 bytes, then TDataEnd);
- `one-small`;
- `exactly-chunk`: records chosen so the pack is exactly `1<<20` bytes →
  one full TData + TDataEnd, no empty frame;
- `chunk-plus-one`;
- `3-mib` (several records across chunks);
- `record-larger-than-chunk` (a 2.5 MiB blob);
- `terr-mid-pack` (TData, TErr busy): the reader error text
  `remote: busy: ` (protocol.RemoteError).

**`ticket/encode.json`**:

- `probe` (§3.2);
- `no-addrs` (member map has 1 key);
- `relay-addr` (`relay:https://relay.example/`);
- `four-members` (the TicketFromView cap);
- `incarnation-0`;
- `incarnation-max` (`18446744073709551615`);
- `empty-cluster-id` (`40`);
- `nil-members` (`f6`; Parse gives `ticket: no members`);
- `short-id` (IDs() skips it);
- `duplicate-ids` (IDs() dedups);
- `unicode-addr`.

**`ticket/parse.json`** (exact error texts):

- empty `""` and `"   "` → `ticket: empty`;
- `nope` → `… invalid length`;
- `dstore1` → the Go text for a CBOR EOF;
- `dstore1!!!` → `ticket: illegal base32 data at input byte 0`;
- upper-case ticket; ticket with surrounding whitespace; ticket with an
  embedded `\r\n`; trailing-bit flips 1 and 2 (ok);
- ticket CBOR with an unknown key 3 (ok);
- `hex(id(1))`; `hex1,hex2`; `hex1 hex2`; `" hex1,\nb32(id2) "`;
  upper-case hex;
- `hex(07×32)` → `data is not a valid public key`;
- 63 hex chars; `hex1,zz`; `ab×32` (64 chars, invalid point);
- base32 with trailing bits flipped (z-base-32 hint text);
- z-base-32 form of `id(1)`;
- NBSP-separated; `;`-separated → `failed to decode base32 string`;
- `hex1,hex2,hex1` (IDs() → `hex1,hex2`).

**`view/views.json`**:

- `probe` (§3.2);
- `nil-voters-nodes` (`f6`);
- `pending-with-ramp` (all `Pending` keys 0-12);
- `former-fenced-acl`;
- `data-endpoints`;
- `deferred-voters`, `remove-voters`, `ramps`;
- `voter-sync-cursor+target+add`;
- `recovered`, `rebalance-pause`, `rate-cap`;
- `zone-set`;
- `writable-false` (`f4`);
- `decode-only-unknown-keys`.

**`view/placement_ops.json`**, three views:

- (i) 3 nodes, weight 100, R 3, minR 2;
- (ii) 5 nodes with zones a/a/b/b/"" and one weight 0, R 3;
- (iii) (i) plus `Pending` over 4 nodes, R 2.

For each, 64 keys `k(1000+i)` plus keys with tails `00…`, `ff…` and
`2^44`: slot, owners, write_set, read_order, pending_owners.

**`placement/placement.json`**:

- `fmix64`: 0, 1 → `0xb456bcfc34c2cb2c`, `0xdeadbeef` → `0xd24bd59f862a1dac`,
  max → `0x64b5720b4b825f21`, plus 16 splitmix values;
- `log2fix`: 1, 2^i for i in 0..63, 3, max, 1000 splitmix values;
- `salts`: ids `01×32`..`05×32` and `id(1..8)`;
- `l`: `(12345, salt(01…))` = `0x22356754f`, plus 32 splitmix pairs, plus a
  case with `fmix64 = 2^64-1` (L = 0) found by search;
- `sets`: the frozen 5-member set (§2.3), the zone set of
  `placement_test.go:200-222`, a 50-node splitmix-weight set, and a set of
  equal weights and equal L (tie by id); each with slots
  `[0,1,12345,0xfffff,524288,777777]` plus `all_slots_blake3`.

**`client/rank.json`**:

- the 7 cases of `rank_test.go`;
- RTT boundaries 4.999ms/5ms/24.999ms/25ms/99.999ms/100ms;
- penalties 1 (unreachable hint) and 3 (both);
- relayed and unmeasured;
- `batches`: the 4 cases of `batch_test.go`, plus a zero-size record, plus
  maxKeys 1;
- `placed`: holders empty, holders = owners, holders at minR, 2 owners in a
  pending transition.

**`admin/requests.json`**: argv per CLI subcommand:

- `token create --weight 50`;
- `cluster ticket`;
- `cluster replicas 2 --yes`;
- `node remove <id> --dead --allow-unsafe`;
- `node drain`; `node weight <id> 300`; `node zone <id> rack-1`;
  `node repair`;
- `voter add`; `voter remove --allow-unsafe`;
- `transition status|abort|refreeze|pause|resume`;
- `gc run`; `gc run --tolerate-missing`; `gc run --garbage 0.5`;
  `gc run --garbage 0.3`;
- `gc status`; `gc hold` (Pause true); `gc release` (Pause false omitted);
  `gc why <64hex>`;
- `catalog backup`; `catalog backups`.

**`admin/replies.json`**: `Text` only; `Names` 3; `Key` 32 → `key %x`;
`Key` 31 (not printed); Text + Names + Key; `Ticket` (used by
`cluster ticket`).

**`status/status.json`**: `probe`; every omitempty field set (Amnesiac,
Retired, IsHolder, Unreachable, Voters 2, GC text, Transition text,
LeaseHolder, ScrubAgeSec, LastLive, Corrupt, UnauditedKeys, Watchers); zero
status.

**`worktree/config.json`** encode: probe; all fields set (Relay,
NoDiscovery); `&<>` and U+2028 in name; invalid UTF-8 user; empty config
(ticket and name present, others omitted). Decode: `{"Ticket":…,"NAME":…}`
(case-insensitive), unknown key, duplicate key (last wins in Go), `null`
fields, trailing garbage → exact error text.

**`worktree/state.json`** encode: no remote (`"remote": ""`,
`"remote_version": ""`); remote with empty version; synced_at whole seconds;
5 ns; pre-1970. Decode: missing remote_version with a remote;
`remote_version` odd length; `base` invalid hex; `base` 31 bytes (key.Parse
text); synced_at with `+02:00` offset; synced_at missing → exact texts
(`bad state file: …`).

**`worktree/trees` and `diff_trees.json` / `unified.json`**: trees built with
builders:

- `A`: `edit.txt` "one\ntwo\nthree\n", `bin` "a\x00b", `gone.txt`, `run.sh`
  0o644, `sub/x`, `link`→`t1`, `flip/inner`, `olddir/x`, `dev` chardev [1,3],
  `fifo`, `xin` with inline xattrs, `xsp` with spilled xattrs, 5 MiB `big`
  (splitmix);
- `B`: the edits of `diff_test.go:15-41` and `change_test.go:55-119`, plus
  a `dev` rdev change → modified, `fifo`→symlink (type), xattr change →
  meta, `big` changed at byte 4 MiB → oversized, so `Binary files … differ`
  (MaxDiffBytes boundary: a second file of exactly 16 MiB is diffed as
  text).

Pairs (A,B), (B,A), (empty,A) (clone diff), (A,A) (nil).
`get_calls` for the pruning case of `change_test.go:121-145` = 2.

**`worktree/merge.json`**: the 13 cases of `merge_test.go:59-83`, plus an
incoming ModeChanged over local MetaChanged (apply) and equivalent retypes on
both sides (apply).

**`udiff/udiff.json`**:

- the 3 probe cases;
- identical (empty output);
- old empty / new empty;
- no trailing newline only in old, only in new;
- two changes 7 lines apart (two hunks) and 6 lines apart (one hunk);
- a change at line 1 and at the last line;
- CRLF lines;
- 100 repeated `"a\n"` with one inserted `"b\n"` in the middle (tie-breaking);
- 2000-line splitmix hex lines with 40 scattered edits;
- unicode lines;
- a line with `\r` only.

**`text/formats.json`**:

- `human_bytes`: §2.4 plus 1<<60, 1<<50−1, 1048576×1.25 (1.2 or 1.3 per Go);
- `rate`: took 0, took 1ns, 3 MiB/2 s;
- `duration`: §2.4 plus 59.5s, 1h0m0.5s, −1s;
- `rfc3339`: the probe zones × {−1, 0, 1700000000123456789,
  1719792000000000000, 253402300799000000000 (year 9999)} plus
  Australia/Lord_Howe and Pacific/Chatham;
- `rfc3339nano_utc`: §2.4;
- `go_quote`: §2.4 plus U+FFFD, U+00A0, `"` and `\`;
- `status_line` (verbatim copy): the `tui_test.go:99-106` cases plus bytes
  only, rate 0, eta rounding, overshoot;
- `describe_change` (copy): dir added `sub/`, type change
  `a.txt (file → directory)`, mode `run.sh (0644 → 0755)`, deleted dir with
  trailing `/`;
- `type_name`: each S_IFMT plus `0` → `type 0` (Go `%#o` of 0 is `0`).

**`cli/size.json`**: the good and bad lists of `size_test.go:9-37` with exact
errors, plus ` 1 Gi` (unknown unit `" Gi"`), `8Ti` ok, `8388607Ti` ok,
`8388608Ti` → `too large`, `+1`, `1KiBB`. `pack_size`: default, flag
`512Mi`, env `1Gi`, env empty → default, `0` →
`--pack-size: 0 is not a positive size`, `x`, `1.5Gi`.

**`errors/text.json`**: §4.3 item 22.

**`refglob/refglob.json`**: `refglob_test.go` `TestMatch` (5), `TestPrefix`
(61), `TestInvalid` (86), with Go error texts.

**`cli/snapshots.json`** (from the Go binary):

- `dstore`; `dstore -h`; `dstore --version`; `dstore -v`; `dstore h`;
- `help <cmd>` and `<cmd> --help` for all 21 commands; `<cmd> <sub> --help`
  for every subcommand; `dstore cluster` (no subcommand); `dstore h cluster`;
- every offline error of §2.5, plus:
  - `store pull` (required flag);
  - `store push --local {CWD}/x a` → `push PATH NAME`;
  - `store push --local {CWD}/x src 'bad@@'` → the ValidateName text;
  - `store pull --local {CWD}/x` → `pull NAME`;
  - `cat a` → `cat NAME PATH`; `init` → `init NAME`;
  - `init 'bad@@'`;
  - `node join --store s` → `Required flags "seed, token" not set`;
  - `node weight <valid64hex> x` → `weight ID GiB`;
  - `refs --ticket` without a value → `flag needs an argument: -ticket`;
  - `refs --ticket '   '` → `ticket: empty`;
  - `bogus` (exit 3); `refs --bogus`;
  - `cluster replicas 2` with stdin `n\n` → prompt on stdout,
    `dstore: aborted`;
  - `diff` in an empty dir; `status` in `wc-no-config` →
    `dstore: working copy {CWD}: open {CWD}/.dstore/config: no such file or directory`;
  - `status` in `wc-no-state` → `dstore: incomplete clone: delete the directory and clone again`;
  - `status` in `wc-bad-state-key` → `dstore: bad state file: base: encoding/hex: invalid byte: U+007A 'z'`;
  - `diff --incoming` in a wc without a remote →
    `dstore: the reference does not exist on the cluster: nothing to pull`
    (the fixture state has `remote ""`);
  - `--log-level bogus refs` (level falls back to info; same error as
    `refs`).

---

## 6. Go tests worth porting (by name)

| Go test | file:line | Rust target | needs |
|---|---|---|---|
| `TestBatchesBalancesBySizerNotByKeyLength`, `TestBatchesCapsKeysPerBatch`, `TestBatchesSendsAnOversizedRecordAlone`, `TestBatchesKeepsOrder` | client/batch_test.go:13,21,27,35 | unit tests in `client::batch` | nothing |
| `TestRankOwnersKeepsRankAmongUnmeasuredOwners`, `…DoesNotDemoteUnmeasuredOwnersBehindAMeasuredOne`, `…PrefersDirectOverRelayed`, `…PrefersUnmeasuredOverRelayed`, `…TiesNearRoundTripsByRank`, `…PrefersAMuchNearerOwner`, `…PutsPenalisedOwnersLast` | client/rank_test.go:29,36,47,59,68,82,94 | unit tests in `client::rank` | nothing |
| `TestParseSize`, `TestPackSizeFlag` | cmd/dstore/size_test.go:9,39 | `cli::size` unit + `cli/size.json` | nothing |
| `TestResolveTicket` | cmd/dstore/wc_test.go:5 | `cli::wc` unit | nothing |
| `TestRateMeter`, `TestStatusLine` | cmd/dstore/tui_test.go:65,99 | `cli::progress` unit | nothing |
| `TestUIModel`, `TestTeaHandler` | tui_test.go:15,84 | only if the TUI keeps the same strings (§8) | ratatui test backend |
| `TestRoundTrip`, `TestParseIDs` | ticket/ticket_test.go:13,43 | `ticket` unit + `ticket/*.json` | nothing |
| `TestFrameRoundTrip`, `TestErrorFrames`, `TestPackFramesInterop` | wire/wire_test.go:12,31,50 | `wire` unit + `wire/*.json` | core-rs amberpack |
| `TestLog2FixExact`, `TestFmix64Vectors`, `TestGoldenVectors`, `TestWeightedDistribution`, `TestAddingNodeMovesOnlyToIt`, `TestZoneRule`, `TestSlot` | placement/placement_test.go:19,43,60,132,158,200,224 | `placement` unit + `placement.json` | nothing |
| `TestMatch`, `TestPrefix`, `TestInvalid` | refglob/refglob_test.go:5,61,86 | fake node's refglob port | nothing |
| `TestCreateOpenRoundTrip`, `TestFindOutsideWorkingCopy`, `TestRemove` | worktree/tree_test.go:11,69,75 | `worktree::tree` unit | tempfile |
| `TestDiffTrees_EveryKind`, `TestDiffTrees_PrunesEqualSubtrees`, `TestCompare` | worktree/change_test.go:55,121,147 | `worktree::change` unit | core-rs ingest + packstore |
| `TestScan_CleanTreeHasNoChanges`, `TestScan_EveryKind`, `TestScan_IgnoredBasePathIsDeleted`, `TestScan_RacyMtime`, `TestScan_TypeChangeExpands`, `TestScan_Xattr` | worktree/scan_test.go:38,46,90,100,123,143 | `worktree::scan` unit | core-rs ingest, xattr |
| `TestMerge` | worktree/merge_test.go:50 | `worktree::merge` unit + `merge.json` | nothing |
| `TestUnified_TreeToTree`, `TestUnified_TreeToDisk`, `TestStat` | worktree/diff_test.go:43,72,87 | `worktree::diff` unit + `unified.json` | core-rs ingest |
| `TestApply_CloneThenUpdateReproducesTrees`, `TestApply_KeepsNonEmptyDirectoryOnDelete`, `TestApply_RefusesUnsafePaths`, `TestApply_RefusesSymlinkedAncestor` | worktree/apply_test.go:74,120,150,160 | `worktree::apply` unit | rustix mkfifo, core-rs ingest |
| `TestIrohLoopback`, `TestIrohDialWaitsForHandshake`, `TestIrohPathRTTIsUnknownUntilSampled`, `TestIrohDiscoverByID` | transport/iroh_test.go:17,97,213,277 | `tests/iroh_loopback.rs` (§4.7) | real UDP, mDNS |
| `TestClusterPushPull`, `TestClusterNodeDownDuringWrite` (client part), `TestClusterPushProgress`, `TestClusterPushPipelines`, `TestClusterGetYieldsBeforeEveryBatchIsFetched`, `TestClusterGetStopsEarlyCleanly`, `TestClusterPullWithNodeDown` | node/cluster_test.go:210,423,455,533,660,688,733 | `tests/fake_cluster.rs` | FakeNode + mem |
| `TestClusterWatchRefs`, `TestClusterWatchBadPattern`, `TestClusterWatchLostHint`, `TestClusterWatchReconnect` | node/watch_test.go:98,177,198,222 | `tests/fake_cluster.rs` | FakeNode + mem |
| `TestWorktreeInitPushCloneEditPull`, `TestWorktreeConflict`, `TestWorktreePushRecoversAfterLostState` | node/worktree_test.go:42,162,206 | `tests/fake_cluster.rs` | FakeNode + mem + core-rs |
| `TestClusterGC`, `TestClusterRemoveNode`, `TestClusterPutStreamsWhileReceiving`, `TestClusterPutGivesUpASlowForward` | node/cluster_test.go:294,379,559,767 | not ported; node behaviour, covered by interop A6/A12 | Go nodes |
| `scripts/e2e-loopback.sh` | whole file | `interop/check.sh` cluster bootstrap + B1-B13, C1, A-group | Go binary |

Harness helpers worth copying from Go tests: `makeTree` (cluster_test.go:183-208) becomes `cmd/mktree` (deterministic instead of `crypto/rand`); `pushTree` (640-658); `waitFor` (103-115, as a tokio poll helper); `startWatch`, `until`, `synced`, `change` (watch_test.go:24-67).

---

## 7. Gaps in core-rs or Rust iroh relative to what the Go code uses

| # | gap | effect on verification | workaround |
|---|---|---|---|
| 1 | `refstore` is redb in core-rs, Pebble in Go (core-rs PORTING.md "Different, by design") | `store push/pull --local DIR` directories cannot be shared between implementations: the `refs/` DB cannot be opened by the other one | separate `--local` dirs; cross-check objects by copying only `packstore/` (B10) and with `storecmp` (B2/B3); assert the failure as a documented exception (G1) |
| 2 | no Pebble meta or paxos acceptor in Rust (`node.OpenOffline`) | `cluster status --store`, `cluster ticket --store`, `catalog restore`, and all node roles are unavailable | the harness always uses Go for node roles; the Rust CLI prints a fixed error (G2, decision §8) |
| 3 | core-rs error texts differ from Go core's (packstore flock/open, ingest walk, `fstree` resolve errors, `key.Parse`) | stderr of `ls NAME missing`, `cat NAME missing`, `store push` on a bad path, wc lock conflicts can differ | B8 and A11 compare exact texts; the Rust CLI maps core-rs errors to Go's texts where they reach users (catalogue them from interop failures); D12 compares exit codes only |
| 4 | zstd frames differ (klauspost vs libzstd; core-rs PORTING.md "Interoperable but not byte-identical") | `built … N new objects`, `U uploaded`, progress `TotalBytes` and `pulled … (B bytes)` depend on which implementation first stored an object | compare byte counts only between pulls of the **same** pushed ref (B4), never across two pushes of the same tree; compare roots, not sizes |
| 5 | core-rs has no placement vectors (architecture §13 M1 says they are "shared with core-rs"; `grep placement` in core-rs finds only binaryfuse and repair) | nothing locks Rust placement | dstore-client-rs owns `placement/placement.json` with `all_slots_blake3` |
| 6 | core-rs `cbor` is hand-rolled for core objects only; there is no general canonical codec with fxamacker's decode leniency | every frame, view, status and admin blob must be canonical and must accept Go-accepted input | a dstore `codec` module (ciborium 0.2.2 `Value` decode + a canonical encoder with shortest float16/32/64); `wire/decode.json` locks accept/reject, `admin` 0.5/0.3 locks floats, nil→`f6` cases lock nulls |
| 7 | mDNS interop between go-iroh v0.2.0 and `iroh-mdns-address-lookup` 0.4.0 is not in go-iroh `COMPATIBILITY.md` (which covers handshake, relay, DNS/pkarr, 0-RTT and vectors against iroh 1.0.3); both use service `irohv1` | id-only tickets (`--ticket <ids>`) with Rust clients against Go nodes are unproven | B13 is the evidence; run with `INTEROP_MDNS=require` on a dev machine before a release |
| 8 | path info API differs: go-iroh `Paths()` `Selected/Relayed/HasRTT` vs Rust `Connection::paths()`, `rtt(path_id)`, `is_relay()` | `rankOwners` inputs from real connections could differ (a never-sampled RTT must be 0, iroh.go:430-444) | rank logic tested over mem paths and `client/rank.json`; `TestIrohPathRTTIsUnknownUntilSampled` port asserts RTT is 0 or < 50 ms on loopback |
| 9 | go-iroh exposes `HandshakeComplete()`/`Used0RTT()`; Rust iroh `connect` has different 0-RTT semantics | `TestIrohDialWaitsForHandshake` cannot be ported literally | keep the invariants (a dead address fails within the timeout, live candidate wins); drop the 0-RTT loop |
| 10 | go-iroh `key.ParseEndpointID` texts and z-base-32 detection (key.go:34-40, 229-238) | ticket error lines on stderr | reproduce the texts in the Rust ticket parser; `ticket/parse.json` locks them |
| 11 | `data_encoding::BASE32_NOPAD` rejects non-canonical trailing bits and newlines | Go accepts tickets that Rust would reject | custom `Specification` (`check_trailing_bits = false`, lowercase translation, `ignore "\r\n"`); vectors lock it |
| 12 | no Rust port of go-udiff's LCS (`lcs/`, `unified.go`) | `diff` output hunks could differ (similar 2.7.0 uses another algorithm) | port go-udiff v0.4.1 verbatim; `udiff/udiff.json` + `worktree/unified.json` |
| 13 | Go `encoding/json` escaping (`& < >    `, `�` for invalid UTF-8) and case-insensitive decode | `.dstore/config`/`state` bytes shared by both implementations could differ | hand-written JSON writer + tolerant reader; `worktree/config.json`, `state.json` |
| 14 | Go local-zone `RFC3339` formatting and floor division for negative ns | `refs`, `watch`, `ref get` lines | jiff 0.2.35 with the system zone from `TZ` / `/etc/localtime`; `text/formats.json`; TZ matrix in the harness |
| 15 | urfave/cli v2 help templates, `Incorrect Usage` output, exit code 3 for unknown commands, required-flag texts | CLI surface | hand-rendered help and errors (not clap's); `cli/snapshots.json` locks them |
| 16 | bubbletea/lipgloss rendering is not reproducible with ratatui | TUI bytes cannot match | verify plain mode only (`--no-tui` / non-tty); TUI tests limited to model logic |
| 17 | slog `TextHandler` stderr lines include timestamps and RTTs | stderr not byte-comparable | the harness compares only the final `dstore: …` line and exit codes; optionally a normalizer for `msg=` keys |

---

## 8. Risks and open decisions

### Risks

- **`scripts/e2e-loopback.sh` is stale at v0.1.9.** Lines 42 and 46 call
  `dstore push --local …` / `dstore pull --local …`; since the working-copy
  PR those are `store push` / `store pull`, and the wc `push` has no
  `--local`, so it fails with `flag provided but not defined: -local`. The
  harness must use `store push/pull` and cannot reuse the script as is.
- **mDNS on GitHub runners** may be unavailable; `INTEROP_MDNS=auto` skips
  B13 when Go also fails, which can hide Rust regressions.
- **Cluster formation time.** A join is a voter change plus a transition
  ("a minute or two", README). The poll allows 300 s. Slow runners can flake;
  keep the 45-minute job timeout and upload logs.
- **Normalized comparisons** (`cluster status` counters, `gc` text) can mask
  real differences; keep them few and exact everywhere else.
- **Map-iteration order in Go** makes some Go outputs non-deterministic:
  `watchOnce` Refs order (watch.go:136-139), `shortError` names
  (tree.go:233-237), `pickBatch` (fetch.go:206-234), node `Status.Voters`
  (status.go:97-99). Vectors avoid these; the harness sorts where the
  order is random.
- **zstd differences** (§7 #4) make upload counts depend on history. Checks
  must not compare counts across independent pushes.
- **`outputHashes`**: the core-rs hash was computed from `git archive`, not
  fetched by Nix; confirm on the first `nix build` and bump with every rev.
- **Nix Darwin sandbox refuses UDP binds**: network suites run only in
  cargo-native CI. `checks.tests` must stay socket-free, or it fails on
  macOS.
- **Offline `go mod tidy` writes a smaller `go.sum`** than an online one
  (§4.2 item 3); commit the online result, or `vectors` CI churns.
- **cgo**: building Go dstore with cgo on macOS hits the Xcode-license
  problem (working-copy plan:15-17); always `CGO_ENABLED=0`.
- **CI runners are UTC**, so local-zone bugs pass silently unless the TZ
  matrix runs.
- **Verbatim copies in vectorgen can drift** from `cmd/dstore`; the AST
  self-check must fail the run.
- **tzdb drift**: jiff's bundled tzdb and the Go or system tzdata may differ
  for future DST rules; the vector timestamps must predate the tzdb release
  of both.
- **CLI snapshots on macOS vs Linux** can differ in path texts
  (`/private/var` vs `/var`); normalize `{CWD}` from `pwd -P` and generate
  the committed file on Linux (CI).
- **Fake-node drift**: the fake node can diverge from real node semantics
  (for example `RefDelete` on an absent name). The live harness stays the
  authority; each fake handler cites its Go handler lines (§4.6).

### Open decisions

1. **`--version` text**: print `dstore version dev`, as an ldflags-less Go
   build does, or the crate version? The snapshot either matches `dev` or
   normalizes the `VERSION:` block and the `--version` line.
2. **Node-side commands** (`serve`, `cluster init`, `node join`,
   `catalog restore`, `--store` ticket derivation): omit them (then the help
   snapshots differ), keep them in help with a fixed error such as
   `dstore: <cmd> is not supported by dstore-client-rs; use the Go node binary`,
   or delegate to a Go binary on `PATH`? G2 tests whichever is chosen.
3. **Local store refs**: accept the Pebble/redb exception (G1), or give
   `store push/pull` a Pebble-compatible refs reader/writer?
4. **Error-text parity for core-rs-originated errors** (§7 #3): map them to
   Go texts in the CLI, or accept differences and compare exit codes only?
5. **CLI snapshots**: committed in `tests/golden/cli` (reviewable, offline
   tests) or generated only in CI?
6. **CI toolchain**: rustup-pinned 1.95.0 with a separate Nix job (as
   designed), or everything through `nix develop -c …` (slower, one
   toolchain)?
7. **mDNS policy** in CI: `auto` (skip) or `require`.
8. **Heavy (A12, A13) and chaos (C3) groups**: opt-in on
   `workflow_dispatch`, or on every PR?
9. **Where the fake node lives**: `src/testing` behind a `test-support`
   feature (part of the public API, reusable by consumers) or
   `tests/common` (private)?
10. **Timezone database**: jiff `tzdb-bundle-always`, or system tzdata with
    `TZDIR` from `pkgs.tzdata` in the flake?
11. **TUI**: port with ratatui, and are `TestUIModel`/`TestTeaHandler`
    strings (`sending`, `waiting for ack`, `eta 2s`, event line format)
    required?
12. **slog lines**: must Rust reproduce Go's `TextHandler` stderr format
    (level/msg/attrs), or only the final `dstore: …` error line?
13. **Lock interop (D12)**: require core-rs flock error texts to equal Go's,
    or check exit codes only?


---

## Addenda (synthesis)

Added by the architecture synthesis. `PORTING.md` is normative where it differs from this spec.

1. **Layout** superseded by the Cargo workspace of PORTING §3.
   - Crates live under `crates/`; the FakeNode is the `dstore-testkit` crate, not `src/testing` behind
     `test-support`.
   - The root package `dstore-client-rs` owns `tests/golden.rs`, `tests/cli_snapshots.rs`,
     `tests/fake_cluster.rs` and `tests/iroh_loopback.rs`.
   - Flake: the fileset adds `./crates` and `./examples`; `cargoBuildFlags = [ "-p" "dstore-client-rs" "--bin" "dstore" ]`;
     `checks.tests` runs `--workspace --lib` plus the three socket-free root test binaries, with no
     `checkFeatures`.
2. **§2.2 correction.** go-iroh **accepts** 52-character base32 ids with non-zero trailing bits
   (`decodeStdBase32NoPad` is Go's `encoding/base32`, which ignores trailing bits). The probe that
   printed `data is not a valid public key: input is z-base-32, …` flipped a data bit.
   `ticket/parse.json` must include the 16 trailing-bit variants, all accepted (codec-wire-ticket §2.4.6).
3. **§2.1.** Decode error texts are **asserted**, not informational (PORTING C7).
4. **§2.6 table.** Stale-view retries are `callRetry` 4 calls, `anyNode` up to 5 calls per node,
   watch 4 per node (`client/client.go:284-362`).
5. **§4.4, §7 #14.** jiff is not used. Each `text/formats.json` RFC3339 case carries `offset_secs`,
   and Rust tests format with `gocompat::time::FixedZone`. A process-level `TZ` matters only for CLI
   snapshots and interop.
6. **§4.7, §7 #7.** mDNS goes through the ported go-iroh resolver in `dstore_transport_iroh::mdns`, not
   iroh-mdns-address-lookup: swarm-discovery reads A/AAAA only from additionals. B13 stays the live
   evidence.
7. **§7 #6 and #11** are superseded: a hand-rolled codec, and the Go base32 decode loop in `gocompat`.
8. **Open decisions resolved.**
   1. `dstore version dev` unless built with `DSTORE_VERSION`.
   2. Node-side commands keep their definitions and fail with PORTING §2.2 texts; G2 asserts them.
   3. Accept exception G1; assert the refusal text of PORTING §2.3.
   4. Map core-rs texts that reach users (`corefmt`, errno rewrite, local wrapping); the rest are
      compared by exit code (DD-12).
   5. CLI snapshots committed under `tests/golden/cli`.
   6. rustup-pinned 1.95.0 plus a separate Nix job.
   7. mDNS `auto` in CI, `require` on a dev machine before releases.
   8. Heavy and chaos groups on `workflow_dispatch`.
   9. The testkit crate.
   10. No tzdb dependency.
   11. Hand-rolled TUI; `TestUIModel` asserts against `UiModel::view`, `TestTeaHandler` is ported.
   12. Unit vectors match slog line formats; interop compares the final `dstore:` line and exit codes.
   13. Lock interop compares exit codes and the text
       `packstore: <dir> is already open: resource temporarily unavailable`, which the errno rewrite
       makes identical.
9. **Additional CLI snapshot and interop cases:**
   - `cat NAME big | head -c1` and `watch … | head -1`, killed by SIGPIPE;
   - `cat NAME /`, exit 2 with the Go panic's first line (live only);
   - `serve --store X`, `cluster init --store X`, `node join --seed <hex> --token <hex> --store X`
     (exact node-side texts);
   - `store pull --local <dir whose refs/ holds Pebble files>` (refusal text).
