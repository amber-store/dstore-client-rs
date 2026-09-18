package main

// Family client: the client-package vectors of dstore v0.1.9 (client-core §5,
// client-transfer §5.2 items 3-5 and 8, verification §4.3 item 22):
//
//	client/rank.json                rankOwners and rttClass scenarios
//	client/batches.json             put batching (batches)
//	client/fetch.json               estSize and pickBatch
//	client/verify_record.json       VerifyRecord outcomes
//	client/progress.json            tracker reports, HumanBytes, Rate, pathAttrs
//	client/backoff.json             handleErr backoff and the watch reconnect delays
//	client/placement_decisions.json Owners, WriteSet, ReadOrder, Primary, Placed through Dial
//	errors/client_text.json         client error texts
//
// Unexported Go functions are copied verbatim below. Before generating,
// clientSelfCheck parses the dstore v0.1.9 sources and this file and compares
// every copy with its original (comments dropped, identifiers renamed per
// clientRenames), so a copy that drifts fails the run. Schemas:
// docs/vectorgen-client.md.

import (
	"bytes"
	"context"
	"crypto/ed25519"
	_ "embed"
	"encoding/binary"
	"errors"
	"fmt"
	"go/ast"
	"go/parser"
	"go/printer"
	"go/token"
	"hash/crc32"
	"io"
	"log/slog"
	"math"
	"os"
	"os/exec"
	"path/filepath"
	"runtime/debug"
	"sort"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/amber-store/core/amberpack"
	"github.com/amber-store/core/fstree"
	"github.com/amber-store/core/ingest"
	"github.com/amber-store/core/key"
	"github.com/amber-store/core/packstore"
	"github.com/amber-store/dstore/client"
	"github.com/amber-store/dstore/ticket"
	"github.com/amber-store/dstore/transport"
	"github.com/amber-store/dstore/view"
	"github.com/amber-store/dstore/wire"
	"github.com/amber-store/transport-iroh/protocol"
)

// clientOutput is one file of the family.
type clientOutput struct {
	rel string
	gen func() (any, error)
}

// clientOutputs lists the family's files (clientAllOutputs, at the end of the file).
var clientOutputs = clientAllOutputs()

func init() {
	owns := make([]string, len(clientOutputs))
	for i, o := range clientOutputs {
		owns[i] = o.rel
	}
	register("client", owns, genClient)
}

func genClient(out string) error {
	if err := clientSelfCheck(); err != nil {
		return err
	}
	for _, o := range clientOutputs {
		v, err := o.gen()
		if err != nil {
			return fmt.Errorf("%s: %w", o.rel, err)
		}
		if err := writeJSON(filepath.Join(out, filepath.FromSlash(o.rel)), v); err != nil {
			return err
		}
	}
	return nil
}

// ---- self-check of the verbatim copies ----

//go:embed family_client.go
var clientSelfSource []byte

const clientDstorePath, clientDstoreVersion = "github.com/amber-store/dstore", "v0.1.9"

// clientRenames maps identifiers of the dstore sources to the names the copies
// use in package main, where the originals would be too generic or would
// need the client package's own types.
var clientRenames = map[string]string{
	"Cluster":        "clientStubCluster",
	"Progress":       "clientProgress",
	"ProgressReport": "clientProgressReport",
	"NodeProgress":   "clientNodeProgress",
	"PutObserver":    "clientPutObserver",
	"RecordSizer":    "clientRecordSizer",
}

// clientCopy names a declaration of the dstore sources copied into this file.
type clientCopy struct {
	file string // slash path inside the dstore module
	recv string // receiver type of a method, e.g. "*tracker"; "" otherwise
	name string // declared name in dstore
}

var clientCopies = []clientCopy{
	{"client/rank.go", "", "rttClass"},
	{"client/rank.go", "", "rankOwners"},
	{"client/batch.go", "", "batches"},
}

// clientStmtCopy is a copy of consecutive statements of a dstore function:
// the first n statements of the copy function's body must appear, in order,
// in one block of the original.
type clientStmtCopy struct {
	file, recv, name string // the original function
	copyName         string // the copy function in this file
	n                int
}

var clientStmtCopies []clientStmtCopy

// clientLiteral is a format string or message used by a vector, which must
// occur as a string literal in the named dstore function.
type clientLiteral struct {
	file, recv, name string
	lit              string
}

var clientLiterals []clientLiteral

// clientExpr is source text a vector relies on (a timing expression or
// statement), which must occur verbatim in the named dstore function.
type clientExpr struct {
	file, recv, name string
	expr             string
}

var clientExprs []clientExpr

// clientModuleDir returns the directory of the dstore module the generator is
// built against, after checking that it is v0.1.9 without a replace.
func clientModuleDir() (string, error) {
	bi, ok := debug.ReadBuildInfo()
	if !ok {
		return "", errors.New("self-check: no build info")
	}
	found := false
	for _, d := range bi.Deps {
		if d.Path == clientDstorePath {
			if d.Version != clientDstoreVersion || d.Replace != nil {
				return "", fmt.Errorf("self-check: built against %s %s (replace %v), want %s", d.Path, d.Version, d.Replace, clientDstoreVersion)
			}
			found = true
		}
	}
	if !found {
		return "", fmt.Errorf("self-check: %s is not a dependency of this build", clientDstorePath)
	}
	cmd := exec.Command("go", "list", "-m", "-f", "{{.Dir}}", clientDstorePath)
	var stderr bytes.Buffer
	cmd.Stderr = &stderr
	b, err := cmd.Output()
	if err != nil {
		return "", fmt.Errorf("self-check: go list -m %s: %v: %s", clientDstorePath, err, stderr.String())
	}
	dir := strings.TrimSpace(string(b))
	if dir == "" {
		return "", fmt.Errorf("self-check: go list -m %s printed no directory", clientDstorePath)
	}
	return dir, nil
}

// clientSources parses Go files once per self-check.
type clientSources struct {
	dir   string
	fset  *token.FileSet
	files map[string]*ast.File
	self  *ast.File
}

func (s *clientSources) file(rel string) (*ast.File, error) {
	if f := s.files[rel]; f != nil {
		return f, nil
	}
	f, err := parser.ParseFile(s.fset, filepath.Join(s.dir, filepath.FromSlash(rel)), nil, parser.SkipObjectResolution)
	if err != nil {
		return nil, fmt.Errorf("self-check: %v", err)
	}
	s.files[rel] = f
	return f, nil
}

// clientRecv renders a method receiver type ("*tracker", "tracker").
func clientRecv(fd *ast.FuncDecl) string {
	if fd.Recv == nil || len(fd.Recv.List) == 0 {
		return ""
	}
	switch t := fd.Recv.List[0].Type.(type) {
	case *ast.StarExpr:
		if id, ok := t.X.(*ast.Ident); ok {
			return "*" + id.Name
		}
	case *ast.Ident:
		return t.Name
	}
	return "?"
}

// clientFindDecl returns the function, type spec or single-name value spec
// declared under name (with the receiver recv for methods).
func clientFindDecl(f *ast.File, recv, name string) ast.Node {
	for _, d := range f.Decls {
		switch d := d.(type) {
		case *ast.FuncDecl:
			if d.Name.Name == name && clientRecv(d) == recv {
				return d
			}
		case *ast.GenDecl:
			if recv != "" {
				continue
			}
			for _, sp := range d.Specs {
				switch sp := sp.(type) {
				case *ast.TypeSpec:
					if sp.Name.Name == name {
						return sp
					}
				case *ast.ValueSpec:
					if len(sp.Names) == 1 && sp.Names[0].Name == name {
						return sp
					}
				}
			}
		}
	}
	return nil
}

// clientStrip removes every doc and line comment reachable from n, so that
// printing compares code only.
func clientStrip(n ast.Node) {
	ast.Inspect(n, func(x ast.Node) bool {
		switch x := x.(type) {
		case *ast.FuncDecl:
			x.Doc = nil
		case *ast.GenDecl:
			x.Doc = nil
		case *ast.TypeSpec:
			x.Doc, x.Comment = nil, nil
		case *ast.ValueSpec:
			x.Doc, x.Comment = nil, nil
		case *ast.Field:
			x.Doc, x.Comment = nil, nil
		}
		return true
	})
}

// clientRename renames identifiers of n in place.
func clientRename(n ast.Node, m map[string]string) {
	ast.Inspect(n, func(x ast.Node) bool {
		if id, ok := x.(*ast.Ident); ok {
			if to, ok := m[id.Name]; ok {
				id.Name = to
			}
		}
		return true
	})
}

func clientPrint(fset *token.FileSet, n any) (string, error) {
	var buf bytes.Buffer
	cfg := printer.Config{Mode: printer.UseSpaces | printer.TabIndent, Tabwidth: 8}
	if err := cfg.Fprint(&buf, fset, n); err != nil {
		return "", err
	}
	return buf.String(), nil
}

// clientFunc finds a function of a dstore file.
func (s *clientSources) function(rel, recv, name string) (*ast.FuncDecl, error) {
	f, err := s.file(rel)
	if err != nil {
		return nil, err
	}
	fd, ok := clientFindDecl(f, recv, name).(*ast.FuncDecl)
	if !ok {
		return nil, fmt.Errorf("self-check: %s has no function %s%s", rel, clientRecvPrefix(recv), name)
	}
	return fd, nil
}

func clientRecvPrefix(recv string) string {
	if recv == "" {
		return ""
	}
	return "(" + recv + ")."
}

func clientSelfCheck() error {
	dir, err := clientModuleDir()
	if err != nil {
		return err
	}
	s := &clientSources{dir: dir, fset: token.NewFileSet(), files: map[string]*ast.File{}}
	s.self, err = parser.ParseFile(s.fset, "family_client.go", clientSelfSource, parser.SkipObjectResolution)
	if err != nil {
		return fmt.Errorf("self-check: %v", err)
	}
	for _, c := range clientCopies {
		f, err := s.file(c.file)
		if err != nil {
			return err
		}
		orig := clientFindDecl(f, c.recv, c.name)
		if orig == nil {
			return fmt.Errorf("self-check: %s declares no %s%s", c.file, clientRecvPrefix(c.recv), c.name)
		}
		copyRecv, copyName := c.recv, c.name
		if to, ok := clientRenames[strings.TrimPrefix(c.recv, "*")]; ok {
			copyRecv = strings.Replace(c.recv, strings.TrimPrefix(c.recv, "*"), to, 1)
		}
		if to, ok := clientRenames[c.name]; ok {
			copyName = to
		}
		cp := clientFindDecl(s.self, copyRecv, copyName)
		if cp == nil {
			return fmt.Errorf("self-check: family_client.go has no copy of %s %s%s", c.file, clientRecvPrefix(c.recv), c.name)
		}
		clientStrip(orig)
		clientStrip(cp)
		clientRename(orig, clientRenames)
		want, err := clientPrint(s.fset, orig)
		if err != nil {
			return err
		}
		got, err := clientPrint(s.fset, cp)
		if err != nil {
			return err
		}
		if got != want {
			return fmt.Errorf("self-check: the copy of %s %s%s differs from dstore %s:\n--- dstore (renamed)\n%s\n--- family_client.go\n%s", c.file, clientRecvPrefix(c.recv), c.name, clientDstoreVersion, want, got)
		}
	}
	for _, c := range clientStmtCopies {
		orig, err := s.function(c.file, c.recv, c.name)
		if err != nil {
			return err
		}
		cp, ok := clientFindDecl(s.self, "", c.copyName).(*ast.FuncDecl)
		if !ok || len(cp.Body.List) < c.n {
			return fmt.Errorf("self-check: family_client.go has no function %s with %d statements", c.copyName, c.n)
		}
		want := make([]string, c.n)
		for i := range want {
			if want[i], err = clientPrint(s.fset, cp.Body.List[i]); err != nil {
				return err
			}
		}
		found := false
		ast.Inspect(orig.Body, func(x ast.Node) bool {
			if found {
				return false
			}
			blk, ok := x.(*ast.BlockStmt)
			if !ok {
				return true
			}
			for start := 0; start+c.n <= len(blk.List); start++ {
				match := true
				for i := 0; i < c.n && match; i++ {
					got, perr := clientPrint(s.fset, blk.List[start+i])
					match = perr == nil && got == want[i]
				}
				if match {
					found = true
					return false
				}
			}
			return true
		})
		if !found {
			return fmt.Errorf("self-check: the first %d statements of %s do not occur in dstore %s %s%s:\n%s", c.n, c.copyName, c.file, clientRecvPrefix(c.recv), c.name, strings.Join(want, "\n"))
		}
	}
	for _, c := range clientLiterals {
		f, err := s.file(c.file)
		if err != nil {
			return err
		}
		decl := clientFindDecl(f, c.recv, c.name)
		if decl == nil {
			return fmt.Errorf("self-check: %s declares no %s%s", c.file, clientRecvPrefix(c.recv), c.name)
		}
		found := false
		ast.Inspect(decl, func(x ast.Node) bool {
			if bl, ok := x.(*ast.BasicLit); ok && bl.Kind == token.STRING {
				if v, err := strconv.Unquote(bl.Value); err == nil && v == c.lit {
					found = true
				}
			}
			return !found
		})
		if !found {
			return fmt.Errorf("self-check: dstore %s %s%s has no string literal %q", c.file, clientRecvPrefix(c.recv), c.name, c.lit)
		}
	}
	for _, c := range clientExprs {
		fd, err := s.function(c.file, c.recv, c.name)
		if err != nil {
			return err
		}
		src, err := os.ReadFile(filepath.Join(s.dir, filepath.FromSlash(c.file)))
		if err != nil {
			return fmt.Errorf("self-check: %v", err)
		}
		from, to := s.fset.Position(fd.Pos()).Offset, s.fset.Position(fd.End()).Offset
		if from < 0 || to > len(src) || from > to || !bytes.Contains(src[from:to], []byte(c.expr)) {
			return fmt.Errorf("self-check: dstore %s %s%s does not contain %s", c.file, clientRecvPrefix(c.recv), c.name, c.expr)
		}
	}
	return nil
}

