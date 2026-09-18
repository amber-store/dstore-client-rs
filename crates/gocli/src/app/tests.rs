//! Tests of `run`/`dispatch` and `Context`: Go-verified outputs of a dstore-shaped test app
//! (`go_cases.rs`), and the urfave/cli v2.27.7 tests of cli.md §6 that exercise features dstore uses.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::task::{Context as TaskContext, Poll, Waker};

use dstore_gocompat::quote::quote;
use dstore_gocompat::strconv::format_float_g;

use super::*;
use crate::{FlagKind, FlagValue};

/// A case of the throwaway urfave/cli harness: the app of `test_app`, run in a fresh Go process.
struct GoCase {
    args: &'static [&'static [u8]],
    env: &'static [(&'static str, &'static [u8])],
    exit: i32,
    stdout: &'static str,
    stderr: &'static str,
    /// For a case that ran an action: the flag names whose `report` lines were kept.
    probes: &'static [&'static str],
}

include!("go_cases.rs");
include!("go_cases_review.rs");

thread_local! {
    static LABEL: RefCell<String> = const { RefCell::new(String::new()) };
}

/// The harness actions' results: `fail` → a plain error, `exit5` → `cli.Exit("exit five", 5)`.
fn outcome(c: &Context) -> Result<(), CliError> {
    match c.first().to_str() {
        Some("fail") => Err(CliError::Msg("action failed".into())),
        Some("exit5") => Err(CliError::Exit {
            msg: "exit five".into(),
            code: 5,
        }),
        _ => Ok(()),
    }
}

macro_rules! action {
    ($fname:ident, $label:expr) => {
        fn $fname(c: &Context) -> Pin<Box<dyn Future<Output = Result<(), CliError>> + Send + '_>> {
            LABEL.with(|l| *l.borrow_mut() = String::from($label));
            Box::pin(async move { outcome(c) })
        }
    };
}

action!(a_cluster_init, "cluster init");
action!(a_cluster_status, "cluster status");
action!(a_cluster_replicas, "cluster replicas");
action!(a_leaf, "cluster deep leaf");
action!(a_token_create, "token create");
action!(a_node_join, "node join");
action!(a_node_remove, "node remove");
action!(a_node_drain, "node drain");
action!(a_store_push, "store push");
action!(a_store_pull, "store pull");
action!(a_refs, "refs");
action!(a_watch, "watch");
action!(a_diff, "diff");
action!(a_alias, "alias");
action!(a_unicode, "ünï");
action!(a_empty, "empty");
action!(a_cmd, "cmd");

fn f(
    name: &'static str,
    kind: FlagKind,
    usage: &'static str,
    env: &'static [&'static str],
    required: bool,
) -> FlagDef {
    FlagDef {
        name,
        aliases: &[],
        kind,
        usage,
        env,
        required,
        disable_default_text: false,
    }
}

fn s(default: &'static str) -> FlagKind {
    FlagKind::String { default }
}

fn b(default: bool) -> FlagKind {
    FlagKind::Bool { default }
}

fn cmd(
    name: &'static str,
    usage: &'static str,
    args_usage: &'static str,
    flags: Vec<FlagDef>,
    action: Option<Action>,
) -> CommandDef {
    CommandDef {
        name,
        aliases: &[],
        usage,
        args_usage,
        description: "",
        flags,
        subcommands: Vec::new(),
        action,
    }
}

fn parent(name: &'static str, usage: &'static str, subcommands: Vec<CommandDef>) -> CommandDef {
    CommandDef {
        name,
        aliases: &[],
        usage,
        args_usage: "",
        description: "",
        flags: Vec::new(),
        subcommands,
        action: None,
    }
}

fn plus(mut a: Vec<FlagDef>, more: Vec<FlagDef>) -> Vec<FlagDef> {
    a.extend(more);
    a
}

fn client_flags() -> Vec<FlagDef> {
    vec![
        f(
            "ticket",
            s(""),
            "cluster ticket (dstore1…) or comma-separated node ids, found by discovery",
            &["DSTORE_TICKET"],
            false,
        ),
        f(
            "relay",
            s(""),
            "relay URL for the fallback path (default: the built-in relay map)",
            &[],
            false,
        ),
        f(
            "no-relay",
            b(false),
            "direct addresses only, no relay",
            &[],
            false,
        ),
        f(
            "no-discovery",
            b(false),
            "neither announce this endpoint nor resolve node ids by discovery (mDNS and, with relays, number0's DNS)",
            &["DSTORE_NO_DISCOVERY"],
            false,
        ),
    ]
}

fn store_flag() -> FlagDef {
    f("store", s(""), "store directory", &["DSTORE_STORE"], false)
}

fn node_flags() -> Vec<FlagDef> {
    vec![
        store_flag(),
        f(
            "paxos-dir",
            s(""),
            "acceptor state directory (default <store>/paxos; put it on its own device)",
            &[],
            false,
        ),
        f(
            "rate",
            FlagKind::Int64 { default: 0 },
            "reconcile copy rate in bytes/s (0 = unlimited)",
            &[],
            false,
        ),
        f(
            "jobs",
            FlagKind::Int { default: 0 },
            "parallelism (0 = cores)",
            &[],
            false,
        ),
        f(
            "pack-size",
            s("2Gi"),
            "size at which the active pack is sealed (bytes or Ki/Mi/Gi/Ti); applies to packs written from now on",
            &["DSTORE_PACK_SIZE"],
            false,
        ),
        f(
            "gateway",
            b(false),
            "also serve the transport-iroh ALPN (not implemented in this version)",
            &[],
            false,
        ),
        f(
            "gc-interval",
            FlagKind::Duration {
                default_ns: 4 * 3_600_000_000_000,
            },
            "",
            &[],
            false,
        ),
        f(
            "put-ttl",
            FlagKind::Duration {
                default_ns: 3_600_000_000_000,
            },
            "",
            &[],
            false,
        ),
        f(
            "advertise-addr",
            FlagKind::StringSlice,
            "direct address to advertise, ip or ip:port (repeatable)",
            &[],
            false,
        ),
        f(
            "no-discovery",
            b(false),
            "neither announce this endpoint nor resolve node ids by discovery (mDNS and, with relays, number0's DNS)",
            &["DSTORE_NO_DISCOVERY"],
            false,
        ),
        f(
            "replicas",
            FlagKind::Int { default: 3 },
            "R: owners per object",
            &[],
            false,
        ),
        f("weight", s("auto"), "capacity in GiB, or auto", &[], false),
        f("zone", s(""), "", &[], false),
        f("allow-unsafe", b(false), "allow min-replicas 1", &[], false),
    ]
}

