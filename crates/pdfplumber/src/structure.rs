//! Port of `pdfplumber/structure.py`: the structure tree of a tagged PDF
//! (`PDFStructTree`, `PDFStructElement`), for the document or for a page.
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
//! parent that reaches it, as many times as it is reached). Elements are
//! built first and filled in place (`mcids`, `children`):
//! [`PDFStructElement`] keeps those two in `RefCell`s. The `seen` maps of
//! the parsers are keyed by `repr(ref)` (a `str`; `repr` of any object, with
//! its C units).
//!
//! The objects of the document are read with Python's operators, whatever
//! their type ([`py_in`], [`py_getitem`], [`py_get`], [`py_iter`], the
//! attributes `.name`, `.objid`, `.items()`): a structure tree is any
//! object graph, and each of these raises where Python raises (`TypeError`,
//! `AttributeError`, `KeyError`, the `PDFKeyError` of a stream...). A
//! [`PDFObject::Str`] is what pdfplumber's metadata decoding may have left
//! in a dict (`crate::pdf`, "Metadata").
//!
//! `to_dict` is `dataclasses.asdict` then pruning: `asdict` rebuilds lists
//! and dicts and deep-copies every other value (`copy.deepcopy`): a
//! `PDFObjRef` (or a `PDFStream` of an encrypted document, whose `decipher`
//! is a method of the document) copies its document, whose file object
//! refuses to be copied when it is an `io.BufferedReader` (`TypeError:
//! cannot pickle 'BufferedReader' instances`), and is copied whole for an
//! `io.BytesIO` (JavaScript): the result depends on
//! `pdfminer::pdfparser::FileKind` ([`PDFStructElement::to_dict`]).
//!
//! # Laziness
//!
//! `find_all` is a generator in Python: [`FindAll`] is an iterator that
//! does the same work in the same order (`find` stops at the first match;
//! a matcher's exception comes when the walk reaches it). The trees are
//! built at once, by the constructor.
//!
//! # Recursion
//!
//! `_find_all`, `all_mcids`, `to_dict`'s pruning and the parsers walk with
//! deques (no recursion), but `prune` (nested in `_parse_struct_tree`)
//! recurses once per level of the tree, `asdict` recurses through
//! `_asdict_inner` (a frame per element, and per list of children with the
//! generator expression that builds it from C: three frames and two C units
//! per level) and through `copy.deepcopy` for the leaves, and the dataclass
//! `repr` through `dataclasses._recursive_repr`'s wrapper (run by C) and the
//! `repr` of the `children` list (two frames and four C units per level). A
//! tree a few hundred levels deep raises `RecursionError` in `to_dict`
//! (so in `pdf.structure_tree`), one about three times deeper in `prune`;
//! adversarial corpus `adv-structure` puts windows around each first
//! failing depth. The standard library's frames are constants measured with
//! `oracle/depth_probe.py` ([`frames`]).
//!
//! # Not modeled
//!
//! A cycle among the elements a page's parent tree reaches makes
//! `_resolve_children` loop for ever, appending to the elements' lists
//! until memory runs out: the port raises `MemoryError` once the walk has
//! visited every element it reaches and only repeats them
//! ([`PDFStructTree::_resolve_children`]). The deep copy of a whole
//! document (a `PDFObjRef` in an element of a document read from bytes) is
//! not run: the port keeps the reference, an equal value, and counts no
//! frame for it.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;

use pdfminer::data_structures::NumberTree;
use pdfminer::pdfdocument::PDFDocument;
use pdfminer::pdfparser::{FileKind, KEYWORD_NULL};
use pdfminer::pdftypes::{resolve1, ObjId, PDFDict, PDFList, PDFObject};
use pdfminer::psparser::LiteralName;
use pycompat::logging::Logger;
use pycompat::recursion::{c_call, c_called_leaf, class_call, class_call_leaf, eval_entry, leaf, py_frame};
use pycompat::repr::str_repr;
use pycompat::{exc, PyInt, PyResult, PyStr};

use crate::obj::{Bbox, Obj, PyValue};
use crate::page::{_invert_box, _normalize_box, apply_crop_fn, Page, PageKind};
use crate::pdf::PDF;
use crate::utils::geometry;
use crate::utils::pdfinternals::{decode_text, decode_text_value, TextArg};

// pdfplumber/structure.py:28 logger
static LOG: Logger = Logger::new("pdfplumber.structure");

const WHILE_CALLING: &str = " while calling a Python object";
const WHILE_REPR: &str = " while getting the repr of an object";

/// The Python frames of the standard library code that `to_dict` and
/// `repr` run (measured with `oracle/depth_probe.py`: `structure`).
pub mod frames {
    /// `dataclasses._is_dataclass_instance(obj)`: a Python function.
    pub const IS_DATACLASS_INSTANCE: u32 = 1;
    /// `copy.deepcopy` of a `PSLiteral` or a `PSKeyword`: `deepcopy`,
    /// `_reconstruct`, `deepcopy` of the state, `_deepcopy_dict`,
    /// `deepcopy` of the name, `_deepcopy_atomic` (the same the first time
    /// a process copies the class).
    pub const DEEPCOPY_NAME: u32 = 6;
    /// `copy.deepcopy` of a `PDFObjRef`, down to the `TypeError` of the
    /// file object: the deepest frame is in the document's cross-reference
    /// sections (`xrefs` comes before `_parser` in the document's
    /// `__dict__`), a reference of a trailer copied down to its object id:
    /// 22. Not modeled: a document whose `xrefs`, `info` or `catalog` hold
    /// direct objects nested deeper than that.
    pub const DEEPCOPY_DOCUMENT: u32 = 22;
}

// ---------------------------------------------------------------------------
// Python's operators on the objects of a structure tree

/// `None` (`PDFObject::Null` is Python's `None`).
const NONE: PDFObject = PDFObject::Null;

/// `key in obj` for a `str` key: a dict's keys, a stream's attributes
/// (`PDFStream.__contains__`, a Python method run by C), a list's items, a
/// substring of a `str`; `TypeError` for the rest.
pub fn py_in(obj: &PDFObject, key: &str) -> PyResult<bool> {
    if let PDFObject::Stream(s) = obj {
        c_called_leaf()?;
        return Ok(s.contains(key));
    }
    obj.py_contains_str(key)
}

