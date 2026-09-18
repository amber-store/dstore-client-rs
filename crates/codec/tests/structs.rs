//! Behaviour of `cbor_struct!` structs through `marshal` and `unmarshal`.
//!
//! These port the core-rs `src/reference.rs` decoder tests named by codec-wire-ticket §6, generalised to
//! any struct (indefinite lengths accepted, fxamacker texts instead of core-rs variants), plus the
//! fxamacker probes of codec-wire-ticket §2.1.2 with dstore's own struct names. The same code paths are
//! pinned against Go by `codec/decode.json` (root `tests/golden.rs`).

use dstore_codec::{DecodeError, cbor_struct, marshal, unmarshal, well_formed};

fn hex(s: &str) -> Vec<u8> {
    let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(s.len().is_multiple_of(2), "odd hex {s}");
    (0..s.len())
        .step_by(2)
        .map(|i| match u8::from_str_radix(&s[i..i + 2], 16) {
            Ok(b) => b,
            Err(e) => panic!("bad hex {s}: {e}"),
        })
        .collect()
}

fn to_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

cbor_struct! {
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct RefInfo = "wire.RefInfo" {
        0 => name: String = "string",
        1 => key: Option<Vec<u8>> = "[]uint8",
        2 => version: Option<Vec<u8>> = "[]uint8",
        3 => created_at: i64 = "int64",
        4 => user: String = "string" [omitempty],
    }
}

cbor_struct! {
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct KeyFailure = "wire.KeyFailure" {
        0 => key: Option<Vec<u8>> = "[]uint8",
        1 => node: Option<Vec<u8>> = "[]uint8",
        2 => reason: String = "string",
        3 => retry_after: i64 = "int64" [omitempty],
    }
}

cbor_struct! {
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct Msg = "wire.Msg" {
        0 => typ: i64 = "int",
        1 => cluster_id: Vec<u8> = "[]uint8" [omitempty],
        2 => incarnation: u64 = "uint64" [omitempty],
        4 => keys: Vec<Vec<u8>> = "[][]uint8" [omitempty],
        6 => name: String = "string" [omitempty],
        7 => record: Vec<u8> = "[]uint8" [omitempty],
        21 => refs: Vec<RefInfo> = "[]wire.RefInfo" [omitempty],
        24 => failed: Vec<KeyFailure> = "[]wire.KeyFailure" [omitempty],
        43 => weight: u32 = "uint32" [omitempty],
    }
}

cbor_struct! {
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct Member = "ticket.Member" {
        0 => id: Option<Vec<u8>> = "[]uint8",
        1 => addrs: Vec<String> = "[]string" [omitempty],
    }
}

cbor_struct! {
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct Ticket = "ticket.Ticket" {
        0 => cluster_id: Option<Vec<u8>> = "[]uint8",
        1 => incarnation: u64 = "uint64",
        2 => members: Option<Vec<Member>> = "[]ticket.Member",
    }
}

cbor_struct! {
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct Pending = "view.Pending" {
        1 => replicas: u8 = "uint8",
    }
}

cbor_struct! {
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct View = "view.View" {
        11 => pending: Option<Box<Pending>> = "view.Pending" [omitempty],
        13 => fenced: Vec<Option<Vec<u8>>> = "[][]uint8" [omitempty],
    }
}

fn msg(input: &str) -> Result<Msg, DecodeError> {
    unmarshal(&hex(input))
}

fn msg_err(input: &str) -> String {
    match msg(input) {
        Ok(m) => panic!("input {input}: decoded {m:?}, want an error"),
        Err(e) => e.to_string(),
    }
}

fn msg_ok(input: &str) -> Msg {
    match msg(input) {
        Ok(m) => m,
        Err(e) => panic!("input {input}: {e}"),
    }
}

