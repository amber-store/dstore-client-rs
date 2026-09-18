//! Golden tests of `dstore-wire` pack framing (owner wire-pack): `wire/pack_frames.json`,
//! `wire/pack_reader.json`.
//!
//! - `pack_frames.json`: `PackSender` produces each stream of Go's `SendPackRecords`: the frame lengths,
//!   heads and data, and the stream hash. For a failing source, the sender is dropped without `finish` and
//!   must have written exactly the frames Go wrote. `PackRecords` then reads each stream back as Go's
//!   `NewPackReader` + amberpack `Records` did.
//! - `pack_reader.json`: `PackReader::read` with Go's buffer size until its sticky error, one more read,
//!   then a fresh `PackRecords` (records and error), `drain`, and the next frame through `read_msg`.
//!
//! The vectors' BLAKE3-256 digests are compared in full.

use amber_store_core::amberpack::{self, REC_HEADER_SIZE};
use amber_store_core::key::{Key, Type};
use dstore_testkit::golden::{self, Payload};
use dstore_testkit::splitmix;
use dstore_wire::{
    Msg, PackReadError, PackReader, PackRecords, PackSender, ProtocolFrameError, WireError,
    read_msg, read_protocol_msg,
};
use serde::Deserialize;

/// BLAKE3-256(`data`), lowercase hex.
fn blake3_hex(data: &[u8]) -> String {
    hex::encode(blake3::hash(data).as_bytes())
}

fn assert_blake3(label: &str, data: &[u8], want: &str) {
    assert_eq!(blake3_hex(data), want, "{label}: BLAKE3-256 digest");
}

fn be32(b: &[u8], at: usize) -> u32 {
    match b.get(at..at + 4) {
        Some(&[a, b, c, d]) => u32::from_be_bytes([a, b, c, d]),
        _ => panic!("be32 at {at} beyond {} bytes", b.len()),
    }
}

/// VECTORS.md "Read result kind" of a `wire.ReadMsg` error.
fn read_kind(e: &WireError) -> &'static str {
    match e {
        WireError::Eof => "eof",
        WireError::UnexpectedEof => "unexpected_eof",
        WireError::TooLarge(_) => "too_large",
        WireError::Short(_) => "short",
        WireError::Decode(_) => "decode",
        WireError::Io(_) => "io",
        other => panic!("read_msg returned a non-read error: {other}"),
    }
}

/// The same kinds for `protocol.ReadMsg` errors.
fn frame_kind(e: &ProtocolFrameError) -> &'static str {
    match e {
        ProtocolFrameError::Eof => "eof",
        ProtocolFrameError::UnexpectedEof => "unexpected_eof",
        ProtocolFrameError::TooLarge(_) => "too_large",
        ProtocolFrameError::Short(_) => "short",
        ProtocolFrameError::Decode(_) => "decode",
        ProtocolFrameError::Io(_) => "io",
    }
}

#[test]
fn blake3_digest_of_the_empty_input() {
    assert_blake3(
        "empty input",
        b"",
        "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262",
    );
}

// ---- wire/pack_frames.json ----

#[derive(Deserialize)]
struct FramesFile {
    cases: Vec<FramesCase>,
}

#[derive(Deserialize)]
struct FramesCase {
    name: String,
    records: Vec<RecordJson>,
    record_seq: Option<RecordSeq>,
    records_blake3: String,
    source_error_after: Option<usize>,
    frames: Vec<FrameJson>,
    stream_len: usize,
    stream_blake3: String,
    stream_hex: Option<String>,
    read_back: ReadBack,
}

#[derive(Deserialize)]
struct RecordJson {
    key: String,
    flags: u8,
    ulen: u32,
    slen: u32,
    header_hex: String,
    payload: Payload,
    record_hex: Option<String>,
}

#[derive(Deserialize)]
struct RecordSeq {
    count: usize,
    #[serde(deserialize_with = "golden::decimal_u64")]
    first_seed: u64,
    len: usize,
}

#[derive(Deserialize)]
struct FrameJson {
    #[serde(rename = "type")]
    typ: i64,
    frame_len: usize,
    head_hex: String,
    data_len: usize,
    data_blake3: String,
    data_hex: Option<String>,
}

