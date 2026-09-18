//! `strconv.ParseBool/ParseInt/ParseUint/ParseFloat` and `FormatFloat(f, 'g', -1, 64)`, go1.26.5.
//!
//! A line-by-line port of go1.26.5 `src/internal/strconv` (`atob.go`, `atoi.go`, `atof.go`,
//! `atofeisel.go`, `decimal.go`, `ftoa.go`, `ftoadbox.go`, `math.go`, `pow10tab.go`) and the error
//! wrapping of `src/strconv/number.go`. Parsing keeps Go's quirks: the exponent accumulator stops at
//! five digits, and the slow decimal path keeps at most 800 digits. Formatting uses Go's Dragonbox
//! port, which breaks exact ties to even where Rust's shortest formatting rounds up.

/// The two `strconv` error classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumErrorKind {
    /// `strconv.ErrSyntax`: "invalid syntax".
    Syntax,
    /// `strconv.ErrRange`: "value out of range".
    Range,
}

impl NumErrorKind {
    /// The Go text of the class.
    pub fn text(&self) -> &'static str {
        match self {
            NumErrorKind::Syntax => "invalid syntax",
            NumErrorKind::Range => "value out of range",
        }
    }
}

/// `*strconv.NumError`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("strconv.{func}: parsing {}: {}", crate::quote::quote(.num.as_bytes()), .kind.text())]
pub struct NumError {
    pub func: &'static str,
    pub num: String,
    pub kind: NumErrorKind,
}

fn num_error(func: &'static str, s: &str, kind: NumErrorKind) -> NumError {
    NumError {
        func,
        num: s.to_owned(),
        kind,
    }
}

/// `lower(c)`: a lower-case letter iff `c` is that letter or its upper-case form.
fn lower(c: u8) -> u8 {
    c | (b'x' - b'X')
}

/// `strconv.ParseBool`.
pub fn parse_bool(s: &str) -> Result<bool, NumError> {
    match s {
        "1" | "t" | "T" | "true" | "TRUE" | "True" => Ok(true),
        "0" | "f" | "F" | "false" | "FALSE" | "False" => Ok(false),
        _ => Err(num_error("ParseBool", s, NumErrorKind::Syntax)),
    }
}

/// `strconv.ParseInt`; base 0 follows the `0x`/`0o`/`0b`/`0` prefixes and the `_` rules.
///
/// # Panics
///
/// When `base` is 1 or above 36, or `bit_size` is above 64. Go returns a `NumError` reading
/// `invalid base N` / `invalid bit size N` there, which `NumErrorKind` cannot carry; every dstore call
/// site passes a constant base and bit size.
pub fn parse_int(s: &str, base: u32, bit_size: u32) -> Result<i64, NumError> {
    parse_int_raw(s.as_bytes(), base, bit_size).map_err(|kind| num_error("ParseInt", s, kind))
}

/// `strconv.ParseUint`.
///
/// # Panics
///
/// As [`parse_int`], for an invalid `base` or `bit_size`.
pub fn parse_uint(s: &str, base: u32, bit_size: u32) -> Result<u64, NumError> {
    parse_uint_raw(s.as_bytes(), base, bit_size)
        .map_err(|(kind, _)| num_error("ParseUint", s, kind))
}

/// `internal/strconv.ParseUint`: on a range error the maximum value is returned with the kind.
fn parse_uint_raw(s: &[u8], base: u32, bit_size: u32) -> Result<u64, (NumErrorKind, u64)> {
    if s.is_empty() {
        return Err((NumErrorKind::Syntax, 0));
    }
    let base0 = base == 0;
    let s0 = s;
    let (base, digits) = match base {
        2..=36 => (base, s),
        0 => {
            if s[0] == b'0' {
                if s.len() >= 3 && lower(s[1]) == b'b' {
                    (2, &s[2..])
                } else if s.len() >= 3 && lower(s[1]) == b'o' {
                    (8, &s[2..])
                } else if s.len() >= 3 && lower(s[1]) == b'x' {
                    (16, &s[2..])
                } else {
                    (8, &s[1..])
                }
            } else {
                (10, s)
            }
        }
        _ => panic!("strconv: invalid base {base}"),
    };
    let bit_size = match bit_size {
        0 => 64,
        1..=64 => bit_size,
        _ => panic!("strconv: invalid bit size {bit_size}"),
    };

    // Cutoff is the smallest number such that cutoff*base > maxUint64.
    let cutoff = u64::MAX / u64::from(base) + 1;
    let max_val = if bit_size == 64 {
        u64::MAX
    } else {
        (1u64 << bit_size) - 1
    };

    let mut underscores = false;
    let mut n: u64 = 0;
    for &c in digits {
        let d = if c == b'_' && base0 {
            underscores = true;
            continue;
        } else if c.is_ascii_digit() {
            c - b'0'
        } else if lower(c).is_ascii_lowercase() {
            lower(c) - b'a' + 10
        } else {
            return Err((NumErrorKind::Syntax, 0));
        };
        if u32::from(d) >= base {
            return Err((NumErrorKind::Syntax, 0));
        }
        if n >= cutoff {
            // n*base overflows
            return Err((NumErrorKind::Range, max_val));
        }
        n *= u64::from(base);
        let n1 = n.wrapping_add(u64::from(d));
        if n1 < n || n1 > max_val {
            // n+d overflows
            return Err((NumErrorKind::Range, max_val));
        }
        n = n1;
    }
    if underscores && !underscore_ok(s0) {
        return Err((NumErrorKind::Syntax, 0));
    }
    Ok(n)
}

/// `internal/strconv.ParseInt`.
fn parse_int_raw(s: &[u8], base: u32, bit_size: u32) -> Result<i64, NumErrorKind> {
    if s.is_empty() {
        return Err(NumErrorKind::Syntax);
    }
    let (neg, rest) = match s[0] {
        b'+' => (false, &s[1..]),
        b'-' => (true, &s[1..]),
        _ => (false, s),
    };
    let un = match parse_uint_raw(rest, base, bit_size) {
        Ok(v) => v,
        Err((NumErrorKind::Range, v)) => v,
        Err((kind, _)) => return Err(kind),
    };
    let bit_size = if bit_size == 0 { 64 } else { bit_size };
    let cutoff = 1u64 << (bit_size - 1);
    if !neg && un >= cutoff {
        return Err(NumErrorKind::Range);
    }
    if neg && un > cutoff {
        return Err(NumErrorKind::Range);
    }
    let n = un as i64;
    Ok(if neg { n.wrapping_neg() } else { n })
}

/// `underscoreOK`: underscores only between digits, or between a base prefix and a digit.
fn underscore_ok(s: &[u8]) -> bool {
    // saw: b'^' beginning, b'0' digit or base prefix, b'_' underscore, b'!' anything else.
    let mut saw = b'^';
    let mut i = 0;
    let s = match s.first() {
        Some(b'-') | Some(b'+') => &s[1..],
        _ => s,
    };
    let mut hex = false;
    if s.len() >= 2 && s[0] == b'0' && matches!(lower(s[1]), b'b' | b'o' | b'x') {
        i = 2;
        saw = b'0'; // base prefix counts as a digit for "underscore as digit separator"
        hex = lower(s[1]) == b'x';
    }
    while i < s.len() {
        let c = s[i];
        i += 1;
        if c.is_ascii_digit() || (hex && (b'a'..=b'f').contains(&lower(c))) {
            saw = b'0';
            continue;
        }
        if c == b'_' {
            if saw != b'0' {
                return false;
            }
            saw = b'_';
            continue;
        }
        if saw == b'_' {
            return false;
        }
        saw = b'!';
    }
    saw != b'_'
}

/// `strconv.ParseFloat(s, 64)`: hex floats, inf/nan, `_` rules.
pub fn parse_float(s: &str) -> Result<f64, NumError> {
    let b = s.as_bytes();
    let (f, n, err) = atof64(b);
    if n != b.len() {
        return Err(num_error("ParseFloat", s, NumErrorKind::Syntax));
    }
    match err {
        Some(kind) => Err(num_error("ParseFloat", s, kind)),
        None => Ok(f),
    }
}

const FLOAT64_MANT_BITS: u32 = 52;
const FLOAT64_EXP_BITS: u32 = 11;
const FLOAT64_BIAS: i64 = -1023;

/// Go's `nan()`: `0x7FF8000000000001`.
const GO_NAN_BITS: u64 = 0x7FF8_0000_0000_0001;

/// `commonPrefixLenIgnoreCase`; `prefix` is lower-case.
fn common_prefix_len_ignore_case(s: &[u8], prefix: &[u8]) -> usize {
    let n = s.len().min(prefix.len());
    for i in 0..n {
        if s[i].to_ascii_lowercase() != prefix[i] {
            return i;
        }
    }
    n
}

/// `special`: inf, infinity (optionally signed) and nan, as a prefix of `s`.
fn special(s: &[u8]) -> Option<(f64, usize)> {
    let first = *s.first()?;
    match first {
        b'+' | b'-' | b'i' | b'I' => {
            let (sign_neg, nsign, rest) = match first {
                b'+' => (false, 1, &s[1..]),
                b'-' => (true, 1, &s[1..]),
                _ => (false, 0, s),
            };
            let mut n = common_prefix_len_ignore_case(rest, b"infinity");
            // Anything longer than "inf" is ok, but if we don't have "infinity", only consume "inf".
            if 3 < n && n < 8 {
                n = 3;
            }
            if n == 3 || n == 8 {
                let f = if sign_neg {
                    f64::NEG_INFINITY
                } else {
                    f64::INFINITY
                };
                return Some((f, nsign + n));
            }
            None
        }
        b'n' | b'N' => {
            if common_prefix_len_ignore_case(s, b"nan") == 3 {
                return Some((f64::from_bits(GO_NAN_BITS), 3));
            }
            None
        }
        _ => None,
    }
}

/// The result of `readFloat`.
#[derive(Default)]
struct ReadFloat {
    mantissa: u64,
    exp: i64,
    neg: bool,
    trunc: bool,
    hex: bool,
    n: usize,
    ok: bool,
}

/// `readFloat`: a decimal or hexadecimal mantissa and exponent from a prefix of `s`.
fn read_float(s: &[u8]) -> ReadFloat {
    let mut r = ReadFloat::default();
    let mut underscores = false;
    let mut i = 0;

    // optional sign
    if i >= s.len() {
        return r;
    }
    match s[i] {
        b'+' => i += 1,
        b'-' => {
            i += 1;
            r.neg = true;
        }
        _ => {}
    }

    // digits
    let mut base: u64 = 10;
    let mut max_mant_digits = 19; // 10^19 fits in uint64
    let mut exp_char = b'e';
    if i + 2 < s.len() && s[i] == b'0' && lower(s[i + 1]) == b'x' {
        base = 16;
        max_mant_digits = 16; // 16^16 fits in uint64
        i += 2;
        exp_char = b'p';
        r.hex = true;
    }
    let mut sawdot = false;
    let mut sawdigits = false;
    let mut nd: i64 = 0;
    let mut nd_mant: i64 = 0;
    let mut dp: i64 = 0;
    while i < s.len() {
        let c = s[i];
        if c == b'_' {
            underscores = true;
        } else if c == b'.' {
            if sawdot {
                break;
            }
            sawdot = true;
            dp = nd;
        } else if c.is_ascii_digit() {
            sawdigits = true;
            if c == b'0' && nd == 0 {
                // ignore leading zeros
                dp -= 1;
            } else {
                nd += 1;
                if nd_mant < max_mant_digits {
                    r.mantissa = r
                        .mantissa
                        .wrapping_mul(base)
                        .wrapping_add(u64::from(c - b'0'));
                    nd_mant += 1;
                } else if c != b'0' {
                    r.trunc = true;
                }
            }
        } else if base == 16 && (b'a'..=b'f').contains(&lower(c)) {
            sawdigits = true;
            nd += 1;
            if nd_mant < max_mant_digits {
                r.mantissa = r
                    .mantissa
                    .wrapping_mul(16)
                    .wrapping_add(u64::from(lower(c) - b'a' + 10));
                nd_mant += 1;
            } else {
                r.trunc = true;
            }
        } else {
            break;
        }
        i += 1;
    }
    if !sawdigits {
        r.n = i;
        return r;
    }
    if !sawdot {
        dp = nd;
    }
    if base == 16 {
        dp *= 4;
        nd_mant *= 4;
    }

    // optional exponent moves decimal point.
    if i < s.len() && lower(s[i]) == exp_char {
        i += 1;
        if i >= s.len() {
            r.n = i;
            return r;
        }
        let mut esign = 1;
        match s[i] {
            b'+' => i += 1,
            b'-' => {
                i += 1;
                esign = -1;
            }
            _ => {}
        }
        if i >= s.len() || !s[i].is_ascii_digit() {
            r.n = i;
            return r;
        }
        let mut e: i64 = 0;
        while i < s.len() && (s[i].is_ascii_digit() || s[i] == b'_') {
            if s[i] == b'_' {
                underscores = true;
            } else if e < 10000 {
                e = e * 10 + i64::from(s[i] - b'0');
            }
            i += 1;
        }
        dp += e * esign;
    } else if base == 16 {
        // Must have exponent.
        r.n = i;
        return r;
    }

    if r.mantissa != 0 {
        r.exp = dp - nd_mant;
    }
    r.n = i;
    if underscores && !underscore_ok(&s[..i]) {
        return r;
    }
    r.ok = true;
    r
}

/// `atof64`: the value, the number of bytes consumed, and the error class.
fn atof64(s: &[u8]) -> (f64, usize, Option<NumErrorKind>) {
    if let Some((val, n)) = special(s) {
        return (val, n, None);
    }
    let r = read_float(s);
    if !r.ok {
        return (0.0, r.n, Some(NumErrorKind::Syntax));
    }
    if r.hex {
        let (f, err) = atof_hex(r.mantissa, r.exp, r.neg, r.trunc);
        return (f, r.n, err);
    }
    // Try pure floating-point arithmetic conversion, and if that fails, the Eisel-Lemire algorithm.
    if !r.trunc
        && let Some(f) = atof64_exact(r.mantissa, r.exp, r.neg)
    {
        return (f, r.n, None);
    }
    if let Some(f) = eisel_lemire64(r.mantissa, r.exp, r.neg) {
        if !r.trunc {
            return (f, r.n, None);
        }
        // Even if the mantissa was truncated, we may have found the correct result. Confirm by
        // converting the upper mantissa bound.
        if let Some(f_up) = eisel_lemire64(r.mantissa.wrapping_add(1), r.exp, r.neg)
            && f == f_up
        {
            return (f, r.n, None);
        }
    }
    // Slow fallback.
    let mut d = Decimal::new();
    if !d.set(&s[..r.n]) {
        return (0.0, r.n, Some(NumErrorKind::Syntax));
    }
    let (bits, overflow) = d.float_bits();
    let err = if overflow {
        Some(NumErrorKind::Range)
    } else {
        None
    };
    (f64::from_bits(bits), r.n, err)
}

/// Exact powers of 10.
const FLOAT64_POW10: [f64; 23] = [
    1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10, 1e11, 1e12, 1e13, 1e14, 1e15, 1e16,
    1e17, 1e18, 1e19, 1e20, 1e21, 1e22,
];

/// `atof64exact`: exact integer, integer * 10^k, or integer / 10^k in floating-point math.
fn atof64_exact(mantissa: u64, exp: i64, neg: bool) -> Option<f64> {
    if mantissa >> FLOAT64_MANT_BITS != 0 {
        return None;
    }
    let mut f = mantissa as f64;
    if neg {
        f = -f;
    }
    if exp == 0 {
        // an integer.
        return Some(f);
    }
    // Exact integers are <= 10^15. Exact powers of ten are <= 10^22.
    if exp > 0 && exp <= 15 + 22 {
        let mut exp = exp;
        // If exponent is big but number of digits is not, can move a few zeros into the integer part.
        if exp > 22 {
            f *= FLOAT64_POW10[(exp - 22) as usize];
            exp = 22;
        }
        if !(-1e15..=1e15).contains(&f) {
            // the exponent was really too large.
            return None;
        }
        return Some(f * FLOAT64_POW10[exp as usize]);
    }
    if (-22..0).contains(&exp) {
        return Some(f / FLOAT64_POW10[(-exp) as usize]);
    }
    None
}

/// `eiselLemire64`.
fn eisel_lemire64(man: u64, exp10: i64, neg: bool) -> Option<f64> {
    // Exp10 Range.
    if man == 0 {
        return Some(if neg { -0.0 } else { 0.0 });
    }
    let (pow_hi, pow_lo, exp2) = pow10(exp10)?;

    // Normalization.
    let clz = man.leading_zeros();
    let man = man << clz;
    let mut ret_exp2 = (exp2 + 63 - FLOAT64_BIAS) as u64;
    ret_exp2 = ret_exp2.wrapping_sub(u64::from(clz));

    // Multiplication.
    let (mut x_hi, mut x_lo) = mul64(man, pow_hi);

    // Wider Approximation.
    if x_hi & 0x1FF == 0x1FF && x_lo.wrapping_add(man) < man {
        let (y_hi, y_lo) = mul64(man, pow_lo);
        let mut merged_hi = x_hi;
        let merged_lo = x_lo.wrapping_add(y_hi);
        if merged_lo < x_lo {
            merged_hi = merged_hi.wrapping_add(1);
        }
        if merged_hi & 0x1FF == 0x1FF
            && merged_lo.wrapping_add(1) == 0
            && y_lo.wrapping_add(man) < man
        {
            return None;
        }
        x_hi = merged_hi;
        x_lo = merged_lo;
    }

    // Shifting to 54 Bits.
    let msb = x_hi >> 63;
    let mut ret_mantissa = x_hi >> (msb + 9);
    ret_exp2 = ret_exp2.wrapping_sub(1 ^ msb);

    // Half-way Ambiguity.
    if x_lo == 0 && x_hi & 0x1FF == 0 && ret_mantissa & 3 == 1 {
        return None;
    }

    // From 54 to 53 Bits.
    ret_mantissa += ret_mantissa & 1;
    ret_mantissa >>= 1;
    if ret_mantissa >> 53 > 0 {
        ret_mantissa >>= 1;
        ret_exp2 = ret_exp2.wrapping_add(1);
    }
    if ret_exp2.wrapping_sub(1) >= 0x7FF - 1 {
        return None;
    }
    let mut ret_bits =
        ret_exp2 << FLOAT64_MANT_BITS | ret_mantissa & ((1 << FLOAT64_MANT_BITS) - 1);
    if neg {
        ret_bits |= 0x8000_0000_0000_0000;
    }
    Some(f64::from_bits(ret_bits))
}

/// `atofHex` for float64: round half to even using the two bits below the mantissa.
fn atof_hex(mantissa: u64, exp: i64, neg: bool, trunc: bool) -> (f64, Option<NumErrorKind>) {
    let mant_bits = FLOAT64_MANT_BITS;
    let max_exp = (1i64 << FLOAT64_EXP_BITS) + FLOAT64_BIAS - 2;
    let min_exp = FLOAT64_BIAS + 1;
    let mut mantissa = mantissa;
    let mut exp = exp + i64::from(mant_bits); // mantissa now implicitly divided by 2^mantbits.

    // Shift mantissa and exponent to bring representation into float range. Eventually we want a
    // mantissa with a leading 1-bit followed by mantbits other bits. For rounding, we need two more,
    // where the bottom bit represents whether that bit or any later bit was non-zero.
    while mantissa != 0 && mantissa >> (mant_bits + 2) == 0 {
        mantissa <<= 1;
        exp -= 1;
    }
    if trunc {
        mantissa |= 1;
    }
    while mantissa >> (1 + mant_bits + 2) != 0 {
        mantissa = mantissa >> 1 | mantissa & 1;
        exp += 1;
    }

    // If exponent is too negative, denormalize in hopes of making it representable.
    while mantissa > 1 && exp < min_exp - 2 {
        mantissa = mantissa >> 1 | mantissa & 1;
        exp += 1;
    }

    // Round using two bottom bits.
    let mut round = mantissa & 3;
    mantissa >>= 2;
    round |= mantissa & 1; // round to even (round up if mantissa is odd)
    exp += 2;
    if round == 3 {
        mantissa += 1;
        if mantissa == 1 << (1 + mant_bits) {
            mantissa >>= 1;
            exp += 1;
        }
    }

    if mantissa >> mant_bits == 0 {
        // Denormal or zero.
        exp = FLOAT64_BIAS;
    }
    let mut err = None;
    if exp > max_exp {
        // infinity and range error
        mantissa = 1 << mant_bits;
        exp = max_exp + 1;
        err = Some(NumErrorKind::Range);
    }
    let mut bits = mantissa & ((1 << mant_bits) - 1);
    bits |= (((exp - FLOAT64_BIAS) & ((1 << FLOAT64_EXP_BITS) - 1)) as u64) << mant_bits;
    if neg {
        bits |= 1 << mant_bits << FLOAT64_EXP_BITS;
    }
    (f64::from_bits(bits), err)
}

