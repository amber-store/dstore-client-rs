//! Go texts for core-rs errors that reach users (core-rs-gaps G4, §2.7) (part A).
//!
//! core-rs renders names with Rust `{:?}` over `String::from_utf8_lossy`; Go uses `%q`
//! (`strconv.Quote` over the raw bytes, `\xNN` for invalid UTF-8 and controls). core-rs `cbor` errors
//! carry a `cbor: ` prefix where Go's `cborx` package says `cborx: `, and Go's truncation error is the bare
//! `io.ErrUnexpectedEOF`. Everything else is identical to core-rs.

use amber_store_core::cbor;
use amber_store_core::fstree::{self, ChildKeysError, WalkError};
use amber_store_core::key::Key;
use dstore_gocompat::quote::quote;

/// Go `key.Type.String()` of a key's type nibble, without core-rs's panic on reserved types.
fn type_name(k: &Key) -> String {
    match k.0[0] >> 4 {
        0 => "Blob".to_owned(),
        1 => "FileNode".to_owned(),
        2 => "DirLeaf".to_owned(),
        3 => "DirNode".to_owned(),
        4 => "XattrSet".to_owned(),
        n => format!("Type({n})"),
    }
}

/// `fstree` codec errors (`fstree.Error`) with names re-quoted.
fn fstree_error_text(e: &fstree::Error) -> String {
    match e {
        fstree::Error::EntryContentKey { name, source } => {
            format!("entry {} content key: {source}", quote(name))
        }
        fstree::Error::EntryXattrsKey { name, source } => {
            format!("entry {} xattrs key: {source}", quote(name))
        }
        other => other.to_string(),
    }
}

/// `fstree.ChildKeys` errors with names re-quoted.
fn child_keys_error_text(e: &ChildKeysError) -> String {
    match e {
        ChildKeysError::DecodeFileNode { key, source } => {
            format!(
                "fstree: decoding FileNode {key}: {}",
                fstree_error_text(source)
            )
        }
        ChildKeysError::DecodeDirNode { key, source } => {
            format!(
                "fstree: decoding DirNode {key}: {}",
                fstree_error_text(source)
            )
        }
        ChildKeysError::DirNodeChildKey { key, source } => {
            format!("fstree: child key in DirNode {key}: {source}")
        }
        ChildKeysError::DecodeDirLeaf { key, source } => {
            format!(
                "fstree: decoding DirLeaf {key}: {}",
                fstree_error_text(source)
            )
        }
        ChildKeysError::EntryContentKey { name, source } => {
            format!("fstree: {}: content key: {source}", quote(name))
        }
        ChildKeysError::EntryXattrsKey { name, source } => {
            format!("fstree: {}: xattrs key: {source}", quote(name))
        }
    }
}

/// fstree `WalkError` Display with names re-quoted by `gocompat::quote` (Go `%q`), else identical to
/// core-rs.
pub fn walk_error_text<E: std::fmt::Display>(e: &WalkError<E>) -> String {
    match e {
        WalkError::Read { key, source } => format!("fstree: reading {key}: {source}"),
        WalkError::DecodeDirLeaf { key, source } => {
            format!(
                "fstree: decoding DirLeaf {key}: {}",
                fstree_error_text(source)
            )
        }
        WalkError::DecodeDirNode { key, source } => {
            format!(
                "fstree: decoding DirNode {key}: {}",
                fstree_error_text(source)
            )
        }
        WalkError::ChildKey { key, source } => {
            format!("fstree: child key in DirNode {key}: {source}")
        }
        WalkError::OutOfOrder { key, entry, prev } => {
            format!(
                "fstree: DirLeaf {key}: entry {} is not after {}",
                quote(entry),
                quote(prev)
            )
        }
        WalkError::NotDirObject { key } => {
            format!(
                "fstree: {key} is not a directory object (type {})",
                type_name(key)
            )
        }
        WalkError::NotFound { name } => format!("fstree: {}: entry not found", quote(name)),
        WalkError::NotDir { name } => format!("fstree: {}: not a directory", quote(name)),
        WalkError::DotDot { path } => {
            format!(
                "fstree: {}: \"..\" is not supported",
                quote(path.as_bytes())
            )
        }
        WalkError::ContentKey { name, source } => {
            format!("fstree: {}: content key: {source}", quote(name))
        }
        WalkError::BadLimit { limit } => {
            format!("fstree: ListEntries limit must be positive, got {limit}")
        }
        WalkError::Children(c) => child_keys_error_text(c),
        WalkError::Missing(m) => m.to_string(),
        WalkError::Has(h) => h.to_string(),
        WalkError::ContentRead { key, source } => format!("reading {key}: {source}"),
        WalkError::NotContentObject { key } => {
            format!(
                "{key} is not a file-content object (type {})",
                type_name(key)
            )
        }
        WalkError::Codec(c) => fstree_error_text(c),
        WalkError::Io(io) => io.to_string(),
    }
}

