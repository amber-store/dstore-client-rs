# core-rs API survey and gap analysis

Area: every `github.com/amber-store/core` API the dstore client library, the
worktree package, the client-side CLI, `wire`, `view` and
`transport-iroh@v0.4.0/protocol` use, mapped onto core-rs, with the gaps.

Pins used for this survey:

| What | Pin |
|---|---|
| Go dstore (normative) | `/Users/dragan/amber-store/dstore`, tag v0.1.9, HEAD `368f2c7` (the uncommitted `cmd/dstore/wc.go` change is formatting only and was ignored) |
| Go core | `github.com/amber-store/core@v0.0.8` (module cache) |
| Go transport-iroh | `github.com/amber-store/transport-iroh@v0.4.0`. Its go.mod asks for core v0.0.7, but MVS picks v0.0.8 in dstore. The `protocol` package only uses `amberpack.NewWriter/Add/AddRecord/Close` and `fstree.Object`, which did not change. |
| core-rs | `/Users/dragan/amber-store/core-rs`, crate `amber-store-core` 0.3.0, HEAD `a85ffa1eb5ed363b9072ab224de179196cd0a046`. This equals tag `v0.3.0` on origin (lightweight tag; `git ls-remote --tags origin` prints `a85ffa1eb5ed363b9072ab224de179196cd0a046 refs/tags/v0.3.0`). The GitHub repo `amber-store/core-rs` is PUBLIC (`gh repo view`). |

A throwaway Go program under `/private/tmp/claude-502/...` checked some
behaviour against core v0.0.8, pebble v2.1.7 and transport-iroh v0.4.0, then
was deleted. Its results are marked **[verified]**. Everything else was read
from source.

---

## 1. Scope

### 1.1 Which core packages the client side pulls in

`GOWORK=off go list -deps ./client ./worktree ./wire ./view ./ticket ./codec ./placement ./transport`
lists these amber-store packages: `core/cborx`, `core/chunkers`, `core/key`,
`core/fstree`, `core/amberpack`, `core/packstore`, `core/reference`,
`core/amberignore`, `core/ingest` and `transport-iroh/protocol`. Adding
`./cmd/dstore` also brings in `core/refstore`, plus the node-side packages
`dstore/paxos`, `catalog`, `meta`, `refglob` and `node`.

core packages the client side does NOT use: `gc`, `inbox`, `tarexport`,
`tarextract`. `binaryfuse` and `chunkers` are only reached through
packstore and ingest internals.

`ingest.ScanWith` was added to core for dstore
(`docs/superpowers/specs/2026-09-15-working-copy-design.md:256-259`). No
dstore v0.1.9 code calls it: `grep -rn 'ScanWith\|ingest\.Scan'` over dstore
finds nothing. core-rs ports it anyway (`src/ingest/scan.rs:43`).

### 1.2 dstore Go files that import core (non-test)

| File | Lines | core packages | Role |
|---|---|---|---|
| `client/tree.go` | 479 | amberpack, fstree, key, packstore, reference | Push (negotiate, upload, write ref) and Pull/PullTree (frontier fetch into a local packstore, completeness check) |
| `client/objects.go` | 395 | amberpack, key | Missing, Put, putOnce (streams records), Placed, Get, `VerifyRecord` |
| `client/fetch.go` | 309 | amberpack, key | fetcher and `getStream` (reads a TGet pack, verifies each record) |
| `client/refs.go` | 150 | reference | RefGet (decodes the record), RefPut, RefDelete, RefList |
| `cmd/dstore/client.go` | 574 | amberpack, fstree, ingest, key, packstore, reference, refstore | `store push/pull --local`, `refs`, `watch`, `ref get/delete`, `ls`, `cat`, `recordPayload`, `openLocal` |
| `cmd/dstore/wc.go` | 501 | reference | clone/init/fetch/pull/push/status/diff CLI; ValidateName, ValidateUser |
| `cmd/dstore/main.go` | 698 | (none directly; `catalog restore` uses `recordPayload`) | app and node commands |
| `worktree/tree.go` | 245 | fstree, key, packstore | working-copy layout, EmptyTree, open/create, state files |
| `worktree/scan.go` | 284 | amberignore, cborx, fstree, ingest, key | disk-vs-tree scan, `entry`, `hashFile` |
| `worktree/apply.go` | 310 | cborx, fstree, key | applies changes to disk, restores xattrs |
| `worktree/change.go` | 238 | fstree, key | Change/Kind, Compare, DiffTrees |
| `worktree/diff.go` | 206 | fstree, key | TreeSource/DiskSource, Unified, Stat |
| `worktree/flow.go` | 331 | ingest, key, packstore | fetch/clone/init/pull/push flows |
| `worktree/merge.go` | 80 | (Change types only) | three-way merge |
| `worktree/xattr*.go` | 55+15+15 | (none; golang.org/x/sys) | local copy of ingest's xattr reader, plus setXattr |

`wire/wire.go` (395 lines) and `view/view.go` (469 lines) import no core
package. wire re-exports `protocol.SendPackRecords` / `protocol.NewPackReader`
(`wire/wire.go:354-362`) and `protocol.ChunkSize/TData/TDataEnd/TErr`
(`wire/wire.go:34,41-43`).

Test files that use core: `wire/wire_test.go` (`key.New`,
`amberpack.EncodeRecord`, `amberpack.NewReader`),
`worktree/{apply,change,diff,scan,merge}_test.go` (`packstore.Open(…,
WithSync(false))`, `ingest.Dir(…, Opts{Jobs: 2})`, `fstree.EncodeBlob`,
`fstree.Entry` literals).

### 1.3 transport-iroh protocol

| File | Lines | Role |
|---|---|---|
| `protocol/pack.go` | 141 | `SendPack` (unused by dstore), `SendPackRecords`, `chunkWriter`, `NewPackReader`/`packReader` |
| `protocol/protocol.go` | 180 | `ChunkSize = 1 << 20`, frame types `TData = 7`, `TDataEnd = 8`, `TErr = 10`, `WriteMsg`/`ReadMsg` (canonical CBOR enc mode, default dec mode), `RemoteError` |

### 1.4 core v0.0.8 implementation files read

`key/{key,type,errors}.go` (112/42/16), `amberpack/{record,pack}.go`
(154/219), `fstree/{object,checkcomplete,children,collect,content,reachable,list,lookup,decode,encode}.go`,
`packstore/{packstore,parallel,prepare,missing,segment}.go` (804/184/52/62/45),
`ingest/{ingest,scan,driver,parallel,meta,xattr_common,xattr_darwin,xattr_linux}.go`,
`amberignore/amberignore.go` (97), `cborx/cborx.go` (151),
`reference/reference.go` (163), `refstore/refstore.go` (159).

### 1.5 core-rs files read

`Cargo.toml`, `Cargo.lock`, `flake.nix`, `PORTING.md`, `README.md`,
`src/lib.rs`, `src/key.rs`, `src/amberpack.rs`, `src/cbor.rs`,
`src/reference.rs`, `src/refstore.rs`, `src/amberignore.rs` (API part),
`src/fstree/{mod,read,encode}.rs`, `src/packstore/{mod,parallel,prepare,missing,gc}.rs`,
`src/ingest/{mod,scan,driver,meta,xattrs}.rs`, and port-notes
`refstore.md`, `packstore.md`, `ingest.md`, `reference.md`,
`fstree-checkcomplete.md`.

### 1.6 Node-side commands (outside core-rs; what they need)

- `dialClusterLog` with `--store` and no `--ticket` (`cmd/dstore/client.go:65-69`), and `cluster ticket --store` (`main.go:341-345`), call `localTicket(dir)` (`main.go:394-405`). That calls `node.OpenOffline(dir)` (`node/node.go:776-784`), which requires `<dir>/identity` (error `node: no identity in %s: %w`) and then `node.Open`. `node.Open` opens `<store>/packstore` (`node/node.go:209`), a **Pebble** meta DB at `<store>/meta` (`node/node.go:215`, `meta/meta.go:36`) and a **Pebble** paxos acceptor at `cfg.PaxosDir` (default `<store>/paxos`; `node/node.go:231`, `paxos/acceptor.go:69`). The view comes from meta. core-rs has no Pebble reader, so Rust cannot derive a ticket from `--store` without a Pebble reader.
- `catalog restore` (`main.go:651-695`) also needs `node.OpenOffline` and `RestoreCatalog`, which writes through the paxos acceptor (Pebble).
- `serve`, `cluster init`, `node join` need the whole node (paxos, catalog, meta, reconcile, GC).

---

## 2. API used client-side, and the core-rs equivalents

Verdicts: **=** same semantics; **≈** same outcome but a different API shape,
or a documented difference that is not observable here; **≠** an observable
difference, listed in section 7.

### 2.1 `key`

| Go (core v0.0.8) | dstore call sites | core-rs (`src/key.rs`) | Verdict |
|---|---|---|---|
| `type Key [32]byte` (`key/key.go:18`) | everywhere; `key.Key(k)` / `[32]byte(k)` conversions | `pub struct Key(pub [u8; 32])` (`:101`); `Key(arr)`, `k.0`, `k.as_bytes()` | = |
| `Blob = 0`, `XattrSet = 4` (`key/type.go:13,17`) | `client/tree.go:300,353` | `Type::Blob`, `Type::XattrSet` (`:20-31`) | = |
| `(Key) Type() Type` (`key.go:21`) | `tree.go:300,353`; `objects.go:387` | `fn type_(&self) -> Type` (`:180`). **Panics** on reserved type nibble 5..15; Go returns `Type(n)`. | ≈ (G10) |
| `(Key) Length() uint64` (`key.go:31`) | `tree.go:204`; `fetch.go:24`; `scan.go:171`; `diff.go:41`; `objects.go:387` | `fn length(&self) -> u64` (`:194`). Does not panic on non-canonical keys. | = |
| `(Key) Validate() error` (`key.go:62`) | `objects.go:380` | `fn validate(&self) -> Result<(), key::Error>` (`:158`), same check order (reserved bit, type, canonical length) | = |
| `Parse(b []byte) (Key, error)` (`key.go:77`) | `tree.go:259`; `flow.go:47,247`; `worktree/tree.go:231`; `change.go:185,189,224`; `scan.go:167,191`; `apply.go:230,299`; `diff.go:37`; `cmd client.go:488,533,545` | `fn parse(b: &[u8]) -> Result<Key, key::Error>` (`:143`) | = |
| `New(t, length, serialized) (Key, error)` (`key.go:55`) | `objects.go:387`; `wire_test.go:50` | `fn new(t: Type, length: u64, serialized: &[u8]) -> Key` (`:117`), infallible. Go's only error is a reserved type, which cannot happen here. | ≈ |
| `(Key) String()` lowercase hex (`key.go:90`) | `root.String()[:16]` (`cmd client.go:264`; `wc.go:168,216,272,274,342,344,366`); `%s` of keys in errors and output (`client.go:338,464`; `objects.go:392`) | `impl Display for Key` (`:220`), lowercase hex | = |
| Error sentinels (`key/errors.go:9-15`) | surfaced in CLI errors | `enum key::Error` (`:75-90`), identical text: `key: data is not 32 bytes: got {n}`, `key: reserved header bit is set`, `key: reserved object type: {n}`, `key: non-canonical length encoding` **[verified Go text]** | = |

