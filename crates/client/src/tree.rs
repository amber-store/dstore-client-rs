//! `client/tree.go`: `Push`, `Pull`, `PullTree` (part B).

use std::sync::Arc;

use amber_store_core::{key, packstore};

use crate::{Cluster, Cond, Ctx, Error, Progress};

/// `client.PushStats`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PushStats {
    pub keys: i64,
    pub uploaded: i64,
    pub bytes: i64,
    pub version: Vec<u8>,
}

/// `client.PullStats`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullStats {
    pub keys: i64,
    pub fetched: i64,
    pub bytes: i64,
    pub root: key::Key,
    pub record: Vec<u8>,
    pub version: Vec<u8>,
}

/// root = Key([0; 32]).
impl Default for PullStats {
    fn default() -> PullStats {
        todo!()
    }
}

impl Cluster {
    #[allow(clippy::too_many_arguments)] // signature fixed by PORTING.md §4.8
    pub async fn push(
        &self,
        ctx: &Ctx,
        local: Arc<packstore::Store>,
        root: key::Key,
        name: &str,
        user: &str,
        cond: Cond,
        prog: Option<Progress>,
    ) -> Result<PushStats, Error> {
        todo!()
    }

    pub async fn pull(
        &self,
        ctx: &Ctx,
        local: Arc<packstore::Store>,
        name: &str,
        prog: Option<Progress>,
    ) -> Result<PullStats, Error> {
        todo!()
    }

    pub async fn pull_tree(
        &self,
        ctx: &Ctx,
        local: Arc<packstore::Store>,
        root: key::Key,
        st: &mut PullStats,
        prog: Option<Progress>,
    ) -> Result<(), Error> {
        todo!()
    }
}

/// `storedSizer`: 46 + slen when stored, else the key's length.
pub(crate) fn stored_size_of(st: &packstore::Store, k: &[u8; 32]) -> usize {
    todo!()
}