fn local_flag() -> FlagDef {
    f(
        "local",
        s(""),
        "local store directory (layout: <dir>/packstore, <dir>/refs)",
        &["AMBER_STORE"],
        true,
    )
}

fn no_tui_flag() -> FlagDef {
    f(
        "no-tui",
        b(false),
        "plain log lines instead of the progress display",
        &["DSTORE_NO_TUI"],
        false,
    )
}

/// The app of the Go harness, definition for definition.
fn test_app() -> AppDef {
    AppDef {
        name: "dstore",
        usage: "a distributed amber store: cluster nodes and the client",
        version: "dev".into(),
        flags: vec![f(
            "log-level",
            s("info"),
            "debug|info|warn|error (a global flag: give it before the command)",
            &["DSTORE_LOG_LEVEL"],
            false,
        )],
        commands: vec![
            parent(
                "cluster",
                "init, status, ticket, replicas",
                vec![
                    cmd(
                        "init",
                        "create a cluster on this store and print its ticket; then run serve",
                        "",
                        node_flags(),
                        Some(a_cluster_init),
                    ),
                    cmd(
                        "status",
                        "view, epoch, reachability, disk, transition, gc, voters",
                        "",
                        plus(client_flags(), vec![store_flag()]),
                        Some(a_cluster_status),
                    ),
                    cmd(
                        "replicas",
                        "change R (a transition that copies 1/R of the store)",
                        "R",
                        plus(
                            client_flags(),
                            vec![f("yes", b(false), "do not ask", &[], false)],
                        ),
                        Some(a_cluster_replicas),
                    ),
                    parent(
                        "deep",
                        "a nested parent",
                        vec![cmd(
                            "leaf",
                            "",
                            "X",
                            vec![
                                f("level", FlagKind::Uint { default: 7 }, "depth", &[], false),
                                f(
                                    "ratio",
                                    FlagKind::Float64 { default: 0.25 },
                                    "dead ratio",
                                    &["RATIO"],
                                    false,
                                ),
                            ],
                            Some(a_leaf),
                        )],
                    ),
                ],
            ),
            parent(
                "token",
                "join tokens",
                vec![cmd(
                    "create",
                    "create a single-use join token",
                    "",
                    plus(
                        client_flags(),
                        vec![f(
                            "weight",
                            FlagKind::Uint { default: 0 },
                            "weight the token imposes on the joiner (GiB)",
                            &[],
                            false,
                        )],
                    ),
                    Some(a_token_create),
                )],
            ),
            parent(
                "node",
                "join, remove, drain",
                vec![
                    cmd(
                        "join",
                        "join a cluster with this store and keep serving",
                        "",
                        plus(
                            node_flags(),
                            vec![
                                f(
                                    "seed",
                                    s(""),
                                    "cluster ticket (dstore1…) or comma-separated node ids, found by discovery",
                                    &[],
                                    true,
                                ),
                                f("token", s(""), "join token (hex)", &[], true),
                                f(
                                    "no-vote",
                                    b(false),
                                    "hold no catalog (a relay-only or archive box)",
                                    &[],
                                    false,
                                ),
                            ],
                        ),
                        Some(a_node_join),
                    ),
                    cmd(
                        "remove",
                        "",
                        "ID",
                        plus(
                            client_flags(),
                            vec![
                                f(
                                    "dead",
                                    b(false),
                                    "the node is gone: remove its vote first",
                                    &[],
                                    false,
                                ),
                                f("allow-unsafe", b(false), "", &[], false),
                            ],
                        ),
                        Some(a_node_remove),
                    ),
                    cmd("drain", "", "ID", client_flags(), Some(a_node_drain)),
                ],
            ),
            parent(
                "store",
                "push and pull between a standalone local store and the cluster",
                vec![
                    cmd(
                        "push",
                        "build a tree from PATH into the local store and push it under NAME",
                        "PATH NAME",
                        plus(
                            client_flags(),
                            vec![
                                local_flag(),
                                f(
                                    "user",
                                    s(""),
                                    "user identity recorded in the reference",
                                    &[],
                                    false,
                                ),
                                f("force", b(false), "replace unconditionally", &[], false),
                                f(
                                    "expected-version",
                                    s(""),
                                    "CAS: the version ref get printed (hex); omit to require the name to be new",
                                    &[],
                                    false,
                                ),
                                f("jobs", FlagKind::Int { default: 0 }, "", &[], false),
                                no_tui_flag(),
                            ],
                        ),
                        Some(a_store_push),
                    ),
                    cmd(
                        "pull",
                        "pull the tree under NAME into the local store",
                        "NAME",
                        plus(
                            client_flags(),
                            vec![
                                local_flag(),
                                f("jobs", FlagKind::Int { default: 0 }, "", &[], false),
                                no_tui_flag(),
                            ],
                        ),
                        Some(a_store_pull),
                    ),
                ],
            ),
            cmd(
                "refs",
                "list references",
                "[PREFIX]",
                client_flags(),
                Some(a_refs),
            ),
            CommandDef {
                name: "watch",
                aliases: &[],
                usage: "watch references matching a glob and print each change until interrupted",
                args_usage: "PATTERN",
                description: "PATTERN is path-style: * and ? match within one /-separated segment.\nEvery matching reference is printed first.",
                flags: client_flags(),
                subcommands: Vec::new(),
                action: Some(a_watch),
            },
            cmd(
                "diff",
                "unified diffs of the working directory against the last synced tree",
                "[PATH...]",
                vec![
                    f(
                        "remote",
                        b(false),
                        "against the tree last fetched from the cluster",
                        &[],
                        false,
                    ),
                    f(
                        "incoming",
                        b(false),
                        "the last synced tree against the fetched one (what pull would apply)",
                        &[],
                        false,
                    ),
                    f(
                        "stat",
                        b(false),
                        "one line per changed path with line counts",
                        &[],
                        false,
                    ),
                    f(
                        "jobs",
                        FlagKind::Int { default: 0 },
                        "parallelism (0 = cores)",
                        &[],
                        false,
                    ),
                ],
                Some(a_diff),
            ),
            CommandDef {
                name: "alias",
                aliases: &["al", "a"],
                usage: "a command with aliases and every flag kind",
                args_usage: "",
                description: "",
                flags: vec![
                    FlagDef {
                        name: "count",
                        aliases: &["c", "n"],
                        kind: FlagKind::Int64 { default: -2 },
                        usage: "how many `N` to take",
                        env: &[],
                        required: false,
                        disable_default_text: false,
                    },
                    FlagDef {
                        name: "names",
                        aliases: &["N"],
                        kind: FlagKind::StringSlice,
                        usage: "names, comma-separated",
                        env: &["NAMES"],
                        required: false,
                        disable_default_text: false,
                    },
                    FlagDef {
                        name: "verbose",
                        aliases: &["V"],
                        kind: b(true),
                        usage: "",
                        env: &[],
                        required: false,
                        disable_default_text: false,
                    },
                    f(
                        "dur",
                        FlagKind::Duration {
                            default_ns: 90_000_000_000,
                        },
                        "a duration",
                        &["DUR"],
                        false,
                    ),
                    f(
                        "f64",
                        FlagKind::Float64 { default: 1e21 },
                        "",
                        &["F64", "F64B"],
                        false,
                    ),
                    f("i64", FlagKind::Int64 { default: 0 }, "", &["I64"], false),
                    f("u", FlagKind::Uint { default: 0 }, "", &["U"], false),
                    f("i", FlagKind::Int { default: 0 }, "", &["I"], true),
                ],
                subcommands: Vec::new(),
                action: Some(a_alias),
            },
            cmd(
                "ünï",
                "UTF-8 names µ",
                "",
                vec![
                    f("µ", s("µ"), "rune widths", &[], false),
                    f("ascii-flag-name", s(""), "wider in bytes", &[], false),
                ],
                Some(a_unicode),
            ),
            cmd("empty", "", "", Vec::new(), Some(a_empty)),
            CommandDef {
                name: "wrap",
                aliases: &[],
                usage: "first line\nsecond line",
                args_usage: "A\nB",
                description: "para one\n\n   indented",
                flags: vec![f("multi", s("x\ny"), "line one\nline two", &[], false)],
                subcommands: Vec::new(),
                action: None,
            },
        ],
    }
}

