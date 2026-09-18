package main

// Family udiff: the vectors of dstore-udiff (PORTING.md §4.9; port-notes/worktree.md §3.9 and §5
// items 5-7), produced by go-udiff v0.4.1 (Unified, ToUnified, Lines, lcs.DiffLines) and Go's
// sort.Slice. Schemas and conventions: docs/udiff.md.

import (
	"encoding/binary"
	"encoding/hex"
	"fmt"
	"hash/fnv"
	"path/filepath"
	"slices"
	"sort"
	"strings"
	"unicode/utf8"

	udiff "github.com/aymanbagabas/go-udiff"
	"github.com/aymanbagabas/go-udiff/difftest"
	"github.com/aymanbagabas/go-udiff/lcs"
)

func init() {
	register("udiff", []string{"udiff/lcs.json", "udiff/pdqsort.json", "udiff/udiff.json"}, genUdiff)
}

// udVerbatimOut is the largest random-case output written verbatim. Every random output also
// carries its length and FNV-1a digest.
const udVerbatimOut = 1024

// udOrderVerbatim is the largest pdqsort case whose permutation is written verbatim. Every case
// also carries the permutation's FNV-1a digest.
const udOrderVerbatim = 256

func genUdiff(out string) error {
	alphabets, err := udAlphabets()
	if err != nil {
		return err
	}
	recipes := udRandomRecipes(alphabets)
	files := []struct {
		name  string
		value any
	}{
		{"udiff.json", udiffJSON{
			Alphabets: alphabets,
			Cases:     udCases(),
			ToUnified: udToUnifiedCases(),
			Random:    udRandomCases(recipes, alphabets),
		}},
		{"lcs.json", lcsJSON{
			Alphabets: alphabets,
			Strings:   lcsStringCases(),
			Random:    lcsRandomCases(recipes, alphabets),
		}},
		{"pdqsort.json", pdqJSON{Cases: pdqCases()}},
	}
	for _, f := range files {
		if err := writeJSON(filepath.Join(out, "udiff", f.name), f.value); err != nil {
			return err
		}
	}
	return nil
}

// --- shared helpers ---

// udText returns b as a JSON text field: in the plain field when b is valid UTF-8, else hex in the
// field's _hex twin.
func udText(b []byte) (text, hexText *string) {
	if utf8.Valid(b) {
		s := string(b)
		return &s, nil
	}
	h := hex.EncodeToString(b)
	return nil, &h
}

// udFNV is the FNV-1a 64-bit digest of b.
func udFNV(b []byte) U64 {
	h := fnv.New64a()
	h.Write(b)
	return U64(h.Sum64())
}

// udFNVInts is the FNV-1a digest of ids written as 8-byte little-endian integers.
func udFNVInts(ids []int) U64 {
	b := make([]byte, 0, 8*len(ids))
	for _, id := range ids {
		b = binary.LittleEndian.AppendUint64(b, uint64(id))
	}
	return udFNV(b)
}

// udParams draws the parameters of generated cases from a splitmix64 stream.
type udParams struct{ state uint64 }

func (p *udParams) next() uint64 { return splitmixNext(&p.state) }

// intn returns a number in [lo, hi].
func (p *udParams) intn(lo, hi int) int { return lo + int(p.next()%uint64(hi-lo+1)) }

// strip returns "none", or with the given percentage one of "old", "new" and "both".
func (p *udParams) strip(percent int) string {
	if p.intn(0, 99) >= percent {
		return "none"
	}
	return []string{"old", "new", "both"}[p.intn(0, 2)]
}

// udNumbered returns "line <i>\n" for i in [from, to].
func udNumbered(from, to int) string {
	var b strings.Builder
	for i := from; i <= to; i++ {
		fmt.Fprintf(&b, "line %d\n", i)
	}
	return b.String()
}

// udEveryNth returns udNumbered(1, n) with every k-th line changed.
func udEveryNth(n, k int) string {
	var b strings.Builder
	for i := 1; i <= n; i++ {
		if i%k == 0 {
			fmt.Fprintf(&b, "changed %d\n", i)
		} else {
			fmt.Fprintf(&b, "line %d\n", i)
		}
	}
	return b.String()
}

// udGap returns two 30-line texts that differ in line 5 and line 6+n, so n unchanged lines lie
// between the two changes.
func udGap(n int) (before, after string) {
	lines := strings.SplitAfter(udNumbered(1, 30), "\n")
	before = strings.Join(lines, "")
	lines[4] = "changed 5\n"
	lines[5+n] = fmt.Sprintf("changed %d\n", 6+n)
	return before, strings.Join(lines, "")
}

// udSplitLines is go-udiff v0.4.1 unified.go splitLines, verbatim (unexported there): it gives
// the line sequences Lines passes to lcs.DiffLines.
func udSplitLines(text string) ([]string, []int) {
	var lines []string
	offsets := []int{0}
	start := 0
	for i, r := range text {
		if r == '\n' {
			lines = append(lines, text[start:i+1])
			start = i + 1
			offsets = append(offsets, start)
		}
	}
	if start < len(text) {
		lines = append(lines, text[start:])
		offsets = append(offsets, len(text))
	}
	return lines, offsets
}

// --- random line texts ---

// udAlphabet is a table of distinct lines that random recipes draw from.
type udAlphabet struct {
	Name  string `json:"name"`
	Lines []Hex  `json:"lines_hex"`
}

