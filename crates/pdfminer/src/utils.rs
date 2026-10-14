//! Port of `pdfminer/utils.py` (partial).
//!
//! Ported: `choplist`. Not ported yet: everything else (predictors, matrix
//! helpers, `Plane`, `decode_text`, number formatting, ...).

// pdfminer/utils.py:402 choplist
/// Groups every `n` elements of the list; a trailing incomplete group is
/// dropped, like the Python generator.
pub fn choplist<T>(n: usize, seq: &[T]) -> std::slice::ChunksExact<'_, T> {
    seq.chunks_exact(n)
}
