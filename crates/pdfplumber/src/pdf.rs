//! Port of `pdfplumber/pdf.py`: `PDF`, what `pdfplumber.open` returns.
//!
//! Wave 4 (base): the interface, bodies `todo!()` (workstream PAGE; the
//! metadata, `annots`, `hyperlinks` and `structure_tree` parts are
//! workstream DOC's: `docs/architecture.md`, "Vague 4").
//!
//! Out of scope (`docs/public-api.md`): `repair=True` (pdfplumber/repair.py
//! runs Ghostscript in a subprocess: `PDF::open` refuses it with
//! `NotPorted`), `gs_path`, `repair_setting`.
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
//! - `close()` flushes the cache (`_pages` included: the next `pdf.pages`
//!   builds new `Page` objects, which `close` then closes) and closes the
//!   file when the `PDF` opened it (a path): any later read of the
//!   document raises the `ValueError` of a closed file (`seek of closed
//!   file`, `read of closed file`), a state `pdfparser` does not model
//!   yet (to add with `close`).
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
//! `LAParams(**laparams)` (class call and `__init__`), `PDFParser(stream)`
//! and `PDFDocument(...)` (counted by `pdfminer`), `PDFResourceManager()`
//! (class call and `__init__`), and, for each metadata value,
//! `resolve_and_decode` (recursive: one frame per level, `map()` calling it
//! from C for a list) and the `logger.warning` of an unparsable value
//! (`pycompat::logging`, logger `pdfplumber.pdf`, disabled as everywhere:
//! `docs/verification.md`, "Étapes"). `pdf.pages` is a frame, its
//! `iter_pages` generator a frame per resumption (on top of
//! `create_pages`' own), each `Page(...)` a class call and the frames of
//! `Page.__init__`.

use std::cell::RefCell;
use std::rc::Rc;

use pdfminer::layout::{IdSource, LAParams};
use pdfminer::pdfdocument::PDFDocument;
use pdfminer::pdfinterp::PDFResourceManager;
use pdfminer::pdfparser::FileKind;
use pycompat::{PyInt, PyResult, PyStr};

use pdfminer::pdftypes::PDFDict;
use pycompat::logging::Logger;
use pycompat::recursion::{c_called_leaf, py_frame};
use pycompat::{diagnostics, ExcKind};

use crate::container::{Container, ContainerCache, Objects};
use crate::structure::{PDFStructTree, Scope};
use crate::utils::pdfinternals::resolve_and_decode_obj;
use crate::obj::{Obj, PyValue};
use crate::page::{Page, PlumberPayload};

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

impl PDF {
    // pdfplumber/pdf.py:73 PDF.open
    /// `pdfplumber.open(path_or_fp, **options)`: the classmethod, then the
    /// class call and pdfplumber/pdf.py:28 `PDF.__init__`. An exception
    /// of `PDFDocument(...)` is `PdfminerException(e)`
    /// (`crate::utils::exceptions::pdfminer_exception`); one of
    /// `LAParams(**laparams)` or of the metadata (with `strict_metadata`)
    /// is raised as it is.
    pub fn open(source: Source, options: OpenOptions) -> PyResult<PDF> {
        let _ = (source, options);
        todo!("pdfplumber/pdf.py:73 PDF.open, pdfplumber/pdf.py:28 PDF.__init__")
    }

    // pdfplumber/pdf.py:124 PDF.close
    pub fn close(&self) -> PyResult<()> {
        todo!("pdfplumber/pdf.py:124 PDF.close")
    }

    // pdfplumber/pdf.py:145 PDF.pages
    /// The pages (see "Laziness and caching"); an exception of the page
    /// tree is `PdfminerException(e)`, one of `Page.__init__` is raised as
    /// it is (the cache keeps the pages built before it).
    pub fn pages(&self) -> PyResult<Vec<Rc<Page>>> {
        todo!("pdfplumber/pdf.py:145 PDF.pages")
    }

    // pdfplumber/pdf.py:173 PDF.objects
    pub fn objects(&self) -> PyResult<Rc<Objects>> {
        todo!("pdfplumber/pdf.py:173 PDF.objects")
    }

    // pdfplumber/pdf.py:184 PDF.annots
    /// Workstream DOC.
    pub fn annots(&self) -> PyResult<Vec<Obj>> {
        let _frame = py_frame()?;
        // list(itertools.chain(*(p.annots for p in self.pages))): the
        // generator is run to the end first
        let pages = self.pages()?;
        let mut gen = Vec::with_capacity(pages.len());
        {
            let _genexpr = py_frame()?;
            for p in &pages {
                gen.push(p.annots()?);
            }
        }
        Ok(gen.into_iter().flatten().collect())
    }

    // pdfplumber/pdf.py:189 PDF.hyperlinks
    /// Workstream DOC.
    pub fn hyperlinks(&self) -> PyResult<Vec<Obj>> {
        let _frame = py_frame()?;
        let pages = self.pages()?;
        let mut gen = Vec::with_capacity(pages.len());
        {
            let _genexpr = py_frame()?;
            for p in &pages {
                gen.push(p.hyperlinks()?);
            }
        }
        Ok(gen.into_iter().flatten().collect())
    }

    // pdfplumber/pdf.py:194 PDF.structure_tree
    /// `[elem.to_dict() for elem in PDFStructTree(self)]`, `[]` when the
    /// document has no structure. Workstream DOC
    /// (`crate::structure`).
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
                // `strict_metadata=False`.
                diagnostics::emit("pdfplumber.pdf", diagnostics::Level::Warning, || {
                    format!("[WARNING] Metadata key \"{k}\" could not be parsed due to exception: {}", e.py_message().to_string_lossy())
                });
                LOG.warning()?;
            }
        }
        i += 1;
    }
    Ok(Obj::from_pairs(metadata.items().into_iter().map(|(k, v)| (k, PyValue::from_pdf(v)))))
}

// pdfplumber/pdf.py:22 logger
static LOG: Logger = Logger::new("pdfplumber.pdf");

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
        let _ = object_types;
        todo!("pdfplumber/pdf.py:201 PDF.to_dict")
    }

    fn container_cache(&self) -> &RefCell<ContainerCache> {
        &self.container
    }

    fn attribute(&self, name: &PyStr) -> PyResult<PyValue> {
        let _ = name;
        todo!("getattr(pdf, name) for pdfplumber/container.py:132 Container.to_csv")
    }

    fn flush_cache(&self, properties: Option<&[&str]>) {
        let _ = (properties, &self.cache);
        todo!("pdfplumber/container.py:28 Container.flush_cache (PDF: + _pages)")
    }
}
