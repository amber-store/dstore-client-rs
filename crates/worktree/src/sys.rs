//! `x/sys/unix` equivalents (core-rs-gaps §4.3): xattrs, device numbers and mtimes, plus the Go `os` and
//! `unix` calls the applier and the scan make, with Go's error shapes.
//!
//! The xattr reader and the device-number splits duplicate core-rs `ingest::xattrs` and `ingest::meta`,
//! which are `pub(crate)` there (core-rs-gaps G9); `mkdev` exists nowhere in core-rs.

use std::collections::BTreeMap;
use std::ffi::{CString, OsStr};
use std::fs::Metadata;
use std::io;
use std::os::fd::IntoRawFd;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::Path;

use dstore_gocompat::errno::{PathError, io_error_text};
use dstore_gocompat::path::to_path;

/// `S_IFMT` and the file-type values (the same on Linux and macOS).
pub(crate) const S_IFMT: u64 = 0o170000;
pub(crate) const S_IFSOCK: u64 = 0o140000;
pub(crate) const S_IFLNK: u64 = 0o120000;
pub(crate) const S_IFREG: u64 = 0o100000;
pub(crate) const S_IFBLK: u64 = 0o060000;
pub(crate) const S_IFDIR: u64 = 0o040000;
pub(crate) const S_IFCHR: u64 = 0o020000;
pub(crate) const S_IFIFO: u64 = 0o010000;

/// The errno Go's `Getxattr` reports when an attribute vanishes between the list and the fetch; the `xattr`
/// crate maps it to `None`.
#[cfg(target_os = "linux")]
const ENOATTR: i32 = libc::ENODATA;
#[cfg(not(target_os = "linux"))]
const ENOATTR: i32 = libc::ENOATTR;

/// Go `worktree.readXattrs`: macOS follows symlinks (`Listxattr`/`Getxattr`), Linux uses the l* calls.
/// ENOTSUP or EOPNOTSUPP from the list call means none; empty names are dropped.
pub fn read_xattrs(path: &Path) -> io::Result<BTreeMap<Vec<u8>, Vec<u8>>> {
    #[cfg(target_os = "linux")]
    let listed = xattr::list(path);
    #[cfg(not(target_os = "linux"))]
    let listed = xattr::list_deref(path);
    let names = match listed {
        Ok(names) => names,
        Err(e) if is_unsupported(&e) => return Ok(BTreeMap::new()),
        Err(e) => return Err(e),
    };
    let mut m = BTreeMap::new();
    for name in names {
        if name.is_empty() {
            continue;
        }
        #[cfg(target_os = "linux")]
        let got = xattr::get(path, &name)?;
        #[cfg(not(target_os = "linux"))]
        let got = xattr::get_deref(path, &name)?;
        match got {
            Some(v) => {
                m.insert(name.as_bytes().to_vec(), v);
            }
            None => return Err(io::Error::from_raw_os_error(ENOATTR)),
        }
    }
    Ok(m)
}

/// Go `worktree.setXattr`: `unix.Setxattr(path, name, value, 0)` on macOS (follows symlinks),
/// `unix.Lsetxattr` on Linux.
pub fn set_xattr(path: &Path, name: &[u8], value: &[u8]) -> io::Result<()> {
    let name = OsStr::from_bytes(name);
    #[cfg(target_os = "linux")]
    return xattr::set(path, name, value);
    #[cfg(not(target_os = "linux"))]
    return xattr::set_deref(path, name, value);
}

/// Go `ignoreUnsupported`: ENOTSUP or EOPNOTSUPP (distinct on macOS, aliases on Linux).
pub(crate) fn is_unsupported(e: &io::Error) -> bool {
    matches!(e.raw_os_error(), Some(code) if code == libc::ENOTSUP || code == libc::EOPNOTSUPP)
}

