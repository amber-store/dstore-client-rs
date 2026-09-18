//! Pass 2: per-field decoding (fxamacker `decode.go` subset) and `unmarshal`.
//!
//! Every reader ports fxamacker `parseToValue` for one Go kind: the self-described tag strip and the
//! built-in tag check, then the `fill*` rules for the item's CBOR type. Go keeps decoding after a type
//! error and returns the first error in document order; pass 1 has already guaranteed the structure,
//! so failing fast returns the same error (callers discard the value on any error).

use crate::{CborType, DecodeError, Struct};

/// The self-described-CBOR tag number, stripped wherever a value is decoded.
const TAG_SELF_DESCRIBED: u64 = 55799;

/// Pass-2 cursor over input that pass 1 already validated.
pub struct Dec<'a> {
    data: &'a [u8],
    off: usize,
}

/// The Go kind a value is decoded into.
#[derive(Clone, Copy, Debug)]
enum Kind {
    /// `uint8` … `uint64`, with the width's maximum.
    Uint(u64),
    /// `int` … `int64`, with the width's range.
    Int(i64, i64),
    Bool,
    Float,
    String,
    /// `[]uint8`.
    Bytes,
    /// Any other slice.
    Slice,
    Struct,
}

/// What a value filled: a scalar, or the head of an array or map still to be read.
enum Fill {
    /// null or undefined: `fillNil`, a no-op for scalars and structs, nil for slices.
    Nil,
    Uint(u64),
    Int(i64),
    Bool(bool),
    Float(f64),
    String(String),
    Bytes(Vec<u8>),
    /// An array into a slice; the cursor is at the array head.
    Array,
    /// A map into a struct; the cursor is at the map head.
    Map,
}

