//! The working-copy commands through the `dstore` binary (cli-wc), beyond the Go snapshots of
//! tests/cli_snapshots.rs:
//! - the ticket precedence: a stored ticket beats `$DSTORE_TICKET`, `--ticket` overrides it for one run and
//!   is never stored, and clone and init take the environment;
//! - `status` and `diff` install no signal handler, so SIGINT kills them (PORTING.md §1.4, §5.4);
//! - `os.Getwd`'s `$PWD` rule decides the working-copy root that `diff PATH` resolves against;
//! - the packstore lock is taken before anything else is read;
//! - paths are printed as raw bytes.
//!
//! No test opens a socket: every connection command here fails at the ticket parse, before an endpoint binds.

use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use amber_store_core::key::Key;
use dstore_gocompat::time::GoTime;
use dstore_worktree::{Config, State, Tree};

/// SIGINT on Linux and macOS.
const SIGINT: i32 = 2;

/// A working copy in a fresh temporary directory (resolved), synced to the empty tree, no remote.
struct Wc {
    root: PathBuf,
    _dir: tempfile::TempDir,
}

fn temp_dir(prefix: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::Builder::new()
        .prefix(prefix)
        .tempdir()
        .expect("temp dir");
    let path = fs::canonicalize(dir.path()).expect("resolve temp dir");
    (dir, path)
}

fn wc(ticket: &str) -> Wc {
    let (dir, root) = temp_dir("dstore-cli-wc-");
    let cfg = Config {
        ticket: ticket.as_bytes().to_vec(),
        name: b"trees/demo".to_vec(),
        ..Default::default()
    };
    let mut t = Tree::create(root.as_os_str().as_bytes(), cfg).expect("create the working copy");
    t.state = State {
        base: dstore_worktree::empty_tree().0,
        remote: Key([0; 32]),
        has_remote: false,
        remote_version: None,
        synced_at: GoTime::from_unix_nano(0),
    };
    t.save_state().expect("save the state");
    t.close().expect("close the working copy");
    Wc { root, _dir: dir }
}

