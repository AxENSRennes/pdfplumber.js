//! A dynamic Python value, the common currency of traces and differential
//! tests.
//!
//! [`Value`] covers what a trace or a typed JSON case can hold: Python
//! scalars and containers, pdfminer's PDF types, an exception, and a
//! "named" object (`LTChar{...}`: a class name and its attributes, used by
//! later stages for the objects they trace). Conversions exist from
//! [`PDFObject`] and [`PyErr`].
//!
//! Values may be nested arbitrarily deep (a parsed `[[[...]]]`): converting,
//! comparing, writing and dropping them is iterative, never limited by the
//! stack, as in `oracle/canon.py`.

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

/// The trace form of a `str` of the port.
impl From<&pycompat::PyStr> for PyStr {
    fn from(s: &pycompat::PyStr) -> PyStr {
        match s.as_str() {
            Some(text) => PyStr::Text(text.to_owned()),
            None => PyStr::CodePoints(s.code_points().collect()),
        }
    }
}

/// The `str` of the port of a traced `str`.
impl From<&PyStr> for pycompat::PyStr {
    fn from(s: &PyStr) -> pycompat::PyStr {
        match s {
            PyStr::Text(text) => pycompat::PyStr::new(text),
            PyStr::CodePoints(cps) => {
                pycompat::PyStr::from_code_points(cps.iter().copied()).expect("code points of a str")
            }
        }
    }
}

impl Value {
    /// A `str` value of the port.
    pub fn py_str(s: &pycompat::PyStr) -> Value {
        Value::Str(PyStr::from(s))
    }
}

/// A UTF-16 surrogate code point (U+D800..U+DFFF).
pub use pycompat::pystr::is_surrogate;

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

/// The values a container holds directly, in order (keys before their
/// values).
fn children(v: &Value) -> Vec<&Value> {
    match v {
        Value::List(items) | Value::Tuple(items) => items.iter().collect(),
        Value::Dict(pairs) | Value::Named { fields: pairs, .. } => pairs.iter().flat_map(|(k, v)| [k, v]).collect(),
        Value::Stream(s) => {
            let mut out = vec![&s.objid, &s.genno];
            out.extend(s.attrs.iter().flat_map(|(k, v)| [k, v]));
            out.extend([&s.raw, &s.data]);
            out
        }
        _ => Vec::new(),
    }
}

impl Drop for Value {
    /// Drops the values this one holds with a work list, not by recursion.
    fn drop(&mut self) {
        fn take_children(v: &mut Value, into: &mut Vec<Value>) {
            match v {
                Value::List(items) | Value::Tuple(items) => into.append(items),
                Value::Dict(pairs) | Value::Named { fields: pairs, .. } => {
                    into.extend(pairs.drain(..).flat_map(|(k, v)| [k, v]));
                }
                Value::Stream(s) => {
                    into.extend(s.attrs.drain(..).flat_map(|(k, v)| [k, v]));
                    for field in [&mut s.objid, &mut s.genno, &mut s.raw, &mut s.data] {
                        into.push(std::mem::replace(field, Value::None));
                    }
                }
                _ => {}
            }
        }
        let mut stack = Vec::new();
        take_children(self, &mut stack);
        while let Some(mut v) = stack.pop() {
            take_children(&mut v, &mut stack);
        }
    }
}

impl Value {
    /// Exact equality: floats compare by bit pattern (so `-0.0 != 0.0` and
    /// a NaN equals a NaN with the same bits), everything else by value.
    pub fn same(&self, other: &Value) -> bool {
        use Value::*;
        let mut pending = vec![(self, other)];
        while let Some((a, b)) = pending.pop() {
            let shallow = match (a, b) {
                (None, None) => true,
                (Bool(a), Bool(b)) => a == b,
                (Int(a), Int(b)) => a == b,
                (Float(a), Float(b)) => a.to_bits() == b.to_bits(),
                (Str(a), Str(b)) => a == b,
                (Bytes(a), Bytes(b)) => a == b,
                (Tuple(a), Tuple(b)) | (List(a), List(b)) => a.len() == b.len(),
                (Dict(a), Dict(b)) => a.len() == b.len(),
                (Literal(a), Literal(b)) => a == b,
                (Keyword(a), Keyword(b)) => a == b,
                (Ref(a), Ref(b)) => a == b,
                (Stream(a), Stream(b)) => a.attrs.len() == b.attrs.len(),
                (Exception { class: c1, message: m1 }, Exception { class: c2, message: m2 }) => c1 == c2 && m1 == m2,
                (Named { name: n1, fields: f1 }, Named { name: n2, fields: f2 }) => n1 == n2 && f1.len() == f2.len(),
                _ => false,
            };
            if !shallow {
                return false;
            }
            pending.extend(children(a).into_iter().zip(children(b)));
        }
        true
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
        Value::Exception { class: e.kind.trace_name(), message: PyStr::from(&e.py_message()) }
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
            attrs: dict_items(Value::from(&PDFObject::Dict(s.attrs.clone()))),
            raw: s.rawdata().map_or(Value::None, |r| Value::Bytes(r.to_vec())),
            data: s.data().map_or(Value::None, |d| Value::Bytes(d.to_vec())),
        }
    }
}

