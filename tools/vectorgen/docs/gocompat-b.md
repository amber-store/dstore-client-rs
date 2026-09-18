# Family `gocompat-numtime`: strconv numbers, durations and time layouts

Owner: gocompat-b. Generator: `tools/vectorgen/family_gocompat_numtime.go`, registered as `gocompat-numtime`,
owning `gocompat/numtime.json`. Regenerate with `go run . ../../tests/golden gocompat-numtime` from
`tools/vectorgen`.

The values come from go1.26.5 stdlib calls: `strconv.ParseBool/ParseInt/ParseUint/ParseFloat(s, 64)`,
`strconv.FormatFloat(f, 'g', -1, 64)`, `time.Duration.String/Round`, `time.ParseDuration`,
`Time.In(time.FixedZone("", off)).Format(time.RFC3339)`, `Time.UTC().Format(time.RFC3339Nano)`,
`Format("15:04:05")`, `time.Parse(time.RFC3339Nano, s)`, and a real `slog.NewTextHandler`, whose `time=`
value is cut out of the record line. The log-package header is unexported, so the generator carries a copy of
`log.itoa` and the `Ldate|Ltime` part of `log.formatHeader`. Before generating, `ntCheckLogCopy` compares
that copy with `$GOROOT/src/log/log.go`, whitespace-insensitively. Random cases use splitmix64 streams
(`util.go` `splitmixNext`) with fixed seeds, so two runs give identical bytes.

Every input, error text and layout output is valid UTF-8, because the Rust APIs take `&str` and return
`String`. The generator panics otherwise, so `json.Marshal` never replaces a byte with U+FFFD. (`log.itoa`
writes a non-UTF-8 first byte for years at or below −49000; no vector reaches them.) Error texts are Go's
`err.Error()`, and `""` means no error.

## Schema

One JSON object; each key holds an array of cases in generation order.