Hex parsing is not a core API. dstore uses `encoding/hex` (`worktree/tree.go:190,216,227`)
and a hand-rolled `hexDecode` (`cmd/dstore/client.go:554-574`, error `bad hex %q`,
ignores an odd trailing nibble). core-rs has no hex decoder, so implement one
locally (G16).

### 2.2 `amberpack`

| Go | Call sites | core-rs (`src/amberpack.rs`) | Verdict |
|---|---|---|---|
| `RecHeaderSize = 46` (`record.go:16`) | `tree.go:202,358`; `fetch.go:24`; `objects.go:383`; `cmd client.go:200` | `pub const REC_HEADER_SIZE: usize = 46` (`:38`) | = |
| `MaxPayload = 256 << 20` (`record.go:23`) | (implicit) | `pub const MAX_PAYLOAD: u32` (`:45`) | = |
| `type Record{Key, Flags, Ulen, Slen}` (`record.go:41`) | `rec.Flags`, `rec.Ulen` (`tree.go:358`; `client.go:200`) | `pub struct Record { key, flags, ulen, slen }` (`:106`) | = |
| `ParseRecord(b) (Record, error)` (`record.go:100`) | `tree.go:354`; `cmd client.go:196` | `pub fn parse_record(b: &[u8]) -> Result<Record, amberpack::Error>` (`:157`). Same check order: truncated header, tag, flags, truncated payload, raw ulen≠slen, ulen>max, compressed slen≥ulen, CRC, key. Same messages behind the `amberpack: corrupt pack data: ` prefix. | = |
| `DecodePayload(flags, ulen, stored) ([]byte, error)` (`record.go:140`) | `tree.go:358`; `objects.go:383`; `cmd client.go:200` | `pub fn decode_payload(flags: u8, ulen: u32, stored: &[u8]) -> Result<Vec<u8>, amberpack::Error>` (`:205`). libzstd capped at `ulen`. Go decodes, then reports a length mismatch. Both are Corrupt class; only the edge-case message differs. | ≈ (not observable: every call site handles records `VerifyRecord` already accepted) |
| `EncodeRecord(k, data)` (`record.go:69`) | tests only (`wire_test.go:54`); `packstore.Put` internally | `pub fn encode_record(k: Key, data: &[u8]) -> Result<Vec<u8>, amberpack::Error>` (`:126`). Raw records are byte-identical; **compressed frames differ** (G8). | ≈ |
| `NewWriter(w) *Writer`, `AddRecord(rec)`, `Close()` (`pack.go:58,92,102`) | through `protocol.SendPackRecords` (`protocol/pack.go:76-91`), which `wire.SendPackRecords` wraps (`objects.go:269`) | `Writer::new(w: W)` (`:238`), `add_record(&mut self, rec: &[u8])` (`:268`), `finish(self) -> Result<W, Error>` (`:277`). Sync `std::io::Write`, BufWriter. | ≈ (G1/G2: needs an async adapter) |
| `(*Writer) Add(o fstree.Object)` (`pack.go:74`) | `protocol.SendPack` only, which dstore does not use | `add(&mut self, k: Key, data: &[u8])` (`:255`) | ≈ |
| `NewReader(r)`, `(*Reader) Records() iter.Seq2[RawRecord, error]` (`pack.go:118,140`) | `fetch.go:293-305` | `Reader::new(r: R)` (`:316`), `records(self) -> Records<R>` (`:424`); `Records: Iterator<Item = Result<RawRecord, Error>>` (`:439-449`). Sync `std::io::Read`; `records` consumes the reader. | ≈ (G2) |
| `type RawRecord struct{ Record; Bytes []byte }` (`pack.go:125`) | `objects.go:378-395` (`raw.Key`, `raw.Flags`, `raw.Ulen`, `raw.Bytes`) | `pub struct RawRecord { pub record: Record, pub bytes: Vec<u8> }` (`:289`); use `raw.record.key`, etc. | = |
| `ErrMalformed` wrap texts (`pack.go:145-190`) | not shown to users (fetch errors are dropped, `fetch.go:252`) | `Error::Malformed(String)`. io error text differs: Go `unexpected EOF`, Rust `failed to fill whole buffer`. | ≈ (G17) |

Reader validation, which both implementations share:

1. Magic `AMBERPK\x03` (8 bytes). Error `reading magic: …` or `bad magic`.
2. Loop over tag bytes:
   - `0x00`: end of stream.
   - `0x01`: read the other 45 header bytes. If `slen > MaxPayload`, fail with `record payload %d exceeds limit %d`. Read `slen` payload bytes, then run `ParseRecord` over the whole record.
   - anything else: `bad record tag %#x`.
3. The payload hash is NOT checked. The client does that in `VerifyRecord` (`objects.go:378-395`).

### 2.3 `fstree`

| Go | Call sites | core-rs (`src/fstree`) | Verdict |
|---|---|---|---|
| `type Object{Key, Bytes}` (`object.go:10`) | `protocol/pack.go:16` (unused); `EmptyTree` (`worktree/tree.go:77-81`) | `pub struct Object { key, bytes }` (`mod.rs:36`) | = |
| `type Entry{Name, Mode, UID, GID, Mtime, ContentKey, LinkTarget, Rdev, XattrsIn, XattrsKey}` (`encode.go:32-43`) | `worktree/change.go:48,79-112`; `scan.go:143-268`; `apply.go:212-310`; `diff.go:26-73`; `cmd client.go:505,542,545` | `pub struct Entry { name, mode, uid, gid, mtime, content_key, link_target, rdev, xattrs_in, xattrs_key }` (`mod.rs:52-75`), all `Vec<u8>`/`Vec<u64>`, empty meaning absent (Go treats nil and empty the same with `bytes.Equal` and omitempty). The decoder keeps `xattrs_in` as the raw item bytes, like Go's `cbor.RawMessage` (`fx.rs:1368,1560`). | = |
| `EncodeDirLeaf(entries) (Object, error)` (`encode.go:86`) | `worktree/tree.go:77` (with `nil`) | `pub fn encode_dir_leaf(entries: &[Entry]) -> Result<Object, fstree::Error>` (`encode.rs:130`) | = (key `2001bbe6…`, bytes `80` **[verified]**) |
| `EncodeXattrSet(m map[string][]byte) (Object, error)` (`encode.go:140`) | `worktree/scan.go:260` | `pub fn encode_xattr_set(m: &BTreeMap<Vec<u8>, Vec<u8>>) -> Object` (`encode.rs:171`), infallible | = |
| `EncodeBlob(data)` (`encode.go:54`) | tests (`apply_test.go:166`) | `pub fn encode_blob(data: &[u8]) -> Object` (`encode.rs:103`) | = |
| `ChildKeys(k, data) ([]Key, error)` (`children.go:12`) | `tree.go:307,362` | `pub fn child_keys(k: Key, data: &[u8]) -> Result<Vec<Key>, ChildKeysError>` (`read.rs:368`). Same encounter order and error texts. Go's `unknown object type` branch cannot happen here because `type_()` panics first (G10). | = |
| `ReachableKeys(root, get) ([]Key, error)` (`reachable.go:23`) | `tree.go:31` (`local.Get`), error wrapped as `walk local tree: %w` | `pub fn reachable_keys<G, E>(root: Key, get: G) -> Result<Vec<Key>, WalkError<E>> where G: Fn(Key) -> Result<Vec<u8>, E> + Sync, E: Send` (`read.rs:694`). Root first, BFS order indexed by frontier position (matches Go), leaves not fetched, parallelism = `available_parallelism`. | = |
| `CheckComplete(root, get, has, jobs) ([]Key, error)` (`checkcomplete.go:27`) | `tree.go:303` (used only as a bool) and `tree.go:383` (wrapped `pull: tree incomplete after fetch: %w`); jobs = `c.cfg.Jobs` (default 8, `client/client.go:66-67`) | `pub fn check_complete<G, H, E>(root: Key, get: G, has: H, jobs: usize) -> Result<Vec<Key>, WalkError<E>> where G: Fn(Key)->Result<Vec<u8>,E> + Sync, H: Fn(Key)->Result<bool,E> + Sync` (`read.rs:742`). `jobs == 0` means available parallelism (Go: `<= 0` means GOMAXPROCS). Leaves are checked with `has` and missing ones give `WalkError::Missing` (`fstree: object <k> is missing`). Interior nodes use `get` and a failure gives `fstree: reading <k>: <err>`. Errors are picked in index order after the level finishes. | = (perf: G12) |
| `CollectEntries(k, get) ([]Entry, error)` (`collect.go:92`) | `worktree/scan.go:72`; `change.go:138,142,228`; `cmd client.go:500` (ls) | `pub fn collect_entries<G, E>(k: Key, get: G) -> Result<Vec<Entry>, WalkError<E>> where G: FnMut(Key) -> Result<Vec<u8>, E>` (`read.rs:536`). Same strictly-increasing check and texts. | = (names quoted per G4) |
| `ResolvePath(root, path, get) (Key, error)` (`collect.go:24`) | `cmd client.go:495` (ls NAME PATH, `strings.Trim(p, "/")`; skipped when `p == ""` or `p == "/"`) | `pub fn resolve_path<G,E>(root: Key, path: &str, get: G) -> Result<Key, WalkError<E>>` (`read.rs:463`). Splits on `/`, skips `""` and `.`, rejects `..` with `fstree: "<path>": ".." is not supported`. | = |
| `ResolveEntry(root, path, get) (*Entry, error)` (`collect.go:56`) | `cmd client.go:538` (cat) | `pub fn resolve_entry<G,E>(root: Key, path: &str, get: G) -> Result<Option<Entry>, WalkError<E>>` (`read.rs:498`). An empty path (or only `""` and `.` components) returns `Ok(None)`; Go returns `(nil, nil)` **[verified]**. | ≈ (G11: Go `cat NAME /` then dereferences nil) |
| `LookupEntry` (`lookup.go:16`) | through ResolvePath/ResolveEntry | `pub fn lookup_entry` (`read.rs:418`) | = |
| `WriteContent(w, k, get) error` (`content.go:13`) | `worktree/apply.go:239`; `diff.go:45`; `cmd client.go:549` (stdout) | `pub fn write_content<W: io::Write + ?Sized, G, E>(w: &mut W, k: Key, get: G) -> Result<(), WalkError<E>>` (`read.rs:653`). Same `reading <k>: <err>` and `<k> is not a file-content object (type <T>)` texts, with no `fstree:` prefix, like Go. | = |
| `ErrNotFound`, `ErrNotDir`, `*MissingObjectError` (`collect.go:14,17`; `checkcomplete.go:12`) | not matched client-side | `WalkError::{NotFound, NotDir, Missing}` plus `is_not_found()`, `is_not_dir()`, `missing_object()` (`read.rs:149-267`) | = |
| `DecodeFileNode/DirLeaf/DirNode` (`decode.go`) | through the read paths | `decode_file_node/decode_dir_leaf/decode_dir_node` (`decode.rs:9-27`), with fxamacker-lax decoding ported | = |

