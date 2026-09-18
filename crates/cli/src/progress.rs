//! `cmd/dstore/tui.go`: the latest report, the rate meter, the plain status line, the TUI model, its
//! inline renderer, the TUI log handler and colours.
//!
//! What is byte-identical to Go (PORTING.md §1.1, DD-6):
//! - `status_line`, `fraction` and the plain progress lines;
//! - `UiModel::view`, the content of Go's `View()` before Bubble Tea draws it: the lipgloss v2.0.6 styles
//!   (`\x1b[1m…\x1b[m`), the bubbles v2.2.1 progress bar and its go-colorful Lab blend;
//! - the event texts `TeaHandler` sends.
//!
//! The renderer's terminal bytes are not a contract: `run_transfer` draws frames with a small crossterm
//! inline renderer (PORTING.md C9). Each frame is downsampled to the colour profile Bubble Tea would
//! pick, with colorprofile's rules and conversions (`colorprofile`), so the colours and attributes on the
//! terminal are Go's (DD-6).
//!
//! Spec: port-notes/cli.md §2.4, §2.6, §3.6, §4.4, §5.4 and its Addenda.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::future::Future;
use std::io::{IsTerminal, Write};
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use dstore_client::{NodeProgress, Progress, ProgressReport, human_bytes};
use dstore_gocli::{CliError, Context};
use dstore_gocompat::ctx::Ctx;
use dstore_gocompat::errno::{io_error_text, rewrite_os_errors};
use dstore_gocompat::quote::quote;
use dstore_gocompat::slog::{Attr, Handler, Level, Logger, Record, Value, log_level};
use dstore_gocompat::strconv::format_float_g;
use dstore_gocompat::time::{
    GoTime, MILLISECOND, SECOND, SystemZone, Zone, duration_round, duration_string, duration_to_ns,
    format_clock, format_rfc3339nano_utc,
};
use dstore_view::{NodeId, short_id};
use futures::StreamExt;
use tokio::sync::mpsc;
use tokio::task::{JoinError, JoinHandle};

pub mod colorprofile;

use colorprofile::Profile;

/// `maxEvents`.
const MAX_EVENTS: usize = 12;
/// `tickEvery`.
const TICK_EVERY: Duration = Duration::from_millis(100);
/// runPlain's ticker.
const PLAIN_EVERY: Duration = Duration::from_secs(5);
/// The window of every rate meter (`newRateMeter(5 * time.Second)`).
const RATE_WINDOW: Duration = Duration::from_secs(5);
/// `newUIModel`: `width = 80`, `bar.SetWidth(60)`.
const DEFAULT_WIDTH: i64 = 80;
const DEFAULT_BAR_WIDTH: i64 = 60;
/// Go's zero `time.Time`, for a record without a time (never produced by `Logger`).
const ZERO_TIME: GoTime = GoTime {
    unix_secs: -62_135_596_800,
    nanos: 0,
};

/// x/ansi `ResetStyle`.
const RESET: &str = "\x1b[m";
/// `titleStyle` (Bold).
const TITLE_STYLE: &str = "\x1b[1m";
/// `faintStyle` (Faint).
const FAINT_STYLE: &str = "\x1b[2m";
/// `warnStyle` (`lipgloss.Color("3")`).
const WARN_STYLE: &str = "\x1b[33m";
/// `errStyle` (`lipgloss.Color("1")`).
const ERR_STYLE: &str = "\x1b[31m";
/// `okStyle` (`lipgloss.Color("2")`).
const OK_STYLE: &str = "\x1b[32m";

/// bubbles progress `defaultBlendStart` (#5A56E0) and `defaultBlendEnd` (#EE6FF8).
const BLEND_START: [u8; 3] = [0x5a, 0x56, 0xe0];
const BLEND_END: [u8; 3] = [0xee, 0x6f, 0xf8];
/// bubbles progress `defaultEmptyColor` (#606060), as x/ansi writes it.
const EMPTY_SGR: &str = "\x1b[38;2;96;96;96m";
/// `DefaultFullCharHalfBlock` and `DefaultEmptyCharBlock`.
const FULL_CHAR: char = '▌';
const EMPTY_CHAR: char = '░';

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The latest progress report.
pub struct Latest {
    inner: Mutex<ProgressReport>,
}

impl Latest {
    pub fn new() -> Arc<Latest> {
        Arc::new(Latest {
            inner: Mutex::new(ProgressReport::default()),
        })
    }

    pub fn get(&self) -> ProgressReport {
        lock(&self.inner).clone()
    }

    /// `latest.set`: the client calls it for every record, so nothing else happens here.
    fn set(&self, r: &ProgressReport) {
        lock(&self.inner).clone_from(r);
    }

    /// The callback given to the client.
    pub fn progress(self: &Arc<Self>) -> Progress {
        let latest = Arc::clone(self);
        Arc::new(move |r: &ProgressReport| latest.set(r))
    }
}

/// `t.Sub(u)`: signed nanoseconds, saturating as Go's `Time.Sub` does.
fn sub_ns(t: Instant, u: Instant) -> i64 {
    match t.checked_duration_since(u) {
        Some(d) => duration_to_ns(d),
        None => {
            let ns = duration_to_ns(u.duration_since(t));
            if ns == i64::MAX { i64::MIN } else { -ns }
        }
    }
}

/// `Duration.Seconds`.
fn seconds(ns: i64) -> f64 {
    let sec = ns / SECOND;
    let nsec = ns % SECOND;
    sec as f64 + nsec as f64 / 1e9
}

/// `rateMeter`.
pub struct RateMeter {
    window: Duration,
    samples: Vec<(Instant, i64)>,
}

impl RateMeter {
    pub fn new(window: Duration) -> RateMeter {
        RateMeter {
            window,
            samples: Vec::new(),
        }
    }

    /// `add`: records n bytes moved by t and returns the rate over the window in bytes per second.
    pub fn add(&mut self, t: Instant, n: i64) -> f64 {
        self.samples.push((t, n));
        let window = duration_to_ns(self.window);
        while self.samples.len() > 2 && sub_ns(t, self.samples[1].0) >= window {
            self.samples.remove(0);
        }
        let Some(&(first_t, first_n)) = self.samples.first() else {
            return 0.0;
        };
        let d = sub_ns(t, first_t);
        if d <= 0 || n < first_n {
            return 0.0;
        }
        n.wrapping_sub(first_n) as f64 / seconds(d)
    }
}

/// `statusLine`.
pub fn status_line(r: &ProgressReport, rate: f64) -> String {
    let mut s = format!("{}/{} objects", r.objects, r.total_objects);
    if r.total_bytes > 0 {
        let _ = write!(
            s,
            "  {} / {}",
            human_bytes(r.bytes),
            human_bytes(r.total_bytes)
        );
    } else if r.bytes > 0 {
        s.push_str("  ");
        s.push_str(&human_bytes(r.bytes));
    }
    // int64(rate): truncation toward zero, saturating as Go on arm64 does.
    let _ = write!(s, "  {}/s", human_bytes(rate as i64));
    let left = r.total_bytes.wrapping_sub(r.bytes);
    if rate > 0.0 && left > 0 {
        let eta = (left as f64 / rate * SECOND as f64) as i64;
        s.push_str("  eta ");
        s.push_str(&duration_string(duration_round(eta, SECOND)));
    }
    s
}

/// Go's builtin `max` over floats: NaN wins, +0 beats -0.
fn builtin_max(x: f64, y: f64) -> f64 {
    if x.is_nan() || y.is_nan() {
        return f64::NAN;
    }
    if x == y {
        return if x.is_sign_negative() { y } else { x };
    }
    if x > y { x } else { y }
}

/// Go's builtin `min` over floats: NaN wins, -0 beats +0.
fn builtin_min(x: f64, y: f64) -> f64 {
    if x.is_nan() || y.is_nan() {
        return f64::NAN;
    }
    if x == y {
        return if x.is_sign_negative() { x } else { y };
    }
    if x < y { x } else { y }
}

/// `math.Max`.
fn math_max(x: f64, y: f64) -> f64 {
    if x == f64::INFINITY || y == f64::INFINITY {
        return f64::INFINITY;
    }
    builtin_max(x, y)
}

/// `math.Min`.
fn math_min(x: f64, y: f64) -> f64 {
    if x == f64::NEG_INFINITY || y == f64::NEG_INFINITY {
        return f64::NEG_INFINITY;
    }
    builtin_min(x, y)
}

/// `fraction`: by bytes once the total is known, by objects before that; clamped to [0, 1].
pub fn fraction(r: &ProgressReport) -> f64 {
    let f = if r.total_bytes > 0 {
        r.bytes as f64 / r.total_bytes as f64
    } else if r.total_objects > 0 {
        r.objects as f64 / r.total_objects as f64
    } else {
        0.0
    };
    builtin_min(builtin_max(f, 0.0), 1.0)
}

/// `nodeState`: what the client is doing with a node right now.
pub fn node_state(n: &NodeProgress) -> &'static str {
    if n.in_flight == 0 {
        "idle"
    } else if n.awaiting == n.in_flight {
        "waiting for ack"
    } else {
        "sending"
    }
}

