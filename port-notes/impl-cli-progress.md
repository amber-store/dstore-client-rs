# impl-cli-progress: `dstore_cli::progress` (layer L3)

Owner: cli-progress. Files: `crates/cli/src/progress.rs` (with its unit tests) and `tests/golden_tests/cli_progress.rs`.
No manifest, `tests/golden.rs` or vector file was changed, and no §4.12 signature was changed.

## Items added to PORTING.md §4.12 (additions only)

- `pub fn node_state(n: &NodeProgress) -> &'static str`: Go `nodeState`. It is read by the `node_state` vectors.
- `pub fn format_event(at: GoTime, level: Level, text: &str, zone: &dyn Zone) -> String`: Go `formatEvent`, read by the
  `format_event` vectors.
- `pub fn progress_bar(width: i64, percent: f64) -> String`: bubbles `ViewAs` of the bar that `newUIModel` builds. The
  `progress_bar` vectors use widths 0, 5, 6 and 7, which `UiModel` cannot reach (its bar is 20..80 wide).
- `impl TeaHandler { pub fn new(level: Level, send: UnboundedSender<UiMsg>) -> TeaHandler }`. §4.12 gives
  `TeaHandler` private fields and no constructor. `run_transfer` builds one internally; the golden tests need a
  public way to build one.
- `#[derive(Clone, Debug, PartialEq)]` on `UiMsg`.
- The private fields of `UiModel` are replaced by Go's: start, now, width, bar width, meters, report, rates,
  events, cancelling, done, err, plus the zone.

## Byte-identical parts

- **`status_line`, `fraction`, `RateMeter::add`** port `tui.go` line by line:
  - `t.Sub(u)` is signed and saturating, and `Duration.Seconds()` uses Go's `sec + nsec/1e9` form.
  - Go's builtin `min`/`max` over floats are ported exactly (NaN wins; +0 beats −0).
- **`UiModel::view`**:
  - Every styled piece goes through a port of lipgloss v2.0.6 `Style.Render` for a one-attribute style:
    - tabs become 4 spaces, then CRLF becomes LF;
    - each line is styled on its own (`\x1b[<sgr>m…\x1b[m`, even for an empty string);
    - a multi-line result pads its lines with spaces to the widest line (`alignTextHorizontal`, left).
  - Error texts that span several lines (errors.Join) are therefore rendered as Go renders them.
- **The bar** follows bubbles v2.2.1 `ViewAs`:
  - the text is `" %3.0f%%"` of the clamped percent. Rust `{:3.0}` rounds ties to even exactly as Go's `%f`
    does (checked: 12.5 → "12");
  - `tw = max(0, width − 5)` and `fw = clamp(round(tw·percent), 0, tw)`;
  - the blend is `Blend1D(tw·2)`; each filled cell is `38;2;fg;48;2;bg`, and the empty run is always emitted,
    even when it has length 0.
  - `math.Max`/`math.Min` semantics (±Inf, NaN, ±0) are kept for the percentage.
- **`blend1d`** is go-colorful v1.4.1 Lab math (linearize, XYZ, Lab D65, interpolate, inverse, delinearize,
  `Clamped`, `RGBA()`), plus x/ansi's channel shift: a 16-bit channel is shifted down by 8 only when it is > 0xff.
  - **FMA.** The gc compiler on arm64 fuses `x*y ± z` into FMADDD/FMSUBD/FNMSUBD (`ARM64.rules` 1772-1776, and
    `useFMA` is true without GOFMAHASH). The vectors come from arm64, so the port uses `mul_add` exactly where
    Go fuses:
    - the matrix rows, as `fma(c3, b, fma(c2, g, c1*r))`, with `a − x*y` as the first FSUBD rule;
    - `1.16*fy − 0.16` and `1.055*pow − 0.055`;
    - `l1 + t*(l2−l1)` and `v*65535 + 0.5`.
  - **Folded constants.** Go folds its constant expressions exactly, and each one is a single correctly rounded
    division in Rust: 216/24389, 108/841, 5/12 (`1.0/2.4`) and 4/29.
  - **`pow` and `cbrt`** are libm's `powf`/`cbrt`, not Go's pure-Go `math.Pow`/`math.Cbrt`. The probe below shows
    no 8-bit difference.
