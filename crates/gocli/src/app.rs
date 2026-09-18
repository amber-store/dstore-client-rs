//! Commands, the app, errors and `run` (urfave `app.go`, `command.go`, `errors.go`).

use std::ffi::OsString;
use std::future::Future;
use std::pin::Pin;

use crate::{Context, FlagDef};

/// A command action.
pub type Action =
    for<'a> fn(&'a Context) -> Pin<Box<dyn Future<Output = Result<(), CliError>> + Send + 'a>>;

/// A command definition.
pub struct CommandDef {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub usage: &'static str,
    pub args_usage: &'static str,
    pub description: &'static str,
    pub flags: Vec<FlagDef>,
    pub subcommands: Vec<CommandDef>,
    /// None → the help action.
    pub action: Option<Action>,
}

/// The app definition.
pub struct AppDef {
    pub name: &'static str,
    pub usage: &'static str,
    pub version: String,
    pub flags: Vec<FlagDef>,
    pub commands: Vec<CommandDef>,
}

/// A command failure.
#[derive(Debug)]
pub enum CliError {
    /// Printed "dstore: <msg>", exit 1.
    Msg(String),
    /// Printed "<msg>", exit `code`.
    Exit { msg: String, code: i32 },
}

impl CliError {
    pub fn msg(e: impl std::fmt::Display) -> CliError {
        todo!()
    }
}

/// cli.md §2.2.2: setup, env application, Go flag parse, Incorrect Usage + help, help/version flags,
/// required flags, subcommand dispatch, action or help action.
pub async fn run(
    app: &AppDef,
    args: Vec<OsString>,
    stdout: &mut (dyn std::io::Write + Send),
) -> Result<(), CliError> {
    todo!()
}
