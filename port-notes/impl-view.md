# impl-view: layer L2 notes

Owner: view. Files:
- `crates/view/Cargo.toml`, `src/lib.rs`, `src/placement.rs`, `src/view.rs`, `tests/all_slots.rs`;
- `tests/golden_tests/view.rs`.

## What is implemented

PORTING.md §4.5 in full, with no `todo!()` left and no `unwrap`/`expect` on data paths.

- **`placement`.**
  - `slot`, `fmix64`, `l`.
  - `salt`: `blake3::Hasher` over `SALT_DOMAIN ‖ id`, first 8 bytes big-endian.
  - `log2fix`: `u128` squaring; panics `placement: Log2Fix(0)` as Go does.
  - `Set::rank`: stable `sort_by` over the 128-bit products `a.weight·lb` against `b.weight·la`, then the id;
    weight-0 members after them, by id.
  - `Set::owners`: byte zone keys; an empty zone uses the 32 id bytes.
  - `Table`: lazy `RwLock<HashMap<u32, Arc<[u32]>>>` caches. Values are computed outside the lock, and a
    poisoned lock still serves its map.
- **`view`.**
  - The CBOR structs are unchanged from L0; `encode`, `decode` (`view: decode: …`) and `compare`.
  - Lookups and helpers: `nodes`, `node`, `is_member`, `data_endpoint_owner`, `is_former`, `is_voter`,
    `voter_ids`, `quorum`, `all_members`, `Node::nid`, `Node::zone_or_id`.
  - Id helpers: `parse_node_id`, `id_string`, `short_id`, `node_short_id`, `contains`, `add_id`, `ids_of`,
    `sort_nodes`.
  - `default_min_replicas`, `validate_change`, and `Placement`.
- **`ticket_from_view`.**

## Additions beyond PORTING.md §4.5 (nothing in §4 was changed)

1. `placement::SALT_DOMAIN: &str`: Go's unexported `saltDomain`. The golden `constants` section asserts it.
2. `view::cluster_id_cap(b: &[u8]) -> Result<usize, ViewError>`: Go's `cap(v.ClusterID)` for the view that
   `view.Decode(b)` returns, or the decode error. See the next section.
3. `view::status_cluster_prefix(cluster_id: Option<&[u8]>, cap: usize) -> Result<String, String>`: printStatus
   `%x` of `v.ClusterID[:4]`. It returns 8 hex characters, with bytes past the length read as zero. With
   `cap < 4` it returns the Go runtime panic text for `go_panic_exit`:
   `runtime error: slice bounds out of range [:4] with capacity N`.
4. `[dev-dependencies]` `dstore-testkit` and `serde` in `crates/view/Cargo.toml`, for `tests/all_slots.rs`. This
   is a dev-dependency cycle (view → testkit → view), which Cargo allows. The review added `dstore-udiff`, for the
   `gosort` differential test.
5. The private module `src/gosort.rs` (review): Go's `sort.Slice`, see "Review".

## `cluster status` prefix: what cli-admin needs, and a DD proposal

### What Go does

Go slices `v.ClusterID[:4]` up to the capacity, not the length. What fxamacker v2.9.3 gives for key 0:

| Encoding of key 0 | `cap` |
|---|---|
| absent, `null`, `undefined` (nil) | 0 |
| definite-length byte string, also inside tags 2/3 (`make` + `copy`) | the length |
| array of integers (`MakeSlice(count, count)`) | the element count |
| indefinite-length byte string | what `append`-ing its chunks to `[]byte{}` gives, through go1.26.5 `growslice` (`nextslicecap`, then `roundupsize` with the `internal/runtime/gc` size classes) |

Only the first key 0 counts, and tags before the map or the value are stripped.

`cluster_id_cap` reproduces this. A scratch Go program against dstore v0.1.9 `view.Decode` checked 60 encodings
(built with `go run` in the scratchpad, then deleted). Those values are the `cluster_id_cap_matches_go` unit
test. They cover:
- definite, indefinite and chunked strings up to 40000 bytes;
- arrays;
- bignum, self-described and other tags;
- duplicate keys, text and negative keys, indefinite maps, top-level tags.

The 13 golden `status_cluster_prefix` cases pass.

### What cli-admin can do

`print_status` gets `Cluster::view() -> Arc<View>`. `View` keeps no capacity and the client keeps no view bytes.
So cli-admin can compute:

