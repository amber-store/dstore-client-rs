# Port notes: cluster view and placement

Normative reference: `github.com/amber-store/dstore` at tag `v0.1.9` (HEAD `368f2c7`). The module cache copy
`/Users/dragan/go/pkg/mod/github.com/amber-store/dstore@v0.1.9` is byte-identical to the checkout for every file
cited here (checked with `cmp`: view, placement, worktree/flow.go, node/status.go, client/{client,rank,objects}.go,
ticket, codec, transport/iroh.go, cmd/dstore/client.go). So a vector generator can pin `v0.1.9` without a `replace`.

Every concrete value in this document was produced by running the real Go code (go1.26.5, offline module cache)
through the generator in Appendix A. Nothing was computed by hand.

---

## 1. Scope

### 1.1 Files covered

| Go file | lines | what the Rust port needs from it |
|---|---:|---|
| `view/view.go` | 469 | all of it: `NodeID`, `ParseNodeID`, `IDString`, `ShortID`, the view types and CBOR tags, `Encode`/`Decode`/`Clone`, `Compare`, the lookups, `Placement` (owners, write set, read order), `Contains`/`AddID`/`IDsOf`/`SortNodes`, `DefaultMinReplicas`, `ValidateChange` |
| `placement/placement.go` | 272 | all of it: slot, salt, fmix64, log2fix, L, the rank comparison, `Set.Rank`, `Set.Owners`, `Table` |
| `placement/placement_test.go` | 234 | the frozen golden vectors and the statistical tests |
| `codec/codec.go` | 38 | the CBOR modes views are encoded and decoded with (`cbor.CanonicalEncOptions()`, `cbor.DecOptions{}`) |
| `worktree/flow.go` | 331 | `TicketFromView` (L305–315), `RefreshTicket` (L317–331) |
| `node/status.go` | 124 | `node.ShortID` (L118–124) |
| `client/client.go` | 403 | how the client caches, adopts and places by the view: `Dial` L59–117, `adoptReply` L136–154, `adopt` L156–167, `addrsOf` L169–178, `stamp` L189–197, `handleErr` L201–218, `penalty` L228–239, `call` L266–281, `Primary`/`Owners`/`WriteSet`/`ReadOrder` L304–326, `anyNode` L329–362, `Nodes` L393–403 |
| `client/rank.go` | 71 | `rttClass`, `rankOwners` (path preference over placement order) |
| `client/objects.go` | 395 | `primaryExcept` L64–71, holder fallback to `WriteSet` L128, `Placed` (ack policy) L306–329 |
| `client/fetch.go` | 309 | `route` L180–203 (walks `ReadOrder`, refreshes once) |
| `client/tree.go` | 479 | `directFill` L171–195, `mergeIDs` L208–218, `shortError` L220–238 (use `WriteSet`, `ShortID`) |
| `client/watch.go` | 245 | `allNodes` L98–108 |
| `transport/iroh.go` | 457 | the address strings nodes publish (L112–133, `Addrs` L225–243), `ParseAddrs` L247–261, `Dial` L265–302, `discoverDial` L307–319, `raceConnect` L322–372, `localAddrPorts` L448–457 |
| `transport/ifaces.go` | – | `interfaceIPs` L22–: no loopback, no link-local, no zones |
| `transport/transport.go` | 243 | `AddrsFunc` L61, and `Pool.Get` calling `addrs(id)` on every dial (L94–146) |
| `ticket/ticket.go` | 96 | the `Ticket` type `TicketFromView` fills (the ticket area owns parsing and encoding) |
| `cmd/dstore/client.go` | 574 | `printStatus` L134–193 (prints view fields) |
| `cmd/dstore/main.go` | 698 | `cluster init` prints L310–314, `cluster ticket` L333–367, `localTicket` L394–405, `node` subcommands using `view.ParseNodeID` L464–578, `voter` L583–601 |
| `cmd/dstore/tui.go` | 374 | `view.ShortID` in the per-node table, L270 |
| `cmd/dstore/wc.go` (committed version) | – | `clone` and `init` store `worktree.TicketFromView(cl.View()).Encode()` (L160, L207) |
| `node/admin.go` | – | op `cluster-ticket` L85–96 (the server-built ticket `cluster ticket` prints) |
| `node/node.go` | 797 | `mkView = "view"` L32, `Open` L201–259, `InitCluster` L716–756, `OpenOffline` L776–783 (node-side, see §8) |
| `node/maintenance.go` | 1045 | how transitions shape views (`proposeTransition` L270–297, `nodeChange` L310–348, `commit` L488–537, `transitionText` L539–557) |
| `node/join.go` | 222 | a join appends the joiner and calls `view.SortNodes` (L125–126) |
| `architecture/dstore.md` | 2349 | §3 (L155–268), §4 (L270–356), §5.5 (L643–675), §6.2 ack policy (L795–807), §6.3 (L809–828), §8.1–8.5 (L949–1288), §11.1 (L1865–1895) |

Dependencies that affect bytes: `github.com/zeebo/blake3 v0.2.4` (salt), `github.com/fxamacker/cbor/v2 v2.9.3`
(view encoding and decoding), `github.com/tmc/go-iroh v0.2.0` (`key`, `netaddr`: id validation and address strings).

### 1.2 What the code does

- **The view** (§3) is the cluster's membership and placement configuration. One CASPaxos register holds it. Clients
  fetch it with `view`, cache it, and replace it on `stale-view`, on `not-owner`, or when a reply carries a newer
  `(incarnation, epoch)`. Clients never write views.
- **Placement** (§4) is weighted rendezvous hashing per 2^20 slot. It uses integer-only arithmetic so every
  implementation produces the same ranking. `owners` is the first `R` ranked nodes, skipping a zone already taken.
  During a transition the *write set* is the union of `owners` under `nodes` and under `pending.nodes`. The *read
  order* is the full ranking under `nodes` followed by the ranking under `pending.nodes`, with duplicates removed.
- The **client** orders a key's owners by measured path (`rankOwners`) to pick the primary. It applies the ack
  policy (`Placed`) to holder lists. It resolves dial addresses from the view, falling back to the ticket.

---

## 2. API used client-side

### 2.1 `placement` package (`placement/placement.go`)

```go
const SlotBits = 20                              // L17
const Slots = 1 << SlotBits                      // L20  (1048576)
const saltDomain = "amber-dstore/placement/1"    // L23  (24 bytes, no NUL, no length prefix)
type NodeID [32]byte                             // L26
type Member struct { ID NodeID; Weight uint32; Zone string } // L29-33
```

**`Slot(key [32]byte) uint32`** (L37–39) returns `uint32(BE64(key[24:32]) >> 44)`, the top 20 bits of the last 8 bytes.

**`Salt(id NodeID) uint64`** (L43–50) is `BE64(first 8 bytes of BLAKE3("amber-dstore/placement/1" ‖ id))`. Go reads 8
bytes from the XOF (`h.Digest().Read`). The generator verified that this equals the first 8 bytes of the normal
32-byte digest (`blake3.Sum256`) for 10 ids. In Rust this is `blake3::Hasher::new().update(domain).update(&id).finalize()`,
taking bytes `[0..8]` big-endian.

**`Fmix64(x)`** (L53–60) is the murmur3 finalizer with wrapping multiplies:
`x ^= x>>33; x *= 0xff51afd7ed558ccd; x ^= x>>33; x *= 0xc4ceb9fe1a85ec53; x ^= x>>33`.

**`Log2Fix(x uint64) uint64`** (L64–81) returns `⌊log2(x)·2^32⌋`. It panics with `"placement: Log2Fix(0)"` on 0.
`L` never passes 0. The algorithm:

```
i = bits.Len64(x) - 1            // Rust: 63 - x.leading_zeros()
m = x << (63 - i)
f = 0
for j = 1..=32:
    hi, lo = bits.Mul64(m, m)    // Rust: let p = (m as u128) * (m as u128); hi = (p >> 64) as u64; lo = p as u64
    if hi & (1<<63) != 0 { m = hi; f |= 1 << (32 - j) }
    else                 { m = hi<<1 | lo>>63 }
return i<<32 | f
```

**`L(slot uint32, salt uint64) uint64`** (L85–91):
`h = Fmix64(uint64(slot) ^ salt)`. If `h == 2^64-1` it returns `0`. Otherwise it returns
`64<<32 - Log2Fix(h+1)`. So `L ∈ [1, 0x40_0000_0000]` when `h ≠ 2^64−1`; `h = 0` gives `L = 0x4000000000`.

**`less(a, b Member, la, lb uint64) bool`** (L101–111) says whether `a` ranks above `b`. It compares the full 128-bit
products `a.Weight·lb` and `b.Weight·la`: the larger product ranks above. On a tie it returns
`compareID(a.ID, b.ID) < 0`, so the smaller id ranks above. `compareID` is a bytewise compare (L113–123). This is an
exact rational comparison of `w/L`, with `L = 0` acting as +∞, so it is a strict weak ordering. Any stable sort
gives the same permutation Go's `sort.SliceStable` gives.

`type Ranked struct{Index int; L uint64}` (L94–97) is declared but unused. Do not port it.

**`NewSet(members []Member) *Set`** (L134–140) precomputes a salt for every member, including weight-0 members.

**`(*Set).Rank(slot) []int`** (L145–166) returns member indexes:
1. It splits members into `weighted` (Weight > 0, keeping input order, with L computed) and `zero` (Weight == 0).
2. It sorts `weighted` with `sort.SliceStable` using `less`.
3. It sorts `zero` by id with `sort.Slice`, which is unstable. With duplicate ids the order among the duplicates is
   unspecified, but their ids are equal, so the id output is identical.
4. It returns `weighted ++ zero`. The result is never nil for a non-empty set. For an empty set Go returns
   `append([]int{}, …)` of length 0, and `Table.Rank` turns nil into `[]int{}`.

**`(*Set).Owners(slot, r int) []int`** (L171–194) walks `Rank(slot)`:
- It stops when `len(out) >= r`.
- It stops (`break`) at the first weight-0 member. Weight-0 members never own anything.
- The zone key is `m.Zone`, or `string(m.ID[:])` (the raw 32 bytes) when `Zone == ""`.
- It skips a member whose zone key is already taken. Otherwise it takes the member and marks its zone.
- `make([]int, 0, r)` panics for negative `r`. Views only pass `int(uint8)`, so this cannot happen. Use `usize` in Rust.
- `r = 0` returns an empty list.
- A set with fewer distinct zones than `r` returns fewer owners, for example two zones with `r=3` gives 2 owners.

The zone key space mixes explicit zone strings and raw id bytes. An explicit zone equal to another member's raw id
bytes collides. This is only possible when those bytes are valid UTF-8, because CBOR text is validated on decode.
Use byte-string zone keys in Rust so collisions behave the same.

**`Table`** (L198–272) is `NewTable(set, r)` plus a lazy per-slot cache of `Owners(slot, r)` and `Rank(slot)`. Go
allocates two `[][]int` of length 2^20 per table: about 48 MiB per table, and 96 MiB per view during a transition.
There is no lock; concurrent callers may compute a slot twice. Nil results are stored as `[]int{}`. The table
exposes `OwnerIDs(key) []NodeID` (always non-nil), `RankIDs(key) []NodeID` (always non-nil), `IsOwner(key, id) bool`,
`Set()` and `Replicas()`.

### 2.2 `view` package (`view/view.go`)

**Node ids and text forms**

- `type NodeID = placement.NodeID` (L19) is a plain 32-byte array. It is **not** validated as an ed25519 point, and
  test views use ids like `0x01×32`.
- **`ParseNodeID(s string) (NodeID, error)`** (L22–30) is `hex.DecodeString(s)`. It accepts upper, lower and mixed
  case and nothing else: no `0x`, no whitespace, no base32. The error is `err != nil || len(b) != 32`, so the input
  must be exactly 64 hex characters. The error text is `fmt.Errorf("view: bad node id %q", s)` with Go `%q` quoting
  (§3.5). This is stricter than `ticket.Parse`, which also accepts iroh's 52-character base32 form. Do not unify them.
- **`IDString(id)`** (L33) is 64 lowercase hex characters.
- **`ShortID(id)`** (L36) is `hex(id[:4])`, 8 lowercase hex characters. Rust iroh's `PublicKey::fmt_short` prints
  **5** bytes (`iroh-base-1.0.3/src/key.rs:142-148`), so do not use it.
- **`node.ShortID(b []byte) string`** (`node/status.go:119-124`) returns `"?"` unless `len(b) == 32`, otherwise
  `view.ShortID`.

**Types** (exact CBOR shape in §3.1)

- `Voter{ID []byte; Since uint64}` (L39–42)
- `Former{ID []byte; Until int64}` (L45–48), nanoseconds since the epoch
- `DataEndpoint{ID []byte; Addrs []string}` (L51–54)
- `Node{ID []byte; Weight uint32; Addrs []string; Data []DataEndpoint; Token []byte; Zone string; Incarnation uint64; Writable bool}` (L57–66)
- `Pending{Nodes []Node; Replicas uint8; ID uint64; ParticipantsAck [][]byte; Participants [][]byte; Frozen bool; Round uint32; PrimaryDone [][]byte; Done [][]byte; FrozenAt int64; Reason string; Ramp *Ramp; Since int64}` (L84–98)
- `Ramp{Node []byte; Target uint32; Step int}` (L101–105). Go `int` is 64-bit on every target dstore builds for, so use `i64`.
- `View{...}` (L114–139) and `ACL{Allowed, Admins [][]byte}` (L142–145)
- `VoterSyncDone = 0`, `VoterSyncPending = 1` (L108–111). These are untyped constants compared against `View.VoterSync int`.
- `ErrNotMember = errors.New("view: not a member")` (L148)

**Node helpers**

- `(Node).NID()` (L69–73) is `copy(id[:], n.ID)`: it copies `min(len, 32)` bytes and zero-pads. A 31-byte id becomes
  a different NodeID with a trailing 0x00, and an empty id becomes all zeros. The `short_node_id` vector pins this.
- `(Node).ZoneOrID()` (L76–81) returns `n.Zone` when it is not empty, otherwise `string(n.ID)` (the raw bytes, of any
  length). Note that `n.ID` is not zero-padded here.

**Encoding and comparison**

- `(*View).Encode()` (L151) is `codec.Marshal(v)` (canonical, §3.1).
- `Decode(b)` (L154–160) wraps any error as `fmt.Errorf("view: decode: %w", err)` and returns `nil` on error.
- `(*View).Clone()` (L163–173) is encode followed by decode. It panics on error, which cannot happen for a decoded view.
- **`(*View).Compare(inc, epoch uint64) int`** (L177–191) compares incarnation first, then epoch. It returns −1 when
  `v` is older, 0 when equal, +1 when newer. `Version` is not part of it.

**Lookups**

- **`(*View).Node(id)`** (L194–208) searches `v.Nodes` first, then `v.Pending.Nodes`. It returns the first entry with
  `bytes.Equal(n.ID, id[:])`, so an entry whose ID is not 32 bytes never matches. When a node is in both lists with
  different fields, the `nodes` entry wins.
- **`IsMember(id)`** (L212–218) is `Node(id)` found, or `DataEndpointOwner(id) != nil`.
- **`DataEndpointOwner(id) *NodeID`** (L221–240) scans every data endpoint `d.ID` of `v.Nodes`, then of
  `v.Pending.Nodes`, and returns the owning node's `NID()`.
- `IsFormer(id)` (L243–250) scans `v.Former`.
- `IsVoter(id)` (L253–260) scans `v.Voters`.
- `VoterIDs()` (L263–269) copies each id with zero padding or truncation.
- `Quorum()` (L272) is `len(v.Voters)/2 + 1`.
- **`AllMembers()`** (L275–292) returns the `NID()` of each entry of `Nodes`, then of `Pending.Nodes`, deduplicated,
  in first-seen order.

**`Placement`** (L297–400)

- **`NewPlacement(v)`** (L305–312) builds `cur = NewTable(NewSet(members(v.Nodes)), int(v.Replicas))`. When
  `v.Pending != nil` it also builds `pending = NewTable(NewSet(members(v.Pending.Nodes)), int(v.Pending.Replicas))`.
- `members` (L314–320) maps each node to `Member{ID: n.NID(), Weight: n.Weight, Zone: n.ZoneOrID()}`, keeping input
  order and duplicates.
