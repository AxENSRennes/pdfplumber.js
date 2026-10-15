//! The result encodings of the JavaScript boundary (`docs/js-runtime.md`).
//!
//! Python values leave the core through a [`Writer`]: a stream of value
//! events (`list(n)` followed by `n` values, `dict(n)` followed by `n` keys
//! and values...). Two writers implement it:
//!
//! - [`binary::BinWriter`], the format the JS API uses (chosen by the
//!   measurements of `docs/js-runtime.md`): tagged values, a shared string
//!   table, and uniform lists of dicts (page objects) written column by
//!   column;
//! - [`json::JsonWriter`]: JSON with an escape for the values JSON lacks,
//!   kept for the measurements and as a readable debugging form.
//!
//! [`write_value`] walks a pdfplumber value ([`PyValue`]) without recursion:
//! a value nested 10^6 levels deep is written like a flat one
//! (`docs/architecture.md`, "Profondeur de récursion"). PDF lists and
//! dicts are mutable in Python and may contain themselves; such a value
//! raises `ValueError("Circular reference detected")`, as `json.dumps`
//! does, instead of looping.

pub mod binary;
pub mod json;

use std::rc::Rc;

use pdfminer::pdftypes::PDFObject;
use pdfminer::psparser::LiteralName;
use pdfplumber::obj::{Obj, PyValue};
use pycompat::{exc, PyInt, PyResult};
use rustc_hash::FxHashSet;

use crate::strings::PyStrRef;

/// 2^53: integers up to this magnitude are exact JavaScript numbers.
pub const MAX_SAFE: i64 = 1 << 53;

/// The integer as an exact JavaScript number, when it is one.
pub fn safe_int(i: &PyInt) -> Option<i64> {
    i.as_i64().filter(|v| (-MAX_SAFE..=MAX_SAFE).contains(v))
}

