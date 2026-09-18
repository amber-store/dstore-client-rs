# impl-transport-iroh: endpoint, dial phases, connections, interfaces

Owner transport-iroh (layer L4). Files:
- `crates/transport-iroh/src/lib.rs`, `endpoint.rs`, `conn.rs`, `ifaces.rs` with their unit tests;
- `tests/iroh_loopback.rs`, `tests/golden_tests/transport_iroh.rs`;
- `[dev-dependencies]` `dstore-testkit` and `serde` in `crates/transport-iroh/Cargo.toml`, for the relay-map
  vector unit test.

`mdns.rs` belongs to transport-iroh-mdns. This crate only calls `MdnsResolver::start`, `resolve` and `close`
(`close` was added by that owner and is not in PORTING.md §4.7), plus `endpoint_label` and
`parse_announcement` in tests.

## Items added (nothing in PORTING.md §4.7 was changed)

- **`pub use iroh;` in `lib.rs`.** The §4.7 API exposes iroh types (`IrohConfig::secret_key`, `raw()`,
  `to_iroh_addr`), and the root package has no direct iroh dependency. Root tests reach
  `iroh::SecretKey::from_bytes` and `iroh::PublicKey::from_bytes` through this re-export.
- **Private fields.** The scaffold's private fields of `IrohEndpoint` were replaced. There is no `cfg` and no
  `bg` token, and `addrs` is a plain `Vec` because Go only sets it in `BindIroh`.
- **Crate-level `#![allow(dead_code, unused_variables)]` removed.** Clippy is clean without it, including
  `mdns.rs` as of this writing.

## iroh 1.2.0 APIs re-verified (iroh-1.2.0, iroh-base-1.2.0, iroh-relay-1.2.0, noq-1.3.0, noq-proto-1.3.0)

- **Builder** (`endpoint.rs`):
  - `Endpoint::builder(presets::Minimal)`: the ring crypto provider only;
  - `.secret_key`, `.alpns`, `.transport_config`, `.portmapper_config(PortmapperConfig::Disabled)`,
    `.relay_mode`, `.clear_ip_transports().bind_addr(SocketAddr)` (the error type is `Infallible`), `.bind()`.
  - `Builder::empty` binds `0.0.0.0:0` and `[::]:0` and has no address lookup.
- **Endpoint methods.**
  - `bound_sockets()` and `add_external_addr(SocketAddr)` (async; stores anything).
  - `online()` waits for a connected home relay and never returns when there is none.
  - `addr().relay_urls()`, `dns_resolver()` (`Err` once closed), `is_closed()`, `close()`.
  - `connect()` awaits `Connecting` and returns `Connection<HandshakeCompleted>`.
  - `accept()` → `Incoming::accept()` → `Accepting` (a future of `Result<Connection, ConnectingError>`).
- **Errors.** `ConnectError` wraps `ConnectWithOptsError::{SelfConnect, NoAddress, EndpointClosed, …}`
  (`stack_error`, so struct variants carry `meta`), `ConnectingError` and noq `ConnectionError`. Display texts
  are iroh's and noq's (`timed out`, `closed by peer: …`, `Connecting to ourself is not supported`).
- **Transport config.** `QuicTransportConfig::builder()` starts from iroh's multipath defaults (keepalive
  5 s, path keepalive 5 s, path idle 15 s, 8 paths) and has `keep_alive_interval`,
  `max_idle_timeout(Option<IdleTimeout>)`, `max_concurrent_bidi_streams`, `initial_rtt`,
  `stream_receive_window` and `send_window`. The noq-proto default `initial_rtt` is 333 ms.
- **RTT sentinel.**
  - `RttEstimator::get()` is `smoothed.unwrap_or(latest)`, and `latest` starts at the configured
    `initial_rtt` for the connection and for every new path (`PathData::new`).
  - A migrated path copies the previous estimator (`PathData::from_previous`).
  - `Connection::paths()` → `Path::is_selected()`, `is_relay()`, `rtt()` = `PathStats.rtt` = `get()`.
