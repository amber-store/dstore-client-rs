//! `client/rank.go` and the ranking part of `client.go:297-326`.

use std::time::Duration;

use dstore_transport::PathInfo;

use crate::NodeId;

/// <5ms 0, <25ms 1, <100ms 2, else 3.
pub(crate) fn rtt_class(rtt: Duration) -> i64 {
    todo!()
}

/// `rankOwners`.
pub(crate) fn rank_owners(
    ids: &[NodeId],
    penalty: impl Fn(NodeId) -> i64,
    path: impl Fn(NodeId) -> Option<PathInfo>,
) -> Vec<NodeId> {
    todo!()
}
