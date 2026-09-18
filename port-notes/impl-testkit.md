# impl-testkit: `dstore_testkit::refglob` and `dstore_testkit::fake`

Owner testkit (layer L3). Files:

- `crates/testkit/src/refglob.rs`: dstore v0.1.9 `refglob/refglob.go`, with unit tests (the Go tests and
  `tests/golden/refglob/refglob.json`).
- `crates/testkit/src/fake.rs`: `FakeCluster` over `dstore_transport::mem`, with unit tests.
- `crates/testkit/src/lib.rs`: the L0 `#![allow(dead_code, unused_variables)]` for the refglob and fake stubs
  is removed (its comment asked for that once they were implemented). Nothing else changed there.

## Items added to PORTING.md §4.13 (no signature changed)

- `refglob::compile_bytes(pattern: &[u8]) -> Result<Glob, String>`: `Compile` over the bytes of a Go string.
  `refglob.json` `invalid` has patterns that are not UTF-8 (`pattern` absent, `pattern_hex` only); they get
  `refglob: pattern is not valid UTF-8` only through this entry point. `compile(&str)` calls it.
- `refglob::MAX_LEN` (4096), `Glob::pattern() -> &str` (Go `String()`), `impl Display for Glob`,
  `#[derive(Clone, Debug)]` on `Glob`.
- `fake::DEFAULT_REF_PAGE_LIMIT` (20000) and `fake::DEFAULT_WATCH_RECONCILE` (30 s).
- `impl Default for FakeClusterConfig`: Go `cluster3` (3 nodes, R = 3, min_replicas 2).
  `#[derive(Clone, Debug)]` on `FakeClusterConfig`; `#[derive(Clone, Debug, PartialEq, Eq)]` on `Injection`.
- `FakeCluster::ref_put_local(&self, id: NodeId, record: &[u8]) -> Result<Vec<u8>, String>`: Go
  `node.RefPutLocal` with `Cond{Force: true}`, the `putLocal` helper of `node/watch_test.go`. Those tests write
  at a chosen node, which a client cannot do. The watch ports need it.

## refglob decisions

- **No regexp.** Go compiles a pattern into an RE2 regexp. The port simulates the equivalent automaton directly,
  which is linear in the name like RE2. Ops: literal rune; `[^/]`; class; `[^/]*`; `.*` (no `\n`, since Go's
  default flags leave `.` without `DotNL`); `(?:[^/]*/)*` (empty or any string ending in `/`). A class
  matches `c != '/'` and (in a range XOR negated). Go carves `/` out of positive classes, adds it to negated
  ones, and turns a class listing only `/` into match-nothing, which is the same set.
- **Compile errors.** The regexp Go builds from a validated pattern always compiles: literals go through
  `QuoteMeta`, class items through `classQuote`, and ranges are ordered. So `refglob: %w` has no reachable
  case, and no sizes approach RE2's limits at 4096 bytes.
- **Rune quoting.** `bad range %q-%q` uses `strconv.QuoteRune`. It is `gocompat::quote::quote` of the rune
  with the quotes swapped, and only `'` and `"` escape differently. Unit test: U+2028 → `' '`.
- **Validation order** is Go's: empty, then length, then UTF-8, then control characters, then syntax.

## FakeCluster decisions

- **Ids and addresses.** Node `i` (0-based) has the Go harness id `nid(i+1)` (bytes 0 and 31 = `i+1`) and
  address `mem:<shortid>`. The endpoints speak `amber-dstore/1` and `amber-dstore-cluster/1`.
- **View.** Incarnation 1, epoch 1, version 1, placement epoch 1. Every node is a voter (since 1),
  `voter_sync` done, weight 100 unless configured, writable, and the cluster id is `splitmix::data(seed, 16)`.
  `min_replicas == 0` → `DefaultMinReplicas`. `ticket()` names every node with its `mem:` address, the cluster
  id and the incarnation.
- **Shared state.** The fake keeps one view, one store per node (key → record, append order standing in for
  pack location), the catalog, and the watch registry.