/// The binary in `dir` with a clean environment (`PATH`, `HOME`, `TZ=UTC`, then `env`), stdin empty.
fn dstore(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_dstore"));
    cmd.args(args)
        .current_dir(dir)
        .env_clear()
        .env("HOME", dir)
        .env("TZ", "UTC")
        .stdin(Stdio::null());
    if let Some(path) = std::env::var_os("PATH") {
        cmd.env("PATH", path);
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd
}

/// Exit code, stdout, stderr.
fn run(cmd: &mut Command) -> (i32, Vec<u8>, String) {
    let out = cmd.output().expect("run dstore");
    let code = out
        .status
        .code()
        .unwrap_or_else(|| panic!("dstore was killed: {:?}", out.status));
    (
        code,
        out.stdout,
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn ticket_error(field: &str) -> String {
    format!(
        "dstore: ticket: \"{field}\" is neither a dstore1 ticket nor a node id: invalid length\n"
    )
}

#[test]
fn stored_ticket_beats_the_environment() {
    let w = wc("storedbogus");
    let config = w.root.join(".dstore/config");
    let before = fs::read(&config).expect("config");
    let env = [("DSTORE_TICKET", "envbogus")];
    for cmd in ["fetch", "pull", "push"] {
        let got = run(&mut dstore(&w.root, &[cmd], &env));
        assert_eq!(got, (1, Vec::new(), ticket_error("storedbogus")), "{cmd}");
    }
    // --ticket overrides for one run; an empty --ticket falls through to the stored one.
    let got = run(&mut dstore(
        &w.root,
        &["fetch", "--ticket", "flagbogus"],
        &env,
    ));
    assert_eq!(got, (1, Vec::new(), ticket_error("flagbogus")));
    let got = run(&mut dstore(&w.root, &["push", "--ticket="], &env));
    assert_eq!(got, (1, Vec::new(), ticket_error("storedbogus")));
    // From a subdirectory the same working copy is found.
    fs::create_dir(w.root.join("sub")).expect("mkdir");
    let got = run(&mut dstore(&w.root.join("sub"), &["pull", "--force"], &env));
    assert_eq!(got, (1, Vec::new(), ticket_error("storedbogus")));
    // Nothing was stored.
    assert_eq!(fs::read(&config).expect("config"), before);

    // Without a stored ticket the environment counts, then nothing does.
    let w = wc("");
    let got = run(&mut dstore(&w.root, &["fetch"], &env));
    assert_eq!(got, (1, Vec::new(), ticket_error("envbogus")));
    let got = run(&mut dstore(&w.root, &["push"], &[]));
    assert_eq!(
        got,
        (
            1,
            Vec::new(),
            "dstore: no cluster: set --ticket or $DSTORE_TICKET\n".to_string()
        )
    );
}

/// clone and init have no stored config: `$DSTORE_TICKET` counts, and a failed ticket leaves nothing behind.
#[test]
fn clone_and_init_take_the_environment() {
    let (_dir, empty) = temp_dir("dstore-cli-wc-empty-");
    let env = [
        ("DSTORE_TICKET", "envbogus"),
        ("DSTORE_NO_DISCOVERY", "maybe"),
    ];
    for args in [
        &["clone", "trees/x"][..],
        &["clone", "trees/x", "target"],
        &["clone", "--ticket", "", "trees/x"],
        &["init", "trees/x"],
        &["init", "trees/x", "extra"],
    ] {
        let got = run(&mut dstore(&empty, args, &env));
        assert_eq!(got, (1, Vec::new(), ticket_error("envbogus")), "{args:?}");
    }
    let got = run(&mut dstore(
        &empty,
        &["clone", "--ticket", "zz", "trees/x"],
        &env,
    ));
    assert_eq!(got, (1, Vec::new(), ticket_error("zz")));
    let left: Vec<_> = fs::read_dir(&empty)
        .expect("read dir")
        .map(|e| e.expect("entry").file_name())
        .collect();
    assert!(left.is_empty(), "left behind: {left:?}");
}

/// Waits for `child`, at most `limit`.
fn wait_for(child: &mut std::process::Child, limit: Duration) -> Option<std::process::ExitStatus> {
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().expect("wait") {
            return Some(status);
        }
        if start.elapsed() > limit {
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Runs `args` with stdout a pipe nobody reads, so the command blocks once its output fills the pipe, then
/// sends SIGINT. Without a handler the default disposition kills the process, whether it is still scanning
/// or already blocked on a write; a registered handler would swallow the signal and leave it blocked.
fn assert_killed_by_sigint(dir: &Path, args: &[&str]) {
    let mut child = dstore(dir, args, &[])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn dstore");
    std::thread::sleep(Duration::from_millis(1500));
    let sent = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .expect("run kill");
    assert!(sent.success(), "kill -INT failed");
    let status = wait_for(&mut child, Duration::from_secs(15));
    if status.is_none() {
        let _ = child.kill();
        let _ = child.wait();
    }
    let status =
        status.unwrap_or_else(|| panic!("{args:?} survived SIGINT: a signal handler is installed"));
    assert_eq!(status.signal(), Some(SIGINT), "{args:?}: {status:?}");
}

#[test]
fn status_and_diff_die_by_sigint() {
    let w = wc("t");
    // 3000 new files: status prints about 250 KiB and diff more, beyond any pipe buffer.
    for i in 0..3000 {
        let name = format!("a-file-with-a-long-name-so-that-each-status-row-is-long-{i:05}.txt");
        fs::write(w.root.join(name), "one line\n").expect("write");
    }
    assert_killed_by_sigint(&w.root, &["status"]);
    assert_killed_by_sigint(&w.root, &["diff"]);
    assert_killed_by_sigint(&w.root, &["diff", "--stat"]);
}

/// `os.Getwd` returns `$PWD` when it names the current directory, symlinks included; the root of the working
/// copy is then the symlinked path, and `diff PATH` resolves PATH lexically against it.
#[test]
fn pwd_decides_the_working_copy_root() {
    let w = wc("t");
    fs::write(w.root.join("a.txt"), "hello\n").expect("write");
    let (_links_dir, links) = temp_dir("dstore-cli-wc-links-");
    let link = links.join("wc");
    std::os::unix::fs::symlink(&w.root, &link).expect("symlink");
    let link_text = link.to_str().expect("UTF-8 temp path");
    let arg = format!("{link_text}/a.txt");
    let a_txt =
        b"diff a/a.txt b/a.txt\n--- /dev/null\n+++ b/a.txt\n@@ -0,0 +1 @@\n+hello\n".to_vec();

    let got = run(&mut dstore(&link, &["diff", &arg], &[("PWD", link_text)]));
    assert_eq!(got, (0, a_txt.clone(), String::new()));
    // A relative path is the same either way.
    let got = run(&mut dstore(
        &link,
        &["diff", "a.txt"],
        &[("PWD", link_text)],
    ));
    assert_eq!(got, (0, a_txt.clone(), String::new()));
    let got = run(&mut dstore(&link, &["diff", "a.txt"], &[]));
    assert_eq!(got, (0, a_txt, String::new()));

    // Without $PWD, or with a $PWD naming another directory, getcwd gives the resolved root.
    let outside = format!("dstore: {arg} is outside the working copy\n");
    let got = run(&mut dstore(&link, &["diff", &arg], &[]));
    assert_eq!(got, (1, Vec::new(), outside.clone()));
    let links_text = links.to_str().expect("UTF-8 temp path");
    let got = run(&mut dstore(&link, &["diff", &arg], &[("PWD", links_text)]));
    assert_eq!(got, (1, Vec::new(), outside.clone()));
    // A relative $PWD is ignored.
    let got = run(&mut dstore(&link, &["diff", &arg], &[("PWD", "wc")]));
    assert_eq!(got, (1, Vec::new(), outside));
}

/// `worktree.Open` takes the packstore lock before it reads the state; the connection commands open the
/// working copy before they look at the ticket.
#[test]
fn a_locked_working_copy_is_refused() {
    let w = wc("storedbogus");
    let held = Tree::open(w.root.as_os_str().as_bytes()).expect("hold the working copy");
    let locked = format!(
        "dstore: packstore: {}/.dstore/packstore is already open: resource temporarily unavailable\n",
        w.root.display()
    );
    for args in [
        &["status"][..],
        &["diff"],
        &["diff", "--stat", "a"],
        &["fetch"],
        &["pull"],
        &["push", "--ticket", "bogus"],
    ] {
        let got = run(&mut dstore(&w.root, args, &[]));
        assert_eq!(got, (1, Vec::new(), locked.clone()), "{args:?}");
    }
    held.close().expect("release");
    let got = run(&mut dstore(&w.root, &["status"], &[]));
    assert_eq!(got.0, 0, "{got:?}");
}

/// Names are bytes: status and diff print them raw. File systems that refuse non-UTF-8 names (APFS) end
/// the test after the working copy is checked with a plain name.
#[test]
fn paths_are_printed_raw() {
    let w = wc("t");
    fs::write(w.root.join("plain.txt"), "p\n").expect("write");
    let raw = w.root.join(OsStr::from_bytes(b"caf\xe9.txt"));
    let have_raw = match fs::write(&raw, "r\n") {
        Ok(()) => true,
        // EILSEQ: this file system accepts UTF-8 names only.
        Err(e) if e.raw_os_error() == Some(92) || e.raw_os_error() == Some(84) => false,
        Err(e) => panic!("write a non-UTF-8 name: {e}"),
    };
    let (code, out, err) = run(&mut dstore(&w.root, &["status"], &[]));
    assert_eq!((code, err.as_str()), (0, ""));
    let mut want = b"reference trees/demo, synced to 2001bbe6a9f5a014\n\
        remote: the reference does not exist on the cluster\nchanges:\n"
        .to_vec();
    if have_raw {
        want.extend_from_slice(b"  new       caf\xe9.txt\n");
    }
    want.extend_from_slice(b"  new       plain.txt\n");
    assert_eq!(
        out.escape_ascii().to_string(),
        want.escape_ascii().to_string()
    );
    if !have_raw {
        return;
    }
    let (code, out, err) = run(&mut dstore(&w.root, &["diff", "--stat"], &[]));
    assert_eq!((code, err.as_str()), (0, ""));
    assert_eq!(
        out.escape_ascii().to_string(),
        b" caf\xe9.txt | +1 -0\n plain.txt | +1 -0\n 2 files changed, 2 insertions(+), 0 deletions(-)\n"
            .escape_ascii()
            .to_string()
    );
}
