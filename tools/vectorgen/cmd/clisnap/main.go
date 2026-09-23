// Command clisnap captures tests/golden/cli/snapshots.json: the stdout, stderr
// and exit status of the Go dstore v0.1.10 CLI for every case of
// port-notes/cli.md §5.2-§5.3 and verification.md §4.3 item 24 and §5 that
// runs without a cluster (help, usage errors, unknown commands and flags,
// required flags, --version, argument validation before dialing, node-side
// validation up to the store step, prompts, offline working-copy commands).
// Where dstore-client-rs substitutes the node-side messages of PORTING.md
// §2.2, the case records Go's behaviour and names the Rust expectation.
//
// Usage, from tools/vectorgen:
//
//	go run ./cmd/clisnap -o OUTDIR
//
// It first runs the AST self-check of the cmd/dstore copies (mainpkg), then
// builds github.com/amber-store/dstore/cmd/dstore with the vectorgen module's
// build list (identical to dstore v0.1.10's go.mod) into a temporary directory,
// runs every case there with a clean environment (PATH, a temporary HOME,
// TZ=UTC) in a freshly built fixture directory, and deletes the binary and
// every fixture afterwards. Schema: docs/vectorgen-cli.md.
package main

import (
	"bytes"
	"context"
	"crypto/ed25519"
	"debug/buildinfo"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"sort"
	"strconv"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/amber-store/core/ingest"
	"github.com/amber-store/core/key"
	"github.com/amber-store/core/packstore"
	"github.com/amber-store/core/refstore"
	"github.com/amber-store/dstore-client-rs/tools/vectorgen/cmd/clisnap/mainpkg"
	"github.com/amber-store/dstore/worktree"
	"golang.org/x/sys/unix"
)

func main() {
	out := flag.String("o", "", "directory to write snapshots.json into")
	verbose := flag.Bool("v", false, "print each case as it runs")
	flag.Parse()
	if *out == "" || flag.NArg() != 0 {
		fmt.Fprintln(os.Stderr, "usage: clisnap -o OUTDIR [-v]")
		os.Exit(2)
	}
	if err := run(*out, *verbose); err != nil {
		fmt.Fprintln(os.Stderr, "clisnap:", err)
		os.Exit(1)
	}
}

// ---- snapshots.json schema ----

type snapFile struct {
	Generator   generator   `json:"generator"`
	Environment environment `json:"environment"`
	Fixtures    []fixture   `json:"fixtures"`
	Cases       []snapCase  `json:"cases"`
}

type generator struct {
	Program string       `json:"program"`
	Go      string       `json:"go"`
	Module  modVersion   `json:"module"`
	Deps    []modVersion `json:"deps"`
}

type modVersion struct {
	Path    string `json:"path"`
	Version string `json:"version"`
}

type environment struct {
	Inherited []string `json:"inherited"`
	Set       []envVar `json:"set"`
}

type envVar struct {
	Name  string `json:"name"`
	Value string `json:"value"`
}

type fixture struct {
	Name  string `json:"name"`
	Steps []step `json:"steps"`
}

// step is one fixture operation; see docs/vectorgen-cli.md for each op.
type step struct {
	Op               string           `json:"op"`
	Path             string           `json:"path,omitempty"`
	Mode             *uint32          `json:"mode,omitempty"`
	Text             *string          `json:"text,omitempty"`
	Target           string           `json:"target,omitempty"`
	UnixNs           *decimal         `json:"unix_ns,omitempty"`
	Config           *worktree.Config `json:"config,omitempty"`
	Dir              string           `json:"dir,omitempty"`
	Into             string           `json:"into,omitempty"`
	Exclude          []string         `json:"exclude,omitempty"`
	Var              string           `json:"var,omitempty"`
	BaseVar          *string          `json:"base_var,omitempty"`
	RemoteVar        *string          `json:"remote_var,omitempty"`
	RemoteVersionHex *string          `json:"remote_version_hex,omitempty"`
	SyncedAtUnixNs   *decimal         `json:"synced_at_unix_ns,omitempty"`
	Names            []string         `json:"names,omitempty"`
}

type snapCase struct {
	Name     string    `json:"name"`
	Group    string    `json:"group"`
	Args     []string  `json:"args"`
	Env      []envVar  `json:"env"`
	Fixture  string    `json:"fixture"`
	Subdir   string    `json:"subdir"`
	Stdin    string    `json:"stdin"`
	Exit     int       `json:"exit"`
	Stdout   string    `json:"stdout"`
	Stderr   string    `json:"stderr"`
	NodeSide *nodeSide `json:"node_side"`
}

// nodeSide marks a case where dstore-client-rs replaces Go's behaviour with a
// designed substitute: the node-side texts of PORTING.md §2.2 (kinds A, B and
// C) or the Pebble refs refusal of §2.3 (kind DD-2). Exit, Stdout and Stderr
// are Go's (with the placeholders of GoNormalized); Rust is what the Rust CLI
// prints.
type nodeSide struct {
	Kind         string   `json:"kind"`
	GoNormalized []string `json:"go_normalized"`
	Rust         output   `json:"rust"`
}

type output struct {
	Exit   int    `json:"exit"`
	Stdout string `json:"stdout"`
	Stderr string `json:"stderr"`
}

// decimal marshals an int64 as a decimal string (VECTORS.md conventions).
type decimal int64

func (d decimal) MarshalJSON() ([]byte, error) {
	return json.Marshal(strconv.FormatInt(int64(d), 10))
}

func dec(n int64) *decimal { d := decimal(n); return &d }

func str(s string) *string { return &s }

func mode(m uint32) *uint32 { return &m }

// ---- constants shared with the Rust harness ----

const (
	program = "tools/vectorgen/cmd/clisnap"

	placeholderCWD  = "{CWD}"
	placeholderROOT = "{ROOT}"

	// Fixture times: T0 for the synced state, T1 for later edits.
	t0      = int64(1600000000000000000)
	t1      = int64(1700000000000000000)
	syncedT = t0 + 3600*int64(time.Second)
)

// PORTING.md §2.2 texts of dstore-client-rs.
const (
	storeTicketUnsupported    = "deriving a ticket from --store needs the node's Pebble meta store, which dstore-client-rs does not implement; pass --ticket or $DSTORE_TICKET, or use the Go dstore binary"
	catalogRestoreUnsupported = "catalog restore writes through the node's paxos acceptor, which dstore-client-rs does not implement; use the Go dstore binary"
)

func nodeSideCommandText(cmd string) string {
	return cmd + " is a node-side command and dstore-client-rs does not implement the dstore node; use the Go dstore binary (github.com/amber-store/dstore v0.1.10)"
}

