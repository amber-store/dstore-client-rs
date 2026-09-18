package main

// Family "admin" (admin/requests.json, admin/replies.json, status/status.json):
// node.AdminRequest per CLI subcommand (argv parsed by urfave/cli v2.27.7 with
// the flag definitions of cmd/dstore/main.go), node.AdminReply and what
// adminAction prints, node.Status variants and printStatus's node lines.
// Schemas: docs/vectorgen-proto.md.

import (
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"unicode/utf8"

	"encoding/hex"

	"github.com/amber-store/dstore/codec"
	"github.com/amber-store/dstore/node"
	"github.com/amber-store/dstore/ticket"
	"github.com/amber-store/dstore/view"
	"github.com/amber-store/dstore/wire"
	"github.com/urfave/cli/v2"
)

func init() {
	register("admin", []string{"admin/requests.json", "admin/replies.json", "status/status.json"}, genAdmin)
}

// admRequest is node.AdminRequest with every field present.
type admRequest struct {
	Op          string   `json:"op"`
	Node        Hex      `json:"node"`
	Weight      uint32   `json:"weight"`
	Zone        string   `json:"zone"`
	Replicas    uint8    `json:"replicas"`
	Dead        bool     `json:"dead"`
	AllowUnsafe bool     `json:"allow_unsafe"`
	Force       bool     `json:"force"`
	Key         Hex      `json:"key"`
	Garbage     string   `json:"garbage"`
	GarbageBits string   `json:"garbage_bits"`
	Tolerate    bool     `json:"tolerate"`
	Forwarded   bool     `json:"forwarded"`
	Pause       bool     `json:"pause"`
	Rate        U64      `json:"rate"`
	Names       []string `json:"names"`
}

func admRequestJSON(r node.AdminRequest) admRequest {
	g, bits := wvFloat(r.Garbage)
	return admRequest{Op: r.Op, Node: wvHexE(r.Node), Weight: r.Weight, Zone: r.Zone, Replicas: r.Replicas, Dead: r.Dead,
		AllowUnsafe: r.AllowUnsafe, Force: r.Force, Key: wvHexE(r.Key), Garbage: g, GarbageBits: bits, Tolerate: r.Tolerate,
		Forwarded: r.Forwarded, Pause: r.Pause, Rate: U64(r.Rate), Names: wvStrList(r.Names)}
}

// admReply is node.AdminReply with every field present.
type admReply struct {
	Text   string   `json:"text"`
	Token  Hex      `json:"token"`
	View   Hex      `json:"view"`
	Names  []string `json:"names"`
	Key    Hex      `json:"key"`
	Ticket string   `json:"ticket"`
	GC     Hex      `json:"gc"`
}

func admReplyJSON(r node.AdminReply) admReply {
	return admReply{Text: r.Text, Token: wvHexE(r.Token), View: wvHexE(r.View), Names: wvStrList(r.Names), Key: wvHexE(r.Key), Ticket: r.Ticket, GC: wvHexE(r.GC)}
}

type admVoterStat struct {
	ID       Hex `json:"id"`
	Calls    U64 `json:"calls"`
	Failures U64 `json:"failures"`
	P99ms    I64 `json:"p99ms"`
}

// admStatus is node.Status with every field present.
type admStatus struct {
	ID            Hex            `json:"id"`
	Epoch         U64            `json:"epoch"`
	Incarnation   U64            `json:"incarnation"`
	Packs         I64            `json:"packs"`
	Records       U64            `json:"records"`
	Bytes         I64            `json:"bytes"`
	Pins          I64            `json:"pins"`
	Unreachable   []Hex          `json:"unreachable"`
	PendingPacks  I64            `json:"pending_packs"`
	Transition    string         `json:"transition"`
	GC            string         `json:"gc"`
	LeaseHolder   Hex            `json:"lease_holder"`
	Voters        []admVoterStat `json:"voters"`
	Writable      bool           `json:"writable"`
	FreeBytes     I64            `json:"free_bytes"`
	TotalBytes    I64            `json:"total_bytes"`
	Puts          U64            `json:"puts"`
	Gets          U64            `json:"gets"`
	RefPuts       U64            `json:"ref_puts"`
	BytesIn       U64            `json:"bytes_in"`
	BytesOut      U64            `json:"bytes_out"`
	Amnesiac      bool           `json:"amnesiac"`
	Retired       bool           `json:"retired"`
	ScrubAgeSec   I64            `json:"scrub_age_sec"`
	LastLive      U64            `json:"last_live"`
	Corrupt       I64            `json:"corrupt"`
	IsHolder      bool           `json:"is_holder"`
	UnauditedKeys I64            `json:"unaudited_keys"`
	Watchers      I64            `json:"watchers"`
}

func admStatusJSON(s node.Status) admStatus {
	j := admStatus{ID: Hex(s.ID), Epoch: U64(s.Epoch), Incarnation: U64(s.Incarnation), Packs: I64(s.Packs), Records: U64(s.Records),
		Bytes: I64(s.Bytes), Pins: I64(s.Pins), Unreachable: wvHexList(s.Unreachable), PendingPacks: I64(s.PendingPacks),
		Transition: s.Transition, GC: s.GC, LeaseHolder: wvHexE(s.LeaseHolder), Voters: []admVoterStat{}, Writable: s.Writable,
		FreeBytes: I64(s.FreeBytes), TotalBytes: I64(s.TotalBytes), Puts: U64(s.Puts), Gets: U64(s.Gets), RefPuts: U64(s.RefPuts),
		BytesIn: U64(s.BytesIn), BytesOut: U64(s.BytesOut), Amnesiac: s.Amnesiac, Retired: s.Retired, ScrubAgeSec: I64(s.ScrubAgeSec),
		LastLive: U64(s.LastLive), Corrupt: I64(s.Corrupt), IsHolder: s.IsHolder, UnauditedKeys: I64(s.UnauditedKeys), Watchers: I64(s.Watchers)}
	for _, v := range s.Voters {
		j.Voters = append(j.Voters, admVoterStat{ID: Hex(v.ID), Calls: U64(v.Calls), Failures: U64(v.Failures), P99ms: I64(v.P99ms)})
	}
	return j
}

