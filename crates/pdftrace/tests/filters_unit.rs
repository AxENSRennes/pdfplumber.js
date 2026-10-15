//! Replays the cases of `oracle/unit/filters.py` (pdfminer results) through
//! `pdfminer::{lzw, ascii85, runlength, utils}` (predictors) and
//! `pdftypes::{PDFStream.get_data, decompress_corrupted}`: generated cases,
//! then every stream of the real corpus.
//!
//! `/CCITTFaxDecode` is ported separately: a case whose Rust result is the
//! `NotPorted` error of the `ccitt` stub is counted as skipped (only cases
//! with that filter may be).

use std::fs;
use std::rc::Rc;

use pdfminer::ascii85::{ascii85decode, asciihexdecode};
use pdfminer::lzw::lzwdecode;
use pdfminer::pdftypes::{decompress_corrupted, PDFDict, PDFList, PDFObject, PDFStream};
use pdfminer::psparser::{LIT_bytes, KWD, LIT};
use pdfminer::runlength::rldecode;
use pdfminer::utils::{apply_png_predictor, apply_tiff_predictor, paeth_predictor};
use pdftrace::canon::sha256_hex;
use pdftrace::typed_json::{self, Json};
use pdftrace::unit::UnitData;
use pdftrace::value::{NameValue, Value};
use pycompat::{ExcKind, PyErr, PyInt, PyResult};

/// `BLOB_MAX` of the generator.
const BLOB_MAX: usize = 2048;

fn sha(b: &[u8]) -> Value {
    Value::Tuple(vec![Value::str("sha256"), Value::Int(PyInt::from(b.len() as u64)), Value::str(sha256_hex(b))])
}

/// The generator's `blob(b)`.
fn blob(b: &[u8]) -> Value {
    if b.len() <= BLOB_MAX {
        Value::Bytes(b.to_vec())
    } else {
        sha(b)
    }
}

/// A result; `None` for the `NotPorted` error of a module not ported yet.
fn result(r: PyResult<impl AsRef<[u8]>>) -> Option<Value> {
    match r {
        Ok(b) => Some(blob(b.as_ref())),
        Err(e) => err(&e),
    }
}

fn err(e: &PyErr) -> Option<Value> {
    (e.kind != ExcKind::NotPorted).then(|| Value::from(e))
}

fn bytes(v: &Value) -> &[u8] {
    match v {
        Value::Bytes(b) => b,
        other => panic!("not bytes: {other:?}"),
    }
}

fn int(v: &Value) -> PyInt {
    match v {
        Value::Int(i) => i.clone(),
        Value::Bool(b) => PyInt::Small(i64::from(*b)),
        other => panic!("not an int: {other:?}"),
    }
}

/// The PDF object of a typed JSON value (no references in these cases).
fn build(v: &Value) -> PDFObject {
    match v {
        Value::None => PDFObject::Null,
        Value::Bool(b) => PDFObject::Bool(*b),
        Value::Int(i) => PDFObject::Int(i.clone()),
        Value::Float(f) => PDFObject::Float(*f),
        Value::Bytes(b) => PDFObject::Bytes(Rc::from(&b[..])),
        Value::Literal(NameValue::Str(s)) => PDFObject::Literal(LIT(s.as_str().expect("a literal name"))),
        Value::Literal(NameValue::Bytes(b)) => PDFObject::Literal(LIT_bytes(b)),
        Value::Keyword(k) => PDFObject::Keyword(KWD(k)),
        Value::List(items) => PDFObject::List(PDFList::new(items.iter().map(build).collect())),
        Value::Dict(items) => PDFObject::Dict(dict(items)),
        Value::Stream(sv) => {
            let (Value::Bytes(raw), Value::None) = (&sv.raw, &sv.data) else { panic!("a decoded stream: {sv:?}") };
            PDFObject::Stream(Rc::new(PDFStream::new(dict(&sv.attrs), Rc::from(&raw[..]), None)))
        }
        other => panic!("not a PDF object: {other:?}"),
    }
}

fn dict(items: &[(Value, Value)]) -> PDFDict {
    PDFDict::from_pairs(items.iter().map(|(k, x)| (Rc::from(k.as_str().expect("a str key")), build(x))))
}

/// Replays `<name>.jsonl`; `run` returns `None` for a skipped case.
fn check(name: &str, min_cases: usize, run: impl Fn(&[Value]) -> Option<Value>) {
    let cases = UnitData::open("filters").cases(name);
    assert!(cases.len() >= min_cases, "{name}: only {} cases", cases.len());
    let mut differ = Vec::new();
    let mut skipped = 0;
    for (k, (args, expected)) in cases.iter().enumerate() {
        match run(args) {
            None => skipped += 1,
            Some(got) if !got.same(expected) => differ.push(format!("#{k} {args:?}\n  expected {expected:?}\n       got {got:?}")),
            Some(_) => {}
        }
    }
    println!("{name}: {}/{} identical, {skipped} skipped (not ported)", cases.len() - differ.len() - skipped, cases.len());
    assert!(
        differ.is_empty(),
        "{name}: {} differ:\n{}",
        differ.len(),
        differ.iter().take(10).cloned().collect::<Vec<_>>().join("\n")
    );
}