/// `formatEvent`: the clock of `at` in `zone`, two spaces and the text; ERROR and above in `errStyle`,
/// WARN and above in `warnStyle`.
pub fn format_event(at: GoTime, level: Level, text: &str, zone: &dyn Zone) -> String {
    let line = format!("{}  {text}", format_clock(at, zone));
    if level >= Level::ERROR {
        render_style(ERR_STYLE, &line)
    } else if level >= Level::WARN {
        render_style(WARN_STYLE, &line)
    } else {
        line
    }
}

/// Messages to the TUI model.
#[derive(Clone, Debug, PartialEq)]
pub enum UiMsg {
    Tick(Instant),
    Resize(u16),
    CtrlC,
    Event {
        at: GoTime,
        level: Level,
        text: String,
    },
    Done(Option<String>),
}

/// `uiModel`.
pub struct UiModel {
    title: String,
    start: Instant,
    now: Instant,
    width: i64,
    bar_width: i64,
    latest: Arc<Latest>,
    meter: RateMeter,
    node_meters: HashMap<NodeId, RateMeter>,
    rep: ProgressReport,
    rate: f64,
    node_rates: HashMap<NodeId, f64>,
    events: Vec<(GoTime, Level, String)>,
    cancel: Ctx,
    cancelling: bool,
    done: bool,
    err: Option<String>,
    zone: Arc<dyn Zone>,
}

impl UiModel {
    /// `newUIModel`: start = now = the current instant, width 80, bar width 60. Ctrl+C cancels `cancel`;
    /// event clocks are formatted in `zone`.
    pub fn new(title: String, latest: Arc<Latest>, cancel: Ctx, zone: Arc<dyn Zone>) -> UiModel {
        let now = Instant::now();
        UiModel {
            title,
            start: now,
            now,
            width: DEFAULT_WIDTH,
            bar_width: DEFAULT_BAR_WIDTH,
            latest,
            meter: RateMeter::new(RATE_WINDOW),
            node_meters: HashMap::new(),
            rep: ProgressReport::default(),
            rate: 0.0,
            node_rates: HashMap::new(),
            events: Vec::new(),
            cancel,
            cancelling: false,
            done: false,
            err: None,
            zone,
        }
    }

    /// `Update`; true = quit (Go returns `tea.Quit`, only for `Done`).
    pub fn update(&mut self, msg: UiMsg) -> bool {
        match msg {
            UiMsg::Resize(w) => {
                self.width = i64::from(w);
                self.bar_width = (self.width - 4).clamp(20, 80);
            }
            UiMsg::CtrlC => {
                if !self.cancelling {
                    self.cancelling = true;
                    self.cancel.cancel();
                    self.append_event(GoTime::now(), Level::WARN, "cancelling".to_owned());
                }
            }
            UiMsg::Tick(t) => self.observe(t),
            UiMsg::Event { at, level, text } => self.append_event(at, level, text),
            UiMsg::Done(err) => {
                self.done = true;
                self.err = err;
                self.observe(Instant::now());
                return true;
            }
        }
        false
    }

    /// `observe`: takes the latest report and updates the rates.
    fn observe(&mut self, now: Instant) {
        self.now = now;
        self.rep = self.latest.get();
        self.rate = self.meter.add(now, self.rep.bytes);
        for n in &self.rep.nodes {
            let meter = self
                .node_meters
                .entry(n.id)
                .or_insert_with(|| RateMeter::new(RATE_WINDOW));
            self.node_rates.insert(n.id, meter.add(now, n.bytes));
        }
    }

    /// `appendEvent`: keeps the last `maxEvents`.
    fn append_event(&mut self, at: GoTime, level: Level, text: String) {
        self.events.push((at, level, text));
        if self.events.len() > MAX_EVENTS {
            let extra = self.events.len() - MAX_EVENTS;
            self.events.drain(..extra);
        }
    }

    /// Byte-identical to the content of Go `View()`.
    pub fn view(&self) -> String {
        let mut b = String::new();
        let elapsed = duration_round(sub_ns(self.now, self.start), SECOND);
        let _ = writeln!(
            b,
            "{}  {}",
            render_style(TITLE_STYLE, &self.title),
            render_style(
                FAINT_STYLE,
                &format!("elapsed {}", duration_string(elapsed))
            )
        );
        b.push_str(&progress_bar(self.bar_width, fraction(&self.rep)));
        b.push('\n');
        b.push_str(&status_line(&self.rep, self.rate));
        b.push('\n');
        if !self.rep.nodes.is_empty() {
            let header = format!(
                "{:<10} {:<7} {:>7} {:>7} {:<15} {:>11} {:>12}",
                "node", "path", "rtt", "batches", "state", "sent", "rate"
            );
            b.push_str(&render_style(FAINT_STYLE, &header));
            b.push('\n');
            for n in &self.rep.nodes {
                let path = if n.direct { "direct" } else { "relay" };
                let rtt_ns = duration_to_ns(n.rtt);
                let rtt = if rtt_ns > 0 {
                    duration_string(duration_round(rtt_ns, MILLISECOND))
                } else {
                    "-".to_owned()
                };
                let rate = self.node_rates.get(&n.id).copied().unwrap_or(0.0);
                let _ = writeln!(
                    b,
                    "{:<10} {:<7} {:>7} {:>7} {:<15} {:>11} {:>10}/s",
                    short_id(&n.id),
                    path,
                    rtt,
                    n.in_flight,
                    node_state(n),
                    human_bytes(n.bytes),
                    human_bytes(rate as i64)
                );
            }
        }
        if !self.events.is_empty() {
            b.push_str(&render_style(FAINT_STYLE, "events"));
            b.push('\n');
            for (at, level, text) in &self.events {
                b.push_str(&format_event(*at, *level, text, self.zone.as_ref()));
                b.push('\n');
            }
        }
        if self.done {
            match &self.err {
                Some(err) => b.push_str(&render_style(ERR_STYLE, &format!("failed: {err}"))),
                None => b.push_str(&render_style(OK_STYLE, "done")),
            }
            b.push('\n');
        }
        b
    }
}

/// `teaHandler`: "msg key=value…", bytes humanised for Int64.
pub struct TeaHandler {
    level: Level,
    send: tokio::sync::mpsc::UnboundedSender<UiMsg>,
}

impl TeaHandler {
    /// `&teaHandler{level: level, send: p.Send}`.
    pub fn new(level: Level, send: tokio::sync::mpsc::UnboundedSender<UiMsg>) -> TeaHandler {
        TeaHandler { level, send }
    }
}

impl Handler for TeaHandler {
    fn enabled(&self, level: Level) -> bool {
        level >= self.level
    }

    fn handle(&self, handler_attrs: &[Attr], r: &Record) {
        let mut text = r.message.clone();
        for a in handler_attrs.iter().chain(r.attrs.iter()) {
            text.push(' ');
            text.push_str(&a.key);
            text.push('=');
            text.push_str(&attr_value(a));
        }
        // p.Send on a finished program is a no-op.
        let _ = self.send.send(UiMsg::Event {
            at: r.time.unwrap_or(ZERO_TIME),
            level: r.level,
            text,
        });
    }
}

/// `attrValue`: `bytes` Int64 attributes humanised, otherwise `Value.String()`, quoted when it holds a
/// space or a tab.
fn attr_value(a: &Attr) -> String {
    if a.key == "bytes"
        && let Value::Int64(n) = a.value
    {
        return human_bytes(n);
    }
    let s = value_string(&a.value);
    if s.contains([' ', '\t']) {
        quote(s.as_bytes())
    } else {
        s
    }
}

/// `slog.Value.String()`.
fn value_string(v: &Value) -> String {
    match v {
        Value::String(s) | Value::Any(s) => s.clone(),
        Value::Int64(n) => n.to_string(),
        Value::Uint64(n) => n.to_string(),
        Value::Float64(f) => format_float_g(*f),
        Value::Bool(b) => b.to_string(),
        Value::Duration(ns) => duration_string(*ns),
        // Go writes `Time.String()`; dstore logs no time attributes (impl-cli-progress.md).
        Value::Time(t) => format_rfc3339nano_utc(*t),
        // `%v` of a []byte.
        Value::Bytes(b) => {
            let items: Vec<String> = b.iter().map(u8::to_string).collect();
            format!("[{}]", items.join(" "))
        }
    }
}

// ---- lipgloss v2.0.6 styles ----