func genAdmin(out string) error {
	src, err := admDstoreSource()
	if err != nil {
		return err
	}
	if err := admCheckSource(src); err != nil {
		return err
	}
	if err := genAdminRequests(out); err != nil {
		return fmt.Errorf("requests: %w", err)
	}
	if err := genAdminReplies(out); err != nil {
		return fmt.Errorf("replies: %w", err)
	}
	return genAdminStatus(out)
}

// admDstoreSource reads cmd/dstore/{main,client}.go of the pinned module.
func admDstoreSource() (string, error) {
	cmd := exec.Command("go", "list", "-m", "-f", "{{.Dir}}", "github.com/amber-store/dstore")
	cmd.Stderr = os.Stderr
	dirb, err := cmd.Output()
	if err != nil {
		return "", fmt.Errorf("go list dstore: %w", err)
	}
	dir := strings.TrimSpace(string(dirb))
	var all strings.Builder
	for _, f := range []string{"main.go", "client.go"} {
		b, err := os.ReadFile(filepath.Join(dir, "cmd", "dstore", f))
		if err != nil {
			return "", err
		}
		all.Write(b)
	}
	return all.String(), nil
}

// admCheckSource fails when cmd/dstore no longer builds the requests, prints
// the replies or formats the node lines the way the copies below do.
func admCheckSource(src string) error {
	norm := func(s string) string { return strings.Join(strings.Fields(s), " ") }
	nsrc := norm(src)
	want := []string{
		`node.AdminRequest{Op: "cluster-ticket"}`,
		`node.AdminRequest{Op: "replicas", Replicas: uint8(r)}`,
		`r, err := strconv.ParseUint(c.Args().First(), 10, 8)`,
		`node.AdminRequest{Op: "token-create", Weight: uint32(c.Uint("weight"))}`,
		`&cli.UintFlag{Name: "weight", Usage: "weight the token imposes on the joiner (GiB)"}`,
		`node.AdminRequest{Op: "node-remove", Node: id, Dead: c.Bool("dead"), AllowUnsafe: c.Bool("allow-unsafe")}`,
		`node.AdminRequest{Op: "node-drain", Node: id}`,
		`w, err := strconv.ParseUint(c.Args().Get(1), 10, 32)`,
		`node.AdminRequest{Op: "node-weight", Node: id, Weight: uint32(w)}`,
		`node.AdminRequest{Op: "node-zone", Node: id, Zone: c.Args().Get(1)}`,
		`node.AdminRequest{Op: "node-repair", Node: id}`,
		`node.AdminRequest{Op: op, Node: id[:], AllowUnsafe: c.Bool("allow-unsafe")}`,
		`{Name: "add", ArgsUsage: "ID", Flags: clientFlags(), Action: act("voter-add")}`,
		`{Name: "remove", ArgsUsage: "ID", Flags: append(clientFlags(), &cli.BoolFlag{Name: "allow-unsafe"}), Action: act("voter-remove")}`,
		`node.AdminRequest{Op: op}`,
		`node.AdminRequest{Op: "gc-run", Tolerate: c.Bool("tolerate-missing"), Garbage: c.Float64("garbage")}`,
		`&cli.BoolFlag{Name: "tolerate-missing"}, &cli.Float64Flag{Name: "garbage", Usage: "re-sweep only, at this dead ratio"}`,
		`node.AdminRequest{Op: "gc-status"}`,
		`node.AdminRequest{Op: "gc-hold", Pause: true}`,
		`node.AdminRequest{Op: "gc-hold", Pause: false}`,
		`k, err := hex.DecodeString(c.Args().First()) if err != nil || len(k) != 32 {`,
		`node.AdminRequest{Op: "gc-why", Key: k}`,
		`node.AdminRequest{Op: "catalog-backup"}`,
		`node.AdminRequest{Op: "catalog-backups"}`,
		`&cli.BoolFlag{Name: "dead", Usage: "the node is gone: remove its vote first"}, &cli.BoolFlag{Name: "allow-unsafe"}`,
		`&cli.BoolFlag{Name: "yes", Usage: "do not ask"}`,
		`id, err := view.ParseNodeID(c.Args().First())`,
		`if r.Text != "" { fmt.Println(r.Text) } for _, n := range r.Names { fmt.Println(n) } if len(r.Key) == 32 { fmt.Printf("key %x\n", r.Key) }`,
		`fmt.Println(r.Text)`,
		`if t, err = ticket.Parse(r.Ticket); err != nil { return err }`,
		`line := fmt.Sprintf("  %s weight %d zone %q voter=%v writable=%v", view.IDString(id), nd.Weight, nd.Zone, v.IsVoter(id), nd.Writable)`,
		`fmt.Println(line, "— unreachable:", err)`,
		`fmt.Println(line, "— bad status")`,
		`fmt.Printf("      epoch %d packs %d records %d bytes %d pins %d pending-packs %d free %d GiB", st.Epoch, st.Packs, st.Records, st.Bytes, st.Pins, st.PendingPacks, st.FreeBytes>>30)`,
		`if st.IsHolder { fmt.Print(" [lease holder]") } if st.Amnesiac { fmt.Print(" [AMNESIAC]") } if st.Retired { fmt.Print(" [retired]") } fmt.Println()`,
		`ids = append(ids, node.ShortID(u))`,
		`fmt.Printf("      cannot reach: %s\n", strings.Join(ids, " "))`,
		`if st.GC != "" { fmt.Printf("      gc: %s\n", st.GC) }`,
		`if st.Transition != "" && st.Transition != "idle" { fmt.Printf("      transition: %s\n", st.Transition) }`,
		`fmt.Printf("      voter %s: %d calls, %d failures, p99 %d ms\n", node.ShortID(vs.ID), vs.Calls, vs.Failures, vs.P99ms)`,
	}
	for _, w := range want {
		if !strings.Contains(nsrc, norm(w)) {
			return fmt.Errorf("cmd/dstore source no longer contains %q: update family_admin.go", w)
		}
	}
	if n := strings.Count(src, "node.AdminRequest{"); n != 17 {
		return fmt.Errorf("cmd/dstore builds %d node.AdminRequest literals, want 17: update family_admin.go", n)
	}
	return nil
}

