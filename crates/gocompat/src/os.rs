//! `os` functions dstore uses, with Go's error selection and texts (go1.26.5 `os/getwd.go`, `os/path.go`,
//! `os/file_unix.go`, `os/removeall_at.go`, `os/tempfile.go`, `os/file.go`), and cgo-less
//! `os/user.Current` (`os/user/cgo_lookup_unix.go` on darwin, `lookup_stubs.go` + `lookup_unix.go` on
//! linux).

use std::ffi::{CStr, CString};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};

use crate::errno::{PathError, io_error_text};
use crate::path::{from_path, to_path};

fn os_err(code: i32) -> io::Error {
    io::Error::from_raw_os_error(code)
}

fn path_err(op: &'static str, path: &[u8], err: io::Error) -> PathError {
    PathError {
        op,
        path: path.to_vec(),
        err,
    }
}

/// Go's `syscall.BytePtrFromString`: a NUL byte inside a path is EINVAL.
fn cstring(path: &[u8]) -> io::Result<CString> {
    CString::new(path).map_err(|_| os_err(libc::EINVAL))
}

fn has_errno(e: &io::Error, code: i32) -> bool {
    e.raw_os_error() == Some(code)
}

/// `os.IsNotExist` for a raw errno.
fn is_not_exist(e: &io::Error) -> bool {
    has_errno(e, libc::ENOENT)
}

/// `syscall.Stat` (follows symlinks).
fn stat(path: &[u8]) -> io::Result<std::fs::Metadata> {
    cstring(path)?;
    std::fs::metadata(to_path(path))
}

/// `syscall.Lstat`.
fn lstat(path: &[u8]) -> io::Result<std::fs::Metadata> {
    cstring(path)?;
    std::fs::symlink_metadata(to_path(path))
}

/// `os.Getwd`: `$PWD` if absolute and the same (dev, ino) as ".".
pub fn getwd() -> io::Result<Vec<u8>> {
    if let Some(pwd) = std::env::var_os("PWD") {
        let dir = pwd.as_bytes();
        if dir.first() == Some(&b'/') {
            let dot = std::fs::metadata(".")
                .map_err(|e| io::Error::new(e.kind(), path_err("stat", b".", e)))?;
            if let Ok(d) = stat(dir)
                && d.dev() == dot.dev()
                && d.ino() == dot.ino()
            {
                return Ok(dir.to_vec());
            }
        }
    }
    let getwd_err =
        |e: io::Error| io::Error::new(e.kind(), format!("getwd: {}", io_error_text(&e)));
    let dir = from_path(&std::env::current_dir().map_err(getwd_err)?);
    if dir.first() != Some(&b'/') {
        // syscall.Getwd on linux: an "(unreachable)" prefix is ENOENT.
        return Err(getwd_err(os_err(libc::ENOENT)));
    }
    Ok(dir)
}

/// `os.Mkdir` (`syscallMode(perm)` = the permission bits).
fn mkdir(path: &[u8], mode: u32) -> Result<(), PathError> {
    cstring(path).map_err(|e| path_err("mkdir", path, e))?;
    std::fs::DirBuilder::new()
        .mode(mode & 0o777)
        .create(to_path(path))
        .map_err(|e| path_err("mkdir", path, e))
}

/// `os.MkdirAll`.
pub fn mkdir_all(path: &[u8], mode: u32) -> Result<(), PathError> {
    // Fast path: stop with success or error if the path's type is known.
    if let Ok(md) = stat(path) {
        if md.is_dir() {
            return Ok(());
        }
        return Err(path_err("mkdir", path, os_err(libc::ENOTDIR)));
    }
    // The parent: drop trailing separators, then the last element.
    let mut j = path.len();
    while j > 0 && path.get(j - 1) == Some(&b'/') {
        j -= 1;
    }
    while j > 0 && path.get(j - 1) != Some(&b'/') {
        j -= 1;
    }
    let parent = path.get(..j.saturating_sub(1)).unwrap_or_default();
    if !parent.is_empty() {
        mkdir_all(parent, mode)?;
    }
    if let Err(e) = mkdir(path, mode) {
        // Handle arguments like "foo/." by double-checking that the directory doesn't exist.
        if lstat(path).is_ok_and(|md| md.is_dir()) {
            return Ok(());
        }
        return Err(e);
    }
    Ok(())
}

