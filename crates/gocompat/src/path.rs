//! `path` and `path/filepath` on Unix, over Go string bytes (go1.26.5 `internal/filepathlite/path.go`,
//! `path/filepath/path.go`, `path/filepath/path_unix.go`, `path/path.go`).

use std::os::unix::ffi::OsStrExt;

const SEP: u8 = b'/';

/// `filepath.Clean`.
pub fn clean(p: &[u8]) -> Vec<u8> {
    if p.is_empty() {
        return b".".to_vec();
    }
    let rooted = p.first() == Some(&SEP);
    let n = p.len();
    let mut out: Vec<u8> = Vec::with_capacity(n);
    // r: next byte of p to read; dotdot: where ".." must stop backtracking.
    let (mut r, mut dotdot) = (0usize, 0usize);
    if rooted {
        out.push(SEP);
        r = 1;
        dotdot = 1;
    }
    let at = |i: usize| p.get(i).copied();
    while r < n {
        let c = at(r);
        if c == Some(SEP) {
            // Empty path element.
            r += 1;
        } else if c == Some(b'.') && (r + 1 == n || at(r + 1) == Some(SEP)) {
            // "." element.
            r += 1;
        } else if c == Some(b'.')
            && at(r + 1) == Some(b'.')
            && (r + 2 == n || at(r + 2) == Some(SEP))
        {
            // ".." element: remove to the last separator.
            r += 2;
            if out.len() > dotdot {
                let mut w = out.len() - 1;
                while w > dotdot && out.get(w) != Some(&SEP) {
                    w -= 1;
                }
                out.truncate(w);
            } else if !rooted {
                // Cannot backtrack, but not rooted, so append a ".." element.
                if !out.is_empty() {
                    out.push(SEP);
                }
                out.extend_from_slice(b"..");
                dotdot = out.len();
            }
        } else {
            // A real path element; add a slash if needed.
            if (rooted && out.len() != 1) || (!rooted && !out.is_empty()) {
                out.push(SEP);
            }
            while let Some(b) = at(r).filter(|&b| b != SEP) {
                out.push(b);
                r += 1;
            }
        }
    }
    if out.is_empty() {
        out.push(b'.');
    }
    out
}

/// `filepath.Join`: the non-empty tail from the first non-empty element, joined with "/" and cleaned;
/// "" when every element is empty.
pub fn join(elems: &[&[u8]]) -> Vec<u8> {
    match elems.iter().position(|e| !e.is_empty()) {
        Some(i) => clean(&elems[i..].join(&SEP)),
        None => Vec::new(),
    }
}

/// `filepath.Dir`.
pub fn dir(p: &[u8]) -> Vec<u8> {
    let end = p.iter().rposition(|&b| b == SEP).map_or(0, |i| i + 1);
    clean(p.get(..end).unwrap_or_default())
}

/// `path.Base`: "" → ".", all slashes → "/".
pub fn base(p: &[u8]) -> Vec<u8> {
    if p.is_empty() {
        return b".".to_vec();
    }
    let trimmed_len = p.iter().rposition(|&b| b != SEP).map_or(0, |i| i + 1);
    let trimmed = p.get(..trimmed_len).unwrap_or_default();
    let last = match trimmed.iter().rposition(|&b| b == SEP) {
        Some(i) => trimmed.get(i + 1..).unwrap_or_default(),
        None => trimmed,
    };
    if last.is_empty() {
        return b"/".to_vec();
    }
    last.to_vec()
}

/// `filepath.Abs`: `Clean(p)` when absolute, else `Join(os.Getwd(), p)`.
pub fn abs(p: &[u8]) -> std::io::Result<Vec<u8>> {
    if p.first() == Some(&SEP) {
        return Ok(clean(p));
    }
    let wd = crate::os::getwd()?;
    Ok(join(&[wd.as_slice(), p]))
}

