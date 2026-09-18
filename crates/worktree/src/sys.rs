//! `x/sys/unix` equivalents (core-rs-gaps §4.3): xattrs, device numbers and mtimes.

/// macOS follows symlinks, Linux uses the l* calls.
pub fn read_xattrs(
    path: &std::path::Path,
) -> std::io::Result<std::collections::BTreeMap<Vec<u8>, Vec<u8>>> {
    todo!()
}

pub fn set_xattr(path: &std::path::Path, name: &[u8], value: &[u8]) -> std::io::Result<()> {
    todo!()
}

pub fn major(dev: u64) -> u32 {
    todo!()
}

pub fn minor(dev: u64) -> u32 {
    todo!()
}

pub fn mkdev(major: u32, minor: u32) -> u64 {
    todo!()
}

/// Wrapping mtime*1e9 + nsec.
pub fn unix_nano(md: &std::fs::Metadata) -> i64 {
    todo!()
}
