//! Go `time`: `Duration.String`/`Round`, `ParseDuration`, the layouts dstore formats and parses
//! (RFC3339, RFC3339Nano, the slog and log-package layouts, `15:04:05`) and the local zone.
//!
//! The duration and RFC3339 helpers are copied from core-rs `examples/amber-store.rs`
//! (LGPL-3.0-only) and completed against go1.26.5 `src/time` (`time.go`, `format.go`,
//! `format_rfc3339.go`, `zoneinfo_unix.go`), `src/log/log.go` and `src/log/slog/handler.go`.

use std::sync::OnceLock;

/// `time.Millisecond` in nanoseconds.
pub const MILLISECOND: i64 = 1_000_000;
/// `time.Second` in nanoseconds.
pub const SECOND: i64 = 1_000_000_000;

const MICROSECOND_U: u64 = 1_000;
const MILLISECOND_U: u64 = 1_000_000;
const SECOND_U: u64 = 1_000_000_000;

/// `time.Duration.String`: "0s", "1.5µs", "2m0s", "-1s".
pub fn duration_string(ns: i64) -> String {
    // Largest time is 2540400h10m10.000000000s
    let mut buf = [0u8; 32];
    let mut w = buf.len();
    let neg = ns < 0;
    let mut u = ns.unsigned_abs();

    if u < SECOND_U {
        // Special case: if duration is smaller than a second, use smaller units, like 1.2ms
        let prec;
        w -= 1;
        buf[w] = b's';
        w -= 1;
        if u == 0 {
            buf[w] = b'0';
            return String::from_utf8_lossy(&buf[w..]).into_owned();
        } else if u < MICROSECOND_U {
            // print nanoseconds
            prec = 0;
            buf[w] = b'n';
        } else if u < MILLISECOND_U {
            // print microseconds; U+00B5 'µ' micro sign == 0xC2 0xB5
            prec = 3;
            w -= 1; // Need room for two bytes.
            buf[w] = 0xC2;
            buf[w + 1] = 0xB5;
        } else {
            // print milliseconds
            prec = 6;
            buf[w] = b'm';
        }
        (w, u) = fmt_frac(&mut buf, w, u, prec);
        w = fmt_int(&mut buf, w, u);
    } else {
        w -= 1;
        buf[w] = b's';
        (w, u) = fmt_frac(&mut buf, w, u, 9);
        // u is now integer seconds
        w = fmt_int(&mut buf, w, u % 60);
        u /= 60;
        // u is now integer minutes
        if u > 0 {
            w -= 1;
            buf[w] = b'm';
            w = fmt_int(&mut buf, w, u % 60);
            u /= 60;
            // u is now integer hours. Stop at hours because days can be different lengths.
            if u > 0 {
                w -= 1;
                buf[w] = b'h';
                w = fmt_int(&mut buf, w, u);
            }
        }
    }
    if neg {
        w -= 1;
        buf[w] = b'-';
    }
    String::from_utf8_lossy(&buf[w..]).into_owned()
}

/// `fmtFrac`: the fraction of v/10**prec into the tail of `buf[..w]`, trailing zeros (and the point)
/// omitted; returns the new start and v/10**prec.
fn fmt_frac(buf: &mut [u8; 32], mut w: usize, mut v: u64, prec: u32) -> (usize, u64) {
    let mut print = false;
    for _ in 0..prec {
        let digit = v % 10;
        print = print || digit != 0;
        if print {
            w -= 1;
            buf[w] = digit as u8 + b'0';
        }
        v /= 10;
    }
    if print {
        w -= 1;
        buf[w] = b'.';
    }
    (w, v)
}

/// `fmtInt`: v into the tail of `buf[..w]`; returns the new start.
fn fmt_int(buf: &mut [u8; 32], mut w: usize, mut v: u64) -> usize {
    if v == 0 {
        w -= 1;
        buf[w] = b'0';
    } else {
        while v > 0 {
            w -= 1;
            buf[w] = (v % 10) as u8 + b'0';
            v /= 10;
        }
    }
    w
}

/// `time.Duration.Round`: half away from zero, saturating.
pub fn duration_round(ns: i64, m: i64) -> i64 {
    if m <= 0 {
        return ns;
    }
    let mut r = ns % m;
    if ns < 0 {
        r = -r;
        if less_than_half(r, m) {
            return ns + r;
        }
        let d1 = ns.wrapping_sub(m).wrapping_add(r);
        if d1 < ns {
            return d1;
        }
        return i64::MIN; // overflow
    }
    if less_than_half(r, m) {
        return ns - r;
    }
    let d1 = ns.wrapping_add(m).wrapping_sub(r);
    if d1 > ns {
        return d1;
    }
    i64::MAX // overflow
}

/// `lessThanHalf`: x+x < y without overflow, for non-negative x and y.
fn less_than_half(x: i64, y: i64) -> bool {
    (x as u64).wrapping_add(x as u64) < y as u64
}

/// The time package's own `quote`: `\xNN` for every byte outside printable ASCII, `\"` and `\\`.
fn tquote(s: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for &c in s {
        if !(b' '..0x80).contains(&c) {
            out.push_str("\\x");
            out.push(char::from(HEX[usize::from(c >> 4)]));
            out.push(char::from(HEX[usize::from(c & 0xF)]));
        } else {
            if c == b'"' || c == b'\\' {
                out.push('\\');
            }
            out.push(char::from(c));
        }
    }
    out.push('"');
    out
}

/// `time.ParseDuration` with Go's error texts.
pub fn parse_duration(s: &str) -> Result<i64, String> {
    // [-+]?([0-9]*(\.[0-9]*)?[a-z]+)+
    let orig = s.as_bytes();
    let invalid = || format!("time: invalid duration {}", tquote(orig));
    let mut s = orig;
    let mut d: u64 = 0;
    let mut neg = false;

    // Consume [-+]?
    if let Some(&c) = s.first()
        && (c == b'-' || c == b'+')
    {
        neg = c == b'-';
        s = &s[1..];
    }
    // Special case: if all that is left is "0", this is zero.
    if s == b"0" {
        return Ok(0);
    }
    if s.is_empty() {
        return Err(invalid());
    }
    while !s.is_empty() {
        // The next character must be [0-9.]
        if !(s[0] == b'.' || s[0].is_ascii_digit()) {
            return Err(invalid());
        }
        // Consume [0-9]*
        let pl = s.len();
        let Some((mut v, rest)) = leading_int(s) else {
            return Err(invalid());
        };
        s = rest;
        let pre = pl != s.len(); // whether we consumed anything before a period

        // Consume (\.[0-9]*)?
        let mut post = false;
        let mut f: u64 = 0;
        let mut scale: f64 = 1.0;
        if !s.is_empty() && s[0] == b'.' {
            s = &s[1..];
            let pl = s.len();
            (f, scale, s) = leading_fraction(s);
            post = pl != s.len();
        }
        if !pre && !post {
            // no digits (e.g. ".s" or "-.s")
            return Err(invalid());
        }

        // Consume unit.
        let i = s
            .iter()
            .position(|&c| c == b'.' || c.is_ascii_digit())
            .unwrap_or(s.len());
        if i == 0 {
            return Err(format!("time: missing unit in duration {}", tquote(orig)));
        }
        let (u, rest) = s.split_at(i);
        s = rest;
        let unit: u64 = match u {
            b"ns" => 1,
            // U+00B5 (micro symbol) and U+03BC (Greek letter mu)
            b"us" | b"\xc2\xb5s" | b"\xce\xbcs" => MICROSECOND_U,
            b"ms" => MILLISECOND_U,
            b"s" => SECOND_U,
            b"m" => 60 * SECOND_U,
            b"h" => 3600 * SECOND_U,
            _ => {
                return Err(format!(
                    "time: unknown unit {} in duration {}",
                    tquote(u),
                    tquote(orig)
                ));
            }
        };
        if v > (1u64 << 63) / unit {
            // overflow
            return Err(invalid());
        }
        v *= unit;
        if f > 0 {
            // float64 is needed to be nanosecond accurate for fractions of hours.
            // v >= 0 && (f*unit/scale) <= 3.6e+12 (ns/h, h is the largest unit)
            v = v.wrapping_add((f as f64 * (unit as f64 / scale)) as u64);
            if v > 1 << 63 {
                // overflow
                return Err(invalid());
            }
        }
        d = d.wrapping_add(v);
        if d > 1 << 63 {
            return Err(invalid());
        }
    }
    if neg {
        return Ok((d as i64).wrapping_neg());
    }
    if d > (1 << 63) - 1 {
        return Err(invalid());
    }
    Ok(d as i64)
}

