//! `dstore store push` / `store pull` at the binary level, without a network (owner cli-client): the order
//! of validation, local build, hex decoding and dialing in `cmd/dstore/client.go`, and what the local store
//! holds afterwards. Every expected output was recorded from the Go dstore v0.1.9 binary with the same
//! arguments (port-notes/impl-cli-client.md); the tree keys are computed here by core-rs over the same
//! fixture, since they depend on its owner and times.
//!
//! The commands that reach a cluster are tested in `dstore_cli::cmd_client` over the fake cluster, and end to
//! end by the interop suite.

use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use dstore::core::key::Key;
use dstore::core::{ingest, packstore, refstore};

const NO_CLUSTER: &str = "dstore: no cluster: set --ticket or $DSTORE_TICKET\n";
const BOGUS: &str =
    "dstore: ticket: \"bogus\" is neither a dstore1 ticket nor a node id: invalid length\n";

fn ok<T, E: std::fmt::Display>(r: Result<T, E>) -> T {
    match r {
        Ok(v) => v,
        Err(e) => panic!("{e}"),
    }
}

/// A fixture root: `src/hello.txt` and the FIFO `fifo`, in a fresh temp dir with its own HOME.
struct Fixture {
    dir: tempfile::TempDir,
    home: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Fixture {
        let dir = ok(tempfile::Builder::new()
            .prefix("dstore-cli-client-")
            .tempdir());
        let home = ok(tempfile::Builder::new()
            .prefix("dstore-cli-client-home-")
            .tempdir());
        ok(fs::create_dir(dir.path().join("src")));
        ok(fs::write(dir.path().join("src/hello.txt"), b"hello\n"));
        let status = ok(Command::new("mkfifo").arg(dir.path().join("fifo")).status());
        assert!(status.success(), "mkfifo: {status}");
        Fixture { dir, home }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }

    /// Runs the dstore binary in the fixture root with a clean environment (PATH, HOME, TZ=UTC, `env`),
    /// stdin from /dev/null and stdout/stderr piped (plain progress mode). Returns (exit, stdout, stderr).
    fn run(&self, env: &[(&str, &str)], args: &[&[u8]]) -> (i32, Vec<u8>, String) {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_dstore"));
        cmd.args(args.iter().map(|a| OsStr::from_bytes(a)))
            .current_dir(self.dir.path())
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", self.home.path())
            .env("TZ", "UTC")
            .stdin(Stdio::null());
        for (k, v) in env {
            cmd.env(k, v);
        }
        let out = ok(cmd.output());
        let Some(code) = out.status.code() else {
            panic!("{args:?}: killed: {}", out.status);
        };
        (
            code,
            out.stdout,
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    /// The key core-rs builds for `rel`, as `store push` does.
    fn key(&self, rel: &str) -> Key {
        let scratch = ok(tempfile::Builder::new()
            .prefix("dstore-cli-client-key-")
            .tempdir());
        let st = ok(packstore::Store::open_with(
            scratch.path(),
            packstore::Options::new().sync(false),
        ));
        let (_, root) = ingest::dir(&st, self.path(rel), ingest::Opts::default());
        let root = ok(root);
        ok(st.close());
        root
    }

    /// Whether `<rel>/packstore` holds `key`, and which of `names` `<rel>/refs` holds a record for.
    fn local_store(&self, rel: &str, key: &Key, names: &[&str]) -> (bool, Vec<String>) {
        let st = ok(packstore::Store::open_with(
            self.path(rel).join("packstore"),
            packstore::Options::new().sync(false),
        ));
        let has = st.get(*key).is_ok();
        ok(st.close());
        let refs = ok(refstore::Store::open(self.path(rel).join("refs"), false));
        let held = names
            .iter()
            .filter(|n| refs.get(n).is_ok())
            .map(|n| (*n).to_owned())
            .collect();
        (has, held)
    }
}

/// `built %s: %d new objects\n` with the key's first 16 hex characters.
fn built(key: &Key, n: usize) -> String {
    let hex: String = key.to_string().chars().take(16).collect();
    format!("built {hex}: {n} new objects\n")
}

fn fail(stderr: String) -> (i32, Vec<u8>, String) {
    (1, Vec::new(), stderr)
}

#[test]
fn push_builds_the_tree_before_decoding_the_version() {
    let f = Fixture::new();
    let k = f.key("src");
    assert_eq!(
        f.run(
            &[],
            &[
                b"store",
                b"push",
                b"--local",
                b"L",
                b"--expected-version",
                b"zz",
                b"src",
                b"trees/x"
            ],
        ),
        fail(format!("{}dstore: bad hex \"zz\"\n", built(&k, 2)))
    );
    // The tree is in the local store; no local reference was written.
    assert_eq!(f.local_store("L", &k, &["trees/x"]), (true, Vec::new()));
    // hexDecode echoes the argument with %q.
    assert_eq!(
        f.run(
            &[],
            &[
                b"store",
                b"push",
                b"--local",
                b"L",
                b"--expected-version",
                b"\xff\xfe",
                b"src",
                b"trees/x"
            ],
        ),
        fail(format!("{}dstore: bad hex \"\\xff\\xfe\"\n", built(&k, 0)))
    );
}

#[test]
fn push_builds_the_tree_before_failing_on_a_missing_ticket() {
    let f = Fixture::new();
    let k = f.key("src");
    let want = fail(format!("{}{NO_CLUSTER}", built(&k, 2)));
    let cases: [&[&[u8]]; 6] = [
        &[b"store", b"push", b"--local", b"L", b"src", b"trees/x"],
        // --force ignores --expected-version, even bad hex.
        &[
            b"store",
            b"push",
            b"--local",
            b"L",
            b"--force",
            b"--expected-version",
            b"zz",
            b"src",
            b"trees/x",
        ],
        // An odd trailing nibble is dropped, so one byte decodes to nothing.
        &[
            b"store",
            b"push",
            b"--local",
            b"L",
            b"--expected-version",
            b"abc",
            b"src",
            b"trees/x",
        ],
        &[
            b"store",
            b"push",
            b"--local",
            b"L",
            b"--expected-version",
            b"\xff",
            b"src",
            b"trees/x",
        ],
        // --jobs below 1 means every core.
        &[
            b"store", b"push", b"--local", b"L", b"--jobs", b"-3", b"src", b"trees/x",
        ],
        &[b"store", b"push", b"--local", b"L", b"src/", b"trees/x"],
    ];
    for args in cases {
        // A fresh fixture has its own times, hence its own key.
        let fresh = Fixture::new();
        let fk = fresh.key("src");
        assert_eq!(
            fresh.run(&[], args),
            fail(format!("{}{NO_CLUSTER}", built(&fk, 2))),
            "{args:?}"
        );
        assert_eq!(
            fresh.local_store("L", &fk, &["trees/x"]),
            (true, Vec::new()),
            "{args:?}"
        );
    }
    // $AMBER_STORE satisfies --local.
    assert_eq!(
        f.run(
            &[("AMBER_STORE", "L")],
            &[b"store", b"push", b"src", b"trees/x"]
        ),
        want
    );
    // Built again: nothing new.
    assert_eq!(
        f.run(
            &[],
            &[b"store", b"push", b"--local", b"L", b"src", b"trees/y"]
        ),
        fail(format!("{}{NO_CLUSTER}", built(&k, 0)))
    );
}

#[test]
fn push_of_a_single_file() {
    let f = Fixture::new();
    let k = f.key("src/hello.txt");
    assert_eq!(
        f.run(
            &[],
            &[
                b"store",
                b"push",
                b"--local",
                b"L",
                b"src/hello.txt",
                b"trees/x"
            ]
        ),
        fail(format!("{}{NO_CLUSTER}", built(&k, 1)))
    );
    assert_eq!(f.local_store("L", &k, &["trees/x"]), (true, Vec::new()));
}

#[test]
fn push_parses_the_ticket_after_building() {
    let f = Fixture::new();
    let k = f.key("src");
    assert_eq!(
        f.run(
            &[],
            &[
                b"store",
                b"push",
                b"--local",
                b"L",
                b"--ticket",
                b"bogus",
                b"src",
                b"trees/x"
            ]
        ),
        fail(format!("{}{BOGUS}", built(&k, 2)))
    );
    assert_eq!(
        f.run(
            &[("DSTORE_TICKET", "bogus")],
            &[b"store", b"push", b"--local", b"L", b"src", b"trees/x"]
        ),
        fail(format!("{}{BOGUS}", built(&k, 0)))
    );
    let hex63 = "4bb675de4f6376ab61737033d701560e4434be1d6736562ae29b8835b763cd2";
    assert_eq!(
        f.run(
            &[],
            &[
                b"store",
                b"push",
                b"--local",
                b"L",
                b"--ticket",
                hex63.as_bytes(),
                b"src",
                b"trees/x"
            ]
        ),
        fail(format!(
            "{}dstore: ticket: \"{hex63}\" is neither a dstore1 ticket nor a node id: failed to decode base32 string\n",
            built(&k, 0)
        ))
    );
    // An invalid --user is not checked before the dial.
    assert_eq!(
        f.run(
            &[],
            &[
                b"store",
                b"push",
                b"--local",
                b"L",
                b"--user",
                b"a\x01",
                b"--ticket",
                b"bogus",
                b"src",
                b"trees/x"
            ]
        ),
        fail(format!("{}{BOGUS}", built(&k, 0)))
    );
}

#[test]
fn push_path_errors_come_after_opening_the_local_store() {
    let f = Fixture::new();
    assert_eq!(
        f.run(
            &[],
            &[b"store", b"push", b"--local", b"L", b"missing", b"trees/x"]
        ),
        fail("dstore: stat missing: no such file or directory\n".to_owned())
    );
    assert!(f.path("L/packstore").is_dir() && f.path("L/refs").is_dir());
    assert_eq!(
        f.run(
            &[],
            &[b"store", b"push", b"--local", b"L", b"fifo", b"trees/x"]
        ),
        fail("dstore: fifo is neither a regular file nor a directory\n".to_owned())
    );
}

#[test]
fn push_checks_arguments_before_touching_the_local_store() {
    let f = Fixture::new();
    let cases: [(&[&[u8]], &str); 3] = [
        (
            &[b"store", b"push", b"--local", b"L", b"src", b"trees/x\x7f"],
            "dstore: reference name must not contain control characters\n",
        ),
        (
            &[b"store", b"push", b"--local", b"L", b"src", b"trees/\xff"],
            "dstore: reference name must be valid UTF-8\n",
        ),
        // Flags after the first positional argument are positional.
        (
            &[
                b"store", b"push", b"--local", b"L", b"src", b"trees/x", b"--user", b"x",
            ],
            "dstore: push PATH NAME\n",
        ),
    ];
    for (args, want) in cases {
        assert_eq!(f.run(&[], args), fail(want.to_owned()), "{args:?}");
        assert!(!f.path("L").exists(), "{args:?}");
    }
}

#[test]
fn pull_opens_the_local_store_before_dialing() {
    let f = Fixture::new();
    assert_eq!(
        f.run(
            &[],
            &[
                b"store",
                b"pull",
                b"--local",
                b"L",
                b"--ticket",
                b"bogus",
                b"trees/x"
            ]
        ),
        fail(BOGUS.to_owned())
    );
    assert!(f.path("L/packstore").is_dir() && f.path("L/refs").is_dir());
    assert_eq!(
        f.run(
            &[("DSTORE_TICKET", "")],
            &[b"store", b"pull", b"--local", b"M", b"trees/x"]
        ),
        fail(NO_CLUSTER.to_owned())
    );
    assert!(f.path("M/packstore").is_dir() && f.path("M/refs").is_dir());
    // An empty NAME fails before the local store is opened.
    assert_eq!(
        f.run(&[], &[b"store", b"pull", b"--local", b"N", b""]),
        fail("dstore: pull NAME\n".to_owned())
    );
    assert!(!f.path("N").exists());
}
