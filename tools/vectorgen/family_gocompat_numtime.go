package main

// Family gocompat-numtime: go1.26.5 strconv parsing and FormatFloat('g', -1, 64), time.Duration
// String/Round, time.ParseDuration, the time layouts dstore prints (RFC3339 in a zone, RFC3339Nano
// UTC, the slog TextHandler time, "15:04:05", the log package header) and Parse(RFC3339Nano).
// Rust: crates/gocompat/src/{strconv,time}.rs, tests/golden_tests/gocompat_numtime.rs.
// Schema: tools/vectorgen/docs/gocompat-b.md.

import (
	"bytes"
	"context"
	"fmt"
	"go/ast"
	"go/build"
	"go/parser"
	"go/printer"
	"go/token"
	"log/slog"
	"math"
	"path/filepath"
	"strconv"
	"strings"
	"time"
	"unicode"
	"unicode/utf8"
)

func init() {
	register("gocompat-numtime", []string{"gocompat/numtime.json"}, genGocompatNumtime)
}

type ntBool struct {
	In    string `json:"in"`
	Value bool   `json:"value"`
	Err   string `json:"err"`
}

type ntInt struct {
	In      string `json:"in"`
	Base    int    `json:"base"`
	BitSize int    `json:"bit_size"`
	Value   I64    `json:"value"`
	Err     string `json:"err"`
}

type ntUint struct {
	In      string `json:"in"`
	Base    int    `json:"base"`
	BitSize int    `json:"bit_size"`
	Value   U64    `json:"value"`
	Err     string `json:"err"`
}

type ntFloat struct {
	In   string `json:"in"`
	Bits U64    `json:"bits"`
	Err  string `json:"err"`
}

type ntFormatFloat struct {
	Bits U64    `json:"bits"`
	Out  string `json:"out"`
}

type ntDurString struct {
	Ns  I64    `json:"ns"`
	Out string `json:"out"`
}

type ntDurRound struct {
	Ns  I64 `json:"ns"`
	M   I64 `json:"m"`
	Out I64 `json:"out"`
}

type ntParseDur struct {
	In  string `json:"in"`
	Ns  I64    `json:"ns"`
	Err string `json:"err"`
}

type ntTimeZoned struct {
	UnixSecs   I64    `json:"unix_secs"`
	Nanos      int    `json:"nanos"`
	OffsetSecs int    `json:"offset_secs"`
	Out        string `json:"out"`
}

type ntTimeUTC struct {
	UnixSecs I64    `json:"unix_secs"`
	Nanos    int    `json:"nanos"`
	Out      string `json:"out"`
}

type ntParseTime struct {
	In       string `json:"in"`
	UnixSecs I64    `json:"unix_secs"`
	Nanos    int    `json:"nanos"`
	Err      string `json:"err"`
}

type ntFile struct {
	ParseBool        []ntBool        `json:"parse_bool"`
	ParseInt         []ntInt         `json:"parse_int"`
	ParseUint        []ntUint        `json:"parse_uint"`
	ParseFloat       []ntFloat       `json:"parse_float"`
	FormatFloatG     []ntFormatFloat `json:"format_float_g"`
	DurationString   []ntDurString   `json:"duration_string"`
	DurationRound    []ntDurRound    `json:"duration_round"`
	ParseDuration    []ntParseDur    `json:"parse_duration"`
	RFC3339Local     []ntTimeZoned   `json:"rfc3339_local"`
	RFC3339NanoUTC   []ntTimeUTC     `json:"rfc3339nano_utc"`
	SlogTime         []ntTimeZoned   `json:"slog_time"`
	Clock            []ntTimeZoned   `json:"clock"`
	LogStd           []ntTimeZoned   `json:"log_std"`
	ParseRFC3339Nano []ntParseTime   `json:"parse_rfc3339nano"`
}

// ntRNG is a splitmix64 stream (util.go splitmixNext).
type ntRNG struct{ s uint64 }

func (r *ntRNG) next() uint64 { return splitmixNext(&r.s) }

// intn returns a value in [0, n).
func (r *ntRNG) intn(n int) int { return int(r.next() % uint64(n)) }

func ntErr(err error) string {
	if err == nil {
		return ""
	}
	return ntUTF8(err.Error())
}

// ntUTF8 panics on a text that is not valid UTF-8: the Rust APIs take &str and return String, and
// json.Marshal would silently replace the invalid bytes with U+FFFD (log.itoa writes such a byte for
// years at or below -49000, which the fixed instants avoid).
func ntUTF8(s string) string {
	if !utf8.ValidString(s) {
		panic(fmt.Sprintf("gocompat-numtime: %q is not valid UTF-8", s))
	}
	return s
}

func genGocompatNumtime(out string) error {
	if err := ntCheckLogCopy(); err != nil {
		return err
	}
	var f ntFile
	f.ParseBool = ntGenParseBool()
	f.ParseInt, f.ParseUint = ntGenParseInts()
	f.ParseFloat = ntGenParseFloat()
	f.FormatFloatG = ntGenFormatFloat()
	f.DurationString, f.DurationRound = ntGenDurations()
	f.ParseDuration = ntGenParseDuration()
	f.RFC3339Local, f.RFC3339NanoUTC, f.SlogTime, f.Clock, f.LogStd = ntGenLayouts()
	f.ParseRFC3339Nano = ntGenParseRFC3339Nano()
	return writeJSON(filepath.Join(out, "gocompat", "numtime.json"), f)
}

