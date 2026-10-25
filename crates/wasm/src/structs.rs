//! pdfplumber's structure trees, pdfplumber/structure.py:151 PDFStructTree
//! as JavaScript reaches them (`docs/js-runtime.md`,
//! "Documents et pages"; the `PDFStructTree` of `docs/public-api.md`).
//!
//! `struct_tree` (on a document, with a page handle or `None`) makes the
//! tree, `PDFStructTree(pdf, page)`, behind a handle, and returns its
//! elements as values; the other operations act on the tree, or on one of
//! its elements named by its *path* (the indices from `tree.children`
//! down: `[0, 2]` is `tree.children[0].children[2]`):
//!
//! | operation | statement (`oracle/unit/js_pages_run.py`) | result |
//! |---|---|---|
//! | `struct_tree [page]` | `PDFStructTree(obj, page)` | `{handle, children}` |
//! | `struct_find_all [path, matcher]` | `list(x.find_all(m))` | paths |
//! | `struct_find [path, matcher]` | `x.find(m)` | a path or `None` |
//! | `struct_element_bbox [path]` | `obj.element_bbox(el)` | a bbox |
//! | `struct_all_mcids [path]` | `list(el.all_mcids())` | `(page_number, mcid)` tuples |
//! | `struct_element_repr [path]` | `repr(el)` | a `str` |
//!
//! (`x`: the tree when `path` is `None`, else the element.) A matcher is
//! an element type (a `str`: `match_tag`), or `{"mask": [truth...]}`, the
//! truth of a function JavaScript ran on every element of the tree, in the
//! tree's depth-first order (`_find_all`'s), looked up by identity by a
//! function of one frame (as a Python predicate, or `match_regex` for a
//! `RegExp`, whose JavaScript semantics decide the truth); any other value
//! is called as Python calls it (`TypeError: '...' object is not
//! callable`, raised at the first element).

use std::rc::Rc;

use pdfplumber::obj::{bbox_value, Obj, PyValue};
use pdfplumber::structure::{Matcher, PDFStructElement, PDFStructTree, Scope};
use pycompat::recursion::{c_call, eval_entry, leaf};
use pycompat::{exc, ExcKind, PyErr, PyInt, PyResult};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::ops::{arg, handle, int, seq, Core, Object, Out};

const WHILE_CALLING: &str = " while calling a Python object";

/// The function of a `Matcher::Func`.
type MatchFn<'a> = dyn Fn(&PDFStructElement) -> PyResult<bool> + 'a;

/// A structure tree handle.
pub struct TreeObj {
    pub doc: u32,
    /// `None` between a reset and the replay that refills it.
    pub tree: Option<Rc<PDFStructTree>>,
}

/// The elements of a tree in depth-first order (that of `_find_all`, from
/// `tree.children`; an element reached twice, in a tree that is a DAG, is
/// listed twice).
fn preorder(tree: &PDFStructTree) -> PyResult<Vec<Rc<PDFStructElement>>> {
    let mut out = Vec::new();
    let mut stack: Vec<Rc<PDFStructElement>> = tree.children.iter().rev().cloned().collect();
    while let Some(el) = stack.pop() {
        if out.len() > MAX_ELEMENTS {
            return Err(exc::MemoryError("a structure tree whose walk does not end"));
        }
        stack.extend(el.children.borrow().iter().rev().cloned());
        out.push(el);
    }
    Ok(out)
}

/// More elements than any document can hold: the walk of a tree whose
/// children hold an ancestor never ends (pdfplumber's own walks would
/// loop, or raise `RecursionError`, first), and a DAG whose walk reaches
/// this many elements took pdfplumber as many steps to build
/// (pdfplumber/structure.py:419 `_resolve_children` queues a child once
/// per parent that lists it).
const MAX_ELEMENTS: usize = 1 << 24;

