//! `diff.go` (`Edit`, `validate`, `SortEdits`, `lineEdits`, `expandEdit`) and `ndiff.go` `Lines`.

/// `udiff.Edit`: replace `content[start..end]` with `new`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub start: usize,
    pub end: usize,
    pub new: Vec<u8>,
}

/// `udiff.Lines`.
pub fn lines(before: &[u8], after: &[u8]) -> Vec<Edit> {
    todo!()
}
