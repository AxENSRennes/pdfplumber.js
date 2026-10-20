//! Port of `pdfplumber/pdf.py`: `PDF`, what `pdfplumber.open` returns.
//!
//! Workstream PAGE wrote `open`, `close`, `pages`, `objects`, `to_dict`;
//! the metadata ([`read_metadata`]), `annots`, `hyperlinks` and
//! `structure_tree` are workstream DOC's (`docs/architecture.md`, "Vague
//! 4").
//!
//! Out of scope (`docs/public-api.md`): `repair=True` (pdfplumber/repair.py
//! runs Ghostscript in a subprocess: `PDF::open` refuses it with
//! `NotPorted`), `gs_path`, `repair_setting`. Not modelled (documented
//! deviations): a `laparams` margin that is not a number (Python stores
//! any object and fails at its first use in the analysis; the port refuses
//! it at `open` with `NotPorted`), a `password` that is neither `None` nor
//! a `str` (`NotPorted`). After `close()`, the file of a `PDF` opened from
//! a path is closed (`PDFDocument::close_file`): a read of the document
//! raises `ValueError: seek of closed file`, as in Python.
//!
//! # State
//!
//! A `PDF` is the document (`PDFDocument`, `PDFResourceManager`, the
//! options) and its cached properties. What pages need of it is in
//! [`PdfCore`], which every [`Page`] holds (`page.pdf`), so that pages and
//! the `PDF` do not own each other. `PDF.__init__` creates one
//! `PDFResourceManager` (pdfplumber/pdf.py:28 `PDF.__init__`, `caching`
//! left `True`): every page's `Page.layout` interprets with it, and its
//! font cache (fonts by object id, with the CMaps they loaded) is shared by
//! the pages of the document. The Wasm boundary keeps a `PDF` per
//! document handle and drops it when a call is void (a predefined CMap
//! missing: `crates/wasm/src/ops.rs`, "Void calls").
//!
//! # Laziness and caching (they decide order, identity and recursion)
//!
//! - `pdf.pages` (pdfplumber/pdf.py:145 `PDF.pages`) is computed on its
//!   first use: `self._pages = []` is set first, then the loop appends each
//!   `Page`: if the page tree or a `Page.__init__` raises, the list keeps
//!   the pages built so far and the next `pdf.pages` returns it as it is
//!   (`hasattr(self, "_pages")`). `doctop` accumulates `p.height` over the
//!   pages selected by `pages=` (`page_number not in pp`: any container a
//!   user gives, `in` with Python's semantics).
//! - `pdf.objects` concatenates each page's `objects` (the same dicts,
//!   new lists) and is cached; `pdf.annots`, `pdf.hyperlinks` and
//!   `pdf.structure_tree` are recomputed at each use.
//! - `close()` flushes the cache (`_pages` included: the `pdf.pages` that
//!   `close` reads builds new `Page` objects, which it then closes).
//!
//! # Metadata
//!
//! `PDF.__init__` merges the document's `/Info` dicts into a new dict and
//! runs `resolve_and_decode` on each value ([`read_metadata`]). That
//! function decodes a `dict` **in place**: the dicts of the document that
//! a metadata value reaches (an `/Info` entry that is a dict, or refers to
//! the catalog, a page, an annotation...) hold, from then on, Python `str`s
//! ([`pdfminer::pdftypes::PDFObject::Str`]), new lists and resolved
//! references, and everything that reads the document afterwards (the
//! pages, the structure tree, `Page.annots`) sees them, as in Python; a
//! dict reached twice is decoded twice, and a dict that refers to itself
//! until `RecursionError`, keeping what the levels already did (the
//! corpus has both: `cupertino_usd_4-6-16.pdf`, an oss-fuzz file whose
//! `/Info` refers to its catalog).
//!
//! # Recursion
//!
//! `pdfplumber.open(...)` is `PDF.open` (a frame), the class call
//! (`pycompat::recursion::class_call`), `PDF.__init__` (a frame), in which
//! `LAParams(**laparams)` (class call, `__init__`, `_validate`),
//! `PDFParser(stream)` and `PDFDocument(...)` (counted by `pdfminer`),
//! `PDFResourceManager()` (class call and `__init__`), and, for each
//! metadata value, `resolve_and_decode` (recursive: one frame per level,
//! `map()` calling it from C for a list) and the `logger.warning` of an
//! unparsable value (`pycompat::logging`, logger `pdfplumber.pdf`, disabled
//! as everywhere: `docs/verification.md`, "Étapes"). `pdf.pages` is a
//! frame, its `iter_pages` generator a frame per resumption from C
//! (`enumerate`), whose `next(gen)` resumes `create_pages` from C, and each
//! `Page(...)` a class call and the frames of `Page.__init__`.

