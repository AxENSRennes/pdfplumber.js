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
//!
//! # Mutations
//!
//! `cargo mutants -f crates/pdfminer/src/pdffont/{mod,cff,truetype,type1,
//! frames,unpack}.rs --test-package pdftrace --cargo-test-arg=--test=fonts_unit
//! --profile mutants -j 1 --timeout 20 --baseline skip` (672 mutants: 208
//! caught, 423 unviable, 4 timeouts, 37 missed), then, after new cases, the
//! functions of the survivors again (`--re`, 117 mutants + 25), `cmapdb.rs`'s
//! frame-counting functions with `cmaps_unit` too (56: all caught or
//! unviable, 1 timeout) and `pycompat::recursion`'s logging model (5); the
//! `Ok(..)` pass (`scripts/mutants-ok.sh`) of the six files (111: 61 caught,
//! 46 unviable, 4 missed, then caught by the INDEX iteration cases) and of
//! `cmapdb.rs` (46: 37 caught, 9 unviable); the unviable `Ok(..)` bodies by
//! hand with concrete values of each pertinent variant (72: 71 caught); and
//! hand mutants of the frame model, which cargo mutants does not mutate
//! (constants, removed frames and logging calls, 37: 27 caught). The
//! survivors are equivalent:
//!
//! - `|` made `^` (`getdict` 3, `unpack_be`): the bits combined never
//!   overlap (`b1 << 8 | b2` with `b2 < 256`...);
//! - `get_font`'s `"Type1" | "MMType1"` arm deleted: the default arm creates
//!   the same `PDFType1Font` (`STRICT` is off);
//! - `get_data`'s `get_any` frame removed: `resolve1`'s frame follows at the
//!   same depth;
//! - the frames of `name2unicode` in `get_encoding_counted`
//!   (`reaches_raise_key_error`, the ligature test, the `RecursionError`
//!   guard, `get_encoding_counted` without frames): at most 4 below
//!   `PDFSimpleFont.__init__`, whose `_parse_bbox` always goes to 5 or more
//!   afterwards (`safe_rect_list` -> `safe_rect` -> ... or its
//!   `log.warning`), and only the logger cache of `encodingdb`, used by no
//!   deeper call, keeps a trace; in a Type 1 program's header, below the
//!   parser's construction (7 or 8);
//! - hand mutants under the parser's construction (7 below the caller of
//!   `run`, 8 with a cold `psparser` logger): `do_keyword`'s `nextobject`
//!   `log.debug` (3-4) and the Type 1 parser's `nextobject` frame and log
//!   (3-5), `_warn_once`'s frame and `log.warning` (4-7, a tie at most);
//!   `IdentityCMap.__init__` and its evaluation loop entry in `get_cmap`
//!   (one below `get_cmap`, under `_parse_bbox`'s 4 or the parser's 7);
//!   `_load_data`'s second `log.debug` (the logger is cached by the first,
//!   2 frames under the 8 of the load); the evaluation loop entry of
//!   `CMapParser(...)` (C units only, never near their limit there).
//! - Timeouts are mutants that loop (`k += 1` made `*=`, `WidthKey::hash`
//!   constant), a format 4 `segcount` multiplied.
//!
//! After the review fixes (`get_data` holding its frames across the decode,
//! `PDFStream::decode_with`, `psbaseparser_seek` filling the `psparser`
//! logger cache, `pycompat::recursion::fill_logger_cache`): `cargo mutants`
//! on those functions (`--re 'get_data|decode_with|PDFStream::decode|
//! fill_logger_cache|psbaseparser_seek'`, the four files, `fonts_unit`,
//! `filters_unit`, `pdftypes_unit`, `psparser_base_unit`, corpus tests
//! skipped, `--timeout 60`: 41 mutants, 13 caught, 26 unviable, 2 missed),
//! their `Ok(..)` pass, and 7 hand mutants of the frame model (the
//! `fill_logger_cache` call removed; `get_data`'s `decode`, `get_data`,
//! `get_filters` and `resolve1` frames removed or dropped early; the
//! `get_filters` call moved before the decipher callback: all caught by
//! `depth`; the `get_any` leaf removed: equivalent, above). The 2 missed:
//! `decode_with`'s two `ZlibError` guards made `true`, equivalent: with
//! their constant arguments (`MAX_WBITS`, the default buffer size and
//! `max_length`), `zlib::decompress` and `decompress_corrupted` raise only
//! `ZlibError` (their `ValueError` branches check those arguments) and
//! count no frames.
//!
//! At the integration of wave 2, `psparser` and `pdftypes` count their own
//! frames (`wave2/document`): `decode_with`, `fill_logger_cache`,
//! `PSStackParser::init_frames` and the frames `pdffont` and `cmapdb` took
//! for them are gone, and the logger caches are `pycompat::logging`'s.
//! Every set passes unchanged; hand mutants re-adding one frame in
//! `get_data` or in `CMapParser.run` are caught (`depth`, `corpus_depth`),
//! one in the Type 1 parser's `do_keyword` is equivalent (above: under the
//! parser's construction).

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::rc::{Rc, Weak};

