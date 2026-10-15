//! Port of `pdfminer/pdfpage.py`: `PDFPage`, its attributes (with the
//! inherited ones), `create_pages` (the page tree walk, or the scan of the
//! xrefs when there is no `/Pages`) and `get_pages`.
//!
//! `create_pages` and its nested `depth_first_search` are generators: the
//! pages are created lazily, and the document is read (`getobj`, labels)
//! only as far as the pages asked for, in Python's order ([`CreatePages`]).
//! The page id is whatever upstream takes as one (`obj.objid` of a
//! reference or a stream, an int given in place of a reference, an xref
//! id), so it is a [`PDFObject`].

use std::collections::HashSet;
use std::rc::Rc;
use std::sync::LazyLock;

use pycompat::recursion::{py_frame, py_frames, Guard};
use pycompat::{exc, ExcKind, PyErr, PyInt, PyResult};

use crate::casting::IterItem;
use crate::pdfdocument::{ObjIds, PDFDocument, PageLabelsIter};
use crate::pdfparser::FileKind;
use crate::pdftypes::{dict_value, int_value, list_value, resolve1_none, ObjId, PDFDict, PDFList, PDFObject};
use crate::psparser::{PSLiteral, LIT};
use crate::utils::parse_rect;

// pdfminer/pdfpage.py:21-22 LITERAL_PAGE, LITERAL_PAGES
pub static LITERAL_PAGE: LazyLock<PSLiteral> = LazyLock::new(|| LIT("Page"));
pub static LITERAL_PAGES: LazyLock<PSLiteral> = LazyLock::new(|| LIT("Pages"));

// pdfminer/pdfpage.py:83 PDFPage.INHERITABLE_ATTRS
pub const INHERITABLE_ATTRS: [&str; 4] = ["Resources", "MediaBox", "CropBox", "Rotate"];

/// A rectangle as `parse_rect` returns it: four floats.
pub type FloatRect = [f64; 4];

/// pdfminer/pdfpage.py:25 `PDFPage`: an object that holds the information
/// about a page.
pub struct PDFPage {
    /// `self.doc`
    pub doc: Rc<PDFDocument>,
    /// `self.pageid`: any Python object that can uniquely identify the page.
    pub pageid: PDFObject,
    /// `self.attrs`: a dictionary of page attributes.
    pub attrs: PDFDict,
    /// `self.label`: the page's label (typically, the logical page number).
    pub label: Option<String>,
    /// `self.lastmod`: the last modified time of the page.
    pub lastmod: PDFObject,
    /// `self.resources`: a dictionary of resources used by the page.
    pub resources: PDFObject,
    /// `self.mediabox`: the physical size of the page.
    pub mediabox: FloatRect,
    /// `self.cropbox`: the crop rectangle of the page.
    pub cropbox: FloatRect,
    /// `self.contents`: the page content (a list: the resolved `/Contents`
    /// list itself, or a new list).
    pub contents: PDFList,
    /// `self.rotate`: the page rotation (in degree).
    pub rotate: PyInt,
    /// `self.annots`: the page annotations.
    pub annots: PDFObject,
    /// `self.beads`: a chain that represents natural reading order.
    pub beads: PDFObject,
}

/// `(0.0, 0.0, 612.0, 792.0)`
const US_LETTER: FloatRect = [0.0, 0.0, 612.0, 792.0];