// pebbleRefsText is the PORTING.md §2.3 refusal for a --local directory whose
// refs/ holds a Pebble database; local is the --local value as given.
func pebbleRefsText(local string) string {
	return "refstore: " + local + "/refs holds a Pebble database written by Go dstore; dstore-client-rs keeps local references in redb and cannot open it (use another --local directory)"
}

// pebbleRefsNames are the entries of a fresh core v0.0.9 refstore (Pebble)
// directory after Open and Close. The pebble_refs step checks them, so a
// Pebble upgrade that changes them fails the run.
var pebbleRefsNames = []string{"000002.log", "LOCK", "MANIFEST-000001", "OPTIONS-000003", "marker.format-version.000001.013", "marker.manifest.000001.MANIFEST-000001"}

// smData is data(seed, n) of VECTORS.md: splitmix64 outputs as 8
// little-endian bytes, truncated to n.
func smData(seed uint64, n int) []byte {
	out := make([]byte, 0, n+8)
	state := seed
	for len(out) < n {
		state += 0x9E3779B97F4A7C15
		z := state
		z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9
		z = (z ^ (z >> 27)) * 0x94D049BB133111EB
		out = binary.LittleEndian.AppendUint64(out, z^(z>>31))
	}
	return out[:n]
}

// nodeID is the hex ed25519 public key of NewKeyFromSeed(data(seed, 32)).
func nodeID(seed uint64) string {
	pub, ok := ed25519.NewKeyFromSeed(smData(seed, 32)).Public().(ed25519.PublicKey)
	if !ok {
		panic("ed25519: public key type")
	}
	return hex.EncodeToString(pub)
}

// ---- fixtures ----

func fixtures() []fixture {
	write := func(path, text string, m uint32) step {
		return step{Op: "write", Path: path, Text: str(text), Mode: mode(m)}
	}
	mkdir := func(path string, m uint32) step { return step{Op: "mkdir", Path: path, Mode: mode(m)} }
	mtime := func(path string, ns int64) step { return step{Op: "mtime", Path: path, UnixNs: dec(ns)} }
	wcCreate := func(name string) step {
		return step{Op: "wc_create", Config: &worktree.Config{Ticket: "bogus", Name: name}}
	}
	empty := ""
	return []fixture{
		{Name: "empty", Steps: []step{}},
		{Name: "files", Steps: []step{
			write("backup.bin", "junk", 0o644),
			mkdir("src", 0o755),
			write("src/hello.txt", "hello\n", 0o644),
			mtime("src/hello.txt", t0),
			mtime("src", t0),
			{Op: "ingest", Dir: "src", Into: "scratch", Exclude: []string{}, Var: "src"},
			mkdir("node", 0o755),
			write("node/identity", hex.EncodeToString(smData(3, 32))+"\n", 0o600),
			mkdir("dirident", 0o755),
			mkdir("dirident/identity", 0o755),
		}},
		{Name: "wc-no-config", Steps: []step{mkdir(".dstore", 0o755)}},
		{Name: "wc-bad-config", Steps: []step{mkdir(".dstore", 0o755), write(".dstore/config", "{not json\n", 0o644)}},
		{Name: "wc-no-state", Steps: []step{wcCreate("trees/demo")}},
		{Name: "wc-bad-state-key", Steps: []step{
			wcCreate("trees/demo"),
			write(".dstore/state", "{\n  \"base\": \""+strings.Repeat("z", 64)+"\",\n  \"remote\": \"\",\n  \"remote_version\": \"\",\n  \"synced_at\": \"2020-09-13T13:26:40Z\"\n}\n", 0o644),
		}},
		{Name: "wc-no-remote", Steps: []step{
			wcCreate("trees/demo"),
			{Op: "wc_state", BaseVar: &empty, SyncedAtUnixNs: dec(syncedT)},
		}},
		// port-notes/cli.md §3.5, first fixture.
		{Name: "wc1", Steps: []step{
			write("a.txt", "one\ntwo\nthree\n", 0o644),
			mkdir("sub", 0o755),
			write("sub/b.txt", "x\ny\n", 0o644),
			write("run.sh", "#!/bin/sh\necho run\n", 0o644),
			// The link's bits are part of its ingested entry, and the
			// type change to a 0755 directory below prints old mode /
			// new mode when they differ. Linux links are always 0777;
			// a macOS link gets 0777 &^ umask unless it is set, so the
			// step asks for 0777 and every platform prints the same.
			{Op: "symlink", Path: "link", Target: "a.txt", Mode: mode(0o777)},
			mtime("a.txt", t0), mtime("sub/b.txt", t0), mtime("sub", t0), mtime("run.sh", t0), mtime("link", t0),
			wcCreate("trees/demo"),
			{Op: "ingest", Dir: ".", Into: "wc", Exclude: []string{".dstore"}, Var: "base"},
			{Op: "wc_state", BaseVar: str("base"), SyncedAtUnixNs: dec(syncedT)},
			write("a.txt", "one\n2\nthree\nfour\n", 0o644),
			{Op: "chmod", Path: "run.sh", Mode: mode(0o755)},
			{Op: "remove", Path: "sub/b.txt"},
			write("new.txt", "new file\n", 0o644),
			{Op: "remove", Path: "link"},
			mkdir("link", 0o755),
			write("link/inner.txt", "inner\n", 0o644),
			mtime("a.txt", t1), mtime("new.txt", t1), mtime("link/inner.txt", t1), mtime("link", t1), mtime("sub", t1),
		}},
		// port-notes/cli.md §3.5, second fixture.
		{Name: "wc2", Steps: []step{
			wcCreate("trees/demo2"),
			mkdir(".remote-src", 0o755),
			write(".remote-src/f1.txt", "beta\n", 0o644),
			write(".remote-src/f2.txt", "added remotely\n", 0o644),
			mtime(".remote-src/f1.txt", t1), mtime(".remote-src/f2.txt", t1), mtime(".remote-src", t1),
			{Op: "ingest", Dir: ".remote-src", Into: "wc", Exclude: []string{}, Var: "remote"},
			{Op: "remove", Path: ".remote-src"},
			write("f1.txt", "alpha\n", 0o644),
			write("keep.txt", "same\n", 0o644),
			mtime("f1.txt", t0), mtime("keep.txt", t0),
			{Op: "ingest", Dir: ".", Into: "wc", Exclude: []string{".dstore"}, Var: "base"},
			{Op: "wc_state", BaseVar: str("base"), RemoteVar: str("remote"), RemoteVersionHex: str("0102"), SyncedAtUnixNs: dec(syncedT)},
			write("f1.txt", "alpha\n", 0o644),
			write("keep.txt", "same\n", 0o644),
			mtime("f1.txt", t1), mtime("keep.txt", t1),
		}},
		// PORTING.md §2.3 (DD-2): a --local directory whose refs/ Go dstore
		// wrote as a Pebble database.
		{Name: "pebble-refs", Steps: []step{
			mkdir("src", 0o755),
			write("src/hello.txt", "hello\n", 0o644),
			mtime("src/hello.txt", t0),
			mtime("src", t0),
			{Op: "ingest", Dir: "src", Into: "scratch", Exclude: []string{}, Var: "src"},
			{Op: "pebble_refs", Path: "P/refs", Names: pebbleRefsNames},
		}},
	}
}

