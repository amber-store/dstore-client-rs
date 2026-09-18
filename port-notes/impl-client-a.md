# impl-client-a: `dstore-client` part A

Owner client-a (layer L4). Files:
- `crates/client/src/{lib.rs, cluster.rs, rank.rs, batch.rs, progress.rs, refs.rs, watch.rs, error.rs, corefmt.rs}`, each with unit tests;
- `crates/client/Cargo.toml` (dev-dependencies);
- `tests/golden_tests/client.rs`;
- the part A scenarios of `tests/fake_cluster.rs`.

A port of dstore v0.1.9 `client/client.go`, `rank.go`, `batch.go`, `progress.go`, `refs.go` and `watch.go`.

## Items added (no PORTING.md §4.8 signature changed)

- `pub fn rate_ns(bytes: i64, took_ns: i64) -> String`: Go `Rate` over a signed `time.Duration`. `client/progress.json`
  has `took_ns < 0` cases, which `std::time::Duration` cannot express. `rate(bytes, took)` calls it with
  `duration_to_ns(took)`.
- `crates/client/Cargo.toml` `[dev-dependencies]`: `dstore-testkit`, `hex`, `serde`, `serde_json`, and `tokio` with
  `test-util`. Crate unit tests load golden JSON and use the paused clock.
- `#[source]` on `Error::NotPlacedLastError.placed`. Go's `fmt.Errorf("%w; last upload error: %v", …)` wraps only
  the short error; the error text is unchanged.
- Crate-internal: `Cond::apply`, `Cluster::all_nodes` (Go `allNodes`), `cluster::backoff_after`,
  `progress::path_attrs_of`.

## Decisions

**Cluster state.**
- `Inner.state` is an `Arc<RwLock<ViewState>>`, shared with the pool's `AddrsFn` closure (`addrsOf`), so the pool
  and the cluster have no reference cycle.
- A poisoned lock is recovered, and no lock is held across an await.
- `ViewState.boot_order` keeps the ticket's 32-byte member ids in first-occurrence order. Go ranges over the
  `bootAddrs` map (random order) in `anyNode` and `allNodes`; Rust uses ticket order (DD-10).
- A later duplicate member overwrites the addresses, as Go's map assignment does. `Dial` still tries every
  duplicate.
- Backoff deadlines and `penalty` use `tokio::time::Instant`, so tests run on the paused clock.
- `failures` is a `u64` with saturating increments.

**Requests.**
- `call` → `go c.RefreshView(context.Background())` is `spawn_refresh`: a plain `fn` that spawns a boxed future
  through `Handle::try_current()`. The boxing breaks the opaque-type cycle `call` → `refresh_view` →
  `any_node` → `call`. Without a tokio runtime nothing is spawned (Go always starts a goroutine), which only a
  non-tokio executor could observe.
- `admin(ctx, Some(zero id), …)` goes to `any_node`, like Go's `id == view.NodeID{}`. The CLI passes the zero id.
- `probe_hinted` runs its probes on a `JoinSet`, in id order (DD-10).
- `Dial` and `call` cancel their child ctx after the call, as Go's `defer cancel()` does.

**Errors.**
- The predicates `remote`, `is_code`, `cas_mismatch`, `incomplete`, `is_unknown_ref` and `ctx_error` walk the
  `source()` chain (Go `errors.As`/`errors.Is`).
- The walk also looks inside `Box<Error>` and `Arc<Error>` sources. thiserror exposes such a source as its own
  chain node, and the node's `source()` skips the inner error's variant.

**Watch** (`watch.rs`).
- An `async_stream::stream!` generator, so it pulls like Go's range-over-func. `watchOnce` is split into
  `watch_open` and a `WatchSession`, whose `next()` returns one event, the end of the stream, or a fatal error.
  The generator yields events without checking ctx between them, as Go's frame handler does.
- Frames are read by a spawned task into `mpsc::channel(1)`, because `read_msg` is not cancel-safe.
  - `abandon()` aborts that task, which drops the receive half (STOP_SENDING 0), and finishes the send half.
  - Dropping the session is the deferred `wire.CloseStream` plus `rcancel`.
  - When the consumer drops the stream, the session is dropped at the yield point. The pool connection stays,
    as in Go.
- `state` is updated right before each event is yielded, and `watch: synced` logs `refs=len(state)` after the
  earlier events of the stream.
- **Known list.**
  - Sorted by name (DD-10), with `Version` null and `CreatedAt` 0.
  - An empty key in the caller's `known` map stands for Go's nil key and encodes as null, which matches the
    `ref-watch-nil-key` vector. Keys learned from frames keep null and empty apart.
