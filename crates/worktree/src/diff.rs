//! `worktree/diff.go`: unified diffs and `--stat` (blocking; writes as it goes, partial output may
//! precede an error).

use std::os::unix::fs::MetadataExt;

use amber_store_core::fstree::{self, Entry};
use amber_store_core::key::Key;
use dstore_gocompat::path;

use crate::error::{io_error, lossy, wrap};
use crate::sys::{self, S_IFLNK, S_IFMT, S_IFREG};
use crate::{Change, Error, Getter, Kind, MAX_DIFF_BYTES, is_dir};

/// Why content is unavailable.
#[derive(Debug)]
pub enum SourceError {
    TooLarge,
    Other(Error),
}

/// Content of one side of a diff. Data may come with an error.
pub trait Source {
    fn content(&self, path: &[u8], e: &Entry) -> (Option<Vec<u8>>, Option<SourceError>);
}

/// Content from the store.
pub struct TreeSource<'a> {
    pub get: Getter<'a>,
}

/// Content from the working directory.
pub struct DiskSource {
    pub root: Vec<u8>,
}

/// `TreeSource.Content`: a link's target; a file's bytes (`ErrTooLarge` over `MAX_DIFF_BYTES` by the key's
/// length); nothing for other types.
impl Source for TreeSource<'_> {
    fn content(&self, _path: &[u8], e: &Entry) -> (Option<Vec<u8>>, Option<SourceError>) {
        match e.mode & S_IFMT {
            S_IFLNK => (Some(e.link_target.clone()), None),
            S_IFREG => {
                let ck = match Key::parse(&e.content_key) {
                    Ok(k) => k,
                    Err(err) => return (None, Some(SourceError::Other(Error::Key(err)))),
                };
                if ck.length() > MAX_DIFF_BYTES {
                    return (None, Some(SourceError::TooLarge));
                }
                let mut buf = Vec::new();
                match fstree::write_content(&mut buf, ck, |k| (self.get)(k)) {
                    Ok(()) => (Some(buf), None),
                    Err(err) => (None, Some(SourceError::Other(Error::Walk(err)))),
                }
            }
            _ => (None, None),
        }
    }
}

/// `DiskSource.Content`: `Join(root, path)`, typed by the change entry. A failed readlink comes with an
/// empty target; a failed read keeps the bytes read before the error, as `os.ReadFile` does.
impl Source for DiskSource {
    fn content(&self, path: &[u8], e: &Entry) -> (Option<Vec<u8>>, Option<SourceError>) {
        let full = path::join(&[&self.root, path]);
        match e.mode & S_IFMT {
            S_IFLNK => match sys::readlink(&full) {
                Ok(t) => (Some(t), None),
                Err(err) => (Some(Vec::new()), Some(SourceError::Other(Error::Path(err)))),
            },
            S_IFREG => {
                let md = match sys::os_lstat(&full) {
                    Ok(md) => md,
                    Err(err) => return (None, Some(SourceError::Other(Error::Path(err)))),
                };
                if md.size() > MAX_DIFF_BYTES {
                    return (None, Some(SourceError::TooLarge));
                }
                let (data, err) = sys::read_file_partial(&full);
                (data, err.map(|pe| SourceError::Other(Error::Path(pe))))
            }
            _ => (None, None),
        }
    }
}

/// `sides`: the content-bearing entries of a change (directories and absent sides are `None`).
fn sides(c: &Change) -> (Option<&Entry>, Option<&Entry>) {
    fn side(e: Option<&Entry>) -> Option<&Entry> {
        e.filter(|e| !is_dir(Some(e)))
    }
    (side(c.old.as_deref()), side(c.new.as_deref()))
}

