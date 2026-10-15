//! Port of `pdfminer/utils.py`.
//!
//! The file is shared by several ports, each in its own marked section, in
//! the order of `utils.py` (a section holds a group of related functions;
//! within a group the upstream order is kept where it can be):
//!
//! - "Predictors" (`paeth_predictor`, `apply_tiff_predictor`,
//!   `apply_png_predictor`): written by the port of the stream filters;
//! - "Matrices and geometry": `INF`, `Point`/`Rect`/`Matrix`/`PathSegment`,
//!   `MATRIX_IDENTITY`, `parse_rect`, `mult_matrix`, `translate_matrix`,
//!   `apply_matrix_pt`, `apply_matrix_rect`, `apply_matrix_norm`,
//!   `get_bound`, `drange`, `bbox2str`, `matrix2str`, and `Plane`;
//! - "Iteration helpers": `choplist`, `uniq`, `fsplit`, `pick`;
//! - "Numbers and text": `isnumber`, `nunpack`, `PDFDocEncoding`,
//!   `decode_text`, `format_int_roman`, `format_int_alpha`;
//! - "AES padding" (`unpad_aes`): written by the port of the security
//!   handlers.
//!
//! Not ported, because nothing on pdfplumber's path uses them:
//! `open_filename` (files are bytes here), `make_compat_bytes`,
//! `make_compat_str` (and its `charset_normalizer` guess),
//! `compatible_encode_method` and `enc` (the text/HTML/XML converters and
//! `TagExtractor`, out of scope), `shorten_str` (no caller) and
//! `vecBetweenBoxes` (no caller).
//!
//! # Numbers
//!
//! Python's `int` and `float` stay apart where they can meet: a matrix
//! built from `MATRIX_IDENTITY` and integral operands is made of ints, a
//! bounding box of no point is `(INF, INF, -INF, -INF)` (ints), and
//! `float` arithmetic on a huge int raises `OverflowError`. The geometry
//! functions therefore take and return [`Num`] with Python's arithmetic
//! (`pycompat::num`), and return `PyResult` for that `OverflowError`.

use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::ops::Range;

use pycompat::float::float_to_int;
use pycompat::{exc, Num, PyErr, PyInt, PyResult};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::casting::IterItem;
use crate::pdftypes::PDFObject;

// ===========================================================================
// Predictors (paeth_predictor, apply_tiff_predictor, apply_png_predictor):
// written by the port of the stream filters.
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

// ===========================================================================
// Matrices and geometry

// pdfminer/utils.py:30 INF
/// `INF = (1 << 31) - 1`, an int: PDF still uses 32 bits ints.
pub const INF: i64 = (1 << 31) - 1;

/// `Num::Int(INF)`
pub fn inf() -> Num {
    Num::Int(PyInt::Small(INF))
}

/// `-INF`
pub fn neg_inf() -> Num {
    Num::Int(PyInt::Small(-INF))
}

/// pdfminer/utils.py:248 `Point`: `(x, y)`.
pub type Point = [Num; 2];
/// pdfminer/utils.py:249 `Rect`: `(x0, y0, x1, y1)`.
pub type Rect = [Num; 4];
/// pdfminer/utils.py:250 `Matrix`: `(a, b, c, d, e, f)`.
pub type Matrix = [Num; 6];

/// pdfminer/utils.py:251 `PathSegment`: a path operator (`"m"`, `"l"`,
/// `"c"`, `"v"`, `"y"`, `"h"`) and its coordinates, the tuple
/// `(op, x1, y1, ...)`.
pub type PathSegment = (char, Vec<Num>);

// pdfminer/utils.py:259 MATRIX_IDENTITY
/// `(1, 0, 0, 1, 0, 0)` (ints).
pub fn matrix_identity() -> Matrix {
    let i = |v: i64| Num::Int(PyInt::Small(v));
    [i(1), i(0), i(0), i(1), i(0), i(0)]
}

/// `min(a, b)`: `a` unless `b < a` (the first of equal values; a NaN `b`
/// never wins, a NaN `a` is never replaced).
pub fn py_min2(a: &Num, b: &Num) -> Num {
    if b.py_lt(a) {
        b.clone()
    } else {
        a.clone()
    }
}

/// `max(a, b)`: `a` unless `b > a`.
pub fn py_max2(a: &Num, b: &Num) -> Num {
    if b.py_gt(a) {
        b.clone()
    } else {
        a.clone()
    }
}

/// `min(items)` of several numbers (Python keeps the first of equal ones).
fn py_min_of(items: &[&Num]) -> Num {
    let mut best = items[0];
    for x in &items[1..] {
        if x.py_lt(best) {
            best = x;
        }
    }
    best.clone()
}

/// `max(items)` of several numbers.
fn py_max_of(items: &[&Num]) -> Num {
    let mut best = items[0];
    for x in &items[1..] {
        if x.py_gt(best) {
            best = x;
        }
    }
    best.clone()
}