/// `os.Remove`: unlink, then rmdir; the rmdir error unless it is ENOTDIR.
pub fn remove(path: &[u8]) -> Result<(), PathError> {
    let (e, e1) = match cstring(path) {
        Err(e) => (e, os_err(libc::EINVAL)),
        Ok(_) => {
            let p = to_path(path);
            let e = match std::fs::remove_file(&p) {
                Ok(()) => return Ok(()),
                Err(e) => e,
            };
            match std::fs::remove_dir(&p) {
                Ok(()) => return Ok(()),
                Err(e1) => (e, e1),
            }
        }
    };
    let err = if has_errno(&e1, libc::ENOTDIR) { e } else { e1 };
    Err(path_err("remove", path, err))
}

fn ends_with_dot(path: &[u8]) -> bool {
    path == b"." || path.ends_with(b"/.")
}

/// `os.splitPath` (`os/path_unix.go`).
fn split_path(path: &[u8]) -> (&[u8], &[u8]) {
    let mut p = path;
    while p.len() > 1 && p.starts_with(b"//") {
        p = p.get(1..).unwrap_or_default();
    }
    while p.len() > 1 && p.ends_with(b"/") {
        p = p.get(..p.len() - 1).unwrap_or_default();
    }
    // Go searches below the last byte, so "/" splits into (".", "/").
    let head = p.get(..p.len().saturating_sub(1)).unwrap_or_default();
    match head.iter().rposition(|&b| b == b'/') {
        Some(0) => (b"/", p.get(1..).unwrap_or_default()),
        Some(i) => (
            p.get(..i).unwrap_or_default(),
            p.get(i + 1..).unwrap_or_default(),
        ),
        None => (b".", p),
    }
}

/// Runs a libc call returning -1 on error, retrying EINTR (`ignoringEINTR`).
fn retry_eintr(mut f: impl FnMut() -> libc::c_int) -> io::Result<libc::c_int> {
    loop {
        let rc = f();
        if rc != -1 {
            return Ok(rc);
        }
        let e = io::Error::last_os_error();
        if !has_errno(&e, libc::EINTR) {
            return Err(e);
        }
    }
}

fn unlinkat(dirfd: RawFd, name: &CStr, flags: libc::c_int) -> io::Result<()> {
    // SAFETY: `name` is a valid NUL-terminated string; unlinkat does not retain it.
    retry_eintr(|| unsafe { libc::unlinkat(dirfd, name.as_ptr(), flags) }).map(|_| ())
}

/// `rootOpenDir`: openat with O_NOFOLLOW|O_DIRECTORY. `Err(Ok(link))` is Go's `errSymlink`.
fn open_dir_at(dirfd: RawFd, name: &CStr) -> Result<std::fs::File, Result<Vec<u8>, io::Error>> {
    let flags = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_DIRECTORY;
    // SAFETY: `name` is a valid NUL-terminated string; the returned fd is owned by the File below.
    match retry_eintr(|| unsafe { libc::openat(dirfd, name.as_ptr(), flags, 0) }) {
        // SAFETY: openat returned a fresh fd that nothing else owns.
        Ok(fd) => Ok(unsafe { std::fs::File::from_raw_fd(fd) }),
        Err(e)
            if has_errno(&e, libc::ELOOP)
                || has_errno(&e, libc::EMLINK)
                || has_errno(&e, libc::ENOTDIR) =>
        {
            // checkSymlink: a symlink becomes errSymlink, anything else keeps the original error.
            let mut buf = vec![0u8; 4096];
            // SAFETY: `buf` is valid for `buf.len()` bytes and `name` is NUL-terminated.
            let n = unsafe {
                libc::readlinkat(dirfd, name.as_ptr(), buf.as_mut_ptr().cast(), buf.len())
            };
            match usize::try_from(n) {
                Ok(n) => {
                    buf.truncate(n);
                    Err(Ok(buf))
                }
                Err(_) => Err(Err(e)),
            }
        }
        Err(e) if has_errno(&e, libc::ENOTSUP) || has_errno(&e, libc::EOPNOTSUPP) => {
            Err(Err(os_err(libc::ENOTDIR)))
        }
        Err(e) => Err(Err(e)),
    }
}

