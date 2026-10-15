//! The pdfplumber object model: `T_obj` (pdfplumber/_typing.py:7 T_obj), a
//! `Dict[str, Any]`, and the dynamic values it holds.
//!
//! pdfplumber represents every page object (`char`, `rect`, `line`,
//! `curve`, `image`, edges, words, annotations...) as a plain `dict` with
//! `str` keys, and its algorithms read them with `itemgetter`, compare and
//! add their values with Python's operators, copy them with `dict(obj)` or
//! `{**obj, ...}`, and compare whole dicts with `==` (`list.index`). This
//! module reproduces those semantics:
//!
//! - [`PyValue`]: a Python value as found in such dicts: `None`, `bool`,
//!   `int` (any size) and `float` kept apart, `str`, `bytes`, `tuple` and
//!   `list` kept apart, nested `dict`s ([`Obj`]), and pdfminer objects
//!   (`PSLiteral`, `PDFStream`, `PDFObjRef`, a resolved PDF `dict`...) kept
//!   as [`PDFObject`]. Python's operators on them are the methods
//!   [`PyValue::add`], [`PyValue::lt`], [`PyValue::py_eq`]..., with
//!   CPython's results, result types and `TypeError` messages.
//! - [`Obj`]: an insertion-ordered `dict` with `str` keys. Cloning shares
//!   it (`Rc`); [`Obj::set`] and [`Obj::remove`] copy it first when it is
//!   shared, so an `Obj` behaves as a value: every upstream function in
//!   this crate's scope that "modifies" an object works on a fresh dict
//!   (`dict(obj)`, `{**obj}`, `obj.__class__(items)`), which is what
//!   copy-on-write gives. Assigning an existing key keeps its position, as
//!   in Python. The keys read in hot loops (`x0`, `top`, `x1`, `bottom`,
//!   `doctop`, `width`, `height`, `text`, `upright`, `size`, `orientation`,
//!   `object_type`, `y0`, `y1`) are found through a small index instead of
//!   a scan.
//!
//! # NaN identity
//!
//! CPython compares container items, looks up set and dict keys and tests
//! membership with an identity shortcut (`PyObject_RichCompareBool`: `x is
//! y or x == y`), so a NaN is equal to itself *as the same object*, and
//! `hash(nan)` is its object's address. The identity of a float matters
//! only for a NaN (any other float equals itself), and a [`PyValue`]
//! carries it in the NaN's payload: [`PyValue::float`] gives every NaN it
//! makes a payload of its own (a new object, as each computation makes in
//! Python), and a clone keeps it (the same object: `min()` returning one of
//! its arguments, `itemgetter`, `dict(obj)` sharing the values). Then:
//!
//! - `a == b` ([`PyValue::py_eq`]) is the operator: a NaN never equals
//!   anything;
//! - [`PyValue::eq_bool`] is `PyObject_RichCompareBool`: a NaN equals the
//!   same object. The items of a `tuple` or `list` and the values of a
//!   `dict` compare with it (also in `<`, which looks for the first items
//!   that differ), as do [`HashKey`]s (sets and dicts) and `in`;
//! - the order in which a `set` lists NaNs, which follows their addresses
//!   in CPython, is fixed by the port where it matters
//!   (`utils/clustering.rs`).
//!
//! Values made from outside (a JavaScript number, a value of a test case)
//! are new objects, except that a NaN whose payload already carries an
//! identity keeps it ([`PyValue::float_with_identity`]): the generators of
//! the test cases write each distinct NaN object of a case with a payload
//! of its own, so that sharing (`rect_to_edges` gives the edges of a rect
//! its very coordinate objects) survives the serialization. Code that
//! builds objects (the port of `page.py`) gives a NaN one identity per
//! Python object and clones the value wherever Python shares the object
//! (`LTChar.size` is its `height` or `width`, `bbox` holds `x0`...).
//!
//! Text is Python's `str` ([`PyStr`]): values and keys may hold lone
//! surrogates (a char's text from `chr(cid)`, a key or value a user gives).

use rustc_hash::FxHashMap as HashMap;
use std::cell::RefCell;
use std::cmp::Ordering;
use std::fmt::Write as _;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use pdfminer::pdftypes::PDFObject;
use pycompat::float::{float_div, float_from_bytes, float_from_str};
use pycompat::repr::{bytes_repr, float_repr, str_repr};
use pycompat::{exc, Num, PyErr, PyInt, PyResult, PyStr};

// -- keys ---------------------------------------------------------------------

thread_local! {
    static INTERNED: RefCell<HashMap<Box<[u8]>, PyStr>> = RefCell::new(HashMap::default());
    static HOT_KEYS: [PyStr; HOT_N] = std::array::from_fn(|i| PyStr::new(HOT[i].name()));
}

/// A dict key: a `str`, given as Rust text or as a [`PyStr`].
pub trait Key {
    /// The key's bytes (`surrogatepass` UTF-8, as [`PyStr::as_bytes`]).
    fn key_bytes(&self) -> &[u8];
    /// The key as a stored `str` (interned when it is Rust text).
    fn to_key(&self) -> PyStr;
}

impl Key for str {
    fn key_bytes(&self) -> &[u8] {
        self.as_bytes()
    }
    fn to_key(&self) -> PyStr {
        intern(self)
    }
}

impl Key for String {
    fn key_bytes(&self) -> &[u8] {
        self.as_bytes()
    }
    fn to_key(&self) -> PyStr {
        intern(self)
    }
}

impl Key for PyStr {
    fn key_bytes(&self) -> &[u8] {
        self.as_bytes()
    }
    fn to_key(&self) -> PyStr {
        match self.as_str() {
            Some(s) => intern(s),
            None => self.clone(),
        }
    }
}

impl Key for Rc<str> {
    fn key_bytes(&self) -> &[u8] {
        self.as_bytes()
    }
    fn to_key(&self) -> PyStr {
        intern(self)
    }
}

impl<K: Key + ?Sized> Key for &K {
    fn key_bytes(&self) -> &[u8] {
        (**self).key_bytes()
    }
    fn to_key(&self) -> PyStr {
        (**self).to_key()
    }
}

/// A shared copy of a dict key (keys repeat across thousands of objects).
pub fn intern(key: &str) -> PyStr {
    if let Some(h) = Hot::of(key) {
        return hot_key(h);
    }
    INTERNED.with(|m| {
        let mut m = m.borrow_mut();
        if let Some(k) = m.get(key.as_bytes()) {
            return k.clone();
        }
        let k = PyStr::new(key);
        if m.len() < 4096 {
            m.insert(Box::from(key.as_bytes()), k.clone());
        }
        k
    })
}