impl PDFPage {
    // pdfminer/pdfpage.py:49 PDFPage.__init__
    /// Initialize a page object: `doc`, `pageid`, the page attributes
    /// `attrs` and the page label.
    pub fn new(doc: Rc<PDFDocument>, pageid: PDFObject, attrs: &PDFObject, label: Option<String>) -> PyResult<PDFPage> {
        let _frame = py_frame()?;
        let attrs = dict_value(attrs)?;
        let lastmod = resolve1_none(&attrs.get("LastModified").unwrap_or(PDFObject::Null))?;
        let resources = resolve1_none(&attrs.get("Resources").unwrap_or_else(|| PDFObject::Dict(PDFDict::new())))?;
        let mediabox = PDFPage::_parse_mediabox(&attrs.get("MediaBox").unwrap_or(PDFObject::Null))?;
        let cropbox = PDFPage::_parse_cropbox(&attrs.get("CropBox").unwrap_or(PDFObject::Null), mediabox)?;
        let contents = PDFPage::_parse_contents(&attrs.get("Contents").unwrap_or(PDFObject::Null))?;
        let rotate = int_value(&attrs.get("Rotate").unwrap_or(PDFObject::Int(PyInt::ZERO)))?;
        let rotate = rotate.add(&PyInt::Small(360)).modulo(&PyInt::Small(360))?;
        let annots = attrs.get("Annots").unwrap_or(PDFObject::Null);
        let beads = attrs.get("B").unwrap_or(PDFObject::Null);
        Ok(PDFPage {
            doc,
            pageid,
            attrs,
            label,
            lastmod,
            resources,
            mediabox,
            cropbox,
            contents,
            rotate,
            annots,
            beads,
        })
    }

    // pdfminer/pdfpage.py:189 PDFPage._parse_mediabox
    fn _parse_mediabox(value: &PDFObject) -> PyResult<FloatRect> {
        let _frame = py_frame()?;
        if matches!(value, PDFObject::Null) {
            // log.warning("MediaBox missing from /Page (and not inherited), defaulting to US Letter")
            drop(py_frames(2)?);
            return Ok(US_LETTER);
        }
        match parse_box(value) {
            Ok(rect) => Ok(rect),
            Err(e) if e.is_instance(ExcKind::PDFValueError) => {
                // log.warning("Invalid MediaBox in /Page, defaulting to US Letter")
                drop(py_frames(2)?);
                Ok(US_LETTER)
            }
            Err(e) => Err(e),
        }
    }

    // pdfminer/pdfpage.py:206 PDFPage._parse_cropbox
    fn _parse_cropbox(value: &PDFObject, mediabox: FloatRect) -> PyResult<FloatRect> {
        let _frame = py_frame()?;
        if matches!(value, PDFObject::Null) {
            // CropBox is optional, and MediaBox is used if not specified.
            return Ok(mediabox);
        }
        match parse_box(value) {
            Ok(rect) => Ok(rect),
            Err(e) if e.is_instance(ExcKind::PDFValueError) => {
                // log.warning("Invalid CropBox in /Page, defaulting to MediaBox")
                drop(py_frames(2)?);
                Ok(mediabox)
            }
            Err(e) => Err(e),
        }
    }

    // pdfminer/pdfpage.py:218 PDFPage._parse_contents
    fn _parse_contents(value: &PDFObject) -> PyResult<PDFList> {
        let _frame = py_frame()?;
        if matches!(value, PDFObject::Null) {
            return Ok(PDFList::default());
        }
        Ok(match resolve1_none(value)? {
            PDFObject::List(l) => l,
            contents => PDFList::new(vec![contents]),
        })
    }

    // pdfminer/pdfpage.py:80 PDFPage.__repr__
    pub fn repr(&self) -> PyResult<String> {
        let mediabox = self.mediabox.map(pycompat::repr::float_repr).join(", ");
        Ok(format!("<PDFPage: Resources={}, MediaBox=({mediabox})>", self.resources.repr()?))
    }

    // pdfminer/pdfpage.py:91 PDFPage.create_pages
    /// The pages of `document`, lazily (see [`CreatePages`]).
    pub fn create_pages(document: &Rc<PDFDocument>) -> CreatePages {
        CreatePages { doc: document.clone(), state: CreateState::NotStarted, done: false }
    }

