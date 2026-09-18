// Package mainpkg holds verbatim copies of the github.com/amber-store/dstore
// v0.1.9 cmd/dstore declarations that the CLI vector families need. Package
// main cannot be imported, so the declarations are copied; SelfCheck
// (selfcheck.go) prints every declaration of this file with go/printer and
// compares it with the declaration of the same name in the module's
// cmd/dstore sources, so the copies cannot drift. This file holds nothing
// but copies: exported wrappers live in api.go. Each copy is preceded by a
// line naming its source file and lines.
package mainpkg

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"math"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"time"

	"charm.land/bubbles/v2/progress"
	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"
	"github.com/amber-store/dstore/client"
	"github.com/amber-store/dstore/view"
	"github.com/amber-store/dstore/worktree"
	"github.com/urfave/cli/v2"
)

// ---- cmd/dstore/main.go:54-64 ----
func logLevel(c *cli.Context) slog.Level {
	switch strings.ToLower(c.String("log-level")) {
	case "debug":
		return slog.LevelDebug
	case "warn":
		return slog.LevelWarn
	case "error":
		return slog.LevelError
	}
	return slog.LevelInfo
}

// ---- cmd/dstore/main.go:72-74 ----
func storeFlag() cli.Flag {
	return &cli.StringFlag{Name: "store", Usage: "store directory", EnvVars: []string{"DSTORE_STORE"}}
}

// ---- cmd/dstore/main.go:90-92 ----
func noDiscoveryFlag() cli.Flag {
	return &cli.BoolFlag{Name: "no-discovery", Usage: "neither announce this endpoint nor resolve node ids by discovery (mDNS and, with relays, number0's DNS)", EnvVars: []string{"DSTORE_NO_DISCOVERY"}}
}

// ---- cmd/dstore/main.go:94-103 ----
func netFlags() []cli.Flag {
	return []cli.Flag{
		&cli.StringFlag{Name: "relay", Usage: "relay URL for the fallback path (default: the built-in relay map)"},
		&cli.BoolFlag{Name: "no-relay", Usage: "direct addresses only, no relay"},
		&cli.StringSliceFlag{Name: "advertise-addr", Usage: "direct address to advertise, ip or ip:port (repeatable)"},
		&cli.BoolFlag{Name: "loopback", Usage: "advertise 127.0.0.1 only (single-machine tests)"},
		&cli.StringFlag{Name: "bind", Usage: "UDP address to bind, ip:port"},
		noDiscoveryFlag(),
	}
}

// ---- cmd/dstore/main.go:105-117 ----
func nodeFlags() []cli.Flag {
	return append([]cli.Flag{
		storeFlag(),
		&cli.StringFlag{Name: "paxos-dir", Usage: "acceptor state directory (default <store>/paxos; put it on its own device)"},
		&cli.Int64Flag{Name: "rate", Usage: "reconcile copy rate in bytes/s (0 = unlimited)"},
		&cli.IntFlag{Name: "jobs", Usage: "parallelism (0 = cores)"},
		&cli.Int64Flag{Name: "min-free", Usage: "free bytes below which uploads are refused (0 = 5% or 100 GiB)"},
		&cli.StringFlag{Name: "pack-size", Value: defaultPackSize, Usage: "size at which the active pack is sealed (bytes or Ki/Mi/Gi/Ti); applies to packs written from now on", EnvVars: []string{"DSTORE_PACK_SIZE"}},
		&cli.BoolFlag{Name: "gateway", Usage: "also serve the transport-iroh ALPN (not implemented in this version)"},
		&cli.DurationFlag{Name: "gc-interval", Value: 4 * time.Hour},
		&cli.DurationFlag{Name: "put-ttl", Value: time.Hour},
	}, netFlags()...)
}

// ---- cmd/dstore/main.go:209-210 ----
// defaultPackSize is the --pack-size default, node.DefaultSegmentSize.
const defaultPackSize = "2Gi"

// ---- cmd/dstore/main.go:212-227 ----
// packSize reads --pack-size. An empty value (an exported but empty
// $DSTORE_PACK_SIZE) means the default.
func packSize(c *cli.Context) (int64, error) {
	v := strings.TrimSpace(c.String("pack-size"))
	if v == "" {
		v = defaultPackSize
	}
	n, err := parseSize(v)
	if err != nil {
		return 0, fmt.Errorf("--pack-size: %w", err)
	}
	if n <= 0 {
		return 0, fmt.Errorf("--pack-size: %d is not a positive size", n)
	}
	return n, nil
}

