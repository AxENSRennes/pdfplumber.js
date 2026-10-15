//! Python operators on the values `pdfdocument.py` computes with, where
//! upstream applies them to whatever objects a file holds.
//!
//! `PDFXRefStream` adds, compares and multiplies the entries of `/Index`
//! and `/W` (`start + i`, `start <= objid`, `self.entlen * i`), which a
//! file can make floats, strings, arrays or names; iterating a dict
//! (`choplist(2, index_array)`, `(fl1, fl2, fl3) = stream["W"]`) gives its
//! keys, `str` values that [`PDFObject`] has no variant for. [`PyVal`] is
//! such a value, and its methods are the Python operators with CPython
//! 3.12's results and `TypeError`s. None of this is a Python module: it is
//! the semantics of the builtin types the upstream code relies on.

use std::cmp::Ordering;
use std::rc::Rc;

use pycompat::{exc, Num, PyInt, PyResult};

use crate::pdftypes::{ObjId, PDFList, PDFObject};

/// A Python value: a PDF object, or a `str`.
#[derive(Clone, Debug)]
pub enum PyVal {
    Obj(PDFObject),
    Str(Rc<str>),
}

impl From<PDFObject> for PyVal {
    fn from(obj: PDFObject) -> PyVal {
        PyVal::Obj(obj)
    }
}

impl From<PyInt> for PyVal {
    fn from(i: PyInt) -> PyVal {
        PyVal::Obj(PDFObject::Int(i))
    }
}

impl PyVal {
    /// `type(x).__name__`
    pub fn type_name(&self) -> &'static str {
        match self {
            PyVal::Obj(o) => o.type_name(),
            PyVal::Str(_) => "str",
        }
    }

    fn num(&self) -> Option<Num> {
        match self {
            PyVal::Obj(o) => o.as_num(),
            PyVal::Str(_) => None,
        }
    }

    /// The value as an int when it is an `int` or a `bool` (an index).
    pub fn as_index(&self) -> Option<PyInt> {
        match self {
            PyVal::Obj(PDFObject::Int(i)) => Some(i.clone()),
            PyVal::Obj(PDFObject::Bool(b)) => Some(PyInt::Small(i64::from(*b))),
            _ => None,
        }
    }

    /// `range(self)`: the int, or the `TypeError` of `range()`.
    pub fn range_arg(&self) -> PyResult<PyInt> {
        self.as_index()
            .ok_or_else(|| exc::TypeError(format!("'{}' object cannot be interpreted as an integer", self.type_name())))
    }

    /// The value as an object id (`hash()` must succeed).
    pub fn to_objid(&self) -> PyResult<ObjId> {
        match self {
            PyVal::Obj(o) => ObjId::new(o.clone()),
            // a str id would need ObjId to hold a str: `start + i` raises
            // for a str start, so no str reaches an id
            PyVal::Str(_) => unreachable!("no str is an object id"),
        }
    }

    /// `len(self)`
    pub fn len(&self) -> PyResult<usize> {
        match self {
            PyVal::Obj(PDFObject::List(l)) => Ok(l.len()),
            PyVal::Obj(PDFObject::Bytes(b)) => Ok(b.len()),
            PyVal::Obj(PDFObject::Dict(d)) => Ok(d.len()),
            PyVal::Str(s) => Ok(s.chars().count()),
            other => Err(exc::TypeError(format!("object of type '{}' has no len()", other.type_name()))),
        }
    }

    /// `list(self)`, or the error of `iter(self)`/its first `next()`;
    /// `not_iterable` writes the `TypeError` of a non-iterable for the
    /// statement that iterates (`for`, unpacking).
    pub fn items(&self, not_iterable: impl FnOnce(&str) -> String) -> PyResult<Vec<PyVal>> {
        match self {
            PyVal::Obj(PDFObject::List(l)) => Ok(l.to_vec().into_iter().map(PyVal::Obj).collect()),
            PyVal::Obj(PDFObject::Bytes(b)) => {
                Ok(b.iter().map(|&c| PyVal::Obj(PDFObject::Int(PyInt::Small(i64::from(c))))).collect())
            }
            PyVal::Obj(PDFObject::Dict(d)) => Ok(d.keys().into_iter().map(PyVal::Str).collect()),
            PyVal::Str(s) => Ok(s.chars().map(|c| PyVal::Str(c.to_string().into())).collect()),
            // no __iter__ but a __getitem__: the sequence protocol calls
            // stream[0], which raises PDFKeyError (not IndexError)
            PyVal::Obj(PDFObject::Stream(s)) => Err(s.missing_attribute("0")),
            other => Err(exc::TypeError(not_iterable(other.type_name()))),
        }
    }

    /// `a + b`
    pub fn add(&self, other: &PyVal) -> PyResult<PyVal> {
        if let (Some(a), Some(b)) = (self.num(), other.num()) {
            return Ok(num_val(a.add(&b)?));
        }
        let unsupported =
            || exc::TypeError(format!("unsupported operand type(s) for +: '{}' and '{}'", self.type_name(), other.type_name()));
        match (self, other) {
            (PyVal::Obj(PDFObject::Bytes(a)), PyVal::Obj(PDFObject::Bytes(b))) => {
                Ok(PyVal::Obj(PDFObject::Bytes([&a[..], &b[..]].concat().into())))
            }
            (PyVal::Obj(PDFObject::Bytes(_)), b) => Err(exc::TypeError(format!("can't concat {} to bytes", b.type_name()))),
            (PyVal::Obj(PDFObject::List(a)), PyVal::Obj(PDFObject::List(b))) => {
                let mut items = a.to_vec();
                items.extend(b.to_vec());
                Ok(PyVal::Obj(PDFObject::List(PDFList::new(items))))
            }
            (PyVal::Obj(PDFObject::List(_)), b) => {
                Err(exc::TypeError(format!("can only concatenate list (not \"{}\") to list", b.type_name())))
            }
            (PyVal::Str(a), PyVal::Str(b)) => Ok(PyVal::Str(format!("{a}{b}").into())),
            (PyVal::Str(_), b) => {
                Err(exc::TypeError(format!("can only concatenate str (not \"{}\") to str", b.type_name())))
            }
            _ => Err(unsupported()),
        }
    }

    /// `a - b`
    pub fn sub(&self, other: &PyVal) -> PyResult<PyVal> {
        match (self.num(), other.num()) {
            (Some(a), Some(b)) => Ok(num_val(a.sub(&b)?)),
            _ => Err(exc::TypeError(format!(
                "unsupported operand type(s) for -: '{}' and '{}'",
                self.type_name(),
                other.type_name()
            ))),
        }
    }

    /// `a <= b` (`op` `"<="`) or `a < b` (`op` `"<"`): numbers, bytes and
    /// strs compare; other pairs raise `TypeError` (no ordering is defined
    /// on PDF names, keywords, references, dicts, `None`, nor between
    /// different kinds).
    pub fn compare(&self, other: &PyVal, op: &str) -> PyResult<bool> {
        let ordering = match (self, other) {
            (a, b) if a.num().is_some() && b.num().is_some() => {
                a.num().expect("a number").py_cmp(&b.num().expect("a number"))
            }
            (PyVal::Obj(PDFObject::Bytes(a)), PyVal::Obj(PDFObject::Bytes(b))) => Some(a.cmp(b)),
            (PyVal::Str(a), PyVal::Str(b)) => Some(a.chars().cmp(b.chars())),
            _ => {
                return Err(exc::TypeError(format!(
                    "'{op}' not supported between instances of '{}' and '{}'",
                    self.type_name(),
                    other.type_name()
                )))
            }
        };
        Ok(match op {
            "<" => ordering == Some(Ordering::Less),
            _ => matches!(ordering, Some(Ordering::Less | Ordering::Equal)),
        })
    }
}

