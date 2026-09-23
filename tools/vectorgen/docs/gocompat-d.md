# gocompat-d: `gocompat/slog.json`

Family `gocompat-slog`, in `tools/vectorgen/family_gocompat_slog.go`. It pins the `log/slog` behaviour that
`dstore_gocompat::slog` reproduces (PORTING.md §4.1; port-notes/client-core.md §2.13, §3.5;
port-notes/cli.md §2.3).

```sh
nix develop -c go -C tools/vectorgen run . ../../tests/golden gocompat-slog
```

## Producing Go code

- **Text handler lines.** go1.26.5 `log/slog`: `slog.New(slog.NewTextHandler(w, nil))`, one `Logger.With` per
  `with` entry, then `slog.NewRecord(t, level, msg, 0)` with `Record.Add(args...)`. `Handler().Handle` is
  called directly, so the record time is fixed. Times are `t.In(time.FixedZone("", offset_secs))`. The
  generator checks that `Handle` writes exactly once.
- **Attribute descriptions.**
  - Built from the `slog.Attr`s that `Record.Add` produced, so each kind is the one Go chose (`int` becomes
    `int64`, `uint8` becomes `uint64`, and `error`, `[]string` and structs become `any`).
  - `With` arguments are described through a scratch `Record.Add`, which converts arguments like
    `Logger.With` does (`argsToAttr`).
- **`slog.Default()` lines.**
  - The generator sets the standard `log.Logger` to flags 0, handles the record with `slog.Default()`'s
    handler, and records what that handler passes on. It then prefixes the `LstdFlags` header for `now`, via
    `now.In(zone).Format("2006/01/02 15:04:05 ")`. `formatHeader`'s itoa layout gives the same text for
    years 1000 to 9999.
  - Before generating, a self-check handles a record with the real `LstdFlags` at the current local time and
    requires the header of the time before or after the call, followed by the flags-0 text.
- **`log_levels`.** `slogvDstoreLogLevel` is a verbatim copy of cmd/dstore `logLevel` (dstore v0.1.10
  `cmd/dstore/main.go`). It runs on a urfave `cli.Context` whose flag set holds `--log-level`. Before
  generating, the copy's signature and body are printed with `go/printer` and compared with the function
  in the module source (`go list -m -f {{.Dir}} github.com/amber-store/dstore`).
- **`needs_quoting`.** `needsQuoting` is unexported, so it is observed through `TextHandler`: the value of
  `slog.String("k", s)` is written either verbatim (false) or as `strconv.Quote(s)` (true). Any other
  output fails the generator.
- **`levels`.** `slog.Level(n).String()`.

## Schema

The file is one object. Conventions are those of VECTORS.md: `unix_secs`, `num` and float bits are decimal
strings.

| Field | Content |
|---|---|
| `levels` | `[{level: i32, text}]`: `Level.String` for −20…20, ±1000 and the `int32` ends |
| `log_levels` | `[{in, level}]`: cmd/dstore `logLevel(--log-level in)` |
| `needs_quoting` | `[{in_hex, valid_utf8, needs_quoting}]`: the input as hex (it may be invalid UTF-8), then `needsQuoting` |
| `records` | `[{name, offset_secs, time, level, msg, with, attrs, line}]` |
| `default_logger` | `[{name, offset_secs, now, level, msg, with, attrs, line}]` |

- `time` / `now`: `{unix_secs, nanos}` = `t.Unix()`, `t.Nanosecond()` (floor seconds). This is
  `dstore_gocompat::time::GoTime`. A record `time` of `null` is Go's zero time, which the handler omits
  (`Record.time = None`).
- `offset_secs`: the zone of the record time and of every `time` attribute (`FixedZone(offset_secs)`).
- `with`: the attribute lists of successive `Logger.With` calls. `attrs`: the record's attributes.
- `line`: the exact bytes written, including the final "\n". Every line is valid UTF-8, because inputs are
  valid UTF-8 and `strconv.Quote` escapes invalid bytes.
- An attribute is `{key, kind, str?, num?, bool?, time?, hex?, go_type?}`, with exactly one value field:

  | `kind` | Value field | Rust `slog::Value` |
  |---|---|---|
  | `string` | `str` | `String` |
  | `int64` | `num` (decimal) | `Int64` |
  | `uint64` | `num` (decimal) | `Uint64` |
  | `float64` | `num`: decimal `math.Float64bits` (NaN and infinities stay exact) | `Float64(f64::from_bits)` |
  | `bool` | `bool` | `Bool` |
  | `duration` | `num`: nanoseconds | `Duration` |
  | `time` | `time` | `Time` |
  | `bytes` | `hex` (a `[]byte` value, nil and empty alike) | `Bytes` |
  | `any` | `str` = `fmt.Sprintf("%+v", v)`; `go_type` = `%T` (informational) | `Any` |

Coverage:

- **Records.**
  - The client log calls of client-core §2.13 and the verified lines of client-core §3.5 and cli.md §2.3.
  - Go `text_handler_test.go`'s `TestTextHandler` (without `TextMarshaler` values) and
    `TestTextHandlerPreformatted`.
  - Nested `With`, and more attributes than a `Record` keeps inline.
  - Keys and messages that need quoting, levels between the named ones, every value kind, and float edge
    cases.
  - Durations up to the `int64` ends, `time` attributes, byte slices (including invalid UTF-8 and all 256
    bytes), errors, `nil`, slices and structs as `any`, Unicode classes, and `!BADKEY`.
  - Record times across epochs and zones: offsets with seconds, below a minute, and +14:00.
- **Default logger.** The unquoted message, the trailing-newline rule, empty messages, custom levels, `With`,
  quoting in attributes, a `time` attribute, and headers in other zones and before the epoch.

## Rust tests

`tests/golden_tests/gocompat_slog.rs` (root `tests/golden.rs`):

- `level_display`: `Level(level).to_string() == text`.
- `log_level`: `slog::log_level(in) == Level(level)`.
- `needs_quoting`:
  - Valid UTF-8: `slog::needs_quoting(s) == needs_quoting`.
  - Invalid UTF-8: Go says true, and so does `needs_quoting` of the lossy string a Rust caller would hold.
    Rust strings cannot carry invalid UTF-8 (PORTING.md DD-8).
- `text_handler_records`:
  - `slog::format_text_record(flattened with, record, FixedZone(offset_secs)) == line`.
  - A `TextHandler` writes the same bytes in a single write.
- `logger_with_matches_go_with`: `Logger::with` once per Go `With` call, then `Logger::log`. The handler
  gets the flattened attributes, which format to `line` once the record time is replaced with the vector's.
- `default_logger_lines`: `slog::format_default_record(flattened with, record, now, FixedZone(offset_secs)) == line`.

Not modelled, by design:

- Invalid UTF-8 in keys, messages and string values. They are Rust `String`s, so invalid UTF-8 only appears
  in `bytes` values and in the `needs_quoting` lossy check.
- `encoding.TextMarshaler` values, groups, `LogValuer`, `ReplaceAttr` and `AddSource`. dstore uses none of
  them.
- The elision of the zero `Attr{}`, which a rendered value cannot express.