// pdfminer/utils.py:262 parse_rect
/// `parse_rect(o)`: unpacks four items of the iterable `o` (pulled as
/// Python's unpacking pulls them: up to a fifth one to report "too many
/// values"; an error of the iterator propagates) and converts each with
/// `float()`. A wrong count or a `ValueError` of `float()` becomes
/// `PDFValueError("Could not parse rectangle")`; other errors (a
/// `TypeError` of `float(None)`, an `OverflowError`) propagate. The items
/// are whatever iterating the caller's object yields ([`IterItem`]):
/// `pdfpage` iterates a resolved `/MediaBox` or `/CropBox` of any type, and
/// a dict yields its `str` keys, which `float()` reads as a `str` (Unicode
/// digits and spaces count), not as bytes.
pub fn parse_rect(o: impl IntoIterator<Item = PyResult<IterItem>>) -> PyResult<[f64; 4]> {
    let attempt = || -> PyResult<[f64; 4]> {
        let mut it = o.into_iter();
        let mut items = Vec::with_capacity(4);
        while items.len() < 4 {
            match it.next() {
                Some(x) => items.push(x?),
                None => {
                    return Err(exc::ValueError(format!(
                        "not enough values to unpack (expected 4, got {})",
                        items.len()
                    )))
                }
            }
        }
        if let Some(x) = it.next() {
            x?;
            return Err(exc::ValueError("too many values to unpack (expected 4)"));
        }
        Ok([items[0].py_float()?, items[1].py_float()?, items[2].py_float()?, items[3].py_float()?])
    };
    attempt().map_err(|e| {
        if e.is_instance(pycompat::ExcKind::ValueError) {
            exc::PDFValueError("Could not parse rectangle")
        } else {
            e
        }
    })
}

// pdfminer/utils.py:270 mult_matrix
/// Returns the multiplication of two matrices.
pub fn mult_matrix(m1: &Matrix, m0: &Matrix) -> PyResult<Matrix> {
    let [a1, b1, c1, d1, e1, f1] = m1;
    let [a0, b0, c0, d0, e0, f0] = m0;
    Ok([
        a0.mul(a1)?.add(&c0.mul(b1)?)?,
        b0.mul(a1)?.add(&d0.mul(b1)?)?,
        a0.mul(c1)?.add(&c0.mul(d1)?)?,
        b0.mul(c1)?.add(&d0.mul(d1)?)?,
        a0.mul(e1)?.add(&c0.mul(f1)?)?.add(e0)?,
        b0.mul(e1)?.add(&d0.mul(f1)?)?.add(f0)?,
    ])
}

// ---------------------------------------------------------------------------
// Matrices of unchecked PDF objects

/// The kind of a sequence [`Operand`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeqKind {
    Bytes,
    List,
}

/// A value of the matrix arithmetic upstream runs on PDF objects it never
/// checked: `pdfinterp.do_Do` passes `list_value(xobj["BBox"])` and
/// `list_value(xobj.get("Matrix", MATRIX_IDENTITY))` as they are to
/// `PDFLayoutAnalyzer.begin_figure`, which computes
/// `LTFigure(name, bbox, mult_matrix(matrix, self.ctm))`
/// (pdfminer/converter.py:89 `PDFLayoutAnalyzer.begin_figure`). Python's
/// operators apply: a `bool` is an
/// `int` (but named `bool` in messages), a `bytes` or a `list` is
/// repeated by an int and concatenated to its own type, anything else
/// raises the `TypeError` of an unsupported operand.
///
/// A sequence keeps its type and length only: its contents could matter to
/// a comparison (`min`/`max` of `apply_matrix_rect`), which these paths
/// never reach with a sequence ([`ObjMatrix`]). The length is what
/// CPython's explicit size checks see (`OverflowError` for a count beyond
/// `Py_ssize_t` or repeated bytes too long, `MemoryError` for a list
/// beyond `PY_SSIZE_T_MAX` pointers); a repetition that passes them is
/// never built here. Python builds it, and raises `MemoryError` if the
/// machine cannot allocate it, which depends on the machine: the port
/// behaves as Python with enough memory.
#[derive(Clone, Debug)]
pub enum Operand {
    /// An `int` or a `float`.
    Num(Num),
    /// A `bool`.
    Bool(bool),
    /// A `bytes` or a `list`, and its length.
    Seq(SeqKind, u64),
    /// Any other object, by its type name (`NoneType`, `PSLiteral`,
    /// `dict`...).
    Other(&'static str),
}

/// `PY_SSIZE_T_MAX`
const PY_SSIZE_T_MAX: u64 = i64::MAX as u64;

/// `PyBytesObject_SIZE` (64-bit): the header of a bytes object and its
/// trailing NUL.
const PY_BYTES_OBJECT_SIZE: u64 = 33;

impl From<Num> for Operand {
    fn from(n: Num) -> Operand {
        Operand::Num(n)
    }
}

impl Operand {
    /// The operand of a PDF object.
    pub fn from_obj(o: &PDFObject) -> Operand {
        match o {
            PDFObject::Bool(b) => Operand::Bool(*b),
            PDFObject::Int(i) => Operand::Num(Num::Int(i.clone())),
            PDFObject::Float(f) => Operand::Num(Num::Float(*f)),
            PDFObject::Bytes(b) => Operand::Seq(SeqKind::Bytes, b.len() as u64),
            PDFObject::List(l) => Operand::Seq(SeqKind::List, l.len() as u64),
            other => Operand::Other(other.type_name()),
        }
    }

    /// `type(x).__name__`
    pub fn type_name(&self) -> &'static str {
        match self {
            Operand::Num(Num::Int(_)) => "int",
            Operand::Num(Num::Float(_)) => "float",
            Operand::Bool(_) => "bool",
            Operand::Seq(SeqKind::Bytes, _) => "bytes",
            Operand::Seq(SeqKind::List, _) => "list",
            Operand::Other(name) => name,
        }
    }

