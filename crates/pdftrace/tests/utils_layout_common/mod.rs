//! Helpers shared by `utils_unit.rs` and `layout_unit.rs`: typed values to
//! numbers and PDF objects, a table document for references, and the
//! replay loop.

#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use pdfminer::pdftypes::{PDFDict, PDFList, PDFObjRef, PDFObject, PDFResolver, PDFStream};
use pdfminer::psparser::{LIT_bytes, KWD, LIT};
use pdftrace::unit::UnitData;
use pdftrace::value::{NameValue, Value};
use pycompat::recursion;
use pycompat::{exc, Num, PyInt, PyResult};

/// The object number whose parsing fails (`BROKEN_OBJID` of the generators).
pub const BROKEN_OBJID: i64 = 13;

/// A document whose objects come from the case.
#[derive(Default)]
pub struct TableDoc {
    pub table: RefCell<HashMap<PyInt, PDFObject>>,
}

impl PDFResolver for TableDoc {
    /// `TableDoc.getobj` of the generators: a Python method (one frame).
    fn getobj(&self, objid: &PyInt) -> PyResult<PDFObject> {
        let _frame = recursion::py_frame()?;
        if let Some(obj) = self.table.borrow().get(objid) {
            return Ok(obj.clone());
        }
        if *objid == PyInt::Small(BROKEN_OBJID) {
            return Err(exc::PDFSyntaxError(format!("broken {objid}")));
        }
        Err(exc::PDFObjectNotFound(objid.to_string()))
    }
}

pub fn doc() -> (Rc<TableDoc>, Weak<dyn PDFResolver>) {
    let doc = Rc::new(TableDoc::default());
    let dyn_doc: Rc<dyn PDFResolver> = doc.clone();
    let weak = Rc::downgrade(&dyn_doc);
    (doc, weak)
}

/// The PDF object of a typed value, its references bound to `doc`.
pub fn build(v: &Value, doc: &Weak<dyn PDFResolver>) -> PDFObject {
    match v {
        Value::None => PDFObject::Null,
        Value::Bool(b) => PDFObject::Bool(*b),
        Value::Int(i) => PDFObject::Int(i.clone()),
        Value::Float(f) => PDFObject::Float(*f),
        Value::Bytes(b) => PDFObject::Bytes(Rc::from(&b[..])),
        Value::Literal(NameValue::Str(s)) => PDFObject::Literal(LIT(s.as_str().expect("a literal name"))),
        Value::Literal(NameValue::Bytes(b)) => PDFObject::Literal(LIT_bytes(b)),
        Value::Keyword(k) => PDFObject::Keyword(KWD(k)),
        Value::List(items) => PDFObject::List(PDFList::new(items.iter().map(|x| build(x, doc)).collect())),
        Value::Dict(items) => PDFObject::Dict(PDFDict::from_pairs(items.iter().map(|(k, x)| {
            let k = k.as_str().unwrap_or_else(|| panic!("dict key {k:?}"));
            (Rc::from(k), build(x, doc))
        }))),
        Value::Ref(objid) => PDFObject::Ref(PDFObjRef::new(Some(doc.clone()), objid.clone())),
        Value::Stream(sv) => {
            let PDFObject::Dict(attrs) = build(&Value::Dict(sv.attrs.clone()), doc) else { unreachable!() };
            let Value::Bytes(raw) = &sv.raw else { panic!("a decoded stream: {sv:?}") };
            let s = PDFStream::new(attrs, Rc::from(&raw[..]), None);
            if let (Value::Int(objid), Value::Int(genno)) = (&sv.objid, &sv.genno) {
                s.set_objid(objid.clone(), genno.clone());
            }
            PDFObject::Stream(Rc::new(s))
        }
        other => panic!("not a PDF object: {other:?}"),
    }
}

/// Fills the document's table from `[[objid, object], ...]`.
pub fn fill_table(doc: &TableDoc, weak: &Weak<dyn PDFResolver>, table: &Value) {
    let Value::List(table) = table else { panic!("a table: {table:?}") };
    for pair in table {
        let Value::List(pair) = pair else { panic!() };
        let Value::Int(objid) = &pair[0] else { panic!() };
        doc.table.borrow_mut().insert(objid.clone(), build(&pair[1], weak));
    }
}