#[cfg(target_os = "macos")]
fn errno_location() -> *mut libc::c_int {
    // SAFETY: __error returns the calling thread's errno slot.
    unsafe { libc::__error() }
}

#[cfg(not(target_os = "macos"))]
fn errno_location() -> *mut libc::c_int {
    // SAFETY: __errno_location returns the calling thread's errno slot.
    unsafe { libc::__errno_location() }
}

/// The op name of Go's `*File.readdir` errors.
const READDIR_OP: &str = if cfg!(target_os = "macos") {
    "readdir"
} else {
    "readdirent"
};

/// The op name of an fdopendir failure: Go's darwin `poll.FD.OpenDir` reports "fdopendir"; Go's linux
/// readdir reads with getdents and has no such step, so its first `readdirent` is the nearest.
const OPENDIR_OP: &str = if cfg!(target_os = "macos") {
    "fdopendir"
} else {
    "readdirent"
};

/// `File.Readdirnames(n)` over an open directory stream: up to `n` names ("." and ".." skipped).
fn readdirnames(dir: *mut libc::DIR, n: usize, base: &[u8]) -> Result<Vec<Vec<u8>>, PathError> {
    let mut names = Vec::new();
    while names.len() < n {
        // SAFETY: errno_location points at this thread's errno.
        unsafe { *errno_location() = 0 };
        // SAFETY: `dir` is a live stream from fdopendir, used by this thread only.
        let ent = unsafe { libc::readdir(dir) };
        if ent.is_null() {
            let e = io::Error::last_os_error();
            if e.raw_os_error().is_some_and(|c| c != 0) {
                return Err(path_err(READDIR_OP, base, e));
            }
            break;
        }
        // Go skips entries with a zero inode (darwin: deleted but not yet removed; linux: direntIno).
        // SAFETY: readdir returned a valid entry.
        if unsafe { (*ent).d_ino } == 0 {
            continue;
        }
        // SAFETY: readdir returned a valid entry whose d_name is NUL-terminated.
        let name = unsafe { CStr::from_ptr((*ent).d_name.as_ptr()) }.to_bytes();
        if name != b"." && name != b".." {
            names.push(name.to_vec());
        }
    }
    Ok(names)
}

fn join_err_path(prefix: &[u8], e: &mut PathError) {
    let mut p = prefix.to_vec();
    p.push(b'/');
    p.extend_from_slice(&e.path);
    e.path = p;
}

