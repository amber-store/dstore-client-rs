//! go-udiff v0.4.1 `unified.go`: `Unified`, `ToUnified`, `toUnified`, `splitLines`,
//! `addEqualLines`, `unified.String`.

use crate::Edit;
use crate::diff::line_edits;

/// `DefaultContextLines`: the number of unchanged lines of surrounding context displayed by
/// [`unified`].
const DEFAULT_CONTEXT_LINES: usize = 3;

/// `udiff.Unified` (3 context lines): a unified diff of `old` and `new`, labelled with the names of
/// the old and new files. Equal inputs give an empty diff.
pub fn unified(old_label: &[u8], new_label: &[u8], old: &[u8], new: &[u8]) -> Vec<u8> {
    let edits = crate::lines(old, new);
    match to_unified(old_label, new_label, old, &edits, DEFAULT_CONTEXT_LINES) {
        Ok(unified) => unified,
        Err(err) => {
            // Can't happen: edits are consistent. Go: log.Fatalf("internal error in
            // diff.Unified: %v", err), which exits the process with status 1.
            eprintln!("internal error in diff.Unified: {err}");
            std::process::exit(1)
        }
    }
}

/// `udiff.ToUnified`: applies the edits to `content` and returns a unified diff with `context_lines`
/// lines of unchanged context around each hunk. Inconsistent edits are an error (`diff has
/// out-of-bounds edits`, `diff has overlapping edits`).
pub fn to_unified(
    old_label: &[u8],
    new_label: &[u8],
    content: &[u8],
    edits: &[Edit],
    context_lines: usize,
) -> Result<Vec<u8>, String> {
    if edits.is_empty() {
        return Ok(render(old_label, new_label, &[]));
    }
    let edits = line_edits(content, edits)?; // expand to whole lines
    let hunks = to_hunks(content, &edits, context_lines as isize);
    Ok(render(old_label, new_label, &hunks))
}

/// `hunk`: a contiguous set of line edits to apply.
struct Hunk<'a> {
    /// The line in the original source where the hunk starts.
    from_line: isize,
    /// The line in the original source where the hunk finishes.
    to_line: isize,
    /// The set of line based edits to apply.
    lines: Vec<Line<'a>>,
}

/// `line`: a single line operation to apply as part of a hunk.
struct Line<'a> {
    kind: OpKind,
    /// For deletion the line being removed, for all others the line to put in the output.
    content: &'a [u8],
}

/// `OpKind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OpKind {
    /// A line that is present in the input but not in the output.
    Delete,
    /// A line that is new in the output.
    Insert,
    /// A line that is the same in the input and output, often used as context.
    Equal,
}

/// `toUnified` after its `lineEdits` call: the hunks of `edits` (already expanded to whole lines).
/// Go's `h`, the hunk being built, is the last element of the result while `open` is set.
fn to_hunks<'a>(content: &'a [u8], edits: &'a [Edit], context_lines: isize) -> Vec<Hunk<'a>> {
    let gap = context_lines.wrapping_mul(2);
    let (lines, _) = split_lines(content);
    let mut hunks: Vec<Hunk<'a>> = Vec::new();
    let mut open = false;
    let mut last: isize = 0;
    let mut to_line: isize = 0;
    for edit in edits {
        // Compute the zero-based line numbers of the edit start and end.
        let start = count_newlines(&content[..edit.start]);
        let mut end = count_newlines(&content[..edit.end]);
        if edit.end == content.len() && !content.is_empty() && content[content.len() - 1] != b'\n' {
            end += 1; // EOF counts as an implicit newline
        }

        if open && start == last {
            // direct extension
        } else if open && start <= last.wrapping_add(gap) {
            // within range of previous lines, add the joiners
            if let Some(h) = hunks.last_mut() {
                add_equal_lines(h, &lines, last, start);
            }
        } else {
            // need to start a new hunk
            if open && let Some(h) = hunks.last_mut() {
                // add the edge to the previous hunk
                add_equal_lines(h, &lines, last, last.wrapping_add(context_lines));
            }
            to_line += start - last;
            let mut h = Hunk {
                from_line: start + 1,
                to_line: to_line + 1,
                lines: Vec::new(),
            };
            // add the edge to the new hunk
            let delta = add_equal_lines(&mut h, &lines, start.wrapping_sub(context_lines), start);
            h.from_line -= delta;
            h.to_line -= delta;
            hunks.push(h);
            open = true;
        }
        last = start;
        if let Some(h) = hunks.last_mut() {
            for i in start..end {
                h.lines.push(Line {
                    kind: OpKind::Delete,
                    content: lines[i as usize],
                });
                last += 1;
            }
            if !edit.new.is_empty() {
                let (v, _) = split_lines(&edit.new);
                for content in v {
                    h.lines.push(Line {
                        kind: OpKind::Insert,
                        content,
                    });
                    to_line += 1;
                }
            }
        }
    }
    if open && let Some(h) = hunks.last_mut() {
        // add the edge to the final hunk
        add_equal_lines(h, &lines, last, last.wrapping_add(context_lines));
    }
    hunks
}