    /// The number of an `int`, a `float` or a `bool` (its int).
    pub fn as_num(&self) -> Option<Num> {
        match self {
            Operand::Num(n) => Some(n.clone()),
            Operand::Bool(b) => Some(Num::Int(PyInt::Small(i64::from(*b)))),
            _ => None,
        }
    }

    /// `x * y` (`PyNumber_Multiply`): numbers multiply; otherwise the left
    /// operand if it is a sequence, else the right one, is repeated by the
    /// other (`sequence_repeat`), which must be an int.
    pub fn mul(&self, other: &Operand) -> PyResult<Operand> {
        if let (Some(x), Some(y)) = (self.as_num(), other.as_num()) {
            return x.mul(&y).map(Operand::Num);
        }
        let (kind, len, count) = match (self, other) {
            (Operand::Seq(kind, len), count) | (count, Operand::Seq(kind, len)) => (*kind, *len, count),
            _ => {
                return Err(exc::TypeError(format!(
                    "unsupported operand type(s) for *: '{}' and '{}'",
                    self.type_name(),
                    other.type_name()
                )))
            }
        };
        let count = match count {
            Operand::Num(Num::Int(i)) => i.clone(),
            Operand::Bool(b) => PyInt::Small(i64::from(*b)),
            other => {
                return Err(exc::TypeError(format!(
                    "can't multiply sequence by non-int of type '{}'",
                    other.type_name()
                )))
            }
        };
        // PyNumber_AsSsize_t(count, PyExc_OverflowError); a negative count
        // repeats nothing
        let Some(count) = count.as_i64() else {
            return Err(exc::OverflowError("cannot fit 'int' into an index-sized integer"));
        };
        let count = count.max(0) as u64;
        match kind {
            // bytes_repeat
            SeqKind::Bytes => {
                if count > 0 && len > PY_SSIZE_T_MAX / count {
                    return Err(exc::OverflowError("repeated bytes are too long"));
                }
                Ok(Operand::Seq(kind, len * count))
            }
            // list_repeat, then list_new_prealloc (PyMem_New of the pointers)
            SeqKind::List => {
                if len == 0 || count == 0 {
                    return Ok(Operand::Seq(kind, 0));
                }
                if len > PY_SSIZE_T_MAX / count || len * count > PY_SSIZE_T_MAX / 8 {
                    return Err(exc::MemoryError(""));
                }
                Ok(Operand::Seq(kind, len * count))
            }
        }
    }

    /// `x + y` (`PyNumber_Add`): numbers add; otherwise the left operand's
    /// concatenation, if it is a sequence.
    pub fn add(&self, other: &Operand) -> PyResult<Operand> {
        if let (Some(x), Some(y)) = (self.as_num(), other.as_num()) {
            return x.add(&y).map(Operand::Num);
        }
        match (self, other) {
            // bytes_concat: an empty operand gives the other one back,
            // then PyBytes_FromStringAndSize of the total size
            (Operand::Seq(SeqKind::Bytes, a), Operand::Seq(SeqKind::Bytes, b)) => {
                if *a > 0 && *b > 0 {
                    if *a > PY_SSIZE_T_MAX - *b {
                        return Err(exc::MemoryError(""));
                    }
                    if a + b > PY_SSIZE_T_MAX - PY_BYTES_OBJECT_SIZE {
                        return Err(exc::OverflowError("byte string is too large"));
                    }
                }
                Ok(Operand::Seq(SeqKind::Bytes, a + b))
            }
            (Operand::Seq(SeqKind::Bytes, _), _) => {
                Err(exc::TypeError(format!("can't concat {} to bytes", other.type_name())))
            }
            // list_concat, then list_new_prealloc of the total size (which
            // cannot overflow: both lists exist)
            (Operand::Seq(SeqKind::List, a), Operand::Seq(SeqKind::List, b)) => {
                if a + b > PY_SSIZE_T_MAX / 8 {
                    return Err(exc::MemoryError(""));
                }
                Ok(Operand::Seq(SeqKind::List, a + b))
            }
            (Operand::Seq(SeqKind::List, _), _) => {
                Err(exc::TypeError(format!("can only concatenate list (not \"{}\") to list", other.type_name())))
            }
            _ => Err(exc::TypeError(format!(
                "unsupported operand type(s) for +: '{}' and '{}'",
                self.type_name(),
                other.type_name()
            ))),
        }
    }

    /// `x + y` where `y` is a number: a number, or the `TypeError` of `+`.
    fn add_num(&self, y: &Num) -> PyResult<Num> {
        match self.add(&Operand::Num(y.clone()))? {
            Operand::Num(n) => Ok(n),
            other => unreachable!("{other:?}: anything but a number plus a number raises"),
        }
    }
}

/// `(a, b, ...) = seq` of a sequence that must have `N` items: Python's
/// `ValueError` otherwise.
pub fn unpack<T: Clone, const N: usize>(seq: &[T]) -> PyResult<[T; N]> {
    match <&[T; N]>::try_from(seq) {
        Ok(items) => Ok(items.clone()),
        Err(_) if seq.len() < N => {
            Err(exc::ValueError(format!("not enough values to unpack (expected {N}, got {})", seq.len())))
        }
        Err(_) => Err(exc::ValueError(format!("too many values to unpack (expected {N})"))),
    }
}

