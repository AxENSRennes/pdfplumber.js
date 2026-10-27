//! The canonical writer `canon(v)` of `docs/verification.md` ("Canonical
//! Trace Format"), shared with `oracle/canon.py`.
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
//! | `PDFObjRef` | `R` + canon(objid) |
//! | `PDFStream` | `Stream(objid, genno, attrs, raw)`, `raw` = canon(`rawdata`) |
//! | exception | `!` + class name + `(` + json(str(e)) + `)` |
//! | named object | class name + dict (`LTChar{...}`) |
//!
//! The separator is always `", "`; a one-element tuple is `(a)` (the
//! elements joined, without Python's trailing comma). In an exception
//! message, the address of every CPython default repr (` at 0x7f3a...>`)
//! is written ` at 0x?>`. Cross-tested against `oracle/canon.py` by
//! `tests/canon_unit.rs`.

use std::fmt::Write;

use pdfminer::pdftypes::PDFObject;
use pdfminer::psparser::LiteralName;
use pycompat::repr::{float_repr, write_json_code_points, write_json_str};
use pycompat::PyErr;
use sha2::{Digest, Sha256};

use crate::value::{NameValue, PyStr, Value};

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
    match &e.surrogate_message {
        None => write_exc(&mut out, &e.kind.trace_name(), ExcMessage::Text(&e.message)),
        Some(_) => {
            let message: Vec<u32> = e.py_message().code_points().collect();
            write_exc(&mut out, &e.kind.trace_name(), ExcMessage::CodePoints(&message))
        }
    }
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

/// `json.dumps(s)` of a Python `str`.
pub fn write_py_str(out: &mut String, s: &PyStr) {
    match s {
        PyStr::Text(s) => write_json_str(out, s),
        PyStr::CodePoints(c) => write_json_code_points(out, c),
    }
}

const ADDRESS_PREFIX: [u32; 6] = [b' ' as u32, b'a' as u32, b't' as u32, b' ' as u32, b'0' as u32, b'x' as u32];

/// `re.sub(r" at 0x[0-9a-fA-F]+>", " at 0x?>", message)` (`oracle/canon.py`
/// `_normalize_message`): hides the memory address of CPython default
/// reprs, which changes from run to run.
fn normalize_addresses(message: &[u32]) -> Vec<u32> {
    let is_hex = |c: u32| char::from_u32(c).is_some_and(|ch| ch.is_ascii_hexdigit());
    let mut out = Vec::with_capacity(message.len());
    let mut i = 0;
    while i < message.len() {
        if message[i..].starts_with(&ADDRESS_PREFIX) {
            let digits = i + ADDRESS_PREFIX.len();
            let end = digits + message[digits..].iter().take_while(|&&c| is_hex(c)).count();
            if end > digits && message.get(end) == Some(&u32::from(b'>')) {
                out.extend(ADDRESS_PREFIX);
                out.extend([u32::from(b'?'), u32::from(b'>')]);
                i = end + 1;
                continue;
            }
        }
        out.push(message[i]);
        i += 1;
    }
    out
}

