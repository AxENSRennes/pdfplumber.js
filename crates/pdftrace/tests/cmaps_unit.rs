//! Replays the cases of `oracle/unit/cmaps.py` (pdfminer results) through
//! `pdfminer::{cmapdb, encodingdb, glyphlist, latin_enc, fontmetrics}`.
//!
//! The predefined CMaps come from the compact files of
//! `oracle/codegen/cmaps.py`, whose directory the generator records in
//! `meta.json` (`cmap_dir`, relative to the cache). Outputs of the sets
//! `methods`, `predefined`, `parse` and `corpus` are compared through their
//! canonical text (the text, or `sha256:<hex>:<length>` when it is long);
//! `python -m oracle.unit.cmaps --dump <set> <index>` prints Python's. With
//! `CMAPS_DUMP_DIR` set, the Rust text of each differing case is written
//! there as `<set>-<index>.txt`.
//!
//! The cases of a set run in their order on one thread, as in the
//! generator, so that the CMap caches hold the same objects.
//!
//! Mutations (`cargo mutants -f crates/pdfminer/src/<file> --test-package
//! pdftrace --cargo-test-arg=--test=cmaps_unit --profile mutants -j 1
//! --timeout 20 --baseline skip`, one file per run, `-E check_growth` for
//! cmapdb.rs): every mutant of cmapdb.rs is caught, unviable or a timeout
//! (an endless loop); fontmetrics.rs: all caught; glyphlist.rs and
//! latin_enc.rs: no mutant. One equivalent mutant survives in
//! encodingdb.rs: the guard `KeyError || ValueError` of `get_encoding` made
//! `true`, since `name2unicode` raises nothing else (`PDFKeyError`, or the
//! `ValueError` of `int()`/`chr()`; at most six hex digits never overflow
//! the C int of `chr()`).

use std::fs;
use std::rc::Rc;
use std::sync::Arc;

use pdfminer::cmapdb::{
    AttrValue, Attrs, CMap, CMapBase, CMapDB, CMapParser, Code2Cid, Code2CidValue, DirCMapProvider, IdentityCMap,
    PlainCMapBase, Unichr, UnicodeMap,
};
use pdfminer::encodingdb::{name2unicode, name2unicode_literal, EncodingDB};
use pdfminer::fontmetrics::{FontMetricValue, FONT_METRICS};
use pdfminer::glyphlist::GLYPHNAME2UNICODE;
use pdfminer::latin_enc::ENCODING;
use pdfminer::pdftypes::{PDFDict, PDFList, PDFObject};
use pdfminer::psparser::{LIT_bytes, LiteralName, KWD, LIT};
use pdftrace::canon::{canon, sha256_hex};
use pdftrace::typed_json::{self, Json};
use pdftrace::unit::{cache_dir, UnitData};
use pdftrace::value::{NameValue, PyStr, Value};
use pycompat::io::BytesIO;
use pycompat::recursion::{self, Depth, C_RECURSION_LIMIT};
use pycompat::{PyInt, PyResult};

/// Python's `INLINE_LIMIT`: longer outputs are compared by SHA-256.
const INLINE_LIMIT: usize = 4000;

/// Opens the data and installs the provider of the predefined CMaps.
fn open() -> UnitData {
    let data = UnitData::open("cmaps");
    let meta = fs::read_to_string(data.path("meta.json")).expect("meta.json");
    let meta = typed_json::parse_json(&meta).expect("meta.json is JSON");
    let Some(Json::String(rel)) = meta.get("cmap_dir") else { panic!("meta.json has no cmap_dir") };
    let dir = cache_dir().join(rel);
    let provider = DirCMapProvider::new(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
    CMapDB::set_provider(Some(Rc::new(provider)));
    data
}

/// Checks the marker `<name>.json` that the generator writes after `<name>.jsonl`.
fn check_marker(data: &UnitData, name: &str) {
    let path = data.path(&format!("{name}.json"));
    let text = fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "missing {}: generate it with python -m oracle.unit.cmaps{}",
            path.display(),
            if name == "corpus" { " --corpus" } else { "" }
        )
    });
    let marker = typed_json::parse_json(&text).expect("marker JSON");
    data.check_sources(&path, marker.get("sources"));
}

// -- values ---------------------------------------------------------------------

