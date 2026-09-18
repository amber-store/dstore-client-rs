//! `log/slog` as dstore uses it (go1.26.5 `log/slog`): levels, attributes with Go's kinds, `Logger`, the
//! exact `TextHandler` line format, and the line `slog.Default()` writes through the `log` package.
//!
//! What is modelled:
//! - Values arrive resolved. `Value::Any` holds the `%+v` text of an error or other value, which is
//!   what `appendTextValue` quotes. dstore uses no groups, `LogValuer`s, `ReplaceAttr` or `AddSource`,
//!   so none are modelled.
//! - Nothing is elided. Go drops only the zero `Attr{}` (an empty key and a nil `any`), and a rendered
//!   `Value` cannot represent that.
//! - Keys, messages and string values are Rust `String`s, so invalid UTF-8 reaches a handler only as
//!   `Value::Bytes` (PORTING.md DD-8).
//! - Specs: port-notes/client-core.md §2.13, §3.5 and cli.md §2.3.

use std::io::Write;
use std::sync::{Arc, Mutex};

use crate::quote::{is_print, is_space, quote};
use crate::strconv::format_float_g;
use crate::time::{GoTime, SystemZone, Zone, duration_string, format_log_std, format_slog_time};

/// `slog.Level`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Level(pub i32);

impl Level {
    pub const DEBUG: Level = Level(-4);
    pub const INFO: Level = Level(0);
    pub const WARN: Level = Level(4);
    pub const ERROR: Level = Level(8);
}

/// `Level.String`: "INFO", "WARN+2", "DEBUG-1".
impl std::fmt::Display for Level {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Relative to the next lower named level; below DEBUG relative to DEBUG. No overflow: the
        // subtraction only moves towards zero.
        let (base, val) = if self.0 < Level::INFO.0 {
            ("DEBUG", self.0 - Level::DEBUG.0)
        } else if self.0 < Level::WARN.0 {
            ("INFO", self.0 - Level::INFO.0)
        } else if self.0 < Level::ERROR.0 {
            ("WARN", self.0 - Level::WARN.0)
        } else {
            ("ERROR", self.0 - Level::ERROR.0)
        };
        match val {
            0 => f.write_str(base),
            v if v > 0 => write!(f, "{base}+{v}"),
            v => write!(f, "{base}{v}"),
        }
    }
}

/// cmd/dstore `logLevel`: lower-cased; debug|warn|error, else INFO.
pub fn log_level(s: &str) -> Level {
    // Go: `switch strings.ToLower(s)`. `strings.ToLower(s)` equals "debug", "warn" or "error" exactly when
    // `s` does ignoring ASCII case. The only non-ASCII runes whose `unicode.ToLower` is ASCII are U+0130
    // ('i') and U+212A ('k'), and neither letter occurs in those words.
    if s.eq_ignore_ascii_case("debug") {
        Level::DEBUG
    } else if s.eq_ignore_ascii_case("warn") {
        Level::WARN
    } else if s.eq_ignore_ascii_case("error") {
        Level::ERROR
    } else {
        Level::INFO
    }
}

/// `slog.Value` kinds dstore uses.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    String(String),
    Int64(i64),
    Uint64(u64),
    Float64(f64),
    Bool(bool),
    /// Nanoseconds.
    Duration(i64),
    Time(GoTime),
    Bytes(Vec<u8>),
    /// `slog.Any` of an error or a `%+v` value, already rendered.
    Any(String),
}

/// `slog.Attr`.
#[derive(Clone, Debug, PartialEq)]
pub struct Attr {
    pub key: String,
    pub value: Value,
}

impl Attr {
    /// `slog.String`.
    pub fn string(k: &str, v: impl Into<String>) -> Attr {
        Attr {
            key: k.to_owned(),
            value: Value::String(v.into()),
        }
    }

    /// `slog.Int64` (also `slog.Int`).
    pub fn int64(k: &str, v: i64) -> Attr {
        Attr {
            key: k.to_owned(),
            value: Value::Int64(v),
        }
    }

