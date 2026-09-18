//! A port of Go `flag.(*FlagSet)` (go1.26.5 `src/flag/flag.go`): `Var`, `Set`, `Parse` with its
//! `parseOne` loop, `Visit`, `Args`, and the `flag.Value` types urfave/cli v2.27.7 registers through it
//! (`stringValue`, `intValue`, `int64Value`, `uintValue`, `float64Value`, `durationValue`, urfave's
//! `boolValue` and `StringSlice`).
//!
//! Arguments and values are Go strings, so bytes. Error texts are bytes too, because they echo the
//! arguments (cli.md §2.2.4).

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::ffi::OsString;
use std::fmt;
use std::os::unix::ffi::{OsStrExt, OsStringExt};

use dstore_gocompat::quote::quote;
use dstore_gocompat::strconv::{self, NumErrorKind};
use dstore_gocompat::strings::trim_space;
use dstore_gocompat::time::{self as gotime, duration_string};

/// `flag.errParse`: the `Set` error of a value that does not parse.
pub const ERR_PARSE: &str = "parse error";
/// `flag.errRange`: the `Set` error of a value out of range.
pub const ERR_RANGE: &str = "value out of range";
/// `flag.ErrHelp`: `-help` or `-h` given while no flag of that name is defined.
pub const ERR_HELP: &str = "flag: help requested";

/// A `Parse` or `Set` failure: Go's error text, as bytes because it echoes arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlagError(pub Vec<u8>);

impl fmt::Display for FlagError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&String::from_utf8_lossy(&self.0))
    }
}

impl std::error::Error for FlagError {}

/// The dynamic value of a flag: the `flag.Value` implementations urfave uses.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// `stringValue`.
    String(Vec<u8>),
    /// urfave `boolValue` (`IsBoolFlag`).
    Bool(bool),
    /// `intValue` (`strconv.IntSize` is 64 on every supported target).
    Int(i64),
    /// `int64Value`.
    Int64(i64),
    /// `uintValue`.
    Uint(u64),
    /// `float64Value`.
    Float64(f64),
    /// `durationValue`, in nanoseconds.
    Duration(i64),
    /// urfave `StringSlice`: the items and `hasBeenSet` (the first `Set` replaces what is there).
    StringSlice {
        items: Vec<Vec<u8>>,
        has_been_set: bool,
    },
}

impl Value {
    /// `IsBoolFlag()`: the flag takes no separate argument.
    pub fn is_bool_flag(&self) -> bool {
        matches!(self, Value::Bool(_))
    }

    /// `Value.String()`.
    pub fn string(&self) -> Vec<u8> {
        match self {
            Value::String(s) => s.clone(),
            Value::Bool(b) => {
                if *b {
                    b"true".to_vec()
                } else {
                    b"false".to_vec()
                }
            }
            Value::Int(v) | Value::Int64(v) => v.to_string().into_bytes(),
            Value::Uint(v) => v.to_string().into_bytes(),
            Value::Float64(v) => strconv::format_float_g(*v).into_bytes(),
            Value::Duration(ns) => duration_string(*ns).into_bytes(),
            // fmt.Sprintf("%s", s.slice)
            Value::StringSlice { items, .. } => {
                let mut out = vec![b'['];
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(b' ');
                    }
                    out.extend_from_slice(item);
                }
                out.push(b']');
                out
            }
        }
    }

    /// `Value.Set(s)`. The error is the text Go's `Set` returns. As in Go, the numeric values take the
    /// parser's result even when it fails (0, or the range limit).
    pub fn set(&mut self, s: &[u8]) -> Result<(), &'static str> {
        match self {
            Value::String(v) => {
                *v = s.to_vec();
                Ok(())
            }
            Value::Bool(v) => match strconv::parse_bool(&numeric_probe(s)) {
                Ok(b) => {
                    *v = b;
                    Ok(())
                }
                Err(_) => Err(ERR_PARSE),
            },
            Value::Int(v) | Value::Int64(v) => match parse_int(s) {
                Ok(n) => {
                    *v = n;
                    Ok(())
                }
                Err((n, kind)) => {
                    *v = n;
                    Err(num_flag_error(kind))
                }
            },
            Value::Uint(v) => match parse_uint(s) {
                Ok(n) => {
                    *v = n;
                    Ok(())
                }
                Err((n, kind)) => {
                    *v = n;
                    Err(num_flag_error(kind))
                }
            },
            Value::Float64(v) => match parse_float(s) {
                Ok(f) => {
                    *v = f;
                    Ok(())
                }
                Err((f, kind)) => {
                    *v = f;
                    Err(num_flag_error(kind))
                }
            },
            Value::Duration(v) => match parse_duration(s) {
                Ok(ns) => {
                    *v = ns;
                    Ok(())
                }
                Err(_) => {
                    *v = 0;
                    Err(ERR_PARSE)
                }
            },
            // urfave StringSlice.Set with the default separator ",".
            Value::StringSlice {
                items,
                has_been_set,
            } => {
                if !*has_been_set {
                    items.clear();
                    *has_been_set = true;
                }
                for part in s.split(|&b| b == b',') {
                    items.push(trim_space(part).to_vec());
                }
                Ok(())
            }
        }
    }
}

