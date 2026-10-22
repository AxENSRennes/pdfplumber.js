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
//! a void call (`crate::ops`, "Void calls"); a page or table JavaScript
//! freed is made again by those replays, out of the handle table
//! (`crate::ops`, "Freed handles").

use std::rc::Rc;

use pdfminer::pdfparser::FileKind;
use pdfplumber::container::{objects_value, Container, SerializeOptions};
use pdfplumber::obj::{bbox_value, Obj, PyValue};
use pdfplumber::page::{CropFn, FilterFn, Page};
use pdfplumber::pdf::{OpenOptions, Source, PDF};
use pdfplumber::table::{CellGroup, Table, TableSettings};
use pdfplumber::utils::text::RegexMatch;
use pycompat::call::Signature;
use pycompat::recursion::{eval_entry, py_frame, Guard};
use pycompat::specialize::{AttrSite, CallSite};
use pycompat::{exc, ExcKind, PyErr, PyResult, PyStr};
use rustc_hash::FxHashMap;

use crate::errors::write_error;
use crate::ops::{arg, group_index, kwargs, list_of_objs, opt_bbox, seq, Core, NamedGroups, Object, Out, Utf16Index};
use crate::wire::write_value;

/// A page handle: the page of a document (`pdf.pages[i]`) or a page
/// derived from one.
pub struct PageObj {
    /// The handle of its document.
    pub doc: u32,
    /// `None` between a reset and the replay that refills it.
    pub page: Option<Rc<Page>>,
}

/// A table handle: a table `find_tables` found, with the page it was found
/// on (`Table.page`).
pub struct TableObj {
    pub doc: u32,
    /// `None` between a reset and the replay that refills it.
    pub table: Option<(Rc<Page>, Rc<Table>)>,
}

// pdfplumber/pdf.py:73 PDF.open (a classmethod: `cls` is bound)
const OPEN: Signature<'static> = Signature {
    qualname: "PDF.open",
    bound: &["cls", "path_or_fp"],
    keywords: Some(&[
        "pages",
        "laparams",
        "password",
        "strict_metadata",
        "unicode_norm",
        "repair",
        "gs_path",
        "repair_setting",
        "raise_unicode_errors",
    ]),
};

/// `pdfplumber.open(io.BytesIO(data), **(options or {}))`: the keyword
/// arguments of pdfplumber/pdf.py:73 `PDF.open`, bound as CPython binds
/// them (`pycompat::call`); their values as given (pdfplumber checks them
/// where it uses them). `gs_path` and `repair_setting` only matter with
/// `repair=True`, which is out of scope (`NotPorted`).
pub fn open(data: &Rc<[u8]>, options: &PyValue) -> PyResult<PDF> {
    open_source(Source::Stream { data: data.clone(), kind: FileKind::BytesIO }, options, None)
}

/// `pdfplumber.open(path, **(options or {}))`: the file at `path` (whose
/// bytes JavaScript read, Node's `open(path)`), opened by the `PDF`
/// itself (`Source::Path`: `close()` closes it, and a later read raises
/// `ValueError("seek of closed file")`, as in Python).
pub fn open_file(data: &Rc<[u8]>, path: &PyValue, options: &PyValue) -> PyResult<PDF> {
    let PyValue::Str(path) = path else {
        return Err(exc::TypeError(format!("expected a str path, got {}", path.type_name())));
    };
    open_source(Source::Path { data: data.clone(), path: path.clone() }, options, None)
}

/// [`open`] and [`open_file`]; `ids`: the `IdSource` of each page's analysis
/// (the oracle hooks of the harness, `crate::oracle`), `CreationOrder`
/// otherwise.
pub fn open_source(source: Source, options: &PyValue, ids: Option<pdfplumber::pdf::IdsFactory>) -> PyResult<PDF> {
    let mut o = OpenOptions::new();
    o.ids = ids;
    let options = if options.truthy() { star_kwargs(options, "pdfplumber.pdf.PDF.open")? } else { Obj::new() };
    bind(OPEN, &options)?;
    for (k, v) in options.items() {
        match k.as_str() {
            Some("pages") => o.pages = Some(v.clone()),
            Some("laparams") => o.laparams = Some(v.clone()),
            Some("password") => o.password = Some(v.clone()),
            Some("strict_metadata") => o.strict_metadata = v.truthy(),
            Some("unicode_norm") => o.unicode_norm = Some(v.clone()),
            Some("raise_unicode_errors") => o.raise_unicode_errors = v.truthy(),
            Some("repair") => o.repair = v.truthy(),
            // gs_path, repair_setting
            _ => {}
        }
    }
    PDF::open(source, o)
}

