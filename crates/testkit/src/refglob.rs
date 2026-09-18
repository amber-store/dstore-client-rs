//! dstore `refglob/refglob.go` (v0.1.9), for the fake node's `watch` patterns.
//!
//! Go compiles a pattern to an RE2 regexp: `*` → `[^/]*`, `?` → `[^/]`, `**` as a whole final segment →
//! `.*` (which does not match `\n`), `**/` at a segment start → `(?:[^/]*/)*`, `[...]` → a class that
//! never matches `/`, `\x` → the literal `x`. RE2 is linear in the name; this port keeps that property
//! by simulating the equivalent automaton directly instead of building a regexp.
//!
//! The regexp Go builds from a validated pattern always compiles (every literal is quoted, class ranges
//! are ordered), so `Compile`'s `refglob: %w` wrapping of a regexp error has no reachable case here.

use std::fmt;

use dstore_gocompat::quote::quote;

/// `refglob.MaxLen`: the bound on a pattern's length in bytes.
pub const MAX_LEN: usize = 4096;

/// A compiled reference-name glob (`refglob.Pattern`).
#[derive(Clone, Debug)]
pub struct Glob {
    pattern: String,
    prefix: String,
    prog: Vec<Op>,
}

/// One step of the compiled pattern, the RE2 fragment Go writes for it in the comment.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Op {
    /// A literal rune (`regexp.QuoteMeta`).
    Lit(char),
    /// `[^/]`.
    OneNotSlash,
    /// A `[...]` class.
    Class(Class),
    /// `[^/]*`.
    StarNotSlash,
    /// `.*`: any runes but `\n`.
    DotStar,
    /// `(?:[^/]*/)*`: the empty string or any string ending in `/`.
    DirStar,
}

/// A character class: the rune is not `/` and lies in one of the ranges (or, negated, in none).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Class {
    negate: bool,
    ranges: Vec<(char, char)>,
}

impl Class {
    fn matches(&self, c: char) -> bool {
        c != '/' && self.ranges.iter().any(|&(lo, hi)| lo <= c && c <= hi) != self.negate
    }
}

/// `refglob.Compile` with Go's error texts.
pub fn compile(pattern: &str) -> Result<Glob, String> {
    compile_bytes(pattern.as_bytes())
}

/// `refglob.Compile` over the bytes of a Go string, so that patterns which are not valid UTF-8 get Go's
/// `refglob: pattern is not valid UTF-8` (a `&str` cannot hold them).
///
/// Validation order: empty, length, UTF-8, control characters (runes below U+0020 and U+007F only),
/// then the syntax.
pub fn compile_bytes(pattern: &[u8]) -> Result<Glob, String> {
    if pattern.is_empty() {
        return Err("refglob: empty pattern".to_string());
    }
    if pattern.len() > MAX_LEN {
        return Err(format!("refglob: pattern exceeds {MAX_LEN} bytes"));
    }
    // utf8.ValidString and str::from_utf8 accept the same set (no surrogates, no overlong forms).
    let Ok(src) = std::str::from_utf8(pattern) else {
        return Err("refglob: pattern is not valid UTF-8".to_string());
    };
    if src.chars().any(|r| r < '\u{20}' || r == '\u{7f}') {
        return Err("refglob: pattern contains a control character".to_string());
    }
    let rs: Vec<char> = src.chars().collect();
    let mut prog = Vec::new();
    let mut prefix = String::new();
    let mut literal = true; // still inside the leading literal run
    let mut seg_start = true;
    let mut i = 0;
    while i < rs.len() {
        let r = rs[i];
        match r {
            '\\' => {
                let Some(&escaped) = rs.get(i + 1) else {
                    return Err("refglob: trailing backslash".to_string());
                };
                prog.push(Op::Lit(escaped));
                if literal {
                    prefix.push(escaped);
                }
                seg_start = escaped == '/';
                i += 2;
            }
            '*' => {
                literal = false;
                let double = rs.get(i + 1) == Some(&'*');
                if double && seg_start && (i + 2 == rs.len() || rs[i + 2] == '/') {
                    if i + 2 == rs.len() {
                        prog.push(Op::DotStar);
                        i += 2;
                    } else {
                        prog.push(Op::DirStar);
                        i += 3;
                    }
                    seg_start = true;
                    continue;
                }
                prog.push(Op::StarNotSlash);
                seg_start = false;
                i += 1;
            }
            '?' => {
                literal = false;
                prog.push(Op::OneNotSlash);
                seg_start = false;
                i += 1;
            }
            '[' => {
                literal = false;
                let (class, n) = parse_class(&rs[i..])?;
                prog.push(Op::Class(class));
                seg_start = false;
                i += n;
            }
            _ => {
                prog.push(Op::Lit(r));
                if literal {
                    prefix.push(r);
                }
                seg_start = r == '/';
                i += 1;
            }
        }
    }
    Ok(Glob {
        pattern: src.to_string(),
        prefix,
        prog,
    })
}

