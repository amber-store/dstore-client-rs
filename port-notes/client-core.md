# client-core: cluster handle, view cache, ranking, connections, references, watch, progress, admin and status

Port spec for part A of the dstore client library. Normative reference:
`github.com/amber-store/dstore` at tag `v0.1.9` (HEAD `368f2c7`), checked out
at `/Users/dragan/amber-store/dstore`. Every behaviour below was read from
that source. Where `architecture/dstore.md` disagrees with the code, the code
wins and the difference is flagged (section 8.1). Part B (Missing, Put, Get,
Push, Pull, the fetcher) is specified elsewhere. It uses the internal helpers
defined here: `call`, `callRetry`, `handleErr`, `ok`, `stamp`, `preferred`,
`probeHinted`, `tracker`, `pathAttrs`, `pool.Open` and `pool.Drop`.

Byte strings marked "verified" were produced by running a Go 1.26.5 generator
against the v0.1.9 packages (see section 5). They are exact.

---

## 1. Scope

### 1.1 Go files covered (normative)

| file | lines | what it does |
|---|---:|---|
| `client/client.go` | 403 | `Config`, `Cluster`, `Dial`, view cache (`View`, `Placement`, `adopt*`, `RefreshView`), stamping, failure bookkeeping (`handleErr`, `ok`, `penalty`, `probeHinted`), `call`/`callRetry`/`anyNode`, `Primary`/`Owners`/`WriteSet`/`ReadOrder`, `Status`, `Admin`, `Nodes` |
| `client/rank.go` | 71 | `rttClass`, `rankOwners` (path preference) |
| `client/batch.go` | 29 | `RecordSizer`, `batches` (byte-balanced put batches) |
| `client/progress.go` | 181 | `Progress`, `ProgressReport`, `NodeProgress`, `PutObserver`, `tracker`, `countKeys`, `pathAttrs`, `Rate`, `HumanBytes` |
| `client/refs.go` | 150 | `Ref`, `ErrUnknownRef`, `CASMismatch`, `Incomplete`, `Cond`, `RefGet`, `RefPut`, `RefDelete`, `RefList` |
| `client/watch.go` | 245 | `RefChange`, `WatchRefs`, `allNodes`, `watchOnce` |
| `client/batch_test.go` | 51 | 4 tests of `batches` |
| `client/rank_test.go` | 110 | 7 tests of `rankOwners` |

### 1.2 Go files this area depends on (read for semantics; owned by other specs)

| file | lines | used for |
|---|---:|---|
| `transport/transport.go` | 243 | `Stream`, `PathInfo`, `Conn`, `Endpoint`, `Pool` (connection pooling, `Call`, `Open`, `Drop`, `Path`) — specified here in full because pooling is in this area |
| `transport/iroh.go` | 457 | `IrohConfig.DirectTimeout` (2 s), `Dial` (direct, then relay, then discovery), `irohConn.Path` (RTT unknown until sampled), QUIC keepalive/idle config |
| `transport/mem.go` | 365 | in-memory `Network` used by Go cluster tests (`Path()` is `{Direct: true, RTT: 1 ms}`) |
| `wire/wire.go` | 395 | `Msg` keys, frame types, error codes, `Error`, `IsCode`, `AsError`, `CloseStream` |
| `view/view.go` | 469 | `View.Compare`, `View.Node`, `NewPlacement`, `Placement.{Owners,WriteSet,ReadOrder}`, `ShortID` |
| `ticket/ticket.go` | 96 | `Ticket.Members` (id + addrs) |
| `codec/codec.go` | 38 | `CanonicalEncOptions` encoder, default decoder |
| `node/server.go` | 264 | server dispatch on the client ALPN, ACL refusals, `stampReply`, `checkEpoch`, `writeStale`, `handleView` |
| `node/watch.go` | 310 | ref-watch server: initial difference, `ref-synced` heartbeat, paging |
| `node/refs.go` | 422 | ref-get/put/delete/list servers, `condOf`, `catalogErr` |
| `node/admin.go` | 302 | `AdminRequest`, `AdminReply`, admin ops and their reply texts |
| `node/status.go` | 124 | `Status` struct returned by `TStatus` |
| `node/node.go` | 797 | `Unreachable()` (465-482), `OpenOffline` (776-783) |
| `cmd/dstore/client.go` | 574 | `dialTicket` (Config used by the CLI), `admin`, `adminAction`, `printStatus`, `refs`/`watch`/`ref` commands |
| `cmd/dstore/main.go` | 698 | `logger()` = slog `TextHandler` on stderr, `--log-level`, admin command → op mapping |
| `cmd/dstore/tui.go` | 374 | `teaHandler` (log records as UI events), `statusLine`, how progress is consumed |
| `worktree/flow.go` | — | calls `RefGet`, `errors.Is(err, client.ErrUnknownRef)`, `View()` (`TicketFromView`, 306-315) |
| `node/watch_test.go` | 260 | 4 watch integration tests |
| `node/cluster_test.go` | 831 | harness (`clientWith`: `RequestTimeout: 20 s`), `TestClusterPushProgress` |

Architecture: `architecture/dstore.md` §2 (114-153), §3 (155-268), §5.5
(643-675), §7 (842-947), §10 (1795-1855), §11.1-11.3 (1857-1940), §13
(2044-2132). Design notes:
`docs/superpowers/specs/2026-09-15-ref-watch-design.md` (150 lines),
`docs/superpowers/specs/2026-09-15-id-bootstrap-design.md` (77 lines).

### 1.3 Node-side needs that surface in this area

`dialClusterLog` (cmd/dstore/client.go:57-74) falls back to `--store DIR`
when `--ticket`/`$DSTORE_TICKET` is empty. It calls `localTicket(dir)`
(main.go:394-405), which needs the node internals:

- `node.OpenOffline(dir)` (node.go:776-783): reads `<dir>/identity` (error
  `node: no identity in %s: %w`), then `node.Open` (node.go:201-258):
  opens `<dir>/packstore`, opens the **Pebble** database `<dir>/meta`
  (meta/meta.go:36). If key `store_id` is absent it **writes** 16 random
  bytes there. It opens the paxos acceptor in `PaxosDir` (default
  `<store>/paxos`) and reads meta key `"view"` (node.go:32, 240-244):
  deterministic CBOR of `view.View`.
- `v == nil` → `errors.New("this store is not a member of a cluster")`.
- `worktree.TicketFromView(v)` (flow.go:306-315): `ClusterID`,
  `Incarnation`, and the members of the first **4** entries of `v.Nodes`
  (`ID`, `Addrs`).

A Rust port can do this only with a Pebble reader for `<store>/meta` (key
`view`). Section 8 lists the options. No other part-A path needs node
internals.

---

## 2. API used client-side

Notation: "Go int" values become `i64` in Rust unless noted. `NodeID` is
`[32]byte`. `ShortID(id)` is the lowercase hex of `id[:4]` (8 chars,
view.go:36). `IDString(id)` is the 64-char lowercase hex.

### 2.1 `Config` (client.go:23-40)

| field | Go type | default applied in `Dial` (client.go:63-84) | notes |
|---|---|---|---|
| `Endpoint` | `transport.Endpoint` | none. `nil` → `errors.New("client: no endpoint")`, checked **first**, before any default | not closed by `Cluster.Close` |
| `Ticket` | `ticket.Ticket` | — | only `Members[].ID` (len 32) and `Members[].Addrs` are used. `ClusterID` and `Incarnation` are **never** read or validated by the client |
| `Conns` | `int` | `<= 0` → `4` | connections per node (`transport.NewPool` perPeer) and put batches in flight per primary (objects.go:164) |
| `Jobs` | `int` | `<= 0` → `8` | worker bound for part B |
| `Logger` | `*slog.Logger` | `nil` → `slog.Default()` | |
| `GCInterval` | `time.Duration` | `== 0` → `4 * time.Hour` (only exactly zero) | part B re-pin at `GCInterval/2` |
| `RequestTimeout` | `time.Duration` | `== 0` → `2 * time.Minute` (only exactly zero) | bound of one `call`; open timeout of a watch stream; part B uses `10*RequestTimeout` for put/get streams |
| `BatchBytes` | `int` | `<= 0` → `defaultBatchBytes` = `16 << 20`; then `min(BatchBytes, wire.MaxPutBatch)` with `MaxPutBatch = 64 << 20` | |
| `WatchIdle` | `time.Duration` | `<= 0` → `2 * time.Minute` | no frame for this long → reconnect |

The CLI passes only `Endpoint`, `Ticket`, `Logger` and `GCInterval:
4*time.Hour` (cmd/dstore/client.go:90). `--jobs` flags feed ingest, not the
client, so the CLI always runs with `Conns=4`, `Jobs=8`, `RequestTimeout=2m`,
`BatchBytes=16MiB`, `WatchIdle=2m`. Go tests use `RequestTimeout: 20s` and
`WatchIdle: 5s`.

### 2.2 `Cluster` state (client.go:43-56)

```
cfg, log, ep, pool *transport.Pool
mu sync.RWMutex protects:
  view      *view.View                 // nil only before the first adopt
  pl        *view.Placement            // built with view.NewPlacement(view) on adopt
  bootAddrs map[NodeID][]string        // from the ticket
  backoff   map[NodeID]time.Time       // until when a node is penalised
  failures  map[NodeID]int             // consecutive non-remote failures
  unreach   map[NodeID]struct{}        // the last view reply's unreachable list
```

Every exported method is safe for concurrent use. No lock is held across I/O.

### 2.3 `Dial(ctx, cfg) (*Cluster, error)` (client.go:59-117)

1. `cfg.Endpoint == nil` → return `client: no endpoint`.
2. Apply the defaults in 2.1.
3. Create the cluster with empty maps. For each `m` in `cfg.Ticket.Members`
   with `len(m.ID) == 32`: `bootAddrs[ID] = m.Addrs`. A later duplicate
   overwrites an earlier one. Id-only members (the short ticket form)
   get `nil` addresses.
4. `pool = transport.NewPool(cfg.Endpoint, c.addrsOf, cfg.Conns)`.
5. For each member **in ticket order** (members with `len(ID) != 32` are
   skipped; duplicates are tried again):
   - `dctx = context.WithTimeout(ctx, 15*time.Second)`.
   - `resp, err := c.pool.Call(dctx, id, wire.ALPNClient, &wire.Msg{Type: wire.TView})`.
     The request is **not stamped** and does not go through `c.call`, so
     there is no `handleErr`/`ok`/`RequestTimeout` here.
   - `err != nil` → `lastErr = err`, next member.
   - `c.adoptReply(resp)` error → `lastErr = err`, next member.
   - Success: log `Info "connected"` with attrs `node=ShortID(id)`,
     `nodes=len(c.View().Nodes)` (int), then `pathAttrs(id)` (2.13), and
     return `c`.
6. No member tried: `lastErr = errors.New("client: ticket names no nodes")`.
7. `c.pool.Close()`; return
   `fmt.Errorf("client: no bootstrap node answered: %w", lastErr)`.

Id-only members: `addrsOf(id)` returns `nil`, so
`IrohEndpoint.Dial(ctx, id, nil, alpn)` takes the discovery dial
(iroh.go:301 → 307-319). It uses mDNS (3 s lookup timeout) and, when a relay
mode is set, number0's DNS lookup. With `--no-discovery` go-iroh has no dial
target and fails, most likely with `iroh: no reachable address for endpoint`
(go-iroh iroh/endpoint.go:1273, 1380). After the first view is adopted,
`addrsOf` prefers the view's addresses (2.4).

Pool side effect: a failed dial marks `failed[(id, alpn)]`. A second attempt
at the same id within 2 s without a live connection fails immediately with
`transport: peer recently unreachable` (2.8). This applies to duplicate
ticket members.

The CLI prints a Dial failure as `dstore: client: no bootstrap node answered: <lastErr>`
and exits 1 (main.go:48-51).

### 2.4 View cache

`View()` / `Placement()` (client.go:123-134): return the cached pointers
under the read lock. Placement tables are built once per adopted view.

`adoptReply(m)` (client.go:136-154):
1. `m.Type != wire.TViewReply` (48) → `fmt.Errorf("client: unexpected reply %d", m.Type)`.
2. `view.Decode(m.View)` error → returned as is (`view: decode: <cbor error>`, view.go:154-160).
3. `c.adopt(v)`.
4. Under the lock, **replace** `unreach` with the entries of `m.Unreachable`
   whose length is 32. This happens even when step 3 refused the view as
   older.

`adopt(v)` (client.go:156-167), under the write lock:
- If a view is cached: `cmp := cached.Compare(v.Incarnation, v.Epoch)`
  (view.go:177-191; +1 means cached is newer). If `cmp > 0 || (cmp == 0 && v.Version <= cached.Version)`, ignore `v`.
- Otherwise `view = v`, `pl = view.NewPlacement(v)`.
- So views order by `(Incarnation, Epoch, Version)`, and an equal triple is not re-adopted.

`addrsOf(id)` (client.go:169-178), the pool's resolver. It is called at dial
time, not when the connection is cached:
- If a view is cached and `view.Node(id)` finds the id under `nodes` or
  `pending.nodes` with `len(Addrs) > 0`, return those addresses.
- Else return `bootAddrs[id]` (`nil` if unknown).

`RefreshView(ctx)` (client.go:181-187): `anyNode(ctx, &Msg{Type: TView})`
(stamped, 2.6), then `adoptReply(resp)`. Any error is returned.

`stamp(m)` (client.go:189-197): if a view is cached, set `m.ClusterID = view.ClusterID`,
`m.Incarnation = view.Incarnation`, `m.Epoch = view.Epoch`. It mutates `m` in place, so
a retried request is re-stamped with the newer view. `Version` is never
sent.

`Nodes()` (client.go:393-403): ids of `view.Nodes`, in view order (canonical:
sorted by id). Pending-only nodes are excluded. `nil` without a view.

`allNodes()` (watch.go:98-108): `Nodes()`. If that is empty, the keys of
`bootAddrs` (Go map order, i.e. random).

### 2.5 Failure bookkeeping and hints

`handleErr(id, err)` (client.go:201-218):
- If `wire.AsError(err)` succeeds (`errors.As` through the wrap chain), the
  node answered. If `len(we.View) > 0` and `view.Decode(we.View)` succeeds,
  `adopt` it (decode errors are ignored). **Return without penalising.**
  This covers every remote code (`stale-view`, `not-owner`, `busy`,
  `unknown-ref`, ...).
