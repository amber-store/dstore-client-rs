package main

// Family "cli": cli/size.json and cli/text.json, the unit vectors of
// cmd/dstore (port-notes/cli.md §5.4, verification.md §4.3 items 20-21 and
// §5). The package main functions come from the verbatim copies in
// cmd/clisnap/mainpkg, which SelfCheck compares with the dstore module's
// cmd/dstore sources before anything is generated. cli/snapshots.json is
// written by cmd/clisnap. Schemas: docs/vectorgen-cli.md.

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"image/color"
	"log/slog"
	"math"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"github.com/amber-store/core/fstree"
	"github.com/amber-store/dstore-client-rs/tools/vectorgen/cmd/clisnap/mainpkg"
	"github.com/amber-store/dstore/client"
	"github.com/amber-store/dstore/view"
	"github.com/amber-store/dstore/worktree"
	"github.com/charmbracelet/colorprofile"
	"github.com/charmbracelet/x/ansi"
)

func init() {
	register("cli", []string{"cli/size.json", "cli/text.json"}, genCLI)
}

func genCLI(out string) error {
	if err := mainpkg.SelfCheck(); err != nil {
		return err
	}
	size, err := cliSizeVectors()
	if err != nil {
		return err
	}
	if err := writeJSON(filepath.Join(out, "cli", "size.json"), size); err != nil {
		return err
	}
	text, err := cliTextVectors()
	if err != nil {
		return err
	}
	return writeJSON(filepath.Join(out, "cli", "text.json"), text)
}

// cliF64 marshals a float64 as the shortest decimal string that parses back
// to the same value (strconv.FormatFloat 'g' -1), so readers never depend on
// a JSON number parser's rounding.
type cliF64 float64

func (f cliF64) MarshalJSON() ([]byte, error) {
	return []byte(strconv.Quote(strconv.FormatFloat(float64(f), 'g', -1, 64))), nil
}

func cliStr(s string) *string { return &s }

func cliI64p(n int64) *I64 { v := I64(n); return &v }

// ---- cli/size.json ----

type cliSizeFile struct {
	Parse    []cliParseCase    `json:"parse"`
	PackSize []cliPackSizeCase `json:"pack_size"`
}

type cliParseCase struct {
	In    string `json:"in"`
	OK    bool   `json:"ok"`
	Out   *I64   `json:"out,omitempty"`
	Error string `json:"error,omitempty"`
}

type cliPackSizeCase struct {
	Name  string  `json:"name"`
	Flag  *string `json:"flag"`
	Env   *string `json:"env"`
	OK    bool    `json:"ok"`
	Out   *I64    `json:"out,omitempty"`
	Error string  `json:"error,omitempty"`
}

func cliSizeVectors() (cliSizeFile, error) {
	var f cliSizeFile
	inputs := []string{
		// size_test.go TestParseSize, good.
		"0", "1024", "512Ki", "256Mi", "2Gi", "1Ti", "2gi", "2GiB", "2G", "2GB", " 2Gi ",
		// size_test.go TestParseSize, bad.
		"", "Gi", "2X", "2.5Gi", "-1", "1e3", "9999999999Ti",
		// port-notes/cli.md §5.4 and verification.md §5.
		"2Gib", "2b", "2kb", "8Ti", "8388607Ti", "8388608Ti", "99999999999999999999", "2 Gi", " 1 Gi", "+1", "1KiBB",
		// Every unit spelling, the int64 limits and the trimming rules.
		"2K", "2k", "2KiB", "2kib", "2KIB", "2M", "2m", "2Mb", "2T", "2tb", "2B", "00012",
		"9223372036854775807", "9223372036854775807b", "9223372036854775808", "8796093022207Mi", "8796093022208Mi",
		"9007199254740991Ki", "9007199254740992Ki", "\t2Gi\n", "\u00a02Gi\u00a0", "3 ", "1.5Gi", "b", "0x10", "1_000",
		"２Gi", "2\u00a0Gi", "2i", "2bb", "2Kb2", "2Pi", "2E", "1 ", "1e", "0Ti",
	}
	for _, in := range inputs {
		n, err := mainpkg.ParseSize(in)
		c := cliParseCase{In: in, OK: err == nil}
		if err != nil {
			c.Error = err.Error()
		} else {
			c.Out = cliI64p(n)
		}
		f.Parse = append(f.Parse, c)
	}
	if err := cliExpectParse(f.Parse); err != nil {
		return f, err
	}
	packCases := []struct {
		name      string
		flag, env *string
	}{
		{"default", nil, nil},
		{"flag", cliStr("512Mi"), nil},
		{"env", nil, cliStr("1Gi")},
		{"env-empty-is-default", nil, cliStr("")},
		{"flag-zero", cliStr("0"), nil},
		{"flag-x", cliStr("x"), nil},
		{"flag-fraction", cliStr("1.5Gi"), nil},
		{"env-x", nil, cliStr("x")},
		{"flag-blank-is-default", cliStr(" "), nil},
		{"env-trimmed", nil, cliStr(" 1Gi ")},
		{"flag-over-env", cliStr("512Mi"), cliStr("1Gi")},
		{"empty-flag-over-env", cliStr(""), cliStr("1Gi")},
		{"flag-negative", cliStr("-1"), nil},
		{"flag-too-large", cliStr("9999999999Ti"), nil},
		{"flag-over-bad-env", cliStr("2Gi"), cliStr("x")},
		{"flag-one-byte", cliStr("1b"), nil},
		{"flag-zero-ti", cliStr("0Ti"), nil},
	}
	for _, pc := range packCases {
		res, err := mainpkg.PackSize(pc.flag, pc.env)
		if err != nil {
			return f, fmt.Errorf("pack_size %s: %w", pc.name, err)
		}
		c := cliPackSizeCase{Name: pc.name, Flag: pc.flag, Env: pc.env, OK: res.Err == nil}
		if res.Err != nil {
			c.Error = res.Err.Error()
		} else {
			c.Out = cliI64p(res.N)
		}
		f.PackSize = append(f.PackSize, c)
	}
	// Spot checks against the verified texts of port-notes/cli.md §3.3.
	for _, want := range []struct{ name, err string }{
		{"flag-zero", "--pack-size: 0 is not a positive size"},
		{"flag-fraction", `--pack-size: bad size "1.5Gi": unknown unit ".5Gi" (want Ki, Mi, Gi or Ti)`},
		{"env-x", `--pack-size: bad size "x": want a number with an optional Ki/Mi/Gi/Ti suffix`},
	} {
		found := false
		for _, c := range f.PackSize {
			if c.Name == want.name {
				found = true
				if c.Error != want.err {
					return f, fmt.Errorf("pack_size %s: error %q, the verified text is %q", want.name, c.Error, want.err)
				}
			}
		}
		if !found {
			return f, fmt.Errorf("pack_size %s: no such case", want.name)
		}
	}
	return f, nil
}