func ntGenParseBool() []ntBool {
	ins := []string{"", "asdf", "0", "f", "F", "FALSE", "false", "False", "1", "t", "T", "TRUE", "true",
		"True", "tRUE", "fALSE", "yes", "no", "on", "off", " true", "true ", "01", "10", "2", "-1", "é", "\x00"}
	var cases []ntBool
	for _, in := range ins {
		v, err := strconv.ParseBool(ntUTF8(in))
		cases = append(cases, ntBool{In: in, Value: v, Err: ntErr(err)})
	}
	return cases
}

// ntIntInputs are parsed in every base of ntBases with bit size 64.
var ntIntInputs = []string{
	// internal/strconv atoi_test.go inputs
	"", "0", "-0", "+0", "1", "-1", "+1", "12345", "-12345", "012345", "-012345", "12345x", "-12345x",
	"98765432100", "-98765432100", "9223372036854775807", "-9223372036854775807", "9223372036854775808",
	"-9223372036854775808", "9223372036854775809", "-9223372036854775809", "18446744073709551615",
	"18446744073709551616", "18446744073709551620", "1_2_3_4_5", "_12345", "1__2345", "12345_", "-1_2_3_4_5",
	"-_12345", "123%45", "0x", "0X", "0x12345", "0X12345", "-0X12345", "0xabcdefg123", "123456789abc",
	"0xFFFFFFFFFFFFFFFF", "0x10000000000000000", "01777777777777777777777", "01777777777777777777778",
	"02000000000000000000000", "0200000000000000000000", "0b", "0B", "0b101", "0B101", "0o", "0O", "0o377",
	"0O377", "0x_1_2_3_4_5", "-0x_1_2_3_4_5", "_0x12345", "-_0x12345", "_-0x12345", "0x__12345", "0x1__2345",
	"0x1234__5", "0x12345_", "1234__5", "0_1_2_3_4_5", "-0_1_2_3_4_5", "_012345", "-_012345", "_-012345",
	"0__12345", "01234__5", "012345_", "0o_1_2_3_4_5", "_0o12345", "0o__12345", "0o1234__5", "0o12345_",
	"0b_1_0_1", "_0b101", "0b__101", "0b1__01", "0b10__1", "0b101_", "1_0_1", "10_1", "101_", "g", "holycow",
	"1010", "7fffffffffffffff", "-123456789abcdef", "+0xf", "-0xf", "0x+f", "0x-f",
	// more
	"+", "-", "++1", "--1", "+-1", "-+1", " 1", "1 ", "1\t", "٣", "1e3", "1.0", "0xg", "0XFF", "0B101",
	"0O17", "08", "09", "0_8", "0_9", "DEADBEEF", "deadbeef", "z", "Z", "_", "__", "0_", "0x_", "0b_", "0o_",
	"0x_ff", "0xf_f", "1_000", "1__000", "-_1", "_-1", "99999999999999999999999x", "x99999999999999999999999",
	"0x_ffff_ffff_ffff_ffff", "00", "0x0", "-0x0", "0b2", "0o8", "0xG", "é", "\x00", "0x8000000000000000",
	"-0x8000000000000000", "-0x8000000000000001", "0_x1", "0x_", "0_0", "0x0_0", "-0_", "1_", "_1",
}

var ntBases = []int{0, 10, 16}

// ntEdge is a bit-size boundary input and a base it is parsed in.
type ntEdge struct {
	in   string
	base int
}

// ntEdgeInputs are the boundaries of every bit size of ntBitSizes, each parsed in the bases that suit
// its form and in every bit size.
func ntEdgeInputs() []ntEdge {
	var ins []ntEdge
	for _, bits := range []uint{8, 16, 32, 64} {
		maxS := uint64(1)<<(bits-1) - 1
		maxU := uint64(math.MaxUint64)
		if bits < 64 {
			maxU = uint64(1)<<bits - 1
		}
		for _, v := range []uint64{maxS, maxS + 1, maxU} {
			vs := []uint64{v}
			if v < math.MaxUint64 {
				vs = append(vs, v+1)
			}
			for _, x := range vs {
				dec := strconv.FormatUint(x, 10)
				hx := strconv.FormatUint(x, 16)
				ins = append(ins, ntEdge{dec, 0}, ntEdge{dec, 10}, ntEdge{"-" + dec, 0}, ntEdge{"-" + dec, 10},
					ntEdge{hx, 16}, ntEdge{"-" + hx, 16}, ntEdge{"0x" + hx, 0}, ntEdge{"-0x" + hx, 0})
			}
			ins = append(ins, ntEdge{"0" + strconv.FormatUint(v, 8), 0}, ntEdge{"0b" + strconv.FormatUint(v, 2), 0})
		}
	}
	return ins
}

var ntBitSizes = []int{8, 16, 32, 64}

