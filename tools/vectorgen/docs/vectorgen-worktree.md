# Working-copy vectors (family `worktree`)

Owner: vectorgen-worktree. Generator: `tools/vectorgen/family_worktree.go`, which registers the family
`worktree`. Helper programs: `tools/vectorgen/cmd/mktree` and `tools/vectorgen/cmd/treekey`. Specs:
port-notes/worktree.md §5 (items 1-4, 8-15) and port-notes/verification.md §4.3 items 13-18 and 22.

Every value comes from the real Go code: `github.com/amber-store/dstore` v0.1.10 package `worktree` over
`github.com/amber-store/core` v0.0.9, run through temporary directories and packstores where the Go API
needs them. The conventions of the root `VECTORS.md` apply: 64-bit integers are decimal strings, bytes are
lowercase hex, and a Go string that is not valid UTF-8 goes into a field whose name ends in `_hex`. When a
text field comes in two forms (`path` / `path_hex`), exactly one of them is present. Before writing a file the
generator walks the whole value and fails on any other string that is not valid UTF-8, because `encoding/json`
would silently replace its invalid bytes with U+FFFD.

```sh
nix develop -c go -C tools/vectorgen run . ../../tests/golden worktree
```

The family owns `worktree/config.json`, `worktree/state.json`, `worktree/trees/`, `worktree/diff_trees.json`,
`worktree/merge.json`, `worktree/unified.json`, `worktree/cli.json` and `errors/worktree_text.json`. Two runs
produce identical bytes. Nothing recorded depends on the clock, the user or the machine: scan outputs leave out
ownership, mtimes and symlink permission bits. So far the output was generated on macOS only; the CI
`vectors` job regenerates it on Linux.

## Before generating: the self-check

`cmd/dstore` is `package main`, so vectorgen holds verbatim copies of `describeChange`, `resolveTicket` and
`filterPaths` (`wtDescribeChange`, `wtResolveTicket`, `wtFilterPaths` in `family_worktree.go`). Before
generating, the family finds the dstore module directory (build info plus `go env GOMODCACHE`, falling back to
`go list -m`). It parses `cmd/dstore/wc.go` and its own source (embedded with `//go:embed`), prints each
function with `go/printer` under the original name and without doc comments, and fails when the copy and the
original differ. It also requires every format string the vectors use to be a string literal of
`cmd/dstore/wc.go` (the `printf` cases, the `cmd/` error texts) or of `worktree/flow.go` (`%w (%v)`,
`%w: %s`).

## Shared shapes

**Entry** (`fstree.Entry`):

```json
{ "name": "x" | "name_hex": "…", "mode": "33188", "uid": "1000", "gid": "1000", "mtime": "1600000001000000000",
  "content_key": "<hex>", "link_target": "t1" | "link_target_hex": "…", "rdev": ["1", "3"],
  "xattrs_in": "<hex>", "xattrs_key": "<hex>" }
```

`content_key`, `link_target`, `rdev`, `xattrs_in` and `xattrs_key` are omitted when empty. In CBOR they are
`omitempty`, so nil and empty are the same entry (core-rs: an empty `Vec`).

**Change** (`worktree.Change`):

```json
{ "path": "sub/x" | "path_hex": "…", "kind": "new", "old": Entry | null, "new": Entry | null }
```

`kind` is `Kind.String()`: `new`, `deleted`, `modified`, `type`, `mode` or `meta`. `old` is null for `new` and
`new` is null for `deleted`. A change list is `null` when Go returned a nil slice.

**{ROOT}.** Texts that embed a temporary directory have it replaced by `{ROOT}`. The directory is the
absolute path the Go call received, before symlink resolution (`os.MkdirTemp`, except `filter_paths`, whose
root has its symlinks resolved). A Rust test substitutes its own temporary root.

## `worktree/config.json`

```json
{
  "encode": [ { "name": "probe", "config": Config, "file": "{\n  \"ticket\": \"dstore1abc\",\n …}\n" } ],
  "decode": [ { "name": "case-insensitive-keys", "file": "…" | "file_hex": "…", "ok": true, "config": Config,
                "error": "json: …" } ]
}
```

`Config` is `{ "ticket"|"ticket_hex", "name"|"name_hex", "relay"|"relay_hex", "no_relay": bool,
"no_discovery": bool, "user"|"user_hex" }`, every string field in exactly one form.