// ---- helpers ----

// clientPathJSON is a transport.PathInfo; null in a vector means no live
// connection (pool.Path returned false).
type clientPathJSON struct {
	Direct bool `json:"direct"`
	RTTNs  I64  `json:"rtt_ns"`
}

func clientPathOf(p *transport.PathInfo) *clientPathJSON {
	if p == nil {
		return nil
	}
	return &clientPathJSON{Direct: p.Direct, RTTNs: I64(p.RTT)}
}

// clientIndexKey is the 32-byte key whose first 8 bytes are i little-endian.
func clientIndexKey(i int) [32]byte {
	var k [32]byte
	for b := 0; b < 8; b++ {
		k[b] = byte(uint64(i) >> (8 * b))
	}
	return k
}

// ---- client/rank.json ----

// rttClass is a verbatim copy of dstore v0.1.9 client/rank.go rttClass.
func rttClass(rtt time.Duration) int {
	switch {
	case rtt < 5*time.Millisecond:
		return 0
	case rtt < 25*time.Millisecond:
		return 1
	case rtt < 100*time.Millisecond:
		return 2
	default:
		return 3
	}
}

// rankOwners is a verbatim copy of dstore v0.1.9 client/rank.go rankOwners.
func rankOwners(ids []view.NodeID, penalty func(view.NodeID) int, path func(view.NodeID) (transport.PathInfo, bool)) []view.NodeID {
	type scored struct {
		id    view.NodeID
		pen   int
		relay int
		class int
		pos   int
	}
	out := make([]scored, len(ids))
	for i, id := range ids {
		s := scored{id: id, pos: i, pen: penalty(id)}
		if p, ok := path(id); ok {
			if !p.Direct {
				s.relay = 1
			}
			s.class = rttClass(p.RTT)
		}
		out[i] = s
	}
	sort.SliceStable(out, func(i, j int) bool {
		a, b := out[i], out[j]
		if a.pen != b.pen {
			return a.pen < b.pen
		}
		if a.relay != b.relay {
			return a.relay < b.relay
		}
		if a.class != b.class {
			return a.class < b.class
		}
		return a.pos < b.pos
	})
	ranked := make([]view.NodeID, len(out))
	for i, s := range out {
		ranked[i] = s.id
	}
	return ranked
}

type clientRankOwner struct {
	ID      Hex             `json:"id"`
	Penalty int             `json:"penalty"`
	Path    *clientPathJSON `json:"path"`
}

type clientRankCase struct {
	Name   string            `json:"name"`
	Owners []clientRankOwner `json:"owners"`
	Want   []int             `json:"want"`
}

type clientRTTClassCase struct {
	RTTNs I64 `json:"rtt_ns"`
	Class int `json:"class"`
}

type clientRankFile struct {
	RTTClass   []clientRTTClassCase `json:"rtt_class"`
	RankOwners []clientRankCase     `json:"rank_owners"`
}

// clientOwner is one owner of a rank scenario.
type clientOwner struct {
	pen  int
	path *transport.PathInfo
}

func clientDirect(rtt time.Duration) *transport.PathInfo {
	return &transport.PathInfo{Direct: true, RTT: rtt}
}

func clientRelayed(rtt time.Duration) *transport.PathInfo {
	return &transport.PathInfo{Direct: false, RTT: rtt}
}

// clientRankScenario ranks owners whose ids are rank_test.go's ownerIDs
// (id[0] = i+1), with id[31] = tag to tell scenarios apart.
func clientRankScenario(name string, tag byte, owners []clientOwner) clientRankCase {
	ids := make([]view.NodeID, len(owners))
	pen := map[view.NodeID]int{}
	paths := map[view.NodeID]transport.PathInfo{}
	c := clientRankCase{Name: name, Owners: []clientRankOwner{}, Want: []int{}}
	for i, o := range owners {
		ids[i][0] = byte(i + 1)
		ids[i][31] = tag
		pen[ids[i]] = o.pen
		if o.path != nil {
			paths[ids[i]] = *o.path
		}
		c.Owners = append(c.Owners, clientRankOwner{ID: Hex(ids[i][:]), Penalty: o.pen, Path: clientPathOf(o.path)})
	}
	got := rankOwners(ids, func(id view.NodeID) int { return pen[id] }, func(id view.NodeID) (transport.PathInfo, bool) {
		p, ok := paths[id]
		return p, ok
	})
	for _, id := range got {
		c.Want = append(c.Want, int(id[0])-1)
	}
	return c
}

func genClientRank() (any, error) {
	var f clientRankFile
	for _, d := range []time.Duration{
		0, 1, 400 * time.Microsecond, time.Millisecond, 4999 * time.Microsecond, 5*time.Millisecond - 1, 5 * time.Millisecond, 5*time.Millisecond + 1,
		24*time.Millisecond + 999*time.Microsecond, 25*time.Millisecond - 1, 25 * time.Millisecond,
		99*time.Millisecond + 999*time.Microsecond, 100*time.Millisecond - 1, 100 * time.Millisecond, 333 * time.Millisecond,
		time.Second, time.Hour, time.Duration(1<<63 - 1),
	} {
		f.RTTClass = append(f.RTTClass, clientRTTClassCase{RTTNs: I64(d), Class: rttClass(d)})
	}

	ms := time.Millisecond
	add := func(c clientRankCase, want ...int) error {
		if want != nil && fmt.Sprint(c.Want) != fmt.Sprint(want) {
			return fmt.Errorf("rank_test.go %s: got %v, want %v", c.Name, c.Want, want)
		}
		f.RankOwners = append(f.RankOwners, c)
		return nil
	}
	// The 7 cases of client/rank_test.go, checked against the test's expectations.
	if err := add(clientRankScenario("keeps_rank_among_unmeasured_owners", 1, []clientOwner{{}, {}, {}}), 0, 1, 2); err != nil {
		return nil, err
	}
	if err := add(clientRankScenario("does_not_demote_unmeasured_owners_behind_a_measured_one", 2,
		[]clientOwner{{}, {}, {path: clientDirect(ms)}}), 0, 1, 2); err != nil {
		return nil, err
	}
	if err := add(clientRankScenario("prefers_direct_over_relayed", 3,
		[]clientOwner{{path: clientRelayed(ms)}, {path: clientDirect(2 * ms)}}), 1, 0); err != nil {
		return nil, err
	}
	if err := add(clientRankScenario("prefers_unmeasured_over_relayed", 4,
		[]clientOwner{{path: clientRelayed(ms)}, {}}), 1, 0); err != nil {
		return nil, err
	}
	if err := add(clientRankScenario("ties_near_round_trips_by_rank", 5,
		[]clientOwner{{path: clientDirect(900 * time.Microsecond)}, {path: clientDirect(400 * time.Microsecond)}}), 0, 1); err != nil {
		return nil, err
	}
	if err := add(clientRankScenario("prefers_a_much_nearer_owner", 6,
		[]clientOwner{{path: clientDirect(60 * ms)}, {path: clientDirect(2 * ms)}}), 1, 0); err != nil {
		return nil, err
	}
	if err := add(clientRankScenario("puts_penalised_owners_last", 7,
		[]clientOwner{{pen: 2}, {}, {}}), 1, 2, 0); err != nil {
		return nil, err
	}

	// Further scenarios (client-core §5 item 7, verification §5).
	more := []clientRankCase{
		clientRankScenario("class_boundaries", 8, []clientOwner{
			{path: clientDirect(100 * ms)}, {path: clientDirect(100*ms - 1)}, {path: clientDirect(25 * ms)},
			{path: clientDirect(25*ms - 1)}, {path: clientDirect(5 * ms)}, {path: clientDirect(5*ms - 1)},
		}),
		clientRankScenario("penalties_one_two_three", 9, []clientOwner{{pen: 3}, {pen: 2}, {pen: 1}, {pen: 0}}),
		clientRankScenario("hint_before_backoff", 10, []clientOwner{{pen: 2}, {pen: 1}}),
		clientRankScenario("penalty_beats_path", 11, []clientOwner{{pen: 1, path: clientDirect(ms)}, {pen: 0, path: clientRelayed(300 * ms)}}),
		clientRankScenario("relayed_near_after_direct_far", 12, []clientOwner{{path: clientRelayed(ms)}, {path: clientDirect(200 * ms)}}),
		clientRankScenario("unmeasured_before_direct_far", 13, []clientOwner{{path: clientDirect(150 * ms)}, {}}),
		clientRankScenario("zero_rtt_counts_as_near", 14, []clientOwner{{path: clientDirect(30 * ms)}, {path: clientDirect(0)}}),
		clientRankScenario("relayed_by_class", 15, []clientOwner{{path: clientRelayed(50 * ms)}, {path: clientRelayed(0)}, {}}),
		clientRankScenario("mem_path_ties_unmeasured", 16, []clientOwner{{}, {path: clientDirect(ms)}, {}}),
		clientRankScenario("single_owner", 17, []clientOwner{{pen: 3, path: clientRelayed(time.Second)}}),
		clientRankScenario("no_owners", 18, nil),
	}
	f.RankOwners = append(f.RankOwners, more...)

	// Pseudo-random scenarios from splitmix64.
	rtts := []time.Duration{0, 400 * time.Microsecond, ms, 5*ms - 1, 5 * ms, 12 * ms, 25*ms - 1, 25 * ms, 60 * ms, 100*ms - 1, 100 * ms, 250 * ms, 2 * time.Second}
	rnd := u64s(0x52414e4b, 2000)
	next := func() uint64 {
		v := rnd[0]
		rnd = rnd[1:]
		return v
	}
	for i := 0; i < 48; i++ {
		n := 1 + int(next()%8)
		owners := make([]clientOwner, n)
		for j := range owners {
			owners[j].pen = int(next() % 4)
			if next()%3 != 0 {
				p := transport.PathInfo{Direct: next()%2 == 0, RTT: rtts[next()%uint64(len(rtts))]}
				owners[j].path = &p
			}
		}
		f.RankOwners = append(f.RankOwners, clientRankScenario(fmt.Sprintf("splitmix_%02d", i), byte(0x40+i), owners))
	}
	return f, nil
}

// ---- client/batches.json ----

// clientRecordSizer stands for client.RecordSizer in the copy of batches.
type clientRecordSizer = func(k [32]byte) int

// The alias must keep client.RecordSizer's signature.
var _ clientRecordSizer = client.RecordSizer(nil)

// batches is a verbatim copy of dstore v0.1.9 client/batch.go batches
// (RecordSizer renamed).
func batches(keys [][32]byte, size clientRecordSizer, maxBytes, maxKeys int) [][][32]byte {
	var out [][][32]byte
	var cur [][32]byte
	bytes := 0
	for _, k := range keys {
		n := size(k)
		if len(cur) > 0 && (bytes+n > maxBytes || len(cur) >= maxKeys) {
			out = append(out, cur)
			cur, bytes = nil, 0
		}
		cur = append(cur, k)
		bytes += n
	}
	if len(cur) > 0 {
		out = append(out, cur)
	}
	return out
}

// clientSizeRun is a run of count consecutive keys of one record size.
type clientSizeRun struct {
	Size  int `json:"size"`
	Count int `json:"count"`
}

type clientBatchCase struct {
	Name     string          `json:"name"`
	Sizes    []clientSizeRun `json:"sizes"`
	MaxBytes int             `json:"max_bytes"`
	MaxKeys  int             `json:"max_keys"`
	WantLens []int           `json:"want_lens"`
}

type clientBatchesFile struct {
	DefaultBatchBytes int               `json:"default_batch_bytes"`
	BatchKeys         int               `json:"batch_keys"`
	MaxPutBatch       int               `json:"max_put_batch"`
	Cases             []clientBatchCase `json:"cases"`
}

// clientRuns expands sizes into per-key sizes.
func clientRuns(sizes []clientSizeRun) []int {
	var out []int
	for _, r := range sizes {
		for i := 0; i < r.Count; i++ {
			out = append(out, r.Size)
		}
	}
	return out
}

func clientBatchScenario(name string, sizes []clientSizeRun, maxBytes, maxKeys int) (clientBatchCase, error) {
	per := clientRuns(sizes)
	keys := make([][32]byte, len(per))
	size := map[[32]byte]int{}
	for i, n := range per {
		keys[i] = clientIndexKey(i)
		size[keys[i]] = n
	}
	got := batches(keys, func(k [32]byte) int { return size[k] }, maxBytes, maxKeys)
	c := clientBatchCase{Name: name, Sizes: sizes, MaxBytes: maxBytes, MaxKeys: maxKeys, WantLens: []int{}}
	if c.Sizes == nil {
		c.Sizes = []clientSizeRun{}
	}
	i := 0
	for _, b := range got {
		c.WantLens = append(c.WantLens, len(b))
		for _, k := range b {
			if k != keys[i] {
				return c, fmt.Errorf("batches %s: order changed at key %d", name, i)
			}
			i++
		}
	}
	if i != len(keys) {
		return c, fmt.Errorf("batches %s: %d keys out, %d in", name, i, len(keys))
	}
	return c, nil
}

