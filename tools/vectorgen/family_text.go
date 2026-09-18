package main

import (
	"fmt"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"unicode"
	"unicode/utf8"
)

// The quote and case families pin the go1.26.5 text functions that crates/gocompat/src/quote.rs and
// strings.rs port (PORTING.md §4.1): strconv.Quote and strconv.IsPrint (text/quote.json), and
// strings.ToLower, strings.ToUpper, strings.TrimSpace, strings.FieldsFunc(unicode.IsSpace),
// unicode.ToLower, unicode.ToUpper and unicode.IsSpace (text/case.json). Schemas: docs/gocompat-a.md.
func init() {
	register("quote", []string{"text/quote.json"}, genQuote)
	register("case", []string{"text/case.json"}, genCase)
}

const (
	quoteSeed   = 0x71756f7465 // splitmix64 seed of the random Quote sample ("quote")
	caseSeed    = 0x63617365   // splitmix64 seed of the random case sample ("case")
	randomCount = 256          // random cases per file
)

// quoteFile is text/quote.json.
type quoteFile struct {
	GoVersion      string      `json:"go_version"`
	UnicodeVersion string      `json:"unicode_version"`
	Cases          []quoteCase `json:"cases"`
	// IsPrintFlips lists, ascending, every rune r where strconv.IsPrint(r) != strconv.IsPrint(r-1),
	// starting from "not printable" at rune 0.
	IsPrintFlips []rune `json:"is_print_flips"`
}

// quoteCase is strconv.Quote over the bytes in.
type quoteCase struct {
	Name  string `json:"name"`
	InHex Hex    `json:"in_hex"`
	Out   string `json:"out"`
}

// caseFile is text/case.json.
type caseFile struct {
	GoVersion      string     `json:"go_version"`
	UnicodeVersion string     `json:"unicode_version"`
	Cases          []caseCase `json:"cases"`
	ToLower        []caseRun  `json:"to_lower"` // unicode.ToLower
	ToUpper        []caseRun  `json:"to_upper"` // unicode.ToUpper
	// IsSpaceFlips lists, ascending, every rune r where unicode.IsSpace(r) != unicode.IsSpace(r-1),
	// starting from "not space" at rune 0.
	IsSpaceFlips []rune `json:"is_space_flips"`
}

// caseCase applies the strings functions to one input.
type caseCase struct {
	Name         string `json:"name"`
	InHex        Hex    `json:"in_hex"`
	ToLowerHex   Hex    `json:"to_lower_hex"`   // strings.ToLower
	ToUpperHex   Hex    `json:"to_upper_hex"`   // strings.ToUpper
	TrimSpaceHex Hex    `json:"trim_space_hex"` // strings.TrimSpace
	FieldsHex    []Hex  `json:"fields_hex"`     // strings.FieldsFunc(in, unicode.IsSpace)
}

// caseRun describes part of a case mapping: the runes lo, lo+stride, …, hi map to rune+delta. Every
// rune outside the runs of a mapping maps to itself.
type caseRun struct {
	Lo     rune `json:"lo"`
	Hi     rune `json:"hi"`
	Stride rune `json:"stride"`
	Delta  rune `json:"delta"`
}

