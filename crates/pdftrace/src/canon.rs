//! The canonical writer `canon(v)` of `docs/verification.md` ("Format
//! canonique"), shared with `oracle/canon.py`.
//!
//! | value | written as |
//! |---|---|
//! | `None`, `True`, `False` | as is |
//! | `int` | decimal |
//! | `float` | `repr()` (`1.0`, `1e-05`, `-0.0`, `inf`, `nan`) |
//! | `str` | `json.dumps(s, ensure_ascii=True)` |
//! | `bytes` (<= 256 bytes) | `x"` lowercase hex `"` |
//! | `bytes` (> 256 bytes) | `x#` length `:` first 16 hex digits of the SHA-256 |
//! | `tuple`, `list`, `dict` | `(a, b)`, `[a, b]`, `{k: v}` (insertion order) |
//! | `PSLiteral` | `/` + canon(name) |
//! | `PSKeyword` | `K` + canon(name bytes) |
//! | `PDFObjRef` | `R` + objid |
//! | `PDFStream` | `Stream(objid, genno, attrs, raw)` |
//! | exception | `!` + class name + `(` + json(str(e)) + `)` |
//! | named object | class name + dict (`LTChar{...}`) |
//!
//! The separator is always `", "`; a one-element tuple is `(a)` (the
//! elements joined, without Python's trailing comma).

use std::fmt::Write;

use pdfminer::pdftypes::{PDFObject, PDFStream};
use pdfminer::psparser::LiteralName;
use pycompat::repr::{float_repr, write_json_str};
use pycompat::PyErr;
use sha2::{Digest, Sha256};

use crate::value::{NameValue, Value};

/// Bytes longer than this are written as their length and hash.
pub const MAX_INLINE_BYTES: usize = 256;

/// `canon(v)`
pub fn canon(v: &Value) -> String {
    let mut out = String::new();
    write_canon(&mut out, v);
    out
}

/// `canon(obj)` for a PDF object, without building a [`Value`].
pub fn canon_obj(obj: &PDFObject) -> String {
    let mut out = String::new();
    write_canon_obj(&mut out, obj);
    out
}

/// The line of a raised exception: `!Class("message")`.
pub fn canon_exc(e: &PyErr) -> String {
    let mut out = String::new();
    write_exc(&mut out, e.kind.name(), &e.message);
    out
}

/// Lowercase hexadecimal.
pub fn hex(b: &[u8]) -> String {
    let mut out = String::with_capacity(b.len() * 2);
    for &c in b {
        let _ = write!(out, "{c:02x}");
    }
    out
}

/// Hexadecimal SHA-256.
pub fn sha256_hex(b: &[u8]) -> String {
    hex(&Sha256::digest(b))
}

pub fn write_bytes(out: &mut String, b: &[u8]) {
    if b.len() <= MAX_INLINE_BYTES {
        out.push_str("x\"");
        out.push_str(&hex(b));
        out.push('"');
    } else {
        let digest = Sha256::digest(b);
        let _ = write!(out, "x#{}:{}", b.len(), &hex(&digest)[..16]);
    }
}

fn write_exc(out: &mut String, class: &str, message: &str) {
    out.push('!');
    out.push_str(class);
    out.push('(');
    write_json_str(out, message);
    out.push(')');
}

fn write_seq<T>(out: &mut String, open: char, items: &[T], close: char, mut item: impl FnMut(&mut String, &T)) {
    out.push(open);
    for (i, x) in items.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        item(out, x);
    }
    out.push(close);
}

fn write_pairs(out: &mut String, items: &[(Value, Value)]) {
    write_seq(out, '{', items, '}', |out, (k, v)| {
        write_canon(out, k);
        out.push_str(": ");
        write_canon(out, v);
    });
}

