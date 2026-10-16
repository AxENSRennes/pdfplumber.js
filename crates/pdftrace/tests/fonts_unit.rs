//! Replays the cases of `oracle/unit/fonts.py` (pdfminer results) through
//! `pdfminer::pdffont`.
//!
//! Every output is compared through its canonical text (the text, or
//! `sha256:<hex>:<length>` when it is long); `python -m oracle.unit.fonts
//! --dump <set> <index>` prints Python's for a generated set. With
//! `FONTS_DUMP_DIR` set, the Rust text of each differing case is written
//! there as `<set>-<index>.txt`.
//!
//! The cases of a set run in their order on one thread, from the recursion
//! depth pdfminer ran them at, with the logger caches cleared where the
//! generator cleared them (`logging.disable`), as `pdffont::frames` models
//! them.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::rc::{Rc, Weak};

use pdfminer::casting::IterItem;
use pdfminer::cmapdb::{AttrValue, CMapBase, CMapDB, DirCMapProvider, Unichr, UnicodeMap};
use pdfminer::layout::{CharDisp, LTCharFont, LTPayload};
use pdfminer::pdffont::cff::SidName;
use pdfminer::pdffont::frames::clear_logger_cache;
use pdfminer::pdffont::{
    get_font, get_widths, get_widths2, getdict, CFFFont, FontKind, FontName, PDFFont, TrueTypeFont,
    Type1FontHeaderParser, WidthKey,
};
use pdfminer::pdftypes::{PDFDict, PDFList, PDFObjRef, PDFObject, PDFResolver, PDFStream};
use pdfminer::psparser::{LIT_bytes, KWD, LIT};
use pdfminer::utils::{Operand, SeqKind};
use pdftrace::canon::{canon, sha256_hex};
use pdftrace::typed_json::{self, Json};
use pdftrace::unit::{cache_dir, UnitData};
use pdftrace::value::{NameValue, PyStr, Value};
use pycompat::io::BytesIO;
use pycompat::recursion::{self, py_frame, Depth, C_RECURSION_LIMIT};
use pycompat::{exc, ExcKind, Num, NumKey, PyErr, PyInt, PyResult};

/// Python's `INLINE_LIMIT`: longer outputs are compared by SHA-256.
const INLINE_LIMIT: usize = 4000;

/// Opens the data and installs the provider of the predefined CMaps.
fn open() -> UnitData {
    let data = UnitData::open("fonts");
    let meta = fs::read_to_string(data.path("meta.json")).expect("meta.json");
    let meta = typed_json::parse_json(&meta).expect("meta.json is JSON");
    let Some(Json::String(rel)) = meta.get("cmap_dir") else { panic!("meta.json has no cmap_dir") };
    let dir = cache_dir().join(rel);
    let provider = DirCMapProvider::new(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
    CMapDB::set_provider(Some(Rc::new(provider)));
    data
}

/// Checks the marker `<name>.json` that the generator writes after a run.
fn check_marker(data: &UnitData, name: &str) {
    let path = data.path(&format!("{name}.json"));
    let text = fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "missing {}: generate it with python -m oracle.unit.fonts{}",
            path.display(),
            if name == "corpus" { " --corpus" } else { "" }
        )
    });
    let marker = typed_json::parse_json(&text).expect("marker JSON");
    data.check_sources(&path, marker.get("sources"));
}

// -- documents and objects -----------------------------------------------------------

/// The generator's `DEEP_OBJIDS`, `DEEP_GETOBJ_CALLS`.
const DEEP_OBJIDS: i64 = 200_000;
const DEEP_GETOBJ_CALLS: u32 = 8;