- One `sync.Mutex` serialises every lookup.
- `View()` (L323) returns the view.
- **`Owners(key)`** (L326–330) is `cur.OwnerIDs`, non-nil and possibly empty.
- **`PendingOwners(key)`** (L333–340) returns **nil** when there is no pending. Otherwise it returns
  `pending.OwnerIDs`, which is non-nil and may be empty (the `pending_no_nodes` vector gives `[]`). Callers test `po != nil`.
- **`WriteSet(key)`** (L343–358) is `Owners(key)` followed by the `PendingOwners(key)` not already present, in pending
  rank order. It takes the lock twice.
- **`ReadOrder(key)`** (L361–378) is `cur.RankIDs(key)`, which includes the weight-0 members at the end. It then
  appends the `pending.RankIDs(key)` entries not already present. Deduplication happens only against earlier output,
  and the first list itself is **not** deduplicated. A view with a duplicated node entry yields that id twice
  (`duplicate_node` vector).
- `IsOwner(key, id)` (L381–385), `IsPendingOwner(key, id)` (L388–395, false without pending), and
  `InWriteSet(key, id)` (L398–400) is `IsOwner || IsPendingOwner`.

**Id-list helpers**

- `Contains(ids [][]byte, id)` (L403–410) uses `bytes.Equal`.
- `AddID(ids, id)` (L413–418) appends a copy if absent.
- `IDsOf(raw [][]byte) []NodeID` (L421–427) copies each entry with zero padding or truncation.
- **`SortNodes(nodes)`** (L430–432) is `sort.Slice` by `bytes.Compare(ID)`, unstable. The generator sorted
  `[02, 0105, 01, nil, ff, 0100]` to `["", "01", "0100", "0105", "02", "ff"]`.

**Replica defaults and change validation**

- **`DefaultMinReplicas(r uint8)`** (L435–444) is `max(r−1, 2)` capped at `r`. Vectors: 0→0, 1→1, 2→2, 3→2, 4→3,
  5→4, 7→6, 10→9, 255→254.
- `ValidateChange(cur, target []Node, replicas int, force bool)` (L448–469) is node-side. It counts each `cur` node
  with `Weight > 0` that has no `target` entry with the same ID and `Weight > 0`. When the count is `≥ replicas` and
  `replicas > 0`, and `force` is false, it returns
  `"view: change drops %d nodes at once with R=%d; every key owned only by them would be lost (use --force)"`.

**How transitions shape views** (node-side, for realistic vectors)

- Init: one voter, one node, `Writable: true`, `Incarnation: 1`, epoch, version and placement epoch all 1
  (`node/node.go:731-736`).
- Join: `pending.nodes = nodes + joiner` at the first ramp weight, then `SortNodes` (`node/join.go:125-126`).
- Remove: the entry is dropped from `pending.nodes`.
- Drain: `Weight = 0` in `pending.nodes`. Weight and zone changes set the field (`node/maintenance.go:323-347`).
- `pending.ID = epoch+1` (L286).
- Commit: `nodes = pending.nodes`, `replicas = pending.replicas`, `min_replicas = min(min_replicas, replicas)`,
  `placement_epoch = pending.ID`, and `pending = nil`. Each removed node is appended to `former` with
  `until = now + 30 days`, and to `remove_voters` if it was a voter. Expired `former` entries are pruned (L488–537).
- `former`, `fenced`, `acl`, `ramps` and the other bookkeeping fields never affect placement.

### 2.3 How the client uses the view (`client/*.go`)

**Dial** (`client.go:59-117`)
- It builds `bootAddrs[NodeID(m.ID)] = m.Addrs` for ticket members with `len(m.ID) == 32`.
- It tries the members **in ticket order**, skipping ids that are not 32 bytes. Each attempt is `pool.Call(…, TView)`
  under `context.WithTimeout(ctx, 15*time.Second)`.
- The first reply that passes `adoptReply` wins. It logs
  `Info("connected", "node", ShortID(id), "nodes", len(View().Nodes), pathAttrs…)`.
- Errors: `"client: no endpoint"`; with no usable member `"client: ticket names no nodes"`; otherwise
  `fmt.Errorf("client: no bootstrap node answered: %w", lastErr)`.

**`adoptReply(m)`** (L136–154)
- It requires `m.Type == TViewReply`, else `"client: unexpected reply %d"`.
- It runs `view.Decode(m.View)`. A decode error is returned, so Dial moves to the next member.
- It calls `adopt(v)`.
- It then **replaces** `unreach` with the reply's `Unreachable` entries that are 32 bytes long. It does this even
  when `adopt` kept the old view.

**`adopt(v)`** (L156–167) keeps the current view when
`cur.Compare(v.Incarnation, v.Epoch) > 0 || (== 0 && v.Version <= cur.Version)`. Otherwise it installs `v` and rebuilds
`NewPlacement(v)`. In other words it adopts exactly when `(inc, epoch, version)` of `v` is lexicographically greater.

**`addrsOf(id)`** (L169–178) returns the view entry's `Addrs` when `view.Node(id)` is found and has at least one
address. Otherwise it returns `bootAddrs[id]`, which may be nil. So a member listed with no addresses falls back to
the ticket's addresses, and with none the dial goes to discovery. `Pool.Get` calls it on every dial
(`transport.go:133`), so addresses always come from the current view.

**Request stamping and replies**
- `stamp` (L189–197) sets `ClusterID`, `Incarnation` and `Epoch` from the cached view.
- `call` (L266–281): after a successful reply with `resp.Epoch > 0 || resp.Incarnation > 0` whose
  `(incarnation, epoch)` is newer than the cached view, it starts `go c.RefreshView(context.Background())`.

**Errors and backoff**
- `handleErr(id, err)` (L201–218), for a remote wire error: if it carries `View` bytes, decode them and `adopt`,
  ignoring decode errors, and return without backoff.
- Any other error increments `failures[id]` and sets backoff to `5s << min(failures-1, 4)` capped at 60 s
  (5, 10, 20, 40, 60 s).
- `ok(id)` clears backoff, failures and the unreachable hint.
- `penalty(id)` is +2 while a backoff is active, plus 1 when the view reply flagged the node unreachable.

**Owner selection**
- **`Primary(key)`** (L304–310) is `preferred(Placement().Owners(key))[0]`, and false when there are no owners. It
  considers owners under `nodes` only.
- `Owners`/`WriteSet` pass through to the placement.
- **`ReadOrder(key)`** (L319–326) is `order = Placement().ReadOrder(key)` and `r = int(View().Replicas)` (the
  *current* replica count, even during a replicas transition). When `len(order) <= r` it returns `preferred(order)`,
  otherwise `preferred(order[:r]) ++ order[r:]`. Go reads the view and the placement under two separate `RLock`s; a
  Rust port should read one snapshot.
- **`rankOwners(ids, penalty, path)`** (`rank.go:34-71`) is a stable sort by `(penalty asc, relayed asc, rttClass asc,
  input position asc)`.
  - An unmeasured node (no open connection) counts as direct with class 0.
  - `rttClass`: `< 5ms → 0`, `< 25ms → 1`, `< 100ms → 2`, otherwise 3.
  - `path` is `pool.Path(id, ALPNClient)`, the first live connection's `PathInfo{Direct, RTT}`.

**Fan-out helpers**
- **`anyNode`** (L329–362) takes ids from `v.Nodes` in view order, not pending. With none it takes the `bootAddrs`
  keys (Go map order, random). It tries `preferred(ids)` in order. On `stale-view` it retries the same node up to 4
  more times, since `handleErr` has adopted the newer view. A remote error answer stands. A transport error moves to
  the next node. With no ids at all it returns `"client: no nodes"`.
- `Nodes()` (L393–403) returns the `NID()` of `v.Nodes`, or nil without a view.
- `allNodes()` (`watch.go:98-108`) returns `Nodes()`, or the bootAddrs keys.
- **`primaryExcept(key, exclude)`** (`objects.go:64-71`) returns the first of `preferred(Owners(key))` that is not
  excluded. `Missing` records `"no owners"` for a key with none left (L48–50). A key the primary holds and that is
  absent from `short` gets `Holders = c.WriteSet(k)` (L128).

**Ack policy**
- **`Placed(k, holders)`** (`objects.go:306-329`) uses `minR = int(v.MinReplicas)` from `pl.View()`.
  - It fails when `count(Owners(k) ∩ holders) < min(minR, len(Owners(k)))`.
  - When `PendingOwners(k) != nil` it also fails when `count(po ∩ holders) < min(minR, len(po))`.
  - Otherwise it succeeds. With zero owners the threshold is 0 and the check passes.
- `Node.Writable` is **not** consulted anywhere in the client (only in `cmd/dstore/client.go:150` output). Do not add
  a "skip non-writable" rule.
- `Node.Data` (data endpoints), `DataEndpointOwner` and `ErrNotMember` are referenced nowhere outside `view/view.go`
  in v0.1.9: grep over client, cmd, worktree, transport, ticket and node, excluding tests. The client never dials data
  endpoints, even though architecture §11.3 describes it. Port the types and helpers for decode parity, and add no
  data-endpoint dialing.

**Get routing** (`fetch.go:180-203`)
- `route` indexes `ReadOrder(key)[attempt]`.
- Past the end, it refreshes the view once per fetcher and restarts at attempt 0.
- Past the end again, it reports the key missing.

**Push fill-in** (`tree.go`)
- `directFill` (L171–195) sends each short key to every `WriteSet(k)` member not in its holders.
- `shortError` (L220–238) returns `fmt.Errorf("push: %d keys could not be placed; owners not confirming: %v", len(short), names)`,
  where `names` are `"%s (%d keys)"` of `ShortID` in Go map order. `%v` of a `[]string` prints `[a b c]`.

### 2.4 Dial addresses

**What nodes put in `Node.Addrs`** (`transport/iroh.go:112-133`, `Addrs` L225–243)
- `"ip:" + netip.AddrPort.String()` for each up, non-bridge interface IP at the bound port
  (`transport/ifaces.go:22-`). Loopback, link-local, multicast link-local and unspecified addresses are excluded, and
  addresses are unmapped and deduplicated. `AddrFromSlice` carries no zone, so **no zoned IPv6 addresses are ever
  published**.
- With no such IP, `ip:127.0.0.1:<port>`.
- Then, when relays are on, `"relay:" + RelayURL.String()` after `Online` (10 s). `Addrs()` also appends relay URLs
  that appeared after bind.
- Examples: `ip:192.168.1.10:51820`, `ip:[2001:db8::1]:4433`, `relay:https://euw1-1.relay.n0.iroh-canary.iroh.link./`.

**Client-side parse, `transport.ParseAddrs`** (L247–261). For each string it tries
`netaddr.ParseTransportAddr(s)` (`go-iroh/netaddr/endpointaddr.go:258-281`):
- It splits at the first `:` into `kind` and `value`.
  - `relay`: `url.Parse(value)` then normalise: lowercase host, and an empty path becomes `/` for http/https/ws/wss/ftp/file (`relayurl.go:25-31,94-100`).
  - `ip`: `netip.ParseAddrPort(value)`.
  - `custom`: `ParseCustomAddr(value)`, which is `<hex u64 id>_<hex data>` (L203–218).
  - Any other kind is an error.
- With no `:` it is `ParseCustomAddr(s)`.
- On any error it falls back to `netip.ParseAddrPort(s)` as a bare `ip:port`. This is how `192.168.1.10:4433` and
  `[::1]:4433` are accepted.
- Anything still unparseable is **silently skipped**.

**`Dial(ctx, id, addrs, alpn)`** (L265–302)
1. `irohkey.NewEndpointID(id)` validates the ed25519 point. The error is `"data is not a valid public key"` (`go-iroh/key/key.go:34,61-81`).
2. It splits candidates into relay and direct (IP and custom).
3. With direct candidates, it races them under `DirectTimeout`, default 2 s. Success returns. On failure with no relays it goes to `discoverDial`.
4. With relays, it races `relays ++ direct` under the parent ctx. On failure it goes to `discoverDial`.
5. With no candidates it goes to `discoverDial(…, nil)`.

`discoverDial` (L307–319) returns `prev` when there is no lookup service and `prev != nil`. Otherwise it runs
`Connect(ctx, NewEndpointAddr(eid), alpn)`. On error that is `errors.Join(prev, fmt.Errorf("discovery: %w", err))`,
or just `err` when `prev` is nil. `raceConnect` (L322–372) needs `awaitHandshake` before a candidate wins. Its errors
are `"transport: no candidate addresses for %s"` (with `id.Short()`) and `"dial %s: %w"` per candidate, joined with
`errors.Join`.

### 2.5 CLI surfaces that touch the view

| surface | behaviour |
|---|---|
| `dstore node remove\|drain\|weight\|zone\|repair ID …` | `idArg` (`main.go:464-470`) runs `view.ParseNodeID(c.Args().First())`. A missing arg gives `""` and the error `view: bad node id ""`. `weight ID GiB` parses the weight with `strconv.ParseUint(…,10,32)` and fails with `weight ID GiB`. The request is `node.AdminRequest{Op: "node-remove"…, Node: id[:]}` |
| `dstore voter add\|remove ID` | `view.ParseNodeID(c.Args().First())` (`main.go:586`) |
| `dstore cluster status` | `printStatus` (§3.4) |
| `dstore cluster ticket [--ids]` | with `--store` and no `--ticket`: `localTicket(dir)`, which is node-side (§8). Otherwise it dials and sends admin `cluster-ticket`. The server (`node/admin.go:85-96`) builds `Ticket{ClusterID, Incarnation, Members: [this node with ep.Addrs()] ++ first 3 of v.Nodes}`, which can list the answering node twice. The CLI re-parses it with `ticket.Parse` and prints `t.Encode()` or `t.IDs()` (which deduplicates) |
| `dstore cluster init`, `node join` | node-side; print `node id: <IDString>` |
| `dstore clone`, `init` (working copy) | `cfg.Ticket = worktree.TicketFromView(cl.View()).Encode()`. Later commands call `RefreshTicket`, which saves the config only when the derived string differs |
| transfer TUI | node column `%-10s` of `view.ShortID(n.ID)` (`tui.go:270`) |
| logs | slog attribute `"node"` is always `view.ShortID(id)` |

---

## 3. Byte formats and text formats

### 3.1 View CBOR (fxamacker v2.9.3, `cbor.CanonicalEncOptions()`)

**Encoding rules that apply** (`fxamacker encode.go:631-639`, other options at their defaults)

- The struct becomes a CBOR map whose count is the number of emitted fields.
  - Keys are the `keyasint` integers, and all of them are < 24, so each is a single byte.
  - Fields are emitted in ascending key order. Canonical order sorts by the encoded key bytes, which for 0–23 is numeric order (`cache.go:214-223,348-357`).
- `omitempty` (`encode.go:2120-2200`, `OmitEmptyCBORValue` default) omits: `false`; integer 0; empty string; a slice of length 0, whether nil or empty; a nil pointer.
  - A **non-nil pointer to a zero struct is emitted**. `&Pending{}` becomes `a3 00 f6 01 00 02 00`, and `&ACL{}` becomes `a0`.
- **A non-omitempty nil slice encodes as `f6`** (`NilContainerAsNull`, `encode.go:395-397,1285-1345`).
  - A non-omitempty empty non-nil slice encodes as `80`, and empty non-nil `[]byte` as `40`.
  - A nil `[]byte` element inside `[][]byte` encodes as `f6`.
- `[]byte` is major type 2. `string` is major type 3, and encoding does **no** UTF-8 validation. Signed integers are major type 0 or 1. `uint8` and the other unsigned types are major type 0. Heads are always shortest-form.
- No tags, no floats, no indefinite lengths.

**Field tables** (`Omit?` means `omitempty`)

`View` (map):

| key | field | Go type | Omit? | notes |
|---:|---|---|---|---|
| 0 | ClusterID | `[]byte` | no | 16 random bytes at init; nil → `f6` |
| 1 | Incarnation | `uint64` | no | |
| 2 | Epoch | `uint64` | no | |
| 3 | Version | `uint64` | no | |
| 4 | PlacementEpoch | `uint64` | no | |
| 5 | Replicas | `uint8` | no | |
| 6 | MinReplicas | `uint8` | no | |
| 7 | Voters | `[]Voter` | no | nil → `f6` |
| 8 | VoterSync | `int` | no | 0 done, 1 pending |
| 9 | VoterSyncCursor | `[]byte` | yes | |
| 10 | Nodes | `[]Node` | no | nil → `f6` |
| 11 | Pending | `*Pending` | yes | |
| 12 | Former | `[]Former` | yes | |
| 13 | Fenced | `[][]byte` | yes | |
| 14 | RecoveredInc | `uint64` | yes | |
| 15 | RecoveredEpoch | `uint64` | yes | |
| 16 | RebalancePause | `bool` | yes | |
| 17 | RateCap | `uint64` | yes | |
| 18 | VoterSyncTarget | `[]byte` | yes | |
| 19 | VoterSyncAdd | `bool` | yes | |
| 20 | DeferredVoters | `[][]byte` | yes | |
| 21 | ACL | `*ACL` | yes | |
| 22 | Ramps | `[]Ramp` | yes | |
| 23 | RemoveVoters | `[][]byte` | yes | |