    // pdfminer/pdfpage.py:153 PDFPage.get_pages
    /// The pages of the file whose content is `data` (a file object of
    /// class `file`, written `fp_repr` in messages), as `get_pages(fp,
    /// pagenos, maxpages, password, caching, check_extractable)`.
    #[allow(clippy::too_many_arguments)]
    pub fn get_pages(
        data: Rc<[u8]>,
        file: FileKind,
        fp_repr: &str,
        pagenos: Option<&HashSet<usize>>,
        maxpages: usize,
        password: &str,
        caching: bool,
        check_extractable: bool,
    ) -> PyResult<GetPages> {
        // Create a PDF parser object associated with the file object.
        let parser = crate::pdfparser::PDFParser::new(pycompat::io::BytesIO::new(data), file)?;
        // Create a PDF document object that stores the document structure.
        let doc = PDFDocument::new(parser, password, caching, true)?;
        // Check if the document allows text extraction.
        // If not, warn the user and proceed.
        if !doc.is_extractable() && check_extractable {
            let error_msg = format!("Text extraction is not allowed: {fp_repr}");
            return Err(PyErr::new(ExcKind::PDFTextExtractionNotAllowed, error_msg));
        }
        // else, when not extractable: log.warning(warning_msg)
        Ok(GetPages {
            pages: PDFPage::create_pages(&doc),
            pagenos: pagenos.cloned().filter(|p| !p.is_empty()),
            maxpages,
            pageno: 0,
            done: false,
        })
    }
}

/// `parse_rect(resolve1(val) for val in resolve1(value))`: iterating the
/// resolved value fails at once (`TypeError`) if it is not iterable; its
/// items are resolved one by one as `parse_rect` unpacks them.
fn parse_box(value: &PDFObject) -> PyResult<Rect4> {
    let items: Box<dyn Iterator<Item = PyResult<IterItem>>> = match resolve1_none(value)? {
        PDFObject::List(l) => Box::new(l.to_vec().into_iter().map(|v| Ok(IterItem::Obj(resolve1_none(&v)?)))),
        PDFObject::Dict(d) => Box::new(d.keys().into_iter().map(|k| Ok(IterItem::Str(k)))),
        PDFObject::Bytes(b) => Box::new(
            (0..b.len()).map(move |i| b[i]).map(|c| Ok(IterItem::Obj(PDFObject::Int(PyInt::Small(i64::from(c)))))),
        ),
        // the sequence protocol: stream[0] raises PDFKeyError
        stream @ PDFObject::Stream(_) => {
            Box::new(std::iter::once(Err(stream.py_getitem_index0().expect_err("a stream has no attribute 0"))))
        }
        other => return Err(exc::TypeError(format!("'{}' object is not iterable", other.type_name()))),
    };
    parse_rect(items)
}

type Rect4 = FloatRect;

/// One `depth_first_search(obj, parent, visited)` generator.
struct Search {
    obj: PDFObject,
    parent: PDFDict,
    /// The kids still to walk, once the node turned out to be a `/Pages`.
    kids: Option<std::vec::IntoIter<PDFObject>>,
    /// `object_properties`, once computed.
    props: Option<PDFDict>,
    /// Whether the generator started (its frame is on the stack while the
    /// chain runs).
    started: bool,
}

enum CreateState {
    NotStarted,
    /// Walking the page tree: the stack of `depth_first_search` generators
    /// (the `yield from` chain), `visited`, whether a page was yielded.
    Tree {
        labels: Labels,
        stack: Vec<Search>,
        visited: HashSet<ObjId>,
        pages: bool,
    },
    /// The fallback over the xrefs: the xref being read and its ids.
    Xrefs {
        labels: Labels,
        xref: usize,
        ids: Option<ObjIds<'static>>,
    },
    Done,
}

/// `page_labels`: the labels generator, or `itertools.repeat(None)`.
enum Labels {
    Labels(Box<PageLabelsIter>),
    Repeat,
}

impl Labels {
    /// `next(page_labels)`
    fn next(&mut self) -> PyResult<Option<String>> {
        match self {
            Labels::Repeat => Ok(None),
            Labels::Labels(it) => match it.next() {
                Some(label) => Ok(Some(label?)),
                // the labels never end; a generator that raised is
                // finished, and the error ended create_pages with it
                None => Err(PyErr::new(ExcKind::RuntimeError, "generator raised StopIteration")),
            },
        }
    }
}

/// The generator `PDFPage.create_pages(document)`: pages are made one at
/// a time, the walk resumes only when the next page is asked for.
pub struct CreatePages {
    doc: Rc<PDFDocument>,
    state: CreateState,
    done: bool,
}

