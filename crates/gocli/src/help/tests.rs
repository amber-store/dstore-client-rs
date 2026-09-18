//! Tests of `help`: the go1.26.5 `text/tabwriter` test table, urfave's flag stringification and wrap
//! tests, and template executions against verified dstore help texts (cli.md §3.1).

use super::*;

/// A row of go1.26.5 `src/text/tabwriter/tabwriter_test.go` `tests`.
struct GoCase {
    name: &'static str,
    minwidth: usize,
    tabwidth: usize,
    padding: usize,
    padchar: u8,
    flags: u32,
    src: &'static [u8],
    expected: &'static [u8],
}

include!("go_tabwriter_tests.rs");

fn write_chunks(case: &GoCase, ranges: &[std::ops::Range<usize>]) -> Vec<u8> {
    let mut w = TabWriter::new(
        Vec::new(),
        case.minwidth,
        case.tabwidth,
        case.padding,
        case.padchar,
        case.flags,
    );
    for r in ranges {
        let chunk = &case.src[r.clone()];
        assert_eq!(w.write(chunk).ok(), Some(chunk.len()), "{}", case.name);
    }
    assert!(w.flush().is_ok(), "{}", case.name);
    w.into_inner()
}

/// go1.26.5 tabwriter_test.go `Test` (`check`): written all at once, byte by byte, and in Fibonacci
/// slices.
#[test]
fn go_tabwriter_table() {
    assert_eq!(GO_TABWRITER_TESTS.len(), 53);
    for case in GO_TABWRITER_TESTS {
        let n = case.src.len();
        let all = write_chunks(case, std::slice::from_ref(&(0..n)));
        assert_eq!(all, case.expected, "{} (written all at once)", case.name);
        let bytes: Vec<_> = (0..n).map(|i| i..i + 1).collect();
        assert_eq!(
            write_chunks(case, &bytes),
            case.expected,
            "{} (written byte-by-byte)",
            case.name
        );
        let mut ranges = Vec::new();
        let (mut i, mut d) = (0, 0);
        while i < n {
            ranges.push(i..i + d);
            i += d;
            d += 1;
            if i + d > n {
                d = n - i;
            }
        }
        assert_eq!(
            write_chunks(case, &ranges),
            case.expected,
            "{} (written in fibonacci slices)",
            case.name
        );
    }
}

struct FailWriter;

impl Write for FailWriter {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::other("cannot write"))
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// tabwriter `TestPanicDuringFlush` / `TestPanicDuringWrite`: output errors are returned; `Flush`
/// resets the writer.
#[test]
fn tabwriter_output_errors() {
    let mut w = TabWriter::new(FailWriter, 0, 0, 5, b'.', 0);
    assert_eq!(w.write(b"a").ok(), Some(1));
    assert!(w.flush().is_err());
    assert!(w.buf.is_empty());
    let mut w = TabWriter::new(FailWriter, 0, 0, 5, b'.', 0);
    assert!(w.write(b"a\n\n").is_err());
}

/// Unflushed text is lost: the truncated `help <parent>` shape.
#[test]
fn tabwriter_drops_unflushed_text() {
    let mut w = TabWriter::new(Vec::new(), 1, 8, 2, b' ', 0);
    assert_eq!(w.write(b"USAGE:\n   x\n\nCOMMANDS:").ok(), Some(22));
    assert_eq!(w.into_inner(), b"USAGE:\n   x\n\n");
    // Cells of a block are padded even when the text after the tab is empty.
    let mut w = TabWriter::new(Vec::new(), 1, 8, 2, b' ', 0);
    assert!(
        w.write_all(b"\n   --zone value\t\n   --no-ramp\tjoin\n")
            .is_ok()
    );
    assert!(w.flush().is_ok());
    assert_eq!(
        w.into_inner(),
        b"\n   --zone value  \n   --no-ramp     join\n"
    );
}

fn flag(
    name: &'static str,
    aliases: &'static [&'static str],
    kind: FlagKind,
    usage: &'static str,
    env: &'static [&'static str],
) -> FlagDef {
    FlagDef {
        name,
        aliases,
        kind,
        usage,
        env,
        required: false,
        disable_default_text: false,
    }
}