/// The shared copy of a hot key.
fn hot_key(h: Hot) -> PyStr {
    HOT_KEYS.with(|keys| keys[h as usize].clone())
}

/// The keys with a fast index in [`Obj`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Hot {
    X0,
    Top,
    X1,
    Bottom,
    Doctop,
    Width,
    Height,
    Text,
    Upright,
    Size,
    Orientation,
    ObjectType,
    Y0,
    Y1,
}

const HOT_N: usize = 14;

/// Every [`Hot`] key, by index.
const HOT: [Hot; HOT_N] = [
    Hot::X0,
    Hot::Top,
    Hot::X1,
    Hot::Bottom,
    Hot::Doctop,
    Hot::Width,
    Hot::Height,
    Hot::Text,
    Hot::Upright,
    Hot::Size,
    Hot::Orientation,
    Hot::ObjectType,
    Hot::Y0,
    Hot::Y1,
];

impl Hot {
    pub fn name(self) -> &'static str {
        match self {
            Hot::X0 => "x0",
            Hot::Top => "top",
            Hot::X1 => "x1",
            Hot::Bottom => "bottom",
            Hot::Doctop => "doctop",
            Hot::Width => "width",
            Hot::Height => "height",
            Hot::Text => "text",
            Hot::Upright => "upright",
            Hot::Size => "size",
            Hot::Orientation => "orientation",
            Hot::ObjectType => "object_type",
            Hot::Y0 => "y0",
            Hot::Y1 => "y1",
        }
    }

    pub fn of(key: &str) -> Option<Hot> {
        Hot::of_bytes(key.as_bytes())
    }

    /// The hot key whose bytes are `key`.
    pub fn of_bytes(key: &[u8]) -> Option<Hot> {
        Some(match key {
            b"x0" => Hot::X0,
            b"top" => Hot::Top,
            b"x1" => Hot::X1,
            b"bottom" => Hot::Bottom,
            b"doctop" => Hot::Doctop,
            b"width" => Hot::Width,
            b"height" => Hot::Height,
            b"text" => Hot::Text,
            b"upright" => Hot::Upright,
            b"size" => Hot::Size,
            b"orientation" => Hot::Orientation,
            b"object_type" => Hot::ObjectType,
            b"y0" => Hot::Y0,
            b"y1" => Hot::Y1,
            _ => return None,
        })
    }
}

/// `KeyError(key)` for a `str` key: `str(e)` is the key's `repr`.
pub fn key_error(key: &(impl Key + ?Sized)) -> PyErr {
    match std::str::from_utf8(key.key_bytes()) {
        Ok(s) => exc::KeyError(str_repr(s)),
        Err(_) => exc::KeyError(key.to_key().repr()),
    }
}

// -- values -------------------------------------------------------------------

/// A Python value held by a pdfplumber object (see the module documentation).
#[derive(Clone)]
pub enum PyValue {
    None,
    Bool(bool),
    Int(PyInt),
    Float(f64),
    Str(PyStr),
    Bytes(Rc<[u8]>),
    Tuple(Rc<[PyValue]>),
    List(Rc<[PyValue]>),
    Dict(Obj),
    /// A pdfminer object without a closer Python equivalent here
    /// (`PSLiteral`, `PSKeyword`, `PDFStream`, `PDFObjRef`, and the `list`
    /// and `dict` objects of a PDF structure, which keep their identity).
    Pdf(PDFObject),
}

/// `None`. (A default lets cargo mutants replace a function returning a
/// value by a constant one.)
impl Default for PyValue {
    fn default() -> PyValue {
        PyValue::None
    }
}

impl std::fmt::Debug for PyValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.repr())
    }
}

impl From<Num> for PyValue {
    fn from(n: Num) -> PyValue {
        match n {
            Num::Int(i) => PyValue::Int(i),
            Num::Float(x) => PyValue::float(x),
        }
    }
}

/// A new `float` object ([`PyValue::float`]).
impl From<f64> for PyValue {
    fn from(x: f64) -> PyValue {
        PyValue::float(x)
    }
}

/// The payload bit of the NaN identities the port makes; the generators of
/// test cases use payloads without it.
const FRESH_NAN: u64 = 1 << 50;

thread_local! {
    static NAN_COUNTER: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// A quiet NaN with a payload no other NaN of this thread has.
fn fresh_nan() -> f64 {
    let k = NAN_COUNTER.with(|c| {
        let k = c.get().wrapping_add(1) & (FRESH_NAN - 1);
        c.set(k);
        k
    });
    f64::from_bits(0x7ff8_0000_0000_0000 | FRESH_NAN | k)
}

impl From<i64> for PyValue {
    fn from(i: i64) -> PyValue {
        PyValue::Int(PyInt::Small(i))
    }
}

impl From<bool> for PyValue {
    fn from(b: bool) -> PyValue {
        PyValue::Bool(b)
    }
}

impl From<&str> for PyValue {
    fn from(s: &str) -> PyValue {
        PyValue::Str(PyStr::new(s))
    }
}

impl From<String> for PyValue {
    fn from(s: String) -> PyValue {
        PyValue::Str(PyStr::new(&s))
    }
}

impl From<PyStr> for PyValue {
    fn from(s: PyStr) -> PyValue {
        PyValue::Str(s)
    }
}

impl From<Obj> for PyValue {
    fn from(o: Obj) -> PyValue {
        PyValue::Dict(o)
    }
}

/// A comparison operator (`Py_LT`...), for messages and dispatch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CmpOp {
    Lt,
    Le,
    Gt,
    Ge,
}

impl CmpOp {
    fn symbol(self) -> &'static str {
        match self {
            CmpOp::Lt => "<",
            CmpOp::Le => "<=",
            CmpOp::Gt => ">",
            CmpOp::Ge => ">=",
        }
    }

    fn holds(self, o: Ordering) -> bool {
        match self {
            CmpOp::Lt => o == Ordering::Less,
            CmpOp::Le => o != Ordering::Greater,
            CmpOp::Gt => o == Ordering::Greater,
            CmpOp::Ge => o != Ordering::Less,
        }
    }
}

impl PyValue {
    /// A `str` value.
    pub fn str(s: &str) -> PyValue {
        PyValue::Str(PyStr::new(s))
    }

    /// A new `float` object: a NaN gets an identity of its own (see the
    /// module documentation).
    #[inline]
    pub fn float(x: f64) -> PyValue {
        PyValue::Float(if x.is_nan() { fresh_nan() } else { x })
    }

