# impl-transport-mem: `dstore_transport::mem`

Owner transport-mem (layer L3). File: `crates/transport/src/mem.rs` with its unit tests. A port of dstore v0.1.9
`transport/mem.go`. No public signature of PORTING.md §4.6 was changed, and no item was added. The private
fields of `Network` and `MemEndpoint` were replaced: the scaffold's were guesses.

## Behaviour ported from Go

- **Dial order.**
  1. The dialing endpoint is closed → `transport: closed`.
  2. The dialing node is down → `mem: local endpoint is down`.
  3. The peer is down, or the link is cut → `mem: <short> unreachable`.
  4. The peer is missing or closed → `mem: <short> not bound`.
  5. The peer lacks the ALPN → `mem: <short> does not speak <alpn>`.

  Then comes the delay (`ctx.sleep`, ctx error on end). Nothing is rechecked after the delay. The pair is
  tracked on both endpoints, and the peer half goes to the accept queue (64 slots). When the ctx ends while
  that queue is full, the local half is closed and the ctx error returned.
- **`accept`** ignores the endpoint's closed flag, as Go does: queued (already closed) connections still come
  out, and an empty queue blocks until the ctx ends.
- **Connections.**
  - Each queues up to 256 unaccepted streams. `open_stream` blocks while the peer's queue is full, and fails
    with `transport: closed` or the ctx error.
  - `close` runs once: it cancels `done`, untracks the half and closes the peer half from a spawned task (Go
    `go c.peer.Close()`). Without a tokio runtime the peer is closed inline.
  - `set_down(id, true)` closes all of `id`'s connections, then every endpoint's connections to `id`, under
    the network lock. `partition(a, b, true)` closes the connections between `a` and `b` only.
- **Pipes.**
  - At most `PIPE_LIMIT` (4 MiB) bytes are buffered, and one `poll_write` takes what fits.
  - A write after `finish` fails with `io: read/write on closed pipe`. After the reader's `cancel_read`, writes
    fail with `mem: stream reset by peer` and reads with `mem: read canceled`. Cancel discards the buffer and
    wins over EOF. An empty write returns `Ok(0)` even on a closed or cancelled pipe.
  - The `io::Error` kinds are ConnectionReset, BrokenPipe and ConnectionAborted. They only matter to Rust
    code; `Display` is the Go text, so `wire::read_msg` renders `WireError::Io` / `ShortCause::Io` with Go's
    text. Go never matches these errors by identity.
- **Quirk kept** (C30): closing a connection, taking a node down or closing an endpoint does not touch the
  pipes of streams already handed out. A blocked read stays blocked, and bytes written afterwards are
  delivered.

## Decisions (no Go counterpart)

- **Drop of stream halves follows noq.**
  - Dropping an unfinished `SendStream` finishes it (the peer reads EOF).
  - Dropping a `RecvStream` that has not reached EOF cancels its read (the peer's writes fail with
    `mem: stream reset by peer`). A receive half dropped at EOF does nothing.
  - `AsyncWrite::poll_shutdown` finishes, as noq's does.

  Go mem streams have no destructor, but Go code always calls `CloseWrite`/`CancelRead` explicitly. The Rust
  port relies on the drop where it aborts a task (PORTING.md §5.1: the watch reader), so code over mem
  behaves as it would over iroh.

  Halves that were never handed to a caller have no drop effect, so their peers block as in Go. These are
  streams still queued when a connection goes away, or lost to a ctx end in `open_stream`.
- **Ownership follows Go's reachability** (changed in review, see below).
  - The `Network` holds a private `Registration` per bound id (id, ALPNs, accept queue, tracked connections,
    closed flag) until `bind` replaces it. Dropping a test's `Arc<MemEndpoint>` does not unbind it.
  - Every `MemEndpoint` handle holds its `Network` strongly, so endpoints keep their network alive:
    `Network::new().bind(..)` and helpers that return only endpoints dial as in Go.
  - There is no reference cycle. A registration does not refer to the network, connection halves hold their
    registration weakly, and the halves of one connection share one `Arc` pair state.
- **Deterministic order** (DD-10). Endpoints and tracked connections are iterated in id and creation order.
  Go iterates maps at random.

## Tests

`cargo test -p dstore-transport --lib -- mem::` runs 24 tests, none ignored:

- ids, ALPN, path `{direct: true, rtt: 1ms}`, dial to self, `addrs()`;
- every dial error text and the check order, including a closed dialer and the delay;
- endpoints keeping their network alive, an unheld endpoint staying bound, and everything freed after the last
  handle;
- FIN, write after finish, cancel and reset, drop semantics, shutdown and flush;
- streams opened by the accepting side;
- blocked reads woken by FIN and by an aborted task that held the peer stream;
- 4 MiB backpressure, and a blocked writer woken by cancel;
- close propagation, and a pending `accept_stream` / `open_stream` woken with `transport: closed`;
- the blocked-read quirk after conn close, `set_down` and endpoint close;
- the 256 stream and 64 connection queue limits with ctx expiry;
- ctx on `accept` / `accept_stream`; the dial delay on a paused clock, with ctx expiry and no recheck after
  the delay;
- `set_down` and `partition` closing exactly the right connections;
- endpoint close with accept still draining; bind replacing without closing;
- close outside a runtime.

