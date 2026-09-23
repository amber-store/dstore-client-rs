# Families `view`, `placement` and `transport` (owner: vectorgen-view)

Sources: `tools/vectorgen/family_view.go` (families `view` and `placement`) and
`tools/vectorgen/family_transport.go` (family `transport`). Regenerate with:

```sh
nix develop -c go -C tools/vectorgen run . ../../tests/golden view placement transport
```

| Family | Files | Wall time |
|---|---|---|
| `view` | `view/view_placement.json` | about 1 s |
| `placement` | `placement/all_slots.json` | about 13 s (26 s CPU, one goroutine per set) |
| `transport` | `transport/addrs.json`, `transport/relay_urls.json`, `transport/ids.json`, `transport/mdns.json`, `transport/pool_scripts.json` | about 3 s, including a real 2.1 s sleep |

The conventions of `VECTORS.md` apply:
- 64-bit integers are decimal strings, marked `<u64>` / `<i64>` below.
- Bytes are lowercase hex (`<hex>`, `<hex32>` for 32 bytes), and `null` means Go nil where a field says so.
- Texts are the exact Go bytes.

There are two exceptions:
- **Placement ids are not ed25519 keys.** They follow port-notes/view-placement.md §5: splitmix64
  `data(seed, 32)`, or 32 copies of one byte (`idFrom(b)`). Placement never validates ids. Only the `salt`
  section also lists the ed25519 ids `id(1..8)` of verification §5.
  Transport ids are ed25519 keys (`NewKeyFromSeed(data(seed, 32))`), except the seed `01 02 … 20` of
  transport §5.6.
- **Go strings that may be invalid UTF-8** go in `input_hex` (`parse_node_id`).

Every output is deterministic: two runs give identical bytes.

---

## `view/view_placement.json`

**Go:**
- dstore v0.1.10 `placement` (`Fmix64`, `Log2Fix`, `Slot`, `Salt`, `L`, `NewSet`, `Set.Rank`, `Set.Owners`);
- `view` (`Encode`, `Decode`, `NewPlacement`, the lookups and helpers, `ParseNodeID`, `ShortID`, `IDString`,
  `SortNodes`, `DefaultMinReplicas`, `ValidateChange`);
- `worktree.TicketFromView`, `node.ShortID`.

The generator is port-notes/view-placement.md Appendix A, with the JSON changes listed at the end.

**Rust tests:** `tests/golden_tests/view.rs` (crate `dstore-view`).

```json
{
  "constants": { "slot_bits": 20, "slots": 1048576, "salt_domain": "amber-dstore/placement/1",
                 "voter_sync_done": 0, "voter_sync_pending": 1, "log2fix_zero_panic": "placement: Log2Fix(0)" },
  "fmix64":  [ { "in": "<u64>", "out": "<u64>" } ],
  "log2fix": [ { "in": "<u64>", "out": "<u64>" } ],
  "slot":    [ { "key": "<hex32>", "slot": 48276 } ],
  "salt":    [ { "id": "<hex32>", "salt": "<u64>" } ],
  "l":       [ { "slot": 12345, "salt": "<u64>", "l": "<u64>" } ],
  "sets": [ { "name": "golden5",
              "members": [ { "id": "<hex32>", "weight": 1000, "zone": "" } ],
              "replicas": [0, 1, 2, 3, 5, 7],
              "slots": [ { "slot": 0, "l": ["<u64>", null], "rank": [3, 4, 2, 1, 0],
                           "owners": [ { "r": 3, "owners": [3, 4, 2] } ] } ] } ],
  "views": [ { "name": "steady", "cbor": "<hex>",
               "keys": [ { "key": "<hex32>", "slot": 0,
                           "owners": ["<hex32>"], "pending_owners": ["<hex32>"] | null,
                           "write_set": ["<hex32>"], "read_order": ["<hex32>"],
                           "flags": [ { "id": "<hex32>", "is_owner": true, "is_pending_owner": false, "in_write_set": true } ] } ] } ],
  "view_cbor":   [ { "name": "zero", "cbor": "<hex>" } ],
  "view_decode": [ { "name": "replicas_256", "input": "<hex>", "ok": false, "reencode": "<hex>" | null, "error": "<text>" | null } ],
  "parse_node_id": [ { "input_hex": "<hex>", "ok": false, "id": "<hex32>" | null, "error": "<text>" | null } ],
  "short_id": [ { "bytes": "<hex>" | null, "node_short": "?", "view_short": "<8 hex>" | null, "id_string": "<64 hex>" | null } ],
  "ticket_from_view": [ { "name": "no_nodes", "view": "<hex>", "ticket": "dstore1…", "ticket_cbor": "<hex>" } ],
  "helpers": [ { "name": "full", "view": "<hex>", "all_members": ["<hex32>"], "voter_ids": ["<hex32>"], "quorum": 2,
                 "lookups": [ { "id": "<hex32>", "node_found": true, "node_weight": 4096, "node_addrs": ["ip:10.0.0.1:4433"] | null,
                                "is_member": true, "data_endpoint_owner": "<hex32>" | null, "is_former": false, "is_voter": true } ] } ],
  "node_helpers": [ { "id": "<hex>" | null, "zone": "rack-1", "nid": "<hex32>", "zone_or_id": "<hex>" } ],
  "id_lists": [ { "ids": ["<hex>" | null], "id": "<hex32>", "contains": false, "add_id": ["<hex>" | null] } ],
  "ids_of":   [ { "raw": ["<hex>"], "ids": ["<hex32>"] } ],
  "compare":  [ { "view_incarnation": "<u64>", "view_epoch": "<u64>", "incarnation": "<u64>", "epoch": "<u64>", "result": -1 } ],
  "default_min_replicas": [ { "r": 3, "min_replicas": 2 } ],
  "validate_change": [ { "name": "drop_three_r3", "cur": [member], "target": [member], "replicas": 3, "force": false, "error": "<text>" | null } ],
  "sort_nodes": { "input": ["<hex>" | null], "output": ["<hex>" | null] },
  "status_cluster_prefix": [ { "name": "indefinite_two_bytes", "view": "<hex>", "cluster_id": "<hex>" | null, "cap": 8,
                               "prefix": "01020000" | null, "panic": "<text>" | null } ]
}
```

