//! Golden tests of `dstore-gocompat` `json`, `hex`, `base32` and `path` (`gocompat/json.json`, `gocompat/hex.json`,
//! `gocompat/base32.json`, `gocompat/path.json`; generator `tools/vectorgen/family_gocompat_io.go`, schemas in
//! `tools/vectorgen/docs/gocompat-c.md`). `errno` and `os` depend on the running OS and are covered by the crate's
//! unit tests.

use dstore_gocompat::{base32, hex as gohex, json, path};
use dstore_testkit::golden::{hex, load_json};
use serde::Deserialize;

/// Asserts that no case failed, showing the first failures.
fn check(family: &str, total: usize, failures: &[String]) {
    assert!(total > 0, "{family}: no cases");
    assert!(
        failures.is_empty(),
        "{family}: {} of {total} cases fail:\n{}",
        failures.len(),
        failures
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

fn leak(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

#[derive(Deserialize)]
struct FieldSpec {
    name: String,
    kind: String,
    omitempty: bool,
}

#[derive(Deserialize)]
struct StructSpec {
    name: String,
    go_type: String,
    fields: Vec<FieldSpec>,
}

#[derive(Deserialize)]
struct FieldValue {
    name: String,
    str_hex: String,
    bool: bool,
}

#[derive(Deserialize)]
struct MarshalCase {
    name: String,
    #[serde(rename = "struct")]
    struct_name: String,
    values: Vec<FieldValue>,
    out: String,
}

#[derive(Deserialize)]
struct Slot {
    name: String,
    set: bool,
    str_hex: String,
    bool: bool,
}

#[derive(Deserialize)]
struct UnmarshalCase {
    name: String,
    #[serde(rename = "struct")]
    struct_name: String,
    in_hex: String,
    slots: Vec<Slot>,
    error: String,
}

#[derive(Deserialize)]
struct JsonVectors {
    structs: Vec<StructSpec>,
    marshal: Vec<MarshalCase>,
    unmarshal: Vec<UnmarshalCase>,
}

fn struct_spec<'a>(v: &'a JsonVectors, name: &str) -> &'a StructSpec {
    match v.structs.iter().find(|s| s.name == name) {
        Some(s) => s,
        None => panic!("json.json: no struct {name}"),
    }
}

#[test]
fn json_marshal_indent() {
    let v: JsonVectors = load_json("gocompat/json.json");
    let mut failures = Vec::new();
    for case in &v.marshal {
        let spec = struct_spec(&v, &case.struct_name);
        assert_eq!(spec.fields.len(), case.values.len(), "{}", case.name);
        let strings: Vec<Vec<u8>> = case.values.iter().map(|f| hex(&f.str_hex)).collect();
        let mut fields = Vec::new();
        for ((field, value), bytes) in spec.fields.iter().zip(&case.values).zip(&strings) {
            assert_eq!(field.name, value.name, "{}", case.name);
            match field.kind.as_str() {
                "string" if field.omitempty && bytes.is_empty() => {}
                "string" => fields.push((field.name.as_str(), json::JsonField::Str(bytes))),
                "bool" if field.omitempty && !value.bool => {}
                "bool" => fields.push((field.name.as_str(), json::JsonField::Bool(value.bool))),
                other => panic!("{}: unknown kind {other}", case.name),
            }
        }
        let got = json::marshal_indent_object(&fields);
        if got != case.out.as_bytes() {
            failures.push(format!(
                "{}: got {:?}, want {:?}",
                case.name,
                String::from_utf8_lossy(&got),
                case.out
            ));
        }
    }
    check("json marshal", v.marshal.len(), &failures);
}

#[test]
fn json_unmarshal_object() {
    let v: JsonVectors = load_json("gocompat/json.json");
    let specs: Vec<(String, &'static str, Vec<json::JsonFieldSpec>)> = v
        .structs
        .iter()
        .map(|s| {
            let fields = s
                .fields
                .iter()
                .map(|f| json::JsonFieldSpec {
                    name: leak(&f.name),
                    kind: match f.kind.as_str() {
                        "bool" => json::JsonKind::Bool,
                        _ => json::JsonKind::String,
                    },
                })
                .collect();
            (s.name.clone(), leak(&s.go_type), fields)
        })
        .collect();
    let mut failures = Vec::new();
    for case in &v.unmarshal {
        let Some((_, go_type, fields)) = specs.iter().find(|(n, _, _)| *n == case.struct_name)
        else {
            panic!("{}: no struct {}", case.name, case.struct_name);
        };
        let (slots, result) = json::unmarshal_object(&hex(&case.in_hex), go_type, fields);
        let got_err = match &result {
            Ok(()) => String::new(),
            Err(e) => e.to_string(),
        };
        if got_err != case.error {
            failures.push(format!(
                "{}: error {got_err:?}, want {:?}",
                case.name, case.error
            ));
        }
        assert_eq!(slots.len(), case.slots.len(), "{}", case.name);
        for ((slot, want), spec) in slots.iter().zip(&case.slots).zip(fields) {
            assert_eq!(spec.name, want.name, "{}", case.name);
            let want_value = match (want.set, spec.kind) {
                (false, _) => None,
                (true, json::JsonKind::String) => Some(json::JsonValue::String(hex(&want.str_hex))),
                (true, json::JsonKind::Bool) => Some(json::JsonValue::Bool(want.bool)),
            };
            if *slot != want_value {
                failures.push(format!(
                    "{}: field {} = {slot:?}, want {want_value:?}",
                    case.name, want.name
                ));
            }
        }
    }
    check("json unmarshal", v.unmarshal.len(), &failures);
}

#[derive(Deserialize)]
struct HexEncode {
    in_hex: String,
    out: String,
}

#[derive(Deserialize)]
struct HexDecode {
    in_hex: String,
    out_hex: Option<String>,
    error: String,
}

#[derive(Deserialize)]
struct HexVectors {
    encode: Vec<HexEncode>,
    decode: Vec<HexDecode>,
}

/// Compares a decode result with a vector's `out_hex` (null on error) and `error`.
fn decode_mismatch<E: std::fmt::Display>(
    got: Result<Vec<u8>, E>,
    out_hex: &Option<String>,
    error: &str,
) -> Option<String> {
    match (got, out_hex) {
        (Ok(b), Some(want)) if b == hex(want) && error.is_empty() => None,
        (Err(e), None) if e.to_string() == error => None,
        (Ok(b), _) => Some(format!("got bytes {b:02x?}, want {out_hex:?} / {error:?}")),
        (Err(e), _) => Some(format!(
            "got error {:?}, want {out_hex:?} / {error:?}",
            e.to_string()
        )),
    }
}

#[test]
fn hex_encode_and_decode() {
    let v: HexVectors = load_json("gocompat/hex.json");
    let mut failures = Vec::new();
    for case in &v.encode {
        let got = gohex::encode(&hex(&case.in_hex));
        if got != case.out {
            failures.push(format!(
                "encode {}: got {got}, want {}",
                case.in_hex, case.out
            ));
        }
    }
    for case in &v.decode {
        if let Some(m) = decode_mismatch(
            gohex::decode_string(&hex(&case.in_hex)),
            &case.out_hex,
            &case.error,
        ) {
            failures.push(format!("decode {}: {m}", case.in_hex));
        }
    }
    check("hex", v.encode.len() + v.decode.len(), &failures);
}

#[derive(Deserialize)]
struct B32Encode {
    alphabet: String,
    in_hex: String,
    out: String,
}

#[derive(Deserialize)]
struct B32Decode {
    alphabet: String,
    in_hex: String,
    out_hex: Option<String>,
    error: String,
}

#[derive(Deserialize)]
struct B32Vectors {
    encode: Vec<B32Encode>,
    decode: Vec<B32Decode>,
}

fn alphabet(name: &str) -> &'static [u8; 32] {
    match name {
        "std" => base32::STD_ALPHABET,
        "zbase32" => base32::ZBASE32_ALPHABET,
        other => panic!("base32.json: unknown alphabet {other}"),
    }
}

#[test]
fn base32_nopad_encode_and_decode() {
    let v: B32Vectors = load_json("gocompat/base32.json");
    let mut failures = Vec::new();
    for case in &v.encode {
        let got = base32::encode_nopad(alphabet(&case.alphabet), &hex(&case.in_hex));
        if got != case.out {
            failures.push(format!(
                "encode {} {}: got {got}, want {}",
                case.alphabet, case.in_hex, case.out
            ));
        }
    }
    for case in &v.decode {
        let got = base32::decode_nopad(alphabet(&case.alphabet), &hex(&case.in_hex));
        if let Some(m) = decode_mismatch(got, &case.out_hex, &case.error) {
            failures.push(format!("decode {} {}: {m}", case.alphabet, case.in_hex));
        }
    }
    check("base32", v.encode.len() + v.decode.len(), &failures);
}

#[derive(Deserialize)]
struct PathCase {
    in_hex: String,
    out_hex: String,
}

#[derive(Deserialize)]
struct JoinCase {
    elems_hex: Vec<String>,
    out_hex: String,
}

#[derive(Deserialize)]
struct RelCase {
    base_hex: String,
    target_hex: String,
    out_hex: Option<String>,
    error_hex: String,
}

#[derive(Deserialize)]
struct PathVectors {
    clean: Vec<PathCase>,
    join: Vec<JoinCase>,
    dir: Vec<PathCase>,
    base: Vec<PathCase>,
    rel: Vec<RelCase>,
    abs: Vec<PathCase>,
}

fn path_cases(
    name: &str,
    cases: &[PathCase],
    f: impl Fn(&[u8]) -> Vec<u8>,
    failures: &mut Vec<String>,
) {
    for case in cases {
        let got = f(&hex(&case.in_hex));
        if got != hex(&case.out_hex) {
            failures.push(format!(
                "{name}({:?}) = {:?}, want {:?}",
                case.in_hex,
                gohex::encode(&got),
                case.out_hex
            ));
        }
    }
}

#[test]
fn path_functions() {
    let v: PathVectors = load_json("gocompat/path.json");
    let mut failures = Vec::new();
    path_cases("Clean", &v.clean, path::clean, &mut failures);
    path_cases("Dir", &v.dir, path::dir, &mut failures);
    path_cases("Base", &v.base, path::base, &mut failures);
    path_cases(
        "Abs",
        &v.abs,
        |p| path::abs(p).unwrap_or_else(|e| format!("error {e}").into_bytes()),
        &mut failures,
    );
    for case in &v.join {
        let elems: Vec<Vec<u8>> = case.elems_hex.iter().map(|e| hex(e)).collect();
        let refs: Vec<&[u8]> = elems.iter().map(Vec::as_slice).collect();
        let got = path::join(&refs);
        if got != hex(&case.out_hex) {
            failures.push(format!(
                "Join({:?}) = {:?}, want {:?}",
                case.elems_hex,
                gohex::encode(&got),
                case.out_hex
            ));
        }
    }
    for case in &v.rel {
        let (base, target) = (hex(&case.base_hex), hex(&case.target_hex));
        let got = path::rel(&base, &target);
        let want = case.out_hex.as_deref().map(hex);
        // `rel` returns None where Go errors; callers that print the error build Go's text from the
        // inputs, so check that formula against Go's bytes too.
        let want_err = hex(&case.error_hex);
        let formula = match got {
            Some(_) => Vec::new(),
            None => [&b"Rel: can't make "[..], &target, b" relative to ", &base].concat(),
        };
        if got != want || want.is_none() == want_err.is_empty() || formula != want_err {
            failures.push(format!(
                "Rel({:?}, {:?}) = {:?}, want {:?} ({:?})",
                case.base_hex,
                case.target_hex,
                got.map(|g| gohex::encode(&g)),
                case.out_hex,
                String::from_utf8_lossy(&want_err)
            ));
        }
    }
    let total =
        v.clean.len() + v.join.len() + v.dir.len() + v.base.len() + v.rel.len() + v.abs.len();
    check("path", total, &failures);
}
