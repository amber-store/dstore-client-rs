# impl-gocli: `dstore-gocli` (layer L2)

Owner: gocli. Files: `crates/gocli/src/{lib,app,context,flag,goflag,help}.rs`, `crates/gocli/src/app/tests.rs`,
`crates/gocli/src/app/go_cases.rs` (generated), `crates/gocli/src/help/tests.rs`,
`crates/gocli/src/help/go_tabwriter_tests.rs` (generated). `Cargo.toml` is unchanged.

## Public API

Every PORTING.md §4.11 signature is kept. Added items (nothing changed):

- `pub fn dispatch(app, args, stdout: &mut dyn Write, getenv: &dyn Fn(&str) -> Option<OsString>) -> Result<Option<(Action, Context)>, CliError>`:
  the synchronous part of `run` (everything up to the action). `run` is `dispatch` with `std::env::var_os`, then
  `action(&ctx).await`.
  - Why: code that takes a `&Context` (e.g. `dstore_cli::size::pack_size`, the port of `TestPackSizeFlag`) can
    be tested with a `Context` built from arguments and an environment map. Without it a test has to mutate the
    process environment and capture results from an `fn`-pointer action.
- `Context::value(name) -> Option<FlagValue>`: urfave `Context.Value`.
- `FlagDef::names() -> Vec<String>`: urfave `FlagNames` (each name cut at its first `,` or space).
- Derives: `Clone, Debug, PartialEq` on `FlagKind` and `FlagDef`; `Debug, Default` on `Context`;
  `thiserror::Error` on `CliError` (`Display` is the message).
- `pub mod goflag`: `FlagSet` (`new`, `name`, `add_value`, `define`, `var`, `lookup`, `slot`, `is_set`, `visit`,
  `nflag`, `parsed`, `args`, `set`, `parse`), `Value` (`is_bool_flag`, `string`, `set`), `FlagError`,
  `ERR_PARSE`, `ERR_RANGE`, `ERR_HELP`.
- `pub mod help`: `wrap`, `WRAP_AT`, `unquote_usage`, `prefixed_names`, `with_env_hint`, `stringify_flag`,
  `CommandRow`, `CommandCategory`, `HelpData`, `Template`, `Execution`, `execute`, `print_help`, `TabWriter`
  (all of Go's flags: `FILTER_HTML` … `DEBUG`, `ESCAPE`).

## Contract for callers (cli-app, dstore-cli)

- **`run(app, args, stdout)`**: `args` is the whole argv, program name first, as Go's `App.Run(os.Args)`. The
  first element is not interpreted. Pass `std::env::args_os().collect()`.
- **Errors.**
  - `CliError::Msg(text)` → `dstore: <text>` (after the errno rewrite of PORTING.md §5.2), exit 1.
  - `CliError::Exit { msg, code }` → `<msg>\n`, exit `code` (3 for `No help topic for '…'`).
  - As in Go, the exit code 3 applies only when the help action runs as a command's action (urfave's
    `HandleExitCoder` runs after the action). Through `--help` with an argument it is a plain error:
    `dstore refs --help extra` → `dstore: No help topic for 'extra'`, exit 1.
- **Output.** Help, version and `Incorrect Usage: …` go to `stdout`; write errors are ignored, as urfave does.
  The caller flushes.
- **Environment.** Read with `std::env::var_os`: a variable set to the empty string is found (Go
  `syscall.Getenv`).

## Model

- **urfave mutates its commands while it runs.** A run keeps that state in an arena of command nodes: the root
  (the app), one node per `CommandDef`, the shared `helpCommand` and `helpCommandDontUse`.
  - `setup` appends the help command and the help flag, names the subcommands and snapshots the categories.
  - `ShowCommandHelp` appends `helpCommandDontUse` and the help flag.
  - The help command keeps the `HelpName` of the first parent that set it up, and `" help"` when it ran from the
    root.
- **Templates** are executed as code over `HelpData`. A command whose categories were never set up makes
  `visibleCommandCategoryTemplate` panic in Go. That comes out here as `Execution { complete: false }`, and
  `print_help` then drops what the tabwriter did not flush: the truncated `help <parent>`.
