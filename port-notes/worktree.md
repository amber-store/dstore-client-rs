# Working copies (`worktree` + the working-copy CLI) — porting spec

Normative reference: `github.com/amber-store/dstore` v0.1.9 (HEAD 368f2c7). The
uncommitted change to `cmd/dstore/wc.go` only reflows the `wcFlags` literals
(same names and usage strings) and is ignored. Line numbers below are for the
committed file set; `wc.go` line numbers are from the working-tree file, which
differ from HEAD only by +12 lines inside `wcFlags` (lines 27-46).

Everything written here was read from the Go sources. Three facts were confirmed
by running Go 1.26.5 in a scratch directory, since deleted: the JSON bytes, the
RFC3339Nano strings, and the diff sort sensitivity (§3.9, §5).

---

## 1. Scope

### 1.1 Go files covered

| file | lines | what it does |
|---|---|---|
| `worktree/tree.go` | 245 | `.dstore/` layout, `Config`/`State` JSON, `Find`, `Open`, `openRaw`, `Create`, `Remove`, `Close`, `Get`, `SaveConfig`, `SaveState`, `loadState`, `writeJSON`, `EmptyTree`, `ErrNotWorkingCopy`, `ErrIncomplete` |
| `worktree/change.go` | 238 | `Kind` (+`String`), `Change`, `IsDir`, `TypeName`, `SameContent`, `Equivalent`, `Compare`, `DiffTrees` (tree against tree) |
| `worktree/scan.go` | 284 | `RacyWindow`, `Scan` (working directory against a tree), `hashFile` |
| `worktree/merge.go` | 80 | `Conflict`, `Merge` (three-way per-path merge) |
| `worktree/apply.go` | 310 | `Apply` (writes a change list to disk), path safety, `applyMeta`, xattr decode |
| `worktree/diff.go` | 206 | `Source`, `TreeSource`, `DiskSource`, `Unified`, `Stat`, `MaxDiffBytes`, `ErrTooLarge`, binary heuristic |
| `worktree/flow.go` | 331 | `Fetch`, `Clone`, `Init`, `Pull`, `Push`, `Status`, `TicketFromView`, `RefreshTicket`, flow errors |
| `worktree/xattr.go` | 55 | `readXattrsWith`, `ignoreUnsupported` |
| `worktree/xattr_darwin.go` | 15 | `readXattrs` = `unix.Listxattr`/`unix.Getxattr`; `setXattr` = `unix.Setxattr(path, name, value, 0)` |
| `worktree/xattr_linux.go` | 15 | `readXattrs` = `unix.Llistxattr`/`unix.Lgetxattr`; `setXattr` = `unix.Lsetxattr(path, name, value, 0)` |
| `worktree/*_test.go` | 179+171+103+91+156+88 | unit tests (§6) |
| `node/worktree_test.go` | 240 | end-to-end over the in-memory 3-node cluster (§6) |
| `cmd/dstore/wc.go` | 501 | CLI: `clone`, `init`, `fetch`, `pull`, `push`, `status`, `diff`; ticket precedence; `withCluster`; `describeChange`; `filterPaths` |
| `cmd/dstore/wc_test.go` | 20 | `TestResolveTicket` |
| `cmd/dstore/main.go` (parts) | 698 | app registration and error printing (35-51), `signalCtx` (258-260), `localTicket` → `worktree.TicketFromView` (394-405), `relayModeOf` (123-137) |
| `cmd/dstore/client.go` (parts) | 574 | `netOpts` (46-50), `dialTicket` (77-96) |
| `cmd/dstore/tui.go` (parts) | 374 | `runTransfer` (27-32), `noTUIFlag` (34-36); the TUI itself belongs to the CLI/TUI spec |
| go-udiff v0.4.1 `unified.go` | 314 | `Unified`, `toUnified`, `splitLines`, `addEqualLines`, `unified.String` |
| go-udiff v0.4.1 `diff.go` | 177 | `Edit`, `validate`, `SortEdits`, `lineEdits`, `expandEdit` |
| go-udiff v0.4.1 `ndiff.go` | 118 | only `Lines` (16-31) is used |
| go-udiff v0.4.1 `lcs/old.go` | 475 | Myers LCS: `DiffLines`, `compute`, `forward`, `twosided`, `twoDone`, `twolcs`, `forwardlcs`, `backwardlcs` |
| go-udiff v0.4.1 `lcs/common.go` | 179 | `lcs.sort`, `lcs.fix`, `overlap`, `prepend`, `append`, `ok` |
| go-udiff v0.4.1 `lcs/labels.go` | 55 | `label` triangular storage |
| go-udiff v0.4.1 `lcs/sequence.go` | 70 | `linesSeqs`, `commonPrefixLen`, `commonSuffixLen` |
| Go 1.26.5 `sort/zsortfunc.go` | 479 | `pdqsort_func` and helpers, used through `sort.Slice` in `lcs.fix`/`lcs.sort`; **must be ported exactly** (§3.9) |
| Go 1.26.5 `sort/sort.go` (59-80) | — | `sortedHint`, `xorshift`, `nextPowerOfTwo` |

### 1.2 APIs used from other packages

- core v0.0.8: `ingest.Objects`, `ingest.Dir`, `ingest.Opts{Jobs, Exclude}`,
  `ingest.DefaultXattrInlineMax` (=256); `fstree.Entry`, `fstree.CollectEntries`,
  `fstree.EncodeDirLeaf`, `fstree.EncodeXattrSet`, `fstree.WriteContent`
  (and `fstree.EncodeBlob` in tests); `amberignore.Root`, `(*Matcher).Descend`,
  `(*Matcher).Ignored`; `cborx.EncodeXattrs`, `cborx.DecodeXattrs`; `key.Key`,
  `key.Parse`, `Key.Length`, `Key.String`; `packstore.Open`,
  `packstore.WithSync`, `Store.Put`, `Store.Get`, `Store.Close`,
  `packstore.WriteStats`; `reference.ValidateName`, `reference.ValidateUser`.
- dstore: `client.Cluster.RefGet`, `.PullTree`, `.Push`, `.View`, `.Close`;
  `client.ErrUnknownRef`, `*client.CASMismatch`, `client.Cond`,
  `client.PullStats` (`Fetched`, `Bytes`), `client.PushStats` (`Keys`,
  `Uploaded`, `Version`), `client.Progress`; `ticket.Ticket`, `ticket.Member`,
  `Ticket.Encode`, `ticket.Parse`; `view.View` (`ClusterID`, `Incarnation`,
  `Nodes[i].ID`, `Nodes[i].Addrs`). Their byte formats belong to the
  client, ticket and view specs; only the calls are specified here.
- Go stdlib behaviour that is observable (§2.12): `encoding/json` v1
  (`Marshal`/`MarshalIndent`/`Unmarshal`), `time.RFC3339Nano`, `os.Getwd`,
  `os.Remove`, `os.MkdirAll`, `os.RemoveAll`, `os.CreateTemp`, `os.ReadDir`,
  `path.Base`, `filepath.Abs`/`Rel`, `strconv.ParseBool`, `strconv.Quote`
  (`%q`), `os/user.Current`, `sort.Slice`.

---

## 2. API used client-side, with exact semantics

### 2.1 Constants and sentinel errors (verbatim)

`tree.go`:
```go
const Dir = ".dstore"                        // 22
configFile = "config"; stateFile = "state"; storeDir = "packstore"   // 24-28
ErrNotWorkingCopy = errors.New("not a dstore working copy (no .dstore in this or any parent directory)")  // 34
ErrIncomplete     = errors.New("incomplete clone: delete the directory and clone again")                  // 35
```
`flow.go` 19-25:
```go
ErrNoRemote      = errors.New("the reference does not exist on the cluster: nothing to pull")
ErrRemoteMoved   = errors.New("the cluster's tree moved since your last sync: pull first, or --force")
ErrRemoteDeleted = errors.New("the reference was deleted on the cluster: --force to recreate it")
ErrConflict      = errors.New("conflicting changes: resolve them, or --force to take the cluster's side")
ErrRefChanged    = errors.New("reference changed on the cluster since your last fetch: pull first, or --force")
```
`scan.go` 20: `RacyWindow = 2 * time.Second`.
`diff.go` 19-21: `MaxDiffBytes = 16 << 20` (16777216); `ErrTooLarge = errors.New("too large to diff")`.
`apply.go` 186: `errNotDir = errors.New("not a directory")` (internal).
`ingest.DefaultXattrInlineMax = 256` (core `ingest/ingest.go` 30).

File-type constants (same values on Linux and Darwin): `S_IFMT 0o170000`,
`S_IFSOCK 0o140000`, `S_IFLNK 0o120000`, `S_IFREG 0o100000`,
`S_IFBLK 0o060000`, `S_IFDIR 0o040000`, `S_IFCHR 0o020000`, `S_IFIFO 0o010000`.

### 2.2 Types

```go
type Getter = func(key.Key) ([]byte, error)                     // tree.go 31
type Config struct {                                             // tree.go 39-46
    Ticket      string `json:"ticket"`
    Name        string `json:"name"`
    Relay       string `json:"relay,omitempty"`
    NoRelay     bool   `json:"no_relay,omitempty"`
    NoDiscovery bool   `json:"no_discovery,omitempty"`
    User        string `json:"user,omitempty"`
}
type State struct {                                              // tree.go 51-57
    Base, Remote  key.Key
    HasRemote     bool     // false: the reference does not exist on the cluster
    RemoteVersion []byte   // CAS token (the ballot ref-get returned)
    SyncedAt      time.Time
}
type Tree struct { Root string; Config Config; State State; Store *packstore.Store }  // 68-73
type Kind int  // Added=0, Deleted=1, Modified=2, TypeChanged=3, ModeChanged=4, MetaChanged=5 (change.go 16-23)
type Change struct { Path string; Kind Kind; Old, New *fstree.Entry }  // 45-49; Old nil for Added, New nil for Deleted
type Conflict struct { Path string; Local, Incoming Change }            // merge.go 9-12
type FetchResult struct { Exists, UpToDate bool; Key key.Key; Stats client.PullStats }  // flow.go 28-33
type PullResult struct { Fetch FetchResult; UpToDate bool; Applied []Change; Conflicts []Conflict }  // 154-159
type PushResult struct { Root key.Key; Nothing, Recovered bool; Built packstore.WriteStats; Stats client.PushStats }  // 205-211
type RemoteState int // RemoteUpToDate=0, RemoteMoved=1, RemoteAbsent=2 (262-268)
type Status struct { Changes []Change; MetaOnly int; Remote RemoteState; Incoming []Change }  // 271-276
type Source interface { Content(p string, e *fstree.Entry) ([]byte, error) }  // diff.go 25-27
type TreeSource struct{ Get Getter }   // 30
type DiskSource struct{ Root string }  // 54
```

`Kind.String()` (change.go 25-41): `Added`→`"new"`, `Deleted`→`"deleted"`,
`Modified`→`"modified"`, `TypeChanged`→`"type"`, `ModeChanged`→`"mode"`,
`MetaChanged`→`"meta"`, otherwise `fmt.Sprintf("Kind(%d)", int(k))`.

`Change.Path` is root-relative, `/`-separated and built by concatenating raw
entry names with `/` (`joinPath`, change.go 114-119: `prefix == ""` → `name`,
else `prefix + "/" + name`). Names are arbitrary bytes, so **the Rust path type
must be bytes (`Vec<u8>`), printed raw**, not `String`.

### 2.3 The working copy on disk (`tree.go`)

**`EmptyTree()`** (76-82): `fstree.EncodeDirLeaf(nil)` gives the bytes `80` (an
empty CBOR array) and the key
`2001bbe6a9f5a0146a1f4d0381e9b0ed1ac2f1a979ce9d5ad84e46ff0b58f36b` (computed with
core v0.0.8). Its first 16 hex digits are `2001bbe6a9f5a014`, which `status`
prints after `init`.

**`Find(dir)`** (86-101): `abs := filepath.Abs(dir)` (joins with `os.Getwd`
when relative, then `Clean`). Loop: if `os.Stat(abs/.dstore)` succeeds **and**
is a directory (Stat follows symlinks, so a symlink to a directory counts),
return `abs`. Otherwise `parent := filepath.Dir(abs)`; if `parent == abs`,
return `ErrNotWorkingCopy`; else continue from `parent`. Any Stat error,
including permission errors, is treated as "not here" and the walk moves up.

**`Open(dir)`** (104-116): `openRaw(dir)`, then `loadState(t.Root)`; on a state
error close the store and return the error. Because of this order the lock is
taken before the state is checked, so a locked copy reports the lock error, not
`ErrIncomplete`.

**`openRaw(dir)`** (120-139), in order:
1. `root, err := Find(dir)` → return err.
2. `os.ReadFile(root/.dstore/config)`; error → `fmt.Errorf("working copy %s: %w", root, err)`, e.g. `working copy /abs: open /abs/.dstore/config: no such file or directory`.
3. `json.Unmarshal(b, &cfg)`; error → `fmt.Errorf("working copy %s: bad config: %w", root, err)`.
4. `packstore.Open(root/.dstore/packstore, packstore.WithSync(true))` → return err raw. This creates the directory with `MkdirAll(0o755)` when missing and takes the lock (§3.4).
5. Return `Tree{Root: root, Config: cfg, State: State{Base: EmptyTree key}, Store: store}`.