- **Members.** Where a Go node calls another member over the cluster ALPN, the fake checks reachability
  first:
  - the operations are forwards in `put`, `negotiateHolders` in `missing`, `negotiateComplete` and `getData`
    in the ref-put completeness walk, and the ref-changed broadcast;
  - the check is a cached cluster-ALPN `Conn` that is not closed, else a fresh `Endpoint::dial`, so
    `Network::set_down` / `partition` apply;
  - on success the fake reads or writes the member's state directly;
  - failures feed `markUnreachable`, so `TViewReply.Unreachable` lists them (2 min TTL); successes and
    incoming conns clear them.
- **Forwards.**
  - A reachable, writable owner stores the records: a dedup hit pins.
  - A non-writable owner fails the keys with `no-space`, what the peer's `handlePut` answers.
  - An unreachable owner fails them with `unreachable` (`reasonOf`).
  - `RetryAfter` is `500 + rand % 1500` from the cluster's splitmix stream.
  - Holder lists are `[self, owners in write-set order]`, and failures are in owner first-use order (DD-10).
- **Versions** are 40-byte ballots (big-endian counter ‖ coordinator id) from one counter, the shape of Go's
  `paxos.Ballot`. verification §4.6 suggested an 8-byte counter; the Go shape is closer and clients treat
  versions as opaque. The watch compares ballots as Go does.
- **Catalog.**
  - `RefPut`/`RefDelete` port `catalog.go` including `Cond.check`, the retry-after-lost-reply success, and
    deleting an absent name under a condition.
  - `condOf` treats an empty omitempty byte field as Go's nil (a Go client never sends a present empty one).
  - `RefList` ports the acceptor scan: inclusive lower bound `prefix`, or exclusive `after` when given (even
    below the prefix, as in Go); upper bound `prefix‖ff ff ff ff`; limit clamped to 100000 when 0 or above;
    the 4 MiB row accounting; `next` = last name when more rows remain.
  - `handleRefList` adds its own 4 MiB accounting.
- **`checkEpoch`** runs against the shared view, so a refresh cannot find a newer one: newer →
  `epoch above the catalog's` (put: `bad-request`; ref-put ignores it), older → stale-view, cluster id mismatch
  → `wrong cluster`.
- **Watch.** A port of `refWatch`:
  - the known list comes from matching request refs with a key;
  - the initial reconcile is paged with the ref page limit, followed by `ref-synced`;
  - hints go to a 1024 queue, and a full queue sets the rescan flag;
  - the ticker is at `watch_reconcile`, and the 4 MiB frame split is kept;
  - the stream ends when the client closes its send side, a write fails, or the cluster closes;
  - the known list is a `BTreeMap`, so deletions are queued in name order.
- **Injections.** `inject(id, op, nth, inj)`:
  - `nth` counts from 1 over client-ALPN requests of type `op` at that node, and `nth == 0` matches every one;
  - all delays apply first;
  - `CloseBeforeReply` runs the handler into `tokio::io::sink()` (side effects happen; a watch is not
    handled), then finishes and stops the stream and closes the connection. Go mem reads survive connection
    close, so the FIN is what ends the client's read;
  - otherwise the first `Err` answers `TErr{code, text, RetryAfter}`; `with_view` adds the view and stamps
    the frame, like `writeStale`;
  - `CorruptRecord(k)` on a `get` serves `k` with a payload bit flipped and the CRC valid;
  - `DropHints` on `ref-put` or `ref-delete` delivers that commit's hint only to the coordinator's watchers;
    on `ref-watch` that stream ignores hints.
- **Transcripts.** `requests(id)` holds every client-ALPN request frame read at the node, including injected
  ones, in arrival order.
- **Admin and status are canned** (doc comment of `FakeCluster`). Admin replies keep Go's texts and shapes;
  `gc-why` walks the stored trees; `catalog-backup` stores a blob at its owners so `catalog restore` can fetch
  it. Unknown ops → `bad-request "unknown admin op X"`; other admin failures → `unavailable`. Status carries
  records, bytes, pins, puts/gets/ref-puts, bytes in/out, writable, transition text, watchers, unreachable.
