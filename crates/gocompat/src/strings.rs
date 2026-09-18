//! `strings.ToLower/ToUpper/TrimSpace/FieldsFunc`, go1.26.5, and the `unicode/utf8` decoding that they
//! and [`crate::quote`] share.

use crate::quote::is_space;
use crate::tables::{TO_LOWER, TO_UPPER};

/// `utf8.RuneError`.
pub(crate) const RUNE_ERROR: char = char::REPLACEMENT_CHARACTER;

/// `utf8.UTFMax`.
const UTF_MAX: usize = 4;

/// `strings.ToLower`: ASCII fast path, else `strings.Map(unicode.ToLower)`. Each invalid UTF-8 byte
/// becomes U+FFFD; the mapping is simple (not full) case mapping.
pub fn to_lower(s: &[u8]) -> Vec<u8> {
    if s.is_ascii() {
        // Optimize for ASCII-only strings.
        return s.to_ascii_lowercase();
    }
    map(s, unicode_to_lower)
}

/// `strings.ToUpper`, with the same rules as [`to_lower`].
pub fn to_upper(s: &[u8]) -> Vec<u8> {
    if s.is_ascii() {
        // Optimize for ASCII-only strings.
        return s.to_ascii_uppercase();
    }
    map(s, unicode_to_upper)
}

/// `strings.TrimSpace` (Unicode White_Space).
pub fn trim_space(s: &[u8]) -> &[u8] {
    // Fast path for ASCII: look for the first ASCII non-space byte.
    for (lo, &c) in s.iter().enumerate() {
        if c >= 0x80 {
            // A non-ASCII byte: fall back to the Unicode-aware method on the remaining bytes.
            return trim_func(tail(s, lo), is_space);
        }
        if is_ascii_space(c) {
            continue;
        }
        let s = tail(s, lo);
        // Now look for the first ASCII non-space byte from the end.
        for (hi, &c) in s.iter().enumerate().rev() {
            if c >= 0x80 {
                return trim_right_func(head(s, hi + 1), is_space);
            }
            if !is_ascii_space(c) {
                // s[..=hi] starts and ends with ASCII non-space bytes: done.
                return head(s, hi + 1);
            }
        }
    }
    &[]
}

/// `strings.FieldsFunc`: the maximal non-empty runs of runes that `is_sep` rejects, in order. Each
/// invalid UTF-8 byte is passed to `is_sep` as U+FFFD.
pub fn fields_func(s: &[u8], is_sep: impl Fn(char) -> bool) -> Vec<&[u8]> {
    // Find the field start and end indices first, as Go does.
    let mut spans: Vec<(usize, usize)> = Vec::with_capacity(32);
    let mut start: Option<usize> = None;
    for (end, r, _) in runes(s) {
        if is_sep(r) {
            if let Some(st) = start.take() {
                spans.push((st, end));
            }
        } else if start.is_none() {
            start = Some(end);
        }
    }
    // The last field might end at EOF.
    if let Some(st) = start {
        spans.push((st, s.len()));
    }
    spans
        .into_iter()
        .filter_map(|(st, end)| s.get(st..end))
        .collect()
}

/// `unicode.ToLower`: the simple lower-case mapping of `r`, or `r` itself.
pub(crate) fn unicode_to_lower(r: char) -> char {
    if r.is_ascii() {
        return r.to_ascii_lowercase();
    }
    lookup(TO_LOWER, r)
}

/// `unicode.ToUpper`: the simple upper-case mapping of `r`, or `r` itself.
pub(crate) fn unicode_to_upper(r: char) -> char {
    if r.is_ascii() {
        return r.to_ascii_uppercase();
    }
    lookup(TO_UPPER, r)
}