/// `obj[key]` for a `str` key.
pub fn py_getitem(obj: &PDFObject, key: &str) -> PyResult<PDFObject> {
    match obj {
        PDFObject::Dict(d) => d.get(key).ok_or_else(|| exc::KeyError(str_repr(key))),
        PDFObject::Stream(s) => s.getitem(key),
        PDFObject::List(_) => Err(exc::TypeError("list indices must be integers or slices, not str")),
        PDFObject::Bytes(_) => Err(exc::TypeError("byte indices must be integers or slices, not str")),
        PDFObject::Str(_) => Err(exc::TypeError("string indices must be integers, not 'str'")),
        other => Err(exc::TypeError(format!("'{}' object is not subscriptable", other.type_name()))),
    }
}

/// `obj.get(key, default)`: a dict's or a stream's (`PDFStream.get`, a
/// Python method); `AttributeError` for the rest.
pub fn py_get(obj: &PDFObject, key: &str, default: PDFObject) -> PyResult<PDFObject> {
    match obj {
        PDFObject::Dict(d) => Ok(d.get(key).unwrap_or(default)),
        PDFObject::Stream(s) => s.get_counted(key, &default),
        other => Err(no_attribute(other, "get")),
    }
}

/// `AttributeError: 'T' object has no attribute 'name'`
fn no_attribute(obj: &PDFObject, name: &str) -> pycompat::PyErr {
    exc::AttributeError(format!("'{}' object has no attribute '{name}'", obj.type_name()))
}

/// `decode_text(obj.name)`: the name of a literal (`str` or `bytes`) or of
/// a keyword (`bytes`).
fn decode_name(obj: &PDFObject) -> PyResult<String> {
    match obj {
        PDFObject::Literal(lit) => match lit.name() {
            LiteralName::Str(s) => decode_text(TextArg::Str(s)),
            LiteralName::Bytes(b) => decode_text(TextArg::Bytes(b)),
        },
        PDFObject::Keyword(k) => decode_text(TextArg::Bytes(k.name())),
        other => Err(no_attribute(other, "name")),
    }
}

/// `obj.objid`: a reference's object number, a stream's id (`None` until
/// the document sets it).
fn objid_attr(obj: &PDFObject) -> PyResult<PDFObject> {
    match obj {
        PDFObject::Ref(r) => Ok(PDFObject::Int(r.objid().clone())),
        PDFObject::Stream(s) => Ok(s.objid().map_or(NONE, |id| id.obj().clone())),
        other => Err(no_attribute(other, "objid")),
    }
}

/// `iter(obj)`, run to the end (`for x in obj`, `deque(obj)`): a list's
/// items, a `bytes`'s ints, a `str`'s characters, a dict's keys; a stream
/// is iterated by `__getitem__(0)`, which raises its `PDFKeyError`.
pub fn py_iter(obj: &PDFObject) -> PyResult<Vec<PDFObject>> {
    match obj {
        PDFObject::List(l) => Ok(l.to_vec()),
        PDFObject::Bytes(b) => Ok(b.iter().map(|&c| PDFObject::Int(PyInt::Small(i64::from(c)))).collect()),
        PDFObject::Str(s) => Ok(s.code_points().map(|c| PDFObject::Str(PyStr::from_code_point(c))).collect()),
        PDFObject::Dict(d) => Ok(d.keys().into_iter().map(|k| PDFObject::Str(PyStr::new(&k))).collect()),
        PDFObject::Stream(s) => {
            // the sequence iterator calls PDFStream.__getitem__(0) from C
            c_called_leaf()?;
            Err(s.missing_attribute("0"))
        }
        other => Err(exc::TypeError(format!("'{}' object is not iterable", other.type_name()))),
    }
}

/// `isinstance(x, int)` (a `bool` is an `int`).
fn is_int(x: &PDFObject) -> bool {
    matches!(x, PDFObject::Int(_) | PDFObject::Bool(_))
}

/// The builtin `repr(x)`: the call's unit, then `PyObject_Repr`.
fn builtin_repr(x: &PDFObject) -> PyResult<String> {
    let _call = c_call(WHILE_CALLING)?;
    x.repr()
}

/// `"...%s" % obj`: `str(obj)` (a dict or a list as the only argument).
fn percent_s(prefix: &str, obj: &PDFObject) -> PyResult<String> {
    Ok(format!("{prefix}{}", obj.py_str()?))
}

/// `resolve1(x)`
fn resolve(x: &PDFObject) -> PyResult<PDFObject> {
    resolve1(x, &NONE)
}

/// A value of the document as an element holds it.
fn value(x: PDFObject) -> PyValue {
    PyValue::from_pdf(x)
}

// ---------------------------------------------------------------------------
// Elements

/// What `find_all` and `find` match
/// (pdfplumber/structure.py:39 `_find_all`): an element type (`x.type ==
/// matcher`, pdfplumber/structure.py:47 `match_tag`), or a function of the
/// element (`matcher(x)`, its result taken for its truth; `matcher.match(x.type)`
/// of an `re.Pattern`, pdfplumber/structure.py:51 `match_regex`: the
/// JavaScript API gives a `RegExp` as a function). The function takes its
/// own frames.
pub enum Matcher<'a> {
    Tag(&'a PyStr),
    Func(&'a dyn Fn(&PDFStructElement) -> PyResult<bool>),
}

impl Matcher<'_> {
    fn matches(&self, el: &PDFStructElement) -> PyResult<bool> {
        match self {
            Matcher::Tag(tag) => {
                // match_tag, a nested function
                leaf(1)?;
                Ok(&el.r#type == *tag)
            }
            Matcher::Func(f) => f(el),
        }
    }
}

// pdfplumber/structure.py:39 _find_all
/// Common code for `find_all()` in trees and elements: the elements of
/// `elements` and their descendants, depth-first, that `matcher` matches
/// (a generator: each [`Iterator::next`] is a resumption).
pub struct FindAll<'a> {
    d: VecDeque<Rc<PDFStructElement>>,
    matcher: Matcher<'a>,
    /// The element last yielded, whose children go to the front of the
    /// deque when the generator resumes.
    pending: Option<Rc<PDFStructElement>>,
    done: bool,
}

impl<'a> FindAll<'a> {
    fn new(elements: &[Rc<PDFStructElement>], matcher: Matcher<'a>) -> PyResult<FindAll<'a>> {
        // the generator is created (its frame checked); `d = deque(elements)`
        // runs at its first resumption, on the list as it then is (the
        // children of an element do not change once the tree is built)
        leaf(1)?;
        Ok(FindAll { d: elements.iter().cloned().collect(), matcher, pending: None, done: false })
    }
}

impl Iterator for FindAll<'_> {
    type Item = PyResult<Rc<PDFStructElement>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let step = (|| -> PyResult<Option<Rc<PDFStructElement>>> {
            let _frame = py_frame()?;
            if let Some(el) = self.pending.take() {
                // d.extendleft(reversed(el.children))
                for child in el.children.borrow().iter().rev() {
                    self.d.push_front(child.clone());
                }
            }
            while let Some(el) = self.d.pop_front() {
                if self.matcher.matches(&el)? {
                    self.pending = Some(el.clone());
                    return Ok(Some(el));
                }
                for child in el.children.borrow().iter().rev() {
                    self.d.push_front(child.clone());
                }
            }
            Ok(None)
        })();
        match step {
            Ok(Some(el)) => Some(Ok(el)),
            Ok(None) => {
                self.done = true;
                None
            }
            Err(e) => {
                self.done = true;
                Some(Err(e))
            }
        }
    }
}

