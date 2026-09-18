//! `strconv.Quote` (`strconv/quote.go`, `isprint.go`) and `unicode.IsSpace`, go1.26.5.

use crate::strings::runes;
use crate::tables::{IS_NOT_PRINT16, IS_NOT_PRINT32, IS_PRINT16, IS_PRINT32, WHITE_SPACE};

/// `strconv.Quote` over the bytes of a Go string: `\xNN` for each invalid UTF-8 byte.
///
/// Printable runes ([`is_print`]) are kept, `"` and `\` are backslashed, `\a \b \f \n \r \t \v` use
/// those escapes, the other runes below U+0020 and U+007F become `\xNN`, and the remaining runes
/// `\uNNNN` or `\UNNNNNNNN`, with lower-case hex digits.
pub fn quote(s: &[u8]) -> String {
    let mut buf = String::with_capacity(s.len().saturating_add(2));
    buf.push('"');
    for (i, r, width) in runes(s) {
        if width == 1 && r == char::REPLACEMENT_CHARACTER {
            if let Some(&b) = s.get(i) {
                push_hex(&mut buf, "\\x", u32::from(b), 2);
            }
            continue;
        }
        append_escaped_rune(&mut buf, r);
    }
    buf.push('"');
    buf
}

/// `strconv.IsPrint` (the same set as `unicode.IsPrint`): letters, marks, numbers, punctuation,
/// symbols and U+0020.
pub fn is_print(r: char) -> bool {
    let r = u32::from(r);
    // Fast check for Latin-1.
    if r <= 0xff {
        if (0x20..=0x7e).contains(&r) {
            // All the ASCII is printable from space through DEL-1.
            return true;
        }
        if (0xa1..=0xff).contains(&r) {
            // Similarly for ¡ through ÿ, except for the soft hyphen.
            return r != 0xad;
        }
        return false;
    }
    if let Ok(rr) = u16::try_from(r) {
        return in_ranges(IS_PRINT16, rr) && IS_NOT_PRINT16.binary_search(&rr).is_err();
    }
    if !in_ranges(IS_PRINT32, r) {
        return false;
    }
    if r >= 0x20000 {
        return true;
    }
    match u16::try_from(r - 0x10000) {
        Ok(rr) => IS_NOT_PRINT32.binary_search(&rr).is_err(),
        Err(_) => true,
    }
}

/// `unicode.IsSpace`: the White_Space property, which in Latin-1 is `\t \n \v \f \r`, U+0020,
/// U+0085 (NEL) and U+00A0 (NBSP).
pub fn is_space(r: char) -> bool {
    if u32::from(r) <= 0xff {
        return matches!(
            r,
            '\t' | '\n' | '\u{0b}' | '\u{0c}' | '\r' | ' ' | '\u{85}' | '\u{a0}'
        );
    }
    WHITE_SPACE.binary_search(&r).is_ok()
}

/// Whether `v` lies in one of the inclusive ranges that `table` holds as sorted pairs `lo, hi`: the
/// `bsearch` and `isPrint[i&^1] <= v <= isPrint[i|1]` test of `strconv.IsPrint`.
fn in_ranges<T: Copy + Ord>(table: &[T], v: T) -> bool {
    let i = table.partition_point(|&x| x < v);
    matches!(
        (table.get(i & !1), table.get(i | 1)),
        (Some(&lo), Some(&hi)) if lo <= v && v <= hi
    )
}