/// `strings.Count(s, "\n")`.
fn count_newlines(s: &[u8]) -> isize {
    s.iter().filter(|&&c| c == b'\n').count() as isize
}

/// `splitLines`: splits after each `\n`, keeping the terminator and a final partial line, and
/// returns the offsets of the line beginnings (plus `len(text)` after a partial last line). Go
/// ranges over runes, but a `\n` byte is never part of another rune, so a bytewise split is the same.
pub(crate) fn split_lines(text: &[u8]) -> (Vec<&[u8]>, Vec<usize>) {
    let mut lines = Vec::new();
    let mut offsets = vec![0];
    let mut start = 0;
    for (i, &c) in text.iter().enumerate() {
        if c == b'\n' {
            lines.push(&text[start..=i]);
            start = i + 1;
            offsets.push(start);
        }
    }
    if start < text.len() {
        lines.push(&text[start..]);
        offsets.push(text.len());
    }
    (lines, offsets)
}

/// `addEqualLines`.
fn add_equal_lines<'a>(h: &mut Hunk<'a>, lines: &[&'a [u8]], start: isize, end: isize) -> isize {
    let mut delta = 0;
    let mut i = start;
    while i < end {
        if i < 0 {
            i += 1;
            continue;
        }
        if i >= lines.len() as isize {
            return delta;
        }
        h.lines.push(Line {
            kind: OpKind::Equal,
            content: lines[i as usize],
        });
        delta += 1;
        i += 1;
    }
    delta
}