**What each section holds, and what Rust asserts:**

- **`constants`.**
  - `placement::SLOT_BITS`/`SLOTS`.
  - `salt_domain` is the unexported Go `saltDomain`. Every `salt` row was checked against
    `BE64(BLAKE3-256(salt_domain ‖ id)[0..8])`.
  - `VOTER_SYNC_DONE`/`PENDING`.
  - `log2fix(0)` panics with `log2fix_zero_panic`.
- **`fmix64`:**
  - 0, 1, `0xdeadbeef` and 2^64−1;
  - the preimage of 2^64−1, which makes `L` = 0;
  - the preimage of `0x0123456789abcdef`;
  - 16 values of `u64s(42, 16)`.
- **`log2fix`:**
  - the fixed list of view-placement §5;
  - 2^i, 2^i+1 and 2^i−1 for i = 1..63;
  - the 1000 values `u64s(7, 1000)` (verification §5).
- **`slot`:**
  - the zero key and tails `ff…ff`, 2^44, 2^44−1 and 2^63;
  - a key whose bytes 0..23 are `ff` and whose tail is 0;
  - `data(1..3, 32)` and `data(600..615, 32)`.
- **`salt`:** `idFrom(00..05)`, `idFrom(ff)`, `data(11..13, 32)`, `data(200..211, 32)`, and the ed25519 ids
  `id(1..8)` of verification §5 (public keys of `NewKeyFromSeed(data(s, 32))`).
- **`l`:**
  - h = 0 gives `0x4000000000`; h = 2^64−1 gives 0; h = 2^64−2 gives 1;
  - `(12345, salt(idFrom(01)))` = `0x22356754f`;
  - 32 pairs with slot = `u64s(31, 32)[i] >> 44` and salt = `u64s(32, 32)[i]`.
- **`sets`.** `Set::new(members)` in the listed order. For each slot:
  - `l[i]` is `L(slot, salt(member i))`, or `null` for a weight-0 member (Go never computes it);
  - `rank` is `Set::rank(slot)`, and `owners[].owners` is `Set::owners(slot, r)`.

  The sets are:
  - `golden5`: `placement_test.go`;
  - `zones`: `TestZoneRule`;
  - `draining`: weight-0 members last, in id order;
  - `heavy`: upper words of the 128-bit products;
  - `tie`: weights equal to the members' own L at the first listed slot, so the smaller id wins;
  - `empty`;
  - `realistic12`;
  - `duplicates`: weighted ids listed twice. The stable sort keeps input order, and the second copy shares the
    first copy's zone key.

  No set has two weight-0 members with the same id: Go sorts those with the unstable `sort.Slice`.
