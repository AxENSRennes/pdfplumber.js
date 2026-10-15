//! Port of `pdfminer/casting.py`: lenient conversions of operands to
//! numbers.
//!
//! The operands are PDF objects ([`PDFObject`]). Python's `int(o)` and
//! `float(o)` accept more than numbers: a `bool`, and a `bytes` string that
//! reads as a number (`float(b"1.5")`, `float(b"nan")`, `int(b" 12 ")`).
//! [`py_int`] and [`py_float`] are those builtins on a PDF object, with
//! CPython's exceptions; the `safe_*` functions catch what upstream
//! catches. `safe_int` does not catch `OverflowError`, so `safe_int(inf)`
//! raises, as in Python.

use pycompat::float::{float_from_bytes, float_from_str, float_to_int};
use pycompat::{exc, ExcKind, PyInt, PyResult};

use crate::pdftypes::PDFObject;

/// `int(o)` (`PyNumber_Long`): `TypeError` for an object that is neither a
/// number nor a bytes string, `ValueError` for bytes that do not read as an
/// int and for a NaN, `OverflowError` for an infinity.
pub fn py_int(o: &PDFObject) -> PyResult<PyInt> {
    match o {
        PDFObject::Bool(b) => Ok(PyInt::Small(i64::from(*b))),
        PDFObject::Int(i) => Ok(i.clone()),
        PDFObject::Float(f) => float_to_int(*f),
        PDFObject::Bytes(b) => PyInt::parse_bytes(b, 10),
        other => Err(exc::TypeError(format!(
            "int() argument must be a string, a bytes-like object or a real number, not '{}'",
            other.type_name()
        ))),
    }
}

/// `float(o)` (`PyNumber_Float`): `TypeError` for an object that is
/// neither a number nor a bytes string, `ValueError` for bytes that do not
/// read as a float, `OverflowError` for an int beyond the float range.
pub fn py_float(o: &PDFObject) -> PyResult<f64> {
    match o {
        PDFObject::Bool(b) => Ok(f64::from(u8::from(*b))),
        PDFObject::Int(i) => i.to_f64(),
        PDFObject::Float(f) => Ok(*f),
        PDFObject::Bytes(b) => float_from_bytes(b),
        other => Err(exc::TypeError(format!(
            "float() argument must be a string or a real number, not '{}'",
            other.type_name()
        ))),
    }
}

// pdfminer/casting.py:10 safe_int
/// `int(o)`, or `None` on `TypeError`/`ValueError`; an `OverflowError`
/// (`int(inf)`) propagates.
pub fn safe_int(o: &PDFObject) -> PyResult<Option<PyInt>> {
    match py_int(o) {
        Ok(i) => Ok(Some(i)),
        Err(e) if e.is_instance(ExcKind::TypeError) || e.is_instance(ExcKind::ValueError) => Ok(None),
        Err(e) => Err(e),
    }
}

// pdfminer/casting.py:17 safe_float
/// `float(o)`, or `None` on `TypeError`/`ValueError`/`OverflowError` (all
/// that `float()` raises on a PDF object).
pub fn safe_float(o: &PDFObject) -> Option<f64> {
    py_float(o).ok()
}

// pdfminer/casting.py:24 safe_matrix
/// The six operands as floats, or `None` if one does not convert (every
/// operand is converted first, as upstream).
pub fn safe_matrix(
    a: &PDFObject,
    b: &PDFObject,
    c: &PDFObject,
    d: &PDFObject,
    e: &PDFObject,
    f: &PDFObject,
) -> Option<[f64; 6]> {
    let a_f = safe_float(a);
    let b_f = safe_float(b);
    let c_f = safe_float(c);
    let d_f = safe_float(d);
    let e_f = safe_float(e);
    let f_f = safe_float(f);
    Some([a_f?, b_f?, c_f?, d_f?, e_f?, f_f?])
}

// pdfminer/casting.py:45 safe_rgb
pub fn safe_rgb(r: &PDFObject, g: &PDFObject, b: &PDFObject) -> Option<[f64; 3]> {
    safe_float_triple(r, g, b)
}

// pdfminer/casting.py:49 safe_cmyk
pub fn safe_cmyk(c: &PDFObject, m: &PDFObject, y: &PDFObject, k: &PDFObject) -> Option<[f64; 4]> {
    safe_float_quadruple(c, m, y, k)
}

/// An item of an iterated PDF object: the items of a list, the keys (`str`)
/// of a dict, the ints of a bytes string.
enum Item {
    Obj(PDFObject),
    Str(std::rc::Rc<str>),
}

impl Item {
    fn safe_float(&self) -> Option<f64> {
        match self {
            Item::Obj(o) => safe_float(o),
            Item::Str(s) => float_from_str(s).ok(),
        }
    }
}

// pdfminer/casting.py:55 safe_rect_list
/// The first four items of an iterable as a float rectangle; `None` when
/// `value` is not iterable, has fewer than four items, or one does not
/// convert. A dict iterates over its keys, a bytes string over its byte
/// values; a stream iterates through `__getitem__(0)`, which raises the
/// `PDFKeyError` of a missing attribute (not caught, as in Python).
pub fn safe_rect_list(value: &PDFObject) -> PyResult<Option<[f64; 4]>> {
    let values: Vec<Item> = match value {
        PDFObject::List(l) => l.borrow().iter().take(4).cloned().map(Item::Obj).collect(),
        PDFObject::Dict(d) => d.keys().into_iter().take(4).map(Item::Str).collect(),
        PDFObject::Bytes(b) => {
            b.iter().take(4).map(|&c| Item::Obj(PDFObject::Int(PyInt::Small(i64::from(c))))).collect()
        }
        PDFObject::Stream(_) => {
            value.py_getitem_index0()?;
            unreachable!("a stream has no attribute 0")
        }
        // TypeError: the object is not iterable
        _ => return Ok(None),
    };
    if values.len() != 4 {
        return Ok(None);
    }
    let a_f = values[0].safe_float();
    let b_f = values[1].safe_float();
    let c_f = values[2].safe_float();
    let d_f = values[3].safe_float();
    Ok((|| Some([a_f?, b_f?, c_f?, d_f?]))())
}

// pdfminer/casting.py:67 safe_rect
pub fn safe_rect(a: &PDFObject, b: &PDFObject, c: &PDFObject, d: &PDFObject) -> Option<[f64; 4]> {
    safe_float_quadruple(a, b, c, d)
}

// pdfminer/casting.py:71 _safe_float_triple
fn safe_float_triple(a: &PDFObject, b: &PDFObject, c: &PDFObject) -> Option<[f64; 3]> {
    let a_f = safe_float(a);
    let b_f = safe_float(b);
    let c_f = safe_float(c);
    Some([a_f?, b_f?, c_f?])
}

// pdfminer/casting.py:82 _safe_float_quadruple
fn safe_float_quadruple(a: &PDFObject, b: &PDFObject, c: &PDFObject, d: &PDFObject) -> Option<[f64; 4]> {
    let a_f = safe_float(a);
    let b_f = safe_float(b);
    let c_f = safe_float(c);
    let d_f = safe_float(d);
    Some([a_f?, b_f?, c_f?, d_f?])
}