// lchmod gives the symlink p the permission bits m. macOS applies them
// (fchmodat with AT_SYMLINK_NOFOLLOW). Linux refuses with EOPNOTSUPP because
// its links always have 0777, so there the link must already have m.
func lchmod(p string, m uint32) error {
	err := unix.Fchmodat(unix.AT_FDCWD, p, m, unix.AT_SYMLINK_NOFOLLOW)
	var st unix.Stat_t
	if lerr := unix.Lstat(p, &st); lerr != nil {
		return lerr
	}
	got := uint32(st.Mode) & 0o7777
	switch {
	case got == m:
		return nil
	case err != nil:
		return fmt.Errorf("lchmod %#o: %w (the link has %#o)", m, err, got)
	default:
		return fmt.Errorf("lchmod %#o: the link has %#o", m, got)
	}
}

// build applies a fixture's steps under root and returns its key variables.
func build(fx fixture, root, scratch string) (map[string]key.Key, error) {
	vars := map[string]key.Key{}
	for i, s := range fx.Steps {
		if err := applyStep(s, root, scratch, vars); err != nil {
			return nil, fmt.Errorf("fixture %s step %d (%s %s): %w", fx.Name, i, s.Op, s.Path, err)
		}
	}
	return vars, nil
}

func applyStep(s step, root, scratch string, vars map[string]key.Key) error {
	p := filepath.Join(root, filepath.FromSlash(s.Path))
	switch s.Op {
	case "mkdir":
		if err := os.Mkdir(p, 0o700); err != nil {
			return err
		}
		return os.Chmod(p, os.FileMode(*s.Mode))
	case "write":
		if err := os.WriteFile(p, []byte(*s.Text), 0o600); err != nil {
			return err
		}
		return os.Chmod(p, os.FileMode(*s.Mode))
	case "symlink":
		if err := os.Symlink(s.Target, p); err != nil {
			return err
		}
		if s.Mode == nil {
			return nil
		}
		return lchmod(p, *s.Mode)
	case "remove":
		return os.RemoveAll(p)
	case "chmod":
		return os.Chmod(p, os.FileMode(*s.Mode))
	case "mtime":
		ts := unix.NsecToTimespec(int64(*s.UnixNs))
		return unix.UtimesNanoAt(unix.AT_FDCWD, p, []unix.Timespec{ts, ts}, unix.AT_SYMLINK_NOFOLLOW)
	case "wc_create":
		t, err := worktree.Create(root, *s.Config)
		if err != nil {
			return err
		}
		return t.Close()
	case "ingest":
		dir := filepath.Join(root, filepath.FromSlash(s.Dir))
		storeDir := filepath.Join(root, worktree.Dir, "packstore")
		if s.Into == "scratch" {
			d, err := os.MkdirTemp(scratch, "ingest-")
			if err != nil {
				return err
			}
			defer os.RemoveAll(d)
			storeDir = d
		} else if s.Into != "wc" {
			return fmt.Errorf("ingest into %q", s.Into)
		}
		st, err := packstore.Open(storeDir, packstore.WithSync(true))
		if err != nil {
			return err
		}
		k, _, err := ingest.Dir(st, dir, ingest.Opts{Jobs: 1, Exclude: s.Exclude})
		if cerr := st.Close(); err == nil {
			err = cerr
		}
		if err != nil {
			return err
		}
		vars[s.Var] = k
		return nil
	case "wc_state":
		lookup := func(name string) (key.Key, error) {
			if name == "" {
				k, _ := worktree.EmptyTree()
				return k, nil
			}
			k, ok := vars[name]
			if !ok {
				return key.Key{}, fmt.Errorf("no variable %q", name)
			}
			return k, nil
		}
		var st worktree.State
		var err error
		if st.Base, err = lookup(*s.BaseVar); err != nil {
			return err
		}
		if s.RemoteVar != nil {
			st.HasRemote = true
			if st.Remote, err = lookup(*s.RemoteVar); err != nil {
				return err
			}
			if s.RemoteVersionHex != nil {
				if st.RemoteVersion, err = hex.DecodeString(*s.RemoteVersionHex); err != nil {
					return err
				}
			}
		}
		st.SyncedAt = time.Unix(0, int64(*s.SyncedAtUnixNs)).UTC()
		return (&worktree.Tree{Root: root, State: st}).SaveState()
	case "pebble_refs":
		rs, err := refstore.Open(p, true)
		if err != nil {
			return err
		}
		if err := rs.Close(); err != nil {
			return err
		}
		entries, err := os.ReadDir(p)
		if err != nil {
			return err
		}
		names := make([]string, len(entries))
		for i, e := range entries {
			names[i] = e.Name()
		}
		sort.Strings(names)
		want := append([]string(nil), s.Names...)
		sort.Strings(want)
		if strings.Join(names, "\n") != strings.Join(want, "\n") {
			return fmt.Errorf("the Pebble refs directory holds %q, the step names %q", names, s.Names)
		}
		return nil
	}
	return fmt.Errorf("unknown op %q", s.Op)
}

// ---- cases ----

// spec is a case before it runs.
type spec struct {
	name, group string
	args        []string
	env         []envVar
	fixture     string
	subdir      string
	stdin       string
	// nodeSideKind is "A", "B" or "C" when the Rust CLI substitutes the
	// node-side behaviour of PORTING.md §2.2, or "DD-2" for the §2.3 Pebble
	// refs refusal; nodeSideCmd names the command for kind A, localDir the
	// --local value for kind DD-2.
	nodeSideKind, nodeSideCmd, localDir string
	// expect holds verified outputs (port-notes/cli.md §3) the run must
	// reproduce, a check of the harness itself.
	expect *output
}

func env(pairs ...string) []envVar {
	out := []envVar{}
	for i := 0; i+1 < len(pairs); i += 2 {
		out = append(out, envVar{Name: pairs[i], Value: pairs[i+1]})
	}
	return out
}