- Otherwise, under the lock: `failures[id]++`;
  `d := 5*time.Second << min(failures[id]-1, 4)`; if `d > 60*time.Second`
  then `d = 60*time.Second`; `backoff[id] = time.Now().Add(d)`.
  So successive failures give 5 s, 10 s, 20 s, 40 s, 60 s, 60 s, ...
  `unreach` is not touched.

`ok(id)` (client.go:220-226): delete `backoff[id]`, `failures[id]` and
`unreach[id]`.

`penalty(id) int` (client.go:228-239), under the read lock:
`+2` if `backoff[id]` exists and `time.Now().Before(backoff[id])`, `+1` if
`id ∈ unreach`. Range 0-3. An expired backoff entry stays in the map and
still feeds the failure count until `ok`.

`probeHinted(ctx)` (client.go:245-263): snapshot the ids in `unreach` and
concurrently run `c.call(ctx with 3*time.Second timeout, id, &Msg{Type: TView})`
for each. Results are ignored, and it waits for all. A success clears the
hint through `ok`. A transport failure adds backoff. A remote error only
adopts a carried view. Called by `Push` (tree.go:50) and `Pull`
(tree.go:264), **not** by `PullTree`, which worktree fetch uses directly.

### 2.6 Request helpers

`call(ctx, id, m) (*Msg, error)` (client.go:266-281):
1. `cctx = context.WithTimeout(ctx, cfg.RequestTimeout)`.
2. `resp, err := pool.Call(cctx, id, wire.ALPNClient, c.stamp(m))`.
3. `err != nil` → `c.handleErr(id, err)`; return `resp, err`. `resp` is the
   TErr frame for a remote error. No caller uses it, so returning only the
   error is equivalent.
4. `c.ok(id)`.
5. If `resp.Epoch > 0 || resp.Incarnation > 0`, a view is cached, and
   `view.Compare(resp.Incarnation, resp.Epoch) < 0`:
   `go c.RefreshView(context.Background())`. This is fire-and-forget, not
   single-flight, errors are ignored, and each inner call is bounded by
   `RequestTimeout`.
6. Return `resp`.

`callRetry(ctx, id, m)` (client.go:284-294): up to **4** attempts of `call`,
retrying only while `wire.IsCode(err, "stale-view")`. `handleErr` has already
adopted the carried view, so each retry is stamped with the newer epoch.
Used by part B (`askPrimaries`).

`anyNode(ctx, m)` (client.go:329-362):
1. `ids` = the NIDs of `view.Nodes` (not pending). If there is no view or no
   nodes, the keys of `bootAddrs` (random order).
