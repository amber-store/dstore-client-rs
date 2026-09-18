//! go-udiff v0.4.1, the Unified path only (`unified.go`, `diff.go`, `ndiff.go` `Lines`, `lcs/old.go`
//! `twosided` with limit 50, `lcs/common.go`, `lcs/labels.go`, `lcs/sequence.go`), plus Go 1.26.5
//! `sort` (`pdqsort_func`, `sort.Stable`). Ported line by line, with no external dependencies: Go's
//! unstable pdqsort order changes heavy random edits.
//!
//! Spec: PORTING.md §4.9; port-notes/worktree.md §3.9.
#![allow(dead_code, unused_variables)] // L0 stubs: remove once the modules are implemented
#![deny(unsafe_op_in_unsafe_fn)]

mod diff;
pub mod gosort;
pub mod lcs;
mod unified;

pub use diff::*;
pub use unified::*;