    /// A `float` from outside the port: a NaN whose payload holds an
    /// identity (non-zero, without the port's own bit) keeps it, any other
    /// NaN is a new object.
    pub fn float_with_identity(x: f64) -> PyValue {
        let payload = x.to_bits() & ((1 << 51) - 1);
        if x.is_nan() && (payload == 0 || payload & FRESH_NAN != 0) {
            return PyValue::Float(fresh_nan());
        }
        PyValue::Float(x)
    }

    /// A `tuple` value.
    pub fn tuple(items: impl IntoIterator<Item = PyValue>) -> PyValue {
        PyValue::Tuple(items.into_iter().collect())
    }

    /// A `list` value.
    pub fn list(items: impl IntoIterator<Item = PyValue>) -> PyValue {
        PyValue::List(items.into_iter().collect())
    }

    /// A value from a PDF object: scalars become Python scalars, the others
    /// stay PDF objects.
    pub fn from_pdf(obj: PDFObject) -> PyValue {
        match obj {
            PDFObject::Null => PyValue::None,
            PDFObject::Bool(b) => PyValue::Bool(b),
            PDFObject::Int(i) => PyValue::Int(i),
            PDFObject::Float(x) => PyValue::float(x),
            PDFObject::Bytes(b) => PyValue::Bytes(b),
            other => PyValue::Pdf(other),
        }
    }

    /// A PDF object that is a Python scalar, `list` or `dict` (pdfminer's
    /// objects are Python values), as that Python value; `None` for every
    /// other value. Operators see a PDF object through it.
    fn as_python(&self) -> Option<PyValue> {
        let PyValue::Pdf(o) = self else { return None };
        Some(match o {
            PDFObject::List(l) => PyValue::List(l.to_vec().into_iter().map(PyValue::from_pdf).collect()),
            PDFObject::Dict(d) => {
                PyValue::Dict(Obj::from_pairs(d.items().into_iter().map(|(k, v)| (k, PyValue::from_pdf(v)))))
            }
            PDFObject::Literal(_) | PDFObject::Keyword(_) | PDFObject::Stream(_) | PDFObject::Ref(_) => return None,
            scalar => PyValue::from_pdf(scalar.clone()),
        })
    }

    /// The two operands of an operator, PDF objects seen as Python values
    /// (`None` when neither changes).
    fn as_python2(&self, other: &PyValue) -> Option<(PyValue, PyValue)> {
        match (self.as_python(), other.as_python()) {
            (None, None) => None,
            (a, b) => Some((a.unwrap_or_else(|| self.clone()), b.unwrap_or_else(|| other.clone()))),
        }
    }