/// Go's multiprecision `decimal`, for the slow `ParseFloat` path.
struct Decimal {
    /// digits, big-endian representation
    d: [u8; 800],
    /// number of digits used
    nd: usize,
    /// decimal point
    dp: i64,
    /// negative flag
    neg: bool,
    /// discarded nonzero digits beyond d[:nd]
    trunc: bool,
}

/// `maxShift` on a 64-bit host.
const MAX_SHIFT: u32 = 64 - 4;

/// `powtab`: decimal power of ten to binary power of two.
const POWTAB: [i64; 9] = [1, 3, 6, 9, 13, 16, 19, 23, 26];

/// `leftcheats`: (new digits, cutoff) per shift count.
const LEFTCHEATS: [(usize, &[u8]); 61] = [
    (0, b""),
    (1, b"5"),
    (1, b"25"),
    (1, b"125"),
    (2, b"625"),
    (2, b"3125"),
    (2, b"15625"),
    (3, b"78125"),
    (3, b"390625"),
    (3, b"1953125"),
    (4, b"9765625"),
    (4, b"48828125"),
    (4, b"244140625"),
    (4, b"1220703125"),
    (5, b"6103515625"),
    (5, b"30517578125"),
    (5, b"152587890625"),
    (6, b"762939453125"),
    (6, b"3814697265625"),
    (6, b"19073486328125"),
    (7, b"95367431640625"),
    (7, b"476837158203125"),
    (7, b"2384185791015625"),
    (7, b"11920928955078125"),
    (8, b"59604644775390625"),
    (8, b"298023223876953125"),
    (8, b"1490116119384765625"),
    (9, b"7450580596923828125"),
    (9, b"37252902984619140625"),
    (9, b"186264514923095703125"),
    (10, b"931322574615478515625"),
    (10, b"4656612873077392578125"),
    (10, b"23283064365386962890625"),
    (10, b"116415321826934814453125"),
    (11, b"582076609134674072265625"),
    (11, b"2910383045673370361328125"),
    (11, b"14551915228366851806640625"),
    (12, b"72759576141834259033203125"),
    (12, b"363797880709171295166015625"),
    (12, b"1818989403545856475830078125"),
    (13, b"9094947017729282379150390625"),
    (13, b"45474735088646411895751953125"),
    (13, b"227373675443232059478759765625"),
    (13, b"1136868377216160297393798828125"),
    (14, b"5684341886080801486968994140625"),
    (14, b"28421709430404007434844970703125"),
    (14, b"142108547152020037174224853515625"),
    (15, b"710542735760100185871124267578125"),
    (15, b"3552713678800500929355621337890625"),
    (15, b"17763568394002504646778106689453125"),
    (16, b"88817841970012523233890533447265625"),
    (16, b"444089209850062616169452667236328125"),
    (16, b"2220446049250313080847263336181640625"),
    (16, b"11102230246251565404236316680908203125"),
    (17, b"55511151231257827021181583404541015625"),
    (17, b"277555756156289135105907917022705078125"),
    (17, b"1387778780781445675529539585113525390625"),
    (18, b"6938893903907228377647697925567626953125"),
    (18, b"34694469519536141888238489627838134765625"),
    (18, b"173472347597680709441192448139190673828125"),
    (19, b"867361737988403547205962240695953369140625"),
];

impl Decimal {
    fn new() -> Decimal {
        Decimal {
            d: [0; 800],
            nd: 0,
            dp: 0,
            neg: false,
            trunc: false,
        }
    }

    /// `decimal.set`.
    fn set(&mut self, s: &[u8]) -> bool {
        let mut i = 0;
        self.neg = false;
        self.trunc = false;

        // optional sign
        if i >= s.len() {
            return false;
        }
        match s[i] {
            b'+' => i += 1,
            b'-' => {
                i += 1;
                self.neg = true;
            }
            _ => {}
        }

        // digits
        let mut sawdot = false;
        let mut sawdigits = false;
        while i < s.len() {
            let c = s[i];
            if c == b'_' {
                // readFloat already checked underscores
            } else if c == b'.' {
                if sawdot {
                    return false;
                }
                sawdot = true;
                self.dp = self.nd as i64;
            } else if c.is_ascii_digit() {
                sawdigits = true;
                if c == b'0' && self.nd == 0 {
                    // ignore leading zeros
                    self.dp -= 1;
                } else if self.nd < self.d.len() {
                    self.d[self.nd] = c;
                    self.nd += 1;
                } else if c != b'0' {
                    self.trunc = true;
                }
            } else {
                break;
            }
            i += 1;
        }
        if !sawdigits {
            return false;
        }
        if !sawdot {
            self.dp = self.nd as i64;
        }

        // optional exponent moves decimal point.
        if i < s.len() && lower(s[i]) == b'e' {
            i += 1;
            if i >= s.len() {
                return false;
            }
            let mut esign = 1;
            match s[i] {
                b'+' => i += 1,
                b'-' => {
                    i += 1;
                    esign = -1;
                }
                _ => {}
            }
            if i >= s.len() || !s[i].is_ascii_digit() {
                return false;
            }
            let mut e: i64 = 0;
            while i < s.len() && (s[i].is_ascii_digit() || s[i] == b'_') {
                if s[i] != b'_' && e < 10000 {
                    e = e * 10 + i64::from(s[i] - b'0');
                }
                i += 1;
            }
            self.dp += e * esign;
        }
        i == s.len()
    }

    /// `trim`: drop trailing zeros.
    fn trim(&mut self) {
        while self.nd > 0 && self.d[self.nd - 1] == b'0' {
            self.nd -= 1;
        }
        if self.nd == 0 {
            self.dp = 0;
        }
    }

    /// `rightShift`: / 2^k, k <= MAX_SHIFT.
    fn right_shift(&mut self, k: u32) {
        let mut r = 0; // read pointer
        let mut w = 0; // write pointer

        // Pick up enough leading digits to cover first shift.
        let mut n: u64 = 0;
        while n >> k == 0 {
            if r >= self.nd {
                if n == 0 {
                    // a == 0; shouldn't get here, but handle anyway.
                    self.nd = 0;
                    return;
                }
                while n >> k == 0 {
                    n *= 10;
                    r += 1;
                }
                break;
            }
            let c = u64::from(self.d[r]);
            n = n * 10 + c - u64::from(b'0');
            r += 1;
        }
        self.dp -= r as i64 - 1;

        let mask: u64 = (1 << k) - 1;

        // Pick up a digit, put down a digit.
        while r < self.nd {
            let c = u64::from(self.d[r]);
            let dig = n >> k;
            n &= mask;
            self.d[w] = dig as u8 + b'0';
            w += 1;
            n = n * 10 + c - u64::from(b'0');
            r += 1;
        }

        // Put down extra digits.
        while n > 0 {
            let dig = n >> k;
            n &= mask;
            if w < self.d.len() {
                self.d[w] = dig as u8 + b'0';
                w += 1;
            } else if dig > 0 {
                self.trunc = true;
            }
            n *= 10;
        }

        self.nd = w;
        self.trim();
    }

    /// `leftShift`: * 2^k, k <= MAX_SHIFT.
    fn left_shift(&mut self, k: u32) {
        let (mut delta, cutoff) = LEFTCHEATS[k as usize];
        if prefix_is_less_than(&self.d[..self.nd], cutoff) {
            delta -= 1;
        }

        let mut r = self.nd; // read index
        let mut w = self.nd + delta; // write index

        // Pick up a digit, put down a digit.
        let mut n: u64 = 0;
        while r > 0 {
            r -= 1;
            n += u64::from(self.d[r] - b'0') << k;
            let quo = n / 10;
            let rem = n - 10 * quo;
            w -= 1;
            if w < self.d.len() {
                self.d[w] = rem as u8 + b'0';
            } else if rem != 0 {
                self.trunc = true;
            }
            n = quo;
        }

        // Put down extra digits.
        while n > 0 {
            let quo = n / 10;
            let rem = n - 10 * quo;
            w -= 1;
            if w < self.d.len() {
                self.d[w] = rem as u8 + b'0';
            } else if rem != 0 {
                self.trunc = true;
            }
            n = quo;
        }

        self.nd += delta;
        if self.nd >= self.d.len() {
            self.nd = self.d.len();
        }
        self.dp += delta as i64;
        self.trim();
    }

    /// `Shift`: left (k > 0) or right (k < 0).
    fn shift(&mut self, k: i64) {
        if self.nd == 0 {
            return;
        }
        let max = i64::from(MAX_SHIFT);
        let mut k = k;
        if k > 0 {
            while k > max {
                self.left_shift(MAX_SHIFT);
                k -= max;
            }
            self.left_shift(k as u32);
        } else if k < 0 {
            while k < -max {
                self.right_shift(MAX_SHIFT);
                k += max;
            }
            self.right_shift((-k) as u32);
        }
    }

    /// `shouldRoundUp`: if we chop at nd digits, should we round up?
    fn should_round_up(&self, nd: i64) -> bool {
        if nd < 0 || nd >= self.nd as i64 {
            return false;
        }
        let nd = nd as usize;
        if self.d[nd] == b'5' && nd + 1 == self.nd {
            // exactly halfway - round to even
            // if we truncated, a little higher than what's recorded - always round up
            if self.trunc {
                return true;
            }
            return nd > 0 && !(self.d[nd - 1] - b'0').is_multiple_of(2);
        }
        // not halfway - digit tells all
        self.d[nd] >= b'5'
    }

    /// `RoundedInteger`: the integer part, rounded; no guarantees about overflow.
    fn rounded_integer(&self) -> u64 {
        if self.dp > 20 {
            return u64::MAX;
        }
        let mut i: i64 = 0;
        let mut n: u64 = 0;
        while i < self.dp && i < self.nd as i64 {
            n = n
                .wrapping_mul(10)
                .wrapping_add(u64::from(self.d[i as usize] - b'0'));
            i += 1;
        }
        while i < self.dp {
            n = n.wrapping_mul(10);
            i += 1;
        }
        if self.should_round_up(self.dp) {
            n = n.wrapping_add(1);
        }
        n
    }

    /// `floatBits` for float64: the bits and whether the value overflowed.
    fn float_bits(&mut self) -> (u64, bool) {
        let mant_bits = FLOAT64_MANT_BITS;
        let exp_bits = FLOAT64_EXP_BITS;
        let bias = FLOAT64_BIAS;
        let mut exp: i64;
        let mut mant: u64;
        let mut overflow = false;

        'out: {
            'overflow: {
                // Zero is always a special case.
                if self.nd == 0 {
                    mant = 0;
                    exp = bias;
                    break 'out;
                }
                // Obvious overflow/underflow.
                if self.dp > 310 {
                    break 'overflow;
                }
                if self.dp < -330 {
                    // zero
                    mant = 0;
                    exp = bias;
                    break 'out;
                }

                // Scale by powers of two until in range [0.5, 1.0)
                exp = 0;
                while self.dp > 0 {
                    let n = if self.dp >= POWTAB.len() as i64 {
                        27
                    } else {
                        POWTAB[self.dp as usize]
                    };
                    self.shift(-n);
                    exp += n;
                }
                while self.dp < 0 || (self.dp == 0 && self.d[0] < b'5') {
                    let n = if -self.dp >= POWTAB.len() as i64 {
                        27
                    } else {
                        POWTAB[(-self.dp) as usize]
                    };
                    self.shift(n);
                    exp -= n;
                }

                // Our range is [0.5,1) but floating point range is [1,2).
                exp -= 1;

                // Minimum representable exponent is bias+1. If the exponent is smaller, move it
                // up and adjust d accordingly.
                if exp < bias + 1 {
                    let n = bias + 1 - exp;
                    self.shift(-n);
                    exp += n;
                }

                if exp - bias >= (1 << exp_bits) - 1 {
                    break 'overflow;
                }

                // Extract 1+mantbits bits.
                self.shift(i64::from(1 + mant_bits));
                mant = self.rounded_integer();

                // Rounding might have added a bit; shift down.
                if mant == 2 << mant_bits {
                    mant >>= 1;
                    exp += 1;
                    if exp - bias >= (1 << exp_bits) - 1 {
                        break 'overflow;
                    }
                }

                // Denormalized?
                if mant & (1 << mant_bits) == 0 {
                    exp = bias;
                }
                break 'out;
            }
            // ±Inf
            mant = 0;
            exp = (1 << exp_bits) - 1 + bias;
            overflow = true;
        }

        // Assemble bits.
        let mut bits = mant & ((1u64 << mant_bits) - 1);
        bits |= (((exp - bias) & ((1 << exp_bits) - 1)) as u64) << mant_bits;
        if self.neg {
            bits |= 1 << mant_bits << exp_bits;
        }
        (bits, overflow)
    }
}

/// `strconv.FormatFloat(f, 'g', -1, 64)`.
pub fn format_float_g(f: f64) -> String {
    let bits = f.to_bits();
    let neg = bits >> (FLOAT64_EXP_BITS + FLOAT64_MANT_BITS) != 0;
    let mut exp = ((bits >> FLOAT64_MANT_BITS) & ((1 << FLOAT64_EXP_BITS) - 1)) as i64;
    let mut mant = bits & ((1u64 << FLOAT64_MANT_BITS) - 1);
    let mut denorm = false;
    match exp {
        0x7FF => {
            // Inf, NaN
            let s = if mant != 0 {
                "NaN"
            } else if neg {
                "-Inf"
            } else {
                "+Inf"
            };
            return s.to_owned();
        }
        0 => {
            // denormalized
            exp += 1;
            denorm = true;
        }
        _ => {
            // add implicit top bit
            mant |= 1u64 << FLOAT64_MANT_BITS;
        }
    }
    exp += FLOAT64_BIAS;

    if mant == 0 {
        // formatDigits with prec -1 and no digits: "0" or "-0".
        return format_digits_g(neg, &[], 0, -1);
    }
    let (dmant, dexp) = dbox_ftoa64(mant, exp - i64::from(FLOAT64_MANT_BITS), denorm);
    let digits = dmant.to_string();
    let nd = digits.len() as i64;
    let dp = nd + dexp;
    // Precision for shortest representation mode.
    format_digits_g(neg, digits.as_bytes(), dp, nd)
}

/// `formatDigits` for fmt 'g' in shortest mode (`eprec` is 6).
fn format_digits_g(neg: bool, d: &[u8], dp: i64, prec: i64) -> String {
    let nd = d.len() as i64;
    let mut prec = prec;
    let exp = dp - 1;
    if !(-4..6).contains(&exp) {
        if prec > nd {
            prec = nd;
        }
        return fmt_e(neg, d, dp, prec - 1);
    }
    if prec > dp {
        prec = nd;
    }
    fmt_f(neg, d, dp, (prec - dp).max(0))
}

/// `fmtE`: -d.ddddde±dd.
fn fmt_e(neg: bool, d: &[u8], dp: i64, prec: i64) -> String {
    let mut dst = String::with_capacity(24);
    if neg {
        dst.push('-');
    }
    // first digit
    dst.push(d.first().map_or('0', |&c| char::from(c)));
    // .moredigits
    if prec > 0 {
        dst.push('.');
        let mut i = 1;
        let m = (d.len() as i64).min(prec + 1);
        while i < m {
            dst.push(char::from(d[i as usize]));
            i += 1;
        }
        while i <= prec {
            dst.push('0');
            i += 1;
        }
    }
    // e±
    dst.push('e');
    let mut exp = dp - 1;
    if d.is_empty() {
        // special case: 0 has exponent 0
        exp = 0;
    }
    if exp < 0 {
        dst.push('-');
        exp = -exp;
    } else {
        dst.push('+');
    }
    // dd or ddd
    let digit = |x: i64| char::from(b'0' + (x % 10) as u8);
    if exp < 10 {
        dst.push('0');
        dst.push(digit(exp));
    } else if exp < 100 {
        dst.push(digit(exp / 10));
        dst.push(digit(exp));
    } else {
        dst.push(digit(exp / 100));
        dst.push(digit(exp / 10));
        dst.push(digit(exp));
    }
    dst
}

/// `fmtF`: -ddddddd.ddddd.
fn fmt_f(neg: bool, d: &[u8], dp: i64, prec: i64) -> String {
    let mut dst = String::with_capacity(24);
    if neg {
        dst.push('-');
    }
    // integer, padded with zeros as needed.
    if dp > 0 {
        let m = (d.len() as i64).min(dp);
        for &c in &d[..m as usize] {
            dst.push(char::from(c));
        }
        for _ in m..dp {
            dst.push('0');
        }
    } else {
        dst.push('0');
    }
    // fraction
    if prec > 0 {
        dst.push('.');
        for i in 0..prec {
            let j = dp + i;
            let ch = if 0 <= j && j < d.len() as i64 {
                char::from(d[j as usize])
            } else {
                '0'
            };
            dst.push(ch);
        }
    }
    dst
}

/// A 128-bit value as Go's `uint128 {Hi, Lo}`.
#[derive(Clone, Copy)]
struct U128 {
    hi: u64,
    lo: u64,
}

/// `bits.Mul64`: (hi, lo).
fn mul64(x: u64, y: u64) -> (u64, u64) {
    let p = u128::from(x) * u128::from(y);
    ((p >> 64) as u64, p as u64)
}

/// Go's `x >> n` for an unsigned 64-bit value: 0 once `n` reaches 64.
fn shr(x: u64, n: i64) -> u64 {
    u32::try_from(n)
        .ok()
        .and_then(|n| x.checked_shr(n))
        .unwrap_or(0)
}

/// Go's `x << n` for an unsigned 64-bit value: 0 once `n` reaches 64.
fn shl(x: u64, n: i64) -> u64 {
    u32::try_from(n)
        .ok()
        .and_then(|n| x.checked_shl(n))
        .unwrap_or(0)
}

/// `dboxFtoa64`: the decimal significand and exponent of `mant * 2^exp` (Dragonbox).
fn dbox_ftoa64(mant: u64, exp: i64, denorm: bool) -> (u64, i64) {
    if mant == 1 << FLOAT64_MANT_BITS && !denorm {
        // Algorithm 5.6 (page 24).
        let k0 = -mul_log10_2_minus_log10_4_over3(exp);
        let (phi, beta) = dbox_pow64(k0, exp);
        let (mut xi, zi) = dbox_range64(phi, beta);
        if exp != 2 && exp != 3 {
            xi = xi.wrapping_add(1);
        }
        let q = zi / 10;
        if xi <= q.wrapping_mul(10) {
            let (q, zeros) = trim_zeros(q);
            return (q, -k0 + 1 + zeros);
        }
        let mut yru = dbox_round_up64(phi, beta);
        if exp == -77 && !yru.is_multiple_of(2) {
            yru = yru.wrapping_sub(1);
        } else if yru < xi {
            yru = yru.wrapping_add(1);
        }
        return (yru, -k0);
    }

    // κ = 2 for float64 (section 5.1.3)
    const P10K: u32 = 100; // 10**κ
    const P10K1: u64 = 1000; // 10**(κ+1)

    // Algorithm 5.2 (page 15).
    let k0 = -mul_log10_2(exp);
    let (phi, beta) = dbox_pow64(2 + k0, exp);
    let (zi, exact) = dbox_mul_pow64(shl(mant.wrapping_mul(2).wrapping_add(1), beta), phi);
    let mut s = zi / P10K1;
    let mut r = (zi % P10K1) as u32;
    let delta_i = dbox_delta64(phi, beta);

    if r < delta_i {
        if r != 0 || !exact || mant.is_multiple_of(2) {
            let (s, zeros) = trim_zeros(s);
            return (s, -k0 + 1 + zeros);
        }
        s = s.wrapping_sub(1);
        r = P10K * 10;
    } else if r == delta_i {
        let (parity, exact) = dbox_parity64(mant.wrapping_mul(2).wrapping_sub(1), phi, beta);
        if parity || (exact && mant.is_multiple_of(2)) {
            let (s, zeros) = trim_zeros(s);
            return (s, -k0 + 1 + zeros);
        }
    }

    // Algorithm 5.4 (page 18).
    let dd = r.wrapping_add(P10K / 2).wrapping_sub(delta_i / 2);
    let t = dd / P10K;
    let rho = dd % P10K;
    let mut yru = s.wrapping_mul(10).wrapping_add(u64::from(t));
    if rho == 0 {
        let (parity, exact) = dbox_parity64(mant.wrapping_mul(2), phi, beta);
        if parity == dd.wrapping_sub(P10K / 2).is_multiple_of(2)
            || (exact && !yru.is_multiple_of(2))
        {
            yru = yru.wrapping_sub(1);
        }
    }
    (yru, -k0)
}

