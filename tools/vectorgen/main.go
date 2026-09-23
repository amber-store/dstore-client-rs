// Command vectorgen generates the golden test vectors of dstore-client-rs
// (tests/golden, conventions in VECTORS.md) by driving the Go implementation:
// github.com/amber-store/dstore v0.1.11 and the modules it pins, the normative
// reference for the Rust port. Usage:
//
//	go run . <out-dir> [family...]
//
// With no family named, every registered family is regenerated.
//
// Each family lives in its own file and registers itself from an init
// function, naming the paths under <out-dir> it owns (slash-separated files or
// directories):
//
//	func init() {
//		register("wire", []string{"wire/frames.json", "wire/frame_errors.json"}, genWire)
//	}
//
// A generator writes its files under out, the root of a fresh, empty
// directory laid out like tests/golden (for example out/wire/frames.json),
// and must not read <out-dir>. The command runs each selected family into a
// temporary directory and checks that the family produced at least one file
// under every path it owns and nothing outside them. Then it deletes exactly
// the paths the family owns from <out-dir>, so stale files go too, and copies
// the new files in. Nothing else in <out-dir> is touched. Two families never
// own overlapping paths, and no family owns cli/snapshots.json (written by
// cmd/clisnap) or .gitkeep. Every output is deterministic: running twice
// produces identical bytes.
package main

import (
	"fmt"
	"io"
	"io/fs"
	"os"
	"path"
	"path/filepath"
	"sort"
	"strings"
)

// family is one registered vector family.
type family struct {
	name string
	owns []string // clean slash paths relative to <out-dir>: files or directories
	gen  func(out string) error
}

// registry maps family names to families. It is only ever read in sorted name
// order.
var registry = map[string]family{}

// reserved lists paths under tests/golden that no family may own: the CLI
// snapshots come from cmd/clisnap, and .gitkeep keeps the directory in git.
var reserved = []string{"cli/snapshots.json", ".gitkeep"}

// register adds a family. Families call it from init functions.
func register(name string, owns []string, gen func(out string) error) {
	if err := checkFamily(name, owns, gen); err != nil {
		panic("vectorgen: " + err.Error())
	}
	registry[name] = family{name: name, owns: append([]string(nil), owns...), gen: gen}
}