Getter shapes. Go's `func(key.Key) ([]byte, error)` becomes `FnMut(Key) -> Result<Vec<u8>, E>`
for the sequential walks and `Fn(Key) -> Result<Vec<u8>, E> + Sync` for
`reachable_keys` and `check_complete`. `|k| store.get(k)` over `&packstore::Store`
satisfies both, because `Store` is `Send + Sync`: its fields are Mutex/RwLock,
`Arc<SealedSegment>` (memmap2), atomics, and `gc::Writes`
(`HashMap<u64, Instant>`). In `check_complete`, `get` and `has` must share one
error type `E`; for packstore both are `packstore::Error`.

Error texts **[verified Go]**. Rust renders the same text for ASCII names:

```
fstree: "x": entry not found
fstree: 0005ea8f163db38682925e4491c5e58d4bb3506ef8c14eb78a86e908c5624a67 is not a directory object (type Blob)
fstree: reading 2009759b92959bb4297b5f40d815b91fb154c1e44ebb99f45399fe5dca16b7e1: packstore: object not found
fstree: object 00036437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd is missing
2001bbe6a9f5a0146a1f4d0381e9b0ed1ac2f1a979ce9d5ad84e46ff0b58f36b is not a file-content object (type DirLeaf)
fstree: "a\x7f\xffb\"c": entry not found        <- Go %q. Rust prints "a\u{7f}\u{fffd}b\"c" (G4)
```

### 2.4 `packstore`

| Go | Call sites | core-rs (`src/packstore`) | Verdict |
|---|---|---|---|
| `Open(dir, opts...)` + `WithSync(true)` (`packstore.go:152,58`) | `worktree/tree.go:133,159` (`<wc>/.dstore/packstore`); `cmd client.go:211` (`<local>/packstore`); tests use `WithSync(false)` | `Store::open_with(dir, Options::new().sync(true)) -> Result<Store, packstore::Error>` (`mod.rs:364`, `Options::sync` `:251`) | = apart from file modes (G5) and error text (G3) |
| directory flock (`packstore.go:164-167`, `unix.Flock(LOCK_EX\|LOCK_NB)`, message `packstore: %s is already open: %w`) | a second process on the same working copy or local store | `libc::flock(fd, LOCK_EX\|LOCK_NB)` (`mod.rs:370-377`), message `packstore: {dir} is already open: {io error}` | = for exclusion. Both use flock(2) on the directory fd, so a Go process and a Rust process exclude each other on Linux and macOS. ≠ for text (G3). **[verified Go text]** `packstore: <dir> is already open: resource temporarily unavailable` |
| `DefaultSegmentSize = 2 << 30` (`packstore.go:31`) | defaults | `DEFAULT_SEGMENT_SIZE: u64 = 2 << 30` (`mod.rs:58`). The port-note mention of "256 MiB" is wrong; the code says 2 GiB. | = |
| `(*Store) Get(k) ([]byte, error)` (`packstore.go:549`) | `client/tree.go:31,303,306,383`; `worktree/tree.go:179` (`Tree.Get`, the Getter for scan, diff, apply) | `fn get(&self, k: Key) -> Result<Vec<u8>, packstore::Error>` (`mod.rs:772`). Looks in the active index, then sealed segments newest first; a corrupt segment fails loudly. | = |
| `(*Store) GetRecord(k) ([]byte, error)` (`packstore.go:584`) | `client/tree.go:40` (RecordSource) | `fn get_record(&self, k: Key) -> Result<Vec<u8>, packstore::Error>` (`mod.rs:807`). Returns the 46-byte header plus the stored payload verbatim, with no CRC check. **[verified Go]** GetRecord equals EncodeRecord output. | = |
| `(*Store) Has(k) (bool, error)` (`packstore.go:692`) | `tree.go:295,303,383` | `fn has(&self, k: Key) -> Result<bool, packstore::Error>` (`mod.rs:914`) | = |
| `(*Store) StoredSize(k) (uint64, bool, error)` (`packstore.go:615`) | `tree.go:201` (storedSizer: `46 + n` when found, else `key.Length()`) | `fn stored_size(&self, k: Key) -> Result<Option<u64>, packstore::Error>` (`mod.rs:832`), index only | ≈ (shape) |
| `(*Store) Put(k, data)` (`packstore.go:523`) | `worktree/tree.go:164` (empty tree) | `fn put(&self, k: Key, data: &[u8]) -> Result<(), packstore::Error>` (`mod.rs:752`). Dedups via `has` and fsyncs when sync is on. | = |
| `WriteParallel(seq, WriteOpts{Writers})` (`parallel.go:46`) | `client/tree.go:419` (localWriter batches of `Object{Key, Record}`, Writers = `c.cfg.Jobs`); through `ingest.Dir` | `fn write_parallel<I, E>(&self, seq: I, opts: WriteOpts) -> (WriteStats, Result<(), packstore::Error>) where I: IntoIterator<Item = Result<Object, E>>, I::IntoIter: Send, E: std::error::Error + Send + Sync + 'static` (`parallel.rs:109`). Same dedup (sharded seen set, then `has`), per-writer fsync at `batch_size`, a final fsync always, and the first error wins. Blocking: it runs scoped threads. | = |
| `type Object{Key, Data, Record}` (`segment.go:41-45`) | `client/tree.go:345` (`Record: r.rec`); `ingest.Dir` (Data) | `pub struct Object { key, data: Vec<u8>, record: Option<Vec<u8>> }` (`mod.rs:73`); `From<fstree::Object>` | ≈. Go rejects `Data != nil` next to `Record`; Rust rejects only non-empty data. The client always sends empty data with a record. |
| `prepare` for records (`prepare.go:16-52`): ParseRecord, key equal, exact length, optional verify | same | `prepare`/`check_record` (`prepare.rs:16,41`), same texts: `record key %s does not match %s`, `record is %d bytes, want %d` | = |
| `WriteStats{Stored, Deduped, BytesStored}` (`parallel.go:20`) | `cmd client.go:264` (`stats.Stored`); `worktree/flow.go:209` (`Built`) | `WriteStats { stored: usize, deduped: usize, bytes_stored: u64 }` (`parallel.rs:21`) | = |
| `WriteOpts{Writers, BatchSize, Verify}` (`parallel.go:27`); `Writers <= 0` means GOMAXPROCS | `tree.go:425` | `WriteOpts { writers: usize, batch_size: usize, verify: bool }` (`parallel.rs:32`); 0 means available parallelism | ≈ (negative ints: G13) |
| `ErrNotFound` text `packstore: object not found` (`packstore.go:24`) | shows up in fstree errors | `Error::NotFound` with the same text (`mod.rs:101`) | = |
| `(*Store) Close()` (`packstore.go:768`) | `worktree/tree.go:176`; `cmd client.go:258,320` | `fn close(&self) -> Result<(), packstore::Error>` (`mod.rs:979`), also called from `Drop` | = |
| `os.MkdirAll(dir, 0o755)` (`packstore.go:157`); active segment `OpenFile(…, 0o644)` (`:299`) | created on first open or write | `fs::create_dir_all(&dir)` (`mod.rs:366`, mode 0777 minus umask); `OpenOptions::new().read(true).write(true).create_new(true)` (`mod.rs:498`, mode 0666 minus umask) | ≠ under a non-022 umask (G5). **[verified Go]** with umask 002: dirs `0755`, segment `0644` |
| `os.Open(dir)` error returned raw, e.g. `open <dir>: permission denied` (`packstore.go:160-163`) | edge case | `File::open(&dir)?` gives `Error::Io`, displayed as the bare io error with no `open <dir>:` prefix (`mod.rs:368`) | ≠ (G3/G6) |

Not used client-side: `Missing` (the client negotiates missing keys with the
cluster over the wire), `WriteBatch`, `SortByLocation`, `Verify`, `Wipe`, and
the GC surface.

### 2.5 `ingest`

| Go | Call sites | core-rs (`src/ingest`) | Verdict |
|---|---|---|---|
| `Dir(st, path, opts) (key.Key, packstore.WriteStats, error)` (`ingest.go:153`) | `cmd client.go:260` (`Opts{Jobs: c.Int("jobs")}`, the `store push` PATH may be a directory or a single file); `worktree/flow.go:227` (`Opts{Jobs: jobs, Exclude: []string{".dstore"}}`) | `pub fn dir(st: &packstore::Store, path: impl AsRef<Path>, opts: Opts) -> (packstore::WriteStats, Result<Key, ingest::Error>)` (`mod.rs:379`). Writers = raw `opts.jobs`, like Go. A build error that travels through write_parallel is unwrapped back to the ingest error. | ≈ (shape) |
| `Objects(path, opts) (iter.Seq2[fstree.Object, error], *key.Key, error)` (`ingest.go:116`), single-file root = content key | `worktree/scan.go:274-283` (hashFile: drain, then `*root`) | `pub fn objects(path, opts: Opts) -> Result<(ObjectStream, Root), ingest::Error>` (`mod.rs:311`); `ObjectStream: Iterator<Item = Result<fstree::Object, ingest::Error>>`; `Root::get() -> Option<Key>`, set only after a clean full drain (`mod.rs:260-268`) | = |
| `statPath` follows symlinks and rejects non-file, non-dir paths (`ingest.go:173-186`) | a `store push` path argument | `stat_path` (`mod.rs:416-431`), with `{path} is neither a regular file nor a directory` | = (except errno text G3). **[verified Go]** `stat <path>: no such file or directory` |
| `Opts{Jobs int; Chunk ChunkOpts; NoIgnore bool; Progress; Exclude []string}` (`ingest.go:52-66`) | `Jobs`, `Exclude` | `Opts { jobs: usize, chunk: ChunkOpts, no_ignore: bool, progress: Option<Arc<dyn Progress>>, exclude: Vec<OsString> }` (`mod.rs:79-95`), derives `Default` | ≈ (negative jobs: G13) |
| `Exclude`: names directly under the root, skipped whatever NoIgnore says, root only (`parallel.go:45`; `scan.go:101`) | `.dstore` | `Exclude::skips(dir == root && names.contains(name))` (`mod.rs:226-248`), bytewise | = |
| `DefaultXattrInlineMax = 256` (`ingest.go:30`) | `worktree/scan.go:257` | `pub const DEFAULT_XATTR_INLINE_MAX: usize = 256` (`mod.rs:50`) | = |
| `ScanWith(dir, opts)` (`scan.go:38`) | **unused** | `pub fn scan_with(dir, opts: &Opts) -> Result<(u64, u64), ingest::Error>` (`scan.rs:43`) | = (not needed) |
| xattr reader `readXattrs` (`ingest/xattr_*.go`) | copied into `worktree/xattr*.go` | `ingest::xattrs::read_xattrs` is `pub(crate)` (`xattrs.rs:29`) | gap (G9) |
| `deviceNumbers` via `unix.Major/Minor` (`ingest/meta.go:31-35`) | copied into `worktree/scan.go:246-248` | `ingest::meta::device_numbers` is `pub(crate)` (`meta.rs:45`) | gap (G9) |

