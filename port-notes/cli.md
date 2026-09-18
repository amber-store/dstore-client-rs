# dstore CLI (`cmd/dstore`) — port notes

Normative reference: `github.com/amber-store/dstore` v0.1.9 (HEAD 368f2c7), Go 1.26.5
(`go.mod`: `go 1.26.5`), `github.com/urfave/cli/v2` v2.27.7, `charm.land/bubbletea/v2` v2.0.9,
`charm.land/bubbles/v2` v2.2.1, `charm.land/lipgloss/v2` v2.0.6.

**Verification.** Every help text, error text and exit code marked *(verified)* was captured
from the real Go CLI: the five `cmd/dstore` sources were copied unchanged into a throwaway
module (`replace github.com/amber-store/dstore => /Users/dragan/amber-store/dstore`), built with
go1.26.5 (`main.version` = `dev`), and run with 190 argument vectors (stdout, stderr and exit
code captured separately; stderr and stdin were pipes; TZ=Europe/Berlin). The harness and its
cached executable were deleted afterwards; §5 says how to rebuild it as the golden generator.

---

## 1. Scope

| Go file | Lines | What it does |
|---|---:|---|
| `cmd/dstore/main.go` | 698 | `main`, the `cli.App`, global `--log-level`, logger, shared flag groups (`storeFlag`, `ticketFlag`, `clientFlags`, `noDiscoveryFlag`, `netFlags`, `nodeFlags`), `relayMode`/`relayModeOf`, `loadOrCreateKey`, `bindNodeEndpoint`, `packSize`, `openNode`, `signalCtx`, `parseWeight`; commands `cluster {init,status,ticket,replicas}`, `serve`, `token create`, `node {join,remove,drain,weight,zone,repair}`, `voter {add,remove}`, `transition {status,abort,refreeze,pause,resume}`, `gc {run,status,hold,release,why}`, `catalog {backup,backups,restore}`; `localTicket` |
| `cmd/dstore/client.go` | 574 | `diskTotal`, `dialCluster`/`dialClusterLog`/`dialTicket`, `netOpts`, `admin`/`adminAction`, `printStatus`, `recordPayload`, `localStoreFlag`/`openLocal`, `store {push,pull}`, `refs`, `watch`, `ref {get,delete}`, `clusterGet`, `ls`, `cat`, `hexDecode` |
| `cmd/dstore/wc.go` | 501 | working-copy commands: `wcFlags`, `jobsFlag`, `resolveTicket`, `wcConfig`, `dialConfig`, `openWC`, `pushUser`, `clone`, `init`, `withCluster`, `fetch`, `pull`, `push`, `status`, `describeChange`, `diff`, `filterPaths` (the uncommitted change only splits the `wcFlags` literal over several lines; line numbers below are the working-tree ones) |
| `cmd/dstore/size.go` | 46 | `parseSize` (for `--pack-size`) |
| `cmd/dstore/tui.go` | 374 | `runTransfer`, `noTUIFlag`, `isTerminal`, `latest`, `runPlain`, `rateMeter`, `statusLine`, `fraction`, the Bubble Tea `uiModel`, `nodeState`, `formatEvent`, `runTUI`, `teaHandler`, `attrValue` |
| `cmd/dstore/size_test.go` | 68 | `TestParseSize`, `TestPackSizeFlag` |
| `cmd/dstore/tui_test.go` | 106 | `TestUIModel`, `TestRateMeter`, `TestTeaHandler`, `TestStatusLine` |
| `cmd/dstore/wc_test.go` | 20 | `TestResolveTicket` |
| `node/admin.go` | 302 | `AdminRequest`, `AdminReply` (CBOR), `handleAdmin`, `Node.Admin` (server side of every admin op; the reply texts the CLI prints) |
| `node/status.go` | 124 | `VoterStat`, `Status` (CBOR), `Node.status`, `DecodeStatus`, `ShortID` |
| `node/gc.go:1275-1309` | 35 | `gcState.statusText` (text of `gc run`/`gc status` replies, `Status.GC`) |
| `node/maintenance.go:539-557` | 19 | `maintenance.transitionText` (text of `transition status`, `Status.Transition`) |
| `catalog/catalog.go:591-605` | 15 | `PhaseName` |

Go library behaviour the user observes, also in scope: urfave/cli v2.27.7 (`app.go` 536,
`command.go` 421, `help.go` 569, `template.go` 146, `flag.go` 419, `flag_bool.go` 178,
`flag_string.go`, `flag_string_slice.go` 216, `flag_int.go`, `flag_int64.go`, `flag_uint.go`,
`flag_float64.go`, `flag_duration.go`, `context.go` 272, `parse.go` 102, `errors.go` 178,
`args.go`, `category.go`, `sort.go`); Go stdlib `flag.(*FlagSet).parseOne`, `log/slog`
(`text_handler.go` 161, `handler.go` 644), `text/tabwriter`, `os/signal.NotifyContext`,
`os/user.Current`; bubbles `progress/progress.go`, lipgloss `blending.go`, go-colorful v1.4.1
`colors.go`, colorprofile v0.4.3 `env.go`.

Everything the commands call into is ported by other areas and only referenced here:
`client`, `worktree`, `ticket`, `view`, `transport`, `codec`, `wire`, and core-rs (`packstore`,
`refstore`, `ingest`, `fstree`, `reference`, `amberpack`, `key`).

### 1.1 Client-side vs node-side

| Command | Side | Node internals touched |
|---|---|---|
| `cluster init` | node | `openNode` (identity/port files, `transport.BindIroh` with both ALPNs, `node.Open`: packstore, Pebble `meta`, Pebble paxos acceptor), `parseWeight` (statfs), `Node.InitCluster` |
| `serve` | node | `openNode`, `Node.View`, `Node.Start`, `Node.Close` |
| `node join` | node | `openNode`, `parseWeight`, `Node.Start`, `Node.Join` |
| `cluster status` | client; node-side only when `--store` is set and `--ticket` is not | `localTicket` → `node.OpenOffline` |
| `cluster ticket` | client; node-side when `--store` is set and `--ticket` is not | `localTicket` → `node.OpenOffline` |
| `catalog restore` | mixed: fetch is client-side, restore is node-side (`--store` required) | `node.OpenOffline`, `Node.RestoreCatalog` |
| `cluster replicas`, `token create`, `node remove/drain/weight/zone/repair`, `voter add/remove`, `transition *`, `gc *`, `catalog backup/backups` | client (admin frames) | none |
| `store push`, `store pull` | client + local core store | none (core packstore + refstore) |
| `refs`, `watch`, `ref get/delete`, `ls`, `cat` | client | none |
| `clone`, `init`, `fetch`, `pull`, `push` | client + working copy | none |
| `status`, `diff` | offline working copy | none |

Details of what each node-side path needs: §2.9.

### 1.2 Stale script

`scripts/e2e-loopback.sh:52,56` still runs `$B push --local local1 …` and `$B pull --local
local2 trees/demo`. In v0.1.9 those names are the working-copy commands, which have no
`--local` flag, so the script fails with `Incorrect Usage: flag provided but not defined: -local`.
Port the script against `store push` / `store pull`.

---

## 2. API used client-side

### 2.1 Process entry, exit codes

`main.go:34-52`:

```go
app := &cli.App{Name: "dstore", Usage: "a distributed amber store: cluster nodes and the client",
    Version: version, Flags: []cli.Flag{&cli.StringFlag{Name: "log-level", Value: "info",
    Usage: "debug|info|warn|error (a global flag: give it before the command)",
    EnvVars: []string{"DSTORE_LOG_LEVEL"}}},
    Commands: []*cli.Command{clusterCmd(), serveCmd(), tokenCmd(), nodeCmd(), voterCmd(), transitionCmd(), gcCmd(), catalogCmd(),
        storeCmd(), cloneCmd(), initCmd(), fetchCmd(), pullCmd(), pushCmd(), statusCmd(), diffCmd(),
        refsCmd(), watchCmd(), refCmd(), lsCmd(), catCmd()}}
if err := app.Run(os.Args); err != nil { fmt.Fprintln(os.Stderr, "dstore:", err); os.Exit(1) }
```

Exit behaviour:

| Situation | stdout | stderr | exit |
|---|---|---|---|
| success (including help and version output) | command output | logs | 0 |
| any error returned by `app.Run` | (help, for usage errors) | `dstore: <err>\n` | 1 |
| unknown command/help topic (`ExitCoder` 3, `help.go:278-286`, handled by `HandleExitCoder` in `command.go:278` before `main` sees it) | nothing | `No help topic for '<name>'\n` (no `dstore:` prefix) | 3 |
| Go runtime panic (only `cat NAME /`, §2.8 `cat`) | — | Go panic trace | 2 |
| killed by SIGINT in a command without `signalCtx` (`status`, `diff`) | — | — | signal |

*(verified)* `dstore nosuch`, `dstore help nosuch`, `dstore version`, `dstore cluster nosuch`,
`dstore ls help x` → stderr `No help topic for 'nosuch'` (resp. `'version'`, `'x'`), exit 3.

`version` defaults to `"dev"`; release images build with
`CGO_ENABLED=0 go build -trimpath -ldflags "-s -w -X main.version=${VERSION}"` where `VERSION` is
the git tag (`Dockerfile:10`, `.github/workflows/release.yml`), so a release prints
`dstore version v0.1.9`. Rust: take the version from a build-time env var (e.g.
`option_env!("DSTORE_VERSION")`), default `dev`.

### 2.2 urfave/cli v2.27.7 behaviour users observe

#### 2.2.1 App setup (`app.go:165-267`)

- `HelpName` = `dstore`; every top-level command gets `HelpName = "dstore <name>"`.
- The global `help` command (aliases `h`, Usage `Shows a list of commands or help for one
  command`, ArgsUsage `[command]`) is appended **last** to the command list; `--help, -h` (Usage
  `show help`, no default text) is appended to the root flags, then `--version, -v` (Usage
  `print the version`, no default text).
- Not used: bash completion, suggestions (`Suggest` false → no "Did you mean"), categories,
  authors, copyright, `DefaultCommand`, `Before`/`After`, `OnUsageError`, custom templates,
  `UseShortOptionHandling` (so `-ab` is not split: `flag provided but not defined: -ab`).
- Slice separator `,`.

#### 2.2.2 Run algorithm per level (`command.go:147-280`)

Applied recursively: root → command → subcommand. `args` = the arguments after this level's name.

1. Non-root only — `setup` (`command.go:113-145`): if it has no subcommand named `help`, append
   the **shared global** `helpCommand` to `Subcommands` (so every leaf ends up with
   `Subcommands = [help]`); append `--help, -h` to its flags; give each subcommand with an empty
   `HelpName` the name `<parent HelpName> <sub name>`.
2. Build the flag set: for each flag in definition order call `Apply` (environment is applied
   here and may fail, §2.2.5), define every name.
3. Parse `args` with Go's `flag.FlagSet.Parse` (ContinueOnError, output discarded), §2.2.4.
4. Error in 2 or 3 → write `Incorrect Usage: <err>\n\n` to **stdout**, then help: root →
   app help; otherwise `ShowCommandHelp(parentCtx, c.Name)`, which for a leaf renders the
   *SubcommandHelpTemplate* because its `Subcommands` now contains `help` — so the output has a
   `COMMANDS:` section listing only `help, h` (differs from `--help` output; verified §3.2).
   Return the error → `dstore: <err>`, exit 1.
5. `--help`/`-h` set → `helpCommand.Action(ctx)` and return.
6. Root only: `--version`/`-v` set → stdout `dstore version <version>\n`, return nil (exit 0).
   Help is checked first: `dstore -h -v` prints help. *(verified)* `--version refs` prints the
   version and exits 0.
7. `checkRequiredFlags` (`context.go:217-244`): a required flag is present iff `IsSet` for any of
   its names (§2.2.5). Missing names are collected in definition order; if any:
   `helpCommand.Action(ctx)` (its error is discarded), then return
   `Required flag "local" not set` (one; `%q` of the name) or `Required flags "seed, token" not set`
   (several; `%q` of the names joined by `", "`) (`errors.go:59-66`).
8. Subcommand dispatch: if there is a positional argument and the first one equals a subcommand
   name or alias exactly (case-sensitive), run that subcommand with all positional arguments.
9. Otherwise run `Action`; a command without one (every parent: `cluster`, `token`, `node`,
   `voter`, `transition`, `gc`, `catalog`, `store`, `ref`, and the root) runs
   `helpCommand.Action`. Then `HandleExitCoder(err)` (only acts on `ExitCoder` errors).

#### 2.2.3 The help action and its quirks (`help.go:32-85`, `250-290`)

`helpCommand.Action(ctx)`:

- Invoked *as* the `help`/`h` subcommand: switch to the parent context. If the first positional
  argument is non-empty → `ShowCommandHelp(parentCtx, firstArg)` (only the first argument is used).
- No non-empty first argument: root context → app help. A command with exactly one subcommand
  (a leaf, `[help]`) → *CommandHelpTemplate*. Otherwise → *SubcommandHelpTemplate*.
- Invoked as the fallback action with a positional argument (unknown subcommand) →
  `ShowCommandHelp(ctx, firstArg)`.