/// A PDF object from its typed value.
fn obj(v: &Value) -> PDFObject {
    match v {
        Value::None => PDFObject::Null,
        Value::Bool(b) => PDFObject::Bool(*b),
        Value::Int(i) => PDFObject::Int(i.clone()),
        Value::Float(f) => PDFObject::Float(*f),
        Value::Bytes(b) => PDFObject::Bytes(Rc::from(&b[..])),
        Value::Literal(NameValue::Str(s)) => PDFObject::Literal(LIT(s.as_str().expect("a literal name"))),
        Value::Literal(NameValue::Bytes(b)) => PDFObject::Literal(LIT_bytes(b)),
        Value::Keyword(k) => PDFObject::Keyword(KWD(k)),
        Value::List(items) => PDFObject::List(PDFList::new(items.iter().map(obj).collect())),
        Value::Dict(items) => PDFObject::Dict(PDFDict::from_pairs(
            items.iter().map(|(k, x)| (Rc::from(k.as_str().expect("a str key")), obj(x))),
        )),
        other => panic!("not a PDF object: {other:?}"),
    }
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

fn bytes(v: &Value) -> &[u8] {
    match v {
        Value::Bytes(b) => b,
        other => panic!("not bytes: {other:?}"),
    }
}

fn list(v: &Value) -> &[Value] {
    match v {
        Value::List(items) => items,
        other => panic!("not a list: {other:?}"),
    }
}

fn unichr(u: &Unichr) -> Value {
    match u {
        Unichr::Text(s) => Value::str(s.to_string()),
        Unichr::Surrogate(c) => Value::Str(PyStr::from_code_points(vec![*c]).expect("a surrogate")),
    }
}

fn attr(a: &AttrValue) -> Value {
    match a {
        AttrValue::Str(s) => Value::str(s.clone()),
        AttrValue::Obj(o) => Value::from(o),
    }
}

fn ints(cids: &[PyInt]) -> Value {
    Value::List(cids.iter().cloned().map(Value::Int).collect())
}

/// A result, or the exception.
fn res<T>(r: PyResult<T>, f: impl FnOnce(T) -> Value) -> Value {
    match r {
        Ok(v) => f(v),
        Err(e) => Value::from(&e),
    }
}

fn none<T>(r: PyResult<T>) -> Value {
    res(r, |_| Value::None)
}

/// `tree(code2cid)` of the generator.
fn tree(d: &Code2Cid) -> Value {
    Value::List(
        d.iter()
            .map(|(k, v)| {
                let v = match v {
                    Code2CidValue::Cid(cid) => Value::Int(cid.clone()),
                    Code2CidValue::Map(m) => tree(m),
                };
                Value::List(vec![Value::Int(PyInt::from(*k)), v])
            })
            .collect(),
    )
}

/// `state(m)` of the generator.
fn state(m: &dyn CMapBase) -> Value {
    let attrs = Value::List(m.attrs().iter().map(|(k, v)| Value::List(vec![Value::str(k.clone()), attr(v)])).collect());
    let code2cid = m.as_cmap().map_or(Value::None, |c| tree(&c.code2cid));
    let cid2unichr = m.as_unicode_map().map_or(Value::None, |u| {
        Value::List(u.cid2unichr.iter().map(|(k, v)| Value::List(vec![Value::Int(k.clone()), unichr(v)])).collect())
    });
    Value::List(vec![attrs, code2cid, cid2unichr])
}

/// `canon_or_hash(v)` of the generator.
fn canon_or_hash(v: &Value) -> String {
    let text = canon(v);
    if text.len() <= INLINE_LIMIT {
        return text;
    }
    format!("sha256:{}:{}", sha256_hex(text.as_bytes()), text.len())
}

/// Exact comparison, with any NaN equal to any NaN.
fn matches(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Float(x), Value::Float(y)) => x.to_bits() == y.to_bits() || (x.is_nan() && y.is_nan()),
        (Value::List(x), Value::List(y)) | (Value::Tuple(x), Value::Tuple(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| matches(p, q))
        }
        _ => a.same(b),
    }
}

