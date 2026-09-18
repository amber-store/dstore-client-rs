// Command gotables prints crates/gocompat/src/tables.rs of dstore-client-rs: the Unicode and strconv
// tables of go1.26.5, the toolchain of dstore v0.1.9 (PORTING.md §4.1 and DD-14):
//
//   - strconv's isPrint16, isNotPrint16, isPrint32, isNotPrint32 and isGraphic, computed from
//     strconv.IsPrint and strconv.IsGraphic with the algorithm of strconv/makeisprint.go;
//   - unicode.White_Space, as the list of its runes;
//   - every rune whose unicode.ToLower or unicode.ToUpper differs from itself.
//
// Before anything is printed, the tables are checked over every rune against strconv.IsPrint,
// unicode.IsPrint, strconv.IsGraphic, unicode.IsGraphic, unicode.IsSpace, unicode.ToLower and
// unicode.ToUpper, through the lookups that crates/gocompat/src/quote.rs and strings.rs perform.
//
// Usage, from tools/vectorgen:
//
//	go run ./cmd/gotables > ../../crates/gocompat/src/tables.rs
package main

import (
	"bufio"
	"cmp"
	"errors"
	"fmt"
	"io"
	"log"
	"os"
	"runtime"
	"slices"
	"strconv"
	"unicode"
	"unicode/utf8"
)

// goVersion is the toolchain the committed tables come from. Change it only together with the go line
// of dstore's go.mod, then regenerate tables.rs and the text vectors.
const goVersion = "go1.26.5"

func main() {
	log.SetFlags(0)
	log.SetPrefix("gotables: ")
	if v := runtime.Version(); v != goVersion {
		log.Fatalf("running on %s, want %s: the tables must come from dstore's toolchain (PORTING.md DD-14)", v, goVersion)
	}
	t, err := compute()
	if err != nil {
		log.Fatal(err)
	}
	if err := t.check(); err != nil {
		log.Fatal(err)
	}
	w := bufio.NewWriter(os.Stdout)
	t.write(w)
	if err := w.Flush(); err != nil {
		log.Fatal(err)
	}
}

// tables holds everything tables.rs declares.
type tables struct {
	print16    []uint16  // pairs lo, hi
	notPrint16 []uint16  // exceptions inside print16 ranges
	print32    []uint32  // pairs lo, hi
	notPrint32 []uint16  // exceptions inside print32 ranges, minus 0x10000
	graphic    []uint16  // graphic runes that are not printable
	whiteSpace []rune    // unicode.White_Space
	toLower    [][2]rune // {r, unicode.ToLower(r)} where they differ
	toUpper    [][2]rune // {r, unicode.ToUpper(r)} where they differ
}

// compute builds the tables from the functions of the running toolchain.
func compute() (*tables, error) {
	t := &tables{}
	var err error
	rang, except := scan(0, 0xFFFF)
	if t.print16, err = to16(rang, 0); err != nil {
		return nil, err
	}
	if t.notPrint16, err = to16(except, 0); err != nil {
		return nil, err
	}
	t.print32, except = scan(0x10000, unicode.MaxRune)
	if t.notPrint32, err = to16(except, 0x10000); err != nil {
		return nil, err
	}
	for r := rune(0); r <= unicode.MaxRune; r++ {
		if strconv.IsPrint(r) != strconv.IsGraphic(r) {
			if !strconv.IsGraphic(r) {
				return nil, fmt.Errorf("%U is printable but not graphic", r)
			}
			if r > 0xFFFF {
				return nil, fmt.Errorf("%U is graphic and not printable but does not fit in 16 bits", r)
			}
			t.graphic = append(t.graphic, uint16(r))
		}
		if unicode.Is(unicode.White_Space, r) {
			t.whiteSpace = append(t.whiteSpace, r)
		}
		if l := unicode.ToLower(r); l != r {
			if !utf8.ValidRune(r) || !utf8.ValidRune(l) {
				return nil, fmt.Errorf("unicode.ToLower(%U) = %U: not a pair of valid runes", r, l)
			}
			t.toLower = append(t.toLower, [2]rune{r, l})
		}
		if u := unicode.ToUpper(r); u != r {
			if !utf8.ValidRune(r) || !utf8.ValidRune(u) {
				return nil, fmt.Errorf("unicode.ToUpper(%U) = %U: not a pair of valid runes", r, u)
			}
			t.toUpper = append(t.toUpper, [2]rune{r, u})
		}
	}
	return t, nil
}

