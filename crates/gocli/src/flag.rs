//! Flag definitions and values (urfave `flag*.go`).

/// The urfave flag types dstore uses.
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
pub struct FlagDef {
    pub name: &'static str,
    /// Only help/h, version/v.
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
