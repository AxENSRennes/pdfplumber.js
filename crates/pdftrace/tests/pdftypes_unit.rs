//! Replays the cases of `oracle/unit/pdftypes.py` (pdfminer results)
//! through `pdfminer::pdftypes` and `psparser::{literal_name, keyword_name}`.
//!
//! References resolve through a table document built from the case. As in
//! the generator, results are converted to values only after every call of
//! a case, so in-place updates (`resolve_all` on a dict) show through
//! earlier results the way they do in Python.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use pdfminer::pdftypes::{
    decipher_all, dict_value, float_value, int_value_obj, list_value, num_value, resolve1_none, resolve_all, str_value,
    DecipherCallable, PDFDict, PDFList, PDFObjRef, PDFObject, PDFResolver, PDFStream,
};
use pdfminer::psparser::{keyword_name, literal_name, KeywordName, LIT_bytes, KWD, LIT};
use pdftrace::unit::UnitData;
use pdftrace::value::{NameValue, Value};
use pycompat::{exc, Num, PyInt, PyResult};

/// A document whose objects come from the case.
#[derive(Default)]
struct TableDoc {
    table: RefCell<HashMap<PyInt, PDFObject>>,
}

impl PDFResolver for TableDoc {
    fn getobj(&self, objid: &PyInt) -> PyResult<PDFObject> {
        self.table.borrow().get(objid).cloned().ok_or_else(|| exc::PDFObjectNotFound(objid.to_string()))
    }
}

fn build(v: &Value, doc: &Weak<dyn PDFResolver>) -> PDFObject {
    match v {
        Value::None => PDFObject::Null,
        Value::Bool(b) => PDFObject::Bool(*b),
        Value::Int(i) => PDFObject::Int(i.clone()),
        Value::Float(f) => PDFObject::Float(*f),
        Value::Bytes(b) => PDFObject::Bytes(Rc::from(&b[..])),
        Value::Literal(NameValue::Str(s)) => PDFObject::Literal(LIT(s)),
        Value::Literal(NameValue::Bytes(b)) => PDFObject::Literal(LIT_bytes(b)),
        Value::Keyword(k) => PDFObject::Keyword(KWD(k)),
        Value::List(items) => PDFObject::List(PDFList::new(items.iter().map(|x| build(x, doc)).collect())),
        Value::Dict(items) => PDFObject::Dict(PDFDict::from_pairs(items.iter().map(|(k, x)| {
            let Value::Str(k) = k else { panic!("dict key {k:?}") };
            (Rc::from(k.as_str()), build(x, doc))
        }))),
        Value::Ref(objid) => PDFObject::Ref(PDFObjRef::new(Some(doc.clone()), objid.clone())),
        other => panic!("not a PDF object: {other:?}"),
    }
}

/// A result to convert once the case has run.
enum Out {
    Obj(PDFObject),
    V(Value),
}

fn obj(r: PyResult<PDFObject>) -> Out {
    match r {
        Ok(o) => Out::Obj(o),
        Err(e) => Out::V(Value::from(&e)),
    }
}

fn finish(outs: Vec<Out>) -> Value {
    Value::List(
        outs.into_iter()
            .map(|o| match o {
                Out::Obj(o) => Value::from(&o),
                Out::V(v) => v,
            })
            .collect(),
    )
}

fn num_obj(n: Num) -> PDFObject {
    match n {
        Num::Int(i) => PDFObject::Int(i),
        Num::Float(f) => PDFObject::Float(f),
    }
}

/// Exact comparison, with any NaN equal to any NaN.
fn matches(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Float(x), Value::Float(y)) => x.to_bits() == y.to_bits() || (x.is_nan() && y.is_nan()),
        (Value::List(x), Value::List(y)) | (Value::Tuple(x), Value::Tuple(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| matches(p, q))
        }
        (Value::Dict(x), Value::Dict(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| matches(&p.0, &q.0) && matches(&p.1, &q.1))
        }
        _ => a.same(b),
    }
}

fn replay(name: &str, mut f: impl FnMut(&[Value]) -> Value) {
    let cases = UnitData::open("pdftypes").cases(name);
    assert!(!cases.is_empty());
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
        failures.iter().take(10).cloned().collect::<Vec<_>>().join("\n")
    );
}

fn doc() -> (Rc<TableDoc>, Weak<dyn PDFResolver>) {
    let doc = Rc::new(TableDoc::default());
    let dyn_doc: Rc<dyn PDFResolver> = doc.clone();
    let weak = Rc::downgrade(&dyn_doc);
    (doc, weak)
}

#[test]
fn objects() {
    let (_doc, weak) = doc();
    replay("objects", |a| {
        let x = build(&a[0], &weak);
        let outs = vec![
            Out::V(Value::Str(x.repr())),
            Out::V(Value::Str(literal_name(&x))),
            match keyword_name(&x) {
                KeywordName::Name(s) => Out::V(Value::Str(s)),
                KeywordName::NotKeyword(o) => Out::Obj(o),
            },
            Out::V(Value::Bool(x.is_truthy())),
            obj(int_value_obj(&x)),
            obj(float_value(&x).map(PDFObject::Float)),
            // `Num` has no bool: Python's num_value(True) is True, here 1
            obj(num_value(&x).map(num_obj)),
            obj(str_value(&x).map(PDFObject::Bytes)),
            obj(list_value(&x).map(PDFObject::List)),
            obj(dict_value(&x).map(PDFObject::Dict)),
            Out::V(match x.py_contains_str("Predictor") {
                Ok(b) => Value::Bool(b),
                Err(e) => Value::from(&e),
            }),
            obj(x.py_getitem_index0()),
        ];
        let mut v = finish(outs);
        if let (Value::List(items), PDFObject::Bool(b)) = (&mut v, &x) {
            items[6] = Value::Bool(*b);
        }
        v
    });
}