use pdfminer::cmapdb::{CMapDB, DirCMapProvider, Unichr};
use pdfminer::layout::{LTChar, LTCharFont, LTPayload};
use pdfminer::pdfdocument::{security_handler_registry, PDFStandardSecurityHandler};
use pdfminer::pdffont::cff::SidName;
use pdfminer::pdffont::{
    get_font, get_widths, get_widths2, getdict, CFFFont, FontName, PDFFont, TrueTypeFont, Type1FontHeaderParser,
    WidthKey,
};
use pdfminer::pdftypes::{int_value, DecipherCallable, PDFDict, PDFList, PDFObjRef, PDFObject, PDFResolver, PDFStream};
use pdfminer::psparser::{LIT_bytes, PSBaseParser, KWD, LIT};

use pdftrace::canon::{canon, sha256_hex};
use pdftrace::fonts_report::{
    self, call_seq, char_disp, fontname, in_frame, named, num, numkey, operand, res, tuple, umap_report, unichr,
};
use pdftrace::typed_json::{self, Json};
use pdftrace::unit::{cache_dir, UnitData};
use pdftrace::value::{NameValue, Value};
use pycompat::io::BytesIO;
use pycompat::logging::disable as clear_logger_cache;
use pycompat::recursion::{self, py_frame, Depth, C_RECURSION_LIMIT};
use pycompat::{exc, ExcKind, Num, PyErr, PyInt, PyResult};

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

/// What the objects of a case are bound to: the document their references
/// resolve through, and, for an encrypted document, the decryption callback
/// of its streams (the generator's `FontDoc.decipher`).
struct Binder {
    doc: Weak<dyn PDFResolver>,
    decipher: Option<DecipherCallable>,
}

/// `security_handler(cipher)` of the generator: the handler of `[docid,
/// param, password]`, built as `PDFDocument._initialize_password` builds
/// it; its `decrypt`.
fn security_handler(cipher: &Value, doc: &Weak<dyn PDFResolver>) -> DecipherCallable {
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
fn load_doc(table: &Value, errors: &Value, cipher: Option<&Value>) -> (Rc<FontDoc>, Binder) {
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
fn build(v: &Value, doc: &Binder) -> PDFObject {
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

// -- reports ---------------------------------------------------------------------------

/// `font_attrs(font)` of the generator (`pdftrace::fonts_report`), and the
/// check of what `LTChar.__init__` reads of the font.
fn font_attrs(font: &PDFFont) -> Value {
    let report = fonts_report::font_attrs(font);
    check_ltchar_font(font);
    report
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

/// `LTChar(..., font, ..., text=font.to_unichr(cid), ...)` (what
/// `PDFLayoutAnalyzer.render_char` builds): the char's text is the font's
/// (`Unichr` into the layout's `PyStr`, a lone surrogate included), which
/// the report compares with Python. A cid whose `char_disp` fails has no
/// `LTChar` (`render_char` raises before building it); an `LTChar` that
/// cannot be built must be a vertical font's (a displacement that is not a
/// number) or a font whose `get_descent` fails. Each outcome is counted in
/// `LTCHAR_TEXT`, and the font tests require a minimum of checked chars and
/// of lone surrogates among them (`check_ltchar_counts`).
fn check_ltchar_text(font: &PDFFont, cid: &PyInt, u: &Unichr) {
    let Ok(textdisp) = font.char_disp(cid) else {
        LTCHAR_TEXT.with(|c| c.borrow_mut().no_disp += 1);
        return;
    };
    let (zero, one) = (Num::Int(PyInt::ZERO), Num::Int(PyInt::Small(1)));
    let matrix = [one.clone(), zero.clone(), zero.clone(), one.clone(), zero.clone(), zero.clone()];
    let gray = pdfminer::pdfcolor::predefined_colorspace("DeviceGray").expect("DeviceGray");
    let built = recursion::with_depth(Depth::default(), || {
        LTChar::<FP>::new(matrix, font, Num::Float(1.0), one, zero.clone(), u.clone(), zero, textdisp, gray, ())
    });
    match built {
        Ok(ch) => {
            assert_eq!(canon(&Value::py_str(ch.get_text())), canon(&unichr(u)), "LTChar text of cid {cid}");
            LTCHAR_TEXT.with(|c| {
                let mut c = c.borrow_mut();
                c.checked += 1;
                if matches!(u, Unichr::Surrogate(_)) {
                    c.surrogates += 1;
                }
            });
        }
        Err(e) => {
            assert!(
                font.is_vertical() || font.get_descent().is_err(),
                "LTChar of cid {cid} of a horizontal font with a descent not built: {e:?}"
            );
            LTCHAR_TEXT.with(|c| c.borrow_mut().not_built += 1);
        }
    }
}

/// Outcomes of `check_ltchar_text` in the current test (each test runs in
/// its own thread).
#[derive(Default, Debug)]
struct LtcharCounts {
    checked: usize,
    surrogates: usize,
    no_disp: usize,
    not_built: usize,
}

thread_local! {
    static LTCHAR_TEXT: RefCell<LtcharCounts> = RefCell::default();
}

/// Requires that the replay checked at least `min_checked` `LTChar` texts,
/// `min_surrogates` of them lone surrogates (floors below the counts of the
/// current data, like the `min_cases` of `replay_canon`).
fn check_ltchar_counts(name: &str, min_checked: usize, min_surrogates: usize) {
    let c = LTCHAR_TEXT.with(|c| std::mem::take(&mut *c.borrow_mut()));
    println!("{name}: LTChar text {c:?}");
    assert!(c.checked >= min_checked, "{name}: only {} LTChar texts checked ({c:?})", c.checked);
    assert!(c.surrogates >= min_surrogates, "{name}: only {} lone surrogates checked ({c:?})", c.surrogates);
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
                res(font.to_unichr(cid), |u| {
                    check_ltchar_text(font, cid, &u);
                    unichr(&u)
                }),
                call_seq(|| font.char_width(cid)),
                res(font.char_disp(cid), |d| char_disp(&d)),
            ])
        })
        .collect();
    Value::List(vec![Value::List(out_strings), Value::List(out_cids)])
}

