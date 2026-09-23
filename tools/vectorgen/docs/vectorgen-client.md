# Client families: `client`, `refglob`

Owner: vectorgen-client. Generator files: `tools/vectorgen/family_client.go` (family `client`) and
`tools/vectorgen/family_refglob.go` (family `refglob`).

```sh
nix develop -c go -C tools/vectorgen run . ../../tests/golden client refglob
```

| Family | Files |
|---|---|
| `client` | `client/rank.json`, `client/batches.json`, `client/fetch.json`, `client/verify_record.json`, `client/progress.json`, `client/backoff.json`, `client/placement_decisions.json`, `errors/client_text.json` |
| `refglob` | `refglob/refglob.json` |

The family owns exactly these files. `client/transcripts/` belongs to a later family.

Specs: port-notes/client-core.md §5, port-notes/client-transfer.md §5.2 items 3-5 and 8,
port-notes/verification.md §4.3 items 22-23. Normative Go: `github.com/amber-store/dstore` v0.1.10 (`client/`,
`refglob/`), `github.com/amber-store/core` v0.0.9, `transport-iroh` v0.4.0 `protocol`.

Conventions are those of the root `VECTORS.md`: 64-bit integers and durations are decimal strings (`I64`/`U64`
in `util.go`; Rust `dstore_testkit::golden::decimal_i64`/`decimal_u64`), bytes are lowercase hex, small
integers and booleans are JSON numbers and booleans. Durations are nanoseconds in fields ending in `_ns`. Field
names are snake_case.

## Verbatim copies and the self-check

`rankOwners`, `rttClass`, `batches`, `estSize`, `pickBatch` (with `fetchKey`, `fetchJob`, `fetchAcc` and the
get batch constants), `tracker` with its methods, `newTracker`, `countKeys` and `(*Cluster).pathAttrs` are
unexported, so `family_client.go` holds verbatim copies. Before generating anything, `clientSelfCheck`:

1. checks from the build info that the generator is built against dstore v0.1.10 without a replace, and finds
   the module directory with `go list -m`;
2. parses the dstore sources and `family_client.go` (embedded with `//go:embed`), and compares each copy with
   its original after dropping comments and renaming identifiers (`Cluster` → `clientStubCluster`, `Progress`,
   `ProgressReport`, `NodeProgress`, `PutObserver`, `RecordSizer` → `client…` aliases of the client package's
   types). The copies of `tracker` and `pathAttrs` run against `clientStubCluster`, whose `pool.Path` is
   scripted;
3. checks that the statements of `clientBackoff` occur verbatim in `(*Cluster).handleErr` and those of
   `clientWatchDelays` in `(*Cluster).WatchRefs`;
4. checks that every format string or message the error vectors use occurs as a string literal in the named
   dstore function (or package-level variable), and that the timing expressions of `backoff.json` occur in the
   source of the named functions.

Any difference fails the run. The Go tests' own expectations (`client/rank_test.go`, `client/batch_test.go`,
`refglob/refglob_test.go`) are also asserted against the copies before their cases are written.

Exported functions (`client.HumanBytes`, `client.Rate`, `client.VerifyRecord`, `client.Dial` and the `Cluster`
methods, `refglob.Compile`) are called directly.

## `client/rank.json`

```json
{
  "rtt_class": [ { "rtt_ns": "4999999", "class": 0 } ],
  "rank_owners": [
    {
      "name": "prefers_direct_over_relayed",
      "owners": [
        { "id": "<hex 32>", "penalty": 0, "path": { "direct": false, "rtt_ns": "1000000" } },
        { "id": "<hex 32>", "penalty": 0, "path": null }
      ],
      "want": [1, 0]
    }
  ]
}
```

- `rtt_class`: `rttClass(rtt)` at and around the 5 ms, 25 ms and 100 ms boundaries (0 to 3): 4.999 ms, 5 ms − 1 ns,
  5 ms, 5 ms + 1 ns, 24.999 ms, 25 ms − 1 ns, 25 ms, 99.999 ms, 100 ms − 1 ns, 100 ms, and 0, 1 ns, 333 ms, 1 h and
  `MaxInt64`.
