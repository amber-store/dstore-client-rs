//! Go `syscall` errno strings (darwin, linux), Go's rendering of I/O errors and `*fs.PathError`.

/// The Go `syscall` error text of an errno for `cfg(target_os)`.
pub fn errno_text(code: i32) -> Option<&'static str> {
    todo!()
}

/// A raw OS error → Go text; `UnexpectedEof` → "unexpected EOF"; else `e.to_string()`.
pub fn io_error_text(e: &std::io::Error) -> String {
    todo!()
}

/// Rewrites every "<Rust strerror> (os error N)" in `msg` to Go's text (PORTING.md §5.2).
pub fn rewrite_os_errors(msg: &str) -> String {
    todo!()
}

/// `*fs.PathError`: "<op> <path>: <errno text>".
#[derive(Debug, thiserror::Error)]
#[error("{op} {}: {}", String::from_utf8_lossy(.path), io_error_text(.err))]
pub struct PathError {
    pub op: &'static str,
    pub path: Vec<u8>,
    #[source]
    pub err: std::io::Error,
}
