//! Stage `plumber_objects`: pdfplumber's page objects (every kind, every
//! attribute), boxes, `doctop`, annotations, hyperlinks, `pdf.objects`,
//! then the same with `laparams={}`. The traversal and the line format are
//! those of `oracle/stages/plumber_objects.py`.
//!
//! The runner mirrors the Python stage's generators, a frame each while
//! they run (`page_lines`, `boxes`, `objects_lines`, `annots_lines` and its
//! `get` lambda, `_pdf_lines`, `_laparams_lines`, whose `list(objects_lines(...))`
//! resumes the generator from C), so that pdfplumber's code runs at the
//! depth Python runs it.
//!
//! The analysis of the second opening (`laparams={}`) takes the `id()`
//! ranking of the oracle's trace (`ids <page_number> [[key, rank], ...]`,
//! `StageInput::oracle`) through `OpenOptions::ids`: [`ContentIds`] gives
//! a box or group the rank Python's element of the same content key had
//! (the ranks of a key in increasing order, to its elements in the order
//! the analysis asks for them; an element Python did not have gets an id
//! after all of them). Without the oracle's trace, the ids are those of the
//! first request (`CreationOrder`). The `ids` line written is the ranking
//! of the Rust analysis, keyed as the Python stage keys it
//! ([`content_ids`]).

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::io::{self, Write};
use std::rc::Rc;

use pdfminer::layout::{IdSource, LTChar, LTItem, LTPage, TextGroupElement, TextLineElement};
use pdfplumber::obj::{Obj, PyValue};
use pdfplumber::page::{Page, PlumberPayload};
use pdfplumber::pdf::{IdsFactory, OpenOptions, Source, PDF};
use pycompat::recursion::{eval_entry, leaf, py_frame};
use pycompat::specialize::{AttrSite, CallSite};
use pycompat::{PyInt, PyResult};

use super::common::{terminated, Lines};
use super::plumber_common::{box_lines, digest, exc, open_pdf, Names, ObjectsWriter};
use super::{Stage, StageInput};
use crate::canon::{canon, sha256_hex};
use crate::layout_dump::{attr, colorspace, fontname, graphic_state, num, ptr, tuple};
use crate::plumber::to_value;
use crate::typed_json::{parse_json, Json};
use crate::value::Value;

pub const STAGE: Stage = Stage { name: "plumber_objects", run: Some(run), oracle_input: true };

type P = PlumberPayload;

/// The `ids` lines of an oracle trace of the stage: page number -> the
/// `[key, rank]` items.
pub fn oracle_ids(trace: &str) -> Result<HashMap<i64, Vec<(String, u64)>>, String> {
    let mut out = HashMap::new();
    for line in trace.lines() {
        let Some(rest) = line.strip_prefix("ids ") else { continue };
        let (page, list) = rest.split_once(' ').ok_or_else(|| format!("ids: {line}"))?;
        let page: i64 = page.parse().map_err(|e| format!("ids: {page}: {e}"))?;
        let json = parse_json(list)?;
        let Json::Array(items) = &json else { return Err(format!("ids: not a list: {line}")) };
        let mut ranks = Vec::with_capacity(items.len());
        for item in items {
            let Json::Array(pair) = item else { return Err(format!("ids: not a pair: {item:?}")) };
            let [Json::String(key), Json::Number(rank)] = pair.as_slice() else {
                return Err(format!("ids: not a [key, rank] pair: {pair:?}"));
            };
            ranks.push((key.clone(), rank.parse::<u64>().map_err(|e| format!("ids: {rank}: {e}"))?));
        }
        out.insert(page, ranks);
    }
    Ok(out)
}

/// `_key(prefix, texts)`: the first 16 hex digits of the SHA-256 of
/// `prefix + "\n" + "\n".join(sorted(texts))`.
fn key_of(prefix: &str, mut texts: Vec<String>) -> String {
    texts.sort();
    sha256_hex(format!("{prefix}\n{}", texts.join("\n")).as_bytes())[..16].to_owned()
}