// quoteSpecials are named Quote inputs: the probes of the port notes, the escapes, invalid UTF-8 of
// every kind, and non-printable runes of every kind.
var quoteSpecials = []struct{ name, in string }{
	{"empty", ""},
	{"plain ASCII", "hello, world"},
	{"quote and backslash", `quote"back\slash`},
	{"C escapes", "\a\b\f\n\r\t\v"},
	{"strconv quotetests control characters", "\a\b\f\r\n\t\v"},
	{"strconv quotetests invalid byte", "abc\xffdef"},
	{"strconv quotetests graphic spaces", "!\u00a0!\u2000!\u3000!"},
	{"ticket probe", "zz\x01\"\u00e9"},
	{"view probe tab", "ab\tcd"},
	{"view probe trailing newline", "d70d3259e4e1cb631c663cf4d73c4c04022ab1ba804098e6cb293e6770eb3a95\n"},
	{"view probe letters and emoji", "h\u00e9llo \U0001f600"},
	{"view probe invalid bytes", "\xff\xfe"},
	{"view probe NUL and DEL", "\x00\x7f"},
	{"zone probe", "zone-a"},
	{"slog probe control byte", "a\x01b"},
	{"slog probe invalid byte", "a\xffb"},
	{"slog probe NBSP", "a\u00a0b"},
	{"slog probe DEL", "a\x7fb"},
	{"U+00AD soft hyphen", "\u00ad"},
	{"U+034F combining grapheme joiner", "\u034f"},
	{"combining acute accent after e", "e\u0301"},
	{"combining acute accent alone", "\u0301"},
	{"U+0378 unassigned", "\u0378"},
	{"U+061C arabic letter mark", "\u061c"},
	{"U+1680 ogham space mark", "\u1680"},
	{"U+180E mongolian vowel separator", "\u180e"},
	{"U+2000 en quad", "\u2000"},
	{"U+200B zero width space", "\u200b"},
	{"U+200D zero width joiner", "\u200d"},
	{"U+2028 line separator", "\u2028"},
	{"U+2029 paragraph separator", "\u2029"},
	{"U+202E right-to-left override", "\u202e"},
	{"U+2060 word joiner", "\u2060"},
	{"U+263A smiling face", "\u263a"},
	{"U+3000 ideographic space", "\u3000"},
	{"U+D7FF last rune before the surrogates", "\ud7ff"},
	{"U+E000 private use", "\ue000"},
	{"U+F8FF private use", "\uf8ff"},
	{"U+FEFF byte order mark", "\ufeff"},
	{"U+FFF9 interlinear annotation anchor", "\ufff9"},
	{"U+FFFC object replacement character", "\ufffc"},
	{"U+FFFD replacement character, valid", "\ufffd"},
	{"U+FFFE noncharacter", "\ufffe"},
	{"U+FFFF noncharacter", "\uffff"},
	{"U+10000 linear b syllable a", "\U00010000"},
	{"U+1D173 musical symbol begin beam", "\U0001d173"},
	{"U+1F600 emoji", "\U0001f600"},
	{"U+1FFFF noncharacter", "\U0001ffff"},
	{"U+20000 CJK extension B", "\U00020000"},
	{"U+2FFFF noncharacter", "\U0002ffff"},
	{"U+E0001 language tag", "\U000e0001"},
	{"U+E0100 variation selector 17", "\U000e0100"},
	{"U+F0000 plane 15 private use", "\U000f0000"},
	{"U+10FFFF last rune", "\U0010ffff"},
	{"surrogate U+D800 encoded", "\xed\xa0\x80"},
	{"surrogate U+DBFF encoded", "\xed\xaf\xbf"},
	{"surrogate U+DC00 encoded", "\xed\xb0\x80"},
	{"surrogate U+DFFF encoded", "\xed\xbf\xbf"},
	{"CESU-8 surrogate pair of U+1F600", "\xed\xa0\xbd\xed\xb8\x80"},
	{"overlong NUL c0 80", "\xc0\x80"},
	{"overlong c1 bf", "\xc1\xbf"},
	{"overlong e0 80 80", "\xe0\x80\x80"},
	{"overlong e0 9f bf", "\xe0\x9f\xbf"},
	{"overlong f0 80 80 80", "\xf0\x80\x80\x80"},
	{"overlong f0 8f bf bf", "\xf0\x8f\xbf\xbf"},
	{"above U+10FFFF f4 90 80 80", "\xf4\x90\x80\x80"},
	{"lead byte f5", "\xf5\x80\x80\x80"},
	{"five-byte form f8", "\xf8\x88\x80\x80\x80"},
	{"six-byte form fc", "\xfc\x84\x80\x80\x80\x80"},
	{"truncated two-byte rune", "\xc3"},
	{"truncated three-byte rune", "\xe2\x82"},
	{"truncated four-byte rune", "\xf0\x9f\x98"},
	{"truncated rune inside ASCII", "a\xe2\x82b"},
	{"valid rune then a stray continuation byte", "\xf0\x9f\x98\x80\x80"},
	{"continuation bytes", "\x80\x80\x80\x80\x80"},
	{"valid, invalid, valid", "\u00e9\xff\u00e9"},
	{"euro sign then a truncated euro sign", "\xe2\x82\xac\xe2\x82"},
	{"lead byte before a valid rune", "\xe2\xe2\x82\xac"},
	{"invalid byte before a quote", "\xff\"\\"},
}

// quoteRandomRunes are runes the random Quote sample draws from besides the uniform picks.
var quoteRandomRunes = []rune{'"', '\\', 0x7f, 0x85, 0xa0, 0xad, 0x200b, 0x2028, 0xfeff, 0xfffd, 0x1f600, 0xe0001, 0x10ffff}