/// The result of `mult_matrix(m1, m0)` for an unchecked `m1` and a CTM
/// `m0`: `a, b, c, d` are whatever the arithmetic gave (a `bytes` times an
/// int and plus a `bytes` stays a `bytes`), `e, f` are numbers, since they
/// add the CTM's translation, a number, last (`x + e0` raises unless `x` is
/// a number). So `apply_matrix_pt` of such a matrix gives numbers or
/// raises: each coordinate ends with `+ e` or `+ f`.
///
/// Every item of `m1` is multiplied by a number of the CTM, which gives a
/// sequence or raises unless the item is a number: so when
/// `LTFigure::new_obj` succeeds, every item of `m1` was a number, and the
/// interpreter's own `mult_matrix(matrix, self.ctm)` (pdfminer/pdfinterp.py
/// `do_Do`, after `begin_figure`) is a [`Matrix`] ([`Self::to_matrix`]).
#[derive(Clone, Debug)]
pub struct ObjMatrix {
    pub abcd: [Operand; 4],
    pub e: Num,
    pub f: Num,
}

impl ObjMatrix {
    /// The matrix, if `a, b, c, d` are numbers.
    pub fn to_matrix(&self) -> Option<Matrix> {
        let [a, b, c, d] = &self.abcd;
        Some([a.as_num()?, b.as_num()?, c.as_num()?, d.as_num()?, self.e.clone(), self.f.clone()])
    }

    // pdfminer/utils.py:295 apply_matrix_pt
    /// `apply_matrix_pt(self, (x, y))`: numbers, or Python's exception.
    pub fn apply_pt(&self, x: &Operand, y: &Operand) -> PyResult<Point> {
        let [a, b, c, d] = &self.abcd;
        let px = a.mul(x)?.add(&c.mul(y)?)?.add_num(&self.e)?;
        let py = b.mul(x)?.add(&d.mul(y)?)?.add_num(&self.f)?;
        Ok([px, py])
    }

    // pdfminer/utils.py:302 apply_matrix_rect
    /// `apply_matrix_rect(self, rect)`.
    pub fn apply_rect(&self, rect: &[Operand; 4]) -> PyResult<Rect> {
        let [x0, y0, x1, y1] = rect;
        let [left1, bottom1] = self.apply_pt(x0, y0)?;
        let [right1, bottom2] = self.apply_pt(x1, y0)?;
        let [right2, top1] = self.apply_pt(x1, y1)?;
        let [left2, top2] = self.apply_pt(x0, y1)?;
        Ok([
            py_min_of(&[&left1, &left2, &right1, &right2]),
            py_min_of(&[&bottom1, &bottom2, &top1, &top2]),
            py_max_of(&[&left1, &left2, &right1, &right2]),
            py_max_of(&[&bottom1, &bottom2, &top1, &top2]),
        ])
    }
}

// pdfminer/utils.py:270 mult_matrix
/// `mult_matrix(m1, m0)` where `m1` is the unchecked sequence of PDF objects
/// `list_value(xobj.get("Matrix", MATRIX_IDENTITY))` and `m0` the CTM, with
/// Python's unpacking and operators ([`Operand`]).
pub fn mult_matrix_obj(m1: &[PDFObject], m0: &Matrix) -> PyResult<ObjMatrix> {
    let [a1, b1, c1, d1, e1, f1] = unpack::<_, 6>(m1)?.map(|x| Operand::from_obj(&x));
    let [a0, b0, c0, d0, e0, f0] = m0;
    let [a0, b0, c0, d0] = [a0, b0, c0, d0].map(|x| Operand::Num(x.clone()));
    let a = a0.mul(&a1)?.add(&c0.mul(&b1)?)?;
    let b = b0.mul(&a1)?.add(&d0.mul(&b1)?)?;
    let c = a0.mul(&c1)?.add(&c0.mul(&d1)?)?;
    let d = b0.mul(&c1)?.add(&d0.mul(&d1)?)?;
    let e = a0.mul(&e1)?.add(&c0.mul(&f1)?)?.add_num(e0)?;
    let f = b0.mul(&e1)?.add(&d0.mul(&f1)?)?.add_num(f0)?;
    Ok(ObjMatrix { abcd: [a, b, c, d], e, f })
}

// pdfminer/utils.py:284 translate_matrix
/// Translates a matrix by `(x, y)` inside the projection: its origin moves
/// to that point of its own coordinate system.
pub fn translate_matrix(m: &Matrix, v: &Point) -> PyResult<Matrix> {
    let [a, b, c, d, e, f] = m;
    let [x, y] = v;
    Ok([
        a.clone(),
        b.clone(),
        c.clone(),
        d.clone(),
        x.mul(a)?.add(&y.mul(c)?)?.add(e)?,
        x.mul(b)?.add(&y.mul(d)?)?.add(f)?,
    ])
}

// pdfminer/utils.py:295 apply_matrix_pt
/// Applies a matrix to a point.
pub fn apply_matrix_pt(m: &Matrix, v: &Point) -> PyResult<Point> {
    let [a, b, c, d, e, f] = m;
    let [x, y] = v;
    Ok([a.mul(x)?.add(&c.mul(y)?)?.add(e)?, b.mul(x)?.add(&d.mul(y)?)?.add(f)?])
}

