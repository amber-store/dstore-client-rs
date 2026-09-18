//! Go texts for core-rs errors that reach users (core-rs-gaps G4, §2.7) (part A).

/// fstree `WalkError` Display with names re-quoted by `gocompat::quote` (Go `%q`), else identical to
/// core-rs.
pub fn walk_error_text<E: std::fmt::Display>(e: &amber_store_core::fstree::WalkError<E>) -> String {
    todo!()
}

/// `cbor::Error` with Go cborx texts: "cborx: …" prefix, plain "unexpected EOF" for truncation.
pub fn cbor_error_text(e: &amber_store_core::cbor::Error) -> String {
    todo!()
}
