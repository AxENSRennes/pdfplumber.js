//! A dynamic Python value, the common currency of traces and differential
//! tests.
//!
//! [`Value`] covers what a trace or a typed JSON case can hold: Python
//! scalars and containers, pdfminer's PDF types, an exception, and a
//! "named" object (`LTChar{...}`: a class name and its attributes, used by
//! later stages for the objects they trace). Conversions exist from
//! [`PDFObject`] and [`PyErr`].

use pdfminer::pdftypes::{PDFObject, PDFStream};
use pdfminer::psparser::LiteralName;
use pycompat::{PyErr, PyInt};

/// A Python `str`.
///
/// A `str` holding a surrogate code point (U+D800..U+DFFF, e.g. `chr(0xd800)`)
/// is kept as its code points, since a Rust `String` cannot hold it; every
/// other `str` is [`PyStr::Text`]. [`PyStr::from_code_points`] keeps this
/// representation unique, so `==` is Python's `==`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum PyStr {
    Text(String),
    /// Code points, at least one of them a surrogate.
    CodePoints(Vec<u32>),
}

impl PyStr {
    /// The `str` of these code points: `None` if one is above U+10FFFF.
    pub fn from_code_points(code_points: Vec<u32>) -> Option<PyStr> {
        if code_points.iter().any(|&c| c > 0x10ffff) {
            return None;
        }
        if code_points.iter().any(|&c| is_surrogate(c)) {
            return Some(PyStr::CodePoints(code_points));
        }
        Some(PyStr::Text(code_points.into_iter().filter_map(char::from_u32).collect()))
    }

    /// The text, when the `str` has no surrogate.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            PyStr::Text(s) => Some(s),
            PyStr::CodePoints(_) => None,
        }
    }

    /// The code points (`[ord(c) for c in s]`).
    pub fn code_points(&self) -> Vec<u32> {
        match self {
            PyStr::Text(s) => s.chars().map(u32::from).collect(),
            PyStr::CodePoints(c) => c.clone(),
        }
    }
}

/// A UTF-16 surrogate code point (U+D800..U+DFFF).
pub fn is_surrogate(c: u32) -> bool {
    (0xd800..0xe000).contains(&c)
}

impl From<String> for PyStr {
    fn from(s: String) -> PyStr {
        PyStr::Text(s)
    }
}

impl From<&str> for PyStr {
    fn from(s: &str) -> PyStr {
        PyStr::Text(s.to_owned())
    }
}

impl PartialEq<str> for PyStr {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == Some(other)
    }
}

impl PartialEq<&str> for PyStr {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == Some(*other)
    }
}

/// The name of a literal: `str`, or `bytes` when it was not valid UTF-8.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NameValue {
    Str(PyStr),
    Bytes(Vec<u8>),
}

/// A `PDFStream` as traced: `Stream(objid, genno, attrs, raw)`.
#[derive(Clone, Debug)]
pub struct StreamValue {
    pub objid: Value,
    pub genno: Value,
    pub attrs: Vec<(Value, Value)>,
    /// `stream.rawdata` (`None` once the stream was decoded).
    pub raw: Value,
    /// `stream.data` (`None` until the stream is decoded). Typed JSON
    /// carries it; `canon` does not write it.
    pub data: Value,
}

#[derive(Clone, Debug)]
pub enum Value {
    None,
    Bool(bool),
    Int(PyInt),
    Float(f64),
    Str(PyStr),
    Bytes(Vec<u8>),
    Tuple(Vec<Value>),
    List(Vec<Value>),
    /// Insertion-ordered items.
    Dict(Vec<(Value, Value)>),
    Literal(NameValue),
    Keyword(Vec<u8>),
    /// `PDFObjRef`, by object number.
    Ref(PyInt),
    Stream(Box<StreamValue>),
    /// A raised exception: its class name (as `canon` writes it) and
    /// `str(e)`.
    Exception {
        class: String,
        message: PyStr,
    },
    /// An object traced as its class name and attributes (`LTChar{...}`).
    Named {
        name: String,
        fields: Vec<(Value, Value)>,
    },
}