// admApp is the admin subset of cmd/dstore's command table: the same names,
// flags and argument handling; actions capture the request instead of dialing.
func admApp(got *node.AdminRequest) *cli.App {
	act := func(req node.AdminRequest) error { *got = req; return nil }
	idArg := func(c *cli.Context) ([]byte, error) {
		id, err := view.ParseNodeID(c.Args().First())
		if err != nil {
			return nil, err
		}
		return id[:], nil
	}
	voter := func(op string) cli.ActionFunc {
		return func(c *cli.Context) error {
			id, err := view.ParseNodeID(c.Args().First())
			if err != nil {
				return err
			}
			return act(node.AdminRequest{Op: op, Node: id[:], AllowUnsafe: c.Bool("allow-unsafe")})
		}
	}
	plain := func(op string) cli.ActionFunc {
		return func(c *cli.Context) error { return act(node.AdminRequest{Op: op}) }
	}
	return &cli.App{Name: "dstore", Writer: io.Discard, ErrWriter: io.Discard, Commands: []*cli.Command{
		{Name: "cluster", Subcommands: []*cli.Command{
			{Name: "ticket", Flags: []cli.Flag{&cli.BoolFlag{Name: "ids"}}, Action: plain("cluster-ticket")},
			{Name: "replicas", Flags: []cli.Flag{&cli.BoolFlag{Name: "yes"}}, Action: func(c *cli.Context) error {
				r, err := strconv.ParseUint(c.Args().First(), 10, 8)
				if err != nil {
					return errors.New("replicas R")
				}
				if !c.Bool("yes") {
					return errors.New("would prompt")
				}
				return act(node.AdminRequest{Op: "replicas", Replicas: uint8(r)})
			}},
		}},
		{Name: "token", Subcommands: []*cli.Command{
			{Name: "create", Flags: []cli.Flag{&cli.UintFlag{Name: "weight"}}, Action: func(c *cli.Context) error {
				return act(node.AdminRequest{Op: "token-create", Weight: uint32(c.Uint("weight"))})
			}},
		}},
		{Name: "node", Subcommands: []*cli.Command{
			{Name: "remove", Flags: []cli.Flag{&cli.BoolFlag{Name: "dead"}, &cli.BoolFlag{Name: "allow-unsafe"}}, Action: func(c *cli.Context) error {
				id, err := idArg(c)
				if err != nil {
					return err
				}
				return act(node.AdminRequest{Op: "node-remove", Node: id, Dead: c.Bool("dead"), AllowUnsafe: c.Bool("allow-unsafe")})
			}},
			{Name: "drain", Action: func(c *cli.Context) error {
				id, err := idArg(c)
				if err != nil {
					return err
				}
				return act(node.AdminRequest{Op: "node-drain", Node: id})
			}},
			{Name: "weight", Action: func(c *cli.Context) error {
				id, err := idArg(c)
				if err != nil {
					return err
				}
				w, err := strconv.ParseUint(c.Args().Get(1), 10, 32)
				if err != nil {
					return errors.New("weight ID GiB")
				}
				return act(node.AdminRequest{Op: "node-weight", Node: id, Weight: uint32(w)})
			}},
			{Name: "zone", Action: func(c *cli.Context) error {
				id, err := idArg(c)
				if err != nil {
					return err
				}
				return act(node.AdminRequest{Op: "node-zone", Node: id, Zone: c.Args().Get(1)})
			}},
			{Name: "repair", Action: func(c *cli.Context) error {
				id, err := idArg(c)
				if err != nil {
					return err
				}
				return act(node.AdminRequest{Op: "node-repair", Node: id})
			}},
		}},
		{Name: "voter", Subcommands: []*cli.Command{
			{Name: "add", Action: voter("voter-add")},
			{Name: "remove", Flags: []cli.Flag{&cli.BoolFlag{Name: "allow-unsafe"}}, Action: voter("voter-remove")},
		}},
		{Name: "transition", Subcommands: []*cli.Command{
			{Name: "status", Action: plain("transition-status")},
			{Name: "abort", Action: plain("transition-abort")},
			{Name: "refreeze", Action: plain("transition-refreeze")},
			{Name: "pause", Action: plain("transition-pause")},
			{Name: "resume", Action: plain("transition-resume")},
		}},
		{Name: "gc", Subcommands: []*cli.Command{
			{Name: "run", Flags: []cli.Flag{&cli.BoolFlag{Name: "tolerate-missing"}, &cli.Float64Flag{Name: "garbage"}}, Action: func(c *cli.Context) error {
				return act(node.AdminRequest{Op: "gc-run", Tolerate: c.Bool("tolerate-missing"), Garbage: c.Float64("garbage")})
			}},
			{Name: "status", Action: func(c *cli.Context) error { return act(node.AdminRequest{Op: "gc-status"}) }},
			{Name: "hold", Action: func(c *cli.Context) error { return act(node.AdminRequest{Op: "gc-hold", Pause: true}) }},
			{Name: "release", Action: func(c *cli.Context) error { return act(node.AdminRequest{Op: "gc-hold", Pause: false}) }},
			{Name: "why", Action: func(c *cli.Context) error {
				k, err := hex.DecodeString(c.Args().First())
				if err != nil || len(k) != 32 {
					return errors.New("why KEY (64 hex chars)")
				}
				return act(node.AdminRequest{Op: "gc-why", Key: k})
			}},
		}},
		{Name: "catalog", Subcommands: []*cli.Command{
			{Name: "backup", Action: func(c *cli.Context) error { return act(node.AdminRequest{Op: "catalog-backup"}) }},
			{Name: "backups", Action: func(c *cli.Context) error { return act(node.AdminRequest{Op: "catalog-backups"}) }},
		}},
	}}
}