/// `modeLines`: "old mode %04o\nnew mode %04o\n" when both sides exist and the permission bits differ.
fn mode_lines(c: &Change) -> Vec<u8> {
    match (c.old.as_deref(), c.new.as_deref()) {
        (Some(o), Some(n)) if o.mode & 0o7777 != n.mode & 0o7777 => format!(
            "old mode {:04o}\nnew mode {:04o}\n",
            o.mode & 0o7777,
            n.mode & 0o7777
        )
        .into_bytes(),
        _ => Vec::new(),
    }
}

fn diff_header(c: &Change) -> Vec<u8> {
    let mut h = Vec::with_capacity(2 * c.path.len() + 12);
    h.extend_from_slice(b"diff a/");
    h.extend_from_slice(&c.path);
    h.extend_from_slice(b" b/");
    h.extend_from_slice(&c.path);
    h.push(b'\n');
    h
}

/// A `fmt.Fprintf` write: errors are ignored, as Go discards them.
fn put(w: &mut dyn std::io::Write, b: &[u8]) {
    let _ = w.write_all(b);
}

/// `content`: nothing for an absent side.
fn content(s: &dyn Source, p: &[u8], e: Option<&Entry>) -> (Option<Vec<u8>>, Option<SourceError>) {
    match e {
        None => (None, None),
        Some(e) => s.content(p, e),
    }
}

/// `errors.Is(err, ErrTooLarge)`.
fn is_too_large(e: &Option<SourceError>) -> bool {
    matches!(
        e,
        Some(SourceError::TooLarge) | Some(SourceError::Other(Error::TooLarge))
    )
}

/// git's heuristic: a NUL byte in the first 8 KiB.
fn is_binary(b: &Option<Vec<u8>>) -> bool {
    let b = b.as_deref().unwrap_or_default();
    b.get(..b.len().min(8192))
        .is_some_and(|head| head.contains(&0))
}

/// The two sides' contents after the binary check: `Ok(None)` when the change is summarised as binary.
enum Contents {
    Binary,
    Text(Vec<u8>, Vec<u8>),
}

fn read_sides(
    c: &Change,
    old_e: Option<&Entry>,
    new_e: Option<&Entry>,
    old: &dyn Source,
    new: &dyn Source,
) -> Result<Contents, Error> {
    let (old_b, old_err) = content(old, &c.path, old_e);
    let (new_b, new_err) = content(new, &c.path, new_e);
    if is_too_large(&old_err) || is_too_large(&new_err) || is_binary(&old_b) || is_binary(&new_b) {
        return Ok(Contents::Binary);
    }
    // The old side's error first, as Go checks oldErr before newErr.
    if let Some(err) = old_err.or(new_err) {
        let inner = match err {
            SourceError::TooLarge => Error::TooLarge,
            SourceError::Other(e) => e,
        };
        let text = format!("{}: {}", lossy(&c.path), inner);
        return Err(wrap(text, inner));
    }
    Ok(Contents::Text(
        old_b.unwrap_or_default(),
        new_b.unwrap_or_default(),
    ))
}

/// `worktree.Unified`.
pub fn unified(
    w: &mut dyn std::io::Write,
    changes: &[Change],
    old: &dyn Source,
    new: &dyn Source,
) -> Result<(), Error> {
    for c in changes {
        let res = unified_one(w, c, old, new);
        let _ = w.flush();
        res?;
    }
    Ok(())
}

