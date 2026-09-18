//! Golden tests of `dstore-codec`: `codec/encode.json` (canonical encoding of scalars and test structs)
//! and `codec/decode.json` (pass-1 well-formedness, decoding decisions, resulting values and verbatim
//! error texts). The structs mirror the Go test structs of `tools/vectorgen/family_codec.go`; the schema
//! is in `tools/vectorgen/docs/codec.md`.

use dstore_codec::{Enc, Struct, cbor_struct, marshal, unmarshal, well_formed};
use dstore_testkit::golden::{self, Payload};
use serde::Deserialize;
use serde_json::{Map, Value, json};

cbor_struct! {
    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct Leaf = "main.codecLeaf" {
        0 => name: String = "string",
        1 => key: Option<Vec<u8>> = "[]uint8",
        2 => n: i64 = "int64" [omitempty],
    }
}

cbor_struct! {
    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct Omit = "main.codecOmit" {
        0 => int: i64 = "int" [omitempty],
        1 => i64v: i64 = "int64" [omitempty],
        2 => u8v: u8 = "uint8" [omitempty],
        3 => u16v: u16 = "uint16" [omitempty],
        4 => u32v: u32 = "uint32" [omitempty],
        5 => u64v: u64 = "uint64" [omitempty],
        6 => flag: bool = "bool" [omitempty],
        7 => f64v: f64 = "float64" [omitempty],
        8 => text: String = "string" [omitempty],
        9 => bytes: Vec<u8> = "[]uint8" [omitempty],
        10 => list: Vec<Vec<u8>> = "[][]uint8" [omitempty],
        11 => list_n: Vec<Option<Vec<u8>>> = "[][]uint8" [omitempty],
        12 => strs: Vec<String> = "[]string" [omitempty],
        13 => u16s: Vec<u16> = "[]uint16" [omitempty],
        14 => leaves: Vec<Leaf> = "[]main.codecLeaf" [omitempty],
        15 => ptr: Option<Box<Leaf>> = "main.codecLeaf" [omitempty],
        24 => far24: u64 = "uint64" [omitempty],
        256 => far256: bool = "bool" [omitempty],
        65536 => far65536: String = "string" [omitempty],
    }
}

cbor_struct! {
    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct NoOmit = "main.codecNoOmit" {
        0 => int: i64 = "int",
        1 => i64v: i64 = "int64",
        2 => u8v: u8 = "uint8",
        3 => u16v: u16 = "uint16",
        4 => u32v: u32 = "uint32",
        5 => u64v: u64 = "uint64",
        6 => flag: bool = "bool",
        7 => f64v: f64 = "float64",
        8 => text: String = "string",
        9 => bytes: Option<Vec<u8>> = "[]uint8",
        14 => leaves: Option<Vec<Leaf>> = "[]main.codecLeaf",
        15 => ptr: Option<Box<Leaf>> = "main.codecLeaf",
    }
}

cbor_struct! {
    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct Mid = "main.codecMid" {
        0 => leaves: Vec<Leaf> = "[]main.codecLeaf" [omitempty],
        1 => leaf: Option<Box<Leaf>> = "main.codecLeaf" [omitempty],
    }
}

cbor_struct! {
    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct Top = "main.codecTop" {
        0 => mids: Vec<Mid> = "[]main.codecMid" [omitempty],
        1 => mid: Option<Box<Mid>> = "main.codecMid" [omitempty],
        2 => name: String = "string" [omitempty],
    }
}