var (
	topCommands = []string{"cluster", "serve", "token", "node", "voter", "transition", "gc", "catalog", "store", "clone", "init", "fetch", "pull", "push", "status", "diff", "refs", "watch", "ref", "ls", "cat"}
	subcommands = []struct {
		parent string
		subs   []string
	}{
		{"cluster", []string{"init", "status", "ticket", "replicas"}},
		{"token", []string{"create"}},
		{"node", []string{"join", "remove", "drain", "weight", "zone", "repair"}},
		{"voter", []string{"add", "remove"}},
		{"transition", []string{"status", "abort", "refreeze", "pause", "resume"}},
		{"gc", []string{"run", "status", "hold", "release", "why"}},
		{"catalog", []string{"backup", "backups", "restore"}},
		{"store", []string{"push", "pull"}},
		{"ref", []string{"get", "delete"}},
	}
)

func cases() ([]spec, error) {
	validID := nodeID(1)
	hex64 := strings.Repeat("01", 32)
	hex62 := strings.Repeat("01", 31)
	invalidPoint := strings.Repeat("07", 32)
	base32ID := "aebagbafaydqqcikbmga2dqpcaireeyuculbogazdinryhi6d4qa"
	var cs []spec
	add := func(group, name string, args ...string) *spec {
		cs = append(cs, spec{name: group + "/" + name, group: group, args: args, env: []envVar{}, fixture: "empty"})
		return &cs[len(cs)-1]
	}
	name := func(args []string) string {
		if len(args) == 0 {
			return "no-args"
		}
		parts := make([]string, len(args))
		for i, a := range args {
			if a == "" {
				a = `""`
			}
			parts[i] = a
		}
		return strings.Join(parts, " ")
	}
	addArgs := func(group string, args ...string) *spec { return add(group, name(args), args...) }

	// ---- help, version, unknown commands ----
	for _, args := range [][]string{
		{}, {"--help"}, {"-h"}, {"-help"}, {"help"}, {"h"}, {""}, {"--version"}, {"-v"}, {"-version"},
		{"--version", "refs"}, {"-h", "-v"}, {"-v", "-h"}, {"help", "help"}, {"h", "h"}, {"--help", "refs"},
	} {
		addArgs("help", args...)
	}
	for _, top := range topCommands {
		addArgs("help", top, "--help")
		addArgs("help", "help", top)
	}
	for _, sc := range subcommands {
		addArgs("help", sc.parent)
		for _, sub := range sc.subs {
			addArgs("help", sc.parent, sub, "--help")
		}
	}
	for _, args := range [][]string{
		{"cluster", "help"}, {"cluster", "h"}, {"cluster", "help", "init"}, {"cluster", "h", "init"}, {"h", "cluster"},
		{"help", "cluster", "init"}, {"refs", "help"}, {"refs", "h"}, {"refs", "-h"}, {"watch", "h"}, {"refs", "--help", "extra"},
		{"store", "pull", "--local", "L", "help"}, {"node", "remove", "help"}, {"gc", "why", "h"}, {"diff", "help", "a.txt"},
	} {
		addArgs("help", args...)
	}
	// Help shows a flag's definition default, not its environment value (cli.md §2.2.5).
	s := add("help", "env DSTORE_PACK_SIZE=1Gi serve --help", "serve", "--help")
	s.env = env("DSTORE_PACK_SIZE", "1Gi")
	for _, args := range [][]string{
		{"nosuch"}, {"help", "nosuch"}, {"version"}, {"cluster", "nosuch"}, {"ls", "help", "x"}, {"cluster", "help", "nosuch"},
		{"store", "nosuch"}, {"refs", "help", "refs"}, {"Refs"},
	} {
		addArgs("unknown", args...)
	}

	// ---- usage errors ----
	for _, args := range [][]string{
		{"--bogus"}, {"--log-level"}, {"-ab"}, {"---x"}, {"-=x"},
		{"refs", "--bogus"}, {"cluster", "status", "--bogus"}, {"cluster", "--bogus"}, {"store", "push", "--bogus"},
		{"node", "join", "--bogus"}, {"serve", "--bogus"}, {"diff", "--bogus"}, {"status", "--bogus"},
		{"refs", "---x"}, {"refs", "-=x"}, {"refs", "--ticket"}, {"refs", "--log-level", "debug"}, {"refs", "--no-relay=maybe"},
		{"refs", "--no-relay="}, {"cluster", "replicas", "--yes=maybe", "3"}, {"store", "pull", "--local", "L", "--jobs", "x", "n"},
		{"gc", "run", "--garbage", "x"}, {"gc", "run", "--garbage", "1e400"}, {"serve", "--gc-interval", "x"}, {"status", "--ticket", "x"},
		{"token", "create", "--weight", "-1"}, {"token", "create", "--weight", "18446744073709551616"},
		{"serve", "--jobs", "9223372036854775808"}, {"serve", "--rate", "1.5"}, {"cluster", "init", "--replicas", "x"},
		{"diff", "--stat=maybe"}, {"serve", "--advertise-addr"}, {"diff", "--jobs", "0x"}, {"clone", "--local", "L", "trees/x"},
		// push --message/-m (dstore v0.1.10) without its value: the alias is named as it was given.
		{"push", "-m"}, {"push", "--message"}, {"push", "--force", "-m"},
	} {
		addArgs("usage", args...)
	}
	s = add("usage", "env DSTORE_NO_DISCOVERY=maybe refs", "refs")
	s.env = env("DSTORE_NO_DISCOVERY", "maybe")
	s = add("usage", "env DSTORE_NO_TUI=maybe store pull --local L n", "store", "pull", "--local", "L", "n")
	s.env = env("DSTORE_NO_TUI", "maybe")
	s = add("usage", "env DSTORE_NO_DISCOVERY=maybe cluster status", "cluster", "status")
	s.env = env("DSTORE_NO_DISCOVERY", "maybe")

	// ---- required flags ----
	for _, args := range [][]string{
		{"store", "pull"}, {"store", "pull", "n"}, {"node", "join"}, {"node", "join", "--seed", "x"}, {"node", "join", "--token", "x"},
		{"node", "join", "extra"}, {"node", "join", "--store", "s"}, {"store", "push"}, {"store", "push", "p", "n"}, {"store", "push", "help"},
		// The shared help command takes its HelpName from the parent on the path (dstore node help).
		{"node", "join", "help"},
	} {
		addArgs("required", args...)
	}
	s = add("required", "env AMBER_STORE= store pull", "store", "pull")
	s.env = env("AMBER_STORE", "")
	s = add("required", "env AMBER_STORE=L store pull", "store", "pull")
	s.env = env("AMBER_STORE", "L")

	// ---- argument validation before dialing ----
	for _, args := range [][]string{
		{"refs"}, {"refs", "--ticket="}, {"refs", "--ticket", "bogus"}, {"refs", "-ticket", "bogus"}, {"refs", "--ticket=bogus"},
		{"refs", "--ticket", "dstore1!!!"}, {"refs", "--ticket", "dstore1"}, {"refs", "--ticket", "dstore1aaaa"}, {"refs", "--ticket", "zz,yy"},
		{"refs", "--ticket", "   "}, {"refs", "--ticket", invalidPoint}, {"refs", "--ticket", validID[:63]}, {"refs", "--ticket", validID + ";" + validID},
		{"refs", "--ticket", validID, "--relay", ":x"}, {"refs", "--ticket", validID, "--relay", "http://[::1"},
		{"refs", "--relay", ":x"}, {"--log-level", "debug", "refs"}, {"--log-level", "bogus", "refs"}, {"refs", "--no-relay", "false"},
		{"refs", "--", "--bogus"}, {"refs", "-", "x"}, {"refs", "--ticket", "--bogus"}, {"cluster", "ticket", "--ticket", "bogus", "--store", "nostore"},
		{"store", "push", "--local", "/nonexistent", "p"},
		{"cat", "x"}, {"cat", "a", "b", "--ticket", "x"}, {"cat", "NAME", "./x"}, {"watch"}, {"watch", ""}, {"watch", "p", "--ticket", "x"},
		{"ls"}, {"ls", "NAME", "//"}, {"ls", "NAME", "../x"},
		{"clone"}, {"clone", ""}, {"clone", "a@b"}, {"clone", "trees/x"}, {"clone", "--ticket", "bogus", "trees/x"}, {"clone", "bad\x01name"},
		{"init"}, {"init", "trees/x"}, {"init", "bad@@"}, {"init", "--ticket", "bogus", "trees/x"},
		{"diff", "--remote", "--incoming"},
		{"node", "remove", "zz"}, {"node", "remove"}, {"node", "weight", validID, "x"}, {"node", "weight", validID},
		{"node", "weight", validID, "4294967296"}, {"node", "zone", validID}, {"node", "drain"}, {"node", "repair", "zz"},
		{"voter", "add"}, {"voter", "add", base32ID}, {"voter", "remove", "zz"},
		{"gc", "why", "zz"}, {"gc", "why", hex64}, {"gc", "why", hex62}, {"gc", "run"},
		{"catalog", "restore"}, {"catalog", "restore", hex64}, {"catalog", "backup"},
		{"cluster", "replicas", "x"}, {"cluster", "replicas"}, {"cluster", "replicas", "300"}, {"cluster", "replicas", "+3"},
		{"cluster", "replicas", "0x3"}, {"cluster", "replicas", "--yes", "3"}, {"cluster", "ticket"}, {"cluster", "status"},
		{"token", "create"}, {"transition", "status"}, {"ref", "get", "x"}, {"ref", "delete", "x", "--expected-version", "zz"},
		{"store", "push", "--local", "L", "p"}, {"store", "push", "--local", "L", "p", "bad@name"}, {"store", "push", "--local", "L", "p", ""},
		{"store", "push", "--local", "L", "p", "@"}, {"store", "push", "--local", "{CWD}/x", "a"}, {"store", "push", "--local", "{CWD}/x", "src", "bad@@"},
		{"store", "pull", "--local", "{CWD}/x"}, {"store", "pull", "--local", "/nonexistent"}, {"store", "pull", "--local", "L", "n"},
	} {
		addArgs("validation", args...)
	}
	for _, e := range [][2]string{{"DSTORE_TICKET", "bogus"}, {"DSTORE_TICKET", ""}, {"DSTORE_NO_DISCOVERY", ""}, {"DSTORE_NO_DISCOVERY", "1"},
		{"DSTORE_NO_DISCOVERY", "t"}, {"DSTORE_NO_DISCOVERY", "FALSE"}, {"DSTORE_LOG_LEVEL", "debug"}, {"DSTORE_TICKET", validID + ",zz"}} {
		s = add("validation", "env "+e[0]+"="+e[1]+" refs", "refs")
		s.env = env(e[0], e[1])
	}
	s = add("validation", "env DSTORE_NO_DISCOVERY=maybe DSTORE_TICKET=bogus clone trees/x", "clone", "trees/x")
	s.env = env("DSTORE_NO_DISCOVERY", "maybe", "DSTORE_TICKET", "bogus")
	s = add("validation", "env DSTORE_NO_TUI=1 store pull --local L n", "store", "pull", "--local", "L", "n")
	s.env = env("DSTORE_NO_TUI", "1")
	s = add("validation", "files catalog restore backup.bin", "catalog", "restore", "backup.bin")
	s.fixture = "files"
	s = add("validation", "files store push --local L src trees/x", "store", "push", "--local", "L", "src", "trees/x")
	s.fixture = "files"
	// reference.ValidateName checks the length before the characters.
	add("validation", "clone <1025 bytes>", "clone", strings.Repeat("a", 1025))
	// PORTING.md §2.3 (DD-2): Go uses the Pebble refs; Rust refuses them at open.
	for _, args := range [][]string{{"store", "pull", "--local", "P", "trees/x"}, {"store", "push", "--local", "P", "src", "trees/x"}} {
		s = add("validation", "pebble-refs "+name(args), args...)
		s.fixture, s.nodeSideKind, s.localDir = "pebble-refs", "DD-2", "P"
	}

	// ---- node-side validation up to the store step (identical in Rust) ----
	for _, args := range [][]string{
		{"serve"}, {"cluster", "init"}, {"node", "join", "--seed", validID, "--token", hex64}, {"serve", "--store", "X", "--pack-size", "0"},
		{"serve", "--store", "X", "--pack-size", "1.5Gi"}, {"cluster", "init", "--store", "X", "--pack-size", "-1"},
		{"node", "join", "--seed", "bogus", "--token", "x"}, {"node", "join", "--seed", validID, "--token", "zz"},
		{"node", "join", "--seed", validID, "--token", hex62}, {"node", "join", "--seed", "", "--token", ""},
		{"node", "join", "--seed", validID, "--token", hex64, "--pack-size", "0"},
		{"node", "join", "--seed", validID, "--token", hex64, "--store", "X", "--pack-size", "0"},
		{"node", "join", "--seed", invalidPoint, "--token", hex64},
		{"cluster", "ticket", "--store", "nostore"}, {"cluster", "status", "--store", "nostore"},
		{"catalog", "restore", "--store", "nostore", hex64},
	} {
		addArgs("node-side", args...)
	}
	s = add("node-side", "env DSTORE_STORE= serve", "serve")
	s.env = env("DSTORE_STORE", "")
	s = add("node-side", "env DSTORE_PACK_SIZE=x serve --store X", "serve", "--store", "X")
	s.env = env("DSTORE_PACK_SIZE", "x")
	s = add("node-side", "env DSTORE_STORE=nostore cluster status", "cluster", "status")
	s.env = env("DSTORE_STORE", "nostore")
	s = add("node-side", "files cluster ticket --store dirident", "cluster", "ticket", "--store", "dirident")
	s.fixture = "files"

	// ---- node-side paths the Rust CLI replaces (PORTING.md §2.2) ----
	nodeSideCase := func(kind, cmd, fixtureName string, args ...string) *spec {
		s := addArgs("node-side", args...)
		s.name = "node-side/" + fixtureName + " " + name(args)
		s.fixture, s.nodeSideKind, s.nodeSideCmd = fixtureName, kind, cmd
		return s
	}
	nr := []string{"--no-relay", "--no-discovery"}
	nodeSideCase("A", "serve", "empty", append([]string{"serve", "--store", "X"}, nr...)...)
	nodeSideCase("A", "serve", "empty", append([]string{"serve", "--store", "X", "--advertise-addr", "x"}, nr...)...)
	nodeSideCase("A", "serve", "empty", append([]string{"serve", "--store", "X", "--bind", "x"}, nr...)...)
	nodeSideCase("A", "serve", "files", append([]string{"serve", "--store", "backup.bin/x"}, nr...)...)
	nodeSideCase("A", "cluster init", "empty", append([]string{"cluster", "init", "--store", "X", "--weight", "100"}, nr...)...)
	nodeSideCase("A", "cluster init", "empty", append([]string{"cluster", "init", "--store", "X", "--weight", "x"}, nr...)...)
	nodeSideCase("A", "node join", "empty", append([]string{"node", "join", "--seed", validID, "--token", hex64, "--store", "X", "--weight", "x"}, nr...)...)
	nodeSideCase("A", "node join", "empty", append([]string{"node", "join", "--seed", validID, "--token", hex64, "--store", "X", "--weight", "100"}, nr...)...)
	nodeSideCase("B", "", "files", "cluster", "ticket", "--store", "node")
	nodeSideCase("B", "", "files", "cluster", "ticket", "--store", "node", "--ids")
	nodeSideCase("B", "", "files", "cluster", "status", "--store", "node")
	nodeSideCase("B", "", "files", "catalog", "restore", "--store", "node", hex64)
	s = nodeSideCase("B", "", "files", "cluster", "ticket")
	s.name = "node-side/files env DSTORE_STORE=node cluster ticket"
	s.env = env("DSTORE_STORE", "node")
	nodeSideCase("C", "", "files", "catalog", "restore", "--store", "node", "backup.bin")
	nodeSideCase("C", "", "files", "catalog", "restore", "--store", "nostore", "backup.bin")

	// ---- prompts ----
	for _, p := range []struct{ r, stdin string }{
		{"3", "n\n"}, {"3", ""}, {"3", "yes please\n"}, {"0", "Y"}, {"2", "N\n"}, {"2", "\n"}, {"2", "  y\n"}, {"2", "no\n"}, {"2", "y extra\n"},
		{"255", "yes\n"},
	} {
		s = add("prompt", "cluster replicas "+p.r+" stdin "+strconv.Quote(p.stdin), "cluster", "replicas", p.r)
		s.stdin = p.stdin
	}

	// ---- working copies (offline) ----
	wc := func(fixtureName, subdir string, args ...string) *spec {
		s := add("wc", fixtureName+" "+name(args), args...)
		s.fixture, s.subdir = fixtureName, subdir
		if subdir != "" {
			s.name = "wc/" + fixtureName + "/" + subdir + " " + name(args)
		}
		return s
	}
	for _, args := range [][]string{{"status"}, {"fetch"}, {"pull"}, {"push"}, {"diff"}, {"diff", "--stat"}, {"diff", "--remote=false", "--incoming"}} {
		wc("empty", "", args...)
	}
	wc("wc-no-config", "", "status")
	wc("wc-no-config", "", "diff")
	wc("wc-no-config", "", "push")
	wc("wc-bad-config", "", "status")
	wc("wc-no-state", "", "status")
	wc("wc-no-state", "", "fetch")
	wc("wc-bad-state-key", "", "status")
	for _, args := range [][]string{
		{"status"}, {"diff"}, {"diff", "--stat"}, {"diff", "--incoming"}, {"diff", "--remote"}, {"push"}, {"push", "--user", ""},
		{"fetch", "--ticket", "zz"}, {"pull"}, {"init", "--ticket", "bogus", "trees/x"}, {"clone", "--ticket", "bogus", "trees/x"},
	} {
		wc("wc-no-remote", "", args...)
	}
	for _, args := range [][]string{
		{"status"}, {"diff"}, {"diff", "--stat"}, {"diff", "a.txt"}, {"diff", "a.txt", "--stat"}, {"diff", "../"}, {"diff", "sub"},
		{"diff", "--incoming"}, {"diff", "--remote"}, {"push"}, {"push", "--user", ""}, {"fetch", "--ticket", "zz"},
		{"init", "--ticket", "bogus", "trees/x"},
		// push --message/-m (dstore v0.1.10): the only aliased flag of a command.
		{"push", "-m", "msg"}, {"push", "--message=msg", "--user", ""}, {"push", "-message", "msg"},
	} {
		wc("wc1", "", args...)
	}
	wc("wc1", "sub", "status")
	wc("wc1", "sub", "diff")
	wc("wc1", "sub", "diff", "..")
	wc("wc1", "sub", "diff", "../a.txt", "b.txt")
	for _, args := range [][]string{
		{"status"}, {"diff"}, {"diff", "--incoming"}, {"diff", "--incoming", "--stat"}, {"diff", "--remote"}, {"diff", "--remote", "--stat"}, {"push"},
	} {
		wc("wc2", "", args...)
	}

	// Verified outputs (port-notes/cli.md §3.2-§3.5) the harness must
	// reproduce; a case named here that does not exist fails the run.
	expect := map[string]output{
		"help/--version refs":                      {Exit: 0, Stdout: "dstore version dev\n"},
		"unknown/nosuch":                           {Exit: 3, Stderr: "No help topic for 'nosuch'\n"},
		"required/store pull n":                    {Exit: 1, Stderr: "dstore: Required flag \"local\" not set\n"},
		"validation/refs":                          {Exit: 1, Stderr: "dstore: no cluster: set --ticket or $DSTORE_TICKET\n"},
		"validation/refs --ticket bogus":           {Exit: 1, Stderr: "dstore: ticket: \"bogus\" is neither a dstore1 ticket nor a node id: invalid length\n"},
		"validation/refs --ticket dstore1!!!":      {Exit: 1, Stderr: "dstore: ticket: illegal base32 data at input byte 0\n"},
		"validation/gc why zz":                     {Exit: 1, Stderr: "dstore: why KEY (64 hex chars)\n"},
		"prompt/cluster replicas 3 stdin \"n\\n\"": {Exit: 1, Stdout: "changing R to 3 moves about 1/3 of every node's data; continue? [y/N] ", Stderr: "dstore: aborted\n"},
		"node-side/serve --store X --pack-size 0":  {Exit: 1, Stderr: "dstore: --pack-size: 0 is not a positive size\n"},
		"wc/wc-no-state status":                    {Exit: 1, Stderr: "dstore: incomplete clone: delete the directory and clone again\n"},
		"wc/wc-bad-state-key status":               {Exit: 1, Stderr: "dstore: bad state file: base: encoding/hex: invalid byte: U+007A 'z'\n"},
		"wc/wc-no-config status":                   {Exit: 1, Stderr: "dstore: working copy {CWD}: open {CWD}/.dstore/config: no such file or directory\n"},
		"wc/wc1 status": {Exit: 0, Stdout: "reference trees/demo, synced to {key16:base}\nremote: the reference does not exist on the cluster\nchanges:\n" +
			"  modified  a.txt\n  type      link/ (symlink → directory)\n  new       link/inner.txt\n  new       new.txt\n" +
			"  mode      run.sh (0644 → 0755)\n  deleted   sub/b.txt\n1 paths differ only in mtime, ownership or xattrs\n"},
		"wc/wc2 status": {Exit: 0, Stdout: "reference trees/demo2, synced to {key16:base}\nremote: moved since your last fetch (+1 ~1 -1; run pull)\n" +
			"2 paths differ only in mtime, ownership or xattrs\n"},
	}
	for i := range cs {
		if want, ok := expect[cs[i].name]; ok {
			w := want
			cs[i].expect = &w
			delete(expect, cs[i].name)
		}
	}
	if len(expect) > 0 {
		var names []string
		for n := range expect {
			names = append(names, n)
		}
		sort.Strings(names)
		return nil, fmt.Errorf("expectations name no case: %q", names)
	}
	return cs, nil
}

