//! `encoding/json` v1 for structs of string and bool fields only (go1.26.5 `encoding/json/encode.go`,
//! `indent.go`, `scanner.go`, `decode.go`, `fold.go`; escapeHTML = true).

/// A field value to marshal.
pub enum JsonField<'a> {
    Str(&'a [u8]),
    Bool(bool),
}

const HEX: &[u8; 16] = b"0123456789abcdef";

/// `appendString(dst, src, escapeHTML=true)`.
fn append_string(dst: &mut Vec<u8>, src: &[u8]) {
    dst.push(b'"');
    let mut i = 0usize;
    while let Some(&b) = src.get(i) {
        if b < 0x80 {
            match b {
                b'"' | b'\\' => dst.extend_from_slice(&[b'\\', b]),
                0x08 => dst.extend_from_slice(b"\\b"),
                0x0c => dst.extend_from_slice(b"\\f"),
                b'\n' => dst.extend_from_slice(b"\\n"),
                b'\r' => dst.extend_from_slice(b"\\r"),
                b'\t' => dst.extend_from_slice(b"\\t"),
                // Other control bytes, and <, >, & (escapeHTML).
                0x00..=0x1f | b'<' | b'>' | b'&' => dst.extend_from_slice(&[
                    b'\\',
                    b'u',
                    b'0',
                    b'0',
                    HEX[usize::from(b >> 4)],
                    HEX[usize::from(b & 0x0f)],
                ]),
                _ => dst.push(b),
            }
            i += 1;
            continue;
        }
        match decode_rune(src.get(i..).unwrap_or_default()) {
            None => {
                dst.extend_from_slice(b"\\ufffd");
                i += 1;
            }
            Some(('\u{2028}', size)) => {
                dst.extend_from_slice(b"\\u2028");
                i += size;
            }
            Some(('\u{2029}', size)) => {
                dst.extend_from_slice(b"\\u2029");
                i += size;
            }
            Some((_, size)) => {
                dst.extend_from_slice(src.get(i..i + size).unwrap_or_default());
                i += size;
            }
        }
    }
    dst.push(b'"');
}

/// `utf8.DecodeRune` for a valid rune: `None` means `(RuneError, 1)` (also for an empty input).
fn decode_rune(s: &[u8]) -> Option<(char, usize)> {
    let width = match *s.first()? {
        0x00..=0x7f => 1,
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return None,
    };
    let chunk = s.get(..width)?;
    let c = std::str::from_utf8(chunk).ok()?.chars().next()?;
    Some((c, width))
}

/// `json.MarshalIndent(v, "", "  ")` of a struct, without a trailing "\n". The caller drops omitempty
/// fields.
pub fn marshal_indent_object(fields: &[(&str, JsonField<'_>)]) -> Vec<u8> {
    if fields.is_empty() {
        return b"{}".to_vec();
    }
    let mut out = Vec::with_capacity(64);
    out.push(b'{');
    for (i, (name, value)) in fields.iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        out.extend_from_slice(b"\n  ");
        append_string(&mut out, name.as_bytes());
        out.extend_from_slice(b": ");
        match value {
            JsonField::Str(s) => append_string(&mut out, s),
            JsonField::Bool(true) => out.extend_from_slice(b"true"),
            JsonField::Bool(false) => out.extend_from_slice(b"false"),
        }
    }
    out.extend_from_slice(b"\n}");
    out
}

/// The Go type of a struct field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JsonKind {
    String,
    Bool,
}

impl JsonKind {
    fn go_type(self) -> &'static str {
        match self {
            JsonKind::String => "string",
            JsonKind::Bool => "bool",
        }
    }
}

/// One struct field to unmarshal into.
pub struct JsonFieldSpec {
    pub name: &'static str,
    pub kind: JsonKind,
}

/// A decoded field value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JsonValue {
    String(Vec<u8>),
    Bool(bool),
}

/// `json.Unmarshal` errors with Go's texts.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JsonError {
    #[error("unexpected end of JSON input")]
    UnexpectedEnd,
    /// `invalid character 'x' looking for beginning of value`, … (Go scanner texts).
    #[error("{0}")]
    Syntax(String),
    #[error(
        "json: cannot unmarshal {value} into Go struct field {go_struct}.{field} of type {go_type}"
    )]
    Type {
        value: &'static str,
        go_struct: &'static str,
        field: String,
        go_type: &'static str,
    },
    /// A top-level value that is not an object (`"x"`, `1`, `true`, `[]`): Go's `UnmarshalTypeError`
    /// without struct context. `go_type` is the `go_struct` argument of [`unmarshal_object`].
    #[error("json: cannot unmarshal {value} into Go value of type {go_type}")]
    ValueType {
        value: &'static str,
        go_type: &'static str,
    },
}

