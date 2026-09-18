// Family gocompat-slog writes tests/golden/gocompat/slog.json: the log/slog
// behaviour that dstore-gocompat's slog module reproduces (PORTING.md §4.1;
// port-notes/client-core.md §2.13, §3.5; port-notes/cli.md §2.3). The schema is
// documented in docs/gocompat-d.md.
//
// Every expected text comes from the Go standard library (go1.26.5) or from a
// verbatim copy of cmd/dstore code that is checked against the dstore v0.1.9
// module source before generating.
package main

import (
	"bytes"
	"context"
	"encoding"
	"encoding/hex"
	"errors"
	"flag"
	"fmt"
	"go/ast"
	"go/parser"
	"go/printer"
	"go/token"
	"io"
	"log"
	"log/slog"
	"math"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/urfave/cli/v2"
)

func init() {
	register("gocompat-slog", []string{"gocompat/slog.json"}, genGocompatSlog)
}

// slogvFile is gocompat/slog.json.
type slogvFile struct {
	Levels        []slogvLevel    `json:"levels"`
	LogLevels     []slogvLogLevel `json:"log_levels"`
	NeedsQuoting  []slogvQuoting  `json:"needs_quoting"`
	Records       []slogvRecord   `json:"records"`
	DefaultLogger []slogvDefault  `json:"default_logger"`
}

// slogvLevel is one slog.Level.String case.
type slogvLevel struct {
	Level int32  `json:"level"`
	Text  string `json:"text"`
}

// slogvLogLevel is one cmd/dstore logLevel case: the --log-level value and the
// resulting level.
type slogvLogLevel struct {
	In    string `json:"in"`
	Level int32  `json:"level"`
}

// slogvQuoting is one needsQuoting case. The input is hex because it may be
// invalid UTF-8.
type slogvQuoting struct {
	InHex        string `json:"in_hex"`
	ValidUTF8    bool   `json:"valid_utf8"`
	NeedsQuoting bool   `json:"needs_quoting"`
}

// slogvTime is an instant as t.Unix() and t.Nanosecond() (floor seconds, so
// the nanoseconds are never negative).
type slogvTime struct {
	UnixSecs I64    `json:"unix_secs"`
	Nanos    uint32 `json:"nanos"`
}

// slogvAttr is one attribute, described the way the Rust side builds its
// slog::Attr. Exactly one value field is set, chosen by Kind.
type slogvAttr struct {
	Key  string `json:"key"`
	Kind string `json:"kind"` // string int64 uint64 float64 bool duration time bytes any
	// Str is the string (kind string) or fmt.Sprintf("%+v", v) (kind any).
	Str *string `json:"str,omitempty"`
	// Num is a decimal integer: int64, uint64, duration (nanoseconds), and for
	// float64 the math.Float64bits of the value.
	Num  *string    `json:"num,omitempty"`
	Bool *bool      `json:"bool,omitempty"`
	Time *slogvTime `json:"time,omitempty"`
	// Hex is the byte slice (kind bytes).
	Hex *string `json:"hex,omitempty"`
	// GoType is the %T of an any value, informational.
	GoType string `json:"go_type,omitempty"`
}

// slogvRecord is one slog.TextHandler.Handle call and the bytes of its single
// Write.
type slogvRecord struct {
	Name       string        `json:"name"`
	OffsetSecs int           `json:"offset_secs"`
	Time       *slogvTime    `json:"time"` // null: the zero time
	Level      int32         `json:"level"`
	Msg        string        `json:"msg"`
	With       [][]slogvAttr `json:"with"` // Logger.With calls, in order
	Attrs      []slogvAttr   `json:"attrs"`
	Line       string        `json:"line"`
}

// slogvDefault is one record through slog.Default()'s handler and the standard
// log.Logger (LstdFlags), with the log header rendered for Now.
type slogvDefault struct {
	Name       string        `json:"name"`
	OffsetSecs int           `json:"offset_secs"`
	Now        slogvTime     `json:"now"`
	Level      int32         `json:"level"`
	Msg        string        `json:"msg"`
	With       [][]slogvAttr `json:"with"`
	Attrs      []slogvAttr   `json:"attrs"`
	Line       string        `json:"line"`
}

// slogvCase is a record to handle: Logger.With argument lists, then
// Record.Add arguments, as dstore's c.log.Info("msg", "key", value, ...) calls
// pass them.
type slogvCase struct {
	name   string
	offset int       // zone offset, seconds east of UTC
	t      time.Time // record time (zero: none); default_logger: the log header time
	level  slog.Level
	msg    string
	with   [][]any
	args   []any
}

// slogvWriter records every Write call.
type slogvWriter struct{ calls [][]byte }

func (w *slogvWriter) Write(p []byte) (int, error) {
	w.calls = append(w.calls, append([]byte(nil), p...))
	return len(p), nil
}

