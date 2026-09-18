//! `*cli.Context` (urfave `context.go`, the `lookup*` helpers of `flag_*.go`, `args.go`): lineage
//! lookups, `IsSet`, positional arguments.

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStringExt;

use dstore_gocompat::quote::quote;
use dstore_gocompat::strconv;
use dstore_gocompat::strings::trim_space;

use crate::FlagValue;
use crate::flag::FlagState;
use crate::goflag::{self, FlagSet, Value};

/// `*cli.Context` of the running command.
#[derive(Debug, Default)]
pub struct Context {
    /// Root .. current.
    levels: Vec<LevelState>,
}

/// One command level of the lineage: the flags of its command and the parsed flag set.
#[derive(Debug)]
pub(crate) struct LevelState {
    pub(crate) flags: Vec<FlagState>,
    pub(crate) set: FlagSet,
}

impl Context {
    pub(crate) fn push(&mut self, level: LevelState) {
        self.levels.push(level);
    }

    /// `lookupFlagSet(name)` then `Lookup(name).Value`: this level, then its ancestors.
    fn lookup(&self, name: &str) -> Option<&Value> {
        self.levels
            .iter()
            .rev()
            .find_map(|l| l.set.lookup(name.as_bytes()))
    }

    /// `Value.String()` of the flag, "" when undefined.
    fn value_string(&self, name: &str) -> Vec<u8> {
        self.lookup(name).map(Value::string).unwrap_or_default()
    }

    /// Lossy; searches this level then its ancestors; "" when undefined.
    pub fn string(&self, name: &str) -> String {
        String::from_utf8_lossy(&self.value_string(name)).into_owned()
    }

    pub fn os_string(&self, name: &str) -> OsString {
        OsString::from_vec(self.value_string(name))
    }

    /// `lookupBool`: `strconv.ParseBool` of `Value.String()`, false when it fails.
    pub fn bool(&self, name: &str) -> bool {
        self.lookup(name).is_some_and(|v| {
            strconv::parse_bool(&goflag::numeric_probe(&v.string())).unwrap_or(false)
        })
    }

    /// `lookupInt`: `strconv.ParseInt(Value.String(), 0, 64)`, 0 when it fails.
    pub fn int(&self, name: &str) -> i64 {
        self.lookup(name)
            .and_then(|v| goflag::parse_int(&v.string()).ok())
            .unwrap_or(0)
    }

    /// `lookupInt64`.
    pub fn int64(&self, name: &str) -> i64 {
        self.int(name)
    }

    /// `lookupUint`: `strconv.ParseUint(Value.String(), 0, 64)`, 0 when it fails.
    pub fn uint(&self, name: &str) -> u64 {
        self.lookup(name)
            .and_then(|v| goflag::parse_uint(&v.string()).ok())
            .unwrap_or(0)
    }

    /// `lookupFloat64`: `strconv.ParseFloat(Value.String(), 64)`, 0 when it fails.
    pub fn float64(&self, name: &str) -> f64 {
        self.lookup(name)
            .and_then(|v| goflag::parse_float(&v.string()).ok())
            .unwrap_or(0.0)
    }

    /// `lookupDuration`: `time.ParseDuration(Value.String())`, 0 when it fails.
    pub fn duration_ns(&self, name: &str) -> i64 {
        self.lookup(name)
            .and_then(|v| goflag::parse_duration(&v.string()).ok())
            .unwrap_or(0)
    }

    /// `lookupStringSlice`: the items of a slice flag (lossy), empty for any other flag.
    pub fn string_slice(&self, name: &str) -> Vec<String> {
        match self.lookup(name) {
            Some(Value::StringSlice { items, .. }) => items
                .iter()
                .map(|i| String::from_utf8_lossy(i).into_owned())
                .collect(),
            _ => Vec::new(),
        }
    }

    /// `Value(name)`: the flag's value, `None` when undefined.
    pub fn value(&self, name: &str) -> Option<FlagValue> {
        self.lookup(name).map(FlagValue::of)
    }

    /// On the command line, or found in the environment (even empty).
    pub fn is_set(&self, name: &str) -> bool {
        let key = name.as_bytes();
        // lookupFlagSet
        let Some(level) = self.levels.iter().rev().find(|l| l.set.slot(key).is_some()) else {
            return false;
        };
        let fs = &level.set;
        if fs.is_set(key) {
            return true;
        }
        // lookupFlag: the command flags of the lineage (the app's flags are the root's).
        let Some(f) = self
            .levels
            .iter()
            .rev()
            .flat_map(|l| l.flags.iter())
            .find(|f| f.names.iter().any(|n| n == name))
        else {
            return false;
        };
        if f.from_env {
            return true;
        }
        // now redo flagset search on aliases
        f.names.iter().any(|alias| fs.is_set(alias.as_bytes()))
    }

    pub fn args(&self) -> &[OsString] {
        self.levels.last().map_or(&[], |l| l.set.args())
    }

    pub fn narg(&self) -> usize {
        self.args().len()
    }

    /// "" when missing (Go `Args().Get`).
    pub fn arg(&self, n: usize) -> &OsStr {
        self.args()
            .get(n)
            .map_or(OsStr::new(""), OsString::as_os_str)
    }

    pub fn first(&self) -> &OsStr {
        self.arg(0)
    }

    /// `checkRequiredFlags(c.Flags)` of the current level: the error text when required flags are
    /// missing (`errors.go:59-66`).
    pub(crate) fn check_required_flags(&self) -> Option<String> {
        let level = self.levels.last()?;
        let mut missing: Vec<&str> = Vec::new();
        for f in level.flags.iter().filter(|f| f.required) {
            let flag_name = f.names.first().map_or("", String::as_str);
            let present = f
                .names
                .iter()
                .any(|key| self.is_set(&String::from_utf8_lossy(trim_space(key.as_bytes()))));
            if !present && !flag_name.is_empty() {
                missing.push(flag_name);
            }
        }
        match missing.as_slice() {
            [] => None,
            [one] => Some(format!("Required flag {} not set", quote(one.as_bytes()))),
            several => Some(format!(
                "Required flags {} not set",
                quote(several.join(", ").as_bytes())
            )),
        }
    }
}
