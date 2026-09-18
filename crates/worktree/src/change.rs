//! `worktree/change.go`: change classification and the tree diff (blocking).

use std::sync::Arc;

use amber_store_core::fstree::{self, Entry};
use amber_store_core::key::Key;

use crate::sys::{S_IFBLK, S_IFCHR, S_IFDIR, S_IFIFO, S_IFLNK, S_IFMT, S_IFREG, S_IFSOCK};
use crate::{Error, Getter};

/// `worktree.Kind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Added,
    Deleted,
    Modified,
    TypeChanged,
    ModeChanged,
    MetaChanged,
}

/// "new" "deleted" "modified" "type" "mode" "meta".
impl std::fmt::Display for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Kind::Added => "new",
            Kind::Deleted => "deleted",
            Kind::Modified => "modified",
            Kind::TypeChanged => "type",
            Kind::ModeChanged => "mode",
            Kind::MetaChanged => "meta",
        })
    }
}

/// `worktree.Change`.
#[derive(Clone, Debug)]
pub struct Change {
    pub path: Vec<u8>,
    pub kind: Kind,
    pub old: Option<Arc<Entry>>,
    pub new: Option<Arc<Entry>>,
}

/// `worktree.TypeName`: the entry's file type, else `type %#o` of the type bits.
pub fn type_name(mode: u64) -> String {
    match mode & S_IFMT {
        S_IFREG => "file".to_string(),
        S_IFDIR => "directory".to_string(),
        S_IFLNK => "symlink".to_string(),
        S_IFIFO => "fifo".to_string(),
        S_IFSOCK => "socket".to_string(),
        S_IFCHR => "char device".to_string(),
        S_IFBLK => "block device".to_string(),
        t => format!("type {}", crate::sys::octal_alt(t)),
    }
}

/// `worktree.IsDir`.
pub fn is_dir(e: Option<&Entry>) -> bool {
    e.is_some_and(|e| e.mode & S_IFMT == S_IFDIR)
}

/// `worktree.SameContent`: by `a`'s type, the content key of a file, the target of a link, the numbers of
/// a device; every other type compares equal.
pub fn same_content(a: &Entry, b: &Entry) -> bool {
    match a.mode & S_IFMT {
        S_IFREG => a.content_key == b.content_key,
        S_IFLNK => a.link_target == b.link_target,
        S_IFCHR | S_IFBLK => a.rdev == b.rdev,
        _ => true,
    }
}

/// `worktree.Equivalent`: same type, content and permission bits.
pub fn equivalent(a: &Entry, b: &Entry) -> bool {
    a.mode & S_IFMT == b.mode & S_IFMT && same_content(a, b) && a.mode & 0o7777 == b.mode & 0o7777
}

/// `worktree.Compare`: `None` when the entries are identical.
pub fn compare(old: &Entry, new: &Entry) -> Option<Kind> {
    if old.mode & S_IFMT != new.mode & S_IFMT {
        Some(Kind::TypeChanged)
    } else if !same_content(old, new) {
        Some(Kind::Modified)
    } else if old.mode & 0o7777 != new.mode & 0o7777 {
        Some(Kind::ModeChanged)
    } else if old.uid != new.uid
        || old.gid != new.gid
        || old.mtime != new.mtime
        || old.xattrs_in != new.xattrs_in
        || old.xattrs_key != new.xattrs_key
    {
        Some(Kind::MetaChanged)
    } else {
        None
    }
}

/// `joinPath`.
pub(crate) fn join_path(prefix: &[u8], name: &[u8]) -> Vec<u8> {
    if prefix.is_empty() {
        return name.to_vec();
    }
    let mut p = Vec::with_capacity(prefix.len() + 1 + name.len());
    p.extend_from_slice(prefix);
    p.push(b'/');
    p.extend_from_slice(name);
    p
}

/// `fstree.CollectEntries` through a `Getter`, each entry shared.
pub(crate) fn collect(get: Getter<'_>, k: Key) -> Result<Vec<Arc<Entry>>, Error> {
    fstree::collect_entries(k, get)
        .map(|entries| entries.into_iter().map(Arc::new).collect())
        .map_err(Error::Walk)
}

/// `worktree.DiffTrees`: changes in walk order.
pub fn diff_trees(get: Getter<'_>, a: Key, b: Key) -> Result<Vec<Change>, Error> {
    let mut out = Vec::new();
    if a == b {
        return Ok(out);
    }
    diff_dirs(get, b"", a, b, &mut out)?;
    Ok(out)
}