```rust
let cap = v.cluster_id.as_ref().map_or(0, Vec::len);
match dstore_view::status_cluster_prefix(v.cluster_id.as_deref(), cap) {
    Ok(prefix) => /* "cluster {prefix} incarnation …" */,
    Err(text) => dstore_cli::go_panic_exit(&text),
}
```

This matches Go for nil and for every definite-length or array cluster id, which covers every canonical node
reply. It differs in one case: key 0 is an indefinite-length byte string of 1..3 bytes (golden
`indefinite_two_bytes`, `indefinite_three_bytes_two_chunks`). There Go has `cap ≥ 8` and prints a
zero-padded prefix with exit 0, while Rust exits 2 with the panic line.

Why the capacity does not travel in `View`:
- `View` is a `cbor_struct!`, so every field is a CBOR field.
- `Vec::capacity` is only guaranteed to be *at least* the requested capacity, and `Clone` drops it.

### Exact alternative, and the DD proposal

The exact alternative needs the view bytes. client-a would keep the adopted reply's view bytes, for example an
added `Cluster::view_bytes()`, and cli-admin would call `dstore_view::cluster_id_cap(&bytes)`. That item is not
in PORTING §4.8, so I did not assume it.

**Proposed DD-15:** "`cluster status` against a view whose cluster id is an indefinite-length CBOR byte string
of 1..3 bytes: Go prints the bytes zero-padded to 4 (the append-grown capacity is at least 8), Rust panics as for
a definite-length short id (DD-7, exit 2). Nodes encode views canonically, with definite lengths, so only a
hand-crafted reply differs."

## Deviations and notes

1. **Where the full `all_slots` digests are tested.** They live in `crates/view/tests/all_slots.rs`, not in root
   `tests/golden_tests/view.rs` as VECTORS.md says.
   - The root package has no `blake3` dependency, core-rs exposes only truncated key hashes, and I may not edit
     the root manifest. The root module checks that file's spots (rank and owners at 6 slots per set).
   - The digest test ranks slots on up to 16 threads: 13 s in a debug build on this machine.
   - PORTING §8 `checks.tests` runs `--workspace --lib` plus the root tests, so this integration test runs only
     under CI's `cargo test --workspace`.
   - Proposal: add `--test all_slots -p dstore-view` to the flake check, or add `blake3` as a root
     dev-dependency and move the test into `tests/golden_tests/view.rs`.
2. **Order of weight-0 members in `Set::rank`.** Withdrawn in review. Rust now sorts them with a port of Go's
   `sort.Slice`, so the indexes match Go for equal ids too.
3. **`sort_nodes`** — withdrawn in review, for the same reason.
4. **`Table::owners`/`rank` with slot ≥ `SLOTS`.** Go panics (index out of range); Rust computes a result.
   Slots derived from keys never reach it.
5. **`WriteSet`/`ReadOrder` deduplicate only against the current list.** Go builds its `seen` map from the
   current list and never updates it while appending. A pending list that repeats a `NodeId` therefore appends
   it twice, for example a 31-byte id and the same bytes followed by `00`: distinct zone keys, the same
   `NodeId`.
   - Ported literally, and checked with a scratch Go program (deleted).
   - Unit test `write_set_and_read_order_dedup_only_against_the_current_list`.
6. **Locking.** `Placement` has no mutex (Go serialises lookups with one). The tables synchronise their caches,
   and results are identical.
7. **`ViewError::NotMember`** is never produced. Go's `ErrNotMember` is unused in v0.1.9 as well.
8. **`log2fix(0)`** panics, per the PORTING §4.5 signature comment. `l` never passes 0.

## Tests

- **Unit tests in `crates/view/src`: 18.**
  - The 7 tests of `placement_test.go`:
    - `log2fix_exact`: splitmix64 instead of `math/rand`, with the same tolerance;
    - `fmix64_vectors`, `golden_vectors`, `weighted_distribution`, `adding_node_moves_only_to_it`, `zone_rule`,
      `slot_of_tails`.
  - Placement additions:
    - the `log2fix(0)` panic;
    - `NodeId` forms;
    - an empty set and r = 0;
    - an explicit zone colliding with raw id bytes;
    - `Table` caching.
  - View additions:
    - `cluster_id_cap` against the 60 Go cases, and the decode error;
    - the growth rules;
    - `status_cluster_prefix` texts;
    - write-set and read-order duplicates;
    - exact 32-byte lookups.
