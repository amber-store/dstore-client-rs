//! CLI snapshots: runs `env!("CARGO_BIN_EXE_dstore")` over `tests/golden/cli/snapshots.json` (captured from
//! the Go binary by `tools/vectorgen/cmd/clisnap`) with a clean environment (`PATH`, a temporary `HOME`,
//! `TZ=UTC`), and compares stdout, stderr and the exit status, with `{CWD}` normalised through `pwd -P`.
//!
//! The procedure is VECTORS.md "Running a case" (clisnap `runCase`): a fresh ROOT with the case's fixture,
//! cwd = ROOT/subdir resolved to a real path, `{CWD}` substituted in args and env values, stdin a pipe
//! carrying `stdin` then EOF, stdout and stderr pipes (so every transfer runs in plain mode), a 60 s limit, a
//! normal exit, then `{CWD}`, `{ROOT}`, `{key:NAME}` and `{key16:NAME}` normalisation. A case with `node_side`
//! is compared with `node_side.rust`, the PORTING.md §2.2/§2.3 substitute.
//!
//! The cases are split by what decides their outcome:
//! - `framework_cases`: groups `help`, `unknown`, `usage`, and `required` flag errors. The command table
//!   (`dstore_cli::app`) and `dstore-gocli` decide them before any action runs.
//! - `admin_cases`, `client_cases`, `wc_cases`: every other case reaches the action of its top-level
//!   command, in `cmd_admin`, `cmd_client` or `cmd_wc` (layer L5). These are the argument, ticket and
//!   relay validation before dialing, the node-side paths up to the store, the `cluster replicas` prompt,
//!   the DD-2 refusal, and the offline working-copy commands.
//!
//! No captured case needs the network in Rust. Go binds an endpoint only in the kind-A node-side cases,
//! and Rust replaces those with the fixed §2.2 A text. Every other case fails or finishes before dialing.
//!
//! Tests that need nothing unfinished run now, over the same vectors:
//! - the classification of every case, and its agreement with `dstore_gocli::dispatch` over the command table:
//!   exactly the action cases reach an action, and nothing is printed before it;
//! - the node-side substitute texts against `dstore_cli::nodeside`;
//! - the pre-dial outcomes that cli-app's own code decides, with the flag values the framework parsed:
//!   `nodeside::local_ticket` over every identity-read case and `size::pack_size` over every `--pack-size`
//!   case;
//! - the fixture builder over every fixture.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use amber_store_core::{ingest, key::Key, packstore};
use dstore_gocli::{AppDef, CliError, Context};
use dstore_testkit::golden::{self, load_json};
use serde::Deserialize;

// ---- the vector file ----

#[derive(Debug, Deserialize)]
struct Snapshots {
    fixtures: Vec<Fixture>,
    cases: Vec<Case>,
}

#[derive(Debug, Deserialize)]
struct Fixture {
    name: String,
    steps: Vec<Step>,
}

/// A fixture step (VECTORS.md "Fixture steps"). An unknown op fails the vector load.
#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Step {
    Mkdir {
        path: String,
        mode: u32,
    },
    Write {
        path: String,
        text: String,
        mode: u32,
    },
    Symlink {
        path: String,
        target: String,
    },
    Remove {
        path: String,
    },
    Chmod {
        path: String,
        mode: u32,
    },
    Mtime {
        path: String,
        #[serde(deserialize_with = "golden::decimal_i64")]
        unix_ns: i64,
    },
    WcCreate {
        config: WcConfig,
    },
    Ingest {
        dir: String,
        into: String,
        #[serde(default)]
        exclude: Vec<String>,
        var: String,
    },
    WcState {
        base_var: String,
        #[serde(default)]
        remote_var: Option<String>,
        #[serde(default)]
        remote_version_hex: Option<String>,
        #[serde(deserialize_with = "golden::decimal_i64")]
        synced_at_unix_ns: i64,
    },
    PebbleRefs {
        path: String,
        names: Vec<String>,
    },
}

/// `.dstore/config` JSON keys.
#[derive(Debug, Deserialize)]
struct WcConfig {
    #[serde(default)]
    ticket: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    relay: String,
    #[serde(default)]
    no_relay: bool,
    #[serde(default)]
    no_discovery: bool,
    #[serde(default)]
    user: String,
}

#[derive(Debug, Deserialize)]
struct Case {
    name: String,
    group: String,
    args: Vec<String>,
    env: Vec<EnvVar>,
    fixture: String,
    subdir: String,
    stdin: String,
    exit: i32,
    stdout: String,
    stderr: String,
    node_side: Option<NodeSide>,
}

