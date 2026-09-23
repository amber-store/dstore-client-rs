package main

// Family worktree: working-copy vectors from github.com/amber-store/dstore v0.1.10 package worktree over
// github.com/amber-store/core v0.0.9 (port-notes/worktree.md §5 items 1-16, port-notes/verification.md
// §4.3 items 13-18 and 22). Schemas: docs/vectorgen-worktree.md.
//
// Every top-level identifier of this file starts with "wt" so that the families of other owners never
// collide with it in package main.

import (
	"bytes"
	"context"
	"crypto/ed25519"
	_ "embed"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"go/ast"
	"go/parser"
	"go/printer"
	"go/token"
	"io/fs"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"runtime/debug"
	"sort"
	"strconv"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/amber-store/core/cborx"
	"github.com/amber-store/core/chunkers"
	"github.com/amber-store/core/commit"
	"github.com/amber-store/core/fstree"
	"github.com/amber-store/core/ingest"
	"github.com/amber-store/core/key"
	"github.com/amber-store/core/packstore"
	"github.com/amber-store/core/reference"
	"github.com/amber-store/dstore/client"
	"github.com/amber-store/dstore/view"
	"github.com/amber-store/dstore/worktree"
	"golang.org/x/sys/unix"
)

// wtSource is this file, parsed by the self-check to compare the verbatim copies of cmd/dstore functions
// with the module's originals.
//
//go:embed family_worktree.go
var wtSource []byte

func init() {
	register("worktree", []string{
		"errors/worktree_text.json",
		"worktree/cli.json",
		"worktree/config.json",
		"worktree/diff_trees.json",
		"worktree/merge.json",
		"worktree/state.json",
		"worktree/trees",
		"worktree/unified.json",
	}, wtGenerate)
}

func wtGenerate(out string) error {
	if err := wtSelfCheck(); err != nil {
		return fmt.Errorf("self-check: %w", err)
	}
	st, err := wtBuildTrees()
	if err != nil {
		return fmt.Errorf("worktree/trees: %w", err)
	}
	steps := []struct {
		rel string
		gen func() (any, error)
	}{
		{"worktree/config.json", wtConfigVectors},
		{"worktree/state.json", wtStateVectors},
		{"worktree/diff_trees.json", func() (any, error) { return wtDiffTreesVectors(st) }},
		{"worktree/merge.json", wtMergeVectors},
		{"worktree/unified.json", func() (any, error) { return wtUnifiedVectors(st) }},
		{"worktree/cli.json", wtCLIVectors},
		{"errors/worktree_text.json", wtErrorVectors},
	}
	for _, s := range steps {
		v, err := s.gen()
		if err != nil {
			return fmt.Errorf("%s: %w", s.rel, err)
		}
		if err := wtCheckUTF8(reflect.ValueOf(v), s.rel); err != nil {
			return err
		}
		if err := writeJSON(filepath.Join(out, filepath.FromSlash(s.rel)), v); err != nil {
			return err
		}
	}
	return st.write(filepath.Join(out, "worktree", "trees"))
}

// ---- shared helpers ----

// wtT0 is the base of the fixture mtimes (2020-09-13T12:26:40Z).
const wtT0 = 1_600_000_000

// wtSynced is the synced_at of the scan and disk-diff cases unless a case says otherwise.
const wtSynced = 1_700_000_000

// wtMt is s seconds plus ns nanoseconds, as an mtime in nanoseconds.
func wtMt(s, ns int64) int64 { return s*1_000_000_000 + ns }

// wtText splits a Go string into a text field (valid UTF-8) or a hex field (anything else).
func wtText(s string) (text, hexText *string) {
	if utf8.ValidString(s) {
		return &s, nil
	}
	h := hex.EncodeToString([]byte(s))
	return nil, &h
}

// wtErrText is err's text (nil for nil) with the temporary root replaced by {ROOT}.
func wtErrText(err error, root string) *string {
	if err == nil {
		return nil
	}
	s := err.Error()
	if root != "" {
		s = strings.ReplaceAll(s, root, "{ROOT}")
	}
	return &s
}

// wtCheckUTF8 walks a vector value and fails on a map (output order must be fixed) or on any string that is
// not valid UTF-8: encoding/json would silently replace its invalid bytes with U+FFFD, and the vector would no
// longer hold Go's bytes. Such strings belong in a _hex field (wtText).
func wtCheckUTF8(v reflect.Value, path string) error {
	switch v.Kind() {
	case reflect.String:
		if !utf8.ValidString(v.String()) {
			return fmt.Errorf("%s: %q is not valid UTF-8 (use a _hex field)", path, v.String())
		}
	case reflect.Pointer, reflect.Interface:
		if !v.IsNil() {
			return wtCheckUTF8(v.Elem(), path)
		}
	case reflect.Struct:
		for i := 0; i < v.NumField(); i++ {
			if err := wtCheckUTF8(v.Field(i), path+"."+v.Type().Field(i).Name); err != nil {
				return err
			}
		}
	case reflect.Slice, reflect.Array:
		if v.Type().Elem().Kind() == reflect.Uint8 {
			return nil // bytes marshal as hex
		}
		for i := 0; i < v.Len(); i++ {
			if err := wtCheckUTF8(v.Index(i), fmt.Sprintf("%s[%d]", path, i)); err != nil {
				return err
			}
		}
	case reflect.Map:
		return fmt.Errorf("%s: a map in vector output", path)
	}
	return nil
}

// wtTemp makes a fresh temporary directory and returns a function deleting it.
func wtTemp() (string, func(), error) {
	dir, err := os.MkdirTemp("", "vectorgen-worktree-")
	if err != nil {
		return "", nil, err
	}
	return dir, func() { wtRemoveAll(dir) }, nil
}

// wtRemoveAll deletes dir, first giving the owner rwx on every directory below it.
func wtRemoveAll(dir string) {
	_ = filepath.WalkDir(dir, func(p string, d fs.DirEntry, err error) error {
		if err == nil && d.IsDir() {
			_ = unix.Chmod(p, 0o700)
		}
		return nil
	})
	_ = os.RemoveAll(dir)
}

// wtBlobKey is the canonical Blob key of b.
func wtBlobKey(b []byte) key.Key {
	k, err := key.New(key.Blob, uint64(len(b)), b)
	if err != nil {
		panic(err) // Blob is a defined type
	}
	return k
}

// ---- JSON shapes shared by several files ----

// wtEntryJSON is a fstree.Entry. name and link_target are text or _hex; the other byte fields are hex and
// omitted when empty (CBOR omitempty: nil and empty are the same entry).
type wtEntryJSON struct {
	Name          *string `json:"name,omitempty"`
	NameHex       *string `json:"name_hex,omitempty"`
	Mode          U64     `json:"mode"`
	UID           U64     `json:"uid"`
	GID           U64     `json:"gid"`
	Mtime         I64     `json:"mtime"`
	ContentKey    Hex     `json:"content_key,omitempty"`
	LinkTarget    *string `json:"link_target,omitempty"`
	LinkTargetHex *string `json:"link_target_hex,omitempty"`
	Rdev          []U64   `json:"rdev,omitempty"`
	XattrsIn      Hex     `json:"xattrs_in,omitempty"`
	XattrsKey     Hex     `json:"xattrs_key,omitempty"`
}

func wtEntry(e *fstree.Entry) *wtEntryJSON {
	if e == nil {
		return nil
	}
	j := &wtEntryJSON{
		Mode:       U64(e.Mode),
		UID:        U64(e.UID),
		GID:        U64(e.GID),
		Mtime:      I64(e.Mtime),
		ContentKey: Hex(e.ContentKey),
		XattrsIn:   Hex(e.XattrsIn),
		XattrsKey:  Hex(e.XattrsKey),
	}
	j.Name, j.NameHex = wtText(string(e.Name))
	if len(e.LinkTarget) > 0 {
		j.LinkTarget, j.LinkTargetHex = wtText(string(e.LinkTarget))
	}
	for _, r := range e.Rdev {
		j.Rdev = append(j.Rdev, U64(r))
	}
	return j
}

// wtChangeJSON is a worktree.Change: kind is Kind.String(); old and new are null when nil.
type wtChangeJSON struct {
	Path    *string      `json:"path,omitempty"`
	PathHex *string      `json:"path_hex,omitempty"`
	Kind    string       `json:"kind"`
	Old     *wtEntryJSON `json:"old"`
	New     *wtEntryJSON `json:"new"`
}

func wtChange(c worktree.Change) wtChangeJSON {
	j := wtChangeJSON{Kind: c.Kind.String(), Old: wtEntry(c.Old), New: wtEntry(c.New)}
	j.Path, j.PathHex = wtText(c.Path)
	return j
}

// wtChanges converts a change list; a nil list stays null.
func wtChanges(cs []worktree.Change) []wtChangeJSON {
	if cs == nil {
		return nil
	}
	out := make([]wtChangeJSON, 0, len(cs))
	for _, c := range cs {
		out = append(out, wtChange(c))
	}
	return out
}

// wtPathKind is a change reduced to its path and kind.
type wtPathKind struct {
	Path    *string `json:"path,omitempty"`
	PathHex *string `json:"path_hex,omitempty"`
	Kind    string  `json:"kind"`
}

// ---- self-check: verbatim copies and format literals of cmd/dstore ----

// wtCopies maps each verbatim copy in this file to the cmd/dstore/wc.go function it copies.
var wtCopies = []struct{ mine, theirs string }{
	{"wtDescribeChange", "describeChange"},
	{"wtResolveTicket", "resolveTicket"},
	{"wtFilterPaths", "filterPaths"},
	{"wtFetchedDesc", "fetchedDesc"},
	{"wtPushedKey", "pushedKey"},
}

// wtWcLiterals are the string literals of cmd/dstore/wc.go the vectors use; wtFlowLiterals those of
// worktree/flow.go the error vectors reproduce with fmt.Errorf.
var (
	wtWcLiterals = []string{
		"cloned %s into %s: %s, %d objects fetched (%d bytes)\n",
		"initialised working copy of %s; the reference exists (%s): status shows everything as new, pull merges\n",
		"initialised working copy of %s; the reference does not exist yet: push creates it\n",
		"%s does not exist on the cluster\n",
		"%s: up to date (%s)\n",
		"fetched %s: %s, %d objects fetched (%d bytes)\n",
		"commit %s, root %s",
		"root ",
		"conflicts:",
		"  %s (local: %s, cluster: %s)\n",
		"already up to date",
		"pulled: %d paths updated",
		", %d conflicts taken from the cluster",
		"nothing to push",
		"%s already holds %s (an earlier push completed); state updated\n",
		"pushed %s: commit %s, root %s, %d objects, %d uploaded, version %x\n",
		"pushed %s: root %s, %d objects, %d uploaded, version %x\n",
		"reference %s, synced to %s\n",
		"remote: up to date",
		"remote: the reference does not exist on the cluster",
		"remote: moved since your last fetch (+%d ~%d -%d; run pull)\n",
		"changes:",
		"  %-9s %s\n",
		"%d paths differ only in mtime, ownership or xattrs\n",
		"clone NAME [DIR]",
		"init NAME",
		"user: %w",
		"--remote and --incoming exclude each other",
	}
	wtFlowLiterals = []string{"%w (%v)", "%w: %s"}
)

func wtSelfCheck() error {
	dir, err := wtModuleDir("github.com/amber-store/dstore")
	if err != nil {
		return err
	}
	wcPath := filepath.Join(dir, "cmd", "dstore", "wc.go")
	wcSrc, err := os.ReadFile(wcPath)
	if err != nil {
		return err
	}
	flowSrc, err := os.ReadFile(filepath.Join(dir, "worktree", "flow.go"))
	if err != nil {
		return err
	}
	theirSet := token.NewFileSet()
	theirs, err := parser.ParseFile(theirSet, wcPath, wcSrc, 0)
	if err != nil {
		return err
	}
	mySet := token.NewFileSet()
	mine, err := parser.ParseFile(mySet, "family_worktree.go", wtSource, 0)
	if err != nil {
		return err
	}
	for _, c := range wtCopies {
		a, err := wtPrintFunc(mySet, mine, c.mine, c.theirs)
		if err != nil {
			return err
		}
		b, err := wtPrintFunc(theirSet, theirs, c.theirs, c.theirs)
		if err != nil {
			return fmt.Errorf("%s: %w", wcPath, err)
		}
		if a != b {
			return fmt.Errorf("%s is not a verbatim copy of cmd/dstore %s:\n--- copy\n%s\n--- dstore v0.1.10\n%s", c.mine, c.theirs, a, b)
		}
	}
	if err := wtCheckLiterals(wcPath, wcSrc, wtWcLiterals); err != nil {
		return err
	}
	return wtCheckLiterals("worktree/flow.go", flowSrc, wtFlowLiterals)
}

// wtPrintFunc prints the function name of f with go/printer, renamed to as and without its doc comment.
func wtPrintFunc(fset *token.FileSet, f *ast.File, name, as string) (string, error) {
	for _, d := range f.Decls {
		fd, ok := d.(*ast.FuncDecl)
		if !ok || fd.Recv != nil || fd.Name.Name != name {
			continue
		}
		cp := *fd
		cp.Doc = nil
		cp.Name = ast.NewIdent(as)
		var buf bytes.Buffer
		if err := printer.Fprint(&buf, fset, &cp); err != nil {
			return "", err
		}
		return buf.String(), nil
	}
	return "", fmt.Errorf("function %s not found", name)
}

// wtCheckLiterals requires every want string to be a string literal of the Go source src.
func wtCheckLiterals(name string, src []byte, want []string) error {
	fset := token.NewFileSet()
	f, err := parser.ParseFile(fset, name, src, 0)
	if err != nil {
		return err
	}
	have := map[string]bool{}
	ast.Inspect(f, func(n ast.Node) bool {
		if lit, ok := n.(*ast.BasicLit); ok && lit.Kind == token.STRING {
			if s, err := strconv.Unquote(lit.Value); err == nil {
				have[s] = true
			}
		}
		return true
	})
	for _, w := range want {
		if !have[w] {
			return fmt.Errorf("%s has no string literal %q", name, w)
		}
	}
	return nil
}

// wtModuleDir finds the module cache directory of the module path this binary was built with.
func wtModuleDir(path string) (string, error) {
	bi, ok := debug.ReadBuildInfo()
	if ok {
		for _, m := range bi.Deps {
			if m.Path != path {
				continue
			}
			if m.Replace != nil {
				m = m.Replace
			}
			out, err := exec.Command("go", "env", "GOMODCACHE").Output()
			if err != nil {
				break
			}
			if strings.ToLower(m.Path) != m.Path {
				break // module cache paths escape upper case; fall back to go list
			}
			dir := filepath.Join(strings.TrimSpace(string(out)), filepath.FromSlash(m.Path)+"@"+m.Version)
			if fi, err := os.Stat(dir); err == nil && fi.IsDir() {
				return dir, nil
			}
		}
	}
	out, err := exec.Command("go", "list", "-m", "-f", "{{.Dir}}", path).Output()
	if err != nil {
		return "", fmt.Errorf("locating module %s: %w", path, err)
	}
	dir := strings.TrimSpace(string(out))
	if dir == "" {
		return "", fmt.Errorf("module %s has no directory", path)
	}
	return dir, nil
}

// ---- verbatim copies of cmd/dstore/wc.go (checked by wtSelfCheck; do not edit) ----

func wtDescribeChange(ch worktree.Change) string {
	p := ch.Path
	if worktree.IsDir(ch.New) || (ch.New == nil && worktree.IsDir(ch.Old)) {
		p += "/"
	}
	switch ch.Kind {
	case worktree.TypeChanged:
		return fmt.Sprintf("%s (%s → %s)", p, worktree.TypeName(ch.Old.Mode), worktree.TypeName(ch.New.Mode))
	case worktree.ModeChanged:
		return fmt.Sprintf("%s (%04o → %04o)", p, ch.Old.Mode&0o7777, ch.New.Mode&0o7777)
	}
	return p
}

func wtFetchedDesc(fr worktree.FetchResult) string {
	if fr.Key.Type() == key.Commit {
		return fmt.Sprintf("commit %s, root %s", fr.Key.String()[:16], fr.Tree.String()[:16])
	}
	return "root " + fr.Key.String()[:16]
}

func wtPushedKey(r worktree.PushResult) key.Key {
	if r.Commit.Type() == key.Commit {
		return r.Commit
	}
	return r.Root
}

func wtResolveTicket(flag, stored, env string) (string, error) {
	for _, s := range []string{flag, stored, env} {
		if s != "" {
			return s, nil
		}
	}
	return "", errors.New("no cluster: set --ticket or $DSTORE_TICKET")
}

func wtFilterPaths(root string, changes []worktree.Change, args []string) ([]worktree.Change, error) {
	var prefixes []string
	for _, a := range args {
		abs, err := filepath.Abs(a)
		if err != nil {
			return nil, err
		}
		rel, err := filepath.Rel(root, abs)
		if err != nil || rel == ".." || strings.HasPrefix(rel, "../") {
			return nil, fmt.Errorf("%s is outside the working copy", a)
		}
		prefixes = append(prefixes, filepath.ToSlash(rel))
	}
	var out []worktree.Change
	for _, ch := range changes {
		for _, p := range prefixes {
			if p == "." || ch.Path == p || strings.HasPrefix(ch.Path, p+"/") {
				out = append(out, ch)
				break
			}
		}
	}
	return out, nil
}

// ---- worktree/config.json ----

type wtConfigJSON struct {
	Ticket      *string `json:"ticket,omitempty"`
	TicketHex   *string `json:"ticket_hex,omitempty"`
	Name        *string `json:"name,omitempty"`
	NameHex     *string `json:"name_hex,omitempty"`
	Relay       *string `json:"relay,omitempty"`
	RelayHex    *string `json:"relay_hex,omitempty"`
	NoRelay     bool    `json:"no_relay"`
	NoDiscovery bool    `json:"no_discovery"`
	User        *string `json:"user,omitempty"`
	UserHex     *string `json:"user_hex,omitempty"`
}