/// `filepath.Rel`, lexical; None where Go returns an error (`Rel: can't make <targ> relative to <base>`).
pub fn rel(base: &[u8], target: &[u8]) -> Option<Vec<u8>> {
    let mut base = clean(base);
    let targ = clean(target);
    if targ == base {
        return Some(b".".to_vec());
    }
    if base == b"." {
        base.clear();
    }
    let base_slashed = base.first() == Some(&SEP);
    let targ_slashed = targ.first() == Some(&SEP);
    if base_slashed != targ_slashed {
        return None;
    }
    // Position base[b0..bi] and targ[t0..ti] at the first differing elements.
    let (bl, tl) = (base.len(), targ.len());
    let (mut b0, mut bi, mut t0, mut ti) = (0usize, 0usize, 0usize, 0usize);
    loop {
        while bi < bl && base.get(bi) != Some(&SEP) {
            bi += 1;
        }
        while ti < tl && targ.get(ti) != Some(&SEP) {
            ti += 1;
        }
        if targ.get(t0..ti) != base.get(b0..bi) {
            break;
        }
        if bi < bl {
            bi += 1;
        }
        if ti < tl {
            ti += 1;
        }
        b0 = bi;
        t0 = ti;
    }
    if base.get(b0..bi) == Some(&b".."[..]) {
        return None;
    }
    if b0 != bl {
        // Base elements left: go up before going down.
        let seps = base
            .get(b0..bl)
            .unwrap_or_default()
            .iter()
            .filter(|&&b| b == SEP)
            .count();
        let mut buf = Vec::with_capacity(2 + seps * 3 + 1 + tl.saturating_sub(t0));
        buf.extend_from_slice(b"..");
        for _ in 0..seps {
            buf.extend_from_slice(b"/..");
        }
        if t0 != tl {
            buf.push(SEP);
            buf.extend_from_slice(targ.get(t0..).unwrap_or_default());
        }
        return Some(clean(&buf));
    }
    Some(targ.get(t0..).unwrap_or_default().to_vec())
}

/// Bytes → `PathBuf` (Unix, no conversion).
pub fn to_path(b: &[u8]) -> std::path::PathBuf {
    std::path::PathBuf::from(std::ffi::OsStr::from_bytes(b))
}