/// `uadd128`: u + n (128 bits).
fn uadd128(u: U128, n: u64) -> U128 {
    let sum = u.lo.wrapping_add(n);
    let hi = if sum < u.lo {
        u.hi.wrapping_add(1)
    } else {
        u.hi
    };
    U128 { hi, lo: sum }
}

/// `umul128`: the 128-bit product x*y.
fn umul128(x: u64, y: u64) -> U128 {
    let (hi, lo) = mul64(x, y);
    U128 { hi, lo }
}

/// `umul192Upper128`: the upper 128 bits (out of 192) of x * y.
fn umul192_upper128(x: u64, y: U128) -> U128 {
    let r = umul128(x, y.hi);
    let t = mul64(x, y.lo).0;
    uadd128(r, t)
}

/// `umul192Lower128`: the lower 128 bits (out of 192) of x * y.
fn umul192_lower128(x: u64, y: U128) -> U128 {
    let high = x.wrapping_mul(y.hi);
    let high_low = umul128(x, y.lo);
    U128 {
        hi: high.wrapping_add(high_low.hi),
        lo: high_low.lo,
    }
}

/// `dboxMulPow64`.
fn dbox_mul_pow64(u: u64, phi: U128) -> (u64, bool) {
    let r = umul192_upper128(u, phi);
    (r.hi, r.lo == 0)
}

/// `dboxParity64`.
fn dbox_parity64(mant2: u64, phi: U128, beta: i64) -> (bool, bool) {
    let r = umul192_lower128(mant2, phi);
    let parity = (shr(r.hi, 64 - beta) & 1) != 0;
    let is_int = (shl(r.hi, beta) | shr(r.lo, 64 - beta)) == 0;
    (parity, is_int)
}

/// `dboxDelta64`.
fn dbox_delta64(phi: U128, beta: i64) -> u32 {
    shr(phi.hi, 64 - 1 - beta) as u32
}

/// `mulLog10_2MinusLog10_4Over3`: ⌊e*log10(2)-log10(4/3)⌋.
fn mul_log10_2_minus_log10_4_over3(e: i64) -> i64 {
    (e * 631305 - 261663) >> 21
}

/// `mulLog10_2`: ⌊x * log(2)/log(10)⌋ for -1600 <= x <= 1600.
fn mul_log10_2(x: i64) -> i64 {
    (x * 78913) >> 18
}

/// `mulLog2_10`: ⌊x * log(10)/log(2)⌋ for -500 <= x <= 500.
fn mul_log2_10(x: i64) -> i64 {
    (x * 108853) >> 15
}

/// `dboxRange64`: the left and right endpoints.
fn dbox_range64(phi: U128, beta: i64) -> (u64, u64) {
    let shift = 64 - i64::from(FLOAT64_MANT_BITS) - 1 - beta;
    let left = shr(
        phi.hi.wrapping_sub(phi.hi >> (FLOAT64_MANT_BITS + 2)),
        shift,
    );
    let right = shr(
        phi.hi.wrapping_add(phi.hi >> (FLOAT64_MANT_BITS + 1)),
        shift,
    );
    (left, right)
}

/// `dboxRoundUp64`.
fn dbox_round_up64(phi: U128, beta: i64) -> u64 {
    shr(phi.hi, 128 / 2 - i64::from(FLOAT64_MANT_BITS) - 2 - beta).wrapping_add(1) / 2
}

/// `dboxPow64`: the precomputed φ̃k and β.
fn dbox_pow64(k: i64, e: i64) -> (U128, i64) {
    // Dragonbox only asks for exponents inside the table.
    let (hi, lo, e1) = pow10(k).unwrap_or((0, 0, 0));
    let mut phi = U128 { hi, lo };
    if !(0..=55).contains(&k) {
        phi.lo = phi.lo.wrapping_add(1);
    }
    (phi, e + e1 - 1)
}

/// `trimZeros`: the largest p with x % 10^p == 0, as (x / 10^p, p).
fn trim_zeros(x: u64) -> (u64, i64) {
    const DIV1E8M: u64 = 0xc767_074b_22e9_0e21;
    const DIV1E8LE: u64 = u64::MAX / 100_000_000;
    const DIV1E4M: u64 = 0xd288_ce70_3afb_7e91;
    const DIV1E4LE: u64 = u64::MAX / 10_000;
    const DIV1E2M: u64 = 0x8f5c_28f5_c28f_5c29;
    const DIV1E2LE: u64 = u64::MAX / 100;
    const DIV1E1M: u64 = 0xcccc_cccc_cccc_cccd;
    const DIV1E1LE: u64 = u64::MAX / 10;

    let mut x = x;
    let mut p = 0;
    loop {
        let d = x.wrapping_mul(DIV1E8M).rotate_right(8);
        if d > DIV1E8LE {
            break;
        }
        x = d;
        p += 8;
    }
    let d = x.wrapping_mul(DIV1E4M).rotate_right(4);
    if d <= DIV1E4LE {
        x = d;
        p += 4;
    }
    let d = x.wrapping_mul(DIV1E2M).rotate_right(2);
    if d <= DIV1E2LE {
        x = d;
        p += 2;
    }
    let d = x.wrapping_mul(DIV1E1M).rotate_right(1);
    if d <= DIV1E1LE {
        x = d;
        p += 1;
    }
    (x, p)
}

const POW10_MIN: i64 = -348;
const POW10_MAX: i64 = 347;

/// `pow10`: the 128-bit mantissa (hi, lo) and binary exponent of 10**e.
fn pow10(e: i64) -> Option<(u64, u64, i64)> {
    if !(POW10_MIN..=POW10_MAX).contains(&e) {
        return None;
    }
    let (hi, lo) = POW10_TAB[(e - POW10_MIN) as usize];
    Some((hi, lo, 1 + mul_log2_10(e)))
}

/// `prefixIsLessThan`: is the leading prefix of b lexicographically less than s?
fn prefix_is_less_than(b: &[u8], s: &[u8]) -> bool {
    for (i, &c) in s.iter().enumerate() {
        if i >= b.len() {
            return true;
        }
        if b[i] != c {
            return b[i] < c;
        }
    }
    false
}