fn unified_one(
    w: &mut dyn std::io::Write,
    c: &Change,
    old: &dyn Source,
    new: &dyn Source,
) -> Result<(), Error> {
    if c.kind == Kind::MetaChanged {
        return Ok(());
    }
    let (old_e, new_e) = sides(c);
    if old_e.is_none() && new_e.is_none() {
        if c.kind == Kind::ModeChanged {
            put(w, &diff_header(c));
            put(w, &mode_lines(c));
        }
        return Ok(());
    }
    put(w, &diff_header(c));
    put(w, &mode_lines(c));
    let label = |side: &[u8], e: Option<&Entry>| -> Vec<u8> {
        match e {
            None => b"/dev/null".to_vec(),
            Some(_) => {
                let mut l = side.to_vec();
                l.push(b'/');
                l.extend_from_slice(&c.path);
                l
            }
        }
    };
    let (label_a, label_b) = (label(b"a", old_e), label(b"b", new_e));
    let (old_b, new_b) = match read_sides(c, old_e, new_e, old, new)? {
        Contents::Binary => {
            let mut line = b"Binary files ".to_vec();
            line.extend_from_slice(&label_a);
            line.extend_from_slice(b" and ");
            line.extend_from_slice(&label_b);
            line.extend_from_slice(b" differ\n");
            put(w, &line);
            return Ok(());
        }
        Contents::Text(o, n) => (o, n),
    };
    if old_b == new_b {
        return Ok(());
    }
    let text = dstore_udiff::unified(&label_a, &label_b, &old_b, &new_b);
    w.write_all(&text).map_err(io_error)
}

