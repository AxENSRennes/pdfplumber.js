//! Port of `pdfminer/pdftypes.py`: the PDF object model, indirect
//! references, streams and the `*_value` accessors.
//!
//! # Object model
//!
//! [`PDFObject`] is the union of the Python types pdfminer's parsers
//! produce: `None`, `bool`, `int` ([`PyInt`]), `float`, `bytes`,
//! [`PSLiteral`], [`PSKeyword`], `list`, `dict` (keys are `str`),
//! [`PDFStream`] and [`PDFObjRef`]. Python lists, dicts, streams and
//! references are shared, mutable objects; so are [`PDFList`], [`PDFDict`]
//! (`Rc<RefCell<..>>`), `Rc<PDFStream>` and [`PDFObjRef`]: cloning a
//! `PDFObject` clones a reference, and an in-place change (`resolve_all` on a
//! dict, `decipher_all`) is seen through every alias, as in Python.
//!
//! `==` on `PDFObject` is Python's `==`: numbers compare by value across
//! `bool`/`int`/`float`, `bytes` by content, lists and dicts by content
//! (with Python's identity shortcut for elements), literals, keywords,
//! streams and references by identity (no `__eq__` upstream). Python's
//! `repr()` and `str()` of an object are [`PDFObject::repr`] and
//! [`PDFObject::py_str`] (the same text for these types).
//!
//! # Depth
//!
//! The parsers build arbitrarily deep lists and dicts without recursing (a
//! content stream of 10^6 `[` is 2 MB), and Python can hold, compare and
//! free them. So can the port: dropping, comparing and writing an object
//! are iterative, never limited by the Rust stack (which is 1 MiB in a
//! browser). Where Python itself has a limit, the port reproduces it with
//! [`pycompat::recursion`]: `repr()`/`str()` take one C recursion unit per
//! nested object, as CPython's do, and raise the same `RecursionError`
//! ("maximum recursion depth exceeded while getting the repr of an
//! object") at the same depth; `resolve_all` and `decipher_all` take one
//! Python frame per level, like their Python versions.
//!
//! # Indirect references and the document
//!
//! [`PDFObjRef`] resolves through the [`PDFResolver`] trait, which the
//! future `pdfdocument::PDFDocument` implements (`getobj`). Python's
//! `PDFObjRef.doc` is a strong reference; the port holds a `Weak` to avoid an
//! `Rc` cycle (document -> object cache -> objects -> reference -> document).
//! The document must therefore outlive the resolution of its references;
//! resolving through a dropped document raises `ReferenceError`, which is
//! what a dead `weakref.proxy` raises in Python.

use std::cell::{Ref, RefCell};
use std::fmt;
use std::rc::{Rc, Weak};
use std::sync::LazyLock;

use indexmap::IndexMap;
use pycompat::recursion::{self, Guard};
use pycompat::repr::{bytes_repr, float_repr, str_repr};
use pycompat::{exc, zlib, ExcKind, Num, PyErr, PyInt, PyResult, PyStr};
use rustc_hash::FxBuildHasher;

use crate::ascii85::{ascii85decode, asciihexdecode};
use crate::ccitt::ccittfaxdecode_obj;
use crate::lzw::lzwdecode;
use crate::psparser::{PSKeyword, PSLiteral, LIT};
use crate::runlength::rldecode;
use crate::utils::{apply_png_predictor, apply_tiff_predictor};

/// Python `bytes`: immutable and shared.
pub type PyBytes = Rc<[u8]>;

// pdfminer/pdftypes.py:28-39 LITERAL_CRYPT, LITERALS_FLATE_DECODE, LITERALS_LZW_DECODE, LITERALS_ASCII85_DECODE,
// LITERALS_ASCIIHEX_DECODE, LITERALS_RUNLENGTH_DECODE, LITERALS_CCITTFAX_DECODE, LITERALS_DCT_DECODE,
// LITERALS_JBIG2_DECODE, LITERALS_JPX_DECODE
pub static LITERAL_CRYPT: LazyLock<PSLiteral> = LazyLock::new(|| LIT("Crypt"));
pub static LITERALS_FLATE_DECODE: LazyLock<[PSLiteral; 2]> = LazyLock::new(|| [LIT("FlateDecode"), LIT("Fl")]);
pub static LITERALS_LZW_DECODE: LazyLock<[PSLiteral; 2]> = LazyLock::new(|| [LIT("LZWDecode"), LIT("LZW")]);
pub static LITERALS_ASCII85_DECODE: LazyLock<[PSLiteral; 2]> = LazyLock::new(|| [LIT("ASCII85Decode"), LIT("A85")]);
pub static LITERALS_ASCIIHEX_DECODE: LazyLock<[PSLiteral; 2]> = LazyLock::new(|| [LIT("ASCIIHexDecode"), LIT("AHx")]);
pub static LITERALS_RUNLENGTH_DECODE: LazyLock<[PSLiteral; 2]> = LazyLock::new(|| [LIT("RunLengthDecode"), LIT("RL")]);
pub static LITERALS_CCITTFAX_DECODE: LazyLock<[PSLiteral; 2]> = LazyLock::new(|| [LIT("CCITTFaxDecode"), LIT("CCF")]);
pub static LITERALS_DCT_DECODE: LazyLock<[PSLiteral; 2]> = LazyLock::new(|| [LIT("DCTDecode"), LIT("DCT")]);
pub static LITERALS_JBIG2_DECODE: LazyLock<[PSLiteral; 1]> = LazyLock::new(|| [LIT("JBIG2Decode")]);
pub static LITERALS_JPX_DECODE: LazyLock<[PSLiteral; 1]> = LazyLock::new(|| [LIT("JPXDecode")]);

/// `x in (LIT(..), LIT(..))`: true when `x` is one of the literals (a
/// literal equals only itself; no other type equals a literal).
pub fn in_literals(x: &PDFObject, literals: &[PSLiteral]) -> bool {
    matches!(x, PDFObject::Literal(lit) if literals.contains(lit))
}

// ---------------------------------------------------------------------------
// Objects

/// A PDF object (see the module documentation).
#[derive(Clone)]
pub enum PDFObject {
    Null,
    Bool(bool),
    Int(PyInt),
    Float(f64),
    Bytes(PyBytes),
    Literal(PSLiteral),
    Keyword(PSKeyword),
    List(PDFList),
    Dict(PDFDict),
    Stream(Rc<PDFStream>),
    Ref(PDFObjRef),
    /// A Python `str`. pdfminer's parsers never make one: pdfplumber's
    /// metadata decoding (pdfplumber/utils/pdfinternals.py:24
    /// `resolve_and_decode`) writes its decoded values into the
    /// document's own dicts, in place, where everything that reads the
    /// document afterwards finds them (see `pdfplumber::pdf`, "Metadata").
    Str(PyStr),
}

/// A Python `list` of objects.
#[derive(Clone, Default)]
pub struct PDFList(Rc<RefCell<Vec<PDFObject>>>);

