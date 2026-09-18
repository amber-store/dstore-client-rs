# impl-vectorgen-view

Label vectorgen-view (layer L1). Files: `tools/vectorgen/family_view.go`, `tools/vectorgen/family_transport.go`,
`tools/vectorgen/docs/vectorgen-view.md`, and the vectors under `tests/golden/{view,placement,transport}/`.
Families registered: `view`, `placement`, `transport`. The schemas are in the docs file.

## Decisions and deviations

1. **Appendix A JSON shape** (view-placement.md) changed to follow the PORTING §7 conventions:
   - u64 values are decimal strings, not `0x%016x`;
   - Go maps became ordered arrays;
   - error and optional values are `null` instead of being omitted;
   - `helpers` is an array.

   The values are unchanged. New cases and sections are listed in the docs file.
2. **Placement ids** follow view-placement §5: splitmix `data(seed, 32)` and `idFrom(b)`, not the ed25519 ids
   of the VECTORS.md convention. The spec's tables (`142e00…`, `9203eb…`) are these ids.
3. **mDNS packet code is unexported in go-iroh v0.2.0.** The generator reaches it through `go:linkname`, with
   mirror structs of `announcementData` and `dnsQuestion`. The spec suggested a temporary copy of the package
   instead. Three safeguards:
   - `tspCheckVersions` refuses any go-iroh other than v0.2.0 and any dstore other than v0.1.9;
   - every generated announcement is parsed back;
   - Go 1.23+ blocks pull-style linknames only into the standard library, so these references build.
4. **Derived texts.** A few texts have no reachable Go code path without the network:
   - `transport: no candidate addresses for %s`;
   - the `dial %s: %w` / `discovery: %w` joins;
   - `client: no bootstrap node answered: %w`.

   The generator applies the Go format strings to real go-iroh values; the docs label these texts as derived.
5. **`mdns.json` parse cases all use service `irohv1`**, because `dstore_transport_iroh::mdns::parse_announcement`
   has no service parameter. Service isolation is covered by the `other_service` announcement parsed as `irohv1`.
6. **`placement/all_slots.json`** holds one entry per (set, r), with debugging spots. Every set avoids duplicate
   weight-0 ids, because Go orders those with the unstable `sort.Slice` and the u8 index hash would depend on it.

## No manifest changes needed

Every package used is already required by `tools/vectorgen/go.mod`, and `go.sum` covers it. The build runs offline
with `GOPROXY=off`.

## Review

Reviewer: review-vectorgen-view (adversarial review of vectorgen-view).

### What was checked

- **Sources read.** PORTING §0-3, §4.5-§4.7 and §5-7; view-placement.md in full, with its Addenda; transport.md §2, §3.3,
  §5 and Addenda; verification.md §4.2-§4.3, §5 and Addenda. On the Go side: dstore `placement`, `view`,
  `transport.go`, `mem.go` and `wire` errors; go-iroh `iroh/mdns` (`dnsmsg.go`, `mdns.go`), `dns.EndpointData`,
  `netaddr`, `relay` and `key`; fxamacker `fillByteString`.
- **Determinism.** `view placement transport` were regenerated twice on an isolated copy of the module: `main.go`,
  `util.go`, `deps.go`, `main_test.go`, `go.mod`, `go.sum` and the two families. Both runs were identical, and equal
  to the committed files.
  - The real module did not build during the review, because of siblings mid-edit:
    `family_client.go:1321 undefined: sync`, later `family_wire.go:1085 undefined: admRequest`.
  - On the copy, `gofmt -l` printed nothing, and `go vet .` and `go test .` passed, both before and after the fixes
    below.
- **Values.** Checked against the spec tables; every row matches.
  - view-placement:
    - the fmix64, log2fix, slot, salt and L tables;
    - the golden5, zones, draining, heavy and tie ranks;
    - every row of the views table in §5;
    - every decode text in §3.2 and every ticket in §3.3;
    - the `full` helpers, compare, default_min_replicas, validate_change and sort_nodes.
  - transport:
    - §5.1, every row;
    - §5.2, §5.3, §5.5, §5.6 and §5.7;
    - §5.8: go_example has port 4242, ANCOUNT 5, and the 5555 address is dropped;
    - §5.9 (1)-(7).