- **`views`.** `View::decode(cbor)`, then `Placement::new`. For every key, Rust checks `owners`,
  `pending_owners`, `write_set` and `read_order`:
  - `pending_owners` is `null` without a pending transition (`None`);
  - `[]` means a pending transition with no owners (`Some([])`).

  `flags` covers every `all_members()` id, then the non-member `data(999, 32)`, plus the former node `data(306, 32)`
  in `drained_committed`. Rust asserts `is_owner`, `is_pending_owner` and `in_write_set`.

  The views are those of the view-placement §5 table: `steady`, `join_pending`, `drain_pending`,
  `remove_pending`, `replicas_pending`, `zone_pending`, `drained_committed`, `unsorted_nodes`,
  `zones_committed`, `short_node_id`, `duplicate_node`, `no_nodes` and `pending_no_nodes`. The keys are:
  - the zero key and tails `ff…ff` and 2^44;
  - `data(500..523, 32)`.

  Some views use only the first 3 or 6 keys.
- **`view_cbor`.** Views built field by field in Go. Rust asserts `View::decode(cbor)?.encode() == cbor`; Go
  checked the same round trip. The cases are:
  - the §3.1 encodings, `full` and `long_addrs`;
  - `acl_nil_and_empty_elements` and `pending_lists`, which pin nil and empty elements inside omitempty
    `[][]byte` lists.
- **`view_decode`.** When `ok`, `View::decode(input)?.encode() == reencode`. Otherwise `ViewError` Display equals
  `error` exactly (PORTING C7). The cases are every row of view-placement §3.2, plus:
  - nesting depths 31, 32 and 33;
  - 131073-element counts;
  - bignum tags, indefinite text and a wrong chunk type;
  - int boundaries of `voter_sync`;
  - null into bool, and invalid UTF-8 in a nested text;
  - null or empty list elements, and wrong element types.
- **`parse_node_id`.** `view::parse_node_id(&input_hex bytes)`. When `ok`, it returns `id`; otherwise
  Display == `error`. The inputs are those of §3.5, plus a `0X` prefix, a trailing `é`, a Cyrillic `а`
  and a leading NBSP.
- **`short_id`.**
  - `view::node_short_id(bytes)` == `node_short` (`?` unless 32 bytes).
  - For 32-byte inputs, `short_id` == `view_short` and `id_string` == `id_string`.
  - `bytes: null` is Go nil.
- **`ticket_from_view`.** `ticket_from_view(&View::decode(view)?)`:
  - `.encode()` == `ticket`;
  - the ticket's CBOR == `ticket_cbor`.

  The `no_nodes` and `empty_non_nil_nodes` cases give members `f6`.
- **`helpers`.** Decode `view`, then check `all_members()`, `voter_ids()` and `quorum()`. Per lookup:
  - `node(id)`: found, weight and addrs (`null` when not found or without addresses);
  - `is_member`, `data_endpoint_owner`, `is_former` and `is_voter`.

  The helpers are:
  - `full`: nodes entries win over pending entries, and data endpoints are members;
  - `short_node_id`: a 31-byte node entry is not found under its zero-padded id;
  - `no_nodes`.
- **`node_helpers`.** A `view::Node` with `id` (`null` = `None`) and `zone`. Rust asserts:
  - `nid()` == `nid` (zero-padded or truncated);
  - `zone_or_id()` == `zone_or_id` (the zone bytes, else the raw id of any length).
- **`id_lists`.** `view::contains(&ids, &id)` == `contains`, and `view::add_id(&mut ids, &id)` leaves `add_id`.
  `null` elements are `None`.
- **`ids_of`.** `view::ids_of(&raw)` == `ids`.
- **`compare`.** `View { incarnation: view_incarnation, epoch: view_epoch, .. }.compare(incarnation, epoch)`
  maps −1/0/1 to `Less`/`Equal`/`Greater`.
- **`default_min_replicas`.** All `r` from 0 to 255.
- **`validate_change`.** `member` = `{ "id": "<hex>" | null, "weight": n, "zone": "" }` as a `view::Node`.
  `validate_change(&cur, &target, replicas, force)` returns `Ok` when `error` is `null`, else an error whose
  Display equals `error`.
- **`sort_nodes`.** Nodes with the listed ids; `sort_nodes` gives `output`. The input holds one nil id and
  no empty id: nil and empty compare equal, and Go's sort is unstable.