/// `removeAllFrom`.
fn remove_all_from(parent_fd: RawFd, base: &[u8]) -> Result<(), PathError> {
    let cbase = match cstring(base) {
        Ok(c) => c,
        Err(e) => return Err(path_err("unlinkat", base, e)),
    };
    let u_err = match unlinkat(parent_fd, &cbase, 0) {
        Ok(()) => return Ok(()),
        Err(e) if is_not_exist(&e) => return Ok(()),
        Err(e)
            if has_errno(&e, libc::EISDIR)
                || has_errno(&e, libc::EPERM)
                || has_errno(&e, libc::EACCES) =>
        {
            e
        }
        Err(e) => return Err(path_err("unlinkat", base, e)),
    };
    const REQ_SIZE: usize = 1024;
    let mut recurse_err: Option<PathError> = None;
    loop {
        let file = match open_dir_at(parent_fd, &cbase) {
            Ok(f) => f,
            Err(Err(e)) if is_not_exist(&e) => return Ok(()),
            Err(Err(e)) if has_errno(&e, libc::ENOTDIR) => {
                return Err(path_err("unlinkat", base, u_err));
            }
            Err(Ok(_symlink)) => {
                recurse_err = Some(path_err(
                    "openfdat",
                    base,
                    os_err(u_err.raw_os_error().unwrap_or(libc::EISDIR)),
                ));
                break;
            }
            Err(Err(e)) => {
                recurse_err = Some(path_err("openfdat", base, e));
                break;
            }
        };
        let fd = file.into_raw_fd();
        // SAFETY: `fd` is an open directory fd; fdopendir takes ownership and closedir closes it.
        let dir = unsafe { libc::fdopendir(fd) };
        if dir.is_null() {
            let e = io::Error::last_os_error();
            // SAFETY: fdopendir failed, so `fd` is still ours to close.
            unsafe { libc::close(fd) };
            return Err(path_err(
                "readdirnames",
                base,
                io::Error::new(e.kind(), path_err(OPENDIR_OP, base, e)),
            ));
        }
        let dir_fd = fd;
        let mut resp_size;
        loop {
            let names = match readdirnames(dir, REQ_SIZE, base) {
                Ok(names) => names,
                Err(read_err) => {
                    // SAFETY: `dir` is live and closed exactly once here.
                    unsafe { libc::closedir(dir) };
                    if is_not_exist(&read_err.err) {
                        return Ok(());
                    }
                    let kind = read_err.err.kind();
                    return Err(path_err(
                        "readdirnames",
                        base,
                        io::Error::new(kind, read_err),
                    ));
                }
            };
            resp_size = names.len();
            let mut num_err = 0usize;
            for name in &names {
                if let Err(mut e) = remove_all_from(dir_fd, name) {
                    join_err_path(base, &mut e);
                    num_err += 1;
                    if recurse_err.is_none() {
                        recurse_err = Some(e);
                    }
                }
            }
            // If any entry could be deleted, start a new iteration.
            if num_err != REQ_SIZE {
                break;
            }
        }
        // SAFETY: `dir` is live and closed exactly once here.
        unsafe { libc::closedir(dir) };
        if resp_size < REQ_SIZE {
            break;
        }
    }
    match unlinkat(parent_fd, &cbase, libc::AT_REMOVEDIR) {
        Ok(()) => Ok(()),
        Err(e) if is_not_exist(&e) => Ok(()),
        Err(e) => Err(recurse_err.unwrap_or_else(|| path_err("unlinkat", base, e))),
    }
}

/// `os.RemoveAll`.
pub fn remove_all(path: &[u8]) -> Result<(), PathError> {
    if path.is_empty() {
        return Ok(());
    }
    if ends_with_dot(path) {
        return Err(path_err("RemoveAll", path, os_err(libc::EINVAL)));
    }
    match remove(path) {
        Ok(()) => return Ok(()),
        Err(e) if is_not_exist(&e.err) => return Ok(()),
        Err(_) => {}
    }
    let (parent_dir, base) = split_path(path);
    let parent = match cstring(parent_dir).and_then(|_| std::fs::File::open(to_path(parent_dir))) {
        Ok(f) => f,
        Err(e) if is_not_exist(&e) => return Ok(()),
        Err(e) => return Err(path_err("open", parent_dir, e)),
    };
    remove_all_from(parent.as_raw_fd(), base).map_err(|mut e| {
        join_err_path(parent_dir, &mut e);
        e
    })
}

/// `os.TempDir`: `$TMPDIR`, else "/tmp".
fn temp_dir() -> Vec<u8> {
    match std::env::var_os("TMPDIR") {
        Some(d) if !d.is_empty() => d.as_bytes().to_vec(),
        _ => b"/tmp".to_vec(),
    }
}

/// `runtime_rand()` truncated to 32 bits.
fn next_random() -> u32 {
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
    if let Ok(d) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        h.write_u128(d.as_nanos());
    }
    h.finish() as u32
}

/// `os.CreateTemp` (patterns such as ".dstore-tmp-*"): decimal u32 names, O_EXCL 0600, 10000 tries.
pub fn create_temp(dir: &[u8], pattern: &str) -> Result<(std::fs::File, Vec<u8>), PathError> {
    let dir = if dir.is_empty() {
        temp_dir()
    } else {
        dir.to_vec()
    };
    if pattern.contains('/') {
        let err = io::Error::new(
            io::ErrorKind::InvalidInput,
            "pattern contains path separator",
        );
        return Err(path_err("createtemp", pattern.as_bytes(), err));
    }
    let (prefix, suffix) = match pattern.rfind('*') {
        Some(pos) => (
            pattern.get(..pos).unwrap_or(""),
            pattern.get(pos + 1..).unwrap_or(""),
        ),
        None => (pattern, ""),
    };
    let mut base = dir;
    if !base.ends_with(b"/") {
        base.push(b'/');
    }
    base.extend_from_slice(prefix.as_bytes());
    let mut tries = 0u32;
    loop {
        let mut name = base.clone();
        name.extend_from_slice(next_random().to_string().as_bytes());
        name.extend_from_slice(suffix.as_bytes());
        let opened = cstring(&name).and_then(|_| {
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(to_path(&name))
        });
        match opened {
            Ok(f) => return Ok((f, name)),
            Err(e) if has_errno(&e, libc::EEXIST) || has_errno(&e, libc::ENOTEMPTY) => {
                tries += 1;
                if tries < 10000 {
                    continue;
                }
                let mut shown = base;
                shown.push(b'*');
                shown.extend_from_slice(suffix.as_bytes());
                let err = io::Error::new(io::ErrorKind::AlreadyExists, "file already exists");
                return Err(PathError {
                    op: "createtemp",
                    path: shown,
                    err,
                });
            }
            Err(e) => {
                return Err(PathError {
                    op: "open",
                    path: name,
                    err: e,
                });
            }
        }
    }
}

