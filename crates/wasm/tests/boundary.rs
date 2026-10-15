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
        3 | 4 => PyValue::Float(float(r)),
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
        (0, _) => PyValue::Float(float(r)),
        (1, 0) => PyValue::None,
        (1, _) => PyValue::Float(float(r)),
        (2, _) => PyValue::str(r.pick(STRS)),
        (3, 0) => PyValue::None,
        (3, _) => PyValue::str(r.pick(STRS)),
        (4, _) => PyValue::Bool(r.below(2) == 0),
        (5, 0) => PyValue::None,
        (5, k) => PyValue::tuple((0..k).map(|_| PyValue::Float(float(r)))),
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
    }
    assert!(seen.iter().all(|&s| s), "column kinds seen: {seen:?}");
    // the narrowest kind
    let k = |cells: &[PyValue]| column_kind(&cells.iter().collect::<Vec<_>>());
    let f = PyValue::Float(1.0);
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
        let (text, fixup) = json::encode(&PyValue::Float(x)).unwrap();
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
    assert_eq!(json::encode(&PyValue::Float(-0.0)).unwrap().0, b"-0");
    assert_eq!(json::encode(&PyValue::Float(1e21)).unwrap().0, b"1e21");
    assert_eq!(json::encode(&PyValue::Float(1e-7)).unwrap().0, b"1e-7");
    assert_eq!(json::encode(&PyValue::Float(1e-6)).unwrap().0, b"0.000001");
    assert_eq!(json::encode(&PyValue::list([])).unwrap().0, b"[]");
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
    std::mem::forget(back); // dropping 200 000 nested Rcs recurses (not the boundary's code)
    std::mem::forget(deep);
    list.set(1, PDFObject::Null); // breaks the cycle
}

/// A value JavaScript can send: plain Python values, no lone surrogate.
fn arg_value(r: &mut Rng, depth: u32) -> PyValue {
    match r.below(if depth > 3 { 6 } else { 9 }) {
        0..=5 => scalar(r),
        6 => PyValue::list((0..r.below(4)).map(|_| arg_value(r, depth + 1))),
        7 => PyValue::tuple((0..r.below(4)).map(|_| arg_value(r, depth + 1))),
        _ => PyValue::Dict(Obj::from_pairs(
            (0..r.below(4)).map(|_| (*r.pick(STRS), arg_value(r, depth + 1))).collect::<Vec<_>>(),
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
    assert_eq!(err(&header(&[0x11, 3, 0xed, 0xa0, 0x80])).kind, pycompat::ExcKind::UnicodeEncodeError);
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
    assert_eq!(err(&special).kind, pycompat::ExcKind::UnicodeEncodeError);
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

#[test]
fn documents_handles_formats_and_the_benchmark() {
    big_stack(documents_handles_formats_and_the_benchmark_body);
}

fn documents_handles_formats_and_the_benchmark_body() {
    let mut core = Core::new();
    let opts = PyValue::Dict(Obj::from_pairs([("password", PyValue::str("pw"))]));
    let Js::Num(h) = ok(call(&mut core, "doc_open", 0, opts, Some(b"%PDF-1.4".to_vec()))) else { panic!("a handle") };
    let h = h as u32;
    assert_ne!(h, 0);
    assert_eq!(
        ok(call(&mut core, "doc_info", h, PyValue::None, None)),
        obj(vec![("size", Js::Num(8.0)), ("options", obj(vec![("password", s("pw"))]))])
    );
    let info = ok(call(&mut core, "info", 0, PyValue::None, None));
    let expected = obj(vec![
        ("version", s(env!("CARGO_PKG_VERSION"))),
        ("pdfminer", s("20260107")),
        ("pdfplumber", s("0.11.9")),
        ("handles", Js::Num(1.0)),
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