- **`status_cluster_prefix`.** `cluster status` prints `%x` of `v.ClusterID[:4]` (`cmd/dstore/client.go:136`)
  for the view that `view.Decode` returned (view-placement Addenda 3.2, PORTING DD-7). Go slices up to the
  **capacity**, not the length:
  - fxamacker copies a definite-length byte string into a slice with `cap == len`, so a nil or shorter than
    4 bytes cluster id panics;
  - an indefinite-length byte string keeps its `append`-grown buffer (Go size classes: 1..8 bytes give cap 8,
    no bytes give cap 0), so a 1..3-byte indefinite cluster id does not panic and prints its bytes followed
    by zero bytes (`01020000`).

  Rust asserts, after `View::decode(view)`: when `panic` is set, `cluster status` calls
  `go_panic_exit(panic)`. A real Go process was checked to write `panic: <panic>` as its first stderr line and
  exit 2. Otherwise the first status line starts with `cluster <prefix> `. `cap` is Go's `cap(v.ClusterID)`,
  and `cluster_id` holds the decoded bytes (`null` = nil).

**Changes from the Appendix A JSON:**
- u64 values are decimal strings, not `0x%016x`.
- Maps became ordered arrays (`owners`, `default_min_replicas`).
- `l` is `null` for weight-0 members instead of `""`.
- `reencode`/`error`/`id` are `null`, not omitted.
- `parse_node_id` carries `input_hex`.
- `helpers` is an array.
- `sort_nodes` also lists its input.
- New sections: `constants`, `node_helpers`, `id_lists`, `ids_of`, `status_cluster_prefix`.
- New cases, as listed above.

---

## `placement/all_slots.json`

**Go:** dstore v0.1.10 `placement.NewSet`, `Set.Rank` and `Set.Owners` over all 2^20 slots (verification.md §4.3
item 9). **Rust tests:** `tests/golden_tests/view.rs`.

```json
{ "slots": 1048576,
  "hash": "BLAKE3-256 over slots 0..2^20-1 in order, each contributing u8 len(rank) ‖ rank indexes as u8 ‖ u8 len(owners) ‖ owners as u8, where rank = Set.Rank(slot) and owners = Set.Owners(slot, r)",
  "sets": [ { "name": "golden5_r3",
              "members": [ { "id": "<hex32>", "weight": 1000, "zone": "" } ],
              "r": 3,
              "spots": [ { "slot": 0, "rank": [3, 4, 2, 1, 0], "owners": [3, 4, 2] } ],
              "all_slots_blake3": "<64 hex>" } ] }
```

**Hash.** For each set, feed one BLAKE3-256 hasher, for `slot` = 0 … 1048575 in order:
1. `len(rank)` as one byte, then each rank index as one byte;
2. `len(owners)` as one byte, then each owner index as one byte.

Here `rank = Set::rank(slot)` and `owners = Set::owners(slot, r)`. Every set has at most 255 members.

**Spots.** The slots 0, 1, 12345, 0xfffff, 524288 and 777777, as a debugging aid when a hash differs.

**Sets:**
- `golden5_r3`, `golden5_r5`, `golden5_r0`;
- `zones_r3`, `draining_r4`, `heavy_r2`, `tie_r1`, `realistic12_r3`;
- `splitmix50_r3`:
  - ids `data(400..449, 32)`;
  - weights `u64s(4242, 50)[i] % 65536`, with weight 0 when `i % 10 == 7`;
  - zones `rack<i%6>` when `i % 4 != 0`, else `""`;
- `duplicates_r2`, `empty_r3`.

The member lists match the sets of `view_placement.json`, except `splitmix50_r3`, which only this file has. A Rust run is about 2^20 × (r + 1) ranks per set;
run it with optimisation or expect a slow debug test.

---

## `transport/addrs.json`

**Go:** go-iroh v0.2.0 `netaddr.ParseTransportAddr`, dstore v0.1.10 `transport.ParseAddrs`, and go1.26.5
`net/netip.ParseAddrPort` (port-notes/transport.md §5.1, §5.4). **Rust tests:** `tests/golden_tests/transport.rs`
(`dstore_transport::addr`).

```json
{ "parse_transport_addr": [ { "input": "ip:[fe80::1%en0]:9", "ok": true, "addr": Addr | null, "error": "<text>" | null,
                              "parse_addrs": [Addr] } ],
  "parse_addrs": [ { "name": "order_kept_skips_unparseable_no_dedup", "input": ["…"], "output": [Addr] } ],
  "parse_addr_port": [ { "input": "[fe80::1%en0]:9", "ok": true, "ip": "fe80::1" | null, "zone": "en0" | null,
                         "port": 9 | null, "string": "[fe80::1%en0]:9" | null, "error": "<text>" | null } ] }
```

`Addr` = `{ "kind": "relay" | "ip" | "custom", "string": "<Go String()>", "relay_url": "<text>" | null, "ip": "<text>" | null, "zone": "<text>" | null, "port": n | null, "custom_id": "<u64>" | null, "custom_data": "<hex>" | null }`.