#[test]
fn probe_decoded_values() {
    // Other tags are unwrapped, simple values fill integers, first duplicate wins.
    assert_eq!(msg_ok("a100d82a1820").typ, 32);
    assert_eq!(msg_ok("d9d9f7a1001820").typ, 32);
    assert_eq!(msg_ok("a1048 1d82a4101").keys, vec![vec![0x01]]);
    assert_eq!(msg_ok("a1075f41614162ff").record, b"ab");
    assert!(msg_ok("a10480").keys.is_empty());
    assert_eq!(msg_ok("a10781f6").record, [0x00]);
    assert_eq!(msg_ok("a100f0").typ, 16);
    assert_eq!(msg_ok("a2001820006161").typ, 32);
    // Skipped values are never examined.
    assert_eq!(msg_ok("a1186361ff"), Msg::default());
    // A top-level null or undefined is the zero value, tagged or not.
    for input in ["f6", "f7", "c6f6", "d9d9f7f7"] {
        assert_eq!(msg_ok(input), Msg::default(), "input {input}");
    }
}

#[test]
fn probe_error_texts() {
    let cases: &[(&str, &str)] = &[
        (
            "a1182b1b0000000100000000",
            "cbor: cannot unmarshal positive integer into Go struct field wire.Msg.43 of type uint32 (4294967296 overflows uint32)",
        ),
        (
            "a1003bffffffffffffffff",
            "cbor: cannot unmarshal negative integer into Go struct field wire.Msg.0 of type int (-18446744073709551616 overflows Go's int64)",
        ),
        (
            "a10220",
            "cbor: cannot unmarshal negative integer into Go struct field wire.Msg.2 of type uint64",
        ),
        (
            "a1064161",
            "cbor: cannot unmarshal byte string into Go struct field wire.Msg.6 of type string",
        ),
        (
            "a1076161",
            "cbor: cannot unmarshal UTF-8 text string into Go struct field wire.Msg.7 of type []uint8",
        ),
        (
            "80",
            "cbor: cannot unmarshal array into Go value of type wire.Msg (cannot decode CBOR array to struct without toarray option)",
        ),
        (
            "a10680",
            "cbor: cannot unmarshal array into Go struct field wire.Msg.6 of type string",
        ),
        (
            "a106a0",
            "cbor: cannot unmarshal map into Go struct field wire.Msg.6 of type string",
        ),
        (
            "a1410001",
            "cbor: cannot unmarshal byte string into Go value of type string (map key is of type byte string and cannot be used to match struct field name)",
        ),
        (
            "a11bffffffffffffffff01",
            "cbor: cannot unmarshal positive integer into Go value of type int64 (18446744073709551615 overflows Go's int64)",
        ),
        (
            "a13bffffffffffffffff01",
            "cbor: cannot unmarshal negative integer into Go value of type int64 (-1-18446744073709551615 overflows Go's int64)",
        ),
        (
            "a102c249010000000000000000",
            "cbor: cannot unmarshal tag into Go struct field wire.Msg.2 of type uint64 (18446744073709551616 overflows uint64)",
        ),
        (
            "a106c24105",
            "cbor: cannot unmarshal tag into Go struct field wire.Msg.6 of type string",
        ),
        (
            "05",
            "cbor: cannot unmarshal positive integer into Go value of type wire.Msg",
        ),
        // The outermost struct field is named.
        (
            "a11581a1004161",
            "cbor: cannot unmarshal byte string into Go struct field wire.Msg.21 of type string",
        ),
        (
            "a1181881a1036161",
            "cbor: cannot unmarshal UTF-8 text string into Go struct field wire.Msg.24 of type int64",
        ),
        // Map-key errors inside a struct field are rewritten too.
        (
            "a11581a1410001",
            "cbor: cannot unmarshal byte string into Go struct field wire.Msg.21 of type string (map key is of type byte string and cannot be used to match struct field name)",
        ),
        ("a161ff00", "cbor: invalid UTF-8 string"),
        (
            "a1049a00020001",
            "cbor: exceeded max number of elements 131072 for CBOR array",
        ),
        (
            "ba00020001",
            "cbor: exceeded max number of key-value pairs 131072 for CBOR map",
        ),
        (
            "a100c005",
            "cbor: tag number 0 must be followed by text string, got positive integer",
        ),
        (
            "a100c1a0",
            "cbor: tag number 1 must be followed by integer or floating-point number, got map",
        ),
        (
            "a107c3c64100",
            "cbor: tag number 2 or 3 must be followed by byte string, got tag",
        ),
    ];
    for &(input, want) in cases {
        assert_eq!(msg_err(input), want, "input {input}");
    }
    let ticket: Result<Ticket, _> = unmarshal(&hex("a10281a1006161"));
    assert_eq!(
        ticket.map_err(|e| e.to_string()),
        Err(
            "cbor: cannot unmarshal UTF-8 text string into Go struct field ticket.Ticket.2 of type []uint8"
                .to_owned()
        )
    );
}