type admRequestCase struct {
	Name      string     `json:"name"`
	Source    string     `json:"source"`
	Argv      []string   `json:"argv"`
	Request   admRequest `json:"request"`
	ParamsHex string     `json:"params_hex"`
	FrameHex  string     `json:"frame_hex"`
}

func genAdminRequests(out string) error {
	var cases []admRequestCase
	addReq := func(name, source string, argv []string, req node.AdminRequest) error {
		params := codec.MustMarshal(req)
		frame, err := wvFrame(wvStamp(&wire.Msg{Type: wire.TAdmin, Params: params}, wvSeq(0, 16), 1, 7))
		if err != nil {
			return err
		}
		cases = append(cases, admRequestCase{Name: name, Source: source, Argv: argv, Request: admRequestJSON(req), ParamsHex: hex.EncodeToString(params), FrameHex: hex.EncodeToString(frame)})
		return nil
	}
	id2 := strings.Repeat("ab", 32)
	idv := hex.EncodeToString(wvEdID(1))
	key32 := strings.Repeat("5a", 32)
	c38 := "cli §3.8"
	vf := "verification §5"
	argvs := []struct {
		name, source string
		argv         []string
	}{
		{"cluster-ticket", c38, []string{"cluster", "ticket"}},
		{"cluster-ticket-ids", vf, []string{"cluster", "ticket", "--ids"}},
		{"cluster-replicas-2", c38, []string{"cluster", "replicas", "--yes", "2"}},
		{"cluster-replicas-0", c38, []string{"cluster", "replicas", "--yes", "0"}},
		{"cluster-replicas-255", g1Source, []string{"cluster", "replicas", "--yes", "255"}},
		{"token-create", c38, []string{"token", "create"}},
		{"token-create-weight-100", c38, []string{"token", "create", "--weight", "100"}},
		{"token-create-weight-50", vf, []string{"token", "create", "--weight", "50"}},
		{"token-create-weight-max", g1Source, []string{"token", "create", "--weight", "4294967295"}},
		{"token-create-weight-2-pow-32-truncates", g1Source, []string{"token", "create", "--weight", "4294967296"}},
		{"node-remove-dead-allow-unsafe", c38, []string{"node", "remove", "--dead", "--allow-unsafe", id2}},
		{"node-remove", c38, []string{"node", "remove", id2}},
		{"node-remove-flags-after-id-ignored", "cli §2.2.4", []string{"node", "remove", id2, "--dead"}},
		{"node-drain", c38, []string{"node", "drain", id2}},
		{"node-weight-50", c38, []string{"node", "weight", id2, "50"}},
		{"node-weight-max", c38, []string{"node", "weight", id2, "4294967295"}},
		{"node-weight-300", vf, []string{"node", "weight", idv, "300"}},
		{"node-zone-rack-1", c38, []string{"node", "zone", id2, "rack-1"}},
		{"node-zone-empty", c38, []string{"node", "zone", id2}},
		{"node-zone-unicode", vf, []string{"node", "zone", idv, "zürich"}},
		{"node-repair", c38, []string{"node", "repair", id2}},
		{"voter-add", c38, []string{"voter", "add", id2}},
		{"voter-remove-allow-unsafe", c38, []string{"voter", "remove", "--allow-unsafe", id2}},
		{"voter-remove", vf, []string{"voter", "remove", idv}},
		{"transition-status", c38, []string{"transition", "status"}},
		{"transition-abort", c38, []string{"transition", "abort"}},
		{"transition-refreeze", c38, []string{"transition", "refreeze"}},
		{"transition-pause", c38, []string{"transition", "pause"}},
		{"transition-resume", c38, []string{"transition", "resume"}},
		{"gc-run", c38, []string{"gc", "run"}},
		{"gc-run-tolerate-missing", c38, []string{"gc", "run", "--tolerate-missing"}},
		{"gc-run-tolerate-and-garbage", "client-core §3.4", []string{"gc", "run", "--tolerate-missing", "--garbage", "0.5"}},
		{"gc-status", c38, []string{"gc", "status"}},
		{"gc-hold", c38, []string{"gc", "hold"}},
		{"gc-release", c38, []string{"gc", "release"}},
		{"gc-why", c38, []string{"gc", "why", key32}},
		{"gc-why-upper-hex", vf, []string{"gc", "why", strings.ToUpper(key32)}},
		{"catalog-backup", c38, []string{"catalog", "backup"}},
		{"catalog-backups", c38, []string{"catalog", "backups"}},
	}
	for _, g := range []string{"0", "-0", "0.5", "1.5", "0.1", "0.25", "0.3", "-1", "100", "100000", "65504", "65520", "1e-7", "1e-40",
		"3.4e38", "0.3333333333333333", "1.7976931348623157e308", "5e-324", "+Inf", "-Inf", "NaN"} {
		argvs = append(argvs, struct {
			name, source string
			argv         []string
		}{"gc-run-garbage-" + g, g1Source, []string{"gc", "run", "--garbage", g}})
	}
	for _, a := range argvs {
		var got node.AdminRequest
		if err := admApp(&got).Run(append([]string{"dstore"}, a.argv...)); err != nil {
			return fmt.Errorf("%s: %v", a.name, err)
		}
		if got.Op == "" {
			return fmt.Errorf("%s: no request built", a.name)
		}
		if err := addReq(a.name, a.source, a.argv, got); err != nil {
			return err
		}
	}
	extras := []struct {
		name string
		req  node.AdminRequest
	}{
		{"rate-cap", node.AdminRequest{Op: "rate-cap", Rate: 1 << 40}},
		{"rate-max", node.AdminRequest{Op: "rate-cap", Rate: 18446744073709551615}},
		{"keep-names", node.AdminRequest{Op: "keep", Names: []string{"trees/a", "", "bäume"}}},
		{"force-forwarded", node.AdminRequest{Op: "node-remove", Node: wvRep(0xab, 32), Force: true, Forwarded: true}},
		{"all-fields", node.AdminRequest{Op: "x", Node: []byte{1}, Weight: 2, Zone: "z", Replicas: 4, Dead: true, AllowUnsafe: true, Force: true,
			Key: []byte{8}, Garbage: 0.75, Tolerate: true, Forwarded: true, Pause: true, Rate: 13, Names: []string{"n"}}},
		{"empty-op", node.AdminRequest{}},
	}
	for _, w := range []uint32{23, 24, 255, 256, 65535, 65536} {
		extras = append(extras, struct {
			name string
			req  node.AdminRequest
		}{fmt.Sprintf("weight-%d", w), node.AdminRequest{Op: "token-create", Weight: w}})
	}
	for _, r := range []uint8{23, 24, 255} {
		extras = append(extras, struct {
			name string
			req  node.AdminRequest
		}{fmt.Sprintf("replicas-%d", r), node.AdminRequest{Op: "replicas", Replicas: r}})
	}
	for _, e := range extras {
		if err := addReq(e.name, "codec-wire-ticket §5 G1 (no CLI producer)", nil, e.req); err != nil {
			return err
		}
	}
	want := map[string]string{
		"cluster-ticket":                    "a1006e636c75737465722d7469636b6574",
		"token-create":                      "a1006c746f6b656e2d637265617465",
		"token-create-weight-100":           "a2006c746f6b656e2d637265617465021864",
		"cluster-replicas-2":                "a200687265706c696361730402",
		"cluster-replicas-0":                "a100687265706c69636173",
		"node-remove-dead-allow-unsafe":     "a4006b6e6f64652d72656d6f7665015820abababababababababababababababababababababababababababababababab05f506f5",
		"node-weight-max":                   "a3006b6e6f64652d776569676874015820abababababababababababababababababababababababababababababababab021affffffff",
		"node-zone-rack-1":                  "a300696e6f64652d7a6f6e65015820abababababababababababababababababababababababababababababababab03667261636b2d31",
		"voter-remove-allow-unsafe":         "a3006c766f7465722d72656d6f7665015820abababababababababababababababababababababababababababababababab06f5",
		"transition-refreeze":               "a100737472616e736974696f6e2d7265667265657a65",
		"gc-run-tolerate-missing":           "a2006667632d72756e0af5",
		"gc-run-garbage-0.25":               "a2006667632d72756e09f93400",
		"gc-run-garbage-0.1":                "a2006667632d72756e09fb3fb999999999999a",
		"gc-run-garbage-1.5":                "a2006667632d72756e09f93e00",
		"gc-run-garbage--1":                 "a2006667632d72756e09f9bc00",
		"gc-run-garbage-100000":             "a2006667632d72756e09fa47c35000",
		"gc-run-garbage-0.3333333333333333": "a2006667632d72756e09fb3fd5555555555555",
		"gc-run-tolerate-and-garbage":       "a3006667632d72756e09f938000af5",
		"gc-hold":                           "a2006767632d686f6c640cf5",
		"gc-release":                        "a1006767632d686f6c64",
		"gc-why":                            "a2006667632d7768790858205a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a",
		"catalog-backups":                   "a1006f636174616c6f672d6261636b757073",
		// The other rows of cli §3.8, and verification §3.2 / codec-wire-ticket §2.5.1.
		"node-remove":        "a2006b6e6f64652d72656d6f7665015820" + id2,
		"node-drain":         "a2006a6e6f64652d647261696e015820" + id2,
		"node-weight-50":     "a3006b6e6f64652d776569676874015820" + id2 + "021832",
		"node-zone-empty":    "a200696e6f64652d7a6f6e65015820" + id2,
		"node-repair":        "a2006b6e6f64652d726570616972015820" + id2,
		"voter-add":          "a20069766f7465722d616464015820" + id2,
		"transition-status":  "a100717472616e736974696f6e2d737461747573",
		"transition-abort":   "a100707472616e736974696f6e2d61626f7274",
		"transition-pause":   "a100707472616e736974696f6e2d7061757365",
		"transition-resume":  "a100717472616e736974696f6e2d726573756d65",
		"gc-run":             "a1006667632d72756e",
		"gc-status":          "a1006967632d737461747573",
		"catalog-backup":     "a1006e636174616c6f672d6261636b7570",
		"gc-run-garbage-0.5": "a2006667632d72756e09f93800",
		"gc-run-garbage-0.3": "a2006667632d72756e09fb3fd3333333333333",
	}
	for _, c := range cases {
		if w, ok := want[c.Name]; ok && c.ParamsHex != w {
			return fmt.Errorf("%s: params %s, want %s", c.Name, c.ParamsHex, w)
		}
		delete(want, c.Name)
	}
	if len(want) > 0 {
		return fmt.Errorf("spec request cases missing: %v", want)
	}
	return writeJSON(filepath.Join(out, "admin", "requests.json"), struct {
		Cases []admRequestCase `json:"cases"`
	}{cases})
}

