//! Lock-interop helper for interop check D12: opens the working copy `DIR`, which takes its lock
//! (`.dstore/lock`, an exclusive flock, dstore v0.1.11), and keeps it open for `SECONDS`.
//!
//! Usage: `cargo run --example holdlock -- DIR SECONDS`
//!
//! One command at a time has a working copy open, and Go and Rust commands exclude each other through the
//! same lock file. Until core v0.0.10 the packstore's single-owner lock did that on the side, and this
//! helper opened the packstore; a packstore is shared now. Once the working copy is open it prints
//! `locked <root>` on stdout, so a caller can wait for that line before it starts the command that must be
//! refused. The Go twin is `tools/vectorgen/cmd/holdlock`.

use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::process::ExitCode;
use std::time::Duration;

fn main() -> ExitCode {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let [dir, secs] = args.as_slice() else {
        eprintln!("usage: holdlock DIR SECONDS");
        return ExitCode::from(2);
    };
    let Some(secs) = secs.to_str().and_then(|s| s.parse::<u64>().ok()) else {
        eprintln!("holdlock: SECONDS must be a non-negative integer");
        return ExitCode::from(2);
    };
    let tree = match dstore::worktree::Tree::open(dir.as_bytes()) {
        Ok(tree) => tree,
        Err(e) => {
            eprintln!("holdlock: {e}");
            return ExitCode::from(1);
        }
    };
    let mut out = std::io::stdout().lock();
    if writeln!(out, "locked {}", String::from_utf8_lossy(&tree.root))
        .and_then(|()| out.flush())
        .is_err()
    {
        return ExitCode::from(1);
    }
    drop(out);
    std::thread::sleep(Duration::from_secs(secs));
    if let Err(e) = tree.close() {
        eprintln!("holdlock: {e}");
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}