- The idle timer is a deadline re-armed on each frame, and it keeps running while the consumer holds an event.
- Jitter is `rand::rng().random_range(0..=delay/2)` in nanoseconds.

**Progress.**
- `Tracker` holds a path lookup: `pool.path(id, ALPN_CLIENT)` of the cluster, or scripted paths in tests.
  The `relayed_path` scenario needs the latter.
- The callback runs under the tracker's mutex. Lock order: tracker, then pool.

**corefmt.**
- `walk_error_text` re-renders names with `gocompat::quote`. That covers `WalkError`, the `ChildKeysError`
  entry names, and `fstree::Error` `EntryContentKey`/`EntryXattrsKey`.
- Type names follow Go `key.Type.String()`, including `Type(n)` for reserved nibbles, where core-rs
  `Key::type_()` panics.
- `cbor_error_text` maps the core-rs `cbor::Error` variants to cborx texts recursively (the `cborx: ` prefix,
  a bare `unexpected EOF`, quoted xattr names).

**lib.rs.**
- The crate-level L0 `#![allow(dead_code, unused_variables)]` is gone. The allowance now sits only on the part B
  module declarations (`fetch`, `objects`, `tree`), so part A is linted fully; it is harmless now that part B
  has landed.
- Seams only part B calls (`batches`, `Tracker`, `call_retry`, `probe_hinted`) carry `#[allow(dead_code)]`.

## Tests

- **Crate unit tests** (`cargo test -p dstore-client --lib`), part A. They run over `dstore_transport::mem` with a
  scripted node (`cluster::test_node`), with no sockets:
  - `rank_test.go` (7) and `batch_test.go` (4);
  - the golden files `client/rank.json`, `client/batches.json`, the tracker, `count_keys`, `human_bytes`/`rate`
    and `path_attrs` of `client/progress.json`, and `client/backoff.json` (backoff on the paused clock, the
    watch delays and jitter bounds);
  - Dial: defaults, errors, member order, the `connected` log;
  - `adopt` ordering and hint replacement;
  - penalties, backoff expiry and remote errors adopting a carried view;
  - the async refresh and stamping;
  - `call_retry` makes 4 attempts; `any_node` makes 5 per node, stops on a remote answer, moves on after a
    transport error, keeps going after cancellation, and returns `client: no nodes`;
  - `status` has no type check;
  - admin frames and replies;
  - read order re-ranks only the first R;
  - `probe_hinted`;
  - the client-core §3.2 ref request frames, and every ref outcome including paging;
  - watch: request frames, events and reconnect with the updated known list, terminal codes, stale-view
    attempts and growing delays, idle reconnect, refusal and unexpected frame, cancellation;
  - corefmt texts, including `cbor::decode_xattrs` outputs;
  - error predicates.
- **`tests/golden_tests/client.rs`** (`--test golden client::`, 3 tests):
  - `human_bytes`/`rate` over `client/progress.json`;
  - every part A case of `errors/client_text.json`, built from its fields. The 20 cases Go produced through
    real client calls (Dial failures, ref and admin replies) are also reproduced through real `Cluster` calls
    over `mem`.
- **`tests/fake_cluster.rs`**, over `dstore_testkit::fake`:
  - Dial (unstamped view, the next member when the first is down, the text with every member down);
  - the newer-epoch refresh and the stale-view ref-put retry;
  - backoff moving requests off a down node;
  - references (create, CAS mismatch, get, keyed put, stale delete, delete, unknown ref, incomplete,
    bad-request);
  - `ref_list` paging at limit 2;
  - `TestClusterWatchRefs`, `TestClusterWatchBadPattern`, `TestClusterWatchLostHint`,
    `TestClusterWatchReconnect`.

  Blobs are stored with part B's `Cluster::put`.

## Results

- `cargo test -p dstore-client --lib`: 83 passed, 0 ignored. That includes part B's tests (client-b landed during
  this task).
- `cargo test -p dstore-client-rs --test golden client::`: 3 passed. `--test fake_cluster`: 9 passed.
- `cargo clippy -p dstore-client --all-targets -- -D warnings`: clean. Root clippy over `--test golden --test
  fake_cluster` reports nothing in these files.
- rustfmt (edition 2024) is clean for every owned file.
- **A flaky test that is not part A.** `objects::tests::get::yields_before_the_batch_is_complete` (client-b)
  failed once in a full run while the machine was loaded: it bounds the first record at 400 ms. It passed 6
  times on its own and in the final full run.

## Findings