// g1Source names the integer and float boundary lists of codec-wire-ticket §5 G1.
const g1Source = "codec-wire-ticket §5 G1"

// admPrinted is what adminAction prints for a reply (cmd/dstore/client.go:118-127).
func admPrinted(r node.AdminReply) string {
	var b strings.Builder
	if r.Text != "" {
		fmt.Fprintln(&b, r.Text)
	}
	for _, n := range r.Names {
		fmt.Fprintln(&b, n)
	}
	if len(r.Key) == 32 {
		fmt.Fprintf(&b, "key %x\n", r.Key)
	}
	return b.String()
}

type admClusterTicket struct {
	OK         bool   `json:"ok"`
	Error      string `json:"error,omitempty"`
	Printed    string `json:"printed,omitempty"`
	PrintedIDs string `json:"printed_ids,omitempty"`
}

type admReplyCase struct {
	Name               string            `json:"name"`
	Source             string            `json:"source"`
	Reply              admReply          `json:"reply"`
	StatusHex          string            `json:"status_hex"`
	FrameHex           string            `json:"frame_hex"`
	Printed            string            `json:"printed"`
	PrintedTokenCreate string            `json:"printed_token_create"`
	ClusterTicket      *admClusterTicket `json:"cluster_ticket,omitempty"`
}