// udAlphabets returns the alphabets of random recipes. Each has 41 distinct lines ending in one
// newline.
func udAlphabets() ([]udAlphabet, error) {
	const letterSet = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNO"
	var letters, crlf, prefix, suffix []string
	for _, c := range letterSet {
		letters = append(letters, string(c)+"\n")
		crlf = append(crlf, string(c)+"\r\n")
	}
	for i := 0; i < 41; i++ {
		prefix = append(prefix, strings.Repeat("common prefix ", 20)+fmt.Sprintf("%02d\n", i))
		suffix = append(suffix, fmt.Sprintf("%02d", i)+strings.Repeat(" common suffix", 20)+"\n")
	}
	words := []string{
		"package main\n", "\n", "import (\n", "\t\"fmt\"\n", "\t\"os\"\n", ")\n", "func main() {\n",
		"}\n", "\treturn nil\n", "\tif err != nil {\n", "\t\treturn err\n", "\t}\n",
		"// Comment describing the next function.\n", "\tx := 1\n", "\tx++\n", "\tfmt.Println(x)\n",
		"type T struct {\n", "\tA int\n", "\tB string\n", "var v = []int{1, 2, 3}\n",
		"\tfor i := range v {\n", "\t\tv[i] *= 2\n", "\tswitch x {\n", "\tcase 1:\n", "\tdefault:\n",
		"\tdefer f.Close()\n", "const c = \"x\"\n", "\tgo func() {}()\n", "\tselect {}\n",
		"/* block comment */\n", "\t// TODO: handle the error\n", "\tpanic(\"unreachable\")\n",
		"\tlog.Fatal(err)\n", "\t\tbreak\n", "\t\tcontinue\n", "\tch <- v\n", "\t<-ch\n",
		"\tvar wg sync.WaitGroup\n", "\twg.Wait()\n", "\tmu.Lock()\n", "\tmu.Unlock()\n",
	}
	edge := []string{
		"\n", "\r\n", "\r\r\n", "a\rb\n", "\xff\n", "\xe2\x82\n", "\xed\xa0\x80\n", "\xc0\xaf\n",
		"\x00\n", "\t\n", " \n", "é\n", "日本語\n", "--x\n", "++y\n", "---\n", "+++\n", "--- a\n",
		"+++ b\n", "@@ -1 +1 @@\n", "\\ No newline at end of file\n", "\x7f\n", "\xe2\x80\xa8\n",
		"\"quoted\"\n", "<&>\n", "a\x00b\n", "\xef\xbb\xbfbom\n", "\xf0\x9f\x98\x80\n", "\\\n",
		"a\tb\n", "-\n", "+\n", " leading space\n", "trailing space \n", "\x1b[31mred\x1b[0m\n",
		"\xf4\x90\x80\x80\n", "\x80\n", "%s %d %q\n", "{\"json\": true}\n", "z\n", "y\n",
	}
	sets := []struct {
		name  string
		lines []string
	}{
		{"letters", letters}, {"words", words}, {"crlf", crlf}, {"edge", edge},
		{"prefix", prefix}, {"suffix", suffix},
	}
	var res []udAlphabet
	for _, s := range sets {
		a := udAlphabet{Name: s.name}
		seen := map[string]bool{}
		for _, l := range s.lines {
			if seen[l] || strings.Count(l, "\n") != 1 || !strings.HasSuffix(l, "\n") {
				return nil, fmt.Errorf("alphabet %s: line %q is repeated or is not one line", s.name, l)
			}
			seen[l] = true
			a.Lines = append(a.Lines, Hex(l))
		}
		if len(a.Lines) != 41 {
			return nil, fmt.Errorf("alphabet %s has %d lines, want 41", s.name, len(a.Lines))
		}
		res = append(res, a)
	}
	return res, nil
}

// udRecipe describes a random old/new text pair; docs/udiff.md "Random recipes" defines generate.
type udRecipe struct {
	Seed     U64    `json:"seed"`
	Alphabet int    `json:"alphabet"` // index into alphabets, or -1 for random "%016x\n" lines
	Distinct int    `json:"distinct"` // lines drawn from alphabet[:distinct]
	Lines    int    `json:"lines"`
	Mode     string `json:"mode"`      // "independent" or "edits"
	NewLines int    `json:"new_lines"` // independent: lines of the new text
	Edits    int    `json:"edits"`     // edits: number of edit operations
	MaxRun   int    `json:"max_run"`   // edits: maximal lines per operation
	StripEOL string `json:"strip_eol"` // "none", "old", "new" or "both"
}

// generate produces the old and new texts of a recipe.
func (r udRecipe) generate(alphabets []udAlphabet) (before, after []byte) {
	state := uint64(r.Seed)
	value := func() uint64 {
		v := splitmixNext(&state)
		if r.Alphabet >= 0 {
			v %= uint64(r.Distinct)
		}
		return v
	}
	oldVals := make([]uint64, r.Lines)
	for i := range oldVals {
		oldVals[i] = value()
	}
	var newVals []uint64
	switch r.Mode {
	case "independent":
		newVals = make([]uint64, r.NewLines)
		for i := range newVals {
			newVals[i] = value()
		}
	case "edits":
		newVals = oldVals
		for e := 0; e < r.Edits; e++ {
			op := splitmixNext(&state) % 3 // 0 insert, 1 delete, 2 replace
			pos := int(splitmixNext(&state) % uint64(len(newVals)+1))
			run := 1 + int(splitmixNext(&state)%uint64(r.MaxRun))
			del := 0
			if op != 0 {
				del = min(run, len(newVals)-pos)
			}
			var ins []uint64
			if op != 1 {
				ins = make([]uint64, run)
				for i := range ins {
					ins[i] = value()
				}
			}
			spliced := make([]uint64, 0, len(newVals)-del+len(ins))
			spliced = append(spliced, newVals[:pos]...)
			spliced = append(spliced, ins...)
			newVals = append(spliced, newVals[pos+del:]...)
		}
	default:
		panic("udiff recipe: unknown mode " + r.Mode)
	}
	before, after = r.render(alphabets, oldVals), r.render(alphabets, newVals)
	if r.StripEOL == "old" || r.StripEOL == "both" {
		before = udStripEOL(before)
	}
	if r.StripEOL == "new" || r.StripEOL == "both" {
		after = udStripEOL(after)
	}
	return before, after
}

func (r udRecipe) render(alphabets []udAlphabet, vals []uint64) []byte {
	var b []byte
	for _, v := range vals {
		if r.Alphabet < 0 {
			b = fmt.Appendf(b, "%016x\n", v)
		} else {
			b = append(b, alphabets[r.Alphabet].Lines[v]...)
		}
	}
	return b
}