- **encode.** `worktree.Create(tmp, config)`, then the bytes of `.dstore/config`: `json.MarshalIndent` with
  two spaces, plus `"\n"`. The generator checks that `SaveConfig` writes the same bytes and leaves no
  `config.tmp`. The cases cover the two samples of worktree.md §3.2, the probe of verification.md §3.3, all
  fields, omitempty, HTML escaping of `<>&`, every control byte, `\x7f`, U+2028/U+2029, invalid UTF-8 (a lone
  `\xff`, a truncated sequence, a surrogate, an overlong form; each invalid byte becomes `�`), and
  quote/backslash.
- **decode.** `json.Unmarshal(file, &worktree.Config{})`, as `openRaw` step 3 does. `config` is the value
  after `Unmarshal` from the zero Config, also when `error` is set: a type error keeps the fields decoded
  before and after it, and a syntax error leaves the zero Config. `error` is Go's text. `Open` wraps it as
  `working copy <root>: bad config: <error>` (see `errors/worktree_text.json`). The cases cover case-folded
  keys (including K/U+212A and S/U+017F), the last duplicate winning, unknown keys, `null`, escapes and lone
  surrogates, invalid UTF-8, type errors (the first one wins), non-object documents, truncation, trailing
  data, a BOM, control bytes and bad escapes.

Rust (worktree-offline, `tests/golden_tests/worktree.rs`, and gocompat-c for `gocompat::json`):
`Tree::create`/`save_config` write `file` exactly, and `unmarshal_object` gives `config` and `error`.

## `worktree/state.json`

```json
{
  "empty_tree": { "key": "2001bbe6…f36b", "bytes": "80", "short": "2001bbe6a9f5a014" },
  "commit": { "key": "5048b642…0329", "bytes": "a5005820…", "short": "5048b642d37e2772" },
  "encode": [ { "name": "probe", "state": State, "file": "{\n  \"base\": …}\n" } ],
  "decode": [ { "name": "offset-plus-0200", "setup": "file", "file": "…" | "file_hex": "…", "ok": true,
                "state": State | null, "error": "bad state file: …" } ]
}
```

`State` is:

```json
{ "base": "<64 hex>", "remote": "<64 hex>", "remote_commit": "<64 hex>", "has_remote": true,
  "is_branch": false, "remote_key": "<64 hex>", "remote_version": "010203" | "" | null,
  "synced_at_unix": "1700000000", "synced_at_nsec": 5, "synced_at_offset_secs": 0 }
```

- `remote` is the zero key when `has_remote` is false.
- `remote_commit` (dstore v0.1.10) is the commit a branch names, and the zero key otherwise. `is_branch` and
  `remote_key` are `State.IsBranch()` and `State.RemoteKey()` of that state.
- `remote_version` is null for Go nil and `""` for an empty non-nil slice. A decoded state without a remote
  has nil; a remote with `"remote_version": ""` decodes to empty.
- `synced_at_unix`/`synced_at_nsec` give the instant.
- `synced_at_offset_secs` is the zone offset of the Go time value. It is 0 for the encode inputs except
  `fixed-zone-instant` (19800, written as UTC anyway). On decode it is the offset `synced_at` carried.