func genClientBatches() (any, error) {
	f := clientBatchesFile{DefaultBatchBytes: 16 << 20, BatchKeys: 8192, MaxPutBatch: 64 << 20}
	run := func(size, count int) clientSizeRun { return clientSizeRun{Size: size, Count: count} }
	type sc struct {
		name     string
		sizes    []clientSizeRun
		maxBytes int
		maxKeys  int
		want     []int // checked when non-nil: the client/batch_test.go expectations
	}
	scs := []sc{
		{"balances_by_sizer_not_by_key_length", []clientSizeRun{run(10, 5)}, 25, 100, []int{2, 2, 1}},
		{"caps_keys_per_batch", []clientSizeRun{run(1, 7)}, 1 << 20, 3, []int{3, 3, 1}},
		{"sends_an_oversized_record_alone", []clientSizeRun{run(5, 1), run(100, 1), run(5, 1)}, 20, 100, []int{1, 1, 1}},
		{"keeps_order", []clientSizeRun{run(1, 4)}, 2, 100, []int{2, 2}},
		{"exact_fit_stays", []clientSizeRun{run(10, 1), run(15, 1), run(1, 1)}, 25, 100, nil},
		{"one_byte_over", []clientSizeRun{run(10, 1), run(16, 1)}, 25, 100, nil},
		{"key_cap_8193", []clientSizeRun{run(1, 8193)}, 16 << 20, 8192, nil},
		{"mib_records_16mib_target", []clientSizeRun{run(46+1<<20, 40)}, 16 << 20, 8192, nil},
		{"first_record_larger_than_target", []clientSizeRun{run(30, 1), run(5, 2)}, 20, 100, nil},
		{"oversized_in_the_middle", []clientSizeRun{run(8, 2), run(64, 1), run(8, 3)}, 20, 100, nil},
		{"empty", nil, 16 << 20, 8192, nil},
		{"zero_size_records", []clientSizeRun{run(0, 3), run(25, 1), run(0, 2), run(1, 1)}, 25, 100, nil},
		{"max_keys_one", []clientSizeRun{run(5, 3)}, 100, 1, nil},
		{"max_bytes_zero", []clientSizeRun{run(0, 2), run(1, 1), run(0, 1)}, 0, 100, nil},
		{"key_cap_before_byte_cap", []clientSizeRun{run(2, 10)}, 100, 4, nil},
		{"byte_cap_before_key_cap", []clientSizeRun{run(30, 10)}, 100, 4, nil},
		{"64mib_cap", []clientSizeRun{run(46+4<<20, 20)}, 64 << 20, 8192, nil},
	}
	for _, s := range scs {
		c, err := clientBatchScenario(s.name, s.sizes, s.maxBytes, s.maxKeys)
		if err != nil {
			return nil, err
		}
		if s.want != nil && fmt.Sprint(c.WantLens) != fmt.Sprint(s.want) {
			return nil, fmt.Errorf("batch_test.go %s: got %v, want %v", s.name, c.WantLens, s.want)
		}
		f.Cases = append(f.Cases, c)
	}
	// Pseudo-random record sizes against small targets.
	rnd := u64s(0x42415443, 4096)
	for i := 0; i < 5; i++ {
		n := 20 + int(rnd[0]%100)
		rnd = rnd[1:]
		sizes := make([]clientSizeRun, n)
		for j := range sizes {
			sizes[j] = run(int(rnd[0]%(100<<10)), 1)
			rnd = rnd[1:]
		}
		maxBytes := 1 << (16 + i%5)
		maxKeys := 3 + i*4
		c, err := clientBatchScenario(fmt.Sprintf("splitmix_%d", i), sizes, maxBytes, maxKeys)
		if err != nil {
			return nil, err
		}
		f.Cases = append(f.Cases, c)
	}
	return f, nil
}

// ---- client/fetch.json ----

// Verbatim copies of dstore v0.1.9 client/fetch.go: the get batch limits,
// estSize, fetchKey, fetchJob, fetchAcc and pickBatch.
const (
	getBatchKeys  = 2048
	getBatchBytes = 8 << 20
	getEstMax     = 64 << 10
)

func estSize(k [32]byte) int {
	return amberpack.RecHeaderSize + min(int(key.Key(k).Length()), getEstMax)
}

type fetchKey struct {
	key     [32]byte
	attempt int
}

type fetchJob struct {
	node view.NodeID
	keys []fetchKey
}

type fetchAcc struct {
	keys  []fetchKey
	bytes int
}

func pickBatch(acc map[view.NodeID]*fetchAcc) (fetchJob, bool) {
	var best view.NodeID
	var bestAcc *fetchAcc
	for id, a := range acc {
		if len(a.keys) >= getBatchKeys || a.bytes >= getBatchBytes {
			best, bestAcc = id, a
			break
		}
		if bestAcc == nil || len(a.keys) > len(bestAcc.keys) {
			best, bestAcc = id, a
		}
	}
	if bestAcc == nil {
		return fetchJob{}, false
	}
	n := 0
	bytes := 0
	for n < len(bestAcc.keys) && n < getBatchKeys && (n == 0 || bytes+estSize(bestAcc.keys[n].key) <= getBatchBytes) {
		bytes += estSize(bestAcc.keys[n].key)
		n++
	}
	job := fetchJob{node: best, keys: bestAcc.keys[:n:n]}
	bestAcc.keys = bestAcc.keys[n:]
	bestAcc.bytes -= bytes
	if len(bestAcc.keys) == 0 {
		delete(acc, best)
	}
	return job, true
}

type clientEstSizeCase struct {
	Name   string `json:"name"`
	Key    Hex    `json:"key"`
	Length U64    `json:"length"`
	Est    I64    `json:"est"`
}

// clientKeyRun is a run of count keys whose length field is length.
type clientKeyRun struct {
	Length U64 `json:"length"`
	Count  int `json:"count"`
}

type clientAcc struct {
	Node  Hex            `json:"node"`
	Keys  []clientKeyRun `json:"keys"`
	Bytes int            `json:"bytes"`
}

type clientJob struct {
	Node  int `json:"node"`
	Count int `json:"count"`
	Bytes int `json:"bytes"`
}

type clientPickCase struct {
	Name string      `json:"name"`
	Accs []clientAcc `json:"accs"`
	Jobs []clientJob `json:"jobs"`
}

type clientFetchFile struct {
	GetBatchKeys  int                 `json:"get_batch_keys"`
	GetBatchBytes int                 `json:"get_batch_bytes"`
	GetEstMax     int                 `json:"get_est_max"`
	EstSize       []clientEstSizeCase `json:"est_size"`
	PickBatch     []clientPickCase    `json:"pick_batch"`
}

// clientLengthKey is key.NewFromHash(t, length, data(seed, 32)).
func clientLengthKey(t key.Type, length uint64, seed uint64) (key.Key, error) {
	return key.NewFromHash(t, length, [32]byte(smData(seed, 32)))
}

// clientPickScenario drains accumulators with pickBatch. Every step must have
// one possible pick whatever the map order: at most one full accumulator and,
// when none is full, one accumulator with the most keys.
func clientPickScenario(name string, runs [][]clientKeyRun) (clientPickCase, error) {
	c := clientPickCase{Name: name, Accs: []clientAcc{}, Jobs: []clientJob{}}
	acc := map[view.NodeID]*fetchAcc{}
	order := make([]*fetchAcc, len(runs))
	index := map[view.NodeID]int{}
	seed := uint64(0x5049434b)
	for i, rs := range runs {
		var id view.NodeID
		id[0], id[1] = byte(i+1), 0x7e
		a := &fetchAcc{}
		for _, r := range rs {
			for j := 0; j < r.Count; j++ {
				k, err := clientLengthKey(key.Blob, uint64(r.Length), seed)
				if err != nil {
					return c, err
				}
				seed++
				a.keys = append(a.keys, fetchKey{key: [32]byte(k)})
				a.bytes += estSize(k)
			}
		}
		acc[id], order[i], index[id] = a, a, i
		c.Accs = append(c.Accs, clientAcc{Node: Hex(id[:]), Keys: rs, Bytes: a.bytes})
	}
	for step := 0; ; step++ {
		if step > 1<<16 {
			return c, fmt.Errorf("pick_batch %s: does not drain", name)
		}
		full, most, mostCount := 0, -1, 0
		for _, a := range acc {
			if len(a.keys) >= getBatchKeys || a.bytes >= getBatchBytes {
				full++
			}
			switch {
			case len(a.keys) > most:
				most, mostCount = len(a.keys), 1
			case len(a.keys) == most:
				mostCount++
			}
		}
		if full > 1 || (full == 0 && mostCount > 1) {
			return c, fmt.Errorf("pick_batch %s: step %d depends on map order (%d full, %d tied at %d keys)", name, step, full, mostCount, most)
		}
		var front []fetchKey
		var frontBytes int
		job, ok := pickBatch(acc)
		if !ok {
			if len(acc) != 0 {
				return c, fmt.Errorf("pick_batch %s: no job with %d accumulators left", name, len(acc))
			}
			return c, nil
		}
		i := index[job.node]
		front = job.keys
		for _, fk := range front {
			frontBytes += estSize(fk.key)
		}
		c.Jobs = append(c.Jobs, clientJob{Node: i, Count: len(front), Bytes: frontBytes})
		if _, kept := acc[job.node]; kept && order[i].bytes < 0 {
			return c, fmt.Errorf("pick_batch %s: negative accumulator bytes", name)
		}
	}
}

func genClientFetch() (any, error) {
	f := clientFetchFile{GetBatchKeys: getBatchKeys, GetBatchBytes: getBatchBytes, GetEstMax: getEstMax}
	add := func(name string, k [32]byte) {
		f.EstSize = append(f.EstSize, clientEstSizeCase{Name: name, Key: Hex(k[:]), Length: U64(key.Key(k).Length()), Est: I64(estSize(k))})
	}
	for i, l := range []uint64{0, 1, 45, 100, 65535, 65536, 65537, 1 << 20, 1 << 32, 1 << 40, 1 << 56, math.MaxInt64} {
		k, err := clientLengthKey(key.Blob, l, 0xe57+uint64(i))
		if err != nil {
			return nil, err
		}
		add(fmt.Sprintf("blob_length_%d", l), k)
	}
	for i, t := range []key.Type{key.FileNode, key.DirLeaf, key.DirNode, key.XattrSet} {
		k, err := clientLengthKey(t, 300000, 0xe5700+uint64(i))
		if err != nil {
			return nil, err
		}
		add(fmt.Sprintf("%s_length_300000", strings.ToLower(t.String())), k)
	}
	// Keys that are not canonical: estSize reads the length field unvalidated.
	// Length fields of 2^63 and above wrap negative in Go's int conversion.
	garbage := [][32]byte{}
	for _, hdr := range [][]byte{
		{0x07, 0x80, 0, 0, 0, 0, 0, 0, 0},
		{0x07, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff},
		{0x07, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xfe},
		{0x07, 0x7f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff},
		{0x01, 0x00, 0x05},
		{0x08, 0x00},
		{0xf3, 0x00, 0x00, 0xff, 0xff},
		{0xff, 0xff},
	} {
		var k [32]byte
		copy(k[:], smData(uint64(0x6b6579)+uint64(len(garbage)), 32))
		copy(k[:], hdr)
		garbage = append(garbage, k)
	}
	for i, k := range garbage {
		add(fmt.Sprintf("noncanonical_%d", i), k)
	}
	for i := 0; i < 16; i++ {
		add(fmt.Sprintf("splitmix_%02d", i), [32]byte(smData(0x455354+uint64(i), 32)))
	}

	run := func(length uint64, count int) clientKeyRun { return clientKeyRun{Length: U64(length), Count: count} }
	type sc struct {
		name string
		runs [][]clientKeyRun
		want []int // job counts, checked when non-nil (client-transfer §5.2 item 5)
	}
	scs := []sc{
		{"one_accumulator_5000_small_keys", [][]clientKeyRun{{run(0, 5000)}}, []int{2048, 2048, 904}},
		{"est_max_keys_cut_at_8mib", [][]clientKeyRun{{run(65536, 200)}}, []int{127, 73}},
		{"full_by_bytes_beats_a_larger_accumulator", [][]clientKeyRun{{run(65536, 128)}, {run(0, 1000)}}, []int{127, 1000, 1}},
		{"most_keys_first", [][]clientKeyRun{{run(0, 10)}, {run(0, 20)}, {run(0, 5)}}, []int{20, 10, 5}},
		{"full_by_keys", [][]clientKeyRun{{run(0, 2048)}, {run(0, 2047)}}, []int{2048, 2047}},
		{"mixed_sizes", [][]clientKeyRun{{run(65536, 100), run(0, 3000), run(100000, 30)}}, nil},
		{"exactly_8mib", [][]clientKeyRun{{run(65536, 127), run(59648, 1), run(0, 1)}}, []int{128, 1}},
		{"three_accumulators", [][]clientKeyRun{{run(0, 3000)}, {run(65536, 100)}, {run(500, 700)}}, nil},
		{"partial_before_full", [][]clientKeyRun{{run(1<<20, 64)}, {run(0, 63)}}, nil},
		{"no_accumulators", nil, []int{}},
	}
	for _, s := range scs {
		c, err := clientPickScenario(s.name, s.runs)
		if err != nil {
			return nil, err
		}
		if s.want != nil {
			got := make([]int, len(c.Jobs))
			for i, j := range c.Jobs {
				got[i] = j.Count
			}
			if fmt.Sprint(got) != fmt.Sprint(s.want) {
				return nil, fmt.Errorf("pick_batch %s: job counts %v, want %v", s.name, got, s.want)
			}
		}
		if c.Accs == nil {
			c.Accs = []clientAcc{}
		}
		f.PickBatch = append(f.PickBatch, c)
	}
	return f, nil
}

// ---- client/verify_record.json ----

type clientRawRecordJSON struct {
	Key   Hex    `json:"key"`
	Flags int    `json:"flags"`
	Ulen  uint32 `json:"ulen"`
	Slen  uint32 `json:"slen"`
	Bytes Hex    `json:"bytes"`
}