// fxamacker DupMapKeyQuiet: the first value wins, and a duplicate's value is skipped without a type
// check.
#[test]
fn decode_duplicate_key_first_wins_and_skips_unchecked() {
    assert_eq!(msg_ok("a206616e0605").name, "n");
    // A container duplicate is skipped wholesale; later keys still decode.
    let m = msg_ok("a306616e06a10102000b");
    assert_eq!((m.name.as_str(), m.typ), ("n", 11));
    // A null first value marks the field as found.
    assert_eq!(msg_ok("a206f606616e").name, "");
    // Duplicates of unknown keys are skipped quietly.
    assert_eq!(msg_ok("a2186300186361ff"), Msg::default());
}

// Integer and text keys that match no field are skipped; other key types are errors.
#[test]
fn decode_map_key_types() {
    assert_eq!(msg_ok("a12001"), Msg::default());
    assert_eq!(
        msg("a1200101"),
        Err(DecodeError::Extraneous { n: 1, index: 3 })
    );
    // Text keys never match keyasint fields, not even "0".
    for input in ["a1617805", "a1613005"] {
        assert_eq!(msg_ok(input), Msg::default(), "input {input}");
    }
    let cases: &[(&str, &str)] = &[
        ("a1440102030401", "byte string"),
        ("a18001", "array"),
        ("a1a001", "map"),
        ("a1c0616101", "tag"),
        ("a1f401", "primitives"),
        ("a1f601", "primitives"),
        ("a1fb3ff000000000000001", "primitives"),
    ];
    for &(input, ty) in cases {
        assert_eq!(
            msg_err(input),
            format!(
                "cbor: cannot unmarshal {ty} into Go value of type string (map key is of type {ty} and cannot be used to match struct field name)"
            ),
            "input {input}"
        );
    }
}

// Pass 1 rejects two-byte simple values below 32 anywhere in the document.
#[test]
fn decode_rejects_low_two_byte_simple_values() {
    assert_eq!(msg("a100f810"), Err(DecodeError::InvalidSimple(16)));
    assert_eq!(msg("f800"), Err(DecodeError::InvalidSimple(0)));
    assert_eq!(msg("f81f"), Err(DecodeError::InvalidSimple(31)));
    assert_eq!(msg_ok("a100f820").typ, 32);
    assert_eq!(msg("a11863f810"), Err(DecodeError::InvalidSimple(16)));
}

// Unassigned simple values fill integer fields as their numeric value; bools and floats do not.
#[test]
fn decode_simple_values_fill_integer_fields() {
    assert_eq!(msg_ok("a100f82a").typ, 42);
    assert_eq!(msg_ok("a100e5").typ, 5);
    assert_eq!(msg_ok("a10bf8ff").typ, 0);
    assert_eq!(
        msg_err("a100f4"),
        "cbor: cannot unmarshal primitives into Go struct field wire.Msg.0 of type int"
    );
    assert_eq!(
        msg_err("a100f93e00"),
        "cbor: cannot unmarshal primitives into Go struct field wire.Msg.0 of type int"
    );
    assert_eq!(
        msg_err("a106f0"),
        "cbor: cannot unmarshal primitives into Go struct field wire.Msg.6 of type string"
    );
    assert_eq!(
        msg_err("a107f0"),
        "cbor: cannot unmarshal primitives into Go struct field wire.Msg.7 of type []uint8"
    );
    assert_eq!(
        msg_err("a1182bf8ff 00".replace(' ', "").as_str()),
        "cbor: 1 bytes of extraneous data starting at index 5"
    );
}

