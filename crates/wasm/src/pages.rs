//! The operations of documents and pages: pdfplumber's `PDF` and `Page`
//! (`crates/pdfplumber`), as JavaScript reaches them (`docs/js-runtime.md`,
//! "Documents et pages").
//!
//! A document handle keeps the `PDF` of `pdfplumber.open(io.BytesIO(data),
//! **options)` (`doc_open`, `crate::ops`); `doc_pages` gives a handle to
//! each page of `pdf.pages`, and the operations that derive a page
//! (`crop`, `within_bbox`, `outside_bbox`, `filter`, `dedupe_chars`) a
//! handle to the derived page; `find_tables` a handle to each table
//! (`Table.extract`). Each operation is one statement of a Python script
//! (`crate::ops::ENTRY_DEPTH`): `pdf.pages`, `page.chars`,
//! `page.extract_text(**kwargs)`... Results are pdfplumber's values,
//! written in the format of the boundary (`PJB1` or JSON).
//!
//! Two operations take what JavaScript computed:
//!
//! - **search** runs in two calls: `page_search_text` gives the text a
//!   pattern runs over (`Page::search_text`: `search`'s text map, cached by
//!   `get_textmap`, computed at the depth Python's `search` computes it),
//!   JavaScript runs its `RegExp` over it, and `page_search` takes the
//!   spans of each match (UTF-16 offsets) as `re.finditer` would give them
//!   (`docs/public-api.md`, "Search": the semantics are JavaScript's);
//! - **filter**: JavaScript evaluates its function on the parent page's
//!   objects (`page_objects`, which computes them) and gives, for each
//!   kind, the truth of each result; the `FilteredPage`'s test function
//!   looks each object up by identity (`id()`) in that mask. Python calls
//!   the function lazily, when the derived page's objects are computed:
//!   on the same dicts unless the parent's cache was flushed in between
//!   (`flush_cache`, `close`), which the mask cannot answer (`NotPorted`).
//!
//! Every operation that reaches a document is recorded and replayed after
//! a void call (`crate::ops`, "Void calls"); a page handle JavaScript
//! freed stays as a tombstone for those replays.

use std::rc::Rc;

use pdfminer::pdfparser::FileKind;
use pdfplumber::container::{objects_value, Container, SerializeOptions};
use pdfplumber::obj::{bbox_value, Obj, PyValue};
use pdfplumber::page::{CropFn, FilterFn, Page};
use pdfplumber::pdf::{OpenOptions, Source, PDF};
use pdfplumber::table::{CellGroup, Table};
use pdfplumber::utils::text::RegexMatch;
use pycompat::recursion::{eval_entry, Guard};
use pycompat::specialize::{AttrSite, CallSite};
use pycompat::{exc, ExcKind, PyErr, PyResult, PyStr};
use rustc_hash::FxHashMap;

use crate::ops::{arg, group_index, kwargs, list_of_objs, opt_bbox, seq, Core, NamedGroups, Object, Out, Utf16Index};

/// A page handle: the page of a document (`pdf.pages[i]`) or a page
/// derived from one.
pub struct PageObj {
    /// The handle of its document.
    pub doc: u32,
    /// `None` between a reset and the replay that refills it, and for a
    /// page JavaScript freed (outside replays).
    pub page: Option<Rc<Page>>,
    /// Freed by JavaScript: refused to its calls, kept for the replays.
    pub freed: bool,
}

/// A table handle: a table `find_tables` found, with the page it was found
/// on (`Table.page`).
pub struct TableObj {
    pub doc: u32,
    pub table: Option<(Rc<Page>, Rc<Table>)>,
}

