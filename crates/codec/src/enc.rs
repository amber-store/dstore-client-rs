//! Canonical encoding (`CanonicalEncOptions`): shortest heads, ascending keys, shortest exact floats.

/// A CBOR output buffer.
pub struct Enc {
    buf: Vec<u8>,
}

impl Enc {
    pub fn new() -> Enc {
        Enc { buf: Vec::new() }
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }

    /// Shortest head (= `amber_store_core::cbor::append_head`).
    pub fn head(&mut self, major: u8, n: u64) {
        amber_store_core::cbor::append_head(&mut self.buf, major, n);
    }

    pub fn uint(&mut self, v: u64) {
        self.head(0, v);
    }

    /// Major type 1 with -1-v when v < 0.
    pub fn int(&mut self, v: i64) {
        if v >= 0 {
            self.head(0, v as u64);
        } else {
            // -1 - v is the bitwise complement, which also holds for i64::MIN.
            self.head(1, !(v as u64));
        }
    }

    pub fn bool(&mut self, v: bool) {
        self.buf.push(if v { 0xf5 } else { 0xf4 });
    }

    /// `0xf6`.
    pub fn null(&mut self) {
        self.buf.push(0xf6);
    }

    pub fn bytes(&mut self, v: &[u8]) {
        self.head(2, v.len() as u64);
        self.buf.extend_from_slice(v);
    }

    pub fn text(&mut self, v: &str) {
        self.head(3, v.len() as u64);
        self.buf.extend_from_slice(v.as_bytes());
    }

    /// E11: NaN `f97e00`, ±Inf `f97c00`/`f9fc00`, then float16/32/64, the shortest exact form.
    pub fn f64_canonical(&mut self, v: f64) {
        // encodeFloat with ShortestFloat16, NaNConvert7e00, InfConvertFloat16.
        if v.is_nan() {
            self.buf.extend_from_slice(&[0xf9, 0x7e, 0x00]);
            return;
        }
        if v.is_infinite() {
            let head = if v > 0.0 { 0x7c } else { 0xfc };
            self.buf.extend_from_slice(&[0xf9, head, 0x00]);
            return;
        }
        let f32v = v as f32;
        if f64::from(f32v) != v {
            self.buf.push(0xfb);
            self.buf.extend_from_slice(&v.to_bits().to_be_bytes());
            return;
        }
        let f16 = f32bits_to_f16bits(f32v.to_bits());
        let exact = match precision_from_f32(f32v) {
            Precision::Exact => true,
            // Try the round trip float32 -> float16 -> float32.
            Precision::Unknown => f32::from_bits(f16bits_to_f32bits(f16)) == f32v,
            Precision::Inexact | Precision::Underflow | Precision::Overflow => false,
        };
        if exact {
            self.buf.push(0xf9);
            self.buf.extend_from_slice(&f16.to_be_bytes());
        } else {
            self.buf.push(0xfa);
            self.buf.extend_from_slice(&f32v.to_bits().to_be_bytes());
        }
    }
}

impl Default for Enc {
    fn default() -> Enc {
        Enc::new()
    }
}

/// A value that encodes itself canonically.
pub trait Encode {
    fn encode(&self, e: &mut Enc);
}

/// `codec.Marshal`.
pub fn marshal<T: Encode + ?Sized>(v: &T) -> Vec<u8> {
    let mut e = Enc::new();
    v.encode(&mut e);
    e.into_bytes()
}

/// x448/float16 v0.8.4 `Precision`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Precision {
    Exact,
    Unknown,
    Inexact,
    Underflow,
    Overflow,
}