cbor_struct! {
    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct Wide = "main.codecWide" {
        0 => f0: u8 = "uint8" [omitempty],
        1 => f1: u8 = "uint8" [omitempty],
        2 => f2: u8 = "uint8" [omitempty],
        3 => f3: u8 = "uint8" [omitempty],
        4 => f4: u8 = "uint8" [omitempty],
        5 => f5: u8 = "uint8" [omitempty],
        6 => f6: u8 = "uint8" [omitempty],
        7 => f7: u8 = "uint8" [omitempty],
        8 => f8: u8 = "uint8" [omitempty],
        9 => f9: u8 = "uint8" [omitempty],
        10 => f10: u8 = "uint8" [omitempty],
        11 => f11: u8 = "uint8" [omitempty],
        12 => f12: u8 = "uint8" [omitempty],
        13 => f13: u8 = "uint8" [omitempty],
        14 => f14: u8 = "uint8" [omitempty],
        15 => f15: u8 = "uint8" [omitempty],
        16 => f16: u8 = "uint8" [omitempty],
        17 => f17: u8 = "uint8" [omitempty],
        18 => f18: u8 = "uint8" [omitempty],
        19 => f19: u8 = "uint8" [omitempty],
        20 => f20: u8 = "uint8" [omitempty],
        21 => f21: u8 = "uint8" [omitempty],
        22 => f22: u8 = "uint8" [omitempty],
        23 => f23: u8 = "uint8" [omitempty],
        24 => f24: u8 = "uint8" [omitempty],
    }
}

/// The JSON form of a test struct (the Go `codecJSON` rendering).
///
/// `from_json` reads Go's rendering, which leaves out zero fields, into the Rust representation. That
/// loses exactly what the Rust types do not keep: nil versus empty for omitempty collections, and nil
/// elements of `Vec<Vec<u8>>` (codec-wire-ticket R6). `to_json` renders every field, so comparing
/// `to_json(decoded)` with `to_json(from_json(go_value))` checks everything the Rust value can hold.
trait Mirror: Struct {
    fn to_json(&self) -> Value;
    fn from_json(v: &Value) -> Self;
}

fn fields<'a>(v: &'a Value, ty: &str) -> &'a Map<String, Value> {
    match v.as_object() {
        Some(m) => m,
        None => panic!("{ty}: want a JSON object, got {v}"),
    }
}

fn j_u64(v: &Value) -> u64 {
    match v.as_str().map(str::parse::<u64>) {
        Some(Ok(n)) => n,
        _ => panic!("want a decimal u64 string, got {v}"),
    }
}

fn j_i64(v: &Value) -> i64 {
    match v.as_str().map(str::parse::<i64>) {
        Some(Ok(n)) => n,
        _ => panic!("want a decimal i64 string, got {v}"),
    }
}

fn j_num<T: TryFrom<u64>>(v: &Value) -> T {
    match v.as_u64().map(T::try_from) {
        Some(Ok(n)) => n,
        _ => panic!("want a small unsigned number, got {v}"),
    }
}

fn j_bool(v: &Value) -> bool {
    match v.as_bool() {
        Some(b) => b,
        None => panic!("want a bool, got {v}"),
    }
}

fn j_str(v: &Value) -> String {
    match v.as_str() {
        Some(s) => s.to_owned(),
        None => panic!("want a string, got {v}"),
    }
}

fn j_hex(v: &Value) -> Option<Vec<u8>> {
    if v.is_null() {
        return None;
    }
    Some(golden::hex(&j_str(v)))
}

fn j_list(v: &Value) -> Option<&Vec<Value>> {
    if v.is_null() {
        return None;
    }
    match v.as_array() {
        Some(a) => Some(a),
        None => panic!("want an array or null, got {v}"),
    }
}

fn j_structs<T: Mirror>(v: &Value) -> Option<Vec<T>> {
    j_list(v).map(|a| a.iter().map(T::from_json).collect())
}

fn t_hex(b: &[u8]) -> Value {
    Value::String(hex::encode(b))
}

fn t_opt_hex(b: &Option<Vec<u8>>) -> Value {
    match b {
        None => Value::Null,
        Some(b) => t_hex(b),
    }
}

fn t_f64(f: f64) -> Value {
    Value::String(f.to_bits().to_string())
}

fn t_structs<T: Mirror>(v: &[T]) -> Value {
    Value::Array(v.iter().map(Mirror::to_json).collect())
}