type admReplyDecodeCase struct {
	Name      string    `json:"name"`
	StatusHex string    `json:"status_hex"`
	OK        bool      `json:"ok"`
	GoError   string    `json:"go_error,omitempty"`
	Reply     *admReply `json:"reply,omitempty"`
	Printed   string    `json:"printed,omitempty"`
}

func admClusterTicketOf(r node.AdminReply) *admClusterTicket {
	t, err := ticket.Parse(r.Ticket)
	if err != nil {
		return &admClusterTicket{Error: err.Error()}
	}
	return &admClusterTicket{OK: true, Printed: t.Encode() + "\n", PrintedIDs: t.IDs() + "\n"}
}

func genAdminReplies(out string) error {
	id1 := wvRep(0x01, 32)
	key32 := wvRep(0x5a, 32)
	nodeTicket := ticket.Ticket{ClusterID: wvSeq(0, 16), Incarnation: 1, Members: []ticket.Member{
		{ID: wvEdID(1), Addrs: []string{"ip:192.168.1.1:4433", "relay:https://use1-1.relay.n0.iroh-canary.iroh.link./"}},
		{ID: wvEdID(1), Addrs: []string{"ip:192.168.1.1:4433"}}, {ID: wvEdID(2), Addrs: []string{"ip:192.168.1.2:4433"}},
	}}
	c38 := "cli §3.8"
	vf := "verification §5"
	replies := []struct {
		name, source string
		r            node.AdminReply
	}{
		{"text-ok", c38, node.AdminReply{Text: "ok"}},
		{"names-2", c38, node.AdminReply{Names: []string{"trees/a", "trees/b"}}},
		{"key-and-text", c38, node.AdminReply{Key: key32, Text: "backup written"}},
		{"token", c38, node.AdminReply{Token: id1, Text: hex.EncodeToString(id1)}},
		{"empty", c38, node.AdminReply{}},
		{"short-key-names-text", c38, node.AdminReply{Key: []byte{1, 2, 3}, Names: []string{"x"}, Text: "t"}},
		{"names-3", vf, node.AdminReply{Names: []string{"trees/a", "trees/b", "bäume/c"}}},
		{"key-31", vf, node.AdminReply{Key: wvRep(0x5a, 31)}},
		{"key-33", vf, node.AdminReply{Key: wvRep(0x5a, 33)}},
		{"text-names-key", vf, node.AdminReply{Text: "t", Names: []string{"a", "b"}, Key: key32}},
		{"ticket", vf, node.AdminReply{Ticket: nodeTicket.Encode()}},
		{"ticket-ids-form", vf, node.AdminReply{Ticket: nodeTicket.IDs()}},
		{"ticket-invalid", vf, node.AdminReply{Ticket: "dstore1!!!"}},
		{"ticket-empty", vf, node.AdminReply{}},
		{"transition-proposed-view", "cli §2.7", node.AdminReply{View: wvCoreView(), Text: "transition proposed at epoch 8"}},
		{"transition-status-text", "cli §2.7", node.AdminReply{Text: "id 3 round 2 (replicas 2): 1/3 done, waiting for [ab12cd34 ef56ab78]"}},
		{"gc-status", "cli §2.7", node.AdminReply{GC: []byte{0xa1, 0x00, 0x03}, Text: "epoch 3 idle; last epoch 2: 10 live records, 2048 bytes freed"}},
		{"text-with-newline", vf, node.AdminReply{Text: "a\nb"}},
		{"names-empty-string", vf, node.AdminReply{Names: []string{""}}},
	}
	var cases []admReplyCase
	for _, rp := range replies {
		st := codec.MustMarshal(rp.r)
		frame, err := wvFrame(wvRStamp(&wire.Msg{Type: wire.TAdminReply, Status: st}, 1, 7))
		if err != nil {
			return err
		}
		c := admReplyCase{Name: rp.name, Source: rp.source, Reply: admReplyJSON(rp.r), StatusHex: hex.EncodeToString(st), FrameHex: hex.EncodeToString(frame),
			Printed: admPrinted(rp.r), PrintedTokenCreate: rp.r.Text + "\n"}
		if strings.HasPrefix(rp.name, "ticket") {
			c.ClusterTicket = admClusterTicketOf(rp.r)
		}
		cases = append(cases, c)
	}
	want := map[string]struct{ hex, printed string }{
		"text-ok":              {"a100626f6b", "ok\n"},
		"names-2":              {"a103826774726565732f616774726565732f62", "trees/a\ntrees/b\n"},
		"key-and-text":         {"a2006e6261636b7570207772697474656e0458205a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a", "backup written\nkey " + strings.Repeat("5a", 32) + "\n"},
		"token":                {"a2007840" + strings.Repeat("3031", 32) + "015820" + strings.Repeat("01", 32), strings.Repeat("01", 32) + "\n"},
		"empty":                {"a0", ""},
		"short-key-names-text": {"a3006174038161780443010203", "t\nx\n"},
	}
	for _, c := range cases {
		if w, ok := want[c.Name]; ok && (c.StatusHex != w.hex || c.Printed != w.printed) {
			return fmt.Errorf("reply %s: %s %q, want %s %q", c.Name, c.StatusHex, c.Printed, w.hex, w.printed)
		}
		delete(want, c.Name)
	}
	if len(want) > 0 {
		return fmt.Errorf("spec reply cases missing: %v", want)
	}
	var decodes []admReplyDecodeCase
	for _, d := range []struct{ name, h string }{
		{"unknown-key-99", "a200626f6b186301"},
		{"break", "ff"},
		{"null", "f6"},
		{"text-as-bytes", "a1004161"},
		{"duplicate-text", "a200626f6b006161"},
		{"names-null-element", "a10382f66161"},
		{"empty-payload", ""},
	} {
		raw, err := hex.DecodeString(d.h)
		if err != nil {
			return err
		}
		c := admReplyDecodeCase{Name: d.name, StatusHex: d.h}
		var r node.AdminReply
		if err := codec.Unmarshal(raw, &r); err != nil {
			c.GoError = err.Error()
		} else {
			j := admReplyJSON(r)
			c.OK, c.Reply, c.Printed = true, &j, admPrinted(r)
		}
		decodes = append(decodes, c)
	}
	return writeJSON(filepath.Join(out, "admin", "replies.json"), struct {
		Cases  []admReplyCase       `json:"cases"`
		Decode []admReplyDecodeCase `json:"decode"`
	}{cases, decodes})
}