/// x448/float16 `PrecisionFromfloat32`.
fn precision_from_f32(f: f32) -> Precision {
    const COEFMASK: u32 = 0x7fffff;
    const EXPSHIFT: u32 = 23;
    const EXPBIAS: u32 = 127;
    const EXPMASK: u32 = 0xff << EXPSHIFT;
    const DROPMASK: u32 = COEFMASK >> 10;

    let u = f.to_bits();
    if u == 0 || u == 0x8000_0000 {
        return Precision::Exact;
    }
    let exp = (((u & EXPMASK) >> EXPSHIFT).wrapping_sub(EXPBIAS)) as i32;
    let coef = u & COEFMASK;
    if exp == 128 {
        return Precision::Exact;
    }
    if exp < -24 {
        return Precision::Underflow;
    }
    if exp > 15 {
        return Precision::Overflow;
    }
    if coef & DROPMASK != 0 {
        return Precision::Inexact;
    }
    if exp < -14 {
        return Precision::Unknown;
    }
    Precision::Exact
}

/// x448/float16 `f16bitsToF32bits`.
pub(crate) fn f16bits_to_f32bits(input: u16) -> u32 {
    let sign = u32::from(input & 0x8000) << 16;
    let mut exp = u32::from(input & 0x7c00) >> 10;
    let mut coef = u32::from(input & 0x03ff) << 13;
    if exp == 0x1f {
        if coef == 0 {
            return sign | 0x7f80_0000 | coef;
        }
        return sign | 0x7fc0_0000 | coef;
    }
    if exp == 0 {
        if coef == 0 {
            return sign;
        }
        // Normalize subnormal numbers.
        exp += 1;
        while coef & 0x7f80_0000 == 0 {
            coef <<= 1;
            exp = exp.wrapping_sub(1);
        }
        coef &= 0x007f_ffff;
    }
    sign | (exp.wrapping_add(0x7f - 0xf) << 23) | coef
}

