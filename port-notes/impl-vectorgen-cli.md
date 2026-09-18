# impl-vectorgen-cli: CLI golden vectors (layer L1)

Owner: vectorgen-cli. Go generator only; no Rust. Schemas: `tools/vectorgen/docs/vectorgen-cli.md`.

## What landed

| Path | Content |
|---|---|
| `tools/vectorgen/cmd/clisnap/main.go`, `main_test.go` | the snapshot program: builds the Go CLI into a temp dir, builds fixtures, runs every case, normalises, writes `snapshots.json`, deletes everything |
| `tools/vectorgen/cmd/clisnap/mainpkg/copied.go` | 40 verbatim `cmd/dstore` declarations (main.go, size.go, client.go, wc.go, tui.go), each preceded by its source lines |
| `tools/vectorgen/cmd/clisnap/mainpkg/selfcheck.go`, `selfcheck_test.go` | the AST self-check (go/printer comparison with the module's `cmd/dstore`, version v0.1.9 required) and its drift tests |
| `tools/vectorgen/cmd/clisnap/mainpkg/api.go` | exported wrappers the `cli` family calls |
| `tools/vectorgen/cmd/clisnap/mainpkg/{size,tui,wc}_test.go` | verbatim copies of the Go tests, run against the copies |
| `tools/vectorgen/family_cli.go` | family `cli`: `cli/size.json`, `cli/text.json` |
| `tools/vectorgen/docs/vectorgen-cli.md` | schemas, the case-running procedure, fixture ops |
| `tests/golden/cli/size.json` | 64 `parse` cases, 17 `pack_size` cases |
| `tests/golden/cli/text.json` | 15 arrays, including 10 `ui_model` scenarios with exact views |
| `tests/golden/cli/snapshots.json` | 364 cases (help 113, unknown 9, usage 37, required 13, validation 98, node-side 35, prompt 10, wc 49), 17 marked `node_side` (A 8, B 5, C 2, DD-2 2), 10 fixtures (counts after the review below) |

## Gates run

- `gofmt -l family_cli.go cmd/clisnap`: clean.
- `go vet ./cmd/clisnap/...` and `go vet main.go util.go family_cli.go`: clean.
- `go test -count=1 ./cmd/clisnap/...` passes: self-check, drift detection, `TestParseSize`, `TestPackSizeFlag`,
  `TestUIModel`, `TestRateMeter`, `TestTeaHandler`, `TestStatusLine`, `TestResolveTicket`, normalisation,
  fixture ops, case table.
- `go test -count=1 main.go util.go main_test.go family_cli.go`: the registry tests with the `cli` family
  registered pass.
- Each generator ran twice with byte-identical output, and a regeneration equals the committed files.
- The whole-module `go run .` / `go test ./...` did not compile during this work because sibling families
  (`family_client.go`, `family_wire.go`) were mid-edit. I generated with
  `go run main.go util.go family_cli.go ../../tests/golden cli`, which has the same registry and gives the
  same output.

## Decisions and deviations

1. **Copies in a library package.** The copies live in `cmd/clisnap/mainpkg`, not in vectorgen's
   `package main` as verification.md §4.2 (`mainpkg_copy.go`) has it. Generic names (`latest`, `tick`,
   `fraction`, `nodeFlags`, `statusLine`) would clash with sibling families that share `package main`.
2. **`go build` instead of `go install @v0.1.9`.** clisnap runs `go build` inside the vectorgen module rather
   than `GOBIN=… go install …@v0.1.9` (VECTORS.md, `tools/vectorgen/cmd/README.md`, verification.md §4.2):
   - vectorgen's `go.mod` requires exactly the module versions of dstore v0.1.9's `go.mod`;
   - the build works offline with the committed `go.sum`;
   - the binary's build information is checked: main module `github.com/amber-store/dstore` v0.1.9.

   The interface `-o OUTDIR` matches ci.yml. VECTORS.md and `cmd/README.md` still say "temporary GOBIN";
   I do not own those files.
3. **Snapshot schema.** It extends verification.md §4.3 item 24:
   - `env` is an ordered list, not a map;
   - fixtures are declared in the file as ops that the Rust harness replays with its own implementation,
     so a Go-written packstore is never shared;
   - cases also carry `subdir`, `group` and `node_side`;
   - the placeholders `{ROOT}`, `{key:NAME}` and `{key16:NAME}` join `{CWD}`. Ingested keys depend on the
     uid, gid and times of the harness user.
4. **`cli/text.json` contents.** It carries `human_bytes`, `rate`, `status_line`/`fraction`, `rate_meter`,
   `hex_decode`, `resolve_ticket`, `log_level`, `describe_change`, `filter_paths`, `node_state`,
   `tea_handler`, `format_event`, `blend1d`, `progress_bar` and `ui_model`.
   - PORTING.md §7 puts `status_line` and `describe_change` in `text/formats.json` (vectorgen-proto).
     Duplicates there are harmless.
   - The doc comment of `tests/golden_tests/cli_progress.rs` names `text/formats.json` `status_line` and
     `fraction`. The cli-progress owner should read `cli/text.json`, which also holds `ui_model`,
     `tea_handler`, `format_event`, `blend1d` and `rate_meter`.
5. **Floats are JSON strings** (shortest round-trip form): serde_json without `float_roundtrip` may misround a
   JSON number.
6. **Nondeterministic TUI views.**
   - The TUI's `cancelling` event carries `time.Now()`, so its clock appears as `{CLOCK}` in views.
   - Views after `done` whose rates depend on the wall clock record `contains`. Every other view is exact.
   - Tick offsets avoid half-second boundaries, because the Rust test's start `Instant` precedes the model's
     own start by a few nanoseconds.
7. **Node-side substitutes record Go's real behaviour.** Where Rust substitutes a PORTING.md §2.2 text:
   - Go binds (always with `--no-relay --no-discovery`) and opens Pebble stores inside the temp dir;
     `cluster init --weight 100` really creates a cluster and exits 0;
   - the random parts are normalised to `{SLOG}`, `{TICKET}` and `{HEX64}`;
   - `node_side.rust` holds the expected Rust output.

## Facts found while generating (for the Rust owners)

- **PORTING.md §2.2 B.1 wording.** When `<store>/identity` is a directory, Go prints
  `node: no identity in dirident: read dirident/identity: is a directory`: `os.ReadFile` reports op `read`
  for a read failure and `open` for an open failure. §2.2 B.1 always writes "open". Case
  `node-side/files cluster ticket --store dirident` locks Go's text, so `nodeside::local_ticket` must render
  the gocompat `ReadFile` error rather than a fixed `open`.
- **Marked cases where Rust's text differs:**
  - `catalog restore --store DIR FILE` with no identity in DIR: Go prints the identity error; Rust prints
    the §2.2 C text (marked C).
  - `serve --store backup.bin/x`: Go prints `mkdir backup.bin: not a directory`; Rust prints the §2.2 A text
    (marked A).
- **Usage and help quirks:**
  - `gc run --garbage 1e400` gives `invalid value "1e400" for flag -garbage: value out of range`.
  - `node join --seed '' --token ''` satisfies the required flags (`IsSet`) and fails with `ticket: empty`.
  - `refs help refs` exits 3 with `No help topic for 'refs'`.
  - `clone --local L trees/x` is `Incorrect Usage: flag provided but not defined: -local`, the stale-script
    shape of cli.md §1.2.
- **The `cluster replicas` prompt** proceeds on `  y\n` (Scanln skips leading blanks) and on `y extra\n`.
- **`store push --local L src NAME` without a ticket** prints `built {key16:src}: 2 new objects` before
  `no cluster`.

## Remaining

- **Live cases** need a cluster and go to the interop harness (L6), not the snapshots:
  - `cat NAME /` (DD-7);
  - SIGPIPE on `cat`/`watch`;
  - the TUI into `/dev/null`;
  - colour downsampling through a pty;
  - `pull` conflict output;
  - the §3.4 output of admin and transfer commands.
- **Linux is unproven.** The snapshots were generated on macOS arm64. Every compared text is an errno or Go
  text common to Linux and macOS, but only the CI `vectors` job (ubuntu) proves identical bytes.
- **Whole-module runs.** Resolved during the review. Once the sibling families compiled, `go build .` and
  `go vet ./...` of the whole vectorgen module passed. The documented `go run . <out> cli` gave files equal to the
  committed `cli/size.json` and `cli/text.json`. A whole-module `go test ./...` was not run; the tests of this
  area's packages pass.

## Review

Reviewer: review-vectorgen-cli. I assumed the work was wrong until checked. All runs used `nix develop` (go1.26.5)
from `tools/vectorgen`.

### Checked and correct

- **Copies.** `mainpkg.SelfCheck` passes against the module cache's dstore v0.1.9. Every `// ---- cmd/dstore/F:A-B ----`
  line reference is byte-exact against `git show HEAD:cmd/dstore/F`, and the module cache files equal the
  checkout's HEAD. The three test copies differ from the originals only in the package clause and a leading
  comment.
- **Build list.** `go list -deps` of `cmd/dstore` gives the same 73 module versions inside the vectorgen module
  and inside dstore's own module. So `go build` in vectorgen builds what `go install …@v0.1.9` builds.
- **Determinism.** Two runs of the `cli` family and two runs of `clisnap` each gave identical bytes, equal to the
  committed files (before my changes, and again after them).
- **Values.** `text.json` matches cli.md §5.4 (human_bytes, rate, fractions, the rate-meter sequence with sample
  counts, hexDecode, describeChange, filterPaths, teaHandler, formatEvent styles). The `two-nodes` bar line equals
  the verified §3.6 frame byte for byte.
- **Coverage.** Every case of cli.md §5.2, the offline cases of §5.3 and verification.md §5 `cli/snapshots.json`
  has a snapshot. Some differ in inessential details: `L` instead of `/nonexistent`, `backup.bin` instead of
  `/etc/hosts`, `nosuch` instead of `bogus`.
- **Node-side marks.** The kinds are right: `serve --store backup.bin/x` is A; `catalog restore --store node
  <hex>` is B (dial through `--store`); `catalog restore --store nostore backup.bin` is C.
- **Unmarked node-side cases.** `node join --seed '' --token ''` (`ticket: empty`) and the identity-read cases
  follow Go.
- **The `dirident` finding.** `node.OpenOffline` is `fmt.Errorf("node: no identity in %s: %w", dir, err)` over
  `os.ReadFile`, so `read …: is a directory` is Go's text on Linux and macOS. The snapshot is right; PORTING §2.2
  B.1's fixed `open` is not.

### Found and fixed

1. **Docs, wrong escape line.** `docs/vectorgen-cli.md` said Go's JSON escapes `<`, `>`, `&` "as `<`, `>` and
   `&`", and the line held a raw ESC byte. It now names `<`, `>`, `&` and ``.
2. **Missing spec'd case, §2.3 refusal.** verification.md Addenda item 9 lists `store pull --local <dir whose
   refs/ holds Pebble files>`, and PORTING C25 applies the refusal to push and pull. Neither was captured.
   - **New fixture.** `pebble-refs` with a new op `pebble_refs {path, names}`. Go runs core `refstore.Open` and
     `Close`, then checks the directory entries against `names`, so a Pebble upgrade fails the run. Rust creates
     empty files with those names.
   - **New cases.** `validation/pebble-refs store pull --local P trees/x` and `… store push --local P src
     trees/x`, marked with the new `node_side.kind` `DD-2`.
   - **What each prints.** Go opens the Pebble refs and fails with `no cluster` (push first prints `built …`).
     `node_side.rust` holds the §2.3 refusal with `<DIR>` = `P`.
3. **Quirk pins, cheap and deterministic.** Each one came from a binary probe before I added it:
   - `required/node join help`: the shared help command's HelpName comes from the parent on the path (`dstore
     node help`; `store push help` gives `dstore store help`).
   - `help/env DSTORE_PACK_SIZE=1Gi serve --help`: help shows the definition default `"2Gi"` (cli.md §2.2.5).
   - `validation/refs --ticket --bogus`: a value flag takes the next argument even when it starts with a dash
     (§2.2.4).
   - `validation/cluster ticket --ticket bogus --store nostore`: the ticket wins over `--store`.
   - `validation/clone <1025 bytes>`: `reference name exceeds 1024 bytes`, checked before the characters.
   - `validation/store push --local /nonexistent p`: the argv of §5.2, verbatim.
4. **Inaccurate comment.** `mainpkg/api.go` claimed "no behaviour of cmd/dstore is restated". In fact
   `StatusChangeLine`, `ProgressBar` and the pack-size and log-level apps restate code the self-check cannot
   compare. The comment now says so.
5. **Docs gaps.**
   - `exclude` of the `ingest` op is absent when empty (`omitempty`).
   - The schema now documents the `pebble_refs` op, kind DD-2 and its Rust text.
   - The spec list names PORTING §2.3.

`main_test.go` gained `TestPebbleRefsStep`, a DD-2 check in `TestNodeSideTexts`, and a check that every marked
case has valid substitute text.

### Considered, no change

- **Flags on the kind-A cases.** They carry `--no-relay --no-discovery`. Without them Go waits for a home relay
  over the network, which is not deterministic. The Rust text does not depend on these flags.
- **slog lines in marked cases.** Only `cluster init --weight 100` ("adopted view", "cluster initialised") and
  `node join --weight 100` ("node started") print any. They are normalised line by line to `{SLOG}`, and their
  number was stable over four macOS runs. Linux stays unproven until the CI `vectors` job runs.

### Gates after the review

- `gofmt -l family_cli.go cmd/clisnap`: clean.
- `go vet ./cmd/clisnap/...` and `go vet main.go util.go family_cli.go`: clean.
- `go test -count=1 ./cmd/clisnap/...` and `go test -count=1 main.go util.go main_test.go family_cli.go`: pass.
- Regeneration: `cli/size.json` and `cli/text.json` are unchanged. `cli/snapshots.json` was regenerated twice
  (identical) and installed: 364 cases, 10 fixtures.

### Still open

- **Linux proof.** Snapshots from the CI `vectors` job on Linux.
- **Files I do not own.**
  - The "temporary GOBIN" wording in `VECTORS.md` and `tools/vectorgen/cmd/README.md`.
  - The `tests/golden_tests/cli_progress.rs` doc comment, which names `text/formats.json`; the values it needs
    are in `cli/text.json`.
- **Rust harness.** `tests/cli_snapshots.rs` must implement the `pebble_refs` op and compare DD-2 cases with
  `node_side.rust`.
