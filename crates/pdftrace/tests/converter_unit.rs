//! Replays the cases of `oracle/unit/converter.py` through
//! `pdfminer::converter` and `pdfplumber::page`'s
//! `PDFPageAggregatorWithMarkedContent`, without the interpreter.
//!
//! A case is the sequence of device calls of one page. Each run replays it
//! on a new device (`PDFPageAggregatorWithMarkedContent::create`, at the
//! case's base depth), every call from its recorded depth
//! (`with_depth`), with the fonts rebuilt from their dumps
//! (`pdftrace::font_case`), the process state of the case (`re`'s cache,
//! the logger caches cleared), and, for a run with `laparams`, Python's
//! `id()` order injected before `end_page` (`layout_dump::RecordedIds`).
//! The output (the advances, the exception, the `layout` stage's lines of
//! the result, `layout_dump`) is compared through its canonical text (or its
//! SHA-256 when long: `python -m oracle.unit.converter --dump SET INDEX
//! RUN` prints Python's; with `CONVERTER_DUMP_DIR` set, the Rust text of a
//! differing run is written there as `<set>-<index>-<run>.txt`).

use std::collections::{HashMap, HashSet};
use std::fs;
use std::rc::Rc;

use pdfminer::cmapdb::{CMapDB, DirCMapProvider};
use pdfminer::converter::{page_aggregator, CurItem, SUBPATH_PATTERN};
use pdfminer::layout::LAParams;
use pdfminer::pdfcolor::PDFColorSpace;
use pdfminer::pdfdevice::{ObjSeq, PDFDevice, PDFTextDevice};
use pdfminer::pdfdocument::PDFDocument;
use pdfminer::pdffont::PDFFont;
use pdfminer::pdfinterp::{inline_image_name, Color, PDFGraphicState, PDFResourceManager, StandardColor};
use pdfminer::pdfpage::PDFPage;
use pdfminer::pdfparser::FileKind;
use pdfminer::pdftypes::{PDFDict, PDFList, PDFObject};
use pdfminer::psparser::PSLiteral;
use pdfminer::utils::{Matrix, PathSegment};
use pdfplumber::page::PDFPageAggregatorWithMarkedContent;
use pdftrace::canon::sha256_hex;
use pdftrace::font_case::{build, build_font, Binder, FontDoc};
use pdftrace::layout_dump::{analyzed, id_ranks, ids_line, line, num, raw_index, RawDump, RecordedIds};
use pdftrace::typed_json::{self, Json};
use pdftrace::unit::{cache_dir, UnitData};
use pdftrace::value::Value;
use pycompat::recursion::{self, Depth, C_RECURSION_LIMIT};
use pycompat::{Num, PyErr, PyInt, PyResult};

/// The generator's `INLINE_LIMIT`.
const INLINE_LIMIT: usize = 20000;

