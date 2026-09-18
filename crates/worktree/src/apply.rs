//! `worktree/apply.go`: the applier (blocking; not abortable, as in Go).

use std::collections::BTreeMap;
use std::io;

use amber_store_core::cbor;
use amber_store_core::fstree::{self, Entry};
use amber_store_core::key::Key;
use dstore_gocompat::errno::{PathError, io_error_text};
use dstore_gocompat::os;
use dstore_gocompat::path::{self, to_path};
use dstore_gocompat::quote::quote;
use dstore_udiff::gosort;

use crate::error::{cbor_error, io_error, lossy, prefixed, wrap};
use crate::sys::{self, S_IFBLK, S_IFCHR, S_IFDIR, S_IFIFO, S_IFLNK, S_IFMT, S_IFREG, S_IFSOCK};
use crate::{Change, Error, Getter, Kind};

/// `worktree.Apply`: deletions first (deepest paths first, directories only when empty), then additions
/// and modifications in path order creating missing parents, then the saved directory modes, then the
/// directory metadata deepest first.
pub fn apply(root: &[u8], changes: &[Change], get: Getter<'_>) -> Result<(), Error> {
    let root = path::abs(root).map_err(io_error)?;
    for c in changes {
        check_path(&c.path)?;
    }
    let mut dels: Vec<&Change> = Vec::new();
    let mut rest: Vec<&Change> = Vec::new();
    for c in changes {
        if c.kind == Kind::Deleted {
            dels.push(c);
        } else {
            rest.push(c);
        }
    }
    gosort::slice(&mut dels, |a, b| a.path > b.path);
    gosort::slice(&mut rest, |a, b| a.path < b.path);

    let mut ap = Applier {
        root,
        restore: BTreeMap::new(),
    };
    for c in dels {
        let t = match ap.target(&c.path) {
            Ok(t) => t,
            // An ancestor became a file: nothing below it can exist.
            Err(e) => match *e {
                TargetError::NotDir(_) => continue,
                TargetError::Other(e) => return Err(e),
            },
        };
        if let Err(e) = ap.writable(&path::dir(&t))
            && !e.is_not_exist()
        {
            return Err(e.into_error());
        }
        if let Err(e) = os::remove(&t) {
            let ignored = [libc::ENOENT, libc::ENOTEMPTY, libc::EEXIST];
            if !ignored
                .iter()
                .any(|&code| e.err.raw_os_error() == Some(code))
            {
                return Err(prefixed(&c.path, Error::Path(e)));
            }
        }
    }

    let mut dirs: Vec<&Change> = Vec::new();
    for c in rest {
        let t = ap.target(&c.path).map_err(|e| (*e).into_error())?;
        os::mkdir_all(&path::dir(&t), 0o755).map_err(Error::Path)?;
        ap.writable(&path::dir(&t))
            .map_err(WritableError::into_error)?;
        // Go dereferences c.New here; a non-deletion without a new entry is refused instead.
        let Some(e) = c.new.as_deref() else {
            return Err(Error::Msg(format!(
                "{}: {} change has no new entry",
                lossy(&c.path),
                c.kind
            )));
        };
        clear_target(&t, e).map_err(|err| prefixed(&c.path, err))?;
        let content_change = c.kind != Kind::ModeChanged && c.kind != Kind::MetaChanged;
        match e.mode & S_IFMT {
            S_IFDIR => {
                if let Err(err) = sys::mkdir(&t, 0o700)
                    && !matches!(err.err.raw_os_error(), Some(libc::EEXIST | libc::ENOTEMPTY))
                {
                    return Err(prefixed(&c.path, Error::Path(err)));
                }
                dirs.push(c);
                continue;
            }
            S_IFREG => {
                if content_change {
                    write_regular(&t, e, get).map_err(|err| prefixed(&c.path, err))?;
                }
            }
            S_IFLNK => {
                if content_change {
                    remove_existing(&t).map_err(|err| prefixed(&c.path, err))?;
                    sys::symlink(&e.link_target, &t)
                        .map_err(|err| prefixed(&c.path, wrap(err.to_string(), err)))?;
                }
            }
            S_IFIFO => {
                if content_change {
                    remove_existing(&t).map_err(|err| prefixed(&c.path, err))?;
                    sys::mkfifo(&to_path(&t), (e.mode & 0o7777) as u32).map_err(|err| {
                        wrap(
                            format!("{}: mkfifo: {}", lossy(&c.path), io_error_text(&err)),
                            err,
                        )
                    })?;
                }
            }
            S_IFCHR | S_IFBLK => {
                if content_change {
                    remove_existing(&t).map_err(|err| prefixed(&c.path, err))?;
                    let (major, minor) = match e.rdev.as_slice() {
                        [major, minor] => (*major as u32, *minor as u32),
                        _ => (0, 0),
                    };
                    let dev = sys::mkdev(major, minor);
                    sys::mknod(&t, e.mode & (S_IFMT | 0o7777), dev).map_err(|err| {
                        wrap(
                            format!("{}: mknod: {}", lossy(&c.path), io_error_text(&err)),
                            err,
                        )
                    })?;
                }
            }
            // Sockets carry no payload and cannot be recreated.
            S_IFSOCK => continue,
            other => {
                return Err(Error::Msg(format!(
                    "{}: unsupported type {}",
                    lossy(&c.path),
                    sys::octal_alt(other)
                )));
            }
        }
        apply_meta(&t, e, get).map_err(|err| prefixed(&c.path, err))?;
    }

    // Go ranges over a map here (random order, PORTING DD-10); deepest paths first.
    for (dir, mode) in ap.restore.iter().rev() {
        sys::chmod(dir, *mode).map_err(io_error)?;
    }
    gosort::slice(&mut dirs, |a, b| a.path > b.path);
    for c in dirs {
        let t = path::join(&[&ap.root, &c.path]);
        if let Some(e) = c.new.as_deref() {
            apply_meta(&t, e, get).map_err(|err| prefixed(&c.path, err))?;
        }
    }
    Ok(())
}

