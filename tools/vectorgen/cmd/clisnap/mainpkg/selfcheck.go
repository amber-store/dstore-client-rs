package mainpkg

import (
	"bytes"
	_ "embed"
	"fmt"
	"go/ast"
	"go/parser"
	"go/printer"
	"go/token"
	"os"
	"os/exec"
	"path/filepath"
	"runtime/debug"
	"sort"
	"strings"
	"unicode"
)

// DstoreModule is the module whose cmd/dstore declarations copied.go holds,
// and DstoreVersion the version they are copied from.
const (
	DstoreModule  = "github.com/amber-store/dstore"
	DstoreVersion = "v0.1.11"
)

//go:embed copied.go
var copiedSource []byte

// SelfCheck compares every declaration of copied.go with the declaration of
// the same name in cmd/dstore of the dstore module the build selected, both
// printed with go/printer from ASTs parsed without comments. Any declaration
// that is missing from cmd/dstore or prints differently fails the check, as
// does a module version other than DstoreVersion.
func SelfCheck() error {
	dir, err := DstoreCmdDir()
	if err != nil {
		return err
	}
	return checkCopies(copiedSource, dir)
}

// DstoreCmdDir returns the directory of cmd/dstore in the dstore module the
// build selected: through `go list -m`, as verification.md §4.2 describes,
// falling back to the build information and $GOMODCACHE when the go command
// runs outside the module.
func DstoreCmdDir() (string, error) {
	out, listErr := exec.Command("go", "list", "-m", "-f", "{{.Dir}}\t{{.Version}}", DstoreModule).Output()
	if listErr == nil {
		dir, version, ok := strings.Cut(strings.TrimSpace(string(out)), "\t")
		if !ok || dir == "" {
			return "", fmt.Errorf("go list -m %s: unexpected output %q", DstoreModule, out)
		}
		if version != DstoreVersion {
			return "", fmt.Errorf("the build selects %s %s, the copies are of %s", DstoreModule, version, DstoreVersion)
		}
		return filepath.Join(dir, "cmd", "dstore"), nil
	}
	bi, ok := debug.ReadBuildInfo()
	if !ok {
		return "", fmt.Errorf("go list -m %s: %v; and no build information", DstoreModule, listErr)
	}
	for _, m := range bi.Deps {
		if m.Path != DstoreModule {
			continue
		}
		if m.Version != DstoreVersion {
			return "", fmt.Errorf("the build selects %s %s, the copies are of %s", DstoreModule, m.Version, DstoreVersion)
		}
		if m.Replace != nil {
			if m.Replace.Version == "" {
				return filepath.Join(m.Replace.Path, "cmd", "dstore"), nil
			}
			m = m.Replace
		}
		cache, err := exec.Command("go", "env", "GOMODCACHE").Output()
		if err != nil {
			return "", fmt.Errorf("go list -m %s: %v; go env GOMODCACHE: %v", DstoreModule, listErr, err)
		}
		return filepath.Join(strings.TrimSpace(string(cache)), escapeModulePath(m.Path)+"@"+m.Version, "cmd", "dstore"), nil
	}
	return "", fmt.Errorf("go list -m %s: %v; and the build information lacks the module", DstoreModule, listErr)
}

// escapeModulePath is the module cache's case encoding: each upper-case
// letter becomes '!' and its lower-case form.
func escapeModulePath(p string) string {
	var b strings.Builder
	for _, r := range p {
		if unicode.IsUpper(r) {
			b.WriteByte('!')
			r = unicode.ToLower(r)
		}
		b.WriteRune(r)
	}
	return b.String()
}

// checkCopies compares the declarations of the copy source with those of the
// non-test .go files of cmdDir.
func checkCopies(copied []byte, cmdDir string) error {
	originals, err := declsOfDir(cmdDir)
	if err != nil {
		return err
	}
	copies, err := declsOfSource("copied.go", copied)
	if err != nil {
		return err
	}
	if len(copies) == 0 {
		return fmt.Errorf("self-check: copied.go declares nothing")
	}
	var problems []string
	for _, name := range sortedNames(copies) {
		orig, ok := originals[name]
		switch {
		case !ok:
			problems = append(problems, fmt.Sprintf("%s is not declared in %s", name, cmdDir))
		case orig != copies[name]:
			problems = append(problems, fmt.Sprintf("%s differs from %s:\n--- cmd/dstore\n%s\n--- copied.go\n%s", name, cmdDir, orig, copies[name]))
		}
	}
	if len(problems) > 0 {
		return fmt.Errorf("self-check of the cmd/dstore copies failed:\n%s", strings.Join(problems, "\n"))
	}
	return nil
}