- **Validation order** is `Command.Run`'s:
  1. setup, then `checkDuplicatedCmds`;
  2. `Apply` of every flag, in order (environment);
  3. `Parse(args.Tail())`, then `normalizeFlags` (`Cannot use two forms of the same flag: h help`);
  4. `Incorrect Usage` plus help;
  5. `checkHelp`;
  6. `checkVersion` (root only);
  7. required flags, with the help action;
  8. subcommand dispatch;
  9. the action, or the help action.
- **Go strings are bytes.**
  - `goflag` parses `OsString` bytes, and error texts echo them raw on stdout.
  - The bool and numeric parsers see every non-ASCII byte as `!`, where Go's parsers reject a non-ASCII byte in
    exactly the same way, and texts quote the original bytes.
  - `time.ParseDuration` over non-UTF-8 bytes (only its error text) and `time.quote` are ported in `goflag`.

## Deviations

1. **Non-UTF-8 argument bytes in a returned error text are lossy** (U+FFFD), because `CliError` holds a
   `String` (the DD-8 class). The `Incorrect Usage: …` line on stdout keeps the raw bytes. Of the 5124
   differential cases, 61 differ, all in this way only.
2. **Where Go panics, a `CliError::Msg` with the panic text is returned instead:** a flag defined twice at one
   level, a name beginning with `-` or containing `=`. These are defects of the command table.
   - Also a panic in Go: an app named `help` or `h` whose root help action would dereference the nil parent
     context. Here the action stays on the root.
3. **Not modelled:** urfave `StringSlice.Set`'s deserialization branch (values starting with the per-process
   `sl:::<nanos>:::` prefix).
4. **Fresh command state per call.** `run`/`dispatch` build new command state on every call. Go's `helpCommand`
   keeps state across `App.Run` calls in one process, but dstore runs once per process.
5. **Program name.** An empty `AppDef.name` is taken from `std::env::args_os()`'s first element (Go
   `filepath.Base(os.Args[0])`), not from `args`.
6. **Not ported** (absent from `AppDef`/`CommandDef`/`FlagDef` and unused by dstore): categories on commands
   and flags, `Hidden`, `UsageText`, `Args`, `DefaultText`, `Destination`, `Base`, `FilePath`, `KeepSpace`,
   `Before`/`After`, flag actions, bash completion, suggestions, custom templates and printers,
   `CommandNotFound`, `OnUsageError`, `UseShortOptionHandling`, `SkipFlagParsing`, authors, copyright. `HelpData`
   still carries `usage_text`, `args`, `category`, `copyright` and named categories, so the template code stays
   complete.

## Verification

- **Differential harness** (throwaway; scratchpad only, binaries deleted).
  - The app: a dstore-shaped urfave v2.27.7 app in Go:
    - every flag kind, env vars (empty, invalid, non-UTF-8, two variables for one flag), aliases, required
      flags;
    - nested parents three levels deep, a description, multi-line usage, args usage and default values, UTF-8
      names (rune widths vs byte offsets), a command without usage or flags;
    - an action printing `IsSet`, `String`, `Bool`, `Int`, `Int64`, `Uint`, `Float64`, `Duration` and
      `StringSlice` for 53 names, and returning a plain error or `cli.Exit(…, 5)`.
  - Each case runs in a fresh Go process. The same app is mirrored in Rust over `dispatch`.
  - 5124 cases: 126 handpicked (every cli.md §5.2 help, unknown, usage and required shape) and 5000 random
    argv/env combinations, including non-UTF-8 bytes.
  - Result: 0 differences except deviation 1.
