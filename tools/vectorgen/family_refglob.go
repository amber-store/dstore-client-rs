package main

// Family refglob: refglob/refglob.json, the reference-name glob of dstore
// v0.1.10 (refglob/refglob.go) that the Rust fake node's watch handler ports
// (dstore_testkit::refglob). Schema: docs/vectorgen-client.md.

import (
	"fmt"
	"path/filepath"
	"strings"
	"unicode/utf8"

	"github.com/amber-store/dstore/refglob"
)

func init() {
	register("refglob", []string{"refglob/refglob.json"}, genRefglob)
}

type refglobMatchCase struct {
	Pattern string `json:"pattern"`
	Name    string `json:"name"`
	Match   bool   `json:"match"`
}

type refglobPrefixCase struct {
	Pattern string `json:"pattern"`
	Prefix  string `json:"prefix"`
}

type refglobInvalidCase struct {
	// Pattern is absent when the pattern is not valid UTF-8; PatternHex always
	// holds its bytes.
	Pattern    *string `json:"pattern,omitempty"`
	PatternHex Hex     `json:"pattern_hex"`
	Error      string  `json:"error"`
}

type refglobFile struct {
	Match   []refglobMatchCase   `json:"match"`
	Prefix  []refglobPrefixCase  `json:"prefix"`
	Invalid []refglobInvalidCase `json:"invalid"`
}