- **Same-record CAS retry.** Go `catalog.RefPut` treats a CAS failure whose current record equals the written
  record as the success of a retry after a lost reply, and the fake ports that. A test of "must not exist" must
  therefore write a different record the second time.

## Review

Reviewer review-client-a. Checked line by line against dstore v0.1.9 `client/client.go`, `rank.go`, `batch.go`,
`progress.go`, `refs.go`, `watch.go`, `wire/wire.go` (`AsError`, `IsCode`, `ErrorFromMsg`, `CloseStream`) and
`view/view.go` (`Compare`, `Node`). Also checked against core v0.0.8 `reference/reference.go`, `fstree/*.go`,
`cborx/cborx.go` and `key/type.go`, and against the Rust seams part A calls (`dstore_transport::Pool`,
`dstore_gocompat::ctx`, `dstore_view`). The Go tests `client/rank_test.go`, `client/batch_test.go` and
`node/watch_test.go` were compared with their ports. No test was `#[ignore]`d, so nothing needed un-ignoring.

### Verified, no change

- **Dial.**
  - The endpoint check comes before the defaults.
  - Members are tried in ticket order, 32-byte ids only. Duplicates are tried again, and the last addresses win.
  - The view request is unstamped and goes through `Pool::call` under 15 s. An `adoptReply` failure moves on.
  - The `connected` attributes; the pool is closed on failure; `client: ticket names no nodes` only when no
    member was tried.
- **View cache.**
  - `adopt` orders views by (incarnation, epoch, version): `View::compare` Greater is Go's +1.
  - `adoptReply` replaces the hints even when it keeps the older view.
  - `addrsOf` prefers non-empty view addresses (nodes, then pending), else the ticket's.
- **Requests.**
  - `call` stamps in place under `RequestTimeout`, runs `handleErr`/`ok`, and refreshes only for a reply newer
    than the cached view.
  - `callRetry`: 4 calls. `anyNode`: 5 calls per node; a remote answer stands; transport errors move on;
    cancellation does not stop the loop; `client: no nodes`.
  - `Status` has no type check. `Admin` sends the zero id to `anyNode`, else `client: unexpected reply`.
- **Bookkeeping.**
  - Backoff is `5s << min(failures-1, 4)` capped at 60 s. Remote errors only adopt a decodable carried view.
  - An expired backoff keeps counting failures until `ok`. Penalties are +2 and +1.
- **Ranking.** A stable sort by (penalty, relay, class, position), with unmeasured nodes direct and class 0.
  `ReadOrder` re-ranks only the first R, from one snapshot.
- **References.**
  - `cond.apply`; `refErr` only for get, put and delete.
  - A `Keys32` failure gives a nil sample.
  - `RefList` stops on an empty `Next` or empty `Refs`, and its errors are not mapped.
- **Watch.**
  - 4 attempts per node on stale-view.
  - A served stream: the 200 ms pause and the delay reset.
  - An unserved round: the refresh bounded by 15 s, `in` = the pre-jitter delay, jitter in `[0, delay/2]`,
    doubling to 30 s.
  - Failures:
    - an open failure → `handleErr`;
    - a write failure → Drop, `handleErr`, then `CloseStream`;
    - a frame error, idle, or an unexpected frame → abandon, Drop, `handleErr` (and a log for the first two),
      then `served()`.
  - TErr codes without a Drop: stale-view retries; bad-request and unauthorized are yielded, then fatal; others
    are logged and the next node is asked.
  - Events are yielded without a select between them, refs before deletions. The state is updated before each
    yield. `refs=len(state)` counts the earlier events. One Synced per stream. The idle deadline is re-armed
    only when a frame arrives.
- **Progress.**
  - Tracker counters; the report runs under the lock; nodes by id with the pool path; `bytes()` reports nothing.
  - `countKeys`; `pathAttrs` rounding to ms; `HumanBytes`; `Rate` with Go's `Duration.Seconds`.
- **Errors and corefmt.**
  - Every text, and the predicates through `Box`/`Arc` sources.
  - Every core v0.0.8 fstree and cborx format string that corefmt re-renders: `%q` names, `Type(n)`, the `cborx: `
    prefix, bare `unexpected EOF`.
  - `ValidateName`/`ValidateUser` check order.