/// `parseClass`: a `[...]` class at the start of `rs` and the number of runes consumed.
fn parse_class(rs: &[char]) -> Result<(Class, usize), String> {
    let mut i = 1;
    let mut negate = false;
    if matches!(rs.get(i), Some('!') | Some('^')) {
        negate = true;
        i += 1;
    }
    let mut ranges = Vec::new();
    loop {
        let Some(&c) = rs.get(i) else {
            return Err("refglob: unterminated character class".to_string());
        };
        if c == ']' {
            break;
        }
        let (lo, n) = class_rune(&rs[i..])?;
        i += n;
        let mut hi = lo;
        if i + 1 < rs.len() && rs[i] == '-' && rs[i + 1] != ']' {
            i += 1;
            let (h, n) = class_rune(&rs[i..])?;
            hi = h;
            i += n;
            if hi < lo {
                return Err(format!(
                    "refglob: bad range {}-{}",
                    quote_rune(lo),
                    quote_rune(hi)
                ));
            }
        }
        ranges.push((lo, hi));
    }
    if ranges.is_empty() {
        return Err("refglob: empty character class".to_string());
    }
    // Go carves '/' out of positive ranges and adds it to negated classes; a class listing only '/'
    // becomes a class matching nothing. `Class::matches` rejects '/' first, which is the same set.
    Ok((Class { negate, ranges }, i + 1))
}

/// `classRune`: one (possibly escaped) rune of a class. `rs` is never empty here.
fn class_rune(rs: &[char]) -> Result<(char, usize), String> {
    match rs {
        ['\\', escaped, ..] => Ok((*escaped, 2)),
        ['\\'] => Err("refglob: trailing backslash in character class".to_string()),
        [c, ..] => Ok((*c, 1)),
        [] => Err("refglob: unterminated character class".to_string()),
    }
}

/// `%q` of a rune (`strconv.QuoteRune`). Only `'` and `"` escape differently from `strconv.Quote`.
fn quote_rune(r: char) -> String {
    match r {
        '\'' => "'\\''".to_string(),
        '"' => "'\"'".to_string(),
        _ => {
            let mut buf = [0u8; 4];
            let quoted = quote(r.encode_utf8(&mut buf).as_bytes());
            let inner = quoted
                .strip_prefix('"')
                .and_then(|q| q.strip_suffix('"'))
                .unwrap_or(&quoted);
            format!("'{inner}'")
        }
    }
}

impl Glob {
    /// `Pattern.Match`: whether the whole name matches (`\A…\z`).
    pub fn matches(&self, name: &str) -> bool {
        let n = self.prog.len();
        // at[i]: the automaton is before op i (at[n]: past the end). mid[i]: inside a `DirStar` segment
        // that has consumed a rune other than '/'.
        let mut at = vec![false; n + 1];
        let mut mid = vec![false; n];
        at[0] = true;
        self.close(&mut at);
        let mut next_at = vec![false; n + 1];
        let mut next_mid = vec![false; n];
        for c in name.chars() {
            next_at.iter_mut().for_each(|s| *s = false);
            next_mid.iter_mut().for_each(|s| *s = false);
            let mut alive = false;
            for (i, op) in self.prog.iter().enumerate() {
                if at[i] {
                    alive = true;
                    match op {
                        Op::Lit(x) => {
                            if c == *x {
                                next_at[i + 1] = true;
                            }
                        }
                        Op::OneNotSlash => {
                            if c != '/' {
                                next_at[i + 1] = true;
                            }
                        }
                        Op::Class(class) => {
                            if class.matches(c) {
                                next_at[i + 1] = true;
                            }
                        }
                        Op::StarNotSlash => {
                            if c != '/' {
                                next_at[i] = true;
                            }
                        }
                        Op::DotStar => {
                            if c != '\n' {
                                next_at[i] = true;
                            }
                        }
                        Op::DirStar => {
                            if c == '/' {
                                next_at[i] = true;
                            } else {
                                next_mid[i] = true;
                            }
                        }
                    }
                }
                if mid[i] {
                    alive = true;
                    if c == '/' {
                        next_at[i] = true;
                    } else {
                        next_mid[i] = true;
                    }
                }
            }
            if !alive {
                return false;
            }
            std::mem::swap(&mut at, &mut next_at);
            std::mem::swap(&mut mid, &mut next_mid);
            self.close(&mut at);
        }
        at[n]
    }