// cliExpectParse checks the parse cases against port-notes/cli.md §5.4.
func cliExpectParse(cases []cliParseCase) error {
	want := map[string]string{
		"8388607Ti":            "9223370937343148032",
		"2kb":                  "2048",
		"99999999999999999999": `bad size "99999999999999999999": strconv.ParseInt: parsing "99999999999999999999": value out of range`,
		"2 Gi":                 `bad size "2 Gi": unknown unit " Gi" (want Ki, Mi, Gi or Ti)`,
		"8388608Ti":            `bad size "8388608Ti": too large`,
		"":                     `bad size "": want a number with an optional Ki/Mi/Gi/Ti suffix`,
	}
	for _, c := range cases {
		w, ok := want[c.In]
		if !ok {
			continue
		}
		got := c.Error
		if c.OK {
			got = strconv.FormatInt(int64(*c.Out), 10)
		}
		if got != w {
			return fmt.Errorf("parse %q: got %q, the verified value is %q", c.In, got, w)
		}
		delete(want, c.In)
	}
	if len(want) > 0 {
		return fmt.Errorf("parse: %d verified cases are not inputs", len(want))
	}
	return nil
}

// ---- cli/text.json ----

type cliTextFile struct {
	HumanBytes     []cliHumanBytesCase    `json:"human_bytes"`
	Rate           []cliRateCase          `json:"rate"`
	StatusLine     []cliStatusLineCase    `json:"status_line"`
	RateMeter      []cliRateMeterCase     `json:"rate_meter"`
	HexDecode      []cliHexDecodeCase     `json:"hex_decode"`
	ResolveTicket  []cliResolveTicketCase `json:"resolve_ticket"`
	LogLevel       []cliLogLevelCase      `json:"log_level"`
	DescribeChange []cliDescribeCase      `json:"describe_change"`
	FilterPaths    []cliFilterPathsCase   `json:"filter_paths"`
	NodeState      []cliNodeStateCase     `json:"node_state"`
	TeaHandler     []cliTeaCase           `json:"tea_handler"`
	FormatEvent    []cliFormatEventCase   `json:"format_event"`
	Blend1D        []cliBlendCase         `json:"blend1d"`
	ProgressBar    []cliBarCase           `json:"progress_bar"`
	UIModel        []cliUIScenario        `json:"ui_model"`
	ColorProfile   []cliColorProfileCase  `json:"color_profile"`
	Convert256     []cliConvertCase       `json:"convert256"`
	Downsample     []cliDownsampleCase    `json:"downsample"`
}

type cliHumanBytesCase struct {
	N   I64    `json:"n"`
	Out string `json:"out"`
}

type cliRateCase struct {
	Bytes  I64    `json:"bytes"`
	TookNs I64    `json:"took_ns"`
	Out    string `json:"out"`
}

type cliReport struct {
	Objects      int       `json:"objects"`
	TotalObjects int       `json:"total_objects"`
	Bytes        I64       `json:"bytes"`
	TotalBytes   I64       `json:"total_bytes"`
	Nodes        []cliNode `json:"nodes"`
}

type cliNode struct {
	ID       Hex  `json:"id"`
	Direct   bool `json:"direct"`
	RTTNs    I64  `json:"rtt_ns"`
	InFlight int  `json:"in_flight"`
	Awaiting int  `json:"awaiting"`
	Bytes    I64  `json:"bytes"`
}

func cliReportJSON(r client.ProgressReport) *cliReport {
	j := &cliReport{Objects: r.Objects, TotalObjects: r.TotalObjects, Bytes: I64(r.Bytes), TotalBytes: I64(r.TotalBytes), Nodes: []cliNode{}}
	for _, n := range r.Nodes {
		j.Nodes = append(j.Nodes, cliNode{ID: Hex(append([]byte(nil), n.ID[:]...)), Direct: n.Direct, RTTNs: I64(n.RTT), InFlight: n.InFlight, Awaiting: n.Awaiting, Bytes: I64(n.Bytes)})
	}
	return j
}

type cliStatusLineCase struct {
	Report   *cliReport `json:"report"`
	Rate     cliF64     `json:"rate"`
	Out      string     `json:"out"`
	Fraction cliF64     `json:"fraction"`
}

type cliRateMeterCase struct {
	Name     string            `json:"name"`
	WindowNs I64               `json:"window_ns"`
	Adds     []cliRateMeterAdd `json:"adds"`
}

type cliRateMeterAdd struct {
	TNs     I64    `json:"t_ns"`
	N       I64    `json:"n"`
	Rate    cliF64 `json:"rate"`
	Samples int    `json:"samples"`
}

type cliHexDecodeCase struct {
	In    string `json:"in"`
	OK    bool   `json:"ok"`
	Out   Hex    `json:"out"`
	Error string `json:"error,omitempty"`
}

type cliResolveTicketCase struct {
	Flag   string `json:"flag"`
	Stored string `json:"stored"`
	Env    string `json:"env"`
	OK     bool   `json:"ok"`
	Out    string `json:"out,omitempty"`
	Error  string `json:"error,omitempty"`
}

type cliLogLevelCase struct {
	Value string `json:"value"`
	Level int    `json:"level"`
}

type cliDescribeCase struct {
	Path       string  `json:"path"`
	Kind       int     `json:"kind"`
	KindString string  `json:"kind_string"`
	OldMode    *uint64 `json:"old_mode"`
	NewMode    *uint64 `json:"new_mode"`
	Describe   string  `json:"describe"`
	Line       string  `json:"line"`
}

type cliFilterPathsCase struct {
	Changes []string `json:"changes"`
	Args    []string `json:"args"`
	OK      bool     `json:"ok"`
	Out     []string `json:"out"`
	Error   string   `json:"error,omitempty"`
}

type cliNodeStateCase struct {
	InFlight int    `json:"in_flight"`
	Awaiting int    `json:"awaiting"`
	Out      string `json:"out"`
}

type cliAttr struct {
	Key   string `json:"key"`
	Kind  string `json:"kind"`
	Value string `json:"value"`
}

type cliTeaCase struct {
	Name         string    `json:"name"`
	HandlerLevel int       `json:"handler_level"`
	With         []cliAttr `json:"with"`
	Level        int       `json:"level"`
	Msg          string    `json:"msg"`
	Attrs        []cliAttr `json:"attrs"`
	Enabled      bool      `json:"enabled"`
	Text         *string   `json:"text"`
}

type cliFormatEventCase struct {
	AtUnixNs   I64    `json:"at_unix_ns"`
	OffsetSecs int    `json:"offset_secs"`
	Level      int    `json:"level"`
	Text       string `json:"text"`
	Out        string `json:"out"`
}

type cliBlendCase struct {
	Steps int        `json:"steps"`
	A     [3]uint8   `json:"a"`
	B     [3]uint8   `json:"b"`
	Out   [][3]uint8 `json:"out"`
}

type cliBarCase struct {
	Width   int    `json:"width"`
	Percent cliF64 `json:"percent"`
	Out     string `json:"out"`
}

type cliUIScenario struct {
	Name       string      `json:"name"`
	Title      string      `json:"title"`
	OffsetSecs int         `json:"offset_secs"`
	Steps      []cliUIStep `json:"steps"`
}

