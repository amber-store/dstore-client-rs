# Client library, part B: objects and tree transfer

Normative reference: `github.com/amber-store/dstore` tag v0.1.9 (HEAD 368f2c7).
The module-cache copy `dstore@v0.1.9` is byte-identical to the local checkout
for `client/ wire/ codec/ view/ placement/ transport/ worktree/ ticket/` and
`cmd/dstore/client.go` (checked with `diff -rq`). Line numbers below refer to
that tree. Core is `github.com/amber-store/core@v0.0.8`; pack framing is
`github.com/amber-store/transport-iroh@v0.4.0/protocol`.

Every hex string in this document was produced by a throwaway Go 1.26.5
program built against dstore v0.1.9 from the module cache (run under the
scratchpad, then deleted). Where a value is stated as "generated", it is exact.

---

## 1. Scope

### 1.1 Files in this area

| File | Lines | What it does |
|---|---|---|
| `client/objects.go` | 395 | `MissingResult`, `Cluster.Missing` (negotiation per primary, pin, 8192-key chunks, short lists, failover to the next owner), `PutResult`, `RecordSource`, `Cluster.Put` (byte-balanced batches, `Conns` in flight per primary, `Jobs` global), `putBatch` (retry on `stale-view`/`busy`), `putOnce` (one put stream), `PutResult.merge`, `Cluster.Placed` (ack policy), `GetResult`, `Cluster.Get` (streaming fetch with a `missing` accessor), `VerifyRecord`. Constants `defaultBatchBytes`, `batchKeys`. |
| `client/fetch.go` | 309 | The fetcher (dispatcher loop, `Jobs` workers, per-owner accumulators, retry down the read order, one view refresh), `estSize`, `pickBatch`, `getStream` (one `get` stream). Constants `getBatchKeys`, `getBatchBytes`, `getEstMax`. |
| `client/tree.go` | 479 | `PushStats`, `Cluster.Push` (ReachableKeys walk, 3 negotiate/put rounds, direct fill, re-pin, ref-put with up to 2 incomplete renegotiations), `directFill`, `storedSizer`, `mergeIDs`, `shortError`, `PullStats`, `Cluster.Pull`, `Cluster.PullTree` (frontier walk with CheckComplete pruning), `localWriter` (batched `WriteParallel` on its own goroutine), `pullWriteBytes`. |
| `client/batch.go` | 29 | `RecordSizer`, `batches` (in-order split by sizer bytes and key count). |
| `client/progress.go` | 181 | `Progress`, `ProgressReport`, `NodeProgress`, `PutObserver`, `tracker` (push progress), `countKeys`, `Cluster.pathAttrs`, `Rate`, `HumanBytes`. |
| `client/batch_test.go` | 51 | Unit tests of `batches`. |

### 1.2 Files this area depends on (owned by other parts, semantics restated where the transfer depends on them)

| File | Lines | Used for |
|---|---|---|
| `client/client.go` | 403 | `Config` defaults, `stamp`, `call`, `callRetry`, `handleErr` (backoff), `ok`, `penalty`, `probeHinted`, `preferred`, `Primary`, `Owners`, `WriteSet`, `ReadOrder`, `RefreshView`, `anyNode`. |
| `client/rank.go` | 71 | `rankOwners`, `rttClass` (owner preference). |
| `client/refs.go` | 150 | `RefGet`, `RefPut`, `Cond`, `CASMismatch`, `Incomplete`, `ErrUnknownRef` (used by Push and Pull). |
| `wire/wire.go` | 395 | `Msg`, frame types, limits, `WriteMsg`, `ReadMsg`, `Expect`, `Error`, `IsCode`, `AsError`, `Keys32`, `RawKeys`, `SendPackRecords`, `NewPackReader`, `CloseStream`. |
| `transport/transport.go` | 243 | `Pool.Open`, `Pool.Call`, `Pool.Path`, `PathInfo`. |
| `view/view.go` | 469 | `Placement.Owners`, `PendingOwners`, `WriteSet`, `ReadOrder`, `View().MinReplicas`, `View().Replicas`, `IDsOf`, `ShortID`. |
| `placement/placement.go` | 272 | `Table.OwnerIDs` (first R zone-distinct ranks), `Table.RankIDs` (full ranking, weighted then weight-0 by id). |
| transport-iroh `protocol/pack.go` | 141 | `SendPackRecords`, `chunkWriter`, `NewPackReader`. |
| transport-iroh `protocol/protocol.go` | 180 | `protocol.Msg` (TData/TDataEnd frames), `ReadMsg`, `WriteMsg`, `RemoteError`, `ChunkSize = 1<<20`. |

### 1.3 Callers in the CLI and worktree

| File | Lines | Caller |
|---|---|---|
| `cmd/dstore/client.go` | 574 | `dialTicket` (client.Config: only Endpoint, Ticket, Logger, `GCInterval: 4 * time.Hour`), `store push` (233-301), `store pull` (303-342), `clusterGet` (453-468, used by `ls` and `cat`), `recordPayload` (195-201). |
| `cmd/dstore/main.go` | 698 | `catalog restore KEY` fetches the backup object with `cl.Get` (≈ lines 660-680). |
| `cmd/dstore/wc.go` | 501 | `clone` (prints `fr.Stats`, line 168), `fetch` (274), `pull` (289), `push` (332, 344); `withCluster`, `dialConfig`. |
| `cmd/dstore/tui.go` | 374 | `runTransfer`, `runPlain`, `statusLine`, `fraction`, per-node table: the consumers of `ProgressReport`. |
| `worktree/flow.go` | 331 | `Tree.fetch` → `cl.PullTree` (54); `Tree.Push` → `cl.Push` with `Cond{Force}` or `Cond{Versioned, ExpectedVersion}`, CAS-mismatch recovery (241-256). |

### 1.4 Core APIs used (v0.0.8)

| Go | File:lines | Used by |
|---|---|---|
| `fstree.ReachableKeys(root, get)` | `fstree/reachable.go:23-72` | Push |
| `fstree.CheckComplete(root, get, has, jobs)` | `fstree/checkcomplete.go:27-74` | PullTree pruning and final gate |
| `fstree.ChildKeys(k, data)` | `fstree/children.go:12-62` | PullTree |
| `fstree.MissingObjectError` | `fstree/checkcomplete.go:12-18` | error text of the final gate |
| `amberpack.RecHeaderSize` (46), `ParseRecord`, `DecodePayload`, `RawRecord`, `Record` | `amberpack/record.go:16,41-47,100-154`; `pack.go:125-128` | VerifyRecord, PullTree, storedSizer, estSize |
| `amberpack.NewReader(r).Records()` | `amberpack/pack.go:140-195` | getStream |
| `amberpack.Writer.AddRecord`, `Close` | `amberpack/pack.go:92-110` | SendPackRecords |
| `packstore.Store.Get`, `GetRecord`, `Has`, `StoredSize`, `WriteParallel`, `WriteOpts`, `Object` | `packstore/packstore.go:549-707`; `parallel.go:26-106`; `segment.go:41-45` | Push, PullTree, localWriter |
| `reference.Reference{...}.Encode`, `reference.Decode` | `reference/reference.go:46-155` | Push (record), RefGet |
| `key.Key`, `key.Parse`, `Validate`, `Type`, `Length`, `key.New`, `key.Blob`, `key.XattrSet` | `key/key.go`, `key/type.go` | everywhere |

### 1.5 Node-side behaviour the client relies on (read for semantics only)

- `node/data.go:26-44` `handleMissing`: `> wire.MaxKeys` keys → `bad-request "too many keys"`; lacking = keys absent or marked corrupt; present keys are synced once and pinned if `Pin`; on the client ALPN `Short` is filled by `negotiateHolders` (`node/data.go:89-155`), which asks every other owner under `WriteSet` and lists a key only when `len(holders) < len(WriteSet(k))`, holders always starting with the primary itself; owners that could not be asked are simply not listed.
- `node/data.go:201-252` `handleGet`: writes `TAbsent{Keys: absent}` (absent = not held or corrupt), then streams the present records sorted by location as one pack; a record that disappears or is found corrupt while streaming is skipped.
- `node/data.go:287-356` `handlePut`: epoch check (`stale-view` with the view), `no-space`, admission slots, per record verification (`rejected` with reason `"verify: " + err` or `"not-owner"`), duplicates in a batch skipped, `> 64 MiB` → `bad-request "batch over 64 MiB"`; the reply lists `Holders` for every accepted key (the primary first, then each owner whose forward confirmed) and `Failed` entries `{key, node, reason, retry_after = 500 + rand(1500) ms}` for forwards that failed (`node/put.go:106-135`).
- `node/server.go:198-204` `stampReply` sets only `Incarnation` and `Epoch` on replies.

---

## 2. API used client-side

### 2.0 Configuration that drives the transfer

`client/client.go:59-84` (`Dial` defaults):

| Field | Default | Transfer use |
|---|---|---|
| `Conns` | 4 (`<= 0` → 4) | connections per node in the pool AND put batches in flight per primary (`slots`) |
| `Jobs` | 8 (`<= 0` → 8) | global semaphore of `Missing` chunks and put batches; fetcher worker count; `CheckComplete` jobs; `WriteParallel` writers in PullTree |
| `GCInterval` | 4 h (`== 0` → 4 h) | Push re-pins when a round ends more than `GCInterval/2` after the last pin |
| `RequestTimeout` | 2 min (`== 0` → 2 min) | per `call`; put and get streams use `10*RequestTimeout` (20 min) |
| `BatchBytes` | `defaultBatchBytes = 16 << 20` (`<= 0` → default), then `min(BatchBytes, wire.MaxPutBatch)` with `MaxPutBatch = 64 << 20` | put batch byte target |

The CLI (`cmd/dstore/client.go:90`) builds `client.Config{Endpoint, Ticket, Logger, GCInterval: 4 * time.Hour}` only, so every CLI transfer runs with these defaults. The `--jobs` flag of `store push`, `store pull`, `pull`, `push` is **not** passed to the client: it only feeds `ingest.Dir` / `worktree.Scan`. Tests set `RequestTimeout: 20 * time.Second` and sometimes `Jobs = 1` or `BatchBytes = 64 << 10`.

Constants of this area, verbatim:

```go
// objects.go:20-23
const (
	defaultBatchBytes = 16 << 20
	batchKeys         = 8192
)
// fetch.go:17-21
const (
	getBatchKeys  = 2048
	getBatchBytes = 8 << 20
	getEstMax     = 64 << 10
)
// tree.go:391
const pullWriteBytes = 16 << 20
// wire.go:29-35
MaxFrame = 16 << 20; MaxKeys = 8192; MaxPutBatch = 64 << 20; MaxPageBytes = 4 << 20; ChunkSize = protocol.ChunkSize // 1 << 20
```

### 2.1 Cluster helpers the transfer calls (part A owns them; semantics that matter here)

- `stamp(m)` (`client.go:189-197`): if a view is cached, sets `ClusterID`, `Incarnation`, `Epoch` from it. Nothing else.
- `call(ctx, id, m)` (`client.go:266-281`): `ctx` bounded by `RequestTimeout`; `pool.Call(cctx, id, ALPNClient, stamp(m))`. On error: `handleErr(id, err)` and return `(resp, err)` (resp is the TErr frame for remote errors). On success: `ok(id)`; if the reply carries `Epoch > 0 || Incarnation > 0` and the cached view `Compare(resp.Incarnation, resp.Epoch) < 0`, start `RefreshView(context.Background())` in the background.
- `callRetry(ctx, id, m)` (`client.go:284-294`): up to 4 `call`s while the error is `stale-view`; returns the last result.
- `handleErr(id, err)` (`client.go:201-218`): for a remote `*wire.Error`, adopt the carried view (`len(we.View) > 0` and it decodes) and return, **no penalty**. For any other error: `failures[id]++`, `d = 5s << min(failures[id]-1, 4)`, capped at 60 s, `backoff[id] = now + d`. So consecutive failures give 5 s, 10 s, 20 s, 40 s, 60 s, 60 s…
- `ok(id)` (`client.go:220-226`): delete `backoff[id]`, `failures[id]`, `unreach[id]`.
- `penalty(id)` (`client.go:228-239`): `+2` while `now < backoff[id]`, `+1` if `id ∈ unreach`.
- `preferred(ids)` = `rankOwners(ids, penalty, pool.Path(·, ALPNClient))` (`rank.go:34-71`): stable sort by (penalty asc, relayed after direct, `rttClass` asc, input position). A node without a live pooled connection counts as direct with class 0. `rttClass`: `< 5ms` → 0, `< 25ms` → 1, `< 100ms` → 2, else 3.
- `Owners(k)` = `Placement().Owners(k)` (under `nodes` only). `WriteSet(k)` = owners under `nodes` then owners under `pending.nodes` not already listed. `ReadOrder(k)` (`client.go:319-326`): `order = Placement().ReadOrder(k)` (full rank under `nodes`, then pending rank entries not yet listed); `r = View().Replicas`; if `len(order) <= r` return `preferred(order)`, else `preferred(order[:r])` followed by `order[r:]` unchanged.
- `probeHinted(ctx)` (`client.go:245-263`): for every id in `unreach` (snapshot), concurrently `call(ctx with 3 s timeout, id, TView)` and discard the result; waits for all. Called by `Push` and `Pull`, **not** by `PullTree` (so worktree fetch/clone/pull do not probe).
- `RefreshView(ctx)` (`client.go:181-187`): `anyNode(TView)` then `adoptReply`.
- `anyNode(ctx, m)` (`client.go:329-362`): candidates = all `View().Nodes` ids (or boot ids), `preferred` order; per node: `call`, and up to 4 more `call`s while `stale-view`; success → return; remote error → return it (the node answered); transport error → next node. None → last error or `"client: no nodes"`.

### 2.2 `Missing` (`objects.go:39-136`)