// scan is scan of strconv/makeisprint.go over strconv.IsPrint: the inclusive ranges of printable runes
// in [lo, hi] as pairs, and the non-printable runes kept inside a range as exceptions because the runes
// on both sides of them are printable.
func scan(lo, hi rune) (rang, except []uint32) {
	start := rune(-1)
	for i := lo; ; i++ {
		if (i > hi || !strconv.IsPrint(i)) && start >= 0 {
			// End range, but avoid flip flop.
			if i+1 <= hi && strconv.IsPrint(i+1) {
				except = append(except, uint32(i))
				continue
			}
			rang = append(rang, uint32(start), uint32(i-1))
			start = -1
		}
		if i > hi {
			break
		}
		if start < 0 && strconv.IsPrint(i) {
			start = i
		}
	}
	return rang, except
}

// to16 converts table values minus base to uint16.
func to16(x []uint32, base uint32) ([]uint16, error) {
	y := make([]uint16, 0, len(x))
	for _, v := range x {
		if v < base || v-base > 0xFFFF {
			return nil, fmt.Errorf("table value %#x does not fit in 16 bits above %#x", v, base)
		}
		y = append(y, uint16(v-base))
	}
	return y, nil
}

// check compares the tables with the functions they stand for, over every rune.
func (t *tables) check() error {
	var errs []error
	for r := rune(0); r <= unicode.MaxRune && len(errs) < 10; r++ {
		p := t.isPrint(r)
		if p != strconv.IsPrint(r) || p != unicode.IsPrint(r) {
			errs = append(errs, fmt.Errorf("%U: tables give IsPrint %t, strconv.IsPrint %t, unicode.IsPrint %t",
				r, p, strconv.IsPrint(r), unicode.IsPrint(r)))
		}
		g := p || t.inGraphicList(r)
		if g != strconv.IsGraphic(r) || g != unicode.IsGraphic(r) {
			errs = append(errs, fmt.Errorf("%U: tables give IsGraphic %t, strconv.IsGraphic %t, unicode.IsGraphic %t",
				r, g, strconv.IsGraphic(r), unicode.IsGraphic(r)))
		}
		if s := t.isSpace(r); s != unicode.IsSpace(r) {
			errs = append(errs, fmt.Errorf("%U: tables give IsSpace %t, unicode.IsSpace %t", r, s, unicode.IsSpace(r)))
		}
		if l := lookup(t.toLower, r); l != unicode.ToLower(r) {
			errs = append(errs, fmt.Errorf("%U: tables give ToLower %U, unicode.ToLower %U", r, l, unicode.ToLower(r)))
		}
		if u := lookup(t.toUpper, r); u != unicode.ToUpper(r) {
			errs = append(errs, fmt.Errorf("%U: tables give ToUpper %U, unicode.ToUpper %U", r, u, unicode.ToUpper(r)))
		}
	}
	return errors.Join(errs...)
}

// isPrint is strconv.IsPrint over the tables (the lookup of quote.rs is_print).
func (t *tables) isPrint(r rune) bool {
	if r <= 0xFF {
		if 0x20 <= r && r <= 0x7E {
			return true
		}
		if 0xA1 <= r && r <= 0xFF {
			return r != 0xAD
		}
		return false
	}
	if r < 1<<16 {
		rr := uint16(r)
		i, _ := slices.BinarySearch(t.print16, rr)
		if i >= len(t.print16) || rr < t.print16[i&^1] || t.print16[i|1] < rr {
			return false
		}
		_, found := slices.BinarySearch(t.notPrint16, rr)
		return !found
	}
	rr := uint32(r)
	i, _ := slices.BinarySearch(t.print32, rr)
	if i >= len(t.print32) || rr < t.print32[i&^1] || t.print32[i|1] < rr {
		return false
	}
	if r >= 0x20000 {
		return true
	}
	_, found := slices.BinarySearch(t.notPrint32, uint16(r-0x10000))
	return !found
}

// inGraphicList is strconv.isInGraphicList over the tables.
func (t *tables) inGraphicList(r rune) bool {
	if r > 0xFFFF {
		return false
	}
	_, found := slices.BinarySearch(t.graphic, uint16(r))
	return found
}

// isSpace is unicode.IsSpace over the tables (the lookup of quote.rs is_space).
func (t *tables) isSpace(r rune) bool {
	if r <= 0xFF {
		switch r {
		case '\t', '\n', '\v', '\f', '\r', ' ', 0x85, 0xA0:
			return true
		}
		return false
	}
	_, found := slices.BinarySearch(t.whiteSpace, r)
	return found
}

