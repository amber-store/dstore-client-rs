//! `encoding/base32` with `NoPadding`: the Go decode loop (go1.26.5 `encoding/base32/base32.go`), used for
//! go-iroh node ids and tickets.

/// RFC 4648 standard alphabet.
pub const STD_ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
/// z-base-32 alphabet.
pub const ZBASE32_ALPHABET: &[u8; 32] = b"ybndrfg8ejkmcpqxot1uwisza345h769";

/// `base32.CorruptInputError`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("illegal base32 data at input byte {0}")]
pub struct CorruptInputError(pub usize);

/// `NoPadding` is `-1`; the decoder compares input bytes against `byte(enc.padChar)`, which is 0xFF.
const NO_PADDING_BYTE: u8 = 0xff;
const INVALID_INDEX: u8 = 0xff;

/// `Encoding.decodeMap`.
fn decode_map(alphabet: &[u8; 32]) -> [u8; 256] {
    let mut map = [INVALID_INDEX; 256];
    for (i, &c) in (0u8..).zip(alphabet.iter()) {
        map[usize::from(c)] = i;
    }
    map
}

/// `Encoding.WithPadding(NoPadding).DecodeString`: strips `\r\n`, drops 1/3/6-symbol tails, no
/// trailing-bit check.
pub fn decode_nopad(alphabet: &[u8; 32], s: &[u8]) -> Result<Vec<u8>, CorruptInputError> {
    let map = decode_map(alphabet);
    // stripNewlines: every '\r' and '\n', wherever they are.
    let src: Vec<u8> = s
        .iter()
        .copied()
        .filter(|&b| b != b'\r' && b != b'\n')
        .collect();
    let olen = src.len();
    let mut out = Vec::with_capacity(olen / 8 * 5 + 5);
    // `pos` is the number of consumed symbols; Go's `len(src)` is `olen - pos`.
    let mut pos = 0usize;
    let mut end = false;
    while pos < olen && !end {
        let mut dbuf = [0u8; 8];
        let mut dlen = 8usize;
        let mut j = 0usize;
        while j < 8 {
            if pos == olen {
                // The end, and no padding is expected.
                dlen = j;
                end = true;
                break;
            }
            let c = src[pos];
            pos += 1;
            let rest = olen - pos;
            if c == NO_PADDING_BYTE && j >= 2 && rest < 8 {
                // Go's padding branch, reachable only through a 0xFF input byte.
                if rest + j < 8 - 1 {
                    return Err(CorruptInputError(olen));
                }
                for k in 0..(8 - 1 - j) {
                    if rest > k && src[pos + k] != NO_PADDING_BYTE {
                        return Err(CorruptInputError(olen - rest + k - 1));
                    }
                }
                dlen = j;
                end = true;
                if dlen == 1 || dlen == 3 || dlen == 6 {
                    return Err(CorruptInputError(olen - rest - 1));
                }
                break;
            }
            dbuf[j] = map[usize::from(c)];
            if dbuf[j] == INVALID_INDEX {
                return Err(CorruptInputError(olen - rest - 1));
            }
            j += 1;
        }
        // Pack 8x 5-bit source blocks into a 5-byte quantum (Go's fallthrough switch).
        let mut quantum = [0u8; 5];
        let n = match dlen {
            8 => 5,
            7 => 4,
            5 => 3,
            4 => 2,
            2 => 1,
            _ => 0,
        };
        if n >= 5 {
            quantum[4] = (dbuf[6] << 5) | dbuf[7];
        }
        if n >= 4 {
            quantum[3] = (dbuf[4] << 7) | (dbuf[5] << 2) | (dbuf[6] >> 3);
        }
        if n >= 3 {
            quantum[2] = (dbuf[3] << 4) | (dbuf[4] >> 1);
        }
        if n >= 2 {
            quantum[1] = (dbuf[1] << 6) | (dbuf[2] << 1) | (dbuf[3] >> 4);
        }
        if n >= 1 {
            quantum[0] = (dbuf[0] << 3) | (dbuf[1] >> 2);
        }
        out.extend_from_slice(&quantum[..n]);
    }
    Ok(out)
}