**`Create(dir, cfg)`** (144-169), in order:
1. `abs := filepath.Abs(dir)`.
2. If `Find(abs)` succeeds → `fmt.Errorf("%s is inside the working copy at %s", abs, root)`. This includes `abs` itself holding `.dstore`, which gives e.g. `/x is inside the working copy at /x`.
3. `os.MkdirAll(abs/.dstore, 0o755)` → err raw.
4. `writeJSON(abs/.dstore/config, cfg)` → err raw.
5. `packstore.Open(abs/.dstore/packstore, WithSync(true))` → err raw. The config is left behind; callers clean up.
6. `store.Put(emptyKey, emptyBytes)`; error → `store.Close()`, return err.
7. Return `Tree{Root: abs, Config: cfg, State: State{Base: empty, SyncedAt: time.Now()}, Store: store}`. **No state file is written.**

**`Remove(dir)`** (172-174): `os.RemoveAll(filepath.Join(dir, ".dstore"))`.
`Close()` returns `Store.Close()`. `Get(k)` returns `Store.Get(k)`.

**`SaveConfig()`** (181-183): `writeJSON(Root/.dstore/config, t.Config)`.

**`SaveState()`** (185-193): `stateJSON{Base: hex(Base), SyncedAt: SyncedAt.UTC().Format(time.RFC3339Nano)}`;
if `HasRemote`, also `Remote: hex(Remote)` and `RemoteVersion: hex(RemoteVersion)`.
Otherwise both stay `""`. Then `writeJSON(Root/.dstore/state, j)`.

**`loadState(root)`** (195-224), with verbatim error strings:
1. `os.ReadFile(root/.dstore/state)`: `ErrNotExist` → `ErrIncomplete`; any other error → raw.
2. `json.Unmarshal` error → `fmt.Errorf("bad state file: %w", err)`.
3. `Base = parseKey(j.Base)`; error → `"bad state file: base: %w"`. `parseKey` is `hex.DecodeString` (which accepts upper- or lowercase hex), then `key.Parse`. An empty base gives `bad state file: base: key: data is not 32 bytes: got 0`.
4. If `j.Remote != ""`: `HasRemote = true`; `Remote = parseKey(j.Remote)`, error → `"bad state file: remote: %w"`; `RemoteVersion = hex.DecodeString(j.RemoteVersion)`, error → `"bad state file: remote_version: %w"`. An empty `remote_version` decodes to an empty, non-nil slice. When `remote` is empty, `remote_version` is ignored and `RemoteVersion` stays nil.
5. `SyncedAt = time.Parse(time.RFC3339Nano, j.SyncedAt)`; error → `"bad state file: synced_at: %w"`. Go's text is e.g. `parsing time "" as "2006-01-02T15:04:05.999999999Z07:00": cannot parse "" as "2006"`.

**`writeJSON(path, v)`** (235-245): `b := json.MarshalIndent(v, "", "  ")`;
`tmp := path + ".tmp"`; `os.WriteFile(tmp, b + "\n", 0o644)`, which opens with
`O_WRONLY|O_CREATE|O_TRUNC` (0644 & ~umask when created; an existing tmp keeps
its permissions); then `os.Rename(tmp, path)`. There is **no fsync** of the file
or the directory, and errors are returned raw.

### 2.4 Change classification and tree diff (`change.go`)

`IsDir(e)` (52) returns `e != nil && e.Mode&S_IFMT == S_IFDIR`.

`TypeName(mode)` (55-73) maps the type bits: `S_IFREG` `"file"`, `S_IFDIR`
`"directory"`, `S_IFLNK` `"symlink"`, `S_IFIFO` `"fifo"`, `S_IFSOCK` `"socket"`,
`S_IFCHR` `"char device"`, `S_IFBLK` `"block device"`, otherwise
`fmt.Sprintf("type %#o", mode&S_IFMT)`. Go's `%#o` prints `0` for zero and
`0160000` for 0o160000, so the Rust form is `if v == 0 { "0" } else { "0{:o}" }`.

`SameContent(a, b)` (79-89) switches on `a`'s type. `S_IFREG` compares
`bytes.Equal(ContentKey)`, `S_IFLNK` compares `bytes.Equal(LinkTarget)`,
`S_IFCHR`/`S_IFBLK` compares `slices.Equal(Rdev)`, and every other type
(dir, fifo, socket) returns `true`. `bytes.Equal` treats nil and empty as equal.

`Equivalent(a, b)` (93-95) is: same `S_IFMT` && `SameContent` && same `Mode&0o7777`.

`Compare(old, new)` (99-112) is evaluated in this order:
1. types differ → `TypeChanged`
2. `!SameContent` → `Modified`
3. perms `&0o7777` differ → `ModeChanged`
4. UID, GID, Mtime, `XattrsIn` bytes or `XattrsKey` bytes differ → `MetaChanged`
5. otherwise `(0, false)`.

A content change that also changes the mode is still reported as `Modified`.

**`DiffTrees(get, a, b)`** (126-135) returns `nil, nil` when `a == b`; otherwise
it runs `diffDirs(get, "", a, b)`.

**`diffDirs`** (137-202):
- `ea := fstree.CollectEntries(a, get)` and `eb := CollectEntries(b, get)`, errors raw.
- Merge-join over the two lists by `bytes.Compare(name)`:
  - only in `a`: `expand(get, prefix, &ea[i], Deleted)`
  - only in `b`: `expand(get, prefix, &eb[j], Added)`
  - in both, with `p = joinPath(prefix, name)`:
    - if `Compare(x, y)` differs, append `Change{p, k, Old: x, New: y}`. If `k == TypeChanged`, then `expandChildren(get, p, x, Deleted)` followed by `expandChildren(get, p, y, Added)`.
    - then, if `IsDir(x) && IsDir(y) && !bytes.Equal(x.ContentKey, y.ContentKey)`, parse both keys (error raw) and recurse `diffDirs(get, p, kx, ky)`.
- A subtree whose keys are equal is never fetched: `TestDiffTrees_PrunesEqualSubtrees` expects exactly two `get` calls.

`expand(get, prefix, e, kind)` (206-216) appends `Change{Path, Kind: kind}` with
`New = e` for `Added` or `Old = e` for `Deleted`, then `expandChildren`.
`expandChildren(get, p, e, kind)` (220-238) does nothing unless `IsDir(e)`.
Otherwise it parses `e.ContentKey`, collects the entries and calls `expand` for each.

**Order.** Changes come out in walk order: depth-first pre-order, each
directory's entries in bytewise name order, a directory before its contents.
This is **not** global bytewise path order when a name contains a byte below
`/` (for example `a`, `a/x`, `a-b`). The design document says "sorted bytewise";
the code wins, and `status` prints in walk order.

### 2.5 Scan: working directory against a tree (`scan.go`)

**`Scan(root, base, get, syncedAt, jobs)`** (34-45):
`ign := amberignore.Root(root)` (error raw; Go returns the `os.ReadFile`
PathError for `root/.amberignore`, e.g. `open /x/.amberignore: permission denied`),
then `s.dir(root, "", base, ign, &out)`.
`s.root` is the `root` string exactly as passed (the callers pass `t.Root`).

**`listDir(abs, ign)`** (49-65):
- `os.ReadDir(abs)`: sorted bytewise by name; the error is raw, e.g. `open /x/sub: permission denied`.
- Drop the entry named `.dstore` when `abs == s.root` (string equality, any entry type).
- Drop the entries where `ign.Ignored(name, de.IsDir())`. `IsDir` comes from readdir `d_type` (lstat fallback), so a symlink to a directory is not a directory.

**`dir(abs, prefix, dirKey, ign)`** (67-107):
`disk := listDir`; `base := fstree.CollectEntries(dirKey, get)`. Base entries
are **not** filtered by ignore rules, so a base path that is now ignored comes
out `Deleted`, and a `.dstore` entry in the base root comes out `Deleted`.
Merge-join by `bytes.Compare(base name, disk name)`:
- base only: `expand(get, prefix, &base[i], Deleted)`
- disk only: `added(abs, prefix, name, ign)`
- both: `both(abs, prefix, &base[i], ign)`

**`added(abs, prefix, name, ign)`** (111-123): `e := entry(abs/name, name, hash=true)`
(error raw); append `Change{Path: p, Kind: Added, New: e}`; if `IsDir(e)`, call
`addedChildren(full, p, name, ign)`.
`addedChildren` (125-140) runs `sub := ign.Descend(full, name)`, then
`listDir(full, sub)`, then `added(full, p, child, sub)` for each child.

**`both(abs, prefix, b, ign)`** (143-198):
1. If `b.Mode&S_IFMT != diskType(full)`:
   - `diskType` (201-207) is `Lstat(full).Mode & S_IFMT`, or `0` on any Lstat error.
   - `e := entry(full, name, true)`, error raw. A vanished file therefore surfaces as `lstat <full>: no such file or directory`.
   - Append `Change{p, TypeChanged, Old: b, New: e}`, then `expandChildren(get, p, b, Deleted)`.
   - If `IsDir(e)`, call `addedChildren(full, p, name, ign)`, using the parent's matcher. Return.
2. `e := entry(full, name, false)`.
3. For `S_IFREG`: `bk := key.Parse(b.ContentKey)` (error raw). **Racily-clean rule:** if
   `e.size != bk.Length() || e.Mtime != b.Mtime || b.Mtime > syncedAt.Add(-RacyWindow).UnixNano()`,
   hash the file with `hashFile(full, jobs)` and set `e.ContentKey`; otherwise
   `e.ContentKey = b.ContentKey`. In Rust: `b.mtime > synced_at_unix_ns - 2_000_000_000`.
4. For `S_IFDIR`: `e.ContentKey = b.ContentKey`, so a directory's own change can only be mode or metadata.
5. If `Compare(b, e)` differs, append `Change{p, k, Old: b, New: e}`.
6. If `IsDir(b)`: `sub := ign.Descend(full, name)`; parse `b.ContentKey`; `dir(full, p, bk, sub)`. Directories always recurse; nothing is pruned.

**`entry(full, name, hash)`** (218-269) mirrors ingest's `buildEntry`:
- `Lstat(full)` (error raw).
- `Entry{Name, Mode: uint64(st_mode), UID, GID, Mtime: ModTime().UnixNano()}` and `size = st_size`.
- `S_IFREG` with `hash`: `ContentKey = hashFile(full)`.
- `S_IFLNK`: `LinkTarget = os.Readlink(full)` (error raw).
- `S_IFCHR`/`S_IFBLK`: `Rdev = [Major(rdev), Minor(rdev)]`, using the x/sys/unix formulas:
  - Darwin: `major = (dev>>24)&0xff`, `minor = dev&0xffffff`; `st_rdev` is int32 and sign-extended to u64 first.
  - Linux: `major = ((dev&0xfff00)>>8) | ((dev&0xfffff00000000000)>>32)`, `minor = (dev&0xff) | ((dev&0xffffff00000)>>12)`.
- Anything other than a symlink: `xattrs := readXattrs(full)` (error raw). If non-empty, `enc := cborx.EncodeXattrs(xattrs)`:
  - if `len(enc) <= 256`, set `XattrsIn = enc`;
  - else set `XattrsKey = fstree.EncodeXattrSet(xattrs).Key`. Nothing is stored.
- An unsupported type is not an error here (ingest would reject it at push).

**`hashFile(path, jobs)`** (273-284) runs `ingest.Objects(path, Opts{Jobs: jobs})`,
the single-file form. It drains the stream, returns the first error, and
otherwise returns `*root`. Nothing is stored. `Jobs` only sizes the channel.

**`readXattrsWith`** (xattr.go 12-48):
- `sz := list(path, nil)`; an error that is `ENOTSUP` or `EOPNOTSUPP` means none; any other error is returned.
- `sz == 0` → none. Allocate `sz` bytes and list again (same error handling).
- Split on NUL and drop empty names. None left → none.
- For each name: `sz := get(path, name, nil)`, error raw; allocate and get again, error raw; `m[name] = val[:sz]`.
- Darwin uses `listxattr`/`getxattr` with options 0 (follows symlinks); Linux uses `llistxattr`/`lgetxattr`.
- Only non-symlinks are read. Map order does not matter because `EncodeXattrs` sorts.

### 2.6 Merge (`merge.go` 23-80)

Inputs are `local` (base→disk, from `Scan`) and `incoming` (base→remote, from
`DiffTrees`). `byPath` maps a path to its local change. `paths` is a
bytewise-sorted copy of the local paths (`sort.Strings`; paths are unique, so
the sort algorithm does not matter).

- `goneAbove(p)` checks the ancestors of `p` from the nearest up:
  `for i := LastIndexByte(p,'/'); i > 0; i = LastIndexByte(p[:i],'/')`. It
  returns the first `byPath[p[:i]]` whose `Kind == Deleted`, or
  `Kind == TypeChanged && IsDir(l.Old)`. Ancestors with other kinds are skipped
  and the walk continues upward.
- `firstBelow(p)`: `i := sort.SearchStrings(paths, p+"/")` (lower bound). If
  `i < len && HasPrefix(paths[i], p+"/")`, return `byPath[paths[i]]`.