/// `leadingInt`: the leading `[0-9]*`; `None` on overflow past 1<<63.
fn leading_int(s: &[u8]) -> Option<(u64, &[u8])> {
    let mut x: u64 = 0;
    let mut i = 0;
    while i < s.len() {
        let c = s[i];
        if !c.is_ascii_digit() {
            break;
        }
        if x > (1 << 63) / 10 {
            // overflow
            return None;
        }
        x = x * 10 + u64::from(c - b'0');
        if x > 1 << 63 {
            // overflow
            return None;
        }
        i += 1;
    }
    Some((x, &s[i..]))
}

/// `leadingFraction`: the leading `[0-9]*` as value and scale; stops accumulating on overflow.
fn leading_fraction(s: &[u8]) -> (u64, f64, &[u8]) {
    let mut i = 0;
    let mut x: u64 = 0;
    let mut scale: f64 = 1.0;
    let mut overflow = false;
    while i < s.len() {
        let c = s[i];
        if !c.is_ascii_digit() {
            break;
        }
        i += 1;
        if overflow {
            continue;
        }
        if x > ((1u64 << 63) - 1) / 10 {
            // It's possible for overflow to give a positive number, so take care.
            overflow = true;
            continue;
        }
        let y = x * 10 + u64::from(c - b'0');
        if y > 1 << 63 {
            overflow = true;
            continue;
        }
        x = y;
        scale *= 10.0;
    }
    (x, scale, &s[i..])
}

/// A non-negative `std::time::Duration` as Go nanoseconds, saturating.
pub fn duration_to_ns(d: std::time::Duration) -> i64 {
    i64::try_from(d.as_nanos()).unwrap_or(i64::MAX)
}

/// `(1969*365 + 1969/4 - 1969/100 + 1969/400) * secondsPerDay`: Unix seconds to Go internal seconds.
const UNIX_TO_INTERNAL: i64 = (1969 * 365 + 1969 / 4 - 1969 / 100 + 1969 / 400) * SECONDS_PER_DAY;
const SECONDS_PER_DAY: i64 = 86_400;
/// `absoluteYears`: years subtracted from internal time to get absolute time.
const ABSOLUTE_YEARS: i64 = 292_277_022_400;
/// Days from March 1 through end of year.
const MARCH_THRU_DECEMBER: i64 = 31 + 30 + 31 + 30 + 31 + 31 + 30 + 31 + 30 + 31;
/// `absoluteToInternal = -(absoluteYears*365.2425 + marchThruDecember) * secondsPerDay`.
const ABSOLUTE_TO_INTERNAL: i64 =
    -((ABSOLUTE_YEARS * 3_652_425 / 10_000 + MARCH_THRU_DECEMBER) * SECONDS_PER_DAY);
/// `unixToInternal + internalToAbsolute`.
const UNIX_TO_ABSOLUTE: i64 = UNIX_TO_INTERNAL - ABSOLUTE_TO_INTERNAL;
/// `absoluteToInternal + internalToUnix`.
const ABSOLUTE_TO_UNIX: i64 = ABSOLUTE_TO_INTERNAL - UNIX_TO_INTERNAL;

/// An instant (Go `time.Time` without a location), UTC based.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct GoTime {
    pub unix_secs: i64,
    pub nanos: u32,
}

impl GoTime {
    /// `time.Now()`.
    pub fn now() -> GoTime {
        match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            Ok(d) => GoTime {
                unix_secs: i64::try_from(d.as_secs()).unwrap_or(i64::MAX),
                nanos: d.subsec_nanos(),
            },
            Err(e) => {
                // Before 1970: floor the seconds so nanos stay in [0, 1e9).
                let d = e.duration();
                let secs = i64::try_from(d.as_secs()).unwrap_or(i64::MAX);
                if d.subsec_nanos() == 0 {
                    GoTime {
                        unix_secs: -secs,
                        nanos: 0,
                    }
                } else {
                    GoTime {
                        unix_secs: -secs - 1,
                        nanos: 1_000_000_000 - d.subsec_nanos(),
                    }
                }
            }
        }
    }

    /// `time.Unix(0, ns)`: floor division for negative `ns`.
    pub fn from_unix_nano(ns: i64) -> GoTime {
        GoTime {
            unix_secs: ns.div_euclid(SECOND),
            nanos: ns.rem_euclid(SECOND) as u32,
        }
    }

    /// `Time.UnixNano`, wrapping as Go does.
    pub fn unix_nano(&self) -> i64 {
        self.unix_secs
            .wrapping_mul(SECOND)
            .wrapping_add(i64::from(self.nanos))
    }

    /// `Time.Add`.
    pub fn add_ns(&self, ns: i64) -> GoTime {
        let mut dsec = ns / SECOND;
        let mut nsec = i64::from(self.nanos) + ns % SECOND;
        if nsec >= SECOND {
            dsec += 1;
            nsec -= SECOND;
        } else if nsec < 0 {
            dsec -= 1;
            nsec += SECOND;
        }
        GoTime {
            unix_secs: add_sec(self.unix_secs, dsec),
            nanos: nsec as u32,
        }
    }
}

/// `Time.addSec` on the internal seconds (since year 1), saturating as Go does.
fn add_sec(unix_secs: i64, d: i64) -> i64 {
    let ext = unix_secs.wrapping_add(UNIX_TO_INTERNAL);
    let sum = ext.wrapping_add(d);
    let ext = if (sum > ext) == (d > 0) {
        sum
    } else if d > 0 {
        i64::MAX
    } else {
        -i64::MAX
    };
    ext.wrapping_sub(UNIX_TO_INTERNAL)
}