func wtConfig(c worktree.Config) wtConfigJSON {
	j := wtConfigJSON{NoRelay: c.NoRelay, NoDiscovery: c.NoDiscovery}
	j.Ticket, j.TicketHex = wtText(c.Ticket)
	j.Name, j.NameHex = wtText(c.Name)
	j.Relay, j.RelayHex = wtText(c.Relay)
	j.User, j.UserHex = wtText(c.User)
	return j
}

type wtConfigEncode struct {
	Name   string       `json:"name"`
	Config wtConfigJSON `json:"config"`
	File   string       `json:"file"`
}

type wtConfigDecode struct {
	Name    string       `json:"name"`
	File    *string      `json:"file,omitempty"`
	FileHex *string      `json:"file_hex,omitempty"`
	OK      bool         `json:"ok"`
	Config  wtConfigJSON `json:"config"`
	Error   *string      `json:"error,omitempty"`
}

type wtConfigFile struct {
	Encode []wtConfigEncode `json:"encode"`
	Decode []wtConfigDecode `json:"decode"`
}

func wtConfigVectors() (any, error) {
	var v wtConfigFile
	encode := []struct {
		name string
		cfg  worktree.Config
	}{
		{"minimal", worktree.Config{Ticket: "t", Name: "n"}},
		{"zero", worktree.Config{}},
		{"probe", worktree.Config{Ticket: "dstore1abc", Name: "trees/demo", NoRelay: true, User: "me"}},
		{"all-fields", worktree.Config{Ticket: "dstore1abc", Name: "trees/demo", Relay: "https://relay.example/", NoRelay: true, NoDiscovery: true, User: "alice"}},
		{"no-discovery-only", worktree.Config{Ticket: "t", Name: "n", NoDiscovery: true}},
		{"relay-only", worktree.Config{Ticket: "t", Name: "n", Relay: "https://r.example:8443/x?y=1&z=2"}},
		{"html-and-controls", worktree.Config{Ticket: "dstore1abc", Name: "trees/<a&b> x\x01\"\\", NoRelay: true, User: "Dr <d@x>"}},
		{"relay-escapes", worktree.Config{Ticket: "t", Name: "n", Relay: "&<>\b\f\n\r\t\x01\x1f\x7f\u2028\u2029"}},
		{"all-control-bytes", worktree.Config{Ticket: "t", Name: "n", User: "\x00\x01\x02\x03\x04\x05\x06\x07\x08\x09\x0a\x0b\x0c\x0d\x0e\x0f\x10\x11\x12\x13\x14\x15\x16\x17\x18\x19\x1a\x1b\x1c\x1d\x1e\x1f"}},
		{"invalid-utf8-ff", worktree.Config{Ticket: "t", Name: "n", Relay: "a\xffb"}},
		{"invalid-utf8-truncated", worktree.Config{Ticket: "t", Name: "n", User: "x\xe2\x82"}},
		{"invalid-utf8-surrogate", worktree.Config{Ticket: "t", Name: "n", Relay: "\xed\xa0\x80"}},
		{"invalid-utf8-overlong", worktree.Config{Ticket: "t", Name: "n", User: "\xc0\xaf"}},
		{"user-quote-backslash", worktree.Config{Ticket: "t", Name: "n", User: `a"b\c`}},
		{"unicode", worktree.Config{Ticket: "t", Name: "trees/é/😀", User: "Zoë \u00a0\ufeff"}},
	}
	for _, c := range encode {
		file, err := wtWriteConfig(c.cfg)
		if err != nil {
			return nil, fmt.Errorf("encode %s: %w", c.name, err)
		}
		if !utf8.Valid(file) {
			return nil, fmt.Errorf("encode %s: config file is not valid UTF-8", c.name)
		}
		v.Encode = append(v.Encode, wtConfigEncode{Name: c.name, Config: wtConfig(c.cfg), File: string(file)})
	}
	decode := []struct{ name, file string }{
		{"probe", "{\n  \"ticket\": \"dstore1abc\",\n  \"name\": \"trees/demo\",\n  \"no_relay\": true,\n  \"user\": \"me\"\n}\n"},
		{"all-fields-compact", `{"ticket":"t","name":"n","relay":"r","no_relay":true,"no_discovery":true,"user":"u"}`},
		{"case-insensitive-keys", `{"Ticket":"t","NAME":"n","Relay":"r","NO_RELAY":true,"No_Discovery":true,"USER":"u"}`},
		{"fold-kelvin-and-long-s", "{\"tic\u212aet\":\"t\",\"u\u017fer\":\"u\",\"no_di\u017fcovery\":true}"},
		{"exact-then-folded-last-wins", `{"name":"a","NAME":"b"}`},
		{"folded-then-exact-last-wins", `{"NAME":"b","name":"a"}`},
		{"duplicate-keys-last-wins", `{"name":"a","name":"b","no_relay":true,"no_relay":false}`},
		{"unknown-keys", `{"ticket":"t","name":"n","extra":1,"nested":{"a":[1,{"b":null}]}}`},
		{"null-values", `{"ticket":null,"name":"n","no_relay":null,"user":null}`},
		{"null-document", `null`},
		{"empty-object", `{}`},
		{"whitespace", " \t\r\n{ \"name\" : \"n\" ,\n\"user\":\"u\"} \n"},
		{"escapes", `{"name":"\u003ca\u0026b\u003e \u00e9 \ud83d\ude00 \/ \b\f\n\r\t \"\\"}`},
		{"key-escape", `{"\u0074icket":"t"}`},
		{"lone-surrogate-escape", `{"name":"\ud800x","user":"\udc00"}`},
		{"invalid-utf8", "{\"name\":\"a\xffb\",\"user\":\"\xe2\x82\",\"relay\":\"\xed\xa0\x80\"}"},
		{"raw-del-and-u2028", "{\"name\":\"a\x7fb\u2028\"}"},
		{"number-into-string", `{"ticket":5,"name":"n"}`},
		{"string-into-bool", `{"no_relay":"true","name":"n"}`},
		{"number-into-bool", `{"no_relay":1}`},
		{"object-into-string", `{"ticket":{"a":1},"name":"n"}`},
		{"array-into-string", `{"user":["u"],"name":"n"}`},
		{"two-type-errors-first-wins", `{"ticket":1,"user":true,"name":"n"}`},
		{"big-number-into-string", `{"ticket":1e999}`},
		{"array-document", `[]`},
		{"string-document", `"x"`},
		{"number-document", `5`},
		{"bool-document", `true`},
		{"empty-input", ``},
		{"only-whitespace", " \n"},
		{"truncated-value", `{"ticket":`},
		{"truncated-string", `{"ticket":"t`},
		{"trailing-garbage", `{"ticket":"t"} x`},
		{"trailing-object", `{"ticket":"t"}{}`},
		{"not-json", `ticket = t`},
		{"bom", "\xef\xbb\xbf{\"name\":\"n\"}"},
		{"control-byte-in-string", "{\"name\":\"a\x01\"}"},
		{"bad-escape", `{"name":"\q"}`},
		{"short-unicode-escape", `{"name":"\u12"}`},
		{"single-quotes", `{'name':'n'}`},
		{"trailing-comma", `{"name":"n",}`},
		{"uppercase-true", `{"no_relay":TRUE}`},
	}
	for _, c := range decode {
		var cfg worktree.Config
		err := json.Unmarshal([]byte(c.file), &cfg)
		d := wtConfigDecode{Name: c.name, OK: err == nil, Config: wtConfig(cfg), Error: wtErrText(err, "")}
		d.File, d.FileHex = wtText(c.file)
		v.Decode = append(v.Decode, d)
	}
	return v, nil
}

// wtWriteConfig creates a working copy with cfg in a temporary directory and returns .dstore/config as
// worktree.Create wrote it (checking that SaveConfig writes the same bytes and leaves no .tmp behind).
func wtWriteConfig(cfg worktree.Config) ([]byte, error) {
	root, done, err := wtTemp()
	if err != nil {
		return nil, err
	}
	defer done()
	tr, err := worktree.Create(root, cfg)
	if err != nil {
		return nil, err
	}
	defer tr.Close()
	path := filepath.Join(root, worktree.Dir, "config")
	b, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}
	if err := tr.SaveConfig(); err != nil {
		return nil, err
	}
	again, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}
	if !bytes.Equal(b, again) {
		return nil, errors.New("SaveConfig wrote other bytes than Create")
	}
	if _, err := os.Lstat(path + ".tmp"); !errors.Is(err, fs.ErrNotExist) {
		return nil, fmt.Errorf("config.tmp left behind: %v", err)
	}
	return b, nil
}

// wtTestCommit is the commit dstore's own tests use: tree by "tester" at 1 ns, no parents, no message.
func wtTestCommit(tree key.Key) (key.Key, []byte, error) {
	id := commit.Identity{Name: "tester", When: 1}
	return commit.Commit{Tree: tree, Author: id, Committer: id}.Object()
}

// ---- worktree/state.json ----

type wtStateJSON struct {
	Base               Hex  `json:"base"`
	Remote             Hex  `json:"remote"`
	RemoteCommit       Hex  `json:"remote_commit"`
	HasRemote          bool `json:"has_remote"`
	IsBranch           bool `json:"is_branch"`
	RemoteKey          Hex  `json:"remote_key"`
	RemoteVersion      Hex  `json:"remote_version"`
	SyncedAtUnix       I64  `json:"synced_at_unix"`
	SyncedAtNsec       int  `json:"synced_at_nsec"`
	SyncedAtOffsetSecs int  `json:"synced_at_offset_secs"`
}

func wtState(s worktree.State) *wtStateJSON {
	_, off := s.SyncedAt.Zone()
	return &wtStateJSON{
		Base:               Hex(s.Base[:]),
		Remote:             Hex(s.Remote[:]),
		RemoteCommit:       Hex(s.RemoteCommit[:]),
		HasRemote:          s.HasRemote,
		IsBranch:           s.IsBranch(),
		RemoteKey:          wtRemoteKey(s),
		RemoteVersion:      Hex(s.RemoteVersion),
		SyncedAtUnix:       I64(s.SyncedAt.Unix()),
		SyncedAtNsec:       s.SyncedAt.Nanosecond(),
		SyncedAtOffsetSecs: off,
	}
}

func wtRemoteKey(s worktree.State) Hex {
	k := s.RemoteKey()
	return Hex(k[:])
}

type wtEmptyTreeJSON struct {
	Key   Hex    `json:"key"`
	Bytes Hex    `json:"bytes"`
	Short string `json:"short"`
}

type wtStateEncode struct {
	Name  string       `json:"name"`
	State *wtStateJSON `json:"state"`
	File  string       `json:"file"`
}

type wtStateDecode struct {
	Name    string       `json:"name"`
	Setup   string       `json:"setup"`
	File    *string      `json:"file,omitempty"`
	FileHex *string      `json:"file_hex,omitempty"`
	OK      bool         `json:"ok"`
	State   *wtStateJSON `json:"state"`
	Error   *string      `json:"error,omitempty"`
}

type wtStateFile struct {
	EmptyTree wtEmptyTreeJSON `json:"empty_tree"`
	Commit    wtEmptyTreeJSON `json:"commit"`
	Encode    []wtStateEncode `json:"encode"`
	Decode    []wtStateDecode `json:"decode"`
}

func wtStateVectors() (any, error) {
	empty, emptyBytes := worktree.EmptyTree()
	v := wtStateFile{EmptyTree: wtEmptyTreeJSON{Key: Hex(empty[:]), Bytes: Hex(emptyBytes), Short: empty.String()[:16]}}
	k1 := wtBlobKey(smData(1, 100))
	k2 := wtBlobKey(smData(2, 200))
	ck, ckBytes, err := wtTestCommit(empty)
	if err != nil {
		return nil, err
	}
	v.Commit = wtEmptyTreeJSON{Key: Hex(ck[:]), Bytes: Hex(ckBytes), Short: ck.String()[:16]}
	at := func(s, ns int64) time.Time { return time.Unix(s, ns).UTC() }
	encode := []struct {
		name string
		s    worktree.State
	}{
		{"no-remote", worktree.State{Base: empty, SyncedAt: at(wtSynced, 0)}},
		{"no-remote-ignores-remote-fields", worktree.State{Base: empty, Remote: k1, RemoteVersion: []byte{1, 2, 3}, SyncedAt: at(wtSynced, 0)}},
		{"probe", worktree.State{Base: empty, Remote: empty, HasRemote: true, RemoteVersion: []byte{1, 2, 3}, SyncedAt: at(wtSynced, 5)}},
		{"remote-empty-version", worktree.State{Base: empty, Remote: k1, HasRemote: true, RemoteVersion: []byte{}, SyncedAt: at(wtSynced, 0)}},
		{"remote-nil-version", worktree.State{Base: empty, Remote: k1, HasRemote: true, SyncedAt: at(wtSynced, 0)}},
		{"base-and-remote-differ", worktree.State{Base: k1, Remote: k2, HasRemote: true, RemoteVersion: []byte{0, 0, 0, 1, 0xfe, 0xdc, 0xba, 0x98}, SyncedAt: at(wtSynced, 123456789)}},
		{"branch", worktree.State{Base: empty, Remote: empty, RemoteCommit: ck, HasRemote: true, RemoteVersion: []byte{1, 2, 3}, SyncedAt: at(wtSynced, 5)}},
		{"branch-base-and-remote-differ", worktree.State{Base: k1, Remote: k2, RemoteCommit: ck, HasRemote: true, RemoteVersion: []byte{}, SyncedAt: at(wtSynced, 0)}},
		{"remote-commit-not-a-commit-omitted", worktree.State{Base: empty, Remote: empty, RemoteCommit: k1, HasRemote: true, RemoteVersion: []byte{1}, SyncedAt: at(wtSynced, 0)}},
		{"remote-commit-is-a-tree-omitted", worktree.State{Base: empty, Remote: empty, RemoteCommit: empty, HasRemote: true, RemoteVersion: []byte{1}, SyncedAt: at(wtSynced, 0)}},
		{"no-remote-ignores-remote-commit", worktree.State{Base: empty, Remote: empty, RemoteCommit: ck, SyncedAt: at(wtSynced, 0)}},
		{"ns-100", worktree.State{Base: empty, SyncedAt: at(wtSynced, 100)}},
		{"ns-120000000", worktree.State{Base: empty, SyncedAt: at(wtSynced, 120000000)}},
		{"ns-123456789", worktree.State{Base: empty, SyncedAt: at(wtSynced, 123456789)}},
		{"ns-999999999", worktree.State{Base: empty, SyncedAt: at(wtSynced, 999999999)}},
		{"pre-1970", worktree.State{Base: empty, SyncedAt: at(-1, 999999999)}},
		{"pre-1970-whole-day", worktree.State{Base: empty, SyncedAt: at(-86400, 0)}},
		{"year-0", worktree.State{Base: empty, SyncedAt: at(-62167219200, 0)}},
		{"year-10000", worktree.State{Base: empty, SyncedAt: at(253402300800, 0)}},
		{"fixed-zone-instant", worktree.State{Base: empty, SyncedAt: time.Unix(wtSynced, 5).In(time.FixedZone("IST", 19800))}},
	}
	for _, c := range encode {
		file, err := wtWriteState(c.s)
		if err != nil {
			return nil, fmt.Errorf("encode %s: %w", c.name, err)
		}
		v.Encode = append(v.Encode, wtStateEncode{Name: c.name, State: wtState(c.s), File: string(file)})
	}
	e, e1, e2 := empty.String(), k1.String(), k2.String()
	syncedAt := "2023-11-14T22:13:20Z"
	st := func(base, remote, version, synced string) string {
		return fmt.Sprintf(`{"base":%q,"remote":%q,"remote_version":%q,"synced_at":%q}`, base, remote, version, synced)
	}
	stc := func(base, remote, rc, version, synced string) string {
		return fmt.Sprintf(`{"base":%q,"remote":%q,"remote_commit":%q,"remote_version":%q,"synced_at":%q}`, base, remote, rc, version, synced)
	}
	c := ck.String()
	var reservedBit, reservedType, nonCanonical [32]byte
	copy(reservedBit[:], k1[:])
	reservedBit[0] |= 0x08
	copy(reservedType[:], k1[:])
	reservedType[0] = 0x60 | reservedType[0]&0x0f // 5 is Commit since core v0.0.9; 6..15 are reserved
	nonCanonical[0] = 0x01                        // Blob, 2-byte length field 00 00
	decode := []struct{ name, setup, file string }{
		{"probe", "file", "{\n  \"base\": \"" + e + "\",\n  \"remote\": \"" + e + "\",\n  \"remote_version\": \"010203\",\n  \"synced_at\": \"2023-11-14T22:13:20.000000005Z\"\n}\n"},
		{"no-remote", "file", st(e, "", "", syncedAt)},
		{"no-remote-version-ignored", "file", st(e, "", "zz1", syncedAt)},
		{"remote-empty-version", "file", st(e, e1, "", syncedAt)},
		{"remote-version-key-missing", "file", fmt.Sprintf(`{"base":%q,"remote":%q,"synced_at":%q}`, e, e1, syncedAt)},
		{"base-and-remote-differ", "file", st(e1, e2, "00000001fedcba98", "2023-11-14T22:13:20.123456789Z")},
		{"uppercase-hex", "file", st(strings.ToUpper(e1), strings.ToUpper(e2), "ABCDEF", syncedAt)},
		{"offset-plus-0200", "file", st(e, "", "", "2023-11-15T00:13:20.5+02:00")},
		{"offset-minus-0330", "file", st(e, "", "", "2023-11-14T18:43:20-03:30")},
		{"offset-minus-0000", "file", st(e, "", "", "2023-11-14T22:13:20-00:00")},
		{"lowercase-t-and-z", "file", st(e, "", "", "2023-11-14t22:13:20z")},
		{"fraction-1-digit", "file", st(e, "", "", "2023-11-14T22:13:20.1Z")},
		{"fraction-12-digits", "file", st(e, "", "", "2023-11-14T22:13:20.123456789123Z")},
		{"comma-fraction", "file", st(e, "", "", "2023-11-14T22:13:20,5Z")},
		{"pre-1970", "file", st(e, "", "", "1969-12-31T23:59:59.999999999Z")},
		{"case-insensitive-keys", "file", fmt.Sprintf(`{"BASE":%q,"Remote":%q,"REMOTE_VERSION":"01","Synced_At":%q}`, e, e1, syncedAt)},
		{"duplicate-keys-last-wins", "file", fmt.Sprintf(`{"base":"zz","base":%q,"synced_at":"bad","synced_at":%q}`, e, syncedAt)},
		{"unknown-keys", "file", fmt.Sprintf(`{"base":%q,"synced_at":%q,"extra":[1,2,{"x":null}]}`, e, syncedAt)},
		{"null-values", "file", fmt.Sprintf(`{"base":%q,"remote":null,"remote_version":null,"synced_at":%q}`, e, syncedAt)},
		{"missing", "missing", ""},
		{"directory", "directory", ""},
		{"empty-file", "file", ""},
		{"not-json", "file", "base = " + e},
		{"array-document", "file", `[]`},
		{"number-base", "file", fmt.Sprintf(`{"base":5,"synced_at":%q}`, syncedAt)},
		{"truncated", "file", `{"base":"`},
		{"base-empty", "file", st("", "", "", syncedAt)},
		{"base-missing", "file", fmt.Sprintf(`{"synced_at":%q}`, syncedAt)},
		{"base-invalid-hex", "file", st("zz"+e[2:], "", "", syncedAt)},
		{"base-non-ascii-hex", "file", st("é"+e[4:], "", "", syncedAt)},
		{"base-odd-length", "file", st(e[1:], "", "", syncedAt)},
		{"base-invalid-byte-before-odd-length", "file", st("zzz", "", "", syncedAt)},
		{"base-31-bytes", "file", st(e[2:], "", "", syncedAt)},
		{"base-33-bytes", "file", st(e+"00", "", "", syncedAt)},
		{"base-reserved-bit", "file", st(hex.EncodeToString(reservedBit[:]), "", "", syncedAt)},
		{"base-reserved-type", "file", st(hex.EncodeToString(reservedType[:]), "", "", syncedAt)},
		{"base-non-canonical-length", "file", st(hex.EncodeToString(nonCanonical[:]), "", "", syncedAt)},
		{"base-is-a-commit", "file", st(c, "", "", syncedAt)},
		{"remote-is-a-commit", "file", st(e, c, "01", syncedAt)},
		{"branch", "file", stc(e, e, c, "010203", syncedAt)},
		{"branch-indented", "file", "{\n  \"base\": \"" + e + "\",\n  \"remote\": \"" + e + "\",\n  \"remote_commit\": \"" + c + "\",\n  \"remote_version\": \"010203\",\n  \"synced_at\": \"2023-11-14T22:13:20.000000005Z\"\n}\n"},
		{"branch-uppercase-hex", "file", stc(e, e2, strings.ToUpper(c), "01", syncedAt)},
		{"branch-case-insensitive-key", "file", fmt.Sprintf(`{"base":%q,"remote":%q,"Remote_COMMIT":%q,"remote_version":"01","synced_at":%q}`, e, e, c, syncedAt)},
		{"remote-commit-empty", "file", stc(e, e, "", "01", syncedAt)},
		{"remote-commit-null", "file", fmt.Sprintf(`{"base":%q,"remote":%q,"remote_commit":null,"remote_version":"01","synced_at":%q}`, e, e, syncedAt)},
		{"remote-commit-without-remote-ignored", "file", stc(e, "", "zz", "", syncedAt)},
		{"remote-commit-invalid-hex", "file", stc(e, e, "zz"+c[2:], "01", syncedAt)},
		{"remote-commit-short", "file", stc(e, e, "50", "01", syncedAt)},
		{"remote-commit-is-a-tree", "file", stc(e, e, e, "01", syncedAt)},
		{"remote-commit-is-a-blob", "file", stc(e, e, e1, "01", syncedAt)},
		{"remote-commit-reserved-type", "file", stc(e, e, hex.EncodeToString(reservedType[:]), "01", syncedAt)},
		{"remote-commit-number", "file", fmt.Sprintf(`{"base":%q,"remote":%q,"remote_commit":5,"synced_at":%q}`, e, e, syncedAt)},
		{"errors-in-order-remote-before-commit", "file", stc(e, "yy", "zz", "xx", "")},
		{"errors-in-order-commit-before-version", "file", stc(e, e, "zz", "xx", "")},
		{"errors-in-order-commit-type-before-version", "file", stc(e, e, e, "xx", "")},
		{"remote-invalid-hex", "file", st(e, "xyz", "", syncedAt)},
		{"remote-short", "file", st(e, "00", "", syncedAt)},
		{"remote-version-odd-length", "file", st(e, e1, "123", syncedAt)},
		{"remote-version-invalid-hex", "file", st(e, e1, "0g", syncedAt)},
		{"synced-at-empty", "file", st(e, "", "", "")},
		{"synced-at-missing", "file", fmt.Sprintf(`{"base":%q}`, e)},
		{"synced-at-space", "file", st(e, "", "", "2023-11-14 22:13:20Z")},
		{"synced-at-no-zone", "file", st(e, "", "", "2023-11-14T22:13:20")},
		{"synced-at-hour-24", "file", st(e, "", "", "2023-11-14T24:00:00Z")},
		{"synced-at-february-30", "file", st(e, "", "", "2023-02-30T00:00:00Z")},
		{"synced-at-offset-hour-24", "file", st(e, "", "", "2023-11-14T22:13:20+24:00")},
		{"synced-at-year-10000", "file", st(e, "", "", "10000-01-01T00:00:00Z")},
		{"synced-at-number", "file", fmt.Sprintf(`{"base":%q,"synced_at":5}`, e)},
		{"errors-in-order-base-first", "file", st("zz", "yy", "xx", "")},
		{"errors-in-order-remote-before-version", "file", st(e, "yy", "xx", "")},
		{"errors-in-order-version-before-synced-at", "file", st(e, e1, "xx", "")},
	}
	for _, c := range decode {
		s, err, root := wtLoadState(c.setup, c.file)
		if root == "" && err != nil {
			return nil, fmt.Errorf("decode %s: %w", c.name, err)
		}
		d := wtStateDecode{Name: c.name, Setup: c.setup, OK: err == nil, Error: wtErrText(err, root)}
		if c.setup == "file" {
			d.File, d.FileHex = wtText(c.file)
		}
		if err == nil {
			d.State = wtState(s)
		}
		v.Decode = append(v.Decode, d)
	}
	return v, nil
}