#[test]
fn lzw() {
    check("lzw", 2000, |a| result(lzwdecode(bytes(&a[0]))));
}

#[test]
fn ascii85() {
    check("ascii85", 3000, |a| result(ascii85decode(bytes(&a[0]))));
}

#[test]
fn asciihex() {
    check("asciihex", 2000, |a| result(asciihexdecode(bytes(&a[0]))));
}

#[test]
fn runlength() {
    check("runlength", 2000, |a| result(rldecode(bytes(&a[0]))));
}

#[test]
fn paeth() {
    check("paeth", 2000, |a| {
        let v = |i: usize| int(&a[i]).as_i64().expect("small");
        Some(Value::Int(PyInt::Small(paeth_predictor(v(0), v(1), v(2)))))
    });
}

#[test]
fn tiff() {
    check("tiff", 2500, |a| result(apply_tiff_predictor(&int(&a[0]), &int(&a[1]), &build(&a[2]), bytes(&a[3]))));
}

#[test]
fn png() {
    check("png", 5000, |a| {
        result(apply_png_predictor(&int(&a[0]), &int(&a[1]), &int(&a[2]), &build(&a[3]), bytes(&a[4])))
    });
}

#[test]
fn corrupted() {
    check("corrupted", 1500, |a| result(decompress_corrupted(bytes(&a[0]))));
}

#[test]
fn streams() {
    check("streams", 4000, |a| {
        let PDFObject::Dict(attrs) = build(&a[0]) else { panic!("attrs") };
        let s = PDFStream::new(attrs, Rc::from(bytes(&a[1])), None);
        let filters = match s.get_filters() {
            Ok(pairs) => Value::List(
                pairs.iter().map(|(f, p)| Value::Tuple(vec![Value::from(f), Value::from(p)])).collect(),
            ),
            Err(e) => err(&e)?,
        };
        let data = result(s.get_data())?;
        let raw = s.get_rawdata().map_or(Value::None, |r| Value::Bytes(r.to_vec()));
        Some(Value::List(vec![filters, data, raw]))
    });
}

/// Every stream of the real corpus: `get_data()` of the recorded filter
/// entries and raw data.
#[test]
fn real_corpus() {
    let data = UnitData::open("filters");
    let json_path = data.path("corpus.json");
    let text = fs::read_to_string(&json_path).unwrap_or_else(|_| {
        panic!("missing {}: generate it with python -m oracle.unit.filters (without --no-corpus)", json_path.display())
    });
    let oracle = typed_json::parse_json(&text).expect("corpus.json");
    data.check_sources(&json_path, oracle.get("sources"));
    let Some(Json::Array(records)) = oracle.get("records") else { panic!("no records") };
    let bin = fs::read(data.path("corpus.bin")).expect("corpus.bin");
    assert_eq!(&bin[..4], b"FLT1");
    let rd = |o: usize| u32::from_le_bytes(bin[o..o + 4].try_into().expect("4 bytes")) as usize;
    assert_eq!(rd(4), records.len());
    assert!(records.len() >= 1000, "only {} streams", records.len());
    let mut off = 8;
    let mut differ = Vec::new();
    let mut skipped = 0;
    let mut total = 0;
    for rec in records {
        let n = rd(off);
        let attrs_json = std::str::from_utf8(&bin[off + 4..off + 4 + n]).expect("ascii");
        off += 4 + n;
        let n = rd(off);
        let raw = &bin[off + 4..off + 4 + n];
        off += 4 + n;
        total += raw.len();
        let attrs = typed_json::from_json(&typed_json::parse_json(attrs_json).expect("attrs")).expect("typed attrs");
        let PDFObject::Dict(attrs) = build(&attrs) else { panic!("attrs") };
        let expected = typed_json::from_json(rec.get("out").expect("out")).expect("typed out");
        let got = match PDFStream::new(attrs, Rc::from(raw), None).get_data() {
            Ok(b) => Some(sha(&b)),
            Err(e) => err(&e),
        };
        match got {
            None => skipped += 1,
            Some(got) if !got.same(&expected) => {
                differ.push(format!("{:?}: expected {expected:?}, got {got:?}", rec.get("id")));
            }
            Some(_) => {}
        }
    }
    assert_eq!(off, bin.len(), "trailing bytes in corpus.bin");
    println!(
        "real corpus: {}/{} streams identical, {skipped} skipped (not ported), {:.1} MB raw",
        records.len() - differ.len() - skipped,
        records.len(),
        total as f64 / 1e6
    );
    assert!(differ.is_empty(), "{} differ:\n{}", differ.len(), differ.iter().take(20).cloned().collect::<Vec<_>>().join("\n"));
}