/// The flag names the harness action probed, in its order.
const PROBE_NAMES: &[&str] = &[
    "log-level",
    "store",
    "paxos-dir",
    "rate",
    "jobs",
    "pack-size",
    "gateway",
    "gc-interval",
    "put-ttl",
    "advertise-addr",
    "no-discovery",
    "replicas",
    "weight",
    "zone",
    "allow-unsafe",
    "ticket",
    "relay",
    "no-relay",
    "yes",
    "level",
    "ratio",
    "seed",
    "token",
    "no-vote",
    "dead",
    "local",
    "user",
    "force",
    "expected-version",
    "no-tui",
    "remote",
    "incoming",
    "stat",
    "count",
    "c",
    "n",
    "names",
    "N",
    "verbose",
    "V",
    "dur",
    "f64",
    "i64",
    "u",
    "i",
    "µ",
    "ascii-flag-name",
    "multi",
    "help",
    "h",
    "version",
    "v",
    "bogus",
];

fn quote_slice<T: AsRef<[u8]>>(items: &[T]) -> String {
    let parts: Vec<String> = items.iter().map(|i| quote(i.as_ref())).collect();
    format!("[{}]", parts.join(" "))
}

/// The harness action's output over the kept probes: Go `%q`, `%v`, `%d` and `FormatFloat('g', -1)` of
/// every accessor.
fn report(label: &str, c: &Context, probes: &[&str]) -> String {
    let args: Vec<Vec<u8>> = c.args().iter().map(|a| a.clone().into_vec()).collect();
    let mut w = format!(
        "action {label} narg={} args={} first={}\n",
        c.narg(),
        quote_slice(&args),
        quote(&c.first().to_os_string().into_vec())
    );
    for name in PROBE_NAMES.iter().filter(|n| probes.contains(n)) {
        w.push_str(&format!(
            "  {name} set={} string={} bool={} int={} int64={} uint={} float={} dur={} slice={}\n",
            c.is_set(name),
            quote(&c.os_string(name).into_vec()),
            c.bool(name),
            c.int(name),
            c.int64(name),
            c.uint(name),
            format_float_g(c.float64(name)),
            c.duration_ns(name),
            quote_slice(&c.string_slice(name)),
        ));
    }
    w
}

fn poll_ready<F: Future>(fut: F) -> F::Output {
    let mut fut = std::pin::pin!(fut);
    let mut cx = TaskContext::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
            return v;
        }
    }
}

/// What a run printed and returned, mapped the way dstore's `main` prints errors.
struct Run {
    exit: i32,
    stdout: Vec<u8>,
    stderr: String,
    ctx: Option<Context>,
    label: String,
}

fn run_case(app: &AppDef, args: &[&[u8]], env: &[(&str, &[u8])]) -> Run {
    let mut argv = vec![OsString::from("dstore")];
    argv.extend(args.iter().map(|a| OsString::from_vec(a.to_vec())));
    let env: HashMap<&str, &[u8]> = env.iter().copied().collect();
    let getenv = |name: &str| env.get(name).map(|v| OsString::from_vec(v.to_vec()));
    let mut stdout = Vec::new();
    LABEL.with(|l| l.borrow_mut().clear());
    let (result, ctx) = match dispatch(app, argv, &mut stdout, &getenv) {
        Ok(None) => (Ok(()), None),
        Ok(Some((action, ctx))) => {
            let result = poll_ready(action(&ctx));
            (result, Some(ctx))
        }
        Err(e) => (Err(e), None),
    };
    let (exit, stderr) = match result {
        Ok(()) => (0, String::new()),
        Err(CliError::Msg(m)) => (1, format!("dstore: {m}\n")),
        Err(CliError::Exit { msg, code }) => (code, format!("{msg}\n")),
    };
    Run {
        exit,
        stdout,
        stderr,
        ctx,
        label: LABEL.with(|l| l.borrow().clone()),
    }
}

fn args_of(args: &[&str]) -> Vec<&'static [u8]> {
    args.iter()
        .map(|a| &*Box::leak(a.as_bytes().to_vec().into_boxed_slice()))
        .collect()
}

#[test]
fn go_verified_cases() {
    assert!(GO_CASES.len() >= 100);
    check_go_cases(GO_CASES);
}

/// The review's Go-verified cases: help topics through `--help`/`-h`, nested parents, `help` twice, `--`
/// before a command, root flags and empty environment values, aliases and `normalizeFlags`, and the
/// number, duration, float and slice edge cases of every flag kind.
#[test]
fn go_verified_review_cases() {
    assert!(GO_CASES_REVIEW.len() >= 100);
    check_go_cases(GO_CASES_REVIEW);
}