func ntGenParseInts() ([]ntInt, []ntUint) {
	type key struct {
		in         string
		base, bits int
	}
	seen := map[key]bool{}
	var ints []ntInt
	var uints []ntUint
	add := func(in string, base, bits int) {
		k := key{in, base, bits}
		if seen[k] {
			return
		}
		seen[k] = true
		i, err := strconv.ParseInt(ntUTF8(in), base, bits)
		ints = append(ints, ntInt{In: in, Base: base, BitSize: bits, Value: I64(i), Err: ntErr(err)})
		u, err := strconv.ParseUint(in, base, bits)
		uints = append(uints, ntUint{In: in, Base: base, BitSize: bits, Value: U64(u), Err: ntErr(err)})
	}
	for _, in := range ntIntInputs {
		for _, base := range ntBases {
			add(in, base, 64)
		}
	}
	for _, e := range ntEdgeInputs() {
		for _, bits := range ntBitSizes {
			add(e.in, e.base, bits)
		}
	}
	// Random values in base 10 and 16, with signs and underscores.
	r := &ntRNG{s: 0x6e74_696e_7473}
	for i := 0; i < 120; i++ {
		v := r.next() >> uint(r.intn(64))
		var s string
		switch r.intn(4) {
		case 0:
			s = strconv.FormatUint(v, 10)
		case 1:
			s = "0x" + strconv.FormatUint(v, 16)
		case 2:
			s = strconv.FormatUint(v, 16)
		default:
			s = "0" + strconv.FormatUint(v, 8)
		}
		if r.intn(3) == 0 && len(s) > 2 {
			p := 1 + r.intn(len(s)-1)
			s = s[:p] + "_" + s[p:]
		}
		if r.intn(3) == 0 {
			s = "-" + s
		}
		add(s, ntBases[r.intn(len(ntBases))], ntBitSizes[r.intn(len(ntBitSizes))])
	}
	return ints, uints
}

