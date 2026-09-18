//! `os` functions dstore uses, with Go's error selection and texts, and cgo-less `os/user.Current`.

use crate::errno::PathError;

/// `os.Getwd`: `$PWD` if absolute and the same (dev, ino) as ".".
pub fn getwd() -> std::io::Result<Vec<u8>> {
    todo!()
}

/// `os.MkdirAll`.
pub fn mkdir_all(path: &[u8], mode: u32) -> Result<(), PathError> {
    todo!()
}

/// `os.Remove`: unlink, then rmdir; error selection as Go.
pub fn remove(path: &[u8]) -> Result<(), PathError> {
    todo!()
}

/// `os.RemoveAll`.
pub fn remove_all(path: &[u8]) -> Result<(), PathError> {
    todo!()
}

/// `os.CreateTemp` (patterns such as ".dstore-tmp-*"): decimal u32 names, O_EXCL 0600, 10000 tries.
pub fn create_temp(dir: &[u8], pattern: &str) -> Result<(std::fs::File, Vec<u8>), PathError> {
    todo!()
}

/// `os.WriteFile`: O_WRONLY|O_CREATE|O_TRUNC.
pub fn write_file(path: &[u8], data: &[u8], mode: u32) -> Result<(), PathError> {
    todo!()
}

/// `os.ReadFile`.
pub fn read_file(path: &[u8]) -> Result<Vec<u8>, PathError> {
    todo!()
}

/// `user.Current().Username` without cgo: `getpwuid_r(getuid())`, else `$USER` when `$USER` and
/// `$HOME` are set.
pub fn current_username() -> Result<String, String> {
    todo!()
}

/// cmd/dstore `isTerminal`: fstat `S_ISCHR`.
pub fn is_char_device(fd: std::os::fd::RawFd) -> bool {
    todo!()
}

/// `os.Geteuid`.
pub fn geteuid() -> u32 {
    todo!()
}