type clientVerifyCase struct {
	Name string              `json:"name"`
	Raw  clientRawRecordJSON `json:"raw"`
	// Parses reports whether amberpack.ParseRecord(raw.bytes) accepts the
	// record with exactly this header (records from a pack stream always do).
	Parses bool   `json:"parses"`
	OK     bool   `json:"ok"`
	OutKey Hex    `json:"out_key,omitempty"`
	Error  string `json:"error,omitempty"`
	// ErrorPortable is false for texts of the zstd library, which core-rs
	// (libzstd) cannot reproduce.
	ErrorPortable *bool `json:"error_portable,omitempty"`
}

type clientVerifyFile struct {
	Cases []clientVerifyCase `json:"cases"`
}

var clientCastagnoli = crc32.MakeTable(crc32.Castagnoli)

// clientRecord frames payload under k with the given header fields and a
// valid CRC-32C, as amberpack.EncodeRecord does, without choosing the flags.
func clientRecord(k key.Key, flags byte, ulen uint32, payload []byte) []byte {
	rec := make([]byte, amberpack.RecHeaderSize+len(payload))
	rec[0] = 0x01
	copy(rec[1:33], k[:])
	rec[33] = flags
	binary.BigEndian.PutUint32(rec[34:38], ulen)
	binary.BigEndian.PutUint32(rec[38:42], uint32(len(payload)))
	copy(rec[amberpack.RecHeaderSize:], payload)
	binary.BigEndian.PutUint32(rec[42:46], crc32.Checksum(rec, clientCastagnoli))
	return rec
}

func clientVerifyScenario(name string, rec []byte) (clientVerifyCase, error) {
	c := clientVerifyCase{Name: name}
	if len(rec) < amberpack.RecHeaderSize {
		return c, fmt.Errorf("verify_record %s: short record", name)
	}
	raw := amberpack.RawRecord{
		Record: amberpack.Record{
			Key:   key.Key(rec[1:33]),
			Flags: rec[33],
			Ulen:  binary.BigEndian.Uint32(rec[34:38]),
			Slen:  binary.BigEndian.Uint32(rec[38:42]),
		},
		Bytes: rec,
	}
	c.Raw = clientRawRecordJSON{Key: Hex(raw.Key[:]), Flags: int(raw.Flags), Ulen: raw.Ulen, Slen: raw.Slen, Bytes: Hex(rec)}
	if parsed, err := amberpack.ParseRecord(rec); err == nil && parsed == raw.Record && len(rec) == amberpack.RecHeaderSize+int(raw.Slen) {
		c.Parses = true
	}
	k, out, err := client.VerifyRecord(raw)
	if err != nil {
		c.Error = err.Error()
		portable := !strings.Contains(c.Error, "zstd:")
		c.ErrorPortable = &portable
		return c, nil
	}
	if k != [32]byte(raw.Key) || !bytes.Equal(out, rec) {
		return c, fmt.Errorf("verify_record %s: accepted with another key or record", name)
	}
	c.OK, c.OutKey = true, Hex(k[:])
	return c, nil
}

func genClientVerifyRecord() (any, error) {
	var f clientVerifyFile
	add := func(name string, rec []byte, wantOK, wantParses bool) error {
		c, err := clientVerifyScenario(name, rec)
		if err != nil {
			return err
		}
		if c.OK != wantOK || c.Parses != wantParses {
			return fmt.Errorf("verify_record %s: ok=%v parses=%v (error %q), want ok=%v parses=%v", name, c.OK, c.Parses, c.Error, wantOK, wantParses)
		}
		f.Cases = append(f.Cases, c)
		return nil
	}
	encode := func(k key.Key, data []byte) ([]byte, error) { return amberpack.EncodeRecord(k, data) }
	newKey := func(t key.Type, length uint64, data []byte) key.Key {
		k, err := key.New(t, length, data)
		if err != nil {
			panic(err) // defined types never fail
		}
		return k
	}
	p100 := smData(1, 100)
	k100 := newKey(key.Blob, 100, p100)
	aaaa := bytes.Repeat([]byte("a"), 4096)
	kaaaa := newKey(key.Blob, 4096, aaaa)
	zaaaa, err := encode(kaaaa, aaaa)
	if err != nil {
		return nil, err
	}
	if zaaaa[33] != 1 {
		return nil, errors.New("verify_record: 4096 x 'a' did not compress")
	}
	zframe := zaaaa[amberpack.RecHeaderSize:]

	rec, err := encode(k100, p100)
	if err != nil {
		return nil, err
	}
	if rec[33] != 0 {
		return nil, errors.New("verify_record: splitmix payload compressed")
	}
	if err := add("raw_blob", rec, true, true); err != nil {
		return nil, err
	}
	empty, err := encode(newKey(key.Blob, 0, nil), nil)
	if err != nil {
		return nil, err
	}
	if err := add("raw_empty_blob", empty, true, true); err != nil {
		return nil, err
	}
	if err := add("zstd_blob", zaaaa, true, true); err != nil {
		return nil, err
	}
	for _, s := range []struct {
		name string
		t    key.Type
		n    uint64
		seed uint64
		len  int
	}{
		{"dirleaf_logical_length", key.DirLeaf, 5000, 2, 64},
		{"filenode_logical_length", key.FileNode, 1 << 40, 3, 90},
		{"xattrset", key.XattrSet, 40, 4, 40},
	} {
		data := smData(s.seed, s.len)
		r, err := encode(newKey(s.t, s.n, data), data)
		if err != nil {
			return nil, err
		}
		if err := add(s.name, r, true, true); err != nil {
			return nil, err
		}
	}
	// The client does not check a Blob's length field against the payload.
	r999, err := encode(newKey(key.Blob, 999, p100), p100)
	if err != nil {
		return nil, err
	}
	if err := add("blob_length_field_999_over_100_bytes", r999, true, true); err != nil {
		return nil, err
	}
	flipped := append([]byte{}, p100...)
	flipped[99] ^= 0x01
	if err := add("payload_byte_flipped_crc_recomputed", clientRecord(k100, 0, 100, flipped), false, true); err != nil {
		return nil, err
	}
	if err := add("payload_of_another_key", clientRecord(k100, 0, 100, smData(2, 100)), false, true); err != nil {
		return nil, err
	}
	bbbb := bytes.Repeat([]byte("b"), 4096)
	if err := add("zstd_payload_of_another_key", clientRecord(newKey(key.Blob, 4096, bbbb), 1, 4096, zframe), false, true); err != nil {
		return nil, err
	}
	if err := add("zstd_frame_stored_raw", clientRecord(kaaaa, 0, uint32(len(zframe)), zframe), false, true); err != nil {
		return nil, err
	}
	reserved := k100
	reserved[0] |= 0x08
	if err := add("key_reserved_header_bit", clientRecord(reserved, 0, 100, p100), false, false); err != nil {
		return nil, err
	}
	type5 := k100
	type5[0] = 0x50 | type5[0]&0x0f
	if err := add("key_reserved_type_5", clientRecord(type5, 0, 100, p100), false, false); err != nil {
		return nil, err
	}
	type15 := k100
	type15[0] = 0xf0 | type15[0]&0x0f
	if err := add("key_reserved_type_15", clientRecord(type15, 0, 100, p100), false, false); err != nil {
		return nil, err
	}
	nonCanon := k100
	nonCanon[0], nonCanon[1], nonCanon[2] = 0x01, 0x00, 100
	if err := add("key_noncanonical_length", clientRecord(nonCanon, 0, 100, p100), false, false); err != nil {
		return nil, err
	}
	if err := add("unknown_flag_bit_accepted", clientRecord(k100, 0x02, 100, p100), true, false); err != nil {
		return nil, err
	}
	if err := add("zstd_with_unknown_flag_bit", clientRecord(kaaaa, 0x03, 4096, zframe), true, false); err != nil {
		return nil, err
	}
	if err := add("raw_ulen_ignored", clientRecord(k100, 0, 5, p100), true, false); err != nil {
		return nil, err
	}
	if err := add("zstd_header_ulen_one_more", clientRecord(kaaaa, 1, 4097, zframe), false, true); err != nil {
		return nil, err
	}
	if err := add("zstd_header_ulen_one_less", clientRecord(kaaaa, 1, 4095, zframe), false, true); err != nil {
		return nil, err
	}
	if err := add("zstd_garbage_frame", clientRecord(kaaaa, 1, 4096, smData(9, 64)), false, true); err != nil {
		return nil, err
	}
	return f, nil
}

// ---- client/progress.json ----

// clientProgress, clientProgressReport, clientNodeProgress and
// clientPutObserver stand for the client package's types in the copies.
type (
	clientProgress       = client.Progress
	clientProgressReport = client.ProgressReport
	clientNodeProgress   = client.NodeProgress
	clientPutObserver    = client.PutObserver
)

// clientStubCluster stands for client.Cluster in the copies of tracker and
// pathAttrs, which use nothing of it but pool.Path.
type clientStubCluster struct {
	pool *clientStubPool
}

// clientStubPool scripts pool.Path: the live connection's path per node.
type clientStubPool struct {
	paths map[view.NodeID]transport.PathInfo
}

func (p *clientStubPool) Path(id view.NodeID, alpn string) (transport.PathInfo, bool) {
	if alpn != wire.ALPNClient {
		return transport.PathInfo{}, false
	}
	pi, ok := p.paths[id]
	return pi, ok
}

// Verbatim copies of dstore v0.1.9 client/progress.go: tracker, newTracker,
// its methods, countKeys and (*Cluster).pathAttrs (types renamed).
type tracker struct {
	c     *clientStubCluster
	prog  clientProgress
	mu    sync.Mutex
	rep   clientProgressReport
	nodes map[view.NodeID]*clientNodeProgress
}

func newTracker(c *clientStubCluster, prog clientProgress) *tracker {
	return &tracker{c: c, prog: prog, nodes: map[view.NodeID]*clientNodeProgress{}}
}

func (t *tracker) observer() clientPutObserver {
	return clientPutObserver{
		Start: func(id view.NodeID) { t.update(func() { t.node(id).InFlight++ }) },
		Sent: func(id view.NodeID, n int) {
			t.update(func() {
				t.node(id).Bytes += int64(n)
				t.rep.Bytes += int64(n)
			})
		},
		Flushed: func(id view.NodeID) { t.update(func() { t.node(id).Awaiting++ }) },
		Done: func(id view.NodeID, flushed bool) {
			t.update(func() {
				np := t.node(id)
				np.InFlight--
				if flushed {
					np.Awaiting--
				}
			})
		},
	}
}

func (t *tracker) totals(objects, done int, bytes int64) {
	t.update(func() { t.rep.TotalObjects, t.rep.Objects, t.rep.TotalBytes = objects, done, bytes })
}

func (t *tracker) more(bytes int64) { t.update(func() { t.rep.TotalBytes += bytes }) }

func (t *tracker) objects(n int) { t.update(func() { t.rep.Objects += n }) }

func (t *tracker) bytes() int64 {
	t.mu.Lock()
	defer t.mu.Unlock()
	return t.rep.Bytes
}

func (t *tracker) node(id view.NodeID) *clientNodeProgress {
	np := t.nodes[id]
	if np == nil {
		np = &clientNodeProgress{ID: id}
		t.nodes[id] = np
	}
	return np
}

func (t *tracker) update(f func()) {
	t.mu.Lock()
	defer t.mu.Unlock()
	f()
	if t.prog != nil {
		t.prog(t.snapshot())
	}
}

func (t *tracker) snapshot() clientProgressReport {
	rep := t.rep
	rep.Nodes = make([]clientNodeProgress, 0, len(t.nodes))
	for _, np := range t.nodes {
		n := *np
		if p, ok := t.c.pool.Path(n.ID, wire.ALPNClient); ok {
			n.Direct, n.RTT = p.Direct, p.RTT
		}
		rep.Nodes = append(rep.Nodes, n)
	}
	sort.Slice(rep.Nodes, func(i, j int) bool { return bytes.Compare(rep.Nodes[i].ID[:], rep.Nodes[j].ID[:]) < 0 })
	return rep
}

func countKeys(m map[view.NodeID][][32]byte, size clientRecordSizer) (objects int, bytes int64) {
	for _, ks := range m {
		objects += len(ks)
		for _, k := range ks {
			bytes += int64(size(k))
		}
	}
	return objects, bytes
}

func (c *clientStubCluster) pathAttrs(id view.NodeID) []any {
	p, ok := c.pool.Path(id, wire.ALPNClient)
	if !ok {
		return []any{"path", "none"}
	}
	kind := "direct"
	if !p.Direct {
		kind = "relay"
	}
	return []any{"path", kind, "rtt", p.RTT.Round(time.Millisecond)}
}

type clientNodeReportJSON struct {
	ID       Hex  `json:"id"`
	Direct   bool `json:"direct"`
	RTTNs    I64  `json:"rtt_ns"`
	InFlight int  `json:"in_flight"`
	Awaiting int  `json:"awaiting"`
	Bytes    I64  `json:"bytes"`
}

type clientReportJSON struct {
	Objects      int                    `json:"objects"`
	TotalObjects int                    `json:"total_objects"`
	Bytes        I64                    `json:"bytes"`
	TotalBytes   I64                    `json:"total_bytes"`
	Nodes        []clientNodeReportJSON `json:"nodes"`
}

func clientReportOf(r clientProgressReport) *clientReportJSON {
	out := &clientReportJSON{Objects: r.Objects, TotalObjects: r.TotalObjects, Bytes: I64(r.Bytes), TotalBytes: I64(r.TotalBytes), Nodes: []clientNodeReportJSON{}}
	for _, n := range r.Nodes {
		out.Nodes = append(out.Nodes, clientNodeReportJSON{ID: Hex(n.ID[:]), Direct: n.Direct, RTTNs: I64(n.RTT), InFlight: n.InFlight, Awaiting: n.Awaiting, Bytes: I64(n.Bytes)})
	}
	return out
}