// udStripEOL removes one final newline byte.
func udStripEOL(b []byte) []byte {
	if len(b) > 0 && b[len(b)-1] == '\n' {
		return b[:len(b)-1]
	}
	return b
}

// udNamed is a named recipe.
type udNamed struct {
	Name   string
	Recipe udRecipe
}

// udRandomRecipes returns the random cases, class by class.
func udRandomRecipes(alphabets []udAlphabet) []udNamed {
	p := &udParams{state: 0x7564696666} // "udiff"
	var res []udNamed
	class := func(name string, n int, pick func(i int) udRecipe) {
		for i := 0; i < n; i++ {
			r := pick(i)
			r.Seed = U64(p.next())
			res = append(res, udNamed{Name: fmt.Sprintf("%s/%04d", name, i), Recipe: r})
		}
	}
	anyAlphabet := func(r *udRecipe, minDistinct, maxDistinct int) {
		r.Alphabet = p.intn(0, len(alphabets)-1)
		r.Distinct = p.intn(minDistinct, min(maxDistinct, len(alphabets[r.Alphabet].Lines)))
	}
	// The shape of the evidence run of port-notes/worktree.md §3.9: 50-450 lines on each side,
	// drawn from 2-41 distinct lines. Most of these reach the search limit and lcs.fix.
	heavy := func(alphabet int) func(int) udRecipe {
		return func(int) udRecipe {
			r := udRecipe{Alphabet: alphabet, Mode: "independent"}
			r.Distinct = p.intn(2, 41)
			r.Lines = p.intn(50, 450)
			r.NewLines = p.intn(50, 450)
			r.StripEOL = p.strip(10)
			return r
		}
	}
	class("heavy", 1500, heavy(0))
	class("heavy_words", 150, heavy(1))
	class("edits", 600, func(int) udRecipe {
		r := udRecipe{Mode: "edits"}
		anyAlphabet(&r, 1, 41)
		r.Lines = p.intn(0, 450)
		r.Edits = p.intn(1, 10)
		r.MaxRun = p.intn(1, 6)
		r.StripEOL = p.strip(20)
		return r
	})
	class("heavy_edits", 500, func(int) udRecipe {
		r := udRecipe{Mode: "edits"}
		anyAlphabet(&r, 2, 41)
		r.Lines = p.intn(50, 450)
		r.Edits = p.intn(20, 150)
		r.MaxRun = p.intn(1, 12)
		r.StripEOL = p.strip(10)
		return r
	})
	class("small", 600, func(int) udRecipe {
		var r udRecipe
		anyAlphabet(&r, 1, 6)
		r.Lines = p.intn(0, 15)
		if p.intn(0, 1) == 0 {
			r.Mode = "independent"
			r.NewLines = p.intn(0, 15)
		} else {
			r.Mode = "edits"
			r.Edits = p.intn(0, 4)
			r.MaxRun = p.intn(1, 3)
		}
		r.StripEOL = p.strip(40)
		return r
	})
	// Unique random lines with scattered edits.
	class("hex", 40, func(int) udRecipe {
		r := udRecipe{Alphabet: -1, Mode: "edits"}
		r.Lines = p.intn(100, 2000)
		r.Edits = p.intn(1, 40)
		r.MaxRun = p.intn(1, 5)
		r.StripEOL = p.strip(10)
		return r
	})
	// Near worktree.MaxDiffBytes (16 MiB): 986895 lines of 17 bytes are 16777215 bytes.
	class("huge", 3, func(i int) udRecipe {
		return udRecipe{
			Alphabet: -1,
			Lines:    986895,
			Mode:     "edits",
			Edits:    []int{0, 3, 8}[i],
			MaxRun:   3,
			StripEOL: []string{"none", "new", "old"}[i],
		}
	})
	return res
}

// --- udiff/udiff.json ---

type udiffJSON struct {
	Alphabets []udAlphabet   `json:"alphabets"`
	Cases     []udCase       `json:"cases"`
	ToUnified []udToUnified  `json:"to_unified"`
	Random    []udRandomCase `json:"random"`
}

// udEdit is a udiff.Edit.
type udEdit struct {
	Start  int     `json:"start"`
	End    int     `json:"end"`
	New    *string `json:"new,omitempty"`
	NewHex *string `json:"new_hex,omitempty"`
}

func udEdits(edits []udiff.Edit) []udEdit {
	res := make([]udEdit, len(edits))
	for i, e := range edits {
		res[i] = udEdit{Start: e.Start, End: e.End}
		res[i].New, res[i].NewHex = udText([]byte(e.New))
	}
	return res
}

// udCase is udiff.Lines(Old, New) and udiff.Unified(OldLabel, NewLabel, Old, New).
type udCase struct {
	Name        string   `json:"name"`
	OldLabel    *string  `json:"old_label,omitempty"`
	OldLabelHex *string  `json:"old_label_hex,omitempty"`
	NewLabel    *string  `json:"new_label,omitempty"`
	NewLabelHex *string  `json:"new_label_hex,omitempty"`
	Old         *string  `json:"old,omitempty"`
	OldHex      *string  `json:"old_hex,omitempty"`
	New         *string  `json:"new,omitempty"`
	NewHex      *string  `json:"new_hex,omitempty"`
	Edits       []udEdit `json:"edits"`
	Out         *string  `json:"out,omitempty"`
	OutHex      *string  `json:"out_hex,omitempty"`
}