/// `str(e)`, borrowed.
enum ExcMessage<'a> {
    Text(&'a str),
    CodePoints(&'a [u32]),
}

fn write_exc(out: &mut String, class: &str, message: ExcMessage<'_>) {
    out.push('!');
    out.push_str(class);
    out.push('(');
    match message {
        ExcMessage::Text(m) if !m.contains(" at 0x") => write_json_str(out, m),
        ExcMessage::Text(m) => {
            write_json_code_points(out, &normalize_addresses(&m.chars().map(u32::from).collect::<Vec<_>>()))
        }
        ExcMessage::CodePoints(c) => write_json_code_points(out, &normalize_addresses(c)),
    }
    out.push(')');
}

/// A step of the canonical writers: a value to write, or fixed text.
enum Step<T> {
    Write(T),
    Text(&'static str),
    Owned(String),
}

/// Pushes the steps that write `open a, b, ... close` (in reverse: the
/// steps are a stack).
fn push_seq<T>(
    steps: &mut Vec<Step<T>>,
    open: &'static str,
    items: impl DoubleEndedIterator<Item = T>,
    close: &'static str,
) {
    steps.push(Step::Text(close));
    let mut items = items.rev().peekable();
    while let Some(x) = items.next() {
        steps.push(Step::Write(x));
        if items.peek().is_some() {
            steps.push(Step::Text(", "));
        }
    }
    steps.push(Step::Text(open));
}

/// Pushes the steps that write `{k: v, ...}`.
fn push_pairs<'a>(steps: &mut Vec<Step<&'a Value>>, items: &'a [(Value, Value)]) {
    steps.push(Step::Text("}"));
    for (i, (k, v)) in items.iter().enumerate().rev() {
        steps.extend([Step::Write(v), Step::Text(": "), Step::Write(k)]);
        if i > 0 {
            steps.push(Step::Text(", "));
        }
    }
    steps.push(Step::Text("{"));
}

/// Appends `canon(v)` to `out` (any depth: an explicit stack, like
/// `oracle/canon.py`).
pub fn write_canon(out: &mut String, v: &Value) {
    let mut steps = vec![Step::Write(v)];
    while let Some(step) = steps.pop() {
        let v = match step {
            Step::Text(t) => {
                out.push_str(t);
                continue;
            }
            Step::Owned(t) => {
                out.push_str(&t);
                continue;
            }
            Step::Write(v) => v,
        };
        match v {
            Value::None => out.push_str("None"),
            Value::Bool(b) => out.push_str(if *b { "True" } else { "False" }),
            Value::Int(i) => {
                let _ = write!(out, "{i}");
            }
            Value::Float(f) => out.push_str(&float_repr(*f)),
            Value::Str(s) => write_py_str(out, s),
            Value::Bytes(b) => write_bytes(out, b),
            Value::Tuple(items) => push_seq(&mut steps, "(", items.iter(), ")"),
            Value::List(items) => push_seq(&mut steps, "[", items.iter(), "]"),
            Value::Dict(items) => push_pairs(&mut steps, items),
            Value::Literal(name) => {
                out.push('/');
                match name {
                    NameValue::Str(s) => write_py_str(out, s),
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
                steps.extend([Step::Text(")"), Step::Write(&s.raw), Step::Text(", ")]);
                push_pairs(&mut steps, &s.attrs);
                steps.extend([Step::Text(", "), Step::Write(&s.genno), Step::Text(", "), Step::Write(&s.objid)]);
            }
            Value::Exception { class, message } => {
                let message = match message {
                    PyStr::Text(m) => ExcMessage::Text(m),
                    PyStr::CodePoints(c) => ExcMessage::CodePoints(c),
                };
                write_exc(out, class, message)
            }
            Value::Named { name, fields } => {
                out.push_str(name);
                push_pairs(&mut steps, fields);
            }
        }
    }
}

/// What [`write_canon_obj`] writes: an object, or a dict key (a `str`).
enum ObjItem {
    Obj(PDFObject),
    Key(std::rc::Rc<str>),
}

/// Pushes the steps that write a dict of `str` keys.
fn push_dict(steps: &mut Vec<Step<ObjItem>>, d: &pdfminer::pdftypes::PDFDict) {
    steps.push(Step::Text("}"));
    for (i, (k, v)) in d.items().into_iter().enumerate().rev() {
        steps.extend([Step::Write(ObjItem::Obj(v)), Step::Text(": "), Step::Write(ObjItem::Key(k))]);
        if i > 0 {
            steps.push(Step::Text(", "));
        }
    }
    steps.push(Step::Text("{"));
}

/// Appends `canon(obj)` to `out`; the same text as
/// `write_canon(out, &Value::from(obj))` (any depth: an explicit stack).
pub fn write_canon_obj(out: &mut String, obj: &PDFObject) {
    let mut steps = vec![Step::Write(ObjItem::Obj(obj.clone()))];
    while let Some(step) = steps.pop() {
        let obj = match step {
            Step::Text(t) => {
                out.push_str(t);
                continue;
            }
            Step::Owned(t) => {
                out.push_str(&t);
                continue;
            }
            Step::Write(ObjItem::Key(k)) => {
                write_json_str(out, &k);
                continue;
            }
            Step::Write(ObjItem::Obj(obj)) => obj,
        };
        match obj {
            PDFObject::Null => out.push_str("None"),
            PDFObject::Bool(b) => out.push_str(if b { "True" } else { "False" }),
            PDFObject::Int(i) => {
                let _ = write!(out, "{i}");
            }
            PDFObject::Float(f) => out.push_str(&float_repr(f)),
            PDFObject::Bytes(b) => write_bytes(out, &b),
            PDFObject::Str(s) => out.push_str(&s.json()),
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
            PDFObject::List(l) => push_seq(&mut steps, "[", l.to_vec().into_iter().map(ObjItem::Obj), "]"),
            PDFObject::Dict(d) => push_dict(&mut steps, &d),
            PDFObject::Stream(s) => {
                out.push_str("Stream(");
                // the id: an int, or any scalar id (`pdftypes::ObjId`)
                match s.objid() {
                    Some(id) => write_canon_obj(out, id.obj()),
                    None => out.push_str("None"),
                }
                let genno = s.genno().map_or_else(|| "None".to_owned(), |i| i.to_string());
                let _ = write!(out, ", {genno}, ");
                let raw = match s.rawdata() {
                    Some(raw) => {
                        let mut text = String::new();
                        write_bytes(&mut text, &raw);
                        text
                    }
                    None => "None".to_owned(),
                };
                steps.extend([Step::Text(")"), Step::Owned(raw), Step::Text(", ")]);
                push_dict(&mut steps, &s.attrs);
            }
            PDFObject::Ref(r) => {
                let _ = write!(out, "R{}", r.objid());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! The examples of the table in docs/verification.md ("Canonical Trace
    //! Format").
    use super::*;
    use pycompat::{ExcKind, PyInt};

    fn s(x: &str) -> Value {
        Value::str(x)
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
            data: Value::None,
        }));
        assert_eq!(canon(&stream), format!("Stream(5, 0, {{\"Length\": 42}}, x\"{}\")", hex(&raw)));
        let e = PyErr::new(ExcKind::PSEOF, "Unexpected EOF");
        assert_eq!(canon_exc(&e), "!PSEOF(\"Unexpected EOF\")");
        assert_eq!(canon(&Value::from(&e)), "!PSEOF(\"Unexpected EOF\")");
        // classes outside builtins, pdfminer and pdfplumber carry their module
        assert_eq!(canon_exc(&PyErr::new(ExcKind::ZlibError, "x")), "!zlib.error(\"x\")");
        let hex = PyErr::new(ExcKind::BinasciiError, "Non-hexadecimal digit found");
        assert_eq!(canon(&Value::from(&hex)), "!binascii.Error(\"Non-hexadecimal digit found\")");
        let addr = PyErr::new(ExcKind::ValueError, "<a.B object at 0x7f3aDEAD> at 0x>, at 0xg>");
        assert_eq!(canon_exc(&addr), "!ValueError(\"<a.B object at 0x?> at 0x>, at 0xg>\")");
        let lone = Value::Exception {
            class: "ValueError".into(),
            message: PyStr::from_code_points(vec![0xd800, 0x20, 0x61, 0x74, 0x20, 0x30, 0x78, 0x31, 0x3e]).unwrap(),
        };
        assert_eq!(canon(&lone), "!ValueError(\"\\ud800 at 0x?>\")");
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
