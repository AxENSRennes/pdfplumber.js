//! Port of `pdfminer/runlength.py`: the `/RunLengthDecode` filter.

use pycompat::{ExcKind, PyErr, PyResult};

// pdfminer/runlength.py:9 rldecode
/// RunLength decoder (Adobe version), PDF Reference 1.4 section 3.3.4: a
/// length byte 0 to 127 copies the next length + 1 bytes, 129 to 255
/// repeats the next byte 257 - length times, 128 (or the end of the data)
/// ends the data.
///
/// The data running out inside a run is an error, as in Python: a literal
/// run reads its bytes in a generator expression, where the
/// `StopIteration` of `next()` becomes `RuntimeError("generator raised
/// StopIteration")` (PEP 479); a repeat run calls `next()` directly, and
/// the `StopIteration` itself escapes (a caller running inside a
/// generator would see it turned into that `RuntimeError` too).
pub fn rldecode(data: &[u8]) -> PyResult<Vec<u8>> {
    let mut decoded_array: Vec<u8> = Vec::new();
    let mut data_iter = data.iter().copied();
    loop {
        let length = data_iter.next().unwrap_or(128);
        if length == 128 {
            break;
        }
        if length < 128 {
            for _ in 0..=length {
                let Some(b) = data_iter.next() else {
                    return Err(PyErr::new(ExcKind::RuntimeError, "generator raised StopIteration"));
                };
                decoded_array.push(b);
            }
        }
        if length > 128 {
            let Some(b) = data_iter.next() else {
                return Err(PyErr::new(ExcKind::StopIteration, ""));
            };
            decoded_array.extend(std::iter::repeat_n(b, 257 - usize::from(length)));
        }
    }
    Ok(decoded_array)
}