// ---- cmd/dstore/size.go:10-46 ----
// parseSize parses a byte count such as "1048576", "512Mi" or "2Gi". The
// suffixes K, M, G and T are binary (1024-based) whether or not they carry
// an "i", and a trailing "B" is ignored, so "2Gi", "2GiB", "2G" and "2GB"
// all mean 2 GiB.
func parseSize(s string) (int64, error) {
	s = strings.TrimSpace(s)
	i := 0
	for i < len(s) && s[i] >= '0' && s[i] <= '9' {
		i++
	}
	if i == 0 {
		return 0, fmt.Errorf("bad size %q: want a number with an optional Ki/Mi/Gi/Ti suffix", s)
	}
	n, err := strconv.ParseInt(s[:i], 10, 64)
	if err != nil {
		return 0, fmt.Errorf("bad size %q: %v", s, err)
	}
	unit := strings.TrimSuffix(strings.ToLower(s[i:]), "b")
	var shift uint
	switch unit {
	case "":
	case "k", "ki":
		shift = 10
	case "m", "mi":
		shift = 20
	case "g", "gi":
		shift = 30
	case "t", "ti":
		shift = 40
	default:
		return 0, fmt.Errorf("bad size %q: unknown unit %q (want Ki, Mi, Gi or Ti)", s, s[i:])
	}
	if n > math.MaxInt64>>shift {
		return 0, fmt.Errorf("bad size %q: too large", s)
	}
	return n << shift, nil
}

// ---- cmd/dstore/client.go:554-574 ----
func hexDecode(s string) ([]byte, error) {
	b := make([]byte, len(s)/2)
	for i := 0; i < len(b); i++ {
		var v byte
		for j := 0; j < 2; j++ {
			ch := s[2*i+j]
			switch {
			case ch >= '0' && ch <= '9':
				v = v<<4 | (ch - '0')
			case ch >= 'a' && ch <= 'f':
				v = v<<4 | (ch - 'a' + 10)
			case ch >= 'A' && ch <= 'F':
				v = v<<4 | (ch - 'A' + 10)
			default:
				return nil, fmt.Errorf("bad hex %q", s)
			}
		}
		b[i] = v
	}
	return b, nil
}

// ---- cmd/dstore/wc.go:38-47 ----
// resolveTicket applies the precedence: the flag, the stored ticket, the
// environment.
func resolveTicket(flag, stored, env string) (string, error) {
	for _, s := range []string{flag, stored, env} {
		if s != "" {
			return s, nil
		}
	}
	return "", errors.New("no cluster: set --ticket or $DSTORE_TICKET")
}