// admNodeLines is printStatus's per-node output (cmd/dstore/client.go:148-190).
func admNodeLines(line string, b []byte, callErr error) string {
	var w strings.Builder
	if callErr != nil {
		fmt.Fprintln(&w, line, "— unreachable:", callErr)
		return w.String()
	}
	st, err := node.DecodeStatus(b)
	if err != nil {
		fmt.Fprintln(&w, line, "— bad status")
		return w.String()
	}
	fmt.Fprintln(&w, line)
	fmt.Fprintf(&w, "      epoch %d packs %d records %d bytes %d pins %d pending-packs %d free %d GiB", st.Epoch, st.Packs, st.Records, st.Bytes, st.Pins, st.PendingPacks, st.FreeBytes>>30)
	if st.IsHolder {
		fmt.Fprint(&w, " [lease holder]")
	}
	if st.Amnesiac {
		fmt.Fprint(&w, " [AMNESIAC]")
	}
	if st.Retired {
		fmt.Fprint(&w, " [retired]")
	}
	fmt.Fprintln(&w)
	if len(st.Unreachable) > 0 {
		var ids []string
		for _, u := range st.Unreachable {
			ids = append(ids, node.ShortID(u))
		}
		fmt.Fprintf(&w, "      cannot reach: %s\n", strings.Join(ids, " "))
	}
	if st.GC != "" {
		fmt.Fprintf(&w, "      gc: %s\n", st.GC)
	}
	if st.Transition != "" && st.Transition != "idle" {
		fmt.Fprintf(&w, "      transition: %s\n", st.Transition)
	}
	for _, vs := range st.Voters {
		fmt.Fprintf(&w, "      voter %s: %d calls, %d failures, p99 %d ms\n", node.ShortID(vs.ID), vs.Calls, vs.Failures, vs.P99ms)
	}
	return w.String()
}

type admNodeLine struct {
	ID       Hex    `json:"id"`
	Weight   uint32 `json:"weight"`
	Zone     string `json:"zone"`
	Voter    bool   `json:"voter"`
	Writable bool   `json:"writable"`
	Line     string `json:"line"`
}

func admLine(id []byte, weight uint32, zone string, voter, writable bool) admNodeLine {
	line := fmt.Sprintf("  %s weight %d zone %q voter=%v writable=%v", view.IDString(view.NodeID(id)), weight, zone, voter, writable)
	return admNodeLine{ID: Hex(id), Weight: weight, Zone: zone, Voter: voter, Writable: writable, Line: line}
}

type admStatusCase struct {
	Name        string      `json:"name"`
	Source      string      `json:"source"`
	Status      admStatus   `json:"status"`
	Hex         string      `json:"hex"`
	NullElement bool        `json:"null_element,omitempty"`
	Node        admNodeLine `json:"node"`
	Printed     string      `json:"printed"`
}

type admStatusDecodeCase struct {
	Name    string      `json:"name"`
	Hex     string      `json:"hex"`
	OK      bool        `json:"ok"`
	GoError string      `json:"go_error,omitempty"`
	Status  *admStatus  `json:"status,omitempty"`
	Node    admNodeLine `json:"node"`
	Printed string      `json:"printed"`
}

type admUnreachableCase struct {
	Name    string      `json:"name"`
	Node    admNodeLine `json:"node"`
	Error   string      `json:"error"`
	Printed string      `json:"printed"`
}