/// `flag.numError`: the `Set` error for a `strconv.NumError` class.
fn num_flag_error(kind: NumErrorKind) -> &'static str {
    match kind {
        NumErrorKind::Syntax => ERR_PARSE,
        NumErrorKind::Range => ERR_RANGE,
    }
}

/// The input of Go's bool and numeric parsers as ASCII: every byte ≥ 0x80 becomes `!`. Those parsers
/// reject a non-ASCII byte exactly where they reject `!`, so the outcome (value, class, position of
/// the failure) is the same, while the error text must echo the original bytes.
pub(crate) fn numeric_probe(s: &[u8]) -> String {
    s.iter()
        .map(|&b| if b.is_ascii() { char::from(b) } else { '!' })
        .collect()
}

/// The text of a `*strconv.NumError` over the original bytes of the input.
pub(crate) fn num_error_text(func: &str, s: &[u8], kind: NumErrorKind) -> String {
    format!("strconv.{func}: parsing {}: {}", quote(s), kind.text())
}

/// `strconv.ParseInt(s, 0, 64)`: the value Go returns alongside a failure (0, or the range limit on
/// the side of the sign) and the class.
pub(crate) fn parse_int(s: &[u8]) -> Result<i64, (i64, NumErrorKind)> {
    strconv::parse_int(&numeric_probe(s), 0, 64).map_err(|e| {
        let v = match e.kind {
            NumErrorKind::Syntax => 0,
            NumErrorKind::Range if s.first() == Some(&b'-') => i64::MIN,
            NumErrorKind::Range => i64::MAX,
        };
        (v, e.kind)
    })
}

/// `strconv.ParseUint(s, 0, 64)`, with the value Go returns alongside a failure.
pub(crate) fn parse_uint(s: &[u8]) -> Result<u64, (u64, NumErrorKind)> {
    strconv::parse_uint(&numeric_probe(s), 0, 64).map_err(|e| {
        let v = match e.kind {
            NumErrorKind::Syntax => 0,
            NumErrorKind::Range => u64::MAX,
        };
        (v, e.kind)
    })
}

/// `strconv.ParseFloat(s, 64)`, with the value Go returns alongside a failure (0, or ±Inf).
pub(crate) fn parse_float(s: &[u8]) -> Result<f64, (f64, NumErrorKind)> {
    strconv::parse_float(&numeric_probe(s)).map_err(|e| {
        let v = match e.kind {
            NumErrorKind::Syntax => 0.0,
            NumErrorKind::Range if s.first() == Some(&b'-') => f64::NEG_INFINITY,
            NumErrorKind::Range => f64::INFINITY,
        };
        (v, e.kind)
    })
}