/// The storage of a [`PDFDict`]: insertion-ordered, assigning an existing
/// key keeps its position, like a Python `dict`.
pub type DictMap = IndexMap<Rc<str>, PDFObject, FxBuildHasher>;

/// A Python `dict` with `str` keys.
#[derive(Clone, Default)]
pub struct PDFDict(Rc<RefCell<DictMap>>);

impl PDFList {
    pub fn new(items: Vec<PDFObject>) -> PDFList {
        PDFList(Rc::new(RefCell::new(items)))
    }

    pub fn borrow(&self) -> Ref<'_, Vec<PDFObject>> {
        self.0.borrow()
    }

    pub fn len(&self) -> usize {
        self.0.borrow().len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.borrow().is_empty()
    }

    /// `l[i]` for `0 <= i < len(l)`.
    pub fn get(&self, i: usize) -> Option<PDFObject> {
        self.0.borrow().get(i).cloned()
    }

    /// A copy of the items (`list(l)`), for iterating while calling code
    /// that may touch the list.
    pub fn to_vec(&self) -> Vec<PDFObject> {
        self.0.borrow().clone()
    }

    /// `l.append(x)`
    pub fn push(&self, x: PDFObject) {
        self.0.borrow_mut().push(x);
    }

    /// `l[i] = x`
    pub fn set(&self, i: usize, x: PDFObject) {
        self.0.borrow_mut()[i] = x;
    }

    /// `a is b`
    pub fn ptr_eq(&self, other: &PDFList) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl PDFDict {
    pub fn new() -> PDFDict {
        PDFDict::default()
    }

    /// `dict(pairs)`: a later duplicate key replaces the value in place.
    pub fn from_pairs(pairs: impl IntoIterator<Item = (Rc<str>, PDFObject)>) -> PDFDict {
        let d = PDFDict::new();
        for (k, v) in pairs {
            d.insert(k, v);
        }
        d
    }

    pub fn borrow(&self) -> Ref<'_, DictMap> {
        self.0.borrow()
    }

    pub fn len(&self) -> usize {
        self.0.borrow().len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.borrow().is_empty()
    }

    /// `d.get(key)`
    pub fn get(&self, key: &str) -> Option<PDFObject> {
        self.0.borrow().get(key).cloned()
    }

    /// `key in d`
    pub fn contains_key(&self, key: &str) -> bool {
        self.0.borrow().contains_key(key)
    }

    /// `d[key] = value`
    pub fn insert(&self, key: Rc<str>, value: PDFObject) {
        self.0.borrow_mut().insert(key, value);
    }

    /// `d.pop(key, None)` / `del d[key]`: later keys keep their order.
    pub fn remove(&self, key: &str) -> Option<PDFObject> {
        self.0.borrow_mut().shift_remove(key)
    }

    /// `list(d.keys())`
    pub fn keys(&self) -> Vec<Rc<str>> {
        self.0.borrow().keys().cloned().collect()
    }

    /// The `i`-th item in insertion order (how a `for` loop over
    /// `d.items()` reads the dict as it goes).
    pub fn get_index(&self, i: usize) -> Option<(Rc<str>, PDFObject)> {
        self.0.borrow().get_index(i).map(|(k, v)| (k.clone(), v.clone()))
    }

    /// `list(d.items())`
    pub fn items(&self) -> Vec<(Rc<str>, PDFObject)> {
        self.0.borrow().iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }

    /// `d.copy()`: a new dict with the same items.
    pub fn copy(&self) -> PDFDict {
        PDFDict(Rc::new(RefCell::new(self.0.borrow().clone())))
    }

    /// `a is b`
    pub fn ptr_eq(&self, other: &PDFDict) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl From<Vec<PDFObject>> for PDFList {
    fn from(items: Vec<PDFObject>) -> PDFList {
        PDFList::new(items)
    }
}

// Freeing an object frees what only it holds without recursing: the
// elements of a list, dict or stream dictionary dropped last are moved to a
// work list first (what CPython's trashcan does for deep containers).

impl Drop for PDFList {
    fn drop(&mut self) {
        if let Some(items) = Rc::get_mut(&mut self.0) {
            drop_iteratively(std::mem::take(items.get_mut()));
        }
    }
}

impl Drop for PDFDict {
    fn drop(&mut self) {
        if let Some(map) = Rc::get_mut(&mut self.0) {
            drop_iteratively(std::mem::take(map.get_mut()).into_values().collect());
        }
    }
}

/// Drops `objects` and everything only they hold, depth first, with a work
/// list instead of the call stack.
fn drop_iteratively(mut stack: Vec<PDFObject>) {
    while let Some(obj) = stack.pop() {
        match obj {
            PDFObject::List(mut l) => {
                if let Some(items) = Rc::get_mut(&mut l.0) {
                    stack.append(items.get_mut());
                }
            }
            PDFObject::Dict(mut d) => {
                if let Some(map) = Rc::get_mut(&mut d.0) {
                    stack.extend(std::mem::take(map.get_mut()).into_values());
                }
            }
            PDFObject::Stream(s) => {
                if let Ok(stream) = Rc::try_unwrap(s) {
                    stack.push(PDFObject::Dict(stream.attrs));
                }
            }
            _ => {}
        }
        // A container is empty here (or still shared): dropping it is shallow.
    }
}