/// Opens the data and installs the provider of the predefined CMaps.
fn open() -> UnitData {
    let data = UnitData::open("converter");
    let meta = fs::read_to_string(data.path("meta.json")).expect("meta.json");
    let meta = typed_json::parse_json(&meta).expect("meta.json is JSON");
    let Some(Json::String(rel)) = meta.get("cmap_dir") else { panic!("meta.json has no cmap_dir") };
    let dir = cache_dir().join(rel);
    let provider = DirCMapProvider::new(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
    CMapDB::set_provider(Some(Rc::new(provider)));
    data
}

/// Calls `f` with the arguments and the result of each case of
/// `<name>.jsonl`, read one line at a time.
fn for_each_case(data: &UnitData, name: &str, mut f: impl FnMut(usize, &[Value], &Value)) -> usize {
    use std::io::BufRead;
    let path = data.path(&format!("{name}.jsonl"));
    let file = fs::File::open(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let mut n = 0;
    for (i, text) in std::io::BufReader::new(file).lines().enumerate() {
        let json = typed_json::parse_json(&text.expect("a line")).expect("JSON");
        let input = typed_json::from_json(json.get("in").expect("in")).expect("typed in");
        let out = typed_json::from_json(json.get("out").expect("out")).expect("typed out");
        let Value::List(args) = &input else { panic!("\"in\" is not a list") };
        f(i, args, &out);
        n += 1;
    }
    n
}

fn items(v: &Value) -> &[Value] {
    match v {
        Value::Tuple(x) | Value::List(x) => x,
        other => panic!("not a sequence: {other:?}"),
    }
}

fn to_num(v: &Value) -> Num {
    match v {
        Value::Int(i) => Num::Int(i.clone()),
        Value::Float(f) => Num::Float(*f),
        Value::Bool(b) => Num::Int(PyInt::Small(i64::from(*b))),
        other => panic!("not a number: {other:?}"),
    }
}

fn nums<const N: usize>(v: &Value) -> [Num; N] {
    let x = items(v);
    assert_eq!(x.len(), N, "{v:?}");
    std::array::from_fn(|i| to_num(&x[i]))
}

fn int(v: &Value) -> i64 {
    match v {
        Value::Int(i) => i.as_i64().expect("a small int"),
        other => panic!("not an int: {other:?}"),
    }
}

fn py_int(v: &Value) -> PyInt {
    match v {
        Value::Int(i) => i.clone(),
        other => panic!("not an int: {other:?}"),
    }
}

fn field<'a>(v: &'a Value, key: &str) -> &'a Value {
    let Value::Dict(items) = v else { panic!("not a dict: {v:?}") };
    items.iter().find(|(k, _)| k.as_str() == Some(key)).map(|(_, v)| v).unwrap_or_else(|| panic!("no {key}"))
}

/// The `{"py": .., "c_left": ..}` depth.
fn depth(v: &Value) -> Depth {
    let d = |k| u32::try_from(int(field(v, k))).expect("a depth");
    Depth { py: d("py"), c: C_RECURSION_LIMIT - d("c_left") }
}

/// A document for the pages (`begin_page` reads only their mediabox).
fn document() -> Rc<PDFDocument> {
    let pdf: &[u8] = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n";
    PDFDocument::open(Rc::from(pdf), FileKind::BytesIO, "").expect("a document")
}

fn page(doc: &Rc<PDFDocument>, mediabox: [f64; 4]) -> PDFPage {
    PDFPage {
        doc: doc.clone(),
        pageid: PDFObject::Null,
        attrs: PDFDict::new(),
        label: None,
        lastmod: PDFObject::Null,
        resources: PDFObject::Null,
        mediabox,
        cropbox: mediabox,
        contents: PDFList::new(Vec::new()),
        rotate: PyInt::ZERO,
        annots: PDFObject::Null,
        beads: PDFObject::Null,
    }
}

/// The fonts of a set, by the SHA-256 of their dump (or the exception of
/// their construction), and the documents their objects resolve through.
struct Fonts {
    fonts: HashMap<String, PyResult<Rc<PDFFont>>>,
    _docs: Vec<Rc<FontDoc>>,
}

fn load_fonts(data: &UnitData, set: &str) -> Fonts {
    let mut fonts = HashMap::new();
    let mut docs = Vec::new();
    for_each_case(data, &format!("{set}_fonts"), |_, a, _| {
        let sha = a[0].as_str().expect("a sha").to_owned();
        let (doc, font) = recursion::with_depth(Depth::default(), || build_font(items(&a[1])));
        docs.push(doc);
        fonts.insert(sha, font.map(Rc::new));
    });
    Fonts { fonts, _docs: docs }
}

fn color(v: &Value) -> Color {
    let standard = |v: &Value| match v {
        Value::Int(_) | Value::Float(_) | Value::Bool(_) => StandardColor::Gray(to_num(v)),
        Value::Tuple(t) if t.len() == 3 => {
            StandardColor::Rgb(std::array::from_fn(|i| to_num(&t[i]).to_f64().expect("a float")))
        }
        Value::Tuple(t) if t.len() == 4 => {
            StandardColor::Cmyk(std::array::from_fn(|i| to_num(&t[i]).to_f64().expect("a float")))
        }
        other => panic!("not a standard color: {other:?}"),
    };
    match v {
        Value::Str(s) => Color::Pattern(s.as_str().expect("a pattern name").to_owned()),
        Value::Tuple(t) if t.len() == 2 => {
            Color::UncoloredPattern(standard(&t[0]), t[1].as_str().expect("a pattern name").to_owned())
        }
        v => Color::Standard(standard(v)),
    }
}