// pdfplumber/structure.py:75 Findable.find_all
fn find_all<'a>(children: &[Rc<PDFStructElement>], matcher: Matcher<'a>) -> PyResult<FindAll<'a>> {
    let _frame = py_frame()?;
    FindAll::new(children, matcher)
}

// pdfplumber/structure.py:86 Findable.find
fn find(children: &[Rc<PDFStructElement>], matcher: Matcher<'_>) -> PyResult<Option<Rc<PDFStructElement>>> {
    let _frame = py_frame()?;
    let mut it = FindAll::new(children, matcher)?;
    // next(...): a builtin, which resumes the generator from C
    let _call = c_call(WHILE_CALLING)?;
    let _entry = eval_entry()?;
    // try: return next(...) except StopIteration: return None
    it.next().transpose()
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

/// The fields of a `PDFStructElement`, in order (`dataclasses.fields`).
const FIELDS: [&str; 11] = [
    "type",
    "revision",
    "id",
    "lang",
    "alt_text",
    "actual_text",
    "title",
    "page_number",
    "attributes",
    "mcids",
    "children",
];

fn opt_str(s: &Option<PyStr>) -> PyValue {
    s.clone().map_or(PyValue::None, PyValue::Str)
}

impl PDFStructElement {
    /// The value of a field that is not a container of elements.
    fn field(&self, name: &str) -> PyValue {
        match name {
            "type" => PyValue::Str(self.r#type.clone()),
            "revision" => self.revision.clone(),
            "id" => opt_str(&self.id),
            "lang" => opt_str(&self.lang),
            "alt_text" => opt_str(&self.alt_text),
            "actual_text" => opt_str(&self.actual_text),
            "title" => opt_str(&self.title),
            "page_number" => self.page_number.clone().map_or(PyValue::None, PyValue::Int),
            "attributes" => PyValue::Dict(self.attributes.clone()),
            "mcids" => PyValue::list(self.mcids.borrow().iter().cloned()),
            other => unreachable!("not a value field: {other}"),
        }
    }

    // pdfplumber/structure.py:115 PDFStructElement.__iter__
    /// `iter(element)`: its children.
    pub fn iter(&self) -> Vec<Rc<PDFStructElement>> {
        self.children.borrow().clone()
    }

    // pdfplumber/structure.py:118 PDFStructElement.all_mcids
    /// Collect all MCIDs (with their page numbers, if there are multiple
    /// pages in the tree) inside a structure element: `(page_number,
    /// mcid)` of this element and its descendants, depth-first (a
    /// generator, consumed to the end).
    pub fn all_mcids(&self) -> PyResult<Vec<(Option<PyInt>, PyValue)>> {
        let _frame = py_frame()?;
        let mut out = Vec::new();
        // Collect them depth-first to preserve ordering
        for mcid in self.mcids.borrow().iter() {
            out.push((self.page_number.clone(), mcid.clone()));
        }
        let mut d: VecDeque<Rc<PDFStructElement>> = self.children.borrow().iter().cloned().collect();
        while let Some(el) = d.pop_front() {
            for mcid in el.mcids.borrow().iter() {
                out.push((el.page_number.clone(), mcid.clone()));
            }
            for child in el.children.borrow().iter().rev() {
                d.push_front(child.clone());
            }
        }
        Ok(out)
    }

    // pdfplumber/structure.py:132 PDFStructElement.to_dict
    /// Return a compacted dict representation: `asdict(self)` (see the
    /// module documentation: `file` is the class of the document's file
    /// object, which decides what the deep copy of a reference does), then
    /// every `None`, `[]` and `{}` value removed from the element dicts.
    pub fn to_dict(&self, file: FileKind) -> PyResult<PyValue> {
        let _frame = py_frame()?;
        let mut r = asdict(self, file)?;
        // Prune empty values (does not matter in which order): Python walks
        // the element dicts with a deque; an `Obj` being a value (copy on
        // write), each pruned child is put back into its parent's list.
        // The depth is the tree's, which asdict bounds.
        prune_dict(&mut r)?;
        Ok(PyValue::Dict(r))
    }

    /// `repr(element)`: the dataclass `__repr__` (fields in order, children
    /// included), as `PyObject_Repr` runs it (its unit, then the wrapper of
    /// `dataclasses._recursive_repr`, a Python function run by C, and the
    /// generated `__repr__`).
    pub fn repr(&self) -> PyResult<PyStr> {
        let _unit = c_call(WHILE_REPR)?;
        self.repr_inner()
    }

    fn repr_inner(&self) -> PyResult<PyStr> {
        // wrapper (run by slot_tp_repr from C), then user_function(self)
        let _entry = eval_entry()?;
        let _wrapper = py_frame()?;
        let _repr = py_frame()?;
        let mut out = String::from("PDFStructElement(");
        for (i, name) in FIELDS.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            out.push_str(name);
            out.push('=');
            if *name == "children" {
                // PyObject_Repr(list): its unit, then each element's
                let _list = c_call(WHILE_REPR)?;
                out.push('[');
                for (j, child) in self.children.borrow().iter().enumerate() {
                    if j > 0 {
                        out.push_str(", ");
                    }
                    let _unit = c_call(WHILE_REPR)?;
                    out.push_str(&child.repr_inner()?.to_string_lossy());
                }
                out.push(']');
            } else {
                out.push_str(&self.field(name).repr_counted()?);
            }
        }
        out.push(')');
        Ok(PyStr::from(out))
    }

    // pdfplumber/structure.py:75 Findable.find_all
    /// Iterate depth-first over matching elements in subtree.
    pub fn find_all<'a>(&self, matcher: Matcher<'a>) -> PyResult<FindAll<'a>> {
        find_all(&self.children.borrow(), matcher)
    }

    // pdfplumber/structure.py:86 Findable.find
    /// Find the first matching element in subtree.
    pub fn find(&self, matcher: Matcher<'_>) -> PyResult<Option<Rc<PDFStructElement>>> {
        let children = self.children.borrow().clone();
        find(&children, matcher)
    }
}