- **Tests.**
  - The 7 `rank_test.go` and 4 `batch_test.go` ports use Go's inputs and assertions.
  - Every case is asserted:
    - `client/rank.json`: 18 `rtt_class`, 66 `rank_owners`;
    - `client/batches.json`: 22;
    - `client/backoff.json`: 15 `handle_err`, 10 `watch_reconnect`, the 4 timeouts;
    - `client/progress.json`: 5 tracker scripts with a report check per op, 3 `count_keys`, 74 `human_bytes`,
      52 `rate`, 12 `path_attrs`.
  - No rank scenario repeats an id, so the per-id penalty and path maps cannot mask a case.
  - The `watch_test.go` ports follow Go's steps.

### Fixed

1. **Stale L0 lint allowances.**
   - `lib.rs` still had `#[allow(dead_code, unused_variables)]` on the part B modules ("until their stubs are
     implemented").
   - `call_retry`, `probe_hinted`, `batches` and `Tracker` had `#[allow(dead_code)] // seam for part B`.

   Part B has landed and calls every seam, so these were removed and `objects` and `tree` are linted fully.
   Without the module allowances clippy reports one item: `fetch::est_size` (PORTING.md §4.8) is used only by
   fetch's tests, because the fetcher uses `est_size_i64`. `mod fetch` keeps a narrow `#[allow(dead_code)]`
   stating that reason. `fetch.rs` belongs to client-b.
2. **Test hacks removed.** They only kept imports or parameters in use:
   - the `refs.rs` tests marshalled a request only to use an import;
   - `tests/golden_tests/client.rs` had `let _ = (T_ADMIN, …)` for unused imports;
   - `cluster.rs` `dial_err` took and discarded a `&TestNet`.
3. **A weakened golden bound.** `client_error_texts` asserted `built_n >= 40`. The file has 46 part A cases (58,
   of which 12 are part B kinds), and the test now asserts exactly 46.
4. **The golden watch timing tested a copy of the formula.** `golden_watch_timing` re-implemented
   `min(2*delay, 30s)`. The generator now calls `watch::next_delay` and the test drives the same function, so all
   10 `watch_reconnect` rounds check production code.

### Tests added

These pin quirks and branches that no test covered:
- `cluster::call_spawns_one_refresh_per_newer_reply`: 3 replies stamped with a newer epoch spawn 3 stamped view
  requests. There is no single-flight (PORTING.md §1.4).
- `cluster::call_retry_stops_on_any_other_outcome`: a remote error, a success and an EOF each end `callRetry`
  after one call.
- `watch::watch_idle_timer_runs_while_the_consumer_holds_an_event` (PORTING.md §1.4): the consumer holds an event
  for 6 s with `WatchIdle` 5 s. When it resumes, the stream idles out at once (under 1 s on the paused clock) and
  the next node serves.
- `watch::watch_served_stream_resets_the_delay`, on the paused clock:
  - an unserved round logs `in=1s`;
  - a stream syncs and then ends;
  - the next unserved round logs `in=1s` again and asks the unpenalised node first.

  The full log sequence is asserted.
- `watch::watch_stale_view_retry_carries_the_adopted_epoch`: a stale-view answer carrying epoch 8 is adopted, and
  the retry to the same node is stamped with epoch 8, without a penalty.

### Left as they are

- DD-10 orders: ticket order for the bootstrap fallback, the known list by name, probes by id.
- **Empty keys in the known map.** `watch_refs` maps an empty key of the caller's `known` map to CBOR null. Go
  tells nil from `[]byte{}`, which `HashMap<String, Vec<u8>>` cannot. Only library callers can observe this: the
  CLI passes nil, and the node ignores empty keys either way.
- **Dropped probes.** `probe_hinted` runs its probes in a `JoinSet`, so dropping its future aborts them, where Go's
  goroutines would finish. Every caller awaits it.
- **Real time in `tests/fake_cluster.rs`.** Those tests run on a multi-thread runtime and real time; the timing
  assertions live in the crate unit tests on the paused clock.

### Results

- `cargo test -p dstore-client --lib`: 88 passed (83 plus the 5 new), 0 ignored, in two full runs.
  - `watch::` and `cluster::` (30 tests) passed 3 more runs.
  - client-b's `objects::tests::get::yields_before_the_batch_is_complete` passed in both full runs.
- `cargo test -p dstore-client-rs --test golden -- client::` (which also selects `client_transfer::`): 6 passed.
  `--test fake_cluster`: 9 passed.
- **Clippy.**
  - `cargo clippy -p dstore-client --all-targets -- -D warnings`: clean, re-checked after touching `lib.rs`.
  - `cargo clippy -p dstore-client-rs --test golden --test fake_cluster --no-deps -- -D warnings`: clean.
- `rustfmt --check --edition 2024` over the owned files: clean.
- Linux byte identity is left to CI. The scratch target directory was deleted.