`ShowCommandHelp(ctx, name)`: search `ctx.Command.Subcommands` (the root's are the app commands).
Found → if it has subcommands and none is `help`, append the private `helpCommandDontUse`; append
`--help`; template = CommandHelpTemplate if it has no subcommands, else SubcommandHelpTemplate.
Not found → `Exit("No help topic for '<name>'", 3)`.

Observable consequences, all *(verified)*:

- App help for `dstore`, `dstore ""`, `dstore help`, `dstore h`, `dstore --help`, `-h`, `-help`.
- Leaf help for `dstore help refs`, `dstore refs help`, `dstore refs h`, `dstore refs -h`,
  `dstore refs --help`.
- **A leaf command whose first positional argument is `help` or `h` shows help instead of
  running**: `dstore refs help` does not list the prefix `help`; `dstore ls help x` exits 3 with
  `No help topic for 'x'`; `dstore watch h` prints watch help. Port this.
- `dstore cluster`, `dstore cluster --help`, `dstore cluster help`, `dstore cluster h` → full
  subcommand help.
- **`dstore help <parent>` prints truncated help** (and `dstore help cluster init` is the same as
  `dstore help cluster`): the template panics in `visibleCommandCategoryTemplate` because the
  parent's categories were never set up; `text/template` returns an error, the tabwriter is never
  flushed and the pending `COMMANDS:` cell is lost. Stdout is exactly (ends `…options]\n\n`),
  stderr empty, exit 0:

  ```text
  NAME:
     dstore cluster - init, status, ticket, replicas

  USAGE:
     dstore cluster [command options]

  ```

  Same shape (NAME + USAGE + one blank line) for `help token|node|voter|transition|gc|catalog|store|ref`.
  With `CLI_TEMPLATE_ERROR_DEBUG=1` set, urfave also writes
  `CLI TEMPLATE ERROR: template.ExecError{Name:"visibleCommandCategoryTemplate", Err:(*fmt.wrapError)(0x…)}`
  to stderr (pointer value varies, not portable; ignore).
- `dstore help help`:

  ```text
  NAME:
      help - Shows a list of commands or help for one command

  USAGE:
      help [command options] [command]

  OPTIONS:
     --help, -h  show help
  ```

  (four spaces: the help command's `HelpName` is `" help"` here).
- The shared `helpCommand` keeps the `HelpName` given by the first parent that set it up in this
  process. `dstore store push help` (no `--local`) goes down the required-flag path and prints the
  help command's own help with the name set by `store`: stdout

  ```text
  NAME:
     dstore store help - Shows a list of commands or help for one command

  USAGE:
     dstore store help [command options] [command]

  OPTIONS:
     --help, -h  show help
  ```

  stderr `dstore: Required flag "local" not set`, exit 1.
- Required flags missing and **no** positional argument → leaf help (CommandHelpTemplate) on
  stdout + error. Missing and a positional argument present → `ShowCommandHelp(ctx, firstArg)`
  fails silently, so only the error is printed. Verified: `store pull` (help + error) versus
  `store pull n` (error only); `node join` versus `node join extra`.

#### 2.2.4 Go flag parsing (`flag.go` `parseOne`, Go 1.26.5)

- Scan left to right. An argument `s` is a flag iff `len(s) >= 2 && s[0] == '-'`; the first
  non-flag argument **stops** flag parsing; it and everything after are positional. Flags after
  positional arguments are therefore positional *(verified)*: `cat a b --ticket x` → 4 args →
  `dstore: cat NAME PATH`; `watch p --ticket x` → no ticket; `diff a.txt --stat` → `--stat` is
  taken as a path.
- `-` alone is positional. `--` alone is consumed and stops parsing.
- One or two dashes are equivalent (`-ticket x` ≡ `--ticket x` ≡ `--ticket=x` ≡ `-ticket=x`).
  After the dashes, an empty name or a name starting with `-` or `=` → `bad flag syntax: <s>`
  (verified `---x`, `-=x`).
- `name=value` splits at the first `=` at index ≥ 1 of the name.
- Unknown name → `flag provided but not defined: -<name>` (always one dash in the message).
- Bool flag: without `=value` → true; with `=value` → urfave `boolValue.Set` =
  `strconv.ParseBool`, failure → `invalid boolean value "<value>" for -<name>: parse error`.
  A separate next argument is **not** consumed (`--no-relay false` makes `false` positional).
- Other flags take `=value` or the next argument, whatever it is (even `--foo` or `""`); none →
  `flag needs an argument: -<name>`. `Set` failure → `invalid value "<value>" for flag -<name>: <err>`
  where `<err>` is `parse error` (syntax) or `value out of range` (range) for int/int64/uint/float64,
  and always `parse error` for durations (verified `--jobs x`, `--garbage x`, `--gc-interval x`).
- Value syntax: `IntFlag`/`Int64Flag` `strconv.ParseInt(s, 0, 64)` (sign, `0x`/`0o`/`0b`/leading
  `0` octal prefixes, `_` separators only with a prefix); `UintFlag` `strconv.ParseUint(s, 0, 64)`;
  `Float64Flag` `strconv.ParseFloat(s, 64)` (`1e3`, `inf`, `NaN`, hex floats); `DurationFlag`
  `time.ParseDuration`. The command reads the value back with the same parsers, so the value is
  the same.
- Repeated scalar flag: last value wins. `StringSliceFlag.Set`: the first `Set` replaces the
  default, then the value is split on `,`, each part `TrimSpace`d and appended
  (`--advertise-addr "a, b" --advertise-addr c` → `[a b c]`).
- Global flags are only parsed before the command: `dstore refs --log-level debug` →
  `Incorrect Usage: flag provided but not defined: -log-level` *(verified)*.

#### 2.2.5 Environment variables and `IsSet`

`flagFromEnvOrFile` (`flag.go:378-393`) uses `syscall.Getenv`, which reports a variable set to the
empty string as *found*.

| Flag type | env found, value "" | env found, value non-empty | parse error text |
|---|---|---|---|
| String | value = "", `HasBeenSet` | value = env, `HasBeenSet` | — |
| Bool | value = false, `HasBeenSet` | `strconv.ParseBool` (`1 t T TRUE true True 0 f F FALSE false False`), `HasBeenSet` | `could not parse "<val>" as bool value from environment variable "<ENV>" for flag <name>: strconv.ParseBool: parsing "<val>": invalid syntax` |
| Int/Int64/Uint/Float64/Duration | ignored | parsed, `HasBeenSet` | `could not parse %q as int value from environment variable %q for flag %s: %s` (resp. `uint`, `float64`, `duration value`) — no dstore numeric flag has an env var |
| StringSlice | — | split on `,`, TrimSpace | — no dstore slice flag has an env var |

- An env parse error surfaces as an `Apply` error: `Incorrect Usage: <msg>` + leaf help with the
  `COMMANDS: help, h` section on stdout, `dstore: <msg>` on stderr, exit 1 *(verified
  `DSTORE_NO_DISCOVERY=maybe dstore refs`, `DSTORE_NO_TUI=maybe dstore store pull …`)*.
- Precedence: command line > environment > default. Env vars are applied only for the flags of
  the commands on the invoked path.
- The help default text is the flag's definition default, not the env value (`--pack-size` shows
  `(default: "2Gi")` even with `DSTORE_PACK_SIZE` set).
- `c.IsSet(name)`: parsed on the command line **or** `HasBeenSet` from the environment. So
  `AMBER_STORE=` (empty) satisfies the required `--local`; `c.String("local")` is then `""` and
  the stores open as `packstore` and `refs` **relative to the current directory**
  *(verified: `AMBER_STORE= dstore store pull` → `dstore: pull NAME`)*.
- `DSTORE_TICKET=` behaves like unset (`no cluster: …`, verified).
- `wcFlags` bind no env var (`wc.go:25-46`); `wcConfig` reads `DSTORE_TICKET` and
  `DSTORE_NO_DISCOVERY` itself (§2.5).

#### 2.2.6 Context lookups (`context.go:176-215`)

`c.String/Bool/Int/…(name)` search this command's flag set, then the ancestors'. An undefined name
yields the zero value, no error. Uses in dstore:

- `logger(c)` reads the root `--log-level` from any depth.
- `dialClusterLog` calls `c.String("store")` on commands without `--store` → `""`.
- `wcConfig` calls `c.IsSet("user")` on `fetch`/`pull` (no `--user`) → false.
- `voter add` reads `c.Bool("allow-unsafe")` without defining the flag → false.

#### 2.2.7 Help rendering (`help.go:345-407`, `template.go:3-99`, `flag.go:237-363`)

Output goes to **stdout** through `text/tabwriter.NewWriter(out, minwidth 1, tabwidth 8,
padding 2, padchar ' ', flags 0)`, flushed only after a successful template execution.

Templates (verbatim from `template.go`):

```text
helpNameTemplate      = `{{$v := offset .HelpName 6}}{{wrap .HelpName 3}}{{if .Usage}} - {{wrap .Usage $v}}{{end}}`
usageTemplate         = `{{if .UsageText}}{{wrap .UsageText 3}}{{else}}{{.HelpName}}{{if .VisibleFlags}} [command options]{{end}}{{if .ArgsUsage}} {{.ArgsUsage}}{{else}}{{if .Args}} [arguments...]{{end}}{{end}}{{end}}`
descriptionTemplate   = `{{wrap .Description 3}}`
visibleCommandTemplate = `{{ $cv := offsetCommands .VisibleCommands 5}}{{range .VisibleCommands}}
   {{$s := join .Names ", "}}{{$s}}{{ $sp := subtract $cv (offset $s 3) }}{{ indent $sp ""}}{{wrap .Usage $cv}}{{end}}`
visibleCommandCategoryTemplate = `{{range .VisibleCategories}}{{if .Name}}
   {{.Name}}:{{range .VisibleCommands}}
     {{join .Names ", "}}{{"\t"}}{{.Usage}}{{end}}{{else}}{{template "visibleCommandTemplate" .}}{{end}}{{end}}`
visibleFlagTemplate   = `{{range $i, $e := .VisibleFlags}}
   {{wrap $e.String 6}}{{end}}`

AppHelpTemplate = `NAME:
   {{template "helpNameTemplate" .}}

USAGE:
   {{if .UsageText}}{{wrap .UsageText 3}}{{else}}{{.HelpName}} {{if .VisibleFlags}}[global options]{{end}}{{if .Commands}} command [command options]{{end}}{{if .ArgsUsage}} {{.ArgsUsage}}{{else}}{{if .Args}} [arguments...]{{end}}{{end}}{{end}}{{if .Version}}{{if not .HideVersion}}

VERSION:
   {{.Version}}{{end}}{{end}}{{if .Description}}

DESCRIPTION:
   {{template "descriptionTemplate" .}}{{end}}
{{- if len .Authors}}

AUTHOR{{template "authorsTemplate" .}}{{end}}{{if .VisibleCommands}}

COMMANDS:{{template "visibleCommandCategoryTemplate" .}}{{end}}{{if .VisibleFlagCategories}}

GLOBAL OPTIONS:{{template "visibleFlagCategoryTemplate" .}}{{else if .VisibleFlags}}

GLOBAL OPTIONS:{{template "visibleFlagTemplate" .}}{{end}}{{if .Copyright}}

COPYRIGHT:
   {{template "copyrightTemplate" .}}{{end}}
`

CommandHelpTemplate = `NAME:
   {{template "helpNameTemplate" .}}

USAGE:
   {{template "usageTemplate" .}}{{if .Category}}

CATEGORY:
   {{.Category}}{{end}}{{if .Description}}

DESCRIPTION:
   {{template "descriptionTemplate" .}}{{end}}{{if .VisibleFlagCategories}}

OPTIONS:{{template "visibleFlagCategoryTemplate" .}}{{else if .VisibleFlags}}

OPTIONS:{{template "visibleFlagTemplate" .}}{{end}}
`

SubcommandHelpTemplate = `NAME:
   {{template "helpNameTemplate" .}}

USAGE:
   {{template "usageTemplate" .}}{{if .Category}}

CATEGORY:
   {{.Category}}{{end}}{{if .Description}}

DESCRIPTION:
   {{template "descriptionTemplate" .}}{{end}}{{if .VisibleCommands}}

COMMANDS:{{template "visibleCommandCategoryTemplate" .}}{{end}}{{if .VisibleFlagCategories}}

OPTIONS:{{template "visibleFlagCategoryTemplate" .}}{{else if .VisibleFlags}}

OPTIONS:{{template "visibleFlagTemplate" .}}{{end}}
`
```

No flag has a category, so `.VisibleFlagCategories` is empty and `visibleFlagTemplate` is used; no
command has a category, so the command list is `visibleCommandTemplate`. No command sets `Args` or
`UsageText`.

Functions: `wrap(s, offset)` with `wrapAt = 10000` (never wraps here); for multi-line input every
line after the first is prefixed by `offset` spaces, empty lines stay empty. `offset(s, n)` =
`len(s) + n` in **bytes**. `offsetCommands(cmds, n)` = max byte length of `join(names, ", ")` + n.

Command list rows: `"\n   " + names + spaces(cv − (len(names) + 3)) + usage`, cv =
`offsetCommands(visible, 5)`; no tabs; a command without Usage leaves trailing spaces
(`"   remove   "`, verified).

Flag rows (`stringifyFlag`, `flag.go:329-363`): `"\n   " + pn + "\t" + usageWithDefault + envHint`.

- `pn`: for each name `"-"` if one character else `"--"`, the name, then `" value"` if the flag
  takes a value (all but bool; placeholder from a backquoted word in Usage — none in dstore); names
  joined by `", "`. Slice flag: `pn + " [ " + pn + " ]"`.
- Default text: Bool `false`/`true` unless `DisableDefaultText` (help and version flags);
  String `%q` of the default if non-empty, else none; Int/Int64/Uint `%d` (always, e.g. `0`);
  Float64 `%v` (`0`); Duration `Duration.String()` (`4h0m0s`, `1h0m0s`); StringSlice quoted
  non-empty defaults joined `", "` (none here).
- `usageWithDefault = strings.TrimSpace(usage + " (default: " + text + ")")`, so a flag with empty
  usage shows `(default: false)` alone, and a string flag with no usage and no default shows
  nothing after the tab.
- `envHint` = `" [$A, $B]"` if env vars exist.
- Tabwriter: consecutive flag rows form one block; column 0 width = max rune count of
  `"   " + pn` + 2; every row is padded to that width even when the text after the tab is empty
  (trailing spaces, verified `"   --zone value" + 39 spaces`). Cell widths count UTF-8 runes
  (`tabwriter.go:416-417`).
- Flag order is definition order; `--help, -h` last (root: `--help, -h` then `--version, -v`).

NAME line: `HelpName` + (`" - " + Usage` if non-empty). Leaf/parent USAGE:
`HelpName + " [command options]" + (" " + ArgsUsage if non-empty)` (every command has at least
`--help`). Root USAGE: `dstore [global options] command [command options]`. VERSION section only at
the root. DESCRIPTION only for `watch`.

### 2.3 Logging (`--log-level`)

`main.go:54-68`:

- `logLevel(c)`: `strings.ToLower(c.String("log-level"))`: `debug` → Debug (−4), `warn` → Warn (4),
  `error` → Error (8), anything else (`info`, `""`, `verbose`, `WARNING`…) → Info (0), silently.
- `logger(c)` = `slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{Level: logLevel(c)}))`;
  a new handler on every call; no AddSource, no ReplaceAttr.

TextHandler line (`handler.go:270-322`): `time=<T> level=<L> msg=<M>` then ` <key>=<value>` per
attribute, `\n`; one `Write` per record under a per-handler mutex.

- `time`: the record time (local zone; `time.Now()` at the log call) truncated to milliseconds,
  layout `2006-01-02T15:04:05.000Z07:00` (always three fraction digits, `Z` when the offset is 0).
- `level`: `DEBUG`, `INFO`, `WARN`, `ERROR`; other values relative to the next lower base:
  `INFO+2`, `DEBUG-1`, `ERROR+4`.
- `msg`, keys and string values go through `needsQuoting` → `strconv.Quote`:
  - true if empty;
  - ASCII byte: true for `' '`, `'='`, `'"'` and control bytes `0x00-0x1F`; `\` does **not** force
    quoting; `0x7F` does not force quoting (it is in `safeSet`);
  - multi-byte: true for invalid UTF-8 (`RuneError`), `unicode.IsSpace`, `!unicode.IsPrint`.
- Values by kind: String → quoting rule; Int64 (all signed Go ints) and Uint64 → decimal; Float64 →
  `strconv.FormatFloat(f, 'g', -1, 64)` (`1.5`, `3.7e+06`); Bool → `true`/`false`; Duration →
  `Duration.String()` (then quoting rule); Time → the time format above; `[]byte` →
  **always** `strconv.Quote`; `encoding.TextMarshaler` → `MarshalText`, quoting rule; anything else
  `fmt.Sprintf("%+v", v)` + quoting rule (error → `Error()`, nil error → `<nil>`, `[]string` →
  `[a b]`).
- Attributes from `Logger.With` come first. An odd trailing argument becomes `!BADKEY=<value>`.

Verified lines (+02:00 and UTC records):

```text
time=2026-09-18T12:34:56.789+02:00 level=INFO msg=connected node=abcd0123 nodes=3 path=direct rtt=3ms
time=2026-09-18T10:34:56.005Z level=WARN msg="upload failed" node=abcd0123 objects=12 err="timeout: no recent network activity"
time=2026-09-18T12:34:56.789+02:00 level=DEBUG msg=x empty="" eq="a=b" quote="say \"hi\"" uni=é… tab="a\tb" bs=a\b f=1.5 b=true bytes="hi" i64=-5 u64=7 dur=1.5s nilerr=<nil>
time=2026-09-18T12:34:56.789+02:00 level=ERROR msg="msg with space" rate="3.0 MiB/s" addrs="[1.2.3.4:5 [::1]:6]" version=0102 in=1s took=1.235s
time=2026-09-18T12:34:56.789+02:00 level=INFO msg=""
time=2026-09-18T12:34:56.789+02:00 level=INFO msg="node started" node=abcd0123 id=abababababababababababababababababababababababababababababababab
time=2026-09-18T12:34:56.789+02:00 level=INFO msg=odd !BADKEY=lonely
time=2026-09-18T12:34:56.789+02:00 level=INFO+2 msg="custom level"
```

(The first record had time `12:34:56.789654321`: truncated, not rounded.)

The CLI logs nothing itself. Log records come from the client library (`client/client.go:109`
`connected`; `client/tree.go:65,68,85,124,135,144,189`; `client/objects.go:57,225,234,238,265,291,299`;
`client/watch.go:85,186,194,210,232`), `transport/iroh.go:158`
(`transport: mdns discovery unavailable`) and, node-side, the node (`n.log.With("node", ShortID)`).
Attribute kinds they use: `node`, `name`, `pattern`, `reason`, `path`, `version` (hex string),
`rate` (string from `client.Rate`, e.g. `1.0 MiB/s` → quoted) are strings; `nodes`, `objects`,
`present`, `upload`, `primaries`, `round`, `attempt`, `uploaded`, `owners`, `refs` are ints;
`bytes` is int64; `took`, `rtt` (both `.Round(time.Millisecond)`), `wait`, `in` are Durations;
`err`/`error` are errors. The message set belongs to the client area; this area provides the
handler and the level.

### 2.4 Signals

`signalCtx()` (`main.go:258-260`) = `signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)`,
created once per action that dials or opens a store (`defer cancel()`):

- The first SIGINT/SIGTERM cancels the context (cause `interrupt signal received`). The handler
  stays registered until the action returns, so **later signals are swallowed**: the process
  exits only when the command unwinds.
- Operations then fail with an error wrapping `context canceled` → `dstore: <err>`, exit 1.
  `watch` is the exception: `WatchRefs` just stops on cancellation, the action returns nil, exit 0.
- No `signalCtx` (default disposition, killed by the signal): `status`, `diff`, and every path
  that fails validation before calling it.
- TUI mode (§2.6): Bubble Tea runs `WithoutSignalHandler()`; with stdin a terminal the tty is in
  raw mode, so Ctrl+C is a key press: the model calls `cancel()` once and adds the event
  `cancelling` (WARN). SIGTERM still reaches `signalCtx`.

Rust: `tokio::signal::unix::signal(SignalKind::interrupt())` and `SignalKind::terminate()` feeding a
`tokio_util::sync::CancellationToken`; keep both streams registered for the whole action (tokio
never restores the default disposition, which gives the "swallow later signals" behaviour). Do not
register handlers for `status`/`diff`.

### 2.5 Shared client-side helpers

- **`dialCluster(ctx, c)`** (`client.go:41-43`) = `dialClusterLog(ctx, c, logger(c))`.
- **`dialClusterLog(ctx, c, log)`** (`client.go:57-74`). Ticket source, in order:
  `c.String("ticket")` non-empty → `ticket.Parse` (its errors are returned verbatim, §3.3);
  else `c.String("store")` non-empty → `localTicket(dir)` (node-side, §2.9); else
  `no cluster: set --ticket or $DSTORE_TICKET`. Then `dialTicket`.
- **`netOpts{Relay string; NoRelay, NoDiscovery bool}`**, `netOptsOf(c)` reads `relay`,
  `no-relay`, `no-discovery` (`client.go:46-54`).
- **`dialTicket(ctx, t, n, log)`** (`client.go:77-96`): `irohkey.GenerateSecretKey()` (fresh
  ephemeral identity every run) → `relayModeOf(n.Relay, n.NoRelay)` →
  `transport.BindIroh(ctx, IrohConfig{SecretKey, RelayMode, Discover: !n.NoDiscovery, Logger: log})`
  (no ALPNs: the client accepts nothing; `Announce` false; no Advertise/BindAddr/Loopback) →
  `client.Dial(ctx, client.Config{Endpoint: ep, Ticket: t, Logger: log, GCInterval: 4 * time.Hour})`
  (other fields default: Conns 4, Jobs 8, RequestTimeout 2 min, BatchBytes 16 MiB, WatchIdle 2 min,
  `client/client.go:23-45`); on `Dial` error close the endpoint. Every command closes the cluster
  with `defer cl.Close()`.
- **`relayModeOf(url, noRelay)`** (`main.go:123-137`): `noRelay` → nil (relays off; `--no-relay`
  wins over `--relay`); `url != ""` → `netaddr.ParseRelayURL(url)` → `relay.ModeCustomURLs(u)`;
  else `relay.ModeDefault()` (number0 production map, `go-iroh relay/relay.go:23-33,249-256`).
  `ParseRelayURL` is Go `url.Parse`, which is very lax *(verified)*:

  | input | Go result |
  |---|---|
  | `https://relay.example.com` | ok, `https://relay.example.com/` |
  | `http://10.0.0.1:3340` | ok, `http://10.0.0.1:3340/` |
  | `https://Relay.Example.COM:443/path?q=1` | ok, `https://relay.example.com:443/path?q=1` |
  | `foo` | ok, `foo` (relative URL accepted) |
  | `http://[::1` | error `failed to parse relay URL: parse "http://[::1": missing ']' in host` |
  | `:x` | error `failed to parse relay URL: parse ":x": missing protocol scheme` |

- **`admin(ctx, cl, req)`** (`client.go:98-108`): `cl.Admin(ctx, view.NodeID{}, req)` (zero id →
  any node, ranked by the client) → `codec.Unmarshal(reply, &AdminReply)` (unknown map keys are
  ignored, verified).
- **`adminAction(c, req)`** (`client.go:110-132`): `signalCtx`; `dialCluster`; `admin`; then print
  `r.Text + "\n"` if non-empty; each of `r.Names` + `"\n"`; if `len(r.Key) == 32`
  `"key %x\n"`. `View`, `GC`, `Token`, `Ticket` are ignored. Returns nil.
- **`printStatus(ctx, cl)`** (`client.go:134-193`), output format in §3.4. Status requests are
  sequential, in `v.Nodes` order, each with its own 5-second timeout (`cl.Status(sctx, id)`); an
  error → one line `<node line> — unreachable: <err>` (`fmt.Println` joins operands with single
  spaces; `—` is U+2014); `node.DecodeStatus` error → `<node line> — bad status`. `v` is
  `cl.View()` as learned at dial time.
- **`recordPayload(rec)`** (`client.go:195-201`): `amberpack.ParseRecord(rec)` then
  `amberpack.DecodePayload(r.Flags, r.Ulen, rec[amberpack.RecHeaderSize:])`.
- **`openLocal(c)`** (`client.go:209-221`): `packstore.Open(<local>/packstore, packstore.WithSync(true))`,
  `refstore.Open(<local>/refs, true)`; closes the packstore if the refstore fails.
- **`clusterGet(ctx, cl)`** (`client.go:454-468`): a getter for one key: first result of
  `cl.Get(ctx, [k])` → error returned, or `recordPayload`; no result and `missing()` non-empty →
  `object <64 hex> not found`; no result and nothing missing → `no data`.
- **`hexDecode(s)`** (`client.go:554-574`): `len(s)/2` bytes, case-insensitive; an odd trailing
  nibble is silently dropped (`"abc"` → `ab`, `"a"` → empty); any other character →
  `bad hex "<s>"` (`%q`). Only used for `--expected-version`.
- **`resolveTicket(flag, stored, env)`** (`wc.go:52-59`): the first non-empty of the three, else
  `no cluster: set --ticket or $DSTORE_TICKET`.
- **`wcConfig(c, stored)`** (`wc.go:64-90`), exact order:
  1. `cfg = *stored` (or zero for clone/init);
  2. `cfg.Ticket = resolveTicket(c.String("ticket"), cfg.Ticket, os.Getenv("DSTORE_TICKET"))` — error returns first;
  3. `if c.IsSet("relay") { cfg.Relay = c.String("relay") }` (can set it to `""`);
  4. `if c.IsSet("no-relay") { cfg.NoRelay = c.Bool("no-relay") }` (`--no-relay=false` overrides a stored true);
  5. `if c.IsSet("no-discovery") { cfg.NoDiscovery = … } else if stored == nil { if v, err := strconv.ParseBool(os.Getenv("DSTORE_NO_DISCOVERY")); err == nil { cfg.NoDiscovery = v } }` — invalid env values are ignored silently (verified); existing working copies ignore the env var;
  6. `if c.IsSet("user") { cfg.User = c.String("user") }`.
- **`dialConfig(ctx, cfg, log)`** (`wc.go:92-98`): `ticket.Parse(cfg.Ticket)` → `dialTicket` with
  `cfg.Relay/NoRelay/NoDiscovery`.
- **`openWC()`** (`wc.go:101-107`): `os.Getwd()` → `worktree.Open(wd)` (nearest ancestor holding
  `.dstore`).
- **`pushUser(c, cfg)`** (`wc.go:111-125`): `c.String("user")` if non-empty, else `cfg.User`, else
  `user.Current().Username` (error ignored → `""`); `reference.ValidateUser(u)` error →
  `user: <err>` (e.g. `user: user must not be empty`). `--user ""` equals no flag here. The release
  binary is `CGO_ENABLED=0`, so `user.Current` = `/etc/passwd` lookup by uid, else `$USER` provided
  `$USER` and `$HOME` are both non-empty (`os/user/lookup_stubs.go`). Rust: `getpwuid_r(getuid())`
  → `pw_name`, falling back to `$USER` when `$USER` and `$HOME` are non-empty.
- **`withCluster(c, title, fn)`** (`wc.go:227-250`): `openWC` → `wcConfig(c, &tr.Config)` →
  `signalCtx` → `runTransfer(ctx, c, title+" "+tr.Config.Name, …)` whose body is `dialConfig` →
  `fn(ctx, tr, cl, prog)` → on success `tr.RefreshTicket(cl)` (rewrites `.dstore/config` when the
  ticket derived from the view differs; its error is returned); `defer cl.Close()`,
  `defer tr.Close()`.
- **`describeChange(ch)`** (`wc.go:405-417`): `ch.Path`, plus `/` if `IsDir(ch.New)` or
  (`ch.New == nil && IsDir(ch.Old)`); `TypeChanged` → `" (" + TypeName(old) + " → " + TypeName(new) + ")"`
  (U+2192); `ModeChanged` → `fmt.Sprintf(" (%04o → %04o)", old.Mode&0o7777, new.Mode&0o7777)`;
  others: path only. `TypeName`: `file`, `directory`, `symlink`, `fifo`, `socket`, `char device`,
  `block device`, else `type %#o` of `mode & S_IFMT` (e.g. `type 0170000`).
- **`filterPaths(root, changes, args)`** (`wc.go:478-501`): for each argument, in order:
  `filepath.Abs(a)` (joins the cwd and `Clean`s), `filepath.Rel(root, abs)`; error, `rel == ".."`
  or `rel` starting with `../` → `<a> is outside the working copy` (the argument as typed);
  prefix = `filepath.ToSlash(rel)`. All arguments are validated before filtering. A change is kept
  (original order) if some prefix is `.`, equals `ch.Path`, or `ch.Path` starts with `prefix + "/"`.
  Vectors in §5.4.
- **`isTerminal(f)`** (`tui.go:38-41`): `f.Stat()` succeeds and the mode has `os.ModeCharDevice`.
  **Not isatty**: `/dev/null` is a terminal here *(verified)*, so `dstore push 2>/dev/null` runs
  the TUI renderer into `/dev/null`. Rust: `fstat` + `S_ISCHR`.

### 2.6 Progress display (`tui.go`)

`runTransfer(ctx, c, title, fn)` (`tui.go:27-32`): `c.Bool("no-tui") || !isTerminal(os.Stderr)` →
`runPlain`, else `runTUI`. `fn(ctx, log, prog)` runs the dial and the transfer. `--no-tui`
(`DSTORE_NO_TUI`) exists on `store push`, `store pull`, `clone`, `init`, `fetch`, `pull`, `push`.
Titles: `push <NAME>`, `pull <NAME>` (store), `clone <NAME>`, `init <NAME>`, `fetch <name>`,
`pull <name>`, `push <name>` (working copy: the stored name).

`latest` (`tui.go:45-60`): a mutex-guarded `client.ProgressReport`; `set` is the `client.Progress`
callback (called per record by the client, so it must stay cheap), `get` returns a copy.

**runPlain** (`tui.go:62-86`): meter = `newRateMeter(5s)`; a goroutine ticks every 5 s
(`time.NewTicker`): `fmt.Fprintln(os.Stderr, statusLine(r, meter.add(now, r.Bytes)))`. The first
status line appears after 5 s; nothing is printed at the end. `fn` gets `logger(c)` (TextHandler on
stderr). After `fn` returns the ticker goroutine is stopped and joined; `fn`'s error is returned.

**rateMeter.add(t, n)** (`tui.go:103-114`): append `{t, n}`; while `len(samples) > 2` and
`t − samples[1].t >= window` drop `samples[0]`; `first = samples[0]`; `d = t − first.t`; if
`d <= 0 || n < first.n` → 0; else `float64(n − first.n) / d.Seconds()`.

**statusLine(r, rate)** (`tui.go:117-130`):
`fmt.Sprintf("%d/%d objects", r.Objects, r.TotalObjects)`; if `r.TotalBytes > 0` append
`fmt.Sprintf("  %s / %s", HumanBytes(r.Bytes), HumanBytes(r.TotalBytes))`, else if `r.Bytes > 0`
append `"  " + HumanBytes(r.Bytes)`; append `fmt.Sprintf("  %s/s", HumanBytes(int64(rate)))`
(truncates toward zero); if `rate > 0` and `left := r.TotalBytes − r.Bytes > 0` append
`"  eta " + time.Duration(float64(left)/rate*float64(time.Second)).Round(time.Second).String()`.

**HumanBytes(n)** (`client/progress.go:170-181`): `n < 1024` → `"%d B"` (negatives too); else
`div, exp := 1024, 0; for m := n/1024; m >= 1024; m /= 1024 { div *= 1024; exp++ }` →
`fmt.Sprintf("%.1f %ciB", float64(n)/float64(div), "KMGTPE"[exp])`. No unit promotion:
`1048575` → `1024.0 KiB`. (core-rs `examples/amber-store.rs:989 human_bytes` promotes at 1023.95 —
do **not** reuse it.) `client.Rate(bytes, took)`: `took <= 0` → `-`, else
`HumanBytes(int64(float64(bytes)/took.Seconds())) + "/s"`.

**fraction(r)** (`tui.go:134-143`): `TotalBytes > 0` → `Bytes/TotalBytes`; else `TotalObjects > 0`
→ `Objects/TotalObjects`; else 0; clamped to [0, 1].

**runTUI** (`tui.go:311-333`):
1. `ctx, cancel := context.WithCancel(ctx)`; `in = os.Stdin` if `isTerminal(os.Stdin)` else nil
   (nil → no input reader, no raw mode, no keyboard).
2. `p := tea.NewProgram(newUIModel(title, &l, cancel), tea.WithOutput(os.Stderr), tea.WithInput(in), tea.WithoutSignalHandler())`.
3. `log := slog.New(&teaHandler{level: logLevel(c), send: p.Send})`; goroutine:
   `err := fn(ctx, log, l.set); result <- err; p.Send(doneMsg{err: err})`.