/// `Encoding.WithPadding(NoPadding).EncodeToString`.
pub fn encode_nopad(alphabet: &[u8; 32], b: &[u8]) -> String {
    let sym = |v: u32| char::from(alphabet[(v & 0x1f) as usize]);
    // EncodedLen with NoPadding: n/5*8 + (n%5*8+4)/5 = ceil(8n/5).
    let mut out = String::with_capacity(b.len().saturating_mul(8).div_ceil(5));
    let mut blocks = b.chunks_exact(5);
    for q in &mut blocks {
        let hi = (u32::from(q[0]) << 24)
            | (u32::from(q[1]) << 16)
            | (u32::from(q[2]) << 8)
            | u32::from(q[3]);
        let lo = (hi << 8) | u32::from(q[4]);
        for v in [
            hi >> 27,
            hi >> 22,
            hi >> 17,
            hi >> 12,
            hi >> 7,
            hi >> 2,
            lo >> 5,
            lo,
        ] {
            out.push(sym(v));
        }
    }
    let rem = blocks.remainder();
    if rem.is_empty() {
        return out;
    }
    // Go encodes the remaining bytes in reverse order into dst[0..8]; collect then emit in order.
    let mut dst = [0u32; 8];
    let mut val = 0u32;
    if rem.len() >= 4 {
        val |= u32::from(rem[3]);
        dst[6] = val << 3;
        dst[5] = val >> 2;
    }
    if rem.len() >= 3 {
        val |= u32::from(rem[2]) << 8;
        dst[4] = val >> 7;
    }
    if rem.len() >= 2 {
        val |= u32::from(rem[1]) << 16;
        dst[3] = val >> 12;
        dst[2] = val >> 17;
    }
    val |= u32::from(rem[0]) << 24;
    dst[1] = val >> 22;
    dst[0] = val >> 27;
    let symbols = (rem.len() * 8).div_ceil(5);
    for &v in &dst[..symbols] {
        out.push(sym(v));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dec(s: &[u8]) -> Result<Vec<u8>, CorruptInputError> {
        decode_nopad(STD_ALPHABET, s)
    }

    // encoding/base32 base32_test.go pairs, without padding.
    #[test]
    fn rfc4648_pairs() {
        let pairs: &[(&str, &str)] = &[
            ("", ""),
            ("f", "MY"),
            ("fo", "MZXQ"),
            ("foo", "MZXW6"),
            ("foob", "MZXW6YQ"),
            ("fooba", "MZXW6YTB"),
            ("foobar", "MZXW6YTBOI"),
            ("sure.", "ON2XEZJO"),
            ("sure", "ON2XEZI"),
            ("sur", "ON2XE"),
            ("su", "ON2Q"),
            ("leasure.", "NRSWC43VOJSS4"),
            ("easure.", "MVQXG5LSMUXA"),
            ("asure.", "MFZXK4TFFY"),
        ];
        for (plain, enc) in pairs {
            assert_eq!(encode_nopad(STD_ALPHABET, plain.as_bytes()), *enc);
            assert_eq!(
                dec(enc.as_bytes()).as_deref(),
                Ok(plain.as_bytes()),
                "{enc}"
            );
        }
    }

    // codec-wire-ticket §2.4.7 probes.
    #[test]
    fn go_decode_loop_quirks() {
        assert_eq!(dec(b"A\nA").as_deref(), Ok(&[0u8][..]));
        assert_eq!(dec(b"ON2X\rEZ\nI").as_deref(), Ok(&b"sure"[..]));
        for tail in [&b"A"[..], b"AAA", b"AAAAAA"] {
            assert_eq!(dec(tail).as_deref(), Ok(&[][..]), "{tail:?}");
        }
        assert_eq!(dec(b"AAAAAAAAA").map(|v| v.len()), Ok(5));
        assert_eq!(dec(b"ME").as_deref(), Ok(&[0x61u8][..]));
        assert_eq!(dec(b"MF").as_deref(), Ok(&[0x61u8][..]));
        assert_eq!(dec(b"A="), Err(CorruptInputError(1)));
        assert_eq!(dec(b"0A"), Err(CorruptInputError(0)));
        assert_eq!(dec(b"a"), Err(CorruptInputError(0)));
        assert_eq!(dec(b"MMMMMMMMM!"), Err(CorruptInputError(9)));
        assert_eq!(dec(b"\n\nAA!"), Err(CorruptInputError(2)));
        assert_eq!(
            CorruptInputError(3).to_string(),
            "illegal base32 data at input byte 3"
        );
    }

    #[test]
    fn padding_branch_through_0xff() {
        assert_eq!(
            dec(b"AA\xff\xff\xff\xff\xff\xff").as_deref(),
            Ok(&[0u8][..])
        );
        assert_eq!(
            dec(b"AA\xff\xff\xff\xff\xff\xffMY").as_deref(),
            Ok(&[0u8][..])
        );
        assert_eq!(dec(b"AA\xff"), Err(CorruptInputError(3)));
        assert_eq!(dec(b"AA\xff\xffA\xff\xff\xff"), Err(CorruptInputError(3)));
        assert_eq!(dec(b"AAA\xff\xff\xff\xff\xff"), Err(CorruptInputError(3)));
        assert_eq!(dec(b"A\xff"), Err(CorruptInputError(1)));
    }

    #[test]
    fn zbase32_round_trip() {
        let data: Vec<u8> = (0u8..=255).collect();
        for n in 0..40 {
            let enc = encode_nopad(ZBASE32_ALPHABET, &data[..n]);
            assert_eq!(
                decode_nopad(ZBASE32_ALPHABET, enc.as_bytes()).as_deref(),
                Ok(&data[..n])
            );
        }
    }
}