func ntGenParseFloat() []ntFloat {
	ins := []string{
		// internal/strconv atof_test.go atoftests inputs
		"", "1", "+1", "1x", "1.1.", "1e23", "1E23", "100000000000000000000000", "1e-100", "123456700",
		"99999999999999974834176", "100000000000000000000001", "100000000000000008388608",
		"100000000000000016777215", "100000000000000016777216", "-1", "-0.1", "-0", "1e-20", "625e-3",
		"0x1p0", "0x1p1", "0x1p-1", "0x1ep-1", "-0x1ep-1", "-0x1_ep-1", "0x1p-200", "0x1p200", "0x1fFe2.p0",
		"0x1fFe2.P0", "-0x2p3", "0x0.fp4", "0x0.fp0", "0x1e2", "1p2", "0", "0e0", "-0e0", "+0e0", "0e-0",
		"-0e-0", "+0e-0", "0e+0", "-0e+0", "+0e+0", "0e+01234567890123456789", "0.00e-01234567890123456789",
		"-0e+01234567890123456789", "-0.00e-01234567890123456789", "0x0p+01234567890123456789",
		"0x0.00p-01234567890123456789", "-0x0p+01234567890123456789", "-0x0.00p-01234567890123456789",
		"0e291", "0e292", "0e347", "0e348", "-0e291", "-0e292", "-0e347", "-0e348", "0x0p126", "0x0p127",
		"0x0p128", "0x0p129", "0x0p130", "0x0p1022", "0x0p1023", "0x0p1024", "0x0p1025", "0x0p1026",
		"-0x0p126", "-0x0p1026", "nan", "NaN", "NAN", "inf", "-Inf", "+INF", "-Infinity", "+INFINITY",
		"Infinity", "1.7976931348623157e308", "-1.7976931348623157e308", "0x1.fffffffffffffp1023",
		"-0x1.fffffffffffffp1023", "0x1fffffffffffffp+971", "-0x1fffffffffffffp+971", "0x.1fffffffffffffp1027",
		"-0x.1fffffffffffffp1027", "1.7976931348623159e308", "-1.7976931348623159e308", "0x1p1024",
		"-0x1p1024", "0x2p1023", "-0x2p1023", "0x.1p1028", "-0x.1p1028", "0x.2p1027", "-0x.2p1027",
		"1.7976931348623158e308", "-1.7976931348623158e308", "0x1.fffffffffffff7fffp1023",
		"-0x1.fffffffffffff7fffp1023", "1.797693134862315808e308", "-1.797693134862315808e308",
		"0x1.fffffffffffff8p1023", "-0x1.fffffffffffff8p1023", "0x1fffffffffffff.8p+971",
		"-0x1fffffffffffff8p+967", "0x.1fffffffffffff8p1027", "-0x.1fffffffffffff9p1027", "1e308", "2e308",
		"1e309", "0x1p1025", "1e310", "-1e310", "1e400", "-1e400", "1e400000", "-1e400000", "0x1p1030",
		"0x1p2000", "0x1p2000000000", "-0x1p1030", "-0x1p2000", "-0x1p2000000000", "1e-305", "1e-306",
		"1e-307", "1e-308", "1e-309", "1e-310", "1e-322", "5e-324", "4e-324", "3e-324", "2e-324", "1e-350",
		"1e-400000", "0x2.00000000000000p-1010", "0x1.fffffffffffff0p-1010", "0x1.fffffffffffff7p-1010",
		"0x1.fffffffffffff8p-1010", "0x1.fffffffffffff9p-1010", "0x2.00000000000000p-1022",
		"0x1.fffffffffffff0p-1022", "0x1.fffffffffffff7p-1022", "0x1.fffffffffffff8p-1022",
		"0x1.fffffffffffff9p-1022", "0x1.00000000000000p-1022", "0x0.fffffffffffff0p-1022",
		"0x0.ffffffffffffe0p-1022", "0x0.ffffffffffffe7p-1022", "0x1.ffffffffffffe8p-1023",
		"0x1.ffffffffffffe9p-1023", "0x0.00000003fffff0p-1022", "0x0.00000003456780p-1022",
		"0x0.00000003456787p-1022", "0x0.00000003456788p-1022", "0x0.00000003456790p-1022",
		"0x0.00000003456789p-1022", "0x0.0000000345678800000000000000000000000001p-1022",
		"0x0.000000000000f0p-1022", "0x0.00000000000060p-1022", "0x0.00000000000058p-1022",
		"0x0.00000000000057p-1022", "0x0.00000000000050p-1022", "0x0.00000000000010p-1022",
		"0x0.000000000000081p-1022", "0x0.00000000000008p-1022", "0x0.00000000000007fp-1022",
		"1e-4294967296", "1e+4294967296", "1e-18446744073709551616", "1e+18446744073709551616",
		"0x1p-4294967296", "0x1p+4294967296", "0x1p-18446744073709551616", "0x1p+18446744073709551616",
		"1e", "1e-", ".e-1", "1\x00.2", "0x", "0x.", "0x1", "0x.1", "0x1p", "0x.1p", "0x1p+", "0x.1p+", "0x1p-",
		"0x.1p-", "0x1p+2", "0x.1p+2", "0x1p-2", "0x.1p-2", "2.2250738585072012e-308",
		"2.2250738585072011e-308", "4.630813248087435e+307", "22.222222222222222",
		"2." + strings.Repeat("2", 4000) + "e+1", "0x1.1111111111111p222", "0x2.2222222222222p221",
		"0x2." + strings.Repeat("2", 4000) + "p221", "1.00000000000000011102230246251565404236316680908203125",
		"0x1.00000000000008p0", "1.00000000000000011102230246251565404236316680908203124",
		"0x1.00000000000007Fp0", "1.00000000000000011102230246251565404236316680908203126",
		"0x1.000000000000081p0", "0x1.00000000000009p0",
		"1.00000000000000011102230246251565404236316680908203125" + strings.Repeat("0", 10000) + "1",
		"0x1.00000000000008" + strings.Repeat("0", 10000) + "1p0",
		"1.00000000000000033306690738754696212708950042724609375", "0x1.00000000000018p0",
		"1090544144181609348671888949248", "1090544144181609348835077142190", "1_23.50_0_0e+1_2",
		"-_123.5e+12", "+_123.5e+12", "_123.5e+12", "1__23.5e+12", "123_.5e+12", "123._5e+12", "123.5_e+12",
		"123.5__0e+12", "123.5e_+12", "123.5e+_12", "123.5e_-12", "123.5e-_12", "123.5e+1__2", "123.5e+12_",
		"0x_1_2.3_4_5p+1_2", "-_0x12.345p+12", "+_0x12.345p+12", "_0x12.345p+12", "0x__12.345p+12",
		"0x1__2.345p+12", "0x12_.345p+12", "0x12._345p+12", "0x12.3__45p+12", "0x12.345_p+12",
		"0x12.345p_+12", "0x12.345p+_12", "0x12.345p_-12", "0x12.345p-_12", "0x12.345p+1__2", "0x12.345p+12_",
		"1e100x", "1e1000x",
		// more
		"infinity", "infinit", "infin", "+inf", "-nan", "+nan", "nan(1)", "iNf", "InFiNiTy", "infx", "nanx",
		".5", "5.", ".", "-.5", "+.e1", "-", "+", "e5", "0x1.8p-1074", "0x1p-1074", "0x1p-1075", "0x1.8p-1075",
		"4.9406564584124654e-324", "2.4703282292062327e-324", "2.4703282292062328e-324", " 1", "1 ", "1_", "_1",
		"0x_1p0", "0x1p_0", "1e+_5", "1e_5", "0.1", "0.3", "1e22", "1e23", "123456789012345678901234567890",
		"9007199254740993", "9007199254740992.5", "0.000001", "1e-7", "1.5", "3.7e+06", "１",
		// Many leading fraction zeros cancelled by the exponent.
		"0." + strings.Repeat("0", 2000) + "1e+2001", "1e+0000000000000000400", "1e-1000000",
		// The slow path keeps 800 digits and sets dp from them: a 905-digit integer scaled back down.
		"100000000000000011102230246251565404236316680908203125" + strings.Repeat("0", 850) + "1e-904",
		"1" + strings.Repeat("0", 900) + "e-900",
		"2" + strings.Repeat("3", 900) + "e-890",
	}
	r := &ntRNG{s: 0x6e74_666c_6f61_74}
	for i := 0; i < 300; i++ {
		x := math.Float64frombits(r.next())
		switch r.intn(4) {
		case 0:
			ins = append(ins, strconv.FormatFloat(x, 'g', -1, 64))
		case 1:
			ins = append(ins, strconv.FormatFloat(x, 'e', r.intn(25), 64))
		case 2:
			ins = append(ins, strconv.FormatFloat(x, 'x', -1, 64))
		default:
			ins = append(ins, strconv.FormatFloat(x, 'x', r.intn(16), 64))
		}
	}
	for i := 0; i < 300; i++ {
		// Random decimal strings: digits, an optional point, an exponent near the float64 range edges.
		var b strings.Builder
		if r.intn(4) == 0 {
			b.WriteByte("+-"[r.intn(2)])
		}
		nd := 1 + r.intn(40)
		dot := r.intn(nd + 2)
		for j := 0; j < nd; j++ {
			if j == dot {
				b.WriteByte('.')
			}
			b.WriteByte(byte('0' + r.intn(10)))
		}
		if r.intn(3) != 0 {
			b.WriteString("e")
			b.WriteString(strconv.Itoa(r.intn(700) - 350))
		}
		ins = append(ins, b.String())
	}
	var cases []ntFloat
	for _, in := range ins {
		v, err := strconv.ParseFloat(ntUTF8(in), 64)
		cases = append(cases, ntFloat{In: in, Bits: U64(math.Float64bits(v)), Err: ntErr(err)})
	}
	return cases
}