impl Mirror for Leaf {
    fn to_json(&self) -> Value {
        json!({"Name": self.name, "Key": t_opt_hex(&self.key), "N": self.n.to_string()})
    }
    fn from_json(v: &Value) -> Self {
        let mut s = Leaf::default();
        for (k, x) in fields(v, "Leaf") {
            match k.as_str() {
                "Name" => s.name = j_str(x),
                "Key" => s.key = j_hex(x),
                "N" => s.n = j_i64(x),
                other => panic!("Leaf: unknown field {other}"),
            }
        }
        s
    }
}

impl Mirror for Omit {
    fn to_json(&self) -> Value {
        json!({
            "Int": self.int.to_string(),
            "I64": self.i64v.to_string(),
            "U8": self.u8v,
            "U16": self.u16v,
            "U32": self.u32v,
            "U64": self.u64v.to_string(),
            "Bool": self.flag,
            "F64": t_f64(self.f64v),
            "Str": self.text,
            "Bytes": t_hex(&self.bytes),
            "List": self.list.iter().map(|b| t_hex(b)).collect::<Vec<_>>(),
            "ListN": self.list_n.iter().map(t_opt_hex).collect::<Vec<_>>(),
            "Strs": self.strs,
            "U16s": self.u16s,
            "Leaves": t_structs(&self.leaves),
            "Ptr": self.ptr.as_ref().map(|p| p.to_json()),
            "Far24": self.far24.to_string(),
            "Far256": self.far256,
            "Far65536": self.far65536,
        })
    }
    fn from_json(v: &Value) -> Self {
        let mut s = Omit::default();
        for (k, x) in fields(v, "Omit") {
            match k.as_str() {
                "Int" => s.int = j_i64(x),
                "I64" => s.i64v = j_i64(x),
                "U8" => s.u8v = j_num(x),
                "U16" => s.u16v = j_num(x),
                "U32" => s.u32v = j_num(x),
                "U64" => s.u64v = j_u64(x),
                "Bool" => s.flag = j_bool(x),
                "F64" => s.f64v = f64::from_bits(j_u64(x)),
                "Str" => s.text = j_str(x),
                "Bytes" => s.bytes = j_hex(x).unwrap_or_default(),
                // R6: a nil element is an empty vector.
                "List" => {
                    s.list = j_list(x)
                        .map(|a| a.iter().map(|b| j_hex(b).unwrap_or_default()).collect())
                        .unwrap_or_default()
                }
                "ListN" => {
                    s.list_n = j_list(x)
                        .map(|a| a.iter().map(j_hex).collect())
                        .unwrap_or_default()
                }
                "Strs" => {
                    s.strs = j_list(x)
                        .map(|a| a.iter().map(j_str).collect())
                        .unwrap_or_default()
                }
                "U16s" => {
                    s.u16s = j_list(x)
                        .map(|a| a.iter().map(j_num).collect())
                        .unwrap_or_default()
                }
                "Leaves" => s.leaves = j_structs(x).unwrap_or_default(),
                "Ptr" => s.ptr = (!x.is_null()).then(|| Box::new(Leaf::from_json(x))),
                "Far24" => s.far24 = j_u64(x),
                "Far256" => s.far256 = j_bool(x),
                "Far65536" => s.far65536 = j_str(x),
                other => panic!("Omit: unknown field {other}"),
            }
        }
        s
    }
}