// ---------------------------------------------------------------------------------------------------
// Scanner (scanner.go): the validity check Unmarshal runs before decoding anything.

#[derive(Clone, Copy, PartialEq, Eq)]
enum Op {
    Continue,
    BeginLiteral,
    BeginObject,
    ObjectKey,
    ObjectValue,
    EndObject,
    BeginArray,
    ArrayValue,
    EndArray,
    SkipSpace,
    End,
    Error,
}

#[derive(Clone, Copy)]
enum State {
    BeginValueOrEmpty,
    BeginValue,
    BeginStringOrEmpty,
    BeginString,
    EndValue,
    EndTop,
    InString,
    InStringEsc,
    InStringEscU,
    InStringEscU1,
    InStringEscU12,
    InStringEscU123,
    Neg,
    One,
    Zero,
    Dot,
    Dot0,
    E,
    ESign,
    E0,
    T,
    Tr,
    Tru,
    F,
    Fa,
    Fal,
    Fals,
    N,
    Nu,
    Nul,
    Error,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ParseState {
    ObjectKey,
    ObjectValue,
    ArrayValue,
}

const MAX_NESTING_DEPTH: usize = 10000;

struct Scanner {
    step: State,
    end_top: bool,
    parse_state: Vec<ParseState>,
    err: Option<JsonError>,
}

fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\r' | b'\n')
}

fn is_hex(c: u8) -> bool {
    c.is_ascii_hexdigit()
}

/// `quoteChar`: `strconv.Quote(string(rune(c)))` with single quotes.
fn quote_char(c: u8) -> String {
    let inner = match c {
        b'\'' => return "'\\''".to_string(),
        b'"' => return "'\"'".to_string(),
        0x07 => "\\a".to_string(),
        0x08 => "\\b".to_string(),
        0x0c => "\\f".to_string(),
        b'\n' => "\\n".to_string(),
        b'\r' => "\\r".to_string(),
        b'\t' => "\\t".to_string(),
        0x0b => "\\v".to_string(),
        b'\\' => "\\\\".to_string(),
        0x00..=0x1f | 0x7f => format!("\\x{c:02x}"),
        _ if crate::hex::latin1_is_print(c) => char::from(c).to_string(),
        _ => format!("\\u{:04x}", u32::from(c)),
    };
    format!("'{inner}'")
}

impl Scanner {
    fn new() -> Scanner {
        Scanner {
            step: State::BeginValue,
            end_top: false,
            parse_state: Vec::new(),
            err: None,
        }
    }

    fn error(&mut self, c: u8, context: &str) -> Op {
        self.step = State::Error;
        self.err = Some(JsonError::Syntax(format!(
            "invalid character {} {context}",
            quote_char(c)
        )));
        Op::Error
    }

    fn push_parse_state(&mut self, c: u8, ps: ParseState, success: Op) -> Op {
        self.parse_state.push(ps);
        if self.parse_state.len() <= MAX_NESTING_DEPTH {
            return success;
        }
        self.error(c, "exceeded max depth")
    }

    fn pop_parse_state(&mut self) {
        self.parse_state.pop();
        if self.parse_state.is_empty() {
            self.step = State::EndTop;
            self.end_top = true;
        } else {
            self.step = State::EndValue;
        }
    }

    fn eof(&mut self) -> Op {
        if self.err.is_some() {
            return Op::Error;
        }
        if self.end_top {
            return Op::End;
        }
        self.step(b' ');
        if self.end_top {
            return Op::End;
        }
        if self.err.is_none() {
            self.err = Some(JsonError::UnexpectedEnd);
        }
        Op::Error
    }