The object order of `objects` is unspecified in both implementations. Keys
and roots are deterministic. core-rs verified root keys against the Go CLI
(`port-notes/ingest.md` "Interop verification").

### 2.6 `amberignore`

| Go | Call sites | core-rs (`src/amberignore.rs`) | Verdict |
|---|---|---|---|
| `Root(rootDir) (*Matcher, error)` (`amberignore.go:32`) | `worktree/scan.go:35` | `Matcher::root(root_dir: impl AsRef<Path>) -> io::Result<Matcher>` (`:49`). Returns a bare `io::Error`. Go's error is the `*fs.PathError` from `os.ReadFile`, i.e. `open <dir>/.amberignore: <errno>`. Wrap it locally the way core-rs `ingest::Error::ignore_load` does (`ingest/mod.rs:214-220`). | ≈ (G3) |
| `(*Matcher) Descend(absDir, name string)` (`:38`) | `scan.go:126,187` | `fn descend(&self, abs_dir: impl AsRef<Path>, name: &[u8]) -> io::Result<Matcher>` (`:55`). Go's nil receiver maps to `descend_opt(Option<&Matcher>, …)` (`:92`); worktree always has a non-nil matcher. | = |
| `(*Matcher) Ignored(name string, isDir bool) bool` (`:48`) | `scan.go:59` with `de.IsDir()` (d_type from readdir, no stat) | `fn ignored(&self, name: &[u8], is_dir: bool) -> bool` (`:64`). Pass `DirEntry::file_type()?.is_dir()`, which also uses d_type and does not follow symlinks. | = |

### 2.7 `cborx`

| Go | Call sites | core-rs (`src/cbor.rs`) | Verdict |
|---|---|---|---|
| `EncodeXattrs(m map[string][]byte) []byte` (`cborx.go:45`) | `worktree/scan.go:256` | `pub fn encode_xattrs(m: &BTreeMap<Vec<u8>, Vec<u8>>) -> Vec<u8>` (`:220`), same encoded-key sort | = **[verified vector §3.6]** |
| `DecodeXattrs(b) (map[string][]byte, error)` (`cborx.go:67`) | `worktree/apply.go:297,307` (errors surface as `<path>: <err>`) | `pub fn decode_xattrs(b: &[u8]) -> Result<BTreeMap<Vec<u8>, Vec<u8>>, cbor::Error>` (`:245`) | ≠ in error text: Go prefixes `cborx: `, Rust `cbor: `; Go's truncation error is plain `unexpected EOF`, Rust says `cbor: unexpected EOF`; name quoting also differs (G4). **[verified Go]** `cborx: xattr map claims 1 pairs in 1 bytes`, Rust `cbor: xattr map claims 1 pairs in 1 bytes` (`cbor.rs:103`). Only corrupt objects trigger it. |

Go ranges over a map, so `apply.go:273` sets xattrs in random order; Rust's
`BTreeMap` iterates sorted. Only the choice of which failing xattr gets
reported could differ, and xattr failures are best-effort anyway (G15).

### 2.8 `reference`

| Go | Call sites | core-rs (`src/reference.rs`) | Verdict |
|---|---|---|---|
| `type Reference{Name, Key, User, CreatedAt, Signature, PublicKey}` (`reference.go:46`) | `client/refs.go:17`; `client/tree.go:126`; `cmd client.go:293` | `pub struct Reference { name: String, key: Vec<u8>, user: String, created_at: i64, signature: Vec<u8>, public_key: Vec<u8> }` (`:317`) | = |
| `(Reference) Encode() ([]byte, error)` (`:128`) | `tree.go:127`; `cmd client.go:294` (error ignored) | `fn encode(&self) -> Result<Vec<u8>, reference::Error>` (`:384`), returns bare validation errors | = **[verified vector §3.5]** |
| `Decode(b) (Reference, error)` (`:139`) | `client/refs.go:86` | `fn decode(b: &[u8]) -> Result<Reference, reference::Error>` (`:394`): unmarshal, validate (wrapped `invalid reference: `), then byte-compare with the re-encoding (`reference encoding is not canonical`) | = (only decode-stage wording is approximate; port-notes list the known classes) **[verified]** `decoding reference: cbor: 1 bytes of extraneous data starting at index 61` has the same format (`reference.rs:211`) |
| `ValidateName(name) error` (`:58`) | `cmd client.go:249`; `wc.go:138,185` | `pub fn validate_name(name: &str) -> Result<(), NameError>` (`:265`). Messages are verbatim. Non-UTF-8 names cannot reach it (`&str`), so the CLI must reject non-UTF-8 args first with Go's text `reference name must be valid UTF-8` (Go receives raw bytes from argv). | ≈ |
| `ValidateUser(user) error` (`:86`) | `wc.go:121` (wrapped `user: %w`) | `pub fn validate_user(user: &str) -> Result<(), UserError>` (`:292`), same messages | ≈ (same UTF-8 caveat) |

**[verified Go]** `reference name must not contain '@'`, `user must not be empty`,
`reference name must not be empty`.

### 2.9 `refstore`

| Go | Call sites | core-rs (`src/refstore.rs`) | Verdict |
|---|---|---|---|
| `Open(dir, sync) (*Store, error)`, Pebble (`refstore.go:39`) | `cmd client.go:215` (`<local>/refs`, sync true), opened before any network I/O by `store push` and `store pull` | `Store::open(dir: impl AsRef<Path>, sync: bool) -> Result<Store, refstore::Error>` (`:114`). redb file `<dir>/refs.redb`; error prefix `refstore: opening redb: `. | ≠ (G7) |
| `(*Store) Put(name, record)` (`:52`) | `cmd client.go:295` (after push, error ignored); `:335` (after pull, error returned) | `fn put(&self, name: &str, record: &[u8]) -> Result<(), refstore::Error>` (`:133`) | = semantics, ≠ storage |
| `(*Store) Close()` (`:157`) | `cmd client.go:259,321` (defer) | dropping the Store closes it; no explicit close | ≈ |

The dstore CLI never reads local refs; it only writes them.

### 2.10 `transport-iroh/protocol` pack helpers (used through `wire`)

`SendPackRecords(w io.Writer, recs iter.Seq2[[]byte, error]) error`
(`protocol/pack.go:76-91`):

- It wraps `w` in `chunkWriter` and that in `amberpack.NewWriter` (bufio). For each record: if `err != nil`, return it without writing the terminator. Otherwise `AddRecord(rec)`. Then `Close()` (magic if not yet written, then `0x00`, then flush) and `finish()`.
- `chunkWriter.Write` (`pack.go:39-52`) buffers into chunks of exactly `ChunkSize = 1 << 20` and emits each full chunk as `WriteMsg(Msg{Type: TData, Data: buf})`. `finish()` (`pack.go:63-68`) flushes a non-empty remainder as one last TData, then writes `Msg{Type: TDataEnd}`. **Frame boundaries are therefore every 1 MiB of pack stream**, whatever bufio's flush pattern. A pack whose length is an exact multiple of 1 MiB has no short TData frame before TDataEnd.
- dstore's `putOnce` (`client/objects.go:269-282`) yields records from `src(k)` and **silently skips a key whose `src` fails** (`continue`).

`NewPackReader(r io.Reader) io.Reader` (`pack.go:101-141`):

- It reads frames with `protocol.ReadMsg`. `TData` feeds bytes; `TDataEnd` makes the reader return EOF; `TErr` returns `*RemoteError` (`remote: <code>: <text>`); any other type returns `protocol: unexpected frame: type %d during pack transfer`. Errors are sticky.
- `amberpack.Reader` stops at its own end marker without reading EOF, so consumers must drain the reader (`io.Copy(io.Discard, pr)`, `client/fetch.go:306`) to consume TDataEnd before reading more frames.

core-rs has no protocol module, so implement both helpers locally over async
streams (G1/G2). `ReadMsg`/`WriteMsg` framing belongs to the wire area. The
TData/TDataEnd CBOR maps come out byte-identical whether built from
`protocol.Msg` (canonical options) or `wire.Msg` (core-det options), because
both use integer keys 0 and 8 **[verified bytes §3.3]**.

### 2.11 Concurrency, blocking, timeouts

- Every core-rs API is synchronous and blocking: pread/mmap reads, fsync, scoped-thread fan-out in `write_parallel`, `check_complete`, `reachable_keys`, `ingest::dir` and `objects`. Go calls them inline from goroutines. In tokio, wrap each call that does I/O in `tokio::task::spawn_blocking` (or `block_in_place` on a multi-thread runtime). The worst offenders are `write_parallel` (localWriter), `ingest::dir`, `reachable_keys` and `check_complete`.
- `client/tree.go:276-386` PullTree calls `local.Has`, `CheckComplete`, `Get` and `ChildKeys` inside the select loop, so the fetch loop blocks while they run. A Rust port can keep that (block_in_place) or move the want-expansion into spawn_blocking. The observable behaviour (counts, order of `st.Keys`) must not change.
- localWriter (`tree.go:391-479`) batches 16 MiB of records (`pullWriteBytes = 16 << 20`) through a channel of capacity 1 on its own goroutine, with `failed` and `done` channels. Rust: a `tokio::sync::mpsc::channel(1)` plus a blocking task calling `write_parallel(objs.into_iter().map(Ok::<_, Infallible>), WriteOpts { writers: jobs, ..Default::default() })`. `std::convert::Infallible` implements `std::error::Error`.
- core has no timeouts or retries of its own. All timeouts and backoff live in client code (`10 * RequestTimeout` per stream and so on, other areas).