/// The generator's `FontDoc`: a table of objects, and the exceptions of the
/// object numbers whose loading raised.
#[derive(Default)]
struct FontDoc {
    table: RefCell<HashMap<PyInt, PDFObject>>,
    errors: RefCell<HashMap<PyInt, PyErr>>,
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

/// A document of `table` and `errors` (the case's dumps).
fn load_doc(table: &Value, errors: &Value) -> (Rc<FontDoc>, Weak<dyn PDFResolver>) {
    let doc = Rc::new(FontDoc::default());
    let dyn_doc: Rc<dyn PDFResolver> = doc.clone();
    let weak = Rc::downgrade(&dyn_doc);
    for pair in list(table) {
        let p = list(pair);
        doc.table.borrow_mut().insert(int(&p[0]), build(&p[1], &weak));
    }
    for e in list(errors) {
        let e = list(e);
        let name = text(&e[1]);
        let kind = ExcKind::ALL.iter().copied().find(|k| k.trace_name() == name).unwrap_or_else(|| panic!("{name}"));
        doc.errors.borrow_mut().insert(int(&e[0]), PyErr::new(kind, text(&e[3])));
    }
    (doc, weak)
}

/// The PDF object of a typed value, its references bound to `doc`.
fn build(v: &Value, doc: &Weak<dyn PDFResolver>) -> PDFObject {
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
        Value::Ref(objid) => PDFObject::Ref(PDFObjRef::new(Some(doc.clone()), objid.clone())),
        Value::Stream(sv) => {
            let PDFObject::Dict(attrs) = build(&Value::Dict(sv.attrs.clone()), doc) else { unreachable!() };
            let Value::Bytes(raw) = &sv.raw else { panic!("a stream not decoded yet: {sv:?}") };
            let s = PDFStream::new(attrs, Rc::from(&raw[..]), None);
            if let (Value::Int(objid), Value::Int(genno)) = (&sv.objid, &sv.genno) {
                s.set_objid(objid.clone(), genno.clone());
            }
            PDFObject::Stream(Rc::new(s))
        }
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
        Value::List(items) | Value::Tuple(items) => items,
        other => panic!("not a list: {other:?}"),
    }
}

/// The `{"py": .., "c_left": ..}` depth of a case.
fn depth(v: &Value) -> Depth {
    let Value::Dict(items) = v else { panic!("a depth: {v:?}") };
    let field = |name: &str| {
        let v = items.iter().find(|(k, _)| k.as_str() == Some(name)).map(|(_, v)| int(v)).expect("depth");
        u32::try_from(v.as_i64().expect("small")).expect("a depth")
    };
    Depth { py: field("py"), c: C_RECURSION_LIMIT - field("c_left") }
}

// -- values of results -----------------------------------------------------------

fn num(n: &Num) -> Value {
    match n {
        Num::Int(i) => Value::Int(i.clone()),
        Num::Float(f) => Value::Float(*f),
    }
}

/// `seqval(x)` of the generator.
fn operand(x: &Operand) -> Value {
    match x {
        Operand::Num(n) => num(n),
        Operand::Bool(b) => Value::Bool(*b),
        Operand::Seq(kind, len) => Value::Named {
            name: "seq".into(),
            fields: vec![
                (Value::str("type"), Value::str(if *kind == SeqKind::Bytes { "bytes" } else { "list" })),
                (Value::str("len"), Value::Int(PyInt::Small(i64::try_from(*len).expect("a length")))),
            ],
        },
        Operand::Other(name) => panic!("an operand of type {name} is never a result"),
    }
}

fn unichr(u: &Unichr) -> Value {
    match u {
        Unichr::Text(s) => Value::str(s.to_string()),
        Unichr::Surrogate(c) => Value::Str(PyStr::from_code_points(vec![*c]).expect("a surrogate")),
    }
}

fn item(x: &IterItem) -> Value {
    match x {
        IterItem::Obj(o) => Value::from(o),
        IterItem::Str(s) => Value::str(s.to_string()),
    }
}

fn numkey(k: &NumKey) -> Value {
    num(k.num())
}

fn attr(a: &AttrValue) -> Value {
    match a {
        AttrValue::Str(s) => Value::str(s.clone()),
        AttrValue::Obj(o) => Value::from(o),
    }
}

fn attrs(m: &dyn CMapBase) -> Value {
    Value::Dict(m.attrs().iter().map(|(k, v)| (Value::str(k.clone()), attr(v))).collect())
}

/// A result, or the exception.
fn res<T>(r: PyResult<T>, f: impl FnOnce(T) -> Value) -> Value {
    match r {
        Ok(v) => f(v),
        Err(e) => Value::from(&e),
    }
}

fn named(name: &str, fields: Vec<(&str, Value)>) -> Value {
    Value::Named { name: name.to_owned(), fields: fields.into_iter().map(|(k, v)| (Value::str(k), v)).collect() }
}

fn tuple(items: Vec<Value>) -> Value {
    Value::Tuple(items)
}

