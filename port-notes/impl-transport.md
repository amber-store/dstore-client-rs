# impl-transport: `dstore_transport` traits, `Pool`, `addr`, errors

Owner transport (layer L3). Files: `crates/transport/src/{lib.rs, traits.rs, pool.rs, addr.rs, error.rs}` with
their unit tests, and `tests/golden_tests/transport.rs`. `mem.rs` belongs to transport-mem
(`impl-transport-mem.md`).

## Items added (no PORTING.md §4.6 signature changed)

- `impl std::fmt::Debug for Stream` (prints `Stream { .. }`), so `Result<Stream, _>` can be matched and
  printed in tests and by callers.
- `crates/transport/Cargo.toml` gains `[dev-dependencies] tokio = { workspace = true, features = ["test-util"] }`
  for the paused-clock pool tests.
- `lib.rs`: the crate-wide L0 `#![allow(dead_code, unused_variables)]` is removed (every module is
  implemented).

## Decisions

**Pool** (`transport/transport.go:63-243`, ported step by step):

- **Lookups.** `get` filters the key's list in place (`retain(!is_closed)`), round robins once
  `per_peer` conns are live, and refuses with `transport: peer recently unreachable` only while no conn is
  live and the last failure is less than 2 s old. It then takes the per-key dial lock, checks the
  **unfiltered** list and returns element 0 without round robin, and otherwise dials with `addrs(id)`
  called outside the pool lock.
- **Counters and locks.**
  - The round-robin counter is a `usize` that is never reset (`wrapping_add`).
  - Dial locks are `tokio::sync::Mutex<()>`, never deleted, and awaited outside any `ctx.run`.
  - The pool lock is a `std::sync::Mutex`, never held across an await. A poisoned lock is recovered,
    because every update is a single map operation.
- **Failure marks** use `tokio::time::Instant`, so `tokio::time::pause` tests apply. A successful dial
  removes the mark, and so does `drop_peer`. `close` keeps marks, counters and dial locks, and the pool stays
  usable. `close` iterates a `HashMap`, in random order as in Go (DD-10).
- **`call`.**
  - `write_msg` is not under ctx, as in Go, and neither a write error nor a read error drops the peer.
    Only an `open_stream` failure drops it, in both `call` and `open`.
  - The read is `ctx.run(read_msg(..))`. When ctx ends first: `cancel_read(0)`, `finish`,
    `CallError::Ctx`.
  - A `TErr` reply becomes `CallError::Remote(wire::error_from_msg(&reply))`. Go also returns the reply;
    the remote error carries code, text, view and retry_after.
  - `wire.CloseStream` (finish, then `cancel_read(0)`) runs from a drop guard, so it also runs when the
    `call` future is dropped. Go has no counterpart, and noq's and mem's drop semantics would finish or stop
    the halves anyway.

**addr** (go-iroh v0.2.0 `netaddr`, go1.26.5 `net/netip` and `net/url`), hand-ported over bytes, so no input
can panic:

- **Addresses.**
  - `parse_addr_port` follows `splitAddrPort` then `ParseAddrPort`: the port goes through
    `gocompat::strconv::parse_uint(port, 10, 16)` before the address is parsed, which is Go's error order.
  - `ParseAddr`, `parseIPv4Fields` and `parseIPv6` keep every error text and `(at "…")` suffix.
  - Formatting is `AddrPort.String`: `::ffff:a.b.c.d` for IPv4-mapped addresses, and otherwise RFC 5952
    with the first longest zero run elided and `%zone` appended.
- **Custom addresses.** `ParseCustomAddr` uses `strconv.ParseUint(id, 16, 64)` (no prefix, no sign, no
  underscore) and `hex.DecodeString`. Every failure becomes `invalid ID` / `invalid data`.