func udCases() []udCase {
	var cases []udCase
	add := func(name, oldLabel, newLabel, before, after string) {
		c := udCase{Name: name}
		c.OldLabel, c.OldLabelHex = udText([]byte(oldLabel))
		c.NewLabel, c.NewLabelHex = udText([]byte(newLabel))
		c.Old, c.OldHex = udText([]byte(before))
		c.New, c.NewHex = udText([]byte(after))
		c.Edits = udEdits(udiff.Lines(before, after))
		c.Out, c.OutHex = udText([]byte(udiff.Unified(oldLabel, newLabel, before, after)))
		cases = append(cases, c)
	}

	// port-notes/verification.md §3.4 probes and port-notes/worktree.md §3.7 samples.
	add("probe/edit", "a/edit.txt", "b/edit.txt", "one\ntwo\nthree\n", "one\n2\nthree\n")
	add("probe/link", "a/link", "b/link", "t1", "t2")
	add("probe/new_file", "/dev/null", "b/new.txt", "", "hi\n")
	add("sample/no_newline_growth", "a/x", "b/x", "a", "a\nb")
	add("sample/added", "/dev/null", "b/x", "", "hi\n")
	add("sample/deleted", "a/x", "/dev/null", "bye\n", "")

	// go-udiff difftest.TestCases.
	for _, tc := range difftest.TestCases {
		add("difftest/"+tc.Name, difftest.FileA, difftest.FileB, tc.In, tc.Out)
	}

	add("identical/empty", "a/f", "b/f", "", "")
	add("identical/line", "a/f", "b/f", "x\n", "x\n")
	add("identical/no_newline", "a/f", "b/f", "x", "x")
	add("identical/many_lines", "a/f", "b/f", udNumbered(1, 300), udNumbered(1, 300))
	add("empty_old/lines", "a/f", "b/f", "", "a\nb\n")
	add("empty_old/no_newline", "a/f", "b/f", "", "a\nb")
	add("empty_old/newline_only", "a/f", "b/f", "", "\n")
	add("empty_new/lines", "a/f", "b/f", "a\nb\n", "")
	add("empty_new/no_newline", "a/f", "b/f", "a\nb", "")
	add("empty_new/newline_only", "a/f", "b/f", "\n", "")
	add("no_newline/old_only", "a/f", "b/f", "a\nb\nc", "a\nb\nc\n")
	add("no_newline/new_only", "a/f", "b/f", "a\nb\nc\n", "a\nb\nc")
	add("no_newline/both_last_line_changed", "a/f", "b/f", "a\nb\nc", "a\nb\nd")
	add("no_newline/both_middle_changed", "a/f", "b/f", "a\nx\nc", "a\ny\nc")
	add("no_newline/far_from_change", "a/f", "b/f", udNumbered(1, 20)+"end",
		strings.Replace(udNumbered(1, 20), "line 3\n", "LINE 3\n", 1)+"end")
	add("eof/insert_after_partial_line", "a/f", "b/f", "a\nb", "a\nb\nc")
	add("eof/extend_partial_line", "a/f", "b/f", "a\nb", "a\nbc")
	add("eof/insert_partial_after_newline", "a/f", "b/f", "a\nb\n", "a\nb\nc")
	add("eof/insert_line_after_newline", "a/f", "b/f", "a\nb\n", "a\nb\nc\n")
	for n := 4; n <= 8; n++ {
		before, after := udGap(n)
		add(fmt.Sprintf("gap/%d_unchanged_lines", n), "a/f", "b/f", before, after)
	}
	lines10 := udNumbered(1, 10)
	add("position/first_line", "a/f", "b/f", lines10, strings.Replace(lines10, "line 1\n", "first\n", 1))
	add("position/last_line", "a/f", "b/f", lines10, strings.Replace(lines10, "line 10\n", "last\n", 1))
	add("position/first_and_last_line", "a/f", "b/f", lines10, "first\n"+udNumbered(2, 9)+"last\n")
	add("position/insert_before_first", "a/f", "b/f", lines10, "new\n"+lines10)
	add("position/append_after_last", "a/f", "b/f", lines10, lines10+"new\n")
	add("crlf/replace", "a/f", "b/f", "a\r\nb\r\nc\r\n", "a\r\nB\r\nc\r\n")
	add("crlf/mixed_endings", "a/f", "b/f", "a\r\nb\nc\r\n", "a\nb\r\nc\r\n")
	add("crlf/cr_without_newline", "a/f", "b/f", "a\r\nb\r", "a\r\nb\r\n")
	add("cr/crlf_only_lines", "a/f", "b/f", "\r\n\r\n", "\r\n")
	add("cr/bare_cr_single_line", "a/f", "b/f", "a\rb\rc", "a\rb\rd")
	add("cr/cr_line_removed", "a/f", "b/f", "a\n\r\nb\n", "a\nb\n")
	add("tie/insert_in_repeated_lines", "a/f", "b/f", strings.Repeat("a\n", 100),
		strings.Repeat("a\n", 50)+"b\n"+strings.Repeat("a\n", 50))
	add("tie/delete_from_repeated_lines", "a/f", "b/f", strings.Repeat("a\n", 100), strings.Repeat("a\n", 99))
	add("tie/replace_in_repeated_lines", "a/f", "b/f", strings.Repeat("a\n", 100),
		strings.Repeat("a\n", 30)+"b\n"+strings.Repeat("a\n", 69))
	add("unicode/lines", "a/f", "b/f", "héllo\n日本語\nemoji 😀\n", "héllo\n日本\nemoji 😀\n")
	add("invalid_utf8/lines", "a/f", "b/f", "\xff\n\xe2\x82\n\xed\xa0\x80\n", "\xff\n\xc0\xaf\n\xed\xa0\x80\n")
	add("invalid_utf8/no_newline", "a/f", "b/f", "ok\n\xff", "ok\n\xfe")
	add("invalid_utf8/labels", "a/\xff", "b/\xfe", "x\n", "y\n")
	add("markers/diff_like_lines", "a/f", "b/f", "--- a\n+++ b\n@@ -1 +1 @@\n",
		"--- a\n+++ c\n@@ -1 +1 @@\n\\ No newline at end of file\n")
	add("markers/stat_miscount_lines", "a/f", "b/f", "--x\nkeep\n", "keep\n++y\n")
	add("newlines/fewer", "a/f", "b/f", "\n\n\n", "\n\n")
	add("newlines/to_empty", "a/f", "b/f", "\n", "")
	add("newlines/empty_line_inserted", "a/f", "b/f", "a\nb\n", "a\n\nb\n")
	add("prefix/long_common_prefix", "a/f", "b/f", udNumbered(1, 1000)+"old tail\n", udNumbered(1, 1000)+"new tail\n")
	add("suffix/long_common_suffix", "a/f", "b/f", "old head\n"+udNumbered(1, 1000), "new head\n"+udNumbered(1, 1000))
	add("prefix/long_line", "a/f", "b/f", strings.Repeat("x", 5000)+"1\n", strings.Repeat("x", 5000)+"2\n")
	add("replace/whole_file", "a/f", "b/f", udNumbered(1, 30), udNumbered(31, 60))
	add("limit/unrelated_files", "a/f", "b/f", udNumbered(1, 200), udNumbered(1001, 1200))
	add("limit/every_other_line", "a/f", "b/f", udNumbered(1, 150), udEveryNth(150, 2))
	add("hunks/every_10th_line", "a/f", "b/f", udNumbered(1, 100), udEveryNth(100, 10))
	add("hunks/every_7th_line", "a/f", "b/f", udNumbered(1, 100), udEveryNth(100, 7))
	add("labels/empty", "", "", "a\n", "b\n")
	add("labels/spaces_and_tab", "a/with space\tand tab", "b/with space\tand tab", "a\n", "b\n")
	add("binary/nul_bytes", "a/f", "b/f", "a\x00b\n", "a\x00c\n")
	return cases
}