- **`parse_transport_addr`.** `addr::parse_transport_addr(input)` returns either `Ok` or an `Err` text equal to
  `error`. On `Ok`:
  - the variant follows `kind`;
  - `GoTransportAddr` Display == `string`: `relay:<url>`, `ip:<addrport>`, or `<id hex>_<data hex>` without
    `custom:`;
  - `Relay(GoRelayUrl(relay_url))`;
  - `Ip { ip, zone, port }`: `ip` is the address without its zone, and IPv4-mapped IPv6 stays IPv6
    (`::ffff:1.2.3.4`);
  - `Custom { id: custom_id, data: custom_data }`.

  `parse_addrs` is `ParseAddrs([input])`: the bare `ip:port` fallback applies, and unparseable input is
  skipped silently. The inputs are:
  - transport §5.1;
  - the netip edges: zones, `%` with an empty zone, leading zeros in the port, unbracketed IPv6, signs, `0x`,
    brackets around IPv4, embedded IPv4 and long IPv6 forms;
  - the ParseCustomAddr edges: empty data, odd hex, uppercase, prefixes, overflow and a doubled `custom:`;
  - the relay edges.
- **`parse_addrs`.** `addr::parse_addrs(&input)` == `output`. Order is kept, nothing is deduplicated, and a Go nil
  list is shown as `[]`.
- **`parse_addr_port`.** `addr::parse_addr_port(input)` gives `Ok((ip, zone, port))`, or an `Err` text equal to
  `error`. `string` is Go `AddrPort.String()`.

---

## `transport/relay_urls.json`

**Go:** go-iroh v0.2.0 `netaddr.ParseRelayURL` and `relay.DefaultMap`, `relay.ModeCustomURLs` (transport §5.2, §5.3).
**Rust tests:** `tests/golden_tests/transport.rs`.

```json
{ "parse_relay_url": [ { "input": "https://Example.COM", "ok": true, "string": "https://example.com/" | null, "error": "<text>" | null } ],
  "default_quic_port": 7842,
  "default_map": [ { "url": "https://aps1-1.relay.n0.iroh-canary.iroh.link./", "quic_port": 7842 | null } ],
  "default_relay_addrs": ["relay:https://aps1-1.relay.n0.iroh-canary.iroh.link./"],
  "custom_url_map": [ { "url": "http://relay.example:8080/", "quic_port": 7842 | null } ] }
```

- **`parse_relay_url`.** `addr::parse_relay_url(input)` gives `Ok(GoRelayUrl(string))`, or an `Err` text equal to
  `error` (`failed to parse relay URL: parse "…": …`). The inputs are:
  - transport §5.2;
  - empty and relative URLs;
  - special and non-special schemes, userinfo, IPv6 hosts with zones, spaces in paths, fragments, `?` and empty
    ports;
  - bad escapes, non-ASCII hosts, an unclosed `[`, and the control character U+007F.
- **`default_map`.** `relay.DefaultMap().Configs()` in URL-sorted order: the `GO_DEFAULT_RELAYS` hosts,
  normalised with a trailing `/`.
- **`default_relay_addrs`.** The Go address strings of those URLs.
- **`custom_url_map`.** `relay.ModeCustomURLs("https://relay.example./", "http://Relay.Example:8080").Map().Configs()`:
  a `--relay` URL gets QUIC port 7842.

---

## `transport/ids.json`

**Go:** go-iroh v0.2.0 `key.NewEndpointID` (filippo.io/edwards25519 `Point.SetBytes`), `key.NewSecretKey`,
`EndpointID.String`, `Short` and `Z32`, and the mDNS label of `iroh/mdns` (transport §5.5, §5.6). **Rust tests:**
`tests/golden_tests/transport_iroh.rs`.

```json
{ "endpoint_ids": [ { "name": "first_byte_2", "bytes": "<hex32>", "valid": false, "error": "data is not a valid public key" | null } ],
  "identities": [ { "name": "seed_01_to_20", "seed": "<hex32>", "id": "<hex32>", "string": "<64 hex>", "short": "79b5562e8f",
                    "z32": "<52 chars>", "mdns_label": "<52 chars>",
                    "no_candidates_error": "transport: no candidate addresses for 79b5562e8f" } ] }
```

- **`endpoint_ids`.** `valid` is Go's curve-point acceptance. The Rust check (`iroh_base::PublicKey::from_bytes`,
  `ticket::is_valid_public_key`) must agree for every row; the error text is `TransportError::InvalidKey`. The rows
  are:
  - first byte 0..9 with the rest zero (2, 7 and 8 are invalid);
  - all `ff`, which is valid;
  - the eight small-order encodings;
  - non-canonical encodings (y = p, y = p+1, y = 1 with the sign bit set, y = p−1 with the sign bit set);
  - the base point, with and without the sign bit;
  - four ed25519 keys, with and without the sign bit flipped;
  - 48 splitmix arrays `data(7000..7047, 32)`.