type clientTrackerOp struct {
	Op      string            `json:"op"`
	Node    *int              `json:"node,omitempty"`
	N       *int              `json:"n,omitempty"`
	Flushed *bool             `json:"flushed,omitempty"`
	Objects *int              `json:"objects,omitempty"`
	Done    *int              `json:"done,omitempty"`
	Bytes   *I64              `json:"bytes,omitempty"`
	Result  *I64              `json:"result,omitempty"`
	Report  *clientReportJSON `json:"report"`
}

type clientTrackerScenario struct {
	Name     string            `json:"name"`
	Nodes    []Hex             `json:"nodes"`
	Paths    []*clientPathJSON `json:"paths"`
	Progress bool              `json:"progress"`
	Ops      []clientTrackerOp `json:"ops"`
}

type clientCountKeysCase struct {
	Name    string  `json:"name"`
	Lists   [][]int `json:"lists"` // per node: the record sizes of its keys
	Objects int     `json:"objects"`
	Bytes   I64     `json:"bytes"`
}

type clientHumanBytesCase struct {
	N   I64    `json:"n"`
	Out string `json:"out"`
}

type clientRateCase struct {
	Bytes  I64    `json:"bytes"`
	TookNs I64    `json:"took_ns"`
	Out    string `json:"out"`
}

type clientAttrJSON struct {
	Key        string  `json:"key"`
	Kind       string  `json:"kind"`
	String     *string `json:"string,omitempty"`
	DurationNs *I64    `json:"duration_ns,omitempty"`
}

type clientPathAttrsCase struct {
	Path  *clientPathJSON  `json:"path"`
	Attrs []clientAttrJSON `json:"attrs"`
	Text  string           `json:"text"`
}

type clientProgressFile struct {
	Tracker    []clientTrackerScenario `json:"tracker"`
	CountKeys  []clientCountKeysCase   `json:"count_keys"`
	HumanBytes []clientHumanBytesCase  `json:"human_bytes"`
	Rate       []clientRateCase        `json:"rate"`
	PathAttrs  []clientPathAttrsCase   `json:"path_attrs"`
}

// clientStep is one tracker call of a scenario.
type clientStep struct {
	op            string
	node, n       int
	flushed       bool
	objects, done int
	bytes         int64
}

func clientTrackerRun(name string, ids []view.NodeID, paths []*transport.PathInfo, progress bool, steps []clientStep) (clientTrackerScenario, error) {
	sc := clientTrackerScenario{Name: name, Progress: progress, Nodes: []Hex{}, Paths: []*clientPathJSON{}, Ops: []clientTrackerOp{}}
	pool := &clientStubPool{paths: map[view.NodeID]transport.PathInfo{}}
	for i, id := range ids {
		sc.Nodes = append(sc.Nodes, Hex(append([]byte{}, id[:]...)))
		sc.Paths = append(sc.Paths, clientPathOf(paths[i]))
		if paths[i] != nil {
			pool.paths[id] = *paths[i]
		}
	}
	var reports []clientProgressReport
	var prog clientProgress
	if progress {
		prog = func(r clientProgressReport) { reports = append(reports, r) }
	}
	t := newTracker(&clientStubCluster{pool: pool}, prog)
	obs := t.observer()
	for _, s := range steps {
		op := clientTrackerOp{Op: s.op}
		node, n, flushed, objects, done, b := s.node, s.n, s.flushed, s.objects, s.done, I64(s.bytes)
		before := len(reports)
		switch s.op {
		case "start":
			op.Node = &node
			obs.Start(ids[s.node])
		case "sent":
			op.Node, op.N = &node, &n
			obs.Sent(ids[s.node], s.n)
		case "flushed":
			op.Node = &node
			obs.Flushed(ids[s.node])
		case "done":
			op.Node, op.Flushed = &node, &flushed
			obs.Done(ids[s.node], s.flushed)
		case "totals":
			op.Objects, op.Done, op.Bytes = &objects, &done, &b
			t.totals(s.objects, s.done, s.bytes)
		case "more":
			op.Bytes = &b
			t.more(s.bytes)
		case "objects":
			op.N = &n
			t.objects(s.n)
		case "bytes":
			r := I64(t.bytes())
			op.Result = &r
		default:
			return sc, fmt.Errorf("progress %s: unknown op %s", name, s.op)
		}
		switch len(reports) - before {
		case 0:
			if progress && s.op != "bytes" {
				return sc, fmt.Errorf("progress %s: %s reported nothing", name, s.op)
			}
		case 1:
			op.Report = clientReportOf(reports[before])
		default:
			return sc, fmt.Errorf("progress %s: %s reported %d times", name, s.op, len(reports)-before)
		}
		sc.Ops = append(sc.Ops, op)
	}
	return sc, nil
}

// clientNodeID is crypto/ed25519.NewKeyFromSeed(data(seed, 32))'s public key.
func clientNodeID(seed uint64) view.NodeID {
	return view.NodeID(ed25519.NewKeyFromSeed(smData(seed, 32)).Public().(ed25519.PublicKey))
}

func genClientProgress() (any, error) {
	var f clientProgressFile
	ms := time.Millisecond
	st := func(op string, node, n int) clientStep { return clientStep{op: op, node: node, n: n} }
	done := func(node int, flushed bool) clientStep { return clientStep{op: "done", node: node, flushed: flushed} }
	totals := func(objects, d int, b int64) clientStep {
		return clientStep{op: "totals", objects: objects, done: d, bytes: b}
	}
	more := func(b int64) clientStep { return clientStep{op: "more", bytes: b} }

	var high, low view.NodeID
	high[0], high[31] = 0xf0, 0x01
	low[0], low[31] = 0x0a, 0x02
	scenarios := []struct {
		name     string
		ids      []view.NodeID
		paths    []*transport.PathInfo
		progress bool
		steps    []clientStep
	}{
		{"ids_sort_reversed_from_first_use", []view.NodeID{high, low}, []*transport.PathInfo{nil, nil}, true, []clientStep{
			totals(10, 4, 600), st("start", 0, 0), st("sent", 0, 100), st("start", 1, 0), st("sent", 1, 50),
			st("flushed", 0, 0), done(0, true), st("objects", 0, 2), st("sent", 1, 25), more(120), st("flushed", 1, 0),
			st("start", 1, 0), done(1, false), done(1, true), st("objects", 0, 4), st("bytes", 0, 0),
		}},
		{"live_mem_paths", []view.NodeID{clientNodeID(1), clientNodeID(2), clientNodeID(3)},
			[]*transport.PathInfo{clientDirect(ms), nil, clientDirect(ms)}, true, []clientStep{
				totals(3, 0, 300), st("start", 2, 0), st("start", 0, 0), st("start", 1, 0), st("sent", 0, 100), st("sent", 1, 100),
				st("sent", 2, 100), st("flushed", 1, 0), st("flushed", 0, 0), st("flushed", 2, 0), done(0, true), done(2, true),
				done(1, true), st("objects", 0, 3), st("bytes", 0, 0),
			}},
		{"relayed_path", []view.NodeID{clientNodeID(4)}, []*transport.PathInfo{clientRelayed(37 * ms)}, true, []clientStep{
			st("start", 0, 0), st("sent", 0, 4096), st("flushed", 0, 0), done(0, true), st("bytes", 0, 0),
		}},
		{"push_without_upload", []view.NodeID{}, []*transport.PathInfo{}, true, []clientStep{
			totals(5, 5, 0), st("bytes", 0, 0),
		}},
		{"no_progress_callback", []view.NodeID{clientNodeID(5)}, []*transport.PathInfo{nil}, false, []clientStep{
			totals(2, 0, 200), st("start", 0, 0), st("sent", 0, 150), done(0, false), more(50), st("objects", 0, 1), st("bytes", 0, 0),
		}},
	}
	for _, s := range scenarios {
		sc, err := clientTrackerRun(s.name, s.ids, s.paths, s.progress, s.steps)
		if err != nil {
			return nil, err
		}
		f.Tracker = append(f.Tracker, sc)
	}

	for _, c := range []struct {
		name  string
		lists [][]int
	}{
		{"empty", [][]int{}},
		{"one_node", [][]int{{146, 246, 46}}},
		{"three_nodes", [][]int{{1048622}, {}, {46, 47, 48, 49}}},
	} {
		m := map[view.NodeID][][32]byte{}
		size := map[[32]byte]int{}
		next := 0
		for i, l := range c.lists {
			var id view.NodeID
			id[0] = byte(i + 1)
			m[id] = [][32]byte{}
			for _, n := range l {
				k := clientIndexKey(next)
				next++
				size[k] = n
				m[id] = append(m[id], k)
			}
		}
		objects, b := countKeys(m, func(k [32]byte) int { return size[k] })
		f.CountKeys = append(f.CountKeys, clientCountKeysCase{Name: c.name, Lists: c.lists, Objects: objects, Bytes: I64(b)})
	}

	hb := []int64{0, 1, 1023, 1024, 1025, 1126, 1152, 1177, 1228, 1280, 1331, 1536, 1792, 3584, 10239, 10240, 10291, 10342,
		1048575, 1048576, 1101004, 1310720, 1<<30 - 1, 5 << 30, 1 << 40, 1<<50 - 1, 1 << 50, 1 << 60, math.MaxInt64,
		-1, -5, -1024, -2048, math.MinInt64}
	for i, v := range u64s(0x485542, 40) {
		hb = append(hb, int64(v>>(1+uint(i)%63)))
	}
	for _, n := range hb {
		f.HumanBytes = append(f.HumanBytes, clientHumanBytesCase{N: I64(n), Out: client.HumanBytes(n)})
	}

	type rc struct {
		bytes int64
		took  time.Duration
	}
	rates := []rc{
		{1 << 20, 2 * time.Second}, {0, time.Second}, {5, 0}, {100, -time.Second}, {1 << 30, 3 * time.Second},
		{123456789, 1234 * ms}, {1048576, time.Second}, {0, 0}, {1000, 3 * time.Second}, {5 << 30, 2 * time.Second},
		{100, -1}, {1 << 20, 1500 * ms}, {3 << 20, 2 * time.Second}, {1, 1}, {7, 3}, {1, 3 * time.Hour},
		{-1048576, time.Second}, {1 << 40, ms}, {1 << 62, time.Second}, {392, 1234567 * time.Microsecond},
	}
	for i, v := range u64s(0x52415445, 32) {
		rates = append(rates, rc{int64(v >> 20), time.Duration(1+v%3600000) * ms / time.Duration(1+i%7)})
	}
	for _, r := range rates {
		f.Rate = append(f.Rate, clientRateCase{Bytes: I64(r.bytes), TookNs: I64(r.took), Out: client.Rate(r.bytes, r.took)})
	}

	for _, p := range []*transport.PathInfo{
		nil, clientDirect(0), clientDirect(ms), clientDirect(1499 * time.Microsecond), clientDirect(1500 * time.Microsecond),
		clientDirect(2500 * time.Microsecond), clientDirect(499 * time.Microsecond), clientDirect(500 * time.Microsecond),
		clientRelayed(37200 * time.Microsecond), clientRelayed(0), clientDirect(333 * ms), clientRelayed(time.Hour + 499*time.Microsecond),
	} {
		var id view.NodeID
		id[0] = 0x9a
		pool := &clientStubPool{paths: map[view.NodeID]transport.PathInfo{}}
		if p != nil {
			pool.paths[id] = *p
		}
		attrs := (&clientStubCluster{pool: pool}).pathAttrs(id)
		c := clientPathAttrsCase{Path: clientPathOf(p), Attrs: []clientAttrJSON{}}
		for i := 0; i+1 < len(attrs); i += 2 {
			k, ok := attrs[i].(string)
			if !ok {
				return nil, fmt.Errorf("path_attrs: key %v is not a string", attrs[i])
			}
			switch v := attrs[i+1].(type) {
			case string:
				s := v
				c.Attrs = append(c.Attrs, clientAttrJSON{Key: k, Kind: "String", String: &s})
			case time.Duration:
				d := I64(v)
				c.Attrs = append(c.Attrs, clientAttrJSON{Key: k, Kind: "Duration", DurationNs: &d})
			default:
				return nil, fmt.Errorf("path_attrs: value %v of type %T", v, v)
			}
		}
		var buf bytes.Buffer
		h := slog.NewTextHandler(&buf, &slog.HandlerOptions{ReplaceAttr: func(groups []string, a slog.Attr) slog.Attr {
			if len(groups) == 0 && (a.Key == slog.TimeKey || a.Key == slog.LevelKey || a.Key == slog.MessageKey) {
				return slog.Attr{}
			}
			return a
		}})
		slog.New(h).Info("", attrs...)
		c.Text = strings.TrimSuffix(buf.String(), "\n")
		f.PathAttrs = append(f.PathAttrs, c)
	}
	return f, nil
}

// ---- client/backoff.json ----

// clientBackoffState holds a node's consecutive failure count, as
// client.Cluster does.
type clientBackoffState struct {
	failures map[view.NodeID]int
}

// clientBackoff runs the statements of dstore v0.1.9 client/client.go
// (*Cluster).handleErr that compute a node's backoff after a failure.
func clientBackoff(c *clientBackoffState, id view.NodeID) time.Duration {
	d := 5 * time.Second << min(c.failures[id]-1, 4)
	if d > 60*time.Second {
		d = 60 * time.Second
	}
	return d
}