For each `in` in incoming order:
1. If a local change `l` exists at `in.Path`:
   a. `l.Kind == MetaChanged` → apply `in`.
   b. both `Deleted` → nothing.
   c. `l.Kind == Deleted || in.Kind == Deleted` → `Conflict{in.Path, l, in}`.
   d. `Equivalent(l.New, in.New)` → apply `in` (the remote's metadata wins).
   e. otherwise → conflict.
   Then continue with the next incoming change.
2. Else if `goneAbove(in.Path)` finds `l`: conflict if `in.Kind != Deleted`, otherwise nothing. Continue.
3. Else if `in.Kind == TypeChanged && IsDir(in.Old)` and `firstBelow(in.Path)` finds `l` → conflict. Continue.
4. Else apply `in`.

Local-only changes are never in `apply`. `apply` and `conflicts` keep incoming
order. A conflict's `Local` is the change at the path, the ancestor, or the
first local change below.

Consequences worth testing:
- A remote chmod against a local edit conflicts, because `in.New` still has the base content.
- If both sides add the same directory, perms decide; the children merge individually.
- If the remote deletes a directory and the local side added a file below it, the directory's own local change is usually `MetaChanged` (its mtime moved). The deletion is applied and `Apply` keeps the non-empty directory.

### 2.7 Applier (`apply.go` 27-170)

`Apply(root, changes, get)`:
1. `root = filepath.Abs(root)`.
2. `checkPath(c.Path)` for **every** change before anything is written (174-184):
   an empty path gives `errors.New("empty path")`; any `/`-component that is
   `""`, `"."` or `".."` gives `fmt.Errorf("refusing unsafe path %q", p)`. `%q`
   is `strconv.Quote`, which needs a full port (§2.12).
3. Split into `dels` (Kind == Deleted) and `rest`. Sort `dels` by Path
   descending and `rest` ascending (`sort.Slice`, bytewise string compare;
   paths are unique, so ties cannot happen).
4. `target(p)` = `filepath.Join(root, p)` plus `rejectSymlinkComponents(root, t)`
   (191-208). That walks `q := Dir(t)` upward while `HasPrefix(q, root + "/")`;
   `root` itself is not checked:
   - Lstat NotExist → continue.
   - Other Lstat error → return it.
   - A symlink → `fmt.Errorf("refusing to write through non-directory %s", q)` (absolute path).
   - Any other non-directory → `fmt.Errorf("%s: %w", q, errNotDir)`, giving `"/abs/q: not a directory"`.
5. `writable(dir)` (57-71): skip if `dir` is already in the `restore` map; `Lstat(dir)` (error returned);
   `mode = st_mode & 0o7777`; if not a directory or `mode&0o700 == 0o700`, return nil.
   Otherwise record `restore[dir] = mode` **before** `unix.Chmod(dir, mode|0o700)`, whose raw errno is returned.
6. **Deletions**, for each `c` in `dels`:
   - `t, err := target(c.Path)`: `errNotDir` → skip the change; other error → return it raw.
   - `writable(filepath.Dir(t))`: an error other than NotExist → return it raw.
   - `os.Remove(t)`: unlink, then rmdir (§2.12). Ignore NotExist, `ENOTEMPTY` and `EEXIST`; otherwise return `fmt.Errorf("%s: %w", c.Path, err)`, e.g. `sub/x: remove /abs/sub/x: permission denied`.
7. **Additions and modifications**, for each `c` in `rest` (with `e := c.New`):
   - `t, err := target(c.Path)` → return err raw (including errNotDir).
   - `os.MkdirAll(filepath.Dir(t), 0o755)` → raw, e.g. `mkdir /abs/a: not a directory`.
   - `writable(filepath.Dir(t))` → raw.
   - `clearTarget(t, e)` (212-225), error wrapped `"%s: %w"` with c.Path: Lstat NotExist → nil; other Lstat error → returned; different type → `os.RemoveAll(t)`; same type → keep.
   - `contentChange := c.Kind != ModeChanged && c.Kind != MetaChanged`.
   - Then by type:
     - `S_IFDIR`: `os.Mkdir(t, 0o700)`, ignoring `ErrExist`; other error `"%s: %w"`. Append to `dirs`; **no meta now** (`continue`).
     - `S_IFREG`: if contentChange → `writeRegular(t, e, get)`, error `"%s: %w"`.
     - `S_IFLNK`: if contentChange → `os.Remove(t)` (ignore NotExist, else `"%s: %w"`), then `os.Symlink(string(LinkTarget), t)` (`"%s: %w"`).
     - `S_IFIFO`: if contentChange → Remove as above, then `unix.Mkfifo(t, mode&0o7777)`; error `"%s: mkfifo: %w"`.
     - `S_IFCHR`/`S_IFBLK`: if contentChange → Remove, then `unix.Mknod(t, mode&(S_IFMT|0o7777), Mkdev(major, minor))`. `major`/`minor` come from `Rdev` only when `len == 2`, else 0. Error `"%s: mknod: %w"`. `Mkdev`: Darwin `major<<24 | minor`; Linux `((major&0xfff)<<8) | ((major&0xfffff000)<<32) | ((minor&0xff)) | ((minor&0xffffff00)<<12)`.
     - `S_IFSOCK`: `continue` (nothing, not even meta).
     - Other: `fmt.Errorf("%s: unsupported type %#o", c.Path, mode&S_IFMT)`.
   - `applyMeta(t, e, get)`, error `"%s: %w"`.
8. `for dir, mode := range restore { unix.Chmod(dir, mode) }` in random order; the raw error is returned.
   On an earlier error return, `restore` is **not** run.
9. Sort `dirs` by Path descending. For each, compute `t := filepath.Join(root, c.Path)` (no symlink check) and run `applyMeta(t, c.New, get)`, error `"%s: %w"`.

**`writeRegular(t, e, get)`** (229-253):
1. `key.Parse(ContentKey)`, error raw.
2. `os.CreateTemp(Dir(t), ".dstore-tmp-*")`: the name is `.dstore-tmp-` plus the decimal of a random uint32; it is opened `O_RDWR|O_CREATE|O_EXCL`, mode 0600, with up to 10000 tries.
3. `fstree.WriteContent(f, ck, get)`: on error, close, remove the tmp and return.
4. Close; on error remove the tmp and return.
5. `os.Rename(tmp, t)`; on error remove the tmp and return.

There is no fsync. A leftover `.dstore-tmp-*` is an ordinary file that `status`
shows and `push` ingests.

**`applyMeta(t, e, get)`** (258-291):
1. If `os.Geteuid() == 0`: `os.Lchown(t, uid, gid)`, error `"chown: %w"` (PathError `lchown /abs: …`).
2. If not a symlink:
   - `unix.Chmod(t, mode&0o7777)`, error `"chmod: %w"` (raw errno text, no path).
   - `xattrs := entryXattrs(e, get)`, error raw:
     - `len(XattrsIn) > 0` → `cborx.DecodeXattrs`;
     - `len(XattrsKey) == 32` → `key.Parse`, `get`, then `DecodeXattrs`;
     - otherwise none.
   - For each `(name, val)` in random order: `setXattr`. `EPERM`, `EACCES`, `ENOTSUP` and `EOPNOTSUPP` are skipped; any other error → `fmt.Errorf("xattr %q: %w", name, err)`.
   - Xattrs present on disk but absent from the entry are **not removed**.
3. `unix.UtimesNanoAt(AT_FDCWD, t, [ts, ts], flags)`, where `ts = NsecToTimespec(e.Mtime)` sets **both** atime and mtime, and `flags = AT_SYMLINK_NOFOLLOW` for symlinks. Error `"set mtime: %w"`.
   `NsecToTimespec`: `sec = n/1e9; ns = n%1e9; if ns < 0 { ns += 1e9; sec-- }`.

Order matters: chown before chmod (chown clears setuid/setgid), mtime last.
Re-applying the same list is a no-op (`TestApply_CloneThenUpdateReproducesTrees`).

### 2.8 Unified diff and `--stat` (`diff.go`)

**`TreeSource.Content(p, e)`** (32-51):
- `S_IFLNK` → `(e.LinkTarget, nil)`.
- `S_IFREG` → `ck := key.Parse` (error raw); if `ck.Length() > MaxDiffBytes`, return `(nil, ErrTooLarge)`; else `fstree.WriteContent` into a buffer (error raw).
- Any other type → `(nil, nil)`.

**`DiskSource.Content(p, e)`** (56-73) reads `full = Join(Root, p)`:
- `S_IFLNK` → `os.Readlink(full)`, returned as `([]byte(t), err)`, so the slice is empty but non-nil on error.
- `S_IFREG` → `Lstat` (error `(nil, err)`); `Size > MaxDiffBytes` → `ErrTooLarge`; else `os.ReadFile(full)`, which can return partial data together with an error.
- Other types → `(nil, nil)`. The type decision uses the change entry, not the disk.

`sides(c)` (91-99) returns `oldE = c.Old` unless nil or a directory, and likewise `newE`.
`content(s, p, e)` returns `(nil, nil)` when `e == nil`.
`isBinary(b)` (154-159) is true when the first `min(len, 8192)` bytes contain a `0x00`.
`modeLines(c)` (101-105): when `c.Old != nil && c.New != nil` and the perms
differ, write `"old mode %04o\nnew mode %04o\n"`. This applies to Modified and TypeChanged changes too.

**`Unified(w, changes, old, new)`** calls `unifiedOne` for each change and stops at the first error.

`unifiedOne` (107-144):
1. `MetaChanged` → nothing.
2. If both sides are nil (a directory's own change): only when `Kind == ModeChanged`, write `"diff a/%s b/%s\n"` and the mode lines. Return.
3. Write `"diff a/%s b/%s\n"` and the mode lines **before** reading any content.
4. Labels: `/dev/null` for a nil side, else `"a/"+Path` / `"b/"+Path`.
5. `oldB, oldErr := content(old)`, then `newB, newErr := content(new)`.
6. If `errors.Is(oldErr, ErrTooLarge) || errors.Is(newErr, ErrTooLarge) || isBinary(oldB) || isBinary(newB)`: write `"Binary files %s and %s differ\n"` with the labels and return nil. Other errors are ignored in this case.
7. `oldErr` → `fmt.Errorf("%s: %w", Path, oldErr)`; then the same for `newErr`. The header is already written.
8. If `bytes.Equal(oldB, newB)`, stop after the header. Header-only cases: an added or deleted empty file; fifos, sockets and devices; a mode-only change on a file; a type change whose symlink target text equals the file bytes.
9. Write `udiff.Unified(labelA, labelB, string(oldB), string(newB))` (§3.9).

**`Stat(w, changes, old, new)`** (163-206):
1. Skip `MetaChanged` and changes where both sides are nil.
2. Read `oldB` and `newB` as above. On the binary/too-large condition, write `" %s | binary\n"`, `files++`, continue.
3. Content errors are wrapped `"%s: %w"`. Equal bytes → skip, with no line and no count.
4. Count over `strings.SplitAfter(udiff.Unified("a", "b", old, new), "\n")`: a line starting with `+++` or `---` is not counted; otherwise a leading `+` adds one insertion and a leading `-` one deletion.
   This is a bug to keep: a deleted content line starting with `--` (`---x`) and an added line starting with `++` are **not counted**. `\ No newline…` lines are not counted.
5. Write `" %s | +%d -%d\n"`; `files++`, and add to the insertion and deletion totals.
6. Always finish with `" %d files changed, %d insertions(+), %d deletions(-)\n"` (plural even for 1 or 0).

### 2.9 Flows over a connected cluster (`flow.go`)

**`fetch(ctx, cl, prog)`** (37-59) changes state in memory only:
1. `ref, err := cl.RefGet(ctx, Config.Name)`.
   - `errors.Is(err, client.ErrUnknownRef)` → `HasRemote = false`, `Remote = {}`, `RemoteVersion = nil`; return `FetchResult{}`, nil.
   - Other error → return it.
2. `k := key.Parse(ref.Ref.Key)`, error raw. Set `r.Exists = true`, `r.Key = k`.
3. If `HasRemote && Remote == k`, set `r.UpToDate = true`. **There is no PullTree even if the local packstore lost objects.**
   Otherwise `cl.PullTree(ctx, t.Store, k, &r.Stats, prog)`, error raw.
4. `HasRemote = true`, `Remote = k`, `RemoteVersion = ref.Version`.

**`Fetch`** (62-68) is `fetch` followed by `SaveState()`. State is saved only when fetch succeeds.

**`Clone(ctx, cl, dir, cfg, prog)`** (73-130). `dir` is the string the user gave.
1. `os.Stat(dir)`, which follows symlinks:
   - NotExist → `os.MkdirAll(dir, 0o755)` (error raw); `created = true`.
   - Other error → raw.
   - Not a directory → `fmt.Errorf("%s is not a directory", dir)`.
   - Otherwise `os.ReadDir(dir)` (error raw); if non-empty → `fmt.Errorf("%s is not empty", dir)`.
2. `t := Create(dir, cfg)`. On error, `if created { os.RemoveAll(dir) }`, then return the error.
3. `fail(err)` does `t.Close()`, then `created ? os.RemoveAll(dir) : Remove(dir)` (only `.dstore`), and returns the error.
   With a pre-existing empty dir, files already applied **stay behind**.
4. `r := t.fetch(...)`, error → fail. If `!r.Exists` → fail with `fmt.Errorf("%w: %s", client.ErrUnknownRef, cfg.Name)`, giving `client: unknown reference: trees/x`.
5. `changes := DiffTrees(t.Get, State.Base /*empty*/, State.Remote)` → fail on error; `Apply(t.Root, changes, t.Get)` → fail on error.
6. `Base = Remote`, `SyncedAt = time.Now()`; `SaveState()` → fail on error. The state file is written last.

**`Init(ctx, cl, dir, cfg, prog)`** (135-151):
1. `t := Create(dir, cfg)`; on error return `(nil, FetchResult{}, err)`. Nothing is cleaned; Create has not created anything before its checks.
2. `r, err := t.fetch()`. On success, set `SyncedAt = now` and `err = SaveState()`.
3. On any error: `t.Close(); Remove(dir)`; return `(nil, r, err)`. `Base` stays the empty tree.

**`Pull(ctx, cl, force, jobs, prog)`** (164-202) returns `(PullResult, error)`. `r` is filled even on error; the CLI needs `r.Conflicts`.
1. `fr, err := t.Fetch(...)` (this saves state); set `r.Fetch = fr`; error → return.
2. `!HasRemote` → `ErrNoRemote`.
3. `Remote == Base` → `r.UpToDate = true`, return nil.
4. `local := Scan(Root, Base, t.Get, SyncedAt, jobs)`; `incoming := DiffTrees(t.Get, Base, Remote)`; `apply, conflicts := Merge(local, incoming)`; `r.Conflicts = conflicts`.
5. If there are conflicts: `!force` → return `ErrConflict`. Nothing is written, but the fetch state is already saved. With `force`, append each `c.Incoming` to `apply`.
6. `Apply(Root, apply, t.Get)`, error raw. State is not saved, and `Base` does not move.
7. `r.Applied = apply`; `Base = Remote`, `SyncedAt = now`; return `SaveState()`.

**`Push(ctx, cl, user, force, jobs, prog)`** (217-259). Push does **not** fetch first.
1. `synced := (HasRemote && Remote == Base) || (!HasRemote && Base == EmptyTree)`.
2. If `!synced && !force`: `!HasRemote` → `ErrRemoteDeleted`, else `ErrRemoteMoved`.
3. `root, stats := ingest.Dir(t.Store, t.Root, ingest.Opts{Jobs: jobs, Exclude: []string{".dstore"}})`, error raw. Set `r.Root`, `r.Built`.
4. If `root == Base && synced` → `r.Nothing = true`, return nil. This includes an empty directory against a reference that does not exist.
5. `cond := client.Cond{Force: force}`; if `!force`: `Versioned = true`, `ExpectedVersion = RemoteVersion` (nil means the name must be new).
6. `ps, err := cl.Push(ctx, t.Store, root, Name, user, cond, prog)`. On error:
   - If it is not `*client.CASMismatch` → return it raw; state unchanged.
   - `cur, perr := key.Parse(cm.Current)`. If `!cm.HasCurrent || perr != nil || cur != root` → `fmt.Errorf("%w (%v)", ErrRefChanged, err)`.
     Text: `reference changed on the cluster since your last fetch: pull first, or --force (cas mismatch: current key <hex>)`, or `(cas mismatch: reference is absent)`.
   - Otherwise `r.Recovered = true`, `RemoteVersion = cm.Version`.
7. On success: `r.Stats = ps`; `RemoteVersion = ps.Version`.
8. `Base = Remote = root`, `HasRemote = true`, `SyncedAt = now`; return `SaveState()`.

**`Status(jobs)`** (278-303) is offline:
- `changes := Scan(Root, Base, t.Get, SyncedAt, jobs)`. `MetaChanged` entries are counted in `MetaOnly`; the rest go to `Changes`, in order.
- `Remote`: `!HasRemote` → `RemoteAbsent`; `Remote == Base` → `RemoteUpToDate`; otherwise `RemoteMoved` with `Incoming = DiffTrees(Base, Remote)`.

**`TicketFromView(v)`** (306-315): `Ticket{ClusterID: v.ClusterID, Incarnation: v.Incarnation}`,
then the first ≤4 `v.Nodes` in view order as `Member{ID: nd.ID, Addrs: nd.Addrs}`.
No filtering on writable, pending or voter status. Encoding belongs to the ticket spec.

**`RefreshTicket(cl)`** (320-331): `v := cl.View()`; nil → nil.
`s := TicketFromView(v).Encode()`; if `s == Config.Ticket`, return nil; else set `Config.Ticket = s` and `SaveConfig()`.
It saves the **stored** config, not a flag-overridden copy, so `--relay` and similar flags given for one run are never persisted.

**Concurrency and cancellation.** The worktree package spawns nothing itself.
Parallelism lives inside `ingest` (`Jobs`) and the client. `ctx` is observed only
by `RefGet`, `PullTree` and `Push`. `Scan`, `DiffTrees`, `Merge`, `Apply` and the
JSON writes do not check `ctx`, so a Ctrl-C during `Apply` does not stop it.
There are no retries or timeouts here; the client owns those (request timeout 2 min, `Dial` 15 s per member).
Mutual exclusion is the packstore flock held from `Open`/`Create` until `Close` (§3.4).

### 2.10 CLI commands (`cmd/dstore/wc.go`)

All seven are top-level commands, registered in `main.go` 43-45 in the order
`storeCmd, cloneCmd, initCmd, fetchCmd, pullCmd, pushCmd, statusCmd, diffCmd`.
Any returned error is printed as `"dstore: " + err + "\n"` on stderr, with exit
status 1 (main.go 48-51). Success exits 0.

The global flag `--log-level` (default `info`, env `DSTORE_LOG_LEVEL`) must be
given before the command. The urfave/cli v2 `flag` parser **stops at the first
positional argument**: `dstore diff PATH --stat` treats `--stat` as a path. `--` ends flags.

**Shared flags.** `wcFlags()` (27-46) take **no env vars**:
| flag | type | usage (verbatim) |
|---|---|---|
| `--ticket` | string | `cluster ticket (dstore1…) or comma-separated node ids; overrides the stored one for this run` |
| `--relay` | string | `relay URL for the fallback path (default: the built-in relay map)` |
| `--no-relay` | bool | `direct addresses only, no relay` |
| `--no-discovery` | bool | `neither announce this endpoint nor resolve node ids by discovery` |

- `jobsFlag()` (48): `--jobs` int, usage `parallelism (0 = cores)`, default 0.
- `noTUIFlag()` (tui.go 34-36): `--no-tui` bool, env `DSTORE_NO_TUI`, usage `plain log lines instead of the progress display`.

**`resolveTicket(flag, stored, env)`** (52-59) returns the first non-empty value,
else `errors.New("no cluster: set --ticket or $DSTORE_TICKET")`.

**`wcConfig(c, stored)`** (64-90), in order:
1. `cfg = *stored` or zero.
2. `cfg.Ticket = resolveTicket(--ticket, cfg.Ticket, $DSTORE_TICKET)` (error returned).
3. If `--relay` IsSet → `cfg.Relay`. If `--no-relay` IsSet → `cfg.NoRelay`.
4. If `--no-discovery` IsSet → `cfg.NoDiscovery`; else, **only when `stored == nil`** (clone/init), `strconv.ParseBool($DSTORE_NO_DISCOVERY)`. A parse error leaves the value unchanged.
5. If `--user` IsSet → `cfg.User`. The fetch and pull commands define no `--user` flag.

IsSet is true only when the flag appears on the command line.

**`dialConfig(ctx, cfg, log)`** (92-98) runs `ticket.Parse(cfg.Ticket)`, then
`dialTicket(ctx, t, netOpts{Relay, NoRelay, NoDiscovery}, log)`. That generates
an ephemeral secret key, builds the relay mode (`NoRelay` → none; `Relay` →
custom URL; else default), binds iroh with `Discover: !NoDiscovery`, and calls
`client.Dial(Config{Endpoint, Ticket, Logger, GCInterval: 4h})`. See client.go 77-96.

`openWC()` (101-107) is `worktree.Open(os.Getwd())`.

**`pushUser(c, cfg)`** (111-125): `--user`, else `cfg.User`, else
`user.Current().Username` (errors ignored). Then `reference.ValidateUser(u)`,
error `fmt.Errorf("user: %w", err)`, e.g. `user: user must not be empty`.

**`withCluster(c, title, fn)`** (227-250):
1. `tr := openWC()`; error → return. `defer tr.Close()`.
2. `cfg := wcConfig(c, &tr.Config)`.
3. `signalCtx()` (SIGINT/SIGTERM → cancel).
4. `runTransfer(ctx, c, title+" "+tr.Config.Name, fn2)`, where `fn2` does `cl := dialConfig`, `defer cl.Close()`, `fn(ctx, tr, cl, prog)`, and on success `tr.RefreshTicket(cl)`.

The TUI runs when stderr is a terminal and `--no-tui` is unset; otherwise plain mode (CLI/TUI spec).

#### `clone`
- Usage `clone the tree under NAME into DIR (default: the last segment of NAME) as a working copy`; ArgsUsage `NAME [DIR]`.
- Flags: wcFlags, `--user` (string, `user identity stored for pushes`), `--jobs`, `--no-tui`. `--jobs` is **parsed but unused**.

Action:
1. `name := Args.First()`; empty → `errors.New("clone NAME [DIR]")`.
2. `reference.ValidateName(name)`, error raw. Texts: `reference name must not be empty`, `reference name exceeds 1024 bytes`, `reference name must be valid UTF-8`, `reference name must not contain '@'`, `reference name must not contain control characters`.
3. `dir := Args.Get(1)`, or `path.Base(name)` when empty (`"a/b/"`→`"b"`, `"/"`→`"/"`).
4. `cfg := wcConfig(c, nil)`; `cfg.Name = name`; `signalCtx`.
5. `runTransfer(title "clone "+name)`: `dialConfig`; `cfg.Ticket = TicketFromView(cl.View()).Encode()` (always replaced, even when a short-form ids ticket was given); `worktree.Clone(ctx, cl, dir, cfg, prog)`.
6. Success, stdout: `"cloned %s into %s: root %s, %d objects fetched (%d bytes)\n"` with `(name, dir, fr.Key.String()[:16], fr.Stats.Fetched, fr.Stats.Bytes)`.

#### `init`
- Usage `make the current directory a working copy of NAME, with nothing synced yet`; ArgsUsage `NAME`.
- Flags: wcFlags, `--user` (`user identity stored for pushes`), `--no-tui`. There is no `--jobs`; extra args are ignored.

Action:
1. Empty name → `errors.New("init NAME")`; then `ValidateName`; `wcConfig(c, nil)`; `cfg.Name`; `wd := os.Getwd()`.
2. `runTransfer("init "+name)`: dial; `cfg.Ticket = TicketFromView(...)`; `worktree.Init(ctx, cl, wd, cfg, prog)`.
3. stdout:
   - `fr.Exists` → `"initialised working copy of %s; the reference exists (root %s): status shows everything as new, pull merges\n"`
   - else `"initialised working copy of %s; the reference does not exist yet: push creates it\n"`

#### `fetch`
- Usage `record the reference's current tree as the remote and fetch its objects`. Flags: wcFlags, `--no-tui`.
- Runs `withCluster("fetch")` → `tr.Fetch`. stdout:
  - `!Exists` → `"%s does not exist on the cluster\n"`
  - `UpToDate` → `"%s: up to date (%s)\n"` (key[:16])
  - else `"fetched %s: root %s, %d objects fetched (%d bytes)\n"`

#### `pull`
- Usage `fetch and apply the cluster's changes over the working directory`.
- Flags: wcFlags, `--force` (`take the cluster's side on conflicting paths`), `--jobs`, `--no-tui`.
- Runs `withCluster("pull")` → `tr.Pull(ctx, cl, --force, --jobs, prog)`.
- If the error `errors.Is(err, ErrConflict)`: **stderr** `"conflicts:\n"`, then for each conflict `"  %s (local: %s, cluster: %s)\n"` with `(Path, Local.Kind, Incoming.Kind)`. Then return the error, which prints `dstore: conflicting changes: …`. `RefreshTicket` does not run on error.
- stdout: `UpToDate` → `"already up to date\n"`; otherwise `"pulled: %d paths updated"` (`len(Applied)`), plus `", %d conflicts taken from the cluster"` when `len(Conflicts) > 0`, then `"\n"`.

#### `push`
- Usage `build the working directory's tree, upload it and write the reference`.
- Flags: wcFlags, `--user` (`user identity recorded in the reference (default: the stored one, then the OS user)`), `--force` (`replace the reference unconditionally`), `--jobs`, `--no-tui`.
- Runs `withCluster("push")` → `pushUser` (after dialing) → `tr.Push`. stdout:
  - `Nothing` → `"nothing to push\n"`
  - `Recovered` → `"%s already holds %s (an earlier push completed); state updated\n"` (name, root[:16])
  - else `"pushed %s: root %s, %d objects, %d uploaded, version %x\n"` with `(name, root[:16], Stats.Keys, Stats.Uploaded, Stats.Version)`.
- `--user` is not persisted.

#### `status`
- Usage `list the working directory's changes since the last sync, and whether the cluster moved`. Flags: `--jobs` only.
- No signal context, so SIGINT kills the process. It still takes the packstore lock.

stdout, in order:
1. `"reference %s, synced to %s\n"` (Name, Base[:16]).
2. One remote line:
   - `RemoteUpToDate` → `"remote: up to date\n"`
   - `RemoteAbsent` → `"remote: the reference does not exist on the cluster\n"`
   - `RemoteMoved` → `"remote: moved since your last fetch (+%d ~%d -%d; run pull)\n"`, counting `Incoming`: `Added` → a, `Deleted` → d, every other kind (including MetaChanged) → m.
3. If there are changes: `"changes:\n"`, then per change `"  %-9s %s\n"` with `(Kind.String(), describeChange(ch))`. `%-9s` left-pads to width 9.
4. If `MetaOnly > 0`: `"%d paths differ only in mtime, ownership or xattrs\n"` (also for 1).
5. If there are no changes and `MetaOnly == 0`: `"nothing to push\n"`, even when the remote moved.

**`describeChange(ch)`** (405-417):
1. `p := Path`; add `"/"` if `IsDir(New) || (New == nil && IsDir(Old))`.
2. `TypeChanged` → `"%s (%s → %s)"` with the type names; `→` is U+2192, bytes `E2 86 92`.
3. `ModeChanged` → `"%s (%04o → %04o)"` with the perm bits.
4. Otherwise `p`.

#### `diff`
- Usage `unified diffs of the working directory against the last synced tree`; ArgsUsage `[PATH...]`.
- Flags:
  - `--remote` (`against the tree last fetched from the cluster`)
  - `--incoming` (`the last synced tree against the fetched one (what pull would apply)`)
  - `--stat` (`one line per changed path with line counts`)
  - `--jobs`

Action:
1. If both `--remote` and `--incoming` → `errors.New("--remote and --incoming exclude each other")`, before opening the copy.
2. `openWC`.
3. `--incoming`: `!HasRemote` → `ErrNoRemote`; `changes = DiffTrees(Base, Remote)`; old = new = TreeSource.
4. `--remote`: `!HasRemote` → `ErrNoRemote`; `changes = Scan(Root, Remote, get, time.Now(), jobs)`; old = TreeSource, new = DiskSource.
5. Default: `Scan(Root, Base, get, SyncedAt, jobs)`; TreeSource / DiskSource.
6. If `NArg > 0`: `filterPaths(tr.Root, changes, args)`.
7. `--stat` → `Stat(os.Stdout, …)`, else `Unified(os.Stdout, …)`. Output is unbuffered, so partial output can precede an error.

**`filterPaths(root, changes, args)`** (478-501): for each arg,
`abs := filepath.Abs(a)` (cwd-relative, cleaned) and `rel := filepath.Rel(root, abs)`.
If `err != nil || rel == ".." || HasPrefix(rel, "../")` → `fmt.Errorf("%s is outside the working copy", a)` with the raw arg.
Symlinks are not resolved, and `..foo` counts as inside. A change is kept when some prefix `p` has `p == "."`, `Path == p` or `HasPrefix(Path, p+"/")`.

### 2.11 Node-side dependency

`dstore cluster ticket --store DIR` (main.go 341-345, 394-405) derives a ticket
with `worktree.TicketFromView(node.OpenOffline(dir).View())`. Its error is
`this store is not a member of a cluster`. This needs the node's persisted view
(Pebble meta store) and is out of reach without node internals.
`TicketFromView` itself is trivial to share.

### 2.12 Go standard-library behaviour the port must reproduce

- **`os.Getwd`** (Go 1.26 `os/getwd.go` 25-80): if `$PWD` is absolute and
  `SameFile(stat($PWD), stat("."))`, return `$PWD`, with its symlinks; otherwise `getcwd`.
  `Tree.Root`, `filterPaths` and the error texts depend on this. Rust's
  `std::env::current_dir` is plain getcwd, so port the `$PWD` rule.
- **`filepath.Abs`** = `Join(Getwd(), p)` → `Clean`. **`filepath.Rel`** is lexical.
  **`path.Base`** strips trailing slashes, gives `"."` for empty and `"/"` for all-slashes, and does no Clean.
- **`os.ReadDir`** sorts by `bytealg.CompareString` (bytewise).
- **`os.Remove`** (`os/file_unix.go` 357-388): `unlink`; if that fails, `rmdir`; if both fail,
  the error is rmdir's unless rmdir said `ENOTDIR`, in which case unlink's.
  The error is `&PathError{Op: "remove", Path, Err}`.
- **`os.MkdirAll`** (`os/path.go` 19-64): `Stat(path)` (follows symlinks). If it is
  a directory → nil; if it exists but is not a directory → `PathError{"mkdir", path, ENOTDIR}`.
  Otherwise recurse on the parent, then `Mkdir`; on error, tolerate an existing directory (`Lstat`).
  Rust's `create_dir_all` gives different errors.
- **`os.RemoveAll`**: nil for a missing path, removes files and trees.
- **`os.CreateTemp(dir, ".dstore-tmp-*")`**: name `prefix + strconv.FormatUint(uint64(uint32(runtime_rand())), 10)`,
  `O_RDWR|O_CREATE|O_EXCL`, mode 0600, retry on EEXIST up to 10000 times.
- **`PathError` text** is `"<op> <path>: <errno text>"` using Go's lowercase errno
  strings (`no such file or directory`, `permission denied`,
  `resource temporarily unavailable`, `not a directory`, `directory not empty`, …).
  Rust `io::Error` prints `Permission denied (os error 13)`, so a Go errno table is
  required for byte-identical error messages (§7).
- **`strconv.Quote`** (`%q`): `\a \b \f \n \r \t \v \\ \"`; other bytes < 0x20 and
  0x7f as `\x..`; printable Unicode raw; non-printable runes as `\u....`/`\U........`;
  each invalid UTF-8 byte as `\x..`. Used by `refusing unsafe path %q` and `xattr %q`.
- **`encoding/json` v1** (`encode.go` builds with `!goexperiment.jsonv2`, so v1 is in effect).
  Marshal escaping, `appendString` 999-1060, `escapeHTML=true`:
  - `"`→`\"`, `\`→`\\`, `\b`→`\b`, `\f`→`\f`, `\n`→`\n`, `\r`→`\r`, `\t`→`\t`;
  - other bytes < 0x20 → `\u00XX` with lowercase hex;
  - `<`→`<`, `>`→`>`, `&`→`&`;
  - 0x7f raw (`tables.go` 113, 219);
  - U+2028/U+2029 → ` `/` `;
  - **each** invalid UTF-8 byte (Go `DecodeRuneInString` returning `RuneError, 1`) → `�`.
    Rust's `from_utf8_lossy` replaces maximal subparts instead and would differ.
  - `MarshalIndent(v, "", "  ")`: `{`, newline, for each field two spaces, `"key": value`, `,` between fields, newline, `}`.
    Bools print as `true`. `omitempty` drops `""` and `false`.
- **`json.Unmarshal` v1** into a struct: exact key match first, then
  case-insensitive fold match (`foldName`, including the special folds
  K/U+212A and S/U+017F); unknown keys ignored; last duplicate wins; `null`
  leaves the field unchanged. Type mismatch → `json: cannot unmarshal number into Go struct field Config.ticket of type string`,
  and decoding continues, returning the first error. Syntax errors read `invalid character 'x' looking for beginning of value`
  or `unexpected end of JSON input`. Invalid UTF-8 inside strings becomes U+FFFD.
- **`time.Format(time.RFC3339Nano)`** in UTC: `YYYY-MM-DDTHH:MM:SS`, then a fraction
  of up to 9 digits with **trailing zeros removed** (omitted entirely when the
  nanoseconds are 0), then `Z`. `time.Parse(RFC3339Nano)` also accepts offsets `±hh:mm`.
- **`strconv.ParseBool`** accepts `1 t T TRUE true True 0 f F FALSE false False`.
- **`os/user.Current().Username`**:
  - Darwin, always: `getpwuid_r(getuid())`, `pw_name`.
  - Linux with cgo: the same.
  - Linux **without cgo** (the dstore Dockerfile builds `CGO_ENABLED=0`, line 10): parse `/etc/passwd` for the uid; if that fails, use `$USER`, and only when uid, `$USER` and `$HOME` are all non-empty; otherwise an error.
  - `pushUser` ignores the error and keeps `""`, which then fails validation.
- **`sort.Slice`** is Go's pdqsort (§3.9). It is observable only in `lcs.fix`.

---

## 3. Byte formats and text formats

### 3.1 `.dstore/` layout

```
<root>/.dstore/            dir, os.MkdirAll 0o755 (& ~umask)
<root>/.dstore/config      JSON (§3.2), 0o644 (& ~umask), written via config.tmp + rename
<root>/.dstore/state       JSON (§3.3), 0o644, via state.tmp + rename; absent = incomplete clone
<root>/.dstore/packstore/  core packstore (segment files, flock on the directory)
<root>/.dstore/config.tmp, state.tmp   only left over after a crash mid-write
```
`.dstore` is excluded from the tree only at the root: by the scan (`listDir`)
and by push (`ingest.Opts.Exclude`). A nested `.dstore` deeper in the tree is data.
Packstore files follow the core-rs contract: interoperable, but compressed
records are not byte-identical. `.dstore-tmp-<n>` files appear only in the working directory.

### 3.2 `config`

Field order: `ticket`, `name`, `relay`, `no_relay`, `no_discovery`, `user`.
The last four are omitted when empty or false. The file ends with `\n`.
Bytes verified with Go 1.26.5:
```
{
  "ticket": "t",
  "name": "n"
}
```
`Config{Ticket:"dstore1abc", Name:"trees/<a&b> x\x01\"\\", NoRelay:true, User:"Dr <d@x>"}`:
```
{
  "ticket": "dstore1abc",
  "name": "trees/<a&b> x\"\\",
  "no_relay": true,
  "user": "Dr <d@x>"
}
```
`user` and `relay` are stored unvalidated, so control characters and invalid
UTF-8 can occur there.

### 3.3 `state`

All four keys are always present, in the order `base`, `remote`, `remote_version`,
`synced_at`. Keys and versions are lowercase hex (`hex.EncodeToString`).
Without a remote, `remote` and `remote_version` are `""`. Example (verified):
```
{
  "base": "20ab",
  "remote": "",
  "remote_version": "",
  "synced_at": "2023-11-14T22:13:20.000000005Z"
}
```
RFC3339Nano samples (unix 1700000000, verified):
- ns 0 → `2023-11-14T22:13:20Z`
- ns 5 → `2023-11-14T22:13:20.000000005Z`
- ns 120000000 → `2023-11-14T22:13:20.12Z`
- ns 123456789 → `2023-11-14T22:13:20.123456789Z`

### 3.4 The lock

`packstore.Open` (core `packstore/packstore.go` 152-175) opens the packstore
directory and calls `flock(fd, LOCK_EX|LOCK_NB)` for the lifetime of the `Store`.
On failure: `fmt.Errorf("packstore: %s is already open: %w", dir, err)`, e.g.
`dstore: packstore: /abs/wc/.dstore/packstore is already open: resource temporarily unavailable`.
core-rs `packstore::Store::open_with` (`src/packstore/mod.rs` 364-378) uses the
same `flock(2)` call, so a Go process and a Rust process exclude each other correctly.
Its message text uses Rust's io::Error Display (§7).
Every command, `status` and `diff` included, holds the lock while it runs.

### 3.5 Inline xattr encoding (shared with core)

`cborx.EncodeXattrs` (core `cborx/cborx.go` 45-62) produces a canonical CBOR map
of bstr→bstr, with keys sorted by their encoded bytes. For `{"user.wc": "1"}` it is
`a147757365722e77634131`. When the encoding is ≤256 bytes it goes in DirLeaf key 8;
otherwise an XattrSet key goes in key 9.

### 3.6 Status output example

```
reference trees/demo, synced to 2001bbe6a9f5a014
remote: moved since your last fetch (+1 ~2 -0; run pull)
changes:
  new       docs/new.txt
  modified  src/main.go
  deleted   old/
  type      bin/tool (file → symlink)
  mode      run.sh (0644 → 0755)
3 paths differ only in mtime, ownership or xattrs
```
Each row is `"  "`, then the kind padded to 9 columns, a space, the detail.

### 3.7 Diff output

For each change: `diff a/P b/P\n`; `old mode %04o\nnew mode %04o\n` when perms
differ; then either `Binary files a/P and b/P differ\n` (a nil side labelled
`/dev/null`) or the go-udiff unified text:
```
--- a/P            (or /dev/null)
+++ b/P            (or /dev/null)
@@ -F[,N] +T[,M] @@
 context / -deleted / +inserted lines
\ No newline at end of file      (after any line lacking a trailing \n)
```
A directory's own mode change is the `diff` line and the mode lines only.
Verified samples:
- `Unified("a/x","b/x","a","a\nb")` = `"--- a/x\n+++ b/x\n@@ -1 +1,2 @@\n-a\n\\ No newline at end of file\n+a\n+b\n\\ No newline at end of file\n"`
- `Unified("/dev/null","b/x","","hi\n")` = `"--- /dev/null\n+++ b/x\n@@ -0,0 +1 @@\n+hi\n"`
- `Unified("a/x","/dev/null","bye\n","")` = `"--- a/x\n+++ /dev/null\n@@ -1 +0,0 @@\n-bye\n"`

### 3.8 `--stat` output

```
 edit.txt | +1 -1
 bin | binary
 new.txt | +1 -0
 3 files changed, 2 insertions(+), 1 deletions(-)
```

### 3.9 go-udiff v0.4.1: the algorithm to port (byte-identical)

`udiff.Unified(oldLabel, newLabel, old, new)` (unified.go 22-30) is
`ToUnified(oldLabel, newLabel, old, Lines(old, new), 3)`. Only this path is
used. `Strings`, `Bytes`, runes, `merge.go` and `ApplyUnified` are not needed.

**Evidence that an exact port is required.** A Go 1.26.5 run over 3000 random
cases used math/rand seed 1, inputs of 50-450 lines each, lines drawn from 2-41
distinct values. `lcs.fix()` was reached in 2952 of them. Replacing the
`sort.Slice` in `fix` with `sort.SliceStable` changed the unified output in
**150 cases (5%)**. A different diff algorithm (`similar`, `imara-diff`), a
different sort, or a different search limit would therefore produce different
hunks on real edits. Port the files below line by line.

Size to port (Go lines, only the used parts):
| part | Go source | ~lines |
|---|---|---|
| `Lines` | ndiff.go 16-31 | 16 |
| `validate`, `SortEdits`, `lineEdits`, `expandEdit` | diff.go 66-177 | 110 |
| `toUnified`, `splitLines`, `addEqualLines`, `String` | unified.go 105-266 | 160 |
| `diff`, `compute`, `editGraph`, `toDiffs`, `forward`, `forwardlcs`, `lookForward`, `setForward`/`getForward`, `backwardlcs`, `lookBackward`, `setBackward`/`getBackward`, `twosided`, `twoDone`, `twolcs` | lcs/old.go 13-475 (skip `backward` 203-249, tests only) | 400 |
| `lcs.sort`, `fix`, `overlap`, `prepend`, `append`, `ok` | lcs/common.go 12-179 | 170 |
| `label` set/get | lcs/labels.go 29-55 | 25 |
| `linesSeqs`, `commonPrefixLen`/`commonSuffixLen` | lcs/sequence.go 38-70 | 30 |
| Go pdqsort: `insertionSort_func`, `siftDown_func`, `heapSort_func`, `pdqsort_func`, `partition_func`, `partitionEqual_func`, `partialInsertionSort_func`, `breakPatterns_func`, `choosePivot_func`, `order2_func`, `median_func`, `medianAdjacent_func`, `reverseRange_func`, `xorshift`, `nextPowerOfTwo`, `sort.Slice` wrapper | sort/zsortfunc.go 10-327, sort/sort.go 59-80, sort/slice.go 24-30 | 300 |

In total about 1200 Go lines, roughly 900-1000 lines of Rust.

Points that are easy to get wrong:

1. **`splitLines(text)`** (unified.go 178-194) splits after each `\n`, keeping the
   terminator; a final partial line is kept; `""` gives no lines. The offsets
   start at `[0]` and gain the start of each following line, plus `len(text)`
   when there is a partial last line. Go ranges over runes, but `\n` is always one
   byte and never a UTF-8 continuation byte, so a bytewise split is equivalent.
2. **`Lines(before, after)`**: `beforeLines, bOffsets := splitLines(before)`;
   `afterLines := splitLines(after)`; `diffs := lcs.DiffLines(a, b)`. Each
   `Edit{Start: bOffsets[d.Start], End: bOffsets[d.End], New: concat(afterLines[d.ReplStart:d.ReplEnd])}`.
3. **`lcs.diff`** is `compute(seqs, twosided, maxDiffs/2)` with `maxDiffs = 100`, so
   **limit = 50**; `limit <= 0` would mean 1<<25. `editGraph{vf, vb: newtriang(limit), limit, lx=ly=0, ux=alen, uy=blen, delta=alen-blen}`.
   `lx`/`ly` are never changed. `twolcs`'s "need to compute another path" case
   temporarily sets `ux=u`, `uy=v`, re-runs `forward(e)`, which rewrites `vf` from
   D=0 with the same limit, then restores them. `delta` is **not** updated.
4. **`label`**: `vec[D][(D+k)/2]`; the row for D is allocated with length D+1 on
   first `set`, and later sets overwrite. All reads are guarded by `ok(d,k)`
   (`d >= 0 && -d <= k <= d`) or happen after the corresponding set.
   Integer `%` and `/` truncate toward zero, the same in Rust; `twoDone`'s parity
   test `(df+db+delta)%2 != 0` depends on it.
5. **`twosided`** loop is `for D := 0; D < limit; D++`:
   - `twoDone(D, D)` → `twolcs`.
   - Forward pass to D+1: set k=-(D+1) from `getForward(D,-D)`; set k=D+1 from `getForward(D,D)+1`; for `k := -D+1; k <= D-1; k += 2`, take `lookv = lookForward(k, getForward(D,k-1)+1)` and `lookh = lookForward(k, getForward(D,k+1))`, keeping `lookv` only if strictly greater.
   - `twoDone(D+1, D)`.
   - Backward pass: set k=-(D+1) from `getBackward(D,-D)-1`; k=D+1 from `getBackward(D,D)`; inner k takes the strictly smaller of `lookBackward(k, getBackward(D,k-1))` and `lookBackward(k, getBackward(D,k+1)-1)`.
   - After the loop, combine the forward LCS at `kmax` (maximal x+y inside the
     rectangle, first k to reach a strictly greater sum, k from -limit to limit
     step 2) with the backward LCS at `kmax` (minimal x+y with x,y >= 0, strictly
     smaller). `lcs = append(forward, backward...)`, then `fix()`.
6. **`twoDone`** and **`twolcs`** (old.go 376-475): port verbatim, including the
   special cases in their order and the inner `for l := k; l <= kmax` loop. It
   returns `l` when `x == u || u == 0 || v == 0 || y == uy || x == ux`, else `k`.
7. **`lcs.sort()`** (common.go 23-31): `sort.Slice` with less `(X asc, then Len desc)`.
   **`fix()`** (50-79):
   - Empty → nil.
   - `sort.Slice(l, Len desc)`. This is Go pdqsort and the order among equal lengths matters (evidence above).
   - Greedy: `tmp = [l[0]]`. For `i := 1..`, set `nxt = l[i]`; for each `in` in `tmp`, `dir, nxt = overlap(in, nxt)` and break on `empty` or `bad`. Append `nxt` if `nxt.Len > 0 && dir != bad`. Note that `dir` stays at its zero value `empty` only when `tmp` is empty, which cannot happen.
   - `tmp.sort()`.
8. **`overlap`** (92-137): port as written. The trims happen in this order: X-end, X-begin, Y-end, Y-begin, then the `leftdown`/`rightup`/`bad` classification.
9. **`toDiffs`** (71-85): `Diff{pa, l.X, pb, l.Y}` when `pa < X || pb < Y`, then a tail diff when `pa < alen || pb < blen`.
10. **`validate`**: when `!sort.IsSorted(edits)`, clone and `sort.Stable` by (Start, End). `Lines` output is already sorted.
11. **`lineEdits`** fast path (diff.go 121-129): return the edits unchanged unless some edit has
    - `Start >= len(src)` (insertion at EOF),
    - `Start > 0 && src[Start-1] != '\n'`,
    - `End > 0 && src[End-1] != '\n'`, or
    - a non-empty `New` not ending in `\n`.
    Otherwise take the slow path: merge consecutive edits whose `src[prev.End:edit.Start]` contains no `\n` (`prev.New += between + edit.New; prev.End = edit.End`), and `expandEdit` each flushed edit (155-177).
12. **`toUnified`** (105-174):
    - `gap = 2*context = 6`; `lines := splitLines(content)`.
    - For each edit: `start = count('\n' in content[:Start])`; `end = count('\n' in content[:End])`; if `End == len(content) && len > 0 && last byte != '\n'`, then `end++`.
    - If `h != nil && start == last`, extend the hunk directly. Else if `h != nil && start <= last+gap`, add Equal lines `[last, start)`. Else close the previous hunk with Equal lines `[last, last+3)` (clamped at EOF), `toLine += start - last`, and open a new hunk with `FromLine = start+1`, `ToLine = toLine+1`, prepending Equal lines `[start-3, start)` (negatives skipped) and subtracting their count from both line numbers.
    - Then `last = start`. Deleted lines are `lines[start:end]` with `last++` for each. Inserted lines are `splitLines(New)` with `toLine++` for each.
    - Finally close with Equal lines `[last, last+3)`.
13. **`String`** (213-266):
    - No hunks → `""`. Otherwise `"--- %s\n+++ %s\n"`.
    - Per hunk: `fromCount` = Delete + Equal lines, `toCount` = Insert + Equal lines.
    - `"@@"`, then the from part: `fromCount > 1` → `" -%d,%d"`; else `FromLine == 1 && fromCount == 0` → `" -0,0"`; else `" -%d"`. The to part follows the same rule with `+`. Then `" @@\n"`.
    - Lines are prefixed `-`, `+` or a space. A line not ending in `\n` is followed by `"\n\\ No newline at end of file\n"`.
14. **pdqsort** (`sort.Slice` → `pdqsort_func(data, 0, n, bits.Len(uint(n)))`):
    - `maxInsertion = 12`: insertion sort when `length <= 12`; heapsort when `limit == 0`.
    - `breakPatterns` only when the previous partition was unbalanced; `xorshift` is seeded with `length`, and the swap window is `idx := a + (length/4)*2 - 1 ..= +1`, with `other = uint(random.Next()) & (nextPowerOfTwo(length)-1)` reduced by `length` if `>= length`.
    - `choosePivot`: `l >= 8` → median of three; `l >= 50` → Tukey ninther; the swap count decides the hint (0 increasing, 12 decreasing).
    - A decreasing hint reverses the range and sets `pivot = (b-1)-(pivot-a)`.
    - `partialInsertionSort`: 5 steps, `shortestShifting = 50`.
    - `partitionEqual` when `a > 0 && !less(a-1, pivot)`; otherwise `partition`.
    - Recurse on the smaller side; `balanceThreshold = length/8`.
    - `less` and `swap` act on indices of the slice being sorted.

---

## 4. Rust design

### 4.1 Modules (proposal; adapt to the workspace layout)

```
dstore_client::worktree::{mod, tree, change, scan, merge, apply, diff, flow, xattr, sys}
dstore_client::udiff::{mod, edits, unified, lcs, labels}      // go-udiff port, Unified path only
dstore_client::gocompat::{sort, json, rfc3339, errno, quote, fs, getwd, user}
dstore CLI: src/bin/dstore/wc.rs (clap 4.6), using gocompat::errno for "dstore: …" messages
```
`gocompat` holds the stdlib behaviour of §2.12. It is shared with the other areas
(errno texts, `%q`, getwd), so decide once where it lives.

### 4.2 Types and signatures

```rust
pub const DIR: &str = ".dstore";
pub const RACY_WINDOW_NS: i64 = 2_000_000_000;
pub const MAX_DIFF_BYTES: u64 = 16 << 20;

/// Go strings from args/JSON can be arbitrary bytes; keep bytes, render with the
/// Go JSON encoder (§2.12). ticket/name are valid UTF-8 in practice; relay/user may not be.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Config { pub ticket: Vec<u8>, pub name: Vec<u8>, pub relay: Vec<u8>,
                    pub no_relay: bool, pub no_discovery: bool, pub user: Vec<u8> }

#[derive(Clone, Debug)]
pub struct State { pub base: Key, pub remote: Key, pub has_remote: bool,
                   pub remote_version: Option<Vec<u8>>,   // None = Go nil, Some(empty) = decoded ""
                   pub synced_at: GoTime }                // (unix_secs: i64, nanos: u32), UTC

pub struct Tree { pub root: PathBuf /* Go string; bytes via OsStr */, pub config: Config,
                  pub state: State, pub store: Arc<packstore::Store> }

#[derive(Clone, Copy, Debug, PartialEq, Eq)] #[repr(u8)]
pub enum Kind { Added, Deleted, Modified, TypeChanged, ModeChanged, MetaChanged }  // Display = "new" …
#[derive(Clone, Debug)] pub struct Change { pub path: Vec<u8>, pub kind: Kind,
                                             pub old: Option<Arc<Entry>>, pub new: Option<Arc<Entry>> }
