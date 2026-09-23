# CLI vectors: `cli/size.json`, `cli/text.json`, `cli/snapshots.json`

Owner: vectorgen-cli. Specs: port-notes/cli.md §5.2-§5.4, verification.md §4.3 items 20, 21, 24 and §5,
PORTING.md §1.1 (CLI surface), §2.2 (node-side handling), §2.3 (local refs policy) and §7.

| File | Producer | Rust tests |
|---|---|---|
| `cli/size.json` | family `cli`, `tools/vectorgen/family_cli.go` | `tests/golden_tests/cli.rs` (`dstore_cli::size`) |
| `cli/text.json` | family `cli`, `tools/vectorgen/family_cli.go` | `tests/golden_tests/cli_progress.rs` (`dstore_cli::progress`), crate unit tests of `dstore-cli` for crate-private helpers |
| `cli/snapshots.json` | `tools/vectorgen/cmd/clisnap` | `tests/cli_snapshots.rs` |

## Where the values come from

- **Go dstore v0.1.11 `cmd/dstore`** is `package main`, so `tools/vectorgen/cmd/clisnap/mainpkg/copied.go`
  holds verbatim copies of the declarations the vectors need:
  - `main.go`: `logLevel`, `storeFlag`, `noDiscoveryFlag`, `netFlags`, `nodeFlags`, `defaultPackSize`,
    `packSize`;
  - `size.go`: `parseSize`; `client.go`: `hexDecode`;
  - `wc.go`: `resolveTicket`, `describeChange`, `filterPaths`;
  - `tui.go`: `latest`, `rateMeter`, `rateSample`, `newRateMeter`, `statusLine`, `fraction`, the message
    types, `maxEvents`, `tickEvery`, the five styles, `uiModel` and its methods, `newUIModel`, `tick`,
    `appendEvent`, `nodeState`, `formatEvent`, `teaHandler` and its methods, `attrValue`.
- **The self-check.** `mainpkg.SelfCheck` finds the dstore module with `go list -m` (falling back to the
  build information and `$GOMODCACHE`), requires version v0.1.11, parses `cmd/dstore/*.go` (non-test)
  without comments, and prints every declaration with `go/printer`. Each copy must print identically,
  and a declaration that `cmd/dstore` lacks fails the check. The `cli` family and `clisnap` run it
  before generating anything.
- **The Go tests.** `mainpkg` also holds verbatim copies of `size_test.go`, `tui_test.go` and
  `wc_test.go`, which `go test ./cmd/clisnap/mainpkg` runs against the copies.
- **Libraries.** `client.HumanBytes` and `client.Rate` (dstore v0.1.11); `lipgloss.Blend1D` (v2.0.6);
  bubbles v2.2.1 `progress`.
- **Snapshots.** `clisnap` builds `github.com/amber-store/dstore/cmd/dstore` with the vectorgen build list,
  which equals dstore v0.1.11's `go.mod`. It builds with go1.26.5 and `CGO_ENABLED=0`, without ldflags (so
  `version` is `dev`), into a temporary directory, and checks the binary's build information (main module
  dstore v0.1.11). It runs every case there and deletes the binary and all fixtures afterwards.

## Regenerating

```sh
nix develop -c go -C tools/vectorgen run . ../../tests/golden cli              # cli/size.json, cli/text.json
nix develop -c go -C tools/vectorgen run ./cmd/clisnap -o ../../tests/golden/cli   # cli/snapshots.json
```

Both are deterministic: two runs give identical bytes. `clisnap -v` prints each case as it runs.

## Conventions beyond VECTORS.md

- **Floats** (`rate`, `fraction`, `percent`) are JSON strings holding the shortest decimal that parses back
  to the same `float64` (`strconv.FormatFloat(f, 'g', -1, 64)`, e.g. `"3.7e+06"`, `"816.6666666666666"`,
  `"1e-07"`). Read them with `str::parse::<f64>()`, which is exact.