use std::cell::RefCell;
use std::rc::Rc;

use pdfminer::layout::{IdSource, LAParams};
use pdfminer::pdfdocument::PDFDocument;
use pdfminer::pdfinterp::PDFResourceManager;
use pdfminer::pdfpage::PDFPage;
use pdfminer::pdfparser::FileKind;
use pdfminer::pdftypes::PDFObject;
use pycompat::logging::Logger;
use pycompat::recursion::{c_call, class_call, eval_entry, leaf, py_frame};
use pycompat::typing_cache::Subscription;
use pycompat::{exc, ExcKind, Num, PyErr, PyInt, PyResult, PyStr};

use pdfminer::pdftypes::PDFDict;
use pycompat::diagnostics;
use pycompat::recursion::c_called_leaf;

use crate::container::{Container, ContainerCache, Objects};
use crate::obj::{py_contains, Obj, PyValue};
use crate::page::{Page, PlumberPayload};
use crate::structure::{PDFStructTree, Scope};
use crate::utils::exceptions::pdfminer_exception;
use crate::utils::pdfinternals::resolve_and_decode_obj;

// `iter_pages`, nested in pdfplumber/pdf.py:145 PDF.pages: `->
// Generator[PDFPage, None, None]`,
// evaluated at each call of `PDF.pages` (depths below its frame, measured:
// `python -m oracle.depth_probe typing`; `pycompat::typing_cache`)
const ITER_PAGES_RETURN: Subscription = Subscription::new("Generator[PDFPage, None, None]", 7, 1);

/// `logger = logging.getLogger(__name__)` of `pdfplumber/pdf.py`.
static LOG: Logger = Logger::new("pdfplumber.pdf");

/// What `pdfplumber.open` reads (`path_or_fp`).
#[derive(Clone)]
pub enum Source {
    /// A path: `open(path, "rb")`, an `io.BufferedReader` the `PDF` owns
    /// (`stream_is_external = False`: `close()` closes it). The oracle's
    /// stages open their files so.
    Path { data: Rc<[u8]>, path: PyStr },
    /// A file object the caller gives (`stream_is_external = True`): an
    /// `io.BytesIO` for JavaScript's bytes, or an `io.BufferedReader`.
    Stream { data: Rc<[u8]>, kind: FileKind },
}

/// The `IdSource` of each page's analysis (`laparams` given): the library
/// default is `CreationOrder`; the `plumber_objects` stage of `pdftrace`
/// injects the `id()` ranking of the oracle's trace (`ORACLE_INPUT`).
pub type IdsFactory = Rc<dyn Fn(&PyInt) -> Box<dyn IdSource<PlumberPayload>>>;