// lookup maps r through a table of {r, mapped} pairs (the lookup of strings.rs).
func lookup(table [][2]rune, r rune) rune {
	i, found := slices.BinarySearchFunc(table, r, func(e [2]rune, r rune) int { return cmp.Compare(e[0], r) })
	if found {
		return table[i][1]
	}
	return r
}

// write prints tables.rs.
func (t *tables) write(w io.Writer) {
	p := func(format string, a ...any) {
		fmt.Fprintf(w, format, a...)
	}
	p("//! Unicode and `strconv` tables of %s (Unicode %s), for `quote` and `strings`:\n", goVersion, unicode.Version)
	p("//!\n")
	p("//! - `strconv`'s `isPrint16`, `isNotPrint16`, `isPrint32`, `isNotPrint32` and `isGraphic`;\n")
	p("//! - `unicode.White_Space`;\n")
	p("//! - every rune whose `unicode.ToLower` or `unicode.ToUpper` differs from itself.\n")
	p("//!\n")
	p("//! GENERATED by `tools/vectorgen/cmd/gotables`; do not edit. CI regenerates this file and diffs it.\n")

	p("\n/// `strconv` `isPrint16`: pairs `lo, hi` of inclusive ranges of printable runes below U+10000. A\n")
	p("/// rune inside a range is printable unless `IS_NOT_PRINT16` lists it.\n")
	p("#[rustfmt::skip]\n")
	p("pub(crate) static IS_PRINT16: &[u16] = &[\n")
	for i := 0; i+1 < len(t.print16); i += 2 {
		p("    0x%04x, 0x%04x,\n", t.print16[i], t.print16[i+1])
	}
	p("];\n")

	p("\n/// `strconv` `isNotPrint16`: the non-printable runes inside `IS_PRINT16` ranges.\n")
	p("#[rustfmt::skip]\n")
	p("pub(crate) static IS_NOT_PRINT16: &[u16] = &[\n")
	for _, v := range t.notPrint16 {
		p("    0x%04x,\n", v)
	}
	p("];\n")

	p("\n/// `strconv` `isPrint32`: pairs `lo, hi` of inclusive ranges of printable runes from U+10000. A\n")
	p("/// rune below U+20000 inside a range is printable unless `IS_NOT_PRINT32` lists it.\n")
	p("#[rustfmt::skip]\n")
	p("pub(crate) static IS_PRINT32: &[u32] = &[\n")
	for i := 0; i+1 < len(t.print32); i += 2 {
		p("    0x%06x, 0x%06x,\n", t.print32[i], t.print32[i+1])
	}
	p("];\n")

	p("\n/// `strconv` `isNotPrint32`: the non-printable runes inside `IS_PRINT32` ranges, minus 0x10000.\n")
	p("#[rustfmt::skip]\n")
	p("pub(crate) static IS_NOT_PRINT32: &[u16] = &[\n")
	for _, v := range t.notPrint32 {
		p("    0x%04x,\n", v)
	}
	p("];\n")

	p("\n/// `strconv` `isGraphic`: the graphic runes that are not printable (`strconv.IsGraphic`). `Quote`\n")
	p("/// does not use it.\n")
	p("#[rustfmt::skip]\n")
	p("#[allow(dead_code)]\n")
	p("pub(crate) static IS_GRAPHIC: &[u16] = &[\n")
	for _, v := range t.graphic {
		p("    0x%04x,\n", v)
	}
	p("];\n")

	p("\n/// `unicode.White_Space`: every rune with the White_Space property, ascending.\n")
	p("#[rustfmt::skip]\n")
	p("pub(crate) static WHITE_SPACE: &[char] = &[\n")
	for _, r := range t.whiteSpace {
		p("    '\\u{%04x}',\n", r)
	}
	p("];\n")

	p("\n/// `(r, unicode.ToLower(r))` for every rune `r` whose simple lower-case mapping differs from `r`,\n")
	p("/// ascending by `r`.\n")
	p("#[rustfmt::skip]\n")
	p("pub(crate) static TO_LOWER: &[(char, char)] = &[\n")
	for _, e := range t.toLower {
		p("    ('\\u{%04x}', '\\u{%04x}'),\n", e[0], e[1])
	}
	p("];\n")

	p("\n/// `(r, unicode.ToUpper(r))` for every rune `r` whose simple upper-case mapping differs from `r`,\n")
	p("/// ascending by `r`.\n")
	p("#[rustfmt::skip]\n")
	p("pub(crate) static TO_UPPER: &[(char, char)] = &[\n")
	for _, e := range t.toUpper {
		p("    ('\\u{%04x}', '\\u{%04x}'),\n", e[0], e[1])
	}
	p("];\n")
}
