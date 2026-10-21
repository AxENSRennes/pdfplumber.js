//! The boundary itself: result encodings against what JavaScript must
//! decode (`common::Js`, the decoders of `js/src/core/wire.ts` in Rust),
//! argument decoding, the ABI, requests and responses, diagnostics,
//! handles. No Python semantics here (transport only); the Python-backed
//! cases are `js_runtime_unit.rs`.

mod common;

use std::rc::Rc;

use common::*;
use pdfminer::pdftypes::{PDFDict, PDFList, PDFObjRef, PDFObject, PDFStream};
use pdfminer::psparser::{LIT_bytes, KWD, LIT};
use pdfplumber::obj::{Obj, PyValue};
use pdfplumber_wasm::abi;
use pdfplumber_wasm::ops::{Core, MAX_DIAGNOSTICS};
use pdfplumber_wasm::strings::PyStrRef;
use pdfplumber_wasm::wire::binary::{self, column_kind, BinWriter};
use pdfplumber_wasm::wire::json::{self, JsonWriter};
use pdfplumber_wasm::wire::{shapes, write_value, Writer, MAX_SHAPES};
use pycompat::PyInt;

/// xorshift64*: a fixed-seed generator of values.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len() as u64) as usize]
    }
}

const FLOATS: &[f64] = &[
    0.0,
    -0.0,
    1.0,
    -1.5,
    0.1,
    1e-7,
    1e21,
    1e22,
    123456789012345680000.0,
    5e-324,
    f64::MAX,
    f64::MIN_POSITIVE,
    f64::NAN,
    f64::INFINITY,
    f64::NEG_INFINITY,
    9007199254740993.0,
    3.0,
];
const STRS: &[&str] =
    &["", "a", "x0", "fontname", "\0", "\0x", "__proto__", "é", "😀", "\"\\\n\t\u{1}\u{7f}\u{2028}", "toString", "1"];

fn int(r: &mut Rng) -> PyValue {
    let v: PyInt = match r.below(9) {
        0 => PyInt::from(0),
        1 => PyInt::from(i32::MAX),
        2 => PyInt::from(i32::MIN),
        3 => PyInt::from(i64::from(i32::MAX) + 1),
        4 => PyInt::from(1_i64 << 53),
        5 => PyInt::from(-(1_i64 << 53) - 1),
        6 => PyInt::from_decimal("-123456789012345678901234567890").unwrap(),
        7 => PyInt::from(r.next() as i64),
        _ => PyInt::from(r.below(1000) as i64 - 500),
    };
    PyValue::Int(v)
}

fn float(r: &mut Rng) -> f64 {
    if r.below(3) == 0 {
        f64::from_bits(r.next())
    } else {
        *r.pick(FLOATS)
    }
}

fn scalar(r: &mut Rng) -> PyValue {
    match r.below(7) {
        0 => PyValue::None,
        1 => PyValue::Bool(r.below(2) == 0),
        2 => int(r),
        3 | 4 => PyValue::float(float(r)),
        5 => PyValue::str(r.pick(STRS)),
        _ => PyValue::Bytes(Rc::from((0..r.below(5)).map(|_| r.next() as u8).collect::<Vec<_>>())),
    }
}

fn pdf_object(r: &mut Rng, depth: u32) -> PDFObject {
    match r.below(if depth > 2 { 5 } else { 9 }) {
        0 => PDFObject::Null,
        1 => PDFObject::Int(PyInt::from(r.below(100) as i64)),
        2 => PDFObject::Float(float(r)),
        3 => PDFObject::Literal(if r.below(2) == 0 { LIT("DeviceRGB") } else { LIT_bytes(&[0xff, 0x41]) }),
        4 => PDFObject::Keyword(KWD(b"BT")),
        5 => PDFObject::Ref(PDFObjRef::new(None, PyInt::from(r.below(50) as i64))),
        6 => PDFObject::List(PDFList::new((0..r.below(4)).map(|_| pdf_object(r, depth + 1)).collect())),
        7 => PDFObject::Dict(PDFDict::from_pairs(
            (0..r.below(4)).map(|i| (Rc::from(format!("K{i}").as_str()), pdf_object(r, depth + 1))),
        )),
        _ => {
            let attrs = PDFDict::from_pairs([(Rc::from("Length"), PDFObject::Int(PyInt::from(3)))]);
            let st = PDFStream::new(attrs, Rc::from(&b"abc"[..]), None);
            if r.below(2) == 0 {
                st.set_objid(PyInt::from(5), PyInt::from(0));
            }
            PDFObject::Stream(Rc::new(st))
        }
    }
}

/// A list of dicts in `nshapes` shapes, cells drawn from `cell`.
fn rows(r: &mut Rng, n: usize, nshapes: usize, cell: &mut dyn FnMut(&mut Rng, usize) -> PyValue) -> PyValue {
    let shapes: Vec<Vec<String>> =
        (0..nshapes).map(|s| (0..1 + (s + r.below(4) as usize) % 5).map(|k| format!("k{s}_{k}")).collect()).collect();
    let mut out = Vec::new();
    for i in 0..n {
        let keys = &shapes[if i < nshapes { i } else { r.below(nshapes as u64) as usize }];
        out.push(PyValue::Dict(Obj::from_pairs(keys.iter().enumerate().map(|(c, k)| (k.as_str(), cell(r, c))))));
    }
    PyValue::List(Rc::from(out))
}

/// A cell of a column whose kind depends on the column index.
fn column_cell(r: &mut Rng, c: usize) -> PyValue {
    match (c % 8, r.below(10)) {
        (0, _) => PyValue::float(float(r)),
        (1, 0) => PyValue::None,
        (1, _) => PyValue::float(float(r)),
        (2, _) => PyValue::str(r.pick(STRS)),
        (3, 0) => PyValue::None,
        (3, _) => PyValue::str(r.pick(STRS)),
        (4, _) => PyValue::Bool(r.below(2) == 0),
        (5, 0) => PyValue::None,
        (5, k) => PyValue::tuple((0..k).map(|_| PyValue::float(float(r)))),
        (6, _) => PyValue::None,
        _ => scalar(r),
    }
}

fn value(r: &mut Rng, depth: u32) -> PyValue {
    if depth > 4 {
        return scalar(r);
    }
    match r.below(12) {
        0..=3 => scalar(r),
        4 => PyValue::list((0..r.below(5)).map(|_| value(r, depth + 1))),
        5 => PyValue::tuple((0..r.below(4)).map(|_| value(r, depth + 1))),
        6 => PyValue::Dict(Obj::from_pairs(
            (0..r.below(5)).map(|_| (*r.pick(STRS), value(r, depth + 1))).collect::<Vec<_>>(),
        )),
        7 => PyValue::Pdf(pdf_object(r, 0)),
        8 => {
            let n = 2 + r.below(20) as usize;
            rows(r, n, 1, &mut column_cell)
        }
        9 => {
            let n = 4 + r.below(40) as usize;
            let k = 1 + r.below(n as u64 / 2) as usize;
            rows(r, n, k.min(MAX_SHAPES), &mut |r, _| value(r, depth + 2))
        }
        10 => {
            let n = 2 + r.below(6) as usize;
            rows(r, n, 1, &mut |r, _| value(r, depth + 2))
        }
        _ => {
            // a uniform column of a single kind
            let n = 2 + r.below(10) as usize;
            let kind = r.below(8) as usize;
            rows(r, n, 1, &mut move |r, _| column_cell(r, kind))
        }
    }
}

fn binary_view(v: &PyValue) -> Js {
    decode_block(&binary::encode(v).unwrap())
}

fn json_view(v: &PyValue) -> Js {
    let (text, fixup) = json::encode(v).unwrap();
    decode_json(&text, fixup)
}

#[test]
fn random_values_decode_to_what_javascript_must_see() {
    let mut r = Rng(0x9e37_79b9_7f4a_7c15);
    for i in 0..4000 {
        let v = value(&mut r, 0);
        let expected = js_view(&v);
        assert_eq!(binary_view(&v), expected, "binary, case {i}: {v:?}");
        assert_eq!(json_view(&v), expected, "JSON, case {i}: {v:?}");
    }
}