impl PDFObject {
    /// `type(x).__name__`, as in Python's error messages.
    pub fn type_name(&self) -> &'static str {
        match self {
            PDFObject::Null => "NoneType",
            PDFObject::Bool(_) => "bool",
            PDFObject::Int(_) => "int",
            PDFObject::Float(_) => "float",
            PDFObject::Bytes(_) => "bytes",
            PDFObject::Literal(_) => "PSLiteral",
            PDFObject::Keyword(_) => "PSKeyword",
            PDFObject::List(_) => "list",
            PDFObject::Dict(_) => "dict",
            PDFObject::Stream(_) => "PDFStream",
            PDFObject::Ref(_) => "PDFObjRef",
            PDFObject::Str(_) => "str",
        }
    }

    /// The number, for `isinstance(x, (int, float))` (a `bool` is an
    /// `int`).
    pub fn as_num(&self) -> Option<Num> {
        match self {
            PDFObject::Bool(b) => Some(Num::Int(PyInt::Small(i64::from(*b)))),
            PDFObject::Int(i) => Some(Num::Int(i.clone())),
            PDFObject::Float(f) => Some(Num::Float(*f)),
            _ => None,
        }
    }

    /// `bool(x)`: Python truthiness. Objects without `__bool__`/`__len__`
    /// (literals, keywords, streams, references) are true.
    pub fn is_truthy(&self) -> bool {
        match self {
            PDFObject::Null => false,
            PDFObject::Bool(b) => *b,
            PDFObject::Int(i) => !i.is_zero(),
            PDFObject::Float(f) => *f != 0.0,
            PDFObject::Bytes(b) => !b.is_empty(),
            PDFObject::List(l) => !l.is_empty(),
            PDFObject::Dict(d) => !d.is_empty(),
            PDFObject::Str(s) => !s.is_empty(),
            PDFObject::Literal(_) | PDFObject::Keyword(_) | PDFObject::Stream(_) | PDFObject::Ref(_) => true,
        }
    }

    /// `repr(x)` as `PyObject_Repr` (the `!r` of an f-string): raises
    /// `RecursionError` where CPython runs out of C recursion units (see the
    /// module documentation), and `ValueError` for an int of more than
    /// 4 300 digits. A call of the builtin `repr(x)` takes one more unit,
    /// the caller's (`pycompat::recursion`).
    pub fn repr(&self) -> PyResult<String> {
        Renderer::render(Item::Obj(self.clone()), Conv::Repr)
    }

    /// `str(x)`: the same text as [`Self::repr`], but entered through
    /// `PyObject_Str` (whose `RecursionError` says "while getting the str of
    /// an object").
    pub fn py_str(&self) -> PyResult<String> {
        Renderer::render(Item::Obj(self.clone()), Conv::Str)
    }

    /// `repr(list(objs))`
    pub fn list_repr(objs: &[PDFObject]) -> PyResult<String> {
        PDFObject::List(PDFList::new(objs.to_vec())).repr()
    }

    /// `"Predictor" in x` for a `str` key: `TypeError` where Python's `in`
    /// raises one.
    pub fn py_contains_str(&self, key: &str) -> PyResult<bool> {
        match self {
            PDFObject::Dict(d) => Ok(d.contains_key(key)),
            PDFObject::Stream(s) => Ok(s.contains(key)),
            // only a str element equals a str (one that pdfplumber's metadata
            // decoding wrote)
            PDFObject::List(l) => Ok(l.to_vec().iter().any(|x| matches!(x, PDFObject::Str(s) if s.as_str() == Some(key)))),
            // a substring
            PDFObject::Str(s) => Ok(s.contains(&PyStr::new(key))),
            PDFObject::Bytes(_) => Err(exc::TypeError("a bytes-like object is required, not 'str'")),
            other => Err(exc::TypeError(format!("argument of type '{}' is not iterable", other.type_name()))),
        }
    }

    /// `x[0]`, as evaluated on the `/F` entry of an inline image.
    pub fn py_getitem_index0(&self) -> PyResult<PDFObject> {
        match self {
            PDFObject::List(l) => l.get(0).ok_or_else(|| exc::IndexError("list index out of range")),
            PDFObject::Bytes(b) => match b.first() {
                Some(&c) => Ok(PDFObject::Int(PyInt::Small(i64::from(c)))),
                None => Err(exc::IndexError("index out of range")),
            },
            // dict[0]: str(KeyError(0)) is repr(0)
            PDFObject::Dict(_) => Err(exc::KeyError("0")),
            PDFObject::Str(s) => match s.code_points().next() {
                Some(c) => Ok(PDFObject::Str(PyStr::from_code_point(c))),
                None => Err(exc::IndexError("string index out of range")),
            },
            // PDFStream.__getitem__ looks the key up in attrs
            PDFObject::Stream(s) => Err(s.missing_attribute("0")),
            other => Err(exc::TypeError(format!("'{}' object is not subscriptable", other.type_name()))),
        }
    }
}

// ---------------------------------------------------------------------------
// repr() and str()

const WHILE_REPR: &str = " while getting the repr of an object";
const WHILE_STR: &str = " while getting the str of an object";
const WHILE_CALLING: &str = " while calling a Python object";

/// A `__repr__` written in Python, run by C (`slot_tp_repr`): the
/// evaluation loop entry, then the method's frame.
fn python_method() -> PyResult<(Guard, Guard)> {
    let entry = recursion::eval_entry()?;
    Ok((entry, recursion::py_frame()?))
}

/// How an object is entered: `PyObject_Repr` or `PyObject_Str`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Conv {
    Repr,
    Str,
}

/// What [`Renderer`] writes.
enum Item {
    Obj(PDFObject),
    /// The fields of `<PDFStream(...): ...>` after the `PyObject_Repr` unit
    /// of the stream.
    Stream(Rc<PDFStream>),
}

/// A step of the depth-first walk of [`Renderer`].
enum Task {
    Visit(Item, Conv),
    Text(&'static str),
    Owned(String),
    /// Leaves C units or a `__repr__` frame (the guard drops).
    Leave(Guard),
    /// `Py_ReprLeave` of the container being written.
    ReprLeave,
}

/// `repr()`/`str()` of PDF objects, iterative, taking the C recursion units
/// CPython takes at each level (`Objects/object.c` `PyObject_Repr` and
/// `PyObject_Str` take one; a `__repr__` written in Python is entered from C,
/// two more and a frame). CPython writes a container being written again
/// (a cycle, `Py_ReprEnter`) as `[...]` or `{...}`.
struct Renderer {
    out: String,
    tasks: Vec<Task>,
    /// The containers being written (`Py_ReprEnter`), by address.
    active: Vec<usize>,
}

impl Renderer {
    fn render(item: Item, conv: Conv) -> PyResult<String> {
        let mut r = Renderer { out: String::new(), tasks: vec![Task::Visit(item, conv)], active: Vec::new() };
        while let Some(task) = r.tasks.pop() {
            match task {
                Task::Visit(item, conv) => r.visit(item, conv)?,
                Task::Text(t) => r.out.push_str(t),
                Task::Owned(t) => r.out.push_str(&t),
                Task::Leave(guard) => drop(guard),
                Task::ReprLeave => {
                    r.active.pop();
                }
            }
        }
        Ok(r.out)
    }