- **Relay maps.**
  - `RelayMap: FromIterator<RelayUrl>`, `RelayMap::from(RelayUrl)`, `RelayMap::empty()`.
  - `RelayConfig::from(RelayUrl)` gives `quic: Some(RelayQuicConfig { port: DEFAULT_RELAY_QUIC_PORT = 7842 })`.
  - Bind accepts `RelayMode::Custom(RelayMap::empty())`: only more than one relay transport is invalid, and
    net_report is skipped for an empty map.
- **DNS lookup.** `address_lookup::DnsAddressLookup::builder(origin).dns_resolver(r).build()`, then
  `AddressLookup::resolve(id)` → `Option<BoxStream<Result<Item, Error>>>`. `N0_DNS_ENDPOINT_ORIGIN_PROD` is
  `dns.iroh.link.` (a unit test pins the private `DNS_ORIGIN` to it).
- **Keys and addresses.** `iroh_base::PublicKey::from_bytes` (ed25519-dalek decompression, the
  `ticket::is_valid_public_key` check), `fmt_short()` (5 bytes, lowercase hex), `to_z32()`,
  `SecretKey::generate()`, `iroh_base::CustomAddr::from_parts`.
- **noq streams.**
  - `SendStream::finish()` returns `Ok` after the peer's STOP_SENDING, and `Err(ClosedStream)` when already
    finished.
  - `RecvStream::stop(code)`.
  - `Drop` finishes (or resets with the stop code) and stops with 0 when unread.
  - Error texts: `sending stopped by peer: error N`, `stream reset by peer: error N`, `connection lost`.

## Behaviour and decisions

- **`bind_iroh` order.**
  - Go registers discovery before `iroh.Bind`. Rust binds first and then starts the mDNS resolver, because
    number0 DNS needs the bound endpoint's resolver.
  - The WARN `transport: mdns discovery unavailable` with `error` Any is logged synchronously at bind rather
    than from a goroutine.
  - `announce = true` fails before anything is bound.
  - The ctx bounds only the relay online wait. An ended ctx still binds, as go-iroh `Bind` never reads its ctx
    (changed in review; it used to give `transport: bind: context canceled`).
- **Advertised port.** iroh binds separate IPv4 and IPv6 sockets where go-iroh binds one dual-stack socket,
  so `127.0.0.1:<port>` and `localAddrPorts` use the IPv4 socket's port.
- **Direct addresses.** Every direct address is listed in `addrs()` (Go appends even the ones go-iroh
  ignores). Only specified, non-zero-port ones go to `add_external_addr`, unmapped, as go-iroh's
  `canonicalNATTraversalCandidate` pins them; Rust iroh would store the others.
