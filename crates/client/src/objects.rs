//! `client/objects.go`: `Missing`, `Put`, `Placed`, `Get`, `VerifyRecord` (part B).

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use amber_store_core::amberpack;
use dstore_wire::KeyFailure;

use crate::{Cluster, Ctx, Error, NodeId, PutObserver, RecordSizer};

/// Reads a record to upload.
pub type RecordSource = Arc<dyn Fn(&[u8; 32]) -> Result<Vec<u8>, Error> + Send + Sync>;

/// `client.MissingResult`.
pub struct MissingResult {
    pub lacking: HashMap<NodeId, Vec<[u8; 32]>>,
    pub holders: HashMap<[u8; 32], Vec<NodeId>>,
    pub failed: HashMap<[u8; 32], Arc<Error>>,
}

/// `client.PutResult`.
pub struct PutResult {
    pub holders: HashMap<[u8; 32], Vec<NodeId>>,
    pub failed: HashMap<[u8; 32], Vec<KeyFailure>>,
    pub rejected: HashMap<[u8; 32], String>,
    pub errors: HashMap<NodeId, Arc<Error>>,
}

/// One record of a `Get`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GetResult {
    pub key: [u8; 32],
    pub record: Vec<u8>,
}

type GetInner = Pin<Box<dyn futures::Stream<Item = Result<GetResult, Error>> + Send>>;

/// Lazy: nothing starts until first polled; dropping it cancels the fetcher. Records arrive in arrival
/// order. A ctx error after the last record is yielded as Err (Go yields ctx.Err()).
pub struct GetStream {
    inner: GetInner,
    missing: Arc<Mutex<Vec<[u8; 32]>>>,
}

impl futures::Stream for GetStream {
    type Item = Result<GetResult, Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        todo!()
    }
}

impl GetStream {
    /// Accumulates across polls, as Go.
    pub fn missing(&self) -> Vec<[u8; 32]> {
        todo!()
    }
}

impl Cluster {
    /// Never Err in v0.1.9.
    pub async fn missing(
        &self,
        ctx: &Ctx,
        keys: &[[u8; 32]],
        pin: bool,
    ) -> Result<MissingResult, Error> {
        todo!()
    }

    pub async fn put(
        &self,
        ctx: &Ctx,
        by_primary: HashMap<NodeId, Vec<[u8; 32]>>,
        src: RecordSource,
        size: RecordSizer,
        obs: PutObserver,
    ) -> PutResult {
        todo!()
    }

    pub fn placed(&self, key: &[u8; 32], holders: &[NodeId]) -> bool {
        todo!()
    }

    pub fn get(&self, ctx: &Ctx, keys: Vec<[u8; 32]>) -> GetStream {
        todo!()
    }
}

/// `client.VerifyRecord`: (key, payload).
pub fn verify_record(raw: &amberpack::RawRecord) -> Result<([u8; 32], Vec<u8>), Error> {
    todo!()
}