impl Value {
    /// Exact equality: floats compare by bit pattern (so `-0.0 != 0.0` and
    /// a NaN equals a NaN with the same bits), everything else by value.
    pub fn same(&self, other: &Value) -> bool {
        use Value::*;
        let pairs_same = |a: &[(Value, Value)], b: &[(Value, Value)]| {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.0.same(&y.0) && x.1.same(&y.1))
        };
        let seq_same = |a: &[Value], b: &[Value]| a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.same(y));
        match (self, other) {
            (None, None) => true,
            (Bool(a), Bool(b)) => a == b,
            (Int(a), Int(b)) => a == b,
            (Float(a), Float(b)) => a.to_bits() == b.to_bits(),
            (Str(a), Str(b)) => a == b,
            (Bytes(a), Bytes(b)) => a == b,
            (Tuple(a), Tuple(b)) | (List(a), List(b)) => seq_same(a, b),
            (Dict(a), Dict(b)) => pairs_same(a, b),
            (Literal(a), Literal(b)) => a == b,
            (Keyword(a), Keyword(b)) => a == b,
            (Ref(a), Ref(b)) => a == b,
            (Stream(a), Stream(b)) => {
                a.objid.same(&b.objid)
                    && a.genno.same(&b.genno)
                    && pairs_same(&a.attrs, &b.attrs)
                    && a.raw.same(&b.raw)
                    && a.data.same(&b.data)
            }
            (Exception { class: c1, message: m1 }, Exception { class: c2, message: m2 }) => c1 == c2 && m1 == m2,
            (Named { name: n1, fields: f1 }, Named { name: n2, fields: f2 }) => n1 == n2 && pairs_same(f1, f2),
            _ => false,
        }
    }

    /// A `str` value.
    pub fn str(s: impl Into<String>) -> Value {
        Value::Str(PyStr::Text(s.into()))
    }

    /// The text of a `str` value without surrogates.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => s.as_str(),
            _ => None,
        }
    }

    /// A `str` key/value pair list from string keys.
    pub fn str_dict(items: impl IntoIterator<Item = (String, Value)>) -> Value {
        Value::Dict(items.into_iter().map(|(k, v)| (Value::str(k), v)).collect())
    }
}

impl From<&PyErr> for Value {
    fn from(e: &PyErr) -> Value {
        Value::Exception { class: e.kind.trace_name(), message: PyStr::Text(e.message.clone()) }
    }
}

impl From<&LiteralName> for NameValue {
    fn from(name: &LiteralName) -> NameValue {
        match name {
            LiteralName::Str(s) => NameValue::Str(PyStr::from(*s)),
            LiteralName::Bytes(b) => NameValue::Bytes(b.to_vec()),
        }
    }
}

fn opt_int(i: Option<PyInt>) -> Value {
    i.map_or(Value::None, Value::Int)
}

impl From<&PDFStream> for StreamValue {
    fn from(s: &PDFStream) -> StreamValue {
        StreamValue {
            objid: opt_int(s.objid()),
            genno: opt_int(s.genno()),
            attrs: s.attrs.items().into_iter().map(|(k, v)| (Value::str(k.to_string()), Value::from(&v))).collect(),
            raw: s.rawdata().map_or(Value::None, |r| Value::Bytes(r.to_vec())),
            data: s.data().map_or(Value::None, |d| Value::Bytes(d.to_vec())),
        }
    }
}

impl From<&PDFObject> for Value {
    fn from(obj: &PDFObject) -> Value {
        match obj {
            PDFObject::Null => Value::None,
            PDFObject::Bool(b) => Value::Bool(*b),
            PDFObject::Int(i) => Value::Int(i.clone()),
            PDFObject::Float(f) => Value::Float(*f),
            PDFObject::Bytes(b) => Value::Bytes(b.to_vec()),
            PDFObject::Literal(lit) => Value::Literal(NameValue::from(&lit.name())),
            PDFObject::Keyword(kwd) => Value::Keyword(kwd.name().to_vec()),
            PDFObject::List(l) => Value::List(l.borrow().iter().map(Value::from).collect()),
            PDFObject::Dict(d) => {
                Value::Dict(d.borrow().iter().map(|(k, v)| (Value::str(k.to_string()), Value::from(v))).collect())
            }
            PDFObject::Stream(s) => Value::Stream(Box::new(StreamValue::from(&**s))),
            PDFObject::Ref(r) => Value::Ref(r.objid().clone()),
        }
    }
}
