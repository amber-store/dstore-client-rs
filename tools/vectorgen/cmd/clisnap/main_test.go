package main

import (
	"encoding/hex"
	"os"
	"path/filepath"
	"reflect"
	"runtime"
	"strings"
	"testing"

	"github.com/amber-store/core/key"
	"golang.org/x/sys/unix"
)

func TestSmDataMatchesVectorsMd(t *testing.T) {
	// The splitmix64 stream of VECTORS.md, as dstore_testkit::golden's tests pin it.
	if got := hex.EncodeToString(smData(1, 20)); got != "c15c0289ec2d0a9167ec8e65a18debbe5e5532fb" {
		t.Errorf("data(1, 20) = %s", got)
	}
	if got := hex.EncodeToString(smData(7, 3)); got != "d70d32" {
		t.Errorf("data(7, 3) = %s", got)
	}
	if id := nodeID(1); len(id) != 64 || id != nodeID(1) || id == nodeID(2) {
		t.Errorf("nodeID(1) = %s", id)
	}
}

func TestNormalise(t *testing.T) {
	k, err := key.Parse(append([]byte{0x20, 0x01}, make([]byte, 30)...))
	if err != nil {
		t.Fatal(err)
	}
	vars := map[string]key.Key{"base": k}
	full := k.String()
	in := "cwd /r/sub root /r/x base " + full + " short " + full[:16]
	if got, want := normalise(in, "/r/sub", "/r", vars), "cwd {CWD} root {ROOT}/x base {key:base} short {key16:base}"; got != want {
		t.Errorf("normalise = %q, want %q", got, want)
	}
	if got, want := normalise("at /r/x", "/r", "/r", nil), "at {CWD}/x"; got != want {
		t.Errorf("normalise same root = %q, want %q", got, want)
	}
}

func TestNormaliseNodeSide(t *testing.T) {
	in := "time=2026-09-18T12:18:46.515Z level=INFO msg=\"adopted view\" node=23dc8f09\nnode id: " + strings.Repeat("ab", 32) + "\ncluster ticket: dstore1umafayhgwhxjez\ndstore: fine\n"
	got, used := normaliseNodeSide(in, nil)
	if want := "{SLOG}\nnode id: {HEX64}\ncluster ticket: {TICKET}\ndstore: fine\n"; got != want {
		t.Errorf("normaliseNodeSide = %q, want %q", got, want)
	}
	if want := []string{"{SLOG}", "{TICKET}", "{HEX64}"}; !reflect.DeepEqual(used, want) {
		t.Errorf("used = %q, want %q", used, want)
	}
	if _, used := normaliseNodeSide("dstore: plain\n", nil); used == nil || len(used) != 0 {
		t.Errorf("used for plain text = %#v, want empty and non-nil", used)
	}
}

func TestNodeSideTexts(t *testing.T) {
	ns, err := nodeSideOf(spec{nodeSideKind: "A", nodeSideCmd: "node join"})
	if err != nil {
		t.Fatal(err)
	}
	if want := "dstore: node join is a node-side command and dstore-client-rs does not implement the dstore node; use the Go dstore binary (github.com/amber-store/dstore v0.1.11)\n"; ns.Rust.Stderr != want || ns.Rust.Exit != 1 {
		t.Errorf("kind A: %+v", ns.Rust)
	}
	ns, err = nodeSideOf(spec{nodeSideKind: "DD-2", localDir: "P"})
	if err != nil {
		t.Fatal(err)
	}
	if want := "dstore: refstore: P/refs holds a Pebble database written by Go dstore v0.1.10 or earlier; dstore-client-rs cannot import it: open the --local directory once with Go dstore v0.1.11 or later, which does\n"; ns.Rust.Stderr != want || ns.Rust.Exit != 1 || ns.Kind != "DD-2" {
		t.Errorf("kind DD-2: %+v", ns)
	}
	for _, bad := range []spec{{nodeSideKind: "A"}, {nodeSideKind: "D"}, {nodeSideKind: "DD-2"}} {
		if _, err := nodeSideOf(bad); err == nil {
			t.Errorf("nodeSideOf(%+v) did not fail", bad)
		}
	}
}

func TestPebbleRefsStep(t *testing.T) {
	root := t.TempDir()
	if _, err := build(fixture{Name: "p", Steps: []step{{Op: "pebble_refs", Path: "P/refs", Names: pebbleRefsNames}}}, root, t.TempDir()); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(filepath.Join(root, "P", "refs", "LOCK")); err != nil {
		t.Errorf("no Pebble LOCK file: %v", err)
	}
	wrong := fixture{Name: "q", Steps: []step{{Op: "pebble_refs", Path: "Q/refs", Names: []string{"CURRENT"}}}}
	if _, err := build(wrong, root, t.TempDir()); err == nil || !strings.Contains(err.Error(), "the step names") {
		t.Errorf("mismatching names: build = %v", err)
	}
}

