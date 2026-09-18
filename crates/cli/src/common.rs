//! Shared helpers of the commands (cli.md §2.5): dial, admin, status printing, records, local stores,
//! hex, signals.

use std::sync::Arc;

use amber_store_core::{amberpack, packstore, refstore};
use dstore_client::Cluster;
use dstore_gocli::{CliError, Context};
use dstore_gocompat::ctx::Ctx;
use dstore_gocompat::slog::Logger;
use dstore_ticket::Ticket;
use dstore_transport_iroh::IrohEndpoint;
use dstore_wire::{AdminReply, AdminRequest};

/// `--relay`, `--no-relay`, `--no-discovery`.
pub struct NetOpts {
    pub relay: String,
    pub no_relay: bool,
    pub no_discovery: bool,
}

pub fn net_opts_of(c: &Context) -> NetOpts {
    todo!()
}

/// A `TextHandler` on stderr, built per call as Go does.
pub fn logger(c: &Context) -> Logger {
    todo!()
}

/// `signalCtx`: SIGINT+SIGTERM → cancel; the handlers stay installed.
pub fn signal_ctx() -> Ctx {
    todo!()
}

/// A dialled cluster and its endpoint.
pub struct Session {
    pub cluster: Cluster,
    pub endpoint: Arc<IrohEndpoint>,
}

impl Session {
    /// `cluster.close()`, then `endpoint.close_bounded(3 s)` (DD-11).
    pub async fn close(self) {
        todo!()
    }
}

pub async fn dial_cluster(ctx: &Ctx, c: &Context, log: &Logger) -> Result<Session, CliError> {
    todo!()
}

pub async fn dial_ticket(
    ctx: &Ctx,
    t: Ticket,
    n: &NetOpts,
    log: &Logger,
) -> Result<Session, CliError> {
    todo!()
}

pub async fn admin(ctx: &Ctx, cl: &Cluster, req: &AdminRequest) -> Result<AdminReply, CliError> {
    todo!()
}

pub async fn admin_action(c: &Context, req: AdminRequest) -> Result<(), CliError> {
    todo!()
}

pub async fn print_status(
    ctx: &Ctx,
    cl: &Cluster,
    out: &mut (dyn std::io::Write + Send),
) -> Result<(), CliError> {
    todo!()
}

pub fn record_payload(rec: &[u8]) -> Result<Vec<u8>, amberpack::Error> {
    todo!()
}

/// PORTING.md §2.3.
pub fn open_local(c: &Context) -> Result<(Arc<packstore::Store>, refstore::Store), CliError> {
    todo!()
}

/// "bad hex %q"; an odd trailing nibble is dropped.
pub fn hex_decode(s: &[u8]) -> Result<Vec<u8>, String> {
    todo!()
}