Go:
- **empty_tree.** `worktree.EmptyTree()`.
- **commit.** The commit of dstore's own tests, which the branch cases use: `commit.Commit{Tree: empty,
  Author: id, Committer: id}.Object()` with `id = commit.Identity{Name: "tester", When: 1}` (core v0.0.9).
- **encode.** `worktree.Create` in a temporary directory, set `Tree.State`, `SaveState()`, read
  `.dstore/state` (no `state.tmp` left). The cases:
  - with and without a remote; a state without a remote but with remote fields set, which are not written;
  - an empty and a nil version;
  - a branch (`remote_commit` written between `remote` and `remote_version`), and the states that are no
    branch, whose `remote_commit` is not written: a `RemoteCommit` that is a Blob or a tree key, and one
    without a remote;
  - RFC3339Nano fractions 0, 100, 5, 120000000, 123456789 and 999999999 ns (trailing zeros trimmed);
  - pre-1970 instants, year 0 and year 10000.
- **decode.** `worktree.Create`, `Close`, then `.dstore/state` is laid out per `setup`: `file` holds the
  bytes, `missing` has no state file, `directory` makes it a directory. Then `worktree.Open(root)` gives
  `state` or `error`. The cases:
  - accepted: upper-case hex, `+02:00`/`-03:30`/`-00:00` offsets, 1- and 12-digit fractions, a comma
    fraction, case-folded, duplicate, unknown and `null` keys;
  - `remote_commit`: a branch (compact, indented, upper-case hex, case-folded key); empty and `null`; ignored
    unread without a remote; refused when it is not hex, not 32 bytes, a reserved type, a number, or a key
    of another type (`bad state file: remote_commit <key> is a DirLeaf`); checked after `remote` and before
    `remote_version`. A commit key is accepted as `base` and as `remote` (no type check there);
  - rejected: every `bad state file: …` path of `loadState`, with Go's texts for `encoding/hex`, `key.Parse`
    and `time.Parse`. This includes lower-case `t`/`z` (rejected), hour 24, February 30, year 10000, and
    the order of the checks;
  - `missing` gives `ErrIncomplete`; `directory` gives the raw read error with `{ROOT}`.

Rust (worktree-offline): `Tree::save_state` writes `file`; `Tree::open` (or the state loader) gives `state` or
`error`; `empty_tree()` gives `empty_tree`; `State::is_branch` and `State::remote_key` give `is_branch` and
`remote_key`; core-rs `Commit::object` gives `commit`.

## `worktree/trees/`

`objects.bin`: for each object in ascending key order, `key (32 bytes) ‖ big-endian u64 length ‖ bytes`
(the core-rs `fstree/objects.bin` layout). `trees.json`:

```json
{
  "objects": [ { "key": "<64 hex>", "size": 123 } ],
  "omitted": [ "<64 hex>" ],
  "trees": [ { "name": "diff-a", "root": "<64 hex>", "doc": "…" } ],
  "xattrs": [ { "name": "user-wc", "attrs": [ { "name": "user.wc" | "name_hex": "…", "value": "31" } ],
                "encoded": "a147757365722e77634131", "inline": true, "xattr_set_key": "<64 hex>" } ]
}
```

The trees are built in memory (no filesystem) through the public core v0.0.9 builders, exactly as `ingest`
builds them from disk:
- file content goes through `chunkers.SplitBytes` with the default sizes, then `fstree.EncodeBlob`, then
  `fstree.NewFileIndexBuilder(chunkers.NewItemChunker(7))`; an empty file is one empty Blob;
- a directory's entries, sorted bytewise, go through `fstree.NewDirBuilder`;
- xattrs stay inline when `len(cborx.EncodeXattrs(m)) <= 256`, else an `XattrSet` object is referenced by key 9.

Unless a tree says otherwise, uid and gid are 1000, symlinks have mode `0o120777`, and mtimes are
`1600000000 + i` seconds. `objects` lists every stored object. `omitted` lists objects some tree references
that are deliberately absent from `objects.bin`. A getter over `objects.bin` returns Go's
`packstore: object not found` for an absent key, as `Tree.Get` does.

| tree | content |
|---|---|
| `empty` | the empty directory (`worktree.EmptyTree`) |
| `diff-a`, `diff-b` | the `diffFixtures` of `worktree/diff_test.go`: edit, binary edit, add, delete, chmod, retargeted link, directory chmod |
| `every-a`, `every-b` | `TestDiffTrees_EveryKind`, with mtime-only changes on `same.txt` and `sub/deep.txt` |
| `prune-a`, `prune-b` | `TestDiffTrees_PrunesEqualSubtrees` |
| `walk-a`, `walk-b` | `a`, `a/x`, `a/y/z`, `a-b`, `a.txt` (walk order is not bytewise path order), names with a space, a tab and `é` |
| `nonutf8-a`, `nonutf8-b` | invalid UTF-8 content, and a name with an `\xff` byte |
| `special-a`, `special-b` | device rdev change, block device, fifo→symlink, added fifo, socket, inline and spilled xattr changes (meta), xattrs of exactly 256 (inline) and 257 (spilled) encoded bytes, symlink→file with equal bytes, mode+content change, directory chmod, `--x`/`++y` lines, missing final newline, unknown type `0o160000` (mode change), an added mode-0 entry, mtime-only and owner-only changes, added and deleted empty files, CRLF lines, NUL at offset 8191 (binary) and 8192 (text), file→directory and directory→file with children, a deleted nested directory, setuid cleared |
| `big-a`, `big-b` | MaxDiffBytes boundaries: `exact16` (16777216 bytes, one line changed: diffed), `over16` (16777217 bytes: binary), `grow` (16777216 → 16777217 bytes: binary) |
| `wide-a`, `wide-b` | 400 files, so the directory is a DirNode over several DirLeaves; two edits, a delete and an add |
| `missing-content-a`, `missing-content-b` | `edit.txt`'s new content object is omitted |
| `broken-a`, `broken-b` | `sub`'s directory object in B is omitted |
| `badkey-a`, `badkey-b` | the same directory, with a valid and with a 3-byte content key |
| `badkey-add` | an added directory with a 3-byte content key |
| `notdir` | a Blob key used as a tree root |

The big fixtures repeat a 64-byte line, so their chunks deduplicate: `objects.bin` is about 2.2 MiB and
compresses to a few kilobytes in git.

`xattrs` locks `cborx.EncodeXattrs` (item 13): `{user.wc: "1"}`, an empty value, key order by encoded bytes
(the shorter key first), 256 bytes (inline), 257 and 400 bytes (spilled), and a non-UTF-8 name.
`xattr_set_key` is always the `fstree.EncodeXattrSet` key.

Rust: load `objects.bin` into a map, which a `Getter` reads. Tree roots come from `trees.json`.

## `worktree/diff_trees.json`

```json
{
  "diff_trees": [ { "name": "prunes-equal-subtrees", "a": "prune-a", "b": "prune-b", "get_calls": 2,
                    "changes": [Change] | null, "error": "…" } ],
  "scan": [ { "name": "every-kind", "doc": "…", "setup": [Op], "base_key": "<hex>",
              "base_objects": [ { "key": "<hex>", "bytes": "<hex>" } ], "edit": [Op],
              "synced_at_unix": "1700000000", "synced_at_nsec": 0, "jobs": 2,
              "changes": [ScanChange] | null, "error": "…" } ]
}
```

**diff_trees.** `worktree.DiffTrees(getter, root(a), root(b))` over `objects.bin`. `get_calls` counts every
getter call, failed ones included: identical roots make 0 and the pruning case makes 2. `changes` is the full
list in walk order, and `error` is Go's text. The cases:
- `diff-a`→`diff-b`, its reverse, the clone (`empty`→`diff-a`), delete-all (`diff-a`→`empty`) and identical roots;
- `every-kind`, `prunes-equal-subtrees`, `walk-order`, `non-utf8`, `big`, `wide`, `missing-content` (DiffTrees
  never reads file content, so this pair succeeds);
- `special`, its reverse and its clone;
- missing objects: `fstree: reading <key>: packstore: object not found`;
- a non-directory root: `fstree: <key> is not a directory object (type Blob)`;
- 3-byte directory content keys: `key: data is not 32 bytes: got 3`.

**scan.** A disk script runs in a fresh temporary root:
1. `setup` runs.
2. The base is `ingest.Dir(store, root, Opts{Jobs: 2})` with no exclusions, as `scan_test.go` does. When
   `base_key` is present, `base_objects` are put into the store instead and nothing is ingested.
3. `edit` runs.
4. `worktree.Scan(root, base, store.Get, synced_at, jobs)` gives `changes` or `error`.

The store lives outside the root. `ScanChange` is:

```json
{ "path": "newdir/c.txt" | "path_hex": "…", "kind": "new", "old_mode": "u64" | null, "new_mode": "u64" | null }
```

Symlink modes carry only `S_IFLNK` (`"40960"`), because symlink permission bits differ between macOS and Linux.
Ownership, mtimes and keys are not recorded: they depend on the machine and the clock. For the same reason, a
directory an edit touches gets a new mtime and comes out `meta`.

Ops (`mode` is a JSON number of permission bits; `path` is a slash path under the root):

| op | effect |
|---|---|
| `dir` | `mkdir(path, 0o700)`, then `chmod(mode)`; the parent must exist |
| `file` | write `text` (a string) or `content` (a Payload) with `O_CREAT\|O_TRUNC` and mode 0o600, then `chmod(mode)` |
| `symlink` | `symlink(target, path)` |
| `fifo` | `mkfifo(path, 0o600)`, then `chmod(mode)` |
| `remove` | remove `path` recursively |
| `chmod` | `chmod(path, mode)` |
| `mtime` | set atime and mtime to `mtime_unix` s + `mtime_nsec` ns (never used on symlinks) |
| `truncate` | truncate `path` to `size` bytes |

The scan cases are:
- the `scan_test.go` tests: clean, every kind, an ignored base path is deleted, type changes expand;
- a `.amberignore` in a subdirectory;
- the racy-clean rule: a base mtime of `synced_at - 2s` is not hashed; `synced_at - 2s + 1ns` is hashed;
- a same-mtime size change;
- walk order;
- a `.dstore` in the base root (deleted) and a nested `.dstore` (data);
- a directory-only pattern against a symlink to a directory;
- an added fifo with a directory chmod;
- errors: `.amberignore` is a directory, the base object is missing, a base file has a 3-byte content key.

Rust (worktree-offline): `diff_trees` with a counting getter; `scan` after `amber_store_core::ingest::dir`,
which gives Go's base key on the same machine.

## `worktree/merge.json`

```json
{ "cases": [ { "name": "remote retypes dir with local edits below", "local": [Change] | null,
               "incoming": [Change] | null, "apply": [ { "path"|"path_hex", "kind" } ] | null,
               "conflicts": [ { "path"|"path_hex", "local_path"|"local_path_hex", "local_kind", "incoming_kind" } ] | null } ] }