/// A number as a value (an int stays an int).
fn num_val(n: Num) -> PyVal {
    PyVal::Obj(match n {
        Num::Int(i) => PDFObject::Int(i),
        Num::Float(f) => PDFObject::Float(f),
    })
}

/// `offset = entlen * index; data[offset : offset + entlen]`: the slice
/// bounds when both are ints, else the error Python raises on the way.
/// `entlen` is `fl1 + fl2 + fl3` (a number, `bytes`, `list` or `str`),
/// `index` a number. A sequence repeated by an int is not built: its
/// slice raises in any case (the `MemoryError` of a repetition too large
/// to allocate is not modeled; the overflow of its size is).
pub fn entry_bounds(entlen: &PyVal, index: &PyVal) -> PyResult<(PyInt, PyInt)> {
    let slice_error = || exc::TypeError("slice indices must be integers or None or have an __index__ method");
    match (entlen.as_index(), index.as_index()) {
        (Some(e), Some(i)) => {
            let offset = e.mul(&i);
            let end = offset.add(&e);
            Ok((offset, end))
        }
        _ => {
            if let (Some(e), Some(i)) = (entlen.num(), index.num()) {
                // a float product: float slice bounds (computing them may
                // overflow a float)
                let offset = e.mul(&i)?;
                offset.add(&e)?;
                return Err(slice_error());
            }
            // sequence * index
            let Some(count) = index.as_index() else {
                return Err(exc::TypeError(format!(
                    "can't multiply sequence by non-int of type '{}'",
                    index.type_name()
                )));
            };
            let Some(count) = count.as_i64() else {
                return Err(exc::OverflowError("cannot fit 'int' into an index-sized integer"));
            };
            let size = entlen.len()? as i64;
            if count > 0 && size > i64::MAX / count {
                return Err(match entlen {
                    PyVal::Obj(PDFObject::Bytes(_)) => exc::OverflowError("repeated bytes are too long"),
                    PyVal::Str(_) => exc::OverflowError("repeated string is too long"),
                    _ => exc::MemoryError(""),
                });
            }
            // offset + entlen: two sequences of the same type
            Err(slice_error())
        }
    }
}

/// `data[start:stop]` with Python's clamping (`None` is an open end; a
/// negative bound counts from the end; an int beyond 64 bits is clamped
/// first, as `PyNumber_AsSsize_t` does for slices).
pub fn slice<'a>(data: &'a [u8], start: Option<&PyInt>, stop: Option<&PyInt>) -> &'a [u8] {
    let len = data.len() as i64;
    let bound = |b: Option<&PyInt>, default: i64| -> i64 {
        let Some(b) = b else { return default };
        let v = b.as_i64().unwrap_or(if b.is_negative() { i64::MIN } else { i64::MAX });
        if v < 0 {
            (v.saturating_add(len)).max(0)
        } else {
            v.min(len)
        }
    };
    let (start, stop) = (bound(start, 0), bound(stop, len));
    if start >= stop {
        return &[];
    }
    &data[start as usize..stop as usize]
}