fn check_go_cases(cases: &[GoCase]) {
    let app = test_app();
    for case in cases {
        let what = format!(
            "{:?} env {:?}",
            case.args
                .iter()
                .map(|a| String::from_utf8_lossy(a))
                .collect::<Vec<_>>(),
            case.env
                .iter()
                .map(|(k, v)| (*k, String::from_utf8_lossy(v)))
                .collect::<Vec<_>>()
        );
        let run = run_case(&app, case.args, case.env);
        assert_eq!(run.exit, case.exit, "exit of {what}");
        assert_eq!(run.stderr, case.stderr, "stderr of {what}");
        let mut stdout = run.stdout;
        match &run.ctx {
            Some(ctx) => {
                assert!(!case.probes.is_empty(), "{what} ran an action");
                stdout.extend_from_slice(report(&run.label, ctx, case.probes).as_bytes());
            }
            None => assert!(case.probes.is_empty(), "{what} ran no action"),
        }
        assert_eq!(
            String::from_utf8(stdout).ok().as_deref(),
            Some(case.stdout),
            "stdout of {what}"
        );
    }
}

/// The help default text is the definition default, not the environment value.
#[test]
fn help_default_text_ignores_env() {
    let app = test_app();
    let plain = run_case(&app, &args_of(&["cluster", "init", "--help"]), &[]);
    let with_env = run_case(
        &app,
        &args_of(&["cluster", "init", "--help"]),
        &[("DSTORE_PACK_SIZE", b"1Gi")],
    );
    assert_eq!(plain.exit, 0);
    assert_eq!(plain.stdout, with_env.stdout);
    assert!(
        String::from_utf8_lossy(&plain.stdout).contains("(default: \"2Gi\") [$DSTORE_PACK_SIZE]")
    );
}

/// Non-UTF-8 argument bytes are written raw on stdout and lossily in the returned error (DD-8).
#[test]
fn non_utf8_argument_in_usage_error() {
    let app = test_app();
    let run = run_case(&app, &[b"refs", b"--b\xffx"], &[]);
    assert_eq!(run.exit, 1);
    assert!(
        run.stdout
            .starts_with(b"Incorrect Usage: flag provided but not defined: -b\xffx\n\n")
    );
    assert_eq!(
        run.stderr,
        "dstore: flag provided but not defined: -b\u{FFFD}x\n"
    );
    let run = run_case(&app, &[b"cluster", b"\xff"], &[]);
    assert_eq!(run.exit, 3);
    assert_eq!(run.stderr, "No help topic for '\u{FFFD}'\n");
}

fn one_command(flags: Vec<FlagDef>) -> AppDef {
    AppDef {
        name: "test",
        usage: "",
        version: String::new(),
        flags: Vec::new(),
        commands: vec![cmd("cmd", "", "", flags, Some(a_cmd))],
    }
}

fn context_of(
    app: &AppDef,
    args: &[&str],
    env: &[(&str, &[u8])],
) -> Result<Option<Context>, CliError> {
    let mut argv = vec![OsString::from("x")];
    argv.extend(args.iter().map(OsString::from));
    let env: HashMap<&str, &[u8]> = env.iter().copied().collect();
    let getenv = |name: &str| env.get(name).map(|v| OsString::from_vec(v.to_vec()));
    dispatch(app, argv, &mut Vec::new(), &getenv).map(|found| found.map(|(_, ctx)| ctx))
}

