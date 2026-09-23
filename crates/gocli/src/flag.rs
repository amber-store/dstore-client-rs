//! Flag definitions and values (urfave `flag.go`, `flag_bool.go`, `flag_string.go`, `flag_int.go`,
//! `flag_int64.go`, `flag_uint.go`, `flag_float64.go`, `flag_duration.go`, `flag_string_slice.go`).

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;

use dstore_gocompat::quote::quote;
use dstore_gocompat::strconv::{self, format_float_g};
use dstore_gocompat::strings::trim_space;
use dstore_gocompat::time::duration_string;

use crate::goflag::{self, FlagSet, Value};

/// The urfave flag types dstore uses.
#[derive(Clone, Debug, PartialEq)]
pub enum FlagKind {
    String { default: &'static str },
    Bool { default: bool },
    Int { default: i64 },
    Int64 { default: i64 },
    Uint { default: u64 },
    Float64 { default: f64 },
    Duration { default_ns: i64 },
    StringSlice,
}

/// A flag definition.
#[derive(Clone, Debug, PartialEq)]
pub struct FlagDef {
    pub name: &'static str,
    /// Only help/h, version/v and push's message/m.
    pub aliases: &'static [&'static str],
    pub kind: FlagKind,
    pub usage: &'static str,
    pub env: &'static [&'static str],
    pub required: bool,
    /// Help and version flags.
    pub disable_default_text: bool,
}

/// A parsed flag value.
#[derive(Clone, Debug, PartialEq)]
pub enum FlagValue {
    Str(std::ffi::OsString),
    Bool(bool),
    I64(i64),
    U64(u64),
    F64(f64),
    DurationNs(i64),
    Slice(Vec<String>),
}

impl FlagValue {
    /// `flag.Getter.Get()` of a destination.
    pub(crate) fn of(v: &Value) -> FlagValue {
        match v {
            Value::String(s) => FlagValue::Str(OsString::from_vec(s.clone())),
            Value::Bool(b) => FlagValue::Bool(*b),
            Value::Int(n) | Value::Int64(n) => FlagValue::I64(*n),
            Value::Uint(n) => FlagValue::U64(*n),
            Value::Float64(f) => FlagValue::F64(*f),
            Value::Duration(ns) => FlagValue::DurationNs(*ns),
            Value::StringSlice { items, .. } => FlagValue::Slice(
                items
                    .iter()
                    .map(|i| String::from_utf8_lossy(i).into_owned())
                    .collect(),
            ),
        }
    }
}

/// urfave `HelpFlag`.
pub(crate) static HELP_FLAG: FlagDef = FlagDef {
    name: "help",
    aliases: &["h"],
    kind: FlagKind::Bool { default: false },
    usage: "show help",
    env: &[],
    required: false,
    disable_default_text: true,
};

/// urfave `VersionFlag`.
pub(crate) static VERSION_FLAG: FlagDef = FlagDef {
    name: "version",
    aliases: &["v"],
    kind: FlagKind::Bool { default: false },
    usage: "print the version",
    env: &[],
    required: false,
    disable_default_text: true,
};

/// A flag as applied to one command level: its names, whether its environment variable was found
/// (`HasBeenSet`), and whether it is required.
#[derive(Clone, Debug)]
pub(crate) struct FlagState {
    pub(crate) names: Vec<String>,
    pub(crate) from_env: bool,
    pub(crate) required: bool,
}

/// An `Apply` failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ApplyError {
    /// An environment value that does not parse: a usage error.
    Usage(Vec<u8>),
    /// Where Go's `FlagSet.Var` panics (a redefined or malformed name): a defect of the command table.
    Define(String),
}

impl FlagDef {
    /// urfave `FlagNames(Name, Aliases)`: the name, then the aliases, each with everything from its first
    /// `,` or space up to the end of the line removed (the v1 → v2 migration rule).
    pub fn names(&self) -> Vec<String> {
        std::iter::once(self.name)
            .chain(self.aliases.iter().copied())
            .map(strip_v1_name)
            .collect()
    }

    /// `TakesValue()`.
    pub(crate) fn takes_value(&self) -> bool {
        !matches!(self.kind, FlagKind::Bool { .. })
    }