- **Levels** are slog level numbers: DEBUG -4, INFO 0, WARN 4, ERROR 8, custom values in between.
- Go's JSON writer escapes `<`, `>` and `&` as `\u003c`, `\u003e` and `\u0026`, and ESC as `\u001b`. Any JSON
  parser restores the characters.

## `cli/size.json`

```json
{
  "parse":     [ {"in": "512Ki", "ok": true, "out": "524288"}, {"in": "2X", "ok": false, "error": "bad size \"2X\": unknown unit \"X\" (want Ki, Mi, Gi or Ti)"} ],
  "pack_size": [ {"name": "env", "flag": null, "env": "1Gi", "ok": true, "out": "1073741824"} ]
}
```

- **`parse`:** `parseSize(in)`. `out` (i64 string) is present when `ok`, and `error` is the exact text
  otherwise. The cases cover `size_test.go` `TestParseSize`, cli.md §5.4, verification.md §5, every unit
  spelling, the int64 limits and Unicode spaces. Rust: `size::parse_size(in) == Ok(out)` or
  `Err(error)`.
- **`pack_size`:** `packSize(c)` inside a urfave app over `nodeFlags()`, as `TestPackSizeFlag` does.
  - `flag`: `null` means no argument; a string means `--pack-size <flag>`.
  - `env`: `null` means `DSTORE_PACK_SIZE` is unset; a string (possibly empty) is its value.
  - Rust: parse a node-side command's flags (for example the `serve` definition of `dstore_cli::app()`)
    with that argument and environment, then call `size::pack_size(&ctx)`.
  - Tests that set the process environment must not run in parallel with each other.

## `cli/text.json`

Every top-level key is an array of cases.

| Key | Case | Go |
|---|---|---|
| `human_bytes` | `{n: i64s, out}` | `client.HumanBytes(n)` |
| `rate` | `{bytes: i64s, took_ns: i64s, out}` | `client.Rate(bytes, took)`; `took_ns <= 0` → `-` |
| `status_line` | `{report: Report, rate: f64s, out, fraction: f64s}` | `statusLine(report, rate)`, `fraction(report)` |
| `rate_meter` | `{name, window_ns: i64s, adds: [{t_ns: i64s, n: i64s, rate: f64s, samples}]}` | `newRateMeter(window)`, then `add(t0 + t_ns, n)` in order. `samples` is the number of samples kept, for internal tests. Rust: `t` = a base `Instant` + `t_ns` (never negative). |
| `hex_decode` | `{in, ok, out: hex\|null, error?}` | `hexDecode(in)`; Rust `common::hex_decode(in.as_bytes())` |
| `resolve_ticket` | `{flag, stored, env, ok, out?, error?}` | `resolveTicket(flag, stored, env)` |
| `log_level` | `{value, level}` | `logLevel(c)` with `--log-level value` |
| `describe_change` | `{path, kind, kind_string, old_mode: n\|null, new_mode: n\|null, describe, line}` | See below. |
| `filter_paths` | `{changes: [path], args: [string], ok, out: [path]\|null, error?}` | See below. |
| `node_state` | `{in_flight, awaiting, out}` | `nodeState(NodeProgress{InFlight, Awaiting})` |
| `tea_handler` | see below | `teaHandler` |
| `format_event` | `{at_unix_ns: i64s, offset_secs, level, text, out}` | `formatEvent(eventMsg{at: time.Unix(0, at_unix_ns).In(FixedZone(offset_secs)), level, text})` |
| `blend1d` | `{steps, a: [r,g,b], b: [r,g,b], out: [[r,g,b], …]}` | `lipgloss.Blend1D(steps, a, b)` over opaque stops. Channels are as x/ansi writes them in `38;2;R;G;B` (`RGBA() >> 8`). Rust `progress::blend1d`. |
| `progress_bar` | `{width, percent: f64s, out}` | bubbles `progress.New(progress.WithDefaultBlend())`, `SetWidth(width)`, `ViewAs(percent)`: the bar line of the view, for the renderer's internal tests |
| `ui_model` | see below | `uiModel` |
| `color_profile` | `{env: ["KEY=VALUE"], tty, out}` | With `tty`, `colorprofile.Env(env)`; without, `colorprofile.Detect(&bytes.Buffer{}, env)`: an output that is not a terminal, so terminfo and tmux are never consulted, and no case sets `TTY_FORCE`. `out` is `Profile.String()`. Rust `progress::colorprofile::{env_profile, detect, color_profile}`. |
| `convert256` | `{rgb: [r,g,b], c256, c16}` | x/ansi `Convert256` and `Convert16` of an opaque `color.RGBA`: the default bar's colours (bar widths 20, 60 and 80), its empty run, points around the cube levels, and a fixed pseudo-random sample. Colours with a channel of 115, 155, 195 or 235 are left out (`cliArchNeutral`): gc fuses `c*255-35` into one FMA on arm64 and not on amd64, so their index depends on the architecture that generates the file. Every other colour agrees on both. Rust `progress::colorprofile::{convert256, convert16}`. |
| `downsample` | `{profile, in, out}` | `colorprofile.Writer{Profile: profile}.Write(in)` for every profile and input. Rust `progress::colorprofile::downsample`. |