/// `checkPath`: no empty path, and no "", "." or ".." component.
fn check_path(p: &[u8]) -> Result<(), Error> {
    if p.is_empty() {
        return Err(Error::Msg("empty path".to_string()));
    }
    if p.split(|&b| b == b'/')
        .any(|part| part.is_empty() || part == b"." || part == b"..")
    {
        return Err(Error::Msg(format!("refusing unsafe path {}", quote(p))));
    }
    Ok(())
}

enum TargetError {
    /// `errNotDir`: an existing ancestor is not a directory.
    NotDir(Error),
    Other(Error),
}

impl TargetError {
    fn into_error(self) -> Error {
        match self {
            TargetError::NotDir(e) | TargetError::Other(e) => e,
        }
    }
}

enum WritableError {
    Lstat(PathError),
    Chmod(io::Error),
}

impl WritableError {
    /// `errors.Is(err, fs.ErrNotExist)`.
    fn is_not_exist(&self) -> bool {
        let e = match self {
            WritableError::Lstat(pe) => &pe.err,
            WritableError::Chmod(e) => e,
        };
        e.raw_os_error() == Some(libc::ENOENT)
    }

    fn into_error(self) -> Error {
        match self {
            WritableError::Lstat(pe) => Error::Path(pe),
            WritableError::Chmod(e) => io_error(e),
        }
    }
}

struct Applier {
    root: Vec<u8>,
    /// Directories opened for writing, with their original permission bits.
    restore: BTreeMap<Vec<u8>, u32>,
}

impl Applier {
    /// `target`: `Join(root, p)`, refusing to write through a symlink or a non-directory below root.
    fn target(&self, p: &[u8]) -> Result<Vec<u8>, Box<TargetError>> {
        let t = path::join(&[&self.root, p]);
        reject_symlink_components(&self.root, &t)?;
        Ok(t)
    }

