//! dstore `placement/placement.go` and `view/view.go`, plus `worktree.TicketFromView`
//! (`worktree/flow.go:306-315`) and `node.ShortID` (`node/status.go:118-124`).
//!
//! [`NodeId`] is defined here and re-exported by transport, client and the facade. It is never validated
//! as a curve point; validation happens only at dial time (transport-iroh).
//!
//! Spec: PORTING.md §4.5; port-notes/view-placement.md.
#![allow(dead_code, unused_variables)] // L0 stubs: remove once the modules are implemented
#![deny(unsafe_op_in_unsafe_fn)]

pub mod placement;
pub mod view;

pub use placement::NodeId;
pub use view::*;

/// `worktree.TicketFromView`: the first ≤ 4 of `v.nodes` in view order; no nodes → members None (f6).
pub fn ticket_from_view(v: &view::View) -> dstore_ticket::Ticket {
    todo!()
}