#[derive(Clone, Debug)] pub struct Conflict { pub path: Vec<u8>, pub local: Change, pub incoming: Change }
pub struct FetchResult { pub exists: bool, pub up_to_date: bool, pub key: Key, pub stats: client::PullStats }
pub struct PullResult { pub fetch: FetchResult, pub up_to_date: bool, pub applied: Vec<Change>, pub conflicts: Vec<Conflict> }
pub struct PushResult { pub root: Key, pub nothing: bool, pub recovered: bool,
                        pub built: packstore::WriteStats, pub stats: client::PushStats }
pub enum RemoteState { UpToDate, Moved, Absent }
pub struct Status { pub changes: Vec<Change>, pub meta_only: usize, pub remote: RemoteState, pub incoming: Vec<Change> }

pub type Getter<'a> = &'a (dyn Fn(Key) -> Result<Vec<u8>, packstore::Error> + Sync);

// tree.rs (blocking)
pub fn empty_tree() -> (Key, Vec<u8>);                                // fstree::encode_dir_leaf(&[])
pub fn find(dir: &Path) -> Result<PathBuf, Error>;
impl Tree {
    pub fn open(dir: &Path) -> Result<Tree, Error>;
    pub(crate) fn open_raw(dir: &Path) -> Result<Tree, Error>;
    pub fn create(dir: &Path, cfg: Config) -> Result<Tree, Error>;
    pub fn close(self) -> Result<(), Error>;
    pub fn get(&self, k: Key) -> Result<Vec<u8>, packstore::Error>;
    pub fn save_config(&self) -> Result<(), Error>;
    pub fn save_state(&self) -> Result<(), Error>;
    pub fn status(&self, jobs: usize) -> Result<Status, Error>;
}
pub fn remove(dir: &Path) -> Result<(), Error>;