// printClasses names the lookup paths of strconv.IsPrint; printClass picks one for a rune.
var printClasses = []string{
	"printable Latin-1 (fast path)",
	"non-printable Latin-1 (fast path)",
	"printable in an isPrint16 range",
	"non-printable inside an isPrint16 range (isNotPrint16)",
	"non-printable between isPrint16 ranges",
	"printable in an isPrint32 range below U+20000",
	"non-printable inside an isPrint32 range (isNotPrint32)",
	"non-printable between isPrint32 ranges below U+20000",
	"printable from U+20000 (no exception list)",
	"non-printable from U+20000",
}

// printClass returns the index in printClasses of r, or -1 for a surrogate.
func printClass(r rune) int {
	if !utf8.ValidRune(r) {
		return -1
	}
	p := strconv.IsPrint(r)
	inside := !p && strconv.IsPrint(r-1) && strconv.IsPrint(r+1)
	switch {
	case r <= 0xFF && p:
		return 0
	case r <= 0xFF:
		return 1
	case r <= 0xFFFF && p:
		return 2
	case r < 0xFFFF && inside:
		return 3
	case r <= 0xFFFF:
		return 4
	case r < 0x20000 && p:
		return 5
	case r > 0x10000 && r < 0x20000 && inside:
		return 6
	case r < 0x20000:
		return 7
	case p:
		return 8
	default:
		return 9
	}
}

func genQuote(out string) error {
	f := quoteFile{
		GoVersion:      runtime.Version(),
		UnicodeVersion: unicode.Version,
		Cases:          []quoteCase{},
		IsPrintFlips:   flips(strconv.IsPrint),
	}
	add := func(name string, in []byte) {
		f.Cases = append(f.Cases, quoteCase{Name: name, InHex: hexOf(in), Out: strconv.Quote(string(in))})
	}
	for b := 0; b < 0x80; b++ {
		add(fmt.Sprintf("ASCII byte 0x%02x", b), []byte{byte(b)})
	}
	for r := rune(0x80); r <= 0xFF; r++ {
		add(fmt.Sprintf("Latin-1 U+%04X", r), utf8.AppendRune(nil, r))
	}
	for b := 0x80; b <= 0xFF; b++ {
		add(fmt.Sprintf("invalid byte 0x%02x", b), []byte{byte(b)})
	}
	add("every ASCII byte", asciiBytes())
	for _, c := range quoteSpecials {
		add(c.name, []byte(c.in))
	}

	// The first, middle and last rune of every IsPrint lookup path.
	members := make([][]rune, len(printClasses))
	for r := rune(0); r <= unicode.MaxRune; r++ {
		if c := printClass(r); c >= 0 {
			members[c] = append(members[c], r)
		}
	}
	for c, name := range printClasses {
		m := members[c]
		if len(m) == 0 {
			return fmt.Errorf("IsPrint class %q has no runes", name)
		}
		picks := []struct {
			which string
			r     rune
		}{{"first", m[0]}, {"middle", m[len(m)/2]}, {"last", m[len(m)-1]}}
		for i, pick := range picks {
			if i > 0 && pick.r == picks[i-1].r {
				continue
			}
			add(fmt.Sprintf("%s: %s U+%04X", name, pick.which, pick.r), utf8.AppendRune(nil, pick.r))
		}
	}
	// Every graphic rune that is not printable.
	for r := rune(0); r <= unicode.MaxRune; r++ {
		if strconv.IsGraphic(r) && !strconv.IsPrint(r) {
			add(fmt.Sprintf("graphic, not printable U+%04X", r), utf8.AppendRune(nil, r))
		}
	}
	// Both sides of every IsPrint boundary, eight boundaries per case.
	for i := 0; i < len(f.IsPrintFlips); i += 8 {
		group := f.IsPrintFlips[i:min(i+8, len(f.IsPrintFlips))]
		var in []byte
		for _, r := range group {
			in = appendValidRune(in, r-1)
			in = appendValidRune(in, r)
		}
		add(fmt.Sprintf("is_print boundaries %d to %d (U+%04X to U+%04X)", i, i+len(group)-1, group[0], group[len(group)-1]), in)
	}
	state := uint64(quoteSeed)
	for i := range randomCount {
		add(fmt.Sprintf("random %d", i), randomQuoteInput(&state))
	}
	for _, c := range f.Cases {
		if !utf8.ValidString(c.Out) {
			return fmt.Errorf("quote case %q: output is not valid UTF-8", c.Name)
		}
	}
	return writeJSON(filepath.Join(out, "text", "quote.json"), f)
}