type cliUIStep struct {
	Op        string     `json:"op"`
	Report    *cliReport `json:"report,omitempty"`
	Width     *int       `json:"width,omitempty"`
	OffsetNs  *I64       `json:"offset_ns,omitempty"`
	AtUnixNs  *I64       `json:"at_unix_ns,omitempty"`
	Level     *int       `json:"level,omitempty"`
	Text      *string    `json:"text,omitempty"`
	Error     *string    `json:"error,omitempty"`
	Quit      *bool      `json:"quit,omitempty"`
	Cancelled *bool      `json:"cancelled,omitempty"`
	Out       *string    `json:"out,omitempty"`
	Contains  []string   `json:"contains,omitempty"`
}

func cliTextVectors() (cliTextFile, error) {
	var f cliTextFile
	var err error
	cliHumanBytes(&f)
	cliRates(&f)
	cliStatusLines(&f)
	cliRateMeters(&f)
	cliHexDecodes(&f)
	cliResolveTickets(&f)
	if err = cliLogLevels(&f); err != nil {
		return f, err
	}
	cliDescribeChanges(&f)
	if err = cliFilterPaths(&f); err != nil {
		return f, err
	}
	cliNodeStates(&f)
	if err = cliTeaHandlers(&f); err != nil {
		return f, err
	}
	cliFormatEvents(&f)
	cliBlends(&f)
	cliBars(&f)
	if err = cliUIScenarios(&f); err != nil {
		return f, err
	}
	cliColorProfiles(&f)
	cliConverts(&f)
	if err = cliDownsamples(&f); err != nil {
		return f, err
	}
	return f, cliCheckText(&f)
}

func cliHumanBytes(f *cliTextFile) {
	for _, n := range []int64{
		0, 1, 1023, 1024, 1126, 1152, 1280, 1331, 1536, 1792, 2304, 3584, 3840, 1048524, 1048575, 1048576, 1101004,
		1310720, 1073741823, 1073741824, 5 << 30, 5497558138880, 1<<50 - 1, 1 << 50, 1 << 60, math.MaxInt64,
		-1, -5, -2048, math.MinInt64,
	} {
		f.HumanBytes = append(f.HumanBytes, cliHumanBytesCase{N: I64(n), Out: client.HumanBytes(n)})
	}
}

func cliRates(f *cliTextFile) {
	for _, c := range []struct {
		bytes int64
		took  time.Duration
	}{
		{1000, 0}, {1000, -1}, {3 << 20, 2 * time.Second}, {1, time.Nanosecond}, {1000, 1500 * time.Millisecond},
		{5 << 30, 3 * time.Second}, {0, time.Second}, {-2048, time.Second}, {1 << 40, time.Millisecond}, {512, 10 * time.Second},
	} {
		f.Rate = append(f.Rate, cliRateCase{Bytes: I64(c.bytes), TookNs: I64(c.took), Out: client.Rate(c.bytes, c.took)})
	}
}

func cliStatusLines(f *cliTextFile) {
	for _, c := range []struct {
		objects, total int
		bytes, totalB  int64
		rate           float64
	}{
		{3, 4, 0, 0, 0},
		{0, 0, 512, 0, 100},
		{5, 10, 1024, 2048, 512},
		{5, 10, 1 << 30, 5 << 30, 3.7e6},
		{5, 10, 3000, 1000, 10},
		{1, 2, 10, 100, 0.5},
		{1, 2, 10, 100000000, 1},
		{0, 0, 0, 0, 0},
		{1, 2, 0, 1500, 1000},
		{1, 2, 0, 2500, 1000},
		{1, 2, 0, 1499, 1000},
		{1, 2, 512, 0, 0},
		{7, 7, 4096, 4096, 2048},
		{1, 2, 10, 100, -5},
		{1, 2, 0, 100, 0.999},
		{-1, 3, -5, 10, 0},
		{0, 1, 0, 1 << 30, 3.7e6},
		{3, 4, 1, 0, 1023.9},
	} {
		r := client.ProgressReport{Objects: c.objects, TotalObjects: c.total, Bytes: c.bytes, TotalBytes: c.totalB}
		f.StatusLine = append(f.StatusLine, cliStatusLineCase{Report: cliReportJSON(r), Rate: cliF64(c.rate), Out: mainpkg.StatusLine(r, c.rate), Fraction: cliF64(mainpkg.Fraction(r))})
	}
}

func cliRateMeters(f *cliTextFile) {
	type add struct {
		t time.Duration
		n int64
	}
	s := time.Second
	cases := []struct {
		name   string
		window time.Duration
		adds   []add
	}{
		{"cli-md-5.4", 5 * s, []add{{0, 0}, {s, 1000}, {2 * s, 1100}, {3 * s, 1200}, {7 * s, 1600}, {8 * s, 1700}, {8 * s, 1700}, {9 * s, 100}, {15 * s, 5000}}},
		{"backwards-time", 5 * s, []add{{10 * s, 0}, {9 * s, 100}, {11 * s, 200}}},
		{"window-2s", 2 * s, []add{{0, 0}, {s, 100}, {2 * s, 300}, {3 * s, 600}, {10 * s, 700}}},
		{"sub-millisecond", 5 * s, []add{{0, 0}, {500 * time.Microsecond, 10}}},
	}
	tui := []add{{0, 0}, {s, 1000}}
	for i := 2; i <= 11; i++ {
		tui = append(tui, add{time.Duration(i) * s, 1000 + int64(i-1)*100})
	}
	tui = append(tui, add{12 * s, 2100})
	cases = append(cases, struct {
		name   string
		window time.Duration
		adds   []add
	}{"tui-test-go", 5 * s, tui})
	t0 := time.Unix(0, 0)
	for _, c := range cases {
		m := mainpkg.NewRateMeter(c.window)
		rc := cliRateMeterCase{Name: c.name, WindowNs: I64(c.window)}
		for _, a := range c.adds {
			rate := m.Add(t0.Add(a.t), a.n)
			rc.Adds = append(rc.Adds, cliRateMeterAdd{TNs: I64(a.t), N: I64(a.n), Rate: cliF64(rate), Samples: m.Samples()})
		}
		f.RateMeter = append(f.RateMeter, rc)
	}
}

func cliHexDecodes(f *cliTextFile) {
	for _, in := range []string{"", "ab", "ABcd", "abc", "a", "zz", "0g", "g0", "abz", "0x12", "é", "00ff7a", "AbCdEf01", "  ", "0 ", "0102030405060708"} {
		b, err := mainpkg.HexDecode(in)
		c := cliHexDecodeCase{In: in, OK: err == nil}
		if err != nil {
			c.Error = err.Error()
		} else {
			c.Out = Hex(b)
		}
		f.HexDecode = append(f.HexDecode, c)
	}
}

func cliResolveTickets(f *cliTextFile) {
	for _, c := range [][3]string{{"f", "s", "e"}, {"", "s", "e"}, {"", "", "e"}, {"", "", ""}, {"f", "", ""}, {" ", "s", "e"}, {"", " ", "e"}} {
		got, err := mainpkg.ResolveTicket(c[0], c[1], c[2])
		rc := cliResolveTicketCase{Flag: c[0], Stored: c[1], Env: c[2], OK: err == nil, Out: got}
		if err != nil {
			rc.Error = err.Error()
		}
		f.ResolveTicket = append(f.ResolveTicket, rc)
	}
}

