//! `worktree/scan.go`: the working directory against a tree (blocking).

use std::os::unix::fs::MetadataExt;
use std::sync::Arc;

use amber_store_core::amberignore::{self, Matcher};
use amber_store_core::fstree::{self, Entry};
use amber_store_core::key::Key;
use amber_store_core::{cbor, ingest};
use dstore_gocompat::errno::PathError;
use dstore_gocompat::path::{self, to_path};
use dstore_gocompat::time::GoTime;

use crate::change::{collect, expand, expand_children, join_path};
use crate::error::io_error;
use crate::sys::{self, S_IFBLK, S_IFCHR, S_IFDIR, S_IFLNK, S_IFMT, S_IFREG};
use crate::{Change, DIR, Error, Getter, Kind, RACY_WINDOW_NS, compare, is_dir};

struct Scanner<'a> {
    root: &'a [u8],
    get: Getter<'a>,
    synced_at: GoTime,
    jobs: usize,
}

/// `worktree.Scan`: the changes from the tree `base` to the working directory `root`, in walk order.
/// `.amberignore` applies, the root's `.dstore` is skipped, and a regular file whose size and mtime match
/// its base entry is not read unless the base mtime lies within `RACY_WINDOW_NS` of `synced_at`.
pub fn scan(
    root: &[u8],
    base: Key,
    get: Getter<'_>,
    synced_at: GoTime,
    jobs: usize,
) -> Result<Vec<Change>, Error> {
    // Go reads `filepath.Join(root, ".amberignore")`, which is lexically cleaned; core-rs joins without
    // cleaning, so hand it the cleaned root (the same file for a root like `a/..`).
    let ign = load_matcher(root, Matcher::root(to_path(&path::clean(root))))?;
    let s = Scanner {
        root,
        get,
        synced_at,
        jobs,
    };
    let mut out = Vec::new();
    s.dir(root, b"", base, &ign, &mut out)?;
    Ok(out)
}

/// Maps a failed `.amberignore` load to Go's `os.ReadFile` PathError (`open …` or `read …`). core-rs returns
/// the bare `io::Error`, so the file is read once more to learn which step fails; if that read succeeds
/// (a race), the error is reported as an `open` failure.
fn load_matcher(dir: &[u8], m: std::io::Result<Matcher>) -> Result<Matcher, Error> {
    match m {
        Ok(m) => Ok(m),
        Err(e) => {
            let file = path::join(&[dir, amberignore::FILE_NAME.as_bytes()]);
            match dstore_gocompat::os::read_file(&file) {
                Err(pe) => Err(Error::Path(pe)),
                Ok(_) => Err(Error::Path(PathError {
                    op: "open",
                    path: file,
                    err: e,
                })),
            }
        }
    }
}

/// `(*amberignore.Matcher).Descend(abs, name)`.
fn descend(ign: &Matcher, abs: &[u8], name: &[u8]) -> Result<Matcher, Error> {
    load_matcher(abs, ign.descend(to_path(abs), name))
}

/// A disk entry as ingest would record it, plus its size.
struct DiskEntry {
    entry: Entry,
    size: u64,
}