The other structs:

| struct | key | field | type | Omit? |
|---|---:|---|---|---|
| Node | 0 | ID | `[]byte` | no |
| Node | 1 | Weight | `uint32` | no |
| Node | 2 | Addrs | `[]string` | yes |
| Node | 3 | Data | `[]DataEndpoint` | yes |
| Node | 4 | Token | `[]byte` | yes |
| Node | 5 | Zone | `string` | yes |
| Node | 6 | Incarnation | `uint64` | yes |
| Node | 7 | Writable | `bool` | **no** (`f4`/`f5` always) |
| DataEndpoint | 0 | ID | `[]byte` | no |
| DataEndpoint | 1 | Addrs | `[]string` | yes |
| Voter | 0 | ID | `[]byte` | no |
| Voter | 1 | Since | `uint64` | no |
| Former | 0 | ID | `[]byte` | no |
| Former | 1 | Until | `int64` | no |
| Pending | 0 | Nodes | `[]Node` | no |
| Pending | 1 | Replicas | `uint8` | no |
| Pending | 2 | ID | `uint64` | no |
| Pending | 3 | ParticipantsAck | `[][]byte` | yes |
| Pending | 4 | Participants | `[][]byte` | yes |
| Pending | 5 | Frozen | `bool` | yes |
| Pending | 6 | Round | `uint32` | yes |
| Pending | 7 | PrimaryDone | `[][]byte` | yes |
| Pending | 8 | Done | `[][]byte` | yes |
| Pending | 9 | FrozenAt | `int64` | yes |
| Pending | 10 | Reason | `string` | yes |
| Pending | 11 | Ramp | `*Ramp` | yes |
| Pending | 12 | Since | `int64` | yes |
| Ramp | 0 | Node | `[]byte` | no |
| Ramp | 1 | Target | `uint32` | no |
| Ramp | 2 | Step | `int` | no |
| ACL | 0 | Allowed | `[][]byte` | yes |
| ACL | 1 | Admins | `[][]byte` | yes |

**Golden encodings** (all verified to decode and re-encode byte-identically in Go)

- `View{}` (21 bytes): `aa00f601000200030004000500060007f608000af6`
- `View{ClusterID: []byte{}, Voters: []Voter{}, Nodes: []Node{}, Former: []Former{}, Fenced: [][]byte{}, VoterSyncCursor: []byte{}, Ramps: []Ramp{}}`:
  `aa0040010002000300040005000600078008000a80`
- `View{Pending: &Pending{}, ACL: &ACL{}}`: `ac00f601000200030004000500060007f608000af60ba300f60100020015a0`
- `View{Pending: &Pending{Ramp: &Ramp{}, Nodes: []Node{}}}`: `ab00f601000200030004000500060007f608000af60ba40080010002000ba300f601000200`
- `View{Nodes: []Node{{ID: nil, Weight: 1}, {ID: []byte{}, Weight: 2}}}`: `aa00f601000200030004000500060007f608000a82a300f6010107f4a30040010207f4`
- negatives, `View{VoterSync: -1, Former: [{ID: id0, Until: -1}], Pending: &Pending{FrozenAt: -5, Since: -1<<63, Ramp: &Ramp{Step: -2}}, Ramps: [{Step: -300}]}`:
  `ad00f601000200030004000500060007f608200af60ba600f60100020009240ba300f6010002210c3b7fffffffffffffff0c81a20058209203ebd1544cb52faeba0d15542ac73dfc9c8a30869bc5d626e5eac8eb1e4d0b01201681a300f601000239012b`
- maxes, `View{Incarnation: MaxUint64, Epoch: 1<<32, Version: 1<<32-1, PlacementEpoch: 65536, Replicas: 255, MinReplicas: 24, VoterSync: 1<<40, RateCap: MaxUint64, Nodes: [{ID: id0, Weight: MaxUint32, Incarnation: MaxUint64, Writable: true}], Pending: &Pending{Round: MaxUint32, ID: 23, Replicas: 23}}`:
  `ac00f6011bffffffffffffffff021b0000000100000000031affffffff041a000100000518ff06181807f6081b00000100000000000a81a40058209203ebd1544cb52faeba0d15542ac73dfc9c8a30869bc5d626e5eac8eb1e4d0b011affffffff061bffffffffffffffff07f50ba400f601170217061affffffff111bffffffffffffffff`
- utf8, `View{Nodes: [{ID: id0, Weight: 1, Zone: "zürich-🏔"}], Pending: &Pending{Reason: "ramp ✓"}}`:
  `ab00f601000200030004000500060007f608000a81a40058209203ebd1544cb52faeba0d15542ac73dfc9c8a30869bc5d626e5eac8eb1e4d0b0101056c7ac3bc726963682df09f8f9407f40ba400f6010002000a6872616d7020e29c93`
- `View{Nodes: [{ID: id0, Data: [{ID: id1}, {}]}]}` (Writable false, a data endpoint with no addrs, and a zero data endpoint):
  `aa00f601000200030004000500060007f608000a81a40058209203ebd1544cb52faeba0d15542ac73dfc9c8a30869bc5d626e5eac8eb1e4d0b01000382a1005820142e00b2a3d225e57948ba6e362f431a41737b020c950e20058eca34a1d31eaaa100f607f4`
- The init-shaped view (217 bytes), annotated:

```
aa                                    map(10)
00 50 48c1cc4d8bba1e3cafbbf4d4829d7ad0  0: cluster id (16 bytes)
01 01  02 01  03 01  04 01            incarnation 1, epoch 1, version 1, placement_epoch 1
05 03  06 02                          replicas 3, min_replicas 2
07 81 a2 00 5820 9203ebd1544cb52faeba0d15542ac73dfc9c8a30869bc5d626e5eac8eb1e4d0b 01 01
                                      voters [{id, since 1}]
08 00                                 voter_sync 0
0a 81 a5 00 5820 9203eb…4d0b 01 1903a3 02 83
      75 "ip:192.168.1.10:51820"
      72 "ip:[fe80::1]:51820"
      7835 "relay:https://euw1-1.relay.n0.iroh-canary.iroh.link./"
      06 01 07 f5                     nodes [{id, weight 931, addrs[3], incarnation 1, writable true}]
```

Full hex:
`aa005048c1cc4d8bba1e3cafbbf4d4829d7ad00101020103010401050306020781a20058209203ebd1544cb52faeba0d15542ac73dfc9c8a30869bc5d626e5eac8eb1e4d0b010108000a81a50058209203ebd1544cb52faeba0d15542ac73dfc9c8a30869bc5d626e5eac8eb1e4d0b011903a302837569703a3139322e3136382e312e31303a35313832307269703a5b666538303a3a315d3a3531383230783572656c61793a68747470733a2f2f657577312d312e72656c61792e6e302e69726f682d63616e6172792e69726f682e6c696e6b2e2f060107f5`

The generator also emits `full` (1204 bytes, every field populated) and `long_addrs` (text heads of 23, 24, 255 and 256 bytes).

### 3.2 View decoding (`cbor.DecOptions{}` defaults), verified outcomes

The decoder runs in two passes.

1. **Well-formedness pass** over the whole input (`decode.go:1291-1303`, `valid.go`). Its errors win over any type error:
   - empty input → `EOF`
   - truncated → `unexpected EOF`
   - trailing bytes → `cbor: <n> bytes of extraneous data starting at index <i>`
   - additional info 28–30 → `cbor: invalid additional information <ai> for type <t>`
   - wrong chunk type in an indefinite string → `cbor: wrong element type <nt> for indefinite-length <t>`
   - depth > 32 over maps, arrays and tags → `cbor: exceeded max nested level 32`
   - more than 131072 elements → `cbor: exceeded max number of elements 131072 for CBOR array`
   - more than 131072 pairs → `cbor: exceeded max number of key-value pairs 131072 for CBOR map`
   - Indefinite lengths, non-shortest heads and tags are all **accepted**.
2. **Decode pass** (`parseMapToStruct`, `decode.go:2714-2898`):
   - Map keys that are integers (major type 0 or 1) are matched against `keyasint`; unknown integer keys, including negative ones, are skipped.
   - Text-string keys never match these fields and are skipped silently.
   - Byte-string keys and simple-value keys set an error, but decoding continues.
   - A **duplicate key keeps the first value** (`checkDupField` → `mapActionSkipValueAndContinue`, `decode_map_utils.go:21-30`).
   - `null` (`f6`) or `undefined` (`f7`) into any field sets the zero value: nil slice or pointer, or 0/false/"" (`fillNil`, `decode.go:3061-3068`).
   - Type mismatches and overflows record the **first** error and continue. `Unmarshal` returns that first error.
   - Text strings are UTF-8-validated → `cbor: invalid UTF-8 string`.
   - `view.Decode` prefixes every error with `view: decode: ` and returns no view.

The error field path always names the **top-level** `View` key, even for nested fields, followed by the leaf Go type.

| case | input hex | Go result |
|---|---|---|
| empty input | `` | `view: decode: EOF` |
| trailing zero byte after the 217-byte init view | `…f500` | `view: decode: cbor: 1 bytes of extraneous data starting at index 217` |
| truncated init view | `…07` | `view: decode: unexpected EOF` |
| top-level array | `80` | `view: decode: cbor: cannot unmarshal array into Go value of type view.View (cannot decode CBOR array to struct without toarray option)` |
| **top-level null** | `f6` | **ok**, the zero view → `aa00f601000200030004000500060007f608000af6` |
| empty map | `a0` | ok → zero view |
| reordered keys | `a30a800205005048c1cc4d8bba1e3cafbbf4d4829d7ad0` | ok → `aa005048c1cc4d8bba1e3cafbbf4d4829d7ad001000205030004000500060007f608000a80` |
| unknown int key 99 | `a2020518636178` | ok, ignored |
| unknown negative key | `a220010205` | ok, ignored |
| duplicate key 2 (1 then 7) | `a202010207` | ok, **epoch 1** (first wins) |
| text key `"2"` | `a1613209` | ok, ignored (epoch stays 0) |
| text key `"Epoch"` | `a16545706f636809` | ok, ignored |
| byte-string key | `a24102090104` | `view: decode: cbor: cannot unmarshal byte string into Go value of type string (map key is of type byte string and cannot be used to match struct field name)` |
| simple-value key | `a1f401` | `view: decode: cbor: cannot unmarshal primitives into Go value of type string (map key is of type primitives and cannot be used to match struct field name)` |
| null nodes and voters | `a20af607f6` | ok, nil |
| null replicas | `a105f6` | ok, 0 |
| undefined epoch | `a102f7` | ok, 0 |
| null pending | `a10bf6` | ok, nil |
| empty pending map | `a10ba0` | ok, `Pending{}` non-nil → `…0ba300f601000200` |
| replicas 256 | `a105190100` | `view: decode: cbor: cannot unmarshal positive integer into Go struct field view.View.5 of type uint8 (256 overflows uint8)` |
| node weight 2^32 | `a10a81a2005820…0b011b0000000100000000` | `view: decode: cbor: cannot unmarshal positive integer into Go struct field view.View.10 of type uint32 (4294967296 overflows uint32)` |
| negative incarnation | `a10120` | `view: decode: cbor: cannot unmarshal negative integer into Go struct field view.View.1 of type uint64` |
| negative voter_sync −5 | `a10824` | ok → `…0824…` |
| half-float epoch | `a102f93c00` | `view: decode: cbor: cannot unmarshal primitives into Go struct field view.View.2 of type uint64` |
| `writable` as integer | `a10a81a2005820…0b0701` | `view: decode: cbor: cannot unmarshal positive integer into Go struct field view.View.10 of type bool` |
| invalid UTF-8 zone | `a10a81a2005820…0b0562c328` | `view: decode: cbor: invalid UTF-8 string` |
| byte-string zone | `a10a81a2005820…0b05417a` | `view: decode: cbor: cannot unmarshal byte string into Go struct field view.View.10 of type string` |
| text cluster id | `a1006461626364` | `view: decode: cbor: cannot unmarshal UTF-8 text string into Go struct field view.View.0 of type []uint8` |
| node id of 2 bytes | `a10a81a2004201020105` | ok (no length validation) |
| non-shortest heads | `b9000118021a00000005` | ok, epoch 5 |
| indefinite map and array | `bf02050a9fa2005820…0b0103ffff` | ok → canonical |
| indefinite byte string | `a1005f4201024103ff` | ok, cluster id `010203` |
| self-describe tag 55799 around the view | `d9d9f7aa…` | ok |
| tag 1 on epoch | `a102c105` | ok, epoch 5 |
| type error, then overflow | `a20161780519012c` | `…view.View.1 of type uint64` (first error wins) |
| overflow, then type error | `a20519012c016178` | `…view.View.5 of type uint8 (300 overflows uint8)` |
| nodes as map | `a10aa0` | `view: decode: cbor: cannot unmarshal map into Go struct field view.View.10 of type []view.Node` |
| pending as array | `a10b80` | `view: decode: cbor: cannot unmarshal array into Go struct field view.View.11 of type view.Pending (cannot decode CBOR array to struct without toarray option)` |
| ramp step −10 | `a11681a10229` | ok |
| ramp step 2^63 | `a11681a1021b8000000000000000` | `view: decode: cbor: cannot unmarshal positive integer into Go struct field view.View.22 of type int (9223372036854775808 overflows int)` |
| former until 2^63 | `a10c81a1011b8000000000000000` | `…view.View.12 of type int64 (9223372036854775808 overflows int64)` |
| reserved additional info | `bc` | `view: decode: cbor: invalid additional information 28 for type map` |
| 33 nested arrays under an unknown key | `a11863` + `81`×33 + `00` | `view: decode: cbor: exceeded max nested level 32` |

CBOR type names in these messages are `positive integer`, `negative integer`, `byte string`, `UTF-8 text string`,
`array`, `map`, `tag` and `primitives`. Go type names are the reflect strings: `uint8`, `uint32`, `uint64`, `int`,
`int64`, `bool`, `string`, `[]uint8`, `[]view.Node`, `view.Pending`, and so on.

These texts reach users through `client: no bootstrap node answered: view: decode: …`.

### 3.3 `TicketFromView` (`worktree/flow.go:305-315`)

```go
tk := ticket.Ticket{ClusterID: v.ClusterID, Incarnation: v.Incarnation}
for _, nd := range v.Nodes {                 // view order, NOT sorted, pending.nodes ignored
    tk.Members = append(tk.Members, ticket.Member{ID: nd.ID, Addrs: nd.Addrs})
    if len(tk.Members) >= 4 { break }
}
```

- `Ticket` is `{0: ClusterID []byte (not omitempty), 1: Incarnation uint64, 2: Members []Member (not omitempty)}`.
- `Member` is `{0: ID []byte, 1: Addrs []string omitempty}`.
- `Encode()` is `"dstore1" + lowercase(base32 std alphabet, no padding, of canonical CBOR)` (`ticket/ticket.go:35-38`).
- A view with no nodes leaves `Members` nil, which encodes as `f6`. The resulting ticket does **not** parse back
  (`ticket: no members`).
- `RefreshTicket` (L320–331) returns nil when `cl.View() == nil`. It saves the config only when the encoded string
  differs.