- **`identities`.** The ids of `NewSecretKey(seed)`: seed `01 02 … 20` and `data(1..3, 32)`.
  - `short` is 5 bytes (go-iroh `Short`), and `z32` is z-base-32.
  - `mdns_label` is go-iroh `endpointLabel`: RFC 4648 base32, lowercase, no padding.
  - `no_candidates_error` applies the `raceConnect` format string (`transport/iroh.go`) to `short`. It is derived:
    Go's `Dial` never calls `raceConnect` without candidates.

---

## `transport/mdns.json`

**Go:** go-iroh v0.2.0 `iroh/mdns`. The packet code is unexported (`dnsmsg.go`: `buildQuery`, `buildAnnouncement`,
`parseAnnouncement`, `parseQuestions`; `mdns.go`: `(*Discovery).announcementInfo`, `endpointLabel`,
`serviceName`, `instanceName`, `hostName`). The generator reaches these through `go:linkname`:
- it declares mirror structs for `announcementData` and `dnsQuestion`;
- it refuses to run unless the build info shows go-iroh v0.2.0 and dstore v0.1.10;
- it parses every announcement back as a layout check.

Hand-made packets (the swarm-discovery layout, compression, malformed packets) come from the generator's own DNS
writer. Transport §5.8. **Rust tests:** `tests/golden_tests/transport_mdns.rs` (`dstore_transport_iroh::mdns`).

```json
{ "service_name": "irohv1",
  "names":   [ { "id": "<hex32>", "label": "pg2vml…", "service": "_irohv1._udp.local",
                 "instance": "<label>._irohv1._udp.local", "host": "<label>.local" } ],
  "queries": [ { "name": "query_79b5562e8f", "id": "<hex32>", "packet": "<hex>" } ],
  "announcements": [ { "name": "go_example", "id": "<hex32>", "service": "irohv1",
                       "addrs": ["ip:192.168.1.2:4242", "relay:https://…/"], "user_data": "<text>" | null,
                       "info": { "port": 4242, "ips": ["192.168.1.2:4242"], "relay": "", "user_data": "" } | null,
                       "error": "<text>" | null, "packet": "<hex>" | null } ],
  "parse": [ { "name": "swarm_discovery_layout", "service": "irohv1", "packet": "<hex>", "ok": true,
               "id": "<hex32>" | null, "ips": ["192.168.1.2:4242", "[2001:db8::1]:4242"] | null,
               "relay": "<text>" | null, "user_data": "<text>" | null, "addrs": ["ip:…", "relay:…"] | null } ],
  "questions": [ { "name": "typed_any_instance", "packet": "<hex>", "ok": true, "questions": [ { "name": "…", "type": 255 } ] | null } ] }
```

- **`names`.** `mdns::endpoint_label(id)` == `label`. The service, instance and host names follow from it.
- **`queries`.** `mdns::build_query(id)` == `packet`: two PTR questions (service, instance), no compression.
- **`announcements`.** `dns.NewEndpointData(addrs…)`, with the user data set, is passed to `announcementInfo`:
  - it picks the port shared by most addresses, the lowest on a tie, and drops the others;
  - it unmaps IPv4-mapped addresses and keeps zones in `info`;
  - it keeps the relay only up to 249 bytes;
  - with no IP address it fails with `mdns: endpoint data has no IP addresses`.

  `buildAnnouncement` then gives `packet`: every record in the answer section, with TTL 120 and no compression.
  Announcing is node-side in v1, so the Rust tests use these packets as parser input. A publisher, if ever ported,
  must reproduce them. The cases are:
  - the go_example of §5.8, with and without user data;
  - IPv4-mapped duplicates, a zoned IPv6 address, tie and majority ports;
  - relays of 249 and 250 bytes, 245-byte user data, a relay before the IP, a custom address;
  - many IPs, port 0, relay only, no addresses, and service `other`.