fn os_args(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

/// urfave app_test.go `TestApp_CommandWithFlagBeforeTerminator`, `TestApp_CommandWithDash`,
/// `TestApp_CommandWithNoFlagBeforeTerminator`.
#[test]
fn urfave_command_arguments() {
    let app = one_command(vec![f("option", s(""), "some option", &[], false)]);
    let ctx = context_of(
        &app,
        &[
            "cmd",
            "--option",
            "my-option",
            "my-arg",
            "--",
            "--notARealFlag",
        ],
        &[],
    );
    let Ok(Some(ctx)) = ctx else {
        panic!("no action");
    };
    assert_eq!(ctx.string("option"), "my-option");
    assert_eq!(
        ctx.args(),
        &os_args(&["my-arg", "--", "--notARealFlag"])[..]
    );
    assert_eq!(ctx.arg(1), "--");
    assert_eq!(ctx.arg(3), "");

    let app = one_command(Vec::new());
    let Ok(Some(ctx)) = context_of(&app, &["cmd", "my-arg", "-"], &[]) else {
        panic!("no action");
    };
    assert_eq!(ctx.args(), &os_args(&["my-arg", "-"])[..]);
    let Ok(Some(ctx)) = context_of(&app, &["cmd", "my-arg", "--", "notAFlagAtAll"], &[]) else {
        panic!("no action");
    };
    assert_eq!(ctx.args(), &os_args(&["my-arg", "--", "notAFlagAtAll"])[..]);
    assert_eq!(ctx.narg(), 3);
    assert_eq!(ctx.first(), "my-arg");
}

/// urfave app_test.go `TestApp_ParseSliceFlags` (the string slice) and flag_test.go
/// `TestStringSliceFlag_TrimSpace` (trimmed values).
#[test]
fn urfave_slice_flags() {
    let app = one_command(vec![f(
        "ip",
        FlagKind::StringSlice,
        "set one or more ports to open",
        &[],
        false,
    )]);
    let Ok(Some(ctx)) = context_of(&app, &["cmd", "-ip", "8.8.8.8", "-ip", "8.8.4.4"], &[]) else {
        panic!("no action");
    };
    assert_eq!(ctx.string_slice("ip"), vec!["8.8.8.8", "8.8.4.4"]);
    let app = one_command(vec![f("trim", FlagKind::StringSlice, "", &[], false)]);
    for (input, want) in [(" asd", "asd"), ("123 ", "123"), (" asd ", "asd")] {
        let Ok(Some(ctx)) = context_of(&app, &["cmd", "--trim", input], &[]) else {
            panic!("no action");
        };
        assert_eq!(ctx.string_slice("trim"), vec![want]);
    }
}

/// urfave app_test.go `TestRequiredFlagAppRunBehavior`, the rows that do not put flags on the app
/// (`AppDef` flags take part in the same check; the root has no action of its own here).
#[test]
fn urfave_required_flag_app_run_behavior() {
    let required = || f("requiredFlag", s(""), "", &[], true);
    let optional = || f("optional", s(""), "", &[], false);
    let on_command = |flags: Vec<FlagDef>| AppDef {
        name: "myCLI",
        usage: "",
        version: String::new(),
        flags: Vec::new(),
        commands: vec![cmd("myCommand", "", "", flags, Some(a_cmd))],
    };
    let on_subcommand = |flags: Vec<FlagDef>| AppDef {
        name: "myCLI",
        usage: "",
        version: String::new(),
        flags: Vec::new(),
        commands: vec![parent(
            "myCommand",
            "",
            vec![cmd("mySubCommand", "", "", flags, Some(a_cmd))],
        )],
    };
    let cases: Vec<(&str, AppDef, &[&str], bool)> = vec![
        (
            "error_case_empty_input_with_required_flag_on_command",
            on_command(vec![required()]),
            &["myCommand"],
            true,
        ),
        (
            "error_case_empty_input_with_required_flag_on_subcommand",
            on_subcommand(vec![required()]),
            &["myCommand", "mySubCommand"],
            true,
        ),
        (
            "valid_case_help_input_with_required_flag_on_command",
            on_command(vec![required()]),
            &["myCommand", "--help"],
            false,
        ),
        (
            "valid_case_help_input_with_required_flag_on_subcommand",
            on_subcommand(vec![required()]),
            &["myCommand", "mySubCommand", "--help"],
            false,
        ),
        (
            "error_case_optional_input_with_required_flag_on_command",
            on_command(vec![required(), optional()]),
            &["myCommand", "--optional", "cats"],
            true,
        ),
        (
            "error_case_optional_input_with_required_flag_on_subcommand",
            on_subcommand(vec![required(), optional()]),
            &["myCommand", "mySubCommand", "--optional", "cats"],
            true,
        ),
        (
            "valid_case_required_flag_input_on_command",
            on_command(vec![required()]),
            &["myCommand", "--requiredFlag", "cats"],
            false,
        ),
        (
            "valid_case_required_flag_input_on_subcommand",
            on_subcommand(vec![required()]),
            &["myCommand", "mySubCommand", "--requiredFlag", "cats"],
            false,
        ),
    ];
    for (name, app, args, want_err) in cases {
        let result = context_of(&app, args, &[]);
        assert_eq!(result.is_err(), want_err, "{name}");
    }
    // A required flag on the app itself.
    let app = AppDef {
        name: "myCLI",
        usage: "",
        version: String::new(),
        flags: vec![required(), optional()],
        commands: Vec::new(),
    };
    assert!(context_of(&app, &[], &[]).is_err());
    assert!(context_of(&app, &["--optional", "cats"], &[]).is_err());
    assert!(matches!(context_of(&app, &["--help"], &[]), Ok(None)));
    assert!(matches!(
        context_of(&app, &["--requiredFlag", "cats"], &[]),
        Ok(None)
    ));
}

/// urfave context_test.go `TestCheckRequiredFlags`.
#[test]
fn urfave_check_required_flags() {
    struct Case {
        name: &'static str,
        parse_input: &'static [&'static str],
        env: &'static [(&'static str, &'static [u8])],
        flags: Vec<FlagDef>,
        expected_error: Option<&'static [&'static str]>,
    }
    let req = |name: &'static str| f(name, s(""), "", &[], true);
    let opt = |name: &'static str| f(name, s(""), "", &[], false);
    let cases = vec![
        Case {
            name: "empty",
            parse_input: &[],
            env: &[],
            flags: vec![],
            expected_error: None,
        },
        Case {
            name: "optional",
            parse_input: &[],
            env: &[],
            flags: vec![opt("optionalFlag")],
            expected_error: None,
        },
        Case {
            name: "required",
            parse_input: &[],
            env: &[],
            flags: vec![req("requiredFlag")],
            expected_error: Some(&["requiredFlag"]),
        },
        Case {
            name: "required_and_present",
            parse_input: &["--requiredFlag", "myinput"],
            env: &[],
            flags: vec![req("requiredFlag")],
            expected_error: None,
        },
        Case {
            name: "required_and_present_via_env_var",
            parse_input: &[],
            env: &[("REQUIRED_FLAG", b"true")],
            flags: vec![f("requiredFlag", s(""), "", &["REQUIRED_FLAG"], true)],
            expected_error: None,
        },
        Case {
            name: "required_and_optional",
            parse_input: &[],
            env: &[],
            flags: vec![req("requiredFlag"), opt("optionalFlag")],
            expected_error: Some(&[]),
        },
        Case {
            name: "required_and_optional_and_optional_present",
            parse_input: &["--optionalFlag", "myinput"],
            env: &[],
            flags: vec![req("requiredFlag"), opt("optionalFlag")],
            expected_error: Some(&[]),
        },
        Case {
            name: "required_and_optional_and_optional_present_via_env_var",
            parse_input: &[],
            env: &[("OPTIONAL_FLAG", b"true")],
            flags: vec![
                req("requiredFlag"),
                f("optionalFlag", s(""), "", &["OPTIONAL_FLAG"], false),
            ],
            expected_error: Some(&[]),
        },
        Case {
            name: "required_and_optional_and_required_present",
            parse_input: &["--requiredFlag", "myinput"],
            env: &[],
            flags: vec![req("requiredFlag"), opt("optionalFlag")],
            expected_error: None,
        },
        Case {
            name: "two_required",
            parse_input: &[],
            env: &[],
            flags: vec![req("requiredFlagOne"), req("requiredFlagTwo")],
            expected_error: Some(&["requiredFlagOne", "requiredFlagTwo"]),
        },
        Case {
            name: "two_required_and_one_present",
            parse_input: &["--requiredFlag", "myinput"],
            env: &[],
            flags: vec![req("requiredFlag"), req("requiredFlagTwo")],
            expected_error: Some(&[]),
        },
        Case {
            name: "two_required_and_both_present",
            parse_input: &["--requiredFlag", "myinput", "--requiredFlagTwo", "myinput"],
            env: &[],
            flags: vec![req("requiredFlag"), req("requiredFlagTwo")],
            expected_error: None,
        },
        Case {
            name: "required_flag_with_short_name",
            parse_input: &["-N", "asd", "-N", "qwe"],
            env: &[],
            flags: vec![FlagDef {
                aliases: &["N"],
                ..f("names", FlagKind::StringSlice, "", &[], true)
            }],
            expected_error: None,
        },
        Case {
            name: "required_flag_with_multiple_short_names",
            parse_input: &["-n", "asd", "-n", "qwe"],
            env: &[],
            flags: vec![FlagDef {
                aliases: &["N", "n"],
                ..f("names", FlagKind::StringSlice, "", &[], true)
            }],
            expected_error: None,
        },
        Case {
            name: "required_flag_with_short_alias_not_printed_on_error",
            parse_input: &[],
            env: &[],
            flags: vec![f("names, n", FlagKind::StringSlice, "", &[], true)],
            expected_error: Some(&["Required flag \"names\" not set"]),
        },
        Case {
            name: "required_flag_with_one_character",
            parse_input: &[],
            env: &[],
            flags: vec![req("n")],
            expected_error: Some(&["Required flag \"n\" not set"]),
        },
        Case {
            name: "required_flag_with_alias_errors_with_actual_name",
            parse_input: &[],
            env: &[],
            flags: vec![FlagDef {
                aliases: &["c"],
                ..req("collection")
            }],
            expected_error: Some(&["Required flag \"collection\" not set"]),
        },
        Case {
            name: "required_flag_without_name_or_aliases_doesnt_error",
            parse_input: &[],
            env: &[],
            flags: vec![req("")],
            expected_error: None,
        },
    ];
    for case in cases {
        let app = one_command(case.flags);
        let mut args = vec!["cmd"];
        args.extend_from_slice(case.parse_input);
        let result = context_of(&app, &args, case.env);
        match case.expected_error {
            None => assert!(matches!(result, Ok(Some(_))), "{}", case.name),
            Some(contents) => {
                let Err(CliError::Msg(msg)) = result else {
                    panic!("{}: expected an error", case.name);
                };
                for c in contents {
                    assert!(msg.contains(c), "{}: {msg}", case.name);
                }
            }
        }
    }
    let app = one_command(vec![req("seed"), req("token")]);
    assert!(matches!(
        context_of(&app, &["cmd"], &[]),
        Err(CliError::Msg(m)) if m == "Required flags \"seed, token\" not set"
    ));
}