/// urfave flag_test.go `TestBoolFlagHelpOutput`, `TestStringFlagHelpOutput`,
/// `TestStringFlagWithEnvVarHelpOutput`, `TestStringSliceFlagHelpOutput` (the rows without defaults),
/// `TestIntFlagHelpOutput`, `TestInt64FlagHelpOutput`, `TestUintFlagHelpOutput`,
/// `TestDurationFlagHelpOutput`, and the `TestFlagStringifying` rows of the kinds dstore uses.
#[test]
fn urfave_flag_help_output() {
    use FlagKind::*;
    let cases: Vec<(FlagDef, &str)> = vec![
        (
            flag("help", &[], Bool { default: false }, "", &[]),
            "--help\t(default: false)",
        ),
        (
            flag("h", &[], Bool { default: false }, "", &[]),
            "-h\t(default: false)",
        ),
        (
            flag("foo", &[], String { default: "" }, "", &[]),
            "--foo value\t",
        ),
        (
            flag(
                "f",
                &[],
                String { default: "all" },
                "The total `foo` desired",
                &[],
            ),
            "-f foo\tThe total foo desired (default: \"all\")",
        ),
        (
            flag(
                "test",
                &[],
                String {
                    default: "Something",
                },
                "",
                &[],
            ),
            "--test value\t(default: \"Something\")",
        ),
        (
            flag(
                "config",
                &["c"],
                String { default: "" },
                "Load configuration from `FILE`",
                &[],
            ),
            "--config FILE, -c FILE\tLoad configuration from FILE",
        ),
        (
            flag(
                "config",
                &["c"],
                String {
                    default: "config.json",
                },
                "Load configuration from `CONFIG`",
                &[],
            ),
            "--config CONFIG, -c CONFIG\tLoad configuration from CONFIG (default: \"config.json\")",
        ),
        (
            flag("foo", &[], StringSlice, "", &[]),
            "--foo value [ --foo value ]\t",
        ),
        (
            flag("f", &[], StringSlice, "", &[]),
            "-f value [ -f value ]\t",
        ),
        (
            flag("hats", &[], Int { default: 9 }, "", &[]),
            "--hats value\t(default: 9)",
        ),
        (
            flag("H", &[], Int { default: 9 }, "", &[]),
            "-H value\t(default: 9)",
        ),
        (
            flag(
                "hats",
                &[],
                Int64 {
                    default: 8589934592,
                },
                "",
                &[],
            ),
            "--hats value\t(default: 8589934592)",
        ),
        (
            flag("nerfs", &[], Uint { default: 41 }, "", &[]),
            "--nerfs value\t(default: 41)",
        ),
        (
            flag("N", &[], Uint { default: 41 }, "", &[]),
            "-N value\t(default: 41)",
        ),
        (
            flag(
                "hooting",
                &[],
                Duration {
                    default_ns: 1_000_000_000,
                },
                "",
                &[],
            ),
            "--hooting value\t(default: 1s)",
        ),
        (
            flag("vividly", &[], Bool { default: false }, "", &[]),
            "--vividly\t(default: false)",
        ),
        (
            flag("scream-for", &[], Duration { default_ns: 0 }, "", &[]),
            "--scream-for value\t(default: 0s)",
        ),
        (
            flag("arduous", &[], Float64 { default: 0.0 }, "", &[]),
            "--arduous value\t(default: 0)",
        ),
        (
            flag("grubs", &[], Int { default: 0 }, "", &[]),
            "--grubs value\t(default: 0)",
        ),
        (
            flag("flume", &[], Int64 { default: 0 }, "", &[]),
            "--flume value\t(default: 0)",
        ),
        (
            flag("arf-sound", &[], String { default: "" }, "", &[]),
            "--arf-sound value\t",
        ),
        (
            flag("meow-sounds", &[], StringSlice, "", &[]),
            "--meow-sounds value [ --meow-sounds value ]\t",
        ),
        (
            flag("jars", &[], Uint { default: 0 }, "", &[]),
            "--jars value\t(default: 0)",
        ),
        (crate::flag::HELP_FLAG.clone(), "--help, -h\tshow help"),
        (
            crate::flag::VERSION_FLAG.clone(),
            "--version, -v\tprint the version",
        ),
    ];
    for (f, want) in cases {
        assert_eq!(stringify_flag(&f), want);
    }
    for f in [
        flag("foo", &[], String { default: "" }, "", &["APP_FOO"]),
        flag(
            "config",
            &["c"],
            String {
                default: "config.json",
            },
            "",
            &["APP_FOO"],
        ),
    ] {
        assert!(stringify_flag(&f).ends_with(" [$APP_FOO]"));
    }
    assert_eq!(
        stringify_flag(&flag(
            "ticket",
            &[],
            String { default: "" },
            "",
            &["A", "B"]
        )),
        "--ticket value\t [$A, $B]"
    );
    // A disabled default text only matters for bool flags.
    let mut f = flag("n", &[], Int { default: 3 }, "count", &[]);
    f.disable_default_text = true;
    assert_eq!(stringify_flag(&f), "-n value\tcount (default: 3)");
}