    /// `GetDefaultText()`: the definition default (never the environment value).
    pub(crate) fn default_text(&self) -> String {
        match &self.kind {
            FlagKind::String { default } => {
                if default.is_empty() {
                    String::new()
                } else {
                    quote(default.as_bytes())
                }
            }
            FlagKind::Bool { default } => default.to_string(),
            FlagKind::Int { default } | FlagKind::Int64 { default } => default.to_string(),
            FlagKind::Uint { default } => default.to_string(),
            FlagKind::Float64 { default } => format_float_g(*default),
            FlagKind::Duration { default_ns } => duration_string(*default_ns),
            FlagKind::StringSlice => String::new(),
        }
    }

    /// The destination before the environment is applied.
    fn initial_value(&self) -> Value {
        match &self.kind {
            FlagKind::String { default } => Value::String(default.as_bytes().to_vec()),
            FlagKind::Bool { default } => Value::Bool(*default),
            FlagKind::Int { default } => Value::Int(*default),
            FlagKind::Int64 { default } => Value::Int64(*default),
            FlagKind::Uint { default } => Value::Uint(*default),
            FlagKind::Float64 { default } => Value::Float64(*default),
            FlagKind::Duration { default_ns } => Value::Duration(*default_ns),
            FlagKind::StringSlice => Value::StringSlice {
                items: Vec::new(),
                has_been_set: false,
            },
        }
    }

    /// `Apply(set)`: apply the environment (cli.md §2.2.5), then define every name over one destination.
    pub(crate) fn apply(
        &self,
        set: &mut FlagSet,
        getenv: &dyn Fn(&str) -> Option<OsString>,
    ) -> Result<FlagState, ApplyError> {
        let mut value = self.initial_value();
        let mut from_env = false;
        if let Some((val, source)) = flag_from_env(self.env, getenv) {
            match &self.kind {
                FlagKind::String { .. } => {
                    value = Value::String(val);
                    from_env = true;
                }
                FlagKind::Bool { .. } => {
                    if val.is_empty() {
                        // the variable is defined but empty: false
                        value = Value::Bool(false);
                    } else {
                        match strconv::parse_bool(&goflag::numeric_probe(&val)) {
                            Ok(b) => value = Value::Bool(b),
                            Err(e) => {
                                let text = goflag::num_error_text(e.func, &val, e.kind);
                                return Err(self.env_error(&val, "bool", &source, &text));
                            }
                        }
                    }
                    from_env = true;
                }
                FlagKind::Int { .. } | FlagKind::Int64 { .. } => {
                    if !val.is_empty() {
                        match strconv::parse_int(&goflag::numeric_probe(&val), 0, 64) {
                            Ok(n) => {
                                value = if matches!(self.kind, FlagKind::Int { .. }) {
                                    Value::Int(n)
                                } else {
                                    Value::Int64(n)
                                };
                                from_env = true;
                            }
                            Err(e) => {
                                let text = goflag::num_error_text(e.func, &val, e.kind);
                                return Err(self.env_error(&val, "int", &source, &text));
                            }
                        }
                    }
                }
                FlagKind::Uint { .. } => {
                    if !val.is_empty() {
                        match strconv::parse_uint(&goflag::numeric_probe(&val), 0, 64) {
                            Ok(n) => {
                                value = Value::Uint(n);
                                from_env = true;
                            }
                            Err(e) => {
                                let text = goflag::num_error_text(e.func, &val, e.kind);
                                return Err(self.env_error(&val, "uint", &source, &text));
                            }
                        }
                    }
                }
                FlagKind::Float64 { .. } => {
                    if !val.is_empty() {
                        match strconv::parse_float(&goflag::numeric_probe(&val)) {
                            Ok(f) => {
                                value = Value::Float64(f);
                                from_env = true;
                            }
                            Err(e) => {
                                let text = goflag::num_error_text(e.func, &val, e.kind);
                                return Err(self.env_error(&val, "float64", &source, &text));
                            }
                        }
                    }
                }
                FlagKind::Duration { .. } => {
                    if !val.is_empty() {
                        match goflag::parse_duration(&val) {
                            Ok(ns) => {
                                value = Value::Duration(ns);
                                from_env = true;
                            }
                            Err(text) => {
                                return Err(self.env_error(&val, "duration", &source, &text));
                            }
                        }
                    }
                }
                FlagKind::StringSlice => {
                    // Each part is trimmed and Set; the first Set replaces the default, and
                    // hasBeenSet is cleared afterwards so a command-line value replaces the list.
                    let items = val
                        .split(|&b| b == b',')
                        .map(|part| trim_space(part).to_vec())
                        .collect();
                    value = Value::StringSlice {
                        items,
                        has_been_set: false,
                    };
                    from_env = true;
                }
            }
        }
        let slot = set.add_value(value);
        let names = self.names();
        for name in &names {
            set.define(name, slot).map_err(ApplyError::Define)?;
        }
        Ok(FlagState {
            names,
            from_env,
            required: self.required,
        })
    }

