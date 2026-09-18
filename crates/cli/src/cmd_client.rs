//! Actions of `store push/pull`, `refs`, `watch`, `ref get/delete`, `ls` and `cat` (`cmd/dstore/client.go`).
//!
//! Layer L5 (cli-client) implements the actions below; `app()` calls them. The stubs were added by cli-app
//! (port-notes/impl-cli-app.md) so that the command table compiles.

use dstore_gocli::{CliError, Context};

/// `store push PATH NAME` (`client.go:244-299`).
pub(crate) async fn store_push(c: &Context) -> Result<(), CliError> {
    todo!()
}

/// `store pull NAME` (`client.go:309-340`).
pub(crate) async fn store_pull(c: &Context) -> Result<(), CliError> {
    todo!()
}

/// `refs [PREFIX]`.
pub(crate) async fn refs(c: &Context) -> Result<(), CliError> {
    todo!()
}

/// `watch PATTERN` (`client.go:378-403`).
pub(crate) async fn watch(c: &Context) -> Result<(), CliError> {
    todo!()
}

/// `ref get NAME`.
pub(crate) async fn ref_get(c: &Context) -> Result<(), CliError> {
    todo!()
}

/// `ref delete NAME` (`client.go:429-448`).
pub(crate) async fn ref_delete(c: &Context) -> Result<(), CliError> {
    todo!()
}

/// `ls NAME [PATH]` (`client.go:476-508`).
pub(crate) async fn ls(c: &Context) -> Result<(), CliError> {
    todo!()
}

/// `cat NAME PATH` (`client.go:518-550`).
pub(crate) async fn cat(c: &Context) -> Result<(), CliError> {
    todo!()
}
