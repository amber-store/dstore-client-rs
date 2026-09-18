//! `encoding/hex` (go1.26.5 `encoding/hex/hex.go`).

const HEXTABLE: &[u8; 16] = b"0123456789abcdef";

/// `hex.EncodeToString`.
pub fn encode(b: &[u8]) -> String {
    let mut out = String::with_capacity(b.len() * 2);
    for &c in b {
        out.push(char::from(HEXTABLE[usize::from(c >> 4)]));
        out.push(char::from(HEXTABLE[usize::from(c & 0x0f)]));
    }
    out
}

/// `encoding/hex` errors with Go's texts.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HexError {
    #[error("encoding/hex: odd length hex string")]
    OddLength,
    /// `%#U` of the byte as a rune: `U+007A 'z'`.
    #[error("encoding/hex: invalid byte: {}", fmt_u(*.0))]
    InvalidByte(u8),
}

/// `strconv.IsPrint` over the Latin-1 range (its fast path): ASCII space through `~`, and `¡` through
/// `ÿ` except the soft hyphen U+00AD.
pub(crate) fn latin1_is_print(b: u8) -> bool {
    (0x20..=0x7e).contains(&b) || (b >= 0xa1 && b != 0xad)
}

/// `fmt.Sprintf("%#U", rune(b))`: `U+0067 'g'`, or `U+0001` when the rune is not printable.
pub fn fmt_u(b: u8) -> String {
    let mut out = format!("U+{b:04X}");
    if latin1_is_print(b) {
        out.push_str(" '");
        out.push(char::from(b));
        out.push('\'');
    }
    out
}

/// `reverseHexTable`: the nibble value, or `None` for a byte that is not a hex digit.
fn nibble(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// `hex.DecodeString`: an invalid byte is reported before an odd length.
pub fn decode_string(s: &[u8]) -> Result<Vec<u8>, HexError> {
    let mut out = Vec::with_capacity(s.len() / 2);
    let mut pairs = s.chunks_exact(2);
    for pair in &mut pairs {
        let (p, q) = (pair[0], pair[1]);
        let a = nibble(p).ok_or(HexError::InvalidByte(p))?;
        let b = nibble(q).ok_or(HexError::InvalidByte(q))?;
        out.push((a << 4) | b);
    }
    if let [last] = pairs.remainder() {
        // Check for an invalid char before reporting the bad length, as Go does.
        if nibble(*last).is_none() {
            return Err(HexError::InvalidByte(*last));
        }
        return Err(HexError::OddLength);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    // encoding/hex hex_test.go encDecTests.
    #[test]
    fn encode_decode_pairs() {
        let cases: &[(&str, &[u8])] = &[
            ("", &[]),
            ("0001020304050607", &[0, 1, 2, 3, 4, 5, 6, 7]),
            ("08090a0b0c0d0e0f", &[8, 9, 10, 11, 12, 13, 14, 15]),
            (
                "f0f1f2f3f4f5f6f7",
                &[0xf0, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7],
            ),
            (
                "f8f9fafbfcfdfeff",
                &[0xf8, 0xf9, 0xfa, 0xfb, 0xfc, 0xfd, 0xfe, 0xff],
            ),
            ("67", b"g"),
            ("e3a1", &[0xe3, 0xa1]),
        ];
        for (text, bytes) in cases {
            assert_eq!(encode(bytes), *text);
            assert_eq!(decode_string(text.as_bytes()).as_deref(), Ok(*bytes));
        }
        assert_eq!(
            decode_string(b"F8F9FAFBFCFDFEFF").as_deref(),
            Ok(&[0xf8, 0xf9, 0xfa, 0xfb, 0xfc, 0xfd, 0xfe, 0xff][..])
        );
    }

    // encoding/hex hex_test.go errTests (the error half) and core-rs-gaps §3.7.
    #[test]
    fn decode_errors() {
        let cases: &[(&[u8], &str)] = &[
            (b"0", "encoding/hex: odd length hex string"),
            (b"zd4aa", "encoding/hex: invalid byte: U+007A 'z'"),
            (b"d4aaz", "encoding/hex: invalid byte: U+007A 'z'"),
            (b"30313", "encoding/hex: odd length hex string"),
            (b"0g", "encoding/hex: invalid byte: U+0067 'g'"),
            (b"00gg", "encoding/hex: invalid byte: U+0067 'g'"),
            (b"0\x01", "encoding/hex: invalid byte: U+0001"),
            (b"ffeed", "encoding/hex: odd length hex string"),
            (b"abc", "encoding/hex: odd length hex string"),
            (b"a", "encoding/hex: odd length hex string"),
            (b"zz", "encoding/hex: invalid byte: U+007A 'z'"),
            (b"abz", "encoding/hex: invalid byte: U+007A 'z'"),
            ("ÿ".as_bytes(), "encoding/hex: invalid byte: U+00C3 'Ã'"),
            (b"0G", "encoding/hex: invalid byte: U+0047 'G'"),
            (b"\x00", "encoding/hex: invalid byte: U+0000"),
        ];
        for (input, text) in cases {
            match decode_string(input) {
                Ok(v) => panic!("{input:?} decoded to {v:?}, want {text}"),
                Err(e) => assert_eq!(e.to_string(), *text, "{input:?}"),
            }
        }
    }

    #[test]
    fn fmt_u_latin1() {
        assert_eq!(fmt_u(b' '), "U+0020 ' '");
        assert_eq!(fmt_u(0x7f), "U+007F");
        assert_eq!(fmt_u(0x80), "U+0080");
        assert_eq!(fmt_u(0xa0), "U+00A0");
        assert_eq!(fmt_u(0xa1), "U+00A1 '¡'");
        assert_eq!(fmt_u(0xad), "U+00AD");
        assert_eq!(fmt_u(0xff), "U+00FF 'ÿ'");
    }
}