    /// `type(v).__name__`
    pub fn type_name(&self) -> &'static str {
        match self {
            PyValue::None => "NoneType",
            PyValue::Bool(_) => "bool",
            PyValue::Int(_) => "int",
            PyValue::Float(_) => "float",
            PyValue::Str(_) => "str",
            PyValue::Bytes(_) => "bytes",
            PyValue::Tuple(_) => "tuple",
            PyValue::List(_) => "list",
            PyValue::Dict(_) => "dict",
            PyValue::Pdf(o) => match o {
                PDFObject::Literal(_) => "PSLiteral",
                PDFObject::Keyword(_) => "PSKeyword",
                PDFObject::Stream(_) => "PDFStream",
                PDFObject::Ref(_) => "PDFObjRef",
                _ => self.as_python().expect("a Python value").type_name(),
            },
        }
    }

    /// `bool(v)`
    pub fn truthy(&self) -> bool {
        match self {
            PyValue::None => false,
            PyValue::Bool(b) => *b,
            PyValue::Int(i) => !i.is_zero(),
            PyValue::Float(x) => *x != 0.0,
            PyValue::Str(s) => !s.is_empty(),
            PyValue::Bytes(b) => !b.is_empty(),
            PyValue::Tuple(t) | PyValue::List(t) => !t.is_empty(),
            PyValue::Dict(d) => !d.is_empty(),
            // names, keywords, streams and references are always true
            PyValue::Pdf(_) => self.as_python().is_none_or(|v| v.truthy()),
        }
    }

    /// The number, for `bool`, `int` and `float` (`bool` is an `int`).
    #[inline]
    pub fn as_num(&self) -> Option<Num> {
        match self {
            PyValue::Float(x) => Some(Num::Float(*x)),
            PyValue::Int(i) => Some(Num::Int(i.clone())),
            PyValue::Bool(b) => Some(Num::Int(PyInt::Small(i64::from(*b)))),
            PyValue::Pdf(_) => self.as_python()?.as_num(),
            _ => None,
        }
    }

    /// The text of a `str` without surrogates (a `str` with one is equal
    /// to no Rust text).
    pub fn as_str(&self) -> Option<&str> {
        match self {
            PyValue::Str(s) => s.as_str(),
            _ => None,
        }
    }

    /// A `str`.
    pub fn as_pystr(&self) -> Option<&PyStr> {
        match self {
            PyValue::Str(s) => Some(s),
            _ => None,
        }
    }

    /// The items of a `tuple` or `list`.
    pub fn as_seq(&self) -> Option<&[PyValue]> {
        match self {
            PyValue::Tuple(t) | PyValue::List(t) => Some(t),
            _ => None,
        }
    }

    pub fn is_none(&self) -> bool {
        matches!(self, PyValue::None | PyValue::Pdf(PDFObject::Null))
    }

    /// `repr(v)`
    pub fn repr(&self) -> String {
        let mut out = String::new();
        self.write_repr(&mut out);
        out
    }

    fn write_repr(&self, out: &mut String) {
        match self {
            PyValue::None => out.push_str("None"),
            PyValue::Bool(b) => out.push_str(if *b { "True" } else { "False" }),
            PyValue::Int(i) => {
                let _ = write!(out, "{i}");
            }
            PyValue::Float(x) => out.push_str(&float_repr(*x)),
            PyValue::Str(s) => out.push_str(&s.repr()),
            PyValue::Bytes(b) => out.push_str(&bytes_repr(b)),
            PyValue::Tuple(t) => {
                out.push('(');
                for (i, v) in t.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    v.write_repr(out);
                }
                if t.len() == 1 {
                    out.push(',');
                }
                out.push(')');
            }
            PyValue::List(t) => {
                out.push('[');
                for (i, v) in t.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    v.write_repr(out);
                }
                out.push(']');
            }
            PyValue::Dict(d) => {
                out.push('{');
                for (i, (k, v)) in d.items().iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&k.repr());
                    out.push_str(": ");
                    v.write_repr(out);
                }
                out.push('}');
            }
            PyValue::Pdf(o) => match o.repr() {
                Ok(text) => out.push_str(&text),
                Err(_) => out.push_str("<?>"),
            },
        }
    }

    /// `str(v)` (`repr` except for `str` itself).
    pub fn py_str(&self) -> PyStr {
        match self {
            PyValue::Str(s) => s.clone(),
            other => PyStr::from(other.repr()),
        }
    }

    // -- ==, <, hash ------------------------------------------------------------

    /// `a is b or a == b` (`PyObject_RichCompareBool`): `==` with the
    /// identity shortcut, which only a NaN can tell apart (see the module
    /// documentation; a `dict` is equal to itself through [`Obj::py_eq`]).
    #[inline]
    pub fn eq_bool(&self, other: &PyValue) -> bool {
        if let (PyValue::Float(a), PyValue::Float(b)) = (self, other) {
            return a == b || (a.is_nan() && a.to_bits() == b.to_bits());
        }
        self.py_eq(other)
    }

    /// `a == b`, the operator: a NaN equals nothing; the items of
    /// containers compare with [`PyValue::eq_bool`].
    pub fn py_eq(&self, other: &PyValue) -> bool {
        if let (PyValue::Float(a), PyValue::Float(b)) = (self, other) {
            return a == b;
        }
        if let Some((a, b)) = self.as_python2(other) {
            return a.py_eq(&b);
        }
        if let (Some(a), Some(b)) = (self.as_num(), other.as_num()) {
            return a.py_eq(&b);
        }
        match (self, other) {
            (PyValue::None, PyValue::None) => true,
            (PyValue::Str(a), PyValue::Str(b)) => a == b,
            (PyValue::Bytes(a), PyValue::Bytes(b)) => a == b,
            (PyValue::Tuple(a), PyValue::Tuple(b)) | (PyValue::List(a), PyValue::List(b)) => {
                a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| x.eq_bool(y))
            }
            (PyValue::Dict(a), PyValue::Dict(b)) => a.py_eq(b),
            (PyValue::Pdf(a), PyValue::Pdf(b)) => a == b,
            _ => false,
        }
    }

    /// `a < b`, `a <= b`, `a > b` or `a >= b`, with CPython's `TypeError`
    /// for unorderable types.
    pub fn compare(&self, other: &PyValue, op: CmpOp) -> PyResult<bool> {
        if let (PyValue::Float(a), PyValue::Float(b)) = (self, other) {
            return Ok(match op {
                CmpOp::Lt => a < b,
                CmpOp::Le => a <= b,
                CmpOp::Gt => a > b,
                CmpOp::Ge => a >= b,
            });
        }
        if let Some((a, b)) = self.as_python2(other) {
            return a.compare(&b, op);
        }
        if let (Some(a), Some(b)) = (self.as_num(), other.as_num()) {
            return Ok(a.py_cmp(&b).is_some_and(|o| op.holds(o)));
        }
        match (self, other) {
            (PyValue::Str(a), PyValue::Str(b)) => Ok(op.holds(a.cmp(b))),
            (PyValue::Bytes(a), PyValue::Bytes(b)) => Ok(op.holds(a.as_ref().cmp(b.as_ref()))),
            (PyValue::Tuple(a), PyValue::Tuple(b)) | (PyValue::List(a), PyValue::List(b)) => {
                // tuplerichcompare / list_richcompare: the first items that
                // differ decide, else the lengths.
                for (x, y) in a.iter().zip(b.iter()) {
                    if !x.eq_bool(y) {
                        return x.compare(y, op);
                    }
                }
                Ok(op.holds(a.len().cmp(&b.len())))
            }
            _ => Err(exc::TypeError(format!(
                "'{}' not supported between instances of '{}' and '{}'",
                op.symbol(),
                self.type_name(),
                other.type_name()
            ))),
        }
    }

    /// `a < b`
    #[inline]
    pub fn lt(&self, other: &PyValue) -> PyResult<bool> {
        self.compare(other, CmpOp::Lt)
    }

    /// `a <= b`
    #[inline]
    pub fn le(&self, other: &PyValue) -> PyResult<bool> {
        self.compare(other, CmpOp::Le)
    }

    /// `a > b`
    #[inline]
    pub fn gt(&self, other: &PyValue) -> PyResult<bool> {
        self.compare(other, CmpOp::Gt)
    }

    /// `a >= b`
    #[inline]
    pub fn ge(&self, other: &PyValue) -> PyResult<bool> {
        self.compare(other, CmpOp::Ge)
    }

    // -- arithmetic -------------------------------------------------------------

    fn binop_error(&self, op: &str, other: &PyValue) -> PyErr {
        match (self, op) {
            (PyValue::Str(_), "+") => {
                exc::TypeError(format!("can only concatenate str (not \"{}\") to str", other.type_name()))
            }
            (PyValue::Tuple(_), "+") => {
                exc::TypeError(format!("can only concatenate tuple (not \"{}\") to tuple", other.type_name()))
            }
            (PyValue::List(_), "+") => {
                exc::TypeError(format!("can only concatenate list (not \"{}\") to list", other.type_name()))
            }
            (PyValue::Bytes(_), "+") => exc::TypeError(format!("can't concat {} to bytes", other.type_name())),
            _ => exc::TypeError(format!(
                "unsupported operand type(s) for {op}: '{}' and '{}'",
                self.type_name(),
                other.type_name()
            )),
        }
    }

    /// The numbers of a binary operation, or its error (the sequence
    /// operations, `str + str` or `list * int`, are [`PyValue::add`]'s and
    /// [`PyValue::mul`]'s).
    fn num_operands(&self, op: &str, other: &PyValue) -> PyResult<(Num, Num)> {
        if let Some((a, b)) = self.as_python2(other) {
            return a.num_operands(op, &b);
        }
        match (self.as_num(), other.as_num()) {
            (Some(a), Some(b)) => Ok((a, b)),
            _ => {
                if op == "*" && (self.is_sequence() || other.is_sequence()) {
                    let non_int = if self.is_sequence() { other } else { self };
                    return Err(exc::TypeError(format!(
                        "can't multiply sequence by non-int of type '{}'",
                        non_int.type_name()
                    )));
                }
                Err(self.binop_error(op, other))
            }
        }
    }

    /// A `str`, `bytes`, `tuple` or `list` (the sequences with `+` and `*`).
    fn is_sequence(&self) -> bool {
        matches!(self, PyValue::Str(_) | PyValue::Bytes(_) | PyValue::Tuple(_) | PyValue::List(_))
    }

    /// `a + b` of two sequences of the same type (`unicode_concatenate`,
    /// `bytes_concat`, `tuple_concat`, `list_concat`), `None` for other
    /// operands.
    fn seq_concat(&self, other: &PyValue) -> Option<PyResult<PyValue>> {
        if let Some((a, b)) = self.as_python2(other) {
            return a.seq_concat(&b);
        }
        Some(Ok(match (self, other) {
            (PyValue::Str(a), PyValue::Str(b)) => PyValue::Str(a.concat(b)),
            (PyValue::Bytes(a), PyValue::Bytes(b)) => {
                if let Err(e) = check_repeat_len(a.len().checked_add(b.len())) {
                    return Some(Err(e));
                }
                PyValue::Bytes(a.iter().chain(b.iter()).copied().collect())
            }
            (PyValue::Tuple(a), PyValue::Tuple(b)) => PyValue::Tuple(a.iter().chain(b.iter()).cloned().collect()),
            (PyValue::List(a), PyValue::List(b)) => PyValue::List(a.iter().chain(b.iter()).cloned().collect()),
            _ => return None,
        }))
    }

    /// `seq * n` or `n * seq` for a sequence and an `int` (`bool`
    /// included), `None` for other operands. `n` must fit a `Py_ssize_t`
    /// (`OverflowError`); `n <= 0` gives an empty sequence; a result longer
    /// than `PY_SSIZE_T_MAX` raises CPython's error (`OverflowError` for
    /// `str` and `bytes`, `MemoryError` for `tuple` and `list`); beyond
    /// [`MAX_REPEAT`] items the port raises `MemoryError` (a resource
    /// guard, where Python would try to allocate).
    fn seq_repeat(&self, other: &PyValue) -> Option<PyResult<PyValue>> {
        if let Some((a, b)) = self.as_python2(other) {
            return a.seq_repeat(&b);
        }
        let (seq, n) = match (self, other) {
            (s, PyValue::Int(_) | PyValue::Bool(_)) if s.is_sequence() => (s, other),
            (PyValue::Int(_) | PyValue::Bool(_), s) if s.is_sequence() => (s, self),
            _ => return None,
        };
        Some(seq.repeat(n))
    }

    fn repeat(&self, n: &PyValue) -> PyResult<PyValue> {
        let n = match n {
            PyValue::Bool(b) => i64::from(*b),
            PyValue::Int(i) => {
                i.as_i64().ok_or_else(|| exc::OverflowError("cannot fit 'int' into an index-sized integer"))?
            }
            _ => unreachable!("an int"),
        };
        let n = n.max(0) as u64;
        let len = match self {
            PyValue::Str(s) => s.len(),
            PyValue::Bytes(b) => b.len(),
            PyValue::Tuple(t) | PyValue::List(t) => t.len(),
            _ => unreachable!("a sequence"),
        } as u64;
        if n > 0 && len > i64::MAX as u64 / n {
            return Err(match self {
                PyValue::Str(_) => exc::OverflowError("repeated string is too long"),
                PyValue::Bytes(_) => exc::OverflowError("repeated bytes are too long"),
                _ => exc::MemoryError(""),
            });
        }
        check_repeat_len(usize::try_from(len * n).ok())?;
        let n = n as usize;
        Ok(match self {
            PyValue::Str(s) => PyValue::Str(s.repeat(n)),
            PyValue::Bytes(b) => PyValue::Bytes(Rc::from(b.repeat(n))),
            PyValue::Tuple(t) => PyValue::Tuple(t.iter().cloned().cycle().take(t.len() * n).collect()),
            PyValue::List(t) => PyValue::List(t.iter().cloned().cycle().take(t.len() * n).collect()),
            _ => unreachable!("a sequence"),
        })
    }

    /// `a + b`
    #[inline]
    pub fn add(&self, other: &PyValue) -> PyResult<PyValue> {
        if let (PyValue::Float(a), PyValue::Float(b)) = (self, other) {
            return Ok(PyValue::float(a + b));
        }
        if let Some(r) = self.seq_concat(other) {
            return r;
        }
        let (a, b) = self.num_operands("+", other)?;
        Ok(a.add(&b)?.into())
    }

    /// `a - b`
    #[inline]
    pub fn sub(&self, other: &PyValue) -> PyResult<PyValue> {
        if let (PyValue::Float(a), PyValue::Float(b)) = (self, other) {
            return Ok(PyValue::float(a - b));
        }
        let (a, b) = self.num_operands("-", other)?;
        Ok(a.sub(&b)?.into())
    }

    /// `a * b`
    pub fn mul(&self, other: &PyValue) -> PyResult<PyValue> {
        if let (PyValue::Float(a), PyValue::Float(b)) = (self, other) {
            return Ok(PyValue::float(a * b));
        }
        if let Some(r) = self.seq_repeat(other) {
            return r;
        }
        let (a, b) = self.num_operands("*", other)?;
        Ok(a.mul(&b)?.into())
    }

    /// `a / b`
    pub fn truediv(&self, other: &PyValue) -> PyResult<PyValue> {
        if let (PyValue::Float(a), PyValue::Float(b)) = (self, other) {
            return Ok(PyValue::float(float_div(*a, *b)?));
        }
        let (a, b) = self.num_operands("/", other)?;
        Ok(a.truediv(&b)?.into())
    }

    /// `-a`
    pub fn neg(&self) -> PyResult<PyValue> {
        match self.as_num() {
            Some(n) => Ok(n.neg().into()),
            None => Err(exc::TypeError(format!("bad operand type for unary -: '{}'", self.type_name()))),
        }
    }

    /// `abs(a)`
    pub fn abs(&self) -> PyResult<PyValue> {
        match self.as_num() {
            Some(n) => Ok(n.abs().into()),
            None => Err(exc::TypeError(format!("bad operand type for abs(): '{}'", self.type_name()))),
        }
    }

    /// `float(a)`: a number, or the parse of a `str` or `bytes`
    /// (`PyFloat_FromString`).
    pub fn to_float(&self) -> PyResult<PyValue> {
        if let Some(p) = self.as_python() {
            return p.to_float();
        }
        match self {
            // float(x) of a float is x itself (PyNumber_Float)
            PyValue::Float(x) => Ok(PyValue::Float(*x)),
            PyValue::Int(i) => Ok(PyValue::Float(i.to_f64()?)),
            PyValue::Bool(b) => Ok(PyValue::Float(f64::from(u8::from(*b)))),
            PyValue::Str(s) => match s.as_str() {
                Some(text) => Ok(PyValue::float(float_from_str(text)?)),
                // _PyUnicode_TransformDecimalAndSpaceToASCII stops at the
                // surrogate (neither a space nor a digit): no parse
                None => Err(exc::ValueError(format!("could not convert string to float: {}", s.repr()))),
            },
            PyValue::Bytes(b) => Ok(PyValue::float(float_from_bytes(b)?)),
            other => Err(exc::TypeError(format!(
                "float() argument must be a string or a real number, not '{}'",
                other.type_name()
            ))),
        }
    }
}

