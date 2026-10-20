//! Port of `pdfplumber/structure.py`: the structure tree of a tagged PDF
//! (`PDFStructTree`, `PDFStructElement`), for the document or for a page.
//!
//! Wave 4 (base): the interface, bodies `todo!()` (workstream DOC,
//! `docs/architecture.md`, "Vague 4").
//!
//! # Values
//!
//! An element's fields hold what pdfplumber puts there: `type` a `str`
//! (`decode_text` of `/S`, through `/RoleMap`), `revision` the raw `/R`
//! (any object), `id`, `lang`, `alt_text`, `actual_text`, `title`
//! `decode_text` of the resolved value (any object: a list of ints goes
//! through `str(s)` when one is past 255, `crate::utils::pdfinternals`),
//! `page_number` an `int` or `None`, `attributes` a dict of decoded names and
//! raw PDF objects, `mcids` the MCIDs as found (ints, or any `/MCID`
//! value), `children` elements (shared: a DAG's element appears under each
//! parent that reaches it). Elements are built first and filled in place
//! (`mcids`, `children`): [`PDFStructElement`] keeps those two in
//! `RefCell`s. The `seen` maps of the parsers are keyed by `repr(ref)` (a
//! `str`; `repr` of any object, with its C recursion).
//!
//! `to_dict` is `dataclasses.asdict` then pruning: `asdict` deep-copies
//! the field values (`copy.deepcopy`): a `PDFObjRef` (or a `PDFStream` of
//! an encrypted document, whose `decipher` is bound to the document) in
//! `attributes` copies its document, whose file object refuses to be
//! copied when it is an `io.BufferedReader` (`TypeError: cannot pickle
//! '_io.BufferedReader' object`), and is copied whole for an `io.BytesIO`
//! (JavaScript): the result depends on `pdfminer::pdfparser::FileKind`.
//!
//! # Recursion
//!
//! `_find_all`, `all_mcids`, `to_dict`'s pruning and the parsers walk with
//! deques (no recursion), but `prune` (nested in `_parse_struct_tree`)
//! recurses once per level of the tree, `asdict` recurses through
//! `_asdict_inner` (a frame per element, and per list of children with the
//! generator expression that builds it: about three frames per level) and
//! through `copy.deepcopy` for the attribute values, and the dataclass
//! `repr` through `reprlib.recursive_repr`'s wrapper and the `repr` of the
//! `children` list (C units). A tree a few hundred levels deep raises
//! `RecursionError` in `to_dict` (so in `pdf.structure_tree`), one about
//! three times deeper in the parsers; adversarial corpus `adv-structure`
//! puts windows around each first failing depth.

use std::cell::RefCell;
use std::rc::Rc;

use pdfminer::pdfdocument::PDFDocument;
use pdfminer::pdftypes::PDFObject;
use pycompat::{PyInt, PyResult, PyStr};

use crate::obj::{Bbox, Obj, PyValue};
use crate::page::Page;