// ---- cmd/dstore/wc.go:391-405 ----
// describeChange renders a status line's path with its detail: a trailing
// slash for directories, the old and new type or mode.
func describeChange(ch worktree.Change) string {
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

// ---- cmd/dstore/wc.go:464-489 ----
// filterPaths keeps the changes at or below the given paths, which are
// relative to the current directory.
func filterPaths(root string, changes []worktree.Change, args []string) ([]worktree.Change, error) {
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

// ---- cmd/dstore/tui.go:43-48 ----
// latest keeps the newest progress report for a renderer to pick up; the
// client calls set for every record, so nothing else happens here.
type latest struct {
	mu  sync.Mutex
	rep client.ProgressReport
}

// ---- cmd/dstore/tui.go:50-54 ----
func (l *latest) set(r client.ProgressReport) {
	l.mu.Lock()
	l.rep = r
	l.mu.Unlock()
}

// ---- cmd/dstore/tui.go:56-60 ----
func (l *latest) get() client.ProgressReport {
	l.mu.Lock()
	defer l.mu.Unlock()
	return l.rep
}

// ---- cmd/dstore/tui.go:88-92 ----
// rateMeter measures throughput over a sliding window.
type rateMeter struct {
	window  time.Duration
	samples []rateSample
}

// ---- cmd/dstore/tui.go:94-97 ----
type rateSample struct {
	t time.Time
	n int64
}

// ---- cmd/dstore/tui.go:99-99 ----
func newRateMeter(window time.Duration) *rateMeter { return &rateMeter{window: window} }

// ---- cmd/dstore/tui.go:101-114 ----
// add records n bytes moved by t and returns the rate over the window in
// bytes per second.
func (m *rateMeter) add(t time.Time, n int64) float64 {
	m.samples = append(m.samples, rateSample{t, n})
	for len(m.samples) > 2 && t.Sub(m.samples[1].t) >= m.window {
		m.samples = m.samples[1:]
	}
	first := m.samples[0]
	d := t.Sub(first.t)
	if d <= 0 || n < first.n {
		return 0
	}
	return float64(n-first.n) / d.Seconds()
}

// ---- cmd/dstore/tui.go:116-130 ----
// statusLine summarises a report: objects, bytes, rate and time left.
func statusLine(r client.ProgressReport, rate float64) string {
	s := fmt.Sprintf("%d/%d objects", r.Objects, r.TotalObjects)
	if r.TotalBytes > 0 {
		s += fmt.Sprintf("  %s / %s", client.HumanBytes(r.Bytes), client.HumanBytes(r.TotalBytes))
	} else if r.Bytes > 0 {
		s += "  " + client.HumanBytes(r.Bytes)
	}
	s += fmt.Sprintf("  %s/s", client.HumanBytes(int64(rate)))
	if left := r.TotalBytes - r.Bytes; rate > 0 && left > 0 {
		eta := time.Duration(float64(left) / rate * float64(time.Second)).Round(time.Second)
		s += "  eta " + eta.String()
	}
	return s
}

// ---- cmd/dstore/tui.go:132-143 ----
// fraction is the completed share of a transfer: by bytes once the total
// is known, by objects before that.
func fraction(r client.ProgressReport) float64 {
	var f float64
	switch {
	case r.TotalBytes > 0:
		f = float64(r.Bytes) / float64(r.TotalBytes)
	case r.TotalObjects > 0:
		f = float64(r.Objects) / float64(r.TotalObjects)
	}
	return min(max(f, 0), 1)
}

// ---- cmd/dstore/tui.go:145-154 ----
// Messages of the transfer UI.
type (
	tickMsg  time.Time
	eventMsg struct {
		at    time.Time
		level slog.Level
		text  string
	}
	doneMsg struct{ err error }
)

// ---- cmd/dstore/tui.go:156-159 ----
const (
	maxEvents = 12
	tickEvery = 100 * time.Millisecond
)

// ---- cmd/dstore/tui.go:161-167 ----
var (
	titleStyle = lipgloss.NewStyle().Bold(true)
	faintStyle = lipgloss.NewStyle().Faint(true)
	warnStyle  = lipgloss.NewStyle().Foreground(lipgloss.Color("3"))
	errStyle   = lipgloss.NewStyle().Foreground(lipgloss.Color("1"))
	okStyle    = lipgloss.NewStyle().Foreground(lipgloss.Color("2"))
)

// ---- cmd/dstore/tui.go:169-188 ----
// uiModel is the Bubble Tea model of a transfer: a progress bar, a status
// line, a per-node table and the last few client events.
type uiModel struct {
	title      string
	start      time.Time
	now        time.Time
	width      int
	bar        progress.Model
	l          *latest
	meter      *rateMeter
	nodeMeters map[view.NodeID]*rateMeter
	rep        client.ProgressReport
	rate       float64
	nodeRates  map[view.NodeID]float64
	events     []eventMsg
	cancel     context.CancelFunc
	cancelling bool
	done       bool
	err        error
}

// ---- cmd/dstore/tui.go:190-198 ----
func newUIModel(title string, l *latest, cancel context.CancelFunc) uiModel {
	now := time.Now()
	bar := progress.New(progress.WithDefaultBlend())
	bar.SetWidth(60)
	return uiModel{
		title: title, start: now, now: now, width: 80, bar: bar, l: l, cancel: cancel,
		meter: newRateMeter(5 * time.Second), nodeMeters: map[view.NodeID]*rateMeter{}, nodeRates: map[view.NodeID]float64{},
	}
}

// ---- cmd/dstore/tui.go:200-202 ----
func tick() tea.Cmd {
	return tea.Tick(tickEvery, func(t time.Time) tea.Msg { return tickMsg(t) })
}

// ---- cmd/dstore/tui.go:204-204 ----
func (m uiModel) Init() tea.Cmd { return tick() }

// ---- cmd/dstore/tui.go:206-228 ----
func (m uiModel) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		m.width = msg.Width
		m.bar.SetWidth(min(max(msg.Width-4, 20), 80))
	case tea.KeyPressMsg:
		if msg.String() == "ctrl+c" && !m.cancelling {
			m.cancelling = true
			m.cancel()
			m.events = appendEvent(m.events, eventMsg{at: time.Now(), level: slog.LevelWarn, text: "cancelling"})
		}
	case tickMsg:
		m.observe(time.Time(msg))
		return m, tick()
	case eventMsg:
		m.events = appendEvent(m.events, msg)
	case doneMsg:
		m.done, m.err = true, msg.err
		m.observe(time.Now())
		return m, tea.Quit
	}
	return m, nil
}

// ---- cmd/dstore/tui.go:230-243 ----
// observe takes the latest report and updates the rates.
func (m *uiModel) observe(now time.Time) {
	m.now = now
	m.rep = m.l.get()
	m.rate = m.meter.add(now, m.rep.Bytes)
	for _, n := range m.rep.Nodes {
		nm := m.nodeMeters[n.ID]
		if nm == nil {
			nm = newRateMeter(5 * time.Second)
			m.nodeMeters[n.ID] = nm
		}
		m.nodeRates[n.ID] = nm.add(now, n.Bytes)
	}
}

