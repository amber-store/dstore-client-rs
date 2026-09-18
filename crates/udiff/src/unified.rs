//! `unified.go`: `Unified`, `toUnified`, `splitLines`, `addEqualLines`, `unified.String`.

use crate::Edit;

/// `udiff.Unified` (3 context lines).
pub fn unified(old_label: &[u8], new_label: &[u8], old: &[u8], new: &[u8]) -> Vec<u8> {
    todo!()
}

/// `udiff.ToUnified`.
pub fn to_unified(
    old_label: &[u8],
    new_label: &[u8],
    content: &[u8],
    edits: &[Edit],
    context_lines: usize,
) -> Result<Vec<u8>, String> {
    todo!()
}