/// `File.Close` with its error (Rust's drop ignores it).
fn close_file(f: std::fs::File) -> io::Result<()> {
    let fd = f.into_raw_fd();
    // SAFETY: `fd` was owned by the File and is closed exactly once.
    if unsafe { libc::close(fd) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// `os.WriteFile`: O_WRONLY|O_CREATE|O_TRUNC.
pub fn write_file(path: &[u8], data: &[u8], mode: u32) -> Result<(), PathError> {
    let mut f = cstring(path)
        .and_then(|_| {
            std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(mode & 0o777)
                .open(to_path(path))
        })
        .map_err(|e| path_err("open", path, e))?;
    let written = f.write_all(data).map_err(|e| path_err("write", path, e));
    let closed = close_file(f).map_err(|e| path_err("close", path, e));
    written.and(closed)
}

/// `os.ReadFile`.
pub fn read_file(path: &[u8]) -> Result<Vec<u8>, PathError> {
    let mut f = cstring(path)
        .and_then(|_| std::fs::File::open(to_path(path)))
        .map_err(|e| path_err("open", path, e))?;
    let mut data = Vec::new();
    f.read_to_end(&mut data)
        .map_err(|e| path_err("read", path, e))?;
    Ok(data)
}

/// `user.Current().Username` without cgo: `getpwuid_r(getuid())` on darwin; on linux `/etc/passwd`, else
/// `$USER` when `$USER` and `$HOME` are set. The error texts are Go's.
pub fn current_username() -> Result<String, String> {
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() };
    current_username_for(uid)
}

#[cfg(target_os = "macos")]
fn current_username_for(uid: libc::uid_t) -> Result<String, String> {
    const MAX_BUFFER_SIZE: usize = 1 << 20;
    // SAFETY: sysconf has no preconditions.
    let initial = unsafe { libc::sysconf(libc::_SC_GETPW_R_SIZE_MAX) };
    // bufferKind.initialSize: -1 → 1024; any other size outside (0, maxBufferSize] → maxBufferSize.
    let mut size = match usize::try_from(initial) {
        _ if initial == -1 => 1024,
        Ok(sz) if sz > 0 && sz <= MAX_BUFFER_SIZE => sz,
        _ => MAX_BUFFER_SIZE,
    };
    loop {
        let mut buf = vec![0 as libc::c_char; size];
        // SAFETY: an all-zero passwd is a valid out-parameter value.
        let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
        let mut result: *mut libc::passwd = std::ptr::null_mut();
        // SAFETY: every pointer is valid for the call; `buf` outlives the use of `pwd` below.
        let errno =
            unsafe { libc::getpwuid_r(uid, &mut pwd, buf.as_mut_ptr(), buf.len(), &mut result) };
        if errno == libc::ERANGE {
            size *= 2;
            if size > MAX_BUFFER_SIZE {
                return Err(format!(
                    "user: lookup userid {uid}: internal buffer exceeds {MAX_BUFFER_SIZE} bytes"
                ));
            }
            continue;
        }
        if errno == libc::ENOENT || (errno == 0 && result.is_null()) {
            return Err(format!("user: unknown userid {uid}"));
        }
        if errno != 0 {
            return Err(format!(
                "user: lookup userid {uid}: {}",
                crate::errno::errno_string(errno)
            ));
        }
        if pwd.pw_name.is_null() {
            return Ok(String::new());
        }
        // SAFETY: getpwuid_r succeeded, so pw_name points to a NUL-terminated string inside `buf`.
        let name = unsafe { CStr::from_ptr(pwd.pw_name) };
        return Ok(String::from_utf8_lossy(name.to_bytes()).into_owned());
    }
}

#[cfg(not(target_os = "macos"))]
fn current_username_for(uid: libc::uid_t) -> Result<String, String> {
    let uid = uid.to_string();
    if let Ok(content) = std::fs::read("/etc/passwd")
        && let Some(name) = passwd_username(&content, &uid)
    {
        return Ok(String::from_utf8_lossy(&name).into_owned());
    }
    let user = std::env::var_os("USER").unwrap_or_default();
    let home = std::env::var_os("HOME").unwrap_or_default();
    if !user.is_empty() && !home.is_empty() {
        return Ok(String::from_utf8_lossy(user.as_bytes()).into_owned());
    }
    let mut missing = String::new();
    if user.is_empty() {
        missing.push_str("$USER");
    }
    if home.is_empty() {
        if !missing.is_empty() {
            missing.push_str(", ");
        }
        missing.push_str("$HOME");
    }
    Err(format!(
        "user: Current requires cgo or {missing} set in environment"
    ))
}

/// `strconv.Atoi` acceptance: an optional sign and decimal digits that fit in an i64.
#[cfg_attr(target_os = "macos", allow(dead_code))]
fn atoi_ok(s: &[u8]) -> bool {
    std::str::from_utf8(s).is_ok_and(|s| {
        let digits = s.strip_prefix(['+', '-']).unwrap_or(s);
        !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) && s.parse::<i64>().is_ok()
    })
}