/// `Style.Render` for the styles of tui.go (one SGR attribute, no width, padding, border or margin):
/// tabs become four spaces, CRLF becomes LF, every line is styled on its own, and a multi-line result
/// pads its lines with spaces to the widest line (`alignTextHorizontal`, left).
fn render_style(open: &str, s: &str) -> String {
    let s = s.replace('\t', "    ").replace("\r\n", "\n");
    let mut styled = String::with_capacity(s.len() + open.len() + RESET.len());
    for (i, line) in s.split('\n').enumerate() {
        if i > 0 {
            styled.push('\n');
        }
        styled.push_str(open);
        styled.push_str(line);
        styled.push_str(RESET);
    }
    if !styled.contains('\n') {
        return styled;
    }
    let lines: Vec<&str> = styled.split('\n').collect();
    let widths: Vec<usize> = lines.iter().map(|l| string_width(l)).collect();
    let widest = widths.iter().copied().max().unwrap_or(0);
    let mut b = String::with_capacity(styled.len());
    for (i, (line, w)) in lines.iter().zip(&widths).enumerate() {
        if i > 0 {
            b.push('\n');
        }
        b.push_str(line);
        b.extend(std::iter::repeat_n(' ', widest - w));
    }
    b
}

/// A piece of styled text: an escape sequence (no width) or a character.
enum Segment<'a> {
    Escape(&'a str),
    Char(char),
}

fn segments(s: &str) -> impl Iterator<Item = Segment<'_>> {
    let mut rest = s;
    std::iter::from_fn(move || {
        let c = rest.chars().next()?;
        if c == '\x1b' {
            let len = escape_len(rest);
            let (esc, tail) = rest.split_at(len);
            rest = tail;
            Some(Segment::Escape(esc))
        } else {
            rest = &rest[c.len_utf8()..];
            Some(Segment::Char(c))
        }
    })
}

/// The byte length of the escape sequence at the start of `s` (`s` starts with ESC): CSI up to its final
/// byte, OSC/DCS/SOS/PM/APC up to BEL or ST, otherwise ESC and one character.
fn escape_len(s: &str) -> usize {
    let b = s.as_bytes();
    match b.get(1) {
        None => 1,
        Some(b'[') => {
            let mut i = 2;
            while let Some(&x) = b.get(i) {
                if x >= 0x80 {
                    break;
                }
                i += 1;
                if (0x40..=0x7e).contains(&x) {
                    break;
                }
            }
            i
        }
        Some(b']' | b'P' | b'X' | b'^' | b'_') => {
            let mut i = 2;
            while let Some(&x) = b.get(i) {
                if x == 0x07 {
                    return i + 1;
                }
                if x == 0x1b && b.get(i + 1) == Some(&b'\\') {
                    return i + 2;
                }
                i += 1;
            }
            i
        }
        Some(_) => 1 + s[1..].chars().next().map_or(0, char::len_utf8),
    }
}

/// Cells of one character: controls 0, combining and zero-width characters 0, East Asian wide and
/// emoji ranges 2, else 1. An approximation of x/ansi's grapheme widths (impl-cli-progress.md).
fn char_width(c: char) -> usize {
    let u = u32::from(c);
    if u < 0x20 || (0x7f..0xa0).contains(&u) {
        return 0;
    }
    const ZERO: &[(u32, u32)] = &[
        (0x0300, 0x036f),
        (0x0483, 0x0489),
        (0x0591, 0x05bd),
        (0x0610, 0x061a),
        (0x064b, 0x065f),
        (0x1ab0, 0x1aff),
        (0x1dc0, 0x1dff),
        (0x200b, 0x200f),
        (0x2028, 0x202e),
        (0x2060, 0x2064),
        (0x20d0, 0x20ff),
        (0xfe00, 0xfe0f),
        (0xfe20, 0xfe2f),
        (0xfeff, 0xfeff),
        (0xe0100, 0xe01ef),
    ];
    const WIDE: &[(u32, u32)] = &[
        (0x1100, 0x115f),
        (0x2e80, 0x303e),
        (0x3041, 0x33ff),
        (0x3400, 0x4dbf),
        (0x4e00, 0x9fff),
        (0xa000, 0xa4cf),
        (0xac00, 0xd7a3),
        (0xf900, 0xfaff),
        (0xfe30, 0xfe4f),
        (0xff00, 0xff60),
        (0xffe0, 0xffe6),
        (0x1f300, 0x1f64f),
        (0x1f900, 0x1f9ff),
        (0x20000, 0x3fffd),
    ];
    let within = |ranges: &[(u32, u32)]| ranges.iter().any(|&(lo, hi)| (lo..=hi).contains(&u));
    if within(ZERO) {
        0
    } else if within(WIDE) {
        2
    } else {
        1
    }
}

/// `ansi.StringWidth`: cells of `s`, escape sequences excluded.
fn string_width(s: &str) -> usize {
    segments(s)
        .map(|seg| match seg {
            Segment::Escape(_) => 0,
            Segment::Char(c) => char_width(c),
        })
        .sum()
}

// ---- bubbles v2.2.1 progress, lipgloss Blend1D, go-colorful v1.4.1 ----

/// The bar `newUIModel` builds (`progress.New(progress.WithDefaultBlend())`) at `width`, rendered with
/// `ViewAs(percent)`: half-block cells blended from #5A56E0 to #EE6FF8 over the whole bar, the empty run
/// in #606060, then the percentage `" %3.0f%%"`.
pub fn progress_bar(width: i64, percent: f64) -> String {
    let percent_view = format!(" {:3.0}%", math_max(0.0, math_min(1.0, percent)) * 100.0);
    let text_width = i64::try_from(string_width(&percent_view)).unwrap_or(i64::MAX);
    let tw = width.saturating_sub(text_width).max(0);
    let fw = ((tw as f64 * percent).round() as i64).clamp(0, tw);
    let mut b = String::new();
    if fw > 0 {
        let blend = blend1d(
            usize::try_from(tw.saturating_mul(2)).unwrap_or(0),
            BLEND_START,
            BLEND_END,
        );
        for pair in blend.chunks_exact(2).take(usize::try_from(fw).unwrap_or(0)) {
            let (fg, bg) = (pair[0], pair[1]);
            let _ = write!(
                b,
                "\x1b[38;2;{};{};{};48;2;{};{};{}m{FULL_CHAR}{RESET}",
                fg[0], fg[1], fg[2], bg[0], bg[1], bg[2]
            );
        }
    }
    b.push_str(EMPTY_SGR);
    b.extend(std::iter::repeat_n(
        EMPTY_CHAR,
        usize::try_from(tw - fw).unwrap_or(0),
    ));
    b.push_str(RESET);
    b.push_str(&percent_view);
    b
}

/// A go-colorful `Color`: sRGB channels in [0, 1].
#[derive(Clone, Copy)]
struct Colorful {
    r: f64,
    g: f64,
    b: f64,
}

/// The gc compiler fuses `x*y + z` and `x*y - z` into FMA instructions on arm64, the platform of the
/// vectors; `fma` marks those places.
fn fma(x: f64, y: f64, z: f64) -> f64 {
    x.mul_add(y, z)
}

/// `colorful.MakeColor` of an opaque `color.RGBA`.
fn make_color(c: [u8; 3]) -> Colorful {
    let ch = |v: u8| f64::from(u32::from(v) * 0x101) / 65535.0;
    Colorful {
        r: ch(c[0]),
        g: ch(c[1]),
        b: ch(c[2]),
    }
}

fn linearize(v: f64) -> f64 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn delinearize(v: f64) -> f64 {
    if v <= 0.0031308 {
        12.92 * v
    } else {
        // 1.055*math.Pow(v, 1.0/2.4) - 0.055; the constant 1.0/2.4 is the exact 5/12.
        fma(1.055, v.powf(5.0 / 12.0), -0.055)
    }
}

/// `lab_f`; the constant `6.0/29.0*6.0/29.0*6.0/29.0` is the exact 216/24389.
fn lab_f(t: f64) -> f64 {
    if t > 216.0 / 24389.0 {
        t.cbrt()
    } else {
        t / 3.0 * 29.0 / 6.0 * 29.0 / 6.0 + 4.0 / 29.0
    }
}

/// `lab_finv`; the constant `3.0*6.0/29.0*6.0/29.0` is the exact 108/841.
fn lab_finv(t: f64) -> f64 {
    if t > 6.0 / 29.0 {
        t * t * t
    } else {
        108.0 / 841.0 * (t - 4.0 / 29.0)
    }
}

/// `D65`.
const D65: [f64; 3] = [0.95047, 1.00000, 1.08883];

/// `Color.Xyz()`: linear RGB, then XYZ.
#[allow(clippy::excessive_precision)] // go-colorful's literals, verbatim; they parse to the same f64
fn xyz(c: Colorful) -> [f64; 3] {
    let (r, g, b) = (linearize(c.r), linearize(c.g), linearize(c.b));
    let x = fma(
        0.180_480_788_401_834_29,
        b,
        fma(0.357_584_339_383_877_96, g, 0.412_390_799_265_959_48 * r),
    );
    let y = fma(
        0.072_192_315_360_733_715,
        b,
        fma(0.715_168_678_767_755_93, g, 0.212_639_005_871_510_36 * r),
    );
    let z = fma(
        0.950_532_152_249_660_58,
        b,
        fma(0.119_194_779_794_625_99, g, 0.019_330_818_715_591_851 * r),
    );
    [x, y, z]
}

