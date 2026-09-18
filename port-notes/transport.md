# Transport layer: Go `dstore/transport` and its mapping onto Rust iroh

Normative Go reference: `github.com/amber-store/dstore` v0.1.9 (HEAD 368f2c7) at
`/Users/dragan/amber-store/dstore`, go-iroh v0.2.0 at
`/Users/dragan/go/pkg/mod/github.com/tmc/go-iroh@v0.2.0`.
Rust sources checked: `iroh-1.0.3`, `iroh-1.1.0`, `iroh-base-1.0.3/1.1.0`,
`iroh-relay-1.0.3/1.1.0`, `iroh-dns-1.0.3`, `noq-1.1.1/1.2.0`,
`noq-proto-1.1.1/1.2.0`, `iroh-mdns-address-lookup-0.4.0`, `swarm-discovery-0.6.3`
under `/Users/dragan/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f`.
Paths below are relative to those roots. Verified Go outputs (section 5) were
produced with go1.26.5 (`go run` in a throwaway module, since deleted).

---

## 1. Scope

### 1.1 dstore files covered

| file | lines | what it does |
|---|---:|---|
| `transport/transport.go` | 243 | `Stream`/`Conn`/`Endpoint` interfaces, `PathInfo`, `ErrClosed`, `AddrsFunc`, and `Pool` (per-(peer, ALPN) connection pool with round robin, failed-dial backoff, `Call`, `Open`). |
| `transport/iroh.go` | 457 | `IrohConfig`, `BindIroh`, discovery setup (mDNS, number0 DNS, pkarr publisher), `Addrs`, `ParseAddrs`, `Dial` (direct first, then relay, then discovery), `raceConnect`, `awaitHandshake`, `irohConn` (`Path`). |
| `transport/ifaces.go` | 55 | `interfaceIPs`: dialable unicast interface addresses, skipping container/virtual bridges. |
| `transport/mem.go` | 365 | In-process `Network` with partitions, down nodes and delay; `MemEndpoint`, `memConn`, `memStream`, 4 MiB `pipe`. |
| `transport/iroh_test.go` | 357 | 4 real-iroh tests (loopback exchange, handshake wait with 0-RTT, RTT unknown until sampled, discover by id). |

Callers that fix the client-side semantics:

- `cmd/dstore/client.go:39-96`: `dialCluster`, `netOpts`, `dialTicket` (ephemeral key, `BindIroh`, `client.Dial`, endpoint closed only when `client.Dial` fails).
- `cmd/dstore/main.go:80-137`: `clientFlags`, `noDiscoveryFlag`, `netFlags`, `relayMode`, `relayModeOf`; `main.go:161-207`: `bindNodeEndpoint` (node side); `main.go:394-405`: `localTicket` (node side).
- `cmd/dstore/wc.go:27-98`: working-copy flags, `wcConfig`, `dialConfig` → `dialTicket`.
- `client/client.go:22-117` (`Config.Conns` default 4, `Dial` with 15 s per bootstrap member), `:169-178` `addrsOf`, `:201-239` backoff/penalty, `:245-263` `probeHinted` (3 s), `:266-281` `call` (RequestTimeout 2 min), `:297-301` `preferred` → `pool.Path`.
- `client/rank.go:17-71` (consumes `PathInfo`), `client/progress.go:149-159` `pathAttrs`.
- `client/objects.go:245-300` (put stream), `client/fetch.go:274-309` (get stream), `client/watch.go:127-245` (watch stream, `abandon`).
- `wire/wire.go:23-25` (ALPNs), `:247-285` (`WriteMsg`/`ReadMsg`), `:390-395` (`CloseStream`).
- `node/server.go:16-101` (what the Go peer does with a stream), `node/node.go:245` (`NewPool(..., 2)`), `node/cluster_test.go:55-185` (mem harness incl. a client).
- `cmd/dstore/tui.go:262-270` (path/rtt columns).

### 1.2 go-iroh v0.2.0 APIs covered

`netaddr/endpointaddr.go` (371), `netaddr/relayurl.go` (108), `relay/relay.go` (269),
`iroh/defaults.go` (40), `iroh/endpoint.go` (1867), `iroh/conn.go` (833),
`iroh/zerortt.go` (196), `iroh/addresslookup.go` (354), `iroh/addresslookup_dns.go` (113),
`iroh/addresslookup_pkarr.go` (359), `iroh/mdns/mdns.go` (606), `iroh/mdns/dnsmsg.go` (438),
`iroh/mdns/reuse_unix.go`, `key/key.go`, `internal/socket/remote_state.go`,
`internal/socket/relay_actor.go`, and the vendored quic-go fork `internal/qng`
(stream close/cancel, flow-control defaults, handshake timeouts, error texts).

### 1.3 Out of scope here but recorded

Node-side transport needs (`serve`, `cluster init`, `node join`, `catalog restore`,
deriving a ticket from `--store`) are listed in 4.10.

---

## 2. API used client-side

### 2.1 Interfaces (`transport/transport.go:18-61`)

```go
type Stream interface {
    io.ReadWriteCloser
    CloseWrite() error        // finishes the send side
    CancelRead(code uint64)   // abandons the receive side
}
type PathInfo struct {
    Direct bool
    RTT    time.Duration // 0 until the path has a measurement
}
type Conn interface {
    RemoteID() view.NodeID
    ALPN() string
    OpenStream(ctx context.Context) (Stream, error)
    AcceptStream(ctx context.Context) (Stream, error)
    Close() error
    Path() PathInfo
    Done() <-chan struct{}    // closed when the connection is closed
}
type Endpoint interface {
    ID() view.NodeID
    Dial(ctx context.Context, id view.NodeID, addrs []string, alpn string) (Conn, error)
    Accept(ctx context.Context) (Conn, error)
    Addrs() []string          // dialable addresses in the view's string form
    Close() error
}
var ErrClosed = errors.New("transport: closed")
type AddrsFunc func(id view.NodeID) []string
```

`view.NodeID` is `[32]byte`. ALPNs (`wire/wire.go:23-25`): `ALPNClient = "amber-dstore/1"`,
`ALPNCluster = "amber-dstore-cluster/1"`, `ALPNGateway = "amber-store-iroh/1"`. The client
only dials `ALPNClient`.

### 2.2 `Pool` (`transport/transport.go:63-243`)

State: `ep`, `addrs AddrsFunc`, `perPeer int`, one mutex over
`conns map[poolKey][]Conn`, `next map[poolKey]int`, `dialing map[poolKey]*sync.Mutex`,
`failed map[poolKey]time.Time`. `poolKey{id view.NodeID; alpn string}`.

`NewPool(ep, addrs, perPeer)` (`:83-88`): `perPeer <= 0` becomes 1. The client passes
`cfg.Conns` (default 4, `client/client.go:63-65,91`); the node passes 2 (`node/node.go:245`).

`Get(ctx, id, alpn)` (`:94-146`), exactly in this order:
1. Lock. Filter `conns[k]` **in place** (`live := p.conns[k][:0]`), keeping each conn whose
   `Done()` channel is not closed. Store `live` back.
2. If `len(live) >= perPeer`: `i := next[k] % len(live)`, `next[k]++`, unlock, return
   `live[i]`. `next` counts forever and is never reset.
3. If `failed[k]` exists **and** `time.Since(failed[k]) < 2*time.Second` **and** `len(live) == 0`:
   unlock and return `errors.New("transport: peer recently unreachable")`. With at least one live
   conn below `perPeer`, a recent failure does not block another dial.
4. Get or create `dialing[k]` (never deleted). Unlock.
5. `dm.Lock()` (not ctx-aware: a waiting `Get` blocks until the running dial finishes,
   whatever its ctx). `defer dm.Unlock()`.
6. Lock. If `len(p.conns[k]) >= perPeer` (the **unfiltered** list), unlock and return
   `p.conns[k][0]` (no round robin). Unlock.
7. `addrs := p.addrs(id)` (called without the pool lock), `c, err := p.ep.Dial(ctx, id, addrs, alpn)`.
8. Error: lock, `failed[k] = time.Now()`, unlock, return `err` unwrapped.
9. Success: lock, `delete(failed, k)`, append `c` to `conns[k]`, unlock, return `c`.

Consequence: with `perPeer = 4` the first four `Get`s (concurrent or not) each dial a new
connection, serialized by `dm`. Round robin starts once four are live. The architecture text
(§11.3: "pools grow under load and shrink after ~90 s idle") is **not** implemented; port the
code.

`Drop(id, alpn)` (`:149-159`): lock, take `conns[k]`, `delete(conns, k)`, `delete(failed, k)`,
unlock, `Close()` each. `next` and `dialing` are kept.

`Path(id, alpn)` (`:162-174`): under the lock, the first conn in `conns[k]` whose `Done()` is
not closed gives `(c.Path(), true)`. Otherwise `(PathInfo{}, false)`.

`Close()` (`:177-187`): swap `conns` for an empty map under the lock, then close every conn.
`failed`/`next`/`dialing` are kept. The pool stays usable (a later `Get` dials again).

`Call(ctx, id, alpn, req)` (`:192-229`):
1. `c, err := p.Get(...)`: return on error.
2. `s, err := c.OpenStream(ctx)`: on error `p.Drop(id, alpn)` and return.
3. `defer wire.CloseStream(s)`.
4. `wire.WriteMsg(s, req)`: on error return it (**no** Drop).
5. `_ = s.CloseWrite()`.
6. Read one frame in a goroutine. `select` on done / `ctx.Done()`. On ctx: `s.CancelRead(0)`,
   `_ = s.Close()`, wait for the goroutine, return `nil, ctx.Err()` (`"context deadline exceeded"`
   or `"context canceled"`).