/// A number argument (a bool is its int).
pub fn num(v: &Value) -> Num {
    match v {
        Value::Int(i) => Num::Int(i.clone()),
        Value::Float(f) => Num::Float(*f),
        Value::Bool(b) => Num::Int(PyInt::Small(i64::from(*b))),
        other => panic!("not a number: {other:?}"),
    }
}

/// The value of a number.
pub fn num_value(n: &Num) -> Value {
    match n {
        Num::Int(i) => Value::Int(i.clone()),
        Num::Float(f) => Value::Float(*f),
    }
}

/// The items of a tuple or list argument.
pub fn items(v: &Value) -> &[Value] {
    match v {
        Value::Tuple(x) | Value::List(x) => x,
        other => panic!("not a sequence: {other:?}"),
    }
}

/// A tuple of numbers.
pub fn nums<const N: usize>(v: &Value) -> [Num; N] {
    let x = items(v);
    assert_eq!(x.len(), N, "{v:?}");
    std::array::from_fn(|i| num(&x[i]))
}

pub fn tuple(ns: &[Num]) -> Value {
    Value::Tuple(ns.iter().map(num_value).collect())
}

pub fn float_tuple(fs: &[f64]) -> Value {
    Value::Tuple(fs.iter().map(|f| Value::Float(*f)).collect())
}

/// `Ok` as its value, `Err` as the exception.
pub fn result<T>(r: PyResult<T>, f: impl FnOnce(T) -> Value) -> Value {
    match r {
        Ok(x) => f(x),
        Err(e) => Value::from(&e),
    }
}

pub fn str_arg(v: &Value) -> &str {
    v.as_str().unwrap_or_else(|| panic!("not a str: {v:?}"))
}

pub fn int_arg(v: &Value) -> i64 {
    match v {
        Value::Int(i) => i.as_i64().expect("a small int"),
        Value::Bool(b) => i64::from(*b),
        other => panic!("not an int: {other:?}"),
    }
}

/// The `{"py": .., "c_left": ..}` depth of a case as a starting depth.
pub fn depth(v: &Value) -> recursion::Depth {
    let Value::Dict(depth) = v else { panic!("a depth: {v:?}") };
    let field = |name: &str| match depth.iter().find(|(k, _)| k.as_str() == Some(name)) {
        Some((_, Value::Int(i))) => u32::try_from(i.as_i64().expect("small")).expect("a depth"),
        other => panic!("{other:?}"),
    };
    recursion::Depth { py: field("py"), c: recursion::C_RECURSION_LIMIT - field("c_left") }
}

/// Exact comparison, with any NaN equal to any NaN.
pub fn matches(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Float(x), Value::Float(y)) => x.to_bits() == y.to_bits() || (x.is_nan() && y.is_nan()),
        (Value::List(x), Value::List(y)) | (Value::Tuple(x), Value::Tuple(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| matches(p, q))
        }
        (Value::Dict(x), Value::Dict(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| matches(&p.0, &q.0) && matches(&p.1, &q.1))
        }
        (Value::Named { name: n1, fields: f1 }, Value::Named { name: n2, fields: f2 }) => {
            n1 == n2 && f1.len() == f2.len() && f1.iter().zip(f2).all(|(p, q)| matches(&p.0, &q.0) && matches(&p.1, &q.1))
        }
        _ => a.same(b),
    }
}

/// Replays the cases of `<module>/<name>.jsonl`; reports the first ones
/// that differ.
pub fn replay(module: &str, name: &str, mut f: impl FnMut(&[Value]) -> Value) -> usize {
    let cases = UnitData::open(module).cases(name);
    assert!(!cases.is_empty(), "{module}/{name}: no case");
    let mut failures = Vec::new();
    for (args, expected) in &cases {
        let actual = f(args);
        if !matches(&actual, expected) {
            failures.push(format!("{args:?}\n    python: {expected:?}\n    rust:   {actual:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{name}: {}/{} cases differ; first ones:\n{}",
        failures.len(),
        cases.len(),
        failures.iter().take(5).cloned().collect::<Vec<_>>().join("\n")
    );
    cases.len()
}