// wtWriteState creates a working copy in a temporary directory, sets its state and returns .dstore/state
// as SaveState wrote it.
func wtWriteState(s worktree.State) ([]byte, error) {
	root, done, err := wtTemp()
	if err != nil {
		return nil, err
	}
	defer done()
	tr, err := worktree.Create(root, worktree.Config{Ticket: "t", Name: "n"})
	if err != nil {
		return nil, err
	}
	defer tr.Close()
	tr.State = s
	if err := tr.SaveState(); err != nil {
		return nil, err
	}
	path := filepath.Join(root, worktree.Dir, "state")
	if _, err := os.Lstat(path + ".tmp"); !errors.Is(err, fs.ErrNotExist) {
		return nil, fmt.Errorf("state.tmp left behind: %v", err)
	}
	return os.ReadFile(path)
}

// wtLoadState creates a working copy in a temporary directory, lays out .dstore/state per setup ("file":
// file holds the bytes; "missing"; "directory") and opens the copy with worktree.Open. A non-empty root
// means the error, if any, came from Open (with that root in its text).
func wtLoadState(setup, file string) (worktree.State, error, string) {
	root, done, err := wtTemp()
	if err != nil {
		return worktree.State{}, err, ""
	}
	defer done()
	tr, err := worktree.Create(root, worktree.Config{Ticket: "t", Name: "n"})
	if err != nil {
		return worktree.State{}, err, ""
	}
	if err := tr.Close(); err != nil {
		return worktree.State{}, err, ""
	}
	path := filepath.Join(root, worktree.Dir, "state")
	switch setup {
	case "file":
		err = os.WriteFile(path, []byte(file), 0o644)
	case "directory":
		err = os.Mkdir(path, 0o755)
	case "missing":
	default:
		err = fmt.Errorf("unknown setup %q", setup)
	}
	if err != nil {
		return worktree.State{}, err, ""
	}
	t, err := worktree.Open(root)
	if err != nil {
		return worktree.State{}, err, root
	}
	s := t.State
	if err := t.Close(); err != nil {
		return worktree.State{}, err, ""
	}
	return s, nil, root
}

// ---- worktree/trees: fixtures built in memory through the core builders, as ingest builds them ----

// wtNode describes one tree entry. Regular files carry content, directories children, symlinks a target,
// devices rdev.
type wtNode struct {
	name     string
	mode     uint64
	uid, gid uint64
	mtime    int64
	content  []byte
	target   string
	rdev     []uint64
	xattrs   map[string][]byte
	children []wtNode
	ck       []byte // non-nil: the entry's content key verbatim, nothing built
	omit     bool   // leave the entry's own content object (file root or directory root) out of objects.bin
}

func wtF(name, content string, perm uint64, mtime int64) wtNode {
	return wtNode{name: name, mode: unix.S_IFREG | perm, uid: 1000, gid: 1000, mtime: mtime, content: []byte(content)}
}

func wtFB(name string, content []byte, perm uint64, mtime int64) wtNode {
	return wtNode{name: name, mode: unix.S_IFREG | perm, uid: 1000, gid: 1000, mtime: mtime, content: content}
}

func wtD(name string, perm uint64, mtime int64, children ...wtNode) wtNode {
	return wtNode{name: name, mode: unix.S_IFDIR | perm, uid: 1000, gid: 1000, mtime: mtime, children: children}
}

func wtL(name, target string, mtime int64) wtNode {
	return wtNode{name: name, mode: unix.S_IFLNK | 0o777, uid: 1000, gid: 1000, mtime: mtime, target: target}
}

// wtSpecial is an entry of any other mode (fifo, socket, device, unknown type) without content.
func wtSpecial(name string, mode uint64, mtime int64, rdev ...uint64) wtNode {
	return wtNode{name: name, mode: mode, uid: 1000, gid: 1000, mtime: mtime, rdev: rdev}
}

func (n wtNode) withXattrs(m map[string][]byte) wtNode { n.xattrs = m; return n }
func (n wtNode) withOwner(uid, gid uint64) wtNode      { n.uid, n.gid = uid, gid; return n }
func (n wtNode) omitted() wtNode                       { n.omit = true; return n }
func (n wtNode) withContentKey(ck []byte) wtNode       { n.ck = ck; return n }

type wtTreeInfo struct {
	name, doc string
	root      key.Key
}

// wtStore holds every built object (deduplicated by key) and the named trees.
type wtStore struct {
	ic    chunkers.ItemChunker
	objs  map[key.Key][]byte
	omit  map[key.Key]bool
	trees []wtTreeInfo
}

func (s *wtStore) emit(o fstree.Object) error {
	if prev, ok := s.objs[o.Key]; ok {
		if !bytes.Equal(prev, o.Bytes) {
			return fmt.Errorf("key %s emitted with differing bytes", o.Key)
		}
		return nil
	}
	s.objs[o.Key] = append([]byte(nil), o.Bytes...)
	return nil
}

// file mirrors core ingest driver.buildFile over in-memory content.
func (s *wtStore) file(content []byte) (key.Key, error) {
	ib := fstree.NewFileIndexBuilder(s.ic)
	saw := false
	err := chunkers.SplitBytes(bytes.NewReader(content), nil, func(chunk []byte) error {
		saw = true
		obj, err := fstree.EncodeBlob(append([]byte(nil), chunk...))
		if err != nil {
			return err
		}
		if err := s.emit(obj); err != nil {
			return err
		}
		return ib.AddChild(s.emit, obj.Key, nil)
	})
	if err != nil {
		return key.Key{}, err
	}
	if !saw {
		obj, err := fstree.EncodeBlob([]byte{})
		if err != nil {
			return key.Key{}, err
		}
		if err := s.emit(obj); err != nil {
			return key.Key{}, err
		}
		if err := ib.AddChild(s.emit, obj.Key, nil); err != nil {
			return key.Key{}, err
		}
	}
	return ib.Finish(s.emit)
}

// entry mirrors core ingest driver.buildEntry.
func (s *wtStore) entry(n wtNode) (fstree.Entry, error) {
	e := fstree.Entry{Name: []byte(n.name), Mode: n.mode, UID: n.uid, GID: n.gid, Mtime: n.mtime}
	switch n.mode & unix.S_IFMT {
	case unix.S_IFREG, unix.S_IFDIR:
		if n.ck != nil {
			e.ContentKey = n.ck
			break
		}
		var k key.Key
		var err error
		if n.mode&unix.S_IFMT == unix.S_IFREG {
			k, err = s.file(n.content)
		} else {
			k, err = s.dir(n.children)
		}
		if err != nil {
			return e, err
		}
		if n.omit {
			s.omit[k] = true
		}
		e.ContentKey = k[:]
	case unix.S_IFLNK:
		e.LinkTarget = []byte(n.target)
	case unix.S_IFCHR, unix.S_IFBLK:
		e.Rdev = n.rdev
	}
	if n.mode&unix.S_IFMT != unix.S_IFLNK && len(n.xattrs) > 0 {
		enc := cborx.EncodeXattrs(n.xattrs)
		if len(enc) <= ingest.DefaultXattrInlineMax {
			e.XattrsIn = enc
		} else {
			obj, err := fstree.EncodeXattrSet(n.xattrs)
			if err != nil {
				return e, err
			}
			if err := s.emit(obj); err != nil {
				return e, err
			}
			e.XattrsKey = obj.Key[:]
		}
	}
	return e, nil
}

// dir mirrors core ingest driver.buildDir: entries sorted bytewise by name through a DirBuilder.
func (s *wtStore) dir(children []wtNode) (key.Key, error) {
	entries := make([]fstree.Entry, 0, len(children))
	for _, c := range children {
		e, err := s.entry(c)
		if err != nil {
			return key.Key{}, err
		}
		entries = append(entries, e)
	}
	sort.Slice(entries, func(i, j int) bool { return bytes.Compare(entries[i].Name, entries[j].Name) < 0 })
	for i := 1; i < len(entries); i++ {
		if bytes.Equal(entries[i-1].Name, entries[i].Name) {
			return key.Key{}, fmt.Errorf("duplicate entry %q", entries[i].Name)
		}
	}
	db := fstree.NewDirBuilder(s.ic)
	for _, e := range entries {
		if err := db.AddEntry(s.emit, e); err != nil {
			return key.Key{}, err
		}
	}
	return db.Finish(s.emit)
}

func (s *wtStore) tree(name, doc string, children ...wtNode) error {
	k, err := s.dir(children)
	if err != nil {
		return fmt.Errorf("tree %s: %w", name, err)
	}
	s.trees = append(s.trees, wtTreeInfo{name: name, doc: doc, root: k})
	return nil
}

func (s *wtStore) root(name string) key.Key {
	for _, t := range s.trees {
		if t.name == name {
			return t.root
		}
	}
	panic("vectorgen worktree: no tree " + name)
}

// getter reads built objects, counting calls; omitted and unknown keys give packstore.ErrNotFound, as
// Tree.Get does for an object the local packstore lacks.
func (s *wtStore) getter(calls *int) worktree.Getter {
	return func(k key.Key) ([]byte, error) {
		if calls != nil {
			*calls++
		}
		b, ok := s.objs[k]
		if !ok || s.omit[k] {
			return nil, packstore.ErrNotFound
		}
		return b, nil
	}
}

type wtObjectJSON struct {
	Key  Hex `json:"key"`
	Size int `json:"size"`
}

type wtTreeJSON struct {
	Name string `json:"name"`
	Root Hex    `json:"root"`
	Doc  string `json:"doc"`
}

type wtXattrPair struct {
	Name    *string `json:"name,omitempty"`
	NameHex *string `json:"name_hex,omitempty"`
	Value   Hex     `json:"value"`
}

type wtXattrJSON struct {
	Name        string        `json:"name"`
	Attrs       []wtXattrPair `json:"attrs"`
	Encoded     Hex           `json:"encoded"`
	Inline      bool          `json:"inline"`
	XattrSetKey Hex           `json:"xattr_set_key"`
}

type wtTreesFile struct {
	Objects []wtObjectJSON `json:"objects"`
	Omitted []Hex          `json:"omitted"`
	Trees   []wtTreeJSON   `json:"trees"`
	Xattrs  []wtXattrJSON  `json:"xattrs"`
}

// write writes objects.bin (key 32 ‖ BE u64 length ‖ bytes, ascending key order, omitted objects left out)
// and trees.json.
func (s *wtStore) write(dir string) error {
	var keys, omitted []key.Key
	for k := range s.objs {
		if s.omit[k] {
			omitted = append(omitted, k)
			continue
		}
		keys = append(keys, k)
	}
	less := func(ks []key.Key) func(i, j int) bool {
		return func(i, j int) bool { return bytes.Compare(ks[i][:], ks[j][:]) < 0 }
	}
	sort.Slice(keys, less(keys))
	sort.Slice(omitted, less(omitted))
	var bin bytes.Buffer
	f := wtTreesFile{Objects: []wtObjectJSON{}, Omitted: []Hex{}}
	for _, k := range keys {
		b := s.objs[k]
		if k.Type() == key.Blob && wtBlobKey(b) != k {
			return fmt.Errorf("blob %s does not hash to its key", k)
		}
		bin.Write(k[:])
		var n [8]byte
		binary.BigEndian.PutUint64(n[:], uint64(len(b)))
		bin.Write(n[:])
		bin.Write(b)
		f.Objects = append(f.Objects, wtObjectJSON{Key: Hex(k[:]), Size: len(b)})
	}
	for _, k := range omitted {
		f.Omitted = append(f.Omitted, Hex(k[:]))
	}
	for _, t := range s.trees {
		f.Trees = append(f.Trees, wtTreeJSON{Name: t.name, Root: Hex(t.root[:]), Doc: t.doc})
	}
	xs, err := wtXattrVectors()
	if err != nil {
		return err
	}
	f.Xattrs = xs
	if err := wtCheckUTF8(reflect.ValueOf(f), "worktree/trees/trees.json"); err != nil {
		return err
	}
	if err := writeFile(filepath.Join(dir, "objects.bin"), bin.Bytes()); err != nil {
		return err
	}
	return writeJSON(filepath.Join(dir, "trees.json"), f)
}

