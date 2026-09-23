# core v0.0.10: the dstore v0.1.11 delta

Area: what changed for the client between Go dstore v0.1.10 and v0.1.11 (branch `core-v0.0.10`: `1b6583e`
"Follow core v0.0.10: commit footprints, shared local stores", `53b88ae` "Working copies keep their own lock;
ref-put refuses what it cannot read", `6c808de` "ref-put refuses an unreadable commit whichever node
coordinates", and beside them a script, README texts and a test; head `6c808de`), and how dstore-client-rs
follows it. Most of the change is not in dstore: Go core went from v0.0.9 to v0.0.10 and
core-rs from 0.4.0 to 0.7.0, and both changed what a commit's key is, where local references live and who may
open a packstore. For the parts listed here this note decides over the area specs of L0-L6 and over
`commit-objects.md`; everything else in them stands.

Pins after the change (PORTING.md §0, §5.11):

| What | Before | After |
|---|---|---|
| Go dstore | v0.1.10, `7bd788e` | v0.1.11 (the branch above) |
| Go core | v0.0.9 (`e318780`) | v0.0.10 (`9026f42`) |
| core-rs | 0.4.0, `7386914b56b9c43ea42b166d81c1f2fbce3aaffb` | 0.7.0, `1a8bca1079e85713052b99c9bde144625015652c` (tag v0.7.0, at parity with core v0.0.10) |

`git diff v0.1.10 v0.1.11 -- go.mod` moves the core line and adds core's new indirect dependencies (the
modernc SQLite driver and what it needs). Every other pin is unchanged.

## 1. What upstream changed

In dstore:

| Go file | Change | Rust side |
|---|---|---|
| `go.mod` | core v0.0.10 | core-rs 0.7.0 (§2) |
| `client/commit.go` (26 → 38) | `TreeOf` holds the commit to core's key rule before it hands out the tree: `commit.Footprint` over `c.Trees()`, and a mismatch is `commit %s: length field %d is not the commit's footprint %d (its own %d bytes plus its trees); a commit keyed by an older rule has to be created again` | `dstore_client::tree_of`, `TreeOfError::{Footprint, Length}` |
| `worktree/tree.go` | `Tree.lock`, `ErrInUse`, `lockWorkingCopy`: `Open` and `Create` take `.dstore/lock`, an exclusive flock without waiting, and hold it until `Close`. A second command fails with `working copy <root>: in use by another dstore command`. `openRaw` takes the lock before it reads the config, which the holder may rewrite. Until core v0.0.10 the packstore's single-owner lock did this on the side | `dstore_worktree::Tree` (private `lock`), `Error::InUse`, `is_in_use`, `Tree::from_parts` |
| `node/data.go` | `verifyRecord` checks a Commit's length field as its footprint, from the record alone (`length field %d is not the commit's footprint %d; a commit keyed by an older rule has to be created again`); bytes under a Commit key that do not decode are refused. `fetchRecord` answers `refusedError` (`record refused: <verifyRecord's text>`) when a peer holds a record that `verifyRecord` refuses | the fake node only (`dstore_testkit::fake::verify_record`, `get_data`, `DataError`) |
| `node/refs.go` | `walkComplete` ends with `errMalformed` (`malformed object under the reference: <cause>`) on an object it fetched and `ChildKeys` refuses, or that a peer holds and `verifyRecord` refuses; `handleRefPut` answers bad-request with that text, `RefPutLocal` returns it. An interior object the walk could not fetch while its owners report it present ends it with `errUnread` (`object held by its owners could not be read: <8 key bytes, hex>: <error>`), which `handleRefPut` answers as `unavailable`, `completeness walk: …` | the fake node only (`walk_complete`, `WalkFail`, `handle_ref_put`, `local_ref_put`) |
| `client/commit_test.go`, `worktree/lock_test.go`, `node/verify_test.go`, `node/oldrule_test.go` | tests | §6 |
| `README.md`, `architecture/dstore.md` | the lock, a directory entry that holds a commit, upgrading a cluster that holds v0.1.10 branches | README.md "Working copies", "Commits made by dstore v0.1.10" |
| `node/gc.go`, `node/node.go`, `client/fetch.go` (a comment), `scripts/` | not client-visible | — |

Unchanged: `wire`, `codec`, `view`, `ticket`, `placement`, `transport`, the rest of `client` (`VerifyRecord`
too), the working-copy flows (`samePush` still decodes a commit without holding it to the rule), the admin
payloads, the TUI, and all of `cmd/dstore`: every help text and every declaration and test the generator
copies verbatim.

Through core v0.0.10, without a line of dstore changing:

- **Commits.** Optional fields `ChangeID`, `ConflictTerms`, `ConflictLabels`; an identity's name may be
  empty; `Commit.Object()` keys by footprint, the commit's own bytes plus the length field of every tree it
  records (`Trees()`: the tree, then the conflict terms; parents are not counted). `fstree.ChildKeys` of a
  commit is its trees, then its parents, and it refuses a commit whose key is not its footprint, so every
  walk does: push, pull, the completeness checks. `fstree.DirOf`: the directory readers take a commit for
  its tree, as the key they are given and as the content of an `S_IFDIR` entry.
- **Local references.** `refstore` is SQLite, `<dir>/refs.sqlite` in WAL mode, a format shared with
  core-rs. Go imports a Pebble directory of an earlier release on the first open.
- **Packstore.** Any number of processes may have one store open. The directory flock is shared; a
  release from before takes it exclusively and is kept out, and keeps the new one out. New files in a
  store directory: `gc.lock`, `<id>.seg.active.idx`.

## 2. What core-rs 0.7.0 brings, and what the bump required

| core-rs 0.7.0 | Here |
|---|---|
| `Commit` gains `change_id`, `conflict_terms`, `conflict_labels` (no `Default`) | every struct literal: `worktree::flow::commit_object` (dstore records none of them) and the tests |
| `IdentityError::NameEmpty` is gone | nothing: `reference::validate_user` still refuses an empty user before a commit is made, so `commit: commit author: …` stays unreachable from the CLI (commit-objects.md §4 says both refused it) |
| `Commit::object()` keys by footprint; `commit::footprint`, `Commit::trees` | every commit key moved: dstore's test commit is `5049b642…` (its own 72 bytes plus the empty tree's 1; `5048b642…` before). One was hard-coded in `flow.rs` |
| `ChildKeysError::{CommitFootprint, CommitLength}` | two arms in `corefmt::child_keys_error_text`, Go's texts from `fstree/dirof.go` `decodeCommit`. core-rs words them alike; the test compares both |
| `fstree::dir_of`; the readers call it first | `worktree/diff_trees.json` `error-not-a-directory`: one `get` instead of two, because the type is checked before the object is read. No code change |
| (found on the way) | `corefmt::type_name` had no `Commit`: a commit key in `… is not a file-content object (type %v)` printed `Type(5)` since 0.4.0, where Go prints `Commit` |
| `refstore` on SQLite (`rusqlite`, bundled SQLite); `Error` reshaped, with `PebbleStore`, `LegacyInUse`, `Migrate`; a `refs.redb` is imported | `common::open_local` (§4), the flake hash, DD-2 |
| `packstore::Store::open` shares the directory; `… is held by an older release, which needs the store to itself: <errno>` replaces `… is already open: <errno>` | the lock tests, vectors and interop checks (§4); `rewrite_os_errors` handles the new text as it did the old |

The flake: `outputHashes."amber-store-core-0.7.0" = "sha256-tT4gGlJ0ZEGCjJXPvAtJ0T2DIlANe7R6o7bCn3ExjnE="`,
by `nix hash path` over `git archive <rev>` (the method reproduces the 0.4.0 hash), confirmed by
`nix build .#dstore`. The bundled SQLite builds in the Nix sandbox with the C compiler the workspace
already needs.

## 3. Mapping

