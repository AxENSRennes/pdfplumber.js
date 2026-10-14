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

/// The name of a literal: `str`, or `bytes` when it was not valid UTF-8.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NameValue {
    Str(String),
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
}

#[derive(Clone, Debug)]
pub enum Value {
    None,
    Bool(bool),
    Int(PyInt),
    Float(f64),
    Str(String),
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
    /// A raised exception: `type(e).__name__` and `str(e)`.
    Exception {
        class: String,
        message: String,
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
                a.objid.same(&b.objid) && a.genno.same(&b.genno) && pairs_same(&a.attrs, &b.attrs) && a.raw.same(&b.raw)
            }
            (Exception { class: c1, message: m1 }, Exception { class: c2, message: m2 }) => c1 == c2 && m1 == m2,
            (Named { name: n1, fields: f1 }, Named { name: n2, fields: f2 }) => n1 == n2 && pairs_same(f1, f2),
            _ => false,
        }
    }

    /// A `str` key/value pair list from string keys.
    pub fn str_dict(items: impl IntoIterator<Item = (String, Value)>) -> Value {
        Value::Dict(items.into_iter().map(|(k, v)| (Value::Str(k), v)).collect())
    }
}

impl From<&PyErr> for Value {
    fn from(e: &PyErr) -> Value {
        Value::Exception { class: e.kind.name().to_owned(), message: e.message.clone() }
    }
}

impl From<&LiteralName> for NameValue {
    fn from(name: &LiteralName) -> NameValue {
        match name {
            LiteralName::Str(s) => NameValue::Str((*s).to_owned()),
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
            attrs: s.attrs.items().into_iter().map(|(k, v)| (Value::Str(k.to_string()), Value::from(&v))).collect(),
            raw: s.rawdata().map_or(Value::None, |r| Value::Bytes(r.to_vec())),
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
                Value::Dict(d.borrow().iter().map(|(k, v)| (Value::Str(k.to_string()), Value::from(v))).collect())
            }
            PDFObject::Stream(s) => Value::Stream(Box::new(StreamValue::from(&**s))),
            PDFObject::Ref(r) => Value::Ref(r.objid().clone()),
        }
    }
}