func cliLogLevels(f *cliTextFile) error {
	for _, v := range []string{"debug", "DEBUG", "Debug", "dEbUg", "info", "INFO", "warn", "WARN", "error", "ERROR", "", "verbose", "WARNING", "warning", "trace", " debug", "debug "} {
		l, err := mainpkg.LogLevel(v)
		if err != nil {
			return err
		}
		f.LogLevel = append(f.LogLevel, cliLogLevelCase{Value: v, Level: int(l)})
	}
	return nil
}

func cliDescribeChanges(f *cliTextFile) {
	entry := func(mode uint64) *fstree.Entry { return &fstree.Entry{Mode: mode} }
	for _, c := range []struct {
		path     string
		kind     worktree.Kind
		old, new *fstree.Entry
	}{
		{"a.txt", worktree.Added, nil, entry(0o100644)},
		{"d", worktree.Added, nil, entry(0o40755)},
		{"d", worktree.Deleted, entry(0o40755), nil},
		{"f.txt", worktree.Deleted, entry(0o100644), nil},
		{"x", worktree.TypeChanged, entry(0o120777), entry(0o40755)},
		{"a.txt", worktree.TypeChanged, entry(0o100644), entry(0o40755)},
		{"link", worktree.TypeChanged, entry(0o40755), entry(0o120777)},
		{"run.sh", worktree.ModeChanged, entry(0o100644), entry(0o100755)},
		{"run.sh", worktree.ModeChanged, entry(0o100644), entry(0o104755)},
		{"d", worktree.ModeChanged, entry(0o40700), entry(0o40755)},
		{"f", worktree.Modified, entry(0o100644), entry(0o100644)},
		{"m", worktree.MetaChanged, entry(0o40755), entry(0o40755)},
		{"p", worktree.TypeChanged, entry(0o10644), entry(0o140755)},
		{"c", worktree.TypeChanged, entry(0o20644), entry(0o60644)},
		{"w", worktree.TypeChanged, entry(0o170644), entry(0o100644)},
		{"z", worktree.TypeChanged, entry(0o644), entry(0o100644)},
		{"k", worktree.Kind(9), entry(0o100644), entry(0o100644)},
		{"sub/dir", worktree.Deleted, entry(0o40755), nil},
		{"é/ü", worktree.Added, nil, entry(0o100600)},
	} {
		ch := worktree.Change{Path: c.path, Kind: c.kind, Old: c.old, New: c.new}
		dc := cliDescribeCase{Path: c.path, Kind: int(c.kind), KindString: c.kind.String(), Describe: mainpkg.DescribeChange(ch), Line: mainpkg.StatusChangeLine(ch)}
		if c.old != nil {
			m := c.old.Mode
			dc.OldMode = &m
		}
		if c.new != nil {
			m := c.new.Mode
			dc.NewMode = &m
		}
		f.DescribeChange = append(f.DescribeChange, dc)
	}
}

// cliRootPlaceholder stands for the working-copy root in filter_paths: the
// process's current directory (os.Getwd), which the vectors pass as root.
const cliRootPlaceholder = "{ROOT}"

func cliFilterPaths(f *cliTextFile) error {
	root, err := os.Getwd()
	if err != nil {
		return err
	}
	changes := []string{"a.txt", "sub", "sub/b.txt", "subx/c"}
	chs := make([]worktree.Change, len(changes))
	for i, p := range changes {
		chs[i] = worktree.Change{Path: p, Kind: worktree.Added, New: &fstree.Entry{Mode: 0o100644}}
	}
	for _, args := range [][]string{
		{"."}, {"sub"}, {"sub/"}, {"./sub/b.txt"}, {"su"}, {".."}, {"../x"}, {"/"}, {"{ROOT}"}, {"{ROOT}/sub"},
		{"{ROOT}/../x"}, {"a.txt", ".."}, {""}, {"sub/../a.txt"}, {"subx"}, {"sub", "subx/c"}, {"./"}, {"a.txt/"},
		{"sub/b.txt/.."}, {"nosuch"},
	} {
		concrete := make([]string, len(args))
		for i, a := range args {
			concrete[i] = strings.ReplaceAll(a, cliRootPlaceholder, root)
		}
		got, err := mainpkg.FilterPaths(root, chs, concrete)
		c := cliFilterPathsCase{Changes: changes, Args: args, OK: err == nil}
		if err != nil {
			c.Error = strings.ReplaceAll(err.Error(), root, cliRootPlaceholder)
		} else {
			c.Out = []string{}
			for _, ch := range got {
				c.Out = append(c.Out, ch.Path)
			}
		}
		f.FilterPaths = append(f.FilterPaths, c)
	}
	return nil
}

func cliNodeStates(f *cliTextFile) {
	for _, c := range [][2]int{{0, 0}, {0, 1}, {1, 1}, {2, 2}, {2, 1}, {1, 0}, {3, 0}} {
		out := mainpkg.NodeState(client.NodeProgress{InFlight: c[0], Awaiting: c[1]})
		f.NodeState = append(f.NodeState, cliNodeStateCase{InFlight: c[0], Awaiting: c[1], Out: out})
	}
}

// cliSlogAttr turns a vector attribute into a slog.Attr of the named kind.
func cliSlogAttr(a cliAttr) (slog.Attr, error) {
	switch a.Kind {
	case "string":
		return slog.String(a.Key, a.Value), nil
	case "int64":
		n, err := strconv.ParseInt(a.Value, 10, 64)
		return slog.Int64(a.Key, n), err
	case "uint64":
		n, err := strconv.ParseUint(a.Value, 10, 64)
		return slog.Uint64(a.Key, n), err
	case "float64":
		x, err := strconv.ParseFloat(a.Value, 64)
		return slog.Float64(a.Key, x), err
	case "bool":
		b, err := strconv.ParseBool(a.Value)
		return slog.Bool(a.Key, b), err
	case "duration":
		n, err := strconv.ParseInt(a.Value, 10, 64)
		return slog.Duration(a.Key, time.Duration(n)), err
	case "error":
		if a.Value == "<nil>" {
			return slog.Any(a.Key, error(nil)), nil
		}
		return slog.Any(a.Key, errors.New(a.Value)), nil
	}
	return slog.Attr{}, fmt.Errorf("tea_handler: unknown attribute kind %q", a.Kind)
}

