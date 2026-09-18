//! dstore `ticket/ticket.go`: `dstore1` tickets, and go-iroh v0.2.0 `key/key.go` `ParseEndpointID`
//! (`decodeBase32OrHex`, `NewPublicKey`, the z-base-32 hint, `key_core.go`).
//!
//! The curve check uses `iroh_base::PublicKey::from_bytes`; a golden vector confirms it accepts exactly
//! what filippo `edwards25519.Point.SetBytes` accepts.
//!
//! Spec: PORTING.md §4.4; port-notes/codec-wire-ticket.md §2.4, §3.4, §4.4.
#![deny(unsafe_op_in_unsafe_fn)]

use std::collections::HashSet;

use dstore_codec::cbor_struct;
use dstore_gocompat::{base32, hex, strings};

/// The ticket string prefix.
pub const PREFIX: &str = "dstore1";

/// go-iroh `key.PublicKeySize`.
const PUBLIC_KEY_SIZE: usize = 32;

cbor_struct! {
    /// `ticket.Member`: one bootstrap node.
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct Member = "ticket.Member" {
        0 => id: Option<Vec<u8>> = "[]uint8",
        1 => addrs: Vec<String> = "[]string" [omitempty],
    }
}

cbor_struct! {
    /// `ticket.Ticket`: the bootstrap information.
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct Ticket = "ticket.Ticket" {
        0 => cluster_id: Option<Vec<u8>> = "[]uint8",
        1 => incarnation: u64 = "uint64",
        2 => members: Option<Vec<Member>> = "[]ticket.Member",
    }
}

impl Ticket {
    /// `Ticket.Encode`: "dstore1" + lower(base32 std nopad (CBOR)).
    pub fn encode(&self) -> String {
        let body = base32::encode_nopad(base32::STD_ALPHABET, &dstore_codec::marshal(self));
        let mut s = String::with_capacity(PREFIX.len() + body.len());
        s.push_str(PREFIX);
        // strings.ToLower of an ASCII string.
        s.push_str(&body.to_ascii_lowercase());
        s
    }

    /// `Ticket.IDs`: hex of the 32-byte member ids, first occurrence, ","-joined.
    pub fn ids(&self) -> String {
        let mut ids: Vec<String> = Vec::with_capacity(self.members().len());
        let mut seen: HashSet<String> = HashSet::new();
        for m in self.members() {
            let raw = m.id.as_deref().unwrap_or_default();
            let id = hex::encode(raw);
            if raw.len() == PUBLIC_KEY_SIZE && !seen.contains(&id) {
                seen.insert(id.clone());
                ids.push(id);
            }
        }
        ids.join(",")
    }

    /// The members; None → &[].
    pub fn members(&self) -> &[Member] {
        self.members.as_deref().unwrap_or_default()
    }
}

/// `Ticket.String` = `encode()`.
impl std::fmt::Display for Ticket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.encode())
    }
}

/// go-iroh key parsing errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeyError {
    #[error("failed to decode hex string")]
    Hex,
    #[error("failed to decode base32 string")]
    Base32,
    #[error("invalid length")]
    Length,
    #[error("data is not a valid public key")]
    KeyData,
    /// Go wraps with `%w`, so the inner error is also the `source()` (PORTING §3.4, §5.2).
    #[error("{0}: input is z-base-32, use ParseEndpointIDZ32")]
    Z32(#[source] Box<KeyError>),
}

