//! Port of `pdfminer/utils.py` (partial).
//!
//! Ported: `choplist`, the predictors (`paeth_predictor`,
//! `apply_tiff_predictor`, `apply_png_predictor`). Not ported yet:
//! everything else (matrix helpers, `Plane`, `decode_text`, number
//! formatting, ...).

use pycompat::{exc, PyErr, PyInt, PyResult};

use crate::pdftypes::PDFObject;

// pdfminer/utils.py:402 choplist
/// Groups every `n` elements of the list; a trailing incomplete group is
/// dropped, like the Python generator.
pub fn choplist<T>(n: usize, seq: &[T]) -> std::slice::ChunksExact<'_, T> {
    seq.chunks_exact(n)
}

// ---------------------------------------------------------------------------
// predictors
//
// The parameters come from the stream's `/DecodeParms` through
// `int_value`: Python ints of any size and sign, or bools. The code below
// follows the Python arithmetic and indexing exactly, including its errors
// (`range()` with a zero step, `IndexError` on short rows or when the
// previous row is shorter than this one). Where Python loops up to a
// number larger than the data, the port stops where Python must fail or
// end (an index past the data), so the numbers are clamped to the data
// length first; that changes no result.
//
// A large positive `/Columns` makes `list(b"\x00" * columns)` allocate that
// many bytes (and 8 times more for the list) in Python, which fails with
// `MemoryError` past the machine's memory. The port only materializes the
// part that can be read, and raises `MemoryError` from `/Columns` 2**32 on
// (`MEMORY_LIMIT`): what the oracle machine does (see there). Below, the
// outcome depends on the machine's memory and the port goes on.

/// `x` as a Python `int` or `bool` (`int_value`'s result): its value, and
/// `str(x)` for messages.
fn int_of(x: &PDFObject) -> PyInt {
    match x {
        PDFObject::Int(i) => i.clone(),
        PDFObject::Bool(b) => PyInt::Small(i64::from(*b)),
        other => unreachable!("int_value returns an int or a bool, not {}", other.type_name()),
    }
}

/// `str(x)` of an int or a bool.
fn str_of(x: &PDFObject) -> PyResult<String> {
    match x {
        PDFObject::Bool(b) => Ok(if *b { "True" } else { "False" }.to_owned()),
        other => int_of(other).py_str(),
    }
}

/// `x` clamped to `lo..=hi` (exact for the values in between).
fn clamp(x: &PyInt, lo: i64, hi: i64) -> i64 {
    match x.as_i64() {
        Some(v) => v.clamp(lo, hi),
        None if x.is_negative() => lo,
        None => hi,
    }
}

/// The `IndexError` of `bytes[i]`.
fn bytes_index_error() -> PyErr {
    exc::IndexError("index out of range")
}

/// The `IndexError` of `list[i]`.
fn list_index_error() -> PyErr {
    exc::IndexError("list index out of range")
}

/// The `IndexError` of `list[i - bpp]` past the end: an index that does
/// not fit in a `Py_ssize_t` fails before the bound check.
fn list_index_error_at(i: i64, bpp: &PyInt) -> PyErr {
    if PyInt::Small(i).sub(bpp).as_i64().is_none() {
        exc::IndexError("cannot fit 'int' into an index-sized integer")
    } else {
        list_index_error()
    }
}

/// The smallest `/Columns` for which `list(b"\x00" * columns)` raises
/// `MemoryError` on the oracle machine, by its profile (64-bit CPython 3.12,
/// Linux with heuristic overcommit, 24 GB of RAM and 4 GB of swap):
/// the list of 2**32 pointers is 32 GiB, more than RAM and swap together,
/// so its single allocation is refused at once (from 2**35 on, the bytes
/// object itself is). Just below, Python fails or not depending on the
/// machine's memory (from about 3.6e9 the list is refused on the oracle
/// machine too); the port does not allocate (see above) and goes on. A
/// wasm32 host cannot address 2**32 bytes either.
const MEMORY_LIMIT: i64 = 1 << 32;