// Integer keys are parsed as int64 before matching; overflow is an error, the boundary is skipped.
#[test]
fn decode_int_key_overflow() {
    assert_eq!(
        msg("a11b800000000000000001"),
        Err(DecodeError::MapKeyOverflow {
            cbor_type: dstore_codec::CborType::PositiveInteger,
            detail: "9223372036854775808".to_owned()
        })
    );
    assert_eq!(
        msg_err("a13b800000000000000001"),
        "cbor: cannot unmarshal negative integer into Go value of type int64 (-1-9223372036854775808 overflows Go's int64)"
    );
    for input in ["a11b7fffffffffffffff01", "a13b7fffffffffffffff01"] {
        assert_eq!(msg_ok(input), Msg::default(), "input {input}");
    }
}

#[test]
fn decode_text_key_utf8_checked() {
    assert_eq!(msg("a162fffe05"), Err(DecodeError::InvalidUtf8));
    assert_eq!(msg("a17f6161ff05"), Ok(Msg::default()));
    assert_eq!(msg("a17f61c361a9ff05"), Err(DecodeError::InvalidUtf8));
}

// Self-described and unknown tags are unwrapped at the top level and around values; built-in tags 0-3
// have their content type checked; bignums follow the big.Int paths.
#[test]
fn decode_tag_handling() {
    assert_eq!(msg_ok("d9d9f7a106616e").name, "n");
    assert_eq!(msg_ok("c4a106616e").name, "n");
    assert_eq!(
        msg_err("c0a106616e"),
        "cbor: tag number 0 must be followed by text string, got map"
    );
    assert_eq!(
        msg_err("c24105"),
        "cbor: cannot unmarshal tag into Go value of type wire.Msg"
    );
    assert_eq!(msg_ok("a106c4616e").name, "n");
    assert_eq!(msg_ok("a106d9d9f7616e").name, "n");
    // Epoch tag 1 and bignum tag 2 into an integer.
    assert_eq!(msg_ok("a100c1182a").typ, 42);
    assert_eq!(msg_ok("a100c2412a").typ, 42);
    // Bignum content bytes fill a []byte.
    assert_eq!(msg_ok("a107c2420102").record, [1, 2]);
    assert_eq!(msg_ok("a107c34105").record, [5]);
    assert_eq!(
        msg_err("a100c205"),
        "cbor: tag number 2 or 3 must be followed by byte string, got positive integer"
    );
    // Overflow boundaries.
    assert_eq!(
        msg_err("a100c2488000000000000000"),
        "cbor: cannot unmarshal tag into Go struct field wire.Msg.0 of type int (9223372036854775808 overflows int)"
    );
    assert_eq!(
        msg_err("a100c3488000000000000000"),
        "cbor: cannot unmarshal tag into Go struct field wire.Msg.0 of type int (-9223372036854775809 overflows int)"
    );
    // Leading zero bytes are insignificant.
    assert_eq!(msg_ok("a100c349007fffffffffffffff").typ, i64::MIN);
    assert_eq!(msg_ok("a100c249000000000000000005").typ, 5);
}

