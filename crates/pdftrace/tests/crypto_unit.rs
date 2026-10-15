//! Replays the cases of `oracle/unit/crypto.py` (pdfminer results) through
//! `pdfminer::arcfour`, `pdfminer::saslprep` (and its UCD 3.2.0 NFKC),
//! `pdfminer::utils::unpad_aes` and `pdfminer::pdfdocument`'s security
//! handlers.
//!
//! References in a case resolve through a table document built from the
//! case (a missing object raises `PDFObjectNotFound`, so the reference is
//! `None`; an exception stored in the table is raised). Byte results longer
//! than `SHOWN_BYTES` are compared by SHA-256, as the generator writes them.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use pdfminer::arcfour::Arcfour;
use pdfminer::pdfdocument::{
    security_handler_registry, PDFStandardSecurityHandler, SecurityHandlerClass, PASSWORD_PADDING,
};
use pdfminer::pdftypes::{
    decipher_all, int_value, DecipherCallable, PDFDict, PDFList, PDFObjRef, PDFObject, PDFResolver, PDFStream,
};
use pdfminer::psparser::{LIT_bytes, KWD, LIT};
use pdfminer::saslprep::{saslprep, ucd_3_2_0_normalize_nfkc, StringprepTable};
use pdfminer::utils::unpad_aes;
use pdftrace::canon::{canon_obj, sha256_hex};
use pdftrace::unit::UnitData;
use pdftrace::value::{NameValue, PyStr, Value};
use pycompat::recursion;
use pycompat::{ExcKind, PyErr, PyInt, PyResult};

/// `SHOWN_BYTES` of the generator.
const SHOWN_BYTES: usize = 512;

/// A document whose objects come from the case.
#[derive(Default)]
struct TableDoc {
    table: RefCell<HashMap<PyInt, PyResult<PDFObject>>>,
}

impl PDFResolver for TableDoc {
    /// `TableDoc.getobj` of the generator: a Python method (one frame).
    fn getobj(&self, objid: &PyInt) -> PyResult<PDFObject> {
        let _frame = recursion::py_frame()?;
        match self.table.borrow().get(objid) {
            Some(r) => r.clone(),
            None => Err(PyErr::new(ExcKind::PDFObjectNotFound, objid.to_string())),
        }
    }
}

/// A PDF object from its typed value; references bound to `doc`.
fn build(v: &Value, doc: Option<&Weak<dyn PDFResolver>>) -> PDFObject {
    match v {
        Value::None => PDFObject::Null,
        Value::Bool(b) => PDFObject::Bool(*b),
        Value::Int(i) => PDFObject::Int(i.clone()),
        Value::Float(f) => PDFObject::Float(*f),
        Value::Bytes(b) => PDFObject::Bytes(Rc::from(&b[..])),
        Value::Literal(NameValue::Str(s)) => PDFObject::Literal(LIT(s.as_str().expect("a literal name"))),
        Value::Literal(NameValue::Bytes(b)) => PDFObject::Literal(LIT_bytes(b)),
        Value::Keyword(k) => PDFObject::Keyword(KWD(k)),
        Value::List(items) | Value::Tuple(items) => {
            PDFObject::List(PDFList::new(items.iter().map(|x| build(x, doc)).collect()))
        }
        Value::Dict(items) => PDFObject::Dict(dict(items, doc)),
        Value::Ref(objid) => PDFObject::Ref(PDFObjRef::new(doc.cloned(), objid.clone())),
        Value::Stream(sv) => {
            let Value::Bytes(raw) = &sv.raw else { panic!("a stream without rawdata: {sv:?}") };
            let s = PDFStream::new(dict(&sv.attrs, doc), Rc::from(&raw[..]), None);
            if let (Value::Int(objid), Value::Int(genno)) = (&sv.objid, &sv.genno) {
                s.set_objid(objid.clone(), genno.clone());
            }
            PDFObject::Stream(Rc::new(s))
        }
        other => panic!("not a PDF object: {other:?}"),
    }
}

fn dict(items: &[(Value, Value)], doc: Option<&Weak<dyn PDFResolver>>) -> PDFDict {
    PDFDict::from_pairs(items.iter().map(|(k, x)| (Rc::from(k.as_str().expect("a str key")), build(x, doc))))
}

fn exc_value(e: &PyErr) -> Value {
    Value::from(e)
}

fn bytes_value(b: &[u8]) -> Value {
    Value::Bytes(b.to_vec())
}