/// A time zone: the offset east of UTC, in seconds, at an instant.
pub trait Zone: Send + Sync {
    fn offset_at(&self, unix_secs: i64) -> i32;
}

/// The local zone: libc `localtime_r` + `tm_gmtoff` (TZ honoured as Go's `initLocal` does where tzdata
/// exists).
///
/// Go's `initLocal` falls back to UTC when `$TZ` is set but names nothing it can load: `TZ=""`,
/// `TZ=UTC`, `TZ=:`, or a name with no TZif file in its zoneinfo directories. libc would instead read
/// POSIX rules such as `XYZ-3`, so those cases give offset 0 here; every other case asks libc.
pub struct SystemZone;

/// A fixed offset in seconds east of UTC (tests, vectors).
pub struct FixedZone(pub i32);

impl Zone for SystemZone {
    fn offset_at(&self, unix_secs: i64) -> i32 {
        static GO_LOCAL_IS_UTC: OnceLock<bool> = OnceLock::new();
        let utc = *GO_LOCAL_IS_UTC.get_or_init(|| {
            let tz = std::env::var_os("TZ");
            go_local_is_utc(
                tz.as_ref()
                    .map(|v| std::os::unix::ffi::OsStrExt::as_bytes(v.as_os_str())),
                is_tzif_file,
            )
        });
        if utc {
            return 0;
        }
        libc_gmtoff(unix_secs)
    }
}

impl Zone for FixedZone {
    fn offset_at(&self, _unix_secs: i64) -> i32 {
        self.0
    }
}

/// Go's `platformZoneSources` (`zoneinfo_unix.go`, standard toolchains).
const PLATFORM_ZONE_SOURCES: [&str; 4] = [
    "/usr/share/zoneinfo/",
    "/usr/share/lib/zoneinfo/",
    "/usr/lib/locale/TZ/",
    "/etc/zoneinfo",
];

/// Whether Go's `initLocal` ends in its UTC fallback for this `$TZ` value (`None` = unset), given a
/// predicate telling whether a path holds loadable TZif data.
fn go_local_is_utc(tz: Option<&[u8]>, loadable: impl Fn(&std::path::Path) -> bool) -> bool {
    let Some(tz) = tz else {
        // No $TZ: /etc/localtime, which libc reads too.
        return false;
    };
    let tz = tz.strip_prefix(b":").unwrap_or(tz);
    if tz.is_empty() {
        return true;
    }
    let path = |b: &[u8]| {
        std::path::PathBuf::from(<std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(b))
    };
    if tz[0] == b'/' {
        return !loadable(&path(tz));
    }
    if tz == b"UTC" {
        return true;
    }
    !PLATFORM_ZONE_SOURCES.iter().any(|dir| {
        let mut p = dir.as_bytes().to_vec();
        p.push(b'/');
        p.extend_from_slice(tz);
        loadable(&path(&p))
    })
}

/// A readable file of at most 10 MiB (Go's `maxFileSize`) starting with the TZif magic.
fn is_tzif_file(p: &std::path::Path) -> bool {
    use std::io::Read;
    let Ok(md) = std::fs::metadata(p) else {
        return false;
    };
    if !md.is_file() || md.len() > 10 << 20 {
        return false;
    }
    let Ok(mut f) = std::fs::File::open(p) else {
        return false;
    };
    let mut magic = [0u8; 4];
    f.read_exact(&mut magic).is_ok() && &magic == b"TZif"
}

/// `tm_gmtoff` of `localtime_r(unix_secs)`; 0 when libc cannot convert the instant.
fn libc_gmtoff(unix_secs: i64) -> i32 {
    // `time_t` is 64-bit on every supported target (PORTING.md §5.13).
    let t: libc::time_t = unix_secs;
    // SAFETY: `libc::tm` is a plain C struct; all-zero bytes are a valid value (null `tm_zone`).
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: `localtime_r` reads `t` and writes only into `tm`, both live locals.
    let r = unsafe { libc::localtime_r(&t, &mut tm) };
    if r.is_null() {
        return 0;
    }
    i32::try_from(tm.tm_gmtoff).unwrap_or(0)
}

/// Calendar fields of an absolute time, as `absSeconds.days().date()` and `clock()` compute them.
struct Civil {
    year: i64,
    month: i64,
    day: i64,
    hour: i64,
    min: i64,
    sec: i64,
}

/// `Time.locabs` for a zone: the absolute seconds of `t` shifted by the zone offset.
fn abs_seconds(t: GoTime, zone: &dyn Zone) -> (i64, u64) {
    let offset = i64::from(zone.offset_at(t.unix_secs));
    let sec = t.unix_secs.wrapping_add(offset);
    (offset, sec.wrapping_add(UNIX_TO_ABSOLUTE) as u64)
}

/// `absDays.date` and `absSeconds.clock`.
fn civil(abs: u64) -> Civil {
    let days = abs / SECONDS_PER_DAY as u64;

    // days.split(): century, cyear, ayday
    let d = days.wrapping_mul(4).wrapping_add(3);
    let century = d / 146_097;
    let cd = ((d % 146_097) as u32) | 3;
    let prod = u64::from(cd) * 2_939_745;
    let cyear = (prod >> 32) as i64;
    let ayday = u64::from(prod as u32) / 2_939_745 / 4;

    // ayday.split(): amonth, mday
    let dm = 2141 * ayday as u32 + 197_913;
    let amonth = i64::from(dm >> 16);
    let day = 1 + i64::from((dm & 0xFFFF) / 2141);

    // janFeb, year, month
    let jan_feb = i64::from(ayday >= MARCH_THRU_DECEMBER as u64);
    let year = (century
        .wrapping_mul(100)
        .wrapping_sub(ABSOLUTE_YEARS as u64) as i64)
        .wrapping_add(cyear)
        .wrapping_add(jan_feb);
    let month = amonth - jan_feb * 12;

    // abs.clock()
    let mut sec = (abs % SECONDS_PER_DAY as u64) as i64;
    let hour = sec / 3600;
    sec -= hour * 3600;
    let min = sec / 60;
    sec -= min * 60;
    Civil {
        year,
        month,
        day,
        hour,
        min,
        sec,
    }
}

/// The time package's `appendInt`: decimal `x`, zero-padded to `width` digits (sign excluded).
fn append_int(b: &mut Vec<u8>, x: i64, width: usize) {
    let mut u = x.unsigned_abs();
    if x < 0 {
        b.push(b'-');
    }
    let mut digits = Vec::with_capacity(20);
    loop {
        digits.push(b'0' + (u % 10) as u8);
        u /= 10;
        if u == 0 {
            break;
        }
    }
    for _ in digits.len()..width {
        b.push(b'0');
    }
    b.extend(digits.iter().rev());
}