- **`parse`.** `parseAnnouncement(packet, "irohv1")`. Rust `mdns::parse_announcement(packet)` returns `None` when
  `ok` is false. Otherwise it returns `Announcement { id, addrs: ips as SocketAddr, relay, user_data }`:
  - `ips` are in record order, deduplicated;
  - `relay` is the first relay URL, normalised by `ParseRelayURL`; an unparseable one is dropped;
  - `addrs` are go-iroh's `EndpointData` strings, for information.

  The cases are:
  - every announcement above;
  - truncations and trailing garbage;
  - the swarm-discovery layout (A/AAAA in additionals, target `<label>-<port>.local`), plain and compressed;
  - go-iroh's layout compressed;
  - pointer chains of 28 (accepted), 29 and 30 hops (refused: `readName` takes at most 32 steps), and a
    compression loop;
  - SRV port 0, no address records, PTR only, and a target case mismatch (host names match case-sensitively);
  - an uppercase service suffix (refused: `TrimSuffix` is case-sensitive);
  - an uppercase label (accepted);
  - a 64-byte label (refused: label type bits `01`), a z-base-32 label and an invalid curve point;
  - two instances of which one is valid;
  - records in the authority section, questions before answers, and a query flag with answers;
  - TXT details: the last `relay=` wins, an unparseable relay, entries without `=`, overruns, empty values and
    extra `=`;
  - duplicate A records, AAAA of an IPv4-mapped address;
  - an ANCOUNT too large, an rdata overrun, an unsupported label type, a short SRV rdata, and an empty packet.
- **`questions`.** `parseQuestions` (responder side, node-side; not needed by v1): query packets, typed questions,
  compressed names, a response flag, no questions, truncation, and a short header.

---

## `transport/pool_scripts.json`

**Go:** dstore v0.1.10 `transport.Pool` over `transport.NewNetwork()` (the in-memory transport), `wire.ReadMsg`,
`WriteMsg`, `ErrMsg` and `CloseStream`, and `time.Duration` (transport §5.7, §5.9). **Rust tests:**
`tests/golden_tests/transport.rs` (`Pool`, `mem`).

```json
{ "scripts": [ { "name": "failed_dial_window", "description": "…", "per_peer": 2,
                 "nodes": [ { "name": "client", "id": "<hex32>", "bound": true, "alpns": [], "server": null },
                            { "name": "a", "id": "<hex32>", "bound": true, "alpns": ["amber-dstore/1"],
                              "server": { "mode": "err", "code": "busy", "text": "slow down", "retry_after_ms": "1500" } } ],
                 "steps": [ Step ] } ],
  "rtt_round":   [ { "ns": "<i64>", "rounded_ns": "<i64>", "string": "1.235s" } ],
  "error_texts": [ { "name": "joined_dial_discovery", "text": "dial ip:127.0.0.1:9: x\ndiscovery: y" } ] }
```

**Harness** (mirror it in Rust with `mem::Network`):
- Every node with `bound` is bound with its `alpns`. Node `client` is always bound, with no ALPNs.
- A `server` runs on the node:
  - `pong`, `err` and `eof` run an accept loop. For every stream it reads one frame, then:
    - `pong` writes `{0: 106, 3: epoch+1}`;
    - `err` writes `ErrMsg(code, text)` with `RetryAfter = retry_after_ms` (absent = 0);
    - `eof` writes nothing.

    Then it calls `CloseStream`.
  - `none` and `manual` run nothing; `manual` nodes are driven by the `server_*` steps.
- The pool is `NewPool(scripted endpoint, addrs(id) = no addresses, per_peer)`. The scripted endpoint wraps the
  client's `MemEndpoint`:
  - `dial_attempts` counts `Dial` calls;
  - successful dials are numbered 0, 1, 2… (`conn`);
  - `fail_next_dial` makes the next `Dial` fail with `text` before it reaches the network.

**Steps.** A step lists its op, its inputs, then its outcome. Fields that do not apply to an op are absent.
- `ok` is always present. `error` is the Go error text, absent on success. The one exception is the harness
  text `not closed`, given when `server_wait_closed` times out.
- `request` and `reply` are `{type, epoch, code, text, retry_after_ms}`. A request sets only `type` and
  `epoch`; its other fields are zero. `data` and `read` are absent when they hold no bytes, as after a read
  of 0 bytes.
- `timeout_ms`, when present, bounds the ctx. Without it the ctx is `Background` (no deadline).
- `repeat` n runs the step n times. Every run had the same `ok`/`error`, and the outcome fields are those of the
  last run.

