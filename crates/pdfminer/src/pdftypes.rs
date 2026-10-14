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
//! `repr()`/`str()` of an object is [`PDFObject::repr`] (both are the same
//! for these types).
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
use pycompat::repr::{bytes_repr, float_repr, str_repr};
use pycompat::{exc, ExcKind, Num, PyErr, PyInt, PyResult};
use rustc_hash::FxBuildHasher;

use crate::psparser::{PSKeyword, PSLiteral, LIT};

/// Python `bytes`: immutable and shared.
pub type PyBytes = Rc<[u8]>;

// pdfminer/pdftypes.py:28-39
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
            PDFObject::Literal(_) | PDFObject::Keyword(_) | PDFObject::Stream(_) | PDFObject::Ref(_) => true,
        }
    }

    /// `repr(x)` (also `str(x)` for every PDF object type).
    pub fn repr(&self) -> String {
        let mut out = String::new();
        self.write_repr(&mut out);
        out
    }

    /// `repr(list(objs))`
    pub fn list_repr(objs: &[PDFObject]) -> String {
        let mut out = String::new();
        write_list_repr(&mut out, objs);
        out
    }

    fn write_repr(&self, out: &mut String) {
        match self {
            PDFObject::Null => out.push_str("None"),
            PDFObject::Bool(b) => out.push_str(if *b { "True" } else { "False" }),
            PDFObject::Int(i) => out.push_str(&i.to_string()),
            PDFObject::Float(f) => out.push_str(&float_repr(*f)),
            PDFObject::Bytes(b) => out.push_str(&bytes_repr(b)),
            PDFObject::Literal(lit) => out.push_str(&lit.repr()),
            PDFObject::Keyword(kwd) => out.push_str(&kwd.repr()),
            PDFObject::List(l) => write_list_repr(out, &l.borrow()),
            PDFObject::Dict(d) => write_dict_repr(out, d),
            PDFObject::Stream(s) => out.push_str(&s.repr()),
            PDFObject::Ref(r) => out.push_str(&r.repr()),
        }
    }

    /// `"Predictor" in x` for a `str` key: `TypeError` where Python's `in`
    /// raises one.
    pub fn py_contains_str(&self, key: &str) -> PyResult<bool> {
        match self {
            PDFObject::Dict(d) => Ok(d.contains_key(key)),
            PDFObject::Stream(s) => Ok(s.contains(key)),
            // no element of a list of PDF objects equals a str
            PDFObject::List(_) => Ok(false),
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
            // PDFStream.__getitem__ looks the key up in attrs
            PDFObject::Stream(s) => Err(s.missing_attribute("0")),
            other => Err(exc::TypeError(format!("'{}' object is not subscriptable", other.type_name()))),
        }
    }
}

fn write_list_repr(out: &mut String, objs: &[PDFObject]) {
    out.push('[');
    for (i, x) in objs.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        x.write_repr(out);
    }
    out.push(']');
}

fn write_dict_repr(out: &mut String, d: &PDFDict) {
    out.push('{');
    for (i, (k, v)) in d.borrow().iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&str_repr(k));
        out.push_str(": ");
        v.write_repr(out);
    }
    out.push('}');
}

impl fmt::Debug for PDFObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.repr())
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

/// `PyObject_RichCompareBool(a, b, Py_EQ)`: identity, then `==`.
fn eq_or_same(a: &PDFObject, b: &PDFObject) -> bool {
    same_object(a, b) || a == b
}

impl PartialEq for PDFObject {
    /// Python's `==` (see the module documentation).
    fn eq(&self, other: &PDFObject) -> bool {
        if let (Some(a), Some(b)) = (self.as_num(), other.as_num()) {
            return a.py_eq(&b);
        }
        match (self, other) {
            (PDFObject::Bytes(a), PDFObject::Bytes(b)) => a == b,
            (PDFObject::List(a), PDFObject::List(b)) => {
                if a.ptr_eq(b) {
                    return true;
                }
                let (a, b) = (a.borrow(), b.borrow());
                a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| eq_or_same(x, y))
            }
            (PDFObject::Dict(a), PDFObject::Dict(b)) => {
                if a.ptr_eq(b) {
                    return true;
                }
                let (a, b) = (a.borrow(), b.borrow());
                a.len() == b.len() && a.iter().all(|(k, x)| b.get(k).is_some_and(|y| eq_or_same(x, y)))
            }
            _ => same_object(self, other),
        }
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

    // pdfminer/pdftypes.py:96 PDFObjRef.__repr__
    pub fn repr(&self) -> String {
        format!("<PDFObjRef:{}>", self.0.objid)
    }