/// `ticket.Parse` errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TicketError {
    #[error("ticket: empty")]
    Empty,
    #[error("ticket: {0}")]
    Base32(#[source] dstore_gocompat::base32::CorruptInputError),
    #[error("ticket: {0}")]
    Cbor(#[source] dstore_codec::DecodeError),
    #[error("ticket: no members")]
    NoMembers,
    #[error("ticket: {} is neither a dstore1 ticket nor a node id: {source}", dstore_gocompat::quote::quote(.field))]
    NotId {
        field: Vec<u8>,
        #[source]
        source: KeyError,
    },
}

/// Go `ticket.Parse`, validation order codec-wire-ticket §2.4.4: trim, empty, prefix test on the
/// lower-cased string, base32 of the upper-cased body, CBOR, members.
pub fn parse(s: &[u8]) -> Result<Ticket, TicketError> {
    let s = strings::trim_space(s);
    if s.is_empty() {
        return Err(TicketError::Empty);
    }
    if !strings::to_lower(s).starts_with(PREFIX.as_bytes()) {
        return parse_ids(s);
    }
    // Go slices the original string: s[len(Prefix):]. Only ASCII runes lower-case to the prefix's
    // bytes, so the original starts with 7 ASCII bytes and `get` never misses.
    let body = s.get(PREFIX.len()..).unwrap_or_default();
    let b = base32::decode_nopad(base32::STD_ALPHABET, &strings::to_upper(body))
        .map_err(TicketError::Base32)?;
    let t: Ticket = dstore_codec::unmarshal(&b).map_err(TicketError::Cbor)?;
    if t.members().is_empty() {
        return Err(TicketError::NoMembers);
    }
    Ok(t)
}

/// `parseIDs`: fields separated by `,`, space, `\t`, `\n`, `\r`, each a node id. The error quotes the
/// original field; the id parser gets the lower-cased one.
fn parse_ids(s: &[u8]) -> Result<Ticket, TicketError> {
    let mut members = Vec::new();
    for f in strings::fields_func(s, |r| matches!(r, ',' | ' ' | '\t' | '\n' | '\r')) {
        let id = parse_endpoint_id(&strings::to_lower(f)).map_err(|source| TicketError::NotId {
            field: f.to_vec(),
            source,
        })?;
        members.push(Member {
            id: Some(id.to_vec()),
            addrs: Vec::new(),
        });
    }
    if members.is_empty() {
        return Err(TicketError::NoMembers);
    }
    Ok(Ticket {
        cluster_id: None,
        incarnation: 0,
        members: Some(members),
    })
}

/// go-iroh `ParseEndpointID` on the given bytes: `ParsePublicKey`, and on failure the z-base-32 hint
/// when the same bytes decode to 32 bytes under the z-base-32 alphabet.
pub fn parse_endpoint_id(s: &[u8]) -> Result<[u8; 32], KeyError> {
    parse_public_key(s).map_err(
        |err| match base32::decode_nopad(base32::ZBASE32_ALPHABET, s) {
            Ok(b) if b.len() == PUBLIC_KEY_SIZE => KeyError::Z32(Box::new(err)),
            _ => err,
        },
    )
}

/// go-iroh `ParsePublicKey`: `decodeBase32OrHex`, then `NewPublicKey`.
fn parse_public_key(s: &[u8]) -> Result<[u8; 32], KeyError> {
    let b = decode_base32_or_hex(s)?;
    if !is_valid_public_key(&b) {
        return Err(KeyError::KeyData);
    }
    Ok(b)
}

/// go-iroh `decodeBase32OrHex`: 64 bytes are hex (`hex.DecodeString`, either case), anything else is
/// RFC 4648 base32 of the upper-cased string with Go's decoder; the result must be 32 bytes.
fn decode_base32_or_hex(s: &[u8]) -> Result<[u8; 32], KeyError> {
    if s.len() == PUBLIC_KEY_SIZE * 2 {
        let b = hex::decode_string(s).map_err(|_| KeyError::Hex)?;
        return Ok(copy32(&b));
    }
    let b = base32::decode_nopad(base32::STD_ALPHABET, &strings::to_upper(s))
        .map_err(|_| KeyError::Base32)?;
    if b.len() != PUBLIC_KEY_SIZE {
        return Err(KeyError::Length);
    }
    Ok(copy32(&b))
}

/// Go `copy(out[:], b)` into a 32-byte array.
fn copy32(b: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    for (o, v) in out.iter_mut().zip(b) {
        *o = *v;
    }
    out
}

/// go-iroh `NewPublicKey`'s curve-point check (filippo `edwards25519.Point.SetBytes`): y is read with
/// bit 255 masked and not reduced, the point is accepted when `(y² - 1) / (d·y² + 1)` is a square, and
/// the sign bit never rejects. `iroh_base::PublicKey::from_bytes` (ed25519-dalek `VerifyingKey::from_bytes`
/// → curve25519-dalek `CompressedEdwardsY::decompress`) follows the same rules; `ticket/curve.json`
/// confirms the accept set.
pub fn is_valid_public_key(b: &[u8; 32]) -> bool {
    iroh_base::PublicKey::from_bytes(b).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use dstore_codec::{marshal, unmarshal};

    fn unhex(s: &str) -> Vec<u8> {
        hex::decode_string(s.as_bytes()).expect("test hex")
    }

    fn arr(s: &str) -> [u8; 32] {
        <[u8; 32]>::try_from(unhex(s).as_slice()).expect("32 bytes")
    }

    /// A valid ed25519 public key, deterministic per seed.
    fn key(seed: u8) -> [u8; 32] {
        *iroh_base::SecretKey::from_bytes(&[seed; 32])
            .public()
            .as_bytes()
    }

    fn b32_lower(b: &[u8]) -> String {
        base32::encode_nopad(base32::STD_ALPHABET, b).to_ascii_lowercase()
    }

    fn err_text(s: &[u8]) -> String {
        match parse(s) {
            Ok(t) => panic!("parse({:?}) succeeded: {t:?}", String::from_utf8_lossy(s)),
            Err(e) => e.to_string(),
        }
    }

    /// The `TestRoundTrip` ticket.
    fn probe() -> Ticket {
        let mut id = vec![0u8; 32];
        id[0] = 7;
        Ticket {
            cluster_id: Some(b"0123456789abcdef".to_vec()),
            incarnation: 3,
            members: Some(vec![Member {
                id: Some(id),
                addrs: vec![
                    "ip:127.0.0.1:4433".to_string(),
                    "relay:https://relay.example/".to_string(),
                ],
            }]),
        }
    }

    /// Go's text (`ticket/encode.json` case `probe`). codec-wire-ticket §2.4.2 prints it with one `a` too
    /// many (183 characters for a stated 182).
    const PROBE_TEXT: &str = "dstore1umafambrgiztinjwg44dsylcmnsgkzqbambidiqalaqaoaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaabqjyws4b2gezdolrqfyyc4mj2gq2dgm3ydrzgk3dbpe5gq5duobztulzpojswyylzfzsxqylnobwgkly";

    // dstore ticket/ticket_test.go TestRoundTrip.
    #[test]
    fn round_trip() {
        let input = probe();
        let s = input.encode();
        assert!(s.starts_with(PREFIX), "prefix: {s}");
        let out = parse(s.as_bytes()).expect("parse");
        assert_eq!(out.incarnation, 3);
        assert_eq!(out.cluster_id.as_deref(), Some(&b"0123456789abcdef"[..]));
        assert_eq!(out.members().len(), 1);
        assert_eq!(out.members()[0].addrs[1], "relay:https://relay.example/");
        assert!(parse(b"nope").is_err(), "expected a prefix error");
        parse(s.to_ascii_uppercase().as_bytes()).expect("case-insensitive parse");
    }

    // dstore ticket/ticket_test.go TestParseIDs. Go draws two random keys per run; the port runs the same
    // body over 16 deterministic key pairs.
    #[test]
    fn parse_id_lists() {
        for seed in (1u8..=31).step_by(2) {
            let (id1, id2) = (key(seed), key(seed + 1));
            parse_id_lists_with(&id1, &id2);
        }
    }

    fn parse_id_lists_with(id1: &[u8; 32], id2: &[u8; 32]) {
        let (hex1, hex2) = (hex::encode(id1), hex::encode(id2));
        let b32 = b32_lower(id2);
        let inputs = [
            hex1.clone(),
            format!("{hex1},{hex2}"),
            format!("{hex1} {hex2}"),
            format!(" {hex1},\n{b32} "),
            hex1.to_ascii_uppercase(),
        ];
        for s in &inputs {
            let tk = parse(s.as_bytes()).unwrap_or_else(|e| panic!("Parse({s:?}): {e}"));
            assert!(
                tk.cluster_id.is_none() && tk.incarnation == 0,
                "Parse({s:?}): cluster fields set: {tk:?}"
            );
            assert_eq!(
                tk.members()[0].id.as_deref(),
                Some(&id1[..]),
                "Parse({s:?})"
            );
            assert!(tk.members()[0].addrs.is_empty(), "Parse({s:?})");
            if s.contains(&hex2) || s.contains(&b32) {
                assert_eq!(tk.members().len(), 2, "Parse({s:?})");
                assert_eq!(
                    tk.members()[1].id.as_deref(),
                    Some(&id2[..]),
                    "Parse({s:?})"
                );
            } else {
                assert_eq!(tk.members().len(), 1, "Parse({s:?})");
            }
        }
        let bad = [
            String::new(),
            "  ".to_string(),
            hex1[..63].to_string(),
            format!("{hex1},zz"),
            "ab".repeat(32),
            "dstore1".to_string(),
            "nope".to_string(),
        ];
        for s in &bad {
            assert!(parse(s.as_bytes()).is_err(), "Parse({s:?}) succeeded");
        }
        let tk = parse(format!("{hex1},{hex2},{hex1}").as_bytes()).expect("parse");
        assert_eq!(tk.members().len(), 3, "duplicates are kept");
        assert_eq!(tk.ids(), format!("{hex1},{hex2}"));
        parse(tk.ids().as_bytes()).expect("IDs() parses back");
    }

    // codec-wire-ticket §2.4.1 probe encodings, and nil vs empty through a decode.
    #[test]
    fn probe_encodings() {
        assert_eq!(hex::encode(&marshal(&Ticket::default())), "a300f6010002f6");
        let nil_id = Ticket {
            members: Some(vec![Member::default()]),
            ..Ticket::default()
        };
        assert_eq!(hex::encode(&marshal(&nil_id)), "a300f601000281a100f6");
        let empty = Ticket {
            cluster_id: Some(vec![]),
            members: Some(vec![]),
            ..Ticket::default()
        };
        assert_eq!(hex::encode(&marshal(&empty)), "a3004001000280");
        for probe in ["a3004001000281a1004140", "a300f601000281a100f6"] {
            let t: Ticket = unmarshal(&unhex(probe)).expect("decode");
            assert_eq!(hex::encode(&marshal(&t)), probe);
        }
    }

    // codec-wire-ticket §2.4.2: the TestRoundTrip ticket's CBOR and text.
    #[test]
    fn probe_text() {
        let t = probe();
        let cbor = format!(
            "a30050{}01030281a2005820{}{}01827169703a3132372e302e302e313a34343333781c72656c61793a68747470733a2f2f72656c61792e6578616d706c652f",
            hex::encode(b"0123456789abcdef"),
            "07",
            "00".repeat(31)
        );
        assert_eq!(hex::encode(&marshal(&t)), cbor);
        assert_eq!(t.encode(), PROBE_TEXT);
        assert_eq!(t.encode().len(), 182);
        assert_eq!(t.to_string(), PROBE_TEXT);
        assert_eq!(parse(PROBE_TEXT.as_bytes()), Ok(t));
    }

    // Encoding is canonical: data-encoding's BASE32_NOPAD gives the same text.
    #[test]
    fn encode_matches_data_encoding() {
        let t = probe();
        let want = format!(
            "{PREFIX}{}",
            data_encoding::BASE32_NOPAD
                .encode(&marshal(&t))
                .to_ascii_lowercase()
        );
        assert_eq!(t.encode(), want);
    }

    // codec-wire-ticket §2.4.3.
    #[test]
    fn ids_keep_first_32_byte_occurrence() {
        let m = |id: Option<Vec<u8>>| Member {
            id,
            addrs: Vec::new(),
        };
        let t = Ticket {
            members: Some(vec![
                m(Some(vec![1; 32])),
                m(Some(vec![0xab; 32])),
                m(Some(vec![1; 32])),
                m(Some(vec![1, 2])),
                m(Some(vec![9; 31])),
                m(None),
            ]),
            ..Ticket::default()
        };
        assert_eq!(t.ids(), format!("{},{}", "01".repeat(32), "ab".repeat(32)));
        assert_eq!(Ticket::default().ids(), "");
        assert!(Ticket::default().members().is_empty());
        let short = Ticket {
            members: Some(vec![m(Some(vec![9; 31])), m(Some(vec![9; 33]))]),
            ..Ticket::default()
        };
        assert_eq!(short.ids(), "");
    }

    // codec-wire-ticket §2.4.9, verbatim.
    #[test]
    fn parse_outcomes() {
        let hex1 = hex::encode(&key(1));
        let hex2 = hex::encode(&key(2));
        assert_eq!(err_text(b""), "ticket: empty");
        assert_eq!(err_text(b"  "), "ticket: empty");
        assert_eq!(err_text(b"dstore1"), "ticket: EOF");
        assert_eq!(
            err_text(b"nope"),
            "ticket: \"nope\" is neither a dstore1 ticket nor a node id: invalid length"
        );
        assert_eq!(err_text(b" , ,"), "ticket: no members");
        assert_eq!(
            err_text(&hex1.as_bytes()[..63]),
            format!(
                "ticket: \"{}\" is neither a dstore1 ticket nor a node id: failed to decode base32 string",
                &hex1[..63]
            )
        );
        let ab = "ab".repeat(32);
        assert_eq!(
            err_text(ab.as_bytes()),
            format!(
                "ticket: \"{ab}\" is neither a dstore1 ticket nor a node id: data is not a valid public key"
            )
        );
        assert_eq!(
            err_text(format!("{hex1},zz").as_bytes()),
            "ticket: \"zz\" is neither a dstore1 ticket nor a node id: invalid length"
        );
        assert_eq!(
            err_text("zz\x01\"é".as_bytes()),
            "ticket: \"zz\\x01\\\"é\" is neither a dstore1 ticket nor a node id: failed to decode base32 string"
        );
        let z32 = "8pinxxgqs41n4aididenw5apqp1urfmzdztr8jt4abrkdn435ewo";
        assert_eq!(
            err_text(z32.as_bytes()),
            format!(
                "ticket: \"{z32}\" is neither a dstore1 ticket nor a node id: failed to decode base32 string: input is z-base-32, use ParseEndpointIDZ32"
            )
        );
        assert_eq!(
            err_text(format!("{hex1}\u{0b}{hex2}").as_bytes()),
            format!(
                "ticket: \"{hex1}\\v{hex2}\" is neither a dstore1 ticket nor a node id: failed to decode base32 string"
            )
        );
        for (extra, n) in [("a", 1), ("aaa", 2), ("aaaaaa", 4), ("aaaaaaaa", 5)] {
            assert_eq!(
                err_text(format!("{PROBE_TEXT}{extra}").as_bytes()),
                format!("ticket: cbor: {n} bytes of extraneous data starting at index 109")
            );
        }
        let body = &PROBE_TEXT[PREFIX.len()..];
        assert_eq!(
            err_text(format!("{PREFIX}{}!{}", &body[..3], &body[4..]).as_bytes()),
            "ticket: illegal base32 data at input byte 3"
        );
        assert_eq!(
            err_text(format!("{PREFIX}{}8{}", &body[..8], &body[9..]).as_bytes()),
            "ticket: illegal base32 data at input byte 8"
        );
        assert_eq!(
            err_text(b"dstore1!!!"),
            "ticket: illegal base32 data at input byte 0"
        );
    }

    // codec-wire-ticket §2.4.9: the accepted ticket variants.
    #[test]
    fn parse_accepts_ticket_variants() {
        let want = probe();
        let body = &PROBE_TEXT[PREFIX.len()..];
        let mixed: String = body
            .chars()
            .enumerate()
            .map(|(i, c)| {
                if i % 2 == 0 {
                    c.to_ascii_uppercase()
                } else {
                    c
                }
            })
            .collect();
        let i_at = body.find('i').expect("an i in the body");
        let last = &PROBE_TEXT[..PROBE_TEXT.len() - 1];
        let variants = [
            PROBE_TEXT.to_ascii_uppercase(),
            format!("DsToRe1{mixed}"),
            format!("{PREFIX}{}\n\r{}", &body[..10], &body[10..]),
            format!("{last}7"),
            format!("{PREFIX}{}\u{131}{}", &body[..i_at], &body[i_at + 1..]),
            format!("\u{a0}{PROBE_TEXT}\u{2003}"),
        ];
        for v in &variants {
            let t = parse(v.as_bytes()).unwrap_or_else(|e| panic!("parse({v:?}): {e}"));
            assert_eq!(t, want, "parse({v:?})");
            assert_eq!(t.encode(), PROBE_TEXT, "parse({v:?})");
        }
        // U+0130 and U+212A stay non-ASCII under ToUpper: not base32 symbols in a body.
        let k_at = body.find('k').expect("a k in the body");
        assert_eq!(
            err_text(format!("{PREFIX}{}\u{130}{}", &body[..i_at], &body[i_at + 1..]).as_bytes()),
            format!("ticket: illegal base32 data at input byte {i_at}")
        );
        assert_eq!(
            err_text(format!("{PREFIX}{}\u{212a}{}", &body[..k_at], &body[k_at + 1..]).as_bytes()),
            format!("ticket: illegal base32 data at input byte {k_at}")
        );
    }

    // codec-wire-ticket §2.4.6: lookalike runes in a base32 id, trailing bits, lengths, hex routing.
    #[test]
    fn endpoint_id_quirks() {
        let (seed, b32) = (1u8..=255)
            .map(|s| (s, b32_lower(&key(s))))
            .find(|(_, b)| b.contains('i') && b.contains('s') && b.contains('k'))
            .expect("a base32 id with i, s and k");
        let id = key(seed);
        let repl = |s: &str, old: char, new: char| s.replacen(old, &new.to_string(), 1);
        for look in [
            repl(&b32, 'i', '\u{130}'),
            repl(&b32, 'i', '\u{131}'),
            repl(&b32, 's', '\u{17f}'),
            repl(&b32, 'k', '\u{212a}'),
            repl(
                &repl(&repl(&b32, 'i', '\u{130}'), 's', '\u{17f}'),
                'k',
                '\u{212a}',
            ),
        ] {
            let t = parse(look.as_bytes()).unwrap_or_else(|e| panic!("parse({look:?}): {e}"));
            assert_eq!(t.ids(), hex::encode(&id), "parse({look:?})");
        }
        // The last of 52 symbols carries 1 data bit and 4 trailing bits: the 16 symbols with the same top
        // bit give the same id, the other 16 flip the id's last bit.
        let stem = &b32[..51];
        let top = base32::STD_ALPHABET
            .iter()
            .position(|&c| c == b32.as_bytes()[51].to_ascii_uppercase())
            .expect("a base32 symbol")
            & 0b10000;
        for sym in &base32::STD_ALPHABET[top..top + 16] {
            let v = format!("{stem}{}", char::from(*sym).to_ascii_lowercase());
            assert_eq!(parse_endpoint_id(v.as_bytes()), Ok(id), "{v}");
        }
        let other = char::from(base32::STD_ALPHABET[top ^ 0b10000]).to_ascii_lowercase();
        let flipped = parse_endpoint_id(format!("{stem}{other}").as_bytes());
        assert!(
            flipped.map_or(true, |f| f[31] == id[31] ^ 1),
            "{stem}{other}"
        );
        for n in [48usize, 51, 53, 54, 60] {
            let v: String = b32.chars().chain(std::iter::repeat('a')).take(n).collect();
            assert_eq!(
                parse_endpoint_id(v.as_bytes()),
                Err(KeyError::Length),
                "{n}"
            );
        }
        // Hex routing uses the lowered length: U+0130 lower-cases to one byte.
        let hex1 = hex::encode(&id);
        let routed = format!("\u{130}{}", &hex1[..62]);
        assert_eq!(routed.len(), 64);
        assert_eq!(
            err_text(routed.as_bytes()),
            format!(
                "ticket: \"{routed}\" is neither a dstore1 ticket nor a node id: failed to decode base32 string"
            )
        );
        // parse_endpoint_id itself takes the bytes as given: hex in either case.
        assert_eq!(
            parse_endpoint_id(hex1.to_ascii_uppercase().as_bytes()),
            Ok(id)
        );
        let bad_hex = format!("{}zz", &hex1[..62]);
        assert_eq!(parse_endpoint_id(bad_hex.as_bytes()), Err(KeyError::Hex));
    }

    // go-iroh key/key_test.go TestPublicKeyFromStringHex.
    #[test]
    fn public_key_from_string_hex() {
        const S: &str = "ae58ff8833241ac82d6ff7611046ed67b5072d142c588d0063e942d9a75502b6";
        let k = parse_endpoint_id(S.as_bytes()).expect("ParsePublicKey");
        assert_eq!(hex::encode(&k), S);
        assert_eq!(k, arr(S));
    }

    // go-iroh key/key_test.go TestPublicKeyAllZeroIsValid.
    #[test]
    fn public_key_all_zero_is_valid() {
        assert!(is_valid_public_key(&[0; 32]));
    }

    // go-iroh key/key_test.go TestParseEndpointIDRejectsGarbage.
    #[test]
    fn parse_endpoint_id_rejects_garbage() {
        assert!(parse_endpoint_id(b"foobarbaz").is_err());
    }

    // go-iroh key/key_test.go TestPublicKeyInvalidCurvePoint.
    #[test]
    fn public_key_invalid_curve_point() {
        let mut b = [0u8; 32];
        b[0] = 2;
        assert!(!is_valid_public_key(&b));
        assert_eq!(
            parse_endpoint_id(hex::encode(&b).as_bytes()),
            Err(KeyError::KeyData)
        );
    }

    // go-iroh key/key_test.go TestParseEndpointIDRejectsZ32.
    #[test]
    fn parse_endpoint_id_rejects_z32() {
        const REJECTED: &str = "8pinxxgqs41n4aididenw5apqp1urfmzdztr8jt4abrkdn435ewo";
        let want = base32::decode_nopad(base32::ZBASE32_ALPHABET, REJECTED.as_bytes())
            .expect("ParseEndpointIDZ32");
        let want = <[u8; 32]>::try_from(want.as_slice()).expect("32 bytes");
        assert!(is_valid_public_key(&want));
        let err = parse_endpoint_id(REJECTED.as_bytes()).expect_err("z32 must not parse");
        assert_eq!(err, KeyError::Z32(Box::new(KeyError::Base32)));
        assert_eq!(
            err.to_string(),
            "failed to decode base32 string: input is z-base-32, use ParseEndpointIDZ32"
        );
        assert_eq!(parse_endpoint_id(hex::encode(&want).as_bytes()), Ok(want));

        const AMBIGUOUS: &str = "bf4nc56yxomts6n7whnrmotaqs7pgro36xdgeenscw4fdd3gexgy";
        let z32 = base32::decode_nopad(base32::ZBASE32_ALPHABET, AMBIGUOUS.as_bytes())
            .expect("ParseEndpointIDZ32");
        let z32 = <[u8; 32]>::try_from(z32.as_slice()).expect("32 bytes");
        assert!(is_valid_public_key(&z32));
        let rfc = parse_endpoint_id(AMBIGUOUS.as_bytes()).expect("ParseEndpointID");
        assert_ne!(rfc, z32, "fixture is no longer ambiguous");
    }

    // go-iroh key/key_test.go TestEndpointIDEncoding (the text and z-base-32 round trips; the port has no
    // binary marshalling). `ParseEndpointIDZ32` is decodeZBase32, then EndpointIDFromSlice: length 32 and
    // the curve check.
    #[test]
    fn endpoint_id_encoding() {
        for seed in 1..=8 {
            let id = key(seed);
            assert_eq!(parse_endpoint_id(hex::encode(&id).as_bytes()), Ok(id));
            let z = base32::encode_nopad(base32::ZBASE32_ALPHABET, &id);
            let back = base32::decode_nopad(base32::ZBASE32_ALPHABET, z.as_bytes()).expect("z32");
            let back = <[u8; 32]>::try_from(back.as_slice()).expect("32 bytes");
            assert!(is_valid_public_key(&back), "{z}");
            assert_eq!(back, id);
        }
    }

    // go-iroh key/key_test.go TestParseBase32UpperAndLower.
    #[test]
    fn parse_base32_upper_and_lower() {
        let id = key(3);
        let upper = base32::encode_nopad(base32::STD_ALPHABET, &id);
        assert_ne!(upper.len(), PUBLIC_KEY_SIZE * 2);
        assert_eq!(parse_endpoint_id(upper.as_bytes()), Ok(id));
        assert_eq!(
            parse_endpoint_id(upper.to_ascii_lowercase().as_bytes()),
            Ok(id)
        );
    }

    // codec-wire-ticket §5 G11 and the L1 note: non-canonical y ≥ p when y mod p decodes, x = 0 with the
    // sign bit set, and all-0xff are valid in Go (rows of ticket/curve.json).
    #[test]
    fn curve_acceptance_rows() {
        let p_plus = |add: u8, sign: bool| {
            let mut b = [0xffu8; 32];
            b[0] = 0xed + add;
            b[31] = if sign { 0xff } else { 0x7f };
            b
        };
        let mut y1_sign = [0u8; 32];
        y1_sign[0] = 1;
        y1_sign[31] = 0x80;
        let mut y0_sign = [0u8; 32];
        y0_sign[31] = 0x80;
        let rows: [([u8; 32], bool); 11] = [
            ([0; 32], true),
            ([0xff; 32], true),
            (y1_sign, true),
            (y0_sign, true),
            (p_plus(0, false), true),
            (p_plus(0, true), true),
            (p_plus(1, true), true),
            (p_plus(2, false), false),
            (p_plus(18, true), true),
            ([0x07; 32], false),
            ([0xab; 32], false),
        ];
        for (b, valid) in rows {
            assert_eq!(is_valid_public_key(&b), valid, "{}", hex::encode(&b));
        }
    }

    #[test]
    fn error_sources() {
        use std::error::Error as _;
        let e = parse(b"nope").expect_err("nope");
        let src = e.source().expect("a source");
        assert_eq!(src.to_string(), "invalid length");
        let e = parse(b"dstore1!").expect_err("bang");
        assert_eq!(
            e.source().map(|s| s.to_string()).as_deref(),
            Some("illegal base32 data at input byte 0")
        );
        let e = parse(b"dstore1").expect_err("empty body");
        assert_eq!(e.source().map(|s| s.to_string()).as_deref(), Some("EOF"));
        // go-iroh wraps the z-base-32 hint with %w: errors.Is(err, ErrDecodeBase32) holds.
        let z32 = b"8pinxxgqs41n4aididenw5apqp1urfmzdztr8jt4abrkdn435ewo";
        let e = parse(z32).expect_err("z32");
        let hint = e.source().expect("the key error");
        assert_eq!(
            hint.to_string(),
            "failed to decode base32 string: input is z-base-32, use ParseEndpointIDZ32"
        );
        // thiserror returns the boxed field itself as the source: downcast through `Box<KeyError>`.
        let inner = hint.source().expect("the wrapped key error");
        assert_eq!(
            inner.downcast_ref::<Box<KeyError>>().map(|b| &**b),
            Some(&KeyError::Base32),
            "{inner}"
        );
        assert!(inner.source().is_none());
    }

    // codec-wire-ticket §2.4.4 step 3 and §2.4.8, which `parse` relies on to slice the original bytes at
    // `PREFIX.len()` and which make the 64-byte hex routing see only ASCII hex digits: under Go simple case
    // mapping, the only non-ASCII runes that lower-case to ASCII are U+0130 and U+212A, and the only ones
    // that upper-case to ASCII are U+0131 and U+017F.
    #[test]
    fn non_ascii_runes_mapping_to_ascii() {
        let (mut lower, mut upper) = (Vec::new(), Vec::new());
        let mut buf = [0u8; 4];
        for c in (0x80u32..=0x10ffff).filter_map(char::from_u32) {
            let enc = c.encode_utf8(&mut buf).as_bytes();
            let l = strings::to_lower(enc);
            if l.is_ascii() {
                lower.push((c, l));
            }
            let u = strings::to_upper(enc);
            if u.is_ascii() {
                upper.push((c, u));
            }
        }
        assert_eq!(
            lower,
            [('\u{130}', b"i".to_vec()), ('\u{212a}', b"k".to_vec())]
        );
        assert_eq!(
            upper,
            [('\u{131}', b"I".to_vec()), ('\u{17f}', b"S".to_vec())]
        );
    }
}