// clientWatchDelays runs the reconnect delay of dstore v0.1.9 client/watch.go
// WatchRefs over rounds in which no node served the watch: the delay logged as
// "in" and the jitter bound of each round's wait.
func clientWatchDelays(rounds int) (delays, jitterMax []time.Duration) {
	delay := time.Second
	for range rounds {
		delays = append(delays, delay)
		jitterMax = append(jitterMax, time.Duration(int64(delay/2)))
		delay = min(2*delay, 30*time.Second)
	}
	return delays, jitterMax
}

type clientBackoffCase struct {
	Failures  int `json:"failures"`
	BackoffNs I64 `json:"backoff_ns"`
}

type clientWatchDelayCase struct {
	Round       int `json:"round"`
	DelayNs     I64 `json:"delay_ns"`
	JitterMaxNs I64 `json:"jitter_max_ns"`
}

type clientBackoffFile struct {
	HandleErr             []clientBackoffCase    `json:"handle_err"`
	WatchReconnect        []clientWatchDelayCase `json:"watch_reconnect"`
	WatchServedPauseNs    I64                    `json:"watch_served_pause_ns"`
	WatchRefreshTimeoutNs I64                    `json:"watch_refresh_timeout_ns"`
	DialMemberTimeoutNs   I64                    `json:"dial_member_timeout_ns"`
	ProbeTimeoutNs        I64                    `json:"probe_timeout_ns"`
}

func genClientBackoff() (any, error) {
	f := clientBackoffFile{
		WatchServedPauseNs:    I64(200 * time.Millisecond),
		WatchRefreshTimeoutNs: I64(15 * time.Second),
		DialMemberTimeoutNs:   I64(15 * time.Second),
		ProbeTimeoutNs:        I64(3 * time.Second),
	}
	var id view.NodeID
	st := &clientBackoffState{failures: map[view.NodeID]int{}}
	for failures := 1; failures <= 1000; failures++ {
		st.failures[id] = failures
		if failures <= 12 || failures == 16 || failures == 64 || failures == 1000 {
			f.HandleErr = append(f.HandleErr, clientBackoffCase{Failures: failures, BackoffNs: I64(clientBackoff(st, id))})
		}
	}
	delays, jitter := clientWatchDelays(10)
	for i := range delays {
		f.WatchReconnect = append(f.WatchReconnect, clientWatchDelayCase{Round: i + 1, DelayNs: I64(delays[i]), JitterMaxNs: I64(jitter[i])})
	}
	return f, nil
}

// ---- in-memory fake node ----

// clientDiscard is the logger of every generated client.
var clientDiscard = slog.New(slog.NewTextHandler(io.Discard, nil))

// clientServe answers every stream of ep with reply(request), or closes the
// stream without a frame when reply returns nil, until ctx ends.
func clientServe(ctx context.Context, ep *transport.MemEndpoint, reply func(*wire.Msg) *wire.Msg) {
	go func() {
		for {
			conn, err := ep.Accept(ctx)
			if err != nil {
				return
			}
			go func() {
				for {
					s, err := conn.AcceptStream(ctx)
					if err != nil {
						return
					}
					go func() {
						defer s.Close()
						m, err := wire.ReadMsg(s)
						if err != nil {
							return
						}
						if r := reply(m); r != nil {
							_ = wire.WriteMsg(s, r)
						}
					}()
				}
			}()
		}
	}()
}

// clientViewReplier answers view requests with v, stamped with its
// incarnation and epoch and carrying unreachable, as node handleView does;
// other requests go to other (nil: bad-request "unknown operation").
func clientViewReplier(v *view.View, unreachable [][]byte, other func(*wire.Msg) *wire.Msg) (func(*wire.Msg) *wire.Msg, error) {
	enc, err := v.Encode()
	if err != nil {
		return nil, err
	}
	return func(m *wire.Msg) *wire.Msg {
		if m.Type == wire.TView {
			return &wire.Msg{Type: wire.TViewReply, Incarnation: v.Incarnation, Epoch: v.Epoch, View: enc, Unreachable: unreachable}
		}
		if other == nil {
			return wire.ErrMsg(wire.CodeBadRequest, "unknown operation")
		}
		return other(m)
	}, nil
}

// clientClientID is the in-memory endpoint id of every generated client.
var clientClientID = clientNodeID(0xc11e47)

// clientWithCluster binds a fake node under boot answering as reply, dials it
// with client.Dial (a ticket naming only boot), runs f, then tears down.
func clientWithCluster(boot view.NodeID, reply func(*wire.Msg) *wire.Msg, f func(ctx context.Context, cl *client.Cluster) error) error {
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	net := transport.NewNetwork()
	nodeEp := net.Bind(boot, wire.ALPNClient)
	defer nodeEp.Close()
	clientServe(ctx, nodeEp, reply)
	ep := net.Bind(clientClientID)
	defer ep.Close()
	cl, err := client.Dial(ctx, client.Config{
		Endpoint: ep,
		Ticket:   ticket.Ticket{Members: []ticket.Member{{ID: append([]byte{}, boot[:]...)}}},
		Logger:   clientDiscard,
	})
	if err != nil {
		return fmt.Errorf("dial the fake node: %w", err)
	}
	defer cl.Close()
	return f(ctx, cl)
}

// ---- client/placement_decisions.json ----

type clientPlacedCase struct {
	Name    string `json:"name"`
	Holders []int  `json:"holders"`
	Placed  bool   `json:"placed"`
}

type clientDecisionKey struct {
	Key           Hex                `json:"key"`
	Primary       *int               `json:"primary"`
	Owners        []int              `json:"owners"`
	PendingOwners []int              `json:"pending_owners"`
	WriteSet      []int              `json:"write_set"`
	ReadOrder     []int              `json:"read_order"`
	Placed        []clientPlacedCase `json:"placed"`
}

type clientDecisionScenario struct {
	Name        string              `json:"name"`
	View        Hex                 `json:"view"`
	Unreachable []int               `json:"unreachable"`
	Members     []Hex               `json:"members"`
	Bootstrap   int                 `json:"bootstrap"`
	Nodes       []int               `json:"nodes"`
	Replicas    int                 `json:"replicas"`
	MinReplicas int                 `json:"min_replicas"`
	Keys        []clientDecisionKey `json:"keys"`
}

type clientDecisionsFile struct {
	Scenarios []clientDecisionScenario `json:"scenarios"`
}

func genClientPlacementDecisions() (any, error) {
	var f clientDecisionsFile
	ids := make([]view.NodeID, 8)
	for i := range ids {
		ids[i] = clientNodeID(0x504c41 + uint64(i))
	}
	node := func(i int, weight uint32, zone string) view.Node {
		return view.Node{ID: append([]byte{}, ids[i][:]...), Weight: weight, Addrs: []string{fmt.Sprintf("ip:192.168.7.%d:4433", 10+i)}, Zone: zone, Writable: true}
	}
	five := func() []view.Node {
		return []view.Node{node(0, 100, ""), node(1, 100, ""), node(2, 200, ""), node(3, 50, ""), node(4, 100, "")}
	}
	stranger := clientNodeID(0x535452)
	type sc struct {
		name    string
		nodes   []view.Node
		r, minR uint8
		pending *view.Pending
		unreach []int
		keys    int
	}
	scs := []sc{
		{"five_nodes", five(), 3, 2, nil, nil, 64},
		{"pending_six_nodes_shared_zone", five(), 3, 2, &view.Pending{
			Nodes:    []view.Node{node(0, 100, ""), node(1, 100, "rack-a"), node(2, 200, "rack-a"), node(3, 50, ""), node(4, 100, ""), node(5, 150, "")},
			Replicas: 3, ID: 1,
		}, nil, 64},
		{"unreachable_hints", five(), 3, 2, nil, []int{1, 3}, 16},
		{"weight_zero_r2", []view.Node{node(0, 100, ""), node(1, 0, ""), node(2, 300, ""), node(6, 100, "")}, 2, 2, nil, nil, 16},
		{"min_replicas_one_pending_r1", []view.Node{node(0, 100, ""), node(1, 100, "")}, 2, 1, &view.Pending{
			Nodes: []view.Node{node(1, 100, ""), node(7, 100, "")}, Replicas: 1, ID: 2,
		}, nil, 16},
		{"no_nodes", nil, 3, 2, nil, nil, 4},
	}
	for _, s := range scs {
		view.SortNodes(s.nodes)
		if s.pending != nil {
			view.SortNodes(s.pending.Nodes)
		}
		v := &view.View{ClusterID: smData(0x434c55, 16), Incarnation: 1, Epoch: 7, Version: 9, PlacementEpoch: 7, Replicas: s.r, MinReplicas: s.minR, Nodes: s.nodes, Pending: s.pending}
		boot := clientNodeID(0x424f4f54)
		if len(s.nodes) > 0 {
			v.Voters = []view.Voter{{ID: s.nodes[0].ID, Since: 1}}
			boot = s.nodes[0].NID()
		}
		enc, err := v.Encode()
		if err != nil {
			return nil, err
		}
		var unreach [][]byte
		for _, i := range s.unreach {
			unreach = append(unreach, append([]byte{}, ids[i][:]...))
		}

		var members []view.NodeID
		index := map[view.NodeID]int{}
		member := func(id view.NodeID) {
			if _, ok := index[id]; !ok {
				index[id] = len(members)
				members = append(members, id)
			}
		}
		for _, n := range v.Nodes {
			member(n.NID())
		}
		if v.Pending != nil {
			for _, n := range v.Pending.Nodes {
				member(n.NID())
			}
		}
		member(boot)
		member(stranger)
		idx := func(list []view.NodeID) []int {
			out := []int{}
			for _, id := range list {
				out = append(out, index[id])
			}
			return out
		}

		out := clientDecisionScenario{Name: s.name, View: Hex(enc), Unreachable: idx(view.IDsOf(unreach)), Members: []Hex{},
			Bootstrap: index[boot], Replicas: int(s.r), MinReplicas: int(s.minR), Keys: []clientDecisionKey{}}
		for _, id := range members {
			out.Members = append(out.Members, Hex(append([]byte{}, id[:]...)))
		}
		reply, err := clientViewReplier(v, unreach, nil)
		if err != nil {
			return nil, err
		}
		keys := [][32]byte{{}, [32]byte(bytes.Repeat([]byte{0xff}, 32))}
		for i := 0; i < s.keys-2; i++ {
			keys = append(keys, [32]byte(smData(0x4b455953+uint64(i), 32)))
		}
		err = clientWithCluster(boot, reply, func(ctx context.Context, cl *client.Cluster) error {
			out.Nodes = idx(cl.Nodes())
			for _, k := range keys {
				owners, po := cl.Owners(k), cl.Placement().PendingOwners(k)
				dk := clientDecisionKey{Key: Hex(append([]byte{}, k[:]...)), Owners: idx(owners), WriteSet: idx(cl.WriteSet(k)), ReadOrder: idx(cl.ReadOrder(k)), Placed: []clientPlacedCase{}}
				if po != nil {
					dk.PendingOwners = idx(po)
				}
				if p, ok := cl.Primary(k); ok {
					i := index[p]
					dk.Primary = &i
				}
				need := min(int(v.MinReplicas), len(owners))
				sets := []struct {
					name    string
					holders []view.NodeID
				}{
					{"empty", nil},
					{"all_owners", owners},
					{"min_replicas_owners", owners[:need]},
				}
				if need > 0 {
					var rep, rev []view.NodeID
					for i := 0; i < int(v.MinReplicas); i++ {
						rep = append(rep, owners[0])
					}
					for i := len(owners) - 1; i >= len(owners)-need; i-- {
						rev = append(rev, owners[i])
					}
					sets = append(sets,
						struct {
							name    string
							holders []view.NodeID
						}{"one_owner_short", owners[:need-1]},
						struct {
							name    string
							holders []view.NodeID
						}{"first_owner_repeated", rep},
						struct {
							name    string
							holders []view.NodeID
						}{"last_owners_reversed", rev},
					)
				}
				var nonOwners []view.NodeID
				for _, id := range members {
					if !slicesContainsID(owners, id) {
						nonOwners = append(nonOwners, id)
					}
				}
				sets = append(sets,
					struct {
						name    string
						holders []view.NodeID
					}{"non_owners", nonOwners},
					struct {
						name    string
						holders []view.NodeID
					}{"write_set", cl.WriteSet(k)},
					struct {
						name    string
						holders []view.NodeID
					}{"read_order_and_stranger", append(cl.ReadOrder(k), stranger)},
				)
				if po != nil {
					pneed := min(int(v.MinReplicas), len(po))
					var onlyPending []view.NodeID
					for _, id := range po {
						if !slicesContainsID(owners, id) {
							onlyPending = append(onlyPending, id)
						}
					}
					sets = append(sets,
						struct {
							name    string
							holders []view.NodeID
						}{"pending_owners", po},
						struct {
							name    string
							holders []view.NodeID
						}{"pending_only_owners", onlyPending},
						struct {
							name    string
							holders []view.NodeID
						}{"min_owners_and_min_pending_owners", append(append([]view.NodeID{}, owners[:need]...), po[:pneed]...)},
					)
				}
				for _, hs := range sets {
					dk.Placed = append(dk.Placed, clientPlacedCase{Name: hs.name, Holders: idx(hs.holders), Placed: cl.Placed(k, hs.holders)})
				}
				out.Keys = append(out.Keys, dk)
			}
			return nil
		})
		if err != nil {
			return nil, fmt.Errorf("placement_decisions %s: %w", s.name, err)
		}
		f.Scenarios = append(f.Scenarios, out)
	}
	return f, nil
}

// slicesContainsID reports whether ids holds id.
func slicesContainsID(ids []view.NodeID, id view.NodeID) bool {
	for _, x := range ids {
		if x == id {
			return true
		}
	}
	return false
}

// ---- errors/client_text.json ----

