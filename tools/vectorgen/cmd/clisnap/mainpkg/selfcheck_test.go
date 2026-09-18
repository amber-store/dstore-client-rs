package mainpkg

import (
	"bytes"
	"strings"
	"testing"
)

func TestSelfCheck(t *testing.T) {
	if err := SelfCheck(); err != nil {
		t.Fatal(err)
	}
}

func TestSelfCheckDetectsDrift(t *testing.T) {
	dir, err := DstoreCmdDir()
	if err != nil {
		t.Fatal(err)
	}
	mutated := bytes.Replace(copiedSource, []byte(`"%d/%d objects"`), []byte(`"%d/%d objectz"`), 1)
	if bytes.Equal(mutated, copiedSource) {
		t.Fatal("the mutation did not apply")
	}
	if err := checkCopies(mutated, dir); err == nil || !strings.Contains(err.Error(), "func statusLine differs") {
		t.Errorf("changed statusLine: checkCopies = %v", err)
	}
	extra := append(append([]byte{}, copiedSource...), []byte("\nfunc notInDstore() {}\n")...)
	if err := checkCopies(extra, dir); err == nil || !strings.Contains(err.Error(), "func notInDstore is not declared") {
		t.Errorf("invented function: checkCopies = %v", err)
	}
	// Comments and blank lines do not take part.
	commented := bytes.Replace(copiedSource, []byte("func parseSize("), []byte("// a note\n\nfunc parseSize("), 1)
	if err := checkCopies(commented, dir); err != nil {
		t.Errorf("added comment: checkCopies = %v", err)
	}
}

func TestDeclNames(t *testing.T) {
	decls, err := declsOfSource("x.go", []byte("package x\ntype (\n\ta int\n\tb struct{}\n)\nconst c, d = 1, 2\nvar e = 3\nfunc f() {}\nfunc (g *a) h() {}\nfunc (a) i() {}\n"))
	if err != nil {
		t.Fatal(err)
	}
	want := []string{"const c, d", "func (a) h", "func (a) i", "func f", "type a", "type b", "var e"}
	if got := sortedNames(decls); strings.Join(got, "|") != strings.Join(want, "|") {
		t.Errorf("names %q, want %q", got, want)
	}
}