- **Report:** `{objects: n, total_objects: n, bytes: i64s, total_bytes: i64s, nodes: [Node]}`.
- **Node:** `{id: hex (32 bytes), direct, rtt_ns: i64s, in_flight: n, awaiting: n, bytes: i64s}`
  (`client.NodeProgress`).

- **`describe_change`:**
  - The change is `worktree.Change{Path: path, Kind: kind, Old: &Entry{Mode: old_mode} or nil, New: likewise}`.
  - `describe` is `describeChange`; `line` is the `status` line `fmt.Sprintf("  %-9s %s", kind, describe)`.
  - `kind` is the Go `worktree.Kind` number: 0 Added, 1 Deleted, 2 Modified, 3 TypeChanged, 4 ModeChanged,
    5 MetaChanged. Kind 9 has no Rust variant; skip it.
- **`filter_paths`:**
  - The call is `filterPaths(root, changes, args)`, with root = the process working directory (`os.Getwd`).
    The changes are Added entries; only their paths matter.
  - `{ROOT}` in `args` and `error` stands for that root: substitute the test process's working directory.
  - `out` is `[]` when nothing matches, and `null` when `ok` is false.
- **`tea_handler`:**
  `{name, handler_level, with: [Attr], level, msg, attrs: [Attr], enabled, text: string|null}`.
  - Go runs `slog.New(&teaHandler{level: handler_level}).With(with…)`; `enabled` is `Enabled(level)`,
    then `LogAttrs(level, msg, attrs…)` is logged.
  - `text` is the text of the one event sent (its level is `level`), or `null` when nothing was sent.
  - `Attr` is `{key, kind, value}`. Kinds map to slog as follows:

    | kind | slog attribute |
    |---|---|
    | `string` | `slog.String` |
    | `int64` | `slog.Int64` |
    | `uint64` | `slog.Uint64` |
    | `float64` | `slog.Float64` |
    | `bool` | `slog.Bool` |
    | `duration` | `slog.Duration`, value in nanoseconds |
    | `error` | `slog.Any` over `errors.New(value)`; the value `<nil>` stands for a nil error |

    Rust: `Value::String`, `Int64`, `Uint64`, `Float64`, `Bool`, `Duration`, and `Any(value)`.

### `ui_model`

`{name, title, offset_secs, steps: [Step]}`. Go drives `newUIModel(title, latest, cancel)` through
`Update` as `TestUIModel` does. The model's zone is `FixedZone(offset_secs)`, and events are created in
that zone.