type clientTextCase struct {
	Name       string   `json:"name"`
	Kind       string   `json:"kind"`
	Go         string   `json:"go"`
	Code       string   `json:"code,omitempty"`
	Text       string   `json:"text,omitempty"`
	HasCurrent *bool    `json:"has_current,omitempty"`
	Current    Hex      `json:"current,omitempty"`
	Shortfall  *int     `json:"shortfall,omitempty"`
	Type       *int     `json:"type,omitempty"`
	Key        Hex      `json:"key,omitempty"`
	Want       Hex      `json:"want,omitempty"`
	Node       Hex      `json:"node,omitempty"`
	Reason     string   `json:"reason,omitempty"`
	Count      *int     `json:"count,omitempty"`
	Names      []string `json:"names,omitempty"`
	Inner      string   `json:"inner,omitempty"`
	Last       string   `json:"last,omitempty"`
	Out        string   `json:"out"`
}

type clientTextFile struct {
	Cases []clientTextCase `json:"cases"`
}

// clientFake is a fake node for clientDialError; reply returning nil closes
// the stream without a frame.
type clientFake struct {
	id    view.NodeID
	reply func(*wire.Msg) *wire.Msg
}

// clientDialError runs client.Dial against fake nodes and returns its error.
func clientDialError(members []ticket.Member, nodes []clientFake, withEndpoint bool) (string, error) {
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	net := transport.NewNetwork()
	for _, n := range nodes {
		ep := net.Bind(n.id, wire.ALPNClient)
		defer ep.Close()
		clientServe(ctx, ep, n.reply)
	}
	cfg := client.Config{Ticket: ticket.Ticket{Members: members}, Logger: clientDiscard}
	if withEndpoint {
		ep := net.Bind(clientClientID)
		defer ep.Close()
		cfg.Endpoint = ep
	}
	cl, err := client.Dial(ctx, cfg)
	if err == nil {
		cl.Close()
		return "", errors.New("client.Dial succeeded, want an error")
	}
	return err.Error(), nil
}

// clientCallError dials a fake node serving v and answering other, runs
// call, and returns the error call must produce.
func clientCallError(v *view.View, boot view.NodeID, other func(*wire.Msg) *wire.Msg, call func(ctx context.Context, cl *client.Cluster) error) (got, err error) {
	reply, err := clientViewReplier(v, nil, other)
	if err != nil {
		return nil, err
	}
	err = clientWithCluster(boot, reply, func(ctx context.Context, cl *client.Cluster) error {
		got = call(ctx, cl)
		return nil
	})
	if err != nil {
		return nil, err
	}
	if got == nil {
		return nil, errors.New("the call succeeded, want an error")
	}
	return got, nil
}

func genClientText() (any, error) {
	var f clientTextFile
	add := func(c clientTextCase) { f.Cases = append(f.Cases, c) }
	intp := func(i int) *int { return &i }
	boolp := func(b bool) *bool { return &b }
	const noBoot = "client: no bootstrap node answered: "

	p100 := smData(1, 100)
	k1, err := key.New(key.Blob, 100, p100)
	if err != nil {
		return nil, err
	}
	var id1, id2, id3, id4 view.NodeID
	id1[0], id1[31] = 1, 1
	id2[0], id2[31] = 2, 2
	id3[0], id3[31] = 3, 3
	id4[0], id4[31] = 4, 4

	// Exported error values.
	add(clientTextCase{Name: "unknown_ref", Kind: "unknown_ref", Go: "client.ErrUnknownRef", Out: client.ErrUnknownRef.Error()})
	for _, c := range []struct {
		name string
		has  bool
		cur  []byte
	}{
		{"cas_mismatch_absent", false, nil},
		{"cas_mismatch_current", true, k1[:]},
		{"cas_mismatch_current_nil", true, nil},
		{"cas_mismatch_current_3_bytes", true, []byte{1, 2, 3}},
	} {
		e := &client.CASMismatch{Current: c.cur, HasCurrent: c.has, Record: []byte{0xa0}, Version: []byte{9}}
		add(clientTextCase{Name: c.name, Kind: "cas_mismatch", Go: fmt.Sprintf("(&client.CASMismatch{HasCurrent: %v, Current: %x}).Error()", c.has, c.cur),
			HasCurrent: boolp(c.has), Current: Hex(c.cur), Out: e.Error()})
	}
	for _, n := range []int{0, 3, 64, 1 << 40} {
		add(clientTextCase{Name: fmt.Sprintf("incomplete_%d", n), Kind: "incomplete", Go: fmt.Sprintf("(&client.Incomplete{Shortfall: %d}).Error()", n),
			Shortfall: intp(n), Out: (&client.Incomplete{Shortfall: n}).Error()})
	}
	for _, c := range []struct{ code, text string }{
		{wire.CodeBusy, ""}, {wire.CodeUnavailable, "no view"}, {wire.CodeStaleView, "request epoch is behind"},
		{wire.CodeBadRequest, "refglob: unterminated character class"}, {wire.CodeUnknownRef, "no such reference"},
		{wire.CodeUnauthorized, "not on the allowlist"}, {wire.CodeNoSpace, "node below its free-space reserve"}, {wire.CodeInternal, ""},
	} {
		add(clientTextCase{Name: "remote_" + strings.ReplaceAll(c.code, "-", "_"), Kind: "remote", Go: fmt.Sprintf("(&wire.Error{Code: %q, Text: %q}).Error()", c.code, c.text),
			Code: c.code, Text: c.text, Out: (&wire.Error{Code: c.code, Text: c.text}).Error()})
	}
	for _, c := range []struct{ code, text string }{{wire.CodeBusy, ""}, {wire.CodeInternal, "sender died"}} {
		add(clientTextCase{Name: "protocol_remote_" + c.code, Kind: "protocol_remote", Go: fmt.Sprintf("(&protocol.RemoteError{Code: %q, Text: %q}).Error()", c.code, c.text),
			Code: c.code, Text: c.text, Out: (&protocol.RemoteError{Code: c.code, Text: c.text}).Error()})
	}
	flipped := append([]byte{}, p100...)
	flipped[99] ^= 0x01
	want, err := key.New(key.Blob, 100, flipped)
	if err != nil {
		return nil, err
	}
	_, _, verr := client.VerifyRecord(amberpack.RawRecord{Record: amberpack.Record{Key: k1, Ulen: 100, Slen: 100}, Bytes: clientRecord(k1, 0, 100, flipped)})
	if verr == nil {
		return nil, errors.New("VerifyRecord accepted a flipped payload")
	}
	add(clientTextCase{Name: "payload_hash", Kind: "payload_hash", Go: "client.VerifyRecord of k1's record with its last payload byte flipped", Want: Hex(want[:]), Key: Hex(k1[:]), Out: verr.Error()})

	// client.Dial.
	notBound, tokNode := clientNodeID(0x4e4f54), clientNodeID(0x544f4b)
	member := func(id view.NodeID) ticket.Member { return ticket.Member{ID: append([]byte{}, id[:]...)} }
	tok := func(*wire.Msg) *wire.Msg { return &wire.Msg{Type: wire.TOK} }
	for _, d := range []struct {
		name, goText string
		members      []ticket.Member
		nodes        []clientFake
		endpoint     bool
	}{
		{"dial_no_endpoint", "client.Dial with a nil Endpoint", nil, nil, false},
		{"dial_ticket_names_no_nodes", "client.Dial with a ticket without members", nil, nil, true},
		{"dial_member_id_31_bytes", "client.Dial with a ticket whose only member id has 31 bytes", []ticket.Member{{ID: make([]byte, 31)}}, nil, true},
		{"dial_member_not_bound", "client.Dial, the only member is not bound on the in-memory network", []ticket.Member{member(notBound)}, nil, true},
		{"dial_unexpected_reply", "client.Dial, the member answers view with ok", []ticket.Member{member(tokNode)}, []clientFake{{tokNode, tok}}, true},
		{"dial_remote_error", `client.Dial, the member answers view with TErr unavailable "no view"`, []ticket.Member{member(tokNode)},
			[]clientFake{{tokNode, func(*wire.Msg) *wire.Msg { return wire.ErrMsg(wire.CodeUnavailable, "no view") }}}, true},
		{"dial_bad_view", "client.Dial, the member answers view with the view bytes 80", []ticket.Member{member(tokNode)},
			[]clientFake{{tokNode, func(*wire.Msg) *wire.Msg {
				return &wire.Msg{Type: wire.TViewReply, Incarnation: 1, Epoch: 7, View: []byte{0x80}}
			}}}, true},
		{"dial_no_reply", "client.Dial, the member closes the stream without a frame", []ticket.Member{member(tokNode)},
			[]clientFake{{tokNode, func(*wire.Msg) *wire.Msg { return nil }}}, true},
		{"dial_last_member_error_wins", "client.Dial, the first member answers ok and the last is not bound", []ticket.Member{member(tokNode), member(notBound)},
			[]clientFake{{tokNode, tok}}, true},
	} {
		out, err := clientDialError(d.members, d.nodes, d.endpoint)
		if err != nil {
			return nil, fmt.Errorf("%s: %w", d.name, err)
		}
		c := clientTextCase{Name: d.name, Kind: "no_endpoint", Go: d.goText, Out: out}
		if d.endpoint {
			inner, ok := strings.CutPrefix(out, noBoot)
			if !ok {
				return nil, fmt.Errorf("%s: %q does not start with %q", d.name, out, noBoot)
			}
			c.Kind, c.Inner = "no_bootstrap", inner
		}
		add(c)
	}

	// Cluster calls against a one-node view.
	oneID := clientNodeID(0x4f4e45)
	oneView := &view.View{ClusterID: smData(0x434c55, 16), Incarnation: 1, Epoch: 7, Version: 9, PlacementEpoch: 7, Replicas: 1, MinReplicas: 1,
		Voters: []view.Voter{{ID: append([]byte{}, oneID[:]...), Since: 1}},
		Nodes:  []view.Node{{ID: append([]byte{}, oneID[:]...), Weight: 100, Writable: true}}}
	stamped := func(m wire.Msg) func(*wire.Msg) *wire.Msg {
		return func(*wire.Msg) *wire.Msg {
			r := m
			r.Incarnation, r.Epoch = oneView.Incarnation, oneView.Epoch
			return &r
		}
	}
	plain := func(code, text string) func(*wire.Msg) *wire.Msg {
		return func(*wire.Msg) *wire.Msg { return wire.ErrMsg(code, text) }
	}
	refGet := func(ctx context.Context, cl *client.Cluster) error { _, err := cl.RefGet(ctx, "trees/a"); return err }
	refPut := func(ctx context.Context, cl *client.Cluster) error {
		_, err := cl.RefPut(ctx, []byte{0xa0}, client.Cond{Versioned: true})
		return err
	}
	refDelete := func(ctx context.Context, cl *client.Cluster) error {
		return cl.RefDelete(ctx, "trees/a", client.Cond{Force: true})
	}
	refList := func(ctx context.Context, cl *client.Cluster) error { _, err := cl.RefList(ctx, "trees/"); return err }
	admin := func(ctx context.Context, cl *client.Cluster) error {
		_, err := cl.Admin(ctx, oneID, struct {
			Op string `cbor:"0,keyasint"`
		}{"gc-status"})
		return err
	}
	for _, c := range []struct {
		name, kind, goText string
		other              func(*wire.Msg) *wire.Msg
		call               func(ctx context.Context, cl *client.Cluster) error
		typ                int
		code, text         string
	}{
		{name: "ref_get_unknown_ref", kind: "unknown_ref", goText: `Cluster.RefGet, the node answers TErr unknown-ref "no such reference"`,
			other: plain(wire.CodeUnknownRef, "no such reference"), call: refGet},
		{name: "ref_get_unexpected_reply", kind: "unexpected_reply", goText: "Cluster.RefGet, the node answers ok", other: stamped(wire.Msg{Type: wire.TOK}), call: refGet, typ: wire.TOK},
		{name: "ref_put_unexpected_reply", kind: "unexpected_reply", goText: "Cluster.RefPut, the node answers refs", other: stamped(wire.Msg{Type: wire.TRefs}), call: refPut, typ: wire.TRefs},
		{name: "ref_delete_unexpected_reply", kind: "unexpected_reply", goText: "Cluster.RefDelete, the node answers ref", other: stamped(wire.Msg{Type: wire.TRef}), call: refDelete, typ: wire.TRef},
		{name: "ref_list_unexpected_reply", kind: "unexpected_reply", goText: "Cluster.RefList, the node answers ok", other: stamped(wire.Msg{Type: wire.TOK}), call: refList, typ: wire.TOK},
		{name: "admin_unexpected_reply", kind: "unexpected_reply", goText: "Cluster.Admin, the node answers ok", other: stamped(wire.Msg{Type: wire.TOK}), call: admin, typ: wire.TOK},
		{name: "ref_list_remote_error_not_mapped", kind: "remote", goText: `Cluster.RefList, the node answers TErr unknown-ref "no such reference"`,
			other: plain(wire.CodeUnknownRef, "no such reference"), call: refList, code: wire.CodeUnknownRef, text: "no such reference"},
		{name: "admin_remote_error", kind: "remote", goText: `Cluster.Admin, the node answers TErr bad-request "unknown admin op nope"`,
			other: plain(wire.CodeBadRequest, "unknown admin op nope"), call: admin, code: wire.CodeBadRequest, text: "unknown admin op nope"},
		{name: "ref_put_cas_mismatch", kind: "cas_mismatch", goText: "Cluster.RefPut, the node answers cas-mismatch with the current key k1",
			other: stamped(wire.Msg{Type: wire.TCASMismatch, HasCurrent: true, Current: k1[:], Record: []byte{0xa0}, Version: []byte{1, 2}}), call: refPut},
		{name: "ref_delete_cas_mismatch_absent", kind: "cas_mismatch", goText: "Cluster.RefDelete, the node answers cas-mismatch without a current record",
			other: stamped(wire.Msg{Type: wire.TCASMismatch, Version: []byte{1}}), call: refDelete},
		{name: "ref_put_incomplete", kind: "incomplete", goText: "Cluster.RefPut, the node answers incomplete with shortfall 7",
			other: stamped(wire.Msg{Type: wire.TIncomplete, Keys: [][]byte{k1[:]}, Shortfall: 7}), call: refPut},
	} {
		got, err := clientCallError(oneView, oneID, c.other, c.call)
		if err != nil {
			return nil, fmt.Errorf("%s: %w", c.name, err)
		}
		tc := clientTextCase{Name: c.name, Kind: c.kind, Go: c.goText, Code: c.code, Text: c.text, Out: got.Error()}
		if c.typ != 0 {
			tc.Type = intp(c.typ)
		}
		var cm *client.CASMismatch
		if errors.As(got, &cm) {
			tc.HasCurrent, tc.Current = boolp(cm.HasCurrent), Hex(cm.Current)
		}
		var inc *client.Incomplete
		if errors.As(got, &inc) {
			tc.Shortfall = intp(inc.Shortfall)
		}
		if (c.kind == "cas_mismatch") != (cm != nil) || (c.kind == "incomplete") != (inc != nil) {
			return nil, fmt.Errorf("%s: error %T %v is not of kind %s", c.name, got, got, c.kind)
		}
		add(tc)
	}

	// A view without nodes, and local packstores.
	tmp, err := os.MkdirTemp("", "vectorgen-client-")
	if err != nil {
		return nil, err
	}
	defer os.RemoveAll(tmp)
	openStore := func(name string) (*packstore.Store, error) {
		dir := filepath.Join(tmp, name)
		if err := os.MkdirAll(dir, 0o755); err != nil {
			return nil, err
		}
		return packstore.Open(dir)
	}
	srcDir := filepath.Join(tmp, "src")
	if err := os.MkdirAll(srcDir, 0o755); err != nil {
		return nil, err
	}
	treeStore, err := openStore("tree")
	if err != nil {
		return nil, err
	}
	defer treeStore.Close()
	root, _, err := ingest.Dir(treeStore, srcDir, ingest.Opts{Jobs: 1})
	if err != nil {
		return nil, err
	}
	emptyStore, err := openStore("empty")
	if err != nil {
		return nil, err
	}
	defer emptyStore.Close()
	bootID := clientNodeID(0x424f4f54)
	emptyView := &view.View{ClusterID: smData(0x434c55, 16), Incarnation: 1, Epoch: 7, Version: 9, PlacementEpoch: 7, Replicas: 3, MinReplicas: 2}
	reply, err := clientViewReplier(emptyView, nil, nil)
	if err != nil {
		return nil, err
	}
	err = clientWithCluster(bootID, reply, func(ctx context.Context, cl *client.Cluster) error {
		mr, err := cl.Missing(ctx, [][32]byte{k1}, false)
		if err != nil {
			return err
		}
		if mr.Failed[k1] == nil {
			return errors.New("Missing over a view without nodes: no failure")
		}
		add(clientTextCase{Name: "missing_no_owners", Kind: "no_owners", Go: "Cluster.Missing over a view without nodes: MissingResult.Failed[k]", Out: mr.Failed[k1].Error()})

		_, perr := cl.Push(ctx, treeStore, root, "trees/a", "alice", client.Cond{Force: true}, nil)
		if perr == nil {
			return errors.New("Push over a view without nodes succeeded")
		}
		inner, ok := strings.CutPrefix(perr.Error(), fmt.Sprintf("negotiate %x at its primary: ", root[:8]))
		if !ok {
			return fmt.Errorf("Push over a view without nodes: %v", perr)
		}
		add(clientTextCase{Name: "push_negotiate_no_owners", Kind: "negotiate", Go: "Cluster.Push of the empty tree over a view without nodes",
			Key: Hex(append([]byte{}, root[:8]...)), Inner: inner, Out: perr.Error()})

		_, perr = cl.Push(ctx, emptyStore, root, "trees/a", "alice", client.Cond{Force: true}, nil)
		if perr == nil {
			return errors.New("Push from an empty packstore succeeded")
		}
		inner, ok = strings.CutPrefix(perr.Error(), "walk local tree: ")
		if !ok {
			return fmt.Errorf("Push from an empty packstore: %v", perr)
		}
		add(clientTextCase{Name: "push_walk_local_tree", Kind: "walk_local_tree", Go: "Cluster.Push of the empty tree from an empty packstore", Inner: inner, Out: perr.Error()})

		perr = cl.PullTree(ctx, emptyStore, root, &client.PullStats{}, nil)
		if perr == nil {
			return errors.New("PullTree over a view without nodes succeeded")
		}
		if perr.Error() != fmt.Sprintf("pull: object %x not found in the cluster", root[:8]) {
			return fmt.Errorf("PullTree over a view without nodes: %v", perr)
		}
		add(clientTextCase{Name: "pull_tree_object_not_found", Kind: "pull_object_not_found", Go: "Cluster.PullTree of the empty tree over a view without nodes",
			Key: Hex(append([]byte{}, root[:8]...)), Out: perr.Error()})
		return nil
	})
	if err != nil {
		return nil, err
	}

	// Format strings and messages of unexported code (verified by clientSelfCheck).
	add(clientTextCase{Name: "no_nodes", Kind: "no_nodes", Go: `errors.New("client: no nodes") (anyNode)`, Out: errors.New("client: no nodes").Error()})
	add(clientTextCase{Name: "watch_idle", Kind: "watch_idle", Go: `errors.New("client: watch stream idle") (errWatchIdle)`, Out: errors.New("client: watch stream idle").Error()})
	for _, t := range []int{99, 7, 0} {
		add(clientTextCase{Name: fmt.Sprintf("unexpected_frame_%d", t), Kind: "unexpected_frame", Go: fmt.Sprintf(`fmt.Errorf("%%w: type %%d", wire.ErrProtocol, %d) (watchOnce)`, t),
			Type: intp(t), Out: fmt.Errorf("%w: type %d", wire.ErrProtocol, t).Error()})
	}
	for _, c := range []struct {
		name  string
		node  view.NodeID
		inner error
	}{
		{"upload_to_remote_busy", id1, &wire.Error{Code: wire.CodeBusy, Text: "x"}},
		{"upload_to_transport_closed", id2, transport.ErrClosed},
	} {
		e := fmt.Errorf("upload to %s: %w", view.ShortID(c.node), c.inner)
		add(clientTextCase{Name: c.name, Kind: "upload_to", Go: `fmt.Errorf("upload to %s: %w", view.ShortID(node), inner) (Push)`,
			Node: Hex(append([]byte{}, c.node[:]...)), Inner: c.inner.Error(), Out: e.Error()})
	}
	for _, c := range []struct{ name, reason string }{
		{"record_rejected_not_owner", "not-owner"},
		{"record_rejected_verify", "verify: amberpack: corrupt pack data: record CRC mismatch"},
	} {
		add(clientTextCase{Name: c.name, Kind: "record_rejected", Go: `fmt.Errorf("record %x rejected: %s", k[:8], reason) (Push)`,
			Key: Hex(append([]byte{}, k1[:8]...)), Reason: c.reason, Out: fmt.Errorf("record %x rejected: %s", k1[:8], c.reason).Error()})
	}
	notPlaced := func(count int, owners []view.NodeID, counts []int) (string, []string) {
		var names []string
		for i, id := range owners {
			names = append(names, fmt.Sprintf("%s (%d keys)", view.ShortID(id), counts[i]))
		}
		return fmt.Errorf("push: %d keys could not be placed; owners not confirming: %v", count, names).Error(), names
	}
	one, oneNames := notPlaced(5, []view.NodeID{id3}, []int{5})
	add(clientTextCase{Name: "not_placed_one_owner", Kind: "not_placed", Go: "shortError's format with one owner", Count: intp(5), Names: oneNames, Out: one})
	two, twoNames := notPlaced(6, []view.NodeID{id3, id4}, []int{5, 2})
	add(clientTextCase{Name: "not_placed_two_owners", Kind: "not_placed", Go: "shortError's format with two owners", Count: intp(6), Names: twoNames, Out: two})
	last := fmt.Errorf("upload to %s: %w", view.ShortID(id2), context.DeadlineExceeded)
	add(clientTextCase{Name: "not_placed_last_error", Kind: "not_placed_last_error", Go: `fmt.Errorf("%w; last upload error: %v", shortError, lastErr) (Push)`,
		Inner: one, Last: last.Error(), Out: fmt.Errorf("%w; last upload error: %v", errors.New(one), last).Error()})
	add(clientTextCase{Name: "fetch_ended_early", Kind: "fetch_ended_early", Go: `errors.New("pull: fetch ended early") (PullTree)`, Out: errors.New("pull: fetch ended early").Error()})
	missing := &fstree.MissingObjectError{Key: k1}
	add(clientTextCase{Name: "pull_incomplete", Kind: "pull_incomplete", Go: `fmt.Errorf("pull: tree incomplete after fetch: %w", &fstree.MissingObjectError{Key: k1}) (PullTree)`,
		Inner: missing.Error(), Out: fmt.Errorf("pull: tree incomplete after fetch: %w", missing).Error()})
	return f, nil
}