#[derive(Deserialize)]
struct ReadBack {
    records: usize,
    error: Option<String>,
    next: String,
}

/// The record bytes of a case: `header_hex ‖ payload`, or the raw records of `record_seq`.
fn case_records(c: &FramesCase) -> Vec<Vec<u8>> {
    if let Some(seq) = &c.record_seq {
        assert!(c.records.is_empty(), "{}: records and record_seq", c.name);
        return (0..seq.count)
            .map(|i| {
                let data = splitmix::data(seq.first_seed + i as u64, seq.len);
                let k = Key::new(Type::Blob, seq.len as u64, &data);
                let rec = amberpack::encode_record(k, &data)
                    .unwrap_or_else(|e| panic!("{}: encode_record {i}: {e}", c.name));
                assert_eq!(rec[33], 0, "{}: record {i} must stay raw", c.name);
                rec
            })
            .collect();
    }
    c.records
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let label = format!("{}: record {i}", c.name);
            let mut rec = golden::hex(&r.header_hex);
            assert_eq!(rec.len(), REC_HEADER_SIZE, "{label}: header length");
            assert_eq!(rec[0], 1, "{label}: tag");
            assert_eq!(hex::encode(&rec[1..33]), r.key, "{label}: key");
            assert_eq!(rec[33], r.flags, "{label}: flags");
            assert_eq!(be32(&rec, 34), r.ulen, "{label}: ulen");
            assert_eq!(be32(&rec, 38), r.slen, "{label}: slen");
            rec.extend_from_slice(&r.payload.bytes());
            assert_eq!(
                rec.len(),
                REC_HEADER_SIZE + r.slen as usize,
                "{label}: payload length"
            );
            if let Some(h) = &r.record_hex {
                assert_eq!(&hex::encode(&rec), h, "{label}: record_hex");
            }
            rec
        })
        .collect()
}

#[tokio::test]
async fn pack_frames() {
    let file: FramesFile = golden::load_json("wire/pack_frames.json");
    assert!(!file.cases.is_empty());
    for c in &file.cases {
        let recs = case_records(c);
        assert_blake3(
            &format!("{}: records", c.name),
            &recs.concat(),
            &c.records_blake3,
        );

        let mut out = Vec::new();
        {
            let mut sender = PackSender::new(&mut out);
            let upto = c.source_error_after.unwrap_or(recs.len());
            assert!(upto <= recs.len(), "{}: source_error_after", c.name);
            for (i, rec) in recs[..upto].iter().enumerate() {
                sender
                    .add_record(rec)
                    .await
                    .unwrap_or_else(|e| panic!("{}: add_record {i}: {e}", c.name));
            }
            if c.source_error_after.is_none() {
                sender
                    .finish()
                    .await
                    .unwrap_or_else(|e| panic!("{}: finish: {e}", c.name));
            }
            // With a source error, Go returns without Close/finish: the sender is dropped.
        }
        assert_eq!(out.len(), c.stream_len, "{}: stream_len", c.name);
        if let Some(h) = &c.stream_hex {
            assert_eq!(&hex::encode(&out), h, "{}: stream_hex", c.name);
        }
        assert_blake3(&format!("{}: stream", c.name), &out, &c.stream_blake3);
        check_frames(c, &out).await;

        let mut rest: &[u8] = &out;
        let (count, error) = {
            let mut records = PackRecords::new(PackReader::new(&mut rest));
            let mut count = 0usize;
            let mut error = None;
            while let Some(r) = records.next().await {
                match r {
                    Ok(_) => count += 1,
                    Err(e) => {
                        error = Some(e.to_string());
                        break;
                    }
                }
            }
            if error.is_none()
                && let Err(e) = records.drain().await
            {
                error = Some(format!("drain: {e}"));
            }
            (count, error)
        };
        assert_eq!(count, c.read_back.records, "{}: read_back records", c.name);
        assert_eq!(error, c.read_back.error, "{}: read_back error", c.name);
        let next = match read_msg(&mut rest).await {
            Ok(_) => "ok",
            Err(e) => read_kind(&e),
        };
        assert_eq!(next, c.read_back.next, "{}: read_back next", c.name);
    }
}