| Step | Go | Rust |
|---|---|---|
| `{"op": "set", "report": Report}` | `latest.set(report)` | the `Latest::progress()` callback |
| `{"op": "resize", "width": n, "quit", "cancelled"}` | `tea.WindowSizeMsg{Width: n, Height: 40}` | `UiMsg::Resize(n)` |
| `{"op": "tick", "offset_ns": i64s, "quit", "cancelled"}` | `tickMsg(start + offset)` | `UiMsg::Tick(start + offset)` |
| `{"op": "event", "at_unix_ns": i64s, "level": n, "text", "quit", "cancelled"}` | `eventMsg` | `UiMsg::Event { at: GoTime::from_unix_nano(at_unix_ns), level: Level(level), text }` |
| `{"op": "ctrl_c", "quit", "cancelled"}` | `tea.KeyPressMsg{Code: 'c', Mod: tea.ModCtrl}` | `UiMsg::CtrlC` |
| `{"op": "done", "error"?: string, "quit", "cancelled"}` | `doneMsg{err}` (no `error`: nil) | `UiMsg::Done(error)` |
| `{"op": "view", "out": string}` | `View().Content`, exact | `view()` |
| `{"op": "view", "contains": [string]}` | the view contains every string | `view()` |

- **`quit`:** whether the update returned `tea.Quit`; Rust `update` returns `true`.
- **`cancelled`:** whether the cancel function had been called by then; Rust: the cancel `Ctx` is
  cancelled.
- **`start`:** the model's creation time. In Rust, take an `Instant` just before `UiModel::new`. The
  offsets avoid half-second boundaries, so the nanoseconds between those two instants never change
  `elapsed`.
- **`{CLOCK}`:** in a `view`, it stands for the `HH:MM:SS` clock of the `cancelling` event that ctrl+c
  adds with `time.Now()`. Replace the 8 bytes before `  cancelling` in the Rust view.
- **After `done`:** the model observes the wall clock. Exact views after `done` appear only when no bytes
  moved between samples, so they show `elapsed 0s` and `0 B/s` (the test must finish within half a
  second). Other views after `done` use `contains`.

## `cli/snapshots.json`

```json
{
  "generator":   {"program": "tools/vectorgen/cmd/clisnap", "go": "go1.26.5", "module": {"path": "github.com/amber-store/dstore", "version": "v0.1.11"}, "deps": [{"path": "…", "version": "…"}]},
  "environment": {"inherited": ["PATH"], "set": [{"name": "HOME", "value": "{HOME}"}, {"name": "TZ", "value": "UTC"}]},
  "fixtures":    [ {"name": "wc1", "steps": [Step, …]} ],
  "cases":       [ Case ]
}
```

A case:

```json
{
  "name": "validation/refs --ticket bogus", "group": "validation",
  "args": ["refs", "--ticket", "bogus"], "env": [{"name": "DSTORE_TICKET", "value": ""}],
  "fixture": "empty", "subdir": "", "stdin": "",
  "exit": 1, "stdout": "", "stderr": "dstore: ticket: …\n",
  "node_side": null
}
```

`generator` and `environment` are provenance: `{HOME}` is the temporary home directory.

### Running a case

`clisnap` runs each case as follows, and `tests/cli_snapshots.rs` must do the same:

1. Make a fresh temporary directory ROOT and build the case's fixture in it (steps below). Make a separate
   temporary HOME.
2. cwd = ROOT joined with `subdir`. Resolve cwd and ROOT to real paths (`pwd -P`).
3. In `args` and in env values, replace `{CWD}` with the resolved cwd.
4. Spawn the binary with the args and that cwd. The environment is exactly `PATH` (inherited),
   `HOME=<HOME>`, `TZ=UTC`, then the case's `env` in order. There is no `PWD`.
5. stdin is a pipe that receives `stdin` and then EOF; stdout and stderr are pipes. Stderr is therefore not
   a character device, and every transfer runs in plain mode.