| Key | Case fields | Go call |
|---|---|---|
| `parse_bool` | `in`, `value` (bool), `err` | `strconv.ParseBool(in)` |
| `parse_int` | `in`, `base` (0, 10 or 16), `bit_size` (8/16/32/64), `value` (decimal string; Go's returned value, also on range errors, which the Rust API does not return), `err` | `strconv.ParseInt(in, base, bit_size)` |
| `parse_uint` | as `parse_int`, `value` as u64 | `strconv.ParseUint(in, base, bit_size)` |
| `parse_float` | `in`, `bits` (decimal u64 of `math.Float64bits` of Go's result, ±Inf on range errors, 0 on syntax errors), `err` | `strconv.ParseFloat(in, 64)` |
| `format_float_g` | `bits` (decimal u64), `out` | `strconv.FormatFloat(math.Float64frombits(bits), 'g', -1, 64)` |
| `duration_string` | `ns` (decimal i64), `out` | `time.Duration(ns).String()` |
| `duration_round` | `ns`, `m`, `out` (decimal i64) | `time.Duration(ns).Round(time.Duration(m))` |
| `parse_duration` | `in`, `ns` (decimal i64), `err` | `time.ParseDuration(in)` |
| `rfc3339_local` | `unix_secs` (decimal i64), `nanos`, `offset_secs`, `out` | `time.Unix(unix_secs, nanos).In(time.FixedZone("", offset_secs)).Format(time.RFC3339)` |
| `rfc3339nano_utc` | `unix_secs`, `nanos`, `out` | `time.Unix(unix_secs, nanos).UTC().Format(time.RFC3339Nano)` |
| `slog_time` | as `rfc3339_local` | the `time=` value of `slog.NewTextHandler(w, nil).Handle(ctx, slog.NewRecord(t, INFO, "m", 0))` for `t` in that zone; zero times (no `time=`) are left out |
| `clock` | as `rfc3339_local` | `t.Format("15:04:05")` |
| `log_std` | as `rfc3339_local` | `log.formatHeader` with `LstdFlags` for `t` in that zone, without the trailing space |
| `parse_rfc3339nano` | `in`, `unix_secs`, `nanos` (0 on error), `err` | `time.Parse(time.RFC3339Nano, in)`; the instant is `t.Unix()`, `t.Nanosecond()` |

Case selection:

- **Integers:** Go's `internal/strconv/atoi_test.go` inputs and extra edge strings, in bases 0/10/16 with bit
  size 64. Each bit-size boundary (max, max+1, uint max, ±, decimal, raw hex, `0x`, octal and binary forms)
  is parsed in bases suited to its form, in every bit size. There are also 120 random values with signs and
  underscores.
- **Floats (parse):**
  - Go's `atof_test.go` `atoftests` inputs, including the 4000- and 10000-digit ones.
  - Specials, and Go's quirks: exponent capping at five digits, and the 800-digit slow path that sets the
    decimal point from the stored digits (`1…e-904` parses to `1.0000000000000002e-105`).
  - 300 values formatted with `'g'`, `'e'` and `'x'`.
  - 300 random decimal strings with exponents in ±350.
- **Floats (format):** specials (±0, NaN patterns, ±Inf, denormal and normal edges), the `ftoa_test.go` values,
  powers of two and ten, and 200 exact ties (`m/4` for odd `m`, where Go's Dragonbox rounds to even). Also
  1500 random bit patterns and 500 random values near the `%e`/`%f` switch.
- **Durations:**
  - Edge values and their negations: 0, ±1, unit boundaries, `math.MinInt64`/`MaxInt64`.
  - Go's `durationRoundTests`, edges × 13 multiples including 0, negative and `MaxInt64`, and random pairs.
- **ParseDuration:** Go's `parseDurationTests` and the valid-UTF-8 `parseDurationErrorTests`, extra syntax
  cases, and 200 random `Duration.String` outputs, a third of them mutated.
- **Layouts:** 26 fixed instants × fixed offsets, and 150 random instants in years 0..9999 with random
  second-precision offsets. The fixed instants include the epoch, pre-1970, leap days, years 0, 1, −1,
  −10000, 9999, 10000 and 123456, the `UnixNano` range edges, and `math.MaxInt64`/`MinInt64` seconds.
  Offsets are 0, ±1, ±59, ±60, ±61, ±3600, +19800, −12600, +50400, −43200, ±86399, ±90000 and +360000;
  `slog_time`, `clock` and `log_std` use a subset.
- **Parse(RFC3339Nano):** the RFC3339 cases of Go's `format_test.go` `parseErrorTests` and `rfc3339Formats`,
  plus accept/reject cases (comma fraction, 1-digit hour, `+24:60`, range messages, extra text, non-ASCII
  input). Also 150 random RFC3339Nano strings in random minute offsets, half of them with one byte mutated.

## Rust tests

`tests/golden_tests/gocompat_numtime.rs` (module `gocompat_numtime` of `tests/golden.rs`) loads the file once
through `dstore_testkit::golden::load_json`, so a missing file fails. For each section it collects every
mismatch and fails listing the first 25.

- `parse_bool_values`, `parse_int_values`, `parse_uint_values`, `parse_float_values`: on success the value
  (float `to_bits`) must equal Go's. On failure `NumError.func`, `num` and `kind` must match the Go text,
  which is not rendered here.
- `parse_number_error_texts`: `NumError` `Display` equals Go's text for every error. It depends on
  `dstore_gocompat::quote`.
- `format_float_g`, `duration_string`, `duration_round`: exact equality.
- `parse_duration`, `parse_rfc3339nano`: the value and nanoseconds, or the exact error text.
- `rfc3339_local`, `slog_time`, `clock`, `log_std`: `format_*(GoTime { unix_secs, nanos }, &FixedZone(offset_secs))`
  equals `out`. `rfc3339nano_utc` checks `format_rfc3339nano_utc`.

Unit tests in `crates/gocompat/src/strconv.rs` and `time.rs` port Go's test tables: `atob_test.go`,
`atoi_test.go`, `atof_test.go` `atoftests`, the `ftoa_test.go` `'g'` shortest cases with the powers-of-two
round trip, `time_test.go` `durationTests`, `durationRoundTests`, `parseDurationTests` and
`parseDurationErrorTests`, and `format_test.go` `rfc3339Formats`, `TestAppendInt` and the RFC3339
`parseErrorTests`.