func ntGenFormatFloat() []ntFormatFloat {
	bits := []uint64{
		0, 1 << 63, 0x7FF8000000000001, 0x7FF0000000000001, 0xFFF8000000000000, 0x7FFFFFFFFFFFFFFF,
		0x7FF0000000000000, 0xFFF0000000000000, 1, 0x000FFFFFFFFFFFFF, 0x0010000000000000,
		0x7FEFFFFFFFFFFFFF, 0x8000000000000001, 0x3FF0000000000000, 0x3FF0000000000001,
	}
	for _, x := range []float64{1, -1, 0.1, 0.5, 1e21, 1e20, 123456789, 1e-4, 1e-5, 0.0001, 0.00001, 100000,
		1000000, 999999, 20, 1234567.8, 200000, 2000000, 1e10, 32, 1e23, 99999999999999974834176,
		100000000000000008388608, 5e-324, 2.2250738585072012e-308, 2.2250738585072011e-308, 383260575764816448,
		498484681984085570, -5.8339553793802237e+23, 1.801439850948199e+16, 5.960464477539063e-08, 1.012e-320,
		108678236358137.625, 1.5, 3.7e+06, 0.3, 2.5, 1.0000000000000002, math.MaxInt64, 1 << 53, 1<<53 + 1,
		0.000123456, 123456.7, 1e6 - 0.5, 9.999999999999999e-5, 1e-4 - 1e-20} {
		bits = append(bits, math.Float64bits(x))
	}
	for e := -1074; e <= 1023; e += 13 {
		bits = append(bits, math.Float64bits(math.Ldexp(1, e)))
	}
	for e := -325; e <= 308; e += 7 {
		bits = append(bits, math.Float64bits(math.Pow(10, float64(e))))
	}
	r := &ntRNG{s: 0x6e74_6674_6f61}
	// Exact ties between two shortest candidates: m/4 for odd m in [2^52, 2^53). Go rounds to even.
	for i := 0; i < 200; i++ {
		m := uint64(1)<<52 | r.next()&(1<<52-1) | 1
		bits = append(bits, math.Float64bits(float64(m)/4))
	}
	// Random bit patterns.
	for i := 0; i < 1500; i++ {
		bits = append(bits, r.next())
	}
	// Random values around the %e/%f switch (exponents -6 ..= 22).
	for i := 0; i < 500; i++ {
		x := float64(r.next()>>11) / (1 << 53) * math.Pow(10, float64(r.intn(29)-6))
		bits = append(bits, math.Float64bits(x))
	}
	var cases []ntFormatFloat
	for _, b := range bits {
		cases = append(cases, ntFormatFloat{Bits: U64(b), Out: strconv.FormatFloat(math.Float64frombits(b), 'g', -1, 64)})
	}
	return cases
}

func ntGenDurations() ([]ntDurString, []ntDurRound) {
	edges := []int64{0, 1, -1, 999, -999, 1000, 1100, 1500, 999999, 1000000, 1500000, 2200000, 999999999,
		1000000000, 1500000000, 3300000000, 30e9, 60e9, 61e9, 120e9, 245e9, 245001e6, 18367001e6, 480000000001,
		3600e9, 3661e9, 93603e9, math.MaxInt64, math.MinInt64, math.MaxInt64 - 1, math.MinInt64 + 1, 400e3,
		500e3, 2500e3, 99400e3, 1234567e3, 1<<63 - 1e9, 26*3600e9 + 3e9}
	var strs []ntDurString
	addS := func(ns int64) {
		strs = append(strs, ntDurString{Ns: I64(ns), Out: time.Duration(ns).String()})
	}
	for _, ns := range edges {
		addS(ns)
		if ns != math.MinInt64 {
			addS(-ns)
		}
	}
	r := &ntRNG{s: 0x6e74_6475_72}
	for i := 0; i < 400; i++ {
		addS(int64(r.next()) >> uint(r.intn(64)))
	}
	ms := []int64{0, -1, -1e9, 1, 7, 1000, 1e6, 7e6, 1e9, 60e9, 3600e9, 3 << 61, math.MaxInt64}
	var rounds []ntDurRound
	addR := func(ns, m int64) {
		rounds = append(rounds, ntDurRound{Ns: I64(ns), M: I64(m), Out: I64(time.Duration(ns).Round(time.Duration(m)))})
	}
	// time_test.go durationRoundTests
	for _, c := range [][2]int64{{0, 1e9}, {60e9, -11e9}, {60e9, 0}, {60e9, 1}, {120e9, 60e9}, {130e9, 60e9},
		{150e9, 60e9}, {170e9, 60e9}, {-60e9, 1}, {-120e9, 60e9}, {-130e9, 60e9}, {-150e9, 60e9},
		{-170e9, 60e9}, {8e18, 3e18}, {9e18, 5e18}, {-8e18, 3e18}, {-9e18, 5e18}, {3<<61 - 1, 3 << 61}} {
		addR(c[0], c[1])
	}
	for _, ns := range edges {
		for _, m := range ms {
			addR(ns, m)
			if ns != math.MinInt64 {
				addR(-ns, m)
			}
		}
	}
	for i := 0; i < 300; i++ {
		addR(int64(r.next())>>uint(r.intn(64)), int64(r.next()>>1)>>uint(r.intn(63)))
	}
	return strs, rounds
}