impl Mirror for NoOmit {
    fn to_json(&self) -> Value {
        json!({
            "Int": self.int.to_string(),
            "I64": self.i64v.to_string(),
            "U8": self.u8v,
            "U16": self.u16v,
            "U32": self.u32v,
            "U64": self.u64v.to_string(),
            "Bool": self.flag,
            "F64": t_f64(self.f64v),
            "Str": self.text,
            "Bytes": t_opt_hex(&self.bytes),
            "Leaves": self.leaves.as_deref().map(t_structs),
            "Ptr": self.ptr.as_ref().map(|p| p.to_json()),
        })
    }
    fn from_json(v: &Value) -> Self {
        let mut s = NoOmit::default();
        for (k, x) in fields(v, "NoOmit") {
            match k.as_str() {
                "Int" => s.int = j_i64(x),
                "I64" => s.i64v = j_i64(x),
                "U8" => s.u8v = j_num(x),
                "U16" => s.u16v = j_num(x),
                "U32" => s.u32v = j_num(x),
                "U64" => s.u64v = j_u64(x),
                "Bool" => s.flag = j_bool(x),
                "F64" => s.f64v = f64::from_bits(j_u64(x)),
                "Str" => s.text = j_str(x),
                "Bytes" => s.bytes = j_hex(x),
                "Leaves" => s.leaves = j_structs(x),
                "Ptr" => s.ptr = (!x.is_null()).then(|| Box::new(Leaf::from_json(x))),
                other => panic!("NoOmit: unknown field {other}"),
            }
        }
        s
    }
}

impl Mirror for Mid {
    fn to_json(&self) -> Value {
        json!({"Leaves": t_structs(&self.leaves), "Leaf": self.leaf.as_ref().map(|p| p.to_json())})
    }
    fn from_json(v: &Value) -> Self {
        let mut s = Mid::default();
        for (k, x) in fields(v, "Mid") {
            match k.as_str() {
                "Leaves" => s.leaves = j_structs(x).unwrap_or_default(),
                "Leaf" => s.leaf = (!x.is_null()).then(|| Box::new(Leaf::from_json(x))),
                other => panic!("Mid: unknown field {other}"),
            }
        }
        s
    }
}

impl Mirror for Top {
    fn to_json(&self) -> Value {
        json!({
            "Mids": t_structs(&self.mids),
            "Mid": self.mid.as_ref().map(|p| p.to_json()),
            "Name": self.name,
        })
    }
    fn from_json(v: &Value) -> Self {
        let mut s = Top::default();
        for (k, x) in fields(v, "Top") {
            match k.as_str() {
                "Mids" => s.mids = j_structs(x).unwrap_or_default(),
                "Mid" => s.mid = (!x.is_null()).then(|| Box::new(Mid::from_json(x))),
                "Name" => s.name = j_str(x),
                other => panic!("Top: unknown field {other}"),
            }
        }
        s
    }
}

impl Wide {
    fn slots(&mut self) -> [&mut u8; 25] {
        [
            &mut self.f0,
            &mut self.f1,
            &mut self.f2,
            &mut self.f3,
            &mut self.f4,
            &mut self.f5,
            &mut self.f6,
            &mut self.f7,
            &mut self.f8,
            &mut self.f9,
            &mut self.f10,
            &mut self.f11,
            &mut self.f12,
            &mut self.f13,
            &mut self.f14,
            &mut self.f15,
            &mut self.f16,
            &mut self.f17,
            &mut self.f18,
            &mut self.f19,
            &mut self.f20,
            &mut self.f21,
            &mut self.f22,
            &mut self.f23,
            &mut self.f24,
        ]
    }
}

impl Mirror for Wide {
    fn to_json(&self) -> Value {
        let mut w = self.clone();
        let mut m = Map::new();
        for (i, v) in w.slots().into_iter().enumerate() {
            m.insert(format!("F{i}"), json!(*v));
        }
        Value::Object(m)
    }
    fn from_json(v: &Value) -> Self {
        let mut s = Wide::default();
        for (k, x) in fields(v, "Wide") {
            let slot = k
                .strip_prefix('F')
                .and_then(|i| i.parse::<usize>().ok())
                .filter(|&i| i < 25);
            match slot {
                Some(i) => *s.slots()[i] = j_num(x),
                None => panic!("Wide: unknown field {k}"),
            }
        }
        s
    }
}

#[derive(Deserialize)]
struct EncodeFile {
    scalars: Vec<ScalarCase>,
    structs: Vec<StructCase>,
}