/// `pdfplumber.open(io.BytesIO(data), **options)`: the keyword arguments
/// of pdfplumber/pdf.py:73 `PDF.open` (`None`: none), with the
/// `TypeError` of an unknown one; their values as given (pdfplumber checks
/// them where it uses them). `gs_path` and `repair_setting` only matter
/// with `repair=True`, which is out of scope (`NotPorted`).
pub fn open(data: &Rc<[u8]>, options: &PyValue) -> PyResult<PDF> {
    let mut o = OpenOptions::new();
    match options {
        PyValue::None => {}
        PyValue::Dict(d) => {
            for (k, v) in d.items() {
                match k.as_str() {
                    Some("pages") => o.pages = Some(v.clone()),
                    Some("laparams") => o.laparams = Some(v.clone()),
                    Some("password") => o.password = Some(v.clone()),
                    Some("strict_metadata") => o.strict_metadata = v.truthy(),
                    Some("unicode_norm") => o.unicode_norm = Some(v.clone()),
                    Some("raise_unicode_errors") => o.raise_unicode_errors = v.truthy(),
                    Some("repair") => o.repair = v.truthy(),
                    Some("gs_path" | "repair_setting") => {}
                    _ => {
                        return Err(PyErr::with_message(
                            ExcKind::TypeError,
                            &PyStr::new("PDF.open() got an unexpected keyword argument ")
                                .concat(&PyStr::from(k.repr())),
                        ))
                    }
                }
            }
        }
        other => return Err(exc::TypeError(format!("expected keyword arguments, got {}", other.type_name()))),
    }
    PDF::open(Source::Stream { data: data.clone(), kind: FileKind::BytesIO }, o)
}

/// A `list[str]` argument (`object_types`...), `None` for `None`.
fn str_list(v: &PyValue) -> PyResult<Option<Vec<PyStr>>> {
    match v {
        PyValue::None => Ok(None),
        v => seq(v)?
            .iter()
            .map(|s| match s {
                PyValue::Str(s) => Ok(s.clone()),
                other => Err(exc::TypeError(format!("expected a str, got {}", other.type_name()))),
            })
            .collect::<PyResult<Vec<_>>>()
            .map(Some),
    }
}

/// The keyword arguments of `to_json`, `to_csv` and `to_dict`
/// (`object_types`, `include_attrs`, `exclude_attrs`, `precision`,
/// `indent`); `stream` is not one: the text is returned.
fn serialize_options(v: &PyValue, func: &str, names: &[&str]) -> PyResult<SerializeOptions> {
    let k = kwargs(v)?;
    check_keywords(&k, func, names)?;
    let none = PyValue::None;
    let get = |name: &str| k.get(name).unwrap_or(&none);
    Ok(SerializeOptions {
        object_types: str_list(get("object_types"))?,
        include_attrs: str_list(get("include_attrs"))?,
        exclude_attrs: str_list(get("exclude_attrs"))?,
        precision: k.get("precision").cloned().filter(|p| !p.is_none()),
        indent: k.get("indent").cloned().filter(|p| !p.is_none()),
    })
}

/// The `TypeError` of a call of `func` (its `__qualname__`) with a
/// keyword argument it does not take (`names`; `stream` is not one here).
fn check_keywords(kwargs: &Obj, func: &str, names: &[&str]) -> PyResult<()> {
    for (name, _) in kwargs.items() {
        if !names.iter().any(|n| *name == **n) {
            return Err(PyErr::with_message(
                ExcKind::TypeError,
                &PyStr::new(&format!("{func}() got an unexpected keyword argument ")).concat(&PyStr::from(name.repr())),
            ));
        }
    }
    Ok(())
}

const TO_DICT: &[&str] = &["object_types"];
const TO_JSON: &[&str] = &["object_types", "include_attrs", "exclude_attrs", "precision", "indent"];
const TO_CSV: &[&str] = &["object_types", "precision", "include_attrs", "exclude_attrs"];

/// `TableSettings.resolve(settings)` of a `None` or a dict.
fn table_settings(v: &PyValue) -> PyResult<Option<Obj>> {
    match v {
        PyValue::None => Ok(None),
        v => kwargs(v).map(Some),
    }
}

fn group_value(g: &CellGroup) -> PyValue {
    PyValue::Dict(Obj::from_pairs([
        ("bbox", bbox_value(&g.bbox)),
        ("cells", PyValue::list(g.cells.iter().map(opt_bbox))),
    ]))
}