// wtXattrVectors are cborx.EncodeXattrs encodings around the 256-byte inline limit.
func wtXattrVectors() ([]wtXattrJSON, error) {
	cases := []struct {
		name  string
		attrs map[string][]byte
	}{
		{"user-wc", map[string][]byte{"user.wc": []byte("1")}},
		{"empty-value", map[string][]byte{"user.e": {}}},
		{"shorter-key-sorts-first", map[string][]byte{"user.aa": []byte("2"), "user.b": []byte("1")}},
		{"inline-256", map[string][]byte{"user.x": bytes.Repeat([]byte{'v'}, 246)}},
		{"spilled-257", map[string][]byte{"user.x": bytes.Repeat([]byte{'v'}, 247)}},
		{"spilled-400", map[string][]byte{"user.big": smData(10, 400)}},
		{"non-utf8-name", map[string][]byte{"user.\xff": []byte("x")}},
	}
	var out []wtXattrJSON
	for _, c := range cases {
		enc := cborx.EncodeXattrs(c.attrs)
		obj, err := fstree.EncodeXattrSet(c.attrs)
		if err != nil {
			return nil, err
		}
		names := make([]string, 0, len(c.attrs))
		for n := range c.attrs {
			names = append(names, n)
		}
		sort.Strings(names)
		j := wtXattrJSON{Name: c.name, Attrs: []wtXattrPair{}, Encoded: Hex(enc), Inline: len(enc) <= ingest.DefaultXattrInlineMax, XattrSetKey: Hex(obj.Key[:])}
		for _, n := range names {
			p := wtXattrPair{Value: Hex(c.attrs[n])}
			p.Name, p.NameHex = wtText(n)
			j.Attrs = append(j.Attrs, p)
		}
		out = append(out, j)
	}
	return out, nil
}

// wtPeriodic is n bytes of a repeated 64-byte text line.
func wtPeriodic(n int) []byte {
	const line = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ-\n"
	b := bytes.Repeat([]byte(line), n/len(line)+1)
	return b[:n]
}

// wtNulText is n bytes of 64-byte text lines with a NUL at offset nul; first replaces the start.
func wtNulText(n, nul int, first string) []byte {
	b := wtPeriodic(n)
	b[nul] = 0
	copy(b, first)
	return b
}

func wtBuildTrees() (*wtStore, error) {
	s := &wtStore{ic: chunkers.NewItemChunker(ingest.DefaultItemBits), objs: map[key.Key][]byte{}, omit: map[key.Key]bool{}}
	m := func(i int64) int64 { return wtMt(wtT0+i, 0) }
	type def struct {
		name, doc string
		nodes     []wtNode
	}
	var defs []def
	add := func(name, doc string, nodes ...wtNode) { defs = append(defs, def{name, doc, nodes}) }

	add("empty", "the empty directory (worktree.EmptyTree)")

	add("diff-a", "the diffFixtures A side of worktree/diff_test.go with fixed metadata",
		wtF("edit.txt", "one\ntwo\nthree\n", 0o644, m(1)),
		wtF("bin", "a\x00b", 0o644, m(2)),
		wtF("gone.txt", "bye\n", 0o644, m(3)),
		wtF("run.sh", "#!/bin/sh\n", 0o644, m(4)),
		wtD("sub", 0o755, m(5), wtF("x", "x\n", 0o644, m(6))),
		wtL("link", "t1", m(7)),
	)
	add("diff-b", "the diffFixtures B side: edit, binary edit, added, deleted, chmod, retargeted link, directory chmod",
		wtF("edit.txt", "one\n2\nthree\n", 0o644, m(11)),
		wtF("bin", "a\x00c", 0o644, m(12)),
		wtF("new.txt", "hi\n", 0o644, m(13)),
		wtF("run.sh", "#!/bin/sh\n", 0o755, m(4)),
		wtD("sub", 0o700, m(5), wtF("x", "x\n", 0o644, m(6))),
		wtL("link", "t2", m(17)),
	)
	add("every-a", "the A side of TestDiffTrees_EveryKind (worktree/change_test.go)",
		wtF("same.txt", "same", 0o644, m(1)),
		wtF("edit.txt", "one", 0o644, m(2)),
		wtF("gone.txt", "bye", 0o644, m(3)),
		wtF("mode.sh", "#!/bin/sh", 0o644, m(4)),
		wtD("sub", 0o755, m(5), wtF("deep.txt", "deep", 0o644, m(6))),
		wtD("olddir", 0o755, m(7), wtF("x", "x", 0o644, m(8))),
		wtL("link", "t1", m(9)),
		wtL("becomes-file", "was-link", m(10)),
		wtD("flip", 0o755, m(11), wtF("inner", "inner", 0o644, m(12))),
	)
	add("every-b", "the B side of TestDiffTrees_EveryKind, with mtime-only changes on same.txt and sub/deep.txt",
		wtF("same.txt", "same", 0o644, m(101)),
		wtF("edit.txt", "two", 0o644, m(2)),
		wtF("new.txt", "hi", 0o644, m(13)),
		wtF("mode.sh", "#!/bin/sh", 0o755, m(4)),
		wtD("sub", 0o755, m(5), wtF("deep.txt", "deep", 0o644, m(106))),
		wtD("newdir", 0o755, m(14), wtF("y", "y", 0o644, m(15))),
		wtF("becomes-file", "now a file", 0o644, m(10)),
		wtF("flip", "flat", 0o644, m(11)),
		wtL("link", "t2", m(9)),
	)
	add("prune-a", "TestDiffTrees_PrunesEqualSubtrees before the edit",
		wtD("sub", 0o755, m(1), wtF("x", "x", 0o644, m(2))),
		wtF("top", "1", 0o644, m(3)),
	)
	add("prune-b", "TestDiffTrees_PrunesEqualSubtrees after the edit: only top differs",
		wtD("sub", 0o755, m(1), wtF("x", "x", 0o644, m(2))),
		wtF("top", "2", 0o644, m(3)),
	)
	add("walk-a", "one file",
		wtF("a.txt", "a\n", 0o644, m(1)),
	)
	add("walk-b", "names ordering a, a/x, a-b, a.txt (walk order is not bytewise path order) and names needing care",
		wtD("a", 0o755, m(2), wtF("x", "x\n", 0o644, m(3)), wtD("y", 0o755, m(4), wtF("z", "z\n", 0o644, m(5)))),
		wtF("a-b", "ab\n", 0o644, m(6)),
		wtF("a.txt", "a\n", 0o644, m(1)),
		wtF("sp ace", "s\n", 0o644, m(7)),
		wtF("tab\tname", "t\n", 0o644, m(8)),
		wtF("caf\xc3\xa9", "cafe\n", 0o644, m(9)),
	)
	add("nonutf8-a", "invalid UTF-8 content",
		wtF("latin1", "caf\xe9\n", 0o644, m(1)),
	)
	add("nonutf8-b", "nonutf8-a with the content edited and a file with an invalid UTF-8 name added",
		wtF("latin1", "caf\xe8\n", 0o644, m(1)),
		wtF("bad\xffname", "ff\n", 0o644, m(2)),
	)
	x256 := map[string][]byte{"user.x": bytes.Repeat([]byte{'v'}, 246)}
	x257 := map[string][]byte{"user.x": bytes.Repeat([]byte{'v'}, 247)}
	add("special-a", "devices, fifo, socket, xattrs, unknown types, binary boundaries and type changes: the A side",
		wtSpecial("dev", unix.S_IFCHR|0o644, m(1), 1, 3),
		wtSpecial("blk", unix.S_IFBLK|0o600, m(2), 259, 0),
		wtSpecial("fifo", unix.S_IFIFO|0o644, m(3)),
		wtSpecial("sock", unix.S_IFSOCK|0o755, m(4)),
		wtF("xin", "xin\n", 0o644, m(5)).withXattrs(map[string][]byte{"user.wc": []byte("1")}),
		wtF("xsp", "xsp\n", 0o644, m(6)).withXattrs(map[string][]byte{"user.big": smData(10, 400)}),
		wtF("x256", "x256\n", 0o644, m(7)).withXattrs(x256),
		wtF("x257", "x257\n", 0o644, m(8)).withXattrs(x257),
		wtL("same-bytes", "payload", m(9)),
		wtF("modecontent", "one\n", 0o644, m(10)),
		wtD("dirmode", 0o755, m(11), wtF("f", "f\n", 0o644, m(12))),
		wtF("miscount", "keep\n--x\n", 0o644, m(13)),
		wtF("nonl", "a", 0o644, m(14)),
		wtSpecial("weird", 0o160644, m(15)),
		wtF("touch", "t\n", 0o644, m(16)),
		wtF("owner", "o\n", 0o644, m(17)),
		wtF("empty-old", "", 0o644, m(18)),
		wtF("crlf", "a\r\nb\r\n", 0o644, m(19)),
		wtFB("nul8191", wtNulText(8192, 8191, "A side\n"), 0o644, m(21)),
		wtFB("nul8192", wtNulText(8193, 8192, "A side\n"), 0o644, m(22)),
		wtF("f2d", "file\n", 0o644, m(23)),
		wtD("d2f", 0o755, m(24), wtF("inner", "inner\n", 0o644, m(25)), wtD("deeper", 0o755, m(26), wtF("z", "z\n", 0o644, m(27)))),
		wtD("olddir", 0o755, m(28), wtF("x", "x\n", 0o644, m(29)), wtD("deep", 0o755, m(30), wtF("y", "y\n", 0o644, m(31)))),
		wtF("setuid", "s\n", 0o4755, m(32)),
	)
	add("special-b", "the B side of special-a",
		wtSpecial("dev", unix.S_IFCHR|0o644, m(1), 1, 5),
		wtSpecial("blk", unix.S_IFBLK|0o600, m(2), 259, 0),
		wtL("fifo", "somewhere", m(3)),
		wtSpecial("fifo2", unix.S_IFIFO|0o600, m(41)),
		wtSpecial("sock", unix.S_IFSOCK|0o755, m(4)),
		wtF("xin", "xin\n", 0o644, m(5)).withXattrs(map[string][]byte{"user.wc": []byte("2")}),
		wtF("xsp", "xsp\n", 0o644, m(6)).withXattrs(map[string][]byte{"user.big": smData(11, 400)}),
		wtF("x256", "x256\n", 0o644, m(7)).withXattrs(x256),
		wtF("x257", "x257\n", 0o644, m(8)).withXattrs(x257),
		wtF("same-bytes", "payload", 0o644, m(9)),
		wtF("modecontent", "two\n", 0o755, m(10)),
		wtD("dirmode", 0o700, m(11), wtF("f", "f\n", 0o644, m(12))),
		wtF("miscount", "keep\n++y\n", 0o644, m(13)),
		wtF("nonl", "a\nb", 0o644, m(14)),
		wtSpecial("weird", 0o160755, m(15)),
		wtSpecial("zero", 0, m(42)),
		wtF("touch", "t\n", 0o644, m(116)),
		wtF("owner", "o\n", 0o644, m(17)).withOwner(0, 0),
		wtF("empty-new", "", 0o644, m(43)),
		wtF("crlf", "a\r\nc\r\n", 0o644, m(19)),
		wtFB("nul8191", wtNulText(8192, 8191, "B side\n"), 0o644, m(21)),
		wtFB("nul8192", wtNulText(8193, 8192, "B side\n"), 0o644, m(22)),
		wtD("f2d", 0o755, m(23), wtF("inner", "i\n", 0o644, m(44))),
		wtF("d2f", "flat\n", 0o644, m(24)),
		wtF("setuid", "s\n", 0o755, m(32)),
	)
	p16 := wtPeriodic(16 << 20)
	p16mod := append([]byte(nil), p16...)
	changedLine := fmt.Sprintf("%-63s\n", "this line was changed in the B side of the big fixture")
	if len(changedLine) != 64 {
		return nil, fmt.Errorf("big fixture: changed line is %d bytes, want 64", len(changedLine))
	}
	copy(p16mod[8<<20:], changedLine)
	add("big-a", "files around MaxDiffBytes (16 MiB): exactly 16 MiB, one byte more, and one that grows past the limit",
		wtFB("exact16", p16, 0o644, m(1)),
		wtFB("over16", append(append([]byte(nil), p16...), 'x'), 0o644, m(2)),
		wtFB("grow", p16, 0o644, m(3)),
	)
	add("big-b", "big-a with one line changed in the middle of exact16 and over16, and grow one byte longer",
		wtFB("exact16", p16mod, 0o644, m(1)),
		wtFB("over16", append(append([]byte(nil), p16mod...), 'x'), 0o644, m(2)),
		wtFB("grow", append(append([]byte(nil), p16...), 'y'), 0o644, m(3)),
	)
	var wideA, wideB []wtNode
	for i := int64(0); i < 400; i++ {
		n := wtF(fmt.Sprintf("f%03d", i), fmt.Sprintf("%d\n", i), 0o644, m(i))
		wideA = append(wideA, n)
		switch i {
		case 100, 350:
			wideB = append(wideB, wtF(n.name, fmt.Sprintf("%d changed\n", i), 0o644, m(i)))
		case 200:
			wideB = append(wideB, wtF("f200a", "new\n", 0o644, m(i)))
		default:
			wideB = append(wideB, n)
		}
	}
	add("wide-a", "400 files: the directory is a DirNode over several DirLeaves", wideA...)
	add("wide-b", "wide-a with f100 and f350 edited and f200 replaced by f200a", wideB...)
	add("missing-content-a", "a file whose new content is absent from the store: the A side",
		wtF("ok.txt", "same\n", 0o644, m(1)),
		wtF("edit.txt", "before (missing-content fixture)\n", 0o644, m(2)),
	)
	add("missing-content-b", "edit.txt's content object is omitted from objects.bin",
		wtF("ok.txt", "same\n", 0o644, m(1)),
		wtF("edit.txt", "after (missing-content fixture)\n", 0o644, m(2)).omitted(),
	)
	add("broken-a", "a subdirectory: the A side",
		wtD("sub", 0o755, m(1), wtF("x", "x (broken fixture)\n", 0o644, m(2))),
	)
	add("broken-b", "sub's directory object is omitted from objects.bin",
		wtD("sub", 0o755, m(1), wtF("x", "x (broken fixture)\n", 0o644, m(2)), wtF("y", "y (broken fixture)\n", 0o644, m(3))).omitted(),
	)
	add("badkey-a", "a directory with a valid content key",
		wtD("d", 0o755, m(1), wtF("f", "f (badkey fixture)\n", 0o644, m(2))),
	)
	add("badkey-b", "the same directory with a 3-byte content key",
		wtD("d", 0o755, m(1)).withContentKey([]byte{1, 2, 3}),
	)
	add("badkey-add", "an added directory with a 3-byte content key",
		wtD("n", 0o755, m(1)).withContentKey([]byte{1, 2, 3}),
	)
	for _, d := range defs {
		if err := s.tree(d.name, d.doc, d.nodes...); err != nil {
			return nil, err
		}
	}
	// A tree whose root is not a directory object.
	nk, err := s.file([]byte("x (not a directory fixture)\n"))
	if err != nil {
		return nil, err
	}
	s.trees = append(s.trees, wtTreeInfo{name: "notdir", doc: "a Blob key used as a tree root", root: nk})

	if empty, _ := worktree.EmptyTree(); s.root("empty") != empty {
		return nil, fmt.Errorf("empty tree %s, want worktree.EmptyTree %s", s.root("empty"), empty)
	}
	if k := s.root("wide-a"); k.Type() != key.DirNode {
		return nil, fmt.Errorf("wide-a root is a %v, want a DirNode", k.Type())
	}
	if len(s.objs[s.root("wide-b")]) == 0 {
		return nil, errors.New("wide-b root not stored")
	}
	return s, nil
}

// ---- worktree/diff_trees.json ----

type wtDiffCase struct {
	Name     string         `json:"name"`
	A        string         `json:"a"`
	B        string         `json:"b"`
	GetCalls int            `json:"get_calls"`
	Changes  []wtChangeJSON `json:"changes"`
	Error    *string        `json:"error,omitempty"`
}

// wtDiskOp is one step of a disk script (see docs/vectorgen-worktree.md).
type wtDiskOp struct {
	Op        string   `json:"op"`
	Path      string   `json:"path"`
	Mode      *int     `json:"mode,omitempty"`
	Text      *string  `json:"text,omitempty"`
	Content   *Payload `json:"content,omitempty"`
	Target    *string  `json:"target,omitempty"`
	MtimeUnix *I64     `json:"mtime_unix,omitempty"`
	MtimeNsec *int     `json:"mtime_nsec,omitempty"`
	Size      *I64     `json:"size,omitempty"`
}

func wtOpDir(p string, perm int) wtDiskOp { return wtDiskOp{Op: "dir", Path: p, Mode: &perm} }
func wtOpFile(p, text string, perm int) wtDiskOp {
	return wtDiskOp{Op: "file", Path: p, Text: &text, Mode: &perm}
}
func wtOpSymlink(p, target string) wtDiskOp { return wtDiskOp{Op: "symlink", Path: p, Target: &target} }
func wtOpFifo(p string, perm int) wtDiskOp  { return wtDiskOp{Op: "fifo", Path: p, Mode: &perm} }
func wtOpRemove(p string) wtDiskOp          { return wtDiskOp{Op: "remove", Path: p} }
func wtOpChmod(p string, perm int) wtDiskOp { return wtDiskOp{Op: "chmod", Path: p, Mode: &perm} }
func wtOpMtime(p string, s int64, ns int) wtDiskOp {
	u := I64(s)
	return wtDiskOp{Op: "mtime", Path: p, MtimeUnix: &u, MtimeNsec: &ns}
}
func wtOpTruncate(p string, size int64) wtDiskOp {
	n := I64(size)
	return wtDiskOp{Op: "truncate", Path: p, Size: &n}
}

