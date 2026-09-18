//! dstore `refglob/refglob.go`, for the fake node's `watch` patterns.

/// A compiled reference-name glob.
pub struct Glob {
    pattern: String,
}

/// `refglob.Compile` with Go's error texts.
pub fn compile(pattern: &str) -> Result<Glob, String> {
    todo!()
}

impl Glob {
    pub fn matches(&self, name: &str) -> bool {
        todo!()
    }

    /// The literal prefix of the pattern.
    pub fn prefix(&self) -> String {
        todo!()
    }
}