- `rank_owners`: `rankOwners(ids, penalty, path)` with `ids` = the `id` of each owner in input order, `penalty(id)`
  = its `penalty` (0-3) and `path(id)` = `(PathInfo{direct, rtt}, true)`, or `(_, false)` when `path` is null (no
  live connection). `want` lists input indexes in ranked order. Cases: the 7 tests of `client/rank_test.go`
  (ids `id[0] = i+1`), RTT class boundaries, penalties 1-3, penalty against path, relayed near against direct
  far, unmeasured against far and relayed, RTT 0, a single owner, no owners, and 48 splitmix64 scenarios (seed
  `0x52414e4b`).
- Rust: crate unit tests of `dstore_client::rank` (`rtt_class`, `rank_owners`), owner client-a.

## `client/batches.json`

```json
{
  "default_batch_bytes": 16777216, "batch_keys": 8192, "max_put_batch": 67108864,
  "cases": [
    { "name": "caps_keys_per_batch", "sizes": [ { "size": 1, "count": 7 } ], "max_bytes": 1048576, "max_keys": 3, "want_lens": [3, 3, 1] }
  ]
}
```

- `sizes` are runs of consecutive keys with one record size; expand them in order. `batches(keys, size,
  max_bytes, max_keys)` must return batches of `want_lens` keys that, concatenated, are the input keys in input
  order (the generator checks the order). Empty input gives `[]`.
- Cases: the 4 tests of `client/batch_test.go`; `bytes+n == max_bytes` stays in the batch; one byte over; 8193
  keys at the 8192 key cap; 16 MiB target over records of 46 + 1 MiB (15 per batch); first record over the target;
  an oversized record in the middle; empty; zero-size records; `max_keys` 1; `max_bytes` 0; key cap before byte cap
  and the reverse; the 64 MiB cap; 5 splitmix64 scenarios (seed `0x42415443`).
- Rust: crate unit tests of `dstore_client::batch` (`batches`), owner client-a.

## `client/fetch.json`

```json
{
  "get_batch_keys": 2048, "get_batch_bytes": 8388608, "get_est_max": 65536,
  "est_size": [ { "name": "blob_length_65537", "key": "<hex 32>", "length": "65537", "est": "65582" } ],
  "pick_batch": [
    {
      "name": "full_by_bytes_beats_a_larger_accumulator",
      "accs": [ { "node": "<hex 32>", "keys": [ { "length": "65536", "count": 128 } ], "bytes": 8394496 } ],
      "jobs": [ { "node": 0, "count": 127, "bytes": 8328914 } ]
    }
  ]
}
```

- `est_size`: `estSize(key)` = `46 + min(int(key.Length()), 64 KiB)` for canonical keys of every type, keys that
  are not canonical, and 16 splitmix64 keys. `length` is `key.Key(k).Length()` read without validation.
  **`est` can be negative**: Go converts the `uint64` length to `int`, so a length field of 2^63 or more wraps
  (`noncanonical_0`: length 2^63, est `-9223372036854775762`; `noncanonical_1`: 2^64-1, est `45`). Such keys come
  only from user-supplied hex keys (`catalog restore KEY`). PORTING.md gives `est_size -> usize`; see
  port-notes/impl-vectorgen-client.md.
- `pick_batch`: accumulators, each with its node id and runs of keys whose length field is `length`
  (key `key.NewFromHash(Blob, length, data(seed, 32))`, the seed starting at `0x5049434b` in each case and counting
  up over the keys of every accumulator in order; only the length matters). `bytes` is the accumulator's byte count, the sum of `est_size` over its keys. `jobs` is the sequence
  of `pickBatch(acc)` results until it returns no job: `node` indexes `accs`, `count` keys are taken from the
  front of that accumulator and `bytes` (their `est_size` sum) is subtracted from its count; an emptied
  accumulator is removed. Every step has exactly one possible pick whatever Go's map order (at most one full
  accumulator, and when none is full a single one with the most keys); the generator refuses ambiguous steps.
  Cases: 5000 small keys (`2048, 2048, 904`), 200 keys at the estimate cap (`127, 73`), a full-by-bytes accumulator
  against a larger one, most keys first, full by keys, mixed sizes, exactly 8 MiB, three accumulators, a larger
  non-full accumulator first, none.
