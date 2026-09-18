//! dstore `placement/placement.go` and `view/view.go`, plus `worktree.TicketFromView`
//! (`worktree/flow.go:306-315`) and `node.ShortID` (`node/status.go:118-124`).
//!
//! [`NodeId`] is defined here and re-exported by transport, client and the facade. It is never validated
//! as a curve point; validation happens only at dial time (transport-iroh).
//!
//! Spec: PORTING.md §4.5; port-notes/view-placement.md.
#![deny(unsafe_op_in_unsafe_fn)]

mod gosort;
pub mod placement;
pub mod view;

pub use placement::NodeId;
pub use view::*;

/// `worktree.TicketFromView`: the first ≤ 4 of `v.nodes` in view order; no nodes → members None (f6).
///
/// Pending-only nodes are ignored, and ids and addresses are copied as the view holds them.
pub fn ticket_from_view(v: &view::View) -> dstore_ticket::Ticket {
    let mut members: Option<Vec<dstore_ticket::Member>> = None;
    for nd in v.nodes() {
        let list = members.get_or_insert_with(Vec::new);
        list.push(dstore_ticket::Member {
            id: nd.id.clone(),
            addrs: nd.addrs.clone(),
        });
        if list.len() >= 4 {
            break;
        }
    }
    dstore_ticket::Ticket {
        cluster_id: v.cluster_id.clone(),
        incarnation: v.incarnation,
        members,
    }
}