impl Scanner<'_> {
    /// `listDir`: the entries ingest would see, sorted bytewise; ignored names and the root's `.dstore`
    /// dropped.
    fn list_dir(&self, abs: &[u8], ign: &Matcher) -> Result<Vec<sys::DirEnt>, Error> {
        let mut ents = sys::read_dir(abs).map_err(Error::Path)?;
        ents.retain(|de| {
            let root_dstore = abs == self.root && de.name == DIR.as_bytes();
            !root_dstore && !ign.ignored(&de.name, de.is_dir)
        });
        Ok(ents)
    }

    fn dir(
        &self,
        abs: &[u8],
        prefix: &[u8],
        dir_key: Key,
        ign: &Matcher,
        out: &mut Vec<Change>,
    ) -> Result<(), Error> {
        let disk = self.list_dir(abs, ign)?;
        let base = collect(self.get, dir_key)?;
        let (mut i, mut j) = (0usize, 0usize);
        loop {
            match (base.get(i), disk.get(j)) {
                (None, None) => return Ok(()),
                (Some(b), None) => {
                    expand(self.get, prefix, b, Kind::Deleted, out)?;
                    i += 1;
                }
                (None, Some(d)) => {
                    self.added(abs, prefix, &d.name, ign, out)?;
                    j += 1;
                }
                (Some(b), Some(d)) => match b.name.as_slice().cmp(d.name.as_slice()) {
                    std::cmp::Ordering::Less => {
                        expand(self.get, prefix, b, Kind::Deleted, out)?;
                        i += 1;
                    }
                    std::cmp::Ordering::Greater => {
                        self.added(abs, prefix, &d.name, ign, out)?;
                        j += 1;
                    }
                    std::cmp::Ordering::Equal => {
                        self.both(abs, prefix, b, ign, out)?;
                        i += 1;
                        j += 1;
                    }
                },
            }
        }
    }

    /// `added`: the disk entry `name` under `abs`, and everything below it, as added.
    fn added(
        &self,
        abs: &[u8],
        prefix: &[u8],
        name: &[u8],
        ign: &Matcher,
        out: &mut Vec<Change>,
    ) -> Result<(), Error> {
        let full = path::join(&[abs, name]);
        let e = self.entry(&full, name, true)?;
        let p = join_path(prefix, name);
        let dir = is_dir(Some(&e.entry));
        out.push(Change {
            path: p.clone(),
            kind: Kind::Added,
            old: None,
            new: Some(Arc::new(e.entry)),
        });
        if dir {
            return self.added_children(&full, &p, name, ign, out);
        }
        Ok(())
    }

    fn added_children(
        &self,
        full: &[u8],
        p: &[u8],
        name: &[u8],
        ign: &Matcher,
        out: &mut Vec<Change>,
    ) -> Result<(), Error> {
        let sub = descend(ign, full, name)?;
        for de in self.list_dir(full, &sub)? {
            self.added(full, p, &de.name, &sub, out)?;
        }
        Ok(())
    }

    /// `both`: the base entry `b` against the disk entry of the same name.
    fn both(
        &self,
        abs: &[u8],
        prefix: &[u8],
        b: &Arc<Entry>,
        ign: &Matcher,
        out: &mut Vec<Change>,
    ) -> Result<(), Error> {
        let name = b.name.as_slice();
        let full = path::join(&[abs, name]);
        let p = join_path(prefix, name);
        if b.mode & S_IFMT != disk_type(&full) {
            let e = self.entry(&full, name, true)?;
            let new_is_dir = is_dir(Some(&e.entry));
            out.push(Change {
                path: p.clone(),
                kind: Kind::TypeChanged,
                old: Some(Arc::clone(b)),
                new: Some(Arc::new(e.entry)),
            });
            expand_children(self.get, &p, b, Kind::Deleted, out)?;
            if new_is_dir {
                return self.added_children(&full, &p, name, ign, out);
            }
            return Ok(());
        }
        let mut e = self.entry(&full, name, false)?;
        match e.entry.mode & S_IFMT {
            S_IFREG => {
                let bk = Key::parse(&b.content_key).map_err(Error::Key)?;
                let racy_limit = self.synced_at.add_ns(-RACY_WINDOW_NS).unix_nano();
                if e.size != bk.length() || e.entry.mtime != b.mtime || b.mtime > racy_limit {
                    e.entry.content_key = hash_file(&full, self.jobs)?.0.to_vec();
                } else {
                    e.entry.content_key = b.content_key.clone();
                }
            }
            // The directory's own change is mode or metadata.
            S_IFDIR => e.entry.content_key = b.content_key.clone(),
            _ => {}
        }
        if let Some(k) = compare(b, &e.entry) {
            out.push(Change {
                path: p.clone(),
                kind: k,
                old: Some(Arc::clone(b)),
                new: Some(Arc::new(e.entry)),
            });
        }
        if is_dir(Some(b)) {
            let sub = descend(ign, &full, name)?;
            let bk = Key::parse(&b.content_key).map_err(Error::Key)?;
            return self.dir(&full, &p, bk, &sub, out);
        }
        Ok(())
    }

    /// `entry`: the entry at `full` as ingest records it (type, mode, ownership, mtime, link target, device
    /// numbers, xattrs); with `hash`, a regular file's content key too.
    fn entry(&self, full: &[u8], name: &[u8], hash: bool) -> Result<DiskEntry, Error> {
        let md = sys::os_lstat(full).map_err(Error::Path)?;
        let mut e = Entry {
            name: name.to_vec(),
            mode: u64::from(md.mode()),
            uid: u64::from(md.uid()),
            gid: u64::from(md.gid()),
            mtime: sys::unix_nano(&md),
            ..Default::default()
        };
        match e.mode & S_IFMT {
            S_IFREG if hash => e.content_key = hash_file(full, self.jobs)?.0.to_vec(),
            S_IFLNK => e.link_target = sys::readlink(full).map_err(Error::Path)?,
            S_IFCHR | S_IFBLK => {
                let rdev = md.rdev();
                e.rdev = vec![u64::from(sys::major(rdev)), u64::from(sys::minor(rdev))];
            }
            _ => {}
        }
        if e.mode & S_IFMT != S_IFLNK {
            let xattrs = sys::read_xattrs(&to_path(full)).map_err(io_error)?;
            if !xattrs.is_empty() {
                let enc = cbor::encode_xattrs(&xattrs);
                if enc.len() <= ingest::DEFAULT_XATTR_INLINE_MAX {
                    e.xattrs_in = enc;
                } else {
                    e.xattrs_key = fstree::encode_xattr_set(&xattrs).key.0.to_vec();
                }
            }
        }
        Ok(DiskEntry {
            entry: e,
            size: md.size(),
        })
    }
}