/// `_char_text(c)`: `canon(Named("LTChar", fields))`, the `__dict__` of
/// the char but `bbox`, its graphic state and color space as values.
fn char_text(c: &LTChar<P>) -> String {
    let mut f: Vec<(Value, Value)> = vec![
        (Value::str("_text"), Value::py_str(&c._text)),
        (Value::str("matrix"), tuple(&c.matrix)),
        (Value::str("fontname"), fontname(&c.fontname)),
        (Value::str("ncs"), colorspace(&c.ncs)),
        (Value::str("graphicstate"), graphic_state(&c.graphicstate)),
        (Value::str("adv"), num(&c.adv)),
        (Value::str("upright"), Value::Bool(c.upright)),
        (Value::str("x0"), num(&c.comp.x0)),
        (Value::str("y0"), num(&c.comp.y0)),
        (Value::str("x1"), num(&c.comp.x1)),
        (Value::str("y1"), num(&c.comp.y1)),
        (Value::str("width"), num(&c.comp.width)),
        (Value::str("height"), num(&c.comp.height)),
        (Value::str("size"), num(&c.size)),
    ];
    for (k, v) in c.attrs.items() {
        f.push((Value::str(k), attr(&v)));
    }
    canon(&Value::Named { name: "LTChar".into(), fields: f })
}

/// The identity of a group element.
fn element_ptr(g: &TextGroupElement<P>) -> usize {
    match g {
        TextGroupElement::Box(b) => ptr(b),
        TextGroupElement::Group(g) => ptr(g),
    }
}

/// The ids of an analysis by content key (see the module documentation).
#[derive(Default)]
pub struct ContentIds {
    /// A key's ranks not given yet, in increasing order.
    ranks: HashMap<String, VecDeque<u64>>,
    /// The ids given, by element identity.
    given: HashMap<usize, u64>,
    /// The keys computed, by element identity (contents never change once
    /// an element exists).
    keys: HashMap<usize, String>,
    next: u64,
}

impl ContentIds {
    fn new(ranks: &[(String, u64)]) -> ContentIds {
        let mut by_key: HashMap<String, Vec<u64>> = HashMap::new();
        for (k, r) in ranks {
            by_key.entry(k.clone()).or_default().push(*r);
        }
        let next = ranks.iter().map(|(_, r)| r + 1).max().unwrap_or(0);
        ContentIds {
            ranks: by_key
                .into_iter()
                .map(|(k, mut v)| {
                    v.sort_unstable();
                    (k, v.into())
                })
                .collect(),
            next,
            ..ContentIds::default()
        }
    }

    /// The content key of an element (post-order, without recursion).
    fn key(&mut self, g: &TextGroupElement<P>) -> String {
        let mut stack: Vec<(TextGroupElement<P>, bool)> = vec![(g.clone(), false)];
        while let Some((e, done)) = stack.pop() {
            let id = element_ptr(&e);
            if self.keys.contains_key(&id) {
                continue;
            }
            match &e {
                TextGroupElement::Box(b) => {
                    let mut texts = Vec::new();
                    for line in &b.borrow()._objs {
                        for o in &line.borrow()._objs {
                            if let TextLineElement::Char(c) = o {
                                texts.push(char_text(c));
                            }
                        }
                    }
                    self.keys.insert(id, key_of("box", texts));
                }
                TextGroupElement::Group(grp) => {
                    if done {
                        let texts = grp.borrow()._objs.iter().map(|c| self.keys[&element_ptr(c)].clone()).collect();
                        self.keys.insert(id, key_of("group", texts));
                    } else {
                        stack.push((e.clone(), true));
                        stack.extend(grp.borrow()._objs.iter().map(|c| (c.clone(), false)));
                    }
                }
            }
        }
        self.keys[&element_ptr(g)].clone()
    }

    fn id(&mut self, g: &TextGroupElement<P>) -> u64 {
        let ptr = element_ptr(g);
        if let Some(id) = self.given.get(&ptr) {
            return *id;
        }
        let key = self.key(g);
        let id = match self.ranks.get_mut(&key).and_then(VecDeque::pop_front) {
            Some(rank) => rank,
            None => {
                self.next += 1;
                self.next - 1
            }
        };
        self.given.insert(ptr, id);
        id
    }
}