/// The items of a dict value.
fn dict_items(mut v: Value) -> Vec<(Value, Value)> {
    match &mut v {
        Value::Dict(items) => std::mem::take(items),
        other => unreachable!("not a dict: {other:?}"),
    }
}

/// A scalar PDF object (not a list, dict or stream).
fn scalar(obj: &PDFObject) -> Value {
    match obj {
        PDFObject::Null => Value::None,
        PDFObject::Bool(b) => Value::Bool(*b),
        PDFObject::Int(i) => Value::Int(i.clone()),
        PDFObject::Float(f) => Value::Float(*f),
        PDFObject::Bytes(b) => Value::Bytes(b.to_vec()),
        PDFObject::Literal(lit) => Value::Literal(NameValue::from(&lit.name())),
        PDFObject::Keyword(kwd) => Value::Keyword(kwd.name().to_vec()),
        PDFObject::Ref(r) => Value::Ref(r.objid().clone()),
        PDFObject::List(_) | PDFObject::Dict(_) | PDFObject::Stream(_) => unreachable!("a container"),
    }
}

impl From<&PDFObject> for Value {
    /// The value of an object, built bottom-up with an explicit stack.
    fn from(obj: &PDFObject) -> Value {
        enum Step {
            Enter(PDFObject),
            List(usize),
            Dict(Vec<std::rc::Rc<str>>),
            Stream(std::rc::Rc<PDFStream>, Vec<std::rc::Rc<str>>),
        }
        let mut steps = vec![Step::Enter(obj.clone())];
        let mut done: Vec<Value> = Vec::new();
        while let Some(step) = steps.pop() {
            match step {
                Step::Enter(PDFObject::List(l)) => {
                    let items = l.to_vec();
                    steps.push(Step::List(items.len()));
                    steps.extend(items.into_iter().rev().map(Step::Enter));
                }
                Step::Enter(PDFObject::Dict(d)) => {
                    let (keys, values): (Vec<_>, Vec<_>) = d.items().into_iter().unzip();
                    steps.push(Step::Dict(keys));
                    steps.extend(values.into_iter().rev().map(Step::Enter));
                }
                Step::Enter(PDFObject::Stream(s)) => {
                    let (keys, values): (Vec<_>, Vec<_>) = s.attrs.items().into_iter().unzip();
                    steps.push(Step::Stream(s.clone(), keys));
                    steps.extend(values.into_iter().rev().map(Step::Enter));
                }
                Step::Enter(leaf) => done.push(scalar(&leaf)),
                Step::List(n) => {
                    let items = done.split_off(done.len() - n);
                    done.push(Value::List(items));
                }
                Step::Dict(keys) => {
                    let values = done.split_off(done.len() - keys.len());
                    done.push(Value::Dict(keys.iter().map(|k| Value::str(k.to_string())).zip(values).collect()));
                }
                Step::Stream(s, keys) => {
                    let values = done.split_off(done.len() - keys.len());
                    done.push(Value::Stream(Box::new(StreamValue {
                        objid: opt_int(s.objid()),
                        genno: opt_int(s.genno()),
                        attrs: keys.iter().map(|k| Value::str(k.to_string())).zip(values).collect(),
                        raw: s.rawdata().map_or(Value::None, |r| Value::Bytes(r.to_vec())),
                        data: s.data().map_or(Value::None, |d| Value::Bytes(d.to_vec())),
                    })));
                }
            }
        }
        done.pop().expect("one value")
    }
}