func ntGenParseDuration() []ntParseDur {
	ins := []string{
		// time_test.go parseDurationTests
		"0", "5s", "30s", "1478s", "-5s", "+5s", "-0", "+0", "5.0s", "5.6s", "5.s", ".5s", "1.0s", "1.00s",
		"1.004s", "1.0040s", "100.00100s", "10ns", "11us", "12µs", "12μs", "13ms", "14s", "15m", "16h", "3h30m",
		"10.5s4m", "-2m3.4s", "1h2m3s4ms5us6ns", "39h9m14.425s", "52763797000ns", "0.3333333333333333333h",
		"9007199254740993ns", "9223372036854775807ns", "9223372036854775.807us", "9223372036s854ms775us807ns",
		"-9223372036854775808ns", "-9223372036854775.808us", "-9223372036s854ms775us808ns",
		"-2562047h47m16.854775808s", "0.100000000000000000000h", "0.830103483285477580700h",
		// time_test.go parseDurationErrorTests (valid UTF-8 ones)
		"", "3", "-", "s", ".", "-.", ".s", "+.s", "1d", "�", "� hello � world",
		"9223372036854775810ns", "9223372036854775808ns", "-9223372036854775809ns", "9223372036854776us",
		"3000000h", "9223372036854775.808us", "9223372036854ms775us808ns",
		// more
		"1µ", "µs", "1.5.s", "1h2", "h", "1hh", "1 h", " 1h", "1h ", "1H", "1Ms", "1e3s", "-1", "+", "0.0",
		"0s", "-0s", "00", "0.", ".0", "0h0m", "2562047h47m16.854775807s", "2562047h47m16.854775808s",
		"2562047.7880152155h", "106751d", "1秒", "1\"s", "1\\s", "1\ts", "1\x01s", "0.1ns", "1.9999999999ns",
		"0.000000001s", "0.0000000001s", "1.00000000000000000000000000000001h", "-.5h", "+.5h", "1m-1s",
		"1s1", "\x7f", "18446744073709551616ns", "99999999999999999999s", "1.99999999999999999999999999999s",
		"2540400h10m10.000000000s", "-2540400h10m10.000000000s", "1_000s", "0x1s",
	}
	r := &ntRNG{s: 0x6e74_7064}
	for i := 0; i < 200; i++ {
		s := time.Duration(int64(r.next()) >> uint(r.intn(64))).String()
		if r.intn(3) == 0 && len(s) > 1 {
			// Mutate one byte.
			p := r.intn(len(s))
			alphabet := "0123456789.+-hmsnuµ x"
			c := alphabet[r.intn(len(alphabet))]
			if c >= utf8.RuneSelf || s[p] >= utf8.RuneSelf {
				c = 'x'
				p = 0
			}
			s = s[:p] + string(c) + s[p+1:]
		}
		ins = append(ins, s)
	}
	var cases []ntParseDur
	for _, in := range ins {
		d, err := time.ParseDuration(ntUTF8(in))
		cases = append(cases, ntParseDur{In: in, Ns: I64(d), Err: ntErr(err)})
	}
	return cases
}

// ntInstant is a (unix seconds, nanoseconds) pair.
type ntInstant struct {
	sec  int64
	nsec int
}

func ntFixedInstants() []ntInstant {
	d := func(y int, mo time.Month, day, h, mi, s, ns int) ntInstant {
		t := time.Date(y, mo, day, h, mi, s, ns, time.UTC)
		return ntInstant{t.Unix(), t.Nanosecond()}
	}
	return []ntInstant{
		{0, 0}, {0, 1}, {-1, 999999999}, {-1, 0}, {1, 100000000},
		d(2026, 9, 18, 10, 11, 12, 345678901), d(2026, 9, 18, 10, 11, 12, 345999999),
		d(2026, 9, 18, 10, 34, 56, 5000000), d(2026, 9, 18, 12, 34, 56, 789654321),
		d(2024, 2, 29, 23, 59, 59, 999999999), d(2000, 2, 29, 12, 0, 0, 120000000), d(1900, 3, 1, 0, 0, 0, 0),
		d(1600, 1, 1, 0, 0, 0, 123456789), d(0, 1, 1, 0, 0, 0, 0), d(1, 1, 1, 0, 0, 0, 1), d(-1, 12, 31, 23, 59, 59, 0),
		d(-10000, 6, 15, 1, 2, 3, 999000000), d(9999, 12, 31, 23, 59, 59, 999999999), d(10000, 1, 1, 0, 0, 0, 1000000),
		d(123456, 7, 8, 9, 10, 11, 999999), {-9223372036, 854775808}, {9223372036, 854775807},
		{math.MaxInt64, 0}, {math.MinInt64, 0}, {math.MaxInt64 - 62135596800, 5}, {-62135596800 - 1, 999999999},
	}
}

var ntFixedOffsets = []int{0, 1, -1, 59, -59, 60, -60, 61, -61, 3600, -3600, 19800, -12600, 50400, -43200,
	86399, -86399, 90000, -90000, 360000}

var ntSomeOffsets = []int{0, 1, -1, -61, 3600, 19800, -12600, 50400, -86399}

func ntRandomInstant(r *ntRNG) ntInstant {
	lo := int64(-62167219200)   // 0000-01-01
	span := int64(315537897600) // through 9999-12-31
	return ntInstant{lo + int64(r.next()%uint64(span)), int(r.next() % 1e9)}
}