// declsOfDir collects the declarations of every non-test .go file of dir.
func declsOfDir(dir string) (map[string]string, error) {
	entries, err := os.ReadDir(dir)
	if err != nil {
		return nil, fmt.Errorf("self-check: %w", err)
	}
	all := map[string]string{}
	for _, e := range entries {
		name := e.Name()
		if e.IsDir() || !strings.HasSuffix(name, ".go") || strings.HasSuffix(name, "_test.go") {
			continue
		}
		src, err := os.ReadFile(filepath.Join(dir, name))
		if err != nil {
			return nil, fmt.Errorf("self-check: %w", err)
		}
		decls, err := declsOfSource(name, src)
		if err != nil {
			return nil, err
		}
		for k, v := range decls {
			if _, dup := all[k]; dup {
				return nil, fmt.Errorf("self-check: %s declared twice in %s", k, dir)
			}
			all[k] = v
		}
	}
	if len(all) == 0 {
		return nil, fmt.Errorf("self-check: no declarations in %s", dir)
	}
	return all, nil
}

// declsOfSource parses one file without comments and prints each top-level
// declaration: functions and methods whole, type, var and const specs one by
// one (so grouping does not matter). Names are "func NAME", "func (TYPE) NAME",
// "type NAME", "var NAME" and "const NAME".
func declsOfSource(filename string, src []byte) (map[string]string, error) {
	fset := token.NewFileSet()
	f, err := parser.ParseFile(fset, filename, src, 0)
	if err != nil {
		return nil, fmt.Errorf("self-check: %w", err)
	}
	decls := map[string]string{}
	add := func(name string, node any) error {
		var buf bytes.Buffer
		cfg := printer.Config{Mode: printer.UseSpaces | printer.TabIndent, Tabwidth: 8}
		if err := cfg.Fprint(&buf, fset, node); err != nil {
			return fmt.Errorf("self-check: printing %s of %s: %w", name, filename, err)
		}
		if _, dup := decls[name]; dup {
			return fmt.Errorf("self-check: %s declared twice in %s", name, filename)
		}
		decls[name] = buf.String()
		return nil
	}
	for _, d := range f.Decls {
		switch d := d.(type) {
		case *ast.FuncDecl:
			name := "func " + d.Name.Name
			if d.Recv != nil && len(d.Recv.List) == 1 {
				name = "func (" + receiverType(d.Recv.List[0].Type) + ") " + d.Name.Name
			}
			if err := add(name, d); err != nil {
				return nil, err
			}
		case *ast.GenDecl:
			if d.Tok == token.IMPORT {
				continue
			}
			for _, spec := range d.Specs {
				switch s := spec.(type) {
				case *ast.TypeSpec:
					if err := add("type "+s.Name.Name, s); err != nil {
						return nil, err
					}
				case *ast.ValueSpec:
					if len(s.Values) == 0 {
						return nil, fmt.Errorf("self-check: %s: a %s spec without values (implicit repetition) cannot be compared", filename, d.Tok)
					}
					names := make([]string, len(s.Names))
					for i, n := range s.Names {
						names[i] = n.Name
					}
					if err := add(d.Tok.String()+" "+strings.Join(names, ", "), s); err != nil {
						return nil, err
					}
				}
			}
		}
	}
	return decls, nil
}

// receiverType names a method receiver's type, without the pointer.
func receiverType(e ast.Expr) string {
	switch t := e.(type) {
	case *ast.StarExpr:
		return receiverType(t.X)
	case *ast.Ident:
		return t.Name
	case *ast.IndexExpr:
		return receiverType(t.X)
	}
	return fmt.Sprintf("%T", e)
}

func sortedNames(m map[string]string) []string {
	names := make([]string, 0, len(m))
	for k := range m {
		names = append(names, k)
	}
	sort.Strings(names)
	return names
}
