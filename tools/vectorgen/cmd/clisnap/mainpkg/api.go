package mainpkg

import (
	"fmt"
	"image/color"
	"io"
	"log/slog"
	"os"
	"strings"
	"time"

	"charm.land/bubbles/v2/progress"
	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"
	"github.com/amber-store/dstore/client"
	"github.com/amber-store/dstore/worktree"
	"github.com/urfave/cli/v2"
)

// Exported entry points over the copies, for tools/vectorgen/family_cli.go.
// Everything here calls the copied declarations or configures library values
// the way cmd/dstore does. Three pieces restate cmd/dstore code that the
// self-check cannot compare, because they sit inside larger functions:
// StatusChangeLine (the change line of statusCmd, wc.go), ProgressBar (the bar
// of newUIModel, tui.go) and the urfave apps of PackSize and LogLevel (an app
// over nodeFlags(), a lone --log-level flag).

// ParseSize calls parseSize (size.go).
func ParseSize(s string) (int64, error) { return parseSize(s) }

// SizeResult is packSize's outcome.
type SizeResult struct {
	N   int64
	Err error
}

// PackSize runs packSize (main.go) as the action of a cli.App over
// nodeFlags(), as size_test.go TestPackSizeFlag does. flag nil means no
// --pack-size argument; env nil means $DSTORE_PACK_SIZE unset. The returned
// error reports a failure of the harness itself, never packSize's error.
func PackSize(flag, env *string) (SizeResult, error) {
	var res SizeResult
	ran := false
	err := withEnv("DSTORE_PACK_SIZE", env, func() error {
		args := []string{"dstore"}
		if flag != nil {
			args = append(args, "--pack-size", *flag)
		}
		app := &cli.App{Flags: nodeFlags(), Writer: io.Discard, ErrWriter: io.Discard, Action: func(c *cli.Context) error {
			ran = true
			res.N, res.Err = packSize(c)
			return nil
		}}
		return app.Run(args)
	})
	if err != nil {
		return res, fmt.Errorf("pack-size app: %w", err)
	}
	if !ran {
		return res, fmt.Errorf("pack-size app: the action did not run")
	}
	return res, nil
}

// LogLevel runs logLevel (main.go) with --log-level set to value.
func LogLevel(value string) (slog.Level, error) {
	var level slog.Level
	ran := false
	app := &cli.App{Flags: []cli.Flag{&cli.StringFlag{Name: "log-level"}}, Writer: io.Discard, ErrWriter: io.Discard, Action: func(c *cli.Context) error {
		ran = true
		level = logLevel(c)
		return nil
	}}
	if err := app.Run([]string{"dstore", "--log-level", value}); err != nil {
		return 0, fmt.Errorf("log-level app: %w", err)
	}
	if !ran {
		return 0, fmt.Errorf("log-level app: the action did not run")
	}
	return level, nil
}

// withEnv runs f with the variable set to *value, or unset when value is nil,
// and restores it afterwards.
func withEnv(name string, value *string, f func() error) error {
	old, had := os.LookupEnv(name)
	var err error
	if value == nil {
		err = os.Unsetenv(name)
	} else {
		err = os.Setenv(name, *value)
	}
	if err != nil {
		return err
	}
	ferr := f()
	if had {
		err = os.Setenv(name, old)
	} else {
		err = os.Unsetenv(name)
	}
	if ferr != nil {
		return ferr
	}
	return err
}

// HexDecode calls hexDecode (client.go).
func HexDecode(s string) ([]byte, error) { return hexDecode(s) }

// ResolveTicket calls resolveTicket (wc.go).
func ResolveTicket(flag, stored, env string) (string, error) {
	return resolveTicket(flag, stored, env)
}

// DescribeChange calls describeChange (wc.go).
func DescribeChange(ch worktree.Change) string { return describeChange(ch) }

// StatusChangeLine is the `status` change line of wc.go (fmt.Printf("  %-9s %s\n", ch.Kind, describeChange(ch))), without the newline.
func StatusChangeLine(ch worktree.Change) string {
	return fmt.Sprintf("  %-9s %s", ch.Kind, describeChange(ch))
}

// FilterPaths calls filterPaths (wc.go).
func FilterPaths(root string, changes []worktree.Change, args []string) ([]worktree.Change, error) {
	return filterPaths(root, changes, args)
}

// StatusLine calls statusLine (tui.go).
func StatusLine(r client.ProgressReport, rate float64) string { return statusLine(r, rate) }

// Fraction calls fraction (tui.go).
func Fraction(r client.ProgressReport) float64 { return fraction(r) }

// NodeState calls nodeState (tui.go).
func NodeState(n client.NodeProgress) string { return nodeState(n) }

// RateMeter wraps rateMeter (tui.go).
type RateMeter struct{ m *rateMeter }

// NewRateMeter calls newRateMeter.
func NewRateMeter(window time.Duration) *RateMeter { return &RateMeter{m: newRateMeter(window)} }

// Add calls add.
func (r *RateMeter) Add(t time.Time, n int64) float64 { return r.m.add(t, n) }

// Samples is the number of samples the meter keeps.
func (r *RateMeter) Samples() int { return len(r.m.samples) }