// udToUnified is udiff.ToUnified(OldLabel, NewLabel, Content, Edits, ContextLines): Out, or Error.
type udToUnified struct {
	Name         string   `json:"name"`
	OldLabel     string   `json:"old_label"`
	NewLabel     string   `json:"new_label"`
	Content      *string  `json:"content,omitempty"`
	ContentHex   *string  `json:"content_hex,omitempty"`
	Edits        []udEdit `json:"edits"`
	ContextLines int      `json:"context_lines"`
	Out          *string  `json:"out,omitempty"`
	OutHex       *string  `json:"out_hex,omitempty"`
	Error        *string  `json:"error,omitempty"`
}

func udToUnifiedCases() []udToUnified {
	var cases []udToUnified
	add := func(name, content string, edits []udiff.Edit, contextLines int) {
		c := udToUnified{Name: name, OldLabel: difftest.FileA, NewLabel: difftest.FileB, ContextLines: contextLines}
		c.Content, c.ContentHex = udText([]byte(content))
		c.Edits = udEdits(edits)
		out, err := udiff.ToUnified(c.OldLabel, c.NewLabel, content, edits, contextLines)
		if err != nil {
			msg := err.Error()
			c.Error = &msg
		} else {
			c.Out, c.OutHex = udText([]byte(out))
		}
		cases = append(cases, c)
	}
	ed := func(start, end int, repl string) udiff.Edit { return udiff.Edit{Start: start, End: end, New: repl} }

	// go-udiff difftest.TestCases: character-level Edits (expanded by lineEdits) and LineEdits.
	for _, tc := range difftest.TestCases {
		add("difftest/"+tc.Name+"/edits", tc.In, tc.Edits, udiff.DefaultContextLines)
		if tc.LineEdits != nil {
			add("difftest/"+tc.Name+"/line_edits", tc.In, tc.LineEdits, udiff.DefaultContextLines)
		}
	}
	before, after := udNumbered(1, 40), udEveryNth(40, 9)
	for _, n := range []int{0, 1, 2, 3, 4, 5, 10, 100} {
		add(fmt.Sprintf("context/%d", n), before, udiff.Lines(before, after), n)
	}
	add("context/0/pure_insertion", "a\nb\n", []udiff.Edit{ed(2, 2, "x\n")}, 0)
	add("validate/unsorted", "A\nB\nC\nD\nE\nF\nG\n", []udiff.Edit{ed(12, 14, "K\n"), ed(2, 8, "H\nI\nJ\n")}, 3)
	add("validate/insertions_at_one_point", "a\nb\n", []udiff.Edit{ed(2, 2, "x\n"), ed(2, 2, "y\n")}, 3)
	add("validate/insertion_sorted_before_deletion", "a\nb\nc\n", []udiff.Edit{ed(2, 4, ""), ed(2, 2, "x\n")}, 3)
	add("validate/end_out_of_bounds", "a\n", []udiff.Edit{ed(0, 3, "x")}, 3)
	add("validate/start_after_end", "a\nb\n", []udiff.Edit{ed(3, 2, "")}, 3)
	add("validate/overlapping", "abcdef\n", []udiff.Edit{ed(0, 4, "x"), ed(2, 6, "y")}, 3)
	add("validate/overlapping_after_sort", "abcdef\n", []udiff.Edit{ed(3, 5, "x"), ed(0, 4, "y")}, 3)
	add("no_edits", "a\n", nil, 3)
	add("no_edits/empty_content", "", nil, 3)
	add("line_edits/merge_on_one_line", "abc\ndef\n", []udiff.Edit{ed(1, 2, "X"), ed(2, 3, "Y")}, 3)
	add("line_edits/two_partial_lines", "abc\ndef\nghi\n", []udiff.Edit{ed(1, 2, "X"), ed(5, 6, "Y")}, 3)
	add("line_edits/insert_at_eof", "a\nb\n", []udiff.Edit{ed(4, 4, "c\n")}, 3)
	add("line_edits/insert_at_eof_without_newline", "a\nb", []udiff.Edit{ed(3, 3, "c")}, 3)
	add("line_edits/partial_insertion", "a\nb\n", []udiff.Edit{ed(2, 2, "x")}, 3)
	add("line_edits/delete_newline", "a\nb\nc\n", []udiff.Edit{ed(1, 2, "")}, 3)
	add("line_edits/multi_line_replacement", "a\nb\nc\n", []udiff.Edit{ed(2, 3, "x\ny\nz")}, 3)
	add("line_edits/invalid_utf8", "\xff\xfe\n", []udiff.Edit{ed(1, 2, "\x80")}, 3)
	return cases
}