func TestFixtureStepsBuildTheTree(t *testing.T) {
	root := t.TempDir()
	fx := fixture{Name: "t", Steps: []step{
		{Op: "mkdir", Path: "d", Mode: mode(0o750)},
		{Op: "write", Path: "d/f", Text: str("hi\n"), Mode: mode(0o640)},
		{Op: "symlink", Path: "l", Target: "d/f"},
		{Op: "mtime", Path: "l", UnixNs: dec(t1)},
		{Op: "mtime", Path: "d/f", UnixNs: dec(t0)},
		{Op: "chmod", Path: "d/f", Mode: mode(0o600)},
		{Op: "ingest", Dir: "d", Into: "scratch", Exclude: []string{}, Var: "d"},
	}}
	vars, err := build(fx, root, t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	fi, err := os.Stat(filepath.Join(root, "d"))
	if err != nil || fi.Mode().Perm() != 0o750 {
		t.Errorf("d: %v %v", fi.Mode(), err)
	}
	fi, err = os.Stat(filepath.Join(root, "d", "f"))
	if err != nil || fi.Mode().Perm() != 0o600 || fi.ModTime().UnixNano() != t0 {
		t.Errorf("d/f: %v %v %v", fi.Mode(), fi.ModTime().UnixNano(), err)
	}
	var st unix.Stat_t
	if err := unix.Lstat(filepath.Join(root, "l"), &st); err != nil || st.Mtim.Nano() != t1 {
		t.Errorf("l mtime %d, want %d (%v)", st.Mtim.Nano(), t1, err)
	}
	if _, ok := vars["d"]; !ok {
		t.Errorf("ingest set no variable: %v", vars)
	}
	if _, err := build(fixture{Name: "bad", Steps: []step{{Op: "nosuch"}}}, root, t.TempDir()); err == nil {
		t.Error("an unknown op did not fail")
	}
}

func TestSymlinkStepMode(t *testing.T) {
	root := t.TempDir()
	lperm := func(name string) os.FileMode {
		t.Helper()
		fi, err := os.Lstat(filepath.Join(root, name))
		if err != nil {
			t.Fatal(err)
		}
		if fi.Mode()&os.ModeSymlink == 0 {
			t.Fatalf("%s is not a symlink: %v", name, fi.Mode())
		}
		return fi.Mode().Perm()
	}
	// wc1's link: 0777 on every platform, whatever the umask.
	fx := fixture{Name: "s", Steps: []step{{Op: "symlink", Path: "l", Target: "f", Mode: mode(0o777)}}}
	if _, err := build(fx, root, t.TempDir()); err != nil {
		t.Fatal(err)
	}
	if got := lperm("l"); got != 0o777 {
		t.Errorf("l: %v, want 0777", got)
	}
	// Bits a Linux link cannot have fail the step there; macOS applies them.
	fx = fixture{Name: "s2", Steps: []step{{Op: "symlink", Path: "m", Target: "f", Mode: mode(0o700)}}}
	_, err := build(fx, root, t.TempDir())
	switch runtime.GOOS {
	case "linux":
		if err == nil {
			t.Error("a 0700 symlink on Linux did not fail")
		}
	case "darwin":
		if err != nil {
			t.Fatal(err)
		}
		if got := lperm("m"); got != 0o700 {
			t.Errorf("m: %v, want 0700", got)
		}
	}
}

func TestCasesAreConsistent(t *testing.T) {
	specs, err := cases()
	if err != nil {
		t.Fatal(err)
	}
	names := map[string]bool{}
	fixtureNames := map[string]bool{}
	for _, fx := range fixtures() {
		fixtureNames[fx.Name] = true
	}
	groups := map[string]bool{"help": true, "unknown": true, "usage": true, "required": true, "validation": true, "node-side": true, "prompt": true, "wc": true}
	for _, sp := range specs {
		if names[sp.name] {
			t.Errorf("duplicate case %q", sp.name)
		}
		names[sp.name] = true
		if !fixtureNames[sp.fixture] {
			t.Errorf("case %q: no fixture %q", sp.name, sp.fixture)
		}
		if !groups[sp.group] || !strings.HasPrefix(sp.name, sp.group+"/") {
			t.Errorf("case %q: group %q", sp.name, sp.group)
		}
		if sp.env == nil || sp.args == nil {
			t.Errorf("case %q: nil env or args", sp.name)
		}
		if sp.nodeSideKind != "" {
			if _, err := nodeSideOf(sp); err != nil {
				t.Errorf("case %q: %v", sp.name, err)
			}
		}
	}
	// Every command and subcommand has its --help case.
	for _, top := range topCommands {
		if !names["help/"+top+" --help"] {
			t.Errorf("no help case for %s", top)
		}
	}
	for _, sc := range subcommands {
		for _, sub := range sc.subs {
			if !names["help/"+sc.parent+" "+sub+" --help"] {
				t.Errorf("no help case for %s %s", sc.parent, sub)
			}
		}
	}
}