- **Committed tests** (`cargo test -p dstore-gocli`: 32 tests, none ignored):
  - `app/go_cases.rs`: 108 of those cases (help, unknown, usage, required, env errors, and action cases with
    their probe lines), generated from the harness output. `app/tests.rs` holds the mirrored app.
  - `help/go_tabwriter_tests.rs`: the 53-row table of go1.26.5 `tabwriter_test.go`, checked written all at
    once, byte by byte and in Fibonacci slices.
  - urfave ports:
    - `TestCheckRequiredFlags`, `TestContext_IsSet`, `TestContext_IsSet_Aliases`,
      `TestContext_IsSet_fromEnv`;
    - `TestFlagsFromEnv` (the dstore kinds, base 0), `TestFlagStringifying`, `Test*FlagHelpOutput`,
      `TestStringFlagWithEnvVarHelpOutput`, `TestStringSliceFlag_TrimSpace`;
    - `Test_helpCommand_Action_ErrorIfNoTopic`, `TestWrap` and the strings of `TestWrappedHelp`;
    - `TestApp_CommandWithFlagBeforeTerminator`, `TestApp_CommandWithDash`,
      `TestApp_CommandWithNoFlagBeforeTerminator`, `TestApp_ParseSliceFlags`, `TestRequiredFlagAppRunBehavior`,
      `TestApp_Run_Help`, `TestApp_Run_Version`, `TestApp_Run_CommandHelpName`;
    - `TestHandleExitCoder_ExitCoder`.
  - Go flag package: `testParse`, `TestParseError`, `TestRangeError`, `TestInvalidFlags` and the redefinition
    panic texts.
  - `ParseDuration` texts for non-UTF-8 input, captured from Go by a probe.
  - The verified help texts of cli.md §3.1-§3.2 (refs, node, the truncated `help node`, the app shape).
- **Gates:** `cargo clippy -p dstore-gocli --all-targets -- -D warnings` is clean, and `rustfmt --edition 2024
  --check` is clean over the crate, including the included generated files.
- The dstore help snapshots (`tests/golden/cli/snapshots.json` groups `help`, `unknown`, `usage`,
  `required`) are asserted by cli-app over the real command table.

## For other owners

- **cli-app:** give commands without an action `action: None` (the help action). Put `--log-level` in
  `AppDef.flags`. `AppDef.version` = `dstore_cli::VERSION`. `help`/`h` are appended by the framework; never
  define them.
- **dstore-cli (`main_entry`/`run`):** pass the whole argv. Map `CliError` as above.
- **`TestPackSizeFlag` port:** `dispatch(&app, argv, &mut Vec::new(), &|k| env.get(k).cloned())` returns
  `Some((_, ctx))`, then `pack_size(&ctx)`.

## Review

Reviewer: review-gocli. Files changed: `crates/gocli/src/app/tests.rs` (new tests), `crates/gocli/src/app/go_cases_review.rs`
(new, generated). No change to the library code: the review found no defect.

### What was checked against the Go source

- **Sources read line by line:** urfave/cli v2.27.7 `app.go`, `command.go`, `help.go`, `template.go`, `flag.go`,
  `flag_{bool,string,int,int64,uint,float64,duration,string_slice}.go`, `context.go`, `errors.go`, `args.go`,
  `category.go`, `parse.go`; go1.26.5 `flag.(*FlagSet).{Var,Set,Parse,parseOne}`, `numError` and the value types;
  `text/tabwriter` (`Write`, `format`, `writeLines`, `writePadding`, `flushNoDefers`, `reset`).
- **Run order** matches `App.RunContext` + `Command.Run`: `checkDuplicatedCmds` of the root, then per level `setup`,
  `checkDuplicatedCmds`, `Apply` of each flag, `Parse`, `normalizeFlags`, Incorrect Usage, `checkHelp`, `checkVersion`,
  `checkRequiredFlags`, dispatch, then the action or the help action with `HandleExitCoder`.
- **`helpCommand.Action`:** the arguments come from the context before the switch to the parent, as in Go. The root
  test (`parentContext.App == nil`) is `level == 0`. `len(Subcommands) == 1` selects the command template.
- **`ShowCommandHelp`:** Go picks `ctx.Command.Subcommands` when it is non-nil; Rust picks it when it is non-empty.
  This is equivalent, because every command on the path has been set up and so holds at least `help`.
- **Alias destinations:**
  - Go gives each name of a string or number flag its own destination, and `normalizeFlags` copies `Value.String()`
    to the others. Rust binds every name to one slot and marks the other names as set.
  - The two agree: `String()` re-parses to the same value, and giving two names is an error.
  - Slice flags share a destination in Go too. Go's `Serialize` round trip through JSON changes invalid UTF-8,
    but only for an aliased slice flag, which dstore does not have, and `string_slice` is lossy anyway.