/// urfave context_test.go `TestContext_IsSet`, `TestContext_IsSet_Aliases`, `TestContext_IsSet_fromEnv`.
#[test]
fn urfave_context_is_set() {
    let app = AppDef {
        name: "test",
        usage: "",
        version: String::new(),
        flags: vec![f("top-flag", b(true), "doc", &[], false)],
        commands: vec![cmd(
            "cmd",
            "",
            "",
            vec![
                f("one-flag", b(false), "doc", &[], false),
                f("two-flag", b(false), "doc", &[], false),
                f("three-flag", s("hello world"), "doc", &[], false),
            ],
            Some(a_cmd),
        )],
    };
    let Ok(Some(ctx)) = context_of(
        &app,
        &[
            "--top-flag",
            "cmd",
            "--one-flag",
            "--two-flag",
            "--three-flag",
            "frob",
        ],
        &[],
    ) else {
        panic!("no action");
    };
    for name in ["one-flag", "two-flag", "three-flag", "top-flag"] {
        assert!(ctx.is_set(name), "{name}");
    }
    assert!(!ctx.is_set("bogus"));
    assert_eq!(ctx.string("three-flag"), "frob");

    let app = one_command(vec![FlagDef {
        aliases: &["f", "t", "igloo"],
        ..f("foo", FlagKind::Int64 { default: 0 }, "", &[], false)
    }]);
    let Ok(Some(ctx)) = context_of(&app, &["cmd"], &[]) else {
        panic!("no action");
    };
    for name in ["foo", "f", "t", "igloo"] {
        assert!(!ctx.is_set(name), "{name}");
    }
    let Ok(Some(ctx)) = context_of(&app, &["cmd", "--t", "10"], &[]) else {
        panic!("no action");
    };
    for name in ["foo", "f", "t", "igloo"] {
        assert!(ctx.is_set(name), "{name}");
        assert_eq!(ctx.int64(name), 10);
    }

    let app = one_command(vec![
        FlagDef {
            aliases: &["t"],
            ..f(
                "timeout",
                FlagKind::Float64 { default: 0.0 },
                "",
                &["APP_TIMEOUT_SECONDS"],
                false,
            )
        },
        FlagDef {
            aliases: &["p"],
            ..f("password", s(""), "", &["APP_PASSWORD"], false)
        },
        FlagDef {
            aliases: &["u"],
            ..f(
                "unparsable",
                FlagKind::Float64 { default: 0.0 },
                "",
                &["APP_UNPARSABLE"],
                false,
            )
        },
        FlagDef {
            aliases: &["n"],
            ..f(
                "no-env-var",
                FlagKind::Float64 { default: 0.0 },
                "",
                &[],
                false,
            )
        },
    ]);
    let env: &[(&str, &[u8])] = &[("APP_TIMEOUT_SECONDS", b"15.5"), ("APP_PASSWORD", b"")];
    let Ok(Some(ctx)) = context_of(&app, &["cmd"], env) else {
        panic!("no action");
    };
    for (name, want) in [
        ("timeout", true),
        ("t", true),
        ("password", true),
        ("p", true),
        ("no-env-var", false),
        ("n", false),
        ("unparsable", false),
    ] {
        assert_eq!(ctx.is_set(name), want, "{name}");
    }
    assert_eq!(ctx.float64("t"), 15.5);
    let env: &[(&str, &[u8])] = &[("APP_UNPARSABLE", b"foobar")];
    assert!(matches!(
        context_of(&app, &["cmd"], env),
        Err(CliError::Msg(m)) if m == "could not parse \"foobar\" as float64 value from environment variable \"APP_UNPARSABLE\" for flag unparsable: strconv.ParseFloat: parsing \"foobar\": invalid syntax"
    ));
}

