//! Golden tests of `dstore-udiff` against go-udiff v0.4.1 and Go 1.26.5 `sort.Slice`, from the `udiff`
//! family of `tools/vectorgen` (schemas: `tools/vectorgen/docs/udiff.md`):
//!
//! - `udiff/udiff.json`: `Unified` and `Lines` over hand-written inputs, `ToUnified` over explicit
//!   edits, and at least 3000 random line edits generated from splitmix64 recipes;
//! - `udiff/lcs.json`: `lcs.DiffLines`;
//! - `udiff/pdqsort.json`: `sort.Slice` permutations. They are read here rather than by crate unit
//!   tests: `dstore-udiff` has no dev-dependencies, and `gosort` is public.

use dstore_testkit::golden::{self, decimal_u64};
use dstore_testkit::splitmix::SplitMix64;
use dstore_udiff::{Edit, gosort, lcs};
use serde::Deserialize;

/// How many failures a test prints.
const SHOW: usize = 8;

/// Fails the test with the first failures.
fn report(what: &str, failures: &[String], total: usize) {
    let shown = &failures[..failures.len().min(SHOW)];
    assert!(
        failures.is_empty(),
        "{what}: {} of {total} vector cases differ from Go; the first {}:\n{}",
        failures.len(),
        shown.len(),
        shown.join("\n")
    );
}

fn lossy(b: &[u8]) -> String {
    format!("{:?}", String::from_utf8_lossy(b))
}

/// A JSON text field and its `_hex` twin, of which exactly one is present (VECTORS.md).
fn bytes(field: &str, text: &Option<String>, hex: &Option<String>) -> Vec<u8> {
    match (text, hex) {
        (Some(text), None) => text.as_bytes().to_vec(),
        (None, Some(hex)) => golden::hex(hex),
        _ => panic!("vector field {field}: want exactly one of {field} and {field}_hex"),
    }
}

