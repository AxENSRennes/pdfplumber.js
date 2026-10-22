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

use std::cell::RefCell;
use std::rc::Rc;

use pdfminer::pdftypes::PDFObject;
use pdfminer::psparser::LiteralName;
use pdfplumber::obj::{Obj, PyValue};
use pycompat::{exc, ExcKind, PyErr, PyInt, PyResult, PyStr};
use rustc_hash::{FxHashMap, FxHashSet};

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
    /// A pdfplumber `Page` (`annot["data"]["P"]`, `getattr(pdf,
    /// "pages")`), followed by two values: the handle of the page
    /// ([`with_page_handles`]) and its `page_number`.
    fn page(&mut self);
    /// A `PDFStream` by reference (the streams of a result, when the core
    /// gives them a handle: [`with_value_handles`]), followed by four
    /// values: its handle, `objid`, `genno` and `attrs` (a dict); its data
    /// is read on demand (`crate::streams`).
    fn stream_ref(&mut self);
    /// Writes `rows` (dicts, grouped by key sequence in `shapes`) as one
    /// list value, if this writer has a form for it, and returns whether it
    /// did.
    fn table(&mut self, _rows: &[PyValue], _shapes: &Shapes<'_>) -> PyResult<bool> {
        Ok(false)
    }
}

thread_local! {
    /// The handle of each page a result holds (by [`PageRef::id`]), while
    /// the core writes it ([`with_page_handles`]).
    ///
    /// [`PageRef::id`]: pdfplumber::obj::PageRef::id
    static PAGE_HANDLES: RefCell<FxHashMap<usize, u32>> = RefCell::new(FxHashMap::default());
    /// The handle of each stream a result holds by reference (by
    /// `Rc::as_ptr`), while the core writes it ([`with_value_handles`]).
    static STREAM_HANDLES: RefCell<FxHashMap<usize, u32>> = RefCell::new(FxHashMap::default());
}

/// Runs `f` (the writing of a result) with the handles of the pages the
/// result holds: a page value is written as its handle
/// (`crate::ops::Core::label_pages` made them).
pub fn with_page_handles<T>(handles: FxHashMap<usize, u32>, f: impl FnOnce() -> T) -> T {
    with_value_handles(handles, FxHashMap::default(), f)
}

/// [`with_page_handles`], and the handles of the streams the result holds:
/// a stream with a handle is written by reference ([`Writer::stream_ref`]),
/// any other by value ([`Writer::stream`]).
pub fn with_value_handles<T>(pages: FxHashMap<usize, u32>, streams: FxHashMap<usize, u32>, f: impl FnOnce() -> T) -> T {
    let previous = PAGE_HANDLES.with(|p| std::mem::replace(&mut *p.borrow_mut(), pages));
    let previous_streams = STREAM_HANDLES.with(|p| std::mem::replace(&mut *p.borrow_mut(), streams));
    let out = f();
    PAGE_HANDLES.with(|p| *p.borrow_mut() = previous);
    STREAM_HANDLES.with(|p| *p.borrow_mut() = previous_streams);
    out
}

/// Writes a Python `str` (a lone surrogate included: [`Writer::py_str`]).
pub fn write_str<W: Writer + ?Sized>(w: &mut W, s: &PyStr) -> PyResult<()> {
    match s.as_str() {
        Some(text) => {
            w.str(text);
            Ok(())
        }
        None => w.py_str(PyStrRef::Wtf8(s.as_bytes())),
    }
}

/// The most key sequences a table holds.
pub const MAX_SHAPES: usize = 64;

/// The dicts of a list grouped by key sequence (their *shape*).
pub struct Shapes<'a> {
    /// A dict of each shape, in order of first appearance.
    pub first: Vec<&'a Obj>,
    /// The shape of each row.
    pub order: Vec<u8>,
}

impl Shapes<'_> {
    /// The rows of shape `s`, in order.
    pub fn rows<'r>(&self, items: &'r [PyValue], s: usize) -> Vec<&'r Obj> {
        items
            .iter()
            .zip(&self.order)
            .filter(|(_, &o)| usize::from(o) == s)
            .map(|(v, _)| match v {
                PyValue::Dict(d) => d,
                _ => unreachable!("table rows are dicts"),
            })
            .collect()
    }
}

