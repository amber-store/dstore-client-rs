//! Go `syscall` errno strings (darwin, linux), Go's rendering of I/O errors and `*fs.PathError`.

use crate::errno_tables::ERRORS;

/// The Go `syscall` error text of an errno for `cfg(target_os)`, when Go's table has one.
pub fn errno_text(code: i32) -> Option<&'static str> {
    let i = ERRORS.binary_search_by_key(&code, |&(c, _)| c).ok()?;
    ERRORS
        .get(i)
        .map(|&(_, text)| text)
        .filter(|t| !t.is_empty())
}

/// `syscall.Errno(code).Error()`: the table text, else `errno <code>`.
pub fn errno_string(code: i32) -> String {
    match errno_text(code) {
        Some(text) => text.to_string(),
        None => format!("errno {code}"),
    }
}

/// A raw OS error → Go text; `UnexpectedEof` → "unexpected EOF"; else `e.to_string()`.
pub fn io_error_text(e: &std::io::Error) -> String {
    if let Some(code) = e.raw_os_error() {
        return errno_string(code);
    }
    if e.kind() == std::io::ErrorKind::UnexpectedEof {
        return "unexpected EOF".to_string();
    }
    e.to_string()
}

/// Rewrites every "<Rust strerror> (os error N)" in `msg` to Go's text (PORTING.md §5.2). An occurrence
/// whose text before " (os error N)" is not Rust's rendering of that errno is left unchanged.
pub fn rewrite_os_errors(msg: &str) -> String {
    const MARK: &str = " (os error ";
    let mut out = String::with_capacity(msg.len());
    let mut cursor = 0usize; // bytes of msg already copied to out
    let mut search = 0usize;
    while let Some(rel) = msg.get(search..).and_then(|rest| rest.find(MARK)) {
        let digits = search + rel + MARK.len();
        let tail = msg.get(digits..).unwrap_or("");
        let num_len = tail
            .bytes()
            .enumerate()
            .take_while(|&(i, b)| b.is_ascii_digit() || (i == 0 && b == b'-'))
            .count();
        let close = digits + num_len;
        search = digits;
        if !tail.get(num_len..).is_some_and(|t| t.starts_with(')')) {
            continue;
        }
        let Some(code) = msg.get(digits..close).and_then(|n| n.parse::<i32>().ok()) else {
            continue;
        };
        let end = close + 1;
        let rust = std::io::Error::from_raw_os_error(code).to_string();
        let Some(start) = end.checked_sub(rust.len()) else {
            continue;
        };
        if start < cursor || msg.get(start..end) != Some(rust.as_str()) {
            continue;
        }
        out.push_str(msg.get(cursor..start).unwrap_or(""));
        out.push_str(&errno_string(code));
        cursor = end;
        search = end;
    }
    out.push_str(msg.get(cursor..).unwrap_or(""));
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_errno_texts() {
        assert_eq!(errno_text(libc::ENOENT), Some("no such file or directory"));
        assert_eq!(errno_text(libc::EACCES), Some("permission denied"));
        assert_eq!(
            errno_text(libc::EAGAIN),
            Some("resource temporarily unavailable")
        );
        assert_eq!(errno_text(libc::ENOTDIR), Some("not a directory"));
        assert_eq!(errno_text(libc::ENOTEMPTY), Some("directory not empty"));
        assert_eq!(errno_text(libc::EEXIST), Some("file exists"));
        assert_eq!(errno_text(libc::EINVAL), Some("invalid argument"));
        assert_eq!(errno_text(0), None);
        assert_eq!(errno_text(-1), None);
        assert_eq!(errno_string(0), "errno 0");
        assert_eq!(errno_string(99999), "errno 99999");
        assert_eq!(errno_string(-3), "errno -3");
    }

    #[test]
    fn tables_are_sorted_and_unique() {
        assert!(!ERRORS.is_empty());
        assert!(ERRORS.windows(2).all(|w| w[0].0 < w[1].0));
    }

    #[test]
    fn io_error_texts() {
        let e = std::io::Error::from_raw_os_error(libc::ENOENT);
        assert_eq!(io_error_text(&e), "no such file or directory");
        let e = std::io::Error::from(std::io::ErrorKind::UnexpectedEof);
        assert_eq!(io_error_text(&e), "unexpected EOF");
        let e = std::io::Error::other("pattern contains path separator");
        assert_eq!(io_error_text(&e), "pattern contains path separator");
    }

    #[test]
    fn rewrite() {
        let enoent = std::io::Error::from_raw_os_error(libc::ENOENT).to_string();
        let eacces = std::io::Error::from_raw_os_error(libc::EACCES).to_string();
        assert_eq!(
            rewrite_os_errors(&format!("open /x: {enoent}")),
            "open /x: no such file or directory"
        );
        assert_eq!(
            rewrite_os_errors(&format!("a: {enoent}\nb: {eacces} tail")),
            "a: no such file or directory\nb: permission denied tail"
        );
        // Not Rust's rendering of that errno: unchanged.
        let odd = format!("weird text (os error {})", libc::ENOENT);
        assert_eq!(rewrite_os_errors(&odd), odd);
        for s in [
            "",
            "no marker",
            " (os error )",
            " (os error 2",
            " (os error x)",
        ] {
            assert_eq!(rewrite_os_errors(s), s);
        }
        let unknown = std::io::Error::from_raw_os_error(99999).to_string();
        assert_eq!(
            rewrite_os_errors(&format!("x: {unknown}")),
            "x: errno 99999"
        );
        assert_eq!(rewrite_os_errors("é (os error 2)é"), "é (os error 2)é");
    }

    #[test]
    fn path_error_display() {
        let e = PathError {
            op: "remove",
            path: b"a/b".to_vec(),
            err: std::io::Error::from_raw_os_error(libc::ENOTEMPTY),
        };
        assert_eq!(e.to_string(), "remove a/b: directory not empty");
        assert!(std::error::Error::source(&e).is_some());
    }
}