func cliTeaHandlers(f *cliTextFile) error {
	node := []cliAttr{{"node", "string", "abcd"}}
	cases := []cliTeaCase{
		{Name: "debug-hidden", HandlerLevel: 0, With: node, Level: -4, Msg: "hidden"},
		{Name: "uploaded", HandlerLevel: 0, With: node, Level: 0, Msg: "uploaded", Attrs: []cliAttr{{"objects", "int64", "3"}, {"bytes", "int64", "3145728"}, {"took", "duration", "2000000000"}, {"path", "string", "direct"}}},
		{Name: "upload-failed", HandlerLevel: 0, With: node, Level: 4, Msg: "upload failed", Attrs: []cliAttr{{"err", "string", "timeout: no recent network activity"}}},
		{Name: "quoting", HandlerLevel: 0, With: node, Level: 0, Msg: "q", Attrs: []cliAttr{{"s", "string", `a"b`}, {"t", "string", "x\ty"}, {"e", "string", ""}, {"n", "string", "a=b"}, {"bytesint", "int64", "5"}, {"bytes", "uint64", "5"}}},
		{Name: "error-value", HandlerLevel: 0, With: node, Level: 8, Msg: "boom", Attrs: []cliAttr{{"err", "error", "disk full"}}},
		{Name: "nil-error", HandlerLevel: 0, With: node, Level: 0, Msg: "nil", Attrs: []cliAttr{{"err", "error", "<nil>"}}},
		{Name: "kinds", HandlerLevel: 0, Level: 0, Msg: "kinds", Attrs: []cliAttr{{"f", "float64", "1.5"}, {"g", "float64", "3.7e+06"}, {"b", "bool", "true"}, {"neg", "int64", "-5"}, {"bytes", "int64", "-5"}, {"max", "uint64", "18446744073709551615"}, {"d", "duration", "3723000000000"}, {"ms", "duration", "1500000"}}},
		{Name: "warn-handler-drops-info", HandlerLevel: 4, With: node, Level: 0, Msg: "info"},
		{Name: "warn-handler-keeps-warn", HandlerLevel: 4, With: node, Level: 4, Msg: "w"},
		{Name: "debug-handler", HandlerLevel: -4, Level: -4, Msg: "d", Attrs: []cliAttr{{"k", "string", "v"}}},
		{Name: "plain", HandlerLevel: 0, Level: 0, Msg: "plain"},
		{Name: "two-with", HandlerLevel: 0, With: []cliAttr{{"node", "string", "abcd"}, {"round", "int64", "2"}}, Level: 0, Msg: "round done", Attrs: []cliAttr{{"objects", "int64", "7"}}},
		{Name: "raw-newline-and-unicode", HandlerLevel: 0, Level: 0, Msg: "m", Attrs: []cliAttr{{"nl", "string", "a\nb"}, {"u", "string", "é"}, {"nbsp", "string", "a\u00a0b"}, {"cr", "string", "a\rb"}}},
		{Name: "bytes-as-string", HandlerLevel: 0, Level: 0, Msg: "m", Attrs: []cliAttr{{"bytes", "string", "3 MiB"}}},
		{Name: "custom-level", HandlerLevel: 0, Level: 2, Msg: "custom"},
		{Name: "message-not-quoted", HandlerLevel: 0, Level: 0, Msg: "upload retry \"x\"", Attrs: []cliAttr{{"reason", "string", "busy"}}},
		{Name: "empty-message", HandlerLevel: 0, Level: 0, Msg: ""},
		{Name: "big-bytes", HandlerLevel: 0, Level: 0, Msg: "m", Attrs: []cliAttr{{"bytes", "int64", "9223372036854775807"}, {"bytes", "int64", "1023"}}},
	}
	for i := range cases {
		c := &cases[i]
		if c.With == nil {
			c.With = []cliAttr{}
		}
		if c.Attrs == nil {
			c.Attrs = []cliAttr{}
		}
		log, events := mainpkg.TeaLogger(slog.Level(c.HandlerLevel))
		var with []any
		for _, a := range c.With {
			sa, err := cliSlogAttr(a)
			if err != nil {
				return err
			}
			with = append(with, sa)
		}
		if len(with) > 0 {
			log = log.With(with...)
		}
		var attrs []slog.Attr
		for _, a := range c.Attrs {
			sa, err := cliSlogAttr(a)
			if err != nil {
				return err
			}
			attrs = append(attrs, sa)
		}
		c.Enabled = log.Enabled(context.Background(), slog.Level(c.Level))
		log.LogAttrs(context.Background(), slog.Level(c.Level), c.Msg, attrs...)
		got := events()
		switch len(got) {
		case 0:
			if c.Enabled {
				return fmt.Errorf("tea_handler %s: enabled but no event", c.Name)
			}
		case 1:
			if got[0].Level != slog.Level(c.Level) {
				return fmt.Errorf("tea_handler %s: event level %v", c.Name, got[0].Level)
			}
			c.Text = cliStr(got[0].Text)
		default:
			return fmt.Errorf("tea_handler %s: %d events", c.Name, len(got))
		}
	}
	f.TeaHandler = cases
	return nil
}

func cliFormatEvents(f *cliTextFile) {
	const t0 = 1758198896000000000 // 2025-09-18T12:34:56Z
	for _, c := range []struct {
		at     int64
		offset int
		level  int
		text   string
	}{
		{t0, 0, -4, "debug x"},
		{t0, 0, 0, "uploaded node=abcd objects=3 bytes=3.0 MiB took=2s path=direct"},
		{t0, 0, 2, "custom level"},
		{t0, 0, 4, "upload failed node=abcd err=\"timeout: no recent network activity\""},
		{t0, 0, 6, "warn+2"},
		{t0, 0, 8, "boom node=abcd"},
		{t0, 0, 12, "error+4"},
		{t0, 7200, 0, "connected node=abcd nodes=3 path=direct rtt=3ms"},
		{t0 + 999999999, -12600, 4, "St John's"},
		{-1, 0, 0, "before the epoch"},
		{t0, 19800, 8, ""},
	} {
		at := time.Unix(0, c.at).In(time.FixedZone("", c.offset))
		f.FormatEvent = append(f.FormatEvent, cliFormatEventCase{AtUnixNs: I64(c.at), OffsetSecs: c.offset, Level: c.level, Text: c.text, Out: mainpkg.FormatEvent(at, slog.Level(c.level), c.text)})
	}
}

// cliBlendStart and cliBlendEnd are the default blend of bubbles v2.2.1
// progress (WithDefaultBlend: #5A56E0 to #EE6FF8).
var (
	cliBlendStart = [3]uint8{0x5a, 0x56, 0xe0}
	cliBlendEnd   = [3]uint8{0xee, 0x6f, 0xf8}
)

func cliBlends(f *cliTextFile) {
	for _, c := range []struct {
		steps int
		a, b  [3]uint8
	}{
		{0, cliBlendStart, cliBlendEnd}, {1, cliBlendStart, cliBlendEnd}, {2, cliBlendStart, cliBlendEnd},
		{3, cliBlendStart, cliBlendEnd}, {4, cliBlendStart, cliBlendEnd}, {10, cliBlendStart, cliBlendEnd},
		{40, cliBlendStart, cliBlendEnd}, {110, cliBlendStart, cliBlendEnd}, {150, cliBlendStart, cliBlendEnd},
		{5, [3]uint8{0, 0, 0}, [3]uint8{255, 255, 255}}, {3, [3]uint8{0x60, 0x60, 0x60}, [3]uint8{0x60, 0x60, 0x60}},
		{7, [3]uint8{255, 0, 0}, [3]uint8{0, 0, 255}},
	} {
		out := mainpkg.Blend1D(c.steps, c.a, c.b)
		if out == nil {
			out = [][3]uint8{}
		}
		f.Blend1D = append(f.Blend1D, cliBlendCase{Steps: c.steps, A: c.a, B: c.b, Out: out})
	}
}