fn same_keys(a: &Obj, b: &Obj) -> bool {
    let (a, b) = (a.items(), b.items());
    // (PyStr's `==` compares the shared bytes by address first)
    a.len() == b.len() && a.iter().zip(b).all(|((x, _), (y, _))| x == y)
}

/// The shapes of `items` when they are at least two dicts of at most
/// [`MAX_SHAPES`] shapes, at most one shape per two rows (otherwise a table
/// saves nothing).
pub fn shapes(items: &[PyValue]) -> Option<Shapes<'_>> {
    if items.len() < 2 {
        return None;
    }
    let mut first: Vec<&Obj> = Vec::new();
    let mut order = Vec::with_capacity(items.len());
    let mut last = 0;
    for item in items {
        let PyValue::Dict(d) = item else { return None };
        let s = if !first.is_empty() && same_keys(first[last], d) {
            last
        } else if let Some(s) = first.iter().position(|f| same_keys(f, d)) {
            s
        } else {
            if first.len() == MAX_SHAPES {
                return None;
            }
            first.push(d);
            first.len() - 1
        };
        order.push(s as u8);
        last = s;
    }
    (first.len() * 2 <= items.len()).then_some(Shapes { first, order })
}

enum Work<'a> {
    Py(&'a PyValue),
    Pdf(PDFObject),
    Key(&'a PyStr),
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
            Work::Key(k) => write_str(w, k)?,
            Work::PdfKey(k) => w.str(&k),
            Work::Exit(addr) => {
                active.remove(&addr);
            }
            Work::Py(v) => match v {
                PyValue::None => w.none(),
                PyValue::Bool(b) => w.bool(*b),
                PyValue::Int(i) => w.int(i),
                PyValue::Float(x) => w.float(x.get()),
                PyValue::Str(s) => write_str(w, s)?,
                PyValue::Bytes(b) => w.bytes(b),
                PyValue::List(items) | PyValue::Tuple(items) => {
                    if let Some(shapes) = shapes(items) {
                        if w.table(items, &shapes)? {
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
                // a `Page` (`annot["data"]["P"]`): its handle
                PyValue::Page(p) => {
                    let handle = PAGE_HANDLES.with(|h| h.borrow().get(&p.id()).copied()).ok_or_else(|| {
                        PyErr::new(ExcKind::RuntimeError, "a page value without a handle (the core did not label it)")
                    })?;
                    w.page();
                    w.int(&PyInt::from(handle));
                    w.int(p.page_number());
                }
            },
            Work::Pdf(o) => match o {
                PDFObject::Null => w.none(),
                PDFObject::Bool(b) => w.bool(b),
                PDFObject::Int(i) => w.int(&i),
                PDFObject::Float(x) => w.float(x),
                PDFObject::Bytes(b) => w.bytes(&b),
                // a value pdfplumber's metadata decoding wrote into the document
                PDFObject::Str(s) => write_str(w, &s)?,
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
                    let opt_int = |i: Option<PyInt>| i.map_or(PDFObject::Null, PDFObject::Int);
                    // an objid is any hashable object (`PDFXRefFallback`)
                    let objid = s.objid().map_or(PDFObject::Null, |id| id.obj().clone());
                    match STREAM_HANDLES.with(|h| h.borrow().get(&addr).copied()) {
                        Some(handle) => {
                            w.stream_ref();
                            w.int(&PyInt::from(handle));
                        }
                        None => {
                            w.stream();
                            let opt_bytes = |b: Option<Rc<[u8]>>| b.map_or(PDFObject::Null, PDFObject::Bytes);
                            // pushed in reverse: rawdata and data after attrs
                            stack.push(Work::Pdf(opt_bytes(s.data())));
                            stack.push(Work::Pdf(opt_bytes(s.rawdata())));
                        }
                    }
                    // pushed in reverse: objid, genno, attrs
                    stack.push(Work::Pdf(PDFObject::Dict(s.attrs.clone())));
                    stack.push(Work::Pdf(opt_int(s.genno())));
                    stack.push(Work::Pdf(objid));
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
