//! dstore `cmd/dstore` (`main.go`, `client.go`, `wc.go`, `size.go`, `tui.go`): the command table,
//! actions, node-side stubs, progress display and TUI.
//!
//! Modules: `app` (command table), `nodeside` (PORTING.md §2.2), `common`, `cmd_admin`, `cmd_client`,
//! `cmd_wc`, `size`, `progress`.
//!
//! Spec: PORTING.md §4.12; port-notes/cli.md.
#![allow(dead_code, unused_variables)] // L0 stubs: remove once the modules are implemented
#![deny(unsafe_op_in_unsafe_fn)]

mod app;
mod cmd_admin;
mod cmd_client;
mod cmd_wc;
pub mod common;
pub mod nodeside;
pub mod progress;
pub mod size;

pub use app::app;

/// `dstore version <VERSION>`.
pub const VERSION: &str = match option_env!("DSTORE_VERSION") {
    Some(v) => v,
    None => "dev",
};

/// Process entry used by the root bin: restore SIGPIPE to SIG_DFL, build a multi-thread tokio runtime,
/// run(), flush stdout and stderr, return the exit code. The caller exits with `std::process::exit`
/// (the runtime is not dropped).
pub fn main_entry() -> i32 {
    todo!()
}

/// Prints "dstore: <err>" (after the errno rewrite) or "<msg>"; returns 0/1/3.
pub async fn run(args: Vec<std::ffi::OsString>) -> i32 {
    todo!()
}

/// Go runtime panic parity (DD-7): writes "panic: <text>\n" to stderr and exits 2.
pub fn go_panic_exit(text: &str) -> ! {
    todo!()
}