/// `run_font(...)` of a font case `[spec, table, errors, strings, probes,
/// depth, query depth, cipher]`.
fn run_font(a: &[Value]) -> Vec<Value> {
    clear_logger_cache();
    let (_doc, binder) = load_doc(&a[1], &a[2], a.get(7));
    let PDFObject::Dict(spec) = build(&a[0], &binder) else { panic!("a dict spec") };
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
        let (_doc, binder) = load_doc(&a[2], &a[3], None);
        let PDFObject::List(seq) = build(&a[1], &binder) else { panic!("a list") };
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
    // `list(index)` (`INDEX.__iter__`), under `attempt`
    let iters = Value::List(
        indexes
            .iter()
            .map(|ix| res(ix.items(&mut font.fp), |items| Value::List(items.into_iter().map(Value::Bytes).collect())))
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
            ("iters", iters),
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
    check_ltchar_counts("fonts", 40_000, 100);
}

#[test]
fn corpus_fonts() {
    let data = open();
    check_marker(&data, "corpus");
    replay_canon(&data, "corpus_fonts", 1000, run_font);
    check_ltchar_counts("corpus_fonts", 100_000, 20);
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

/// Replays a set of recursion boundaries (`font_depth_cases` of the
/// generator): `[spec, table, errors, op, arg, depth, depth + 1, cipher]`,
/// the outcome at both depths.
fn replay_depth(data: &UnitData, name: &str, min_cases: usize) {
    // the font of the queries, built once per font dictionary
    let mut built: Option<(String, Rc<FontDoc>, Option<PDFFont>)> = None;
    replay_canon(data, name, min_cases, |a| {
        let op = text(&a[3]);
        let cipher = a.get(7);
        // the generator builds the font once first (which loads the
        // predefined CMaps it uses), then searches the boundaries
        let key = format!("{:?}{:?}{:?}{:?}", a[0], a[1], a[2], cipher);
        if built.as_ref().is_none_or(|(k, _, _)| *k != key) {
            clear_logger_cache();
            let (doc, binder) = load_doc(&a[1], &a[2], cipher);
            let PDFObject::Dict(spec) = build(&a[0], &binder) else { panic!("a dict spec") };
            let font = get_font(&spec).ok();
            built = Some((key, doc, font));
        }
        if op == "get_font" || op == "get_font_cold" || op == "get_font@parsed" {
            return [&a[5], &a[6]]
                .iter()
                .map(|d| {
                    if op == "get_font_cold" {
                        // clear_cmap_caches() of the generator: first loads
                        CMapDB::clear_caches();
                    }
                    clear_logger_cache();
                    if op == "get_font@parsed" {
                        // a parser created first, as a document's is
                        PSBaseParser::init(Some(BytesIO::new(Rc::from(&b""[..]))), ()).expect("a parser");
                    }
                    let (_doc, binder) = load_doc(&a[1], &a[2], cipher);
                    let PDFObject::Dict(spec) = build(&a[0], &binder) else { panic!("a dict spec") };
                    let font = recursion::with_depth(depth(d), || get_font(&spec));
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
                    let (_doc, binder) = load_doc(&a[1], &a[2], cipher);
                    let PDFObject::Dict(spec) = build(&a[0], &binder) else { panic!("a dict spec") };
                    let fresh = get_font(&spec).expect("a font for the queries");
                    res(recursion::with_depth(depth(d), || query_call(&fresh, op, &a[4])), |v| v)
                })
                .collect();
        }
        let font = built.as_ref().and_then(|(_, _, f)| f.as_ref()).expect("a font for the queries");
        [&a[5], &a[6]].iter().map(|d| res(at(d, || query_call(font, op, &a[4])), |v| v)).collect()
    });
}

#[test]
fn depth_boundaries() {
    let data = open();
    replay_depth(&data, "depth", 100);
}

/// The boundaries of the first fonts of the encrypted documents of the set
/// `all` (their streams deciphered by the document's handler).
#[test]
fn corpus_depth() {
    let data = open();
    check_marker(&data, "corpus");
    replay_depth(&data, "corpus_depth", 20);
}