// ---- running ----

func run(outDir string, verbose bool) (err error) {
	if err := mainpkg.SelfCheck(); err != nil {
		return err
	}
	work, err := os.MkdirTemp("", "clisnap-")
	if err != nil {
		return err
	}
	defer func() {
		if rmErr := os.RemoveAll(work); err == nil {
			err = rmErr
		}
	}()
	work, err = filepath.EvalSymlinks(work)
	if err != nil {
		return err
	}
	bin := filepath.Join(work, "bin", "dstore")
	gen, err := buildDstore(bin)
	if err != nil {
		return err
	}
	fxs := fixtures()
	byName := map[string]fixture{}
	for _, fx := range fxs {
		byName[fx.Name] = fx
	}
	snap := snapFile{
		Generator:   gen,
		Environment: environment{Inherited: []string{"PATH"}, Set: []envVar{{Name: "HOME", Value: "{HOME}"}, {Name: "TZ", Value: "UTC"}}},
		Fixtures:    fxs,
	}
	specs, err := cases()
	if err != nil {
		return err
	}
	seen := map[string]bool{}
	for i, sp := range specs {
		if seen[sp.name] {
			return fmt.Errorf("case %q defined twice", sp.name)
		}
		seen[sp.name] = true
		fx, ok := byName[sp.fixture]
		if !ok {
			return fmt.Errorf("case %s: no fixture %q", sp.name, sp.fixture)
		}
		if verbose {
			fmt.Fprintf(os.Stderr, "clisnap: %s\n", sp.name)
		}
		c, err := runCase(bin, filepath.Join(work, "cases", strconv.Itoa(i)), fx, sp)
		if err != nil {
			return fmt.Errorf("case %s: %w", sp.name, err)
		}
		if strings.Contains(c.Stdout, work) || strings.Contains(c.Stderr, work) {
			return fmt.Errorf("case %s: the output names the work directory:\n%s%s", sp.name, c.Stdout, c.Stderr)
		}
		snap.Cases = append(snap.Cases, c)
	}
	b, err := json.MarshalIndent(snap, "", "  ")
	if err != nil {
		return err
	}
	if err := os.MkdirAll(outDir, 0o755); err != nil {
		return err
	}
	return os.WriteFile(filepath.Join(outDir, "snapshots.json"), append(b, '\n'), 0o644)
}