| case | ticket | CBOR |
|---|---|---|
| no nodes, cid `48c1…d0`, inc 4 | `dstore1umafasgbzrgyxoq6hsx3x5guqkoxvuabaqbpm` | `a3005048c1cc4d8bba1e3cafbbf4d4829d7ad0010402f6` |
| nil cluster id, one node without addrs | `dstore1umapmaiaaka2cacyec5jwwg5awsecogl7wszqsq4nras6vszgawkoutrcgmfaapraliei` | `a300f601000281a1005820ba9b58dd05a44138cbfda5984a1c6c412f5659302ca7527111985001f102d044` |
| init view | `dstore1umafasgbzrgyxoq6hsx3x5guqkoxvuabaebidiqalaqjea7l2fkeznjpv25a2fkufldt37e4riying6f2ytol2wi5mpe2cybqn2ws4b2ge4telrrgy4c4mjogeydunjrhazda4tjoa5fwztfhayduorrlu5dkmjygiyhqnlsmvwgc6j2nb2hi4dthixs6zlvo4ys2mjoojswyylzfzxdaltjojxwqlldmfxgc4tzfzuxe33ifzwgs3tlfyxq` | `a3005048c1cc4d8bba1e3cafbbf4d4829d7ad001010281a20058209203eb…0b01837569703a…2f` |
| 6 nodes: only the first 4 in view order | `dstore1umafasgbzrgyxoq6hsx3x5guqkoxvuabaebijiialaqbilqawkr5ejpfpfelu3rwf5bruqltpmbazfioeacy5sruuhjr5kvcabmcbeqd5pivitfvf6xludivkqvmopp4tsfdbbu3yxlcnzpkzdvr4tilagaxi2lqhiytsmroge3dqlrrfyytaorugqzthiialaqlvg2y3uc2iqjyzp62lgckdrwecl2wleyczj2soeizquab6ebnarfbabmcbwdd3qpjvxz5su5kwkjanhmbopywrnt7p6rh2blqd235ct6ovl2x` | `a3…0284…` |

The generator also emits `pending_only_nodes_ignored` (the same string as the 6-node case) and `unsorted_order_kept`.

### 3.4 `cluster status` view lines (`cmd/dstore/client.go:134-150`), verbatim

```go
fmt.Printf("cluster %x incarnation %d epoch %d version %d\n", v.ClusterID[:4], v.Incarnation, v.Epoch, v.Version)
fmt.Printf("replicas %d min_replicas %d nodes %d voters %d", v.Replicas, v.MinReplicas, len(v.Nodes), len(v.Voters))
if len(v.Voters) < 3 { fmt.Print(" (no catalog fault tolerance)") }
fmt.Println()
if v.Pending != nil {
    fmt.Printf("transition %d (%s): frozen=%v acked=%d participants=%d done=%d\n", v.Pending.ID, v.Pending.Reason, v.Pending.Frozen, len(v.Pending.ParticipantsAck), len(v.Pending.Participants), len(v.Pending.Done))
}
if v.VoterSync == view.VoterSyncPending {
    fmt.Printf("voter change in progress (target %s)\n", node.ShortID(v.VoterSyncTarget))   // "?" unless 32 bytes
}
for _, nd := range v.Nodes {   // view order; pending-only nodes are not listed
    id := nd.NID()
    line := fmt.Sprintf("  %s weight %d zone %q voter=%v writable=%v", view.IDString(id), nd.Weight, nd.Zone, v.IsVoter(id), nd.Writable)
    // then Status per node, 5 s timeout:
    //   error:        fmt.Println(line, "— unreachable:", err)   -> line + " — unreachable: " + err
    //   decode error: fmt.Println(line, "— bad status")
    //   ok: line, then the Status lines (L164-190, owned by the status/CLI spec)
}
```

- `%x` of `v.ClusterID[:4]` prints 8 lowercase hex characters. **A cluster id shorter than 4 bytes makes Go panic**
  (slice bounds). See §8.
- `%q` of the zone uses Go quoting (§3.5). An empty zone prints `zone ""`.
- `%v` of a bool prints `true` or `false`.
- The node-side `Status.Transition` string comes from `transitionText` (`node/maintenance.go:539-557`). The client
  prints it only when it is neither `""` nor `"idle"`. Its forms are:
  - `idle`
  - `idle (%d ramp(s) pending)`
  - `id %d (%s): adopting, %d/%d acked`
  - `id %d round %d (%s): %d/%d done, waiting for %v`, where `%v` of a `[]string` of ShortIDs prints `[a b]` or `[]`

### 3.5 Go `%q` of a string (needed for `view: bad node id %q` and `zone %q`)

`%q` is `strconv.Quote`. Starting with `"`, it decodes the string rune by rune:
- An invalid UTF-8 byte becomes `\xHH`.
- `"` becomes `\"` and `\` becomes `\\`.
- A rune with `unicode.IsPrint(r)` (letters, marks, numbers, punctuation, symbols, and U+0020) is written literally.
- `\a \b \f \n \r \t \v` use those escapes.
- Other runes below U+0020 and U+007F become `\xHH`.
- Other runes below U+10000 become `\uHHHH`; the rest become `\UHHHHHHHH`.
- Hex digits are lowercase. It ends with `"`.

The Rust port needs Go's `strconv/isprint.go` tables for Go 1.26.5.

`ParseNodeID` vectors (id = `d70d3259e4e1cb631c663cf4d73c4c04022ab1ba804098e6cb293e6770eb3a95`):

| input | result |
|---|---|
| 64 lowercase hex | ok |
| 64 uppercase hex | ok, same id |
| mixed case | ok |
| 63 characters | `view: bad node id "d70d…eb3a9"` |
| 65 characters | bad node id |
| 66 characters (33 bytes) | bad node id |
| `0x` + 62 hex | `view: bad node id "0xd70d…3a"` |
| leading space | `view: bad node id " 70d3…"` |
| trailing `\n` | `view: bad node id "d70d…3a95\n"` (escape characters) |
| base32 `24gtewpe4hfwghdght2nopcmaqbcvmn2qbajrzwlfe7go4hlhkkq` | `view: bad node id "24gtewpe4hfwghdght2nopcmaqbcvmn2qbajrzwlfe7go4hlhkkq"` |
| `""` | `view: bad node id ""` |
| `zz`×32 | bad node id |
| `ab<TAB>cd` | `view: bad node id "ab\tcd"` |
| `héllo 😀` | `view: bad node id "héllo 😀"` (kept literally) |
| bytes `ff fe` | `view: bad node id "\xff\xfe"` |
| `quote"back\slash` | `view: bad node id "quote\"back\\slash"` |
| bytes `00 7f` | `view: bad node id "\x00\x7f"` |
| U+200B | `view: bad node id "​"` |

`node.ShortID` vectors: 32 bytes → `d70d3259`; 4 bytes, empty, nil or 33 bytes → `?`.

### 3.6 Address strings

`ip:<addr:port>` (IPv6 in brackets), `relay:<url>`, `custom:<hex id>_<hex data>`. Rust
`iroh_base::TransportAddr`'s `Display` produces the same prefixes (`iroh-base-1.0.3/src/endpoint_addr.rs:81-89`), and
`CustomAddr` displays as `{:x}_{hexlower}` (L193–197).

---

## 4. Rust design

### 4.1 Modules

| Rust path | contents |
|---|---|
| `src/placement.rs` | the §2.1 math, `Set`, `Table` (pure, no async) |
| `src/view/mod.rs` | `NodeId`, types, lookups, `Placement`, helpers |
| `src/view/codec.rs` | encode and decode for the view types, on the shared fxamacker-compatible codec (§7) |
| `src/view/addrs.rs` | `parse_addrs` (ParseAddrs) and `endpoint_addr(id, addrs)` |
| `src/gofmt.rs` (shared with the CLI area) | `go_quote(&[u8]) -> String` |
| `src/worktree/ticket.rs` (or inside `worktree`) | `ticket_from_view` |
| `src/status.rs` (or `view`) | `node_short_id` |
| `tests/placement_golden.rs`, `tests/view_golden.rs` | read `tests/golden/view_placement.json` |
| `tools/vectorgen/` | the Go generator, pinned to `github.com/amber-store/dstore v0.1.9` (Appendix A) |

### 4.2 `placement.rs`

```rust
pub const SLOT_BITS: u32 = 20;
pub const SLOTS: u32 = 1 << SLOT_BITS;
const SALT_DOMAIN: &[u8] = b"amber-dstore/placement/1";

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct NodeId(pub [u8; 32]);          // plain bytes, never validated as a curve point
impl NodeId {
    pub fn from_slice_lossy(b: &[u8]) -> NodeId;   // Go copy(): min(len,32) bytes, zero-padded
    pub fn to_hex(&self) -> String;                // IDString
    pub fn short(&self) -> String;                 // ShortID: hex of [0..4]
}
// Derived Ord on [u8;32] is bytewise, the same as compareID.

pub struct Member { pub id: NodeId, pub weight: u32, pub zone: Vec<u8> } // empty => the id bytes

pub fn slot(key: &[u8; 32]) -> u32;       // (u64::from_be_bytes(key[24..32]) >> 44) as u32
pub fn salt(id: &NodeId) -> u64;          // blake3::Hasher::new().update(SALT_DOMAIN).update(&id.0).finalize(), BE of [0..8]
pub fn fmix64(x: u64) -> u64;             // wrapping_mul
pub fn log2fix(x: u64) -> u64;            // assert!(x != 0, "placement: Log2Fix(0)")
pub fn l(slot: u32, salt: u64) -> u64;

/// Ordering::Less means a ranks above b.
fn cmp_rank(a: &Member, la: u64, b: &Member, lb: u64) -> std::cmp::Ordering {
    let pa = (a.weight as u128) * (lb as u128);
    let pb = (b.weight as u128) * (la as u128);
    pb.cmp(&pa).then_with(|| a.id.cmp(&b.id))
}

pub struct Set { members: Vec<Member>, salts: Vec<u64> }
impl Set {
    pub fn new(members: Vec<Member>) -> Set;
    pub fn members(&self) -> &[Member];
    pub fn rank(&self, slot: u32) -> Vec<u32>;          // weighted: slice::sort_by (stable) with cmp_rank; zero: sort_by_key(id)
    pub fn owners(&self, slot: u32, r: usize) -> Vec<u32>;
}

pub struct Table {
    set: Set,
    r: usize,
    owners: std::sync::RwLock<HashMap<u32, Arc<[u32]>>>,  // lazy, not 2^20 preallocated slots
    rank:   std::sync::RwLock<HashMap<u32, Arc<[u32]>>>,
}
impl Table {
    pub fn new(set: Set, r: usize) -> Table;
    pub fn owners(&self, slot: u32) -> Arc<[u32]>;
    pub fn rank(&self, slot: u32) -> Arc<[u32]>;
    pub fn owner_ids(&self, key: &[u8; 32]) -> Vec<NodeId>;
    pub fn rank_ids(&self, key: &[u8; 32]) -> Vec<NodeId>;
    pub fn is_owner(&self, key: &[u8; 32], id: &NodeId) -> bool;
}
```

Implementation notes:
- Use `u128` products. Weights are below 2^32 and `L ≤ 2^38`, so products stay below 2^71.
- `Set.owners` keeps a `HashSet<&[u8]>` of zone keys. `zone.is_empty()` means the 32 id bytes.
- `rank` sorts the weighted indexes with a **stable** sort (`sort_by`). Go's `sort.SliceStable` with a strict weak
  ordering yields the same permutation.
- A per-slot cache avoids Go's eager 48 MiB allocation with no observable difference.
- The work is CPU-only. Callers on tokio may call it directly; a full ranking over 50 nodes is about 3 µs.

### 4.3 `view/mod.rs` types

Nil and empty must stay distinct wherever the Go field is **not** `omitempty`, because they encode differently
(`f6` versus `80`/`40`). Use `Option<Vec<_>>` there. `Vec<_>` is enough for `omitempty` slices, since nil and empty
both encode as absent.

```rust
pub type GoBytes = Option<Vec<u8>>;        // non-omitempty []byte and [][]byte elements: None = nil

pub struct Voter        { pub id: GoBytes, pub since: u64 }
pub struct Former       { pub id: GoBytes, pub until: i64 }
pub struct DataEndpoint { pub id: GoBytes, pub addrs: Vec<String> }
pub struct Node {
    pub id: GoBytes, pub weight: u32, pub addrs: Vec<String>, pub data: Vec<DataEndpoint>,
    pub token: Vec<u8>, pub zone: String, pub incarnation: u64, pub writable: bool,
}
pub struct Ramp    { pub node: GoBytes, pub target: u32, pub step: i64 }
pub struct Acl     { pub allowed: Vec<GoBytes>, pub admins: Vec<GoBytes> }
pub struct Pending {
    pub nodes: Option<Vec<Node>>, pub replicas: u8, pub id: u64,
    pub participants_ack: Vec<GoBytes>, pub participants: Vec<GoBytes>, pub frozen: bool, pub round: u32,
    pub primary_done: Vec<GoBytes>, pub done: Vec<GoBytes>, pub frozen_at: i64, pub reason: String,
    pub ramp: Option<Ramp>, pub since: i64,
}
pub struct View {
    pub cluster_id: GoBytes, pub incarnation: u64, pub epoch: u64, pub version: u64, pub placement_epoch: u64,
    pub replicas: u8, pub min_replicas: u8, pub voters: Option<Vec<Voter>>, pub voter_sync: i64,
    pub voter_sync_cursor: Vec<u8>, pub nodes: Option<Vec<Node>>, pub pending: Option<Box<Pending>>,
    pub former: Vec<Former>, pub fenced: Vec<GoBytes>, pub recovered_inc: u64, pub recovered_epoch: u64,
    pub rebalance_pause: bool, pub rate_cap: u64, pub voter_sync_target: Vec<u8>, pub voter_sync_add: bool,
    pub deferred_voters: Vec<GoBytes>, pub acl: Option<Acl>, pub ramps: Vec<Ramp>, pub remove_voters: Vec<GoBytes>,
}
pub const VOTER_SYNC_DONE: i64 = 0;
pub const VOTER_SYNC_PENDING: i64 = 1;
```

- A `Some(vec![])` element inside an `omitempty` `[][]byte` list is a present-but-empty id and encodes as `40`. A
  `None` element encodes as `f6`.
- Decode maps absent to `None` or the default, `f6`/`f7` to `None` or the default, and `80`/`40` to `Some(empty)`.
- `Ramp.step` and `voter_sync` are `i64`. On decode, reject values outside `i64` using the Go overflow message with
  type name `int` or `int64` as appropriate (§3.2).

```rust
impl View {
    pub fn encode(&self) -> Vec<u8>;                           // infallible
    pub fn decode(b: &[u8]) -> Result<View, ViewError>;        // Display: "view: decode: {cbor error}"
    pub fn compare(&self, inc: u64, epoch: u64) -> std::cmp::Ordering;  // Go −1/0/+1
    pub fn nodes(&self) -> &[Node];                            // None -> &[]
    pub fn node(&self, id: &NodeId) -> Option<&Node>;          // nodes, then pending.nodes; exact 32-byte match
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
    pub fn zone_or_id(&self) -> &[u8];
}

pub fn parse_node_id(s: &str) -> Result<NodeId, ViewError>;    // Display: "view: bad node id {go_quote(s)}"
pub fn id_string(id: &NodeId) -> String;
pub fn short_id(id: &NodeId) -> String;
pub fn node_short_id(b: &[u8]) -> String;                      // "?" unless len == 32
pub fn contains(ids: &[GoBytes], id: &NodeId) -> bool;
pub fn add_id(ids: &mut Vec<GoBytes>, id: &NodeId);
pub fn ids_of(raw: &[GoBytes]) -> Vec<NodeId>;
pub fn sort_nodes(nodes: &mut [Node]);                         // by id bytes (None == empty)
pub fn default_min_replicas(r: u8) -> u8;
pub fn validate_change(cur: &[Node], target: &[Node], replicas: i64, force: bool) -> Result<(), ViewError>;
```

`parse_node_id` takes `&str`, but Go strings can hold invalid UTF-8. The CLI receives OS arguments; on Unix take
`OsStr` bytes and pass `&[u8]` so `\xff` escapes stay reproducible. Signature variant:
`parse_node_id_bytes(&[u8])`.

```rust
pub struct Placement { view: Arc<View>, cur: Table, pending: Option<Table> }
impl Placement {
    pub fn new(view: Arc<View>) -> Placement;
    pub fn view(&self) -> &Arc<View>;
    pub fn owners(&self, key: &[u8; 32]) -> Vec<NodeId>;
    pub fn pending_owners(&self, key: &[u8; 32]) -> Option<Vec<NodeId>>;   // None without pending
    pub fn write_set(&self, key: &[u8; 32]) -> Vec<NodeId>;
    pub fn read_order(&self, key: &[u8; 32]) -> Vec<NodeId>;               // no dedup within the current table
    pub fn is_owner(&self, key: &[u8; 32], id: &NodeId) -> bool;
    pub fn is_pending_owner(&self, key: &[u8; 32], id: &NodeId) -> bool;
    pub fn in_write_set(&self, key: &[u8; 32], id: &NodeId) -> bool;
}
```

