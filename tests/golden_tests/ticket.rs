//! Golden tests of `dstore-ticket` (`ticket/encode.json`, `ticket/parse.json`, `ticket/curve.json`, and the
//! `ticket` section of `wire/decode.json`; generators `tools/vectorgen/family_ticket.go` and
//! `family_wire.go`, schemas in VECTORS.md).

use dstore_gocompat::base32;
use dstore_testkit::golden::{self, hex, load_json};
use dstore_ticket::{Member, Ticket, is_valid_public_key, parse, parse_endpoint_id};
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

/// `TicketJSON` member: `id` null is nil; `addrs` null and `[]` are both empty.
#[derive(Deserialize)]
struct MemberJson {
    id: Option<String>,
    addrs: Option<Vec<String>>,
}

/// `TicketJSON`: `members` null is `None`, `[]` is `Some(vec![])`.
#[derive(Deserialize)]
struct TicketJson {
    cluster_id: Option<String>,
    #[serde(deserialize_with = "golden::decimal_u64")]
    incarnation: u64,
    members: Option<Vec<MemberJson>>,
}

impl TicketJson {
    fn to_ticket(&self) -> Ticket {
        Ticket {
            cluster_id: self.cluster_id.as_deref().map(hex),
            incarnation: self.incarnation,
            members: self.members.as_ref().map(|ms| {
                ms.iter()
                    .map(|m| Member {
                        id: m.id.as_deref().map(hex),
                        addrs: m.addrs.clone().unwrap_or_default(),
                    })
                    .collect()
            }),
        }
    }
}

/// `ticket.Parse(input)`, and on success the re-encoding and `IDs()`.
#[derive(Deserialize)]
struct ParseResult {
    ok: bool,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    ticket: Option<TicketJson>,
    #[serde(default)]
    encoded: Option<String>,
    ids: String,
}

fn check_parse(input: &[u8], want: &ParseResult) -> Result<(), String> {
    match (parse(input), want.ok) {
        (Ok(t), true) => {
            let Some(wt) = want.ticket.as_ref().map(TicketJson::to_ticket) else {
                return Err("vector has ok without a ticket".into());
            };
            if t != wt {
                return Err(format!("ticket {t:?}, want {wt:?}"));
            }
            let encoded = t.encode();
            if Some(&encoded) != want.encoded.as_ref() {
                return Err(format!("encode {encoded:?}, want {:?}", want.encoded));
            }
            if t.ids() != want.ids {
                return Err(format!("ids {:?}, want {:?}", t.ids(), want.ids));
            }
            Ok(())
        }
        (Ok(t), false) => Err(format!("parsed to {t:?}, want error {:?}", want.error)),
        (Err(e), true) => Err(format!("error {:?}, want ok", e.to_string())),
        (Err(e), false) => {
            let text = e.to_string();
            if Some(&text) != want.error.as_ref() {
                return Err(format!("error {text:?}, want {:?}", want.error));
            }
            Ok(())
        }
    }
}

#[derive(Deserialize)]
struct EncodeCase {
    name: String,
    ticket: TicketJson,
    cbor_hex: String,
    encoded: String,
    ids: String,
    parse: ParseResult,
}

#[derive(Deserialize)]
struct EncodeFile {
    cases: Vec<EncodeCase>,
}

#[test]
fn encode() {
    let file: EncodeFile = load_json("ticket/encode.json");
    let mut failures = Vec::new();
    for c in &file.cases {
        let t = c.ticket.to_ticket();
        let cbor = dstore_codec::marshal(&t);
        if dstore_gocompat::hex::encode(&cbor) != c.cbor_hex {
            failures.push(format!(
                "{}: cbor {}, want {}",
                c.name,
                dstore_gocompat::hex::encode(&cbor),
                c.cbor_hex
            ));
            continue;
        }
        match dstore_codec::unmarshal::<Ticket>(&cbor) {
            Ok(back) if back == t => {}
            other => failures.push(format!("{}: decode {other:?}, want {t:?}", c.name)),
        }
        if t.encode() != c.encoded || t.to_string() != c.encoded {
            failures.push(format!(
                "{}: encode {:?}, want {:?}",
                c.name,
                t.encode(),
                c.encoded
            ));
        }
        if t.ids() != c.ids {
            failures.push(format!("{}: ids {:?}, want {:?}", c.name, t.ids(), c.ids));
        }
        let members = c.ticket.members.as_ref().map_or(0, Vec::len);
        if t.members().len() != members {
            failures.push(format!(
                "{}: members() has {}, want {members}",
                c.name,
                t.members().len()
            ));
        }
        if let Err(e) = check_parse(c.encoded.as_bytes(), &c.parse) {
            failures.push(format!("{}: parse(encoded): {e}", c.name));
        }
    }
    check("ticket/encode.json", file.cases.len(), &failures);
}

#[derive(Deserialize)]
struct ParseCase {
    name: String,
    input: Option<String>,
    input_hex: String,
    #[serde(flatten)]
    result: ParseResult,
}

#[derive(Deserialize)]
struct ParseFile {
    cases: Vec<ParseCase>,
}

#[test]
fn parse_cases() {
    let file: ParseFile = load_json("ticket/parse.json");
    let mut failures = Vec::new();
    for c in &file.cases {
        let input = hex(&c.input_hex);
        if let Some(s) = &c.input
            && s.as_bytes() != input.as_slice()
        {
            failures.push(format!("{}: vector input and input_hex differ", c.name));
            continue;
        }
        if let Err(e) = check_parse(&input, &c.result) {
            failures.push(format!("{}: {e}", c.name));
        }
    }
    check("ticket/parse.json", file.cases.len(), &failures);
}