    // pdfminer/pdftypes.py:99 PDFObjRef.resolve
    pub fn resolve(&self, default: &PDFObject) -> PyResult<PDFObject> {
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

// pdfminer/pdftypes.py:107 resolve1
/// Resolves an object. If this is an array or dictionary, it may still
/// contain some indirect objects inside.
pub fn resolve1(x: &PDFObject, default: &PDFObject) -> PyResult<PDFObject> {
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

/// Python's recursion limit (`sys.getrecursionlimit()`), used where the
/// port recurses like upstream does. Python counts every frame on the stack,
/// so the depth at which it raises `RecursionError` is approximated here by
/// the depth of the recursive function alone.
pub const RECURSION_LIMIT: usize = 1000;

fn recursion_error() -> PyErr {
    exc::RecursionError("maximum recursion depth exceeded")
}

// pdfminer/pdftypes.py:118 resolve_all
/// Recursively resolves the given object and all the internals: lists are
/// rebuilt, dicts are updated in place.
pub fn resolve_all(x: &PDFObject, default: &PDFObject) -> PyResult<PDFObject> {
    resolve_all_depth(x, default, 0)
}

fn resolve_all_depth(x: &PDFObject, default: &PDFObject, depth: usize) -> PyResult<PDFObject> {
    if depth >= RECURSION_LIMIT {
        return Err(recursion_error());
    }
    let mut x = x.clone();
    while let PDFObject::Ref(r) = &x {
        x = r.resolve(default)?;
    }
    match &x {
        PDFObject::List(l) => {
            let items =
                l.to_vec().iter().map(|v| resolve_all_depth(v, default, depth + 1)).collect::<PyResult<Vec<_>>>()?;
            Ok(PDFObject::List(PDFList::new(items)))
        }
        PDFObject::Dict(d) => {
            for (k, v) in d.items() {
                let v = resolve_all_depth(&v, default, depth + 1)?;
                d.insert(k, v);
            }
            Ok(x)
        }
        _ => Ok(x),
    }
}

/// `decipher(objid, genno, data, attrs)`: the document's decryption
/// callback (`DecipherCallable`).
pub type DecipherCallable = Rc<dyn Fn(&PyInt, &PyInt, &[u8], Option<&PDFDict>) -> PyResult<Vec<u8>>>;

// pdfminer/pdftypes.py:134 decipher_all
/// Recursively deciphers the given object: lists are rebuilt, dicts are
/// updated in place.
pub fn decipher_all(decipher: &DecipherCallable, objid: &PyInt, genno: &PyInt, x: &PDFObject) -> PyResult<PDFObject> {
    decipher_all_depth(decipher, objid, genno, x, 0)
}

fn decipher_all_depth(
    decipher: &DecipherCallable,
    objid: &PyInt,
    genno: &PyInt,
    x: &PDFObject,
    depth: usize,
) -> PyResult<PDFObject> {
    if depth >= RECURSION_LIMIT {
        return Err(recursion_error());
    }
    match x {
        PDFObject::Bytes(b) => {
            if b.is_empty() {
                return Ok(x.clone());
            }
            Ok(PDFObject::Bytes(decipher(objid, genno, b, None)?.into()))
        }
        PDFObject::List(l) => {
            let items = l
                .to_vec()
                .iter()
                .map(|v| decipher_all_depth(decipher, objid, genno, v, depth + 1))
                .collect::<PyResult<Vec<_>>>()?;
            Ok(PDFObject::List(PDFList::new(items)))
        }
        PDFObject::Dict(d) => {
            for (k, v) in d.items() {
                let v = decipher_all_depth(decipher, objid, genno, &v, depth + 1)?;
                d.insert(k, v);
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
    Ok(match int_value_obj(x)? {
        PDFObject::Int(i) => i,
        PDFObject::Bool(b) => PyInt::Small(i64::from(b)),
        _ => unreachable!("int_value_obj returns an int or a bool"),
    })
}

// pdfminer/pdftypes.py:157 float_value
/// `float_value(x)` with `STRICT = False`: the float, or 0.0 (an int is not
/// a float here).
pub fn float_value(x: &PDFObject) -> PyResult<f64> {
    Ok(match resolve1_none(x)? {
        PDFObject::Float(f) => f,
        _ => 0.0,
    })
}

// pdfminer/pdftypes.py:166 num_value
/// `num_value(x)` with `STRICT = False`: the int or float, or the int 0. A
/// `bool` is returned as 0/1 (see [`int_value`]).
pub fn num_value(x: &PDFObject) -> PyResult<Num> {
    Ok(resolve1_none(x)?.as_num().unwrap_or(Num::Int(PyInt::ZERO)))
}

// pdfminer/pdftypes.py:175 uint_value
/// Resolves a number and interprets it as a two's-complement unsigned
/// number.
pub fn uint_value(x: &PDFObject, n_bits: u32) -> PyResult<PyInt> {
    let xi = int_value(x)?;
    if xi > PyInt::ZERO {
        Ok(xi)
    } else {
        let two_pow = PyInt::Small(2).pow(&PyInt::from(n_bits)).expect("non-negative exponent");
        Ok(xi.add(&two_pow))
    }
}

// pdfminer/pdftypes.py:184 str_value
/// `str_value(x)` with `STRICT = False`: the bytes, or `b""`.
pub fn str_value(x: &PDFObject) -> PyResult<PyBytes> {
    Ok(match resolve1_none(x)? {
        PDFObject::Bytes(b) => b,
        _ => Rc::from(&[][..]),
    })
}

// pdfminer/pdftypes.py:193 list_value
/// `list_value(x)` with `STRICT = False`: the list, or a new empty list.
pub fn list_value(x: &PDFObject) -> PyResult<PDFList> {
    Ok(match resolve1_none(x)? {
        PDFObject::List(l) => l,
        _ => PDFList::default(),
    })
}

// pdfminer/pdftypes.py:202 dict_value
/// `dict_value(x)` with `STRICT = False`: the dict, or a new empty dict.
pub fn dict_value(x: &PDFObject) -> PyResult<PDFDict> {
    Ok(match resolve1_none(x)? {
        PDFObject::Dict(d) => d,
        _ => PDFDict::new(),
    })
}

// pdfminer/pdftypes.py:212 stream_value
/// `stream_value(x)` with `STRICT = False`: the stream, or a new empty
/// stream `PDFStream({}, b"")`.
pub fn stream_value(x: &PDFObject) -> PyResult<Rc<PDFStream>> {
    Ok(match resolve1_none(x)? {
        PDFObject::Stream(s) => s,
        _ => Rc::new(PDFStream::new(PDFDict::new(), Rc::from(&[][..]), None)),
    })
}

// ---------------------------------------------------------------------------
// Filters whose modules are not ported yet. Each is replaced by a call to
// its port (`crate::lzw::lzwdecode`, ...) when that module lands.

fn zlib_decompress(_data: &[u8]) -> PyResult<Vec<u8>> {
    Err(PyErr::not_ported("zlib.decompress"))
}

// pdfminer/pdftypes.py:221 decompress_corrupted (needs zlib.decompressobj)
fn decompress_corrupted(_data: &[u8]) -> PyResult<Vec<u8>> {
    Err(PyErr::not_ported("pdfminer.pdftypes.decompress_corrupted"))
}

fn lzwdecode(_data: &[u8]) -> PyResult<Vec<u8>> {
    Err(PyErr::not_ported("pdfminer.lzw.lzwdecode"))
}

fn ascii85decode(_data: &[u8]) -> PyResult<Vec<u8>> {
    Err(PyErr::not_ported("pdfminer.ascii85.ascii85decode"))
}

fn asciihexdecode(_data: &[u8]) -> PyResult<Vec<u8>> {
    Err(PyErr::not_ported("pdfminer.ascii85.asciihexdecode"))
}

fn rldecode(_data: &[u8]) -> PyResult<Vec<u8>> {
    Err(PyErr::not_ported("pdfminer.runlength.rldecode"))
}

fn ccittfaxdecode(_data: &[u8], _params: &PDFObject) -> PyResult<Vec<u8>> {
    Err(PyErr::not_ported("pdfminer.ccitt.ccittfaxdecode"))
}

fn apply_tiff_predictor(
    _colors: &PyInt,
    _columns: &PyInt,
    _bitspercomponent: &PyInt,
    _data: &[u8],
) -> PyResult<Vec<u8>> {
    Err(PyErr::not_ported("pdfminer.utils.apply_tiff_predictor"))
}

fn apply_png_predictor(
    _pred: &PyInt,
    _colors: &PyInt,
    _columns: &PyInt,
    _bitspercomponent: &PyInt,
    _data: &[u8],
) -> PyResult<Vec<u8>> {
    Err(PyErr::not_ported("pdfminer.utils.apply_png_predictor"))
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
    objid: RefCell<Option<PyInt>>,
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
    pub fn set_objid(&self, objid: PyInt, genno: PyInt) {
        *self.objid.borrow_mut() = Some(objid);
        *self.genno.borrow_mut() = Some(genno);
    }

    /// `self.objid`
    pub fn objid(&self) -> Option<PyInt> {
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

    fn objid_repr(&self) -> String {
        self.objid.borrow().as_ref().map_or_else(|| "None".to_owned(), |i| i.to_string())
    }

    // pdfminer/pdftypes.py:262 PDFStream.__repr__
    pub fn repr(&self) -> String {
        let attrs = PDFObject::Dict(self.attrs.clone()).repr();
        match &*self.data.borrow() {
            None => {
                let raw = self.rawdata.borrow().as_ref().map_or(0, |r| r.len());
                format!("<PDFStream({}): raw={raw}, {attrs}>", self.objid_repr())
            }
            Some(data) => format!("<PDFStream({}): len={}, {attrs}>", self.objid_repr(), data.len()),
        }
    }

    // pdfminer/pdftypes.py:272 PDFStream.__contains__
    pub fn contains(&self, name: &str) -> bool {
        self.attrs.contains_key(name)
    }

    fn missing_attribute(&self, name: &str) -> PyErr {
        // str() of a KeyError is the repr() of its argument.
        let msg = format!("PDF stream object {} does not have attribute '{name}'", self.objid_repr());
        exc::PDFKeyError(str_repr(&msg))
    }

    // pdfminer/pdftypes.py:275 PDFStream.__getitem__
    /// `stream[name]`
    pub fn getitem(&self, name: &str) -> PyResult<PDFObject> {
        self.attrs.get(name).ok_or_else(|| self.missing_attribute(name))
    }

    // pdfminer/pdftypes.py:283 PDFStream.get
    pub fn get(&self, name: &str, default: &PDFObject) -> PDFObject {
        self.attrs.get(name).unwrap_or_else(|| default.clone())
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
        let filters = resolve1_none(&self.get_any(&["F", "Filter"], PDFObject::List(PDFList::default())))?;
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
                data = match zlib_decompress(&data) {
                    Ok(d) => d.into(),
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
                data = ccittfaxdecode(&data, params)?.into();
            } else if in_literals(f, &*LITERALS_DCT_DECODE) {
                // This is probably a JPG stream
                // it does not need to be decoded twice.
                // Just return the stream to the user.
            } else if in_literals(f, &*LITERALS_JBIG2_DECODE) || in_literals(f, &*LITERALS_JPX_DECODE) {
            } else if in_literals(f, &[*LITERAL_CRYPT]) {
                // not yet..
                return Err(exc::PDFNotImplementedError("/Crypt filter is unsupported"));
            } else {
                return Err(exc::PDFNotImplementedError(format!("Unsupported filter: {}", f.repr())));
            }
            // apply predictors
            if params.is_truthy() && params.py_contains_str("Predictor")? {
                let get = |name: &str, default: i64| -> PDFObject {
                    let default = PDFObject::Int(PyInt::Small(default));
                    match params {
                        PDFObject::Dict(d) => d.get(name).unwrap_or(default),
                        PDFObject::Stream(s) => s.get(name, &default),
                        _ => unreachable!("only a dict or a stream contains a str key"),
                    }
                };
                let predictor = match params {
                    PDFObject::Dict(d) => d.get("Predictor").expect("checked by `in`"),
                    PDFObject::Stream(s) => s.getitem("Predictor")?,
                    _ => unreachable!("only a dict or a stream contains a str key"),
                };
                let pred_obj = int_value_obj(&predictor)?;
                let pred = int_value(&pred_obj)?;
                if pred == PyInt::Small(1) {
                    // no predictor
                } else if pred == PyInt::Small(2) {
                    // TIFF predictor 2
                    let colors = int_value(&get("Colors", 1))?;
                    let columns = int_value(&get("Columns", 1))?;
                    let raw_bits_per_component = get("BitsPerComponent", 8);
                    let bitspercomponent = int_value(&raw_bits_per_component)?;
                    data = apply_tiff_predictor(&colors, &columns, &bitspercomponent, &data)?.into();
                } else if pred >= PyInt::Small(10) {
                    // PNG predictor
                    let colors = int_value(&get("Colors", 1))?;
                    let columns = int_value(&get("Columns", 1))?;
                    let raw_bits_per_component = get("BitsPerComponent", 8);
                    let bitspercomponent = int_value(&raw_bits_per_component)?;
                    data = apply_png_predictor(&pred, &colors, &columns, &bitspercomponent, &data)?.into();
                } else {
                    let error_msg = format!("Unsupported predictor: {}", pred_obj.repr());
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
