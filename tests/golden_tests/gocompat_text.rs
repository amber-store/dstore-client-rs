//! Golden tests of `dstore-gocompat` `quote` and `strings` (`text/quote.json`, `text/case.json`).
//!
//! The vectors come from the go1.26.5 standard library through `tools/vectorgen` (families `quote` and
//! `case`, documented in `tools/vectorgen/docs/gocompat-a.md`). `strconv.IsPrint`, `unicode.IsSpace`,
//! `unicode.ToLower` and `unicode.ToUpper` are checked over every rune.

use std::collections::HashMap;

use dstore_gocompat::quote::{is_print, is_space, quote};
use dstore_gocompat::strings::{fields_func, to_lower, to_upper, trim_space};
use dstore_testkit::golden::{self, load_json};
use serde::Deserialize;

/// The toolchain whose behaviour the vectors pin (PORTING.md DD-14).
const GO_VERSION: &str = "go1.26.5";

#[derive(Deserialize)]
struct QuoteFile {
    go_version: String,
    cases: Vec<QuoteCase>,
    is_print_flips: Vec<u32>,
}

#[derive(Deserialize)]
struct QuoteCase {
    name: String,
    in_hex: String,
    out: String,
}

#[derive(Deserialize)]
struct CaseFile {
    go_version: String,
    cases: Vec<CaseCase>,
    to_lower: Vec<CaseRun>,
    to_upper: Vec<CaseRun>,
    is_space_flips: Vec<u32>,
}

#[derive(Deserialize)]
struct CaseCase {
    name: String,
    in_hex: String,
    to_lower_hex: String,
    to_upper_hex: String,
    trim_space_hex: String,
    fields_hex: Vec<String>,
}

/// The runes `lo, lo + stride, …, hi` map to `rune + delta`.
#[derive(Debug, Deserialize)]
struct CaseRun {
    lo: u32,
    hi: u32,
    stride: u32,
    delta: i64,
}

/// Fails with every collected failure.
fn report(what: &str, failures: &[String], total: usize) {
    assert!(
        failures.is_empty(),
        "{what}: {} of {total} checks differ from Go:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn quote_matches_go() {
    let f: QuoteFile = load_json("text/quote.json");
    assert_eq!(f.go_version, GO_VERSION);
    assert!(!f.cases.is_empty(), "text/quote.json has no cases");
    let mut failures = Vec::new();
    for c in &f.cases {
        let got = quote(&golden::hex(&c.in_hex));
        if got != c.out {
            failures.push(format!(
                "{}: quote({}) = {got}, want {}",
                c.name, c.in_hex, c.out
            ));
        }
    }
    report("quote", &failures, f.cases.len());
}

/// Checks `pred` over every rune against the runes where Go's predicate flips, starting from false.
fn check_flips(what: &str, flips: &[u32], pred: impl Fn(char) -> bool) {
    assert!(!flips.is_empty(), "{what}: no flips");
    assert!(
        flips.windows(2).all(|w| w[0] < w[1]),
        "{what}: flips are not ascending"
    );
    let mut next = flips.iter().copied().peekable();
    let mut want = false;
    let mut failures = Vec::new();
    for v in 0..=0x10_ffff_u32 {
        if next.peek() == Some(&v) {
            want = !want;
            next.next();
        }
        if let Some(c) = char::from_u32(v)
            && pred(c) != want
            && failures.len() < 50
        {
            failures.push(format!("{what}(U+{v:04X}) = {}, want {want}", !want));
        }
    }
    assert_eq!(next.next(), None, "{what}: flips beyond U+10FFFF");
    report(what, &failures, 0x11_0000);
}

#[test]
fn is_print_matches_go_for_every_rune() {
    let f: QuoteFile = load_json("text/quote.json");
    check_flips("is_print", &f.is_print_flips, is_print);
}

#[test]
fn is_space_matches_go_for_every_rune() {
    let f: CaseFile = load_json("text/case.json");
    check_flips("is_space", &f.is_space_flips, is_space);
}

#[test]
fn strings_functions_match_go() {
    let f: CaseFile = load_json("text/case.json");
    assert_eq!(f.go_version, GO_VERSION);
    assert!(!f.cases.is_empty(), "text/case.json has no cases");
    let mut failures = Vec::new();
    for c in &f.cases {
        let input = golden::hex(&c.in_hex);
        let checks = [
            ("to_lower", hex::encode(to_lower(&input)), &c.to_lower_hex),
            ("to_upper", hex::encode(to_upper(&input)), &c.to_upper_hex),
            (
                "trim_space",
                hex::encode(trim_space(&input)),
                &c.trim_space_hex,
            ),
        ];
        for (func, got, want) in checks {
            if &got != want {
                failures.push(format!(
                    "{}: {func}({}) = {got}, want {want}",
                    c.name, c.in_hex
                ));
            }
        }
        let fields: Vec<String> = fields_func(&input, is_space)
            .into_iter()
            .map(hex::encode)
            .collect();
        if fields != c.fields_hex {
            failures.push(format!(
                "{}: fields_func({}, is_space) = {fields:?}, want {:?}",
                c.name, c.in_hex, c.fields_hex
            ));
        }
    }
    report("strings", &failures, f.cases.len());
}

/// Expands the runs of a mapping into the map of every rune it changes.
fn expand(what: &str, runs: &[CaseRun]) -> HashMap<u32, char> {
    let mut map = HashMap::new();
    for run in runs {
        // A run whose hi is not lo plus a multiple of stride would drop hi from the expansion.
        assert!(
            (run.stride == 1 || run.stride == 2)
                && run.lo <= run.hi
                && (run.hi - run.lo) % run.stride == 0,
            "{what}: bad run {run:?}"
        );
        for r in (run.lo..=run.hi).step_by(run.stride as usize) {
            let to = u32::try_from(i64::from(r) + run.delta)
                .ok()
                .and_then(char::from_u32);
            match to {
                Some(to) => assert!(
                    map.insert(r, to).is_none(),
                    "{what}: U+{r:04X} is in two runs"
                ),
                None => panic!("{what}: U+{r:04X} maps to no rune in {run:?}"),
            }
        }
    }
    map
}

#[test]
fn case_mapping_matches_go_for_every_rune() {
    let f: CaseFile = load_json("text/case.json");
    let lower = expand("to_lower", &f.to_lower);
    let upper = expand("to_upper", &f.to_upper);
    assert!(lower.len() > 1000 && upper.len() > 1000);
    let mut failures = Vec::new();
    for c in (0..=0x10_ffff_u32).filter_map(char::from_u32) {
        let input = c.to_string();
        let v = u32::from(c);
        let want_lower = lower.get(&v).copied().unwrap_or(c).to_string();
        let want_upper = upper.get(&v).copied().unwrap_or(c).to_string();
        if to_lower(input.as_bytes()) != want_lower.as_bytes() && failures.len() < 50 {
            failures.push(format!("to_lower(U+{v:04X}) differs, want {want_lower:?}"));
        }
        if to_upper(input.as_bytes()) != want_upper.as_bytes() && failures.len() < 50 {
            failures.push(format!("to_upper(U+{v:04X}) differs, want {want_upper:?}"));
        }
    }
    report("case mapping", &failures, 0x11_0000);
}