// refglobTestMatch is refglob_test.go TestMatch, verbatim.
var refglobTestMatch = []refglobMatchCase{
	{"trees/a", "trees/a", true},
	{"trees/a", "trees/ab", false},
	{"trees/*", "trees/a", true},
	{"trees/*", "trees/a/b", false},
	{"trees/*", "trees/", true},
	{"trees/*", "trees", false},
	{"trees/**", "trees/a", true},
	{"trees/**", "trees/a/b", true},
	{"trees/**", "trees/", true},
	{"trees/**", "trees", false},
	{"**", "a", true},
	{"**", "a/b/c", true},
	{"**", "", true},
	{"**/x", "x", true},
	{"**/x", "a/x", true},
	{"**/x", "a/b/x", true},
	{"**/x", "ax", false},
	{"a/**/x", "a/x", true},
	{"a/**/x", "a/b/c/x", true},
	{"a/**/x", "ab/x", false},
	{"a**b", "axxb", true},
	{"a**b", "ax/xb", false},
	{"t?ees/a", "trees/a", true},
	{"t?ees/a", "t/ees/a", false},
	{"trees/[ab]", "trees/a", true},
	{"trees/[ab]", "trees/c", false},
	{"trees/[!ab]", "trees/c", true},
	{"trees/[^ab]", "trees/a", false},
	{"trees/[a-c]x", "trees/bx", true},
	{"trees/[/]", "trees//", false},
	{`trees/\*`, "trees/*", true},
	{`trees/\*`, "trees/a", false},
	{`trees/\\`, `trees/\`, true},
	{"a.b", "a.b", true},
	{"a.b", "axb", false},
	{"ünï/*", "ünï/cödé", true},
	{"*", "abc", true},
	{"*", "a/b", false},
}

// refglobTestPrefix is refglob_test.go TestPrefix, verbatim.
var refglobTestPrefix = []refglobPrefixCase{
	{"trees/a", "trees/a"},
	{"trees/*", "trees/"},
	{"trees/**", "trees/"},
	{"**", ""},
	{"tr?es/a", "tr"},
	{"trees/[ab]", "trees/"},
	{`trees/\*x`, "trees/*x"},
	{`a\\*`, `a\`},
}

// refglobTestInvalid is refglob_test.go TestInvalid, verbatim.
var refglobTestInvalid = []string{"", "trees/[ab", `trees/\`, "a[]b", "\x00", "[z-a]"}

// refglobNEL (U+0085) and refglobLS (U+2028) keep invisible runes out of the
// source.
var refglobNEL, refglobLS = string(rune(0x85)), string(rune(0x2028))

// refglobMoreMatch adds names for patterns the Go tests do not cover: every
// class form, escapes, double-star placement, non-ASCII, and names with a
// newline (RE2's `.` does not match '\n', `[^/]` does).
var refglobMoreMatch = []struct {
	pattern string
	names   []string
}{
	{"trees/**", []string{"trees/a\nb", "trees/a/b/c/", "trees//", "tree/a"}},
	{"**", []string{"/", "a\nb", "\n", "ü/ö"}},
	{"trees/*", []string{"trees/a\nb", "trees/ü", "trees/*"}},
	{"**/", []string{"", "a/", "a/b/", "a"}},
	{"/**/", []string{"/", "//", "/a/", "a/"}},
	{"a/**", []string{"a/", "a/b", "a", "ab", "a/\n", "a/b\nc"}},
	{"**/x", []string{"a\n/x", "a/\n/x", "\n"}},
	// C1 controls (U+0080-U+009F) and U+2028 are not refused: only runes
	// below 0x20 and 0x7f are.
	{"a" + refglobNEL + "*", []string{"a" + refglobNEL + "b", "a" + refglobNEL + "/b", "ab"}},
	{refglobLS + "/*", []string{refglobLS + "/x", "/x"}},
	{"a/**b", []string{"a/b", "a/xb", "a/x/b"}},
	{"**b", []string{"b", "xb", "x/b"}},
	{"a/***", []string{"a/", "a/x", "a/x/y"}},
	{"*/*", []string{"a/b", "/", "a/b/c", "ab"}},
	{"?", []string{"a", "ü", "/", "", "ab"}},
	{"t?", []string{"tü", "t/", "t"}},
	{"[a-]", []string{"a", "-", "b"}},
	{"[-a]", []string{"a", "-", "b"}},
	{`[\]]`, []string{"]", "["}},
	{`[a\-z]`, []string{"a", "-", "z", "b"}},
	{"[.-0]", []string{".", "0", "/"}},
	{"[!/]", []string{"a", "/"}},
	{"[^a-c]", []string{"d", "b", "/"}},
	{"[ä-ö]", []string{"ö", "ä", "a", "ü"}},
	{"[[]", []string{"["}},
	{`[\^]`, []string{"^", "a"}},
	{`\a\/b`, []string{"a/b", `a\/b`}},
	{"a\\/**", []string{"a/", "a/b/c"}},
	{"trees/a+b(c)|d$", []string{"trees/a+b(c)|d$", "trees/aab(c)|d"}},
	{"x{1,2}", []string{"x{1,2}", "x", "xx"}},
	{"trees/[ab]*", []string{"trees/a", "trees/bcd", "trees/c", "trees/a/b"}},
	{"trees/a?*/x", []string{"trees/ab/x", "trees/a/x", "trees/abc/x"}},
	{"*a*", []string{"a", "bab", "b/a", "ba/"}},
	{"a b", []string{"a b", "ab"}},
	{"ref-世界/*", []string{"ref-世界/x", "ref-世/x"}},
}

// refglobMorePrefix adds prefix cases.
var refglobMorePrefix = []string{
	"a\\/**", `\*`, "trees/a/[b]", "trees/a?", "[ab]", "*", "ünï/cödé", "a/b/c", `a\b`, "trees/**/x",
}

// refglobMoreInvalid adds every other refglob error text.
func refglobMoreInvalid() [][]byte {
	return [][]byte{
		[]byte(strings.Repeat("a", refglob.MaxLen+1)),
		{'a', 0xff, 'b'},
		{0xc3},
		[]byte("a\tb"),
		[]byte("a\nb"),
		[]byte("a\x7fb"),
		[]byte("a\x1f"),
		[]byte("\\"),
		[]byte("a[\\"),
		[]byte("[a-\\"),
		[]byte("["),
		[]byte("[!"),
		[]byte("[]"),
		[]byte("[!]"),
		[]byte("[^]"),
		[]byte("[!]a]"),
		[]byte("[b-a]"),
		[]byte("[ö-ä]"),
		[]byte("trees/[a-c"),
		[]byte("a[]"),
		// Validation order: empty, length, UTF-8, control characters, then
		// the pattern's syntax.
		append([]byte(strings.Repeat("a", refglob.MaxLen)), 0xff),
		append([]byte{0x01}, strings.Repeat("a", refglob.MaxLen)...),
		{0x01, 0xff},
		[]byte("[\x01"),
		[]byte("\\\x01"),
		[]byte("[b-a]\x00"),
	}
}

func genRefglob(out string) error {
	var f refglobFile
	compile := func(pattern string) (*refglob.Pattern, error) {
		p, err := refglob.Compile(pattern)
		if err != nil {
			return nil, fmt.Errorf("refglob: Compile(%q): %v", pattern, err)
		}
		if p.String() != pattern {
			return nil, fmt.Errorf("refglob: %q.String() = %q", pattern, p.String())
		}
		return p, nil
	}

	for _, c := range refglobTestMatch {
		p, err := compile(c.Pattern)
		if err != nil {
			return err
		}
		if got := p.Match(c.Name); got != c.Match {
			return fmt.Errorf("refglob: TestMatch %q.Match(%q) = %v, want %v", c.Pattern, c.Name, got, c.Match)
		}
		f.Match = append(f.Match, c)
	}
	for _, c := range refglobMoreMatch {
		p, err := compile(c.pattern)
		if err != nil {
			return err
		}
		for _, name := range c.names {
			f.Match = append(f.Match, refglobMatchCase{Pattern: c.pattern, Name: name, Match: p.Match(name)})
		}
	}

	for _, c := range refglobTestPrefix {
		p, err := compile(c.Pattern)
		if err != nil {
			return err
		}
		if got := p.Prefix(); got != c.Prefix {
			return fmt.Errorf("refglob: TestPrefix %q.Prefix() = %q, want %q", c.Pattern, got, c.Prefix)
		}
		f.Prefix = append(f.Prefix, c)
	}
	for _, pattern := range refglobMorePrefix {
		p, err := compile(pattern)
		if err != nil {
			return err
		}
		f.Prefix = append(f.Prefix, refglobPrefixCase{Pattern: pattern, Prefix: p.Prefix()})
	}
	// Every matching pattern above also contributes its prefix.
	seen := map[string]bool{}
	for _, c := range f.Prefix {
		seen[c.Pattern] = true
	}
	for _, c := range f.Match {
		if seen[c.Pattern] {
			continue
		}
		seen[c.Pattern] = true
		p, err := compile(c.Pattern)
		if err != nil {
			return err
		}
		f.Prefix = append(f.Prefix, refglobPrefixCase{Pattern: c.Pattern, Prefix: p.Prefix()})
	}

	invalid := make([][]byte, 0, len(refglobTestInvalid))
	for _, s := range refglobTestInvalid {
		invalid = append(invalid, []byte(s))
	}
	invalid = append(invalid, refglobMoreInvalid()...)
	for _, b := range invalid {
		_, err := refglob.Compile(string(b))
		if err == nil {
			return fmt.Errorf("refglob: Compile(%q) succeeded, want an error", b)
		}
		c := refglobInvalidCase{PatternHex: Hex(b), Error: err.Error()}
		if utf8.Valid(b) {
			s := string(b)
			c.Pattern = &s
		}
		f.Invalid = append(f.Invalid, c)
	}
	// The length bound is inclusive: exactly MaxLen bytes compiles.
	long := strings.Repeat("a", refglob.MaxLen)
	p, err := compile(long)
	if err != nil {
		return err
	}
	f.Match = append(f.Match, refglobMatchCase{Pattern: long, Name: long, Match: p.Match(long)})

	return writeJSON(filepath.Join(out, "refglob", "refglob.json"), f)
}