`members()` uses `nid()` and `zone_or_id()` in node order, keeping duplicates.

The client-side view state is owned by the client area. It must implement §2.3 exactly:

```rust
struct ViewState {
    view: Option<Arc<View>>,
    placement: Option<Arc<Placement>>,
    boot_addrs: HashMap<NodeId, Vec<String>>,
    backoff: HashMap<NodeId, Instant>,
    failures: HashMap<NodeId, u32>,
    unreach: HashSet<NodeId>,
}
impl Cluster {
    fn adopt(&self, v: View);                       // (inc, epoch, version) lexicographic
    fn adopt_reply(&self, m: &wire::Msg) -> Result<(), ClientError>;
    fn addrs_of(&self, id: &NodeId) -> Vec<String>;
    pub fn view(&self) -> Option<Arc<View>>;
    pub fn placement(&self) -> Option<Arc<Placement>>;
    pub fn primary(&self, key: &[u8; 32]) -> Option<NodeId>;
    pub fn read_order(&self, key: &[u8; 32]) -> Vec<NodeId>;
    pub fn placed(&self, key: &[u8; 32], holders: &[NodeId]) -> bool;
}
```

Take one `(view, placement)` snapshot per call. Go's two lock acquisitions in `ReadOrder` differ only under
concurrent adoption.

### 4.4 Addresses and iroh mapping

```rust
pub fn parse_addrs(addrs: &[String]) -> Vec<iroh_base::TransportAddr>;
pub fn endpoint_addr(id: &NodeId, addrs: &[String]) -> Result<iroh_base::EndpointAddr, DialError>;
```

`parse_addrs` mirrors Go:
- `split_once(':')`
  - `"relay"` → `RelayUrl::from_str` (`iroh-base-1.0.3/src/relay_url.rs:38-45`, `url::Url`, WHATWG normalisation that adds `/`)
  - `"ip"` → `SocketAddr::from_str`
  - `"custom"` → `CustomAddr::from_str` (L199–214)
  - any other kind → error
- No `:` → `CustomAddr::from_str(s)`
- On error, `SocketAddr::from_str(s)` of the whole string. Otherwise skip.

`endpoint_addr` does `PublicKey::from_bytes(&id.0)`, which validates the point like go-iroh `NewEndpointID`
(`iroh-base-1.0.3/src/key.rs:122-127`). Keep Go's error text `data is not a valid public key` if the dial error is
surfaced. It then builds `EndpointAddr::from_parts(pk, parsed)` (`endpoint_addr.rs:104`).

Dialing is `iroh::Endpoint::connect(addr, alpn).await` (`iroh-1.0.3/src/endpoint.rs:1052-1056`). The transport area
owns the direct-first race: 2 s `tokio::time::timeout` for the direct candidates, relays on retry, discovery fallback.
This module only supplies candidates.

Verified parse differences (same inputs, `rustc` std versus Go `netip`):

| input | Rust `SocketAddr` | Go `netip.ParseAddrPort` |
|---|---|---|
| `[fe80::1%en0]:4433` | **Err** | ok |
| `[fe80::1%2]:4433` | ok | ok |
| `01.2.3.4:1` | Err | Err |
| `192.168.1.10:4433`, `[::ffff:1.2.3.4]:1`, `[2001:db8::1]:65535`, `1.2.3.4:0` | ok | ok |

Go nodes never publish zoned addresses, so only hand-written tickets differ. Rust `CustomAddr` data uses `HEXLOWER`,
while Go `hex.DecodeString` also accepts uppercase. That is irrelevant in practice.

### 4.5 `ticket_from_view`

```rust
pub fn ticket_from_view(v: &View) -> ticket::Ticket {
    let members: Option<Vec<ticket::Member>> = v.nodes.as_ref()
        .filter(|n| !n.is_empty())
        .map(|n| n.iter().take(4).map(|nd| ticket::Member { id: nd.id.clone(), addrs: nd.addrs.clone() }).collect());
    ticket::Ticket { cluster_id: v.cluster_id.clone(), incarnation: v.incarnation, members }
}
```

`Ticket.members` must be `Option<Vec<_>>` (nil → `f6`) and `Member.id` must be `GoBytes`. Go's `append` on a nil
slice only allocates when there is at least one node, so zero nodes means nil. Both `None` and `Some([])` nodes
produce `members: None`.

### 4.6 Crates

- `blake3 = "1.8"` (registry has 1.8.5–1.8.7; core-rs uses `"1"`)
- `iroh-base = "1.0.3"`, or the `iroh = "1.0.3"` re-exports, for `PublicKey`, `EndpointAddr`, `TransportAddr`, `RelayUrl`, `CustomAddr`
- `data-encoding = "2"` (hex, base32; iroh already depends on it) or `hex = "0.4"`
- `amber-store-core = "0.3.0"` for `cbor::append_head` (shortest-form heads) if the shared codec builds on it
- No serde.
- `ciborium-ll 0.2.2` is an option for header parsing. Its `Title::from(Header)` always emits shortest-form heads
  (`ciborium-ll-0.2.2/src/hdr.rs:118-163`), but it has no fxamacker well-formedness texts or limits, so a hand-rolled
  reader is recommended (§7).

---

## 5. Golden vectors (`tests/golden/view_placement.json`)

Deterministic inputs:
- Ids and keys come from splitmix64 `data(seed, 32)`, as defined in core-rs `VECTORS.md`.
- `idFrom(b)` is 32 copies of `b`.
- u64s are JSON strings `0x%016x`.
- NodeIds are 64-character hex strings.

The Appendix A generator produced every value below.

**`fmix64`**

| in | out |
|---|---|
| 0x0 | 0x0 |
| 0x1 | 0xb456bcfc34c2cb2c |
| 0xdeadbeef | 0xd24bd59f862a1dac |
| 0xffffffffffffffff | 0x64b5720b4b825f21 |
| 0x89a5850e63c5f8aa | **0xffffffffffffffff** (the preimage giving L = 0) |
| 0x2984f0b201423235 | 0x0123456789abcdef |

**`log2fix`**

| x | out |
|---|---|
| 1 | 0x0 |
| 2 | 0x100000000 |
| 3 | 0x195c01a39 |
| 5 | 0x25269e12f |
| 7 | 0x2ceaecfea |
| 255 | 0x7fe8df263 |
| 256 | 0x800000000 |
| 12345 | 0xd9775aaeb |
| 2^32−1 | 0x1ffffffffe |
| 2^32 | 0x2000000000 |
| 2^32+1 | 0x2000000001 |
| 2^63−1 | 0x3effffffff |
| 2^63 | 0x3f00000000 |
| 2^63+1 | 0x3f00000000 |
| 2^64−2 | 0x3fffffffff |
| 2^64−1 | 0x3fffffffff |
| splitmix(7)[0] = 0x63cbe1e459320dd7 | 0x3ea41313be |

**`slot`**

| key | slot |
|---|---|
| zero key | 0 |
| tail `ffffffffffffffff` | 1048575 |
| tail `0000100000000000` (2^44) | 1 |
| tail 2^44−1 | 0 |
| tail 2^63 | 524288 |
| bytes 0..23 = ff, tail 0 | 0 |
| `c15c…c171` | 48276 |

**`salt`**

| id | salt |
|---|---|
| idFrom(00) | 0x8886cc28faaccc20 |
| idFrom(01) | 0x91d3f42d715bdc8a |
| idFrom(02) | 0xbccce6d5ce42bd89 |
| idFrom(03) | 0xfcbb45e5214c9669 |
| idFrom(04) | 0xdf53eb64ba4f869e |
| idFrom(05) | 0x62dda6d9b7d802f3 |
| idFrom(ff) | 0xd496fd505008c003 |
| `9d3080237d64f550a1136b7ad25c2a436d129b6e30be56a3f06d2e2799622e81` | 0x6f71e490e694107f |

**`L`**

| slot | salt | L |
|---:|---|---|
| 0 | 0x0 | 0x4000000000 (h = 0) |
| 0 | 0x89a5850e63c5f8aa | 0x0 (h = 2^64−1) |
| 0 | 0xb7a9fd6380bbd367 | 0x1 (h = 2^64−2) |
| 12345 | salt(01) | 0x22356754f (also in `placement_test.go:109`) |
| 0 | salt(01) | 0x667f287c6 |
| 1048575 | salt(01) | 0x19a28fd6d |
| 524288 | salt(04) | 0x732d93ef |
| 777777 | salt(05) | 0x51bdbdb |

**`sets`** give member lists with per-slot `l[]`, `rank[]` (indexes) and `owners{rN}`:

- `golden5`: weights 1000/2000/500/4000/1000 for idFrom(1..5), matching `placement_test.go`. Ranks:
  - slot 0 → `[3,4,2,1,0]`
  - slot 1 → `[3,0,4,1,2]`
  - slot 12345 → `[1,3,4,2,0]`
  - slot 0xfffff → `[3,1,4,0,2]`
  - slot 524288 → `[3,0,1,4,2]`
  - slot 777777 → `[4,1,2,3,0]`
  - plus 10 splitmix slots, for example 274234 → `[3,1,0,2,4]`
  - owners for r ∈ {0,1,2,3,5,7}: r0 → `[]`, r7 → the full ranking
- `zones`: `TestZoneRule`'s set (a, a, b, b, and weight-0 c). With r3 there are only 2 owners.
  - slot 0: rank `[3,2,1,0,4]`, owners r3 `[3,1]`
  - slot 1048575: rank `[1,0,3,2,4]`, owners `[1,3]`
- `draining`: members given in descending id order, weights 0/300/0/300/0/300. Weight-0 members come last in id
  order: slot 0 rank `[5,1,3,4,2,0]`. With r4 there are 3 owners.
- `heavy`: weights 0xffffffff, 1, 0x80000000, 3. Exercises the upper words of the 128-bit products: slot 0 rank
  `[2,0,1,3]`, slot 1 rank `[0,2,1,3]`.
- `tie`: idFrom(09) with weight 340899772 listed first, idFrom(07) with weight 1359894136 second. Each weight is that
  member's own L at slot 2, so the products are equal. Rank at slot 2 is `[1,0]`: the smaller id wins the tie.
- `empty`: no members, so rank `[]` and owners `[]`.
- `realistic12`: splitmix ids (seeds 200–211), weights 4096/4096/8192/2048/512/16384/4096/0/1024/4096/3/1, zones
  h1,h1,h2,h2,h3,"","",h4,h4,h5,"","", r ∈ {1,3,5,12}.

**`views`** (keys: the zero key, tail all ones, tail 2^44, plus 24 splitmix keys from seeds 500–523; every member and
one non-member id get `is_owner`, `is_pending_owner` and `in_write_set` flags). The node ids are
`142e00…1eaa` (w4096), `9203eb…4d0b` (w4096, one addr), `ba9b58…d044` (w8192), `d863dc…af57` (w2048) and
`f743e7…1ae0` (w1024), with R=3. Prefixes below are the first 3 bytes.

| view | key slot | owners | pending owners | write set | read order |
|---|---|---|---|---|---|
| steady | 0 | f743e7 ba9b58 d863dc | null | = owners | f743e7 ba9b58 d863dc 9203eb 142e00 |
| steady | 1 | 9203eb ba9b58 142e00 | null | = owners | 9203eb ba9b58 142e00 d863dc f743e7 |
| join_pending (+a692ae w512) | 0 | f743e7 ba9b58 d863dc | f743e7 ba9b58 d863dc | = owners | … 142e00 **a692ae** |
| drain_pending (ba9b58 → 0) | 0 | f743e7 ba9b58 d863dc | f743e7 d863dc 9203eb | f743e7 ba9b58 d863dc 9203eb | f743e7 ba9b58 d863dc 9203eb 142e00 |
| remove_pending (−9203eb) | 1 | 9203eb ba9b58 142e00 | ba9b58 142e00 d863dc | 9203eb ba9b58 142e00 d863dc | 9203eb ba9b58 142e00 d863dc f743e7 |
| replicas_pending (R 2 → 3) | 0 | f743e7 ba9b58 | f743e7 ba9b58 d863dc | f743e7 ba9b58 d863dc | f743e7 ba9b58 d863dc 9203eb 142e00 |
| zone_pending (142e00, 9203eb in rack1) | 1 | 9203eb ba9b58 142e00 | 9203eb ba9b58 d863dc | 9203eb ba9b58 142e00 d863dc | … |
| drained_committed (ba9b58 w0, one former) | 0 | f743e7 d863dc 9203eb | null | = owners | f743e7 d863dc 9203eb 142e00 **ba9b58** |
| zones_committed (142e00, 9203eb, d863dc in hostA) | 1048575 | ba9b58 d863dc **f743e7** | null | = owners | ba9b58 d863dc 142e00 9203eb f743e7 |
| unsorted_nodes (reversed) | all | same as steady | | | |
| short_node_id (f743e7 truncated to 31 bytes) | 0 | ba9b58 d863dc 9203eb | null | | ba9b58 d863dc 9203eb 142e00 f743e7 (zero-padded) |
| duplicate_node (9203eb twice) | 1 | 9203eb ba9b58 142e00 | null | | 9203eb **9203eb** ba9b58 142e00 d863dc f743e7 |
| no_nodes | 0 | `[]` | null | `[]` | `[]` |
| pending_no_nodes | 0 | f743e7 ba9b58 d863dc | **`[]`** | = owners | = steady |

**Other sections**
- `view_cbor`: the §3.1 encodings plus `full` and `long_addrs`. Test encode(decode(hex)) == hex, and the Rust
  struct built by hand == decode(hex).
- `view_decode`: every §3.2 row, with an ok/error flag, the canonical re-encoding or the exact error text.
- `parse_node_id` and `short_id`: §3.5.
- `ticket_from_view`: §3.3.
- `helpers` for the `full` view:
  - `all_members` = `9203eb, 142e00, d863dc, a692ae` (nodes, then pending-only)
  - `voter_ids` = the 3 voters, `quorum` = 2
  - per id: `node_found`, `node_weight`, `node_addrs` (the `nodes` entry wins over the pending entry, for example
    9203eb gives 4096 with `["ip:10.0.0.1:4433"]`), `is_member` (true for data endpoints `2822106c…` and `f5d18957…`,
    with owner 9203eb), `is_former` (`bc991ae0…`), `is_voter`
- `compare`:
  - `(1,5)` vs `(1,5)` → 0; vs `(1,6)` → −1; vs `(1,4)` → 1
  - `(2,1)` vs `(1,99)` → 1; `(1,99)` vs `(2,1)` → −1
  - `(max,0)` vs `(max,max)` → −1
- `default_min_replicas`: §2.2.
- `validate_change`:
  - dropping 2 of 4 with R=3 → ok
  - dropping 3 → `view: change drops 3 nodes at once with R=3; every key owned only by them would be lost (use --force)`
  - forced → ok
  - weight 0 in the target counts as dropped
  - weight-0 current nodes are not counted
  - R=0 → ok
- `sort_nodes`: §2.2.

Add these to the vector file when the ticket area ships its own vectors: `Placed` over the pending views (the
generator can call `client.Cluster.Placed` only through a live cluster, so compute it in Rust from `Placement` and
`min_replicas`, or add a copy of the logic to the generator and label it derived). Also add `rankOwners` cases from
`client/rank_test.go` (7 tests).

---

## 6. Go tests worth porting

- `placement/placement_test.go`:
  - `TestLog2FixExact`: exact powers of two, plus 10 000 `rand.New(rand.NewSource(1))` values against float log2 with tolerance `[want−4096, want+1]`. The Rust version can use its own PRNG; the tolerance check is portable.
  - `TestFmix64Vectors`
  - `TestGoldenVectors`: the frozen ranks, salt of idFrom(1), `L(12345, salt1) = 0x22356754f`
  - `TestWeightedDistribution`: 200 000 slots, ±3 %
  - `TestAddingNodeMovesOnlyToIt`: 50 000 slots, r=3
  - `TestZoneRule`: 2000 slots, 2 owners, weight-0 last
  - `TestSlot`
