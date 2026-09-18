# impl-transport-iroh-mdns: `dstore_transport_iroh::mdns`

Owner transport-iroh-mdns (layer L4). Files:

- `crates/transport-iroh/src/mdns.rs`, with its unit tests.
- `tests/golden_tests/transport_mdns.rs`.

It ports the go-iroh v0.2.0 `iroh/mdns` resolver: `mdns.go`, `dnsmsg.go`, `reuse_unix.go`, plus
`dns/endpointinfo.go` (address de-duplication, `NewUserData`), `key/key.go` (`ParseEndpointID`) and x/net v0.56.0
`ipv4`/`ipv6` `JoinGroup`. The responder and publisher are node-side and not ported (transport Addenda 2), so a
Rust client answers no query, as a passive go-iroh `Discovery` does.

## Items added to PORTING.md §4.7 (no signature changed)

- **`pub fn MdnsResolver::close(&self)`.** It stops the listener, as cancelling the ctx given to go-iroh's `Start`
  does: the read loops end and the sockets close.
  - The cache stays usable. A later `resolve` answers from it, and a miss multicasts its query from a fresh socket
    per family (Go's `writeMulticast` without `Start`), then waits out the timeout.
  - Why: the endpoint holds `Arc<MdnsResolver>` behind `&self` and needs to stop discovery on close without
    dropping the `Arc`. Dropping the last `Arc` does the same.
- **`pub fn MdnsResolver::without_listener(logger: Logger) -> Arc<MdnsResolver>`** (added in review). A resolver
  that never listens, which is go-iroh's registered `Discovery` whose `Start` failed.
  - Its `resolve` multicasts the query from a fresh socket per family and waits out the timeout, because nothing
    fills the cache.
  - Why: dstore registers the Discovery before `Start` runs and keeps it when `Start` fails. `start` returns
    `Err` in that case, and without this item the endpoint would need a stand-in sleep, which loses the query.
- **Private fields.** The scaffold's fields are replaced by `MdnsResolver { shared: Arc<Shared> }`, where `Shared`
  holds the logger, the cache, the query transmitter and the background token. The read loops hold `Shared`, not
  the resolver, so they never keep the resolver alive.

## Notes for the transport-iroh owner (`endpoint.rs`)

- **Binding.** `start` binds and joins before it returns, where Go runs `Start` in a goroutine. Log an `Err` as
  WARN `transport: mdns discovery unavailable` with `Attr::any("error", text)`.
- **When `start` fails.** Go keeps the resolver registered, so a lookup still sends its query and waits out its 3 s
  timeout (transport §4.6, §8 item 7). Register `MdnsResolver::without_listener(logger)` in its place and treat it
  like a started resolver.
- **Read failures.** A read error on the IPv4 socket after `start` is logged by the module with the same WARN
  line, because Go's dstore goroutine logs `Start`'s return value. The listener then stops. An IPv6 read error
  only ends that loop, as in Go.
- **Lookups.**
  - Call `resolve(ctx, id, MDNS_LOOKUP_TIMEOUT)`.
  - Skip an announcement whose `addrs` is empty. Only malformed A/AAAA records can produce one; see "Parser" below.
  - `Announcement.relay` is the `ParseRelayURL`-normalised string, without the `relay:` prefix.
  - On endpoint close, call `close()`.
- **Manifest.** `crates/transport-iroh/Cargo.toml` `[dev-dependencies]` carries `tokio` with `test-util` for the
  paused-clock resolve tests. It is in the same table as your `dstore-testkit` and `serde`; both were added at the
  same moment, and the review merged the resulting duplicate table into one.

## Decisions

### Sockets

- **Listen** (`listenMDNS`, Go `net.ListenConfig.ListenPacket`) runs in Go's order:
  1. `socket(AF, SOCK_DGRAM, 0)`;
  2. `IPV6_V6ONLY` for udp6, with its error ignored;
  3. `SO_BROADCAST`;
  4. the control function: `SO_REUSEADDR`, then `SO_REUSEPORT`, returning the first failure;
  5. `bind` to `0.0.0.0:5353` or `[::]:5353`.
- **Listen error texts** follow Go's `net.OpError`, wrapped by go-iroh. Steps 1, 3 and 5 are `os.NewSyscallError`s;
  the control function returns a bare errno. Examples:
  - `mdns: listen udp4: listen udp4 0.0.0.0:5353: bind: address already in use`;
  - `mdns: listen udp6: listen udp6 [::]:5353: permission denied`.

  Errno texts come from `gocompat::errno`.
- **Joins.** x/net's `JoinGroup` uses `MCAST_JOIN_GROUP` with a `struct group_req` naming the interface by index,
  on Darwin and on Linux.
  - libc 0.2.189 defines neither `group_req` nor Apple's `MCAST_JOIN_GROUP` (80), so `mdns.rs` defines both.
    Compile-time asserts check the struct size against x/net's `sizeofGroupReq`: 0x84 on Darwin (packed to 4) and
    0x88 on 64-bit Linux.
  - The group sockaddr sets `sin_len`/`sin6_len` on Darwin, as x/net's `setGroup` does.
  - socket2's `join_multicast_v4_n` is not used. It passes `ip_mreqn` to `IP_ADD_MEMBERSHIP`, which XNU reads as
    `ip_mreq`, so the interface index would be lost.
- **`joinGroup`.** It joins on every interface that is up and multicast-capable, in index order.
  - When none accepts, it retries with index 0. Only that fallback's error is returned:
    `mdns: join ipv4 multicast: setsockopt: <errno>`.
  - Interfaces come from `nix::ifaddrs::getifaddrs`: the flags of each name's first entry, and the index from
    `if_nametoindex`.
  - A listing error gives no interfaces, then the fallback. Go ignores `net.Interfaces` errors the same way.
- **Other socket options.** Neither side sets `IP_MULTICAST_IF`, TTL or loop options.
- **Unsafe code** is confined to `setsockopt` and the sockaddr writes, which PORTING.md §3.4 allows for socket
  options.

### Resolver

- **Cache.** Every datagram of up to 1500 bytes is parsed; longer ones are truncated, as Go's `readLoop` buffer
  truncates them. An announcement replaces its id's cache entry. Nothing is ever evicted.
- **`resolve`** (go-iroh `Resolve`):
  1. The cache is checked first, even when the ctx has ended.
  2. A miss sends the query even on an ended ctx: `tokio::spawn`, fire and forget, write errors ignored.
  3. Then it polls the cache on a 25 ms interval (first tick after 25 ms, missed ticks skipped) until the timeout
     or the ctx ends. An unbiased `select!` stands in for Go's random `select`.
  - A zero timeout means go-iroh's 10 s default: `WithLookupTimeout` ignores non-positive values.
  - Go yields a ctx error item where this returns `None`; dstore's `lookupAddr` skips that item.
- **Queries.** While listening, a query goes to 224.0.0.251:5353 on the IPv4 socket, then to [ff02::fb]:5353 on the
  IPv6 socket if there is one. After `close`, after a failed IPv4 read loop, and from `without_listener`, it goes
  out from a fresh wildcard socket per family (`writeOnce`).

### Parser (`parseAnnouncement` + `infoFromAnnouncement`)

- **Structure.**
  - Questions are skipped, then ANCOUNT + NSCOUNT + ARCOUNT records are read together.
  - One malformed record refuses the packet. Trailing bytes are ignored, and flags are not checked.
  - `readName` takes at most 32 steps, labels and pointers together, and follows forward pointers. PTR and SRV
    target names are read from the whole packet.
- **Instances and addresses.**
  - An instance must end in `._irohv1._udp.local` exactly, and its label must parse as an endpoint id.
  - Its last SRV must carry a non-zero port. Its target must own A/AAAA records; host names match exactly.
  - Addresses keep record order and are de-duplicated. go-iroh's `IPAddr.Compare` compares whole `netip.Addr`s, so
    an A record and the IPv4-mapped AAAA of the same address stay distinct, as with `SocketAddr` equality.
- **TXT.** The last TXT record of an instance replaces earlier ones, and within one record the last `relay=` or
  `user-data=` entry wins. Keys are case-sensitive.
  - `relay` goes through `dstore_transport::addr::parse_relay_url` and is dropped when it does not parse.
  - User data longer than 245 bytes is dropped.
- **Simplifications proved equivalent to Go.**
  - `strings.EqualFold(name, "_irohv1._udp.local")` is an ASCII case-insensitive compare. The name has no `k` or
    `s`, the only letters with non-ASCII case partners.
  - Go's `HasSuffix(ToLower(inst), suffix)` followed by the case-sensitive `TrimSuffix` accepts exactly the names
    that end in the suffix: an untrimmed name keeps `.` and `_`, and never parses as an endpoint id.
- **Label parsing** follows `key.ParseEndpointID`:
  - 64 bytes are decoded as hex;
  - anything else goes through `gocompat::strings::to_upper`, then Go's base32 decoder (`gocompat::base32`), and
    must give 32 bytes;
  - the bytes must pass `iroh_base::PublicKey::from_bytes`.
  - Consequences: z-base-32 labels are refused, and a label with U+0131 in place of an `i` is accepted, because Go's
    `ToUpper` maps it to `I`.
- **Divergences only malformed input can observe.**
  - An A/AAAA record with a wrong rdata length gives Go an invalid `netip.Addr`. That address counts for "at least
    one address" and stays in the address set. Here it is dropped, so the announcement can come back with fewer
    addresses, even none.
  - Relay or user-data bytes that are not UTF-8 are converted lossily after the length check (DD-8).
  - When several instances qualify, the first one seen wins. Go iterates a map (DD-10).

## Tests

- **Unit tests** (`cargo test -p dstore-transport-iroh --lib -- mdns::`): 22 tests, none ignored, no sockets.
  - Resolver tests use a `#[cfg(test)]` transmitter that records queries instead of opening sockets. They run on
    tokio's paused clock (`start_paused`) and assert Go's timing exactly.
  - Ported go-iroh tests:
    - `TestAnnouncementRoundTrip`, with and without a relay, using a test-only port of `buildAnnouncement`;
    - `TestDiscoveryResolveFromPacket`;
    - `TestServiceNameIsolation`;
    - `TestEndpointLabelMatchesRustMDNS`, extended with the `ParseEndpointID` edges;
    - the caching half of `TestHandlePacketAnswersQueries`.
  - New tests:
    - `readName` step limits and pointers, TXT rules, parse details;
    - Go-verified label edges;
    - interface selection, error texts, the query layout;
    - resolve (miss and timeouts, 25 ms polling, ctx deadline, cancel and ended ctx);
    - close and drop, and `without_listener`.
- **Not ported.**
  - Node-side responder and publisher tests: `TestDiscoveryAnnouncementUsesLocalID`,
    `TestAnnouncementInfoPreservesIPv6Zone`, `TestAnnouncementPort`, `TestPublishLogsWhatItCannotAnnounce`,
    `TestAnswerForQuery`, and the answering half of `TestHandlePacketAnswersQueries`. The golden `announcements`
    rows cover what they produce, as parser input.
  - Socket tests: `TestListenIPv6MDNS` and `TestReadLoopCachesAnnouncementOverIPv6`. They belong in root
    `tests/iroh_loopback.rs` (layer L6). Through the public API: `MdnsResolver::start`, send a go-iroh announcement
    unicast to `127.0.0.1:5353` or `[::1]:5353`, then `resolve` must answer at once.
- **Golden tests** (`tests/golden_tests/transport_mdns.rs`, 8 tests):
  - `names` and `queries`;
  - every `parse` case, split into packets with and without a `relay=` entry. Accepted cases also check `addrs`,
    go-iroh's `EndpointData` strings;
  - every go-iroh-built announcement packet parsed back to its `announcementInfo`: the id, the addresses without
    zones and de-duplicated, the relay and the user data;
  - the error rows.

  `questions` (`parseQuestions`) is node-side and not read.
- **Checks run** (implementer): `cargo clippy -p dstore-transport-iroh --all-targets --no-deps -- -D warnings` and
  `rustfmt --check --edition 2024` over both files. The review's results are under "Review".
- **Real-socket smoke test** (implementer; scratch programs outside the repository, deleted afterwards), on macOS
  arm64.
  - The peer was a go-iroh v0.2.0 `Discovery` publishing 192.0.2.7:4242, [2001:db8::7]:4242, 10.9.9.9:5555, relay
    `https://Relay.Example.:8443` and user data `smoke`.
  - `MdnsResolver::start` bound both families.
  - The first `resolve` sent the query and got Go's answer after 53 ms: the two 4242 addresses (Go dropped 5555),
    relay `https://relay.example.:8443/` and user data `smoke`.
  - A cache hit took microseconds, and an unknown id waited out its 500 ms.
  - A second `start` in the same process succeeded (SO_REUSEPORT), and the cache still answered after `close`.

## Review

Reviewer: review-transport-iroh-mdns.

### Sources checked

- go-iroh v0.2.0: `mdns.go`, `dnsmsg.go`, `reuse_unix.go`, `mdns_test.go`, `dns/endpointinfo.go` (`contains` →
  `netip.AddrPort.Compare`, `NewUserData`), `key/key.go` and `key_core.go` (`decodeBase32OrHex` =
  `stdBase32NoPad.DecodeString(strings.ToUpper(s))`, `NewPublicKey`), and `iroh/endpoint.go` `lookupAddr`.
- x/net v0.56.0: `ipv4`/`ipv6` `dgramopt.go`, `sys_ssmreq.go`, `sys_darwin.go`, `sys_linux.go`, `zsys_*`, and
  `internal/socket/rawconn.go`.
- dstore v0.1.9 `transport/iroh.go` `setupDiscovery`.

### Confirmed

- **Parser.** `parseDNS`, `readRR`, `readName`, `parseTXT`, `parseAnnouncement` and `infoFromAnnouncement` match,
  including both simplifications, last SRV and last TXT winning, address de-duplication, and the relay and
  user-data rules. All 51 `parse` cases and 17 `announcements` rows are asserted; none is skipped.
- **Label parsing.** It matches `key.ParseEndpointID`. The curve check is the one `dstore_ticket::is_valid_public_key`
  uses, which `ticket/curve.json` pins.
- **Joins.**
  - x/net sets `MCAST_JOIN_GROUP` at `IPPROTO_IP` / `IPPROTO_IPV6` with a `group_req` by index: value 80 and
    0x84 packed with `sin_len` on Darwin, value 42 and 0x88 on 64-bit Linux.
  - Errors reach go-iroh as `setsockopt: <errno>` (`Option.set`).
  - libc 0.2.189 exports `MCAST_JOIN_GROUP` for `linux_like`, and linux `sockaddr_storage` is 128 bytes aligned to
    8, so the 0x88 assert holds.
- **Listen.** The system-call order and the `net.OpError` texts match.
- **Resolver.**
  - Cache before ctx; the query is sent even on an ended ctx.
  - The timer starts after the query, and the ticker's first tick comes after 25 ms.
  - A non-positive timeout means the 10 s default.
  - The read-loop WARN mirrors dstore's `md.Start` logging, and `writeOnce` is used once `Start` is not running.

### Go probe

A scratch module reached `parseAnnouncement` through `go:linkname`; it was run with `go run` and deleted. Results:
- C4 trailing bits (`…b`, `…p`): accepted.
- `\n` and `\r\n` inside a label: accepted.
- 52 base32 symbols plus 12 `\n` (64 bytes, so decoded as hex): refused.
- U+017F in place of `s`: accepted. U+212A in place of `k`: refused.
- A trailing 0xFF byte: refused.
- Wrong-length A/AAAA rdata: Go keeps an invalid address in the set, the documented divergence.

`label_edges_match_go` now pins these, starting from the probe's packet bytes.

### Fixed or added

1. **Timing tests on the real clock.** PORTING.md §7 asks for `tokio::time::pause()`; the tests had loose
   `< 2 s` bounds. They now run with `start_paused` and assert:
   - timeouts: 80 ms, `MDNS_LOOKUP_TIMEOUT` = 3 s, and zero → 10 s;
   - an announcement heard at 60 ms is found by the 75 ms poll;
   - a ctx deadline ends the lookup at 60 ms, and a cancel at 40 ms;
   - a cache hit and an ended ctx return at 0 ms.

   This needed `tokio` `test-util` as a dev-dependency of dstore-transport-iroh. The transport-iroh owner added
   its own `[dev-dependencies]` table at the same moment, so the duplicate table was merged into one.
2. **`MdnsResolver::without_listener`** (see "Items added"). The failure path of `start` had no Go-faithful
   counterpart for the endpoint to register.
3. **Golden `addrs`.** The informational `addrs` of accepted `parse` cases were loaded but never compared. They are
   now asserted.
4. **Unit tests.** `label_edges_match_go` and `without_listener_never_listens` are new. The stale comment about an
   "ignored variant" is fixed.

### Accepted as they are

- The documented malformed-input divergences.
- The `unsafe` blocks: layout asserts, writes inside `sockaddr_storage`, `setsockopt` with the struct's size.
- `start` doing its few socket system calls inline in an async fn.

### Review results

- `cargo test -p dstore-transport-iroh --lib -- mdns::`: 22 passed, 0 ignored.
- Golden tests: 8 passed, 0 ignored.
  - The implementer's run used the root binary (`cargo test --test golden -- transport_mdns`). At the end of this
    review that binary no longer built: a sibling's `crates/client/src/error.rs` was mid-edit (E0106), and 2
    retries a minute apart failed the same way.
  - So the same module ran from a scratch crate outside the repository, with path dependencies on dstore-testkit
    and dstore-transport-iroh and the workspace `Cargo.lock`, then was deleted: 8 passed, and clippy
    `-D warnings` was clean.
  - Re-run the root binary once `dstore-client` compiles.
- `rustfmt --check --edition 2024` over both files: clean.
- `cargo clippy -p dstore-transport-iroh --all-targets --no-deps -- -D warnings`: clean.

### Remaining

- The socket tests for `tests/iroh_loopback.rs` (L6), described under "Tests".
- The Linux `cfg` paths are compiled only by CI.
- The review did not repeat the real-socket smoke test: the socket code is unchanged.
