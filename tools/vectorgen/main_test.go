package main

import (
	"errors"
	"io/fs"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
)

// withRegistry runs f with an empty registry, then restores the real one.
func withRegistry(t *testing.T, f func()) {
	t.Helper()
	saved := registry
	registry = map[string]family{}
	defer func() { registry = saved }()
	f()
}

// put writes content to root/rel.
func put(t *testing.T, root, rel, content string) {
	t.Helper()
	if err := writeFile(filepath.Join(root, filepath.FromSlash(rel)), []byte(content)); err != nil {
		t.Fatal(err)
	}
}

// tree returns every regular file under root as slash path → content.
func tree(t *testing.T, root string) map[string]string {
	t.Helper()
	got := map[string]string{}
	err := filepath.WalkDir(root, func(p string, d fs.DirEntry, err error) error {
		if err != nil || d.IsDir() {
			return err
		}
		b, err := os.ReadFile(p)
		if err != nil {
			return err
		}
		rel, err := filepath.Rel(root, p)
		if err != nil {
			return err
		}
		got[filepath.ToSlash(rel)] = string(b)
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}
	return got
}

func TestRegenerateReplacesExactlyTheOwnedPaths(t *testing.T) {
	withRegistry(t, func() {
		register("fam", []string{"fam/a.json", "fam/trees"}, func(out string) error {
			if err := writeFile(filepath.Join(out, "fam", "a.json"), []byte("new a\n")); err != nil {
				return err
			}
			return writeFile(filepath.Join(out, "fam", "trees", "t1.bin"), []byte("t1\n"))
		})
		dir := t.TempDir()
		put(t, dir, "fam/a.json", "old a\n")
		put(t, dir, "fam/trees/stale.bin", "stale\n")
		put(t, dir, "fam/other.json", "not owned\n")
		put(t, dir, "cli/snapshots.json", "clisnap\n")
		put(t, dir, ".gitkeep", "")
		if err := run(dir, nil); err != nil {
			t.Fatal(err)
		}
		want := map[string]string{
			"fam/a.json":         "new a\n",
			"fam/trees/t1.bin":   "t1\n",
			"fam/other.json":     "not owned\n",
			"cli/snapshots.json": "clisnap\n",
			".gitkeep":           "",
		}
		if got := tree(t, dir); !reflect.DeepEqual(got, want) {
			t.Fatalf("out dir after run:\n got %v\nwant %v", got, want)
		}
	})
}

func TestRegenerateChecksOutputBeforeTouchingOutDir(t *testing.T) {
	cases := []struct {
		name    string
		owns    []string
		gen     func(out string) error
		wantErr string
	}{
		{
			name: "outside",
			owns: []string{"fam/a.json"},
			gen: func(out string) error {
				if err := writeFile(filepath.Join(out, "fam", "a.json"), []byte("a")); err != nil {
					return err
				}
				return writeFile(filepath.Join(out, "fam", "b.json"), []byte("b"))
			},
			wantErr: "produced fam/b.json, outside the paths it owns",
		},
		{
			name:    "missing",
			owns:    []string{"fam/a.json"},
			gen:     func(out string) error { return nil },
			wantErr: "owns fam/a.json but produced nothing there",
		},
		{
			name:    "generator error",
			owns:    []string{"fam/a.json"},
			gen:     func(out string) error { return errors.New("boom") },
			wantErr: "boom",
		},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			withRegistry(t, func() {
				register("fam", tc.owns, tc.gen)
				dir := t.TempDir()
				put(t, dir, "fam/a.json", "committed\n")
				err := run(dir, []string{"fam"})
				if err == nil || !strings.Contains(err.Error(), tc.wantErr) {
					t.Fatalf("run error = %v, want it to contain %q", err, tc.wantErr)
				}
				want := map[string]string{"fam/a.json": "committed\n"}
				if got := tree(t, dir); !reflect.DeepEqual(got, want) {
					t.Fatalf("out dir changed on error:\n got %v\nwant %v", got, want)
				}
			})
		})
	}
}

func TestCheckFamilyRefusesBadRegistrations(t *testing.T) {
	gen := func(out string) error { return nil }
	cases := []struct {
		name    string
		owns    []string
		wantErr string
	}{
		{"bad name", []string{"x.json"}, "bad family registration"},
		{"base", []string{"x.json"}, "registered twice"},
		{"none", nil, "owns no paths"},
		{"empty", []string{""}, "bad owned path"},
		{"dot", []string{"."}, "bad owned path"},
		{"abs", []string{"/abs"}, "bad owned path"},
		{"trailing", []string{"a/"}, "bad owned path"},
		{"dotslash", []string{"./a"}, "bad owned path"},
		{"parent", []string{"../a"}, "bad owned path"},
		{"unclean", []string{"a/../b"}, "bad owned path"},
		{"cli dir", []string{"cli"}, "covers reserved cli/snapshots.json"},
		{"snapshots", []string{"cli/snapshots.json"}, "covers reserved"},
		{"gitkeep", []string{".gitkeep"}, "covers reserved .gitkeep"},
		{"self overlap", []string{"a", "a/b.json"}, "overlap"},
		{"other overlap", []string{"base/dir/x.json"}, `overlaps "base/dir" of family "base"`},
		{"other parent", []string{"base"}, `overlaps "base/dir" of family "base"`},
	}
	withRegistry(t, func() {
		register("base", []string{"base/dir"}, gen)
		for _, tc := range cases {
			name := "fam"
			switch tc.name {
			case "bad name":
				name = "a b"
			case "base":
				name = "base"
			}
			err := checkFamily(name, tc.owns, gen)
			if err == nil || !strings.Contains(err.Error(), tc.wantErr) {
				t.Errorf("%s: checkFamily error = %v, want it to contain %q", tc.name, err, tc.wantErr)
			}
		}
		if err := checkFamily("sibling", []string{"base/dir2", "cli/size.json"}, gen); err != nil {
			t.Errorf("sibling paths refused: %v", err)
		}
	})
}