```go
func (c *Cluster) Missing(ctx context.Context, keys [][32]byte, pin bool) (*MissingResult, error)
type MissingResult struct {
	Lacking map[view.NodeID][][32]byte   // primary → keys it lacks
	Holders map[[32]byte][]view.NodeID   // key the primary holds → owners holding it
	Failed  map[[32]byte]error           // key whose primary could not be asked → error
}
```

The error return is always `nil` in v0.1.9.

Algorithm:

1. `res` with three empty maps; `tried := map[key]map[NodeID]bool{}`; `pending := keys` (input order, duplicates are NOT removed).
2. Loop `attempt = 0, 1, …` while `len(pending) > 0`:
   1. `byPrimary := {}`. For each `k` in `pending` in order: `p, ok := primaryExcept(k, tried[k])` where `primaryExcept` walks `preferred(Placement().Owners(k))` and returns the first owner not in `tried[k]`. If none: set `res.Failed[k] = errors.New("no owners")` **only if `res.Failed[k]` is nil** (so the last transport error is kept when one exists), and skip the key. Otherwise append `k` to `byPrimary[p]` (per-primary key order = input order).
   2. `pending = askPrimaries(ctx, byPrimary, pin, res, tried)`.
   3. If `len(pending) > 0`: log `Warn "negotiation failed at a primary, asking the next owner" objects=<len(pending)> attempt=<attempt+1>`.
   There is no attempt limit; the loop ends because each retry adds an owner to `tried`.

`askPrimaries` (`objects.go:76-136`), one round:

- `sem` = channel of capacity `Jobs`, shared by every chunk of every primary.
- For each primary `p` (Go map order, i.e. random) and each chunk `ks[i:min(i+8192, len)]` (`batchKeys`): spawn a goroutine which acquires `sem`, then `resp, err := callRetry(ctx, p, &wire.Msg{Type: TMissing, Keys: RawKeys(ks), Pin: pin})`. All chunks of all primaries run concurrently up to `Jobs`.
- Under a mutex:
  - Error: `answered := wire.AsError(err)` succeeded (remote error). For each key in the chunk: if `answered || ctx.Err() != nil` → `res.Failed[k] = err` (final, no retry). Else `tried[k][p] = true`, `res.Failed[k] = err`, and append `k` to the retry list.
  - Success: `lacking, _ := wire.Keys32(resp.Keys)` — **the error is ignored**: one bad-length key makes `lacking` nil, so every key of the chunk is then treated as held. `short` = map from each `resp.Short` entry with `len(Key) == 32` to `view.IDsOf(Holders)` (raw ids copied into 32-byte arrays; shorter ids are zero-padded, longer truncated). For each key of the chunk, in chunk order: `delete(res.Failed, k)`; if lacking → append to `res.Lacking[p]` (so a key that appears twice in the input appears twice here); else `res.Holders[k] = short[k]` if listed, else `res.Holders[k] = c.WriteSet(k)` ("held everywhere").
- Wait for every goroutine; return the retry list (order of completion).

Notes:

- Only the owners under `nodes` are candidates for negotiation; `pending.nodes`-only owners are never asked here.
- The primary's `missing` pins present keys when `pin` (atomic has-and-pin, §9.5) and, on the client ALPN, negotiates the other owners with the same `pin`.
- A key whose primary failed and was retried elsewhere ends with `Lacking`/`Holders` from the owner that answered.

### 2.3 `Put` (`objects.go:138-301`, `batch.go`)

```go
type PutResult struct {
	Holders  map[[32]byte][]view.NodeID
	Failed   map[[32]byte][]wire.KeyFailure
	Rejected map[[32]byte]string
	Errors   map[view.NodeID]error
}
type RecordSource func(k [32]byte) ([]byte, error)
type RecordSizer func(k [32]byte) int
func (c *Cluster) Put(ctx context.Context, byPrimary map[view.NodeID][][32]byte, src RecordSource, size RecordSizer, obs PutObserver) *PutResult
```

**Batching** (`batch.go:12-29`): `batches(keys, size, maxBytes, maxKeys)` walks keys in order with `cur`, `bytes`; for each key `n := size(k)`; if `len(cur) > 0 && (bytes+n > maxBytes || len(cur) >= maxKeys)` the current batch is closed; then `k` is appended and `bytes += n`. A record larger than `maxBytes` becomes a batch of its own; order is preserved; no empty batches. Put uses `batches(ks, size, c.cfg.BatchBytes, batchKeys)`.

**Concurrency** (`objects.go:153-194`):

- `sem` = capacity `Jobs`, global across primaries.
- One goroutine per primary. Inside it: `slots` = capacity `Conns` (4); for each batch in order:
  1. Under the lock, if `res.Errors[p] != nil` → return (remaining batches of this primary are not sent; batches already started continue).
  2. Acquire a slot (blocks while `Conns` batches of `p` are in flight), then acquire `sem`.
  3. Spawn the batch goroutine: `resp, err := c.putBatch(ctx, p, b, src, size, obs)`; release `sem` and the slot; under the lock: error → `res.Errors[p]` set if nil (first error wins); success → `res.merge(resp)`.
  The primary goroutine waits (`pwg.Wait`) for its batches before returning; `Put` waits for all primaries.
- So per primary at most `Conns` = 4 batches (≈ 4 × 16 MiB) are in flight, and at most `Jobs` = 8 batches overall.

**merge** (`objects.go:197-213`), under the lock:

- `Holders`: for each entry with `len(Key) == 32`: `r.Holders[key] = view.IDsOf(Holders)` (replace).
- `Failed`: for each entry with `len(Key) == 32`: append the `wire.KeyFailure` to `r.Failed[key]`.
- `Rejected`: for each entry with `len(Key) == 32`: `r.Rejected[key] = Reason` (replace).
- The reply's `Incarnation/Epoch` are not looked at (putOnce does not go through `call`).

**putBatch** (`objects.go:216-242`), up to 4 attempts:

```
for attempt := 0; attempt < 4; attempt++ {
	resp, err := putOnce(...)
	if err == nil { return resp, nil }
	last = err
	if IsCode(err, "stale-view") {
		Warn "upload retry" node=<ShortID(p)> reason="stale view" attempt=<attempt+1>
		handleErr(p, err)        // adopt the carried view
		continue                 // same primary p, immediately
	}
	if IsCode(err, "busy") {
		wait := time.Second
		if we.RetryAfter > 0 { wait = we.RetryAfter }   // ms from the TErr frame
		Warn "upload retry" node=<ShortID(p)> reason=busy wait=<wait> attempt=<attempt+1>
		time.Sleep(wait)         // NOT cancellable by ctx
		continue
	}
	Warn "upload failed" node=<ShortID(p)> objects=<len(keys)> err=<err>
	return nil, err
}
return nil, last
```

Notes: the retry goes to the same primary even after adopting a view that moved ownership (the node then rejects keys it no longer owns as `not-owner`). `busy` and `stale-view` only arrive as a TErr frame reply to the whole batch; per-key `busy`/`stale-view` from replicas arrive in `Failed` and are not retried here.

**putOnce** (`objects.go:244-301`):

1. `cctx = ctx` with timeout `10 * RequestTimeout`.
2. `total = Σ size(k)` (for logging only).
3. `obs.Start(p)` if set; `flushed := false`; defer `obs.Done(p, flushed)` if set (runs on every exit path, after the stream is closed).
4. `began := time.Now()`.
5. `s, err := pool.Open(cctx, p, ALPNClient)`; error → `handleErr(p, err)` and return err.
6. `defer wire.CloseStream(s)` (Close = FIN of the send side, then `CancelRead(0)`).
7. Log `Info "uploading" node=<ShortID> objects=<len(keys)> bytes=<total> path=… [rtt=…]` (pathAttrs, §3.6).
8. `wire.WriteMsg(s, stamp(&Msg{Type: TPut}))`; error → return err (no handleErr).
9. `wire.SendPackRecords(s, seq)` where `seq` yields, for each key in batch order, `rec, err := src(k)`; **if `err != nil` the key is silently skipped** (`continue`); otherwise yield `rec`, and after the yield returns true call `obs.Sent(p, len(rec))`. Error → return err (no handleErr).
10. `_ = s.CloseWrite()`; `flushed = true`; `obs.Flushed(p)`.
11. Log `Info "batch sent, waiting for the node to store and replicate it" node=<ShortID> objects=<n> bytes=<total>`.
12. `resp, err := wire.Expect(s, TPutResult)`; error → `handleErr(p, err)` and return err.
13. `ok(p)`; `took := time.Since(began)`; log `Info "uploaded" node=<ShortID> objects=<n> bytes=<total> took=<took rounded to ms> rate=<Rate(total, took)>`; return resp.

A TErr frame with any code (`busy`, `stale-view`, `no-space`, `bad-request`, `internal`, `not-owner`…) surfaces as `*wire.Error` from `Expect`; `handleErr` adopts a carried view and does not penalise.

### 2.4 `Placed` (`objects.go:306-329`)

```go
func (c *Cluster) Placed(k [32]byte, holders []view.NodeID) bool
```

`pl := Placement()`, `minR := int(pl.View().MinReplicas)`; `count(owners)` = number of owners that appear in `holders` (each owner counted once, duplicates in `holders` do not inflate). Return false if `count(pl.Owners(k)) < min(minR, len(pl.Owners(k)))`; also false if `po := pl.PendingOwners(k)` is non-nil and `count(po) < min(minR, len(po))`; else true. With no owners (`len == 0`) the requirement is 0 and the key is placed. Holders that are not owners do not count.

### 2.5 `Get`, the fetcher, `getStream`, `VerifyRecord` (`objects.go:331-395`, `fetch.go`)

```go
type GetResult struct { Key [32]byte; Record []byte }
func (c *Cluster) Get(ctx context.Context, keys [][32]byte) (iter.Seq2[GetResult, error], func() [][32]byte)
func VerifyRecord(raw amberpack.RawRecord) ([32]byte, []byte, error)
```

**Get** (`objects.go:340-375`). Nothing happens until the sequence is iterated. Each iteration:

1. `f := newFetcher(ctx)`; `defer f.stop()`.
2. A goroutine feeds keys: skips duplicates (first occurrence wins), `f.add(k)` for each (returns false once the fetch's context is cancelled, then the goroutine returns without `finish`), then `f.finish()`.
3. `for r := range f.results()`: `r.rec == nil` → append `r.key` to `missing` (under a mutex); else `yield(GetResult{Key: r.key, Record: r.rec}, nil)`; a false yield returns at once (the deferred `stop` cancels and waits for every goroutine).
4. After the channel closes: `if err := ctx.Err(); err != nil { yield(GetResult{}, err) }` (the caller's ctx, not the fetcher's).

The second return value reads the accumulated `missing` slice under the mutex. It is not reset between iterations, so iterating twice appends twice. Records arrive in arrival order, not input order. `Record` is the complete record (46-byte header + stored payload, possibly zstd-compressed), as read from the wire.

**fetcher** (`fetch.go:53-175`):

- `newFetcher(ctx)`: `fctx, cancel := context.WithCancel(ctx)`; channels `in` (cap 1024), `retry` (cap 1024), `out` (cap 256), `jobs` (unbuffered), `jobDone` (cap `Jobs`), `done`; starts `Jobs` workers and the dispatcher `run`.
- `add(k)`: `select { in <- k: true; <-fctx.Done(): false }`. `finish()`: `close(in)`. `results()`: `out`. `stop()`: `cancel()` then wait for `done`.
- `run` (dispatcher), with `acc map[NodeID]*fetchAcc{keys []fetchKey; bytes int}`, `inflight`, `inOpen = true`. Deferred on return: `close(jobs)`, wait for the workers, `close(out)`, `close(done)`. Loop:
  1. While `inOpen`, drain `in` without blocking: a closed channel sets `inOpen = false`; each key is `route`d with `attempt = 0`.
  2. Drain `retry` without blocking, `route` each.
  3. While `inflight < Jobs` and `pickBatch(acc)` returns a job: `inflight++`, `select { jobs <- job; <-fctx.Done(): return }`. Partial batches are dispatched as soon as a worker is free ("take everything queued before dispatching, so that batches fill while the workers are busy").
  4. If `!inOpen && inflight == 0 && len(acc) == 0` → return (this closes `out`).
  5. Block on: `in` (only while open), `retry`, `jobDone` (`inflight--`), `fctx.Done()` (return).
- `route(acc, fk)` (`fetch.go:180-203`):
  1. `order := ReadOrder(fk.key)` (recomputed at every routing, with the current penalties).
  2. If `fk.attempt >= len(order) && !f.refreshed`: `f.refreshed = true` (once per fetcher, whichever key gets there first), `_ = RefreshView(f.ctx)` synchronously inside the dispatcher, `order = ReadOrder(k)`, `fk.attempt = 0`.
  3. If `fk.attempt >= len(order)`: send `fetched{key: k}` (nil record: not found) on `out` (or give up on cancel) and return.
  4. `node := order[fk.attempt]`; append to `acc[node]`, `bytes += estSize(k)`.
  Because the order is re-ranked between attempts, a key whose first owner failed (and got penalised to the back) may skip an owner at `attempt = 1` and re-ask the failed one later. This is v0.1.9 behaviour; port it as is.
- `estSize(k) = amberpack.RecHeaderSize + min(int(key.Key(k).Length()), getEstMax)` = `46 + min(length, 65536)`.
- `pickBatch(acc)` (`fetch.go:206-234`): iterate the map (random order); the first accumulator with `len(keys) >= 2048 || bytes >= 8 MiB` is taken immediately; otherwise the one with the most keys (strictly more replaces). Take `n` keys from its front: `n < len && n < 2048 && (n == 0 || bytes+estSize(keys[n]) <= 8<<20)`. The job is `{node, keys[:n]}`; the accumulator keeps the rest, `bytes -= taken`, and is deleted when empty.
- `worker`: for each job, `fetch(job)` then `jobDone <- struct{}{}`.
- `fetch(job)` (`fetch.go:246-271`): `got := map[key]bool`; `_ = getStream(f.ctx, job.node, keys, emit)` where `emit(k, rec)` sets `got[k] = true` and sends `fetched{k, rec}` on `out` (returns false on cancel). The error of `getStream` is ignored. Then every job key not in `got` goes back on `retry` with `attempt + 1` (stop on cancel).

**getStream** (`fetch.go:275-309`):

1. `cctx` = timeout `10 * RequestTimeout`.
2. `s, err := pool.Open(cctx, id, ALPNClient)`; error → `handleErr(id, err)`, return err.
3. `defer wire.CloseStream(s)`.
4. `wire.WriteMsg(s, stamp(&Msg{Type: TGet, Keys: RawKeys(keys)}))`; error → return err.
5. `_ = s.CloseWrite()`.
6. `wire.Expect(s, TAbsent)`; error → `handleErr(id, err)`, return err. The absent key list is **ignored**; absence is inferred from `got`.
7. `pr := wire.NewPackReader(s)`; for each `raw, err` of `amberpack.NewReader(pr).Records()`: error → return err (no `handleErr`, no `ok`: a mid-stream failure does not penalise); `VerifyRecord(raw)` error → `continue` ("a corrupt copy: the next owner is asked"); `emit(k, rec) == false` → return `ctx.Err()`.
8. `_, _ = io.Copy(io.Discard, pr)` (consumes TDataEnd; its error is ignored).
9. `ok(id)`; return nil.

Records are not checked against the requested keys: a valid record for an unrequested key is emitted, and a record sent twice is emitted twice.

**VerifyRecord** (`objects.go:378-395`), in this order:

1. `k := raw.Key`; `k.Validate()` error → return it (e.g. `key: reserved header bit is set`).
2. `payload, err := amberpack.DecodePayload(raw.Flags, raw.Ulen, raw.Bytes[46:])` → error returned.
3. `want, err := key.New(k.Type(), k.Length(), payload)` (cannot fail after Validate).
4. `want != k` → `fmt.Errorf("payload hashes to %s, not %s", want, k)`.
5. Return `k` and a copy of `raw.Bytes`.

Unlike the node's `verifyRecord` (`node/data.go:263-283`), the client does **not** check `Length() == len(payload)` for Blob/XattrSet. Generated: a Blob key with length field 999 over a 100-byte payload is accepted.

**CLI uses of Get**

- `clusterGet(ctx, cl)` (`cmd/dstore/client.go:454-468`), getter for `ls`/`cat`: `seq, missing := cl.Get(ctx, [][32]byte{k})`; on the first item: error → return it; else `return recordPayload(r.Record)` (breaks the iteration). After an empty iteration: `len(missing()) > 0` → `fmt.Errorf("object %s not found", k)`; else `errors.New("no data")`. `recordPayload` = `ParseRecord` + `DecodePayload`.
- `catalog restore KEY` (`cmd/dstore/main.go`): iterates every item, keeping the last payload; `data == nil` → `errors.New("backup object not found in the cluster")`.

### 2.6 `Push` (`tree.go:17-238`)

```go
type PushStats struct { Keys int; Uploaded int; Bytes int64; Version []byte }
func (c *Cluster) Push(ctx context.Context, local *packstore.Store, root key.Key, name, user string, cond Cond, prog Progress) (PushStats, error)
```

Step by step:

1. `keys, err := fstree.ReachableKeys(root, local.Get)`; error → `fmt.Errorf("walk local tree: %w", err)` (e.g. `walk local tree: fstree: reading <64 hex>: packstore: object not found`). Order: root first, then BFS discovery order (deterministic in practice).
2. `all` = keys as `[32]byte`; `st.Keys = len(all)`.
3. `src(k) = local.GetRecord(k)` (verbatim stored record); `size = storedSizer(local)`; `tr := newTracker(c, prog)`; `obs := tr.observer()`.
4. `start := now`; `lastPin := start`; `holders := map[key][]NodeID{}`; `uploaded := 0`; `lastErr := nil`.
5. `c.probeHinted(ctx)`.
6. **Rounds** `round = 0, 1, 2`:
   1. `mr, err := c.Missing(ctx, all, true)` (pin); err → return (never happens).
   2. `holders[k] = h` for every `mr.Holders` entry.
   3. If `mr.Failed` is non-empty: return `fmt.Errorf("negotiate %x at its primary: %w", k[:8], err)` for whichever failed key the map yields first (random). Generated example: `negotiate 00644b75a8842dc1 at its primary: transport: peer recently unreachable`.
   4. `lacking, lackBytes := countKeys(mr.Lacking, size)` (§2.9).
   5. Round 0: `tr.totals(len(all), len(all)-lacking, lackBytes)` and log `Info "negotiated" objects=<len(all)> present=<len(all)-lacking> upload=<lacking> bytes=<lackBytes> primaries=<len(mr.Lacking)>`. Other rounds: `tr.more(lackBytes)` and log `Info "re-sending objects short at their primaries" round=<round+1> objects=<lacking> bytes=<lackBytes>`.
   6. If `len(mr.Lacking) > 0`:
      - `pr := c.Put(ctx, mr.Lacking, src, size, obs)`.
      - For each `k, h` in `pr.Holders`: if `k` was not yet a key of `holders` → `uploaded++`, `n++`; then `holders[k] = h` (replace). `tr.objects(n)`.
      - For each `p, err` in `pr.Errors`: `lastErr = fmt.Errorf("upload to %s: %w", view.ShortID(p), err)`; log `Warn "upload to a primary failed, its objects go to another owner" node=<ShortID> err=<err>`. (The keys stay short; the next negotiation picks another owner because the failed one is penalised, unless the error was remote.)
      - For each `k, reason` in `pr.Rejected`: return `fmt.Errorf("record %x rejected: %s", k[:8], reason)` (first in map order).
   7. Ack policy: `short` = every key of `all` (in order) with `!c.Placed(k, holders[k])`.
   8. `len(short) == 0` → break.
   9. `round == 2` → `err := c.shortError(short, holders)`; if `lastErr != nil` → `fmt.Errorf("%w; last upload error: %v", err, lastErr)`; return it.
   10. `round == 1` → `c.directFill(ctx, short, holders, src, size, tr)` (error returned, never non-nil).
   11. If `time.Since(lastPin) > c.cfg.GCInterval/2`: `_, _ = c.Missing(ctx, all, true)`; `lastPin = now`.
   So the full-set re-pin runs only between rounds that left keys short; a single long `Put` is never interrupted to re-pin, and the architecture's "pinning any owner the reply shows unconfirmed directly" is not implemented.
7. `st.Uploaded = uploaded`; `st.Bytes = tr.bytes()`; log `Info "upload complete" uploaded=<n> bytes=<st.Bytes> took=<took rounded ms> rate=<Rate(st.Bytes, took)>`.
8. Reference record: `reference.Reference{Name: name, Key: root[:], User: user, CreatedAt: time.Now().UnixNano()}`; `enc, err := rec.Encode()` → validation errors are returned as is (e.g. `reference name must not be empty`, `reference user: user must not contain control characters`).
9. **ref-put** `attempt = 0, 1, 2`:
   1. `version, err := c.RefPut(ctx, enc, cond)`. Success → `st.Version = version`; log `Info "reference written" name=<name> version=<fmt.Sprintf("%x", version)>`; return `st, nil`.
   2. If `err` is not `*Incomplete`, or `attempt == 2` → return `st, err` (a `*CASMismatch` surfaces here unchanged).
   3. Log `Warn "reference write incomplete, renegotiating" attempt=<attempt+1> err=<err>`.
   4. `mr, merr := c.Missing(ctx, all, true)`; `merr` → return it.
   5. If `len(mr.Lacking) > 0`: `_, lackBytes := countKeys(mr.Lacking, size)`; `tr.more(lackBytes)`; `c.Put(ctx, mr.Lacking, src, size, obs)` (**result ignored**).
   6. `short` = keys of `all` with `mr.Holders[k]` absent or `!Placed(k, mr.Holders[k])`. Every key that was just uploaded is absent from `mr.Holders` (computed before the put), so it counts as short.
   7. If `short` is non-empty: `c.directFill(ctx, short, mr.Holders, src, size, tr)` (so those keys are sent again to every owner of their write set).
   8. `st.Bytes = tr.bytes()`.
   At most 3 `RefPut` calls and 2 renegotiation rounds. The trailing `errors.New("push: reference write did not complete")` (`tree.go:167`) is unreachable.

`st.Uploaded` is not updated by the incomplete-retry path. `st.Bytes` counts every record byte handed to the wire, including re-sends.

**directFill** (`tree.go:171-195`): `byOwner` = for each short key (in order), each owner in `WriteSet(k)` not in `holders[k]` gets the key appended. Empty → return nil. `_, bytes := countKeys(byOwner, size)`; `tr.more(bytes)`; log `Info "sending short objects to their owners directly" objects=<len(short)> owners=<len(byOwner)> bytes=<bytes>`; `pr := c.Put(ctx, byOwner, src, size, tr.observer())`; for each `k, h` in `pr.Holders`: `holders[k] = mergeIDs(holders[k], h)`; return nil. Errors, failures and rejections of this put are ignored, and `uploaded`/`tr.objects` are not touched. On a non-primary owner the client ALPN `put` still replicates to the key's other owners.

**storedSizer** (`tree.go:199-206`): `n, ok, err := local.StoredSize(k)`; if `err == nil && ok` → `46 + int(n)` (the exact record length); else `int(key.Key(k).Length())` (logical length; for a tree object this is the subtree size).

**mergeIDs(a, b)** (`tree.go:208-218`): concatenation `a ++ b` with duplicates removed, first occurrence kept.

**shortError(short, holders)** (`tree.go:220-238`): `missing[o]++` for each short key and each `WriteSet` owner not in `holders[k]`; `names` = `fmt.Sprintf("%s (%d keys)", view.ShortID(id), n)` in map order (random); return `fmt.Errorf("push: %d keys could not be placed; owners not confirming: %v", len(short), names)`. `%v` of a `[]string` renders `[a b c]`. Generated: `push: 5 keys could not be placed; owners not confirming: [03000000 (5 keys)]` and, with a last error, `push: 5 keys could not be placed; owners not confirming: [03000000 (5 keys)]; last upload error: upload to 02000000: context deadline exceeded`.

### 2.7 Reference calls used by Push and Pull (`refs.go`, owned by part A)

- `Cond{ExpectedVersion []byte; Versioned bool; ExpectedOld []byte; Keyed bool; Force bool}`; `apply(m)`: `m.Force = Force`; `Versioned` → `HasExpected = true`, `ExpectedVersion = ExpectedVersion` (nil means "must not exist"); `Keyed` → `HasExpected = true`, `ExpectedOld = ExpectedOld`.
- `RefPut(ctx, record, cond)`: `anyNode(&Msg{Type: TRefPut, Record: record} + cond)`; error → `refErr` (`unknown-ref` → `ErrUnknownRef`); `TOK` → `resp.Version`; `TCASMismatch` → `&CASMismatch{Current, Record, Version, HasCurrent}`; `TIncomplete` → `&Incomplete{Sample: Keys32(resp.Keys) (error ignored), Shortfall}`; otherwise `fmt.Errorf("client: unexpected reply %d", type)`.
- `CASMismatch.Error()`: `cas mismatch: reference is absent` when `!HasCurrent`, else `cas mismatch: current key %x`. `Incomplete.Error()`: `incomplete: %d keys short`. `ErrUnknownRef`: `client: unknown reference`.
- `RefGet(ctx, name)`: `anyNode(&Msg{Type: TRefGet, Name: name})`; `TRef` → `reference.Decode(resp.Record)` → `&Ref{Name, Record, Version, Ref}`.

Callers of Push:

- `store push PATH NAME` (`cmd/dstore/client.go:233-301`): `c.NArg() != 2` → `errors.New("push PATH NAME")`; `reference.ValidateName(name)`; open the local store (`<local>/packstore` with sync, `<local>/refs` refstore); `ingest.Dir(st, path, ingest.Opts{Jobs: c.Int("jobs")})`; stderr `built %s: %d new objects\n` (`root.String()[:16]`, `stats.Stored`); `cond := Cond{Force: --force}`; if not force: `Versioned = true`, and `--expected-version` hex (via the CLI's `hexDecode`, error `bad hex %q`) as `ExpectedVersion`. `runTransfer(ctx, c, "push "+name, …)` dials and calls `cl.Push(ctx, st, root, name, --user, cond, prog)`. A `*CASMismatch` error becomes `fmt.Errorf("%w (pull first, or --force)", err)`. On success the CLI writes its **own** reference record (a new `CreatedAt`) to the local refstore, ignoring errors, and prints `pushed %s: %d objects, %d uploaded, version %x\n` (`ps.Keys`, `ps.Uploaded`, `ps.Version`).
- `worktree.Tree.Push` (`worktree/flow.go:217-259`): `cond = Cond{Force: force}`; if not force `Versioned = true`, `ExpectedVersion = State.RemoteVersion` (nil: must be new). On `*CASMismatch`: if `HasCurrent`, `key.Parse(Current)` succeeds and equals the pushed root → recovered (`State.RemoteVersion = cm.Version`); otherwise `fmt.Errorf("%w (%v)", ErrRefChanged, err)` with `ErrRefChanged = "reference changed on the cluster since your last fetch: pull first, or --force"`. CLI `push` prints `pushed %s: root %s, %d objects, %d uploaded, version %x\n` (root first 16 hex) or `%s already holds %s (an earlier push completed); state updated\n`.

### 2.8 `Pull`, `PullTree`, `localWriter` (`tree.go:240-479`)

```go
type PullStats struct { Keys int; Fetched int; Bytes int64; Root key.Key; Record []byte; Version []byte }
func (c *Cluster) Pull(ctx context.Context, local *packstore.Store, name string, prog Progress) (PullStats, error)
func (c *Cluster) PullTree(ctx context.Context, local *packstore.Store, root key.Key, st *PullStats, prog Progress) error
```

**Pull** (`tree.go:253-269`): `ref, err := c.RefGet(ctx, name)` → return err (`client: unknown reference` for an absent name); `root, err := key.Parse(ref.Ref.Key)` → return err; `st.Root, st.Record, st.Version = root, ref.Record, ref.Version`; `c.probeHinted(ctx)`; `c.PullTree(ctx, local, root, &st, prog)` → return `st, err`. Pull does **not** write the local reference; the caller does.

**PullTree** (`tree.go:276-387`):

1. `ctx, cancel := context.WithCancel(ctx)`; `defer cancel()`; `f := c.newFetcher(ctx)`; `defer f.stop()`; `w := newLocalWriter(local, c.cfg.Jobs)`; `defer w.stop()`.
2. `seen := map[key.Key]bool{}`; `queue [][32]byte`; `pending := 0`.
3. `want(k)` (recursive, synchronous, runs on the loop's goroutine):
   1. `seen[k]` → return nil; set `seen[k] = true`.
   2. `has, err := local.Has(k)` → return err.
   3. If `has`:
      - Blob or XattrSet → return nil (pruned).
      - `fstree.CheckComplete(k, local.Get, local.Has, c.cfg.Jobs)` succeeds → return nil (the subtree is complete locally). Any error means "incomplete".
      - Else if `local.Get(k)` succeeds and `fstree.ChildKeys(k, data)` succeeds: `want(kid)` for every child in order (returning the first error) and return nil. If either fails, fall through to fetching `k`.
   4. `queue = append(queue, k)`; `pending++`; `st.Keys++`.
4. `want(root)` → return err.
5. While `pending > 0`, `select` over:
   - send `queue[0]` on `f.input()` (only when the queue is non-empty) → `queue = queue[1:]`;
   - `r, ok := <-f.results()`:
     - `!ok` → `ctx.Err()` if set, else `errors.New("pull: fetch ended early")`;
     - `r.rec == nil` → `fmt.Errorf("pull: object %x not found in the cluster", r.key[:8])`;
     - `w.write(packstore.Object{Key: k, Record: r.rec})` → return err;
     - `st.Fetched++`; `st.Bytes += len(r.rec)`; if `prog != nil` → `prog(ProgressReport{Objects: st.Fetched, TotalObjects: st.Keys, Bytes: st.Bytes})` (no `TotalBytes`, no `Nodes`);
     - if `k` is not Blob/XattrSet: `amberpack.ParseRecord(r.rec)` → err; `DecodePayload(flags, ulen, rec[46:])` → err; `ChildKeys(k, data)` → err; `want(kid)` for each child → err;
     - `pending--`;
   - `<-w.failed` → return `w.close()`;
   - `<-ctx.Done()` → return `ctx.Err()`.
6. `f.finish()`.
7. `w.close()` → return err.
8. `fstree.CheckComplete(root, local.Get, local.Has, c.cfg.Jobs)` error → `fmt.Errorf("pull: tree incomplete after fetch: %w", err)`. Generated: `pull: tree incomplete after fetch: fstree: object <64 hex> is missing`.

Properties: `st.Keys` counts only the keys that had to be fetched (it grows while the walk runs), not the whole tree. Records are stored verbatim (`Object.Record`) without `Verify`, because `getStream` verified each. A result for an unrequested key or a duplicate decrements `pending` too; the final completeness gate catches the resulting early exit. `CheckComplete` on every held-but-incomplete interior node can make resuming a mostly complete pull quadratic; that is how v0.1.9 behaves.

**localWriter** (`tree.go:393-479`):

- `newLocalWriter(local, jobs)`: `ch` capacity 1, `failed` and `done` channels; starts `run`.
- `run`: for each `objs` from `ch`: if `w.err != nil` skip; else `local.WriteParallel(seq over objs, packstore.WriteOpts{Writers: jobs})` (BatchSize default 16 MiB, `Verify` false); on error set `w.err`, `close(w.failed)` (once: later batches are skipped). Closes `done` when `ch` is closed.
- `write(o)`: append to `batch`, `bytes += len(o.Record)`; if `bytes < 16 MiB` return nil; else `flush()`.
- `flush()`: empty → nil; else hand the batch over with `select { ch <- objs: nil; <-failed: w.err }` and reset `batch, bytes`.
- `close()`: if already closed return `w.err`; `closed = true`; `err := flush()`; `close(ch)`; wait `done`; return `err` if set, else `w.err`.
- `stop()`: if not closed: `closed = true`, `close(ch)`, wait `done` (unwritten `batch` is dropped).

Writing overlaps fetching: the loop hands a 16 MiB batch to the writer and keeps receiving; with `ch` capacity 1 the loop blocks only when two batches are queued.

**Callers**

- `store pull NAME` (`cmd/dstore/client.go:303-342`): empty name → `errors.New("pull NAME")`; open the local store; `runTransfer(ctx, c, "pull "+name, …)` → `cl.Pull(ctx, st, name, prog)`; then `refs.Put(name, ps.Record)` (error returned); prints `pulled %s: root %s, %d objects fetched (%d bytes)\n` with `ps.Root` as the full 64-hex string.
- `worktree.Tree.fetch` (`worktree/flow.go:37-59`): `RefGet`; `ErrUnknownRef` → state `HasRemote = false` and no error; `key.Parse`; if `State.HasRemote && State.Remote == k` → up to date (no PullTree); else `cl.PullTree(ctx, t.Store, k, &r.Stats, prog)`. CLI outputs: `cloned %s into %s: root %s, %d objects fetched (%d bytes)\n` (`wc.go:168`), `fetched %s: root %s, %d objects fetched (%d bytes)\n` (`wc.go:274`), `%s: up to date (%s)\n`, `%s does not exist on the cluster\n`; root printed as the first 16 hex characters.

### 2.9 Progress accounting (`progress.go`)

```go
type Progress func(ProgressReport)
type ProgressReport struct {
	Objects      int   // objects done, including ones the cluster already held
	TotalObjects int
	Bytes        int64 // bytes sent or received so far
	TotalBytes   int64 // bytes to transfer, 0 until known
	Nodes        []NodeProgress // the nodes the transfer has talked to, ordered by id
}
type NodeProgress struct {
	ID       view.NodeID
	Direct   bool          // current path is direct
	RTT      time.Duration // 0 if unknown
	InFlight int           // batches in progress
	Awaiting int           // of those, fully sent and waiting for the reply
	Bytes    int64
}
type PutObserver struct {
	Start   func(node view.NodeID)
	Sent    func(node view.NodeID, n int)
	Flushed func(node view.NodeID)
	Done    func(node view.NodeID, flushed bool)
}
```

`tracker` (push only):

- Every mutation runs `update(f)`: lock; `f()`; if `prog != nil` → `prog(snapshot())` **while still holding the lock**; unlock. The callback must be cheap and must not call back into the client.
- `observer()`: `Start` → `node(id).InFlight++`; `Sent(n)` → `node(id).Bytes += n`, `rep.Bytes += n`; `Flushed` → `node(id).Awaiting++`; `Done(flushed)` → `InFlight--`, and `Awaiting--` if `flushed`. `node(id)` creates the entry on first use (so `Nodes` lists every primary or owner a put batch was started for, even one whose dial failed).
- `totals(objects, done, bytes)`: `TotalObjects = objects`, `Objects = done`, `TotalBytes = bytes`.
- `more(bytes)`: `TotalBytes += bytes`. `objects(n)`: `Objects += n`. `bytes()`: `rep.Bytes` under the lock.
- `snapshot()`: copy of `rep`; `Nodes` = copies of every entry with `Direct, RTT` overwritten from `pool.Path(id, ALPNClient)` when a live connection exists (else `Direct = false`, `RTT = 0`); sorted by `bytes.Compare(ID)` ascending.
- `countKeys(m, size)` = `(Σ len(keys), Σ size(k))` over every per-node list.

Semantics tested by `TestClusterPushProgress`: `Bytes` never decreases; `TotalObjects == PushStats.Keys` in every report; the last report has `Objects == TotalObjects`, `Bytes == TotalBytes == PushStats.Bytes`, `PushStats.Bytes` equals the sum of `len(GetRecord(k))` over the tree (on a fresh cluster), every node `InFlight == 0 && Awaiting == 0`, `Σ Nodes[i].Bytes == Bytes`, and every node received bytes. A push that needs no upload emits the `totals` report (Objects = Keys, TotalBytes = 0) and nothing else from the tracker.

Pull progress (§2.8) is a bare report per fetched record, emitted on the loop goroutine.

Consumers (`cmd/dstore/tui.go`): `latest.set` stores the report; `runPlain` prints `statusLine(r, rate)` to stderr every 5 s; the TUI (`bubbletea`) redraws every 100 ms: `fraction` (by bytes once `TotalBytes > 0`, else by objects), status line, per-node table `%-10s %-7s %7s %7d %-15s %11s %10s/s` (ShortID, `direct`/`relay`, RTT rounded to ms or `-`, InFlight, `nodeState`, HumanBytes(Bytes), per-node rate) and `nodeState`: `idle` when `InFlight == 0`, `waiting for ack` when `Awaiting == InFlight`, else `sending`. These belong to the CLI/TUI port; this area must supply the fields exactly.

### 2.10 Where the code and the architecture document disagree (the code is normative)

- §11.2 says `Jobs` defaults to GOMAXPROCS; the code defaults to 8.
- §11.2 describes `Put` as returning per-key replica counts; the code returns holder lists, failures, rejections and per-node errors.
- §6.2 step 3 says per-primary batches of 16 MiB with `Conns` in flight; that matches, but the global limit `Jobs` (8) also bounds all batches together.
- §6.2 step 5 speaks of `busy_deadline`; the client only retries a batch 4 times on `busy`.
- §11.1 says "at most R times per key" for stale-view retries; the code uses 4 attempts per call or batch.
- §11.4 re-pinning "at that interval … pinning any owner the reply shows unconfirmed directly": the code re-runs `Missing(all, pin)` only between rounds and pins no owner directly.
- §11.4 "sends what is missing anywhere to an owner that lacks it": the code puts the lacking keys to their primaries and then direct-fills every key not in the (pre-put) holders.
- §6.3 says keys come back re-asked for what came back "absent or failed": the client ignores the absent list and infers absence from the records received.

---

## 3. Byte and text formats

### 3.1 Frames

A frame is a 4-byte big-endian payload length followed by the payload, a CBOR map encoded with fxamacker `cbor.CanonicalEncOptions()` (`dstore/codec/codec.go:15`). Integer map keys come out in numeric ascending order, every `omitempty` field that is zero, nil or empty is left out, integers use the shortest head, and `Pin` is `0xf5`. Decoding uses `cbor.DecOptions{}`: unknown map keys are ignored (generated check: a payload `a2 00 18 33 18 63 01` decodes as a `TPutResult` with no error). Limit: payload ≤ 16 MiB on both write and read.

The `wire.Msg` fields used by this area:

| key | field | CBOR |
|---|---|---|
| 0 | `Type` | uint (33 missing, 34 get, 35 put, 49 missing-reply, 50 absent, 51 put-result, 10 err) |
| 1 | `ClusterID` | bstr (request stamp) |
| 2 | `Incarnation` | uint, omitted when 0 |
| 3 | `Epoch` | uint, omitted when 0 |
| 4 | `Keys` | array of bstr(32): request keys, `lacking` in missing-reply, `absent` in absent |
| 5 | `Pin` | `true`, omitted when false |
| 10 | `Code` | tstr (err) |
| 11 | `Text` | tstr (err) |
| 12 | `View` | bstr (err `stale-view`/`not-owner`) |
| 23 | `Holders` | array of `KeyHolders{0: bstr key, 1: array of bstr ids (omitempty)}` |
| 24 | `Failed` | array of `KeyFailure{0: key, 1: node, 2: reason tstr, 3: retry_after int64 ms (omitempty)}` |
| 25 | `Rejected` | array of `KeyReject{0: key, 1: reason tstr}` |
| 26 | `Short` | array of `KeyHolders` |
| 28 | `RetryAfter` | int64 ms (err `busy`), omitted when 0 |

Generated frames (hex, including the 4-byte length). Fixtures: `cid` = 16 bytes of `0x11`; `k1` = `00644b75a8842dc16ded120ee1f96d229ac409028f4d5d429499a4ad548aae85` = `key.New(Blob, 100, splitmix(1, 100))`; `k2` = `00c8069dce890d2e93a2e70679f56a31d5efbdfb4981e1a7c6c80fe602a93f36` = `key.New(Blob, 200, splitmix(2, 200))`; `idN` = 32 bytes with `[0] = [31] = N`. `splitmix(seed, n)` is core's VECTORS.md stream (state += 0x9E3779B97F4A7C15; z mix; 8 little-endian bytes per step; truncate).

- `missing` `{Type 33, cid, inc 1, epoch 7, [k1, k2], pin}`:
  `00000062a6001821015011111111111111111111111111111111020103070482582000644b75a8842dc16ded120ee1f96d229ac409028f4d5d429499a4ad548aae85582000c8069dce890d2e93a2e70679f56a31d5efbdfb4981e1a7c6c80fe602a93f3605f5`
- `missing` `{Type 33, cid, inc 0, epoch 300, [k1]}` (no pin, no incarnation):
  `0000003ea40018210150111111111111111111111111111111110319012c0481582000644b75a8842dc16ded120ee1f96d229ac409028f4d5d429499a4ad548aae85`
- `get` `{Type 34, cid, inc 1, epoch 7, [k1, k2]}`:
  `00000060a5001822015011111111111111111111111111111111020103070482582000644b75a8842dc16ded120ee1f96d229ac409028f4d5d429499a4ad548aae85582000c8069dce890d2e93a2e70679f56a31d5efbdfb4981e1a7c6c80fe602a93f36`
- `put` `{Type 35, cid, inc 1, epoch 7}`:
  `0000001aa400182301501111111111111111111111111111111102010307`
- `missing-reply` `{Type 49, inc 1, epoch 7, lacking [k2], short [{k1, [id1, id2]}]}`:
  `00000099a5001831020103070481582000c8069dce890d2e93a2e70679f56a31d5efbdfb4981e1a7c6c80fe602a93f36181a81a200582000644b75a8842dc16ded120ee1f96d229ac409028f4d5d429499a4ad548aae8501825820010000000000000000000000000000000000000000000000000000000000000158200200000000000000000000000000000000000000000000000000000000000002`
- `missing-reply` with nothing lacking and nothing short: `00000008a300183102010307`
- `put-result` `{Type 51, inc 1, epoch 7, holders [{k1,[id1,id2,id3]},{k2,[id1]}], failed [{k2,id2,"busy",1234},{k2,id3,"stale-view",0}], rejected [{k1,"not-owner"}]}`:
  `000001b7a6001833020103071782a200582000644b75a8842dc16ded120ee1f96d229ac409028f4d5d429499a4ad548aae850183582001000000000000000000000000000000000000000000000000000000000000015820020000000000000000000000000000000000000000000000000000000000000258200300000000000000000000000000000000000000000000000000000000000003a200582000c8069dce890d2e93a2e70679f56a31d5efbdfb4981e1a7c6c80fe602a93f36018158200100000000000000000000000000000000000000000000000000000000000001181882a400582000c8069dce890d2e93a2e70679f56a31d5efbdfb4981e1a7c6c80fe602a93f360158200200000000000000000000000000000000000000000000000000000000000002026462757379031904d2a300582000c8069dce890d2e93a2e70679f56a31d5efbdfb4981e1a7c6c80fe602a93f360158200300000000000000000000000000000000000000000000000000000000000003026a7374616c652d76696577181981a200582000644b75a8842dc16ded120ee1f96d229ac409028f4d5d429499a4ad548aae8501696e6f742d6f776e6572`
- `absent` `{Type 50, inc 1, epoch 7, [k2]}`: `0000002ca4001832020103070481582000c8069dce890d2e93a2e70679f56a31d5efbdfb4981e1a7c6c80fe602a93f36`
- `absent` with no absent keys: `00000008a300183202010307`
- `err` `busy` `{Type 10, inc 1, epoch 7, code "busy", text "too many streams", retry_after 750}`:
  `00000024a6000a020103070a64627573790b70746f6f206d616e792073747265616d73181c1902ee`
- `err` `stale-view` with a minimal view `{cid, inc 1, epoch 8, version 3, placement_epoch 1, R 3, min 2, voters [{id1, 1}], nodes [{id1, weight 100, writable}]}` (view bytes `aa0050111111111111111111111111111111110101020803030401050306020781a20058200100000000000000000000000000000000000000000000000000000000000001010108000a81a3005820010000000000000000000000000000000000000000000000000000000000000101186407f5`):
  `000000a3a6000a020103080a6a7374616c652d766965770b77726571756573742065706f636820697320626568696e640c5874aa0050111111111111111111111111111111110101020803030401050306020781a20058200100000000000000000000000000000000000000000000000000000000000001010108000a81a3005820010000000000000000000000000000000000000000000000000000000000000101186407f5`

Stream discipline (`wire.go:388-395`, go-iroh `iroh/conn.go:69-90`): the client opens a bidirectional stream per operation and writes first. `CloseWrite` and `Close` both FIN the send side; `CloseStream` = `Close()` then `CancelRead(0)` (STOP_SENDING code 0). `put`: request frame, TData…TDataEnd, FIN, then read `put-result` or `err`. `get`: request frame, FIN, read `absent` or `err`, then TData…TDataEnd.

### 3.2 Pack framing (transport-iroh `protocol/pack.go`)

`SendPackRecords(w, recs)` builds one amberpack stream `"AMBERPK\x03"` ‖ record ‖ … ‖ `0x00` through a `bufio.Writer` over a `chunkWriter`. The chunk writer cuts the byte stream at exact `ChunkSize = 1 << 20` boundaries; each chunk becomes a `protocol.Msg{Type: 7, Data: chunk}` frame (`a2 00 07 08 <bstr>`), a final partial chunk is flushed at the end if non-empty, then a `TDataEnd` frame `00000003a10008`. An error from `recs` aborts without the terminator. The frame bytes are fully deterministic given the records.

`NewPackReader(r)` returns bytes of consecutive TData frames and reports EOF after TDataEnd. A `TErr` frame (decoded with transport-iroh's `protocol.Msg`) becomes `*protocol.RemoteError` (text `remote: <code>: <text>`, always with the second colon); any other type → `protocol: unexpected frame: type %d during pack transfer` (wrapping `protocol: unexpected frame`). amberpack stops at its end marker without reading TDataEnd, so callers drain the reader before reading the next frame.

Generated pack streams:

- No records: `0000000ea200070849414d424552504b030000000003a10008` (one TData of 14 payload bytes: magic + end marker, then TDataEnd).
- Records of `k1` then `k2` (raw records, 146 and 246 bytes): total 419 bytes, frames `[TData len=408 (bstr 401 bytes), TDataEnd len=3]`, prefix `00000198a2000708590191414d424552504b030100644b75`, SHA-256 `9031de2f144e916cbe8fe6f664970e60f398c5c98a525d90a808c40883e9fec2`.
- One raw Blob record of `splitmix(3, 1048521)` (pack exactly 1 MiB): frames `[len=1048585 head a20007085a00100000, len=3 a10008]`, SHA-256 `35dff4ed54e5f98f57aff54cc8631682d85a6450f01782f6f937c6ba36652554`.
- One raw Blob record of `splitmix(3, 1048522)` (pack 1 MiB + 1): frames `[len=1048585, len=6 a2000708 4100, len=3]` (the second TData carries only the end marker), SHA-256 `576a30d47faa6e9d7c8e1243c68c1fca3542a21604cd80ad0e7b9716ea1cc39d`.

### 3.3 Records

`tag 0x01 ‖ key[32] ‖ flags u8 ‖ ulen u32be ‖ slen u32be ‖ crc32c u32be ‖ payload[slen]`; CRC-32C (Castagnoli) over the whole record with the CRC field zeroed; `flags & 1` = zstd; raw records require `ulen == slen`. Generated raw record of `k1`:
`0100644b75a8842dc16ded120ee1f96d229ac409028f4d5d429499a4ad548aae85000000006400000064ce6c2ad6c15c0289ec2d0a9167ec8e65a18debbe5e5532fbeea293f80bc942ee9086c171b9b501d1d854bb7180021590ff0b4dc3a53c36d76cec99e0758527120fbbe785a83d7e35de181749966761748e5c43cb614f560177dc7567fe8bcf144dd4fc9ac05daa4b`

Generated Go zstd record (4096 × `'a'`, key `011000cf657d3fd42311a258afdf3b5261c256983e3deb2bb38980cb0e754db9`): `01011000cf657d3fd42311a258afdf3b5261c256983e3deb2bb38980cb0e754db901000010000000000fca6d649528b52ffd64000f0380006103162c89`. The Rust side must decode it. It will not produce these bytes itself (libzstd differs from klauspost), which is fine because records travel verbatim.

### 3.4 Error texts (verbatim)

From this area:

| Where | Text |
|---|---|
| `objects.go:49` | `no owners` |
| `objects.go:392` | `payload hashes to %s, not %s` (64-hex want, 64-hex key) |
| `tree.go:33` | `walk local tree: %w` |
| `tree.go:60` | `negotiate %x at its primary: %w` (first 8 key bytes) |
| `tree.go:84` | `upload to %s: %w` (ShortID = first 4 id bytes hex) |
| `tree.go:88` | `record %x rejected: %s` |
| `tree.go:104` | `%w; last upload error: %v` |
| `tree.go:167` | `push: reference write did not complete` (unreachable) |
| `tree.go:237` | `push: %d keys could not be placed; owners not confirming: %v` with items `%s (%d keys)` |
| `tree.go:339` | `pull: fetch ended early` |
| `tree.go:342` | `pull: object %x not found in the cluster` |
| `tree.go:384` | `pull: tree incomplete after fetch: %w` |
| `refs.go` | `cas mismatch: reference is absent`, `cas mismatch: current key %x`, `incomplete: %d keys short`, `client: unknown reference`, `client: unexpected reply %d` |
| CLI | `push PATH NAME`, `pull NAME`, `%w (pull first, or --force)`, `object %s not found`, `no data`, `backup object not found in the cluster`, `bad hex %q` |

Surfaced from dependencies: `remote: %s` / `remote: %s: %s` (`wire.Error`); `wire: unexpected frame: type %d, want %d`; `wire: frame of %d bytes exceeds limit %d`; `wire: short frame: %w`; `wire: decode frame: %w`; `wire: encode frame: %w`; `transport: peer recently unreachable`; `transport: closed`; `context canceled`; `context deadline exceeded`; `fstree: reading %s: %w`; `fstree: object %s is missing`; `packstore: object not found`; `packstore: store closed`.

Generated examples: `negotiate 00644b75a8842dc1 at its primary: transport: peer recently unreachable`; `record 00644b75a8842dc1 rejected: not-owner`; `upload to 01000000: remote: busy: x`; `remote: busy`; `pull: object 00644b75a8842dc1 not found in the cluster`; `cas mismatch: current key 00644b75a8842dc16ded120ee1f96d229ac409028f4d5d429499a4ad548aae85 (pull first, or --force)`; `payload hashes to 0064d6691a7ff54a6dd3956e159812f25a272c090d34658e3533e68a80c44f57, not 00644b75a8842dc16ded120ee1f96d229ac409028f4d5d429499a4ad548aae85` (k1's record with its last payload byte flipped and the CRC recomputed).

### 3.5 Log lines

Written through `*slog.Logger`; the CLI uses `slog.NewTextHandler(os.Stderr, level)` in plain mode (`--log-level`, default `info`, env `DSTORE_LOG_LEVEL`) or the TUI's `teaHandler` (message then ` key=value` pairs; the `bytes` attribute, when an int64, is rendered with `HumanBytes`; values containing a space or tab are `%q`-quoted). Complete list for this area, in code order:

| Level | Message | Attributes |
|---|---|---|
| WARN | `negotiation failed at a primary, asking the next owner` | `objects` `attempt` |
| WARN | `upload retry` | `node` `reason="stale view"` `attempt` |
| WARN | `upload retry` | `node` `reason=busy` `wait` (Duration) `attempt` |
| WARN | `upload failed` | `node` `objects` `err` |
| INFO | `uploading` | `node` `objects` `bytes` (int64) + pathAttrs |
| INFO | `batch sent, waiting for the node to store and replicate it` | `node` `objects` `bytes` |
| INFO | `uploaded` | `node` `objects` `bytes` `took` (Duration rounded to ms) `rate` (string) |
| INFO | `negotiated` | `objects` `present` `upload` `bytes` `primaries` |
| INFO | `re-sending objects short at their primaries` | `round` `objects` `bytes` |
| WARN | `upload to a primary failed, its objects go to another owner` | `node` `err` |
| INFO | `upload complete` | `uploaded` `bytes` `took` `rate` |
| INFO | `reference written` | `name` `version` (hex string) |
| WARN | `reference write incomplete, renegotiating` | `attempt` `err` |
| INFO | `sending short objects to their owners directly` | `objects` `owners` `bytes` |

`pathAttrs(id)`: no live connection → `path=none`; else `path=direct|relay rtt=<RTT rounded to ms>` (0 renders `0s`). Fetch and Pull log nothing.

Generated TextHandler lines (time attribute removed):

```
level=INFO msg=uploading node=01000000 objects=2 bytes=392 path=direct rtt=1ms
level=INFO msg=uploading node=01000000 objects=2 bytes=392 path=none
level=INFO msg=uploaded node=01000000 objects=2 bytes=392 took=1.235s rate="682.7 KiB/s"
level=WARN msg="upload retry" node=01000000 reason="stale view" attempt=1
level=WARN msg="upload retry" node=01000000 reason=busy wait=750ms attempt=2
level=WARN msg="upload failed" node=01000000 objects=2 err="remote: no-space: node below its free-space reserve"
level=INFO msg=negotiated objects=10 present=4 upload=6 bytes=12345 primaries=3
level=INFO msg="reference written" name=trees/a version=0a0b
level=WARN msg="reference write incomplete, renegotiating" attempt=1 err="incomplete: 3 keys short"
level=WARN msg="negotiation failed at a primary, asking the next owner" objects=12 attempt=1
level=INFO msg="upload complete" uploaded=6 bytes=12345 took=2s rate="6.0 KiB/s"
```

### 3.6 Number and duration formats

`HumanBytes(n)` (`progress.go:170-181`): `n < 1024` → `%d B` (negative numbers too); else divide by 1024 while the quotient stays ≥ 1024, then `%.1f %ciB` with `"KMGTPE"[exp]`. Generated: `-5 B`, `0 B`, `1 B`, `1023 B`, `1.0 KiB` (1024), `1.0 KiB` (1025), `1.5 KiB` (1536), `10.0 KiB` (10239), `1024.0 KiB` (1048575: the unit is chosen by integer division, the value rounds up), `1.0 MiB`, `1.0 GiB`, `1.0 TiB`, `1.0 PiB`, `1.0 EiB` (1<<60), `8.0 EiB` (MaxInt64).

`Rate(bytes, took)` (`progress.go:162-167`): `took <= 0` → `-`; else `HumanBytes(int64(float64(bytes)/took.Seconds())) + "/s"`. Generated: `Rate(1048576, 1s) = "1.0 MiB/s"`, `Rate(0, 0) = "-"`, `Rate(1000, 3s) = "333 B/s"`, `Rate(5<<30, 2s) = "2.5 GiB/s"`, `Rate(100, -1ns) = "-"`, `Rate(1<<20, 1.5s) = "682.7 KiB/s"`.

Go `time.Duration.String()` after `Round(time.Millisecond)`: 400µs → `0s`, 1ms → `1ms`, 1.234567s → `1.235s`, 61.5s → `1m1.5s`, 2h → `2h0m0s`, 750ms → `750ms`.

---

## 4. Rust design

### 4.1 Modules

| Rust module | Go source | Contents |
|---|---|---|
| `dstore_client::client::batch` | `client/batch.go` | `RecordSizer`, `batches` |
| `dstore_client::client::objects` | `client/objects.go` | `MissingResult`, `PutResult`, `RecordSource`, `Cluster::missing`, `Cluster::put`, `put_batch`, `put_once`, `Cluster::placed`, `GetResult`, `GetStream`, `Cluster::get`, `verify_record`, `DEFAULT_BATCH_BYTES`, `BATCH_KEYS` |
| `dstore_client::client::fetch` | `client/fetch.go` | `Fetcher`, `FetchKey`, `Fetched`, `FetchJob`, `FetchAcc`, `est_size`, `pick_batch`, `Cluster::get_stream`, `GET_BATCH_KEYS`, `GET_BATCH_BYTES`, `GET_EST_MAX` |
| `dstore_client::client::tree` | `client/tree.go` | `PushStats`, `PullStats`, `Cluster::push`, `direct_fill`, `stored_sizer`, `merge_ids`, `short_error`, `Cluster::pull`, `Cluster::pull_tree`, `LocalWriter`, `PULL_WRITE_BYTES` |
| `dstore_client::client::progress` | `client/progress.go` | `Progress`, `ProgressReport`, `NodeProgress`, `PutObserver`, `Tracker`, `count_keys`, `Cluster::path_attrs`, `rate`, `human_bytes` |
| `dstore_client::wire::pack` | transport-iroh `protocol/pack.go` + the TData/TDataEnd/TErr subset of `protocol.go` | `PackSender`, `PackReader`, `AsyncRecords`, `RemoteError` |

The `Cluster` handle, `Ctx`, `wire::Msg`, `wire::Error`, `transport::Pool` and the ref calls come from part A and the wire/transport parts. This area needs these crate-internal items from them:

```rust
impl Cluster {
    pub(crate) fn cfg(&self) -> &Config;                  // conns, jobs, gc_interval, request_timeout, batch_bytes
    pub(crate) fn log(&self) -> &Logger;                  // slog-compatible logger, §3.5
    pub(crate) fn pool(&self) -> &transport::Pool;
    pub(crate) fn stamp(&self, m: wire::Msg) -> wire::Msg;
    pub(crate) async fn call(&self, ctx: &Ctx, id: NodeId, m: wire::Msg) -> Result<wire::Msg, ClientError>;
    pub(crate) async fn call_retry(&self, ctx: &Ctx, id: NodeId, m: wire::Msg) -> Result<wire::Msg, ClientError>;
    pub(crate) fn handle_err(&self, id: NodeId, err: &ClientError);
    pub(crate) fn ok(&self, id: NodeId);
    pub(crate) fn preferred(&self, ids: &[NodeId]) -> Vec<NodeId>;
    pub fn placement(&self) -> Arc<view::Placement>;
    pub fn write_set(&self, k: &[u8; 32]) -> Vec<NodeId>;
    pub fn read_order(&self, k: &[u8; 32]) -> Vec<NodeId>;
    pub async fn refresh_view(&self, ctx: &Ctx) -> Result<(), ClientError>;
    pub(crate) async fn probe_hinted(&self, ctx: &Ctx);
    pub async fn ref_get(&self, ctx: &Ctx, name: &str) -> Result<Ref, ClientError>;
    pub async fn ref_put(&self, ctx: &Ctx, record: &[u8], cond: &Cond) -> Result<Vec<u8>, ClientError>;
}
impl transport::Pool {
    pub async fn open(&self, ctx: &Ctx, id: NodeId, alpn: &str) -> Result<Stream, ClientError>; // Stream = (SendStream, RecvStream) with close_write / close_stream
    pub fn path(&self, id: NodeId, alpn: &str) -> Option<PathInfo>;                              // PathInfo { direct: bool, rtt: Duration }
}
```

`Ctx` stands in for `context.Context`: a `tokio_util::sync::CancellationToken` plus an optional deadline, with `child()`, `with_timeout(d)`, `done()` (a future) and `err() -> Option<CtxError>`, where `CtxError` displays `context canceled` or `context deadline exceeded`. It is needed because the Go code branches on `ctx.Err()` (Missing gives up retrying, Get yields the error, PullTree returns it) and because CLI error text includes these strings.

`ClientError` (part A) must support `is_code(code)`, `as_remote() -> Option<&wire::Error>`, `as_cas_mismatch()`, `as_incomplete()` (Go `errors.As`) and `Display` exactly as in §3.4. It must be `Clone` or be wrapped in `Arc`, because `MissingResult.failed` stores one error per key.

### 4.2 Types and signatures

```rust
pub type NodeId = [u8; 32];

pub const DEFAULT_BATCH_BYTES: usize = 16 << 20;
pub const BATCH_KEYS: usize = 8192;
pub const GET_BATCH_KEYS: usize = 2048;
pub const GET_BATCH_BYTES: usize = 8 << 20;
pub const GET_EST_MAX: u64 = 64 << 10;
pub const PULL_WRITE_BYTES: usize = 16 << 20;

pub type RecordSizer = Arc<dyn Fn(&[u8; 32]) -> usize + Send + Sync>;
pub type RecordSource = Arc<dyn Fn(&[u8; 32]) -> Result<Vec<u8>, ClientError> + Send + Sync>;
pub fn batches(keys: &[[u8; 32]], size: &dyn Fn(&[u8; 32]) -> usize, max_bytes: usize, max_keys: usize) -> Vec<Vec<[u8; 32]>>;

pub struct MissingResult {
    pub lacking: HashMap<NodeId, Vec<[u8; 32]>>,
    pub holders: HashMap<[u8; 32], Vec<NodeId>>,
    pub failed: HashMap<[u8; 32], Arc<ClientError>>,
}
pub struct PutResult {
    pub holders: HashMap<[u8; 32], Vec<NodeId>>,
    pub failed: HashMap<[u8; 32], Vec<wire::KeyFailure>>,
    pub rejected: HashMap<[u8; 32], String>,
    pub errors: HashMap<NodeId, Arc<ClientError>>,
}
pub struct GetResult { pub key: [u8; 32], pub record: Vec<u8> }

impl Cluster {
    pub async fn missing(&self, ctx: &Ctx, keys: &[[u8; 32]], pin: bool) -> Result<MissingResult, ClientError>;
    pub async fn put(&self, ctx: &Ctx, by_primary: HashMap<NodeId, Vec<[u8; 32]>>,
                     src: RecordSource, size: RecordSizer, obs: PutObserver) -> PutResult;
    pub fn placed(&self, k: &[u8; 32], holders: &[NodeId]) -> bool;
    /// Lazily starts a fetch when first polled; dropping the stream cancels it.
    pub fn get(&self, ctx: &Ctx, keys: Vec<[u8; 32]>) -> GetStream;
    pub async fn push(&self, ctx: &Ctx, local: Arc<packstore::Store>, root: key::Key, name: &str, user: &str,
                      cond: Cond, prog: Option<Progress>) -> Result<PushStats, ClientError>;
    pub async fn pull(&self, ctx: &Ctx, local: Arc<packstore::Store>, name: &str,
                      prog: Option<Progress>) -> Result<PullStats, ClientError>;
    pub async fn pull_tree(&self, ctx: &Ctx, local: Arc<packstore::Store>, root: key::Key,
                           st: &mut PullStats, prog: Option<Progress>) -> Result<(), ClientError>;
}
pub struct GetStream { /* impl futures::Stream<Item = Result<GetResult, ClientError>> */ }
impl GetStream { pub fn missing(&self) -> Vec<[u8; 32]>; }
pub fn verify_record(raw: &amberpack::RawRecord) -> Result<([u8; 32], Vec<u8>), ClientError>;

pub struct PushStats { pub keys: usize, pub uploaded: usize, pub bytes: i64, pub version: Vec<u8> }
pub struct PullStats { pub keys: usize, pub fetched: usize, pub bytes: i64, pub root: key::Key, pub record: Vec<u8>, pub version: Vec<u8> }

pub type Progress = Arc<dyn Fn(&ProgressReport) + Send + Sync>;
#[derive(Clone, Default)] pub struct ProgressReport { pub objects: usize, pub total_objects: usize, pub bytes: i64, pub total_bytes: i64, pub nodes: Vec<NodeProgress> }
#[derive(Clone)] pub struct NodeProgress { pub id: NodeId, pub direct: bool, pub rtt: Duration, pub in_flight: i64, pub awaiting: i64, pub bytes: i64 }
#[derive(Clone, Default)] pub struct PutObserver {
    pub start: Option<Arc<dyn Fn(NodeId) + Send + Sync>>,
    pub sent: Option<Arc<dyn Fn(NodeId, usize) + Send + Sync>>,
    pub flushed: Option<Arc<dyn Fn(NodeId) + Send + Sync>>,
    pub done: Option<Arc<dyn Fn(NodeId, bool) + Send + Sync>>,
}
pub fn human_bytes(n: i64) -> String;
pub fn rate(bytes: i64, took: Duration) -> String;   // Go passes a signed Duration; the Rust caller never has a negative one
```

Go returns partial `PushStats`/`PullStats` together with an error; every caller in the tree ignores them on error, so `Result` is enough. `human_bytes` must use Go's `%.1f` rounding (round half to even on the binary value, which Rust's `{:.1}` also does for f64). Check the §3.6 vectors, including `1024.0 KiB`.

### 4.3 Concurrency mapping

| Go | Rust |
|---|---|
| goroutine + `sync.WaitGroup` | `tokio::task::JoinSet` (join all before returning) |
| `sem := make(chan struct{}, Jobs)` | `Arc<tokio::sync::Semaphore>` with `acquire_owned()` |
| per-primary `slots` (`Conns`) | one `Semaphore::new(conns)` per primary task; acquire the slot, then the global permit, in that order |
| mutex-protected result maps | `std::sync::Mutex`, never held across `.await` |
| `context.WithTimeout(ctx, 10*RequestTimeout)` | `ctx.with_timeout(10 * request_timeout)`; when it fires the error is `context deadline exceeded` |
| `time.Sleep(wait)` in `putBatch` | `tokio::time::sleep(wait).await` (Go ignores ctx here; the push future being dropped cancels it anyway) |
| fetcher `in` (1024), `retry` (1024), `out` (256), `jobDone` (`Jobs`) | `tokio::sync::mpsc::channel` with the same capacities |
| fetcher `jobs` (unbuffered, many receivers) | `async_channel::bounded(1)`; the `inflight < Jobs` guard means a free worker exists, so the extra slot does not change dispatch |
| `select { … default: }` drains | `try_recv` loops |
| `select` over several channels | `tokio::select!` (unbiased) |
| `f.stop()` (cancel, wait for `done`) | `token.cancel()` then await the dispatcher `JoinHandle`, which awaits the workers. `GetStream::drop` cannot await: cancel the token and let the tasks finish (none holds a stream open after cancellation, so `Cluster::close` does not hang, cf. `TestClusterGetStopsEarlyCleanly`) |
| `localWriter.ch` (cap 1), `failed` (close once) | `mpsc::channel::<Vec<packstore::Object>>(1)`; a `CancellationToken` as the one-shot `failed` signal; `Arc<Mutex<Option<ClientError>>>` for `err` |
| tracker lock + callback under the lock | `std::sync::Mutex<TrackerState>`; call `prog(&snapshot)` while holding the guard |

Blocking core-rs calls run off the async threads: `fstree::reachable_keys`, `fstree::check_complete` and `packstore::Store::write_parallel` go through `tokio::task::spawn_blocking` with an `Arc<packstore::Store>`. `get_record`, `has`, `stored_size` and `get` are index lookups or a single mmap/pread and can be called inline on a multi-thread runtime. `want` in `pull_tree` calls `check_complete` for every held interior node; run each call in `spawn_blocking` and await it, keeping the walk sequential like Go.

### 4.4 Pack I/O over iroh streams

core-rs `amberpack::Writer` and `Reader` are synchronous (`std::io::Write`/`Read`), so the async side is re-implemented in `wire::pack` on top of core-rs's record functions:

```rust
pub struct PackSender<'a, W: AsyncWrite + Unpin> { w: &'a mut W, buf: Vec<u8>, wrote_magic: bool }
impl<'a, W: AsyncWrite + Unpin> PackSender<'a, W> {
    pub fn new(w: &'a mut W) -> Self;
    /// Appends a pre-encoded record; writes the magic first; emits TData frames of exactly 1 MiB.
    pub async fn add_record(&mut self, rec: &[u8]) -> io::Result<()>;
    /// Magic if nothing was added, end marker 0x00, final partial TData, then TDataEnd.
    pub async fn finish(self) -> io::Result<()>;
}
pub struct PackReader<R: AsyncRead + Unpin> { r: R, cur: Bytes, done: bool, err: Option<PackError> }
impl<R: AsyncRead + Unpin> AsyncRead for PackReader<R> { /* TData → bytes, TDataEnd → EOF, TErr → RemoteError, other → protocol error */ }
impl<R: AsyncRead + Unpin> PackReader<R> { pub async fn drain(&mut self) -> io::Result<u64>; }
pub struct AsyncRecords<R> { /* BufReader over PackReader; state Magic | Records | Done */ }
impl<R: AsyncRead + Unpin> AsyncRecords<R> {
    /// Mirrors amberpack.Reader.Records: magic, tag, 45 header bytes, slen ≤ MAX_PAYLOAD, payload,
    /// core-rs parse_record; exactly one error, then None.
    pub async fn next(&mut self) -> Option<Result<amberpack::RawRecord, amberpack::Error>>;
    pub fn into_inner(self) -> PackReader<R>;   // must hand back buffered bytes, or drain through the BufReader
}
```

The TData frame is transport-iroh's `protocol.Msg` (`{0: 7, 8: bstr}`), encoded canonically like `wire.Msg`. It is short enough to encode by hand: `a2 00 07 08` + bstr head (`0x40+n` for n < 24, `0x58 n`, `0x59 nn`, `0x5a nnnn`) + data. `TDataEnd` is `a1 00 08`. On receive, decode `protocol.Msg` leniently (unknown keys ignored), since a Go node sends a `TErr` as a `protocol.Msg`-shaped frame: `wire.WriteErr` writes a `wire.Msg`, but code/text sit at keys 10/11 in both types.

`AsyncRecords` wraps the `PackReader` in a `BufReader`, so draining must go through the same buffered reader, otherwise the TDataEnd frame may already sit in its buffer.

Stream handling on Rust iroh 1.0.3 (noq 1.1.1):

- `pool.Open` → `Connection::open_bi().await` (`iroh-1.0.3/src/endpoint/connection.rs:885`), giving `(SendStream, RecvStream)`, which implement `tokio::io::AsyncWrite`/`AsyncRead` (`noq-1.1.1/src/send_stream.rs:332`, `recv_stream.rs:594`).
- `CloseWrite()` → `SendStream::finish()` (`send_stream.rs:188`); error ignored like Go's `_ =`.
- `CloseStream(s)` → `finish()` then `RecvStream::stop(0u32.into())` (`recv_stream.rs:275`), errors ignored. Dropping both halves does the same (`send_stream.rs:350-374` finishes; `recv_stream.rs:605-630` stops with 0 unless all data was read).
- Write request frames with `write_all` and do not flush per record: noq sends as flow control allows.

### 4.5 Core API mapping (core-rs rev a85ffa1, crate `amber-store-core` 0.3.0)

| Go (core v0.0.8) | core-rs | Source checked | Notes |
|---|---|---|---|
| `key.Key` (`[32]byte`) | `key::Key(pub [u8; 32])` | `src/key.rs:101` | `Copy + Ord + Hash`; `Display` lowercase hex |
| `key.Parse(b)` | `Key::parse(&[u8])` | `src/key.rs:143` | error texts identical, e.g. `key: data is not 32 bytes: got N` |
| `k.Validate()` | `Key::validate()` | `src/key.rs:158` | |
| `k.Type()` | `Key::type_()` | `src/key.rs:180` | **panics** on a reserved type nibble: call `validate` first (`verify_record` does); keys from `parse_record`, `Key::parse` and `child_keys` are already canonical |
| `k.Length()` | `Key::length()` | `src/key.rs:194` | used by `est_size` and `stored_sizer` without validation, which is safe |
| `key.New(t, n, payload)` | `Key::new(t, n, &payload)` | `src/key.rs:117` | infallible |
| `key.Blob`, `key.XattrSet` | `key::Type::Blob`, `key::Type::XattrSet` | `src/key.rs:20-31` | |
| `amberpack.RecHeaderSize` | `amberpack::REC_HEADER_SIZE` | `src/amberpack.rs:38` | 46 |
| `amberpack.MaxPayload` | `amberpack::MAX_PAYLOAD` | `src/amberpack.rs:45` | |
| `amberpack.ParseRecord` | `amberpack::parse_record` | `src/amberpack.rs:157` | same validation order |
| `amberpack.DecodePayload` | `amberpack::decode_payload(flags, ulen, stored)` | `src/amberpack.rs:205` | libzstd decodes Go's klauspost frames (PORTING.md) |
| `amberpack.RawRecord{Record, Bytes}` | `amberpack::RawRecord{record, bytes}` | `src/amberpack.rs:289` | Go `raw.Key` is `raw.record.key` |
| `amberpack.NewReader(r).Records()` | `amberpack::Reader::new(r).records()` | `src/amberpack.rs:309-470` | synchronous: use as the reference for `AsyncRecords` |
| `amberpack.Writer.AddRecord/Close` | `amberpack::Writer::add_record/finish` | `src/amberpack.rs:229-282` | synchronous: `PackSender` re-implements it |
| `fstree.ReachableKeys(root, get)` | `fstree::reachable_keys(root, get)` with `G: Fn(Key) -> Result<Vec<u8>, E> + Sync` | `src/fstree/read.rs:694` | same BFS order; `WalkError::Read` displays `fstree: reading <key>: <err>` |
| `fstree.CheckComplete(root, get, has, jobs)` | `fstree::check_complete(root, get, has, jobs)` → `Result<Vec<Key>, WalkError<E>>` | `src/fstree/read.rs:742` | `jobs == 0` means available parallelism (Go: `<= 0`); pass `cfg.jobs` |
| `fstree.ChildKeys(k, data)` | `fstree::child_keys(k, &data)` → `ChildKeysError` | `src/fstree/read.rs:368` | |
| `*fstree.MissingObjectError` | `fstree::MissingObjectError{key}` via `WalkError::Missing` | `src/fstree/read.rs:126-137` | `fstree: object <key> is missing` |
| `packstore.Store.Get` | `packstore::Store::get(k)` | `src/packstore/mod.rs:772` | `packstore: object not found` |
| `packstore.Store.GetRecord` | `packstore::Store::get_record(k)` | `src/packstore/mod.rs:807` | |
| `packstore.Store.Has` | `packstore::Store::has(k)` | `src/packstore/mod.rs:914` | |
| `packstore.Store.StoredSize` → `(n, ok, err)` | `packstore::Store::stored_size(k)` → `Result<Option<u64>, Error>` | `src/packstore/mod.rs:832` | |
| `packstore.Store.WriteParallel(seq, WriteOpts{Writers: jobs})` | `packstore::Store::write_parallel(seq, WriteOpts{writers: jobs, batch_size: 0, verify: false})` → `(WriteStats, Result<(), Error>)` | `src/packstore/parallel.rs:109` | seq items `Result<Object, std::convert::Infallible>`; `I::IntoIter: Send` |
| `packstore.Object{Key, Record}` | `packstore::Object{key, data: Vec::new(), record: Some(rec)}` | `src/packstore/mod.rs:73`, `src/packstore/prepare.rs` | a non-empty `data` next to a record is rejected, like Go |
| `packstore.Open(dir, WithSync(true))` | `packstore::Store::open_with(dir, Options::new().sync(true))` | `src/packstore/mod.rs:364` | |
| `reference.Reference{…}.Encode()` | `reference::Reference{name, key, user, created_at, signature, public_key}.encode()` | `src/reference.rs:317-392` | same validation texts |
| `reference.Decode` | `reference::Reference::decode` | `src/reference.rs:394` | |
| `refstore.Open(<local>/refs, true)` (Pebble) | `refstore` (redb) | `src/refstore.rs`, PORTING.md | not on-disk compatible, §7 |

### 4.6 Crate dependencies (all present offline)

| Crate | Version | Use |
|---|---|---|
| `amber-store-core` | 0.3.0, git rev `a85ffa1` (or path `../core-rs`) | §4.5 |
| `iroh` | 1.0.3 (the version verified wire-compatible with go-iroh v0.2.0) | streams, paths |
| `tokio` | 1.53.1 (`rt-multi-thread`, `macros`, `sync`, `time`, `io-util`) | runtime |
| `tokio-util` | 0.7.19 | `CancellationToken` |
| `futures` | 0.3.34 | `Stream`, `FuturesUnordered` |
| `async-channel` | 2.5.0 | MPMC fetcher job channel |
| `bytes` | 1.12.1 | pack chunks |
| `thiserror` | 2.0.20 | errors |
| `hex` | 0.4.3 | `%x` formatting |
| `pin-project-lite` | 0.2.17 | `PackReader`, `GetStream` |

---

## 5. Golden vectors

### 5.1 Generator

Add `tools/xfervectors/` (its own `go.mod`: `require github.com/amber-store/dstore v0.1.9`, Go 1.26.5, `GOFLAGS=-mod=mod`) writing JSON under `tests/golden/transfer/`. Payloads use core's splitmix64 stream (`VECTORS.md`), so the Rust tests can rebuild inputs. Records are built from incompressible splitmix data wherever the Rust side has to *produce* identical bytes (raw records are byte-identical in both implementations). Go-zstd records appear only as decode inputs. The generator can run offline the way the §3 values were produced: `GOPROXY=file:///Users/dragan/go/pkg/mod/cache/download GOSUMDB=off GOMODCACHE=<scratch> GOCACHE=<scratch> GOTOOLCHAIN=local` with the cached `golang.org/toolchain@v0.0.1-go1.26.5.darwin-arm64`. Rust tests must fail, not skip, when a vector file is missing.

### 5.2 Cases

1. **`frames.json`**: encode/decode vectors, `{name, msg fields, hex}`.
   - Every frame of §3.1 (the Rust encoder must match request frames byte for byte; replies are decoded and compared field by field).
   - `missing` with exactly 8192 keys (frame length and SHA-256; the array head is `99 2000`).
   - `missing-reply` whose `Keys` holds a 31-byte key: the client treats every key of the chunk as held (Keys32 error ignored).
   - `missing-reply` / `put-result` entries whose `Key` is not 32 bytes (skipped) and whose holder ids are 16 or 40 bytes (`IDsOf`: zero-padded or truncated).
   - `put-result` with only `Holders`, with `Failed.retry_after` omitted, and with an unknown map key (ignored).
   - `err` frames: `stale-view` and `not-owner` with a view, `busy` with and without `retry_after`, `no-space`, `bad-request "batch over 64 MiB"`, `bad-request "too many keys"`; `wire.Error.Error()` text for each.
2. **`packs.json`**: `{records: [splitmix seed, len], frame lengths, sha256}`.
   - The four §3.2 streams.
   - A pack of exactly 2 MiB.
   - 10 000 records of 100 bytes (many records per TData, a record split across frames).
   - A source error after 2 records (no terminator, the error is returned).
   - Reader inputs: TErr in the middle of a pack (`remote: internal: sender died`); a frame of type 51 inside a pack (`protocol: unexpected frame: type 51 during pack transfer`); EOF before TDataEnd; bad magic; a record with `slen > MAX_PAYLOAD`.
3. **`verify_record.json`**:
   - Raw record: ok.
   - Go zstd record (§3.3): ok, and `record` equals the input bytes.
   - Blob key with length field 999 over 100 bytes: accepted.
   - Payload byte flipped with the CRC recomputed: `payload hashes to …, not …`.
   - `RawRecord.Key` with the reserved bit: `key: reserved header bit is set`.
   - Type nibble 5: `key: reserved object type: 5`.
   - Non-canonical length: `key: non-canonical length encoding`.
4. **`batches.json`**: `batches` is unexported, so the generator carries a copy of `batch.go:12-29` and asserts it against the four `client/batch_test.go` cases first. Cases:
   - The four test cases.
   - `bytes + n == maxBytes` (the key stays in the batch).
   - 8193 keys of size 1 at the 8192 key cap → `[8192, 1]`.
   - 16 MiB target with records of 46 + 1 MiB → 15 per batch.
   - A first record larger than the target → its own batch.
   - Empty input → no batches.
5. **`est_size.json` / `pick_batch.json`** (copies of `fetch.go:23-25,206-234`):
   - `est_size` for key lengths 0, 100, 65535, 65536, 65537, 1<<40 → 46, 146, 65581, 65582, 65582, 65582.
   - One accumulator of 5000 keys with est 46 → jobs `[2048, 2048, 904]`.
   - 200 keys with est 65582 → a first job of 127 keys.
   - Selection: a full accumulator wins over a larger non-full one. When several are full, or several tie for the most keys, Go's pick is map-order random: vectors must contain a single candidate.
6. **`human_bytes.json`, `rate.json`, `durations.json`**: §3.6 plus random int64 values (`HumanBytes` is exported; call it directly).
7. **`errors.json`**: every §3.4 format with fixed arguments, e.g. `shortError` with one owner, `CASMismatch` with and without a current key, `Incomplete`, `negotiate…`, `record … rejected`, `upload to …`, `pull: …`, `walk local tree: fstree: reading <key>: packstore: object not found`.
8. **`placement_decisions.json`**, through the exported API: the generator stands up `transport.NewNetwork()`, binds a fake node that answers `view` with a crafted view (5 nodes, R = 3, min_replicas = 2, and a second view with a `pending` of 6 nodes and one zone shared by two nodes), dials `client.Dial` with a ticket naming it, and records for 64 splitmix keys:
   - `Owners`, `WriteSet`, and `ReadOrder` (no pooled connections to the other nodes, so preference = rank order).
   - `Placed(k, holders)` for holder sets: all owners; `min_replicas − 1` owners; non-owners only; duplicated owners; pending-only owners; empty.
9. **`transcripts/*.json`**: conversations with scripted fake nodes on the in-memory network. Record every request frame per node (masking `created_at` inside ref-put records) and the scripted replies; the Rust test replays the replies through an in-memory transport and asserts the same requests. Keep to scenarios whose request order does not depend on Go map order (one primary, or per-node multisets).
   - `missing_8193`: R = 1, one node, 8193 keys with pin → two `missing` frames of 8192 and 1 keys.
   - `missing_failover`: owner A's missing fails at the transport level, B answers → A is penalised and the keys are asked at B; a remote error from A instead fails the keys with no retry.
   - `put_split`: `BatchBytes = 64 KiB`, 30 records of 20 KiB → batches of 3, each a `put` + TData… + TDataEnd, with at most `Conns` streams open at once.
   - `put_busy`: first reply `err busy retry_after=50` → second stream after ≥ 50 ms; 4 × busy → `remote: busy: <text>`.
   - `put_stale`: `err stale-view` carrying epoch 9 → the retry is stamped with epoch 9.
   - `get_absent_corrupt`: owner 1 answers absent for k1 and a tampered record for k2 → both keys asked at owner 2; every owner lacks k3 → one `view` refresh, the read order walked again, then missing.
   - `push_happy`: 1 node, R = 1, tree of 3 files → `missing{pin}`, `put`, `ref-put` with `HasExpected` and `ExpectedVersion`.
   - `push_incomplete`: ref-put answered `incomplete` twice, then `ok` → 3 ref-put frames, each preceded by `missing{pin}`.
   - `push_cas`: ref-put answered `cas-mismatch` → the `*CASMismatch` error with its text.
   - `pull_prune`: the local store holds one complete subtree → no `get` frame contains its keys.

### 5.3 Interoperability runs (not byte vectors)

- A Go v0.1.9 three-node cluster (`dstore cluster init` / `node join` / `serve --loopback`). Rust client push → Go client `store pull` into a fresh store; compare `Get` bytes of every reachable key. Then the reverse.
- A shared working copy: `dstore push` by Rust, then `dstore pull` and `status` by Go in the same directory, and vice versa (the packstore under `.dstore/` is shared; see §7 for refs).
- Progress invariants of `TestClusterPushProgress` asserted on the Rust client against the Go cluster.

---

## 6. Go tests worth porting

| Test | File | What it pins |
|---|---|---|
| `TestBatchesBalancesBySizerNotByKeyLength` | `client/batch_test.go:13` | the sizer decides |
| `TestBatchesCapsKeysPerBatch` | `client/batch_test.go:22` | key cap |
| `TestBatchesSendsAnOversizedRecordAlone` | `client/batch_test.go:29` | oversized record |
| `TestBatchesKeepsOrder` | `client/batch_test.go:37` | order |
| `TestRankOwners*` (7 tests) | `client/rank_test.go` | primary choice and read preference (part A owns the code; this area depends on it) |
| `TestPackRoundTrip`, `TestSendPackRecordsRoundTrip`, `TestSendPackRecordsPropagatesSourceError`, `TestPackReaderSurfacesRemoteError`, `TestPackReaderRejectsUnexpectedFrame` | transport-iroh `protocol/pack_test.go` | `PackSender`, `PackReader`, `AsyncRecords` |
| `TestClusterPushPull` | `node/cluster_test.go:210` | every key on ≥ min_replicas nodes; second push uploads 0; wrong ExpectedVersion fails; pull reproduces the objects; ref list/get/delete |
| `TestClusterNodeDownDuringWrite` | `node/cluster_test.go:423` | push succeeds with one owner down (min_replicas 2) |
| `TestClusterPushProgress` | `node/cluster_test.go:455` | progress invariants (§2.9) |
| `TestClusterPushPipelines` | `node/cluster_test.go:533` | `BatchBytes = 64 KiB` gives `InFlight ≥ 2` per node |
| `TestClusterGetYieldsBeforeEveryBatchIsFetched` | `node/cluster_test.go:660` | with `Jobs = 1` and 300 ms dial latency, the first record arrives before 600 ms |
| `TestClusterGetStopsEarlyCleanly` | `node/cluster_test.go:688` | 3 early breaks, then a full Get returns everything with 0 missing; `Close` does not hang (10 s) |
| `TestClusterPullWithNodeDown` | `node/cluster_test.go:733` | every key served by its next owner; `Fetched == len(keys)` into a fresh store |
| `TestClusterGC`, `TestClusterRemoveNode` | `node/cluster_test.go:294, 379` | push/pull around GC and removal (integration) |
| `TestClusterPutStreamsWhileReceiving`, `TestClusterPutGivesUpASlowForward` | `node/cluster_test.go:559, 767` | node-side put semantics; useful as interop checks of the put-result shape (3 holders; failures under slow forwards) |
| `TestWorktreeInitPushCloneEditPull`, `TestWorktreeConflict`, `TestWorktreePushRecoversAfterLostState` | `node/worktree_test.go:42, 162, 206` | PullTree via fetch, Push with version CAS, CAS-mismatch recovery |
| `TestStatusLine`, `TestUIModel` | `cmd/dstore/tui_test.go:94, 15` | consumers of `ProgressReport` (CLI part) |

The Go cluster tests run nodes in process over `transport.Network`. The Rust port has no node, so either (a) run Go v0.1.9 nodes as subprocesses over loopback iroh in integration tests, or (b) write a minimal Rust fake node (HashMap store; `view`, `missing`, `get`, `put`, `ref-get`, `ref-put` with a scripted `incomplete`/`cas-mismatch`) over an in-memory transport for unit tests. Both are needed: (b) for fast deterministic tests of this area, (a) for compatibility.

---

## 7. Gaps in core-rs and Rust iroh, with workarounds

| # | Gap | Impact | Recommended workaround |
|---|---|---|---|
| G1 | core-rs `amberpack::Writer` and `Reader` are synchronous (`std::io`) | put and get streams are async iroh streams | Implement `PackSender`/`AsyncRecords` in `wire::pack` (§4.4) on top of `parse_record`/`decode_payload`, mirroring `Reader::next_record` (`src/amberpack.rs:337-383`) including the `Malformed` messages. Not recommended: `tokio_util::io::SyncIoBridge` in `spawn_blocking` (a blocking thread per stream, awkward cancellation). |
| G2 | `Key::type_()` panics on a reserved type nibble (Go returns `Type(n)`) | a hostile record could crash the client | Call `validate()` before `type_()` in `verify_record`; use only `parse_record`-validated, `Key::parse`d or `child_keys` keys elsewhere. `est_size`/`stored_sizer` use `length()` only. |
| G3 | `fstree::reachable_keys`, `check_complete` and `Store::write_parallel` block and spawn scoped threads | blocking the async runtime | `spawn_blocking` with `Arc<Store>` (§4.3). `pull_tree` calls `check_complete` for every held interior node; accept the thread churn or add a pooled variant later. |
| G4 | core-rs `refstore` uses redb; Go core's refstore is Pebble | `dstore store push/pull --local DIR` reads/writes `<DIR>/refs`. A Go and a Rust CLI cannot share that directory; `<DIR>/packstore` is compatible | Open decision D1. Interim: before opening, detect a Pebble directory (`MANIFEST-*`, `OPTIONS-*`, `CURRENT`) and refuse with a clear error instead of creating a redb file beside it. |
| G5 | Rust iroh has no `HasRTT`. `Path::rtt()` (`path_watcher.rs:494`) returns the estimator's `latest`, which is `initial_rtt` (noq default 333 ms) until the first sample (`noq-proto-1.1.1/src/connection/paths.rs:772-779`) | Go reports `RTT = 0` for an unmeasured path (class 0, "near"); a naive port reports class 3, so fresh owners rank behind measured ones and pushes stop spreading over owners (breaks `TestClusterPushProgress`'s "every node received bytes") | In `PathInfo`, treat RTT as 0 while the selected path's `stats().frame_rx.acks == 0` (no ACK yet, so no sample), or while `rtt == initial_rtt` exactly. `direct = !path.is_relay()` for the path with `is_selected()`; with no selected path `direct = true` (Go default, `transport/iroh.go:433-444`). |
| G6 | zstd output differs (klauspost vs libzstd) | records ingested by Rust differ in bytes from Go's for the same tree, so `PushStats.bytes` and progress totals differ for identical trees; keys and interop are unaffected | None needed. Golden vectors that must be produced identically use incompressible data. |
| G7 | No in-memory transport in Rust iroh (Go has `transport.Network` with `SetDown`, `SetDelay`) | the §6 cluster tests and §5.2 transcripts need one | The transport part should expose a trait (`Endpoint`, `Conn`, `Stream`) with an iroh implementation and a mem implementation (`PathInfo{direct: true, rtt: 1ms}`, as `transport/mem.go:210`). |
| G8 | noq streams have no `CloseWrite`/`CancelRead` names | stream discipline | `SendStream::finish()`, `RecvStream::stop(0u32.into())`, or drop both halves (§4.4). |
| G9 | core-rs `stored_size` returns `Option<u64>`; Go returns `(n, ok, err)` | none | `Ok(Some(n))` → `46 + n`; `Ok(None)` or `Err(_)` → `key.length()`. |
| G10 | core-rs `write_parallel` returns `(WriteStats, Result<(), Error>)` | none | Take `.1` for the error, like Go's `_, err :=`. |

Nothing else this area uses is missing from core-rs: `get`, `get_record`, `has`, `stored_size`, `child_keys`, `check_complete` (visited keys), `reachable_keys`, `parse_record`, `decode_payload`, `Reference::encode/decode` and `Key` all exist with Go-compatible error texts.

---

## 8. Risks and open decisions

### 8.1 v0.1.9 behaviours to reproduce as they are (a "fix" would change interop or observable output)

1. `askPrimaries` ignores a `Keys32` error in the reply, so a malformed lacking list makes every key of the chunk "held" (`objects.go:108`).
2. `putOnce` silently skips a key whose record cannot be read (`objects.go:271-273`); the key stays short and fails the ack policy later.
3. `putBatch` sleeps on `busy` without looking at ctx and retries the same primary after a view change.
4. `getStream` emits records it did not ask for, and duplicates; `PullTree` counts them against `pending`, and the final gate then fails with `pull: tree incomplete after fetch`.
5. The read order is recomputed per routing with current penalties, so a retry can skip an owner (§2.5); only the first key to exhaust its order triggers the single view refresh.
6. `VerifyRecord` does not check a Blob's length field.
7. Push re-pins only between rounds; the incomplete-retry path ignores the put result and direct-fills every key just uploaded.
8. `negotiate %x…`, `record %x rejected…` and `shortError`'s owner order depend on Go map iteration (random): the Rust port may pick any; tests must not compare these beyond one entry.
9. `Get`'s `missing` accessor accumulates across repeated iterations.

Decision D0: port all of these verbatim and note each in the implementation's port notes; change them only upstream first.

### 8.2 Risks

- **R1 Context semantics.** Go's `ctx.Err()` drives retry decisions and error text (`context canceled` on SIGINT, `context deadline exceeded` on the 20 min stream timeout). Dropping futures loses that information; use the `Ctx` type of §4.1 throughout.
- **R2 Recursion in `want`.** Recursive async functions need boxing, and deep trees could exhaust the stack. Implement `want` iteratively with an explicit stack that yields the same pre-order queue order (the order only affects batching, not compatibility).
- **R3 Progress callback under the lock.** The Rust tracker must call `prog` while holding a `std::sync::Mutex`; a callback that blocks or re-enters the client deadlocks, exactly as in Go. Document it on `Progress`.
- **R4 Per-record timing.** Go reports `Sent` after each record enters a 4 KiB bufio buffer; Rust will report after `add_record` returns. Reports and TUI rates differ slightly in granularity. Only the invariants of §2.9 are tested.
- **R5 `%.1f` and float rounding in `human_bytes`.** Go uses correctly rounded decimal conversion; Rust's `format!("{:.1}")` is also exact, but check it against §3.6 (`1024.0 KiB`, `682.7 KiB/s`).
- **R6 Throughput.** §11.3's QUIC window tuning is a transport concern; without it a Rust client can be much slower than Go on WAN paths. Performance only, not compatibility.
- **R7 `ClientError` cloning.** `MissingResult.failed` and `PutResult.errors` hold errors per key and per node; use `Arc<ClientError>`.
- **R8 Blocking reads on the async side.** `get_record`, `has` and `stored_size` read mmaps or `pread`; inline calls are fine on a multi-thread runtime but must not run on a current-thread runtime serving the pack stream.
- **R9 Dispatcher refresh.** `route` runs `RefreshView` synchronously inside the dispatcher (up to `RequestTimeout` per node). Keep it inline to preserve dispatch order; do not spawn it.
- **R10 Architecture drift.** §2.10 lists where the architecture document and v0.1.9 disagree; the implementer must follow the code.

### 8.3 Open decisions

- **D1** The local refs directory of `store push/pull`: (a) accept incompatibility and refuse on a Pebble directory; (b) write a minimal Pebble-compatible reader/writer; (c) change the layout for the Rust CLI. Recommend (a) for now.
- **D2** The heuristic for "RTT not yet measured" on Rust iroh (G5): `frame_rx.acks == 0` vs `rtt == initial_rtt`.
- **D3** The test harness: Go v0.1.9 node subprocesses plus a Rust fake node over an in-memory transport (§6). Recommend both.
- **D4** Which `iroh` version: 1.0.3 (verified against go-iroh v0.2.0) or 1.1.0 (present offline, unverified). Recommend 1.0.3.
- **D5** The shape of `get`: a `futures::Stream` with a `missing()` accessor (recommended), or a callback API.
- **D6** The logger: a small slog-text-compatible formatter shared with part A (§3.5: `level=… msg=… key=value`, quoting values with spaces, Durations in Go's `String()` form, TUI rendering `bytes` with `human_bytes`).
- **D7** Whether `Config.jobs`/`conns`/`batch_bytes` stay hidden from the CLI, as in Go (no flag feeds them): keep them hidden.


---

## Addenda (synthesis)

Added by the architecture synthesis. `PORTING.md` is normative where it differs from this spec.

1. **Open decisions resolved.**
   - D0: port the listed quirks verbatim.
   - D1: `open_local` refuses a Pebble `refs/` directory for both `store push` and `store pull`
     (PORTING §2.3).
   - D2: the sentinel `rtt == NOQ_INITIAL_RTT`, with `initial_rtt` set explicitly.
   - D3: both a testkit `FakeNode` and live Go interop.
   - D4: iroh `=1.0.3`.
   - D5: `GetStream` with `missing()`.
   - D6: `gocompat::slog`.
   - D7: `jobs`, `conns` and `batch_bytes` stay unreachable from the CLI.
2. **Signatures** per PORTING.md §4.8:
   - `stamp(&mut Msg)` and `call/call_retry/any_node(ctx, id, &mut Msg)`;
   - `Progress` takes `&ProgressReport`; counts are `i64`;
   - `RecordSource`/`RecordSizer` are `Arc<dyn Fn(&[u8; 32]) …>`;
   - `PullStats.root` is `amber_store_core::key::Key`, with a manual `Default`.
3. **`want` in `pull_tree`.** For each held interior node, run a sequential local completeness check
   in `spawn_blocking`. It gives the same boolean as `fstree::check_complete` (core-rs-gaps G12)
   without spawning threads per level. The final gate still uses core-rs `check_complete` for its
   error text. `want` is iterative and keeps Go's pre-order queue order.
4. **Pack I/O** lives in `dstore_wire::pack` (`PackSender`, `PackReader`, `PackRecords`).
   `PackRecords` reads from the `PackReader`'s own chunk buffer (no `BufReader`), so `drain()` always
   consumes TDataEnd.