```

`worktree.Merge(local, incoming)`. `apply` and `conflicts` keep incoming order. A conflict's `local_path` is
the local change's path: the path itself, the ancestor that `goneAbove` found, or the first change below.
The first 13 cases are `TestMerge` verbatim; like the Go test, their content keys are not canonical keys, and
Merge never parses them. The further cases:
- the ancestor walk skipping an intermediate mode change; a deep deleted ancestor; an ancestor meta change;
- both sides adding the same directory with equal and with different permissions;
- the same file content with a different mtime on both sides, and different files;
- chmod against edit in both directions; the same chmod twice; chmod over a meta change;
- equivalent retypes; meta change against delete in both directions;
- `firstBelow` with `d-x` and `d0` around `d/`; a retype file→dir not looked up below;
- the `i > 0` bound of `goneAbove` with a leading slash;
- incoming order; non-UTF-8 paths.

Rust (worktree-offline): `merge` gives exactly `apply` and `conflicts`.

## `worktree/unified.json`

```json
{
  "cases": [ { "name": "special", "a": "special-a", "b": "special-b", "unified": "diff a/crlf b/crlf\n…" | "unified_hex": "…",
               "unified_error": "…", "stat": " crlf | +1 -1\n…" | "stat_hex": "…", "stat_error": "…" } ],
  "disk": [ { "name": "tree-to-disk", "doc": "…", "setup": [Op], "edit": [Op], "after_scan": [Op],
              "synced_at_unix": "1700000000", "synced_at_nsec": 0, "jobs": 2, "changes": ["modified bin", …],
              "unified": "…", "unified_error": "…", "stat": "…", "stat_error": "…" } ]
}
```

- **cases.** `changes = DiffTrees(getter, root(a), root(b))`, which never fails for these pairs. Then
  `Unified(&buf, changes, TreeSource{getter}, TreeSource{getter})`, and separately
  `Stat(&buf, changes, …)` on a fresh buffer. `unified`/`stat` are the bytes written, including the partial
  output before an error; `unified_error`/`stat_error` are Go's texts. This locks:
  - the diff header and mode lines (also for type changes);
  - `Binary files … differ` for NUL bytes and for over-16-MiB content;
  - headers without a body for equal bytes (devices, fifos, empty files, unknown types, symlink→file with equal
    bytes, mode-only changes);
  - `--stat`'s miss of `---x`/`+++y` lines;
  - hunks in a 16-MiB file;
  - the error after a header for a missing content object.
- **disk.** The scan procedure of `diff_trees.json` with `synced_at` 1700000000 and jobs 2. Then `after_scan`
  runs, and Unified and Stat render with `TreeSource{store.Get}` as old and `DiskSource{root}` as new.
  `changes` is the scan result as `"<kind> <path>"` strings, a quick check before comparing renders. The cases:
  - `TestUnified_TreeToDisk`;
  - DiskSource size checks: 16777217 bytes gives binary without reading; exactly 16777216 bytes of zeros is
    read and binary;
  - headers only;
  - a file and a symlink removed between Scan and Unified: the header is written, then
    `new.txt: lstat {ROOT}/new.txt: …` or `link: readlink {ROOT}/link: …`.

Rust (worktree-offline): `unified` and `stat` write the same bytes and return the same error texts.

## `worktree/cli.json`

```json
{
  "kind_string": [ { "kind": 6, "out": "Kind(6)" } ],
  "type_name": [ { "mode": "57600", "out": "type 0160000" } ],
  "describe_change": [ { "name": "mode directory", "change": Change, "out": "sub/ (0755 → 0700)" | "out_hex": "…" } ],
  "status_row": [ { "name": "mode directory", "change": Change, "out": "  mode      sub/ (0755 → 0700)\n" } ],
  "resolve_ticket": [ { "flag": "", "stored": "s", "env": "e", "ok": true, "out": "s", "error": "…" } ],
  "filter_paths": [ { "name": "outside", "cwd": ".", "args": ["../outside"], "changes": ["..foo", …],
                      "kept": ["…"] | null, "error": "../outside is outside the working copy" } ],
  "fetched_desc": [ { "name": "branch", "key": "<64 hex>", "tree": "<64 hex>", "out": "commit 5048…, root 2001…" } ],
  "pushed_key": [ { "name": "branch", "root": "<64 hex>", "commit": "<64 hex>", "out": "<64 hex>" } ],
  "printf": [ { "name": "push", "stream": "stdout", "func": "Printf", "format": "pushed %s: …\n",
                "args": [ { "type": "string" | "int" | "bytes" | "kind", "value": "…" } ], "out": "…" } ],
  "ticket_from_view": [ { "name": "five-nodes-capped-at-four", "cluster_id": "<hex>", "incarnation": "u64",
                          "nodes": [ { "id": "<hex>", "addrs": ["ip:127.0.0.1:4433", …] | null } ] | null,
                          "ticket": "dstore1…" } ]
}
```

- **kind_string.** `worktree.Kind(k).String()` for 0-6 and -1.
- **type_name.** `worktree.TypeName(mode)` for every file type, 0, bare permissions, unknown types (Go
  `%#o`) and high bits.