// checkFamily validates a registration against the families registered so
// far.
func checkFamily(name string, owns []string, gen func(out string) error) error {
	if name == "" || strings.ContainsAny(name, " \t\n/") || gen == nil {
		return fmt.Errorf("bad family registration %q", name)
	}
	if _, dup := registry[name]; dup {
		return fmt.Errorf("family %q registered twice", name)
	}
	if len(owns) == 0 {
		return fmt.Errorf("family %q owns no paths", name)
	}
	for i, p := range owns {
		if p == "" || p == "." || path.IsAbs(p) || path.Clean(p) != p || p == ".." ||
			strings.HasPrefix(p, "../") || strings.Contains(p, `\`) {
			return fmt.Errorf("family %q: bad owned path %q (want a clean relative slash path)", name, p)
		}
		for _, r := range reserved {
			if within(r, p) {
				return fmt.Errorf("family %q: owned path %q covers reserved %s", name, p, r)
			}
		}
		for _, q := range owns[:i] {
			if within(p, q) || within(q, p) {
				return fmt.Errorf("family %q: owned paths %q and %q overlap", name, q, p)
			}
		}
		for _, other := range familyNames() {
			for _, q := range registry[other].owns {
				if within(p, q) || within(q, p) {
					return fmt.Errorf("family %q: owned path %q overlaps %q of family %q", name, p, q, other)
				}
			}
		}
	}
	return nil
}

// within reports whether rel is dir itself or lies below it.
func within(rel, dir string) bool {
	return rel == dir || strings.HasPrefix(rel, dir+"/")
}

// familyNames returns the registered family names, sorted.
func familyNames() []string {
	names := make([]string, 0, len(registry))
	for name := range registry {
		names = append(names, name)
	}
	sort.Strings(names)
	return names
}

func main() {
	if len(os.Args) < 2 {
		fmt.Fprintln(os.Stderr, "usage: vectorgen <out-dir> [family...]")
		fmt.Fprintf(os.Stderr, "families: %s\n", strings.Join(familyNames(), " "))
		os.Exit(2)
	}
	if err := run(os.Args[1], os.Args[2:]); err != nil {
		fmt.Fprintln(os.Stderr, "vectorgen:", err)
		os.Exit(1)
	}
}

// run regenerates the selected families (all when none are given) into outDir.
func run(outDir string, selected []string) error {
	names := selected
	if len(names) == 0 {
		names = familyNames()
	}
	seen := map[string]bool{}
	for _, name := range names {
		if _, ok := registry[name]; !ok {
			return fmt.Errorf("unknown family %q (registered: %s)", name, strings.Join(familyNames(), " "))
		}
		if seen[name] {
			return fmt.Errorf("family %q named twice", name)
		}
		seen[name] = true
	}
	if err := os.MkdirAll(outDir, 0o755); err != nil {
		return err
	}
	for _, name := range names {
		if err := regenerate(outDir, registry[name]); err != nil {
			return fmt.Errorf("family %s: %w", name, err)
		}
	}
	return nil
}

// regenerate runs one family into a temporary directory, checks the output
// against the paths the family owns, then replaces those paths in outDir.
// When the generator or the check fails, outDir is left untouched.
func regenerate(outDir string, f family) (err error) {
	stage, err := os.MkdirTemp("", "vectorgen-"+f.name+"-")
	if err != nil {
		return err
	}
	defer func() {
		if rmErr := os.RemoveAll(stage); err == nil {
			err = rmErr
		}
	}()
	if err := f.gen(stage); err != nil {
		return err
	}
	files, err := producedFiles(stage)
	if err != nil {
		return err
	}
	if err := checkProduced(f, files); err != nil {
		return err
	}
	for _, p := range f.owns {
		if err := os.RemoveAll(filepath.Join(outDir, filepath.FromSlash(p))); err != nil {
			return err
		}
	}
	for _, rel := range files {
		src := filepath.Join(stage, filepath.FromSlash(rel))
		if err := copyFile(src, filepath.Join(outDir, filepath.FromSlash(rel))); err != nil {
			return err
		}
	}
	return nil
}

// checkProduced requires at least one produced file under every owned path and
// no produced file outside them.
func checkProduced(f family, files []string) error {
	covered := make([]bool, len(f.owns))
	for _, rel := range files {
		owner := -1
		for i, p := range f.owns {
			if within(rel, p) {
				owner = i
				break
			}
		}
		if owner < 0 {
			return fmt.Errorf("produced %s, outside the paths it owns (%s)", rel, strings.Join(f.owns, " "))
		}
		covered[owner] = true
	}
	for i, p := range f.owns {
		if !covered[i] {
			return fmt.Errorf("owns %s but produced nothing there", p)
		}
	}
	return nil
}

// producedFiles lists the regular files under root as sorted slash paths.
func producedFiles(root string) ([]string, error) {
	var files []string
	err := filepath.WalkDir(root, func(p string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if d.IsDir() {
			return nil
		}
		rel, err := filepath.Rel(root, p)
		if err != nil {
			return err
		}
		if !d.Type().IsRegular() {
			return fmt.Errorf("%s is not a regular file", filepath.ToSlash(rel))
		}
		files = append(files, filepath.ToSlash(rel))
		return nil
	})
	sort.Strings(files)
	return files, err
}

// copyFile copies src to dst (0644), creating dst's parent directories.
func copyFile(src, dst string) (err error) {
	if err := os.MkdirAll(filepath.Dir(dst), 0o755); err != nil {
		return err
	}
	in, err := os.Open(src)
	if err != nil {
		return err
	}
	defer in.Close()
	out, err := os.OpenFile(dst, os.O_WRONLY|os.O_CREATE|os.O_TRUNC, 0o644)
	if err != nil {
		return err
	}
	defer func() {
		if cerr := out.Close(); err == nil {
			err = cerr
		}
	}()
	_, err = io.Copy(out, in)
	return err
}
