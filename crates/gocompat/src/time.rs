//! Go `time`: `Duration.String`/`Round`, `ParseDuration`, the layouts dstore formats and parses
//! (RFC3339, RFC3339Nano, the slog and log-package layouts, `15:04:05`) and the local zone.
//!
//! The duration and RFC3339 helpers are copied from core-rs `examples/amber-store.rs`
//! (LGPL-3.0-only).

/// `time.Millisecond` in nanoseconds.
pub const MILLISECOND: i64 = 1_000_000;
/// `time.Second` in nanoseconds.
pub const SECOND: i64 = 1_000_000_000;

/// `time.Duration.String`: "0s", "1.5µs", "2m0s", "-1s".
pub fn duration_string(ns: i64) -> String {
    todo!()
}

/// `time.Duration.Round`: half away from zero, saturating.
pub fn duration_round(ns: i64, m: i64) -> i64 {
    todo!()
}

/// `time.ParseDuration` with Go's error texts.
pub fn parse_duration(s: &str) -> Result<i64, String> {
    todo!()
}

/// A non-negative `std::time::Duration` as Go nanoseconds, saturating.
pub fn duration_to_ns(d: std::time::Duration) -> i64 {
    todo!()
}

/// An instant (Go `time.Time` without a location), UTC based.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct GoTime {
    pub unix_secs: i64,
    pub nanos: u32,
}

impl GoTime {
    /// `time.Now()`.
    pub fn now() -> GoTime {
        todo!()
    }

    /// `time.Unix(0, ns)`: floor division for negative `ns`.
    pub fn from_unix_nano(ns: i64) -> GoTime {
        todo!()
    }

    /// `Time.UnixNano`, wrapping as Go does.
    pub fn unix_nano(&self) -> i64 {
        todo!()
    }

    /// `Time.Add`.
    pub fn add_ns(&self, ns: i64) -> GoTime {
        todo!()
    }
}

/// A time zone: the offset east of UTC, in seconds, at an instant.
pub trait Zone: Send + Sync {
    fn offset_at(&self, unix_secs: i64) -> i32;
}

/// The local zone: libc `localtime_r` + `tm_gmtoff` (TZ honoured as Go's `initLocal` does where tzdata
/// exists).
pub struct SystemZone;

/// A fixed offset in seconds east of UTC (tests, vectors).
pub struct FixedZone(pub i32);

impl Zone for SystemZone {
    fn offset_at(&self, unix_secs: i64) -> i32 {
        todo!()
    }
}

impl Zone for FixedZone {
    fn offset_at(&self, unix_secs: i64) -> i32 {
        todo!()
    }
}

/// `t.In(zone).Format(time.RFC3339)`: "Z" for offset 0.
pub fn format_rfc3339(t: GoTime, zone: &dyn Zone) -> String {
    todo!()
}

/// `t.UTC().Format(time.RFC3339Nano)`: trailing zeros trimmed, "Z".
pub fn format_rfc3339nano_utc(t: GoTime) -> String {
    todo!()
}

/// The slog layout "2006-01-02T15:04:05.000Z07:00", truncated to milliseconds.
pub fn format_slog_time(t: GoTime, zone: &dyn Zone) -> String {
    todo!()
}

/// `Format("15:04:05")`.
pub fn format_clock(t: GoTime, zone: &dyn Zone) -> String {
    todo!()
}

/// The log package layout "2006/01/02 15:04:05" (`slog.Default()`).
pub fn format_log_std(t: GoTime, zone: &dyn Zone) -> String {
    todo!()
}

/// `time.Parse(time.RFC3339Nano, s)`: accepts ±hh:mm; Go error texts, e.g.
/// `parsing time "" as "2006-01-02T15:04:05.999999999Z07:00": cannot parse "" as "2006"`.
pub fn parse_rfc3339nano(s: &str) -> Result<GoTime, String> {
    todo!()
}