- **Differential probe** (a scratch Go program over lipgloss v2.0.6 and bubbles v2.2.1, run with
  `go -C tools/vectorgen run <scratch file>`; not committed, deleted afterwards). All of these are identical
  between Go and Rust:
  - 869 `Blend1D` results: the default stops over steps 0..400, and five other stop pairs over steps 3..80;
  - 1674 bars: widths −2..90 × 18 percents, including −0, −0.1, 1.5 and ties;
  - 65 lipgloss renders: 5 styles × 13 inputs with tabs, CR, CRLF, empty lines and embedded SGR.

  Every bar that `UiModel` can draw (bar widths 20..80, so blends of 30..150 steps) is covered.
- **`TeaHandler`** follows `attrValue`:
  - `bytes` with kind Int64 is humanised; other values use `Value.String()`;
  - the result is `%q`-quoted when it holds a space or a tab.
  - `Value::Bytes` is written as `%v` of a `[]byte` (`[104 105]`).

## Differences and approximations (none reached by the vectors)

- **`ansi.StringWidth`** is approximated: escape sequences are skipped, controls count 0, a table of combining and
  zero-width ranges counts 0, and East Asian wide and emoji ranges count 2. It only affects the padding of
  multi-line styled texts that contain such characters. The workspace has no Unicode width crate.
- **`Value::Time`** in a TUI event: Go writes `Time.String()`, and the port writes RFC3339Nano UTC. dstore logs no time
  attributes.