/// `diskType`: the `S_IFMT` bits of the entry at `full`, 0 when Lstat fails.
fn disk_type(full: &[u8]) -> u64 {
    match sys::lstat(full) {
        Ok(md) => u64::from(md.mode()) & S_IFMT,
        Err(_) => 0,
    }
}

/// `hashFile`: the content key ingest gives the regular file at `path`, without storing anything.
fn hash_file(path: &[u8], jobs: usize) -> Result<Key, Error> {
    let opts = ingest::Opts {
        jobs,
        ..Default::default()
    };
    let (stream, root) = ingest::objects(to_path(path), opts).map_err(Error::Ingest)?;
    for item in stream {
        item.map_err(Error::Ingest)?;
    }
    // Unreachable: a fully drained stream without error sets the root.
    root.get().ok_or(Error::Ingest(ingest::Error::Stopped))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{Scratch, ingest_dir, kinds, open_store, set_mtime, symlink, write_file};
    use std::collections::BTreeMap;

    /// `scanFixture`.
    fn scan_fixture(st: &amber_store_core::packstore::Store) -> (Scratch, Key) {
        let dir = Scratch::new();
        write_file(&dir.join("a.txt"), "alpha", 0o644);
        write_file(&dir.join("run.sh"), "#!/bin/sh", 0o644);
        write_file(&dir.join("sub/b.txt"), "beta", 0o644);
        write_file(&dir.join("gone.txt"), "bye", 0o644);
        symlink("a.txt", &dir.join("link"));
        let k = ingest_dir(st, &dir.path());
        (dir, k)
    }

    fn scan_kinds(
        dir: &Scratch,
        base: Key,
        st: &amber_store_core::packstore::Store,
        synced_at: GoTime,
    ) -> BTreeMap<Vec<u8>, Kind> {
        let get = |k: Key| st.get(k);
        kinds(&scan(&dir.bytes(), base, &get, synced_at, 2).expect("scan"))
    }

    fn want(pairs: &[(&str, Kind)]) -> BTreeMap<Vec<u8>, Kind> {
        pairs
            .iter()
            .map(|(p, k)| (p.as_bytes().to_vec(), *k))
            .collect()
    }

    // Port of TestScan_CleanTreeHasNoChanges.
    #[test]
    fn clean_tree_has_no_changes() {
        let (_sd, st) = open_store();
        let (dir, base) = scan_fixture(&st);
        let got = scan_kinds(&dir, base, &st, GoTime::now());
        assert!(got.is_empty(), "changes on a clean tree: {got:?}");
    }

    // Port of TestScan_EveryKind.
    #[test]
    fn every_kind() {
        let (_sd, st) = open_store();
        let (dir, base) = scan_fixture(&st);
        write_file(&dir.join("a.txt"), "alpha 2", 0o644);
        crate::sys::chmod(&dir.join_bytes("run.sh"), 0o755).expect("chmod");
        std::fs::remove_file(dir.join("gone.txt")).expect("remove");
        write_file(&dir.join("new.txt"), "new", 0o644);
        write_file(&dir.join("newdir/c.txt"), "c", 0o644);
        set_mtime(&dir.join("sub/b.txt"), 1_600_000_000, 0);
        std::fs::remove_file(dir.join("link")).expect("remove link");
        symlink("run.sh", &dir.join("link"));
        write_file(&dir.join(".dstore/junk"), "x", 0o644);

        let mut got = scan_kinds(&dir, base, &st, GoTime::now());
        got.remove(b"sub".as_slice());
        assert_eq!(
            got,
            want(&[
                ("a.txt", Kind::Modified),
                ("run.sh", Kind::ModeChanged),
                ("gone.txt", Kind::Deleted),
                ("new.txt", Kind::Added),
                ("newdir", Kind::Added),
                ("newdir/c.txt", Kind::Added),
                ("sub/b.txt", Kind::MetaChanged),
                ("link", Kind::Modified),
            ])
        );
    }

    // Port of TestScan_IgnoredBasePathIsDeleted.
    #[test]
    fn ignored_base_path_is_deleted() {
        let (_sd, st) = open_store();
        let (dir, base) = scan_fixture(&st);
        write_file(&dir.join(".amberignore"), "gone.txt\n", 0o644);
        let got = scan_kinds(&dir, base, &st, GoTime::now());
        assert_eq!(got.get(b"gone.txt".as_slice()), Some(&Kind::Deleted));
        assert_eq!(got.get(b".amberignore".as_slice()), Some(&Kind::Added));
    }

    // Port of TestScan_RacyMtime.
    #[test]
    fn racy_mtime() {
        let (_sd, st) = open_store();
        let (dir, base) = scan_fixture(&st);
        let md = std::fs::symlink_metadata(dir.join("a.txt")).expect("lstat");
        write_file(&dir.join("a.txt"), "ALPHA", 0o644);
        set_mtime(&dir.join("a.txt"), md.mtime(), md.mtime_nsec() as u32);
        let far = GoTime::now().add_ns(3600 * 1_000_000_000);
        let got = scan_kinds(&dir, base, &st, far);
        assert!(got.is_empty(), "far-future syncedAt: {got:?}");
        let got = scan_kinds(&dir, base, &st, GoTime::now());
        assert_eq!(got.get(b"a.txt".as_slice()), Some(&Kind::Modified));
    }

    // Port of TestScan_TypeChangeExpands.
    #[test]
    fn type_change_expands() {
        let (_sd, st) = open_store();
        let (dir, base) = scan_fixture(&st);
        std::fs::remove_file(dir.join("a.txt")).expect("remove");
        write_file(&dir.join("a.txt/inner"), "i", 0o644);
        std::fs::remove_dir_all(dir.join("sub")).expect("remove sub");
        write_file(&dir.join("sub"), "flat", 0o644);
        let got = scan_kinds(&dir, base, &st, GoTime::now());
        for (p, k) in [
            ("a.txt", Kind::TypeChanged),
            ("a.txt/inner", Kind::Added),
            ("sub", Kind::TypeChanged),
            ("sub/b.txt", Kind::Deleted),
        ] {
            assert_eq!(got.get(p.as_bytes()), Some(&k), "{p}");
        }
    }

    // Port of TestScan_Xattr.
    #[test]
    fn xattr_change_is_meta() {
        let (_sd, st) = open_store();
        let (dir, base) = scan_fixture(&st);
        if let Err(e) = crate::sys::set_xattr(&dir.join("a.txt"), b"user.wc", b"1") {
            if crate::sys::is_unsupported(&e) {
                eprintln!("no xattr support here");
                return;
            }
            panic!("setxattr: {e}");
        }
        let got = scan_kinds(&dir, base, &st, GoTime::now());
        assert_eq!(got.get(b"a.txt".as_slice()), Some(&Kind::MetaChanged));
    }

    /// The inline-or-spilled decision at the boundary. `user.x` is sized so that the entry's encoded xattr
    /// map, including attributes the file system adds on its own (macOS `com.apple.provenance`, SELinux
    /// labels), is exactly 256 bytes (inline) or 257 (spilled to an XattrSet key). A clean scan means the
    /// disk entries equal what ingest recorded.
    #[test]
    fn xattr_inline_limit_matches_ingest() {
        let (_sd, st) = open_store();
        let dir = Scratch::new();
        for (name, want_len) in [("small", 256usize), ("big", 257)] {
            let p = dir.join(name);
            write_file(&p, name, 0o644);
            let mut m = crate::sys::read_xattrs(&p).expect("read xattrs");
            let sized = |n: usize| {
                let mut probe = m.clone();
                probe.insert(b"user.x".to_vec(), vec![b'v'; n]);
                cbor::encode_xattrs(&probe).len()
            };
            let Some(n) = (0..=want_len).find(|&n| sized(n) == want_len) else {
                eprintln!("{name}: the file system's own xattrs are too large for this test");
                return;
            };
            if let Err(e) = crate::sys::set_xattr(&p, b"user.x", &vec![b'v'; n]) {
                if crate::sys::is_unsupported(&e) {
                    eprintln!("no xattr support here");
                    return;
                }
                panic!("setxattr: {e}");
            }
            m.insert(b"user.x".to_vec(), vec![b'v'; n]);
            let got = crate::sys::read_xattrs(&p).expect("read xattrs");
            assert_eq!(got, m, "{name}: attributes on disk");
        }
        let base = ingest_dir(&st, &dir.path());
        let get = |k: Key| st.get(k);
        let changes = scan(&dir.bytes(), base, &get, GoTime::now(), 2).expect("scan");
        assert!(changes.is_empty(), "changes: {:?}", kinds(&changes));

        let s = Scanner {
            root: &dir.bytes(),
            get: &get,
            synced_at: GoTime::now(),
            jobs: 2,
        };
        for (name, inline) in [("small", true), ("big", false)] {
            let xattrs = crate::sys::read_xattrs(&dir.join(name)).expect("read xattrs");
            if xattrs.len() != 1 {
                // The file system added attributes of its own; the clean scan above still holds.
                continue;
            }
            let e = s
                .entry(&dir.join_bytes(name), name.as_bytes(), false)
                .expect("entry")
                .entry;
            let enc = cbor::encode_xattrs(&xattrs);
            assert_eq!(enc.len(), if inline { 256 } else { 257 }, "{name}");
            if inline {
                assert_eq!(e.xattrs_in, enc, "{name}");
                assert!(e.xattrs_key.is_empty(), "{name}");
            } else {
                assert!(e.xattrs_in.is_empty(), "{name}");
                assert_eq!(
                    e.xattrs_key,
                    fstree::encode_xattr_set(&xattrs).key.0,
                    "{name}"
                );
            }
        }
    }

    #[test]
    fn walk_order_is_not_bytewise() {
        let (_sd, st) = open_store();
        let (dir, base) = scan_fixture(&st);
        write_file(&dir.join("a/x"), "x", 0o644);
        write_file(&dir.join("a-b"), "ab", 0o644);
        let get = |k: Key| st.get(k);
        let changes = scan(&dir.bytes(), base, &get, GoTime::now(), 2).expect("scan");
        let paths: Vec<&[u8]> = changes.iter().map(|c| c.path.as_slice()).collect();
        assert_eq!(paths, [b"a".as_slice(), b"a/x", b"a-b"]);
    }

    #[test]
    fn amberignore_directory_is_a_read_error() {
        let (_sd, st) = open_store();
        let (dir, base) = scan_fixture(&st);
        std::fs::create_dir(dir.join(".amberignore")).expect("mkdir");
        let get = |k: Key| st.get(k);
        let e = scan(&dir.bytes(), base, &get, GoTime::now(), 2).expect_err("scan must fail");
        assert_eq!(
            e.to_string(),
            format!("read {}/.amberignore: is a directory", dir.path().display())
        );
    }
}
