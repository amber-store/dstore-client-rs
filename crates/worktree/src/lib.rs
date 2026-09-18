//! dstore `worktree` (`tree.go`, `change.go`, `scan.go`, `merge.go`, `apply.go`, `diff.go`, `flow.go`,
//! `xattr*.go`). `TicketFromView` lives in `dstore-view` and is re-exported here.
//!
//! Paths and `Config` fields are bytes. Scan, `diff_trees`, apply, ingest and `hash_file` are blocking;
//! callers wrap them in `spawn_blocking`.
//!
//! Spec: PORTING.md §4.10; port-notes/worktree.md.
#![deny(unsafe_op_in_unsafe_fn)]

pub use dstore_view::ticket_from_view;

/// The working-copy metadata directory.
pub const DIR: &str = ".dstore";
pub const RACY_WINDOW_NS: i64 = 2_000_000_000;
pub const MAX_DIFF_BYTES: u64 = 16 << 20;

mod apply;
mod change;
mod diff;
mod error;
// L0 stubs until worktree-flow lands: remove this allow with them.
#[allow(dead_code, unused_variables)]
mod flow;
mod merge;
mod scan;
pub mod sys;
mod tree;

pub use apply::*;
pub use change::*;
pub use diff::*;
pub use error::*;
pub use flow::*;
pub use merge::*;
pub use scan::*;
pub use tree::*;

/// Helpers shared by the unit tests (ports of the Go tests' `openStore`, `writeFile`, `ingestDir`, `kinds`).
#[cfg(test)]
pub(crate) mod testutil {
    use std::collections::BTreeMap;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use amber_store_core::key::Key;
    use amber_store_core::{ingest, packstore};

    use crate::{Change, Kind};

    /// A fresh directory under the system temp dir, removed on drop after giving the owner rwx on every
    /// directory below it (Go `t.TempDir` plus the tests' chmod cleanups).
    pub struct Scratch(PathBuf);

    impl Scratch {
        pub fn new() -> Scratch {
            static N: AtomicU64 = AtomicU64::new(0);
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default();
            let name = format!(
                "dstore-worktree-test-{}-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed),
                nanos
            );
            let dir = std::env::temp_dir().join(name);
            std::fs::create_dir(&dir).expect("create scratch dir");
            Scratch(dir)
        }

        pub fn path(&self) -> PathBuf {
            self.0.clone()
        }

        pub fn bytes(&self) -> Vec<u8> {
            self.0.as_os_str().as_bytes().to_vec()
        }

        pub fn join(&self, rel: &str) -> PathBuf {
            self.0.join(rel)
        }

        pub fn join_bytes(&self, rel: &str) -> Vec<u8> {
            self.join(rel).as_os_str().as_bytes().to_vec()
        }
    }

    fn open_up(p: &Path) {
        if let Ok(md) = std::fs::symlink_metadata(p)
            && md.is_dir()
        {
            let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700));
            if let Ok(rd) = std::fs::read_dir(p) {
                for de in rd.flatten() {
                    open_up(&de.path());
                }
            }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            open_up(&self.0);
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// `openStore`: a packstore without sync in its own scratch directory.
    pub fn open_store() -> (Scratch, packstore::Store) {
        let dir = Scratch::new();
        let st = packstore::Store::open_with(dir.join("ps"), packstore::Options::new().sync(false))
            .expect("open packstore");
        (dir, st)
    }

    /// `writeFile`: parents created, content written, mode set exactly.
    pub fn write_file(p: &Path, content: &str, mode: u32) {
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("mkdir parents");
        }
        std::fs::write(p, content).expect("write file");
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).expect("chmod");
    }

    pub fn symlink(target: &str, p: &Path) {
        std::os::unix::fs::symlink(target, p).expect("symlink");
    }

    /// `os.Chtimes(p, t, t)`.
    pub fn set_mtime(p: &Path, secs: i64, nsec: u32) {
        let ns = i128::from(secs) * 1_000_000_000 + i128::from(nsec);
        let t = std::time::UNIX_EPOCH
            + std::time::Duration::from_nanos(u64::try_from(ns).expect("post-1970 time"));
        let f = std::fs::File::open(p).expect("open for times");
        f.set_times(std::fs::FileTimes::new().set_accessed(t).set_modified(t))
            .expect("set times");
    }

    /// `ingestDir`: `ingest.Dir(st, dir, Opts{Jobs: 2})`.
    pub fn ingest_dir(st: &packstore::Store, dir: &Path) -> Key {
        let opts = ingest::Opts {
            jobs: 2,
            ..Default::default()
        };
        ingest::dir(st, dir, opts).1.expect("ingest")
    }

    /// `kinds`: path → kind.
    pub fn kinds(changes: &[Change]) -> BTreeMap<Vec<u8>, Kind> {
        changes.iter().map(|c| (c.path.clone(), c.kind)).collect()
    }
}