/// A table as a value: `bbox`, `cells`, `rows`, `columns` (each a
/// `CellGroup`'s `bbox` and `cells`), and its handle.
fn table_value(handle: PyValue, t: &Table) -> PyResult<PyValue> {
    Ok(PyValue::Dict(Obj::from_pairs([
        ("handle", handle),
        ("bbox", bbox_value(&t.bbox()?)),
        ("cells", PyValue::list(t.cells.iter().map(bbox_value))),
        ("rows", PyValue::list(t.rows()?.iter().map(group_value))),
        ("columns", PyValue::list(t.columns()?.iter().map(group_value))),
    ])))
}

/// `Table.extract()`'s rows (`None` where there is no cell).
fn rows_value(rows: Vec<Vec<Option<PyStr>>>) -> PyValue {
    PyValue::list(
        rows.into_iter().map(|row| PyValue::list(row.into_iter().map(|c| c.map_or(PyValue::None, PyValue::Str)))),
    )
}

/// The geometry of a page: what `Page.__init__` sets (`page_number`,
/// `initial_doctop`, `rotation`, `mediabox`, `cropbox`, the optional
/// boxes when the page has them, `bbox`), and `width`/`height`.
fn page_geometry(page: &Page, width: &AttrSite, height: &AttrSite) -> PyResult<Obj> {
    let mut d = Obj::from_pairs([
        ("page_number", PyValue::Int(page.page_number.clone())),
        ("initial_doctop", page.initial_doctop.clone()),
        ("rotation", page.rotation.clone()),
        ("mediabox", page.mediabox.clone()),
        ("cropbox", page.cropbox.clone()),
    ]);
    for (name, b) in [("trimbox", &page.trimbox), ("bleedbox", &page.bleedbox), ("artbox", &page.artbox)] {
        if let Some(b) = b {
            d.set(name, b.clone());
        }
    }
    d.set("bbox", page.bbox.clone());
    d.set("width", width.get(page.class_name(), || page.width())?);
    d.set("height", height.get(page.class_name(), || page.height())?);
    Ok(d)
}

/// A derived page as JavaScript receives it: its handle, `page_number`
/// and `bbox` (its `width` and `height`, which may raise for a box given
/// as is, are `page_info`'s).
fn derived_value(handle: PyValue, page: &Page) -> PyValue {
    PyValue::Dict(Obj::from_pairs([
        ("handle", handle),
        ("page_number", PyValue::Int(page.page_number.clone())),
        ("bbox", page.bbox.clone()),
    ]))
}

/// The test function of a `FilteredPage` made from a JavaScript mask: the
/// truth of each object of the parent's `objects`, by kind, in order.
fn mask_filter(parent: &Page, mask: &PyValue) -> PyResult<FilterFn> {
    let mask = kwargs(mask)?;
    let objects = parent.objects()?;
    let mut kept: Vec<(Obj, bool)> = Vec::new();
    for (kind, objs) in objects.iter() {
        let truth = match mask.get(kind) {
            Some(v) => seq(v)?,
            None => &[],
        };
        if truth.len() != objs.len() {
            return Err(exc::ValueError(format!(
                "the mask of {:?} has {} values for {} objects",
                kind.to_string_lossy(),
                truth.len(),
                objs.len()
            )));
        }
        kept.extend(objs.iter().cloned().zip(truth.iter().map(PyValue::truthy)));
    }
    // the objects are kept alive: their ids cannot be reused
    let index: FxHashMap<usize, usize> = kept.iter().enumerate().map(|(i, (o, _))| (o.id(), i)).collect();
    Ok(Rc::new(move |o: &Obj| match index.get(&o.id()) {
        Some(&i) => Ok(PyValue::Bool(kept[i].1)),
        None => Err(PyErr::new(
            ExcKind::NotPorted,
            "not modelled: Page.filter's function on objects computed after the filter was made \
             (JavaScript evaluated it on the parent's objects it had then)",
        )),
    }))
}