- **`crates/view/tests/all_slots.rs`: 1 test**, the 11 set digests.
- **Root `tests/golden.rs` `view::`: 25 tests over every section of `view/view_placement.json`, plus the
  `all_slots.json` spots.**
  - `view_cbor_hand_built` also compares the structs built field by field (the §3.1 cases) with both the
    encoding and the decoded value.
  - `ticket_from_view_encode` needs `dstore_ticket::Ticket::encode`, which the ticket owner has landed.
  - Nothing is `#[ignore]`d.

## Gates

- `cargo test -p dstore-view` (unit tests and `all_slots`): pass.
- `cargo clippy -p dstore-view --all-targets --no-deps -- -D warnings`: clean.
- `rustfmt --edition 2024` over the owned files.
- Root `cargo test -p dstore-client-rs --test golden view::`: 25 passed.
  - The first run could not build: sibling `dstore-worktree` was mid-edit. A retry loop in the real checkout
    passed once it built.
  - Before that, the same module passed in a scratch workspace that `#[path]`-included it with only view,
    testkit, codec, gocompat and serde. `cargo clippy --tests --no-deps -- -D warnings` was clean there.

## Review

Reviewer: review-view. This was an adversarial review of the view implementation, with the same file ownership.

### What was checked

- **Sources read.**
  - Specs: PORTING §0-3, §4.5 and §5-7; view-placement.md in full, including Appendix A and the Addenda;
    VECTORS.md families `view` and `placement`; the impl notes of vectorgen-view, codec and ticket.
  - Go, dstore v0.1.9: `placement/placement.go`, `placement_test.go`, `view/view.go`, `worktree/flow.go`
    `TicketFromView`, `node/status.go` `ShortID`, and `cmd/dstore/client.go` `printStatus`.
  - fxamacker v2.9.3 `decode.go`: `parseToValue`, `parseByteString`, `fillByteString`, `parseArrayToSlice`,
    `parseMapToStruct`, `skip` and `getHead`.
  - go1.26.5: `runtime/slice.go` (`growslice`, `nextslicecap`), `runtime/msize.go` (`roundupsize`), and
    `internal/runtime/gc` (`sizeclasses.go`, `malloc.go`).
- **Line by line against Go.**
  - Placement math: `slot`, `salt`, `fmix64`, `log2fix` and `L`.
  - Ranking: `less`/`rank_order`, `Set.Rank`, `Set.Owners` with its byte zone keys, and `Table`, which stores nil as
    `[]`.
  - Node and view lookups: `members`, `NID`, `ZoneOrID`, and every view lookup.
  - `Placement.WriteSet`/`ReadOrder`: the seen map is built once, from the current list.
  - Id lists: `Contains`, `AddID` and `IDsOf`.
  - `DefaultMinReplicas`.
  - `ValidateChange`: the counting rule, `dropped >= replicas && replicas > 0`, and the text.
  - Every CBOR field (key, Go type name, omitempty) against the `view.go` tags.
  - `ticket_from_view`: members stay nil with no nodes, and nil and empty ids are kept.
- **Coverage.**
  - All 22 sections of `view_placement.json` are deserialised and asserted, and none is ignored.
  - The spots of `all_slots.json` are checked in the root module; the full digests are in `tests/all_slots.rs`.
  - The 7 tests of `placement_test.go` are ported.
- **`cluster_id_cap` internals.**
  - The three size-class tables match go1.26.5 `sizeclasses.go` exactly (68, 129 and 249 entries, compared by a
    script).
  - `growslice`/`nextslicecap`/`roundupsize` for noscan objects, including the index wrap for sizes 1017..1023.
  - fxamacker's definite (make + copy), indefinite (append), array (`MakeSlice(count, count)`) and tag 2/3 paths.
- **Differential corpus for `cluster_id_cap`.** A scratch go1.26.5 program over dstore v0.1.9 `view.Decode` (run
  with `go run` in the scratchpad and deleted afterwards) produced 6000 random view encodings. They cover:
  - definite and indefinite byte strings (chunks up to 32779 bytes, up to 299 chunks);
  - arrays with null and tagged elements;
  - nested tags (2, 3, 1, 21-24, 32, 100, 1000, 55799), null and undefined;
  - duplicate and non-shortest key 0;
  - text, negative and unknown keys, definite and indefinite maps, and top-level tags.

  Results, from a temporary integration test that was deleted afterwards:
  - For the 4875 views that decode, Rust `View::decode` gave Go's `len` and nil, and `cluster_id_cap` gave Go's
    `cap`, in every case.
  - For the 1125 that fail, the `view: decode: …` texts are identical and `cluster_id_cap` returns `Err`.