- Rust: crate unit tests of `dstore_client::fetch` (`est_size`, and `pick_batch` or whatever the fetcher's batch
  picker is called), owner client-b.

## `client/verify_record.json`

```json
{
  "cases": [
    {
      "name": "payload_byte_flipped_crc_recomputed",
      "raw": { "key": "<hex 32>", "flags": 0, "ulen": 100, "slen": 100, "bytes": "<record hex>" },
      "parses": true,
      "ok": false,
      "error": "payload hashes to 0064…, not 0064…",
      "error_portable": true
    }
  ]
}
```

- `raw` is an `amberpack.RawRecord`: the header fields and `bytes`, the complete record (46-byte header +
  stored payload). The header fields equal the header inside `bytes` in every case; `bytes` carries a valid
  CRC-32C.
- `parses`: whether `amberpack.ParseRecord(bytes)` accepts the record with exactly this header, as every record
  of a pack stream is. The cases with `parses: false` exercise `VerifyRecord` on inputs a stream never yields
  (reserved key bits, unknown flag bits, raw `ulen != slen`).
- On success (`ok: true`) `VerifyRecord` returns `out_key` (= `raw.key`) and a copy of `bytes`. Otherwise `error`
  is the exact Go text. `error_portable: false` marks texts from the zstd library (klauspost), which core-rs
  (libzstd) cannot reproduce: assert only that the record is refused.
- Cases: raw Blob, empty Blob, zstd Blob (4096 × `a`), DirLeaf and FileNode with logical lengths, XattrSet, a
  Blob whose length field is 999 over 100 bytes (accepted), a Commit and a Commit whose length field is one
  too large (accepted: the client, unlike the node, checks no length field), a flipped payload byte, another key's payload (raw and
  zstd), a zstd frame stored raw, reserved header bit, reserved types 6 and 15 (5 is Commit since core v0.0.9), non-canonical length, unknown flag
  bit 2 (accepted, raw) and 3 (accepted, zstd), raw `ulen` ignored, zstd `ulen` one more than the frame (portable
  `decompressed to 4096 bytes, header says 4097`), one less, and a garbage frame.
- Rust: `tests/golden_tests/client_transfer.rs` builds `amberpack::RawRecord { record: Record { key, flags, ulen,
  slen }, bytes }` and calls `dstore_client::verify_record`, owner client-b.

## `client/progress.json`

```json
{
  "tracker": [
    {
      "name": "ids_sort_reversed_from_first_use",
      "nodes": ["<hex 32>", "<hex 32>"],
      "paths": [null, { "direct": true, "rtt_ns": "1000000" }],
      "progress": true,
      "ops": [
        { "op": "totals", "objects": 10, "done": 4, "bytes": "600", "report": { "objects": 4, "total_objects": 10, "bytes": "0", "total_bytes": "600", "nodes": [] } },
        { "op": "start", "node": 0, "report": { "…": "…" } },
        { "op": "sent", "node": 0, "n": 100, "report": { "…": "…" } },
        { "op": "done", "node": 0, "flushed": true, "report": { "…": "…" } },
        { "op": "bytes", "result": "175", "report": null }
      ]
    }
  ],
  "count_keys": [ { "name": "three_nodes", "lists": [[1048622], [], [46, 47, 48, 49]], "objects": 5, "bytes": "1048812" } ],
  "human_bytes": [ { "n": "1048575", "out": "1024.0 KiB" } ],
  "rate": [ { "bytes": "1048576", "took_ns": "1500000000", "out": "682.7 KiB/s" } ],
  "path_attrs": [
    { "path": { "direct": false, "rtt_ns": "37200000" },
      "attrs": [ { "key": "path", "kind": "String", "string": "relay" }, { "key": "rtt", "kind": "Duration", "duration_ns": "37000000" } ],
      "text": "path=relay rtt=37ms" }
  ]
}
```