- **describe_change.** The verbatim `describeChange` copy. Only the `mode` fields of the entries matter.
  Directories get a trailing slash; the code gives `a.txt/ (file → directory)`.
- **status_row.** `fmt.Sprintf("  %-9s %s\n", kind, describeChange(change))`, the row of `dstore status`.
- **resolve_ticket.** The verbatim `resolveTicket` copy (`TestResolveTicket` plus the error).
- **filter_paths.** The verbatim `filterPaths` copy. Each case runs with the process working directory at
  `cwd` (relative to the root) inside a temporary working-copy root whose symlinks are resolved, over changes
  at `changes` (all `new` files). `{ROOT}` in `args` and `error` stands for the root. `kept` lists the kept
  paths, and is null when Go returned nil.
- **fetched_desc**, **pushed_key.** The verbatim `fetchedDesc` and `pushedKey` copies (dstore v0.1.10) over a
  `worktree.FetchResult{Key, Tree}` and a `worktree.PushResult{Root, Commit}`: a tree, a branch, the zero key
  of an absent reference, and keys of other types.
- **printf.** Every stdout/stderr format of the working-copy commands (`cmd/dstore/wc.go`, literals checked by
  the self-check), run with fixed arguments. `func` is `Printf`/`Fprintf`/`Sprintf` (`out = fmt.Sprintf(format,
  args…)`) or `Println`/`Fprintln` (`out = format + "\n"`; an empty `format` is the bare `fmt.Println()` that
  ends the `pulled:` line). `int` values are decimal, `bytes` hex (`%x`), and `kind` a `worktree.Kind`
  printed with `%s`. The `%s` that the clone, init and fetch lines take for what was fetched is
  `fetchedDesc`'s text, once for a tree and once for a branch.