/// `appendFormatRFC3339`: RFC3339, with the trimmed fraction when `nanos`.
fn append_rfc3339(b: &mut Vec<u8>, t: GoTime, zone: &dyn Zone, nanos: bool) {
    let (offset, abs) = abs_seconds(t, zone);
    let c = civil(abs);
    append_int(b, c.year, 4);
    b.push(b'-');
    append_int(b, c.month, 2);
    b.push(b'-');
    append_int(b, c.day, 2);
    b.push(b'T');
    append_int(b, c.hour, 2);
    b.push(b':');
    append_int(b, c.min, 2);
    b.push(b':');
    append_int(b, c.sec, 2);
    if nanos && t.nanos != 0 {
        // appendNano with stdFracSecond9, 9 digits: trailing zeros and the dot trimmed.
        b.push(b'.');
        append_int(b, i64::from(t.nanos), 9);
        while b.last() == Some(&b'0') {
            b.pop();
        }
        if b.last() == Some(&b'.') {
            b.pop();
        }
    }
    if offset == 0 {
        b.push(b'Z');
        return;
    }
    let mut zone_min = offset / 60; // convert to minutes
    if zone_min < 0 {
        b.push(b'-');
        zone_min = -zone_min;
    } else {
        b.push(b'+');
    }
    append_int(b, zone_min / 60, 2);
    b.push(b':');
    append_int(b, zone_min % 60, 2);
}

fn into_string(b: Vec<u8>) -> String {
    String::from_utf8(b).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

/// `t.In(zone).Format(time.RFC3339)`: "Z" for offset 0.
pub fn format_rfc3339(t: GoTime, zone: &dyn Zone) -> String {
    let mut b = Vec::with_capacity(25);
    append_rfc3339(&mut b, t, zone, false);
    into_string(b)
}

/// `t.UTC().Format(time.RFC3339Nano)`: trailing zeros trimmed, "Z".
pub fn format_rfc3339nano_utc(t: GoTime) -> String {
    let mut b = Vec::with_capacity(30);
    append_rfc3339(&mut b, t, &FixedZone(0), true);
    into_string(b)
}

/// The slog layout "2006-01-02T15:04:05.000Z07:00", truncated to milliseconds.
///
/// slog's `appendRFC3339Millis`: truncate to ms, add 100µs so RFC3339Nano keeps four fraction digits,
/// then drop the byte at offset 23 (the fourth digit for four-digit years).
pub fn format_slog_time(t: GoTime, zone: &dyn Zone) -> String {
    let truncated = GoTime {
        unix_secs: t.unix_secs,
        nanos: t.nanos - t.nanos % MILLISECOND as u32,
    };
    let t = truncated.add_ns(MILLISECOND / 10);
    let mut b = Vec::with_capacity(30);
    append_rfc3339(&mut b, t, zone, true);
    const PREFIX_LEN: usize = "2006-01-02T15:04:05.000".len();
    if b.len() > PREFIX_LEN {
        b.remove(PREFIX_LEN);
    }
    into_string(b)
}

/// `Format("15:04:05")`.
pub fn format_clock(t: GoTime, zone: &dyn Zone) -> String {
    let (_, abs) = abs_seconds(t, zone);
    let c = civil(abs);
    let mut b = Vec::with_capacity(8);
    append_int(&mut b, c.hour, 2);
    b.push(b':');
    append_int(&mut b, c.min, 2);
    b.push(b':');
    append_int(&mut b, c.sec, 2);
    into_string(b)
}

/// The log package's `itoa`: fixed-width decimal, including its byte arithmetic for negative values.
fn log_itoa(b: &mut Vec<u8>, i: i64, wid: i64) {
    let mut rev = Vec::with_capacity(20);
    let mut i = i;
    let mut wid = wid;
    while i >= 10 || wid > 1 {
        wid -= 1;
        let q = i / 10;
        rev.push((i64::from(b'0') + i - q * 10) as u8);
        i = q;
    }
    // i < 10
    rev.push((i64::from(b'0') + i) as u8);
    b.extend(rev.iter().rev());
}

/// The log package layout "2006/01/02 15:04:05" (`slog.Default()`), without the trailing space
/// `formatHeader` writes after it.
///
/// For years at or below −49000, `log.itoa`'s byte arithmetic makes Go write a first byte that is not
/// valid UTF-8; the `String` result carries U+FFFD in its place. `slog.Default()` logs the current
/// time, so this is never observed.
pub fn format_log_std(t: GoTime, zone: &dyn Zone) -> String {
    let (_, abs) = abs_seconds(t, zone);
    let c = civil(abs);
    let mut b = Vec::with_capacity(19);
    log_itoa(&mut b, c.year, 4);
    b.push(b'/');
    log_itoa(&mut b, c.month, 2);
    b.push(b'/');
    log_itoa(&mut b, c.day, 2);
    b.push(b' ');
    log_itoa(&mut b, c.hour, 2);
    b.push(b':');
    log_itoa(&mut b, c.min, 2);
    b.push(b':');
    log_itoa(&mut b, c.sec, 2);
    String::from_utf8_lossy(&b).into_owned()
}

/// `time.RFC3339Nano`.
const RFC3339NANO: &str = "2006-01-02T15:04:05.999999999Z07:00";

/// `ParseError.Error()` without a message.
fn parse_error(value: &[u8], layout_elem: &str, value_elem: &[u8]) -> String {
    format!(
        "parsing time {} as {}: cannot parse {} as {}",
        tquote(value),
        tquote(RFC3339NANO.as_bytes()),
        tquote(value_elem),
        tquote(layout_elem.as_bytes())
    )
}

/// `ParseError.Error()` with a message.
fn parse_error_msg(value: &[u8], message: &str) -> String {
    format!("parsing time {}{}", tquote(value), message)
}

/// `isDigit`.
fn is_digit(s: &[u8], i: usize) -> bool {
    s.get(i).is_some_and(u8::is_ascii_digit)
}

/// `getnum`: one or two digits (`fixed` forces two).
fn getnum(s: &[u8], fixed: bool) -> Option<(i64, &[u8])> {
    if !is_digit(s, 0) {
        return None;
    }
    if !is_digit(s, 1) {
        if fixed {
            return None;
        }
        return Some((i64::from(s[0] - b'0'), &s[1..]));
    }
    Some((
        i64::from(s[0] - b'0') * 10 + i64::from(s[1] - b'0'),
        &s[2..],
    ))
}

/// `skip` for a prefix without spaces.
fn skip<'a>(value: &'a [u8], prefix: &[u8]) -> Result<&'a [u8], &'a [u8]> {
    let mut value = value;
    for &p in prefix {
        if value.first() != Some(&p) {
            return Err(value);
        }
        value = &value[1..];
    }
    Ok(value)
}