- **Cluster ALPN** answers only `view` and `ping` to members (`not-member` to others); nothing else in the
  fake uses it.

## Not ported (node behaviour client tests do not need)

- Paxos, maintenance, transitions, GC cycles, scrub.
- `isCorrupt` marks and the reconcile's forward healing: keys a forward failed stay short until a client
  re-sends them.
- `completeCache` in the completeness walk (a cache only).
- Forward timeouts and `putTx` streaming: forwards happen after the whole batch is read.
- `refreshOnce`.
- Write/forward admission slots.
- An API to set a view ACL: the allowlist check is ported but views from `start` have none.

`set_watch_reconcile` affects watch streams started afterwards, as a node config would.

## Tests

- `cargo test -p dstore-testkit --lib`: refglob, 10 tests (Go `TestMatch`, `TestPrefix`, `TestInvalid`; golden
  `match` 144, `prefix` 59, `invalid` 32 cases with `compile_bytes` plus `compile` when `pattern` is present).
- fake, 20 tests:
  - pure: `verifyRecord`, tamper, `condOf`, catalog put/delete/list, ballots, `checkEpoch`, texts, `localMissing`;
  - over `transport::mem` with a hand-rolled client: view/ping/transcript, put replication with a node down
    and missing negotiation, put refusals (stale, no-space, not-owner, verify), get absent/corrupt, references
    (incomplete, ok, CAS, list paging at limit 2, delete), watch (initial difference, hint, DropHints +
    reconcile), injections, admin/status.
- No test is ignored. The over-mem tests were written against the §4 APIs while wire, view, ticket and
  transport-mem were stubs; those landed during this task and the tests were un-ignored.
- The test client's `put` ignores pack write errors. A node that refuses a batch (stale view, no space)
  answers before reading the pack and resets the stream; Go's `putBatch` sees the same write failure. With
  `must` on the writes, `put_refusals` failed once in a full run.
- Results:
  - `cargo test -p dstore-testkit --lib`: 40 passed (refglob 10, fake 20, golden and splitmix 10).
  - The 8 over-mem tests: 20 repeated runs, no failure.
  - `cargo clippy -p dstore-testkit --all-targets -- -D warnings`: clean.
  - rustfmt: clean.

## Review

Reviewer review-testkit. Checked line by line against dstore v0.1.9 `refglob/refglob.go`,
`node/{server,data,put,refs,watch,admin,status,node,maintenance,gc}.go`, `catalog/catalog.go`,
`paxos/{ballot,acceptor,proposer}.go`, `transport/mem.go` and core v0.0.8 `fstree/reachable.go`.

### Verified, no change

- **refglob.**
  - Validation order and texts, and the prefix.
  - The automaton against the RE2 program Go builds: `.` without `DotNL`, negated classes that match `\n`
    (`ClassNL`), the carving of `/`, and match-nothing classes.
  - `%q` of runes (`strconv.QuoteRune`).
  - RE2's size and height limits cannot trigger at 4096 bytes.
  - All refglob.json cases (match 144, prefix 59, invalid 32) and Go's `TestMatch`, `TestPrefix` and
    `TestInvalid` are asserted. The non-UTF-8 invalid cases go through `compile_bytes`.
- **Catalog.**
  - `Cond.check`, `condOf`, the lost-reply retry of `RefPut`, and `RefDelete`'s absent-name rules.
  - The acceptor scan: a strict `after` (even below the prefix), the limit clamp, the 4 MiB row accounting,
    and `next` only when more rows remain.
  - `handleRefList`'s own accounting.
- **Handlers.**
  - Validation order and texts of put, ref-put (stale-view only for `errStale`), ref-get, ref-delete (no epoch
    check), missing and get.
  - `verifyRecord`, `writeStale`, and the 2 min TTL of `Unreachable`.
  - The watch: the known list, reconcile, the ballot comparisons of `applyHint`, the 1024 queue and its rescan
    flag, frame sizes and the heartbeat.