#[test]
fn tables_cover_every_column_type() {
    let mut r = Rng(7);
    let mut seen = [false; 8];
    for _ in 0..400 {
        let kind = r.below(8) as usize;
        let n = 2 + r.below(6) as usize;
        let v = rows(&mut r, n, 1, &mut |r, _| column_cell(r, kind));
        let cells: Vec<PyValue> = v
            .as_seq()
            .unwrap()
            .iter()
            .map(|d| match d {
                PyValue::Dict(d) => d.items()[0].1.clone(),
                _ => unreachable!(),
            })
            .collect();
        let refs: Vec<&PyValue> = cells.iter().collect();
        seen[usize::from(column_kind(&refs))] = true;
        assert_eq!(binary_view(&v), js_view(&v));
        // written as a table (the value right after the header), not as a list
        assert_eq!(binary::encode(&v).unwrap()[16], binary::TAG_TABLE);
    }
    assert!(seen.iter().all(|&s| s), "column kinds seen: {seen:?}");
    // the narrowest kind
    let k = |cells: &[PyValue]| column_kind(&cells.iter().collect::<Vec<_>>());
    let f = PyValue::float(1.0);
    let i = PyValue::Int(PyInt::from(1));
    let big = PyValue::Int(PyInt::from(1_i64 << 60));
    let n = PyValue::None;
    let t = PyValue::str("x");
    let b = PyValue::Bool(true);
    let seq = PyValue::tuple([f.clone(), i.clone()]);
    assert_eq!(k(&[n.clone(), n.clone()]), binary::COL_NONE);
    assert_eq!(k(&[b.clone(), b.clone()]), binary::COL_BOOL);
    assert_eq!(k(&[b.clone(), n.clone()]), binary::COL_GENERIC);
    assert_eq!(k(&[f.clone(), i.clone()]), binary::COL_F64);
    assert_eq!(k(&[f.clone(), big.clone()]), binary::COL_GENERIC);
    assert_eq!(k(&[n.clone(), i.clone()]), binary::COL_OPT_F64);
    assert_eq!(k(&[t.clone(), t.clone()]), binary::COL_STR);
    assert_eq!(k(&[n.clone(), t.clone()]), binary::COL_OPT_STR);
    assert_eq!(k(&[t.clone(), f.clone()]), binary::COL_GENERIC);
    assert_eq!(k(&[seq.clone(), n.clone()]), binary::COL_NUM_SEQ);
    assert_eq!(k(&[seq.clone(), seq.clone()]), binary::COL_NUM_SEQ);
    assert_eq!(k(&[seq.clone(), f.clone()]), binary::COL_GENERIC);
    assert_eq!(k(&[PyValue::tuple([big.clone()]), seq.clone()]), binary::COL_GENERIC);
    assert_eq!(k(&[PyValue::list([PyValue::str("a")])]), binary::COL_GENERIC);
    let long = PyValue::tuple((0..255).map(|_| f.clone()));
    let short = PyValue::tuple((0..254).map(|_| f.clone()));
    assert_eq!(k(&[long]), binary::COL_GENERIC);
    assert_eq!(k(std::slice::from_ref(&short)), binary::COL_NUM_SEQ);
    assert_eq!(k(&[b.clone(), f.clone()]), binary::COL_GENERIC);
    assert_eq!(k(&[b, t]), binary::COL_GENERIC);
}

#[test]
fn shapes_group_dicts_and_give_up_when_tables_save_nothing() {
    let d = |keys: &[&str]| PyValue::Dict(Obj::from_pairs(keys.iter().map(|k| (*k, PyValue::None))));
    let a = d(&["x", "y"]);
    let b = d(&["y", "x"]);
    let c = d(&["x"]);
    assert!(shapes(std::slice::from_ref(&a)).is_none(), "one row");
    assert!(shapes(&[a.clone(), PyValue::None]).is_none(), "not dicts");
    let v = [a.clone(), a.clone()];
    let s = shapes(&v).unwrap();
    assert_eq!((s.first.len(), s.order.clone()), (1, vec![0, 0]));
    let v = [a.clone(), b.clone(), a.clone(), b.clone()];
    let s = shapes(&v).unwrap();
    assert_eq!((s.first.len(), s.order.clone()), (2, vec![0, 1, 0, 1]));
    assert!(shapes(&[a.clone(), b.clone(), c.clone()]).is_none(), "more than one shape per two rows");
    let v = [a.clone(), b.clone(), c.clone(), a.clone(), b.clone(), c.clone()];
    assert_eq!(shapes(&v).unwrap().order, vec![0, 1, 2, 0, 1, 2]);
    // equal keys held by distinct strings (the key interning table of
    // pdfplumber::obj is full): still one shape
    let filler: Vec<String> = (0..5000).map(|i| format!("filler{i}")).collect();
    let _ = Obj::from_pairs(filler.iter().map(|k| (k.as_str(), PyValue::None)));
    let late = [d(&["late_key"]), d(&["late_key"])];
    assert_eq!(shapes(&late).unwrap().first.len(), 1);
    let empty = d(&[]);
    assert_eq!(shapes(&[empty.clone(), empty.clone()]).unwrap().first.len(), 1);
    // at most MAX_SHAPES shapes
    let many: Vec<PyValue> = (0..=MAX_SHAPES).flat_map(|i| [d(&[&format!("k{i}")]), d(&[&format!("k{i}")])]).collect();
    assert!(shapes(&many).is_none());
    assert!(shapes(&many[2..]).is_some());
    for v in [&many[2..], &[a.clone(), b.clone(), c.clone(), a.clone(), b, c][..]] {
        let v = PyValue::list(v.iter().cloned());
        assert_eq!(binary_view(&v), js_view(&v));
    }
}

#[test]
fn strings_with_surrogates_go_to_the_special_table() {
    let mut w = BinWriter::new();
    w.list(6);
    w.py_str(PyStrRef::Wtf8(&[0x61, 0xed, 0xa0, 0x80])).unwrap();
    w.py_str(PyStrRef::Utf16(&[0xdc00])).unwrap();
    w.py_str(PyStrRef::CodePoints(&[0xd83d, 0xde00])).unwrap(); // a pair of code points
    w.py_str(PyStrRef::Wtf8(&[0x61, 0xed, 0xa0, 0x80])).unwrap(); // interned once
    w.str("b");
    w.py_str(PyStrRef::Utf8("b")).unwrap();
    let block = w.finish();
    assert_eq!(
        decode_block(&block),
        Js::Arr(vec![
            Js::Str(vec![0x61, 0xd800]),
            Js::Str(vec![0xdc00]),
            Js::Str(vec![0xd83d, 0xde00]),
            Js::Str(vec![0x61, 0xd800]),
            s("b"),
            s("b"),
        ])
    );
    // 3 strings with 2 special: the pair of surrogates is valid UTF-16, plain text
    let table = u32::from_le_bytes(block[8..12].try_into().unwrap()) as usize;
    assert_eq!(&block[table..table + 8], &[4, 0, 0, 0, 2, 0, 0, 0]);
    assert!(w_err(|w| w.py_str(PyStrRef::Wtf8(&[0xc0, 0x80]))));
    assert!(w_err(|w| w.py_str(PyStrRef::CodePoints(&[0x110000]))));
    // JSON: the same strings as \u escapes
    let mut j = JsonWriter::new();
    j.dict(3);
    j.py_str(PyStrRef::Utf16(&[0, 0xd800])).unwrap();
    j.py_str(PyStrRef::Utf16(&[0xdfff, 0x22, 0x0a, 0xe9])).unwrap();
    j.py_str(PyStrRef::Utf16(&[0xd800])).unwrap();
    j.str("\0");
    j.py_str(PyStrRef::Utf16(&[0x61])).unwrap();
    j.none();
    let fix = j.needs_fixup();
    let text = j.finish();
    assert_eq!(
        String::from_utf8(text.clone()).unwrap(),
        "{\"\\u0000\\u0000\\ud800\":\"\\udfff\\u0022\\u000aé\",\"\\ud800\":\"\\u0000\",\"a\":null}"
    );
    assert!(fix);
    assert_eq!(
        decode_json(&text, fix),
        Js::Obj(vec![
            (vec![0, 0xd800], Js::Str(vec![0xdfff, 0x22, 0x0a, 0xe9])),
            (vec![0xd800], Js::Str(vec![0])),
            (vec![0x61], Js::Null),
        ])
    );
    assert!(json_err(|w| w.py_str(PyStrRef::CodePoints(&[0x110000]))));
}