// The sites of the statements of `oracle/unit/js_pages_run.py`
// (`pycompat::specialize`): one per operation, as one statement of a
// script runs every call of that operation.
static DOC_PAGES: AttrSite = AttrSite::new("js doc_pages obj.pages");
static DOC_PAGES_WIDTH: AttrSite = AttrSite::new("js doc_pages p.width");
static DOC_PAGES_HEIGHT: AttrSite = AttrSite::new("js doc_pages p.height");
static DOC_OBJECTS: AttrSite = AttrSite::new("js doc_objects obj.objects");
static DOC_ATTR: CallSite = CallSite::new("js doc_attr getattr(obj, name)");
static DOC_ANNOTS: AttrSite = AttrSite::new("js doc_annots obj.annots");
static DOC_HYPERLINKS: AttrSite = AttrSite::new("js doc_hyperlinks obj.hyperlinks");
static DOC_STRUCTURE_TREE: AttrSite = AttrSite::new("js doc_structure_tree obj.structure_tree");
static PAGE_INFO_WIDTH: AttrSite = AttrSite::new("js page_info obj.width");
static PAGE_INFO_HEIGHT: AttrSite = AttrSite::new("js page_info obj.height");
static PAGE_OBJECTS: AttrSite = AttrSite::new("js page_objects obj.objects");
static PAGE_ATTR: CallSite = CallSite::new("js page_attr getattr(obj, name)");
static PAGE_STRUCTURE_TREE: AttrSite = AttrSite::new("js page_structure_tree obj.structure_tree");

/// `obj.method(*args, **kwargs)`: `CALL_FUNCTION_EX` calls the bound
/// method through `PyObject_Call`, which enters its frame from C (the two
/// units of an evaluation loop entry, held while it runs).
fn method_call() -> PyResult<Guard> {
    eval_entry()
}

/// A `str` argument.
fn str_arg(v: &PyValue) -> PyResult<PyStr> {
    match v {
        PyValue::Str(s) => Ok(s.clone()),
        other => Err(exc::TypeError(format!("expected a str, got {}", other.type_name()))),
    }
}

impl Core {
    /// The `PDF` of the document `h` (the operation reaches it).
    fn pdf(&mut self, h: u32) -> PyResult<Rc<PDF>> {
        let pdf = match self.handles.get(h) {
            Some(Object::Document(d)) => d.state.pdf.clone(),
            _ => return Err(exc::ValueError("invalid handle")),
        };
        self.reach(h)?;
        pdf.ok_or_else(|| PyErr::new(ExcKind::RuntimeError, "the document was not rebuilt"))
    }

    /// The page of the page handle `h` (the operation reaches its
    /// document); a freed page only while replaying.
    fn page(&mut self, h: u32) -> PyResult<Rc<Page>> {
        let (doc, page) = match self.handles.get(h) {
            Some(Object::Page(p)) if !p.freed || self.replay.is_some() => (p.doc, p.page.clone()),
            _ => return Err(exc::ValueError("invalid handle")),
        };
        self.reach(doc)?;
        page.ok_or_else(|| PyErr::new(ExcKind::RuntimeError, "the page was not rebuilt"))
    }

    /// The document of the page handle `h`.
    fn page_doc(&self, h: u32) -> u32 {
        match self.handles.get(h) {
            Some(Object::Page(p)) => p.doc,
            _ => 0,
        }
    }

    /// A new page handle of document `doc`.
    fn insert_page(&mut self, doc: u32, page: Rc<Page>) -> PyResult<PyValue> {
        self.insert(Object::Page(PageObj { doc, page: Some(page), freed: false }))
    }