/// urfave flag_test.go `TestFlagsFromEnv`, the rows of the kinds dstore uses (base 0 only).
#[test]
fn urfave_flags_from_env() {
    struct Row {
        input: &'static [u8],
        output: Option<FlagValue>,
        flag: FlagDef,
        err: &'static str,
    }
    let debug = || f("debug", b(false), "", &["DEBUG"], false);
    let time = || {
        f(
            "time",
            FlagKind::Duration { default_ns: 0 },
            "",
            &["TIME"],
            false,
        )
    };
    let seconds = |kind| f("seconds", kind, "", &["SECONDS"], false);
    let rows = vec![
        Row {
            input: b"",
            output: Some(FlagValue::Bool(false)),
            flag: debug(),
            err: "",
        },
        Row {
            input: b"1",
            output: Some(FlagValue::Bool(true)),
            flag: debug(),
            err: "",
        },
        Row {
            input: b"false",
            output: Some(FlagValue::Bool(false)),
            flag: debug(),
            err: "",
        },
        Row {
            input: b"foobar",
            output: None,
            flag: debug(),
            err: "could not parse \"foobar\" as bool value from environment variable \"DEBUG\" for flag debug: strconv.ParseBool: parsing \"foobar\": invalid syntax",
        },
        Row {
            input: b"1s",
            output: Some(FlagValue::DurationNs(1_000_000_000)),
            flag: time(),
            err: "",
        },
        Row {
            input: b"foobar",
            output: None,
            flag: time(),
            err: "could not parse \"foobar\" as duration value from environment variable \"TIME\" for flag time: time: invalid duration \"foobar\"",
        },
        Row {
            input: b"1.2",
            output: Some(FlagValue::F64(1.2)),
            flag: seconds(FlagKind::Float64 { default: 0.0 }),
            err: "",
        },
        Row {
            input: b"1",
            output: Some(FlagValue::F64(1.0)),
            flag: seconds(FlagKind::Float64 { default: 0.0 }),
            err: "",
        },
        Row {
            input: b"foobar",
            output: None,
            flag: seconds(FlagKind::Float64 { default: 0.0 }),
            err: "could not parse \"foobar\" as float64 value from environment variable \"SECONDS\" for flag seconds: strconv.ParseFloat: parsing \"foobar\": invalid syntax",
        },
        Row {
            input: b"1",
            output: Some(FlagValue::I64(1)),
            flag: seconds(FlagKind::Int64 { default: 0 }),
            err: "",
        },
        Row {
            input: b"1.2",
            output: None,
            flag: seconds(FlagKind::Int64 { default: 0 }),
            err: "could not parse \"1.2\" as int value from environment variable \"SECONDS\" for flag seconds: strconv.ParseInt: parsing \"1.2\": invalid syntax",
        },
        Row {
            input: b"1",
            output: Some(FlagValue::I64(1)),
            flag: seconds(FlagKind::Int { default: 0 }),
            err: "",
        },
        Row {
            input: b"08",
            output: None,
            flag: seconds(FlagKind::Int { default: 0 }),
            err: "could not parse \"08\" as int value from environment variable \"SECONDS\" for flag seconds: strconv.ParseInt: parsing \"08\": invalid syntax",
        },
        Row {
            input: b"foobar",
            output: None,
            flag: seconds(FlagKind::Int { default: 0 }),
            err: "could not parse \"foobar\" as int value from environment variable \"SECONDS\" for flag seconds: strconv.ParseInt: parsing \"foobar\": invalid syntax",
        },
        Row {
            input: b"foo",
            output: Some(FlagValue::Str(OsString::from("foo"))),
            flag: f("name", s(""), "", &["NAME"], false),
            err: "",
        },
        Row {
            input: b"foo,bar",
            output: Some(FlagValue::Slice(vec!["foo".into(), "bar".into()])),
            flag: f("names", FlagKind::StringSlice, "", &["NAMES"], false),
            err: "",
        },
        Row {
            input: b" no space ",
            output: Some(FlagValue::Slice(vec!["no space".into()])),
            flag: f("names", FlagKind::StringSlice, "", &["NAMES"], false),
            err: "",
        },
        Row {
            input: b"1",
            output: Some(FlagValue::U64(1)),
            flag: seconds(FlagKind::Uint { default: 0 }),
            err: "",
        },
        Row {
            input: b"08",
            output: None,
            flag: seconds(FlagKind::Uint { default: 0 }),
            err: "could not parse \"08\" as uint value from environment variable \"SECONDS\" for flag seconds: strconv.ParseUint: parsing \"08\": invalid syntax",
        },
        Row {
            input: b"1.2",
            output: None,
            flag: seconds(FlagKind::Uint { default: 0 }),
            err: "could not parse \"1.2\" as uint value from environment variable \"SECONDS\" for flag seconds: strconv.ParseUint: parsing \"1.2\": invalid syntax",
        },
    ];
    for row in rows {
        let name = row.flag.name;
        let env_name = row.flag.env[0];
        let app = one_command(vec![row.flag]);
        let result = context_of(&app, &["cmd"], &[(env_name, row.input)]);
        match (result, row.output) {
            (Ok(Some(ctx)), Some(want)) => {
                assert_eq!(ctx.value(name), Some(want), "{name} {:?}", row.input);
                assert!(ctx.is_set(name), "{name} {:?}", row.input);
            }
            (Err(CliError::Msg(msg)), None) => assert_eq!(msg, row.err),
            (other, want) => panic!(
                "{name} {:?}: got {:?}, want {want:?}",
                row.input,
                other.map(|c| c.map(|c| c.value(name)))
            ),
        }
    }
}

/// urfave help_test.go `Test_helpCommand_Action_ErrorIfNoTopic` and errors_test.go
/// `TestHandleExitCoder_ExitCoder`: an unknown topic exits 3; an action's exit code passes through.
#[test]
fn urfave_help_topic_and_exit_codes() {
    let app = AppDef {
        name: "test",
        usage: "",
        version: String::new(),
        flags: Vec::new(),
        commands: Vec::new(),
    };
    assert!(matches!(
        context_of(&app, &["foo"], &[]),
        Err(CliError::Exit { msg, code: 3 }) if msg == "No help topic for 'foo'"
    ));
}

/// urfave help_test.go `Test_helpSubcommand_Action_ErrorIfNoTopic` (the help action of a parent and of
/// its `help` subcommand) and app_test.go `TestApp_CommandNotFound` without a `CommandNotFound` hook,
/// which `AppDef` does not have: an unknown topic exits 3 and runs no action.
#[test]
fn urfave_subcommand_help_topic_and_command_not_found() {
    let app = AppDef {
        name: "test",
        usage: "",
        version: String::new(),
        flags: Vec::new(),
        commands: vec![
            parent("p", "", vec![cmd("c", "", "", Vec::new(), Some(a_cmd))]),
            cmd("bar", "", "", Vec::new(), Some(a_cmd)),
        ],
    };
    for args in [
        &["p", "foo"][..],
        &["p", "help", "foo"],
        &["p", "h", "foo", "c"],
    ] {
        assert!(
            matches!(
                context_of(&app, args, &[]),
                Err(CliError::Exit { msg, code: 3 }) if msg == "No help topic for 'foo'"
            ),
            "{args:?}"
        );
    }
    let run = run_case(&app, &args_of(&["foo"]), &[]);
    assert_eq!(
        (run.exit, run.stderr.as_str(), run.label.as_str()),
        (3, "No help topic for 'foo'\n", "")
    );
    assert!(run.stdout.is_empty());
}

/// urfave help_test.go `TestHelpNameConsistency` with the default templates: the app help, a parent's
/// help and the help action of a command without an action name the app, not the program.
#[test]
fn urfave_help_name_consistency() {
    let app = AppDef {
        name: "bar",
        usage: "",
        version: String::new(),
        flags: Vec::new(),
        commands: vec![parent(
            "command1",
            "",
            vec![cmd("subcommand1", "", "", Vec::new(), None)],
        )],
    };
    for (args, name_line) in [
        (&[][..], "NAME:\n   bar - A new cli application\n"),
        (&["command1"], "NAME:\n   bar command1\n"),
        (
            &["command1", "subcommand1"],
            "NAME:\n   bar command1 subcommand1\n",
        ),
    ] {
        let run = run_case(&app, &args_of(args), &[]);
        assert_eq!(run.exit, 0, "{args:?}");
        assert!(
            String::from_utf8_lossy(&run.stdout).starts_with(name_line),
            "{args:?}"
        );
    }
}