/// x/sys/unix `Major` (Linux: glibc `gnu_dev_major`).
#[cfg(target_os = "linux")]
pub fn major(dev: u64) -> u32 {
    (((dev & 0x0000_0000_000f_ff00) >> 8) | ((dev & 0xffff_f000_0000_0000) >> 32)) as u32
}

/// x/sys/unix `Minor` (Linux: glibc `gnu_dev_minor`).
#[cfg(target_os = "linux")]
pub fn minor(dev: u64) -> u32 {
    ((dev & 0x0000_0000_0000_00ff) | ((dev & 0x0000_0fff_fff0_0000) >> 12)) as u32
}

/// x/sys/unix `Mkdev` (Linux).
#[cfg(target_os = "linux")]
pub fn mkdev(major: u32, minor: u32) -> u64 {
    let (major, minor) = (u64::from(major), u64::from(minor));
    ((major & 0x0000_0fff) << 8)
        | ((major & 0xffff_f000) << 32)
        | (minor & 0x0000_00ff)
        | ((minor & 0xffff_ff00) << 12)
}

/// x/sys/unix `Major` (Darwin). `st_rdev` is an `int32` that `uint64(sys.Rdev)` sign-extends, as
/// `MetadataExt::rdev` does.
#[cfg(not(target_os = "linux"))]
pub fn major(dev: u64) -> u32 {
    ((dev >> 24) & 0xff) as u32
}

/// x/sys/unix `Minor` (Darwin).
#[cfg(not(target_os = "linux"))]
pub fn minor(dev: u64) -> u32 {
    (dev & 0xff_ffff) as u32
}

/// x/sys/unix `Mkdev` (Darwin).
#[cfg(not(target_os = "linux"))]
pub fn mkdev(major: u32, minor: u32) -> u64 {
    (u64::from(major) << 24) | u64::from(minor)
}

/// Go `info.ModTime().UnixNano()`: wrapping mtime*1e9 + nsec.
pub fn unix_nano(md: &Metadata) -> i64 {
    md.mtime()
        .wrapping_mul(1_000_000_000)
        .wrapping_add(md.mtime_nsec())
}

/// x/sys/unix `Mkfifo(path, mode)`: the bare errno on failure.
pub fn mkfifo(path: &Path, mode: u32) -> io::Result<()> {
    let c = cpath(path.as_os_str().as_bytes())?;
    // SAFETY: `c` is a valid NUL-terminated path for the duration of the call.
    let rc = unsafe { libc::mkfifo(c.as_ptr(), mode as libc::mode_t) };
    check(rc)
}

/// Go's `syscall.BytePtrFromString`: a NUL byte inside a path is EINVAL.
fn cpath(path: &[u8]) -> io::Result<CString> {
    CString::new(path).map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))
}