impl CreatePages {
    fn step(&mut self) -> PyResult<Option<PDFPage>> {
        // the frame of the create_pages generator, resumed
        let _frame = py_frame()?;
        loop {
            match &mut self.state {
                CreateState::Done => return Ok(None),
                CreateState::NotStarted => {
                    let labels = match self.doc.get_page_labels() {
                        Ok(labels) => Labels::Labels(Box::new(labels)),
                        Err(e) if e.is_instance(ExcKind::PDFNoPageLabels) => Labels::Repeat,
                        Err(e) => return Err(e),
                    };
                    let catalog = self.doc.catalog();
                    self.state = match catalog.get("Pages") {
                        Some(pages) => {
                            let root = Search { obj: pages, parent: catalog, kids: None, props: None, started: false };
                            CreateState::Tree { labels, stack: vec![root], visited: HashSet::new(), pages: false }
                        }
                        None => CreateState::Xrefs { labels, xref: 0, ids: None },
                    };
                }
                CreateState::Tree { labels, stack, visited, pages } => {
                    match depth_first_search(&self.doc, stack, visited)? {
                        Some((objid, tree)) => {
                            let label = labels.next()?;
                            let page = PDFPage::new(self.doc.clone(), objid, &PDFObject::Dict(tree), label)?;
                            *pages = true;
                            return Ok(Some(page));
                        }
                        None => {
                            if *pages {
                                self.state = CreateState::Done;
                            } else {
                                let labels = std::mem::replace(labels, Labels::Repeat);
                                self.state = CreateState::Xrefs { labels, xref: 0, ids: None };
                            }
                        }
                    }
                }
                CreateState::Xrefs { labels, xref, ids } => {
                    // fallback when /Pages is missing.
                    let Some(ids_iter) = ids else {
                        let xrefs = self.doc.xrefs();
                        let Some(x) = xrefs.get(*xref).cloned() else {
                            self.state = CreateState::Done;
                            continue;
                        };
                        *ids = Some(x.objids());
                        continue;
                    };
                    let Some(objid) = ids_iter.next() else {
                        *ids = None;
                        *xref += 1;
                        continue;
                    };
                    let objid = objid?;
                    let mut attempt = || -> PyResult<Option<PDFPage>> {
                        let obj = self.doc.getobj(&objid)?;
                        if let PDFObject::Dict(d) = &obj {
                            if matches!(d.get("Type"), Some(PDFObject::Literal(t)) if t == *LITERAL_PAGE) {
                                let label = labels.next()?;
                                return Ok(Some(PDFPage::new(self.doc.clone(), objid.obj().clone(), &obj, label)?));
                            }
                        }
                        Ok(None)
                    };
                    match attempt() {
                        Ok(Some(page)) => return Ok(Some(page)),
                        Ok(None) => {}
                        Err(e) if e.is_instance(ExcKind::PDFObjectNotFound) => {}
                        Err(e) => return Err(e),
                    }
                }
            }
        }
    }
}