/// A page's [`ContentIds`], shared by its analysis and the stage.
struct SharedIds(Rc<RefCell<ContentIds>>);

impl IdSource<P> for SharedIds {
    fn id(&mut self, obj: &TextGroupElement<P>) -> u64 {
        self.0.borrow_mut().id(obj)
    }
}

/// `content_ids(ltpage)`: `[key, rank]` of the boxes and groups of the
/// page's groups and its figures' (depth first), ranked by their ids.
pub fn content_ids(ltpage: &LTPage<P>, ids: &mut ContentIds) -> Value {
    let mut roots: Vec<TextGroupElement<P>> = ltpage.layout.groups.clone().unwrap_or_default();
    let mut containers: Vec<LTItem<P>> =
        ltpage.layout._objs.iter().filter(|o| matches!(o, LTItem::Figure(_))).cloned().collect();
    while !containers.is_empty() {
        let c = containers.remove(0);
        let LTItem::Figure(fig) = &c else { unreachable!() };
        let fig = fig.borrow();
        let figures: Vec<LTItem<P>> =
            fig.layout._objs.iter().filter(|o| matches!(o, LTItem::Figure(_))).cloned().collect();
        containers.splice(0..0, figures);
        roots.extend(fig.layout.groups.clone().unwrap_or_default());
    }
    let mut found: Vec<(u64, String)> = Vec::new();
    for root in &roots {
        let mut stack: Vec<(TextGroupElement<P>, bool)> = vec![(root.clone(), false)];
        while let Some((g, done)) = stack.pop() {
            match &g {
                TextGroupElement::Box(_) => {
                    let key = ids.key(&g);
                    found.push((ids.id(&g), key));
                }
                TextGroupElement::Group(grp) => {
                    if done {
                        let key = ids.key(&g);
                        found.push((ids.id(&g), key));
                    } else {
                        stack.push((g.clone(), true));
                        stack.extend(grp.borrow()._objs.iter().rev().map(|c| (c.clone(), false)));
                    }
                }
            }
        }
    }
    found.sort();
    Value::List(
        found
            .into_iter()
            .enumerate()
            .map(|(rank, (_, key))| Value::List(vec![Value::str(key), Value::Int(PyInt::Small(rank as i64))]))
            .collect(),
    )
}

/// `annot_value(a, names)`: an annotation as written, its `data` dict
/// holding the `Page` in place of `/P`.
pub fn annot_value(a: &Obj, names: &Names) -> PyResult<Value> {
    let mut items = Vec::with_capacity(a.len());
    for (k, v) in a.items() {
        let value = if *k == "data" {
            match v {
                PyValue::Dict(d) => names.obj(d),
                // names.obj(a["data"]) reads its items(): the stage itself
                // raises for the data of an annotation that is a stream
                other => {
                    return Err(pycompat::exc::AttributeError(format!(
                        "'{}' object has no attribute 'items'",
                        other.type_name()
                    )))
                }
            }
        } else if *k == "name" || *k == "P" {
            names.value(v)
        } else {
            to_value(v)
        };
        items.push((Value::py_str(k), value));
    }
    Ok(Value::Dict(items))
}

/// `annots_lines(label, item, get, names)`: a generator, and the lambda
/// it calls.
fn annots_lines(
    lines: &mut Lines,
    label: &str,
    item: &str,
    get: impl FnOnce() -> PyResult<Vec<Obj>>,
    names: &Names,
) -> PyResult<()> {
    let _gen = py_frame()?;
    let annots = {
        let _lambda = py_frame()?;
        get()
    };
    match annots {
        Err(e) => lines.push(format!("{label} {}", exc(&e))),
        Ok(annots) => {
            lines.push(format!("{label} {}", annots.len()));
            for (i, a) in annots.iter().enumerate() {
                lines.push(format!("{item} {i} {}", canon(&annot_value(a, names)?)));
            }
        }
    }
    Ok(())
}