### Found and fixed

1. **`Set::rank` (weight-0 members) and `sort_nodes` did not reproduce Go's `sort.Slice`.**
   - **Before.** Both used stable sorts, documented as differing only beyond 12 elements with equal ids. The
     rank indexes are public API and the input of the `all_slots` digest, and `sort_nodes` is public API.
   - **Confirmed against Go.** A second scratch Go program produced the cases:
     - 600 `placement.NewSet(members).Rank(slot)` sets of 1..70 members, drawn from at most 6 distinct ids and mostly
       weight 0;
     - 600 `view.SortNodes` lists of 0..89 nodes with nil, empty and 1..2-byte ids.

     Before the fix, many rank cases gave other indexes than Go.
   - **Fix.** The new private module `src/gosort.rs` is a copy of the reviewed `dstore_udiff::gosort` pdqsort port,
     reduced to `sort.Slice`. It is a copy because PORTING §3.2 gives `dstore-view` no dependency on
     `dstore-udiff`, and this task may not add one. `Set::rank` sorts the weight-0 members through it, and
     `sort_nodes` sorts the nodes through it. The weighted members keep a stable sort: `rank_order` is a strict weak
     ordering, so any stable sort equals `sort.SliceStable`. After the fix, all 1200 Go cases match.
   - **Tests added.**
     - `gosort::tests::matches_the_udiff_port` compares the copy with `dstore_udiff::gosort::slice`: 22 sizes × 7
       moduli × 5 shapes, plus 3 coin comparators whose answers depend on the call sequence and reach
       `breakPatterns` and the heapsort fallback. `dstore-udiff` is a new dev-dependency.
     - `placement::tests::weight_zero_members_in_go_sort_slice_order` and
       `view::tests::sort_nodes_in_go_sort_slice_order` each pin 2 Go-verified cases where a stable sort differs.
   - Deviations 2 and 3 above are withdrawn.
2. **The hand-built `view_cbor` comparison covered 5 of 14 cases.** view-placement §5 asks for "the Rust struct built
   by hand == decode(hex)".
   - `view_cbor_hand_built` now builds every generator view field by field: `init`, `full` (every field populated),
     `negatives`, `maxes`, `utf8`, `writable_false_and_data_no_addrs`, `long_addrs`,
     `acl_nil_and_empty_elements`, `pending_lists`, and the first 5.
   - For each, it compares the encoding and the decoded value.
   - It fails when a `view_cbor` case has no hand-built counterpart.

### Checked, no change

- **Deviations 4-8 stand:**
  - a slot ≥ `SLOTS` computes a result where Go panics;
  - the write set and read order deduplicate only against the current list, exactly as Go's seen map;
  - `Placement` has no mutex;
  - `NotMember` is never produced;
  - `log2fix(0)` panics.
- **`cluster_id_cap` and `status_cluster_prefix`** are correct (the corpus above). The panic text is the vector's Go
  first line.
- **`crates/view/tests/all_slots.rs`** passes (13-26 s in a debug build). The flake's `checks.tests` flags do not
  include it (flake.nix is not ours to edit), so only CI's `cargo test --workspace` runs it.
- **The DD-15 proposal stays open.** `dstore_cli::common::print_status` is still `todo!()`, and `Cluster` keeps no
  view bytes. The choice between `cap = len` plus DD-15 and adding `Cluster::view_bytes()` belongs to cli-admin and
  client-a.

### Gates (review)

All run in the flake dev shell, with the review's own target directory.
- `cargo test -p dstore-view`: 21 unit tests and `all_slots_blake3` pass.
- `cargo test -p dstore-client-rs --test golden view::`: 25 pass.
- `cargo clippy -p dstore-view --all-targets --no-deps -- -D warnings`: clean.
- `cargo clippy -p dstore-client-rs --test golden --no-deps -- -D warnings`: no diagnostic in
  `tests/golden_tests/view.rs`.
- `rustfmt --edition 2024 --check` on every owned file: clean.
- The scratch Go programs, their corpora and the temporary tests are deleted.