/// The keyword arguments of `pdfplumber.open`
/// (pdfplumber/pdf.py:73 `PDF.open`), as given: they are checked where
/// Python uses them.
#[derive(Clone, Default)]
pub struct OpenOptions {
    /// `pages`: the page numbers to build (`page_number not in pp`), any
    /// container; `None`: every page.
    pub pages: Option<PyValue>,
    /// `laparams`, as given: `LAParams(**laparams)` (a mapping of keyword
    /// arguments; anything else raises the `TypeError` of `**`).
    pub laparams: Option<PyValue>,
    /// `password`, as given (`None` and `""` both open with `""`;
    /// pdfminer's security handlers decide what else they accept).
    pub password: Option<PyValue>,
    pub strict_metadata: bool,
    /// `unicode_norm`, as given: `normalize_unicode(form, text)` runs (and
    /// raises for a form it does not know) in `Page.process_object`.
    pub unicode_norm: Option<PyValue>,
    /// `raise_unicode_errors` (default `True`)
    pub raise_unicode_errors: bool,
    /// `repair=True` is out of scope (`NotPorted`).
    pub repair: bool,
    /// Not a Python option: the `id()` order of the analyses (see
    /// [`IdsFactory`]); `None`: `CreationOrder`.
    pub ids: Option<IdsFactory>,
}

impl OpenOptions {
    /// The defaults of `pdfplumber.open(path)` (`raise_unicode_errors=True`).
    pub fn new() -> OpenOptions {
        OpenOptions { raise_unicode_errors: true, ..OpenOptions::default() }
    }
}

/// What a `Page` reads of its `PDF` (`page.pdf.*`), shared by the `PDF`
/// and its pages.
pub struct PdfCore {
    /// `self.doc`
    pub doc: Rc<PDFDocument>,
    /// `self.rsrcmgr`: one per document, shared by every `Page.layout`.
    pub rsrcmgr: Rc<PDFResourceManager>,
    /// `self.laparams`: `None`, or `LAParams(**laparams)`
    pub laparams: Option<LAParams>,
    /// `self.unicode_norm`
    pub unicode_norm: Option<PyValue>,
    /// `self.raise_unicode_errors`
    pub raise_unicode_errors: bool,
    /// See [`OpenOptions::ids`].
    pub ids: Option<IdsFactory>,
}

/// pdfplumber/pdf.py:25 `PDF`
pub struct PDF {
    pub core: Rc<PdfCore>,
    /// `self.stream_is_external`
    pub stream_is_external: bool,
    /// `self.path`
    pub path: Option<PyStr>,
    /// `self.pages_to_parse`
    pub pages_to_parse: Option<PyValue>,
    /// `self.password`
    pub password: Option<PyValue>,
    /// `self.metadata`: the `/Info` dicts merged, each value through
    /// `resolve_and_decode` (`str` keys: the PDF dict's).
    pub metadata: Obj,
    pub cache: RefCell<PdfCache>,
    container: RefCell<ContainerCache>,
}

/// The cached properties of a `PDF` besides [`ContainerCache`].
#[derive(Default)]
pub struct PdfCache {
    /// `_pages`: set (empty) before the loop that fills it.
    pub pages: Option<Vec<Rc<Page>>>,
    /// `_objects`
    pub objects: Option<Rc<Objects>>,
}

/// The names of `LAParams.__init__`'s parameters, in order
/// (pdfminer/layout.py:76 `LAParams.__init__`).
const LAPARAMS_NAMES: [&str; 7] =
    ["line_overlap", "char_margin", "line_margin", "word_margin", "boxes_flow", "detect_vertical", "all_texts"];

