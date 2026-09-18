//! Golden tests of `dstore-gocompat` `slog` and `ctx`.
//!
//! - `gocompat/slog.json` (family `gocompat-slog`, schema in `tools/vectorgen/docs/gocompat-d.md`):
//!   `Level` display, `log_level`, `needs_quoting`, `format_text_record` with `TextHandler` and
//!   `Logger::with`, and `format_default_record`.
//! - `ctx`: no vectors. These tests run on tokio's paused clock, which the crate's unit tests cannot
//!   enable.

use std::io::{self, Write};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use dstore_gocompat::ctx::{Ctx, CtxError};
use dstore_gocompat::slog::{self, Attr, Handler, Level, Logger, Record, TextHandler, Value};
use dstore_gocompat::time::{FixedZone, GoTime};
use dstore_testkit::golden;
use serde::Deserialize;
use tokio::time::Instant;

const VECTORS: &str = "gocompat/slog.json";

#[derive(Deserialize)]
struct Vectors {
    levels: Vec<LevelCase>,
    log_levels: Vec<LogLevelCase>,
    needs_quoting: Vec<QuotingCase>,
    records: Vec<RecordCase>,
    default_logger: Vec<DefaultCase>,
}

#[derive(Deserialize)]
struct LevelCase {
    level: i32,
    text: String,
}

#[derive(Deserialize)]
struct LogLevelCase {
    #[serde(rename = "in")]
    input: String,
    level: i32,
}

#[derive(Deserialize)]
struct QuotingCase {
    in_hex: String,
    valid_utf8: bool,
    needs_quoting: bool,
}

#[derive(Deserialize, Clone, Copy)]
struct TimeCase {
    #[serde(deserialize_with = "golden::decimal_i64")]
    unix_secs: i64,
    nanos: u32,
}

#[derive(Deserialize)]
struct AttrCase {
    key: String,
    kind: String,
    #[serde(rename = "str")]
    text: Option<String>,
    num: Option<String>,
    #[serde(rename = "bool")]
    flag: Option<bool>,
    time: Option<TimeCase>,
    hex: Option<String>,
}

#[derive(Deserialize)]
struct RecordCase {
    name: String,
    offset_secs: i32,
    time: Option<TimeCase>,
    level: i32,
    msg: String,
    with: Vec<Vec<AttrCase>>,
    attrs: Vec<AttrCase>,
    line: String,
}

#[derive(Deserialize)]
struct DefaultCase {
    name: String,
    offset_secs: i32,
    now: TimeCase,
    level: i32,
    msg: String,
    with: Vec<Vec<AttrCase>>,
    attrs: Vec<AttrCase>,
    line: String,
}

fn vectors() -> Vectors {
    golden::load_json(VECTORS)
}

fn go_time(t: TimeCase) -> GoTime {
    GoTime {
        unix_secs: t.unix_secs,
        nanos: t.nanos,
    }
}

fn num<T>(a: &AttrCase) -> T
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    match a.num.as_deref().map(str::parse::<T>) {
        Some(Ok(n)) => n,
        Some(Err(e)) => panic!("attribute {:?}: bad num: {e}", a.key),
        None => panic!("attribute {:?} of kind {} has no num", a.key, a.kind),
    }
}

fn text(a: &AttrCase) -> String {
    match &a.text {
        Some(s) => s.clone(),
        None => panic!("attribute {:?} of kind {} has no str", a.key, a.kind),
    }
}

fn attr(a: &AttrCase) -> Attr {
    let value = match a.kind.as_str() {
        "string" => Value::String(text(a)),
        "any" => Value::Any(text(a)),
        "int64" => Value::Int64(num(a)),
        "uint64" => Value::Uint64(num(a)),
        "float64" => Value::Float64(f64::from_bits(num(a))),
        "duration" => Value::Duration(num(a)),
        "bool" => match a.flag {
            Some(b) => Value::Bool(b),
            None => panic!("attribute {:?} has no bool", a.key),
        },
        "time" => match a.time {
            Some(t) => Value::Time(go_time(t)),
            None => panic!("attribute {:?} has no time", a.key),
        },
        "bytes" => match &a.hex {
            Some(h) => Value::Bytes(golden::hex(h)),
            None => panic!("attribute {:?} has no hex", a.key),
        },
        k => panic!("attribute {:?}: unknown kind {k:?}", a.key),
    };
    Attr {
        key: a.key.clone(),
        value,
    }
}