fn diff_dirs(
    get: Getter<'_>,
    prefix: &[u8],
    a: Key,
    b: Key,
    out: &mut Vec<Change>,
) -> Result<(), Error> {
    let ea = collect(get, a)?;
    let eb = collect(get, b)?;
    let (mut i, mut j) = (0usize, 0usize);
    loop {
        let (x, y) = (ea.get(i), eb.get(j));
        match (x, y) {
            (None, None) => return Ok(()),
            (Some(x), None) => {
                expand(get, prefix, x, Kind::Deleted, out)?;
                i += 1;
            }
            (None, Some(y)) => {
                expand(get, prefix, y, Kind::Added, out)?;
                j += 1;
            }
            (Some(x), Some(y)) => match x.name.cmp(&y.name) {
                std::cmp::Ordering::Less => {
                    expand(get, prefix, x, Kind::Deleted, out)?;
                    i += 1;
                }
                std::cmp::Ordering::Greater => {
                    expand(get, prefix, y, Kind::Added, out)?;
                    j += 1;
                }
                std::cmp::Ordering::Equal => {
                    let p = join_path(prefix, &x.name);
                    if let Some(k) = compare(x, y) {
                        out.push(Change {
                            path: p.clone(),
                            kind: k,
                            old: Some(Arc::clone(x)),
                            new: Some(Arc::clone(y)),
                        });
                        if k == Kind::TypeChanged {
                            // A directory that became something else loses its contents; something that
                            // became a directory gains them.
                            expand_children(get, &p, x, Kind::Deleted, out)?;
                            expand_children(get, &p, y, Kind::Added, out)?;
                        }
                    }
                    if is_dir(Some(x)) && is_dir(Some(y)) && x.content_key != y.content_key {
                        let kx = Key::parse(&x.content_key).map_err(Error::Key)?;
                        let ky = Key::parse(&y.content_key).map_err(Error::Key)?;
                        diff_dirs(get, &p, kx, ky, out)?;
                    }
                    i += 1;
                    j += 1;
                }
            },
        }
    }
}

/// `expand`: a change of `kind` (Added or Deleted) for `e` and, for a directory, every path below it.
pub(crate) fn expand(
    get: Getter<'_>,
    prefix: &[u8],
    e: &Arc<Entry>,
    kind: Kind,
    out: &mut Vec<Change>,
) -> Result<(), Error> {
    let p = join_path(prefix, &e.name);
    let (old, new) = if kind == Kind::Added {
        (None, Some(Arc::clone(e)))
    } else {
        (Some(Arc::clone(e)), None)
    };
    out.push(Change {
        path: p.clone(),
        kind,
        old,
        new,
    });
    expand_children(get, &p, e, kind, out)
}

