# Commit objects: the dstore v0.1.10 delta

Area: what changed on the client side between Go dstore v0.1.9 and v0.1.10 (merge `7bd788e`, change
`e4d783f` "Support commit objects (core v0.0.9)"), and how dstore-client-rs follows it. The area specs of
L0-L6 were written against v0.1.9 and keep their v0.1.9 line numbers. For the parts listed here this note
decides; everything else in them stands, because v0.1.10 does not touch it.

Pins after the change (PORTING.md §0, §5.11):

| What | Before | After |
|---|---|---|
| Go dstore | v0.1.9, `368f2c7` | v0.1.10, `7bd788e` |
| Go core | v0.0.8 | v0.0.9 (`e318780`) |
| core-rs | 0.3.0, `a85ffa1eb5ed363b9072ab224de179196cd0a046` | 0.4.0, `7386914b56b9c43ea42b166d81c1f2fbce3aaffb` (tag v0.4.0, the backport of core PR #12) |

`git diff v0.1.9 v0.1.10 -- go.mod` changes the core line only, so every other pin is unchanged.

## 1. What upstream changed

| Go file | Change | Rust side |
|---|---|---|
| `go.mod` | core v0.0.9: the Commit object, CAS type 5. `fstree.ChildKeys` yields a commit's tree, then its parents, so every graph walk follows history | core-rs 0.4.0 (§2) |
| `client/commit.go` (new, 26 lines) | `TreeOf(k, get)`: a Commit's recorded tree, or `k` itself | `dstore_client::tree_of` |
| `cmd/dstore/client.go` (574 → 580) | `ls` and `cat` resolve the reference's key through `TreeOf` | `cmd_client::ls_with`, `cat_with` |
| `cmd/dstore/wc.go` (501 → 511) | `push --message/-m`; `fetchedDesc`, `pushedKey`; the clone, init, fetch and push lines name the commit on a branch | `app::push_cmd`, `cmd_wc` |
| `worktree/tree.go` (245 → 274) | `State.RemoteCommit`, `IsBranch`, `RemoteKey`; `remote_commit,omitempty` in `.dstore/state` | `dstore_worktree::State` |
| `worktree/flow.go` (331 → 404) | fetch records the commit beside its tree; a push on a branch, or with a message, records a commit; `samePush` | `dstore_worktree::Tree::{fetch, push}` |
| `node/data.go` | `verifyRecord` checks a Commit's length field as its own serialized length | the fake node only (`dstore_testkit::fake`) |
| `client/commit_test.go`, `worktree/tree_test.go`, `node/branch_test.go`, `node/verify_test.go` | tests | §6 |

Unchanged: `wire`, `codec`, `view`, `ticket`, `placement`, `transport`, the rest of `client`, the admin
payloads, the TUI, and every help text except `push`'s.

The model: **a reference that names a commit is a branch.** `base` and `remote` of a working copy stay trees,
so status, diff, merge and pull are unchanged. The commit is recorded beside its tree (`remote_commit`). On a
branch every push wraps the ingested tree in a new commit whose single parent is the fetched commit, with the
user as author and committer and `--message` as its message, and moves the reference to it. A message on a
plain or new reference makes a root commit, and so starts a branch.

## 2. What core-rs 0.4.0 brings

- `amber_store_core::commit`: `Commit`, `Identity`, `Commit::{encode, object, decode, signature_payload}`,
  with Go's error texts byte for byte (core-rs `port-notes/commit.md`: 34 355 differential decode cases).
- `key::Type::Commit = 5`. Types 6..15 stay reserved.
- `fstree::child_keys` follows a commit, with the new variant `ChildKeysError::DecodeCommit`. Everything in
  this workspace that walks a graph goes through `child_keys`: `Cluster::push`'s local walk, `pull_tree`'s
  frontier fetch and its completeness check, and the fake node's completeness check of a `ref-put`. They
  follow history without a code change.
- `packstore` verifies a Commit's length field like a Blob's.

What the bump required here: one new arm in `corefmt::child_keys_error_text` (`fstree: decoding Commit
<key>: <err>`; a commit's error texts hold no `%q` names, so core-rs's text is Go's), and the flake's
`outputHashes` entry, whose key carries the crate version.

`dstore_client::verify_record` is unchanged, as Go's `client.VerifyRecord` is: it checks no length field,
also not a Commit's (vector `commit_length_field_off_by_one`).

## 3. Mapping

| Go | Rust |
|---|---|
| `client.TreeOf(k, get)` | `dstore_client::tree_of(k, get)`, generic over the getter's error; `TreeOfError::{Read, Decode}` render `reading commit %s: %w` and `commit %s: %w` |
| `State.RemoteCommit`, `IsBranch()`, `RemoteKey()` | `State::remote_commit` (zero key when none), `is_branch()`, `remote_key()` |
| `stateJSON.RemoteCommit` | `remote_commit` between `remote` and `remote_version`; written only when `is_branch()`; read only beside a non-empty `remote` |
| `FetchResult.Tree` | `FetchResult::tree` |
| `PushResult.Commit` | `PushResult::commit` (zero key for a plain tree) |
| `Tree.Push(ctx, cl, user, message, force, jobs, prog)` | `Tree::push(ctx, cl, user, message: &[u8], force, jobs, prog)` |
| `Tree.commit`, `Tree.samePush` | private `Tree::commit`, `Tree::same_push`, and the pure `commit_object` |
| `fetchedDesc`, `pushedKey` | `cmd_wc::fetched_desc`, `cmd_wc::pushed_key` |
| `&cli.StringFlag{Name: "message", Aliases: []string{"m"}}` | `app::string_aliased("message", &["m"], …)`: the first command flag with an alias; `dstore-gocli` already handled aliases for `help/h` and `version/v` |
| node `verifyRecord` | `dstore_testkit::fake::verify_record` |

## 4. Decisions

- **Type checks read the type nibble.** Go's `k.Type() == key.Commit` is false for a reserved nibble, where
  core-rs `Key::type_` panics. `tree_of`, `State::is_branch`, the flows and the CLI use
  `Type::from_u8(k.0[0] >> 4) == Some(Type::Commit)`. The zero key of an empty result is a Blob key, as in Go,
  so `fetchedDesc` of an absent reference is `root 0000000000000000`.
- **The message is a Go string, so bytes.** `push -m` with bytes that are not UTF-8 fails as Go's commit
  validation fails it, `commit: commit message must be valid UTF-8`, after the tree is built and before the
  reference is written. It is not sent lossily (PORTING.md DD-8). `commit_object` keeps Go's check order for
  that case: tree, parents, author, committer, then the message's length, then its encoding.
- **The identity.** Name = the push user, no email, `when` = now in nanoseconds, `tz_offset` = the local
  zone's offset at now in whole minutes, truncated toward zero (Go `off / 60`), through
  `gocompat::time::SystemZone`, the zone rule the slog timestamps use. A user that passes `ValidateUser` also
  passes the identity's name rules (both refuse an empty name and control characters; `ValidateUser`'s length
  limit is not above the identity's), so `commit: commit author: …` is not reachable from the CLI.
- **Blocking work.** The commit is stored, and the cluster's commit is read for `samePush`, on the blocking
  pool. `tree_of` runs there too, and is skipped for a key that is no commit (it would return the key
  unread).
- **`samePush` reads only the local packstore**, as Go does: an interrupted push is recognised from the
  working copy that made it (or a copy of it). From another working copy the same situation is a changed
  reference.
- **An up-to-date fetch of a branch still reads the commit** from the local packstore (`TreeOf`), as Go
  does. It pulls nothing.
- **Type 5 is no longer a reserved type.** Two vector families used it as their reserved-type example
  (`client/verify_record.json` `key_reserved_type_5`, `worktree/state.json` `base-reserved-type`); both use
  type 6 now.

## 5. Vectors

Regenerated with the generator at dstore v0.1.10 and core v0.0.9. Only these files changed:

| File | Change |
|---|---|
| `worktree/state.json` | `commit` (dstore's own test commit, key and bytes); `State` gains `remote_commit`, `is_branch`, `remote_key`; 5 encode and 18 decode cases for `remote_commit`; the reserved type is 6 |
| `worktree/cli.json` | the three `%s`-for-what-was-fetched formats, `pushed %s: commit %s, root %s, …`, `commit %s, root %s`; new lists `fetched_desc` and `pushed_key` from verbatim copies of `fetchedDesc` and `pushedKey` (self-checked against `cmd/dstore/wc.go`) |
| `client/verify_record.json` | `commit`, `commit_length_field_off_by_one`; `key_reserved_type_6` replaces `key_reserved_type_5` |
| `cli/snapshots.json` | built from the v0.1.10 binary: `push`'s help with `--message value, -m value`, `push -m msg`, `--message=msg`, `-message msg` on a working copy, the missing-value usage errors of `-m` and `--message`, and the node-side refusal text, which names v0.1.10 |

Every verbatim copy the generator checks against the dstore sources (`cmd/dstore` declarations and tests,
`client/rank.go`, `batch.go`, `fetch.go`, `progress.go`) is unchanged in v0.1.10, so the self-checks pass with
the version constants moved.

## 6. Tests

| Go | Rust |
|---|---|
| `client/commit_test.go` `TestTreeOf` | `crates/client/src/commit.rs` `tree_of_resolves_commits`, plus the reserved nibble |
| `worktree/tree_test.go` `TestCreateOpenRoundTrip` (now a branch state) | `crates/worktree/src/tree.rs` `create_open_round_trip`, plus `remote_commit_in_the_state_file` |
| `node/verify_test.go` `TestVerifyRecordCommitLength` | `crates/testkit/src/fake.rs` `verify_record_accepts_and_rejects_as_the_node` |
| `node/branch_test.go` `TestWorktreeBranch` | `tests/fake_cluster_worktree.rs` `worktree_branch`. The garbage-collection part is not ported (the fake nodes have no GC); instead the test asserts that every key reachable from the tip is on a node and that a fresh clone holds the whole history |
| `node/worktree_test.go` (signature change only) | `tests/fake_cluster_worktree.rs`, an empty message everywhere |
| — | `a_message_turns_a_plain_reference_into_a_branch` (a message on a plain reference, a message that is not UTF-8); `commit_object_checks_the_message_in_go_order`; `fetched_desc_and_pushed_key_vectors`; the golden `test_commit` |
| live | `interop/check.sh` D14: a branch started by Go, cloned and continued by Rust, pulled back by Go |

## 7. Following the next upstream release

The steps this change took, in order. The dstore v0.1.11 bump (`core-v0.0.10.md`) followed them and added
what is marked "(v0.1.11)".

1. Read the delta: `git -C ../dstore diff --stat vOLD vNEW`, then the client-side files in full. Check
   `go.mod` for moved dependencies, and `cmd/dstore/*_test.go` and the files the generator copies verbatim.
   (v0.1.11) When a core moved, read its delta and the core-rs release notes too
   (`gh release view vX -R amber-store/core-rs`): a core release can change what dstore does without a line
   of dstore changing. Where it changes what a store directory holds or who may open it, build the Go CLI
   and the generator's helpers and try it; the old behaviour is written into tests, vectors, fixture
   steps that build stores through core (`pebble_refs`), the interop checks and several documents.
2. core-rs: move `rev` in `Cargo.toml`, run `cargo check --workspace --all-targets`, fix what the new API
   breaks. Move the flake's `outputHashes` entry: its key is `amber-store-core-<crate version>`, its value
   `nix hash path` over `git archive <rev>` (the bare repository under `~/.cargo/git/db/core-rs-*` has the
   rev after the first fetch). (v0.1.11) The compiler finds struct literals and non-exhaustive matches
   (`crates/client/src/corefmt.rs` renders every `ChildKeysError` and `WalkError` with Go's text); it does
   not find a type name missing from `corefmt::type_name`, a key hard-coded in a test (grep for the old
   test commit's key), or a text that core-rs now words differently.
3. Port the code and the Go tests.
4. `tools/vectorgen`: `go get github.com/amber-store/dstore@vNEW github.com/amber-store/core@vNEW`,
   `go mod tidy`, move the version constants (`family_client.go`, `family_transport.go`,
   `cmd/clisnap/mainpkg/selfcheck.go`, the node-side text in `cmd/clisnap/main.go`), extend the families,
   then regenerate all families and `clisnap` (VECTORS.md "Regenerating"). `gotables` and `goerrno` only
   change with the Go toolchain. (v0.1.11) `tools/vectorgen/docs/` holds the per-family sources of
   VECTORS.md: a change to one goes into the other. When the upstream tag does not exist yet, see
   `core-v0.0.10.md` §7 for working from a checkout, and for what must not be committed.
5. Move the pins in the documents and the automation: PORTING.md §0, §5.11 and §7, README.md, VECTORS.md,
   `interop/` (the tag `check.sh` requires of `../dstore`), `DSTORE_GO_REF` in `.github/workflows/`
   (`ci.yml` and `release.yml`), the node-side refusal text in `crates/cli/src/nodeside.rs`, the
   descriptions in `Cargo.toml` and `flake.nix`. Mentions that record history ("added with dstore
   v0.1.10", "5 is Commit since core v0.0.9") stay.
6. Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`,
   `TZ=UTC cargo test --workspace --all-targets --locked`, the generator's `go vet ./...` and
   `go test ./...`, a second generation run (identical bytes), `bash interop/check.sh`, and
   `nix build .#dstore` (it proves the `outputHashes` entry).