fn w_err(f: impl Fn(&mut BinWriter) -> pycompat::PyResult<()>) -> bool {
    f(&mut BinWriter::new()).is_err()
}

fn json_err(f: impl Fn(&mut JsonWriter) -> pycompat::PyResult<()>) -> bool {
    f(&mut JsonWriter::new()).is_err()
}

#[test]
fn json_numbers_are_exact_and_flags_track_escapes() {
    for &x in FLOATS.iter().chain(&[1e-6, 9.999999e-7, 1e20, 0.3, -2.5e-300, 1.0 / 3.0]) {
        let (text, fixup) = json::encode(&PyValue::float(x)).unwrap();
        assert_eq!(decode_json(&text, fixup), Js::Num(x), "{x:e}");
        assert_eq!(fixup, !x.is_finite(), "{x}");
    }
    for v in [PyValue::None, PyValue::str("x"), PyValue::list([PyValue::from(1_i64)])] {
        assert!(!json::encode(&v).unwrap().1);
    }
    let escaped = PyValue::Dict(Obj::from_pairs([("\0", PyValue::None)]));
    assert_eq!(json::encode(&escaped).unwrap(), (b"{\"\\u0000\\u0000\":null}".to_vec(), true));
    // a str value that starts with U+0000 is not escaped (only keys are)
    assert_eq!(json::encode(&PyValue::str("\0")).unwrap(), (b"\"\\u0000\"".to_vec(), false));
    let big = PyValue::Int(PyInt::from((1_i64 << 53) + 1));
    assert_eq!(json::encode(&big).unwrap(), (b"{\"\\u0000\":[\"int\",\"9007199254740993\"]}".to_vec(), true));
    assert_eq!(json::encode(&PyValue::from(1_i64 << 53)).unwrap(), (b"9007199254740992".to_vec(), false));
    assert_eq!(json::encode(&PyValue::float(-0.0)).unwrap().0, b"-0");
    assert_eq!(json::encode(&PyValue::float(1e21)).unwrap().0, b"1e21");
    assert_eq!(json::encode(&PyValue::float(1e-7)).unwrap().0, b"1e-7");
    assert_eq!(json::encode(&PyValue::float(1e-6)).unwrap().0, b"0.000001");
    assert_eq!(json::encode(&PyValue::list([])).unwrap().0, b"[]");
    // short escapes for a quote, a backslash and a newline (the common ones), \u00XX for the other controls
    let text = PyValue::str("\"\\\n\t\u{1}é");
    assert_eq!(json::encode(&text).unwrap().0, "\"\\\"\\\\\\n\\u0009\\u0001é\"".as_bytes());
    assert_eq!(json::encode(&PyValue::Dict(Obj::new())).unwrap().0, b"{}");
}

#[test]
fn binary_integers_use_the_smallest_tag() {
    let tag = |v: i64| binary::encode(&PyValue::from(v)).unwrap()[16];
    assert_eq!(tag(0), binary::TAG_I32);
    assert_eq!(tag(i64::from(i32::MAX)), binary::TAG_I32);
    assert_eq!(tag(i64::from(i32::MIN)), binary::TAG_I32);
    assert_eq!(tag(i64::from(i32::MAX) + 1), binary::TAG_INT_F64);
    assert_eq!(tag(i64::from(i32::MIN) - 1), binary::TAG_INT_F64);
    assert_eq!(tag(1 << 53), binary::TAG_INT_F64);
    assert_eq!(tag(-(1 << 53)), binary::TAG_INT_F64);
    assert_eq!(tag((1 << 53) + 1), binary::TAG_BIGINT);
    assert_eq!(tag(-(1 << 53) - 1), binary::TAG_BIGINT);
}

#[test]
fn cyclic_pdf_values_raise_and_deep_values_do_not_recurse() {
    let list = PDFList::new(vec![PDFObject::Null]);
    list.push(PDFObject::List(list.clone()));
    let v = PyValue::Pdf(PDFObject::List(list.clone()));
    for e in [binary::encode(&v).unwrap_err(), json::encode(&v).unwrap_err()] {
        assert_eq!((e.kind, e.message.as_str()), (pycompat::ExcKind::ValueError, "Circular reference detected"));
    }
    // the same dict twice, not nested: no cycle
    let d = PDFObject::Dict(PDFDict::from_pairs([(Rc::from("A"), PDFObject::Null)]));
    let twice = PyValue::Pdf(PDFObject::List(PDFList::new(vec![d.clone(), d.clone()])));
    assert_eq!(binary_view(&twice), js_view(&twice));
    // 200 000 levels: no recursion in the writers or in argument decoding
    let mut deep = PyValue::None;
    for _ in 0..200_000 {
        deep = PyValue::list([deep]);
    }
    let block = binary::encode(&deep).unwrap();
    assert_eq!(block.len(), 16 + 2 * 200_000 + 1 + 3 + 12);
    let (text, _) = json::encode(&deep).unwrap();
    assert_eq!(text.len(), 2 * 200_000 + 4);
    let mut w = BinWriter::without_tables();
    write_value(&mut w, &deep).unwrap();
    let back = binary::decode(&w.finish()).unwrap();
    let mut depth = 0;
    let mut cur = &back;
    while let PyValue::List(items) = cur {
        depth += 1;
        cur = &items[0];
    }
    assert_eq!(depth, 200_000);
    // freed without recursion too (this thread's stack is 2 MiB natively,
    // 1 MiB under WebAssembly: a recursive drop overflows it)
    drop(back);
    // a request whose argument is nested 200 000 levels deep: decoded,
    // echoed, encoded and freed by the core without recursion
    let mut core = Core::new();
    let response = core.call(&request("echo", 0, &deep, 0), None);
    let payload_len = u32::from_le_bytes(response[12..16].try_into().unwrap()) as usize;
    assert_eq!((response[4], payload_len), (0, block.len()));
    drop(deep);
    assert_eq!(core.call(&request("info", 0, &PyValue::None, 0), None)[4], 0);
    // tuples and dicts, alternately, and a value shared with a live one
    let mut deep = PyValue::None;
    for i in 0..200_000 {
        deep = if i % 2 == 0 { PyValue::tuple([deep, PyValue::from(1.5)]) } else { obj_of("k", deep) };
    }
    let kept = PyValue::list([deep.clone()]);
    drop(deep);
    assert!(matches!(&kept, PyValue::List(items) if matches!(&items[0], PyValue::Dict(_))));
    drop(kept);
    list.set(1, PDFObject::Null); // breaks the cycle
}

fn obj_of(key: &str, v: PyValue) -> PyValue {
    PyValue::Dict(Obj::from_pairs([(key, v)]))
}

/// A `str` JavaScript can send: one of [`STRS`], or one holding lone
/// surrogates (JavaScript strings can).
fn arg_str(r: &mut Rng) -> pycompat::PyStr {
    match r.below(4) {
        0 => pycompat::PyStr::from_code_points([0xd800]).unwrap(),
        1 => pycompat::PyStr::from_code_points([0x61, 0xdc00, 0x1f600, 0xdbff]).unwrap(),
        _ => pycompat::PyStr::from(*r.pick(STRS)),
    }
}

/// A value JavaScript can send: plain Python values, lone surrogates
/// included.
fn arg_value(r: &mut Rng, depth: u32) -> PyValue {
    match r.below(if depth > 3 { 7 } else { 10 }) {
        0..=5 => scalar(r),
        6 => PyValue::Str(arg_str(r)),
        7 => PyValue::list((0..r.below(4)).map(|_| arg_value(r, depth + 1))),
        8 => PyValue::tuple((0..r.below(4)).map(|_| arg_value(r, depth + 1))),
        _ => PyValue::Dict(Obj::from_pairs(
            (0..r.below(4)).map(|_| (arg_str(r), arg_value(r, depth + 1))).collect::<Vec<_>>(),
        )),
    }
}

fn arg_block(v: &PyValue) -> Vec<u8> {
    let mut w = BinWriter::without_tables();
    write_value(&mut w, v).unwrap();
    w.finish()
}