    /// `writable`: gives the owner rwx on a directory without it, recording its mode first.
    fn writable(&mut self, dir: &[u8]) -> Result<(), WritableError> {
        if self.restore.contains_key(dir) {
            return Ok(());
        }
        let md = sys::os_lstat(dir).map_err(WritableError::Lstat)?;
        let mode = std::os::unix::fs::MetadataExt::mode(&md) & 0o7777;
        if !md.is_dir() || mode & 0o700 == 0o700 {
            return Ok(());
        }
        self.restore.insert(dir.to_vec(), mode);
        sys::chmod(dir, mode | 0o700).map_err(WritableError::Chmod)
    }
}

/// `rejectSymlinkComponents`: every existing ancestor of `t` strictly below `root` must be a real directory.
fn reject_symlink_components(root: &[u8], t: &[u8]) -> Result<(), Box<TargetError>> {
    let mut prefix = root.to_vec();
    prefix.push(b'/');
    let mut p = path::dir(t);
    while p.starts_with(&prefix) {
        match sys::lstat(&p) {
            Err(e) if e.raw_os_error() == Some(libc::ENOENT) => {}
            Err(e) => {
                return Err(Box::new(TargetError::Other(Error::Path(PathError {
                    op: "lstat",
                    path: p,
                    err: e,
                }))));
            }
            Ok(md) if md.file_type().is_symlink() => {
                return Err(Box::new(TargetError::Other(Error::Msg(format!(
                    "refusing to write through non-directory {}",
                    lossy(&p)
                )))));
            }
            Ok(md) if !md.is_dir() => {
                return Err(Box::new(TargetError::NotDir(Error::Msg(format!(
                    "{}: not a directory",
                    lossy(&p)
                )))));
            }
            Ok(_) => {}
        }
        p = path::dir(&p);
    }
    Ok(())
}

/// `clearTarget`: removes what is at `t` when its type differs from `e`'s.
fn clear_target(t: &[u8], e: &Entry) -> Result<(), Error> {
    match sys::lstat(t) {
        Err(err) if err.raw_os_error() == Some(libc::ENOENT) => Ok(()),
        Err(err) => Err(Error::Path(PathError {
            op: "lstat",
            path: t.to_vec(),
            err,
        })),
        Ok(md) => {
            let have = u64::from(std::os::unix::fs::MetadataExt::mode(&md)) & S_IFMT;
            if have != e.mode & S_IFMT {
                os::remove_all(t).map_err(Error::Path)
            } else {
                Ok(())
            }
        }
    }
}

/// `os.Remove(t)`, a missing entry ignored.
fn remove_existing(t: &[u8]) -> Result<(), Error> {
    match os::remove(t) {
        Err(e) if e.err.raw_os_error() != Some(libc::ENOENT) => Err(Error::Path(e)),
        _ => Ok(()),
    }
}

/// A temporary file whose write errors read like Go's (`write <name>: <errno>`).
struct TempFile {
    f: std::fs::File,
    name: Vec<u8>,
}

impl io::Write for TempFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.f.write(buf).map_err(|e| self.go_error(e))
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.f.write_all(buf).map_err(|e| self.go_error(e))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl TempFile {
    fn go_error(&self, e: io::Error) -> io::Error {
        let kind = e.kind();
        io::Error::new(
            kind,
            PathError {
                op: "write",
                path: self.name.clone(),
                err: e,
            },
        )
    }
}

/// `writeRegular`: streams the content under `e`'s key to `.dstore-tmp-*` beside `t` and renames it over
/// `t`. No fsync.
fn write_regular(t: &[u8], e: &Entry, get: Getter<'_>) -> Result<(), Error> {
    let ck = Key::parse(&e.content_key).map_err(Error::Key)?;
    let (f, tmp) = os::create_temp(&path::dir(t), ".dstore-tmp-*").map_err(Error::Path)?;
    let mut w = TempFile { f, name: tmp };
    if let Err(err) = fstree::write_content(&mut w, ck, get) {
        let _ = sys::close(w.f);
        let _ = os::remove(&w.name);
        return Err(Error::Walk(err));
    }
    let TempFile { f, name: tmp } = w;
    if let Err(err) = sys::close(f) {
        let _ = os::remove(&tmp);
        return Err(Error::Path(PathError {
            op: "close",
            path: tmp,
            err,
        }));
    }
    if let Err(err) = sys::rename(&tmp, t) {
        let _ = os::remove(&tmp);
        return Err(wrap(err.to_string(), err));
    }
    Ok(())
}