/// `canon_or_hash(v)` of the generator.
fn canon_or_hash(v: &Value) -> String {
    let text = canon(v);
    if text.len() <= INLINE_LIMIT {
        return text;
    }
    format!("sha256:{}:{}", sha256_hex(text.as_bytes()), text.len())
}

/// Replays `<name>.jsonl`, whose outputs are `canon_or_hash` texts (or a
/// list of them).
fn replay_canon(data: &UnitData, name: &str, min_cases: usize, mut f: impl FnMut(&[Value]) -> Vec<Value>) {
    let cases = data.cases(name);
    assert!(cases.len() >= min_cases, "{name}: only {} cases", cases.len());
    let dump_dir = std::env::var_os("FONTS_DUMP_DIR");
    let mut failures = Vec::new();
    for (k, (args, expected)) in cases.iter().enumerate() {
        let expected: Vec<&str> = match expected {
            Value::List(items) => items.iter().map(|x| x.as_str().expect("a text")).collect(),
            other => vec![other.as_str().expect("a text")],
        };
        let actual = f(args);
        let got: Vec<String> = actual.iter().map(canon_or_hash).collect();
        if got.iter().map(String::as_str).ne(expected.iter().copied()) {
            let shown = |s: &str| s.chars().take(700).collect::<String>();
            let input = shown(&format!("{args:?}"));
            failures.push(format!(
                "#{k} {input}\n    python: {}\n    rust:   {}",
                shown(&expected.join(" | ")),
                shown(&got.join(" | "))
            ));
            if let Some(dir) = &dump_dir {
                fs::create_dir_all(dir).unwrap();
                let text: Vec<String> = actual.iter().map(canon).collect();
                fs::write(std::path::Path::new(dir).join(format!("{name}-{k}.txt")), text.join("\n")).unwrap();
            }
        }
    }
    println!("{name}: {}/{} cases identical", cases.len() - failures.len(), cases.len());
    assert!(
        failures.is_empty(),
        "{name}: {}/{} cases differ (python -m oracle.unit.fonts --dump {name} <index>); first ones:\n{}",
        failures.len(),
        cases.len(),
        failures.iter().take(6).cloned().collect::<Vec<_>>().join("\n")
    );
}

/// Runs `f` from the depth of a case, its logger caches cleared.
fn at<T>(d: &Value, f: impl FnOnce() -> T) -> T {
    clear_logger_cache();
    recursion::with_depth(depth(d), f)
}

/// `at_depth(f, ...)` of the generator: `f` (a Python function) takes its
/// frame.
fn in_frame<T>(f: impl FnOnce() -> PyResult<T>) -> PyResult<T> {
    let _frame = py_frame()?;
    f()
}

// -- reports ---------------------------------------------------------------------------

fn umap_report(m: Option<&UnicodeMap>) -> Value {
    let Some(m) = m else { return Value::None };
    named(
        m.class_name(),
        vec![
            ("attrs", attrs(m)),
            (
                "items",
                Value::List(m.cid2unichr.iter().map(|(k, v)| tuple(vec![Value::Int(k.clone()), unichr(v)])).collect()),
            ),
        ],
    )
}

fn cmap_report(m: &dyn CMapBase) -> Value {
    let mut fields = vec![("attrs", attrs(m))];
    if let Some(c) = m.as_cmap() {
        fields.push(("code2cid", Value::Int(PyInt::Small(c.code2cid.len() as i64))));
    }
    named(m.class_name(), fields)
}

fn fontname(n: &FontName) -> Value {
    match n {
        FontName::Str(s) => Value::str(s.clone()),
        FontName::Obj(o) => Value::from(o),
    }
}

fn char_disp(d: &CharDisp) -> Value {
    match d {
        CharDisp::Scalar(n) => num(n),
        CharDisp::Vertical(vx, vy) => tuple(vec![item(vx), item(vy)]),
    }
}

/// A method called through `call_seq` (one more frame: `attempt`'s).
fn call_seq(f: impl FnOnce() -> PyResult<Operand>) -> Value {
    res(in_frame(f), |x| operand(&x))
}