- **Relay URLs.** `parse_relay_url` is `url.Parse` (fragment cut, control bytes, `getScheme`, force
  query, opaque, authority, `parseHost`, `setPath`/`setFragment`), then `normalizeURL`, then `URL.String`.
  - GODEBUG `urlstrictcolons` takes its go1.26 default (dstore's `go 1.26.5`): a host with several
    colons fails at the first colon for http and https, and at the last colon for other schemes.
  - The host is lower-cased with `gocompat::strings::to_lower`: Go's simple case mapping, with invalid
    UTF-8 turned into U+FFFD, so `https://%C3.com` gives `https://%EF%BF%BD.com/`.
  - The result is valid UTF-8: every part is either escaped to ASCII or a sub-slice of the input cut at
    ASCII delimiters.
- **`parse_addrs`** keeps the input order, deduplicates nothing, and skips silently. A string that
  `parse_transport_addr` refuses is still accepted when `parse_addr_port` parses it.

**errors**: `joined` is `errors.Join`'s layout, the texts separated by "\n". `DialAddr` and `Discovery` expose
their inner error through `source()`.

## Tests

- **Unit tests** (`cargo test -p dstore-transport --lib`), besides mem's:
  - `addr`: the go-iroh netaddr tests `TestRelayURLNormalization`, `TestTransportAddrStringRoundTrip`,
    `TestCustomAddrParseErrors`, `TestCustomAddrStringPrefix` and `TestTransportAddrTextRoundTrip`; transport
    §5.4; RFC 5952 layouts; a `ParseAddr` table (32 rows); a `ParseRelayURL` table (57 rows plus 5
    `ParseTransportAddr` rows); should-escape spot checks.
  - The expected rows of the `ParseAddr`, layout and `ParseRelayURL` tables were printed by go1.26.5 and
    go-iroh v0.2.0 in a throwaway probe module under the scratchpad (since deleted). They cover what the
    vectors do not: embedded IPv4, zones in URLs, userinfo, opaque and relative URLs, escapes and non-ASCII
    hosts.
  - `pool`, over a fake endpoint/conn/stream:
    - growth then round robin; `per_peer` 0 → 1;
    - the 2 s window at 1999 ms and at 2000 ms (paused clock), and a success clearing the mark;
    - the window ignored with a live conn; closed conns filtered with the counter never reset;
    - `drop_peer`, `path` and `close` semantics; `addrs(id)` passed to `dial`;
    - the second check against the unfiltered list (three concurrent gets, `futures::poll!`); the dial lock
      ignoring a cancelled ctx;
    - `open`/`call` dropping only on `open_stream` failure;
    - `call` reply, TErr → `Remote`, EOF, write error and ctx deadline, each with the exact stream event
      sequence (finish, cancel_read order);
    - `Send` futures.
  - `traits` (helper order) and `error` (every text and `source()`).
- **Golden tests** (`cargo test --test golden -- transport::`):
  - `transport/addrs.json`: `parse_transport_addr` + `ParseAddrs([input])`, `parse_addrs`, `parse_addr_port`.
  - `transport/relay_urls.json`:
    - `parse_relay_url`;
    - the Go strings of `GO_DEFAULT_RELAYS` and of the `--relay` map, with `RELAY_QUIC_PORT` (the maps
      themselves are transport-iroh's).
  - `transport/pool_scripts.json`:
    - the 13 scripts, driven by a harness that mirrors `tspRunScript`: pong/err/eof servers and a scripted
      endpoint that counts attempts, numbers dials and fails the next dial;
    - `rtt_round` through `gocompat::time`;
    - `error_texts` rebuilt from `TransportError` values.
  - A TErr step's Go `reply` is only checked for consistency with its `remote`.
  - The root package has no `async-trait` dev-dependency, so the harness endpoint implements the trait's
    `#[async_trait]` expansion by hand.

## Status

- `cargo test -p dstore-transport --lib`: 57 passed, none ignored. Besides this owner's tests these include
  transport-mem's 21. The pool `call` tests that needed `dstore_wire::frame` were un-ignored once wire landed.
- `cargo test --test golden -- transport::`: 7 passed (`parse_transport_addr_and_parse_addrs`,
  `parse_addr_port`, `parse_relay_url`, `relay_map_strings`, `pool_scripts`, `rtt_round`, `error_texts`).
- `cargo clippy -p dstore-transport --all-targets -- -D warnings` is clean, and so is rustfmt (edition 2024)
  for every file this owner owns.
- `cargo clippy -p dstore-client-rs --test golden --no-deps -- -D warnings` reports nothing in
  `tests/golden_tests/transport.rs`. At the time of checking, the binary as a whole still failed on lints in
  other owners' golden modules.

## Review

Reviewer: review-transport. This adversarial pass came after wire, view and transport-mem landed. The owned files
had no `#[ignore]`d tests, so nothing needed un-ignoring.

### Checked line by line against the Go sources

**`Pool` against `transport/transport.go:63-243`.** No defect found. Checked:
- the in-place filter;
- round robin over the filtered list, with a counter that is never reset (also across `Drop` and `Close`);
- the 2 s window, compared with `<` and applied only while no conn is live;
- a dial lock that is created once, never deleted and not ctx-aware;
- the second check against the unfiltered list, returning element 0;
- `addrs(id)` called outside the pool lock;
- the failure mark: set on a dial error, cleared on success and by `Drop`;
- `Close` swapping the map, and `Path` taking the first live conn under the lock;
- `Call`:
  - an `open_stream` failure drops the peer;
  - `WriteMsg` does not run under ctx and never drops;
  - then `CloseWrite`;
  - when ctx ends: `CancelRead(0)`, then `Close`;
  - a TErr becomes `Remote`;
  - the deferred `CloseStream` (finish, then `cancel_read(0)`) runs on every exit;
- `Open`, with the same drop rule.

**`addr` against go1.26.5 and go-iroh v0.2.0.** No defect found.
- `net/netip`: `splitAddrPort`, `ParseAddrPort`, `ParseAddr`, `parseIPv4Fields`, `parseIPv6`, `AddrPort.String`,
  `appendTo6`, `appendTo4In6`.
- `net/url`: `Parse`, `parse`, `getScheme`, `stringContainsCTLByte`, `parseAuthority`, `validUserinfo`, `parseHost`,
  `validOptionalPort`, `unescape`, `escape`, `setPath`/`EscapedPath`, `setFragment`/`EscapedFragment`,
  `validEncoded`, `String`. `shouldEscape` was checked against `gen_encoding_table.go`.
- go-iroh `netaddr`: `ParseTransportAddr`, `ParseCustomAddr`, `customString`, `ParseRelayURL`, `normalizeURL`.
- GODEBUG `urlstrictcolons`: `internal/godebugs/table.go` has `Changed: 26, Old: "0"`, and the `go.mod` of both
  dstore and vectorgen says `go 1.26.5`. Strict colons for http and https is therefore the right default.

**Golden harness against `tools/vectorgen/family_transport.go`.**
- Compared with `tspScriptEndpoint`, `tspServe`, `tspHandle`, `stepCtx`, `run`, `tspStreamOp` and `tspRunScript`.
- Every op's outcome fields are asserted, and so is the ok/error of every repeated run. That is stronger than the
  generator's check.
- The 13 script names are pinned. A TErr step's Go `reply` is checked against `remote`.

### Independent re-verification

A throwaway probe module re-printed every hand-written Go row of the `addr` unit tables, with 0 mismatches:
- 32 `ParseAddr` rows;
- 57 `ParseRelayURL` rows;
- 5 `ParseTransportAddr` rows;
- 11 IPv6 layouts.

It used go1.26.5 and go-iroh v0.2.0, lived in the scratchpad and has been deleted.

### Changes

- **`addr` tests: 85 adversarial rows** printed by the same probe. All pass against the unchanged code.
  - 28 `ParseAddr`: zones after embedded IPv4, `::` at either end, trailing garbage, over-long groups, spaces.
  - 37 `ParseRelayURL`:
    - `RawPath`/`RawFragment` retention, including `%2F`, `%7e` and `#` inside a fragment;
    - control characters;
    - strict colons for http but not for ws;
    - `%41` in a host, NBSP and U+0080;
    - opaque and host-less forms, and a `+` in the scheme.
  - 11 `ParseTransportAddr`: an empty kind, upper-case hex data, the u64 maximum id, a doubled `_`.
  - 9 RFC 5952 layouts: tied zero runs, IPv4-mapped edges.
- **`addr` tests: two more go-iroh ports,** `TestCustomAddrRoundTrip` and `TestRelayURLEqualCompare`.
  - Not ported, because §4.6 has no counterpart:
    - `TestCustomAddrBinary`;
    - `TestTransportAddrNetAddr`, which tests `Network()` (`is_relay` covers the kind);
    - `TestEndpointAddr*` and `TestTransportAddrOrderingMatchesRustOrd`;
    - the `Host()` half of `TestRelayURLNormalization`.
  - `TestDefaultMapHasN0Relays` is covered by golden `relay_map_strings` (the map belongs to transport-iroh).
- **`pool` test `drop_and_close_keep_the_round_robin_counter`.** Go keeps `next` in both `Drop` and `Close`, and no
  test covered it.
- **Golden harness `stream_write`.** It now loops as Go's `pipe.Write` does and reports the bytes written before a
  failure. Before, it made one `poll_write` and reported `n = 0` on an error. No vector changed.

### Gates

- `cargo test -p dstore-transport --lib`: 63 passed, 0 ignored. The count includes transport-mem's tests.
- `cargo clippy -p dstore-transport --all-targets --no-deps -- -D warnings`: clean.
- `rustfmt --check --edition 2024` over the owned files: clean.
- Golden, before the harness change: `cargo test -p dstore-client-rs --test golden -- transport::` gave 7 passed, and
  `cargo clippy -p dstore-client-rs --test golden --no-deps -- -D warnings` was clean for the whole binary.
- Golden, after the harness change: `cargo test -p dstore-client-rs --test golden -- transport::` gave 7 passed, 0
  ignored. The first attempt did not build because another owner was mid-edit in `crates/testkit/src/fake.rs`
  (`local_ref_put`); the rerun built once that landed. `cargo clippy -p dstore-client-rs --test golden --no-deps --
  -D warnings` is clean for the whole binary (0 errors), which resolves the implementer's remaining note about
  `worktree.rs` lints.
- Still open (not this owner's): Linux byte identity of the vectors is left to CI. `error_texts` checks only the
  layout of `client: no bootstrap node answered: …`; the wrapper error type belongs to dstore-client.