func cliBars(f *cliTextFile) {
	for _, c := range []struct {
		width   int
		percent float64
	}{
		{60, 0}, {60, 0.5}, {60, 1}, {20, 1.0 / 3}, {21, 0.5}, {22, 0.5}, {22, 1.0 / 3}, {5, 0.5}, {0, 0.5}, {7, 0.5},
		{6, 1}, {80, 0.999}, {80, 1.5}, {80, -0.1}, {26, 0.004}, {26, 0.996},
	} {
		f.ProgressBar = append(f.ProgressBar, cliBarCase{Width: c.width, Percent: cliF64(c.percent), Out: mainpkg.ProgressBar(c.width, c.percent)})
	}
}

// ---- colorprofile v0.4.3: Bubble Tea's colour profile and downsampling ----

type cliColorProfileCase struct {
	Env []string `json:"env"`
	TTY bool     `json:"tty"`
	Out string   `json:"out"`
}

type cliConvertCase struct {
	RGB  [3]uint8 `json:"rgb"`
	C256 int      `json:"c256"`
	C16  int      `json:"c16"`
}

type cliDownsampleCase struct {
	Profile string `json:"profile"`
	In      string `json:"in"`
	Out     string `json:"out"`
}

// cliArchNeutral reports whether x/ansi Convert256 gives c the same index
// on arm64 and on amd64. On arm64 the gc compiler fuses c*255-35 into one
// FMA, which moves a channel of exactly 115, 155, 195 or 235 into the cube
// level below; amd64 (GOAMD64=v1) rounds the product first. Every other
// colour agrees (checked over all 2^24, port-notes/impl-interop-fixes.md),
// and the vectors only use those, so that they regenerate on either.
func cliArchNeutral(c [3]uint8) bool {
	for _, v := range c {
		switch v {
		case 115, 155, 195, 235:
			return false
		}
	}
	return true
}

func cliColorProfiles(f *cliTextFile) {
	// No case sets TTY_FORCE: with it, Detect would read this machine's
	// terminfo.
	for _, env := range [][]string{
		{},
		{"TERM=dumb"},
		{"TERM=dumb", "COLORTERM=truecolor"},
		{"TERM=dumb", "COLORTERM=truecolor", "CLICOLOR_FORCE=1"},
		{"TERM=dumb", "CLICOLOR_FORCE=1"},
		{"TERM=dumb", "CLICOLOR=1"},
		{"TERM=xterm-256color"},
		{"TERM=xterm-256color", "CLICOLOR=1"},
		{"TERM=xterm-256color", "COLORTERM=yes"},
		{"TERM=xterm-256color", "COLORTERM=truecolor"},
		{"TERM=xterm-256color", "COLORTERM=24BIT"},
		{"TERM=xterm-256color", "NO_COLOR=1"},
		{"TERM=xterm-256color", "NO_COLOR=0"},
		{"TERM=xterm-256color", "NO_COLOR=yes"},
		{"TERM=xterm-256color", "NO_COLOR=1", "CLICOLOR_FORCE=1"},
		{"TERM=xterm-256color", "COLORTERM=truecolor", "NO_COLOR=1"},
		{"TERM=xterm"},
		{"TERM=xterm", "NO_COLOR=1"},
		{"TERM=xterm", "CLICOLOR=1"},
		{"TERM=xterm", "CLICOLOR_FORCE=1"},
		{"TERM=xterm", "GOOGLE_CLOUD_SHELL=true"},
		{"TERM=xterm-16color"},
		{"TERM=xterm-color"},
		{"TERM=xterm-direct"},
		{"TERM=vt100"},
		{"TERM=linux"},
		{"TERM=screen"},
		{"TERM=screen", "COLORTERM=truecolor"},
		{"TERM=screen-256color"},
		{"TERM=tmux", "COLORTERM=truecolor"},
		{"TERM=tmux-256color"},
		{"TERM=alacritty"},
		{"TERM=xterm-kitty"},
		{"TERM=xterm-ghostty"},
		{"TERM=wezterm"},
		{"TERM=foot"},
		{"TERM=st-256color"},
		{"WT_SESSION=1"},
		{"TERM=xterm-256color", "WT_SESSION=1"},
		{"COLORTERM=truecolor"},
		{"TERM="},
		{"TERM=", "CLICOLOR=1"},
		{"TERM"},
		{"TERM=dumb", "TERM=xterm-256color"},
		{"TERM=xterm-256color", "TERM=dumb"},
	} {
		f.ColorProfile = append(f.ColorProfile,
			cliColorProfileCase{Env: env, TTY: true, Out: colorprofile.Env(env).String()},
			cliColorProfileCase{Env: env, TTY: false, Out: colorprofile.Detect(&bytes.Buffer{}, env).String()})
	}
}

func cliConverts(f *cliTextFile) {
	seen := map[[3]uint8]bool{}
	add := func(c [3]uint8) {
		if seen[c] || !cliArchNeutral(c) {
			return
		}
		seen[c] = true
		col := color.RGBA{R: c[0], G: c[1], B: c[2], A: 0xff}
		f.Convert256 = append(f.Convert256, cliConvertCase{RGB: c, C256: int(ansi.Convert256(col)), C16: int(ansi.Convert16(col))})
	}
	// The default bar at widths 20, 60 and 80 (tw = width-5, Blend1D(2*tw)),
	// and its empty run.
	for _, w := range []int{20, 60, 80} {
		for _, c := range mainpkg.Blend1D(2*(w-5), cliBlendStart, cliBlendEnd) {
			add(c)
		}
	}
	add([3]uint8{0x60, 0x60, 0x60})
	// Around the cube levels (0, 95, 135, 175, 215, 255), the grey ramp and
	// the corners.
	for _, v := range []uint8{0, 1, 7, 8, 47, 48, 94, 95, 96, 114, 116, 134, 135, 136, 154, 156, 174, 175, 176, 194, 196, 214, 215, 216, 234, 236, 238, 239, 254, 255} {
		add([3]uint8{v, v, v})
		add([3]uint8{v, 0, 0})
		add([3]uint8{0, v, 0})
		add([3]uint8{0, 0, v})
		add([3]uint8{v, 255 - v, 128})
	}
	// A fixed xorshift sample.
	s := uint64(0x9e3779b97f4a7c15)
	for range 400 {
		s ^= s << 13
		s ^= s >> 7
		s ^= s << 17
		add([3]uint8{uint8(s), uint8(s >> 8), uint8(s >> 16)})
	}
}