    fn step(&mut self, c: u8) -> Op {
        match self.step {
            State::BeginValueOrEmpty => {
                if is_space(c) {
                    Op::SkipSpace
                } else if c == b']' {
                    self.end_value(c)
                } else {
                    self.begin_value(c)
                }
            }
            State::BeginValue => self.begin_value(c),
            State::BeginStringOrEmpty => {
                if is_space(c) {
                    Op::SkipSpace
                } else if c == b'}' {
                    if let Some(last) = self.parse_state.last_mut() {
                        *last = ParseState::ObjectValue;
                    }
                    self.end_value(c)
                } else {
                    self.begin_string(c)
                }
            }
            State::BeginString => self.begin_string(c),
            State::EndValue => self.end_value(c),
            State::EndTop => self.end_top_state(c),
            State::InString => {
                if c == b'"' {
                    self.step = State::EndValue;
                    Op::Continue
                } else if c == b'\\' {
                    self.step = State::InStringEsc;
                    Op::Continue
                } else if c < 0x20 {
                    self.error(c, "in string literal")
                } else {
                    Op::Continue
                }
            }
            State::InStringEsc => match c {
                b'b' | b'f' | b'n' | b'r' | b't' | b'\\' | b'/' | b'"' => {
                    self.step = State::InString;
                    Op::Continue
                }
                b'u' => {
                    self.step = State::InStringEscU;
                    Op::Continue
                }
                _ => self.error(c, "in string escape code"),
            },
            State::InStringEscU => self.hex_digit(c, State::InStringEscU1),
            State::InStringEscU1 => self.hex_digit(c, State::InStringEscU12),
            State::InStringEscU12 => self.hex_digit(c, State::InStringEscU123),
            State::InStringEscU123 => self.hex_digit(c, State::InString),
            State::Neg => {
                if c == b'0' {
                    self.step = State::Zero;
                    Op::Continue
                } else if (b'1'..=b'9').contains(&c) {
                    self.step = State::One;
                    Op::Continue
                } else {
                    self.error(c, "in numeric literal")
                }
            }
            State::One => {
                if c.is_ascii_digit() {
                    self.step = State::One;
                    Op::Continue
                } else {
                    self.zero(c)
                }
            }
            State::Zero => self.zero(c),
            State::Dot => {
                if c.is_ascii_digit() {
                    self.step = State::Dot0;
                    Op::Continue
                } else {
                    self.error(c, "after decimal point in numeric literal")
                }
            }
            State::Dot0 => {
                if c.is_ascii_digit() {
                    Op::Continue
                } else if c == b'e' || c == b'E' {
                    self.step = State::E;
                    Op::Continue
                } else {
                    self.end_value(c)
                }
            }
            State::E => {
                if c == b'+' || c == b'-' {
                    self.step = State::ESign;
                    Op::Continue
                } else {
                    self.e_sign(c)
                }
            }
            State::ESign => self.e_sign(c),
            State::E0 => {
                if c.is_ascii_digit() {
                    Op::Continue
                } else {
                    self.end_value(c)
                }
            }
            State::T => self.literal(c, b'r', State::Tr, "in literal true (expecting 'r')"),
            State::Tr => self.literal(c, b'u', State::Tru, "in literal true (expecting 'u')"),
            State::Tru => self.literal(c, b'e', State::EndValue, "in literal true (expecting 'e')"),
            State::F => self.literal(c, b'a', State::Fa, "in literal false (expecting 'a')"),
            State::Fa => self.literal(c, b'l', State::Fal, "in literal false (expecting 'l')"),
            State::Fal => self.literal(c, b's', State::Fals, "in literal false (expecting 's')"),
            State::Fals => {
                self.literal(c, b'e', State::EndValue, "in literal false (expecting 'e')")
            }
            State::N => self.literal(c, b'u', State::Nu, "in literal null (expecting 'u')"),
            State::Nu => self.literal(c, b'l', State::Nul, "in literal null (expecting 'l')"),
            State::Nul => self.literal(c, b'l', State::EndValue, "in literal null (expecting 'l')"),
            State::Error => Op::Error,
        }
    }

    fn begin_value(&mut self, c: u8) -> Op {
        if is_space(c) {
            return Op::SkipSpace;
        }
        match c {
            b'{' => {
                self.step = State::BeginStringOrEmpty;
                self.push_parse_state(c, ParseState::ObjectKey, Op::BeginObject)
            }
            b'[' => {
                self.step = State::BeginValueOrEmpty;
                self.push_parse_state(c, ParseState::ArrayValue, Op::BeginArray)
            }
            b'"' => self.begin_literal(State::InString),
            b'-' => self.begin_literal(State::Neg),
            b'0' => self.begin_literal(State::Zero),
            b't' => self.begin_literal(State::T),
            b'f' => self.begin_literal(State::F),
            b'n' => self.begin_literal(State::N),
            b'1'..=b'9' => self.begin_literal(State::One),
            _ => self.error(c, "looking for beginning of value"),
        }
    }

    fn begin_literal(&mut self, next: State) -> Op {
        self.step = next;
        Op::BeginLiteral
    }