    /// `PyObject_Repr(x)` or `PyObject_Str(x)`: one unit while `x` is
    /// written.
    fn visit(&mut self, item: Item, conv: Conv) -> PyResult<()> {
        // PyObject_Str of a str is the str itself (no unit: PyUnicode_CheckExact
        // comes first)
        if let (Item::Obj(PDFObject::Str(s)), Conv::Str) = (&item, conv) {
            self.out.push_str(&s.to_string_lossy());
            return Ok(());
        }
        let unit = recursion::c_call(if conv == Conv::Repr { WHILE_REPR } else { WHILE_STR })?;
        let obj = match item {
            Item::Obj(obj) => obj,
            Item::Stream(s) => return self.stream(&s, unit),
        };
        match obj {
            PDFObject::Null => self.out.push_str("None"),
            PDFObject::Bool(b) => self.out.push_str(if b { "True" } else { "False" }),
            PDFObject::Int(i) => self.out.push_str(&i.py_str()?),
            PDFObject::Float(f) => self.out.push_str(&float_repr(f)),
            PDFObject::Bytes(b) => self.out.push_str(&bytes_repr(&b)),
            // pdfminer/psparser.py:50 PSLiteral.__repr__, :69 PSKeyword.__repr__:
            // f"/{name!r}", a Python method called by C
            PDFObject::Literal(lit) => {
                let _method = python_method()?;
                let _name = recursion::c_call(WHILE_REPR)?;
                self.out.push_str(&lit.repr());
            }
            PDFObject::Keyword(kwd) => {
                let _method = python_method()?;
                let _name = recursion::c_call(WHILE_REPR)?;
                self.out.push_str(&kwd.repr());
            }
            // pdfminer/pdftypes.py:96 PDFObjRef.__repr__: f"<PDFObjRef:{self.objid}>",
            // whose format() of an int is str()
            PDFObject::Ref(r) => {
                let _method = python_method()?;
                let _objid = recursion::c_call(WHILE_STR)?;
                self.out.push_str(&format!("<PDFObjRef:{}>", r.objid().py_str()?));
            }
            PDFObject::Stream(s) => return self.stream(&s, unit),
            PDFObject::Str(s) => self.out.push_str(&s.repr()),
            PDFObject::List(l) => {
                let items = l.to_vec();
                if items.is_empty() {
                    self.out.push_str("[]");
                } else if !self.enter(Rc::as_ptr(&l.0) as *const () as usize) {
                    self.out.push_str("[...]");
                } else {
                    self.out.push('[');
                    self.tasks.extend([Task::ReprLeave, Task::Leave(unit), Task::Text("]")]);
                    for (i, x) in items.into_iter().enumerate().rev() {
                        self.tasks.push(Task::Visit(Item::Obj(x), Conv::Repr));
                        if i > 0 {
                            self.tasks.push(Task::Text(", "));
                        }
                    }
                    return Ok(());
                }
            }
            PDFObject::Dict(d) => {
                if !self.enter(Rc::as_ptr(&d.0) as *const () as usize) {
                    self.out.push_str("{...}");
                } else if d.is_empty() {
                    self.active.pop();
                    self.out.push_str("{}");
                } else {
                    self.out.push('{');
                    self.tasks.extend([Task::ReprLeave, Task::Leave(unit), Task::Text("}")]);
                    // Each key's repr (a str) takes a unit at the depth of
                    // the values, before its value: if it fails, it fails for
                    // the first key, before any value is written.
                    drop(recursion::c_call(WHILE_REPR)?);
                    for (i, (k, x)) in d.items().into_iter().enumerate().rev() {
                        self.tasks.push(Task::Visit(Item::Obj(x), Conv::Repr));
                        self.tasks.push(Task::Owned(format!("{}: ", str_repr(&k))));
                        if i > 0 {
                            self.tasks.push(Task::Text(", "));
                        }
                    }
                    return Ok(());
                }
            }
        }
        drop(unit);
        Ok(())
    }

    /// `Py_ReprEnter`: false when the container is already being written.
    fn enter(&mut self, address: usize) -> bool {
        if self.active.contains(&address) {
            return false;
        }
        self.active.push(address);
        true
    }

    // pdfminer/pdftypes.py:262 PDFStream.__repr__
    /// `f"<PDFStream({self.objid!r}): len={len(self.data)}, {self.attrs!r}>"`
    /// (`raw=` and the raw length before decoding), a Python method called
    /// by C.
    fn stream(&mut self, s: &PDFStream, unit: Guard) -> PyResult<()> {
        let method = python_method()?;
        let objid = match s.objid() {
            // PyObject_Repr of another id ([`ObjId`]) takes its own unit
            Some(id) if id.as_int().is_none() => id.repr()?,
            id => {
                let _unit = recursion::c_call(WHILE_REPR)?;
                match id.as_ref().and_then(ObjId::as_int) {
                    Some(i) => i.py_str()?,
                    None => "None".to_owned(),
                }
            }
        };
        // len(self.data): a builtin call
        drop(recursion::c_call(WHILE_CALLING)?);
        let size = match &*s.data.borrow() {
            Some(data) => format!("len={}", data.len()),
            None => format!("raw={}", s.rawdata.borrow().as_ref().map_or(0, |r| r.len())),
        };
        self.out.push_str(&format!("<PDFStream({objid}): {size}, "));
        self.tasks.extend([
            Task::Leave(unit),
            Task::Leave(method.1),
            Task::Leave(method.0),
            Task::Text(">"),
            Task::Visit(Item::Obj(PDFObject::Dict(s.attrs.clone())), Conv::Repr),
        ]);
        Ok(())
    }
}

impl fmt::Debug for PDFObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.repr() {
            Ok(text) => f.write_str(&text),
            Err(e) => write!(f, "<{e}>"),
        }
    }
}

/// `x is y` for the objects that have an identity in the model.
fn same_object(a: &PDFObject, b: &PDFObject) -> bool {
    match (a, b) {
        (PDFObject::List(x), PDFObject::List(y)) => x.ptr_eq(y),
        (PDFObject::Dict(x), PDFObject::Dict(y)) => x.ptr_eq(y),
        (PDFObject::Stream(x), PDFObject::Stream(y)) => Rc::ptr_eq(x, y),
        (PDFObject::Ref(x), PDFObject::Ref(y)) => x == y,
        (PDFObject::Literal(x), PDFObject::Literal(y)) => x == y,
        (PDFObject::Keyword(x), PDFObject::Keyword(y)) => x == y,
        (PDFObject::Null, PDFObject::Null) => true,
        _ => false,
    }
}

impl PartialEq for PDFObject {
    /// Python's `==` (see the module documentation), iterative. CPython's
    /// comparison of nested containers takes a C recursion unit per level
    /// ("maximum recursion depth exceeded in comparison"); no ported upstream
    /// code compares containers, so that limit is not modeled here.
    fn eq(&self, other: &PDFObject) -> bool {
        let mut pending = vec![(self.clone(), other.clone())];
        while let Some((a, b)) = pending.pop() {
            if let (Some(x), Some(y)) = (a.as_num(), b.as_num()) {
                if !x.py_eq(&y) {
                    return false;
                }
                continue;
            }
            match (&a, &b) {
                (PDFObject::Bytes(x), PDFObject::Bytes(y)) => {
                    if x != y {
                        return false;
                    }
                }
                (PDFObject::Str(x), PDFObject::Str(y)) => {
                    if x != y {
                        return false;
                    }
                }
                (PDFObject::List(x), PDFObject::List(y)) => {
                    if x.ptr_eq(y) {
                        continue;
                    }
                    let (x, y) = (x.to_vec(), y.to_vec());
                    if x.len() != y.len() {
                        return false;
                    }
                    // PyObject_RichCompareBool: identity first
                    pending.extend(x.into_iter().zip(y).filter(|(p, q)| !same_object(p, q)));
                }
                (PDFObject::Dict(x), PDFObject::Dict(y)) => {
                    if x.ptr_eq(y) {
                        continue;
                    }
                    if x.len() != y.len() {
                        return false;
                    }
                    for (k, p) in x.items() {
                        match y.get(&k) {
                            None => return false,
                            Some(q) if !same_object(&p, &q) => pending.push((p, q)),
                            Some(_) => {}
                        }
                    }
                }
                _ => {
                    if !same_object(&a, &b) {
                        return false;
                    }
                }
            }
        }
        true
    }
}

// ---------------------------------------------------------------------------
// PDFObjRef