/// `worktree.Stat`: " PATH | +A -D" or " PATH | binary" per content change, then the totals.
pub fn stat(
    w: &mut dyn std::io::Write,
    changes: &[Change],
    old: &dyn Source,
    new: &dyn Source,
) -> Result<(), Error> {
    let (mut files, mut ins, mut dels) = (0i64, 0i64, 0i64);
    for c in changes {
        if c.kind == Kind::MetaChanged {
            continue;
        }
        let (old_e, new_e) = sides(c);
        if old_e.is_none() && new_e.is_none() {
            continue;
        }
        let (old_b, new_b) = match read_sides(c, old_e, new_e, old, new)? {
            Contents::Binary => {
                let mut line = b" ".to_vec();
                line.extend_from_slice(&c.path);
                line.extend_from_slice(b" | binary\n");
                put(w, &line);
                let _ = w.flush();
                files += 1;
                continue;
            }
            Contents::Text(o, n) => (o, n),
        };
        if old_b == new_b {
            continue;
        }
        let (mut a, mut d) = (0i64, 0i64);
        let text = dstore_udiff::unified(b"a", b"b", &old_b, &new_b);
        // strings.SplitAfter(text, "\n"): "+++"/"---" lines are not counted, so neither are content lines
        // starting with "++" or "--" (a Go bug kept on purpose).
        for line in text.split_inclusive(|&b| b == b'\n') {
            if line.starts_with(b"+++") || line.starts_with(b"---") {
                continue;
            }
            if line.starts_with(b"+") {
                a += 1;
            } else if line.starts_with(b"-") {
                d += 1;
            }
        }
        let mut line = b" ".to_vec();
        line.extend_from_slice(&c.path);
        line.extend_from_slice(format!(" | +{a} -{d}\n").as_bytes());
        put(w, &line);
        let _ = w.flush();
        files += 1;
        ins += a;
        dels += d;
    }
    put(
        w,
        format!(" {files} files changed, {ins} insertions(+), {dels} deletions(-)\n").as_bytes(),
    );
    let _ = w.flush();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{Scratch, ingest_dir, open_store, symlink, write_file};
    use crate::{diff_trees, scan};
    use dstore_gocompat::time::GoTime;

    /// `diffFixtures`.
    fn diff_fixtures(st: &amber_store_core::packstore::Store) -> (Key, Key, Scratch, Scratch) {
        let adir = Scratch::new();
        write_file(&adir.join("edit.txt"), "one\ntwo\nthree\n", 0o644);
        write_file(&adir.join("bin"), "a\x00b", 0o644);
        write_file(&adir.join("gone.txt"), "bye\n", 0o644);
        write_file(&adir.join("run.sh"), "#!/bin/sh\n", 0o644);
        write_file(&adir.join("sub/x"), "x\n", 0o644);
        symlink("t1", &adir.join("link"));
        let a = ingest_dir(st, &adir.path());
        let bdir = Scratch::new();
        write_file(&bdir.join("edit.txt"), "one\n2\nthree\n", 0o644);
        write_file(&bdir.join("bin"), "a\x00c", 0o644);
        write_file(&bdir.join("new.txt"), "hi\n", 0o644);
        write_file(&bdir.join("run.sh"), "#!/bin/sh\n", 0o755);
        write_file(&bdir.join("sub/x"), "x\n", 0o644);
        crate::sys::chmod(&bdir.join_bytes("sub"), 0o700).expect("chmod");
        symlink("t2", &bdir.join("link"));
        let b = ingest_dir(st, &bdir.path());
        (a, b, adir, bdir)
    }

    // Port of TestUnified_TreeToTree.
    #[test]
    fn unified_tree_to_tree() {
        let (_sd, st) = open_store();
        let (a, b, _adir, _bdir) = diff_fixtures(&st);
        let get = |k: Key| st.get(k);
        let changes = diff_trees(&get, a, b).expect("diff");
        let src = TreeSource { get: &get };
        let mut buf = Vec::new();
        unified(&mut buf, &changes, &src, &src).expect("unified");
        let out = String::from_utf8_lossy(&buf).into_owned();
        for want in [
            "diff a/edit.txt b/edit.txt\n--- a/edit.txt\n+++ b/edit.txt\n",
            "-two\n+2\n",
            "Binary files a/bin and b/bin differ\n",
            "--- /dev/null\n+++ b/new.txt\n",
            "+hi\n",
            "--- a/gone.txt\n+++ /dev/null\n",
            "-bye\n",
            "diff a/run.sh b/run.sh\nold mode 0644\nnew mode 0755\n",
            "diff a/link b/link\n",
            "-t1",
            "+t2",
            "diff a/sub b/sub\nold mode 0755\nnew mode 0700\n",
        ] {
            assert!(out.contains(want), "output lacks {want:?}:\n{out}");
        }
        assert!(
            !out.contains("--- a/sub\n") && !out.contains("--- a/run.sh\n"),
            "{out}"
        );
    }

    // Port of TestUnified_TreeToDisk.
    #[test]
    fn unified_tree_to_disk() {
        let (_sd, st) = open_store();
        let (a, _b, _adir, bdir) = diff_fixtures(&st);
        let get = |k: Key| st.get(k);
        let changes = scan(&bdir.bytes(), a, &get, GoTime::now(), 2).expect("scan");
        let mut buf = Vec::new();
        let disk = DiskSource { root: bdir.bytes() };
        unified(&mut buf, &changes, &TreeSource { get: &get }, &disk).expect("unified");
        let out = String::from_utf8_lossy(&buf).into_owned();
        assert!(out.contains("-two\n+2\n") && out.contains("+t2"), "{out}");
    }

    // Port of TestStat.
    #[test]
    fn stat_lines() {
        let (_sd, st) = open_store();
        let (a, b, _adir, _bdir) = diff_fixtures(&st);
        let get = |k: Key| st.get(k);
        let changes = diff_trees(&get, a, b).expect("diff");
        let src = TreeSource { get: &get };
        let mut buf = Vec::new();
        stat(&mut buf, &changes, &src, &src).expect("stat");
        let out = String::from_utf8_lossy(&buf).into_owned();
        for want in [
            " edit.txt | +1 -1\n",
            " bin | binary\n",
            " new.txt | +1 -0\n",
            " gone.txt | +0 -1\n",
            " link | +1 -1\n",
            "files changed",
        ] {
            assert!(out.contains(want), "stat lacks {want:?}:\n{out}");
        }
        assert!(!out.contains("sub |"), "{out}");
    }

    #[test]
    fn binary_heuristic_window() {
        let mut b = vec![b'x'; 9000];
        b[8191] = 0;
        assert!(is_binary(&Some(b.clone())));
        b[8191] = b'x';
        b[8192] = 0;
        assert!(!is_binary(&Some(b)));
        assert!(!is_binary(&None));
    }
}