- **A record without a time** (Go's zero time) gets the year-1 instant. `Logger` always sets a time.
- **Float-to-int conversions** use Rust `as` (saturating; NaN → 0), which equals Go on arm64. Go on amd64
  (CVTTSD2SQ) gives MinInt64 for out-of-range values. This affects `int64(rate)`, `time.Duration(float64)` and
  `uint32(v*65535+0.5)`. The vector inputs never reach those values.
- **amd64 regeneration.** Go on amd64 (GOAMD64=v1) does not fuse FMA. If the CI `vectors` job regenerates on amd64
  and a blend channel lands on an 8-bit boundary, the difference is between Go builds, not a Rust defect.

## `run_transfer`

- **Mode.** Plain when `c.bool("no-tui") || !is_char_device(2)`. `DSTORE_NO_TUI` reaches the flag through its env
  binding, so `--no-tui=false` beats the env as in Go. The env var is never read directly.
- **Plain mode.**
  - A spawned tokio interval writes `status_line + "\n"` to stderr, once per write and flush. The first line comes
    after 5 s, and ticks run with `MissedTickBehavior::Skip`, like Go's dropping ticker.
  - The meter time is `Instant::now()` at the tick, like Go's tick value.
  - `f` gets `common::logger(c)`. After `f` returns, the ticker is stopped through a oneshot and joined, and a panic
    in it is re-raised.
- **TUI mode.**
  - `ctx.with_cancel()`. The transfer runs in a spawned task, as Go's goroutine does. The logger is
    `TeaHandler(log_level(c.string("log-level")))`, and the zone is `SystemZone`.
  - **Input.** Raw mode and a crossterm `EventStream` are used only when stdin `is_terminal()`. Bubble Tea puts only
    a tty input into raw mode; a non-tty character device such as `/dev/null` gives EOF and no keys.
  - **Ctrl+C.** A key event `Char('c')` with exactly CONTROL, and not a release, becomes `UiMsg::CtrlC`: cancel
    once and add the WARN event `cancelling`.
  - **Terminal size.** It is queried, and SIGWINCH is listened for, only when stderr `is_terminal()` (Bubble Tea
    `ttyOutput`).
    - The size comes from `crossterm::terminal::window_size()`, which asks `/dev/tty` (else stdout), not fd 2.
      PORTING §3.4 does not allow `unsafe` in dstore-cli, which rules out `ioctl(2, TIOCGWINSZ)` here.
      (`crossterm::terminal::size()` would also fall back to running `tput`.)
    - At start the model always gets a resize, as Bubble Tea's `Run` always sends a `WindowSizeMsg`: the
      terminal's width, or 0 when stderr is not a terminal (the bar then clamps to 20). See "Review".
    - A failed size query is ignored, whereas Go's `Run` (initial size) and `checkResize` (SIGWINCH) end the
      program with the error.
  - **Event order.** When the transfer task ends, the loop first drains the pending log events, then sends
    `Done`. The final frame then holds every event logged before the return, as Go's single channel guarantees.
  - **`failed: <err>`.** For `CliError::Msg` it is the text after `gocompat::errno::rewrite_os_errors`, which is
    what main prints after `dstore: `. For `CliError::Exit` it is `msg`.
  - **Run errors.** A raw-mode failure cancels, awaits the transfer and returns
    `CliError::Msg("error entering raw mode: <Go errno text>")`.
  - **A panicking transfer task** draws `failed: <JoinError text>`, restores the terminal, then re-raises the panic.
    Go's process would crash.
- **Renderer** (not a contract, DD-6):
  - The cursor is hidden at the start.
  - Each frame writes `\r`, `CSI n A` back to the first line of the previous frame, `CSI J`, then its lines, each
    followed by CR LF, because raw mode clears OPOST.
  - When the terminal reports a size, lines are cut to its width, ANSI-aware with a reset added, and a frame taller
    than the terminal keeps its last rows−1 lines. A 0 × 0 size neither trims nor cuts.
  - Identical frames are skipped. A frame is drawn after each message batch, tick, key and resize.
  - On quit the final frame stays on the terminal and the cursor is shown again. The `Screen` guard restores raw
    mode even on early returns.
- **Manual pty check** (macOS `script`, a temporary env-gated test, removed afterwards):
  - Raw mode was active, and Ctrl+C arrived as a key: it cancelled the transfer, which returned
    `context canceled at 38`.
  - The `cancelling` event and the `failed:` line were drawn, raw mode was off afterwards, and the cursor was hidden
    once and shown once.
  - Normal completion drew `done`.
  - The first run exposed the 0 × 0 trimming bug fixed above.

## Tests

Go tests ported:
- `TestRateMeter` → `rate_meter_go_test`.
- `TestStatusLine` → `fraction_go_test` and `status_line_go_test`.
- `TestUIModel` against `UiModel::view` → `ui_model_go_test`; the golden scenario `tui-test-go` checks it byte for
  byte.
- `TestTeaHandler` → `tea_handler_go_test`.

Unit tests (`crates/cli/src/progress.rs`), 23 in total, 5 of them ignored:
- **Running:**
  - the rate meter: the §5.4 table with sample counts, and backwards time;
  - `fraction`, `node_state`, `format_event` styles;
  - lipgloss render padding and tabs, `string_width`, the bar against the verified §3.6 frame, `blend1d`, `channel8`;
  - Go min/max semantics;
  - `UiModel` resize clamps, ctrl+c once, and the 12-event scroll;
  - `TeaHandler` values without `bytes`;
  - renderer frame bytes, truncation, and 0 × 0.
- **Ignored `needs dstore_client::progress (human_bytes)`:** `status_line_go_test`, `ui_model_go_test`,
  `tea_handler_go_test`, `run_plain_prints_status_lines_while_the_transfer_runs`,
  `run_tui_draws_events_and_the_done_frame`.

Golden tests (`tests/golden_tests/cli_progress.rs`, `cli/text.json`), 10 in total:
- **Running:** `fraction_cases`, `rate_meter_cases`, `node_state_cases`, `tea_handler_cases` (the cases without an
  Int64 `bytes` attribute), `format_event_cases`, `blend1d_cases`, `progress_bar_cases`.
- **Ignored `needs dstore_client::progress (human_bytes)`:** `status_line_cases`,
  `tea_handler_humanised_bytes_cases`, `ui_model_scenarios`.

**Verification while siblings were mid-edit:**
- The in-repo `cargo test -p dstore-cli` could not compile: first `crates/transport/src/mem.rs`, then
  `crates/worktree/src/sys.rs` and `diff.rs` failed.
- I used a scratch harness package named `dstore-cli`. It compiled `progress.rs` and the golden module through
  `#[path]` against the real `dstore-client`, `dstore-gocli`, `dstore-gocompat`, `dstore-view` and `dstore-testkit`,
  with the workspace `Cargo.lock`, and was deleted afterwards.
- With a stand-in `human_bytes` (Go's `HumanBytes`) shimmed over `dstore-client`, all 23 unit tests and all 10 golden
  tests pass. That includes all 10 `ui_model` scenarios, byte for byte.
- Against the real crates, the non-ignored tests pass.
- `cargo clippy --all-targets -- -D warnings` over the harness is clean. It covers `progress.rs` with its unit tests
  and the golden module. The two go-colorful matrices carry `#[allow(clippy::excessive_precision)]` so the literals
  stay verbatim.
- In-repo `cargo clippy -p dstore-cli --all-targets -- -D warnings` stopped earlier, on three `collapsible_if` errors
  in the sibling `dstore-gocli`, and reported nothing in `progress.rs` or `cli_progress.rs`.
- `rustfmt --check --edition 2024` is clean.

## Remaining

- Un-ignore the 8 tests above once `dstore_client::human_bytes` lands (client-a).
- `run_transfer`'s plain path calls `crate::common::logger(c)`, still a stub (cli-app/cli-admin owner).
- Nothing else: once the siblings compiled again, the in-repo runs passed. `cargo test -p dstore-cli --lib` gave 34
  passed, 5 ignored (the crate's other modules included). `cargo test -p dstore-client-rs --test golden --
  cli_progress` gave 7 passed, 3 ignored.
- Linux: the vectors are from macOS arm64. The progress texts contain no errno, but see the amd64 notes above.

## Review

Reviewer: review-cli-progress, owning the same two files. I checked the port against these sources:
- `tui.go` and `tui_test.go`;
- bubbles v2.2.1 `progress`;
- lipgloss v2.0.6 `Style.Render`, `alignTextHorizontal` and `Blend1D`;
- x/ansi v0.11.8 `Style.Styled` and `shift`, and go-colorful v1.4.1;
- bubbletea v2.0.9 `Program.Run`, `initInput`, `handleResize`, `checkResize` and `eventLoop`;
- ultraviolet `TerminalReader.StreamEvents` and `Key.Keystroke`.

### Fixed

1. **The initial resize was missing, so the model drew a different bar.**
   - Bubble Tea's `Run` always sends `WindowSizeMsg{p.width, p.height}` at start. When stderr is a character
     device but not a terminal (`2>/dev/null`, which `isTerminal` accepts), the size is 0 × 0. `Update` then
     sets the bar to `min(max(0−4, 20), 80)` = 20.
   - The port resized only when stderr was a terminal, so its model kept the 60-wide bar.
   - `Screen::initial_width` now gives 0 when stderr is not a terminal.
   - `run_tui_draws_events_and_the_done_frame` checks the 20-wide bar: 8 filled cells at 50 %.
2. **runTUI's `defer cancel()` was missing.**
   - The child ctx was cancelled only on the raw-mode error path. Anything the transfer left running on it
     outlived `run_transfer`.
   - `run_tui` now cancels it after the event loop.
3. **`crossterm::terminal::size()` could run `tput`.** It falls back to spawning `tput` when the ioctl fails.
   The port now uses `window_size()`, which asks the same fds and has no fallback.
4. **The vectors' rate-meter `samples` field was not asserted.**
   - The golden test cannot see it because the field is private.
   - The unit test `rate_meter_vector_sample_counts` now checks the `window-2s`, `sub-millisecond` and
     `tui-test-go` cases. `cli-md-5.4` was already covered.
5. **A timing race in a test.**
   - `run_plain_prints_status_lines_while_the_transfer_runs` slept a fixed 90 ms and expected a 20 ms tick
     in between. On a loaded machine, the stop could win against the first tick.
   - The transfer now waits, bounded at 10 s, until a line has been written.

### Verified, no change needed

- **`tui.go` functions**, line by line: `statusLine`, `fraction` (builtin `min`/`max` with ±0 and NaN),
  `rateMeter.add` (`Time.Sub`, `Duration.Seconds`), `uiModel` `Update`/`observe`/`View`, `nodeState`,
  `formatEvent`, and `teaHandler` with `attrValue`.
- **Styles.**
  - Each line of a styled text is written `SGR…\x1b[m`, even an empty line.
  - Tabs become 4 spaces, then CRLF becomes LF.
  - Multi-line text is padded with plain spaces, because `ansi.Style{}.Styled` returns its input unchanged.
- **The bar.**
  - `tw = max(0, width − StringWidth(percentView))`, and `fw` is computed from the unclamped percent.
  - The blend is `Blend1D(tw·2)` over the whole bar, and the empty run is always written.
  - `Blend1D` returns the stops themselves when steps ≤ 2.
- **Bubble Tea behaviour.**
  - Raw mode is used only for a terminal input. On a non-terminal input, EOF ends the read loop without an
    error.
  - SIGWINCH is watched only when the output is a terminal.
  - `eventLoop` does not handle ctrl+c itself.
  - A normal quit renders the final model.
  - `Keystroke()` is `ctrl+c` only when ctrl is the only modifier (alt, shift, meta, hyper and super add
    prefixes). `is_ctrl_c` matches exactly CONTROL, which is the same rule.
- **Test coverage.**
  - TestRateMeter, TestStatusLine, TestUIModel and TestTeaHandler are ported.
  - Every `cli/text.json` section of this module is read and asserted, with floats compared bit for bit.
  - A missing vector file fails the test.

### Remaining differences (recorded, not fixed)

- **Go's `Run` ends with an error in three cases the port handles differently.**
  - The cases: the initial `term.GetSize` fails (`bubbletea: error getting terminal size: …`), a SIGWINCH
    size query fails, or a read from a terminal input fails with anything but EOF.
  - Go then cancels the transfer and returns that error. The port ignores size errors and stops reading keys.
  - None of these is likely: the size ioctl on a terminal practically never fails, and a terminal read
    error comes with SIGHUP, whose default action kills both programs.
- **A non-terminal character device on stdin that yields bytes** (`< /dev/zero`): Go parses the bytes as keys.
  The port reads no keys.
- **Frame writes.** Frames are written from the event loop on a tokio worker, where Go writes from its own
  goroutine. A stalled terminal blocks that worker, not the transfer.
- **`ansi.StringWidth` stays approximated.** This matters only for multi-line styled texts; for example, a
  ZWJ emoji sequence counts 6 cells instead of 2.

### Tests (review)

- **Un-ignored.** The 8 tests marked `needs dstore_client::progress (human_bytes)` now run: client-a's
  `human_bytes` is implemented in the working tree. In the golden module, `tea_handler_cases` and
  `tea_handler_humanised_bytes_cases` are merged into one test over every case. It also asserts that some
  case humanises an Int64 `bytes` attribute.
- **In-repo** (flake dev shell, own target dir):
  - `cargo test -p dstore-cli --lib -- progress::`: 24 passed, 0 ignored.
  - `cargo test -p dstore-client-rs --test golden -- cli_progress`: 9 passed, 0 ignored.
  - `cargo clippy -p dstore-cli --all-targets -- -D warnings` stops before `dstore-cli`, on a sibling lint in
    `crates/client/src/progress.rs:177` (`sort_by_key`). Without `-D warnings`, `cargo clippy -p dstore-cli
    --all-targets` and `cargo clippy -p dstore-client-rs --test golden` report nothing in `progress.rs` or
    `cli_progress.rs`.
  - `rustfmt --check --edition 2024` on both files: clean.
- **Earlier in the review, in-repo builds were blocked by a sibling.** `crates/client/src/error.rs` did not
  compile (E0106 at line 98, and a lifetime error at line 111), so the first checks used a scratch harness.
- **Scratch harness** (deleted afterwards).
  - A package named `dstore-cli` compiled `progress.rs` and the golden module through `#[path]`. It built
    against the real `dstore-gocli`, `dstore-gocompat`, `dstore-view` and `dstore-testkit`.
  - A stand-in `dstore-client` provided `ProgressReport`, `NodeProgress`, `Progress` and a line-by-line port
    of Go `HumanBytes`.
  - `cargo test --lib -- --include-ignored`: 24 passed, in each of 3 runs.
  - `cargo test --test golden -- --include-ignored`: 10 passed.
  - `cargo clippy --all-targets -- -D warnings`: clean.
  - `rustfmt --check --edition 2024` on both files: clean.
- **Still ignored** with `needs dstore_client::progress (human_bytes)`, because client-a has not landed and
  `human_bytes` is still `todo!()`:
  - 5 unit tests: `status_line_go_test`, `ui_model_go_test`, `tea_handler_go_test`,
    `run_plain_prints_status_lines_while_the_transfer_runs`, `run_tui_draws_events_and_the_done_frame`;
  - 3 golden tests: `status_line_cases`, `tea_handler_humanised_bytes_cases`, `ui_model_scenarios`.
- **Unit test count.** `progress.rs` now has 24 unit tests; the "Tests" section above predates the review and
  says 23.