/// go1.26.5 `time.ParseDuration` over the bytes of a Go string: nanoseconds, or Go's error text. Valid
/// UTF-8 goes to `gocompat::time::parse_duration`. Other input never parses (its invalid bytes can only
/// end up in a unit), and its error text is built here by the same steps over bytes.
pub(crate) fn parse_duration(orig: &[u8]) -> Result<i64, String> {
    if let Ok(s) = std::str::from_utf8(orig) {
        return gotime::parse_duration(s);
    }
    const LIMIT: u64 = 1 << 63;
    let invalid = || format!("time: invalid duration {}", time_quote(orig));
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
    while let Some(&first) = s.first() {
        // The next character must be [0-9.]
        if !(first == b'.' || first.is_ascii_digit()) {
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
        let (mut f, mut scale) = (0u64, 1f64);
        if s.first() == Some(&b'.') {
            s = &s[1..];
            let pl = s.len();
            let (x, sc, rest) = leading_fraction(s);
            (f, scale, s) = (x, sc, rest);
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
            return Err(format!(
                "time: missing unit in duration {}",
                time_quote(orig)
            ));
        }
        let (u, rest) = s.split_at(i);
        s = rest;
        let unit: u64 = match u {
            b"ns" => 1,
            b"us" | b"\xc2\xb5s" | b"\xce\xbcs" => 1_000,
            b"ms" => 1_000_000,
            b"s" => 1_000_000_000,
            b"m" => 60_000_000_000,
            b"h" => 3_600_000_000_000,
            _ => {
                return Err(format!(
                    "time: unknown unit {} in duration {}",
                    time_quote(u),
                    time_quote(orig)
                ));
            }
        };
        if v > LIMIT / unit {
            // overflow
            return Err(invalid());
        }
        v *= unit;
        if f > 0 {
            // float64 is needed to be nanosecond accurate for fractions of hours.
            v = v.wrapping_add((f as f64 * (unit as f64 / scale)) as u64);
            if v > LIMIT {
                return Err(invalid());
            }
        }
        d = d.wrapping_add(v);
        if d > LIMIT {
            return Err(invalid());
        }
    }
    if neg {
        return Ok((d as i64).wrapping_neg());
    }
    if d > LIMIT - 1 {
        return Err(invalid());
    }
    Ok(d as i64)
}

/// `time.leadingInt`: `None` on overflow.
fn leading_int(s: &[u8]) -> Option<(u64, &[u8])> {
    let mut x: u64 = 0;
    let mut i = 0;
    while let Some(&c) = s.get(i) {
        if !c.is_ascii_digit() {
            break;
        }
        if x > (1u64 << 63) / 10 {
            return None;
        }
        x = x * 10 + u64::from(c - b'0');
        if x > 1 << 63 {
            return None;
        }
        i += 1;
    }
    Some((x, &s[i..]))
}

/// `time.leadingFraction`: stops accumulating precision on overflow.
fn leading_fraction(s: &[u8]) -> (u64, f64, &[u8]) {
    let mut x: u64 = 0;
    let mut scale = 1f64;
    let mut overflow = false;
    let mut i = 0;
    while let Some(&c) = s.get(i) {
        if !c.is_ascii_digit() {
            break;
        }
        i += 1;
        if overflow {
            continue;
        }
        if x > ((1u64 << 63) - 1) / 10 {
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

/// `time.quote`: `\x` escapes for non-ASCII and control runes (all bytes of a valid rune; one byte of an
/// invalid one, three of a literal U+FFFD).
fn time_quote(s: &[u8]) -> String {
    const LOWERHEX: &[u8; 16] = b"0123456789abcdef";
    let mut buf = String::from("\"");
    let mut i = 0;
    while i < s.len() {
        let (c, size) = decode_rune(&s[i..]);
        if u32::from(c) >= 0x80 || u32::from(c) < 0x20 {
            let width = if c == char::REPLACEMENT_CHARACTER {
                if s.get(i..i + 3) == Some("\u{FFFD}".as_bytes()) {
                    3
                } else {
                    1
                }
            } else {
                c.len_utf8()
            };
            for &b in s.get(i..i + width).unwrap_or_default() {
                buf.push_str("\\x");
                buf.push(char::from(LOWERHEX[usize::from(b >> 4)]));
                buf.push(char::from(LOWERHEX[usize::from(b & 0xf)]));
            }
        } else {
            if c == '"' || c == '\\' {
                buf.push('\\');
            }
            buf.push(c);
        }
        i += size;
    }
    buf.push('"');
    buf
}

/// `utf8.DecodeRune`: `(U+FFFD, 1)` for an invalid encoding.
fn decode_rune(s: &[u8]) -> (char, usize) {
    match s.utf8_chunks().next() {
        Some(chunk) => match chunk.valid().chars().next() {
            Some(c) => (c, c.len_utf8()),
            None => (char::REPLACEMENT_CHARACTER, 1),
        },
        None => (char::REPLACEMENT_CHARACTER, 1),
    }
}

/// `flag.FlagSet` with `ContinueOnError` and discarded output, as urfave creates it.
#[derive(Clone, Debug, Default)]
pub struct FlagSet {
    name: String,
    values: Vec<Value>,
    formal: HashMap<Vec<u8>, usize>,
    actual: BTreeSet<Vec<u8>>,
    args: Vec<OsString>,
    parsed: bool,
}

impl FlagSet {
    /// `flag.NewFlagSet(name, flag.ContinueOnError)`.
    pub fn new(name: &str) -> FlagSet {
        FlagSet {
            name: name.to_owned(),
            ..FlagSet::default()
        }
    }

    /// `Name()`.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Adds a destination; `define` binds names to it. urfave's bool and slice flags bind every name of
    /// a flag to one destination.
    pub fn add_value(&mut self, value: Value) -> usize {
        self.values.push(value);
        self.values.len() - 1
    }

    /// `Var(value, name, usage)` over an existing destination. Where Go panics, the panic message is
    /// returned.
    pub fn define(&mut self, name: &str, slot: usize) -> Result<(), String> {
        if name.starts_with('-') {
            return Err(format!("flag {} begins with -", quote(name.as_bytes())));
        }
        if name.contains('=') {
            return Err(format!("flag {} contains =", quote(name.as_bytes())));
        }
        if self.formal.contains_key(name.as_bytes()) {
            return Err(if self.name.is_empty() {
                format!("flag redefined: {name}")
            } else {
                format!("{} flag redefined: {name}", self.name)
            });
        }
        if slot >= self.values.len() {
            return Err(format!("flag {name}: no such destination"));
        }
        self.formal.insert(name.as_bytes().to_vec(), slot);
        Ok(())
    }

    /// `Var(value, name, usage)` with a new destination.
    pub fn var(&mut self, name: &str, value: Value) -> Result<usize, String> {
        let slot = self.add_value(value);
        self.define(name, slot)?;
        Ok(slot)
    }

    /// `Lookup(name).Value`.
    pub fn lookup(&self, name: &[u8]) -> Option<&Value> {
        self.formal
            .get(name)
            .and_then(|&slot| self.values.get(slot))
    }

    /// The destination `name` is bound to.
    pub fn slot(&self, name: &[u8]) -> Option<usize> {
        self.formal.get(name).copied()
    }

    /// Whether `name` was set (`Visit` reports it).
    pub fn is_set(&self, name: &[u8]) -> bool {
        self.actual.contains(name)
    }

    /// `Visit`: the names that were set, in lexicographical order.
    pub fn visit(&self) -> impl Iterator<Item = &[u8]> {
        self.actual.iter().map(Vec::as_slice)
    }

    /// `NFlag()`.
    pub fn nflag(&self) -> usize {
        self.actual.len()
    }

    /// `Parsed()`.
    pub fn parsed(&self) -> bool {
        self.parsed
    }

    /// `Args()`: the arguments left after parsing.
    pub fn args(&self) -> &[OsString] {
        &self.args
    }

    /// `Set(name, value)`.
    pub fn set(&mut self, name: &[u8], value: &[u8]) -> Result<(), FlagError> {
        let Some(v) = self
            .formal
            .get(name)
            .and_then(|&slot| self.values.get_mut(slot))
        else {
            return Err(FlagError(concat(&[b"no such flag -", name])));
        };
        v.set(value).map_err(|e| FlagError(e.as_bytes().to_vec()))?;
        self.actual.insert(name.to_vec());
        Ok(())
    }

    /// Records `name` as set without changing its value (urfave `normalizeFlags` copying a value to
    /// the other names of a flag that share its destination).
    pub(crate) fn mark_set(&mut self, name: &[u8]) {
        if self.formal.contains_key(name) {
            self.actual.insert(name.to_vec());
        }
    }

    /// `Parse(arguments)`: `parseOne` until the first non-flag argument, `--`, or an error.
    pub fn parse(&mut self, arguments: &[OsString]) -> Result<(), FlagError> {
        self.parsed = true;
        let mut args: VecDeque<OsString> = arguments.iter().cloned().collect();
        let result = loop {
            match self.parse_one(&mut args) {
                Ok(true) => continue,
                Ok(false) => break Ok(()),
                Err(e) => break Err(e),
            }
        };
        self.args = args.into();
        result
    }

    /// `parseOne`: whether a flag was seen.
    fn parse_one(&mut self, args: &mut VecDeque<OsString>) -> Result<bool, FlagError> {
        let s: Vec<u8> = match args.front() {
            None => return Ok(false),
            Some(a) => a.as_bytes().to_vec(),
        };
        if s.len() < 2 || s[0] != b'-' {
            return Ok(false);
        }
        let mut num_minuses = 1;
        if s[1] == b'-' {
            num_minuses += 1;
            if s.len() == 2 {
                // "--" terminates the flags
                args.pop_front();
                return Ok(false);
            }
        }
        let mut name: &[u8] = &s[num_minuses..];
        if name.is_empty() || name[0] == b'-' || name[0] == b'=' {
            return Err(FlagError(concat(&[b"bad flag syntax: ", &s])));
        }

        // it's a flag. does it have an argument?
        args.pop_front();
        let mut has_value = false;
        let mut value: Vec<u8> = Vec::new();
        if let Some(i) = name.iter().skip(1).position(|&b| b == b'=') {
            // equals cannot be first
            let i = i + 1;
            value = name[i + 1..].to_vec();
            has_value = true;
            name = &name[..i];
        }

        let Some(v) = self
            .formal
            .get(name)
            .and_then(|&slot| self.values.get_mut(slot))
        else {
            if name == b"help" || name == b"h" {
                // special case for nice help message.
                return Err(FlagError(ERR_HELP.as_bytes().to_vec()));
            }
            return Err(FlagError(concat(&[
                b"flag provided but not defined: -",
                name,
            ])));
        };

        if v.is_bool_flag() {
            // special case: doesn't need an arg
            if has_value {
                if let Err(e) = v.set(&value) {
                    return Err(FlagError(concat(&[
                        b"invalid boolean value ",
                        quote(&value).as_bytes(),
                        b" for -",
                        name,
                        b": ",
                        e.as_bytes(),
                    ])));
                }
            } else if let Err(e) = v.set(b"true") {
                return Err(FlagError(concat(&[
                    b"invalid boolean flag ",
                    name,
                    b": ",
                    e.as_bytes(),
                ])));
            }
        } else {
            // It must have a value, which might be the next argument.
            if !has_value && let Some(next) = args.pop_front() {
                has_value = true;
                value = next.into_vec();
            }
            if !has_value {
                return Err(FlagError(concat(&[b"flag needs an argument: -", name])));
            }
            if let Err(e) = v.set(&value) {
                return Err(FlagError(concat(&[
                    b"invalid value ",
                    quote(&value).as_bytes(),
                    b" for flag -",
                    name,
                    b": ",
                    e.as_bytes(),
                ])));
            }
        }
        self.actual.insert(name.to_vec());
        Ok(true)
    }
}

fn concat(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    fn err(set: &mut FlagSet, args: &[&str]) -> String {
        match set.parse(&os(args)) {
            Ok(()) => String::from("<ok>"),
            Err(e) => e.to_string(),
        }
    }

    /// go1.26.5 flag_test.go `testParse` (uint64 has no urfave kind here; `-uint64` is a `Uint`).
    #[test]
    fn go_test_parse() {
        let mut f = FlagSet::new("test");
        assert!(!f.parsed());
        let b = f.var("bool", Value::Bool(false)).unwrap_or(usize::MAX);
        let b2 = f.var("bool2", Value::Bool(false)).unwrap_or(usize::MAX);
        let i = f.var("int", Value::Int(0)).unwrap_or(usize::MAX);
        let i64s = f.var("int64", Value::Int64(0)).unwrap_or(usize::MAX);
        let u = f.var("uint", Value::Uint(0)).unwrap_or(usize::MAX);
        let u64s = f.var("uint64", Value::Uint(0)).unwrap_or(usize::MAX);
        let s = f
            .var("string", Value::String(b"0".to_vec()))
            .unwrap_or(usize::MAX);
        let fl = f.var("float64", Value::Float64(0.0)).unwrap_or(usize::MAX);
        let d = f
            .var("duration", Value::Duration(5_000_000_000))
            .unwrap_or(usize::MAX);
        let args = os(&[
            "-bool",
            "-bool2=true",
            "--int",
            "22",
            "--int64",
            "0x23",
            "-uint",
            "24",
            "--uint64",
            "25",
            "-string",
            "hello",
            "-float64",
            "2718e28",
            "-duration",
            "2m",
            "one-extra-argument",
        ]);
        assert_eq!(f.parse(&args), Ok(()));
        assert!(f.parsed());
        assert_eq!(f.values.get(b), Some(&Value::Bool(true)));
        assert_eq!(f.values.get(b2), Some(&Value::Bool(true)));
        assert_eq!(f.values.get(i), Some(&Value::Int(22)));
        assert_eq!(f.values.get(i64s), Some(&Value::Int64(0x23)));
        assert_eq!(f.values.get(u), Some(&Value::Uint(24)));
        assert_eq!(f.values.get(u64s), Some(&Value::Uint(25)));
        assert_eq!(f.values.get(s), Some(&Value::String(b"hello".to_vec())));
        assert_eq!(f.values.get(fl), Some(&Value::Float64(2718e28)));
        assert_eq!(f.values.get(d), Some(&Value::Duration(120_000_000_000)));
        assert_eq!(f.args(), &os(&["one-extra-argument"])[..]);
        assert_eq!(f.nflag(), 9);
    }

    /// go1.26.5 flag_test.go `TestParseError` and `TestRangeError`.
    #[test]
    fn go_test_parse_and_range_errors() {
        fn set() -> FlagSet {
            let mut fs = FlagSet::new("parse error test");
            for (name, v) in [
                ("bool", Value::Bool(false)),
                ("int", Value::Int(0)),
                ("int64", Value::Int64(0)),
                ("uint", Value::Uint(0)),
                ("uint64", Value::Uint(0)),
                ("float64", Value::Float64(0.0)),
                ("duration", Value::Duration(0)),
            ] {
                assert_eq!(fs.var(name, v).map(|_| ()), Ok(()));
            }
            fs
        }
        for typ in [
            "bool", "int", "int64", "uint", "uint64", "float64", "duration",
        ] {
            let text = err(&mut set(), &[&format!("-{typ}=x")]);
            assert!(
                text.contains("invalid") && text.contains("parse error"),
                "{typ}: {text}"
            );
        }
        for arg in [
            "-int=123456789012345678901",
            "-int64=123456789012345678901",
            "-uint=123456789012345678901",
            "-uint64=123456789012345678901",
            "-float64=1e1000",
        ] {
            let text = err(&mut set(), &[arg]);
            assert!(
                text.contains("invalid") && text.contains("value out of range"),
                "{arg}: {text}"
            );
        }
    }

    #[test]
    fn parse_one_texts_and_termination() {
        fn fs() -> FlagSet {
            let mut f = FlagSet::new("refs");
            let _ = f.var("ticket", Value::String(Vec::new()));
            let _ = f.var("no-relay", Value::Bool(false));
            let _ = f.var("jobs", Value::Int(0));
            let _ = f.var("gc-interval", Value::Duration(0));
            f
        }
        assert_eq!(
            err(&mut fs(), &["--bogus"]),
            "flag provided but not defined: -bogus"
        );
        assert_eq!(
            err(&mut fs(), &["-bogus=1"]),
            "flag provided but not defined: -bogus"
        );
        assert_eq!(err(&mut fs(), &["---x"]), "bad flag syntax: ---x");
        assert_eq!(err(&mut fs(), &["-=x"]), "bad flag syntax: -=x");
        assert_eq!(err(&mut fs(), &["--="]), "bad flag syntax: --=");
        assert_eq!(err(&mut fs(), &["-h"]), ERR_HELP);
        assert_eq!(err(&mut fs(), &["--help=1"]), ERR_HELP);
        assert_eq!(
            err(&mut fs(), &["--ticket"]),
            "flag needs an argument: -ticket"
        );
        assert_eq!(
            err(&mut fs(), &["--no-relay=maybe"]),
            "invalid boolean value \"maybe\" for -no-relay: parse error"
        );
        assert_eq!(
            err(&mut fs(), &["--no-relay="]),
            "invalid boolean value \"\" for -no-relay: parse error"
        );
        assert_eq!(
            err(&mut fs(), &["--jobs", "x"]),
            "invalid value \"x\" for flag -jobs: parse error"
        );
        assert_eq!(
            err(&mut fs(), &["--jobs", "9223372036854775808"]),
            "invalid value \"9223372036854775808\" for flag -jobs: value out of range"
        );
        assert_eq!(
            err(&mut fs(), &["--gc-interval", "1e3"]),
            "invalid value \"1e3\" for flag -gc-interval: parse error"
        );

        // A value is the next argument, whatever it looks like; a bool flag takes none.
        let mut f = fs();
        assert_eq!(f.parse(&os(&["--ticket", "--no-relay", "p"])), Ok(()));
        assert_eq!(
            f.lookup(b"ticket"),
            Some(&Value::String(b"--no-relay".to_vec()))
        );
        assert_eq!(f.args(), &os(&["p"])[..]);
        let mut f = fs();
        assert_eq!(f.parse(&os(&["--no-relay", "false"])), Ok(()));
        assert_eq!(f.lookup(b"no-relay"), Some(&Value::Bool(true)));
        assert_eq!(f.args(), &os(&["false"])[..]);
        // name=value splits at the first '='.
        let mut f = fs();
        assert_eq!(f.parse(&os(&["-ticket=a=b"])), Ok(()));
        assert_eq!(f.lookup(b"ticket"), Some(&Value::String(b"a=b".to_vec())));
        // Parsing stops at the first positional argument; "-" is positional; "--" is consumed.
        let mut f = fs();
        assert_eq!(f.parse(&os(&["x", "--bogus"])), Ok(()));
        assert_eq!(f.args(), &os(&["x", "--bogus"])[..]);
        let mut f = fs();
        assert_eq!(f.parse(&os(&["-", "--bogus"])), Ok(()));
        assert_eq!(f.args(), &os(&["-", "--bogus"])[..]);
        let mut f = fs();
        assert_eq!(f.parse(&os(&["--jobs=3", "--", "--bogus"])), Ok(()));
        assert_eq!(f.args(), &os(&["--bogus"])[..]);
        assert_eq!(f.visit().collect::<Vec<_>>(), vec![&b"jobs"[..]]);
        let mut f = fs();
        assert_eq!(f.parse(&os(&["", "x"])), Ok(()));
        assert_eq!(f.args(), &os(&["", "x"])[..]);
    }

    #[test]
    fn non_utf8_arguments() {
        use std::os::unix::ffi::OsStringExt;
        let mut f = FlagSet::new("x");
        let _ = f.var("jobs", Value::Int(0));
        let _ = f.var("path", Value::String(Vec::new()));
        let bad = OsString::from_vec(b"--b\xffx".to_vec());
        assert_eq!(
            f.parse(&[bad]),
            Err(FlagError(
                b"flag provided but not defined: -b\xffx".to_vec()
            ))
        );
        let mut f = FlagSet::new("x");
        let _ = f.var("jobs", Value::Int(0));
        let args = vec![
            OsString::from("--jobs"),
            OsString::from_vec(b"1\xff".to_vec()),
        ];
        assert_eq!(
            f.parse(&args),
            Err(FlagError(
                b"invalid value \"1\\xff\" for flag -jobs: parse error".to_vec()
            ))
        );
        let mut f = FlagSet::new("x");
        let _ = f.var("path", Value::String(Vec::new()));
        let args = vec![OsString::from_vec(b"-path=\xfe".to_vec())];
        assert_eq!(f.parse(&args), Ok(()));
        assert_eq!(f.lookup(b"path"), Some(&Value::String(b"\xfe".to_vec())));
    }

    #[test]
    fn values_strings_and_slices() {
        assert_eq!(Value::Float64(1e21).string(), b"1e+21");
        assert_eq!(Value::Float64(0.0).string(), b"0");
        assert_eq!(Value::Duration(4 * 3_600_000_000_000).string(), b"4h0m0s");
        assert_eq!(Value::Uint(u64::MAX).string(), b"18446744073709551615");
        let mut v = Value::StringSlice {
            items: vec![b"default".to_vec()],
            has_been_set: false,
        };
        assert_eq!(v.set(b"a, b"), Ok(()));
        assert_eq!(v.set(b" c "), Ok(()));
        assert_eq!(
            v,
            Value::StringSlice {
                items: vec![b"a".to_vec(), b"b".to_vec(), b"c".to_vec()],
                has_been_set: true
            }
        );
        assert_eq!(v.string(), b"[a b c]");
        let mut v = Value::StringSlice {
            items: Vec::new(),
            has_been_set: false,
        };
        assert_eq!(v.set(b""), Ok(()));
        assert_eq!(v.string(), b"[]");
        assert_eq!(
            v,
            Value::StringSlice {
                items: vec![Vec::new()],
                has_been_set: true
            }
        );
    }

    /// go1.26.5 `time.ParseDuration` texts for non-UTF-8 input (captured from Go by a throwaway probe).
    #[test]
    fn parse_duration_non_utf8_texts() {
        let cases: &[(&[u8], &str)] = &[
            (b"\xff", "time: invalid duration \"\\xff\""),
            (
                b"1h\xff",
                "time: unknown unit \"h\\xff\" in duration \"1h\\xff\"",
            ),
            (
                b"1\xff",
                "time: unknown unit \"\\xff\" in duration \"1\\xff\"",
            ),
            (b"-\xff", "time: invalid duration \"-\\xff\""),
            (
                b"1.5\xc2\xb5\xff",
                "time: unknown unit \"\\xc2\\xb5\\xff\" in duration \"1.5\\xc2\\xb5\\xff\"",
            ),
            (
                b"99999999999999999999h\xff",
                "time: invalid duration \"99999999999999999999h\\xff\"",
            ),
            (
                b"1h2\xffm",
                "time: unknown unit \"\\xffm\" in duration \"1h2\\xffm\"",
            ),
            (
                b"\xef\xbf\xbd1s\xff",
                "time: invalid duration \"\\xef\\xbf\\xbd1s\\xff\"",
            ),
            (b".\xff", "time: invalid duration \".\\xff\""),
            (
                b"1.\xff",
                "time: unknown unit \"\\xff\" in duration \"1.\\xff\"",
            ),
            (
                b"+0\xff",
                "time: unknown unit \"\\xff\" in duration \"+0\\xff\"",
            ),
            (
                b"\xff\xef\xbf\xbd",
                "time: invalid duration \"\\xff\\xef\\xbf\\xbd\"",
            ),
            (
                b"1\xe2\x82s",
                "time: unknown unit \"\\xe2\\x82s\" in duration \"1\\xe2\\x82s\"",
            ),
            (
                b"1h\x01\xff",
                "time: unknown unit \"h\\x01\\xff\" in duration \"1h\\x01\\xff\"",
            ),
            (
                b"1s\"\\\xff",
                "time: unknown unit \"s\\\"\\\\\\xff\" in duration \"1s\\\"\\\\\\xff\"",
            ),
            (
                b"2562047h47m16.854775808s\xff",
                "time: unknown unit \"s\\xff\" in duration \"2562047h47m16.854775808s\\xff\"",
            ),
            (
                b"9223372036854775808ns1s\xff",
                "time: unknown unit \"s\\xff\" in duration \"9223372036854775808ns1s\\xff\"",
            ),
            (
                b"1\xce\xbcs2\xc2\xb5s3\xff",
                "time: unknown unit \"\\xff\" in duration \"1\\xce\\xbcs2\\xc2\\xb5s3\\xff\"",
            ),
        ];
        for (input, want) in cases {
            assert_eq!(parse_duration(input), Err((*want).to_owned()), "{input:?}");
        }
        assert_eq!(parse_duration(b"1h30m"), Ok(5_400_000_000_000));
        assert_eq!(parse_duration("1µs".as_bytes()), Ok(1_000));
    }

    #[test]
    fn define_panics_become_errors() {
        let mut f = FlagSet::new("serve");
        assert_eq!(f.var("x", Value::Bool(false)).map(|_| ()), Ok(()));
        assert_eq!(
            f.var("x", Value::Bool(false)).map(|_| ()),
            Err(String::from("serve flag redefined: x"))
        );
        let mut f = FlagSet::new("");
        assert_eq!(f.var("x", Value::Bool(false)).map(|_| ()), Ok(()));
        assert_eq!(
            f.var("x", Value::Bool(false)).map(|_| ()),
            Err(String::from("flag redefined: x"))
        );
        assert_eq!(
            f.var("-foo", Value::Bool(false)).map(|_| ()),
            Err(String::from("flag \"-foo\" begins with -"))
        );
        assert_eq!(
            f.var("foo=bar", Value::Bool(false)).map(|_| ()),
            Err(String::from("flag \"foo=bar\" contains ="))
        );
        assert_eq!(
            f.set(b"nosuch", b"1"),
            Err(FlagError(b"no such flag -nosuch".to_vec()))
        );
    }
}