2. For each `id` in `c.preferred(ids)`:
   - `resp, err := c.call(ctx, id, m)`.
   - While `err` is `stale-view` and `attempt < 4`: `call` again. That is up
     to **5** calls per node.
   - `err == nil` → return `resp`.
   - `wire.AsError(err)` succeeds → return `resp, err` immediately ("the
     node answered; its answer stands"). This includes a `stale-view` that
     survived 5 attempts.
   - Else `lastErr = err`, next node.
3. `lastErr == nil` (no ids) → `errors.New("client: no nodes")`. Return `nil, lastErr`.

A cancelled or expired `ctx` does not stop the loop early. Every remaining
node is called, fails fast with the ctx error, and is penalised by
`handleErr`. The returned error is the last node's.

### 2.7 Ranking (rank.go, client.go:297-326)

`rttClass(rtt)` (rank.go:17-28): `rtt < 5ms` → 0; `< 25ms` → 1; `< 100ms` →
2; else 3. Strict `<`.

`rankOwners(ids, penalty, path)` (rank.go:34-71): for each input position
`i`, build `{id, pos: i, pen: penalty(id), relay: 0, class: 0}`. If
`path(id)` returns ok: `relay = 1` when `!p.Direct`, and `class = rttClass(p.RTT)`.
An undialed node, or a path with `RTT == 0`, therefore counts as
**direct and class 0 (near)**. Stable sort by `(pen asc, relay asc, class asc, pos asc)`.
Return the ids.

`preferred(ids)` (client.go:297-301): `rankOwners(ids, c.penalty, func(id) { return c.pool.Path(id, wire.ALPNClient) })`.
The path is the live connection's **current** path, read at ranking time, so
a path change re-orders the next ranking only.

`Primary(key) (NodeID, bool)` (client.go:304-310): `owners := Placement().Owners(key)`
(under `nodes` only). Empty → `(zero, false)`; else `(preferred(owners)[0], true)`.

`Owners(key)` = `Placement().Owners(key)`. `WriteSet(key)` =
`Placement().WriteSet(key)`, i.e. owners under nodes followed by the pending
owners not already present (view.go:343-358).

`ReadOrder(key)` (client.go:319-326): `order := Placement().ReadOrder(key)`
(rank under nodes, then rank under pending, deduplicated; view.go:361-378);
`r := int(View().Replicas)`. If `len(order) <= r` → `preferred(order)`; else
`append(preferred(order[:r]), order[r:]...)`. Only the first R are
re-ranked.

Not implemented in v0.1.9, although architecture §11.1 describes it: ageing
out measurements "after a minute without traffic". The RTT is whatever the
live connection reports.

### 2.8 Connection pool (`transport.Pool`, transport.go:63-243)

State: `perPeer` (`NewPool`: `<= 0` → 1). Maps keyed by `(id, alpn)`:
`conns []Conn`, `next int` (round-robin cursor), `dialing *sync.Mutex`,
`failed time.Time`. One pool mutex `mu`.

`Get(ctx, id, alpn)` (94-146):
1. Lock. Drop dead conns (their `Done()` channel is closed) from `conns[k]`.
2. If `len(live) >= perPeer`: `i := next[k] % len(live)`; `next[k]++`;
   unlock; return `live[i]`.
3. If `failed[k]` exists, `time.Since(failed[k]) < 2*time.Second` and
   `len(live) == 0`: unlock; return
   `errors.New("transport: peer recently unreachable")`.
4. Get or create `dialing[k]`; unlock.
5. `dm.Lock()` serialises dials per key. It is not ctx-aware.
6. Lock. If `len(conns[k]) >= perPeer`: return `conns[k][0]` (no round-robin,
   no liveness check). Unlock.
7. `addrs := p.addrs(id)` (the cluster's `addrsOf`).
8. `c, err := ep.Dial(ctx, id, addrs, alpn)`. On error: lock,
   `failed[k] = time.Now()`, unlock, return `err`.
9. Lock; `delete(failed, k)`; append `c`; unlock; return `c`.

Growth: while fewer than `perPeer` live connections exist, **every** `Get`
dials a new one, even sequentially. The first `Conns` requests to a node
open `Conns` connections, and the pool round-robins after that. There is
**no shrinking and no idle eviction**. A connection leaves only when it
dies. The client endpoint's QUIC config keeps connections alive with a 5 s
keepalive and a 60 s max idle (iroh.go:100-104), so connections persist while
the peer answers. Architecture §11.3 says "grow under load and shrink after
~90 s idle"; that is not implemented.

`Drop(id, alpn)` (149-159): remove all conns of the key and its `failed`
marker, then close each connection.

`Path(id, alpn) (PathInfo, bool)` (162-174): the `Path()` of the **first
live** connection of the key; `(zero, false)` if none.

`Close()` (177-187): close every connection and reset `conns` (`next`,
`dialing` and `failed` are kept).

`Call(ctx, id, alpn, req)` (192-229):
1. `c, err := Get(...)`; error → return.
2. `s, err := c.OpenStream(ctx)`; error → `Drop(id, alpn)`, return `err`.
   Only this failure drops the peer's connections.
3. `defer wire.CloseStream(s)`.
4. `wire.WriteMsg(s, req)`; error → return it (no drop).
5. `_ = s.CloseWrite()` (FIN).
6. Read **one** frame on a goroutine. `select` on done vs `ctx.Done()`. On
   ctx: `s.CancelRead(0)`, `s.Close()`, wait for the reader, return
   `ctx.Err()` (`context deadline exceeded` or `context canceled`).
7. Read error → return it (e.g. `EOF`, `wire: short frame: ...`, 3.1).
8. `reply.Type == wire.TErr` (10) → return `reply, wire.ErrorFromMsg(reply)`.
9. Return `reply`. The responder's FIN is not awaited.

`Open(ctx, id, alpn)` (232-243): `Get`; `OpenStream` error → `Drop` + return
`err`; return the stream. The caller closes it.

`wire.CloseStream(s)` (wire.go:390-395): `s.Close()` (FIN on the send side,
error ignored), then `CancelRead(0)`.

`PathInfo` (transport.go:28-31): `{Direct bool; RTT time.Duration}`, where
RTT is 0 until measured. `irohConn.Path()` (iroh.go:433-444): start with
`{Direct: true, RTT: 0}`. For each go-iroh path with `Selected`:
`Direct = !Relayed`, and `RTT = p.RTT` when `p.HasRTT`. The last selected
path wins. `HasRTT` is go-iroh's `rttStats.HasMeasurement()`
(internal/qng/internal/ackhandler/sent_packet_handler.go:700). Test
`TestIrohPathRTTIsUnknownUntilSampled` locks this.

`IrohEndpoint.Dial` summary (iroh.go:265-319; the transport spec owns the
details):
- Parse the address strings (`ip:<ip:port>`, `relay:<url>`, custom, bare
  `ip:port`; unparsable ones are skipped).
- With direct addresses, race them with `DirectTimeout` (default **2 s**).
- On failure with no relays, discovery dial. With relays, race
  relays+direct with no extra timeout, then discovery dial on failure. With
  no addresses at all, discovery dial.
- Candidate errors are `dial <addr>: <err>`, joined with `errors.Join`
  (newline-separated). A discovery failure appends `discovery: <err>`.
- A connection is only returned after the handshake completes
  (`awaitHandshake`).

`BindIroh` for a CLI client (cmd/dstore/client.go:86): random key, no ALPNs,
relay mode default n0 unless `--no-relay` (nil) or `--relay URL`, `Discover:
!--no-discovery`, `Announce: false`. When a relay mode is set, bind waits up
to 10 s for `Online` (iroh.go:126-133).

### 2.9 `Status` and `Admin` (client.go:365-390)

`Status(ctx, id) ([]byte, error)`: `resp, err := c.call(ctx, id, &Msg{Type: TStatus})`;
on error return it; else return `resp.Status` **without checking the reply
type**. A reply of another type gives `nil` bytes. The server
(status.go:106-109) answers `TStatusReply` (57) with `Status` = CBOR of
`node.Status` (3.4), stamped. The server does no epoch or ACL check beyond
the allowlist.

`Admin(ctx, id, req any) ([]byte, error)`:
1. `m := &Msg{Type: TAdmin, Params: codec.MustMarshal(req)}`, where `req` is
   `node.AdminRequest` (3.4).
2. `id == NodeID{}` → `anyNode(ctx, m)`; else `call(ctx, id, m)`.
3. Error → return it.
4. `resp.Type != TAdminReply` (58) → `fmt.Errorf("client: unexpected reply %d", resp.Type)`.
5. Return `resp.Status`, the CBOR of `node.AdminReply`.

Server (admin.go:49-62):
- `Params` decode error → `bad-request` with the decoder's text.
- `Admin` error that is a `*wire.Error` → forwarded with its code and text.
- Any other error → `unavailable` with `err.Error()`.
- Success → `TAdminReply`, stamped.
- An unknown op gives `bad-request` `unknown admin op <op>` (admin.go:260).

CLI use (cmd/dstore/client.go:98-132): `admin` decodes `AdminReply`.
`adminAction` prints `r.Text` if non-empty (`Println`), then each of
`r.Names` (`Println`), then, if `len(r.Key) == 32`, `key %x\n`.

Admin commands and their server texts (main.go, admin.go):

| CLI | `AdminRequest` | server reply text |
|---|---|---|
| `token create [--weight]` | `{Op:"token-create", Weight}` | `Text` = hex of the token (CLI prints `r.Text`) |
| `cluster ticket [--ids]` | `{Op:"cluster-ticket"}` | `Ticket` = encoded ticket of this node (with its own endpoint addrs) + first 3 view nodes; CLI prints `t.Encode()` or `t.IDs()` |
| `cluster replicas R [--yes]` | `{Op:"replicas", Replicas}` | `transition proposed` |
| `node remove ID [--dead] [--allow-unsafe]` | `{Op:"node-remove", Node, Dead, AllowUnsafe}` | `transition proposed at epoch %d` |
| `node drain ID` | `{Op:"node-drain", Node}` | same |
| `node weight ID GiB` | `{Op:"node-weight", Node, Weight}` | same |
| `node zone ID ZONE` | `{Op:"node-zone", Node, Zone}` | same |
| `node repair ID` | `{Op:"node-repair", Node}` | `repair scheduled: holders will refill <ShortID>` |
| `voter add ID` / `voter remove ID [--allow-unsafe]` | `{Op:"voter-add"/"voter-remove", Node, AllowUnsafe}` | `voters now %d` |
| `transition status` | `{Op:"transition-status"}` | `n.maint.transitionText(v)` |
| `transition abort` | `{Op:"transition-abort"}` | `transition aborted` |
| `transition refreeze` | `{Op:"transition-refreeze"}` | `participants re-frozen` |
| `transition pause` / `resume` | `{Op:"transition-pause"/"transition-resume"}` | `ok` |
| `gc run [--tolerate-missing] [--garbage F]` | `{Op:"gc-run", Tolerate, Garbage}` | `gc.statusText(g)` |
| `gc status` | `{Op:"gc-status"}` | `gc.statusText(g)` |
| `gc hold` / `gc release` | `{Op:"gc-hold", Pause: true/false}` | `ok` |
| `gc why KEY` | `{Op:"gc-why", Key}` | `Names` (reference names) |
| `catalog backup` | `{Op:"catalog-backup"}` | `backup written` + `Key` (CLI prints `key <hex>`) |
| `catalog backups` | `{Op:"catalog-backups"}` | `Names` = hex keys |
| (gateway) | `{Op:"keep", Names}` | `ok` |

Node id parse errors on the client side come from `view.ParseNodeID`:
`view: bad node id %q`. Server-side node id errors: `node id must be 32 bytes`
(sent as `unavailable`, since it is not a `*wire.Error`) and `key must be 32 bytes`.

### 2.10 References (refs.go)

Types:
- `Ref {Name string; Record []byte; Version []byte; Ref reference.Reference}` (13-18).
- `var ErrUnknownRef = errors.New("client: unknown reference")` (21).
- `CASMismatch {Current []byte; Record []byte; Version []byte; HasCurrent bool}` (24-36).
  - `Error()`: `!HasCurrent` → `cas mismatch: reference is absent`; else
    `fmt.Sprintf("cas mismatch: current key %x", e.Current)`.
  - Verified: `HasCurrent` with nil `Current` gives `cas mismatch: current key ` (trailing space).
- `Incomplete {Sample [][32]byte; Shortfall int}`. `Error()` = `fmt.Sprintf("incomplete: %d keys short", e.Shortfall)` (40-47).
- `Cond {ExpectedVersion []byte; Versioned bool; ExpectedOld []byte; Keyed bool; Force bool}` (50-56).

`cond.apply(m)` (58-68):
- `m.Force = cond.Force`.
- If `Versioned`: `m.HasExpected = true`; `m.ExpectedVersion = cond.ExpectedVersion`.
- If `Keyed`: `m.HasExpected = true`; `m.ExpectedOld = cond.ExpectedOld`.
- Both may be set.

Server interpretation `condOf` (node/refs.go:27-41):
- `Force = m.Force`.
- If `HasExpected`:
  - if `ExpectedVersion != nil || (ExpectedOld == nil && Version == nil)` →
    Versioned with `ExpectedVersion` (nil means "must not exist");
  - if `ExpectedOld != nil` → Keyed, and Versioned is cleared.
- An empty `ExpectedVersion` is omitted on the wire (`omitempty`) and arrives
  as nil ("must not exist").

`refErr(err)` (70-75): `wire.IsCode(err, "unknown-ref")` → `ErrUnknownRef`;
else `err`.

`RefGet(ctx, name) (*Ref, error)` (78-91):
1. `resp, err := anyNode(ctx, &Msg{Type: TRefGet, Name: name})`; error → `refErr(err)`.
2. `resp.Type != TRef` (52) → `client: unexpected reply %d`.
3. `r, err := reference.Decode(resp.Record)`; error returned **unwrapped**
   (core's texts).
4. Return `&Ref{Name: name, Record: resp.Record, Version: resp.Version, Ref: r}`.

Server (refs.go:68-77):
- `reference.ValidateName(m.Name)` error → `bad-request` with its text.
- Catalog error → `catalogErr` (below).
- Success → `TRef {Record, Version}`, stamped.

`RefPut(ctx, record, cond) (version []byte, err error)` (95-112):
1. `m := &Msg{Type: TRefPut, Record: record}`; `cond.apply(m)`.
2. `anyNode` error → `refErr(err)`.
3. By type:
   - `TOK` (53) → return `resp.Version`.
   - `TCASMismatch` (54) → `&CASMismatch{Current: resp.Current, Record: resp.Record, Version: resp.Version, HasCurrent: resp.HasCurrent}`.
   - `TIncomplete` (55) → `sample, _ := wire.Keys32(resp.Keys)` (nil if any
     key is not 32 bytes); `&Incomplete{Sample: sample, Shortfall: resp.Shortfall}`.
   - Other → `client: unexpected reply %d`.

Server (refs.go:125-175), validation in this order:
1. `reference.Decode(m.Record)` err → `bad-request` `record: <err>`.
2. `len(rec.Key) != 32` → `bad-request` `record key`.
3. `ValidateName(rec.Name)` err → `bad-request` `name: <err>`.
4. `m.Name != "" && m.Name != rec.Name` → `bad-request` `frame name differs from the record's`.
5. `key.Parse(rec.Key)` err → `bad-request` `root key: <err>`.
6. `checkEpoch(m)` returns stale → `writeStale`: **stamped**
   `TErr {Code: "stale-view", Text: "request epoch is behind", View: <node's view>}`.
   A different `ClusterID` gives error `wrong cluster`, which is **not**
   treated as stale; the put continues. A request epoch above the node's
   triggers a single-flight refresh (server.go:211-246).
7. Completeness walk:
   - deadline → `timeout` `completeness walk exceeded put_ttl`;
   - other error → `unavailable` `completeness walk: <err>`;
   - missing keys → `TIncomplete {Keys: first ≤64 missing, Shortfall}`, stamped.
8. Catalog CAS error → `catalogErr`.
9. Success → `TOK {Key: rec.Key, Version}`, stamped.

`RefDelete(ctx, name, cond) error` (115-129):
1. `m := &Msg{Type: TRefDelete, Name: name}`; `cond.apply(m)`; `anyNode`;
   error → `refErr`.
2. `TOK` → nil. `TCASMismatch` → `&CASMismatch{...}` (same fields). Other →
   `client: unexpected reply %d`.

Server (refs.go:104-122):
- `ValidateName` → `bad-request`.
- Catalog error → `catalogErr`.
- Success → empty `TOK`, stamped.
- With an allowlist, a non-admin peer gets `unauthorized`
  `ref-delete needs an admin peer` (server.go:108-110).

CLI `ref delete` (cmd/dstore/client.go:437-447): `Force` unless
`--expected-version HEX` (then `Versioned`). With neither flag, `Force =
true`.

`catalogErr` (refs.go:52-66):
- `*catalog.CASMismatch` → `TCASMismatch` reply, stamped, with
  `Version`, `HasCurrent`, `Record = current record`,
  `Current = key of the current record` (nil if the record fails to decode).
- `catalog.ErrUnknownRef` → `unknown-ref` `no such reference`.
- `context.DeadlineExceeded` → `timeout` with `err.Error()`.
- Code `expired` → `timeout` `reference commit expired`.
- Anything else → `unavailable` with `err.Error()`.

`RefList(ctx, prefix) ([]wire.RefInfo, error)` (132-150):
```
out = nil; after = nil
loop:
  resp, err := anyNode(ctx, &Msg{Type: TRefList, Prefix: []byte(prefix), After: after})
  err → return nil, err            // NOT mapped through refErr
  resp.Type != TRefs (56) → "client: unexpected reply %d"
  out = append(out, resp.Refs...)
  if len(resp.Next) == 0 || len(resp.Refs) == 0 → break
  after = resp.Next
return out
```
Each page is a separate `anyNode`, so pages may come from different nodes.

Server (refs.go:79-102):
- `cat.RefList(prefix, after, 20000)`; error → `catalogErr`.
- Per entry: skip records that fail to decode. Append `RefInfo{Name, Key,
  Version, CreatedAt, User}`. Add `len(name)+32+40+len(user)+16` to a size
  estimate. If it exceeds `wire.MaxPageBytes` (`4 << 20`), set
  `next = e.Name` and stop.
- `next != ""` → `Next = []byte(next)`. `after` is exclusive.
- Edge: a page on which every record failed to decode has `Refs` empty and
  ends the client loop early. Replicate it.

The CLI `refs [PREFIX]` prints each entry as
`fmt.Printf("%s\t%x\t%s\t%s\n", r.Name, r.Key, time.Unix(0, r.CreatedAt).Format(time.RFC3339), r.User)`
(local time zone).

### 2.11 `WatchRefs` (watch.go)

`RefChange` (17-29): `{Name string; Key []byte /* nil when Deleted */; Version []byte; CreatedAt int64; User string; Deleted bool; Synced bool; Node view.NodeID}`.

`errWatchIdle = errors.New("client: watch stream idle")` (32).

`WatchRefs(ctx, pattern, known map[string][]byte) iter.Seq2[RefChange, error]` (44-94):
- `state` is a copy of `known` (a nil map is fine). The copy is made when
  `WatchRefs` is called, not when iteration starts.
- The returned iterator runs on the consumer's goroutine (pull semantics).
  `yield` returning false means the consumer stopped.

```
delay := 1s
for ctx.Err() == nil {
    served := false
    for _, id := range c.preferred(c.allNodes()) {
        res := c.watchOnce(ctx, id, pattern, state, yield)
        for attempt := 0; res == watchRetry && attempt < 3; attempt++ {
            res = c.watchOnce(ctx, id, pattern, state, yield)   // ≤ 4 attempts per node
        }
        switch res { case watchStop, watchFatal: return; case watchServed: served = true }
        if served { break }            // leave the for loop: re-rank
        // watchFailed, or watchRetry after 4 attempts: next node
    }
    if served {
        delay = 1s
        select { case <-ctx.Done(): return; case <-time.After(200ms): }
        continue
    }
    rctx, cancel := context.WithTimeout(ctx, 15s); _ = c.RefreshView(rctx); cancel()
    c.log.Warn("watch: no node answered, retrying", "pattern", pattern, "in", delay)
    select { case <-ctx.Done(): return; case <-time.After(delay + time.Duration(rand.Int64N(int64(delay/2)+1))): }
    delay = min(2*delay, 30s)
}
```
- Jitter is a uniform integer number of nanoseconds in `[0, delay/2]`,
  inclusive. The logged `in` is the pre-jitter delay: 1s, 2s, 4s, 8s, 16s,
  30s, 30s, ...
- Cancellation ends the sequence **without yielding an error**. The CLI
  `watch` then returns nil and exits 0 on Ctrl+C.

`watchResult` (110-118): `watchFailed` (nothing came of this node),
`watchRetry` (stale view adopted), `watchServed` (the stream synced, then
died), `watchStop` (consumer stopped or ctx ended), `watchFatal` (terminal
error yielded).

`watchOnce(ctx, id, pattern, state, yield)` (127-245):
1. `octx = WithTimeout(ctx, RequestTimeout)`; `s, err := pool.Open(octx, id, ALPNClient)`;
   `cancel()` right away (the timeout only bounds opening). Error →
   `handleErr(id, err)` → `watchFailed`.
2. `defer wire.CloseStream(s)`.
3. `refs := []RefInfo{}` with one `{Name, Key}` per `state` entry (Go map
   order; `Version`, `CreatedAt`, `User` zero). Write the stamped
   `&Msg{Type: TRefWatch, Pattern: pattern, Refs: refs}`. The request is
   **not** finished with CloseWrite; the client keeps its send side open.
   Write error → `pool.Drop(id, ALPNClient)`; `handleErr(id, err)`;
   `watchFailed`.
4. Reader goroutine: loop `ReadMsg(s)` → send `{m, err}` on `frames` (buffer 1),
   or exit on `rctx.Done()`. It exits after sending an error. `rctx` is
   cancelled on return.
5. `abandon()`: `s.CancelRead(0)`; `s.Close()`.
6. `synced := false`; `served()` returns `watchServed` if `synced`, else `watchFailed`.
7. `idle := time.NewTimer(WatchIdle)`. Loop on `select`:
   - `ctx.Done()` → `abandon()` → `watchStop`.
   - `idle.C` → `abandon()`; `pool.Drop(id, ALPNClient)`; `handleErr(id, errWatchIdle)`
     (penalises); `log.Warn "watch: stream idle, reconnecting" node=ShortID(id)`
     → `served()`.
   - frame with `err != nil` → `abandon()`; `pool.Drop`; `handleErr(id, err)`;
     `log.Warn "watch: stream ended, reconnecting" node=ShortID(id) error=<err>`
     → `served()`. A clean server FIN surfaces as `EOF`.
   - Any frame: `idle.Reset(WatchIdle)`, then by `m.Type`:
     - `TErr`: `e := wire.ErrorFromMsg(m)`; `handleErr(id, e)` (adopts
       `e.View`, no penalty). By code:
       - `stale-view` → `watchRetry`;
       - `bad-request` or `unauthorized` → `yield(RefChange{}, e)` (return
         value ignored) → `watchFatal`;
       - other codes → `log.Warn "watch: node refused, trying the next" node=ShortID(id) error=<e>`
         → `watchFailed`.
       - No abandon or drop; the deferred CloseStream closes the stream.
     - `TRefChanges` (59): `ok(id)`. For each `r` in `m.Refs`, in order:
       `state[r.Name] = r.Key`; `yield(RefChange{Name, Key, Version, CreatedAt, User}, nil)`;
       false → `abandon()` → `watchStop`. Then for each `name` in
       `m.Deleted`: `delete(state, name)`; `yield(RefChange{Name: name, Deleted: true}, nil)`;
       false → abandon → stop. Refs are always yielded before deletions
       within one frame.
     - `TRefSynced` (60): `ok(id)`. If `!synced`: `synced = true`;
       `log.Info "watch: synced" pattern=<pattern> node=ShortID(id) refs=len(state)`;
       `yield(RefChange{Synced: true, Node: id}, nil)`; false → abandon →
       stop. Later heartbeats are silent (they still reset idle and call `ok`).
     - Any other type: `abandon()`; `pool.Drop`;
       `handleErr(id, fmt.Errorf("%w: type %d", wire.ErrProtocol, m.Type))`
       → `served()`. Not logged.

Server behaviour the client relies on (node/watch.go, server.go:129-130):
- Pattern compile error → `bad-request` with refglob's text. No view →
  `unavailable` `no view`. With an allowlist, non-allowed peers get
  `unauthorized` `not on the allowlist`.
- Known list: entries matching the pattern with `len(Key) > 0` (null or empty
  keys are ignored).
- Initial reconcile: scan pages of 20000 under the pattern's literal prefix,
  2 min timeout. Changes are sent as `TRefChanges {Refs, Deleted}` (stamped),
  flushed when the size estimate exceeds 4 MiB (`name+key+version+user+16`
  per ref, `name+8` per deletion). Then `TRefSynced`, stamped. An error in
  the initial reconcile → `catalogErr` (e.g. `unavailable`).
- Afterwards: hints are forwarded as they arrive. Every
  `WatchReconcile` (default 30 s) the server rescans and sends `TRefSynced`
  again (the heartbeat). A failed rescan sends **nothing** and is retried
  next tick, so ≥4 consecutive failed rescans (≥2 min) trip the client's
  idle timeout.
- The watch ends when the client closes its side (the server reads until
  EOF or reset), a write fails, or the node shuts down.
- The per-stream `2*PutTTL` timeout does not apply to watches.

CLI `watch PATTERN` (cmd/dstore/client.go:370-405):
- `known = nil`.
- Change: `fmt.Printf("%s\t%x\t%s\t%s\n", ch.Name, ch.Key, time.Unix(0, ch.CreatedAt).Format(time.RFC3339), ch.User)`.
- Deletion: `fmt.Printf("%s\tdeleted\n", ch.Name)`. Synced: prints nothing.
- On error: `return err` → `dstore: remote: bad-request: <text>`, exit 1.

### 2.12 Progress (progress.go) and batching (batch.go)

`Progress func(ProgressReport)` (17). It is called for every tracker update,
**synchronously and under the tracker's mutex**, from many goroutines. It
must be cheap and must not call back into the client.

`ProgressReport` (22-29): `{Objects int; TotalObjects int; Bytes int64; TotalBytes int64; Nodes []NodeProgress}`.
- `Bytes` counts record lengths sent or received. A resent record counts
  twice, and `TotalBytes` grows with it. `TotalBytes` is 0 until known.
- `Nodes` is ordered by id (bytewise ascending).

`NodeProgress` (32-39): `{ID view.NodeID; Direct bool; RTT time.Duration; InFlight int; Awaiting int; Bytes int64}`.

`PutObserver` (45-50): `{Start func(node); Sent func(node, n int); Flushed func(node); Done func(node, flushed bool)}`.
Nil funcs are skipped. Emitted by part B `putOnce` (objects.go:244-301):
- `Start` at the beginning, before the stream opens;
- `Done` is deferred, with `flushed` set once the whole batch was handed to
  the wire;
- `Sent(p, len(rec))` after each record;
- `Flushed` after `CloseWrite`.

`tracker` (53-135):
- `newTracker(c, prog)`: empty `nodes` map.
- `observer()`:
  - `Start` → `node(id).InFlight++`;
  - `Sent(n)` → `node(id).Bytes += n` and `rep.Bytes += n`;
  - `Flushed` → `node(id).Awaiting++`;
  - `Done(flushed)` → `node(id).InFlight--`, and if `flushed`, `Awaiting--`.
  - Each is one `update`.
- `totals(objects, done, bytes)`: `rep.TotalObjects = objects`, `rep.Objects = done`, `rep.TotalBytes = bytes`.
- `more(bytes)`: `rep.TotalBytes += bytes`.
- `objects(n)`: `rep.Objects += n`.
- `bytes()`: returns `rep.Bytes` under the lock and does **not** report.
- `node(id)`: get or create `&NodeProgress{ID: id}`.
- `update(f)`: lock; `f()`; if `prog != nil`, `prog(snapshot())`; unlock.
- `snapshot()`: copy `rep`. `Nodes` = a copy of each node entry. If
  `pool.Path(n.ID, ALPNClient)` is ok, set `Direct` and `RTT` from it; else
  they keep their stored values, which are never written, so `Direct=false`
  and `RTT=0`. Sort by `bytes.Compare(ID)`. Lock order is tracker mutex,
  then pool mutex.

Emitters:
- `Push` (tree.go:29-168):
  - round 0: `totals(len(all), len(all)-lacking, lackBytes)`;
  - later rounds: `more(lackBytes)`;
  - `Put(..., obs)`, then `objects(n)` for keys newly held;
  - after an incomplete ref-put: `more(lackBytes)` + `Put(obs)`.
- `directFill`: `more(bytes)` + `Put(..., tr.observer())`.
- `PullTree` (tree.go:350-351) calls `prog` **directly**, not through the
  tracker: `prog(ProgressReport{Objects: st.Fetched, TotalObjects: st.Keys, Bytes: st.Bytes})`
  (`TotalBytes` 0, `Nodes` nil).

`countKeys(m map[NodeID][][32]byte, size)` (138-146): the sum of `len` and
the sum of `size(k)` over all keys.

`RecordSizer func(k [32]byte) int` (batch.go:7).

`batches(keys, size, maxBytes, maxKeys)` (batch.go:12-29):
- Keep order. Accumulate into `cur` with running `bytes`.
- Before adding `k` (with `n := size(k)`): if `len(cur) > 0 && (bytes+n > maxBytes || len(cur) >= maxKeys)`,
  flush `cur` and reset.
- Then append `k` and add `n`. Flush the rest at the end.
- A record larger than `maxBytes` becomes a batch of its own.
- Constants: `defaultBatchBytes = 16 << 20`, `batchKeys = 8192` (objects.go:20-23).

`Rate(bytes int64, took time.Duration) string` (162-167): `took <= 0` → `"-"`;
else `HumanBytes(int64(float64(bytes)/took.Seconds())) + "/s"`.

`HumanBytes(n int64) string` (170-181): `n < 1024` (negatives included) →
`fmt.Sprintf("%d B", n)`. Otherwise `div, exp = 1024, 0`; `for m := n/1024; m >= 1024; m /= 1024 { div *= 1024; exp++ }`;
`fmt.Sprintf("%.1f %ciB", float64(n)/float64(div), "KMGTPE"[exp])`.
`%.1f` rounds exact decimal ties to even (1280 → `1.2 KiB`, 1792 →
`1.8 KiB`). Rust's `{:.1}` does the same (verified with rustc 1.86: 1.25→1.2,
1.75→1.8, 0.25→0.2).

### 2.13 Every logging call of the client package

All calls go to `c.log` (`Config.Logger`). The CLI's default handler is
`slog.NewTextHandler(os.Stderr, &HandlerOptions{Level: logLevel(c)})`
(main.go:66-68). `--log-level` / `$DSTORE_LOG_LEVEL`: `debug`, `warn`,
`error`, anything else → info. The TUI replaces it with `teaHandler`
(tui.go:336-374), which renders `msg key=value...`. It humanises the key
`bytes` only when its kind is `Int64`, and quotes values containing a space
or tab with `%q`. **Attribute kinds matter**: Go `int` becomes slog `Int64`,
`time.Duration` becomes `Duration`, and errors are `Any`.

| where | level | message (verbatim) | attributes in order (slog kind) |
|---|---|---|---|
| client.go:109 | INFO | `connected` | `node` (String ShortID), `nodes` (Int64), `path` (String `none`/`direct`/`relay`), `rtt` (Duration, only when path ≠ none) |
| watch.go:85 | WARN | `watch: no node answered, retrying` | `pattern` (String), `in` (Duration) |
| watch.go:186 | WARN | `watch: stream idle, reconnecting` | `node` (String) |
| watch.go:194 | WARN | `watch: stream ended, reconnecting` | `node` (String), `error` (Any: error) |
| watch.go:210 | WARN | `watch: node refused, trying the next` | `node` (String), `error` (Any: `*wire.Error`) |
| watch.go:232 | INFO | `watch: synced` | `pattern` (String), `node` (String), `refs` (Int64) |
| objects.go:57 (part B) | WARN | `negotiation failed at a primary, asking the next owner` | `objects` (Int64), `attempt` (Int64) |
| objects.go:225 (B) | WARN | `upload retry` | `node`, `reason` (String `stale view`), `attempt` (Int64) |
| objects.go:234 (B) | WARN | `upload retry` | `node`, `reason` (String `busy`), `wait` (Duration), `attempt` (Int64) |
| objects.go:238 (B) | WARN | `upload failed` | `node`, `objects` (Int64), `err` (Any) |
| objects.go:265 (B) | INFO | `uploading` | `node`, `objects` (Int64), `bytes` (Int64), then `pathAttrs` |
| objects.go:291 (B) | INFO | `batch sent, waiting for the node to store and replicate it` | `node`, `objects` (Int64), `bytes` (Int64) |
| objects.go:299 (B) | INFO | `uploaded` | `node`, `objects` (Int64), `bytes` (Int64), `took` (Duration rounded to ms), `rate` (String) |
| tree.go:65 (B) | INFO | `negotiated` | `objects`, `present`, `upload` (Int64), `bytes` (Int64), `primaries` (Int64) |
| tree.go:68 (B) | INFO | `re-sending objects short at their primaries` | `round` (Int64), `objects` (Int64), `bytes` (Int64) |
| tree.go:85 (B) | WARN | `upload to a primary failed, its objects go to another owner` | `node` (String), `err` (Any) |
| tree.go:124 (B) | INFO | `upload complete` | `uploaded` (Int64), `bytes` (Int64), `took` (Duration ms-rounded), `rate` (String) |
| tree.go:135 (B) | INFO | `reference written` | `name` (String), `version` (String `%x`) |
| tree.go:144 (B) | WARN | `reference write incomplete, renegotiating` | `attempt` (Int64), `err` (Any) |
| tree.go:189 (B) | INFO | `sending short objects to their owners directly` | `objects` (Int64), `owners` (Int64), `bytes` (Int64) |

The watch messages use the key `error`; part B uses `err`. The transport
also logs `WARN "transport: mdns discovery unavailable" error=<err>`
(iroh.go:158).

`pathAttrs(id)` (progress.go:149-159): no live connection →
`["path", "none"]`; else `["path", "direct" | "relay", "rtt", p.RTT.Round(time.Millisecond)]`.

`Dial` logs `connected` on **every** CLI client command at the default info
level. For example, `dstore refs` writes
`time=... level=INFO msg=connected node=1a2b3c4d nodes=3 path=direct rtt=1ms`
to stderr.

### 2.14 Error texts (verbatim)

| source | text |
|---|---|
| client.go:61 | `client: no endpoint` |
| client.go:113 | `client: ticket names no nodes` |
| client.go:116 | `client: no bootstrap node answered: %w` |
| client.go:138, 387; refs.go:84, 111, 128, 141 | `client: unexpected reply %d` |
| client.go:359 | `client: no nodes` |
| refs.go:21 | `client: unknown reference` |
| refs.go:33 / 35 | `cas mismatch: reference is absent` / `cas mismatch: current key %x` |
| refs.go:46 | `incomplete: %d keys short` |
| watch.go:32 | `client: watch stream idle` |
| wire.go:244; watch.go:241 | `wire: unexpected frame` / `%w: type %d` → `wire: unexpected frame: type 99` |
| wire.go:295-300 | `remote: <code>` or `remote: <code>: <text>` |
| wire.go:250, 253, 274, 278, 282 | `wire: encode frame: %w`, `wire: frame of %d bytes exceeds limit %d`, `wire: short frame: %w`, `wire: decode frame: %w` |
| wire.go:264-270 | `EOF` (clean end before a header), `unexpected EOF` (cut header) |
| wire.go:347; 369 | `%w: type %d, want %d`; `wire: key %d has %d bytes` |
| transport.go:115 | `transport: peer recently unreachable` |
| transport.go:58 | `transport: closed` |
| iroh.go:324, 343, 314 | `transport: no candidate addresses for %s`, `dial %s: %w`, `discovery: %w` |
| view.go:157 | `view: decode: %w` |
| context | `context canceled`, `context deadline exceeded` |

Verified read errors: empty input → `EOF`; 2 header bytes → `unexpected EOF`;
header only → `wire: short frame: EOF`; header plus 1 of 4 bytes →
`wire: short frame: unexpected EOF`; header `0x01000001` →
`wire: frame of 16777217 bytes exceeds limit 16777216`; payload `80` →
`wire: decode frame: cbor: cannot unmarshal array into Go value of type wire.Msg (cannot decode CBOR array to struct without toarray option)`;
payload `a1 06 01` →
`wire: decode frame: cbor: cannot unmarshal positive integer into Go struct field wire.Msg.6 of type string`.
Decoder texts belong to the codec spec. Rust need not reproduce fxamacker's
texts unless the codec spec decides to.

### 2.15 Concurrency summary

- `Cluster`: RWMutex over the view, hints and backoff. The pool has its own
  mutex plus per-peer dial mutexes. The progress tracker has its own mutex.
- Async work spawned: `RefreshView` from `call` (unbounded), the reader
  goroutine per watch stream, and the goroutines in `probeHinted`.
- `WatchRefs` runs on the consumer goroutine. Events are produced only while
  the consumer iterates. The idle timer keeps running while the consumer
  processes an event, so a consumer slower than `WatchIdle` causes an idle
  reconnect. Replicate this.

---

## 3. Byte formats and text formats

### 3.1 Frames

A frame is a 4-byte big-endian payload length followed by the payload: an
fxamacker `CanonicalEncOptions` CBOR map with integer keys, sorted
length-first (equivalent to bytewise order for keys 0-255). Maximum payload
`16 << 20`. The codec spec owns the general rules. The rules that matter for
this area:

- `Type` (key 0) is always present. Every other `Msg` field is `omitempty`:
  zero ints/uints, `false`, empty strings and empty or nil slices are
  omitted.
- `RefInfo` (keys 0-4): `Name`, `Key`, `Version` and `CreatedAt` have **no**
  omitempty. A nil `[]byte` encodes as CBOR `null` (`f6`), `CreatedAt 0` as
  `00`. `User` (4) is omitempty.
- Floats (AdminRequest `Garbage`) use the shortest encoding that preserves
  the value: `0.5` → `f9 3800`, `1/3` → `fb 3fd5555555555555`.
- The decoder ignores unknown keys. fxamacker defaults: duplicate keys are
  quiet (first wins), `MaxArrayElements = MaxMapPairs = 131072`, nesting cap
  32.

`Msg` keys used here (wire.go:170-240):

| key | field | CBOR |
|---:|---|---|
| 0 | Type | uint |
| 1 | ClusterID | bstr |
| 2 | Incarnation | uint |
| 3 | Epoch | uint |
| 4 | Keys | array of bstr |
| 6 | Name | tstr |
| 7 | Record | bstr |
| 9 | Key | bstr |
| 10 | Code | tstr |
| 11 | Text | tstr |
| 12 | View | bstr (CBOR of `view.View`) |
| 13 | Version | bstr |
| 14 | ExpectedVersion | bstr |
| 15 | ExpectedOld | bstr |
| 16 | Force | bool |
| 17 | HasExpected | bool |
| 18 | Prefix | bstr |
| 19 | After | bstr |
| 21 | Refs | array of RefInfo |
| 22 | Next | bstr |
| 27 | Unreachable | array of bstr |
| 28 | RetryAfter | uint/int (ms) |
| 29 | Current | bstr |
| 30 | Shortfall | int |
| 31 | HasCurrent | bool |
| 51 | Params | bstr (CBOR of `AdminRequest`) |
| 57 | Status | bstr (CBOR of `node.Status` / `AdminReply`) |
| 60 | Pattern | tstr |
| 61 | Deleted | array of tstr |

Frame types (wire.go:40-104): `TErr=10`, `TView=32`, `TRefGet=36`,
`TRefPut=37`, `TRefDelete=38`, `TRefList=39`, `TStatus=40`, `TAdmin=41`,
`TRefWatch=42`, `TViewReply=48`, `TRef=52`, `TOK=53`, `TCASMismatch=54`,
`TIncomplete=55`, `TRefs=56`, `TStatusReply=57`, `TAdminReply=58`,
`TRefChanges=59`, `TRefSynced=60`, `TPing=105`, `TPong=106`.

Error codes (wire.go:107-129): `stale-view`, `not-owner`, `no-space`, `busy`,
`bad-request`, `unauthorized`, `unknown-ref`, `cas-mismatch`, `incomplete`,
`unavailable`, `timeout`, `internal`, `not-member`, `need-view`, `expired`,
`amnesiac`, `mark-frozen`, `retired`, `too-soon`, `conflict`, `no-mark`.

`wire.Error` (288-311): `{Code, Text, View []byte, RetryAfter time.Duration}`.
`ErrorFromMsg` converts ms to a Duration (1500 → `1.5s`, verified).
`Error.Is(target)` matches by code, and also by text when the target has
one. `IsCode`/`AsError` use `errors.As` and see through `%w` wrapping.

ALPN: `amber-dstore/1` (the client's only ALPN).

Stream discipline on the client:
- **One-shot**: write the request, CloseWrite (FIN), read exactly one frame,
  then CloseStream (FIN + STOP_SENDING code 0).
- **Watch**: write the request, never FIN while reading. On return,
  CloseStream. Abandoning also sends STOP_SENDING 0 and FIN.

Server-side stamping: success replies and `writeStale` carry the node's
`Incarnation` and `Epoch` (keys 2, 3). Plain `WriteErr` frames (`bad-request`,
`unknown-ref`, `unavailable`, `unauthorized`, ...) carry **only** keys 0, 10
and 11. So `call`'s async-refresh check sees epochs only on successful
replies.

### 3.2 Requests the client sends (verified hex, whole frames)

Common stamping in these vectors: `ClusterID = 00 01 ... 0f` (16 bytes),
`Incarnation = 1`, `Epoch = 7`.

Reference record used below (core deterministic CBOR; name `trees/a`, key
`01..20`, user `alice`, created_at `1700000000123456789`):
```
a4006774726565732f610158200102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f200265616c696365031b17979cfe3d85cd15
```

| case | frame hex |
|---|---|
| Dial view request (unstamped) | `00000004a1001820` |
| RefreshView / probe view (stamped) | `0000001aa40018200150000102030405060708090a0b0c0d0e0f02010307` |
| Status (stamped) | `0000001aa40018280150000102030405060708090a0b0c0d0e0f02010307` |
| RefGet `trees/a` | `00000023a50018240150000102030405060708090a0b0c0d0e0f02010307066774726565732f61` |
| RefPut, `Cond{Versioned}` with nil version (must not exist) | `0000005da60018250150000102030405060708090a0b0c0d0e0f0201030707583e<record>11f5` |
| RefPut, `Cond{Versioned, ExpectedVersion: 010203}` | `00000062a70018250150000102030405060708090a0b0c0d0e0f0201030707583e<record>0e4301020311f5` |
| RefPut, `Cond{Keyed, ExpectedOld: 40..5f}` | `00000080a70018250150000102030405060708090a0b0c0d0e0f0201030707583e<record>0f5820404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f11f5` |
| RefPut, `Cond{Force}` | `0000005da60018250150000102030405060708090a0b0c0d0e0f0201030707583e<record>10f5` |
| RefDelete `trees/a`, Force | `00000025a60018260150000102030405060708090a0b0c0d0e0f02010307066774726565732f6110f5` |
| RefDelete, Versioned `010203` | `0000002aa70018260150000102030405060708090a0b0c0d0e0f02010307066774726565732f610e4301020311f5` |
| RefList, prefix `""` (omitted) | `0000001aa40018270150000102030405060708090a0b0c0d0e0f02010307` |
| RefList, prefix `trees/` | `00000022a50018270150000102030405060708090a0b0c0d0e0f02010307124674726565732f` |
| RefList, prefix `trees/`, after `trees/b` | `0000002ba60018270150000102030405060708090a0b0c0d0e0f02010307124674726565732f134774726565732f62` |
| RefWatch `trees/**`, empty known (Refs omitted) | `00000025a500182a0150000102030405060708090a0b0c0d0e0f02010307183c6874726565732f2a2a` |
| RefWatch `trees/**`, known `{trees/a: aa×32}` | `00000058a600182a0150000102030405060708090a0b0c0d0e0f020103071581a4006774726565732f61015820aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa02f60300183c6874726565732f2a2a` |
| RefWatch `trees/*`, known `{x: nil}` | `00000030a600182a0150000102030405060708090a0b0c0d0e0f020103071581a400617801f602f60300183c6774726565732f2a` |
| Admin `{Op:"gc-status"}` | `00000029a50018290150000102030405060708090a0b0c0d0e0f0201030718334ca1006967632d737461747573` |

In each RefPut row, substitute the record above for `<record>`.

Known-list order in Go follows map iteration and is random. With more than
one entry the request bytes are not deterministic in Go, and the server does
not care about order. Rust may sort by name.

### 3.3 Replies the client reads (verified hex)

Test view (10 map pairs): `ClusterID 00..0f, Incarnation 1, Epoch 7, Version 9, PlacementEpoch 7, Replicas 3, MinReplicas 2, Voters [{11×32, 1}], VoterSync 0, Nodes [{ID 11×32, Weight 100, Addrs ["ip:192.168.1.10:4433"], Writable true}]`:
```
aa0050000102030405060708090a0b0c0d0e0f0101020703090407050306020781a20058201111111111111111111111111111111111111111111111111111111111111111010108000a81a4005820111111111111111111111111111111111111111111111111111111111111111101186402817469703a3139322e3136382e312e31303a3434333307f5
```

| case | frame hex |
|---|---|
| `TViewReply {Inc 1, Epoch 7, View, Unreachable [22×32]}` | `000000bba5001830020103070c588b<view>181b8158202222222222222222222222222222222222222222222222222222222222222222` |
| `TErr stale-view "request epoch is behind" + View (stamped Inc 1, Epoch 8)` | `000000baa6000a020103080a6a7374616c652d766965770b77726571756573742065706f636820697320626568696e640c588b<view>` |
| `TErr unknown-ref "no such reference"` (unstamped) | `00000023a3000a0a6b756e6b6e6f776e2d7265660b716e6f2073756368207265666572656e6365` |
| `TErr busy "slow down" RetryAfter 1500` | `00000019a4000a0a64627573790b69736c6f7720646f776e181c1905dc` |
| `TRef {Record, Version 30..3f}` | `0000005ba50018340201030707583e<record>0d50303132333435363738393a3b3c3d3e3f` |
| `TOK {Key 01..20, Version 30..3f}` (ref-put) | `0000003da5001835020103070958200102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f200d50303132333435363738393a3b3c3d3e3f` |
| `TOK` (ref-delete) | `00000008a300183502010307` |
| `TCASMismatch {Version, HasCurrent, Record, Current}` | `00000082a70018360201030707583e<record>0d50303132333435363738393a3b3c3d3e3f181d58200102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20181ff5` |
| `TCASMismatch` absent (HasCurrent false) | `00000008a300183602010307` |
| `TIncomplete {Keys [01..20], Shortfall 5}` | `0000002fa500183702010307048158200102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20181e05` |
| `TRefs {2 refs (second without user), Next "trees/b"}` | `000000aca5001838020103071582a5006774726565732f610158200102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f200250303132333435363738393a3b3c3d3e3f031b17979cfe3d85cd150465616c696365a4006774726565732f6201582002030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202102503132333435363738393a3b3c3d3e3f40031b17979cfe3d85cd16164774726565732f62` |
| `TRefChanges {Refs [trees/a], Deleted [trees/gone]}` | `00000068a500183b020103071581a5006774726565732f610158200102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f200250303132333435363738393a3b3c3d3e3f031b17979cfe3d85cd150465616c696365183d816a74726565732f676f6e65` |
| `TRefSynced` | `00000008a300183c02010307` |
| `TAdminReply {Status: AdminReply{Text:"ok"}}` | `00000010a400183a02010307183945a100626f6b` |

In these rows, substitute the test view for `<view>` and the record from 3.2
for `<record>`.

### 3.4 Admin and status payloads

`node.AdminRequest` (admin.go:19-35). Every field is `omitempty` except `Op`:

| key | field | type |
|---:|---|---|
| 0 | Op | tstr |
| 1 | Node | bstr |
| 2 | Weight | uint32 |
| 3 | Zone | tstr |
| 4 | Replicas | uint8 |
| 5 | Dead | bool |
| 6 | AllowUnsafe | bool |
| 7 | Force | bool |
| 8 | Key | bstr |
| 9 | Garbage | float64 |
| 10 | Tolerate | bool |
| 11 | Forwarded | bool |
| 12 | Pause | bool |
| 13 | Rate | uint64 |
| 14 | Names | array tstr |

Verified: `{Op:"gc-status"}` = `a1006967632d737461747573`;
`{Op:"gc-run", Garbage:0.5, Tolerate:true}` = `a3006667632d72756e09f938000af5`;
`{Op:"gc-run", Garbage:1/3}` = `a2006667632d72756e09fb3fd5555555555555`;
`{Op:"node-weight", Node:11×32, Weight:100}` = `a3006b6e6f64652d7765696768740158201111111111111111111111111111111111111111111111111111111111111111021864`.

`node.AdminReply` (admin.go:38-46), all omitempty: `0 Text tstr`, `1 Token bstr`,
`2 View bstr`, `3 Names [tstr]`, `4 Key bstr`, `5 Ticket tstr`, `6 GC bstr`.
Verified `{Text:"ok"}` = `a100626f6b`.

`node.Status` (status.go:22-52):

| key | field | type | omitempty |
|---:|---|---|---|
| 0 | ID | bstr | no |
| 1 | Epoch | u64 | no |
| 2 | Incarnation | u64 | no |
| 3 | Packs | int | no |
| 4 | Records | u64 | no |
| 5 | Bytes | i64 | no |
| 6 | Pins | int | no |
| 7 | Unreachable | [bstr] | yes |
| 8 | PendingPacks | int | no |
| 9 | Transition | tstr | yes |
| 10 | GC | tstr | yes |
| 11 | LeaseHolder | bstr | yes |
| 12 | Voters | [VoterStat] | yes |
| 13 | Writable | bool | no |
| 14 | FreeBytes | i64 | no |
| 15 | TotalBytes | i64 | no |
| 16 | Puts | u64 | no |
| 17 | Gets | u64 | no |
| 18 | RefPuts | u64 | no |
| 19 | BytesIn | u64 | no |
| 20 | BytesOut | u64 | no |
| 21 | Amnesiac | bool | yes |
| 22 | Retired | bool | yes |
| 23 | ScrubAgeSec | i64 | yes |
| 24 | LastLive | u64 | yes |
| 25 | Corrupt | int | yes |
| 26 | IsHolder | bool | yes |
| 27 | UnauditedKeys | int | yes |
| 28 | Watchers | int | yes |

`VoterStat {0 ID bstr, 1 Calls u64, 2 Failures u64, 3 P99ms i64}` (status.go:14-19).

### 3.5 slog `TextHandler` line format (Go 1.26.5 `log/slog`)

Per record, one `Write` of:
`time=<T> level=<L> msg=<M>` then ` <key>=<value>` for each attribute, then `\n`.

- `time`: omitted when the record time is zero. Otherwise truncate to ms and
  format RFC3339 with exactly 3 fractional digits in the **local** zone:
  `2026-09-18T10:11:12.345Z` for a zero offset, `+02:00`/`-05:30`
  otherwise (handler.go:622-633).
- `level`: `DEBUG`, `INFO`, `WARN`, `ERROR`, with `+N`/`-N` for in-between
  levels (e.g. `WARN+2`).
- String values and `msg` are quoted with Go `strconv.Quote` when
  `needsQuoting` holds (text_handler.go):
  - the string is empty;
  - an ASCII byte is ` `, `=`, `"`, or a control character `< 0x20`.
    Backslash and DEL (0x7f) do **not** force quoting, and DEL is printed raw;
  - a non-ASCII rune is invalid UTF-8 / `U+FFFD`, `unicode.IsSpace`, or not
    `unicode.IsPrint`.
- `strconv.Quote`:
  - `\"` and `\\`; printable runes literal;
  - `\a \b \f \n \r \t \v`;
  - other bytes `< 0x20` and `0x7f` → `\xHH`; invalid UTF-8 bytes → `\xHH`;
  - other non-printable runes → `\uHHHH` or `\UHHHHHHHH`; lowercase hex.
- Int64/Uint64 → decimal; Bool → `true`/`false`; Float64 → `strconv.FormatFloat(f, 'g', -1, 64)`.
- Duration → `time.Duration.String()` (3.6).
- Any: an `error` gives `Error()` (then quoted if needed); `[]byte` gives
  `strconv.Quote` always.

Verified lines (the time fixed at 2026-09-18 10:11:12.345678901 UTC):
```
time=2026-09-18T10:11:12.345Z level=INFO msg=connected node=1a2b3c4d nodes=3 path=direct rtt=2ms
time=2026-09-18T12:11:12.345+02:00 level=INFO msg=connected node=1a2b3c4d nodes=3 path=none
time=2026-09-18T04:41:12.345-05:30 level=INFO msg=connected node=1a2b3c4d nodes=1 path=relay rtt=0s
time=2026-09-18T10:11:12.345Z level=WARN msg="watch: no node answered, retrying" pattern=trees/** in=1s
time=2026-09-18T10:11:12.345Z level=WARN msg="watch: stream idle, reconnecting" node=1a2b3c4d
time=2026-09-18T10:11:12.345Z level=WARN msg="watch: stream ended, reconnecting" node=1a2b3c4d error=EOF
time=2026-09-18T10:11:12.345Z level=WARN msg="watch: node refused, trying the next" node=1a2b3c4d error="remote: unavailable: no view"
time=2026-09-18T10:11:12.345Z level=INFO msg="watch: synced" pattern=trees/** node=1a2b3c4d refs=2
time=2026-09-18T10:11:12.345Z level=INFO msg=quoting empty="" space="a b" eq="a=b" quote="a\"b" backslash=a\b tab="a\tb" nl="a\nb" unicode=é nbsp="a b" ctrl="a\x01b" del=a<0x7f>b invalid="a\xffb" zwsp="a​b" u64=5 i64=-5 f=0.5 b=true bytes="hi"
time=2026-09-18T10:11:12.346Z level=ERROR msg="truncate ms"
level=DEBUG msg="zero time"
time=2026-09-18T10:11:12.345Z level=WARN+2 msg="warn plus two"
```
`<0x7f>` stands for the raw DEL byte. The ms line had 345.678901 ms + 999 µs
= 346.677901 ms, which truncates to `.346`.

`slog.Default()` (library use without a logger) goes through the `log`
package: `2026/09/18 12:11:12 INFO connected node=1a2b3c4d nodes=3 path=none`
(local time, seconds, no `time=`/`level=` keys). The CLI never uses it.

### 3.6 Go `time.Duration` strings

`String()`:
- `0` → `0s`; `<1µs` → `<n>ns`; `<1ms` → µs with the fraction trimmed of
  trailing zeros (`µ` is U+00B5); `<1s` → `ms` with a trimmed fraction;
- otherwise `[<h>h][<m>m]<s>[.frac]s`: hours appear when nonzero, minutes
  whenever hours or minutes are nonzero, and seconds always;
- a leading `-` for negatives.

`Round(m)`: halfway rounds away from zero.

Verified: `1ns`, `999ns`, `1µs`, `1.5µs`, `1.5ms`, `1s`, `30s`, `2m0s`, `1.5s`,
`26h0m3s`. Round to ms: `0`→`0s`, `400µs`→`0s`, `500µs`→`1ms`, `1.5ms`→`2ms`,
`2.5ms`→`3ms`, `99.4ms`→`99ms`, `1.234567s`→`1.235s`, `61s`→`1m1s`,
`3661s`→`1h1m1s`, `90m`→`1h30m0s`.

### 3.7 `HumanBytes` / `Rate` (verified)

`HumanBytes`: `0`→`0 B`, `1`→`1 B`, `1023`→`1023 B`, `1024`→`1.0 KiB`,
`1126`→`1.1 KiB`, `1177`→`1.1 KiB`, `1228`→`1.2 KiB`, `1280`→`1.2 KiB`,
`1331`→`1.3 KiB`, `1536`→`1.5 KiB`, `1792`→`1.8 KiB`, `10291`→`10.0 KiB`,
`10342`→`10.1 KiB`, `1048575`→`1024.0 KiB`, `1048576`→`1.0 MiB`,
`5<<30`→`5.0 GiB`, `1<<40`→`1.0 TiB`, `1<<50`→`1.0 PiB`, `1<<60`→`1.0 EiB`,
`MaxInt64`→`8.0 EiB`, `-1`→`-1 B`, `-2048`→`-2048 B`.

`Rate`: `(1MiB, 2s)`→`512.0 KiB/s`, `(0, 1s)`→`0 B/s`, `(5, 0)`→`-`,
`(100, -1s)`→`-`, `(1GiB, 3s)`→`341.3 MiB/s`, `(123456789, 1234ms)`→`95.4 MiB/s`.

### 3.8 CLI text that comes straight from this area (owned by the CLI spec)

- `refs`: `%s\t%x\t%s\t%s\n` (name, key, RFC3339 local created, user).
- `watch`: `%s\t%x\t%s\t%s\n`, or `%s\tdeleted\n`.
- `ref get`: `name %s\nkey %x\nversion %x\nuser %s\ncreated %s\n` (client.go:425).
- `cluster status` (client.go:134-193):
  - `cluster %x incarnation %d epoch %d version %d\n` with `ClusterID[:4]`;
  - `replicas %d min_replicas %d nodes %d voters %d`, plus
    ` (no catalog fault tolerance)` when voters < 3, then a newline;
  - optional `transition %d (%s): frozen=%v acked=%d participants=%d done=%d\n`;
  - optional `voter change in progress (target %s)\n`;
  - per node `  %s weight %d zone %q voter=%v writable=%v`. `Status` is
    called with a **5 s** timeout, node by node. On error the line gets
    ` — unreachable: <err>` (via `Println(line, "— unreachable:", err)`);
    on decode failure ` — bad status`;
  - then `      epoch %d packs %d records %d bytes %d pins %d pending-packs %d free %d GiB`
    plus ` [lease holder]`/` [AMNESIAC]`/` [retired]`;
  - `      cannot reach: <shortids joined by space>`;
  - `      gc: %s`;
  - `      transition: %s` (when not `""`/`idle`);
  - `      voter %s: %d calls, %d failures, p99 %d ms`.
- Any command error: stderr `dstore: <err>`, exit 1.

---

## 4. Rust design

### 4.1 Crate dependencies (all present offline in `~/.cargo/registry`)

| crate | version | use |
|---|---|---|
| `tokio` | 1.52 (`rt-multi-thread`, `macros`, `sync`, `time`, `io-util`) | runtime, timers, async mutexes |
| `tokio-util` | 0.7.17 | `sync::CancellationToken` for `Ctx` |
| `futures` | 0.3.31 | `Stream`, `future::join_all`, `BoxFuture` |
| `async-stream` | 0.3.6 | generator for `watch_refs` (pull semantics like Go range-over-func) |
| `iroh` | **=1.0.3** | QUIC endpoint (go-iroh v0.2.0 is verified wire-compatible with 1.0.3) |
| `iroh-mdns-address-lookup` | 0.4.0 | mDNS resolver (`advertise(false)` for clients) |
| `rand` | 0.9.2 | watch jitter |
| `hex` | 0.4.3 | `%x`, ShortID |
| `thiserror` | 2.0.17 | error enums |
| `chrono` | 0.4.42 | local-offset timestamps for the slog text format (`Local`, honours `TZ`) |
| `amber-store-core` | 0.3.0 (git/path rev `a85ffa1`) | `reference::Reference::decode`, `key::Key` |

The toolchain is nixpkgs rustc 1.95 (iroh 1.0.3 declares `rust-version = "1.91"`).

### 4.2 Module layout

```
src/ctx.rs                 Ctx: Go context equivalent (cancel + deadline, errors "context canceled"/"context deadline exceeded")
src/slog.rs                Level, Value, Attr, Record, Handler, Logger, TextHandler, DefaultHandler (Go log format)
src/gofmt.rs               duration_string (Go Duration.String), duration_round, go_quote (strconv.Quote), needs_quoting, is_print/is_space tables
src/transport/mod.rs       NodeId re-export, PathInfo, Endpoint + Conn traits, Stream halves, AddrsFn
src/transport/pool.rs      Pool (2.8)
src/transport/iroh.rs      IrohEndpoint/IrohConn (transport spec; Path() per 4.5)
src/transport/mem.rs       in-memory Network for tests (Path = {direct: true, rtt: 1ms})
src/client/mod.rs          Config, Cluster, dial, view cache, call/call_retry/any_node, status, admin, nodes, primary/owners/write_set/read_order
src/client/error.rs        Error
src/client/rank.rs         rtt_class, rank_owners
src/client/batch.rs        RecordSizer, batches
src/client/progress.rs     Progress, ProgressReport, NodeProgress, PutObserver, Tracker, count_keys, path_attrs, rate, human_bytes
src/client/refs.rs         Ref, Cond, CasMismatch, Incomplete, ref_get/ref_put/ref_delete/ref_list
src/client/watch.rs        RefChange, watch_refs
```
The codec, wire, view, placement and ticket modules come from their own specs.
This spec relies on: `wire::{Msg, RefInfo, RemoteError, read_msg, write_msg, T_*, CODE_*, ALPN_CLIENT, MAX_PUT_BATCH, MAX_PAGE_BYTES}`,
`view::{View, Placement, NodeId, short_id}`, `ticket::Ticket`.

### 4.3 Types and signatures

```rust
// ---- ctx.rs ----
#[derive(Clone)]
pub struct Ctx { token: tokio_util::sync::CancellationToken, deadline: Option<tokio::time::Instant> }
pub enum CtxError { Canceled, DeadlineExceeded }            // Display: "context canceled" / "context deadline exceeded"
impl Ctx {
    pub fn background() -> Ctx;
    pub fn with_cancel(&self) -> (Ctx, CancelHandle);          // child token
    pub fn with_timeout(&self, d: Duration) -> Ctx;            // deadline = min(parent, now + d), child token
    pub fn err(&self) -> Option<CtxError>;                     // Canceled if token cancelled, DeadlineExceeded if past deadline
    pub async fn done(&self);                                  // resolves on cancel or deadline
    pub async fn run<F: Future>(&self, f: F) -> Result<F::Output, CtxError>; // select!{done, f}
}

// ---- transport/mod.rs ----
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PathInfo { pub direct: bool, pub rtt: Duration }     // rtt ZERO = not measured
pub trait SendHalf: tokio::io::AsyncWrite + Send + Unpin { fn finish(&mut self); }          // Go Close/CloseWrite
pub trait RecvHalf: tokio::io::AsyncRead + Send + Unpin { fn cancel_read(&mut self, code: u32); }
pub struct Stream { pub send: Box<dyn SendHalf>, pub recv: Box<dyn RecvHalf> }
pub trait Conn: Send + Sync + 'static {
    fn remote_id(&self) -> NodeId;
    fn alpn(&self) -> &str;
    fn open_stream<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Stream, TransportError>>;
    fn accept_stream<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Stream, TransportError>>;
    fn close(&self);
    fn path(&self) -> PathInfo;
    fn is_closed(&self) -> bool;                                 // Go: Done() closed
}
pub trait Endpoint: Send + Sync + 'static {
    fn id(&self) -> NodeId;
    fn dial<'a>(&'a self, ctx: &'a Ctx, id: NodeId, addrs: Vec<String>, alpn: &'a str) -> BoxFuture<'a, Result<Arc<dyn Conn>, TransportError>>;
    fn accept<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Arc<dyn Conn>, TransportError>>;
    fn addrs(&self) -> Vec<String>;
    fn close<'a>(&'a self) -> BoxFuture<'a, ()>;
}
pub type AddrsFn = Arc<dyn Fn(NodeId) -> Vec<String> + Send + Sync>;

// ---- transport/pool.rs ----
pub struct Pool { /* ep, addrs, per_peer, std::sync::Mutex<State{conns, next, dialing: HashMap<Key, Arc<tokio::sync::Mutex<()>>>, failed: HashMap<Key, Instant>}> */ }
impl Pool {
    pub fn new(ep: Arc<dyn Endpoint>, addrs: AddrsFn, per_peer: usize) -> Pool;   // 0 → 1
    pub fn endpoint(&self) -> &Arc<dyn Endpoint>;
    pub async fn get(&self, ctx: &Ctx, id: NodeId, alpn: &str) -> Result<Arc<dyn Conn>, Error>;
    pub fn drop_peer(&self, id: NodeId, alpn: &str);                             // Go Drop
    pub fn path(&self, id: NodeId, alpn: &str) -> Option<PathInfo>;
    pub fn close(&self);
    pub async fn call(&self, ctx: &Ctx, id: NodeId, alpn: &str, req: &wire::Msg) -> Result<wire::Msg, Error>;
    pub async fn open(&self, ctx: &Ctx, id: NodeId, alpn: &str) -> Result<Stream, Error>;
}
pub fn close_stream(s: &mut Stream);   // send.finish(); recv.cancel_read(0)

// ---- client/error.rs ----
#[derive(Debug, Clone, thiserror::Error)]
pub enum Error {
    #[error("client: no endpoint")] NoEndpoint,
    #[error("client: ticket names no nodes")] TicketNamesNoNodes,
    #[error("client: no bootstrap node answered: {0}")] NoBootstrap(Box<Error>),
    #[error("client: unexpected reply {0}")] UnexpectedReply(i64),
    #[error("client: no nodes")] NoNodes,
    #[error("client: unknown reference")] UnknownRef,
    #[error("{0}")] CasMismatch(CasMismatch),
    #[error("{0}")] Incomplete(Incomplete),
    #[error("client: watch stream idle")] WatchIdle,
    #[error("{0}")] Remote(wire::RemoteError),                 // "remote: <code>[: <text>]"
    #[error("wire: unexpected frame: type {0}")] Protocol(i64),
    #[error("transport: peer recently unreachable")] PeerRecentlyUnreachable,
    #[error("{0}")] Ctx(CtxError),
    #[error("{0}")] Frame(wire::FrameError),                   // EOF / unexpected EOF / wire: short frame: … / …
    #[error("{0}")] View(view::DecodeError),                   // "view: decode: …"
    #[error("{0}")] Reference(amber_store_core::reference::Error),
    #[error("{0}")] Transport(Arc<TransportError>),
}
impl Error {
    pub fn remote(&self) -> Option<&wire::RemoteError>;       // looks through NoBootstrap (Go errors.As)
    pub fn is_code(&self, code: &str) -> bool;                // Go wire.IsCode
}

// ---- client/mod.rs ----
#[derive(Clone)]
pub struct Config {
    pub endpoint: Option<Arc<dyn Endpoint>>,
    pub ticket: ticket::Ticket,
    pub conns: usize,              // 0 → 4
    pub jobs: usize,               // 0 → 8
    pub logger: Option<slog::Logger>,
    pub gc_interval: Duration,     // ZERO → 4h
    pub request_timeout: Duration, // ZERO → 2min
    pub batch_bytes: usize,        // 0 → 16 MiB; then min(64 MiB)
    pub watch_idle: Duration,      // ZERO → 2min
}
#[derive(Clone)]
pub struct Cluster(Arc<Inner>);    // Inner { cfg, log, ep, pool, state: std::sync::RwLock<State> }
impl Cluster {
    pub async fn dial(ctx: &Ctx, cfg: Config) -> Result<Cluster, Error>;
    pub fn close(&self);
    pub fn view(&self) -> Option<Arc<view::View>>;
    pub fn placement(&self) -> Option<Arc<view::Placement>>;
    pub async fn refresh_view(&self, ctx: &Ctx) -> Result<(), Error>;
    pub fn primary(&self, key: &[u8; 32]) -> Option<NodeId>;
    pub fn owners(&self, key: &[u8; 32]) -> Vec<NodeId>;
    pub fn write_set(&self, key: &[u8; 32]) -> Vec<NodeId>;
    pub fn read_order(&self, key: &[u8; 32]) -> Vec<NodeId>;
    pub fn nodes(&self) -> Vec<NodeId>;
    pub async fn status(&self, ctx: &Ctx, id: NodeId) -> Result<Vec<u8>, Error>;
    pub async fn admin(&self, ctx: &Ctx, id: Option<NodeId>, params: Vec<u8>) -> Result<Vec<u8>, Error>; // params = AdminRequest CBOR
    pub async fn ref_get(&self, ctx: &Ctx, name: &str) -> Result<Ref, Error>;
    pub async fn ref_put(&self, ctx: &Ctx, record: Vec<u8>, cond: Cond) -> Result<Vec<u8>, Error>;
    pub async fn ref_delete(&self, ctx: &Ctx, name: &str, cond: Cond) -> Result<(), Error>;
    pub async fn ref_list(&self, ctx: &Ctx, prefix: &str) -> Result<Vec<wire::RefInfo>, Error>;
    pub fn watch_refs(&self, ctx: Ctx, pattern: String, known: HashMap<String, Vec<u8>>)
        -> Pin<Box<dyn Stream<Item = Result<RefChange, Error>> + Send + 'static>>;
    // crate-internal, used by part B
    pub(crate) async fn call(&self, ctx: &Ctx, id: NodeId, m: &mut wire::Msg) -> Result<wire::Msg, Error>;
    pub(crate) async fn call_retry(&self, ctx: &Ctx, id: NodeId, m: &mut wire::Msg) -> Result<wire::Msg, Error>;
    pub(crate) async fn any_node(&self, ctx: &Ctx, m: &mut wire::Msg) -> Result<wire::Msg, Error>;
    pub(crate) fn stamp(&self, m: &mut wire::Msg);
    pub(crate) fn handle_err(&self, id: NodeId, err: &Error);
    pub(crate) fn ok(&self, id: NodeId);
    pub(crate) fn penalty(&self, id: NodeId) -> i32;
    pub(crate) fn preferred(&self, ids: &[NodeId]) -> Vec<NodeId>;
    pub(crate) async fn probe_hinted(&self, ctx: &Ctx);
    pub(crate) fn path_attrs(&self, id: NodeId) -> Vec<slog::Attr>;
    pub(crate) fn pool(&self) -> &Pool;
    pub(crate) fn cfg(&self) -> &Config;
    pub(crate) fn log(&self) -> &slog::Logger;
}

// ---- client/rank.rs ----
pub(crate) fn rtt_class(rtt: Duration) -> i32;
pub(crate) fn rank_owners(ids: &[NodeId], penalty: impl Fn(NodeId) -> i32, path: impl Fn(NodeId) -> Option<PathInfo>) -> Vec<NodeId>; // Vec::sort_by is stable

// ---- client/batch.rs ----
pub type RecordSizer = Arc<dyn Fn(&[u8; 32]) -> usize + Send + Sync>;
pub(crate) fn batches(keys: &[[u8; 32]], size: &dyn Fn(&[u8; 32]) -> usize, max_bytes: usize, max_keys: usize) -> Vec<Vec<[u8; 32]>>;
pub(crate) const DEFAULT_BATCH_BYTES: usize = 16 << 20;
pub(crate) const BATCH_KEYS: usize = 8192;

// ---- client/progress.rs ----
pub type Progress = Arc<dyn Fn(ProgressReport) + Send + Sync>;
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProgressReport { pub objects: i64, pub total_objects: i64, pub bytes: i64, pub total_bytes: i64, pub nodes: Vec<NodeProgress> }
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NodeProgress { pub id: NodeId, pub direct: bool, pub rtt: Duration, pub in_flight: i64, pub awaiting: i64, pub bytes: i64 }
#[derive(Clone, Default)]
pub struct PutObserver {
    pub start: Option<Arc<dyn Fn(NodeId) + Send + Sync>>,
    pub sent: Option<Arc<dyn Fn(NodeId, usize) + Send + Sync>>,
    pub flushed: Option<Arc<dyn Fn(NodeId) + Send + Sync>>,
    pub done: Option<Arc<dyn Fn(NodeId, bool) + Send + Sync>>,
}
pub(crate) struct Tracker { /* cluster: Cluster, prog: Option<Progress>, state: std::sync::Mutex<(ProgressReport, HashMap<NodeId, NodeProgress>)> */ }
impl Tracker {
    pub(crate) fn new(c: &Cluster, prog: Option<Progress>) -> Arc<Tracker>;
    pub(crate) fn observer(self: &Arc<Self>) -> PutObserver;
    pub(crate) fn totals(&self, objects: i64, done: i64, bytes: i64);
    pub(crate) fn more(&self, bytes: i64);
    pub(crate) fn objects(&self, n: i64);
    pub(crate) fn bytes(&self) -> i64;
}
pub(crate) fn count_keys(m: &HashMap<NodeId, Vec<[u8; 32]>>, size: &dyn Fn(&[u8; 32]) -> usize) -> (i64, i64);
pub fn rate(bytes: i64, took: Duration) -> String;   // took.is_zero() → "-"
pub fn human_bytes(n: i64) -> String;

// ---- client/refs.rs ----
pub struct Ref { pub name: String, pub record: Vec<u8>, pub version: Vec<u8>, pub reference: amber_store_core::reference::Reference }
#[derive(Clone, Debug, Default)]
pub struct Cond { pub expected_version: Vec<u8>, pub versioned: bool, pub expected_old: Vec<u8>, pub keyed: bool, pub force: bool }
#[derive(Clone, Debug)]
pub struct CasMismatch { pub current: Vec<u8>, pub record: Vec<u8>, pub version: Vec<u8>, pub has_current: bool }   // Display per 2.10
#[derive(Clone, Debug)]
pub struct Incomplete { pub sample: Vec<[u8; 32]>, pub shortfall: i64 }                                           // Display "incomplete: {} keys short"

// ---- client/watch.rs ----
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RefChange { pub name: String, pub key: Option<Vec<u8>>, pub version: Vec<u8>, pub created_at: i64, pub user: String, pub deleted: bool, pub synced: bool, pub node: NodeId }

// ---- slog.rs ----
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)] pub struct Level(pub i32);   // DEBUG=-4 INFO=0 WARN=4 ERROR=8
pub enum Value { String(String), Int64(i64), Uint64(u64), Float64(f64), Bool(bool), Duration(i64 /* ns */), Any(String /* Display of error */), Bytes(Vec<u8>) }
pub struct Attr { pub key: String, pub value: Value }
pub struct Record { pub time: Option<chrono::DateTime<chrono::Local>>, pub level: Level, pub message: String, pub attrs: Vec<Attr> }
pub trait Handler: Send + Sync { fn enabled(&self, level: Level) -> bool; fn handle(&self, r: &Record); }
#[derive(Clone)] pub struct Logger(Arc<dyn Handler>);
impl Logger { pub fn new(h: Arc<dyn Handler>) -> Self; pub fn default_logger() -> Self; pub fn log(&self, level: Level, msg: &str, attrs: Vec<Attr>); /* info/warn/error/debug */ }
pub struct TextHandler<W: std::io::Write + Send> { level: Level, w: std::sync::Mutex<W> }   // format of 3.5, one write per record
```

`Logger::log` checks `enabled` before building the record, as Go's `Logger.log` does.

### 4.4 Behavioural mapping notes

- **Go context → `Ctx`.** Go code checks `ctx.Err()`, nests timeouts (a
  child never extends the parent) and returns the ctx error text. `Ctx`
  mirrors `context.WithTimeout`/`WithCancel`. Every blocking step is
  wrapped in `ctx.run(...)` so that `Pool::call` returns
  `Error::Ctx(DeadlineExceeded)` exactly where Go returns `ctx.Err()`.
  Dropping futures alone would lose the "fail and continue with the next
  node" behaviour of `any_node` and the ctx texts.
- **`go c.RefreshView(context.Background())`** → `tokio::spawn(async move { let _ = c.refresh_view(&Ctx::background()).await; })`.
  `Cluster` must be used inside a tokio runtime.
- **Locks**: `std::sync::RwLock`/`Mutex`, never held across `.await`. The
  per-peer dial lock is a `tokio::sync::Mutex` (Go's is not ctx-aware;
  honouring ctx while waiting is an acceptable improvement).
- **`rank_owners`**: sort a `Vec<(pen, relay, class, pos, id)>` with
  `sort_by_key`, which is stable. Equality on `pos` never occurs.
- **Map iteration order**: Go iterates `bootAddrs` and the known list
  randomly. Rust may use ticket order and name order. Every Go ordering is
  compatible because the order is random there.
- **`watch_refs`**: implement with `async_stream::try_stream!`/`stream!`,
  with `watch_once` inlined into the generator body (a nested async fn
  cannot `yield`). This gives Go's pull semantics: nothing runs while the
  consumer is not polling.
  - The frame reader must be a spawned task owning the `RecvStream` and
    feeding an `mpsc::channel(1)`, because `read_msg` is not cancel-safe
    inside `select!`.
  - `abandon()` = abort the reader task (dropping `RecvStream` sends
    STOP_SENDING 0) and finish the send half.
  - "yield returned false" = the consumer dropped the stream. The generator
    is dropped at its `yield` point, which drops both halves: the same wire
    effect as abandon + CloseStream. The pool connection is **not** dropped,
    as in Go.
  - For the fatal case, yield `Err(e)` and return.
  - Idle timer: `tokio::time::sleep` re-armed on each frame.
- **Jitter**: `rand::rng().random_range(0..=delay_ns/2)` nanoseconds.
- **Progress callback** runs under the tracker's `std::sync::Mutex`. Document
  that it must not re-enter the tracker.

### 4.5 Mapping onto Rust iroh 1.0.3 (sources checked)

- **Endpoint** (iroh-1.0.3/src/endpoint.rs, Builder 180-840): `Endpoint::builder(presets::Minimal)`
  (or `Empty` plus explicit parts) with:
  - `.secret_key(SecretKey::generate(..))` and `.alpns(vec![])` for a client;
  - `.relay_mode(RelayMode::Default | RelayMode::Disabled | RelayMode::Custom(map))` (endpoint.rs:1925-1946);
  - `.address_lookup(MdnsAddressLookup::builder().advertise(false))`
    (iroh-mdns-address-lookup-0.4.0/src/lib.rs:161-215) when discovery is on;
  - `.address_lookup(DnsAddressLookup::n0_dns())` (src/address_lookup/dns.rs:92)
    when relays are on as well;
  - `.transport_config(QuicTransportConfig::builder().keep_alive_interval(5s).max_idle_timeout(Some(60s.try_into()?)).max_concurrent_bidi_streams(1024u32.into()).initial_rtt(Duration::from_millis(333)).build())`
    (src/endpoint/quic.rs:176, 211, 281, 365);
  - `.bind().await`, then `endpoint.online()` bounded by 10 s when relays
    are on (endpoint.rs:1358).
- **Dial**: `endpoint.connect(EndpointAddr::from_parts(id, addrs), b"amber-dstore/1").await`
  (endpoint.rs:1052-1089). It waits for the handshake to complete, so
  go-iroh's `awaitHandshake` 0-RTT guard is unnecessary. The address
  lookup is used when the given addresses fail and no relay URL is given
  (doc at endpoint.rs:1037-1045). Emulate the "direct 2 s, then relay,
  then discovery" order with separate connect attempts (transport spec).
  `TransportAddr` Display is `relay:{url}` / `ip:{addr}` / `custom:{addr}`
  (iroh-base-1.0.3/src/endpoint_addr.rs:81-89), matching the view's
  `ip:`/`relay:` strings.
- **Connection** (src/endpoint/connection.rs):
  - `open_bi()` (885) → `(SendStream, RecvStream)`;
  - `close(0u32.into(), b"")` (959);
  - `close_reason().is_some()` (925) = Go `Done()` closed;
  - `remote_id()` (1127); `paths()` (1144).
- **Streams** (noq-1.1.1):
  - `SendStream::finish()` (send_stream.rs:188) = Go `Close`/`CloseWrite`;
  - `RecvStream::stop(VarInt::from_u32(0))` (recv_stream.rs:275) = `CancelRead(0)`;
  - dropping a `RecvStream` that was not fully read calls `stop(0)`
    (recv_stream.rs:605-630);
  - dropping a `SendStream` calls `finish()`, or `reset` if the peer stopped
    it (send_stream.rs:350-374);
  - so `close_stream` can simply drop both halves.
- **`Conn::path()`** (path_watcher.rs:356-505): iterate `conn.paths()` and
  take the `Path` with `is_selected()`. `direct = !p.is_relay()`,
  `rtt = p.rtt()`. `Path::rtt()` returns `stats().rtt` =
  `RttEstimator::get()` = `smoothed.unwrap_or(latest)`, and before any sample
  `latest = initial_rtt` = 333 ms (noq-proto-1.1.1/src/connection/paths.rs:760-804;
  config/transport.rs:564). There is **no has-sample flag**. Set
  `initial_rtt` explicitly to 333 ms and map `rtt == initial_rtt` to 0
  ("not measured"), matching go-iroh's `HasRTT`. With no selected path,
  return `{direct: true, rtt: 0}` like Go.

### 4.6 Mapping onto core-rs (amber-store-core 0.3.0, rev a85ffa1)

- `RefGet` decodes with `amber_store_core::reference::Reference::decode(&[u8]) -> Result<Reference, reference::Error>`
  (src/reference.rs:394-406). Fields `name`, `key`, `user`, `created_at`,
  `signature`, `public_key` (317-330). Its `Display` strings reproduce Go's
  verbatim (port-notes/reference.md). The error is returned unwrapped, as Go does.
- core-rs `cbor.rs` offers only primitives (`append_head` 125, `append_bstr` 145,
  `read_head` 157, `read_bstr` 200). Struct codecs for `Msg`, `RefInfo`,
  `View`, `AdminRequest`, `AdminReply` and `Status` live in this crate
  (codec spec).

### 4.7 Testing strategy

- Pure unit tests: `rank_owners`, `batches`, `human_bytes`/`rate`, Go
  duration and quoting helpers, slog lines (3.5), `adopt` ordering,
  `handle_err` backoff sequence, `penalty`, `Cond` → `Msg` encoding
  (3.2 hex), reply decoding (3.3 hex).
- Pool and cluster logic over `transport::mem` with a scripted fake node:
  - `any_node`: a remote error stops, a transport error moves on, and
    stale-view is retried up to 5 times;
  - `call_retry` makes 4 attempts;
  - `ref_list` paging stop conditions;
  - pool growth to `per_peer` then round-robin, and "recently unreachable"
    within 2 s;
  - watch: known-list request, idle reconnect, served → 200 ms pause,
    delay doubling with jitter bounds, terminal `bad-request`, stale-view
    retry.
- Interop: a Go 3-node cluster (`dstore cluster init`/`node join`) with the
  Rust client running the watch scenarios of `node/watch_test.go`.

---

## 5. Golden vectors a Go generator should emit

A generator under `tools/vectorgen` (Go, module with
`replace github.com/amber-store/dstore => <checkout>`, run with Go 1.26.5,
`GOTOOLCHAIN=local GOPROXY=off GOFLAGS=-mod=mod`) should emit JSON with:

1. **Request frames** (3.2): every row, via `wire.WriteMsg` on
   `&wire.Msg{...}`, stamped as in 3.2. Also a RefWatch with 2 known entries
   whose `Refs` slice is built in a fixed order (Go's map order is random).
2. **Reply frames** (3.3): every row, plus decoded expectations for each:
   - the fields `RefGet`, `RefPut`, `RefDelete` and `RefList` extract;
   - `CASMismatch` / `Incomplete` `Error()`;
   - `ErrorFromMsg(...).RetryAfter.String()`.
3. **Error strings** (2.14): `(&wire.Error{Code:"busy"}).Error()` =
   `remote: busy`; `{unavailable, "no view"}` = `remote: unavailable: no view`;
   the CASMismatch variants (absent, current, current nil); `incomplete: 3 keys short`;
   `client: unknown reference`; `wire: unexpected frame: type 99`;
   `client: no bootstrap node answered: client: ticket names no nodes`;
   the ReadMsg errors of 2.14.
4. **AdminRequest/AdminReply CBOR** (3.4), including float cases `0.5`, `1/3`,
   `1.5`, `100.0`, `1e-7`.
5. **`HumanBytes`/`Rate`** (3.7) and **Duration** strings and ms rounding (3.6).
6. **slog lines** (3.5): build `slog.NewRecord(t, level, msg, 0)` with fixed
   times in `UTC`, `+02:00` and `-05:30`, call `h.Handle` directly, and
   include the quoting line and a level offset.
7. **`rankOwners` scenarios**: exported through a small test-only file
   copied into the generator (the function is unexported). Inputs are ids,
   penalties and paths; output is the order. Cover the 7 rank_test cases plus:
   - class boundaries at 4.999 ms / 5 ms / 24.999 ms / 25 ms / 99.999 ms /
     100 ms;
   - penalty 1 (hint) vs 2 (backoff) vs 3;
   - relayed but near vs direct but far.
8. **`batches` scenarios**: the 4 batch_test cases plus a boundary where
   `bytes+n == maxBytes` (it stays in the same batch).
9. **Backoff sequence**: durations after failures 1-7:
   `5s,10s,20s,40s,60s,60s,60s` (computed by the same formula).
10. **Progress snapshot**: a tracker driven through
    `Start/Sent/Flushed/Done` for two nodes whose ids sort reversed from
    first use. Emit each report. The Nodes order is by id, and Direct/RTT
    are false/0 without a pool.

---

## 6. Go tests worth porting

| test | file | port as |
|---|---|---|
| `TestRankOwnersKeepsRankAmongUnmeasuredOwners` | client/rank_test.go:29 | unit |
| `TestRankOwnersDoesNotDemoteUnmeasuredOwnersBehindAMeasuredOne` | :37 | unit |
| `TestRankOwnersPrefersDirectOverRelayed` | :49 | unit |
| `TestRankOwnersPrefersUnmeasuredOverRelayed` | :61 | unit |
| `TestRankOwnersTiesNearRoundTripsByRank` | :70 | unit |
| `TestRankOwnersPrefersAMuchNearerOwner` | :85 | unit |
| `TestRankOwnersPutsPenalisedOwnersLast` | :97 | unit |
| `TestBatchesBalancesBySizerNotByKeyLength` | client/batch_test.go:13 | unit |
| `TestBatchesCapsKeysPerBatch` | :22 | unit |
| `TestBatchesSendsAnOversizedRecordAlone` | :29 | unit |
| `TestBatchesKeepsOrder` | :37 | unit |
| `TestClusterWatchRefs` | node/watch_test.go:98 | interop against Go nodes (initial difference with stale key and vanished name, hints from every coordinator, deletion, same key is not a change, outside-pattern silence, cancel ends cleanly) |
| `TestClusterWatchBadPattern` | :177 | interop / fake node (`trees/[` → error `is_code("bad-request")`) |
| `TestClusterWatchLostHint` | :198 | interop (needs partitions; Go harness only) |
| `TestClusterWatchReconnect` | :222 | interop or fake node (`WatchIdle 5s`; the resynced node differs from the downed one; `trees/a` is not re-sent) |
| `TestClusterPushProgress` | node/cluster_test.go:455 | shared with part B (monotone Bytes, `TotalObjects == Keys`, final `Objects==TotalObjects`, `Bytes==TotalBytes==stats.Bytes`, zero InFlight/Awaiting, node byte sum, every node talked to) |
| `TestClusterNodeDownDuringWrite` | :423 | part B, exercises backoff and preference |
| `TestErrorFrames` | wire/wire_test.go:31 | unit (Error carries View; `is_code`; `remote()`) |
| `TestIrohPathRTTIsUnknownUntilSampled` | transport/iroh_test.go:213 | transport integration (validates the 333 ms sentinel mapping) |
| `TestIrohDialWaitsForHandshake` | :97 | transport integration |
| `TestIrohDiscoverByID` | :277 | transport integration (id-only bootstrap) |

Rust-only tests to add, since no Go test covers them:
- `adopt` ordering `(inc, epoch, version)`;
- `adoptReply` replaces hints even when it rejects an older view;
- `handleErr` does not penalise remote errors but adopts their view;
- `anyNode` semantics (2.6);
- `RefList` stop on empty `Next` or empty `Refs`;
- Pool growth, round-robin and the 2 s recently-unreachable window;
- `Status` returns bytes without a type check;
- watch delay sequence and 200 ms pause;
- `pathAttrs` forms.

---

## 7. Gaps in core-rs / Rust iroh and workarounds

1. **No struct CBOR codec in core-rs** (only head/bstr helpers). Needed for
   `Msg`, `RefInfo`, `View`, `AdminRequest`/`AdminReply`, `Status`.
   *Workaround*: a `codec` module in this crate on top of
   `amber_store_core::cbor` primitives. It must provide:
   - omitempty rules and `null` for nil byte slices in `RefInfo` (keep
     `Option<Vec<u8>>` for `RefInfo.key`/`version` so null and empty stay
     distinct);
   - shortest float encoding;
   - lax decoding that ignores unknown keys (quiet duplicates, 131072
     limits).
2. **No slog-compatible logging in core-rs** (core-rs inbox chose a callback).
   *Workaround*: `src/slog.rs` as in 4.3, with the 3.5 format.
3. **No Go formatting helpers**: `time.Duration.String`, `Duration.Round`,
   `strconv.Quote`, `unicode.IsPrint`/`IsSpace`. No Unicode
   general-category crate is available offline (only `unicode-ident`,
   `-xid`, `-width`, `-segmentation`, `-normalization`). *Workaround*: port
   the Duration code directly. Generate range tables for Go's
   `unicode.IsPrint` and `unicode.IsSpace` with a Go program into
   `src/gofmt_tables.rs`.
4. **Rust iroh has no "RTT measured" flag** (`Path::rtt()` returns the
   333 ms initial guess before a sample). *Workaround*: set `initial_rtt`
   explicitly and treat equality as unmeasured (4.5). Verify with a port of
   `TestIrohPathRTTIsUnknownUntilSampled`.
5. **Rust mDNS lookup runs for a fixed 10 s** (`LOOKUP_DURATION`,
   iroh-mdns-address-lookup-0.4.0/src/lib.rs:95) vs go-iroh's configured
   3 s. *Workaround*: bound id-only discovery dials with a timeout in the
   transport layer. Accept different timing only if the transport spec
   decides so.
6. **Transport error texts differ** (go-iroh's `dial ip:…: …`, `iroh: no
   reachable address for endpoint`). *Workaround*: keep the Go wrapping
   layout (`dial %s: %w`, newline-joined, `discovery: %w`) around iroh's
   errors. Inner texts cannot match, so document the deviation.
7. **Go range-over-func iterators and contexts** have no direct Rust
   equivalent. *Workaround*: `async-stream` and `Ctx` (4.4).
8. **go-iroh `Endpoint.Connect` returns at the 0-RTT window** with a cached
   ticket. Rust `connect` waits for the handshake, so this is not a gap.
   Do not use `into_0rtt`.
9. **Node-side `--store` ticket derivation** needs a Pebble reader (1.3).
   Nothing in core-rs or iroh covers it.

---

## 8. Risks and open decisions

### 8.1 Code vs architecture (port the code)

- Pool: architecture §11.3 says "grow under load and shrink after ~90 s idle"
  and "total cap is at least one control connection per node". The code has
  a fixed `perPeer = Conns`, growth on the first `Conns` requests, and no
  shrink or eviction (2.8).
- Measurement aging after a minute (§11.1) is not implemented.
- Stale-view retry counts: §11.1 says "at most R times per key". The code
  does 4 attempts (`callRetry`), 5 per node (`anyNode`) and 4 per node
  (watch).
- §3 says writes and reference operations are refused with `stale-view`. On
  the client ALPN only `ref-put` (and part B's data ops) run `checkEpoch`.
  `ref-get`, `ref-delete`, `ref-list`, `ref-watch`, `view`, `status` and
  `admin` never answer `stale-view`. A wrong `cluster_id` does not refuse a
  ref-put.
- Tickets: the client never validates `ClusterID`/`Incarnation` against the
  adopted view.

### 8.2 Open decisions

1. **`Ctx` vs plain future cancellation.** Recommendation: an explicit `Ctx`
   for faithful error texts and loop behaviour (2.6, 2.11). The CLI then
   maps SIGINT to `Ctx` cancellation, exactly like `signal.NotifyContext`.
2. **Watch implementation**: `async-stream` generator (pull, faithful) or a
   spawned task with a channel (push, read-ahead of one event, simpler
   `Send` story). Recommendation: the generator.
3. **iroh version**: pin `=1.0.3` (the compatibility matrix is for 1.0.3)
   or move to 1.1.0 (CustomAddr ticket format changed; irrelevant to dstore
   `ip:`/`relay:` strings). Recommendation: 1.0.3.
4. **Node-side `--store` fallback** (`cluster status --store`,
   `cluster ticket --store`, every `dialCluster` without `--ticket`). Options:
   - (a) unsupported: print an error such as
     `no cluster: set --ticket or $DSTORE_TICKET` when `--store` is given;
   - (b) a read-only Pebble reader for `<store>/meta` key `view` (no Rust
     Pebble exists; SSTable and WAL parsing required; Go also writes
     `store_id` and opens paxos, which Rust would skip);
   - (c) exec a Go helper.
   Recommendation: (a) for v1, recorded as a documented incompatibility.
5. **Error text byte-compatibility for transport failures**: cannot be exact
   (7.6). Decide whether golden CLI tests exclude them.
6. **Logging implementation**: custom `slog` module vs `tracing` with a
   custom formatter. Recommendation: custom, since exact attribute kinds
   and order matter for the text handler and the TUI's `bytes` humanising.
7. **`RefInfo` optional bytes**: `Option<Vec<u8>>` (faithful null vs empty)
   vs `Vec<u8>` (simpler; empty encodes as null). Coordinate with the codec
   spec.
8. **Default logger** when `Config.logger` is `None`: replicate Go's
   `log`-package format (3.5, last paragraph) or discard. Recommendation:
   replicate.

### 8.3 Risks

- **Unicode quoting tables**: an approximation diverges only for non-ASCII
  space or non-printable characters in log values (pattern and ref names).
  Use generated Go tables (7.3).
- **Local time zone**: slog and `refs`/`watch` output use the local zone.
  chrono's `Local` must honour `TZ` like Go. Test with `TZ=UTC` and a named
  zone.
- **RTT sentinel**: if noq resets `latest` from a PATH_CHALLENGE RTT on a new
  path (paths.rs:540, 793), a non-sampled path reports that value rather
  than 333 ms. That is a real measurement, so ranking stays sensible, but it
  differs from go-iroh's `HasRTT=false` for that window.
- **Progress callback under lock**: a Rust callback that blocks or re-enters
  deadlocks the transfer. Document it.
- **Unbounded async refreshes** (`call` spawns one per stale reply) can pile
  up under a burst of replies carrying a newer epoch. This is faithful to Go,
  but consider single-flight; it is not observable on the wire beyond fewer
  `view` requests.
- **Watch idle race**: a consumer slower than `WatchIdle` triggers a
  reconnect. Faithful to Go; document it for library users.
- **`anyNode` after cancellation** penalises every remaining node. Replicate,
  otherwise error texts and backoff state diverge.
- **`Status` without a type check**: an unexpected reply type yields empty
  bytes, so the CLI prints `— bad status`. Replicate.
- **fxamacker decoder limits**: a known list above 131072 entries is refused
  by Go nodes (decode error → the node drops the stream without replying).
  The client sees `EOF` and reconnects forever. Faithful, but worth a
  client-side guard only if the project accepts a deviation.

---

## Addenda (synthesis)

Added by the architecture synthesis. `PORTING.md` is normative where it differs from this spec.

1. **Superseded.**
   - §4.1: the `iroh-mdns-address-lookup` and `chrono` dependencies.
   - §4.5: `.address_lookup(MdnsAddressLookup::builder()…)`, `DnsAddressLookup::n0_dns()` and
     `RelayMode::Default`.

   Use the ported go-iroh mDNS resolver, explicit discovery, the canary relay map, and the libc local
   zone (`gocompat::time::SystemZone`) (PORTING C1-C3, C12).
2. **Unified signatures** (PORTING.md §4.1, §4.8):
   - `Ctx::with_cancel(&self) -> Ctx` plus `Ctx::cancel()`;
   - `Pool::new(.., per_peer: usize)`;
   - `Progress = Arc<dyn Fn(&ProgressReport) + Send + Sync>` with `i64` counts;
   - `admin(ctx, id: Option<NodeId>, req: &dstore_wire::AdminRequest)`;
   - `ref_list(ctx, prefix: &[u8])`;
   - `WatchStream` type alias;
   - slog lives in `dstore-gocompat` with `Handler::handle(handler_attrs, record)` and
     `Value::{String, Int64, Uint64, Float64, Bool, Duration, Time, Bytes, Any}`.
3. **Open decisions resolved.**
   1. Explicit `Ctx`.
   2. An async-stream generator for `watch_refs`.
   3. iroh `=1.0.3`.
   4. `--store` fallback per PORTING §2.2 B.
   5. CLI golden tests compare the dstore-level wrapper texts but not inner QUIC texts (DD-4).
   6. The custom slog facade.
   7. `RefInfo.key` and `version` are `Option<Vec<u8>>`.
   8. `Logger::default_logger()` replicates Go's log-package format.
4. **New helpers** `dstore_client::validate_name_bytes` and `validate_user_bytes` apply Go
   `reference.ValidateName`/`ValidateUser` to raw argv bytes (length and UTF-8 checks before the core-rs
   `&str` validators).
5. **New module `dstore_client::corefmt`** (`walk_error_text`, `cbor_error_text`) gives Go `%q`
   names and `cborx:` prefixes for core-rs errors that reach stderr (core-rs-gaps G4).
6. **Root CLI entry** (`dstore_cli::main_entry`): SIGPIPE set to default, flush, `process::exit`
   without waiting for background `refresh_view` tasks (PORTING §5.9).