/// The mapping of `f(**v)`: a dict, else CPython's `TypeError` (`func`:
/// the function's module and qualified name, as `_PyObject_FunctionStr`
/// writes it), raised before the call.
fn star_kwargs(v: &PyValue, func: &str) -> PyResult<Obj> {
    match v {
        PyValue::Dict(d) => Ok(d.clone()),
        other => {
            Err(exc::TypeError(format!("{func}() argument after ** must be a mapping, not {}", other.type_name())))
        }
    }
}

/// Binds the keyword arguments of a call (`pycompat::call`).
fn bind(signature: Signature<'_>, kwargs: &Obj) -> PyResult<()> {
    signature.bind(kwargs.items().iter().map(|(k, _)| k))
}

/// The signature of a method `Class.name(self, **kwargs)`.
const fn method(qualname: &str) -> Signature<'_> {
    Signature { qualname, bound: &["self"], keywords: None }
}

/// `TypeError` for a call of `qualname` (a method) with more positional
/// arguments than its `max` parameters after `self` (CPython's message).
fn check_arity(qualname: &str, given: usize, min: usize, max: usize) -> PyResult<()> {
    if given <= max {
        return Ok(());
    }
    let takes = if min == max {
        format!("{} positional argument{}", max + 1, if max == 0 { "" } else { "s" })
    } else {
        format!("from {} to {} positional arguments", min + 1, max + 1)
    };
    Err(exc::TypeError(format!("{qualname}() takes {takes} but {} were given", given + 1)))
}

/// The options of `to_json`, `to_csv` and `to_dict`, bound to the
/// method's parameters as given: `stream=None` returns the text (another
/// stream is not modelled), the other values go to the ported code as
/// they are, which raises where Python raises (`t + "s"`,
/// `CSV_COLS_REQUIRED + include_attrs`, `set(exclude_attrs)`).
fn serialize_options(v: &PyValue, func: &str, signature: Signature<'_>) -> PyResult<SerializeOptions> {
    let k = star_kwargs(v, func)?;
    bind(signature, &k)?;
    if k.get("stream").is_some_and(|s| !s.is_none()) {
        return Err(PyErr::new(
            ExcKind::NotPorted,
            "not modelled: to_json/to_csv into a stream (stream=None returns the text)",
        ));
    }
    let none = PyValue::None;
    let get = |name: &str| k.get(name).unwrap_or(&none).clone();
    Ok(SerializeOptions {
        object_types: get("object_types"),
        include_attrs: get("include_attrs"),
        exclude_attrs: get("exclude_attrs"),
        precision: k.get("precision").cloned().filter(|p| !p.is_none()),
        indent: k.get("indent").cloned().filter(|p| !p.is_none()),
    })
}

// pdfplumber/page.py:617 Page.to_dict, pdfplumber/pdf.py:201 PDF.to_dict
const fn to_dict(qualname: &str) -> Signature<'_> {
    Signature { qualname, bound: &["self"], keywords: Some(&["object_types"]) }
}
// pdfplumber/container.py:108 Container.to_json
const TO_JSON: Signature<'static> = Signature {
    qualname: "Container.to_json",
    bound: &["self"],
    keywords: Some(&["stream", "object_types", "include_attrs", "exclude_attrs", "precision", "indent"]),
};
// pdfplumber/container.py:132 Container.to_csv
const TO_CSV: Signature<'static> = Signature {
    qualname: "Container.to_csv",
    bound: &["self"],
    keywords: Some(&["stream", "object_types", "precision", "include_attrs", "exclude_attrs"]),
};
// pdfplumber/page.py:538 Page.extract_text_lines, called with `strip` and
// `return_chars` as positional arguments
const EXTRACT_TEXT_LINES: Signature<'static> =
    Signature { qualname: "Page.extract_text_lines", bound: &["self", "strip", "return_chars"], keywords: None };
// pdfplumber/page.py:509 Page.search, called with `pattern` (the other
// search options are taken apart by JavaScript)
const SEARCH: Signature<'static> = Signature {
    qualname: "Page.search",
    bound: &["self", "pattern", "regex", "case", "main_group", "return_chars", "return_groups"],
    keywords: None,
};