/// `Path` → bytes (Unix, no conversion).
pub fn from_path(p: &std::path::Path) -> Vec<u8> {
    p.as_os_str().as_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(b: Vec<u8>) -> String {
        String::from_utf8_lossy(&b).into_owned()
    }

    // path/filepath path_test.go cleantests + nonwincleantests.
    #[test]
    fn clean_table() {
        let cases = [
            ("abc", "abc"),
            ("abc/def", "abc/def"),
            ("a/b/c", "a/b/c"),
            (".", "."),
            ("..", ".."),
            ("../..", "../.."),
            ("../../abc", "../../abc"),
            ("/abc", "/abc"),
            ("/", "/"),
            ("", "."),
            ("abc/", "abc"),
            ("abc/def/", "abc/def"),
            ("a/b/c/", "a/b/c"),
            ("./", "."),
            ("../", ".."),
            ("../../", "../.."),
            ("/abc/", "/abc"),
            ("abc//def//ghi", "abc/def/ghi"),
            ("abc//", "abc"),
            ("abc/./def", "abc/def"),
            ("/./abc/def", "/abc/def"),
            ("abc/.", "abc"),
            ("abc/def/ghi/../jkl", "abc/def/jkl"),
            ("abc/def/../ghi/../jkl", "abc/jkl"),
            ("abc/def/..", "abc"),
            ("abc/def/../..", "."),
            ("/abc/def/../..", "/"),
            ("abc/def/../../..", ".."),
            ("/abc/def/../../..", "/"),
            ("abc/def/../../../ghi/jkl/../../../mno", "../../mno"),
            ("/../abc", "/abc"),
            ("a/../b:/../../c", "../c"),
            ("abc/./../def", "def"),
            ("abc//./../def", "def"),
            ("abc/../../././../def", "../../def"),
            ("//abc", "/abc"),
            ("///abc", "/abc"),
            ("//abc//", "/abc"),
        ];
        for (input, want) in cases {
            assert_eq!(s(clean(input.as_bytes())), want, "Clean({input:?})");
            assert_eq!(
                s(clean(want.as_bytes())),
                want,
                "Clean({want:?}) is idempotent"
            );
        }
    }

    // jointests + nonwinjointests.
    #[test]
    fn join_table() {
        let cases: &[(&[&str], &str)] = &[
            (&[], ""),
            (&[""], ""),
            (&["/"], "/"),
            (&["a"], "a"),
            (&["a", "b"], "a/b"),
            (&["a", ""], "a"),
            (&["", "b"], "b"),
            (&["/", "a"], "/a"),
            (&["/", "a/b"], "/a/b"),
            (&["/", ""], "/"),
            (&["/a", "b"], "/a/b"),
            (&["a", "/b"], "a/b"),
            (&["/a", "/b"], "/a/b"),
            (&["a/", "b"], "a/b"),
            (&["a/", ""], "a"),
            (&["", ""], ""),
            (&["/", "a", "b"], "/a/b"),
            (&["//", "a"], "/a"),
        ];
        for (elems, want) in cases {
            let bytes: Vec<&[u8]> = elems.iter().map(|e| e.as_bytes()).collect();
            assert_eq!(s(join(&bytes)), *want, "Join({elems:?})");
        }
    }

    // basetests (path and filepath agree on Unix).
    #[test]
    fn base_table() {
        let cases = [
            ("", "."),
            (".", "."),
            ("/.", "."),
            ("/", "/"),
            ("////", "/"),
            ("x/", "x"),
            ("abc", "abc"),
            ("abc/def", "def"),
            ("a/b/.x", ".x"),
            ("a/b/c.", "c."),
            ("a/b/c.x", "c.x"),
            ("a/b//", "b"),
            ("a/../", ".."),
        ];
        for (input, want) in cases {
            assert_eq!(s(base(input.as_bytes())), want, "Base({input:?})");
        }
    }

    // dirtests + nonwindirtests.
    #[test]
    fn dir_table() {
        let cases = [
            ("", "."),
            (".", "."),
            ("/.", "/"),
            ("/", "/"),
            ("/foo", "/"),
            ("x/", "x"),
            ("abc", "."),
            ("abc/def", "abc"),
            ("a/b/.x", "a/b"),
            ("a/b/c.", "a/b"),
            ("a/b/c.x", "a/b"),
            ("////", "/"),
        ];
        for (input, want) in cases {
            assert_eq!(s(dir(input.as_bytes())), want, "Dir({input:?})");
        }
    }

    // reltests.
    #[test]
    fn rel_table() {
        let cases = [
            ("a/b", "a/b", "."),
            ("a/b/.", "a/b", "."),
            ("a/b", "a/b/.", "."),
            ("./a/b", "a/b", "."),
            ("a/b", "./a/b", "."),
            ("ab/cd", "ab/cde", "../cde"),
            ("ab/cd", "ab/c", "../c"),
            ("a/b", "a/b/c/d", "c/d"),
            ("a/b", "a/b/../c", "../c"),
            ("a/b/../c", "a/b", "../b"),
            ("a/b/c", "a/c/d", "../../c/d"),
            ("a/b", "c/d", "../../c/d"),
            ("a/b/c/d", "a/b", "../.."),
            ("a/b/c/d", "a/b/", "../.."),
            ("a/b/c/d/", "a/b", "../.."),
            ("a/b/c/d/", "a/b/", "../.."),
            ("../../a/b", "../../a/b/c/d", "c/d"),
            ("/a/b", "/a/b", "."),
            ("/a/b/.", "/a/b", "."),
            ("/a/b", "/a/b/.", "."),
            ("/ab/cd", "/ab/cde", "../cde"),
            ("/ab/cd", "/ab/c", "../c"),
            ("/a/b", "/a/b/c/d", "c/d"),
            ("/a/b", "/a/b/../c", "../c"),
            ("/a/b/../c", "/a/b", "../b"),
            ("/a/b/c", "/a/c/d", "../../c/d"),
            ("/a/b", "/c/d", "../../c/d"),
            ("/a/b/c/d", "/a/b", "../.."),
            ("/a/b/c/d", "/a/b/", "../.."),
            ("/a/b/c/d/", "/a/b", "../.."),
            ("/a/b/c/d/", "/a/b/", "../.."),
            ("/../../a/b", "/../../a/b/c/d", "c/d"),
            (".", "a/b", "a/b"),
            (".", "..", ".."),
            ("", "../../.", "../.."),
            ("..", ".", "err"),
            ("..", "a", "err"),
            ("../..", "..", "err"),
            ("a", "/a", "err"),
            ("/a", "a", "err"),
        ];
        for (b, t, want) in cases {
            let got = rel(b.as_bytes(), t.as_bytes()).map_or_else(|| "err".to_string(), s);
            assert_eq!(got, want, "Rel({b:?}, {t:?})");
        }
    }

    #[test]
    fn path_conversions_keep_bytes() {
        let raw = b"a/\xff\x01b";
        assert_eq!(from_path(&to_path(raw)), raw);
    }

    #[test]
    fn abs_of_absolute_is_clean() {
        assert_eq!(abs(b"/a/./b/../c/").ok(), Some(b"/a/c".to_vec()));
    }

    // filepath.Abs of a relative path: Join(os.Getwd(), p); "" gives the working directory.
    #[test]
    fn abs_of_relative_joins_getwd() {
        let wd = match crate::os::getwd() {
            Ok(wd) => wd,
            Err(e) => panic!("getwd: {e}"),
        };
        assert_eq!(abs(b"x/../y/").ok(), Some(join(&[wd.as_slice(), b"y"])));
        assert_eq!(abs(b"").ok(), Some(clean(&wd)));
    }
}
