//! go-udiff v0.4.1, the Unified path only (`unified.go`, `diff.go`, `ndiff.go` `Lines`, `lcs/old.go`
//! `twosided` with limit 50, `lcs/common.go`, `lcs/labels.go`, `lcs/sequence.go`), plus Go 1.26.5
//! `sort` (`pdqsort_func`, `sort.Stable`). Ported line by line, with no external dependencies: Go's
//! unstable pdqsort order changes heavy random edits.
//!
//! Go's `int` is `isize` inside the ports, so arithmetic that goes negative in Go (diagonals, loop
//! bounds, `strings.LastIndex` results) behaves the same; the public API uses `usize` offsets.
//!
//! Spec: PORTING.md §4.9; port-notes/worktree.md §3.9.
#![deny(unsafe_op_in_unsafe_fn)]

mod diff;
pub mod gosort;
pub mod lcs;
mod unified;

pub use diff::*;
pub use unified::*;