- `client/rank_test.go` (client area, placement-adjacent):
  - `TestRankOwnersKeepsRankAmongUnmeasuredOwners`
  - `TestRankOwnersDoesNotDemoteUnmeasuredOwnersBehindAMeasuredOne`
  - `TestRankOwnersPrefersDirectOverRelayed`
  - `TestRankOwnersPrefersUnmeasuredOverRelayed`
  - `TestRankOwnersTiesNearRoundTripsByRank`
  - `TestRankOwnersPrefersAMuchNearerOwner`
  - `TestRankOwnersPutsPenalisedOwnersLast`
- `ticket/ticket_test.go`: `TestRoundTrip`, `TestParseIDs` (ticket area; `TicketFromView` output must satisfy them).
- There is no `view` test file in Go. Port the vector tests above instead.
- `node/cluster_test.go` is integration-only with the in-memory transport and node internals: `TestClusterRemoveNode`,
  `TestClusterNodeDownDuringWrite`, `TestClusterPullWithNodeDown`, `TestClusterPushPull`. They are useful as interop
  scenarios against real Go nodes, for example removing a node mid-push to exercise stale-view adoption and the
  write set, but they cannot be ported without a node.

---

## 7. Gaps in core-rs and Rust iroh, with workarounds

1. **No fxamacker-compatible CBOR codec in core-rs.**
   - core-rs `src/cbor.rs` only has `append_head`, `append_bstr`, `read_head`, `read_bstr` and the xattr map codec. There is no text, array, map, negative integer, simple value, tag or indefinite-length support.
   - There is no well-formedness pass with fxamacker's limits (32 levels, 131072 elements or pairs) and texts, and core-rs error texts differ.
   - Workaround: one shared `codec` module in dstore-client-rs, used by view, wire, ticket and status.
     - Encoder: shortest heads, `f6` for nil non-omitempty containers, omitempty rules. It can reuse `amber_store_core::cbor::append_head`.
     - Reader: a well-formedness pass mirroring `valid.go` with the §3.2 texts, then a pull decoder that skips unknown integer keys, keeps the first duplicate, maps null/undefined to default, records the first type error and keeps going, and validates UTF-8.
     - Verify by a differential corpus against Go, as core-rs did for xattrs.
2. **No placement code** anywhere in core-rs (expected). Port §2.1. `blake3` is already a core-rs dependency, so versions align.
3. **`iroh_base::TransportAddr` has no `FromStr`**; only `Display` exists. Hand-write `parse_addrs` (§4.4). `RelayUrl` and `CustomAddr` do have `FromStr`.
4. **Rust `SocketAddr` rejects IPv6 zone names** that Go `netip` accepts. Skip them, as Go skips unparseable addresses. Go nodes never publish such addresses.
5. **`url::Url` versus Go `url.Parse`**: WHATWG versus RFC 3986 laxness, for example Go accepts a relative `foo` that Rust rejects. This only affects dial candidates. Skip unparseable relays.
6. **iroh `PublicKey` validates the curve point.** `NodeId` must be a plain `[u8;32]`. Convert only when dialing, where go-iroh also validates.
7. **`PublicKey::fmt_short` is 5 bytes**; `ShortID` is 4. Write `short_id` by hand.
8. **No Go `unicode.IsPrint` in Rust std.** Port `strconv/isprint.go` from Go 1.26.5 (BSD) into `gofmt.rs` and test it against a Go-generated table of all runes.
9. **Go map-iteration randomness** in `anyNode`'s bootstrap fallback order, `shortError` name order and `Missing` grouping. Rust may use any order, and tests must not assert order there.

---

## 8. Risks and open decisions

**Risks**
- **Nil versus empty.** Byte identity of re-encoded views and derived tickets depends on keeping nil and empty
  distinct in every non-omitempty field (§4.3). A naive `Vec` model turns `f6` into `80`, for example a ticket from a
  view with no nodes.
- **Decode laxness must match Go.**
  - Top-level `f6` decodes to a zero view.
  - The first duplicate key wins.
  - Text keys are ignored.
  - Tags and indefinite lengths are accepted.
  - Id lengths are not validated.
  - A stricter Rust decoder would refuse views Go accepts. A laxer one would, for example, let a later duplicate
    override an earlier one.
- **Error text parity.** Decode errors surface in `client: no bootstrap node answered: …`. The §3.2 table pins the
  common texts; exotic fxamacker paths (bignum tags, huge counts) are not pinned.
- **Status panic.** `printStatus` panics on a cluster id shorter than 4 bytes: Go exit status 2 with a runtime panic
  trace. Real views always carry 16 bytes.
- **Read order helper.** `ReadOrder` preference covers the first `View().Replicas` entries (current R, not pending R).
  Port it literally.
- **Placement is order-independent** for weighted members, and weight-0 members are sorted by id. Nothing else in the
  client is order-independent: `TicketFromView`, `anyNode` and `printStatus` all use view order. Never sort the nodes
  in Rust.
- **Zone keys.** Zone keys mix explicit zones and raw id bytes. Use bytes, not an enum, to keep collision behaviour.
- **Performance.** Go preallocates 48 MiB per placement table per adopted view. The Rust lazy map is observably
  identical but has different memory behaviour, which is intended.
- **Unknown fields.** Unknown future view fields are dropped on decode, in Go too. Fine for a client, which never
  writes views back.

**Open decisions**
1. Match fxamacker decode error texts exactly for every path, or only for the §3.2 table (recommended), with a
   documented fallback text otherwise.
2. For a short cluster id in `cluster status`, mimic the Go panic (exit 2) or print gracefully. Recommendation:
   exit 2 with a `panic: runtime error: slice bounds out of range [:4] with capacity N` line on stderr, for parity.
3. `localTicket(--store DIR)` for `cluster ticket --store`, and for any client command given `--store` without
   `--ticket` (`cmd/dstore/client.go:65-69`). It needs node internals. `node.OpenOffline` (`node/node.go:776-783`,
   `Open` L201–259):
   1. reads `<store>/identity`, failing with `node: no identity in <dir>: <err>`;
   2. opens `<store>/packstore` with sync;
   3. opens the Pebble DB `<store>/meta`, and **writes** a random 16-byte `store_id` if absent;
   4. opens the paxos acceptor at `cfg.PaxosDir` (per §13 `<store>/paxos` by default; `Config.defaults` not checked);
   5. reads meta key `"view"`, and on `view.Decode` success adopts it. Decode errors are ignored.
   6. With no view it fails with `this store is not a member of a cluster`.

   Options: (a) a read-only Pebble reader for one key (SST and WAL format work, high effort); (b) shell out to the Go
   `dstore` binary; (c) refuse with a clear error. Recommendation: (c) first, (b) behind a flag.
4. Whether `parse_node_id` takes raw OS bytes (exact `\xff` escapes in errors) or `&str` (non-UTF-8 arguments rejected
   earlier by clap with a different message). Recommendation: raw bytes on Unix.
5. The placement cache structure: a lazy `RwLock<HashMap>` per table (recommended) versus a sharded or bounded LRU
   for million-key pushes.
6. Where `go_quote` and the fxamacker codec live. Recommendation: shared modules `gofmt` and `codec`, owned by the
   codec/CLI areas, consumed here.

---

## Appendix A: vector generator (Go, verified to run)

Run with the cached toolchain, offline:

```sh
GOTOOLCHAIN=local GOPROXY=off GOFLAGS=-mod=mod \
  /Users/dragan/go/pkg/mod/golang.org/toolchain@v0.0.1-go1.26.5.darwin-arm64/bin/go run . > view_placement.json
```

The research run used `replace github.com/amber-store/dstore => /Users/dragan/amber-store/dstore`, which is identical
to `v0.1.9`, and copied dstore's `go.sum`. In the real tool, `require github.com/amber-store/dstore v0.1.9` with no
replace. It imports `node` and `worktree` only for `node.ShortID` and `worktree.TicketFromView`, which pulls in Pebble
and go-iroh at build time. That is acceptable for a generator. Output is deterministic, about 700 KB.