#[derive(Deserialize)]
struct CurveCase {
    name: String,
    bytes: String,
    valid: bool,
    #[serde(default)]
    hex_parse_error: Option<String>,
    #[serde(default)]
    base32_parse_error: Option<String>,
}

#[derive(Deserialize)]
struct CurveFile {
    cases: Vec<CurveCase>,
}

fn check_endpoint_id(
    form: &str,
    input: &str,
    want_id: &[u8; 32],
    want_err: Option<&String>,
) -> Result<(), String> {
    match (parse_endpoint_id(input.as_bytes()), want_err) {
        (Ok(id), None) if &id == want_id => Ok(()),
        (Ok(id), None) => Err(format!(
            "{form}: id {}, want {}",
            dstore_gocompat::hex::encode(&id),
            dstore_gocompat::hex::encode(want_id)
        )),
        (Ok(_), Some(e)) => Err(format!("{form}: parsed, want error {e:?}")),
        (Err(e), None) => Err(format!("{form}: error {:?}, want ok", e.to_string())),
        (Err(e), Some(want)) if &e.to_string() == want => Ok(()),
        (Err(e), Some(want)) => Err(format!("{form}: error {:?}, want {want:?}", e.to_string())),
    }
}

/// PORTING §4.4: `iroh_base::PublicKey::from_bytes` (through `is_valid_public_key`) must accept exactly the
/// points filippo `edwards25519.Point.SetBytes` accepts, and `parse_endpoint_id` must agree for both string
/// forms.
#[test]
fn curve() {
    let file: CurveFile = load_json("ticket/curve.json");
    let mut failures = Vec::new();
    for c in &file.cases {
        let Ok(b) = <[u8; 32]>::try_from(hex(&c.bytes).as_slice()) else {
            failures.push(format!("{}: vector bytes are not 32 bytes", c.name));
            continue;
        };
        if is_valid_public_key(&b) != c.valid {
            failures.push(format!(
                "{}: is_valid_public_key {}, want {}",
                c.name, !c.valid, c.valid
            ));
        }
        if c.valid != c.hex_parse_error.is_none() {
            failures.push(format!("{}: vector valid and hex error disagree", c.name));
        }
        let hex_form = dstore_gocompat::hex::encode(&b);
        let b32_form = base32::encode_nopad(base32::STD_ALPHABET, &b).to_ascii_lowercase();
        for (form, input, want_err) in [
            ("hex", &hex_form, c.hex_parse_error.as_ref()),
            ("base32", &b32_form, c.base32_parse_error.as_ref()),
        ] {
            if let Err(e) = check_endpoint_id(form, input, &b, want_err) {
                failures.push(format!("{}: {e}", c.name));
            }
        }
        // The same id through ticket::parse: an id list of one field.
        let want = match &c.hex_parse_error {
            None => Ok(hex_form.clone()),
            Some(e) => Err(format!(
                "ticket: \"{hex_form}\" is neither a dstore1 ticket nor a node id: {e}"
            )),
        };
        let got = parse(hex_form.as_bytes())
            .map(|t| t.ids())
            .map_err(|e| e.to_string());
        if got != want {
            failures.push(format!("{}: parse(hex) {got:?}, want {want:?}", c.name));
        }
    }
    check("ticket/curve.json", file.cases.len(), &failures);
}

#[derive(Deserialize)]
struct DecodeCase {
    name: String,
    payload_hex: Option<String>,
    ok: bool,
    #[serde(default)]
    go_error: Option<String>,
    #[serde(default)]
    value: Option<TicketJson>,
    #[serde(default)]
    canonical_hex: Option<String>,
}

#[derive(Deserialize)]
struct DecodeFile {
    ticket: Vec<DecodeCase>,
}

/// `codec.Unmarshal(payload, &ticket.Ticket)`: decisions, values and re-encodings (`wire/decode.json`, the
/// `ticket` section: the field matrix of codec-wire-ticket G4 and the structure cases).
#[test]
fn decode() {
    let file: DecodeFile = load_json("wire/decode.json");
    let mut failures = Vec::new();
    for c in &file.ticket {
        let Some(payload) = c.payload_hex.as_deref().map(hex) else {
            failures.push(format!("{}: no payload_hex", c.name));
            continue;
        };
        match (dstore_codec::unmarshal::<Ticket>(&payload), c.ok) {
            (Ok(t), true) => {
                let Some(want) = c.value.as_ref().map(TicketJson::to_ticket) else {
                    failures.push(format!("{}: vector has ok without a value", c.name));
                    continue;
                };
                if t != want {
                    failures.push(format!("{}: value {t:?}, want {want:?}", c.name));
                }
                let canonical = dstore_gocompat::hex::encode(&dstore_codec::marshal(&t));
                if Some(&canonical) != c.canonical_hex.as_ref() {
                    failures.push(format!(
                        "{}: canonical {canonical}, want {:?}",
                        c.name, c.canonical_hex
                    ));
                }
            }
            (Ok(t), false) => failures.push(format!(
                "{}: decoded {t:?}, want error {:?}",
                c.name, c.go_error
            )),
            (Err(e), true) => {
                failures.push(format!("{}: error {:?}, want ok", c.name, e.to_string()))
            }
            (Err(e), false) => {
                if Some(&e.to_string()) != c.go_error.as_ref() {
                    failures.push(format!(
                        "{}: error {:?}, want {:?}",
                        c.name,
                        e.to_string(),
                        c.go_error
                    ));
                }
            }
        }
    }
    check("wire/decode.json ticket", file.ticket.len(), &failures);
}