func ntGenLayouts() (local []ntTimeZoned, nano []ntTimeUTC, slogs []ntTimeZoned, clocks []ntTimeZoned, logs []ntTimeZoned) {
	zoned := func(in ntInstant, off int, f func(t time.Time) (string, bool)) (ntTimeZoned, bool) {
		t := time.Unix(in.sec, int64(in.nsec)).In(time.FixedZone("", off))
		out, ok := f(t)
		return ntTimeZoned{UnixSecs: I64(in.sec), Nanos: in.nsec, OffsetSecs: off, Out: ntUTF8(out)}, ok
	}
	rfc := func(t time.Time) (string, bool) { return t.Format(time.RFC3339), true }
	clock := func(t time.Time) (string, bool) { return t.Format("15:04:05"), true }
	std := func(t time.Time) (string, bool) { return ntLogStd(t), true }
	slogT := func(t time.Time) (string, bool) {
		if t.IsZero() {
			// slog omits the time of a zero record time.
			return "", false
		}
		var buf bytes.Buffer
		h := slog.NewTextHandler(&buf, nil)
		if err := h.Handle(context.Background(), slog.NewRecord(t, slog.LevelInfo, "m", 0)); err != nil {
			panic(err)
		}
		line := buf.String()
		rest, ok := strings.CutPrefix(line, "time=")
		if !ok {
			panic("gocompat-numtime: slog line without time: " + line)
		}
		ts, _, ok := strings.Cut(rest, " level=")
		if !ok {
			panic("gocompat-numtime: slog line without level: " + line)
		}
		return ts, true
	}
	add := func(dst *[]ntTimeZoned, in ntInstant, off int, f func(t time.Time) (string, bool)) {
		if c, ok := zoned(in, off, f); ok {
			*dst = append(*dst, c)
		}
	}
	fixed := ntFixedInstants()
	for _, in := range fixed {
		for _, off := range ntFixedOffsets {
			add(&local, in, off, rfc)
		}
		for _, off := range ntSomeOffsets {
			add(&slogs, in, off, slogT)
			add(&clocks, in, off, clock)
			add(&logs, in, off, std)
		}
		nano = append(nano, ntTimeUTC{UnixSecs: I64(in.sec), Nanos: in.nsec,
			Out: ntUTF8(time.Unix(in.sec, int64(in.nsec)).UTC().Format(time.RFC3339Nano))})
	}
	r := &ntRNG{s: 0x6e74_6c61_796f_7574}
	for i := 0; i < 150; i++ {
		in := ntRandomInstant(r)
		off := r.intn(2*50400+1) - 50400
		add(&local, in, off, rfc)
		add(&slogs, in, off, slogT)
		add(&clocks, in, off, clock)
		add(&logs, in, off, std)
		nano = append(nano, ntTimeUTC{UnixSecs: I64(in.sec), Nanos: in.nsec,
			Out: ntUTF8(time.Unix(in.sec, int64(in.nsec)).UTC().Format(time.RFC3339Nano))})
	}
	return local, nano, slogs, clocks, logs
}

func ntGenParseRFC3339Nano() []ntParseTime {
	ins := []string{
		// time format_test.go (RFC3339 layout cases)
		"2006-01-02T15:04:05Z07:00", "2006-01-02T15:04_abc", "2006-01-02T15:04:05_abc", "2006-01-02T15:04:05Z_abc",
		"2010-02-04T21:00:67.012345678-08:00", "0000-01-01T00:00:.0+00:00", "\"", "0000-01-01T00:00:00+00:+0",
		"0000-01-01T00:00:00+-0:00", "2008-09-17T20:04:26Z", "1994-09-17T20:04:26-05:00", "2000-12-26T01:15:06+04:20",
		// more
		"", "2026", "2026-09-18", "2026-09-18T10:11:12", "2026-09-18T10:11:12Z", "2026-09-18T10:11:12.345678901Z",
		"2026-09-18T10:11:12.1Z", "2026-09-18T10:11:12.12Z", "2026-09-18T10:11:12.123456789123Z",
		"2026-09-18T10:11:12.0000000009Z", "2026-09-18T10:11:12,5Z", "2026-09-18T10:11:12.Z", "2026-09-18T10:11:12.5",
		"2026-09-18T10:11:12+05:30", "2026-09-18T10:11:12-00:00", "2026-09-18T10:11:12+23:59",
		"2026-09-18T10:11:12+24:00", "2026-09-18T10:11:12+24:60", "2026-09-18T10:11:12+25:00",
		"2026-09-18T10:11:12+05:61", "2026-09-18T10:11:12+25:61", "2026-09-18T10:11:12*05:00",
		"2026-09-18T10:11:12+0530", "2026-09-18T10:11:12+05:3x", "2026-09-18T10:11:12+x5:30",
		"2026-09-18T10:11:12+25:3x", "2026-09-18T10:11:12Zjunk", "2026-09-18T10:11:12+05:30junk",
		"2026-09-18 10:11:12Z", "2026-09-18t10:11:12Z", "2026-09-18T10:11:12z", "0000-01-01T00:00:00Z",
		"9999-12-31T23:59:59.999999999+14:00", "9999-12-31T23:59:59.999999999-14:00", "2024-02-29T00:00:00Z",
		"2023-02-29T00:00:00Z", "2100-02-29T00:00:00Z", "2000-02-29T00:00:00Z", "2026-04-31T00:00:00Z",
		"2026-13-01T00:00:00Z", "2026-00-10T00:00:00Z", "2026-09-00T00:00:00Z", "2026-09-32T00:00:00Z",
		"2026-09-18T24:00:00Z", "2026-09-18T23:60:00Z", "2026-09-18T23:59:60Z", "2026-09-18T1:02:03Z",
		"2026-09-18T:02:03Z", "2026-9-18T10:11:12Z", "2026-09-8T10:11:12Z", "+2026-09-18T10:11:12Z",
		"20266-09-18T10:11:12Z", "abcd-09-18T10:11:12Z", "２０２６-09-18T10:11:12Z", "2026-09-18T10:11:12Z\n",
		"\"2026-09-18T10:11:12Z\"", "2026-09-18T10:11:1", "2026-09-18T10:1", "2026-09-18T10", "2026-09-18T",
		"2026-09-1", "2026-0", "202", "2026-09-18T10:11:12.999999999999999999999Z", "1969-12-31T23:59:59.999999999Z",
		"1970-01-01T00:00:00.000000001+00:01", "2026-09-18T10:11:12.5x", "2026-09-18T10:11:12é", "2026-09-18T10:11:12\\",
	}
	r := &ntRNG{s: 0x6e74_7072_6663}
	for i := 0; i < 150; i++ {
		in := ntRandomInstant(r)
		off := (r.intn(2*1440+1) - 1440) * 60
		s := time.Unix(in.sec, int64(in.nsec)).In(time.FixedZone("", off)).Format(time.RFC3339Nano)
		if r.intn(2) == 0 {
			// Mutate one byte.
			alphabet := "0123456789-+:TZ.,_ xz"
			p := r.intn(len(s))
			s = s[:p] + string(alphabet[r.intn(len(alphabet))]) + s[p+1:]
		}
		ins = append(ins, s)
	}
	var cases []ntParseTime
	for _, in := range ins {
		t, err := time.Parse(time.RFC3339Nano, ntUTF8(in))
		c := ntParseTime{In: in, Err: ntErr(err)}
		if err == nil {
			c.UnixSecs = I64(t.Unix())
			c.Nanos = t.Nanosecond()
		}
		cases = append(cases, c)
	}
	return cases
}