- **all_slots layout.** A separate program built the whole byte stream and hashed it once with `blake3.Sum256`. It
  reproduced `golden5_r3`, `golden5_r0`, `duplicates_r2` and `empty_r3`; `empty_r3` is BLAKE3 of 2^21 zero bytes.
- **Determinism hazards.**
  - `parseAnnouncement` loops over a Go map of instances, but no parse case has two valid instances.
  - `Network.SetDown` closes the client halves synchronously through `dropPeer`, so `closed_conns_are_filtered` is not
    racy.
  - `relay.Map.Configs()` and `URLs()` sort, and `EndpointData.AddAddrs` deduplicates, as the docs say.
- **Linkname mirrors.** They match go-iroh v0.2.0 `announcementData` and `dnsQuestion` field for field.

### Found and fixed

1. **No vector for the short-cluster-id panic** (view-placement Addenda 3.2 asks for "a vector with the Go first
   line"). Added `status_cluster_prefix` to `view/view_placement.json`: 13 views run through `view.Decode`. Each case
   records `cap(v.ClusterID)`, and either the `%x` of `v.ClusterID[:4]` or the runtime panic text.
   - **Finding for the view and CLI owners.** Go slices up to the capacity, not the length.
     - fxamacker copies a definite-length byte string with `cap == len`. So a nil cluster id, or one shorter than 4
       bytes, panics with `runtime error: slice bounds out of range [:4] with capacity N`.
     - fxamacker keeps the `append`-grown buffer of an indefinite-length byte string: 1..8 bytes give cap 8. So a
       1..3-byte indefinite cluster id does not panic, and prints zero-padded prefixes such as `01020000`.
   - A scratch Go process (deleted afterwards) confirmed:
     - for 2 definite bytes: stderr `panic: runtime error: slice bounds out of range [:4] with capacity 2`, exit 2;
     - for 2 indefinite bytes: `cluster 01020000 incarnation 0 …`, exit 0.
   - `View.cluster_id: Option<Vec<u8>>` does not carry Go's capacity. The Rust decoder or `print_status` has to know
     whether the byte string was indefinite and apply Go's size classes to be exact, or a DD has to record the
     difference. Real nodes never send indefinite lengths.
2. **Missing verification §5 cases.** `log2fix` asked for 1000 splitmix values and had 64. `salts` asked for the
   ed25519 ids `id(1..8)` and had none. Added `u64s(7, 1000)` and `id(1..8)`, appended after the existing rows, so the
   earlier rows are unchanged.
3. **Docs gaps.** `docs/vectorgen-view.md` now also documents:
   - `data` and `read` being absent when they hold no bytes;
   - the field sets of `request` and `reply`;
   - the harness-only `not closed` text of `server_wait_closed`;
   - the y = p−1 sign-bit row in `ids.json`;
   - `splitmix50_r3` existing only in `all_slots.json`;
   - the ed25519 salt ids;
   - the new section.

### Noted, not changed

- **U+200B in view-placement §3.5.** The table shows the character quoted literally. Go `%q` gives `"​"`
  (`IsPrint` is false for it), and the vector holds Go's text, so the spec table is wrong.
- **Pool second dial check.** `Get` has a second check against the unfiltered connection list, reached only by
  concurrent `Get`s. Transport §5.9 lists no such script. It belongs in a Rust unit test with a gated endpoint.
- **Deviations accepted.** The implementer's deviations stand: the Appendix A JSON shape, placement ids from splitmix,
  linkname instead of a package copy, derived error texts, service `irohv1` for every parse case, and the
  `pool_scripts` DSL. Each is documented and consistent with PORTING §7.
- **Still to run.** `go vet .`, `go test .` and generation on the real module, once the siblings compile.