func cliDownsamples(f *cliTextFile) error {
	// Every 24-bit colour in these inputs is arch-neutral (cliArchNeutral),
	// and each colour type of ReadStyleColor is valid: Writer panics on an
	// invalid one under ANSI and ANSI256.
	inputs := []string{
		mainpkg.ProgressBar(60, 0),
		"\x1b[1mpush trees/demo\x1b[m  \x1b[2melapsed 2s\x1b[m",
		"\x1b[33m12:34:56  upload retry node=abcd reason=busy\x1b[m",
		"\x1b[31mfailed: context canceled\x1b[m",
		"\x1b[32mdone\x1b[m",
		"\x1b[1mt\x1b[m \x1b[38;2;90;86;224;48;2;92;86;225m▌\x1b[m\x1b[38;2;96;96;96m░\x1b[m \x1b[32mdone\x1b[m \x1b[0;1;38:2::1:2:3m x\x1b[?25l\x1b[2A",
		"\x1b[38;5;200;48;5;17m x \x1b[91;101;39;49m y \x1b[58;2;255;0;0;59m z \x1b[4:3m w\x1b[m",
		"\x1b[38:2:10:20:30;48:5:232m a \x1b[38:3::11:21:31m b \x1b[48:4::10:20:30:40m c \x1b[38:6::1:2:3:128m d",
		"\x1b[38;1m t \x1b[;m u \x1b[m v \x1b[?1m w \x1b[1 m x",
		"a\tb\x1b]8;;http://x\x07c\x1b]0;t\x1b\\d\x1b[?25l\x1b[2Ae\x1b7f",
	}
	// Full bars have channels of 155 and 235, so they are only downsampled
	// where no colour is converted.
	bars := []string{mainpkg.ProgressBar(60, 0.5), mainpkg.ProgressBar(20, 1)}
	for _, p := range []colorprofile.Profile{colorprofile.NoTTY, colorprofile.ASCII, colorprofile.ANSI, colorprofile.ANSI256, colorprofile.TrueColor} {
		in := inputs
		if p != colorprofile.ANSI && p != colorprofile.ANSI256 {
			in = append(append([]string{}, inputs...), bars...)
		}
		for _, s := range in {
			var buf bytes.Buffer
			w := &colorprofile.Writer{Forward: &buf, Profile: p}
			if _, err := w.WriteString(s); err != nil {
				return err
			}
			f.Downsample = append(f.Downsample, cliDownsampleCase{Profile: p.String(), In: s, Out: buf.String()})
		}
	}
	return nil
}

// cliUIRun records a uiModel scenario while driving it.
type cliUIRun struct {
	ui   *mainpkg.UI
	sc   cliUIScenario
	zone *time.Location
	err  error
}

func cliNewUIRun(name, title string, offsetSecs int) *cliUIRun {
	return &cliUIRun{ui: mainpkg.NewUI(title), sc: cliUIScenario{Name: name, Title: title, OffsetSecs: offsetSecs}, zone: time.FixedZone("", offsetSecs)}
}

func (r *cliUIRun) step(s cliUIStep, quit bool) {
	cancelled := r.ui.Cancels() > 0
	s.Quit, s.Cancelled = &quit, &cancelled
	r.sc.Steps = append(r.sc.Steps, s)
}

func (r *cliUIRun) set(rep client.ProgressReport) {
	r.ui.Set(rep)
	r.sc.Steps = append(r.sc.Steps, cliUIStep{Op: "set", Report: cliReportJSON(rep)})
}

func (r *cliUIRun) resize(w int) {
	q := r.ui.Resize(w)
	r.step(cliUIStep{Op: "resize", Width: &w}, q)
}

func (r *cliUIRun) tick(d time.Duration) {
	q := r.ui.Tick(d)
	r.step(cliUIStep{Op: "tick", OffsetNs: cliI64p(int64(d))}, q)
}

func (r *cliUIRun) event(atUnixNs int64, level slog.Level, text string) {
	q := r.ui.Event(time.Unix(0, atUnixNs).In(r.zone), level, text)
	l := int(level)
	r.step(cliUIStep{Op: "event", AtUnixNs: cliI64p(atUnixNs), Level: &l, Text: cliStr(text)}, q)
}

func (r *cliUIRun) ctrlC() {
	q, err := r.ui.CtrlC()
	if err != nil && r.err == nil {
		r.err = fmt.Errorf("ui_model %s: %w", r.sc.Name, err)
	}
	r.step(cliUIStep{Op: "ctrl_c"}, q)
}

func (r *cliUIRun) done(err error) {
	q := r.ui.Done(err)
	s := cliUIStep{Op: "done"}
	if err != nil {
		s.Error = cliStr(err.Error())
	}
	r.step(s, q)
}

func (r *cliUIRun) view() {
	r.sc.Steps = append(r.sc.Steps, cliUIStep{Op: "view", Out: cliStr(r.ui.View())})
}

// viewContains records substrings for a view whose rates depend on the wall
// clock (done after ticks with progress between them).
func (r *cliUIRun) viewContains(subs ...string) {
	v := r.ui.View()
	for _, s := range subs {
		if !strings.Contains(v, s) && r.err == nil {
			r.err = fmt.Errorf("ui_model %s: the view lacks %q:\n%s", r.sc.Name, s, v)
		}
	}
	r.sc.Steps = append(r.sc.Steps, cliUIStep{Op: "view", Contains: subs})
}