6. Wait for the process, with a 60 s limit. It must exit normally, not by a signal.
7. Normalise stdout and stderr, in this order:
   1. Replace the resolved cwd with `{CWD}`.
   2. When `subdir` is not empty, replace the resolved ROOT with `{ROOT}`.
   3. For each fixture variable NAME in sorted order, replace its key's 64 hex digits with `{key:NAME}`,
      then their first 16 with `{key16:NAME}`.
8. Compare `exit`, `stdout` and `stderr` byte for byte. For a case with `node_side`, compare with
   `node_side.rust` instead.

### Groups

The groups follow cli.md §5.2 and verification.md §5.

| `group` | Content |
|---|---|
| `help` | App, command and subcommand help (`--help` and `help <cmd>` of all 21 commands, `--help` of every subcommand, parents without arguments), `--version`, `help help`, `<leaf> help\|h`, truncated `help <parent>`, `store push help` |
| `unknown` | `No help topic for '…'`, exit 3 |
| `usage` | `Incorrect Usage: …` plus help, for flag syntax, unknown flags, missing values, value parse errors (bool, int, uint, float64, duration), global flags after the command, and bool env parse errors |
| `required` | `Required flag(s) … not set`, with and without help, plus the empty `AMBER_STORE` |
| `validation` | Argument and ticket validation before dialing: `no cluster`, ticket texts, relay URL errors, name validation, `replicas R`, `why KEY…`, `view: bad node id`, `store push` building the tree before failing, and the DD-2 cases over a `--local` refs directory that still holds a Pebble store |
| `node-side` | The node-side path up to the store step (`no store directory`, `--pack-size`, seed and token, the identity read of `--store`), and the cases where Rust substitutes |
| `prompt` | The `cluster replicas` prompt with stdin `n\n`, EOF, `yes please\n`, `Y`, and others |
| `wc` | Offline working-copy commands: outside a working copy, broken `.dstore` files, a copy without a remote, and the two fixtures of cli.md §3.5 (`status`, `diff`, `diff --stat`, path filters, `--incoming`, `--remote`), plus `push`/`fetch`/`init` failing on the ticket |

### `node_side`

`node_side` is `{kind, go_normalized: [placeholder], rust: {exit, stdout, stderr}}`. It marks a case where
dstore-client-rs replaces Go's behaviour with a designed substitute: a node-side text of PORTING.md §2.2, or
the Pebble refs refusal of §2.3.

- **A:** `serve`, `cluster init` or `node join` once parsing, seed and token, `--store` and `--pack-size`
  pass.
- **B:** a ticket derived from `--store` whose identity file is readable.
- **C:** `catalog restore` with its data and `--store`.
- **DD-2:** `store push` or `store pull` with a `--local` directory whose `refs/` still holds the Pebble
  database of Go dstore v0.1.10 or earlier (fixture `pebble-refs`, group `validation`). Go imports it into
  `refs.sqlite` on this first open and fails later on the missing ticket; Rust, which cannot import Pebble,
  refuses at open.

The top-level `exit`, `stdout` and `stderr` record what Go does. Go builds, binds (always with
`--no-relay --no-discovery`) and opens its stores inside ROOT, and anything random in its output is
replaced by the placeholders listed in `go_normalized`:

| Placeholder | Replaces |
|---|---|
| `{SLOG}` | a whole slog line |
| `{TICKET}` | a `dstore1…` ticket |
| `{HEX64}` | a 64-hex node id |

`rust` is what the Rust CLI must print: exit 1, empty stdout, and stderr `dstore: <text>\n`, where the text is
the §2.2 text of the kind, or for DD-2 the §2.3 text with `<DIR>` = the `--local` value as given
(`refstore: P/refs holds a Pebble database written by Go dstore v0.1.10 or earlier; …`).

### Fixture steps

Paths are relative to ROOT and `/`-separated.