/// `font_attrs(font)` of the generator.
fn font_attrs(font: &PDFFont) -> Value {
    let mut fields: Vec<(&str, Value)> = Vec::new();
    let basefont = match &font.kind {
        FontKind::Simple(s) => s.basefont.clone(),
        FontKind::CID(c) => Some(c.basefont.clone()),
    };
    if let Some(b) = basefont {
        fields.push(("basefont", Value::str(b)));
    }
    fields.push(("fontname", fontname(&font.fontname)));
    fields.push(("flags", Value::from(&font.flags)));
    fields.push(("ascent", operand(&font.ascent)));
    fields.push(("descent", operand(&font.descent)));
    fields.push(("italic_angle", operand(&font.italic_angle)));
    fields.push(("default_width", item(&font.default_width)));
    fields.push(("leading", operand(&font.leading)));
    fields.push(("bbox", tuple(font.bbox.iter().map(|&f| Value::Float(f)).collect())));
    fields.push(("hscale", operand(&font.hscale)));
    fields.push(("vscale", operand(&font.vscale)));
    let widths = font
        .widths
        .iter()
        .map(|(k, v)| {
            let k = match k {
                WidthKey::Num(n) => numkey(n),
                WidthKey::Str(s) => Value::str(s.to_string()),
            };
            tuple(vec![k, Value::from(v)])
        })
        .collect();
    fields.push(("widths", Value::List(widths)));
    match &font.kind {
        FontKind::Simple(s) => {
            let items = s.cid2unicode.iter().map(|(k, v)| tuple(vec![Value::Int(k.clone()), Value::str(v.clone())]));
            fields.push(("cid2unicode", Value::List(items.collect())));
            fields.push(("unicode_map", umap_report(s.unicode_map.as_deref())));
            if s.fontfile.is_some() {
                fields.push(("fontfile", Value::Bool(true)));
            }
            if let Some(m) = &s.matrix {
                fields.push(("matrix", tuple(m.iter().map(Value::from).collect())));
            }
        }
        FontKind::CID(c) => {
            fields.push(("unicode_map", umap_report(c.unicode_map.as_deref())));
            if c.fontfile.is_some() {
                fields.push(("fontfile", Value::Bool(true)));
            }
            fields.push(("cidcoding", Value::str(c.cidcoding.clone())));
            fields.push(("cmap", cmap_report(&*c.cmap)));
            fields.push(("vertical", Value::Bool(c.vertical)));
            let disps = c
                .disps
                .iter()
                .map(|(k, (vx, vy))| tuple(vec![numkey(k), tuple(vec![Value::from(vx), Value::from(vy)])]));
            fields.push(("disps", Value::List(disps.collect())));
            fields.push((
                "default_disp",
                match &c.default_disp {
                    pdfminer::pdffont::DefaultDisp::Zero => Value::Int(PyInt::ZERO),
                    pdfminer::pdffont::DefaultDisp::Vertical(vy) => tuple(vec![Value::None, item(vy)]),
                },
            ));
        }
    }
    fields.push(("is_vertical", Value::Bool(font.is_vertical())));
    fields.push(("is_multibyte", Value::Bool(font.is_multibyte())));
    fields.push(("repr", Value::str(font.repr())));
    check_ltchar_font(font);
    // called through call_seq from font_attrs, at the depth of the queries
    fields.push(("get_ascent", call_seq(|| font.get_ascent())));
    fields.push(("get_descent", call_seq(|| font.get_descent())));
    fields.push(("get_width", call_seq(|| font.get_width())));
    fields.push(("get_height", call_seq(|| font.get_height())));
    named(font.class_name(), fields)
}

/// A layout payload whose font name is the font's.
#[derive(Clone)]
struct FP;

impl LTPayload for FP {
    type FontName = FontName;
    type GraphicState = ();
    type Color = ();
    type Attr = ();
}

/// What `LTChar.__init__` reads of a font (`LTCharFont`) is what the
/// report shows (`fontname`, `is_vertical()`, `get_descent()`, compared
/// with Python there).
fn check_ltchar_font(font: &PDFFont) {
    let name: FontName = LTCharFont::<FP>::fontname(font);
    assert_eq!(canon(&fontname(&name)), canon(&fontname(&font.fontname)), "LTCharFont::fontname");
    assert_eq!(LTCharFont::<FP>::is_vertical(font), font.is_vertical(), "LTCharFont::is_vertical");
    let direct = res(font.get_descent(), |x| operand(&x));
    let via = res(LTCharFont::<FP>::get_descent(font), |n| num(&n));
    assert_eq!(canon(&via), canon(&direct), "LTCharFont::get_descent");
}

/// `decode_loop(font, s)` called through `attempt`.
fn decode_loop(font: &PDFFont, s: &[u8]) -> PyResult<Vec<PyInt>> {
    in_frame(|| font.decode(s))
}