/// x448/float16 `f32bitsToF16bits`: round to nearest, ties to even.
fn f32bits_to_f16bits(u: u32) -> u16 {
    let sign = u & 0x8000_0000;
    let exp = u & 0x7f80_0000;
    let coef = u & 0x007f_ffff;
    if exp == 0x7f80_0000 {
        let nan_bit = if coef != 0 { 0x0200 } else { 0 };
        return ((sign >> 16) | 0x7c00 | nan_bit | (coef >> 13)) as u16;
    }
    let half_sign = sign >> 16;
    let unbiased_exp = (exp >> 23) as i32 - 127;
    let half_exp = unbiased_exp + 15;
    if half_exp >= 0x1f {
        return (half_sign | 0x7c00) as u16;
    }
    if half_exp <= 0 {
        if 14 - half_exp > 24 {
            return half_sign as u16;
        }
        let coef = coef | 0x0080_0000;
        let mut half_coef = coef >> (14 - half_exp) as u32;
        let round_bit = 1u32 << (13 - half_exp) as u32;
        if coef & round_bit != 0 && coef & (3 * round_bit - 1) != 0 {
            half_coef += 1;
        }
        return (half_sign | half_coef) as u16;
    }
    let u_half_exp = (half_exp as u32) << 10;
    let half_coef = coef >> 13;
    let round_bit = 0x0000_1000u32;
    if coef & round_bit != 0 && coef & (3 * round_bit - 1) != 0 {
        return ((half_sign | u_half_exp | half_coef) + 1) as u16;
    }
    (half_sign | u_half_exp | half_coef) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    fn enc(f: impl FnOnce(&mut Enc)) -> String {
        let mut e = Enc::new();
        f(&mut e);
        hex(&e.into_bytes())
    }

    #[test]
    fn shortest_heads() {
        let cases: &[(u64, &str)] = &[
            (0, "00"),
            (23, "17"),
            (24, "1818"),
            (255, "18ff"),
            (256, "190100"),
            (65535, "19ffff"),
            (65536, "1a00010000"),
            (u32::MAX as u64, "1affffffff"),
            (1 << 32, "1b0000000100000000"),
            (u64::MAX, "1bffffffffffffffff"),
        ];
        for &(n, want) in cases {
            assert_eq!(enc(|e| e.uint(n)), want);
            let mut want5 = hex(&[0xa0 | (u8::from_str_radix(&want[..2], 16).unwrap_or(0) & 0x1f)]);
            want5.push_str(&want[2..]);
            assert_eq!(enc(|e| e.head(5, n)), want5);
        }
    }

    #[test]
    fn integers() {
        assert_eq!(enc(|e| e.int(0)), "00");
        assert_eq!(enc(|e| e.int(-1)), "20");
        assert_eq!(enc(|e| e.int(-24)), "37");
        assert_eq!(enc(|e| e.int(-25)), "3818");
        assert_eq!(enc(|e| e.int(-257)), "390100");
        assert_eq!(enc(|e| e.int(i64::MAX)), "1b7fffffffffffffff");
        assert_eq!(enc(|e| e.int(i64::MIN)), "3b7fffffffffffffff");
    }

    #[test]
    fn simple_and_strings() {
        assert_eq!(enc(|e| e.bool(false)), "f4");
        assert_eq!(enc(|e| e.bool(true)), "f5");
        assert_eq!(enc(|e| e.null()), "f6");
        assert_eq!(enc(|e| e.bytes(&[])), "40");
        assert_eq!(enc(|e| e.bytes(&[1, 2])), "420102");
        assert_eq!(enc(|e| e.text("")), "60");
        assert_eq!(enc(|e| e.text("a")), "6161");
        assert_eq!(enc(|e| e.bytes(&[0u8; 24]))[..4], *"5818");
    }

    #[test]
    fn floats() {
        let cases: &[(f64, &str)] = &[
            (0.0, "f90000"),
            (-0.0, "f98000"),
            (0.5, "f93800"),
            (1.5, "f93e00"),
            (65504.0, "f97bff"),
            // Rounds to +Inf as float16: inexact, so float32.
            (65520.0, "fa477ff000"),
            (0.1, "fb3fb999999999999a"),
            (3.4e38, "fb47eff933c78cdfad"),
            (f64::from(f32::MAX), "fa7f7fffff"),
            (f64::MAX, "fb7fefffffffffffff"),
            (f64::from_bits(1), "fb0000000000000001"),
            (f64::INFINITY, "f97c00"),
            (f64::NEG_INFINITY, "f9fc00"),
            (f64::NAN, "f97e00"),
            (f64::from_bits(0xfff8_0000_0000_0001), "f97e00"),
            // float16 subnormals: 2^-24 is the smallest, 2^-15 the largest power of two.
            (2f64.powi(-24), "f90001"),
            (2f64.powi(-15), "f90200"),
            (2f64.powi(-25), "fa33000000"),
            (1e-40, "fb37a16c262777579c"),
        ];
        for &(v, want) in cases {
            assert_eq!(enc(|e| e.f64_canonical(v)), want, "{v:e}");
        }
    }

    #[test]
    fn float16_round_trips_every_half() {
        // Every float16 bit pattern converts to float32 and back unchanged (x448 confirms this for
        // f16bitsToF32bits and f32bitsToF16bits), except NaNs, which gain the quiet bit.
        for bits in 0u16..=u16::MAX {
            let f32bits = f16bits_to_f32bits(bits);
            let back = f32bits_to_f16bits(f32bits);
            let is_nan = bits & 0x7c00 == 0x7c00 && bits & 0x03ff != 0;
            if is_nan {
                assert_eq!(back, bits | 0x0200, "{bits:04x}");
            } else {
                assert_eq!(back, bits, "{bits:04x}");
            }
        }
    }

    #[test]
    fn marshal_uses_encode() {
        struct Two;
        impl Encode for Two {
            fn encode(&self, e: &mut Enc) {
                e.head(4, 2);
                e.uint(1);
                e.int(-2);
            }
        }
        assert_eq!(hex(&marshal(&Two)), "820121");
        assert_eq!(hex(&Enc::default().into_bytes()), "");
    }
}