/// The objects of one case.
struct Case<'a> {
    binder: &'a Binder,
    fonts: Vec<Rc<PDFFont>>,
    gstates: Vec<Rc<PDFGraphicState>>,
    colorspaces: HashMap<String, Rc<PDFColorSpace>>,
    inline: HashSet<String>,
}

impl Case<'_> {
    fn colorspace(&mut self, v: &Value) -> Rc<PDFColorSpace> {
        let key = pdftrace::canon::canon(v);
        if let Some(cs) = self.colorspaces.get(&key) {
            return cs.clone();
        }
        let x = items(v);
        let cs = Rc::new(PDFColorSpace::new(x[0].as_str().expect("a name"), build(&x[1], self.binder)));
        self.colorspaces.insert(key, cs.clone());
        cs
    }

    fn gstate(&mut self, fields: &Value) -> PDFGraphicState {
        let f = items(fields);
        let o = |v: &Value| build(v, self.binder);
        let dash = match &f[4] {
            Value::None => None,
            d => {
                let d = items(d);
                Some((o(&d[0]), o(&d[1])))
            }
        };
        PDFGraphicState {
            linewidth: to_num(&f[0]),
            linecap: o(&f[1]),
            linejoin: o(&f[2]),
            miterlimit: o(&f[3]),
            dash,
            intent: o(&f[5]),
            flatness: o(&f[6]),
            scolor: color(&f[7]),
            scs: self.colorspace(&f[8]),
            ncolor: color(&f[9]),
            ncs: self.colorspace(&f[10]),
        }
    }

    fn name(&mut self, v: &Value) -> String {
        match v {
            Value::Named { name, fields } if name == "inline" => {
                let n = int(&fields[0].1);
                let name = inline_image_name(usize::try_from(n).expect("a count"));
                self.inline.insert(name.clone());
                name
            }
            v => v.as_str().expect("a name").to_owned(),
        }
    }

    fn seq(&self, v: &Value) -> ObjSeq {
        match v {
            Value::List(_) => {
                let PDFObject::List(l) = build(v, self.binder) else { unreachable!() };
                ObjSeq::List(l)
            }
            Value::Tuple(t) => ObjSeq::Tuple(t.iter().map(|x| build(x, self.binder)).collect()),
            other => panic!("not a sequence: {other:?}"),
        }
    }
}

fn path(v: &Value) -> Vec<PathSegment> {
    items(v)
        .iter()
        .map(|seg| {
            let seg = items(seg);
            let op = seg[0].as_str().expect("an operator").chars().next().expect("an operator");
            (op, seg[1..].iter().map(to_num).collect())
        })
        .collect()
}

fn laparams(v: &Value) -> Option<LAParams> {
    let Value::Dict(items) = v else { return None };
    let mut lp = LAParams::default();
    for (k, v) in items {
        match k.as_str().expect("a key") {
            "line_overlap" => lp.line_overlap = to_num(v),
            "char_margin" => lp.char_margin = to_num(v),
            "line_margin" => lp.line_margin = to_num(v),
            "word_margin" => lp.word_margin = to_num(v),
            "boxes_flow" => lp.boxes_flow = if matches!(v, Value::None) { None } else { Some(to_num(v)) },
            "detect_vertical" => lp.detect_vertical = matches!(v, Value::Bool(true)),
            "all_texts" => lp.all_texts = matches!(v, Value::Bool(true)),
            other => panic!("{other}"),
        }
    }
    Some(lp)
}

fn exc(e: &PyErr) -> Value {
    Value::from(e)
}

fn canon_or_hash(text: &str) -> String {
    if text.len() <= INLINE_LIMIT {
        return text.to_owned();
    }
    format!("sha256:{}:{}", sha256_hex(text.as_bytes()), text.len())
}

