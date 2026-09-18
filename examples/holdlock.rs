//! Lock-interop helper for interop check D12: opens `<DIR>/.dstore/packstore` (taking its flock) and sleeps
//! for `SECONDS`.
//!
//! Usage: `cargo run --example holdlock -- DIR SECONDS`
//!
//! Once the lock is held it prints `locked <path>` on stdout, so a caller can wait for that line before it
//! starts the process that must be refused. The Go twin is `tools/vectorgen/cmd/holdlock`.

use std::io::Write;
use std::path::Path;
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
    let path = Path::new(dir).join(".dstore").join("packstore");
    let store = match dstore::core::packstore::Store::open(&path) {
        Ok(store) => store,
        Err(e) => {
            eprintln!("holdlock: {e}");
            return ExitCode::from(1);
        }
    };
    let mut out = std::io::stdout().lock();
    if writeln!(out, "locked {}", path.display())
        .and_then(|()| out.flush())
        .is_err()
    {
        return ExitCode::from(1);
    }
    drop(out);
    std::thread::sleep(Duration::from_secs(secs));
    if let Err(e) = store.close() {
        eprintln!("holdlock: {e}");
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}