/// `pow10Tab` (go1.26.5 `internal/strconv/pow10tab.go`): 128-bit mantissas of 10^-348 ..= 10^347, scaled so
/// the high bit is always set.
#[rustfmt::skip]
const POW10_TAB: [(u64, u64); 696] = [
    (0xfa8f_d5a0_081c_0288, 0x1732_c869_cd60_e453), // 1e-348 * 2**1284
    (0x9c99_e584_0511_8195, 0x0e7f_bd42_205c_8eb4), // 1e-347 * 2**1280
    (0xc3c0_5ee5_0655_e1fa, 0x521f_ac92_a873_b261), // 1e-346 * 2**1277
    (0xf4b0_769e_47eb_5a78, 0xe6a7_97b7_5290_9ef9), // 1e-345 * 2**1274
    (0x98ee_4a22_ecf3_188b, 0x9028_bed2_939a_635c), // 1e-344 * 2**1270
    (0xbf29_dcab_a82f_deae, 0x7432_ee87_3880_fc33), // 1e-343 * 2**1267
    (0xeef4_53d6_923b_d65a, 0x113f_aa29_06a1_3b3f), // 1e-342 * 2**1264
    (0x9558_b466_1b65_65f8, 0x4ac7_ca59_a424_c507), // 1e-341 * 2**1260
    (0xbaae_e17f_a23e_bf76, 0x5d79_bcf0_0d2d_f649), // 1e-340 * 2**1257
    (0xe95a_99df_8ace_6f53, 0xf4d8_2c2c_1079_73dc), // 1e-339 * 2**1254
    (0x91d8_a02b_b6c1_0594, 0x7907_1b9b_8a4b_e869), // 1e-338 * 2**1250
    (0xb64e_c836_a471_46f9, 0x9748_e282_6cde_e284), // 1e-337 * 2**1247
    (0xe3e2_7a44_4d8d_98b7, 0xfd1b_1b23_0816_9b25), // 1e-336 * 2**1244
    (0x8e6d_8c6a_b078_7f72, 0xfe30_f0f5_e50e_20f7), // 1e-335 * 2**1240
    (0xb208_ef85_5c96_9f4f, 0xbdbd_2d33_5e51_a935), // 1e-334 * 2**1237
    (0xde8b_2b66_b3bc_4723, 0xad2c_7880_35e6_1382), // 1e-333 * 2**1234
    (0x8b16_fb20_3055_ac76, 0x4c3b_cb50_21af_cc31), // 1e-332 * 2**1230
    (0xaddc_b9e8_3c6b_1793, 0xdf4a_be24_2a1b_bf3d), // 1e-331 * 2**1227
    (0xd953_e862_4b85_dd78, 0xd71d_6dad_34a2_af0d), // 1e-330 * 2**1224
    (0x87d4_713d_6f33_aa6b, 0x8672_648c_40e5_ad68), // 1e-329 * 2**1220
    (0xa9c9_8d8c_cb00_9506, 0x680e_fdaf_511f_18c2), // 1e-328 * 2**1217
    (0xd43b_f0ef_fdc0_ba48, 0x0212_bd1b_2566_def2), // 1e-327 * 2**1214
    (0x84a5_7695_fe98_746d, 0x014b_b630_f760_4b57), // 1e-326 * 2**1210
    (0xa5ce_d43b_7e3e_9188, 0x419e_a3bd_3538_5e2d), // 1e-325 * 2**1207
    (0xcf42_894a_5dce_35ea, 0x5206_4cac_8286_75b9), // 1e-324 * 2**1204
    (0x8189_95ce_7aa0_e1b2, 0x7343_efeb_d194_0993), // 1e-323 * 2**1200
    (0xa1eb_fb42_1949_1a1f, 0x1014_ebe6_c5f9_0bf8), // 1e-322 * 2**1197
    (0xca66_fa12_9f9b_60a6, 0xd41a_26e0_7777_4ef6), // 1e-321 * 2**1194
    (0xfd00_b897_4782_38d0, 0x8920_b098_9555_22b4), // 1e-320 * 2**1191
    (0x9e20_735e_8cb1_6382, 0x55b4_6e5f_5d55_35b0), // 1e-319 * 2**1187
    (0xc5a8_9036_2fdd_bc62, 0xeb21_89f7_34aa_831d), // 1e-318 * 2**1184
    (0xf712_b443_bbd5_2b7b, 0xa5e9_ec75_01d5_23e4), // 1e-317 * 2**1181
    (0x9a6b_b0aa_5565_3b2d, 0x47b2_33c9_2125_366e), // 1e-316 * 2**1177
    (0xc106_9cd4_eabe_89f8, 0x999e_c0bb_696e_840a), // 1e-315 * 2**1174
    (0xf148_440a_256e_2c76, 0xc006_70ea_43ca_250d), // 1e-314 * 2**1171
    (0x96cd_2a86_5764_dbca, 0x3804_0692_6a5e_5728), // 1e-313 * 2**1167
    (0xbc80_7527_ed3e_12bc, 0xc605_0837_04f5_ecf2), // 1e-312 * 2**1164
    (0xeba0_9271_e88d_976b, 0xf786_4a44_c633_682e), // 1e-311 * 2**1161
    (0x9344_5b87_3158_7ea3, 0x7ab3_ee6a_fbe0_211d), // 1e-310 * 2**1157
    (0xb815_7268_fdae_9e4c, 0x5960_ea05_bad8_2964), // 1e-309 * 2**1154
    (0xe61a_cf03_3d1a_45df, 0x6fb9_2487_298e_33bd), // 1e-308 * 2**1151
    (0x8fd0_c162_0630_6bab, 0xa5d3_b6d4_79f8_e056), // 1e-307 * 2**1147
    (0xb3c4_f1ba_87bc_8696, 0x8f48_a489_9877_186c), // 1e-306 * 2**1144
    (0xe0b6_2e29_29ab_a83c, 0x331a_cdab_fe94_de87), // 1e-305 * 2**1141
    (0x8c71_dcd9_ba0b_4925, 0x9ff0_c08b_7f1d_0b14), // 1e-304 * 2**1137
    (0xaf8e_5410_288e_1b6f, 0x07ec_f0ae_5ee4_4dd9), // 1e-303 * 2**1134
    (0xdb71_e914_32b1_a24a, 0xc9e8_2cd9_f69d_6150), // 1e-302 * 2**1131
    (0x8927_31ac_9faf_056e, 0xbe31_1c08_3a22_5cd2), // 1e-301 * 2**1127
    (0xab70_fe17_c79a_c6ca, 0x6dbd_630a_48aa_f406), // 1e-300 * 2**1124
    (0xd64d_3d9d_b981_787d, 0x092c_bbcc_dad5_b108), // 1e-299 * 2**1121
    (0x85f0_4682_93f0_eb4e, 0x25bb_f560_08c5_8ea5), // 1e-298 * 2**1117
    (0xa76c_5823_38ed_2621, 0xaf2a_f2b8_0af6_f24e), // 1e-297 * 2**1114
    (0xd147_6e2c_0728_6faa, 0x1af5_af66_0db4_aee1), // 1e-296 * 2**1111
    (0x82cc_a4db_8479_45ca, 0x50d9_8d9f_c890_ed4d), // 1e-295 * 2**1107
    (0xa37f_ce12_6597_973c, 0xe50f_f107_bab5_28a0), // 1e-294 * 2**1104
    (0xcc5f_c196_fefd_7d0c, 0x1e53_ed49_a962_72c8), // 1e-293 * 2**1101
    (0xff77_b1fc_bebc_dc4f, 0x25e8_e89c_13bb_0f7a), // 1e-292 * 2**1098
    (0x9faa_cf3d_f736_09b1, 0x77b1_9161_8c54_e9ac), // 1e-291 * 2**1094
    (0xc795_830d_7503_8c1d, 0xd59d_f5b9_ef6a_2417), // 1e-290 * 2**1091
    (0xf97a_e3d0_d244_6f25, 0x4b05_7328_6b44_ad1d), // 1e-289 * 2**1088
    (0x9bec_ce62_836a_c577, 0x4ee3_67f9_430a_ec32), // 1e-288 * 2**1084
    (0xc2e8_01fb_2445_76d5, 0x229c_41f7_93cd_a73f), // 1e-287 * 2**1081
    (0xf3a2_0279_ed56_d48a, 0x6b43_5275_78c1_110f), // 1e-286 * 2**1078
    (0x9845_418c_3456_44d6, 0x830a_1389_6b78_aaa9), // 1e-285 * 2**1074
    (0xbe56_91ef_416b_d60c, 0x23cc_986b_c656_d553), // 1e-284 * 2**1071
    (0xedec_366b_11c6_cb8f, 0x2cbf_be86_b7ec_8aa8), // 1e-283 * 2**1068
    (0x94b3_a202_eb1c_3f39, 0x7bf7_d714_32f3_d6a9), // 1e-282 * 2**1064
    (0xb9e0_8a83_a5e3_4f07, 0xdaf5_ccd9_3fb0_cc53), // 1e-281 * 2**1061
    (0xe858_ad24_8f5c_22c9, 0xd1b3_400f_8f9c_ff68), // 1e-280 * 2**1058
    (0x9137_6c36_d999_95be, 0x2310_0809_b9c2_1fa1), // 1e-279 * 2**1054
    (0xb585_4744_8fff_fb2d, 0xabd4_0a0c_2832_a78a), // 1e-278 * 2**1051
    (0xe2e6_9915_b3ff_f9f9, 0x16c9_0c8f_323f_516c), // 1e-277 * 2**1048
    (0x8dd0_1fad_907f_fc3b, 0xae3d_a7d9_7f67_92e3), // 1e-276 * 2**1044
    (0xb144_2798_f49f_fb4a, 0x99cd_11cf_df41_779c), // 1e-275 * 2**1041
    (0xdd95_317f_31c7_fa1d, 0x4040_5643_d711_d583), // 1e-274 * 2**1038
    (0x8a7d_3eef_7f1c_fc52, 0x4828_35ea_666b_2572), // 1e-273 * 2**1034
    (0xad1c_8eab_5ee4_3b66, 0xda32_4365_0005_eecf), // 1e-272 * 2**1031
    (0xd863_b256_369d_4a40, 0x90be_d43e_4007_6a82), // 1e-271 * 2**1028
    (0x873e_4f75_e222_4e68, 0x5a77_44a6_e804_a291), // 1e-270 * 2**1024
    (0xa90d_e353_5aaa_e202, 0x7115_15d0_a205_cb36), // 1e-269 * 2**1021
    (0xd351_5c28_3155_9a83, 0x0d5a_5b44_ca87_3e03), // 1e-268 * 2**1018
    (0x8412_d999_1ed5_8091, 0xe858_790a_fe94_86c2), // 1e-267 * 2**1014
    (0xa517_8fff_668a_e0b6, 0x626e_974d_be39_a872), // 1e-266 * 2**1011
    (0xce5d_73ff_402d_98e3, 0xfb0a_3d21_2dc8_128f), // 1e-265 * 2**1008
    (0x80fa_687f_881c_7f8e, 0x7ce6_6634_bc9d_0b99), // 1e-264 * 2**1004
    (0xa139_029f_6a23_9f72, 0x1c1f_ffc1_ebc4_4e80), // 1e-263 * 2**1001
    (0xc987_4347_44ac_874e, 0xa327_ffb2_66b5_6220), // 1e-262 * 2**998
    (0xfbe9_1419_15d7_a922, 0x4bf1_ff9f_0062_baa8), // 1e-261 * 2**995
    (0x9d71_ac8f_ada6_c9b5, 0x6f77_3fc3_603d_b4a9), // 1e-260 * 2**991
    (0xc4ce_17b3_9910_7c22, 0xcb55_0fb4_384d_21d3), // 1e-259 * 2**988
    (0xf601_9da0_7f54_9b2b, 0x7e2a_53a1_4660_6a48), // 1e-258 * 2**985
    (0x99c1_0284_4f94_e0fb, 0x2eda_7444_cbfc_426d), // 1e-257 * 2**981
    (0xc031_4325_637a_1939, 0xfa91_1155_fefb_5308), // 1e-256 * 2**978
    (0xf03d_93ee_bc58_9f88, 0x7935_55ab_7eba_27ca), // 1e-255 * 2**975
    (0x9626_7c75_35b7_63b5, 0x4bc1_558b_2f34_58de), // 1e-254 * 2**971
    (0xbbb0_1b92_8325_3ca2, 0x9eb1_aaed_fb01_6f16), // 1e-253 * 2**968
    (0xea9c_2277_23ee_8bcb, 0x465e_15a9_79c1_cadc), // 1e-252 * 2**965
    (0x92a1_958a_7675_175f, 0x0bfa_cd89_ec19_1ec9), // 1e-251 * 2**961
    (0xb749_faed_1412_5d36, 0xcef9_80ec_671f_667b), // 1e-250 * 2**958
    (0xe51c_79a8_5916_f484, 0x82b7_e127_80e7_401a), // 1e-249 * 2**955
    (0x8f31_cc09_37ae_58d2, 0xd1b2_ecb8_b090_8810), // 1e-248 * 2**951
    (0xb2fe_3f0b_8599_ef07, 0x861f_a7e6_dcb4_aa15), // 1e-247 * 2**948
    (0xdfbd_cece_6700_6ac9, 0x67a7_91e0_93e1_d49a), // 1e-246 * 2**945
    (0x8bd6_a141_0060_42bd, 0xe0c8_bb2c_5c6d_24e0), // 1e-245 * 2**941
    (0xaecc_4991_4078_536d, 0x58fa_e9f7_7388_6e18), // 1e-244 * 2**938
    (0xda7f_5bf5_9096_6848, 0xaf39_a475_506a_899e), // 1e-243 * 2**935
    (0x888f_9979_7a5e_012d, 0x6d84_06c9_5242_9603), // 1e-242 * 2**931
    (0xaab3_7fd7_d8f5_8178, 0xc8e5_087b_a6d3_3b83), // 1e-241 * 2**928
    (0xd560_5fcd_cf32_e1d6, 0xfb1e_4a9a_9088_0a64), // 1e-240 * 2**925
    (0x855c_3be0_a17f_cd26, 0x5cf2_eea0_9a55_067f), // 1e-239 * 2**921
    (0xa6b3_4ad8_c9df_c06f, 0xf42f_aa48_c0ea_481e), // 1e-238 * 2**918
    (0xd060_1d8e_fc57_b08b, 0xf13b_94da_f124_da26), // 1e-237 * 2**915
    (0x823c_1279_5db6_ce57, 0x76c5_3d08_d6b7_0858), // 1e-236 * 2**911
    (0xa2cb_1717_b524_81ed, 0x5476_8c4b_0c64_ca6e), // 1e-235 * 2**908
    (0xcb7d_dcdd_a26d_a268, 0xa994_2f5d_cf7d_fd09), // 1e-234 * 2**905
    (0xfe5d_5415_0b09_0b02, 0xd3f9_3b35_435d_7c4c), // 1e-233 * 2**902
    (0x9efa_548d_26e5_a6e1, 0xc47b_c501_4a1a_6daf), // 1e-232 * 2**898
    (0xc6b8_e9b0_709f_109a, 0x359a_b641_9ca1_091b), // 1e-231 * 2**895
    (0xf867_241c_8cc6_d4c0, 0xc301_63d2_03c9_4b62), // 1e-230 * 2**892
    (0x9b40_7691_d7fc_44f8, 0x79e0_de63_425d_cf1d), // 1e-229 * 2**888
    (0xc210_9436_4dfb_5636, 0x9859_15fc_12f5_42e4), // 1e-228 * 2**885
    (0xf294_b943_e17a_2bc4, 0x3e6f_5b7b_17b2_939d), // 1e-227 * 2**882
    (0x979c_f3ca_6cec_5b5a, 0xa705_992c_eecf_9c42), // 1e-226 * 2**878
    (0xbd84_30bd_0827_7231, 0x50c6_ff78_2a83_8353), // 1e-225 * 2**875
    (0xece5_3cec_4a31_4ebd, 0xa4f8_bf56_3524_6428), // 1e-224 * 2**872
    (0x940f_4613_ae5e_d136, 0x871b_7795_e136_be99), // 1e-223 * 2**868
    (0xb913_1798_99f6_8584, 0x28e2_557b_5984_6e3f), // 1e-222 * 2**865
    (0xe757_dd7e_c074_26e5, 0x331a_eada_2fe5_89cf), // 1e-221 * 2**862
    (0x9096_ea6f_3848_984f, 0x3ff0_d2c8_5def_7621), // 1e-220 * 2**858
    (0xb4bc_a50b_065a_be63, 0x0fed_077a_756b_53a9), // 1e-219 * 2**855
    (0xe1eb_ce4d_c7f1_6dfb, 0xd3e8_4959_12c6_2894), // 1e-218 * 2**852
    (0x8d33_60f0_9cf6_e4bd, 0x6471_2dd7_abbb_d95c), // 1e-217 * 2**848
    (0xb080_392c_c434_9dec, 0xbd8d_794d_96aa_cfb3), // 1e-216 * 2**845
    (0xdca0_4777_f541_c567, 0xecf0_d7a0_fc55_83a0), // 1e-215 * 2**842
    (0x89e4_2caa_f949_1b60, 0xf416_86c4_9db5_7244), // 1e-214 * 2**838
    (0xac5d_37d5_b79b_6239, 0x311c_2875_c522_ced5), // 1e-213 * 2**835
    (0xd774_85cb_2582_3ac7, 0x7d63_3293_366b_828b), // 1e-212 * 2**832
    (0x86a8_d39e_f771_64bc, 0xae5d_ff9c_0203_3197), // 1e-211 * 2**828
    (0xa853_0886_b54d_bdeb, 0xd9f5_7f83_0283_fdfc), // 1e-210 * 2**825
    (0xd267_caa8_62a1_2d66, 0xd072_df63_c324_fd7b), // 1e-209 * 2**822
    (0x8380_dea9_3da4_bc60, 0x4247_cb9e_59f7_1e6d), // 1e-208 * 2**818
    (0xa461_1653_8d0d_eb78, 0x52d9_be85_f074_e608), // 1e-207 * 2**815
    (0xcd79_5be8_7051_6656, 0x6790_2e27_6c92_1f8b), // 1e-206 * 2**812
    (0x806b_d971_4632_dff6, 0x00ba_1cd8_a3db_53b6), // 1e-205 * 2**808
    (0xa086_cfcd_97bf_97f3, 0x80e8_a40e_ccd2_28a4), // 1e-204 * 2**805
    (0xc8a8_83c0_fdaf_7df0, 0x6122_cd12_8006_b2cd), // 1e-203 * 2**802
    (0xfad2_a4b1_3d1b_5d6c, 0x796b_8057_2008_5f81), // 1e-202 * 2**799
    (0x9cc3_a6ee_c631_1a63, 0xcbe3_3036_7405_3bb0), // 1e-201 * 2**795
    (0xc3f4_90aa_77bd_60fc, 0xbedb_fc44_1106_8a9c), // 1e-200 * 2**792
    (0xf4f1_b4d5_15ac_b93b, 0xee92_fb55_1548_2d44), // 1e-199 * 2**789
    (0x9917_1105_2d8b_f3c5, 0x751b_dd15_2d4d_1c4a), // 1e-198 * 2**785
    (0xbf5c_d546_78ee_f0b6, 0xd262_d45a_78a0_635d), // 1e-197 * 2**782
    (0xef34_0a98_172a_ace4, 0x86fb_8971_16c8_7c34), // 1e-196 * 2**779
    (0x9580_869f_0e7a_ac0e, 0xd45d_35e6_ae3d_4da0), // 1e-195 * 2**775
    (0xbae0_a846_d219_5712, 0x8974_8360_59cc_a109), // 1e-194 * 2**772
    (0xe998_d258_869f_acd7, 0x2bd1_a438_703f_c94b), // 1e-193 * 2**769
    (0x91ff_8377_5423_cc06, 0x7b63_06a3_4627_ddcf), // 1e-192 * 2**765
    (0xb67f_6455_292c_bf08, 0x1a3b_c84c_17b1_d542), // 1e-191 * 2**762
    (0xe41f_3d6a_7377_eeca, 0x20ca_ba5f_1d9e_4a93), // 1e-190 * 2**759
    (0x8e93_8662_882a_f53e, 0x547e_b47b_7282_ee9c), // 1e-189 * 2**755
    (0xb238_67fb_2a35_b28d, 0xe99e_619a_4f23_aa43), // 1e-188 * 2**752
    (0xdec6_81f9_f4c3_1f31, 0x6405_fa00_e2ec_94d4), // 1e-187 * 2**749
    (0x8b3c_113c_38f9_f37e, 0xde83_bc40_8dd3_dd04), // 1e-186 * 2**745
    (0xae0b_158b_4738_705e, 0x9624_ab50_b148_d445), // 1e-185 * 2**742
    (0xd98d_daee_1906_8c76, 0x3bad_d624_dd9b_0957), // 1e-184 * 2**739
    (0x87f8_a8d4_cfa4_17c9, 0xe54c_a5d7_0a80_e5d6), // 1e-183 * 2**735
    (0xa9f6_d30a_038d_1dbc, 0x5e9f_cf4c_cd21_1f4c), // 1e-182 * 2**732
    (0xd474_87cc_8470_652b, 0x7647_c320_0069_671f), // 1e-181 * 2**729
    (0x84c8_d4df_d2c6_3f3b, 0x29ec_d9f4_0041_e073), // 1e-180 * 2**725
    (0xa5fb_0a17_c777_cf09, 0xf468_1071_0052_5890), // 1e-179 * 2**722
    (0xcf79_cc9d_b955_c2cc, 0x7182_148d_4066_eeb4), // 1e-178 * 2**719
    (0x81ac_1fe2_93d5_99bf, 0xc6f1_4cd8_4840_5530), // 1e-177 * 2**715
    (0xa217_27db_38cb_002f, 0xb8ad_a00e_5a50_6a7c), // 1e-176 * 2**712
    (0xca9c_f1d2_06fd_c03b, 0xa6d9_0811_f0e4_851c), // 1e-175 * 2**709
    (0xfd44_2e46_88bd_304a, 0x908f_4a16_6d1d_a663), // 1e-174 * 2**706
    (0x9e4a_9cec_1576_3e2e, 0x9a59_8e4e_0432_87fe), // 1e-173 * 2**702
    (0xc5dd_4427_1ad3_cdba, 0x40ef_f1e1_853f_29fd), // 1e-172 * 2**699
    (0xf754_9530_e188_c128, 0xd12b_ee59_e68e_f47c), // 1e-171 * 2**696
    (0x9a94_dd3e_8cf5_78b9, 0x82bb_74f8_3019_58ce), // 1e-170 * 2**692
    (0xc13a_148e_3032_d6e7, 0xe36a_5236_3c1f_af01), // 1e-169 * 2**689
    (0xf188_99b1_bc3f_8ca1, 0xdc44_e6c3_cb27_9ac1), // 1e-168 * 2**686
    (0x96f5_600f_15a7_b7e5, 0x29ab_103a_5ef8_c0b9), // 1e-167 * 2**682
    (0xbcb2_b812_db11_a5de, 0x7415_d448_f6b6_f0e7), // 1e-166 * 2**679
    (0xebdf_6617_91d6_0f56, 0x111b_495b_3464_ad21), // 1e-165 * 2**676
    (0x936b_9fce_bb25_c995, 0xcab1_0dd9_00be_ec34), // 1e-164 * 2**672
    (0xb846_87c2_69ef_3bfb, 0x3d5d_514f_40ee_a742), // 1e-163 * 2**669
    (0xe658_29b3_046b_0afa, 0x0cb4_a5a3_112a_5112), // 1e-162 * 2**666
    (0x8ff7_1a0f_e2c2_e6dc, 0x47f0_e785_eaba_72ab), // 1e-161 * 2**662
    (0xb3f4_e093_db73_a093, 0x59ed_2167_6569_0f56), // 1e-160 * 2**659
    (0xe0f2_18b8_d250_88b8, 0x3068_69c1_3ec3_532c), // 1e-159 * 2**656
    (0x8c97_4f73_8372_5573, 0x1e41_4218_c73a_13fb), // 1e-158 * 2**652
    (0xafbd_2350_644e_eacf, 0xe5d1_929e_f908_98fa), // 1e-157 * 2**649
    (0xdbac_6c24_7d62_a583, 0xdf45_f746_b74a_bf39), // 1e-156 * 2**646
    (0x894b_c396_ce5d_a772, 0x6b8b_ba8c_328e_b783), // 1e-155 * 2**642
    (0xab9e_b47c_81f5_114f, 0x066e_a92f_3f32_6564), // 1e-154 * 2**639
    (0xd686_619b_a272_55a2, 0xc80a_537b_0efe_febd), // 1e-153 * 2**636
    (0x8613_fd01_4587_7585, 0xbd06_742c_e95f_5f36), // 1e-152 * 2**632
    (0xa798_fc41_96e9_52e7, 0x2c48_1138_23b7_3704), // 1e-151 * 2**629
    (0xd17f_3b51_fca3_a7a0, 0xf75a_1586_2ca5_04c5), // 1e-150 * 2**626
    (0x82ef_8513_3de6_48c4, 0x9a98_4d73_dbe7_22fb), // 1e-149 * 2**622
    (0xa3ab_6658_0d5f_daf5, 0xc13e_60d0_d2e0_ebba), // 1e-148 * 2**619
    (0xcc96_3fee_10b7_d1b3, 0x318d_f905_0799_26a8), // 1e-147 * 2**616
    (0xffbb_cfe9_94e5_c61f, 0xfdf1_7746_497f_7052), // 1e-146 * 2**613
    (0x9fd5_61f1_fd0f_9bd3, 0xfeb6_ea8b_edef_a633), // 1e-145 * 2**609
    (0xc7ca_ba6e_7c53_82c8, 0xfe64_a52e_e96b_8fc0), // 1e-144 * 2**606
    (0xf9bd_690a_1b68_637b, 0x3dfd_ce7a_a3c6_73b0), // 1e-143 * 2**603
    (0x9c16_61a6_5121_3e2d, 0x06be_a10c_a65c_084e), // 1e-142 * 2**599
    (0xc31b_fa0f_e569_8db8, 0x486e_494f_cff3_0a62), // 1e-141 * 2**596
    (0xf3e2_f893_dec3_f126, 0x5a89_dba3_c3ef_ccfa), // 1e-140 * 2**593
    (0x986d_db5c_6b3a_76b7, 0xf896_2946_5a75_e01c), // 1e-139 * 2**589
    (0xbe89_5233_8609_1465, 0xf6bb_b397_f113_5823), // 1e-138 * 2**586
    (0xee2b_a6c0_678b_597f, 0x746a_a07d_ed58_2e2c), // 1e-137 * 2**583
    (0x94db_4838_40b7_17ef, 0xa8c2_a44e_b457_1cdc), // 1e-136 * 2**579
    (0xba12_1a46_50e4_ddeb, 0x92f3_4d62_616c_e413), // 1e-135 * 2**576
    (0xe896_a0d7_e51e_1566, 0x77b0_20ba_f9c8_1d17), // 1e-134 * 2**573
    (0x915e_2486_ef32_cd60, 0x0ace_1474_dc1d_122e), // 1e-133 * 2**569
    (0xb5b5_ada8_aaff_80b8, 0x0d81_9992_1324_56ba), // 1e-132 * 2**566
    (0xe323_1912_d5bf_60e6, 0x10e1_fff6_97ed_6c69), // 1e-131 * 2**563
    (0x8df5_efab_c597_9c8f, 0xca8d_3ffa_1ef4_63c1), // 1e-130 * 2**559
    (0xb173_6b96_b6fd_83b3, 0xbd30_8ff8_a6b1_7cb2), // 1e-129 * 2**556
    (0xddd0_467c_64bc_e4a0, 0xac7c_b3f6_d05d_dbde), // 1e-128 * 2**553
    (0x8aa2_2c0d_bef6_0ee4, 0x6bcd_f07a_423a_a96b), // 1e-127 * 2**549
    (0xad4a_b711_2eb3_929d, 0x86c1_6c98_d2c9_53c6), // 1e-126 * 2**546
    (0xd89d_64d5_7a60_7744, 0xe871_c7bf_077b_a8b7), // 1e-125 * 2**543
    (0x8762_5f05_6c7c_4a8b, 0x1147_1cd7_64ad_4972), // 1e-124 * 2**539
    (0xa93a_f6c6_c79b_5d2d, 0xd598_e40d_3dd8_9bcf), // 1e-123 * 2**536
    (0xd389_b478_7982_3479, 0x4aff_1d10_8d4e_c2c3), // 1e-122 * 2**533
    (0x8436_10cb_4bf1_60cb, 0xcedf_722a_5851_39ba), // 1e-121 * 2**529
    (0xa543_94fe_1eed_b8fe, 0xc297_4eb4_ee65_8828), // 1e-120 * 2**526
    (0xce94_7a3d_a6a9_273e, 0x733d_2262_29fe_ea32), // 1e-119 * 2**523
    (0x811c_cc66_8829_b887, 0x0806_357d_5a3f_525f), // 1e-118 * 2**519
    (0xa163_ff80_2a34_26a8, 0xca07_c2dc_b0cf_26f7), // 1e-117 * 2**516
    (0xc9bc_ff60_34c1_3052, 0xfc89_b393_dd02_f0b5), // 1e-116 * 2**513
    (0xfc2c_3f38_41f1_7c67, 0xbbac_2078_d443_ace2), // 1e-115 * 2**510
    (0x9d9b_a783_2936_edc0, 0xd54b_944b_84aa_4c0d), // 1e-114 * 2**506
    (0xc502_9163_f384_a931, 0x0a9e_795e_65d4_df11), // 1e-113 * 2**503
    (0xf643_35bc_f065_d37d, 0x4d46_17b5_ff4a_16d5), // 1e-112 * 2**500
    (0x99ea_0196_163f_a42e, 0x504b_ced1_bf8e_4e45), // 1e-111 * 2**496
    (0xc064_81fb_9bcf_8d39, 0xe45e_c286_2f71_e1d6), // 1e-110 * 2**493
    (0xf07d_a27a_82c3_7088, 0x5d76_7327_bb4e_5a4c), // 1e-109 * 2**490
    (0x964e_858c_91ba_2655, 0x3a6a_07f8_d510_f86f), // 1e-108 * 2**486
    (0xbbe2_26ef_b628_afea, 0x8904_89f7_0a55_368b), // 1e-107 * 2**483
    (0xeada_b0ab_a3b2_dbe5, 0x2b45_ac74_ccea_842e), // 1e-106 * 2**480
    (0x92c8_ae6b_464f_c96f, 0x3b0b_8bc9_0012_929d), // 1e-105 * 2**476
    (0xb77a_da06_17e3_bbcb, 0x09ce_6ebb_4017_3744), // 1e-104 * 2**473
    (0xe559_9087_9ddc_aabd, 0xcc42_0a6a_101d_0515), // 1e-103 * 2**470
    (0x8f57_fa54_c2a9_eab6, 0x9fa9_4682_4a12_232d), // 1e-102 * 2**466
    (0xb32d_f8e9_f354_6564, 0x4793_9822_dc96_abf9), // 1e-101 * 2**463
    (0xdff9_7724_7029_7ebd, 0x5978_7e2b_93bc_56f7), // 1e-100 * 2**460
    (0x8bfb_ea76_c619_ef36, 0x57eb_4edb_3c55_b65a), // 1e-99 * 2**456
    (0xaefa_e514_77a0_6b03, 0xede6_2292_0b6b_23f1), // 1e-98 * 2**453
    (0xdab9_9e59_9588_85c4, 0xe95f_ab36_8e45_eced), // 1e-97 * 2**450
    (0x88b4_02f7_fd75_539b, 0x11db_cb02_18eb_b414), // 1e-96 * 2**446
    (0xaae1_03b5_fcd2_a881, 0xd652_bdc2_9f26_a119), // 1e-95 * 2**443
    (0xd599_44a3_7c07_52a2, 0x4be7_6d33_46f0_495f), // 1e-94 * 2**440
    (0x857f_cae6_2d84_93a5, 0x6f70_a440_0c56_2ddb), // 1e-93 * 2**436
    (0xa6df_bd9f_b8e5_b88e, 0xcb4c_cd50_0f6b_b952), // 1e-92 * 2**433
    (0xd097_ad07_a71f_26b2, 0x7e20_00a4_1346_a7a7), // 1e-91 * 2**430
    (0x825e_cc24_c873_782f, 0x8ed4_0066_8c0c_28c8), // 1e-90 * 2**426
    (0xa2f6_7f2d_fa90_563b, 0x7289_0080_2f0f_32fa), // 1e-89 * 2**423
    (0xcbb4_1ef9_7934_6bca, 0x4f2b_40a0_3ad2_ffb9), // 1e-88 * 2**420
    (0xfea1_26b7_d781_86bc, 0xe2f6_10c8_4987_bfa8), // 1e-87 * 2**417
    (0x9f24_b832_e6b0_f436, 0x0dd9_ca7d_2df4_d7c9), // 1e-86 * 2**413
    (0xc6ed_e63f_a05d_3143, 0x9150_3d1c_7972_0dbb), // 1e-85 * 2**410
    (0xf8a9_5fcf_8874_7d94, 0x75a4_4c63_97ce_912a), // 1e-84 * 2**407
    (0x9b69_dbe1_b548_ce7c, 0xc986_afbe_3ee1_1aba), // 1e-83 * 2**403
    (0xc244_52da_229b_021b, 0xfbe8_5bad_ce99_6168), // 1e-82 * 2**400
    (0xf2d5_6790_ab41_c2a2, 0xfae2_7299_423f_b9c3), // 1e-81 * 2**397
    (0x97c5_60ba_6b09_19a5, 0xdccd_879f_c967_d41a), // 1e-80 * 2**393
    (0xbdb6_b8e9_05cb_600f, 0x5400_e987_bbc1_c920), // 1e-79 * 2**390
    (0xed24_6723_473e_3813, 0x2901_23e9_aab2_3b68), // 1e-78 * 2**387
    (0x9436_c076_0c86_e30b, 0xf9a0_b672_0aaf_6521), // 1e-77 * 2**383
    (0xb944_7093_8fa8_9bce, 0xf808_e40e_8d5b_3e69), // 1e-76 * 2**380
    (0xe795_8cb8_7392_c2c2, 0xb60b_1d12_30b2_0e04), // 1e-75 * 2**377
    (0x90bd_77f3_483b_b9b9, 0xb1c6_f22b_5e6f_48c2), // 1e-74 * 2**373
    (0xb4ec_d5f0_1a4a_a828, 0x1e38_aeb6_360b_1af3), // 1e-73 * 2**370
    (0xe228_0b6c_20dd_5232, 0x25c6_da63_c38d_e1b0), // 1e-72 * 2**367
    (0x8d59_0723_948a_535f, 0x579c_487e_5a38_ad0e), // 1e-71 * 2**363
    (0xb0af_48ec_79ac_e837, 0x2d83_5a9d_f0c6_d851), // 1e-70 * 2**360
    (0xdcdb_1b27_9818_2244, 0xf8e4_3145_6cf8_8e65), // 1e-69 * 2**357
    (0x8a08_f0f8_bf0f_156b, 0x1b8e_9ecb_641b_58ff), // 1e-68 * 2**353
    (0xac8b_2d36_eed2_dac5, 0xe272_467e_3d22_2f3f), // 1e-67 * 2**350
    (0xd7ad_f884_aa87_9177, 0x5b0e_d81d_cc6a_bb0f), // 1e-66 * 2**347
    (0x86cc_bb52_ea94_baea, 0x98e9_4712_9fc2_b4e9), // 1e-65 * 2**343
    (0xa87f_ea27_a539_e9a5, 0x3f23_98d7_47b3_6224), // 1e-64 * 2**340
    (0xd29f_e4b1_8e88_640e, 0x8eec_7f0d_19a0_3aad), // 1e-63 * 2**337
    (0x83a3_eeee_f915_3e89, 0x1953_cf68_3004_24ac), // 1e-62 * 2**333
    (0xa48c_eaaa_b75a_8e2b, 0x5fa8_c342_3c05_2dd7), // 1e-61 * 2**330
    (0xcdb0_2555_6531_31b6, 0x3792_f412_cb06_794d), // 1e-60 * 2**327
    (0x808e_1755_5f3e_bf11, 0xe2bb_d88b_bee4_0bd0), // 1e-59 * 2**323
    (0xa0b1_9d2a_b70e_6ed6, 0x5b6a_ceae_ae9d_0ec4), // 1e-58 * 2**320
    (0xc8de_0475_64d2_0a8b, 0xf245_825a_5a44_5275), // 1e-57 * 2**317
    (0xfb15_8592_be06_8d2e, 0xeed6_e2f0_f0d5_6712), // 1e-56 * 2**314
    (0x9ced_737b_b6c4_183d, 0x5546_4dd6_9685_606b), // 1e-55 * 2**310
    (0xc428_d05a_a475_1e4c, 0xaa97_e14c_3c26_b886), // 1e-54 * 2**307
    (0xf533_0471_4d92_65df, 0xd53d_d99f_4b30_66a8), // 1e-53 * 2**304
    (0x993f_e2c6_d07b_7fab, 0xe546_a803_8efe_4029), // 1e-52 * 2**300
    (0xbf8f_db78_849a_5f96, 0xde98_5204_72bd_d033), // 1e-51 * 2**297
    (0xef73_d256_a5c0_f77c, 0x963e_6685_8f6d_4440), // 1e-50 * 2**294
    (0x95a8_6376_2798_9aad, 0xdde7_0013_79a4_4aa8), // 1e-49 * 2**290
    (0xbb12_7c53_b17e_c159, 0x5560_c018_580d_5d52), // 1e-48 * 2**287
    (0xe9d7_1b68_9dde_71af, 0xaab8_f01e_6e10_b4a6), // 1e-47 * 2**284
    (0x9226_7121_62ab_070d, 0xcab3_9613_04ca_70e8), // 1e-46 * 2**280
    (0xb6b0_0d69_bb55_c8d1, 0x3d60_7b97_c5fd_0d22), // 1e-45 * 2**277
    (0xe45c_10c4_2a2b_3b05, 0x8cb8_9a7d_b77c_506a), // 1e-44 * 2**274
    (0x8eb9_8a7a_9a5b_04e3, 0x77f3_608e_92ad_b242), // 1e-43 * 2**270
    (0xb267_ed19_40f1_c61c, 0x55f0_38b2_3759_1ed3), // 1e-42 * 2**267
    (0xdf01_e85f_912e_37a3, 0x6b6c_46de_c52f_6688), // 1e-41 * 2**264
    (0x8b61_313b_babc_e2c6, 0x2323_ac4b_3b3d_a015), // 1e-40 * 2**260
    (0xae39_7d8a_a96c_1b77, 0xabec_975e_0a0d_081a), // 1e-39 * 2**257
    (0xd9c7_dced_53c7_2255, 0x96e7_bd35_8c90_4a21), // 1e-38 * 2**254
    (0x881c_ea14_545c_7575, 0x7e50_d641_77da_2e54), // 1e-37 * 2**250
    (0xaa24_2499_6973_92d2, 0xdde5_0bd1_d5d0_b9e9), // 1e-36 * 2**247
    (0xd4ad_2dbf_c3d0_7787, 0x955e_4ec6_4b44_e864), // 1e-35 * 2**244
    (0x84ec_3c97_da62_4ab4, 0xbd5a_f13b_ef0b_113e), // 1e-34 * 2**240
    (0xa627_4bbd_d0fa_dd61, 0xecb1_ad8a_eacd_d58e), // 1e-33 * 2**237
    (0xcfb1_1ead_4539_94ba, 0x67de_18ed_a581_4af2), // 1e-32 * 2**234
    (0x81ce_b32c_4b43_fcf4, 0x80ea_cf94_8770_ced7), // 1e-31 * 2**230
    (0xa242_5ff7_5e14_fc31, 0xa125_8379_a94d_028d), // 1e-30 * 2**227
    (0xcad2_f7f5_359a_3b3e, 0x096e_e458_13a0_4330), // 1e-29 * 2**224
    (0xfd87_b5f2_8300_ca0d, 0x8bca_9d6e_1888_53fc), // 1e-28 * 2**221
    (0x9e74_d1b7_91e0_7e48, 0x775e_a264_cf55_347d), // 1e-27 * 2**217
    (0xc612_0625_7658_9dda, 0x9536_4afe_032a_819d), // 1e-26 * 2**214
    (0xf796_87ae_d3ee_c551, 0x3a83_ddbd_83f5_2204), // 1e-25 * 2**211
    (0x9abe_14cd_4475_3b52, 0xc492_6a96_7279_3542), // 1e-24 * 2**207
    (0xc16d_9a00_9592_8a27, 0x75b7_053c_0f17_8293), // 1e-23 * 2**204
    (0xf1c9_0080_baf7_2cb1, 0x5324_c68b_12dd_6338), // 1e-22 * 2**201
    (0x971d_a050_74da_7bee, 0xd3f6_fc16_ebca_5e03), // 1e-21 * 2**197
    (0xbce5_0864_9211_1aea, 0x88f4_bb1c_a6bc_f584), // 1e-20 * 2**194
    (0xec1e_4a7d_b695_61a5, 0x2b31_e9e3_d06c_32e5), // 1e-19 * 2**191
    (0x9392_ee8e_921d_5d07, 0x3aff_322e_6243_9fcf), // 1e-18 * 2**187
    (0xb877_aa32_36a4_b449, 0x09be_feb9_fad4_87c2), // 1e-17 * 2**184
    (0xe695_94be_c44d_e15b, 0x4c2e_be68_7989_a9b3), // 1e-16 * 2**181
    (0x901d_7cf7_3ab0_acd9, 0x0f9d_3701_4bf6_0a10), // 1e-15 * 2**177
    (0xb424_dc35_095c_d80f, 0x5384_84c1_9ef3_8c94), // 1e-14 * 2**174
    (0xe12e_1342_4bb4_0e13, 0x2865_a5f2_06b0_6fb9), // 1e-13 * 2**171
    (0x8cbc_cc09_6f50_88cb, 0xf93f_87b7_442e_45d3), // 1e-12 * 2**167
    (0xafeb_ff0b_cb24_aafe, 0xf78f_69a5_1539_d748), // 1e-11 * 2**164
    (0xdbe6_fece_bded_d5be, 0xb573_440e_5a88_4d1b), // 1e-10 * 2**161
    (0x8970_5f41_36b4_a597, 0x3168_0a88_f895_3030), // 1e-9 * 2**157
    (0xabcc_7711_8461_cefc, 0xfdc2_0d2b_36ba_7c3d), // 1e-8 * 2**154
    (0xd6bf_94d5_e57a_42bc, 0x3d32_9076_0469_1b4c), // 1e-7 * 2**151
    (0x8637_bd05_af6c_69b5, 0xa63f_9a49_c2c1_b10f), // 1e-6 * 2**147
    (0xa7c5_ac47_1b47_8423, 0x0fcf_80dc_3372_1d53), // 1e-5 * 2**144
    (0xd1b7_1758_e219_652b, 0xd3c3_6113_404e_a4a8), // 1e-4 * 2**141
    (0x8312_6e97_8d4f_df3b, 0x645a_1cac_0831_26e9), // 1e-3 * 2**137
    (0xa3d7_0a3d_70a3_d70a, 0x3d70_a3d7_0a3d_70a3), // 1e-2 * 2**134
    (0xcccc_cccc_cccc_cccc, 0xcccc_cccc_cccc_cccc), // 1e-1 * 2**131
    (0x8000_0000_0000_0000, 0x0000_0000_0000_0000), // 1e0 * 2**127
    (0xa000_0000_0000_0000, 0x0000_0000_0000_0000), // 1e1 * 2**124
    (0xc800_0000_0000_0000, 0x0000_0000_0000_0000), // 1e2 * 2**121
    (0xfa00_0000_0000_0000, 0x0000_0000_0000_0000), // 1e3 * 2**118
    (0x9c40_0000_0000_0000, 0x0000_0000_0000_0000), // 1e4 * 2**114
    (0xc350_0000_0000_0000, 0x0000_0000_0000_0000), // 1e5 * 2**111
    (0xf424_0000_0000_0000, 0x0000_0000_0000_0000), // 1e6 * 2**108
    (0x9896_8000_0000_0000, 0x0000_0000_0000_0000), // 1e7 * 2**104
    (0xbebc_2000_0000_0000, 0x0000_0000_0000_0000), // 1e8 * 2**101
    (0xee6b_2800_0000_0000, 0x0000_0000_0000_0000), // 1e9 * 2**98
    (0x9502_f900_0000_0000, 0x0000_0000_0000_0000), // 1e10 * 2**94
    (0xba43_b740_0000_0000, 0x0000_0000_0000_0000), // 1e11 * 2**91
    (0xe8d4_a510_0000_0000, 0x0000_0000_0000_0000), // 1e12 * 2**88
    (0x9184_e72a_0000_0000, 0x0000_0000_0000_0000), // 1e13 * 2**84
    (0xb5e6_20f4_8000_0000, 0x0000_0000_0000_0000), // 1e14 * 2**81
    (0xe35f_a931_a000_0000, 0x0000_0000_0000_0000), // 1e15 * 2**78
    (0x8e1b_c9bf_0400_0000, 0x0000_0000_0000_0000), // 1e16 * 2**74
    (0xb1a2_bc2e_c500_0000, 0x0000_0000_0000_0000), // 1e17 * 2**71
    (0xde0b_6b3a_7640_0000, 0x0000_0000_0000_0000), // 1e18 * 2**68
    (0x8ac7_2304_89e8_0000, 0x0000_0000_0000_0000), // 1e19 * 2**64
    (0xad78_ebc5_ac62_0000, 0x0000_0000_0000_0000), // 1e20 * 2**61
    (0xd8d7_26b7_177a_8000, 0x0000_0000_0000_0000), // 1e21 * 2**58
    (0x8786_7832_6eac_9000, 0x0000_0000_0000_0000), // 1e22 * 2**54
    (0xa968_163f_0a57_b400, 0x0000_0000_0000_0000), // 1e23 * 2**51
    (0xd3c2_1bce_cced_a100, 0x0000_0000_0000_0000), // 1e24 * 2**48
    (0x8459_5161_4014_84a0, 0x0000_0000_0000_0000), // 1e25 * 2**44
    (0xa56f_a5b9_9019_a5c8, 0x0000_0000_0000_0000), // 1e26 * 2**41
    (0xcecb_8f27_f420_0f3a, 0x0000_0000_0000_0000), // 1e27 * 2**38
    (0x813f_3978_f894_0984, 0x4000_0000_0000_0000), // 1e28 * 2**34
    (0xa18f_07d7_36b9_0be5, 0x5000_0000_0000_0000), // 1e29 * 2**31
    (0xc9f2_c9cd_0467_4ede, 0xa400_0000_0000_0000), // 1e30 * 2**28
    (0xfc6f_7c40_4581_2296, 0x4d00_0000_0000_0000), // 1e31 * 2**25
    (0x9dc5_ada8_2b70_b59d, 0xf020_0000_0000_0000), // 1e32 * 2**21
    (0xc537_1912_364c_e305, 0x6c28_0000_0000_0000), // 1e33 * 2**18
    (0xf684_df56_c3e0_1bc6, 0xc732_0000_0000_0000), // 1e34 * 2**15
    (0x9a13_0b96_3a6c_115c, 0x3c7f_4000_0000_0000), // 1e35 * 2**11
    (0xc097_ce7b_c907_15b3, 0x4b9f_1000_0000_0000), // 1e36 * 2**8
    (0xf0bd_c21a_bb48_db20, 0x1e86_d400_0000_0000), // 1e37 * 2**5
    (0x9676_9950_b50d_88f4, 0x1314_4480_0000_0000), // 1e38 * 2**1
    (0xbc14_3fa4_e250_eb31, 0x17d9_55a0_0000_0000), // 1e39 * 2**-2
    (0xeb19_4f8e_1ae5_25fd, 0x5dcf_ab08_0000_0000), // 1e40 * 2**-5
    (0x92ef_d1b8_d0cf_37be, 0x5aa1_cae5_0000_0000), // 1e41 * 2**-9
    (0xb7ab_c627_0503_05ad, 0xf14a_3d9e_4000_0000), // 1e42 * 2**-12
    (0xe596_b7b0_c643_c719, 0x6d9c_cd05_d000_0000), // 1e43 * 2**-15
    (0x8f7e_32ce_7bea_5c6f, 0xe482_0023_a200_0000), // 1e44 * 2**-19
    (0xb35d_bf82_1ae4_f38b, 0xdda2_802c_8a80_0000), // 1e45 * 2**-22
    (0xe035_2f62_a19e_306e, 0xd50b_2037_ad20_0000), // 1e46 * 2**-25
    (0x8c21_3d9d_a502_de45, 0x4526_f422_cc34_0000), // 1e47 * 2**-29
    (0xaf29_8d05_0e43_95d6, 0x9670_b12b_7f41_0000), // 1e48 * 2**-32
    (0xdaf3_f046_51d4_7b4c, 0x3c0c_dd76_5f11_4000), // 1e49 * 2**-35
    (0x88d8_762b_f324_cd0f, 0xa588_0a69_fb6a_c800), // 1e50 * 2**-39
    (0xab0e_93b6_efee_0053, 0x8eea_0d04_7a45_7a00), // 1e51 * 2**-42
    (0xd5d2_38a4_abe9_8068, 0x72a4_9045_98d6_d880), // 1e52 * 2**-45
    (0x85a3_6366_eb71_f041, 0x47a6_da2b_7f86_4750), // 1e53 * 2**-49
    (0xa70c_3c40_a64e_6c51, 0x9990_90b6_5f67_d924), // 1e54 * 2**-52
    (0xd0cf_4b50_cfe2_0765, 0xfff4_b4e3_f741_cf6d), // 1e55 * 2**-55
    (0x8281_8f12_81ed_449f, 0xbff8_f10e_7a89_21a4), // 1e56 * 2**-59
    (0xa321_f2d7_2268_95c7, 0xaff7_2d52_192b_6a0d), // 1e57 * 2**-62
    (0xcbea_6f8c_eb02_bb39, 0x9bf4_f8a6_9f76_4490), // 1e58 * 2**-65
    (0xfee5_0b70_25c3_6a08, 0x02f2_36d0_4753_d5b4), // 1e59 * 2**-68
    (0x9f4f_2726_179a_2245, 0x01d7_6242_2c94_6590), // 1e60 * 2**-72
    (0xc722_f0ef_9d80_aad6, 0x424d_3ad2_b7b9_7ef5), // 1e61 * 2**-75
    (0xf8eb_ad2b_84e0_d58b, 0xd2e0_8987_65a7_deb2), // 1e62 * 2**-78
    (0x9b93_4c3b_330c_8577, 0x63cc_55f4_9f88_eb2f), // 1e63 * 2**-82
    (0xc278_1f49_ffcf_a6d5, 0x3cbf_6b71_c76b_25fb), // 1e64 * 2**-85
    (0xf316_271c_7fc3_908a, 0x8bef_464e_3945_ef7a), // 1e65 * 2**-88
    (0x97ed_d871_cfda_3a56, 0x9775_8bf0_e3cb_b5ac), // 1e66 * 2**-92
    (0xbde9_4e8e_43d0_c8ec, 0x3d52_eeed_1cbe_a317), // 1e67 * 2**-95
    (0xed63_a231_d4c4_fb27, 0x4ca7_aaa8_63ee_4bdd), // 1e68 * 2**-98
    (0x945e_455f_24fb_1cf8, 0x8fe8_caa9_3e74_ef6a), // 1e69 * 2**-102
    (0xb975_d6b6_ee39_e436, 0xb3e2_fd53_8e12_2b44), // 1e70 * 2**-105
    (0xe7d3_4c64_a9c8_5d44, 0x60db_bca8_7196_b616), // 1e71 * 2**-108
    (0x90e4_0fbe_ea1d_3a4a, 0xbc89_55e9_46fe_31cd), // 1e72 * 2**-112
    (0xb51d_13ae_a4a4_88dd, 0x6bab_ab63_98bd_be41), // 1e73 * 2**-115
    (0xe264_589a_4dcd_ab14, 0xc696_963c_7eed_2dd1), // 1e74 * 2**-118
    (0x8d7e_b760_70a0_8aec, 0xfc1e_1de5_cf54_3ca2), // 1e75 * 2**-122
    (0xb0de_6538_8cc8_ada8, 0x3b25_a55f_4329_4bcb), // 1e76 * 2**-125
    (0xdd15_fe86_affa_d912, 0x49ef_0eb7_13f3_9ebe), // 1e77 * 2**-128
    (0x8a2d_bf14_2dfc_c7ab, 0x6e35_6932_6c78_4337), // 1e78 * 2**-132
    (0xacb9_2ed9_397b_f996, 0x49c2_c37f_0796_5404), // 1e79 * 2**-135
    (0xd7e7_7a8f_87da_f7fb, 0xdc33_745e_c97b_e906), // 1e80 * 2**-138
    (0x86f0_ac99_b4e8_dafd, 0x69a0_28bb_3ded_71a3), // 1e81 * 2**-142
    (0xa8ac_d7c0_2223_11bc, 0xc408_32ea_0d68_ce0c), // 1e82 * 2**-145
    (0xd2d8_0db0_2aab_d62b, 0xf50a_3fa4_90c3_0190), // 1e83 * 2**-148
    (0x83c7_088e_1aab_65db, 0x7926_67c6_da79_e0fa), // 1e84 * 2**-152
    (0xa4b8_cab1_a156_3f52, 0x5770_01b8_9118_5938), // 1e85 * 2**-155
    (0xcde6_fd5e_09ab_cf26, 0xed4c_0226_b55e_6f86), // 1e86 * 2**-158
    (0x80b0_5e5a_c60b_6178, 0x544f_8158_315b_05b4), // 1e87 * 2**-162
    (0xa0dc_75f1_778e_39d6, 0x6963_61ae_3db1_c721), // 1e88 * 2**-165
    (0xc913_936d_d571_c84c, 0x03bc_3a19_cd1e_38e9), // 1e89 * 2**-168
    (0xfb58_7849_4ace_3a5f, 0x04ab_48a0_4065_c723), // 1e90 * 2**-171
    (0x9d17_4b2d_cec0_e47b, 0x62eb_0d64_283f_9c76), // 1e91 * 2**-175
    (0xc45d_1df9_4271_1d9a, 0x3ba5_d0bd_324f_8394), // 1e92 * 2**-178
    (0xf574_6577_930d_6500, 0xca8f_44ec_7ee3_6479), // 1e93 * 2**-181
    (0x9968_bf6a_bbe8_5f20, 0x7e99_8b13_cf4e_1ecb), // 1e94 * 2**-185
    (0xbfc2_ef45_6ae2_76e8, 0x9e3f_edd8_c321_a67e), // 1e95 * 2**-188
    (0xefb3_ab16_c59b_14a2, 0xc5cf_e94e_f3ea_101e), // 1e96 * 2**-191
    (0x95d0_4aee_3b80_ece5, 0xbba1_f1d1_5872_4a12), // 1e97 * 2**-195
    (0xbb44_5da9_ca61_281f, 0x2a8a_6e45_ae8e_dc97), // 1e98 * 2**-198
    (0xea15_7514_3cf9_7226, 0xf52d_09d7_1a32_93bd), // 1e99 * 2**-201
    (0x924d_692c_a61b_e758, 0x593c_2626_705f_9c56), // 1e100 * 2**-205
    (0xb6e0_c377_cfa2_e12e, 0x6f8b_2fb0_0c77_836c), // 1e101 * 2**-208
    (0xe498_f455_c38b_997a, 0x0b6d_fb9c_0f95_6447), // 1e102 * 2**-211
    (0x8edf_98b5_9a37_3fec, 0x4724_bd41_89bd_5eac), // 1e103 * 2**-215
    (0xb297_7ee3_00c5_0fe7, 0x58ed_ec91_ec2c_b657), // 1e104 * 2**-218
    (0xdf3d_5e9b_c0f6_53e1, 0x2f29_67b6_6737_e3ed), // 1e105 * 2**-221
    (0x8b86_5b21_5899_f46c, 0xbd79_e0d2_0082_ee74), // 1e106 * 2**-225
    (0xae67_f1e9_aec0_7187, 0xecd8_5906_80a3_aa11), // 1e107 * 2**-228
    (0xda01_ee64_1a70_8de9, 0xe80e_6f48_20cc_9495), // 1e108 * 2**-231
    (0x8841_34fe_9086_58b2, 0x3109_058d_147f_dcdd), // 1e109 * 2**-235
    (0xaa51_823e_34a7_eede, 0xbd4b_46f0_599f_d415), // 1e110 * 2**-238
    (0xd4e5_e2cd_c1d1_ea96, 0x6c9e_18ac_7007_c91a), // 1e111 * 2**-241
    (0x850f_adc0_9923_329e, 0x03e2_cf6b_c604_ddb0), // 1e112 * 2**-245
    (0xa653_9930_bf6b_ff45, 0x84db_8346_b786_151c), // 1e113 * 2**-248
    (0xcfe8_7f7c_ef46_ff16, 0xe612_6418_6567_9a63), // 1e114 * 2**-251
    (0x81f1_4fae_158c_5f6e, 0x4fcb_7e8f_3f60_c07e), // 1e115 * 2**-255
    (0xa26d_a399_9aef_7749, 0xe3be_5e33_0f38_f09d), // 1e116 * 2**-258
    (0xcb09_0c80_01ab_551c, 0x5cad_f5bf_d307_2cc5), // 1e117 * 2**-261
    (0xfdcb_4fa0_0216_2a63, 0x73d9_732f_c7c8_f7f6), // 1e118 * 2**-264
    (0x9e9f_11c4_014d_da7e, 0x2867_e7fd_dcdd_9afa), // 1e119 * 2**-268
    (0xc646_d635_01a1_511d, 0xb281_e1fd_5415_01b8), // 1e120 * 2**-271
    (0xf7d8_8bc2_4209_a565, 0x1f22_5a7c_a91a_4226), // 1e121 * 2**-274
    (0x9ae7_5759_6946_075f, 0x3375_788d_e9b0_6958), // 1e122 * 2**-278
    (0xc1a1_2d2f_c397_8937, 0x0052_d6b1_641c_83ae), // 1e123 * 2**-281
    (0xf209_787b_b47d_6b84, 0xc067_8c5d_bd23_a49a), // 1e124 * 2**-284
    (0x9745_eb4d_50ce_6332, 0xf840_b7ba_9636_46e0), // 1e125 * 2**-288
    (0xbd17_6620_a501_fbff, 0xb650_e5a9_3bc3_d898), // 1e126 * 2**-291
    (0xec5d_3fa8_ce42_7aff, 0xa3e5_1f13_8ab4_cebe), // 1e127 * 2**-294
    (0x93ba_47c9_80e9_8cdf, 0xc66f_336c_36b1_0137), // 1e128 * 2**-298
    (0xb8a8_d9bb_e123_f017, 0xb80b_0047_445d_4184), // 1e129 * 2**-301
    (0xe6d3_102a_d96c_ec1d, 0xa60d_c059_1574_91e5), // 1e130 * 2**-304
    (0x9043_ea1a_c7e4_1392, 0x87c8_9837_ad68_db2f), // 1e131 * 2**-308
    (0xb454_e4a1_79dd_1877, 0x29ba_be45_98c3_11fb), // 1e132 * 2**-311
    (0xe16a_1dc9_d854_5e94, 0xf429_6dd6_fef3_d67a), // 1e133 * 2**-314
    (0x8ce2_529e_2734_bb1d, 0x1899_e4a6_5f58_660c), // 1e134 * 2**-318
    (0xb01a_e745_b101_e9e4, 0x5ec0_5dcf_f72e_7f8f), // 1e135 * 2**-321
    (0xdc21_a117_1d42_645d, 0x7670_7543_f4fa_1f73), // 1e136 * 2**-324
    (0x8995_04ae_7249_7eba, 0x6a06_494a_791c_53a8), // 1e137 * 2**-328
    (0xabfa_45da_0edb_de69, 0x0487_db9d_1763_6892), // 1e138 * 2**-331
    (0xd6f8_d750_9292_d603, 0x45a9_d284_5d3c_42b6), // 1e139 * 2**-334
    (0x865b_8692_5b9b_c5c2, 0x0b8a_2392_ba45_a9b2), // 1e140 * 2**-338
    (0xa7f2_6836_f282_b732, 0x8e6c_ac77_68d7_141e), // 1e141 * 2**-341
    (0xd1ef_0244_af23_64ff, 0x3207_d795_430c_d926), // 1e142 * 2**-344
    (0x8335_616a_ed76_1f1f, 0x7f44_e6bd_49e8_07b8), // 1e143 * 2**-348
    (0xa402_b9c5_a8d3_a6e7, 0x5f16_206c_9c62_09a6), // 1e144 * 2**-351
    (0xcd03_6837_1308_90a1, 0x36db_a887_c37a_8c0f), // 1e145 * 2**-354
    (0x8022_2122_6be5_5a64, 0xc249_4954_da2c_9789), // 1e146 * 2**-358
    (0xa02a_a96b_06de_b0fd, 0xf2db_9baa_10b7_bd6c), // 1e147 * 2**-361
    (0xc835_53c5_c896_5d3d, 0x6f92_8294_94e5_acc7), // 1e148 * 2**-364
    (0xfa42_a8b7_3abb_f48c, 0xcb77_2339_ba1f_17f9), // 1e149 * 2**-367
    (0x9c69_a972_84b5_78d7, 0xff2a_7604_1453_6efb), // 1e150 * 2**-371
    (0xc384_13cf_25e2_d70d, 0xfef5_1385_1968_4aba), // 1e151 * 2**-374
    (0xf465_18c2_ef5b_8cd1, 0x7eb2_5866_5fc2_5d69), // 1e152 * 2**-377
    (0x98bf_2f79_d599_3802, 0xef2f_773f_fbd9_7a61), // 1e153 * 2**-381
    (0xbeee_fb58_4aff_8603, 0xaafb_550f_facf_d8fa), // 1e154 * 2**-384
    (0xeeaa_ba2e_5dbf_6784, 0x95ba_2a53_f983_cf38), // 1e155 * 2**-387
    (0x952a_b45c_fa97_a0b2, 0xdd94_5a74_7bf2_6183), // 1e156 * 2**-391
    (0xba75_6174_393d_88df, 0x94f9_7111_9aee_f9e4), // 1e157 * 2**-394
    (0xe912_b9d1_478c_eb17, 0x7a37_cd56_01aa_b85d), // 1e158 * 2**-397
    (0x91ab_b422_ccb8_12ee, 0xac62_e055_c10a_b33a), // 1e159 * 2**-401
    (0xb616_a12b_7fe6_17aa, 0x577b_986b_314d_6009), // 1e160 * 2**-404
    (0xe39c_4976_5fdf_9d94, 0xed5a_7e85_fda0_b80b), // 1e161 * 2**-407
    (0x8e41_ade9_fbeb_c27d, 0x1458_8f13_be84_7307), // 1e162 * 2**-411
    (0xb1d2_1964_7ae6_b31c, 0x596e_b2d8_ae25_8fc8), // 1e163 * 2**-414
    (0xde46_9fbd_99a0_5fe3, 0x6fca_5f8e_d9ae_f3bb), // 1e164 * 2**-417
    (0x8aec_23d6_8004_3bee, 0x25de_7bb9_480d_5854), // 1e165 * 2**-421
    (0xada7_2ccc_2005_4ae9, 0xaf56_1aa7_9a10_ae6a), // 1e166 * 2**-424
    (0xd910_f7ff_2806_9da4, 0x1b2b_a151_8094_da04), // 1e167 * 2**-427
    (0x87aa_9aff_7904_2286, 0x90fb_44d2_f05d_0842), // 1e168 * 2**-431
    (0xa995_41bf_5745_2b28, 0x353a_1607_ac74_4a53), // 1e169 * 2**-434
    (0xd3fa_922f_2d16_75f2, 0x4288_9b89_9791_5ce8), // 1e170 * 2**-437
    (0x847c_9b5d_7c2e_09b7, 0x6995_6135_feba_da11), // 1e171 * 2**-441
    (0xa59b_c234_db39_8c25, 0x43fa_b983_7e69_9095), // 1e172 * 2**-444
    (0xcf02_b2c2_1207_ef2e, 0x94f9_67e4_5e03_f4bb), // 1e173 * 2**-447
    (0x8161_afb9_4b44_f57d, 0x1d1b_e0ee_bac2_78f5), // 1e174 * 2**-451
    (0xa1ba_1ba7_9e16_32dc, 0x6462_d92a_6973_1732), // 1e175 * 2**-454
    (0xca28_a291_859b_bf93, 0x7d7b_8f75_03cf_dcfe), // 1e176 * 2**-457
    (0xfcb2_cb35_e702_af78, 0x5cda_7352_44c3_d43e), // 1e177 * 2**-460
    (0x9def_bf01_b061_adab, 0x3a08_8813_6afa_64a7), // 1e178 * 2**-464
    (0xc56b_aec2_1c7a_1916, 0x088a_aa18_45b8_fdd0), // 1e179 * 2**-467
    (0xf6c6_9a72_a398_9f5b, 0x8aad_549e_5727_3d45), // 1e180 * 2**-470
    (0x9a3c_2087_a63f_6399, 0x36ac_54e2_f678_864b), // 1e181 * 2**-474
    (0xc0cb_28a9_8fcf_3c7f, 0x8457_6a1b_b416_a7dd), // 1e182 * 2**-477
    (0xf0fd_f2d3_f3c3_0b9f, 0x656d_44a2_a11c_51d5), // 1e183 * 2**-480
    (0x969e_b7c4_7859_e743, 0x9f64_4ae5_a4b1_b325), // 1e184 * 2**-484
    (0xbc46_65b5_9670_6114, 0x873d_5d9f_0dde_1fee), // 1e185 * 2**-487
    (0xeb57_ff22_fc0c_7959, 0xa90c_b506_d155_a7ea), // 1e186 * 2**-490
    (0x9316_ff75_dd87_cbd8, 0x09a7_f124_42d5_88f2), // 1e187 * 2**-494
    (0xb7dc_bf53_54e9_bece, 0x0c11_ed6d_538a_eb2f), // 1e188 * 2**-497
    (0xe5d3_ef28_2a24_2e81, 0x8f16_68c8_a86d_a5fa), // 1e189 * 2**-500
    (0x8fa4_7579_1a56_9d10, 0xf96e_017d_6944_87bc), // 1e190 * 2**-504
    (0xb38d_92d7_60ec_4455, 0x37c9_81dc_c395_a9ac), // 1e191 * 2**-507
    (0xe070_f78d_3927_556a, 0x85bb_e253_f47b_1417), // 1e192 * 2**-510
    (0x8c46_9ab8_43b8_9562, 0x9395_6d74_78cc_ec8e), // 1e193 * 2**-514
    (0xaf58_4166_54a6_babb, 0x387a_c8d1_9700_27b2), // 1e194 * 2**-517
    (0xdb2e_51bf_e9d0_696a, 0x0699_7b05_fcc0_319e), // 1e195 * 2**-520
    (0x88fc_f317_f222_41e2, 0x441f_ece3_bdf8_1f03), // 1e196 * 2**-524
    (0xab3c_2fdd_eeaa_d25a, 0xd527_e81c_ad76_26c3), // 1e197 * 2**-527
    (0xd60b_3bd5_6a55_86f1, 0x8a71_e223_d8d3_b074), // 1e198 * 2**-530
    (0x85c7_0565_6275_7456, 0xf687_2d56_6784_4e49), // 1e199 * 2**-534
    (0xa738_c6be_bb12_d16c, 0xb428_f8ac_0165_61db), // 1e200 * 2**-537
    (0xd106_f86e_69d7_85c7, 0xe133_36d7_01be_ba52), // 1e201 * 2**-540
    (0x82a4_5b45_0226_b39c, 0xecc0_0246_6117_3473), // 1e202 * 2**-544
    (0xa34d_7216_42b0_6084, 0x27f0_02d7_f95d_0190), // 1e203 * 2**-547
    (0xcc20_ce9b_d35c_78a5, 0x31ec_038d_f7b4_41f4), // 1e204 * 2**-550
    (0xff29_0242_c833_96ce, 0x7e67_0471_75a1_5271), // 1e205 * 2**-553
    (0x9f79_a169_bd20_3e41, 0x0f00_62c6_e984_d386), // 1e206 * 2**-557
    (0xc758_09c4_2c68_4dd1, 0x52c0_7b78_a3e6_0868), // 1e207 * 2**-560
    (0xf92e_0c35_3782_6145, 0xa770_9a56_ccdf_8a82), // 1e208 * 2**-563
    (0x9bbc_c7a1_42b1_7ccb, 0x88a6_6076_400b_b691), // 1e209 * 2**-567
    (0xc2ab_f989_935d_dbfe, 0x6acf_f893_d00e_a435), // 1e210 * 2**-570
    (0xf356_f7eb_f835_52fe, 0x0583_f6b8_c412_4d43), // 1e211 * 2**-573
    (0x9816_5af3_7b21_53de, 0xc372_7a33_7a8b_704a), // 1e212 * 2**-577
    (0xbe1b_f1b0_59e9_a8d6, 0x744f_18c0_592e_4c5c), // 1e213 * 2**-580
    (0xeda2_ee1c_7064_130c, 0x1162_def0_6f79_df73), // 1e214 * 2**-583
    (0x9485_d4d1_c63e_8be7, 0x8add_cb56_45ac_2ba8), // 1e215 * 2**-587
    (0xb9a7_4a06_37ce_2ee1, 0x6d95_3e2b_d717_3692), // 1e216 * 2**-590
    (0xe811_1c87_c5c1_ba99, 0xc8fa_8db6_ccdd_0437), // 1e217 * 2**-593
    (0x910a_b1d4_db99_14a0, 0x1d9c_9892_400a_22a2), // 1e218 * 2**-597
    (0xb54d_5e4a_127f_59c8, 0x2503_beb6_d00c_ab4b), // 1e219 * 2**-600
    (0xe2a0_b5dc_971f_303a, 0x2e44_ae64_840f_d61d), // 1e220 * 2**-603
    (0x8da4_71a9_de73_7e24, 0x5cea_ecfe_d289_e5d2), // 1e221 * 2**-607
    (0xb10d_8e14_5610_5dad, 0x7425_a83e_872c_5f47), // 1e222 * 2**-610
    (0xdd50_f199_6b94_7518, 0xd12f_124e_28f7_7719), // 1e223 * 2**-613
    (0x8a52_96ff_e33c_c92f, 0x82bd_6b70_d99a_aa6f), // 1e224 * 2**-617
    (0xace7_3cbf_dc0b_fb7b, 0x636c_c64d_1001_550b), // 1e225 * 2**-620
    (0xd821_0bef_d30e_fa5a, 0x3c47_f7e0_5401_aa4e), // 1e226 * 2**-623
    (0x8714_a775_e3e9_5c78, 0x65ac_faec_3481_0a71), // 1e227 * 2**-627
    (0xa8d9_d153_5ce3_b396, 0x7f18_39a7_41a1_4d0d), // 1e228 * 2**-630
    (0xd310_45a8_341c_a07c, 0x1ede_4811_1209_a050), // 1e229 * 2**-633
    (0x83ea_2b89_2091_e44d, 0x934a_ed0a_ab46_0432), // 1e230 * 2**-637
    (0xa4e4_b66b_68b6_5d60, 0xf81d_a84d_5617_853f), // 1e231 * 2**-640
    (0xce1d_e406_42e3_f4b9, 0x3625_1260_ab9d_668e), // 1e232 * 2**-643
    (0x80d2_ae83_e9ce_78f3, 0xc1d7_2b7c_6b42_6019), // 1e233 * 2**-647
    (0xa107_5a24_e442_1730, 0xb24c_f65b_8612_f81f), // 1e234 * 2**-650
    (0xc949_30ae_1d52_9cfc, 0xdee0_33f2_6797_b627), // 1e235 * 2**-653
    (0xfb9b_7cd9_a4a7_443c, 0x1698_40ef_017d_a3b1), // 1e236 * 2**-656
    (0x9d41_2e08_06e8_8aa5, 0x8e1f_2895_60ee_864e), // 1e237 * 2**-660
    (0xc491_798a_08a2_ad4e, 0xf1a6_f2ba_b92a_27e2), // 1e238 * 2**-663
    (0xf5b5_d7ec_8acb_58a2, 0xae10_af69_6774_b1db), // 1e239 * 2**-666
    (0x9991_a6f3_d6bf_1765, 0xacca_6da1_e0a8_ef29), // 1e240 * 2**-670
    (0xbff6_10b0_cc6e_dd3f, 0x17fd_090a_58d3_2af3), // 1e241 * 2**-673
    (0xeff3_94dc_ff8a_948e, 0xddfc_4b4c_ef07_f5b0), // 1e242 * 2**-676
    (0x95f8_3d0a_1fb6_9cd9, 0x4abd_af10_1564_f98e), // 1e243 * 2**-680
    (0xbb76_4c4c_a7a4_440f, 0x9d6d_1ad4_1abe_37f1), // 1e244 * 2**-683
    (0xea53_df5f_d18d_5513, 0x84c8_6189_216d_c5ed), // 1e245 * 2**-686
    (0x9274_6b9b_e2f8_552c, 0x32fd_3cf5_b4e4_9bb4), // 1e246 * 2**-690
    (0xb711_8682_dbb6_6a77, 0x3fbc_8c33_221d_c2a1), // 1e247 * 2**-693
    (0xe4d5_e823_92a4_0515, 0x0fab_af3f_eaa5_334a), // 1e248 * 2**-696
    (0x8f05_b116_3ba6_832d, 0x29cb_4d87_f2a7_400e), // 1e249 * 2**-700
    (0xb2c7_1d5b_ca90_23f8, 0x743e_20e9_ef51_1012), // 1e250 * 2**-703
    (0xdf78_e4b2_bd34_2cf6, 0x914d_a924_6b25_5416), // 1e251 * 2**-706
    (0x8bab_8eef_b640_9c1a, 0x1ad0_89b6_c2f7_548e), // 1e252 * 2**-710
    (0xae96_72ab_a3d0_c320, 0xa184_ac24_73b5_29b1), // 1e253 * 2**-713
    (0xda3c_0f56_8cc4_f3e8, 0xc9e5_d72d_90a2_741e), // 1e254 * 2**-716
    (0x8865_8996_17fb_1871, 0x7e2f_a67c_7a65_8892), // 1e255 * 2**-720
    (0xaa7e_ebfb_9df9_de8d, 0xddbb_901b_98fe_eab7), // 1e256 * 2**-723
    (0xd51e_a6fa_8578_5631, 0x552a_7422_7f3e_a565), // 1e257 * 2**-726
    (0x8533_285c_936b_35de, 0xd53a_8895_8f87_275f), // 1e258 * 2**-730
    (0xa67f_f273_b846_0356, 0x8a89_2aba_f368_f137), // 1e259 * 2**-733
    (0xd01f_ef10_a657_842c, 0x2d2b_7569_b043_2d85), // 1e260 * 2**-736
    (0x8213_f56a_67f6_b29b, 0x9c3b_2962_0e29_fc73), // 1e261 * 2**-740
    (0xa298_f2c5_01f4_5f42, 0x8349_f3ba_91b4_7b8f), // 1e262 * 2**-743
    (0xcb3f_2f76_4271_7713, 0x241c_70a9_3621_9a73), // 1e263 * 2**-746
    (0xfe0e_fb53_d30d_d4d7, 0xed23_8cd3_83aa_0110), // 1e264 * 2**-749
    (0x9ec9_5d14_63e8_a506, 0xf436_3804_324a_40aa), // 1e265 * 2**-753
    (0xc67b_b459_7ce2_ce48, 0xb143_c605_3edc_d0d5), // 1e266 * 2**-756
    (0xf81a_a16f_dc1b_81da, 0xdd94_b786_8e94_050a), // 1e267 * 2**-759
    (0x9b10_a4e5_e991_3128, 0xca7c_f2b4_191c_8326), // 1e268 * 2**-763
    (0xc1d4_ce1f_63f5_7d72, 0xfd1c_2f61_1f63_a3f0), // 1e269 * 2**-766
    (0xf24a_01a7_3cf2_dccf, 0xbc63_3b39_673c_8cec), // 1e270 * 2**-769
    (0x976e_4108_8617_ca01, 0xd5be_0503_e085_d813), // 1e271 * 2**-773
    (0xbd49_d14a_a79d_bc82, 0x4b2d_8644_d8a7_4e18), // 1e272 * 2**-776
    (0xec9c_459d_5185_2ba2, 0xddf8_e7d6_0ed1_219e), // 1e273 * 2**-779
    (0x93e1_ab82_52f3_3b45, 0xcabb_90e5_c942_b503), // 1e274 * 2**-783
    (0xb8da_1662_e7b0_0a17, 0x3d6a_751f_3b93_6243), // 1e275 * 2**-786
    (0xe710_9bfb_a19c_0c9d, 0x0cc5_1267_0a78_3ad4), // 1e276 * 2**-789
    (0x906a_617d_4501_87e2, 0x27fb_2b80_668b_24c5), // 1e277 * 2**-793
    (0xb484_f9dc_9641_e9da, 0xb1f9_f660_802d_edf6), // 1e278 * 2**-796
    (0xe1a6_3853_bbd2_6451, 0x5e78_73f8_a039_6973), // 1e279 * 2**-799
    (0x8d07_e334_5563_7eb2, 0xdb0b_487b_6423_e1e8), // 1e280 * 2**-803
    (0xb049_dc01_6abc_5e5f, 0x91ce_1a9a_3d2c_da62), // 1e281 * 2**-806
    (0xdc5c_5301_c56b_75f7, 0x7641_a140_cc78_10fb), // 1e282 * 2**-809
    (0x89b9_b3e1_1b63_29ba, 0xa9e9_04c8_7fcb_0a9d), // 1e283 * 2**-813
    (0xac28_20d9_623b_f429, 0x5463_45fa_9fbd_cd44), // 1e284 * 2**-816
    (0xd732_290f_baca_f133, 0xa97c_1779_47ad_4095), // 1e285 * 2**-819
    (0x867f_59a9_d4be_d6c0, 0x49ed_8eab_cccc_485d), // 1e286 * 2**-823
    (0xa81f_3014_49ee_8c70, 0x5c68_f256_bfff_5a74), // 1e287 * 2**-826
    (0xd226_fc19_5c6a_2f8c, 0x7383_2eec_6fff_3111), // 1e288 * 2**-829
    (0x8358_5d8f_d9c2_5db7, 0xc831_fd53_c5ff_7eab), // 1e289 * 2**-833
    (0xa42e_74f3_d032_f525, 0xba3e_7ca8_b77f_5e55), // 1e290 * 2**-836
    (0xcd3a_1230_c43f_b26f, 0x28ce_1bd2_e55f_35eb), // 1e291 * 2**-839
    (0x8044_4b5e_7aa7_cf85, 0x7980_d163_cf5b_81b3), // 1e292 * 2**-843
    (0xa055_5e36_1951_c366, 0xd7e1_05bc_c332_621f), // 1e293 * 2**-846
    (0xc86a_b5c3_9fa6_3440, 0x8dd9_472b_f3fe_faa7), // 1e294 * 2**-849
    (0xfa85_6334_878f_c150, 0xb14f_98f6_f0fe_b951), // 1e295 * 2**-852
    (0x9c93_5e00_d4b9_d8d2, 0x6ed1_bf9a_569f_33d3), // 1e296 * 2**-856
    (0xc3b8_3581_09e8_4f07, 0x0a86_2f80_ec47_00c8), // 1e297 * 2**-859
    (0xf4a6_42e1_4c62_62c8, 0xcd27_bb61_2758_c0fa), // 1e298 * 2**-862
    (0x98e7_e9cc_cfbd_7dbd, 0x8038_d51c_b897_789c), // 1e299 * 2**-866
    (0xbf21_e440_03ac_dd2c, 0xe047_0a63_e6bd_56c3), // 1e300 * 2**-869
    (0xeeea_5d50_0498_1478, 0x1858_ccfc_e06c_ac74), // 1e301 * 2**-872
    (0x9552_7a52_02df_0ccb, 0x0f37_801e_0c43_ebc8), // 1e302 * 2**-876
    (0xbaa7_18e6_8396_cffd, 0xd305_6025_8f54_e6ba), // 1e303 * 2**-879
    (0xe950_df20_247c_83fd, 0x47c6_b82e_f32a_2069), // 1e304 * 2**-882
    (0x91d2_8b74_16cd_d27e, 0x4cdc_331d_57fa_5441), // 1e305 * 2**-886
    (0xb647_2e51_1c81_471d, 0xe013_3fe4_adf8_e952), // 1e306 * 2**-889
    (0xe3d8_f9e5_63a1_98e5, 0x5818_0fdd_d977_23a6), // 1e307 * 2**-892
    (0x8e67_9c2f_5e44_ff8f, 0x570f_09ea_a7ea_7648), // 1e308 * 2**-896
    (0xb201_833b_35d6_3f73, 0x2cd2_cc65_51e5_13da), // 1e309 * 2**-899
    (0xde81_e40a_034b_cf4f, 0xf807_7f7e_a65e_58d1), // 1e310 * 2**-902
    (0x8b11_2e86_420f_6191, 0xfb04_afaf_27fa_f782), // 1e311 * 2**-906
    (0xadd5_7a27_d293_39f6, 0x79c5_db9a_f1f9_b563), // 1e312 * 2**-909
    (0xd94a_d8b1_c738_0874, 0x1837_5281_ae78_22bc), // 1e313 * 2**-912
    (0x87ce_c76f_1c83_0548, 0x8f22_9391_0d0b_15b5), // 1e314 * 2**-916
    (0xa9c2_794a_e3a3_c69a, 0xb2eb_3875_504d_db22), // 1e315 * 2**-919
    (0xd433_179d_9c8c_b841, 0x5fa6_0692_a461_51eb), // 1e316 * 2**-922
    (0x849f_eec2_81d7_f328, 0xdbc7_c41b_a6bc_d333), // 1e317 * 2**-926
    (0xa5c7_ea73_224d_eff3, 0x12b9_b522_906c_0800), // 1e318 * 2**-929
    (0xcf39_e50f_eae1_6bef, 0xd768_226b_3487_0a00), // 1e319 * 2**-932
    (0x8184_2f29_f2cc_e375, 0xe6a1_1583_00d4_6640), // 1e320 * 2**-936
    (0xa1e5_3af4_6f80_1c53, 0x6049_5ae3_c109_7fd0), // 1e321 * 2**-939
    (0xca5e_89b1_8b60_2368, 0x385b_b19c_b14b_dfc4), // 1e322 * 2**-942
    (0xfcf6_2c1d_ee38_2c42, 0x4672_9e03_dd9e_d7b5), // 1e323 * 2**-945
    (0x9e19_db92_b4e3_1ba9, 0x6c07_a2c2_6a83_46d1), // 1e324 * 2**-949
    (0xc5a0_5277_621b_e293, 0xc709_8b73_0524_1885), // 1e325 * 2**-952
    (0xf708_6715_3aa2_db38, 0xb8cb_ee4f_c66d_1ea7), // 1e326 * 2**-955
    (0x9a65_406d_44a5_c903, 0x737f_74f1_dc04_3328), // 1e327 * 2**-959
    (0xc0fe_9088_95cf_3b44, 0x505f_522e_5305_3ff2), // 1e328 * 2**-962
    (0xf13e_34aa_bb43_0a15, 0x6477_26b9_e7c6_8fef), // 1e329 * 2**-965
    (0x96c6_e0ea_b509_e64d, 0x5eca_7834_30dc_19f5), // 1e330 * 2**-969
    (0xbc78_9925_624c_5fe0, 0xb67d_1641_3d13_2072), // 1e331 * 2**-972
    (0xeb96_bf6e_badf_77d8, 0xe41c_5bd1_8c57_e88f), // 1e332 * 2**-975
    (0x933e_37a5_34cb_aae7, 0x8e91_b962_f7b6_f159), // 1e333 * 2**-979
    (0xb80d_c58e_81fe_95a1, 0x7236_27bb_b5a4_adb0), // 1e334 * 2**-982
    (0xe611_36f2_227e_3b09, 0xcec3_b1aa_a30d_d91c), // 1e335 * 2**-985
    (0x8fca_c257_558e_e4e6, 0x213a_4f0a_a5e8_a7b1), // 1e336 * 2**-989
    (0xb3bd_72ed_2af2_9e1f, 0xa988_e2cd_4f62_d19d), // 1e337 * 2**-992
    (0xe0ac_cfa8_75af_45a7, 0x93eb_1b80_a33b_8605), // 1e338 * 2**-995
    (0x8c6c_01c9_498d_8b88, 0xbc72_f130_6605_33c3), // 1e339 * 2**-999
    (0xaf87_023b_9bf0_ee6a, 0xeb8f_ad7c_7f86_80b4), // 1e340 * 2**-1002
    (0xdb68_c2ca_82ed_2a05, 0xa673_98db_9f68_20e1), // 1e341 * 2**-1005
    (0x8921_79be_91d4_3a43, 0x8808_3f89_43a1_148c), // 1e342 * 2**-1009
    (0xab69_d82e_3649_48d4, 0x6a0a_4f6b_9489_59b0), // 1e343 * 2**-1012
    (0xd644_4e39_c3db_9b09, 0x848c_e346_79ab_b01c), // 1e344 * 2**-1015
    (0x85ea_b0e4_1a69_40e5, 0xf2d8_0e0c_0c0b_4e11), // 1e345 * 2**-1019
    (0xa765_5d1d_2103_911f, 0x6f8e_118f_0f0e_2195), // 1e346 * 2**-1022
    (0xd13e_b464_6944_7567, 0x4b71_95f2_d2d1_a9fb), // 1e347 * 2**-1025
];

