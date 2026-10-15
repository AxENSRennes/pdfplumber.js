//! Port of `pdfplumber/utils/generic.py`.

// pdfplumber/utils/generic.py:10 to_list
/// `to_list(collection)`: the objects as a `list`. A Rust slice is already
/// a sequence (the `DataFrame` branch has no counterpart), so this is a
/// copy of the (shared) objects.
pub fn to_list<T: Clone>(collection: &[T]) -> Vec<T> {
    collection.to_vec()
}