/// `LAParams(**laparams)`: the `TypeError`s of `**` (a value that is not a
/// mapping, a key that is not a `str`) and of an unknown keyword, then
/// pdfminer/layout.py:76 `LAParams.__init__` and :96 `LAParams._validate`.
fn laparams_from(laparams: &PyValue) -> PyResult<LAParams> {
    let pairs: Vec<(PyValue, PyValue)> = match laparams {
        PyValue::Dict(d) => d.items().iter().map(|(k, v)| (PyValue::Str(k.clone()), v.clone())).collect(),
        PyValue::Pdf(PDFObject::Dict(d)) => {
            d.items().into_iter().map(|(k, v)| (PyValue::str(&k), PyValue::from_pdf(v))).collect()
        }
        other => {
            return Err(exc::TypeError(format!(
                "pdfminer.layout.LAParams() argument after ** must be a mapping, not {}",
                other.type_name()
            )))
        }
    };
    let mut values: [Option<PyValue>; 7] = Default::default();
    for (k, v) in &pairs {
        let PyValue::Str(k) = k else { return Err(exc::TypeError("keywords must be strings")) };
        match LAPARAMS_NAMES.iter().position(|n| *k == **n) {
            Some(i) => values[i] = Some(v.clone()),
            None => {
                return Err(PyErr::with_message(
                    ExcKind::TypeError,
                    &PyStr::new("LAParams.__init__() got an unexpected keyword argument ")
                        .concat(&PyStr::from(k.repr())),
                ))
            }
        }
    }
    // the class call, __init__ (which calls _validate)
    let _call = class_call()?;
    let _init = py_frame()?;
    let margin = |i: usize, default: f64| -> PyResult<Num> {
        match &values[i] {
            None => Ok(Num::Float(default)),
            Some(v) => v.as_num().ok_or_else(|| {
                not_modelled(&format!(
                    "LAParams({}=<{}>): a layout parameter that is not a number",
                    LAPARAMS_NAMES[i],
                    v.type_name()
                ))
            }),
        }
    };
    let line_overlap = margin(0, 0.5)?;
    let char_margin = margin(1, 2.0)?;
    let line_margin = margin(2, 0.5)?;
    let word_margin = margin(3, 0.1)?;
    leaf(1)?; // self._validate()
    let boxes_flow = match &values[4] {
        None => Some(Num::Float(0.5)),
        Some(v) if v.is_none() => None,
        Some(v) => match v.as_num() {
            Some(n) => Some(n),
            None => return Err(exc::PDFTypeError("LAParam boxes_flow should be None, or a number between -1 and +1")),
        },
    };
    let truth = |i: usize| values[i].as_ref().is_some_and(PyValue::truthy);
    LAParams::new(line_overlap, char_margin, line_margin, word_margin, boxes_flow, truth(5), truth(6))
}

/// A documented deviation (see the module documentation).
fn not_modelled(what: &str) -> PyErr {
    PyErr::new(ExcKind::NotPorted, format!("not modelled: {what}"))
}

/// `password or ""` as the `str` `PDFDocument` receives.
fn password_str(password: &Option<PyValue>) -> PyResult<String> {
    match password {
        Some(p) if p.truthy() => match p {
            PyValue::Str(s) => match s.as_str() {
                Some(s) => Ok(s.to_owned()),
                None => Err(not_modelled("a password holding a lone surrogate")),
            },
            other => Err(not_modelled(&format!("a password of type '{}'", other.type_name()))),
        },
        _ => Ok(String::new()),
    }
}

impl PDF {
    // pdfplumber/pdf.py:73 PDF.open
    /// `pdfplumber.open(path_or_fp, **options)`: the classmethod, then the
    /// class call and pdfplumber/pdf.py:28 `PDF.__init__`. An exception
    /// of `PDFDocument(...)` is `PdfminerException(e)`
    /// (`crate::utils::exceptions::pdfminer_exception`); one of
    /// `LAParams(**laparams)` or of the metadata (with `strict_metadata`)
    /// is raised as it is.
    pub fn open(source: Source, options: OpenOptions) -> PyResult<PDF> {
        let _frame = py_frame()?;
        if options.repair {
            return Err(PyErr::new(
                ExcKind::NotPorted,
                "out of scope: pdfplumber.open(repair=True) runs Ghostscript (pdfplumber/repair.py)",
            ));
        }
        let (data, file, stream_is_external, path) = match source {
            Source::Path { data, path } => {
                // open(path_or_fp, "rb") (C), then pathlib.Path(path_or_fp):
                // the class call, `Path.__new__`, `__init__` and the
                // `PurePath.__init__` it calls (measured: two frames deep,
                // `python -m oracle.depth_probe open`)
                {
                    let _call = class_call()?;
                    leaf(2)?;
                }
                (data, FileKind::BufferedReader, false, Some(path))
            }
            Source::Stream { data, kind } => (data, kind, true, None),
        };
        // cls(stream, path=path, pages=pages, ...); on a PdfminerException a
        // stream the PDF opened is closed (nothing to do here)
        let _call = class_call()?;
        PDF::init(data, file, stream_is_external, path, options)
    }