#[cfg(test)]
mod tests {
    use super::*;

    const SYN: Option<NumErrorKind> = Some(NumErrorKind::Syntax);
    const RNG: Option<NumErrorKind> = Some(NumErrorKind::Range);

    fn kind<T>(r: &Result<T, NumError>) -> Option<NumErrorKind> {
        r.as_ref().err().map(|e| e.kind)
    }

    // internal/strconv atob_test.go
    #[test]
    fn parse_bool_table() {
        for (input, out, err) in [
            ("", false, SYN),
            ("asdf", false, SYN),
            ("0", false, None),
            ("f", false, None),
            ("F", false, None),
            ("FALSE", false, None),
            ("false", false, None),
            ("False", false, None),
            ("1", true, None),
            ("t", true, None),
            ("T", true, None),
            ("TRUE", true, None),
            ("true", true, None),
            ("True", true, None),
        ] {
            let got = parse_bool(input);
            assert_eq!(kind(&got), err, "{input:?}");
            if err.is_none() {
                assert_eq!(got, Ok(out), "{input:?}");
            }
        }
    }

    // internal/strconv atoi_test.go parseUint64Tests, parseUint64BaseTests, parseUint32Tests
    #[test]
    fn parse_uint_tables() {
        let base10_64: &[(&str, u64, Option<NumErrorKind>)] = &[
            ("", 0, SYN),
            ("0", 0, None),
            ("1", 1, None),
            ("12345", 12345, None),
            ("012345", 12345, None),
            ("12345x", 0, SYN),
            ("98765432100", 98765432100, None),
            ("18446744073709551615", u64::MAX, None),
            ("18446744073709551616", u64::MAX, RNG),
            ("18446744073709551620", u64::MAX, RNG),
            ("1_2_3_4_5", 0, SYN),
            ("_12345", 0, SYN),
            ("1__2345", 0, SYN),
            ("12345_", 0, SYN),
            ("-0", 0, SYN),
            ("-1", 0, SYN),
            ("+1", 0, SYN),
        ];
        for &(input, out, err) in base10_64 {
            for bits in [64, 0] {
                let got = parse_uint(input, 10, bits);
                assert_eq!(kind(&got), err, "{input:?}");
                if err.is_none() {
                    assert_eq!(got, Ok(out), "{input:?}");
                }
            }
        }
        let base: &[(&str, u32, u64, Option<NumErrorKind>)] = &[
            ("", 0, 0, SYN),
            ("0", 0, 0, None),
            ("0x", 0, 0, SYN),
            ("0X", 0, 0, SYN),
            ("1", 0, 1, None),
            ("12345", 0, 12345, None),
            ("012345", 0, 0o12345, None),
            ("0x12345", 0, 0x12345, None),
            ("0X12345", 0, 0x12345, None),
            ("12345x", 0, 0, SYN),
            ("0xabcdefg123", 0, 0, SYN),
            ("123456789abc", 0, 0, SYN),
            ("98765432100", 0, 98765432100, None),
            ("18446744073709551615", 0, u64::MAX, None),
            ("18446744073709551616", 0, u64::MAX, RNG),
            ("18446744073709551620", 0, u64::MAX, RNG),
            ("0xFFFFFFFFFFFFFFFF", 0, u64::MAX, None),
            ("0x10000000000000000", 0, u64::MAX, RNG),
            ("01777777777777777777777", 0, u64::MAX, None),
            ("01777777777777777777778", 0, 0, SYN),
            ("02000000000000000000000", 0, u64::MAX, RNG),
            ("0200000000000000000000", 0, 1 << 61, None),
            ("0b", 0, 0, SYN),
            ("0B", 0, 0, SYN),
            ("0b101", 0, 5, None),
            ("0B101", 0, 5, None),
            ("0o", 0, 0, SYN),
            ("0O", 0, 0, SYN),
            ("0o377", 0, 255, None),
            ("0O377", 0, 255, None),
            ("1_2_3_4_5", 0, 12345, None),
            ("_12345", 0, 0, SYN),
            ("1__2345", 0, 0, SYN),
            ("12345_", 0, 0, SYN),
            ("1_2_3_4_5", 10, 0, SYN),
            ("_12345", 10, 0, SYN),
            ("1__2345", 10, 0, SYN),
            ("12345_", 10, 0, SYN),
            ("0x_1_2_3_4_5", 0, 0x12345, None),
            ("_0x12345", 0, 0, SYN),
            ("0x__12345", 0, 0, SYN),
            ("0x1__2345", 0, 0, SYN),
            ("0x1234__5", 0, 0, SYN),
            ("0x12345_", 0, 0, SYN),
            ("1_2_3_4_5", 16, 0, SYN),
            ("_12345", 16, 0, SYN),
            ("1__2345", 16, 0, SYN),
            ("1234__5", 16, 0, SYN),
            ("12345_", 16, 0, SYN),
            ("0_1_2_3_4_5", 0, 0o12345, None),
            ("_012345", 0, 0, SYN),
            ("0__12345", 0, 0, SYN),
            ("01234__5", 0, 0, SYN),
            ("012345_", 0, 0, SYN),
            ("0o_1_2_3_4_5", 0, 0o12345, None),
            ("_0o12345", 0, 0, SYN),
            ("0o__12345", 0, 0, SYN),
            ("0o1234__5", 0, 0, SYN),
            ("0o12345_", 0, 0, SYN),
            ("0_1_2_3_4_5", 8, 0, SYN),
            ("_012345", 8, 0, SYN),
            ("0__12345", 8, 0, SYN),
            ("01234__5", 8, 0, SYN),
            ("012345_", 8, 0, SYN),
            ("0b_1_0_1", 0, 5, None),
            ("_0b101", 0, 0, SYN),
            ("0b__101", 0, 0, SYN),
            ("0b1__01", 0, 0, SYN),
            ("0b10__1", 0, 0, SYN),
            ("0b101_", 0, 0, SYN),
            ("1_0_1", 2, 0, SYN),
            ("_101", 2, 0, SYN),
            ("1_01", 2, 0, SYN),
            ("10_1", 2, 0, SYN),
            ("101_", 2, 0, SYN),
        ];
        for &(input, b, out, err) in base {
            let got = parse_uint(input, b, 64);
            assert_eq!(kind(&got), err, "{input:?} base {b}");
            if err.is_none() {
                assert_eq!(got, Ok(out), "{input:?} base {b}");
            }
        }
        for (input, out, err) in [
            ("", 0, SYN),
            ("0", 0, None),
            ("1", 1, None),
            ("12345", 12345, None),
            ("012345", 12345, None),
            ("12345x", 0, SYN),
            ("987654321", 987654321, None),
            ("4294967295", u64::from(u32::MAX), None),
            ("4294967296", u64::from(u32::MAX), RNG),
            ("1_2_3_4_5", 0, SYN),
            ("_12345", 0, SYN),
            ("1__2345", 0, SYN),
            ("12345_", 0, SYN),
        ] {
            let got = parse_uint(input, 10, 32);
            assert_eq!(kind(&got), err, "{input:?}");
            if err.is_none() {
                assert_eq!(got, Ok(out), "{input:?}");
            }
        }
    }