    /// The operations of documents, pages and tables; `None` for a name
    /// that is none of them. Each is the statement of
    /// `oracle/unit/js_pages_run.py`: a property load at its own site, a
    /// method called with `*args, **kwargs` (`CALL_FUNCTION_EX`: the bound
    /// method from C, [`method_call`]), `getattr` of a name.
    pub(crate) fn page_op(&mut self, name: &str, h: u32, args: &PyValue) -> PyResult<Option<Out>> {
        let a = || seq(args);
        let value = match name {
            // documents (`h`: the document)
            "doc_metadata" => self.pdf(h)?.metadata_value(),
            "doc_pages" => {
                // pages = pdf.pages, then each page's handle and geometry
                let pdf = self.pdf(h)?;
                let pages = DOC_PAGES.get("PDF", || pdf.pages())?;
                let mut out = Vec::with_capacity(pages.len());
                for p in pages {
                    let mut d = Obj::from_pairs([("handle", self.insert_page(h, p.clone())?)]);
                    for (k, v) in page_geometry(&p, &DOC_PAGES_WIDTH, &DOC_PAGES_HEIGHT)?.items() {
                        d.set(k, v.clone());
                    }
                    out.push(PyValue::Dict(d));
                }
                PyValue::list(out)
            }
            "doc_objects" => {
                let pdf = self.pdf(h)?;
                objects_value(&*DOC_OBJECTS.get("PDF", || pdf.objects())?)
            }
            "doc_attr" => {
                // getattr(pdf, name): the lists by kind, the edges
                let name = str_arg(arg(a()?, 0)?)?;
                pdfplumber::container::getattr(&*self.pdf(h)?, &name, &DOC_ATTR)?
            }
            "doc_annots" => {
                let pdf = self.pdf(h)?;
                list_of_objs(DOC_ANNOTS.get("PDF", || pdf.annots())?)
            }
            "doc_hyperlinks" => {
                let pdf = self.pdf(h)?;
                list_of_objs(DOC_HYPERLINKS.get("PDF", || pdf.hyperlinks())?)
            }
            "doc_structure_tree" => {
                let pdf = self.pdf(h)?;
                PyValue::list(DOC_STRUCTURE_TREE.get("PDF", || pdf.structure_tree())?)
            }
            "doc_to_dict" => {
                let pdf = self.pdf(h)?;
                let _call = method_call()?;
                let o = serialize_options(arg(a()?, 0)?, "PDF.to_dict", TO_DICT)?;
                PyValue::Dict(pdf.to_dict(o.object_types.as_deref())?)
            }
            "doc_to_json" => {
                let pdf = self.pdf(h)?;
                let _call = method_call()?;
                PyValue::Str(pdf.to_json(&serialize_options(arg(a()?, 0)?, "Container.to_json", TO_JSON)?)?)
            }
            "doc_to_csv" => {
                let pdf = self.pdf(h)?;
                let _call = method_call()?;
                PyValue::Str(pdf.to_csv(&serialize_options(arg(a()?, 0)?, "Container.to_csv", TO_CSV)?)?)
            }
            "doc_close" => {
                let pdf = self.pdf(h)?;
                let _call = method_call()?;
                pdf.close()?;
                PyValue::None
            }
            // pages (`h`: the page)
            "page_info" => PyValue::Dict(page_geometry(&*self.page(h)?, &PAGE_INFO_WIDTH, &PAGE_INFO_HEIGHT)?),
            "page_objects" => {
                let page = self.page(h)?;
                objects_value(&*PAGE_OBJECTS.get(page.class_name(), || page.objects())?)
            }
            "page_attr" => {
                // getattr(page, name): chars, rects, ..., edges, annots,
                // hyperlinks, objects
                let name = str_arg(arg(a()?, 0)?)?;
                pdfplumber::container::getattr(&*self.page(h)?, &name, &PAGE_ATTR)?
            }
            "page_structure_tree" => {
                let page = self.page(h)?;
                PyValue::list(PAGE_STRUCTURE_TREE.get(page.class_name(), || page.structure_tree())?)
            }
            "page_extract_text" => {
                let page = self.page(h)?;
                let _call = method_call()?;
                PyValue::Str(page.extract_text(&kwargs(arg(a()?, 0)?)?)?)
            }
            "page_extract_text_simple" => {
                let page = self.page(h)?;
                let _call = method_call()?;
                PyValue::Str(page.extract_text_simple(&kwargs(arg(a()?, 0)?)?)?)
            }
            "page_extract_words" => {
                let page = self.page(h)?;
                let _call = method_call()?;
                list_of_objs(page.extract_words(&kwargs(arg(a()?, 0)?)?)?)
            }
            "page_extract_text_lines" => {
                // [strip, return_chars, kwargs]
                let a = a()?;
                let page = self.page(h)?;
                let _call = method_call()?;
                let (strip, chars) = (arg(a, 0)?.truthy(), arg(a, 1)?.truthy());
                list_of_objs(page.extract_text_lines(strip, chars, &kwargs(arg(a, 2)?)?)?)
            }
            "page_search_text" => {
                // the first half of `page.search(pattern, **options)`
                let page = self.page(h)?;
                let _call = method_call()?;
                PyValue::Str(page.search_text(&kwargs(arg(a()?, 0)?)?)?)
            }
            "page_search" => {
                // [kwargs, matches, return_groups, return_chars, main_group,
                // named], as `textmap_search` (crate::ops) takes them
                let a = a()?;
                let page = self.page(h)?;
                let raw_matches = seq(arg(a, 1)?)?;
                let none = PyValue::None;
                let named_arg = a.get(5).unwrap_or(&none);
                // the named groups of each match, read with the matches
                // (inside `search`, before the main group of any of them)
                let named = std::cell::RefCell::new(None::<Vec<NamedGroups>>);
                let mut read = |text: &PyStr| -> PyResult<Vec<RegexMatch>> {
                    let index = Utf16Index::new(text);
                    let mut matches = Vec::with_capacity(raw_matches.len());
                    for m in raw_matches {
                        let spans = seq(m)?.iter().map(|s| index.span(s)).collect::<PyResult<Vec<_>>>()?;
                        matches.push(RegexMatch { spans });
                    }
                    if !named_arg.is_none() {
                        let groups = seq(named_arg)?
                            .iter()
                            .map(|g| {
                                kwargs(g)?
                                    .items()
                                    .iter()
                                    .map(|(k, s)| Ok((k.clone(), index.span(s)?)))
                                    .collect::<PyResult<NamedGroups>>()
                            })
                            .collect::<PyResult<Vec<_>>>()?;
                        if groups.len() != matches.len() {
                            return Err(exc::ValueError("one dict of named groups per match"));
                        }
                        *named.borrow_mut() = Some(groups);
                    }
                    Ok(matches)
                };
                let main_group = arg(a, 4)?.clone();
                let (return_groups, return_chars) = (arg(a, 2)?.truthy(), arg(a, 3)?.truthy());
                let group = |k: usize, m: &RegexMatch| {
                    let named = named.borrow();
                    group_index(&main_group, m, named.as_ref().map(|n| &n[k]))
                };
                let kw = kwargs(arg(a, 0)?)?;
                let _call = method_call()?;
                list_of_objs(page.search_with(&mut read, return_groups, return_chars, &group, &kw)?)
            }
            "page_crop" | "page_within_bbox" | "page_outside_bbox" => {
                // page.crop(bbox, **kwargs): [bbox, {relative, strict}]
                let a = a()?;
                let (crop_fn, func) = match name {
                    "page_crop" => (CropFn::CropToBbox, "Page.crop"),
                    "page_within_bbox" => (CropFn::WithinBbox, "Page.within_bbox"),
                    _ => (CropFn::OutsideBbox, "Page.outside_bbox"),
                };
                let page = self.page(h)?;
                let _call = method_call()?;
                let kw = kwargs(a.get(1).unwrap_or(&PyValue::None))?;
                check_keywords(&kw, func, &["relative", "strict"])?;
                let relative = kw.get("relative").is_some_and(PyValue::truthy);
                let strict = kw.get("strict").is_none_or(PyValue::truthy);
                let derived = page.crop(arg(a, 0)?.clone(), crop_fn, relative, strict)?;
                let handle = self.insert_page(self.page_doc(h), derived.clone())?;
                derived_value(handle, &derived)
            }
            "page_filter" => {
                // page.filter(test): [mask], {kind: [truth of each object]}
                let page = self.page(h)?;
                let test = mask_filter(&page, arg(a()?, 0)?)?;
                let derived = page.filter(test)?;
                let handle = self.insert_page(self.page_doc(h), derived.clone())?;
                derived_value(handle, &derived)
            }
            "page_dedupe_chars" => {
                let page = self.page(h)?;
                let _call = method_call()?;
                let derived = page.dedupe_chars(&kwargs(arg(a()?, 0)?)?)?;
                let handle = self.insert_page(self.page_doc(h), derived.clone())?;
                derived_value(handle, &derived)
            }
            "page_find_tables" => {
                // page.find_tables(*args): [] or [settings]
                let page = self.page(h)?;
                let _call = method_call()?;
                let tables = page.find_tables(table_settings(a()?.first().unwrap_or(&PyValue::None))?.as_ref())?;
                let doc = self.page_doc(h);
                let mut out = Vec::with_capacity(tables.len());
                for t in tables {
                    let t = Rc::new(t);
                    let handle =
                        self.insert(Object::Table(TableObj { doc, table: Some((page.clone(), t.clone())) }))?;
                    out.push(table_value(handle, &t)?);
                }
                PyValue::list(out)
            }
            "page_find_table" => {
                let page = self.page(h)?;
                let found = {
                    let _call = method_call()?;
                    page.find_table(table_settings(a()?.first().unwrap_or(&PyValue::None))?.as_ref())?
                };
                match found {
                    None => PyValue::None,
                    Some(t) => {
                        let t = Rc::new(t);
                        let doc = self.page_doc(h);
                        let handle = self.insert(Object::Table(TableObj { doc, table: Some((page, t.clone())) }))?;
                        table_value(handle, &t)?
                    }
                }
            }
            "page_extract_tables" => {
                let page = self.page(h)?;
                let _call = method_call()?;
                let tables = page.extract_tables(table_settings(a()?.first().unwrap_or(&PyValue::None))?.as_ref())?;
                PyValue::list(tables.into_iter().map(rows_value))
            }
            "page_extract_table" => {
                let page = self.page(h)?;
                let _call = method_call()?;
                match page.extract_table(table_settings(a()?.first().unwrap_or(&PyValue::None))?.as_ref())? {
                    None => PyValue::None,
                    Some(rows) => rows_value(rows),
                }
            }
            "page_to_dict" => {
                let page = self.page(h)?;
                let _call = method_call()?;
                let o = serialize_options(arg(a()?, 0)?, "Page.to_dict", TO_DICT)?;
                PyValue::Dict(page.to_dict(o.object_types.as_deref())?)
            }
            "page_to_json" => {
                let page = self.page(h)?;
                let _call = method_call()?;
                PyValue::Str(page.to_json(&serialize_options(arg(a()?, 0)?, "Container.to_json", TO_JSON)?)?)
            }
            "page_to_csv" => {
                let page = self.page(h)?;
                let _call = method_call()?;
                PyValue::Str(page.to_csv(&serialize_options(arg(a()?, 0)?, "Container.to_csv", TO_CSV)?)?)
            }
            "page_close" => {
                let page = self.page(h)?;
                let _call = method_call()?;
                page.close()?;
                PyValue::None
            }
            // tables (`h`: the table)
            "table_extract" => {
                // table.extract(**kwargs)
                let (doc, table) = match self.handles.get(h) {
                    Some(Object::Table(t)) => (t.doc, t.table.clone()),
                    _ => return Err(exc::ValueError("invalid handle")),
                };
                self.reach(doc)?;
                let (page, table) =
                    table.ok_or_else(|| PyErr::new(ExcKind::RuntimeError, "the table was not rebuilt"))?;
                let _call = method_call()?;
                rows_value(table.extract(&*page, &kwargs(arg(a()?, 0)?)?)?)
            }
            _ => return Ok(None),
        };
        Ok(Some(Out::Value(value)))
    }
}