    fn begin_string(&mut self, c: u8) -> Op {
        if is_space(c) {
            return Op::SkipSpace;
        }
        if c == b'"' {
            self.step = State::InString;
            return Op::BeginLiteral;
        }
        self.error(c, "looking for beginning of object key string")
    }

    fn end_value(&mut self, c: u8) -> Op {
        let Some(&ps) = self.parse_state.last() else {
            // Completed the top-level value before the current byte.
            self.step = State::EndTop;
            self.end_top = true;
            return self.end_top_state(c);
        };
        if is_space(c) {
            self.step = State::EndValue;
            return Op::SkipSpace;
        }
        match ps {
            ParseState::ObjectKey => {
                if c == b':' {
                    if let Some(last) = self.parse_state.last_mut() {
                        *last = ParseState::ObjectValue;
                    }
                    self.step = State::BeginValue;
                    return Op::ObjectKey;
                }
                self.error(c, "after object key")
            }
            ParseState::ObjectValue => {
                if c == b',' {
                    if let Some(last) = self.parse_state.last_mut() {
                        *last = ParseState::ObjectKey;
                    }
                    self.step = State::BeginString;
                    return Op::ObjectValue;
                }
                if c == b'}' {
                    self.pop_parse_state();
                    return Op::EndObject;
                }
                self.error(c, "after object key:value pair")
            }
            ParseState::ArrayValue => {
                if c == b',' {
                    self.step = State::BeginValue;
                    return Op::ArrayValue;
                }
                if c == b']' {
                    self.pop_parse_state();
                    return Op::EndArray;
                }
                self.error(c, "after array element")
            }
        }
    }

    fn end_top_state(&mut self, c: u8) -> Op {
        if !is_space(c) {
            // Complain about a non-space byte on the next call.
            self.error(c, "after top-level value");
        }
        Op::End
    }

    fn hex_digit(&mut self, c: u8, next: State) -> Op {
        if is_hex(c) {
            self.step = next;
            return Op::Continue;
        }
        self.error(c, "in \\u hexadecimal character escape")
    }

    fn zero(&mut self, c: u8) -> Op {
        if c == b'.' {
            self.step = State::Dot;
            return Op::Continue;
        }
        if c == b'e' || c == b'E' {
            self.step = State::E;
            return Op::Continue;
        }
        self.end_value(c)
    }

    fn e_sign(&mut self, c: u8) -> Op {
        if c.is_ascii_digit() {
            self.step = State::E0;
            return Op::Continue;
        }
        self.error(c, "in exponent of numeric literal")
    }

    fn literal(&mut self, c: u8, want: u8, next: State, context: &str) -> Op {
        if c == want {
            self.step = next;
            return Op::Continue;
        }
        self.error(c, context)
    }
}