// one returns the only Write.
func (w *slogvWriter) one() (string, error) {
	if len(w.calls) != 1 {
		return "", fmt.Errorf("handler wrote %d times, want once", len(w.calls))
	}
	return string(w.calls[0]), nil
}

// slogvName has a String method (Go text_handler_test.go "String method").
type slogvName struct {
	First, Last string
}

func (n slogvName) String() string { return n.Last + ", " + n.First }

func genGocompatSlog(out string) error {
	if err := slogvCheckLogLevelCopy(); err != nil {
		return err
	}
	if err := slogvCheckLogHeader(); err != nil {
		return err
	}
	f := slogvFile{
		Levels:        slogvLevels(),
		Records:       []slogvRecord{},
		DefaultLogger: []slogvDefault{},
	}
	var err error
	if f.LogLevels, err = slogvLogLevels(); err != nil {
		return err
	}
	if f.NeedsQuoting, err = slogvQuotingCases(); err != nil {
		return err
	}
	for _, c := range slogvRecordCases() {
		rec, err := slogvRunRecord(c)
		if err != nil {
			return fmt.Errorf("record %q: %w", c.name, err)
		}
		f.Records = append(f.Records, rec)
	}
	for _, c := range slogvDefaultCases() {
		rec, err := slogvRunDefault(c)
		if err != nil {
			return fmt.Errorf("default logger %q: %w", c.name, err)
		}
		f.DefaultLogger = append(f.DefaultLogger, rec)
	}
	return writeJSON(filepath.Join(out, "gocompat", "slog.json"), f)
}

// ---- levels ----

func slogvLevels() []slogvLevel {
	var ns []int
	for n := -20; n <= 20; n++ {
		ns = append(ns, n)
	}
	ns = append(ns, -1000, 1000, math.MinInt32, math.MinInt32+4, math.MaxInt32-8, math.MaxInt32)
	out := make([]slogvLevel, 0, len(ns))
	for _, n := range ns {
		out = append(out, slogvLevel{Level: int32(n), Text: slog.Level(n).String()})
	}
	return out
}

// slogvDstoreLogLevel is a verbatim copy of cmd/dstore logLevel (dstore v0.1.9
// cmd/dstore/main.go), checked by slogvCheckLogLevelCopy. The name differs so
// that it cannot collide with other copies in package main.
func slogvDstoreLogLevel(c *cli.Context) slog.Level {
	switch strings.ToLower(c.String("log-level")) {
	case "debug":
		return slog.LevelDebug
	case "warn":
		return slog.LevelWarn
	case "error":
		return slog.LevelError
	}
	return slog.LevelInfo
}

func slogvLogLevels() ([]slogvLogLevel, error) {
	ins := []string{
		"", "debug", "DEBUG", "Debug", "dEbUg", "info", "INFO", "Info", "warn", "WARN", "Warn",
		"warning", "WARNING", "error", "ERROR", "Error", "err", "verbose", "trace", "fatal",
		"debug ", " debug", "\tdebug", "4", "-4", "0", "8",
		"\u0130nfo", "\u212aebug", "DEBUG\u0130", "\uff44\uff45\uff42\uff55\uff47", "ERRO\u0280",
	}
	out := make([]slogvLogLevel, 0, len(ins))
	for _, in := range ins {
		set := flag.NewFlagSet("dstore", flag.ContinueOnError)
		set.String("log-level", "info", "")
		if err := set.Set("log-level", in); err != nil {
			return nil, err
		}
		lvl := slogvDstoreLogLevel(cli.NewContext(cli.NewApp(), set, nil))
		out = append(out, slogvLogLevel{In: in, Level: int32(lvl)})
	}
	return out, nil
}

// slogvCheckLogLevelCopy compares slogvDstoreLogLevel with cmd/dstore logLevel
// in the dstore module source.
func slogvCheckLogLevelCopy() error {
	self := "family_gocompat_slog.go"
	if _, file, _, ok := runtime.Caller(0); ok {
		if _, err := os.Stat(file); err == nil {
			self = file
		}
	}
	cmd := exec.Command("go", "list", "-m", "-f", "{{.Dir}}", "github.com/amber-store/dstore")
	cmd.Dir = filepath.Dir(self)
	cmd.Stderr = os.Stderr
	dir, err := cmd.Output()
	if err != nil {
		return fmt.Errorf("locating the dstore module: %w", err)
	}
	upstream := filepath.Join(strings.TrimSpace(string(dir)), "cmd", "dstore", "main.go")
	want, err := slogvFuncText(upstream, "logLevel")
	if err != nil {
		return err
	}
	got, err := slogvFuncText(self, "slogvDstoreLogLevel")
	if err != nil {
		return err
	}
	if got != want {
		return fmt.Errorf("slogvDstoreLogLevel is not a copy of cmd/dstore logLevel:\n%s\nwant:\n%s", got, want)
	}
	return nil
}