/// `isLeap`.
fn is_leap(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// `daysIn`.
fn days_in(month: i64, year: i64) -> i64 {
    if month == 2 {
        if is_leap(year) {
            return 29;
        }
        return 28;
    }
    30 + ((month + (month >> 3)) & 1)
}

/// `dateToAbsDays`.
fn date_to_abs_days(year: i64, month: i64, day: i64) -> u64 {
    let mut amonth = month as u32;
    let jan_feb = u32::from(amonth < 3);
    amonth += 12 * jan_feb;
    let y = (year as u64)
        .wrapping_sub(u64::from(jan_feb))
        .wrapping_add(ABSOLUTE_YEARS as u64);
    let ayday = (979 * amonth - 2919) >> 5;
    let century = y / 100;
    let cyear = (y % 100) as u32;
    let cday = 1461 * cyear / 4;
    let centurydays = 146_097 * century / 4;
    centurydays.wrapping_add((i64::from(cday + ayday) + day - 1) as u64)
}

/// `Date(year, month, day, hour, min, sec, nsec, UTC)` for in-range fields.
fn date_utc(year: i64, month: i64, day: i64, hour: i64, min: i64, sec: i64, nsec: i64) -> GoTime {
    let unix = (date_to_abs_days(year, month, day) as i64)
        .wrapping_mul(SECONDS_PER_DAY)
        .wrapping_add(hour * 3600 + min * 60 + sec)
        .wrapping_add(ABSOLUTE_TO_UNIX);
    GoTime {
        unix_secs: unix,
        nanos: nsec as u32,
    }
}

/// `parseNanoseconds` over `value[..nbytes]` (value[0] is ',' or '.', then digits).
fn parse_nanoseconds(value: &[u8], nbytes: usize) -> i64 {
    let nbytes = nbytes.min(10);
    let mut ns: i64 = 0;
    for &c in &value[1..nbytes] {
        ns = ns * 10 + i64::from(c - b'0');
    }
    // We need nanoseconds, which means scaling by the number of missing digits in the format,
    // maximum length 10.
    for _ in 0..(10 - nbytes) {
        ns *= 10;
    }
    ns
}

/// `time.Parse(time.RFC3339Nano, s)`: accepts ±hh:mm; Go error texts, e.g.
/// `parsing time "" as "2006-01-02T15:04:05.999999999Z07:00": cannot parse "" as "2006"`.
///
/// This is Go's general `parse` walking the RFC3339Nano layout. Go first tries its `parseRFC3339` fast
/// path, which accepts a subset of these inputs with the same instants, so it is not ported.
pub fn parse_rfc3339nano(s: &str) -> Result<GoTime, String> {
    let avalue = s.as_bytes();
    let mut value = avalue;

    // stdLongYear "2006"
    let hold = value;
    if value.len() < 4 || !is_digit(value, 0) || !value[..4].iter().all(u8::is_ascii_digit) {
        return Err(parse_error(avalue, "2006", hold));
    }
    let year = value[..4]
        .iter()
        .fold(0i64, |x, &c| x * 10 + i64::from(c - b'0'));
    value = &value[4..];

    // "-", stdZeroMonth "01"
    value = skip(value, b"-").map_err(|v| parse_error(avalue, "-", v))?;
    let hold = value;
    let Some((month, rest)) = getnum(value, true) else {
        return Err(parse_error(avalue, "01", hold));
    };
    if month <= 0 || 12 < month {
        return Err(parse_error_msg(avalue, ": month out of range"));
    }
    value = rest;

    // "-", stdZeroDay "02"
    value = skip(value, b"-").map_err(|v| parse_error(avalue, "-", v))?;
    let hold = value;
    let Some((day, rest)) = getnum(value, true) else {
        return Err(parse_error(avalue, "02", hold));
    };
    value = rest;

    // "T", stdHour "15"
    value = skip(value, b"T").map_err(|v| parse_error(avalue, "T", v))?;
    let hold = value;
    let Some((hour, rest)) = getnum(value, false) else {
        return Err(parse_error(avalue, "15", hold));
    };
    if 24 <= hour {
        return Err(parse_error_msg(avalue, ": hour out of range"));
    }
    value = rest;

    // ":", stdZeroMinute "04"
    value = skip(value, b":").map_err(|v| parse_error(avalue, ":", v))?;
    let hold = value;
    let Some((min, rest)) = getnum(value, true) else {
        return Err(parse_error(avalue, "04", hold));
    };
    if 60 <= min {
        return Err(parse_error_msg(avalue, ": minute out of range"));
    }
    value = rest;

    // ":", stdZeroSecond "05"
    value = skip(value, b":").map_err(|v| parse_error(avalue, ":", v))?;
    let hold = value;
    let Some((sec, rest)) = getnum(value, true) else {
        return Err(parse_error(avalue, "05", hold));
    };
    if 60 <= sec {
        return Err(parse_error_msg(avalue, ": second out of range"));
    }
    value = rest;

    // stdFracSecond9 ".999999999": optional; any number of digits.
    let mut nsec = 0;
    if value.len() >= 2 && (value[0] == b'.' || value[0] == b',') && value[1].is_ascii_digit() {
        let mut i = 0;
        while i + 1 < value.len() && value[i + 1].is_ascii_digit() {
            i += 1;
        }
        nsec = parse_nanoseconds(value, 1 + i);
        value = &value[1 + i..];
    }

    // stdISO8601ColonTZ "Z07:00"
    let hold = value;
    let mut zone_offset: Option<i64> = None;
    if value.first() == Some(&b'Z') {
        value = &value[1..];
    } else {
        if value.len() < 6 || value[3] != b':' {
            return Err(parse_error(avalue, "Z07:00", hold));
        }
        let sign = value[0];
        let hr_s = &value[1..3];
        let mm_s = &value[4..6];
        value = &value[6..];
        let mut ok = true;
        let mut hr = 0;
        let mut mm = 0;
        match getnum(hr_s, true) {
            Some((h, _)) => {
                hr = h;
                match getnum(mm_s, true) {
                    Some((m, _)) => mm = m,
                    None => ok = false,
                }
            }
            None => ok = false,
        }
        // The range test use > rather than >=, as some people do write offsets of 24 hours or
        // 60 minutes; a later check overrides an earlier one.
        let mut range_err = "";
        if hr > 24 {
            range_err = "time zone offset hour";
        }
        if mm > 60 {
            range_err = "time zone offset minute";
        }
        let mut offset = (hr * 60 + mm) * 60;
        match sign {
            b'+' => {}
            b'-' => offset = -offset,
            _ => ok = false,
        }
        if !range_err.is_empty() {
            return Err(parse_error_msg(
                avalue,
                &format!(": {range_err} out of range"),
            ));
        }
        if !ok {
            return Err(parse_error(avalue, "Z07:00", hold));
        }
        zone_offset = Some(offset);
    }

    // End of layout.
    if !value.is_empty() {
        return Err(parse_error_msg(
            avalue,
            &format!(": extra text: {}", tquote(value)),
        ));
    }

    // Validate the day of the month.
    if day < 1 || day > days_in(month, year) {
        return Err(parse_error_msg(avalue, ": day out of range"));
    }
    let t = date_utc(year, month, day, hour, min, sec, nsec);
    Ok(match zone_offset {
        Some(offset) => GoTime {
            unix_secs: add_sec(t.unix_secs, -offset),
            nanos: t.nanos,
        },
        None => t,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE: i64 = 60 * SECOND;
    const HOUR: i64 = 60 * MINUTE;
    const MICROSECOND: i64 = 1_000;

    fn t(unix_secs: i64, nanos: u32) -> GoTime {
        GoTime { unix_secs, nanos }
    }

    #[test]
    fn constants_match_go() {
        assert_eq!(UNIX_TO_INTERNAL, 62_135_596_800);
        assert_eq!(ABSOLUTE_TO_INTERNAL, -9_223_371_966_606_163_200);
        assert_eq!(UNIX_TO_ABSOLUTE, 9_223_372_028_741_760_000);
        assert_eq!(ABSOLUTE_TO_UNIX, -9_223_372_028_741_760_000);
    }

    // time_test.go durationTests, and client-core §3.6
    #[test]
    fn duration_string_table() {
        for (s, d) in [
            ("0s", 0),
            ("1ns", 1),
            ("1.1µs", 1100),
            ("2.2ms", 2200 * MICROSECOND),
            ("3.3s", 3300 * MILLISECOND),
            ("4m5s", 4 * MINUTE + 5 * SECOND),
            ("4m5.001s", 4 * MINUTE + 5001 * MILLISECOND),
            ("5h6m7.001s", 5 * HOUR + 6 * MINUTE + 7001 * MILLISECOND),
            ("8m0.000000001s", 8 * MINUTE + 1),
            ("2562047h47m16.854775807s", i64::MAX),
            ("-2562047h47m16.854775808s", i64::MIN),
        ] {
            assert_eq!(duration_string(d), s);
            if d > 0 {
                assert_eq!(duration_string(-d), format!("-{s}"));
            }
        }
        for (d, s) in [
            (999, "999ns"),
            (1000, "1µs"),
            (1500, "1.5µs"),
            (1_500_000, "1.5ms"),
            (SECOND, "1s"),
            (30 * SECOND, "30s"),
            (2 * MINUTE, "2m0s"),
            (1500 * MILLISECOND, "1.5s"),
            (26 * HOUR + 3 * SECOND, "26h0m3s"),
            (-SECOND, "-1s"),
        ] {
            assert_eq!(duration_string(d), s);
        }
    }

    // time_test.go durationRoundTests, and client-core §3.6 (Round(time.Millisecond))
    #[test]
    fn duration_round_table() {
        for (d, m, want) in [
            (0, SECOND, 0),
            (MINUTE, -11 * SECOND, MINUTE),
            (MINUTE, 0, MINUTE),
            (MINUTE, 1, MINUTE),
            (2 * MINUTE, MINUTE, 2 * MINUTE),
            (2 * MINUTE + 10 * SECOND, MINUTE, 2 * MINUTE),
            (2 * MINUTE + 30 * SECOND, MINUTE, 3 * MINUTE),
            (2 * MINUTE + 50 * SECOND, MINUTE, 3 * MINUTE),
            (-MINUTE, 1, -MINUTE),
            (-2 * MINUTE, MINUTE, -2 * MINUTE),
            (-2 * MINUTE - 10 * SECOND, MINUTE, -2 * MINUTE),
            (-2 * MINUTE - 30 * SECOND, MINUTE, -3 * MINUTE),
            (-2 * MINUTE - 50 * SECOND, MINUTE, -3 * MINUTE),
            (
                8_000_000_000_000_000_000,
                3_000_000_000_000_000_000,
                9_000_000_000_000_000_000,
            ),
            (
                9_000_000_000_000_000_000,
                5_000_000_000_000_000_000,
                i64::MAX,
            ),
            (
                -8_000_000_000_000_000_000,
                3_000_000_000_000_000_000,
                -9_000_000_000_000_000_000,
            ),
            (
                -9_000_000_000_000_000_000,
                5_000_000_000_000_000_000,
                i64::MIN,
            ),
            ((3 << 61) - 1, 3 << 61, 3 << 61),
        ] {
            assert_eq!(duration_round(d, m), want, "{d} round {m}");
        }
        for (d, s) in [
            (0, "0s"),
            (400 * MICROSECOND, "0s"),
            (500 * MICROSECOND, "1ms"),
            (1500 * MICROSECOND, "2ms"),
            (2500 * MICROSECOND, "3ms"),
            (99400 * MICROSECOND, "99ms"),
            (1_234_567 * MICROSECOND, "1.235s"),
            (61 * SECOND, "1m1s"),
            (3661 * SECOND, "1h1m1s"),
            (90 * MINUTE, "1h30m0s"),
        ] {
            assert_eq!(duration_string(duration_round(d, MILLISECOND)), s);
        }
    }

    // time_test.go parseDurationTests
    #[test]
    fn parse_duration_table() {
        for (input, want) in [
            ("0", 0),
            ("5s", 5 * SECOND),
            ("30s", 30 * SECOND),
            ("1478s", 1478 * SECOND),
            ("-5s", -5 * SECOND),
            ("+5s", 5 * SECOND),
            ("-0", 0),
            ("+0", 0),
            ("5.0s", 5 * SECOND),
            ("5.6s", 5 * SECOND + 600 * MILLISECOND),
            ("5.s", 5 * SECOND),
            (".5s", 500 * MILLISECOND),
            ("1.0s", SECOND),
            ("1.00s", SECOND),
            ("1.004s", SECOND + 4 * MILLISECOND),
            ("1.0040s", SECOND + 4 * MILLISECOND),
            ("100.00100s", 100 * SECOND + MILLISECOND),
            ("10ns", 10),
            ("11us", 11 * MICROSECOND),
            ("12µs", 12 * MICROSECOND),
            ("12μs", 12 * MICROSECOND),
            ("13ms", 13 * MILLISECOND),
            ("14s", 14 * SECOND),
            ("15m", 15 * MINUTE),
            ("16h", 16 * HOUR),
            ("3h30m", 3 * HOUR + 30 * MINUTE),
            ("10.5s4m", 4 * MINUTE + 10 * SECOND + 500 * MILLISECOND),
            ("-2m3.4s", -(2 * MINUTE + 3 * SECOND + 400 * MILLISECOND)),
            (
                "1h2m3s4ms5us6ns",
                HOUR + 2 * MINUTE + 3 * SECOND + 4 * MILLISECOND + 5 * MICROSECOND + 6,
            ),
            (
                "39h9m14.425s",
                39 * HOUR + 9 * MINUTE + 14 * SECOND + 425 * MILLISECOND,
            ),
            ("52763797000ns", 52_763_797_000),
            ("0.3333333333333333333h", 20 * MINUTE),
            ("9007199254740993ns", (1 << 53) + 1),
            ("9223372036854775807ns", i64::MAX),
            ("9223372036854775.807us", i64::MAX),
            ("9223372036s854ms775us807ns", i64::MAX),
            ("-9223372036854775808ns", i64::MIN),
            ("-9223372036854775.808us", i64::MIN),
            ("-9223372036s854ms775us808ns", i64::MIN),
            ("-2562047h47m16.854775808s", i64::MIN),
            ("0.100000000000000000000h", 6 * MINUTE),
            (
                "0.830103483285477580700h",
                49 * MINUTE + 48 * SECOND + 372_539_827,
            ),
        ] {
            assert_eq!(parse_duration(input), Ok(want), "{input:?}");
        }
    }

    // time_test.go parseDurationErrorTests (the valid UTF-8 inputs), with the full texts
    #[test]
    fn parse_duration_errors() {
        for (input, expect) in [
            ("", r#""""#),
            ("3", r#""3""#),
            ("-", r#""-""#),
            ("s", r#""s""#),
            (".", r#"".""#),
            ("-.", r#""-.""#),
            (".s", r#"".s""#),
            ("+.s", r#""+.s""#),
            ("1d", r#""1d""#),
            ("\u{FFFD}", r#""\xef\xbf\xbd""#),
            (
                "\u{FFFD} hello \u{FFFD} world",
                r#""\xef\xbf\xbd hello \xef\xbf\xbd world""#,
            ),
            ("9223372036854775810ns", r#""9223372036854775810ns""#),
            ("9223372036854775808ns", r#""9223372036854775808ns""#),
            ("-9223372036854775809ns", r#""-9223372036854775809ns""#),
            ("9223372036854776us", r#""9223372036854776us""#),
            ("3000000h", r#""3000000h""#),
            ("9223372036854775.808us", r#""9223372036854775.808us""#),
            (
                "9223372036854ms775us808ns",
                r#""9223372036854ms775us808ns""#,
            ),
        ] {
            match parse_duration(input) {
                Ok(d) => panic!("{input:?} parsed as {d}"),
                Err(e) => assert!(e.contains(expect), "{input:?}: {e}"),
            }
        }
        assert_eq!(
            parse_duration(""),
            Err(r#"time: invalid duration """#.to_owned())
        );
        assert_eq!(
            parse_duration("3"),
            Err(r#"time: missing unit in duration "3""#.to_owned())
        );
        assert_eq!(
            parse_duration("1d"),
            Err(r#"time: unknown unit "d" in duration "1d""#.to_owned())
        );
        assert_eq!(
            parse_duration("1秒\"\\"),
            Err(
                r#"time: unknown unit "\xe7\xa7\x92\"\\" in duration "1\xe7\xa7\x92\"\\""#
                    .to_owned()
            )
        );
    }

    // time_test.go TestParseDurationRoundTrip
    #[test]
    fn parse_duration_round_trip() {
        for d in [i64::MAX, i64::MIN] {
            assert_eq!(parse_duration(&duration_string(d)), Ok(d));
        }
        let mut state: u64 = 7;
        for _ in 0..100 {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            let d0 = i64::from((state >> 33) as u32 & 0x7fff_ffff) * MILLISECOND;
            assert_eq!(parse_duration(&duration_string(d0)), Ok(d0));
        }
    }

    #[test]
    fn duration_to_ns_saturates() {
        assert_eq!(
            duration_to_ns(std::time::Duration::from_millis(1500)),
            1_500_000_000
        );
        assert_eq!(duration_to_ns(std::time::Duration::MAX), i64::MAX);
    }

    #[test]
    fn go_time_helpers() {
        assert_eq!(GoTime::from_unix_nano(-1), t(-1, 999_999_999));
        assert_eq!(GoTime::from_unix_nano(1_500_000_000), t(1, 500_000_000));
        assert_eq!(GoTime::from_unix_nano(i64::MIN).unix_nano(), i64::MIN);
        assert_eq!(GoTime::from_unix_nano(i64::MAX).unix_nano(), i64::MAX);
        assert_eq!(t(0, 999_999_999).add_ns(1), t(1, 0));
        assert_eq!(t(0, 0).add_ns(-1), t(-1, 999_999_999));
        assert_eq!(t(10, 5).add_ns(-3 * SECOND - 6), t(6, 999_999_999));
        // Internal seconds saturate at MaxInt64, as Time.addSec does.
        let max = i64::MAX - UNIX_TO_INTERNAL;
        assert_eq!(t(max, 0).add_ns(2 * SECOND), t(max, 0));
        let now = GoTime::now();
        assert!(now.nanos < 1_000_000_000);
        assert!(now.unix_secs > 1_700_000_000);
    }

    // format_test.go rfc3339Formats
    #[test]
    fn rfc3339_formats() {
        let utc = date_utc(2008, 9, 17, 20, 4, 26, 0);
        assert_eq!(utc.unix_secs, 1_221_681_866);
        assert_eq!(format_rfc3339(utc, &FixedZone(0)), "2008-09-17T20:04:26Z");
        let est = date_utc(1994, 9, 17, 20, 4, 26, 0).add_ns(18000 * SECOND);
        assert_eq!(
            format_rfc3339(est, &FixedZone(-18000)),
            "1994-09-17T20:04:26-05:00"
        );
        let oto = date_utc(2000, 12, 26, 1, 15, 6, 0).add_ns(-15600 * SECOND);
        assert_eq!(
            format_rfc3339(oto, &FixedZone(15600)),
            "2000-12-26T01:15:06+04:20"
        );
        // Offsets below a minute print +00:00 (Go divides toward zero).
        assert_eq!(
            format_rfc3339(t(0, 0), &FixedZone(1)),
            "1970-01-01T00:00:01+00:00"
        );
        assert_eq!(
            format_rfc3339(t(0, 0), &FixedZone(-61)),
            "1969-12-31T23:58:59-00:01"
        );
    }

    // format_test.go TestAppendInt
    #[test]
    fn append_int_table() {
        for (x, width, want) in [
            (0, 0, "0"),
            (0, 1, "0"),
            (0, 2, "00"),
            (0, 3, "000"),
            (1, 0, "1"),
            (1, 1, "1"),
            (1, 2, "01"),
            (1, 3, "001"),
            (-1, 0, "-1"),
            (-1, 1, "-1"),
            (-1, 2, "-01"),
            (-1, 3, "-001"),
            (99, 2, "99"),
            (100, 2, "100"),
            (1, 4, "0001"),
            (12, 4, "0012"),
            (123, 4, "0123"),
            (1234, 4, "1234"),
            (12345, 4, "12345"),
            (1, 5, "00001"),
            (12, 5, "00012"),
            (123, 5, "00123"),
            (1234, 5, "01234"),
            (12345, 5, "12345"),
            (123456, 5, "123456"),
            (0, 9, "000000000"),
            (123, 9, "000000123"),
            (123456, 9, "000123456"),
            (123456789, 9, "123456789"),
        ] {
            let mut b = Vec::new();
            append_int(&mut b, x, width);
            assert_eq!(String::from_utf8_lossy(&b), want, "{x} width {width}");
        }
    }

    // client-core §3.5 and cli §2.3 verified lines
    #[test]
    fn slog_log_and_clock_examples() {
        let ts = date_utc(2026, 9, 18, 10, 11, 12, 345_678_901);
        assert_eq!(
            format_slog_time(ts, &FixedZone(0)),
            "2026-09-18T10:11:12.345Z"
        );
        assert_eq!(
            format_slog_time(ts, &FixedZone(7200)),
            "2026-09-18T12:11:12.345+02:00"
        );
        assert_eq!(
            format_slog_time(ts, &FixedZone(-19800)),
            "2026-09-18T04:41:12.345-05:30"
        );
        assert_eq!(
            format_slog_time(ts.add_ns(999 * MICROSECOND), &FixedZone(0)),
            "2026-09-18T10:11:12.346Z"
        );
        let local = date_utc(2026, 9, 18, 12, 34, 56, 789_654_321).add_ns(-7200 * SECOND);
        assert_eq!(
            format_slog_time(local, &FixedZone(7200)),
            "2026-09-18T12:34:56.789+02:00"
        );
        assert_eq!(format_log_std(ts, &FixedZone(7200)), "2026/09/18 12:11:12");
        assert_eq!(format_clock(ts, &FixedZone(7200)), "12:11:12");
        assert_eq!(format_rfc3339nano_utc(ts), "2026-09-18T10:11:12.345678901Z");
        assert_eq!(
            format_rfc3339nano_utc(t(0, 120_000_000)),
            "1970-01-01T00:00:00.12Z"
        );
        assert_eq!(format_rfc3339nano_utc(t(0, 0)), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn log_itoa_matches_go_byte_arithmetic() {
        let mut b = Vec::new();
        log_itoa(&mut b, 2026, 4);
        log_itoa(&mut b, 9, 2);
        log_itoa(&mut b, 12345, 4);
        log_itoa(&mut b, -1, 4);
        assert_eq!(b, b"20260912345000/");
        // Go writes b"\x90'*+/10/12 15:35:41" for year -93088965 (go1.26.5, verified); the String
        // result replaces the invalid first byte.
        let t = t(-2_937_666_142_895_494, 0);
        assert_eq!(
            format_log_std(t, &FixedZone(36435)),
            String::from_utf8_lossy(b"\x90'*+/10/12 15:35:41")
        );
        assert_eq!(
            format_rfc3339(t, &FixedZone(36435)),
            "-93088965-10-12T15:35:41+10:07"
        );
    }

    #[test]
    fn parse_rfc3339nano_accepts() {
        assert_eq!(
            parse_rfc3339nano("2026-09-18T10:11:12.345678901Z"),
            Ok(date_utc(2026, 9, 18, 10, 11, 12, 345_678_901))
        );
        assert_eq!(
            parse_rfc3339nano("1970-01-01T00:00:00.000000001+00:01"),
            Ok(t(-60, 1))
        );
        assert_eq!(
            parse_rfc3339nano("2026-09-18T10:11:12,5Z"),
            Ok(date_utc(2026, 9, 18, 10, 11, 12, 500_000_000))
        );
        assert_eq!(
            parse_rfc3339nano("2026-09-18T1:02:03Z"),
            Ok(date_utc(2026, 9, 18, 1, 2, 3, 0))
        );
        assert_eq!(
            parse_rfc3339nano("2026-09-18T10:11:12.1234567891-05:30"),
            Ok(date_utc(2026, 9, 18, 15, 41, 12, 123_456_789))
        );
        assert_eq!(
            parse_rfc3339nano("0000-01-01T00:00:00Z"),
            Ok(t(-62_167_219_200, 0))
        );
    }

    // format_test.go parseErrorTests for the RFC3339 layouts, with the RFC3339Nano layout text
    #[test]
    fn parse_rfc3339nano_errors() {
        let as_layout = r#" as "2006-01-02T15:04:05.999999999Z07:00": "#;
        for (input, want) in [
            (
                "",
                format!(r#"parsing time ""{as_layout}cannot parse "" as "2006""#),
            ),
            (
                "2006-01-02T15:04:05Z07:00",
                r#"parsing time "2006-01-02T15:04:05Z07:00": extra text: "07:00""#.to_owned(),
            ),
            (
                "2006-01-02T15:04_abc",
                format!(
                    r#"parsing time "2006-01-02T15:04_abc"{as_layout}cannot parse "_abc" as ":""#
                ),
            ),
            (
                "2006-01-02T15:04:05_abc",
                format!(
                    r#"parsing time "2006-01-02T15:04:05_abc"{as_layout}cannot parse "_abc" as "Z07:00""#
                ),
            ),
            (
                "2006-01-02T15:04:05Z_abc",
                r#"parsing time "2006-01-02T15:04:05Z_abc": extra text: "_abc""#.to_owned(),
            ),
            (
                "2010-02-04T21:00:67.012345678-08:00",
                r#"parsing time "2010-02-04T21:00:67.012345678-08:00": second out of range"#
                    .to_owned(),
            ),
            (
                "0000-01-01T00:00:.0+00:00",
                format!(
                    r#"parsing time "0000-01-01T00:00:.0+00:00"{as_layout}cannot parse ".0+00:00" as "05""#
                ),
            ),
            (
                "\"",
                format!(r#"parsing time "\""{as_layout}cannot parse "\"" as "2006""#),
            ),
            (
                "0000-01-01T00:00:00+00:+0",
                format!(
                    r#"parsing time "0000-01-01T00:00:00+00:+0"{as_layout}cannot parse "+00:+0" as "Z07:00""#
                ),
            ),
            (
                "0000-01-01T00:00:00+-0:00",
                format!(
                    r#"parsing time "0000-01-01T00:00:00+-0:00"{as_layout}cannot parse "+-0:00" as "Z07:00""#
                ),
            ),
            (
                "2026-09-18T10:11:12+25:00",
                r#"parsing time "2026-09-18T10:11:12+25:00": time zone offset hour out of range"#
                    .to_owned(),
            ),
            (
                "2026-09-18T10:11:12-23:61",
                r#"parsing time "2026-09-18T10:11:12-23:61": time zone offset minute out of range"#
                    .to_owned(),
            ),
            (
                "2023-02-29T00:00:00Z",
                r#"parsing time "2023-02-29T00:00:00Z": day out of range"#.to_owned(),
            ),
            (
                "２026-09-18T10:11:12Z",
                format!(
                    r#"parsing time "\xef\xbc\x92026-09-18T10:11:12Z"{as_layout}cannot parse "\xef\xbc\x92026-09-18T10:11:12Z" as "2006""#
                ),
            ),
        ] {
            assert_eq!(parse_rfc3339nano(input), Err(want), "{input:?}");
        }
    }

    #[test]
    fn go_local_is_utc_rules() {
        let none = |_: &std::path::Path| false;
        let all = |_: &std::path::Path| true;
        assert!(!go_local_is_utc(None, none));
        assert!(go_local_is_utc(Some(b""), all));
        assert!(go_local_is_utc(Some(b":"), all));
        assert!(go_local_is_utc(Some(b"UTC"), all));
        assert!(go_local_is_utc(Some(b":UTC"), all));
        assert!(go_local_is_utc(Some(b"XYZ-3"), none));
        assert!(go_local_is_utc(Some(b"/nonexistent"), none));
        assert!(!go_local_is_utc(Some(b"/etc/localtime"), all));
        let kolkata =
            |p: &std::path::Path| p == std::path::Path::new("/usr/share/zoneinfo//Asia/Kolkata");
        assert!(!go_local_is_utc(Some(b"Asia/Kolkata"), kolkata));
        assert!(!go_local_is_utc(Some(b":Asia/Kolkata"), kolkata));
        assert!(go_local_is_utc(Some(b"Asia/Nowhere"), kolkata));
    }

    #[test]
    fn zones() {
        assert_eq!(FixedZone(-3600).offset_at(123), -3600);
        let off = SystemZone.offset_at(GoTime::now().unix_secs);
        assert!((-26 * 3600..=26 * 3600).contains(&off), "{off}");
        assert!(!is_tzif_file(std::path::Path::new(
            "/nonexistent/zoneinfo/file"
        )));
    }
}