/// Splits the stream into frames and compares each with the vector.
async fn check_frames(c: &FramesCase, out: &[u8]) {
    let mut off = 0usize;
    let mut got = 0usize;
    while off < out.len() {
        let label = format!("{}: frame {got}", c.name);
        let want = c
            .frames
            .get(got)
            .unwrap_or_else(|| panic!("{label}: more frames than the {} of Go", c.frames.len()));
        let n = be32(out, off) as usize;
        assert_eq!(n, want.frame_len, "{label}: frame_len");
        let frame = out
            .get(off..off + 4 + n)
            .unwrap_or_else(|| panic!("{label}: cut frame"));
        let m = read_protocol_msg(&mut &frame[..])
            .await
            .unwrap_or_else(|e| panic!("{label}: {e}"));
        assert_eq!(m.typ, want.typ, "{label}: type");
        let head_len = frame.len() - m.data.len();
        assert_eq!(
            &frame[head_len..],
            &m.data[..],
            "{label}: data at the end of the frame"
        );
        assert_eq!(
            hex::encode(&frame[..head_len]),
            want.head_hex,
            "{label}: head_hex"
        );
        assert_eq!(m.data.len(), want.data_len, "{label}: data_len");
        if let Some(h) = &want.data_hex {
            assert_eq!(&hex::encode(&m.data), h, "{label}: data_hex");
        }
        assert_blake3(&label, &m.data, &want.data_blake3);
        off += 4 + n;
        got += 1;
    }
    assert_eq!(got, c.frames.len(), "{}: frame count", c.name);
}

// ---- wire/pack_reader.json ----

#[derive(Deserialize)]
struct ReaderFile {
    cases: Vec<ReaderCase>,
}

#[derive(Deserialize)]
struct ReaderCase {
    name: String,
    stream_hex: String,
    read: ReadJson,
    records: RecordsJson,
}

#[derive(Deserialize)]
struct ReadJson {
    read_size: usize,
    data_hex: String,
    end: EndJson,
    again: EndJson,
}

