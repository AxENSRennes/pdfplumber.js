//! The fonts of the unit cases of `oracle/unit/fonts.py`, rebuilt from their
//! dumps: `[spec, table, errors, cipher]`, a font dictionary, the objects of
//! the document it reaches (`[[objid, object], ...]`, streams with their raw
//! data), the object numbers whose loading raised (`[[objid, class, args,
//! str(e)], ...]`) and, for an encrypted document, `[docid, param,
//! password]` (else `None`). Shared by `tests/fonts_unit.rs` and
//! `tests/converter_unit.rs` (whose cases carry their fonts the same way).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use pdfminer::pdfdocument::{security_handler_registry, PDFStandardSecurityHandler};
use pdfminer::pdffont::{get_font, PDFFont};
use pdfminer::pdftypes::PDFDict;
use pdfminer::pdftypes::{int_value, DecipherCallable, PDFList, PDFObjRef, PDFObject, PDFResolver, PDFStream};
use pdfminer::psparser::{LIT_bytes, KWD, LIT};
use pycompat::recursion::{self, py_frame};
use pycompat::{exc, ExcKind, PyErr, PyInt, PyResult};

use crate::value::{NameValue, Value};

/// The generator's `DEEP_OBJIDS`, `DEEP_GETOBJ_CALLS`.
pub const DEEP_OBJIDS: i64 = 200_000;
pub const DEEP_GETOBJ_CALLS: u32 = 8;

/// The generator's `FontDoc`: a table of objects, and the exceptions of the
/// object numbers whose loading raised.
#[derive(Default)]
pub struct FontDoc {
    pub table: RefCell<HashMap<PyInt, PDFObject>>,
    pub errors: RefCell<HashMap<PyInt, PyErr>>,
}

impl PDFResolver for FontDoc {
    /// `FontDoc.getobj`: a Python method (one frame); from `DEEP_OBJIDS`
    /// up, `FontDoc._deep` takes `DEEP_GETOBJ_CALLS + 1` more and finds
    /// only table objects.
    fn getobj(&self, objid: &PyInt) -> PyResult<PDFObject> {
        let _frame = py_frame()?;
        if *objid >= PyInt::Small(DEEP_OBJIDS) {
            drop(recursion::py_frames(DEEP_GETOBJ_CALLS + 1)?);
            return self.table.borrow().get(objid).cloned().ok_or_else(|| exc::PDFObjectNotFound(objid.to_string()));
        }
        if let Some(obj) = self.table.borrow().get(objid) {
            return Ok(obj.clone());
        }
        if let Some(e) = self.errors.borrow().get(objid) {
            return Err(e.clone());
        }
        Err(exc::PDFObjectNotFound(objid.to_string()))
    }
}

/// What the objects of a case are bound to: the document their references
/// resolve through, and, for an encrypted document, the decryption callback
/// of its streams (the generator's `FontDoc.decipher`).
pub struct Binder {
    pub doc: Weak<dyn PDFResolver>,
    pub decipher: Option<DecipherCallable>,
}

fn int(v: &Value) -> PyInt {
    match v {
        Value::Int(i) => i.clone(),
        other => panic!("not an int: {other:?}"),
    }
}

fn text(v: &Value) -> &str {
    v.as_str().unwrap_or_else(|| panic!("not a str: {v:?}"))
}

fn list(v: &Value) -> &[Value] {
    match v {
        Value::List(items) | Value::Tuple(items) => items,
        other => panic!("not a list: {other:?}"),
    }
}

/// `security_handler(cipher)` of the generator: the handler of `[docid,
/// param, password]`, built as `PDFDocument._initialize_password` builds
/// it; its `decrypt`.
pub fn security_handler(cipher: &Value, doc: &Weak<dyn PDFResolver>) -> DecipherCallable {
    let c = list(cipher);
    let unbound = Binder { doc: doc.clone(), decipher: None };
    let docid = list(&c[0]).iter().map(|x| build(x, &unbound)).collect();
    let PDFObject::Dict(param) = build(&c[1], &unbound) else { panic!("a dict param") };
    let v = int_value(&param.get("V").unwrap_or(PDFObject::Int(PyInt::ZERO))).expect("int_value(V)");
    let class = security_handler_registry(&v).expect("a handler class");
    let h = PDFStandardSecurityHandler::new(class, docid, param, text(&c[2])).expect("a security handler");
    Rc::new(h).decipher()
}

/// A document of `table`, `errors` and `cipher` (the case's dumps; `cipher`
/// absent or `None` for a document that is not encrypted).
pub fn load_doc(table: &Value, errors: &Value, cipher: Option<&Value>) -> (Rc<FontDoc>, Binder) {
    let doc = Rc::new(FontDoc::default());
    let dyn_doc: Rc<dyn PDFResolver> = doc.clone();
    let weak = Rc::downgrade(&dyn_doc);
    let decipher = cipher.filter(|c| !matches!(c, Value::None)).map(|c| security_handler(c, &weak));
    let binder = Binder { doc: weak, decipher };
    for pair in list(table) {
        let p = list(pair);
        doc.table.borrow_mut().insert(int(&p[0]), build(&p[1], &binder));
    }
    for e in list(errors) {
        let e = list(e);
        let name = text(&e[1]);
        let kind = ExcKind::ALL.iter().copied().find(|k| k.trace_name() == name).unwrap_or_else(|| panic!("{name}"));
        doc.errors.borrow_mut().insert(int(&e[0]), PyErr::new(kind, text(&e[3])));
    }
    (doc, binder)
}

/// The PDF object of a typed value, its references bound to the document,
/// its streams with an object number given the document's decryption
/// callback (the generator's `bind`).
pub fn build(v: &Value, doc: &Binder) -> PDFObject {
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
        Value::Dict(items) => PDFObject::Dict(PDFDict::from_pairs(
            items.iter().map(|(k, x)| (Rc::from(k.as_str().expect("a str key")), build(x, doc))),
        )),
        Value::Ref(objid) => PDFObject::Ref(PDFObjRef::new(Some(doc.doc.clone()), objid.clone())),
        Value::Stream(sv) => {
            let PDFObject::Dict(attrs) = build(&Value::Dict(sv.attrs.clone()), doc) else { unreachable!() };
            let Value::Bytes(raw) = &sv.raw else { panic!("a stream not decoded yet: {sv:?}") };
            let decipher = doc.decipher.clone().filter(|_| !matches!(sv.objid, Value::None));
            let s = PDFStream::new(attrs, Rc::from(&raw[..]), decipher);
            if let (Value::Int(objid), Value::Int(genno)) = (&sv.objid, &sv.genno) {
                s.set_objid(objid.clone(), genno.clone());
            }
            PDFObject::Stream(Rc::new(s))
        }
        other => panic!("not a PDF object: {other:?}"),
    }
}

/// A font of a dump `[spec, table, errors, cipher]`, as
/// `PDFResourceManager().get_font(None, spec)` builds it (`pdffont::get_font`),
/// and the document its objects resolve through (to keep alive).
pub fn build_font(dump: &[Value]) -> (Rc<FontDoc>, PyResult<PDFFont>) {
    let (doc, binder) = load_doc(&dump[1], &dump[2], dump.get(3));
    let PDFObject::Dict(spec) = build(&dump[0], &binder) else { panic!("a dict spec") };
    let font = get_font(&spec);
    (doc, font)
}