/// urfave help_test.go `TestShowCommandHelp_HelpPrinter`, the row the default printer can take:
/// `help ""` has no topic, so it prints the app help.
#[test]
fn urfave_help_with_empty_topic_prints_app_help() {
    let app = AppDef {
        name: "my-app",
        usage: "",
        version: String::new(),
        flags: Vec::new(),
        commands: vec![cmd("my-command", "", "", Vec::new(), Some(a_cmd))],
    };
    let run = run_case(&app, &args_of(&["help", ""]), &[]);
    assert_eq!(run.exit, 0);
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "NAME:\n   my-app - A new cli application\n\nUSAGE:\n   my-app [global options] command [command options]\n\nCOMMANDS:\n   my-command  \n   help, h     Shows a list of commands or help for one command\n\nGLOBAL OPTIONS:\n   --help, -h  show help\n"
    );
}

/// The future of `run` is `Send`: a caller may spawn it on a multi-thread runtime.
#[test]
fn run_future_is_send() {
    fn assert_send<T: Send>(_: &T) {}
    let app = one_command(Vec::new());
    let mut out = Vec::new();
    let fut = run(&app, os_args(&["test", "cmd"]), &mut out);
    assert_send(&fut);
    assert!(poll_ready(fut).is_ok());
}

fn galactic(_: &Context) -> Pin<Box<dyn Future<Output = Result<(), CliError>> + Send + '_>> {
    Box::pin(async {
        Err(CliError::Exit {
            msg: "galactic perimeter breach".into(),
            code: 9,
        })
    })
}

#[tokio::test]
async fn run_awaits_the_action() {
    let app = AppDef {
        name: "test",
        usage: "",
        version: String::new(),
        flags: Vec::new(),
        commands: vec![
            cmd("breach", "", "", Vec::new(), Some(galactic)),
            cmd("cmd", "", "", Vec::new(), Some(a_cmd)),
        ],
    };
    let mut out = Vec::new();
    let result = run(&app, os_args(&["test", "breach"]), &mut out).await;
    assert!(matches!(result, Err(CliError::Exit { code: 9, .. })));
    assert!(run(&app, os_args(&["test", "cmd"]), &mut out).await.is_ok());
    assert!(out.is_empty());
    let result = run(&app, os_args(&["test", "cmd", "exit5"]), &mut out).await;
    assert!(matches!(result, Err(CliError::Exit { code: 5, .. })));
    assert_eq!(CliError::msg("x").to_string(), "x");
}

/// urfave app_test.go `TestApp_Run_Help`, `TestApp_Run_Version` and `TestApp_Run_CommandHelpName` (with
/// the default help names).
#[test]
fn urfave_run_help_version_and_help_names() {
    let app = AppDef {
        name: "boom",
        usage: "make an explosive entrance",
        version: "0.1.0".into(),
        flags: Vec::new(),
        commands: Vec::new(),
    };
    for args in [&["--help"][..], &["-h"], &["help"]] {
        let run = run_case(&app, &args_of(args), &[]);
        assert!(String::from_utf8_lossy(&run.stdout).contains("boom - make an explosive entrance"));
    }
    for args in [&["--version"][..], &["-v"]] {
        let run = run_case(&app, &args_of(args), &[]);
        assert_eq!(run.stdout, b"boom version 0.1.0\n");
    }
    let app = AppDef {
        name: "command",
        usage: "",
        version: String::new(),
        flags: Vec::new(),
        commands: vec![CommandDef {
            description: "foo commands",
            ..parent(
                "foo",
                "",
                vec![cmd("bar", "does bar things", "", Vec::new(), Some(a_cmd))],
            )
        }],
    };
    let run = run_case(&app, &args_of(&["foo", "bar", "--help"]), &[]);
    let out = String::from_utf8_lossy(&run.stdout);
    assert!(out.contains("command foo bar - does bar things"), "{out}");
    assert!(out.contains("command foo bar [command options]"), "{out}");
}

/// App defaults: the process name, "A new cli application", no version flag without a version; a
/// user command named help replaces the help command and flag; duplicate names fail.
#[test]
fn app_setup_defaults_and_duplicates() {
    let app = AppDef {
        name: "",
        usage: "",
        version: String::new(),
        flags: Vec::new(),
        commands: Vec::new(),
    };
    let base = String::from_utf8_lossy(&path::base(
        std::env::args_os().next().unwrap_or_default().as_bytes(),
    ))
    .into_owned();
    let run = run_case(&app, &args_of(&["--help"]), &[]);
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        format!(
            "NAME:\n   {base} - A new cli application\n\nUSAGE:\n   {base} [global options] command [command options]\n\nCOMMANDS:\n   help, h  Shows a list of commands or help for one command\n\nGLOBAL OPTIONS:\n   --help, -h  show help\n"
        )
    );
    let run = run_case(&app, &args_of(&["--version"]), &[]);
    assert_eq!(
        run.stderr,
        "dstore: flag provided but not defined: -version\n"
    );

    let app = AppDef {
        name: "x",
        usage: "",
        version: String::new(),
        flags: Vec::new(),
        commands: vec![cmd("help", "my help", "", Vec::new(), Some(a_cmd))],
    };
    let run = run_case(&app, &args_of(&["--help"]), &[]);
    assert_eq!(run.stderr, "dstore: flag: help requested\n");
    assert!(context_of(&app, &["help"], &[]).is_ok_and(|c| c.is_some()));

    let app = AppDef {
        name: "x",
        usage: "",
        version: String::new(),
        flags: Vec::new(),
        commands: vec![
            cmd("a", "", "", Vec::new(), Some(a_cmd)),
            CommandDef {
                aliases: &["a"],
                ..cmd("b", "", "", Vec::new(), Some(a_cmd))
            },
        ],
    };
    assert!(matches!(
        context_of(&app, &[], &[]),
        Err(CliError::Msg(m)) if m == "parent command [x] has duplicated subcommand name or alias: a"
    ));
}

#[test]
fn empty_context_accessors() {
    let ctx = Context::default();
    assert_eq!(ctx.string("x"), "");
    assert!(!ctx.bool("x") && !ctx.is_set("x"));
    assert_eq!(
        (ctx.int("x"), ctx.uint("x"), ctx.duration_ns("x")),
        (0, 0, 0)
    );
    assert_eq!(ctx.float64("x"), 0.0);
    assert!(ctx.string_slice("x").is_empty() && ctx.value("x").is_none());
    assert_eq!((ctx.narg(), ctx.first()), (0, std::ffi::OsStr::new("")));
}