/// `findUserId` over `/etc/passwd` content (`matchUserIndexValue(uid, 2)`).
#[cfg_attr(target_os = "macos", allow(dead_code))]
fn passwd_username(content: &[u8], uid: &str) -> Option<Vec<u8>> {
    let needle = format!(":{uid}:");
    for raw in content.split(|&b| b == b'\n') {
        // bytes.TrimSpace: ASCII \t \n \v \f \r and space, and Unicode White_Space.
        let line = crate::strings::trim_space(raw);
        if line.is_empty() || line.first() == Some(&b'#') {
            continue;
        }
        if !line.windows(needle.len()).any(|w| w == needle.as_bytes())
            || line.iter().filter(|&&b| b == b':').count() < 6
        {
            continue;
        }
        let parts: Vec<&[u8]> = line.splitn(7, |&b| b == b':').collect();
        let (Some(name), Some(p2), Some(p3)) = (parts.first(), parts.get(2), parts.get(3)) else {
            continue;
        };
        if parts.len() < 6
            || *p2 != uid.as_bytes()
            || name.is_empty()
            || name.starts_with(b"+")
            || name.starts_with(b"-")
        {
            continue;
        }
        if !atoi_ok(p2) || !atoi_ok(p3) {
            continue;
        }
        return Some(name.to_vec());
    }
    None
}

/// cmd/dstore `isTerminal`: fstat `S_ISCHR`.
pub fn is_char_device(fd: RawFd) -> bool {
    // SAFETY: an all-zero stat is a valid out-parameter value.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: `st` is valid for writes; fstat on an invalid fd only returns EBADF.
    if unsafe { libc::fstat(fd, &mut st) } != 0 {
        return false;
    }
    (st.st_mode & libc::S_IFMT) == libc::S_IFCHR
}

