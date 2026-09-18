//! Node-side commands (PORTING.md §2.2): identical definitions and pre-store validation, then fixed
//! errors.

use dstore_gocli::CliError;
use dstore_ticket::Ticket;

/// `<cmd> is a node-side command and dstore-client-rs does not implement the dstore node; use the Go
/// dstore binary (github.com/amber-store/dstore v0.1.9)`, where `<cmd>` is `serve`, `cluster init` or
/// `node join`.
pub fn node_side_error(cmd: &str) -> CliError {
    todo!()
}

pub const STORE_TICKET_UNSUPPORTED: &str = "deriving a ticket from --store needs the node's Pebble meta store, which dstore-client-rs does not implement; pass --ticket or $DSTORE_TICKET, or use the Go dstore binary";

pub const CATALOG_RESTORE_UNSUPPORTED: &str = "catalog restore writes through the node's paxos acceptor, which dstore-client-rs does not implement; use the Go dstore binary";

/// §2.2 B: the identity read, then `STORE_TICKET_UNSUPPORTED`.
pub fn local_ticket(dir: &[u8]) -> Result<Ticket, CliError> {
    todo!()
}