// change.rs / scan.rs / merge.rs / apply.rs (blocking)
pub fn type_name(mode: u64) -> String;
pub fn same_content(a: &Entry, b: &Entry) -> bool;
pub fn equivalent(a: &Entry, b: &Entry) -> bool;
pub fn compare(old: &Entry, new: &Entry) -> Option<Kind>;
pub fn diff_trees(get: Getter, a: Key, b: Key) -> Result<Vec<Change>, Error>;
pub fn scan(root: &Path, base: Key, get: Getter, synced_at: GoTime, jobs: usize) -> Result<Vec<Change>, Error>;
pub fn merge(local: &[Change], incoming: &[Change]) -> (Vec<Change>, Vec<Conflict>);
pub fn apply(root: &Path, changes: &[Change], get: Getter) -> Result<(), Error>;

// diff.rs (blocking; w is stdout, unbuffered or flushed per write to keep partial output)
pub enum SourceError { TooLarge, Other(Error) }
pub trait Source { fn content(&self, p: &[u8], e: &Entry) -> (Option<Vec<u8>>, Option<SourceError>); }  // data may accompany an error (Go os.ReadFile)
pub struct TreeSource<'a> { pub get: Getter<'a> }
pub struct DiskSource { pub root: PathBuf }
pub fn unified(w: &mut dyn io::Write, changes: &[Change], old: &dyn Source, new: &dyn Source) -> Result<(), Error>;
pub fn stat(w: &mut dyn io::Write, changes: &[Change], old: &dyn Source, new: &dyn Source) -> Result<(), Error>;