/// Appends `canon(v)` to `out`.
pub fn write_canon(out: &mut String, v: &Value) {
    match v {
        Value::None => out.push_str("None"),
        Value::Bool(b) => out.push_str(if *b { "True" } else { "False" }),
        Value::Int(i) => {
            let _ = write!(out, "{i}");
        }
        Value::Float(f) => out.push_str(&float_repr(*f)),
        Value::Str(s) => write_json_str(out, s),
        Value::Bytes(b) => write_bytes(out, b),
        Value::Tuple(items) => write_seq(out, '(', items, ')', write_canon),
        Value::List(items) => write_seq(out, '[', items, ']', write_canon),
        Value::Dict(items) => write_pairs(out, items),
        Value::Literal(name) => {
            out.push('/');
            match name {
                NameValue::Str(s) => write_json_str(out, s),
                NameValue::Bytes(b) => write_bytes(out, b),
            }
        }
        Value::Keyword(name) => {
            out.push('K');
            write_bytes(out, name);
        }
        Value::Ref(objid) => {
            let _ = write!(out, "R{objid}");
        }
        Value::Stream(s) => {
            out.push_str("Stream(");
            write_canon(out, &s.objid);
            out.push_str(", ");
            write_canon(out, &s.genno);
            out.push_str(", ");
            write_pairs(out, &s.attrs);
            out.push_str(", ");
            write_canon(out, &s.raw);
            out.push(')');
        }
        Value::Exception { class, message } => write_exc(out, class, message),
        Value::Named { name, fields } => {
            out.push_str(name);
            write_pairs(out, fields);
        }
    }
}

fn write_stream(out: &mut String, s: &PDFStream) {
    out.push_str("Stream(");
    match s.objid() {
        Some(i) => {
            let _ = write!(out, "{i}");
        }
        None => out.push_str("None"),
    }
    out.push_str(", ");
    match s.genno() {
        Some(i) => {
            let _ = write!(out, "{i}");
        }
        None => out.push_str("None"),
    }
    out.push_str(", ");
    write_canon_obj(out, &PDFObject::Dict(s.attrs.clone()));
    out.push_str(", ");
    match s.rawdata() {
        Some(raw) => write_bytes(out, &raw),
        None => out.push_str("None"),
    }
    out.push(')');
}

/// Appends `canon(obj)` to `out`; the same text as
/// `write_canon(out, &Value::from(obj))`.
pub fn write_canon_obj(out: &mut String, obj: &PDFObject) {
    match obj {
        PDFObject::Null => out.push_str("None"),
        PDFObject::Bool(b) => out.push_str(if *b { "True" } else { "False" }),
        PDFObject::Int(i) => {
            let _ = write!(out, "{i}");
        }
        PDFObject::Float(f) => out.push_str(&float_repr(*f)),
        PDFObject::Bytes(b) => write_bytes(out, b),
        PDFObject::Literal(lit) => {
            out.push('/');
            match lit.name() {
                LiteralName::Str(s) => write_json_str(out, s),
                LiteralName::Bytes(b) => write_bytes(out, b),
            }
        }
        PDFObject::Keyword(kwd) => {
            out.push('K');
            write_bytes(out, kwd.name());
        }
        PDFObject::List(l) => write_seq(out, '[', &l.borrow(), ']', write_canon_obj),
        PDFObject::Dict(d) => {
            let d = d.borrow();
            out.push('{');
            for (i, (k, v)) in d.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_json_str(out, k);
                out.push_str(": ");
                write_canon_obj(out, v);
            }
            out.push('}');
        }
        PDFObject::Stream(s) => write_stream(out, s),
        PDFObject::Ref(r) => {
            let _ = write!(out, "R{}", r.objid());
        }
    }
}

#[cfg(test)]
mod tests {
    //! The examples of the table in docs/verification.md.
    use super::*;
    use pycompat::{ExcKind, PyInt};

    fn s(x: &str) -> Value {
        Value::Str(x.to_owned())
    }