// buildDstore builds cmd/dstore of the selected dstore module and returns
// its build information, checked against mainpkg.DstoreVersion.
func buildDstore(bin string) (generator, error) {
	cmd := exec.Command("go", "build", "-trimpath", "-o", bin, mainpkg.DstoreModule+"/cmd/dstore")
	cmd.Env = append(os.Environ(), "CGO_ENABLED=0")
	cmd.Stdout, cmd.Stderr = os.Stderr, os.Stderr
	if err := cmd.Run(); err != nil {
		return generator{}, fmt.Errorf("go build %s/cmd/dstore: %w", mainpkg.DstoreModule, err)
	}
	bi, err := buildinfo.ReadFile(bin)
	if err != nil {
		return generator{}, err
	}
	if bi.Main.Path != mainpkg.DstoreModule || bi.Main.Version != mainpkg.DstoreVersion {
		return generator{}, fmt.Errorf("built %s %s, want %s %s", bi.Main.Path, bi.Main.Version, mainpkg.DstoreModule, mainpkg.DstoreVersion)
	}
	gen := generator{Program: program, Go: bi.GoVersion, Module: modVersion{Path: bi.Main.Path, Version: bi.Main.Version}, Deps: []modVersion{}}
	wanted := map[string]bool{
		"github.com/urfave/cli/v2": true, "charm.land/bubbletea/v2": true, "charm.land/lipgloss/v2": true, "charm.land/bubbles/v2": true,
		"github.com/amber-store/core": true, "github.com/amber-store/transport-iroh": true, "github.com/tmc/go-iroh": true,
		"github.com/fxamacker/cbor/v2": true, "github.com/aymanbagabas/go-udiff": true,
	}
	for _, d := range bi.Deps {
		if wanted[d.Path] {
			gen.Deps = append(gen.Deps, modVersion{Path: d.Path, Version: d.Version})
		}
	}
	sort.Slice(gen.Deps, func(i, j int) bool { return gen.Deps[i].Path < gen.Deps[j].Path })
	if len(gen.Deps) != len(wanted) {
		return generator{}, fmt.Errorf("the build information lists %d of the %d expected dependencies", len(gen.Deps), len(wanted))
	}
	return gen, nil
}

