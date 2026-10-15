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

// ---------------------------------------------------------------------------
// unpad_aes

// pdfminer/utils.py:858 unpad_aes
/// Remove block padding as described in PDF 1.7 section 7.6.2:
///
/// > For an original message length of M, the pad shall consist of 16 -
/// > (M mod 16) bytes whose value shall also be 16 - (M mod 16).
/// > Note that the pad is present when M is evenly divisible by 16;
/// > it contains 16 bytes of 0x10.
///
/// As upstream, a last byte of 0 is a "padding" of the whole input
/// (`padded[-0:]`): an input of zeros becomes `b""`. Takes the frames of the
/// function and of the generator expression `all()` runs.
pub fn unpad_aes(padded: &[u8]) -> pycompat::PyResult<&[u8]> {
    let _frame = pycompat::recursion::py_frame()?;
    let Some(&padding) = padded.last() else {
        return Ok(padded);
    };
    // Check for a potential padding byte (bytes are unsigned)
    let padding = usize::from(padding);
    if padding > 16 {
        return Ok(padded);
    }
    // A valid padding byte is the length of the padding
    if padding > padded.len() {
        // Obviously invalid
        return Ok(padded);
    }
    // Every byte of padding is equal to the length of padding
    let end = padded.len() - padding;
    // padded[-padding:] (all of it when padding is 0)
    let tail = if padding == 0 { padded } else { &padded[end..] };
    let all = {
        let _genexpr = pycompat::recursion::py_frame()?;
        tail.iter().all(|&x| usize::from(x) == padding)
    };
    if all {
        // padded[:-padding] (empty when padding is 0)
        return Ok(if padding == 0 { &padded[..0] } else { &padded[..end] });
    }
    Ok(padded)
}