/// A sink of value events (see the module documentation). Containers
/// announce their length; `dict(n)` is followed by `n` pairs, each a key
/// (written with [`Writer::str`] or [`Writer::py_str`]) then a value.
pub trait Writer {
    fn none(&mut self);
    fn bool(&mut self, b: bool);
    fn int(&mut self, i: &PyInt);
    fn float(&mut self, x: f64);
    fn str(&mut self, s: &str);
    /// A `str` that may hold surrogates.
    fn py_str(&mut self, s: PyStrRef<'_>) -> PyResult<()>;
    fn bytes(&mut self, b: &[u8]);
    fn list(&mut self, n: usize);
    fn tuple(&mut self, n: usize);
    fn dict(&mut self, n: usize);
    /// A `PSLiteral`, followed by its name (a `str` or `bytes` value).
    fn literal(&mut self);
    /// A `PSKeyword`.
    fn keyword(&mut self, name: &[u8]);
    /// A `PDFObjRef`, followed by its `objid`.
    fn objref(&mut self);
    /// A `PDFStream`, followed by five values: `objid`, `genno`, `attrs`
    /// (a dict), `rawdata` and `data` (`bytes` or `None`).
    fn stream(&mut self);
    /// Writes `rows` (at least two dicts with the same keys in the same
    /// order) as one list value, if this writer has a form for it, and
    /// returns whether it did.
    fn table(&mut self, _rows: &[PyValue], _keys: &[Rc<str>]) -> PyResult<bool> {
        Ok(false)
    }
}

/// The keys of `items` when they are at least two dicts with the same
/// keys in the same order.
pub fn uniform_keys(items: &[PyValue]) -> Option<Vec<Rc<str>>> {
    let PyValue::Dict(first) = items.first()? else { return None };
    if items.len() < 2 || first.is_empty() {
        return None;
    }
    let keys = first.items();
    for item in &items[1..] {
        let PyValue::Dict(d) = item else { return None };
        let other = d.items();
        if other.len() != keys.len()
            || !other.iter().zip(keys).all(|((a, _), (b, _))| Rc::ptr_eq(a, b) || a.as_ref() == b.as_ref())
        {
            return None;
        }
    }
    Some(keys.iter().map(|(k, _)| k.clone()).collect())
}

enum Work<'a> {
    Py(&'a PyValue),
    Pdf(PDFObject),
    Key(&'a str),
    PdfKey(Rc<str>),
    /// Leaves a PDF container (cycle detection).
    Exit(usize),
}

/// Writes a pdfplumber value (see the module documentation).
pub fn write_value<W: Writer + ?Sized>(w: &mut W, v: &PyValue) -> PyResult<()> {
    let mut stack: Vec<Work<'_>> = vec![Work::Py(v)];
    // the PDF containers being written (Python lists and dicts are mutable)
    let mut active: FxHashSet<usize> = FxHashSet::default();
    while let Some(work) = stack.pop() {
        match work {
            Work::Key(k) => w.str(k),
            Work::PdfKey(k) => w.str(&k),
            Work::Exit(addr) => {
                active.remove(&addr);
            }
            Work::Py(v) => match v {
                PyValue::None => w.none(),
                PyValue::Bool(b) => w.bool(*b),
                PyValue::Int(i) => w.int(i),
                PyValue::Float(x) => w.float(*x),
                PyValue::Str(s) => w.str(s),
                PyValue::Bytes(b) => w.bytes(b),
                PyValue::List(items) | PyValue::Tuple(items) => {
                    if let Some(keys) = uniform_keys(items) {
                        if w.table(items, &keys)? {
                            continue;
                        }
                    }
                    if matches!(v, PyValue::List(_)) {
                        w.list(items.len());
                    } else {
                        w.tuple(items.len());
                    }
                    stack.extend(items.iter().rev().map(Work::Py));
                }
                PyValue::Dict(d) => push_obj(w, &mut stack, d),
                PyValue::Pdf(o) => stack.push(Work::Pdf(o.clone())),
            },
            Work::Pdf(o) => match o {
                PDFObject::Null => w.none(),
                PDFObject::Bool(b) => w.bool(b),
                PDFObject::Int(i) => w.int(&i),
                PDFObject::Float(x) => w.float(x),
                PDFObject::Bytes(b) => w.bytes(&b),
                PDFObject::Literal(lit) => {
                    w.literal();
                    match lit.name() {
                        LiteralName::Str(s) => w.str(s),
                        LiteralName::Bytes(b) => w.bytes(b),
                    }
                }
                PDFObject::Keyword(kw) => w.keyword(kw.name()),
                PDFObject::Ref(r) => {
                    w.objref();
                    w.int(r.objid());
                }
                PDFObject::List(list) => {
                    let addr = std::ptr::from_ref(&*list.borrow()) as usize;
                    let items = list.borrow().clone();
                    enter(&mut active, &mut stack, addr)?;
                    w.list(items.len());
                    stack.extend(items.into_iter().rev().map(Work::Pdf));
                }
                PDFObject::Dict(dict) => {
                    let addr = std::ptr::from_ref(&*dict.borrow()) as usize;
                    enter(&mut active, &mut stack, addr)?;
                    let items: Vec<(Rc<str>, PDFObject)> =
                        dict.borrow().iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                    w.dict(items.len());
                    for (k, v) in items.into_iter().rev() {
                        stack.push(Work::Pdf(v));
                        stack.push(Work::PdfKey(k));
                    }
                }
                PDFObject::Stream(s) => {
                    let addr = Rc::as_ptr(&s) as usize;
                    enter(&mut active, &mut stack, addr)?;
                    w.stream();
                    let opt_int = |i: Option<PyInt>| i.map_or(PDFObject::Null, PDFObject::Int);
                    let opt_bytes = |b: Option<Rc<[u8]>>| b.map_or(PDFObject::Null, PDFObject::Bytes);
                    // pushed in reverse: objid, genno, attrs, rawdata, data
                    stack.push(Work::Pdf(opt_bytes(s.data())));
                    stack.push(Work::Pdf(opt_bytes(s.rawdata())));
                    stack.push(Work::Pdf(PDFObject::Dict(s.attrs.clone())));
                    stack.push(Work::Pdf(opt_int(s.genno())));
                    stack.push(Work::Pdf(opt_int(s.objid())));
                }
            },
        }
    }
    Ok(())
}

fn push_obj<'a, W: Writer + ?Sized>(w: &mut W, stack: &mut Vec<Work<'a>>, d: &'a Obj) {
    w.dict(d.len());
    for (k, v) in d.items().iter().rev() {
        stack.push(Work::Py(v));
        stack.push(Work::Key(k));
    }
}

fn enter(active: &mut FxHashSet<usize>, stack: &mut Vec<Work<'_>>, addr: usize) -> PyResult<()> {
    if !active.insert(addr) {
        return Err(exc::ValueError("Circular reference detected"));
    }
    stack.push(Work::Exit(addr));
    Ok(())
}