/// urfave help_test.go `TestWrap` and the wrapped strings of `TestWrappedHelp` (wrapAt 30).
#[test]
fn urfave_wrap() {
    assert_eq!(wrap("", 4, 16), "");
    assert_eq!(
        wrap(
            "here's a sample App.Usage string long enough that it should be wrapped in this test",
            6,
            30
        ),
        "here's a sample\n      App.Usage string long\n      enough that it should be\n      wrapped in this test"
    );
    assert_eq!(
        wrap(
            "i'm not sure how App.UsageText differs from App.Usage, but this should also be wrapped in this test",
            3,
            30
        ),
        "i'm not sure how\n   App.UsageText differs from\n   App.Usage, but this should\n   also be wrapped in this\n   test"
    );
    assert_eq!(
        wrap(
            "here's a sample App.Description string long enough that it should be wrapped in this test\n\nwith a newline\n   and an indented line",
            3,
            30
        ),
        "here's a sample\n   App.Description string long\n   enough that it should be\n   wrapped in this test\n\n   with a newline\n      and an indented line"
    );
    assert_eq!(
        wrap(
            "--foo, -h\there's a really long help text line, let's see where it wraps. blah blah blah and so on. (default: false)",
            6,
            30
        ),
        "--foo, -h here's a\n      really long help text\n      line, let's see where it\n      wraps. blah blah blah\n      and so on. (default:\n      false)"
    );
    assert_eq!(
        wrap(
            "Here's a sample copyright text string long enough that it should be wrapped.\nIncluding newlines.\n   And also indented lines.\n\n\nAnd then another long line. Blah blah blah does anybody ever read these things?",
            3,
            30
        ),
        "Here's a sample copyright\n   text string long enough\n   that it should be wrapped.\n   Including newlines.\n      And also indented lines.\n\n\n   And then another long line.\n   Blah blah blah does anybody\n   ever read these things?"
    );
    // The default wrapAt never wraps dstore's texts.
    let long = "x ".repeat(3000);
    assert_eq!(wrap(&long, 3, WRAP_AT), long);
}

fn client_flags() -> Vec<String> {
    use FlagKind::*;
    [
        flag(
            "ticket",
            &[],
            String { default: "" },
            "cluster ticket (dstore1…) or comma-separated node ids, found by discovery",
            &["DSTORE_TICKET"],
        ),
        flag(
            "relay",
            &[],
            String { default: "" },
            "relay URL for the fallback path (default: the built-in relay map)",
            &[],
        ),
        flag(
            "no-relay",
            &[],
            Bool { default: false },
            "direct addresses only, no relay",
            &[],
        ),
        flag(
            "no-discovery",
            &[],
            Bool { default: false },
            "neither announce this endpoint nor resolve node ids by discovery (mDNS and, with relays, number0's DNS)",
            &["DSTORE_NO_DISCOVERY"],
        ),
        crate::flag::HELP_FLAG.clone(),
    ]
    .iter()
    .map(stringify_flag)
    .collect()
}

fn printed(template: Template, d: &HelpData) -> std::string::String {
    let mut out = Vec::new();
    assert!(print_help(&mut out, &execute(template, d)).is_ok());
    std::string::String::from_utf8_lossy(&out).into_owned()
}

fn row(names: &[&str], usage: &str) -> CommandRow {
    CommandRow {
        names: names.iter().map(|n| (*n).to_owned()).collect(),
        usage: usage.to_owned(),
    }
}

/// cli.md §3.1 `dstore refs --help` and §3.2 `dstore refs --bogus` (verified against Go).
#[test]
fn command_templates_verified_refs() {
    let mut d = HelpData {
        help_name: "dstore refs".into(),
        usage: "list references".into(),
        args_usage: "[PREFIX]".into(),
        visible_flags: client_flags(),
        ..HelpData::default()
    };
    let options = "OPTIONS:\n   --ticket value  cluster ticket (dstore1…) or comma-separated node ids, found by discovery [$DSTORE_TICKET]\n   --relay value   relay URL for the fallback path (default: the built-in relay map)\n   --no-relay      direct addresses only, no relay (default: false)\n   --no-discovery  neither announce this endpoint nor resolve node ids by discovery (mDNS and, with relays, number0's DNS) (default: false) [$DSTORE_NO_DISCOVERY]\n   --help, -h      show help\n";
    let head = "NAME:\n   dstore refs - list references\n\nUSAGE:\n   dstore refs [command options] [PREFIX]\n\n";
    assert_eq!(printed(Template::Command, &d), format!("{head}{options}"));
    d.visible_commands = vec![row(&["help", "h"], HELP_USAGE)];
    d.categories = Some(vec![CommandCategory {
        name: std::string::String::new(),
        commands: vec![row(&["help", "h"], HELP_USAGE)],
    }]);
    assert_eq!(
        printed(Template::Subcommand, &d),
        format!(
            "{head}COMMANDS:\n   help, h  Shows a list of commands or help for one command\n\n{options}"
        )
    );
}