func cliUIScenarios(f *cliTextFile) error {
	const t0 = 1758198896000000000 // 2025-09-18T12:34:56Z
	s := time.Second
	ms := time.Millisecond
	idAbcd := view.NodeID{0xab, 0xcd}
	idEf01 := view.NodeID{0xef, 0x01}
	var runs []*cliUIRun

	// tui_test.go TestUIModel, with every deterministic view recorded.
	r := cliNewUIRun("tui-test-go", "push demo", 0)
	r.set(client.ProgressReport{Objects: 5, TotalObjects: 10, Bytes: 512, TotalBytes: 1024,
		Nodes: []client.NodeProgress{{ID: idAbcd, Direct: true, RTT: 3 * ms, InFlight: 1, Bytes: 512}}})
	r.resize(100)
	r.tick(0)
	r.view()
	r.set(client.ProgressReport{Objects: 5, TotalObjects: 10, Bytes: 1024, TotalBytes: 2048,
		Nodes: []client.NodeProgress{{ID: idAbcd, Direct: true, RTT: 3 * ms, InFlight: 1, Bytes: 1024}}})
	r.tick(s)
	r.view()
	r.event(t0, slog.LevelWarn, "upload retry node=abcd reason=busy")
	r.view()
	r.set(client.ProgressReport{Objects: 5, TotalObjects: 10, Bytes: 2048, TotalBytes: 2048,
		Nodes: []client.NodeProgress{{ID: idAbcd, Direct: true, InFlight: 1, Awaiting: 1, Bytes: 2048}}})
	r.tick(2 * s)
	r.view()
	r.ctrlC()
	r.view()
	r.ctrlC()
	r.done(errors.New("context canceled"))
	r.viewContains("push demo", "failed: context canceled", "cancelling", "waiting for ack")
	runs = append(runs, r)

	// port-notes/cli.md §3.6: two nodes, two events, default width.
	r = cliNewUIRun("two-nodes", "push trees/demo", 7200)
	two := func(bytes int64) client.ProgressReport {
		return client.ProgressReport{Objects: 5, TotalObjects: 10, Bytes: bytes, TotalBytes: 2048, Nodes: []client.NodeProgress{
			{ID: idAbcd, Direct: true, RTT: 3 * ms, InFlight: 2, Awaiting: 2, Bytes: bytes},
			{ID: idEf01, Direct: false, RTT: 45 * ms, InFlight: 1, Bytes: 0},
		}}
	}
	r.set(two(512))
	r.tick(0)
	r.set(two(1024))
	r.tick(1200 * ms)
	r.event(t0, slog.LevelWarn, "upload retry node=abcd reason=busy")
	r.event(t0, slog.LevelInfo, "connected node=abcd nodes=3 path=direct rtt=3ms")
	r.view()
	r.resize(100)
	r.view()
	runs = append(runs, r)

	// port-notes/cli.md §3.6: empty report, width 10, done without error.
	r = cliNewUIRun("empty-done-width-10", "pull x", 0)
	r.resize(10)
	r.done(nil)
	r.view()
	runs = append(runs, r)

	// port-notes/cli.md §5.3: the bar at 100 %.
	r = cliNewUIRun("bar-full", "pull trees/demo", 0)
	r.set(client.ProgressReport{Objects: 3, TotalObjects: 3, Bytes: 4096, TotalBytes: 4096})
	r.tick(0)
	r.view()
	runs = append(runs, r)

	// port-notes/cli.md §5.3: 0 < fw < tw with an odd tw (width 26: bar 22, tw 17).
	r = cliNewUIRun("odd-bar-third", "fetch trees/demo", 0)
	r.resize(26)
	r.set(client.ProgressReport{Objects: 1, TotalObjects: 3})
	r.tick(0)
	r.view()
	runs = append(runs, r)

	// Done after ticks without progress between them: every rate is 0.
	r = cliNewUIRun("done-after-steady-ticks", "push trees/steady", 0)
	steady := client.ProgressReport{Objects: 4, TotalObjects: 10, Bytes: 2048, TotalBytes: 8192,
		Nodes: []client.NodeProgress{{ID: idAbcd, Direct: true, RTT: 2 * ms, Bytes: 2048}}}
	r.set(steady)
	r.tick(0)
	r.tick(s)
	r.view()
	r.done(nil)
	r.view()
	runs = append(runs, r)

	// Twelve events kept of fourteen, every level style.
	r = cliNewUIRun("events-scroll", "pull trees/events", 3600)
	levels := []slog.Level{slog.LevelDebug, slog.LevelInfo, slog.LevelWarn, slog.LevelError, slog.Level(2), slog.Level(6), slog.Level(12)}
	for i := 0; i < 14; i++ {
		r.event(t0+int64(i)*int64(s), levels[i%len(levels)], fmt.Sprintf("event %d", i))
	}
	r.view()
	runs = append(runs, r)

	// Node rows: RTT forms, idle, relay, large byte counts.
	r = cliNewUIRun("node-rows", "push trees/rows", 0)
	r.set(client.ProgressReport{Objects: 9, TotalObjects: 9, Bytes: 5 << 30, TotalBytes: 6 << 30, Nodes: []client.NodeProgress{
		{ID: view.NodeID{0x01}, Direct: true, RTT: 250 * time.Microsecond, InFlight: 0, Bytes: 5 << 30},
		{ID: view.NodeID{0x02}, Direct: false, RTT: 0, InFlight: 3, Awaiting: 1, Bytes: 1023},
		{ID: view.NodeID{0x03}, Direct: true, RTT: 1234 * ms, InFlight: 1, Awaiting: 1, Bytes: 1 << 20},
		{ID: view.NodeID{0x04}, Direct: false, RTT: 999 * time.Microsecond, InFlight: 4, Awaiting: 0, Bytes: 0},
	}})
	r.tick(0)
	r.view()
	runs = append(runs, r)

	// Done with an error and no ticks.
	r = cliNewUIRun("done-error-no-ticks", "clone trees/x", 0)
	r.done(errors.New("client: no bootstrap node answered: dial 0707070707070707: timeout"))
	r.view()
	runs = append(runs, r)

	// Width clamps: 200 → bar 80, 24 → 20, 25 → 21.
	r = cliNewUIRun("resize-bounds", "init trees/bounds", 0)
	r.set(client.ProgressReport{Objects: 1, TotalObjects: 2})
	r.resize(200)
	r.tick(0)
	r.view()
	r.resize(24)
	r.view()
	r.resize(25)
	r.view()
	runs = append(runs, r)

	for _, run := range runs {
		if run.err != nil {
			return run.err
		}
		f.UIModel = append(f.UIModel, run.sc)
	}
	return nil
}

// cliCheckText compares a few generated values with the verified texts of
// port-notes/cli.md, to catch a harness that drives the copies wrongly.
func cliCheckText(f *cliTextFile) error {
	var problems []string
	expect := func(what, got, want string) {
		if got != want {
			problems = append(problems, fmt.Sprintf("%s: got %q, verified %q", what, got, want))
		}
	}
	for _, c := range f.HumanBytes {
		if c.N == 1048575 {
			expect("human_bytes 1048575", c.Out, "1024.0 KiB")
		}
	}
	lines := []string{
		"3/4 objects  0 B/s", "0/0 objects  512 B  100 B/s", "5/10 objects  1.0 KiB / 2.0 KiB  512 B/s  eta 2s",
		"5/10 objects  1.0 GiB / 5.0 GiB  3.5 MiB/s  eta 19m21s", "5/10 objects  2.9 KiB / 1000 B  10 B/s",
		"1/2 objects  10 B / 100 B  0 B/s  eta 3m0s", "1/2 objects  10 B / 95.4 MiB  1 B/s  eta 27777h46m30s",
	}
	for i, want := range lines {
		expect(fmt.Sprintf("status_line %d", i), f.StatusLine[i].Out, want)
	}
	if last := f.RateMeter[0].Adds[len(f.RateMeter[0].Adds)-1]; last.Rate != cliF64(816.6666666666666) || last.Samples != 2 {
		problems = append(problems, fmt.Sprintf("rate_meter cli-md-5.4 last add: %v (%d samples)", float64(last.Rate), last.Samples))
	}
	for _, c := range f.TeaHandler {
		if c.Name == "quoting" && c.Text != nil {
			expect("tea_handler quoting", *c.Text, `q node=abcd s=a"b t="x\ty" e= n=a=b bytesint=5 bytes=5`)
		}
	}
	for _, sc := range f.UIModel {
		if sc.Name == "empty-done-width-10" {
			last := sc.Steps[len(sc.Steps)-1]
			if last.Out != nil {
				expect("ui_model empty-done-width-10", *last.Out, "\x1b[1mpull x\x1b[m  \x1b[2melapsed 0s\x1b[m\n\x1b[38;2;96;96;96m░░░░░░░░░░░░░░░\x1b[m   0%\n0/0 objects  0 B/s\n\x1b[32mdone\x1b[m\n")
			}
		}
	}
	if len(problems) > 0 {
		return fmt.Errorf("cli text vectors disagree with port-notes/cli.md:\n%s", strings.Join(problems, "\n"))
	}
	return nil
}