/// Replays `<name>.jsonl`, whose outputs are typed values.
fn replay(data: &UnitData, name: &str, min_cases: usize, mut f: impl FnMut(&[Value]) -> Value) {
    let cases = data.cases(name);
    assert!(cases.len() >= min_cases, "{name}: only {} cases", cases.len());
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

/// Replays `<name>.jsonl`, whose outputs are `canon_or_hash` texts.
fn replay_canon(data: &UnitData, name: &str, min_cases: usize, mut f: impl FnMut(&[Value]) -> Value) {
    let cases = data.cases(name);
    assert!(cases.len() >= min_cases, "{name}: only {} cases", cases.len());
    let dump_dir = std::env::var_os("CMAPS_DUMP_DIR");
    let mut failures = Vec::new();
    for (k, (args, expected)) in cases.iter().enumerate() {
        let actual = f(args);
        let got = canon_or_hash(&actual);
        if Some(got.as_str()) != expected.as_str() {
            let shown = |s: &str| s.chars().take(600).collect::<String>();
            let input = shown(&format!("{args:?}"));
            failures.push(format!(
                "#{k} {input}\n    python: {}\n    rust:   {}",
                shown(expected.as_str().unwrap_or("?")),
                shown(&got)
            ));
            if let Some(dir) = &dump_dir {
                fs::create_dir_all(dir).unwrap();
                fs::write(std::path::Path::new(dir).join(format!("{name}-{k}.txt")), canon(&actual)).unwrap();
            }
        }
    }
    println!("{name}: {}/{} cases identical", cases.len() - failures.len(), cases.len());
    assert!(
        failures.is_empty(),
        "{name}: {}/{} cases differ (python -m oracle.unit.cmaps --dump {name} <index>); first ones:\n{}",
        failures.len(),
        cases.len(),
        failures.iter().take(8).cloned().collect::<Vec<_>>().join("\n")
    );
}

// -- objects of every class ---------------------------------------------------------

/// An instance of one of the classes the generator instantiates.
enum Instance {
    Base(PlainCMapBase),
    CMap(CMap),
    Identity(IdentityCMap),
    Unicode(UnicodeMap),
}

impl Instance {
    fn new(class: &str, attrs: Attrs) -> Instance {
        match class {
            "CMapBase" => Instance::Base(PlainCMapBase::new(attrs)),
            "CMap" => Instance::CMap(CMap::new(attrs)),
            "FileCMap" => Instance::CMap(CMap::new_file(attrs)),
            "IdentityCMap" => Instance::Identity(IdentityCMap::new(attrs)),
            "IdentityCMapByte" => Instance::Identity(IdentityCMap::new_byte(attrs)),
            "UnicodeMap" => Instance::Unicode(UnicodeMap::new(attrs)),
            "IdentityUnicodeMap" => Instance::Unicode(UnicodeMap::new_identity(attrs)),
            "FileUnicodeMap" => Instance::Unicode(UnicodeMap::new_file(attrs)),
            other => panic!("unknown class {other}"),
        }
    }

    fn base(&self) -> &dyn CMapBase {
        match self {
            Instance::Base(m) => m,
            Instance::CMap(m) => m,
            Instance::Identity(m) => m,
            Instance::Unicode(m) => m,
        }
    }

    fn base_mut(&mut self) -> &mut dyn CMapBase {
        match self {
            Instance::Base(m) => m,
            Instance::CMap(m) => m,
            Instance::Identity(m) => m,
            Instance::Unicode(m) => m,
        }
    }

    fn unicode(&self) -> &UnicodeMap {
        match self {
            Instance::Unicode(m) => m,
            _ => panic!("not a UnicodeMap"),
        }
    }
}

/// `**kwargs` of a constructor: `str` or `int` values.
fn kwargs(v: &Value) -> Attrs {
    let Value::Dict(items) = v else { panic!("kwargs {v:?}") };
    items
        .iter()
        .map(|(k, v)| {
            let v = match v {
                Value::Str(s) => AttrValue::Str(s.as_str().expect("text").to_owned()),
                Value::Int(i) => AttrValue::Obj(PDFObject::Int(i.clone())),
                other => panic!("kwarg {other:?}"),
            };
            (text(k).to_owned(), v)
        })
        .collect()
}

// -- tables ----------------------------------------------------------------------------

#[test]
fn tables() {
    let data = open();
    check_marker(&data, "synthetic");
    let metric = |v: &FontMetricValue| match v {
        FontMetricValue::Str(s) => Value::str(*s),
        FontMetricValue::Int(i) => Value::Int(PyInt::Small(*i)),
        FontMetricValue::Float(f) => Value::Float(*f),
        FontMetricValue::FloatTuple(t) => Value::Tuple(t.iter().map(|f| Value::Float(*f)).collect()),
    };
    let opt = |v: Option<i64>| v.map_or(Value::None, |v| Value::Int(PyInt::Small(v)));
    replay(&data, "tables", 5, |a| match (text(&a[0]), a.get(1)) {
        ("glyphname2unicode", None) => Value::List(
            GLYPHNAME2UNICODE.iter().map(|(k, v)| Value::List(vec![Value::str(*k), Value::str(*v)])).collect(),
        ),
        ("ENCODING", None) => Value::List(
            ENCODING
                .iter()
                .map(|&(name, a, b, c, d)| Value::List(vec![Value::str(name), opt(a), opt(b), opt(c), opt(d)]))
                .collect(),
        ),
        ("FONT_METRICS", None) => Value::List(FONT_METRICS.iter().map(|(n, _)| Value::str(*n)).collect()),
        ("encodings", Some(names)) => {
            let db = EncodingDB::get();
            let tables = [
                ("std", &db.std2unicode),
                ("mac", &db.mac2unicode),
                ("win", &db.win2unicode),
                ("pdf", &db.pdf2unicode),
            ];
            Value::List(
                list(names)
                    .iter()
                    .map(|name| {
                        let found = EncodingDB::encodings(text(name))
                            .and_then(|e| tables.iter().find(|(_, t)| std::ptr::eq(*t, e)))
                            .map_or(Value::None, |(k, _)| Value::str(*k));
                        Value::List(vec![name.clone(), found])
                    })
                    .collect(),
            )
        }
        ("FONT_METRICS", Some(name)) => {
            let m = pdfminer::fontmetrics::get(text(name)).expect("a font");
            let first = FONT_METRICS.iter().find(|(_, x)| std::ptr::eq(*x, m)).expect("listed").0;
            Value::List(vec![
                Value::List(m.descriptor.iter().map(|(k, v)| Value::List(vec![Value::str(*k), metric(v)])).collect()),
                Value::List(
                    m.widths
                        .iter()
                        .map(|(k, v)| Value::List(vec![Value::str(*k), Value::Int(PyInt::Small(*v))]))
                        .collect(),
                ),
                Value::str(first),
            ])
        }
        other => panic!("unknown table {other:?}"),
    });
    // the dict view of the generated pairs
    for (k, v) in GLYPHNAME2UNICODE {
        assert_eq!(pdfminer::glyphlist::glyphname2unicode.get(k), Some(v));
    }
    assert_eq!(pdfminer::glyphlist::glyphname2unicode.len(), GLYPHNAME2UNICODE.len());
}

// -- encodingdb ---------------------------------------------------------------------------

#[test]
fn name2unicode_cases() {
    let data = open();
    replay(&data, "name2unicode", 7000, |a| match &a[0] {
        Value::Str(s) => res(name2unicode(s.as_str().expect("text")), Value::str),
        Value::Bytes(b) => {
            res(name2unicode_literal(LiteralName::Bytes(Box::leak(b.clone().into_boxed_slice()))), Value::str)
        }
        other => panic!("name {other:?}"),
    });
}

#[test]
fn encoding() {
    let data = open();
    let db = EncodingDB::get();
    let bases = [&db.std2unicode, &db.mac2unicode, &db.win2unicode, &db.pdf2unicode];
    replay(&data, "encoding", 800, |a| {
        let diff: Option<Vec<PDFObject>> = match &a[1] {
            Value::None => None,
            v => Some(list(v).iter().map(obj).collect()),
        };
        res(EncodingDB::get_encoding(text(&a[0]), diff.as_deref()), |d| {
            let is_base = bases.iter().any(|b| std::ptr::eq(*b, &*d));
            let items =
                d.iter().map(|(k, v)| Value::List(vec![Value::Int(k.clone()), Value::str(v.clone())])).collect();
            Value::List(vec![Value::Bool(is_base), Value::List(items)])
        })
    });
}

// -- methods --------------------------------------------------------------------------------

/// `build_source(spec)` of the generator.
fn build_source(spec: &[Value]) -> PyResult<Rc<dyn CMapBase>> {
    match text(&spec[0]) {
        "predefined" => CMapDB::get_cmap(text(&spec[1])),
        "new" => {
            let mut m = Instance::new(text(&spec[1]), Attrs::default());
            for pair in list(&spec[2]) {
                let pair = list(pair);
                m.base_mut().add_code2cid(text(&pair[0]), int(&pair[1]))?;
            }
            Ok(match m {
                Instance::Base(m) => Rc::new(m),
                Instance::CMap(m) => Rc::new(m),
                Instance::Identity(m) => Rc::new(m),
                Instance::Unicode(m) => Rc::new(m),
            })
        }
        other => panic!("source {other}"),
    }
}

/// `apply_op(m, op)` of the generator.
fn apply_op(m: &mut Instance, op: &[Value]) -> Value {
    match text(&op[0]) {
        "add_code2cid" => none(m.base_mut().add_code2cid(text(&op[1]), int(&op[2]))),
        "add_cid2unichr" => none(m.base_mut().add_cid2unichr(int(&op[1]), &obj(&op[2]))),
        "use_cmap" => none(build_source(list(&op[1])).and_then(|src| m.base_mut().use_cmap(&*src))),
        "set_attr" => {
            m.base_mut().set_attr(text(&op[1]).to_owned(), AttrValue::Obj(obj(&op[2])));
            Value::None
        }
        "decode" => res(m.base().decode(bytes(&op[1])), |c| ints(&c)),
        "get_unichr" => res(m.unicode().get_unichr(&int(&op[1])), |u| unichr(&u)),
        "is_vertical" => Value::Bool(m.base().is_vertical()),
        "repr" => match m {
            Instance::CMap(c) => res(c.repr(), Value::str),
            Instance::Unicode(u) => res(u.repr(), Value::str),
            _ => panic!("repr of a {}", m.base().class_name()),
        },
        "dump" => match m {
            Instance::CMap(c) => Value::str(c.dump()),
            Instance::Unicode(u) => Value::str(u.dump()),
            _ => panic!("dump of a {}", m.base().class_name()),
        },
        other => panic!("unknown op {other}"),
    }
}

#[test]
fn methods() {
    let data = open();
    replay_canon(&data, "methods", 2000, |a| {
        let mut m = Instance::new(text(&a[0]), kwargs(&a[1]));
        let mut results: Vec<Value> = list(&a[2]).iter().map(|op| apply_op(&mut m, list(op))).collect();
        results.push(state(m.base()));
        Value::List(results)
    });
}

// -- predefined CMaps ------------------------------------------------------------------------

/// The byte strings of the codes of `code2cid` (keys below 256 only).
fn leaf_paths(d: &Code2Cid, prefix: &mut Vec<u32>, out: &mut Vec<u8>) {
    for (k, v) in d {
        prefix.push(*k);
        match v {
            Code2CidValue::Map(m) => leaf_paths(m, prefix, out),
            Code2CidValue::Cid(_) => {
                if prefix.iter().all(|&x| x < 256) {
                    out.extend(prefix.iter().map(|&x| x as u8));
                }
            }
        }
        prefix.pop();
    }
}

/// `cmap_report(samples, m)` of the generator.
fn cmap_report(samples: &[Value], m: &dyn CMapBase) -> Value {
    let decode = |code: &[u8]| ints(&m.decode(code).expect("a CMap decodes"));
    let mut out = vec![
        Value::str(m.class_name()),
        Value::List(m.attrs().iter().map(|(k, v)| Value::List(vec![Value::str(k.clone()), attr(v)])).collect()),
        Value::Bool(m.is_vertical()),
    ];
    match m.as_cmap() {
        Some(c) => {
            out.push(tree(&c.code2cid));
            let mut paths = Vec::new();
            leaf_paths(&c.code2cid, &mut Vec::new(), &mut paths);
            out.push(decode(&paths));
        }
        None => out.extend([Value::None, Value::None]),
    }
    out.push(Value::List((0..=255u8).map(|b| decode(&[b])).collect()));
    let all: Vec<u8> = (0..=255u8).flat_map(|a| (0..=255u8).flat_map(move |b| [a, b])).collect();
    out.push(decode(&all));
    out.push(Value::List(samples.iter().map(|s| Value::List(vec![s.clone(), decode(bytes(s))])).collect()));
    Value::List(out)
}

/// `umap_report(m)` of the generator.
fn umap_report(m: &UnicodeMap) -> Value {
    let keys: Vec<PyInt> = m.cid2unichr.keys().cloned().collect();
    let mut probes: Vec<PyInt> = keys.iter().step_by(97).cloned().collect();
    let max = keys.iter().max().cloned().unwrap_or(PyInt::ZERO);
    probes.extend([PyInt::Small(-1), PyInt::ZERO, max.add(&PyInt::Small(1)), PyInt::Small(1 << 40)]);
    Value::List(vec![
        Value::str(m.class_name()),
        Value::List(m.attrs.iter().map(|(k, v)| Value::List(vec![Value::str(k.clone()), attr(v)])).collect()),
        Value::Bool(m.is_vertical()),
        Value::List(m.cid2unichr.iter().map(|(k, v)| Value::List(vec![Value::Int(k.clone()), unichr(v)])).collect()),
        Value::List(
            probes
                .iter()
                .map(|c| Value::List(vec![Value::Int(c.clone()), res(m.get_unichr(c), |u| unichr(&u))]))
                .collect(),
        ),
    ])
}

#[test]
fn predefined() {
    let data = open();
    replay_canon(&data, "predefined", 180, |a| match text(&a[0]) {
        "get_cmap" => {
            let name = text(&a[1]);
            res(CMapDB::get_cmap(name), |m| {
                let again = CMapDB::get_cmap(name).expect("found once");
                Value::List(vec![cmap_report(list(&a[2]), &*m), Value::Bool(Rc::ptr_eq(&again, &m))])
            })
        }
        "get_unicode_map" => {
            let (name, vertical) = (text(&a[1]), matches!(a[2], Value::Bool(true)));
            res(CMapDB::get_unicode_map(name, vertical), |m| {
                let again = CMapDB::get_unicode_map(name, vertical).expect("found once");
                Value::List(vec![umap_report(&m), Value::Bool(Rc::ptr_eq(&again, &m))])
            })
        }
        other => panic!("unknown call {other}"),
    });
}

// -- CMapParser ----------------------------------------------------------------------------------

/// The samples of `parse_job`.
const DECODE_SAMPLES: [&[u8]; 7] =
    [b"", b"\x00", b"A", b"\x00A", b"\x81\x40\x82", b"\xff\xff", b"\x00\x01\x02\x03\x04"];

/// `parse_job(target, data)` of the generator, from the recursion depth
/// pdfminer ran it at.
fn parse_job(a: &[Value]) -> Value {
    let depth = |name: &str| match &a[2] {
        Value::Dict(items) => {
            let v = items.iter().find(|(k, _)| k.as_str() == Some(name)).map(|(_, v)| int(v)).expect("depth");
            u32::try_from(v.as_i64().expect("small")).expect("a depth")
        }
        other => panic!("depth {other:?}"),
    };
    let base = Depth { py: depth("py"), c: C_RECURSION_LIMIT - depth("c_left") };
    recursion::with_depth(base, || {
        let mut m = Instance::new(text(&a[0]), Attrs::default());
        let (error, warnings) = {
            let mut parser =
                CMapParser::new(m.base_mut(), BytesIO::new(Rc::from(bytes(&a[1])))).expect("a parser over bytes");
            let error = none(parser.run());
            let mut warnings = parser.ext.warnings.clone();
            warnings.sort();
            (error, warnings)
        };
        let mut out = vec![error, state(m.base()), Value::List(warnings.into_iter().map(Value::str).collect())];
        out.push(Value::List(DECODE_SAMPLES.iter().map(|s| res(m.base().decode(s), |c| ints(&c))).collect()));
        out.push(match &m {
            Instance::Unicode(u) => {
                let keys: Vec<PyInt> = u.cid2unichr.keys().cloned().collect();
                let mut probes: Vec<PyInt> = keys.iter().take(3).cloned().collect();
                probes.extend(keys.last().cloned());
                probes.extend(
                    [0, 1, 32, 65, 0xA0, 0xD800, 0x10FFFF, 0x110000, -1, 1 << 40].into_iter().map(PyInt::Small),
                );
                Value::List(
                    probes
                        .iter()
                        .map(|c| Value::List(vec![Value::Int(c.clone()), res(u.get_unichr(c), |x| unichr(&x))]))
                        .collect(),
                )
            }
            _ => Value::None,
        });
        Value::List(out)
    })
}

#[test]
fn parse_adversarial() {
    let data = open();
    replay_canon(&data, "parse", 3000, parse_job);
}

#[test]
fn parse_real_corpus() {
    let data = open();
    check_marker(&data, "corpus");
    replay_canon(&data, "corpus", 1000, parse_job);
}

// -- the compact format of the predefined CMaps (the port's own format, not pdfminer's) ----------

/// A compact file: the header, then the given attributes.
fn compact(attrs: &[&[u8]]) -> Vec<u8> {
    let mut out = b"PMCM\x01".to_vec();
    out.push(u8::try_from(attrs.len()).unwrap());
    for a in attrs {
        out.extend_from_slice(a);
    }
    out
}

#[test]
fn compact_format() {
    use pdfminer::cmapdb::CMapModule;
    // IS_VERTICAL, then CODE2CID {1: 7, 2: 8, 3: 9, 0x41: {0x42: 300}}, then the two unicode maps
    let vertical: &[u8] = b"\x0bIS_VERTICALB\x01";
    let code2cid: &[u8] = b"\x08CODE2CIDC\x02\x06\x01\x07\x01\x41\x01\x02\x42\xac\x02";
    let h: &[u8] = b"\x0cCID2UNICHR_HU\x02\x04\x05\x41\x01\x07\x02\xc3\xa9";
    let v: &[u8] = b"\x0cCID2UNICHR_VU\x00";
    let m = CMapModule::parse("X", &compact(&[vertical, code2cid, h, v])).expect("a valid file");
    assert_eq!(m.is_vertical, Some(true));
    let c2c = m.code2cid.as_ref().expect("CODE2CID");
    let cid = |k: u32| match c2c.get(&k) {
        Some(Code2CidValue::Cid(c)) => c.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!((cid(1), cid(2), cid(3)), (PyInt::Small(7), PyInt::Small(8), PyInt::Small(9)));
    assert_eq!(c2c.len(), 4);
    let Some(Code2CidValue::Map(sub)) = c2c.get(&0x41) else { panic!("a nested dict") };
    assert_eq!(sub.get(&0x42), Some(&Code2CidValue::Cid(PyInt::Small(300))));
    let h = m.cid2unichr_h.as_ref().expect("H");
    let get = |k: i64| h.get(&PyInt::Small(k)).cloned();
    assert_eq!((get(5), get(6), get(7), h.len()), (Some("A".into()), Some("B".into()), Some("\u{e9}".into()), 3));
    assert!(m.cid2unichr_v.as_ref().expect("V").is_empty());
    let cmap = CMap::new_py("X", &m).expect("a CMap");
    assert!(cmap.is_vertical());
    assert_eq!(cmap.decode_cids(b"\x01\x41\x42\x03\x41"), [7, 300, 9].map(PyInt::Small));
    let err = UnicodeMap::new_py("X", &CMapModule::parse("X", &compact(&[vertical])).unwrap(), false).unwrap_err();
    assert_eq!(err.message, "type object 'X' has no attribute 'CID2UNICHR_H'");

    // malformed files
    let bad: &[(&[u8], &str)] = &[
        (b"PMCX\x01\x00", "not a version 1 compact CMap"),
        (b"PMCM\x02\x00", "not a version 1 compact CMap"),
        (b"PMC", "truncated data"),
        (b"PMCM\x01\x00\x00", "trailing bytes"),
        (b"PMCM\x01\x01\x0bIS_VERTICALC\x00", "unexpected attribute IS_VERTICAL (tag 67)"),
        (b"PMCM\x01\x01\x02X\xffB\x00", "invalid UTF-8"),
        (b"PMCM\x01\x01\x08CODE2CIDC\x01\x80\x80\x80\x80\x80\x80\x80\x80\x80\x80\x01", "varint too long"),
        (b"PMCM\x01\x01\x08CODE2CIDC\x01\x01\x80\x80\x80\x80\x10\x00", "key out of range"),
        (b"PMCM\x01\x01\x08CODE2CIDC\x01\x04\xff\xff\xff\xff\x0f\x00", "key out of range"),
        (b"PMCM\x01\x01\x08CODE2CIDC\x01\x04\x00\xff\xff\xff\xff\xff\xff\xff\xff\xff\x01", "value out of range"),
        (b"PMCM\x01\x01\x0cCID2UNICHR_HU\x01\x04\xff\xff\xff\xff\xff\xff\xff\xff\xff\x01\x41", "value out of range"),
        (b"PMCM\x01\x01\x0cCID2UNICHR_HU\x01\x02\x00\x80\xb0\x03", "not a character"),
        (b"PMCM\x01\x01\x0cCID2UNICHR_HU\x01\x04\x00\xff\xff\x43", "not a character"),
        (b"PMCM\x01\x01\x0cCID2UNICHR_HU\x01\x04\x00\xff\xff\xff\xff\x0f", "not a character"),
        (b"PMCM\x01\x01\x0cCID2UNICHR_HU\x01\x04\x00\xff\xff\xff\xff\xff\xff\xff\xff\xff\x01", "not a character"),
        (b"PMCM\x01\x01\x0cCID2UNICHR_HU\x01\x01\x00\x02\xc3", "truncated data"),
    ];
    for (data, msg) in bad {
        assert_eq!(CMapModule::parse("X", data).unwrap_err(), *msg, "{data:?}");
    }
    let nested = |levels: usize| {
        let mut data = b"PMCM\x01\x01\x08CODE2CIDC".to_vec();
        for _ in 0..levels {
            data.extend_from_slice(b"\x01\x01\x00");
        }
        data.push(0);
        data
    };
    assert!(CMapModule::parse("X", &nested(64)).is_ok());
    assert_eq!(CMapModule::parse("X", &nested(65)).unwrap_err(), "tree too deep");
}

#[test]
fn providers() {
    use pdfminer::cmapdb::CMapProvider;
    struct Fixed(Vec<u8>);
    impl CMapProvider for Fixed {
        fn load(&self, name: &str) -> Option<Vec<u8>> {
            (name == "Corrupt").then(|| self.0.clone())
        }
    }
    // (load_data: get_cmap may have "H" in its cache, from another test of this thread)
    let load = |name: &str| match CMapDB::load_data(name) {
        Ok(_) => ("loaded".to_owned(), String::new()),
        Err(e) => (e.kind.name().to_owned(), e.message.clone()),
    };
    let not_found = |name: &str| ("CMapNotFound".to_owned(), name.to_owned());
    let corrupt = ("ValueError".to_owned(), "corrupt compact CMap \"Corrupt\": truncated data".to_owned());
    // No provider installed at all is a misconfiguration, never mistaken for an installation without
    // the data: pdffont turns CMapNotFound into an empty CMap. (Only this test touches the process
    // default; every other test installs its own thread provider with open().)
    CMapDB::clear_provider();
    CMapDB::set_default_provider(None);
    let unset = (
        "RuntimeError".to_owned(),
        "no CMap provider installed: call CMapDB::set_default_provider or CMapDB::set_provider".to_owned(),
    );
    assert_eq!(load("H"), unset);
    assert_eq!(load("../../H"), unset);
    assert!(CMapDB::get_cmap("Identity-H").is_ok());
    // the process default serves a thread without its own provider
    CMapDB::set_default_provider(Some(Arc::new(Fixed(b"PMCM".to_vec()))));
    assert_eq!((load("Corrupt"), load("H")), (corrupt.clone(), not_found("H")));
    // a thread provider overrides it; None: an installation without the data
    CMapDB::set_provider(None);
    assert_eq!((load("Corrupt"), load("H")), (not_found("Corrupt"), not_found("H")));
    CMapDB::set_provider(Some(Rc::new(Fixed(b"PMCMx".to_vec()))));
    assert_eq!(
        load("Corrupt"),
        ("ValueError".to_owned(), "corrupt compact CMap \"Corrupt\": not a version 1 compact CMap".to_owned())
    );
    // and clearing it goes back to the default
    CMapDB::clear_provider();
    assert_eq!(load("Corrupt"), corrupt);
    CMapDB::set_default_provider(None);
    assert_eq!(load("Corrupt"), unset);
    CMapDB::set_provider(Some(Rc::new(Fixed(b"PMCM".to_vec()))));
    let err = CMapDB::get_cmap("Corrupt").err().expect("corrupt data");
    assert_eq!(err.message, "corrupt compact CMap \"Corrupt\": truncated data");
    // a directory provider serves only the names it lists
    // under the cache (WASI has no temp_dir nor process id): a unique name
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let dir = cache_dir().join("tmp").join(format!("cmaps-unit-provider-{stamp}"));
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("names.txt"), "A\n").unwrap();
    fs::write(dir.join("A.pdfcmap"), b"a").unwrap();
    fs::write(dir.join("B.pdfcmap"), b"b").unwrap();
    let provider = DirCMapProvider::new(&dir).unwrap();
    assert_eq!((provider.load("A"), provider.load("B")), (Some(b"a".to_vec()), None));
    assert!(DirCMapProvider::new(dir.join("missing")).is_err());
    fs::remove_dir_all(&dir).unwrap();
}