/// One call of a case.
enum Call {
    SetCtm(Matrix),
    BeginPage(PDFPage, Matrix),
    EndPage(PDFPage),
    BeginFigure(String, ObjSeq, ObjSeq),
    EndFigure(String),
    RenderImage(String, Rc<pdfminer::pdftypes::PDFStream>),
    PaintPath(Rc<PDFGraphicState>, bool, bool, bool, Vec<PathSegment>),
    RenderChar(Matrix, Rc<PDFFont>, Num, Num, Num, PyInt, Rc<PDFColorSpace>, Rc<PDFGraphicState>),
    BeginTag(PSLiteral, Option<PDFObject>),
    EndTag,
    DoTag(PSLiteral, Option<PDFObject>),
}

fn literal(v: &Value, binder: &Binder) -> PSLiteral {
    let PDFObject::Literal(lit) = build(v, binder) else { panic!("a literal: {v:?}") };
    lit
}

fn props(v: &Value, binder: &Binder) -> Option<PDFObject> {
    match v {
        Value::None => None,
        v => Some(build(v, binder)),
    }
}

fn bool_arg(v: &Value) -> bool {
    matches!(v, Value::Bool(true))
}

/// The `render_char` calls of a `chars` call (the generator's `expand`):
/// `[kind, py, (a, b, c, d), font, fontsize, scaling, rise, ncs, gstate,
/// packed]`, `packed` the zlib-compressed columns of `e`, `f` (f64) and
/// the cids (i64), little-endian.
fn expand(c: &[Value]) -> Vec<Vec<Value>> {
    let Value::Bytes(packed) = &c[9] else { panic!("packed chars") };
    let data = pycompat::zlib::decompress(packed).expect("zlib data");
    let n = data.len() / 24;
    let word = |k: usize| <[u8; 8]>::try_from(&data[8 * k..8 * k + 8]).expect("8 bytes");
    (0..n)
        .map(|i| {
            let mut matrix = items(&c[2]).to_vec();
            matrix.push(Value::Float(f64::from_le_bytes(word(i))));
            matrix.push(Value::Float(f64::from_le_bytes(word(n + i))));
            let cid = Value::Int(PyInt::Small(i64::from_le_bytes(word(2 * n + i))));
            vec![
                Value::str("render_char"),
                c[1].clone(),
                Value::Tuple(matrix),
                c[3].clone(),
                c[4].clone(),
                c[5].clone(),
                c[6].clone(),
                cid,
                c[7].clone(),
                c[8].clone(),
            ]
        })
        .collect()
}

/// The calls of a written case: a list, or the zlib-compressed text of
/// their typed JSON (the generator's `pack_calls`).
fn unpack_calls(v: &Value) -> Vec<Value> {
    match v {
        Value::Bytes(packed) => {
            let text = pycompat::zlib::decompress(packed).expect("zlib data");
            let json = typed_json::parse_json(std::str::from_utf8(&text).expect("ASCII")).expect("JSON");
            let mut calls = typed_json::from_json(&json).expect("typed calls");
            match &mut calls {
                Value::List(calls) => std::mem::take(calls),
                other => panic!("calls: {other:?}"),
            }
        }
        v => items(v).to_vec(),
    }
}

/// The `paint_path` call of a `path` call: `[kind, py, gstate, stroke,
/// fill, evenodd, ops, packed]`, `packed` the zlib-compressed coordinates
/// (f64, little-endian), as many for each operator as it takes.
fn expand_path(c: &[Value]) -> Vec<Value> {
    let Value::Bytes(packed) = &c[7] else { panic!("packed coordinates") };
    let data = pycompat::zlib::decompress(packed).expect("zlib data");
    let mut coords = data.as_chunks::<8>().0.iter().map(|w| f64::from_le_bytes(*w));
    let path = c[6]
        .as_str()
        .expect("the operators")
        .chars()
        .map(|op| {
            let n = match op {
                'm' | 'l' => 2,
                'c' => 6,
                'v' | 'y' => 4,
                'h' => 0,
                other => panic!("operator {other}"),
            };
            let mut seg = vec![Value::str(op.to_string())];
            seg.extend(coords.by_ref().take(n).map(Value::Float));
            Value::Tuple(seg)
        })
        .collect();
    let mut out = vec![Value::str("paint_path")];
    out.extend(c[1..6].iter().cloned());
    out.push(Value::List(path));
    out
}

