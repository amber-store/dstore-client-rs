//! go-udiff v0.4.1 `diff.go` (`Edit`, `validate`, `SortEdits`, `lineEdits`, `expandEdit`) and
//! `ndiff.go` `Lines`.

use std::borrow::Cow;

use crate::gosort;
use crate::lcs;
use crate::unified::split_lines;

/// `udiff.Edit`: replace `content[start..end]` with `new`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub start: usize,
    pub end: usize,
    pub new: Vec<u8>,
}

/// `udiff.Lines`: the differences between two texts. All edits are at line boundaries.
pub fn lines(before: &[u8], after: &[u8]) -> Vec<Edit> {
    let (before_lines, b_offsets) = split_lines(before);
    let (after_lines, _) = split_lines(after);
    let diffs = lcs::diff_lines(&before_lines, &after_lines);

    // Convert from LCS diffs to Edits
    diffs
        .iter()
        .map(|d| Edit {
            start: b_offsets[d.start],
            end: b_offsets[d.end],
            new: after_lines[d.repl_start..d.repl_end].concat(),
        })
        .collect()
}

/// `validate` checks that edits are consistent with src, and returns the size of the patched output.
/// It returns a sorted copy when the edits are not sorted.
pub(crate) fn validate<'a>(
    src: &[u8],
    edits: &'a [Edit],
) -> Result<(Cow<'a, [Edit]>, isize), String> {
    let edits = if gosort::is_sorted(edits, edits_less) {
        Cow::Borrowed(edits)
    } else {
        let mut sorted = edits.to_vec();
        sort_edits(&mut sorted);
        Cow::Owned(sorted)
    };

    // Check validity of edits and compute final size.
    let mut size = src.len() as isize;
    let mut last_end = 0;
    for edit in edits.iter() {
        // Go: !(0 <= edit.Start && edit.Start <= edit.End && edit.End <= len(src))
        if edit.start > edit.end || edit.end > src.len() {
            return Err("diff has out-of-bounds edits".to_string());
        }
        if edit.start < last_end {
            return Err("diff has overlapping edits".to_string());
        }
        size += edit.new.len() as isize + edit.start as isize - edit.end as isize;
        last_end = edit.end;
    }

    Ok((edits, size))
}

/// `SortEdits` orders edits by (start, end) offset with `sort.Stable`: insertions (end = start) come
/// before deletions (end > start) at the same point, and multiple insertions at one point keep
/// their order.
pub(crate) fn sort_edits(edits: &mut [Edit]) {
    gosort::slice_stable(edits, edits_less);
}

/// `editsSort.Less`.
fn edits_less(a: &Edit, b: &Edit) -> bool {
    if a.start != b.start {
        return a.start < b.start;
    }
    a.end < b.end
}

/// `lineEdits` expands and merges a sequence of edits so that each resulting edit replaces one or
/// more complete lines.
pub(crate) fn line_edits<'a>(src: &[u8], edits: &'a [Edit]) -> Result<Cow<'a, [Edit]>, String> {
    let (edits, _) = validate(src, edits)?;

    // Do all deletions begin and end at the start of a line,
    // and all insertions end with a newline?
    // (This is merely a fast path.)
    let aligned = edits.iter().all(|edit| {
        !(edit.start >= src.len() // insertion at EOF
            || edit.start > 0 && src[edit.start - 1] != b'\n' // not at line start
            || edit.end > 0 && src[edit.end - 1] != b'\n' // not at line start
            || !edit.new.is_empty() && edit.new[edit.new.len() - 1] != b'\n') // partial insert
    });
    if aligned {
        return Ok(edits); // aligned
    }

    // expand:
    let Some((first, rest)) = edits.split_first() else {
        return Ok(edits); // no edits (unreachable due to fast path)
    };
    let mut expanded = Vec::with_capacity(edits.len()); // a guess
    let mut prev = first.clone();
    for edit in rest {
        let between = &src[prev.end..edit.start];
        if !between.contains(&b'\n') {
            // overlapping lines: combine with previous edit.
            prev.new.extend_from_slice(between);
            prev.new.extend_from_slice(&edit.new);
            prev.end = edit.end;
        } else {
            // non-overlapping lines: flush previous edit.
            expanded.push(expand_edit(prev, src));
            prev = edit.clone();
        }
    }
    expanded.push(expand_edit(prev, src)); // flush final edit
    Ok(Cow::Owned(expanded))
}