/// What a [`PDFObjRef`] resolves through: `PDFDocument.getobj`.
pub trait PDFResolver {
    /// `doc.getobj(objid)`; raises `PDFObjectNotFound` for a missing object.
    fn getobj(&self, objid: &PyInt) -> PyResult<PDFObject>;
}

struct PDFObjRefInner {
    doc: Option<Weak<dyn PDFResolver>>,
    objid: PyInt,
}

/// pdfminer/pdftypes.py:69 `PDFObjRef`: a reference to an indirect object.
/// Equality is identity (no `__eq__` upstream): two references to the
/// same object number are different objects.
#[derive(Clone)]
pub struct PDFObjRef(Rc<PDFObjRefInner>);

impl PartialEq for PDFObjRef {
    fn eq(&self, other: &PDFObjRef) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl PDFObjRef {
    // pdfminer/pdftypes.py:70 PDFObjRef.__init__
    /// `PDFObjRef(doc, objid)`; `doc` is `None` before a parser is attached
    /// to a document. (With `STRICT = False`, an object id of 0 is
    /// accepted.)
    pub fn new(doc: Option<Weak<dyn PDFResolver>>, objid: PyInt) -> PDFObjRef {
        PDFObjRef(Rc::new(PDFObjRefInner { doc, objid }))
    }

    /// `self.objid`
    pub fn objid(&self) -> &PyInt {
        &self.0.objid
    }

    /// `self.doc`
    pub fn doc(&self) -> Option<&Weak<dyn PDFResolver>> {
        self.0.doc.as_ref()
    }

    // pdfminer/pdftypes.py:99 PDFObjRef.resolve
    /// `self.resolve(default)`, a Python frame (the document's `getobj`
    /// counts its own).
    pub fn resolve(&self, default: &PDFObject) -> PyResult<PDFObject> {
        let _frame = recursion::py_frame()?;
        let Some(doc) = &self.0.doc else {
            return Err(exc::AssertionError(""));
        };
        let doc = doc.upgrade().ok_or_else(|| exc::ReferenceError("weakly-referenced object no longer exists"))?;
        match doc.getobj(&self.0.objid) {
            Err(e) if e.is_instance(ExcKind::PDFObjectNotFound) => Ok(default.clone()),
            other => other,
        }
    }
}

// ---------------------------------------------------------------------------
// Object ids

/// A Python object used as an object id: an argument of
/// `PDFDocument.getobj`, a key of `PDFXRef.offsets` and of the document's
/// caches, `PDFStream.objid`.
///
/// Upstream's object ids are ints, except where `PDFXRefFallback` stores the
/// object an object stream holds at an id position as it is (`5.5`, `True`,
/// a name, a reference...) and where `PDFXRefStream` yields `start + i` for
/// whatever `start` its `/Index` holds: any hashable value can reach
/// `getobj`, which stores it in the stream it returns (`set_objid`). `==`
/// and `hash()` are Python's (`docs/architecture.md`, "Résolution des
/// références"): numbers by value across `bool`/`int`/`float`
/// (`1 == 1.0 == True`), `bytes` by content, `None` equal to itself,
/// literals, keywords, references and streams by identity (no `__eq__`
/// upstream). A list or a dict is unhashable ([`ObjId::new`]). The id keeps
/// the object it was made from (`5.0` stays a float in a trace), as a dict
/// keeps its first key.
#[derive(Clone)]
pub struct ObjId(PDFObject);

impl ObjId {
    /// The id of `obj`, or `TypeError("unhashable type: 'list'")` (what
    /// `hash(obj)` raises).
    pub fn new(obj: PDFObject) -> PyResult<ObjId> {
        match obj {
            PDFObject::List(_) | PDFObject::Dict(_) => {
                Err(exc::TypeError(format!("unhashable type: '{}'", obj.type_name())))
            }
            obj => Ok(ObjId(obj)),
        }
    }

    /// The id of an int.
    pub fn int(i: PyInt) -> ObjId {
        ObjId(PDFObject::Int(i))
    }

    /// The object.
    pub fn obj(&self) -> &PDFObject {
        &self.0
    }

    /// The id when it is an `int` (not a `bool`).
    pub fn as_int(&self) -> Option<&PyInt> {
        match &self.0 {
            PDFObject::Int(i) => Some(i),
            _ => None,
        }
    }

    /// `str(objid)`
    pub fn py_str(&self) -> PyResult<String> {
        self.0.py_str()
    }

    /// `repr(objid)`
    pub fn repr(&self) -> PyResult<String> {
        self.0.repr()
    }
}

impl From<PyInt> for ObjId {
    fn from(i: PyInt) -> ObjId {
        ObjId::int(i)
    }
}

impl PartialEq for ObjId {
    fn eq(&self, other: &ObjId) -> bool {
        match (self.0.as_num(), other.0.as_num()) {
            (Some(a), Some(b)) => a.py_eq(&b),
            (Some(_), None) | (None, Some(_)) => false,
            (None, None) => match (&self.0, &other.0) {
                (PDFObject::Bytes(a), PDFObject::Bytes(b)) => a == b,
                (PDFObject::Str(a), PDFObject::Str(b)) => a == b,
                (a, b) => same_object(a, b),
            },
        }
    }
}

impl Eq for ObjId {}

impl std::hash::Hash for ObjId {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        if let Some(n) = self.0.as_num() {
            // A NaN cannot be parsed (the tokenizer reads no "nan"); its hash
            // would be its identity: any constant keeps Hash and Eq
            // consistent.
            state.write_i64(n.py_hash().unwrap_or(0));
            return;
        }
        match &self.0 {
            PDFObject::Null => state.write_u8(0),
            PDFObject::Bytes(b) => {
                state.write_u8(1);
                b.hash(state);
            }
            PDFObject::Literal(lit) => {
                state.write_u8(2);
                lit.hash(state);
            }
            PDFObject::Keyword(kwd) => {
                state.write_u8(3);
                kwd.hash(state);
            }
            PDFObject::Ref(r) => {
                state.write_u8(4);
                std::ptr::hash(Rc::as_ptr(&r.0), state);
            }
            PDFObject::Stream(s) => {
                state.write_u8(5);
                std::ptr::hash(Rc::as_ptr(s), state);
            }
            PDFObject::Str(s) => {
                state.write_u8(6);
                s.hash(state);
            }
            _ => unreachable!("numbers are hashed above; lists and dicts are not ids"),
        }
    }
}

impl fmt::Debug for ObjId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

// pdfminer/pdftypes.py:107 resolve1
/// Resolves an object. If this is an array or dictionary, it may still
/// contain some indirect objects inside. Takes its Python frame (as do the
/// accessors below, which call it).
pub fn resolve1(x: &PDFObject, default: &PDFObject) -> PyResult<PDFObject> {
    let _frame = recursion::py_frame()?;
    let mut x = x.clone();
    while let PDFObject::Ref(r) = &x {
        x = r.resolve(default)?;
    }
    Ok(x)
}

