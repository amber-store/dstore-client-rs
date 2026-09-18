//! dstore `cmd/dstore` (`main.go`, `client.go`, `wc.go`, `size.go`, `tui.go`): the command table,
//! actions, node-side stubs, progress display and TUI.
//!
//! Modules: `app` (command table), `nodeside` (PORTING.md §2.2), `common`, `cmd_admin`, `cmd_client`,
//! `cmd_wc`, `size`, `progress`.
//!
//! Spec: PORTING.md §4.12; port-notes/cli.md.
#![deny(unsafe_op_in_unsafe_fn)]

mod app;
mod cmd_admin;
mod cmd_client;
mod cmd_wc;
pub mod common;
pub mod nodeside;
pub mod progress;
pub mod size;

use std::ffi::OsString;
use std::io::Write;

use dstore_gocli::CliError;
use dstore_gocompat::errno::rewrite_os_errors;

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
    // PORTING.md §5.9: a write to a closed stdout or stderr kills the process by SIGPIPE, as in Go.
    dstore_gocompat::os::restore_sigpipe();
    let args: Vec<OsString> = std::env::args_os().collect();
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            write_stderr(format!("dstore: {}\n", rewrite_os_errors(&e.to_string())).as_bytes());
            return 1;
        }
    };
    let code = runtime.block_on(run(args));
    flush_stdio();
    // Go's exit abandons goroutines; dropping the runtime would wait for blocking tasks instead.
    std::mem::forget(runtime);
    code
}

/// Prints "dstore: <err>" (after the errno rewrite) or "<msg>"; returns 0/1/3.
///
/// `args` is the whole command line, program name first, as Go's `app.Run(os.Args)`.
pub async fn run(args: Vec<OsString>) -> i32 {
    let app = app();
    let mut stdout = std::io::stdout();
    let result = dstore_gocli::run(&app, args, &mut stdout).await;
    let _ = stdout.flush();
    match result {
        Ok(()) => 0,
        // main.go:49-52: fmt.Fprintln(os.Stderr, "dstore:", err); os.Exit(1).
        Err(CliError::Msg(msg)) => {
            write_stderr(format!("dstore: {}\n", rewrite_os_errors(&msg)).as_bytes());
            1
        }
        // urfave HandleExitCoder: Fprintln(ErrWriter, err) when the text is not empty, then OsExiter(code).
        Err(CliError::Exit { msg, code }) => {
            if !msg.is_empty() {
                write_stderr(format!("{msg}\n").as_bytes());
            }
            code
        }
    }
}

/// Go runtime panic parity (DD-7): writes "panic: <text>\n" to stderr and exits 2.
pub fn go_panic_exit(text: &str) -> ! {
    // Go's stdout is unbuffered: whatever the command printed before the panic is already out.
    let _ = std::io::stdout().flush();
    write_stderr(format!("panic: {text}\n").as_bytes());
    std::process::exit(2)
}

/// One write of `b` to stderr; errors are ignored as `fmt.Fprintln` errors are.
fn write_stderr(b: &[u8]) {
    let mut e = std::io::stderr().lock();
    let _ = e.write_all(b);
    let _ = e.flush();
}

fn flush_stdio() {
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
}
