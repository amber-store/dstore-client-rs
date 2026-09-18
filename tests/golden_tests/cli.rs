//! Golden tests of `dstore-cli` `size` (`cli/size.json`: `parse_size`, `pack_size`). Crate-private helpers
//! (`describe_change`, `hex_decode` cases) are tested by the crate's unit tests; the CLI snapshots run in
//! `tests/cli_snapshots.rs`.
//!
//! `pack_size` runs as `size_test.go` `TestPackSizeFlag` does: `dstore_gocli::dispatch` parses the case's
//! argument over the `nodeFlags` of a node-side command of `dstore_cli::app()`, with `$DSTORE_PACK_SIZE` supplied
//! through its `getenv`, then `size::pack_size` reads the context. The process environment is never read or
//! changed, so the cases run in parallel with the other tests.

use std::ffi::OsString;

use dstore_gocli::AppDef;
use dstore_testkit::golden::load_json;
use serde::Deserialize;

#[derive(Deserialize)]
struct SizeVectors {
    parse: Vec<ParseCase>,
    pack_size: Vec<PackSizeCase>,
}

#[derive(Deserialize)]
struct ParseCase {
    #[serde(rename = "in")]
    input: String,
    ok: bool,
    #[serde(default)]
    out: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Deserialize)]
struct PackSizeCase {
    name: String,
    flag: Option<String>,
    env: Option<String>,
    ok: bool,
    #[serde(default)]
    out: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

/// The expected result of a case: `out` (a decimal i64) when `ok`, else `error`.
fn want(ok: bool, out: &Option<String>, error: &Option<String>, name: &str) -> Result<i64, String> {
    if ok {
        let out = out
            .as_deref()
            .unwrap_or_else(|| panic!("{name}: ok case without out"));
        Ok(out
            .parse::<i64>()
            .unwrap_or_else(|e| panic!("{name}: bad out {out:?}: {e}")))
    } else {
        Err(error
            .clone()
            .unwrap_or_else(|| panic!("{name}: error case without error")))
    }
}

#[test]
fn parse_size() {
    let v: SizeVectors = load_json("cli/size.json");
    assert!(!v.parse.is_empty());
    let mut failures = Vec::new();
    for c in &v.parse {
        let want = want(c.ok, &c.out, &c.error, &c.input);
        let got = dstore_cli::size::parse_size(&c.input);
        if got != want {
            failures.push(format!(
                "parseSize({:?}): got {got:?}, want {want:?}",
                c.input
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Every case over the node-side commands that carry `nodeFlags` and require no other flag: `serve` (exactly
/// `nodeFlags`, as `TestPackSizeFlag`'s app) and `cluster init` (`nodeFlags` plus its own). `node join` requires
/// `--seed` and `--token`.
#[test]
fn pack_size() {
    let v: SizeVectors = load_json("cli/size.json");
    assert!(!v.pack_size.is_empty());
    let app = dstore_cli::app();
    let mut failures = Vec::new();
    for c in &v.pack_size {
        let want = want(c.ok, &c.out, &c.error, &c.name);
        for command in [&["serve"][..], &["cluster", "init"][..]] {
            match pack_size_of(&app, command, c.flag.as_deref(), c.env.as_deref()) {
                Ok(got) if got == want => {}
                got => failures.push(format!(
                    "case {} (dstore {} flag {:?}, env {:?}): got {got:?}, want {want:?}",
                    c.name,
                    command.join(" "),
                    c.flag,
                    c.env
                )),
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// `packSize(c)` for `dstore <command> [--pack-size <flag>]`, with `$DSTORE_PACK_SIZE` unset (`None`) or set to
/// `env` (possibly empty). The outer `Err` says the framework did not reach the command's action.
fn pack_size_of(
    app: &AppDef,
    command: &[&str],
    flag: Option<&str>,
    env: Option<&str>,
) -> Result<Result<i64, String>, String> {
    let mut args: Vec<OsString> = std::iter::once("dstore")
        .chain(command.iter().copied())
        .map(OsString::from)
        .collect();
    if let Some(f) = flag {
        args.push("--pack-size".into());
        args.push(f.into());
    }
    let getenv = |name: &str| match (name, env) {
        ("DSTORE_PACK_SIZE", Some(v)) => Some(OsString::from(v)),
        _ => None,
    };
    let mut stdout = Vec::new();
    match dstore_gocli::dispatch(app, args, &mut stdout, &getenv) {
        Ok(Some((_action, ctx))) => Ok(dstore_cli::size::pack_size(&ctx)),
        Ok(None) => Err(format!(
            "no action reached; stdout {:?}",
            String::from_utf8_lossy(&stdout)
        )),
        Err(e) => Err(format!(
            "{e:?}; stdout {:?}",
            String::from_utf8_lossy(&stdout)
        )),
    }
}