/// `font_queries(font, strings, probes)` of the generator, from the depth
/// of its calls.
fn font_queries(font: &PDFFont, strings: &[Value], probes: &[Value]) -> Value {
    let mut out_strings = Vec::new();
    let mut cids: Vec<PyInt> = Vec::new();
    for s in strings {
        let s = bytes(s);
        let decoded = decode_loop(font, s);
        if let Ok(d) = &decoded {
            for c in d {
                if !cids.contains(c) {
                    cids.push(c.clone());
                }
            }
        }
        out_strings.push(Value::List(vec![
            Value::Bytes(s.to_vec()),
            res(decoded, |d| Value::List(d.into_iter().map(Value::Int).collect())),
            call_seq(|| font.string_width(s)),
        ]));
    }
    for p in probes {
        let p = int(p);
        if !cids.contains(&p) {
            cids.push(p);
        }
    }
    let out_cids = cids
        .iter()
        .map(|cid| {
            Value::List(vec![
                Value::Int(cid.clone()),
                res(font.to_unichr(cid), |u| unichr(&u)),
                call_seq(|| font.char_width(cid)),
                res(font.char_disp(cid), |d| char_disp(&d)),
            ])
        })
        .collect();
    Value::List(vec![Value::List(out_strings), Value::List(out_cids)])
}

/// `run_font(...)` of a font case `[spec, table, errors, strings, probes,
/// depth, query depth]`.
fn run_font(a: &[Value]) -> Vec<Value> {
    clear_logger_cache();
    let (_doc, weak) = load_doc(&a[1], &a[2]);
    let PDFObject::Dict(spec) = build(&a[0], &weak) else { panic!("a dict spec") };
    let font = at(&a[5], || get_font(&spec));
    let out = match font {
        Err(e) => Value::from(&e),
        Ok(font) => {
            let attrs = recursion::with_depth(depth(&a[6]), || font_attrs(&font));
            let queries = recursion::with_depth(depth(&a[6]), || font_queries(&font, list(&a[3]), list(&a[4])));
            Value::List(vec![attrs, queries])
        }
    };
    vec![out]
}

// -- unit sets -------------------------------------------------------------------------

#[test]
fn widths() {
    let data = open();
    replay_canon(&data, "widths", 1000, |a| {
        let (_doc, weak) = load_doc(&a[2], &a[3]);
        let PDFObject::List(seq) = build(&a[1], &weak) else { panic!("a list") };
        let out = at(&a[4], || match text(&a[0]) {
            "get_widths" => get_widths(&seq).map(|w| {
                w.iter()
                    .map(|(k, v)| {
                        let k = match k {
                            WidthKey::Num(n) => numkey(n),
                            WidthKey::Str(s) => Value::str(s.to_string()),
                        };
                        tuple(vec![k, Value::from(v)])
                    })
                    .collect::<Vec<_>>()
            }),
            _ => get_widths2(&seq).map(|w| {
                w.iter()
                    .map(|(k, (w, (vx, vy)))| {
                        tuple(vec![
                            numkey(k),
                            tuple(vec![Value::from(w), tuple(vec![Value::from(vx), Value::from(vy)])]),
                        ])
                    })
                    .collect()
            }),
        });
        vec![res(out, Value::List)]
    });
}

#[test]
fn getdict_cases() {
    let data = open();
    replay_canon(&data, "getdict", 1000, |a| {
        let out = at(&a[1], || getdict(bytes(&a[0])));
        vec![res(out, |d| {
            Value::List(
                d.iter()
                    .map(|(k, v)| {
                        tuple(vec![Value::Int(PyInt::Small(i64::from(*k))), Value::List(v.iter().map(num).collect())])
                    })
                    .collect(),
            )
        })]
    });
}

fn sidname(n: &SidName) -> Value {
    match n {
        SidName::Str(s) => Value::str(*s),
        SidName::Bytes(b) => Value::Bytes(b.clone()),
    }
}

fn int_pairs<'a>(m: impl IntoIterator<Item = (&'a i64, &'a i64)>) -> Value {
    Value::List(
        m.into_iter().map(|(k, v)| tuple(vec![Value::Int(PyInt::Small(*k)), Value::Int(PyInt::Small(*v))])).collect(),
    )
}