/// `resolve1(x)` with the default `None`.
pub fn resolve1_none(x: &PDFObject) -> PyResult<PDFObject> {
    resolve1(x, &PDFObject::Null)
}

// pdfminer/pdftypes.py:118 resolve_all
/// Recursively resolves the given object and all the internals: lists are
/// rebuilt, dicts are updated in place. One Python frame per level (and the
/// frames of `resolve()` and of the document's `getobj`), so a structure
/// nested too deep raises `RecursionError` where Python does.
pub fn resolve_all(x: &PDFObject, default: &PDFObject) -> PyResult<PDFObject> {
    let _frame = recursion::py_frame()?;
    let mut x = x.clone();
    while let PDFObject::Ref(r) = &x {
        x = r.resolve(default)?;
    }
    match &x {
        PDFObject::List(l) => {
            // [resolve_all(v, default=default) for v in x] (inlined, no frame)
            let mut items = Vec::with_capacity(l.len());
            let mut i = 0;
            while let Some(v) = l.get(i) {
                items.push(resolve_all(&v, default)?);
                i += 1;
            }
            Ok(PDFObject::List(PDFList::new(items)))
        }
        PDFObject::Dict(d) => {
            // for k, v in x.items(): x[k] = ...; the items are read as the
            // loop reaches them (a nested call may have replaced a value)
            let mut i = 0;
            while let Some((k, v)) = d.get_index(i) {
                let v = resolve_all(&v, default)?;
                d.insert(k, v);
                i += 1;
            }
            Ok(x)
        }
        _ => Ok(x),
    }
}

/// `decipher(objid, genno, data, attrs)`: the document's decryption
/// callback (`DecipherCallable`). `objid` is whatever `getobj` was called
/// with, or a stream's `objid` ([`ObjId`]).
pub type DecipherCallable = Rc<dyn Fn(&ObjId, &PyInt, &[u8], Option<&PDFDict>) -> PyResult<Vec<u8>>>;

// pdfminer/pdftypes.py:134 decipher_all
/// Recursively deciphers the given object: lists are rebuilt, dicts are
/// updated in place. One Python frame per level (the callback counts its
/// own).
pub fn decipher_all(decipher: &DecipherCallable, objid: &ObjId, genno: &PyInt, x: &PDFObject) -> PyResult<PDFObject> {
    let _frame = recursion::py_frame()?;
    match x {
        PDFObject::Bytes(b) => {
            if b.is_empty() {
                return Ok(x.clone());
            }
            Ok(PDFObject::Bytes(decipher(objid, genno, b, None)?.into()))
        }
        PDFObject::List(l) => {
            let mut items = Vec::with_capacity(l.len());
            let mut i = 0;
            while let Some(v) = l.get(i) {
                items.push(decipher_all(decipher, objid, genno, &v)?);
                i += 1;
            }
            Ok(PDFObject::List(PDFList::new(items)))
        }
        PDFObject::Dict(d) => {
            let mut i = 0;
            while let Some((k, v)) = d.get_index(i) {
                let v = decipher_all(decipher, objid, genno, &v)?;
                d.insert(k, v);
                i += 1;
            }
            Ok(x.clone())
        }
        _ => Ok(x.clone()),
    }
}

// pdfminer/pdftypes.py:148 int_value
/// `int_value(x)` with `STRICT = False`, as Python returns it: the `int`,
/// a `bool` (an `int` subclass, which passes the check), or the int 0.
pub fn int_value_obj(x: &PDFObject) -> PyResult<PDFObject> {
    let _frame = recursion::py_frame()?;
    Ok(match resolve1_none(x)? {
        x @ (PDFObject::Int(_) | PDFObject::Bool(_)) => x,
        // With settings.STRICT, PDFTypeError(f"Integer required: {x!r}").
        _ => PDFObject::Int(PyInt::ZERO),
    })
}

/// [`int_value_obj`] as a number: a `bool` becomes 0/1, which only differs
/// from Python's `True`/`False` in `repr()` and type checks (use
/// [`int_value_obj`] where the value is shown).
pub fn int_value(x: &PDFObject) -> PyResult<PyInt> {
    Ok(int_of(&int_value_obj(x)?))
}

/// The value of an `int` or a `bool` (what [`int_value_obj`] returns) as a
/// number (not a Python call).
pub fn int_of(x: &PDFObject) -> PyInt {
    match x {
        PDFObject::Int(i) => i.clone(),
        PDFObject::Bool(b) => PyInt::Small(i64::from(*b)),
        _ => unreachable!("int_value_obj returns an int or a bool"),
    }
}

// pdfminer/pdftypes.py:157 float_value
/// `float_value(x)` with `STRICT = False`: the float, or 0.0 (an int is not
/// a float here).
pub fn float_value(x: &PDFObject) -> PyResult<f64> {
    let _frame = recursion::py_frame()?;
    Ok(match resolve1_none(x)? {
        PDFObject::Float(f) => f,
        _ => 0.0,
    })
}

// pdfminer/pdftypes.py:166 num_value
/// `num_value(x)` with `STRICT = False`: the int or float, or the int 0. A
/// `bool` is returned as 0/1 (see [`int_value`]).
pub fn num_value(x: &PDFObject) -> PyResult<Num> {
    let _frame = recursion::py_frame()?;
    Ok(resolve1_none(x)?.as_num().unwrap_or(Num::Int(PyInt::ZERO)))
}

// pdfminer/pdftypes.py:175 uint_value
/// Resolves a number and interprets it as a two's-complement unsigned
/// number: `int_value(x)` when it is positive (a `True` stays `True`), else
/// that value plus `2**n_bits` (an int).
pub fn uint_value(x: &PDFObject, n_bits: u32) -> PyResult<PDFObject> {
    let _frame = recursion::py_frame()?;
    let xi = int_value_obj(x)?;
    let value = int_of(&xi);
    if value > PyInt::ZERO {
        return Ok(xi);
    }
    let two_pow = PyInt::Small(2).pow(&PyInt::from(n_bits))?.expect("non-negative exponent");
    Ok(PDFObject::Int(value.add(&two_pow)))
}

// pdfminer/pdftypes.py:184 str_value
/// `str_value(x)` with `STRICT = False`: the bytes, or `b""`.
pub fn str_value(x: &PDFObject) -> PyResult<PyBytes> {
    let _frame = recursion::py_frame()?;
    Ok(match resolve1_none(x)? {
        PDFObject::Bytes(b) => b,
        _ => Rc::from(&[][..]),
    })
}

// pdfminer/pdftypes.py:193 list_value
/// `list_value(x)` with `STRICT = False`: the list, or a new empty list.
pub fn list_value(x: &PDFObject) -> PyResult<PDFList> {
    let _frame = recursion::py_frame()?;
    Ok(match resolve1_none(x)? {
        PDFObject::List(l) => l,
        _ => PDFList::default(),
    })
}