/// `checkValid`.
fn check_valid(data: &[u8]) -> Result<(), JsonError> {
    let mut scan = Scanner::new();
    for &c in data {
        if scan.step(c) == Op::Error {
            return Err(scan.err.take().unwrap_or(JsonError::UnexpectedEnd));
        }
    }
    if scan.eof() == Op::Error {
        return Err(scan.err.take().unwrap_or(JsonError::UnexpectedEnd));
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------------
// Decoding (decode.go) over input that check_valid accepted.

/// `getu4`: the `\uXXXX` escape at the start of `s`, or `None`.
fn getu4(s: &[u8]) -> Option<u32> {
    let esc = s.get(..6)?;
    if esc.first() != Some(&b'\\') || esc.get(1) != Some(&b'u') {
        return None;
    }
    let mut r = 0u32;
    for &c in esc.get(2..)? {
        r = r * 16 + char::from(c).to_digit(16)?;
    }
    Some(r)
}

fn push_rune(out: &mut Vec<u8>, r: char) {
    let mut buf = [0u8; 4];
    out.extend_from_slice(r.encode_utf8(&mut buf).as_bytes());
}

/// `unquoteBytes` of a string literal's inner bytes (the quotes stripped), for scanner-validated input.
fn unquote(s: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    let mut r = 0usize;
    while let Some(&c) = s.get(r) {
        if c == b'\\' {
            match s.get(r + 1) {
                Some(&e @ (b'"' | b'\\' | b'/' | b'\'')) => {
                    out.push(e);
                    r += 2;
                }
                Some(b'b') => {
                    out.push(0x08);
                    r += 2;
                }
                Some(b'f') => {
                    out.push(0x0c);
                    r += 2;
                }
                Some(b'n') => {
                    out.push(b'\n');
                    r += 2;
                }
                Some(b'r') => {
                    out.push(b'\r');
                    r += 2;
                }
                Some(b't') => {
                    out.push(b'\t');
                    r += 2;
                }
                Some(b'u') => {
                    let Some(rr) = getu4(s.get(r..).unwrap_or_default()) else {
                        // Unreachable after check_valid.
                        return out;
                    };
                    r += 6;
                    if (0xd800..0xe000).contains(&rr) {
                        let rr1 = getu4(s.get(r..).unwrap_or_default());
                        if let Some(dec) = rr1.and_then(|rr1| decode_surrogates(rr, rr1)) {
                            r += 6;
                            push_rune(&mut out, dec);
                            continue;
                        }
                        push_rune(&mut out, char::REPLACEMENT_CHARACTER);
                        continue;
                    }
                    push_rune(
                        &mut out,
                        char::from_u32(rr).unwrap_or(char::REPLACEMENT_CHARACTER),
                    );
                }
                // Unreachable after check_valid.
                _ => return out,
            }
        } else if c < 0x80 {
            out.push(c);
            r += 1;
        } else {
            match decode_rune(s.get(r..).unwrap_or_default()) {
                Some((ch, size)) => {
                    out.extend_from_slice(s.get(r..r + size).unwrap_or_default());
                    let _ = ch;
                    r += size;
                }
                None => {
                    push_rune(&mut out, char::REPLACEMENT_CHARACTER);
                    r += 1;
                }
            }
        }
    }
    out
}

/// `utf16.DecodeRune`: a high surrogate followed by a low one.
fn decode_surrogates(r1: u32, r2: u32) -> Option<char> {
    if (0xd800..0xdc00).contains(&r1) && (0xdc00..0xe000).contains(&r2) {
        return char::from_u32((((r1 - 0xd800) << 10) | (r2 - 0xdc00)) + 0x10000);
    }
    None
}

/// `foldName`: ASCII upper-casing plus `foldRune` for multi-byte runes. `foldRune` is exact for every rune
/// that folds to ASCII (U+212A KELVIN SIGN → `K`, U+017F LATIN SMALL LETTER LONG S → `S`); other runes are
/// kept, so field names must be ASCII (Go struct tags such as `no_relay`) for the match to be Go's.
fn fold_name(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0usize;
    while let Some(&c) = input.get(i) {
        if c < 0x80 {
            out.push(c.to_ascii_uppercase());
            i += 1;
            continue;
        }
        let (r, n) = decode_rune(input.get(i..).unwrap_or_default())
            .unwrap_or((char::REPLACEMENT_CHARACTER, 1));
        let folded = match r {
            '\u{212a}' => 'K',
            '\u{017f}' => 'S',
            other => other,
        };
        push_rune(&mut out, folded);
        i += n;
    }
    out
}

struct Decoder<'a> {
    data: &'a [u8],
    pos: usize,
    go_type: &'static str,
    struct_name: &'static str,
    fields: &'a [JsonFieldSpec],
    slots: Vec<Option<JsonValue>>,
    err: Option<JsonError>,
}

impl Decoder<'_> {
    fn peek(&self) -> Option<u8> {
        self.data.get(self.pos).copied()
    }

    fn skip_space(&mut self) {
        while self.peek().is_some_and(is_space) {
            self.pos += 1;
        }
    }

    fn save(&mut self, err: JsonError) {
        if self.err.is_none() {
            self.err = Some(err);
        }
    }

    /// The inner bytes of the string literal at `pos`; `pos` moves past its closing quote.
    fn string_literal(&mut self) -> &[u8] {
        let start = self.pos + 1;
        let mut i = start;
        while let Some(&c) = self.data.get(i) {
            match c {
                b'\\' => i += 2,
                b'"' => break,
                _ => i += 1,
            }
        }
        self.pos = i + 1;
        self.data.get(start..i).unwrap_or_default()
    }

    /// Skips the value at `pos` (valid JSON).
    fn skip_value(&mut self) {
        let mut depth = 0usize;
        loop {
            self.skip_space();
            match self.peek() {
                None => return,
                Some(b'"') => {
                    self.string_literal();
                }
                Some(b'{' | b'[') => {
                    depth += 1;
                    self.pos += 1;
                }
                Some(b'}' | b']') => {
                    depth = depth.saturating_sub(1);
                    self.pos += 1;
                }
                Some(b',' | b':') => self.pos += 1,
                Some(_) => {
                    // true, false, null or a number.
                    while self.peek().is_some_and(|c| {
                        c.is_ascii_alphanumeric() || matches!(c, b'-' | b'+' | b'.')
                    }) {
                        self.pos += 1;
                    }
                }
            }
            if depth == 0 {
                return;
            }
        }
    }

    /// The Go value kind named in an `UnmarshalTypeError` for the value at `pos`, or `None` for null.
    fn literal_kind(&self) -> Option<&'static str> {
        match self.peek() {
            Some(b'"') => Some("string"),
            Some(b't' | b'f') => Some("bool"),
            Some(b'n') => None,
            Some(b'[') => Some("array"),
            Some(b'{') => Some("object"),
            _ => Some("number"),
        }
    }

    fn top_level(&mut self) {
        self.skip_space();
        if self.peek() == Some(b'{') {
            self.object();
            return;
        }
        if let Some(value) = self.literal_kind() {
            self.save(JsonError::ValueType {
                value,
                go_type: self.go_type,
            });
        }
        self.skip_value();
    }

    fn object(&mut self) {
        self.pos += 1; // {
        self.skip_space();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return;
        }
        loop {
            self.skip_space();
            let key = unquote(self.string_literal());
            self.skip_space();
            self.pos += 1; // :
            self.skip_space();
            match self.lookup(&key) {
                Some(i) => self.store(i),
                None => self.skip_value(),
            }
            self.skip_space();
            match self.peek() {
                Some(b',') => self.pos += 1,
                _ => {
                    self.pos += 1; // }
                    return;
                }
            }
        }
    }

    /// `byExactName`, then `byFoldedName` (the first field whose folded name matches).
    fn lookup(&self, key: &[u8]) -> Option<usize> {
        if let Some(i) = self.fields.iter().position(|f| f.name.as_bytes() == key) {
            return Some(i);
        }
        let folded = fold_name(key);
        self.fields
            .iter()
            .position(|f| fold_name(f.name.as_bytes()) == folded)
    }

    fn store(&mut self, i: usize) {
        let Some(spec) = self.fields.get(i) else {
            self.skip_value();
            return;
        };
        let (name, kind) = (spec.name, spec.kind);
        let value = match (self.peek(), kind) {
            (Some(b'"'), JsonKind::String) => {
                let v = unquote(self.string_literal());
                Some(JsonValue::String(v))
            }
            (Some(c @ (b't' | b'f')), JsonKind::Bool) => {
                self.skip_value();
                Some(JsonValue::Bool(c == b't'))
            }
            _ => {
                if let Some(value) = self.literal_kind() {
                    self.save(JsonError::Type {
                        value,
                        go_struct: self.struct_name,
                        field: name.to_string(),
                        go_type: kind.go_type(),
                    });
                }
                self.skip_value();
                None
            }
        };
        if let (Some(v), Some(slot)) = (value, self.slots.get_mut(i)) {
            *slot = Some(v);
        }
    }
}