/// The calls of a case, with their depth.
fn calls(case: &mut Case<'_>, doc: &Rc<PDFDocument>, values: &[Value]) -> Vec<(u32, Call)> {
    let binder = case.binder;
    let mut expanded: Vec<Vec<Value>> = Vec::with_capacity(values.len());
    for c in values {
        let c = items(c);
        match c[0].as_str() {
            Some("chars") => expanded.extend(expand(c)),
            Some("path") => expanded.push(expand_path(c)),
            _ => expanded.push(c.to_vec()),
        }
    }
    expanded
        .iter()
        .map(|c| {
            let py = u32::try_from(int(&c[1])).expect("a depth");
            let call = match c[0].as_str().expect("a kind") {
                "set_ctm" => Call::SetCtm(nums(&c[2])),
                "begin_page" => {
                    let mb = nums::<4>(&c[2]).map(|n| n.to_f64().expect("a float"));
                    Call::BeginPage(page(doc, mb), nums(&c[3]))
                }
                "end_page" => Call::EndPage(page(doc, [0.0; 4])),
                "begin_figure" => Call::BeginFigure(case.name(&c[2]), case.seq(&c[3]), case.seq(&c[4])),
                "end_figure" => Call::EndFigure(case.name(&c[2])),
                "render_image" => {
                    let PDFObject::Stream(s) = build(&c[3], binder) else { panic!("a stream") };
                    Call::RenderImage(case.name(&c[2]), s)
                }
                "paint_path" => Call::PaintPath(
                    case.gstates[usize::try_from(int(&c[2])).expect("an index")].clone(),
                    bool_arg(&c[3]),
                    bool_arg(&c[4]),
                    bool_arg(&c[5]),
                    path(&c[6]),
                ),
                "render_char" => Call::RenderChar(
                    nums(&c[2]),
                    case.fonts[usize::try_from(int(&c[3])).expect("an index")].clone(),
                    to_num(&c[4]),
                    to_num(&c[5]),
                    to_num(&c[6]),
                    py_int(&c[7]),
                    case.colorspace(&c[8]),
                    case.gstates[usize::try_from(int(&c[9])).expect("an index")].clone(),
                ),
                "begin_tag" => Call::BeginTag(literal(&c[2], binder), props(&c[3], binder)),
                "end_tag" => Call::EndTag,
                "do_tag" => Call::DoTag(literal(&c[2], binder), props(&c[3], binder)),
                other => panic!("call {other}"),
            };
            (py, call)
        })
        .collect()
}

/// `getattr(device, kind)(*args)`; the advance of a `render_char`.
fn dispatch(dev: &mut PDFPageAggregatorWithMarkedContent, call: &Call) -> PyResult<Option<Num>> {
    match call {
        Call::SetCtm(ctm) => dev.set_ctm(ctm.clone()).map(|_| None),
        Call::BeginPage(page, ctm) => dev.begin_page(page, ctm).map(|_| None),
        Call::EndPage(page) => dev.end_page(page).map(|_| None),
        Call::BeginFigure(name, bbox, matrix) => dev.begin_figure(name, bbox, matrix).map(|_| None),
        Call::EndFigure(name) => dev.end_figure(name).map(|_| None),
        Call::RenderImage(name, stream) => dev.render_image(name, stream).map(|_| None),
        Call::PaintPath(gs, stroke, fill, evenodd, path) => {
            dev.paint_path(gs, *stroke, *fill, *evenodd, path).map(|_| None)
        }
        Call::RenderChar(matrix, font, fontsize, scaling, rise, cid, ncs, gs) => {
            dev.render_char(matrix.clone(), font, fontsize, scaling, rise, cid, ncs, gs).map(Some)
        }
        Call::BeginTag(tag, props) => dev.begin_tag(tag, props.as_ref()).map(|_| None),
        Call::EndTag => dev.end_tag().map(|_| None),
        Call::DoTag(tag, props) => dev.do_tag(tag, props.as_ref()).map(|_| None),
    }
}