- `tracker`: a tracker over a cluster whose pool reports `paths[i]` for `nodes[i]` (null: no live connection),
  with a progress callback when `progress` is true. Each op is one call:
  - `start` / `flushed`: `observer().start(nodes[node])` / `.flushed(…)`;
  - `sent`: `observer().sent(nodes[node], n)`;
  - `done`: `observer().done(nodes[node], flushed)`;
  - `totals`: `totals(objects, done, bytes)`; `more`: `more(bytes)`; `objects`: `objects(n)`;
  - `bytes`: `bytes()` returns `result` and reports nothing.

  `report` is the one report the call delivered (null when none: `bytes`, or no callback):
  `{ "objects": n, "total_objects": n, "bytes": i64s, "total_bytes": i64s, "nodes": [ { "id": hex, "direct": bool,
  "rtt_ns": i64s, "in_flight": n, "awaiting": n, "bytes": i64s } ] }`. `nodes` of a report are ordered by id; a
  node's `direct`/`rtt_ns` come from its pool path when there is one, else `false`/`0`. Scenarios:
  ids that sort reversed from first use, mem-transport paths (`direct`, 1 ms), a relayed path, a push without
  upload (only `totals`), no callback. Node ids here and elsewhere are `crypto/ed25519.NewKeyFromSeed(data(seed,
  32))` public keys, or hand-made ids.
- `count_keys`: `countKeys` over one key list per node with the given record sizes: `objects` keys, `bytes` their
  sum.
- `human_bytes`: `client.HumanBytes(n)`, including negatives, `MinInt64` and 40 splitmix64 values.
- `rate`: `client.Rate(bytes, took)`. Cases with `took_ns <= 0` give `-`; a Rust `Duration` cannot be negative, so
  tests map `took_ns < 0` to "not positive" or skip those cases.
- `path_attrs`: `(*Cluster).pathAttrs(id)` for a live path or none: the attributes in order with their slog kind
  (`String` or `Duration`, the RTT rounded to ms), and `text`, the attributes as `slog.TextHandler` renders them.
- Rust: `human_bytes`/`rate` in `tests/golden_tests/client.rs`; `Tracker`, `count_keys` and `path_attrs` in crate
  unit tests of `dstore_client::progress` / `cluster` (they are `pub(crate)`), owner client-a.

## `client/backoff.json`

```json
{
  "handle_err": [ { "failures": 1, "backoff_ns": "5000000000" } ],
  "watch_reconnect": [ { "round": 1, "delay_ns": "1000000000", "jitter_max_ns": "500000000" } ],
  "watch_served_pause_ns": "200000000",
  "watch_refresh_timeout_ns": "15000000000",
  "dial_member_timeout_ns": "15000000000",
  "probe_timeout_ns": "3000000000"
}
```

- `handle_err`: the backoff `handleErr` sets after the node's `failures`-th consecutive non-remote failure
  (`5s << min(failures-1, 4)`, capped at 60 s): 5, 10, 20, 40, 60, 60 … s for failures 1-12, 16, 64, 1000.
- `watch_reconnect`: after `round` consecutive rounds in which no node served the watch, the delay logged as
  `in` and the upper bound of the uniform jitter added to the wait (`rand.Int64N(int64(delay/2)+1)`, so the wait
  is in `[delay_ns, delay_ns + jitter_max_ns]`): 1, 2, 4, 8, 16, 30, 30 … s. A served stream resets the delay to
  1 s and pauses `watch_served_pause_ns`.
- The timeouts: the view refresh of an unserved round, one Dial attempt per ticket member, one probe of a hinted
  node.
- Rust: crate unit tests of `dstore_client::cluster` (`handle_err`/`penalty` with a paused clock) and `watch`,
  owner client-a.

## `client/placement_decisions.json`