/// Go `json.Unmarshal` into a struct: exact key match, then case-folding match (incl. K/U+212A,
/// S/U+017F); the last duplicate wins; null leaves a field unset; unknown keys are ignored; the first
/// type error is recorded and decoding continues; invalid UTF-8 inside strings becomes U+FFFD per byte.
/// Returns one slot per spec (None = not set) and the first error.
///
/// `go_struct` names the Go type. Pass it package-qualified (`"worktree.Config"`): a field error uses
/// the part after the last '.' (`Go struct field Config.ticket`), a top-level non-object uses the whole
/// name (`Go value of type worktree.Config`), as Go's `reflect.Type.Name()` and `String()` do. A syntax
/// error leaves every slot unset, because Go validates the whole input before decoding.
pub fn unmarshal_object(
    data: &[u8],
    go_struct: &'static str,
    fields: &[JsonFieldSpec],
) -> (Vec<Option<JsonValue>>, Result<(), JsonError>) {
    let slots = vec![None; fields.len()];
    if let Err(e) = check_valid(data) {
        return (slots, Err(e));
    }
    let struct_name = go_struct.rsplit('.').next().unwrap_or(go_struct);
    let mut d = Decoder {
        data,
        pos: 0,
        go_type: go_struct,
        struct_name,
        fields,
        slots,
        err: None,
    };
    d.top_level();
    let result = match d.err {
        Some(e) => Err(e),
        None => Ok(()),
    };
    (d.slots, result)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &[JsonFieldSpec] = &[
        JsonFieldSpec {
            name: "ticket",
            kind: JsonKind::String,
        },
        JsonFieldSpec {
            name: "name",
            kind: JsonKind::String,
        },
        JsonFieldSpec {
            name: "relay",
            kind: JsonKind::String,
        },
        JsonFieldSpec {
            name: "no_relay",
            kind: JsonKind::Bool,
        },
        JsonFieldSpec {
            name: "no_discovery",
            kind: JsonKind::Bool,
        },
        JsonFieldSpec {
            name: "user",
            kind: JsonKind::String,
        },
    ];

    fn s(v: &[u8]) -> Option<JsonValue> {
        Some(JsonValue::String(v.to_vec()))
    }

    fn err_text(data: &[u8]) -> String {
        match unmarshal_object(data, "worktree.Config", CONFIG).1 {
            Ok(()) => String::new(),
            Err(e) => e.to_string(),
        }
    }

    // worktree §3.2 (verified with Go 1.26.5).
    #[test]
    fn marshal_config_samples() {
        let out = marshal_indent_object(&[
            ("ticket", JsonField::Str(b"t")),
            ("name", JsonField::Str(b"n")),
        ]);
        assert_eq!(out, b"{\n  \"ticket\": \"t\",\n  \"name\": \"n\"\n}");
        let out = marshal_indent_object(&[
            ("ticket", JsonField::Str(b"dstore1abc")),
            ("name", JsonField::Str(b"trees/<a&b> x\x01\"\\")),
            ("no_relay", JsonField::Bool(true)),
            ("user", JsonField::Str(b"Dr <d@x>")),
        ]);
        let want = "{\n  \"ticket\": \"dstore1abc\",\n  \"name\": \"trees/\\u003ca\\u0026b\\u003e x\\u0001\\\"\\\\\",\n  \"no_relay\": true,\n  \"user\": \"Dr \\u003cd@x\\u003e\"\n}";
        assert_eq!(String::from_utf8_lossy(&out), want);
        assert_eq!(marshal_indent_object(&[]), b"{}");
    }

    #[test]
    fn marshal_escapes() {
        let mut out = Vec::new();
        append_string(&mut out, b"\x08\x0c\n\r\t\x1f\x7f\xe2\x80\xa8\xe2\x80\xa9\xff\xe2\x82\xed\xa0\x80\xc0\xaf\xc3\xa9");
        assert_eq!(
            String::from_utf8_lossy(&out),
            "\"\\b\\f\\n\\r\\t\\u001f\x7f\\u2028\\u2029\\ufffd\\ufffd\\ufffd\\ufffd\\ufffd\\ufffd\\ufffd\\ufffdé\""
        );
    }

    #[test]
    fn syntax_errors() {
        let cases: &[(&[u8], &str)] = &[
            (b"", "unexpected end of JSON input"),
            (b"  ", "unexpected end of JSON input"),
            (
                b"tru",
                "invalid character ' ' in literal true (expecting 'e')",
            ),
            (
                b"fals",
                "invalid character ' ' in literal false (expecting 'e')",
            ),
            (
                b"nul",
                "invalid character ' ' in literal null (expecting 'l')",
            ),
            (
                b"123e",
                "invalid character ' ' in exponent of numeric literal",
            ),
            (b"\"hello", "unexpected end of JSON input"),
            (b"[1,2,3", "unexpected end of JSON input"),
            (b"{\"key\":1", "unexpected end of JSON input"),
            (b"{\"key\":1,", "unexpected end of JSON input"),
            (
                b"{\"X\": \"foo\", \"Y\"}",
                "invalid character '}' after object key",
            ),
            (
                b"{\"X\": \"foo\" \"Y\": \"bar\"}",
                "invalid character '\"' after object key:value pair",
            ),
            (b"x", "invalid character 'x' looking for beginning of value"),
            (b"{}x", "invalid character 'x' after top-level value"),
            (
                b"{'a':1}",
                "invalid character '\\'' looking for beginning of object key string",
            ),
            (
                b"\xff",
                "invalid character 'ÿ' looking for beginning of value",
            ),
            (
                b"\x80",
                "invalid character '\\u0080' looking for beginning of value",
            ),
            (
                b"\x01",
                "invalid character '\\x01' looking for beginning of value",
            ),
            (b"\"\x0a\"", "invalid character '\\n' in string literal"),
            (b"\"\\x\"", "invalid character 'x' in string escape code"),
            (
                b"\"\\u12g4\"",
                "invalid character 'g' in \\u hexadecimal character escape",
            ),
            (b"-", "invalid character ' ' in numeric literal"),
            (
                b"1.",
                "invalid character ' ' after decimal point in numeric literal",
            ),
            (b"[1 2]", "invalid character '2' after array element"),
            (
                b"\\",
                "invalid character '\\\\' looking for beginning of value",
            ),
        ];
        for (input, want) in cases {
            let (slots, res) = unmarshal_object(input, "worktree.Config", CONFIG);
            assert!(slots.iter().all(Option::is_none));
            assert_eq!(
                res.map_err(|e| e.to_string()),
                Err(want.to_string()),
                "{input:?}"
            );
        }
        let deep = vec![b'['; MAX_NESTING_DEPTH + 1];
        assert_eq!(err_text(&deep), "invalid character '[' exceeded max depth");
    }

    // encoding/json scanner_test.go TestValid: the invalid inputs fail with these texts; the valid
    // ones pass the scanner.
    #[test]
    fn go_valid_tests() {
        let invalid: &[(&[u8], &str)] = &[
            (
                b"foo",
                "invalid character 'o' in literal false (expecting 'a')",
            ),
            (
                b"}{",
                "invalid character '}' looking for beginning of value",
            ),
            (
                b"{]",
                "invalid character ']' looking for beginning of object key string",
            ),
        ];
        for (input, want) in invalid {
            assert_eq!(
                check_valid(input).map_err(|e| e.to_string()),
                Err(want.to_string()),
                "{input:?}"
            );
        }
        for valid in [
            &b"{}"[..],
            b"{\"foo\":\"bar\"}",
            b"{\"foo\":\"bar\",\"bar\":{\"baz\":[\"qux\"]}}",
        ] {
            assert_eq!(check_valid(valid), Ok(()), "{valid:?}");
        }
    }

    #[test]
    fn decode_fields() {
        let (slots, res) = unmarshal_object(
            b" {\"TICKET\":\"a\",\"name\":\"n\xc5\xbf\",\"ticket\":\"b\",\"no_relay\":true,\"x\":{\"y\":[1,{}]},\"user\":null,\"relay\":\"\\u00e9\\ud83d\\ude00\\ud800x\\/\\u0000\xff\"} ",
            "worktree.Config",
            CONFIG,
        );
        assert_eq!(res, Ok(()));
        assert_eq!(slots[0], s(b"b"));
        assert_eq!(slots[1], s("nſ".as_bytes()));
        assert_eq!(slots[2], s("é😀\u{fffd}x/\0\u{fffd}".as_bytes()));
        assert_eq!(slots[3], Some(JsonValue::Bool(true)));
        assert_eq!(slots[4], None);
        assert_eq!(slots[5], None);
    }

    #[test]
    fn case_folding_specials() {
        let (slots, res) = unmarshal_object(
            b"{\"tic\xe2\x84\xaaet\":\"k\",\"u\xc5\xbfer\":\"s\"}",
            "worktree.Config",
            CONFIG,
        );
        assert_eq!(res, Ok(()));
        assert_eq!(slots[0], s(b"k"));
        assert_eq!(slots[5], s(b"s"));
    }

    #[test]
    fn type_errors_keep_decoding() {
        let (slots, res) = unmarshal_object(
            b"{\"ticket\":5,\"name\":true,\"no_relay\":\"yes\",\"user\":\"u\",\"relay\":[]}",
            "worktree.Config",
            CONFIG,
        );
        assert_eq!(
            res.map_err(|e| e.to_string()),
            Err(
                "json: cannot unmarshal number into Go struct field Config.ticket of type string"
                    .to_string()
            )
        );
        assert_eq!(slots[5], s(b"u"));
        assert!(slots[..5].iter().all(Option::is_none));
        assert_eq!(
            err_text(b"{\"no_relay\":{}}"),
            "json: cannot unmarshal object into Go struct field Config.no_relay of type bool"
        );
        assert_eq!(
            err_text(b"[]"),
            "json: cannot unmarshal array into Go value of type worktree.Config"
        );
        assert_eq!(
            err_text(b"\"x\""),
            "json: cannot unmarshal string into Go value of type worktree.Config"
        );
        assert_eq!(
            err_text(b"-1.5e3"),
            "json: cannot unmarshal number into Go value of type worktree.Config"
        );
        assert_eq!(
            err_text(b"false"),
            "json: cannot unmarshal bool into Go value of type worktree.Config"
        );
        assert_eq!(err_text(b"null"), "");
    }
}
