//! Conversions between [`Value`] (traces, typed JSON) and the pdfplumber
//! object model ([`pdfplumber::obj`]).
//!
//! pdfplumber objects are shallow (a page object's values are numbers,
//! strings, short tuples and lists): these conversions recurse. PDF objects
//! inside them ([`PyValue::Pdf`]) convert through [`Value::from`] for
//! `PDFObject`, which does not.

use std::rc::Rc;

use pdfminer::pdftypes::{PDFDict, PDFList, PDFObjRef, PDFObject, PDFStream};
use pdfminer::psparser::{LIT_bytes, KWD, LIT};
use pdfplumber::obj::{Obj, PyValue};

use crate::value::{NameValue, Value};

/// The value of a pdfplumber value.
pub fn to_value(v: &PyValue) -> Value {
    match v {
        PyValue::None => Value::None,
        PyValue::Bool(b) => Value::Bool(*b),
        PyValue::Int(i) => Value::Int(i.clone()),
        PyValue::Float(x) => Value::Float(*x),
        PyValue::Str(s) => Value::py_str(s),
        PyValue::Bytes(b) => Value::Bytes(b.to_vec()),
        PyValue::Tuple(t) => Value::Tuple(t.iter().map(to_value).collect()),
        PyValue::List(t) => Value::List(t.iter().map(to_value).collect()),
        PyValue::Dict(d) => obj_value(d),
        PyValue::Pdf(o) => Value::from(o),
    }
}

/// The `dict` value of an object.
pub fn obj_value(o: &Obj) -> Value {
    Value::Dict(o.items().iter().map(|(k, v)| (Value::py_str(k), to_value(v))).collect())
}

/// A pdfplumber value from a value: `str`-keyed dicts become [`Obj`]s,
/// PDF names, keywords, references and streams become PDF objects.
pub fn from_value(v: &Value) -> Result<PyValue, String> {
    Ok(match v {
        Value::None => PyValue::None,
        Value::Bool(b) => PyValue::Bool(*b),
        Value::Int(i) => PyValue::Int(i.clone()),
        Value::Float(x) => PyValue::float_with_identity(*x),
        Value::Str(s) => PyValue::Str(pycompat::PyStr::from(s)),
        Value::Bytes(b) => PyValue::Bytes(Rc::from(&b[..])),
        Value::Tuple(t) => PyValue::Tuple(t.iter().map(from_value).collect::<Result<_, _>>()?),
        Value::List(t) => PyValue::List(t.iter().map(from_value).collect::<Result<_, _>>()?),
        Value::Dict(items) => PyValue::Dict(obj_from_items(items)?),
        Value::Literal(_) | Value::Keyword(_) | Value::Ref(_) | Value::Stream(_) => PyValue::Pdf(pdf_object(v)?),
        Value::Exception { .. } | Value::Named { .. } => return Err(format!("not an object value: {v:?}")),
    })
}

/// An [`Obj`] from a `dict` value.
pub fn obj_from_value(v: &Value) -> Result<Obj, String> {
    match v {
        Value::Dict(items) => obj_from_items(items),
        other => Err(format!("not a dict: {other:?}")),
    }
}

fn obj_from_items(items: &[(Value, Value)]) -> Result<Obj, String> {
    let mut o = Obj::new();
    for (k, v) in items {
        let Value::Str(k) = k else { return Err(format!("a dict key that is not a str: {k:?}")) };
        o.set(&pycompat::PyStr::from(k), from_value(v)?);
    }
    Ok(o)
}

/// A PDF object from a value (references have no document).
pub fn pdf_object(v: &Value) -> Result<PDFObject, String> {
    Ok(match v {
        Value::None => PDFObject::Null,
        Value::Bool(b) => PDFObject::Bool(*b),
        Value::Int(i) => PDFObject::Int(i.clone()),
        Value::Float(x) => PDFObject::Float(*x),
        Value::Bytes(b) => PDFObject::Bytes(Rc::from(&b[..])),
        Value::Literal(NameValue::Str(s)) => PDFObject::Literal(LIT(s.as_str().ok_or("a literal with a surrogate")?)),
        Value::Literal(NameValue::Bytes(b)) => PDFObject::Literal(LIT_bytes(b)),
        Value::Keyword(k) => PDFObject::Keyword(KWD(k)),
        Value::Ref(objid) => PDFObject::Ref(PDFObjRef::new(None, objid.clone())),
        Value::List(items) => PDFObject::List(PDFList::new(items.iter().map(pdf_object).collect::<Result<_, _>>()?)),
        Value::Dict(items) => {
            let d = PDFDict::new();
            for (k, x) in items {
                d.insert(Rc::from(k.as_str().ok_or("a PDF dict key that is not a str")?), pdf_object(x)?);
            }
            PDFObject::Dict(d)
        }
        Value::Stream(s) => {
            let PDFObject::Dict(attrs) = pdf_object(&Value::Dict(s.attrs.clone()))? else { unreachable!() };
            let Value::Bytes(raw) = &s.raw else { return Err("a stream without raw data".into()) };
            let stream = PDFStream::new(attrs, Rc::from(&raw[..]), None);
            if let (Value::Int(objid), Value::Int(genno)) = (&s.objid, &s.genno) {
                stream.set_objid(objid.clone(), genno.clone());
            }
            PDFObject::Stream(Rc::new(stream))
        }
        other => return Err(format!("not a PDF object: {other:?}")),
    })
}
