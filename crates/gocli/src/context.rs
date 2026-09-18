//! `*cli.Context` (urfave `context.go`): lineage lookups, `IsSet`, positional arguments.

use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};

use crate::FlagValue;

/// `*cli.Context` of the running command.
pub struct Context {
    /// Root .. current.
    levels: Vec<LevelState>,
    args: Vec<OsString>,
}

/// One command level of the lineage.
struct LevelState {
    command_path: Vec<&'static str>,
    values: HashMap<&'static str, FlagValue>,
    set_on_cli: HashSet<&'static str>,
    set_from_env: HashSet<&'static str>,
}

impl Context {
    /// Lossy; searches this level then its ancestors; "" when undefined.
    pub fn string(&self, name: &str) -> String {
        todo!()
    }

    pub fn os_string(&self, name: &str) -> OsString {
        todo!()
    }

    pub fn bool(&self, name: &str) -> bool {
        todo!()
    }

    pub fn int(&self, name: &str) -> i64 {
        todo!()
    }

    pub fn int64(&self, name: &str) -> i64 {
        todo!()
    }

    pub fn uint(&self, name: &str) -> u64 {
        todo!()
    }

    pub fn float64(&self, name: &str) -> f64 {
        todo!()
    }

    pub fn duration_ns(&self, name: &str) -> i64 {
        todo!()
    }

    pub fn string_slice(&self, name: &str) -> Vec<String> {
        todo!()
    }

    /// On the command line, or found in the environment (even empty).
    pub fn is_set(&self, name: &str) -> bool {
        todo!()
    }

    pub fn args(&self) -> &[OsString] {
        todo!()
    }

    pub fn narg(&self) -> usize {
        todo!()
    }

    /// "" when missing (Go `Args().Get`).
    pub fn arg(&self, n: usize) -> &OsStr {
        todo!()
    }

    pub fn first(&self) -> &OsStr {
        todo!()
    }
}