/// What `find_all` and `find` match
/// (pdfplumber/structure.py:39 `_find_all`): an element type (`x.type == matcher`), or a function of
/// the element (`matcher(x)`, or `matcher.match(x.type)` of an
/// `re.Pattern`: the JavaScript API gives a `RegExp` as a function).
pub enum Matcher<'a> {
    Tag(&'a PyStr),
    Func(&'a dyn Fn(&PDFStructElement) -> PyResult<bool>),
}

/// pdfplumber/structure.py:102 `PDFStructElement` (a dataclass).
pub struct PDFStructElement {
    pub r#type: PyStr,
    pub revision: PyValue,
    pub id: Option<PyStr>,
    pub lang: Option<PyStr>,
    pub alt_text: Option<PyStr>,
    pub actual_text: Option<PyStr>,
    pub title: Option<PyStr>,
    pub page_number: Option<PyInt>,
    pub attributes: Obj,
    pub mcids: RefCell<Vec<PyValue>>,
    pub children: RefCell<Vec<Rc<PDFStructElement>>>,
}

impl PDFStructElement {
    // pdfplumber/structure.py:118 PDFStructElement.all_mcids
    /// `(page_number, mcid)` of this element and its descendants,
    /// depth-first.
    pub fn all_mcids(&self) -> PyResult<Vec<(Option<PyInt>, PyValue)>> {
        todo!("pdfplumber/structure.py:118 PDFStructElement.all_mcids")
    }

    // pdfplumber/structure.py:132 PDFStructElement.to_dict
    /// `asdict(self)`, then every `None`, `[]` and `{}` value removed
    /// (see the module documentation).
    pub fn to_dict(&self) -> PyResult<PyValue> {
        todo!("pdfplumber/structure.py:132 PDFStructElement.to_dict")
    }

    /// `repr(element)`: the dataclass `__repr__` (fields in order,
    /// children included).
    pub fn repr(&self) -> PyResult<PyStr> {
        todo!("the dataclass __repr__ of pdfplumber/structure.py:102 PDFStructElement")
    }

    // pdfplumber/structure.py:75 Findable.find_all
    pub fn find_all(&self, matcher: &Matcher<'_>) -> PyResult<Vec<Rc<PDFStructElement>>> {
        let _ = matcher;
        todo!("pdfplumber/structure.py:75 Findable.find_all")
    }

    // pdfplumber/structure.py:86 Findable.find
    pub fn find(&self, matcher: &Matcher<'_>) -> PyResult<Option<Rc<PDFStructElement>>> {
        let _ = matcher;
        todo!("pdfplumber/structure.py:86 Findable.find")
    }
}

/// The scope of a tree: the document (its pages, `doc.pages`: built
/// first), or one page.
pub enum Scope<'a> {
    Document(&'a [Rc<Page>]),
    Page(&'a Rc<Page>),
}

/// pdfplumber/structure.py:151 `PDFStructTree`
pub struct PDFStructTree {
    /// `self.doc` (the `PDFDocument` of the `PDF`)
    pub doc: Rc<PDFDocument>,
    /// `self.root`: `resolve1(catalog["StructTreeRoot"])`
    pub root: PDFObject,
    pub role_map: PDFObject,
    pub class_map: PDFObject,
    pub children: Vec<Rc<PDFStructElement>>,
    /// `self.page`
    pub page: Option<Rc<Page>>,
    /// `self.pages`: page number -> page
    pub pages: Vec<(PyInt, Rc<Page>)>,
    /// `self.page_dict`: `pageid` -> page number (document scope only)
    pub page_dict: Option<Vec<(PDFObject, PyInt)>>,
}

impl PDFStructTree {
    // pdfplumber/structure.py:171 PDFStructTree.__init__
    /// `PDFStructTree(pdf)` or `PDFStructTree(pdf, page)`:
    /// `StructTreeMissing("PDF has no structure")` without
    /// `/StructTreeRoot`; for a page, the page's `/StructParents` in the
    /// `/ParentTree` (`NumberTree`), or the whole tree when there is no
    /// parent tree.
    pub fn new(doc: &Rc<PDFDocument>, scope: Scope<'_>) -> PyResult<PDFStructTree> {
        let _ = (doc, scope);
        todo!("pdfplumber/structure.py:171 PDFStructTree.__init__")
    }

    // pdfplumber/structure.py:217 PDFStructTree._make_attributes
    pub fn _make_attributes(&self, obj: &PDFObject, revision: &PDFObject) -> PyResult<Obj> {
        let _ = (obj, revision);
        todo!("pdfplumber/structure.py:217 PDFStructTree._make_attributes")
    }

    // pdfplumber/structure.py:266 PDFStructTree._make_element
    pub fn _make_element(&self, obj: &PDFObject) -> PyResult<(Option<Rc<PDFStructElement>>, Vec<PDFObject>)> {
        let _ = obj;
        todo!("pdfplumber/structure.py:266 PDFStructTree._make_element")
    }

    // pdfplumber/structure.py:339 PDFStructTree.on_parsed_page
    pub fn on_parsed_page(&self, obj: &PDFObject) -> PyResult<bool> {
        let _ = obj;
        todo!("pdfplumber/structure.py:339 PDFStructTree.on_parsed_page")
    }

    // pdfplumber/structure.py:466 PDFStructTree.element_bbox
    /// The element's `/BBox` attribute on its page (cropped by a
    /// `CroppedPage`'s `_crop_fn`: `IndexError("Element no longer on
    /// page")`), else the bbox of the page objects with its MCIDs
    /// (`IndexError("No objects found")`).
    pub fn element_bbox(&self, el: &PDFStructElement) -> PyResult<Bbox> {
        let _ = el;
        todo!("pdfplumber/structure.py:466 PDFStructTree.element_bbox")
    }

    // pdfplumber/structure.py:75 Findable.find_all
    pub fn find_all(&self, matcher: &Matcher<'_>) -> PyResult<Vec<Rc<PDFStructElement>>> {
        let _ = matcher;
        todo!("pdfplumber/structure.py:75 Findable.find_all")
    }

    // pdfplumber/structure.py:86 Findable.find
    pub fn find(&self, matcher: &Matcher<'_>) -> PyResult<Option<Rc<PDFStructElement>>> {
        let _ = matcher;
        todo!("pdfplumber/structure.py:86 Findable.find")
    }
}