#[derive(Debug, Deserialize)]
struct EnvVar {
    name: String,
    value: String,
}

#[derive(Debug, Deserialize)]
struct NodeSide {
    kind: String,
    rust: Outcome,
}

#[derive(Debug, Deserialize)]
struct Outcome {
    exit: i32,
    stdout: String,
    stderr: String,
}

impl Case {
    /// What the Rust binary must print: Go's outcome, or the designed substitute of a node-side case.
    fn expected(&self) -> (i32, &str, &str) {
        match &self.node_side {
            Some(ns) => (ns.rust.exit, &ns.rust.stdout, &ns.rust.stderr),
            None => (self.exit, &self.stdout, &self.stderr),
        }
    }
}

// ---- classification ----

/// What produces a case's outcome in Rust.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Needs {
    /// The command table and `dstore-gocli`: help, unknown topics, usage errors, required flags.
    Framework,
    /// An action of `cmd_admin`: cluster, serve, token, node, voter, transition, gc, catalog.
    Admin,
    /// An action of `cmd_client`: store, refs, watch, ref, ls, cat.
    Client,
    /// An action of `cmd_wc`: clone, init, fetch, pull, push, status, diff.
    Wc,
}

fn classify(case: &Case) -> Result<Needs, String> {
    match case.group.as_str() {
        "help" | "unknown" | "usage" => return Ok(Needs::Framework),
        "required" if case.stderr.starts_with("dstore: Required flag") => {
            return Ok(Needs::Framework);
        }
        "required" | "validation" | "node-side" | "prompt" | "wc" => {}
        other => return Err(format!("{}: unknown group {other:?}", case.name)),
    }
    match command_of(&case.args) {
        Some(
            "cluster" | "serve" | "token" | "node" | "voter" | "transition" | "gc" | "catalog",
        ) => Ok(Needs::Admin),
        Some("store" | "refs" | "watch" | "ref" | "ls" | "cat") => Ok(Needs::Client),
        Some("clone" | "init" | "fetch" | "pull" | "push" | "status" | "diff") => Ok(Needs::Wc),
        other => Err(format!(
            "{}: no known command in {:?} (found {other:?})",
            case.name, case.args
        )),
    }
}

/// The top-level command: the first argument that is not a global flag (`--log-level VALUE`,
/// `-log-level=VALUE`).
fn command_of(args: &[String]) -> Option<&str> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a.len() >= 2 && a.starts_with('-') {
            if a.trim_start_matches('-') == "log-level" {
                it.next();
            }
            continue;
        }
        return Some(a);
    }
    None
}

/// `serve`, `cluster init` or `node join` of a kind-A case.
fn node_side_command(args: &[String]) -> String {
    match args.first().map(String::as_str) {
        Some("serve") => "serve".to_string(),
        Some(parent) => format!(
            "{parent} {}",
            args.get(1).map(String::as_str).unwrap_or_default()
        ),
        None => String::new(),
    }
}

/// The value of `--name V`, `-name V`, `--name=V` or `-name=V`.
fn flag_value<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let bare = a.trim_start_matches('-');
        if bare == name && a.starts_with('-') {
            return it.next().map(String::as_str);
        }
        if let Some(v) = bare.strip_prefix(name).and_then(|r| r.strip_prefix('=')) {
            return Some(v);
        }
    }
    None
}

/// How `dstore_cli::run` prints an error.
fn rendered(e: CliError) -> String {
    match e {
        CliError::Msg(m) => format!("dstore: {m}\n"),
        CliError::Exit { msg, .. } => format!("{msg}\n"),
    }
}

// ---- library-level checks ----

/// `dstore_gocli::dispatch` over the command table with the case's argument vector and environment, `{CWD}`
/// standing for `cwd`. `run_case` also passes `PATH`, `HOME` and `TZ`, which no flag binds. Returns the context
/// of the action the framework reached, if any, and what it wrote to stdout.
fn dispatch_case(
    app: &AppDef,
    case: &Case,
    cwd: &str,
) -> (Result<Option<Context>, CliError>, Vec<u8>) {
    let args: Vec<OsString> = std::iter::once(OsString::from("dstore"))
        .chain(
            case.args
                .iter()
                .map(|a| OsString::from(a.replace("{CWD}", cwd))),
        )
        .collect();
    // A later entry wins, as for `Command::env` and Go's `exec.Cmd.Env`.
    let env: BTreeMap<&str, String> = case
        .env
        .iter()
        .map(|e| (e.name.as_str(), e.value.replace("{CWD}", cwd)))
        .collect();
    let getenv = |name: &str| env.get(name).map(OsString::from);
    let mut stdout = Vec::new();
    let found = dstore_gocli::dispatch(app, args, &mut stdout, &getenv)
        .map(|found| found.map(|(_action, ctx)| ctx));
    (found, stdout)
}