    /// `slog.Uint64`.
    pub fn uint64(k: &str, v: u64) -> Attr {
        Attr {
            key: k.to_owned(),
            value: Value::Uint64(v),
        }
    }

    /// `slog.Bool`.
    pub fn bool(k: &str, v: bool) -> Attr {
        Attr {
            key: k.to_owned(),
            value: Value::Bool(v),
        }
    }

    /// `slog.Duration`.
    pub fn duration(k: &str, ns: i64) -> Attr {
        Attr {
            key: k.to_owned(),
            value: Value::Duration(ns),
        }
    }

    /// `slog.Any` of errors and `%+v` values.
    pub fn any(k: &str, v: impl std::fmt::Display) -> Attr {
        Attr {
            key: k.to_owned(),
            value: Value::Any(v.to_string()),
        }
    }
}

/// `slog.Record`. `time: None` is Go's zero time, which handlers omit.
#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    pub time: Option<GoTime>,
    pub level: Level,
    pub message: String,
    pub attrs: Vec<Attr>,
}

/// `slog.Handler`.
pub trait Handler: Send + Sync + 'static {
    fn enabled(&self, level: Level) -> bool;
    /// `handler_attrs` are the `Logger::with` attributes, which come first.
    fn handle(&self, handler_attrs: &[Attr], r: &Record);
}

/// `*slog.Logger`.
#[derive(Clone)]
pub struct Logger {
    handler: Arc<dyn Handler>,
    attrs: Arc<Vec<Attr>>,
}

impl Logger {
    /// `slog.New`.
    pub fn new(h: Arc<dyn Handler>) -> Logger {
        Logger {
            handler: h,
            attrs: Arc::new(Vec::new()),
        }
    }

    /// `Logger.With`: the handler attributes followed by `attrs`; no attributes returns the logger.
    pub fn with(&self, attrs: Vec<Attr>) -> Logger {
        if attrs.is_empty() {
            return self.clone();
        }
        let mut all = Vec::with_capacity(self.attrs.len() + attrs.len());
        all.extend(self.attrs.iter().cloned());
        all.extend(attrs);
        Logger {
            handler: Arc::clone(&self.handler),
            attrs: Arc::new(all),
        }
    }

    /// `Logger.Enabled`.
    pub fn enabled(&self, level: Level) -> bool {
        self.handler.enabled(level)
    }

    /// `Logger.Log`: checks `enabled` first; time = `GoTime::now()`.
    pub fn log(&self, level: Level, msg: &str, attrs: Vec<Attr>) {
        if !self.enabled(level) {
            return;
        }
        let r = Record {
            time: Some(GoTime::now()),
            level,
            message: msg.to_owned(),
            attrs,
        };
        self.handler.handle(&self.attrs, &r);
    }

    pub fn debug(&self, msg: &str, attrs: Vec<Attr>) {
        self.log(Level::DEBUG, msg, attrs);
    }

    pub fn info(&self, msg: &str, attrs: Vec<Attr>) {
        self.log(Level::INFO, msg, attrs);
    }

    pub fn warn(&self, msg: &str, attrs: Vec<Attr>) {
        self.log(Level::WARN, msg, attrs);
    }

    pub fn error(&self, msg: &str, attrs: Vec<Attr>) {
        self.log(Level::ERROR, msg, attrs);
    }

    /// `slog.Default()`: log-package format on stderr, level INFO.
    pub fn default_logger() -> Logger {
        Logger::new(Arc::new(DefaultHandler))
    }
}

impl std::fmt::Debug for Logger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Logger")
            .field("attrs", &self.attrs)
            .finish_non_exhaustive()
    }
}

/// `slog.Default()`'s handler (handler.go `defaultHandler`): enabled from INFO (`logLoggerLevel`'s
/// zero value), each line written by the standard `log.Logger` (stderr, `LstdFlags`, local time read
/// when the line is written).
struct DefaultHandler;