impl<'a> Dec<'a> {
    /// A cursor over `data`, which must be well-formed ([`crate::well_formed`]).
    pub(crate) fn new(data: &'a [u8]) -> Dec<'a> {
        Dec { data, off: 0 }
    }

    /// The next initial byte (0 past the end, which validated input never reaches).
    fn cur(&self) -> u8 {
        self.data.get(self.off).copied().unwrap_or(0)
    }

    fn at_end(&self) -> bool {
        self.off >= self.data.len()
    }

    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.off)
    }

    pub fn peek_major(&self) -> u8 {
        self.cur() >> 5
    }

    /// `f6` / `f7` consumed → true.
    pub fn take_null(&mut self) -> bool {
        if !self.at_end() && matches!(self.cur(), 0xf6 | 0xf7) {
            self.off += 1;
            true
        } else {
            false
        }
    }

    /// fxamacker `getHead`: (major, additional information, argument).
    fn head(&mut self) -> (u8, u8, u64) {
        let first = self.cur();
        self.off = self.off.saturating_add(1);
        let ai = first & 0x1f;
        let size = match ai {
            24 => 1,
            25 => 2,
            26 => 4,
            27 => 8,
            _ => return (first >> 5, ai, u64::from(ai)),
        };
        let mut val = 0u64;
        for _ in 0..size {
            val = val << 8 | u64::from(self.cur());
            self.off = self.off.saturating_add(1);
        }
        (first >> 5, ai, val)
    }

    /// fxamacker `foundBreak`; the end of the input also ends a container.
    fn found_break(&mut self) -> bool {
        if self.at_end() {
            return true;
        }
        if self.cur() == 0xff {
            self.off += 1;
            return true;
        }
        false
    }

    /// The next `n` bytes of a string body.
    fn take(&mut self, n: u64) -> &'a [u8] {
        let start = self.off.min(self.data.len());
        let n = usize::try_from(n).unwrap_or(usize::MAX);
        let end = start.saturating_add(n).min(self.data.len());
        self.off = start.saturating_add(n);
        self.data.get(start..end).unwrap_or(&[])
    }

    /// fxamacker `parseByteString` after the head: definite, or chunks concatenated.
    fn byte_string_body(&mut self, ai: u8, val: u64) -> Vec<u8> {
        if ai != 31 {
            return self.take(val).to_vec();
        }
        let mut out = Vec::new();
        while !self.found_break() {
            let (_, _, n) = self.head();
            out.extend_from_slice(self.take(n));
        }
        out
    }

    /// fxamacker `parseTextString` after the head: UTF-8 checked per chunk.
    fn text_string_body(&mut self, ai: u8, val: u64) -> Result<String, DecodeError> {
        if ai != 31 {
            return match std::str::from_utf8(self.take(val)) {
                Ok(s) => Ok(s.to_owned()),
                Err(_) => Err(DecodeError::InvalidUtf8),
            };
        }
        let mut out = String::new();
        while !self.found_break() {
            let (_, _, n) = self.head();
            match std::str::from_utf8(self.take(n)) {
                Ok(s) => out.push_str(s),
                Err(_) => return Err(DecodeError::InvalidUtf8),
            }
        }
        Ok(out)
    }

    /// D2: strips leading self-described tags (55799), then checks every tag of the chain against the
    /// head of its immediate content without consuming the chain.
    pub fn tag_preamble(&mut self) -> Result<(), DecodeError> {
        while !self.at_end() && self.peek_major() == 6 {
            let save = self.off;
            let (_, _, num) = self.head();
            if num != TAG_SELF_DESCRIBED {
                self.off = save;
                break;
            }
        }
        let save = self.off;
        while !self.at_end() && self.peek_major() == 6 {
            let (_, _, num) = self.head();
            if let Err(e) = valid_builtin_tag(num, self.cur()) {
                self.off = save;
                return Err(e);
            }
        }
        self.off = save;
        Ok(())
    }

    /// fxamacker `parseToValue` for a non-pointer target of `kind`.
    fn value(&mut self, kind: Kind, go_type: &'static str) -> Result<Fill, DecodeError> {
        loop {
            self.tag_preamble()?;
            match self.peek_major() {
                0 => {
                    let (_, _, val) = self.head();
                    return fill_positive_int(CborType::PositiveInteger, val, kind, go_type);
                }
                1 => {
                    let (_, _, val) = self.head();
                    if val > i64::MAX as u64 {
                        return Err(DecodeError::type_detail(
                            CborType::NegativeInteger,
                            go_type,
                            format!("-{} overflows Go's int64", u128::from(val) + 1),
                        ));
                    }
                    return fill_negative_int(
                        CborType::NegativeInteger,
                        -1 - val as i64,
                        kind,
                        go_type,
                    );
                }
                2 => {
                    let (_, ai, val) = self.head();
                    let b = self.byte_string_body(ai, val);
                    return fill_byte_string(CborType::ByteString, b, kind, go_type);
                }
                3 => {
                    let (_, ai, val) = self.head();
                    let s = self.text_string_body(ai, val)?;
                    return fill_text_string(CborType::TextString, s, kind, go_type);
                }
                4 => {
                    return match kind {
                        Kind::Slice | Kind::Bytes => Ok(Fill::Array),
                        Kind::Struct => {
                            self.skip();
                            Err(DecodeError::type_detail(
                                CborType::Array,
                                go_type,
                                "cannot decode CBOR array to struct without toarray option"
                                    .to_owned(),
                            ))
                        }
                        _ => {
                            self.skip();
                            Err(DecodeError::type_error(CborType::Array, go_type))
                        }
                    };
                }
                5 => {
                    return match kind {
                        Kind::Struct => Ok(Fill::Map),
                        _ => {
                            self.skip();
                            Err(DecodeError::type_error(CborType::Map, go_type))
                        }
                    };
                }
                6 => {
                    let (_, _, num) = self.head();
                    if num == 2 || num == 3 {
                        return self.bignum(num == 3, kind, go_type);
                    }
                    // Any other tag: its content is decoded into the same target.
                }
                _ => {
                    let (_, ai, val) = self.head();
                    return match ai {
                        25 => {
                            let f = f32::from_bits(crate::enc::f16bits_to_f32bits(val as u16));
                            fill_float(CborType::Primitives, f64::from(f), kind, go_type)
                        }
                        26 => fill_float(
                            CborType::Primitives,
                            f64::from(f32::from_bits(val as u32)),
                            kind,
                            go_type,
                        ),
                        27 => fill_float(CborType::Primitives, f64::from_bits(val), kind, go_type),
                        _ => match val {
                            20 | 21 => fill_bool(CborType::Primitives, val == 21, kind, go_type),
                            22 | 23 => Ok(Fill::Nil),
                            _ => fill_positive_int(CborType::Primitives, val, kind, go_type),
                        },
                    };
                }
            }
        }
    }

    /// D3: tag 2 (`negative` false) or 3, the cursor after the tag head; the content is a byte string.
    fn bignum(
        &mut self,
        negative: bool,
        kind: Kind,
        go_type: &'static str,
    ) -> Result<Fill, DecodeError> {
        let (_, ai, val) = self.head();
        let content = self.byte_string_body(ai, val);
        if matches!(kind, Kind::Slice | Kind::Bytes) {
            return fill_byte_string(CborType::Tag, content, kind, go_type);
        }
        let magnitude = bignum_u64(&content);
        if !negative {
            if let Some(v) = magnitude {
                return fill_positive_int(CborType::Tag, v, kind, go_type);
            }
            return Err(DecodeError::type_detail(
                CborType::Tag,
                go_type,
                format!("{} overflows {go_type}", big_decimal(&content, false)),
            ));
        }
        match magnitude {
            Some(m) if m <= i64::MAX as u64 => {
                fill_negative_int(CborType::Tag, -1 - m as i64, kind, go_type)
            }
            _ => Err(DecodeError::type_detail(
                CborType::Tag,
                go_type,
                format!("-{} overflows {go_type}", big_decimal(&content, true)),
            )),
        }
    }

    /// D3/D4/D8.
    pub fn read_uint(&mut self, go_type: &'static str, max: u64) -> Result<u64, DecodeError> {
        match self.value(Kind::Uint(max), go_type)? {
            Fill::Uint(v) => Ok(v),
            _ => Ok(0),
        }
    }

    /// D3/D4/D5/D8.
    pub fn read_int(
        &mut self,
        go_type: &'static str,
        min: i64,
        max: i64,
    ) -> Result<i64, DecodeError> {
        match self.value(Kind::Int(min, max), go_type)? {
            Fill::Int(v) => Ok(v),
            _ => Ok(0),
        }
    }

    pub fn read_bool(&mut self, go_type: &'static str) -> Result<bool, DecodeError> {
        match self.value(Kind::Bool, go_type)? {
            Fill::Bool(v) => Ok(v),
            _ => Ok(false),
        }
    }

    pub fn read_f64(&mut self, go_type: &'static str) -> Result<f64, DecodeError> {
        match self.value(Kind::Float, go_type)? {
            Fill::Float(v) => Ok(v),
            _ => Ok(0.0),
        }
    }

    /// D7, chunks, UTF-8 check.
    pub fn read_text(&mut self, go_type: &'static str) -> Result<String, DecodeError> {
        match self.value(Kind::String, go_type)? {
            Fill::String(v) => Ok(v),
            _ => Ok(String::new()),
        }
    }

    /// D6, D9 (array of uint8), D3 bignum. A null leaves the slice empty.
    pub fn read_bytes(&mut self, go_type: &'static str) -> Result<Vec<u8>, DecodeError> {
        Ok(self.read_bytes_opt(go_type)?.unwrap_or_default())
    }

    /// [`Dec::read_bytes`] keeping Go's nil: null or undefined → `None`; a byte string, bignum or
    /// array → `Some`, even when empty.
    pub fn read_bytes_opt(
        &mut self,
        go_type: &'static str,
    ) -> Result<Option<Vec<u8>>, DecodeError> {
        match self.value(Kind::Bytes, go_type)? {
            Fill::Bytes(b) => Ok(Some(b)),
            Fill::Array => {
                let elem = elem_type(go_type);
                let v = self.array_items(|d| Ok(d.read_uint(elem, u64::from(u8::MAX))? as u8))?;
                Ok(Some(v))
            }
            _ => Ok(None),
        }
    }

    /// D9: element-wise into a slice. A null leaves the slice empty.
    pub fn read_array<T>(
        &mut self,
        go_type: &'static str,
        elem: impl FnMut(&mut Dec<'a>) -> Result<T, DecodeError>,
    ) -> Result<Vec<T>, DecodeError> {
        Ok(self.read_array_opt(go_type, elem)?.unwrap_or_default())
    }

    /// [`Dec::read_array`] keeping Go's nil: null or undefined → `None`; an array → `Some`, even
    /// when empty.
    pub fn read_array_opt<T>(
        &mut self,
        go_type: &'static str,
        elem: impl FnMut(&mut Dec<'a>) -> Result<T, DecodeError>,
    ) -> Result<Option<Vec<T>>, DecodeError> {
        match self.value(Kind::Slice, go_type)? {
            Fill::Array => Ok(Some(self.array_items(elem)?)),
            _ => Ok(None),
        }
    }

    /// fxamacker `parseArrayToSlice` from the array head.
    fn array_items<T>(
        &mut self,
        mut elem: impl FnMut(&mut Dec<'a>) -> Result<T, DecodeError>,
    ) -> Result<Vec<T>, DecodeError> {
        let (_, ai, val) = self.head();
        if ai == 31 {
            let mut out = Vec::new();
            while !self.found_break() {
                out.push(elem(self)?);
            }
            return Ok(out);
        }
        // Pass 1 capped the count at 131072, and every element takes at least one byte.
        let n = usize::try_from(val).unwrap_or(usize::MAX);
        let mut out = Vec::with_capacity(n.min(self.remaining()));
        for _ in 0..val {
            if self.at_end() {
                break;
            }
            out.push(elem(self)?);
        }
        Ok(out)
    }

    /// D10: key dispatch; `field` returns false for unknown keys (the value is skipped); duplicates of
    /// a matched key are skipped unexamined. Errors leaving a field are rewritten to
    /// "<go_type>.<key>". A null or undefined value decodes nothing.
    pub fn read_map_struct(
        &mut self,
        go_type: &'static str,
        mut field: impl FnMut(&mut Dec<'a>, u64) -> Result<bool, DecodeError>,
    ) -> Result<(), DecodeError> {
        match self.value(Kind::Struct, go_type)? {
            Fill::Map => {}
            _ => return Ok(()),
        }
        let (_, ai, count) = self.head();
        let indefinite = ai == 31;
        let mut left = count;
        let mut seen = Seen::default();
        loop {
            if indefinite {
                if self.found_break() {
                    break;
                }
            } else {
                if left == 0 || self.at_end() {
                    break;
                }
                left -= 1;
            }
            match self.peek_major() {
                0 => {
                    let (_, _, key) = self.head();
                    if key > i64::MAX as u64 {
                        return Err(DecodeError::MapKeyOverflow {
                            cbor_type: CborType::PositiveInteger,
                            detail: key.to_string(),
                        });
                    }
                    if seen.contains(key) {
                        self.skip();
                        continue;
                    }
                    match field(self, key) {
                        Ok(true) => seen.insert(key),
                        Ok(false) => self.skip(),
                        Err(e) => return Err(e.in_struct_field(go_type, key)),
                    }
                }
                1 => {
                    let (_, _, key) = self.head();
                    if key > i64::MAX as u64 {
                        return Err(DecodeError::MapKeyOverflow {
                            cbor_type: CborType::NegativeInteger,
                            detail: format!("-1-{key}"),
                        });
                    }
                    // Keyasint fields have non-negative keys: no match.
                    self.skip();
                }
                3 => {
                    // Parsed with the UTF-8 check, then matched by name: keyasint fields never match.
                    let (_, ai, val) = self.head();
                    self.text_string_body(ai, val)?;
                    self.skip();
                }
                major => {
                    return Err(DecodeError::MapKey {
                        key_type: CborType::of_initial_byte(major << 5),
                    });
                }
            }
        }
        Ok(())
    }

    /// fxamacker `skip`: one item, unexamined.
    pub fn skip(&mut self) {
        let (major, ai, val) = self.head();
        if ai == 31 && (2..=5).contains(&major) {
            while !self.found_break() {
                self.skip();
            }
            return;
        }
        match major {
            2 | 3 => {
                self.take(val);
            }
            4 | 5 => {
                let items = if major == 5 {
                    val.saturating_mul(2)
                } else {
                    val
                };
                for _ in 0..items {
                    if self.at_end() {
                        break;
                    }
                    self.skip();
                }
            }
            6 => self.skip(),
            _ => {}
        }
    }
}

/// `codec.Unmarshal`: pass 1, then pass 2; a top-level null gives `T::default()`.
pub fn unmarshal<T: Struct>(b: &[u8]) -> Result<T, DecodeError> {
    crate::well_formed(b)?;
    let mut d = Dec::new(b);
    T::decode_struct(&mut d)
}

/// The element Go type of a slice type ("[][]uint8" → "[]uint8").
pub(crate) fn elem_type(go_type: &'static str) -> &'static str {
    go_type.strip_prefix("[]").unwrap_or(go_type)
}

/// fxamacker `validBuiltinTag`.
fn valid_builtin_tag(num: u64, content_head: u8) -> Result<(), DecodeError> {
    let t = CborType::of_initial_byte(content_head);
    match num {
        0 if t != CborType::TextString => Err(DecodeError::BadTag(format!(
            "cbor: tag number 0 must be followed by text string, got {t}"
        ))),
        1 if t != CborType::PositiveInteger
            && t != CborType::NegativeInteger
            && !(0xf9..=0xfb).contains(&content_head) =>
        {
            Err(DecodeError::BadTag(format!(
                "cbor: tag number 1 must be followed by integer or floating-point number, got {t}"
            )))
        }
        2 | 3 if t != CborType::ByteString => Err(DecodeError::BadTag(format!(
            "cbor: tag number 2 or 3 must be followed by byte string, got {t}"
        ))),
        _ => Ok(()),
    }
}

/// fxamacker `fillPositiveInt`.
fn fill_positive_int(
    t: CborType,
    val: u64,
    kind: Kind,
    go_type: &str,
) -> Result<Fill, DecodeError> {
    match kind {
        Kind::Int(_, max) => {
            if val > i64::MAX as u64 || val as i64 > max {
                return Err(DecodeError::type_detail(
                    t,
                    go_type,
                    format!("{val} overflows {go_type}"),
                ));
            }
            Ok(Fill::Int(val as i64))
        }
        Kind::Uint(max) => {
            if val > max {
                return Err(DecodeError::type_detail(
                    t,
                    go_type,
                    format!("{val} overflows {go_type}"),
                ));
            }
            Ok(Fill::Uint(val))
        }
        Kind::Float => Ok(Fill::Float(val as f64)),
        _ => Err(DecodeError::type_error(t, go_type)),
    }
}

/// fxamacker `fillNegativeInt`.
fn fill_negative_int(
    t: CborType,
    val: i64,
    kind: Kind,
    go_type: &str,
) -> Result<Fill, DecodeError> {
    match kind {
        Kind::Int(min, max) => {
            if val < min || val > max {
                return Err(DecodeError::type_detail(
                    t,
                    go_type,
                    format!("{val} overflows {go_type}"),
                ));
            }
            Ok(Fill::Int(val))
        }
        Kind::Float => Ok(Fill::Float(val as f64)),
        _ => Err(DecodeError::type_error(t, go_type)),
    }
}

/// fxamacker `fillBool`.
fn fill_bool(t: CborType, val: bool, kind: Kind, go_type: &str) -> Result<Fill, DecodeError> {
    match kind {
        Kind::Bool => Ok(Fill::Bool(val)),
        _ => Err(DecodeError::type_error(t, go_type)),
    }
}

/// fxamacker `fillFloat` (a float64 never overflows).
fn fill_float(t: CborType, val: f64, kind: Kind, go_type: &str) -> Result<Fill, DecodeError> {
    match kind {
        Kind::Float => Ok(Fill::Float(val)),
        _ => Err(DecodeError::type_error(t, go_type)),
    }
}

/// fxamacker `fillByteString` with `ByteStringToStringForbidden`: only `[]uint8` accepts bytes.
fn fill_byte_string(
    t: CborType,
    val: Vec<u8>,
    kind: Kind,
    go_type: &str,
) -> Result<Fill, DecodeError> {
    match kind {
        Kind::Bytes => Ok(Fill::Bytes(val)),
        _ => Err(DecodeError::type_error(t, go_type)),
    }
}

/// fxamacker `fillTextString`.
fn fill_text_string(
    t: CborType,
    val: String,
    kind: Kind,
    go_type: &str,
) -> Result<Fill, DecodeError> {
    match kind {
        Kind::String => Ok(Fill::String(val)),
        _ => Err(DecodeError::type_error(t, go_type)),
    }
}

/// The magnitude of a bignum's big-endian content, if it fits 64 bits (`big.Int.IsUint64`).
fn bignum_u64(content: &[u8]) -> Option<u64> {
    let digits = match content.iter().position(|&b| b != 0) {
        Some(first) => &content[first..],
        None => return Some(0),
    };
    if digits.len() > 8 {
        return None;
    }
    Some(digits.iter().fold(0u64, |v, &b| v << 8 | u64::from(b)))
}

/// The decimal digits of a big-endian magnitude, plus one when `plus_one` (Go `big.Int.String` of
/// `SetBytes(content)`, or of its negation minus one without the sign).
///
/// A peer can put megabytes of bignum into an integer field, and Go renders the text in
/// subquadratic time (0.4 s for 1 MiB). Repeated division by 10^9 is quadratic (96 s for 1 MiB),
/// so large numbers are split recursively and the halves recombined with Karatsuba multiplication
/// in base 10^9.
fn big_decimal(content: &[u8], plus_one: bool) -> String {
    use std::fmt::Write as _;

    // Little-endian base-2^32 limbs.
    let mut limbs: Vec<u32> = content
        .rchunks(4)
        .map(|c| c.iter().fold(0u32, |v, &b| v << 8 | u32::from(b)))
        .collect();
    if plus_one {
        let mut carry = true;
        for limb in limbs.iter_mut() {
            if !carry {
                break;
            }
            let (v, overflow) = limb.overflowing_add(1);
            *limb = v;
            carry = overflow;
        }
        if carry {
            limbs.push(1);
        }
    }
    let digits = to_decimal_limbs(&limbs, &mut Vec::new());
    let Some((top, rest)) = trimmed(&digits).split_last() else {
        return "0".to_owned();
    };
    let mut s = top.to_string();
    for chunk in rest.iter().rev() {
        // Writing to a String cannot fail.
        let _ = write!(s, "{chunk:09}");
    }
    s
}

/// The base of the decimal limbs [`big_decimal`] computes.
const DEC_BASE: u32 = 1_000_000_000;
/// Binary numbers of at most this many limbs convert by repeated division.
const CONVERT_LEAF_LIMBS: usize = 64;
/// Products with an operand of fewer decimal limbs use schoolbook multiplication.
const KARATSUBA_MIN_LIMBS: usize = 32;

/// `v` without its most significant zero limbs.
fn trimmed(v: &[u32]) -> &[u32] {
    let end = v.iter().rposition(|&x| x != 0).map_or(0, |i| i + 1);
    &v[..end]
}

/// The little-endian base-10^9 limbs of the little-endian base-2^32 number `limbs`. `pows` caches
/// 2^(32·2^j) in base 10^9.
fn to_decimal_limbs(limbs: &[u32], pows: &mut Vec<Vec<u32>>) -> Vec<u32> {
    let limbs = trimmed(limbs);
    if limbs.len() <= CONVERT_LEAF_LIMBS {
        return to_decimal_limbs_by_division(limbs);
    }
    // limbs = hi·2^(32·m) + lo, with m the largest power of two below the length.
    let j = (usize::BITS - 1 - (limbs.len() - 1).leading_zeros()) as usize;
    let (lo, hi) = limbs.split_at(1 << j);
    let lo = to_decimal_limbs(lo, pows);
    let hi = to_decimal_limbs(hi, pows);
    let mut out = decimal_mul(&hi, power_of_two_32(pows, j));
    decimal_add_at(&mut out, &lo, 0);
    out
}

/// 2^(32·2^j) in base 10^9, by repeated squaring.
fn power_of_two_32(pows: &mut Vec<Vec<u32>>, j: usize) -> &[u32] {
    if pows.is_empty() {
        // 2^32 = 4·10^9 + 294967296.
        pows.push(vec![294_967_296, 4]);
    }
    while pows.len() <= j {
        let Some(p) = pows.last() else { break };
        let square = trimmed(&decimal_mul(p, p)).to_vec();
        pows.push(square);
    }
    pows.get(j).map_or(&[], Vec::as_slice)
}

/// Repeated division by 10^9 (quadratic; for small numbers and as the test oracle).
fn to_decimal_limbs_by_division(limbs: &[u32]) -> Vec<u32> {
    let base = u64::from(DEC_BASE);
    let mut limbs = trimmed(limbs).to_vec();
    let mut out = Vec::new();
    while !limbs.is_empty() {
        let mut rem = 0u64;
        for limb in limbs.iter_mut().rev() {
            let cur = rem << 32 | u64::from(*limb);
            *limb = (cur / base) as u32;
            rem = cur % base;
        }
        out.push(rem as u32);
        while limbs.last() == Some(&0) {
            limbs.pop();
        }
    }
    out
}

/// `acc += x·10^(9·shift)` in base 10^9.
fn decimal_add_at(acc: &mut Vec<u32>, x: &[u32], shift: usize) {
    let x = trimmed(x);
    if x.is_empty() {
        return;
    }
    let end = shift + x.len();
    if acc.len() < end {
        acc.resize(end, 0);
    }
    let mut carry = 0u32;
    for (a, &d) in acc[shift..end].iter_mut().zip(x) {
        let s = *a + d + carry;
        carry = u32::from(s >= DEC_BASE);
        *a = s - carry * DEC_BASE;
    }
    for a in acc[end..].iter_mut() {
        if carry == 0 {
            break;
        }
        let s = *a + carry;
        carry = u32::from(s >= DEC_BASE);
        *a = s - carry * DEC_BASE;
    }
    if carry != 0 {
        acc.push(carry);
    }
}

/// `acc -= x` in base 10^9, where `acc >= x`.
fn decimal_sub(acc: &mut [u32], x: &[u32]) {
    let mut borrow = 0u32;
    let mut rest = acc.iter_mut();
    for (&d, a) in trimmed(x).iter().zip(rest.by_ref()) {
        let sub = d + borrow;
        borrow = u32::from(*a < sub);
        *a = *a + borrow * DEC_BASE - sub;
    }
    for a in rest {
        if borrow == 0 {
            break;
        }
        borrow = u32::from(*a == 0);
        *a = *a + borrow * DEC_BASE - 1;
    }
}

/// The schoolbook product of base-10^9 numbers.
fn decimal_mul_schoolbook(a: &[u32], b: &[u32]) -> Vec<u32> {
    let base = u64::from(DEC_BASE);
    let mut out = vec![0u32; a.len() + b.len()];
    for (i, &x) in a.iter().enumerate() {
        if x == 0 {
            continue;
        }
        let mut carry = 0u64;
        let mut row = out[i..].iter_mut();
        for (&y, o) in b.iter().zip(row.by_ref()) {
            let t = u64::from(*o) + u64::from(x) * u64::from(y) + carry;
            *o = (t % base) as u32;
            carry = t / base;
        }
        for o in row {
            if carry == 0 {
                break;
            }
            let t = u64::from(*o) + carry;
            *o = (t % base) as u32;
            carry = t / base;
        }
    }
    out
}

/// The product of base-10^9 numbers: Karatsuba down to [`KARATSUBA_MIN_LIMBS`]. The result may
/// have most significant zero limbs.
fn decimal_mul(a: &[u32], b: &[u32]) -> Vec<u32> {
    let (a, b) = (trimmed(a), trimmed(b));
    if a.len().min(b.len()) < KARATSUBA_MIN_LIMBS {
        return decimal_mul_schoolbook(a, b);
    }
    let k = a.len().max(b.len()) / 2;
    let (a0, a1) = a.split_at(k.min(a.len()));
    let (b0, b1) = b.split_at(k.min(b.len()));
    let z0 = decimal_mul(a0, b0);
    let z2 = decimal_mul(a1, b1);
    let mut sa = a0.to_vec();
    decimal_add_at(&mut sa, a1, 0);
    let mut sb = b0.to_vec();
    decimal_add_at(&mut sb, b1, 0);
    // z1 = (a0 + a1)(b0 + b1) - z0 - z2 = a0·b1 + a1·b0.
    let mut z1 = decimal_mul(&sa, &sb);
    decimal_sub(&mut z1, &z0);
    decimal_sub(&mut z1, &z2);
    let mut out = z0;
    decimal_add_at(&mut out, &z1, k);
    decimal_add_at(&mut out, &z2, 2 * k);
    out
}

/// The matched keys of one map (fxamacker `foundFldIdx`).
#[derive(Default)]
struct Seen {
    low: u128,
    high: Vec<u64>,
}

impl Seen {
    fn contains(&self, key: u64) -> bool {
        if key < 128 {
            self.low & (1u128 << key) != 0
        } else {
            self.high.contains(&key)
        }
    }

    fn insert(&mut self, key: u64) {
        if key < 128 {
            self.low |= 1u128 << key;
        } else if !self.high.contains(&key) {
            self.high.push(key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn big_decimal_matches_go() {
        assert_eq!(big_decimal(&[], false), "0");
        assert_eq!(big_decimal(&[0, 0], false), "0");
        assert_eq!(big_decimal(&[], true), "1");
        assert_eq!(big_decimal(&[5], false), "5");
        assert_eq!(
            big_decimal(&[1, 0, 0, 0, 0, 0, 0, 0, 0], false),
            "18446744073709551616"
        );
        assert_eq!(big_decimal(&[0xff; 8], true), "18446744073709551616");
        assert_eq!(
            big_decimal(&[0x80, 0, 0, 0, 0, 0, 0, 0], true),
            "9223372036854775809"
        );
        // 2^128 = 340282366920938463463374607431768211456
        let mut b = vec![1u8];
        b.extend([0u8; 16]);
        assert_eq!(
            big_decimal(&b, false),
            "340282366920938463463374607431768211456"
        );
        assert_eq!(big_decimal(&[0x3b, 0x9a, 0xca, 0x00], false), "1000000000");
        // 2^2080 (66 limbs) takes the recursive path; digits from Python's int.
        let mut b = vec![1u8];
        b.extend([0u8; 260]);
        let s = big_decimal(&b, false);
        assert_eq!(s.len(), 627);
        assert!(
            s.starts_with("13880048418091422020") && s.ends_with("64469717632392626176"),
            "{s}"
        );
        // 2^2080 - 1 + 1 carries through every limb.
        assert_eq!(big_decimal(&[0xff; 260], true), s);
    }

    /// A deterministic pseudo-random stream for the oracle tests.
    fn xorshift(state: &mut u64) -> u64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    }

    #[test]
    fn decimal_conversion_matches_division() {
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        let mut pows = Vec::new();
        let sizes = (0..=140).chain([255, 256, 257, 300, 511, 512, 513, 1000, 1025, 2048, 3000]);
        for n in sizes {
            for pattern in 0..4 {
                let limbs: Vec<u32> = (0..n)
                    .map(|i| match pattern {
                        0 => xorshift(&mut state) as u32,
                        1 => u32::MAX,
                        // A power of two.
                        2 => u32::from(i + 1 == n),
                        _ if i % 7 == 3 => xorshift(&mut state) as u32,
                        _ => 0,
                    })
                    .collect();
                assert_eq!(
                    trimmed(&to_decimal_limbs(&limbs, &mut pows)),
                    trimmed(&to_decimal_limbs_by_division(&limbs)),
                    "{n} limbs, pattern {pattern}"
                );
            }
        }
    }

    #[test]
    fn decimal_karatsuba_matches_schoolbook() {
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut digits = |n: usize, max: bool| -> Vec<u32> {
            (0..n)
                .map(|_| {
                    if max {
                        DEC_BASE - 1
                    } else {
                        (xorshift(&mut state) % u64::from(DEC_BASE)) as u32
                    }
                })
                .collect()
        };
        let shapes = [
            (0, 40),
            (1, 900),
            (31, 500),
            (32, 32),
            (32, 500),
            (33, 32),
            (63, 500),
            (64, 64),
            (64, 1000),
            (65, 64),
            (65, 100),
            (96, 97),
            (127, 128),
            (200, 1000),
            (500, 33),
            (1000, 1000),
        ];
        for (na, nb) in shapes {
            for max in [false, true] {
                let a = digits(na, max);
                let b = digits(nb, max);
                assert_eq!(
                    trimmed(&decimal_mul(&a, &b)),
                    trimmed(&decimal_mul_schoolbook(&a, &b)),
                    "{na}x{nb}, max {max}"
                );
            }
        }
    }

    #[test]
    fn bignum_magnitude() {
        assert_eq!(bignum_u64(&[]), Some(0));
        assert_eq!(bignum_u64(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 5]), Some(5));
        assert_eq!(bignum_u64(&[0xff; 8]), Some(u64::MAX));
        assert_eq!(bignum_u64(&[1, 0, 0, 0, 0, 0, 0, 0, 0]), None);
    }

    #[test]
    fn seen_keys() {
        let mut s = Seen::default();
        for k in [0u64, 127, 128, 1 << 40] {
            assert!(!s.contains(k));
            s.insert(k);
            assert!(s.contains(k));
        }
        assert!(!s.contains(1));
    }

    #[test]
    fn element_types() {
        assert_eq!(elem_type("[][]uint8"), "[]uint8");
        assert_eq!(elem_type("[]wire.RefInfo"), "wire.RefInfo");
        assert_eq!(elem_type("uint8"), "uint8");
    }

    #[test]
    fn cursor_primitives() {
        let data = [0xd9, 0xd9, 0xf7, 0xc6, 0x61, 0x61];
        let mut d = Dec::new(&data);
        assert_eq!(d.peek_major(), 6);
        assert_eq!(d.tag_preamble(), Ok(()));
        // The self-described tag is consumed; tag 6 stays.
        assert_eq!(d.off, 3);
        assert_eq!(d.read_text("string"), Ok("a".to_owned()));
        assert!(d.at_end());

        let data = [0xf7, 0xf6, 0x00];
        let mut d = Dec::new(&data);
        assert!(d.take_null());
        assert!(d.take_null());
        assert!(!d.take_null());
        assert_eq!(d.read_uint("uint8", 255), Ok(0));

        let data = [0xc0, 0x00];
        let mut d = Dec::new(&data);
        assert_eq!(
            d.tag_preamble(),
            Err(DecodeError::BadTag(
                "cbor: tag number 0 must be followed by text string, got positive integer"
                    .to_owned()
            ))
        );
        assert_eq!(d.off, 0);

        // skip over indefinite and nested items.
        let data = [0xbf, 0x01, 0x9f, 0x5f, 0x41, 0x00, 0xff, 0xff, 0xff, 0x07];
        let mut d = Dec::new(&data);
        d.skip();
        assert_eq!(d.read_uint("uint64", u64::MAX), Ok(7));
    }
}