#[derive(Deserialize)]
struct ScalarCase {
    name: String,
    kind: String,
    #[serde(default)]
    u64: Option<String>,
    #[serde(default)]
    i64: Option<String>,
    #[serde(default)]
    bits: Option<String>,
    #[serde(default)]
    bool: Option<bool>,
    #[serde(default)]
    data: Option<Payload>,
    #[serde(default)]
    output_hex: Option<String>,
    #[serde(default)]
    output_head_hex: Option<String>,
}

#[derive(Deserialize)]
struct StructCase {
    name: String,
    #[serde(rename = "type")]
    ty: String,
    value: Value,
    output_hex: String,
}

#[derive(Deserialize)]
struct DecodeFile {
    cases: Vec<DecodeCase>,
}

#[derive(Deserialize)]
struct DecodeCase {
    name: String,
    #[serde(rename = "type")]
    ty: String,
    input_hex: String,
    #[serde(default)]
    repeat_hex: String,
    #[serde(default)]
    repeat_count: usize,
    #[serde(default)]
    suffix_hex: String,
    wellformed_error: Option<String>,
    go_error: Option<String>,
    value: Value,
    value_cbor_hex: Option<String>,
}

impl DecodeCase {
    fn input(&self) -> Vec<u8> {
        let mut b = golden::hex(&self.input_hex);
        let repeat = golden::hex(&self.repeat_hex);
        for _ in 0..self.repeat_count {
            b.extend_from_slice(&repeat);
        }
        b.extend_from_slice(&golden::hex(&self.suffix_hex));
        b
    }
}

/// Fails the test with every mismatch, not just the first.
fn report(what: &str, total: usize, failures: &[String]) {
    assert!(total > 0, "{what}: no cases in the vector file");
    if !failures.is_empty() {
        let shown: Vec<&str> = failures.iter().take(40).map(String::as_str).collect();
        panic!(
            "{what}: {} of {total} cases failed:\n{}",
            failures.len(),
            shown.join("\n")
        );
    }
}

fn run_scalar(c: &ScalarCase) -> Result<(), String> {
    let mut e = Enc::new();
    let parse_u64 = |s: &Option<String>| s.as_deref().and_then(|s| s.parse::<u64>().ok());
    let expected = match c.kind.as_str() {
        "uint" => {
            e.uint(parse_u64(&c.u64).ok_or("missing u64")?);
            c.output_hex.clone()
        }
        "int" => {
            let v = c.i64.as_deref().and_then(|s| s.parse::<i64>().ok());
            e.int(v.ok_or("missing i64")?);
            c.output_hex.clone()
        }
        "float64" => {
            e.f64_canonical(f64::from_bits(parse_u64(&c.bits).ok_or("missing bits")?));
            c.output_hex.clone()
        }
        "bool" => {
            e.bool(c.bool.ok_or("missing bool")?);
            c.output_hex.clone()
        }
        "null" => {
            e.null();
            c.output_hex.clone()
        }
        "bytes" | "text" => {
            let data = c.data.as_ref().ok_or("missing data")?.bytes();
            if c.kind == "bytes" {
                e.bytes(&data);
            } else {
                let s = std::str::from_utf8(&data).map_err(|err| format!("text data: {err}"))?;
                e.text(s);
            }
            c.output_head_hex
                .as_ref()
                .map(|head| format!("{head}{}", hex::encode(&data)))
        }
        other => return Err(format!("unknown kind {other}")),
    };
    let expected = expected.ok_or("missing expected output")?;
    let got = hex::encode(e.into_bytes());
    if got != expected {
        return Err(format!("got {}, want {}", trim(&got), trim(&expected)));
    }
    Ok(())
}

/// Shortens a long hex string for a failure message.
fn trim(s: &str) -> String {
    if s.len() <= 200 {
        return s.to_owned();
    }
    format!("{}…({} chars)", &s[..200], s.len())
}