#[derive(Debug, Deserialize)]
struct EndJson {
    kind: String,
    frame_kind: Option<String>,
    error: Option<String>,
    remote: Option<RemoteJson>,
    unexpected_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RemoteJson {
    code: String,
    text: String,
    current: String,
}

#[derive(Deserialize)]
struct RecordsJson {
    records: Vec<ReadRecordJson>,
    error: Option<String>,
    drain_error: Option<String>,
    next: NextJson,
}

#[derive(Deserialize)]
struct ReadRecordJson {
    key: String,
    flags: u8,
    ulen: u32,
    slen: u32,
    bytes_hex: String,
}

#[derive(Deserialize)]
struct NextJson {
    result: String,
    msg: Option<serde_json::Map<String, serde_json::Value>>,
    go_error: Option<String>,
}

/// Compares how a read ended (`Ok` = EOF) with the vector.
fn check_end(label: &str, got: &Result<(), PackReadError>, want: &EndJson) {
    match (want.kind.as_str(), got) {
        ("eof", Ok(())) => {}
        ("frame", Err(PackReadError::Frame(e))) => {
            assert_eq!(
                Some(frame_kind(e)),
                want.frame_kind.as_deref(),
                "{label}: frame_kind"
            );
        }
        ("remote", Err(PackReadError::Remote(re))) => {
            let w = want
                .remote
                .as_ref()
                .unwrap_or_else(|| panic!("{label}: remote without details"));
            assert_eq!(
                (re.code.as_str(), re.text.as_str(), hex::encode(&re.current)),
                (w.code.as_str(), w.text.as_str(), w.current.clone()),
                "{label}: remote error"
            );
        }
        ("unexpected", Err(PackReadError::Unexpected(t))) => {
            assert_eq!(
                Some(t.to_string()),
                want.unexpected_type,
                "{label}: unexpected type"
            );
        }
        _ => panic!("{label}: got {got:?}, want {want:?}"),
    }
    let text = got.as_ref().err().map(|e| e.to_string());
    assert_eq!(text, want.error, "{label}: error text");
}

#[tokio::test]
async fn pack_reader() {
    let file: ReaderFile = golden::load_json("wire/pack_reader.json");
    assert!(!file.cases.is_empty());
    for c in &file.cases {
        let stream = golden::hex(&c.stream_hex);

        // packReader.Read with a read_size buffer until an error, then once more.
        let mut pr = PackReader::new(&stream[..]);
        let mut buf = vec![0u8; c.read.read_size];
        let mut data = Vec::new();
        let end = loop {
            match pr.read(&mut buf).await {
                Ok(0) => break Ok(()),
                Ok(n) => data.extend_from_slice(&buf[..n]),
                Err(e) => break Err(e),
            }
        };
        assert_eq!(hex::encode(&data), c.read.data_hex, "{}: data read", c.name);
        check_end(&format!("{}: end", c.name), &end, &c.read.end);
        let again = match pr.read(&mut buf).await {
            Ok(0) => Ok(()),
            Ok(n) => panic!("{}: read {n} bytes after the end", c.name),
            Err(e) => Err(e),
        };
        check_end(&format!("{}: again", c.name), &again, &c.read.again);

        // amberpack Records over a fresh reader, then io.Copy(io.Discard, pr), then wire.ReadMsg.
        let mut rest: &[u8] = &stream;
        let (records, error, drain_error) = {
            let mut recs = PackRecords::new(PackReader::new(&mut rest));
            let mut records = Vec::new();
            let mut error = None;
            while let Some(r) = recs.next().await {
                match r {
                    Ok(rec) => records.push(rec),
                    Err(e) => {
                        error = Some(e.to_string());
                        assert!(
                            recs.next().await.is_none(),
                            "{}: one error, then None",
                            c.name
                        );
                        break;
                    }
                }
            }
            let drain_error = recs.drain().await.err().map(|e| e.to_string());
            (records, error, drain_error)
        };
        assert_eq!(
            records.len(),
            c.records.records.len(),
            "{}: record count",
            c.name
        );
        for (i, (got, want)) in records.iter().zip(&c.records.records).enumerate() {
            let label = format!("{}: record {i}", c.name);
            assert_eq!(got.record.key.to_string(), want.key, "{label}: key");
            assert_eq!(
                (got.record.flags, got.record.ulen, got.record.slen),
                (want.flags, want.ulen, want.slen),
                "{label}: header"
            );
            assert_eq!(hex::encode(&got.bytes), want.bytes_hex, "{label}: bytes");
        }
        assert_eq!(error, c.records.error, "{}: records error", c.name);
        assert_eq!(
            drain_error, c.records.drain_error,
            "{}: drain error",
            c.name
        );
        check_next(&c.name, read_msg(&mut rest).await, &c.records.next);
    }
}

/// `wire.ReadMsg` on the stream after the drain.
fn check_next(name: &str, got: Result<Msg, WireError>, want: &NextJson) {
    match got {
        Ok(m) => {
            assert_eq!(want.result, "ok", "{name}: next read gave {m:?}");
            let fields = want
                .msg
                .as_ref()
                .unwrap_or_else(|| panic!("{name}: next without msg"));
            let typ = match fields.get("typ") {
                Some(serde_json::Value::String(s)) => s
                    .parse::<i64>()
                    .unwrap_or_else(|e| panic!("{name}: next typ {s:?}: {e}")),
                other => panic!("{name}: next typ {other:?}"),
            };
            assert!(
                fields.keys().all(|k| k == "typ"),
                "{name}: next msg fields beyond typ are not compared: {fields:?}"
            );
            assert_eq!(
                m,
                Msg {
                    typ,
                    ..Default::default()
                },
                "{name}: next msg"
            );
        }
        Err(e) => {
            assert_eq!(read_kind(&e), want.result, "{name}: next read ({e})");
            let text = match e {
                WireError::Eof => None,
                e => Some(e.to_string()),
            };
            assert_eq!(text, want.go_error, "{name}: next error text");
        }
    }
}