fn check(rc: libc::c_int) -> io::Result<()> {
    if rc == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Retries `f` while it fails with EINTR (Go `ignoringEINTR`).
fn retry_eintr(mut f: impl FnMut() -> libc::c_int) -> io::Result<()> {
    loop {
        match check(f()) {
            Err(e) if e.raw_os_error() == Some(libc::EINTR) => continue,
            other => return other,
        }
    }
}

/// x/sys/unix `Mknod(path, mode, dev)`: the bare errno on failure. The mode and device are truncated to
/// the platform's `mode_t` and `dev_t`, as the Go syscall is.
pub(crate) fn mknod(path: &[u8], mode: u64, dev: u64) -> io::Result<()> {
    let c = cpath(path)?;
    // SAFETY: `c` is a valid NUL-terminated path for the duration of the call.
    let rc = unsafe { libc::mknod(c.as_ptr(), mode as libc::mode_t, dev as libc::dev_t) };
    check(rc)
}

/// x/sys/unix `Chmod(path, mode)`: the bare errno on failure.
pub(crate) fn chmod(path: &[u8], mode: u32) -> io::Result<()> {
    let c = cpath(path)?;
    // SAFETY: `c` is a valid NUL-terminated path for the duration of the call.
    let rc = unsafe { libc::chmod(c.as_ptr(), mode as libc::mode_t) };
    check(rc)
}

/// Go `os.Lchown(name, int(uid), int(gid))`: `lchown <path>: <errno>`.
pub(crate) fn lchown(path: &[u8], uid: u64, gid: u64) -> Result<(), PathError> {
    let err = |e| PathError {
        op: "lchown",
        path: path.to_vec(),
        err: e,
    };
    let c = cpath(path).map_err(err)?;
    // SAFETY: `c` is a valid NUL-terminated path for the duration of each call.
    retry_eintr(|| unsafe { libc::lchown(c.as_ptr(), uid as libc::uid_t, gid as libc::gid_t) })
        .map_err(err)
}

/// x/sys/unix `NsecToTimespec`.
fn nsec_to_timespec(n: i64) -> libc::timespec {
    let mut sec = n / 1_000_000_000;
    let mut nsec = n % 1_000_000_000;
    if nsec < 0 {
        nsec += 1_000_000_000;
        sec -= 1;
    }
    // SAFETY: timespec is a plain C struct for which all-zero bytes are a valid value.
    let mut ts: libc::timespec = unsafe { std::mem::zeroed() };
    ts.tv_sec = sec as libc::time_t;
    ts.tv_nsec = nsec as libc::c_long;
    ts
}

/// x/sys/unix `UtimesNanoAt(AT_FDCWD, path, [ts, ts], flags)`: atime and mtime both set to `ns`;
/// `AT_SYMLINK_NOFOLLOW` when `nofollow`. The bare errno on failure.
pub(crate) fn utimes_nano_at(path: &[u8], ns: i64, nofollow: bool) -> io::Result<()> {
    let c = cpath(path)?;
    let ts = nsec_to_timespec(ns);
    let times = [ts, ts];
    let flags = if nofollow {
        libc::AT_SYMLINK_NOFOLLOW
    } else {
        0
    };
    // SAFETY: `c` is a valid NUL-terminated path and `times` two valid timespecs for the call.
    let rc = unsafe { libc::utimensat(libc::AT_FDCWD, c.as_ptr(), times.as_ptr(), flags) };
    check(rc)
}

/// `File.Close` with its error (dropping a `File` ignores it).
pub(crate) fn close(f: std::fs::File) -> io::Result<()> {
    let fd = f.into_raw_fd();
    // SAFETY: `fd` was owned by the File and is closed exactly once.
    let rc = unsafe { libc::close(fd) };
    check(rc)
}

/// `*os.LinkError`: `<op> <old> <new>: <errno text>`.
#[derive(Debug, thiserror::Error)]
#[error("{op} {} {}: {}", String::from_utf8_lossy(.old), String::from_utf8_lossy(.new), io_error_text(.err))]
pub(crate) struct LinkError {
    pub op: &'static str,
    pub old: Vec<u8>,
    pub new: Vec<u8>,
    #[source]
    pub err: io::Error,
}

fn link_error(op: &'static str, old: &[u8], new: &[u8], err: io::Error) -> LinkError {
    LinkError {
        op,
        old: old.to_vec(),
        new: new.to_vec(),
        err,
    }
}

/// `syscall.Lstat` with Go's EINVAL for a NUL byte.
pub(crate) fn lstat(path: &[u8]) -> io::Result<Metadata> {
    cpath(path)?;
    std::fs::symlink_metadata(to_path(path))
}

/// `os.Lstat`: `lstat <path>: <errno>`.
pub(crate) fn os_lstat(path: &[u8]) -> Result<Metadata, PathError> {
    lstat(path).map_err(|e| PathError {
        op: "lstat",
        path: path.to_vec(),
        err: e,
    })
}

/// Go `os.Rename` (`os/file_unix.go` `rename`): a directory at `new` is refused with EEXIST unless it is the
/// same file as `old` (a case-only rename); errors are `rename <old> <new>: <errno>`.
pub(crate) fn rename(old: &[u8], new: &[u8]) -> Result<(), LinkError> {
    if let Ok(fi) = lstat(new)
        && fi.is_dir()
    {
        match lstat(old) {
            Err(e) => return Err(link_error("rename", old, new, e)),
            Ok(ofi) => {
                if new == old || fi.dev() != ofi.dev() || fi.ino() != ofi.ino() {
                    let e = io::Error::from_raw_os_error(libc::EEXIST);
                    return Err(link_error("rename", old, new, e));
                }
            }
        }
    }
    let (co, cn) = match (cpath(old), cpath(new)) {
        (Ok(o), Ok(n)) => (o, n),
        (Err(e), _) | (_, Err(e)) => return Err(link_error("rename", old, new, e)),
    };
    // SAFETY: both are valid NUL-terminated paths for the duration of each call.
    retry_eintr(|| unsafe { libc::rename(co.as_ptr(), cn.as_ptr()) })
        .map_err(|e| link_error("rename", old, new, e))
}

/// Go `os.Symlink(old, new)`: `symlink <old> <new>: <errno>`.
pub(crate) fn symlink(old: &[u8], new: &[u8]) -> Result<(), LinkError> {
    let (co, cn) = match (cpath(old), cpath(new)) {
        (Ok(o), Ok(n)) => (o, n),
        (Err(e), _) | (_, Err(e)) => return Err(link_error("symlink", old, new, e)),
    };
    // SAFETY: both are valid NUL-terminated paths for the duration of each call.
    retry_eintr(|| unsafe { libc::symlink(co.as_ptr(), cn.as_ptr()) })
        .map_err(|e| link_error("symlink", old, new, e))
}

/// Go `os.Mkdir(name, perm)`: `mkdir <path>: <errno>`.
pub(crate) fn mkdir(path: &[u8], mode: u32) -> Result<(), PathError> {
    let err = |e| PathError {
        op: "mkdir",
        path: path.to_vec(),
        err: e,
    };
    cpath(path).map_err(err)?;
    std::fs::DirBuilder::new()
        .mode(mode & 0o777)
        .create(to_path(path))
        .map_err(err)
}

/// Go `os.Readlink`: `readlink <path>: <errno>`.
pub(crate) fn readlink(path: &[u8]) -> Result<Vec<u8>, PathError> {
    let err = |e| PathError {
        op: "readlink",
        path: path.to_vec(),
        err: e,
    };
    cpath(path).map_err(err)?;
    std::fs::read_link(to_path(path))
        .map(|t| t.into_os_string().into_vec())
        .map_err(err)
}

/// The op Go's `File.ReadDir` reports for a failed directory read.
#[cfg(target_os = "linux")]
const READDIR_OP: &str = "readdirent";
#[cfg(not(target_os = "linux"))]
const READDIR_OP: &str = "readdir";

/// One directory entry: its raw name and whether readdir's `d_type` (lstat as a fallback) says directory,
/// as Go's `DirEntry.IsDir` (a symlink to a directory is not a directory).
pub(crate) struct DirEnt {
    pub name: Vec<u8>,
    pub is_dir: bool,
}

/// Go `os.ReadDir`: entries sorted bytewise by name. An entry that vanishes between readdir and its lstat
/// fallback is skipped, as Go does.
pub(crate) fn read_dir(path: &[u8]) -> Result<Vec<DirEnt>, PathError> {
    let err = |op: &'static str, e: io::Error| PathError {
        op,
        path: path.to_vec(),
        err: e,
    };
    cpath(path).map_err(|e| err("open", e))?;
    let rd = std::fs::read_dir(to_path(path)).map_err(|e| err("open", e))?;
    let mut out = Vec::new();
    for de in rd {
        let de = de.map_err(|e| err(READDIR_OP, e))?;
        // Go's darwin and linux `readdir` skip zero-inode entries (deleted, not yet removed); std does not.
        if std::os::unix::fs::DirEntryExt::ino(&de) == 0 {
            continue;
        }
        let name = de.file_name().into_vec();
        let is_dir = match de.file_type() {
            Ok(ft) => ft.is_dir(),
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            // Go's DT_UNKNOWN fallback is `File.lstatat`, which reports `fstatat <dir>: <errno>`
            // (`os/statat_unix.go`, `File.wrapErr`), naming the directory, not the entry.
            Err(e) => return Err(err("fstatat", e)),
        };
        out.push(DirEnt { name, is_dir });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Go `os.ReadFile`, keeping the data read before an error (`open`/`read` PathErrors).
pub(crate) fn read_file_partial(path: &[u8]) -> (Option<Vec<u8>>, Option<PathError>) {
    use std::io::Read;
    let err = |op: &'static str, e: io::Error| PathError {
        op,
        path: path.to_vec(),
        err: e,
    };
    if let Err(e) = cpath(path) {
        return (None, Some(err("open", e)));
    }
    let mut f = match std::fs::File::open(to_path(path)) {
        Ok(f) => f,
        Err(e) => return (None, Some(err("open", e))),
    };
    let mut data = Vec::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        match f.read(&mut buf) {
            Ok(0) => return (Some(data), None),
            Ok(n) => data.extend_from_slice(buf.get(..n).unwrap_or_default()),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return (Some(data), Some(err("read", e))),
        }
    }
}