// randomQuoteInput draws up to 23 units: ASCII bytes, Latin-1 runes, BMP runes (surrogates as their
// invalid three-byte form), astral runes, invalid bytes, and quoteRandomRunes.
func randomQuoteInput(state *uint64) []byte {
	n := int(splitmixNext(state) % 24)
	b := []byte{}
	for range n {
		x := splitmixNext(state)
		y := x >> 8
		switch x % 8 {
		case 0, 1:
			b = append(b, byte(y%0x80))
		case 2:
			b = utf8.AppendRune(b, rune(0x80+y%0x80))
		case 3:
			b = appendRuneBytes(b, rune(y%0x10000))
		case 4:
			b = utf8.AppendRune(b, rune(0x10000+y%0x100000))
		case 5:
			b = append(b, byte(0x80+y%0x80))
		default:
			b = utf8.AppendRune(b, quoteRandomRunes[y%uint64(len(quoteRandomRunes))])
		}
	}
	return b
}

// caseSpecials are named inputs of the strings functions: the case-mapping lookalikes of the ticket
// parser, length-changing mappings, titlecase digraphs, invalid UTF-8, and white space at the edges.
var caseSpecials = []struct{ name, in string }{
	{"empty", ""},
	{"ASCII lower", "hello world"},
	{"ASCII upper", "HELLO WORLD"},
	{"ASCII mixed", "MiXeD CaSe 123"},
	{"ASCII without letters", "123 !?#{}"},
	{"strings upperTests nonascii", "long\u0250string\u0250with\u0250nonascii\u2c6fchars"},
	{"strings upperTests grows", "\u0250\u0250\u0250\u0250\u0250"},
	{"strings lowerTests shrinks", "\u2c6d\u2c6d\u2c6d\u2c6d\u2c6d"},
	{"strings RuneSelf and MaxRune", "a\u0080\U0010FFFF"},
	{"U+0130 capital I with dot above", "\u0130"},
	{"U+0131 dotless i", "\u0131"},
	{"U+017F long s", "\u017f"},
	{"U+212A Kelvin sign", "\u212a"},
	{"U+212B Angstrom sign", "\u212b"},
	{"U+2126 Ohm sign", "\u2126"},
	{"lookalikes in a base32 id", "\u0130\u0131\u017f\u212aabcxyz"},
	{"Istanbul with U+0130", "\u0130stanbul"},
	{"U+00DF sharp s", "\u00df"},
	{"U+1E9E capital sharp s", "\u1e9e"},
	{"strasse with U+00DF", "stra\u00dfe"},
	{"U+01C4 to U+01C6 DZ with caron", "\u01c4\u01c5\u01c6"},
	{"U+01C7 to U+01CC LJ and NJ", "\u01c7\u01c8\u01c9\u01ca\u01cb\u01cc"},
	{"U+01F1 to U+01F3 DZ", "\u01f1\u01f2\u01f3"},
	{"U+03C2 final sigma", "\u03c2"},
	{"Greek capitals with sigma", "\u03a3\u038a\u03a3\u03a5\u03a6\u039f\u03a3"},
	{"U+0390 iota with dialytika and tonos", "\u0390"},
	{"U+0149 n preceded by apostrophe", "\u0149"},
	{"U+01F0 j with caron", "\u01f0"},
	{"U+FB00 ligature ff", "\ufb00"},
	{"U+00B5 micro sign", "\u00b5"},
	{"U+00FF y with diaeresis", "\u00ff"},
	{"U+0178 capital Y with diaeresis", "\u0178"},
	{"U+0345 combining ypogegrammeni", "\u0345"},
	{"U+1C80 cyrillic small rounded ve", "\u1c80"},
	{"U+13A0 cherokee A", "\u13a0"},
	{"U+AB70 cherokee small a", "\uab70"},
	{"U+10D0 georgian an", "\u10d0"},
	{"U+1C90 georgian mtavruli an", "\u1c90"},
	{"U+023A A with stroke", "\u023a"},
	{"U+2C65 a with stroke", "\u2c65"},
	{"U+0250 turned a", "\u0250"},
	{"U+24B6 circled A", "\u24b6"},
	{"U+2160 roman numeral one", "\u2160"},
	{"U+A640 cyrillic zemlya", "\ua640\ua641"},
	{"U+10400 deseret capital long i", "\U00010400"},
	{"U+10428 deseret small long i", "\U00010428"},
	{"U+1E900 adlam capital alif", "\U0001e900"},
	{"U+1E922 adlam small alif", "\U0001e922"},
	{"U+FFFD valid", "\ufffd"},
	{"valid U+FFFD among letters", "a\ufffdB"},
	{"invalid byte alone", "\xff"},
	{"invalid byte between ASCII letters", "A\xffb"},
	{"ASCII letters then an invalid byte", "abcDEF\xfe"},
	{"invalid bytes around U+0130", "\xff\u0130\xfe"},
	{"truncated rune at the end", "Ab\xe2\x82"},
	{"surrogate U+D800 encoded", "a\xed\xa0\x80B"},
	{"overlong NUL", "\xc0\x80A"},
	{"above U+10FFFF", "\xf4\x90\x80\x80z"},
	{"continuation bytes", "\x80\x80\x80"},
	{"ASCII spaces around", "  \t\n\v\f\r abc \r\n\t "},
	{"only ASCII spaces", " \t\n\v\f\r"},
	{"inner spaces kept", "  a b  "},
	{"strings trimSpaceTests mixed", " \u2000\t\r\n x\t\t\r\r\ny\n \u3000"},
	{"strings trimSpaceTests invalid tail", "x \xc0\xc0 "},
	{"strings trimSpaceTests smiley and invalid", "x \u263a\xc0\xc0 "},
	{"U+0085 next line", "\u0085a\u0085"},
	{"U+00A0 no-break space", "\u00a0a\u00a0b\u00a0"},
	{"only Unicode spaces", "\u2003\u3000\u00a0"},
	{"ASCII then a Unicode space at the end", "a \u3000"},
	{"Unicode space then ASCII at the start", "\u2003 a"},
	{"non-ASCII first non-space", "  \u00e9 "},
	{"non-ASCII last non-space", " a\u00e9  "},
	{"U+180E is not space", "\u180ea\u180e"},
	{"U+200B is not space", "\u200ba\u200b"},
	{"U+FEFF is not space", "\ufeffa\ufeff"},
	{"U+2060 is not space", "\u2060a\u2060"},
	{"invalid bytes at the edges", " \xff a \xfe "},
	{"truncated U+3000 at the end", " a\xe3\x80"},
	{"U+3000 after a stray lead byte", "a\xe3\xe3\x80\x80"},
	{"stray continuation byte after U+3000", "a\u3000\x80"},
	{"fields separated by Unicode spaces", "a\u00a0b\u3000c\u2028d"},
	{"fields around invalid bytes", "\xff \xfe\xfd x"},
	{"many separators", " \t a \n\n b \u2003\u2003 c "},
}