| Go | Rust |
|---|---|
| `commit.Footprint(uint64(len(data)), c.Trees())` in `TreeOf` | `commit::footprint(data.len() as u64, &c.trees())`; failure `TreeOfError::Footprint` (`commit %s: %w`), mismatch `TreeOfError::Length { key, length, want, own }` |
| `worktree.ErrInUse`, wrapped `working copy %s: %w` | `Error::InUse(root)`; `Error::is_in_use()` for `errors.Is` |
| `lockWorkingCopy(root)` | private `lock_working_copy`: `OpenOptions` read, write, create, mode 0644 on `<root>/.dstore/lock`; `libc::flock(LOCK_EX \| LOCK_NB)`, EINTR retried; `EWOULDBLOCK` → `InUse`; another errno, or the open's own failure (a `*PathError`), → `working copy <root>: <error>` |
| `Tree.lock *os.File` (unexported) | private `lock: Option<File>`. `Tree` can no longer be built by a struct literal outside the crate: `Tree::from_parts` is Go's `&worktree.Tree{Root: …, State: …}`, which the snapshot fixtures use to write a state file, and holds no lock |
| `openRaw`: find, lock, read and parse config, packstore | the same order in `Tree::open_raw`; an error after the lock drops it |
| `Create`: `MkdirAll(.dstore)`, lock, config, packstore, empty tree | the same order in `Tree::create` |
| `Close`: the store, then the lock | `Tree::close`; a tree that is dropped lets go as well |
| node `verifyRecord`, `case key.Commit` | `fake::verify_record`: `commit: <decode error>`, `commit: <footprint error>`, `length field %d is not the commit's footprint %d` |
| node `errMalformed`, `errUnread`, `walkComplete`, `handleRefPut`, `RefPutLocal` | `fake::{MALFORMED, UNREAD}`, `walk_complete` returns `Result<_, WalkFail>`, bad-request or unavailable in `handle_ref_put`, the text as it is from `local_ref_put`. `FakeCluster::plant` puts a record into a node's store unverified (Go tests: `n.Store().Put`) |
| node `fetchRecord`, `getFromChecked`, `refusedError` | `FakeState::get_data`: the fake's nodes share one process, and a record taken from another node's store is verified as one that came over the wire; refused by every owner that has it → `DataError::Refused` |

## 4. Decisions

- **`client.VerifyRecord` is unchanged, so `dstore_client::verify_record` is.** It checks no length field, a
  commit's neither. The vectors say so: `commit` (footprint key), `commit_length_field_off_by_one` (now one
  above the footprint) and the new `commit_keyed_by_own_length` (core v0.0.9's rule) are all accepted. What
  holds a commit to the rule on the client is `tree_of` and every `child_keys` walk.
- **What Go does when a local store is open elsewhere was tried, not assumed.** With the Go CLI and the
  generator's `holdlock` built against core v0.0.10, and a `holdlock` built against core v0.0.9:
  - two processes open one packstore, and `status` runs beside a holder of the *packstore*: nothing at the
    packstore level refuses any more;
  - a core v0.0.9 holder makes the new CLI fail with `dstore: packstore: <dir> is held by an older release,
    which needs the store to itself: resource temporarily unavailable`, exit 1; the other way round the old
    release still says `is already open`;
  - at `1b6583e` that left two commands in one working copy running at once. `53b88ae` closed it with
    `.dstore/lock`: with `worktree.Open` held, `status`, `diff`, `fetch`, `pull` and `push`, from the root or a
    subdirectory, print `dstore: working copy <root>: in use by another dstore command`, exit 1;
  - `store push --local L` leaves `L/packstore/{0000000000000001.seg.active, ….seg.active.idx, gc.lock}` and
    `L/refs/refs.sqlite`.
- **A working copy is locked by `.dstore/lock`, in both implementations.** Go and Rust share working copies,
  so the Rust side takes the same file with the same call. `examples/holdlock.rs` and the generator's
  `cmd/holdlock` now open the working copy (`Tree::open`, `worktree.Open`) instead of its packstore, which
  locks nobody out any more, and print `locked <root>`.
- **A standalone local store is shared, and nothing replaces its old lock**, by design of the new core.
  `open_local` opens twice; its unit test says so.
- **The older release is still a case.** A dstore v0.1.10 command knows no `.dstore/lock` but holds the
  packstore directory's flock exclusively, so a v0.1.11 command gets past its own lock and is refused by
  the packstore. No such binary is at hand in a test, so the tests, the generator
  (`wtHoldAsOlderRelease`) and interop D12 (`perl`) take that flock by hand. The vector is
  `tree/open-held-by-an-older-release`; like the lock, it is reported before the state is read.
- **Fork windows.** The test binaries spawn `dstore` from several threads, and between fork and exec a
  child holds a copy of every descriptor, flocks included. A shared packstore lock is harmless there, so
  the fixture builder's retry around packstore opens went; it stays around `Tree::open` (`retry_in_use`),
  and around taking the exclusive flock by hand.
- **DD-2 is rewritten.** Rust kept references in redb because there is no Rust Pebble; now both keep them
  in `refs.sqlite`, and what is left of DD-2 is one direction of one migration: core-rs cannot import the
  Pebble directory of Go dstore v0.1.10 or earlier, Go v0.1.11 can. core-rs decides what such a directory
  is (`marker.manifest.*` and no `refs.sqlite`; `refstore::Error::PebbleStore`), and `open_local` words the
  refusal for a dstore user: `refstore: <DIR>/refs holds a Pebble database written by Go dstore v0.1.10 or
  earlier; dstore-client-rs cannot import it: open the --local directory once with Go dstore v0.1.11 or
  later, which does`. The old check, which listed `DIR/refs` for Pebble's file names, had to go: Go's
  import leaves the poison marker `marker.format-version.999999.999`, which it would have refused.
- **`refstore: creating <DIR>/refs: …` is rendered here**, like `packstore: creating …`: core-rs creates
  with 0777 and words the errno as Rust does (core-rs-gaps G5, G6).
- **The `pebble_refs` fixture step opens Pebble itself.** It made its directory through core's
  `refstore.Open`, which is SQLite now. It calls `pebble.Open` as core v0.0.9's refstore did (pebble/v2
  v2.1.7 becomes a direct dependency of the generator) and still checks the six file names. Go's run over
  that fixture now imports the store and then fails on the missing ticket, as before.