fn run_struct<T: Mirror>(c: &StructCase) -> Result<(), String> {
    let v = T::from_json(&c.value);
    let got = hex::encode(marshal(&v));
    if got != c.output_hex {
        return Err(format!("got {}, want {}", trim(&got), trim(&c.output_hex)));
    }
    Ok(())
}

/// Whether Go's value holds a nil element in `Omit.List`, which Rust decodes as an empty vector (R6)
/// and so re-encodes differently.
fn has_nil_list_element(v: &Value) -> bool {
    v.get("List")
        .and_then(Value::as_array)
        .is_some_and(|a| a.iter().any(Value::is_null))
}

fn run_decode<T: Mirror>(c: &DecodeCase, input: &[u8]) -> Result<(), String> {
    match (unmarshal::<T>(input), &c.go_error) {
        (Err(e), Some(want)) => {
            if e.to_string() != *want {
                return Err(format!("error {:?}, want {want:?}", e.to_string()));
            }
        }
        (Err(e), None) => return Err(format!("error {:?}, want success", e.to_string())),
        (Ok(v), Some(want)) => {
            return Err(format!("decoded {}, want error {want:?}", v.to_json()));
        }
        (Ok(v), None) => {
            let got = v.to_json();
            let want = T::from_json(&c.value).to_json();
            if got != want {
                return Err(format!("value {got}, want {want}"));
            }
            let want_cbor = c
                .value_cbor_hex
                .as_deref()
                .ok_or("missing value_cbor_hex")?;
            let got_cbor = hex::encode(marshal(&v));
            if got_cbor != want_cbor && !has_nil_list_element(&c.value) {
                return Err(format!(
                    "re-encoded {}, want {}",
                    trim(&got_cbor),
                    trim(want_cbor)
                ));
            }
        }
    }
    Ok(())
}

#[test]
fn encode_scalars() {
    let file: EncodeFile = golden::load_json("codec/encode.json");
    let failures: Vec<String> = file
        .scalars
        .iter()
        .filter_map(|c| run_scalar(c).err().map(|e| format!("{}: {e}", c.name)))
        .collect();
    report("codec/encode.json scalars", file.scalars.len(), &failures);
}

#[test]
fn encode_structs() {
    let file: EncodeFile = golden::load_json("codec/encode.json");
    let failures: Vec<String> = file
        .structs
        .iter()
        .filter_map(|c| {
            let r = match c.ty.as_str() {
                "Leaf" => run_struct::<Leaf>(c),
                "Omit" => run_struct::<Omit>(c),
                "NoOmit" => run_struct::<NoOmit>(c),
                "Mid" => run_struct::<Mid>(c),
                "Top" => run_struct::<Top>(c),
                "Wide" => run_struct::<Wide>(c),
                other => Err(format!("unknown type {other}")),
            };
            r.err().map(|e| format!("{}: {e}", c.name))
        })
        .collect();
    report("codec/encode.json structs", file.structs.len(), &failures);
}

#[test]
fn decode_cases() {
    let file: DecodeFile = golden::load_json("codec/decode.json");
    let mut failures = Vec::new();
    for c in &file.cases {
        let input = c.input();
        let wf = well_formed(&input).err().map(|e| e.to_string());
        if wf != c.wellformed_error {
            failures.push(format!(
                "{}: well_formed {wf:?}, want {:?}",
                c.name, c.wellformed_error
            ));
        }
        let r = match c.ty.as_str() {
            "Leaf" => run_decode::<Leaf>(c, &input),
            "Omit" => run_decode::<Omit>(c, &input),
            "NoOmit" => run_decode::<NoOmit>(c, &input),
            "Mid" => run_decode::<Mid>(c, &input),
            "Top" => run_decode::<Top>(c, &input),
            "Wide" => run_decode::<Wide>(c, &input),
            other => Err(format!("unknown type {other}")),
        };
        if let Err(e) = r {
            failures.push(format!("{}: {e}", c.name));
        }
    }
    report("codec/decode.json", file.cases.len(), &failures);
}