/// `el[k] == []`
fn is_empty_list(v: &PyValue) -> bool {
    matches!(v, PyValue::List(items) if items.is_empty())
}

/// `el[k] == {}`
fn is_empty_dict(v: &PyValue) -> bool {
    matches!(v, PyValue::Dict(d) if d.is_empty())
}

/// The pruning of `to_dict` on an element dict and the element dicts of
/// its `children`: every value that `is None`, `== []` or `== {}` removed.
fn prune_dict(el: &mut Obj) -> PyResult<()> {
    for k in el.keys().cloned().collect::<Vec<_>>() {
        let v = el.get(&k).expect("a key of the dict");
        if v.is_none() || is_empty_list(v) || is_empty_dict(v) {
            el.remove(&k)?;
        }
    }
    if let Some(PyValue::List(children)) = el.get("children") {
        let mut pruned = Vec::with_capacity(children.len());
        for child in children.iter() {
            let PyValue::Dict(c) = child else { unreachable!("asdict makes a dict of each child element") };
            let mut c = c.clone();
            prune_dict(&mut c)?;
            pruned.push(PyValue::Dict(c));
        }
        el.set("children", PyValue::list(pruned));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// dataclasses.asdict

/// `dataclasses.asdict(element)` (CPython 3.12 `Lib/dataclasses.py`):
/// `asdict`, then `_asdict_inner`.
fn asdict(el: &PDFStructElement, file: FileKind) -> PyResult<Obj> {
    let _frame = py_frame()?;
    leaf(frames::IS_DATACLASS_INSTANCE)?;
    asdict_element(el, file)
}

/// `_asdict_inner` of an element: `{f.name: _asdict_inner(getattr(obj,
/// f.name), dict) for f in fields(obj)}` (a comprehension: no frame of its
/// own).
fn asdict_element(el: &PDFStructElement, file: FileKind) -> PyResult<Obj> {
    let _frame = py_frame()?;
    // type(obj) in _ATOMIC_TYPES; _is_dataclass_instance(obj)
    leaf(frames::IS_DATACLASS_INSTANCE)?;
    // fields(obj): `tuple(f for f in fields.values() if ...)`, its generator
    // expression resumed by tuple() from C
    {
        let _fields = py_frame()?;
        let _entry = eval_entry()?;
        leaf(1)?;
    }
    let mut out = Obj::new();
    for name in FIELDS {
        let v = if name == "children" {
            asdict_children(&el.children.borrow(), file)?
        } else {
            asdict_value(&el.field(name), file)?
        };
        out.set(name, v);
    }
    Ok(out)
}

/// `_asdict_inner` of the `children` list: `type(obj)(_asdict_inner(v,
/// dict_factory) for v in obj)`.
fn asdict_children(children: &[Rc<PDFStructElement>], file: FileKind) -> PyResult<PyValue> {
    let _frame = py_frame()?;
    leaf(frames::IS_DATACLASS_INSTANCE)?;
    // the generator expression, created then resumed by list() from C
    leaf(1)?;
    let _entry = eval_entry()?;
    let _genexpr = py_frame()?;
    let mut items = Vec::with_capacity(children.len());
    for child in children {
        items.push(PyValue::Dict(asdict_element(child, file)?));
    }
    Ok(PyValue::list(items))
}

/// `_asdict_inner` of a value: atomic types as they are, lists and dicts
/// rebuilt (their items through `_asdict_inner`), anything else
/// `copy.deepcopy`.
fn asdict_value(v: &PyValue, file: FileKind) -> PyResult<PyValue> {
    let _frame = py_frame()?;
    match v {
        // type(obj) in _ATOMIC_TYPES
        PyValue::None
        | PyValue::Bool(_)
        | PyValue::Int(_)
        | PyValue::Float(_)
        | PyValue::Str(_)
        | PyValue::Bytes(_) => Ok(v.clone()),
        PyValue::Pdf(PDFObject::List(l)) => asdict_list(l.to_vec().into_iter().map(PyValue::from_pdf).collect(), file),
        PyValue::List(items) => asdict_list(items.to_vec(), file),
        PyValue::Pdf(PDFObject::Dict(d)) => {
            asdict_dict(d.items().into_iter().map(|(k, v)| (PyStr::new(&k), PyValue::from_pdf(v))).collect(), file)
        }
        PyValue::Dict(d) => asdict_dict(d.items().to_vec(), file),
        other => {
            leaf(frames::IS_DATACLASS_INSTANCE)?;
            deepcopy(other, file)
        }
    }
}

/// `type(obj)(_asdict_inner(v, dict_factory) for v in obj)` of a list.
fn asdict_list(items: Vec<PyValue>, file: FileKind) -> PyResult<PyValue> {
    leaf(frames::IS_DATACLASS_INSTANCE)?;
    leaf(1)?;
    let _entry = eval_entry()?;
    let _genexpr = py_frame()?;
    let mut out = Vec::with_capacity(items.len());
    for v in &items {
        out.push(asdict_value(v, file)?);
    }
    Ok(PyValue::list(out))
}

/// `type(obj)((_asdict_inner(k, ...), _asdict_inner(v, ...)) for k, v in
/// obj.items())` of a dict (its keys are `str`: atomic).
fn asdict_dict(items: Vec<(PyStr, PyValue)>, file: FileKind) -> PyResult<PyValue> {
    leaf(frames::IS_DATACLASS_INSTANCE)?;
    leaf(1)?;
    let _entry = eval_entry()?;
    let _genexpr = py_frame()?;
    let mut out = Obj::new();
    for (k, v) in &items {
        leaf(1)?;
        out.set(k, asdict_value(v, file)?);
    }
    Ok(PyValue::Dict(out))
}

/// `copy.deepcopy(obj)` of a value that is not a list, a dict or an
/// atomic type: a literal or a keyword is copied (an equal value), a
/// reference, or a stream whose `decipher` is the document's, copies its
/// document, which an `io.BufferedReader` refuses.
fn deepcopy(v: &PyValue, file: FileKind) -> PyResult<PyValue> {
    let copies_document = match v {
        PyValue::Pdf(PDFObject::Ref(_)) => true,
        PyValue::Pdf(PDFObject::Stream(s)) => s.decipher.is_some(),
        _ => false,
    };
    if copies_document {
        leaf(frames::DEEPCOPY_DOCUMENT)?;
        if file == FileKind::BufferedReader {
            return Err(exc::TypeError("cannot pickle 'BufferedReader' instances"));
        }
    } else {
        leaf(frames::DEEPCOPY_NAME)?;
    }
    Ok(v.clone())
}

// ---------------------------------------------------------------------------
// The tree

/// pdfplumber/structure.py:147 `StructTreeMissing` (a `ValueError`).
pub fn struct_tree_missing() -> pycompat::PyErr {
    pycompat::PyErr::new(pycompat::ExcKind::StructTreeMissing, "PDF has no structure")
}

/// The scope of a tree: the document (whose `pages` the constructor reads
/// after the root, `/RoleMap` and `/ClassMap`, as Python does), or one
/// page.
pub enum Scope<'a> {
    Document(&'a PDF),
    Page(&'a Rc<Page>),
}

/// What the parsers record for an element (`s[repr(ref)] = element,
/// children`): the element and its children as found (any object, then
/// pruned to a list).
type Seen = HashMap<String, (Rc<PDFStructElement>, PDFObject)>;

/// pdfplumber/structure.py:151 `PDFStructTree`: parse the structure tree of
/// a PDF, the portion of it that reaches marked content sections, for a
/// single page or for the whole document.
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
    pub page_dict: Option<HashMap<ObjId, PyInt>>,
}

impl PDFStructTree {
    // pdfplumber/structure.py:171 PDFStructTree.__init__
    /// `PDFStructTree(pdf)` or `PDFStructTree(pdf, page)` (`doc` is
    /// `pdf.doc`): `StructTreeMissing("PDF has no structure")` without
    /// `/StructTreeRoot`; for a page, the page's `/StructParents` in the
    /// `/ParentTree` (`NumberTree`), or the whole tree when there is no
    /// parent tree.
    pub fn new(doc: &Rc<PDFDocument>, scope: Scope<'_>) -> PyResult<PDFStructTree> {
        let _call = class_call()?;
        let _frame = py_frame()?;
        let catalog = doc.catalog();
        if !catalog.contains_key("StructTreeRoot") {
            return Err(struct_tree_missing());
        }
        let root = resolve(&catalog.get("StructTreeRoot").expect("checked"))?;
        let role_map = resolve(&py_get(&root, "RoleMap", PDFObject::Dict(PDFDict::new()))?)?;
        let class_map = resolve(&py_get(&root, "ClassMap", PDFObject::Dict(PDFDict::new()))?)?;
        let mut tree = PDFStructTree {
            doc: doc.clone(),
            root,
            role_map,
            class_map,
            children: Vec::new(),
            page: None,
            pages: Vec::new(),
            page_dict: None,
        };
        // If we have a specific page then we will work backwards from its
        // ParentTree - this is because structure elements could span
        // multiple pages, and the "Pg" attribute is *optional*, so this is
        // the approved way to get a page's structure...
        match scope {
            Scope::Page(page) => {
                tree.page = Some(page.clone());
                tree.pages = vec![(page.page_number.clone(), page.clone())];
                tree.page_dict = None;
                // ...EXCEPT that the ParentTree is sometimes missing, in
                // which case we fall back to the non-approved way.
                let parent_tree_obj = py_get(&tree.root, "ParentTree", NONE)?;
                if matches!(parent_tree_obj, PDFObject::Null) {
                    tree._parse_struct_tree()?;
                } else {
                    let parent_tree = {
                        let _call = class_call()?;
                        NumberTree::new(&parent_tree_obj)?
                    };
                    // If there is no marked content in the structure tree
                    // for this page (which can happen even when there is a
                    // structure tree) then there is no `StructParents`.
                    // Note however that if there are XObjects in a page,
                    // *they* may have `StructParent` (not `StructParents`)
                    let attrs = &page.page_obj.attrs;
                    if !attrs.contains_key("StructParents") {
                        return Ok(tree);
                    }
                    let parent_id = attrs.get("StructParents").expect("checked");
                    // NumberTree should have a `get` method like it does in
                    // pdf.js...: next(array for num, array in
                    // parent_tree.values if num == parent_id), the values
                    // read where the generator expression is created
                    let values = parent_tree.values()?;
                    leaf(1)?;
                    let found = {
                        let _call = c_call(WHILE_CALLING)?;
                        let _entry = eval_entry()?;
                        let _genexpr = py_frame()?;
                        values.into_iter().find(|(num, _)| *num == parent_id).map(|(_, array)| array)
                    };
                    let Some(array) = found else {
                        return Err(pycompat::PyErr::new(pycompat::ExcKind::StopIteration, ""));
                    };
                    let parent_array = resolve(&array)?;
                    tree._parse_parent_tree(&parent_array)?;
                }
            }
            Scope::Document(pdf) => {
                tree.page = None;
                // Overhead of creating pages shouldn't be too bad we hope!
                let pages = pdf.pages()?;
                // {page.page_number: page for page in doc.pages}
                for page in &pages {
                    match tree.pages.iter_mut().find(|(n, _)| *n == page.page_number) {
                        Some(slot) => slot.1 = page.clone(),
                        None => tree.pages.push((page.page_number.clone(), page.clone())),
                    }
                }
                let mut page_dict = HashMap::new();
                for (number, page) in &tree.pages {
                    page_dict.insert(ObjId::new(page.page_obj.pageid.clone())?, number.clone());
                }
                tree.page_dict = Some(page_dict);
                tree._parse_struct_tree()?;
            }
        }
        Ok(tree)
    }

    // pdfplumber/structure.py:217 PDFStructTree._make_attributes
    pub fn _make_attributes(&self, obj: &PDFObject, revision: &PDFObject) -> PyResult<Obj> {
        let _frame = py_frame()?;
        let mut attr_obj_list: Vec<PDFObject> = Vec::new();
        for key in ["C", "A"] {
            if !py_in(obj, key)? {
                continue;
            }
            let attr_obj = resolve(&py_getitem(obj, key)?)?;
            // It could be a list of attribute objects (why?)
            match &attr_obj {
                PDFObject::List(l) => attr_obj_list.extend(l.to_vec()),
                _ => attr_obj_list.push(attr_obj),
            }
        }
        let mut attr_objs: Vec<PDFObject> = Vec::new();
        let mut prev_obj = NONE;
        for aref in attr_obj_list {
            // If we find a revision number, which might "follow the revision
            // object" (the spec is not clear about what this should look
            // like but it implies they are simply adjacent in a flat array),
            // then use it to decide whether to take the previous object...
            if is_int(&aref) {
                if aref == *revision && !matches!(prev_obj, PDFObject::Null) {
                    attr_objs.push(prev_obj.clone());
                }
                prev_obj = NONE;
            } else {
                if !matches!(prev_obj, PDFObject::Null) {
                    attr_objs.push(prev_obj.clone());
                }
                prev_obj = resolve(&aref)?;
            }
        }
        if !matches!(prev_obj, PDFObject::Null) {
            attr_objs.push(prev_obj);
        }
        // Now merge all the attribute objects in the collected to a single
        // set (again, the spec doesn't really explain this but does say that
        // attributes in /A supersede those in /C)
        let mut attr = Obj::new();
        for obj in attr_objs {
            let mut obj = obj;
            if let PDFObject::Literal(_) = obj {
                let key = decode_name(&obj)?;
                if !py_in(&self.class_map, &key)? {
                    LOG.warning()?;
                    continue;
                }
                obj = py_getitem(&self.class_map, &key)?;
            }
            // for k, v in obj.items()
            let PDFObject::Dict(d) = &obj else {
                return Err(no_attribute(&obj, "items"));
            };
            let mut i = 0;
            while let Some((k, v)) = d.get_index(i) {
                if let PDFObject::Literal(_) = v {
                    attr.set(&k, PyValue::Str(PyStr::from(decode_name(&v)?)));
                } else {
                    attr.set(&k, value(v));
                }
                i += 1;
            }
        }
        Ok(attr)
    }

    // pdfplumber/structure.py:266 PDFStructTree._make_element
    pub fn _make_element(&self, obj: &PDFObject) -> PyResult<(Rc<PDFStructElement>, PDFObject)> {
        let _frame = py_frame()?;
        // We hopefully caught these earlier
        if py_in(obj, "MCID")? {
            return Err(exc::AssertionError(percent_s("Uncaught MCR: ", obj)?));
        }
        if py_in(obj, "Obj")? {
            return Err(exc::AssertionError(percent_s("Uncaught OBJR: ", obj)?));
        }
        // Get page number if necessary
        let mut page_number = None;
        if let Some(page_dict) = &self.page_dict {
            if py_in(obj, "Pg")? {
                let page_objid = objid_attr(&py_getitem(obj, "Pg")?)?;
                let id = ObjId::new(page_objid)?;
                match page_dict.get(&id) {
                    Some(n) => page_number = Some(n.clone()),
                    None => return Err(exc::AssertionError(percent_s("Object on unparsed page: ", obj)?)),
                }
            }
        }
        let mut obj_tag = String::new();
        if py_in(obj, "S")? {
            obj_tag = decode_name(&py_getitem(obj, "S")?)?;
            if py_in(&self.role_map, &obj_tag)? {
                obj_tag = decode_name(&py_getitem(&self.role_map, &obj_tag)?)?;
            }
        }
        let mut children =
            if py_in(obj, "K")? { resolve(&py_getitem(obj, "K")?)? } else { PDFObject::List(PDFList::default()) };
        if is_int(&children) {
            // ugh... isinstance...
            children = PDFObject::List(PDFList::new(vec![children]));
        } else if let PDFObject::Dict(_) = children {
            // a single object.. ugh...
            children = PDFObject::List(PDFList::new(vec![py_getitem(obj, "K")?]));
        }
        let revision = py_get(obj, "R", NONE)?;
        let attributes = self._make_attributes(obj, &revision)?;
        let text = |key: &str| -> PyResult<Option<PyStr>> {
            if py_in(obj, key)? {
                Ok(Some(decode_text_value(&value(resolve(&py_getitem(obj, key)?)?))?))
            } else {
                Ok(None)
            }
        };
        let element_id = text("ID")?;
        let title = text("T")?;
        let lang = text("Lang")?;
        let alt_text = text("Alt")?;
        let actual_text = text("ActualText")?;
        // PDFStructElement(...): the dataclass's __init__ assigns and calls
        // the default factories (dict, list: C)
        class_call_leaf()?;
        let element = PDFStructElement {
            r#type: PyStr::from(obj_tag),
            revision: value(revision),
            id: element_id,
            lang,
            alt_text,
            actual_text,
            title,
            page_number,
            attributes,
            mcids: RefCell::new(Vec::new()),
            children: RefCell::new(Vec::new()),
        };
        Ok((Rc::new(element), children))
    }

    // pdfplumber/structure.py:308 PDFStructTree._parse_parent_tree
    /// Populate the structure tree using the leaves of the parent tree for
    /// a given page.
    fn _parse_parent_tree(&mut self, parent_array: &PDFObject) -> PyResult<()> {
        let _frame = py_frame()?;
        // First walk backwards from the leaves to the root, tracking references
        let mut d: VecDeque<PDFObject> = py_iter(parent_array)?.into();
        let mut s: Seen = HashMap::new();
        let mut found_root = false;
        while let Some(r) = d.pop_front() {
            // In the case where an MCID is not associated with any
            // structure, there will be a "null" in the parent tree.
            if matches!(r, PDFObject::Keyword(k) if k == *KEYWORD_NULL) {
                continue;
            }
            if s.contains_key(&builtin_repr(&r)?) {
                continue;
            }
            let obj = resolve(&r)?;
            // This is required! It's in the spec!
            let is_root = py_in(&obj, "Type")? && decode_name(&py_getitem(&obj, "Type")?)? == "StructTreeRoot";
            if is_root {
                found_root = true;
            } else {
                // We hope that these are actual elements and not references
                // or marked-content sections...
                let (element, children) = self._make_element(&obj)?;
                // We have no page tree so we assume this page was parsed
                s.insert(builtin_repr(&r)?, (element, children));
                d.push_back(py_getitem(&obj, "P")?);
            }
        }
        // If we didn't reach the root something is quite wrong!
        if !found_root {
            return Err(exc::AssertionError(""));
        }
        self._resolve_children(&s)
    }

    // pdfplumber/structure.py:339 PDFStructTree.on_parsed_page
    /// Whether a dict (an element, an MCR, an OBJR) is on a parsed page:
    /// its `/Pg`'s object id among the document's pages, or the page's.
    pub fn on_parsed_page(&self, obj: &PDFObject) -> PyResult<bool> {
        let _frame = py_frame()?;
        if !py_in(obj, "Pg")? {
            return Ok(true);
        }
        let page_objid = objid_attr(&py_getitem(obj, "Pg")?)?;
        if let Some(page_dict) = &self.page_dict {
            return Ok(page_dict.contains_key(&ObjId::new(page_objid)?));
        }
        if let Some(page) = &self.page {
            // We have to do this to satisfy mypy
            if page_objid != page.page_obj.pageid {
                return Ok(false);
            }
        }
        Ok(true)
    }

    // pdfplumber/structure.py:351 PDFStructTree._parse_struct_tree
    /// Populate the structure tree starting from the root, skipping
    /// unparsed pages and empty elements.
    fn _parse_struct_tree(&mut self) -> PyResult<()> {
        let _frame = py_frame()?;
        let mut root = resolve(&py_getitem(&self.root, "K")?)?;
        // It could just be a single object ... it's in the spec (argh)
        if let PDFObject::Dict(_) = root {
            root = PDFObject::List(PDFList::new(vec![py_getitem(&self.root, "K")?]));
        }
        let mut d: VecDeque<PDFObject> = py_iter(&root)?.into();
        let mut s: Seen = HashMap::new();
        while let Some(r) = d.pop_front() {
            let mut r = r;
            // In case the tree is actually a DAG and not a tree...
            if s.contains_key(&builtin_repr(&r)?) {
                continue;
            }
            let mut obj = resolve(&r)?;
            // Deref top-level OBJR skipping refs to unparsed pages
            if matches!(obj, PDFObject::Dict(_)) && py_in(&obj, "Obj")? {
                if !self.on_parsed_page(&obj)? {
                    continue;
                }
                r = py_getitem(&obj, "Obj")?;
                obj = resolve(&r)?;
            }
            let (element, children) = self._make_element(&obj)?;
            // Similar to above, delay resolving the children to avoid
            // tree-recursion.
            s.insert(builtin_repr(&r)?, (element, children.clone()));
            for child in py_iter(&children)? {
                let mut child = child;
                let obj = resolve(&child)?;
                if let PDFObject::Dict(_) = obj {
                    if !self.on_parsed_page(&obj)? {
                        continue;
                    }
                    if py_in(&obj, "Obj")? {
                        child = py_getitem(&obj, "Obj")?;
                    } else if py_in(&obj, "MCID")? {
                        continue;
                    }
                }
                if let PDFObject::Ref(_) = child {
                    d.push_back(child);
                }
            }
        }
        // Traverse depth-first, removing empty elements (unsure how to do
        // this non-recursively)
        self.prune(&mut s, &root)?;
        self._resolve_children(&s)
    }

    // pdfplumber/structure.py:391 PDFStructTree._parse_struct_tree.prune
    /// The nested `prune(elements)`: the elements with children (MCIDs, or
    /// elements that have some), recursively; `s` keeps their pruned
    /// children and loses the others.
    fn prune(&self, s: &mut Seen, elements: &PDFObject) -> PyResult<PDFObject> {
        let _frame = py_frame()?;
        let mut next_elements = Vec::new();
        for r in py_iter(elements)? {
            let mut r = r;
            let obj = resolve(&r)?;
            if is_int(&r) {
                next_elements.push(r);
                continue;
            } else if let PDFObject::Dict(_) = obj {
                if !self.on_parsed_page(&obj)? {
                    continue;
                }
                if py_in(&obj, "MCID")? {
                    next_elements.push(py_getitem(&obj, "MCID")?);
                    continue;
                } else if py_in(&obj, "Obj")? {
                    r = py_getitem(&obj, "Obj")?;
                }
            }
            let key = builtin_repr(&r)?;
            let (element, children) = s.get(&key).cloned().ok_or_else(|| exc::KeyError(str_repr(&key)))?;
            let children = self.prune(s, &children)?;
            // See assertions below
            let key = builtin_repr(&r)?;
            let empty = matches!(&children, PDFObject::List(l) if l.is_empty());
            if empty {
                if s.remove(&key).is_none() {
                    return Err(exc::KeyError(str_repr(&key)));
                }
            } else {
                s.insert(key, (element, children));
                next_elements.push(r);
            }
        }
        Ok(PDFObject::List(PDFList::new(next_elements)))
    }

    // pdfplumber/structure.py:419 PDFStructTree._resolve_children
    /// Resolve children starting from the tree root based on references we
    /// saw when traversing the structure tree.
    ///
    /// A cycle among the elements reached makes Python loop for ever (each
    /// pass appends to the elements' lists): once every element the walk
    /// reaches has been processed once (the processing of an element
    /// depends on nothing that changes, so later passes raise nothing new)
    /// and the elements still queued reach a cycle, the port raises
    /// `MemoryError`, where Python would run out of memory.
    fn _resolve_children(&mut self, seen: &Seen) -> PyResult<()> {
        let _frame = py_frame()?;
        let mut root = resolve(&py_getitem(&self.root, "K")?)?;
        // It could just be a single object ... it's in the spec (argh)
        if let PDFObject::Dict(_) = root {
            root = PDFObject::List(PDFList::new(vec![py_getitem(&self.root, "K")?]));
        }
        self.children = Vec::new();
        // Create top-level self.children
        let mut parsed_root = Vec::new();
        for r in py_iter(&root)? {
            let mut r = r;
            let obj = resolve(&r)?;
            if matches!(obj, PDFObject::Dict(_)) && py_in(&obj, "Obj")? {
                if !self.on_parsed_page(&obj)? {
                    continue;
                }
                r = py_getitem(&obj, "Obj")?;
            }
            if seen.contains_key(&builtin_repr(&r)?) {
                parsed_root.push(r);
            }
        }
        let mut d: VecDeque<PDFObject> = parsed_root.iter().cloned().collect();
        // The cycle check: the keys each processed element queued, and the
        // keys queued but not processed yet; `None` once decided.
        let mut edges: HashMap<String, Vec<String>> = HashMap::new();
        let mut unprocessed: HashSet<String> = HashSet::new();
        let mut checking = true;
        while let Some(r) = d.pop_front() {
            let key = builtin_repr(&r)?;
            let (element, children) = seen.get(&key).cloned().ok_or_else(|| exc::KeyError(str_repr(&key)))?;
            // assert element is not None, "Unparsed element": never None here
            let first_time = checking && !edges.contains_key(&key);
            let mut queued = Vec::new();
            for child in py_iter(&children)? {
                let mut child = child;
                let obj = resolve(&child)?;
                if is_int(&obj) {
                    element.mcids.borrow_mut().push(value(obj));
                } else if let PDFObject::Dict(_) = obj {
                    // Skip out-of-page MCIDS and OBJRs
                    if !self.on_parsed_page(&obj)? {
                        continue;
                    }
                    if py_in(&obj, "MCID")? {
                        element.mcids.borrow_mut().push(value(py_getitem(&obj, "MCID")?));
                    } else if py_in(&obj, "Obj")? {
                        child = py_getitem(&obj, "Obj")?;
                    }
                }
                // NOTE: if, not elif, in case of OBJR above
                if let PDFObject::Ref(_) = child {
                    let child_key = builtin_repr(&child)?;
                    if let Some((child_element, _)) = seen.get(&child_key) {
                        element.children.borrow_mut().push(child_element.clone());
                        d.push_back(child);
                        if first_time {
                            queued.push(child_key);
                        }
                    }
                }
            }
            if first_time {
                unprocessed.remove(&key);
                for k in &queued {
                    if !edges.contains_key(k) && *k != key {
                        unprocessed.insert(k.clone());
                    }
                }
                edges.insert(key, queued);
                if unprocessed.is_empty() {
                    // every element the walk reaches has been processed
                    checking = false;
                    let starts: Vec<String> = parsed_root.iter().map(|r| r.repr()).collect::<PyResult<_>>()?;
                    if reaches_cycle(&edges, &starts) {
                        return Err(exc::MemoryError(""));
                    }
                }
            }
        }
        self.children = parsed_root
            .iter()
            .map(|r| {
                let key = builtin_repr(r)?;
                seen.get(&key).map(|(e, _)| e.clone()).ok_or_else(|| exc::KeyError(str_repr(&key)))
            })
            .collect::<PyResult<_>>()?;
        Ok(())
    }

    /// `iter(tree)`: its top-level elements.
    pub fn iter(&self) -> Vec<Rc<PDFStructElement>> {
        self.children.clone()
    }

    // pdfplumber/structure.py:466 PDFStructTree.element_bbox
    /// Get the bounding box for an element for visual debugging: its
    /// `/BBox` attribute on its page (cropped by a `CroppedPage`'s
    /// `_crop_fn`: `IndexError("Element no longer on page")`), else the bbox
    /// of the page objects with its MCIDs (`IndexError("No objects
    /// found")`).
    pub fn element_bbox(&self, el: &PDFStructElement) -> PyResult<Bbox> {
        let _frame = py_frame()?;
        let mut page = None;
        if let Some(p) = &self.page {
            page = Some(p.clone());
        } else if let Some(n) = &el.page_number {
            page = Some(self.page_of(n)?);
        }
        let bbox = el.attributes.get("BBox").cloned().unwrap_or(PyValue::None);
        if let (Some(page), false) = (&page, bbox.is_none()) {
            // Use secret knowledge of CroppedPage (cannot use page.height
            // because it is the *cropped* dimension, but cropping does not
            // actually translate coordinates)
            let box_raw = pdf_of(&bbox);
            let normalized = _normalize_box(&box_raw, &PDFObject::Int(PyInt::ZERO))?;
            let mediabox = page.mediabox.as_seq().expect("a tuple of four numbers").to_vec();
            let mb_height = mediabox[3].sub(&mediabox[1])?;
            let mb_height = mb_height.as_num().expect("a number");
            let inverted = _invert_box(&normalized, &mb_height)?;
            let bbox: Bbox = inverted.map(PyValue::from_pdf);
            // Use more secret knowledge of CroppedPage
            if let PageKind::Cropped { crop_bbox, crop_fn, .. } = &page.kind {
                let rect = geometry::bbox_to_rect(&bbox);
                let rects = apply_crop_fn(*crop_fn, crop_bbox, &[rect])?;
                let Some(first) = rects.first() else {
                    return Err(exc::IndexError("Element no longer on page"));
                };
                return geometry::obj_to_bbox(first);
            }
            // Not sure why mypy complains here
            return Ok(bbox);
        }
        let mut mcid_objs: Vec<Obj> = Vec::new();
        for (page_number, mcid) in el.all_mcids()? {
            let objects = match &page_number {
                None => match &page {
                    Some(p) => Some(p.objects()?),
                    None => None,
                },
                Some(n) => Some(self.page_of(n)?.objects()?),
            };
            let Some(objects) = objects else { continue };
            // itertools.chain.from_iterable(objects.values())
            for (_, objs) in objects.iter() {
                for c in objs {
                    if c.item("mcid")?.py_eq(&mcid) {
                        mcid_objs.push(c.clone());
                    }
                }
            }
        }
        if mcid_objs.is_empty() {
            return Err(exc::IndexError("No objects found"));
        }
        geometry::objects_to_bbox(&mcid_objs)
    }

    /// `self.pages[page_number]`
    fn page_of(&self, n: &PyInt) -> PyResult<Rc<Page>> {
        self.pages.iter().find(|(k, _)| k == n).map(|(_, p)| p.clone()).ok_or_else(|| exc::KeyError(n.to_string()))
    }

    // pdfplumber/structure.py:75 Findable.find_all
    /// Iterate depth-first over matching elements in subtree.
    pub fn find_all<'a>(&self, matcher: Matcher<'a>) -> PyResult<FindAll<'a>> {
        find_all(&self.children, matcher)
    }

    // pdfplumber/structure.py:86 Findable.find
    /// Find the first matching element in subtree.
    pub fn find(&self, matcher: Matcher<'_>) -> PyResult<Option<Rc<PDFStructElement>>> {
        find(&self.children, matcher)
    }
}