// wtRunOps executes a disk script under root.
func wtRunOps(root string, ops []wtDiskOp) error {
	for _, op := range ops {
		full := filepath.Join(root, filepath.FromSlash(op.Path))
		perm := func() uint32 {
			if op.Mode == nil {
				return 0
			}
			return uint32(*op.Mode)
		}
		var err error
		switch op.Op {
		case "dir":
			if err = os.Mkdir(full, 0o700); err == nil {
				err = unix.Chmod(full, perm())
			}
		case "file":
			var b []byte
			switch {
			case op.Text != nil:
				b = []byte(*op.Text)
			case op.Content != nil:
				b = op.Content.Materialize()
			}
			if err = os.WriteFile(full, b, 0o600); err == nil {
				err = unix.Chmod(full, perm())
			}
		case "symlink":
			err = os.Symlink(*op.Target, full)
		case "fifo":
			if err = unix.Mkfifo(full, 0o600); err == nil {
				err = unix.Chmod(full, perm())
			}
		case "remove":
			err = os.RemoveAll(full)
		case "chmod":
			err = unix.Chmod(full, perm())
		case "mtime":
			t := time.Unix(int64(*op.MtimeUnix), int64(*op.MtimeNsec))
			err = os.Chtimes(full, t, t)
		case "truncate":
			err = os.Truncate(full, int64(*op.Size))
		default:
			err = fmt.Errorf("unknown op %q", op.Op)
		}
		if err != nil {
			return fmt.Errorf("op %s %s: %w", op.Op, op.Path, err)
		}
	}
	return nil
}

type wtBaseObject struct {
	Key   Hex `json:"key"`
	Bytes Hex `json:"bytes"`
}

type wtScanChange struct {
	Path    *string `json:"path,omitempty"`
	PathHex *string `json:"path_hex,omitempty"`
	Kind    string  `json:"kind"`
	OldMode *U64    `json:"old_mode"`
	NewMode *U64    `json:"new_mode"`
}

type wtScanCase struct {
	Name         string         `json:"name"`
	Doc          string         `json:"doc"`
	Setup        []wtDiskOp     `json:"setup"`
	BaseKey      Hex            `json:"base_key,omitempty"`
	BaseObjects  []wtBaseObject `json:"base_objects,omitempty"`
	Edit         []wtDiskOp     `json:"edit"`
	SyncedAtUnix I64            `json:"synced_at_unix"`
	SyncedAtNsec int            `json:"synced_at_nsec"`
	Jobs         int            `json:"jobs"`
	Changes      []wtScanChange `json:"changes"`
	Error        *string        `json:"error,omitempty"`
}

type wtDiffTreesFile struct {
	DiffTrees []wtDiffCase `json:"diff_trees"`
	Scan      []wtScanCase `json:"scan"`
}

func wtModeOf(e *fstree.Entry) *U64 {
	if e == nil {
		return nil
	}
	m := e.Mode
	if m&unix.S_IFMT == unix.S_IFLNK {
		m = unix.S_IFLNK // symlink permission bits differ between macOS and Linux
	}
	v := U64(m)
	return &v
}

// wtScanFixture is scanFixture of worktree/scan_test.go with fixed mtimes.
func wtScanFixture(extra ...wtDiskOp) []wtDiskOp {
	ops := []wtDiskOp{
		wtOpFile("a.txt", "alpha", 0o644), wtOpMtime("a.txt", wtT0+1, 0),
		wtOpFile("run.sh", "#!/bin/sh", 0o644), wtOpMtime("run.sh", wtT0+2, 0),
		wtOpDir("sub", 0o755),
		wtOpFile("sub/b.txt", "beta", 0o644), wtOpMtime("sub/b.txt", wtT0+3, 0),
		wtOpMtime("sub", wtT0+4, 0),
		wtOpFile("gone.txt", "bye", 0o644), wtOpMtime("gone.txt", wtT0+5, 0),
		wtOpSymlink("link", "a.txt"),
	}
	return append(ops, extra...)
}

type wtScanDef struct {
	name, doc   string
	setup, edit []wtDiskOp
	baseKey     *key.Key
	baseObjects []fstree.Object
	syncedAt    time.Time
}

func wtScanDefs() ([]wtScanDef, error) {
	synced := time.Unix(wtSynced, 0)
	badLeaf, err := fstree.EncodeDirLeaf([]fstree.Entry{{Name: []byte("a.txt"), Mode: unix.S_IFREG | 0o644, UID: 1000, GID: 1000, Mtime: wtMt(wtT0+1, 0), ContentKey: []byte{1, 2, 3}}})
	if err != nil {
		return nil, err
	}
	missing, err := key.New(key.DirLeaf, 7, []byte("missing"))
	if err != nil {
		return nil, err
	}
	return []wtScanDef{
		{name: "clean", doc: "TestScan_CleanTreeHasNoChanges", setup: wtScanFixture(), syncedAt: synced},
		{name: "every-kind", doc: "TestScan_EveryKind: modified, mode, deleted, new file and directory, mtime only, retargeted link; the root .dstore is invisible",
			setup: wtScanFixture(), syncedAt: synced, edit: []wtDiskOp{
				wtOpFile("a.txt", "alpha 2", 0o644),
				wtOpChmod("run.sh", 0o755),
				wtOpRemove("gone.txt"),
				wtOpFile("new.txt", "new", 0o644),
				wtOpDir("newdir", 0o755),
				wtOpFile("newdir/c.txt", "c", 0o644),
				wtOpMtime("sub/b.txt", 1_500_000_000, 0),
				wtOpRemove("link"),
				wtOpSymlink("link", "run.sh"),
				wtOpDir(".dstore", 0o755),
				wtOpFile(".dstore/junk", "x", 0o644),
			}},
		{name: "ignored-base-path", doc: "TestScan_IgnoredBasePathIsDeleted: base entries are not filtered by .amberignore",
			setup: wtScanFixture(), syncedAt: synced, edit: []wtDiskOp{wtOpFile(".amberignore", "gone.txt\n", 0o644)}},
		{name: "ignored-in-subdirectory", doc: "a .amberignore below the root applies to its directory",
			setup: wtScanFixture(), syncedAt: synced, edit: []wtDiskOp{wtOpFile("sub/.amberignore", "b.txt\n", 0o644)}},
		{name: "racy-window-exact", doc: "base mtime = syncedAt - 2s exactly: not hashed, the same-size same-mtime edit is invisible",
			setup: wtScanFixture(wtOpMtime("a.txt", wtSynced-2, 0)), syncedAt: synced,
			edit: []wtDiskOp{wtOpFile("a.txt", "ALPHA", 0o644), wtOpMtime("a.txt", wtSynced-2, 0)}},
		{name: "racy-window-1ns-inside", doc: "base mtime = syncedAt - 2s + 1ns: hashed, the edit is found",
			setup: wtScanFixture(wtOpMtime("a.txt", wtSynced-2, 1)), syncedAt: synced,
			edit: []wtDiskOp{wtOpFile("a.txt", "ALPHA", 0o644), wtOpMtime("a.txt", wtSynced-2, 1)}},
		{name: "stat-heuristic-size-differs", doc: "same mtime, other size: hashed",
			setup: wtScanFixture(), syncedAt: synced,
			edit: []wtDiskOp{wtOpFile("a.txt", "alpha!", 0o644), wtOpMtime("a.txt", wtT0+1, 0)}},
		{name: "type-change-expands", doc: "TestScan_TypeChangeExpands: file to directory and directory to file",
			setup: wtScanFixture(), syncedAt: synced, edit: []wtDiskOp{
				wtOpRemove("a.txt"), wtOpDir("a.txt", 0o755), wtOpFile("a.txt/inner", "i", 0o644),
				wtOpRemove("sub"), wtOpFile("sub", "flat", 0o644),
			}},
		{name: "walk-order", doc: "added a/x, a-b: changes come in walk order, a directory before its contents",
			setup: wtScanFixture(), syncedAt: synced, edit: []wtDiskOp{
				wtOpDir("a", 0o755), wtOpFile("a/x", "x", 0o644), wtOpFile("a-b", "ab", 0o644),
			}},
		{name: "root-dstore-in-base", doc: "a .dstore the base tree holds at the root comes out deleted",
			setup:    wtScanFixture(wtOpDir(".dstore", 0o755), wtOpFile(".dstore/junk", "x", 0o644), wtOpMtime(".dstore/junk", wtT0+6, 0), wtOpMtime(".dstore", wtT0+7, 0)),
			syncedAt: synced},
		{name: "nested-dstore-is-data", doc: "a .dstore below the root is ordinary data",
			setup: wtScanFixture(), syncedAt: synced, edit: []wtDiskOp{
				wtOpDir("sub/.dstore", 0o755), wtOpFile("sub/.dstore/f", "f", 0o644),
			}},
		{name: "symlink-to-directory-and-patterns", doc: "a directory-only pattern does not match a symlink to a directory; ignored names never appear",
			setup:    wtScanFixture(wtOpDir("realdir", 0o755), wtOpFile("realdir/f", "f", 0o644), wtOpMtime("realdir/f", wtT0+6, 0), wtOpMtime("realdir", wtT0+7, 0)),
			syncedAt: synced, edit: []wtDiskOp{
				wtOpFile(".amberignore", "linkdir/\nbuild/\n*.log\n", 0o644),
				wtOpSymlink("linkdir", "realdir"),
				wtOpDir("build", 0o755), wtOpFile("build/o", "o", 0o644),
				wtOpFile("x.log", "l", 0o644),
			}},
		{name: "fifo-and-directory-mode", doc: "an added fifo and a directory chmod",
			setup: wtScanFixture(), syncedAt: synced, edit: []wtDiskOp{wtOpFifo("pipe", 0o644), wtOpChmod("sub", 0o700)}},
		{name: "error-amberignore-is-a-directory", doc: "amberignore.Root's read error is returned raw",
			setup: wtScanFixture(), syncedAt: synced, edit: []wtDiskOp{wtOpDir(".amberignore", 0o755)}},
		{name: "error-missing-base-object", doc: "the base key is not in the store",
			setup: wtScanFixture(), baseKey: &missing, syncedAt: synced},
		{name: "error-bad-base-content-key", doc: "a base file entry with a 3-byte content key",
			setup:   []wtDiskOp{wtOpFile("a.txt", "alpha", 0o644), wtOpMtime("a.txt", wtT0+1, 0)},
			baseKey: &badLeaf.Key, baseObjects: []fstree.Object{badLeaf}, syncedAt: synced},
	}, nil
}

// wtRunScan lays out setup in a temporary root, takes the base (ingested, or base_key with base_objects
// stored), runs edit and scans.
func wtRunScan(d wtScanDef) (wtScanCase, error) {
	c := wtScanCase{Name: d.name, Doc: d.doc, Setup: d.setup, Edit: d.edit, SyncedAtUnix: I64(d.syncedAt.Unix()), SyncedAtNsec: d.syncedAt.Nanosecond(), Jobs: 2}
	if c.Setup == nil {
		c.Setup = []wtDiskOp{}
	}
	if c.Edit == nil {
		c.Edit = []wtDiskOp{}
	}
	root, done, err := wtTemp()
	if err != nil {
		return c, err
	}
	defer done()
	storeDir, doneStore, err := wtTemp()
	if err != nil {
		return c, err
	}
	defer doneStore()
	st, err := packstore.Open(filepath.Join(storeDir, "ps"), packstore.WithSync(false))
	if err != nil {
		return c, err
	}
	defer st.Close()
	if err := wtRunOps(root, d.setup); err != nil {
		return c, err
	}
	var base key.Key
	if d.baseKey != nil {
		base = *d.baseKey
		c.BaseKey = Hex(base[:])
		for _, o := range d.baseObjects {
			if err := st.Put(o.Key, o.Bytes); err != nil {
				return c, err
			}
			c.BaseObjects = append(c.BaseObjects, wtBaseObject{Key: Hex(o.Key[:]), Bytes: Hex(o.Bytes)})
		}
	} else if base, _, err = ingest.Dir(st, root, ingest.Opts{Jobs: 2}); err != nil {
		return c, err
	}
	if err := wtRunOps(root, d.edit); err != nil {
		return c, err
	}
	changes, err := worktree.Scan(root, base, st.Get, d.syncedAt, 2)
	c.Error = wtErrText(err, root)
	if changes != nil {
		c.Changes = make([]wtScanChange, 0, len(changes))
		for _, ch := range changes {
			sc := wtScanChange{Kind: ch.Kind.String(), OldMode: wtModeOf(ch.Old), NewMode: wtModeOf(ch.New)}
			sc.Path, sc.PathHex = wtText(ch.Path)
			c.Changes = append(c.Changes, sc)
		}
	}
	return c, nil
}

// wtDiffPairs are the tree pairs of diff_trees.json; wtUnifiedPairs those unified.json renders.
var (
	wtDiffPairs = []struct{ name, a, b string }{
		{"diff", "diff-a", "diff-b"},
		{"diff-reverse", "diff-b", "diff-a"},
		{"clone", "empty", "diff-a"},
		{"delete-all", "diff-a", "empty"},
		{"identical", "diff-a", "diff-a"},
		{"every-kind", "every-a", "every-b"},
		{"prunes-equal-subtrees", "prune-a", "prune-b"},
		{"walk-order", "walk-a", "walk-b"},
		{"non-utf8", "nonutf8-a", "nonutf8-b"},
		{"special", "special-a", "special-b"},
		{"special-reverse", "special-b", "special-a"},
		{"clone-special", "empty", "special-a"},
		{"big", "big-a", "big-b"},
		{"wide", "wide-a", "wide-b"},
		{"missing-content", "missing-content-a", "missing-content-b"},
		{"error-missing-subtree", "broken-a", "broken-b"},
		{"error-not-a-directory", "empty", "notdir"},
		{"error-bad-directory-content-key", "badkey-a", "badkey-b"},
		{"error-bad-added-directory-content-key", "empty", "badkey-add"},
	}
	wtUnifiedPairs = []struct{ name, a, b string }{
		{"diff", "diff-a", "diff-b"},
		{"diff-reverse", "diff-b", "diff-a"},
		{"clone", "empty", "diff-a"},
		{"delete-all", "diff-a", "empty"},
		{"identical", "diff-a", "diff-a"},
		{"every-kind", "every-a", "every-b"},
		{"walk-order", "walk-a", "walk-b"},
		{"non-utf8", "nonutf8-a", "nonutf8-b"},
		{"special", "special-a", "special-b"},
		{"special-reverse", "special-b", "special-a"},
		{"clone-special", "empty", "special-a"},
		{"big", "big-a", "big-b"},
		{"wide", "wide-a", "wide-b"},
		{"missing-content", "missing-content-a", "missing-content-b"},
	}
)

func wtDiffTreesVectors(s *wtStore) (any, error) {
	var v wtDiffTreesFile
	for _, p := range wtDiffPairs {
		calls := 0
		changes, err := worktree.DiffTrees(s.getter(&calls), s.root(p.a), s.root(p.b))
		v.DiffTrees = append(v.DiffTrees, wtDiffCase{Name: p.name, A: p.a, B: p.b, GetCalls: calls, Changes: wtChanges(changes), Error: wtErrText(err, "")})
	}
	defs, err := wtScanDefs()
	if err != nil {
		return nil, err
	}
	for _, d := range defs {
		c, err := wtRunScan(d)
		if err != nil {
			return nil, fmt.Errorf("scan %s: %w", d.name, err)
		}
		v.Scan = append(v.Scan, c)
	}
	return v, nil
}

// ---- worktree/merge.json ----

type wtConflictJSON struct {
	Path         *string `json:"path,omitempty"`
	PathHex      *string `json:"path_hex,omitempty"`
	LocalPath    *string `json:"local_path,omitempty"`
	LocalPathHex *string `json:"local_path_hex,omitempty"`
	LocalKind    string  `json:"local_kind"`
	IncomingKind string  `json:"incoming_kind"`
}

type wtMergeCase struct {
	Name      string           `json:"name"`
	Local     []wtChangeJSON   `json:"local"`
	Incoming  []wtChangeJSON   `json:"incoming"`
	Apply     []wtPathKind     `json:"apply"`
	Conflicts []wtConflictJSON `json:"conflicts"`
}

type wtMergeFile struct {
	Cases []wtMergeCase `json:"cases"`
}

// wtFileEntry and wtDirEntry are fileEntry and dirEntry of worktree/merge_test.go.
func wtFileEntry(name string, ck byte, mode uint64) *fstree.Entry {
	k := make([]byte, 32)
	k[1] = ck
	return &fstree.Entry{Name: []byte(name), Mode: unix.S_IFREG | mode, ContentKey: k}
}

func wtDirEntry(name string) *fstree.Entry {
	k := make([]byte, 32)
	k[0] = 0x20
	return &fstree.Entry{Name: []byte(name), Mode: unix.S_IFDIR | 0o755, ContentKey: k}
}