impl Handler for DefaultHandler {
    fn enabled(&self, level: Level) -> bool {
        level >= Level::INFO
    }

    fn handle(&self, handler_attrs: &[Attr], r: &Record) {
        let line = format_default_record(handler_attrs, r, GoTime::now(), &SystemZone);
        let mut err = std::io::stderr().lock();
        // Logger discards handler errors.
        let _ = err.write_all(&line);
        let _ = err.flush();
    }
}

/// `slog.TextHandler`.
pub struct TextHandler {
    level: Level,
    zone: Arc<dyn Zone>,
    out: Mutex<Box<dyn Write + Send>>,
}

impl TextHandler {
    /// `slog.NewTextHandler(out, &slog.HandlerOptions{Level: level})`, formatting times in `zone`.
    pub fn new(out: Box<dyn Write + Send>, level: Level, zone: Arc<dyn Zone>) -> TextHandler {
        TextHandler {
            level,
            zone,
            out: Mutex::new(out),
        }
    }
}

/// One `write_all` per record.
impl Handler for TextHandler {
    fn enabled(&self, level: Level) -> bool {
        level >= self.level
    }

    fn handle(&self, handler_attrs: &[Attr], r: &Record) {
        // Format first, then write the whole line under the lock, as `commonHandler.handle` does.
        let line = format_text_record(handler_attrs, r, &*self.zone);
        let mut out = match self.out.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        // Logger discards handler errors. The flush hands the line on at once, as Go's unbuffered
        // `Write` does, even when `out` buffers.
        let _ = out.write_all(&line);
        let _ = out.flush();
    }
}

/// slog `text_handler.go` `needsQuoting`.
pub fn needs_quoting(s: &str) -> bool {
    if s.is_empty() {
        return true;
    }
    for c in s.chars() {
        if c.is_ascii() {
            let b = c as u8;
            // Quote anything except a backslash that would need quoting in a JSON string, as well as
            // space and '='.
            if b != b'\\' && (b == b' ' || b == b'=' || !safe_set(b)) {
                return true;
            }
        } else if c == char::REPLACEMENT_CHARACTER || is_space(c) || !is_print(c) {
            // Go decodes invalid UTF-8 as RuneError; a Rust `str` holds only the valid U+FFFD.
            return true;
        }
    }
    false
}

/// slog `safeSet` (json_handler.go): the ASCII bytes a JSON string holds without escaping: everything
/// from ' ' to DEL except '"' and '\\'.
fn safe_set(b: u8) -> bool {
    (0x20..=0x7f).contains(&b) && b != b'"' && b != b'\\'
}

/// The exact `TextHandler` line of a record, including the trailing "\n".
pub fn format_text_record(handler_attrs: &[Attr], r: &Record, zone: &dyn Zone) -> Vec<u8> {
    let mut buf = Vec::with_capacity(256);
    // Built-in attributes: time (omitted when zero), level, msg.
    if let Some(t) = r.time {
        buf.extend_from_slice(b"time=");
        buf.extend_from_slice(format_slog_time(t, zone).as_bytes());
        buf.push(b' ');
    }
    buf.extend_from_slice(b"level=");
    append_string(&mut buf, &r.level.to_string());
    buf.extend_from_slice(b" msg=");
    append_string(&mut buf, &r.message);
    append_attrs(&mut buf, handler_attrs, &r.attrs, zone);
    buf.push(b'\n');
    buf
}

/// The exact line `slog.Default()` writes for a record: the `log.LstdFlags` header at `now` in `zone`,
/// then `defaultHandler.Handle`'s text (level, a space, the raw unquoted message, the attributes as in
/// [`format_text_record`]), then "\n" unless the text already ends with one (`log.Logger.output`).
/// `r.time` is not used: the log package reads the clock itself.
pub fn format_default_record(
    handler_attrs: &[Attr],
    r: &Record,
    now: GoTime,
    zone: &dyn Zone,
) -> Vec<u8> {
    let mut buf = Vec::with_capacity(256);
    buf.extend_from_slice(format_log_std(now, zone).as_bytes());
    buf.push(b' ');
    buf.extend_from_slice(r.level.to_string().as_bytes());
    buf.push(b' ');
    buf.extend_from_slice(r.message.as_bytes());
    append_attrs(&mut buf, handler_attrs, &r.attrs, zone);
    if buf.last() != Some(&b'\n') {
        buf.push(b'\n');
    }
    buf
}