4. `p.Run()` error → `cancel(); <-result; return runErr` (the transfer's error is discarded);
   else `return <-result`.

Bubble Tea v2 renders inline (not the alternate screen) with its "cursed" renderer; the last frame
stays on the terminal after quitting. It also emits and later restores terminal modes (cursor
visibility, bracketed paste, modifyOtherKeys/Kitty keyboard flags when input is enabled,
synchronized-output queries). These bytes are an implementation detail, not a contract.

**uiModel** (`tui.go:171-287`):
- `newUIModel`: `start = now = time.Now()`, `width = 80`, `bar = progress.New(progress.WithDefaultBlend())`,
  `bar.SetWidth(60)`, `meter = newRateMeter(5s)`, per-node meters created on first sight.
- `Init` → `tick()` = `tea.Tick(100ms, tickMsg)`.
- `Update`: `WindowSizeMsg` → `width = w`, `bar.SetWidth(min(max(w−4, 20), 80))`;
  `KeyPressMsg` with `String() == "ctrl+c"` and not yet cancelling → `cancelling = true`,
  `cancel()`, append event `{time.Now(), WARN, "cancelling"}`; `tickMsg` → `observe(t)` and re-tick;
  `eventMsg` → append (keep the last `maxEvents = 12`); `doneMsg` → `done = true`, `err`,
  `observe(time.Now())`, `tea.Quit`.
- `observe(now)`: `now`; `rep = l.get()`; `rate = meter.add(now, rep.Bytes)`; for each node its
  meter `add(now, n.Bytes)` → `nodeRates[id]`.
- `View` (exact, `\n` after every line):
  1. `titleStyle.Render(title) + "  " + faintStyle.Render("elapsed " + now.Sub(start).Round(time.Second).String())`
  2. `bar.ViewAs(fraction(rep))`
  3. `statusLine(rep, rate)`
  4. if `len(rep.Nodes) > 0`: `faintStyle.Render(fmt.Sprintf("%-10s %-7s %7s %7s %-15s %11s %12s", "node", "path", "rtt", "batches", "state", "sent", "rate"))`,
     then per node (client order: sorted by id bytes)
     `fmt.Sprintf("%-10s %-7s %7s %7d %-15s %11s %10s/s", view.ShortID(n.ID), path, rtt, n.InFlight, nodeState(n), HumanBytes(n.Bytes), HumanBytes(int64(nodeRates[n.ID])))`
     with `path` = `direct` if `n.Direct` else `relay`, `rtt` = `-` if `n.RTT <= 0` else
     `n.RTT.Round(time.Millisecond).String()` (so 250 µs → `0s`).
  5. if events: `faintStyle.Render("events")`, then `formatEvent(e)` per event.
  6. if done: `errStyle.Render("failed: " + err.Error())` or `okStyle.Render("done")`.
- `nodeState(n)`: `InFlight == 0` → `idle`; `Awaiting == InFlight` → `waiting for ack`; else `sending`.
- `formatEvent(e)`: `e.at.Format("15:04:05") + "  " + e.text` (local zone); level ≥ ERROR →
  `errStyle`, ≥ WARN → `warnStyle`, else unstyled.
- Styles (lipgloss v2 before terminal downsampling): `titleStyle` Bold → `\x1b[1m…\x1b[m`;
  `faintStyle` Faint → `\x1b[2m…\x1b[m`; `warnStyle` Color("3") → `\x1b[33m…\x1b[m`;
  `errStyle` Color("1") → `\x1b[31m…\x1b[m`; `okStyle` Color("2") → `\x1b[32m…\x1b[m`.
- Progress bar (bubbles v2.2.1 `progress.go:232-430`): Full `▌` (U+258C), Empty `░` (U+2591),
  `PercentFormat = " %3.0f%%"` of the clamped percent × 100 (`"  50%"`, `"   0%"`, `" 100%"`),
  text width = display width of that string (5); `tw = max(0, width − 5)`;
  `fw = clamp(int(math.Round(tw*percent)), 0, tw)` (half away from zero);
  blend = `lipgloss.Blend1D(tw*2, #5A56E0, #EE6FF8)`; filled cell i (0-based) is
  `\x1b[38;2;R;G;B;48;2;R;G;Bm▌\x1b[m` with foreground `blend[2i]`, background `blend[2i+1]`; the
  empty part is one run `\x1b[38;2;96;96;96m` + `░`×(tw−fw) + `\x1b[m`; then the percentage.
- `Blend1D(steps, a, b)` (`lipgloss blending.go`): steps ≤ 2 → the stops themselves; otherwise
  one segment, factor `j/(steps−1)`, `colorful.BlendLab(a, b, f).Clamped()`. go-colorful v1.4.1:
  sRGB channel c → linear `c <= 0.04045 ? c/12.92 : ((c+0.055)/1.055)^2.4`; XYZ =
  `[0.41239079926595948 0.35758433938387796 0.18048078840183429; 0.21263900587151036 0.71516867876775593 0.072192315360733715; 0.019330818715591851 0.11919477979462599 0.95053215224966058]`;
  Lab with D65 `[0.95047, 1.0, 1.08883]`, `f(t) = t > (6/29)^3 ? cbrt(t) : t/3*(29/6)^2 + 4/29`,
  `L = 1.16 f(y) − 0.16`, `a = 5(f(x/0.95047) − f(y))`, `b = 2(f(y) − f(z/1.08883))`; interpolate
  linearly; inverse `finv(t) = t > 6/29 ? t³ : 3*(6/29)^2*(t − 4/29)`, XYZ→linear matrix
  `[3.2409699419045214 −1.5373831775700935 −0.49861076029300328; −0.96924363628087983 1.8759675015077207 0.041555057407175613; 0.055630079696993609 −0.20397695888897657 1.0569715142428786]`,
  delinear `v <= 0.0031308 ? 12.92v : 1.055 v^(1/2.4) − 0.055`; clamp to [0,1]; 8-bit =
  `uint8(v*255 + 0.5)`. Stops enter as 16-bit RGBA divided by 65535.
- Bubble Tea downsamples colours to the profile `colorprofile.Detect(stderr, environ)` finds
  (`TERM`, `COLORTERM`, `NO_COLOR`, `CLICOLOR`, `CLICOLOR_FORCE`, `TTY_FORCE`, tmux/screen rules;
  `colorprofile env.go`). Verified frame in §3.6.

**teaHandler** (`tui.go:335-374`): `Enabled(l) = l >= level`; `Handle`: text = `r.Message`, then
for the `With` attributes and then the record attributes `" " + key + "=" + attrValue(a)`;
`send(eventMsg{at: r.Time, level: r.Level, text})`; `WithAttrs` appends; `WithGroup` returns the
handler unchanged (no key prefixes).
`attrValue(a)`: key `bytes` with kind Int64 (any signed int) → `HumanBytes`; otherwise
`a.Value.String()` (strings raw, ints decimal, Duration.String, errors `Error()`); if it contains
`' '` or `'\t'` → `%q` (`strconv.Quote`), else raw (empty, `=`, `"` are not quoted). Vectors §5.4.

### 2.7 Admin and status CBOR types

Encoding: `codec.Marshal` = fxamacker `cbor.CanonicalEncOptions()` (RFC 8949 core deterministic:
definite lengths, shortest integer heads, map keys sorted by encoded bytes, floats in the shortest
form that preserves the value). Decoding: `cbor.DecOptions{}` (unknown keys ignored). All structs
use integer keys (`keyasint`).

```go
// node/admin.go:19-35 — carried in a TAdmin frame's Params
type AdminRequest struct {
    Op          string   `cbor:"0,keyasint"`
    Node        []byte   `cbor:"1,keyasint,omitempty"`
    Weight      uint32   `cbor:"2,keyasint,omitempty"`
    Zone        string   `cbor:"3,keyasint,omitempty"`
    Replicas    uint8    `cbor:"4,keyasint,omitempty"`
    Dead        bool     `cbor:"5,keyasint,omitempty"`
    AllowUnsafe bool     `cbor:"6,keyasint,omitempty"`
    Force       bool     `cbor:"7,keyasint,omitempty"`
    Key         []byte   `cbor:"8,keyasint,omitempty"`
    Garbage     float64  `cbor:"9,keyasint,omitempty"`
    Tolerate    bool     `cbor:"10,keyasint,omitempty"`
    Forwarded   bool     `cbor:"11,keyasint,omitempty"`
    Pause       bool     `cbor:"12,keyasint,omitempty"`
    Rate        uint64   `cbor:"13,keyasint,omitempty"`
    Names       []string `cbor:"14,keyasint,omitempty"`
}
// node/admin.go:38-46 — carried in a TAdminReply frame's Status field
type AdminReply struct {
    Text   string   `cbor:"0,keyasint,omitempty"`
    Token  []byte   `cbor:"1,keyasint,omitempty"`
    View   []byte   `cbor:"2,keyasint,omitempty"`
    Names  []string `cbor:"3,keyasint,omitempty"`
    Key    []byte   `cbor:"4,keyasint,omitempty"`
    Ticket string   `cbor:"5,keyasint,omitempty"`
    GC     []byte   `cbor:"6,keyasint,omitempty"`
}
```

`omitempty` drops zero values: nil/empty slices, `""`, 0, `false`, `0.0`. `Op` is always present.
The CLI never sets `Force`, `Forwarded`, `Rate`, `Names` (ops `rate-cap` and `keep` have no CLI
command). Hex vectors in §3.8.

Server side (`Node.Admin`, `admin.go:72-261`); every op first requires a view (`no view`). Errors
become `TErr` frames: `wire.Error` codes kept, others `CodeUnavailable` with `err.Error()`; the
client surfaces them as errors → `dstore: <text>`.

| Op (CLI command) | Server action | Reply the CLI prints |
|---|---|---|
| `token-create` (`token create`) | `cat.CreateToken(Weight)` | `Text` = hex of the 32-byte token (`Token` also set) |
| `cluster-ticket` (`cluster ticket`) | ticket {ClusterID, Incarnation, Members: this node (its endpoint `Addrs`) first, then up to 3 view nodes (may repeat this node)} | `Ticket` (CLI parses and re-encodes it) |
| `node-remove`, `node-drain`, `node-weight`, `node-zone` | `nodeIDOf(Node)` (`node id must be 32 bytes`); run as lease holder (forwarded) `maint.nodeChange` | `transition proposed at epoch %d` |
| `node-repair` | broadcast `TViewChanged{Node}` (5 s per member, errors ignored) | `repair scheduled: holders will refill <ShortID>` |
| `voter-add` / `voter-remove` | as holder: `addVoter` (errors e.g. `already a voter`, `a voter change is in progress`) / `removeVoter(AllowUnsafe)` | `voters now %d` |
| `replicas` | as holder: propose transition reason `replicas %d`; `Replicas == 0` → `replicas must be ≥ 1` | `transition proposed` |
| `transition-status` | — | `transitionText(v)` (below) |
| `transition-abort` | CAS view: clear `Pending` (nothing pending is not an error) | `transition aborted` |
| `transition-refreeze` | `maint.refreeze` | `participants re-frozen` |
| `transition-pause` / `-resume` | CAS view `RebalancePause` | `ok` |
| `gc-run` | as holder: `Garbage > 0` → `resweep(Garbage)`, else `runCycle(Tolerate, true)`; then read the GC register | `statusText(g)` |
| `gc-status` | read GC register | `statusText(g)` |
| `gc-hold` (`gc hold` Pause=true, `gc release` Pause=false) | CAS GC `Hold` | `ok` |
| `gc-why` | `len(Key) != 32` → `key must be 32 bytes`; `n.Why` | `Names` (references reaching the key) |
| `catalog-backup` | `maint.backupCatalog` | `Text` `backup written`, `Key` → `key <hex>` |
| `catalog-backups` | meta scan of `backup/` | `Names` = hex keys (last 24) |
| other | — | error `unknown admin op <op>` (CodeBadRequest) |

`transitionText(v)` (`maintenance.go:539-557`): no pending → `idle`, or
`idle (%d ramp(s) pending)` when `len(v.Ramps) > 0`; pending not frozen →
`id %d (%s): adopting, %d/%d acked` (ID, Reason, len(ParticipantsAck), len(AllMembers)); frozen →
`id %d round %d (%s): %d/%d done, waiting for %v` (waiting = ShortIDs not done, Go `%v` of a
`[]string` → `[ab12cd34 ef56ab78]`).

`statusText(st)` (`gc.go:1276-1309`): `epoch %d <phase>`; if `BarrierAt > 0`
`, barrier <RFC3339 in the node's local zone>`; if phase ∈ {barrier, mark, sweep} `, %d acked`; if
sweep `, %d/%d swept (wave %d)`; if Hold `, ON HOLD`; if Error `, last error: %s`; if missing
`, %d missing objects`; if `len(Last) > 0` `; last epoch %d: %d live records, %d bytes freed`.
Phases (`catalog.PhaseName`): `idle`, `barrier`, `mark`, `sweep`, `aborted`, else `phase(%d)`.

```go
// node/status.go:14-52 — a TStatusReply frame's Status field
type VoterStat struct {
    ID       []byte `cbor:"0,keyasint"`
    Calls    uint64 `cbor:"1,keyasint"`
    Failures uint64 `cbor:"2,keyasint"`
    P99ms    int64  `cbor:"3,keyasint"`
}
type Status struct {
    ID            []byte      `cbor:"0,keyasint"`
    Epoch         uint64      `cbor:"1,keyasint"`
    Incarnation   uint64      `cbor:"2,keyasint"`
    Packs         int         `cbor:"3,keyasint"`
    Records       uint64      `cbor:"4,keyasint"`
    Bytes         int64       `cbor:"5,keyasint"`
    Pins          int         `cbor:"6,keyasint"`
    Unreachable   [][]byte    `cbor:"7,keyasint,omitempty"`
    PendingPacks  int         `cbor:"8,keyasint"`
    Transition    string      `cbor:"9,keyasint,omitempty"`
    GC            string      `cbor:"10,keyasint,omitempty"`
    LeaseHolder   []byte      `cbor:"11,keyasint,omitempty"`
    Voters        []VoterStat `cbor:"12,keyasint,omitempty"`
    Writable      bool        `cbor:"13,keyasint"`
    FreeBytes     int64       `cbor:"14,keyasint"`
    TotalBytes    int64       `cbor:"15,keyasint"`
    Puts          uint64      `cbor:"16,keyasint"`
    Gets          uint64      `cbor:"17,keyasint"`
    RefPuts       uint64      `cbor:"18,keyasint"`
    BytesIn       uint64      `cbor:"19,keyasint"`
    BytesOut      uint64      `cbor:"20,keyasint"`
    Amnesiac      bool        `cbor:"21,keyasint,omitempty"`
    Retired       bool        `cbor:"22,keyasint,omitempty"`
    ScrubAgeSec   int64       `cbor:"23,keyasint,omitempty"`
    LastLive      uint64      `cbor:"24,keyasint,omitempty"`
    Corrupt       int         `cbor:"25,keyasint,omitempty"`
    IsHolder      bool        `cbor:"26,keyasint,omitempty"`
    UnauditedKeys int         `cbor:"27,keyasint,omitempty"`
    Watchers      int         `cbor:"28,keyasint,omitempty"`
}
```

`DecodeStatus(b)` = `codec.Unmarshal(b, &st)`. A nil `[]byte` field without `omitempty` encodes
as CBOR null (`f6`, see the zero-status vector in §3.8), so the decoder must accept null for byte
strings. `node.ShortID(b)`: `len(b) != 32` → `?`, else hex of the first 4 bytes.
`printStatus` uses `Epoch, Packs, Records, Bytes, Pins, PendingPacks, FreeBytes, IsHolder, Amnesiac,
Retired, Unreachable, GC, Transition, Voters`; the rest are for scrapers.

### 2.8 Command reference

#### 2.8.1 Flag groups (definition order matters for help and required-flag messages)

| Group | Flags (name · type · default · usage · env · required) |
|---|---|
| global (root) | `log-level` · string · `"info"` · `debug\|info\|warn\|error (a global flag: give it before the command)` · `DSTORE_LOG_LEVEL`; then `help, h` · bool · `show help`; `version, v` · bool · `print the version` |
| `storeFlag()` | `store` · string · — · `store directory` · `DSTORE_STORE` |
| `ticketFlag()` | `ticket` · string · — · `cluster ticket (dstore1…) or comma-separated node ids, found by discovery` · `DSTORE_TICKET` |
| `noDiscoveryFlag()` | `no-discovery` · bool · false · `neither announce this endpoint nor resolve node ids by discovery (mDNS and, with relays, number0's DNS)` · `DSTORE_NO_DISCOVERY` |
| `clientFlags()` | `ticketFlag`; `relay` · string · — · `relay URL for the fallback path (default: the built-in relay map)`; `no-relay` · bool · `direct addresses only, no relay`; `noDiscoveryFlag` |
| `netFlags()` | `relay` (as above); `no-relay` (as above); `advertise-addr` · StringSlice · — · `direct address to advertise, ip or ip:port (repeatable)`; `loopback` · bool · `advertise 127.0.0.1 only (single-machine tests)`; `bind` · string · `UDP address to bind, ip:port`; `noDiscoveryFlag` |
| `nodeFlags()` | `storeFlag`; `paxos-dir` · string · `acceptor state directory (default <store>/paxos; put it on its own device)`; `rate` · int64 · 0 · `reconcile copy rate in bytes/s (0 = unlimited)`; `jobs` · int · 0 · `parallelism (0 = cores)`; `min-free` · int64 · 0 · `free bytes below which uploads are refused (0 = 5% or 100 GiB)`; `pack-size` · string · `"2Gi"` · `size at which the active pack is sealed (bytes or Ki/Mi/Gi/Ti); applies to packs written from now on` · `DSTORE_PACK_SIZE`; `gateway` · bool · `also serve the transport-iroh ALPN (not implemented in this version)`; `gc-interval` · duration · 4h · (no usage); `put-ttl` · duration · 1h · (no usage); then `netFlags` |
| `wcFlags()` | `ticket` · string · `cluster ticket (dstore1…) or comma-separated node ids; overrides the stored one for this run`; `relay` · string · `relay URL for the fallback path (default: the built-in relay map)`; `no-relay` · bool · `direct addresses only, no relay`; `no-discovery` · bool · `neither announce this endpoint nor resolve node ids by discovery` — **no env vars** |
| `jobsFlag()` | `jobs` · int · 0 · `parallelism (0 = cores)` |
| `noTUIFlag()` | `no-tui` · bool · false · `plain log lines instead of the progress display` · `DSTORE_NO_TUI` |
| `localStoreFlag()` | `local` · string · — · `local store directory (layout: <dir>/packstore, <dir>/refs)` · `AMBER_STORE` · **Required** |

Top-level command order: `cluster`, `serve`, `token`, `node`, `voter`, `transition`, `gc`,
`catalog`, `store`, `clone`, `init`, `fetch`, `pull`, `push`, `status`, `diff`, `refs`, `watch`,
`ref`, `ls`, `cat`, then `help, h`.

Unless stated otherwise an action ignores extra positional arguments, and "adminAction {…}" means:
validate arguments, `signalCtx`, `dialCluster`, send the request, print the reply as in §2.5.
Validation that happens **before** dialing produces its error even without a ticket (verified).

#### 2.8.2 `cluster` — Usage `init, status, ticket, replicas`; subcommands `init`, `status`, `ticket`, `replicas`

**`cluster init`** — node-side. Usage `create a cluster on this store and print its ticket; then run serve`.
Flags: `nodeFlags` + `replicas` · uint · 3 · `R: owners per object`; `min-replicas` · uint · 0 ·
`owners that must hold an object before a write succeeds (default max(R−1, 2))` (U+2212);
`weight` · string · `"auto"` · `capacity in GiB, or auto`; `zone` · string ·
`failure domain (default: the node id)`; `allow-unsafe` · bool · `allow min-replicas 1`.
Action (`main.go:294-316`): `signalCtx` → `openNode` (§2.9) → `parseWeight(--weight, --store)` →
`n.InitCluster(ctx, uint8(replicas), uint8(min-replicas), w, zone, allow-unsafe)` (uint8
truncation: `--replicas 256` → 0 → server default 3) → stdout:

```text
node id: <64 hex>
cluster ticket: dstore1…
now run: dstore serve --store <--store as given>
```

The ticket is `{ClusterID, Incarnation, Members: [{ID: self, Addrs: endpoint Addrs()}]}`. Errors:
`no store directory: set --store or $DSTORE_STORE`, `--pack-size: …`, `bad --advertise-addr "<v>"`,
`bad --bind "<b>"`, `--weight auto: cannot stat the store filesystem`, `bad weight "<s>" (GiB or auto)`,
`node: already a member of a cluster`, `node: min_replicas below 2 needs --allow-unsafe`. The weight
is parsed after the store and endpoint were opened (files already created).

**`cluster status`** — client-side (node-side via `--store` without `--ticket`). Usage
`view, epoch, reachability, disk, transition, gc, voters`. Flags: `clientFlags`, `storeFlag`.
Action: `signalCtx` → `dialCluster` → `printStatus` (§3.4).

**`cluster ticket`** — client-side (node-side via `--store`). Usage
`print the bootstrap ticket, or with --ids the member ids to hand out instead`. Flags:
`clientFlags`, `storeFlag`, `ids` · bool · `print comma-separated node ids (the short form, found by discovery)`.
Action (`main.go:337-366`): `signalCtx`; if `--ticket == "" && --store != ""` → `localTicket(store)`
(no network); else `dialCluster` (no ticket → `no cluster: …`), `admin {Op: "cluster-ticket"}`,
`ticket.Parse(r.Ticket)`; print `t.IDs()` if `--ids` else `t.Encode()`, plus `\n`. The Docker
entrypoint uses the exit status of `dstore cluster ticket --store "$STORE" >/dev/null 2>&1` to decide
whether a store already belongs to a cluster.

**`cluster replicas`** — client-side. Usage `change R (a transition that copies 1/R of the store)`;
ArgsUsage `R`. Flags: `clientFlags`, `yes` · bool · `do not ask`. Action (`main.go:373-387`):
`strconv.ParseUint(First(), 10, 8)` error → `replicas R` (also `""`, `300`, `+3`, `0x3`; `0` is
accepted); unless `--yes`: stdout `changing R to %d moves about 1/%d of every node's data; continue? [y/N] `
(no newline), `fmt.Scanln(&ans)` (first whitespace-separated token of one line; EOF → `""`),
`!strings.HasPrefix(strings.ToLower(ans), "y")` → `aborted`; then adminAction
`{Op: "replicas", Replicas: uint8(r)}` → `transition proposed`. The prompt comes before dialing
*(verified: `yes please` → proceeds → `no cluster: …`; `Y` without newline proceeds; `n` and EOF →
`dstore: aborted`)*.

#### 2.8.3 `serve` — node-side. Usage `run a node`. Flags: `nodeFlags`

Action (`main.go:414-432`): `signalCtx` → `openNode` → if `n.View() == nil`: close,
`this store is not a member of a cluster: run cluster init or node join` → `n.Start(ctx)` → wait for
the signal → `n.Log().Info("shutting down")` → `return n.Close()`.

#### 2.8.4 `token` — Usage `join tokens`; subcommand `create`

**`token create`** — client-side. Usage `create a single-use join token`. Flags: `clientFlags`,
`weight` · uint · 0 · `weight the token imposes on the joiner (GiB)`. Action (`main.go:444-458`):
`signalCtx` → `dialCluster` → `admin {Op: "token-create", Weight: uint32(weight)}` →
`fmt.Println(r.Text)` (prints `\n` even when Text is empty — unlike `adminAction`).

#### 2.8.5 `node` — Usage `join, remove, drain, weight, zone, repair`

**`node join`** — node-side. Usage `join a cluster with this store and keep serving`. Flags:
`nodeFlags` + `seed` · string · Required · `cluster ticket (dstore1…) or comma-separated node ids, found by discovery`;
`token` · string · Required · `join token (hex)`; `weight` · string · `"auto"` ·
`capacity in GiB, or auto`; `zone` · string · (no usage); `no-ramp` · bool ·
`join at full weight in one step`; `no-vote` · bool · `hold no catalog (a relay-only or archive box)`.
Action (`main.go:486-533`): `signalCtx` → `ticket.Parse(--seed)` → `hex.DecodeString(--token)`
(strict, even length) and length 32, else `token must be 32 bytes of hex` → `openNode` →
`parseWeight` (close on error) → `n.Start(ctx)` → if `n.View() == nil`: for each ticket member
with a 32-byte id: `n.Join(ctx with 10 min timeout, id, m.Addrs, tok, w, zone, no-vote, no-ramp)`;
first success ends the loop; all failed → close, `join: <last error>` (if every member id was
malformed the loop runs zero joins and continues as if joined) → stdout `node id: <64 hex>` →
wait for the signal → `n.Close()`.

Subcommands without Usage, each `ArgsUsage` as given, flags `clientFlags` (+ extras), validation
`view.ParseNodeID(First())` first (64 hex characters only; error `view: bad node id "<s>"`, also for
iroh's base32 form and for `""`):

| Command | ArgsUsage | Extra flags | Request | Extra validation |
|---|---|---|---|---|
| `node remove` | `ID` | `dead` · bool · `the node is gone: remove its vote first`; `allow-unsafe` · bool · (no usage) | `{Op: "node-remove", Node, Dead, AllowUnsafe}` | — |
| `node drain` | `ID` | — | `{Op: "node-drain", Node}` | — |
| `node weight` | `ID GiB` | — | `{Op: "node-weight", Node, Weight: uint32}` | `strconv.ParseUint(Get(1), 10, 32)` error → `weight ID GiB` |
| `node zone` | `ID ZONE` | — | `{Op: "node-zone", Node, Zone: Get(1)}` (empty zone allowed) | — |
| `node repair` | `ID` | — | `{Op: "node-repair", Node}` | — |

#### 2.8.6 `voter` — Usage `change a node's vote after join`

| Command | ArgsUsage | Flags | Request |
|---|---|---|---|
| `voter add` | `ID` | `clientFlags` | `{Op: "voter-add", Node, AllowUnsafe: false}` |
| `voter remove` | `ID` | `clientFlags`, `allow-unsafe` · bool · (no usage) | `{Op: "voter-remove", Node, AllowUnsafe}` |

Both validate with `view.ParseNodeID` before dialing; adminAction; reply `voters now %d`.

#### 2.8.7 `transition` — Usage `status, abort, refreeze, pause, resume`

Subcommands `status`, `abort`, `refreeze`, `pause`, `resume`: no Usage, no ArgsUsage, flags
`clientFlags`; adminAction `{Op: "transition-<name>"}`; replies in §2.7.

#### 2.8.8 `gc` — Usage `run, status, why, hold, release`; subcommand order `run`, `status`, `hold`, `release`, `why`

| Command | ArgsUsage | Flags | Request |
|---|---|---|---|
| `gc run` | — | `clientFlags`, `tolerate-missing` · bool · (no usage), `garbage` · float64 · 0 · `re-sweep only, at this dead ratio` | `{Op: "gc-run", Tolerate, Garbage}` |
| `gc status` | — | `clientFlags` | `{Op: "gc-status"}` |
| `gc hold` | — | `clientFlags` | `{Op: "gc-hold", Pause: true}` |
| `gc release` | — | `clientFlags` | `{Op: "gc-hold"}` (Pause false, omitted) |
| `gc why` | `KEY` | `clientFlags` | `{Op: "gc-why", Key}` after `hex.DecodeString(First())` (strict) of length 32, else `why KEY (64 hex chars)` |

#### 2.8.9 `catalog` — Usage `backup, restore, backups`; order `backup`, `backups`, `restore`

- **`catalog backup`**: no Usage; `clientFlags`; adminAction `{Op: "catalog-backup"}` → `backup written` + `key <hex>`.
- **`catalog backups`**: Usage `list the backup object keys a node remembers`; `clientFlags`;
  `{Op: "catalog-backups"}` → one hex key per line.
- **`catalog restore`** — mixed/node-side. Usage `force-write every reference of a backup object`;
  ArgsUsage `KEY|FILE`; flags `clientFlags`, `storeFlag`. Action (`main.go:652-695`): `signalCtx`;
  if `os.ReadFile(arg)` succeeds → data = file contents; else `hex.DecodeString(arg)` of length 32
  or `restore KEY|FILE` (so `""` → `restore KEY|FILE`); `dialCluster` (ticket, or the local ticket
  of `--store`); `cl.Get(ctx, [k])`: an error result → returned; each record →
  `data = recordPayload(record)`; `data == nil` → `backup object not found in the cluster`. Then,
  for both sources, `--store == ""` → `restore runs on a voter: give --store` (checked **after**
  fetching); `node.OpenOffline(store)`; `written, err := n.RestoreCatalog(ctx, data)`; stdout
  `%d references written\n` (also printed when err != nil); return err (e.g. `<name>: <err>`).

#### 2.8.10 `store` — Usage `push and pull between a standalone local store and the cluster`

**`store push`** — client-side + local store. Usage
`build a tree from PATH into the local store and push it under NAME`; ArgsUsage `PATH NAME`. Flags:
`clientFlags`, `localStoreFlag` (Required), `user` · string · `user identity recorded in the reference`,
`force` · bool · `replace unconditionally`, `expected-version` · string ·
`CAS: the version ref get printed (hex); omit to require the name to be new`, `jobs` · int · 0 ·
(no usage), `noTUIFlag`. Action (`client.go:244-299`), in order:
1. `c.NArg() != 2` → `push PATH NAME`; `reference.ValidateName(NAME)` (texts §3.3).
2. `signalCtx`; `openLocal` (store errors); `ingest.Dir(st, PATH, ingest.Opts{Jobs: --jobs})`.
3. stderr `built %s: %d new objects\n` (`root.String()[:16]`, `stats.Stored`).
4. `cond := client.Cond{Force: --force}`; if not force: `Versioned = true`,
   `ExpectedVersion = hexDecode(--expected-version)` when non-empty (error `bad hex "…"`), else nil
   (the name must be new).
5. `runTransfer("push "+NAME)`: `dialClusterLog(ctx, c, log)`;
   `cl.Push(ctx, st, root, NAME, --user, cond, prog)`.
6. Error: `*client.CASMismatch` → `%w (pull first, or --force)`, i.e.
   `cas mismatch: current key <hex> (pull first, or --force)` or
   `cas mismatch: reference is absent (pull first, or --force)`; other errors verbatim.
7. Local record: `reference.Reference{Name: NAME, Key: root[:], User: --user, CreatedAt: time.Now().UnixNano()}.Encode()`
   → `refs.Put(NAME, enc)`; encoding and put errors are ignored (an invalid `--user` just skips it).
   This record's `CreatedAt` differs from the cluster's record.
8. stdout `pushed %s: %d objects, %d uploaded, version %x\n` (`ps.Keys`, `ps.Uploaded`, `ps.Version`).

Consequence of the order: without a ticket the tree is still built into the local store before
`no cluster: …` is returned.

**`store pull`** — Usage `pull the tree under NAME into the local store`; ArgsUsage `NAME`. Flags:
`clientFlags`, `localStoreFlag`, `jobs` · int · 0 · (no usage; **unused**), `noTUIFlag`. Action
(`client.go:309-340`): `First() == ""` → `pull NAME`; `signalCtx`; `openLocal`;
`runTransfer("pull "+NAME)`: `dialClusterLog`; `ps, err = cl.Pull(ctx, st, NAME, prog)`; then
`refs.Put(NAME, ps.Record)` (error returned); stdout
`pulled %s: root %s, %d objects fetched (%d bytes)\n` (`ps.Root.String()` — full 64 hex,
`ps.Fetched`, `ps.Bytes`).

#### 2.8.11 Working-copy commands (`wc.go`)

**`clone`** — Usage `clone the tree under NAME into DIR (default: the last segment of NAME) as a working copy`;
ArgsUsage `NAME [DIR]`. Flags: `wcFlags`, `user` · string · `user identity stored for pushes`,
`jobsFlag` (**unused** by the action), `noTUIFlag`. Action (`wc.go:133-170`): `First() == ""` →
`clone NAME [DIR]`; `ValidateName`; `dir = Get(1)` or `path.Base(NAME)`; `cfg = wcConfig(c, nil)`;
`cfg.Name = NAME`; `signalCtx`; `runTransfer("clone "+NAME)`: `dialConfig`;
`cfg.Ticket = worktree.TicketFromView(cl.View()).Encode()` (the stored ticket is the one derived
from the view, not the one given); `worktree.Clone(ctx, cl, dir, cfg, prog)`. stdout
`cloned %s into %s: root %s, %d objects fetched (%d bytes)\n` (NAME, dir, `fr.Key.String()[:16]`,
`fr.Stats.Fetched`, `fr.Stats.Bytes`). Worktree errors: `<dir> is not a directory`,
`<dir> is not empty`, `<abs> is inside the working copy at <root>`,
`client: unknown reference: <NAME>`.

**`init`** — Usage `make the current directory a working copy of NAME, with nothing synced yet`;
ArgsUsage `NAME`. Flags: `wcFlags`, `user` · string · `user identity stored for pushes`, `noTUIFlag`.
Action (`wc.go:180-221`): `First() == ""` → `init NAME`; `ValidateName`; `wcConfig(c, nil)`;
`cfg.Name`; `os.Getwd()`; `signalCtx`; `runTransfer("init "+NAME)`: `dialConfig`; derived ticket;
`worktree.Init(ctx, cl, wd, cfg, prog)`. stdout, reference exists:
`initialised working copy of %s; the reference exists (root %s): status shows everything as new, pull merges\n`;
otherwise `initialised working copy of %s; the reference does not exist yet: push creates it\n`.

**`fetch`** — Usage `record the reference's current tree as the remote and fetch its objects`;
no ArgsUsage. Flags: `wcFlags`, `noTUIFlag`. Action: `withCluster(c, "fetch", tr.Fetch)`. stdout:
`!fr.Exists` → `%s does not exist on the cluster\n`; `fr.UpToDate` → `%s: up to date (%s)\n`
(key[:16]); else `fetched %s: root %s, %d objects fetched (%d bytes)\n`.

**`pull`** — Usage `fetch and apply the cluster's changes over the working directory`. Flags:
`wcFlags`, `force` · bool · `take the cluster's side on conflicting paths`, `jobsFlag`, `noTUIFlag`.
Action (`wc.go:286-311`): `withCluster(c, "pull", tr.Pull(ctx, cl, --force, --jobs, prog))`. If
`errors.Is(err, worktree.ErrConflict)`: stderr `conflicts:\n` then per conflict
`  %s (local: %s, cluster: %s)\n` (`cf.Path`, `cf.Local.Kind`, `cf.Incoming.Kind`; kinds `new`,
`deleted`, `modified`, `type`, `mode`, `meta`), then the error →
`dstore: conflicting changes: resolve them, or --force to take the cluster's side`. The ticket is
not refreshed on error. Success: `r.UpToDate` → `already up to date\n`; else
`pulled: %d paths updated` + (`, %d conflicts taken from the cluster` if conflicts) + `\n`.
`ErrNoRemote` → `the reference does not exist on the cluster: nothing to pull`.

**`push`** — Usage `build the working directory's tree, upload it and write the reference`. Flags:
`wcFlags`, `user` · string · `user identity recorded in the reference (default: the stored one, then the OS user)`,
`force` · bool · `replace the reference unconditionally`, `jobsFlag`, `noTUIFlag`. Action
(`wc.go:323-348`): `withCluster(c, "push", …)` body: `pushUser(c, tr.Config)` (after dialing,
verified) → `tr.Push(ctx, cl, u, --force, --jobs, prog)`. stdout: `r.Nothing` → `nothing to push\n`;
`r.Recovered` → `%s already holds %s (an earlier push completed); state updated\n` (root[:16]); else
`pushed %s: root %s, %d objects, %d uploaded, version %x\n`. Worktree errors
`the cluster's tree moved since your last sync: pull first, or --force`,
`the reference was deleted on the cluster: --force to recreate it`,
`reference changed on the cluster since your last fetch: pull first, or --force (<cas error>)`.

**`status`** — offline. Usage `list the working directory's changes since the last sync, and whether the cluster moved`.
Flags: `jobsFlag` only. Action (`wc.go:357-399`): `openWC`; `tr.Status(--jobs)`; stdout:
- `reference %s, synced to %s\n` (name, `State.Base.String()[:16]`);
- `remote: up to date` | `remote: the reference does not exist on the cluster` |
  `remote: moved since your last fetch (+%d ~%d -%d; run pull)` (counts over `st.Incoming`: Added,
  anything else, Deleted);
- if changes: `changes:` then `fmt.Printf("  %-9s %s\n", ch.Kind, describeChange(ch))`;
- if `st.MetaOnly > 0`: `%d paths differ only in mtime, ownership or xattrs`;
- if no changes and `MetaOnly == 0`: `nothing to push`.

**`diff`** — offline. Usage `unified diffs of the working directory against the last synced tree`;
ArgsUsage `[PATH...]`. Flags: `remote` · bool · `against the tree last fetched from the cluster`;
`incoming` · bool · `the last synced tree against the fetched one (what pull would apply)`;
`stat` · bool · `one line per changed path with line counts`; `jobsFlag`. Action
(`wc.go:430-473`): both `--remote` and `--incoming` → `--remote and --incoming exclude each other`
(before `openWC`); `openWC`; `--incoming`: no remote → `ErrNoRemote`,
`changes = worktree.DiffTrees(tr.Get, Base, Remote)`, old = new = `TreeSource{Get}`; `--remote`: no
remote → `ErrNoRemote`, `worktree.Scan(Root, Remote, Get, time.Now(), jobs)`, old = TreeSource, new =
`DiskSource{Root}`; default: `Scan(Root, Base, Get, State.SyncedAt, jobs)`, old TreeSource, new
DiskSource. `NArg > 0` → `filterPaths`. `--stat` → `worktree.Stat(os.Stdout, …)`, else
`worktree.Unified(os.Stdout, …)` (formats belong to the worktree area; verified examples §3.5).

#### 2.8.12 Reference and tree commands (`client.go`)

**`refs`** — Usage `list references`; ArgsUsage `[PREFIX]`; `clientFlags`. Action: `signalCtx`;
`dialCluster`; `cl.RefList(ctx, First())`; per ref
`fmt.Printf("%s\t%x\t%s\t%s\n", r.Name, r.Key, time.Unix(0, r.CreatedAt).Format(time.RFC3339), r.User)`
(local zone, seconds precision, `Z` when the offset is 0).

**`watch`** — Usage `watch references matching a glob and print each change until interrupted`;
ArgsUsage `PATTERN`; Description (two lines, verbatim in §3.1); `clientFlags`. Action
(`client.go:378-403`): `First() == ""` → `watch PATTERN` (before dialing); `signalCtx`;
`dialCluster`; `for ch, err := range cl.WatchRefs(ctx, pattern, nil)`: err → return it; `ch.Synced`
→ nothing; `ch.Deleted` → `%s\tdeleted\n`; else the `refs` line format. Returns nil when the
context is cancelled (exit 0). Go writes each line with one unbuffered `write`; Rust must flush
per line.

**`ref`** — Usage `get or delete a reference`.
- **`ref get`**: no Usage; ArgsUsage `NAME`; `clientFlags`. `cl.RefGet(ctx, First())` (no local name
  validation) → stdout `name %s\nkey %x\nversion %x\nuser %s\ncreated %s\n` (created as in `refs`).
  Unknown name → `client: unknown reference`.
- **`ref delete`**: no Usage; ArgsUsage `NAME`; flags `clientFlags`, `force` · bool · (no usage),
  `expected-version` · string · (no usage). Action (`client.go:429-448`): `signalCtx`;
  `dialCluster`; `cond = Cond{Force: --force}`; if `--expected-version != ""`: `hexDecode` (error
  after dialing) → `Versioned = true, ExpectedVersion`; else if not force → `Force = true`
  (default is unconditional). `cl.RefDelete(ctx, First(), cond)`; no output. A CAS failure prints
  the raw `cas mismatch: …`.

**`ls`** — Usage `list a directory of a pushed tree`; ArgsUsage `NAME [PATH]`; `clientFlags`. Action
(`client.go:476-508`): `signalCtx`; `dialCluster`; `cl.RefGet(ctx, First())`;
`key.Parse(r.Ref.Key)`; `get = clusterGet`; `dir = root`; if `p := Get(1); p != "" && p != "/"`:
`fstree.ResolvePath(root, strings.Trim(p, "/"), get)`; `fstree.CollectEntries(dir, get)`; print
`string(e.Name) + "\n"` per entry (name order). fstree errors verbatim, e.g.
`fstree: "sub": not a directory`, `fstree: "a/../b": ".." is not supported`, wrapped `entry not found`.

**`cat`** — Usage `write a file of a pushed tree to stdout`; ArgsUsage `NAME PATH`; `clientFlags`.
Action (`client.go:518-550`): `NArg() != 2` → `cat NAME PATH` (before dialing); `signalCtx`;
`dialCluster`; `RefGet`; `key.Parse`; `e = fstree.ResolveEntry(root, strings.Trim(PATH, "/"), get)`;
`len(e.ContentKey) != 32` → `not a regular file with content`; `key.Parse(e.ContentKey)`;
`fstree.WriteContent(os.Stdout, k, get)`. Quirks: `ResolveEntry` returns a nil entry for an empty
path, so `cat NAME /` (or `""`, `.`, `//`) **panics** in Go (nil pointer dereference, exit 2).
Directories carry a content key, so `cat NAME somedir` reaches `WriteContent`, which fails with
`<64 hex> is not a file-content object (type <type>)`; symlinks and devices have no content key →
`not a regular file with content`.

### 2.9 Node-side paths and what they need

**`openNode(ctx, c)`** (`main.go:229-256`), used by `cluster init`, `serve`, `node join`:
1. `--store == ""` → `no store directory: set --store or $DSTORE_STORE`.
2. `packSize(c)` (`main.go:214-227`): `TrimSpace(--pack-size)`, empty → `"2Gi"`; `parseSize`
   error → `--pack-size: <err>`; `n <= 0` → `--pack-size: %d is not a positive size`.
3. `os.MkdirAll(store, 0o755)`.
4. `bindNodeEndpoint(ctx, c, store)` (`main.go:161-207`):
   - `loadOrCreateKey(<store>/identity)`: exists → `irohkey.ParseSecretKey(TrimSpace(contents))`;
     missing → `GenerateSecretKey`, `MkdirAll(parent, 0755)`, write `hex(seed) + "\n"` mode 0600;
     other read errors returned.
   - `transport.IrohConfig{SecretKey, ALPNs: [wire.ALPNClient, wire.ALPNCluster], RelayMode, Loopback: --loopback, Discover: !--no-discovery, Announce: same, Logger}`.
   - `--advertise-addr` non-empty: each value `netip.ParseAddrPort`, else `netip.ParseAddr` → port 0
     (fixed after bind), else `bad --advertise-addr "<v>"`.
   - `--bind` non-empty: `netip.ParseAddrPort` or `bad --bind "<b>"`; else if `<store>/port` parses
     (`ParseUint(TrimSpace, 10, 16)`) → bind `0.0.0.0:<port>`.
   - `transport.BindIroh`; if `--bind` was empty write `<store>/port` = `"<local UDP port>\n"`
     mode 0644 (errors ignored).
5. `node.Open(node.Config{StoreDir, PaxosDir: --paxos-dir, Endpoint, Logger: logger(c), Jobs, Rate, MinFree, Gateway, GCInterval, PutTTL, SegmentSize})`
   (`node/node.go:201-270`): `<store>/packstore` (sync, segment size), Pebble `<store>/meta`
   (`meta/meta.go:35`, error `meta: open <dir>: …`), random `store_id` in meta, Pebble paxos
   acceptor at `--paxos-dir` or `<store>/paxos` (`paxos/acceptor.go:68`), view from meta key
   `view`, transport pool, paxos proposer, catalog, maintenance, gc, reconciler. On error the
   endpoint is closed.

**`parseWeight(s, dir)`** (`main.go:262-275`): `auto` → `diskTotal(dir)` = statfs
`Blocks * Bsize` (`client.go:31-37`); 0 → `--weight auto: cannot stat the store filesystem`;
`uint32(total >> 30)`. Otherwise `ParseUint(s, 10, 32)` or `bad weight "<s>" (GiB or auto)`.

**Node methods used**: `Node.InitCluster` (acceptor `SetMarker`/`Install`, meta `view` and
registration keys, catalog `InitView` through the paxos proposer), `Node.Start` (accept loop, view
refresh, maintenance, reconcile, self-entry loops), `Node.Join` (cluster-ALPN `TJoin` with the token,
expects `TViewReply`), `Node.View`, `Node.ID`, `Node.Endpoint().Addrs()`, `Node.Log()`,
`Node.Close`, `Node.RestoreCatalog` (`maintenance.go:985-1000`: CBOR `[]backupEntry`, catalog
`RefPut(Force)` per entry, CAS GC `BarrierAt = now, Hold = true`).

**`localTicket(dir)`** (`main.go:394-405`), reached from `cluster ticket --store`,
`cluster status --store`, and `catalog restore` with `--store` but no ticket:
`node.OpenOffline(dir)` (`node.go:776-783`): read `<dir>/identity` or
`node: no identity in <dir>: open <dir>/identity: no such file or directory` *(verified)*; then
`node.Open` with an `offlineEndpoint` (zero id, `Dial` → `node: offline`, no addresses), which opens
the packstore and both Pebble databases (Pebble holds directory locks, so expect failures while a
node process owns the store); `n.View() == nil` → `this store is not a member of a cluster`; else
`worktree.TicketFromView(v)` (first four nodes with their view addresses); close.

**Recommendation.** The Rust project has no Pebble, paxos or node. Keep every node-side command
and flag in the command table so help, flag parsing and argument validation stay identical, and make
the actions return an explicit error once they would open a store (open decision §8). `--store` on
`cluster status`/`cluster ticket`/`catalog restore` needs either the same error or a read-only
Pebble reader for the `view` key.

---

## 3. Byte formats and text formats (verbatim)

### 3.1 Help output *(verified, `version` = `dev`)*

All on stdout, exit 0. Several lines end in spaces (padded empty cells); keep them.

`dstore`, `dstore --help`, `-h`, `-help`, `help`, `h`, `""`:

```text
NAME:
   dstore - a distributed amber store: cluster nodes and the client

USAGE:
   dstore [global options] command [command options]

VERSION:
   dev

COMMANDS:
   cluster     init, status, ticket, replicas
   serve       run a node
   token       join tokens
   node        join, remove, drain, weight, zone, repair
   voter       change a node's vote after join
   transition  status, abort, refreeze, pause, resume
   gc          run, status, why, hold, release
   catalog     backup, restore, backups
   store       push and pull between a standalone local store and the cluster
   clone       clone the tree under NAME into DIR (default: the last segment of NAME) as a working copy
   init        make the current directory a working copy of NAME, with nothing synced yet
   fetch       record the reference's current tree as the remote and fetch its objects
   pull        fetch and apply the cluster's changes over the working directory
   push        build the working directory's tree, upload it and write the reference
   status      list the working directory's changes since the last sync, and whether the cluster moved
   diff        unified diffs of the working directory against the last synced tree
   refs        list references
   watch       watch references matching a glob and print each change until interrupted
   ref         get or delete a reference
   ls          list a directory of a pushed tree
   cat         write a file of a pushed tree to stdout
   help, h     Shows a list of commands or help for one command

GLOBAL OPTIONS:
   --log-level value  debug|info|warn|error (a global flag: give it before the command) (default: "info") [$DSTORE_LOG_LEVEL]
   --help, -h         show help
   --version, -v      print the version
```

`dstore --version`, `-v`: `dstore version dev\n`.

`dstore cluster` (also `cluster --help`, `cluster help`, `cluster h`):

```text
NAME:
   dstore cluster - init, status, ticket, replicas

USAGE:
   dstore cluster [command options]

COMMANDS:
   init      create a cluster on this store and print its ticket; then run serve
   status    view, epoch, reachability, disk, transition, gc, voters
   ticket    print the bootstrap ticket, or with --ids the member ids to hand out instead
   replicas  change R (a transition that copies 1/R of the store)
   help, h   Shows a list of commands or help for one command

OPTIONS:
   --help, -h  show help
```

`dstore token --help` (and `dstore token`):

```text
NAME:
   dstore token - join tokens

USAGE:
   dstore token [command options]

COMMANDS:
   create   create a single-use join token
   help, h  Shows a list of commands or help for one command

OPTIONS:
   --help, -h  show help
```

`dstore node --help`:

```text
NAME:
   dstore node - join, remove, drain, weight, zone, repair

USAGE:
   dstore node [command options]

COMMANDS:
   join     join a cluster with this store and keep serving
   remove   
   drain    
   weight   
   zone     
   repair   
   help, h  Shows a list of commands or help for one command

OPTIONS:
   --help, -h  show help
```

`dstore voter --help`:

```text
NAME:
   dstore voter - change a node's vote after join

USAGE:
   dstore voter [command options]

COMMANDS:
   add      
   remove   
   help, h  Shows a list of commands or help for one command

OPTIONS:
   --help, -h  show help
```

`dstore transition --help`:

```text
NAME:
   dstore transition - status, abort, refreeze, pause, resume

USAGE:
   dstore transition [command options]

COMMANDS:
   status    
   abort     
   refreeze  
   pause     
   resume    
   help, h   Shows a list of commands or help for one command

OPTIONS:
   --help, -h  show help
```

`dstore gc --help`:

```text
NAME:
   dstore gc - run, status, why, hold, release

USAGE:
   dstore gc [command options]

COMMANDS:
   run      
   status   
   hold     
   release  
   why      
   help, h  Shows a list of commands or help for one command

OPTIONS:
   --help, -h  show help
```

`dstore catalog --help`:

```text
NAME:
   dstore catalog - backup, restore, backups

USAGE:
   dstore catalog [command options]

COMMANDS:
   backup   
   backups  list the backup object keys a node remembers
   restore  force-write every reference of a backup object
   help, h  Shows a list of commands or help for one command

OPTIONS:
   --help, -h  show help
```

`dstore store --help` (and `dstore store`):

```text
NAME:
   dstore store - push and pull between a standalone local store and the cluster

USAGE:
   dstore store [command options]

COMMANDS:
   push     build a tree from PATH into the local store and push it under NAME
   pull     pull the tree under NAME into the local store
   help, h  Shows a list of commands or help for one command

OPTIONS:
   --help, -h  show help
```

`dstore ref --help` (and `dstore ref`):

```text
NAME:
   dstore ref - get or delete a reference

USAGE:
   dstore ref [command options]

COMMANDS:
   get      
   delete   
   help, h  Shows a list of commands or help for one command

OPTIONS:
   --help, -h  show help
```

`dstore serve --help`:

```text
NAME:
   dstore serve - run a node

USAGE:
   dstore serve [command options]

OPTIONS:
   --store value                                      store directory [$DSTORE_STORE]
   --paxos-dir value                                  acceptor state directory (default <store>/paxos; put it on its own device)
   --rate value                                       reconcile copy rate in bytes/s (0 = unlimited) (default: 0)
   --jobs value                                       parallelism (0 = cores) (default: 0)
   --min-free value                                   free bytes below which uploads are refused (0 = 5% or 100 GiB) (default: 0)
   --pack-size value                                  size at which the active pack is sealed (bytes or Ki/Mi/Gi/Ti); applies to packs written from now on (default: "2Gi") [$DSTORE_PACK_SIZE]
   --gateway                                          also serve the transport-iroh ALPN (not implemented in this version) (default: false)
   --gc-interval value                                (default: 4h0m0s)
   --put-ttl value                                    (default: 1h0m0s)
   --relay value                                      relay URL for the fallback path (default: the built-in relay map)
   --no-relay                                         direct addresses only, no relay (default: false)
   --advertise-addr value [ --advertise-addr value ]  direct address to advertise, ip or ip:port (repeatable)
   --loopback                                         advertise 127.0.0.1 only (single-machine tests) (default: false)
   --bind value                                       UDP address to bind, ip:port
   --no-discovery                                     neither announce this endpoint nor resolve node ids by discovery (mDNS and, with relays, number0's DNS) (default: false) [$DSTORE_NO_DISCOVERY]
   --help, -h                                         show help
```

`dstore clone --help`:

```text
NAME:
   dstore clone - clone the tree under NAME into DIR (default: the last segment of NAME) as a working copy

USAGE:
   dstore clone [command options] NAME [DIR]

OPTIONS:
   --ticket value  cluster ticket (dstore1…) or comma-separated node ids; overrides the stored one for this run
   --relay value   relay URL for the fallback path (default: the built-in relay map)
   --no-relay      direct addresses only, no relay (default: false)
   --no-discovery  neither announce this endpoint nor resolve node ids by discovery (default: false)
   --user value    user identity stored for pushes
   --jobs value    parallelism (0 = cores) (default: 0)
   --no-tui        plain log lines instead of the progress display (default: false) [$DSTORE_NO_TUI]
   --help, -h      show help
```

`dstore init --help`:

```text
NAME:
   dstore init - make the current directory a working copy of NAME, with nothing synced yet

USAGE:
   dstore init [command options] NAME

OPTIONS:
   --ticket value  cluster ticket (dstore1…) or comma-separated node ids; overrides the stored one for this run
   --relay value   relay URL for the fallback path (default: the built-in relay map)
   --no-relay      direct addresses only, no relay (default: false)
   --no-discovery  neither announce this endpoint nor resolve node ids by discovery (default: false)
   --user value    user identity stored for pushes
   --no-tui        plain log lines instead of the progress display (default: false) [$DSTORE_NO_TUI]
   --help, -h      show help
```

`dstore fetch --help`:

```text
NAME:
   dstore fetch - record the reference's current tree as the remote and fetch its objects

USAGE:
   dstore fetch [command options]

OPTIONS:
   --ticket value  cluster ticket (dstore1…) or comma-separated node ids; overrides the stored one for this run
   --relay value   relay URL for the fallback path (default: the built-in relay map)
   --no-relay      direct addresses only, no relay (default: false)
   --no-discovery  neither announce this endpoint nor resolve node ids by discovery (default: false)
   --no-tui        plain log lines instead of the progress display (default: false) [$DSTORE_NO_TUI]
   --help, -h      show help
```

`dstore pull --help`:

```text
NAME:
   dstore pull - fetch and apply the cluster's changes over the working directory

USAGE:
   dstore pull [command options]

OPTIONS:
   --ticket value  cluster ticket (dstore1…) or comma-separated node ids; overrides the stored one for this run
   --relay value   relay URL for the fallback path (default: the built-in relay map)
   --no-relay      direct addresses only, no relay (default: false)
   --no-discovery  neither announce this endpoint nor resolve node ids by discovery (default: false)
   --force         take the cluster's side on conflicting paths (default: false)
   --jobs value    parallelism (0 = cores) (default: 0)
   --no-tui        plain log lines instead of the progress display (default: false) [$DSTORE_NO_TUI]
   --help, -h      show help
```

`dstore push --help`:

```text
NAME:
   dstore push - build the working directory's tree, upload it and write the reference

USAGE:
   dstore push [command options]

OPTIONS:
   --ticket value  cluster ticket (dstore1…) or comma-separated node ids; overrides the stored one for this run
   --relay value   relay URL for the fallback path (default: the built-in relay map)
   --no-relay      direct addresses only, no relay (default: false)
   --no-discovery  neither announce this endpoint nor resolve node ids by discovery (default: false)
   --user value    user identity recorded in the reference (default: the stored one, then the OS user)
   --force         replace the reference unconditionally (default: false)
   --jobs value    parallelism (0 = cores) (default: 0)
   --no-tui        plain log lines instead of the progress display (default: false) [$DSTORE_NO_TUI]
   --help, -h      show help
```

`dstore status --help`:

```text
NAME:
   dstore status - list the working directory's changes since the last sync, and whether the cluster moved

USAGE:
   dstore status [command options]

OPTIONS:
   --jobs value  parallelism (0 = cores) (default: 0)
   --help, -h    show help
```

`dstore diff --help`:

```text
NAME:
   dstore diff - unified diffs of the working directory against the last synced tree

USAGE:
   dstore diff [command options] [PATH...]

OPTIONS:
   --remote      against the tree last fetched from the cluster (default: false)
   --incoming    the last synced tree against the fetched one (what pull would apply) (default: false)
   --stat        one line per changed path with line counts (default: false)
   --jobs value  parallelism (0 = cores) (default: 0)
   --help, -h    show help
```

`dstore refs --help`:

```text
NAME:
   dstore refs - list references

USAGE:
   dstore refs [command options] [PREFIX]

OPTIONS:
   --ticket value  cluster ticket (dstore1…) or comma-separated node ids, found by discovery [$DSTORE_TICKET]
   --relay value   relay URL for the fallback path (default: the built-in relay map)
   --no-relay      direct addresses only, no relay (default: false)
   --no-discovery  neither announce this endpoint nor resolve node ids by discovery (mDNS and, with relays, number0's DNS) (default: false) [$DSTORE_NO_DISCOVERY]
   --help, -h      show help
```

`dstore watch --help`:

```text
NAME:
   dstore watch - watch references matching a glob and print each change until interrupted

USAGE:
   dstore watch [command options] PATTERN

DESCRIPTION:
   PATTERN is path-style: * and ? match within one /-separated segment, ** as a whole segment matches any number of segments, [...] is a character class.
   Every matching reference is printed first, then each change as it happens: NAME<TAB>KEY<TAB>CREATED<TAB>USER, or NAME<TAB>deleted.

OPTIONS:
   --ticket value  cluster ticket (dstore1…) or comma-separated node ids, found by discovery [$DSTORE_TICKET]
   --relay value   relay URL for the fallback path (default: the built-in relay map)
   --no-relay      direct addresses only, no relay (default: false)
   --no-discovery  neither announce this endpoint nor resolve node ids by discovery (mDNS and, with relays, number0's DNS) (default: false) [$DSTORE_NO_DISCOVERY]
   --help, -h      show help
```

`dstore ls --help` and `dstore cat --help` are identical to `refs` except for the NAME/USAGE lines:

```text
NAME:
   dstore ls - list a directory of a pushed tree

USAGE:
   dstore ls [command options] NAME [PATH]
```

```text
NAME:
   dstore cat - write a file of a pushed tree to stdout

USAGE:
   dstore cat [command options] NAME PATH
```

(Both followed by the same `OPTIONS:` block as `refs`.)

#### 3.1.1 Subcommand help *(verified)*

The standard client block (column width 18), used by every admin subcommand and `ref get`:

```text
OPTIONS:
   --ticket value  cluster ticket (dstore1…) or comma-separated node ids, found by discovery [$DSTORE_TICKET]
   --relay value   relay URL for the fallback path (default: the built-in relay map)
   --no-relay      direct addresses only, no relay (default: false)
   --no-discovery  neither announce this endpoint nor resolve node ids by discovery (mDNS and, with relays, number0's DNS) (default: false) [$DSTORE_NO_DISCOVERY]
   --help, -h      show help
```

Commands whose help is exactly `NAME:` / `USAGE:` + that block:

| Command | NAME line | USAGE line |
|---|---|---|
| `node drain` | `   dstore node drain` | `   dstore node drain [command options] ID` |
| `node weight` | `   dstore node weight` | `   dstore node weight [command options] ID GiB` |
| `node zone` | `   dstore node zone` | `   dstore node zone [command options] ID ZONE` |
| `node repair` | `   dstore node repair` | `   dstore node repair [command options] ID` |
| `voter add` | `   dstore voter add` | `   dstore voter add [command options] ID` |
| `transition status` | `   dstore transition status` | `   dstore transition status [command options]` |
| `transition abort` | `   dstore transition abort` | `   dstore transition abort [command options]` |
| `transition refreeze` | `   dstore transition refreeze` | `   dstore transition refreeze [command options]` |
| `transition pause` | `   dstore transition pause` | `   dstore transition pause [command options]` |
| `transition resume` | `   dstore transition resume` | `   dstore transition resume [command options]` |
| `gc status` | `   dstore gc status` | `   dstore gc status [command options]` |
| `gc hold` | `   dstore gc hold` | `   dstore gc hold [command options]` |
| `gc release` | `   dstore gc release` | `   dstore gc release [command options]` |
| `gc why` | `   dstore gc why` | `   dstore gc why [command options] KEY` |
| `catalog backup` | `   dstore catalog backup` | `   dstore catalog backup [command options]` |
| `catalog backups` | `   dstore catalog backups - list the backup object keys a node remembers` | `   dstore catalog backups [command options]` |
| `ref get` | `   dstore ref get` | `   dstore ref get [command options] NAME` |

e.g. `dstore node drain --help`:

```text
NAME:
   dstore node drain

USAGE:
   dstore node drain [command options] ID

OPTIONS:
   --ticket value  cluster ticket (dstore1…) or comma-separated node ids, found by discovery [$DSTORE_TICKET]
   --relay value   relay URL for the fallback path (default: the built-in relay map)
   --no-relay      direct addresses only, no relay (default: false)
   --no-discovery  neither announce this endpoint nor resolve node ids by discovery (mDNS and, with relays, number0's DNS) (default: false) [$DSTORE_NO_DISCOVERY]
   --help, -h      show help
```

Commands with extra flags (full text):

`dstore cluster status --help`:

```text
NAME:
   dstore cluster status - view, epoch, reachability, disk, transition, gc, voters

USAGE:
   dstore cluster status [command options]

OPTIONS:
   --ticket value  cluster ticket (dstore1…) or comma-separated node ids, found by discovery [$DSTORE_TICKET]
   --relay value   relay URL for the fallback path (default: the built-in relay map)
   --no-relay      direct addresses only, no relay (default: false)
   --no-discovery  neither announce this endpoint nor resolve node ids by discovery (mDNS and, with relays, number0's DNS) (default: false) [$DSTORE_NO_DISCOVERY]
   --store value   store directory [$DSTORE_STORE]
   --help, -h      show help
```

`dstore cluster ticket --help`:

```text
NAME:
   dstore cluster ticket - print the bootstrap ticket, or with --ids the member ids to hand out instead

USAGE:
   dstore cluster ticket [command options]

OPTIONS:
   --ticket value  cluster ticket (dstore1…) or comma-separated node ids, found by discovery [$DSTORE_TICKET]
   --relay value   relay URL for the fallback path (default: the built-in relay map)
   --no-relay      direct addresses only, no relay (default: false)
   --no-discovery  neither announce this endpoint nor resolve node ids by discovery (mDNS and, with relays, number0's DNS) (default: false) [$DSTORE_NO_DISCOVERY]
   --store value   store directory [$DSTORE_STORE]
   --ids           print comma-separated node ids (the short form, found by discovery) (default: false)
   --help, -h      show help
```

`dstore cluster replicas --help`:

```text
NAME:
   dstore cluster replicas - change R (a transition that copies 1/R of the store)

USAGE:
   dstore cluster replicas [command options] R

OPTIONS:
   --ticket value  cluster ticket (dstore1…) or comma-separated node ids, found by discovery [$DSTORE_TICKET]
   --relay value   relay URL for the fallback path (default: the built-in relay map)
   --no-relay      direct addresses only, no relay (default: false)
   --no-discovery  neither announce this endpoint nor resolve node ids by discovery (mDNS and, with relays, number0's DNS) (default: false) [$DSTORE_NO_DISCOVERY]
   --yes           do not ask (default: false)
   --help, -h      show help
```

`dstore token create --help`:

```text
NAME:
   dstore token create - create a single-use join token

USAGE:
   dstore token create [command options]

OPTIONS:
   --ticket value  cluster ticket (dstore1…) or comma-separated node ids, found by discovery [$DSTORE_TICKET]
   --relay value   relay URL for the fallback path (default: the built-in relay map)
   --no-relay      direct addresses only, no relay (default: false)
   --no-discovery  neither announce this endpoint nor resolve node ids by discovery (mDNS and, with relays, number0's DNS) (default: false) [$DSTORE_NO_DISCOVERY]
   --weight value  weight the token imposes on the joiner (GiB) (default: 0)
   --help, -h      show help
```

`dstore node remove --help`:

```text
NAME:
   dstore node remove

USAGE:
   dstore node remove [command options] ID

OPTIONS:
   --ticket value  cluster ticket (dstore1…) or comma-separated node ids, found by discovery [$DSTORE_TICKET]
   --relay value   relay URL for the fallback path (default: the built-in relay map)
   --no-relay      direct addresses only, no relay (default: false)
   --no-discovery  neither announce this endpoint nor resolve node ids by discovery (mDNS and, with relays, number0's DNS) (default: false) [$DSTORE_NO_DISCOVERY]
   --dead          the node is gone: remove its vote first (default: false)
   --allow-unsafe  (default: false)
   --help, -h      show help
```

`dstore voter remove --help`:

```text
NAME:
   dstore voter remove

USAGE:
   dstore voter remove [command options] ID

OPTIONS:
   --ticket value  cluster ticket (dstore1…) or comma-separated node ids, found by discovery [$DSTORE_TICKET]
   --relay value   relay URL for the fallback path (default: the built-in relay map)
   --no-relay      direct addresses only, no relay (default: false)
   --no-discovery  neither announce this endpoint nor resolve node ids by discovery (mDNS and, with relays, number0's DNS) (default: false) [$DSTORE_NO_DISCOVERY]
   --allow-unsafe  (default: false)
   --help, -h      show help
```

`dstore gc run --help` (column width 22):

```text
NAME:
   dstore gc run

USAGE:
   dstore gc run [command options]

OPTIONS:
   --ticket value      cluster ticket (dstore1…) or comma-separated node ids, found by discovery [$DSTORE_TICKET]
   --relay value       relay URL for the fallback path (default: the built-in relay map)
   --no-relay          direct addresses only, no relay (default: false)
   --no-discovery      neither announce this endpoint nor resolve node ids by discovery (mDNS and, with relays, number0's DNS) (default: false) [$DSTORE_NO_DISCOVERY]
   --tolerate-missing  (default: false)
   --garbage value     re-sweep only, at this dead ratio (default: 0)
   --help, -h          show help
```

`dstore catalog restore --help`:

```text
NAME:
   dstore catalog restore - force-write every reference of a backup object

USAGE:
   dstore catalog restore [command options] KEY|FILE

OPTIONS:
   --ticket value  cluster ticket (dstore1…) or comma-separated node ids, found by discovery [$DSTORE_TICKET]
   --relay value   relay URL for the fallback path (default: the built-in relay map)
   --no-relay      direct addresses only, no relay (default: false)
   --no-discovery  neither announce this endpoint nor resolve node ids by discovery (mDNS and, with relays, number0's DNS) (default: false) [$DSTORE_NO_DISCOVERY]
   --store value   store directory [$DSTORE_STORE]
   --help, -h      show help
```

`dstore store push --help` (column width 28):

```text
NAME:
   dstore store push - build a tree from PATH into the local store and push it under NAME

USAGE:
   dstore store push [command options] PATH NAME

OPTIONS:
   --ticket value            cluster ticket (dstore1…) or comma-separated node ids, found by discovery [$DSTORE_TICKET]
   --relay value             relay URL for the fallback path (default: the built-in relay map)
   --no-relay                direct addresses only, no relay (default: false)
   --no-discovery            neither announce this endpoint nor resolve node ids by discovery (mDNS and, with relays, number0's DNS) (default: false) [$DSTORE_NO_DISCOVERY]
   --local value             local store directory (layout: <dir>/packstore, <dir>/refs) [$AMBER_STORE]
   --user value              user identity recorded in the reference
   --force                   replace unconditionally (default: false)
   --expected-version value  CAS: the version ref get printed (hex); omit to require the name to be new
   --jobs value              (default: 0)
   --no-tui                  plain log lines instead of the progress display (default: false) [$DSTORE_NO_TUI]
   --help, -h                show help
```

`dstore store pull --help`:

```text
NAME:
   dstore store pull - pull the tree under NAME into the local store

USAGE:
   dstore store pull [command options] NAME

OPTIONS:
   --ticket value  cluster ticket (dstore1…) or comma-separated node ids, found by discovery [$DSTORE_TICKET]
   --relay value   relay URL for the fallback path (default: the built-in relay map)
   --no-relay      direct addresses only, no relay (default: false)
   --no-discovery  neither announce this endpoint nor resolve node ids by discovery (mDNS and, with relays, number0's DNS) (default: false) [$DSTORE_NO_DISCOVERY]
   --local value   local store directory (layout: <dir>/packstore, <dir>/refs) [$AMBER_STORE]
   --jobs value    (default: 0)
   --no-tui        plain log lines instead of the progress display (default: false) [$DSTORE_NO_TUI]
   --help, -h      show help
```

`dstore ref delete --help` (note the trailing spaces after `--expected-version value`):

```text
NAME:
   dstore ref delete

USAGE:
   dstore ref delete [command options] NAME

OPTIONS:
   --ticket value            cluster ticket (dstore1…) or comma-separated node ids, found by discovery [$DSTORE_TICKET]
   --relay value             relay URL for the fallback path (default: the built-in relay map)
   --no-relay                direct addresses only, no relay (default: false)
   --no-discovery            neither announce this endpoint nor resolve node ids by discovery (mDNS and, with relays, number0's DNS) (default: false) [$DSTORE_NO_DISCOVERY]
   --force                   (default: false)
   --expected-version value  
   --help, -h                show help
```

The node-side commands (column width 55). `dstore cluster init --help`:

```text
NAME:
   dstore cluster init - create a cluster on this store and print its ticket; then run serve

USAGE:
   dstore cluster init [command options]

OPTIONS:
   --store value                                      store directory [$DSTORE_STORE]
   --paxos-dir value                                  acceptor state directory (default <store>/paxos; put it on its own device)
   --rate value                                       reconcile copy rate in bytes/s (0 = unlimited) (default: 0)
   --jobs value                                       parallelism (0 = cores) (default: 0)
   --min-free value                                   free bytes below which uploads are refused (0 = 5% or 100 GiB) (default: 0)
   --pack-size value                                  size at which the active pack is sealed (bytes or Ki/Mi/Gi/Ti); applies to packs written from now on (default: "2Gi") [$DSTORE_PACK_SIZE]
   --gateway                                          also serve the transport-iroh ALPN (not implemented in this version) (default: false)
   --gc-interval value                                (default: 4h0m0s)
   --put-ttl value                                    (default: 1h0m0s)
   --relay value                                      relay URL for the fallback path (default: the built-in relay map)
   --no-relay                                         direct addresses only, no relay (default: false)
   --advertise-addr value [ --advertise-addr value ]  direct address to advertise, ip or ip:port (repeatable)
   --loopback                                         advertise 127.0.0.1 only (single-machine tests) (default: false)
   --bind value                                       UDP address to bind, ip:port
   --no-discovery                                     neither announce this endpoint nor resolve node ids by discovery (mDNS and, with relays, number0's DNS) (default: false) [$DSTORE_NO_DISCOVERY]
   --replicas value                                   R: owners per object (default: 3)
   --min-replicas value                               owners that must hold an object before a write succeeds (default max(R−1, 2)) (default: 0)
   --weight value                                     capacity in GiB, or auto (default: "auto")
   --zone value                                       failure domain (default: the node id)
   --allow-unsafe                                     allow min-replicas 1 (default: false)
   --help, -h                                         show help
```

`dstore node join --help` (trailing spaces after `--zone value`):

```text
NAME:
   dstore node join - join a cluster with this store and keep serving

USAGE:
   dstore node join [command options]

OPTIONS:
   --store value                                      store directory [$DSTORE_STORE]
   --paxos-dir value                                  acceptor state directory (default <store>/paxos; put it on its own device)
   --rate value                                       reconcile copy rate in bytes/s (0 = unlimited) (default: 0)
   --jobs value                                       parallelism (0 = cores) (default: 0)
   --min-free value                                   free bytes below which uploads are refused (0 = 5% or 100 GiB) (default: 0)
   --pack-size value                                  size at which the active pack is sealed (bytes or Ki/Mi/Gi/Ti); applies to packs written from now on (default: "2Gi") [$DSTORE_PACK_SIZE]
   --gateway                                          also serve the transport-iroh ALPN (not implemented in this version) (default: false)
   --gc-interval value                                (default: 4h0m0s)
   --put-ttl value                                    (default: 1h0m0s)
   --relay value                                      relay URL for the fallback path (default: the built-in relay map)
   --no-relay                                         direct addresses only, no relay (default: false)
   --advertise-addr value [ --advertise-addr value ]  direct address to advertise, ip or ip:port (repeatable)
   --loopback                                         advertise 127.0.0.1 only (single-machine tests) (default: false)
   --bind value                                       UDP address to bind, ip:port
   --no-discovery                                     neither announce this endpoint nor resolve node ids by discovery (mDNS and, with relays, number0's DNS) (default: false) [$DSTORE_NO_DISCOVERY]
   --seed value                                       cluster ticket (dstore1…) or comma-separated node ids, found by discovery
   --token value                                      join token (hex)
   --weight value                                     capacity in GiB, or auto (default: "auto")
   --zone value                                       
   --no-ramp                                          join at full weight in one step (default: false)
   --no-vote                                          hold no catalog (a relay-only or archive box) (default: false)
   --help, -h                                         show help
```

### 3.2 Usage-error output *(verified)*

`dstore refs --bogus` → stdout (leaf help **with** the `COMMANDS:` section), stderr
`dstore: flag provided but not defined: -bogus`, exit 1:

```text
Incorrect Usage: flag provided but not defined: -bogus

NAME:
   dstore refs - list references

USAGE:
   dstore refs [command options] [PREFIX]

COMMANDS:
   help, h  Shows a list of commands or help for one command

OPTIONS:
   --ticket value  cluster ticket (dstore1…) or comma-separated node ids, found by discovery [$DSTORE_TICKET]
   --relay value   relay URL for the fallback path (default: the built-in relay map)
   --no-relay      direct addresses only, no relay (default: false)
   --no-discovery  neither announce this endpoint nor resolve node ids by discovery (mDNS and, with relays, number0's DNS) (default: false) [$DSTORE_NO_DISCOVERY]
   --help, -h      show help
```

The same shape for every leaf; for nested leaves the NAME is the full path
(`dstore cluster status - …`). `dstore --bogus` prints `Incorrect Usage: …\n\n` + the app help.

| Arguments / env | First stdout line | stderr |
|---|---|---|
| `--bogus` | `Incorrect Usage: flag provided but not defined: -bogus` | `dstore: flag provided but not defined: -bogus` |
| `--log-level` | `Incorrect Usage: flag needs an argument: -log-level` | `dstore: flag needs an argument: -log-level` |
| `refs ---x` | `Incorrect Usage: bad flag syntax: ---x` | `dstore: bad flag syntax: ---x` |
| `refs -=x` | `Incorrect Usage: bad flag syntax: -=x` | `dstore: bad flag syntax: -=x` |
| `refs --ticket` | `Incorrect Usage: flag needs an argument: -ticket` | `dstore: flag needs an argument: -ticket` |
| `refs --log-level debug` | `Incorrect Usage: flag provided but not defined: -log-level` | same with `dstore: ` |
| `refs --no-relay=maybe` | `Incorrect Usage: invalid boolean value "maybe" for -no-relay: parse error` | same with `dstore: ` |
| `cluster replicas --yes=maybe 3` | `Incorrect Usage: invalid boolean value "maybe" for -yes: parse error` | same |
| `store pull --local L --jobs x n` | `Incorrect Usage: invalid value "x" for flag -jobs: parse error` | same |
| `gc run --garbage x` | `Incorrect Usage: invalid value "x" for flag -garbage: parse error` | same |
| `serve --gc-interval x` | `Incorrect Usage: invalid value "x" for flag -gc-interval: parse error` | same |
| `status --ticket x` | `Incorrect Usage: flag provided but not defined: -ticket` | same |
| env `DSTORE_NO_DISCOVERY=maybe`, `refs` | `Incorrect Usage: could not parse "maybe" as bool value from environment variable "DSTORE_NO_DISCOVERY" for flag no-discovery: strconv.ParseBool: parsing "maybe": invalid syntax` | same with `dstore: ` |
| env `DSTORE_NO_TUI=maybe`, `store pull --local L n` | `Incorrect Usage: could not parse "maybe" as bool value from environment variable "DSTORE_NO_TUI" for flag no-tui: strconv.ParseBool: parsing "maybe": invalid syntax` | same |

Required flags:

| Arguments | stdout | stderr |
|---|---|---|
| `store pull` | CommandHelpTemplate help of `store pull` (as in §3.1.1, no COMMANDS section) | `dstore: Required flag "local" not set` |
| `store pull n` | — | `dstore: Required flag "local" not set` |
| `store push help` | help of `dstore store help` (§2.2.3) | `dstore: Required flag "local" not set` |
| `node join` | `node join` help | `dstore: Required flags "seed, token" not set` |
| `node join --seed x` | `node join` help | `dstore: Required flag "token" not set` |
| `node join extra` | — | `dstore: Required flags "seed, token" not set` |

### 3.3 Error catalogue *(verified unless marked "code")*

All go to stderr as `dstore: <text>` with exit 1, stdout empty.

| Arguments (no env unless given) | stderr |
|---|---|
| `refs`, `ls`, `token create`, `transition status`, `ref get x`, `ref delete x --expected-version zz`, `gc why <64hex>`, `catalog restore <64hex>`, `node zone <64hex>`, `cluster ticket`, `replicas --yes 3`, `--log-level debug refs`, `refs --ticket=`, env `DSTORE_TICKET=` | `dstore: no cluster: set --ticket or $DSTORE_TICKET` |
| `refs --ticket bogus` (also `-ticket bogus`, `--ticket=bogus`, env `DSTORE_TICKET=bogus`, `clone --ticket bogus trees/x`, `node join --seed bogus --token x`) | `dstore: ticket: "bogus" is neither a dstore1 ticket nor a node id: invalid length` |
| `refs --ticket 'dstore1!!!'` | `dstore: ticket: illegal base32 data at input byte 0` |
| `refs --ticket dstore1` | `dstore: ticket: EOF` |
| `refs --ticket dstore1aaaa` | `dstore: ticket: cbor: 1 bytes of extraneous data starting at index 1` |
| `refs --ticket zz,yy` | `dstore: ticket: "zz" is neither a dstore1 ticket nor a node id: invalid length` |
| `cat x`, `cat a b --ticket x` | `dstore: cat NAME PATH` |
| `watch` | `dstore: watch PATTERN` |
| `clone` | `dstore: clone NAME [DIR]` |
| `clone a@b`, `store push --local L p bad@name` | `dstore: reference name must not contain '@'` |
| `store push --local L p ""` | `dstore: reference name must not be empty` |
| `init` | `dstore: init NAME` |
| `store pull --local L`, env `AMBER_STORE=` + `store pull` | `dstore: pull NAME` |
| `store push --local L p` | `dstore: push PATH NAME` |
| `status`, `fetch`, `pull`, `push`, `diff --remote=false --incoming` outside a working copy | `dstore: not a dstore working copy (no .dstore in this or any parent directory)` |
| `diff --remote --incoming` (anywhere) | `dstore: --remote and --incoming exclude each other` |
| `diff ../` in a working-copy root | `dstore: ../ is outside the working copy` |
| `diff --incoming` / `diff --remote` with no remote | `dstore: the reference does not exist on the cluster: nothing to pull` |
| `push` in a working copy with stored ticket `bogus` (also with `--user ""`) | `dstore: ticket: "bogus" is neither a dstore1 ticket nor a node id: invalid length` |
| `fetch --ticket zz` in a working copy | `dstore: ticket: "zz" is neither a dstore1 ticket nor a node id: invalid length` |
| `node remove zz` | `dstore: view: bad node id "zz"` |
| `node remove`, `voter add` | `dstore: view: bad node id ""` |
| `voter add aebagbafaydqqcikbmga2dqpcaireeyuculbogazdinryhi6d4qa` | `dstore: view: bad node id "aebagbafaydqqcikbmga2dqpcaireeyuculbogazdinryhi6d4qa"` |
| `node weight <64hex> x` | `dstore: weight ID GiB` |
| `gc why zz` | `dstore: why KEY (64 hex chars)` |
| `catalog restore` | `dstore: restore KEY\|FILE` |
| `catalog restore /etc/hosts` | `dstore: restore runs on a voter: give --store` |
| `cluster replicas x`, `cluster replicas`, `cluster replicas 300` | `dstore: replicas R` |
| `cluster replicas 3` with stdin `n\n` or EOF | stdout `changing R to 3 moves about 1/3 of every node's data; continue? [y/N] ` (no newline), stderr `dstore: aborted` |
| `cluster ticket --store /nonexistent-dstore`, `cluster status --store …`, env `DSTORE_STORE=…` + `cluster status` | `dstore: node: no identity in /nonexistent-dstore: open /nonexistent-dstore/identity: no such file or directory` |
| `serve`, `cluster init`, `node join --seed <64hex> --token <64hex>` | `dstore: no store directory: set --store or $DSTORE_STORE` |
| `serve --store X --pack-size 0` | `dstore: --pack-size: 0 is not a positive size` |
| `serve --store X --pack-size 1.5Gi` | `dstore: --pack-size: bad size "1.5Gi": unknown unit ".5Gi" (want Ki, Mi, Gi or Ti)` |
| env `DSTORE_PACK_SIZE=x` + `serve --store X` | `dstore: --pack-size: bad size "x": want a number with an optional Ki/Mi/Gi/Ti suffix` |
| `node join --seed <64hex> --token zz` | `dstore: token must be 32 bytes of hex` |
| code: `serve` on a store without a view | `dstore: this store is not a member of a cluster: run cluster init or node join` |
| code: local ticket of a store without a view | `dstore: this store is not a member of a cluster` |
| code: `store push` CAS failure | `dstore: cas mismatch: current key <hex> (pull first, or --force)` / `dstore: cas mismatch: reference is absent (pull first, or --force)` |
| code: `--expected-version zz` | `dstore: bad hex "zz"` |
| code: `push` user fallback empty | `dstore: user: user must not be empty` |
| code: `ls`/`cat`, missing object | `dstore: object <64 hex> not found` |
| code: `cat` on a symlink | `dstore: not a regular file with content` |
| code: `catalog restore <key>` not found | `dstore: backup object not found in the cluster` |
| code: `node join`, every join failed | `dstore: join: <last error>` |
| code: `--advertise-addr x` | `dstore: bad --advertise-addr "x"` |
| code: `--bind x` | `dstore: bad --bind "x"` |
| code: `--weight x` | `dstore: bad weight "x" (GiB or auto)` |

`reference.ValidateName` texts (core v0.0.8 `reference/reference.go:58-77`, checked in this order):
`reference name must not be empty`, `reference name exceeds 1024 bytes` (`MaxNameLen = 1024`; core-rs
`reference.rs:13` `MAX_NAME_LEN = 1024` with the same message), `reference name must be valid UTF-8`, per rune `reference name must not contain '@'` /
`reference name must not contain control characters` (`r < 0x20 || r == 0x7f`). `ValidateUser`:
`user must not be empty`, `user exceeds %d bytes`, `user must be valid UTF-8`,
`user must not contain control characters`.

Ticket parsing texts (from `ticket/ticket.go:60-96`, verified): `ticket: empty`;
`ticket: "<field>" is neither a dstore1 ticket nor a node id: <iroh error>` (e.g. `invalid length`,
`data is not a valid public key` for 64 hex characters that are not a curve point); base32 and CBOR
errors prefixed `ticket: `; `ticket: no members`.

### 3.4 Command output formats (Go format strings)

| Command | Stream | Format |
|---|---|---|
| `cluster init` | stdout | `fmt.Println("node id:", <64 hex>)`, `fmt.Println("cluster ticket:", t.Encode())`, `fmt.Println("now run: dstore serve --store", store)` → `node id: …\ncluster ticket: dstore1…\nnow run: dstore serve --store <dir>\n` |
| `node join` | stdout | `node id: <64 hex>\n` after joining |
| `cluster ticket` | stdout | `t.Encode()` or `t.IDs()` + `\n` |
| `token create` | stdout | `r.Text + "\n"` |
| admin ops | stdout | `Text\n`, each `Names[i]\n`, `key %x\n` |
| `catalog restore` | stdout | `%d references written\n` |
| `store push` | stderr | `built %s: %d new objects\n` |
| `store push` | stdout | `pushed %s: %d objects, %d uploaded, version %x\n` |
| `store pull` | stdout | `pulled %s: root %s, %d objects fetched (%d bytes)\n` |
| `refs`, `watch` | stdout | `%s\t%x\t%s\t%s\n` (name, key, RFC3339 local, user); watch deletion `%s\tdeleted\n` |
| `ref get` | stdout | `name %s\nkey %x\nversion %x\nuser %s\ncreated %s\n` |
| `ls` | stdout | entry name + `\n` |
| `cat` | stdout | raw file bytes |
| `clone` | stdout | `cloned %s into %s: root %s, %d objects fetched (%d bytes)\n` |
| `init` | stdout | `initialised working copy of %s; the reference exists (root %s): status shows everything as new, pull merges\n` / `initialised working copy of %s; the reference does not exist yet: push creates it\n` |
| `fetch` | stdout | `%s does not exist on the cluster\n` / `%s: up to date (%s)\n` / `fetched %s: root %s, %d objects fetched (%d bytes)\n` |
| `pull` | stderr | `conflicts:\n` + `  %s (local: %s, cluster: %s)\n` |
| `pull` | stdout | `already up to date\n` / `pulled: %d paths updated[, %d conflicts taken from the cluster]\n` |
| `push` | stdout | `nothing to push\n` / `%s already holds %s (an earlier push completed); state updated\n` / `pushed %s: root %s, %d objects, %d uploaded, version %x\n` |
| `status` | stdout | §2.8.11 |
| plain progress | stderr | `statusLine` + `\n` every 5 s |

`%x` of a nil or empty `[]byte` prints nothing (`version ` with a trailing space in `ref get` when
the version is empty). `root %s` / `synced to %s` use the first 16 hex characters of the key,
except `store pull`, which prints all 64.

`printStatus` (`client.go:134-193`):

```text
cluster %x incarnation %d epoch %d version %d                      ← ClusterID[:4] (8 hex)
replicas %d min_replicas %d nodes %d voters %d[ (no catalog fault tolerance)]   ← suffix when len(Voters) < 3
[transition %d (%s): frozen=%v acked=%d participants=%d done=%d]    ← if Pending != nil: ID, Reason, Frozen, len(ParticipantsAck), len(Participants), len(Done)
[voter change in progress (target %s)]                              ← if VoterSync == 1: node.ShortID(VoterSyncTarget)
per node, in v.Nodes order:
  %s weight %d zone %q voter=%v writable=%v                         ← two leading spaces; IDString (64 hex), Weight, Zone (%q), v.IsVoter(id), nd.Writable
      epoch %d packs %d records %d bytes %d pins %d pending-packs %d free %d GiB[ [lease holder]][ [AMNESIAC]][ [retired]]   ← six spaces; FreeBytes>>30
[      cannot reach: %s]                                            ← ShortIDs joined by " "
[      gc: %s]                                                      ← if GC != ""
[      transition: %s]                                              ← if Transition != "" && != "idle"
[      voter %s: %d calls, %d failures, p99 %d ms]                  ← per Voters entry: ShortID, Calls, Failures, P99ms
```

The node line and the follow-ups are separate `Println`/`Printf` calls. Unreachable node:
`  <id> weight … writable=true — unreachable: <err>\n`; undecodable status:
`  <id> … writable=true — bad status\n`; no status sub-lines in either case.
`%q` = Go `strconv.Quote` (`zone ""`, `zone "rack-1"`, `zone "a\"b"`, `zone "é"`, `zone "\x01"`,
`zone "tab\t"`, `zone "…"`, `zone " "`, `zone "\xff"` — verified). Bools print `true`/`false`.

### 3.5 Working-copy output examples *(verified)*

Fixture: files `a.txt`, `sub/b.txt`, `run.sh` (0644), symlink `link → a.txt`; base = their tree;
then `a.txt` edited, `run.sh` chmod 0755, `sub/b.txt` removed, `new.txt` added, `link` replaced by a
directory containing `inner.txt`; no remote.

`dstore status` (same output from `sub/`):

```text
reference trees/demo, synced to 2101cfca00328ad5
remote: the reference does not exist on the cluster
changes:
  modified  a.txt
  type      link/ (symlink → directory)
  new       link/inner.txt
  new       new.txt
  mode      run.sh (0644 → 0755)
  deleted   sub/b.txt
1 paths differ only in mtime, ownership or xattrs
```

(The 16-hex prefix depends on the fixture's mtimes; compare the structure, or build the fixture
with fixed mtimes.)

`dstore diff`:

```text
diff a/a.txt b/a.txt
--- a/a.txt
+++ b/a.txt
@@ -1,3 +1,4 @@
 one
-two
+2
 three
+four
diff a/link b/link
--- a/link
+++ /dev/null
@@ -1 +0,0 @@
-a.txt
\ No newline at end of file
diff a/link/inner.txt b/link/inner.txt
--- /dev/null
+++ b/link/inner.txt
@@ -0,0 +1 @@
+inner
diff a/new.txt b/new.txt
--- /dev/null
+++ b/new.txt
@@ -0,0 +1 @@
+new file
diff a/run.sh b/run.sh
old mode 0644
new mode 0755
diff a/sub/b.txt b/sub/b.txt
--- a/sub/b.txt
+++ /dev/null
@@ -1,2 +0,0 @@
-x
-y
```

`dstore diff --stat`:

```text
 a.txt | +2 -1
 link | +0 -1
 link/inner.txt | +1 -0
 new.txt | +1 -0
 sub/b.txt | +0 -2
 5 files changed, 4 insertions(+), 4 deletions(-)
```

`dstore diff a.txt` and `dstore diff a.txt --stat` (flag after the path is a path) print only the
`a.txt` hunk.

Second fixture: base = {f1.txt `alpha`, keep.txt `same`}, remote = {f1.txt `beta`, f2.txt
`added remotely`}, working directory = base content rewritten (new mtimes).

`dstore status`:

```text
reference trees/demo2, synced to 20d432bf25934fe5
remote: moved since your last fetch (+1 ~1 -1; run pull)
2 paths differ only in mtime, ownership or xattrs
```

`dstore diff --incoming --stat`:

```text
 f1.txt | +1 -1
 f2.txt | +1 -0
 keep.txt | +0 -1
 3 files changed, 2 insertions(+), 2 deletions(-)
```

`dstore diff --incoming` shows `f1.txt` `-alpha +beta`, `f2.txt` added, `keep.txt` deleted;
`dstore diff --remote` shows the reverse direction (remote tree as old, disk as new).

### 3.6 Progress output *(verified)*

Plain status lines (stderr, one per 5 s):

```text
3/4 objects  0 B/s
0/0 objects  512 B  100 B/s
5/10 objects  1.0 KiB / 2.0 KiB  512 B/s  eta 2s
5/10 objects  1.0 GiB / 5.0 GiB  3.5 MiB/s  eta 19m21s
5/10 objects  2.9 KiB / 1000 B  10 B/s
1/2 objects  10 B / 100 B  0 B/s  eta 3m0s
1/2 objects  10 B / 95.4 MiB  1 B/s  eta 27777h46m30s
```

(inputs in §5.4; the sixth line has rate 0.5 → `HumanBytes(0)` but an ETA because `rate > 0`).

TUI frame after two ticks (1.5 s apart), two events, default bar width 60 — raw
`View().Content` (before Bubble Tea's colour downsampling):

```text
\x1b[1mpush trees/demo\x1b[m  \x1b[2melapsed 2s\x1b[m\n
\x1b[38;2;90;86;224;48;2;92;86;225m▌\x1b[m\x1b[38;2;94;86;225;48;2;95;87;225m▌\x1b[m\x1b[38;2;97;87;225;48;2;99;87;225m▌\x1b[m\x1b[38;2;101;87;226;48;2;102;87;226m▌\x1b[m\x1b[38;2;104;88;226;48;2;106;88;226m▌\x1b[m\x1b[38;2;108;88;227;48;2;109;88;227m▌\x1b[m\x1b[38;2;111;89;227;48;2;112;89;227m▌\x1b[m\x1b[38;2;114;89;227;48;2;116;89;228m▌\x1b[m\x1b[38;2;117;90;228;48;2;119;90;228m▌\x1b[m\x1b[38;2;120;90;228;48;2;122;90;229m▌\x1b[m\x1b[38;2;123;90;229;48;2;125;91;229m▌\x1b[m\x1b[38;2;126;91;229;48;2;128;91;229m▌\x1b[m\x1b[38;2;129;91;230;48;2;131;92;230m▌\x1b[m\x1b[38;2;132;92;230;48;2;134;92;230m▌\x1b[m\x1b[38;2;135;92;231;48;2;137;93;231m▌\x1b[m\x1b[38;2;138;93;231;48;2;140;93;231m▌\x1b[m\x1b[38;2;141;93;231;48;2;142;93;232m▌\x1b[m\x1b[38;2;144;94;232;48;2;145;94;232m▌\x1b[m\x1b[38;2;147;94;232;48;2;148;94;233m▌\x1b[m\x1b[38;2;149;95;233;48;2;151;95;233m▌\x1b[m\x1b[38;2;152;95;233;48;2;153;95;233m▌\x1b[m\x1b[38;2;155;96;234;48;2;156;96;234m▌\x1b[m\x1b[38;2;157;96;234;48;2;159;96;234m▌\x1b[m\x1b[38;2;160;96;235;48;2;161;97;235m▌\x1b[m\x1b[38;2;163;97;235;48;2;164;97;235m▌\x1b[m\x1b[38;2;165;97;235;48;2;167;98;236m▌\x1b[m\x1b[38;2;168;98;236;48;2;169;98;236m▌\x1b[m\x1b[38;2;171;98;236;48;2;172;99;237m▌\x1b[m\x1b[38;2;96;96;96m░░░░░░░░░░░░░░░░░░░░░░░░░░░\x1b[m  50%\n
5/10 objects  1.0 KiB / 2.0 KiB  341 B/s  eta 3s\n
\x1b[2mnode       path        rtt batches state                  sent         rate\x1b[m\n
abcd0000   direct      3ms       2 waiting for ack     1.0 KiB      341 B/s\n
ef010000   relay      45ms       1 sending                 0 B        0 B/s\n
\x1b[2mevents\x1b[m\n
\x1b[33m12:34:56  upload retry node=abcd reason=busy\x1b[m\n
12:34:56  connected node=abcd nodes=3 path=direct rtt=3ms\n
```

(line breaks added after each `\n` for readability). Stripped of escapes:

```text
push trees/demo  elapsed 2s
▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌░░░░░░░░░░░░░░░░░░░░░░░░░░░  50%
5/10 objects  1.0 KiB / 2.0 KiB  341 B/s  eta 3s
node       path        rtt batches state                  sent         rate
abcd0000   direct      3ms       2 waiting for ack     1.0 KiB      341 B/s
ef010000   relay      45ms       1 sending                 0 B        0 B/s
events
12:34:56  upload retry node=abcd reason=busy
12:34:56  connected node=abcd nodes=3 path=direct rtt=3ms
```

After `WindowSizeMsg{Width: 100}` (bar width 80) and `doneMsg{err: context canceled}`:

```text
push trees/demo  elapsed 0s
▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌▌░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░  50%
5/10 objects  1.0 KiB / 2.0 KiB  2.0 MiB/s  eta 0s
node       path        rtt batches state                  sent         rate
abcd0000   direct      3ms       2 waiting for ack     1.0 KiB    2.0 MiB/s
ef010000   relay      45ms       1 sending                 0 B        0 B/s
events
12:34:56  upload retry node=abcd reason=busy
12:34:56  connected node=abcd nodes=3 path=direct rtt=3ms
failed: context canceled
```

(`elapsed 0s` and the rate come from `observe(time.Now())` in the done handler, with `start` in the
past of the synthetic ticks; the done line is `\x1b[31mfailed: context canceled\x1b[m`.)

Empty report, width 10 (bar clamps to 20), done without error — raw:
`"\x1b[1mpull x\x1b[m  \x1b[2melapsed 0s\x1b[m\n\x1b[38;2;96;96;96m░░░░░░░░░░░░░░░\x1b[m   0%\n0/0 objects  0 B/s\n\x1b[32mdone\x1b[m\n"`.

The 100 % case (empty run of length 0) was not captured: add it to the generator (§5.3).

### 3.7 Files the CLI writes

| File | Writer | Format |
|---|---|---|
| `<store>/identity` | node-side `loadOrCreateKey` | lower-case hex of the 32-byte Ed25519 seed + `\n`, mode 0600; read with `TrimSpace` |
| `<store>/port` | node-side `bindNodeEndpoint` | decimal UDP port + `\n`, mode 0644 |
| `<local>/refs` | `store push`/`store pull` | core refstore (Pebble in Go, redb in core-rs — not cross-openable), values = canonical reference records |
| `<local>/packstore` | `store push`/`store pull` | core packstore (interoperable) |
| `.dstore/config`, `.dstore/state`, `.dstore/packstore` | working-copy commands (worktree area; `RefreshTicket` rewrites `config`) | JSON (`MarshalIndent`, 2 spaces, trailing `\n`, via `<path>.tmp` + rename) |

### 3.8 CBOR vectors *(verified, codec = CanonicalEncOptions)*

`id2` = 32×`ab`, `key32` = 32×`5a`, `id1` = 32×`01`.

| AdminRequest | Hex |
|---|---|
| `{Op: "cluster-ticket"}` | `a1006e636c75737465722d7469636b6574` |
| `{Op: "token-create"}` | `a1006c746f6b656e2d637265617465` |
| `{Op: "token-create", Weight: 100}` | `a2006c746f6b656e2d637265617465021864` |
| `{Op: "replicas", Replicas: 2}` | `a200687265706c696361730402` |
| `{Op: "replicas"}` | `a100687265706c69636173` |
| `{Op: "node-remove", Node: id2, Dead: true, AllowUnsafe: true}` | `a4006b6e6f64652d72656d6f7665015820abababababababababababababababababababababababababababababababab05f506f5` |
| `{Op: "node-remove", Node: id2}` | `a2006b6e6f64652d72656d6f7665015820abababababababababababababababababababababababababababababababab` |
| `{Op: "node-drain", Node: id2}` | `a2006a6e6f64652d647261696e015820abababababababababababababababababababababababababababababababab` |
| `{Op: "node-weight", Node: id2, Weight: 50}` | `a3006b6e6f64652d776569676874015820abababababababababababababababababababababababababababababababab021832` |
| `{Op: "node-weight", Node: id2, Weight: 4294967295}` | `a3006b6e6f64652d776569676874015820abababababababababababababababababababababababababababababababab021affffffff` |
| `{Op: "node-zone", Node: id2, Zone: "rack-1"}` | `a300696e6f64652d7a6f6e65015820abababababababababababababababababababababababababababababababab03667261636b2d31` |
| `{Op: "node-zone", Node: id2}` | `a200696e6f64652d7a6f6e65015820abababababababababababababababababababababababababababababababab` |
| `{Op: "node-repair", Node: id2}` | `a2006b6e6f64652d726570616972015820abababababababababababababababababababababababababababababababab` |
| `{Op: "voter-add", Node: id2}` | `a20069766f7465722d616464015820abababababababababababababababababababababababababababababababab` |
| `{Op: "voter-remove", Node: id2, AllowUnsafe: true}` | `a3006c766f7465722d72656d6f7665015820abababababababababababababababababababababababababababababababab06f5` |
| `{Op: "transition-status"}` | `a100717472616e736974696f6e2d737461747573` |
| `{Op: "transition-abort"}` | `a100707472616e736974696f6e2d61626f7274` |
| `{Op: "transition-refreeze"}` | `a100737472616e736974696f6e2d7265667265657a65` |
| `{Op: "transition-pause"}` | `a100707472616e736974696f6e2d7061757365` |
| `{Op: "transition-resume"}` | `a100717472616e736974696f6e2d726573756d65` |
| `{Op: "gc-run"}` | `a1006667632d72756e` |
| `{Op: "gc-run", Tolerate: true}` | `a2006667632d72756e0af5` |
| `{Op: "gc-run", Garbage: 0.25}` | `a2006667632d72756e09f93400` |
| `{Op: "gc-run", Garbage: 0.1}` | `a2006667632d72756e09fb3fb999999999999a` |
| `{Op: "gc-run", Garbage: 1.5}` | `a2006667632d72756e09f93e00` |
| `{Op: "gc-run", Garbage: -1}` | `a2006667632d72756e09f9bc00` |
| `{Op: "gc-run", Garbage: 100000}` | `a2006667632d72756e09fa47c35000` |
| `{Op: "gc-status"}` | `a1006967632d737461747573` |
| `{Op: "gc-hold", Pause: true}` | `a2006767632d686f6c640cf5` |
| `{Op: "gc-hold"}` | `a1006767632d686f6c64` |
| `{Op: "gc-why", Key: key32}` | `a2006667632d7768790858205a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a` |
| `{Op: "catalog-backup"}` | `a1006e636174616c6f672d6261636b7570` |
| `{Op: "catalog-backups"}` | `a1006f636174616c6f672d6261636b757073` |

Note map key order: integer keys sort by encoded bytes, so `10` (`0a`), `12` (`0c`) come after `9`
(`09`) and before `24`+ keys; floats use the shortest IEEE form that round-trips (half, single, double).

| AdminReply | Hex | `adminAction` prints |
|---|---|---|
| `{Text: "ok"}` | `a100626f6b` | `ok\n` |
| `{Names: ["trees/a", "trees/b"]}` | `a103826774726565732f616774726565732f62` | `trees/a\ntrees/b\n` |
| `{Key: key32, Text: "backup written"}` | `a2006e6261636b7570207772697474656e0458205a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a` | `backup written\nkey 5a5a…5a\n` |
| `{Token: id1, Text: hex(id1)}` | `a2007840` + hex(64 ASCII `01…`) + `015820` + id1 | `0101…01\n` |
| `{}` | `a0` | nothing |
| `{Key: 010203, Names: ["x"], Text: "t"}` | `a3006174038161780443010203` | `t\nx\n` (short key not printed) |
| `a2 00 62 6f6b 18 63 01` (unknown key 99) | decodes, Text = `ok` | `ok\n` |

`node.Status{ID: id2, Epoch: 5, Incarnation: 1, Packs: 2, Records: 10, Bytes: 1234, Pins: 1, Unreachable: [id1], PendingPacks: 0, Transition: "idle", GC: "epoch 3 idle", Voters: [{ID: id1, Calls: 10, Failures: 1, P99ms: 12}], Writable: true, FreeBytes: 5<<30 + 123, TotalBytes: 10<<30, IsHolder: true}`:

```text
b5005820abababababababababababababababababababababababababababababababab010502010302040a051904d206010781582001010101010101010101010101010101010101010101010101010101010101010800096469646c650a6c65706f636820332069646c650c81a40058200101010101010101010101010101010101010101010101010101010101010101010a0201030c0df50e1b000000014000007b0f1b000000028000000010001100120013001400181af5
```

Zero `node.Status`: `b000f601000200030004000500060008000df40e000f0010001100120013001400`
(nil `ID` → `f6`). `DecodeStatus([ff])` → error `cbor: unexpected "break" code` (the CLI prints
`— bad status`, not the message).

---

## 4. Rust design

### 4.1 Layout

```text
dstore-client-rs/
  src/lib.rs                      library crate (client, worktree, ticket, … — other areas)
  src/log.rs                      slog-compatible logger facade + TextHandler (shared with the client)
  src/goutil/                     Go-compatible formatting and parsing helpers (§4.6)
  src/bin/dstore/main.rs          entry point, exit-code mapping
  src/bin/dstore/gocli/mod.rs     urfave/cli-compatible framework: AppDef, CommandDef, FlagDef, Context, run()
  src/bin/dstore/gocli/goflag.rs  port of Go flag.parseOne + value parsers
  src/bin/dstore/gocli/help.rs    templates, command rows, flag stringification, tabwriter
  src/bin/dstore/app.rs           the command table (definitions only; mirrors main.go/client.go/wc.go order)
  src/bin/dstore/common.rs        dial_cluster, dial_ticket, relay_mode_of, admin, admin_action, print_status, record_payload, open_local, cluster_get, hex_decode
  src/bin/dstore/cmd_node.rs      cluster/serve/token/node/voter/transition/gc/catalog actions
  src/bin/dstore/cmd_client.rs    store push/pull, refs, watch, ref get/delete, ls, cat
  src/bin/dstore/cmd_wc.rs        clone, init, fetch, pull, push, status, diff + resolve_ticket, wc_config, push_user, describe_change, filter_paths
  src/bin/dstore/size.rs          parse_size, pack_size
  src/bin/dstore/progress/        latest.rs, rate_meter.rs, status_line.rs, plain.rs, ui_model.rs (pure model), render.rs (inline renderer), tea_handler.rs, colors.rs (Lab blend, profile downsampling)
  src/bin/dstore/admin_types.rs   AdminRequest, AdminReply, Status, VoterStat (CBOR, §2.7)
```

The admin types may instead live in the library (the node agent types are part of the wire contract);
the CLI only needs encode/decode.

### 4.2 CLI framework: do not use clap

clap 4.6.6 (available offline, `clap_builder-4.6.6/src/builder/command.rs`) cannot express the Go
behaviour without fighting it:

1. Go accepts long names with one dash (`-ticket x`); clap parses `-t…` as clustered short options.
2. Go stops parsing at the first positional argument; clap interleaves options and positionals
   (`trailing_var_arg`/`allow_hyphen_values` only affect the last positional).
3. Every leaf has a `help`/`h` subcommand that captures a first positional `help`/`h`.
4. Help text must match byte for byte: urfave templates, `(default: …)` rules, env hints,
   tabwriter padding with trailing spaces, the `COMMANDS: help, h` section in usage errors, the
   truncated `help <parent>` output. clap's `help_template` cannot reproduce these.
5. Errors: `Incorrect Usage: …` on stdout followed by help, `dstore: flag provided but not defined: -x`
   on stderr, exit 1; `No help topic for '…'` exit 3; clap exits 2 with its own texts.
6. Env semantics: an empty env var counts as set (`IsSet`), bool env parse errors use urfave's text.

core-rs's example CLI uses clap and documents deviations (`port-notes/cli-gc.md`: exit 2 for parse
failures); dstore requires identical behaviour, so write a small framework (estimated 900-1200
lines) and lock it with the golden outputs of §5.

```rust
pub enum FlagKind {
    String { default: &'static str },
    Bool,
    Int { default: i64 },      // Go int
    Int64 { default: i64 },
    Uint { default: u64 },
    Float64 { default: f64 },
    Duration { default_ns: i64 },
    StringSlice,
}
pub struct FlagDef {
    pub name: &'static str,
    pub aliases: &'static [&'static str],   // only help/h and version/v
    pub kind: FlagKind,
    pub usage: &'static str,
    pub env: &'static [&'static str],
    pub required: bool,
    pub disable_default_text: bool,         // help and version flags
}
pub type Action = for<'a> fn(&'a Context) -> Pin<Box<dyn Future<Output = Result<(), CliError>> + 'a>>;
pub struct CommandDef {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub usage: &'static str,
    pub args_usage: &'static str,
    pub description: &'static str,
    pub flags: Vec<FlagDef>,
    pub subcommands: Vec<CommandDef>,
    pub action: Option<Action>,             // None → help action
}
pub struct AppDef { pub name: &'static str, pub usage: &'static str, pub version: &'static str, pub flags: Vec<FlagDef>, pub commands: Vec<CommandDef> }

pub enum FlagValue { Str(String), Bool(bool), I64(i64), U64(u64), F64(f64), DurationNs(i64), Slice(Vec<String>) }
struct LevelState { cmd: *const CommandDef /* or index path */, values: HashMap<&'static str, (FlagValue, bool /*on cli*/, bool /*from env*/)>, args: Vec<OsString> }
pub struct Context { levels: Vec<LevelState> /* root .. current */ }
impl Context {
    pub fn string(&self, name: &str) -> String;       // lineage search, "" if undefined
    pub fn bool(&self, name: &str) -> bool;
    pub fn int(&self, name: &str) -> i64;
    pub fn int64(&self, name: &str) -> i64;
    pub fn uint(&self, name: &str) -> u64;
    pub fn float64(&self, name: &str) -> f64;
    pub fn duration_ns(&self, name: &str) -> i64;
    pub fn string_slice(&self, name: &str) -> Vec<String>;
    pub fn is_set(&self, name: &str) -> bool;         // on cli || from env (per §2.2.5)
    pub fn args(&self) -> &[OsString];
    pub fn narg(&self) -> usize;
    pub fn arg(&self, n: usize) -> &OsStr;            // "" when missing (Go Args().Get)
}
pub enum CliError {
    Msg(String),                    // main prints "dstore: {msg}\n" to stderr, exit 1
    Exit { msg: String, code: i32 }, // printed "{msg}\n" to stderr, exit code (help topic: 3)
}
pub async fn run(app: &AppDef, args: Vec<OsString>, out: &mut dyn Write) -> Result<(), CliError>;
```

- Arguments are `OsString` (Go works on bytes; `PATH` arguments of `store push` and `diff` may be
  non-UTF-8). Flag names and values that must be strings are converted lossily only where Go would
  treat them as text.
- `goflag::parse(defs, &mut values, args) -> Result<Vec<OsString> /*positional*/, String /*Go error text*/>`
  ports `parseOne` exactly (§2.2.4), including `bad flag syntax`, one-dash messages and the value
  parsers.
- Environment application happens in definition order before parsing and can fail (§2.2.5).
- Required-flag checks, help flag, version flag and dispatch follow §2.2.2 in that order.
- Help action cases §2.2.3, including: the `help` subcommand exists on every command after setup;
  `help <parent>` renders NAME + USAGE + `\n` only; the help command's HelpName is
  `<first parent set up on this path> help` (for `dstore <top> … help` that is `dstore <top> help`),
  and `" help"` when invoked from the root.
- `help::render_*` ports the three templates and a tabwriter with (minwidth 1, tabwidth 8,
  padding 2, pad ' ', no flags): cells split on `\t`, a block of consecutive lines with at least
  one tab shares column widths = max rune count + 2; a line without a tab ends the block; the text
  after the last tab is not padded; cells before a tab are padded even when the trailing text is
  empty.

### 4.3 Logger (`src/log.rs`)

A small `slog`-shaped facade, shared with the client library so its log calls keep Go's attribute
kinds (tracing's formatting differs and would need a custom layer anyway):

```rust
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)] pub struct Level(pub i32); // DEBUG -4, INFO 0, WARN 4, ERROR 8
pub enum Value { Str(String), I64(i64), U64(u64), F64(f64), Bool(bool), DurationNs(i64), Err(String), Bytes(Vec<u8>), Any(String) }
pub trait Handler: Send + Sync { fn enabled(&self, level: Level) -> bool; fn handle(&self, t: SystemTime, level: Level, msg: &str, attrs: &[(String, Value)]); }
#[derive(Clone)] pub struct Logger { handler: Arc<dyn Handler>, attrs: Arc<Vec<(String, Value)>> }
impl Logger {
    pub fn new(h: Arc<dyn Handler>) -> Self;
    pub fn with(&self, attrs: impl IntoIterator<Item = (String, Value)>) -> Self;
    pub fn log(&self, level: Level, msg: &str, attrs: &[(String, Value)]);
    pub fn debug(&self, msg: &str, attrs: &[(String, Value)]); // info, warn, error likewise
}
pub struct TextHandler { level: Level, out: Mutex<Box<dyn Write + Send>> }   // §2.3, one write_all per line
pub fn log_level(s: &str) -> Level;  // §2.3 mapping
```

`TextHandler::handle` formats `time=` with the local offset (libc `localtime_r` + `tm_gmtoff`, as
core-rs `examples/amber-store.rs:579 local_tm`), milliseconds truncated; `level=` Go names with
`+n`/`-n`; values per §2.3 using `goutil::needs_quoting` + `goutil::quote`.

### 4.4 Progress (`progress/`)

- `Latest { inner: Mutex<ProgressReport> }` with `set`/`get`; the `Progress` callback given to the
  client is `Arc<dyn Fn(ProgressReport) + Send + Sync>`.
- `RateMeter { window: Duration, samples: Vec<(Instant, i64)> }`, `add(&mut self, t: Instant, n: i64) -> f64`
  exactly §2.6 (Go uses the monotonic clock reading of `time.Time`; `Instant` is equivalent).
- `status_line(r: &ProgressReport, rate: f64) -> String`, `fraction(r) -> f64`.
- `run_transfer<T, F, Fut>(c: &Context, cancel: CancellationToken, title: String, f: F) -> Result<T, CliError>`
  where `F: FnOnce(CancellationToken, Logger, Progress) -> Fut`, `Fut: Future<Output = Result<T, CliError>>`;
  chooses plain or TUI by `c.bool("no-tui") || !is_char_device(2)`.
- Plain: `tokio::time::interval_at(now + 5s, 5s)` task writing `status_line` lines to stderr;
  stop it (and await it) after `f` returns.
- TUI: a pure `UiModel` (fields as Go; `update(&mut self, msg: UiMsg) -> UiCmd` with
  `UiMsg::{Tick(Instant), Resize(u16), CtrlC, Event{at: SystemTime, level, text}, Done(Option<String>)}`,
  `view(&self) -> String` returning the frame with SGR sequences as in §3.6) so the Go
  `TestUIModel` ports directly, plus an inline renderer over crossterm 0.29: hide cursor, for each
  frame move up to the start of the previous frame (`\r`, `MoveUp(n)`), `Clear(FromCursorDown)`,
  write the frame; tick every 100 ms; on `Done` render the final frame, show the cursor and stop,
  leaving the frame on screen. Raw mode + key events only when stdin is a character device;
  `KeyCode::Char('c')` with `KeyModifiers::CONTROL` → `UiMsg::CtrlC`. Width: `TIOCGWINSZ` on fd 2
  (0 when not a tty → bar 20) and SIGWINCH updates.
- `TeaHandler { level: Level, send: tokio::sync::mpsc::UnboundedSender<UiMsg> }` implementing
  `log::Handler` with `attr_value` of §2.6.
- `colors.rs`: `blend1d(steps, a: [u8;3], b: [u8;3]) -> Vec<[u8;3]>` (go-colorful Lab port, §2.6),
  and downsampling by a colorprofile subset (`NO_COLOR`, `TERM` unset/`dumb`, `COLORTERM`
  truecolor/24bit, `*256color`, known truecolor terminals) — see §8.

### 4.5 Signals and runtime

`main` builds a multi-thread tokio runtime, runs `gocli::run`, maps `CliError` to stderr + exit code
(`std::process::exit`, after flushing stdout). Actions that call `signalCtx` in Go call
`signal_token() -> CancellationToken` which spawns a task awaiting
`tokio::signal::unix::signal(SignalKind::interrupt())` / `terminate()` and cancels the token; the
signal streams stay alive until the process exits.

### 4.6 Go-compatible helpers (`src/goutil/`)

| Rust function | Go behaviour |
|---|---|
| `quote(s: &[u8]) -> String` | `strconv.Quote` (`\a \b \f \n \r \t \v \\ \"`, `\xNN` for invalid UTF-8, `\uNNNN`/`\UNNNNNNNN` for non-printable runes; port the `isPrint` tables from Go `strconv/isprint.go` for exactness) |
| `needs_quoting(s: &str) -> bool` | slog `needsQuoting` (§2.3) |
| `format_duration(ns: i64) -> String` | `Duration.String` (copy `format_go_duration` from core-rs `examples/amber-store.rs:1241`) |
| `round_duration(ns: i64, m: i64) -> i64` | `Duration.Round` half away from zero, saturating (generalise core-rs `round_ms`) |
| `parse_duration(s: &str) -> Result<i64, String>` | `time.ParseDuration` (copy `parse_go_duration`, core-rs `examples/amber-store.rs:1053`) |
| `rfc3339_local(ns: i64) -> String` | `time.Unix(0, ns).Format(time.RFC3339)` (core-rs `rfc3339_local` approach) |
| `rfc3339_millis_local(t: SystemTime) -> String` | slog time |
| `clock_hms_local(t: SystemTime) -> String` | `Format("15:04:05")` |
| `parse_bool(s: &str) -> Option<bool>` | `strconv.ParseBool` |
| `parse_int(s: &str, base0: bool, bits: u32) -> Result<i64, NumErr>`, `parse_uint(…) -> Result<u64, NumErr>` | `strconv.ParseInt`/`ParseUint` incl. base 0 prefixes and `_` rules; `NumErr::{Syntax, Range}` |
| `parse_float(s: &str) -> Result<f64, NumErr>` | `strconv.ParseFloat(s, 64)` (Rust `f64::from_str` differs on hex floats and `_`) |
| `path_base(s: &str) -> String` | `path.Base` (slash-only; `""` → `.`, trailing slashes stripped, all slashes → `/`) |
| `abs_clean(p: &OsStr) -> PathBuf`, `rel(base, target) -> Option<String>` | `filepath.Abs` (lexical `Clean`, no symlink resolution) and `filepath.Rel` |
| `is_char_device(fd: RawFd) -> bool` | `isTerminal` (fstat `S_ISCHR`) |
| `current_username() -> String` | pure-Go `user.Current` fallback chain (§2.5) |

`HumanBytes`/`Rate` belong to the client area (`client::human_bytes`, `client::rate`); the CLI uses
those.

### 4.7 Mapping onto core-rs (amber-store-core 0.3.0, rev a85ffa1)

| Go call | Rust (checked file) |
|---|---|
| `packstore.Open(dir, WithSync(true))` | `packstore::Store::open(dir, Options)` with sync (`src/packstore/mod.rs:359`; README shows `Store::open(dir, packstore::Options::new())`) |
| `refstore.Open(dir, true)` | `refstore::Store::open(dir, true)` (`src/refstore.rs:114`; redb, not Pebble — §7) |
| `ingest.Dir(st, path, Opts{Jobs})` → `(root, WriteStats)` | `ingest::dir(&store, path, ingest::Opts{..})` (`src/ingest/mod.rs:379`), `WriteStats.stored` (`src/packstore/parallel.rs:21-23`) |
| `reference.ValidateName`, `ValidateUser`, `Reference.Encode` | `reference::validate_name`, `validate_user`, `Reference::encode` (`src/reference.rs:265,292,384`) |
| `fstree.ResolvePath`, `ResolveEntry`, `CollectEntries`, `WriteContent` | `fstree::resolve_path`, `resolve_entry` (returns `Option<Entry>`: handle `None` — §8), `collect_entries`, `write_content` (`src/fstree/read.rs:463,498,536,653`) |
| `amberpack.ParseRecord`, `DecodePayload`, `RecHeaderSize` | `amberpack::parse_record`, `decode_payload`, `REC_HEADER_SIZE` (`src/amberpack.rs:157,205,38`) |
| `key.Parse`, `Key.String` | `key::Key` parse / `Display` (`src/key.rs`) |

Error `Display` texts in core-rs must equal Go's where the CLI prints them (`reference name …`,
`fstree: …`, `… is not a file-content object (type …)`); verify with the golden cases.

### 4.8 Mapping onto Rust iroh (via the transport area)

`relay_mode_of(url, no_relay)` → `Option<iroh::RelayMode>`: `no_relay` → `RelayMode::Disabled`;
url → `iroh_base::RelayUrl::from_str` (`iroh-base-1.0.3/src/relay_url.rs:38-45`, WHATWG `url`
parsing) → `RelayMode::Custom(RelayMap::from(url))`; else `RelayMode::Default`. Go's `url.Parse`
accepts relative strings such as `foo` that `url::Url` rejects (§7). Ephemeral identity:
`iroh::SecretKey::generate(rng)`. `Discover`/`Announce` map to the transport area's discovery
configuration (mDNS via `iroh-mdns-address-lookup-0.4.0`, n0 DNS only with relays).

### 4.9 Crate dependencies (all present in the offline registry)

| Crate | Version | Use |
|---|---|---|
| `tokio` | 1.53.1 | runtime, `signal`, `time`, `sync` |
| `tokio-util` | 0.7.19 | `CancellationToken` |
| `crossterm` | 0.29.0 | raw mode, key events (`event-stream`), cursor/clear |
| `futures` | 0.3.34 | `StreamExt` for events and the watch stream |
| `libc` | 0.2.189 | `fstat`, `getuid`, `getpwuid_r`, `localtime_r`, `ioctl(TIOCGWINSZ)` |
| `thiserror` | 2.0.20 | error enums |
| `amber-store-core` | 0.3.0 (path/git, rev a85ffa1) | stores, ingest, fstree, reference, amberpack, key |
| dev: `tempfile` | 3.27.0 | fixtures |
| dev: `similar` | 2.7.0 | golden diff messages |

Not used: `clap` (§4.2), `ratatui` 0.30.2 (its buffer model re-lays-out the frame; the inline
string renderer is simpler and exact), `tracing` (§4.3).

---

## 5. Golden vectors

### 5.1 Generator program (`tools/cligen`, Go)

1. A Go module with `replace github.com/amber-store/dstore => <dstore checkout at v0.1.9>`, the
   `require` blocks and `go.sum` of dstore, and **copies** of `cmd/dstore/{main,client,wc,size,tui}.go`
   (package `main`) plus one `harness.go`.
2. `harness.go` `init()`:
   - `HARNESS_ARGS` set → `os.Args = append([]string{"dstore"}, decoded args...)` and return (the
     real `main` runs);
   - `HARNESS_VECTORS` set → print the unit vectors (§5.4) and `os.Exit(0)`;
   - `HARNESS_CASES` set → build fixtures, then for each case exec `os.Executable()` with a clean env
     (drop `DSTORE_*`, `AMBER_STORE`, `HARNESS_*`), the case env, `HARNESS_ARGS`, cwd and stdin;
     capture stdout, stderr, exit code (30 s timeout); write JSON; `os.Exit(0)`.
3. Run with go1.26.5 (`GOTOOLCHAIN=local GOPROXY=off GOFLAGS=-mod=mod`), `main.version` left `dev`,
   stderr and stdin as pipes (plain progress path), fixed `TZ`.
4. The Rust test harness runs the built `dstore` binary with the same cases and compares all three
   outputs byte for byte (with fixture-dependent hashes normalised, or fixtures with fixed mtimes).
5. Clean up the generator binary afterwards (`go run` caches executables in `GOCACHE` since Go 1.24).

### 5.2 CLI cases (all captured once for this spec; outputs in §3)

- Root: `[]`, `--help`, `-h`, `-help`, `help`, `h`, `--version`, `-v`, `--version refs`, `""`,
  `nosuch`, `help nosuch`, `version`, `--bogus`, `--log-level`, `help help`.
- `--help` of all 21 top-level commands and all 30 subcommands.
- Help variants: `cluster`, `store`, `token`, `ref`, `help cluster`, `cluster help`,
  `cluster help init`, `cluster h`, `help refs`, `help cluster init`, `refs help`, `refs h`,
  `refs -h`, `ls help x`, `cluster nosuch`, `store push help`.
- Flag errors: `refs --bogus`, `cluster status --bogus`, `refs ---x`, `refs -=x`, `refs --ticket`,
  `store pull --local L --jobs x n`, `gc run --garbage x`, `serve --gc-interval x`,
  `cluster replicas --yes=maybe 3`, `refs --log-level debug`, `refs --no-relay=maybe`,
  `status --ticket x`.
- Ticket/env: `refs`, `refs --ticket=`, `refs --ticket bogus`, `refs -ticket bogus`,
  `refs --ticket=bogus`, `refs --ticket 'dstore1!!!'`, `refs --ticket dstore1`,
  `refs --ticket dstore1aaaa`, `refs --ticket zz,yy`, env `DSTORE_TICKET=bogus`, `DSTORE_TICKET=`,
  `DSTORE_NO_DISCOVERY=maybe`, `DSTORE_NO_DISCOVERY=`, `--log-level debug refs`.
- Required flags: `store pull`, `store pull n`, `AMBER_STORE= store pull`, `node join`,
  `node join --seed x`, `node join extra`.
- Argument validation: `store pull --local /nonexistent`, `store push --local /nonexistent p`,
  `… p bad@name`, `… p ""`, `… p @`, `DSTORE_NO_TUI=maybe store pull --local L n`, `cat x`,
  `cat a b --ticket x`, `watch`, `watch p --ticket x`, `ls`, `clone`, `clone a@b`, `clone trees/x`,
  `clone --ticket bogus trees/x`, `DSTORE_NO_DISCOVERY=maybe DSTORE_TICKET=bogus clone trees/x`,
  `init`, `init trees/x`, `status`/`fetch`/`pull`/`push` in `/`, `diff --remote --incoming`,
  `diff --remote=false --incoming`, `node remove zz`, `node remove`, `node weight <hex> x`,
  `node zone <hex>`, `voter add`, `voter add <base32 id>`, `gc why zz`, `gc why <hex>`,
  `catalog restore`, `catalog restore <hex>`, `catalog restore /etc/hosts`, `cluster replicas x`,
  `cluster replicas`, `cluster replicas 300`, `cluster replicas 3` with stdin `n\n`, `""`,
  `yes please\n`, `cluster replicas 0` with stdin `Y`, `cluster replicas --yes 3`, `cluster ticket`,
  `cluster ticket --store /nonexistent`, `cluster status --store /nonexistent`,
  `DSTORE_STORE=/nonexistent cluster status`, `serve`, `serve --store X --pack-size 0`,
  `serve --store X --pack-size 1.5Gi`, `DSTORE_PACK_SIZE=x serve --store X`, `cluster init`,
  `node join --seed bogus --token x`, `node join --seed <hex> --token zz`,
  `node join --seed <hex> --token <hex>`, `token create`, `transition status`, `ref get x`,
  `ref delete x --expected-version zz`.
- Working copy fixture 1 (§3.5): `status`, `status` from `sub/`, `diff`, `diff --stat`, `diff a.txt`,
  `diff a.txt --stat`, `diff ../`, `diff --incoming`, `diff --remote`, `push`, `push --user ""`,
  `fetch --ticket zz`, `init --ticket bogus trees/x`.
- Working copy fixture 2: `status`, `diff --incoming`, `diff --incoming --stat`, `diff --remote`, `push`.

### 5.3 Cases to add (not captured yet)

- Progress bar at 100 % and at 0 < fw < tw with odd `tw`; `View()` with `NO_COLOR` and
  `TERM=xterm-256color` through a real Bubble Tea program writing to a pty (downsampling bytes).
- `dstore push 2>/dev/null` (TUI into a character device) — expect no output.
- `pull` conflict output (needs a cluster: use the in-memory transport cluster of
  `node/cluster_test.go` inside the generator, or record against a loopback cluster).
- `cluster status`, `refs`, `watch`, `ref get`, `ls`, `cat`, `store push/pull`, `clone`, `init`,
  `fetch`, `pull`, `push`, admin ops against a three-node loopback cluster started by the generator
  (the stdout formats of §3.4); normalise ids, keys, times.
- `cat NAME /` panic output and exit status.
- An env value for every bool flag (`DSTORE_NO_DISCOVERY=1`, `=t`, `=FALSE`).
- `ls NAME //`, `cat NAME ./x`, `ls NAME ../x`.

### 5.4 Unit vectors *(verified)*

HumanBytes: `0` → `0 B`; `1` → `1 B`; `1023` → `1023 B`; `1024` → `1.0 KiB`; `1536` → `1.5 KiB`;
`1048575` → `1024.0 KiB`; `1048576` → `1.0 MiB`; `1073741823` → `1024.0 MiB`; `1073741824` →
`1.0 GiB`; `5497558138880` → `5.0 TiB`; `1<<50` → `1.0 PiB`; `1<<60` → `1.0 EiB`;
`9223372036854775807` → `8.0 EiB`; `-1` → `-1 B`; `-2048` → `-2048 B`.
Rate: `(1000, 0)` → `-`; `(3<<20, 2s)` → `1.5 MiB/s`.

statusLine / fraction (inputs for the lines of §3.6, in order):
`{Objects 3, TotalObjects 4}`, rate 0 → fraction 0.75;
`{0, 0, Bytes 512}`, rate 100 → 0; `{5, 10, 1024, 2048}`, 512 → 0.5;
`{5, 10, 1<<30, 5<<30}`, 3.7e6 → 0.2; `{5, 10, 3000, 1000}`, 10 → 1 (clamped);
`{1, 2, 10, 100}`, 0.5 → 0.1; `{1, 2, 10, 100000000}`, 1 → 1e-07.

rateMeter (window 5 s), `add(t, n)` sequence → result (samples kept):
`(0s, 0)` → 0 (1); `(1s, 1000)` → 1000 (2); `(2s, 1100)` → 550 (3); `(3s, 1200)` → 400 (4);
`(7s, 1600)` → 100 (3); `(8s, 1700)` → 100 (3); `(8s, 1700)` → 100 (4); `(9s, 100)` → 0 (5);
`(15s, 5000)` → 816.6666666666666 (2).

parseSize: `"0"` 0; `"1024"` 1024; `"512Ki"` 524288; `"256Mi"` 268435456; `"2Gi"`, `"2gi"`,
`"2GiB"`, `"2G"`, `"2GB"`, `" 2Gi "`, `"2Gib"` 2147483648; `"1Ti"` 1099511627776; `"2b"` 2;
`"2kb"` 2048; `"8Ti"` 8796093022208; `"8388607Ti"` 9223370937343148032. Errors:
`""` → `bad size "": want a number with an optional Ki/Mi/Gi/Ti suffix`; `"Gi"` → same with `"Gi"`;
`"-1"` → same with `"-1"`; `"2X"` → `bad size "2X": unknown unit "X" (want Ki, Mi, Gi or Ti)`;
`"2.5Gi"` → `bad size "2.5Gi": unknown unit ".5Gi" (want Ki, Mi, Gi or Ti)`;
`"1e3"` → `bad size "1e3": unknown unit "e3" (want Ki, Mi, Gi or Ti)`;
`"2 Gi"` → `bad size "2 Gi": unknown unit " Gi" (want Ki, Mi, Gi or Ti)`;
`"9999999999Ti"`, `"8388608Ti"` → `bad size "…": too large`;
`"99999999999999999999"` → `bad size "99999999999999999999": strconv.ParseInt: parsing "99999999999999999999": value out of range`.
(The unit check is `strings.TrimSuffix(strings.ToLower(unit), "b")` against `"", k, ki, m, mi, g, gi, t, ti`;
the error message shows the original unit text.)

hexDecode: `""` → empty; `"ab"` → `ab`; `"ABcd"` → `abcd`; `"abc"` → `ab`; `"a"` → empty;
`"zz"` → `bad hex "zz"`; `"0g"` → `bad hex "0g"`.

Durations: `0` → `0s`; `999ms` → String `999ms`, Round(s) `1s`; `1.5s` → `1.5s` / `2s`; `59s`;
`61s` → `1m1s`; `3723s` → `1h2m3s`; `25h` → `25h0m0s`. RTT `Round(ms)`: 250µs → `0s`, 499µs → `0s`,
500µs → `1ms`, 1.5ms → `2ms`, 3ms → `3ms`, 1234ms → `1.234s`. ETA example
`left 1<<30, rate 3.7e6` → `4m50s`.

RFC3339 of `time.Unix(0, ns)`: `0` → UTC `1970-01-01T00:00:00Z`, Europe/Berlin
`1970-01-01T01:00:00+01:00`, America/New_York `1969-12-31T19:00:00-05:00`;
`1758198896123456789` → `2025-09-18T12:34:56Z`, `2025-09-18T14:34:56+02:00`,
`2025-09-18T08:34:56-04:00`; `-1` → `1969-12-31T23:59:59Z`, `1970-01-01T00:59:59+01:00`,
`1969-12-31T18:59:59-05:00` (floor, not truncation toward zero).

describeChange (`fmt.Sprintf("  %-9s %s", kind, describeChange)`):
Added file `a.txt` → `  new       a.txt`; Added dir `d` → `  new       d/`; Deleted dir →
`  deleted   d/`; TypeChanged 0o120777→0o40755 `x` → `  type      x/ (symlink → directory)`;
ModeChanged 0o100644→0o104755 `run.sh` → `  mode      run.sh (0644 → 4755)`; Modified `f` →
`  modified  f`; TypeChanged fifo→socket → `  type      p (fifo → socket)`; char→block →
`  type      c (char device → block device)`; 0o170644→file → `  type      w (type 0170000 → file)`;
`Kind(9).String()` → `Kind(9)`.

filterPaths with root = cwd and changes `[a.txt, sub, sub/b.txt, subx/c]`: `["."]` → all;
`["sub"]` and `["sub/"]` → `[sub, sub/b.txt]`; `["./sub/b.txt"]` → `[sub/b.txt]`; `["su"]` → `[]`;
`[".."]` → error `.. is outside the working copy`; `["../x"]` → `../x is outside the working copy`;
`["/"]` → `/ is outside the working copy`; `[<root absolute>]` → all.

Tickets: `Ticket{ClusterID: 16×aa, Incarnation: 1, Members: [{ID: 32×01, Addrs: ["192.168.1.10:4433"]}, {ID: 32×ab}, {ID: 32×01}, {ID: 0102}]}`
→ CBOR
`a30050aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa01010284a200582001010101010101010101010101010101010101010101010101010101010101010181713139322e3136382e312e31303a34343333a1005820ababababababababababababababababababababababababababababababababa10058200101010101010101010101010101010101010101010101010101010101010101a100420102`,
`Encode()` →
`dstore1umafbkvkvkvkvkvkvkvkvkvkvkvkvkqbaebijiqalaqacaibaeaqcaibaeaqcaibaeaqcaibaeaqcaibaeaqcaibaeaqcaibqfytcojsfyytmobogexdcmb2gq2dgm5babmcbk5lvov2xk5lvov2xk5lvov2xk5lvov2xk5lvov2xk5lvov2xk5lueafqiabaeaqcaibaeaqcaibaeaqcaibaeaqcaibaeaqcaibaeaqcaibagqqaqqbai`,
`IDs()` → `0101010101010101010101010101010101010101010101010101010101010101,abababababababababababababababababababababababababababababababab`
(duplicates and non-32-byte ids skipped). The upper-case form parses too. `ParseNodeID("zz")` →
`view: bad node id "zz"`; `node.ShortID([01])` → `?`; `ShortID(32×ab)` → `abababab`.

teaHandler (logger `With("node", "abcd")`, level Info):
`Info("uploaded", "objects", 3, "bytes", int64(3<<20), "took", 2s, "path", "direct")` →
`uploaded node=abcd objects=3 bytes=3.0 MiB took=2s path=direct`;
`Warn("upload failed", "err", "timeout: no recent network activity")` →
`upload failed node=abcd err="timeout: no recent network activity"`;
`Info("q", "s", "a\"b", "t", "x\ty", "e", "", "n", "a=b", "bytesint", 5, "bytes", uint64(5))` →
`q node=abcd s=a"b t="x\ty" e= n=a=b bytesint=5 bytes=5` (uint64 `bytes` is not humanised);
`Debug` records are dropped. `formatEvent` at 12:34:56: INFO →
`12:34:56  uploaded …`; WARN → `\x1b[33m12:34:56  upload failed …\x1b[m`; ERROR →
`\x1b[31m12:34:56  boom node=abcd\x1b[m`.

---

## 6. Go tests worth porting

From `cmd/dstore`:

| Test | File | What to keep |
|---|---|---|
| `TestParseSize` | `size_test.go:9-38` | the good/bad tables (extend with §5.4 vectors) |
| `TestPackSizeFlag` | `size_test.go:40-68` | default, flag, env, empty env = default, bad values `0`, `x`, `1.5Gi` |
| `TestResolveTicket` | `wc_test.go:5-20` | precedence flag > stored > env, error when all empty |
| `TestUIModel` | `tui_test.go:15-56` | model update/view strings: `push demo`, `5/10 objects`, `1.0 KiB / 2.0 KiB`, `512 B/s`, `eta 2s`, `50%`, `direct`, `3ms`, `sending`, event text, `waiting for ack`, ctrl+c cancels, done → quit, `failed: context canceled`, `cancelling` |
| `TestRateMeter` | `tui_test.go:58-74` | first sample 0, 1000 after 1 s, windowed ≈100 |
| `TestTeaHandler` | `tui_test.go:76-92` | debug hidden, attribute formatting, quoting, level |
| `TestStatusLine` | `tui_test.go:94-106` | `3/4 objects  0 B/s`, fraction by objects, clamp to 1 |

From urfave/cli v2.27.7, for the framework (names verified in the module cache):
`context_test.go` `TestCheckRequiredFlags`, `TestContext_IsSet`, `TestContext_IsSet_fromEnv`;
`flag_test.go` `TestFlagsFromEnv`, `TestFlagStringifying`, `TestBoolFlagHelpOutput`,
`TestStringFlagHelpOutput`, `TestStringFlagWithEnvVarHelpOutput`, `TestStringSliceFlagHelpOutput`,
`TestIntFlagHelpOutput`, `TestInt64FlagHelpOutput`, `TestUintFlagHelpOutput`,
`TestDurationFlagHelpOutput`, `TestStringSliceFlag_TrimSpace`; `help_test.go`
`Test_helpCommand_Action_ErrorIfNoTopic`, `Test_helpSubcommand_Action_ErrorIfNoTopic`,
`TestShowCommandHelp_HelpPrinter`, `TestHelpNameConsistency`, `TestWrap`; `app_test.go`
`TestApp_CommandWithFlagBeforeTerminator`, `TestApp_CommandWithDash`,
`TestApp_CommandWithNoFlagBeforeTerminator`, `TestApp_ParseSliceFlags`,
`TestRequiredFlagAppRunBehavior`, `TestApp_Run_Help`, `TestApp_Run_Version`,
`TestApp_CommandNotFound`, `TestApp_Run_SubcommandHelpName`, `TestApp_Run_CommandHelpName`;
`errors_test.go` `TestHandleExitCoder_ExitCoder`. Port only the cases that exercise features dstore
uses (no categories, aliases on commands, short-option handling, custom templates).

The primary lock is the golden CLI suite of §5.2 run against the Rust binary.

---

## 7. Gaps in core-rs and Rust iroh, with workarounds

| Gap | Effect on the CLI | Workaround |
|---|---|---|
| core-rs `refstore` is redb, Go's is Pebble (`core-rs PORTING.md`, "Different, by design") | `store push/pull --local DIR` directories written by Go cannot be opened by Rust and vice versa (the packstore part interoperates) | Document; detect a Pebble directory (`MANIFEST-*`, `OPTIONS-*` files) and fail with a clear error instead of creating a redb file next to it; open decision §8 |
| No Pebble, paxos acceptor or node in Rust | `cluster init`, `serve`, `node join`, `catalog restore --store`, and ticket derivation from `--store` (`localTicket`) | Keep definitions and validation; action error (§2.9, §8) |
| core-rs has `human_bytes` only in an example, and it promotes at 1023.95 | wrong progress text if reused | Use the client area's `human_bytes` (no promotion) |
| core-rs `parse_go_duration`, `format_go_duration`, `round_ms`, `rfc3339_local`, `local_tm` live in `examples/amber-store.rs`, not the library | needed for durations, ETAs, `refs` times | Copy into `src/goutil/` (LGPL-3.0-only code: keep the licence notice) |
| core-rs `fstree::resolve_entry` returns `Option<Entry>` where Go returns a nil pointer | `cat NAME /` panics in Go | Decide in §8; no panic is possible in safe Rust without an explicit choice |
| No Go `strconv.Quote` / `unicode.IsPrint` in Rust std | `%q` zones, slog quoting, TUI event quoting | Port `strconv/quote.go` + `isprint.go` tables |
| Go `net/url.Parse` accepts relative strings (`--relay foo`); `iroh_base::RelayUrl::from_str` uses `url::Url` (absolute only) | `--relay foo` succeeds in Go (and fails later at dial), fails early in Rust | Emit Go's error shape `failed to parse relay URL: …` for inputs `url::Url` rejects; list as a known deviation |
| go-iroh default relay map (`*.relay.n0.iroh-canary.iroh.link.`, `go-iroh relay/relay.go:23-33`) vs Rust iroh `RelayMode::Default` | which relays are used by default; tickets carry relay URLs | Transport area: pin the same map explicitly |
| Bubble Tea renderer and colorprofile have no Rust port | TUI bytes differ | Hand-rolled inline renderer + colorprofile subset (§4.4) |
| Go `os.Getwd` prefers `$PWD` when it names the cwd; Rust `current_dir` returns the resolved path | paths in messages (`… is inside the working copy at …`, `init` root) differ under symlinked cwds | Port Go's `Getwd`: use `$PWD` if absolute and `stat($PWD)` equals `stat(".")` (dev, ino) |
| Pure-Go `user.Current` fallback | default push user | libc `getpwuid_r`, then `$USER` when `$USER` and `$HOME` are set |
| Rust iroh internals log through `tracing`, not the dstore logger | fewer/different log lines at `--log-level debug` | Accept; the log format is compatible, the message set of iroh internals is not a contract |

---

## 8. Risks and open decisions

**Open decisions**

1. **urfave quirks.** Reproduce the truncated `help <parent>` output, `help help`, the shared help
   HelpName, `<leaf> help|h` capturing a positional argument, `Incorrect Usage` help with the
   `COMMANDS: help, h` section, trailing spaces? Recommendation: reproduce all of them (they are
   deterministic and locked by §5.2 goldens); document them in the project README.
2. **Node-side commands** (`cluster init`, `serve`, `node join`, `catalog restore` restore step,
   `--store` ticket derivation): (a) keep the definitions for help parity and fail with an explicit
   message once a store would be opened — recommended; (b) omit them (breaks help parity); (c) shell
   out to a Go `dstore` binary when present. Needs the exact message text decided (suggestion:
   `<command> needs the Go dstore node (not available in dstore-client-rs)`), and whether to exit 1.
3. **`cat NAME /` panic.** Go exits 2 with a goroutine dump. Options: mimic (print
   `panic: runtime error: invalid memory address or nil pointer dereference` and exit 2) or return
   `not a regular file with content` with exit 1. Recommendation: the latter, listed as a deviation.
4. **Local refstore format** for `store push/pull`: accept redb (no sharing of `--local` directories
   with Go) or build a Pebble-compatible reader/writer. Recommendation: accept, detect and refuse
   Pebble directories.
5. **TUI fidelity.** Byte-identical frames are only defined before downsampling; Bubble Tea's
   terminal control sequences and downsampled colours are not reproducible cheaply. Recommendation:
   `UiModel::view` byte-identical (golden §3.6), renderer visually equivalent.
6. **Logger facade vs tracing** for the library: the client area must adopt the same facade (§4.3)
   so its log lines have Go's attribute kinds; decide jointly with the client spec.
7. **`--relay` laxness** (§7): accept the early failure for relative URLs?
8. **Version string** source (`DSTORE_VERSION` at build time, default `dev`) and whether Rust
   release builds print the Go tag they are compatible with or their own version.

**Risks**

1. The help text is compared byte for byte, and the cell-width rules (rune counts, `offset` in
   bytes, padding of empty cells) are easy to get subtly wrong; generate goldens for every command.
2. Flag parsing stops at the first positional argument; users coming from clap-style CLIs will put
   flags last and get positional arguments instead (e.g. `diff a.txt --stat`). Reproduce, do not
   "fix".
3. Env semantics: an empty `AMBER_STORE` satisfies `--local` and opens `packstore`/`refs` in the
   current directory; empty bool env vars mean false; invalid bool env values are usage errors for
   `clientFlags`/`noTUIFlag` but silently ignored by `wcConfig` for `DSTORE_NO_DISCOVERY`.
4. `isTerminal` is a character-device check (`/dev/null` counts): using `isatty` changes which mode
   runs and what reaches stderr.
5. Signal behaviour: later Ctrl+C presses are swallowed while a command runs; `status`/`diff` die
   on SIGINT. A Rust implementation that installs handlers globally changes `status`/`diff`.
6. Validation order is observable (e.g. `store push` builds the tree before failing on a missing
   ticket; `cluster replicas` prompts before dialing; `push` checks the user after dialing;
   `catalog restore` fetches before requiring `--store`). Follow §2.8 step by step.
7. Output streams: Go writes stdout unbuffered; Rust must flush per line (`watch`, `refs`) and
   before exiting through `std::process::exit`.
8. `%x` of an empty byte slice prints nothing, `%q` uses Go escaping, `%v` of bools and slices
   (`waiting for [a b]` comes from the server); port the formatting helpers exactly.
9. Time zones: `refs`/`watch`/`ref get` times, slog times and TUI event times use the local zone;
   Go reads `TZ`/zoneinfo itself, Rust via libc `localtime_r` — identical where the same tzdata is
   present; containers without tzdata give UTC in both.
10. `scripts/e2e-loopback.sh` is stale (`push --local`); porting it verbatim fails.
11. CBOR: floats must use the shortest preserving form (`--garbage 0.25` → `f93400`), nil byte slices
   in `node.Status` decode from null, unknown reply keys must be ignored.
12. The prompt of `cluster replicas` reads with `fmt.Scanln` (byte by byte up to the newline; the
    first whitespace-separated token; empty line or EOF → `aborted`; a token starting with `y`/`Y`
    accepts, including `yes please` and `Y` without a newline). Read one line from unbuffered stdin
    and take its first token.


---

## Addenda (synthesis)

Added by the architecture synthesis. `PORTING.md` is normative where it differs from this spec.

1. **Crates.**
   - `dstore-gocli`: the urfave framework (`goflag`, `help`, tabwriter, `Context`, `CliError`, `run`).
   - `dstore-cli`: `app` table, `nodeside`, `common`, `cmd_admin`, `cmd_client`, `cmd_wc`, `size`, `progress`.
   - §4.3 `log.rs` and §4.6 `goutil` → `dstore-gocompat` (`slog`, `time`, `strconv`, `quote`, `path`, `os`).
   - Admin and status types → `dstore_wire::admin`.
   - `human_bytes`/`rate` → `dstore-client`.
   - The binary is the root package's `src/bin/dstore.rs` → `dstore_cli::main_entry()`.
2. **Open decisions resolved.**
   1. Reproduce every urfave quirk.
   2. Keep the node-side definitions and fail with the exact messages of PORTING §2.2, after Go's
      pre-store validation (identity read for `--store`).
   3. `cat NAME /` → `go_panic_exit("runtime error: invalid memory address or nil pointer dereference")`,
      exit 2 (overrides the recommendation here; PORTING C24).
   4. redb refs; Pebble directories refused at open for push and pull with the PORTING §2.3 text.
   5. `UiModel::view` byte-identical; the renderer only visually equivalent.
   6. A shared slog facade in `gocompat`.
   7. `--relay` leniency: accept like Go (DD-13), overriding §7 "fail early".
   8. Version from `DSTORE_VERSION`, default `dev`.
3. **Default relay map.** §2.5 quotes go-iroh's own comment ("number0 production map"), but the hosts
   are `use1-1/usw1-1/euc1-1/aps1-1.relay.n0.iroh-canary.iroh.link.` (`relay/relay.go:23-30`). Pin
   exactly those (PORTING C2). §4.8's `RelayMode::Default` and mDNS through iroh-mdns-address-lookup
   are superseded (PORTING C1-C3).
4. **Stale script.** `scripts/e2e-loopback.sh` has the stale `--local` lines at 42 and 46 at HEAD.
5. **Gaps found at synthesis.**
   - **SIGPIPE.**
     - Go dies by SIGPIPE when writing to a closed stdout or stderr: `dstore cat NAME big | head -c1`
       and `dstore watch 'trees/**' | head -1` end with the shell showing 141.
     - Rust ignores SIGPIPE, and `print!` then panics (exit 101).
     - Fix: `main_entry` sets `SIGPIPE` to `SIG_DFL` first (PORTING §5.9).
     - Add CLI cases for both commands.
   - **Exit.** Flush stdout and stderr, then `std::process::exit(code)` without dropping the tokio
     runtime; background `refresh_view` tasks are abandoned, as goroutines are.
   - **`ls`** prints entry names as raw bytes (`string(e.Name)`), not lossy UTF-8.
   - **Session close.** Every command that dialed calls `cluster.close()` and then awaits
     `Endpoint::close()` bounded to 3 s (PORTING DD-11), so Go nodes see CONNECTION_CLOSE.
6. **`node join` validation order in Rust:**
   1. Framework parsing and required flags.
   2. `ticket::parse(--seed)`.
   3. The token must decode with strict hex to 32 bytes.
   4. `--store` empty → `no store directory: set --store or $DSTORE_STORE`.
   5. `pack_size`.
   6. The fixed node-side error.