### 2.12 Client code that re-implements core internals

| Need | Go location | core-rs status |
|---|---|---|
| xattr listing and reading the way ingest does it | `worktree/xattr.go`, `xattr_darwin.go` (`unix.Listxattr/Getxattr`, follows symlinks), `xattr_linux.go` (`Llistxattr/Lgetxattr`) | `pub(crate) read_xattrs` (`ingest/xattrs.rs:29-73`), not exported |
| setting xattrs | `worktree/xattr_darwin.go` `unix.Setxattr(path, name, value, 0)`; linux `unix.Lsetxattr` | none (core-rs `tarextract` has its own private one) |
| major/minor/mkdev | `worktree/scan.go:247-248`, `apply.go:144` | `pub(crate) device_numbers` (`ingest/meta.rs:45-76`); no mkdev |
| mtime in ns (`info.ModTime().UnixNano()`) | `worktree/scan.go:229` | `pub(crate) entry_meta` (`ingest/meta.rs:31`) |

---

## 3. Byte and text formats (verbatim)

### 3.1 Key

`k[0] = type << 4 | (lengthSize - 1)`, bit 3 is reserved. Then the length,
big-endian, minimal (`length == 0` is a single `0x00`). Then the leading bytes
of the 32-byte BLAKE3 digest, filling the rest. The text form is 64 lowercase
hex chars. Output uses `root.String()[:16]` in many places.

### 3.2 Record

```
offset 0    tag      0x01
offset 1    key      32 bytes
offset 33   flags    0x00 raw | 0x01 zstd
offset 34   ulen     u32 BE (uncompressed length)
offset 38   slen     u32 BE (stored length)
offset 42   crc      u32 BE, CRC-32C (Castagnoli) over bytes[0:42] ‖ 00000000 ‖ payload
offset 46   payload  slen bytes
```

Compression is used only if strictly smaller. **[verified Go]** Blob `"hello"`, raw:

```
01 0005ea8f163db38682925e4491c5e58d4bb3506ef8c14eb78a86e908c5624a67 00 00000005 00000005 da59ea19 68656c6c6f
```

**[verified Go]** 4096 zero bytes give a record of 61 bytes, flags `01`, slen
`0000000f`, frame `28b52ffd64000f03800000dbbbd832`. That is klauspost output:
frame header `0x64` means single-segment, 2-byte content size and **content
checksum**. libzstd's default (`zstd::bulk::compress(data, 3)`) has no
checksum, so the Rust frame, and with it slen, the CRC and the record bytes,
differ. Never golden-compare compressed records.

### 3.3 Wire pack and TData framing

Pack stream: `"AMBERPK\x03"` (`41 4d 42 45 52 50 4b 03`), then records
concatenated, then `00`.

TData frame: `u32 BE len ‖ CBOR {0: 7, 8: bstr(chunk)}`. TDataEnd frame:
`00000003 a1 00 08`.

**[verified Go]** One-record pack (the "hello" record):

```
00000042 a2 00 07 08 58 3c 414d424552504b03 010005ea8f163db38682925e4491c5e58d4bb3506ef8c14eb78a86e908c5624a67000000000500000005da59ea1968656c6c6f 00
00000003 a1 00 08
```

**[verified Go]** Empty pack:

```
0000000e a2 00 07 08 49 414d424552504b03 00
00000003 a1 00 08
```

A full 1 MiB chunk has the byte-string head `5a 00100000`.

### 3.4 Empty tree (DirLeaf with no entries)

**[verified Go]** key `2001bbe6a9f5a0146a1f4d0381e9b0ed1ac2f1a979ce9d5ad84e46ff0b58f36b`,
bytes `80`.

### 3.5 Reference record

Canonical CBOR map with integer keys 0-5, omitting keys 4 and 5 when empty.
**[verified Go]** `Reference{Name: "team/a", Key: <hello key>, User: "alice", CreatedAt: 1700000000000000000}`:

```
a4 00 66 7465616d2f61 01 58 20 0005ea8f163db38682925e4491c5e58d4bb3506ef8c14eb78a86e908c5624a67 02 65 616c696365 03 1b 17979cfe362a0000
```

### 3.6 Xattr map (inline DirLeaf key 8, or the XattrSet body)

**[verified Go]** `{"user.a": "1", "user.b": "2"}` gives
`a2 46 757365722e61 41 31 46 757365722e62 41 32`. Inline when
`len(enc) <= 256`, otherwise spilled to an XattrSet and referenced by key 9.

### 3.7 Error strings the client can print (Go, verbatim)

| Source | Text |
|---|---|
| key | `key: data is not 32 bytes: got 31` · `key: reserved object type: 5` · `key: reserved header bit is set` · `key: non-canonical length encoding` **[verified]** |
| packstore | `packstore: object not found` · `packstore: store closed` · `packstore: <dir> is already open: resource temporarily unavailable` **[verified]** · `packstore: creating <dir>: <err>` |
| refstore (Pebble) | `refstore: opening pebble: lock held by current process` **[verified, second open in the same process]** · `refstore: reference not found` |
| fstree | see §2.3 |
| ingest | `stat <path>: no such file or directory` **[verified]** · `<path> is neither a regular file nor a directory` · `<path>: unsupported file type 0140000` |
| reference | `reference name must not be empty` · `reference name exceeds 1024 bytes` · `reference name must be valid UTF-8` · `reference name must not contain '@'` · `reference name must not contain control characters` · `user must not be empty` · `user exceeds 1024 bytes` · `user must be valid UTF-8` · `user must not contain control characters` · `reference key: <key err>` · `invalid reference: <err>` · `reference encoding is not canonical` |
| cborx | `cborx: expected CBOR map (major 5), got major %d` · `cborx: xattr map claims %d pairs in %d bytes` **[verified]** · `cborx: xattr key %d: %w` · `cborx: xattr value for %q: %w` · `cborx: %d trailing bytes after xattr map` · `cborx: unsupported additional info %d` · `unexpected EOF` |
| encoding/hex (worktree state) | `encoding/hex: odd length hex string` ("abc", "a") · `encoding/hex: invalid byte: U+007A 'z'` ("zz", and "abz": the invalid byte is reported before the odd length) · `encoding/hex: invalid byte: U+0067 'g'` ("0g") · `encoding/hex: invalid byte: U+00C3 'Ã'` ("ÿ": the first raw byte `c3` is printed as a rune) **[verified]** |
| client | `walk local tree: %w` · `pull: tree incomplete after fetch: %w` · `payload hashes to %s, not %s` · `object %s not found` · `no data` · `not a regular file with content` |

---

## 4. Rust design

### 4.1 Depending on core-rs

```toml
[dependencies]
# Public repo; tag v0.3.0 == a85ffa1 (lightweight tag). Pin the rev: tags can move.
amber-store-core = { git = "https://github.com/amber-store/core-rs", rev = "a85ffa1eb5ed363b9072ab224de179196cd0a046" }
xattr = "1"          # same major core-rs locks (1.6.1)
libc = "0.2"
thiserror = "2"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "io-util", "sync", "time", "fs", "signal"] }
```