    // pdfplumber/pdf.py:28 PDF.__init__
    fn init(
        data: Rc<[u8]>,
        file: FileKind,
        stream_is_external: bool,
        path: Option<PyStr>,
        options: OpenOptions,
    ) -> PyResult<PDF> {
        let _frame = py_frame()?;
        let laparams = match &options.laparams {
            Some(l) if !l.is_none() => Some(laparams_from(l)?),
            _ => None,
        };
        let password = password_str(&options.password)?;
        let doc = match PDFDocument::open(data, file, &password) {
            Ok(doc) => doc,
            Err(e) => {
                // raise PdfminerException(e): the class call of an
                // Exception subclass without __init__ of its own
                drop(c_call(" while calling a Python object")?);
                return Err(pdfminer_exception(e));
            }
        };
        // PDFResourceManager(): the class call, its __init__
        pycompat::recursion::class_call_leaf()?;
        let rsrcmgr = Rc::new(PDFResourceManager::new(true));
        let metadata = read_metadata(&doc, options.strict_metadata)?;
        let core = PdfCore {
            doc,
            rsrcmgr,
            laparams,
            unicode_norm: options.unicode_norm.clone().filter(|n| !n.is_none()),
            raise_unicode_errors: options.raise_unicode_errors,
            ids: options.ids.clone(),
        };
        Ok(PDF {
            core: Rc::new(core),
            stream_is_external,
            path,
            pages_to_parse: options.pages.clone().filter(|p| !p.is_none()),
            password: options.password.clone(),
            metadata,
            cache: RefCell::new(PdfCache::default()),
            container: RefCell::new(ContainerCache::default()),
        })
    }

    // pdfplumber/pdf.py:124 PDF.close
    /// `flush_cache()`, then `page.close()` for each page of `self.pages`
    /// (built again: the cache was just flushed), then the file closed
    /// when the `PDF` opened it (`PDFDocument::close_file`).
    pub fn close(&self) -> PyResult<()> {
        let _frame = py_frame()?;
        {
            let _flush = py_frame()?;
            Container::flush_cache(self, None);
        }
        for page in self.pages()? {
            page.close()?;
        }
        if !self.stream_is_external {
            // self.stream.close()
            self.core.doc.close_file()?;
        }
        Ok(())
    }

    // pdfplumber/pdf.py:145 PDF.pages
    /// The pages (see "Laziness and caching"); an exception of the page
    /// tree is `PdfminerException(e)`, one of `Page.__init__` is raised as
    /// it is (the cache keeps the pages built before it).
    pub fn pages(&self) -> PyResult<Vec<Rc<Page>>> {
        let _frame = py_frame()?;
        if let Some(pages) = &self.cache.borrow().pages {
            return Ok(pages.clone());
        }
        let mut doctop = PyValue::Int(PyInt::ZERO);
        let pp = self.pages_to_parse.clone();
        self.cache.borrow_mut().pages = Some(Vec::new());
        // def iter_pages() -> Generator[PDFPage, None, None]: its
        // annotation, evaluated at each call
        ITER_PAGES_RETURN.eval()?;
        // iter_pages(): the generator is created
        leaf(1)?;
        let mut gen = PDFPage::create_pages(&self.core.doc).resumed_from_c();
        let mut i: i64 = 0;
        loop {
            // enumerate() resumes iter_pages from C; its `next(gen)` resumes
            // create_pages from C
            let page = {
                let _entry = eval_entry()?;
                let _iter_pages = py_frame()?;
                match gen.next() {
                    None => break,
                    Some(Ok(page)) => page,
                    Some(Err(e)) => {
                        // except Exception as e: raise PdfminerException(e)
                        drop(c_call(" while calling a Python object")?);
                        return Err(pdfminer_exception(e));
                    }
                }
            };
            let page_number = PyInt::Small(i + 1);
            i += 1;
            if let Some(pp) = &pp {
                // `page_number not in pp`
                if !py_contains(pp, &PyValue::Int(page_number.clone()))? {
                    continue;
                }
            }
            let p = Rc::new(Page::new(self.core.clone(), Rc::new(page), page_number, doctop.clone())?);
            self.cache.borrow_mut().pages.as_mut().expect("set above").push(p.clone());
            doctop = doctop.add(&p.height()?)?;
        }
        Ok(self.cache.borrow().pages.clone().expect("set above"))
    }