// The stage's loads of properties whose getter can run the interpretation
// (`pycompat::specialize::AttrSite`, as the library's: the first execution
// of a site holds the two C units of the getter's call from C while it
// runs). Loads of cheap getters (`getattr(page, name)` of `boxes`: `width`,
// `height`) are left out: nothing deep runs under them.
static RUN_PAGES: AttrSite = AttrSite::new("plumber_objects run pdf.pages");
static OBJECTS_LINES_OBJECTS: AttrSite = AttrSite::new("plumber_objects objects_lines page.objects");
static ANNOTS_LAMBDA: AttrSite = AttrSite::new("plumber_objects page_lines lambda: page.annots");
static HYPERLINKS_LAMBDA: AttrSite = AttrSite::new("plumber_objects page_lines lambda: page.hyperlinks");
static PDF_LINES_OBJECTS: AttrSite = AttrSite::new("plumber_objects _pdf_lines pdf.objects");
static PDF_LINES_GETATTR: CallSite = CallSite::new("plumber_objects _pdf_lines getattr(pdf, what)");
static LAPARAMS_PAGES: AttrSite = AttrSite::new("plumber_objects _laparams_lines pdf.pages");
static LAPARAMS_LAYOUT: AttrSite = AttrSite::new("plumber_objects _laparams_lines page.layout");
static OPEN_LINES_PAGES: AttrSite = AttrSite::new("unit.plumber open_lines pdf.pages");
static OPEN_LINES_LAYOUT: AttrSite = AttrSite::new("unit.plumber open_lines page.layout");

/// `objects_lines(page, writer, refer)`, a generator whose frame is taken
/// by the caller.
fn objects_lines(
    lines: &mut Lines,
    page: &Page,
    writer: &mut ObjectsWriter,
    refer: Option<&HashMap<String, usize>>,
) -> bool {
    match OBJECTS_LINES_OBJECTS.get(page.class_name(), || page.objects()) {
        Err(e) => {
            lines.push(format!("objects {}", exc(&e)));
            false
        }
        Ok(objects) => {
            writer.write(lines, &objects, refer);
            true
        }
    }
}

/// `page_lines(page, writer)`
fn page_lines(lines: &mut Lines, page: &Rc<Page>, writer: &mut ObjectsWriter) -> PyResult<()> {
    let _gen = py_frame()?;
    lines.push(format!("page {}", page.page_number));
    {
        let _boxes = py_frame()?;
        box_lines(lines, page)?;
    }
    {
        let _objects_lines = py_frame()?;
        objects_lines(lines, page, writer, None);
    }
    let names = Names::default();
    annots_lines(lines, "annots", "annot", || ANNOTS_LAMBDA.get(page.class_name(), || page.annots()), &names)?;
    annots_lines(
        lines,
        "hyperlinks",
        "hyperlink",
        || HYPERLINKS_LAMBDA.get(page.class_name(), || page.hyperlinks()),
        &names,
    )?;
    Ok(())
}

/// `_pdf_lines(pdf)`
fn pdf_lines(lines: &mut Lines, pdf: &PDF) -> PyResult<()> {
    let _gen = py_frame()?;
    match PDF_LINES_OBJECTS.get("PDF", || pdf.objects()) {
        Err(e) => lines.push(format!("pdf objects {}", exc(&e))),
        Ok(objects) => {
            let counts = Value::List(
                objects
                    .iter()
                    .map(|(k, v)| Value::List(vec![Value::py_str(k), Value::Int(PyInt::Small(v.len() as i64))]))
                    .collect(),
            );
            lines.push(format!("pdf objects {}", canon(&counts)));
        }
    }
    let names = Names::default();
    for what in ["annots", "hyperlinks"] {
        // getattr(pdf, what): the builtin's call, the getter from C
        let values = {
            let _call = PDF_LINES_GETATTR.builtin_call()?;
            let _entry = eval_entry()?;
            if what == "annots" {
                pdf.annots()
            } else {
                pdf.hyperlinks()
            }
        };
        match values {
            Err(e) => lines.push(format!("pdf {what} {}", exc(&e))),
            Ok(values) => {
                let list = Value::List(values.iter().map(|a| annot_value(a, &names)).collect::<PyResult<_>>()?);
                lines.push(format!("pdf {what} {} {}", values.len(), digest(&list)));
            }
        }
    }
    Ok(())
}