// pdfminer/pdftypes.py:202 dict_value
/// `dict_value(x)` with `STRICT = False`: the dict, or a new empty dict.
pub fn dict_value(x: &PDFObject) -> PyResult<PDFDict> {
    let _frame = recursion::py_frame()?;
    Ok(match resolve1_none(x)? {
        PDFObject::Dict(d) => d,
        _ => PDFDict::new(),
    })
}

// pdfminer/pdftypes.py:212 stream_value
/// `stream_value(x)` with `STRICT = False`: the stream, or a new empty
/// stream `PDFStream({}, b"")`.
pub fn stream_value(x: &PDFObject) -> PyResult<Rc<PDFStream>> {
    let _frame = recursion::py_frame()?;
    Ok(match resolve1_none(x)? {
        PDFObject::Stream(s) => s,
        _ => {
            // PDFStream({}, b""): its __init__, called from C
            recursion::c_called_leaf()?;
            Rc::new(PDFStream::new(PDFDict::new(), Rc::from(&[][..]), None))
        }
    })
}

// ---------------------------------------------------------------------------
// Flate fallback

/// `logger = logging.getLogger(__name__)` of `pdfminer/pdftypes.py`.
static LOG: pycompat::logging::Logger = pycompat::logging::Logger::new("pdfminer.pdftypes");

// pdfminer/pdftypes.py:221 decompress_corrupted
/// Called on some data that can't be properly decoded because of CRC
/// checksum error. Attempt to decode it skipping the CRC: a
/// `zlib.decompressobj()` is fed one byte at a time, and the output up to
/// the first `zlib.error` is returned, unless that error comes before the
/// last 4 bytes (the Adler-32 trailer), where it is raised.
pub fn decompress_corrupted(data: &[u8]) -> PyResult<Vec<u8>> {
    let _frame = recursion::py_frame()?;
    let mut d = zlib::Decompress::new(zlib::MAX_WBITS)?;
    let mut result_str = Vec::new();
    // i: the number of bytes decompressed without error
    for (i, byte) in data.chunks(1).enumerate() {
        match d.decompress(byte) {
            Ok(out) => result_str.extend_from_slice(&out),
            Err(e) if e.is_instance(ExcKind::ZlibError) => {
                // Let the error propagate if we're not yet in the CRC checksum
                if (i as i64) < data.len() as i64 - 3 {
                    return Err(e);
                }
                // logger.warning("Data-loss while decompressing corrupted data")
                LOG.warning()?;
                break;
            }
            Err(e) => return Err(e),
        }
    }
    Ok(result_str)
}

// ---------------------------------------------------------------------------
// PDFStream

/// pdfminer/pdftypes.py:243 `PDFStream`.
pub struct PDFStream {
    /// `self.attrs` (the same dict object the parser built).
    pub attrs: PDFDict,
    rawdata: RefCell<Option<PyBytes>>,
    /// `self.decipher`
    pub decipher: Option<DecipherCallable>,
    data: RefCell<Option<PyBytes>>,
    objid: RefCell<Option<ObjId>>,
    genno: RefCell<Option<PyInt>>,
}

impl PDFStream {
    // pdfminer/pdftypes.py:244 PDFStream.__init__
    pub fn new(attrs: PDFDict, rawdata: PyBytes, decipher: Option<DecipherCallable>) -> PDFStream {
        PDFStream {
            attrs,
            rawdata: RefCell::new(Some(rawdata)),
            decipher,
            data: RefCell::new(None),
            objid: RefCell::new(None),
            genno: RefCell::new(None),
        }
    }

    // pdfminer/pdftypes.py:258 PDFStream.set_objid
    /// `objid` is an int, or any id `getobj` was called with ([`ObjId`]).
    /// (Not counted: callers that run it as Python take its frame, see
    /// [`recursion::leaf`].)
    pub fn set_objid(&self, objid: impl Into<ObjId>, genno: PyInt) {
        *self.objid.borrow_mut() = Some(objid.into());
        *self.genno.borrow_mut() = Some(genno);
    }

    /// `self.objid`
    pub fn objid(&self) -> Option<ObjId> {
        self.objid.borrow().clone()
    }

    /// `self.genno`
    pub fn genno(&self) -> Option<PyInt> {
        self.genno.borrow().clone()
    }

    /// `self.data`: the decoded data, once `decode()` ran.
    pub fn data(&self) -> Option<PyBytes> {
        self.data.borrow().clone()
    }

    /// `self.rawdata`: `None` once `decode()` ran.
    pub fn rawdata(&self) -> Option<PyBytes> {
        self.rawdata.borrow().clone()
    }

    /// `str(self.objid)`
    fn objid_str(&self) -> PyResult<String> {
        match self.objid() {
            Some(id) => id.py_str(),
            None => Ok("None".to_owned()),
        }
    }

    // pdfminer/pdftypes.py:262 PDFStream.__repr__
    /// `repr(stream)` (see [`PDFObject::repr`]).
    pub fn repr(self: &Rc<Self>) -> PyResult<String> {
        Renderer::render(Item::Stream(self.clone()), Conv::Repr)
    }

    // pdfminer/pdftypes.py:272 PDFStream.__contains__
    pub fn contains(&self, name: &str) -> bool {
        self.attrs.contains_key(name)
    }

    /// The error of `stream[name]` for a missing `name` (`name` as `str()`
    /// writes it).
    pub fn missing_attribute(&self, name: &str) -> PyErr {
        let objid = match self.objid_str() {
            Ok(objid) => objid,
            Err(e) => return e,
        };
        // str() of a KeyError is the repr() of its argument.
        let msg = format!("PDF stream object {objid} does not have attribute '{name}'");
        exc::PDFKeyError(str_repr(&msg))
    }

    // pdfminer/pdftypes.py:275 PDFStream.__getitem__
    /// `stream[name]`, with its frame (a Python method called from C:
    /// `BINARY_SUBSCR` calls it directly, without a C recursion unit).
    pub fn getitem(&self, name: &str) -> PyResult<PDFObject> {
        recursion::leaf(1)?;
        self.attrs.get(name).ok_or_else(|| self.missing_attribute(name))
    }

    // pdfminer/pdftypes.py:283 PDFStream.get
    /// `stream.get(name, default)` (its frame is the caller's to count:
    /// [`PDFStream::get_counted`]).
    pub fn get(&self, name: &str, default: &PDFObject) -> PDFObject {
        self.attrs.get(name).unwrap_or_else(|| default.clone())
    }

    /// [`PDFStream::get`] with its Python frame.
    pub fn get_counted(&self, name: &str, default: &PDFObject) -> PyResult<PDFObject> {
        recursion::leaf(1)?;
        Ok(self.get(name, default))
    }

    // pdfminer/pdftypes.py:286 PDFStream.get_any
    pub fn get_any(&self, names: &[&str], default: PDFObject) -> PDFObject {
        for name in names {
            if let Some(v) = self.attrs.get(name) {
                return v;
            }
        }
        default
    }

