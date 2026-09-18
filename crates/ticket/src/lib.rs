//! dstore `ticket/ticket.go`: `dstore1` tickets, and go-iroh v0.2.0 `key/key.go` `ParseEndpointID`
//! (`decodeBase32OrHex`, `NewPublicKey`, the z-base-32 hint, `key_core.go`).
//!
//! The curve check uses `iroh_base::PublicKey::from_bytes`; a golden vector confirms it accepts exactly
//! what filippo `edwards25519.Point.SetBytes` accepts.
//!
//! Spec: PORTING.md §4.4; port-notes/codec-wire-ticket.md §2.4, §3.4, §4.4.
#![allow(dead_code, unused_variables)] // L0 stubs: remove once the crate is implemented
#![deny(unsafe_op_in_unsafe_fn)]

use dstore_codec::cbor_struct;

/// The ticket string prefix.
pub const PREFIX: &str = "dstore1";

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

impl Ticket {
    /// `Ticket.Encode`: "dstore1" + lower(base32 std nopad (CBOR)).
    pub fn encode(&self) -> String {
        todo!()
    }

    /// `Ticket.IDs`: hex of the 32-byte member ids, first occurrence, ","-joined.
    pub fn ids(&self) -> String {
        todo!()
    }

    /// The members; None → &[].
    pub fn members(&self) -> &[Member] {
        todo!()
    }
}

/// `Ticket.String` = `encode()`.
impl std::fmt::Display for Ticket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        todo!()
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
    #[error("{0}: input is z-base-32, use ParseEndpointIDZ32")]
    Z32(Box<KeyError>),
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

/// Go `ticket.Parse`, validation order codec-wire-ticket §2.4.4.
pub fn parse(s: &[u8]) -> Result<Ticket, TicketError> {
    todo!()
}

/// go-iroh `ParseEndpointID` on the given bytes.
pub fn parse_endpoint_id(s: &[u8]) -> Result<[u8; 32], KeyError> {
    todo!()
}

/// go-iroh `NewPublicKey`'s curve-point check.
pub fn is_valid_public_key(b: &[u8; 32]) -> bool {
    todo!()
}
