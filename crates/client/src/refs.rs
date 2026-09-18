//! `client/refs.go`: `RefGet`, `RefPut`, `RefDelete`, `RefList`, CAS conditions and their errors.

use amber_store_core::reference;
use dstore_wire::RefInfo;

use crate::{Cluster, Ctx, Error};

/// `client.Ref`.
pub struct Ref {
    pub name: String,
    pub record: Vec<u8>,
    pub version: Vec<u8>,
    pub reference: reference::Reference,
}

/// `client.Cond`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Cond {
    pub expected_version: Vec<u8>,
    pub versioned: bool,
    pub expected_old: Vec<u8>,
    pub keyed: bool,
    pub force: bool,
}

/// `*client.CASMismatch`.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{}", self.message())]
pub struct CasMismatch {
    pub current: Vec<u8>,
    pub record: Vec<u8>,
    pub version: Vec<u8>,
    pub has_current: bool,
}

impl CasMismatch {
    /// "cas mismatch: reference is absent" | "cas mismatch: current key <%x>".
    pub fn message(&self) -> String {
        todo!()
    }
}

/// `*client.Incomplete`.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("incomplete: {shortfall} keys short")]
pub struct Incomplete {
    pub sample: Vec<[u8; 32]>,
    pub shortfall: i64,
}

impl Cluster {
    pub async fn ref_get(&self, ctx: &Ctx, name: &str) -> Result<Ref, Error> {
        todo!()
    }

    pub async fn ref_put(&self, ctx: &Ctx, record: &[u8], cond: &Cond) -> Result<Vec<u8>, Error> {
        todo!()
    }

    pub async fn ref_delete(&self, ctx: &Ctx, name: &str, cond: &Cond) -> Result<(), Error> {
        todo!()
    }

    /// Errors are not mapped through refErr.
    pub async fn ref_list(&self, ctx: &Ctx, prefix: &[u8]) -> Result<Vec<RefInfo>, Error> {
        todo!()
    }
}

/// Go `reference.ValidateName` over raw argv bytes: empty, then > 1024 bytes, then "must be valid UTF-8",
/// then core-rs `validate_name`.
pub fn validate_name_bytes(name: &[u8]) -> Result<(), String> {
    todo!()
}

/// The same for `reference.ValidateUser` ("user must …").
pub fn validate_user_bytes(user: &[u8]) -> Result<(), String> {
    todo!()
}