/// `os.Geteuid`.
pub(crate) fn geteuid() -> u32 {
    dstore_gocompat::os::geteuid()
}

/// Go `%#o` of a file-type value: `0` for zero, else a leading `0`.
pub(crate) fn octal_alt(v: u64) -> String {
    if v == 0 {
        "0".to_string()
    } else {
        format!("0{v:o}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_numbers_round_trip() {
        for (maj, min) in [(0u32, 0u32), (1, 3), (8, 1), (255, 0xff_ffff), (4, 64)] {
            let dev = mkdev(maj, min);
            assert_eq!((major(dev), minor(dev)), (maj, min), "{maj},{min}");
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_split_formulas() {
        // glibc makedev(0x12345, 0x6789abcd): 0x345<<8 | 0x12000<<32 | 0xcd | 0x6789ab00<<12.
        let dev = mkdev(0x12345, 0x6789abcd);
        assert_eq!(dev, 0x0001_2678_9ab3_45cd);
        assert_eq!(major(dev), 0x12345);
        assert_eq!(minor(dev), 0x6789abcd);
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn darwin_split_formulas() {
        assert_eq!(mkdev(1, 2), 0x0100_0002);
        // A negative int32 st_rdev sign-extends: only the low bits count.
        let dev = (-1i32) as u64;
        assert_eq!(major(dev), 0xff);
        assert_eq!(minor(dev), 0xff_ffff);
    }

    #[test]
    fn timespec_of_negative_ns() {
        let ts = nsec_to_timespec(-1);
        assert_eq!((ts.tv_sec, ts.tv_nsec), (-1, 999_999_999));
        let ts = nsec_to_timespec(1_600_000_000_123_456_789);
        assert_eq!((ts.tv_sec, ts.tv_nsec), (1_600_000_000, 123_456_789));
    }

    #[test]
    fn octal_alt_forms() {
        assert_eq!(octal_alt(0), "0");
        assert_eq!(octal_alt(0o160000), "0160000");
        assert_eq!(octal_alt(0o30000), "030000");
    }

    #[test]
    fn nul_in_path_is_einval() {
        let e = chmod(b"a\0b", 0o644).unwrap_err();
        assert_eq!(e.raw_os_error(), Some(libc::EINVAL));
        assert_eq!(io_error_text(&e), "invalid argument");
    }

    #[test]
    fn unsupported_errnos() {
        assert!(is_unsupported(&io::Error::from_raw_os_error(libc::ENOTSUP)));
        assert!(is_unsupported(&io::Error::from_raw_os_error(
            libc::EOPNOTSUPP
        )));
        assert!(!is_unsupported(&io::Error::from_raw_os_error(libc::EACCES)));
    }
}