/// `expandEdit` returns edit expanded to complete whole lines.
fn expand_edit(mut edit: Edit, src: &[u8]) -> Edit {
    // Expand start left to start of line.
    // (delta is the zero-based column number of start.)
    let start = edit.start;
    let last_newline = match src[..start].iter().rposition(|&c| c == b'\n') {
        Some(i) => i as isize,
        None => -1,
    };
    let delta = start as isize - 1 - last_newline;
    if delta > 0 {
        let delta = delta as usize;
        edit.start -= delta;
        let mut new = src[start - delta..start].to_vec();
        new.extend_from_slice(&edit.new);
        edit.new = new;
    }

    // Expand end right to end of line.
    let end = edit.end;
    if end > 0 && src[end - 1] != b'\n'
        || !edit.new.is_empty() && edit.new[edit.new.len() - 1] != b'\n'
    {
        match src[end..].iter().position(|&c| c == b'\n') {
            None => edit.end = src.len(),        // extend to EOF
            Some(nl) => edit.end = end + nl + 1, // extend beyond \n
        }
    }
    edit.new.extend_from_slice(&src[end..edit.end]);

    edit
}

/// `udiff.Apply`: applies a sequence of edits to `src` (tests only; not part of the Unified path).
#[cfg(test)]
pub(crate) fn apply(src: &[u8], edits: &[Edit]) -> Result<Vec<u8>, String> {
    let (edits, size) = validate(src, edits)?;

    // Apply edits.
    let mut out = Vec::with_capacity(size.max(0) as usize);
    let mut last_end = 0;
    for edit in edits.iter() {
        if last_end < edit.start {
            out.extend_from_slice(&src[last_end..edit.start]);
        }
        out.extend_from_slice(&edit.new);
        last_end = edit.end;
    }
    out.extend_from_slice(&src[last_end..]);

    assert_eq!(out.len() as isize, size, "wrong size");
    Ok(out)
}

#[cfg(test)]
pub(crate) mod difftest {
    //! go-udiff v0.4.1 `difftest.TestCases`.

    use crate::Edit;

    pub(crate) const FILE_A: &[u8] = b"from";
    pub(crate) const FILE_B: &[u8] = b"to";
    const UNIFIED_PREFIX: &str = "--- from\n+++ to\n";

    pub(crate) struct TestCase {
        pub(crate) name: &'static str,
        pub(crate) input: &'static str,
        pub(crate) output: &'static str,
        pub(crate) unified: String,
        pub(crate) edits: Vec<Edit>,
        /// `None`: already line-aligned.
        pub(crate) line_edits: Option<Vec<Edit>>,
        pub(crate) no_diff: bool,
    }

    pub(crate) fn e(start: usize, end: usize, new: &str) -> Edit {
        Edit {
            start,
            end,
            new: new.as_bytes().to_vec(),
        }
    }

    fn case(
        name: &'static str,
        input: &'static str,
        output: &'static str,
        hunks: &str,
        edits: Vec<Edit>,
        line_edits: Option<Vec<Edit>>,
        no_diff: bool,
    ) -> TestCase {
        let unified = if hunks.is_empty() {
            String::new()
        } else {
            format!("{UNIFIED_PREFIX}{hunks}")
        };
        TestCase {
            name,
            input,
            output,
            unified,
            edits,
            line_edits,
            no_diff,
        }
    }