/// A safety net for mutation testing, active with debug assertions only
/// (and excluded from the mutants, `-E size_guard`): a predictor never
/// outputs more bytes than it reads, so a broken row loop that would fill
/// the memory panics at once.
#[inline]
fn size_guard(output: usize, data: usize) {
    debug_assert!(output <= data, "a predictor writes more than it reads");
}

/// The `ValueError` of `range(start, stop, 0)`.
fn range_step_zero() -> PyErr {
    exc::ValueError("range() arg 3 must not be zero")
}

// pdfminer/utils.py:107 paeth_predictor
pub fn paeth_predictor(left: i64, above: i64, upper_left: i64) -> i64 {
    // From http://www.libpng.org/pub/png/spec/1.2/PNG-Filters.html
    // Initial estimate
    let p = left + above - upper_left;
    // Distances to a,b,c
    let pa = (p - left).abs();
    let pb = (p - above).abs();
    let pc = (p - upper_left).abs();
    // Return nearest of a,b,c breaking ties in order a,b,c
    if pa <= pb && pa <= pc {
        left
    } else if pb <= pc {
        above
    } else {
        upper_left
    }
}

// pdfminer/utils.py:125 apply_tiff_predictor
/// Reverse the effect of the TIFF predictor 2. `bitspercomponent` is an
/// int or a bool (only 8 is supported).
pub fn apply_tiff_predictor(
    colors: &PyInt,
    columns: &PyInt,
    bitspercomponent: &PDFObject,
    data: &[u8],
) -> PyResult<Vec<u8>> {
    let bpc = int_of(bitspercomponent);
    if bpc != PyInt::Small(8) {
        let error_msg = format!("Unsupported `bitspercomponent': {}", str_of(bitspercomponent)?);
        return Err(exc::PDFValueError(error_msg));
    }
    // bpp = colors * (bitspercomponent // 8), with bitspercomponent == 8
    let bpp = colors.clone();
    let nbytes = columns.mul(&bpp);
    if nbytes.is_zero() {
        return Err(range_step_zero());
    }
    let len = data.len() as i64;
    let bpp_int = bpp;
    // past the data, a row fails or the loop ends: clamping is exact
    let nbytes = clamp(&nbytes, -1, len + 1);
    let bpp = clamp(&bpp_int, -1, len + 1);
    let mut buf: Vec<u8> = Vec::with_capacity(data.len());
    if nbytes < 0 {
        return Ok(buf); // an empty range
    }
    let mut scanline_i = 0;
    while scanline_i < len {
        let mut raw: Vec<u8> = Vec::new();
        for i in 0..nbytes {
            let Some(&byte) = data.get((scanline_i + i) as usize) else {
                return Err(bytes_index_error());
            };
            let mut new_value = i64::from(byte);
            if i >= bpp {
                // raw[i - bpp]: i - bpp >= 0, and < len(raw) == i only if bpp > 0
                let Some(&r) = raw.get((i - bpp) as usize) else {
                    return Err(list_index_error_at(i, &bpp_int));
                };
                new_value += i64::from(r);
                new_value %= 256;
            }
            raw.push(new_value as u8);
        }
        buf.extend_from_slice(&raw);
        size_guard(buf.len(), data.len());
        scanline_i += nbytes;
    }
    Ok(buf)
}