#[test]
fn eq() {
    let (_doc, weak) = doc();
    replay("eq", |a| {
        let x = build(&a[0], &weak);
        let y = if matches!(&a[1], Value::Str(s) if s == "same") { x.clone() } else { build(&a[1], &weak) };
        Value::List(vec![Value::Bool(x == y), Value::Bool(y == x)])
    });
}

#[test]
fn resolve() {
    replay("resolve", |a| {
        let (doc, weak) = doc();
        let Value::List(table) = &a[0] else { panic!() };
        for pair in table {
            let Value::List(pair) = pair else { panic!() };
            let Value::Int(objid) = &pair[0] else { panic!() };
            let o = build(&pair[1], &weak);
            doc.table.borrow_mut().insert(objid.clone(), o);
        }
        let x = build(&a[1], &weak);
        let outs = vec![
            obj(resolve1_none(&x)),
            obj(resolve_all(&x, &PDFObject::Null)),
            obj(resolve1_none(&x)),
            obj(int_value_obj(&x)),
            obj(num_value(&x).map(num_obj)),
            obj(dict_value(&x).map(PDFObject::Dict)),
            obj(list_value(&x).map(PDFObject::List)),
            obj(str_value(&x).map(PDFObject::Bytes)),
            Out::V(Value::Str(x.repr())),
        ];
        let resolved_bool = match resolve1_none(&x) {
            Ok(PDFObject::Bool(b)) => Some(b),
            _ => None,
        };
        let mut v = finish(outs);
        if let (Value::List(items), Some(b)) = (&mut v, resolved_bool) {
            items[4] = Value::Bool(b);
        }
        v
    });
}

#[test]
fn streams() {
    let (_doc, weak) = doc();
    replay("streams", |a| {
        let PDFObject::Dict(attrs) = build(&a[0], &weak) else { panic!() };
        let Value::Bytes(raw) = &a[1] else { panic!() };
        let s = PDFStream::new(attrs, Rc::from(&raw[..]), None);
        if let Value::Int(objid) = &a[2] {
            s.set_objid(objid.clone(), PyInt::ZERO);
        }
        let before = s.repr();
        let filters = match s.get_filters() {
            Ok(pairs) => {
                Value::List(pairs.iter().map(|(f, p)| Value::Tuple(vec![Value::from(f), Value::from(p)])).collect())
            }
            Err(e) => Value::from(&e),
        };
        let data = match s.get_data() {
            Ok(d) => Value::Bytes(d.to_vec()),
            Err(e) => Value::from(&e),
        };
        let any = if s.contains("F") || s.contains("Filter") {
            Value::from(&s.get_any(&["F", "Filter"], PDFObject::Null))
        } else {
            Value::Str("none".into())
        };
        let outs = vec![
            Out::V(Value::Str(before)),
            Out::V(filters),
            Out::V(data),
            Out::V(Value::Str(s.repr())),
            Out::V(s.get_rawdata().map_or(Value::None, |r| Value::Bytes(r.to_vec()))),
            obj(s.getitem("Missing")),
            obj(s.getitem("Length")),
            Out::Obj(s.get("Length", &PDFObject::Int(PyInt::Small(-1)))),
            Out::V(any),
            Out::V(Value::Bool(s.contains("Length"))),
        ];
        finish(outs)
    });
}

/// `test_decipher` of oracle/unit/pdftypes.py.
fn test_decipher(objid: &PyInt, genno: &PyInt, data: &[u8], attrs: Option<&PDFDict>) -> PyResult<Vec<u8>> {
    let small = |i: &PyInt| i.as_i64().expect("small test ids");
    let n = attrs.map_or(100, |a| a.len() as i64);
    let key = ((small(objid) + 7 * small(genno) + n) & 0xff) as u8;
    let mut out: Vec<u8> = data.iter().rev().map(|b| b ^ key).collect();
    out.push((data.len() & 0xff) as u8);
    Ok(out)
}

#[test]
fn decipher() {
    let (_doc, weak) = doc();
    let callback: DecipherCallable = Rc::new(test_decipher);
    replay("decipher", |a| {
        if a.len() == 3 {
            // decipher_all(test_decipher, objid, genno, x), then x itself
            let x = build(&a[0], &weak);
            let (Value::Int(objid), Value::Int(genno)) = (&a[1], &a[2]) else { panic!() };
            let out = obj(decipher_all(&callback, objid, genno, &x));
            return finish(vec![out, Out::Obj(x)]);
        }
        let PDFObject::Dict(attrs) = build(&a[0], &weak) else { panic!() };
        let Value::Bytes(raw) = &a[1] else { panic!() };
        let s = PDFStream::new(attrs, Rc::from(&raw[..]), Some(callback.clone()));
        if let (Value::Int(objid), Value::Int(genno)) = (&a[2], &a[3]) {
            s.set_objid(objid.clone(), genno.clone());
        }
        let data = match s.get_data() {
            Ok(d) => Value::Bytes(d.to_vec()),
            Err(e) => Value::from(&e),
        };
        Value::List(vec![data, Value::Str(s.repr())])
    });
}

#[test]
fn refs_without_document() {
    replay("refs", |a| {
        let Value::Int(objid) = &a[0] else { panic!() };
        let r = PDFObjRef::new(None, objid.clone());
        let x = PDFObject::Ref(r.clone());
        finish(vec![
            Out::V(Value::Str(r.repr())),
            obj(r.resolve(&PDFObject::Null)),
            obj(resolve1_none(&x)),
            obj(resolve_all(&PDFObject::List(PDFList::new(vec![x.clone()])), &PDFObject::Null)),
        ])
    });
}