// Arrays fill []byte element-wise; elements follow the uint8 rules.
#[test]
fn decode_arrays_fill_byte_fields() {
    assert_eq!(msg_ok("a1078218ff00").record, [255, 0]);
    assert_eq!(msg_ok("a1079f0102ff").record, [1, 2]);
    assert_eq!(msg_ok("a10782f601").record, [0, 1]);
    assert_eq!(msg_ok("a10781c24105").record, [5]);
    let cases: &[(&str, &str)] = &[
        (
            "a10781 19012c",
            "cbor: cannot unmarshal positive integer into Go struct field wire.Msg.7 of type uint8 (300 overflows uint8)",
        ),
        (
            "a1078120",
            "cbor: cannot unmarshal negative integer into Go struct field wire.Msg.7 of type uint8",
        ),
        (
            "a107816178",
            "cbor: cannot unmarshal UTF-8 text string into Go struct field wire.Msg.7 of type uint8",
        ),
        (
            "a107818101",
            "cbor: cannot unmarshal array into Go struct field wire.Msg.7 of type uint8",
        ),
        (
            "a10781c2420101",
            "cbor: cannot unmarshal tag into Go struct field wire.Msg.7 of type uint8 (257 overflows uint8)",
        ),
        (
            "a10781c34105",
            "cbor: cannot unmarshal tag into Go struct field wire.Msg.7 of type uint8",
        ),
        (
            "a104816161",
            "cbor: cannot unmarshal UTF-8 text string into Go struct field wire.Msg.4 of type []uint8",
        ),
    ];
    for &(input, want) in cases {
        assert_eq!(msg_err(&input.replace(' ', "")), want, "input {input}");
    }
}

// Hostile nesting fails cleanly at fxamacker's boundaries: under the top-level map 31 nested arrays
// pass and 32 fail; 32 nested tags pass and 33 fail (the first tag of a chain adds no level).
#[test]
fn decode_nesting_boundaries() {
    let nested = |item: &str, n: usize| {
        let mut s = String::from("a11863");
        s.push_str(&item.repeat(n));
        s.push_str("01");
        msg(&s)
    };
    assert_eq!(nested("81", 31), Ok(Msg::default()));
    assert_eq!(nested("81", 32), Err(DecodeError::MaxNested));
    assert_eq!(nested("9f", 32), Err(DecodeError::MaxNested));
    assert_eq!(nested("c0", 32), Ok(Msg::default()));
    assert_eq!(nested("c0", 33), Err(DecodeError::MaxNested));
    assert_eq!(nested("c0", 1_000_000), Err(DecodeError::MaxNested));
    assert_eq!(nested("a100", 31), Ok(Msg::default()));
    assert_eq!(nested("a100", 32), Err(DecodeError::MaxNested));
}

// Pass 1 checks the whole document before any field is decoded.
#[test]
fn decode_well_formedness_precedes_field_decoding() {
    let deep = |n: usize| format!("a107{}01", "81".repeat(n));
    assert_eq!(
        msg_err(&deep(31)),
        "cbor: cannot unmarshal array into Go struct field wire.Msg.7 of type uint8"
    );
    assert_eq!(msg(&deep(32)), Err(DecodeError::MaxNested));
    assert_eq!(
        msg("a1000501"),
        Err(DecodeError::Extraneous { n: 1, index: 3 })
    );
    assert_eq!(msg("0505"), Err(DecodeError::Extraneous { n: 1, index: 1 }));
    assert_eq!(msg(""), Err(DecodeError::Eof));
    assert_eq!(msg("a2060007"), Err(DecodeError::UnexpectedEof));
    assert_eq!(well_formed(&hex("a0")), Ok(()));
}

// Inverted from core-rs `decode_rejects_indefinite_length_map`: fxamacker accepts indefinite lengths.
#[test]
fn decode_accepts_indefinite_length_items() {
    assert_eq!(msg_ok("bf06616eff").name, "n");
    assert_eq!(msg_ok("bf067f616e616fffff").name, "no");
    assert_eq!(msg_ok("bf049f4101ffff").keys, vec![vec![1]]);
    assert_eq!(msg("bf06ff"), Err(DecodeError::UnexpectedBreak));
    let t: Ticket = match unmarshal(&hex("bf029fbf00410101 9f6161ffffffff"
        .replace(' ', "")
        .as_str()))
    {
        Ok(t) => t,
        Err(e) => panic!("{e}"),
    };
    assert_eq!(
        t.members,
        Some(vec![Member {
            id: Some(vec![1]),
            addrs: vec!["a".to_owned()]
        }])
    );
}