// caseRandomRunes are runes the random case sample draws from besides the uniform picks.
var caseRandomRunes = []rune{0x130, 0x131, 0x17f, 0x212a, 0xdf, 0x1e9e, 0x1c5, 0x3c2, 0xb5, 0xff, 0xfffd, 0x180e, 0x200b, 0xfeff, 0x2c65, 0x23a, 0x10400, 0x1e900}

func genCase(out string) error {
	f := caseFile{
		GoVersion:      runtime.Version(),
		UnicodeVersion: unicode.Version,
		Cases:          []caseCase{},
		ToLower:        caseRuns(unicode.ToLower),
		ToUpper:        caseRuns(unicode.ToUpper),
		IsSpaceFlips:   flips(unicode.IsSpace),
	}
	add := func(name string, in []byte) {
		s := string(in)
		fields := strings.FieldsFunc(s, unicode.IsSpace)
		c := caseCase{
			Name:         name,
			InHex:        hexOf(in),
			ToLowerHex:   hexOf([]byte(strings.ToLower(s))),
			ToUpperHex:   hexOf([]byte(strings.ToUpper(s))),
			TrimSpaceHex: hexOf([]byte(strings.TrimSpace(s))),
			FieldsHex:    make([]Hex, len(fields)),
		}
		for i, field := range fields {
			c.FieldsHex[i] = hexOf([]byte(field))
		}
		f.Cases = append(f.Cases, c)
	}
	add("every ASCII byte", asciiBytes())
	for _, c := range caseSpecials {
		add(c.name, []byte(c.in))
	}
	var spaces []rune
	for r := rune(0); r <= unicode.MaxRune; r++ {
		if unicode.IsSpace(r) {
			spaces = append(spaces, r)
			ws := string(r)
			add(fmt.Sprintf("white space U+%04X around and between fields", r), []byte(ws+"x"+ws+ws+"y"+ws))
		}
	}
	lower, upper := mappedRunes(unicode.ToLower), mappedRunes(unicode.ToUpper)
	state := uint64(caseSeed)
	for i := range randomCount {
		add(fmt.Sprintf("random %d", i), randomCaseInput(&state, lower, upper, spaces))
	}
	return writeJSON(filepath.Join(out, "text", "case.json"), f)
}