// flow.rs (async over the client port)
impl Tree {
    pub async fn fetch(&mut self, cl: &client::Cluster, prog: Option<client::Progress>) -> Result<FetchResult, Error>;
    pub async fn pull(&mut self, cl: &client::Cluster, force: bool, jobs: usize, prog: Option<client::Progress>)
        -> (PullResult, Result<(), Error>);   // result even on ErrConflict
    pub async fn push(&mut self, cl: &client::Cluster, user: &str, force: bool, jobs: usize,
                      prog: Option<client::Progress>) -> Result<PushResult, Error>;
    pub fn refresh_ticket(&mut self, cl: &client::Cluster) -> Result<(), Error>;
}
pub async fn clone(cl: &client::Cluster, dir: &Path, cfg: Config, prog: Option<client::Progress>) -> Result<(Tree, FetchResult), Error>;
pub async fn init(cl: &client::Cluster, dir: &Path, cfg: Config, prog: Option<client::Progress>) -> Result<(Tree, FetchResult), Error>;
pub fn ticket_from_view(v: &view::View) -> ticket::Ticket;

// udiff
pub fn unified(old_label: &[u8], new_label: &[u8], old: &[u8], new: &[u8]) -> Vec<u8>;   // go-udiff Unified
pub(crate) fn lines(before: &[u8], after: &[u8]) -> Vec<Edit>;
pub(crate) mod lcs { pub fn diff_lines(a: &[&[u8]], b: &[&[u8]]) -> Vec<Diff>; }
// gocompat::sort
pub fn slice<T>(v: &mut [T], less: impl FnMut(&T, &T) -> bool);                   // Go pdqsort_func port
```

`Error` is an enum whose Display strings are exactly the Go texts of §2 (the
sentinels, the `fmt.Errorf` wraps, and the wrapped core/client errors rendered
through gocompat). `Clone`/`Init` accept `ctx` cancellation through the client's
futures. `fetch`, `pull_tree` and `push` are the only await points. Run the
blocking parts (`scan`, `diff_trees`, `apply`, `ingest::dir`, `hash_file`) with
`tokio::task::spawn_blocking`, holding `Arc<Store>`. Like Go, they must **not**
be abortable mid-way by SIGINT. The CLI installs `tokio::signal` handlers for
SIGINT and SIGTERM only for clone, init, fetch, pull and push, and cancels a token
the client observes. With the handler installed, the process is not killed by
the signal; a cancelled transfer ends as `dstore: context canceled` with exit 1
(check the exact text against the client spec). `status` and `diff` install no
handler, so they die by signal.

### 4.3 Mapping onto core-rs (amber-store-core 0.3.0, rev a85ffa1; files checked)

| Go | Rust | notes |
|---|---|---|
| `ingest.Objects(path, Opts{Jobs})` | `ingest::objects(path, ingest::Opts{ jobs, ..Default::default() })` → `(ObjectStream, Root)`; drain the iterator, return the first `Err`, then `root.get()` (`src/ingest/mod.rs` 311-372) | spawns one thread per call; fine |
| `ingest.Dir(st, root, Opts{Jobs, Exclude: [".dstore"]})` | `ingest::dir(&store, root, Opts{ jobs, exclude: vec![".dstore".into()], .. })` → `(WriteStats, Result<Key>)` (379-411) | Go negative `--jobs` → map to 0 |
| `ingest.DefaultXattrInlineMax` | `ingest::DEFAULT_XATTR_INLINE_MAX` (50) | 256 |
| `fstree.Entry` | `fstree::Entry` (`src/fstree/mod.rs` 52-73); empty `Vec` = absent | same fields |
| `fstree.CollectEntries(k, get)` | `fstree::collect_entries(k, |k| store.get(k))` (`src/fstree/read.rs` 536) | `WalkError<packstore::Error>` |
| `fstree.WriteContent(w, k, get)` | `fstree::write_content(&mut w, k, get)` (653) | |
| `fstree.EncodeDirLeaf(nil)` | `fstree::encode_dir_leaf(&[])` (`src/fstree/encode.rs` 130) | |
| `fstree.EncodeXattrSet(m)` | `fstree::encode_xattr_set(&BTreeMap<Vec<u8>,Vec<u8>>)` (171) | infallible |
| `cborx.EncodeXattrs/DecodeXattrs` | `cbor::encode_xattrs` / `cbor::decode_xattrs` (`src/cbor.rs` 220, 245) | BTreeMap keys = bytes |
| `amberignore.Root/Descend/Ignored` | `amberignore::Matcher::root(dir)`, `.descend(abs, name: &[u8])`, `.ignored(name, is_dir)` (`src/amberignore.rs` 49-64) | root error is a bare `io::Error`; wrap it as `open <dir>/.amberignore: …` (Go PathError) |
| `key.Parse/Length/String/Type` | `key::Key::parse`, `.length()`, `Display` (lowercase hex), `.type_()` (`src/key.rs` 143-229) | |
| `packstore.Open(dir, WithSync(true))` | `packstore::Store::open_with(dir, packstore::Options::new().sync(true))` (`src/packstore/mod.rs` 364) | same flock |
| `Store.Put/Get/Close` | `.put(k, &bytes)`, `.get(k)`, `.close()` (752, 772, 979) | |
| `reference.ValidateName/ValidateUser` | `reference::validate_name(&str)`, `validate_user(&str)` (`src/reference.rs` 265, 292) | take `&str`; non-UTF-8 input must produce Go's `… must be valid UTF-8` text before calling |
| xattr read (`unix.Listxattr`/`Getxattr`, `Llistxattr`/`Lgetxattr`) | `xattr` 1.6.1: macOS `xattr::list_deref`/`get_deref`, Linux `xattr::list`/`get` (as `src/ingest/xattrs.rs` does) | `get` → `None` must become an ENOATTR error, like core-rs |
| xattr set (`unix.Setxattr(…,0)` darwin / `Lsetxattr` linux) | macOS `xattr::set_deref`, Linux `xattr::set` (`xattr-1.6.1/src/sys/linux_macos.rs` 95-98: `setxattr`/`lsetxattr`, flags empty) | |
| Lstat/mode/uid/gid/mtime | `std::fs::symlink_metadata` + `MetadataExt`; `mtime*1e9 + mtime_nsec` wrapping (as `src/ingest/meta.rs` 31-41) | |
| `unix.Major/Minor/Mkdev` | copy the formulas (`src/ingest/meta.rs` 53-76) plus Mkdev (§2.7) | |
| chmod/lchown/mkfifo/mknod/utimensat/geteuid/flock/getpwuid_r | `libc` 0.2.186 (`chmod`, `lchown`, `mkfifo`, `mknod`, `utimensat` with `AT_FDCWD`/`AT_SYMLINK_NOFOLLOW`, `geteuid`, `getuid`, `getpwuid_r`) | or `nix` 0.30.1 / `rustix` 1.1.4 |

Rust iroh is not touched directly. The client port supplies `Cluster::ref_get`,
`pull_tree`, `push`, `view`, `close`, `ErrUnknownRef`, `CasMismatch`, `Cond`
and the stats structs.

### 4.4 Crate dependencies (offline versions available)

`amber-store-core` 0.3.0 (path or git rev a85ffa1), `libc` 0.2.186, `xattr` 1.6.1,
`tokio` 1.52/1.53 (`rt-multi-thread`, `signal`, `macros`), `thiserror` 2.0.20,
`hex` 0.4.3, `clap` 4.6.6 (CLI). Optionally `serde_json` 1.0.151, for reading
only (§8). Do **not** use `similar`, `imara-diff`, `serde_json`'s writer,
`tempfile` (different temp-file names), `std::fs::create_dir_all` (errors), or
`String::from_utf8_lossy` (replacement rule).

## 5. Golden vectors a Go generator should emit

Put a generator at `tools/wtgen` in this repo that pins dstore v0.1.9, core v0.0.8
and go-udiff v0.4.1, writes `tests/golden/worktree/*.json`, and emits:

1. **Empty tree**: key `2001bbe6a9f5a0146a1f4d0381e9b0ed1ac2f1a979ce9d5ad84e46ff0b58f36b`, bytes `80`, `[:16]` = `2001bbe6a9f5a014`.
2. **Config JSON bytes** (`writeJSON` output including the trailing `\n`) for:
   - zero-ish `{t, n}`;
   - all six fields set;
   - `relay` containing `&`, `<`, `>`, `\b\f\n\r\t`, `\x01`, `\x7f`, ` `, ` `;
   - invalid UTF-8: `\xff`, a truncated `\xe2\x82`, a surrogate `\xed\xa0\x80`, and an overlong `\xc0\xaf`;
   - `user` with `"` and `\`.
   Verified example in §3.2.
3. **State JSON bytes**: no remote; with remote and version `01 02 03`; `SyncedAt` ns ∈ {0, 5, 120000000, 123456789, 999999999}; a pre-1970 time (negative unix).
4. **loadState decoding**: accepted inputs (uppercase hex, `+02:00` offset, keys in other case such as `"BASE"`, duplicate keys, unknown keys, `null` values) and rejected inputs, with Go's exact error strings (§2.3).
5. **go-udiff**. Write every input pair and output; don't try to replay Go's `math/rand` in Rust:
   - the three samples in §3.7;
   - all `difftest.TestCases` inputs (`empty`, `no_diff`, `replace_all`, `insert_rune`, `delete_rune`, `replace_rune`, `replace_partials`, `insert_line`, `replace_no_newline`, `delete_empty`, `append_empty`, `add_end`, `add_empty`, `add_newline`, `delete_front`, `replace_last_line`, `multiple_replace`, `extra_newline`, `unified_lines`, `60379`) as `Unified("from","to",In,Out)`;
   - edits close to the 6-line gap boundary (5, 6, 7 unchanged lines between edits);
   - an insertion at EOF without a trailing newline;
   - ≥3000 random line-alphabet cases (sizes 50-450 lines, 2-41 distinct lines), which exercise `twosided`'s limit-50 fallback, `fix()` and the `twolcs` special cases;
   - inputs with invalid UTF-8 and with `\r\n`;
   - huge inputs near 16 MiB (a few, for performance).
6. **Go pdqsort permutation vectors**: `sort.Slice` over `[]struct{X,Y,Len int}` with `Len desc` less, for n ∈ {0..13, 49, 50, 51, 100, 500} and random duplicate-heavy lengths. Emit the resulting order to test `gocompat::sort` alone.
7. **lcs**: `DiffLines` outputs (`[]Diff`) for the random cases above, plus `TestLcsFix` before/after sets.
8. **Change lists** (`path`, `kind`, old/new type and perms) from `DiffTrees` and `Scan`:
   - over the `change_test`/`scan_test` fixtures with fixed mtimes and xattrs;
   - a walk-order case with names `a/` (containing `x`), `a-b` and `a.txt`;
   - a type change file→dir and dir→file;
   - an ignored base path;
   - a root `.dstore` in the base tree;
   - the racy-mtime rule: base mtime = syncedAt−2s exactly (not hashed), and −2s+1ns (hashed).
9. **Merge decision table**: the 13 `TestMerge` cases, plus an ancestor with an intermediate ModeChanged, both sides adding the same dir with equal and with different perms, and an incoming ModeChanged against a local Modified.
10. **Unified/Stat full outputs**. Build trees in a packstore with fixed mtimes (the `diffFixtures` shape) and add:
    - an added empty file (header only);
    - an added fifo;
    - a symlink→file type change whose target text equals the file bytes;
    - a file of exactly 16777216 bytes (diffed) and one of 16777217 bytes (binary);
    - a NUL at offset 8191 (binary) and at 8192 (text);
    - a mode+content change (mode lines followed by a hunk);
    - a dir mode change;
    - stat's miscount: a deleted line `--x` and an added line `++y`;
    - `filterPaths` with `.`, a subdir, and an outside path.
11. **describeChange/TypeName**: all types, `type 0` and `type 0160000`, directory slash rules, `%-9s` rows.
12. **Apply results**: after applying the fixture update onto a disk tree, re-ingest and compare the root key (`TestApply_CloneThenUpdateReproducesTrees`). Error texts for unsafe paths (`refusing unsafe path "a//x"`), a symlinked ancestor, and an ancestor that is a file.
13. **xattr encodings**: `{user.wc:"1"}` = `a147757365722e77634131`, plus encodings of 256 bytes (inline) and 257 bytes (spill key).
14. **TicketFromView**: views with 0, 1, 4 and 5 nodes, with nil and non-nil `Addrs` → `Encode()` string. Share this with the ticket spec.
15. **CLI texts**: every printf in §2.10 with fixed keys and stats, the conflict listing, and the `dstore: <err>` lines for each sentinel.
16. **Cross-implementation check**:
    - Go `dstore status` on a copy created by the Rust `clone`, and the reverse (same `config`/`state` bytes, `nothing to push`);
    - one tool holding the lock while the other reports `packstore: … is already open`;
    - a Rust push followed by a Go pull, and the reverse.

---

## 6. Go tests worth porting

- `worktree/tree_test.go`: `TestCreateOpenRoundTrip`, `TestFindOutsideWorkingCopy`, `TestRemove`.
- `worktree/change_test.go`: `TestDiffTrees_EveryKind`, `TestDiffTrees_PrunesEqualSubtrees` (exactly 2 getter calls), `TestCompare`.
- `worktree/scan_test.go`: `TestScan_CleanTreeHasNoChanges`, `TestScan_EveryKind`, `TestScan_IgnoredBasePathIsDeleted`, `TestScan_RacyMtime`, `TestScan_TypeChangeExpands`, `TestScan_Xattr` (skip on ENOTSUP).
- `worktree/merge_test.go`: `TestMerge`, all 13 table rows.
- `worktree/apply_test.go`: `TestApply_CloneThenUpdateReproducesTrees` (includes the re-apply no-op and the read-only `ro` dir), `TestApply_KeepsNonEmptyDirectoryOnDelete`, `TestApply_RefusesUnsafePaths`, `TestApply_RefusesSymlinkedAncestor`.
- `worktree/diff_test.go`: `TestUnified_TreeToTree`, `TestUnified_TreeToDisk`, `TestStat`.
- `cmd/dstore/wc_test.go`: `TestResolveTicket`.
- `node/worktree_test.go` (end-to-end; needs a cluster harness or a live Go cluster):
  - `TestWorktreeInitPushCloneEditPull`
  - `TestWorktreeConflict`
  - `TestWorktreePushRecoversAfterLostState`
- go-udiff:
  - `difftest.TestVerifyUnified` (the `TestCases` with `NoDiff == false`, rendered via `Lines`)
  - `diff_test.go`: `TestToUnified`, `TestLineEdits`, `TestRegressionOld001`, `TestRegressionOld002`, `TestNEdits`, `TestNRandom` (as property tests: apply the edits and get the new text)
  - `lcs/old_test.go`: `TestAlgosOld` (twosided part), `TestIntOld`, `TestSpecialOld` (exercises `fix` with limit 4), `TestRegressionOld001`/`002`/`003`, `TestRandOld`
  - `lcs/common_test.go`: `TestLcsFix`
- Go `sort` package: port `TestSortLarge_Random`-style property tests for the pdqsort port. Correctness is not enough on its own, so pair them with the vectors of §5.6.

---

## 7. Gaps in core-rs or Rust iroh, with workarounds

1. **`ingest::xattrs::read_xattrs` is `pub(crate)`** (`src/ingest/xattrs.rs` 29).
   The scan must read xattrs exactly as ingest does. Workaround: duplicate it in
   `worktree::xattr` with the same `xattr` crate calls (macOS `list_deref`/`get_deref`,
   Linux `list`/`get`), ENOTSUP/EOPNOTSUPP from list meaning none, and `get → None`
   becoming ENOATTR. Longer term, upstream a `pub` re-export.
2. **`ingest::meta::{entry_meta, device_numbers, S_IF*}` are `pub(crate)`** (`src/ingest/meta.rs`).
   Duplicate them, including the wrapping mtime arithmetic and the per-platform Major/Minor formulas. `Mkdev` does not exist anywhere in core-rs; add it.
3. **`ingest::driver::read_dir_sorted` is `pub(crate)`** (85-106). Duplicate it: bytewise sort, `is_dir` from `DirEntry::file_type()`.
4. **Error text.** core-rs wraps io errors with Rust's Display:
   - `ingest::Error::Io` renders `"{op} {path}: {io::Error}"`, i.e. `… : Permission denied (os error 13)`;
   - `packstore::Error::Other` gives `packstore: … is already open: Resource temporarily unavailable (os error 35)`.
   Go prints `permission denied` and `resource temporarily unavailable`. Workaround:
   a `gocompat::errno` table (Go `syscall` errno strings for darwin and linux) and
   a renderer that walks error sources and replaces `io::Error` Display with the
   Go text. The alternative is matching `raw_os_error()` when building dstore
   errors. This gap is shared with every area that prints OS errors.
5. **`amberignore::Matcher::root` returns a bare `io::Error`** with no path.
   Go's `worktree.Scan` returns the `os.ReadFile` PathError. Wrap it as
   `open <dir>/.amberignore: <errno>`, the way core-rs's `ingest::Error::ignore_load` does.
6. **No public Go `%q` implementation**. `tarexport::GoQuote` is `pub(crate)` and
   best-effort for non-ASCII. Implement a full `strconv.Quote` port, with
   `unicode.IsPrint` tables for non-ASCII runes, in `gocompat::quote`.
7. **No Go-compatible JSON, RFC3339Nano or pdqsort code in core-rs**. Implement them in `gocompat` (§2.12, §3.9).
8. **`reference::validate_name`/`validate_user` take `&str`**. Go accepts byte
   strings and reports `must be valid UTF-8`. Check UTF-8 first and emit Go's text
   (`reference name must be valid UTF-8` / `user must be valid UTF-8`).
9. **Stats naming**: `packstore::WriteStats{stored, deduped, bytes_stored}` against Go's `Stored/Deduped/BytesStored`. Not printed by the working-copy commands, so no impact.
10. **Rust iroh**: no direct gap. The worktree depends only on the client port (`ref_get`, `pull_tree`, `push`, `view`).
    The `cluster ticket --store` path needs the node's persisted view (Pebble) with no Rust reader (§2.11, §8).

## 8. Risks and open decisions

**Risks**
- **Diff bytes.** Anything other than a line-for-line port of go-udiff's `twosided` (limit 50), `fix` and Go's pdqsort changes hunks. It was measured at 5% of heavy random edits (§3.9). Gate it with ≥3000 Go-generated vectors.
- **JSON bytes.** `serde_json` does not HTML-escape `<>&`, does not escape U+2028/U+2029, and uses a different invalid-UTF-8 rule. The config and state files would then differ from Go's for names with `&` or users like `Name <mail>`.
- **Error text** is pervasive: Go PathError `op path: errno` against Rust io errors (§7.4). Many CLI failure messages are otherwise mismatched.
- **`$PWD` rule of `os.Getwd`**: without it, `Root` in messages and `filterPaths` decisions differ under symlinked directories.
- **Walk order against bytewise order**: status, diff and `Applied` follow walk order, while `Apply` sorts bytewise. Don't unify them.
- **Racy-clean rule** relies on nanosecond mtimes. Both implementations use `st_mtim`, so this is fine. `diff --remote` uses `time.Now()`, so its behaviour is time-dependent; test it with a controllable clock.
- **Partial application.** A failed `Apply` in `pull` does not move `Base`, and a failed `clone` into a pre-existing empty directory leaves applied files behind. Replicate this; don't "fix" it.
- **Xattrs are never removed by `Apply`**, and `.dstore-tmp-*` leftovers are ingested by push. These are Go quirks to keep.
- **SIGINT semantics**: Go ignores SIGINT during `Apply` (the context is not observed) but aborts transfers. `status` and `diff` die by signal. The Rust signal handling must match (§4.2).
- **`os/user.Current`**: Linux builds without cgo (the Dockerfile) use `/etc/passwd` then `$USER`/`$HOME`; macOS always uses `getpwuid_r`.
  Using `getpwuid_r(getuid())` with `$USER` as a fallback matches both except in
  pathological environments.
- **Packstore compression bytes differ** (zstd, per the core-rs contract). The lock and record formats interoperate, so two tools can share one working copy sequentially.
- **JSON reading leniency**: Go accepts case-folded keys, duplicates, `null`, and invalid UTF-8 in strings. A strict Rust reader could reject a Go-accepted file. Edited files are the only realistic case.
- **Blocking file-system work inside tokio** must use `spawn_blocking` (or run before or after the runtime), otherwise it can starve the client's transport tasks.

**Open decisions**
1. Store `Config` string fields and `Change.path` as bytes (recommended, faithful to Go), or as `String`, rejecting non-UTF-8 arguments with a Rust-only error?
2. Duplicate the core-rs `pub(crate)` helpers (xattrs, meta, read_dir_sorted), or upstream `pub` exports in core-rs first?
3. Where do `gocompat` (errno table, `%q`, JSON, RFC3339Nano, getwd, pdqsort) and the go-udiff port live: private modules of this crate, or a small shared crate?
4. JSON reader: a hand-written Go-v1-compatible parser (recommended), or `serde_json::Value` with case-insensitive post-matching (it differs on invalid UTF-8 and on some error texts)?
5. `dstore cluster ticket --store DIR` (and every `--store`-derived ticket) needs the node's Pebble-persisted view: `node.OpenOffline` → `View()` → `TicketFromView`. Choose between not supporting it in the Rust CLI (a clear error), or reading the view some other way. Pebble has no Rust implementation.
6. Keep Go's unbuffered, partial-output-then-error streaming for `diff` (recommended), or buffer?
7. How golden vectors are regenerated and committed: a Go generator pinned to dstore v0.1.9, core v0.0.8 and go-udiff v0.4.1, like core-rs's `tools/vectorgen`.


---

## Addenda (synthesis)

Added by the architecture synthesis. `PORTING.md` is normative where it differs from this spec.

1. **Crates.** `dstore-worktree` holds this spec's modules; `dstore-udiff` holds the go-udiff port
   and `gosort` (the Go pdqsort port). The Go stdlib behaviour of §2.12 lives in `dstore-gocompat`:
   `errno`, `quote`, `json`, `time` (RFC3339Nano), `os` (getwd, mkdir_all, remove, create_temp,
   write_file, current_username), `path`, `hex`. Signatures are in PORTING.md §4.9-§4.10: flows take
   `&Ctx`, paths are `&[u8]`, `State.synced_at` is `gocompat::time::GoTime`.
2. **Superseded.** §4.1 "`src/bin/dstore/wc.rs` (clap 4.6)" becomes `dstore-cli::cmd_wc` on the
   urfave-compatible `dstore-gocli` framework (PORTING C8).
3. **`TicketFromView`** lives in `dstore-view`; the worktree crate re-exports it as
   `dstore_worktree::ticket_from_view`.
4. **Open decisions resolved.**
   1. Bytes for `Config` fields and `Change.path`.
   2. Duplicate the core-rs `pub(crate)` helpers in `dstore_worktree::sys` and add `mkdev`; propose
      public exports upstream later.
   3. The `gocompat` and `udiff` crates.
   4. A hand-written Go-v1 JSON reader (`gocompat::json::unmarshal_object`).
   5. `cluster ticket --store` per PORTING §2.2 B.
   6. Unbuffered partial output kept (flush per change).
   7. Generator families `worktree` and `udiff` in `tools/vectorgen`.
5. **Signals.** clone, init, fetch, pull and push use `dstore_cli::common::signal_ctx()`. `status` and
   `diff` register nothing. `apply` and `scan` are not abortable (PORTING §5.4).
6. **Error type.** `Error` has sentinel variants with the verbatim texts, plus `Msg`/`Wrapped` for
   formatted Go wraps (PORTING.md §4.10). `is_conflict()` replaces `errors.Is(err, ErrConflict)`.