// udRandomCase is a random recipe with the digests of its inputs and of
// udiff.Unified(OldLabel, NewLabel, old, new).
type udRandomCase struct {
	Name     string   `json:"name"`
	OldLabel string   `json:"old_label"`
	NewLabel string   `json:"new_label"`
	Recipe   udRecipe `json:"recipe"`
	OldLen   int      `json:"old_len"`
	OldFNV   U64      `json:"old_fnv1a64"`
	NewLen   int      `json:"new_len"`
	NewFNV   U64      `json:"new_fnv1a64"`
	Edits    int      `json:"edits"` // len(udiff.Lines(old, new))
	OutLen   int      `json:"out_len"`
	OutFNV   U64      `json:"out_fnv1a64"`
	Out      *string  `json:"out,omitempty"`
	OutHex   *string  `json:"out_hex,omitempty"`
}

func udRandomCases(recipes []udNamed, alphabets []udAlphabet) []udRandomCase {
	res := make([]udRandomCase, len(recipes))
	for i, n := range recipes {
		before, after := n.Recipe.generate(alphabets)
		c := udRandomCase{Name: n.Name, OldLabel: "a/" + n.Name, NewLabel: "b/" + n.Name, Recipe: n.Recipe}
		c.OldLen, c.OldFNV = len(before), udFNV(before)
		c.NewLen, c.NewFNV = len(after), udFNV(after)
		c.Edits = len(udiff.Lines(string(before), string(after)))
		out := []byte(udiff.Unified(c.OldLabel, c.NewLabel, string(before), string(after)))
		c.OutLen, c.OutFNV = len(out), udFNV(out)
		if len(out) <= udVerbatimOut {
			c.Out, c.OutHex = udText(out)
		}
		res[i] = c
	}
	return res
}

// --- udiff/lcs.json ---

type lcsJSON struct {
	Alphabets []udAlphabet `json:"alphabets"`
	Strings   []lcsString  `json:"strings"`
	Random    []lcsRandom  `json:"random"`
}

// lcsString is lcs.DiffLines over the bytes of A and B, each byte one element.
type lcsString struct {
	Name  string   `json:"name"`
	A     string   `json:"a"`
	B     string   `json:"b"`
	Diffs [][4]int `json:"diffs"`
}

// lcsRandom is lcs.DiffLines over the lines (splitLines) of a random recipe's texts.
type lcsRandom struct {
	Name     string   `json:"name"`
	Recipe   udRecipe `json:"recipe"`
	OldLines int      `json:"old_lines"`
	NewLines int      `json:"new_lines"`
	Diffs    [][4]int `json:"diffs"`
}

func lcsDiffs(diffs []lcs.Diff) [][4]int {
	res := make([][4]int, len(diffs))
	for i, d := range diffs {
		res[i] = [4]int{d.Start, d.End, d.ReplStart, d.ReplEnd}
	}
	return res
}

func lcsBytes(s string) []string {
	res := make([]string, len(s))
	for i := range len(s) {
		res[i] = s[i : i+1]
	}
	return res
}

func lcsStringCases() []lcsString {
	var res []lcsString
	add := func(name, a, b string) {
		res = append(res, lcsString{Name: name, A: a, B: b, Diffs: lcsDiffs(lcs.DiffLines(lcsBytes(a), lcsBytes(b)))})
	}
	// lcs/common_test.go Btests, both ways, and TestIntOld's fills.
	btests := [][2]string{
		{"aaabab", "abaab"}, {"aabbba", "baaba"}, {"cabbx", "cbabx"}, {"c", "cb"}, {"aaba", "bbb"},
		{"bbaabb", "b"}, {"baaabb", "bbaba"}, {"baaabb", "abbab"}, {"baaba", "aaabba"}, {"ca", "cba"},
		{"ccbcbc", "abba"}, {"ccbcbc", "aabba"}, {"ccb", "cba"}, {"caef", "axe"}, {"bbaabb", "baabb"},
		{"abcabba", "cbabac"}, {"3456aaa", "aaa"}, {"aaa", "aaa123"}, {"aabaa", "aacaa"}, {"1a", "a"},
		{"abab", "bb"}, {"123", "ab"}, {"a", "b"}, {"abc", "123"}, {"aa", "aa"}, {"abcde", "12345"},
		{"aaa3456", "aaa"}, {"abcde", "12345a"}, {"ab", "123"}, {"1a2", "a"}, {"babaab", "cccaba"},
		{"aabbab", "cbcabc"}, {"abaabb", "bcacab"}, {"abaabb", "abaaaa"}, {"bababb", "baaabb"},
		{"abbbaa", "cabacc"}, {"aabbaa", "aacaba"},
	}
	const lfill, rfill = "AAAAAAAAAAAA", "BBBBBBBBBBBB"
	for i, t := range btests {
		name := fmt.Sprintf("btests/%02d", i)
		add(name, t[0], t[1])
		add(name+"/swapped", t[1], t[0])
		if len(t[0]) >= 2 && len(t[1]) >= 2 {
			add(name+"/fill_right", t[0]+lfill, t[1]+rfill)
			add(name+"/fill_right/swapped", t[1]+rfill, t[0]+lfill)
			add(name+"/fill_left", lfill+t[0], rfill+t[1])
			add(name+"/fill_left/swapped", rfill+t[1], lfill+t[0])
		}
	}
	add("special_old", "golang.org/x/tools/intern", "github.com/google/safehtml/template\"\n\t\"golang.org/x/tools/intern")
	add("regression_old_001",
		"// Copyright 2019 The Go Authors. All rights reserved.\n// Use of this source code is governed by a BSD-style\n// license that can be found in the LICENSE file.\n\npackage diff_test\n\nimport (\n\t\"fmt\"\n\t\"math/rand\"\n\t\"strings\"\n\t\"testing\"\n\n\t\"golang.org/x/tools/gopls/internal/lsp/diff\"\n\t\"github.com/aymanbagabas/go-udiff/difftest\"\n\t\"golang.org/x/tools/gopls/internal/span\"\n)\n",
		"// Copyright 2019 The Go Authors. All rights reserved.\n// Use of this source code is governed by a BSD-style\n// license that can be found in the LICENSE file.\n\npackage diff_test\n\nimport (\n\t\"fmt\"\n\t\"math/rand\"\n\t\"strings\"\n\t\"testing\"\n\n\t\"github.com/google/safehtml/template\"\n\t\"golang.org/x/tools/gopls/internal/lsp/diff\"\n\t\"github.com/aymanbagabas/go-udiff/difftest\"\n\t\"golang.org/x/tools/gopls/internal/span\"\n)\n")
	add("regression_old_002", "n\"\n)\n", "n\"\n\t\"golang.org/x//nnal/stack\"\n)\n")
	add("regression_old_003", "golang.org/x/hello v1.0.0\nrequire golang.org/x/unused v1", "golang.org/x/hello v1")
	add("diff_api/ascii", "abcXdef", "abcxdef")
	add("diff_api/non_ascii", "abcωdef", "abcΩdef")
	add("empty/both", "", "")
	add("empty/a", "", "abc")
	add("empty/b", "abc", "")
	add("limit/unrelated", strings.Repeat("abc", 60), strings.Repeat("xyz", 60))
	add("limit/shared_prefix_and_suffix", "head"+strings.Repeat("ab", 80)+"tail", "head"+strings.Repeat("ba", 70)+"tail")
	return res
}

