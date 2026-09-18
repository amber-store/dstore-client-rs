//! `path` and `path/filepath` on Unix, over Go string bytes.

/// `filepath.Clean`.
pub fn clean(p: &[u8]) -> Vec<u8> {
    todo!()
}

/// `filepath.Join`.
pub fn join(elems: &[&[u8]]) -> Vec<u8> {
    todo!()
}

/// `filepath.Dir`.
pub fn dir(p: &[u8]) -> Vec<u8> {
    todo!()
}

/// `path.Base`: "" → ".", all slashes → "/".
pub fn base(p: &[u8]) -> Vec<u8> {
    todo!()
}

/// `filepath.Abs`: `Clean(Join(getwd(), p))` when relative.
pub fn abs(p: &[u8]) -> std::io::Result<Vec<u8>> {
    todo!()
}

/// `filepath.Rel`, lexical; None where Go returns an error.
pub fn rel(base: &[u8], target: &[u8]) -> Option<Vec<u8>> {
    todo!()
}

/// Bytes → `PathBuf` (Unix, no conversion).
pub fn to_path(b: &[u8]) -> std::path::PathBuf {
    todo!()
}

/// `Path` → bytes (Unix, no conversion).
pub fn from_path(p: &std::path::Path) -> Vec<u8> {
    todo!()
}