/// `cbor::Error` with Go cborx texts: "cborx: …" prefix, plain "unexpected EOF" for truncation.
pub fn cbor_error_text(e: &cbor::Error) -> String {
    match e {
        cbor::Error::UnexpectedEof => "unexpected EOF".to_owned(),
        cbor::Error::UnsupportedAdditionalInfo(ai) => {
            format!("cborx: unsupported additional info {ai}")
        }
        cbor::Error::ExpectedMap { got } => {
            format!("cborx: expected CBOR map (major 5), got major {got}")
        }
        cbor::Error::ExpectedByteString { got } => {
            format!("cborx: expected byte string (major 2), got major {got}")
        }
        cbor::Error::TrailingBytes(n) => format!("cborx: {n} trailing bytes after xattr map"),
        cbor::Error::PairCount { pairs, bytes } => {
            format!("cborx: xattr map claims {pairs} pairs in {bytes} bytes")
        }
        cbor::Error::XattrKey { index, source } => {
            format!("cborx: xattr key {index}: {}", cbor_error_text(source))
        }
        cbor::Error::XattrValue { name, source } => {
            format!(
                "cborx: xattr value for {}: {}",
                quote(name),
                cbor_error_text(source)
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use amber_store_core::fstree::MissingObjectError;
    use amber_store_core::key;

    use super::*;

    fn k(hex: &str) -> Key {
        match Key::parse(&dstore_testkit::golden::hex(hex)) {
            Ok(k) => k,
            Err(e) => panic!("key {hex}: {e}"),
        }
    }

    const EMPTY_TREE: &str = "2001bbe6a9f5a0146a1f4d0381e9b0ed1ac2f1a979ce9d5ad84e46ff0b58f36b";
    const HELLO: &str = "0005ea8f163db38682925e4491c5e58d4bb3506ef8c14eb78a86e908c5624a67";

    // core-rs-gaps §2.3, verified Go texts.
    #[test]
    fn walk_errors_match_go() {
        let not_found: WalkError<String> = WalkError::NotFound {
            name: b"x".to_vec(),
        };
        assert_eq!(
            walk_error_text(&not_found),
            "fstree: \"x\": entry not found"
        );

        let quoted: WalkError<String> = WalkError::NotFound {
            name: b"a\x7f\xffb\"c".to_vec(),
        };
        assert_eq!(
            walk_error_text(&quoted),
            "fstree: \"a\\x7f\\xffb\\\"c\": entry not found"
        );

        let not_dir: WalkError<String> = WalkError::NotDirObject { key: k(HELLO) };
        assert_eq!(
            walk_error_text(&not_dir),
            format!("fstree: {HELLO} is not a directory object (type Blob)")
        );

        let read: WalkError<String> = WalkError::Read {
            key: k("2009759b92959bb4297b5f40d815b91fb154c1e44ebb99f45399fe5dca16b7e1"),
            source: "packstore: object not found".to_owned(),
        };
        assert_eq!(
            walk_error_text(&read),
            "fstree: reading 2009759b92959bb4297b5f40d815b91fb154c1e44ebb99f45399fe5dca16b7e1: packstore: object not found"
        );

        let missing: WalkError<String> = WalkError::Missing(MissingObjectError {
            key: k("00036437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd"),
        });
        assert_eq!(
            walk_error_text(&missing),
            "fstree: object 00036437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd is missing"
        );

        let content: WalkError<String> = WalkError::NotContentObject { key: k(EMPTY_TREE) };
        assert_eq!(
            walk_error_text(&content),
            format!("{EMPTY_TREE} is not a file-content object (type DirLeaf)")
        );
    }

    #[test]
    fn walk_error_quoting_and_reserved_types() {
        let out_of_order: WalkError<String> = WalkError::OutOfOrder {
            key: k(EMPTY_TREE),
            entry: b"a\tb".to_vec(),
            prev: "é\u{200b}".as_bytes().to_vec(),
        };
        assert_eq!(
            walk_error_text(&out_of_order),
            format!("fstree: DirLeaf {EMPTY_TREE}: entry \"a\\tb\" is not after \"é\\u200b\"")
        );
        let dotdot: WalkError<String> = WalkError::DotDot {
            path: "a/../\n".to_owned(),
        };
        assert_eq!(
            walk_error_text(&dotdot),
            "fstree: \"a/../\\n\": \"..\" is not supported"
        );
        let content_key: WalkError<String> = WalkError::ContentKey {
            name: b"\x01".to_vec(),
            source: key::Error::BadKeyLength(3),
        };
        assert_eq!(
            walk_error_text(&content_key),
            "fstree: \"\\x01\": content key: key: data is not 32 bytes: got 3"
        );
        // A reserved type nibble renders as Go's Type(n) instead of panicking.
        let mut raw = [0u8; 32];
        raw[0] = 0x50;
        let reserved: WalkError<String> = WalkError::NotDirObject { key: Key(raw) };
        assert_eq!(
            walk_error_text(&reserved),
            format!(
                "fstree: {} is not a directory object (type Type(5))",
                Key(raw)
            )
        );
        let children: WalkError<String> = WalkError::Children(ChildKeysError::EntryXattrsKey {
            name: b"\xfe".to_vec(),
            source: key::Error::ReservedBitSet,
        });
        assert_eq!(
            walk_error_text(&children),
            "fstree: \"\\xfe\": xattrs key: key: reserved header bit is set"
        );
        let limit: WalkError<String> = WalkError::BadLimit { limit: 0 };
        assert_eq!(walk_error_text(&limit), limit.to_string());
    }

    #[test]
    fn cbor_errors_use_cborx_texts() {
        assert_eq!(
            cbor_error_text(&cbor::Error::UnexpectedEof),
            "unexpected EOF"
        );
        assert_eq!(
            cbor_error_text(&cbor::Error::PairCount { pairs: 1, bytes: 0 }),
            "cborx: xattr map claims 1 pairs in 0 bytes"
        );
        assert_eq!(
            cbor_error_text(&cbor::Error::ExpectedMap { got: 4 }),
            "cborx: expected CBOR map (major 5), got major 4"
        );
        assert_eq!(
            cbor_error_text(&cbor::Error::ExpectedByteString { got: 3 }),
            "cborx: expected byte string (major 2), got major 3"
        );
        assert_eq!(
            cbor_error_text(&cbor::Error::UnsupportedAdditionalInfo(28)),
            "cborx: unsupported additional info 28"
        );
        assert_eq!(
            cbor_error_text(&cbor::Error::TrailingBytes(2)),
            "cborx: 2 trailing bytes after xattr map"
        );
        assert_eq!(
            cbor_error_text(&cbor::Error::XattrKey {
                index: 0,
                source: Box::new(cbor::Error::UnexpectedEof)
            }),
            "cborx: xattr key 0: unexpected EOF"
        );
        assert_eq!(
            cbor_error_text(&cbor::Error::XattrValue {
                name: b"user.\xff".to_vec(),
                source: Box::new(cbor::Error::ExpectedByteString { got: 0 })
            }),
            "cborx: xattr value for \"user.\\xff\": cborx: expected byte string (major 2), got major 0"
        );
    }

    // The decoder's own errors, rendered, equal Go's DecodeXattrs texts for the same inputs.
    #[test]
    fn decode_xattrs_errors() {
        let cases: &[(&[u8], &str)] = &[
            (&[], "unexpected EOF"),
            (&[0x80], "cborx: expected CBOR map (major 5), got major 4"),
            (&[0xa1], "cborx: xattr map claims 1 pairs in 0 bytes"),
            (&[0xa0, 0x00], "cborx: 1 trailing bytes after xattr map"),
        ];
        for (input, want) in cases {
            match cbor::decode_xattrs(input) {
                Ok(m) => panic!("{input:02x?} decoded to {m:?}"),
                Err(e) => assert_eq!(cbor_error_text(&e), *want, "{input:02x?}"),
            }
        }
    }
}