    #[test]
    fn doc_table_examples() {
        assert_eq!(canon(&Value::None), "None");
        assert_eq!(canon(&Value::Int(PyInt::Small(-12))), "-12");
        assert_eq!(canon(&Value::Int(PyInt::Small(0))), "0");
        for (f, text) in [(1.0, "1.0"), (0.1, "0.1"), (1e-05, "1e-05"), (1e16, "1e+16"), (-0.0, "-0.0")] {
            assert_eq!(canon(&Value::Float(f)), text);
        }
        assert_eq!(canon(&Value::Float(f64::INFINITY)), "inf");
        assert_eq!(canon(&Value::Float(f64::NAN)), "nan");
        assert_eq!(canon(&s("café")), "\"caf\\u00e9\"");
        assert_eq!(canon(&Value::Bytes(b"\r\n".to_vec())), "x\"0d0a\"");
        let long = vec![0u8; 5120];
        let digest = sha256_hex(&long);
        assert_eq!(canon(&Value::Bytes(long)), format!("x#5120:{}", &digest[..16]));
        assert_eq!(canon(&Value::Tuple(vec![Value::Float(1.0), Value::Int(PyInt::Small(2))])), "(1.0, 2)");
        assert_eq!(canon(&Value::Tuple(vec![])), "()");
        assert_eq!(canon(&Value::List(vec![])), "[]");
        assert_eq!(
            canon(&Value::Dict(vec![(s("Type"), Value::Literal(NameValue::Str("Page".into())))])),
            "{\"Type\": /\"Page\"}"
        );
        assert_eq!(canon(&Value::Literal(NameValue::Str("F1".into()))), "/\"F1\"");
        assert_eq!(canon(&Value::Literal(NameValue::Bytes(vec![0xff]))), "/x\"ff\"");
        assert_eq!(canon(&Value::Keyword(b"BT".to_vec())), "Kx\"4254\"");
        assert_eq!(canon(&Value::Ref(PyInt::Small(12))), "R12");
        let raw = vec![7u8; 42];
        let stream = Value::Stream(Box::new(crate::value::StreamValue {
            objid: Value::Int(PyInt::Small(5)),
            genno: Value::Int(PyInt::Small(0)),
            attrs: vec![(s("Length"), Value::Int(PyInt::Small(42)))],
            raw: Value::Bytes(raw.clone()),
        }));
        assert_eq!(canon(&stream), format!("Stream(5, 0, {{\"Length\": 42}}, x\"{}\")", hex(&raw)));
        let e = PyErr::new(ExcKind::PSEOF, "Unexpected EOF");
        assert_eq!(canon_exc(&e), "!PSEOF(\"Unexpected EOF\")");
        assert_eq!(canon(&Value::from(&e)), "!PSEOF(\"Unexpected EOF\")");
        let named = Value::Named { name: "LTChar".into(), fields: vec![(s("x0"), Value::Float(1.5))] };
        assert_eq!(canon(&named), "LTChar{\"x0\": 1.5}");
    }

    #[test]
    fn pdf_objects_write_like_their_values() {
        use pdfminer::pdftypes::{PDFDict, PDFList};
        use pdfminer::psparser::{LIT_bytes, KWD, LIT};
        use std::rc::Rc;
        let d = PDFDict::new();
        d.insert("A".into(), PDFObject::Literal(LIT("x")));
        d.insert("B".into(), PDFObject::Literal(LIT_bytes(b"\xff")));
        let obj = PDFObject::List(PDFList::new(vec![
            PDFObject::Null,
            PDFObject::Bool(true),
            PDFObject::Int(PyInt::Small(3)),
            PDFObject::Float(2.5),
            PDFObject::Bytes(Rc::from(&b"ab"[..])),
            PDFObject::Keyword(KWD(b"Tj")),
            PDFObject::Dict(d.clone()),
            PDFObject::Stream(Rc::new(pdfminer::pdftypes::PDFStream::new(d, Rc::from(&b"q"[..]), None))),
        ]));
        assert_eq!(canon_obj(&obj), canon(&Value::from(&obj)));
    }
}