    // pdfplumber/pdf.py:173 PDF.objects
    pub fn objects(&self) -> PyResult<Rc<Objects>> {
        let _frame = py_frame()?;
        if let Some(objects) = &self.cache.borrow().objects {
            return Ok(objects.clone());
        }
        let mut all_objects = Objects::new();
        for p in self.pages()? {
            let objects = p.objects()?;
            for kind in objects.keys() {
                // all_objects[kind] = all_objects.get(kind, []) + p.objects[kind]
                let mut list = all_objects.get_key(kind).map(<[Obj]>::to_vec).unwrap_or_default();
                list.extend(p.objects()?.get_key(kind).unwrap_or(&[]).iter().cloned());
                all_objects.insert(kind.clone(), list);
            }
        }
        let all_objects = Rc::new(all_objects);
        self.cache.borrow_mut().objects = Some(all_objects.clone());
        Ok(all_objects)
    }

    // pdfplumber/pdf.py:184 PDF.annots
    /// `list(itertools.chain(*(p.annots for p in self.pages)))`. Workstream
    /// DOC (`Page::annots`).
    pub fn annots(&self) -> PyResult<Vec<Obj>> {
        let _frame = py_frame()?;
        let pages = self.pages()?;
        // the generator expression is created, then unpacked (resumed from
        // C, once per page and once more) into chain()
        leaf(1)?;
        let mut out = Vec::new();
        for p in &pages {
            let _entry = eval_entry()?;
            let _gen = py_frame()?;
            out.extend(p.annots()?);
        }
        let _entry = eval_entry()?;
        leaf(1)?;
        Ok(out)
    }

    // pdfplumber/pdf.py:189 PDF.hyperlinks
    /// `list(itertools.chain(*(p.hyperlinks for p in self.pages)))`.
    /// Workstream DOC (`Page::hyperlinks`).
    pub fn hyperlinks(&self) -> PyResult<Vec<Obj>> {
        let _frame = py_frame()?;
        let pages = self.pages()?;
        leaf(1)?;
        let mut out = Vec::new();
        for p in &pages {
            let _entry = eval_entry()?;
            let _gen = py_frame()?;
            out.extend(p.hyperlinks()?);
        }
        let _entry = eval_entry()?;
        leaf(1)?;
        Ok(out)
    }