    /// The empty-string closure: a starred op may be skipped. Epsilon edges only go forward, so one
    /// ascending pass reaches the fixpoint.
    fn close(&self, at: &mut [bool]) {
        for (i, op) in self.prog.iter().enumerate() {
            if at[i] && matches!(op, Op::StarNotSlash | Op::DotStar | Op::DirStar) {
                at[i + 1] = true;
            }
        }
    }

    /// `Pattern.Prefix`: the literal prefix every matching name starts with.
    pub fn prefix(&self) -> String {
        self.prefix.clone()
    }

    /// `Pattern.String`: the pattern's source.
    pub fn pattern(&self) -> &str {
        &self.pattern
    }
}

impl fmt::Display for Glob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.pattern)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::golden;

    fn must(pattern: &str) -> Glob {
        match compile(pattern) {
            Ok(g) => g,
            Err(e) => panic!("compile({pattern:?}): {e}"),
        }
    }

    /// refglob_test.go TestMatch.
    #[test]
    fn go_test_match() {
        let cases: &[(&str, &str, bool)] = &[
            ("trees/a", "trees/a", true),
            ("trees/a", "trees/ab", false),
            ("trees/*", "trees/a", true),
            ("trees/*", "trees/a/b", false),
            ("trees/*", "trees/", true),
            ("trees/*", "trees", false),
            ("trees/**", "trees/a", true),
            ("trees/**", "trees/a/b", true),
            ("trees/**", "trees/", true),
            ("trees/**", "trees", false),
            ("**", "a", true),
            ("**", "a/b/c", true),
            ("**", "", true),
            ("**/x", "x", true),
            ("**/x", "a/x", true),
            ("**/x", "a/b/x", true),
            ("**/x", "ax", false),
            ("a/**/x", "a/x", true),
            ("a/**/x", "a/b/c/x", true),
            ("a/**/x", "ab/x", false),
            ("a**b", "axxb", true),
            ("a**b", "ax/xb", false),
            ("t?ees/a", "trees/a", true),
            ("t?ees/a", "t/ees/a", false),
            ("trees/[ab]", "trees/a", true),
            ("trees/[ab]", "trees/c", false),
            ("trees/[!ab]", "trees/c", true),
            ("trees/[^ab]", "trees/a", false),
            ("trees/[a-c]x", "trees/bx", true),
            ("trees/[/]", "trees//", false),
            (r"trees/\*", "trees/*", true),
            (r"trees/\*", "trees/a", false),
            (r"trees/\\", r"trees/\", true),
            ("a.b", "a.b", true),
            ("a.b", "axb", false),
            ("ünï/*", "ünï/cödé", true),
            ("*", "abc", true),
            ("*", "a/b", false),
        ];
        for &(pattern, name, want) in cases {
            assert_eq!(
                must(pattern).matches(name),
                want,
                "{pattern:?}.Match({name:?})"
            );
        }
    }

    /// refglob_test.go TestPrefix.
    #[test]
    fn go_test_prefix() {
        let cases: &[(&str, &str)] = &[
            ("trees/a", "trees/a"),
            ("trees/*", "trees/"),
            ("trees/**", "trees/"),
            ("**", ""),
            ("tr?es/a", "tr"),
            ("trees/[ab]", "trees/"),
            (r"trees/\*x", "trees/*x"),
            (r"a\\*", r"a\"),
        ];
        for &(pattern, prefix) in cases {
            let g = must(pattern);
            assert_eq!(g.prefix(), prefix, "{pattern:?}.Prefix()");
            assert_eq!(g.pattern(), pattern, "{pattern:?}.String()");
            assert_eq!(g.to_string(), pattern);
        }
    }

    /// refglob_test.go TestInvalid.
    #[test]
    fn go_test_invalid() {
        for pattern in ["", "trees/[ab", r"trees/\", "a[]b", "\x00", "[z-a]"] {
            assert!(compile(pattern).is_err(), "compile({pattern:?}) succeeded");
        }
    }

    #[derive(serde::Deserialize)]
    struct Vectors {
        #[serde(rename = "match")]
        matches: Vec<MatchCase>,
        prefix: Vec<PrefixCase>,
        invalid: Vec<InvalidCase>,
    }

    #[derive(serde::Deserialize)]
    struct MatchCase {
        pattern: String,
        name: String,
        #[serde(rename = "match")]
        want: bool,
    }

    #[derive(serde::Deserialize)]
    struct PrefixCase {
        pattern: String,
        prefix: String,
    }

    #[derive(serde::Deserialize)]
    struct InvalidCase {
        pattern: Option<String>,
        pattern_hex: String,
        error: String,
    }

    fn vectors() -> Vectors {
        golden::load_json("refglob/refglob.json")
    }

    #[test]
    fn golden_match() {
        let v = vectors();
        assert!(!v.matches.is_empty());
        for c in &v.matches {
            assert_eq!(
                must(&c.pattern).matches(&c.name),
                c.want,
                "{:?}.Match({:?})",
                c.pattern,
                c.name
            );
        }
    }

    #[test]
    fn golden_prefix() {
        let v = vectors();
        assert!(!v.prefix.is_empty());
        for c in &v.prefix {
            let g = must(&c.pattern);
            assert_eq!(g.prefix(), c.prefix, "{:?}.Prefix()", c.pattern);
            assert_eq!(g.pattern(), c.pattern);
        }
    }

    #[test]
    fn golden_invalid() {
        let v = vectors();
        assert!(!v.invalid.is_empty());
        let mut non_utf8 = 0;
        for c in &v.invalid {
            let bytes = golden::hex(&c.pattern_hex);
            match compile_bytes(&bytes) {
                Ok(_) => panic!("compile_bytes({}) succeeded", c.pattern_hex),
                Err(e) => assert_eq!(e, c.error, "compile_bytes({})", c.pattern_hex),
            }
            match &c.pattern {
                Some(p) => {
                    assert_eq!(p.as_bytes(), bytes.as_slice(), "pattern and pattern_hex");
                    match compile(p) {
                        Ok(_) => panic!("compile({p:?}) succeeded"),
                        Err(e) => assert_eq!(e, c.error, "compile({p:?})"),
                    }
                }
                None => non_utf8 += 1,
            }
        }
        assert!(non_utf8 > 0, "the vectors carry non-UTF-8 patterns");
    }

    #[test]
    fn max_len_boundary() {
        let exact = "a".repeat(MAX_LEN);
        let g = must(&exact);
        assert!(g.matches(&exact));
        assert!(!g.matches(&exact[1..]));
        let over = "a".repeat(MAX_LEN + 1);
        assert_eq!(
            compile(&over).err().as_deref(),
            Some("refglob: pattern exceeds 4096 bytes")
        );
    }

    #[test]
    fn bad_range_quotes_runes_as_go() {
        let cases: &[(&str, &str)] = &[
            ("[z-a]", "refglob: bad range 'z'-'a'"),
            ("[ö-ä]", "refglob: bad range 'ö'-'ä'"),
            ("[\u{2028}-a]", "refglob: bad range '\\u2028'-'a'"),
            ("[\u{85}-a]", "refglob: bad range '\\u0085'-'a'"),
            ("[z-']", "refglob: bad range 'z'-'\\''"),
            ("[z-\"]", "refglob: bad range 'z'-'\"'"),
            (r"[z-\\]", "refglob: bad range 'z'-'\\\\'"),
            (
                "[\u{10FFFF}-\u{E000}]",
                "refglob: bad range '\\U0010ffff'-'\\ue000'",
            ),
        ];
        for &(pattern, want) in cases {
            assert_eq!(compile(pattern).err().as_deref(), Some(want), "{pattern:?}");
        }
    }

    #[test]
    fn classes_never_match_slash() {
        assert!(!must("[/]").matches("/"));
        assert!(!must("[.-0]").matches("/"));
        assert!(!must("[!a]").matches("/"));
        assert!(must("[!a]").matches("\n"));
        assert!(must("[ -\u{10FFFF}]").matches("\u{10FFFF}"));
        assert!(must("[!a]").matches("\u{10FFFF}"));
        assert!(!must("[ -\u{10FFFF}]").matches("/"));
    }

    #[test]
    fn star_forms() {
        // `**/` at a segment start matches any run of segments, including ones holding '\n'.
        let g = must("a/**/b");
        assert!(g.matches("a/b"));
        assert!(g.matches("a/\n/b"));
        assert!(g.matches("a/x/y/z/b"));
        assert!(!g.matches("a/xb"));
        // A final `**` is `.*`: no '\n'.
        assert!(!must("a/**").matches("a/x\n"));
        // `***` is never a double star.
        assert!(!must("***").matches("a/b"));
        assert!(must("***").matches("ab"));
    }
}