// randomCaseInput draws up to 15 units: printable ASCII, ASCII spaces, white space runes, runes whose
// lower or upper mapping differs, invalid bytes, caseRandomRunes, and any rune (surrogates as their
// invalid three-byte form).
func randomCaseInput(state *uint64, lower, upper, spaces []rune) []byte {
	n := int(splitmixNext(state) % 16)
	b := []byte{}
	for range n {
		x := splitmixNext(state)
		y := x >> 8
		switch x % 8 {
		case 0:
			b = append(b, byte(0x20+y%0x5f))
		case 1:
			b = append(b, " \t\n\v\f\r"[y%6])
		case 2:
			b = utf8.AppendRune(b, spaces[y%uint64(len(spaces))])
		case 3:
			b = utf8.AppendRune(b, lower[y%uint64(len(lower))])
		case 4:
			b = utf8.AppendRune(b, upper[y%uint64(len(upper))])
		case 5:
			b = append(b, byte(0x80+y%0x80))
		case 6:
			b = utf8.AppendRune(b, caseRandomRunes[y%uint64(len(caseRandomRunes))])
		default:
			b = appendRuneBytes(b, rune(y%(unicode.MaxRune+1)))
		}
	}
	return b
}

// flips lists every rune r where pred(r) != pred(r-1), with pred(-1) taken as false.
func flips(pred func(rune) bool) []rune {
	out := []rune{}
	prev := false
	for r := rune(0); r <= unicode.MaxRune; r++ {
		if p := pred(r); p != prev {
			out = append(out, r)
			prev = p
		}
	}
	return out
}

// caseRuns describes a case mapping as runs of runes with a common delta and a stride of 1 or 2.
func caseRuns(mapping func(rune) rune) []caseRun {
	runs := []caseRun{}
	for r := rune(0); r <= unicode.MaxRune; r++ {
		m := mapping(r)
		if m == r {
			continue
		}
		d := m - r
		if n := len(runs); n > 0 && runs[n-1].Delta == d {
			last := &runs[n-1]
			gap := r - last.Hi
			if last.Lo == last.Hi && (gap == 1 || gap == 2) {
				last.Stride, last.Hi = gap, r
				continue
			}
			if last.Lo != last.Hi && gap == last.Stride {
				last.Hi = r
				continue
			}
		}
		runs = append(runs, caseRun{Lo: r, Hi: r, Stride: 1, Delta: d})
	}
	return runs
}

// mappedRunes lists the runes a mapping changes.
func mappedRunes(mapping func(rune) rune) []rune {
	var out []rune
	for r := rune(0); r <= unicode.MaxRune; r++ {
		if mapping(r) != r {
			out = append(out, r)
		}
	}
	return out
}

// asciiBytes returns the bytes 0x00 to 0x7f.
func asciiBytes() []byte {
	b := make([]byte, 0x80)
	for i := range b {
		b[i] = byte(i)
	}
	return b
}

// appendValidRune appends the UTF-8 form of r when r is a valid rune.
func appendValidRune(b []byte, r rune) []byte {
	if utf8.ValidRune(r) {
		return utf8.AppendRune(b, r)
	}
	return b
}

// appendRuneBytes appends the UTF-8 form of r, and for a surrogate its invalid three-byte form, which
// utf8.AppendRune would replace with U+FFFD.
func appendRuneBytes(b []byte, r rune) []byte {
	if 0xD800 <= r && r <= 0xDFFF {
		return append(b, 0xE0|byte(r>>12), 0x80|byte(r>>6)&0x3F, 0x80|byte(r)&0x3F)
	}
	return utf8.AppendRune(b, r)
}

// hexOf copies b into a non-nil Hex, so that empty bytes marshal as "" rather than null.
func hexOf(b []byte) Hex {
	return Hex(append([]byte{}, b...))
}