// lcsRandomCases takes the first recipes of each random class.
func lcsRandomCases(recipes []udNamed, alphabets []udAlphabet) []lcsRandom {
	quotas := []struct {
		class string
		n     int
	}{{"heavy", 150}, {"heavy_words", 20}, {"edits", 80}, {"heavy_edits", 80}, {"small", 150}, {"hex", 10}}
	var res []lcsRandom
	for _, q := range quotas {
		taken := 0
		for _, n := range recipes {
			if taken == q.n {
				break
			}
			if !strings.HasPrefix(n.Name, q.class+"/") {
				continue
			}
			before, after := n.Recipe.generate(alphabets)
			a, _ := udSplitLines(string(before))
			b, _ := udSplitLines(string(after))
			res = append(res, lcsRandom{
				Name:     n.Name,
				Recipe:   n.Recipe,
				OldLines: len(a),
				NewLines: len(b),
				Diffs:    lcsDiffs(lcs.DiffLines(a, b)),
			})
			taken++
		}
	}
	return res
}

// --- udiff/pdqsort.json ---

type pdqJSON struct {
	Cases []pdqCase `json:"cases"`
}

// pdqCase is sort.Slice over the elements of a recipe (docs/udiff.md "Sort recipes"): Order holds
// the element ids in sorted order.
type pdqCase struct {
	Name     string `json:"name"`
	N        int    `json:"n"`
	Pattern  string `json:"pattern"`
	Modulus  int    `json:"modulus"`
	Swaps    int    `json:"swaps"`
	Seed     U64    `json:"seed"`
	Less     string `json:"less"`             // "len_desc" (lcs.fix), "x_asc_len_desc" (lcs.sort) or a coin
	Values   []int  `json:"values,omitempty"` // pattern "adversary" only: ranks below n, so JSON numbers
	Order    []int  `json:"order"`
	OrderFNV U64    `json:"order_fnv1a64"`
}

type pdqElem struct {
	X, Len uint64
	ID     int
}

// pdqAdversary is M. Douglas McIlroy's antiquicksort adversary (go1.26.5 sort/sort_test.go
// adversaryTestingData), extended with the original index of the element at each position.
type pdqAdversary struct {
	data      []int // item values, initialized to special gas value and changed by Less
	ids       []int // original index of the element at each position
	nsolid    int   // number of elements that have been set to non-gas values
	candidate int   // guess at current pivot
	gas       int   // special value for unset elements, higher than everything else
}

func (d *pdqAdversary) Len() int { return len(d.data) }

func (d *pdqAdversary) Less(i, j int) bool {
	if d.data[i] == d.gas && d.data[j] == d.gas {
		if i == d.candidate {
			// freeze i
			d.data[i] = d.nsolid
			d.nsolid++
		} else {
			// freeze j
			d.data[j] = d.nsolid
			d.nsolid++
		}
	}
	if d.data[i] == d.gas {
		d.candidate = i
	} else if d.data[j] == d.gas {
		d.candidate = j
	}
	return d.data[i] < d.data[j]
}

func (d *pdqAdversary) Swap(i, j int) {
	d.data[i], d.data[j] = d.data[j], d.data[i]
	d.ids[i], d.ids[j] = d.ids[j], d.ids[i]
}

// pdqAdversaryValues returns the values of the adversary's pattern for n elements under the given
// comparator, or nil for any other pattern. It runs the adversary through sort.Sort (the same
// pdqsort as sort.Slice) and takes the value each element was frozen to. Every answer the adversary
// gave agrees with those values, so sorting them as a fixed input repeats its comparisons exactly:
// an input tuned against pdqsort's pivot choices. (go1.26.5 still avoids its heapsort fallback on
// it; the coin_15_16 comparator reaches that.) len_desc orders larger values first, so it gets
// n-1-value.
func pdqAdversaryValues(pattern string, n int, less string) []int {
	if pattern != "adversary" {
		return nil
	}
	d := &pdqAdversary{data: make([]int, n), ids: make([]int, n), gas: n - 1}
	for i := range d.data {
		d.data[i] = d.gas
		d.ids[i] = i
	}
	sort.Sort(d)
	values := make([]int, n)
	for pos, id := range d.ids {
		values[id] = d.data[pos]
		if less == "len_desc" {
			values[id] = n - 1 - values[id]
		}
	}
	return values
}

