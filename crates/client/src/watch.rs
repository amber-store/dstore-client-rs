//! `client/watch.go`: `WatchRefs`.

use std::collections::HashMap;

use crate::{Cluster, Ctx, Error, NodeId};

/// `client.RefChange`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RefChange {
    pub name: String,
    pub key: Option<Vec<u8>>,
    pub version: Vec<u8>,
    pub created_at: i64,
    pub user: String,
    pub deleted: bool,
    pub synced: bool,
    pub node: NodeId,
}

/// The stream `watch_refs` returns.
pub type WatchStream =
    std::pin::Pin<Box<dyn futures::Stream<Item = Result<RefChange, Error>> + Send + 'static>>;

impl Cluster {
    /// An async-stream generator (pull semantics). Ends without an item when ctx ends; yields Err for
    /// bad-request/unauthorized, then ends.
    pub fn watch_refs(
        &self,
        ctx: Ctx,
        pattern: String,
        known: HashMap<String, Vec<u8>>,
    ) -> WatchStream {
        todo!()
    }
}