    pub(crate) fn test_cases() -> Vec<TestCase> {
        vec![
            case("empty", "", "", "", vec![], None, false),
            case(
                "no_diff",
                "gargantuan\n",
                "gargantuan\n",
                "",
                vec![],
                None,
                false,
            ),
            case(
                "replace_all",
                "fruit\n",
                "cheese\n",
                "@@ -1 +1 @@\n-fruit\n+cheese\n",
                vec![e(0, 5, "cheese")],
                Some(vec![e(0, 6, "cheese\n")]),
                false,
            ),
            case(
                "insert_rune",
                "gord\n",
                "gourd\n",
                "@@ -1 +1 @@\n-gord\n+gourd\n",
                vec![e(2, 2, "u")],
                Some(vec![e(0, 5, "gourd\n")]),
                false,
            ),
            case(
                "delete_rune",
                "groat\n",
                "goat\n",
                "@@ -1 +1 @@\n-groat\n+goat\n",
                vec![e(1, 2, "")],
                Some(vec![e(0, 6, "goat\n")]),
                false,
            ),
            case(
                "replace_rune",
                "loud\n",
                "lord\n",
                "@@ -1 +1 @@\n-loud\n+lord\n",
                vec![e(2, 3, "r")],
                Some(vec![e(0, 5, "lord\n")]),
                false,
            ),
            case(
                "replace_partials",
                "blanket\n",
                "bunker\n",
                "@@ -1 +1 @@\n-blanket\n+bunker\n",
                vec![e(1, 3, "u"), e(6, 7, "r")],
                Some(vec![e(0, 8, "bunker\n")]),
                false,
            ),
            case(
                "insert_line",
                "1: one\n3: three\n",
                "1: one\n2: two\n3: three\n",
                "@@ -1,2 +1,3 @@\n 1: one\n+2: two\n 3: three\n",
                vec![e(7, 7, "2: two\n")],
                None,
                false,
            ),
            case(
                "replace_no_newline",
                "A",
                "B",
                "@@ -1 +1 @@\n-A\n\\ No newline at end of file\n+B\n\\ No newline at end of file\n",
                vec![e(0, 1, "B")],
                None,
                false,
            ),
            case(
                "delete_empty",
                "meow",
                "",
                "@@ -1 +0,0 @@\n-meow\n\\ No newline at end of file\n",
                vec![e(0, 4, "")],
                Some(vec![e(0, 4, "")]),
                false,
            ),
            case(
                "append_empty",
                "",
                "AB\nC",
                "@@ -0,0 +1,2 @@\n+AB\n+C\n\\ No newline at end of file\n",
                vec![e(0, 0, "AB\nC")],
                Some(vec![e(0, 0, "AB\nC")]),
                false,
            ),
            case(
                "add_end",
                "A",
                "AB",
                "@@ -1 +1 @@\n-A\n\\ No newline at end of file\n+AB\n\\ No newline at end of file\n",
                vec![e(1, 1, "B")],
                Some(vec![e(0, 1, "AB")]),
                false,
            ),
            case(
                "add_empty",
                "",
                "AB\nC",
                "@@ -0,0 +1,2 @@\n+AB\n+C\n\\ No newline at end of file\n",
                vec![e(0, 0, "AB\nC")],
                Some(vec![e(0, 0, "AB\nC")]),
                false,
            ),
            case(
                "add_newline",
                "A",
                "A\n",
                "@@ -1 +1 @@\n-A\n\\ No newline at end of file\n+A\n",
                vec![e(1, 1, "\n")],
                Some(vec![e(0, 1, "A\n")]),
                false,
            ),
            case(
                "delete_front",
                "A\nB\nC\nA\nB\nB\nA\n",
                "C\nB\nA\nB\nA\nC\n",
                "@@ -1,7 +1,6 @@\n-A\n+C\nB\n-C\nA\n-B\nB\nA\n+C\n",
                vec![e(0, 2, "C\n"), e(4, 6, ""), e(8, 10, ""), e(14, 14, "C\n")],
                Some(vec![
                    e(0, 2, "C\n"),
                    e(4, 6, ""),
                    e(8, 10, ""),
                    e(14, 14, "C\n"),
                ]),
                true, // unified diff is different but valid
            ),
            case(
                "replace_last_line",
                "A\nB\n",
                "A\nC\n\n",
                "@@ -1,2 +1,3 @@\n A\n-B\n+C\n+\n",
                vec![e(2, 3, "C\n")],
                Some(vec![e(2, 4, "C\n\n")]),
                false,
            ),
            case(
                "multiple_replace",
                "A\nB\nC\nD\nE\nF\nG\n",
                "A\nH\nI\nJ\nE\nF\nK\n",
                "@@ -1,7 +1,7 @@\n A\n-B\n-C\n-D\n+H\n+I\n+J\n E\n F\n-G\n+K\n",
                vec![e(2, 8, "H\nI\nJ\n"), e(12, 14, "K\n")],
                None,
                true, // diff algorithm produces different delete/insert pattern
            ),
            case(
                "extra_newline",
                "\nA\n",
                "A\n",
                "@@ -1,2 +1 @@\n-\n A\n",
                vec![e(0, 1, "")],
                None,
                false,
            ),
            case(
                "unified_lines",
                "aaa\nccc\n",
                "aaa\nbbb\nccc\n",
                "@@ -1,2 +1,3 @@\n aaa\n+bbb\n ccc\n",
                vec![e(4, 4, "bbb\n")],
                Some(vec![e(4, 4, "bbb\n")]),
                false,
            ),
            case(
                "60379",
                "package a\n\ntype S struct {\ns fmt.Stringer\n}\n",
                "package a\n\ntype S struct {\n\ts fmt.Stringer\n}\n",
                "@@ -1,5 +1,5 @@\n package a\n \n type S struct {\n-s fmt.Stringer\n+\ts fmt.Stringer\n }\n",
                vec![e(27, 27, "\t")],
                Some(vec![e(27, 42, "\ts fmt.Stringer\n")]),
                false,
            ),
        ]
    }
}