    // pdfplumber/pdf.py:194 PDF.structure_tree
    /// `[elem.to_dict() for elem in PDFStructTree(self)]`, `[]` when the
    /// document has no structure. Workstream DOC (`crate::structure`).
    pub fn structure_tree(&self) -> PyResult<Vec<PyValue>> {
        let _frame = py_frame()?;
        let tree = match PDFStructTree::new(&self.core.doc, Scope::Document(self)) {
            Ok(tree) => tree,
            Err(e) if e.is_instance(ExcKind::StructTreeMissing) => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        // [elem.to_dict() for elem in tree]: PDFStructTree.__iter__, run by C
        c_called_leaf()?;
        let file = self.core.doc.file_kind()?;
        tree.iter().iter().map(|elem| elem.to_dict(file)).collect()
    }

    /// `pdf.metadata` as a value.
    pub fn metadata_value(&self) -> PyValue {
        PyValue::Dict(self.metadata.clone())
    }
}

// pdfplumber/pdf.py:28 PDF.__init__
/// The metadata part of `PDF.__init__`: `self.metadata = {}`, updated with
/// each of the document's `/Info` dicts, then each value through
/// `resolve_and_decode` (in place: see "Metadata" in the module
/// documentation). An exception there (any `Exception`) is raised with
/// `strict_metadata`, otherwise logged (`logger.warning`, logger
/// `pdfplumber.pdf`, its message formatted first) and the value kept.
pub fn read_metadata(doc: &PDFDocument, strict_metadata: bool) -> PyResult<Obj> {
    let metadata = PDFDict::new();
    for info in doc.info() {
        // self.metadata.update(info)
        for (k, v) in info.items() {
            metadata.insert(k, v);
        }
    }
    let mut i = 0;
    while let Some((k, v)) = metadata.get_index(i) {
        match resolve_and_decode_obj(&v) {
            Ok(decoded) => metadata.insert(k, decoded),
            Err(e) if strict_metadata || !e.is_instance(ExcKind::Exception) => return Err(e),
            Err(e) => {
                // This metadata value could not be parsed. Instead of failing
                // the PDF read, treat it as a warning only if
                // `strict_metadata=False`: the f-string's str(e) (a call of
                // the builtin) before the call.
                drop(c_call(" while calling a Python object")?);
                diagnostics::emit("pdfplumber.pdf", diagnostics::Level::Warning, || {
                    format!(
                        "[WARNING] Metadata key \"{k}\" could not be parsed due to exception: {}",
                        e.py_message().to_string_lossy()
                    )
                });
                LOG.warning()?;
            }
        }
        i += 1;
    }
    Ok(Obj::from_pairs(metadata.items().into_iter().map(|(k, v)| (k, PyValue::from_pdf(v)))))
}

impl Container for PDF {
    fn pages(&self) -> PyResult<Option<Vec<Rc<Page>>>> {
        PDF::pages(self).map(Some)
    }

    fn objects(&self) -> PyResult<Rc<Objects>> {
        PDF::objects(self)
    }

    // pdfplumber/pdf.py:201 PDF.to_dict
    /// `{"metadata": ..., "pages": [page.to_dict(object_types) ...]}`
    fn to_dict(&self, object_types: Option<&[PyStr]>) -> PyResult<Obj> {
        let _frame = py_frame()?;
        let mut pages = Vec::new();
        for page in PDF::pages(self)? {
            pages.push(PyValue::Dict(page.to_dict(object_types)?));
        }
        Ok(Obj::from_pairs([("metadata", self.metadata_value()), ("pages", PyValue::List(pages.into()))]))
    }

    fn container_cache(&self) -> &RefCell<ContainerCache> {
        &self.container
    }

    /// `getattr(pdf, name)` for the attribute names of the container's
    /// lists (`chars`, `rects`...), `pages`, `objects`, `metadata`,
    /// `annots`, `hyperlinks` and the edges; `AttributeError` for a name
    /// that is not an attribute; `NotPorted` for a method or another
    /// attribute that is not a value of the object model.
    fn attribute(&self, name: &PyStr) -> PyResult<PyValue> {
        match name.as_str() {
            Some("pages") => Ok(PyValue::List(PDF::pages(self)?.iter().map(|p| PyValue::Page(p.as_value())).collect())),
            Some("metadata") => Ok(self.metadata_value()),
            Some("annots") => Ok(crate::container::obj_list(self.annots()?)),
            Some("hyperlinks") => Ok(crate::container::obj_list(self.hyperlinks()?)),
            _ => crate::container::container_attribute(self, "PDF", name),
        }
    }

    fn flush_cache(&self, properties: Option<&[&str]>) {
        // cached_properties = Container.cached_properties + ["_pages"]
        let props = properties.unwrap_or(&["_rect_edges", "_curve_edges", "_edges", "_objects", "_pages"]);
        crate::container::flush_container_cache(&self.container, props);
        let mut cache = self.cache.borrow_mut();
        for p in props {
            match *p {
                "_objects" => cache.objects = None,
                "_pages" => cache.pages = None,
                _ => {}
            }
        }
    }
}