func wtMergeVectors() (any, error) {
	type C = worktree.Change
	const (
		added    = worktree.Added
		deleted  = worktree.Deleted
		modified = worktree.Modified
		typeChg  = worktree.TypeChanged
		modeChg  = worktree.ModeChanged
		metaChg  = worktree.MetaChanged
	)
	base := wtFileEntry("f", 1, 0o644)
	v2 := wtFileEntry("f", 2, 0o644)
	v3 := wtFileEntry("f", 3, 0o644)
	base755 := wtFileEntry("f", 1, 0o755)
	v2late := wtFileEntry("f", 2, 0o644)
	v2late.Mtime = 5
	link := &fstree.Entry{Name: []byte("f"), Mode: unix.S_IFLNK | 0o777, LinkTarget: []byte("t")}
	dir700 := wtDirEntry("e")
	dir700.Mode = unix.S_IFDIR | 0o700
	dirOther := wtDirEntry("n")
	dirOther.ContentKey[5] = 9
	dirOther.Mtime = 7
	cases := []struct {
		name            string
		local, incoming []C
	}{
		// The 13 cases of TestMerge (worktree/merge_test.go).
		{"remote only", nil, []C{{Path: "a", Kind: modified, Old: base, New: v2}}},
		{"local only", []C{{Path: "a", Kind: modified, Old: base, New: v2}}, nil},
		{"both differ", []C{{Path: "a", Kind: modified, Old: base, New: v2}}, []C{{Path: "a", Kind: modified, Old: base, New: v3}}},
		{"same edit twice", []C{{Path: "a", Kind: modified, Old: base, New: v2}}, []C{{Path: "a", Kind: modified, Old: base, New: v2}}},
		{"local touch", []C{{Path: "a", Kind: metaChg, Old: base, New: base}}, []C{{Path: "a", Kind: modified, Old: base, New: v2}}},
		{"both delete", []C{{Path: "a", Kind: deleted, Old: base}}, []C{{Path: "a", Kind: deleted, Old: base}}},
		{"local delete, remote edit", []C{{Path: "a", Kind: deleted, Old: base}}, []C{{Path: "a", Kind: modified, Old: base, New: v2}}},
		{"local edit, remote delete", []C{{Path: "a", Kind: modified, Old: base, New: v2}}, []C{{Path: "a", Kind: deleted, Old: base}}},
		{"remote add under locally deleted dir",
			[]C{{Path: "d", Kind: deleted, Old: wtDirEntry("d")}, {Path: "d/x", Kind: deleted, Old: base}},
			[]C{{Path: "d/y", Kind: added, New: v2}}},
		{"remote delete under locally deleted dir",
			[]C{{Path: "d", Kind: deleted, Old: wtDirEntry("d")}, {Path: "d/x", Kind: deleted, Old: base}},
			[]C{{Path: "d/x", Kind: deleted, Old: base}}},
		{"remote add under locally retyped dir",
			[]C{{Path: "d", Kind: typeChg, Old: wtDirEntry("d"), New: v2}, {Path: "d/x", Kind: deleted, Old: base}},
			[]C{{Path: "d/y", Kind: added, New: v2}}},
		{"remote retypes dir with local edits below",
			[]C{{Path: "d/x", Kind: modified, Old: base, New: v2}},
			[]C{{Path: "d", Kind: typeChg, Old: wtDirEntry("d"), New: v3}, {Path: "d/x", Kind: deleted, Old: base}}},
		{"remote deletes dir, local adds below",
			[]C{{Path: "d/new", Kind: added, New: v2}},
			[]C{{Path: "d", Kind: deleted, Old: wtDirEntry("d")}, {Path: "d/x", Kind: deleted, Old: base}}},
		// Further rules.
		{"ancestor walk skips an intermediate mode change",
			[]C{{Path: "d", Kind: deleted, Old: wtDirEntry("d")}, {Path: "d/e", Kind: modeChg, Old: wtDirEntry("e"), New: dir700}},
			[]C{{Path: "d/e/f", Kind: added, New: v2}}},
		{"deep ancestor deleted", []C{{Path: "d", Kind: deleted, Old: wtDirEntry("d")}}, []C{{Path: "d/e/f", Kind: modified, Old: base, New: v2}}},
		{"ancestor meta change is no obstacle", []C{{Path: "d", Kind: metaChg, Old: wtDirEntry("d"), New: wtDirEntry("d")}}, []C{{Path: "d/x", Kind: modified, Old: base, New: v2}}},
		{"local retype file to dir, remote adds below", []C{{Path: "a", Kind: typeChg, Old: base, New: wtDirEntry("a")}}, []C{{Path: "a/x", Kind: added, New: v2}}},
		{"remote delete under locally retyped dir", []C{{Path: "d", Kind: typeChg, Old: wtDirEntry("d"), New: v2}}, []C{{Path: "d/x", Kind: deleted, Old: base}}},
		{"both add same dir with equal perms", []C{{Path: "n", Kind: added, New: wtDirEntry("n")}}, []C{{Path: "n", Kind: added, New: dirOther}}},
		{"both add same dir with different perms", []C{{Path: "n", Kind: added, New: dir700}}, []C{{Path: "n", Kind: added, New: dirOther}}},
		{"both add same file content, different mtime", []C{{Path: "a", Kind: added, New: v2}}, []C{{Path: "a", Kind: added, New: v2late}}},
		{"both add different files", []C{{Path: "a", Kind: added, New: v2}}, []C{{Path: "a", Kind: added, New: v3}}},
		{"remote chmod against local edit", []C{{Path: "a", Kind: modified, Old: base, New: v2}}, []C{{Path: "a", Kind: modeChg, Old: base, New: base755}}},
		{"remote chmod over local meta change", []C{{Path: "a", Kind: metaChg, Old: base, New: base}}, []C{{Path: "a", Kind: modeChg, Old: base, New: base755}}},
		{"local chmod against remote edit", []C{{Path: "a", Kind: modeChg, Old: base, New: base755}}, []C{{Path: "a", Kind: modified, Old: base, New: v2}}},
		{"same chmod twice", []C{{Path: "a", Kind: modeChg, Old: base, New: base755}}, []C{{Path: "a", Kind: modeChg, Old: base, New: base755}}},
		{"equivalent retypes on both sides", []C{{Path: "a", Kind: typeChg, Old: base, New: link}}, []C{{Path: "a", Kind: typeChg, Old: base, New: link}}},
		{"local meta change, remote delete", []C{{Path: "a", Kind: metaChg, Old: base, New: base}}, []C{{Path: "a", Kind: deleted, Old: base}}},
		{"local delete, remote meta change", []C{{Path: "a", Kind: deleted, Old: base}}, []C{{Path: "a", Kind: metaChg, Old: base, New: base}}},
		{"remote retypes dir, local mode change at the dir", []C{{Path: "d", Kind: modeChg, Old: wtDirEntry("d"), New: dir700}}, []C{{Path: "d", Kind: typeChg, Old: wtDirEntry("d"), New: v2}}},
		{"remote retypes dir, local change deep below", []C{{Path: "d/e/f", Kind: added, New: v2}}, []C{{Path: "d", Kind: typeChg, Old: wtDirEntry("d"), New: v2}}},
		{"firstBelow ignores d-x before d/", []C{{Path: "d-x", Kind: modified, Old: base, New: v2}}, []C{{Path: "d", Kind: typeChg, Old: wtDirEntry("d"), New: v2}}},
		{"firstBelow ignores d0 after d/", []C{{Path: "d0", Kind: modified, Old: base, New: v2}}, []C{{Path: "d", Kind: typeChg, Old: wtDirEntry("d"), New: v2}}},
		{"remote retypes file to dir, local change below is not looked up", []C{{Path: "a/x", Kind: added, New: v2}}, []C{{Path: "a", Kind: typeChg, Old: base, New: wtDirEntry("a")}}},
		{"goneAbove stops before a leading slash", []C{{Path: "", Kind: deleted, Old: wtDirEntry("r")}}, []C{{Path: "/x", Kind: added, New: v2}}},
		{"incoming order is kept",
			[]C{{Path: "b", Kind: modified, Old: base, New: v2}, {Path: "z", Kind: deleted, Old: base}},
			[]C{{Path: "z", Kind: modified, Old: base, New: v3}, {Path: "y", Kind: added, New: v2}, {Path: "b", Kind: modified, Old: base, New: v3}, {Path: "a", Kind: deleted, Old: base}}},
		{"non-UTF-8 paths", []C{{Path: "d\xff", Kind: deleted, Old: wtDirEntry("d\xff")}}, []C{{Path: "d\xff/x", Kind: added, New: v2}, {Path: "e\xfe", Kind: added, New: v2}}},
	}
	var v wtMergeFile
	for _, c := range cases {
		apply, conflicts := worktree.Merge(c.local, c.incoming)
		mc := wtMergeCase{Name: c.name, Local: wtChanges(c.local), Incoming: wtChanges(c.incoming)}
		if apply != nil {
			mc.Apply = []wtPathKind{}
		}
		for _, a := range apply {
			pk := wtPathKind{Kind: a.Kind.String()}
			pk.Path, pk.PathHex = wtText(a.Path)
			mc.Apply = append(mc.Apply, pk)
		}
		if conflicts != nil {
			mc.Conflicts = []wtConflictJSON{}
		}
		for _, cf := range conflicts {
			cj := wtConflictJSON{LocalKind: cf.Local.Kind.String(), IncomingKind: cf.Incoming.Kind.String()}
			cj.Path, cj.PathHex = wtText(cf.Path)
			cj.LocalPath, cj.LocalPathHex = wtText(cf.Local.Path)
			mc.Conflicts = append(mc.Conflicts, cj)
		}
		v.Cases = append(v.Cases, mc)
	}
	return v, nil
}

// ---- worktree/unified.json ----

type wtUnifiedCase struct {
	Name         string  `json:"name"`
	A            string  `json:"a"`
	B            string  `json:"b"`
	Unified      *string `json:"unified,omitempty"`
	UnifiedHex   *string `json:"unified_hex,omitempty"`
	UnifiedError *string `json:"unified_error,omitempty"`
	Stat         *string `json:"stat,omitempty"`
	StatHex      *string `json:"stat_hex,omitempty"`
	StatError    *string `json:"stat_error,omitempty"`
}

type wtDiskDiffCase struct {
	Name         string     `json:"name"`
	Doc          string     `json:"doc"`
	Setup        []wtDiskOp `json:"setup"`
	Edit         []wtDiskOp `json:"edit"`
	AfterScan    []wtDiskOp `json:"after_scan"`
	SyncedAtUnix I64        `json:"synced_at_unix"`
	SyncedAtNsec int        `json:"synced_at_nsec"`
	Jobs         int        `json:"jobs"`
	Changes      []string   `json:"changes"`
	Unified      *string    `json:"unified,omitempty"`
	UnifiedHex   *string    `json:"unified_hex,omitempty"`
	UnifiedError *string    `json:"unified_error,omitempty"`
	Stat         *string    `json:"stat,omitempty"`
	StatHex      *string    `json:"stat_hex,omitempty"`
	StatError    *string    `json:"stat_error,omitempty"`
}

type wtUnifiedFile struct {
	Cases []wtUnifiedCase  `json:"cases"`
	Disk  []wtDiskDiffCase `json:"disk"`
}

// wtRendered is what Unified and Stat wrote and returned.
type wtRendered struct {
	u, uHex, uErr, st, stHex, stErr *string
}

// wtRender runs Unified and Stat over changes. Outputs must not contain the temporary root.
func wtRender(changes []worktree.Change, old, new worktree.Source, root string) (wtRendered, error) {
	var ub, sb bytes.Buffer
	uerr := worktree.Unified(&ub, changes, old, new)
	serr := worktree.Stat(&sb, changes, old, new)
	if root != "" && (strings.Contains(ub.String(), root) || strings.Contains(sb.String(), root)) {
		return wtRendered{}, errors.New("diff output holds the temporary root")
	}
	r := wtRendered{uErr: wtErrText(uerr, root), stErr: wtErrText(serr, root)}
	r.u, r.uHex = wtText(ub.String())
	r.st, r.stHex = wtText(sb.String())
	return r, nil
}

func wtUnifiedVectors(s *wtStore) (any, error) {
	var v wtUnifiedFile
	get := s.getter(nil)
	src := worktree.TreeSource{Get: get}
	for _, p := range wtUnifiedPairs {
		changes, err := worktree.DiffTrees(get, s.root(p.a), s.root(p.b))
		if err != nil {
			return nil, fmt.Errorf("%s: %w", p.name, err)
		}
		r, err := wtRender(changes, src, src, "")
		if err != nil {
			return nil, fmt.Errorf("%s: %w", p.name, err)
		}
		v.Cases = append(v.Cases, wtUnifiedCase{Name: p.name, A: p.a, B: p.b, Unified: r.u, UnifiedHex: r.uHex, UnifiedError: r.uErr, Stat: r.st, StatHex: r.stHex, StatError: r.stErr})
	}
	diffA := []wtDiskOp{
		wtOpFile("edit.txt", "one\ntwo\nthree\n", 0o644), wtOpMtime("edit.txt", wtT0+1, 0),
		wtOpFile("bin", "a\x00b", 0o644), wtOpMtime("bin", wtT0+2, 0),
		wtOpFile("gone.txt", "bye\n", 0o644), wtOpMtime("gone.txt", wtT0+3, 0),
		wtOpFile("run.sh", "#!/bin/sh\n", 0o644), wtOpMtime("run.sh", wtT0+4, 0),
		wtOpDir("sub", 0o755), wtOpFile("sub/x", "x\n", 0o644), wtOpMtime("sub/x", wtT0+5, 0), wtOpMtime("sub", wtT0+6, 0),
		wtOpSymlink("link", "t1"),
	}
	disk := []struct {
		name, doc              string
		setup, edit, afterScan []wtDiskOp
	}{
		{"tree-to-disk", "TestUnified_TreeToDisk: the diffFixtures edits made on disk", diffA, []wtDiskOp{
			wtOpFile("edit.txt", "one\n2\nthree\n", 0o644),
			wtOpFile("bin", "a\x00c", 0o644),
			wtOpFile("new.txt", "hi\n", 0o644),
			wtOpChmod("run.sh", 0o755),
			wtOpChmod("sub", 0o700),
			wtOpRemove("gone.txt"),
			wtOpRemove("link"), wtOpSymlink("link", "t2"),
		}, nil},
		{"disk-sizes", "DiskSource reads the size first: over MaxDiffBytes is binary without reading; exactly MaxDiffBytes of zeros is read and binary",
			[]wtDiskOp{wtOpFile("big", "x\n", 0o644), wtOpMtime("big", wtT0+1, 0), wtOpFile("exact", "y\n", 0o644), wtOpMtime("exact", wtT0+2, 0)},
			[]wtDiskOp{wtOpTruncate("big", 16<<20+1), wtOpTruncate("exact", 16<<20)}, nil},
		{"mode-only-and-empty", "mode changes on a file and a directory, an added empty file and an added fifo: headers only",
			wtScanFixture(), []wtDiskOp{wtOpChmod("run.sh", 0o755), wtOpChmod("sub", 0o700), wtOpFile("empty", "", 0o644), wtOpFifo("pipe", 0o644)}, nil},
		{"vanished-after-scan", "a file removed between Scan and Unified: the header is written, then the lstat error",
			wtScanFixture(), []wtDiskOp{wtOpRemove("link"), wtOpSymlink("link", "run.sh"), wtOpFile("new.txt", "n\n", 0o644)},
			[]wtDiskOp{wtOpRemove("new.txt")}},
		{"readlink-error", "a symlink removed between Scan and Unified", wtScanFixture(),
			[]wtDiskOp{wtOpRemove("link"), wtOpSymlink("link", "run.sh")}, []wtDiskOp{wtOpRemove("link")}},
	}
	for _, d := range disk {
		c, err := wtRunDiskDiff(d.name, d.doc, d.setup, d.edit, d.afterScan)
		if err != nil {
			return nil, fmt.Errorf("disk %s: %w", d.name, err)
		}
		v.Disk = append(v.Disk, c)
	}
	return v, nil
}

func wtRunDiskDiff(name, doc string, setup, edit, afterScan []wtDiskOp) (wtDiskDiffCase, error) {
	c := wtDiskDiffCase{Name: name, Doc: doc, Setup: setup, Edit: edit, AfterScan: afterScan, SyncedAtUnix: wtSynced, Jobs: 2}
	if c.AfterScan == nil {
		c.AfterScan = []wtDiskOp{}
	}
	root, done, err := wtTemp()
	if err != nil {
		return c, err
	}
	defer done()
	storeDir, doneStore, err := wtTemp()
	if err != nil {
		return c, err
	}
	defer doneStore()
	st, err := packstore.Open(filepath.Join(storeDir, "ps"), packstore.WithSync(false))
	if err != nil {
		return c, err
	}
	defer st.Close()
	if err := wtRunOps(root, setup); err != nil {
		return c, err
	}
	base, _, err := ingest.Dir(st, root, ingest.Opts{Jobs: 2})
	if err != nil {
		return c, err
	}
	if err := wtRunOps(root, edit); err != nil {
		return c, err
	}
	changes, err := worktree.Scan(root, base, st.Get, time.Unix(wtSynced, 0), 2)
	if err != nil {
		return c, err
	}
	c.Changes = []string{}
	for _, ch := range changes {
		c.Changes = append(c.Changes, ch.Kind.String()+" "+ch.Path)
	}
	if err := wtRunOps(root, afterScan); err != nil {
		return c, err
	}
	r, err := wtRender(changes, worktree.TreeSource{Get: st.Get}, worktree.DiskSource{Root: root}, root)
	if err != nil {
		return c, err
	}
	c.Unified, c.UnifiedHex, c.UnifiedError, c.Stat, c.StatHex, c.StatError = r.u, r.uHex, r.uErr, r.st, r.stHex, r.stErr
	return c, nil
}

// ---- worktree/cli.json ----

type wtKindString struct {
	Kind int    `json:"kind"`
	Out  string `json:"out"`
}

type wtTypeName struct {
	Mode U64    `json:"mode"`
	Out  string `json:"out"`
}

type wtDescribeCase struct {
	Name   string       `json:"name"`
	Change wtChangeJSON `json:"change"`
	Out    *string      `json:"out,omitempty"`
	OutHex *string      `json:"out_hex,omitempty"`
}

type wtResolveTicketCase struct {
	Flag   string  `json:"flag"`
	Stored string  `json:"stored"`
	Env    string  `json:"env"`
	OK     bool    `json:"ok"`
	Out    string  `json:"out"`
	Error  *string `json:"error,omitempty"`
}