/// `shown(b)` of the generator.
fn shown(b: &[u8]) -> Value {
    if b.len() > SHOWN_BYTES {
        Value::List(vec![Value::str("sha256"), Value::str(sha256_hex(b))])
    } else {
        bytes_value(b)
    }
}

fn result<T>(r: PyResult<T>, f: impl FnOnce(T) -> Value) -> Value {
    match r {
        Ok(v) => f(v),
        Err(e) => exc_value(&e),
    }
}

fn str_arg(v: &Value) -> &str {
    v.as_str().unwrap_or_else(|| panic!("a str: {v:?}"))
}

fn bytes_arg(v: &Value) -> &[u8] {
    match v {
        Value::Bytes(b) => b,
        other => panic!("bytes: {other:?}"),
    }
}

fn int_arg(v: &Value) -> &PyInt {
    match v {
        Value::Int(i) => i,
        other => panic!("an int: {other:?}"),
    }
}

fn list_arg(v: &Value) -> &[Value] {
    match v {
        Value::List(items) | Value::Tuple(items) => items,
        other => panic!("a list: {other:?}"),
    }
}

fn replay(name: &str, mut f: impl FnMut(&[Value]) -> Value) -> usize {
    let cases = UnitData::open("crypto").cases(name);
    assert!(!cases.is_empty(), "{name}: no cases");
    let mut failures = Vec::new();
    for (args, expected) in &cases {
        let actual = f(args);
        if !actual.same(expected) {
            let shown_args = format!("{args:?}");
            let shown_args = if shown_args.len() > 2000 { format!("{}...", &shown_args[..2000]) } else { shown_args };
            failures.push(format!("{shown_args}\n    python: {expected:?}\n    rust:   {actual:?}"));
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

#[test]
fn arcfour() {
    replay("arcfour", |a| {
        let key = bytes_arg(&a[0]);
        let run = || -> PyResult<Vec<Value>> {
            let mut rc4 = Arcfour::new(key)?;
            list_arg(&a[1]).iter().map(|c| Ok(bytes_value(&rc4.process(bytes_arg(c))?))).collect()
        };
        result(run(), Value::List)
    });
}

#[test]
fn unpad() {
    replay("unpad", |a| result(unpad_aes(bytes_arg(&a[0])), bytes_value));
}

#[test]
fn password_padding() {
    // the constant is the bytes literal of upstream: also checked through
    // every authentication of the handler cases
    assert_eq!(PASSWORD_PADDING.len(), 32);
}

fn table_of(name: &str) -> StringprepTable {
    match name {
        "A1" => StringprepTable::A1,
        "B1" => StringprepTable::B1,
        "C12" => StringprepTable::C12,
        "C21_C22" => StringprepTable::C21C22,
        "C3" => StringprepTable::C3,
        "C4" => StringprepTable::C4,
        "C5" => StringprepTable::C5,
        "C6" => StringprepTable::C6,
        "C7" => StringprepTable::C7,
        "C8" => StringprepTable::C8,
        "C9" => StringprepTable::C9,
        "D1" => StringprepTable::D1,
        "D2" => StringprepTable::D2,
        other => panic!("unknown table {other}"),
    }
}

/// The generated tables are the ranges `stringprep` gives now, and
/// `contains` reads them right (every code point checked against the
/// ranges of the oracle).
#[test]
fn tables() {
    replay("tables", |a| {
        let table = table_of(str_arg(&a[0]));
        let ranges = table.ranges();
        // contains() on every scalar value agrees with the ranges
        for c in (0..0x110000u32).filter_map(char::from_u32) {
            let inside = ranges.iter().any(|&(lo, hi)| lo <= c as u32 && c as u32 <= hi);
            assert_eq!(table.contains(c), inside, "{table:?} U+{:04X}", c as u32);
        }
        Value::List(
            ranges
                .iter()
                .map(|&(lo, hi)| Value::Tuple(vec![Value::Int(PyInt::from(lo)), Value::Int(PyInt::from(hi))]))
                .collect(),
        )
    });
}

fn chars_of(v: &Value) -> Vec<char> {
    match v {
        Value::Str(s) => s.code_points().into_iter().map(|c| char::from_u32(c).expect("no surrogate")).collect(),
        other => panic!("a str: {other:?}"),
    }
}

fn str_value(chars: &[char]) -> Value {
    Value::Str(PyStr::Text(chars.iter().collect()))
}

/// `ucd_3_2_0.normalize("NFKC", s)`: every code point the oracle changes,
/// every other scalar value unchanged, and random strings.
#[test]
fn nfkc() {
    let mut changed: HashMap<u32, Value> = HashMap::new();
    let n = replay("nfkc", |a| match str_arg(&a[0]) {
        "cp" => {
            let c = u32::try_from(int_arg(&a[1]).as_i64().expect("small")).expect("a code point");
            let out = str_value(&ucd_3_2_0_normalize_nfkc(&[char::from_u32(c).expect("a scalar value")]));
            changed.insert(c, out.clone());
            out
        }
        "str" => str_value(&ucd_3_2_0_normalize_nfkc(&chars_of(&a[1]))),
        other => panic!("{other}"),
    });
    assert!(n > 10000 && changed.len() > 4000, "{n} cases, {} code points", changed.len());
    for ch in (0..0x110000u32).filter_map(char::from_u32) {
        if !changed.contains_key(&(ch as u32)) {
            assert_eq!(ucd_3_2_0_normalize_nfkc(&[ch]), vec![ch], "U+{:04X} is unchanged in Python", ch as u32);
        }
    }
}

#[test]
fn saslprep_cases() {
    replay("saslprep", |a| {
        let s: String = chars_of(&a[0]).into_iter().collect();
        let Value::Bool(flag) = a[1] else { panic!() };
        result(saslprep(&s, flag), |r| Value::Str(PyStr::Text(r)))
    });
}

// ---------------------------------------------------------------------------
// handlers

/// The table document of a case, and a weak reference for the objects.
fn table_doc(table: &Value) -> (Rc<TableDoc>, Weak<dyn PDFResolver>) {
    let doc = Rc::new(TableDoc::default());
    let dyn_doc: Rc<dyn PDFResolver> = doc.clone();
    let weak = Rc::downgrade(&dyn_doc);
    for pair in list_arg(table) {
        let pair = list_arg(pair);
        let entry = match &pair[1] {
            Value::Exception { class, message } => Err(PyErr::new(
                *ExcKind::ALL.iter().find(|k| k.trace_name() == *class).expect("a known class"),
                message.as_str().expect("a message"),
            )),
            v => Ok(build(v, Some(&weak))),
        };
        doc.table.borrow_mut().insert(int_arg(&pair[0]).clone(), entry);
    }
    (doc, weak)
}

fn opt_bytes(b: Option<Rc<[u8]>>) -> Value {
    b.map_or(Value::None, |b| bytes_value(&b))
}

fn obj_value(o: &PDFObject) -> Value {
    Value::from(o)
}

/// `describe(h)` of the generator.
fn describe(h: &PDFStandardSecurityHandler) -> Value {
    let mut d: Vec<(String, Value)> = vec![
        ("class".into(), Value::str(h.class.name())),
        ("v".into(), obj_value(&h.v)),
        ("r".into(), obj_value(&h.r)),
        ("p".into(), obj_value(&h.p)),
        ("o".into(), bytes_value(&h.o)),
        ("u".into(), bytes_value(&h.u)),
        ("length".into(), obj_value(&h.length)),
        ("key".into(), opt_bytes(h.key.clone())),
        ("printable".into(), result(h.is_printable(), Value::Bool)),
        ("modifiable".into(), result(h.is_modifiable(), Value::Bool)),
        ("extractable".into(), result(h.is_extractable(), Value::Bool)),
    ];
    if h.cf.is_some() {
        d.push(("stmf".into(), Value::str(h.stmf.clone().expect("V4"))));
        d.push(("strf".into(), Value::str(h.strf.clone().expect("V4"))));
        d.push(("encrypt_metadata".into(), Value::Bool(h.encrypt_metadata.expect("V4"))));
        d.push((
            "cfm".into(),
            Value::List(
                h.cfm.iter().map(|(k, f)| Value::List(vec![Value::str(k.clone()), Value::str(f.name())])).collect(),
            ),
        ));
    }
    if h.oe.is_some() {
        for (name, v) in [
            ("oe", &h.oe),
            ("ue", &h.ue),
            ("o_hash", &h.o_hash),
            ("o_validation_salt", &h.o_validation_salt),
            ("o_key_salt", &h.o_key_salt),
            ("u_hash", &h.u_hash),
            ("u_validation_salt", &h.u_validation_salt),
            ("u_key_salt", &h.u_key_salt),
        ] {
            d.push((name.into(), opt_bytes(v.clone())));
        }
    }
    Value::str_dict(d)
}

/// `run_handler_case(docid, param, table, passwords, items, method_args)` of
/// the generator.
fn run_handler_case(
    docid: &Value,
    param: &Value,
    table: &Value,
    passwords: &Value,
    items: &Value,
    methods: &Value,
) -> Value {
    let (_doc, weak) = table_doc(table);
    let param_obj = |weak: &Weak<dyn PDFResolver>| match build(param, Some(weak)) {
        PDFObject::Dict(d) => d,
        other => panic!("param is not a dict: {}", other.type_name()),
    };
    let docid_objs =
        |weak: &Weak<dyn PDFResolver>| list_arg(docid).iter().map(|x| build(x, Some(weak))).collect::<Vec<_>>();
    // factory_of(param): PDFDocument._initialize_password's choice
    let v = int_value(&param_obj(&weak).get("V").unwrap_or(PDFObject::Int(PyInt::ZERO))).expect("int_value(V)");
    let factory = security_handler_registry(&v);
    let mut out = Vec::new();
    let mut handler: Option<PDFStandardSecurityHandler> = None;
    for pw in list_arg(passwords) {
        let Some(class) = factory else {
            out.push(Value::None);
            continue;
        };
        match PDFStandardSecurityHandler::new(class, docid_objs(&weak), param_obj(&weak), str_arg(pw)) {
            Err(e) => out.push(exc_value(&e)),
            Ok(h) => {
                out.push(describe(&h));
                if handler.is_none() {
                    handler = Some(h);
                }
            }
        }
    }
    let Some(h) = handler else {
        return Value::List(vec![Value::List(out), Value::None]);
    };
    let auth = |r: PyResult<Option<Rc<[u8]>>>| result(r, opt_bytes);
    let extra: Vec<Value> = list_arg(methods)
        .iter()
        .map(|m| {
            let m = list_arg(m);
            match str_arg(&m[0]) {
                "authenticate" => auth(h.authenticate(str_arg(&m[1]))),
                "user" => auth(h.authenticate_user_password(bytes_arg(&m[1]))),
                "owner" => auth(h.authenticate_owner_password(bytes_arg(&m[1]))),
                "compute_u" => result(h.compute_u(bytes_arg(&m[1])), |b| bytes_value(&b)),
                "key" => result(h.compute_encryption_key(bytes_arg(&m[1])), |b| bytes_value(&b)),
                other => panic!("unknown method {other}"),
            }
        })
        .collect();
    let decrypts: Vec<Value> = list_arg(items)
        .iter()
        .map(|item| {
            let item = list_arg(item);
            let (objid, genno, data) = (int_arg(&item[0]), int_arg(&item[1]), bytes_arg(&item[2]));
            let attrs = match &item[3] {
                Value::None => None,
                Value::Dict(items) => Some(dict(items, None)),
                other => panic!("attrs {other:?}"),
            };
            let r = match &item[4] {
                Value::None => h.decrypt(objid, genno, data, attrs.as_ref()),
                name => h.decrypt_v4(objid, genno, data, attrs.as_ref(), Some(str_arg(name))),
            };
            result(r, |b| shown(&b))
        })
        .collect();
    Value::List(vec![Value::List(out), Value::List(extra), Value::List(decrypts)])
}

#[test]
fn handlers() {
    replay("handlers", |a| run_handler_case(&a[0], &a[1], &a[2], &a[3], &a[4], &a[5]));
}

#[test]
fn corpus() {
    let n = replay("corpus", |a| {
        // a[0], a[1]: the file and PDFDocument's own outcome (for the record)
        run_handler_case(&a[2], &a[3], &a[4], &a[5], &a[6], &a[7])
    });
    assert!(n >= 40, "the corpus has {n} encrypted PDFs");
}

/// `decipher_all(decrypt, 1, 0, nested_list(n, leaf))` called at the depth
/// the generator measured: the same `RecursionError`, at the same depth.
/// `decrypt` is `handler.decrypt`, or a function of one frame calling it
/// with a stream dictionary (`attrs`).
#[test]
fn deep() {
    replay("deep", |a| {
        let (_doc, weak) = table_doc(&Value::List(Vec::new()));
        let PDFObject::Dict(param) = build(&a[2], Some(&weak)) else { panic!() };
        let docid = list_arg(&a[1]).iter().map(|x| build(x, Some(&weak))).collect();
        let v = int_value(&param.get("V").unwrap_or(PDFObject::Int(PyInt::ZERO))).expect("V");
        let class = security_handler_registry(&v).expect("a handler class");
        let h = Rc::new(PDFStandardSecurityHandler::new(class, docid, param, str_arg(&a[3])).expect("a handler"));
        let callback: DecipherCallable = match &a[4] {
            Value::None => h.decipher(),
            Value::Dict(items) => {
                let attrs = dict(items, None);
                Rc::new(move |objid: &PyInt, genno: &PyInt, data: &[u8], _attrs: Option<&PDFDict>| {
                    let _frame = recursion::py_frame()?;
                    h.decrypt(objid, genno, data, Some(&attrs))
                })
            }
            other => panic!("attrs {other:?}"),
        };
        let Value::Dict(depth) = &a[5] else { panic!() };
        let field = |name: &str| match depth.iter().find(|(k, _)| k.as_str() == Some(name)) {
            Some((_, Value::Int(i))) => u32::try_from(i.as_i64().expect("small")).expect("a depth"),
            other => panic!("{other:?}"),
        };
        let base = recursion::Depth { py: field("py"), c: recursion::C_RECURSION_LIMIT - field("c_left") };
        let n = int_arg(&a[6]).as_i64().expect("small");
        let mut x = PDFObject::Bytes(Rc::from(bytes_arg(&a[7])));
        for _ in 0..n {
            x = PDFObject::List(PDFList::new(vec![x]));
        }
        recursion::with_depth(base, || match decipher_all(&callback, &PyInt::Small(1), &PyInt::ZERO, &x) {
            Ok(obj) => Value::str(sha256_hex(canon_obj(&obj).as_bytes())),
            Err(e) => exc_value(&e),
        })
    });
}

/// The methods called on handlers whose attributes the case sets.
#[test]
fn methods() {
    replay("methods", |a| {
        let bytes_out = |r: PyResult<Vec<u8>>| result(r, |b| bytes_value(&b));
        match str_arg(&a[0]) {
            "password_hash" => {
                let mut h = PDFStandardSecurityHandler::bare(SecurityHandlerClass::PDFStandardSecurityHandlerV5);
                h.r = build(&a[1], None);
                let vector = match &a[4] {
                    Value::None => None,
                    v => Some(bytes_arg(v)),
                };
                bytes_out(h._password_hash(bytes_arg(&a[2]), bytes_arg(&a[3]), vector))
            }
            "bytes_mod_3" => {
                result(PDFStandardSecurityHandler::_bytes_mod_3(bytes_arg(&a[1])), |n| Value::Int(PyInt::from(n)))
            }
            "aes_cbc_encrypt" => {
                let mut h = PDFStandardSecurityHandler::bare(SecurityHandlerClass::PDFStandardSecurityHandlerV5);
                h.r = PDFObject::Int(PyInt::Small(6));
                bytes_out(h._aes_cbc_encrypt(bytes_arg(&a[1]), bytes_arg(&a[2]), bytes_arg(&a[3])))
            }
            "decrypt" => {
                let mut h = PDFStandardSecurityHandler::bare(SecurityHandlerClass::PDFStandardSecurityHandlerV5);
                h.key = Some(Rc::from(bytes_arg(&a[2])));
                let (objid, genno, data) = (int_arg(&a[3]), int_arg(&a[4]), bytes_arg(&a[5]));
                let r = match str_arg(&a[1]) {
                    "decrypt_rc4" => h.decrypt_rc4(objid, genno, data),
                    "decrypt_aes128" => h.decrypt_aes128(objid, genno, data),
                    "decrypt_aes256" => h.decrypt_aes256(objid, genno, data),
                    "decrypt_identity" => h.decrypt_identity(objid, genno, data),
                    other => panic!("{other}"),
                };
                result(r, |b| shown(&b))
            }
            "rc4" => {
                let mut h = PDFStandardSecurityHandler::bare(SecurityHandlerClass::PDFStandardSecurityHandlerV4);
                h.r = build(&a[1], None);
                h.length = build(&a[2], None);
                h.o = Rc::from(bytes_arg(&a[3]));
                h.u = Rc::from(bytes_arg(&a[4]));
                h.p = build(&a[5], None);
                h.docid = list_arg(&a[6]).iter().map(|x| build(x, None)).collect();
                let Value::Bool(encrypt_metadata) = a[7] else { panic!() };
                h.encrypt_metadata = Some(encrypt_metadata);
                let (pw, key) = (bytes_arg(&a[8]), bytes_arg(&a[9]));
                let auth = |r: PyResult<Option<Rc<[u8]>>>| result(r, opt_bytes);
                Value::List(vec![
                    bytes_out(h.compute_encryption_key(pw)),
                    bytes_out(h.compute_u(key)),
                    result(h.verify_encryption_key(key), Value::Bool),
                    auth(h.authenticate_user_password(pw)),
                    auth(h.authenticate_owner_password(pw)),
                ])
            }
            other => panic!("unknown method case {other}"),
        }
    });
}