    // internal/strconv atoi_test.go parseInt64Tests, parseInt64BaseTests, parseInt32Tests
    #[test]
    fn parse_int_tables() {
        let base10_64: &[(&str, i64, Option<NumErrorKind>)] = &[
            ("", 0, SYN),
            ("0", 0, None),
            ("-0", 0, None),
            ("+0", 0, None),
            ("1", 1, None),
            ("-1", -1, None),
            ("+1", 1, None),
            ("12345", 12345, None),
            ("-12345", -12345, None),
            ("012345", 12345, None),
            ("-012345", -12345, None),
            ("98765432100", 98765432100, None),
            ("-98765432100", -98765432100, None),
            ("9223372036854775807", i64::MAX, None),
            ("-9223372036854775807", -i64::MAX, None),
            ("9223372036854775808", i64::MAX, RNG),
            ("-9223372036854775808", i64::MIN, None),
            ("9223372036854775809", i64::MAX, RNG),
            ("-9223372036854775809", i64::MIN, RNG),
            ("-1_2_3_4_5", 0, SYN),
            ("-_12345", 0, SYN),
            ("_12345", 0, SYN),
            ("1__2345", 0, SYN),
            ("12345_", 0, SYN),
            ("123%45", 0, SYN),
        ];
        for &(input, out, err) in base10_64 {
            for bits in [64, 0] {
                let got = parse_int(input, 10, bits);
                assert_eq!(kind(&got), err, "{input:?}");
                if err.is_none() {
                    assert_eq!(got, Ok(out), "{input:?}");
                }
            }
        }
        let holycow35 = ((((((17 * 35 + 24) * 35 + 21) * 35 + 34) * 35 + 12) * 35 + 24) * 35) + 32;
        let holycow36 = ((((((17 * 36 + 24) * 36 + 21) * 36 + 34) * 36 + 12) * 36 + 24) * 36) + 32;
        let base: &[(&str, u32, i64, Option<NumErrorKind>)] = &[
            ("", 0, 0, SYN),
            ("0", 0, 0, None),
            ("-0", 0, 0, None),
            ("1", 0, 1, None),
            ("-1", 0, -1, None),
            ("12345", 0, 12345, None),
            ("-12345", 0, -12345, None),
            ("012345", 0, 0o12345, None),
            ("-012345", 0, -0o12345, None),
            ("0x12345", 0, 0x12345, None),
            ("-0X12345", 0, -0x12345, None),
            ("12345x", 0, 0, SYN),
            ("-12345x", 0, 0, SYN),
            ("98765432100", 0, 98765432100, None),
            ("-98765432100", 0, -98765432100, None),
            ("9223372036854775807", 0, i64::MAX, None),
            ("-9223372036854775807", 0, -i64::MAX, None),
            ("9223372036854775808", 0, i64::MAX, RNG),
            ("-9223372036854775808", 0, i64::MIN, None),
            ("9223372036854775809", 0, i64::MAX, RNG),
            ("-9223372036854775809", 0, i64::MIN, RNG),
            ("g", 17, 16, None),
            ("10", 25, 25, None),
            ("holycow", 35, holycow35, None),
            ("holycow", 36, holycow36, None),
            ("0", 2, 0, None),
            ("-1", 2, -1, None),
            ("1010", 2, 10, None),
            ("1000000000000000", 2, 1 << 15, None),
            (
                "111111111111111111111111111111111111111111111111111111111111111",
                2,
                i64::MAX,
                None,
            ),
            (
                "1000000000000000000000000000000000000000000000000000000000000000",
                2,
                i64::MAX,
                RNG,
            ),
            (
                "-1000000000000000000000000000000000000000000000000000000000000000",
                2,
                i64::MIN,
                None,
            ),
            (
                "-1000000000000000000000000000000000000000000000000000000000000001",
                2,
                i64::MIN,
                RNG,
            ),
            ("-10", 8, -8, None),
            ("57635436545", 8, 0o57635436545, None),
            ("100000000", 8, 1 << 24, None),
            ("10", 16, 16, None),
            ("-123456789abcdef", 16, -0x123456789abcdef, None),
            ("7fffffffffffffff", 16, i64::MAX, None),
            ("-0x_1_2_3_4_5", 0, -0x12345, None),
            ("0x_1_2_3_4_5", 0, 0x12345, None),
            ("-_0x12345", 0, 0, SYN),
            ("_-0x12345", 0, 0, SYN),
            ("_0x12345", 0, 0, SYN),
            ("0x__12345", 0, 0, SYN),
            ("0x1__2345", 0, 0, SYN),
            ("0x1234__5", 0, 0, SYN),
            ("0x12345_", 0, 0, SYN),
            ("-0_1_2_3_4_5", 0, -0o12345, None),
            ("0_1_2_3_4_5", 0, 0o12345, None),
            ("-_012345", 0, 0, SYN),
            ("_-012345", 0, 0, SYN),
            ("_012345", 0, 0, SYN),
            ("0__12345", 0, 0, SYN),
            ("01234__5", 0, 0, SYN),
            ("012345_", 0, 0, SYN),
            ("+0xf", 0, 0xf, None),
            ("-0xf", 0, -0xf, None),
            ("0x+f", 0, 0, SYN),
            ("0x-f", 0, 0, SYN),
        ];
        for &(input, b, out, err) in base {
            let got = parse_int(input, b, 64);
            assert_eq!(kind(&got), err, "{input:?} base {b}");
            if err.is_none() {
                assert_eq!(got, Ok(out), "{input:?} base {b}");
            }
        }
        for (input, out, err) in [
            ("", 0, SYN),
            ("0", 0, None),
            ("-0", 0, None),
            ("1", 1, None),
            ("-1", -1, None),
            ("12345", 12345, None),
            ("-12345", -12345, None),
            ("012345", 12345, None),
            ("-012345", -12345, None),
            ("12345x", 0, SYN),
            ("-12345x", 0, SYN),
            ("987654321", 987654321, None),
            ("-987654321", -987654321, None),
            ("2147483647", i64::from(i32::MAX), None),
            ("-2147483647", -i64::from(i32::MAX), None),
            ("2147483648", i64::from(i32::MAX), RNG),
            ("-2147483648", i64::from(i32::MIN), None),
            ("2147483649", i64::from(i32::MAX), RNG),
            ("-2147483649", i64::from(i32::MIN), RNG),
            ("-1_2_3_4_5", 0, SYN),
            ("-_12345", 0, SYN),
            ("_12345", 0, SYN),
            ("1__2345", 0, SYN),
            ("12345_", 0, SYN),
            ("123%45", 0, SYN),
        ] {
            let got = parse_int(input, 10, 32);
            assert_eq!(kind(&got), err, "{input:?}");
            if err.is_none() {
                assert_eq!(got, Ok(out), "{input:?}");
            }
        }
    }