```json
{
  "scenarios": [
    {
      "name": "unreachable_hints",
      "view": "<CBOR of view.View>",
      "unreachable": [3, 0],
      "members": ["<hex 32>"],
      "bootstrap": 0,
      "nodes": [0, 1, 2, 3, 4],
      "replicas": 3,
      "min_replicas": 2,
      "keys": [
        {
          "key": "<hex 32>",
          "primary": 2,
          "owners": [3, 0, 2],
          "pending_owners": null,
          "write_set": [3, 0, 2],
          "read_order": [2, 3, 0, 1, 4],
          "placed": [ { "name": "min_replicas_owners", "holders": [3, 0], "placed": true } ]
        }
      ]
    }
  ]
}
```

Produced through the exported API over `transport.Network`: a fake node bound under the bootstrap id answers
`view` with `TViewReply { Incarnation 1, Epoch 7, View: view, Unreachable: members[unreachable] }` (as
`node.handleView` does); `client.Dial` with a ticket naming only the bootstrap id adopts it. Then, per key:
`Primary` (null when the key has no owners), `Owners`, `Placement().PendingOwners` (null without a pending set),
`WriteSet`, `ReadOrder`, and `Placed(key, holders)` for named holder sets (`empty`, `all_owners`,
`min_replicas_owners`, `one_owner_short`, `first_owner_repeated`, `last_owners_reversed`, `non_owners`, `write_set`,
`read_order_and_stranger`, and with a pending set `pending_owners`, `pending_only_owners`,
`min_owners_and_min_pending_owners`).

- Every id list is indexes into `members`: the view's nodes in view order, then pending-only nodes, then the
  bootstrap id when it is not a member, then one stranger id that belongs to neither set.
- `nodes` is `Cluster::nodes()`. The only live connection is the bootstrap's (mem path: direct, 1 ms, class 0),
  so preference equals rank order except for the penalties of `unreachable` hints (+1).
- Keys: all zeros, all `ff`, then `data(0x4b455953 + i, 32)`; 64 keys in the first two scenarios, 16 in the next
  three, 4 in the last.
- Scenarios: five weighted nodes (R 3, min 2); the same with a pending set of six nodes where two share zone
  `rack-a`; unreachable hints for two nodes; a weight-0 node with R 2; min_replicas 1 with a pending set at R 1;
  a view without nodes.
- Rust: `tests/golden_tests/client_transfer.rs` serves `view` and `unreachable` from a responder on
  `dstore_transport::mem`, dials with `Cluster::dial`, and compares, owner client-b.

## `errors/client_text.json`

```json
{
  "cases": [
    { "name": "dial_member_not_bound", "kind": "no_bootstrap", "go": "client.Dial, the only member is not bound on the network", "inner": "mem: 1f0c4e3b not bound", "out": "client: no bootstrap node answered: mem: 1f0c4e3b not bound" },
    { "name": "cas_mismatch_current", "kind": "cas_mismatch", "go": "(&client.CASMismatch{HasCurrent: true, Current: k1}).Error()", "has_current": true, "current": "<hex>", "out": "cas mismatch: current key 0064…" }
  ]
}
```

`out` is the exact `Error()` text; `go` says how Go produced it. `kind` names the Rust `dstore_client::Error`
variant (snake_case) or the error type, and the other fields hold what a test needs to build the value:

| `kind` | fields | Rust |
|---|---|---|
| `no_endpoint`, `no_nodes`, `watch_idle`, `fetch_ended_early`, `no_owners` | — | the unit variants |
| `no_bootstrap` | `inner` | `NoBootstrap(inner)` |
| `unknown_ref` | — | `UnknownRef` |
| `unexpected_reply`, `unexpected_frame` | `type` | `UnexpectedReply(type)`, `UnexpectedFrame(type)` |
| `cas_mismatch` | `has_current`, `current` | `CasMismatch` |
| `incomplete` | `shortfall` | `Incomplete` |
| `remote` | `code`, `text` | `dstore_wire::RemoteError` |
| `protocol_remote` | `code`, `text` | `dstore_wire::ProtocolRemoteError` |
| `payload_hash` | `want`, `key` | `PayloadHash` |
| `negotiate` | `key` (first 8 bytes), `inner` | `Negotiate` |
| `walk_local_tree`, `pull_incomplete` | `inner` | `WalkLocalTree`, `PullIncomplete` |
| `upload_to` | `node`, `inner` | `UploadTo` (node = ShortID) |
| `record_rejected` | `key` (first 8 bytes), `reason` | `RecordRejected` |
| `not_placed` | `count`, `names` | `NotPlaced` (`names` rendered `[a b]`) |
| `not_placed_last_error` | `inner` (the `not_placed` text), `last` | `NotPlacedLastError` |
| `pull_object_not_found` | `key` (first 8 bytes) | `PullObjectNotFound` |

Where the text comes from:

- real Go values: `client.ErrUnknownRef`, `client.CASMismatch`, `client.Incomplete`, `wire.Error`,
  `protocol.RemoteError`, `client.VerifyRecord`;
- real client calls over `transport.Network` with a scripted fake node: `Dial` (no endpoint, no member, a 31-byte
  member id, a member not bound, a node answering `ok`, a remote error, an undecodable view, no frame, two members
  where the last error wins), `RefGet`/`RefPut`/`RefDelete`/`RefList`/`Admin` against unexpected or remote replies,
  `Missing` and `Push` over a view without nodes (`no owners`), `Push` over an empty local packstore (`walk local
  tree`), `PullTree` over a view without nodes (`pull: object … not found in the cluster`);
- `fmt.Errorf` with the format strings of `Push`, `shortError`, `PullTree` and `watchOnce`, and the messages of
  `anyNode` and `errWatchIdle`, each checked by the self-check to occur verbatim in dstore.

Texts of the transport below the dstore wrappers are the in-memory network's (`mem: … not bound`), which
`dstore_transport::mem` reproduces. Worktree and CLI texts are not in this file.

- Rust: `tests/golden_tests/client.rs` (part A kinds) and `tests/golden_tests/client_transfer.rs` (part B kinds:
  `payload_hash`, `negotiate`, `walk_local_tree`, `upload_to`, `record_rejected`, `not_placed*`, `pull_*`,
  `fetch_ended_early`, `no_owners`).

## `refglob/refglob.json`

```json
{
  "match": [ { "pattern": "trees/**", "name": "trees/a/b", "match": true } ],
  "prefix": [ { "pattern": "tr?es/a", "prefix": "tr" } ],
  "invalid": [ { "pattern": "trees/[ab", "pattern_hex": "74726565732f5b6162", "error": "refglob: unterminated character class" } ]
}
```

- `match`: `refglob.Compile(pattern).Match(name)`; `prefix`: `.Prefix()`; `invalid`: `Compile` fails with the
  exact `error` (the node sends it as `bad-request`, printed `dstore: remote: bad-request: <error>`).
  `pattern_hex` holds the pattern's bytes; `pattern` is absent when they are not valid UTF-8.
- Cases: `refglob_test.go` `TestMatch` (38), `TestPrefix` (8) and `TestInvalid` (6), checked against Go first;
  more names per pattern (class forms, escapes, double-star placement, non-ASCII, names with `\n`, which a final
  `**` does not match but `**/` does); the prefix of every pattern used; every error text (empty, over 4096 bytes,
  invalid UTF-8, control characters, trailing backslash inside and outside a class, unterminated and empty classes,
  bad ranges). A pattern of exactly 4096 bytes compiles. Validation order is locked by patterns that fail several
  checks: empty, then length, then UTF-8, then control characters (runes below `0x20` and `0x7f` only: C1 controls
  such as U+0085 and U+2028 compile), then the syntax.
- Rust: unit tests of `dstore_testkit::refglob` (`compile`, `Glob::matches`, `Glob::prefix`). `compile` takes
  `&str`, so the invalid-UTF-8 cases apply only to a byte-level entry point; skip them otherwise.