- **Edition 2024.** Cargo.toml declares no `rust-version`, but core-rs uses `if let … && …` let-chains (e.g. `src/fstree/read.rs:555-557`), which were stabilised in Rust 1.88. So the effective MSRV is **1.88**. iroh 1.0.3 declares `rust-version = "1.91"`, so the workspace MSRV is **1.91**. nixpkgs rustc 1.95 is fine; the local rustc 1.86 cannot build core-rs or iroh.
- **Features:** core-rs defines none. Its dependencies (Cargo.lock): blake3 1.8.6, crc32c 0.6.8, zstd 0.13.3 → zstd-safe 7.2.4 → zstd-sys 2.0.16+zstd.1.5.7 (**builds vendored libzstd with `cc`**, so the dev shell needs a C compiler; core-rs's flake sets `hardeningDisable = ["all"]`), redb 2.6.3 (rust-version 1.85), thiserror 2.0.20, memmap2 0.9.11, libc 0.2.189, xattr 1.6.1. iroh 1.0.3 needs `blake3 >= 1.8.3`, which unifies with core-rs.
- **Platforms:** Unix only (`std::os::unix`, `libc::flock`, xattr). That matches Go's `golang.org/x/sys/unix` use.
- **Nix flake:** `rustPlatform.buildRustPackage` with `cargoLock.lockFile` needs `outputHashes."amber-store-core-0.3.0" = "sha256-…"` for the git dependency. crane handles git dependencies from Cargo.lock without extra hashes. The repo is public, so no fetch credentials are needed.

### 4.2 Module layout

`src/corex/`: a thin adapter layer over core-rs. The rest of the client uses
it rather than core-rs directly.

```rust
// src/corex/mod.rs
pub use amber_store_core::{amberignore, amberpack, cbor, fstree, ingest, key, packstore, reference, refstore};
pub use amber_store_core::key::{Key, Type};

// src/corex/store.rs — local packstore handle
pub struct LocalStore { inner: std::sync::Arc<packstore::Store> }
impl LocalStore {
    /// packstore.Open(dir, WithSync(true)). Pre-creates dir with mode 0755 (G5).
    pub fn open(dir: &std::path::Path) -> Result<LocalStore, packstore::Error>;
    pub fn raw(&self) -> &packstore::Store;                                   // for fstree getters
    pub async fn get(&self, k: Key) -> Result<Vec<u8>, packstore::Error>;        // spawn_blocking
    pub async fn get_record(&self, k: Key) -> Result<Vec<u8>, packstore::Error>;
    pub async fn has(&self, k: Key) -> Result<bool, packstore::Error>;
    pub fn stored_size(&self, k: Key) -> Result<Option<u64>, packstore::Error>; // index only
    pub async fn put(&self, k: Key, data: Vec<u8>) -> Result<(), packstore::Error>;
    /// localWriter batch: Object { key, data: vec![], record: Some(rec) }.
    pub async fn write_records(&self, objs: Vec<packstore::Object>, writers: usize)
        -> Result<packstore::WriteStats, packstore::Error>;
    pub fn close(&self) -> Result<(), packstore::Error>;
}
/// client/tree.go:199-206 storedSizer
pub fn stored_size_of(st: &packstore::Store, k: &[u8; 32]) -> usize; // 46 + slen, else key.length()

// src/corex/record.rs
/// client/objects.go:378-395
pub fn verify_record(raw: &amberpack::RawRecord) -> Result<([u8; 32], Vec<u8>), VerifyError>;
/// cmd/dstore/client.go:195-201
pub fn record_payload(rec: &[u8]) -> Result<Vec<u8>, amberpack::Error>;
/// client/fetch.go:23-25
pub fn est_size(k: &[u8; 32]) -> usize; // 46 + min(length, 64 << 10)

// src/corex/pack.rs — transport-iroh protocol pack helpers, async
pub const CHUNK_SIZE: usize = 1 << 20;
/// protocol.SendPackRecords: magic ‖ records ‖ 00 into exact 1 MiB TData frames, then TDataEnd.
pub async fn send_pack_records<W, I, E>(w: &mut W, recs: I) -> Result<(), PackError>
where W: tokio::io::AsyncWrite + Unpin, I: IntoIterator<Item = Result<Vec<u8>, E>>, E: Into<PackError>;
/// protocol.NewPackReader + amberpack.Reader.Records, as one async state machine.
pub struct PackRecords<R> { /* frame reader, buffered TData bytes, state: Magic|Records|Done, sticky err */ }
impl<R: tokio::io::AsyncRead + Unpin> PackRecords<R> {
    pub fn new(r: R) -> Self;
    /// Ok(None) on the pack's end marker. Validation per amberpack.go:140-194, using amberpack::parse_record.
    pub async fn next(&mut self) -> Result<Option<amberpack::RawRecord>, PackError>;
    /// io.Copy(io.Discard, pr): read frames through TDataEnd.
    pub async fn drain(&mut self) -> Result<(), PackError>;
}

// src/corex/getter.rs — run fstree walks against the cluster
/// Runs f on the blocking pool. Inside it, a getter may call Handle::block_on(cluster.get_one(k)).
pub async fn blocking<F, T>(f: F) -> T where F: FnOnce() -> T + Send + 'static, T: Send + 'static;

// src/corex/xattr.rs — worktree/xattr*.go
pub fn read_xattrs(path: &std::path::Path) -> std::io::Result<std::collections::BTreeMap<Vec<u8>, Vec<u8>>>;
pub fn set_xattr(path: &std::path::Path, name: &[u8], value: &[u8]) -> std::io::Result<()>;

// src/corex/meta.rs — x/sys/unix and time.UnixNano
pub fn unix_nano(md: &std::fs::Metadata) -> i64; // mtime.wrapping_mul(1e9).wrapping_add(mtime_nsec)
pub fn major(dev: u64) -> u32; pub fn minor(dev: u64) -> u32; pub fn mkdev(major: u32, minor: u32) -> u64;

// src/corex/refs.rs — <local>/refs
pub struct LocalRefs(refstore::Store);
impl LocalRefs {
    pub fn open(dir: &std::path::Path) -> Result<LocalRefs, RefsError>; // refstore::Store::open(dir, true), policy per D1
    pub fn put(&self, name: &str, record: &[u8]) -> Result<(), refstore::Error>;
}
pub fn looks_like_pebble(dir: &std::path::Path) -> std::io::Result<bool>;

// src/corex/goerr.rs — Go-compatible rendering (G3, G4)
pub fn go_errno_text(code: i32) -> Option<&'static str>;
pub fn rewrite_os_errors(msg: &str) -> String;
pub fn go_quote(b: &[u8]) -> String;                       // fmt %q of a []byte/string
pub fn render_walk_error<E: std::fmt::Display>(e: &fstree::WalkError<E>) -> String;
pub fn render_cbor_error(e: &cbor::Error) -> String;       // "cborx: …" texts

// src/corex/gohex.rs — encoding/hex
pub fn decode_string(s: &str) -> Result<Vec<u8>, HexError>; // texts per §3.7
pub fn encode_to_string(b: &[u8]) -> String;
```

### 4.3 Sketches for the local implementations

**send_pack_records** (byte-identical framing):

```rust
let mut buf = Vec::with_capacity(CHUNK_SIZE);
let mut wrote_magic = false;
push(&mut buf, b"AMBERPK\x03");                 // on the first record, or in finish
for rec in recs { let rec = rec?; push(&mut buf, &rec); }
push(&mut buf, &[0x00]);
// push(): copy into buf; whenever buf.len() == CHUNK_SIZE, write_msg(TData, buf) and clear.
if !buf.is_empty() { write_msg(w, Msg { typ: 7, data: buf }) }
write_msg(w, Msg { typ: 8 })
```

An iterator error aborts without the terminator and is returned. Go's
behaviour differs slightly: bytes still sitting in bufio or chunkWriter are
never written, while whole 1 MiB chunks already flushed stay written. To
match, emit a TData frame only when a chunk fills, never earlier.

**PackRecords::next**:

1. Keep a byte cursor over the current TData payload. When it runs out, read the next frame:
   - `TData`: set it as the current payload.
   - `TDataEnd`: this is EOF.
   - `TErr`: `remote: <code>: <text>` (protocol `RemoteError`).
   - anything else: `protocol: unexpected frame: type <n> during pack transfer`.
2. Parse the stream exactly as `amberpack.Reader.Records` does:
   - magic: `amberpack: malformed pack stream: reading magic: unexpected EOF` / `bad magic`
   - tag `00`: done
   - tag `01`: read 45 more header bytes; check `slen <= 256 MiB`; read the payload; `parse_record` wrapped as Malformed
   - other tag: `bad record tag 0x..`
3. Errors are sticky.

**verify_record**:

```rust
let k = raw.record.key; k.validate()?;
let payload = amberpack::decode_payload(raw.record.flags, raw.record.ulen, &raw.bytes[46..])?;
let want = Key::new(k.type_(), k.length(), &payload);
if want != k { return Err(format!("payload hashes to {want}, not {k}")) }
Ok((k.0, raw.bytes.clone()))
```

**read_xattrs** (Go `worktree/xattr.go:11-47`):

- List with `xattr::list_deref` on macOS and `xattr::list` (llistxattr) on Linux (`xattr-1.6.1/src/sys/linux_macos.rs:110-119`).
- If the list call fails with ENOTSUP or EOPNOTSUPP, return an empty map. Any other list error is returned.
- Skip empty names.
- For each name, `get_deref` / `get`. `Ok(None)` becomes an ENOATTR (macOS) or ENODATA (Linux) error. core-rs does the same in `ingest/xattrs.rs:40-66`.

**set_xattr**: `xattr::set_deref` on macOS (setxattr, follows), `xattr::set` on
Linux (lsetxattr). Both use empty flags (`linux_macos.rs:95-99`).

**meta** (golang.org/x/sys v0.47.0 `unix/dev_darwin.go`, `dev_linux.go`):

- Darwin: `major = (dev >> 24) & 0xff`, `minor = dev & 0xffffff`, `mkdev = major << 24 | minor`.
- Linux: `major = ((dev & 0xfff00) >> 8) | ((dev & 0xfffff00000000000) >> 32)`; `minor = (dev & 0xff) | ((dev & 0x00000ffffff00000) >> 12)`; `mkdev = (maj & 0xfff) << 8 | (maj & 0xfffff000) << 32 | (min & 0xff) | (min & 0xffffff00) << 12`.

**go_errno_text**: generate the table from Go's own `syscall` tables (e.g.
`/Users/dragan/go/pkg/mod/golang.org/toolchain@v0.0.1-go1.26.5.darwin-arm64/src/syscall/zerrors_darwin_arm64.go`
and `zerrors_linux_amd64.go` / `zerrors_linux_arm64.go`, the `errors` array),
per `cfg(target_os)`. **[verified]** EAGAIN renders as `resource temporarily unavailable`
and ENOENT as `no such file or directory`.

**rewrite_os_errors**: Rust renders an OS io::Error as `"<strerror> (os error N)"`.
For each ` (os error N)` suffix in a message, rebuild the prefix via
`std::io::Error::from_raw_os_error(N).to_string()` and replace the whole
substring with `go_errno_text(N)`. Apply it once, at the top-level
`dstore: <err>` print.

### 4.4 Rust iroh mapping (only what matters here)

`iroh::endpoint::{SendStream, RecvStream}` are re-exported from `noq`
(iroh-1.0.3 `src/endpoint.rs:103-113`; `Cargo.toml [dependencies.noq] version = "1.1.0"`).
`noq-1.1.1/src/send_stream.rs` and `recv_stream.rs` implement tokio
`AsyncWrite`/`AsyncRead`, so `send_pack_records` and `PackRecords` run directly
on them. Go's `s.CloseWrite()` corresponds to `SendStream::finish()` and
`CancelRead(0)` to `RecvStream::stop(0u32.into())`; that belongs to the
transport area. core-rs needs nothing from iroh.

---

## 5. Golden vectors a Go generator should emit

Write them under `tests/golden/core/` as JSON plus binary files. Use splitmix64
payloads as in core-rs `VECTORS.md`.

1. **Records, raw**: Blob "hello" (§3.2); Blob 0 bytes; incompressible splitmix blobs of 1, 64 KiB and 64 KiB + 1 bytes. Expected: key hex and record hex. Rust `encode_record` must match byte for byte when the payload is incompressible.
2. **Records, compressed (decode only)**: 4096 zeros (§3.2) and a 1 MiB repeating-text payload. Expected: Go record hex plus the payload hash. Rust `parse_record` + `decode_payload` must accept them and reproduce the payload. Do NOT compare Rust encodings.
3. **Pack frames**: one record (§3.3); empty pack (§3.3); records adding up to a pack of exactly `1<<20` bytes (one TData, then TDataEnd directly); `1<<20 + 1` bytes (two TData); three records with a mid-stream iterator error (bytes written before the abort, no TDataEnd). Expected: full hex streams. The Rust `send_pack_records` output must match.
4. **Pack reader negatives**: a TErr frame in the middle (code `busy`, text `x`); an unknown frame type 9; bad magic; truncated before the end marker; CRC mismatch; a key with the reserved bit set. Expected: Go error strings (for texts §7 G17 decides to reproduce).
5. **Reference records**: §3.5; one with a signature and public key; `created_at` negative; user empty. Expected: hex. Also a Decode rejection of trailing bytes (§2.8 text).
6. **Empty tree** (§3.4) and an xattr map (§3.6), including one set whose encoding is exactly 256 bytes (inline) and one of 257 bytes (spilled, with the XattrSet key).
7. **Ingest roots**: build a fixture under the generator's temp dir. Files 0 B, 1 B, 700 KiB splitmix; a nested dir with `.amberignore` (negation, dir-only); a symlink; a fifo; `.dstore/` holding junk at the root and a `sub/.dstore/` file deeper down. Ingest with `Opts{Exclude: [".dstore"]}` and without it. Emit root keys and `WriteStats.Stored`. Commit a tar of the fixture (PAX, ns mtimes, modes) so Rust materialises the same tree. uid/gid depend on the running user, so either record them or generate on the test machine.
8. **hashFile**: content keys of single-file ingest for 0 B, 1 B, 64 KiB + 1 and 700 KiB payloads.
9. **ReachableKeys / CheckComplete order**: over the fixture tree, emit both key lists in order (root first, BFS by frontier position). Rust must match exactly. Go documents only root-first, but both implementations happen to be deterministic.
10. **fstree error texts**: `ResolvePath(root, "missing/x")`, `ResolvePath(root, "file/x")`, `ResolvePath(root, "a/../b")`, `CollectEntries(blobKey)`, `WriteContent(dirKey)`, `LookupEntry` with names `a\x7f\xffb"c`, `tab\tname`, `é`. Expected: Go strings (checks G4).
11. **encoding/hex**: the cases in §3.7 plus `""`, `"0G"`, `"\x00"`.
12. **Errno texts**: `ingest.Dir` on a missing path, a path w/o permission (EACCES), and a second `packstore.Open` of the same dir. Expected: Go strings (checks G3).
13. **Pebble coexistence**: none; the Rust test is behavioural (§7 G7).

