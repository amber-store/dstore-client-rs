//! Go `syscall` errno tables (`zerrors_darwin_*.go`, `zerrors_linux_*.go` of go1.26.5), one per
//! `cfg(target_os)`, used by `errno::errno_text`.
//!
//! L0 placeholder: the gocompat owner fills the tables.

/// `(errno, Go text)` pairs for this target.
#[cfg(target_os = "macos")]
pub(crate) static ERRORS: &[(i32, &str)] = &[];

/// `(errno, Go text)` pairs for this target.
#[cfg(target_os = "linux")]
pub(crate) static ERRORS: &[(i32, &str)] = &[];