#[test]
fn pointers_and_nil() {
    let view = |input: &str| -> View {
        match unmarshal(&hex(input)) {
            Ok(v) => v,
            Err(e) => panic!("input {input}: {e}"),
        }
    };
    assert_eq!(view("a10bf6").pending, None);
    assert_eq!(view("a10bf7").pending, None);
    // A tagged null allocates the pointer first, as Go does.
    assert_eq!(view("a10bc6f6").pending, Some(Box::default()));
    assert_eq!(view("a10ba0").pending, Some(Box::default()));
    assert_eq!(
        view("a10ba101190100".replace("190100", "18ff").as_str()).pending,
        Some(Box::new(Pending { replicas: 255 }))
    );
    let err: Result<View, _> = unmarshal(&hex("a10ba101190100"));
    assert_eq!(
        err.map_err(|e| e.to_string()),
        Err(
            "cbor: cannot unmarshal positive integer into Go struct field view.View.11 of type uint8 (256 overflows uint8)"
                .to_owned()
        )
    );
    // [][]byte in views keeps nil elements.
    assert_eq!(view("a10d82f640").fenced, vec![None, Some(vec![])]);
    assert_eq!(to_hex(&marshal(&view("a10d82f640"))), "a10d82f640");
    // [][]byte in wire turns them into empty vectors (R6).
    assert_eq!(msg_ok("a10482f640").keys, vec![Vec::<u8>::new(), vec![]]);
    // Non-omitempty []byte and []T keep nil versus empty.
    let t: Ticket = match unmarshal(&hex("a300f601000280")) {
        Ok(t) => t,
        Err(e) => panic!("{e}"),
    };
    assert_eq!(t.cluster_id, None);
    assert_eq!(t.members, Some(vec![]));
    let r: RefInfo = match unmarshal(&hex("a2014002f6")) {
        Ok(r) => r,
        Err(e) => panic!("{e}"),
    };
    assert_eq!((r.key, r.version), (Some(vec![]), None));
}

#[test]
fn encode_nil_empty_and_omitempty() {
    assert_eq!(to_hex(&marshal(&Msg::default())), "a10000");
    assert_eq!(
        to_hex(&marshal(&RefInfo::default())),
        "a40060 01f6 02f6 0300".replace(' ', "")
    );
    let r = RefInfo {
        key: Some(vec![]),
        user: "u".to_owned(),
        ..RefInfo::default()
    };
    assert_eq!(
        to_hex(&marshal(&r)),
        "a5006001400 2f6030004 6175".replace(' ', "")
    );
    assert_eq!(to_hex(&marshal(&Ticket::default())), "a300f6010002f6");
    let t = Ticket {
        members: Some(vec![]),
        ..Ticket::default()
    };
    assert_eq!(to_hex(&marshal(&t)), "a300f601000280");
    let m = Msg {
        typ: 7,
        keys: vec![vec![1]],
        weight: 5,
        refs: vec![RefInfo::default()],
        ..Msg::default()
    };
    assert_eq!(
        to_hex(&marshal(&m)),
        "a4 0007 04814101 1581a40060 01f6 02f6 0300 182b05".replace(' ', "")
    );
    assert_eq!(to_hex(&marshal(&View::default())), "a0");
    let v = View {
        pending: Some(Box::default()),
        fenced: vec![None],
    };
    assert_eq!(to_hex(&marshal(&v)), "a20ba101000d81f6");
}

#[test]
fn round_trip() {
    let m = Msg {
        typ: -3,
        cluster_id: vec![0xaa; 16],
        incarnation: u64::MAX,
        keys: vec![vec![0x11; 32], vec![0x22; 32]],
        name: "héllo".to_owned(),
        record: vec![],
        refs: vec![
            RefInfo {
                name: "a".to_owned(),
                key: Some(vec![1; 32]),
                version: None,
                created_at: i64::MIN,
                user: String::new(),
            },
            RefInfo::default(),
        ],
        failed: vec![KeyFailure {
            key: Some(vec![]),
            node: None,
            reason: "r".to_owned(),
            retry_after: 1500,
        }],
        weight: u32::MAX,
    };
    let b = marshal(&m);
    assert_eq!(unmarshal::<Msg>(&b), Ok(m.clone()));
    assert_eq!(marshal(&unmarshal::<Msg>(&b).unwrap_or_default()), b);
}