    // pdfminer/pdftypes.py:292 PDFStream.get_filters
    /// The `(filter, params)` pairs, both resolved.
    pub fn get_filters(&self) -> PyResult<Vec<(PDFObject, PDFObject)>> {
        let _frame = recursion::py_frame()?;
        recursion::leaf(1)?; // get_any
        let filters = resolve1_none(&self.get_any(&["F", "Filter"], PDFObject::List(PDFList::default())))?;
        recursion::leaf(1)?; // get_any
        let params =
            resolve1_none(&self.get_any(&["DP", "DecodeParms", "FDecodeParms"], PDFObject::Dict(PDFDict::new())))?;
        if !filters.is_truthy() {
            return Ok(Vec::new());
        }
        let filters = match filters {
            PDFObject::List(l) => l.to_vec(),
            other => vec![other],
        };
        let params = match params {
            PDFObject::List(l) => l.to_vec(),
            // Make sure the parameters list is the same as filters.
            other => vec![other; filters.len()],
        };
        let resolved_filters = filters.iter().map(resolve1_none).collect::<PyResult<Vec<_>>>()?;
        let resolved_params = params.iter().map(resolve1_none).collect::<PyResult<Vec<_>>>()?;
        Ok(resolved_filters.into_iter().zip(resolved_params).collect())
    }

    // pdfminer/pdftypes.py:309 PDFStream.decode
    pub fn decode(&self) -> PyResult<()> {
        let _frame = recursion::py_frame()?;
        let rawdata = self.rawdata.borrow().clone();
        let Some(rawdata) = rawdata.filter(|_| self.data.borrow().is_none()) else {
            let show = |b: &Option<PyBytes>| b.as_ref().map_or_else(|| "None".to_owned(), |b| bytes_repr(b));
            return Err(exc::AssertionError(format!(
                "({}, {})",
                show(&self.data.borrow()),
                show(&self.rawdata.borrow())
            )));
        };
        let mut data: PyBytes = rawdata;
        if let Some(decipher) = &self.decipher {
            // Handle encryption
            let (Some(objid), Some(genno)) = (self.objid(), self.genno()) else {
                return Err(exc::AssertionError(""));
            };
            data = decipher(&objid, &genno, &data, Some(&self.attrs))?.into();
        }
        let filters = self.get_filters()?;
        if filters.is_empty() {
            *self.data.borrow_mut() = Some(data);
            *self.rawdata.borrow_mut() = None;
            return Ok(());
        }
        for (f, params) in &filters {
            if in_literals(f, &*LITERALS_FLATE_DECODE) {
                // will get errors if the document is encrypted.
                data = match zlib::decompress(&data) {
                    Ok(d) => d.into(),
                    // (with settings.STRICT, a PDFException)
                    Err(e) if e.is_instance(ExcKind::ZlibError) => match decompress_corrupted(&data) {
                        Ok(d) => d.into(),
                        Err(e) if e.is_instance(ExcKind::ZlibError) => Rc::from(&[][..]),
                        Err(e) => return Err(e),
                    },
                    Err(e) => return Err(e),
                };
            } else if in_literals(f, &*LITERALS_LZW_DECODE) {
                data = lzwdecode(&data)?.into();
            } else if in_literals(f, &*LITERALS_ASCII85_DECODE) {
                data = ascii85decode(&data)?.into();
            } else if in_literals(f, &*LITERALS_ASCIIHEX_DECODE) {
                data = asciihexdecode(&data)?.into();
            } else if in_literals(f, &*LITERALS_RUNLENGTH_DECODE) {
                data = rldecode(&data)?.into();
            } else if in_literals(f, &*LITERALS_CCITTFAX_DECODE) {
                data = ccittfaxdecode_obj(&data, params)?.into();
            } else if in_literals(f, &*LITERALS_DCT_DECODE) {
                // This is probably a JPG stream
                // it does not need to be decoded twice.
                // Just return the stream to the user.
            } else if in_literals(f, &*LITERALS_JBIG2_DECODE) || in_literals(f, &*LITERALS_JPX_DECODE) {
            } else if in_literals(f, &[*LITERAL_CRYPT]) {
                // not yet..
                return Err(exc::PDFNotImplementedError("/Crypt filter is unsupported"));
            } else {
                return Err(exc::PDFNotImplementedError(format!("Unsupported filter: {}", f.repr()?)));
            }
            // apply predictors
            if params.is_truthy() && {
                if let PDFObject::Stream(_) = params {
                    // PDFStream.__contains__, called from C
                    recursion::c_called_leaf()?;
                }
                params.py_contains_str("Predictor")?
            } {
                // params.get(name, default): a dict, or a stream
                let get = |name: &str, default: i64| -> PyResult<PDFObject> {
                    let default = PDFObject::Int(PyInt::Small(default));
                    Ok(match params {
                        PDFObject::Dict(d) => d.get(name).unwrap_or(default),
                        PDFObject::Stream(s) => s.get_counted(name, &default)?,
                        _ => unreachable!("only a dict or a stream contains a str key"),
                    })
                };
                let predictor = match params {
                    PDFObject::Dict(d) => d.get("Predictor").expect("checked by `in`"),
                    PDFObject::Stream(s) => s.getitem("Predictor")?,
                    _ => unreachable!("only a dict or a stream contains a str key"),
                };
                let pred_obj = int_value_obj(&predictor)?;
                let pred = int_of(&pred_obj);
                if pred == PyInt::Small(1) {
                    // no predictor
                } else if pred == PyInt::Small(2) {
                    // TIFF predictor 2
                    let colors = int_value(&get("Colors", 1)?)?;
                    let columns = int_value(&get("Columns", 1)?)?;
                    let raw_bits_per_component = get("BitsPerComponent", 8)?;
                    let bitspercomponent = int_value_obj(&raw_bits_per_component)?;
                    data = apply_tiff_predictor(&colors, &columns, &bitspercomponent, &data)?.into();
                } else if pred >= PyInt::Small(10) {
                    // PNG predictor
                    let colors = int_value(&get("Colors", 1)?)?;
                    let columns = int_value(&get("Columns", 1)?)?;
                    let raw_bits_per_component = get("BitsPerComponent", 8)?;
                    let bitspercomponent = int_value_obj(&raw_bits_per_component)?;
                    data = apply_png_predictor(&pred, &colors, &columns, &bitspercomponent, &data)?.into();
                } else {
                    let error_msg = format!("Unsupported predictor: {}", pred_obj.repr()?);
                    return Err(exc::PDFNotImplementedError(error_msg));
                }
            }
        }
        *self.data.borrow_mut() = Some(data);
        *self.rawdata.borrow_mut() = None;
        Ok(())
    }

    // pdfminer/pdftypes.py:399 PDFStream.get_data
    pub fn get_data(&self) -> PyResult<PyBytes> {
        let _frame = recursion::py_frame()?;
        if self.data.borrow().is_none() {
            self.decode()?;
        }
        Ok(self.data.borrow().clone().expect("decode() sets data"))
    }

    // pdfminer/pdftypes.py:405 PDFStream.get_rawdata
    pub fn get_rawdata(&self) -> Option<PyBytes> {
        self.rawdata()
    }
}