func genAdminStatus(out string) error {
	id1, id2 := wvRep(0x01, 32), wvRep(0xab, 32)
	probe := node.Status{ID: id2, Epoch: 5, Incarnation: 1, Packs: 2, Records: 10, Bytes: 1234, Pins: 1, Unreachable: [][]byte{id1},
		Transition: "idle", GC: "epoch 3 idle", Voters: []node.VoterStat{{ID: id1, Calls: 10, Failures: 1, P99ms: 12}}, Writable: true,
		FreeBytes: 5<<30 + 123, TotalBytes: 10 << 30, IsHolder: true}
	all := node.Status{ID: wvEdID(1), Epoch: 9, Incarnation: 2, Packs: 3, Records: 4, Bytes: 5, Pins: 6, Unreachable: [][]byte{wvEdID(2), wvEdID(3)},
		PendingPacks: 8, Transition: "id 3 (replicas 2): adopting, 1/3 acked", GC: "epoch 4 mark, 2 acked, ON HOLD", LeaseHolder: wvEdID(1),
		Voters:   []node.VoterStat{{ID: wvEdID(1), Calls: 100, Failures: 0, P99ms: 3}, {ID: wvEdID(2), Calls: 99, Failures: 7, P99ms: 250}},
		Writable: true, FreeBytes: 1 << 40, TotalBytes: 2 << 40, Puts: 16, Gets: 17, RefPuts: 18, BytesIn: 19, BytesOut: 20, Amnesiac: true,
		Retired: true, ScrubAgeSec: 3600, LastLive: 12345, Corrupt: 2, IsHolder: true, UnauditedKeys: 7, Watchers: 4}
	statuses := []struct {
		name, source string
		s            node.Status
		line         admNodeLine
	}{
		{"probe", "cli §3.8", probe, admLine(id2, 100, "rack-1", true, true)},
		{"zero", "cli §3.8", node.Status{}, admLine(id1, 0, "", false, false)},
		{"all-omitempty-set", "verification §5", all, admLine(wvEdID(1), 300, "zone \"a\"", true, true)},
		{"transition-empty-gc-empty", vfSource, node.Status{ID: id1, Epoch: 1, Transition: "", GC: ""}, admLine(id1, 1, "é", false, true)},
		{"transition-idle-hidden", vfSource, node.Status{ID: id1, Transition: "idle"}, admLine(id1, 1, "\x01", true, false)},
		{"free-bytes-below-1gib", vfSource, node.Status{ID: id1, FreeBytes: 1<<30 - 1}, admLine(id1, 1, "tab\t", false, false)},
		{"free-bytes-negative", vfSource, node.Status{ID: id1, FreeBytes: -1}, admLine(id1, 1, "­", false, false)},
		{"integer-boundaries", g1Source, node.Status{ID: id1, Epoch: 18446744073709551615, Incarnation: 1 << 32, Packs: -1, Records: 1<<64 - 1,
			Bytes: -9223372036854775808, Pins: 9223372036854775807, PendingPacks: 24, FreeBytes: 9223372036854775807, TotalBytes: -1,
			Puts: 23, Gets: 24, RefPuts: 255, BytesIn: 256, BytesOut: 65536, ScrubAgeSec: -1, LastLive: 18446744073709551615,
			Corrupt: -24, UnauditedKeys: 65535, Watchers: 1}, admLine(id1, 4294967295, "…", true, true)},
		{"voter-stat-variants", "codec-wire-ticket §5 G1", node.Status{ID: id1, Voters: []node.VoterStat{{}, {ID: []byte{}, P99ms: -5}, {ID: wvRep(9, 31), Calls: 1}}}, admLine(id1, 1, " ", true, true)},
		{"unreachable-odd-ids", "cli §2.7 node.ShortID", node.Status{ID: []byte{}, Unreachable: [][]byte{nil, wvRep(2, 31), id2}}, admLine(id1, 1, "", false, true)},
	}
	var cases []admStatusCase
	for _, s := range statuses {
		b := codec.MustMarshal(s.s)
		if !utf8.ValidString(s.line.Zone) {
			return fmt.Errorf("status %s: zone is not valid UTF-8 (a JSON string cannot hold it)", s.name)
		}
		cases = append(cases, admStatusCase{Name: s.name, Source: s.source, Status: admStatusJSON(s.s), Hex: hex.EncodeToString(b),
			NullElement: wvStatusNil(&s.s), Node: s.line, Printed: admNodeLines(s.line.Line, b, nil)})
	}
	if cases[0].Hex != "b5005820abababababababababababababababababababababababababababababababab010502010302040a051904d206010781582001010101010101010101010101010101010101010101010101010101010101010800096469646c650a6c65706f636820332069646c650c81a40058200101010101010101010101010101010101010101010101010101010101010101010a0201030c0df50e1b000000014000007b0f1b000000028000000010001100120013001400181af5" {
		return fmt.Errorf("probe status differs: %s", cases[0].Hex)
	}
	if cases[1].Hex != "b000f601000200030004000500060008000df40e000f0010001100120013001400" {
		return fmt.Errorf("zero status differs: %s", cases[1].Hex)
	}
	var decodes []admStatusDecodeCase
	for _, d := range []struct{ name, h string }{
		{"break", "ff"},
		{"empty-payload", ""},
		{"a0", "a0"},
		{"unknown-key", "a20105186301"},
		{"type-error", "a1014161"},
		{"id-null", "a100f6"},
	} {
		raw, err := hex.DecodeString(d.h)
		if err != nil {
			return err
		}
		line := admLine(id1, 100, "", false, true)
		c := admStatusDecodeCase{Name: d.name, Hex: d.h, Node: line, Printed: admNodeLines(line.Line, raw, nil)}
		st, err := node.DecodeStatus(raw)
		if err != nil {
			c.GoError = err.Error()
		} else {
			j := admStatusJSON(st)
			c.OK, c.Status = true, &j
		}
		decodes = append(decodes, c)
	}
	line := admLine(id1, 100, "rack-1", true, true)
	unreach := []admUnreachableCase{
		{Name: "remote-unavailable", Node: line, Error: (&wire.Error{Code: wire.CodeUnavailable, Text: "no view"}).Error()},
		{Name: "deadline", Node: line, Error: "context deadline exceeded"},
	}
	for i := range unreach {
		unreach[i].Printed = admNodeLines(unreach[i].Node.Line, nil, errors.New(unreach[i].Error))
	}
	return writeJSON(filepath.Join(out, "status", "status.json"), struct {
		Cases       []admStatusCase       `json:"cases"`
		Decode      []admStatusDecodeCase `json:"decode"`
		Unreachable []admUnreachableCase  `json:"unreachable"`
	}{cases, decodes, unreach})
}

// vfSource names verification.md §5.
const vfSource = "verification §5"