The mem parts of `transport/pool_scripts.json` (`mem_dial_errors`, `call_ctx_deadline`,
`open_stream_failure_drops`, `mem_stream_pipes`, `closed_conns_are_filtered`) are consumed by
`tests/golden_tests/transport.rs` (owner: transport), whose harness mirrors `tspRunScript` over this mem
transport. The unit tests mirror their mem steps with the same expected texts.

**Scratch harness** (original implementer; not committed, deleted afterwards). It replayed every script of
`transport/pool_scripts.json` against the real `Pool` and this `mem` on a 4-worker runtime: 13 scripts, 103
steps, 0 mismatches, in three runs. The transport owner's golden module now does the same.

**Gates.**
- rustfmt (edition 2024) is clean for `mem.rs`.
- `cargo clippy -p dstore-transport --all-targets -- -D warnings` is clean.

## Review (review-transport-mem)

Checked `mem.rs` line by line against dstore v0.1.9 `transport/mem.go` (the whole file), transport.md §2.9,
§4.9 and Addenda 2, verification.md §4.6, PORTING.md §4.6, C30, §5.1 and §5.5, and noq 1.3.0's stream `Drop`
impls.

**Confirmed, no change needed.**
- **Dial.** The check order and every text match Go. The delay is ctx-aware, and nothing is rechecked after
  it. Both halves are tracked before the accept-queue send, and a ctx end on a full queue closes the local
  half.
- **Accept and connections.**
  - `accept` ignores the closed flag.
  - `close` runs once, untracks, and closes the peer from a task.
  - `open_stream` and `accept_stream` watch only their own half's `done`, as Go's selects do.
- **Pipes.**
  - `Write` blocks while the pipe is full, then checks cancelled before closed. `Read` checks cancelled before
    EOF. Cancel discards the buffer, and an empty write succeeds.
  - Wakers are registered and taken under the pipe lock, so no wake-up is lost. Each pipe has exactly one
    reader half and one writer half.
- **Constants.** The 64 and 256 queue limits are exact (tokio bounded mpsc). `PIPE_LIMIT` is 4 MiB, the path is
  `{true, 1 ms}`, and `mem:<short>` uses 4 bytes of hex.
- **Drop.** noq 1.3.0 agrees:
  - `SendStream::drop` finishes, or resets if the peer stopped the stream.
  - `RecvStream::drop` stops unless all data was read.
  - Mem counts a drained pipe whose writer finished as read to the end. The peer then keeps
    `io: read/write on closed pipe` instead of a reset.
- **Locks.** The order is `close_lock` → registration `conns`. `drop_all` and `drop_peer` release `conns`
  before closing anything. `set_down` and `partition` close under the network lock, which no close path takes.
- **Races.** Go's `select` picks at random when a ctx has ended and a channel is ready. `Ctx::run` returns the
  ctx error there, which is one of Go's possible outcomes.
- **Vectors.** Before the changes below, `cargo test --test golden -- transport::` passed 7/7, including
  `pool_scripts` (all 13 scripts, the five mem scripts among them).

**Fixed.**
1. **Network lifetime.** An endpoint held its network weakly. When nobody else held the `Arc<Network>` (Go's
   `transport.NewNetwork().Bind(..)`, as in `node/corrupt_test.go`, or a test helper returning only
   endpoints), every dial failed with a wrong `mem: <short> not bound`. Go cannot reach that state.
   - The private `Registration` is now split from the `MemEndpoint` handle: the network holds registrations,
     and handles hold the network. There is still no cycle.
   - `endpoints_keep_their_network_and_unheld_endpoints_stay_bound` covers the helper shape, a bound endpoint
     whose handle was dropped, and the network and connections being freed after the last handle.
   - The assertion of the old deviation was removed from `dial_error_texts_in_go_order`.
2. **Timing test.** `dial_waits_for_the_delay` asserted wall-clock upper bounds (`elapsed < 150 ms`) that a
   loaded CI machine can break. It now runs on a paused clock (PORTING.md §5.5). It also pins "nothing is
   rechecked after the delay": a peer taken down during the delay still gets the new, open connection.
3. **Coverage.**
   - `streams_opened_by_the_acceptor`: server-opened streams and `close_stream`.
   - `blocked_reads_wake_on_fin_and_on_an_aborted_peer`: a blocked read woken by FIN. Also PORTING.md §5.1:
     aborting the task that holds a stream gives the peer EOF, and its writes fail with the reset text.

**Unchanged.**
- No test was ignored, so there was nothing to un-ignore.
- dstore has no Go test for `mem.go`. Its behaviour is pinned by the vectors and the unit tests.

**Gates after the fixes.**
- `cargo test -p dstore-transport --lib -- mem::`: 24 passed, in 5 repeated runs.
- `cargo test -p dstore-transport --lib` (the whole crate): 60 passed, none ignored.
- `cargo check -p dstore-testkit` (FakeCluster holds `Arc<Network>` and `Arc<MemEndpoint>`) is clean.
- `cargo clippy -p dstore-transport --all-targets -- -D warnings` is clean.
- rustfmt (edition 2024) is clean for `mem.rs`.
- Golden rerun: `tests/golden.rs` could not be built at the time, because `dstore-client` (a sibling's
  `crates/client/src/error.rs`) was mid-edit. A scratch crate instead included the unmodified
  `tests/golden_tests/transport.rs` through `#[path]`, with path dev-dependencies on transport, transport-iroh,
  wire, view, gocompat and testkit. It passed 7/7, `pool_scripts` included, in three runs. The scratch
  crate was deleted afterwards.
- Linux has not been run; CI covers it.