/// `cff_report(font)` of the generator.
fn cff_report(mut font: CFFFont) -> Value {
    let indexes = [&font.name_index, &font.dict_index, &font.string_index, &font.subr_index, &font.charstring];
    let index_fields = Value::List(
        indexes
            .iter()
            .map(|ix| {
                tuple(vec![
                    Value::List(ix.offsets.iter().cloned().map(Value::Int).collect()),
                    Value::Int(PyInt::Small(ix.base)),
                ])
            })
            .collect(),
    );
    let top_dict = Value::List(
        font.top_dict
            .iter()
            .map(|(k, v)| {
                tuple(vec![Value::Int(PyInt::Small(i64::from(*k))), Value::List(v.iter().map(num).collect())])
            })
            .collect(),
    );
    let name2gid =
        Value::List(font.name2gid.iter().map(|(k, v)| tuple(vec![sidname(k), Value::Int(PyInt::Small(*v))])).collect());
    let gid2name =
        Value::List(font.gid2name.iter().map(|(k, v)| tuple(vec![Value::Int(PyInt::Small(*k)), sidname(v)])).collect());
    let (code2gid, gid2code) = (int_pairs(&font.code2gid), int_pairs(&font.gid2code));
    let indexes: Vec<_> = [&font.name_index, &font.dict_index, &font.string_index, &font.subr_index, &font.charstring]
        .into_iter()
        .cloned()
        .collect();
    // `[index[i] for i in range(len(index))]` through a lambda, under
    // `attempt`
    let items = Value::List(
        indexes
            .iter()
            .map(|ix| {
                res(
                    in_frame(|| {
                        in_frame(|| (0..ix.len()).map(|i| ix.get(&mut font.fp, i)).collect::<PyResult<Vec<_>>>())
                    }),
                    |items| Value::List(items.into_iter().map(Value::Bytes).collect()),
                )
            })
            .collect(),
    );
    named(
        "CFFFont",
        vec![
            ("name", Value::str(font.name.clone())),
            ("indexes", index_fields),
            ("top_dict", top_dict),
            ("nglyphs", Value::Int(PyInt::Small(font.nglyphs as i64))),
            ("code2gid", code2gid),
            ("gid2code", gid2code),
            ("name2gid", name2gid),
            ("gid2name", gid2name),
            ("items", items),
            ("reprs", Value::List(indexes.iter().map(|ix| Value::str(ix.repr())).collect())),
            ("tell", Value::Int(PyInt::Small(font.fp.tell()))),
        ],
    )
}

/// `cff_case(data)` of the generator.
fn cff_case(a: &[Value]) -> Vec<Value> {
    let font = at(&a[1], || CFFFont::new("F", BytesIO::new(Rc::from(bytes(&a[0])))));
    vec![res(font, cff_report)]
}

#[test]
fn cff() {
    let data = open();
    replay_canon(&data, "cff", 1000, cff_case);
}

/// `tt_case(data)` of the generator.
fn tt_case(a: &[Value]) -> Vec<Value> {
    let font = at(&a[1], || TrueTypeFont::new("F", BytesIO::new(Rc::from(bytes(&a[0])))));
    let mut font = match font {
        Err(e) => return vec![Value::from(&e)],
        Ok(font) => font,
    };
    let report = named(
        "TrueTypeFont",
        vec![
            ("name", Value::str(font.name.clone())),
            ("fonttype", Value::Bytes(font.fonttype.clone())),
            (
                "tables",
                Value::List(
                    font.tables
                        .iter()
                        .map(|(k, (o, l))| {
                            tuple(vec![
                                Value::Bytes(k.clone()),
                                tuple(vec![Value::Int(PyInt::Small(*o)), Value::Int(PyInt::Small(*l))]),
                            ])
                        })
                        .collect(),
                ),
            ),
        ],
    );
    let umap = recursion::with_depth(depth(&a[1]), || font.create_unicode_map());
    vec![Value::List(vec![report, res(umap, |m| umap_report(Some(&m)))])]
}

#[test]
fn truetype() {
    let data = open();
    replay_canon(&data, "truetype", 1000, tt_case);
}

/// `type1_encoding(data)` of the generator.
fn type1_encoding(data: &[u8]) -> PyResult<pdfminer::encodingdb::Cid2Unicode> {
    in_frame(|| {
        let mut parser = {
            let (_entry, _init) = pdfminer::pdffont::frames::c_called()?;
            Type1FontHeaderParser::new(BytesIO::new(Rc::from(data)))?
        };
        parser.get_encoding()
    })
}