/// `table_settings` of `find_tables(*args)` and the other table methods:
/// at most one positional argument, `None` when there is none.
fn table_settings(args: &[PyValue], qualname: &str) -> PyResult<PyValue> {
    check_arity(qualname, args.len(), 0, 1)?;
    Ok(args.first().cloned().unwrap_or(PyValue::None))
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

/// The fields of a resolved `TableSettings` (pdfplumber/table.py:486, a
/// dataclass), in order: `{f.name: getattr(s, f.name) for f in
/// dataclasses.fields(s)}` (`__post_init__` set every `UNSET` one).
fn settings_value(s: &TableSettings) -> Obj {
    let opt = |v: &Option<PyValue>| v.clone().unwrap_or(PyValue::None);
    Obj::from_pairs([
        ("vertical_strategy", s.vertical_strategy.clone()),
        ("horizontal_strategy", s.horizontal_strategy.clone()),
        ("explicit_vertical_lines", s.explicit_vertical_lines.clone()),
        ("explicit_horizontal_lines", s.explicit_horizontal_lines.clone()),
        ("snap_tolerance", s.snap_tolerance.clone()),
        ("snap_x_tolerance", opt(&s.snap_x_tolerance)),
        ("snap_y_tolerance", opt(&s.snap_y_tolerance)),
        ("join_tolerance", s.join_tolerance.clone()),
        ("join_x_tolerance", opt(&s.join_x_tolerance)),
        ("join_y_tolerance", opt(&s.join_y_tolerance)),
        ("edge_min_length", s.edge_min_length.clone()),
        ("edge_min_length_prefilter", s.edge_min_length_prefilter.clone()),
        ("min_words_vertical", s.min_words_vertical.clone()),
        ("min_words_horizontal", s.min_words_horizontal.clone()),
        ("intersection_tolerance", s.intersection_tolerance.clone()),
        ("intersection_x_tolerance", opt(&s.intersection_x_tolerance)),
        ("intersection_y_tolerance", opt(&s.intersection_y_tolerance)),
        ("text_settings", s.text_settings.clone()),
    ])
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
    let mut d = page_boxes(page);
    d.set("width", width.get(page.class_name(), || page.width())?);
    d.set("height", height.get(page.class_name(), || page.height())?);
    Ok(d)
}

/// What `Page.__init__` sets: [`page_geometry`] without `width` and
/// `height`.
fn page_boxes(page: &Page) -> Obj {
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
    d
}

/// `page_info`: [`page_boxes`], then `width` and `height`, each read in a
/// statement of its own (a property that may raise, as `page.width` does
/// for a crop box kept as given): its value, or its exception under
/// `width_error`/`height_error` (the dict of `crate::errors`).
fn page_info(page: &Page) -> Out {
    let boxes = page_boxes(page);
    let mut read = Vec::new();
    for (name, site) in [("width", &PAGE_INFO_WIDTH), ("height", &PAGE_INFO_HEIGHT)] {
        read.push(match site.get(page.class_name(), || if name == "width" { page.width() } else { page.height() }) {
            Ok(v) => (name.to_owned(), Ok(v)),
            Err(e) => (format!("{name}_error"), Err(e)),
        });
    }
    Out::Written(Box::new(move |w| {
        w.dict(boxes.len() + read.len());
        for (k, v) in boxes.items() {
            crate::wire::write_str(w, k)?;
            write_value(w, v)?;
        }
        for (k, v) in &read {
            w.str(k);
            match v {
                Ok(v) => write_value(w, v)?,
                Err(e) => write_error(w, e),
            }
        }
        Ok(())
    }))
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
/// truth of each object of the parent's `objects`, by kind, in order. The
/// statement's `mask_of(page, name)` (`oracle/unit/js_pages_run.py`): a
/// function's frame, which reads `page.objects` at a site of its own
/// (computed there if no statement computed them before; JavaScript's
/// `filter` computes them with `page_objects` first, to run its function).
fn mask_filter(parent: &Page, mask: &PyValue) -> PyResult<FilterFn> {
    let mask = kwargs(mask)?;
    let objects = {
        let _mask_of = py_frame()?;
        MASK_OF_OBJECTS.get(parent.class_name(), || parent.objects())?
    };
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
static MASK_OF_OBJECTS: AttrSite = AttrSite::new("js page_filter mask_of page.objects");

/// `obj.method(*args, **kwargs)`: `CALL_FUNCTION_EX` calls the bound
/// method through `PyObject_Call`, which enters its frame from C (the two
/// units of an evaluation loop entry, held while it runs).
fn method_call() -> PyResult<Guard> {
    eval_entry()
}

/// A `str` argument.
/// The name of a `doc_attr`/`page_attr`: the operations are the `getattr`
/// of `to_dict` and `to_csv`, whose names end with `s` (`t + "s"`); any
/// other attribute (`width`, `bbox`, `layout`...) is not modelled.
fn attr_name(v: &PyValue) -> PyResult<PyStr> {
    let name = str_arg(v)?;
    if name.as_str().is_some_and(|n| n.ends_with('s')) {
        Ok(name)
    } else {
        Err(PyErr::new(
            ExcKind::NotPorted,
            format!("not modelled: getattr of {} (the names to_dict and to_csv build end with 's')", name.repr()),
        ))
    }
}

fn str_arg(v: &PyValue) -> PyResult<PyStr> {
    match v {
        PyValue::Str(s) => Ok(s.clone()),
        other => Err(exc::TypeError(format!("expected a str, got {}", other.type_name()))),
    }
}

impl Core {
    /// The `PDF` of the document `h` (the operation reaches it).
    pub(crate) fn pdf(&mut self, h: u32) -> PyResult<Rc<PDF>> {
        let pdf = match self.handles.get(h) {
            Some(Object::Document(d)) => d.state.pdf.clone(),
            _ => return Err(exc::ValueError("invalid handle")),
        };
        self.reach(h)?;
        pdf.ok_or_else(|| PyErr::new(ExcKind::RuntimeError, "the document was not rebuilt"))
    }

    /// The page of the page handle `h` (the operation reaches its
    /// document).
    pub(crate) fn page(&mut self, h: u32) -> PyResult<Rc<Page>> {
        let (doc, page) = match self.handles.get(h) {
            Some(Object::Page(p)) => (p.doc, p.page.clone()),
            _ => return Err(exc::ValueError("invalid handle")),
        };
        self.reach(doc)?;
        page.ok_or_else(|| PyErr::new(ExcKind::RuntimeError, "the page was not rebuilt"))
    }

    /// The document of the page handle `h`.
    pub(crate) fn page_doc(&self, h: u32) -> u32 {
        match self.handles.get(h) {
            Some(Object::Page(p)) => p.doc,
            _ => 0,
        }
    }

    /// A new page handle of document `doc`.
    fn insert_page(&mut self, doc: u32, page: Rc<Page>) -> PyResult<PyValue> {
        self.insert(Object::Page(PageObj { doc, page: Some(page) }))
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
                // getattr(pdf, name): the lists by kind, the edges, pages
                let name = attr_name(arg(a()?, 0)?)?;
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
                let o = serialize_options(arg(a()?, 0)?, "pdfplumber.pdf.PDF.to_dict", to_dict("PDF.to_dict"))?;
                let _call = method_call()?;
                PyValue::Dict(pdf.to_dict(&o.object_types)?)
            }
            "doc_to_json" => {
                let pdf = self.pdf(h)?;
                let o = serialize_options(arg(a()?, 0)?, "pdfplumber.container.Container.to_json", TO_JSON)?;
                let _call = method_call()?;
                PyValue::Str(pdf.to_json(&o)?)
            }
            "doc_to_csv" => {
                let pdf = self.pdf(h)?;
                let o = serialize_options(arg(a()?, 0)?, "pdfplumber.container.Container.to_csv", TO_CSV)?;
                let _call = method_call()?;
                PyValue::Str(pdf.to_csv(&o)?)
            }
            "doc_close" => {
                let pdf = self.pdf(h)?;
                check_arity("PDF.close", a()?.len(), 0, 0)?;
                let _call = method_call()?;
                pdf.close()?;
                PyValue::None
            }
            // pages (`h`: the page)
            "page_info" => return Ok(Some(page_info(&*self.page(h)?))),
            "page_objects" => {
                let page = self.page(h)?;
                objects_value(&*PAGE_OBJECTS.get(page.class_name(), || page.objects())?)
            }
            "page_attr" => {
                // getattr(page, name): chars, rects, ..., edges, annots,
                // hyperlinks, objects
                let name = attr_name(arg(a()?, 0)?)?;
                pdfplumber::container::getattr(&*self.page(h)?, &name, &PAGE_ATTR)?
            }
            "page_structure_tree" => {
                let page = self.page(h)?;
                PyValue::list(PAGE_STRUCTURE_TREE.get(page.class_name(), || page.structure_tree())?)
            }
            "page_extract_text" => {
                let page = self.page(h)?;
                let kw = star_kwargs(arg(a()?, 0)?, "pdfplumber.page.Page.extract_text")?;
                bind(method("Page.extract_text"), &kw)?;
                let _call = method_call()?;
                PyValue::Str(page.extract_text(&kw)?)
            }
            "page_extract_text_simple" => {
                let page = self.page(h)?;
                let kw = star_kwargs(arg(a()?, 0)?, "pdfplumber.page.Page.extract_text_simple")?;
                bind(method("Page.extract_text_simple"), &kw)?;
                let _call = method_call()?;
                PyValue::Str(page.extract_text_simple(&kw)?)
            }
            "page_extract_words" => {
                let page = self.page(h)?;
                let kw = star_kwargs(arg(a()?, 0)?, "pdfplumber.page.Page.extract_words")?;
                bind(method("Page.extract_words"), &kw)?;
                let _call = method_call()?;
                list_of_objs(page.extract_words(&kw)?)
            }
            "page_extract_text_lines" => {
                // [strip, return_chars, kwargs]
                let a = a()?;
                let page = self.page(h)?;
                let kw = star_kwargs(arg(a, 2)?, "pdfplumber.page.Page.extract_text_lines")?;
                bind(EXTRACT_TEXT_LINES, &kw)?;
                let _call = method_call()?;
                let (strip, chars) = (arg(a, 0)?.truthy(), arg(a, 1)?.truthy());
                list_of_objs(page.extract_text_lines(strip, chars, &kw)?)
            }
            "page_search_text" => {
                // the first half of `page.search(pattern, **options)`
                let page = self.page(h)?;
                let kw = star_kwargs(arg(a()?, 0)?, "pdfplumber.page.Page.search")?;
                bind(SEARCH, &kw)?;
                let _call = method_call()?;
                PyValue::Str(page.search_text(&kw)?)
            }
            "page_search" => {
                // [kwargs, matches, return_groups, return_chars, main_group,
                // named, code_points], as `textmap_search` (crate::ops)
                // takes them; `code_points` true (the harness of js/trace,
                // which gives Python's matches): the offsets are Python's
                // code point offsets, not JavaScript's UTF-16 ones
                let a = a()?;
                let page = self.page(h)?;
                let raw_matches = seq(arg(a, 1)?)?;
                let none = PyValue::None;
                let named_arg = a.get(5).unwrap_or(&none);
                let code_points = a.get(6).is_some_and(PyValue::truthy);
                // the named groups of each match, read with the matches
                // (inside `search`, before the main group of any of them)
                let named = std::cell::RefCell::new(None::<Vec<NamedGroups>>);
                let mut read = |text: &PyStr| -> PyResult<Vec<RegexMatch>> {
                    let index = if code_points { Utf16Index::identity(text) } else { Utf16Index::new(text) };
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
                let kw = star_kwargs(arg(a, 0)?, "pdfplumber.page.Page.search")?;
                bind(SEARCH, &kw)?;
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
                // obj.crop(bbox, **options): options may be left out
                let kw = match a.get(1) {
                    None => Obj::new(),
                    Some(v) => star_kwargs(v, &format!("pdfplumber.page.{func}"))?,
                };
                let signature =
                    Signature { qualname: func, bound: &["self", "bbox"], keywords: Some(&["relative", "strict"]) };
                bind(signature, &kw)?;
                let _call = method_call()?;
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
                let kw = star_kwargs(arg(a()?, 0)?, "pdfplumber.page.Page.dedupe_chars")?;
                bind(method("Page.dedupe_chars"), &kw)?;
                let _call = method_call()?;
                let derived = page.dedupe_chars(&kw)?;
                let handle = self.insert_page(self.page_doc(h), derived.clone())?;
                derived_value(handle, &derived)
            }
            "page_find_tables" => {
                // page.find_tables(*args): [] or [settings]
                let page = self.page(h)?;
                let settings = table_settings(a()?, "Page.find_tables")?;
                let _call = method_call()?;
                let tables = page.find_tables(&settings)?;
                let doc = self.page_doc(h);
                let mut out = Vec::with_capacity(tables.len());
                for t in tables {
                    let t = Rc::new(t);
                    let table = TableObj { doc, table: Some((page.clone(), t.clone())) };
                    let handle = self.insert(Object::Table(table))?;
                    out.push(table_value(handle, &t)?);
                }
                PyValue::list(out)
            }
            "page_find_table" => {
                let page = self.page(h)?;
                let found = {
                    let settings = table_settings(a()?, "Page.find_table")?;
                    let _call = method_call()?;
                    page.find_table(&settings)?
                };
                match found {
                    None => PyValue::None,
                    Some(t) => {
                        let t = Rc::new(t);
                        let doc = self.page_doc(h);
                        let table = TableObj { doc, table: Some((page, t.clone())) };
                        let handle = self.insert(Object::Table(table))?;
                        table_value(handle, &t)?
                    }
                }
            }
            "page_debug_tablefinder" => {
                // page.debug_tablefinder(*args): the finder's edges,
                // intersections (`[[(x, top), {"v": [...], "h": [...]}],
                // ...]`, the dict's items), cells, tables (each with its
                // handle, as find_tables gives them) and resolved settings
                let page = self.page(h)?;
                let finder = {
                    let settings = table_settings(a()?, "Page.debug_tablefinder")?;
                    let _call = method_call()?;
                    page.debug_tablefinder(&settings)?
                };
                let doc = self.page_doc(h);
                let mut tables = Vec::with_capacity(finder.tables.len());
                for t in finder.tables {
                    let t = Rc::new(t);
                    let table = TableObj { doc, table: Some((page.clone(), t.clone())) };
                    let handle = self.insert(Object::Table(table))?;
                    tables.push(table_value(handle, &t)?);
                }
                let intersections = finder.intersections.points.iter().map(|((x, top), v, h)| {
                    PyValue::list([
                        PyValue::tuple([x.clone(), top.clone()]),
                        PyValue::Dict(Obj::from_pairs([("v", list_of_objs(v.clone())), ("h", list_of_objs(h.clone()))])),
                    ])
                });
                PyValue::Dict(Obj::from_pairs([
                    ("edges", list_of_objs(finder.edges)),
                    ("intersections", PyValue::list(intersections)),
                    ("cells", PyValue::list(finder.cells.iter().map(bbox_value))),
                    ("tables", PyValue::list(tables)),
                    ("settings", PyValue::Dict(settings_value(&finder.settings))),
                ]))
            }
            "page_extract_tables" => {
                let page = self.page(h)?;
                let settings = table_settings(a()?, "Page.extract_tables")?;
                let _call = method_call()?;
                let tables = page.extract_tables(&settings)?;
                PyValue::list(tables.into_iter().map(rows_value))
            }
            "page_extract_table" => {
                let page = self.page(h)?;
                let settings = table_settings(a()?, "Page.extract_table")?;
                let _call = method_call()?;
                match page.extract_table(&settings)? {
                    None => PyValue::None,
                    Some(rows) => rows_value(rows),
                }
            }
            "page_to_dict" => {
                let page = self.page(h)?;
                let o = serialize_options(arg(a()?, 0)?, "pdfplumber.page.Page.to_dict", to_dict("Page.to_dict"))?;
                let _call = method_call()?;
                PyValue::Dict(page.to_dict(&o.object_types)?)
            }
            "page_to_json" => {
                let page = self.page(h)?;
                let o = serialize_options(arg(a()?, 0)?, "pdfplumber.container.Container.to_json", TO_JSON)?;
                let _call = method_call()?;
                PyValue::Str(page.to_json(&o)?)
            }
            "page_to_csv" => {
                let page = self.page(h)?;
                let o = serialize_options(arg(a()?, 0)?, "pdfplumber.container.Container.to_csv", TO_CSV)?;
                let _call = method_call()?;
                PyValue::Str(page.to_csv(&o)?)
            }
            "page_close" => {
                let page = self.page(h)?;
                check_arity("Page.close", a()?.len(), 0, 0)?;
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
                let kw = star_kwargs(arg(a()?, 0)?, "pdfplumber.table.Table.extract")?;
                bind(method("Table.extract"), &kw)?;
                let _call = method_call()?;
                rows_value(table.extract(&*page, &kw)?)
            }
            _ => return Ok(None),
        };
        Ok(Some(Out::Value(value)))
    }
}

/// The handle an insertion returned (0 for an object a replay keeps out of
/// the table).
pub(crate) fn int_handle(v: &PyValue) -> PyResult<i64> {
    match v {
        PyValue::Int(i) => i.as_i64().ok_or_else(|| exc::ValueError("invalid handle")),
        _ => Ok(0),
    }
}
