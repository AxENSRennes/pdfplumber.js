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
    uint_value, DecipherCallable, PDFDict, PDFList, PDFObjRef, PDFObject, PDFResolver, PDFStream,
};
use pdfminer::psparser::{keyword_name, literal_name, KeywordName, LIT_bytes, KWD, LIT};
use pdftrace::canon::{canon_obj, sha256_hex};
use pdftrace::unit::UnitData;
use pdftrace::value::{NameValue, Value};
use pycompat::recursion;
use pycompat::{exc, Num, PyInt, PyResult};

/// A document whose objects come from the case.
#[derive(Default)]
struct TableDoc {
    table: RefCell<HashMap<PyInt, PDFObject>>,
}

impl PDFResolver for TableDoc {
    /// `TableDoc.getobj` of the generator: a Python method (one frame).
    fn getobj(&self, objid: &PyInt) -> PyResult<PDFObject> {
        let _frame = recursion::py_frame()?;
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
        Value::Literal(NameValue::Str(s)) => {
            PDFObject::Literal(LIT(s.as_str().expect("a literal name without surrogates")))
        }
        Value::Literal(NameValue::Bytes(b)) => PDFObject::Literal(LIT_bytes(b)),
        Value::Keyword(k) => PDFObject::Keyword(KWD(k)),
        Value::List(items) => PDFObject::List(PDFList::new(items.iter().map(|x| build(x, doc)).collect())),
        Value::Dict(items) => PDFObject::Dict(PDFDict::from_pairs(items.iter().map(|(k, x)| {
            let k = k.as_str().unwrap_or_else(|| panic!("dict key {k:?}"));
            (Rc::from(k), build(x, doc))
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

/// A `str` result, or the exception.
fn text(r: PyResult<String>) -> Value {
    match r {
        Ok(s) => Value::str(s),
        Err(e) => Value::from(&e),
    }
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
            Out::V(text(x.repr())),
            Out::V(text(literal_name(&x))),
            match keyword_name(&x) {
                KeywordName::Name(s) => Out::V(Value::str(s)),
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
            Out::V(text(x.repr())),
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
        let s = Rc::new(PDFStream::new(attrs, Rc::from(&raw[..]), None));
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
            Value::str("none")
        };
        let outs = vec![
            Out::V(text(before)),
            Out::V(filters),
            Out::V(data),
            Out::V(text(s.repr())),
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

/// `test_decipher` of oracle/unit/pdftypes.py, with the frames it takes in
/// Python: its own, and the generator expression `bytes()` resumes from C.
fn test_decipher(objid: &PyInt, genno: &PyInt, data: &[u8], attrs: Option<&PDFDict>) -> PyResult<Vec<u8>> {
    let _frame = recursion::py_frame()?;
    // bytes(<genexpr>): the call of the type, then each step of the
    // generator runs from C
    {
        let _entry = recursion::eval_entry()?;
        let _frame = recursion::py_frame()?;
    }
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
        let s = Rc::new(PDFStream::new(attrs, Rc::from(&raw[..]), Some(callback.clone())));
        if let (Value::Int(objid), Value::Int(genno)) = (&a[2], &a[3]) {
            s.set_objid(objid.clone(), genno.clone());
        }
        let data = match s.get_data() {
            Ok(d) => Value::Bytes(d.to_vec()),
            Err(e) => Value::from(&e),
        };
        Value::List(vec![data, text(s.repr())])
    });
}

#[test]
fn refs_without_document() {
    replay("refs", |a| {
        let Value::Int(objid) = &a[0] else { panic!() };
        let r = PDFObjRef::new(None, objid.clone());
        let x = PDFObject::Ref(r.clone());
        finish(vec![
            Out::V(text(PDFObject::Ref(r.clone()).repr())),
            obj(r.resolve(&PDFObject::Null)),
            obj(resolve1_none(&x)),
            obj(resolve_all(&PDFObject::List(PDFList::new(vec![x.clone()])), &PDFObject::Null)),
        ])
    });
}

/// An object of the `deep` cases from its generator spec (`build` of
/// oracle/unit/pdftypes.py: `["v", value]`, `["lit", name]`, `["kw", name]`,
/// `["ref", objid]`, `["list", [spec...]]`, `["dict", [[key, spec]...]]`,
/// `["stream", [[key, spec]...]]`).
fn spec_object(spec: &Value, doc: &Weak<dyn PDFResolver>) -> PDFObject {
    let Value::List(spec) = spec else { panic!("{spec:?}") };
    let pairs = |v: &Value| -> PDFDict {
        let Value::List(items) = v else { panic!() };
        PDFDict::from_pairs(items.iter().map(|pair| {
            let Value::List(pair) = pair else { panic!() };
            (Rc::from(pair[0].as_str().expect("a str key")), spec_object(&pair[1], doc))
        }))
    };
    match spec[0].as_str().expect("a spec kind") {
        "v" => build(&spec[1], doc),
        "lit" => match &spec[1] {
            Value::Bytes(b) => PDFObject::Literal(LIT_bytes(b)),
            name => PDFObject::Literal(LIT(name.as_str().expect("a literal name"))),
        },
        "kw" => match &spec[1] {
            Value::Bytes(b) => PDFObject::Keyword(KWD(b)),
            other => panic!("{other:?}"),
        },
        "ref" => match &spec[1] {
            Value::Int(objid) => PDFObject::Ref(PDFObjRef::new(Some(doc.clone()), objid.clone())),
            other => panic!("{other:?}"),
        },
        "list" => {
            let Value::List(items) = &spec[1] else { panic!() };
            PDFObject::List(PDFList::new(items.iter().map(|x| spec_object(x, doc)).collect()))
        }
        "dict" => PDFObject::Dict(pairs(&spec[1])),
        "stream" => PDFObject::Stream(Rc::new(PDFStream::new(pairs(&spec[1]), Rc::from(&b"raw"[..]), None))),
        other => panic!("unknown spec {other}"),
    }
}

/// `deep_object(shape, n, leaf)` of the generator, built iteratively.
fn deep_object(spec: &Value, doc: &Weak<dyn PDFResolver>) -> PDFObject {
    let Value::List(spec) = spec else { panic!() };
    let (shape, Value::Int(n)) = (spec[1].as_str().expect("a shape"), &spec[2]) else { panic!() };
    let mut x = spec_object(&spec[3], doc);
    if shape == "cycle" {
        let l = PDFList::new(vec![x]);
        l.push(PDFObject::List(l.clone()));
        return PDFObject::List(l);
    }
    for i in 0..n.as_i64().expect("a depth") {
        x = if shape == "dict" || (shape == "mixed" && i % 2 == 1) {
            PDFObject::Dict(PDFDict::from_pairs([(Rc::from("K"), x)]))
        } else {
            PDFObject::List(PDFList::new(vec![x]))
        };
    }
    x
}

/// The SHA-256 of a text, or the exception.
fn text_sha(r: PyResult<String>) -> Value {
    match r {
        Ok(text) => Value::str(sha256_hex(text.as_bytes())),
        Err(e) => Value::from(&e),
    }
}

/// The SHA-256 of an object's `canon`, or the exception.
fn canon_sha(r: PyResult<PDFObject>) -> Value {
    match r {
        Ok(obj) => Value::str(sha256_hex(canon_obj(&obj).as_bytes())),
        Err(e) => Value::from(&e),
    }
}

/// `repr`, `literal_name`, `resolve_all` and `decipher_all` of objects
/// nested close to Python's recursion limits, from the depth the generator
/// called them at: the same `RecursionError`, at the same depth.
#[test]
fn deep() {
    let callback: DecipherCallable = Rc::new(test_decipher);
    replay("deep", |a| {
        let (doc, weak) = doc();
        let Value::Dict(depth) = &a[1] else { panic!() };
        let field = |name: &str| match depth.iter().find(|(k, _)| k.as_str() == Some(name)) {
            Some((_, Value::Int(i))) => u32::try_from(i.as_i64().expect("small")).expect("a depth"),
            other => panic!("{other:?}"),
        };
        let base = recursion::Depth { py: field("py"), c: recursion::C_RECURSION_LIMIT - field("c_left") };
        let Value::List(table) = &a[3] else { panic!() };
        for pair in table {
            let Value::List(pair) = pair else { panic!() };
            let Value::Int(objid) = &pair[0] else { panic!() };
            let o = spec_object(&pair[1], &weak);
            doc.table.borrow_mut().insert(objid.clone(), o);
        }
        let x = deep_object(&a[2], &weak);
        let one = PyInt::Small(1);
        // at_depth calls f(*args): a builtin function takes a unit, a Python
        // function is run inline (its frame is its own).
        recursion::with_depth(base, || match a[0].as_str().expect("an op") {
            "repr" => text_sha(recursion::c_call(" while calling a Python object").and_then(|_call| x.repr())),
            "literal_name" => text_sha(recursion::py_frame().and_then(|_frame| {
                // literal_name's own frame (psparser takes no frames yet)
                literal_name(&x)
            })),
            "resolve_all" => canon_sha(resolve_all(&x, &PDFObject::Null)),
            "decipher_all" => canon_sha(decipher_all(&callback, &one, &PyInt::ZERO, &x)),
            other => panic!("unknown op {other}"),
        })
    });
}

/// `uint_value(x, n_bits)`, directly or through a reference.
#[test]
fn uint() {
    replay("uint", |a| {
        let (doc, weak) = doc();
        let x = match &a[0] {
            Value::List(table) if matches!(table.first(), Some(Value::List(_))) => {
                let Value::List(pair) = &table[0] else { panic!() };
                let Value::Int(objid) = &pair[0] else { panic!() };
                doc.table.borrow_mut().insert(objid.clone(), spec_object(&pair[1], &weak));
                PDFObject::Ref(PDFObjRef::new(Some(weak.clone()), objid.clone()))
            }
            v => build(v, &weak),
        };
        let Value::Int(n_bits) = &a[1] else { panic!() };
        match uint_value(&x, n_bits.as_i64().expect("small") as u32) {
            Ok(o) => Value::from(&o),
            Err(e) => Value::from(&e),
        }
    });
}