// ---- cmd/dstore/tui.go:245-251 ----
func appendEvent(events []eventMsg, e eventMsg) []eventMsg {
	events = append(events, e)
	if len(events) > maxEvents {
		events = events[len(events)-maxEvents:]
	}
	return events
}

// ---- cmd/dstore/tui.go:253-287 ----
func (m uiModel) View() tea.View {
	var b strings.Builder
	elapsed := m.now.Sub(m.start).Round(time.Second)
	fmt.Fprintf(&b, "%s  %s\n", titleStyle.Render(m.title), faintStyle.Render("elapsed "+elapsed.String()))
	b.WriteString(m.bar.ViewAs(fraction(m.rep)) + "\n")
	b.WriteString(statusLine(m.rep, m.rate) + "\n")
	if len(m.rep.Nodes) > 0 {
		b.WriteString(faintStyle.Render(fmt.Sprintf("%-10s %-7s %7s %7s %-15s %11s %12s", "node", "path", "rtt", "batches", "state", "sent", "rate")) + "\n")
		for _, n := range m.rep.Nodes {
			path := "relay"
			if n.Direct {
				path = "direct"
			}
			rtt := "-"
			if n.RTT > 0 {
				rtt = n.RTT.Round(time.Millisecond).String()
			}
			fmt.Fprintf(&b, "%-10s %-7s %7s %7d %-15s %11s %10s/s\n", view.ShortID(n.ID), path, rtt, n.InFlight, nodeState(n), client.HumanBytes(n.Bytes), client.HumanBytes(int64(m.nodeRates[n.ID])))
		}
	}
	if len(m.events) > 0 {
		b.WriteString(faintStyle.Render("events") + "\n")
		for _, e := range m.events {
			b.WriteString(formatEvent(e) + "\n")
		}
	}
	if m.done {
		if m.err != nil {
			b.WriteString(errStyle.Render("failed: "+m.err.Error()) + "\n")
		} else {
			b.WriteString(okStyle.Render("done") + "\n")
		}
	}
	return tea.NewView(b.String())
}

// ---- cmd/dstore/tui.go:289-298 ----
// nodeState names what the client is doing with a node right now.
func nodeState(n client.NodeProgress) string {
	switch {
	case n.InFlight == 0:
		return "idle"
	case n.Awaiting == n.InFlight:
		return "waiting for ack"
	}
	return "sending"
}

// ---- cmd/dstore/tui.go:300-309 ----
func formatEvent(e eventMsg) string {
	line := e.at.Format("15:04:05") + "  " + e.text
	switch {
	case e.level >= slog.LevelError:
		return errStyle.Render(line)
	case e.level >= slog.LevelWarn:
		return warnStyle.Render(line)
	}
	return line
}

// ---- cmd/dstore/tui.go:335-340 ----
// teaHandler is a slog.Handler that turns log records into UI events.
type teaHandler struct {
	level slog.Level
	send  func(tea.Msg)
	attrs []slog.Attr
}

// ---- cmd/dstore/tui.go:342-342 ----
func (h *teaHandler) Enabled(_ context.Context, l slog.Level) bool { return l >= h.level }

// ---- cmd/dstore/tui.go:344-357 ----
func (h *teaHandler) Handle(_ context.Context, r slog.Record) error {
	var b strings.Builder
	b.WriteString(r.Message)
	write := func(a slog.Attr) bool {
		b.WriteString(" " + a.Key + "=" + attrValue(a))
		return true
	}
	for _, a := range h.attrs {
		write(a)
	}
	r.Attrs(write)
	h.send(eventMsg{at: r.Time, level: r.Level, text: b.String()})
	return nil
}

// ---- cmd/dstore/tui.go:359-361 ----
func (h *teaHandler) WithAttrs(attrs []slog.Attr) slog.Handler {
	return &teaHandler{level: h.level, send: h.send, attrs: append(append([]slog.Attr{}, h.attrs...), attrs...)}
}

// ---- cmd/dstore/tui.go:363-363 ----
func (h *teaHandler) WithGroup(string) slog.Handler { return h }

// ---- cmd/dstore/tui.go:365-374 ----
func attrValue(a slog.Attr) string {
	if a.Key == "bytes" && a.Value.Kind() == slog.KindInt64 {
		return client.HumanBytes(a.Value.Int64())
	}
	s := a.Value.String()
	if strings.ContainsAny(s, " \t") {
		return fmt.Sprintf("%q", s)
	}
	return s
}