/// `appendEscapedRune` with `quote = '"'`, `ASCIIonly = false` and `graphicOnly = false`.
fn append_escaped_rune(buf: &mut String, r: char) {
    if r == '"' || r == '\\' {
        // Always backslashed.
        buf.push('\\');
        buf.push(r);
        return;
    }
    if is_print(r) {
        buf.push(r);
        return;
    }
    match r {
        '\u{07}' => buf.push_str("\\a"),
        '\u{08}' => buf.push_str("\\b"),
        '\u{0c}' => buf.push_str("\\f"),
        '\n' => buf.push_str("\\n"),
        '\r' => buf.push_str("\\r"),
        '\t' => buf.push_str("\\t"),
        '\u{0b}' => buf.push_str("\\v"),
        _ => {
            let v = u32::from(r);
            if v < 0x20 || v == 0x7f {
                push_hex(buf, "\\x", v, 2);
            } else if v < 0x10000 {
                push_hex(buf, "\\u", v, 4);
            } else {
                push_hex(buf, "\\U", v, 8);
            }
        }
    }
}

/// Appends `prefix`, then the low `digits` hex digits of `v` in lower case, most significant first.
fn push_hex(buf: &mut String, prefix: &str, v: u32, digits: u32) {
    buf.push_str(prefix);
    for k in (0..digits).rev() {
        let nibble = v.checked_shr(4 * k).unwrap_or(0) & 0xf;
        buf.push(char::from_digit(nibble, 16).unwrap_or('0'));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tables::IS_GRAPHIC;

    fn q(s: &[u8]) -> String {
        quote(s)
    }

    /// strconv/quote_test.go `quotetests`, the `out` column.
    #[test]
    fn go_quotetests() {
        assert_eq!(q(b"\x07\x08\x0c\r\n\t\x0b"), r#""\a\b\f\r\n\t\v""#);
        assert_eq!(q(b"\\"), r#""\\""#);
        assert_eq!(q(b"abc\xffdef"), r#""abc\xffdef""#);
        assert_eq!(q("\u{263a}".as_bytes()), "\"\u{263a}\"");
        assert_eq!(q("\u{10ffff}".as_bytes()), r#""\U0010ffff""#);
        assert_eq!(q(b"\x04"), r#""\x04""#);
        assert_eq!(
            q("!\u{a0}!\u{2000}!\u{3000}!".as_bytes()),
            r#""!\u00a0!\u2000!\u3000!""#
        );
        assert_eq!(q(b"\x7f"), r#""\x7f""#);
    }

    /// The probes of port-notes codec-wire-ticket §2.4.9, view-placement §3.5 and client-core §3.5.
    #[test]
    fn port_note_probes() {
        assert_eq!(q("zz\x01\"é".as_bytes()), r#""zz\x01\"é""#);
        assert_eq!(q(b"ab\tcd"), r#""ab\tcd""#);
        assert_eq!(q("héllo 😀".as_bytes()), "\"héllo 😀\"");
        assert_eq!(q(b"\xff\xfe"), r#""\xff\xfe""#);
        assert_eq!(q(br#"quote"back\slash"#), r#""quote\"back\\slash""#);
        assert_eq!(q(b"\x00\x7f"), r#""\x00\x7f""#);
        assert_eq!(q(b""), r#""""#);
        assert_eq!(q(b"zone-a"), r#""zone-a""#);
        assert_eq!(q(b"a\x01b"), r#""a\x01b""#);
        assert_eq!(q("a\u{a0}b".as_bytes()), r#""a\u00a0b""#);
    }

    #[test]
    fn invalid_utf8_is_escaped_per_byte() {
        // Surrogate encodings, overlong forms, runes above U+10FFFF and truncated runes.
        assert_eq!(q(b"\xed\xa0\x80"), r#""\xed\xa0\x80""#);
        assert_eq!(q(b"\xc0\x80"), r#""\xc0\x80""#);
        assert_eq!(q(b"\xe0\x9f\xbf"), r#""\xe0\x9f\xbf""#);
        assert_eq!(q(b"\xf4\x90\x80\x80"), r#""\xf4\x90\x80\x80""#);
        assert_eq!(q(b"a\xe2\x82b"), r#""a\xe2\x82b""#);
        assert_eq!(q(b"\xe2\xe2\x82\xac"), "\"\\xe2\u{20ac}\"");
        // A valid U+FFFD is printable and kept.
        assert_eq!(q("\u{fffd}".as_bytes()), "\"\u{fffd}\"");
    }

    #[test]
    fn escapes_by_plane() {
        assert_eq!(q("\u{378}".as_bytes()), r#""\u0378""#);
        assert_eq!(q("\u{feff}".as_bytes()), r#""\ufeff""#);
        assert_eq!(q("\u{e0001}".as_bytes()), r#""\U000e0001""#);
        assert_eq!(q("\u{1f600}".as_bytes()), "\"\u{1f600}\"");
        assert_eq!(q("\u{85}".as_bytes()), r#""\u0085""#);
    }

    #[test]
    fn is_print_latin1() {
        for b in 0u8..=0xff {
            let want = (0x20..=0x7e).contains(&b) || (b >= 0xa1 && b != 0xad);
            assert_eq!(is_print(char::from(b)), want, "U+{b:04X}");
        }
    }

    #[test]
    fn is_print_tables() {
        // isPrint16 ranges, an isNotPrint16 exception and a gap.
        assert!(is_print('\u{0100}'));
        assert!(is_print('\u{038c}'));
        assert!(!is_print('\u{038b}'));
        assert!(!is_print('\u{0378}'));
        assert!(!is_print('\u{200b}'));
        assert!(!is_print('\u{d7ff}'));
        assert!(!is_print('\u{ffff}'));
        // isPrint32 ranges, an isNotPrint32 exception, and the U+20000 shortcut.
        assert!(is_print('\u{10000}'));
        assert!(!is_print('\u{1000c}'));
        assert!(is_print('\u{1000d}'));
        assert!(is_print('\u{20000}'));
        assert!(!is_print('\u{e0001}'));
        assert!(is_print('\u{e0100}'));
        assert!(!is_print('\u{10ffff}'));
    }

    #[test]
    fn graphic_list_holds_the_space_separators() {
        assert_eq!(
            IS_GRAPHIC,
            [
                0x00a0, 0x1680, 0x2000, 0x2001, 0x2002, 0x2003, 0x2004, 0x2005, 0x2006, 0x2007,
                0x2008, 0x2009, 0x200a, 0x202f, 0x205f, 0x3000
            ]
        );
        for &v in IS_GRAPHIC {
            let c = char::from_u32(u32::from(v));
            assert!(c.is_some_and(|c| !is_print(c) && is_space(c)), "U+{v:04X}");
        }
    }

    #[test]
    fn tables_are_sorted_pairs() {
        assert_eq!(IS_PRINT16.len() % 2, 0);
        assert_eq!(IS_PRINT32.len() % 2, 0);
        assert!(IS_PRINT16.windows(2).all(|w| w[0] <= w[1]));
        assert!(IS_PRINT32.windows(2).all(|w| w[0] <= w[1]));
        assert!(IS_NOT_PRINT16.windows(2).all(|w| w[0] < w[1]));
        assert!(IS_NOT_PRINT32.windows(2).all(|w| w[0] < w[1]));
        assert!(WHITE_SPACE.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn is_space_set() {
        let spaces: Vec<char> = "\t\n\u{0b}\u{0c}\r \u{85}\u{a0}\u{1680}\u{2000}\u{2001}\u{2002}\u{2003}\u{2004}\u{2005}\u{2006}\u{2007}\u{2008}\u{2009}\u{200a}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}"
            .chars()
            .collect();
        assert_eq!(WHITE_SPACE, spaces.as_slice());
        for c in spaces {
            assert!(is_space(c), "U+{:04X}", u32::from(c));
        }
        for c in [
            '\u{1c}', '\u{1f}', 'x', '\u{180e}', '\u{200b}', '\u{2060}', '\u{feff}',
        ] {
            assert!(!is_space(c), "U+{:04X}", u32::from(c));
        }
    }
}