/// `expandChildren`: a change of `kind` for every path below the directory entry `e` at `p`; nothing for
/// a non-directory.
pub(crate) fn expand_children(
    get: Getter<'_>,
    p: &[u8],
    e: &Entry,
    kind: Kind,
    out: &mut Vec<Change>,
) -> Result<(), Error> {
    if !is_dir(Some(e)) {
        return Ok(());
    }
    let k = Key::parse(&e.content_key).map_err(Error::Key)?;
    for child in collect(get, k)? {
        expand(get, p, &child, kind, out)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::testutil::{Scratch, ingest_dir, kinds, open_store, symlink, write_file};

    #[test]
    fn kind_strings() {
        let got: Vec<String> = [
            Kind::Added,
            Kind::Deleted,
            Kind::Modified,
            Kind::TypeChanged,
            Kind::ModeChanged,
            Kind::MetaChanged,
        ]
        .iter()
        .map(|k| k.to_string())
        .collect();
        assert_eq!(got, ["new", "deleted", "modified", "type", "mode", "meta"]);
    }

    #[test]
    fn type_names() {
        assert_eq!(type_name(0o100644), "file");
        assert_eq!(type_name(0o040755), "directory");
        assert_eq!(type_name(0o120777), "symlink");
        assert_eq!(type_name(0o010644), "fifo");
        assert_eq!(type_name(0o140755), "socket");
        assert_eq!(type_name(0o020644), "char device");
        assert_eq!(type_name(0o060660), "block device");
        assert_eq!(type_name(0), "type 0");
        assert_eq!(type_name(0o755), "type 0");
        assert_eq!(type_name(0o160000), "type 0160000");
    }

    // Port of TestDiffTrees_EveryKind.
    #[test]
    fn diff_trees_every_kind() {
        let (_sd, st) = open_store();
        let a = Scratch::new();
        write_file(&a.join("same.txt"), "same", 0o644);
        write_file(&a.join("edit.txt"), "one", 0o644);
        write_file(&a.join("gone.txt"), "bye", 0o644);
        write_file(&a.join("mode.sh"), "#!/bin/sh", 0o644);
        write_file(&a.join("sub/deep.txt"), "deep", 0o644);
        write_file(&a.join("olddir/x"), "x", 0o644);
        symlink("t1", &a.join("link"));
        symlink("was-link", &a.join("becomes-file"));
        write_file(&a.join("flip/inner"), "inner", 0o644);
        let ka = ingest_dir(&st, &a.path());

        let b = Scratch::new();
        write_file(&b.join("same.txt"), "same", 0o644);
        write_file(&b.join("edit.txt"), "two", 0o644);
        write_file(&b.join("new.txt"), "hi", 0o644);
        write_file(&b.join("mode.sh"), "#!/bin/sh", 0o755);
        write_file(&b.join("sub/deep.txt"), "deep", 0o644);
        write_file(&b.join("newdir/y"), "y", 0o644);
        write_file(&b.join("becomes-file"), "now a file", 0o644);
        write_file(&b.join("flip"), "flat", 0o644);
        symlink("t2", &b.join("link"));
        let kb = ingest_dir(&st, &b.path());

        let get = |k: Key| st.get(k);
        let changes = diff_trees(&get, ka, kb).expect("diff");
        let got = kinds(&changes);
        let want: BTreeMap<&str, Kind> = [
            ("edit.txt", Kind::Modified),
            ("gone.txt", Kind::Deleted),
            ("new.txt", Kind::Added),
            ("mode.sh", Kind::ModeChanged),
            ("link", Kind::Modified),
            ("becomes-file", Kind::TypeChanged),
            ("olddir", Kind::Deleted),
            ("olddir/x", Kind::Deleted),
            ("newdir", Kind::Added),
            ("newdir/y", Kind::Added),
            ("flip", Kind::TypeChanged),
            ("flip/inner", Kind::Deleted),
        ]
        .into_iter()
        .collect();
        for (p, k) in &want {
            assert_eq!(got.get(p.as_bytes()), Some(k), "{p}");
        }
        for (p, k) in &got {
            let p = String::from_utf8_lossy(p);
            if want.contains_key(p.as_ref()) {
                continue;
            }
            assert!(
                ["same.txt", "sub", "sub/deep.txt"].contains(&p.as_ref()),
                "unexpected change {p}: {k}"
            );
            assert_eq!(*k, Kind::MetaChanged, "{p}: at most MetaChanged");
        }
        for w in changes.windows(2) {
            assert!(w[0].path < w[1].path, "out of order");
        }
    }

    // Port of TestDiffTrees_PrunesEqualSubtrees.
    #[test]
    fn diff_trees_prunes_equal_subtrees() {
        let (_sd, st) = open_store();
        let a = Scratch::new();
        write_file(&a.join("sub/x"), "x", 0o644);
        write_file(&a.join("top"), "1", 0o644);
        let ka = ingest_dir(&st, &a.path());
        write_file(&a.join("top"), "2", 0o644);
        let kb = ingest_dir(&st, &a.path());
        let calls = AtomicUsize::new(0);
        let get = |k: Key| {
            calls.fetch_add(1, Ordering::Relaxed);
            st.get(k)
        };
        let changes = diff_trees(&get, ka, kb).expect("diff");
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, b"top");
        assert_eq!(changes[0].kind, Kind::Modified);
        assert_eq!(calls.load(Ordering::Relaxed), 2, "only the two roots");
    }

    #[test]
    fn diff_trees_identical_roots_read_nothing() {
        let calls = AtomicUsize::new(0);
        let get = |_: Key| {
            calls.fetch_add(1, Ordering::Relaxed);
            Err(amber_store_core::packstore::Error::NotFound)
        };
        let (k, _) = crate::empty_tree();
        assert!(diff_trees(&get, k, k).expect("diff").is_empty());
        assert_eq!(calls.load(Ordering::Relaxed), 0);
    }

    // Port of TestCompare.
    #[test]
    fn compare_cases() {
        let file = |ck: u8, mode: u64, mtime: i64| {
            let mut k = vec![0u8; 32];
            k[1] = ck;
            Entry {
                name: b"f".to_vec(),
                mode: S_IFREG | mode,
                mtime,
                content_key: k,
                ..Default::default()
            }
        };
        let link = Entry {
            name: b"f".to_vec(),
            mode: S_IFLNK | 0o777,
            link_target: b"x".to_vec(),
            ..Default::default()
        };
        let cases = [
            ("equal", file(1, 0o644, 1), file(1, 0o644, 1), None),
            (
                "content",
                file(1, 0o644, 1),
                file(2, 0o644, 1),
                Some(Kind::Modified),
            ),
            (
                "mode",
                file(1, 0o644, 1),
                file(1, 0o755, 1),
                Some(Kind::ModeChanged),
            ),
            (
                "mtime",
                file(1, 0o644, 1),
                file(1, 0o644, 2),
                Some(Kind::MetaChanged),
            ),
            ("type", file(1, 0o644, 1), link, Some(Kind::TypeChanged)),
        ];
        for (name, a, b, want) in cases {
            assert_eq!(compare(&a, &b), want, "{name}");
        }
        // A content change that also changes the mode is Modified.
        assert_eq!(
            compare(&file(1, 0o644, 1), &file(2, 0o755, 1)),
            Some(Kind::Modified)
        );
    }

    #[test]
    fn same_content_rules() {
        let dev = |rdev: Vec<u64>| Entry {
            mode: S_IFCHR | 0o644,
            rdev,
            ..Default::default()
        };
        assert!(same_content(&dev(vec![1, 3]), &dev(vec![1, 3])));
        assert!(!same_content(&dev(vec![1, 3]), &dev(vec![1, 4])));
        let fifo = |mtime| Entry {
            mode: S_IFIFO | 0o644,
            mtime,
            ..Default::default()
        };
        assert!(same_content(&fifo(1), &fifo(2)));
        assert!(is_dir(Some(&Entry {
            mode: S_IFDIR | 0o755,
            ..Default::default()
        })));
        assert!(!is_dir(None));
    }
}