#[cfg(test)]
mod tests {
    //! Ports of go-udiff v0.4.1 `diff_test.go`: `TestApply`, `TestLineEdits`, and `TestNEdits`,
    //! `TestNRandom`, `TestRegressionOld001`/`002` as round-trip properties of [`lines`] (go-udiff's
    //! `Strings` is not ported).

    use super::difftest::{e, test_cases};
    use super::*;
    use crate::gosort::testrng::Rng;

    fn lossy(b: &[u8]) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(b)
    }

    #[test]
    fn apply_edits() {
        for tc in test_cases() {
            let got = apply(tc.input.as_bytes(), &tc.edits)
                .unwrap_or_else(|err| panic!("{}: Apply(Edits) failed: {err}", tc.name));
            assert_eq!(lossy(&got), tc.output, "{}: Apply(Edits)", tc.name);
            if let Some(line_edits) = &tc.line_edits {
                let got = apply(tc.input.as_bytes(), line_edits)
                    .unwrap_or_else(|err| panic!("{}: Apply(LineEdits) failed: {err}", tc.name));
                assert_eq!(lossy(&got), tc.output, "{}: Apply(LineEdits)", tc.name);
            }
        }
    }

    #[test]
    fn line_edits_of_test_cases() {
        for tc in test_cases() {
            let want = tc.line_edits.as_ref().unwrap_or(&tc.edits); // nil: already line-aligned
            let got = line_edits(tc.input.as_bytes(), &tc.edits)
                .unwrap_or_else(|err| panic!("{}: LineEdits: {err}", tc.name));
            assert_eq!(
                &*got, want,
                "{}: in=<<{}>>\nout=<<{}>>\nraw  edits={:?}",
                tc.name, tc.input, tc.output, tc.edits
            );
            // make sure that applying the edits gives the expected result
            let fixed = apply(tc.input.as_bytes(), &got)
                .unwrap_or_else(|err| panic!("{}: Apply(LineEdits): {err}", tc.name));
            assert_eq!(lossy(&fixed), tc.output, "{}: Apply(LineEdits)", tc.name);
        }
    }

    #[test]
    fn lines_round_trip_test_cases() {
        for tc in test_cases() {
            let edits = lines(tc.input.as_bytes(), tc.output.as_bytes());
            let got = apply(tc.input.as_bytes(), &edits)
                .unwrap_or_else(|err| panic!("{}: Apply failed: {err}", tc.name));
            assert_eq!(lossy(&got), tc.output, "{}", tc.name);
        }
    }

    /// A random text of up to `n` lines drawn from `pieces`, sometimes without its final newline.
    pub(crate) fn random_text(rng: &mut Rng, pieces: &[&[u8]], n: usize) -> Vec<u8> {
        let mut text = Vec::new();
        for _ in 0..rng.intn(n + 1) {
            text.extend_from_slice(pieces[rng.intn(pieces.len())]);
        }
        if rng.intn(4) == 0 && text.last() == Some(&b'\n') {
            text.pop();
        }
        text
    }

    #[test]
    fn lines_round_trip_random() {
        let mut rng = Rng(1);
        let pieces: [&[u8]; 5] = [b"a\n", b"b\n", "ω\n".as_bytes(), b"c\n", b"\n"];
        for i in 0..1000 {
            let a = random_text(&mut rng, &pieces[..3], 16);
            let b = random_text(&mut rng, &pieces, 16);
            let edits = lines(&a, &b);
            let got = apply(&a, &edits).unwrap_or_else(|err| panic!("{i}: Apply failed: {err}"));
            assert!(
                got == b,
                "{i}: got {:?}, wanted {:?}, starting with {:?}",
                lossy(&got),
                lossy(&b),
                lossy(&a)
            );
        }
    }

    #[test]
    fn regression_old_001() {
        let a = "// Copyright 2019 The Go Authors. All rights reserved.\n// Use of this source code is governed by a BSD-style\n// license that can be found in the LICENSE file.\n\npackage udiff_test\n\nimport (\n\t\"fmt\"\n\t\"math/rand\"\n\t\"strings\"\n\t\"testing\"\n\n\t\"golang.org/x/tools/gopls/internal/lsp/diff\"\n\t\"github.com/aymanbagabas/go-udiff/difftest\"\n\t\"golang.org/x/tools/gopls/internal/span\"\n)\n";
        let b = "// Copyright 2019 The Go Authors. All rights reserved.\n// Use of this source code is governed by a BSD-style\n// license that can be found in the LICENSE file.\n\npackage udiff_test\n\nimport (\n\t\"fmt\"\n\t\"math/rand\"\n\t\"strings\"\n\t\"testing\"\n\n\t\"github.com/google/safehtml/template\"\n\t\"golang.org/x/tools/gopls/internal/lsp/diff\"\n\t\"github.com/aymanbagabas/go-udiff/difftest\"\n\t\"golang.org/x/tools/gopls/internal/span\"\n)\n";
        let diffs = lines(a.as_bytes(), b.as_bytes());
        let got = apply(a.as_bytes(), &diffs).unwrap_or_else(|err| panic!("Apply failed: {err}"));
        assert_eq!(lossy(&got), b, "oops {diffs:?}");
        // A single insertion of the new import line.
        let at = a
            .find("\t\"golang.org/x/tools/gopls/internal/lsp/diff\"")
            .unwrap_or(usize::MAX);
        assert_eq!(
            diffs,
            [e(at, at, "\t\"github.com/google/safehtml/template\"\n")]
        );
    }

    #[test]
    fn regression_old_002() {
        let a = "n\"\n)\n";
        let b = "n\"\n\t\"golang.org/x//nnal/stack\"\n)\n";
        let diffs = lines(a.as_bytes(), b.as_bytes());
        let got = apply(a.as_bytes(), &diffs).unwrap_or_else(|err| panic!("Apply failed: {err}"));
        assert_eq!(lossy(&got), b, "oops {diffs:?}");
    }

    #[test]
    fn validate_checks_and_sorts() {
        assert_eq!(
            validate(b"abc", &[e(0, 4, "")]).err().as_deref(),
            Some("diff has out-of-bounds edits")
        );
        assert_eq!(
            validate(b"abc", &[e(2, 1, "")]).err().as_deref(),
            Some("diff has out-of-bounds edits")
        );
        assert_eq!(
            validate(b"abcdef", &[e(0, 4, ""), e(2, 5, "")])
                .err()
                .as_deref(),
            Some("diff has overlapping edits")
        );
        // Unsorted edits are cloned and stably sorted by (start, end).
        let edits = [e(4, 5, "x"), e(2, 2, "y"), e(2, 3, ""), e(2, 2, "z")];
        let (sorted, size) = validate(b"abcdef", &edits).unwrap_or_else(|err| panic!("{err}"));
        assert!(matches!(sorted, Cow::Owned(_)));
        assert_eq!(
            &*sorted,
            [e(2, 2, "y"), e(2, 2, "z"), e(2, 3, ""), e(4, 5, "x")]
        );
        assert_eq!(size, 7);
        let (same, size) = validate(b"abcdef", &edits[1..2]).unwrap_or_else(|err| panic!("{err}"));
        assert!(matches!(same, Cow::Borrowed(_)));
        assert_eq!(size, 7);
    }

    #[test]
    fn line_edits_expand_and_merge() {
        // Edits on one line merge; the result covers whole lines.
        let edits = [e(1, 2, "X"), e(2, 3, "Y")];
        let got = line_edits(b"abc\ndef\n", &edits).unwrap_or_else(|err| panic!("{err}"));
        assert_eq!(&*got, [e(0, 4, "aXY\n")]);
        // Insertion at EOF takes the slow path without changing the edit.
        let edits = [e(4, 4, "c\n")];
        let got = line_edits(b"a\nb\n", &edits).unwrap_or_else(|err| panic!("{err}"));
        assert_eq!(&*got, [e(4, 4, "c\n")]);
        // A partial insertion at EOF of a text without a final newline extends to EOF.
        let edits = [e(3, 3, "c")];
        let got = line_edits(b"a\nb", &edits).unwrap_or_else(|err| panic!("{err}"));
        assert_eq!(&*got, [e(2, 3, "bc")]);
    }
}