```go
// Command viewgen emits golden vectors for the Rust port of dstore's view
// and placement packages.
package main

import (
	"bytes"
	"encoding/base32"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"strings"

	"github.com/amber-store/dstore/node"
	"github.com/amber-store/dstore/placement"
	"github.com/amber-store/dstore/view"
	"github.com/amber-store/dstore/worktree"
	"github.com/zeebo/blake3"
)

func splitmixNext(state *uint64) uint64 {
	*state += 0x9E3779B97F4A7C15
	z := *state
	z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9
	z = (z ^ (z >> 27)) * 0x94D049BB133111EB
	return z ^ (z >> 31)
}

func smData(seed uint64, n int) []byte {
	out := make([]byte, 0, n+8)
	state := seed
	for len(out) < n {
		out = binary.LittleEndian.AppendUint64(out, splitmixNext(&state))
	}
	return out[:n]
}

func u64s(seed uint64, n int) []uint64 {
	out := make([]uint64, n)
	state := seed
	for i := range out {
		out[i] = splitmixNext(&state)
	}
	return out
}

func h64(x uint64) string { return fmt.Sprintf("0x%016x", x) }

func idFrom(b byte) placement.NodeID {
	var id placement.NodeID
	for i := range id {
		id[i] = b
	}
	return id
}

func idSeed(seed uint64) placement.NodeID {
	var id placement.NodeID
	copy(id[:], smData(seed, 32))
	return id
}

func keySeed(seed uint64) [32]byte {
	var k [32]byte
	copy(k[:], smData(seed, 32))
	return k
}

func modInverse(a uint64) uint64 {
	inv := a
	for i := 0; i < 6; i++ {
		inv *= 2 - a*inv
	}
	return inv
}

// invFmix64 inverts the murmur3 finalizer (shift 33 > 32 makes x ^= x>>33 an involution).
func invFmix64(x uint64) uint64 {
	x ^= x >> 33
	x *= modInverse(0xc4ceb9fe1a85ec53)
	x ^= x >> 33
	x *= modInverse(0xff51afd7ed558ccd)
	x ^= x >> 33
	return x
}

func must(err error) {
	if err != nil {
		panic(err)
	}
}

func must1(b []byte, err error) []byte {
	must(err)
	return b
}

type kv struct {
	In  string `json:"in"`
	Out string `json:"out"`
}
type slotVec struct {
	Key  string `json:"key"`
	Slot uint32 `json:"slot"`
}
type saltVec struct {
	ID   string `json:"id"`
	Salt string `json:"salt"`
}
type lVec struct {
	Slot uint32 `json:"slot"`
	Salt string `json:"salt"`
	L    string `json:"l"`
}
type memberJ struct {
	ID     string `json:"id"`
	Weight uint32 `json:"weight"`
	Zone   string `json:"zone"`
}
type setSlot struct {
	Slot   uint32           `json:"slot"`
	L      []string         `json:"l"`
	Rank   []int            `json:"rank"`
	Owners map[string][]int `json:"owners"`
}
type setVec struct {
	Name     string    `json:"name"`
	Members  []memberJ `json:"members"`
	Replicas []int     `json:"replicas"`
	Slots    []setSlot `json:"slots"`
}
type idFlags struct {
	ID             string `json:"id"`
	IsOwner        bool   `json:"is_owner"`
	IsPendingOwner bool   `json:"is_pending_owner"`
	InWriteSet     bool   `json:"in_write_set"`
}
type keyVec struct {
	Key           string    `json:"key"`
	Slot          uint32    `json:"slot"`
	Owners        []string  `json:"owners"`
	PendingOwners []string  `json:"pending_owners"`
	WriteSet      []string  `json:"write_set"`
	ReadOrder     []string  `json:"read_order"`
	Flags         []idFlags `json:"flags"`
}
type viewVec struct {
	Name string   `json:"name"`
	CBOR string   `json:"cbor"`
	Keys []keyVec `json:"keys"`
}
type cborVec struct {
	Name string `json:"name"`
	CBOR string `json:"cbor"`
}
type decodeVec struct {
	Name     string `json:"name"`
	Input    string `json:"input"`
	OK       bool   `json:"ok"`
	Reencode string `json:"reencode,omitempty"`
	Error    string `json:"error,omitempty"`
}
type parseVec struct {
	Input string `json:"input"`
	OK    bool   `json:"ok"`
	ID    string `json:"id,omitempty"`
	Error string `json:"error,omitempty"`
}
type shortVec struct {
	Bytes     string `json:"bytes"`
	NodeShort string `json:"node_short"`
	ViewShort string `json:"view_short,omitempty"`
	IDString  string `json:"id_string,omitempty"`
}
type ticketVec struct {
	Name   string `json:"name"`
	View   string `json:"view"`
	Ticket string `json:"ticket"`
	TCBOR  string `json:"ticket_cbor_hex"`
}
type lookupVec struct {
	ID                string   `json:"id"`
	NodeFound         bool     `json:"node_found"`
	NodeWeight        uint32   `json:"node_weight"`
	NodeAddrs         []string `json:"node_addrs"`
	IsMember          bool     `json:"is_member"`
	DataEndpointOwner string   `json:"data_endpoint_owner"`
	IsFormer          bool     `json:"is_former"`
	IsVoter           bool     `json:"is_voter"`
}
type helpersVec struct {
	View       string      `json:"view"`
	AllMembers []string    `json:"all_members"`
	VoterIDs   []string    `json:"voter_ids"`
	Quorum     int         `json:"quorum"`
	Lookups    []lookupVec `json:"lookups"`
}
type compareVec struct {
	ViewInc   uint64 `json:"view_incarnation"`
	ViewEpoch uint64 `json:"view_epoch"`
	Inc       uint64 `json:"incarnation"`
	Epoch     uint64 `json:"epoch"`
	Result    int    `json:"result"`
}
type validateVec struct {
	Name     string    `json:"name"`
	Cur      []memberJ `json:"cur"`
	Target   []memberJ `json:"target"`
	Replicas int       `json:"replicas"`
	Force    bool      `json:"force"`
	Error    string    `json:"error"`
}
type out struct {
	Fmix64      []kv           `json:"fmix64"`
	Log2Fix     []kv           `json:"log2fix"`
	Slot        []slotVec      `json:"slot"`
	Salt        []saltVec      `json:"salt"`
	L           []lVec         `json:"l"`
	Sets        []setVec       `json:"sets"`
	Views       []viewVec      `json:"views"`
	ViewCBOR    []cborVec      `json:"view_cbor"`
	ViewDecode  []decodeVec    `json:"view_decode"`
	ParseNodeID []parseVec     `json:"parse_node_id"`
	ShortID     []shortVec     `json:"short_id"`
	TicketFrom  []ticketVec    `json:"ticket_from_view"`
	Helpers     helpersVec     `json:"helpers"`
	Compare     []compareVec   `json:"compare"`
	DefaultMinR map[string]int `json:"default_min_replicas"`
	Validate    []validateVec  `json:"validate_change"`
	SortNodes   []string       `json:"sort_nodes"`
}

func hexIDs(ids []placement.NodeID) []string {
	if ids == nil {
		return nil
	}
	o := make([]string, len(ids))
	for i, id := range ids {
		o[i] = hex.EncodeToString(id[:])
	}
	return o
}

func setVector(name string, members []placement.Member, rs []int, slots []uint32) setVec {
	set := placement.NewSet(members)
	sv := setVec{Name: name, Replicas: rs}
	for _, m := range members {
		sv.Members = append(sv.Members, memberJ{ID: hex.EncodeToString(m.ID[:]), Weight: m.Weight, Zone: m.Zone})
	}
	for _, s := range slots {
		ss := setSlot{Slot: s, Owners: map[string][]int{}}
		for _, m := range members {
			if m.Weight == 0 {
				ss.L = append(ss.L, "")
			} else {
				ss.L = append(ss.L, h64(placement.L(s, placement.Salt(m.ID))))
			}
		}
		ss.Rank = set.Rank(s)
		for _, r := range rs {
			ss.Owners[fmt.Sprintf("r%d", r)] = set.Owners(s, r)
		}
		sv.Slots = append(sv.Slots, ss)
	}
	return sv
}

func keyWithTail(tail uint64) [32]byte {
	var k [32]byte
	binary.BigEndian.PutUint64(k[24:], tail)
	return k
}

func viewVector(name string, v *view.View, keys [][32]byte, extra []placement.NodeID) viewVec {
	pl := view.NewPlacement(v)
	vv := viewVec{Name: name, CBOR: hex.EncodeToString(must1(v.Encode()))}
	ids := append(v.AllMembers(), extra...)
	for _, k := range keys {
		kvv := keyVec{Key: hex.EncodeToString(k[:]), Slot: placement.Slot(k)}
		kvv.Owners = hexIDs(pl.Owners(k))
		kvv.PendingOwners = hexIDs(pl.PendingOwners(k))
		kvv.WriteSet = hexIDs(pl.WriteSet(k))
		kvv.ReadOrder = hexIDs(pl.ReadOrder(k))
		for _, id := range ids {
			kvv.Flags = append(kvv.Flags, idFlags{ID: hex.EncodeToString(id[:]), IsOwner: pl.IsOwner(k, id), IsPendingOwner: pl.IsPendingOwner(k, id), InWriteSet: pl.InWriteSet(k, id)})
		}
		vv.Keys = append(vv.Keys, kvv)
	}
	return vv
}

func vnode(id placement.NodeID, w uint32, zone string, addrs ...string) view.Node {
	return view.Node{ID: append([]byte{}, id[:]...), Weight: w, Addrs: addrs, Zone: zone, Writable: true, Incarnation: 1}
}

func cloneNodes(nodes []view.Node) []view.Node {
	v, _ := view.Decode(must1((&view.View{Nodes: nodes}).Encode()))
	return v.Nodes
}

// cw is a tiny CBOR writer for hand-built decode inputs.
type cw struct{ b []byte }

func (c *cw) head(major byte, n uint64) *cw {
	m := major << 5
	switch {
	case n < 24:
		c.b = append(c.b, m|byte(n))
	case n <= 0xff:
		c.b = append(c.b, m|24, byte(n))
	case n <= 0xffff:
		c.b = append(c.b, m|25, byte(n>>8), byte(n))
	case n <= 0xffffffff:
		c.b = append(c.b, m|26, byte(n>>24), byte(n>>16), byte(n>>8), byte(n))
	default:
		c.b = append(c.b, m|27)
		c.b = binary.BigEndian.AppendUint64(c.b, n)
	}
	return c
}
func (c *cw) u(n uint64) *cw   { return c.head(0, n) }
func (c *cw) neg(n uint64) *cw { return c.head(1, n) }
func (c *cw) bs(b []byte) *cw  { c.head(2, uint64(len(b))); c.b = append(c.b, b...); return c }
func (c *cw) ts(s string) *cw  { c.head(3, uint64(len(s))); c.b = append(c.b, s...); return c }
func (c *cw) arr(n uint64) *cw { return c.head(4, n) }
func (c *cw) mp(n uint64) *cw  { return c.head(5, n) }
func (c *cw) raw(h string) *cw { b, err := hex.DecodeString(h); must(err); c.b = append(c.b, b...); return c }
func (c *cw) bytes() []byte    { return c.b }

func decodeCase(name string, in []byte) decodeVec {
	d := decodeVec{Name: name, Input: hex.EncodeToString(in)}
	v, err := view.Decode(in)
	if err != nil {
		d.Error = err.Error()
		return d
	}
	d.OK = true
	d.Reencode = hex.EncodeToString(must1(v.Encode()))
	return d
}

func main() {
	var o out

	for _, x := range append([]uint64{0, 1, 0xdeadbeef, ^uint64(0), invFmix64(^uint64(0)), invFmix64(0x0123456789abcdef)}, u64s(42, 4)...) {
		o.Fmix64 = append(o.Fmix64, kv{In: h64(x), Out: h64(placement.Fmix64(x))})
	}
	if placement.Fmix64(invFmix64(^uint64(0))) != ^uint64(0) {
		panic("invFmix64")
	}
	for _, x := range append([]uint64{1, 2, 3, 5, 7, 255, 256, 12345, 1<<32 - 1, 1 << 32, 1<<32 + 1, 1<<63 - 1, 1 << 63, 1<<63 + 1, ^uint64(0) - 1, ^uint64(0)}, u64s(7, 6)...) {
		o.Log2Fix = append(o.Log2Fix, kv{In: h64(x), Out: h64(placement.Log2Fix(x))})
	}
	slotKeys := [][32]byte{{}, keyWithTail(^uint64(0)), keyWithTail(1 << 44), keyWithTail(1<<44 - 1), keyWithTail(0x8000000000000000), keySeed(1), keySeed(2), keySeed(3)}
	{
		var k [32]byte
		for i := 0; i < 24; i++ {
			k[i] = 0xff
		}
		slotKeys = append(slotKeys, k)
	}
	for _, k := range slotKeys {
		o.Slot = append(o.Slot, slotVec{Key: hex.EncodeToString(k[:]), Slot: placement.Slot(k)})
	}
	for _, id := range []placement.NodeID{idFrom(0), idFrom(1), idFrom(2), idFrom(3), idFrom(4), idFrom(5), idFrom(0xff), idSeed(11), idSeed(12), idSeed(13)} {
		s := placement.Salt(id)
		sum := blake3.Sum256(append([]byte("amber-dstore/placement/1"), id[:]...))
		if binary.BigEndian.Uint64(sum[:8]) != s {
			panic("salt is not the Sum256 prefix")
		}
		o.Salt = append(o.Salt, saltVec{ID: hex.EncodeToString(id[:]), Salt: h64(s)})
	}
	salt1 := placement.Salt(idFrom(1))
	for _, c := range []struct {
		slot uint32
		salt uint64
	}{
		{0, 0}, {0, invFmix64(^uint64(0))}, {0, invFmix64(^uint64(0) - 1)}, {12345, salt1}, {0, salt1}, {placement.Slots - 1, salt1},
		{524288, placement.Salt(idFrom(4))}, {777777, placement.Salt(idFrom(5))}, {3, placement.Salt(idSeed(11))},
	} {
		o.L = append(o.L, lVec{Slot: c.slot, Salt: h64(c.salt), L: h64(placement.L(c.slot, c.salt))})
	}

	goldenSlots := []uint32{0, 1, 12345, 0xfffff, 524288, 777777}
	for _, x := range u64s(99, 10) {
		goldenSlots = append(goldenSlots, uint32(x>>44))
	}
	o.Sets = append(o.Sets, setVector("golden5", []placement.Member{
		{ID: idFrom(0x01), Weight: 1000}, {ID: idFrom(0x02), Weight: 2000}, {ID: idFrom(0x03), Weight: 500},
		{ID: idFrom(0x04), Weight: 4000}, {ID: idFrom(0x05), Weight: 1000},
	}, []int{0, 1, 2, 3, 5, 7}, goldenSlots))
	o.Sets = append(o.Sets, setVector("zones", []placement.Member{
		{ID: idFrom(1), Weight: 100, Zone: "a"}, {ID: idFrom(2), Weight: 100, Zone: "a"},
		{ID: idFrom(3), Weight: 100, Zone: "b"}, {ID: idFrom(4), Weight: 100, Zone: "b"}, {ID: idFrom(5), Weight: 0, Zone: "c"},
	}, []int{1, 2, 3}, goldenSlots[:8]))
	o.Sets = append(o.Sets, setVector("draining", []placement.Member{
		{ID: idFrom(9), Weight: 0}, {ID: idFrom(8), Weight: 300}, {ID: idFrom(7), Weight: 0},
		{ID: idFrom(6), Weight: 300}, {ID: idFrom(5), Weight: 0}, {ID: idFrom(4), Weight: 300},
	}, []int{2, 3, 4}, goldenSlots[:8]))
	o.Sets = append(o.Sets, setVector("heavy", []placement.Member{
		{ID: idFrom(0x10), Weight: 0xffffffff}, {ID: idFrom(0x11), Weight: 1}, {ID: idFrom(0x12), Weight: 0x80000000}, {ID: idFrom(0x13), Weight: 3},
	}, []int{2, 4}, goldenSlots))
	{ // weights equal to the members' own L at one slot give equal products
		s9, s7 := placement.Salt(idFrom(9)), placement.Salt(idFrom(7))
		for slot := uint32(0); slot < placement.Slots; slot++ {
			la, lb := placement.L(slot, s9), placement.L(slot, s7)
			if la > 0 && lb > 0 && la < 1<<32 && lb < 1<<32 && la != lb {
				o.Sets = append(o.Sets, setVector("tie", []placement.Member{{ID: idFrom(9), Weight: uint32(la)}, {ID: idFrom(7), Weight: uint32(lb)}}, []int{1, 2}, []uint32{slot, 0, 1}))
				break
			}
		}
	}
	o.Sets = append(o.Sets, setVector("empty", nil, []int{0, 3}, []uint32{0, 5}))
	var realistic []placement.Member
	weights := []uint32{4096, 4096, 8192, 2048, 512, 16384, 4096, 0, 1024, 4096, 3, 1}
	zs := []string{"h1", "h1", "h2", "h2", "h3", "", "", "h4", "h4", "h5", "", ""}
	for i := range weights {
		realistic = append(realistic, placement.Member{ID: idSeed(uint64(200 + i)), Weight: weights[i], Zone: zs[i]})
	}
	o.Sets = append(o.Sets, setVector("realistic12", realistic, []int{1, 3, 5, 12}, goldenSlots))

	ids := make([]placement.NodeID, 12)
	for i := range ids {
		ids[i] = idSeed(uint64(300 + i))
	}
	nodes5 := []view.Node{vnode(ids[0], 4096, "", "ip:192.168.1.10:4433"), vnode(ids[1], 4096, ""), vnode(ids[2], 2048, ""), vnode(ids[3], 8192, ""), vnode(ids[4], 1024, "")}
	view.SortNodes(nodes5)
	cid := smData(1000, 16)
	base := func() *view.View {
		return &view.View{ClusterID: cid, Incarnation: 1, Epoch: 9, Version: 20, PlacementEpoch: 8, Replicas: 3, MinReplicas: 2,
			Voters: []view.Voter{{ID: nodes5[0].ID, Since: 1}, {ID: nodes5[1].ID, Since: 3}, {ID: nodes5[2].ID, Since: 5}},
			Nodes:  cloneNodes(nodes5)}
	}
	vkeys := [][32]byte{{}, keyWithTail(^uint64(0)), keyWithTail(1 << 44)}
	for i := uint64(0); i < 24; i++ {
		vkeys = append(vkeys, keySeed(500+i))
	}
	nonMember := []placement.NodeID{idSeed(999)}

	o.Views = append(o.Views, viewVector("steady", base(), vkeys, nonMember))
	join := base()
	join.Epoch = 10
	join.Pending = &view.Pending{Nodes: append(cloneNodes(nodes5), vnode(ids[5], 512, "")), Replicas: 3, ID: 10, Reason: "join " + view.ShortID(ids[5]), Since: 1757000000000000000}
	view.SortNodes(join.Pending.Nodes)
	o.Views = append(o.Views, viewVector("join_pending", join, vkeys, nonMember))
	drain := base()
	drain.Epoch = 10
	drain.Pending = &view.Pending{Nodes: cloneNodes(nodes5), Replicas: 3, ID: 10, Reason: "node-drain"}
	drain.Pending.Nodes[2].Weight = 0
	o.Views = append(o.Views, viewVector("drain_pending", drain, vkeys, nonMember))
	remove := base()
	remove.Epoch = 10
	remove.Pending = &view.Pending{Nodes: append(cloneNodes(nodes5[:1]), cloneNodes(nodes5[2:])...), Replicas: 3, ID: 10, Reason: "node-remove"}
	o.Views = append(o.Views, viewVector("remove_pending", remove, vkeys, nonMember))
	repl := base()
	repl.Replicas, repl.Epoch = 2, 10
	repl.Pending = &view.Pending{Nodes: cloneNodes(nodes5), Replicas: 3, ID: 10, Reason: "replicas"}
	o.Views = append(o.Views, viewVector("replicas_pending", repl, vkeys, nonMember))
	zonep := base()
	zonep.Epoch = 10
	zonep.Pending = &view.Pending{Nodes: cloneNodes(nodes5), Replicas: 3, ID: 10, Reason: "node-zone"}
	zonep.Pending.Nodes[0].Zone, zonep.Pending.Nodes[1].Zone, zonep.Pending.Nodes[2].Zone = "rack1", "rack1", "rack2"
	o.Views = append(o.Views, viewVector("zone_pending", zonep, vkeys, nonMember))
	drained := base()
	drained.Nodes[2].Weight = 0
	drained.Former = []view.Former{{ID: ids[6][:], Until: 1760000000000000000}}
	o.Views = append(o.Views, viewVector("drained_committed", drained, vkeys, append(nonMember, ids[6])))
	unsorted := base()
	for i, j := 0, len(unsorted.Nodes)-1; i < j; i, j = i+1, j-1 {
		unsorted.Nodes[i], unsorted.Nodes[j] = unsorted.Nodes[j], unsorted.Nodes[i]
	}
	o.Views = append(o.Views, viewVector("unsorted_nodes", unsorted, vkeys, nonMember))
	zoned := base()
	zoned.Nodes[0].Zone, zoned.Nodes[1].Zone, zoned.Nodes[3].Zone = "hostA", "hostA", "hostA"
	o.Views = append(o.Views, viewVector("zones_committed", zoned, vkeys, nonMember))
	shortid := base()
	shortid.Nodes[4].ID = shortid.Nodes[4].ID[:31]
	o.Views = append(o.Views, viewVector("short_node_id", shortid, vkeys[:6], nonMember))
	dup := base()
	dup.Nodes = append(dup.Nodes, cloneNodes(nodes5[1:2])...)
	o.Views = append(o.Views, viewVector("duplicate_node", dup, vkeys[:6], nonMember))
	nonodes := base()
	nonodes.Nodes = nil
	o.Views = append(o.Views, viewVector("no_nodes", nonodes, vkeys[:3], nonMember))
	pendEmpty := base()
	pendEmpty.Pending = &view.Pending{Replicas: 3, ID: 10}
	o.Views = append(o.Views, viewVector("pending_no_nodes", pendEmpty, vkeys[:6], nonMember))

	init := &view.View{ClusterID: cid, Incarnation: 1, Epoch: 1, Version: 1, PlacementEpoch: 1, Replicas: 3, MinReplicas: 2,
		Voters: []view.Voter{{ID: ids[0][:], Since: 1}},
		Nodes:  []view.Node{{ID: ids[0][:], Weight: 931, Addrs: []string{"ip:192.168.1.10:51820", "ip:[fe80::1]:51820", "relay:https://euw1-1.relay.n0.iroh-canary.iroh.link./"}, Writable: true, Incarnation: 1}}}
	full := &view.View{
		ClusterID: cid, Incarnation: 2, Epoch: 57, Version: 311, PlacementEpoch: 54, Replicas: 3, MinReplicas: 2,
		Voters:    []view.Voter{{ID: ids[0][:], Since: 1}, {ID: ids[1][:], Since: 12}, {ID: ids[2][:], Since: 30}},
		VoterSync: view.VoterSyncPending, VoterSyncCursor: []byte("join/\x00\x01"),
		Nodes: []view.Node{
			{ID: ids[0][:], Weight: 4096, Addrs: []string{"ip:10.0.0.1:4433"}, Data: []view.DataEndpoint{{ID: ids[9][:], Addrs: []string{"ip:10.0.0.1:4434"}}, {ID: ids[10][:]}}, Token: smData(77, 32), Zone: "rack-1", Incarnation: 3, Writable: true},
			{ID: ids[1][:], Weight: 2048, Zone: "rack-2", Incarnation: 1, Writable: false},
			{ID: ids[2][:], Weight: 0, Addrs: []string{"relay:https://relay.example/"}, Writable: true, Incarnation: 1},
		},
		Pending: &view.Pending{
			Nodes: []view.Node{{ID: ids[0][:], Weight: 4096, Writable: true}, {ID: ids[5][:], Weight: 512, Writable: true, Incarnation: 1}}, Replicas: 2, ID: 57,
			ParticipantsAck: [][]byte{ids[0][:], ids[5][:]}, Participants: [][]byte{ids[0][:]}, Frozen: true, Round: 2,
			PrimaryDone: [][]byte{ids[0][:]}, Done: [][]byte{ids[0][:]}, FrozenAt: 1757999999123456789, Reason: "join " + view.ShortID(ids[5]),
			Ramp: &view.Ramp{Node: ids[5][:], Target: 4096, Step: 1}, Since: 1757999990000000000,
		},
		Former: []view.Former{{ID: ids[7][:], Until: 1760000000000000000}}, Fenced: [][]byte{ids[8][:]},
		RecoveredInc: 1, RecoveredEpoch: 40, RebalancePause: true, RateCap: 104857600,
		VoterSyncTarget: ids[5][:], VoterSyncAdd: true, DeferredVoters: [][]byte{ids[11][:]},
		ACL:   &view.ACL{Allowed: [][]byte{ids[9][:], ids[10][:]}, Admins: [][]byte{ids[9][:]}},
		Ramps: []view.Ramp{{Node: ids[5][:], Target: 4096, Step: 1}}, RemoveVoters: [][]byte{ids[7][:]},
	}
	for _, c := range []struct {
		name string
		v    *view.View
	}{
		{"zero", &view.View{}},
		{"init", init},
		{"full", full},
		{"empty_non_nil", &view.View{ClusterID: []byte{}, Voters: []view.Voter{}, Nodes: []view.Node{}, Former: []view.Former{}, Fenced: [][]byte{}, VoterSyncCursor: []byte{}, Ramps: []view.Ramp{}}},
		{"zero_pointers", &view.View{Pending: &view.Pending{}, ACL: &view.ACL{}}},
		{"pending_zero_ramp", &view.View{Pending: &view.Pending{Ramp: &view.Ramp{}, Nodes: []view.Node{}}}},
		{"negatives", &view.View{VoterSync: -1, Former: []view.Former{{ID: ids[0][:], Until: -1}}, Pending: &view.Pending{FrozenAt: -5, Since: -1 << 63, Ramp: &view.Ramp{Step: -2}}, Ramps: []view.Ramp{{Step: -300}}}},
		{"maxes", &view.View{Incarnation: ^uint64(0), Epoch: 1 << 32, Version: 1<<32 - 1, PlacementEpoch: 65536, Replicas: 255, MinReplicas: 24, VoterSync: 1 << 40, RateCap: ^uint64(0),
			Nodes: []view.Node{{ID: ids[0][:], Weight: 0xffffffff, Incarnation: ^uint64(0), Writable: true}}, Pending: &view.Pending{Round: 0xffffffff, ID: 23, Replicas: 23}}},
		{"utf8", &view.View{Nodes: []view.Node{{ID: ids[0][:], Weight: 1, Zone: "zürich-\U0001F3D4"}}, Pending: &view.Pending{Reason: "ramp ✓"}}},
		{"node_nil_and_empty_id", &view.View{Nodes: []view.Node{{ID: nil, Weight: 1}, {ID: []byte{}, Weight: 2}}}},
		{"writable_false_and_data_no_addrs", &view.View{Nodes: []view.Node{{ID: ids[0][:], Data: []view.DataEndpoint{{ID: ids[1][:]}, {}}}}}},
		{"long_addrs", &view.View{Nodes: []view.Node{{ID: ids[0][:], Weight: 1, Addrs: []string{strings.Repeat("a", 23), strings.Repeat("b", 24), strings.Repeat("c", 255), strings.Repeat("d", 256)}}}}},
	} {
		enc := must1(c.v.Encode())
		dec, err := view.Decode(enc)
		must(err)
		if !bytes.Equal(must1(dec.Encode()), enc) {
			panic("round trip " + c.name)
		}
		o.ViewCBOR = append(o.ViewCBOR, cborVec{Name: c.name, CBOR: hex.EncodeToString(enc)})
	}
	o.Helpers = helpersVec{View: hex.EncodeToString(must1(full.Encode())), Quorum: full.Quorum()}
	for _, id := range full.AllMembers() {
		o.Helpers.AllMembers = append(o.Helpers.AllMembers, hex.EncodeToString(id[:]))
	}
	for _, id := range full.VoterIDs() {
		o.Helpers.VoterIDs = append(o.Helpers.VoterIDs, hex.EncodeToString(id[:]))
	}
	for _, id := range append(append([]placement.NodeID{}, ids...), idSeed(999)) {
		nd, ok := full.Node(id)
		lv := lookupVec{ID: hex.EncodeToString(id[:]), NodeFound: ok, NodeWeight: nd.Weight, NodeAddrs: nd.Addrs, IsMember: full.IsMember(id), IsFormer: full.IsFormer(id), IsVoter: full.IsVoter(id)}
		if p := full.DataEndpointOwner(id); p != nil {
			lv.DataEndpointOwner = hex.EncodeToString(p[:])
		}
		o.Helpers.Lookups = append(o.Helpers.Lookups, lv)
	}

	id32 := ids[0][:]
	canon := must1(init.Encode())
	o.ViewDecode = append(o.ViewDecode,
		decodeCase("empty_input", []byte{}),
		decodeCase("canonical_init", canon),
		decodeCase("trailing_zero_byte", append(append([]byte{}, canon...), 0x00)),
		decodeCase("truncated", canon[:len(canon)-1]),
		decodeCase("top_level_array", (&cw{}).arr(0).bytes()),
		decodeCase("top_level_null", (&cw{}).raw("f6").bytes()),
		decodeCase("empty_map", (&cw{}).mp(0).bytes()),
		decodeCase("reordered_keys", (&cw{}).mp(3).u(10).arr(0).u(2).u(5).u(0).bs(cid).bytes()),
		decodeCase("unknown_int_key", (&cw{}).mp(2).u(2).u(5).u(99).ts("x").bytes()),
		decodeCase("unknown_negative_key", (&cw{}).mp(2).neg(0).u(1).u(2).u(5).bytes()),
		decodeCase("duplicate_key_epoch", (&cw{}).mp(2).u(2).u(1).u(2).u(7).bytes()),
		decodeCase("text_key_2", (&cw{}).mp(1).ts("2").u(9).bytes()),
		decodeCase("text_key_Epoch", (&cw{}).mp(1).ts("Epoch").u(9).bytes()),
		decodeCase("bytes_key", (&cw{}).mp(2).bs([]byte{2}).u(9).u(1).u(4).bytes()),
		decodeCase("null_nodes_and_voters", (&cw{}).mp(2).u(10).raw("f6").u(7).raw("f6").bytes()),
		decodeCase("null_replicas", (&cw{}).mp(1).u(5).raw("f6").bytes()),
		decodeCase("undefined_epoch", (&cw{}).mp(1).u(2).raw("f7").bytes()),
		decodeCase("null_pending", (&cw{}).mp(1).u(11).raw("f6").bytes()),
		decodeCase("empty_pending_map", (&cw{}).mp(1).u(11).mp(0).bytes()),
		decodeCase("replicas_256", (&cw{}).mp(1).u(5).u(256).bytes()),
		decodeCase("weight_2pow32", (&cw{}).mp(1).u(10).arr(1).mp(2).u(0).bs(id32).u(1).u(1<<32).bytes()),
		decodeCase("negative_incarnation", (&cw{}).mp(1).u(1).neg(0).bytes()),
		decodeCase("negative_voter_sync", (&cw{}).mp(1).u(8).neg(4).bytes()),
		decodeCase("float_epoch", (&cw{}).mp(1).u(2).raw("f93c00").bytes()),
		decodeCase("bool_as_int_writable", (&cw{}).mp(1).u(10).arr(1).mp(2).u(0).bs(id32).u(7).u(1).bytes()),
		decodeCase("frozen_true", (&cw{}).mp(1).u(11).mp(1).u(5).raw("f5").bytes()),
		decodeCase("invalid_utf8_zone", (&cw{}).mp(1).u(10).arr(1).mp(2).u(0).bs(id32).u(5).raw("62c328").bytes()),
		decodeCase("bytes_zone", (&cw{}).mp(1).u(10).arr(1).mp(2).u(0).bs(id32).u(5).bs([]byte("z")).bytes()),
		decodeCase("text_cluster_id", (&cw{}).mp(1).u(0).ts("abcd").bytes()),
		decodeCase("node_id_2_bytes", (&cw{}).mp(1).u(10).arr(1).mp(2).u(0).bs([]byte{1, 2}).u(1).u(5).bytes()),
		decodeCase("non_shortest_heads", (&cw{}).raw("b90001").raw("1802").raw("1a00000005").bytes()),
		decodeCase("indefinite_map_and_array", (&cw{}).raw("bf").u(2).u(5).u(10).raw("9f").mp(2).u(0).bs(id32).u(1).u(3).raw("ff").raw("ff").bytes()),
		decodeCase("indefinite_bytes_cluster_id", (&cw{}).mp(1).u(0).raw("5f").bs([]byte{1, 2}).bs([]byte{3}).raw("ff").bytes()),
		decodeCase("self_describe_tag", append([]byte{0xd9, 0xd9, 0xf7}, canon...)),
		decodeCase("tag_on_epoch", (&cw{}).mp(1).u(2).raw("c1").u(5).bytes()),
		decodeCase("type_error_then_overflow", (&cw{}).mp(2).u(1).ts("x").u(5).u(300).bytes()),
		decodeCase("error_then_malformed_later", (&cw{}).mp(2).u(5).u(300).u(1).ts("x").bytes()),
		decodeCase("nodes_not_array", (&cw{}).mp(1).u(10).mp(0).bytes()),
		decodeCase("pending_as_array", (&cw{}).mp(1).u(11).arr(0).bytes()),
		decodeCase("ramp_step_negative", (&cw{}).mp(1).u(22).arr(1).mp(1).u(2).neg(9).bytes()),
		decodeCase("ramp_step_2pow63", (&cw{}).mp(1).u(22).arr(1).mp(1).u(2).u(1<<63).bytes()),
		decodeCase("former_until_uint_2pow63", (&cw{}).mp(1).u(12).arr(1).mp(1).u(1).u(1<<63).bytes()),
		decodeCase("reserved_ai_28", (&cw{}).raw("bc").bytes()),
		decodeCase("simple_value_key", (&cw{}).mp(1).raw("f4").u(1).bytes()),
	)
	{
		c := (&cw{}).mp(1).u(99)
		for i := 0; i < 33; i++ {
			c.arr(1)
		}
		o.ViewDecode = append(o.ViewDecode, decodeCase("nested_33_levels_unknown_key", c.u(0).bytes()))
	}

	pid := idSeed(7)
	hx := hex.EncodeToString(pid[:])
	b32 := strings.ToLower(base32.StdEncoding.WithPadding(base32.NoPadding).EncodeToString(pid[:]))
	for _, s := range []string{hx, strings.ToUpper(hx), hx[:10] + strings.ToUpper(hx[10:40]) + hx[40:], hx[:63], hx + "0", hx + "00", "0x" + hx[:62], " " + hx[1:], hx + "\n", b32, "", strings.Repeat("zz", 32), "ab\tcd", "héllo \U0001F600", "\xff\xfe", `quote"back\slash`, "\x00\x7f", "​"} {
		p := parseVec{Input: s}
		if id, err := view.ParseNodeID(s); err != nil {
			p.Error = err.Error()
		} else {
			p.OK, p.ID = true, hex.EncodeToString(id[:])
		}
		o.ParseNodeID = append(o.ParseNodeID, p)
	}
	for _, b := range [][]byte{pid[:], pid[:4], {}, nil, append(append([]byte{}, pid[:]...), 0)} {
		sv := shortVec{Bytes: hex.EncodeToString(b), NodeShort: node.ShortID(b)}
		if len(b) == 32 {
			sv.ViewShort, sv.IDString = view.ShortID(view.NodeID(b)), view.IDString(view.NodeID(b))
		}
		o.ShortID = append(o.ShortID, sv)
	}
	five := base()
	five.Nodes = append(five.Nodes, vnode(ids[11], 9, "", "ip:127.0.0.1:1"))
	for _, c := range []struct {
		name string
		v    *view.View
	}{
		{"five_nodes_first_four", five},
		{"init_one_node", init},
		{"no_nodes", &view.View{ClusterID: cid, Incarnation: 4}},
		{"nil_cluster_id_node_without_addrs", &view.View{Nodes: []view.Node{{ID: ids[3][:], Weight: 1}}}},
		{"pending_only_nodes_ignored", pendEmpty},
		{"unsorted_order_kept", unsorted},
	} {
		s := worktree.TicketFromView(c.v).Encode()
		b, err := base32.StdEncoding.WithPadding(base32.NoPadding).DecodeString(strings.ToUpper(s[len("dstore1"):]))
		must(err)
		o.TicketFrom = append(o.TicketFrom, ticketVec{Name: c.name, View: hex.EncodeToString(must1(c.v.Encode())), Ticket: s, TCBOR: hex.EncodeToString(b)})
	}
	for _, c := range [][4]uint64{{1, 5, 1, 5}, {1, 5, 1, 6}, {1, 5, 1, 4}, {2, 1, 1, 99}, {1, 99, 2, 1}, {0, 0, 0, 0}, {^uint64(0), 0, ^uint64(0), ^uint64(0)}} {
		v := &view.View{Incarnation: c[0], Epoch: c[1]}
		o.Compare = append(o.Compare, compareVec{ViewInc: c[0], ViewEpoch: c[1], Inc: c[2], Epoch: c[3], Result: v.Compare(c[2], c[3])})
	}
	o.DefaultMinR = map[string]int{}
	for _, r := range []int{0, 1, 2, 3, 4, 5, 7, 10, 255} {
		o.DefaultMinR[fmt.Sprint(r)] = int(view.DefaultMinReplicas(uint8(r)))
	}
	mk := func(ws ...uint32) []view.Node {
		var ns []view.Node
		for i, w := range ws {
			ns = append(ns, view.Node{ID: []byte{byte(i + 1)}, Weight: w})
		}
		return ns
	}
	mj := func(ns []view.Node) []memberJ {
		var o []memberJ
		for _, n := range ns {
			o = append(o, memberJ{ID: hex.EncodeToString(n.ID), Weight: n.Weight})
		}
		return o
	}
	for _, c := range []struct {
		name        string
		cur, target []view.Node
		r           int
		force       bool
	}{
		{"drop_two_r3", mk(1, 1, 1, 1), mk(1, 1), 3, false},
		{"drop_three_r3", mk(1, 1, 1, 1), mk(1), 3, false},
		{"drop_three_r3_forced", mk(1, 1, 1, 1), mk(1), 3, true},
		{"weight_zero_counts_as_dropped", mk(1, 1, 1), mk(0, 0, 0), 3, false},
		{"current_weight_zero_not_counted", mk(0, 0, 0, 1), mk(), 2, false},
		{"replicas_zero", mk(1, 1), mk(), 0, false},
	} {
		e := ""
		if err := view.ValidateChange(c.cur, c.target, c.r, c.force); err != nil {
			e = err.Error()
		}
		o.Validate = append(o.Validate, validateVec{Name: c.name, Cur: mj(c.cur), Target: mj(c.target), Replicas: c.r, Force: c.force, Error: e})
	}
	sn := []view.Node{{ID: []byte{2}}, {ID: []byte{1, 5}}, {ID: []byte{1}}, {ID: nil}, {ID: []byte{0xff}}, {ID: []byte{1, 0}}}
	view.SortNodes(sn)
	for _, n := range sn {
		o.SortNodes = append(o.SortNodes, hex.EncodeToString(n.ID))
	}

	enc := json.NewEncoder(os.Stdout)
	enc.SetIndent("", " ")
	must(enc.Encode(o))
}
```

---

## Addenda (synthesis)

Added by the architecture synthesis. `PORTING.md` is normative where it differs from this spec.

1. **Crate `dstore-view`** holds `placement`, `view`, `ticket_from_view` (re-exported by
   `dstore-worktree`) and `node_short_id`. `NodeId` is the newtype `NodeId(pub [u8; 32])`, defined
   here and re-exported by transport, client and the facade. Signatures: PORTING.md §4.5.
2. **Address parsing** (§4.4 `parse_addrs`, `endpoint_addr`) moved to `dstore_transport::addr`: a
   hand-written parser with Go `netip`/`net/url` rules, not `iroh_base::TransportAddr` parsing. The
   conversion to iroh candidates is `dstore_transport_iroh::to_iroh_addr` (PORTING C20).
3. **Open decisions resolved.**
   1. Every generated view-decode vector asserts its text.
   2. A cluster id shorter than 4 bytes in `cluster status` calls
      `dstore_cli::go_panic_exit("runtime error: slice bounds out of range [:4] with capacity N")`
      and exits 2 (PORTING DD-7). Add a vector with the Go first line.
   3. `--store` ticket derivation per PORTING §2.2 B.
   4. `parse_node_id(&[u8])`.
   5. The lazy `RwLock<HashMap<u32, Arc<[u32]>>>` cache.
   6. Shared helpers live in `dstore-gocompat`, the codec in `dstore-codec`.
4. **Helper signatures.** `ids_of(raw: &[Vec<u8>])` serves wire holder lists; `contains`/`add_id`
   operate on `Vec<Option<Vec<u8>>>`.
5. **Appendix A generator** becomes the `view` family of `tools/vectorgen`, writing
   `tests/golden/view/view_placement.json`.