// ntLogItoa is go1.26.5 src/log/log.go itoa, verbatim apart from the name (checked by ntCheckLogCopy).
func ntLogItoa(buf *[]byte, i int, wid int) {
	// Assemble decimal in reverse order.
	var b [20]byte
	bp := len(b) - 1
	for i >= 10 || wid > 1 {
		wid--
		q := i / 10
		b[bp] = byte('0' + i - q*10)
		bp--
		i = q
	}
	// i < 10
	b[bp] = byte('0' + i)
	*buf = append(*buf, b[bp:]...)
}

// ntLogStd is the Ldate and Ltime part of go1.26.5 log.formatHeader (flags LstdFlags, the standard
// logger slog.Default writes through), without the trailing space.
func ntLogStd(t time.Time) string {
	var b []byte
	buf := &b
	year, month, day := t.Date()
	ntLogItoa(buf, year, 4)
	*buf = append(*buf, '/')
	ntLogItoa(buf, int(month), 2)
	*buf = append(*buf, '/')
	ntLogItoa(buf, day, 2)
	*buf = append(*buf, ' ')
	hour, min, sec := t.Clock()
	ntLogItoa(buf, hour, 2)
	*buf = append(*buf, ':')
	ntLogItoa(buf, min, 2)
	*buf = append(*buf, ':')
	ntLogItoa(buf, sec, 2)
	return string(b)
}

// ntCheckLogCopy checks ntLogItoa and ntLogStd against the toolchain's src/log/log.go.
func ntCheckLogCopy() error {
	path := filepath.Join(build.Default.GOROOT, "src", "log", "log.go")
	fset := token.NewFileSet()
	file, err := parser.ParseFile(fset, path, nil, 0)
	if err != nil {
		return fmt.Errorf("gocompat-numtime: reading the log package source: %w", err)
	}
	funcs := map[string]string{}
	for _, decl := range file.Decls {
		if fd, ok := decl.(*ast.FuncDecl); ok && fd.Recv == nil {
			var buf bytes.Buffer
			if err := printer.Fprint(&buf, fset, fd); err != nil {
				return err
			}
			funcs[fd.Name.Name] = ntNoSpace(buf.String())
		}
	}
	want := []struct{ fn, snippet string }{
		{"itoa", `func itoa(buf *[]byte, i int, wid int) {
			var b [20]byte
			bp := len(b) - 1
			for i >= 10 || wid > 1 {
				wid--
				q := i / 10
				b[bp] = byte('0' + i - q*10)
				bp--
				i = q
			}
			b[bp] = byte('0' + i)
			*buf = append(*buf, b[bp:]...)
		}`},
		{"formatHeader", `if flag&Ldate != 0 {
				year, month, day := t.Date()
				itoa(buf, year, 4)
				*buf = append(*buf, '/')
				itoa(buf, int(month), 2)
				*buf = append(*buf, '/')
				itoa(buf, day, 2)
				*buf = append(*buf, ' ')
			}
			if flag&(Ltime|Lmicroseconds) != 0 {
				hour, min, sec := t.Clock()
				itoa(buf, hour, 2)
				*buf = append(*buf, ':')
				itoa(buf, min, 2)
				*buf = append(*buf, ':')
				itoa(buf, sec, 2)`},
	}
	for _, w := range want {
		if !strings.Contains(funcs[w.fn], ntNoSpace(w.snippet)) {
			return fmt.Errorf("gocompat-numtime: %s in %s changed; update ntLogItoa/ntLogStd", w.fn, path)
		}
	}
	return nil
}

func ntNoSpace(s string) string {
	return strings.Map(func(r rune) rune {
		if unicode.IsSpace(r) {
			return -1
		}
		return r
	}, s)
}