    #[test]
    fn parse_int_quirks() {
        // A range error from ParseUint with bitSize 1 is swallowed for negatives.
        assert_eq!(parse_int("-2", 10, 1), Ok(-1));
        assert_eq!(kind(&parse_int("1", 10, 1)), RNG);
        // A range error is reported before a later syntax error.
        assert_eq!(kind(&parse_uint("99999999999999999999x", 10, 64)), RNG);
        assert_eq!(kind(&parse_int("-99999999999999999999x", 0, 64)), RNG);
    }

    #[test]
    #[should_panic(expected = "invalid base 37")]
    fn parse_int_invalid_base_panics() {
        let _ = parse_int("0", 37, 64);
    }

    #[test]
    #[should_panic(expected = "invalid bit size 65")]
    fn parse_uint_invalid_bit_size_panics() {
        let _ = parse_uint("0", 0, 65);
    }

    #[test]
    fn num_error_text() {
        assert_eq!(
            parse_int("12345x", 10, 64).map_err(|e| e.to_string()),
            Err("strconv.ParseInt: parsing \"12345x\": invalid syntax".to_owned())
        );
        assert_eq!(
            parse_uint("18446744073709551616", 0, 64).map_err(|e| e.to_string()),
            Err(
                "strconv.ParseUint: parsing \"18446744073709551616\": value out of range"
                    .to_owned()
            )
        );
        assert_eq!(
            parse_float("1\x00.2").map_err(|e| e.to_string()),
            Err("strconv.ParseFloat: parsing \"1\\x00.2\": invalid syntax".to_owned())
        );
        assert_eq!(
            parse_bool("yes").map_err(|e| e.to_string()),
            Err("strconv.ParseBool: parsing \"yes\": invalid syntax".to_owned())
        );
    }

