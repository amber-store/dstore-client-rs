//! Actions of `cluster`, `token`, `node`, `voter`, `transition`, `gc` and `catalog` (`cmd/dstore/main.go`,
//! `client.go:134-193`).
//!
//! Layer L5 (cli-admin) implements the actions below; `app()` calls them. The stubs were added by cli-app
//! (port-notes/impl-cli-app.md) so that the command table compiles.

use dstore_gocli::{CliError, Context};

/// `cluster init` (`main.go:294-316`): node-side, PORTING.md §2.2 A.
pub(crate) async fn cluster_init(c: &Context) -> Result<(), CliError> {
    todo!()
}

/// `cluster status` (`main.go:319-333`).
pub(crate) async fn cluster_status(c: &Context) -> Result<(), CliError> {
    todo!()
}

/// `cluster ticket` (`main.go:334-368`).
pub(crate) async fn cluster_ticket(c: &Context) -> Result<(), CliError> {
    todo!()
}

/// `cluster replicas R` (`main.go:369-392`).
pub(crate) async fn cluster_replicas(c: &Context) -> Result<(), CliError> {
    todo!()
}

/// `serve` (`main.go:409-434`): node-side, PORTING.md §2.2 A.
pub(crate) async fn serve(c: &Context) -> Result<(), CliError> {
    todo!()
}

/// `token create` (`main.go:444-458`).
pub(crate) async fn token_create(c: &Context) -> Result<(), CliError> {
    todo!()
}

/// `node join` (`main.go:486-533`): node-side, PORTING.md §2.2 A.
pub(crate) async fn node_join(c: &Context) -> Result<(), CliError> {
    todo!()
}

/// `node remove ID`.
pub(crate) async fn node_remove(c: &Context) -> Result<(), CliError> {
    todo!()
}

/// `node drain ID`.
pub(crate) async fn node_drain(c: &Context) -> Result<(), CliError> {
    todo!()
}

/// `node weight ID GiB`.
pub(crate) async fn node_weight(c: &Context) -> Result<(), CliError> {
    todo!()
}

/// `node zone ID ZONE`.
pub(crate) async fn node_zone(c: &Context) -> Result<(), CliError> {
    todo!()
}

/// `node repair ID`.
pub(crate) async fn node_repair(c: &Context) -> Result<(), CliError> {
    todo!()
}

/// `voter add ID` (`act("voter-add")`).
pub(crate) async fn voter_add(c: &Context) -> Result<(), CliError> {
    todo!()
}

/// `voter remove ID` (`act("voter-remove")`).
pub(crate) async fn voter_remove(c: &Context) -> Result<(), CliError> {
    todo!()
}

/// `transition status`.
pub(crate) async fn transition_status(c: &Context) -> Result<(), CliError> {
    todo!()
}

/// `transition abort`.
pub(crate) async fn transition_abort(c: &Context) -> Result<(), CliError> {
    todo!()
}

/// `transition refreeze`.
pub(crate) async fn transition_refreeze(c: &Context) -> Result<(), CliError> {
    todo!()
}

/// `transition pause`.
pub(crate) async fn transition_pause(c: &Context) -> Result<(), CliError> {
    todo!()
}

/// `transition resume`.
pub(crate) async fn transition_resume(c: &Context) -> Result<(), CliError> {
    todo!()
}

/// `gc run`.
pub(crate) async fn gc_run(c: &Context) -> Result<(), CliError> {
    todo!()
}

/// `gc status`.
pub(crate) async fn gc_status(c: &Context) -> Result<(), CliError> {
    todo!()
}

/// `gc hold`.
pub(crate) async fn gc_hold(c: &Context) -> Result<(), CliError> {
    todo!()
}

/// `gc release`.
pub(crate) async fn gc_release(c: &Context) -> Result<(), CliError> {
    todo!()
}

/// `gc why KEY`.
pub(crate) async fn gc_why(c: &Context) -> Result<(), CliError> {
    todo!()
}

/// `catalog backup`.
pub(crate) async fn catalog_backup(c: &Context) -> Result<(), CliError> {
    todo!()
}

/// `catalog backups`.
pub(crate) async fn catalog_backups(c: &Context) -> Result<(), CliError> {
    todo!()
}

/// `catalog restore KEY|FILE` (`main.go:652-695`): PORTING.md §2.2 C.
pub(crate) async fn catalog_restore(c: &Context) -> Result<(), CliError> {
    todo!()
}