    /// `could not parse %q as <type> value from %s for flag %s: %s`.
    fn env_error(&self, val: &[u8], typ: &str, source: &str, err: &str) -> ApplyError {
        ApplyError::Usage(
            format!(
                "could not parse {} as {typ} value from {source} for flag {}: {err}",
                quote(val),
                self.name
            )
            .into_bytes(),
        )
    }
}

/// `flagFromEnvOrFile(EnvVars, "")`: the first variable found (an empty value counts), with
/// `environment variable %q`.
fn flag_from_env(
    env_vars: &[&str],
    getenv: &dyn Fn(&str) -> Option<OsString>,
) -> Option<(Vec<u8>, String)> {
    for env_var in env_vars {
        let name = String::from_utf8_lossy(trim_space(env_var.as_bytes())).into_owned();
        // syscall.Getenv finds no empty key and no key containing '='.
        if name.is_empty() || name.contains('=') {
            continue;
        }
        if let Some(v) = getenv(&name) {
            return Some((
                v.into_vec(),
                format!("environment variable {}", quote(name.as_bytes())),
            ));
        }
    }
    None
}

/// `commaWhitespace.ReplaceAllString(part, "")` with `[, ]+.*`: from each `,` or space to the end of its
/// line (`.` does not match a newline).
fn strip_v1_name(part: &str) -> String {
    let mut out = String::with_capacity(part.len());
    let mut cutting = false;
    for c in part.chars() {
        if cutting {
            if c == '\n' {
                cutting = false;
                out.push(c);
            }
        } else if c == ',' || c == ' ' {
            cutting = true;
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_the_v1_rule() {
        let f = FlagDef {
            name: "names, n",
            aliases: &["N", "x y"],
            kind: FlagKind::StringSlice,
            usage: "",
            env: &[],
            required: false,
            disable_default_text: false,
        };
        assert_eq!(f.names(), vec!["names", "N", "x"]);
        assert_eq!(strip_v1_name("a b\nc,d"), "a\nc");
        assert_eq!(HELP_FLAG.names(), vec!["help", "h"]);
        assert!(!HELP_FLAG.takes_value());
    }

    #[test]
    fn default_texts() {
        let mk = |kind| FlagDef {
            name: "x",
            aliases: &[],
            kind,
            usage: "",
            env: &[],
            required: false,
            disable_default_text: false,
        };
        assert_eq!(mk(FlagKind::String { default: "" }).default_text(), "");
        assert_eq!(
            mk(FlagKind::String { default: "2Gi" }).default_text(),
            "\"2Gi\""
        );
        assert_eq!(mk(FlagKind::Bool { default: true }).default_text(), "true");
        assert_eq!(mk(FlagKind::Int { default: 0 }).default_text(), "0");
        assert_eq!(mk(FlagKind::Int64 { default: -3 }).default_text(), "-3");
        assert_eq!(mk(FlagKind::Uint { default: 41 }).default_text(), "41");
        assert_eq!(mk(FlagKind::Float64 { default: 0.0 }).default_text(), "0");
        assert_eq!(
            mk(FlagKind::Float64 { default: 1e21 }).default_text(),
            "1e+21"
        );
        assert_eq!(
            mk(FlagKind::Float64 {
                default: f64::INFINITY
            })
            .default_text(),
            "+Inf"
        );
        assert_eq!(
            mk(FlagKind::Duration {
                default_ns: 3_600_000_000_000
            })
            .default_text(),
            "1h0m0s"
        );
        assert_eq!(mk(FlagKind::StringSlice).default_text(), "");
    }
}