fn type1_case(a: &[Value]) -> Vec<Value> {
    let out = at(&a[1], || type1_encoding(bytes(&a[0])));
    vec![res(out, |m| {
        Value::List(m.iter().map(|(k, v)| tuple(vec![Value::Int(k.clone()), Value::str(v.clone())])).collect())
    })]
}

#[test]
fn type1() {
    let data = open();
    replay_canon(&data, "type1", 1000, type1_case);
}

// -- fonts -------------------------------------------------------------------------------

#[test]
fn fonts() {
    let data = open();
    check_marker(&data, "generated");
    replay_canon(&data, "fonts", 3000, run_font);
}

#[test]
fn corpus_fonts() {
    let data = open();
    check_marker(&data, "corpus");
    replay_canon(&data, "corpus_fonts", 1000, run_font);
}

#[test]
fn corpus_programs() {
    let data = open();
    check_marker(&data, "corpus");
    replay_canon(&data, "corpus_programs", 500, |a| match text(&a[0]) {
        "type1" => type1_case(&a[1..]),
        "truetype" => tt_case(&a[1..]),
        "cff" => cff_case(&a[1..]),
        other => panic!("unknown program kind {other}"),
    });
}

/// One query of the set `depth` (`query_call` of the generator, a
/// function: its frame).
fn query_call(font: &PDFFont, op: &str, arg: &Value) -> PyResult<Value> {
    in_frame(|| {
        Ok(match op {
            "decode" => Value::List(decode_loop(font, bytes(arg))?.into_iter().map(Value::Int).collect()),
            "string_width" => operand(&font.string_width(bytes(arg))?),
            "to_unichr" => unichr(&font.to_unichr(&int(arg))?),
            "char_width" => operand(&font.char_width(&int(arg))?),
            "char_disp" => char_disp(&font.char_disp(&int(arg))?),
            "get_descent" => operand(&font.get_descent()?),
            "get_width" => operand(&font.get_width()?),
            other => panic!("unknown query {other}"),
        })
    })
}

#[test]
fn depth_boundaries() {
    let data = open();
    // the font of the queries, built once per font dictionary
    let mut built: Option<(String, Rc<FontDoc>, Option<PDFFont>)> = None;
    replay_canon(&data, "depth", 100, |a| {
        let op = text(&a[3]);
        // the generator builds the font once first (which loads the
        // predefined CMaps it uses), then searches the boundaries
        let key = format!("{:?}{:?}{:?}", a[0], a[1], a[2]);
        if built.as_ref().is_none_or(|(k, _, _)| *k != key) {
            clear_logger_cache();
            let (doc, weak) = load_doc(&a[1], &a[2]);
            let PDFObject::Dict(spec) = build(&a[0], &weak) else { panic!("a dict spec") };
            let font = get_font(&spec).ok();
            built = Some((key, doc, font));
        }
        if op == "get_font" || op == "get_font_cold" {
            return [&a[5], &a[6]]
                .iter()
                .map(|d| {
                    if op == "get_font_cold" {
                        // clear_cmap_caches() of the generator: first loads
                        CMapDB::clear_caches();
                    }
                    let (_doc, weak) = load_doc(&a[1], &a[2]);
                    let PDFObject::Dict(spec) = build(&a[0], &weak) else { panic!("a dict spec") };
                    let font = at(d, || get_font(&spec));
                    res(font, |f| font_attrs(&f))
                })
                .collect();
        }
        if let Some(op) = op.strip_suffix("@build") {
            // the query right after a new creation of the font, the logger
            // caches as the creation left them
            return [&a[5], &a[6]]
                .iter()
                .map(|d| {
                    clear_logger_cache();
                    let (_doc, weak) = load_doc(&a[1], &a[2]);
                    let PDFObject::Dict(spec) = build(&a[0], &weak) else { panic!("a dict spec") };
                    let fresh = get_font(&spec).expect("a font for the queries");
                    res(recursion::with_depth(depth(d), || query_call(&fresh, op, &a[4])), |v| v)
                })
                .collect();
        }
        let font = built.as_ref().and_then(|(_, _, f)| f.as_ref()).expect("a font for the queries");
        [&a[5], &a[6]].iter().map(|d| res(at(d, || query_call(font, op, &a[4])), |v| v)).collect()
    });
}