/// The elements of a tree as values: each `PDFStructElement`'s fields in
/// order (pdfplumber/structure.py:102 PDFStructElement, a dataclass), `children` the values
/// of its children (built without recursion: any depth).
fn elements_value(children: &[Rc<PDFStructElement>]) -> PyResult<PyValue> {
    struct Frame {
        el: Rc<PDFStructElement>,
        children: Vec<Rc<PDFStructElement>>,
        done: Vec<PyValue>,
    }
    fn fields(el: &PDFStructElement, children: Vec<PyValue>) -> PyValue {
        let opt = |s: &Option<pycompat::PyStr>| s.clone().map_or(PyValue::None, PyValue::Str);
        PyValue::Dict(Obj::from_pairs([
            ("type", PyValue::Str(el.r#type.clone())),
            ("revision", el.revision.clone()),
            ("id", opt(&el.id)),
            ("lang", opt(&el.lang)),
            ("alt_text", opt(&el.alt_text)),
            ("actual_text", opt(&el.actual_text)),
            ("title", opt(&el.title)),
            ("page_number", el.page_number.clone().map_or(PyValue::None, PyValue::Int)),
            ("attributes", PyValue::Dict(el.attributes.clone())),
            ("mcids", PyValue::list(el.mcids.borrow().iter().cloned())),
            ("children", PyValue::list(children)),
        ]))
    }
    let open = |el: &Rc<PDFStructElement>| Frame {
        el: el.clone(),
        children: el.children.borrow().iter().rev().cloned().collect(),
        done: Vec::new(),
    };
    let mut top = Vec::with_capacity(children.len());
    // the elements being written (an element holding an ancestor would
    // never end)
    let mut active: FxHashSet<usize> = FxHashSet::default();
    for root in children {
        let mut stack = vec![open(root)];
        active.insert(Rc::as_ptr(root) as usize);
        while let Some(frame) = stack.last_mut() {
            match frame.children.pop() {
                Some(child) => {
                    if !active.insert(Rc::as_ptr(&child) as usize) {
                        return Err(exc::ValueError("Circular reference detected"));
                    }
                    stack.push(open(&child));
                }
                None => {
                    let frame = stack.pop().expect("a frame");
                    active.remove(&(Rc::as_ptr(&frame.el) as usize));
                    let value = fields(&frame.el, frame.done);
                    match stack.last_mut() {
                        Some(parent) => parent.done.push(value),
                        None => top.push(value),
                    }
                }
            }
        }
    }
    Ok(PyValue::list(top))
}

/// The path of each element of a tree (its first one, in a DAG), by
/// identity.
fn paths(tree: &PDFStructTree) -> PyResult<FxHashMap<usize, PyValue>> {
    let mut out = FxHashMap::default();
    let mut stack: Vec<(Vec<usize>, Rc<PDFStructElement>)> =
        tree.children.iter().enumerate().rev().map(|(i, el)| (vec![i], el.clone())).collect();
    // (the elements visited, as preorder counts them: in a DAG, the
    // distinct elements stay few while the walk grows)
    let mut visited = 0_usize;
    while let Some((path, el)) = stack.pop() {
        visited += 1;
        if visited > MAX_ELEMENTS {
            return Err(exc::MemoryError("a structure tree whose walk does not end"));
        }
        for (j, child) in el.children.borrow().iter().enumerate().rev() {
            let mut p = path.clone();
            p.push(j);
            stack.push((p, child.clone()));
        }
        out.entry(Rc::as_ptr(&el) as usize)
            .or_insert_with(|| PyValue::list(path.iter().map(|&i| PyValue::Int(PyInt::from(i as i64)))));
    }
    Ok(out)
}

/// The element of a path (`IndexError` as Python's indexing raises it).
fn element(tree: &PDFStructTree, path: &PyValue) -> PyResult<Rc<PDFStructElement>> {
    let mut children = tree.children.clone();
    let mut found = None;
    for i in seq(path)? {
        let i = usize::try_from(int(i)?).map_err(|_| exc::IndexError("list index out of range"))?;
        let el = children.get(i).cloned().ok_or_else(|| exc::IndexError("list index out of range"))?;
        children = el.children.borrow().clone();
        found = Some(el);
    }
    found.ok_or_else(|| exc::ValueError("an element's path is not empty"))
}

impl Core {
    /// The tree of the handle `h` (the operation reaches its document).
    fn tree(&mut self, h: u32) -> PyResult<Rc<PDFStructTree>> {
        let (doc, tree) = match self.handles.get(h) {
            Some(Object::StructTree(t)) => (t.doc, t.tree.clone()),
            _ => return Err(exc::ValueError("invalid handle")),
        };
        self.reach(doc)?;
        tree.ok_or_else(|| PyErr::new(ExcKind::RuntimeError, "the tree was not rebuilt"))
    }

    /// The operations of structure trees (see the module documentation);
    /// `None` for another name.
    pub(crate) fn struct_op(&mut self, name: &str, h: u32, args: &PyValue) -> PyResult<Option<Out>> {
        let a = || seq(args);
        let value = match name {
            "struct_tree" => {
                // [page handle or None] on the document: PDFStructTree(obj, page)
                let pdf = self.pdf(h)?;
                let page = match arg(a()?, 0)? {
                    PyValue::None => None,
                    p => {
                        let p = handle(p)?;
                        match self.handles.get(p) {
                            Some(Object::Page(x)) if x.doc == h => {}
                            // Python takes it (the tree of one document, the
                            // page of another); an operation reaches one
                            // document (crate::ops, "Void calls")
                            Some(Object::Page(_)) => {
                                return Err(PyErr::new(
                                    ExcKind::NotPorted,
                                    "not modelled: PDFStructTree(pdf, page) of a page of another document",
                                ))
                            }
                            _ => return Err(exc::ValueError("invalid handle")),
                        }
                        Some(self.page(p)?)
                    }
                };
                let tree = match &page {
                    None => PDFStructTree::new(&pdf.core.doc, Scope::Document(&pdf))?,
                    Some(p) => PDFStructTree::new(&pdf.core.doc, Scope::Page(p))?,
                };
                let children = elements_value(&tree.children)?;
                let handle = self.insert(Object::StructTree(TreeObj { doc: h, tree: Some(Rc::new(tree)) }))?;
                PyValue::Dict(Obj::from_pairs([("handle", handle), ("children", children)]))
            }
            "struct_find_all" | "struct_find" => {
                // [path or None, matcher]
                let a = a()?;
                let tree = self.tree(h)?;
                let (path, m) = (arg(a, 0)?, arg(a, 1)?);
                let el = if path.is_none() { None } else { Some(element(&tree, path)?) };
                let mask_index;
                let mask_truth;
                let not_callable;
                let mask_fn: Box<MatchFn<'_>>;
                let matcher = match m {
                    PyValue::Str(tag) => Matcher::Tag(tag),
                    PyValue::Dict(d) if d.len() == 1 && d.get("mask").is_some() => {
                        let truth = seq(d.get("mask").expect("checked"))?;
                        let elements = preorder(&tree)?;
                        if truth.len() != elements.len() {
                            return Err(exc::ValueError(format!(
                                "the mask has {} values for {} elements",
                                truth.len(),
                                elements.len()
                            )));
                        }
                        let mut index = FxHashMap::default();
                        for (i, el) in elements.iter().enumerate() {
                            index.entry(Rc::as_ptr(el) as usize).or_insert(i);
                        }
                        mask_index = index;
                        mask_truth = truth.iter().map(PyValue::truthy).collect::<Vec<_>>();
                        // the predicate (or match_regex): a function's frame
                        mask_fn = Box::new(|el: &PDFStructElement| {
                            leaf(1)?;
                            let i = mask_index.get(&(std::ptr::from_ref(el) as usize)).ok_or_else(|| {
                                PyErr::new(ExcKind::NotPorted, "not modelled: an element the mask does not have")
                            })?;
                            Ok(mask_truth[*i])
                        });
                        Matcher::Func(&*mask_fn)
                    }
                    other => {
                        // match_func = matcher; match_func(el)
                        not_callable = format!("'{}' object is not callable", other.type_name());
                        mask_fn = Box::new(|_: &PDFStructElement| Err(exc::TypeError(not_callable.clone())));
                        Matcher::Func(&*mask_fn)
                    }
                };
                let found = if name == "struct_find" {
                    let found = match &el {
                        None => tree.find(matcher)?,
                        Some(el) => el.find(matcher)?,
                    };
                    found.into_iter().collect::<Vec<_>>()
                } else {
                    // list(x.find_all(m)): the generator made, then resumed
                    // by `list` (a builtin, from C) until it ends
                    let it = match &el {
                        None => tree.find_all(matcher)?,
                        Some(el) => el.find_all(matcher)?,
                    };
                    let _call = c_call(WHILE_CALLING)?;
                    let mut out = Vec::new();
                    for x in it {
                        let _entry = eval_entry()?;
                        out.push(x?);
                    }
                    out
                };
                let paths = paths(&tree)?;
                let path_of = |el: &Rc<PDFStructElement>| {
                    paths
                        .get(&(Rc::as_ptr(el) as usize))
                        .cloned()
                        .ok_or_else(|| PyErr::new(ExcKind::RuntimeError, "an element found outside its tree"))
                };
                if name == "struct_find" {
                    match found.first() {
                        None => PyValue::None,
                        Some(el) => path_of(el)?,
                    }
                } else {
                    PyValue::list(found.iter().map(path_of).collect::<PyResult<Vec<_>>>()?)
                }
            }
            "struct_element_bbox" => {
                // tree.element_bbox(el)
                let tree = self.tree(h)?;
                let el = element(&tree, arg(a()?, 0)?)?;
                bbox_value(&tree.element_bbox(&el)?)
            }
            "struct_all_mcids" => {
                // list(el.all_mcids()): a generator resumed by `list`
                let tree = self.tree(h)?;
                let el = element(&tree, arg(a()?, 0)?)?;
                let _call = c_call(WHILE_CALLING)?;
                let _entry = eval_entry()?;
                let mcids = el.all_mcids()?;
                PyValue::list(
                    mcids.into_iter().map(|(n, mcid)| PyValue::tuple([n.map_or(PyValue::None, PyValue::Int), mcid])),
                )
            }
            "struct_element_repr" => {
                // repr(el): the builtin's call, then PyObject_Repr
                let tree = self.tree(h)?;
                let el = element(&tree, arg(a()?, 0)?)?;
                let _call = c_call(WHILE_CALLING)?;
                PyValue::Str(el.repr()?)
            }
            _ => return Ok(None),
        };
        Ok(Some(Out::Value(value)))
    }
}
