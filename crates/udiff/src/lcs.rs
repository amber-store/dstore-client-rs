//! go-udiff `lcs`: `old.go` `twosided` (limit 50; `backward` skipped), `common.go`, `labels.go`,
//! `sequence.go`.

/// `lcs.Diff`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Diff {
    pub start: usize,
    pub end: usize,
    pub repl_start: usize,
    pub repl_end: usize,
}

/// `lcs.DiffLines`.
pub fn diff_lines(a: &[&[u8]], b: &[&[u8]]) -> Vec<Diff> {
    todo!()
}