/// `min(items)` (first of equal items wins).
pub fn py_min<'a>(items: impl IntoIterator<Item = &'a PyValue>) -> PyResult<PyValue> {
    let mut iter = items.into_iter();
    let mut best = iter.next().ok_or_else(|| exc::ValueError("min() iterable argument is empty"))?;
    for item in iter {
        if item.lt(best)? {
            best = item;
        }
    }
    Ok(best.clone())
}

/// `max(items)` (first of equal items wins).
pub fn py_max<'a>(items: impl IntoIterator<Item = &'a PyValue>) -> PyResult<PyValue> {
    let mut iter = items.into_iter();
    let mut best = iter.next().ok_or_else(|| exc::ValueError("max() iterable argument is empty"))?;
    for item in iter {
        if item.gt(best)? {
            best = item;
        }
    }
    Ok(best.clone())
}

/// `sum(items)`: Python 3.12 `sum()` over numbers (`pycompat::py_sum`),
/// `TypeError` for anything else (the start value is `0`).
pub fn py_sum<'a>(items: impl IntoIterator<Item = &'a PyValue>) -> PyResult<PyValue> {
    let mut nums = Vec::new();
    let mut acc_type_err = None;
    for item in items {
        match item.as_num() {
            Some(n) => nums.push(n),
            None => {
                acc_type_err = Some(item);
                break;
            }
        }
    }
    let total = pycompat::py_sum0(nums)?;
    match acc_type_err {
        None => Ok(total.into()),
        Some(item) => Err(PyValue::from(total).binop_error("+", item)),
    }
}