/// A value of an element's attributes as the PDF object it is.
fn pdf_of(v: &PyValue) -> PDFObject {
    match v {
        PyValue::None => NONE,
        PyValue::Bool(b) => PDFObject::Bool(*b),
        PyValue::Int(i) => PDFObject::Int(i.clone()),
        PyValue::Float(f) => PDFObject::Float(f.get()),
        PyValue::Bytes(b) => PDFObject::Bytes(b.clone()),
        PyValue::Str(s) => PDFObject::Str(s.clone()),
        PyValue::Pdf(o) => o.clone(),
        other => unreachable!("not a value of the document: {other:?}"),
    }
}

/// Whether the graph `edges` (an element's key -> the keys it queues)
/// has a cycle reachable from `starts` (iterative depth-first search).
fn reaches_cycle(edges: &HashMap<String, Vec<String>>, starts: &[String]) -> bool {
    // 0: unseen, 1: on the current path, 2: done
    let mut state: HashMap<&str, u8> = HashMap::new();
    for start in starts {
        if state.contains_key(start.as_str()) {
            continue;
        }
        let mut stack: Vec<(&str, usize)> = vec![(start.as_str(), 0)];
        state.insert(start.as_str(), 1);
        while let Some((node, i)) = stack.pop() {
            let next = edges.get(node).and_then(|e| e.get(i));
            match next {
                Some(child) => {
                    stack.push((node, i + 1));
                    match state.get(child.as_str()) {
                        Some(1) => return true,
                        Some(_) => {}
                        None => {
                            state.insert(child.as_str(), 1);
                            stack.push((child.as_str(), 0));
                        }
                    }
                }
                None => {
                    state.insert(node, 2);
                }
            }
        }
    }
    false
}
