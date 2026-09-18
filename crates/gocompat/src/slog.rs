//! `log/slog` as dstore uses it: levels, attributes with Go's kinds, `Logger`, and the exact
//! `TextHandler` line format.

use std::io::Write;
use std::sync::{Arc, Mutex};

use crate::time::{GoTime, Zone};

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
        todo!()
    }
}

/// cmd/dstore `logLevel`: lower-cased; debug|warn|error, else INFO.
pub fn log_level(s: &str) -> Level {
    todo!()
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
        todo!()
    }

    /// `slog.Int64` (also `slog.Int`).
    pub fn int64(k: &str, v: i64) -> Attr {
        todo!()
    }

    /// `slog.Uint64`.
    pub fn uint64(k: &str, v: u64) -> Attr {
        todo!()
    }

    /// `slog.Bool`.
    pub fn bool(k: &str, v: bool) -> Attr {
        todo!()
    }

    /// `slog.Duration`.
    pub fn duration(k: &str, ns: i64) -> Attr {
        todo!()
    }

    /// `slog.Any` of errors and `%+v` values.
    pub fn any(k: &str, v: impl std::fmt::Display) -> Attr {
        todo!()
    }
}

/// `slog.Record`.
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
        todo!()
    }

    /// `Logger.With`.
    pub fn with(&self, attrs: Vec<Attr>) -> Logger {
        todo!()
    }

    /// `Logger.Enabled`.
    pub fn enabled(&self, level: Level) -> bool {
        todo!()
    }

    /// `Logger.Log`: checks `enabled` first; time = `GoTime::now()`.
    pub fn log(&self, level: Level, msg: &str, attrs: Vec<Attr>) {
        todo!()
    }

    pub fn debug(&self, msg: &str, attrs: Vec<Attr>) {
        todo!()
    }

    pub fn info(&self, msg: &str, attrs: Vec<Attr>) {
        todo!()
    }

    pub fn warn(&self, msg: &str, attrs: Vec<Attr>) {
        todo!()
    }

    pub fn error(&self, msg: &str, attrs: Vec<Attr>) {
        todo!()
    }

    /// `slog.Default()`: log-package format on stderr, level INFO.
    pub fn default_logger() -> Logger {
        todo!()
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
        todo!()
    }
}

/// One `write_all` per record.
impl Handler for TextHandler {
    fn enabled(&self, level: Level) -> bool {
        todo!()
    }

    fn handle(&self, handler_attrs: &[Attr], r: &Record) {
        todo!()
    }
}

/// slog `text_handler.go` `needsQuoting`.
pub fn needs_quoting(s: &str) -> bool {
    todo!()
}

/// The exact `TextHandler` line of a record, including the trailing "\n".
pub fn format_text_record(handler_attrs: &[Attr], r: &Record, zone: &dyn Zone) -> Vec<u8> {
    todo!()
}