/// FNV-1a 64 (Go `hash/fnv.New64a`).
fn fnv1a64(b: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &c in b {
        h ^= u64::from(c);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

#[derive(Deserialize)]
struct Alphabet {
    lines_hex: Vec<String>,
}

fn decode_alphabets(alphabets: &[Alphabet]) -> Vec<Vec<Vec<u8>>> {
    alphabets
        .iter()
        .map(|a| a.lines_hex.iter().map(|h| golden::hex(h)).collect())
        .collect()
}

#[derive(Deserialize)]
struct EditJson {
    start: usize,
    end: usize,
    new: Option<String>,
    new_hex: Option<String>,
}

impl EditJson {
    fn edit(&self) -> Edit {
        Edit {
            start: self.start,
            end: self.end,
            new: bytes("new", &self.new, &self.new_hex),
        }
    }
}

/// A random text pair (docs/udiff.md "Random recipes").
#[derive(Debug, Deserialize)]
struct Recipe {
    #[serde(deserialize_with = "decimal_u64")]
    seed: u64,
    alphabet: i64,
    distinct: u64,
    lines: usize,
    mode: String,
    new_lines: usize,
    edits: usize,
    max_run: u64,
    strip_eol: String,
}

/// `udRecipe.generate` of `tools/vectorgen/family_udiff.go`.
fn generate(r: &Recipe, alphabets: &[Vec<Vec<u8>>]) -> (Vec<u8>, Vec<u8>) {
    let mut rng = SplitMix64(r.seed);
    let value = |rng: &mut SplitMix64| {
        let v = rng.next_u64();
        if r.alphabet >= 0 { v % r.distinct } else { v }
    };
    let old_vals: Vec<u64> = (0..r.lines).map(|_| value(&mut rng)).collect();
    let new_vals: Vec<u64> = match r.mode.as_str() {
        "independent" => (0..r.new_lines).map(|_| value(&mut rng)).collect(),
        "edits" => {
            let mut vals = old_vals.clone();
            for _ in 0..r.edits {
                let op = rng.next_u64() % 3; // 0 insert, 1 delete, 2 replace
                let pos = (rng.next_u64() % (vals.len() as u64 + 1)) as usize;
                let run = 1 + (rng.next_u64() % r.max_run) as usize;
                let del = if op != 0 {
                    run.min(vals.len() - pos)
                } else {
                    0
                };
                let ins: Vec<u64> = if op != 1 {
                    (0..run).map(|_| value(&mut rng)).collect()
                } else {
                    Vec::new()
                };
                vals.splice(pos..pos + del, ins);
            }
            vals
        }
        other => panic!("recipe mode {other:?} is unknown"),
    };
    let mut old = render(r, alphabets, &old_vals);
    let mut new = render(r, alphabets, &new_vals);
    if matches!(r.strip_eol.as_str(), "old" | "both") && old.last() == Some(&b'\n') {
        old.pop();
    }
    if matches!(r.strip_eol.as_str(), "new" | "both") && new.last() == Some(&b'\n') {
        new.pop();
    }
    (old, new)
}

fn render(r: &Recipe, alphabets: &[Vec<Vec<u8>>], vals: &[u64]) -> Vec<u8> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut b = Vec::new();
    for &v in vals {
        if r.alphabet < 0 {
            // Go: fmt.Appendf(b, "%016x\n", v)
            for shift in (0..16).rev() {
                b.push(HEX[((v >> (shift * 4)) & 0xf) as usize]);
            }
            b.push(b'\n');
        } else {
            b.extend_from_slice(&alphabets[r.alphabet as usize][v as usize]);
        }
    }
    b
}

/// `splitLines`: after each newline, keeping a final partial line.
fn split_lines(text: &[u8]) -> Vec<&[u8]> {
    let mut lines = Vec::new();
    let mut start = 0;
    for (i, &c) in text.iter().enumerate() {
        if c == b'\n' {
            lines.push(&text[start..=i]);
            start = i + 1;
        }
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

// --- udiff/udiff.json ---

#[derive(Deserialize)]
struct UdiffFile {
    alphabets: Vec<Alphabet>,
    cases: Vec<UnifiedCase>,
    to_unified: Vec<ToUnifiedCase>,
    random: Vec<RandomCase>,
}

#[derive(Deserialize)]
struct UnifiedCase {
    name: String,
    old_label: Option<String>,
    old_label_hex: Option<String>,
    new_label: Option<String>,
    new_label_hex: Option<String>,
    old: Option<String>,
    old_hex: Option<String>,
    new: Option<String>,
    new_hex: Option<String>,
    edits: Vec<EditJson>,
    out: Option<String>,
    out_hex: Option<String>,
}

#[derive(Deserialize)]
struct ToUnifiedCase {
    name: String,
    old_label: String,
    new_label: String,
    content: Option<String>,
    content_hex: Option<String>,
    edits: Vec<EditJson>,
    context_lines: usize,
    out: Option<String>,
    out_hex: Option<String>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct RandomCase {
    name: String,
    old_label: String,
    new_label: String,
    recipe: Recipe,
    old_len: usize,
    #[serde(deserialize_with = "decimal_u64")]
    old_fnv1a64: u64,
    new_len: usize,
    #[serde(deserialize_with = "decimal_u64")]
    new_fnv1a64: u64,
    edits: usize,
    out_len: usize,
    #[serde(deserialize_with = "decimal_u64")]
    out_fnv1a64: u64,
    out: Option<String>,
    out_hex: Option<String>,
}

#[test]
fn unified_cases() {
    let file: UdiffFile = golden::load_json("udiff/udiff.json");
    assert!(!file.cases.is_empty(), "udiff/udiff.json has no cases");
    let mut failures = Vec::new();
    for c in &file.cases {
        let old_label = bytes("old_label", &c.old_label, &c.old_label_hex);
        let new_label = bytes("new_label", &c.new_label, &c.new_label_hex);
        let old = bytes("old", &c.old, &c.old_hex);
        let new = bytes("new", &c.new, &c.new_hex);

        let want_edits: Vec<Edit> = c.edits.iter().map(EditJson::edit).collect();
        let got_edits = dstore_udiff::lines(&old, &new);
        if got_edits != want_edits {
            failures.push(format!(
                "{}: lines:\n got {got_edits:?}\nwant {want_edits:?}",
                c.name
            ));
        }

        let want = bytes("out", &c.out, &c.out_hex);
        let got = dstore_udiff::unified(&old_label, &new_label, &old, &new);
        if got != want {
            failures.push(format!(
                "{}: unified:\n got {}\nwant {}",
                c.name,
                lossy(&got),
                lossy(&want)
            ));
        }
    }
    report("udiff/udiff.json cases", &failures, file.cases.len());
}

#[test]
fn to_unified_cases() {
    let file: UdiffFile = golden::load_json("udiff/udiff.json");
    assert!(
        !file.to_unified.is_empty(),
        "udiff/udiff.json has no to_unified cases"
    );
    let mut failures = Vec::new();
    for c in &file.to_unified {
        let content = bytes("content", &c.content, &c.content_hex);
        let edits: Vec<Edit> = c.edits.iter().map(EditJson::edit).collect();
        let want = match &c.error {
            Some(err) => Err(err.clone()),
            None => Ok(bytes("out", &c.out, &c.out_hex)),
        };
        let got = dstore_udiff::to_unified(
            c.old_label.as_bytes(),
            c.new_label.as_bytes(),
            &content,
            &edits,
            c.context_lines,
        );
        if got != want {
            let show = |r: &Result<Vec<u8>, String>| match r {
                Ok(out) => lossy(out),
                Err(err) => format!("error {err:?}"),
            };
            failures.push(format!(
                "{}: to_unified:\n got {}\nwant {}",
                c.name,
                show(&got),
                show(&want)
            ));
        }
    }
    report(
        "udiff/udiff.json to_unified",
        &failures,
        file.to_unified.len(),
    );
}

#[test]
fn unified_random() {
    let file: UdiffFile = golden::load_json("udiff/udiff.json");
    let alphabets = decode_alphabets(&file.alphabets);
    assert!(
        file.random.len() >= 3000,
        "udiff/udiff.json has {} random cases, want at least 3000",
        file.random.len()
    );
    let mut failures = Vec::new();
    for c in &file.random {
        let (old, new) = generate(&c.recipe, &alphabets);
        if (old.len(), fnv1a64(&old), new.len(), fnv1a64(&new))
            != (c.old_len, c.old_fnv1a64, c.new_len, c.new_fnv1a64)
        {
            failures.push(format!(
                "{}: recipe {:?} generates other inputs than vectorgen (a test generator bug, not a udiff bug)",
                c.name, c.recipe
            ));
            continue;
        }

        let edits = dstore_udiff::lines(&old, &new);
        if edits.len() != c.edits {
            failures.push(format!(
                "{}: lines gives {} edits, want {} (recipe {:?})",
                c.name,
                edits.len(),
                c.edits,
                c.recipe
            ));
        }

        let got = dstore_udiff::unified(c.old_label.as_bytes(), c.new_label.as_bytes(), &old, &new);
        let verbatim = match (&c.out, &c.out_hex) {
            (None, None) => None,
            _ => Some(bytes("out", &c.out, &c.out_hex)),
        };
        let digest_ok = got.len() == c.out_len && fnv1a64(&got) == c.out_fnv1a64;
        if !digest_ok || verbatim.as_ref().is_some_and(|want| *want != got) {
            let head = &got[..got.len().min(1500)];
            let want = match &verbatim {
                Some(want) => lossy(want),
                None => format!("{} bytes, fnv1a64 {}", c.out_len, c.out_fnv1a64),
            };
            failures.push(format!(
                "{}: unified of recipe {:?}:\n got {} bytes, fnv1a64 {}, beginning {}\nwant {want}",
                c.name,
                c.recipe,
                got.len(),
                fnv1a64(&got),
                lossy(head)
            ));
        }
    }
    report("udiff/udiff.json random", &failures, file.random.len());
}

// --- udiff/lcs.json ---

#[derive(Deserialize)]
struct LcsFile {
    alphabets: Vec<Alphabet>,
    strings: Vec<LcsString>,
    random: Vec<LcsRandom>,
}

#[derive(Deserialize)]
struct LcsString {
    name: String,
    a: String,
    b: String,
    diffs: Vec<[usize; 4]>,
}

#[derive(Deserialize)]
struct LcsRandom {
    name: String,
    recipe: Recipe,
    old_lines: usize,
    new_lines: usize,
    diffs: Vec<[usize; 4]>,
}

fn quads(diffs: &[lcs::Diff]) -> Vec<[usize; 4]> {
    diffs
        .iter()
        .map(|d| [d.start, d.end, d.repl_start, d.repl_end])
        .collect()
}

#[test]
fn lcs_diff_lines() {
    let file: LcsFile = golden::load_json("udiff/lcs.json");
    let alphabets = decode_alphabets(&file.alphabets);
    assert!(
        !file.strings.is_empty() && !file.random.is_empty(),
        "udiff/lcs.json has no cases"
    );
    let mut failures = Vec::new();
    for c in &file.strings {
        // Each byte is one element.
        let a: Vec<&[u8]> = c.a.as_bytes().chunks(1).collect();
        let b: Vec<&[u8]> = c.b.as_bytes().chunks(1).collect();
        let got = quads(&lcs::diff_lines(&a, &b));
        if got != c.diffs {
            failures.push(format!(
                "{}: diff_lines({:?}, {:?}):\n got {got:?}\nwant {:?}",
                c.name, c.a, c.b, c.diffs
            ));
        }
    }
    for c in &file.random {
        let (old, new) = generate(&c.recipe, &alphabets);
        let (a, b) = (split_lines(&old), split_lines(&new));
        if (a.len(), b.len()) != (c.old_lines, c.new_lines) {
            failures.push(format!(
                "{}: recipe {:?} generates {} and {} lines, want {} and {} (a test generator bug)",
                c.name,
                c.recipe,
                a.len(),
                b.len(),
                c.old_lines,
                c.new_lines
            ));
            continue;
        }
        let got = quads(&lcs::diff_lines(&a, &b));
        if got != c.diffs {
            failures.push(format!(
                "{}: diff_lines of recipe {:?}:\n got {got:?}\nwant {:?}",
                c.name, c.recipe, c.diffs
            ));
        }
    }
    report(
        "udiff/lcs.json",
        &failures,
        file.strings.len() + file.random.len(),
    );
}

// --- udiff/pdqsort.json ---

#[derive(Deserialize)]
struct PdqFile {
    cases: Vec<PdqCase>,
}

/// A sort recipe (docs/udiff.md "Sort recipes").
#[derive(Deserialize)]
struct PdqCase {
    name: String,
    n: usize,
    pattern: String,
    modulus: u64,
    swaps: usize,
    #[serde(deserialize_with = "decimal_u64")]
    seed: u64,
    less: String,
    /// Pattern `adversary`: McIlroy's antiquicksort input, verbatim.
    values: Option<Vec<u64>>,
    order: Option<Vec<usize>>,
    #[serde(deserialize_with = "decimal_u64")]
    order_fnv1a64: u64,
}

#[derive(Clone, Copy, Debug)]
struct Elem {
    x: u64,
    len: u64,
    id: usize,
}

/// `pdqCase.elements` of `tools/vectorgen/family_udiff.go`.
fn elements(c: &PdqCase) -> Vec<Elem> {
    let mut rng = SplitMix64(c.seed);
    let (n, m) = (c.n, c.modulus);
    let mut v: Vec<u64> = (0..n)
        .map(|i| match c.pattern.as_str() {
            "random" => rng.next_u64() % m,
            "asc" | "asc_swaps" => i as u64 / m,
            "desc" | "desc_swaps" => (n - 1 - i) as u64 / m,
            "equal" => 0,
            "saw" => i as u64 % m,
            "organ" => i.min(n - 1 - i) as u64 / m,
            "adversary" => match c.values.as_ref().and_then(|values| values.get(i)) {
                Some(&value) => value,
                None => panic!("{}: adversary case without value {i}", c.name),
            },
            other => panic!("pdqsort recipe: unknown pattern {other:?}"),
        })
        .collect();
    if c.pattern.ends_with("_swaps") && n > 0 {
        for _ in 0..c.swaps {
            let p = (rng.next_u64() % n as u64) as usize;
            let q = (rng.next_u64() % n as u64) as usize;
            v.swap(p, q);
        }
    }
    v.iter()
        .enumerate()
        .map(|(id, &value)| match c.less.as_str() {
            "len_desc" | "coin_1_2" | "coin_1_16" | "coin_15_16" => Elem {
                x: 0,
                len: value,
                id,
            },
            "x_asc_len_desc" => Elem {
                x: value,
                len: rng.next_u64() % 3,
                id,
            },
            other => panic!("pdqsort recipe: unknown less {other:?}"),
        })
        .collect()
}

#[test]
fn pdqsort_slice() {
    let file: PdqFile = golden::load_json("udiff/pdqsort.json");
    assert!(!file.cases.is_empty(), "udiff/pdqsort.json has no cases");
    let mut failures = Vec::new();
    for c in &file.cases {
        let mut els = elements(c);
        // The coins answer from their own stream, starting at the case seed.
        let mut coin = SplitMix64(c.seed);
        match c.less.as_str() {
            "len_desc" => gosort::slice(&mut els, |a, b| a.len > b.len),
            "x_asc_len_desc" => {
                gosort::slice(
                    &mut els,
                    |a, b| {
                        if a.x != b.x { a.x < b.x } else { a.len > b.len }
                    },
                )
            }
            "coin_1_2" => gosort::slice(&mut els, |_, _| coin.next_u64().is_multiple_of(2)),
            "coin_1_16" => gosort::slice(&mut els, |_, _| coin.next_u64().is_multiple_of(16)),
            "coin_15_16" => gosort::slice(&mut els, |_, _| !coin.next_u64().is_multiple_of(16)),
            other => panic!("pdqsort recipe: unknown less {other:?}"),
        }
        let ids: Vec<usize> = els.iter().map(|e| e.id).collect();
        let digest: Vec<u8> = ids
            .iter()
            .flat_map(|&id| (id as u64).to_le_bytes())
            .collect();
        let verbatim_ok = c.order.as_ref().is_none_or(|want| *want == ids);
        if !verbatim_ok || fnv1a64(&digest) != c.order_fnv1a64 {
            failures.push(format!(
                "{}: sort order:\n got {:?}\nwant {:?} (fnv1a64 {})",
                c.name,
                &ids[..ids.len().min(300)],
                c.order,
                c.order_fnv1a64
            ));
        }
    }
    report("udiff/pdqsort.json", &failures, file.cases.len());
}