/// The analyses' ids of the pages a `PDF` opens, by page number.
pub type IdsRegistry = Rc<RefCell<HashMap<i64, Rc<RefCell<ContentIds>>>>>;

/// An `IdsFactory` giving each page's analysis the ranks of the oracle's
/// `ids` line of that page ([`ContentIds`]), and the registry where the
/// stage finds them again.
pub fn ids_factory(oracle: &HashMap<i64, Vec<(String, u64)>>) -> (IdsFactory, IdsRegistry) {
    let registry: IdsRegistry = Rc::default();
    let factory: IdsFactory = {
        let registry = registry.clone();
        let oracle = oracle.clone();
        Rc::new(move |page_number: &PyInt| {
            let n = page_number.as_i64().expect("a page number");
            let ids = Rc::new(RefCell::new(ContentIds::new(oracle.get(&n).map_or(&[][..], Vec::as_slice))));
            registry.borrow_mut().insert(n, ids.clone());
            Box::new(SharedIds(ids)) as Box<dyn IdSource<P>>
        })
    };
    (factory, registry)
}

/// `open_lines(path, options)` of `oracle/unit/plumber.py` (the `open`
/// cases, replayed by `tests/plumber_unit.rs`): `pdfplumber.open(path,
/// **options)`, then for each page its `initial_doctop`, its objects and,
/// with `laparams`, the `id()` ranking of its analysis, with the oracle's
/// ranks (`input.oracle`, the expected trace).
pub fn open_trace(input: &StageInput<'_>, options: &Obj, out: &mut dyn Write) -> io::Result<()> {
    let oracle = match input.oracle.map(oracle_ids).transpose() {
        Ok(ids) => ids.unwrap_or_default(),
        Err(e) => return Err(io::Error::new(io::ErrorKind::InvalidData, format!("the oracle's trace: {e}"))),
    };
    terminated(out, |lines: &mut Lines| {
        let (factory, registry) = ids_factory(&oracle);
        let mut o = OpenOptions::new();
        for (k, v) in options.items() {
            match k.as_str() {
                Some("pages") => o.pages = Some(v.clone()),
                Some("laparams") => o.laparams = Some(v.clone()),
                Some("password") => o.password = Some(v.clone()),
                Some("strict_metadata") => o.strict_metadata = v.truthy(),
                Some("unicode_norm") => o.unicode_norm = Some(v.clone()),
                Some("raise_unicode_errors") => o.raise_unicode_errors = v.truthy(),
                other => panic!("an option of pdfplumber.open: {other:?}"),
            }
        }
        o.ids = Some(factory);
        let pdf = match PDF::open(Source::Path { data: Rc::from(input.pdf), path: pycompat::PyStr::new("") }, o) {
            Ok(pdf) => pdf,
            Err(e) => {
                lines.push(format!("open {}", exc(&e)));
                return Ok(());
            }
        };
        let pages = match OPEN_LINES_PAGES.get("PDF", || pdf.pages()) {
            Ok(pages) => pages,
            Err(e) => {
                lines.push(format!("pages {}", exc(&e)));
                return Ok(());
            }
        };
        for page in &pages {
            lines.push(format!("page {} {}", page.page_number, canon(&to_value(&page.initial_doctop))));
            let n = page.page_number.as_i64().expect("a page number");
            // list(objects_lines(page)): the generator resumed from C
            let ok = {
                let _entry = eval_entry()?;
                let _gen = py_frame()?;
                objects_lines(lines, page, &mut ObjectsWriter::default(), None)
            };
            if pdf.core.laparams.is_some() && ok {
                let layout = OPEN_LINES_LAYOUT.get(page.class_name(), || page.layout())?;
                leaf(1)?;
                let ids = registry.borrow().get(&n).cloned().unwrap_or_default();
                let value = content_ids(&layout, &mut ids.borrow_mut());
                lines.push(format!("ids {n} {}", canon(&value)));
            }
        }
        Ok(())
    })
}