- **ticket_from_view.** `worktree.TicketFromView(&view).Encode()` for 0, 1 (with and without addresses), 4
  and 5 nodes (capped at 4). Node ids are ed25519 public keys of `data(1+i, 32)`, and the cluster id is
  `00…0f`.

Rust: `dstore-cli` `cmd_wc` unit tests (cli-wc) for `describe_change`, `resolve_ticket`, `filter_paths` and the
command output; worktree-offline for `Kind` Display and `type_name`; `dstore-view` for `ticket_from_view`.

## `errors/worktree_text.json`

```json
{ "cases": [ { "name": "tree/open-locked", "out": "packstore: {ROOT}/.dstore/packstore is already open: resource temporarily unavailable" | "out_hex": "…" } ] }
```

Name prefixes:

| prefix | source |
|---|---|
| `worktree/`, `client/` | the sentinel errors (`tree.go`, `flow.go`, `diff.go`; `client.ErrUnknownRef`) |
| `cli-line/` | what `dstore` prints for them on stderr: `"dstore: " + text + "\n"` |
| `flow/` | `Push`'s `%w (%v)` over `*client.CASMismatch` (absent, current, a short current key), `Clone`'s `%w: %s`, and real `Init`/`Clone` failures before the cluster is used (`is not a directory`, `is not empty`, a stat error, inside a working copy) |
| `tree/` | real `Find`, `Open` and `Create` failures: no working copy, `.dstore` a file, no config, config a directory, bad config (syntax and type), packstore path a file, incomplete, locked (the lock is taken before the state check), inside a working copy. `Find` walks every ancestor of the temporary directory, so the generator requires `ErrNotWorkingCopy` (and `ErrIncomplete`) for those cases and fails when a `.dstore` above `$TMPDIR` would change them |
| `apply/` | real `Apply` failures: `empty path`, `refusing unsafe path %q` (including `%q` of invalid UTF-8, tab, quote, backslash, U+00A0, emoji), a symlinked ancestor, a file ancestor, unsupported types `0160000` and `0`, a 3-byte content key, a missing content object, bad inline xattrs, a missing or invalid xattr set key |
| `cmd/` | `cmd/dstore/wc.go` texts: usage errors, `--remote and --incoming exclude each other`, `no cluster: …`, `user: …` over `reference.ValidateUser`, and the `reference.ValidateName` texts |