fn attrs(list: &[AttrCase]) -> Vec<Attr> {
    list.iter().map(attr).collect()
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// A writer recording every `write` call.
#[derive(Clone, Default)]
struct Writes(Arc<Mutex<Vec<Vec<u8>>>>);

impl Write for Writes {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        lock(&self.0).push(b.to_vec());
        Ok(b.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A handler keeping what it is given, at every level.
#[derive(Default)]
struct Keep(Mutex<Vec<(Vec<Attr>, Record)>>);

impl Handler for Keep {
    fn enabled(&self, _level: Level) -> bool {
        true
    }

    fn handle(&self, handler_attrs: &[Attr], r: &Record) {
        lock(&self.0).push((handler_attrs.to_vec(), r.clone()));
    }
}

#[test]
fn level_display() {
    let v = vectors();
    assert!(!v.levels.is_empty());
    for c in &v.levels {
        assert_eq!(Level(c.level).to_string(), c.text, "Level({})", c.level);
    }
}

#[test]
fn log_level() {
    let v = vectors();
    assert!(!v.log_levels.is_empty());
    for c in &v.log_levels {
        assert_eq!(slog::log_level(&c.input), Level(c.level), "{:?}", c.input);
    }
}

#[test]
fn needs_quoting() {
    let v = vectors();
    assert!(!v.needs_quoting.is_empty());
    for c in &v.needs_quoting {
        let bytes = golden::hex(&c.in_hex);
        match std::str::from_utf8(&bytes) {
            Ok(s) => {
                assert!(c.valid_utf8, "{s:?} is valid UTF-8");
                assert_eq!(slog::needs_quoting(s), c.needs_quoting, "{s:?}");
            }
            Err(_) => {
                assert!(!c.valid_utf8, "{} is invalid UTF-8", c.in_hex);
                // Go quotes invalid UTF-8 (RuneError). A Rust caller holds the lossy string, whose U+FFFD
                // is quoted too (PORTING.md DD-8).
                assert!(c.needs_quoting, "{}", c.in_hex);
                assert!(
                    slog::needs_quoting(&String::from_utf8_lossy(&bytes)),
                    "{}",
                    c.in_hex
                );
            }
        }
    }
}

#[test]
fn text_handler_records() {
    let v = vectors();
    assert!(!v.records.is_empty());
    for c in &v.records {
        let handler_attrs: Vec<Attr> = c.with.iter().flat_map(|w| attrs(w)).collect();
        let r = Record {
            time: c.time.map(go_time),
            level: Level(c.level),
            message: c.msg.clone(),
            attrs: attrs(&c.attrs),
        };
        let line = slog::format_text_record(&handler_attrs, &r, &FixedZone(c.offset_secs));
        assert_eq!(
            String::from_utf8_lossy(&line),
            c.line,
            "record {:?}",
            c.name
        );
        assert_eq!(line, c.line.as_bytes(), "record {:?}", c.name);

        // TextHandler writes the same bytes in one write.
        let w = Writes::default();
        let h = TextHandler::new(
            Box::new(w.clone()),
            Level(i32::MIN),
            Arc::new(FixedZone(c.offset_secs)),
        );
        h.handle(&handler_attrs, &r);
        assert_eq!(
            *lock(&w.0),
            vec![c.line.as_bytes().to_vec()],
            "record {:?}",
            c.name
        );
    }
}

#[test]
fn logger_with_matches_go_with() {
    // One Logger::with per Go With call: the handler gets the attributes of every call, in order, before
    // the record's.
    let v = vectors();
    for c in &v.records {
        let keep = Arc::new(Keep::default());
        let mut log = Logger::new(keep.clone());
        for w in &c.with {
            log = log.with(attrs(w));
        }
        log.log(Level(c.level), &c.msg, attrs(&c.attrs));
        let got = lock(&keep.0).clone();
        assert_eq!(got.len(), 1, "record {:?}", c.name);
        let (handler_attrs, r) = &got[0];
        assert!(r.time.is_some());
        let r = Record {
            time: c.time.map(go_time),
            ..r.clone()
        };
        let line = slog::format_text_record(handler_attrs, &r, &FixedZone(c.offset_secs));
        assert_eq!(
            String::from_utf8_lossy(&line),
            c.line,
            "record {:?}",
            c.name
        );
    }
}

#[test]
fn default_logger_lines() {
    let v = vectors();
    assert!(!v.default_logger.is_empty());
    for c in &v.default_logger {
        let handler_attrs: Vec<Attr> = c.with.iter().flat_map(|w| attrs(w)).collect();
        let r = Record {
            time: None,
            level: Level(c.level),
            message: c.msg.clone(),
            attrs: attrs(&c.attrs),
        };
        let line = slog::format_default_record(
            &handler_attrs,
            &r,
            go_time(c.now),
            &FixedZone(c.offset_secs),
        );
        assert_eq!(
            String::from_utf8_lossy(&line),
            c.line,
            "default logger {:?}",
            c.name
        );
        assert_eq!(line, c.line.as_bytes(), "default logger {:?}", c.name);
    }
}

#[tokio::test(start_paused = true)]
async fn ctx_deadline_fires_at_its_instant() {
    let start = Instant::now();
    let c = Ctx::background().with_timeout(Duration::from_secs(5));
    assert_eq!(c.deadline(), Some(start + Duration::from_secs(5)));
    tokio::time::advance(Duration::from_millis(4999)).await;
    assert_eq!(c.err(), None);
    tokio::time::advance(Duration::from_millis(1)).await;
    assert_eq!(c.err(), Some(CtxError::DeadlineExceeded));
    c.done().await;
    assert_eq!(Instant::now() - start, Duration::from_secs(5));
}

#[tokio::test(start_paused = true)]
async fn ctx_nested_timeouts() {
    let start = Instant::now();
    let parent = Ctx::background().with_timeout(Duration::from_secs(10));
    let child = parent.with_timeout(Duration::from_secs(20));
    assert_eq!(child.deadline(), Some(start + Duration::from_secs(10)));
    let short = parent.with_timeout(Duration::from_secs(3));
    assert_eq!(short.deadline(), Some(start + Duration::from_secs(3)));

    assert_eq!(
        short.sleep(Duration::from_secs(60)).await,
        Err(CtxError::DeadlineExceeded)
    );
    let elapsed = Instant::now() - start;
    assert!(
        elapsed >= Duration::from_secs(3) && elapsed < Duration::from_secs(4),
        "{elapsed:?}"
    );
    assert_eq!(parent.err(), None);
    assert_eq!(child.err(), None);

    child.done().await;
    let elapsed = Instant::now() - start;
    assert!(
        elapsed >= Duration::from_secs(10) && elapsed < Duration::from_secs(11),
        "{elapsed:?}"
    );
    assert_eq!(child.err(), Some(CtxError::DeadlineExceeded));
    assert_eq!(parent.err(), Some(CtxError::DeadlineExceeded));
}

#[tokio::test(start_paused = true)]
async fn ctx_events_at_one_instant() {
    // A zero timeout has ended before a cancel at the same instant.
    let c = Ctx::background().with_timeout(Duration::ZERO);
    c.cancel();
    assert_eq!(c.err(), Some(CtxError::DeadlineExceeded));

    // A child of an ended parent takes the parent's error, even with a zero timeout of its own.
    let p = Ctx::background().with_cancel();
    p.cancel();
    assert_eq!(
        p.with_timeout(Duration::ZERO).err(),
        Some(CtxError::Canceled)
    );

    // A cancel at the deadline instant comes too late.
    let d = Ctx::background().with_timeout(Duration::from_secs(1));
    tokio::time::advance(Duration::from_secs(1)).await;
    d.cancel();
    assert_eq!(d.err(), Some(CtxError::DeadlineExceeded));

    // A cancel just before the deadline stays the cause.
    let e = Ctx::background().with_timeout(Duration::from_secs(1));
    let child = e.with_cancel();
    tokio::time::advance(Duration::from_millis(999)).await;
    e.cancel();
    tokio::time::advance(Duration::from_secs(5)).await;
    assert_eq!(e.err(), Some(CtxError::Canceled));
    assert_eq!(child.err(), Some(CtxError::Canceled));

    // A child cancelled at its parent's deadline instant inherits the deadline.
    let p = Ctx::background().with_timeout(Duration::from_secs(1));
    let child = p.with_cancel();
    tokio::time::advance(Duration::from_secs(1)).await;
    child.cancel();
    assert_eq!(child.err(), Some(CtxError::DeadlineExceeded));
}

#[tokio::test(start_paused = true)]
async fn ctx_run_and_sleep() {
    let bg = Ctx::background();
    let start = Instant::now();
    assert_eq!(bg.sleep(Duration::from_secs(30)).await, Ok(()));
    assert!(Instant::now() - start >= Duration::from_secs(30));

    let t = bg.with_timeout(Duration::from_secs(2));
    assert_eq!(
        t.run(tokio::time::sleep(Duration::from_secs(5))).await,
        Err(CtxError::DeadlineExceeded)
    );
    assert_eq!(t.run(async { 7 }).await, Err(CtxError::DeadlineExceeded));
    assert_eq!(
        bg.with_timeout(Duration::from_secs(5))
            .run(async {
                tokio::time::sleep(Duration::from_secs(1)).await;
                7
            })
            .await,
        Ok(7)
    );

    let c = bg.with_cancel();
    let canceller = {
        let c = c.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(4)).await;
            c.cancel();
        })
    };
    assert_eq!(
        c.sleep(Duration::from_secs(3600)).await,
        Err(CtxError::Canceled)
    );
    if let Err(e) = canceller.await {
        panic!("canceller failed: {e}");
    }
}