/// Resumes the `depth_first_search` chain: the next `(object_id,
/// object_properties)` of a `/Page`, or `None` once the walk is over.
///
/// The consumer resumes `create_pages` (its frame is taken by the
/// caller), which resumes each generator of the `yield from` chain: their
/// frames are back on the stack while the walk goes on, and a generator
/// that returns leaves it.
// An ObjId never holds a list or a dict, the only values whose hash could
// change (see pdftypes::ObjId): clippy only sees its RefCells.
#[allow(clippy::mutable_key_type)]
fn depth_first_search(
    doc: &Rc<PDFDocument>,
    stack: &mut Vec<Search>,
    visited: &mut HashSet<ObjId>,
) -> PyResult<Option<(PDFObject, PDFDict)>> {
    let mut frames: Vec<Guard> = Vec::new();
    for _ in stack.iter().filter(|s| s.started) {
        frames.push(py_frame()?);
    }
    loop {
        let Some(top) = stack.last_mut() else {
            return Ok(None);
        };
        if !top.started {
            // `depth_first_search`, nested in pdfminer/pdfpage.py:91 PDFPage.create_pages
            frames.push(py_frame()?);
            top.started = true;
            let (object_id, object_properties) = match &top.obj {
                PDFObject::Int(_) | PDFObject::Bool(_) => {
                    let object_id = top.obj.clone();
                    let obj = doc.getobj(&ObjId::new(object_id.clone())?)?;
                    (object_id, dict_value(&obj)?.copy())
                }
                obj => {
                    // This looks broken. obj.objid means obj could be either
                    // PDFObjRef or PDFStream, but neither is valid for dict_value.
                    let object_id = match obj {
                        PDFObject::Ref(r) => PDFObject::Int(r.objid().clone()),
                        PDFObject::Stream(s) => s.objid().map_or(PDFObject::Null, |id| id.obj().clone()),
                        other => {
                            return Err(exc::AttributeError(format!(
                                "'{}' object has no attribute 'objid'",
                                other.type_name()
                            )))
                        }
                    };
                    (object_id, dict_value(obj)?.copy())
                }
            };
            // Avoid recursion errors by keeping track of visited nodes
            let key = ObjId::new(object_id.clone())?;
            if visited.contains(&key) {
                frames.pop();
                stack.pop();
                continue;
            }
            visited.insert(key);
            for (k, v) in top.parent.items() {
                if INHERITABLE_ATTRS.contains(&&*k) && !object_properties.contains_key(&k) {
                    object_properties.insert(k, v);
                }
            }
            let mut object_type = object_properties.get("Type");
            if object_type.is_none() {
                // See #64 (not settings.STRICT)
                object_type = object_properties.get("type");
            }
            let is = |lit: &PSLiteral| matches!(&object_type, Some(PDFObject::Literal(t)) if t == lit);
            top.props = Some(object_properties.clone());
            if is(&LITERAL_PAGES) && object_properties.contains_key("Kids") {
                // log.debug("Pages: Kids=%r", object_properties["Kids"])
                drop(py_frames(2)?);
                let kids = list_value(&object_properties.get("Kids").expect("checked"))?.to_vec();
                top.kids = Some(kids.into_iter());
            } else if is(&LITERAL_PAGE) {
                // log.debug("Page: %r", object_properties)
                drop(py_frames(2)?);
                // the generator returns after this yield
                top.kids = Some(Vec::new().into_iter());
                return Ok(Some((object_id, object_properties)));
            } else {
                frames.pop();
                stack.pop();
            }
            continue;
        }
        // for child in kids: yield from depth_first_search(child, object_properties, visited)
        let next_kid = top.kids.as_mut().and_then(Iterator::next);
        match next_kid {
            Some(child) => {
                let parent = top.props.clone().expect("set with the kids");
                stack.push(Search { obj: child, parent, kids: None, props: None, started: false });
            }
            None => {
                frames.pop();
                stack.pop();
            }
        }
    }
}

impl Iterator for CreatePages {
    type Item = PyResult<PDFPage>;

    fn next(&mut self) -> Option<PyResult<PDFPage>> {
        if self.done {
            return None;
        }
        match self.step() {
            Ok(Some(page)) => Some(Ok(page)),
            Ok(None) => {
                self.done = true;
                None
            }
            Err(e) => {
                // an exception finishes the generator
                self.done = true;
                self.state = CreateState::Done;
                Some(Err(e))
            }
        }
    }
}

/// The generator `PDFPage.get_pages(...)`.
pub struct GetPages {
    pages: CreatePages,
    pagenos: Option<HashSet<usize>>,
    maxpages: usize,
    pageno: usize,
    done: bool,
}

impl Iterator for GetPages {
    type Item = PyResult<PDFPage>;

    fn next(&mut self) -> Option<PyResult<PDFPage>> {
        // Process each page contained in the document.
        while !self.done {
            let page = match self.pages.next()? {
                Ok(page) => page,
                Err(e) => {
                    self.done = true;
                    return Some(Err(e));
                }
            };
            let pageno = self.pageno;
            self.pageno += 1;
            if self.pagenos.as_ref().is_some_and(|p| !p.contains(&pageno)) {
                continue;
            }
            if self.maxpages != 0 && self.maxpages <= pageno + 1 {
                self.done = true;
            }
            return Some(Ok(page));
        }
        None
    }
}