7. Read error: return it (**no** Drop; the doc comment "Transport failures drop the
   connection" only holds for `OpenStream`).
8. `reply.Type == wire.TErr` → `return reply, wire.ErrorFromMsg(reply)` (both non-nil).
9. Otherwise `return reply, nil`.

`Open(ctx, id, alpn)` (`:232-243`): `Get`, then `OpenStream`, with `Drop` on an `OpenStream`
error. The caller owns the stream.

`wire.CloseStream(s)` (`wire/wire.go:390-395`): `_ = s.Close()`, then `CancelRead(0)` if the
stream has it. Architecture §10: "a peer's `CancelRead` makes `Close` return an error — ignore it".

How the client drives pooled streams (behaviour to port unchanged):
- `client.Dial` (`client/client.go:93-111`): for each ticket member with a 32-byte id, in
  ticket order, `pool.Call` `TView` under `context.WithTimeout(ctx, 15*time.Second)`. The first
  answer whose `adoptReply` succeeds wins. On total failure it calls `pool.Close()` and returns
  `fmt.Errorf("client: no bootstrap node answered: %w", lastErr)`, or
  `"client: ticket names no nodes"` as `lastErr` when no member qualified.
- `addrsOf` (`:169-178`): the view node's `Addrs` if the view has the node with non-empty
  addrs, else the ticket member's addrs (nil for an id-only ticket, which triggers discovery).
- Put (`client/objects.go:259-300`): `pool.Open` under `10*RequestTimeout`, `WriteMsg(TPut)`,
  `SendPackRecords`, `CloseWrite`, `Expect(TPutResult)`, deferred `CloseStream`.
- Get (`client/fetch.go:274-309`): `Open`, `WriteMsg(TGet)`, `CloseWrite`, `Expect(TAbsent)`,
  pack reader, `io.Copy(io.Discard, pr)` drain, deferred `CloseStream`.
- Watch (`client/watch.go:127-245`): `Open` under `RequestTimeout`, `WriteMsg(TRefWatch)` (on
  error `Drop`), a reader goroutine, and `abandon := func(){ s.CancelRead(0); _ = s.Close() }`
  called from the consumer goroutine (concurrently with a blocked `Read`), plus `Drop` on idle
  timeout, read error or unexpected frame.
- `Cluster.Close()` (`client/client.go:120`) only closes the pool. `dialTicket`
  (`cmd/dstore/client.go:90-93`) closes the endpoint only when `client.Dial` fails. On success the
  CLI never closes the endpoint before exit.
- `pathAttrs` (`client/progress.go:149-159`): `"path", "none"` when there is no live conn, else
  `"path", "direct"|"relay", "rtt", p.RTT.Round(time.Millisecond)`.

### 2.3 `IrohConfig` / `BindIroh` (`transport/iroh.go:22-140`)

```go
type IrohConfig struct {
    SecretKey     irohkey.SecretKey
    ALPNs         []string
    RelayMode     *relay.Mode      // nil disables relays
    Advertise     []netip.AddrPort // nil: bound port on every interfaceIPs address
    BindAddr      netip.AddrPort   // tests / node --bind / port file
    Loopback      bool             // advertise 127.0.0.1:<port>
    DirectTimeout time.Duration    // default 2 s
    Discover      bool             // resolve ids through mDNS (+ number0 DNS with relays)
    Announce      bool             // publish addrs (mDNS + pkarr with relays); nodes only
    Logger        *slog.Logger     // default slog.Default()
}
const mdnsLookupTimeout = 3 * time.Second
```

The client binds with `IrohConfig{SecretKey: <fresh irohkey.GenerateSecretKey()>, RelayMode: rm,
Discover: !NoDiscovery, Logger: log}` (`cmd/dstore/client.go:78-86`): no ALPNs, no
`Advertise`, no `BindAddr`, no `Loopback`, `DirectTimeout` 0 (so 2 s), `Announce` false.

`relayModeOf(url, noRelay)` (`cmd/dstore/main.go:123-137`):
- `noRelay` → `nil, nil` (relays disabled). `--no-relay` wins over `--relay`.
- `url != ""` → `netaddr.ParseRelayURL(u)`, returning its error verbatim
  (`"failed to parse relay URL: <net/url error>"`), then `relay.ModeCustomURLs(ru)`.
- else `relay.ModeDefault()`.

`BindIroh(ctx, cfg)` steps:
1. `Logger` defaults to `slog.Default()`.
2. `e.ctx, e.cancel = context.WithCancel(context.Background())`: discovery goroutines outlive
   the bind ctx.
3. Options `WithSecretKey`, `WithALPNs(cfg.ALPNs...)`.
4. If `Discover || Announce`: `setupDiscovery()` (on error `cancel` and return the error), then
   `WithAddressLookup(e.lookup)`.
5. `RelayMode == nil` → `WithRelayMode(relay.ModeDisabled())`, else `WithRelayMode(*cfg.RelayMode)`.
6. `BindAddr.IsValid()` → `WithBindAddr` (default bind is `[::]:0`, dual-stack).
7. `WithTransportConfig(&iroh.QUICTransportConfig{KeepAlivePeriod: 5*time.Second,
   MaxIdleTimeout: 60*time.Second, MaxIncomingStreams: 1024})` (`:100-104`).
8. `iroh.Bind(ctx, opts...)`. Error: `e.close()`, `fmt.Errorf("transport: bind: %w", err)`.
9. `e.id = view.NodeID(ep.ID().Bytes())`, `port := ep.LocalAddr().Port()`.
10. Direct addrs: `Advertise != nil` → exactly those (an empty non-nil slice advertises none);
    else `Loopback` → `127.0.0.1:<port>`; else `localAddrPorts(port)` (`:448-457`), which pairs
    `interfaceIPs()` with `port` and falls back to `127.0.0.1:<port>`.
11. For each direct addr: `ep.AddExternalAddr(ap)` (a pinned QNT candidate; invalid, unspecified
    or zero-port addrs are silently ignored by go-iroh, `iroh/endpoint.go:855-869`) and append
    `netaddr.IPAddr{Addr: ap}.String()` to `e.addrs`.
12. If `RelayMode != nil`: `octx, cancel := context.WithTimeout(ctx, 10*time.Second)`;
    `_ = ep.Online(octx)` (blocks until a home relay is connected, error ignored); append
    `netaddr.RelayAddr{URL: u}.String()` for each `ep.Addr().RelayURLs()`. **So every client
    command with relays enabled can spend up to 10 s in bind.**
13. `Announce`: `publish()` then `republishLoop` every 30 s (node only).

`setupDiscovery` (`:144-177`):
- `e.lookup = &iroh.AddressLookupServices{}`.
- `md := mdns.New(id, mdns.WithPassive(!cfg.Announce), mdns.WithLookupTimeout(3*time.Second),
  mdns.WithLogger(cfg.Logger))`. `Discover` → `AddResolver(md)`; `Announce` → `AddPublisher(md)`.
- Always `go md.Start(e.ctx)`. If it returns an error while `e.ctx` is alive:
  `cfg.Logger.Warn("transport: mdns discovery unavailable", "error", err)`. The resolver stays
  registered (a lookup then just waits out the 3 s timeout).
- `RelayMode == nil` → return: mDNS only, "no external infrastructure".
- `Discover` → `AddResolver(iroh.N0DNSAddressLookup(nil))`.
- `Announce` → `iroh.N0PkarrPublisher(sk, &iroh.PkarrPublisherConfig{AddrFilter: identity})`,
  error `fmt.Errorf("transport: pkarr publisher: %w", err)`; `AddPublisher`; keep for `Close`.

`publish()` (`:181-191`): `key := strings.Join(Addrs(), " ")`. If it changed and there are
addresses: `e.lookup.Publish(dns.NewEndpointData(ParseAddrs(addrs)...))`. `republishLoop`
(`:195-207`) runs every 30 s until `e.ctx` ends.

`Addrs()` (`:225-243`): a copy of `e.addrs`, plus `relay:<url>` for every
`ep.Addr().RelayURLs()` not already present (relay URLs may appear after bind).

`Close()` (`:401-406`): `e.close()` (cancel, `pkarr.Close()`, `wg.Wait()`), then
`ep.Shutdown(ctx)` with a 5 s timeout.

### 2.4 `ParseAddrs` (`transport/iroh.go:247-261`)

For each string:
- `netaddr.ParseTransportAddr(s)` succeeds → keep it (`RelayAddr`, `IPAddr` or `CustomAddr`).
- Otherwise, if `netip.ParseAddrPort(s)` succeeds → `netaddr.IPAddr{Addr: ap}`.
- Otherwise drop the string silently.
No deduplication and no sorting; the order is preserved. Verified outputs are in 5.1.

`netaddr.ParseTransportAddr` (`netaddr/endpointaddr.go:258-281`): `kind, value, ok :=
strings.Cut(s, ":")`. No colon → `ParseCustomAddr(s)`. `"relay"` → `ParseRelayURL(value)`
(error returned unwrapped). `"ip"` → `netip.ParseAddrPort(value)`, error
`fmt.Errorf("transport address %q: %w", s, err)`. `"custom"` → `ParseCustomAddr(value)`. Any other
kind (case-sensitive) → `fmt.Errorf("transport address %q: unknown kind %q", s, kind)`.
`ParseCustomAddr` (`:203-218`) trims `"custom:"`, cuts at `"_"`; errors are
`"missing '_' separator"`, `"invalid ID"` (hex `ParseUint(...,16,64)`) and `"invalid data"` (hex).

`netaddr.ParseRelayURL` (`netaddr/relayurl.go:25-39,94-108`): Go `net/url.Parse` (very
lenient: `"example.com"` and `""` parse as relative URLs), error
`fmt.Errorf("%w: %v", ErrParseRelayURL, err)` with `ErrParseRelayURL = "failed to parse relay URL"`.
Normalization: `u.Host = strings.ToLower(u.Host)`, and an empty path for scheme
`http|https|ws|wss|ftp|file` becomes `"/"`. The string form is `u.String()` of the normalized URL.
The default port stays (`https://example.com:443/`); this differs from the Rust `url` crate,
which drops it.

### 2.5 `Dial` (`transport/iroh.go:265-302`)

```
Dial(ctx, id, addrs, alpn):
  eid, err := irohkey.NewEndpointID(id)      // err: "data is not a valid public key"
  cands := ParseAddrs(addrs)
  direct = every cand that is not RelayAddr (IPAddr AND CustomAddr), in order
  relays = every RelayAddr, in order
  timeout := cfg.DirectTimeout or 2s
  if len(direct) > 0:
      dctx := WithTimeout(ctx, timeout)
      c, err := raceConnect(dctx, ep, eid, direct, alpn)
      if ok → return irohConn{c}
      if len(relays) == 0 → return discoverDial(ctx, eid, alpn, err)
      // with relays the direct error is discarded
  if len(relays) > 0:
      c, err := raceConnect(ctx, ep, eid, append(relays, direct...), alpn)   // no DirectTimeout
      if err → return discoverDial(ctx, eid, alpn, err)
      return irohConn{c}
  return discoverDial(ctx, eid, alpn, nil)
```
The architecture (§11.3) says "race the relay only on retry". The code tries relay plus direct
immediately after the direct phase fails, inside the same `Dial`. Port the code.

`discoverDial(ctx, eid, alpn, prev)` (`:307-319`):
- `e.lookup == nil && prev != nil` → `return nil, prev`.
- `c, err := e.ep.Connect(ctx, netaddr.NewEndpointAddr(eid), alpn)`: bare id. go-iroh's
  `connectEarly` (`iroh/endpoint.go:1370-1381`) runs `lookupAddr`. With no lookup, or no answer
  before ctx ends, it returns `iroh.ErrNoAddress` = `"iroh: no reachable address for endpoint"`.
  A ctx deadline during lookup therefore surfaces as that text, **not** as
  `"context deadline exceeded"`.
- Error with `prev != nil` → `errors.Join(prev, fmt.Errorf("discovery: %w", err))`
  (`"<prev>\ndiscovery: <err>"`); without `prev` → `err`.
- This path does **not** call `awaitHandshake`.

`lookupAddr` (`iroh/endpoint.go:1758-1773`): iterate `lookup.Resolve(ctx, id)`, skip error items,
skip items whose id differs or that carry no addrs, and return `addr.WithAddrs(found.Addrs()...)`
for the first usable one. `AddressLookupServices.Resolve` (`iroh/addresslookup.go:289-354`) runs
every resolver concurrently and merges the items. mDNS (`iroh/mdns/mdns.go:297-326`) yields a
cached item at once, else sends a query and polls the cache every 25 ms until the 3 s timeout or
ctx. DNS (`iroh/addresslookup_dns.go:20,53-113`) queries TXT `_iroh.<z32-id>.dns.iroh.link.`
through the system resolver, staggered at `[200, 300, 600, 1000, 2000, 3000]` ms, 3 s per query.

`raceConnect(ctx, ep, id, cands, alpn)` (`:322-372`):
- `len(cands) == 0` → `fmt.Errorf("transport: no candidate addresses for %s", id.Short())`
  (`Short` is the first 5 bytes as lowercase hex, `key/key.go:156-158`). `Dial` never reaches it.
- One goroutine per candidate: `actx := WithCancel(ctx)`,
  `conn, err := ep.Connect(actx, netaddr.NewEndpointAddr(id, ta), alpn)`, then
  `awaitHandshake(actx, conn)`. A failure becomes `fmt.Errorf("dial %s: %w", ta, err)`, where `ta`
  is the Go string form, e.g. `dial ip:127.0.0.1:9: ...`.
- The first success cancels every `actx` and returns. A background goroutine drains the remaining
  results and `Close()`s late winners.
- All failed: cancel all, `errors.Join(errs...)` in **completion order** (lines joined by `"\n"`).

`awaitHandshake(ctx, conn)` (`:379-389`): select `conn.HandshakeComplete()` → nil;
`conn.Context().Done()` → `context.Cause(conn.Context())`; `ctx.Done()` → `conn.Close()`,
`ctx.Err()`. Reason: with a cached session ticket go-iroh's `Connect` returns at the 0-RTT window
before the peer has answered, so a dead address could otherwise "win".

go-iroh `Endpoint.Connect` (`iroh/endpoint.go:1304-1427`), the parts that matter:
- If ctx has no deadline, wrap it in `ConnectTimeout = 10*time.Second` (`iroh/defaults.go:36`).
- Self-dial: `"iroh: cannot connect to self"`; closed endpoint: `"iroh: endpoint closed"`.
- `dialTargets` (`:1449-1494`): IP addrs in order, then custom, then relay-mapped addrs (only
  with a relay transport). A remembered last-good target is moved to the front.
- `DialEarly` each target in turn. For an unproven target that is not the last, wait for
  `HandshakeComplete` up to `dialAttemptTimeout = 3*time.Second`. dstore's `raceConnect` always
  passes a single address, so this loop only matters for `discoverDial`.
- All targets failed → `fmt.Errorf("iroh: connect to %s: %w", addr.ID, tlsHandshakeFailure(firstErr))`
  (`addr.ID` as 64 hex chars).
- The qng handshake idle timeout is 5 s and the total handshake timeout 10 s
  (`internal/qng/internal/protocol/params.go:96-97`, `internal/qng/config.go:18-20`), so a
  silent target fails after ~5 s with `"timeout: handshake did not complete in time"` unless ctx
  ends first.

### 2.6 `irohConn` and QUIC semantics (`transport/iroh.go:408-445`)

| method | go-iroh call | semantics |
|---|---|---|
| `RemoteID` | `c.RemoteID().Bytes()` | peer identity verified by the RFC 7250 handshake |
| `ALPN` | `c.ALPN()` | negotiated ALPN |
| `OpenStream(ctx)` | `c.OpenStreamSync(ctx)` | blocks until the peer's MAX_STREAMS credit permits it or ctx ends (`internal/qng/streams_map_outgoing.go:82-122`). The peer (a Go node) allows 1024 concurrent incoming bidi streams. Nothing reaches the peer until the first write. |
| `AcceptStream(ctx)` | `c.AcceptStream(ctx)` | node side |
| `Close()` | `CloseWithError(0, "")` | sends CONNECTION_CLOSE (application code 0, empty reason) and **blocks until the connection run loop has exited** (`internal/qng/connection.go:3012-3019`) |
| `Path()` | `c.Paths()` | see below |
| `Done()` | `c.Context().Done()` | closed on any close: local, remote, idle timeout, handshake failure |

`Path()` (`:433-444`): start with `PathInfo{Direct: true}`. For each `p` in `c.Paths()` with
`p.Selected`: `Direct = !p.Relayed`, and if `p.HasRTT` then `RTT = p.RTT` (a later selected path
overwrites an earlier one). No selected path → `{Direct: true, RTT: 0}`. `p.RTT` is that path's
smoothed RTT, and `HasRTT` is false until the path has a sample
(`internal/socket/remote_state.go:940-980`, `iroh/conn.go:723-765`). The comment explains why:
the connection-level smoothed RTT is the initial guess (100 ms) after a path migration, which
would put a LAN node in the far class.

Stream methods (`iroh/conn.go:55-93`, qng):
- `Close()` and `CloseWrite()` are identical: `SendStream.Close()` sends FIN on the send side.
  A second call is a no-op returning nil. If the peer already sent STOP_SENDING, it returns
  `fmt.Errorf("close called for canceled stream %d", id)` (`internal/qng/send_stream.go:745-775`).
  The receive side is untouched (`internal/qng/stream.go:204-206`).
- `CancelRead(code)` queues STOP_SENDING(code) unless the FIN/RESET was already read or the stream
  was already cancelled. Later `Read`s return
  `&StreamError{Remote: false}` = `"stream %d canceled by local with error code %d"`
  (`internal/qng/receive_stream.go:425-457`, `internal/qng/errors.go:88-94`).
- `Read` at FIN → `io.EOF`. After a peer RESET_STREAM → `"stream %d canceled by remote with error code %d"`.
- `Write` after a peer STOP_SENDING → StreamError remote. qng answers STOP_SENDING with RESET_STREAM.
- After the peer closes the connection, operations return `"Application error 0x0 (remote)"`
  (`internal/qng/internal/qerr/errors.go:72-77`). After an idle timeout:
  `"timeout: no recent network activity"` (`:92`).

What each side observes (Go client, Go node):

| action | frame | peer observes |
|---|---|---|
| `CloseWrite()`/`Close()` | STREAM FIN | `wire.ReadMsg` returns `io.EOF` at the next frame boundary, or `io.ErrUnexpectedEOF` mid-header / `"wire: short frame: unexpected EOF"` mid-payload |
| `CancelRead(0)` | STOP_SENDING(0) | peer `Write` fails with StreamError (remote, code 0); peer `Close` returns `"close called for canceled stream N"` (ignored by `CloseStream`); peer stack sends RESET_STREAM(0) |
| `wire.CloseStream` | FIN, then STOP_SENDING(0) if the read side has not hit EOF | both of the above |
| `Conn.Close()` | CONNECTION_CLOSE app 0 | every op fails `"Application error 0x0 (remote)"`; node `serveConn`'s `AcceptStream` fails → node closes its side |
| exit without close | nothing | peer idle timeout after min(60 s, 60 s) = 60 s |

### 2.7 0-RTT, `HandshakeComplete`

go-iroh keeps a per-endpoint session cache keyed by the TLS server name (`iroh.ServerName(id)`)
and dials with `DialEarly`. A second dial to the same peer in the same process may resume and
return before any packet has come back (`iroh/endpoint.go:1343-1427`, `iroh/zerortt.go`).
dstore therefore calls `awaitHandshake` in `raceConnect`. The Go node accepts early data
(`Allow0RTT: true`, `iroh/endpoint.go:527-531`). The CLI client key is ephemeral per process, so
tickets only live within one process (pool redials, several connections per node).

### 2.8 `interfaceIPs` (`transport/ifaces.go:9-55`)

```go
var bridgePrefixes = []string{"docker", "br-", "cni", "flannel", "veth", "virbr", "lxc", "utun", "awdl", "llw"}
```
`net.Interfaces()` (error → nil). Skip an interface when `Flags&net.FlagUp == 0` (IFF_UP clear)
or when its name has one of the prefixes (`strings.HasPrefix`, case-sensitive). For each
`ifc.Addrs()` that is `*net.IPNet`: `netip.AddrFromSlice` (skip on failure), `Unmap()`, skip if
already seen, loopback, link-local unicast (169.254/16, fe80::/10), link-local multicast
(224.0.0.0/24, ff02::/16), invalid or unspecified. Output order is interface index order, then OS
address order. Only nodes publish these; for a client they only become pinned QNT candidates
(`AddExternalAddr`), which Rust iroh derives on its own.

### 2.9 In-memory transport (`transport/mem.go`)

- `Network{endpoints map[NodeID]*MemEndpoint, down map[NodeID]bool, cut map[[2]NodeID]bool, delay}`.
- `SetDown(id, down)` (`:31-43`): set `down[id]`. If `down`, close all of `id`'s conns, then
  `dropPeer(id)` on every endpoint, all under `n.mu`.
- `Partition(a, b, cut)` (`:46-59`): set both directions; if `cut`, `a.dropPeer(b)` and
  `b.dropPeer(a)`.
- `SetDelay(d)`: a fixed latency added to every dial.
- `Bind(id, alpns...)` (`:69-78`): accept channel of capacity 64. It **replaces** any endpoint
  already bound under `id` without closing it.
- `reachable(from, to)` (`:80-94`): `down[from]` → `"mem: local endpoint is down"`;
  `down[to] || cut[{from,to}]` → `fmt.Errorf("mem: %s unreachable", view.ShortID(to))`; missing or
  closed endpoint → `"mem: %s not bound"`. `view.ShortID` is the first 4 bytes as lowercase hex
  (`view/view.go:36`).
- `MemEndpoint.Addrs()` = `[]string{"mem:" + view.ShortID(id)}` (which `ParseAddrs` drops).
- `Dial` (`:110-143`): closed → `ErrClosed`; `reachable`; `!peer.alpns[alpn]` →
  `fmt.Errorf("mem: %s does not speak %s", view.ShortID(id), alpn)`; delay (ctx → `ctx.Err()`);
  create a conn pair with stream channels of capacity 256, track both; send the remote conn on
  `peer.accept`, or on ctx `local.Close()` and `ctx.Err()`.
- `Accept(ctx)`: channel receive or `ctx.Err()`. `Close()`: mark closed and close all conns.
- `memConn.Path()` = `{Direct: true, RTT: time.Millisecond}`. `Close()` runs once: close `done`,
  untrack, `go peer.Close()`.
- `OpenStream`: `done` closed → `ErrClosed`; send the peer half on `peer.streams`, select
  `done` → `ErrClosed`, `ctx` → `ctx.Err()`. `AcceptStream`: receive / `ErrClosed` / `ctx.Err()`.
- `memStream`: `Read` from the inbound pipe, `Write` to the outbound pipe, `CloseWrite` and
  `Close` close the outbound pipe (no read cancel), `CancelRead(code)` cancels the inbound pipe
  (the code is ignored).
- `pipe` with `pipeLimit = 4 << 20`: `Write` blocks while `len(buf) >= 4 MiB`. Cancelled →
  `(n, errors.New("mem: stream reset by peer"))`; write-closed → `(n, io.ErrClosedPipe)` =
  `"io: read/write on closed pipe"`. `Read` waits for data/close/cancel. Cancelled →
  `(0, errors.New("mem: read canceled"))` (the buffer is cleared on cancel); empty and closed →
  `io.EOF`.
- Quirk: closing a `memConn` does not touch its streams' pipes, so a blocked `Read` stays blocked
  until its ctx path cancels it.

### 2.10 go-iroh defaults vs what dstore sets

| setting | go-iroh v0.2.0 / qng default | dstore sets | source |
|---|---|---|---|
| keepalive | 5 s (`HeartbeatInterval`) | 5 s | `iroh/defaults.go:15`, `iroh.go:101` |
| max idle timeout | 30 s (`RelayPathMaxIdleTimeout`) | 60 s | `iroh/endpoint.go:522`, `iroh.go:102` |
| max incoming bidi streams | 100 | 1024 | `internal/qng/internal/protocol/params.go:34`, `iroh.go:103` |
| handshake idle / total | 5 s / 10 s | – | `internal/qng/internal/protocol/params.go:97`, `internal/qng/config.go:18-20` |
| stream receive window | initial 512 KiB, auto-tuned to 6 MiB | – | `internal/qng/internal/protocol/params.go:18-25` |
| connection receive window | initial 768 KiB (×1.5), auto-tuned to 15 MiB | – | `internal/qng/internal/protocol/params.go:16,22,28` |
| direct / relay path idle | 15 s / 30 s | – | `internal/socket/remote_state.go:35-39` |
| multipath paths / QNT addrs | 8 / 32 | – | `iroh/defaults.go:27,31` |
| datagrams, 0-RTT accept, token store | on, on, LRU(32, 8) | – | `iroh/endpoint.go:520-538` |
| net_report | every 5 min whenever relays are configured | – | `iroh/endpoint.go:500-502,679-693` |
| NAT-PMP | off unless `WithNATPMP` | – | `iroh/endpoint.go:334-343` |
| connection id length | 8 | – | `iroh/endpoint.go:589` |

dstore configures **no** flow-control window anywhere: grepping `dstore` for `Window` finds only
the TUI rate meter and `worktree.RacyWindow`. The WAN-BDP requirement in architecture §11.3/§16
exists because Rust iroh's defaults (noq 1.25 MB fixed stream window) cap one stream at ~27 MB/s
at 43 ms. go-iroh's auto-tuning reaches line rate unconfigured.

Default relay map (`relay/relay.go:24-30,92-98,249-256`): `MapFromURLs` over
`https://use1-1.relay.n0.iroh-canary.iroh.link.`, `https://usw1-1.relay.n0.iroh-canary.iroh.link.`,
`https://euc1-1.relay.n0.iroh-canary.iroh.link.`, `https://aps1-1.relay.n0.iroh-canary.iroh.link.`,
each with `QUIC: &QUICConfig{Port: 7842}`. `URLs()` sorts by string, and `Bind` picks `urls[0]`
(`aps1-1…`) as the bootstrap home relay until net_report chooses (`iroh/endpoint.go:641-645`).
`ModeCustomURLs(u)` uses the same QUIC port 7842.

---

## 3. Byte and text formats

### 3.1 Address strings (view `Node.Addrs`, ticket member addrs, `Endpoint.Addrs()`)

- IP: `"ip:" + netip.AddrPort.String()`. IPv4 `ip:192.168.1.2:4242`; IPv6
  `ip:[2001:db8::1]:4242`; IPv4-mapped `ip:[::ffff:10.0.0.1]:1`; with zone `ip:[fe80::1%en0]:7`.
- Relay: `"relay:" + normalized URL` (2.4), e.g.
  `relay:https://use1-1.relay.n0.iroh-canary.iroh.link./`.
- Custom: `"<id lowercase hex, no leading zeros>_<data lowercase hex>"` **without** a prefix
  (`netaddr/endpointaddr.go:106,189-191`). Rust iroh-base Display adds `custom:`
  (`iroh-base-1.0.3/src/endpoint_addr.rs:86`), so do not use it for string forms.
- Mem: `"mem:" + 8 hex chars`.

The client only parses these strings, never generates them, but error texts embed them
(`dial ip:…: …`), so the Rust type must render the Go form.

### 3.2 DNS discovery record

TXT at `_iroh.<z32-endpoint-id>.dns.iroh.link.` (`dns/resolver.go:17-18,58-61`). Rust
`iroh-dns-1.0.3/src/dns.rs:45` has `N0_DNS_ENDPOINT_ORIGIN_PROD = "dns.iroh.link."`. Go↔Rust
interop is verified (`COMPATIBILITY.md` rows `discovery/go-publish-rust-dns`,
`discovery/rust-publish-go-dns`, `vectors/pkarr-txt`).

### 3.3 mDNS wire format (go-iroh `iroh/mdns`)

- Multicast `224.0.0.251:5353` (required) and `[ff02::fb]:5353` (best effort) (`mdns.go:39-41`).
  Listener sockets set `SO_REUSEADDR` and `SO_REUSEPORT` (`reuse_unix.go:11-26`) and join the group
  on every interface that is up and multicast-capable; if none joins, `join(nil, group)`
  (`mdns.go:248-263`).
- Service `_irohv1._udp.local` (`DefaultServiceName = "irohv1"`, `mdns.go:31,570-576`).
  Instance `<label>._irohv1._udp.local`, host `<label>.local`, where `label` is RFC 4648
  base32, lowercase, no padding, of the 32 id bytes (`mdns.go:561-584`).
- Query (`dnsmsg.go:29-40`): 12-byte header, all zero except QDCOUNT = 2; question 1 = service
  name, type PTR (12), class IN (1); question 2 = instance name, PTR, IN. No compression.
- Announcement (`dnsmsg.go:42-88,90-181`): header flags `0x8400`, ANCOUNT = `3 + len(ips)`,
  QDCOUNT/NSCOUNT/ARCOUNT 0. **All records are in the answer section**, TTL 120, class IN, no name
  compression:
  1. PTR `_irohv1._udp.local` → instance
  2. SRV instance: priority 0, weight 0, `port`, target `<label>.local`
  3. TXT instance: `"relay=<url>"` if a relay URL ≤ 249 bytes, `"user-data=<…>"` if set; strings
     over 255 bytes are skipped
  4. A (for IPv4 or 4-in-6) or AAAA per IP, name `<label>.local`
- One port only: the port shared by most addresses, lowest on a tie. Other addresses are dropped
  with a Warn log (`mdns.go:495-550`).
- Parser (`dnsmsg.go` `parseDNS`/`readRR`/`readName`/`parseAnnouncement`): reads answer,
  authority and additional records **together** and handles compression pointers (up to depth 32).
  It needs an SRV with non-zero port whose instance ends in `._irohv1._udp.local`
  (case-insensitive), a decodable label, and at least one A/AAAA for the SRV target. TXT keys
  `relay` and `user-data`.
- Responder: answers PTR/ANY questions naming the service or its own instance, and SRV/TXT naming
  its instance, after a random 20–120 ms delay, only while `Start` runs and only when not passive
  (`mdns.go:365-427`).
- Resolver (`mdns.go:297-326`): cache hit → yield; else `query(id)`, poll the cache every 25 ms
  until the timeout (dstore: 3 s) or ctx (on ctx it yields `ctx.Err()` as an error item). The
  cache is filled by any announcement received while `Start` runs (`mdns.go:339-354`).

**Rust incompatibility:** `iroh-mdns-address-lookup-0.4.0` uses `swarm-discovery-0.6.3`, whose
receiver takes A/AAAA **only from the additional section** (`swarm-discovery-0.6.3/src/receiver.rs:133-160`).
In the answer section it only uses SRV/TXT under the service name (`:77-131`). go-iroh puts
A/AAAA in the answer section, so a Rust client using that crate finds no addresses for Go nodes.
It also waits `LOOKUP_DURATION = 10 s` (`iroh-mdns-address-lookup-0.4.0/src/lib.rs:95`) where
dstore uses 3 s. See 4.6 and 7.

### 3.4 Transport-related log and text output

- `slog` Warn: msg `transport: mdns discovery unavailable`, attr `error=<err>`.
- Client Info `connected` attrs (`client/client.go:109`): `node=<ShortID> nodes=<n> path=none|direct|relay [rtt=<Duration>]`.
  The Duration is Go `time.Duration.Round(time.Millisecond).String()` (`0s`, `1ms`, `12ms`,
  `1.235s`).
- TUI (`cmd/dstore/tui.go:262-270`): path `direct`/`relay`; rtt `-` when 0, else rounded Duration.
- Error strings, verbatim: `transport: closed`, `transport: peer recently unreachable`,
  `transport: bind: %w`, `transport: pkarr publisher: %w`, `transport: no candidate addresses for %s`,
  `dial %s: %w`, `discovery: %w`, `data is not a valid public key`,
  `iroh: no reachable address for endpoint`, `iroh: cannot connect to self`,
  `mem: local endpoint is down`, `mem: %s unreachable`, `mem: %s not bound`,
  `mem: %s does not speak %s`, `mem: stream reset by peer`, `mem: read canceled`,
  `io: read/write on closed pipe`, `context deadline exceeded`, `context canceled`.
- The CLI prints `dstore: <err>` (`cmd/dstore/main.go:48-51`). Transport failures during bootstrap
  surface as `dstore: client: no bootstrap node answered: dial ip:…: <quic error>`, and the inner
  QUIC text comes from qng.

---

## 4. Rust design

### 4.1 Module layout

```
src/transport/mod.rs     traits, PathInfo, TransportError, re-exports
src/transport/addr.rs    Go-compatible TransportAddr parse/format (ParseAddrs)
src/transport/pool.rs    Pool
src/transport/iroh.rs    IrohConfig, IrohEndpoint, IrohConn, Dial logic
src/transport/discover.rs explicit discovery (DNS via iroh, mDNS via mdns.rs), first usable answer
src/transport/mdns.rs    port of go-iroh iroh/mdns (resolver; publisher/responder for nodes)
src/transport/ifaces.rs  interface_ips()
src/transport/mem.rs     in-memory Network
src/ctx.rs               Go-context analogue (shared with the client; coordinate with other notes)
```

### 4.2 Cargo dependencies

```toml
iroh        = { version = "=1.0.3", default-features = false, features = ["tls-ring", "fast-apple-datapath"] }
tokio       = { version = "1.52", features = ["rt-multi-thread", "macros", "net", "time", "sync", "io-util"] }
tokio-util  = "0.7"          # CancellationToken
async-trait = "0.1"          # dyn Endpoint / dyn Conn
futures     = "0.3"          # merging lookup streams, select
socket2     = { version = "0.6", features = ["all"] }   # SO_REUSEPORT, multicast joins for mDNS
nix         = { version = "0.30", features = ["net"] }  # getifaddrs flags, if_nametoindex
data-encoding = "2"          # base32 mDNS label
thiserror   = "2"
tracing     = "0.1"
url         = "2.5"          # iroh RelayUrl conversion
rand        = "0.9"          # mDNS responder delay (node side only)
```
All are in the offline registry (tokio 1.52.3/1.53.1, tokio-util 0.7.17/0.7.19,
async-trait 0.1.89+, socket2 0.6.4/0.6.5, nix 0.30.1, data-encoding 2.10/2.11, url 2.5.7/2.5.8).

iroh features (`iroh-1.0.3/Cargo.toml:67-98`): the defaults are `metrics`,
`fast-apple-datapath`, `portmapper`, `tls-ring`.
- Keep `tls-ring`: it enables `presets::Minimal` through `cfg(with_crypto_provider)` (`build.rs`).
- Keep `fast-apple-datapath`.
- Drop `portmapper`: the Go client does no port mapping, and it avoids SSDP firewall prompts on
  macOS. Also set `Builder::portmapper_config(PortmapperConfig::Disabled)`.
- Drop `metrics` (unused; confirm on the first build that iroh compiles without it).
- Leave `qlog`, `unstable-custom-transports` and `test-utils` off.
- Do **not** depend on `iroh-mdns-address-lookup` (3.3).
- Rust toolchain ≥ 1.91 (`rust-version` of iroh), satisfied by nixpkgs rustc 1.95.

### 4.3 Context, errors, traits

```rust
// src/ctx.rs
#[derive(Clone)]
pub struct Ctx { deadline: Option<tokio::time::Instant>, cancel: tokio_util::sync::CancellationToken }
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CtxError { #[error("context canceled")] Canceled, #[error("context deadline exceeded")] DeadlineExceeded }
impl Ctx {
    pub fn background() -> Ctx;
    pub fn with_timeout(&self, d: Duration) -> Ctx;   // child token, deadline = min(parent, now + d)
    pub fn with_cancel(&self) -> Ctx;
    pub fn cancel(&self);
    pub fn err(&self) -> Option<CtxError>;
    pub async fn done(&self);
    pub async fn run<F: Future>(&self, f: F) -> Result<F::Output, CtxError>; // select f / deadline / cancel
}
```

```rust
// src/transport/mod.rs
pub type NodeId = [u8; 32];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PathInfo { pub direct: bool, pub rtt: Duration }

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("transport: closed")] Closed,
    #[error("transport: peer recently unreachable")] RecentlyUnreachable,
    #[error("transport: no candidate addresses for {0}")] NoCandidates(String), // 10 hex chars
    #[error("transport: bind: {0}")] Bind(String),
    #[error("transport: pkarr publisher: {0}")] PkarrPublisher(String),
    #[error("dial {addr}: {source}")] DialAddr { addr: String, source: Box<TransportError> },
    #[error("discovery: {0}")] Discovery(Box<TransportError>),
    #[error("{}", join_lines(.0))] Joined(Vec<TransportError>),   // errors.Join: "\n"
    #[error("data is not a valid public key")] InvalidKey,
    #[error("iroh: no reachable address for endpoint")] NoAddress,
    #[error("iroh: cannot connect to self")] SelfConnect,
    #[error("{0}")] Ctx(#[from] CtxError),
    #[error("mem: local endpoint is down")] MemLocalDown,
    #[error("mem: {0} unreachable")] MemUnreachable(String),
    #[error("mem: {0} not bound")] MemNotBound(String),
    #[error("mem: {0} does not speak {1}")] MemAlpn(String, String),
    #[error("{0}")] Quic(String),   // underlying iroh/noq error text (divergent, see 8)
}

#[async_trait::async_trait]
pub trait Endpoint: Send + Sync + 'static {
    fn id(&self) -> NodeId;
    async fn dial(&self, ctx: &Ctx, id: NodeId, addrs: &[String], alpn: &str) -> Result<Arc<dyn Conn>, TransportError>;
    async fn accept(&self, ctx: &Ctx) -> Result<Arc<dyn Conn>, TransportError>;
    fn addrs(&self) -> Vec<String>;
    async fn close(&self) -> Result<(), TransportError>;
}

#[async_trait::async_trait]
pub trait Conn: Send + Sync + 'static {
    fn remote_id(&self) -> NodeId;
    fn alpn(&self) -> String;
    async fn open_stream(&self, ctx: &Ctx) -> Result<Stream, TransportError>;
    async fn accept_stream(&self, ctx: &Ctx) -> Result<Stream, TransportError>;
    fn close(&self);                 // Go Close() error is ignored at every call site
    fn path(&self) -> PathInfo;
    fn is_closed(&self) -> bool;     // non-blocking `select { case <-c.Done(): default: }`
    async fn closed(&self);          // <-c.Done()
}

pub struct Stream { pub send: SendHalf, pub recv: RecvHalf }
pub enum SendHalf { Iroh(iroh::endpoint::SendStream), Mem(mem::PipeWriter) }
pub enum RecvHalf { Iroh(iroh::endpoint::RecvStream), Mem(mem::PipeReader) }
impl tokio::io::AsyncWrite for SendHalf { /* delegate; noq implements it: noq-1.1.1/src/send_stream.rs:332 */ }
impl tokio::io::AsyncRead  for RecvHalf { /* delegate; noq-1.1.1/src/recv_stream.rs:594 */ }
impl Stream {
    pub fn close_write(&mut self) -> Result<(), TransportError>;  // Go CloseWrite
    pub fn close(&mut self) -> Result<(), TransportError>;        // Go Close == CloseWrite for iroh and mem
    pub fn cancel_read(&mut self, code: u64);                     // Go CancelRead
    pub fn split(self) -> (SendHalf, RecvHalf);
}
// wire::close_stream(&mut Stream): let _ = s.close(); s.cancel_read(0);
```

Stream mapping onto noq (re-exported as `iroh::endpoint::{SendStream, RecvStream, VarInt}`):
- `close_write`/`close` → `SendStream::finish()` (`noq-1.1.1/src/send_stream.rs:188-201`). It
  returns `Ok` when the peer already sent STOP_SENDING (Go returns an error there, which callers
  ignore) and `Err(ClosedStream)` when already finished or reset (Go returns nil). Map both to
  `Ok(())` except a reset.
- `cancel_read(code)` → `RecvStream::stop(VarInt::from_u64(code))`, ignoring `ClosedStream`
  (`noq-1.1.1/src/recv_stream.rs:275-287`).
- Implicit behaviour: dropping an unfinished `SendStream` finishes it, or resets with the stop code
  if stopped (`send_stream.rs:350-372`). Dropping a `RecvStream` that has not read to the end sends
  `stop(0)` (`recv_stream.rs:605-632`). So a plain drop equals `wire.CloseStream`, but call it
  explicitly for clarity.
- Error texts: `ReadError::Reset(code)` `"stream reset by peer: error {code}"`,
  `ReadError::ConnectionLost(e)` `"connection lost"`, `WriteError::Stopped(code)`
  `"sending stopped by peer: error {code}"` (`recv_stream.rs:636-656`, `send_stream.rs:378-398`).
  These differ from qng's texts.
- Concurrency: Go's `watch` calls `CancelRead` from another goroutine while `Read` blocks. In Rust,
  read inside a `tokio::select!` next to the idle timer and ctx, then call `cancel_read`/`close` after
  the select has dropped the read future. Nothing is shared across tasks. `Pool::call` does the
  same: `ctx.run(read_msg(&mut s.recv))`; on `Err` call `s.cancel_read(0)`, `s.close()`, return the
  ctx error.

### 4.4 `Pool` (`src/transport/pool.rs`)

```rust
pub type AddrsFn = Arc<dyn Fn(NodeId) -> Vec<String> + Send + Sync>;
#[derive(Clone, PartialEq, Eq, Hash)] struct PoolKey { id: NodeId, alpn: String }
struct PoolState {
    conns: HashMap<PoolKey, Vec<Arc<dyn Conn>>>,
    next: HashMap<PoolKey, usize>,
    dialing: HashMap<PoolKey, Arc<tokio::sync::Mutex<()>>>,
    failed: HashMap<PoolKey, std::time::Instant>,
}
pub struct Pool { ep: Arc<dyn Endpoint>, addrs: AddrsFn, per_peer: usize, st: std::sync::Mutex<PoolState> }
impl Pool {
    pub fn new(ep: Arc<dyn Endpoint>, addrs: AddrsFn, per_peer: i64) -> Pool;    // <=0 → 1
    pub fn endpoint(&self) -> &Arc<dyn Endpoint>;
    pub async fn get(&self, ctx: &Ctx, id: NodeId, alpn: &str) -> Result<Arc<dyn Conn>, TransportError>;
    pub fn drop_conns(&self, id: NodeId, alpn: &str);                            // Go Drop
    pub fn path(&self, id: NodeId, alpn: &str) -> Option<PathInfo>;
    pub fn close(&self);
    pub async fn call(&self, ctx: &Ctx, id: NodeId, alpn: &str, req: &wire::Msg) -> Result<wire::Msg, wire::CallError>;
    pub async fn open(&self, ctx: &Ctx, id: NodeId, alpn: &str) -> Result<Stream, TransportError>;
}
```
Port the steps of 2.2 one for one: in-place filter with `retain(|c| !c.is_closed())`, the same
round-robin counter, the 2 s backoff only when there are no live conns, and the second check against
the **unfiltered** list returning element 0. Never hold the std mutex across an await; the dial lock
is a `tokio::sync::Mutex<()>` held across `ep.dial`. `call` returns the remote error for `TErr`
(`wire::ErrorFromMsg` carries code, text, view and retry_after, so the reply itself adds nothing).

### 4.5 `IrohEndpoint` on Rust iroh 1.0.3 (`src/transport/iroh.rs`)

```rust
pub enum RelayChoice { Default, Custom(GoRelayUrl) }        // None = disabled
pub struct IrohConfig {
    pub secret_key: iroh::SecretKey,
    pub alpns: Vec<String>,
    pub relay: Option<RelayChoice>,
    pub advertise: Option<Vec<std::net::SocketAddr>>,
    pub bind_addr: Option<std::net::SocketAddr>,
    pub loopback: bool,
    pub direct_timeout: Option<Duration>,                  // None → 2 s
    pub discover: bool,
    pub announce: bool,
}
pub struct IrohEndpoint { ep: iroh::Endpoint, id: NodeId, cfg: IrohConfig, addrs: Mutex<Vec<String>>, disc: Option<Discovery>, bg: tokio_util::sync::CancellationToken }
pub async fn bind_iroh(ctx: &Ctx, cfg: IrohConfig) -> Result<IrohEndpoint, TransportError>;
```

`bind_iroh` mapped onto the iroh 1.0.3 builder (`iroh-1.0.3/src/endpoint.rs`):

```rust
let tc = iroh::endpoint::QuicTransportConfig::builder()          // endpoint/quic.rs:153-163 already sets keepalive 5 s,
    .keep_alive_interval(Duration::from_secs(5))                  //   path keepalive 5 s, path idle 15 s, 8 paths, 32 QNT addrs
    .max_idle_timeout(Some(Duration::from_secs(60).try_into().unwrap()))   // quic.rs:211-214
    .max_concurrent_bidi_streams(VarInt::from_u32(1024))          // quic.rs:176-179
    .stream_receive_window(VarInt::from_u32(STREAM_RECEIVE_WINDOW))  // quic.rs:224-227 (see 4.8)
    .send_window(SEND_WINDOW)                                     // quic.rs:246-249
    .build();
let relay_mode = match &cfg.relay {
    None => RelayMode::Disabled,
    Some(RelayChoice::Default) => RelayMode::Custom(RelayMap::from_iter(GO_DEFAULT_RELAYS.iter().map(|s| s.parse::<RelayUrl>().unwrap()))),
    Some(RelayChoice::Custom(u)) => RelayMode::custom([u.to_iroh()?]),   // RelayConfig::from(RelayUrl) → QUIC port 7842
};
let mut b = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)   // presets.rs:57-79: crypto provider only
    .secret_key(cfg.secret_key.clone())                            // endpoint.rs:524
    .alpns(cfg.alpns.iter().map(|a| a.as_bytes().to_vec()).collect())   // :535
    .transport_config(tc)                                           // :669
    .portmapper_config(PortmapperConfig::Disabled)                  // :786
    .relay_mode(relay_mode);                                        // :557-580
if let Some(a) = cfg.bind_addr { b = b.clear_ip_transports().bind_addr(a)?; }   // :503, :363
// no .address_lookup(...): discovery is explicit (4.6)
let ep = b.bind().await.map_err(|e| TransportError::Bind(e.to_string()))?;
```
- `Builder::empty()` defaults (`endpoint.rs:191-215`): IPv4 `0.0.0.0:0` and IPv6 `[::]:0` sockets,
  relays disabled, no address lookup, `max_tls_tickets` 8, `BiasedRttPathSelector`. Never use
  `presets::N0`: it adds a pkarr publisher and resolver, switches to staging infrastructure when
  `IROH_FORCE_STAGING_RELAYS` is set (`endpoint.rs:1970-1987`), and uses the production relay hosts.
- Direct addrs (node only): as 2.3 step 10, with `ep.add_external_addr(a).await`
  (`endpoint.rs:1011-1017`) for each and `"ip:" + go_addrport_string(a)` appended. `LocalAddr().Port()`
  becomes `ep.bound_sockets()` (`endpoint.rs:1445`); pick the port of the socket bound to
  `0.0.0.0`/`[::]`.
- Relay online: only when relays are configured (Rust `online()` never returns with relays
  disabled; Go returns `ErrNoRelay` there), `let _ = ctx.with_timeout(10s).run(ep.online()).await;`
  (`endpoint.rs:1358-1373`). Then append `"relay:" + go_relay_string(u)` for each
  `ep.addr().relay_urls()`.
- Discovery listeners: if `discover || announce`, start `mdns::Discovery` (4.6). If `announce &&
  relay.is_some()`, build the pkarr publisher (4.10).
- `close()`: cancel background tasks, then `ep.close().await` bounded by 5 s. `EndpointInner::close`
  sends CONNECTION_CLOSE on every connection and waits for draining (`socket.rs:1133-1173`).
- The CLI should `ep.close()` (bounded, e.g. 3 s) before exit. In Rust `Connection::close` only
  queues CONNECTION_CLOSE (`endpoint/connection.rs:959-961`); Go's `CloseWithError` blocks until it
  has been sent (2.6). Without this a Go node keeps dead connections until its 60 s idle timeout.

`dial` (Go `Dial`, 2.5):

```rust
async fn dial(&self, ctx: &Ctx, id: NodeId, addrs: &[String], alpn: &str) -> Result<Arc<dyn Conn>, TransportError> {
    let eid = iroh::EndpointId::from_bytes(&id).map_err(|_| TransportError::InvalidKey)?;  // iroh-base key.rs:122-127, same text
    let cands = addr::parse_addrs(addrs);
    let (relays, direct): (Vec<_>, Vec<_>) = cands.into_iter().partition(|a| a.is_relay());
    let timeout = self.cfg.direct_timeout.unwrap_or(Duration::from_secs(2));
    if !direct.is_empty() {
        match ctx.with_timeout(timeout).run(self.connect_phase(eid, &direct, alpn)).await.flatten_ctx() {
            Ok(c) => return Ok(c),
            Err(e) if relays.is_empty() => return self.discover_dial(ctx, eid, alpn, Some(e)).await,
            Err(_) => {}
        }
    }
    if !relays.is_empty() {
        let all: Vec<_> = relays.iter().chain(direct.iter()).cloned().collect();
        return match ctx.run(self.connect_phase(eid, &all, alpn)).await.flatten_ctx() {
            Ok(c) => Ok(c),
            Err(e) => self.discover_dial(ctx, eid, alpn, Some(e)).await,
        };
    }
    self.discover_dial(ctx, eid, alpn, None).await
}
```

`connect_phase(eid, cands, alpn)`: **one** `ep.connect(EndpointAddr::from_parts(eid, iroh_addrs), alpn.as_bytes())`
(`endpoint.rs:1052-1072`), not one connect per candidate. Why:
- Rust iroh keeps one `RemoteStateActor` per remote id. `connect_with_opts` adds every given address
  (`remote_state.rs:850-860`), and while no path is selected each outgoing datagram, including the
  QUIC Initial, is sent **to all known paths** (`remote_state.rs:788-830`).
- Concurrent per-candidate connects would not isolate addresses. The first path that answers becomes
  selected, which gives Go's "live beats dead" result.
- `Endpoint::connect` awaits `Connecting` and returns `Connection<HandshakeCompleted>`, so Go's
  `awaitHandshake` is inherent. Never call `Connecting::into_0rtt` (`endpoint/connection.rs:528-550`).
- Convert candidates with `GoTransportAddr::to_iroh()`: `Ip` → `TransportAddr::Ip(SocketAddr)` (a
  zone becomes `scope_id` via `nix::net::if_::if_nametoindex`, or a numeric zone); `Relay` →
  `TransportAddr::Relay(url.parse()?)`. A relay string `url::Url` cannot parse (e.g. `relay:example.com`)
  is skipped at this point but still counts for the phase logic. `Custom` →
  `TransportAddr::Custom(CustomAddr::from_parts(id, &data))`.
- If no candidate converts, fail at once with `NoAddress`.
- On error, produce `Joined(cands.map(|a| DialAddr{addr: a.to_string(), source: e.clone()}))` in
  candidate order. For a single candidate that is just `"dial <addr>: <err>"`, matching Go's shape.
  The inner text is noq/iroh's (8).

`discover_dial(ctx, eid, alpn, prev)` (Go `discoverDial`):

```rust
if self.disc.is_none() { if let Some(p) = prev { return Err(p) } }
let r = async {
    let found = match &self.disc { Some(d) => d.first_usable(ctx, eid).await, None => None }; // bounded by ctx; None on timeout
    let Some(addr) = found else { return Err(TransportError::NoAddress) };
    ctx.run(self.ep.connect(addr, alpn.as_bytes())).await?.map_err(quic_err)
}.await;
match (r, prev) { (Ok(c), _) => Ok(wrap(c)), (Err(e), Some(p)) => Err(Joined(vec![p, Discovery(Box::new(e))])), (Err(e), None) => Err(e) }
```

`accept(ctx)` (node): `loop { let inc = ctx.run(ep.accept()).await?.ok_or(TransportError::Closed)?;
match inc.await { Ok(c) => return Ok(wrap(c)), Err(e) if died_during_handshake(&e) => continue,
Err(e) => return Err(quic_err(e)) } }`. That mirrors go-iroh `accept` (`iroh/endpoint.go:1572-1594`).

`IrohConn` on `iroh::endpoint::Connection` (`endpoint/connection.rs`):

| Go | Rust iroh 1.0.3 |
|---|---|
| `RemoteID()` | `conn.remote_id().as_bytes()` (`:1127`) |
| `ALPN()` | `String::from_utf8_lossy(conn.alpn())` (`:1115`) |
| `OpenStream(ctx)` | `ctx.run(conn.open_bi()).await` (`:885`), blocks on MAX_STREAMS as in Go |
| `AcceptStream(ctx)` | `ctx.run(conn.accept_bi()).await` (`:901`) |
| `Close()` | `conn.close(VarInt::from_u32(0), b"")` (`:959`); the Go peer sees `"Application error 0x0 (remote)"` |
| `Done()` | `conn.close_reason().is_some()` (`:925`) / `conn.closed().await` (`:917`) |
| `Path()` | see below |

```rust
const NOQ_INITIAL_RTT: Duration = Duration::from_millis(333);   // noq-proto-1.1.1/src/config/transport.rs:564
fn path(&self) -> PathInfo {
    let mut info = PathInfo { direct: true, rtt: Duration::ZERO };
    for p in self.conn.paths().iter() {                         // connection.rs:1144 → PathList
        if p.is_selected() {                                    // path_watcher.rs:470
            info.direct = !p.is_relay();                        // :480
            let rtt = p.rtt();                                  // :494-496 = PathStats.rtt = RttEstimator::get()
            if rtt != NOQ_INITIAL_RTT { info.rtt = rtt; }       // unsampled estimator reports initial_rtt exactly
        }
    }
    info
}
```
`RttEstimator::get()` returns `smoothed.unwrap_or(latest)`, and `latest` starts at `initial_rtt`
for the connection and for every new path (`noq-proto-1.1.1/src/connection/paths.rs:303,772-804`).
There is no public "has sample" flag, so equality with the configured `initial_rtt` is the sentinel.
Do not override `initial_rtt` in the transport config. If you do, update the sentinel.

### 4.6 Explicit discovery (`src/transport/discover.rs`, `src/transport/mdns.rs`)

Do **not** attach lookup services to the iroh endpoint. `RemoteStateActor::trigger_address_lookup`
(`remote_state.rs:866-881`) would then run the lookup on every connect with no selected path, i.e.
during the direct phase too, whereas Go only looks up in `discoverDial`. It would also bring
swarm-discovery's incompatibility. Instead:

```rust
pub struct Discovery {
    mdns: Option<Arc<mdns::Discovery>>,        // when discover (listener started at bind, as Go)
    dns: Option<iroh::address_lookup::DnsAddressLookup>, // when discover && relays
}
impl Discovery {
    /// Go lookupAddr: first item for `id` with a non-empty address set; errors skipped; None when all end or ctx ends.
    pub async fn first_usable(&self, ctx: &Ctx, id: iroh::EndpointId) -> Option<iroh::EndpointAddr>;
}
```
- DNS: `DnsAddressLookup::builder(N0_DNS_ENDPOINT_ORIGIN_PROD.to_string()).dns_resolver(ep.dns_resolver()?.clone()).build()`
  (`iroh-1.0.3/src/address_lookup/dns.rs:60-83`), then `AddressLookup::resolve(id)`
  (`address_lookup.rs:333-350`, `dns.rs:113-135`). It uses the same `[200,300,600,1000,2000,3000]` ms
  stagger. Avoid `DnsAddressLookup::n0_dns()`, which honours `IROH_FORCE_STAGING_RELAYS`.
- mDNS: port go-iroh `iroh/mdns` (3.3). Resolver side for clients: `Start` (IPv4 socket required,
  IPv6 best effort, `socket2` with `SO_REUSEADDR`+`SO_REUSEPORT`, join on every up multicast interface
  via `nix::ifaddrs::getifaddrs` and `if_nametoindex`), the cache map, `query(id)` with the 2-question
  PTR packet, a parser that accepts records in any section (both Go's all-answers layout and the
  swarm-discovery additionals layout), `resolve(ctx, id)` polling every 25 ms up to 3 s. A listener
  failure logs `transport: mdns discovery unavailable` with `error`, keeps the resolver registered,
  and lookups then return nothing after 3 s.
- Merge both with `futures::stream::select` and take the first `Ok(item)` whose
  `item.endpoint_id() == id` and whose address set is non-empty.

### 4.7 Address parsing (`src/transport/addr.rs`)

```rust
pub enum GoTransportAddr {
    Relay(GoRelayUrl),                                 // string as Go normalizes it
    Ip { ip: std::net::IpAddr, zone: Option<String>, port: u16 },
    Custom { id: u64, data: Vec<u8> },
}
impl std::fmt::Display for GoTransportAddr { /* "relay:<s>", "ip:<ip>:<port>" / "ip:[<v6>%zone]:<port>", "<id:x>_<hex>" */ }
pub fn parse_transport_addr(s: &str) -> Result<GoTransportAddr, String>;   // error texts per 5.1
pub fn parse_addrs(addrs: &[String]) -> Vec<GoTransportAddr>;              // 2.4 fallback rule
impl GoTransportAddr { pub fn is_relay(&self) -> bool; pub fn to_iroh(&self) -> Option<iroh::TransportAddr>; }
pub struct GoRelayUrl(String);
pub fn parse_relay_url(s: &str) -> Result<GoRelayUrl, String>;             // Go net/url leniency + normalizeURL
```
iroh-base has no `FromStr` for `TransportAddr` (only Display, `endpoint_addr.rs:81-89`), and `url::Url`
is stricter than Go and drops default ports. So parse by hand:
- Cut at the first `:`.
- `ip` → Go `netip.ParseAddrPort` rules: IPv4 dotted quad without leading zeros; IPv6 only in
  brackets, optional `%zone`; decimal port 0–65535. Keep `::ffff:a.b.c.d` as IPv6.
- `relay` → Go-lenient URL split, lowercase the host, `/` for an empty path on special schemes. Keep
  `"relay:"` and `"relay:example.com"` as `Relay`, since they select the relay phase.
- `custom` or no colon → custom rule.
- Any other kind → error, then the bare `ip:port` fallback.
- Golden vectors: 5.1–5.4.

### 4.8 Flow-control windows (architecture §11.3, §16)

| setting | noq default (`noq-proto-1.1.1/src/config/transport.rs:544-600`) | Rust iroh override (`endpoint/quic.rs:153-163`) | recommended dstore-client-rs |
|---|---|---|---|
| `stream_receive_window` | 1,250,000 (12.5 MB/s × 100 ms) | – | **16 MiB** |
| `receive_window` | `VarInt::MAX` | – | leave default |
| `send_window` | 10,000,000 | – | **64 MiB** |
| `max_concurrent_bidi_streams` | 100 | – | **1024** (Go parity) |
| `max_idle_timeout` | 30 s | – | **60 s** (Go parity) |
| `keep_alive_interval` | None | 5 s | 5 s |
| path keepalive / path idle | – | 5 s / 15 s | default |
| multipath paths / QNT addrs | – | 8 / 32 | default |
| `initial_rtt` | 333 ms | – | default (RTT sentinel) |

Rationale: BDP(1 Gbit, 43 ms) ≈ 125 MB/s × 0.043 s ≈ 5.4 MB. go-iroh reaches line rate with its
6 MiB auto-tuned stream window; 16 MiB leaves ~3× headroom. The send window bounds unacked data
across a connection's streams, and `Conns` = 4 put batches can be in flight per node.
Flow-control windows are local limits advertised in transport parameters and do not affect wire
compatibility. Measure with the §16 methodology before freezing (8, open decision).

### 4.9 In-memory transport (`src/transport/mem.rs`)

Port `mem.go` for client tests (a fake node that speaks the wire protocol in-process, since there is
no Rust node):
```rust
pub struct Network { st: std::sync::Mutex<NetState> }   // endpoints, down: HashSet, cut: HashSet<(NodeId,NodeId)>, delay
impl Network {
    pub fn new() -> Arc<Network>;
    pub fn set_down(&self, id: NodeId, down: bool);
    pub fn partition(&self, a: NodeId, b: NodeId, cut: bool);
    pub fn set_delay(&self, d: Duration);
    pub fn bind(self: &Arc<Self>, id: NodeId, alpns: &[&str]) -> Arc<MemEndpoint>;
}
pub struct MemEndpoint { /* accept: mpsc(64), conns: Mutex<HashSet<ConnId>>, closed: AtomicBool, alpns */ }
struct MemConn { /* peer: Weak<MemConn>, streams: mpsc(256), done: CancellationToken, once */ }
pub struct PipeReader / PipeWriter   // shared Mutex<PipeState{buf, closed, canceled}> + wakers; 4 MiB limit
```
- Keep every error text and ordering from 2.9, `Path = {direct: true, rtt: 1ms}` and
  `addrs = ["mem:<shortid>"]`.
- Close the peer conn from a spawned task (Go `go c.peer.Close()`) to avoid re-entrancy.
- Decision for the port: optionally wake pipe readers when their conn closes. This deviates from Go
  and is invisible on the wire.
- `node/cluster_test.go` uses `SetDown` (`:431,437,741`) and `SetDelay` (`:669-670`); port them
  all, including `Partition`.

### 4.10 Node-side needs (record; node commands need node internals)

- `serve`, `cluster init`, `node join` bind with `ALPNs [amber-dstore/1, amber-dstore-cluster/1]`,
  `Discover = Announce = !--no-discovery`, `Loopback = --loopback`, and `Advertise` from
  `--advertise-addr` (ip:port or bare ip → port 0). Go bug: port 0 is never replaced; `AddExternalAddr`
  ignores it but `Addrs()` publishes `ip:<ip>:0` (`cmd/dstore/main.go:172-183`,
  `transport/iroh.go:122-125`).
- `BindAddr` from `--bind`, or the `<store>/port` file (`netip.AddrPortFrom(IPv4Unspecified, port)`,
  IPv4-only). After bind the port is written back as `"<port>\n"`, 0644 (`main.go:184-205`). Identity:
  `<store>/identity`, hex seed + `"\n"`, 0600 (`main.go:139-159`).
- Announce: a go-iroh-compatible mDNS **responder/publisher** (3.3, port `Publish`/`answerFor`/`respond`).
  With relays, a pkarr publisher to `https://dns.iroh.link/pkarr` with an **unfiltered** address filter,
  TTL 30 s, republish 5 min (`iroh/addresslookup_pkarr.go:25-38`). Rust:
  `iroh::address_lookup::PkarrPublisher::builder(N0_DNS_PKARR_RELAY_PROD.parse()?).addr_filter(AddrFilter::unfiltered())`
  (`address_lookup/pkarr.rs:127,143,146,217,290`; `iroh-dns-1.0.3/src/endpoint_info.rs:243-274`).
  Plus the 30 s republish loop keyed on `Addrs().join(" ")`.
- An `accept` loop (`node/server.go:16-55`), `Pool` with `perPeer` 2 (`node/node.go:245`), and
  `offlineEndpoint` (`node/node.go:785-792`).
- Deriving a ticket from `--store` (`cmd/dstore/main.go:394-405`) needs `node.OpenOffline(dir)`
  (Pebble meta, view decoding) and `worktree.TicketFromView`. No transport is involved.

---

## 5. Golden vectors (Go generator should emit; values marked ✓ verified with go1.26.5 against go-iroh v0.2.0 and dstore v0.1.9)

### 5.1 `netaddr.ParseTransportAddr` and `transport.ParseAddrs` ✓

| input | ParseTransportAddr | ParseAddrs |
|---|---|---|
| `ip:127.0.0.1:9` | IPAddr `ip:127.0.0.1:9` | `[ip:127.0.0.1:9]` |
| `ip:[::1]:9` | IPAddr `ip:[::1]:9` | `[ip:[::1]:9]` |
| `ip:[fe80::1%en0]:9` | IPAddr `ip:[fe80::1%en0]:9` | `[ip:[fe80::1%en0]:9]` |
| `127.0.0.1:9` | err `transport address "127.0.0.1:9": unknown kind "127.0.0.1"` | `[ip:127.0.0.1:9]` |
| `[::1]:9` | err `transport address "[::1]:9": unknown kind "["` | `[ip:[::1]:9]` |
| `[::ffff:1.2.3.4]:5` | err `transport address "[::ffff:1.2.3.4]:5": unknown kind "["` | `[ip:[::ffff:1.2.3.4]:5]` |
| `ip:[::ffff:1.2.3.4]:5` | IPAddr `ip:[::ffff:1.2.3.4]:5` | same |
| `ip:1.2.3.4` | err `transport address "ip:1.2.3.4": not an ip:port` | `[]` |
| `ip:999.1.1.1:5` | err `transport address "ip:999.1.1.1:5": ParseAddr("999.1.1.1"): IPv4 field has value >255` | `[]` |
| `ip:01.2.3.4:5` | err `transport address "ip:01.2.3.4:5": ParseAddr("01.2.3.4"): IPv4 field has octet with leading zero` | `[]` |
| `ip:1.2.3.4:0` | IPAddr `ip:1.2.3.4:0` | `[ip:1.2.3.4:0]` |
| `ip:1.2.3.4:65536` | err `transport address "ip:1.2.3.4:65536": invalid port "65536" parsing "1.2.3.4:65536"` | `[]` |
| `relay:https://use1-1.relay.n0.iroh-canary.iroh.link./` | RelayAddr same | same |
| `relay:https://Example.COM` | RelayAddr `relay:https://example.com/` | same |
| `relay:http://example.com:80` | RelayAddr `relay:http://example.com:80/` | same |
| `relay:example.com` | RelayAddr `relay:example.com` | same |
| `relay:` | RelayAddr `relay:` | same |
| `relay:https://ex ample.com` | err `failed to parse relay URL: parse "https://ex ample.com": invalid character " " in host name` | `[]` |
| `relay:https://example.com:443/x?y=1` | RelayAddr `relay:https://example.com:443/x?y=1` | same |
| `mem:0102abcd` | err `transport address "mem:0102abcd": unknown kind "mem"` | `[]` |
| `custom:1_abcd` | CustomAddr `1_abcd` | `[1_abcd]` (goes to the **direct** list) |
| `1_abcd` | CustomAddr `1_abcd` | `[1_abcd]` |
| `abc` | err `missing '_' separator` | `[]` |
| `` (empty) | err `missing '_' separator` | `[]` |
| `http://x` | err `transport address "http://x": unknown kind "http"` | `[]` |
| `custom:zz_00` | err `invalid ID` | `[]` |
| `IP:1.2.3.4:5` | err `transport address "IP:1.2.3.4:5": unknown kind "IP"` | `[]` |

### 5.2 `netaddr.ParseRelayURL` ✓

| input | String() / error |
|---|---|
| `https://Example.COM` | `https://example.com/` |
| `https://example.com:443` | `https://example.com:443/` |
| `https://example.com/path` | `https://example.com/path` |
| `HTTPS://example.com` | `https://example.com/` |
| `relay.example.com` | `relay.example.com` (accepted) |
| `https://example.com?x=1` | `https://example.com/?x=1` |
| `https://user@example.com` | `https://user@example.com/` |
| `https://ex ample.com` | `failed to parse relay URL: parse "https://ex ample.com": invalid character " " in host name` |
| `:bad` | `failed to parse relay URL: parse ":bad": missing protocol scheme` |

### 5.3 Default relay addresses (`relay.DefaultMap().URLs()` order) ✓

```
relay:https://aps1-1.relay.n0.iroh-canary.iroh.link./
relay:https://euc1-1.relay.n0.iroh-canary.iroh.link./
relay:https://use1-1.relay.n0.iroh-canary.iroh.link./
relay:https://usw1-1.relay.n0.iroh-canary.iroh.link./
```

### 5.4 `netaddr.IPAddr{}.String()` ✓

`192.168.1.2:4242` → `ip:192.168.1.2:4242`; `[2001:db8::1]:4242` → `ip:[2001:db8::1]:4242`;
`[::ffff:10.0.0.1]:1` → `ip:[::ffff:10.0.0.1]:1`; `[fe80::1%en0]:7` → `ip:[fe80::1%en0]:7`.

### 5.5 Endpoint id validation (`irohkey.NewEndpointID`) ✓

32-byte arrays with byte 0 = i and the rest zero: i ∈ {0,1,3,4,5,6} valid; i = 2 and i = 7 →
`data is not a valid public key`. All `0xff` bytes (non-canonical y) → **valid** in Go. The Rust
`iroh::EndpointId::from_bytes` (ed25519-dalek decompression) must give the same answers; add this
as a test.

### 5.6 Identity-derived strings ✓ (label computed with RFC 4648 base32)

Seed bytes `01 02 … 20` (`irohkey.NewSecretKey`):
- id `79b5562e8fe654f94078b112e8a98ba7901f853ae695bed7e0e3910bad049664`
- `Short()` `79b5562e8f`
- `Z32()` `xg4icmwxh3kx1odasrjqtkcmw6eb9bj4h4k57i9yhqeozmer131y`
- mDNS label `pg2vmlup4zkpsqdywejorkmlu6ib7bj242k35v7a4oiqxlieszsa` (to be confirmed by the generator
  through go-iroh's `endpointLabel`)
- `transport: no candidate addresses for 79b5562e8f`

### 5.7 Error joining ✓

`errors.Join(fmt.Errorf("dial %s: %w", IPAddr{127.0.0.1:9}, errors.New("x")), fmt.Errorf("discovery: %w", errors.New("y")))`
prints `dial ip:127.0.0.1:9: x\ndiscovery: y`. `transport.ErrClosed.Error()` = `transport: closed`.

### 5.8 mDNS packets (generator: a `_test.go` in a temp copy of go-iroh's `iroh/mdns` package, since the builders are unexported)

- `buildQuery(serviceName("irohv1"), instanceName("irohv1", id(seed 01..20)))`: full hex.
- `buildAnnouncement("irohv1", announcementInfo(dns.NewEndpointData(IPAddr 192.168.1.2:4242, IPAddr [2001:db8::1]:4242, IPAddr 10.0.0.1:5555, RelayAddr https://use1-1.relay.n0.iroh-canary.iroh.link./)))`:
  full hex. Expected: port 4242, the 5555 address dropped, ANCOUNT 5.
- The same with user data `"dstore"`.
- Parse vectors, each giving an expected `EndpointInfo` or "no announcement":
  - the Go announcement above;
  - a swarm-discovery-shaped response (SRV/TXT in answers, A/AAAA in additionals, target
    `<label>-<port>.local.`, `swarm-discovery-0.6.3/src/sender.rs:165-205`);
  - a packet for service `other`;
  - a compressed-name packet;
  - a truncated packet.
- `parseQuestions` cases for the responder (node side): PTR for the service, ANY for the instance, SRV
  for the service (must not answer).

### 5.9 Behavioural scripts (a Go harness over `transport.NewNetwork()`, printing outcomes; mirror them as Rust tests)

1. Pool `perPeer=4` against one mem node: 6 sequential `Get`s dial 4 times and the conns returned
   follow index order 0,1,2,3 then round robin by `next % 4`.
2. `Get` to a down node → `mem: <short> unreachable`. An immediate second `Get` →
   `transport: peer recently unreachable`. After 2 s it dials again.
3. `Drop` then `Get` → a new dial. `Close` then `Path` → `false`.
4. `Call` with a ctx that expires while the peer never answers → `context deadline exceeded`. In mem,
   `CancelRead` cancels the caller's inbound pipe, which is the peer's outbound pipe
   (`newStreamPair`, `mem.go:259-263`): a later peer `Write` returns `mem: stream reset by peer`,
   and a caller `Read` returns `mem: read canceled`.
5. `Call` receiving a `TErr` frame → both the reply and a `*wire.Error`.
6. `Path()` of a mem conn → `{true, 1ms}`. Via `client.pathAttrs`: `path direct rtt 1ms`.
7. Durations: `time.Duration.Round(time.Millisecond).String()` for 0, 400µs, 500µs, 1499µs,
   12.3ms, 1.2345s (for the log/TUI rtt).

---

## 6. Go tests worth porting

dstore `transport/iroh_test.go`:
- `TestIrohLoopback`: two real endpoints, relays disabled, loopback advertised, `TPing`→`TPong` with
  epoch +1, `conn.Path().Direct`.
- `TestIrohDialWaitsForHandshake`: keep the dead-address dial (fails within `DirectTimeout` =
  500 ms, asserted < 3 s), the dead+live race (live wins, ping works), and repeated dials to the same
  server (every returned conn has completed its handshake). The 0-RTT resumption loop
  (`Used0RTT`) has no counterpart because Rust `connect` returns `Connection<HandshakeCompleted>`;
  keep the loop count small instead.
- `TestIrohPathRTTIsUnknownUntilSampled`: RTT is 0 or < 50 ms right after dial and after one exchange
  on loopback. This exercises the 333 ms sentinel.
- `TestIrohDiscoverByID`: server `Loopback+Discover+Announce`, client `Discover` dials by id alone;
  `ip:127.0.0.1:9` falls back to discovery; a client without discovery fails by id. Skip when no mDNS
  listener can be opened. Add an **interop** variant: a Go `dstore serve --no-relay --loopback` node
  announcing and the Rust client resolving it.

go-iroh (for the mDNS port): `TestAnnouncementRoundTrip`, `TestDiscoveryResolveFromPacket`,
`TestDiscoveryAnnouncementUsesLocalID`, `TestServiceNameIsolation`, `TestEndpointLabelMatchesRustMDNS`,
`TestAnnouncementInfoPreservesIPv6Zone`, `TestAnnouncementPort`, `TestAnswerForQuery` (node),
`TestHandlePacketAnswersQueries` (node), `TestListenIPv6MDNS`, `TestReadLoopCachesAnnouncementOverIPv6`
(`iroh/mdns/mdns_test.go`). For `addr.rs`: `TestRelayURLNormalization`,
`TestTransportAddrStringRoundTrip`, `TestCustomAddrParseErrors`, `TestCustomAddrStringPrefix`,
`TestTransportAddrTextRoundTrip` (`netaddr/endpointaddr_test.go`), and `TestDefaultMapHasN0Relays`
(`relay/relay_test.go`).

Integration: `node/cluster_test.go` needs Go nodes. Port its client scenarios as an e2e interop suite
that runs `scripts/e2e-loopback.sh` with the Rust binary for client commands (`push`, `refs`, `ls`,
`cat`, `pull`, `refs --ticket <ids>`, `ref get`, `ref delete`, `watch`) against Go `dstore serve`
nodes (`--no-relay --loopback`).

New Rust unit tests (no Go counterpart exists): Pool scripts 5.9(1)–(5), `parse_addrs` 5.1,
`parse_relay_url` 5.2, `interface_ips` filtering with a fake interface list (bridge prefixes, down,
loopback, link-local, dedup, unmap), and the mem pipe semantics from 2.9.

---

## 7. Gaps in core-rs and Rust iroh, with workarounds

core-rs (`amber-store-core` 0.3.0) has no networking at all (deps: blake3, crc32c, zstd, redb,
thiserror, memmap2, libc, xattr). Everything in this note is new code.

| gap | evidence | workaround |
|---|---|---|
| No `FromStr` for `TransportAddr`; Display renders `custom:` and url-crate forms | `iroh-base-1.0.3/src/endpoint_addr.rs:54-89` | own `addr.rs` (4.7) with Go parse/format rules and golden vectors 5.1–5.4 |
| `RelayUrl` parsing is stricter than Go `net/url` and drops default ports | `iroh-base-1.0.3/src/relay_url.rs:38-45`, 5.2 | keep the Go-normalized string for display and phase logic; convert with `url::Url` only for dialing; skip unparseable ones at connect |
| Default relay map is the production hosts `use1-1.relay.n0.iroh.link.` etc., not go-iroh's `iroh-canary` hosts | `iroh-1.0.3/src/defaults.rs:27-43` vs `relay/relay.go:25-30` | `RelayMode::Custom(RelayMap::from_iter(<5.3 URLs>))` (QUIC port 7842 via `RelayConfig::from`, `iroh-relay-1.0.3/src/relay_map.rs:272-310`) |
| `IROH_FORCE_STAGING_RELAYS` switches `default_relay_mode`, `DnsAddressLookup::n0_dns`, `PkarrResolver::n0_dns` | `endpoint.rs:1970-1987`, `address_lookup/dns.rs:92-98`, `pkarr.rs:525-528` | never use those helpers; pass explicit origins and URLs |
| `iroh-mdns-address-lookup` 0.4.0 cannot read go-iroh v0.2.0 announcements (A/AAAA expected in additionals); 10 s lookup | `swarm-discovery-0.6.3/src/receiver.rs:77-160`, `iroh/mdns/dnsmsg.go:42-88`, `lib.rs:95` | port go-iroh `iroh/mdns` (4.6), parsing records in any section |
| Endpoint-attached lookups run on every connect without a selected path | `remote_state.rs:850-881` | explicit discovery only in `discover_dial` (4.6) |
| No per-address dial isolation; datagrams fan out to all known paths, and paths persist per remote (actor idle 60 s) | `remote_state.rs:73,788-830` | one connect per phase. A "direct-only" phase may also probe relay paths learned by an earlier dial; accept this |
| No "RTT sampled" flag on `Path` | `path_watcher.rs:494-496`, `noq-proto-1.1.1/src/connection/paths.rs:772-804` | compare with the configured `initial_rtt` (333 ms) |
| No handshake idle timeout separate from `max_idle_timeout` (60 s configured); Go qng fails a silent handshake after 5 s idle / 10 s total | `internal/qng/internal/protocol/params.go:97`, `internal/qng/config.go:18-20` | always bound connects by the ctx deadline. Optionally cap each connect phase at 10 s to approach Go's timing (open decision) |
| `Endpoint::online()` never returns when relays are disabled | `endpoint.rs:1358-1373` | call only with relays configured, under a 10 s timeout |
| Portmapper on by default | `iroh-1.0.3/src/portmapper.rs:35-39`, `Cargo.toml:68-73` | feature off plus `PortmapperConfig::Disabled` |
| `Connection::close` does not wait for CONNECTION_CLOSE to be flushed | `endpoint/connection.rs:959-961` vs `internal/qng/connection.go:3012-3019` | `Endpoint::close().await` (bounded) at CLI exit |
| QUIC/iroh error texts differ from qng's | `noq-proto-1.1.1/src/connection/mod.rs:7407-7435`, `iroh/endpoint.go:1273,1426` | keep dstore-level wrappers byte-identical (3.4); document the inner divergence |
| IPv6 zones: `SocketAddr::from_str` rejects `%zone` | Go accepts (5.1) | parse zones by hand, `if_nametoindex` → `scope_id` |
| `interfaceIPs` needs IFF_UP flags and interface names | `transport/ifaces.go:22-55` | `nix::ifaddrs::getifaddrs()` (`nix-0.30.1/src/ifaddrs.rs:20-32,172`) filtered as in 2.8 |
| Go `Pool` dial mutex is not ctx-aware | `transport/transport.go:124` | `tokio::sync::Mutex`, same semantics. Optionally `select!` on the ctx while waiting; the only visible effect is earlier failure |

---

## 8. Risks and open decisions

1. **Rust iroh 1.0.3 vs 1.1.0.** Recommendation: pin `iroh = "=1.0.3"` (noq/noq-proto/noq-udp 1.1.x).
   - Evidence for 1.0.3: go-iroh v0.2.0's `COMPATIBILITY.md` verifies Rust 1.0.3 in every stable
     scenario, including `handshake/rust-client-go-server`, `handshake/zero-rtt`,
     `handshake/close-semantics`, relay protocol, DNS/pkarr and key vectors.
   - 1.1.0 is only pinned for CustomAddr ticket vectors, which dstore does not use: its tickets are
     dstore CBOR.
   - What changes in 1.1.0: the client TLS stops sending SNI (`iroh-1.1.0/src/tls.rs:88-94`).
     go-iroh servers do not use SNI (`iroh/tls.go:173-195`, `VerifyConnection` checks only the client
     key), so this should interoperate but is unverified.
   - The relay protocol adds `Status::RateLimited` = discriminant 2 (`iroh-relay-1.1.0/src/protos/relay.rs`),
     which 1.0.3 decodes as `Unknown(2)`.
   - Mapped addresses are random; net_report gains proxy support; noq 1.2.0 with no diff in
     `transport_parameters.rs`/`frame.rs` beyond comments. There is no QUIC multipath or QNT
     wire-format change in those files.
   - The public API is unchanged: `endpoint.rs`, `endpoint/*.rs` and `address_lookup*` are identical,
     so moving to 1.1.0 is a Cargo change. Architecture §11.3/§16 measured throughput with Rust 1.1.0.
   - Gate any switch on the e2e interop suite (6).
2. **Default relay hosts.** Canary hosts for Go parity (recommended) vs the Rust production hosts.
   Nodes publish their own home relay URL in the view, so dialing nodes via relay works either way.
   The choice affects the client's home relay, the 10 s online wait and net_report probes.
3. **Flow-control window constants** (4.8) have no Go counterpart. They need measurement (§16).
   They are memory/throughput choices, not compatibility.
4. **mDNS implementation.** Porting go-iroh's resolver (recommended) is extra code (socket2, nix,
   hand-rolled DNS parse). The crate route cannot interoperate (3.3). Port the responder/publisher
   too if node commands are ever ported.
5. **Dial error text shape.** dstore wrappers are identical; inner texts come from iroh/noq. Joined
   per-candidate lines reuse one inner error (Go has one per candidate, in completion order, which is
   nondeterministic anyway).
6. **RTT sentinel** misreads a true 333.000 ms smoothed RTT as unmeasured (class 3 → 0). Negligible.
7. **Timing differences.**
   - Rust connects have no 5 s handshake idle timeout. Relay-phase failures last until the ctx
     deadline (15 s bootstrap, 2 min requests) unless capped (7).
   - A mDNS listener that cannot bind makes lookups skip mDNS in Go only after its 3 s timeout; in
     Rust, register the resolver anyway to keep the 3 s wait.
   - Go's `discoverDial` `Connect` does not await the handshake with a cached ticket; Rust always does
     (stricter, fine).
8. **`Online()` 10 s bind delay** with relays enabled is Go behaviour. Keep it for parity (a flaky
   relay map slows every command) or make it an opt-out (divergence). Recommend parity.
9. **`--relay` leniency.** Go accepts `--relay relay.example.com` (never connects; online times out
   after 10 s). Rust `url::Url` rejects it. Either replicate the acceptance through `GoRelayUrl`
   (then undialable) or reject with `failed to parse relay URL: …` (divergence). Recommend
   replicating the acceptance and skipping at connect.
10. **Endpoint-id edge encodings** (5.5): dalek and filippo edwards25519 may differ on non-canonical
    points. Test vectors required. Ids come from views/tickets, so real ids are canonical.
11. **mem transport quirk** (blocked reads survive conn close). Decide whether the Rust mem port
    mirrors it (faithful) or wakes readers (more robust tests). No wire impact.
12. **Pool growth.** Port Go (grow to `perPeer`, never shrink), not architecture §11.3's 90 s shrink.
13. **Node-side commands** need the transport pieces in 4.10: ALPNs, announce (mDNS responder plus
    pkarr publisher with unfiltered addrs), port file, `Addrs()` format, accept loop. Also Pebble-backed
    `node.OpenOffline` for `--store` tickets. Decide whether the Rust CLI implements node commands at
    all.
14. **Go bug to keep or fix:** `--advertise-addr <ip>` publishes `ip:<ip>:0` (4.10).
15. **Architecture vs code:** §11.3 "race the relay only on retry" vs code (relay immediately after
    the direct phase in the same `Dial`); §2.2 "Transport failures drop the connection" vs `Call`
    (only `OpenStream` failures drop). Port the code.

---

## Addenda (synthesis)

Added by the architecture synthesis. `PORTING.md` is normative where it differs from this spec.

1. **Crates.** `dstore-transport` holds the traits, `Pool`, `addr` and `mem`, with no iroh
   dependency. `dstore-transport-iroh` holds the endpoint, dial phases, discovery, `mdns` and `ifaces`.
   - Traits: boxed `SendStream`/`RecvStream` plus `async-trait`, replacing §4.3's enum halves.
   - `Pool::new(.., per_peer: usize)`.
   - `Pool::call` returns `CallError { Transport | Wire | Ctx | Remote }`.

   Signatures: PORTING.md §4.6-§4.7.
2. **Open decisions resolved.**
   - iroh `=1.0.3` with `default-features = false` and `tls-ring`, `fast-apple-datapath`.
   - The canary relay map.
   - Stream receive window 16 MiB, send window 64 MiB.
   - The go-iroh mDNS resolver is ported. The responder and publisher are out of v1:
     `announce = true` returns `transport: bind: announce is node-side and not implemented`.
   - Discovery runs only in `discover_dial`.
   - Each connect phase is capped at 10 s (`CONNECT_PHASE_CAP`).
   - The 10 s online wait at bind is kept.
   - `--relay` leniency is kept; an undialable URL gives `RelayMap::empty()`.
   - The mem transport quirk is kept (faithful).
   - The `--advertise-addr` bug is not relevant (node-side).
3. **RTT sentinel.** Set `initial_rtt(NOQ_INITIAL_RTT)` explicitly;
   `QuicTransportConfigBuilder::initial_rtt` exists at `iroh-1.0.3/src/endpoint/quic.rs:281`.
4. **CLI exit.** `Endpoint::close()` is awaited, bounded to 3 s, after `Cluster::close()` (PORTING DD-11).
5. **iroh 1.0.3 APIs confirmed at synthesis.**
   - `iroh-1.0.3/src/endpoint.rs`: `Endpoint::online` (:1358), `Endpoint::close` (:1706),
     `bound_sockets` (:1445), `add_external_addr` (:1011), `Builder::portmapper_config` (:786),
     `clear_ip_transports` (:503), `RelayMode::{Disabled, Default, Staging, Custom}` (:1925).
   - `iroh-1.0.3/src/endpoint/presets.rs`: `presets::Minimal` (:59).
   - `iroh-relay-1.0.3/src/relay_map.rs`: `RelayMap::empty` (:47),
     `impl FromIterator<RelayUrl> for RelayMap` (:203), `impl From<RelayUrl> for RelayConfig` (:272).
   - `iroh-base-1.0.3/src/key.rs`: `PublicKey::from_bytes` (:122).
6. **Scope** confirmed: swarm-discovery reads A/AAAA records only from the additionals section
   (`swarm-discovery-0.6.3/src/receiver.rs:77-160`), so no Rust service uses iroh-mdns-address-lookup
   (PORTING C1).