/// The resource guard of repeated sequences (`[x] * n`, `"ab" * n`, a
/// loop over `range(n)` that appends): a length no real page reaches.
/// Python would try to allocate (`MemoryError` when it cannot, which is
/// certain from about 2**44 items on, beyond any address space) or loop
/// until memory runs out.
pub const MAX_REPEAT: usize = 10_000_000;

/// `MemoryError` for a sequence longer than [`MAX_REPEAT`] (`None`: longer
/// than `usize`).
fn check_repeat_len(len: Option<usize>) -> PyResult<()> {
    match len {
        Some(n) if n <= MAX_REPEAT => Ok(()),
        _ => Err(exc::MemoryError("")),
    }
}

/// The items of `iter(v)` (a `for` loop, `list(v)`, unpacking, `*v`):
/// `None` when `v` is not iterable (each caller raises its own
/// `TypeError`). A `str` gives its characters, `bytes` its ints, a `dict`
/// its keys. A `PDFStream` iterates through `__getitem__`, whose
/// `self.attrs[0]` raises `KeyError(0)`.
pub fn py_iter(v: &PyValue) -> Option<PyResult<Vec<PyValue>>> {
    if let PyValue::Pdf(PDFObject::Stream(stream)) = v {
        return Some(Err(stream_missing_attribute(stream, "0")));
    }
    if let Some(p) = v.as_python() {
        return py_iter(&p);
    }
    Some(Ok(match v {
        PyValue::Str(s) => s.code_points().map(|c| PyValue::Str(PyStr::from_code_point(c))).collect(),
        PyValue::Bytes(b) => b.iter().map(|&x| PyValue::from(i64::from(x))).collect(),
        PyValue::Tuple(t) | PyValue::List(t) => t.to_vec(),
        PyValue::Dict(d) => d.keys().map(|k| PyValue::Str(k.clone())).collect(),
        _ => return None,
    }))
}

/// The `PDFKeyError` of `PDFStream.__getitem__` for a missing attribute
/// (`str(e)` is the `repr()` of its message, as for any `KeyError`).
fn stream_missing_attribute(stream: &pdfminer::pdftypes::PDFStream, name: &str) -> PyErr {
    let objid = stream.objid().map_or_else(|| "None".to_owned(), |i| i.to_string());
    exc::PDFKeyError(str_repr(&format!("PDF stream object {objid} does not have attribute '{name}'")))
}

/// `v[i]` for an `int` index `i >= 0` (`BBOX_ORIGIN_KEYS` reads a bbox so):
/// a `str` gives its character, `bytes` an `int`, a `dict` (whose keys
/// are `str`) raises `KeyError`, a `PDFStream` its `PDFKeyError`.
pub fn py_getitem_index(v: &PyValue, i: usize) -> PyResult<PyValue> {
    if let PyValue::Pdf(PDFObject::Stream(stream)) = v {
        return Err(stream_missing_attribute(stream, &i.to_string()));
    }
    if let Some(p) = v.as_python() {
        return py_getitem_index(&p, i);
    }
    match v {
        PyValue::Tuple(t) => t.get(i).cloned().ok_or_else(|| exc::IndexError("tuple index out of range")),
        PyValue::List(t) => t.get(i).cloned().ok_or_else(|| exc::IndexError("list index out of range")),
        PyValue::Str(s) => s
            .code_points()
            .nth(i)
            .map(|c| PyValue::Str(PyStr::from_code_point(c)))
            .ok_or_else(|| exc::IndexError("string index out of range")),
        PyValue::Bytes(b) => {
            b.get(i).map(|&x| PyValue::from(i64::from(x))).ok_or_else(|| exc::IndexError("index out of range"))
        }
        PyValue::Dict(_) => Err(exc::KeyError(i.to_string())),
        other => Err(exc::TypeError(format!("'{}' object is not subscriptable", other.type_name()))),
    }
}