/// `os.Geteuid`.
pub fn geteuid() -> u32 {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A fresh directory under the system temp dir, removed on drop.
    struct Scratch(Vec<u8>);

    impl Scratch {
        fn new(tag: &str) -> Scratch {
            use std::sync::atomic::{AtomicU32, Ordering};
            static N: AtomicU32 = AtomicU32::new(0);
            let n = N.fetch_add(1, Ordering::Relaxed);
            let dir =
                std::env::temp_dir().join(format!("gocompat-os-{}-{tag}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            if let Err(e) = std::fs::create_dir_all(&dir) {
                panic!("scratch dir: {e}");
            }
            Scratch(from_path(&dir))
        }
        fn join(&self, rel: &str) -> Vec<u8> {
            let mut p = self.0.clone();
            p.push(b'/');
            p.extend_from_slice(rel.as_bytes());
            p
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(to_path(&self.0));
        }
    }

    fn text<T: std::fmt::Debug>(r: Result<T, PathError>) -> String {
        match r {
            Ok(v) => format!("ok {v:?}"),
            Err(e) => e.to_string(),
        }
    }

    fn show(p: &[u8]) -> String {
        String::from_utf8_lossy(p).into_owned()
    }

    #[test]
    fn mkdir_all_and_remove() {
        let s = Scratch::new("mkdir");
        let deep = s.join("a/b/c/");
        assert!(mkdir_all(&deep, 0o755).is_ok());
        assert!(stat(&s.join("a/b/c")).is_ok_and(|m| m.is_dir()));
        assert!(mkdir_all(&s.join("a/b/."), 0o755).is_ok());
        assert!(write_file(&s.join("f"), b"x", 0o644).is_ok());
        assert_eq!(
            text(mkdir_all(&s.join("f"), 0o755)),
            format!("mkdir {}: not a directory", show(&s.join("f")))
        );
        // Go recurses into MkdirAll("f"), which reports the file.
        assert_eq!(
            text(mkdir_all(&s.join("f/g"), 0o755)),
            format!("mkdir {}: not a directory", show(&s.join("f")))
        );
        assert_eq!(
            text(mkdir_all(b"", 0o755)),
            "mkdir : no such file or directory"
        );
        assert_eq!(
            text(mkdir_all(b"a\0b", 0o755)),
            "mkdir a\0b: invalid argument"
        );

        assert_eq!(
            text(remove(&s.join("a/b"))),
            format!("remove {}: directory not empty", show(&s.join("a/b")))
        );
        assert_eq!(
            text(remove(&s.join("missing"))),
            format!(
                "remove {}: no such file or directory",
                show(&s.join("missing"))
            )
        );
        assert_eq!(
            text(remove(&s.join("f/x"))),
            format!("remove {}: not a directory", show(&s.join("f/x")))
        );
        assert!(remove(&s.join("a/b/c")).is_ok());
        assert!(remove(&s.join("f")).is_ok());
    }

    #[test]
    fn remove_all_trees() {
        let s = Scratch::new("removeall");
        assert!(mkdir_all(&s.join("t/x/y"), 0o755).is_ok());
        for i in 0..1100 {
            assert!(write_file(&s.join(&format!("t/x/f{i}")), b"", 0o644).is_ok());
        }
        assert!(std::os::unix::fs::symlink("/", to_path(&s.join("t/link"))).is_ok());
        assert!(remove_all(&s.join("t")).is_ok());
        assert!(lstat(&s.join("t")).is_err());
        assert!(remove_all(&s.join("t")).is_ok());
        assert!(remove_all(b"").is_ok());
        assert_eq!(text(remove_all(b".")), "RemoveAll .: invalid argument");
        assert_eq!(text(remove_all(b"a/.")), "RemoveAll a/.: invalid argument");
        assert_eq!(split_path(b"//a//b//"), (&b"/a/"[..], &b"b"[..]));
        assert_eq!(split_path(b"/"), (&b"."[..], &b"/"[..]));
        assert_eq!(split_path(b"a/b"), (&b"a"[..], &b"b"[..]));
        assert_eq!(split_path(b"/a"), (&b"/"[..], &b"a"[..]));
        assert_eq!(split_path(b"a"), (&b"."[..], &b"a"[..]));
    }

    #[test]
    fn create_temp_names() {
        let s = Scratch::new("temp");
        let (f, name) = match create_temp(&s.0, ".dstore-tmp-*") {
            Ok(v) => v,
            Err(e) => panic!("create_temp: {e}"),
        };
        drop(f);
        let prefix = s.join(".dstore-tmp-");
        assert!(name.starts_with(&prefix));
        let digits = &name[prefix.len()..];
        assert!(
            !digits.is_empty() && digits.iter().all(u8::is_ascii_digit),
            "{}",
            show(&name)
        );
        assert!(std::str::from_utf8(digits).is_ok_and(|d| d.parse::<u32>().is_ok()));
        assert!(stat(&name).is_ok_and(|m| m.permissions().mode() & 0o777 == 0o600));
        let (_, with_suffix) = match create_temp(&s.0, "a*b*.tmp") {
            Ok(v) => v,
            Err(e) => panic!("create_temp: {e}"),
        };
        assert!(with_suffix.starts_with(&s.join("a*b")) && with_suffix.ends_with(b".tmp"));
        assert_eq!(
            text(create_temp(&s.0, "x/*")),
            "createtemp x/*: pattern contains path separator"
        );
        assert_eq!(
            text(create_temp(&s.join("nodir"), "p*"))
                .split(": ")
                .last()
                .map(str::to_string),
            Some("no such file or directory".to_string())
        );
    }

    #[test]
    fn write_and_read_files() {
        let s = Scratch::new("rw");
        let p = s.join("data");
        assert!(write_file(&p, b"hello", 0o600).is_ok());
        assert!(write_file(&p, b"hi", 0o600).is_ok());
        assert_eq!(read_file(&p).ok(), Some(b"hi".to_vec()));
        assert_eq!(
            text(read_file(&s.join("none"))),
            format!("open {}: no such file or directory", show(&s.join("none")))
        );
        assert_eq!(
            text(read_file(&s.0)),
            format!("read {}: is a directory", show(&s.0))
        );
        assert_eq!(
            text(write_file(&s.0, b"", 0o644)),
            format!("open {}: is a directory", show(&s.0))
        );
    }

    #[test]
    fn getwd_is_absolute_and_dot() {
        let wd = match getwd() {
            Ok(wd) => wd,
            Err(e) => panic!("getwd: {e}"),
        };
        assert_eq!(wd.first(), Some(&b'/'));
        let (a, b) = match (stat(&wd), std::fs::metadata(".")) {
            (Ok(a), Ok(b)) => (a, b),
            other => panic!("stat: {other:?}"),
        };
        assert_eq!((a.dev(), a.ino()), (b.dev(), b.ino()));
    }

    #[test]
    fn char_devices_and_ids() {
        let null = match std::fs::File::open("/dev/null") {
            Ok(f) => f,
            Err(e) => panic!("/dev/null: {e}"),
        };
        assert!(is_char_device(null.as_raw_fd()));
        let s = Scratch::new("chr");
        assert!(write_file(&s.join("f"), b"", 0o644).is_ok());
        let f = match std::fs::File::open(to_path(&s.join("f"))) {
            Ok(f) => f,
            Err(e) => panic!("open: {e}"),
        };
        assert!(!is_char_device(f.as_raw_fd()));
        assert!(!is_char_device(-1));
        // SAFETY: geteuid has no preconditions.
        assert_eq!(geteuid(), unsafe { libc::geteuid() });
    }

    #[test]
    fn username_lookup() {
        match current_username() {
            Ok(name) => assert!(!name.is_empty()),
            Err(e) => assert!(e.starts_with("user: "), "{e}"),
        }
    }

    #[test]
    fn passwd_parsing() {
        let content = b"# comment\n\n +x:x:1000:1000::/h:/bin/sh\nbad:x:1000:1000\nwrong:x:1000:gid::/h:/bin/sh\n  kevin:x:1000:1006:Kevin,,,:/home/kevin:/usr/bin/zsh  \nlater:x:1000:1000::/h:/bin/sh\n";
        assert_eq!(passwd_username(content, "1000"), Some(b"kevin".to_vec()));
        assert_eq!(passwd_username(content, "7"), None);
        // bytes.TrimSpace also strips \v and Unicode White_Space (U+00A0, U+2003), then '#' comments count.
        let spaced =
            "\u{b}\u{a0}ann:x:42:42::/h:/bin/sh\u{2003}\r\n\u{85}#bob:x:43:43::/h:/bin/sh\n"
                .as_bytes();
        assert_eq!(passwd_username(spaced, "42"), Some(b"ann".to_vec()));
        assert_eq!(passwd_username(spaced, "43"), None);
        assert!(
            atoi_ok(b"-5")
                && atoi_ok(b"+5")
                && !atoi_ok(b"")
                && !atoi_ok(b"1a")
                && !atoi_ok(b"99999999999999999999")
        );
    }
}