/// `utf8.DecodeRuneInString`: the first rune of `s` and its width in bytes. An invalid encoding
/// (malformed, overlong, a surrogate, above U+10FFFF, or truncated) gives `(U+FFFD, 1)`, and an empty
/// `s` gives `(U+FFFD, 0)`.
pub(crate) fn decode_rune(s: &[u8]) -> (char, usize) {
    let Some((&b0, rest)) = s.split_first() else {
        return (RUNE_ERROR, 0);
    };
    if b0 < 0x80 {
        return (char::from(b0), 1);
    }
    // Go's `first` and `acceptRanges` tables: the length of the sequence and the range of its
    // second byte.
    let (size, lo, hi): (usize, u8, u8) = match b0 {
        0xc2..=0xdf => (2, 0x80, 0xbf),
        0xe0 => (3, 0xa0, 0xbf),
        0xe1..=0xec | 0xee..=0xef => (3, 0x80, 0xbf),
        0xed => (3, 0x80, 0x9f),
        0xf0 => (4, 0x90, 0xbf),
        0xf1..=0xf3 => (4, 0x80, 0xbf),
        0xf4 => (4, 0x80, 0x8f),
        _ => return (RUNE_ERROR, 1),
    };
    let second = |b: u8| (lo..=hi).contains(&b);
    let cont = |b: u8| (0x80..=0xbf).contains(&b);
    let v = match (size, rest) {
        (2, &[b1, ..]) if second(b1) => (u32::from(b0 & 0x1f) << 6) | u32::from(b1 & 0x3f),
        (3, &[b1, b2, ..]) if second(b1) && cont(b2) => {
            (u32::from(b0 & 0x0f) << 12) | (u32::from(b1 & 0x3f) << 6) | u32::from(b2 & 0x3f)
        }
        (4, &[b1, b2, b3, ..]) if second(b1) && cont(b2) && cont(b3) => {
            (u32::from(b0 & 0x07) << 18)
                | (u32::from(b1 & 0x3f) << 12)
                | (u32::from(b2 & 0x3f) << 6)
                | u32::from(b3 & 0x3f)
        }
        _ => return (RUNE_ERROR, 1),
    };
    match char::from_u32(v) {
        Some(c) => (c, size),
        None => (RUNE_ERROR, 1),
    }
}

/// `utf8.DecodeLastRuneInString`: the last rune of `s` and its width in bytes, `(U+FFFD, 1)` for an
/// invalid encoding and `(U+FFFD, 0)` for an empty `s`.
pub(crate) fn decode_last_rune(s: &[u8]) -> (char, usize) {
    let end = s.len();
    let Some(&last) = s.last() else {
        return (RUNE_ERROR, 0);
    };
    if last < 0x80 {
        return (char::from(last), 1);
    }
    // Guard against O(n^2) behaviour when traversing backwards through long runs of invalid UTF-8.
    let lim = end.saturating_sub(UTF_MAX);
    let before = s.get(lim..end - 1).unwrap_or_default();
    let start = match before.iter().rposition(|&b| rune_start(b)) {
        Some(p) => lim + p,
        None => lim.saturating_sub(1),
    };
    let (r, size) = decode_rune(tail(s, start));
    if start + size != end {
        return (RUNE_ERROR, 1);
    }
    (r, size)
}

/// Go's `for i, r := range s` over a string: the byte offset, rune and width of every rune, with
/// `(U+FFFD, 1)` for each invalid byte.
pub(crate) fn runes(s: &[u8]) -> Runes<'_> {
    Runes { s, off: 0 }
}

/// The iterator of [`runes`].
pub(crate) struct Runes<'a> {
    s: &'a [u8],
    off: usize,
}

impl Iterator for Runes<'_> {
    type Item = (usize, char, usize);

    fn next(&mut self) -> Option<Self::Item> {
        let rest = self.s.get(self.off..)?;
        if rest.is_empty() {
            return None;
        }
        let (r, width) = decode_rune(rest);
        let off = self.off;
        self.off += width.max(1);
        Some((off, r, width))
    }
}

/// `strings.Map` for a mapping that never drops a rune: each rune of `s` becomes `mapping(r)`, and each
/// invalid UTF-8 byte `mapping(U+FFFD)`.
fn map(s: &[u8], mapping: fn(char) -> char) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len().saturating_add(UTF_MAX));
    let mut enc = [0u8; UTF_MAX];
    for (_, r, _) in runes(s) {
        out.extend_from_slice(mapping(r).encode_utf8(&mut enc).as_bytes());
    }
    out
}

/// Looks `r` up in a table of `(rune, mapped)` pairs sorted by rune.
fn lookup(table: &[(char, char)], r: char) -> char {
    match table.binary_search_by_key(&r, |&(from, _)| from) {
        Ok(i) => table.get(i).map_or(r, |&(_, to)| to),
        Err(_) => r,
    }
}

/// `strings.asciiSpace`.
fn is_ascii_space(c: u8) -> bool {
    matches!(c, b'\t' | b'\n' | b'\x0b' | b'\x0c' | b'\r' | b' ')
}

/// `utf8.RuneStart`.
fn rune_start(b: u8) -> bool {
    b & 0xc0 != 0x80
}

/// `s[i:]`, empty when out of range.
fn tail(s: &[u8], i: usize) -> &[u8] {
    s.get(i..).unwrap_or_default()
}

/// `s[:i]`, empty when out of range.
fn head(s: &[u8], i: usize) -> &[u8] {
    s.get(..i).unwrap_or_default()
}

