//! The subset of urfave/cli v2.27.7 dstore uses (`app.go`, `command.go`, `help.go`, `template.go`,
//! `flag*.go`, `context.go`, `errors.go`), Go `flag.(*FlagSet).parseOne` with its value parsers, and
//! `text/tabwriter` (minwidth 1, tabwidth 8, padding 2, pad ' ', flags 0). Every quirk of PORTING.md §1.4
//! is reproduced.
//!
//! Spec: PORTING.md §4.11; port-notes/cli.md §2.1-§2.2, §3.1-§3.2, §4.2.
#![allow(dead_code, unused_variables)] // L0 stubs: remove once the modules are implemented
#![deny(unsafe_op_in_unsafe_fn)]

mod app;
mod context;
mod flag;
pub mod goflag;
pub mod help;

pub use app::*;
pub use context::*;
pub use flag::*;