#[test]
fn arguments_decode_to_the_same_python_values() {
    let mut r = Rng(42);
    for i in 0..5000 {
        let v = arg_value(&mut r, 0);
        let back = binary::decode(&arg_block(&v)).unwrap();
        // repr tells int from float, tuple from list, and -0.0 from 0.0
        assert_eq!(back.repr(), v.repr(), "case {i}");
    }
    // duplicate keys: the last value at the first position, as a Python dict display
    let mut w = BinWriter::without_tables();
    w.dict(3);
    for (k, v) in [("a", 1), ("b", 2), ("a", 3)] {
        w.str(k);
        w.int(&PyInt::from(v));
    }
    assert_eq!(binary::decode(&w.finish()).unwrap().repr(), "{'a': 3, 'b': 2}");
}

#[test]
fn malformed_arguments_raise_and_never_panic() {
    let mut r = Rng(3);
    let err = |b: &[u8]| binary::decode(b).unwrap_err();
    for _ in 0..300 {
        let v = arg_value(&mut r, 0);
        let block = arg_block(&v);
        for cut in 0..block.len() {
            let _ = binary::decode(&block[..cut]);
        }
        for _ in 0..20 {
            let mut b = block.clone();
            let i = r.below(b.len() as u64) as usize;
            b[i] ^= 1 << r.below(8);
            let _ = binary::decode(&b);
        }
    }
    let header = |extra: &[u8]| {
        let mut b = b"PJB1".to_vec();
        b.extend_from_slice(&[0; 12]);
        b.extend_from_slice(extra);
        b
    };
    let m = |b: &[u8]| err(b).message.clone();
    assert_eq!(m(b"PJB2\0\0\0\0\0\0\0\0\0\0\0\0\0"), "malformed PJB1 block: bad header");
    assert_eq!(m(b"PJB1"), "malformed PJB1 block: bad header");
    assert_eq!(m(&header(&[])), "malformed PJB1 block: truncated");
    assert_eq!(m(&header(&[0, 0])), "malformed PJB1 block: trailing bytes");
    assert_eq!(m(&header(&[0x09, 0x05, 0])), "malformed PJB1 block: container longer than its block");
    assert_eq!(err(&header(&[0x0b, 1, 0x03, 1, 0, 0, 0, 0])).kind, pycompat::ExcKind::TypeError);
    assert_eq!(err(&header(&[0x0c, 0])).kind, pycompat::ExcKind::TypeError, "no literal in arguments");
    assert_eq!(m(&header(&[0x07, 0])), "malformed PJB1 block: string index");
    // a lone surrogate in generalized UTF-8 (what JavaScript writes) is a Python str
    assert_eq!(binary::decode(&header(&[0x11, 3, 0xed, 0xa0, 0x80])).unwrap().repr(), "'\\ud800'");
    assert_eq!(m(&header(&[0x11, 2, 0xc0, 0x80])), "malformed PJB1 block: invalid generalized UTF-8 at byte 0");
    assert_eq!(m(&header(&[0x05, 2, b'1', b'x'])), "malformed PJB1 block: BigInt");
    let mut not_int = header(&[0x04]);
    not_int.extend_from_slice(&0.5f64.to_le_bytes());
    assert_eq!(m(&not_int), "malformed PJB1 block: IntF64 is not a safe integer");
    let mut too_big = header(&[0x04]);
    too_big.extend_from_slice(&((1u64 << 54) as f64).to_le_bytes());
    assert_eq!(m(&too_big), "malformed PJB1 block: IntF64 is not a safe integer");
    let mut ok = header(&[0x04]);
    ok.extend_from_slice(&(-((1u64 << 53) as f64)).to_le_bytes());
    assert_eq!(binary::decode(&ok).unwrap().repr(), "-9007199254740992");
    assert_eq!(m(&header(&[0x80])), "unsupported argument value (PJB1 tag 0x80)");
    assert_eq!(
        m(&header(&[0x09, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80])),
        "malformed PJB1 block: varint too long"
    );
    // a string table that does not hold together
    let mut bad_table = header(&[0x07, 0, 0, 0]);
    bad_table[8] = 20;
    bad_table.extend_from_slice(&[1, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(m(&bad_table), "malformed PJB1 block: string counts");
    let mut table = header(&[0x07, 0, 0, 0]);
    table[8] = 20;
    table.extend_from_slice(&[1, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, b'a', b'b', 0, 0, 2, 0, 0, 0]);
    assert_eq!(binary::decode(&table).unwrap().repr(), "'ab'");
    // one string more than the 4 bytes after the blob have offsets for:
    // refused before anything is allocated for them (a hostile count of
    // 2^32 would ask for 64 GiB)
    let mut over = table.clone();
    over[20] = 2;
    assert_eq!(m(&over), "malformed PJB1 block: string counts");
    let mut cut = table.clone();
    cut[36] = 3; // an end offset past the blob
    assert_eq!(m(&cut), "malformed PJB1 block: string offsets");
    let mut pair = header(&[0x07, 0, 0, 0]);
    pair[8] = 20;
    pair.extend_from_slice(&[1, 0, 0, 0, 0, 0, 0, 0, 4, 0, 0, 0, 0xf0, 0x9f, 0x98, 0x80, 1, 0, 0, 0]);
    assert_eq!(m(&pair), "malformed PJB1 block: string cut in a pair");
    let mut special = header(&[0x07, 0, 0, 0]);
    special[8] = 20;
    special.extend_from_slice(&[1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0xd8, 0, 0]);
    assert_eq!(binary::decode(&special).unwrap().repr(), "'\\ud800'", "a string of the table with a lone surrogate");
    let mut twice = special.clone();
    twice[20] = 2; // two strings, both claiming index 0
    twice[24] = 2;
    twice.extend_from_slice(&[0, 0, 0, 0, 1, 0, 0, 0, 0, 0xd8, 0, 0]);
    assert_eq!(m(&twice), "malformed PJB1 block: string index");
    let mut many = special.clone();
    many[24] = 0xff; // more strings with a surrogate than strings
    assert_eq!(m(&many), "malformed PJB1 block: string counts");
    // `k` strings with a surrogate, each at least 8 bytes (index, length):
    // one more than the 16 bytes after the empty blob can hold is refused
    // before anything is read or allocated for them (a hostile count of
    // 2^30 would ask for 8 GiB of slots)
    let specials = |k: u32| {
        let mut b = header(&[0x07, 0, 0, 0]);
        b[8] = 20;
        b.extend_from_slice(&k.to_le_bytes());
        b.extend_from_slice(&k.to_le_bytes());
        b.extend_from_slice(&[0; 4 + 16]);
        b
    };
    assert_eq!(m(&specials(3)), "malformed PJB1 block: string counts");
    assert_eq!(m(&specials(1 << 30)), "malformed PJB1 block: string counts");
    assert_eq!(m(&specials(2)), "malformed PJB1 block: string index", "two strings claiming index 0");
    let mut overlap = header(&[0x03, 1, 0, 0, 0]); // an I32 that runs into the table at 18
    overlap[8] = 18;
    overlap.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(m(&overlap), "malformed PJB1 block: value overlaps the string table");
}

#[test]
fn the_abi_hands_buffers_over_without_leaks() {
    assert_eq!(abi::pp_buffers(), 0);
    let p0 = abi::pp_alloc(0);
    let p1 = abi::pp_alloc(0);
    assert!(p0 != 0 && p1 != 0 && p0 != p1);
    assert_eq!(abi::pp_buffers(), 2);
    abi::pp_free(p0);
    abi::pp_free(p0);
    abi::pp_free(12345);
    assert_eq!(abi::pp_buffers(), 1);
    abi::pp_free(p1);
    let data = abi::put(b"PMCM\x01\x00");
    let req = abi::put(&request("cmap_configure", 0, &list(vec![PyValue::str("X")]), 0));
    assert_eq!(abi::pp_buffers(), 2);
    assert_eq!(abi::pp_call(req, 0), 0);
    assert_eq!(abi::pp_buffers(), 1, "the request is taken");
    let req = abi::put(&request("cmap_add", 0, &PyValue::str("X"), 0));
    assert_eq!(abi::pp_call(req, data), 0);
    assert_eq!(abi::pp_buffers(), 0, "the data is taken");
    let r = abi::last_response();
    assert_eq!(abi::pp_result_len(), r.len());
    assert_eq!((abi::pp_result_ptr(), abi::pp_result_len()), abi::last_response_place());
    assert_eq!(response(&r).value, Js::Null);
    let req = abi::put(&request("nope", 0, &PyValue::None, 0));
    assert_eq!(abi::pp_call(req, 0), 1);
    assert_eq!(response(&abi::last_response()).status, 1);
    assert_eq!(abi::pp_call(0, 0), 1, "no request: a malformed one");
    abi::pp_result_free();
    assert_eq!(abi::pp_result_len(), 0);
}

fn error_name(r: &Resp) -> (String, String) {
    let Js::Obj(items) = &r.value else { panic!("an error dict: {:?}", r.value) };
    let get = |k: &str| {
        items.iter().find(|(kk, _)| *kk == k.encode_utf16().collect::<Vec<_>>()).map(|(_, v)| v.clone()).unwrap()
    };
    let (Js::Str(n), Js::Str(m)) = (get("name"), get("message")) else { panic!("name and message") };
    (String::from_utf16(&n).unwrap(), String::from_utf16(&m).unwrap())
}

fn err(core: &mut Core, op: &str, h: u32, args: PyValue) -> (String, String) {
    let r = call(core, op, h, args, None);
    assert_eq!(r.status, 1, "{op}");
    error_name(&r)
}

#[test]
fn requests_report_their_errors() {
    let mut core = Core::new();
    let e = |core: &mut Core, op: &str, args: PyValue| err(core, op, 0, args);
    let pair = |a: &str, b: &str| (a.to_owned(), b.to_owned());
    assert_eq!(e(&mut core, "nope", PyValue::None), pair("ValueError", "unknown operation \"nope\""));
    assert_eq!(err(&mut core, "free", 5, PyValue::None), pair("ValueError", "invalid handle"));
    assert_eq!(e(&mut core, "doc_open", PyValue::None).0, "TypeError");
    assert_eq!(e(&mut core, "set_format", PyValue::str("xml")).1, "unknown result format 'xml'");
    assert_eq!(e(&mut core, "raise", list(vec![PyValue::str("NoSuchError"), PyValue::str("")])).0, "ValueError");
    assert_eq!(e(&mut core, "raise", list(vec![PyValue::str("zlib.error"), PyValue::str("m")])), pair("error", "m"));
    let q = "pdfminer.pdffont.TrueTypeFont.CMapNotFound";
    assert_eq!(e(&mut core, "raise", list(vec![PyValue::str(q), PyValue::str("")])).0, "CMapNotFound");
    assert_eq!(e(&mut core, "extract_text", PyValue::None).0, "TypeError");
    assert_eq!(e(&mut core, "extract_text", list(vec![])).1, "missing argument 0");
    assert_eq!(
        e(&mut core, "extract_text", list(vec![PyValue::None, PyValue::None])).1,
        "expected a list, got NoneType"
    );
    let chars_none = list(vec![list(vec![PyValue::None]), PyValue::None]);
    assert_eq!(e(&mut core, "extract_text", chars_none).1, "expected a dict, got NoneType");
    let bad_kwargs = list(vec![list(vec![]), PyValue::from(1_i64)]);
    assert_eq!(e(&mut core, "extract_text", bad_kwargs).1, "expected keyword arguments, got int");
    let lines = |h: PyValue| list(vec![h, PyValue::Bool(true), PyValue::Bool(true)]);
    assert_eq!(e(&mut core, "textmap_lines", lines(PyValue::from(99_i64))).1, "invalid handle");
    assert_eq!(e(&mut core, "textmap_lines", lines(PyValue::from(-1_i64))).1, "invalid handle");
    assert_eq!(e(&mut core, "textmap_lines", lines(PyValue::Int(PyInt::from(1_i64 << 40)))).1, "invalid handle");
    assert_eq!(
        e(&mut core, "textmap_lines", lines(PyValue::Int(PyInt::from_decimal("1".repeat(30).as_str()).unwrap()))).1,
        "int too large"
    );
    assert_eq!(e(&mut core, "textmap_lines", lines(PyValue::str("x"))).1, "expected an int, got str");
    assert_eq!(e(&mut core, "cmap_add", PyValue::str("A")).1, "cmap_add needs the file");
    assert_eq!(
        e(&mut core, "cmap_decode", list(vec![PyValue::str("Identity-H"), PyValue::str("x")])).1,
        "code must be bytes"
    );
    let unichr = list(vec![PyValue::str("Adobe-Japan1"), PyValue::Bool(false), list(vec![])]);
    assert_eq!(e(&mut core, "cmap_unichr", unichr).0, "CMapNotFound", "no CMap names configured");
    let warn = list(vec![list(vec![PyValue::str("s"), PyValue::from(31_i64), PyValue::str("m")])]);
    assert_eq!(e(&mut core, "warn", warn).1, "unknown level");
    assert_eq!(e(&mut core, "echo_wtf8", PyValue::str("x")).0, "TypeError");
    let be = list(vec![PyValue::str("binary"), PyValue::None]);
    assert_eq!(err(&mut core, "bench_encode", 7, be).1, "invalid handle");
    assert_eq!(err(&mut core, "doc_info", 3, PyValue::None).1, "invalid handle");
    assert_eq!(e(&mut core, "bench_load", PyValue::None).0, "TypeError");
    // a malformed request
    assert_eq!(error_name(&response(&core.call(b"PJB1", None))).0, "ValueError");
    assert_eq!(response(&core.call(&request("info", 0, &PyValue::None, 0)[..20], None)).status, 1);
    let no_op = arg_block(&list(vec![PyValue::from(1_i64), PyValue::None, PyValue::None, PyValue::from(0_i64)]));
    assert_eq!(error_name(&response(&core.call(&no_op, None))), pair("TypeError", "expected a str, got int"));
    let short = arg_block(&list(vec![PyValue::str("info"), PyValue::None, PyValue::None]));
    assert_eq!(error_name(&response(&core.call(&short, None))).1, "missing argument 3");
}

/// Runs `f` on a thread with a large stack (the test decoders recurse, and
/// `test_values` holds a list nested 100 000 levels deep).
fn big_stack(f: impl FnOnce() + Send + 'static) {
    on_thread(1 << 30, f);
}

/// `doc_cmap_decode` as `PDFCIDFont.get_cmap_from_spec` gets a CMap: an
/// empty `CMap()` for a name `CMapDB` has no file for, any other error
/// raised.
#[test]
fn doc_cmap_decode_falls_back_to_an_empty_cmap_on_cmap_not_found_only() {
    let mut core = Core::new();
    ok(call(&mut core, "cmap_configure", 0, list(vec![PyValue::str("X-Bad")]), None));
    let Js::Num(doc) = ok(call(&mut core, "doc_open", 0, PyValue::None, Some(tiny_pdf(b"")))) else { panic!() };
    let doc = PyValue::from(doc as i64);
    let decode = |core: &mut Core, doc: &PyValue, name: &str, code: PyValue| {
        call(core, "doc_cmap_decode", 0, list(vec![doc.clone(), PyValue::str(name), code]), None)
    };
    let code = PyValue::Bytes(Rc::from(&[1u8, 2][..]));
    let Js::Arr(r) = ok(decode(&mut core, &doc, "No-Such-CMap", code.clone())) else { panic!() };
    assert_eq!(r[0], Js::Arr(vec![]));
    // a compact file without CODE2CID: PyCMap's AttributeError goes through
    ok(call(&mut core, "cmap_add", 0, PyValue::str("X-Bad"), Some(b"PMCM\x01\x00".to_vec())));
    assert_eq!(error_name(&decode(&mut core, &doc, "X-Bad", code.clone())).0, "AttributeError");
    assert_eq!(error_name(&decode(&mut core, &doc, "No-Such-CMap", PyValue::None)).1, "code must be bytes");
    assert_eq!(error_name(&decode(&mut core, &PyValue::from(12345_i64), "No-Such-CMap", code)).1, "invalid handle");
}

/// Spans no `RegExp` gives (JavaScript computes them; a bug or a hostile
/// caller could send them): refused with an error, never a panic.
#[test]
fn search_spans_from_javascript_are_checked() {
    let mut core = Core::new();
    let ch = |i: usize, c: &str| {
        let f = |x: f64| PyValue::float(x);
        PyValue::Dict(Obj::from_pairs([
            ("text", PyValue::str(c)),
            ("x0", f(i as f64 * 10.0)),
            ("x1", f(i as f64 * 10.0 + 8.0)),
            ("top", f(0.0)),
            ("bottom", f(10.0)),
            ("doctop", f(0.0)),
            ("upright", PyValue::Bool(true)),
            ("height", f(10.0)),
            ("width", f(8.0)),
            ("size", f(10.0)),
        ]))
    };
    let chars = list(vec![ch(0, "a"), ch(1, "\u{1F600}"), ch(2, "b")]);
    let Js::Arr(pair) = ok(call(&mut core, "textmap", 0, list(vec![chars, PyValue::None]), None)) else { panic!() };
    let Js::Num(h) = pair[0] else { panic!() };
    let h = PyValue::from(h as i64);
    let n = |x: i64| PyValue::from(x);
    let span = |a: i64, b: i64| list(vec![n(a), n(b)]);
    let search = |core: &mut Core, spans: Vec<PyValue>, main: PyValue, named: PyValue| {
        let args =
            list(vec![h.clone(), list(vec![list(spans)]), PyValue::Bool(true), PyValue::Bool(false), main, named]);
        call(core, "textmap_search", 0, args, None)
    };
    let message = |r: &Resp| error_name(r).1;
    // UTF-16 offsets 3..4 are the code points 2..3: "b"
    let r = ok(search(&mut core, vec![span(3, 4)], n(0), PyValue::None));
    let Js::Arr(found) = r else { panic!() };
    let Js::Obj(found) = &found[0] else { panic!() };
    assert_eq!(found[0].1, s("b"));
    assert_eq!(
        message(&search(&mut core, vec![span(0, 2)], n(0), PyValue::None)),
        "match offset 2 splits a surrogate pair"
    );
    assert_eq!(message(&search(&mut core, vec![span(0, 5)], n(0), PyValue::None)), "match offset 5 outside the text");
    assert_eq!(message(&search(&mut core, vec![span(-1, 1)], n(0), PyValue::None)), "negative match offset");
    // offsets in the text, but a start after the end
    let backwards = search(&mut core, vec![span(4, 3)], n(0), PyValue::None);
    assert_eq!(message(&backwards), "match span outside TextMap.as_string");
    // named groups: one dict per match, spans that are groups' spans
    let named = |k: &str, v: PyValue| list(vec![PyValue::Dict(Obj::from_pairs([(k, v)]))]);
    let w = PyValue::str("w");
    let r = search(&mut core, vec![span(0, 4), span(3, 4)], w.clone(), named("w", span(3, 4)));
    assert_eq!(r.status, 0);
    let two = list(vec![PyValue::Dict(Obj::new()), PyValue::Dict(Obj::new())]);
    assert_eq!(message(&search(&mut core, vec![span(0, 4)], w.clone(), two)), "one dict of named groups per match");
    assert_eq!(
        message(&search(&mut core, vec![span(0, 4)], w.clone(), named("w", span(0, 1)))),
        "the span of group \"w\" is no group's span"
    );
    assert_eq!(
        message(&search(&mut core, vec![span(0, 4)], w.clone(), named("w", PyValue::None))),
        "the span of group \"w\" is no group's span"
    );
    assert_eq!(message(&search(&mut core, vec![span(0, 4)], w, named("v", PyValue::None))), "no such group");
}

/// The values `test_values` writes by hand, as JavaScript must receive them
/// (transport only, no Python semantics): the expectations of
/// `expectedTestValues` in `js/test/shared/tests.mjs`, here so that
/// mutation testing sees them.
#[test]
fn test_values_are_what_javascript_expects() {
    if cfg!(target_family = "wasm") {
        return; // the recursive test decoders cannot read "deep" on WebAssembly's stack
    }
    big_stack(|| {
        let units = |u: &[u16]| Js::Str(u.to_vec());
        let num = Js::Num;
        let row = |i: i64| {
            let f = i as f64;
            obj(vec![
                ("n", num(if i == 1 { 7.0 } else { f + 0.5 })),
                ("s", s(if i % 2 == 0 { "even" } else { "odd" })),
                ("os", if i == 1 { Js::Null } else { s("x") }),
                ("b", Js::Bool(i % 2 == 0)),
                ("none", Js::Null),
                ("seq", if i == 2 { Js::Null } else { Js::Arr(vec![num(f), num(1.0)]) }),
                ("on", if i == 0 { Js::Null } else { num(-0.0) }),
                ("g", if i == 0 { Js::Big("1152921504606846976".into()) } else { Js::Arr(vec![s("a")]) }),
                ("__proto__", num(f)),
            ])
        };
        let mut deep = Js::Null;
        for _ in 0..100_000 {
            deep = Js::Arr(vec![deep]);
        }
        let expected = obj(vec![
            ("nan", num(f64::NAN)),
            ("inf", num(f64::INFINITY)),
            ("-inf", num(f64::NEG_INFINITY)),
            ("-0.0", num(-0.0)),
            ("0.1", num(0.1)),
            ("5e-324", num(5e-324)),
            ("1.7976931348623157e308", num(f64::MAX)),
            ("1e21", num(1e21)),
            ("float 3.0", num(3.0)),
            ("int 3", num(3.0)),
            ("int -2147483649", num(-2_147_483_649.0)),
            ("int 2**53", num(9_007_199_254_740_992.0)),
            ("int 2**53+1", Js::Big("9007199254740993".into())),
            ("int big", Js::Big("-123456789012345678901234567890".into())),
            ("str wtf8 lone high", units(&[0x61, 0xd800, 0x62])),
            ("str wtf8 lone low", units(&[0xdc00])),
            ("str wtf8 split pair", units(&[0xd83d, 0xde00])),
            ("str utf16 lone", units(&[0xdfff, 0x41, 0xd800])),
            ("str code points", units(&[0xd83d, 0xde00, 0xdc00, 0xe9, 0])),
            ("str nul", s("\0x")),
            ("str escapes", s("\"\\\n\t\u{1}\u{7f}\u{2028}é😀")),
            ("bytes", Js::Bytes(vec![0, 255, 128])),
            (
                "dict keys",
                Js::Obj(vec![
                    ("__proto__".encode_utf16().collect(), num(1.0)),
                    (vec![0], num(2.0)),
                    (vec![0xd800], num(3.0)),
                    ("constructor".encode_utf16().collect(), Js::Null),
                ]),
            ),
            (
                "pdf objects",
                Js::Arr(vec![
                    Js::Lit(Box::new(s("DeviceRGB"))),
                    Js::Lit(Box::new(Js::Bytes(vec![0xff]))),
                    Js::Kw(b"BT".to_vec()),
                    Js::Ref(Box::new(num(12.0))),
                ]),
            ),
            (
                "stream",
                Js::Stream(vec![
                    num(5.0),
                    num(0.0),
                    obj(vec![("Length", num(3.0))]),
                    Js::Bytes(b"abc".to_vec()),
                    Js::Null,
                ]),
            ),
            ("deep", deep),
            ("table", Js::Arr((0..3).map(row).collect())),
        ]);
        let mut core = Core::new();
        for format in FORMATS {
            ok(call(&mut core, "set_format", 0, PyValue::str(format), None));
            assert_eq!(ok(call(&mut core, "test_values", 0, PyValue::None, None)), expected, "{format}");
        }
    });
}

#[test]
fn documents_handles_formats_and_the_benchmark() {
    big_stack(documents_handles_formats_and_the_benchmark_body);
}

fn documents_handles_formats_and_the_benchmark_body() {
    let mut core = Core::new();
    let opts = PyValue::Dict(Obj::from_pairs([("password", PyValue::str("pw"))]));
    let Js::Num(h) = ok(call(&mut core, "doc_open", 0, opts, Some(tiny_pdf(b"")))) else { panic!("a handle") };
    let h = h as u32;
    assert_ne!(h, 0);
    assert_eq!(
        ok(call(&mut core, "doc_info", h, PyValue::None, None)),
        obj(vec![("size", Js::Num(tiny_pdf(b"").len() as f64)), ("options", obj(vec![("password", s("pw"))]))])
    );
    let info = ok(call(&mut core, "info", 0, PyValue::None, None));
    let expected = obj(vec![
        ("version", s(env!("CARGO_PKG_VERSION"))),
        ("pdfminer", s("20260107")),
        ("pdfplumber", s("0.11.9")),
        ("handles", Js::Num(1.0)),
        ("history", Js::Num(1.0)),
    ]);
    assert_eq!(info, expected);
    assert_eq!(ok(call(&mut core, "free", h, PyValue::None, None)), Js::Null);
    assert_eq!(err(&mut core, "doc_info", h, PyValue::None).1, "invalid handle");
    assert_eq!(err(&mut core, "free", h, PyValue::None).1, "invalid handle");
    // a None handle is handle 0
    let req = arg_block(&list(vec![PyValue::str("echo"), PyValue::None, PyValue::from(5_i64), PyValue::from(0_i64)]));
    assert_eq!(ok(response(&core.call(&req, None))), Js::Num(5.0));
    // formats: the same values in JSON (the test values hold a list nested
    // 100 000 levels deep, which the recursive test decoders cannot read
    // with the small stack of WebAssembly)
    let deep_ok = !cfg!(target_family = "wasm");
    let tv = deep_ok.then(|| ok(call(&mut core, "test_values", 0, PyValue::None, None)));
    assert_eq!(ok(call(&mut core, "set_format", 0, PyValue::str("json"), None)), Js::Null);
    let r = core.call(&request("test_values", 0, &PyValue::None, 0), None);
    assert_eq!((r[5], r[6]), (1, 1), "JSON with fixups");
    if let Some(tv) = tv {
        assert_eq!(response(&r).value, tv);
    }
    let r = core.call(&request("echo", 0, &PyValue::from(1_i64), 0), None);
    assert_eq!((r[5], r[6]), (1, 0), "JSON without fixups");
    let r = core.call(&request("nope", 0, &PyValue::None, 0), None);
    assert_eq!((r[4], r[5]), (1, 0), "errors stay binary");
    assert_eq!(ok(call(&mut core, "set_format", 0, PyValue::str("binary"), None)), Js::Null);
    assert_eq!(core.call(&request("echo", 0, &PyValue::from(1_i64), 0), None)[5], 0);
    let wtf8 = PyValue::Bytes(Rc::from(&[0x61, 0xed, 0xa0, 0x80][..]));
    assert_eq!(ok(call(&mut core, "echo_wtf8", 0, wtf8, None)), Js::Str(vec![0x61, 0xd800]));
    // the benchmark: a stored list, by item or whole, in both formats
    let corpus = arg_block(&list(vec![PyValue::from(1_i64), PyValue::str("x")]));
    let Js::Num(b) = ok(call(&mut core, "bench_load", 0, PyValue::None, Some(corpus))) else { panic!() };
    let b = b as u32;
    for f in ["binary", "json"] {
        let item = ok(call(&mut core, "bench_encode", b, list(vec![PyValue::str(f), PyValue::from(1_i64)]), None));
        assert_eq!(item, s("x"));
        let whole = ok(call(&mut core, "bench_encode", b, list(vec![PyValue::str(f), PyValue::None]), None));
        assert_eq!(whole, Js::Arr(vec![Js::Num(1.0), s("x")]));
    }
    let past = list(vec![PyValue::str("binary"), PyValue::from(2_i64)]);
    assert_eq!(err(&mut core, "bench_encode", b, past).0, "IndexError");
    let negative = list(vec![PyValue::str("binary"), PyValue::from(-1_i64)]);
    assert_eq!(err(&mut core, "bench_encode", b, negative).0, "IndexError");
    let Js::Arr(kinds) = ok(call(&mut core, "exception_kinds", 0, PyValue::None, None)) else { panic!() };
    assert_eq!(kinds.len(), pycompat::ExcKind::ALL.len());
}

#[test]
fn diagnostics_are_collected_by_level_and_bounded() {
    let mut core = Core::new();
    let entry = |src: &str, lvl: i64, msg: &str| list(vec![PyValue::str(src), PyValue::from(lvl), PyValue::str(msg)]);
    let all = list(vec![
        entry("a", 10, "d"),
        entry("b", 20, "i"),
        entry("c", 30, "w"),
        entry("d", 40, "e"),
        entry("e", 50, "c"),
    ]);
    let diag = |entries: Vec<(&str, f64, &str)>, dropped: f64| {
        let entries = entries.into_iter().map(|(s0, l, m)| Js::Arr(vec![s(s0), Js::Num(l), s(m)])).collect();
        obj(vec![("entries", Js::Arr(entries)), ("dropped", Js::Num(dropped))])
    };
    let at = |core: &mut Core, level: i64| call_level(core, "warn", 0, all.clone(), None, level).diagnostics;
    assert_eq!(at(&mut core, 0), diag(vec![], 0.0));
    assert_eq!(at(&mut core, 7), diag(vec![], 0.0), "not a level: nothing");
    assert_eq!(at(&mut core, 30), diag(vec![("c", 30.0, "w"), ("d", 40.0, "e"), ("e", 50.0, "c")], 0.0));
    let everything = vec![("a", 10.0, "d"), ("b", 20.0, "i"), ("c", 30.0, "w"), ("d", 40.0, "e"), ("e", 50.0, "c")];
    assert_eq!(at(&mut core, 10), diag(everything, 0.0));
    assert_eq!(
        at(&mut core, 20).clone(),
        diag(vec![("b", 20.0, "i"), ("c", 30.0, "w"), ("d", 40.0, "e"), ("e", 50.0, "c")], 0.0)
    );
    assert_eq!(at(&mut core, 40), diag(vec![("d", 40.0, "e"), ("e", 50.0, "c")], 0.0));
    assert_eq!(at(&mut core, 50), diag(vec![("e", 50.0, "c")], 0.0));
    let many = list((0..MAX_DIAGNOSTICS + 3).map(|i| entry("x", 30, &i.to_string())).collect());
    let Js::Obj(d) = call_level(&mut core, "warn", 0, many, None, 30).diagnostics else { panic!() };
    let Js::Arr(entries) = &d[0].1 else { panic!() };
    assert_eq!((entries.len(), &d[1].1), (MAX_DIAGNOSTICS, &Js::Num(3.0)));
    // a failing call keeps its diagnostics
    let r = call_level(&mut core, "warn", 0, list(vec![entry("x", 30, "w"), PyValue::None]), None, 30);
    assert_eq!(r.status, 1);
    assert_eq!(r.diagnostics, diag(vec![("x", 30.0, "w")], 0.0));
}

#[test]
fn the_response_envelope_aligns_its_parts() {
    let c = pycompat::diagnostics::Collected::default();
    for n in 0..20 {
        let payload = vec![7u8; n];
        let r = pdfplumber_wasm::ops::response(2, 1, true, &payload, &c);
        assert_eq!(&r[..8], &[b'P', b'J', b'R', b'1', 2, 1, 1, 0]);
        let u = |i: usize| u32::from_le_bytes(r[i..i + 4].try_into().unwrap()) as usize;
        assert_eq!((u(8), u(12)), (24, n));
        assert_eq!(&r[24..24 + n], &payload[..]);
        assert_eq!(u(16), (24 + n).next_multiple_of(8));
        assert_eq!(u(16) + u(20), r.len());
        assert!(r[24 + n..u(16)].iter().all(|&b| b == 0));
        let r = pdfplumber_wasm::ops::response(0, 0, false, &payload, &c);
        assert_eq!(&r[4..8], &[0, 0, 0, 0]);
    }
}

/// Handles of pages and tables: a freed one is refused and its slot is
/// free (the replays make its object again, out of the table), freeing a
/// document frees its pages and tables, `free` cannot run in a `sequence`
/// (a void call would not undo it), and an operation reaches one document.
#[test]
fn page_and_table_handles_are_freed() {
    let mut core = Core::new();
    // a ruled 2 x 2 grid: one table
    let grid: &[u8] = b"10 10 m 110 10 l 10 60 m 110 60 l 10 110 m 110 110 l \
        10 10 m 10 110 l 60 10 m 60 110 l 110 10 m 110 110 l S";
    let Js::Num(doc) = ok(call(&mut core, "doc_open", 0, PyValue::None, Some(tiny_pdf(grid)))) else { panic!() };
    let Js::Num(other) = ok(call(&mut core, "doc_open", 0, PyValue::None, Some(tiny_pdf(b"")))) else { panic!() };
    let handle_of = |v: &Js| match v {
        Js::Obj(items) => match &items[0].1 {
            Js::Num(h) => *h as u32,
            x => panic!("a handle: {x:?}"),
        },
        x => panic!("a dict: {x:?}"),
    };
    let Js::Arr(pages) = ok(call(&mut core, "doc_pages", doc as u32, list(vec![]), None)) else { panic!() };
    let Js::Arr(others) = ok(call(&mut core, "doc_pages", other as u32, list(vec![]), None)) else { panic!() };
    let (page, other_page) = (handle_of(&pages[0]), handle_of(&others[0]));
    let Js::Arr(tables) = ok(call(&mut core, "page_find_tables", page, list(vec![]), None)) else { panic!() };
    assert_eq!(tables.len(), 1);
    let table = handle_of(&tables[0]);
    let rows = ok(call(&mut core, "table_extract", table, list(vec![PyValue::Dict(Obj::new())]), None));
    assert!(matches!(rows, Js::Arr(ref r) if r.len() == 2), "{rows:?}");
    // one operation, one document: a sequence may reach two
    let info = |h: u32| list(vec![PyValue::str("page_info"), PyValue::from(i64::from(h)), list(vec![])]);
    let Js::Arr(both) = ok(call(&mut core, "sequence", 0, list(vec![info(page), info(other_page)]), None)) else {
        panic!()
    };
    assert_eq!(both.len(), 2);
    let free_table = list(vec![PyValue::str("free"), PyValue::from(i64::from(table)), PyValue::None]);
    let pair = |a: &str, b: &str| (a.to_owned(), b.to_owned());
    assert_eq!(
        err(&mut core, "sequence", 0, list(vec![free_table])),
        pair("ValueError", "free cannot run in a sequence")
    );
    for h in [table, page] {
        assert_eq!(ok(call(&mut core, "free", h, PyValue::None, None)), Js::Null);
        assert_eq!(err(&mut core, "free", h, PyValue::None).1, "invalid handle");
    }
    assert_eq!(err(&mut core, "table_extract", table, list(vec![PyValue::Dict(Obj::new())])).1, "invalid handle");
    assert_eq!(err(&mut core, "page_info", page, list(vec![])).1, "invalid handle");
    let count = |core: &mut Core| {
        let Js::Obj(info) = ok(call(core, "info", 0, PyValue::None, None)) else { panic!() };
        info[3].1.clone()
    };
    // two documents and the other page: the freed handles are free
    assert_eq!(count(&mut core), Js::Num(3.0));
    // a crop made and freed again and again holds no handle (the table
    // has room for 2^20 - 1: it used to fill up with tombstones)
    let Js::Arr(pages) = ok(call(&mut core, "doc_pages", other as u32, list(vec![]), None)) else { panic!() };
    let page = handle_of(&pages[0]);
    let bbox = list(vec![0, 0, 50, 50].into_iter().map(PyValue::from).collect());
    let mut handles = std::collections::BTreeSet::new();
    for _ in 0..2000 {
        let crop = ok(call(&mut core, "page_crop", page, list(vec![bbox.clone()]), None));
        let crop = handle_of(&crop);
        handles.insert(crop & 0xf_ffff);
        assert_eq!(ok(call(&mut core, "free", crop, PyValue::None, None)), Js::Null);
    }
    assert_eq!(handles.len(), 1, "one slot reused");
    assert_eq!(count(&mut core), Js::Num(4.0));
    assert_eq!(ok(call(&mut core, "free", doc as u32, PyValue::None, None)), Js::Null);
    assert_eq!(count(&mut core), Js::Num(3.0));
    assert!(matches!(ok(call(&mut core, "page_info", other_page, list(vec![]), None)), Js::Obj(_)));
}

/// The documented deviations of `pdfplumber.open` reach JavaScript as
/// `NotPorted`, not wrapped in `PdfminerException`: `repair=True`
/// (Ghostscript), a password that is not a `str` for an encrypted document
/// (an unencrypted one never reads it: `js_runtime.py` `open.jsonl`).
/// `doc_attr`/`page_attr` are the `getattr` of `to_dict` and `to_csv`:
/// a name they cannot build (not ending with `s`) is not modelled, though
/// Python has the attribute.
#[test]
fn attributes_outside_to_dict_and_to_csv_are_not_ported() {
    let mut core = Core::new();
    let Js::Num(doc) = ok(call(&mut core, "doc_open", 0, PyValue::None, Some(tiny_pdf(b"0 0 m 10 10 l S")))) else {
        panic!()
    };
    let Js::Arr(pages) = ok(call(&mut core, "doc_pages", doc as u32, list(vec![]), None)) else { panic!() };
    let page = match &pages[0] {
        Js::Obj(items) => match &items[0].1 {
            Js::Num(h) => *h as u32,
            x => panic!("a handle: {x:?}"),
        },
        x => panic!("a dict: {x:?}"),
    };
    for name in ["width", "height", "layout", "bbox", "structure_tree"] {
        let r = call(&mut core, "page_attr", page, list(vec![PyValue::str(name)]), None);
        assert_eq!(error_name(&r).0, "NotPorted", "page {name}");
        let r = call(&mut core, "doc_attr", doc as u32, list(vec![PyValue::str(name)]), None);
        assert_eq!(error_name(&r).0, "NotPorted", "doc {name}");
    }
    let r = call(&mut core, "page_attr", page, list(vec![PyValue::str("lines")]), None);
    assert_eq!(r.status, 0);
}

#[test]
fn open_reports_what_is_not_modelled_as_not_ported() {
    let root = std::path::PathBuf::from(std::env::var("PDFPLUMBER_ROOT").expect("PDFPLUMBER_ROOT"));
    let encrypted = std::fs::read(root.join("upstream/pdfplumber-python/tests/pdfs/password-example.pdf")).unwrap();
    let mut core = Core::new();
    let open = |core: &mut Core, key: &str, value: PyValue, data: Vec<u8>| {
        let r = call(core, "doc_open", 0, PyValue::Dict(Obj::from_pairs([(key, value)])), Some(data));
        error_name(&r).0
    };
    assert_eq!(open(&mut core, "password", PyValue::Bytes(Rc::from(&b"test"[..])), encrypted.clone()), "NotPorted");
    assert_eq!(open(&mut core, "repair", PyValue::Bool(true), tiny_pdf(b"")), "NotPorted");
    assert_eq!(open(&mut core, "password", PyValue::str("wrong"), encrypted.clone()), "PdfminerException");
    let ok_open = call(
        &mut core,
        "doc_open",
        0,
        PyValue::Dict(Obj::from_pairs([("password", PyValue::str("test"))])),
        Some(encrypted),
    );
    assert_eq!(ok_open.status, 0);
}

/// A chain of derived pages, each the crop of the previous one, is freed
/// without recursion (`Drop for Page`), as CPython frees it: a recursive
/// drop overflowed the native stack after about 12 000 pages under
/// WebAssembly (an 8 MiB stack), and here on a 256 KiB thread.
#[test]
fn a_long_chain_of_derived_pages_drops_without_recursion() {
    use pdfplumber::page::CropFn;
    use pdfplumber::pdf::{OpenOptions, Source, PDF};
    let run = || {
        let data: Rc<[u8]> = Rc::from(tiny_pdf(b"BT /F1 10 Tf (a) Tj ET"));
        let kind = pdfminer::pdfparser::FileKind::BytesIO;
        let pdf = PDF::open(Source::Stream { data, kind }, OpenOptions::new()).unwrap();
        let mut page = pdf.pages().unwrap()[0].clone();
        let bbox = page.bbox.clone();
        for _ in 0..100_000 {
            page = page.crop(bbox.clone(), CropFn::CropToBbox, false, true).unwrap();
        }
        drop(pdf);
        drop(page);
    };
    if cfg!(target_family = "wasm") {
        run();
    } else {
        std::thread::Builder::new().stack_size(256 << 10).spawn(run).unwrap().join().unwrap();
    }
}