// runCase builds the case's fixture under dir, runs the binary there and
// returns the normalised result.
func runCase(bin, dir string, fx fixture, sp spec) (snapCase, error) {
	root := filepath.Join(dir, "root")
	home := filepath.Join(dir, "home")
	scratch := filepath.Join(dir, "scratch")
	for _, d := range []string{root, home, scratch} {
		if err := os.MkdirAll(d, 0o755); err != nil {
			return snapCase{}, err
		}
	}
	vars, err := build(fx, root, scratch)
	if err != nil {
		return snapCase{}, err
	}
	cwd := filepath.Join(root, filepath.FromSlash(sp.subdir))
	if cwd, err = filepath.EvalSymlinks(cwd); err != nil {
		return snapCase{}, err
	}
	if root, err = filepath.EvalSymlinks(root); err != nil {
		return snapCase{}, err
	}
	args := make([]string, len(sp.args))
	for i, a := range sp.args {
		args[i] = strings.ReplaceAll(a, placeholderCWD, cwd)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, bin, args...)
	cmd.Dir = cwd
	// QUIC_GO_DISABLE_RECEIVE_BUFFER_WARNING: on Linux hosts with small UDP buffer limits the Go
	// binary's quic-go fork logs a buffer warning when it binds (PORTING.md DD-16); it is the host's
	// output, not dstore's, so the snapshots leave it out.
	cmd.Env = []string{"PATH=" + os.Getenv("PATH"), "HOME=" + home, "TZ=UTC", "QUIC_GO_DISABLE_RECEIVE_BUFFER_WARNING=true"}
	for _, e := range sp.env {
		cmd.Env = append(cmd.Env, e.Name+"="+strings.ReplaceAll(e.Value, placeholderCWD, cwd))
	}
	cmd.Stdin = strings.NewReader(sp.stdin)
	var stdout, stderr bytes.Buffer
	cmd.Stdout, cmd.Stderr = &stdout, &stderr
	cmd.WaitDelay = 5 * time.Second
	runErr := cmd.Run()
	if ctx.Err() != nil {
		return snapCase{}, fmt.Errorf("timed out after 60 s")
	}
	var exitErr *exec.ExitError
	if runErr != nil && !errors.As(runErr, &exitErr) {
		return snapCase{}, runErr
	}
	if !cmd.ProcessState.Exited() {
		return snapCase{}, fmt.Errorf("the process did not exit normally: %v", cmd.ProcessState)
	}
	c := snapCase{
		Name: sp.name, Group: sp.group, Args: sp.args, Env: sp.env, Fixture: sp.fixture, Subdir: sp.subdir, Stdin: sp.stdin,
		Exit:   cmd.ProcessState.ExitCode(),
		Stdout: normalise(stdout.String(), cwd, root, vars),
		Stderr: normalise(stderr.String(), cwd, root, vars),
	}
	if !utf8.ValidString(c.Stdout) || !utf8.ValidString(c.Stderr) {
		return snapCase{}, fmt.Errorf("the output is not valid UTF-8")
	}
	if sp.nodeSideKind != "" {
		ns, err := nodeSideOf(sp)
		if err != nil {
			return snapCase{}, err
		}
		c.Stdout, ns.GoNormalized = normaliseNodeSide(c.Stdout, nil)
		c.Stderr, ns.GoNormalized = normaliseNodeSide(c.Stderr, ns.GoNormalized)
		c.NodeSide = ns
	}
	if sp.expect != nil {
		got := output{Exit: c.Exit, Stdout: c.Stdout, Stderr: c.Stderr}
		if got != *sp.expect {
			return snapCase{}, fmt.Errorf("the harness does not reproduce the verified output:\n got %+v\nwant %+v", got, *sp.expect)
		}
	}
	return c, nil
}