/// `_laparams_lines(path, password, max_pages, chars)`
fn laparams_lines(
    lines: &mut Lines,
    input: &StageInput<'_>,
    chars: &HashMap<i64, HashMap<String, usize>>,
    oracle: &HashMap<i64, Vec<(String, u64)>>,
) -> PyResult<()> {
    let _gen = py_frame()?;
    lines.push("laparams {}");
    let (factory, registry) = ids_factory(oracle);
    let pdf = match open_pdf(input, |o| {
        o.laparams = Some(PyValue::Dict(Obj::new()));
        o.ids = Some(factory);
    }) {
        Ok(pdf) => pdf,
        Err(e) => {
            lines.push(format!("open {}", exc(&e)));
            return Ok(());
        }
    };
    let pages = match LAPARAMS_PAGES.get("PDF", || pdf.pages()) {
        Ok(pages) => pages,
        Err(e) => {
            lines.push(format!("pages {}", exc(&e)));
            return Ok(());
        }
    };
    let empty = HashMap::new();
    for page in &pages {
        lines.push(format!("page {}", page.page_number));
        let n = page.page_number.as_i64().expect("a page number");
        // list(objects_lines(page, refer=...)): the generator resumed from C
        let ok = {
            let _entry = eval_entry()?;
            let _gen = py_frame()?;
            let mut writer = ObjectsWriter::default();
            objects_lines(lines, page, &mut writer, Some(chars.get(&n).unwrap_or(&empty)))
        };
        if ok {
            // content_ids(page.layout): the property (cached), then the
            // harness's function
            let layout = LAPARAMS_LAYOUT.get(page.class_name(), || page.layout())?;
            leaf(1)?;
            let ids = registry.borrow().get(&n).cloned().unwrap_or_default();
            let value = content_ids(&layout, &mut ids.borrow_mut());
            lines.push(format!("ids {n} {}", canon(&value)));
        }
    }
    Ok(())
}

fn run(input: &StageInput<'_>, out: &mut dyn Write) -> io::Result<()> {
    let oracle = match input.oracle.map(oracle_ids).transpose() {
        Ok(ids) => ids.unwrap_or_default(),
        Err(e) => return Err(io::Error::new(io::ErrorKind::InvalidData, format!("the oracle's trace: {e}"))),
    };
    terminated(out, |lines: &mut Lines| {
        let pdf = match open_pdf(input, |_| {}) {
            Ok(pdf) => pdf,
            Err(e) => {
                lines.push(format!("open {}", exc(&e)));
                return Ok(());
            }
        };
        let pages = match RUN_PAGES.get("PDF", || pdf.pages()) {
            Ok(pages) => pages,
            Err(e) => {
                lines.push(format!("pages {}", exc(&e)));
                Vec::new()
            }
        };
        let mut chars: HashMap<i64, HashMap<String, usize>> = HashMap::new();
        for page in &pages {
            let mut writer = ObjectsWriter::default();
            page_lines(lines, page, &mut writer)?;
            chars.insert(page.page_number.as_i64().expect("a page number"), writer.chars);
        }
        pdf_lines(lines, &pdf)?;
        laparams_lines(lines, input, &chars, &oracle)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_ids_lines() {
        let trace = "page 1\nids 1 [[\"ab\", 0], [\"cd\", 1]]\nids 3 []\n!end\n";
        let ids = oracle_ids(trace).unwrap();
        assert_eq!(ids[&1], vec![("ab".to_owned(), 0), ("cd".to_owned(), 1)]);
        assert!(ids[&3].is_empty());
        assert!(oracle_ids("ids 1 [[\"ab\"]]\n").is_err());
    }

    #[test]
    fn a_key_takes_its_ranks_in_order() {
        let ids = ContentIds::new(&[("k".into(), 3), ("k".into(), 1)]);
        assert_eq!(ids.ranks["k"], [1, 3]);
        assert_eq!(ids.next, 4);
        assert_eq!(key_of("box", vec!["b".into(), "a".into()]), key_of("box", vec!["a".into(), "b".into()]));
    }
}