/// `item in container` (`PySequence_Contains`): a substring test for a
/// `str` (whose left operand must be a `str`) and for `bytes`, `==` with
/// each item (after an identity check, see the module documentation) for
/// a `tuple` or `list`, a key look-up for a `dict` and for a `PDFStream`
/// (`__contains__`, on its attributes); `TypeError` for a container that is
/// not iterable.
pub fn py_contains(container: &PyValue, item: &PyValue) -> PyResult<bool> {
    if let Some(p) = item.as_python() {
        return py_contains(container, &p);
    }
    if let PyValue::Pdf(PDFObject::Stream(stream)) = container {
        // PDFStream.__contains__: `name in self.attrs`
        check_hashable(item)?;
        return Ok(match item {
            PyValue::Str(k) => k.as_str().is_some_and(|k| stream.attrs.get(k).is_some()),
            _ => false, // the keys are str
        });
    }
    if let Some(p) = container.as_python() {
        return py_contains(&p, item);
    }
    match container {
        PyValue::Str(s) => match item {
            PyValue::Str(sub) => Ok(s.contains(sub)),
            other => {
                Err(exc::TypeError(format!("'in <string>' requires string as left operand, not {}", other.type_name())))
            }
        },
        PyValue::Bytes(b) => match item {
            PyValue::Bytes(sub) => Ok(sub.is_empty() || b.windows(sub.len()).any(|w| w == &sub[..])),
            PyValue::Int(i) => match i.as_i64() {
                Some(x @ 0..=255) => Ok(b.contains(&(x as u8))),
                _ => Err(exc::ValueError("byte must be in range(0, 256)")),
            },
            PyValue::Bool(x) => Ok(b.contains(&u8::from(*x))),
            other => Err(exc::TypeError(format!("a bytes-like object is required, not '{}'", other.type_name()))),
        },
        PyValue::Tuple(t) | PyValue::List(t) => Ok(t.iter().any(|x| x.eq_bool(item))),
        PyValue::Dict(d) => {
            check_hashable(item)?;
            Ok(match item {
                PyValue::Str(k) => d.contains(k),
                _ => false, // the keys are str
            })
        }
        other => Err(exc::TypeError(format!("argument of type '{}' is not iterable", other.type_name()))),
    }
}

// -- hashing ------------------------------------------------------------------

/// A value used as a `dict` key or `set` member: Python's `hash()` and
/// `==` (`3 == 3.0 == True`); unhashable values (`list`, `dict`) raise
/// `TypeError` when the key is made.
#[derive(Clone, Debug)]
pub struct HashKey(pub PyValue);

impl HashKey {
    pub fn new(v: PyValue) -> PyResult<HashKey> {
        check_hashable(&v)?;
        Ok(HashKey(v))
    }
}

/// `hash(v)` succeeds: `TypeError("unhashable type: ...")` for a `list` or
/// `dict` (also inside a `tuple`).
pub fn check_hashable(v: &PyValue) -> PyResult<()> {
    if let Some(p) = v.as_python() {
        return check_hashable(&p);
    }
    match v {
        PyValue::List(_) | PyValue::Dict(_) => Err(exc::TypeError(format!("unhashable type: '{}'", v.type_name()))),
        PyValue::Tuple(t) => t.iter().try_for_each(check_hashable),
        _ => Ok(()),
    }
}

/// Feeds a number to a hasher, consistently with `==` between numbers
/// (`1 == 1.0 == True`): integral values in the `i64` range hash as that
/// integer, other integral values by Python's `hash()` (which agrees
/// between `int` and `float`), other floats by their bits.
fn hash_num<H: Hasher>(v: &PyValue, state: &mut H) -> bool {
    const TWO_63: f64 = 9_223_372_036_854_775_808.0;
    match v {
        PyValue::Float(x) => {
            if x.fract() == 0.0 && *x >= -TWO_63 && *x < TWO_63 {
                state.write_u8(0);
                state.write_i64(*x as i64);
            } else if x.is_finite() && x.fract() == 0.0 {
                state.write_u8(1);
                state.write_i64(pycompat::num::float_hash(*x).unwrap_or(0));
            } else {
                state.write_u8(2);
                state.write_u64(x.to_bits());
            }
        }
        PyValue::Int(PyInt::Small(i)) => {
            state.write_u8(0);
            state.write_i64(*i);
        }
        PyValue::Int(i) => {
            state.write_u8(1);
            state.write_i64(pycompat::num::int_hash(i));
        }
        PyValue::Bool(b) => {
            state.write_u8(0);
            state.write_i64(i64::from(*b));
        }
        _ => return false,
    }
    true
}

fn hash_value<H: Hasher>(v: &PyValue, state: &mut H) {
    if let Some(p) = v.as_python() {
        return hash_value(&p, state);
    }
    if hash_num(v, state) {
        return;
    }
    match v {
        PyValue::None => state.write_u8(1),
        PyValue::Str(s) => {
            state.write_u8(2);
            s.hash(state);
        }
        PyValue::Bytes(b) => {
            state.write_u8(3);
            b.hash(state);
        }
        PyValue::Tuple(t) => {
            state.write_u8(4);
            state.write_usize(t.len());
            for x in t.iter() {
                hash_value(x, state);
            }
        }
        _ => state.write_u8(5),
    }
}

/// Set and dict keys compare with the identity shortcut.
impl PartialEq for HashKey {
    fn eq(&self, other: &HashKey) -> bool {
        self.0.eq_bool(&other.0)
    }
}

impl Eq for HashKey {}

impl Hash for HashKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        hash_value(&self.0, state);
    }
}

// -- dicts --------------------------------------------------------------------

#[derive(Clone)]
struct ObjData {
    items: Vec<(PyStr, PyValue)>,
    /// Position + 1 of each [`Hot`] key in `items` (0: absent).
    hot: [u32; HOT_N],
}

/// A pdfplumber object: an insertion-ordered `dict` with `str` keys (see the
/// module documentation).
#[derive(Clone)]
pub struct Obj(Rc<ObjData>);

impl Default for Obj {
    fn default() -> Obj {
        Obj::new()
    }
}

impl std::fmt::Debug for Obj {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&PyValue::Dict(self.clone()).repr())
    }
}

impl Obj {
    /// `{}`
    pub fn new() -> Obj {
        Obj(Rc::new(ObjData { items: Vec::new(), hot: [0; HOT_N] }))
    }

    /// `dict(pairs)`: a repeated key keeps its first position and its last
    /// value.
    pub fn from_pairs<K: Key>(pairs: impl IntoIterator<Item = (K, PyValue)>) -> Obj {
        let pairs = pairs.into_iter();
        let mut o = Obj(Rc::new(ObjData { items: Vec::with_capacity(pairs.size_hint().0), hot: [0; HOT_N] }));
        for (k, v) in pairs {
            o.set(&k, v);
        }
        o
    }