/// The context of the action a case reaches; panics when the framework stops before it.
fn action_context(app: &AppDef, case: &Case, cwd: &str) -> Context {
    match dispatch_case(app, case, cwd) {
        (Ok(Some(ctx)), _) => ctx,
        (other, stdout) => panic!(
            "{}: no action reached: {:?}, stdout {:?}",
            case.name,
            other.map(|found| found.is_some()),
            String::from_utf8_lossy(&stdout)
        ),
    }
}

fn fixture_named<'a>(snaps: &'a Snapshots, name: &str) -> &'a Fixture {
    snaps
        .fixtures
        .iter()
        .find(|f| f.name == name)
        .unwrap_or_else(|| panic!("no fixture {name:?}"))
}

/// A fixture built in fresh temporary directories, removed on drop.
struct Built {
    /// The resolved ROOT.
    root: PathBuf,
    vars: BTreeMap<String, String>,
    _root_dir: tempfile::TempDir,
    _scratch: tempfile::TempDir,
}

fn build_in_temp(fixture: &Fixture) -> Built {
    let root_dir = tempfile::Builder::new()
        .prefix("dstore-snap-root-")
        .tempdir()
        .expect("temp root");
    let scratch = tempfile::Builder::new()
        .prefix("dstore-snap-scratch-")
        .tempdir()
        .expect("temp scratch");
    let root = fs::canonicalize(root_dir.path()).expect("resolve root");
    let vars = build_fixture(&root, scratch.path(), fixture)
        .unwrap_or_else(|e| panic!("fixture {}: {e}", fixture.name));
    Built {
        root,
        vars,
        _root_dir: root_dir,
        _scratch: scratch,
    }
}

// ---- the tests ----