/// `applyMeta`: ownership (as root), permission bits, xattrs, then atime and mtime.
fn apply_meta(t: &[u8], e: &Entry, get: Getter<'_>) -> Result<(), Error> {
    let is_link = e.mode & S_IFMT == S_IFLNK;
    if sys::geteuid() == 0
        && let Err(err) = sys::lchown(t, e.uid, e.gid)
    {
        return Err(wrap(format!("chown: {err}"), err));
    }
    if !is_link {
        sys::chmod(t, (e.mode & 0o7777) as u32)
            .map_err(|err| wrap(format!("chmod: {}", io_error_text(&err)), err))?;
        let xattrs = entry_xattrs(e, get)?;
        for (name, val) in &xattrs {
            if let Err(err) = sys::set_xattr(&to_path(t), name, val) {
                let skip = [libc::EPERM, libc::EACCES, libc::ENOTSUP, libc::EOPNOTSUPP];
                if skip.iter().any(|&code| err.raw_os_error() == Some(code)) {
                    continue; // best effort
                }
                return Err(wrap(
                    format!("xattr {}: {}", quote(name), io_error_text(&err)),
                    err,
                ));
            }
        }
    }
    sys::utimes_nano_at(t, e.mtime, is_link)
        .map_err(|err| wrap(format!("set mtime: {}", io_error_text(&err)), err))
}