/// `appendNonBuiltIns`: the handler's attributes, then the record's, each as " key=value".
fn append_attrs(buf: &mut Vec<u8>, handler_attrs: &[Attr], attrs: &[Attr], zone: &dyn Zone) {
    for a in handler_attrs.iter().chain(attrs) {
        buf.push(b' ');
        append_string(buf, &a.key);
        buf.push(b'=');
        append_value(buf, &a.value, zone);
    }
}

/// `handleState.appendString` (text): `strconv.Quote` when `needsQuoting`, else verbatim.
fn append_string(buf: &mut Vec<u8>, s: &str) {
    if needs_quoting(s) {
        buf.extend_from_slice(quote(s.as_bytes()).as_bytes());
    } else {
        buf.extend_from_slice(s.as_bytes());
    }
}

/// `appendTextValue`.
fn append_value(buf: &mut Vec<u8>, v: &Value, zone: &dyn Zone) {
    match v {
        Value::String(s) | Value::Any(s) => append_string(buf, s),
        // `Value.append`: written verbatim, never quoted.
        Value::Int64(n) => buf.extend_from_slice(n.to_string().as_bytes()),
        Value::Uint64(n) => buf.extend_from_slice(n.to_string().as_bytes()),
        Value::Float64(f) => buf.extend_from_slice(format_float_g(*f).as_bytes()),
        Value::Bool(b) => buf.extend_from_slice(if *b { "true" } else { "false" }.as_bytes()),
        Value::Duration(ns) => buf.extend_from_slice(duration_string(*ns).as_bytes()),
        Value::Time(t) => buf.extend_from_slice(format_slog_time(*t, zone).as_bytes()),
        // `[]byte` values are always quoted.
        Value::Bytes(b) => buf.extend_from_slice(quote(b).as_bytes()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io;

    use crate::time::FixedZone;

    /// 2026-09-18T10:11:12.345678901Z.
    const T2026: GoTime = GoTime {
        unix_secs: 1_789_726_272,
        nanos: 345_678_901,
    };
    /// slog's `testTime`, 2000-01-02T03:04:05Z.
    const T2000: GoTime = GoTime {
        unix_secs: 946_782_245,
        nanos: 0,
    };

    fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        match m.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn text(b: &[u8]) -> String {
        String::from_utf8_lossy(b).into_owned()
    }

    /// A writer recording every `write` call.
    #[derive(Clone, Default)]
    struct Writes(Arc<Mutex<Vec<Vec<u8>>>>);

    impl Writes {
        fn calls(&self) -> Vec<Vec<u8>> {
            lock(&self.0).clone()
        }
    }

    impl Write for Writes {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            lock(&self.0).push(b.to_vec());
            Ok(b.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// A handler keeping what it is given.
    struct Keep {
        min: Level,
        got: Mutex<Vec<(Vec<Attr>, Record)>>,
    }

    impl Handler for Keep {
        fn enabled(&self, level: Level) -> bool {
            level >= self.min
        }

        fn handle(&self, handler_attrs: &[Attr], r: &Record) {
            lock(&self.got).push((handler_attrs.to_vec(), r.clone()));
        }
    }

    fn record(time: Option<GoTime>, level: Level, msg: &str, attrs: Vec<Attr>) -> Record {
        Record {
            time,
            level,
            message: msg.to_owned(),
            attrs,
        }
    }

    #[test]
    fn level_string() {
        // Go level_test.go TestLevelString, plus the ends of the range.
        let cases = [
            (0, "INFO"),
            (8, "ERROR"),
            (10, "ERROR+2"),
            (6, "WARN+2"),
            (4, "WARN"),
            (3, "INFO+3"),
            (1, "INFO+1"),
            (-3, "DEBUG+1"),
            (-4, "DEBUG"),
            (-6, "DEBUG-2"),
            (-5, "DEBUG-1"),
            (12, "ERROR+4"),
            (i32::MIN, "DEBUG-2147483644"),
            (i32::MAX, "ERROR+2147483639"),
        ];
        for (n, want) in cases {
            assert_eq!(Level(n).to_string(), want, "Level({n})");
        }
        assert!(Level::DEBUG < Level::INFO && Level::WARN < Level::ERROR);
    }

    #[test]
    fn log_level_of_flag_values() {
        let cases = [
            ("debug", Level::DEBUG),
            ("DEBUG", Level::DEBUG),
            ("dEbUg", Level::DEBUG),
            ("warn", Level::WARN),
            ("WARN", Level::WARN),
            ("error", Level::ERROR),
            ("Error", Level::ERROR),
            ("info", Level::INFO),
            ("", Level::INFO),
            ("verbose", Level::INFO),
            ("WARNING", Level::INFO),
            ("debug ", Level::INFO),
            ("err", Level::INFO),
            ("\u{212a}", Level::INFO),
            ("\u{130}nfo", Level::INFO),
        ];
        for (s, want) in cases {
            assert_eq!(log_level(s), want, "{s:?}");
        }
    }

    #[test]
    fn needs_quoting_ascii() {
        // Go text_handler_test.go TestNeedsQuoting (ASCII cases).
        let cases = [
            ("", true),
            ("ab", false),
            ("a=b", true),
            ("\"ab\"", true),
            ("\x07\x08", true),
            ("a\tb", true),
            ("a b", true),
            ("a\\b", false),
            ("a\x7fb", false),
            ("trees/**", false),
            ("<>&!#$%'()*+,-./:;?@[]^_`{|}~", false),
        ];
        for (s, want) in cases {
            assert_eq!(needs_quoting(s), want, "{s:?}");
        }
        for b in 0u8..0x80 {
            let s = format!("a{}b", b as char);
            let want = b <= 0x20 || b == b'=' || b == b'"';
            assert_eq!(needs_quoting(&s), want, "byte {b:#04x}");
        }
    }

    #[test]
    fn needs_quoting_unicode() {
        let cases = [
            ("µåπ", false),
            ("é", false),
            ("日本語", false),
            ("a\u{a0}b", true),
            ("\u{3000}", true),
            ("\u{fffd}", true),
            ("a\u{200b}b", true),
            ("\u{e000}", true),
        ];
        for (s, want) in cases {
            assert_eq!(needs_quoting(s), want, "{s:?}");
        }
    }

    #[test]
    fn attr_constructors_keep_go_kinds() {
        assert_eq!(
            Attr::string("node", "abcd"),
            Attr {
                key: "node".into(),
                value: Value::String("abcd".into())
            }
        );
        assert_eq!(Attr::int64("nodes", -3).value, Value::Int64(-3));
        assert_eq!(Attr::uint64("u", u64::MAX).value, Value::Uint64(u64::MAX));
        assert_eq!(Attr::bool("b", true).value, Value::Bool(true));
        assert_eq!(Attr::duration("rtt", -1).value, Value::Duration(-1));
        assert_eq!(
            Attr::any("err", CtxLike).value,
            Value::Any("context canceled".into())
        );
    }

    struct CtxLike;

    impl std::fmt::Display for CtxLike {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("context canceled")
        }
    }

    #[test]
    fn text_record_without_time() {
        let r = record(
            None,
            Level(6),
            "connected",
            vec![
                Attr::int64("nodes", 3),
                Attr::uint64("u64", 7),
                Attr::bool("ok", false),
                Attr::string("!BADKEY", "lonely"),
                Attr::string("bs", "a\\b"),
            ],
        );
        let line = format_text_record(&[Attr::string("node", "1a2b3c4d")], &r, &FixedZone(0));
        assert_eq!(
            text(&line),
            "level=WARN+2 msg=connected node=1a2b3c4d nodes=3 u64=7 ok=false !BADKEY=lonely bs=a\\b\n"
        );
    }

    #[test]
    fn text_handler_writes_once_per_record() {
        let w = Writes::default();
        let h = TextHandler::new(Box::new(w.clone()), Level::WARN, Arc::new(FixedZone(0)));
        assert!(!h.enabled(Level::INFO));
        assert!(h.enabled(Level::WARN));
        assert!(h.enabled(Level::ERROR));
        let attrs: Vec<Attr> = (0..12).map(|i| Attr::int64(&format!("a{i}"), i)).collect();
        h.handle(&[], &record(None, Level::ERROR, "m", attrs));
        h.handle(
            &[Attr::string("node", "abcd")],
            &record(None, Level::WARN, "n", vec![]),
        );
        let calls = w.calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(
            text(&calls[0]),
            "level=ERROR msg=m a0=0 a1=1 a2=2 a3=3 a4=4 a5=5 a6=6 a7=7 a8=8 a9=9 a10=10 a11=11\n"
        );
        assert_eq!(text(&calls[1]), "level=WARN msg=n node=abcd\n");
    }

    #[test]
    fn text_handler_survives_a_poisoned_lock() {
        struct PanicOnce(bool, Writes);
        impl Write for PanicOnce {
            fn write(&mut self, b: &[u8]) -> io::Result<usize> {
                if !self.0 {
                    self.0 = true;
                    panic!("writer failure");
                }
                self.1.write(b)
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let w = Writes::default();
        let h = TextHandler::new(
            Box::new(PanicOnce(false, w.clone())),
            Level::INFO,
            Arc::new(FixedZone(0)),
        );
        let r = record(None, Level::INFO, "m", vec![]);
        let first = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| h.handle(&[], &r)));
        assert!(first.is_err());
        h.handle(&[], &r);
        assert_eq!(w.calls(), vec![b"level=INFO msg=m\n".to_vec()]);
    }

    #[test]
    fn go_text_handler_cases() {
        // Go text_handler_test.go TestTextHandler (the cases a rendered Value can express).
        let prefix = "time=2000-01-02T03:04:05.000Z level=INFO msg=\"a message\"";
        let cases = [
            (Attr::int64("a", 1), "a=1"),
            (Attr::string("x = y", "qu\"o"), "\"x = y\"=\"qu\\\"o\""),
            (Attr::any("name", "Hoek, Ren"), "name=\"Hoek, Ren\""),
            (Attr::any("x", "&{A:1 b:2}"), "x=\"&{A:1 b:2}\""),
            (Attr::any("a", "<nil>"), "a=<nil>"),
        ];
        for (attr, want) in cases {
            let r = record(Some(T2000), Level::INFO, "a message", vec![attr]);
            let line = format_text_record(&[], &r, &FixedZone(0));
            assert_eq!(text(&line), format!("{prefix} {want}\n"));
        }
    }

    #[test]
    fn go_text_handler_preformatted() {
        // Go text_handler_test.go TestTextHandlerPreformatted.
        let with = [
            Attr::duration("dur", 60 * crate::time::SECOND),
            Attr::bool("b", true),
        ];
        let r = record(None, Level::INFO, "m", vec![Attr::int64("a", 1)]);
        let line = format_text_record(&with, &r, &FixedZone(0));
        assert_eq!(text(&line), "level=INFO msg=m dur=1m0s b=true a=1\n");
    }

    #[test]
    fn text_record_values_and_zones() {
        let r = record(
            Some(T2026),
            Level::DEBUG,
            "quoting",
            vec![
                Attr::string("empty", ""),
                Attr::string("tab", "a\tb"),
                Attr {
                    key: "f".into(),
                    value: Value::Float64(0.5),
                },
                Attr {
                    key: "nan".into(),
                    value: Value::Float64(f64::NAN),
                },
                Attr::duration("took", 1_235 * crate::time::MILLISECOND),
                Attr {
                    key: "bytes".into(),
                    value: Value::Bytes(b"a\xffb".to_vec()),
                },
                Attr {
                    key: "at".into(),
                    value: Value::Time(T2000),
                },
            ],
        );
        let line = format_text_record(&[], &r, &FixedZone(7200));
        assert_eq!(
            text(&line),
            "time=2026-09-18T12:11:12.345+02:00 level=DEBUG msg=quoting empty=\"\" tab=\"a\\tb\" f=0.5 nan=NaN took=1.235s bytes=\"a\\xffb\" at=2000-01-02T05:04:05.000+02:00\n"
        );
    }

    #[test]
    fn logger_with_and_levels() {
        let keep = Arc::new(Keep {
            min: Level::INFO,
            got: Mutex::new(Vec::new()),
        });
        let base = Logger::new(keep.clone());
        let log = base
            .with(vec![Attr::string("node", "abcd")])
            .with(vec![])
            .with(vec![Attr::int64("round", 1)]);
        assert!(!log.enabled(Level::DEBUG));
        assert!(log.enabled(Level::INFO));
        log.debug("dropped", vec![]);
        log.info("i", vec![Attr::int64("objects", 3)]);
        log.warn("w", vec![]);
        log.error("e", vec![]);
        log.log(Level(6), "custom", vec![]);
        base.info("plain", vec![]);
        let got = lock(&keep.got).clone();
        let summary: Vec<(Vec<String>, Level, String, usize)> = got
            .iter()
            .map(|(h, r)| {
                (
                    h.iter().map(|a| a.key.clone()).collect(),
                    r.level,
                    r.message.clone(),
                    r.attrs.len(),
                )
            })
            .collect();
        let node_round = vec!["node".to_owned(), "round".to_owned()];
        assert_eq!(
            summary,
            vec![
                (node_round.clone(), Level::INFO, "i".to_owned(), 1),
                (node_round.clone(), Level::WARN, "w".to_owned(), 0),
                (node_round.clone(), Level::ERROR, "e".to_owned(), 0),
                (node_round, Level(6), "custom".to_owned(), 0),
                (vec![], Level::INFO, "plain".to_owned(), 0),
            ]
        );
        assert!(got.iter().all(|(_, r)| r.time.is_some()));
    }

    #[test]
    fn default_logger_is_enabled_from_info() {
        let log = Logger::default_logger();
        assert!(!log.enabled(Level::DEBUG));
        assert!(!log.enabled(Level(-1)));
        assert!(log.enabled(Level::INFO));
        assert!(log.enabled(Level::ERROR));
        // Debug records are dropped before any formatting or clock reading.
        log.debug("never printed", vec![]);
    }

    #[test]
    fn default_record_layout() {
        let zone = FixedZone(7200);
        let r = record(
            Some(T2000),
            Level::INFO,
            "connected",
            vec![Attr::int64("nodes", 3), Attr::string("path", "none")],
        );
        let line = format_default_record(&[Attr::string("node", "1a2b3c4d")], &r, T2026, &zone);
        assert_eq!(
            text(&line),
            "2026/09/18 12:11:12 INFO connected node=1a2b3c4d nodes=3 path=none\n"
        );
        // The message is not quoted; a trailing newline is not doubled.
        let r = record(None, Level::WARN, "a b\n", vec![]);
        assert_eq!(
            text(&format_default_record(&[], &r, T2026, &zone)),
            "2026/09/18 12:11:12 WARN a b\n"
        );
        let r = record(None, Level::INFO, "", vec![]);
        assert_eq!(
            text(&format_default_record(&[], &r, T2026, &zone)),
            "2026/09/18 12:11:12 INFO \n"
        );
    }
}