func (c pdqCase) elements() []pdqElem {
	state := uint64(c.Seed)
	n, m := c.N, uint64(c.Modulus)
	v := make([]uint64, n)
	for i := range v {
		switch c.Pattern {
		case "random":
			v[i] = splitmixNext(&state) % m
		case "asc", "asc_swaps":
			v[i] = uint64(i) / m
		case "desc", "desc_swaps":
			v[i] = uint64(n-1-i) / m
		case "equal":
			v[i] = 0
		case "saw":
			v[i] = uint64(i) % m
		case "organ":
			v[i] = uint64(min(i, n-1-i)) / m
		case "adversary":
			v[i] = uint64(c.Values[i])
		default:
			panic("pdqsort recipe: unknown pattern " + c.Pattern)
		}
	}
	if strings.HasSuffix(c.Pattern, "_swaps") && n > 0 {
		for s := 0; s < c.Swaps; s++ {
			p := splitmixNext(&state) % uint64(n)
			q := splitmixNext(&state) % uint64(n)
			v[p], v[q] = v[q], v[p]
		}
	}
	els := make([]pdqElem, n)
	for i := range els {
		switch c.Less {
		case "len_desc", "coin_1_2", "coin_1_16", "coin_15_16":
			els[i] = pdqElem{Len: v[i], ID: i}
		case "x_asc_len_desc":
			els[i] = pdqElem{X: v[i], Len: splitmixNext(&state) % 3, ID: i}
		default:
			panic("pdqsort recipe: unknown less " + c.Less)
		}
	}
	return els
}

func (c pdqCase) sortedIDs() []int {
	els := c.elements()
	switch c.Less {
	case "len_desc":
		sort.Slice(els, func(i, j int) bool { return els[i].Len > els[j].Len })
	case "x_asc_len_desc":
		sort.Slice(els, func(i, j int) bool {
			if els[i].X != els[j].X {
				return els[i].X < els[j].X
			}
			return els[i].Len > els[j].Len
		})
	default:
		// A seeded coin instead of a comparison: "less" with probability 1/2, 1/16 or 15/16, from
		// its own splitmix64 stream starting at the case seed. The permutation depends on the exact
		// sequence of less calls, and coin_15_16 unbalances every partition, which leads to
		// breakPatterns and the heapsort fallback.
		state := uint64(c.Seed)
		sort.Slice(els, func(i, j int) bool {
			v := splitmixNext(&state)
			switch c.Less {
			case "coin_1_2":
				return v%2 == 0
			case "coin_1_16":
				return v%16 == 0
			default:
				return v%16 != 0
			}
		})
	}
	ids := make([]int, len(els))
	for i, e := range els {
		ids[i] = e.ID
	}
	return ids
}

// withOrder sorts the case's elements and fills in Order and OrderFNV.
func (c pdqCase) withOrder() pdqCase {
	ids := c.sortedIDs()
	if c.N <= udOrderVerbatim {
		c.Order = ids
	}
	c.OrderFNV = udFNVInts(ids)
	return c
}

func pdqCases() []pdqCase {
	p := &udParams{state: 0x706471736f7274} // "pdqsort"
	sizes := []int{
		0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 20, 24, 31, 32, 33, 40, 48, 49, 50,
		51, 52, 63, 64, 65, 99, 100, 101, 127, 128, 129, 150, 200, 255, 256, 257, 400, 500, 511, 512,
		513, 1000, 1023, 1024, 1025, 2048, 4096,
	}
	// Sizes that also get McIlroy's antiquicksort adversary input.
	adversarySizes := []int{64, 129, 500, 1000, 2048}
	type shape struct {
		pattern        string
		modulus, swaps int
	}
	var res []pdqCase
	for _, n := range sizes {
		shapes := []shape{
			{"random", 2, 0}, {"random", 3, 0}, {"random", n/4 + 1, 0},
			{"asc", 1, 0}, {"desc", 1, 0}, {"equal", 1, 0}, {"saw", 7, 0}, {"organ", 4, 0},
			{"asc_swaps", 1, n/10 + 1}, {"desc_swaps", 1, 3},
		}
		if n <= 64 {
			shapes = append(shapes,
				shape{"random", 1, 0}, shape{"random", n + 1, 0}, shape{"random", 2, 0},
				shape{"asc", 3, 0}, shape{"desc", 3, 0}, shape{"saw", 2, 0}, shape{"organ", 1, 0},
				shape{"asc_swaps", 1, 1}, shape{"desc_swaps", 2, n/4 + 1},
			)
		}
		if slices.Contains(adversarySizes, n) {
			shapes = append(shapes, shape{"adversary", 0, 0})
		}
		for _, less := range []string{"len_desc", "x_asc_len_desc"} {
			for i, s := range shapes {
				c := pdqCase{
					Name:    fmt.Sprintf("n=%d/%s/m=%d/swaps=%d/%s/%d", n, s.pattern, s.modulus, s.swaps, less, i),
					N:       n,
					Pattern: s.pattern,
					Modulus: s.modulus,
					Values:  pdqAdversaryValues(s.pattern, n, less),
					Swaps:   s.swaps,
					Seed:    U64(p.next()),
					Less:    less,
				}
				res = append(res, c.withOrder())
			}
		}
	}
	// Seeded coin comparators over equal elements, two seeds per size and coin.
	for _, n := range []int{13, 49, 50, 64, 100, 257, 500, 1000, 4096} {
		for _, less := range []string{"coin_1_2", "coin_1_16", "coin_15_16"} {
			for i := 0; i < 2; i++ {
				c := pdqCase{
					Name:    fmt.Sprintf("n=%d/equal/m=1/swaps=0/%s/%d", n, less, i),
					N:       n,
					Pattern: "equal",
					Modulus: 1,
					Seed:    U64(p.next()),
					Less:    less,
				}
				res = append(res, c.withOrder())
			}
		}
	}
	return res
}