| op | Fields | Effect |
|---|---|---|
| `mkdir` | `path`, `mode` | mkdir, then chmod `mode`, so the umask does not matter |
| `write` | `path`, `text`, `mode` | create or truncate, write `text`, chmod `mode` |
| `symlink` | `path`, `target`, `mode`? | symlink. With `mode`, `fchmodat(AT_SYMLINK_NOFOLLOW)` to `mode`, which macOS applies and Linux refuses (its links are always 0777). The step fails unless the link then has `mode`. |
| `remove` | `path` | remove recursively |
| `chmod` | `path`, `mode` | chmod |
| `mtime` | `path`, `unix_ns` | `utimensat(AT_FDCWD, path, [unix_ns, unix_ns], AT_SYMLINK_NOFOLLOW)` |
| `wc_create` | `config` | `worktree.Create(ROOT, config)` (Rust `Tree::create`), then close. `config` uses the `.dstore/config` JSON keys. |
| `ingest` | `dir`, `into`, `exclude`?, `var` | Ingest ROOT/`dir` with core ingest: Go `ingest.Dir(st, dir, Opts{Jobs: 1, Exclude: exclude})`, Rust core-rs `ingest::dir` with `exclude`. An absent `exclude` means none. `into` is `wc` (ROOT/.dstore/packstore, opened with sync) or `scratch` (a throwaway packstore outside ROOT). The root key becomes variable `var`. |
| `wc_state` | `base_var`, `remote_var`?, `remote_version_hex`?, `synced_at_unix_ns` | Write `.dstore/state` as worktree `SaveState` does. `base` is variable `base_var` (`""` is the empty tree). The remote is present when `remote_var` is. `synced_at` is in UTC. |
| `pebble_refs` | `path`, `names` | A refs directory that Go dstore v0.1.10 or earlier wrote as a Pebble database. Go: `pebble.Open(path, …)` as core v0.0.9's `refstore.Open` did it (core v0.0.10's refstore is SQLite), then close, then check that the directory holds exactly `names`. Rust: create `path` with its parents and an empty regular file for each entry of `names`; core-rs knows a Pebble store by the `marker.manifest.*` name with no `refs.sqlite` beside it, which is all the PORTING.md §2.3 refusal looks at. |

`mode` is a JSON number (for example 420 = 0644). Ingested keys depend on the uid, gid and times of the
user running the harness, which is why outputs name them through `{key:…}` placeholders. The fixtures set
every mtime they rely on and keep them far from `synced_at`, so the racy window never triggers.

Every step that creates an entry sets its mode, so no fixture depends on the umask. Fixture `wc1` gives its
`link` mode 0777. A link's bits are part of its ingested entry, and `wc1` replaces the link with a 0755
directory, so `wc/wc1 diff`, `wc/wc1/sub diff` and `wc/wc1/sub diff ..` print `old mode 0777` /
`new mode 0755` for it. Linux links are always 0777, and a macOS link would otherwise get `0777 &^ umask`. With
the explicit mode the three cases are the same on both platforms.

### Which cases need what

- **`help`, `unknown`, `usage`, `required`:** only the command table and `dstore-gocli` (owner cli-app).
- **`validation`, `prompt`, `node-side`:** the actions up to dialing (cli-admin, cli-client, cli-wc,
  `nodeside`).
- **`validation/files store push …`:** ingest into `--local`.
- **`validation/pebble-refs …`:** `common::open_local` with the PORTING.md §2.3 refusal, which core-rs
  `refstore::Error::PebbleStore` decides (cli-client).
- **`wc`:** `dstore-worktree` offline and cli-wc. The fixture builder needs `Tree::create`, `save_state`
  and core-rs `ingest::dir`.

Tests of unfinished modules are `#[ignore = "needs <crate>::<module>"]`, never skipped silently.

### Not captured

The live cases of cli.md §5.3 need a cluster and belong to the interop harness (verification.md §4.5):
- `cat NAME /` (DD-7);
- SIGPIPE on `cat`/`watch`;
- the TUI into `/dev/null`;
- colour downsampling;
- `pull` conflicts;
- the §3.4 output of admin and transfer commands.