// slogvFuncText prints the signature and body of the top-level function name
// in file, without comments.
func slogvFuncText(file, name string) (string, error) {
	fset := token.NewFileSet()
	f, err := parser.ParseFile(fset, file, nil, 0)
	if err != nil {
		return "", err
	}
	for _, d := range f.Decls {
		fn, ok := d.(*ast.FuncDecl)
		if !ok || fn.Recv != nil || fn.Name.Name != name {
			continue
		}
		var b bytes.Buffer
		if err := printer.Fprint(&b, fset, fn.Type); err != nil {
			return "", err
		}
		b.WriteByte(' ')
		if err := printer.Fprint(&b, fset, fn.Body); err != nil {
			return "", err
		}
		return b.String(), nil
	}
	return "", fmt.Errorf("%s: no function %s", file, name)
}

// ---- needsQuoting ----

func slogvQuotingCases() ([]slogvQuoting, error) {
	ins := []string{
		"", "a", "ab", "a=b", "=", `"ab"`, `a"b`, "\a\b", "a\tb", "a\nb", "\x00", "\x1f", " ", "a b",
		`\`, `a\b`, "\x7f", "a\x7fb", "~!#$%&'()*+,-./:;<>?@[]^_`{|}", "trees/**", "1a2b3c4d",
		"remote: unavailable: no view", "1.0 MiB/s", "µåπ", "é", "日本語", "😀", "e\u0301",
		"\u00a0", "a\u00a0b", "\u0085", "\u1680", "\u2000", "\u2028", "\u2029", "\u202f", "\u205f",
		"\u3000", "\u200b", "\u00ad", "\u0378", "\ue000", "\U000e0001", "\U0010ffff", "\ufeff",
		"\ufffd", "a\ufffdb", "badutf8\xf6", "\xff", "a\xffb", "\xc0\x80", "\xed\xa0\x80",
		"\xf4\x90\x80\x80", "\xe2\x82",
	}
	for b := 0; b < utf8.RuneSelf; b++ {
		ins = append(ins, string([]byte{'a', byte(b), 'b'}))
	}
	out := make([]slogvQuoting, 0, len(ins))
	for _, s := range ins {
		q, err := slogvNeedsQuoting(s)
		if err != nil {
			return nil, err
		}
		out = append(out, slogvQuoting{
			InHex:        hex.EncodeToString([]byte(s)),
			ValidUTF8:    utf8.ValidString(s),
			NeedsQuoting: q,
		})
	}
	return out, nil
}

// slogvNeedsQuoting observes the unexported needsQuoting through TextHandler:
// a string value is written verbatim or as strconv.Quote.
func slogvNeedsQuoting(s string) (bool, error) {
	var w slogvWriter
	h := slog.NewTextHandler(&w, nil)
	r := slog.NewRecord(time.Time{}, slog.LevelInfo, "m", 0)
	r.AddAttrs(slog.String("k", s))
	if err := h.Handle(context.Background(), r); err != nil {
		return false, err
	}
	line, err := w.one()
	if err != nil {
		return false, err
	}
	const prefix = "level=INFO msg=m k="
	if !strings.HasPrefix(line, prefix) || !strings.HasSuffix(line, "\n") {
		return false, fmt.Errorf("needsQuoting(%q): unexpected line %q", s, line)
	}
	switch v := line[len(prefix) : len(line)-1]; v {
	case s:
		return false, nil
	case strconv.Quote(s):
		return true, nil
	default:
		return false, fmt.Errorf("needsQuoting(%q): value %q is neither verbatim nor quoted", s, v)
	}
}

// ---- records ----

func slogvRecordCases() []slogvCase {
	base := time.Date(2026, time.September, 18, 10, 11, 12, 345678901, time.UTC)
	t2000 := time.Date(2000, time.January, 2, 3, 4, 5, 0, time.UTC) // slog's testTime
	cliT := time.Date(2026, time.September, 18, 10, 34, 56, 789654321, time.UTC)
	const ms = time.Millisecond
	info, warn, errl, debug := slog.LevelInfo, slog.LevelWarn, slog.LevelError, slog.LevelDebug

	var cases []slogvCase
	add := func(c slogvCase) { cases = append(cases, c) }

	// port-notes/client-core.md §2.13 and §3.5.
	add(slogvCase{name: "connected direct", t: base, level: info, msg: "connected",
		args: []any{"node", "1a2b3c4d", "nodes", 3, "path", "direct", "rtt", 2 * ms}})
	add(slogvCase{name: "connected none +02:00", offset: 7200, t: base, level: info, msg: "connected",
		args: []any{"node", "1a2b3c4d", "nodes", 3, "path", "none"}})
	add(slogvCase{name: "connected relay -05:30", offset: -19800, t: base, level: info, msg: "connected",
		args: []any{"node", "1a2b3c4d", "nodes", 1, "path", "relay", "rtt", time.Duration(0)}})
	add(slogvCase{name: "watch retrying", t: base, level: warn, msg: "watch: no node answered, retrying",
		args: []any{"pattern", "trees/**", "in", time.Second}})
	add(slogvCase{name: "watch idle", t: base, level: warn, msg: "watch: stream idle, reconnecting",
		args: []any{"node", "1a2b3c4d"}})
	add(slogvCase{name: "watch ended", t: base, level: warn, msg: "watch: stream ended, reconnecting",
		args: []any{"node", "1a2b3c4d", "error", io.EOF}})
	add(slogvCase{name: "watch refused", t: base, level: warn, msg: "watch: node refused, trying the next",
		args: []any{"node", "1a2b3c4d", "error", errors.New("remote: unavailable: no view")}})
	add(slogvCase{name: "watch synced", t: base, level: info, msg: "watch: synced",
		args: []any{"pattern", "trees/**", "node", "1a2b3c4d", "refs", 2}})
	add(slogvCase{name: "quoting", t: base, level: info, msg: "quoting", args: []any{
		"empty", "", "space", "a b", "eq", "a=b", "quote", `a"b`, "backslash", `a\b`, "tab", "a\tb",
		"nl", "a\nb", "unicode", "é", "nbsp", "a\u00a0b", "ctrl", "a\x01b", "del", "a\x7fb",
		"invalid", []byte("a\xffb"), "zwsp", "a\u200bb", "u64", uint64(5), "i64", int64(-5),
		"f", 0.5, "b", true, "bytes", []byte("hi"),
	}})
	add(slogvCase{name: "truncate ms", t: base.Add(999 * time.Microsecond), level: errl, msg: "truncate ms"})
	add(slogvCase{name: "zero time", level: debug, msg: "zero time"})
	add(slogvCase{name: "warn plus two", t: base, level: warn + 2, msg: "warn plus two"})
	add(slogvCase{name: "uploaded", t: base, level: info, msg: "uploaded", args: []any{
		"node", "1a2b3c4d", "objects", 12, "bytes", int64(3 << 20), "took", 1235 * ms, "rate", "2.4 MiB/s",
	}})
	add(slogvCase{name: "reference written", t: base, level: info, msg: "reference written",
		args: []any{"name", "trees/main", "version", "0102abcd"}})
	add(slogvCase{name: "mdns unavailable", t: base, level: warn, msg: "transport: mdns discovery unavailable",
		args: []any{"error", fmt.Errorf("listen udp4 224.0.0.251:5353: %w", errors.New("bind: address already in use"))}})

	// port-notes/cli.md §2.3.
	add(slogvCase{name: "cli connected", offset: 7200, t: cliT, level: info, msg: "connected",
		args: []any{"node", "abcd0123", "nodes", 3, "path", "direct", "rtt", 3 * ms}})
	add(slogvCase{name: "cli upload failed", t: time.Date(2026, time.September, 18, 10, 34, 56, 5000000, time.UTC),
		level: warn, msg: "upload failed",
		args: []any{"node", "abcd0123", "objects", 12, "err", errors.New("timeout: no recent network activity")}})
	add(slogvCase{name: "cli kinds", offset: 7200, t: cliT, level: debug, msg: "x", args: []any{
		"empty", "", "eq", "a=b", "quote", `say "hi"`, "uni", "é…", "tab", "a\tb", "bs", `a\b`,
		"f", 1.5, "b", true, "bytes", []byte("hi"), "i64", int64(-5), "u64", uint64(7), "dur", 1500 * ms,
		"nilerr", error(nil),
	}})
	add(slogvCase{name: "cli strings", offset: 7200, t: cliT, level: errl, msg: "msg with space", args: []any{
		"rate", "3.0 MiB/s", "addrs", []string{"1.2.3.4:5", "[::1]:6"}, "version", "0102",
		"in", time.Second, "took", 1235 * ms,
	}})
	add(slogvCase{name: "cli empty message", offset: 7200, t: cliT, level: info, msg: ""})
	add(slogvCase{name: "cli node started", offset: 7200, t: cliT, level: info, msg: "node started",
		args: []any{"node", "abcd0123", "id", strings.Repeat("ab", 32)}})
	add(slogvCase{name: "cli odd argument", offset: 7200, t: cliT, level: info, msg: "odd", args: []any{"lonely"}})
	add(slogvCase{name: "cli custom level", offset: 7200, t: cliT, level: info + 2, msg: "custom level"})

	// Go log/slog text_handler_test.go (the cases without TextMarshaler values).
	add(slogvCase{name: "go unquoted", t: t2000, level: info, msg: "a message", args: []any{slog.Int("a", 1)}})
	add(slogvCase{name: "go quoted", t: t2000, level: info, msg: "a message", args: []any{slog.String("x = y", `qu"o`)}})
	add(slogvCase{name: "go String method", t: t2000, level: info, msg: "a message",
		args: []any{slog.Any("name", slogvName{"Ren", "Hoek"})}})
	add(slogvCase{name: "go struct", t: t2000, level: info, msg: "a message",
		args: []any{slog.Any("x", &struct{ A, b int }{A: 1, b: 2})}})
	add(slogvCase{name: "go nil value", t: t2000, level: info, msg: "a message", args: []any{slog.Any("a", nil)}})
	add(slogvCase{name: "go preformatted", level: info, msg: "m",
		with: [][]any{{slog.Duration("dur", time.Minute), slog.Bool("b", true)}},
		args: []any{slog.Int("a", 1)}})

	// Logger.With.
	add(slogvCase{name: "with twice", t: base, level: info, msg: "uploaded",
		with: [][]any{{"node", "abcd"}, {"round", 1}},
		args: []any{"objects", 3, "bytes", int64(3 << 20), "took", 2 * time.Second, "path", "direct"}})
	add(slogvCase{name: "with only", t: base, level: warn, msg: "upload failed", with: [][]any{{"node", "abcd"}}})
	add(slogvCase{name: "with three calls", t: base, level: info, msg: "m",
		with: [][]any{{"a", 1}, {"b", "x y"}, {"c", true, "d", uint64(4)}}})

	// More attributes than a Record keeps inline.
	var many []any
	for i := 0; i < 12; i++ {
		many = append(many, fmt.Sprintf("a%d", i), i)
	}
	add(slogvCase{name: "many attributes", t: base, level: info, msg: "many", with: [][]any{{"w", 0}}, args: many})

	// Keys that need quoting.
	add(slogvCase{name: "keys", t: base, level: info, msg: "keys", args: []any{
		"", "empty key", "a b", 1, "a=b", 2, `a"b`, 3, "é", 4, "k\n", 5, `\`, 6, "\u00a0", 7, "日本", 8,
		"\x7f", 9, "!BADKEY", 10, "trees/**", 11, "a.b", 12,
	}})

	// Messages.
	for i, m := range []string{
		"tab\tin", "line\nbreak", "é", "\u200b", `quote"`, "a=b", "\x7f", "trailing\n", "日本語", "😀",
		"e\u0301", "\u3000", "\ufffd", `\`, "<b>&amp;</b>", " ", "\x00", "\u2028",
	} {
		add(slogvCase{name: fmt.Sprintf("message %d", i), t: base, level: info, msg: m})
	}

	// Levels.
	for _, l := range []slog.Level{-100, -5, -4, -1, 0, 3, 4, 7, 8, 12, 100} {
		add(slogvCase{name: "level " + l.String(), t: base, level: l, msg: "level"})
	}

	// Values by kind.
	add(slogvCase{name: "float64", t: base, level: info, msg: "floats", args: []any{
		"zero", 0.0, "negzero", math.Copysign(0, -1), "half", 0.5, "onehalf", 1.5, "big", 3.7e6,
		"e20", 1e20, "e21", 1e21, "e-4", 1e-4, "e-5", 1e-5, "e-7", 1e-7, "int", 123456789.0,
		"hundred", 100.0, "third", 1.0 / 3, "max", math.MaxFloat64, "min", math.SmallestNonzeroFloat64,
		"f32", float32(0.1), "nan", math.NaN(), "inf", math.Inf(1), "neginf", math.Inf(-1),
	}})
	add(slogvCase{name: "integers", t: base, level: info, msg: "ints", args: []any{
		"min", int64(math.MinInt64), "max", int64(math.MaxInt64), "zero", 0, "umax", uint64(math.MaxUint64),
		"int", 42, "u8", uint8(255), "i32", int32(-7), "uint", uint(9), "i8", int8(-128), "u16", uint16(65535),
	}})
	add(slogvCase{name: "durations", t: base, level: info, msg: "durations", args: []any{
		"zero", time.Duration(0), "ns", time.Nanosecond, "999ns", 999 * time.Nanosecond,
		"us", time.Microsecond, "1.5ms", 1500 * time.Microsecond, "2m", 2 * time.Minute,
		"26h0m3s", 26*time.Hour + 3*time.Second, "neg", -time.Second,
		"min", time.Duration(math.MinInt64), "max", time.Duration(math.MaxInt64),
		"frac", 1234567 * time.Microsecond,
	}})
	zone2 := time.FixedZone("", 7200)
	add(slogvCase{name: "time attributes", offset: 7200, t: base, level: info, msg: "times", args: []any{
		"at", base.In(zone2), "epoch", time.Unix(0, 0).In(zone2), "pre-epoch", time.Unix(-1, 999999999).In(zone2),
		"exact", time.Date(2026, time.September, 18, 10, 11, 12, 0, time.UTC).In(zone2), "y2000", t2000.In(zone2),
	}})

	// Record times and zones.
	add(slogvCase{name: "exact second", t: time.Date(2026, time.September, 18, 10, 11, 12, 0, time.UTC), level: info, msg: "t"})
	add(slogvCase{name: "last nanosecond", t: time.Date(2026, time.September, 18, 10, 11, 12, 999999999, time.UTC), level: info, msg: "t"})
	add(slogvCase{name: "pre-epoch", t: time.Unix(-1, 999500000), level: info, msg: "t"})
	add(slogvCase{name: "epoch", t: time.Unix(0, 0), level: info, msg: "t"})
	add(slogvCase{name: "year 1900", t: time.Date(1900, time.January, 1, 0, 0, 0, 1, time.UTC), level: info, msg: "t"})
	add(slogvCase{name: "year 2000 +05:30", offset: 19800, t: t2000, level: info, msg: "t"})
	add(slogvCase{name: "-03:30", offset: -12600, t: base, level: info, msg: "t"})
	add(slogvCase{name: "offset with seconds", offset: 3661, t: base, level: info, msg: "t"})
	add(slogvCase{name: "negative offset with seconds", offset: -3599, t: base, level: info, msg: "t"})
	add(slogvCase{name: "offset below a minute", offset: 30, t: base, level: info, msg: "t"})
	add(slogvCase{name: "+14:00 across midnight", offset: 50400, t: base, level: info, msg: "t"})

	// Byte slices.
	all := make([]byte, 256)
	for i := range all {
		all[i] = byte(i)
	}
	add(slogvCase{name: "bytes", t: base, level: info, msg: "bytes", args: []any{
		"empty", []byte{}, "nil", []byte(nil), "hi", []byte("hi"), "quote", []byte(`a"b\c`),
		"binary", []byte("\xff\x00\x7f"), "utf8", []byte("é"), "all", all, "ls", []byte("\u2028"),
	}})

	// Errors and other values, rendered with %+v.
	add(slogvCase{name: "any values", t: base, level: info, msg: "any", args: []any{
		"eof", io.EOF,
		"wrapped", fmt.Errorf("dial %s: %w", "ip:1.2.3.4:5", errors.New("timeout")),
		"joined", errors.Join(errors.New("a"), errors.New("b")),
		"nil", error(nil), "empty", []string{}, "one", []string{"a"}, "ints", []int{1, 2, 3},
		"struct", struct {
			A int
			B string
		}{1, "x y"},
		"nilptr", (*int)(nil), "quoted", errors.New(`say "hi"`),
	}})

	// Strings by Unicode class.
	add(slogvCase{name: "unicode", t: base, level: info, msg: "unicode", args: []any{
		"cjk", "日本語", "emoji", "😀", "combining", "e\u0301", "soft_hyphen", "\u00ad", "tag", "\U000e0001",
		"unassigned", "\u0378", "private", "\ue000", "line_sep", "\u2028", "ideographic_space", "\u3000",
		"nel", "\u0085", "replacement", "\ufffd", "greek", "µåπ", "bom", "\ufeff", "max", "\U0010ffff",
	}})
	add(slogvCase{name: "non-string argument", t: base, level: info, msg: "badkey", args: []any{42, "k", "v", true}})
	return cases
}

func slogvRunRecord(c slogvCase) (slogvRecord, error) {
	zone := time.FixedZone("", c.offset)
	rec := slogvRecord{
		Name: c.name, OffsetSecs: c.offset, Level: int32(c.level), Msg: c.msg,
		With: [][]slogvAttr{}, Attrs: []slogvAttr{},
	}
	if !utf8.ValidString(c.msg) {
		return rec, fmt.Errorf("message %q is not valid UTF-8", c.msg)
	}
	var w slogvWriter
	l := slog.New(slog.NewTextHandler(&w, nil))
	for _, args := range c.with {
		l = l.With(args...)
		attrs, err := slogvDescribeArgs(args, c.offset)
		if err != nil {
			return rec, err
		}
		rec.With = append(rec.With, attrs)
	}
	t := c.t
	if !t.IsZero() {
		t = t.In(zone)
		rec.Time = &slogvTime{UnixSecs: I64(t.Unix()), Nanos: uint32(t.Nanosecond())}
	}
	r := slog.NewRecord(t, c.level, c.msg, 0)
	r.Add(c.args...)
	attrs, err := slogvDescribeRecord(r, c.offset)
	if err != nil {
		return rec, err
	}
	rec.Attrs = attrs
	if err := l.Handler().Handle(context.Background(), r); err != nil {
		return rec, err
	}
	line, err := w.one()
	if err != nil {
		return rec, err
	}
	if !utf8.ValidString(line) {
		return rec, fmt.Errorf("line %q is not valid UTF-8", line)
	}
	rec.Line = line
	return rec, nil
}

// ---- slog.Default() ----

func slogvDefaultCases() []slogvCase {
	now := time.Date(2026, time.September, 18, 10, 11, 12, 345678901, time.UTC)
	info := slog.LevelInfo
	var cases []slogvCase
	add := func(c slogvCase) { cases = append(cases, c) }
	add(slogvCase{name: "connected +02:00", offset: 7200, t: now, level: info, msg: "connected",
		args: []any{"node", "1a2b3c4d", "nodes", 3, "path", "none"}})
	add(slogvCase{name: "message with spaces", t: now, level: info, msg: "watch: synced",
		args: []any{"pattern", "trees/**", "node", "1a2b3c4d", "refs", 2}})
	add(slogvCase{name: "trailing newline", t: now, level: info, msg: "hello\n"})
	add(slogvCase{name: "trailing newline then attributes", t: now, level: info, msg: "hello\n", args: []any{"k", "v"}})
	add(slogvCase{name: "empty message", t: now, level: info, msg: ""})
	add(slogvCase{name: "upload failed", t: now, level: slog.LevelWarn, msg: "upload failed",
		args: []any{"node", "1a2b3c4d", "objects", 12, "err", errors.New("timeout: no recent network activity")}})
	add(slogvCase{name: "info plus two", t: now, level: info + 2, msg: "custom"})
	add(slogvCase{name: "error plus four", t: now, level: slog.LevelError + 4, msg: "custom"})
	add(slogvCase{name: "debug", t: now, level: slog.LevelDebug, msg: "debug"})
	add(slogvCase{name: "debug minus one", t: now, level: slog.LevelDebug - 1, msg: "debug"})
	add(slogvCase{name: "with", t: now, level: info, msg: "uploaded", with: [][]any{{"node", "abcd"}},
		args: []any{"objects", 3, "took", 2 * time.Second}})
	add(slogvCase{name: "quoting", t: now, level: info, msg: "q", args: []any{
		"empty", "", "space", "a b", "quote", `a"b`, "tab", "a\tb", "bytes", []byte("a\xffb"), "f", 0.5,
		"nilerr", error(nil), "", "empty key",
	}})
	add(slogvCase{name: "raw message characters", t: now, level: info, msg: "a\"b\tc=d\x7f"})
	add(slogvCase{name: "time attribute -05:30", offset: -19800, t: now, level: info, msg: "t",
		args: []any{"at", now.In(time.FixedZone("", -19800))}})
	add(slogvCase{name: "header -05:30", offset: -19800, t: now, level: info, msg: "x"})
	add(slogvCase{name: "header pre-epoch", t: time.Unix(-1, 0), level: info, msg: "x"})
	add(slogvCase{name: "header year 2000 +05:30", offset: 19800,
		t: time.Date(2000, time.January, 2, 3, 4, 5, 999999999, time.UTC), level: info, msg: "x"})
	return cases
}

// slogvLogHeaderLayout is log.LstdFlags' header: formatHeader's itoa layout
// equals this time layout for years 1000 to 9999.
const slogvLogHeaderLayout = "2006/01/02 15:04:05 "

// slogvRunDefault handles a record with slog.Default()'s handler, capturing
// what it passes to the standard logger with flags 0, and prefixes the
// LstdFlags header for c.t (slogvCheckLogHeader checks that composition).
func slogvRunDefault(c slogvCase) (rec slogvDefault, err error) {
	if h := fmt.Sprintf("%T", slog.Default().Handler()); h != "*slog.defaultHandler" {
		return rec, fmt.Errorf("slog.Default() has handler %s, want the default handler", h)
	}
	zone := time.FixedZone("", c.offset)
	rec = slogvDefault{
		Name: c.name, OffsetSecs: c.offset, Now: slogvTime{UnixSecs: I64(c.t.Unix()), Nanos: uint32(c.t.Nanosecond())},
		Level: int32(c.level), Msg: c.msg, With: [][]slogvAttr{}, Attrs: []slogvAttr{},
	}
	if !utf8.ValidString(c.msg) {
		return rec, fmt.Errorf("message %q is not valid UTF-8", c.msg)
	}
	var w slogvWriter
	log.SetOutput(&w)
	log.SetFlags(0)
	defer func() {
		log.SetOutput(os.Stderr)
		log.SetFlags(log.LstdFlags)
	}()
	l := slog.Default()
	for _, args := range c.with {
		l = l.With(args...)
		attrs, err := slogvDescribeArgs(args, c.offset)
		if err != nil {
			return rec, err
		}
		rec.With = append(rec.With, attrs)
	}
	// The default handler ignores the record time; the log package reads the clock.
	r := slog.NewRecord(time.Time{}, c.level, c.msg, 0)
	r.Add(c.args...)
	if rec.Attrs, err = slogvDescribeRecord(r, c.offset); err != nil {
		return rec, err
	}
	if err := l.Handler().Handle(context.Background(), r); err != nil {
		return rec, err
	}
	body, err := w.one()
	if err != nil {
		return rec, err
	}
	rec.Line = c.t.In(zone).Format(slogvLogHeaderLayout) + body
	if !utf8.ValidString(rec.Line) {
		return rec, fmt.Errorf("line %q is not valid UTF-8", rec.Line)
	}
	return rec, nil
}

// slogvCheckLogHeader checks that slog.Default() with the standard logger's
// real flags writes the LstdFlags header of the current local time followed by
// the text slogvRunDefault captures with flags 0.
func slogvCheckLogHeader() error {
	if h := fmt.Sprintf("%T", slog.Default().Handler()); h != "*slog.defaultHandler" {
		return fmt.Errorf("slog.Default() has handler %s, want the default handler", h)
	}
	if f := log.Flags(); f != log.LstdFlags {
		return fmt.Errorf("log flags are %d, want LstdFlags", f)
	}
	var w slogvWriter
	log.SetOutput(&w)
	defer log.SetOutput(os.Stderr)
	r := slog.NewRecord(time.Time{}, slog.LevelInfo, "self-check", 0)
	r.Add("k", "a b")
	before := time.Now()
	if err := slog.Default().Handler().Handle(context.Background(), r); err != nil {
		return err
	}
	after := time.Now()
	line, err := w.one()
	if err != nil {
		return err
	}
	const body = "INFO self-check k=\"a b\"\n"
	if line != before.Format(slogvLogHeaderLayout)+body && line != after.Format(slogvLogHeaderLayout)+body {
		return fmt.Errorf("log header self-check: got %q, want the header of %v or %v then %q", line, before, after, body)
	}
	return nil
}

// ---- attribute descriptions ----

// slogvDescribeArgs describes the attributes Logger.With(args...) adds, which
// Record.Add builds the same way (argsToAttr).
func slogvDescribeArgs(args []any, offset int) ([]slogvAttr, error) {
	r := slog.NewRecord(time.Time{}, slog.LevelInfo, "", 0)
	r.Add(args...)
	return slogvDescribeRecord(r, offset)
}

func slogvDescribeRecord(r slog.Record, offset int) ([]slogvAttr, error) {
	out := []slogvAttr{}
	var err error
	r.Attrs(func(a slog.Attr) bool {
		var d slogvAttr
		if d, err = slogvDescribe(a, offset); err != nil {
			return false
		}
		out = append(out, d)
		return true
	})
	return out, err
}

func slogvDescribe(a slog.Attr, offset int) (slogvAttr, error) {
	d := slogvAttr{Key: a.Key}
	if !utf8.ValidString(a.Key) {
		return d, fmt.Errorf("key %q is not valid UTF-8", a.Key)
	}
	num := func(s string) *string { return &s }
	v := a.Value
	switch v.Kind() {
	case slog.KindString:
		s := v.String()
		if !utf8.ValidString(s) {
			return d, fmt.Errorf("value %q of %q is not valid UTF-8", s, a.Key)
		}
		d.Kind, d.Str = "string", &s
	case slog.KindInt64:
		d.Kind, d.Num = "int64", num(strconv.FormatInt(v.Int64(), 10))
	case slog.KindUint64:
		d.Kind, d.Num = "uint64", num(strconv.FormatUint(v.Uint64(), 10))
	case slog.KindFloat64:
		d.Kind, d.Num = "float64", num(strconv.FormatUint(math.Float64bits(v.Float64()), 10))
	case slog.KindBool:
		b := v.Bool()
		d.Kind, d.Bool = "bool", &b
	case slog.KindDuration:
		d.Kind, d.Num = "duration", num(strconv.FormatInt(int64(v.Duration()), 10))
	case slog.KindTime:
		t := v.Time()
		if _, off := t.Zone(); off != offset {
			return d, fmt.Errorf("time attribute %q has offset %d, want the case's %d", a.Key, off, offset)
		}
		d.Kind, d.Time = "time", &slogvTime{UnixSecs: I64(t.Unix()), Nanos: uint32(t.Nanosecond())}
	case slog.KindAny:
		x := v.Any()
		if bs, ok := x.([]byte); ok {
			h := hex.EncodeToString(bs)
			d.Kind, d.Hex = "bytes", &h
			break
		}
		if _, ok := x.(encoding.TextMarshaler); ok {
			return d, fmt.Errorf("attribute %q: TextMarshaler values are not modelled", a.Key)
		}
		s := fmt.Sprintf("%+v", x)
		if !utf8.ValidString(s) {
			return d, fmt.Errorf("value %q of %q is not valid UTF-8", s, a.Key)
		}
		d.Kind, d.Str, d.GoType = "any", &s, fmt.Sprintf("%T", x)
	default:
		return d, fmt.Errorf("attribute %q: kind %s is not modelled", a.Key, v.Kind())
	}
	return d, nil
}