const HELP_USAGE: &str = "Shows a list of commands or help for one command";

/// cli.md §3.1 `dstore node --help` (trailing spaces of usage-less rows) and the truncated
/// `dstore help node`.
#[test]
fn subcommand_template_verified_node() {
    let rows = vec![
        row(&["join"], "join a cluster with this store and keep serving"),
        row(&["remove"], ""),
        row(&["drain"], ""),
        row(&["weight"], ""),
        row(&["zone"], ""),
        row(&["repair"], ""),
        row(&["help", "h"], HELP_USAGE),
    ];
    let mut d = HelpData {
        help_name: "dstore node".into(),
        usage: "join, remove, drain, weight, zone, repair".into(),
        visible_commands: rows.clone(),
        categories: Some(vec![CommandCategory {
            name: std::string::String::new(),
            commands: rows,
        }]),
        visible_flags: vec![stringify_flag(&crate::flag::HELP_FLAG)],
        ..HelpData::default()
    };
    assert_eq!(
        printed(Template::Subcommand, &d),
        "NAME:\n   dstore node - join, remove, drain, weight, zone, repair\n\nUSAGE:\n   dstore node [command options]\n\nCOMMANDS:\n   join     join a cluster with this store and keep serving\n   remove   \n   drain    \n   weight   \n   zone     \n   repair   \n   help, h  Shows a list of commands or help for one command\n\nOPTIONS:\n   --help, -h  show help\n"
    );
    d.categories = None;
    let exec = execute(Template::Subcommand, &d);
    assert!(!exec.complete);
    assert_eq!(
        printed(Template::Subcommand, &d),
        "NAME:\n   dstore node - join, remove, drain, weight, zone, repair\n\nUSAGE:\n   dstore node [command options]\n\n"
    );
}

/// cli.md §3.1 app help (verified): VERSION, the command list, GLOBAL OPTIONS.
#[test]
fn app_template_verified_shape() {
    let rows = vec![
        row(&["refs"], "list references"),
        row(&["help", "h"], HELP_USAGE),
    ];
    let d = HelpData {
        help_name: "dstore".into(),
        usage: "a distributed amber store: cluster nodes and the client".into(),
        version: "dev".into(),
        visible_commands: rows.clone(),
        categories: Some(vec![CommandCategory {
            name: std::string::String::new(),
            commands: rows,
        }]),
        visible_flags: vec![
            stringify_flag(&flag(
                "log-level",
                &[],
                FlagKind::String { default: "info" },
                "debug|info|warn|error (a global flag: give it before the command)",
                &["DSTORE_LOG_LEVEL"],
            )),
            stringify_flag(&crate::flag::HELP_FLAG),
            stringify_flag(&crate::flag::VERSION_FLAG),
        ],
        ..HelpData::default()
    };
    assert_eq!(
        printed(Template::App, &d),
        "NAME:\n   dstore - a distributed amber store: cluster nodes and the client\n\nUSAGE:\n   dstore [global options] command [command options]\n\nVERSION:\n   dev\n\nCOMMANDS:\n   refs     list references\n   help, h  Shows a list of commands or help for one command\n\nGLOBAL OPTIONS:\n   --log-level value  debug|info|warn|error (a global flag: give it before the command) (default: \"info\") [$DSTORE_LOG_LEVEL]\n   --help, -h         show help\n   --version, -v      print the version\n"
    );
    // A named category uses a tab, not the offset rows.
    let named = HelpData {
        categories: Some(vec![CommandCategory {
            name: "cat".into(),
            commands: vec![row(&["a", "b"], "x")],
        }]),
        ..d.clone()
    };
    assert!(
        execute(Template::App, &named)
            .text
            .contains("\n   cat:\n     a, b\tx")
    );
}

#[test]
fn unquote_and_prefixed_names() {
    assert_eq!(
        unquote_usage("Load `FILE` then `X`"),
        ("FILE".to_owned(), "Load FILE then `X`".to_owned())
    );
    assert_eq!(
        unquote_usage("an `unterminated quote"),
        (
            std::string::String::new(),
            "an `unterminated quote".to_owned()
        )
    );
    let names = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    assert_eq!(prefixed_names(&names(&["µ"]), "value"), "--µ value");
    assert_eq!(prefixed_names(&names(&["a", ""]), ""), "-a, ");
    assert_eq!(rune_count("µ\u{2026}".as_bytes()), 2);
    assert_eq!(rune_count(b"\xe2\x82A\xff"), 4);
}