The Scan, DiffTrees, state-file and diff-rendering errors live with their cases in the files above.

Rust: the Display of `dstore_worktree::Error` (worktree-offline, worktree-flow) and the cli-wc texts,
compared after substituting `{ROOT}`.

## Not generated here

- go-udiff, pdqsort and LCS vectors (worktree.md §5 items 5-7): family `udiff` (`family_udiff.go`) owns
  `udiff/udiff.json`, `udiff/pdqsort.json` and `udiff/lcs.json`.
- `TestApply_CloneThenUpdateReproducesTrees` (item 12) compares re-ingested keys, which depend on the
  runner's uid and gid. It stays a ported Go test; the apply error texts are in `errors/worktree_text.json`.
- `TestScan_Xattr`: whether a temporary directory accepts `user.*` xattrs depends on the file system (tmpfs,
  APFS, policies), so a disk xattr scan would not give the same vector everywhere. It stays a ported Go test
  that skips on `ENOTSUP`. Inline and spilled xattr changes are locked in memory by the `special` fixtures
  and by `xattrs` in `trees.json`.
- The cross-implementation checks (item 16) belong to the live interop harness, which uses `mktree` and
  `treekey`.

## Helper programs

- `go run ./cmd/mktree [-seed S] [-small N] DIR` writes a deterministic source tree into a new or empty `DIR`:
  - splitmix64 files of 0 B, 1 B and 5 MiB, and 300 KiB of `0xaa`;
  - a unicode name, directories nested four deep, `N` small files (default 2000);
  - a symlink, a fifo, a 0o755 script, `.amberignore` with an ignored `*.log` file, and a best-effort
    `user.mktree` xattr.

  Contents depend only on the seed. Every entry gets a fixed mode and mtime, and directories are dated
  after their contents.
- `go run ./cmd/treekey [-exclude NAME]... [-jobs N] [-no-ignore] PATH` prints the root key core ingest
  computes for `PATH` without storing anything (`ingest.Objects`). A working copy needs `-exclude .dstore`.