type wtFilterPathsCase struct {
	Name    string   `json:"name"`
	Cwd     string   `json:"cwd"`
	Args    []string `json:"args"`
	Changes []string `json:"changes"`
	Kept    []string `json:"kept"`
	Error   *string  `json:"error,omitempty"`
}

type wtPrintfArg struct {
	Type  string `json:"type"`
	Value string `json:"value"`
}

type wtPrintfCase struct {
	Name   string        `json:"name"`
	Stream string        `json:"stream"`
	Func   string        `json:"func"`
	Format string        `json:"format"`
	Args   []wtPrintfArg `json:"args"`
	Out    *string       `json:"out,omitempty"`
	OutHex *string       `json:"out_hex,omitempty"`
}

type wtViewNode struct {
	ID    Hex      `json:"id"`
	Addrs []string `json:"addrs"`
}

type wtTicketFromViewCase struct {
	Name        string       `json:"name"`
	ClusterID   Hex          `json:"cluster_id"`
	Incarnation U64          `json:"incarnation"`
	Nodes       []wtViewNode `json:"nodes"`
	Ticket      string       `json:"ticket"`
}

type wtFetchedDescCase struct {
	Name string `json:"name"`
	Key  Hex    `json:"key"`
	Tree Hex    `json:"tree"`
	Out  string `json:"out"`
}

type wtPushedKeyCase struct {
	Name   string `json:"name"`
	Root   Hex    `json:"root"`
	Commit Hex    `json:"commit"`
	Out    Hex    `json:"out"`
}

type wtCLIFile struct {
	KindString     []wtKindString         `json:"kind_string"`
	TypeName       []wtTypeName           `json:"type_name"`
	DescribeChange []wtDescribeCase       `json:"describe_change"`
	StatusRow      []wtDescribeCase       `json:"status_row"`
	ResolveTicket  []wtResolveTicketCase  `json:"resolve_ticket"`
	FilterPaths    []wtFilterPathsCase    `json:"filter_paths"`
	FetchedDesc    []wtFetchedDescCase    `json:"fetched_desc"`
	PushedKey      []wtPushedKeyCase      `json:"pushed_key"`
	Printf         []wtPrintfCase         `json:"printf"`
	TicketFromView []wtTicketFromViewCase `json:"ticket_from_view"`
}

func wtCLIVectors() (any, error) {
	var v wtCLIFile
	for _, k := range []int{0, 1, 2, 3, 4, 5, 6, -1} {
		v.KindString = append(v.KindString, wtKindString{Kind: k, Out: worktree.Kind(k).String()})
	}
	for _, m := range []uint64{
		unix.S_IFREG | 0o644, unix.S_IFDIR | 0o755, unix.S_IFLNK | 0o777, unix.S_IFIFO | 0o600, unix.S_IFSOCK | 0o755,
		unix.S_IFCHR | 0o644, unix.S_IFBLK | 0o660, 0, 0o755, 0o160000, 0o160644, 0o170000, 0o030000, 0o050000,
		0o1100644, 1<<63 | unix.S_IFDIR,
	} {
		v.TypeName = append(v.TypeName, wtTypeName{Mode: U64(m), Out: worktree.TypeName(m)})
	}
	file := func(perm uint64) *fstree.Entry { return &fstree.Entry{Mode: unix.S_IFREG | perm} }
	dir := func(perm uint64) *fstree.Entry { return &fstree.Entry{Mode: unix.S_IFDIR | perm} }
	lnk := &fstree.Entry{Mode: unix.S_IFLNK | 0o777, LinkTarget: []byte("t")}
	type C = worktree.Change
	describe := []struct {
		name string
		ch   C
	}{
		{"added file", C{Path: "docs/new.txt", Kind: worktree.Added, New: file(0o644)}},
		{"added directory", C{Path: "sub", Kind: worktree.Added, New: dir(0o755)}},
		{"deleted file", C{Path: "old.txt", Kind: worktree.Deleted, Old: file(0o644)}},
		{"deleted directory", C{Path: "old", Kind: worktree.Deleted, Old: dir(0o755)}},
		{"modified file", C{Path: "src/main.go", Kind: worktree.Modified, Old: file(0o644), New: file(0o644)}},
		{"modified symlink", C{Path: "link", Kind: worktree.Modified, Old: lnk, New: lnk}},
		{"type file to directory", C{Path: "a.txt", Kind: worktree.TypeChanged, Old: file(0o644), New: dir(0o755)}},
		{"type directory to file", C{Path: "sub", Kind: worktree.TypeChanged, Old: dir(0o755), New: file(0o644)}},
		{"type file to symlink", C{Path: "bin/tool", Kind: worktree.TypeChanged, Old: file(0o755), New: lnk}},
		{"type unknown to zero", C{Path: "x", Kind: worktree.TypeChanged, Old: &fstree.Entry{Mode: 0o160644}, New: &fstree.Entry{}}},
		{"type fifo to socket", C{Path: "p", Kind: worktree.TypeChanged, Old: &fstree.Entry{Mode: unix.S_IFIFO | 0o600}, New: &fstree.Entry{Mode: unix.S_IFSOCK | 0o600}}},
		{"type char to block device", C{Path: "dev", Kind: worktree.TypeChanged, Old: &fstree.Entry{Mode: unix.S_IFCHR | 0o600}, New: &fstree.Entry{Mode: unix.S_IFBLK | 0o600}}},
		{"mode file", C{Path: "run.sh", Kind: worktree.ModeChanged, Old: file(0o644), New: file(0o755)}},
		{"mode directory", C{Path: "sub", Kind: worktree.ModeChanged, Old: dir(0o755), New: dir(0o700)}},
		{"mode setuid sticky", C{Path: "s", Kind: worktree.ModeChanged, Old: file(0o4755), New: file(0o1777)}},
		{"mode zero", C{Path: "z", Kind: worktree.ModeChanged, Old: file(0), New: file(0o7777)}},
		{"meta file", C{Path: "touched", Kind: worktree.MetaChanged, Old: file(0o644), New: file(0o644)}},
		{"meta directory", C{Path: "d", Kind: worktree.MetaChanged, Old: dir(0o755), New: dir(0o755)}},
		{"non-UTF-8 path", C{Path: "caf\xe9", Kind: worktree.Added, New: file(0o644)}},
		{"path with spaces and unicode", C{Path: "sp ace/é", Kind: worktree.Deleted, Old: dir(0o755)}},
	}
	for _, d := range describe {
		dc := wtDescribeCase{Name: d.name, Change: wtChange(d.ch)}
		dc.Out, dc.OutHex = wtText(wtDescribeChange(d.ch))
		v.DescribeChange = append(v.DescribeChange, dc)
		row := wtDescribeCase{Name: d.name, Change: wtChange(d.ch)}
		row.Out, row.OutHex = wtText(fmt.Sprintf("  %-9s %s\n", d.ch.Kind, wtDescribeChange(d.ch)))
		v.StatusRow = append(v.StatusRow, row)
	}
	for _, c := range []struct{ flag, stored, env string }{
		{"f", "s", "e"}, {"", "s", "e"}, {"", "", "e"}, {"", "", ""}, {" ", "s", "e"}, {"f", "", ""},
	} {
		out, err := wtResolveTicket(c.flag, c.stored, c.env)
		v.ResolveTicket = append(v.ResolveTicket, wtResolveTicketCase{Flag: c.flag, Stored: c.stored, Env: c.env, OK: err == nil, Out: out, Error: wtErrText(err, "")})
	}
	fp, err := wtFilterPathsVectors()
	if err != nil {
		return nil, err
	}
	v.FilterPaths = fp
	empty, _ := worktree.EmptyTree()
	ck, _, err := wtTestCommit(empty)
	if err != nil {
		return nil, err
	}
	blob := wtBlobKey(smData(1, 100))
	for _, c := range []struct {
		name      string
		key, tree key.Key
	}{
		{"tree", empty, empty},
		{"branch", ck, empty},
		{"absent reference", key.Key{}, key.Key{}},
		{"blob", blob, blob},
	} {
		v.FetchedDesc = append(v.FetchedDesc, wtFetchedDescCase{Name: c.name, Key: Hex(c.key[:]), Tree: Hex(c.tree[:]),
			Out: wtFetchedDesc(worktree.FetchResult{Key: c.key, Tree: c.tree})})
	}
	for _, c := range []struct {
		name         string
		root, commit key.Key
	}{
		{"plain tree", empty, key.Key{}},
		{"branch", empty, ck},
		{"commit field holding a tree", blob, empty},
	} {
		k := wtPushedKey(worktree.PushResult{Root: c.root, Commit: c.commit})
		v.PushedKey = append(v.PushedKey, wtPushedKeyCase{Name: c.name, Root: Hex(c.root[:]), Commit: Hex(c.commit[:]), Out: Hex(k[:])})
	}
	pf, err := wtPrintfVectors(ck)
	if err != nil {
		return nil, err
	}
	v.Printf = pf
	v.TicketFromView = wtTicketFromViewVectors()
	return v, nil
}

// wtFilterPathsVectors runs the filterPaths copy with the process working directory inside a temporary
// working-copy root (restored afterwards). {ROOT} in args and errors stands for that root.
func wtFilterPathsVectors() ([]wtFilterPathsCase, error) {
	tmp, done, err := wtTemp()
	if err != nil {
		return nil, err
	}
	defer done()
	root, err := filepath.EvalSymlinks(tmp)
	if err != nil {
		return nil, err
	}
	if err := os.Mkdir(filepath.Join(root, "sub"), 0o755); err != nil {
		return nil, err
	}
	orig, err := os.Getwd()
	if err != nil {
		return nil, err
	}
	defer func() { _ = os.Chdir(orig) }()
	paths := []string{"..foo", "a", "sub", "sub-b", "sub/x", "sub/y/z", "subx"}
	var changes []worktree.Change
	for _, p := range paths {
		changes = append(changes, worktree.Change{Path: p, Kind: worktree.Added, New: &fstree.Entry{Mode: unix.S_IFREG | 0o644}})
	}
	cases := []struct {
		name, cwd string
		args      []string
	}{
		{"dot", ".", []string{"."}},
		{"subdirectory", ".", []string{"sub"}},
		{"trailing slash", ".", []string{"sub/"}},
		{"unclean", ".", []string{"./sub/../sub"}},
		{"two paths keep change order", ".", []string{"sub/x", "a"}},
		{"absolute", ".", []string{"{ROOT}/sub"}},
		{"absolute root", ".", []string{"{ROOT}"}},
		{"dot-dot-prefixed name is inside", ".", []string{"..foo"}},
		{"no match", ".", []string{"nomatch"}},
		{"outside", ".", []string{"../outside"}},
		{"parent", ".", []string{".."}},
		{"filesystem root", ".", []string{"/"}},
		{"absolute outside", ".", []string{"{ROOT}/../other"}},
		{"second argument outside", ".", []string{"sub", "../x"}},
		{"from subdirectory: dot", "sub", []string{"."}},
		{"from subdirectory: parent", "sub", []string{".."}},
		{"from subdirectory: child", "sub", []string{"x"}},
		{"from subdirectory: outside", "sub", []string{"../.."}},
	}
	var out []wtFilterPathsCase
	for _, c := range cases {
		if err := os.Chdir(filepath.Join(root, c.cwd)); err != nil {
			return nil, err
		}
		args := make([]string, len(c.args))
		for i, a := range c.args {
			args[i] = strings.ReplaceAll(a, "{ROOT}", root)
		}
		kept, err := wtFilterPaths(root, changes, args)
		fc := wtFilterPathsCase{Name: c.name, Cwd: c.cwd, Args: c.args, Changes: paths, Error: wtErrText(err, root)}
		if kept != nil {
			fc.Kept = []string{}
		}
		for _, k := range kept {
			fc.Kept = append(fc.Kept, k.Path)
		}
		out = append(out, fc)
	}
	return out, nil
}

func wtPrintfVectors(ck key.Key) ([]wtPrintfCase, error) {
	key16 := "2001bbe6a9f5a014"
	commit16 := ck.String()[:16]
	empty, _ := worktree.EmptyTree()
	if empty.String()[:16] != key16 {
		return nil, fmt.Errorf("printf: the empty tree is %s", empty)
	}
	// What fetchedDesc gives the clone, init and fetch lines.
	rootDesc := wtFetchedDesc(worktree.FetchResult{Key: empty, Tree: empty})
	commitDesc := wtFetchedDesc(worktree.FetchResult{Key: ck, Tree: empty})
	cases := []struct {
		name, stream, fn, format string
		args                     []any
	}{
		{"clone", "stdout", "Printf", "cloned %s into %s: %s, %d objects fetched (%d bytes)\n", []any{"trees/demo", "demo", rootDesc, 12, int64(34567)}},
		{"clone-unicode", "stdout", "Printf", "cloned %s into %s: %s, %d objects fetched (%d bytes)\n", []any{"trees/é", "é/sub dir", rootDesc, 0, int64(0)}},
		{"clone-branch", "stdout", "Printf", "cloned %s into %s: %s, %d objects fetched (%d bytes)\n", []any{"trees/demo", "demo", commitDesc, 13, int64(34700)}},
		{"init-exists", "stdout", "Printf", "initialised working copy of %s; the reference exists (%s): status shows everything as new, pull merges\n", []any{"trees/demo", rootDesc}},
		{"init-exists-branch", "stdout", "Printf", "initialised working copy of %s; the reference exists (%s): status shows everything as new, pull merges\n", []any{"trees/demo", commitDesc}},
		{"init-absent", "stdout", "Printf", "initialised working copy of %s; the reference does not exist yet: push creates it\n", []any{"trees/new"}},
		{"fetch-absent", "stdout", "Printf", "%s does not exist on the cluster\n", []any{"trees/demo"}},
		{"fetch-up-to-date", "stdout", "Printf", "%s: up to date (%s)\n", []any{"trees/demo", key16}},
		{"fetch-up-to-date-branch", "stdout", "Printf", "%s: up to date (%s)\n", []any{"trees/demo", commit16}},
		{"fetch", "stdout", "Printf", "fetched %s: %s, %d objects fetched (%d bytes)\n", []any{"trees/demo", rootDesc, 3, int64(1 << 40)}},
		{"fetch-branch", "stdout", "Printf", "fetched %s: %s, %d objects fetched (%d bytes)\n", []any{"trees/demo", commitDesc, 4, int64(133)}},
		{"fetched-desc-commit", "stdout", "Sprintf", "commit %s, root %s", []any{commit16, key16}},
		{"pull-conflicts-header", "stderr", "Fprintln", "conflicts:", nil},
		{"pull-conflict-row", "stderr", "Fprintf", "  %s (local: %s, cluster: %s)\n", []any{"d/y", worktree.Deleted, worktree.Added}},
		{"pull-conflict-row-meta-type", "stderr", "Fprintf", "  %s (local: %s, cluster: %s)\n", []any{"a b", worktree.MetaChanged, worktree.TypeChanged}},
		{"pull-up-to-date", "stdout", "Println", "already up to date", nil},
		{"pull-applied", "stdout", "Printf", "pulled: %d paths updated", []any{3}},
		{"pull-conflicts-taken", "stdout", "Printf", ", %d conflicts taken from the cluster", []any{2}},
		{"pull-line-end", "stdout", "Println", "", nil},
		{"push-nothing", "stdout", "Println", "nothing to push", nil},
		{"push-recovered", "stdout", "Printf", "%s already holds %s (an earlier push completed); state updated\n", []any{"trees/demo", key16}},
		{"push-recovered-branch", "stdout", "Printf", "%s already holds %s (an earlier push completed); state updated\n", []any{"trees/demo", commit16}},
		{"push-commit", "stdout", "Printf", "pushed %s: commit %s, root %s, %d objects, %d uploaded, version %x\n", []any{"trees/demo", commit16, key16, 6, 3, []byte{0x01, 0xab, 0x00}}},
		{"push", "stdout", "Printf", "pushed %s: root %s, %d objects, %d uploaded, version %x\n", []any{"trees/demo", key16, 5, 2, []byte{0x01, 0xab, 0x00}}},
		{"push-empty-version", "stdout", "Printf", "pushed %s: root %s, %d objects, %d uploaded, version %x\n", []any{"trees/demo", key16, 0, 0, []byte{}}},
		{"status-header", "stdout", "Printf", "reference %s, synced to %s\n", []any{"trees/demo", key16}},
		{"status-remote-up-to-date", "stdout", "Println", "remote: up to date", nil},
		{"status-remote-absent", "stdout", "Println", "remote: the reference does not exist on the cluster", nil},
		{"status-remote-moved", "stdout", "Printf", "remote: moved since your last fetch (+%d ~%d -%d; run pull)\n", []any{1, 2, 0}},
		{"status-changes", "stdout", "Println", "changes:", nil},
		{"status-row", "stdout", "Printf", "  %-9s %s\n", []any{worktree.TypeChanged, "bin/tool (file → symlink)"}},
		{"status-row-deleted-directory", "stdout", "Printf", "  %-9s %s\n", []any{worktree.Deleted, "old/"}},
		{"status-meta-only", "stdout", "Printf", "%d paths differ only in mtime, ownership or xattrs\n", []any{1}},
		{"status-nothing", "stdout", "Println", "nothing to push", nil},
	}
	var out []wtPrintfCase
	for _, c := range cases {
		pc := wtPrintfCase{Name: c.name, Stream: c.stream, Func: c.fn, Format: c.format, Args: []wtPrintfArg{}}
		var text string
		switch c.fn {
		case "Printf", "Fprintf", "Sprintf":
			text = fmt.Sprintf(c.format, c.args...)
		case "Println", "Fprintln":
			if c.format == "" {
				text = fmt.Sprintln()
			} else {
				text = fmt.Sprintln(c.format)
			}
		default:
			return nil, fmt.Errorf("printf %s: unknown func %s", c.name, c.fn)
		}
		for _, a := range c.args {
			switch x := a.(type) {
			case string:
				pc.Args = append(pc.Args, wtPrintfArg{Type: "string", Value: x})
			case int:
				pc.Args = append(pc.Args, wtPrintfArg{Type: "int", Value: strconv.Itoa(x)})
			case int64:
				pc.Args = append(pc.Args, wtPrintfArg{Type: "int", Value: strconv.FormatInt(x, 10)})
			case []byte:
				pc.Args = append(pc.Args, wtPrintfArg{Type: "bytes", Value: hex.EncodeToString(x)})
			case worktree.Kind:
				pc.Args = append(pc.Args, wtPrintfArg{Type: "kind", Value: strconv.Itoa(int(x))})
			default:
				return nil, fmt.Errorf("printf %s: unsupported arg %T", c.name, a)
			}
		}
		pc.Out, pc.OutHex = wtText(text)
		out = append(out, pc)
	}
	return out, nil
}