// TeaEvent is one eventMsg a teaHandler sent.
type TeaEvent struct {
	At    time.Time
	Level slog.Level
	Text  string
}

// TeaLogger returns a logger over a teaHandler at level (tui.go), and a
// function returning the events it has sent so far.
func TeaLogger(level slog.Level) (*slog.Logger, func() []TeaEvent) {
	var got []TeaEvent
	h := &teaHandler{level: level, send: func(m tea.Msg) {
		if e, ok := m.(eventMsg); ok {
			got = append(got, TeaEvent{At: e.at, Level: e.level, Text: e.text})
		}
	}}
	return slog.New(h), func() []TeaEvent { return got }
}

// FormatEvent calls formatEvent (tui.go).
func FormatEvent(at time.Time, level slog.Level, text string) string {
	return formatEvent(eventMsg{at: at, level: level, text: text})
}

// ClockPlaceholder replaces, in UI views, the clock of the "cancelling" event
// that a ctrl+c adds with time.Now().
const ClockPlaceholder = "{CLOCK}"

// UI drives a uiModel (tui.go) through Update, as tui_test.go TestUIModel
// does.
type UI struct {
	l       *latest
	model   tea.Model
	start   time.Time
	cancels int
	clocks  []string
}

// NewUI calls newUIModel with a cancel function that counts its calls.
func NewUI(title string) *UI {
	u := &UI{l: &latest{}}
	m := newUIModel(title, u.l, func() { u.cancels++ })
	u.start = m.start
	u.model = m
	return u
}

// Set stores a report in the model's latest.
func (u *UI) Set(r client.ProgressReport) { u.l.set(r) }

// update applies msg and reports whether the returned command quits. Only
// non-tick commands are run: a tick's command sleeps and yields the next tick.
func (u *UI) update(msg tea.Msg) bool {
	var cmd tea.Cmd
	u.model, cmd = u.model.Update(msg)
	if _, tick := msg.(tickMsg); tick || cmd == nil {
		return false
	}
	_, quit := cmd().(tea.QuitMsg)
	return quit
}

// Resize sends tea.WindowSizeMsg{Width: w, Height: 40}.
func (u *UI) Resize(w int) bool { return u.update(tea.WindowSizeMsg{Width: w, Height: 40}) }

// Tick sends tickMsg(start + offset).
func (u *UI) Tick(offset time.Duration) bool { return u.update(tickMsg(u.start.Add(offset))) }

// Event sends an eventMsg.
func (u *UI) Event(at time.Time, level slog.Level, text string) bool {
	return u.update(eventMsg{at: at, level: level, text: text})
}

// CtrlC sends the ctrl+c key press and remembers the clock of a new
// "cancelling" event.
func (u *UI) CtrlC() (bool, error) {
	before := u.events()
	quit := u.update(tea.KeyPressMsg{Code: 'c', Mod: tea.ModCtrl})
	after := u.events()
	if len(after) != len(before) {
		last := after[len(after)-1]
		if last.text != "cancelling" {
			return quit, fmt.Errorf("ctrl+c added the event %q", last.text)
		}
		u.clocks = append(u.clocks, last.at.Format("15:04:05"))
	}
	return quit, nil
}

// Done sends doneMsg{err}.
func (u *UI) Done(err error) bool { return u.update(doneMsg{err: err}) }

// Cancels is the number of calls of the model's cancel function.
func (u *UI) Cancels() int { return u.cancels }

func (u *UI) events() []eventMsg {
	if m, ok := u.model.(uiModel); ok {
		return m.events
	}
	return nil
}

// View is the model's View().Content, with the clock of every ctrl+c
// "cancelling" event replaced by ClockPlaceholder.
func (u *UI) View() string {
	s := u.model.View().Content
	for _, c := range u.clocks {
		s = strings.ReplaceAll(s, c+"  cancelling", ClockPlaceholder+"  cancelling")
	}
	return s
}

// ProgressBar renders the bar newUIModel builds (progress.New with the
// default blend) at the given width and percentage.
func ProgressBar(width int, percent float64) string {
	bar := progress.New(progress.WithDefaultBlend())
	bar.SetWidth(width)
	return bar.ViewAs(percent)
}

// Blend1D is lipgloss.Blend1D over two opaque stops, each result as the
// 8-bit channels x/ansi writes in a "38;2;R;G;B" sequence (RGBA() shifted
// down to 8 bits).
func Blend1D(steps int, a, b [3]uint8) [][3]uint8 {
	colors := lipgloss.Blend1D(steps, color.RGBA{R: a[0], G: a[1], B: a[2], A: 0xff}, color.RGBA{R: b[0], G: b[1], B: b[2], A: 0xff})
	out := make([][3]uint8, len(colors))
	for i, c := range colors {
		r, g, bl, _ := c.RGBA()
		out[i] = [3]uint8{uint8(shift8(r)), uint8(shift8(g)), uint8(shift8(bl))}
	}
	return out
}

// shift8 is x/ansi's shift for 16-bit colour channels.
func shift8(x uint32) uint32 {
	if x > 0xff {
		x >>= 8
	}
	return x
}