    // internal/strconv atof_test.go atoftests: in → FormatFloat(out, 'g', -1, 64)
    #[test]
    fn parse_float_table() {
        let long_2 = format!("2.{}e+1", "2".repeat(4000));
        let long_x2 = format!("0x2.{}p221", "2".repeat(4000));
        let long_half = format!(
            "1.00000000000000011102230246251565404236316680908203125{}1",
            "0".repeat(10000)
        );
        let long_xhalf = format!("0x1.00000000000008{}1p0", "0".repeat(10000));
        let cases: &[(&str, &str, Option<NumErrorKind>)] = &[
            ("", "0", SYN),
            ("1", "1", None),
            ("+1", "1", None),
            ("1x", "0", SYN),
            ("1.1.", "0", SYN),
            ("1e23", "1e+23", None),
            ("1E23", "1e+23", None),
            ("100000000000000000000000", "1e+23", None),
            ("1e-100", "1e-100", None),
            ("123456700", "1.234567e+08", None),
            ("99999999999999974834176", "9.999999999999997e+22", None),
            ("100000000000000000000001", "1.0000000000000001e+23", None),
            ("100000000000000008388608", "1.0000000000000001e+23", None),
            ("100000000000000016777215", "1.0000000000000001e+23", None),
            ("100000000000000016777216", "1.0000000000000003e+23", None),
            ("-1", "-1", None),
            ("-0.1", "-0.1", None),
            ("-0", "-0", None),
            ("1e-20", "1e-20", None),
            ("625e-3", "0.625", None),
            ("0x1p0", "1", None),
            ("0x1p1", "2", None),
            ("0x1p-1", "0.5", None),
            ("0x1ep-1", "15", None),
            ("-0x1ep-1", "-15", None),
            ("-0x1_ep-1", "-15", None),
            ("0x1p-200", "6.223015277861142e-61", None),
            ("0x1p200", "1.6069380442589903e+60", None),
            ("0x1fFe2.p0", "131042", None),
            ("0x1fFe2.P0", "131042", None),
            ("-0x2p3", "-16", None),
            ("0x0.fp4", "15", None),
            ("0x0.fp0", "0.9375", None),
            ("0x1e2", "0", SYN),
            ("1p2", "0", SYN),
            ("0", "0", None),
            ("0e0", "0", None),
            ("-0e0", "-0", None),
            ("+0e0", "0", None),
            ("0e+01234567890123456789", "0", None),
            ("-0.00e-01234567890123456789", "-0", None),
            ("0x0p+01234567890123456789", "0", None),
            ("-0x0.00p-01234567890123456789", "-0", None),
            ("0e291", "0", None),
            ("0e348", "0", None),
            ("-0e348", "-0", None),
            ("0x0p1026", "0", None),
            ("-0x0p1026", "-0", None),
            ("nan", "NaN", None),
            ("NaN", "NaN", None),
            ("NAN", "NaN", None),
            ("inf", "+Inf", None),
            ("-Inf", "-Inf", None),
            ("+INF", "+Inf", None),
            ("-Infinity", "-Inf", None),
            ("+INFINITY", "+Inf", None),
            ("Infinity", "+Inf", None),
            ("1.7976931348623157e308", "1.7976931348623157e+308", None),
            ("-1.7976931348623157e308", "-1.7976931348623157e+308", None),
            ("0x1.fffffffffffffp1023", "1.7976931348623157e+308", None),
            ("0x1fffffffffffffp+971", "1.7976931348623157e+308", None),
            ("0x.1fffffffffffffp1027", "1.7976931348623157e+308", None),
            ("1.7976931348623159e308", "+Inf", RNG),
            ("-1.7976931348623159e308", "-Inf", RNG),
            ("0x1p1024", "+Inf", RNG),
            ("0x2p1023", "+Inf", RNG),
            ("0x.1p1028", "+Inf", RNG),
            ("0x.2p1027", "+Inf", RNG),
            ("1.7976931348623158e308", "1.7976931348623157e+308", None),
            (
                "0x1.fffffffffffff7fffp1023",
                "1.7976931348623157e+308",
                None,
            ),
            ("1.797693134862315808e308", "+Inf", RNG),
            ("0x1.fffffffffffff8p1023", "+Inf", RNG),
            ("0x1fffffffffffff.8p+971", "+Inf", RNG),
            ("-0x1fffffffffffff8p+967", "-Inf", RNG),
            ("0x.1fffffffffffff8p1027", "+Inf", RNG),
            ("1e308", "1e+308", None),
            ("2e308", "+Inf", RNG),
            ("1e309", "+Inf", RNG),
            ("0x1p1025", "+Inf", RNG),
            ("1e310", "+Inf", RNG),
            ("-1e400000", "-Inf", RNG),
            ("0x1p2000000000", "+Inf", RNG),
            ("1e-305", "1e-305", None),
            ("1e-310", "1e-310", None),
            ("1e-322", "1e-322", None),
            ("5e-324", "5e-324", None),
            ("4e-324", "5e-324", None),
            ("3e-324", "5e-324", None),
            ("2e-324", "0", None),
            ("1e-350", "0", None),
            ("1e-400000", "0", None),
            ("0x2.00000000000000p-1010", "1.8227805048890994e-304", None),
            ("0x1.fffffffffffff0p-1010", "1.8227805048890992e-304", None),
            ("0x1.fffffffffffff7p-1010", "1.8227805048890992e-304", None),
            ("0x1.fffffffffffff8p-1010", "1.8227805048890994e-304", None),
            ("0x1.fffffffffffff9p-1010", "1.8227805048890994e-304", None),
            ("0x2.00000000000000p-1022", "4.450147717014403e-308", None),
            ("0x1.fffffffffffff0p-1022", "4.4501477170144023e-308", None),
            ("0x1.00000000000000p-1022", "2.2250738585072014e-308", None),
            ("0x0.fffffffffffff0p-1022", "2.225073858507201e-308", None),
            ("0x0.ffffffffffffe0p-1022", "2.2250738585072004e-308", None),
            ("0x0.ffffffffffffe7p-1022", "2.2250738585072004e-308", None),
            ("0x1.ffffffffffffe8p-1023", "2.225073858507201e-308", None),
            ("0x0.00000003fffff0p-1022", "2.072261e-317", None),
            ("0x0.00000003456788p-1022", "1.694649e-317", None),
            ("0x0.00000003456789p-1022", "1.6946496e-317", None),
            (
                "0x0.0000000345678800000000000000000000000001p-1022",
                "1.6946496e-317",
                None,
            ),
            ("0x0.000000000000f0p-1022", "7.4e-323", None),
            ("0x0.00000000000058p-1022", "3e-323", None),
            ("0x0.00000000000057p-1022", "2.5e-323", None),
            ("0x0.00000000000010p-1022", "5e-324", None),
            ("0x0.000000000000081p-1022", "5e-324", None),
            ("0x0.00000000000008p-1022", "0", None),
            ("0x0.00000000000007fp-1022", "0", None),
            ("1e-4294967296", "0", None),
            ("1e+4294967296", "+Inf", RNG),
            ("1e-18446744073709551616", "0", None),
            ("1e+18446744073709551616", "+Inf", RNG),
            ("0x1p-4294967296", "0", None),
            ("0x1p+18446744073709551616", "+Inf", RNG),
            ("1e", "0", SYN),
            ("1e-", "0", SYN),
            (".e-1", "0", SYN),
            ("1\x00.2", "0", SYN),
            ("0x", "0", SYN),
            ("0x.", "0", SYN),
            ("0x1", "0", SYN),
            ("0x.1", "0", SYN),
            ("0x1p", "0", SYN),
            ("0x.1p", "0", SYN),
            ("0x1p+", "0", SYN),
            ("0x.1p-", "0", SYN),
            ("0x1p+2", "4", None),
            ("0x.1p+2", "0.25", None),
            ("0x1p-2", "0.25", None),
            ("0x.1p-2", "0.015625", None),
            ("2.2250738585072012e-308", "2.2250738585072014e-308", None),
            ("2.2250738585072011e-308", "2.225073858507201e-308", None),
            ("4.630813248087435e+307", "4.630813248087435e+307", None),
            ("22.222222222222222", "22.22222222222222", None),
            (&long_2, "22.22222222222222", None),
            ("0x1.1111111111111p222", "7.18931911124017e+66", None),
            ("0x2.2222222222222p221", "7.18931911124017e+66", None),
            (&long_x2, "7.18931911124017e+66", None),
            (
                "1.00000000000000011102230246251565404236316680908203125",
                "1",
                None,
            ),
            ("0x1.00000000000008p0", "1", None),
            (
                "1.00000000000000011102230246251565404236316680908203124",
                "1",
                None,
            ),
            ("0x1.00000000000007Fp0", "1", None),
            (
                "1.00000000000000011102230246251565404236316680908203126",
                "1.0000000000000002",
                None,
            ),
            ("0x1.000000000000081p0", "1.0000000000000002", None),
            ("0x1.00000000000009p0", "1.0000000000000002", None),
            (&long_half, "1.0000000000000002", None),
            (&long_xhalf, "1.0000000000000002", None),
            (
                "1.00000000000000033306690738754696212708950042724609375",
                "1.0000000000000004",
                None,
            ),
            ("0x1.00000000000018p0", "1.0000000000000004", None),
            (
                "1090544144181609348671888949248",
                "1.0905441441816093e+30",
                None,
            ),
            (
                "1090544144181609348835077142190",
                "1.0905441441816094e+30",
                None,
            ),
            ("1_23.50_0_0e+1_2", "1.235e+14", None),
            ("-_123.5e+12", "0", SYN),
            ("+_123.5e+12", "0", SYN),
            ("_123.5e+12", "0", SYN),
            ("1__23.5e+12", "0", SYN),
            ("123_.5e+12", "0", SYN),
            ("123._5e+12", "0", SYN),
            ("123.5_e+12", "0", SYN),
            ("123.5__0e+12", "0", SYN),
            ("123.5e_+12", "0", SYN),
            ("123.5e+_12", "0", SYN),
            ("123.5e_-12", "0", SYN),
            ("123.5e-_12", "0", SYN),
            ("123.5e+1__2", "0", SYN),
            ("123.5e+12_", "0", SYN),
            ("0x_1_2.3_4_5p+1_2", "74565", None),
            ("-_0x12.345p+12", "0", SYN),
            ("+_0x12.345p+12", "0", SYN),
            ("_0x12.345p+12", "0", SYN),
            ("0x__12.345p+12", "0", SYN),
            ("0x1__2.345p+12", "0", SYN),
            ("0x12_.345p+12", "0", SYN),
            ("0x12._345p+12", "0", SYN),
            ("0x12.3__45p+12", "0", SYN),
            ("0x12.345_p+12", "0", SYN),
            ("0x12.345p_+12", "0", SYN),
            ("0x12.345p+_12", "0", SYN),
            ("0x12.345p_-12", "0", SYN),
            ("0x12.345p-_12", "0", SYN),
            ("0x12.345p+1__2", "0", SYN),
            ("0x12.345p+12_", "0", SYN),
            ("1e100x", "0", SYN),
            ("1e1000x", "0", SYN),
        ];
        for &(input, out, err) in cases {
            let got = parse_float(input);
            let shown: String = input.chars().take(60).collect();
            assert_eq!(kind(&got), err, "{shown:?}");
            if let Ok(f) = got {
                assert_eq!(format_float_g(f), out, "{shown:?}");
            }
        }
        // Go's nan() bit pattern.
        assert_eq!(parse_float("NaN").map(f64::to_bits), Ok(GO_NAN_BITS));
    }

    // internal/strconv ftoa_test.go ftoatests with fmt 'g', prec -1
    #[test]
    #[allow(clippy::excessive_precision)] // Go's literals, kept as written
    fn format_float_g_table() {
        let below1e23 = 99999999999999974834176.0;
        let above1e23 = 100000000000000008388608.0;
        for (f, want) in [
            (1.0, "1"),
            (20.0, "20"),
            (1234567.8, "1.2345678e+06"),
            (200000.0, "200000"),
            (2000000.0, "2e+06"),
            (1e10, "1e+10"),
            (0.0, "0"),
            (-0.0, "-0"),
            (-1.0, "-1"),
            (12.0, "12"),
            (123456700.0, "1.234567e+08"),
            (108678236358137.625, "1.0867823635813762e+14"),
            (1e23, "1e+23"),
            (below1e23, "9.999999999999997e+22"),
            (above1e23, "1.0000000000000001e+23"),
            (5e-304 / 1e20, "5e-324"),
            (-5e-304 / 1e20, "-5e-324"),
            (32.0, "32"),
            (f64::NAN, "NaN"),
            (-f64::NAN, "NaN"),
            (f64::INFINITY, "+Inf"),
            (f64::NEG_INFINITY, "-Inf"),
            (2.2250738585072012e-308, "2.2250738585072014e-308"),
            (2.2250738585072011e-308, "2.225073858507201e-308"),
            (383260575764816448.0, "3.8326057576481645e+17"),
            (-5.8339553793802237e+23, "-5.8339553793802237e+23"),
            (1.801439850948199e+16, "1.801439850948199e+16"),
            (5.960464477539063e-08, "5.960464477539063e-08"),
            (1.012e-320, "1.012e-320"),
            // slog attribute examples (client-core §3.5, cli §2.3)
            (0.5, "0.5"),
            (1.5, "1.5"),
            (3.7e+06, "3.7e+06"),
            (0.0001, "0.0001"),
            (0.00001, "1e-05"),
            (123456.0, "123456"),
            (1e21, "1e+21"),
        ] {
            assert_eq!(format_float_g(f), want, "{f:e}");
        }
        // Exact ties between two shortest candidates round to even, unlike Rust's `{}`.
        assert_eq!(
            format_float_g(1125899906842624.25),
            "1.1258999068426242e+15"
        );
        assert_eq!(
            format_float_g(1125899906842624.75),
            "1.1258999068426248e+15"
        );
    }

    // internal/strconv ftoa_test.go TestFtoaPowersOfTwo
    #[test]
    fn format_float_g_powers_of_two_round_trip() {
        for exp in -1100..=1100 {
            let f = 2f64.powi(exp);
            if f.is_infinite() || f == 0.0 {
                continue;
            }
            for x in [f, -f] {
                let s = format_float_g(x);
                assert_eq!(parse_float(&s).map(f64::to_bits), Ok(x.to_bits()), "{s}");
            }
        }
    }

    // internal/strconv atof_test.go TestAtofRandom, with a splitmix64 stream
    #[test]
    fn format_parse_random_round_trip() {
        let mut state: u64 = 42;
        for _ in 0..20000 {
            state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^= z >> 31;
            let x = f64::from_bits(z);
            if x.is_nan() {
                continue;
            }
            let s = format_float_g(x);
            assert_eq!(parse_float(&s).map(f64::to_bits), Ok(z), "{s}");
        }
    }

    #[test]
    fn helpers() {
        assert_eq!(trim_zeros(123_000_000_000), (123, 9));
        assert_eq!(trim_zeros(7), (7, 0));
        assert_eq!(trim_zeros(10), (1, 1));
        assert_eq!(pow10(0), Some((0x8000_0000_0000_0000, 0, 1)));
        assert_eq!(pow10(-349), None);
        assert_eq!(pow10(348), None);
        assert_eq!(mul_log10_2(1600), 481);
        assert_eq!(mul_log2_10(500), 1660);
        assert!(underscore_ok(b"0x_1_2"));
        assert!(!underscore_ok(b"1__2"));
        assert!(!underscore_ok(b"_1"));
        assert!(!underscore_ok(b"1_"));
    }
}