func wtTicketFromViewVectors() []wtTicketFromViewCase {
	cid := make([]byte, 16)
	for i := range cid {
		cid[i] = byte(i)
	}
	id := func(seed uint64) []byte {
		return ed25519.NewKeyFromSeed(smData(seed, 32)).Public().(ed25519.PublicKey)
	}
	nodes := func(n int, addrs bool) []view.Node {
		var out []view.Node
		for i := 0; i < n; i++ {
			nd := view.Node{ID: id(uint64(1 + i)), Weight: 100, Writable: i%2 == 0}
			if addrs {
				nd.Addrs = []string{fmt.Sprintf("ip:127.0.0.1:%d", 4433+i), "relay:https://relay.example/"}
			}
			out = append(out, nd)
		}
		return out
	}
	cases := []struct {
		name string
		v    view.View
	}{
		{"no-nodes", view.View{ClusterID: cid, Incarnation: 1}},
		{"one-node-no-addrs", view.View{ClusterID: cid, Incarnation: 1, Nodes: nodes(1, false)}},
		{"one-node", view.View{ClusterID: cid, Incarnation: 2, Nodes: nodes(1, true)}},
		{"four-nodes", view.View{ClusterID: cid, Incarnation: 3, Nodes: nodes(4, true)}},
		{"five-nodes-capped-at-four", view.View{ClusterID: cid, Incarnation: 1<<64 - 1, Nodes: nodes(5, true)}},
	}
	var out []wtTicketFromViewCase
	for _, c := range cases {
		tc := wtTicketFromViewCase{Name: c.name, ClusterID: Hex(c.v.ClusterID), Incarnation: U64(c.v.Incarnation), Ticket: worktree.TicketFromView(&c.v).Encode()}
		for _, nd := range c.v.Nodes {
			tc.Nodes = append(tc.Nodes, wtViewNode{ID: Hex(nd.ID), Addrs: nd.Addrs})
		}
		out = append(out, tc)
	}
	return out
}

// ---- errors/worktree_text.json ----

type wtErrorCase struct {
	Name   string  `json:"name"`
	Out    *string `json:"out,omitempty"`
	OutHex *string `json:"out_hex,omitempty"`
}

type wtErrorsFile struct {
	Cases []wtErrorCase `json:"cases"`
}

func wtErrorVectors() (any, error) {
	var v wtErrorsFile
	add := func(name, out string) {
		c := wtErrorCase{Name: name}
		c.Out, c.OutHex = wtText(out)
		v.Cases = append(v.Cases, c)
	}
	addErr := func(name string, err error, root string) error {
		if err == nil {
			return fmt.Errorf("%s: no error", name)
		}
		add(name, *wtErrText(err, root))
		return nil
	}
	sentinels := []struct {
		name string
		err  error
	}{
		{"worktree/ErrNotWorkingCopy", worktree.ErrNotWorkingCopy},
		{"worktree/ErrIncomplete", worktree.ErrIncomplete},
		{"worktree/ErrNoRemote", worktree.ErrNoRemote},
		{"worktree/ErrRemoteMoved", worktree.ErrRemoteMoved},
		{"worktree/ErrRemoteDeleted", worktree.ErrRemoteDeleted},
		{"worktree/ErrConflict", worktree.ErrConflict},
		{"worktree/ErrRefChanged", worktree.ErrRefChanged},
		{"worktree/ErrTooLarge", worktree.ErrTooLarge},
		{"client/ErrUnknownRef", client.ErrUnknownRef},
	}
	for _, s := range sentinels {
		add(s.name, s.err.Error())
	}
	for _, s := range sentinels {
		add("cli-line/"+s.name, "dstore: "+s.err.Error()+"\n")
	}
	k1 := wtBlobKey(smData(1, 100))
	add("flow/push-ref-changed-absent", fmt.Errorf("%w (%v)", worktree.ErrRefChanged, &client.CASMismatch{Version: []byte{1, 2}}).Error())
	add("flow/push-ref-changed-current", fmt.Errorf("%w (%v)", worktree.ErrRefChanged, &client.CASMismatch{Current: k1[:], HasCurrent: true, Version: []byte{3}}).Error())
	add("flow/push-ref-changed-current-short-key", fmt.Errorf("%w (%v)", worktree.ErrRefChanged, &client.CASMismatch{Current: []byte{0xab, 0xcd}, HasCurrent: true}).Error())
	add("flow/clone-unknown-ref", fmt.Errorf("%w: %s", client.ErrUnknownRef, "trees/x").Error())
	add("cli-line/flow/push-ref-changed-absent", "dstore: "+fmt.Errorf("%w (%v)", worktree.ErrRefChanged, &client.CASMismatch{}).Error()+"\n")

	if err := wtTreeErrors(addErr); err != nil {
		return nil, err
	}
	if err := wtApplyErrors(addErr); err != nil {
		return nil, err
	}

	// cmd/dstore texts (wc.go literals, checked by the self-check).
	add("cmd/clone-usage", errors.New("clone NAME [DIR]").Error())
	add("cmd/init-usage", errors.New("init NAME").Error())
	add("cmd/remote-and-incoming", errors.New("--remote and --incoming exclude each other").Error())
	if _, err := wtResolveTicket("", "", ""); err != nil {
		add("cmd/resolve-ticket", err.Error())
	}
	for _, u := range []struct{ name, user string }{
		{"empty", ""}, {"invalid-utf8", "\xff"}, {"control", "a\x01"}, {"too-long", strings.Repeat("u", reference.MaxUserLen+1)},
	} {
		if err := reference.ValidateUser(u.user); err != nil {
			add("cmd/push-user-"+u.name, fmt.Errorf("user: %w", err).Error())
		}
	}
	for _, n := range []struct{ name, ref string }{
		{"empty", ""}, {"too-long", strings.Repeat("n", reference.MaxNameLen+1)}, {"invalid-utf8", "trees/\xff"}, {"at", "trees/a@b"}, {"control", "trees/a\tb"},
	} {
		if err := reference.ValidateName(n.ref); err != nil {
			add("cmd/reference-name-"+n.name, err.Error())
		}
	}
	return v, nil
}

// wtTreeErrors records the errors of Find, Open, Create, Init and Clone on real temporary directories.
func wtTreeErrors(addErr func(string, error, string) error) error {
	ctx := context.Background()
	cfg := worktree.Config{Ticket: "t", Name: "trees/x"}
	type step func(root string) error
	// wants pins the sentinel a case must produce. Find walks every ancestor of the temporary directory, so a
	// .dstore somewhere above $TMPDIR would otherwise change these texts without failing.
	wants := map[string]error{
		"tree/find-not-a-working-copy": worktree.ErrNotWorkingCopy,
		"tree/open-not-a-working-copy": worktree.ErrNotWorkingCopy,
		"tree/open-dstore-is-a-file":   worktree.ErrNotWorkingCopy,
		"tree/open-incomplete":         worktree.ErrIncomplete,
	}
	run := func(name string, prepare, act step) error {
		root, done, err := wtTemp()
		if err != nil {
			return err
		}
		defer done()
		if prepare != nil {
			if err := prepare(root); err != nil {
				return fmt.Errorf("%s: %w", name, err)
			}
		}
		aerr := act(root)
		if want := wants[name]; want != nil && !errors.Is(aerr, want) {
			return fmt.Errorf("%s: got %v, want %v (is there a .dstore above %s?)", name, aerr, want, root)
		}
		return addErr(name, aerr, root)
	}
	create := func(root string) error {
		tr, err := worktree.Create(root, cfg)
		if err != nil {
			return err
		}
		return tr.Close()
	}
	open := func(root string) error {
		t, err := worktree.Open(root)
		if err == nil {
			_ = t.Close()
		}
		return err
	}
	cases := []struct {
		name    string
		prepare step
		act     step
	}{
		{"tree/find-not-a-working-copy", nil, func(root string) error { _, err := worktree.Find(root); return err }},
		{"tree/open-not-a-working-copy", nil, open},
		{"tree/open-dstore-is-a-file", func(root string) error { return os.WriteFile(filepath.Join(root, worktree.Dir), nil, 0o644) }, open},
		{"tree/open-no-config", func(root string) error { return os.Mkdir(filepath.Join(root, worktree.Dir), 0o755) }, open},
		{"tree/open-config-is-a-directory", func(root string) error { return os.MkdirAll(filepath.Join(root, worktree.Dir, "config"), 0o755) }, open},
		{"tree/open-bad-config", func(root string) error {
			if err := os.Mkdir(filepath.Join(root, worktree.Dir), 0o755); err != nil {
				return err
			}
			return os.WriteFile(filepath.Join(root, worktree.Dir, "config"), []byte("x"), 0o644)
		}, open},
		{"tree/open-bad-config-type", func(root string) error {
			if err := os.Mkdir(filepath.Join(root, worktree.Dir), 0o755); err != nil {
				return err
			}
			return os.WriteFile(filepath.Join(root, worktree.Dir, "config"), []byte(`{"name":1}`), 0o644)
		}, open},
		{"tree/open-packstore-is-a-file", func(root string) error {
			if err := create(root); err != nil {
				return err
			}
			if err := os.RemoveAll(filepath.Join(root, worktree.Dir, "packstore")); err != nil {
				return err
			}
			return os.WriteFile(filepath.Join(root, worktree.Dir, "packstore"), nil, 0o644)
		}, open},
		{"tree/open-incomplete", create, open},
		{"tree/open-locked", func(root string) error {
			if err := create(root); err != nil {
				return err
			}
			return os.WriteFile(filepath.Join(root, worktree.Dir, "state"), []byte(`{"base":"2001bbe6a9f5a0146a1f4d0381e9b0ed1ac2f1a979ce9d5ad84e46ff0b58f36b","synced_at":"2023-11-14T22:13:20Z"}`), 0o644)
		}, func(root string) error {
			t, err := worktree.Open(root)
			if err != nil {
				return err
			}
			defer t.Close()
			return open(root)
		}},
		{"tree/open-locked-before-state-check", create, func(root string) error {
			// The copy has no state file: the lock is taken first, so Open reports the lock.
			st, err := packstore.Open(filepath.Join(root, worktree.Dir, "packstore"))
			if err != nil {
				return err
			}
			defer st.Close()
			return open(root)
		}},
		{"tree/create-over-existing", create, func(root string) error {
			tr, err := worktree.Create(root, cfg)
			if err == nil {
				_ = tr.Close()
			}
			return err
		}},
		{"tree/create-inside", func(root string) error {
			if err := create(root); err != nil {
				return err
			}
			return os.Mkdir(filepath.Join(root, "sub"), 0o755)
		}, func(root string) error {
			tr, err := worktree.Create(filepath.Join(root, "sub"), cfg)
			if err == nil {
				_ = tr.Close()
			}
			return err
		}},
		{"flow/init-inside", create, func(root string) error {
			tr, _, err := worktree.Init(ctx, nil, filepath.Join(root, "sub", "deeper"), cfg, nil)
			if err == nil {
				_ = tr.Close()
			}
			return err
		}},
		{"flow/clone-not-a-directory", func(root string) error { return os.WriteFile(filepath.Join(root, "file"), []byte("x"), 0o644) }, func(root string) error {
			_, _, err := worktree.Clone(ctx, nil, filepath.Join(root, "file"), cfg, nil)
			return err
		}},
		{"flow/clone-not-empty", func(root string) error { return os.WriteFile(filepath.Join(root, "x"), []byte("x"), 0o644) }, func(root string) error {
			_, _, err := worktree.Clone(ctx, nil, root, cfg, nil)
			return err
		}},
		{"flow/clone-parent-is-a-file", func(root string) error { return os.WriteFile(filepath.Join(root, "file"), []byte("x"), 0o644) }, func(root string) error {
			_, _, err := worktree.Clone(ctx, nil, filepath.Join(root, "file", "sub"), cfg, nil)
			return err
		}},
		{"flow/clone-empty-directory-inside", func(root string) error {
			if err := create(root); err != nil {
				return err
			}
			return os.Mkdir(filepath.Join(root, "empty"), 0o755)
		}, func(root string) error {
			_, _, err := worktree.Clone(ctx, nil, filepath.Join(root, "empty"), cfg, nil)
			return err
		}},
	}
	for _, c := range cases {
		if err := run(c.name, c.prepare, c.act); err != nil {
			return err
		}
	}
	return nil
}

// wtApplyErrors records Apply's refusals and wrapped errors on real temporary directories.
func wtApplyErrors(addErr func(string, error, string) error) error {
	blob, err := fstree.EncodeBlob([]byte("content\n"))
	if err != nil {
		return err
	}
	get := func(k key.Key) ([]byte, error) {
		if k == blob.Key {
			return blob.Bytes, nil
		}
		return nil, packstore.ErrNotFound
	}
	file := func(mode uint64) *fstree.Entry {
		return &fstree.Entry{Name: []byte("x"), Mode: unix.S_IFREG | mode, ContentKey: blob.Key[:], Mtime: wtMt(wtT0, 0)}
	}
	missing := wtBlobKey([]byte("missing content\n"))
	var reserved [32]byte
	reserved[0] = 0x08
	type C = worktree.Change
	cases := []struct {
		name    string
		prepare func(root string) error
		changes []C
	}{
		{"apply/empty-path", nil, []C{{Path: "", Kind: worktree.Added, New: file(0o644)}}},
		{"apply/unsafe-parent", nil, []C{{Path: "../x", Kind: worktree.Added, New: file(0o644)}}},
		{"apply/unsafe-inner-parent", nil, []C{{Path: "a/../x", Kind: worktree.Added, New: file(0o644)}}},
		{"apply/unsafe-empty-component", nil, []C{{Path: "a//x", Kind: worktree.Added, New: file(0o644)}}},
		{"apply/unsafe-dot", nil, []C{{Path: "./x", Kind: worktree.Added, New: file(0o644)}}},
		{"apply/unsafe-trailing-slash", nil, []C{{Path: "x/", Kind: worktree.Added, New: file(0o644)}}},
		{"apply/unsafe-leading-slash", nil, []C{{Path: "/x", Kind: worktree.Added, New: file(0o644)}}},
		{"apply/unsafe-trailing-dot", nil, []C{{Path: "a/.", Kind: worktree.Deleted, Old: file(0o644)}}},
		{"apply/unsafe-quoted-bytes", nil, []C{{Path: "\xff\t\"\\\u00a0😀/../x", Kind: worktree.Added, New: file(0o644)}}},
		{"apply/unsafe-checked-before-writing", nil, []C{{Path: "ok", Kind: worktree.Added, New: file(0o644)}, {Path: "..", Kind: worktree.Deleted, Old: file(0o644)}}},
		{"apply/symlinked-ancestor", func(root string) error { return os.Symlink(os.TempDir(), filepath.Join(root, "lnk")) },
			[]C{{Path: "lnk/f", Kind: worktree.Added, New: file(0o644)}}},
		{"apply/file-ancestor", func(root string) error { return os.WriteFile(filepath.Join(root, "f"), []byte("x"), 0o644) },
			[]C{{Path: "f/g", Kind: worktree.Added, New: file(0o644)}}},
		{"apply/unsupported-type", nil, []C{{Path: "x", Kind: worktree.Added, New: &fstree.Entry{Name: []byte("x"), Mode: 0o160644}}}},
		{"apply/unsupported-type-zero", nil, []C{{Path: "x", Kind: worktree.Added, New: &fstree.Entry{Name: []byte("x")}}}},
		{"apply/bad-content-key", nil, []C{{Path: "x", Kind: worktree.Added, New: &fstree.Entry{Name: []byte("x"), Mode: unix.S_IFREG | 0o644, ContentKey: []byte{1, 2, 3}}}}},
		{"apply/missing-content", nil, []C{{Path: "x", Kind: worktree.Added, New: &fstree.Entry{Name: []byte("x"), Mode: unix.S_IFREG | 0o644, ContentKey: missing[:]}}}},
		{"apply/bad-inline-xattrs", nil, []C{{Path: "x", Kind: worktree.Added, New: func() *fstree.Entry { e := file(0o644); e.XattrsIn = []byte{0xa1}; return e }()}}},
		{"apply/inline-xattrs-not-a-map", nil, []C{{Path: "x", Kind: worktree.Added, New: func() *fstree.Entry { e := file(0o644); e.XattrsIn = []byte{0x80}; return e }()}}},
		{"apply/missing-xattr-set", nil, []C{{Path: "x", Kind: worktree.Added, New: func() *fstree.Entry { e := file(0o644); e.XattrsKey = missing[:]; return e }()}}},
		{"apply/bad-xattr-set-key", nil, []C{{Path: "x", Kind: worktree.Added, New: func() *fstree.Entry { e := file(0o644); e.XattrsKey = reserved[:]; return e }()}}},
	}
	for _, c := range cases {
		root, done, err := wtTemp()
		if err != nil {
			return err
		}
		if c.prepare != nil {
			if err := c.prepare(root); err != nil {
				done()
				return fmt.Errorf("%s: %w", c.name, err)
			}
		}
		aerr := worktree.Apply(root, c.changes, get)
		err = addErr(c.name, aerr, root)
		done()
		if err != nil {
			return err
		}
	}
	return nil
}