/// One run of a case: its output text. With `oracle_ids`, the analysis
/// has Python's `id()` order (else the library's, `CreationOrder`).
fn run(
    rsrcmgr: &Rc<PDFResourceManager>,
    case: &Case<'_>,
    calls: &[(u32, Call)],
    args: &[Value],
    run: &[Value],
    oracle_ids: bool,
) -> String {
    let base = depth(&args[4]);
    let pageno = py_int(&args[5]);
    let lp = laparams(&run[0]);
    let compiled = bool_arg(field(&args[1], "re_compiled"));
    pycompat::re_cache::set_compiled(SUBPATH_PATTERN, compiled);
    pycompat::logging::disable();
    let is_inline = |name: &str| case.inline.contains(name);
    let mut dev =
        recursion::with_depth(base, || PDFPageAggregatorWithMarkedContent::create(rsrcmgr.clone(), pageno, lp.clone()))
            .expect("the device");
    let mut out: Vec<String> = Vec::new();
    let mut raw: Option<RawDump> = None;
    for (i, (py, call)) in calls.iter().enumerate() {
        if let (Call::EndPage(_), Some(_)) = (call, &lp) {
            let a = &mut dev.aggregator.analyzer;
            if let (true, Some(CurItem::Page(p))) = (a._stack.is_empty(), &a.cur_item) {
                let ranks: Vec<[i64; 3]> = items(&run[1])
                    .iter()
                    .map(|r| {
                        let r = items(r);
                        [int(&r[0]), int(&r[1]), int(&r[2])]
                    })
                    .collect();
                if oracle_ids {
                    a.ids = Box::new(RecordedIds::new(&ranks, raw_index(&p.layout._objs)));
                }
                raw = Some(RawDump::new(&p.layout._objs, &is_inline));
            }
        }
        let result = recursion::with_depth(Depth { py: *py, c: base.c }, || dispatch(&mut dev, call));
        match result {
            Err(e) => {
                out.push(line("raised", &[Value::Int(PyInt::Small(i as i64)), exc(&e)]));
                break;
            }
            Ok(Some(adv)) => out.push(line("adv", &[Value::Int(PyInt::Small(i as i64)), num(&adv)])),
            Ok(None) => {}
        }
    }
    let result = recursion::with_depth(base, || page_aggregator::get_result(&dev).map(|_| ()));
    if let Err(e) = &result {
        out.push(line("result", &[exc(e)]));
    }
    let agg = &mut dev.aggregator;
    match (&lp, raw) {
        (None, _) => {
            if let Some(page) = &agg.result {
                out.extend(RawDump::new(&page.layout._objs, &is_inline).write(page));
            }
        }
        (Some(_), Some(raw)) => {
            let page = match &agg.result {
                Some(page) => {
                    out.extend(analyzed(page, &raw));
                    page
                }
                None => match &agg.analyzer.cur_item {
                    Some(CurItem::Page(page)) => page,
                    _ => panic!("an analysis without a page"),
                },
            };
            let ranks = id_ranks(page, &raw.index, &mut *agg.analyzer.ids);
            out.push(ids_line(&ranks));
        }
        (Some(_), None) => {}
    }
    let mut text = out.join("\n");
    text.push('\n');
    text
}