// pdfminer/utils.py:153 apply_png_predictor
/// Reverse the effect of the PNG predictor. `bitspercomponent` is an int
/// or a bool (8 and 1 are supported; `True` is 1). `pred` is not used.
pub fn apply_png_predictor(
    _pred: &PyInt,
    colors: &PyInt,
    columns: &PyInt,
    bitspercomponent: &PDFObject,
    data: &[u8],
) -> PyResult<Vec<u8>> {
    let bpc = int_of(bitspercomponent);
    if bpc != PyInt::Small(8) && bpc != PyInt::Small(1) {
        let msg = format!("Unsupported `bitspercomponent': {}", str_of(bitspercomponent)?);
        return Err(exc::PDFValueError(msg));
    }
    let eight = PyInt::Small(8);
    let nbytes = colors.mul(columns).mul(&bpc).floordiv(&eight)?;
    // number of bytes per complete pixel
    let bpp = colors.mul(&bpc).floordiv(&eight)?;
    // line_above = list(b"\x00" * columns): only indexes below len(data) are
    // ever read
    let Some(columns) = columns.as_i64() else {
        return Err(exc::OverflowError("cannot fit 'int' into an index-sized integer"));
    };
    if columns >= MEMORY_LIMIT {
        return Err(exc::MemoryError(""));
    }
    let len = data.len() as i64;
    let mut line_above: Vec<u8> = vec![0; columns.clamp(0, len) as usize];
    let step = nbytes.add(&PyInt::Small(1));
    if step.is_zero() {
        return Err(range_step_zero());
    }
    let step = clamp(&step, -1, len + 1);
    let nbytes = clamp(&nbytes, -1, len + 1);
    let bpp_int = bpp;
    let bpp = clamp(&bpp_int, -(len + 1), len + 1);
    let mut buf: Vec<u8> = Vec::with_capacity(data.len());
    if step < 0 {
        return Ok(buf); // an empty range
    }
    let mut scanline_i = 0;
    while scanline_i < len {
        let start = scanline_i as usize;
        let filter_type = data[start];
        let line_encoded = &data[(start + 1).min(data.len())..(start as i64 + 1 + nbytes).min(len) as usize];
        let mut raw: Vec<u8> = Vec::with_capacity(line_encoded.len());
        // raw[j - bpp] when j - bpp >= 0 (else 0)
        let raw_x_bpp = |raw: &[u8], j: i64| -> PyResult<i64> {
            if j - bpp < 0 {
                Ok(0)
            } else {
                raw.get((j - bpp) as usize).map(|&r| i64::from(r)).ok_or_else(|| list_index_error_at(j, &bpp_int))
            }
        };
        // line_above[j] and line_above[j - bpp] (j - bpp >= 0)
        let above = |line_above: &[u8], j: i64, minus_bpp: bool| -> PyResult<i64> {
            let k = if minus_bpp { j - bpp } else { j };
            line_above.get(k as usize).map(|&r| i64::from(r)).ok_or_else(|| {
                if minus_bpp {
                    list_index_error_at(j, &bpp_int)
                } else {
                    list_index_error()
                }
            })
        };
        match filter_type {
            // Filter type 0: None
            0 => raw.extend_from_slice(line_encoded),
            // Filter type 1: Sub
            1 => {
                for (j, &sub_x) in line_encoded.iter().enumerate() {
                    let raw_x_bpp = raw_x_bpp(&raw, j as i64)?;
                    raw.push(((i64::from(sub_x) + raw_x_bpp) & 255) as u8);
                }
            }
            // Filter type 2: Up
            2 => {
                for (&up_x, &prior_x) in line_encoded.iter().zip(&line_above) {
                    raw.push(((u32::from(up_x) + u32::from(prior_x)) & 255) as u8);
                }
            }
            // Filter type 3: Average
            3 => {
                for (j, &average_x) in line_encoded.iter().enumerate() {
                    let raw_x_bpp = raw_x_bpp(&raw, j as i64)?;
                    let prior_x = above(&line_above, j as i64, false)?;
                    raw.push(((i64::from(average_x) + (raw_x_bpp + prior_x) / 2) & 255) as u8);
                }
            }
            // Filter type 4: Paeth
            4 => {
                for (j, &paeth_x) in line_encoded.iter().enumerate() {
                    let j = j as i64;
                    let (raw_x_bpp, prior_x_bpp) =
                        if j - bpp < 0 { (0, 0) } else { (raw_x_bpp(&raw, j)?, above(&line_above, j, true)?) };
                    let prior_x = above(&line_above, j, false)?;
                    let paeth = paeth_predictor(raw_x_bpp, prior_x, prior_x_bpp);
                    raw.push(((i64::from(paeth_x) + paeth) & 255) as u8);
                }
            }
            _ => return Err(exc::PDFValueError(format!("Unsupported predictor value: {filter_type}"))),
        }
        buf.extend_from_slice(&raw);
        size_guard(buf.len(), data.len());
        line_above = raw;
        scanline_i += step;
    }
    Ok(buf)
}