// pdfminer/utils.py:302 apply_matrix_rect
/// Applies a matrix to a rectangle: the result is the rectangle of the same
/// orientation that tightly fits the transformed corners.
pub fn apply_matrix_rect(m: &Matrix, rect: &Rect) -> PyResult<Rect> {
    let [x0, y0, x1, y1] = rect;
    let [left1, bottom1] = apply_matrix_pt(m, &[x0.clone(), y0.clone()])?;
    let [right1, bottom2] = apply_matrix_pt(m, &[x1.clone(), y0.clone()])?;
    let [right2, top1] = apply_matrix_pt(m, &[x1.clone(), y1.clone()])?;
    let [left2, top2] = apply_matrix_pt(m, &[x0.clone(), y1.clone()])?;
    Ok([
        py_min_of(&[&left1, &left2, &right1, &right2]),
        py_min_of(&[&bottom1, &bottom2, &top1, &top2]),
        py_max_of(&[&left1, &left2, &right1, &right2]),
        py_max_of(&[&bottom1, &bottom2, &top1, &top2]),
    ])
}

// pdfminer/utils.py:332 apply_matrix_norm
/// `apply_matrix_pt(m, (p, q)) - apply_matrix_pt(m, (0, 0))`.
pub fn apply_matrix_norm(m: &Matrix, v: &Point) -> PyResult<Point> {
    let [a, b, c, d, _e, _f] = m;
    let [p, q] = v;
    Ok([a.mul(p)?.add(&c.mul(q)?)?, b.mul(p)?.add(&d.mul(q)?)?])
}

// pdfminer/utils.py:371 drange
/// `range(int(v0) // d, int(v1 + d) // d)`. `int()` of an infinite or NaN
/// float raises as in Python. A bound beyond 64 bits (a range Python would
/// spend years iterating) raises `OverflowError`.
pub fn drange(v0: &Num, v1: &Num, d: i64) -> PyResult<Range<i64>> {
    let d = PyInt::Small(d);
    let lo = py_int_of(v0)?.floordiv(&d)?;
    let hi = py_int_of(&v1.add(&Num::Int(d.clone()))?)?.floordiv(&d)?;
    match (lo.as_i64(), hi.as_i64()) {
        (Some(lo), Some(hi)) => Ok(lo..hi),
        _ => Err(exc::OverflowError("Python int too large to convert to C ssize_t")),
    }
}

/// `int(x)` of a number: truncation of a float.
fn py_int_of(x: &Num) -> PyResult<PyInt> {
    match x {
        Num::Int(i) => Ok(i.clone()),
        Num::Float(f) => float_to_int(*f),
    }
}

// pdfminer/utils.py:376 get_bound
/// Computes a minimal rectangle that covers all the points:
/// `(INF, INF, -INF, -INF)` (ints) for no point, and a coordinate is kept
/// by `min`/`max` only when it compares beyond the current bound.
pub fn get_bound<'a>(pts: impl IntoIterator<Item = &'a Point>) -> Rect {
    let [mut x0, mut y0, mut x1, mut y1] = [inf(), inf(), neg_inf(), neg_inf()];
    for [x, y] in pts {
        x0 = py_min2(&x0, x);
        y0 = py_min2(&y0, y);
        x1 = py_max2(&x1, x);
        y1 = py_max2(&y1, y);
    }
    [x0, y0, x1, y1]
}

/// `format(x, ".{prec}f")` (`%.{prec}f`) of a number: an int is converted
/// to float first (`OverflowError` beyond the float range).
pub fn format_fixed(x: &Num, prec: usize) -> PyResult<String> {
    let v = x.to_f64()?;
    Ok(if v.is_nan() {
        "nan".to_owned()
    } else if v.is_infinite() {
        if v > 0.0 { "inf" } else { "-inf" }.to_owned()
    } else {
        // Rust's fixed-precision formatting is exact, ties to even, like
        // PyOS_double_to_string's "f" mode.
        format!("{v:.prec$}")
    })
}

// pdfminer/utils.py:699 bbox2str
pub fn bbox2str(bbox: &Rect) -> PyResult<String> {
    let [x0, y0, x1, y1] = bbox;
    Ok(format!("{},{},{},{}", format_fixed(x0, 3)?, format_fixed(y0, 3)?, format_fixed(x1, 3)?, format_fixed(y1, 3)?))
}

// pdfminer/utils.py:704 matrix2str
pub fn matrix2str(m: &Matrix) -> PyResult<String> {
    let [a, b, c, d, e, f] = m;
    Ok(format!(
        "[{},{},{},{}, ({},{})]",
        format_fixed(a, 2)?,
        format_fixed(b, 2)?,
        format_fixed(c, 2)?,
        format_fixed(d, 2)?,
        format_fixed(e, 2)?,
        format_fixed(f, 2)?
    ))
}

/// What a [`Plane`] holds: an object with a bounding box, compared by
/// identity (layout objects have no `__eq__`/`__hash__` of their own).
pub trait PlaneItem: Clone {
    /// `(obj.x0, obj.y0, obj.x1, obj.y1)`, read when the plane needs them.
    fn plane_bbox(&self) -> Rect;
    /// The object's identity.
    fn plane_id(&self) -> usize;
}