func init() {
	clientLiterals = append(clientLiterals,
		clientLiteral{"client/client.go", "*Cluster", "anyNode", "client: no nodes"},
		clientLiteral{"client/watch.go", "", "errWatchIdle", "client: watch stream idle"},
		clientLiteral{"client/watch.go", "*Cluster", "watchOnce", "%w: type %d"},
		clientLiteral{"client/tree.go", "*Cluster", "Push", "upload to %s: %w"},
		clientLiteral{"client/tree.go", "*Cluster", "Push", "record %x rejected: %s"},
		clientLiteral{"client/tree.go", "*Cluster", "Push", "%w; last upload error: %v"},
		clientLiteral{"client/tree.go", "*Cluster", "shortError", "%s (%d keys)"},
		clientLiteral{"client/tree.go", "*Cluster", "shortError", "push: %d keys could not be placed; owners not confirming: %v"},
		clientLiteral{"client/tree.go", "*Cluster", "PullTree", "pull: fetch ended early"},
		clientLiteral{"client/tree.go", "*Cluster", "PullTree", "pull: tree incomplete after fetch: %w"},
	)
}

// ---- family tables ----

func clientAllOutputs() []clientOutput {
	return []clientOutput{
		{"client/rank.json", genClientRank},
		{"client/batches.json", genClientBatches},
		{"client/fetch.json", genClientFetch},
		{"client/verify_record.json", genClientVerifyRecord},
		{"client/progress.json", genClientProgress},
		{"client/backoff.json", genClientBackoff},
		{"client/placement_decisions.json", genClientPlacementDecisions},
		{"errors/client_text.json", genClientText},
	}
}

func init() {
	clientCopies = append(clientCopies,
		clientCopy{"client/progress.go", "", "tracker"},
		clientCopy{"client/progress.go", "", "newTracker"},
		clientCopy{"client/progress.go", "*tracker", "observer"},
		clientCopy{"client/progress.go", "*tracker", "totals"},
		clientCopy{"client/progress.go", "*tracker", "more"},
		clientCopy{"client/progress.go", "*tracker", "objects"},
		clientCopy{"client/progress.go", "*tracker", "bytes"},
		clientCopy{"client/progress.go", "*tracker", "node"},
		clientCopy{"client/progress.go", "*tracker", "update"},
		clientCopy{"client/progress.go", "*tracker", "snapshot"},
		clientCopy{"client/progress.go", "", "countKeys"},
		clientCopy{"client/progress.go", "*Cluster", "pathAttrs"},
	)
	clientStmtCopies = append(clientStmtCopies,
		clientStmtCopy{"client/client.go", "*Cluster", "handleErr", "clientBackoff", 2},
		clientStmtCopy{"client/watch.go", "*Cluster", "WatchRefs", "clientWatchDelays", 1},
	)
	clientExprs = append(clientExprs,
		clientExpr{"client/watch.go", "*Cluster", "WatchRefs", "delay = min(2*delay, 30*time.Second)"},
		clientExpr{"client/watch.go", "*Cluster", "WatchRefs", "time.Duration(rand.Int64N(int64(delay/2)+1))"},
		clientExpr{"client/watch.go", "*Cluster", "WatchRefs", "delay = time.Second"},
		clientExpr{"client/watch.go", "*Cluster", "WatchRefs", "time.After(200 * time.Millisecond)"},
		clientExpr{"client/watch.go", "*Cluster", "WatchRefs", "context.WithTimeout(ctx, 15*time.Second)"},
		clientExpr{"client/watch.go", "*Cluster", "WatchRefs", `c.log.Warn("watch: no node answered, retrying", "pattern", pattern, "in", delay)`},
		clientExpr{"client/client.go", "", "Dial", "context.WithTimeout(ctx, 15*time.Second)"},
		clientExpr{"client/client.go", "*Cluster", "probeHinted", "context.WithTimeout(ctx, 3*time.Second)"},
	)
}

func init() {
	clientCopies = append(clientCopies,
		clientCopy{"client/fetch.go", "", "getBatchKeys"},
		clientCopy{"client/fetch.go", "", "getBatchBytes"},
		clientCopy{"client/fetch.go", "", "getEstMax"},
		clientCopy{"client/fetch.go", "", "estSize"},
		clientCopy{"client/fetch.go", "", "fetchKey"},
		clientCopy{"client/fetch.go", "", "fetchJob"},
		clientCopy{"client/fetch.go", "", "fetchAcc"},
		clientCopy{"client/fetch.go", "", "pickBatch"},
	)
}