/// `entryXattrs`: an entry's xattrs, inline or spilled.
fn entry_xattrs(e: &Entry, get: Getter<'_>) -> Result<BTreeMap<Vec<u8>, Vec<u8>>, Error> {
    if !e.xattrs_in.is_empty() {
        return cbor::decode_xattrs(&e.xattrs_in).map_err(cbor_error);
    }
    if e.xattrs_key.len() == 32 {
        let k = Key::parse(&e.xattrs_key).map_err(Error::Key)?;
        let b = get(k).map_err(Error::Packstore)?;
        return cbor::decode_xattrs(&b).map_err(cbor_error);
    }
    Ok(BTreeMap::new())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use amber_store_core::packstore;

    use super::*;
    use crate::testutil::{Scratch, ingest_dir, open_store, set_mtime, symlink, write_file};
    use crate::{diff_trees, empty_tree};

    const OLD_SECS: i64 = 1_600_000_000;
    const OLD_NSEC: u32 = 123_456_789;

    /// `applyFixtureA`.
    fn apply_fixture_a() -> Scratch {
        let dir = Scratch::new();
        write_file(&dir.join("keep.txt"), "keep", 0o644);
        write_file(&dir.join("edit.txt"), "one", 0o644);
        write_file(&dir.join("gone.txt"), "bye", 0o644);
        write_file(&dir.join("run.sh"), "#!/bin/sh", 0o644);
        write_file(&dir.join("sub/deep.txt"), "deep", 0o600);
        write_file(&dir.join("olddir/x"), "x", 0o644);
        write_file(&dir.join("flip/inner"), "inner", 0o644);
        write_file(&dir.join("flop"), "flop", 0o644);
        symlink("keep.txt", &dir.join("link"));
        crate::sys::mkfifo(&dir.join("pipe"), 0o600).expect("mkfifo");
        for p in ["keep.txt", "edit.txt", "sub/deep.txt", "sub", "run.sh"] {
            set_mtime(&dir.join(p), OLD_SECS, OLD_NSEC);
        }
        dir
    }

    /// `applyFixtureB`.
    fn apply_fixture_b() -> Scratch {
        let dir = Scratch::new();
        write_file(&dir.join("keep.txt"), "keep", 0o644);
        write_file(&dir.join("edit.txt"), "two", 0o644);
        write_file(&dir.join("run.sh"), "#!/bin/sh", 0o755);
        write_file(&dir.join("sub/deep.txt"), "deep", 0o600);
        write_file(&dir.join("newdir/y"), "y", 0o644);
        write_file(&dir.join("flip"), "flat", 0o644);
        write_file(&dir.join("flop/inner"), "inner", 0o644);
        write_file(&dir.join("ro/child"), "c", 0o644);
        symlink("edit.txt", &dir.join("link"));
        crate::sys::mkfifo(&dir.join("pipe"), 0o600).expect("mkfifo");
        for p in ["keep.txt", "sub/deep.txt", "sub"] {
            set_mtime(&dir.join(p), OLD_SECS, OLD_NSEC);
        }
        std::fs::set_permissions(dir.join("ro"), std::fs::Permissions::from_mode(0o500))
            .expect("chmod ro");
        dir
    }

    fn put_empty(st: &packstore::Store) {
        let (k, b) = empty_tree();
        st.put(k, &b).expect("put empty tree");
    }

    // Port of TestApply_CloneThenUpdateReproducesTrees.
    #[test]
    fn clone_then_update_reproduces_trees() {
        let (_sd, st) = open_store();
        let get = |k: Key| st.get(k);
        let a = apply_fixture_a();
        let ka = ingest_dir(&st, &a.path());
        put_empty(&st);
        let (empty, _) = empty_tree();

        let work = Scratch::new();
        let initial = diff_trees(&get, empty, ka).expect("diff");
        apply(&work.bytes(), &initial, &get).expect("initial apply");
        assert_eq!(ingest_dir(&st, &work.path()), ka, "after clone");

        let b = apply_fixture_b();
        let kb = ingest_dir(&st, &b.path());
        let update = diff_trees(&get, ka, kb).expect("diff");
        apply(&work.bytes(), &update, &get).expect("update apply");
        assert_eq!(ingest_dir(&st, &work.path()), kb, "after update");
        apply(&work.bytes(), &update, &get).expect("second apply");
        assert_eq!(ingest_dir(&st, &work.path()), kb, "after re-apply");
        let md = std::fs::symlink_metadata(work.join("ro")).expect("lstat ro");
        assert_eq!(md.permissions().mode() & 0o7777, 0o500);
    }

    // Port of TestApply_KeepsNonEmptyDirectoryOnDelete.
    #[test]
    fn keeps_non_empty_directory_on_delete() {
        let (_sd, st) = open_store();
        let get = |k: Key| st.get(k);
        let a = apply_fixture_a();
        let ka = ingest_dir(&st, &a.path());
        let work = Scratch::new();
        put_empty(&st);
        let (empty, _) = empty_tree();
        let initial = diff_trees(&get, empty, ka).expect("diff");
        apply(&work.bytes(), &initial, &get).expect("apply");
        write_file(&work.join("olddir/mine"), "mine", 0o644);
        std::fs::remove_dir_all(a.join("olddir")).expect("remove olddir");
        let kb = ingest_dir(&st, &a.path());
        let update = diff_trees(&get, ka, kb).expect("diff");
        apply(&work.bytes(), &update, &get).expect("apply update");
        assert!(work.join("olddir/mine").exists(), "local file was lost");
        assert!(!work.join("olddir/x").exists(), "olddir/x should be gone");
    }

    // Port of TestApply_RefusesUnsafePaths.
    #[test]
    fn refuses_unsafe_paths() {
        let work = Scratch::new();
        let e = std::sync::Arc::new(Entry {
            name: b"x".to_vec(),
            mode: S_IFREG | 0o644,
            ..Default::default()
        });
        let get = |_: Key| Err(packstore::Error::NotFound);
        for p in ["../x", "a/../x", "a//x", "./x", ""] {
            let c = Change {
                path: p.as_bytes().to_vec(),
                kind: Kind::Added,
                old: None,
                new: Some(e.clone()),
            };
            let err = apply(&work.bytes(), &[c], &get).err();
            assert!(err.is_some(), "path {p:?} accepted");
        }
    }

    // Port of TestApply_RefusesSymlinkedAncestor.
    #[test]
    fn refuses_symlinked_ancestor() {
        let work = Scratch::new();
        let outside = Scratch::new();
        std::os::unix::fs::symlink(outside.path(), work.join("lnk")).expect("symlink");
        let blob = fstree::encode_blob(b"pwned");
        let bytes = blob.bytes.clone();
        let get = move |_: Key| Ok(bytes.clone());
        let e = std::sync::Arc::new(Entry {
            name: b"f".to_vec(),
            mode: S_IFREG | 0o644,
            content_key: blob.key.0.to_vec(),
            ..Default::default()
        });
        let c = Change {
            path: b"lnk/f".to_vec(),
            kind: Kind::Added,
            old: None,
            new: Some(e),
        };
        let err = apply(&work.bytes(), &[c], &get).expect_err("must refuse");
        assert!(err.to_string().contains("non-directory"), "{err}");
        let leaked = std::fs::read_dir(outside.path()).expect("readdir").count();
        assert_eq!(leaked, 0, "wrote through the link");
    }

    #[test]
    fn refusal_texts() {
        let work = Scratch::new();
        let get = |_: Key| Err(packstore::Error::NotFound);
        let file = std::sync::Arc::new(Entry {
            name: b"x".to_vec(),
            mode: 0o160644,
            ..Default::default()
        });
        let one = |path: &[u8], kind: Kind| Change {
            path: path.to_vec(),
            kind,
            old: Some(file.clone()),
            new: Some(file.clone()),
        };
        let text = |c: Change| {
            apply(&work.bytes(), &[c], &get)
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default()
        };
        assert_eq!(text(one(b"", Kind::Added)), "empty path");
        assert_eq!(
            text(one(b"a//x", Kind::Added)),
            "refusing unsafe path \"a//x\""
        );
        assert_eq!(
            text(one(b"\xff\t\"\\/../x", Kind::Added)),
            "refusing unsafe path \"\\xff\\t\\\"\\\\/../x\""
        );
        assert_eq!(text(one(b"x", Kind::Added)), "x: unsupported type 0160000");
        write_file(&work.join("f"), "x", 0o644);
        assert_eq!(
            text(one(b"f/g", Kind::Added)),
            format!("{}/f: not a directory", work.path().display())
        );
        // A deletion below a file is skipped.
        assert_eq!(text(one(b"f/g", Kind::Deleted)), "");
    }

    #[test]
    fn writes_devices_fifos_and_restores_modes() {
        let work = Scratch::new();
        let get = |_: Key| Err(packstore::Error::NotFound);
        let dir = std::sync::Arc::new(Entry {
            name: b"d".to_vec(),
            mode: S_IFDIR | 0o500,
            mtime: 1_500_000_000_000_000_001,
            ..Default::default()
        });
        let fifo = std::sync::Arc::new(Entry {
            name: b"p".to_vec(),
            mode: S_IFIFO | 0o640,
            mtime: 1_500_000_000_000_000_002,
            ..Default::default()
        });
        let changes = [
            Change {
                path: b"d".to_vec(),
                kind: Kind::Added,
                old: None,
                new: Some(dir),
            },
            Change {
                path: b"d/p".to_vec(),
                kind: Kind::Added,
                old: None,
                new: Some(fifo),
            },
        ];
        apply(&work.bytes(), &changes, &get).expect("apply");
        let md = std::fs::symlink_metadata(work.join("d")).expect("lstat d");
        assert_eq!(md.permissions().mode() & 0o7777, 0o500);
        assert_eq!(
            crate::sys::unix_nano(&md),
            1_500_000_000_000_000_001,
            "directory mtime set last"
        );
        let md = std::fs::symlink_metadata(work.join("d/p")).expect("lstat p");
        assert_eq!(
            u64::from(std::os::unix::fs::MetadataExt::mode(&md)),
            S_IFIFO | 0o640
        );
        // Re-applying into the read-only directory works and keeps its mode.
        apply(&work.bytes(), &changes, &get).expect("re-apply");
        let md = std::fs::symlink_metadata(work.join("d")).expect("lstat d");
        assert_eq!(md.permissions().mode() & 0o7777, 0o500);
    }
}