/// pdfminer/utils.py:738 `Plane`: a set-like data structure for objects
/// placed on a plane, which can efficiently find the objects in a
/// rectangular area. It keeps the objects in insertion order and in a grid
/// of `gridsize`-sized cells.
pub struct Plane<T: PlaneItem> {
    /// `_seq`: every object added, in order (never pruned, as upstream).
    seq: Vec<T>,
    /// `_objs`: the identities of the objects currently placed.
    objs: FxHashSet<usize>,
    /// `_grid`: the objects of each cell, in insertion order.
    grid: FxHashMap<(i64, i64), Vec<T>>,
    pub gridsize: i64,
    pub x0: Num,
    pub y0: Num,
    pub x1: Num,
    pub y1: Num,
}

impl<T: PlaneItem> Plane<T> {
    // pdfminer/utils.py:746 Plane.__init__
    pub fn new(bbox: &Rect, gridsize: i64) -> Plane<T> {
        let [x0, y0, x1, y1] = bbox.clone();
        Plane { seq: Vec::new(), objs: FxHashSet::default(), grid: HashMap::default(), gridsize, x0, y0, x1, y1 }
    }

    // pdfminer/utils.py:756 Plane.__iter__
    /// The objects in insertion order (an object removed and added again
    /// comes twice, as upstream).
    pub fn iter(&self) -> impl Iterator<Item = &T> + '_ {
        self.seq.iter().filter(|obj| self.objs.contains(&obj.plane_id()))
    }

    // pdfminer/utils.py:759 Plane.__len__
    pub fn len(&self) -> usize {
        self.objs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.objs.is_empty()
    }

    // pdfminer/utils.py:762 Plane.__contains__
    pub fn contains(&self, obj: &T) -> bool {
        self.objs.contains(&obj.plane_id())
    }

    // pdfminer/utils.py:765 Plane._getrange
    /// The grid cells under `bbox`, rows of `x` for each `y`; nothing when
    /// `bbox` does not overlap the plane. `int()` of the clipped bounds can
    /// raise; it does before any cell is produced.
    fn getrange(&self, bbox: &Rect) -> PyResult<Vec<(i64, i64)>> {
        let [x0, y0, x1, y1] = bbox;
        if x1.py_le(&self.x0) || self.x1.py_le(x0) || y1.py_le(&self.y0) || self.y1.py_le(y0) {
            return Ok(Vec::new());
        }
        let x0 = py_max2(&self.x0, x0);
        let y0 = py_max2(&self.y0, y0);
        let x1 = py_min2(&self.x1, x1);
        let y1 = py_min2(&self.y1, y1);
        let mut cells = Vec::new();
        let rows = drange(&y0, &y1, self.gridsize)?;
        if rows.is_empty() {
            return Ok(cells);
        }
        // the inner range is evaluated for each row, always to this value
        let columns = drange(&x0, &x1, self.gridsize)?;
        for grid_y in rows {
            for grid_x in columns.clone() {
                cells.push((grid_x, grid_y));
            }
        }
        Ok(cells)
    }

    // pdfminer/utils.py:777 Plane.extend
    pub fn extend<'a>(&mut self, objs: impl IntoIterator<Item = &'a T>) -> PyResult<()>
    where
        T: 'a,
    {
        for obj in objs {
            self.add(obj)?;
        }
        Ok(())
    }

    // pdfminer/utils.py:781 Plane.add
    /// Places an object.
    pub fn add(&mut self, obj: &T) -> PyResult<()> {
        for k in self.getrange(&obj.plane_bbox())? {
            self.grid.entry(k).or_default().push(obj.clone());
        }
        self.seq.push(obj.clone());
        self.objs.insert(obj.plane_id());
        Ok(())
    }

    // pdfminer/utils.py:793 Plane.remove
    /// Displaces an object (from the cells under its current bounding box;
    /// a missing cell or entry is ignored). Removing an object that is not
    /// in the plane raises `KeyError` (whose message, the `repr` of the
    /// object, is not reproduced: no caller removes an absent object).
    pub fn remove(&mut self, obj: &T) -> PyResult<()> {
        let id = obj.plane_id();
        for k in self.getrange(&obj.plane_bbox())? {
            if let Some(r) = self.grid.get_mut(&k) {
                if let Some(i) = r.iter().position(|o| o.plane_id() == id) {
                    r.remove(i);
                }
            }
        }
        if !self.objs.remove(&id) {
            return Err(exc::KeyError("<object not in the plane>"));
        }
        Ok(())
    }

    // pdfminer/utils.py:800 Plane.find
    /// Calls `f` with each object that overlaps `bbox` (strictly), in the
    /// order of the grid cells, each object once; stops early when `f`
    /// returns `false` (the Python generator is then simply not resumed).
    pub fn find_each(&self, bbox: &Rect, mut f: impl FnMut(&T) -> PyResult<bool>) -> PyResult<()> {
        let [x0, y0, x1, y1] = bbox;
        let mut done = FxHashSet::default();
        for k in self.getrange(bbox)? {
            let Some(objs) = self.grid.get(&k) else { continue };
            for obj in objs {
                if !done.insert(obj.plane_id()) {
                    continue;
                }
                let [ox0, oy0, ox1, oy1] = obj.plane_bbox();
                if ox1.py_le(x0) || x1.py_le(&ox0) || oy1.py_le(y0) || y1.py_le(&oy0) {
                    continue;
                }
                if !f(obj)? {
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    /// `list(plane.find(bbox))`
    pub fn find(&self, bbox: &Rect) -> PyResult<Vec<T>> {
        let mut out = Vec::new();
        self.find_each(bbox, |obj| {
            out.push(obj.clone());
            Ok(true)
        })?;
        Ok(out)
    }
}

// ===========================================================================
// Iteration helpers

// pdfminer/utils.py:402 choplist
/// Groups every `n` elements of the list; a trailing incomplete group is
/// dropped, like the Python generator.
pub fn choplist<T>(n: usize, seq: &[T]) -> std::slice::ChunksExact<'_, T> {
    seq.chunks_exact(n)
}

// pdfminer/utils.py:349 uniq
/// Eliminates duplicated elements, keeping the first of each (`key` is
/// what Python hashes and compares: the identity of a layout object).
pub fn uniq<T, K: Hash + Eq>(objs: impl IntoIterator<Item = T>, mut key: impl FnMut(&T) -> K) -> Vec<T> {
    let mut done = HashSet::new();
    let mut out = Vec::new();
    for obj in objs {
        if done.insert(key(&obj)) {
            out.push(obj);
        }
    }
    out
}

// pdfminer/utils.py:359 fsplit
/// Splits a list into two classes according to the predicate (which may
/// raise, as a Python predicate can).
pub fn fsplit<T>(
    mut pred: impl FnMut(&T) -> PyResult<bool>,
    objs: impl IntoIterator<Item = T>,
) -> PyResult<(Vec<T>, Vec<T>)> {
    let mut t = Vec::new();
    let mut f = Vec::new();
    for obj in objs {
        if pred(&obj)? {
            t.push(obj);
        } else {
            f.push(obj);
        }
    }
    Ok((t, f))
}

// pdfminer/utils.py:388 pick
/// Picks the object `obj` where `func(obj)` has the highest value (the
/// first of equal ones; a score that does not compare greater, a NaN, never
/// wins), else `maxobj`.
pub fn pick<T>(seq: impl IntoIterator<Item = T>, mut func: impl FnMut(&T) -> Num, maxobj: Option<T>) -> Option<T> {
    let mut maxscore: Option<Num> = None;
    let mut maxobj = maxobj;
    for obj in seq {
        let score = func(&obj);
        if maxscore.as_ref().is_none_or(|m| m.py_lt(&score)) {
            maxscore = Some(score);
            maxobj = Some(obj);
        }
    }
    maxobj
}

// ===========================================================================
// Numbers and text

// pdfminer/utils.py:342 isnumber
/// `isinstance(x, (int, float))` (a `bool` is an `int`).
pub fn isnumber(x: &PDFObject) -> bool {
    x.as_num().is_some()
}

// pdfminer/utils.py:412 nunpack
/// Unpacks a variable-length unsigned integer (big endian); `default` for
/// no bytes.
pub fn nunpack(s: &[u8], default: i64) -> PyInt {
    if s.is_empty() {
        return PyInt::Small(default);
    }
    if s.len() <= 7 || (s.len() == 8 && s[0] < 0x80) {
        return PyInt::Small(s.iter().fold(0i64, |acc, &b| (acc << 8) | i64::from(b)));
    }
    let byte = PyInt::Small(256);
    s.iter().fold(PyInt::ZERO, |acc, &b| acc.mul(&byte).add(&PyInt::Small(i64::from(b))))
}

// pdfminer/utils.py:421 PDFDocEncoding
/// The code points of `PDFDocEncoding`, indexed by byte (printed from the
/// Python string, and checked on every byte by `oracle/unit/utils.py`).
#[rustfmt::skip]
pub const PDF_DOC_ENCODING: [u16; 256] = [
    0x0000, 0x0001, 0x0002, 0x0003, 0x0004, 0x0005, 0x0006, 0x0007,
    0x0008, 0x0009, 0x000a, 0x000b, 0x000c, 0x000d, 0x000e, 0x000f,
    0x0010, 0x0011, 0x0012, 0x0013, 0x0014, 0x0015, 0x0017, 0x0017,
    0x02d8, 0x02c7, 0x02c6, 0x02d9, 0x02dd, 0x02db, 0x02da, 0x02dc,
    0x0020, 0x0021, 0x0022, 0x0023, 0x0024, 0x0025, 0x0026, 0x0027,
    0x0028, 0x0029, 0x002a, 0x002b, 0x002c, 0x002d, 0x002e, 0x002f,
    0x0030, 0x0031, 0x0032, 0x0033, 0x0034, 0x0035, 0x0036, 0x0037,
    0x0038, 0x0039, 0x003a, 0x003b, 0x003c, 0x003d, 0x003e, 0x003f,
    0x0040, 0x0041, 0x0042, 0x0043, 0x0044, 0x0045, 0x0046, 0x0047,
    0x0048, 0x0049, 0x004a, 0x004b, 0x004c, 0x004d, 0x004e, 0x004f,
    0x0050, 0x0051, 0x0052, 0x0053, 0x0054, 0x0055, 0x0056, 0x0057,
    0x0058, 0x0059, 0x005a, 0x005b, 0x005c, 0x005d, 0x005e, 0x005f,
    0x0060, 0x0061, 0x0062, 0x0063, 0x0064, 0x0065, 0x0066, 0x0067,
    0x0068, 0x0069, 0x006a, 0x006b, 0x006c, 0x006d, 0x006e, 0x006f,
    0x0070, 0x0071, 0x0072, 0x0073, 0x0074, 0x0075, 0x0076, 0x0077,
    0x0078, 0x0079, 0x007a, 0x007b, 0x007c, 0x007d, 0x007e, 0x0000,
    0x2022, 0x2020, 0x2021, 0x2026, 0x2014, 0x2013, 0x0192, 0x2044,
    0x2039, 0x203a, 0x2212, 0x2030, 0x201e, 0x201c, 0x201d, 0x2018,
    0x2019, 0x201a, 0x2122, 0xfb01, 0xfb02, 0x0141, 0x0152, 0x0160,
    0x0178, 0x017d, 0x0131, 0x0142, 0x0153, 0x0161, 0x017e, 0x0000,
    0x20ac, 0x00a1, 0x00a2, 0x00a3, 0x00a4, 0x00a5, 0x00a6, 0x00a7,
    0x00a8, 0x00a9, 0x00aa, 0x00ab, 0x00ac, 0x0000, 0x00ae, 0x00af,
    0x00b0, 0x00b1, 0x00b2, 0x00b3, 0x00b4, 0x00b5, 0x00b6, 0x00b7,
    0x00b8, 0x00b9, 0x00ba, 0x00bb, 0x00bc, 0x00bd, 0x00be, 0x00bf,
    0x00c0, 0x00c1, 0x00c2, 0x00c3, 0x00c4, 0x00c5, 0x00c6, 0x00c7,
    0x00c8, 0x00c9, 0x00ca, 0x00cb, 0x00cc, 0x00cd, 0x00ce, 0x00cf,
    0x00d0, 0x00d1, 0x00d2, 0x00d3, 0x00d4, 0x00d5, 0x00d6, 0x00d7,
    0x00d8, 0x00d9, 0x00da, 0x00db, 0x00dc, 0x00dd, 0x00de, 0x00df,
    0x00e0, 0x00e1, 0x00e2, 0x00e3, 0x00e4, 0x00e5, 0x00e6, 0x00e7,
    0x00e8, 0x00e9, 0x00ea, 0x00eb, 0x00ec, 0x00ed, 0x00ee, 0x00ef,
    0x00f0, 0x00f1, 0x00f2, 0x00f3, 0x00f4, 0x00f5, 0x00f6, 0x00f7,
    0x00f8, 0x00f9, 0x00fa, 0x00fb, 0x00fc, 0x00fd, 0x00fe, 0x00ff,
];

// pdfminer/utils.py:684 decode_text
/// Decodes a PDFDocEncoding string to Unicode; a string starting with the
/// UTF-16BE byte order mark is `str(s[2:], "utf-16be", "ignore")` (a lone
/// surrogate and a trailing odd byte are dropped).
pub fn decode_text(s: &[u8]) -> String {
    if let Some(body) = s.strip_prefix(b"\xfe\xff") {
        let units = body.as_chunks::<2>().0.iter().map(|p| u16::from_be_bytes(*p));
        char::decode_utf16(units).filter_map(Result::ok).collect()
    } else {
        s.iter()
            .map(|&c| char::from_u32(u32::from(PDF_DOC_ENCODING[usize::from(c)])).expect("no surrogate in the table"))
            .collect()
    }
}

// pdfminer/utils.py:816-817 ROMAN_ONES, ROMAN_FIVES
const ROMAN_ONES: [&str; 4] = ["i", "x", "c", "m"];
const ROMAN_FIVES: [&str; 3] = ["v", "l", "d"];

// pdfminer/utils.py:820 format_int_roman
/// Formats a number as lowercase Roman numerals; `AssertionError` outside
/// `0 < value < 4000`.
pub fn format_int_roman(value: &PyInt) -> PyResult<String> {
    let mut value = match value.as_i64() {
        Some(v) if 0 < v && v < 4000 => v as usize,
        _ => return Err(exc::AssertionError("")),
    };
    let mut result: Vec<String> = Vec::new();
    let mut index = 0;
    while value != 0 {
        let mut remainder = value % 10;
        value /= 10;
        if remainder == 9 {
            result.insert(0, ROMAN_ONES[index].to_owned());
            result.insert(1, ROMAN_ONES[index + 1].to_owned());
        } else if remainder == 4 {
            result.insert(0, ROMAN_ONES[index].to_owned());
            result.insert(1, ROMAN_FIVES[index].to_owned());
        } else {
            let over_five = remainder >= 5;
            if over_five {
                result.insert(0, ROMAN_FIVES[index].to_owned());
                remainder -= 5;
            }
            result.insert(usize::from(over_five), ROMAN_ONES[index].repeat(remainder));
        }
        index += 1;
    }
    Ok(result.concat())
}

// pdfminer/utils.py:845 format_int_alpha
/// Formats a number as lowercase letters a-z, aa-zz, etc.; `AssertionError`
/// unless `value > 0`.
pub fn format_int_alpha(value: &PyInt) -> PyResult<String> {
    if *value <= PyInt::ZERO {
        return Err(exc::AssertionError(""));
    }
    let letters = PyInt::Small(26);
    let mut value = value.clone();
    let mut result: Vec<u8> = Vec::new();
    while !value.is_zero() {
        let v = value.sub(&PyInt::Small(1));
        let remainder = v.modulo(&letters)?;
        value = v.floordiv(&letters)?;
        result.push(b'a' + remainder.as_i64().expect("a remainder below 26") as u8);
    }
    result.reverse();
    Ok(String::from_utf8(result).expect("ASCII letters"))
}

// ===========================================================================
// AES padding (unpad_aes): written by the port of the security handlers.

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