/// `Color.Lab()`: XYZ, then L*a*b* with the D65 white.
fn to_lab(c: Colorful) -> [f64; 3] {
    let [x, y, z] = xyz(c);
    let fy = lab_f(y / D65[1]);
    [
        fma(1.16, fy, -0.16),
        5.0 * (lab_f(x / D65[0]) - fy),
        2.0 * (fy - lab_f(z / D65[2])),
    ]
}

/// `colorful.Lab(l, a, b)`: XYZ with the D65 white, linear RGB, then sRGB.
#[allow(clippy::excessive_precision)] // go-colorful's literals, verbatim; they parse to the same f64
fn from_lab(lab: [f64; 3]) -> Colorful {
    let l2 = (lab[0] + 0.16) / 1.16;
    let x = D65[0] * lab_finv(l2 + lab[1] / 5.0);
    let y = D65[1] * lab_finv(l2);
    let z = D65[2] * lab_finv(fma(-lab[2], 0.5, l2));
    let r = fma(
        -0.498_610_760_293_003_28,
        z,
        fma(-1.537_383_177_570_093_5, y, 3.240_969_941_904_521_4 * x),
    );
    let g = fma(
        0.041_555_057_407_175_613,
        z,
        fma(1.875_967_501_507_720_7, y, -0.969_243_636_280_879_83 * x),
    );
    let b = fma(
        1.056_971_514_242_878_6,
        z,
        fma(-0.203_976_958_888_976_57, y, 0.055_630_079_696_993_609 * x),
    );
    Colorful {
        r: delinearize(r),
        g: delinearize(g),
        b: delinearize(b),
    }
}

/// `clamp01`: `math.Max(0.0, math.Min(v, 1.0))`.
fn clamp01(v: f64) -> f64 {
    math_max(0.0, math_min(v, 1.0))
}

/// The 8-bit channel x/ansi writes for a colorful channel: `uint32(v*65535.0 + 0.5)`, shifted down by 8
/// only when above 0xff.
fn channel8(v: f64) -> u8 {
    let x = fma(v, 65535.0, 0.5) as u32;
    let x = if x > 0xff { x >> 8 } else { x };
    u8::try_from(x).unwrap_or(u8::MAX)
}

/// lipgloss `Blend1D` / go-colorful `BlendLab`.
pub fn blend1d(steps: usize, a: [u8; 3], b: [u8; 3]) -> Vec<[u8; 3]> {
    if steps <= 2 {
        return [a, b][..steps].to_vec();
    }
    let (la, lb) = (to_lab(make_color(a)), to_lab(make_color(b)));
    let divisor = (steps - 1) as f64;
    (0..steps)
        .map(|j| {
            let t = j as f64 / divisor;
            let c = from_lab([
                fma(t, lb[0] - la[0], la[0]),
                fma(t, lb[1] - la[1], la[1]),
                fma(t, lb[2] - la[2], la[2]),
            ]);
            [
                channel8(clamp01(c.r)),
                channel8(clamp01(c.g)),
                channel8(clamp01(c.b)),
            ]
        })
        .collect()
}

// ---- runTransfer ----

/// A transfer function as `run_transfer` takes it.
type TransferFuture<T> = Pin<Box<dyn Future<Output = Result<T, CliError>> + Send>>;

/// `runTransfer`: plain mode unless stderr is a character device and there is no `--no-tui`; runs `f`
/// with the logger and the progress callback.
pub async fn run_transfer<T: Send + 'static>(
    c: &Context,
    ctx: &Ctx,
    title: String,
    f: impl FnOnce(Ctx, Logger, Progress) -> Pin<Box<dyn Future<Output = Result<T, CliError>> + Send>>
    + Send
    + 'static,
) -> Result<T, CliError> {
    if c.bool("no-tui") || !dstore_gocompat::os::is_char_device(2) {
        let log = crate::common::logger(c);
        return run_plain(ctx, log, Box::new(std::io::stderr()), PLAIN_EVERY, f).await;
    }
    let tty_output = std::io::stderr().is_terminal();
    // Bubble Tea's Run: colorprofile.Detect(os.Stderr, os.Environ()), which may read terminfo files and
    // run `tmux info`.
    let profile = join_result(
        tokio::task::spawn_blocking(move || {
            colorprofile::detect(tty_output, &colorprofile::Environ::from_process())
        })
        .await,
    )?;
    let term = TermConfig {
        out: Box::new(std::io::stderr()),
        input: std::io::stdin().is_terminal(),
        tty_output,
        profile,
    };
    let level = log_level(&c.string("log-level"));
    run_tui(ctx, title, level, Arc::new(SystemZone), term, f).await
}

/// Re-raises a task's panic on the caller, as a panicking goroutine takes the process down in Go.
fn join_result<T>(joined: Result<T, JoinError>) -> Result<T, CliError> {
    match joined {
        Ok(v) => Ok(v),
        Err(e) if e.is_panic() => std::panic::resume_unwind(e.into_panic()),
        Err(e) => Err(CliError::Msg(e.to_string())),
    }
}

/// `runPlain`: a status line on `out` every `every` (the first after `every`, nothing at the end) while
/// `f` runs with `log`.
async fn run_plain<T, F>(
    ctx: &Ctx,
    log: Logger,
    mut out: Box<dyn Write + Send>,
    every: Duration,
    f: F,
) -> Result<T, CliError>
where
    T: Send + 'static,
    F: FnOnce(Ctx, Logger, Progress) -> TransferFuture<T> + Send + 'static,
{
    let latest = Latest::new();
    let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();
    let ticker_latest = Arc::clone(&latest);
    let ticker = tokio::spawn(async move {
        let mut meter = RateMeter::new(RATE_WINDOW);
        let mut t = tokio::time::interval_at(tokio::time::Instant::now() + every, every);
        t.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = &mut stop_rx => return,
                _ = t.tick() => {
                    let r = ticker_latest.get();
                    let mut line = status_line(&r, meter.add(Instant::now(), r.bytes));
                    line.push('\n');
                    let _ = out.write_all(line.as_bytes());
                    let _ = out.flush();
                }
            }
        }
    });
    let res = f(ctx.clone(), log, latest.progress()).await;
    let _ = stop_tx.send(());
    join_result(ticker.await)?;
    res
}

/// Where and how the TUI draws: Bubble Tea's `WithOutput(os.Stderr)`, `WithInput(stdin)` and its tty
/// checks.
struct TermConfig {
    out: Box<dyn Write + Send>,
    /// stdin is a terminal: raw mode and key events (Bubble Tea `initInput`).
    input: bool,
    /// stderr is a terminal: size queries and SIGWINCH (Bubble Tea `ttyOutput`).
    tty_output: bool,
    /// The colour profile frames are downsampled to (Bubble Tea `Program.profile`).
    profile: Profile,
}

/// `runTUI`: the model on an inline renderer while `f` runs in a task with a `TeaHandler` logger; the
/// transfer's own result is returned.
async fn run_tui<T, F>(
    ctx: &Ctx,
    title: String,
    level: Level,
    zone: Arc<dyn Zone>,
    term: TermConfig,
    f: F,
) -> Result<T, CliError>
where
    T: Send + 'static,
    F: FnOnce(Ctx, Logger, Progress) -> TransferFuture<T> + Send + 'static,
{
    let ctx = ctx.with_cancel();
    let latest = Latest::new();
    let (tx, mut rx) = mpsc::unbounded_channel::<UiMsg>();
    let mut model = UiModel::new(title, Arc::clone(&latest), ctx.clone(), zone);
    let log = Logger::new(Arc::new(TeaHandler::new(level, tx)));
    let mut handle = tokio::spawn(f(ctx.clone(), log, latest.progress()));
    let mut screen = match Screen::open(term) {
        Ok(s) => s,
        Err(err) => {
            // p.Run() failed: cancel, wait for the transfer, return the run error.
            ctx.cancel();
            let _ = (&mut handle).await;
            return Err(CliError::Msg(err));
        }
    };
    let joined = event_loop(&mut model, &mut rx, &mut handle, &mut screen).await;
    screen.close();
    // runTUI's `defer cancel()`: whatever the transfer left running on ctx ends with it.
    ctx.cancel();
    join_result(joined)?
}