// nodeSideOf is the Rust CLI's output for a case of PORTING.md §2.2 or §2.3.
func nodeSideOf(sp spec) (*nodeSide, error) {
	var text string
	switch sp.nodeSideKind {
	case "A":
		if sp.nodeSideCmd == "" {
			return nil, fmt.Errorf("node-side kind A without a command")
		}
		text = nodeSideCommandText(sp.nodeSideCmd)
	case "B":
		text = storeTicketUnsupported
	case "C":
		text = catalogRestoreUnsupported
	case "DD-2":
		if sp.localDir == "" {
			return nil, fmt.Errorf("kind DD-2 without a --local directory")
		}
		text = pebbleRefsText(sp.localDir)
	default:
		return nil, fmt.Errorf("node-side kind %q", sp.nodeSideKind)
	}
	return &nodeSide{Kind: sp.nodeSideKind, GoNormalized: []string{}, Rust: output{Exit: 1, Stdout: "", Stderr: "dstore: " + text + "\n"}}, nil
}

// normalise replaces the case's directories with placeholders, then every
// key variable of the fixture: the 64 hex digits as {key:NAME}, the first 16
// as {key16:NAME}.
func normalise(s, cwd, root string, vars map[string]key.Key) string {
	if cwd != root {
		s = strings.ReplaceAll(s, cwd, placeholderCWD)
		s = strings.ReplaceAll(s, root, placeholderROOT)
	} else {
		s = strings.ReplaceAll(s, cwd, placeholderCWD)
	}
	names := make([]string, 0, len(vars))
	for n := range vars {
		names = append(names, n)
	}
	sort.Strings(names)
	for _, n := range names {
		full := vars[n].String()
		s = strings.ReplaceAll(s, full, "{key:"+n+"}")
		s = strings.ReplaceAll(s, full[:16], "{key16:"+n+"}")
	}
	return s
}

var (
	slogLine  = regexp.MustCompile(`(?m)^time=\S+ level=.*$`)
	hex64Word = regexp.MustCompile(`\b[0-9a-f]{64}\b`)
	ticketStr = regexp.MustCompile(`\bdstore1[a-z2-7]+\b`)
)

// normaliseNodeSide replaces what a node-side Go run prints at random (slog
// lines with times and addresses, node ids, tickets) and appends the
// placeholders it applied to used.
func normaliseNodeSide(s string, used []string) (string, []string) {
	for _, r := range []struct {
		re          *regexp.Regexp
		placeholder string
	}{{slogLine, "{SLOG}"}, {ticketStr, "{TICKET}"}, {hex64Word, "{HEX64}"}} {
		if r.re.MatchString(s) {
			s = r.re.ReplaceAllString(s, r.placeholder)
			if !contains(used, r.placeholder) {
				used = append(used, r.placeholder)
			}
		}
	}
	if used == nil {
		used = []string{}
	}
	return s, used
}

func contains(list []string, s string) bool {
	for _, x := range list {
		if x == s {
			return true
		}
	}
	return false
}