- **Where Go panics** (deviation 2): Go's `flag.Var` also writes the message to stderr first, because urfave calls
  `SetOutput(io.Discard)` only after every `Apply`. A command-table defect; not reproduced.
- **tabwriter:** Go's `format` loop runs `this++` again after a column block. Rust does not, but the line it would
  skip has no cell in that column, so nothing changes. The committed table has the 53 rows of go1.26.5
  `tabwriter_test.go`.
- **Numeric probe** (`!` for bytes ≥ 0x80): `ParseBool`, `ParseInt`/`ParseUint` (`lower(c)`, digits, `_`) and
  `ParseFloat` (`special`, `readFloat`, `underscoreOK`) reject a non-ASCII byte at the same position and with the same
  class as `!`.
- **No `unwrap`/`expect`** in library code. Every byte index is guarded.

### Differential re-check (throwaway; scratchpad only, deleted)

- **Harness.** The Go mirror of `test_app` was rebuilt from `app/tests.rs` (urfave v2.27.7, each case in a fresh
  process).
- **The 108 committed cases** of `go_cases.rs`, re-run and compared with the committed expectations: 0 mismatches.
  So the new mirror equals the original harness.
- **115 new handpicked cases**, committed as `app/go_cases_review.rs` and asserted by `go_verified_review_cases`. They
  cover:
  - help topics through `-h`/`--help` (`refs -h help`, `cluster -h init`, and the truncated `cluster -h deep`);
  - nested parents, `help` twice (`refs help help`, `h h h`), `--` before a command, empty topics (`help ""`,
    `cluster ""`);
  - root flags given twice, empty and invalid environment values, first-found env var (`F64=` hides `F64B`);
  - `normalizeFlags` errors and aliases;
  - numbers at the limits (`0x_1f`, `0b1_0`, `08`, ±2⁶³, 2⁶⁴, `inf`, `NaN`, hex floats, `1_000`), durations at the
    limit, slice trimming and replacement of env values;
  - required-flag help with `h`/`help` topics.
- **4000 random argv/env cases** (seeded PCG; 90 tokens including non-UTF-8 bytes; 15 env vars with empty, invalid and
  valid values; all 53 probes on every action): 0 mismatches. 50 cases differ only in the lossy stderr text of
  deviation 1.

### Tests added

- **`go_verified_review_cases`:** the 115 cases above.
- **`urfave_subcommand_help_topic_and_command_not_found`:**
  - `Test_helpSubcommand_Action_ErrorIfNoTopic`, for a parent and its `help` subcommand;
  - `TestApp_CommandNotFound` without the hook: exit 3, no action runs.
- **`urfave_help_name_consistency`:** `TestHelpNameConsistency` with the default templates.
- **`urfave_help_with_empty_topic_prints_app_help`:** the `TestShowCommandHelp_HelpPrinter` row that the default
  printer can take (`help ""`).
- **`run_future_is_send`:** a compile-time lock that `run`'s future is `Send`.
- **Not portable** (the feature is absent from `AppDef`/`CommandDef`):
  - custom `HelpPrinter`s;
  - the `CommandNotFound` hook;
  - `TestApp_Run_SubcommandHelpName`'s custom `HelpName` and `Args`;
  - the `HideHelp` rows of `TestApp_Run_Help`.

### Notes

- **Spec inaccuracy.** cli.md §2.2.4 says `_` separators work "only with a prefix". Go's base-0
  `ParseInt`/`ParseUint` and `ParseFloat` accept `1_000` (`underscoreOK`): the Go-verified case
  `alias -i 1 -f64 1_000 -c 1_000` gives 1000 for both. The implementation follows Go.
- **Resolved from the original report.** `cargo check -p dstore-cli --all-targets --offline` now compiles
  against this API.
- **Gates.**
  - `cargo test -p dstore-gocli --offline`: 37 passed, 0 ignored.
  - `cargo clippy -p dstore-gocli --all-targets -- -D warnings`: clean.
  - `cargo fmt -p dstore-gocli --check`, and `rustfmt --edition 2024 --check` over the three include files: clean.
- **Still open.**
  - The dstore help snapshots (`tests/golden/cli/snapshots.json`) belong to cli-app.
  - Linux byte identity is left to CI.