- **Constants.** Watch reconcile 30 s, page limit 20000, forward retry hint `500 + rand(1500)` ms. The probe
  dial timeout is the harness `ForwardTimeout` (10 s).

### Fixed

1. **Unreachable marks.** `reachable()` marked every member it probed. The completeness walk probes every
   member and the ref-changed broadcast dials every member, so a down or partitioned member that a Go node
   never called (a non-owner, or only a broadcast target) showed in `TViewReply.Unreachable`, which feeds the
   client's `probeHinted`.
   - Now `probe()` marks nothing. The marks follow Go: `remoteMissing` in `negotiateHolders` and
     `negotiateComplete` (both outcomes); a forward (a reply marks the owner reachable, a failed dial marks it
     unreachable, a refusal marks nothing); `getFrom` in the walk's fetch.
   - The broadcast marks nobody; incoming connections still clear a mark.
2. **`catalog-backup`** wrote an invented blob and no reference. It now ports `backupCatalog`:
   - the blob is `[]backupEntry{Name, Record}` over every reference but `dstore/catalog-backup` (CBOR null
     when there is none);
   - it is stored here and at the write-set owners that lack it, without a pin;
   - the reference `dstore/catalog-backup` (user `dstore`) is committed through `RefPutLocal` (completeness
     walk, hint);
   - the key is noted, and `catalog-backups` keeps the last 24.
3. **`gc-run` / `gc-status`** sent `GC = a0`. Go sends `MustMarshal(GCState)`, whose `Epoch` and `Phase` are
   not omitempty: `a2 00 00 01 00`, or `a3 00 00 01 00 09 f5` after `gc-hold`.
4. **`transition-refreeze`** always answered `participants re-frozen`. Go fails with `no frozen transition`
   unless a transition is frozen. Ported with the forced refreeze mutation.
5. **Admin checks.**
   - `node-remove|drain|weight|zone` fail with `<shortid> is not a member` for a non-member.
   - These ops and `replicas` check `a transition is already in progress` and `a voter change is in progress`
     first; `replicas must be ≥ 1` comes after.
6. **View nodes** carry `Incarnation: 1`, as `InitCluster` and join write them.
7. **`FakeClusterConfig.replicas == 0`** means 3, as in `InitCluster`.
8. **The scan's upper bound** is always `prefix ‖ ff ff ff ff`: the acceptor's prefix is `ref/‖prefix` and
   never empty. The old `ff` bound for an empty prefix was equivalent for UTF-8 names; now it is literal.

New unit tests:
- `admin_checks_gc_state_and_catalog_backup`;
- `close_before_reply_keeps_the_commit_and_the_retry_succeeds`: the reply is lost, then a retry of the same
  record answers `ok` at the committed version;
- `unreachable_lists_only_the_members_a_node_called`: 5 nodes, R = 2;
- `watch_lost_hint_by_partition`: `TestClusterWatchLostHint` at the frame level; it also asserts that the walk
  marks the partitioned owner.

### Known differences, left as they are

- **Forward to a non-writable owner.** Its keys fail with `no-space`. A Go owner answers before reading the
  pack and cancels its read, so the forwarder's pack write usually fails first and the reason is
  `unreachable`. Both are possible Go outcomes.
- **Tombstones.** Deleted references leave no tombstone rows. A Go acceptor's scan page can spend its limit on
  tombstones until they are purged; the fake's cannot.
- **Admin effects.** Admin transitions are not proposed (no pending transition, no epoch bump), and `gc-run`
  runs no cycle.

### Results

- `cargo test -p dstore-testkit --lib`: 44 passed, none ignored. The 12 over-mem tests passed 25 more runs.
- `cargo test -p dstore-client-rs --test fake_cluster` (the L6 port, owned elsewhere): 9 passed against the
  fixed fake.
- `cargo clippy -p dstore-testkit --all-targets -- -D warnings`: clean. rustfmt: clean.