    /// Same dict object (`a is b`).
    pub fn ptr_eq(&self, other: &Obj) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }

    pub fn len(&self) -> usize {
        self.0.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.items.is_empty()
    }

    /// `(key, value)` pairs, in order.
    pub fn items(&self) -> &[(PyStr, PyValue)] {
        &self.0.items
    }

    fn position(&self, key: &(impl Key + ?Sized)) -> Option<usize> {
        let key = key.key_bytes();
        if let Some(h) = Hot::of_bytes(key) {
            return self.hot_position(h);
        }
        self.0.items.iter().position(|(k, _)| k.as_bytes() == key)
    }

    #[inline]
    fn hot_position(&self, h: Hot) -> Option<usize> {
        match self.0.hot[h as usize] {
            0 => None,
            p => Some(p as usize - 1),
        }
    }

    /// `obj.get(key)`
    pub fn get(&self, key: &(impl Key + ?Sized)) -> Option<&PyValue> {
        self.position(key).map(|p| &self.0.items[p].1)
    }

    /// `obj.get(key)` for a hot key.
    #[inline]
    pub fn get_hot(&self, h: Hot) -> Option<&PyValue> {
        self.hot_position(h).map(|p| &self.0.items[p].1)
    }

    /// `obj[key]`
    pub fn item(&self, key: &(impl Key + ?Sized)) -> PyResult<&PyValue> {
        self.get(key).ok_or_else(|| key_error(key))
    }

    /// `obj[key]` for a hot key.
    #[inline]
    pub fn hot(&self, h: Hot) -> PyResult<&PyValue> {
        self.get_hot(h).ok_or_else(|| key_error(h.name()))
    }

    /// `key in obj`
    pub fn contains(&self, key: &(impl Key + ?Sized)) -> bool {
        self.position(key).is_some()
    }

    /// `obj[key] = value` (copies a shared dict first).
    pub fn set(&mut self, key: &(impl Key + ?Sized), value: PyValue) {
        let bytes = key.key_bytes();
        let hot = Hot::of_bytes(bytes);
        let position = match hot {
            Some(h) => self.hot_position(h),
            None => self.0.items.iter().position(|(k, _)| k.as_bytes() == bytes),
        };
        if let Some(p) = position {
            Rc::make_mut(&mut self.0).items[p].1 = value;
            return;
        }
        let data = Rc::make_mut(&mut self.0);
        match hot {
            Some(h) => {
                data.items.push((hot_key(h), value));
                data.hot[h as usize] = data.items.len() as u32;
            }
            None => data.items.push((key.to_key(), value)),
        }
    }

    /// `del obj[key]` (`KeyError` when absent).
    pub fn remove(&mut self, key: &(impl Key + ?Sized)) -> PyResult<PyValue> {
        let p = self.position(key).ok_or_else(|| key_error(key))?;
        let data = Rc::make_mut(&mut self.0);
        let (_, v) = data.items.remove(p);
        data.hot = [0; HOT_N];
        for (i, (k, _)) in data.items.iter().enumerate() {
            if let Some(h) = Hot::of_bytes(k.as_bytes()) {
                data.hot[h as usize] = i as u32 + 1;
            }
        }
        Ok(v)
    }

    /// `obj.update(pairs)`
    pub fn update<K: Key>(&mut self, pairs: impl IntoIterator<Item = (K, PyValue)>) {
        for (k, v) in pairs {
            self.set(&k, v);
        }
    }

    /// The keys, in order.
    pub fn keys(&self) -> impl Iterator<Item = &PyStr> {
        self.0.items.iter().map(|(k, _)| k)
    }

    /// `a == b` for dicts: same keys, equal values (order-insensitive).
    pub fn py_eq(&self, other: &Obj) -> bool {
        if self.ptr_eq(other) {
            // dict_equal compares values with PyObject_RichCompareBool,
            // whose identity shortcut makes a dict equal to itself.
            return true;
        }
        self.len() == other.len() && self.0.items.iter().all(|(k, v)| other.get(k).is_some_and(|w| v.eq_bool(w)))
    }
}

/// A bounding box as pdfplumber passes it around: `(x0, top, x1, bottom)`
/// (pdfplumber/_typing.py:6 T_bbox), of any values.
pub type Bbox = [PyValue; 4];

/// The `tuple` of a bbox.
pub fn bbox_value(b: &Bbox) -> PyValue {
    PyValue::tuple(b.iter().cloned())
}

/// `x0, top, x1, bottom = value`: a bbox from a `tuple`/`list` of four
/// items (`ValueError` for another length, `TypeError` for a non-sequence).
pub fn bbox_from_value(v: &PyValue) -> PyResult<Bbox> {
    let Some(items) = py_iter(v) else {
        return Err(exc::TypeError(format!("cannot unpack non-iterable {} object", v.type_name())));
    };
    let items = items?;
    match &items[..] {
        [a, b, c, d] => Ok([a.clone(), b.clone(), c.clone(), d.clone()]),
        _ if items.len() > 4 => Err(exc::ValueError("too many values to unpack (expected 4)")),
        _ => Err(exc::ValueError(format!("not enough values to unpack (expected 4, got {})", items.len()))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dict_semantics() {
        let mut a = Obj::from_pairs([("x0", PyValue::from(1.0)), ("k", PyValue::from(2i64))]);
        let b = a.clone();
        a.set("x0", PyValue::from(3.0));
        assert!(b.get("x0").unwrap().py_eq(&PyValue::from(1.0)));
        assert_eq!(a.keys().collect::<Vec<_>>(), ["x0", "k"]);
        a.set("top", PyValue::None);
        assert!(a.get_hot(Hot::Top).unwrap().is_none());
        a.remove("x0").unwrap();
        assert_eq!(a.keys().collect::<Vec<_>>(), ["k", "top"]);
        assert!(a.get_hot(Hot::Top).is_some());
        assert_eq!(a.item("zz").unwrap_err().message, "'zz'");
    }

    #[test]
    fn operators() {
        let one = PyValue::from(1i64);
        let half = PyValue::from(0.5);
        assert!(matches!(one.add(&one).unwrap(), PyValue::Int(_)));
        assert!(matches!(one.add(&half).unwrap(), PyValue::Float(_)));
        let e = PyValue::None.lt(&half).unwrap_err();
        assert_eq!(e.message, "'<' not supported between instances of 'NoneType' and 'float'");
        let e = half.add(&PyValue::None).unwrap_err();
        assert_eq!(e.message, "unsupported operand type(s) for +: 'float' and 'NoneType'");
        assert!(PyValue::tuple([one.clone(), half.clone()]).lt(&PyValue::tuple([one.clone(), one.clone()])).unwrap());
        assert_eq!(PyValue::tuple([one.clone()]).repr(), "(1,)");
    }
}