| op | inputs | Go call | outcome |
|---|---|---|---|
| `get` | `peer`, `alpn`, `timeout_ms`? | `Pool.Get` | `ok`/`error`, `conn`, `dial_attempts`, `dials` |
| `open` | `peer`, `alpn`, `timeout_ms`?, `repeat`? | `Pool.Open`; the stream gets the next client stream index | `ok`/`error`, `stream`, counts |
| `call` | `peer`, `alpn`, `timeout_ms`?, `request` `{type, epoch}` | `Pool.Call(Msg{Type, Epoch})` | `ok`/`error`, `reply` when Go returned one, `remote` `{code, text, retry_after_ns}` when the error is a `*wire.Error`, counts |
| `drop` | `peer`, `alpn` | `Pool.Drop` | `ok` |
| `close` | | `Pool.Close` | `ok` |
| `path` | `peer`, `alpn` | `Pool.Path` | `ok` = a live conn exists, `path` `{direct, rtt_ns}` |
| `set_down` | `peer`, `down` | `Network.SetDown` | `ok` |
| `partition` | `a`, `b`, `cut` | `Network.Partition` | `ok` |
| `fail_next_dial` | `text` | scripted endpoint | `ok` |
| `sleep` | `ms` | real sleep | `ok` |
| `close_endpoint` | `peer` | `MemEndpoint.Close` | `ok` |
| `stream_write`, `server_write` | `stream` / `server_stream`, `data` | `Write` | `ok`/`error`, `n` |
| `stream_read`, `server_read` | `stream` / `server_stream`, `buf_len` | one `Read` into a `buf_len` buffer | `ok`/`error` (`EOF`), `n`, `read` |
| `stream_close_write`, `server_close_write` | `stream` / `server_stream` | `CloseWrite` | `ok` |
| `stream_cancel_read`, `server_cancel_read` | `stream` / `server_stream` | `CancelRead(0)` | `ok` |
| `server_accept` | `peer` | `MemEndpoint.Accept` (2 s bound) | `ok`/`error`, `server_conn` |
| `server_accept_stream`, `server_open_stream` | `server_conn` | `AcceptStream` / `OpenStream` (2 s bound) | `ok`/`error`, `server_stream` |
| `server_wait_closed` | `server_conn`, `ms` | wait for `Done()` up to `ms` | `ok` |

In Rust, `Pool::call` returns `CallError::Remote` instead of Go's (reply, error) pair, so `reply` of a TErr is for
information only. Short ids in `mem: <8 hex> …` texts are `view::short_id`.

**Scripts:**
- **`grow_to_per_peer_then_round_robin`.** Dial up to `per_peer`, then rotate with a counter that is never reset.
- **`per_peer_zero_means_one`.**
- **`failed_dial_window`.** An unreachable peer; then `transport: peer recently unreachable` without dialing,
  even after the peer is back. After 2.1 s the pool dials again.
- **`failed_dial_ignored_with_live_conn`.** The 2 s window applies only with no live connection.
- **`drop_close_and_path`.** `Drop` and `Close` forget connections, and a later `Get` dials again. `Path` is
  `{direct: true, rtt: 1 ms}`.
- **`closed_conns_are_filtered`.** A peer going down closes the conns; they are filtered, with no failure window.
- **`mem_dial_errors`.** `does not speak`, `not bound`, `unreachable` (partition), `local endpoint is down` and
  `transport: closed`. Each uses its own pool key.
- **`call_reply`.** One frame each way; the connection stays pooled.
- **`call_remote_error`.** A TErr with and without text and retry-after.
- **`call_ctx_deadline`.** `context deadline exceeded`; the connection is not dropped. The peer then reads the
  request frame and EOF, and its write fails with `mem: stream reset by peer`.
- **`call_eof_does_not_drop`.** A read error (`EOF`) does not drop the connection.
- **`open_stream_failure_drops`.** The peer buffers 256 unaccepted streams, so the 257th `OpenStream` blocks until
  the ctx ends. That failure drops the pool's connections.
- **`mem_stream_pipes`.**
  - FIN reads as `EOF`, and a write after `CloseWrite` fails with `io: read/write on closed pipe`.
  - A cancelled read gives `mem: read canceled`, and the peer's write gives `mem: stream reset by peer`.
  - After `Drop`, the server conn closes: `AcceptStream` and `OpenStream` return `transport: closed`.

**`rtt_round`.** `Duration.Round(time.Millisecond)` and its `String()`, as used by the client's `pathAttrs` `rtt`
and the TUI (`gocompat::time::duration_round`, `duration_string`).

**`error_texts`:**
- real Go values: `transport.ErrClosed`, `key.ErrInvalidKeyData`, `iroh.ErrNoAddress`;
- derived: the `dial %s: %w`, `discovery: %w` and `transport: no candidate addresses for %s` format strings of
  `transport/iroh.go`, and `client: no bootstrap node answered: %w`, applied through Go `fmt` and `errors.Join` to
  real go-iroh address strings. The Rust `TransportError` Display must produce the same texts.