/// `unified.String`: the standard textual form of the diff.
fn render(from: &[u8], to: &[u8], hunks: &[Hunk<'_>]) -> Vec<u8> {
    if hunks.is_empty() {
        return Vec::new();
    }
    let mut b = Vec::new();
    b.extend_from_slice(b"--- ");
    b.extend_from_slice(from);
    b.extend_from_slice(b"\n+++ ");
    b.extend_from_slice(to);
    b.push(b'\n');
    for hunk in hunks {
        let (mut from_count, mut to_count) = (0, 0);
        for l in &hunk.lines {
            match l.kind {
                OpKind::Delete => from_count += 1,
                OpKind::Insert => to_count += 1,
                OpKind::Equal => {
                    from_count += 1;
                    to_count += 1;
                }
            }
        }
        b.extend_from_slice(b"@@");
        if from_count > 1 {
            b.extend_from_slice(format!(" -{},{}", hunk.from_line, from_count).as_bytes());
        } else if hunk.from_line == 1 && from_count == 0 {
            // Match odd GNU diff -u behavior adding to empty file.
            b.extend_from_slice(b" -0,0");
        } else {
            b.extend_from_slice(format!(" -{}", hunk.from_line).as_bytes());
        }
        if to_count > 1 {
            b.extend_from_slice(format!(" +{},{}", hunk.to_line, to_count).as_bytes());
        } else if hunk.to_line == 1 && to_count == 0 {
            // Match odd GNU diff -u behavior adding to empty file.
            b.extend_from_slice(b" +0,0");
        } else {
            b.extend_from_slice(format!(" +{}", hunk.to_line).as_bytes());
        }
        b.extend_from_slice(b" @@\n");
        for l in &hunk.lines {
            b.push(match l.kind {
                OpKind::Delete => b'-',
                OpKind::Insert => b'+',
                OpKind::Equal => b' ',
            });
            b.extend_from_slice(l.content);
            if !l.content.ends_with(b"\n") {
                b.extend_from_slice(b"\n\\ No newline at end of file\n");
            }
        }
    }
    b
}

#[cfg(test)]
mod tests {
    //! Ports of go-udiff v0.4.1 `difftest.DiffTest` over `Lines` with `TestVerifyUnified`'s expected
    //! texts, and `TestToUnified` with an in-process patch instead of `patch -p0 -u`. The exact
    //! bytes over thousands of inputs are locked by `udiff/udiff.json`.

    use super::*;
    use crate::diff::apply;
    use crate::diff::difftest::{FILE_A, FILE_B, test_cases};
    use crate::gosort::testrng::Rng;
    use crate::lines;

    fn lossy(b: &[u8]) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(b)
    }

    #[test]
    fn split_lines_keeps_terminators_and_partial_last_line() {
        let empty: Vec<&[u8]> = Vec::new();
        assert_eq!(split_lines(b""), (empty, vec![0]));
        assert_eq!(split_lines(b"a"), (vec![&b"a"[..]], vec![0, 1]));
        assert_eq!(split_lines(b"a\n"), (vec![&b"a\n"[..]], vec![0, 2]));
        assert_eq!(
            split_lines(b"a\nbc"),
            (vec![&b"a\n"[..], &b"bc"[..]], vec![0, 2, 4])
        );
        assert_eq!(
            split_lines(b"\n\n"),
            (vec![&b"\n"[..], &b"\n"[..]], vec![0, 1, 2])
        );
        assert_eq!(
            split_lines(b"\xe2\n\x82"),
            (vec![&b"\xe2\n"[..], &b"\x82"[..]], vec![0, 2, 3])
        );
    }

    #[test]
    fn diff_test_over_lines() {
        for tc in test_cases() {
            let edits = lines(tc.input.as_bytes(), tc.output.as_bytes());
            let got = apply(tc.input.as_bytes(), &edits)
                .unwrap_or_else(|err| panic!("{}: Apply failed: {err}", tc.name));
            let u = to_unified(
                FILE_A,
                FILE_B,
                tc.input.as_bytes(),
                &edits,
                DEFAULT_CONTEXT_LINES,
            )
            .unwrap_or_else(|err| panic!("{}: ToUnified: {err}", tc.name));
            assert_eq!(
                lossy(&got),
                tc.output,
                "{}: Apply: from diff:\n{}",
                tc.name,
                lossy(&u)
            );
            if !tc.no_diff {
                assert_eq!(
                    lossy(&u),
                    tc.unified,
                    "{}: Unified: diffs:{edits:?}",
                    tc.name
                );
            }
            assert_eq!(
                unified(FILE_A, FILE_B, tc.input.as_bytes(), tc.output.as_bytes()),
                u
            );
        }
    }

    /// Parses `@@ -F[,N] +T[,M] @@\n`; a missing count is 1.
    fn parse_hunk_header(h: &[u8]) -> Result<(usize, usize, usize, usize), String> {
        let text = std::str::from_utf8(h).map_err(|err| err.to_string())?;
        let rest = text
            .strip_prefix("@@ -")
            .and_then(|r| r.strip_suffix(" @@\n"))
            .ok_or_else(|| format!("bad hunk header {text:?}"))?;
        let (from, to) = rest
            .split_once(" +")
            .ok_or_else(|| format!("bad hunk header {text:?}"))?;
        let range = |s: &str| -> Result<(usize, usize), String> {
            let num = |n: &str| n.parse::<usize>().map_err(|err| format!("{text:?}: {err}"));
            match s.split_once(',') {
                Some((start, count)) => Ok((num(start)?, num(count)?)),
                None => Ok((num(s)?, 1)),
            }
        };
        let (from_start, from_count) = range(from)?;
        let (to_start, to_count) = range(to)?;
        Ok((from_start, from_count, to_start, to_count))
    }

    /// Applies a unified diff to `old`, checking every context and deleted line and the hunk counts
    /// (the role `patch -p0 -u` plays in go-udiff's `TestToUnified`).
    fn apply_unified(old: &[u8], diff: &[u8]) -> Result<Vec<u8>, String> {
        if diff.is_empty() {
            return Ok(old.to_vec());
        }
        let (old_lines, _) = split_lines(old);
        let (dl, _) = split_lines(diff);
        if dl.len() < 2 || !dl[0].starts_with(b"--- ") || !dl[1].starts_with(b"+++ ") {
            return Err("missing --- and +++ header".to_string());
        }
        let mut out = Vec::new();
        let mut pos = 0;
        let mut i = 2;
        while i < dl.len() {
            let (from_start, from_count, _, to_count) = parse_hunk_header(dl[i])?;
            // With a zero count the hunk inserts after line F.
            let first = if from_count == 0 {
                from_start
            } else {
                from_start - 1
            };
            if first < pos || first > old_lines.len() {
                return Err(format!("hunk {:?} out of range", lossy(dl[i])));
            }
            for l in &old_lines[pos..first] {
                out.extend_from_slice(l);
            }
            pos = first;
            let header = i;
            i += 1;
            let (mut from_seen, mut to_seen) = (0, 0);
            while i < dl.len() && !dl[i].starts_with(b"@@ ") {
                let l = dl[i];
                let mut content = &l[1..];
                if dl.get(i + 1) == Some(&&b"\\ No newline at end of file\n"[..]) {
                    content = &content[..content.len() - 1];
                    i += 1;
                }
                match l[0] {
                    b' ' | b'-' => {
                        if old_lines.get(pos) != Some(&content) {
                            return Err(format!(
                                "line {pos}: diff has {:?}, old has {:?}",
                                lossy(content),
                                old_lines.get(pos).map(|l| lossy(l))
                            ));
                        }
                        pos += 1;
                        from_seen += 1;
                        if l[0] == b' ' {
                            out.extend_from_slice(content);
                            to_seen += 1;
                        }
                    }
                    b'+' => {
                        out.extend_from_slice(content);
                        to_seen += 1;
                    }
                    _ => return Err(format!("impossible line {:?}", lossy(l))),
                }
                i += 1;
            }
            if (from_seen, to_seen) != (from_count, to_count) {
                return Err(format!(
                    "hunk {:?} has -{from_seen} +{to_seen} lines",
                    lossy(dl[header])
                ));
            }
        }
        for l in &old_lines[pos..] {
            out.extend_from_slice(l);
        }
        Ok(out)
    }

    #[test]
    fn to_unified_patches_back() {
        for tc in test_cases() {
            let nedits = lines(tc.input.as_bytes(), tc.output.as_bytes());
            let xunified = to_unified(
                FILE_A,
                FILE_B,
                tc.input.as_bytes(),
                &nedits,
                DEFAULT_CONTEXT_LINES,
            )
            .unwrap_or_else(|err| panic!("{}: {err}", tc.name));
            if xunified.is_empty() {
                continue;
            }
            let got = apply_unified(tc.input.as_bytes(), &xunified)
                .unwrap_or_else(|err| panic!("{}: patch: {err}\n{}", tc.name, lossy(&xunified)));
            assert_eq!(
                lossy(&got),
                tc.output,
                "{}: applying unified failed: unified\n{}",
                tc.name,
                lossy(&xunified)
            );
        }
    }

    /// A random text of up to `n` pieces, sometimes without its final newline.
    fn random_text(rng: &mut Rng, pieces: &[&[u8]], n: usize) -> Vec<u8> {
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
    fn random_texts_patch_back() {
        let mut rng = Rng(42);
        let pieces: [&[u8]; 6] = [b"a\n", b"b\n", b"c\n", b"\n", b"d\r\n", b"e"];
        for i in 0..2000 {
            let old = random_text(&mut rng, &pieces, 40);
            let new = random_text(&mut rng, &pieces, 40);
            let u = unified(b"a", b"b", &old, &new);
            assert_eq!(u.is_empty(), old == new, "{i}");
            let got = apply_unified(&old, &u)
                .unwrap_or_else(|err| panic!("{i}: patch: {err}\n{}", lossy(&u)));
            assert!(
                got == new,
                "{i}: patched {:?}, want {:?}\n{}",
                lossy(&got),
                lossy(&new),
                lossy(&u)
            );
        }
    }

    #[test]
    fn samples() {
        // port-notes/worktree.md §3.7 and verification.md §3.4.
        assert_eq!(
            lossy(&unified(b"a/x", b"b/x", b"a", b"a\nb")),
            "--- a/x\n+++ b/x\n@@ -1 +1,2 @@\n-a\n\\ No newline at end of file\n+a\n+b\n\\ No newline at end of file\n"
        );
        assert_eq!(
            lossy(&unified(b"/dev/null", b"b/x", b"", b"hi\n")),
            "--- /dev/null\n+++ b/x\n@@ -0,0 +1 @@\n+hi\n"
        );
        assert_eq!(
            lossy(&unified(b"a/x", b"/dev/null", b"bye\n", b"")),
            "--- a/x\n+++ /dev/null\n@@ -1 +0,0 @@\n-bye\n"
        );
        assert_eq!(
            lossy(&unified(
                b"a/edit.txt",
                b"b/edit.txt",
                b"one\ntwo\nthree\n",
                b"one\n2\nthree\n"
            )),
            "--- a/edit.txt\n+++ b/edit.txt\n@@ -1,3 +1,3 @@\n one\n-two\n+2\n three\n"
        );
        assert_eq!(
            lossy(&unified(b"a/link", b"b/link", b"t1", b"t2")),
            "--- a/link\n+++ b/link\n@@ -1 +1 @@\n-t1\n\\ No newline at end of file\n+t2\n\\ No newline at end of file\n"
        );
        assert!(unified(b"a", b"b", b"same\n", b"same\n").is_empty());
        assert!(unified(b"a", b"b", b"", b"").is_empty());
    }

    #[test]
    fn context_lines_and_errors() {
        let content = b"1\n2\n3\n4\n5\n6\n7\n8\n9\n";
        let edits = [Edit {
            start: 8,
            end: 10,
            new: b"X\n".to_vec(),
        }];
        assert_eq!(
            lossy(
                &to_unified(b"a", b"b", content, &edits, 0).unwrap_or_else(|err| panic!("{err}"))
            ),
            "--- a\n+++ b\n@@ -5 +5 @@\n-5\n+X\n"
        );
        assert_eq!(
            lossy(
                &to_unified(b"a", b"b", content, &edits, 1).unwrap_or_else(|err| panic!("{err}"))
            ),
            "--- a\n+++ b\n@@ -4,3 +4,3 @@\n 4\n-5\n+X\n 6\n"
        );
        assert_eq!(
            to_unified(b"a", b"b", content, &[], 3).unwrap_or_else(|err| panic!("{err}")),
            b""
        );
        let bad = [Edit {
            start: 3,
            end: 20,
            new: Vec::new(),
        }];
        assert_eq!(
            to_unified(b"a", b"b", content, &bad, 3),
            Err("diff has out-of-bounds edits".to_string())
        );
    }
}