---

## 6. Go tests worth porting (core-facing parts)

dstore:

- `wire/wire_test.go`: `TestPackFramesInterop` (key.New + EncodeRecord + SendPackRecords → NewPackReader + amberpack.NewReader). Port it against the local async `send_pack_records` / `PackRecords`.
- `client/batch_test.go`: `TestBatchesBalancesBySizerNotByKeyLength`, `TestBatchesCapsKeysPerBatch`, `TestBatchesSendsAnOversizedRecordAlone`, `TestBatchesKeepsOrder` (the sizer contract behind storedSizer).
- `worktree/scan_test.go`: `TestScan_CleanTreeHasNoChanges`, `TestScan_EveryKind`, `TestScan_IgnoredBasePathIsDeleted`, `TestScan_RacyMtime`, `TestScan_TypeChangeExpands`, `TestScan_Xattr`. These exercise ingest.Dir, amberignore, cborx and EncodeXattrSet parity with the local disk entry code.
- `worktree/change_test.go`: `TestDiffTrees_EveryKind`, `TestDiffTrees_PrunesEqualSubtrees`, `TestCompare`.
- `worktree/apply_test.go`: `TestApply_CloneThenUpdateReproducesTrees`, `TestApply_KeepsNonEmptyDirectoryOnDelete`, `TestApply_RefusesUnsafePaths`, `TestApply_RefusesSymlinkedAncestor`.
- `worktree/diff_test.go`: `TestUnified_TreeToTree`, `TestUnified_TreeToDisk`, `TestStat`.
- `worktree/tree_test.go`: `TestCreateOpenRoundTrip`, `TestFindOutsideWorkingCopy`, `TestRemove`.
- `worktree/merge_test.go`: `TestMerge`.

core v0.0.8 (already ported in core-rs; re-run them as dependency smoke tests only if pinning a different rev):

- `ingest/exclude_test.go`: `TestExclude_SkipsRootNameOnly`, `TestExclude_IgnoresNoIgnore`, `TestScanWith_HonorsExclude`.
- `fstree/checkcomplete_test.go`: `TestCheckComplete_CompleteTree`, `TestCheckComplete_MissingLeaf`, `TestCheckComplete_MissingInteriorNode`.
- `fstree/reachable_test.go`: `TestReachableKeys`, `TestReachableKeys_FileRoot`, `TestReachableKeys_MissingObject`, `TestReachableKeys_WideParallel`.
- `packstore/record_test.go`: `TestWriteParallelRecordsStoredAndReadable`, `TestWriteParallelRecordsDedupAgainstPresent`, `TestWriteParallelRecordVerifyCatchesWrongPayload`, `TestWriteParallelRecordCorruptFails`, `TestWriteParallelRecordKeyMismatchFails`, `TestWriteParallelDataAndRecordFails`.
- `amberpack/pack_test.go`: `TestWriter_AddRecord_RoundTrip`, `TestReader_Records_RoundTrip`, `TestReader_Records_Truncated`, `TestReader_Records_CRCMismatch`, `TestReader_BadMagic`, `TestReader_TruncatedMissingEndMarker`, `TestReader_OversizedPayloadRejected`. Port these against the local async reader.
- `reference/reference_test.go`: `TestGoldenVector`, `TestDecodeRejectsNonCanonicalEncoding` (smoke).

New Rust-only tests:

- A Go/Rust **packstore flock exclusion** test (run a Go `packstore.Open` via `go run` in CI, then Rust open; expect the "already open" error).
- A **refstore coexistence** test (§7 G7).
- An **umask 002 modes** test (G5).

---

## 7. Gaps and recommended fixes

Severity: **H** breaks interop or visible output in common cases; **M** visible
in plausible cases; **L** edge case or perf only.

**G1 (M). Every core-rs API is synchronous.** Go's goroutine inline calls
become blocking calls in tokio.
*Fix, local:* use the `corex` adapters (§4.2), which run through
`spawn_blocking`. For fstree walks over the cluster (ls, cat, clusterGet),
run the whole walk inside `spawn_blocking` with a getter that calls
`tokio::runtime::Handle::block_on(cluster.get_one(k))`. That keeps core-rs
error texts. Rewriting the walks as async is the alternative, but it would
duplicate core-rs error wording.

**G2 (M). There is no async pack reader or writer, and no `protocol` module.**
`amberpack::Reader::records(self)` consumes the reader. With `Reader::new(&mut pr)`
the draining still works, but it needs a sync `Read` (e.g. `tokio_util::io::SyncIoBridge`,
tokio-util 0.7 `io-util` feature, inside spawn_blocking).
*Fix, local:* implement `send_pack_records` and `PackRecords` natively async
(§4.3), reusing `amberpack::parse_record`, `REC_HEADER_SIZE` and `MAX_PAYLOAD`.
That gives exact TData boundaries and lets the implementation reproduce Go's
error texts.

**G3 (H for CLI text parity). io::Error text differs from Go errno text.** For
example `stat /x: No such file or directory (os error 2)` against Go's
`stat /x: no such file or directory`, or
`packstore: /x is already open: Resource temporarily unavailable (os error 35)`
against `…: resource temporarily unavailable`. It reaches stderr for common
mistakes: `dstore store push /missing NAME`, a working copy already open,
permission errors. core-rs embeds io::Error `Display` inside its own messages
(`ingest::Error::Io`, `packstore::Error::Io/Other`, amberignore's bare
`io::Error`).
*Fix, local:* `goerr::rewrite_os_errors` (§4.3) at the single top-level print,
plus local `op path: ` wrapping where core-rs returns bare io errors
(amberignore root, `packstore::Error::Io` from `File::open(dir)`). *Optional
core-rs change, later:* a `go_display()` helper. Not required.

**G4 (L). `%q` rendering differs.** core-rs quotes names with Rust `{:?}` over
`String::from_utf8_lossy`. Go `%q` gives `\x7f` / `\xff` where Rust gives
`\u{7f}` / U+FFFD. It only shows for names containing control characters or
invalid UTF-8, in fstree `NotFound`/`NotDir`/`ContentKey`/`OutOfOrder`/
`EntryContentKey`/`EntryXattrsKey`, and in cbor `XattrValue`.
*Fix, local:* the variants expose `name: Vec<u8>`, so `goerr::render_walk_error`
and `render_cbor_error` can re-render them with `go_quote`. Implement `go_quote`
with `strconv.Quote` semantics:
- `\a \b \f \n \r \t \v \\ \"` for those characters;
- `\xNN` for invalid UTF-8 bytes and for runes < 0x20 or 0x7f;
- `\uNNNN` / `\UNNNNNNNN` for other non-printable runes;
- printable runes as-is.

**G5 (M). File and directory modes under a non-022 umask.** Go creates store
directories with `0755` and segment files with `0644`, then applies the umask
(`packstore.go:157,299`). Rust uses `fs::create_dir_all` (0777) and
`OpenOptions` (0666) (`packstore/mod.rs:366,498`). Under umask 002, Go gives
0755/0644 **[verified]** and Rust gives 0775/0664. redb's `refs.redb` is 0666
minus umask; Pebble's files are not comparable anyway.
*Fix, local, partial:* pre-create `<wc>/.dstore`, `.dstore/packstore`,
`<local>/packstore` and `<local>/refs` with `DirBuilder::new().recursive(true).mode(0o755)`.
Go's `worktree.Create` does `MkdirAll(meta, 0o755)`, so the mode is the same.
*Needs a core-rs change for segment files:* `.mode(0o644)` via
`std::os::unix::fs::OpenOptionsExt` in `create_active` (`mod.rs:498`) and
`DirBuilderExt::mode(0o755)` in `open_with`. It is a one-line, low-risk patch.
Otherwise document the divergence (D2).

**G6 (L). Go's `packstore.Open` returns the raw `os.Open(dir)` error, which
carries an `open <dir>:` prefix; core-rs does not.**
*Fix, local:* before calling `open_with`, open the directory once with
`std::fs::File::open` and map a failure to `open <dir>: <go errno>`.

**G7 (H for mixed Go/Rust local stores). refstore uses redb instead of Pebble.**

What happens with `dstore store push/pull --local DIR`:

1. `openLocal` (`cmd/dstore/client.go:209-221`) opens `DIR/packstore` (interoperable, flock-excluded) and `DIR/refs`.
2. Go opens `DIR/refs` as a **Pebble** DB. Rust opens `DIR/refs/refs.redb` (`refstore.rs:21,118`).
3. **[verified]** Pebble v2.1.7 opens a directory holding a foreign `refs.redb`, ignores that file and does not delete it. After Put, Close and reopen, the dir holds `000005.sst 000007.log LOCK MANIFEST-000001 MANIFEST-000006 OPTIONS-000008 marker.format-version.000001.013 marker.manifest.000002.MANIFEST-000006 refs.redb`, and `refs.redb` is byte-unchanged.
4. On a directory that contains only `refs.redb`, Pebble silently creates a new empty DB. redb's `Database::create(dir/refs.redb)` ignores the Pebble files.
5. So **neither side errors. One directory ends up holding two independent reference databases.**
6. There is no cross-implementation lock. Pebble locks `DIR/refs/LOCK` with `fcntl(F_SETLK)` (pebble `vfs/file_lock_unix.go:42-63`); redb flocks `refs.redb` (redb-2.6.3 `src/tree_store/page_store/file_backend/unix.rs`). A Go and a Rust process can write "refs" at the same time.

Consequences:

- A name that Rust `store pull --local DIR NAME` records only lands in `refs.redb`. Go `amber` / Go `dstore` tools reading `DIR/refs` do not see it: `refstore: reference not found`, and `ref:NAME` lookups fail.
- Objects are in the shared packstore, so the tree itself is readable by key.
- Rust `store push --local` writes the ref after the push. Go ignores that error too (`client.go:294-296`), and the ref again lands only in redb.
- Go-written refs are invisible to Rust tools (core-rs example CLI) using `DIR`.
- dstore itself never reads local refs, so dstore's own output is unaffected.
- Working copies (`.dstore/`) have no refs directory and are fully interoperable.

Practical mitigations:

- (a) Document that a local store belongs to one implementation.
- (b) Detect a Pebble DB before opening: any of `LOCK`, `CURRENT`, `MANIFEST-*`, `OPTIONS-*`, `marker.manifest.*`, `marker.format-version.*`, `*.log`, `*.sst` in `DIR/refs`. Then refuse, or skip the ref write with a warning. For `store pull`, where Go returns the Put error, refusing is closest in spirit. For `store push`, where Go ignores the Put error, silently skipping is identical in outcome to a failed Put.
- (c) Write-only Pebble WAL injection: dstore only needs `Put`, so appending a batch as a new `NNNNNN.log` would be enough. That means parsing the MANIFEST for the last sequence number, the LevelDB-style log chunk format with CRC, and Pebble's batch repr. It is feasible (~300-500 lines) but fragile across Pebble format versions. Not recommended for v1.
- (d) A Pebble reader or writer crate: none exists.

Recommendation: (a) + (b), with the exact policy decided in D1. No core-rs change.

**G8 (L). zstd frames are not byte-identical.** Does anything client-side depend
on identical compressed bytes? **No.**

- Keys hash uncompressed bytes.
- Record CRCs are self-consistent per record. Go nodes accept Rust-built records verbatim: `node/data.go:399-404` stores `packstore.Object{Key, Record}` via WriteParallel, and scrub re-verifies CRC plus payload hash (`node/reconcile.go:630`).
- TPut frames carry no per-key bytes; replies are keyed.
- Pull writes fetched records verbatim, so a pulled local store holds the node's (Go) frames, re-pushes send those bytes, and `pulled …(%d bytes)` counts the same bytes a Go client would.

What does differ, and is not part of stdout:

- `storedSizer` (46 + slen), which moves put-batch boundaries.
- Progress byte totals, and the `negotiated … bytes=` / `uploaded … bytes=` log attributes.
- `PushStats.Bytes`, which is not printed.
- Rust frames also lack the 4-byte content checksum klauspost adds (§3.2).

*Fix:* none needed. Keep golden vectors to raw records or decoded payloads.

**G9 (L). Crate-private helpers the client needs:** xattr read, xattr set,
major/minor/mkdev, mtime ns.
*Fix, local:* ~80 lines (§4.3). A future core-rs `pub` export would remove the
duplication; it is not required.

**G10 (L). `Key::type_()` panics on non-canonical keys** (`key.rs:180-186`).
Go returns `Type(n)`. All client call sites apply `type_()` only to validated
keys:
- `VerifyRecord` validates first;
- child keys come from `child_keys` (parsed);
- roots come from `Key::parse` or ingest.

Unvalidated `[32]byte` from the wire only reach `length()` (estSize,
storedSizer) and placement, neither of which panics.
*Fix:* keep this invariant, and never call `type_()` on raw wire bytes before
`validate()`.

**G11 (M, CLI). `cat NAME /` (or `.`): Go crashes.** `ResolveEntry` returns
`(nil, nil)` **[verified]**, then `len(e.ContentKey)` (`cmd/dstore/client.go:542`)
dereferences nil. The process panics (runtime error, exit status 2, goroutine
dump on stderr). core-rs returns `Ok(None)`.
*Fix, local, per D4:* either reproduce "exit 2 with a panic-like message" or
treat it as a Go bug and print `dstore: not a regular file with content` with
exit 1. Byte-exact panic output is not practical.

**G12 (L, perf). Threads per BFS level.** `check_complete` and
`reachable_keys` spawn scoped OS threads for every level
(`read.rs:796-830`). PullTree's `want` calls `CheckComplete` for every held
interior key (`client/tree.go:303`), which is O(subtree) per call, the same
algorithm as Go but with thread-spawn overhead.
*Fix, local:* inside `want`, only the success bit matters, so a local
sequential completeness check reusing `fstree::child_keys` is fine. Keep core's
`check_complete` for the final check, whose error text is printed.

**G13 (L). Negative `--jobs` values.** Go treats `Jobs < 1` (ingest) and
`Writers <= 0` as GOMAXPROCS. core-rs takes `usize`.
*Fix, local:* the CLI parses `--jobs` as i64 (urfave `IntFlag`) and maps
`<= 0` to 0.

**G14 (L). Shape of `ingest::dir`:** `(WriteStats, Result<Key>)` instead of a
triple. No fix needed.

**G15 (L). xattr set order:** Go map order against BTreeMap order. No fix
needed.

**G16 (M). No hex decoder in core-rs.** `worktree/tree.go:190,216,227` uses
`encoding/hex` and its error texts end up in `bad state file: base: …`.
*Fix, local:* `gohex::decode_string` with Go's order. Walk the pairs and report
an invalid byte first; then, for an odd length, check the last character, and
report an invalid byte if it is one, otherwise `ErrLength`. Format invalid
bytes with `%#U` (`U+007A 'z'`; non-printable as `U+0000`). See §3.7
[verified].

**G17 (L). amberpack Reader truncation texts:** Go `unexpected EOF`, Rust
`failed to fill whole buffer`. Not observable, because getStream errors are
discarded (`fetch.go:252`).
*Fix:* the local async reader (G2) reproduces Go's texts.

**G18 (L). packstore `Object` with empty data plus a record:** accepted in
Rust, rejected in Go only for a non-nil empty slice. The client never builds
that. No fix needed.

**G19 (L). Edge-case message of `decode_payload` for over-long frames.** Not
observable. No fix needed.

**G20 (L). refstore open error prefix:** `refstore: opening redb: …` against
`refstore: opening pebble: …`.
*Fix, local, if D1 keeps redb:* re-render as `refstore: opening pebble: <cause>`
only if byte parity is wanted. Arguably misleading; decide with D1.

**G21. MSRV.** core-rs effectively needs 1.88 (let-chains), iroh 1.91.
*Fix:* set `rust-version = "1.91"` and pin nixpkgs rustc (1.95).

**G22. ScanWith is ported but unused.** Nothing to do.

**Rust iroh gaps for this area:** none. Stream I/O is tokio-compatible (§4.4).

---

## 8. Risks and open decisions

### Decisions

- **D1: local refs policy for `store push/pull --local`** (G7). Choose one:
  - (i) plain redb (split-brain when Go also uses DIR; document it);
  - (ii) detect Pebble and refuse on pull / skip on push;
  - (iii) a Pebble WAL appender.

  Recommended: (ii) + documentation.
- **D2: file modes** (G5). Patch core-rs (`create_active` 0644, `open_with` 0755) or accept a divergence that only appears under a non-022 umask. Pre-creating the directories locally is recommended either way.
- **D3: scope of Go-compatible error rendering** (G3/G4/G16/G17/G20): a rewrite layer at the top-level print (recommended) or per-call-site re-rendering.
- **D4: `cat NAME /` Go panic** (G11): reproduce exit 2 or fix.
- **D5: dependency pin:** `rev = "a85ffa1…"` (recommended, immutable) or `tag = "v0.3.0"` (same commit today).
- **D6: thread and blocking model:** `spawn_blocking` adapters (recommended) or upstreaming async-friendly APIs to core-rs (not needed).

### Risks

- **R1.** zstd frames differ (G8). Bytes on the wire differ from a Go client for compressible objects built locally. By contract they are interoperable, not identical, and golden tests must avoid compressed records.
- **R2.** Silent refs split-brain (G7) if D1 picks (i).
- **R3.** Error-text parity depends on the rewrite layer covering every io error that reaches stderr. Error paths inside core-rs may differ in structure, not only in errno text; e.g. Go `readdirent` vs `open` appears for directory read errors. Test with the §5 vector 12.
- **R4.** Build: zstd-sys compiles C (the nix devshell needs cc), and edition 2024 / MSRV 1.91.
- **R5.** core-rs parity gaps upstream: it ports core v0.0.8 except Go PR #8's gc write-span gate (`Collector.BeginWrite`) and `inbox.WithGate` (`PORTING.md` intro). Neither is used client-side.
- **R6.** Performance: thread churn in the fstree walks (G12) and `write_parallel` spawning a scoped thread pool per 16 MiB pull batch. It is correct but slower than Go on very large pulls. Benchmark it.
- **R7.** Node-side commands (`serve`, `cluster init`, `node join`, `catalog restore`, ticket from `--store`) need Pebble meta and paxos (§1.6). core-rs cannot help, so these stay out of scope unless a Pebble reader is written.

---

## Addenda (synthesis)

Added by the architecture synthesis. `PORTING.md` is normative where it differs from this spec.

1. **No `corex` crate.** §4.2's adapters are distributed:
   - `verify_record`, `stored_size_of` and `est_size` → `dstore-client`;
   - the pack helpers → `dstore_wire::pack`;
   - `record_payload`, `open_local` and Pebble detection → `dstore_cli::common`;
   - xattr, meta and `mkdev` → `dstore_worktree::sys`;
   - `goerr` (errno table, `rewrite_os_errors`, `go_quote`) → `dstore_gocompat::{errno, quote}`;
   - `gohex` → `dstore_gocompat::hex`;
   - `render_walk_error`/`render_cbor_error` → `dstore_client::corefmt::{walk_error_text, cbor_error_text}`.
2. **D1 resolved.** Refuse a Pebble `DIR/refs` at open, for both `store push` and `store pull`: Go
   returns `refstore.Open` failures for both, so refusing at open keeps the structure. The text is in
   PORTING §2.3. This overrides "skip on push".
3. **D2.** Pre-create store directories with 0755. The segment-file mode divergence under a non-022
   umask is documented as DD-9; propose the one-line core-rs patch upstream without blocking on it.
4. **D3-D6.**
   - D3: a top-level errno rewrite plus local wrapping where core-rs drops op/path prefixes.
   - D4: `cat NAME /` emulates Go's panic (first line, exit 2; PORTING DD-7).
   - D5: pin by rev.
   - D6: `spawn_blocking` adapters, as PORTING §5.1 lists.
5. **G20.** Keep core-rs `refstore: opening redb: …` texts; do not fake the Pebble prefix.
6. **G13.** The CLI parses `--jobs` as `i64` and maps values ≤ 0 to 0 (= available parallelism).