/// The Bubble Tea program loop: ticks every 100 ms, log events, keys, resizes; quits on done.
async fn event_loop<T>(
    model: &mut UiModel,
    rx: &mut mpsc::UnboundedReceiver<UiMsg>,
    handle: &mut JoinHandle<Result<T, CliError>>,
    screen: &mut Screen,
) -> Result<Result<T, CliError>, JoinError> {
    let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + TICK_EVERY, TICK_EVERY);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut keys = if screen.input {
        Some(EventStream::new())
    } else {
        None
    };
    let mut winch = if screen.tty_output {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::window_change()).ok()
    } else {
        None
    };
    // Bubble Tea's Run always sends a WindowSizeMsg at start: the terminal's size, or 0 × 0 when stderr is
    // not a terminal (the bar then clamps to 20).
    if let Some(w) = screen.initial_width() {
        model.update(UiMsg::Resize(w));
    }
    screen.draw(&model.view());
    let mut rx_open = true;
    loop {
        tokio::select! {
            joined = &mut *handle => {
                // Events the transfer logged before returning come first (Go sends doneMsg after them).
                while let Ok(msg) = rx.try_recv() {
                    model.update(msg);
                }
                let text = match &joined {
                    Ok(Ok(_)) => None,
                    Ok(Err(e)) => Some(cli_error_text(e)),
                    Err(e) => Some(e.to_string()),
                };
                model.update(UiMsg::Done(text));
                screen.draw(&model.view());
                return joined;
            }
            msg = rx.recv(), if rx_open => match msg {
                Some(msg) => {
                    model.update(msg);
                    while let Ok(msg) = rx.try_recv() {
                        model.update(msg);
                    }
                }
                None => rx_open = false,
            },
            _ = ticker.tick() => {
                model.update(UiMsg::Tick(Instant::now()));
            }
            ev = next_event(&mut keys), if keys.is_some() => match ev {
                Some(Ok(Event::Key(k))) if is_ctrl_c(&k) => {
                    model.update(UiMsg::CtrlC);
                }
                Some(Ok(_)) => {}
                Some(Err(_)) | None => keys = None,
            },
            sig = next_signal(&mut winch), if winch.is_some() => match sig {
                Some(()) => {
                    if let Some(w) = screen.check_resize() {
                        model.update(UiMsg::Resize(w));
                    }
                }
                None => winch = None,
            },
        }
        screen.draw(&model.view());
    }
}

async fn next_event(keys: &mut Option<EventStream>) -> Option<std::io::Result<Event>> {
    match keys {
        Some(k) => k.next().await,
        None => std::future::pending().await,
    }
}

async fn next_signal(sig: &mut Option<tokio::signal::unix::Signal>) -> Option<()> {
    match sig {
        Some(s) => s.recv().await,
        None => std::future::pending().await,
    }
}

/// `msg.String() == "ctrl+c"`.
fn is_ctrl_c(k: &KeyEvent) -> bool {
    k.code == KeyCode::Char('c')
        && k.modifiers == KeyModifiers::CONTROL
        && k.kind != KeyEventKind::Release
}

/// `err.Error()` of a transfer error, with Rust OS error texts rewritten to Go's (PORTING.md §5.2).
fn cli_error_text(e: &CliError) -> String {
    match e {
        CliError::Msg(m) => rewrite_os_errors(m),
        CliError::Exit { msg, .. } => msg.clone(),
    }
}

/// The terminal while the TUI runs: raw mode (when the input is a terminal) and the renderer. Closing
/// shows the cursor and restores the terminal; dropping closes.
struct Screen {
    renderer: InlineRenderer,
    raw: bool,
    input: bool,
    tty_output: bool,
    closed: bool,
}

impl Screen {
    fn open(term: TermConfig) -> Result<Screen, String> {
        let mut raw = false;
        if term.input {
            crossterm::terminal::enable_raw_mode()
                .map_err(|e| format!("error entering raw mode: {}", io_error_text(&e)))?;
            raw = true;
        }
        let mut renderer = InlineRenderer::new(term.out, term.profile);
        renderer.start();
        Ok(Screen {
            renderer,
            raw,
            input: term.input,
            tty_output: term.tty_output,
            closed: false,
        })
    }

    /// The width of Bubble Tea's initial `WindowSizeMsg` (`Program.Run`): `p.width`, still 0, when stderr is
    /// not a terminal, else the terminal's.
    fn initial_width(&mut self) -> Option<u16> {
        if !self.tty_output {
            return Some(0);
        }
        self.check_resize()
    }

    /// `checkResize`: the terminal width when stderr is a terminal.
    fn check_resize(&mut self) -> Option<u16> {
        if !self.tty_output {
            return None;
        }
        // `window_size` asks /dev/tty (else stdout); unlike `size`, it never falls back to running `tput`.
        let ws = crossterm::terminal::window_size().ok()?;
        self.renderer.resize(ws.columns, ws.rows);
        Some(ws.columns)
    }

    fn draw(&mut self, frame: &str) {
        self.renderer.draw(frame);
    }

    fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.renderer.finish();
        if self.raw {
            let _ = crossterm::terminal::disable_raw_mode();
        }
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        self.close();
    }
}

/// An inline renderer: each frame replaces the previous one below the cursor, and the last frame stays on
/// the terminal.
struct InlineRenderer {
    out: Box<dyn Write + Send>,
    profile: Profile,
    /// Columns and rows, when stderr is a terminal.
    size: Option<(u16, u16)>,
    /// Lines of the frame on the terminal; the cursor is on the line below it.
    drawn: usize,
    last: Option<String>,
}

impl InlineRenderer {
    fn new(out: Box<dyn Write + Send>, profile: Profile) -> InlineRenderer {
        InlineRenderer {
            out,
            profile,
            size: None,
            drawn: 0,
            last: None,
        }
    }

    fn start(&mut self) {
        let _ = crossterm::queue!(self.out, crossterm::cursor::Hide);
        let _ = self.out.flush();
    }

    fn resize(&mut self, w: u16, h: u16) {
        self.size = Some((w, h));
        self.last = None;
    }

    fn draw(&mut self, frame: &str) {
        if self.last.as_deref() == Some(frame) {
            return;
        }
        let (bytes, lines) = frame_bytes(frame, self.drawn, self.size, self.profile);
        let _ = self.out.write_all(&bytes);
        let _ = self.out.flush();
        self.drawn = lines;
        self.last = Some(frame.to_owned());
    }

    fn finish(&mut self) {
        let _ = crossterm::queue!(self.out, crossterm::cursor::Show);
        let _ = self.out.flush();
    }
}

/// The bytes that replace a frame of `drawn` lines with `frame`, and the new frame's line count: back to
/// the first line of the old frame, clear below, then the lines, each ended by CR LF (raw mode does not
/// map LF). Lines are cut to the terminal width, and a frame taller than the terminal keeps its last
/// lines. Each line is then downsampled to `profile`.
fn frame_bytes(
    frame: &str,
    drawn: usize,
    size: Option<(u16, u16)>,
    profile: Profile,
) -> (Vec<u8>, usize) {
    let mut buf = Vec::with_capacity(frame.len() + 32);
    buf.push(b'\r');
    if drawn > 0 {
        let up = u16::try_from(drawn).unwrap_or(u16::MAX);
        let _ = crossterm::queue!(buf, crossterm::cursor::MoveUp(up));
    }
    let _ = crossterm::queue!(
        buf,
        crossterm::terminal::Clear(crossterm::terminal::ClearType::FromCursorDown)
    );
    let body = frame.strip_suffix('\n').unwrap_or(frame);
    let mut lines: Vec<&str> = if frame.is_empty() {
        Vec::new()
    } else {
        body.split('\n').collect()
    };
    // A terminal that reports no size (0 × 0, e.g. a pty nobody sized) neither trims nor cuts.
    if let Some((_, h)) = size
        && h > 0
    {
        let keep = usize::from(h).saturating_sub(1).max(1);
        if lines.len() > keep {
            lines.drain(..lines.len() - keep);
        }
    }
    for line in &lines {
        let line = match size {
            Some((w, _)) if w > 0 => truncate_cells(line, usize::from(w)),
            _ => Cow::Borrowed(*line),
        };
        buf.extend_from_slice(colorprofile::downsample(&line, profile).as_bytes());
        buf.extend_from_slice(b"\r\n");
    }
    (buf, lines.len())
}

