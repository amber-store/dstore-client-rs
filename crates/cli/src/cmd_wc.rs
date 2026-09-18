//! Working-copy actions `clone`, `init`, `fetch`, `pull`, `push`, `status`, `diff`, and `resolve_ticket`,
//! `wc_config`, `push_user`, `describe_change`, `filter_paths` (`cmd/dstore/wc.go`).
//!
//! Layer L5 (cli-wc) implements the actions below; `app()` calls them. The stubs were added by cli-app
//! (port-notes/impl-cli-app.md) so that the command table compiles.

use dstore_gocli::{CliError, Context};

/// `clone NAME [DIR]`.
pub(crate) async fn clone(c: &Context) -> Result<(), CliError> {
    todo!()
}

/// `init NAME`.
pub(crate) async fn init(c: &Context) -> Result<(), CliError> {
    todo!()
}

/// `fetch`.
pub(crate) async fn fetch(c: &Context) -> Result<(), CliError> {
    todo!()
}

/// `pull`.
pub(crate) async fn pull(c: &Context) -> Result<(), CliError> {
    todo!()
}

/// `push`.
pub(crate) async fn push(c: &Context) -> Result<(), CliError> {
    todo!()
}

/// `status`.
pub(crate) async fn status(c: &Context) -> Result<(), CliError> {
    todo!()
}

/// `diff [PATH...]`.
pub(crate) async fn diff(c: &Context) -> Result<(), CliError> {
    todo!()
}