/// `strings.TrimFunc`.
fn trim_func(s: &[u8], f: impl Fn(char) -> bool + Copy) -> &[u8] {
    trim_right_func(trim_left_func(s, f), f)
}

/// `strings.TrimLeftFunc`.
fn trim_left_func(s: &[u8], f: impl Fn(char) -> bool) -> &[u8] {
    match index_func(s, f, false) {
        Some(i) => tail(s, i),
        None => &[],
    }
}

/// `strings.TrimRightFunc`.
fn trim_right_func(s: &[u8], f: impl Fn(char) -> bool) -> &[u8] {
    let end = match last_index_func(s, f, false) {
        Some(i) => i + decode_rune(tail(s, i)).1,
        None => 0,
    };
    head(s, end)
}

/// `strings.indexFunc`: the offset of the first rune with `f(r) == truth`.
fn index_func(s: &[u8], f: impl Fn(char) -> bool, truth: bool) -> Option<usize> {
    runes(s).find(|&(_, r, _)| f(r) == truth).map(|(i, _, _)| i)
}

/// `strings.lastIndexFunc`: the offset of the last rune with `f(r) == truth`, decoding backwards.
fn last_index_func(s: &[u8], f: impl Fn(char) -> bool, truth: bool) -> Option<usize> {
    let mut i = s.len();
    while i > 0 {
        let (r, size) = decode_last_rune(head(s, i));
        i = i.saturating_sub(size.max(1));
        if f(r) == truth {
            return Some(i);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// strings/strings_test.go `upperTests`.
    #[test]
    fn go_upper_tests() {
        let cases = [
            ("", ""),
            ("ONLYUPPER", "ONLYUPPER"),
            ("abc", "ABC"),
            ("AbC123", "ABC123"),
            ("azAZ09_", "AZAZ09_"),
            (
                "longStrinGwitHmixofsmaLLandcAps",
                "LONGSTRINGWITHMIXOFSMALLANDCAPS",
            ),
            (
                "RENAN BASTOS 93 AOSDAJDJAIDJAIDAJIaidsjjaidijadsjiadjiOOKKO",
                "RENAN BASTOS 93 AOSDAJDJAIDJAIDAJIAIDSJJAIDIJADSJIADJIOOKKO",
            ),
            (
                "long\u{0250}string\u{0250}with\u{0250}nonascii\u{2C6F}chars",
                "LONG\u{2C6F}STRING\u{2C6F}WITH\u{2C6F}NONASCII\u{2C6F}CHARS",
            ),
            (
                "\u{0250}\u{0250}\u{0250}\u{0250}\u{0250}",
                "\u{2C6F}\u{2C6F}\u{2C6F}\u{2C6F}\u{2C6F}",
            ),
            ("a\u{0080}\u{10FFFF}", "A\u{0080}\u{10FFFF}"),
        ];
        for (input, want) in cases {
            assert_eq!(
                to_upper(input.as_bytes()),
                want.as_bytes(),
                "ToUpper({input:?})"
            );
        }
    }

    /// strings/strings_test.go `lowerTests`.
    #[test]
    fn go_lower_tests() {
        let cases = [
            ("", ""),
            ("abc", "abc"),
            ("AbC123", "abc123"),
            ("azAZ09_", "azaz09_"),
            (
                "longStrinGwitHmixofsmaLLandcAps",
                "longstringwithmixofsmallandcaps",
            ),
            (
                "renan bastos 93 AOSDAJDJAIDJAIDAJIaidsjjaidijadsjiadjiOOKKO",
                "renan bastos 93 aosdajdjaidjaidajiaidsjjaidijadsjiadjiookko",
            ),
            (
                "LONG\u{2C6F}STRING\u{2C6F}WITH\u{2C6F}NONASCII\u{2C6F}CHARS",
                "long\u{0250}string\u{0250}with\u{0250}nonascii\u{0250}chars",
            ),
            (
                "\u{2C6D}\u{2C6D}\u{2C6D}\u{2C6D}\u{2C6D}",
                "\u{0251}\u{0251}\u{0251}\u{0251}\u{0251}",
            ),
            ("A\u{0080}\u{10FFFF}", "a\u{0080}\u{10FFFF}"),
        ];
        for (input, want) in cases {
            assert_eq!(
                to_lower(input.as_bytes()),
                want.as_bytes(),
                "ToLower({input:?})"
            );
        }
    }

    /// strings/strings_test.go `trimSpaceTests`.
    #[test]
    fn go_trim_space_tests() {
        const SPACE: &str = "\t\u{0b}\r\u{0c}\n\u{85}\u{a0}\u{2000}\u{3000}";
        let spaced = format!("{SPACE}abc{SPACE}");
        let smiley = [&b"x "[..], "\u{263a}".as_bytes(), b"\xc0\xc0 "].concat();
        let smiley_want = [&b"x "[..], "\u{263a}".as_bytes(), b"\xc0\xc0"].concat();
        let cases: [(&[u8], &[u8]); 16] = [
            (b"", b""),
            (b"abc", b"abc"),
            (spaced.as_bytes(), b"abc"),
            (b" ", b""),
            (b" \t\r\n \t\t\r\r\n\n ", b""),
            (b" \t\r\n x\t\t\r\r\n\n ", b"x"),
            (
                " \u{2000}\t\r\n x\t\t\r\r\ny\n \u{3000}".as_bytes(),
                b"x\t\t\r\r\ny",
            ),
            (b"1 \t\r\n2", b"1 \t\r\n2"),
            (b" x\x80", b"x\x80"),
            (b" x\xc0", b"x\xc0"),
            (b"x \xc0\xc0 ", b"x \xc0\xc0"),
            (b"x \xc0", b"x \xc0"),
            (b"x \xc0 ", b"x \xc0"),
            (b"x \xc0\xc0 ", b"x \xc0\xc0"),
            (&smiley, &smiley_want),
            ("x \u{263a} ".as_bytes(), "x \u{263a}".as_bytes()),
        ];
        for (input, want) in cases {
            assert_eq!(trim_space(input), want, "TrimSpace({input:x?})");
        }
    }

    /// strings/strings_test.go `TestFieldsFunc`: `fieldstests` with `unicode.IsSpace`.
    #[test]
    fn go_fieldstests() {
        let faces = "\u{263a}\u{263b}\u{2639}";
        let cases: &[(&str, &[&str])] = &[
            ("", &[]),
            (" ", &[]),
            (" \t ", &[]),
            ("\u{2000}", &[]),
            ("  abc  ", &["abc"]),
            ("1 2 3 4", &["1", "2", "3", "4"]),
            ("1  2  3  4", &["1", "2", "3", "4"]),
            ("1\t\t2\t\t3\t4", &["1", "2", "3", "4"]),
            ("1\u{2000}2\u{2001}3\u{2002}4", &["1", "2", "3", "4"]),
            ("\u{2000}\u{2001}\u{2002}", &[]),
            ("\n\u{2122}\t\u{2122}\n", &["\u{2122}", "\u{2122}"]),
            (
                "\n\u{2000}1\u{2122}2\u{2000} \u{2001} \u{2122}",
                &["1\u{2122}2", "\u{2122}"],
            ),
            (
                "\n1\u{FFFD} \u{FFFD}2\u{2000}3\u{FFFD}4",
                &["1\u{FFFD}", "\u{FFFD}2", "3\u{FFFD}4"],
            ),
            (faces, &[faces]),
        ];
        for &(input, want) in cases {
            let want: Vec<&[u8]> = want.iter().map(|w| w.as_bytes()).collect();
            assert_eq!(
                fields_func(input.as_bytes(), is_space),
                want,
                "FieldsFunc({input:?})"
            );
        }
        let invalid = [&b"1\xff"[..], "\u{2000}".as_bytes(), b"\xff2\xff \xff"].concat();
        assert_eq!(
            fields_func(&invalid, is_space),
            [&b"1\xff"[..], b"\xff2\xff", b"\xff"]
        );
    }

    /// strings/strings_test.go `TestFieldsFunc`: `FieldsFuncTests` with `c == 'X'`.
    #[test]
    fn go_fields_func_tests() {
        let cases: &[(&str, &[&str])] = &[
            ("", &[]),
            ("XX", &[]),
            ("XXhiXXX", &["hi"]),
            ("aXXbXXXcX", &["a", "b", "c"]),
        ];
        for &(input, want) in cases {
            let want: Vec<&[u8]> = want.iter().map(|w| w.as_bytes()).collect();
            assert_eq!(
                fields_func(input.as_bytes(), |c| c == 'X'),
                want,
                "FieldsFunc({input:?})"
            );
        }
    }

    #[test]
    fn invalid_utf8_becomes_replacement_per_byte() {
        assert_eq!(to_lower(b"A\xffB"), "a\u{fffd}b".as_bytes());
        assert_eq!(
            to_upper(b"\xed\xa0\x80z"),
            "\u{fffd}\u{fffd}\u{fffd}Z".as_bytes()
        );
        assert_eq!(to_upper("a\u{fffd}b".as_bytes()), "A\u{fffd}B".as_bytes());
        // ASCII-only input keeps every byte but the letters.
        assert_eq!(to_lower(b"\x00\x7fABC"), b"\x00\x7fabc");
    }

    /// codec-wire-ticket §2.4.8: `ToLower` then `ToUpper` maps the four lookalikes to ASCII.
    #[test]
    fn lookalikes_map_to_ascii() {
        assert_eq!(to_upper("\u{131}\u{17f}".as_bytes()), b"IS");
        assert_eq!(to_lower("\u{130}\u{212a}".as_bytes()), b"ik");
        assert_eq!(
            to_upper(&to_lower("\u{130}\u{131}\u{17f}\u{212a}".as_bytes())),
            b"IISK"
        );
        // U+0130 shrinks from 2 bytes to 1 and U+212A from 3 bytes to 1.
        assert_eq!(to_lower("\u{130}".as_bytes()).len(), 1);
        assert_eq!(to_lower("\u{212a}".as_bytes()).len(), 1);
        // Simple, not full, mapping: U+00DF has no simple upper-case form; U+1E9E lower-cases to it.
        assert_eq!(to_upper("stra\u{df}e".as_bytes()), "STRA\u{df}E".as_bytes());
        assert_eq!(to_lower("\u{1e9e}".as_bytes()), "\u{df}".as_bytes());
        assert_eq!(to_upper("\u{345}".as_bytes()), "\u{399}".as_bytes());
    }

    #[test]
    fn decode_rune_as_go() {
        assert_eq!(decode_rune(b""), (RUNE_ERROR, 0));
        assert_eq!(decode_rune(b"a"), ('a', 1));
        assert_eq!(decode_rune("\u{e9}".as_bytes()), ('\u{e9}', 2));
        assert_eq!(decode_rune("\u{20ac}x".as_bytes()), ('\u{20ac}', 3));
        assert_eq!(decode_rune("\u{1f600}".as_bytes()), ('\u{1f600}', 4));
        assert_eq!(decode_rune("\u{fffd}".as_bytes()), (RUNE_ERROR, 3));
        assert_eq!(decode_rune(b"\xed\x9f\xbf"), ('\u{d7ff}', 3));
        assert_eq!(decode_rune(b"\xee\x80\x80"), ('\u{e000}', 3));
        assert_eq!(decode_rune(b"\xe0\xa0\x80"), ('\u{800}', 3));
        assert_eq!(decode_rune(b"\xf0\x90\x80\x80"), ('\u{10000}', 4));
        assert_eq!(decode_rune(b"\xf4\x8f\xbf\xbf"), ('\u{10ffff}', 4));
        let invalid: [&[u8]; 13] = [
            b"\x80",
            b"\xc0\x80",
            b"\xc1\xbf",
            b"\xe0\x9f\xbf",
            b"\xed\xa0\x80",
            b"\xf0\x8f\xbf\xbf",
            b"\xf4\x90\x80\x80",
            b"\xf5\x80\x80\x80",
            b"\xff",
            b"\xc3",
            b"\xe2\x82",
            b"\xf0\x9f\x98",
            b"\xe2\x28\xa1",
        ];
        for s in invalid {
            assert_eq!(decode_rune(s), (RUNE_ERROR, 1), "{s:x?}");
        }
    }

    #[test]
    fn decode_last_rune_as_go() {
        assert_eq!(decode_last_rune(b""), (RUNE_ERROR, 0));
        assert_eq!(decode_last_rune(b"ab"), ('b', 1));
        assert_eq!(decode_last_rune("a\u{20ac}".as_bytes()), ('\u{20ac}', 3));
        assert_eq!(decode_last_rune("\u{1f600}".as_bytes()), ('\u{1f600}', 4));
        assert_eq!(decode_last_rune(b"a\xe2\x82"), (RUNE_ERROR, 1));
        assert_eq!(decode_last_rune(b"\xe3\xe3\x80\x80"), ('\u{3000}', 3));
        assert_eq!(decode_last_rune(b"\xe3\x80\x80\x80\x80"), (RUNE_ERROR, 1));
        assert_eq!(decode_last_rune(b"\x80"), (RUNE_ERROR, 1));
        assert_eq!(decode_last_rune(b"\xed\xa0\x80"), (RUNE_ERROR, 1));
    }

    #[test]
    fn runes_as_go_range_loop() {
        let got: Vec<(usize, char, usize)> = runes(b"a\xe2\x82\xac\xff\xe2\x82z").collect();
        assert_eq!(
            got,
            [
                (0, 'a', 1),
                (1, '\u{20ac}', 3),
                (4, RUNE_ERROR, 1),
                (5, RUNE_ERROR, 1),
                (6, RUNE_ERROR, 1),
                (7, 'z', 1)
            ]
        );
    }
}