/// Cuts `line` to `w` cells, keeping its escape sequences and resetting the style when cut.
fn truncate_cells(line: &str, w: usize) -> Cow<'_, str> {
    if string_width(line) <= w {
        return Cow::Borrowed(line);
    }
    let mut out = String::with_capacity(line.len());
    let mut width = 0;
    let mut full = false;
    for seg in segments(line) {
        match seg {
            Segment::Escape(e) => out.push_str(e),
            Segment::Char(c) => {
                let cw = char_width(c);
                if full || width + cw > w {
                    full = true;
                    continue;
                }
                width += cw;
                out.push(c);
            }
        }
    }
    out.push_str(RESET);
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dstore_gocompat::time::FixedZone;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn rate_meter_go_test() {
        // tui_test.go TestRateMeter.
        let mut m = RateMeter::new(secs(5));
        let t0 = Instant::now();
        assert_eq!(m.add(t0, 0), 0.0, "first sample rate");
        assert_eq!(m.add(t0 + secs(1), 1000), 1000.0, "rate after 1 s");
        // Ten seconds at 100 B/s: the window forgets the fast start.
        for i in 2..=11u64 {
            m.add(t0 + secs(i), 1000 + (i as i64 - 1) * 100);
        }
        let r = m.add(t0 + secs(12), 2100);
        assert!(
            (99.0..=101.0).contains(&r),
            "windowed rate {r}, want about 100"
        );
    }

    #[test]
    fn rate_meter_cli_md_sequence_with_samples() {
        // cli.md §5.4: add(t, n) → rate (samples kept).
        let cases: &[(u64, i64, f64, usize)] = &[
            (0, 0, 0.0, 1),
            (1000, 1000, 1000.0, 2),
            (2000, 1100, 550.0, 3),
            (3000, 1200, 400.0, 4),
            (7000, 1600, 100.0, 3),
            (8000, 1700, 100.0, 3),
            (8000, 1700, 100.0, 4),
            (9000, 100, 0.0, 5),
            (15000, 5000, 816.6666666666666, 2),
        ];
        let mut m = RateMeter::new(secs(5));
        let t0 = Instant::now();
        for &(ms, n, rate, samples) in cases {
            assert_eq!(
                m.add(t0 + Duration::from_millis(ms), n),
                rate,
                "add({ms} ms, {n})"
            );
            assert_eq!(m.samples.len(), samples, "samples after add({ms} ms, {n})");
        }
    }

    #[test]
    fn rate_meter_backwards_time() {
        let mut m = RateMeter::new(secs(5));
        let t0 = Instant::now();
        assert_eq!(m.add(t0 + secs(10), 0), 0.0);
        assert_eq!(m.add(t0 + secs(9), 100), 0.0);
        assert_eq!(m.add(t0 + secs(11), 200), 200.0);
        assert_eq!(m.samples.len(), 3);
    }

    #[test]
    fn rate_meter_vector_sample_counts() {
        // cli/text.json rate_meter "window-2s", "sub-millisecond" and "tui-test-go", with the `samples` the
        // golden test cannot see: (window ns, [(t ns, n, rate, samples)]).
        type Adds = &'static [(u64, i64, f64, usize)];
        let cases: &[(u64, Adds)] = &[
            (
                2_000_000_000,
                &[
                    (0, 0, 0.0, 1),
                    (1_000_000_000, 100, 100.0, 2),
                    (2_000_000_000, 300, 150.0, 3),
                    (3_000_000_000, 600, 250.0, 3),
                    (10_000_000_000, 700, 14.285714285714286, 2),
                ],
            ),
            (5_000_000_000, &[(0, 0, 0.0, 1), (500_000, 10, 20000.0, 2)]),
            (
                5_000_000_000,
                &[
                    (0, 0, 0.0, 1),
                    (1_000_000_000, 1000, 1000.0, 2),
                    (2_000_000_000, 1100, 550.0, 3),
                    (3_000_000_000, 1200, 400.0, 4),
                    (4_000_000_000, 1300, 325.0, 5),
                    (5_000_000_000, 1400, 280.0, 6),
                    (6_000_000_000, 1500, 100.0, 6),
                    (7_000_000_000, 1600, 100.0, 6),
                    (8_000_000_000, 1700, 100.0, 6),
                    (9_000_000_000, 1800, 100.0, 6),
                    (10_000_000_000, 1900, 100.0, 6),
                    (11_000_000_000, 2000, 100.0, 6),
                    (12_000_000_000, 2100, 100.0, 6),
                ],
            ),
        ];
        for &(window, adds) in cases {
            let mut m = RateMeter::new(Duration::from_nanos(window));
            let t0 = Instant::now();
            for &(t, n, rate, samples) in adds {
                assert_eq!(
                    m.add(t0 + Duration::from_nanos(t), n).to_bits(),
                    rate.to_bits(),
                    "window {window}: add({t} ns, {n})"
                );
                assert_eq!(
                    m.samples.len(),
                    samples,
                    "window {window}: samples after add({t} ns, {n})"
                );
            }
        }
    }

    #[test]
    fn fraction_go_test() {
        // tui_test.go TestStatusLine, the fraction part.
        let mut r = ProgressReport {
            objects: 3,
            total_objects: 4,
            ..ProgressReport::default()
        };
        assert_eq!(fraction(&r), 0.75, "fraction by objects");
        (r.bytes, r.total_bytes) = (3000, 1000); // a re-send overshoots the total
        assert_eq!(fraction(&r), 1.0, "fraction clamps to 1");
        (r.bytes, r.total_bytes) = (-5, 10);
        let f = fraction(&r);
        assert!(
            f == 0.0 && f.is_sign_positive(),
            "negative bytes clamp to +0"
        );
        assert_eq!(fraction(&ProgressReport::default()), 0.0);
    }

    #[test]
    fn status_line_go_test() {
        // tui_test.go TestStatusLine, the status-line part, plus the verified lines of cli.md §3.6.
        let r = ProgressReport {
            objects: 3,
            total_objects: 4,
            ..ProgressReport::default()
        };
        assert_eq!(status_line(&r, 0.0), "3/4 objects  0 B/s");
        let report = |objects, total_objects, bytes, total_bytes| ProgressReport {
            objects,
            total_objects,
            bytes,
            total_bytes,
            nodes: Vec::new(),
        };
        let cases = [
            (report(0, 0, 512, 0), 100.0, "0/0 objects  512 B  100 B/s"),
            (
                report(5, 10, 1024, 2048),
                512.0,
                "5/10 objects  1.0 KiB / 2.0 KiB  512 B/s  eta 2s",
            ),
            (
                report(5, 10, 1 << 30, 5 << 30),
                3.7e6,
                "5/10 objects  1.0 GiB / 5.0 GiB  3.5 MiB/s  eta 19m21s",
            ),
            (
                report(5, 10, 3000, 1000),
                10.0,
                "5/10 objects  2.9 KiB / 1000 B  10 B/s",
            ),
            (
                report(1, 2, 10, 100),
                0.5,
                "1/2 objects  10 B / 100 B  0 B/s  eta 3m0s",
            ),
            (
                report(1, 2, 10, 100_000_000),
                1.0,
                "1/2 objects  10 B / 95.4 MiB  1 B/s  eta 27777h46m30s",
            ),
        ];
        for (r, rate, want) in cases {
            assert_eq!(status_line(&r, rate), want);
        }
    }

    #[test]
    fn node_state_cases() {
        let n = |in_flight, awaiting| NodeProgress {
            in_flight,
            awaiting,
            ..NodeProgress::default()
        };
        assert_eq!(node_state(&n(0, 0)), "idle");
        assert_eq!(node_state(&n(0, 1)), "idle");
        assert_eq!(node_state(&n(1, 1)), "waiting for ack");
        assert_eq!(node_state(&n(2, 1)), "sending");
        assert_eq!(node_state(&n(3, 0)), "sending");
    }

    #[test]
    fn format_event_styles() {
        // cli.md §5.4: INFO unstyled, WARN yellow, ERROR red; clock in the zone.
        let at = GoTime::from_unix_nano(1_758_198_896_000_000_000);
        let utc = FixedZone(0);
        assert_eq!(
            format_event(at, Level::INFO, "uploaded x", &utc),
            "12:34:56  uploaded x"
        );
        assert_eq!(
            format_event(at, Level::WARN, "upload failed", &utc),
            "\x1b[33m12:34:56  upload failed\x1b[m"
        );
        assert_eq!(
            format_event(at, Level::ERROR, "boom node=abcd", &utc),
            "\x1b[31m12:34:56  boom node=abcd\x1b[m"
        );
        assert_eq!(
            format_event(at, Level(6), "warn+2", &FixedZone(7200)),
            "\x1b[33m14:34:56  warn+2\x1b[m"
        );
    }

    #[test]
    fn render_style_lines_tabs_and_padding() {
        assert_eq!(render_style(ERR_STYLE, ""), "\x1b[31m\x1b[m");
        assert_eq!(render_style(TITLE_STYLE, "push x"), "\x1b[1mpush x\x1b[m");
        assert_eq!(render_style(WARN_STYLE, "a\tb"), "\x1b[33ma    b\x1b[m");
        // Every line styled on its own and padded to the widest line.
        assert_eq!(
            render_style(ERR_STYLE, "failed: a\r\nbb\nccc"),
            "\x1b[31mfailed: a\x1b[m\n\x1b[31mbb\x1b[m       \n\x1b[31mccc\x1b[m      "
        );
        assert_eq!(
            render_style(OK_STYLE, "a\n"),
            "\x1b[32ma\x1b[m\n\x1b[32m\x1b[m "
        );
    }

    #[test]
    fn string_width_skips_escapes() {
        assert_eq!(string_width(""), 0);
        assert_eq!(string_width("\x1b[38;2;1;2;3;48;2;4;5;6m▌\x1b[m"), 1);
        assert_eq!(string_width("\x1b]8;;https://x\x07link\x1b]8;;\x1b\\"), 4);
        assert_eq!(string_width("é中a\u{301}"), 4);
        assert_eq!(string_width("a\x1bxb"), 2);
        assert_eq!(string_width("\x1b[1é"), 1);
    }

    #[test]
    fn progress_bar_verified_frame() {
        // cli.md §3.6: width 60 at 50 %: 28 half-block cells, 27 empty cells, "  50%".
        let bar = progress_bar(60, 0.5);
        assert!(bar.starts_with(
            "\x1b[38;2;90;86;224;48;2;92;86;225m▌\x1b[m\x1b[38;2;94;86;225;48;2;95;87;225m▌\x1b[m"
        ));
        assert!(bar.ends_with(
            "\x1b[38;2;171;98;236;48;2;172;99;237m▌\x1b[m\x1b[38;2;96;96;96m░░░░░░░░░░░░░░░░░░░░░░░░░░░\x1b[m  50%"
        ));
        assert_eq!(bar.matches('▌').count(), 28);
        assert_eq!(
            progress_bar(20, 0.0),
            "\x1b[38;2;96;96;96m░░░░░░░░░░░░░░░\x1b[m   0%"
        );
        assert_eq!(progress_bar(5, 0.5), "\x1b[38;2;96;96;96m\x1b[m  50%");
        assert!(progress_bar(26, 0.996).ends_with("\x1b[38;2;96;96;96m\x1b[m 100%"));
        assert_eq!(progress_bar(20, 0.125).matches('▌').count(), 2);
        assert!(
            progress_bar(20, 0.125).ends_with("  12%"),
            "ties round to even"
        );
    }

    #[test]
    fn blend1d_default_blend() {
        assert!(blend1d(0, BLEND_START, BLEND_END).is_empty());
        assert_eq!(blend1d(2, BLEND_START, BLEND_END), [BLEND_START, BLEND_END]);
        assert_eq!(
            blend1d(4, BLEND_START, BLEND_END),
            [
                [90, 86, 224],
                [147, 94, 232],
                [194, 103, 240],
                [238, 111, 248]
            ]
        );
        assert_eq!(
            blend1d(7, [255, 0, 0], [0, 0, 255]),
            [
                [255, 0, 0],
                [241, 0, 59],
                [223, 0, 99],
                [202, 0, 137],
                [173, 0, 175],
                [130, 0, 215],
                [0, 0, 255]
            ]
        );
    }

    #[test]
    fn channel8_keeps_small_values_unshifted() {
        // x/ansi shifts a 16-bit channel only when it exceeds 0xff.
        assert_eq!(channel8(0.0), 0);
        assert_eq!(channel8(1.0), 255);
        assert_eq!(channel8(100.0 / 65535.0), 100);
        assert_eq!(channel8(300.0 / 65535.0), 1);
    }

    #[test]
    fn go_min_max_semantics() {
        assert!(builtin_max(-0.0, 0.0).is_sign_positive());
        assert!(builtin_min(0.0, -0.0).is_sign_negative());
        assert!(builtin_max(f64::NAN, 1.0).is_nan());
        assert!(math_max(0.0, math_min(1.0, f64::NAN)).is_nan());
        assert_eq!(math_max(f64::NAN, f64::INFINITY), f64::INFINITY);
        assert_eq!(math_min(f64::NAN, f64::NEG_INFINITY), f64::NEG_INFINITY);
    }

    #[test]
    fn ui_model_resize_clamps_the_bar() {
        let mut m = UiModel::new(
            "t".into(),
            Latest::new(),
            Ctx::background().with_cancel(),
            Arc::new(FixedZone(0)),
        );
        assert_eq!((m.width, m.bar_width), (80, 60));
        for (w, bar) in [(10u16, 20i64), (24, 20), (25, 21), (84, 80), (200, 80)] {
            assert!(!m.update(UiMsg::Resize(w)));
            assert_eq!((m.width, m.bar_width), (i64::from(w), bar));
        }
    }

    #[test]
    fn ui_model_ctrl_c_cancels_once() {
        let ctx = Ctx::background().with_cancel();
        let mut m = UiModel::new(
            "t".into(),
            Latest::new(),
            ctx.clone(),
            Arc::new(FixedZone(0)),
        );
        assert!(!m.update(UiMsg::CtrlC));
        assert!(ctx.err().is_some(), "ctrl+c cancels the transfer");
        assert!(!m.update(UiMsg::CtrlC));
        assert_eq!(m.events.len(), 1);
        assert_eq!(
            (m.events[0].1, m.events[0].2.as_str()),
            (Level::WARN, "cancelling")
        );
    }

    #[test]
    fn ui_model_keeps_twelve_events() {
        let mut m = UiModel::new(
            "t".into(),
            Latest::new(),
            Ctx::background(),
            Arc::new(FixedZone(0)),
        );
        for i in 0..14 {
            m.update(UiMsg::Event {
                at: GoTime::from_unix_nano(0),
                level: Level::INFO,
                text: format!("event {i}"),
            });
        }
        let texts: Vec<&str> = m.events.iter().map(|e| e.2.as_str()).collect();
        assert_eq!(texts.len(), 12);
        assert_eq!((texts[0], texts[11]), ("event 2", "event 13"));
    }

    #[test]
    fn ui_model_go_test() {
        // tui_test.go TestUIModel, against UiModel::view.
        let latest = Latest::new();
        let set = latest.progress();
        let ctx = Ctx::background().with_cancel();
        let start = Instant::now();
        let mut m = UiModel::new(
            "push demo".into(),
            Arc::clone(&latest),
            ctx.clone(),
            Arc::new(FixedZone(0)),
        );
        let id = NodeId({
            let mut b = [0u8; 32];
            (b[0], b[1]) = (0xab, 0xcd);
            b
        });
        let node = |bytes, rtt, awaiting| NodeProgress {
            id,
            direct: true,
            rtt,
            in_flight: 1,
            awaiting,
            bytes,
        };
        set(&ProgressReport {
            objects: 5,
            total_objects: 10,
            bytes: 512,
            total_bytes: 1024,
            nodes: vec![node(512, Duration::from_millis(3), 0)],
        });
        m.update(UiMsg::Resize(100));
        m.update(UiMsg::Tick(start));
        set(&ProgressReport {
            objects: 5,
            total_objects: 10,
            bytes: 1024,
            total_bytes: 2048,
            nodes: vec![node(1024, Duration::from_millis(3), 0)],
        });
        m.update(UiMsg::Tick(start + secs(1)));
        m.update(UiMsg::Event {
            at: GoTime::now(),
            level: Level::WARN,
            text: "upload retry node=abcd reason=busy".into(),
        });
        let out = m.view();
        for want in [
            "push demo",
            "5/10 objects",
            "1.0 KiB / 2.0 KiB",
            "512 B/s",
            "eta 2s",
            "50%",
            "direct",
            "3ms",
            "sending",
            "upload retry node=abcd reason=busy",
        ] {
            assert!(out.contains(want), "view lacks {want:?}:\n{out}");
        }
        set(&ProgressReport {
            objects: 5,
            total_objects: 10,
            bytes: 2048,
            total_bytes: 2048,
            nodes: vec![node(2048, Duration::ZERO, 1)],
        });
        m.update(UiMsg::Tick(start + secs(2)));
        let out = m.view();
        assert!(
            out.contains("waiting for ack"),
            "view lacks the ack state:\n{out}"
        );
        m.update(UiMsg::CtrlC);
        assert!(ctx.err().is_some(), "ctrl+c did not cancel the transfer");
        assert!(
            m.update(UiMsg::Done(Some("context canceled".into()))),
            "done did not quit"
        );
        let out = m.view();
        assert!(
            out.contains("failed: context canceled") && out.contains("cancelling"),
            "final view:\n{out}"
        );
    }

    #[test]
    fn tea_handler_go_test() {
        // tui_test.go TestTeaHandler.
        let (tx, mut rx) = mpsc::unbounded_channel();
        let log = Logger::new(Arc::new(TeaHandler::new(Level::INFO, tx)))
            .with(vec![Attr::string("node", "abcd")]);
        log.debug("hidden", vec![]);
        log.info(
            "uploaded",
            vec![
                Attr::int64("objects", 3),
                Attr::int64("bytes", 3 << 20),
                Attr::duration("took", 2 * SECOND),
                Attr::string("path", "direct"),
            ],
        );
        log.warn(
            "upload failed",
            vec![Attr::string("err", "timeout: no recent network activity")],
        );
        let mut got = Vec::new();
        while let Ok(UiMsg::Event { level, text, .. }) = rx.try_recv() {
            got.push((level, text));
        }
        assert_eq!(
            got,
            [
                (
                    Level::INFO,
                    "uploaded node=abcd objects=3 bytes=3.0 MiB took=2s path=direct".to_owned()
                ),
                (
                    Level::WARN,
                    r#"upload failed node=abcd err="timeout: no recent network activity""#
                        .to_owned()
                ),
            ]
        );
    }

    #[test]
    fn tea_handler_values_without_bytes() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let h = TeaHandler::new(Level::WARN, tx);
        assert!(!h.enabled(Level::INFO) && h.enabled(Level::WARN) && h.enabled(Level::ERROR));
        let at = GoTime::from_unix_nano(5);
        h.handle(
            &[Attr::string("node", "abcd")],
            &Record {
                time: Some(at),
                level: Level::ERROR,
                message: "q".into(),
                attrs: vec![
                    Attr::string("s", "a\"b"),
                    Attr::string("t", "x\ty"),
                    Attr::string("e", ""),
                    Attr::string("n", "a=b"),
                    Attr::uint64("bytes", 5),
                    Attr::any("err", "disk full"),
                    Attr {
                        key: "raw".into(),
                        value: Value::Bytes(b"hi".to_vec()),
                    },
                    Attr {
                        key: "f".into(),
                        value: Value::Float64(3.7e6),
                    },
                    Attr::duration("d", 1_500_000),
                ],
            },
        );
        assert_eq!(
            rx.try_recv(),
            Ok(UiMsg::Event {
                at,
                level: Level::ERROR,
                text: r#"q node=abcd s=a"b t="x\ty" e= n=a=b bytes=5 err="disk full" raw="[104 105]" f=3.7e+06 d=1.5ms"#
                    .to_owned(),
            })
        );
    }

    #[test]
    fn frame_bytes_replace_the_previous_frame() {
        let tc = Profile::TrueColor;
        let (b, n) = frame_bytes("a\nbb\n", 0, None, tc);
        assert_eq!((b.as_slice(), n), (&b"\r\x1b[Ja\r\nbb\r\n"[..], 2));
        let (b, n) = frame_bytes("x\n", 2, None, tc);
        assert_eq!((b.as_slice(), n), (&b"\r\x1b[2A\x1b[Jx\r\n"[..], 1));
        // Cut to the width, last lines kept when taller than the terminal.
        let (b, n) = frame_bytes("1\n2\n\x1b[1mabcdef\x1b[m\n", 1, Some((4, 3)), tc);
        assert_eq!(
            (b.as_slice(), n),
            (&b"\r\x1b[1A\x1b[J2\r\n\x1b[1mabcd\x1b[m\x1b[m\r\n"[..], 2)
        );
        // A terminal that reports 0 × 0: no trimming, no cutting.
        let (b, n) = frame_bytes("1\n2\n3\n", 0, Some((0, 0)), tc);
        assert_eq!((b.as_slice(), n), (&b"\r\x1b[J1\r\n2\r\n3\r\n"[..], 3));
        let (b, n) = frame_bytes("", 3, None, tc);
        assert_eq!((b.as_slice(), n), (&b"\r\x1b[3A\x1b[J"[..], 0));
    }

    #[test]
    fn frame_bytes_downsample_each_line_after_the_cut() {
        // NoTTY: no SGR at all, the cut's reset included; the renderer's own sequences stay.
        let (b, _) = frame_bytes("\x1b[1mabcdef\x1b[m\n", 1, Some((4, 3)), Profile::NoTty);
        assert_eq!(b.as_slice(), &b"\r\x1b[1A\x1b[Jabcd\r\n"[..]);
        let (b, _) = frame_bytes(
            "\x1b[38;2;96;96;96m\u{2591}\x1b[m \x1b[31mfailed\x1b[m\n",
            0,
            None,
            Profile::Ansi256,
        );
        assert_eq!(
            b.as_slice(),
            "\r\x1b[J\x1b[38;5;59m\u{2591}\x1b[m \x1b[31mfailed\x1b[m\r\n".as_bytes()
        );
        let (b, _) = frame_bytes("\x1b[2mx\x1b[m \x1b[33my\x1b[m\n", 0, None, Profile::Ascii);
        assert_eq!(
            b.as_slice(),
            &b"\r\x1b[J\x1b[2mx\x1b[m \x1b[my\x1b[m\r\n"[..]
        );
    }

    #[test]
    fn truncate_cells_wide_characters() {
        assert_eq!(truncate_cells("abc", 3), "abc");
        assert_eq!(truncate_cells("a中b", 2), "a\x1b[m");
        assert_eq!(
            truncate_cells("\x1b[31m中中\x1b[m", 3),
            "\x1b[31m中\x1b[m\x1b[m"
        );
    }

    /// A writer shared with the test.
    #[derive(Clone, Default)]
    struct SharedBuf(Arc<Mutex<Vec<u8>>>);

    impl Write for SharedBuf {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            lock(&self.0).extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl SharedBuf {
        fn text(&self) -> String {
            String::from_utf8_lossy(&lock(&self.0)).into_owned()
        }
    }

    #[tokio::test]
    async fn run_plain_prints_status_lines_while_the_transfer_runs() {
        let buf = SharedBuf::default();
        let seen = buf.clone();
        let res = run_plain(
            &Ctx::background(),
            Logger::default_logger(),
            Box::new(buf.clone()),
            Duration::from_millis(20),
            move |_ctx, _log, prog| {
                Box::pin(async move {
                    prog(&ProgressReport {
                        objects: 3,
                        total_objects: 4,
                        ..ProgressReport::default()
                    });
                    // Run until the ticker has written a line, however slow the machine (bounded at 10 s).
                    for _ in 0..2000 {
                        if !seen.text().is_empty() {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                    Ok::<_, CliError>(7)
                })
            },
        )
        .await;
        assert!(matches!(res, Ok(7)));
        let text = buf.text();
        assert!(!text.is_empty(), "no status line");
        for line in text.lines() {
            assert_eq!(line, "3/4 objects  0 B/s");
        }
        let after = buf.text();
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(
            buf.text(),
            after,
            "status lines after the transfer returned"
        );
    }

    #[tokio::test]
    async fn run_tui_draws_events_and_the_done_frame() {
        let buf = SharedBuf::default();
        let term = TermConfig {
            out: Box::new(buf.clone()),
            input: false,
            tty_output: false,
            profile: Profile::TrueColor,
        };
        let res = run_tui(
            &Ctx::background(),
            "push trees/x".into(),
            Level::INFO,
            Arc::new(FixedZone(0)),
            term,
            |ctx, log, prog| {
                Box::pin(async move {
                    prog(&ProgressReport {
                        objects: 1,
                        total_objects: 2,
                        ..ProgressReport::default()
                    });
                    log.debug("hidden", vec![]);
                    log.warn("upload retry", vec![Attr::string("reason", "busy")]);
                    tokio::time::sleep(Duration::from_millis(150)).await;
                    assert!(ctx.err().is_none());
                    Err::<(), _>(CliError::Msg(
                        "open x: No such file or directory (os error 2)".into(),
                    ))
                })
            },
        )
        .await;
        assert!(
            matches!(res, Err(CliError::Msg(ref m)) if m == "open x: No such file or directory (os error 2)")
        );
        let text = buf.text();
        assert!(
            text.starts_with("\x1b[?25l"),
            "cursor hidden first: {text:?}"
        );
        assert!(text.ends_with("\x1b[?25h"), "cursor shown last: {text:?}");
        let last = text.rsplit("\x1b[J").next().unwrap_or_default();
        // stderr is not a terminal: Bubble Tea's initial WindowSizeMsg is 0 × 0, so the bar is 20 wide
        // (tw 15, fw round(7.5) = 8).
        assert_eq!(last.matches('▌').count(), 8, "{last:?}");
        assert!(last.contains("\x1b[m  50%\r\n"), "{last:?}");
        assert!(last.contains("1/2 objects  0 B/s\r\n"), "{last:?}");
        assert!(
            last.contains("  upload retry reason=busy\x1b[m\r\n"),
            "{last:?}"
        );
        assert!(!last.contains("hidden"));
        assert!(
            last.contains("\x1b[31mfailed: open x: no such file or directory\x1b[m\r\n"),
            "{last:?}"
        );
    }

    /// Bubble Tea converts the frame to the detected profile: plain text for NoTTY (TERM=dumb), bold and
    /// faint without colours for Ascii (NO_COLOR), indexed colours for ANSI256; never 24-bit colours.
    #[tokio::test]
    async fn run_tui_downsamples_frames_to_the_profile() {
        for (profile, attrs, colours) in [
            (Profile::NoTty, false, false),
            (Profile::Ascii, true, false),
            (Profile::Ansi256, true, true),
        ] {
            let buf = SharedBuf::default();
            let term = TermConfig {
                out: Box::new(buf.clone()),
                input: false,
                tty_output: false,
                profile,
            };
            let res = run_tui(
                &Ctx::background(),
                "pull trees/x".into(),
                Level::INFO,
                Arc::new(FixedZone(0)),
                term,
                |_ctx, _log, prog| {
                    Box::pin(async move {
                        prog(&ProgressReport {
                            objects: 1,
                            total_objects: 2,
                            ..ProgressReport::default()
                        });
                        Ok::<_, CliError>(())
                    })
                },
            )
            .await;
            assert!(res.is_ok(), "{profile}");
            let text = buf.text();
            assert!(!text.contains("\x1b[38;2;"), "{profile}: {text:?}");
            let last = text.rsplit("\x1b[J").next().unwrap_or_default();
            assert!(last.contains("pull trees/x"), "{profile}: {last:?}");
            assert!(last.contains("done"), "{profile}: {last:?}");
            assert_eq!(
                last.contains("\x1b[1mpull trees/x\x1b[m"),
                attrs,
                "{profile}: {last:?}"
            );
            assert_eq!(
                last.contains("\x1b[38;5;59m\u{2591}"),
                colours,
                "{profile}: {last:?}"
            );
            assert_eq!(
                last.contains("\x1b[32mdone\x1b[m"),
                colours,
                "{profile}: {last:?}"
            );
            if profile == Profile::NoTty {
                assert!(
                    !last.contains("\x1b[1m") && !last.contains("\x1b[m"),
                    "{last:?}"
                );
            }
        }
    }
}