- **Relay strings.**
  - A home relay is advertised as the Go string of the configured URL: the canary hosts with their `/`, and
    a `--relay` URL as `netaddr.ParseRelayURL` normalised it, keeping a `:443` that `url::Url` drops.
  - Unknown URLs go through `go_relay_string` (Go normalisation of `url::Url`'s string).
  - The maps are checked against `transport/relay_urls.json` `default_map`, `default_relay_addrs` and
    `custom_url_map` in `endpoint::tests::relay_maps_match_go_vectors`.
- **Dial phases.**
  - Each phase is one `Endpoint::connect` over every convertible candidate, capped at min(ctx deadline, 10 s).
    The direct phase also respects `direct_timeout`.
  - The error is `dial <addr>: <inner>` for every candidate of the phase, joined in candidate order (relays
    first in the relay phase), with the phase's single inner error (DD-4).
  - A ctx end is the ctx text (`context deadline exceeded` / `context canceled`), which is what Go's
    `awaitHandshake` returns.
  - When no candidate converts (e.g. `relay:example.com`), every line is
    `dial <addr>: iroh: no reachable address for endpoint`.
  - Without relays, a relay candidate is left out of the connect and its line reads
    `iroh: no reachable address for endpoint` (go-iroh `dialTargets`; added in review).
  - A closed endpoint or the own id fails every line before the ctx is looked at (go-iroh `connectEarly`).
- **`discover_dial`.**
  - Without discovery an earlier phase error stands. Without one, the bare-id texts of go-iroh `connectEarly`
    apply: `iroh: endpoint closed`, `iroh: cannot connect to self`, `iroh: no reachable address for endpoint`.
  - With discovery, the same pre-checks run, then the lookup: mDNS for 3 s, plus number0 DNS with relays, in
    `futures::stream::select_all`. The first answer for the id with a non-empty address set wins.
  - Relay addresses of an answer are dropped without a relay transport (go-iroh `dialTargets`), and an empty
    set is `iroh: no reachable address for endpoint`.
  - The connect is capped like the other phases. The lookup is bounded by its own timeouts and the ctx.
  - A failure is joined to the earlier one as `…\ndiscovery: …`.
  - When the mDNS listener could not start, `MdnsResolver::without_listener` is registered, so a lookup still
    multicasts its query and waits its 3 s (Go's resolver stays registered).
  - The zero id is never looked up (go-iroh `lookupAddr`).
- **Connect error texts.** `SelfConnect`, `NoAddress` and `EndpointClosed` map to go-iroh's texts
  (`TransportError::SelfConnect`, `NoAddress`, and `Quic("iroh: endpoint closed")`). Everything else is
  `Quic(<iroh text>)`.
- **`accept`.**
  - A closed endpoint gives `iroh: endpoint closed`, Go's `ErrEndpointClosed` text. transport §4.5 suggested
    `TransportError::Closed`, but the Go source decides.
  - No ALPNs gives `iroh: no ALPNs configured; nothing to accept`.
  - A connection that dies during its handshake (`ConnectingError::ConnectionError`) is skipped, as go-iroh
    skips `ErrConnClosedDuringHandshake`.
- **`to_iroh_addr`.**
  - An IPv4-mapped IPv6 candidate is dialled over IPv4. Go's dual-stack socket reaches IPv4 there, and iroh's
    IPv6 socket may be v6-only.
  - A zone resolves by interface name (`if_nametoindex`), else by Go's `dtoi` leading digits (saturating at
    0xFFFFFF), else None.
  - An unparseable relay URL gives None, and a custom address becomes `CustomAddr::from_parts`.
- **Close.** `close_bounded(d)` closes the mDNS listener, then awaits `Endpoint::close()` under
  `tokio::time::timeout(d)`. `Endpoint::close` is `close_bounded(CLOSE_TIMEOUT)`.
- **Remembered paths (divergence).**
  - Rust iroh keeps a remote's known paths in its `RemoteStateActor` while connections exist and for an idle
    period after. So a direct-only phase at a dead address may still connect over a path this endpoint
    learned earlier. Go's per-candidate race would fail.
  - The peer is authenticated either way, so this only makes a dial succeed where Go reports failure.
  - `dial_returns_handshaken_connections_and_dead_addresses_fail` therefore uses a fresh endpoint for the
    dead-address dial.
- **Interfaces.** `interface_ips` uses `nix::ifaddrs::getifaddrs`, stably sorted by interface index, which
  gives Go's order (interface index, then OS address order). The filters are those of `transport/ifaces.go`:
  up, case-sensitive bridge prefixes, `Unmap`, dedup, loopback, link-local unicast (169.254/16, fe80::/10),
  link-local multicast (224.0.0/24, IPv6 under Go's `0xff0f == 0xff02` mask), unspecified. The unit tests
  cover the pure filter only, because glibc's `getifaddrs`/`if_nametoindex` open sockets; the root loopback
  test calls `interface_ips` itself.

## Tests

- **Unit tests.** `cargo test -p dstore-transport-iroh --lib` runs 23 tests of these modules (the rest of
  the crate's 45 are `mdns::`). None bind a socket.
  - `conn`: the path-selection loop and the RTT sentinel.
  - `endpoint`:
    - `relay_mode_of`, the relay modes, and `transport/relay_urls.json` `default_map` /
      `default_relay_addrs` / `custom_url_map` against the maps bind configures;
    - the DNS origin and the transport-config values;
    - `to_iroh_addr`, the zone index rules, `go_relay_string`, the Go `ip:` strings;
    - the external-address filter, phase error joining, announcement conversion.
  - `ifaces`: the filters over fake interface lists.
- **Golden tests** (`tests/golden_tests/transport_iroh.rs`, 2 tests, `transport/ids.json`):
  - all 81 endpoint-id rows agree with Go for `iroh::PublicKey::from_bytes` and
    `ticket::is_valid_public_key`, and rejected ids render as `data is not a valid public key`;
  - the identities give Go's id, `String`, `Short`, `Z32`, mDNS label and `no candidate addresses` text.
- **Loopback tests** (`tests/iroh_loopback.rs`): 9 tests plus 1 ignored.
  - `loopback_exchange`, `dial_returns_handshaken_connections_and_dead_addresses_fail`,
    `path_rtt_is_unknown_until_sampled`, `discover_by_id_over_mdns` port the four Go tests.
  - `dial_phase_errors` pins these texts:
    - an invalid key, and a bare id without discovery;
    - self-dials, and an unconvertible relay;
    - relay + dead direct joined in candidate order, a cancelled ctx;
    - accept without ALPNs, and a closed endpoint.
  - `streams_fin_stop_and_connection_close`:
    - FIN, then `EOF`;
    - a write after STOP_SENDING: `sending stopped by peer: error 0`;
    - CONNECTION_CLOSE reaches the peer, and `accept_stream` fails.
  - `pool_call_over_iroh`: `Pool::call` and `Pool::path` over real endpoints.
  - `bind_options`: announce refused, cancelled bind, port selection, advertise, `interface_ips` addresses.
  - `interface_ips_are_dialable_unicast`.
  - `live_go_node_view_call` (ignored) needs a Go node: `DSTORE_LIVE_STORE=<store dir> cargo test --test
    iroh_loopback -- --ignored live_go_node_view_call`.
  - `discover_by_id_over_mdns` multicasts a hand-built go-iroh announcement (checked with
    `mdns::parse_announcement` first). It skips when no mDNS listener opens or when the host's multicast does
    not loop the announcement back to a probe resolver.
- **Where they ran.**
  - The root harness passes: `cargo test -p dstore-client-rs --test golden transport_iroh` (2 passed) and
    `--test iroh_loopback` (9 passed, 1 ignored).
  - While a sibling's `crates/client` was mid-edit, the same two files ran from a throwaway scratch package
    (`[[test]] path = <repo>/tests/iroh_loopback.rs`, path dependencies on the workspace crates), since
    deleted. Three loopback runs in a row passed (about 4.5 s each), and clippy `-D warnings` was clean over
    them.
- **Timings seen on macOS arm64.** A dead-address direct phase with `direct_timeout` 500 ms fails at about
  500 ms. An unknown id with discovery fails after the direct phase plus the 3 s mDNS lookup.

## Live check against a Go dstore v0.1.9 node (2026-09-18, macOS arm64)

- **Setup.**
  - Go `dstore` built from `git archive` of `/Users/dragan/amber-store/dstore` HEAD `368f2c7` with
    `CGO_ENABLED=0` into the scratchpad.
  - `dstore cluster init --store n1 --replicas 3 --weight 10 --no-relay --loopback`, then
    `dstore serve --store n1 --no-relay --loopback`, listening on `ip:127.0.0.1:63152`.
- **Rust side.** `live_go_node_view_call` with `DSTORE_LIVE_STORE=<scratch>/live/n1`. The client uses the
  CLI's configuration (fresh key, no ALPNs, default sockets, no loopback, `discover`) with relays disabled.
  It takes the node id from `<store>/identity` and the address from `<store>/port`.
- **Result: pass.**
  - `bind_iroh` advertised the machine's interface addresses (`ip:192.168.1.223:61131`,
    `ip:10.241.247.228:61131`).
  - `Endpoint::dial` at `ip:127.0.0.1:63152` connected in 3.7 ms: `remote_id` = the Go node id, ALPN
    `amber-dstore/1`, path `{direct: true, rtt: 1.25ms}` right after the handshake.
  - `Pool::call` with an unstamped `TView` returned `TViewReply` incarnation 1, epoch 1, a 138-byte view;
    pool path `{direct: true, rtt: 305µs}`.
  - Dial by id alone resolved the Go node's own mDNS announcement through the ported resolver in 81 ms (the
    first interop run of that resolver against go-iroh), and a ping on that connection was answered.
  - The Go node logged no warning or error.
- **Cleanup.** The node was stopped, and its binary, store, source tree and log were deleted.

## Review

Reviewer: review-transport-iroh. This adversarial pass came after transport, transport-iroh-mdns and wire had
landed and been reviewed. No test in the owned files was `#[ignore]`d for a sibling. `live_go_node_view_call`
stays ignored because it needs a running Go node; it was run by hand (see "Live check").

### Sources checked

- **dstore v0.1.9:**
  - `transport/iroh.go`, `transport/ifaces.go` and `transport/iroh_test.go`;
  - `cmd/dstore/main.go` `relayModeOf` and `cmd/dstore/client.go` `dialTicket`.
- **go-iroh v0.2.0:**
  - `iroh/endpoint.go`: `Bind`, `AddExternalAddr` / `canonicalNATTraversalCandidate`, `Addr`, `Online`, `Connect` /
    `connectEarly`, `dialTargets`, `acceptIncoming` / `accept` / `finishAccepting`, `lookupAddr` and `Shutdown`;
  - `iroh/addresslookup.go` `Resolve`, `iroh/addresslookup_dns.go`, `iroh/mdns/mdns.go` `Resolve`;
  - `iroh/conn.go` `Close` and `Paths`, `relay/relay.go`, `internal/socket/relay_actor.go` `SetHomeRelay`, and
    `key/key.go` `IsZero`.
- **go1.26.5:** `net/interface.go` `ipv6ZoneCache.index`, `net/parse.go` `dtoi`, and `net/netip` `IsLinkLocalMulticast`.
- **Rust iroh 1.2.0:**
  - `endpoint.rs`, `endpoint/connection.rs` (`Incoming::accept`, `Accepting`, `ConnectingError`) and `endpoint/quic.rs`;
  - `address_lookup/dns.rs`, `socket.rs` (`resolve_remote`, `add_external_addr`, `home_relay`);
  - `socket/remote_map/remote_state.rs` (`to_transports_addr`), `socket/transports.rs` (`poll_send`) and
    `path_watcher.rs`.
- **Other crates:**
  - iroh-relay 1.2.0 `relay_map.rs` and `defaults.rs`;
  - noq 1.3.0 `finish`, `stop`, the `Drop` impls and `poll_shutdown`;
  - noq-proto 1.3.0, for the `initial_rtt` default.

### Fixed

1. **The bind ran under ctx; Go's does not.** `bind_iroh` awaited `builder.bind()` inside `ctx.run`, so an ended
   ctx gave `transport: bind: context canceled`. go-iroh `Bind(ctx, …)` never reads its ctx: Go binds, and only
   the relay online wait sees the ctx. The bind no longer runs under ctx.
2. **Relay candidates were dialled on an endpoint without relays.** `connect_phase` passed relay URLs to
   `Endpoint::connect` even with relays disabled.
   - iroh keeps such a path (`to_transports_addr`) and blackholes its datagrams: `TransportsSender::poll_send`
     returns `Ok` when no transport fits. A relay-only phase therefore waited min(ctx, 10 s) and ended with
     `context deadline exceeded`.
   - go-iroh `dialTargets` gives such a candidate no target, and `connectEarly` fails it at once with
     `iroh: no reachable address for endpoint`.
   - Now relay candidates stay out of the connect when relays are disabled, and their lines carry that text. The
     other lines still share the connect's error (DD-4).
   - The common case: a `--no-relay` client dialling a Go node whose view publishes a relay URL.
   - With discovery, go-iroh looks such a candidate up inside its `Connect`. Here `discover_dial` runs that
     lookup after the phase. The outcome is the same (a connection, or the same `no reachable address` text); it
     only comes sooner.
3. **The ctx was checked before the closed and self checks.** go-iroh `connectEarly` fails a closed endpoint or the
   own id before the ctx matters. `connect_phase` now runs `connect_precheck` first, so a self-dial under a
   cancelled ctx gives `dial <addr>: iroh: cannot connect to self`, not `context canceled`.
4. **The mDNS failure path lost the query.** When `MdnsResolver::start` failed, the endpoint registered nothing and
   slept 3 s in place of the lookup. Go still multicasts its query in that case. The transport-iroh-mdns review
   added `MdnsResolver::without_listener` for this path, and it is now registered.
5. **The zero id was looked up.** go-iroh `lookupAddr` returns at once for `EndpointID{}`, which is a valid key
   (`ids.json` `first_byte_0`). `discover_connect` now fails with `iroh: no reachable address for endpoint`
   without the 3 s lookup.
6. **`direct_timeout: Some(0)` expired at once.** Go's zero `DirectTimeout` means 2 s, so zero now maps to
   `DIRECT_TIMEOUT`.
7. **External candidates were not unmapped.** go-iroh pins `canonicalNATTraversalCandidate(addr)`, which also
   unmaps IPv4-mapped addresses. `external_candidate` now unmaps before `add_external_addr`. This only affects the
   node-side `advertise`.

### Tests added or changed

- **Unit:**
  - `direct_timeout_defaults_like_go`;
  - `external_candidates_skip_unspecified_and_port_zero_and_unmap`, replacing the filter test;
  - relay lines in `phase_errors_join_candidates_in_order`.
- **Loopback `discovery_without_answers`**, which needs no multicast loopback:
  - the zero id fails in < 1 s;
  - an unknown id waits out the 3 s mDNS lookup;
  - a relay candidate plus discovery gives
    `dial relay:…: iroh: no reachable address for endpoint\ndiscovery: iroh: no reachable address for endpoint`;
  - then a self-dial and a dial on a closed endpoint.
- **Loopback `dial_phase_errors`:**
  - a relay URL that parses, with relays disabled, fails in < 1 s;
  - the mixed relay and dead-address phase pins the relay lines `no reachable address` and the direct line
    `context deadline exceeded`;
  - a self-dial under a cancelled ctx.
- **Loopback `bind_options`:**
  - an ended ctx binds;
  - with relays enabled (the DD-13 empty map, so no network), the online wait returns at once under an ended ctx
    and after about 400 ms under a 400 ms ctx.

### Confirmed without change

- **Relays and config.**
  - `relay_mode_of`.
  - The default relay map, with QUIC port 7842 through `RelayConfig::from`. `relay_urls.json` `default_map`,
    `default_relay_addrs` and `custom_url_map` are asserted.
  - The DNS origin.
  - The transport config values, including `initial_rtt` 333 ms, which is also noq-proto's default.
- **Dial order.**
  - The direct phase runs under min(ctx, `direct_timeout`); its error is discarded only when there are relay
    candidates.
  - The relay phase lists relays first, then direct candidates.
  - `discover_dial` joins as `…\ndiscovery: …`. Without a lookup the earlier error stands; without an earlier error,
    the bare-id texts apply.
- **Lookup.**
  - The first answer with the right id and a non-empty address set wins, and errors are skipped.
  - Without relays, relay addresses of an answer are dropped, and DNS is not used.
- **`accept`.** The closed and no-ALPN texts. A handshake death is skipped (go-iroh `ErrConnClosedDuringHandshake`),
  while a post-handshake authentication failure is returned.
- **`IrohConn`.**
  - Close with code 0 and an empty reason.
  - The path loop with the RTT sentinel; `is_closed` and `closed`.
  - Stream `finish` and `stop`; noq's `poll_shutdown` is `finish`.
- **`to_iroh_addr`.** Zone rules match `ipv6ZoneCache.index` plus `dtoi` (big = 0xFFFFFF), and a 4-in-6 address is
  dialled over IPv4.
- **`ifaces`.** The netip predicates, including the `0xff0f` multicast mask, and the order.
- **Golden `transport/ids.json`.** All 81 `endpoint_ids` rows and 4 `identities` are asserted; none is skipped.
- **The four `iroh_test.go` tests** are ported. The dead-address dial uses a fresh endpoint, because transport §7
  accepts that iroh remembers a remote's paths.

### Divergences recorded, not fixed

- **`addrs()` right after bind.** go-iroh's `Addr()` includes the bootstrap home relay before it connects
  (`SetHomeRelay(urls[0])`, state Connecting), so Go lists `relay:https://aps1-1…/` even offline. iroh 1.2.0 lists a
  relay only once it has connected. This is node-side publishing; the client never reads `addrs()`.
- **Concurrent `accept`.** go-iroh refuses a second concurrent Accept with `iroh: endpoint accept loop in use`,
  where noq serves both. This is node-side only.
- **Discovery timing.**
  - Without a ctx deadline, go-iroh bounds lookup and connect together by one 10 s `ConnectTimeout`. Here the
    lookup is bounded by its own timeouts and the ctx, then the connect by the 10 s cap.
  - Go looks up a relay candidate on an endpoint without relays during the relay phase, in parallel with the
    direct dials. Here that lookup comes after the phase.

### Gates

All runs used the nix dev shell, `--offline` and a private `CARGO_TARGET_DIR` on macOS arm64.

- **Baseline, before any change:** `--test golden transport_iroh` gave 2 passed, and `--test iroh_loopback` gave 9
  passed and 1 ignored.
- **After the fixes:**
  - `cargo test -p dstore-transport-iroh --lib`: 46 passed, 0 ignored. This owner's 24 are conn 5, endpoint 14 and
    ifaces 5; the other 22 are `mdns::`.
  - `cargo test -p dstore-client-rs --test golden -- transport_iroh`: 2 passed.
  - `cargo test -p dstore-client-rs --test iroh_loopback -- --nocapture`: 10 passed, 1 ignored, about 6 s. No skip
    line was printed. The binary was then run three more times in a row, all passing.
- **Lint and format:**
  - `cargo clippy -p dstore-transport-iroh --all-targets --no-deps -- -D warnings`: clean.
  - `cargo clippy -p dstore-client-rs --test iroh_loopback --test golden --no-deps -- -D warnings`: clean.
  - `rustfmt --check --edition 2024` over the six owned Rust files: clean.

### Live check

This check was re-run after the fixes (2026-09-18, macOS arm64), because they touch bind, the dial phases and the
discovery wiring.

- **Setup.**
  - Go `dstore` was built from `git archive` of `/Users/dragan/amber-store/dstore` HEAD `368f2c7` with
    `CGO_ENABLED=0 GOPROXY=off` into the scratchpad.
  - `cluster init --store n1 --replicas 3 --weight 10 --no-relay --loopback`, then
    `serve --store n1 --no-relay --loopback`. The node listened on `ip:127.0.0.1:57937`.
- **Run:** `DSTORE_LIVE_STORE=<scratch>/live/n1 iroh_loopback --ignored live_go_node_view_call --nocapture`.
- **Result: pass.**
  - `bind_iroh` advertised `ip:192.168.1.223:59644` and `ip:10.241.247.228:59644`.
  - `Endpoint::dial` connected in 3.6 ms, with path `{direct: true, rtt: 1.32ms}`.
  - `Pool::call` with TView returned TViewReply: incarnation 1, epoch 1, a 138-byte view. Pool path
    `{direct: true, rtt: 473µs}`.
  - Dialling by id alone resolved the node's own mDNS announcement in 81 ms, and a ping on that connection answered
    (TPong).
  - The node log has no WARN or ERROR line.
- **Cleanup.** The node was stopped, and its binary, store, source tree and logs were deleted. So was this review's
  cargo target directory.

### Remaining

- Linux runs of `tests/iroh_loopback.rs` are left to CI, including the multicast skip path of
  `discover_by_id_over_mdns`.
- The go-iroh mDNS socket tests `TestListenIPv6MDNS` and `TestReadLoopCachesAnnouncementOverIPv6`. The mDNS review
  defers them to this file in L6.
- Relay paths, the online wait against real relays, and number0 DNS discovery were not exercised live.