- **The fake node models fetching, so it mirrors the whole rule.** Its coordinator reads an owner's store
  when it lacks an object, so the second half of Go's fix applies: such a record is verified, a refused one
  is malformed (`… record refused: …`), and an interior object that stays unread while the negotiation
  finds it held fails the put as unavailable. In a steady fake cluster read order and write set name the
  same nodes, so the last case has no test here, as it has none in Go.
- **The fake node words a malformed object as core-rs does.** For a commit that is Go's text. For a
  directory whose entry names need quoting it is core-rs's quoting (core-rs-gaps G4): the testkit cannot use
  `dstore_client::corefmt` (the client's tests depend on the testkit).

## 5. Vectors

Regenerated with the generator at dstore v0.1.11 (§7) and core v0.0.10. Only these files changed:

| File | Change |
|---|---|
| `worktree/state.json` | the test commit's key, `5049b642…`, wherever a case uses it; its bytes are unchanged |
| `worktree/cli.json` | the same key in `fetched_desc`, `pushed_key` and the clone, init, fetch and push lines |
| `worktree/diff_trees.json` | `error-not-a-directory`: `get_calls` 2 → 1 (`DirOf` checks the type first) |
| `client/verify_record.json` | 23 → 24 cases: the commit's key carries its footprint, the off-by-one is one above it, new `commit_keyed_by_own_length` |
| `errors/worktree_text.json` | `tree/open-locked` and `tree/open-locked-before-state-check` are `working copy {ROOT}: in use by another dstore command` (a second `Open`; an `Open` during a `Create`); new `tree/open-locked-before-config-check` (no config, the lock file flocked by hand: the lock is reported), `tree/open-lock-is-a-directory` (`working copy {ROOT}: open {ROOT}/.dstore/lock: is a directory`) and `tree/open-held-by-an-older-release` |
| `cli/snapshots.json` | the generator's module versions, the node-side refusal text (it names v0.1.11) and the DD-2 text. No Go output changed |

`crates/gocompat/src/tables.rs` and `errno_tables.rs` are unchanged (same Go toolchain).

## 6. Tests

| Go | Rust |
|---|---|
| `client/commit_test.go` `TestTreeOf` (a conflicted commit, a key of the older rule) | `crates/client/src/commit.rs` `tree_of_resolves_commits`, with the exact text |
| `worktree/lock_test.go` `TestWorkingCopyTakesOneCommandAtATime` | `crates/worktree/src/tree.rs` `working_copy_takes_one_command_at_a_time`, plus a dropped tree, `the_lock_comes_before_the_config` and `a_copy_held_by_an_older_release_reports_it_before_the_state` |
| `worktree/lock_test.go` `TestWorkingCopyLockHoldsAcrossProcesses` (the holder is a re-executed test process) | `tests/cli_wc.rs` `a_working_copy_in_use_is_refused`: the test process holds the working copy and `dstore` processes are refused, which is the same proof with the roles the CLI has. A re-executed test binary would bring fork windows into the worktree crate's lock tests (§4) |
| `node/verify_test.go` `TestVerifyRecordCommitFootprint` | `crates/testkit/src/fake.rs` `verify_record_accepts_and_rejects_as_the_node` |
| `node/oldrule_test.go` `TestRefPutRefusesACommitOfTheOlderKeyRule`, with its control case and the bad-request code | `tests/fake_cluster.rs` `ref_put_refuses_a_commit_of_the_older_key_rule`, plus `RefPutLocal` |
| `node/oldrule_test.go` `TestRefPutRefusesACommitTheCoordinatorDoesNotHold` | `tests/fake_cluster.rs` `ref_put_refuses_a_commit_the_coordinator_does_not_hold`, with the exact text |
| core `fstree/dirof.go` texts | `crates/client/src/corefmt.rs` `commit_key_rule_errors_match_go` |
| — | `tests/cli_wc.rs` `a_working_copy_in_use_is_refused` (every working-copy command, and from a subdirectory) and `a_working_copy_held_by_an_older_release_is_refused`; `crates/cli/src/common.rs` `open_local_creates_both_stores` (shared; an older release), `open_local_refuses_pebble_refs_and_releases_the_packstore`, `open_local_opens_refs_that_go_imported`, `open_local_refs_errors_have_go_texts` |
| live | `interop/check.sh` D12: a Go holder blocks Rust and a Rust holder blocks Go, same line and status from both; an exclusive flock on the packstore blocks both. G1: Rust refuses Pebble refs; each client runs on the `--local` directory the other wrote, and no second reference store appears |

Not ported: nothing of core's own tests (core-rs ports them). A legacy `refs.redb` and a real Pebble store
are not at hand in a test here; core-rs and core test their imports.

## 7. When the upstream tag does not exist yet

This change was made before Go dstore v0.1.11 was tagged, from a checkout of its branch. The generator
pins Go modules by version and refuses anything else, so:

- For the work, `tools/vectorgen/go.mod` got `github.com/amber-store/dstore v0.1.11`,
  `github.com/amber-store/core v0.0.10` and a temporary `replace github.com/amber-store/dstore => <checkout>`,
  and two self-checks were patched by hand to accept the replace (`clientModuleDir` in `family_client.go`,
  `tspCheckVersions` in `family_transport.go`). Neither the replace, nor the patches, nor a `go.sum` made
  under them is committed: `go.mod` and `go.sum` stay as they were until the tag exists, and until then
  the generator's self-checks fail on purpose.
- `interop/check.sh` takes a prebuilt Go CLI (`DSTORE_GO_BIN`), built from `git archive HEAD` of the
  checkout with `CGO_ENABLED=0 go build -trimpath ./cmd/dstore`.
- Once the tag exists: `go get github.com/amber-store/dstore@v0.1.11 github.com/amber-store/core@v0.0.10`
  and `go mod tidy` in `tools/vectorgen`, then every generation command of VECTORS.md "Regenerating". The
  bytes must be the committed ones (`git status` stays clean under `tests/golden` and
  `crates/gocompat/src`), because a replace changes no vector: the module version in `cli/snapshots.json`
  comes from the build information, where a replaced module keeps the required version.