/// Replays the cases of a set; panics with the first differences. With
/// `creation_order`, also counts the runs of `LAParams()` whose analysis
/// with the library's default ids (`CreationOrder`) gives Python's result
/// (the `ids` line left out): the statistic of `docs/architecture.md`
/// ("Vague 3").
fn replay(set: &str, min_cases: usize, creation_order: bool) {
    let data = open();
    let fonts = load_fonts(&data, set);
    let doc = document();
    let rsrcmgr = Rc::new(PDFResourceManager::new(true));
    let dump_dir = std::env::var_os("CONVERTER_DUMP_DIR");
    let mut failures = Vec::new();
    let mut runs_total = 0;
    let (mut analyzed_runs, mut same_with_creation_order) = (0, 0);
    let n = for_each_case(&data, set, |index, args, expected| {
        let (_font_doc, binder) = pdftrace::font_case::load_doc(&Value::List(vec![]), &Value::List(vec![]), None);
        let font_list: Vec<Rc<PDFFont>> = items(&args[2])
            .iter()
            .map(|sha| match &fonts.fonts[sha.as_str().expect("a sha")] {
                Ok(f) => f.clone(),
                Err(e) => panic!("{}: font {sha:?} not built: {e:?}", args[0].as_str().unwrap_or("?")),
            })
            .collect();
        let mut case = Case {
            binder: &binder,
            fonts: font_list,
            gstates: Vec::new(),
            colorspaces: HashMap::new(),
            inline: HashSet::new(),
        };
        let gstates: Vec<Rc<PDFGraphicState>> = items(&args[3]).iter().map(|f| Rc::new(case.gstate(f))).collect();
        case.gstates = gstates;
        let calls = calls(&mut case, &doc, &unpack_calls(&args[6]));
        for (k, (r, expected)) in items(&args[7]).iter().zip(items(expected)).enumerate() {
            runs_total += 1;
            let text = run(&rsrcmgr, &case, &calls, args, items(r), true);
            if creation_order && matches!(&items(r)[0], Value::Dict(d) if d.is_empty()) {
                let without_ids = |t: &str| t.lines().filter(|l| !l.starts_with("ids ")).collect::<Vec<_>>().join("\n");
                let default = run(&rsrcmgr, &case, &calls, args, items(r), false);
                analyzed_runs += 1;
                same_with_creation_order += usize::from(without_ids(&default) == without_ids(&text));
            }
            let actual = canon_or_hash(&text);
            let expected = expected.as_str().expect("an output");
            if actual != expected {
                if let Some(dir) = &dump_dir {
                    let _ = fs::write(std::path::Path::new(dir).join(format!("{set}-{index}-{k}.txt")), &text);
                }
                if failures.len() < 5 {
                    let i = actual.bytes().zip(expected.bytes()).take_while(|(a, b)| a == b).count();
                    let lo = i.saturating_sub(400);
                    failures.push(format!(
                        "{set} #{index} {:?} run {k}: at byte {i}\n    python: ...{}\n    rust:   ...{}",
                        args[0].as_str().unwrap_or("?"),
                        &expected[lo..(i + 300).min(expected.len())],
                        &actual[lo..(i + 300).min(actual.len())]
                    ));
                } else {
                    failures.push(String::new());
                }
            }
        }
    });
    assert!(n >= min_cases, "{set}: only {n} cases");
    assert!(
        failures.is_empty(),
        "{set}: {} of {runs_total} runs differ; first ones:\n{}",
        failures.len(),
        failures.iter().filter(|f| !f.is_empty()).cloned().collect::<Vec<_>>().join("\n")
    );
    println!("{set}: {n} cases, {runs_total} runs");
    if creation_order {
        println!(
            "{set}: CreationOrder gives Python's analysis on {same_with_creation_order} of {analyzed_runs} pages \
             (LAParams())"
        );
    }
}

#[test]
fn adversarial() {
    replay("adversarial", 200, false);
}

#[test]
fn depth_boundaries() {
    replay("depth", 400, false);
}

/// Checks the marker `<name>.json` that `--corpus` writes after a run.
fn check_marker(data: &UnitData, name: &str) {
    let path = data.path(&format!("{name}.json"));
    let text = fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!("missing {}: generate it with python -m oracle.unit.converter --corpus", path.display())
    });
    let marker = typed_json::parse_json(&text).expect("marker JSON");
    data.check_sources(&path, marker.get("sources"));
}

#[test]
fn corpus() {
    check_marker(&open(), "corpus");
    // the CreationOrder statistic (a second analysis of each page) on demand
    replay("corpus", 3000, std::env::var_os("CONVERTER_CREATION_ORDER").is_some());
}