/// Every case belongs to exactly one of the case tests below, names a fixture that exists, and every
/// node-side substitute is the text that `dstore_cli::nodeside` (PORTING.md §2.2) or `common::open_local`
/// (§2.3) produces.
#[test]
fn cases_are_classified_and_substitutes_match() {
    let snaps: Snapshots = load_json("cli/snapshots.json");
    let fixtures: BTreeSet<&str> = snaps.fixtures.iter().map(|f| f.name.as_str()).collect();
    let mut counts: BTreeMap<Needs, usize> = BTreeMap::new();
    let mut problems = Vec::new();
    for case in &snaps.cases {
        match classify(case) {
            Ok(n) => *counts.entry(n).or_default() += 1,
            Err(e) => problems.push(e),
        }
        if !fixtures.contains(case.fixture.as_str()) {
            problems.push(format!("{}: unknown fixture {:?}", case.name, case.fixture));
        }
        let Some(ns) = &case.node_side else {
            continue;
        };
        let want = match ns.kind.as_str() {
            "A" => rendered(dstore_cli::nodeside::node_side_error(&node_side_command(
                &case.args,
            ))),
            "B" => format!(
                "dstore: {}\n",
                dstore_cli::nodeside::STORE_TICKET_UNSUPPORTED
            ),
            "C" => format!(
                "dstore: {}\n",
                dstore_cli::nodeside::CATALOG_RESTORE_UNSUPPORTED
            ),
            "DD-2" => format!(
                "dstore: refstore: {}/refs holds a Pebble database written by Go dstore; dstore-client-rs keeps local references in redb and cannot open it (use another --local directory)\n",
                flag_value(&case.args, "local").unwrap_or_default()
            ),
            other => {
                problems.push(format!("{}: unknown node-side kind {other:?}", case.name));
                continue;
            }
        };
        let got = (
            ns.rust.exit,
            ns.rust.stdout.as_str(),
            ns.rust.stderr.as_str(),
        );
        if got != (1, "", want.as_str()) {
            problems.push(format!(
                "{}: node_side.rust is {got:?}, dstore_cli gives {want:?}",
                case.name
            ));
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
    assert_eq!(counts.values().sum::<usize>(), snaps.cases.len());
    for n in [Needs::Framework, Needs::Admin, Needs::Client, Needs::Wc] {
        assert!(counts.get(&n).copied().unwrap_or(0) > 0, "no {n:?} cases");
    }
}

/// The classification is what the framework does: `dstore_gocli::dispatch` over `dstore_cli::app()` reaches an
/// action for exactly the admin, client and wc cases, and writes nothing to stdout before it (Go prints nothing
/// before an action runs). This locks the command table's routing of every action case before layer L5 fills
/// the actions.
#[test]
fn classification_agrees_with_dispatch() {
    let snaps: Snapshots = load_json("cli/snapshots.json");
    let app = dstore_cli::app();
    let mut problems = Vec::new();
    let mut actions = 0usize;
    for case in &snaps.cases {
        let needs = match classify(case) {
            Ok(n) => n,
            Err(e) => {
                problems.push(e);
                continue;
            }
        };
        let (found, stdout) = dispatch_case(&app, case, "/cwd");
        let stdout = String::from_utf8_lossy(&stdout);
        match (needs, found) {
            (Needs::Framework, Ok(Some(_))) => {
                problems.push(format!("{}: the framework reached an action", case.name));
            }
            (Needs::Framework, _) => {}
            (_, Ok(Some(_))) if stdout.is_empty() => actions += 1,
            (_, Ok(Some(_))) => problems.push(format!(
                "{}: stdout before the action: {stdout:?}",
                case.name
            )),
            (_, other) => problems.push(format!(
                "{}: no action reached: {:?}, stdout {stdout:?}",
                case.name,
                other.map(|found| found.is_some())
            )),
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
    assert!(actions > 0, "no action cases");
}

/// `nodeside::local_ticket` against every Go outcome that the identity read of PORTING.md §2.2 B decides:
/// `cluster ticket`, `cluster status` and `catalog restore KEY` with `--store DIR` or `$DSTORE_STORE` and no
/// ticket. A missing directory gives `open …: no such file or directory`, an identity that is a directory
/// `read …: is a directory`, and a readable identity the §2.2 B substitute. The `--store` value is what the
/// framework parsed from the case, and the directory lies in the case's fixture.
#[test]
fn local_ticket_matches_the_identity_read_cases() {
    let snaps: Snapshots = load_json("cli/snapshots.json");
    let app = dstore_cli::app();
    let mut fixtures: BTreeMap<&str, Built> = BTreeMap::new();
    let mut checked = Vec::new();
    for case in &snaps.cases {
        let identity_read = match &case.node_side {
            Some(ns) => ns.kind == "B",
            None => case.stderr.starts_with("dstore: node: no identity in "),
        };
        if !identity_read {
            continue;
        }
        let built = fixtures
            .entry(case.fixture.as_str())
            .or_insert_with(|| build_in_temp(fixture_named(&snaps, &case.fixture)));
        let root_text = path_text(&built.root).expect("UTF-8 temp path");
        let ctx = action_context(&app, case, &root_text);
        // main.go: localTicket runs when --ticket is empty and --store is not.
        let store = ctx.os_string("store");
        assert!(
            ctx.string("ticket").is_empty() && !store.is_empty(),
            "{}: --ticket {:?}, --store {store:?}",
            case.name,
            ctx.string("ticket")
        );
        let dir = if store.as_bytes().starts_with(b"/") {
            store.as_bytes().to_vec()
        } else {
            [root_text.as_bytes(), b"/", store.as_bytes()].concat()
        };
        let e = match dstore_cli::nodeside::local_ticket(&dir) {
            Ok(t) => panic!("{}: local_ticket gave a ticket {t:?}", case.name),
            Err(e) => e,
        };
        let got = rendered(e).replace(&format!("{root_text}/"), "");
        let (exit, stdout, stderr) = case.expected();
        assert_eq!((exit, stdout), (1, ""), "{}", case.name);
        assert_eq!(got, stderr, "{}", case.name);
        checked.push(case.name.as_str());
    }
    // Missing directories (flags, `$DSTORE_STORE`, `catalog restore KEY`), a directory identity, and the readable
    // identities of kind B.
    assert!(checked.len() >= 10, "identity-read cases: {checked:?}");
}

/// `size::pack_size` against the node-side cases (`main.go` `openNode`, PORTING.md §2.2 A step 4): each Go
/// `--pack-size: …` error, from the flag or `$DSTORE_PACK_SIZE`, and a valid size for every kind-A case, which
/// passed Go's validation.
#[test]
fn pack_size_matches_the_node_side_cases() {
    let snaps: Snapshots = load_json("cli/snapshots.json");
    let app = dstore_cli::app();
    let (mut errors, mut valid) = (0usize, 0usize);
    for case in &snaps.cases {
        let kind_a = case.node_side.as_ref().is_some_and(|ns| ns.kind == "A");
        let pack_size_error =
            case.node_side.is_none() && case.stderr.starts_with("dstore: --pack-size: ");
        if !kind_a && !pack_size_error {
            continue;
        }
        let ctx = action_context(&app, case, "/cwd");
        let got = dstore_cli::size::pack_size(&ctx);
        if kind_a {
            assert!(got.is_ok(), "{}: {got:?}", case.name);
            valid += 1;
        } else {
            let want = case
                .stderr
                .strip_prefix("dstore: ")
                .and_then(|s| s.strip_suffix('\n'))
                .unwrap_or_default();
            assert_eq!(got, Err(want.to_string()), "{}", case.name);
            errors += 1;
        }
    }
    assert!(errors >= 5 && valid >= 8, "errors {errors}, valid {valid}");
}

/// Every fixture builds (VECTORS.md "Fixture steps"): plain files, modes and mtimes, core-rs ingest into a
/// scratch store or the working copy's packstore, `Tree::create`, `.dstore/state` as worktree `SaveState` writes
/// it, and the Pebble refs directory. The fixtures with a state open as working copies.
#[test]
fn fixture_builder_builds_every_fixture() {
    let snaps: Snapshots = load_json("cli/snapshots.json");
    let mut src_keys = Vec::new();
    for fixture in &snaps.fixtures {
        let built = build_in_temp(fixture);
        let root = &built.root;
        let var = |name: &str| -> String {
            built
                .vars
                .get(name)
                .cloned()
                .unwrap_or_else(|| panic!("{}: no variable {name}", fixture.name))
        };
        for step in &fixture.steps {
            match step {
                Step::Ingest { var: name, .. } => {
                    assert_eq!(var(name).len(), 64, "{}: {name}", fixture.name);
                }
                Step::WcState {
                    base_var,
                    remote_var,
                    remote_version_hex,
                    ..
                } => {
                    let state: serde_json::Value = serde_json::from_slice(
                        &fs::read(root.join(".dstore/state")).expect("read .dstore/state"),
                    )
                    .expect("state JSON");
                    let base = if base_var.is_empty() {
                        dstore_worktree::empty_tree().0.to_string()
                    } else {
                        var(base_var)
                    };
                    let (remote, version) = match remote_var {
                        Some(name) => (var(name), remote_version_hex.clone().unwrap_or_default()),
                        None => (String::new(), String::new()),
                    };
                    assert_eq!(
                        (
                            state["base"].as_str(),
                            state["remote"].as_str(),
                            state["remote_version"].as_str()
                        ),
                        (
                            Some(base.as_str()),
                            Some(remote.as_str()),
                            Some(version.as_str())
                        ),
                        "{}: {state}",
                        fixture.name
                    );
                }
                _ => {}
            }
        }
        if fixture
            .steps
            .iter()
            .any(|s| matches!(s, Step::WcState { .. }))
        {
            let tree = dstore_worktree::Tree::open(root.as_os_str().as_bytes())
                .unwrap_or_else(|e| panic!("{}: open the working copy: {e}", fixture.name));
            tree.close().expect("close the working copy");
        }
        if matches!(fixture.name.as_str(), "files" | "pebble-refs") {
            src_keys.push(var("src"));
            let hello = fs::symlink_metadata(root.join("src/hello.txt")).expect("hello.txt");
            assert_eq!(hello.permissions().mode() & 0o7777, 0o644);
            assert_eq!((hello.mtime(), hello.mtime_nsec()), (1_600_000_000, 0));
        }
        if fixture.name == "files" {
            assert_eq!(
                fs::read(root.join("backup.bin")).expect("backup.bin"),
                b"junk"
            );
            let identity = fs::symlink_metadata(root.join("node/identity")).expect("identity");
            assert_eq!(identity.permissions().mode() & 0o7777, 0o600);
            assert!(root.join("dirident/identity").is_dir());
        }
        if fixture.name == "pebble-refs" {
            let mut names: Vec<_> = fs::read_dir(root.join("P/refs"))
                .expect("P/refs")
                .map(|e| e.expect("entry").file_name())
                .collect();
            names.sort();
            assert_eq!(names.len(), 6, "{names:?}");
            assert!(names.iter().any(|n| n == "LOCK"));
        }
    }
    // The same source tree with the same times gives the same key.
    assert_eq!(src_keys.len(), 2);
    assert_eq!(src_keys[0], src_keys[1]);
}

#[test]
fn framework_cases() {
    run_cases(Needs::Framework);
}

#[test]
#[ignore = "needs dstore_cli::cmd_admin (layer L5)"]
fn admin_cases() {
    run_cases(Needs::Admin);
}

#[test]
#[ignore = "needs dstore_cli::cmd_client and dstore_cli::common (layer L5)"]
fn client_cases() {
    run_cases(Needs::Client);
}

#[test]
#[ignore = "needs dstore_cli::cmd_wc (layer L5)"]
fn wc_cases() {
    run_cases(Needs::Wc);
}

// ---- running cases ----

fn run_cases(needs: Needs) {
    let snaps: Snapshots = load_json("cli/snapshots.json");
    let fixtures: BTreeMap<&str, &Fixture> = snaps
        .fixtures
        .iter()
        .map(|f| (f.name.as_str(), f))
        .collect();
    let mut ran = 0usize;
    let mut failures = Vec::new();
    for case in &snaps.cases {
        match classify(case) {
            Ok(n) if n == needs => {}
            Ok(_) => continue,
            Err(e) => {
                failures.push(e);
                continue;
            }
        }
        ran += 1;
        let Some(fixture) = fixtures.get(case.fixture.as_str()) else {
            failures.push(format!("{}: unknown fixture {:?}", case.name, case.fixture));
            continue;
        };
        if let Err(e) = run_case(case, fixture) {
            failures.push(e);
        }
    }
    assert!(ran > 0, "no {needs:?} cases");
    assert!(
        failures.is_empty(),
        "{} of {ran} {needs:?} cases failed:\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

fn run_case(case: &Case, fixture: &Fixture) -> Result<(), String> {
    let fail = |what: &str, e: &dyn std::fmt::Display| format!("{}: {what}: {e}", case.name);
    let temp = |prefix: &str| {
        tempfile::Builder::new()
            .prefix(prefix)
            .tempdir()
            .map_err(|e| fail("temp dir", &e))
    };
    let root_dir = temp("dstore-snap-root-")?;
    let home_dir = temp("dstore-snap-home-")?;
    let scratch_dir = temp("dstore-snap-scratch-")?;
    let root = fs::canonicalize(root_dir.path()).map_err(|e| fail("resolve root", &e))?;
    // clisnap's HOME lies in its resolved work directory.
    let home = fs::canonicalize(home_dir.path()).map_err(|e| fail("resolve home", &e))?;
    let vars = build_fixture(&root, scratch_dir.path(), fixture)
        .map_err(|e| format!("{}: fixture {}: {e}", case.name, fixture.name))?;
    let cwd = fs::canonicalize(root.join(&case.subdir)).map_err(|e| fail("resolve cwd", &e))?;
    let cwd_text = path_text(&cwd)?;
    let root_text = path_text(&root)?;

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_dstore"));
    cmd.args(case.args.iter().map(|a| a.replace("{CWD}", &cwd_text)))
        .current_dir(&cwd)
        .env_clear();
    if let Some(path) = std::env::var_os("PATH") {
        cmd.env("PATH", path);
    }
    cmd.env("HOME", &home).env("TZ", "UTC");
    for e in &case.env {
        cmd.env(&e.name, e.value.replace("{CWD}", &cwd_text));
    }
    let output = run_with_timeout(cmd, case.stdin.as_bytes(), Duration::from_secs(60))
        .map_err(|e| fail("run", &e))?;
    let code = output
        .status
        .code()
        .ok_or_else(|| fail("terminated by a signal", &output.status))?;

    // clisnap `normalise`: `{ROOT}` only when cwd and ROOT differ.
    let subdir_root = (cwd_text != root_text).then_some(root_text.as_str());
    let stdout = normalise(&output.stdout, &cwd_text, subdir_root, &vars);
    let stderr = normalise(&output.stderr, &cwd_text, subdir_root, &vars);
    let (want_exit, want_stdout, want_stderr) = case.expected();
    let mut problems = Vec::new();
    if code != want_exit {
        problems.push(format!("exit {code}, want {want_exit}"));
    }
    if stdout != want_stdout.as_bytes() {
        problems.push(format!("stdout differs:\n{}", diff(want_stdout, &stdout)));
    }
    if stderr != want_stderr.as_bytes() {
        problems.push(format!("stderr differs:\n{}", diff(want_stderr, &stderr)));
    }
    if problems.is_empty() {
        return Ok(());
    }
    let env: Vec<String> = case
        .env
        .iter()
        .map(|e| format!("{}={}", e.name, e.value))
        .collect();
    Err(format!(
        "{} (args {:?}, env {env:?}, fixture {}, subdir {:?}):\n{}",
        case.name,
        case.args,
        case.fixture,
        case.subdir,
        problems.join("\n")
    ))
}

fn path_text(p: &Path) -> Result<String, String> {
    p.to_str()
        .map(str::to_string)
        .ok_or_else(|| format!("non-UTF-8 temp path {}", p.display()))
}

struct Output {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn run_with_timeout(mut cmd: Command, stdin: &[u8], limit: Duration) -> Result<Output, String> {
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("spawn {}: {e}", env!("CARGO_BIN_EXE_dstore")))?;
    let writer = child.stdin.take().map(|mut pipe| {
        let data = stdin.to_vec();
        std::thread::spawn(move || {
            // The command may exit without reading stdin; a broken pipe is not an error here.
            let _ = pipe.write_all(&data);
        })
    });
    let out_reader = child.stdout.take().map(read_in_thread);
    let err_reader = child.stderr.take().map(read_in_thread);
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() > limit => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("no exit within {limit:?}"));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(2)),
            Err(e) => return Err(format!("wait: {e}")),
        }
    };
    if let Some(w) = writer {
        let _ = w.join();
    }
    Ok(Output {
        status,
        stdout: join_reader(out_reader)?,
        stderr: join_reader(err_reader)?,
    })
}

fn read_in_thread(mut pipe: impl Read + Send + 'static) -> JoinHandle<std::io::Result<Vec<u8>>> {
    std::thread::spawn(move || {
        let mut b = Vec::new();
        pipe.read_to_end(&mut b).map(|_| b)
    })
}

fn join_reader(h: Option<JoinHandle<std::io::Result<Vec<u8>>>>) -> Result<Vec<u8>, String> {
    match h {
        None => Ok(Vec::new()),
        Some(h) => h
            .join()
            .map_err(|_| "reader thread panicked".to_string())?
            .map_err(|e| format!("read: {e}")),
    }
}

/// VECTORS.md "Running a case" step 7, in that order.
fn normalise(
    out: &[u8],
    cwd: &str,
    subdir_root: Option<&str>,
    vars: &BTreeMap<String, String>,
) -> Vec<u8> {
    let mut b = replace_all(out, cwd.as_bytes(), b"{CWD}");
    if let Some(root) = subdir_root {
        b = replace_all(&b, root.as_bytes(), b"{ROOT}");
    }
    for (name, key) in vars {
        b = replace_all(&b, key.as_bytes(), format!("{{key:{name}}}").as_bytes());
        let short = key.get(..16).unwrap_or(key);
        b = replace_all(&b, short.as_bytes(), format!("{{key16:{name}}}").as_bytes());
    }
    b
}

fn replace_all(hay: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    if from.is_empty() {
        return hay.to_vec();
    }
    let mut out = Vec::with_capacity(hay.len());
    let mut rest = hay;
    while !rest.is_empty() {
        if rest.starts_with(from) {
            out.extend_from_slice(to);
            rest = &rest[from.len()..];
        } else {
            out.push(rest[0]);
            rest = &rest[1..];
        }
    }
    out
}

/// A unified diff, then the first differing line escaped (trailing spaces are invisible in a diff).
fn diff(want: &str, got: &[u8]) -> String {
    let got = String::from_utf8_lossy(got);
    let mut text = similar::TextDiff::from_lines(want, &*got)
        .unified_diff()
        .header("want", "got")
        .to_string();
    let first = want
        .split_inclusive('\n')
        .zip(got.split_inclusive('\n'))
        .find(|(w, g)| w != g);
    match first {
        Some((w, g)) => text.push_str(&format!("first difference: want {w:?}, got {g:?}\n")),
        None => text.push_str(&format!(
            "want {} bytes, got {} bytes\n",
            want.len(),
            got.len()
        )),
    }
    text
}

// ---- fixtures ----

/// Builds a fixture in `root` (a resolved temporary directory) and returns its key variables (64 hex).
/// `scratch` holds the throwaway packstore of `ingest into scratch`.
fn build_fixture(
    root: &Path,
    scratch: &Path,
    fixture: &Fixture,
) -> Result<BTreeMap<String, String>, String> {
    let mut vars = BTreeMap::new();
    for (i, step) in fixture.steps.iter().enumerate() {
        apply_step(root, scratch, step, &mut vars)
            .map_err(|e| format!("step {i} {step:?}: {e}"))?;
    }
    Ok(vars)
}

fn apply_step(
    root: &Path,
    scratch: &Path,
    step: &Step,
    vars: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    let io = |e: std::io::Error| e.to_string();
    match step {
        Step::Mkdir { path, mode } => {
            fs::create_dir(root.join(path)).map_err(io)?;
            chmod(&root.join(path), *mode)
        }
        Step::Write { path, text, mode } => {
            fs::write(root.join(path), text).map_err(io)?;
            chmod(&root.join(path), *mode)
        }
        Step::Symlink { path, target } => {
            std::os::unix::fs::symlink(target, root.join(path)).map_err(io)
        }
        // `os.RemoveAll`: a missing path is not an error.
        Step::Remove { path } => {
            let p = root.join(path);
            match fs::symlink_metadata(&p) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e.to_string()),
                Ok(md) if md.is_dir() => fs::remove_dir_all(&p).map_err(io),
                Ok(_) => fs::remove_file(&p).map_err(io),
            }
        }
        Step::Chmod { path, mode } => chmod(&root.join(path), *mode),
        Step::Mtime { path, unix_ns } => set_mtime(&root.join(path), *unix_ns),
        Step::WcCreate { config } => {
            let cfg = dstore_worktree::Config {
                ticket: config.ticket.clone().into_bytes(),
                name: config.name.clone().into_bytes(),
                relay: config.relay.clone().into_bytes(),
                no_relay: config.no_relay,
                no_discovery: config.no_discovery,
                user: config.user.clone().into_bytes(),
            };
            let tree = dstore_worktree::Tree::create(root.as_os_str().as_bytes(), cfg)
                .map_err(|e| e.to_string())?;
            tree.close().map_err(|e| e.to_string())
        }
        Step::Ingest {
            dir,
            into,
            exclude,
            var,
        } => {
            let store_dir = match into.as_str() {
                "wc" => root.join(".dstore").join("packstore"),
                "scratch" => scratch.join("packstore"),
                other => return Err(format!("unknown ingest target {other:?}")),
            };
            let st = packstore::Store::open_with(&store_dir, packstore::Options::new().sync(true))
                .map_err(|e| e.to_string())?;
            let src = if dir == "." {
                root.to_path_buf()
            } else {
                root.join(dir)
            };
            let opts = ingest::Opts {
                jobs: 1,
                exclude: exclude.iter().map(OsString::from).collect(),
                ..Default::default()
            };
            let (_stats, key) = ingest::dir(&st, &src, opts);
            let key = key.map_err(|e| e.to_string())?;
            vars.insert(var.clone(), key.to_string());
            drop(st);
            Ok(())
        }
        // clisnap: `(&worktree.Tree{Root: root, State: st}).SaveState()`. The Rust `Tree` holds its store, so the
        // step opens the working copy's packstore (created by `wc_create`) and closes it again.
        Step::WcState {
            base_var,
            remote_var,
            remote_version_hex,
            synced_at_unix_ns,
        } => {
            let key_of = |name: &str| -> Result<Key, String> {
                let hex = vars
                    .get(name)
                    .ok_or_else(|| format!("unknown variable {name:?}"))?;
                Key::parse(&golden::hex(hex)).map_err(|e| e.to_string())
            };
            let base = if base_var.is_empty() {
                dstore_worktree::empty_tree().0
            } else {
                key_of(base_var)?
            };
            let (remote, has_remote, remote_version) = match remote_var {
                Some(name) => (
                    key_of(name)?,
                    true,
                    Some(golden::hex(
                        remote_version_hex.as_deref().unwrap_or_default(),
                    )),
                ),
                None => (Key([0; 32]), false, None),
            };
            let store = packstore::Store::open_with(
                root.join(".dstore").join("packstore"),
                packstore::Options::new().sync(true),
            )
            .map_err(|e| e.to_string())?;
            let tree = dstore_worktree::Tree {
                root: root.as_os_str().as_bytes().to_vec(),
                config: dstore_worktree::Config::default(),
                state: dstore_worktree::State {
                    base,
                    remote,
                    has_remote,
                    remote_version,
                    synced_at: dstore_gocompat::time::GoTime::from_unix_nano(*synced_at_unix_ns),
                },
                store: Arc::new(store),
            };
            tree.save_state().map_err(|e| e.to_string())?;
            tree.close().map_err(|e| e.to_string())
        }
        Step::PebbleRefs { path, names } => {
            let dir = root.join(path);
            fs::create_dir_all(&dir).map_err(io)?;
            for n in names {
                fs::File::create(dir.join(n)).map_err(io)?;
            }
            Ok(())
        }
    }
}

fn chmod(p: &Path, mode: u32) -> Result<(), String> {
    fs::set_permissions(p, fs::Permissions::from_mode(mode)).map_err(|e| e.to_string())
}

/// `utimensat(AT_FDCWD, path, [unix_ns, unix_ns], AT_SYMLINK_NOFOLLOW)`. rustix's `Nsecs` is 64 bits on
/// every supported target (PORTING.md §5.13).
fn set_mtime(p: &Path, unix_ns: i64) -> Result<(), String> {
    let ts = || rustix::fs::Timespec {
        tv_sec: unix_ns.div_euclid(1_000_000_000),
        tv_nsec: unix_ns.rem_euclid(1_000_000_000),
    };
    let times = rustix::fs::Timestamps {
        last_access: ts(),
        last_modification: ts(),
    };
    rustix::fs::utimensat(
        rustix::fs::CWD,
        p,
        &times,
        rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
    )
    .map_err(|e| e.to_string())
}
