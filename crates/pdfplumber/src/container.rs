//! Port of `pdfplumber/container.py`: `Container`, the base of `PDF` and
//! `Page` (object lists by kind, edges, JSON and CSV output).
//!
//! Wave 4 (base): the interface, bodies `todo!()` (workstream PAGE,
//! `docs/architecture.md`, "Vague 4"). [`Objects`] is complete.
//!
//! # Python classes in Rust
//!
//! `Container` is a trait: `PDF` (`crate::pdf::PDF`) and `Page`
//! (`crate::page::Page`, every kind of page) implement its required
//! methods (the abstract properties `pages`, `objects`, `to_dict`, the
//! cache and `getattr`), and the provided methods are `Container`'s own
//! bodies, which reach the implementor's overrides through `self`, as
//! Python's `self.objects` does.
//!
//! # Cached properties
//!
//! `cached_properties` (pdfplumber/container.py:12 `Container`:
//! `["_rect_edges", "_curve_edges", "_edges", "_objects"]`, plus `_pages`
//! for `PDF`, `_layout` for `Page`) are attributes set on first use and
//! deleted by `flush_cache`; an implementor keeps them in a
//! [`ContainerCache`] plus its own fields. A value is shared once computed
//! (`Rc`): `page.edges` is the same list each time, and the objects of
//! `page.objects` are the same dicts that `pdf.objects` lists.
//!
//! # Recursion
//!
//! Each property and method is a Python frame (`py_frame`); the lists are
//! built by C (`list(chain(*gen))`: a generator expression resumed from C,
//! `eval_entry` and a frame per resumption; `list(map(utils.line_to_edge,
//! self.lines))`, `filter(test, ...)`: each call from C). None of it
//! recurses: the frames only matter when a container is used near the
//! limit, which the `plumber_*` stages never do (they start from the stage
//! entry depth); they are counted anyway, as everywhere in the port, so
//! that a page derived deep in a user's recursion fails where Python's
//! does. `to_json` recurses through `Serializer.serialize` (two frames per
//! level of nesting: `serialize`, `do_list`/`do_tuple`/`do_dict`, plus the
//! generator expression of `do_list`/`do_tuple`) and then through the C
//! encoder of `json.dumps` (`Py_EnterRecursiveCall` per container): a
//! value nested a few hundred levels deep (an `mcid` from a marked-content
//! property list, a `colorspace` array) raises `RecursionError` there.

use std::cell::RefCell;
use std::rc::Rc;

use pycompat::{PyResult, PyStr};

use crate::obj::{Obj, PyValue};
use crate::page::Page;

/// `Dict[str, T_obj_list]`: the objects of a container by kind
/// (`"char"`, `"rect"`...), kinds in the order they were first added (a
/// Python `dict`).
#[derive(Clone, Debug, Default)]
pub struct Objects {
    kinds: Vec<(PyStr, Vec<Obj>)>,
}

impl Objects {
    pub fn new() -> Objects {
        Objects::default()
    }

    /// `objects.get(kind)`
    pub fn get(&self, kind: &str) -> Option<&[Obj]> {
        self.kinds.iter().find(|(k, _)| k.as_str() == Some(kind)).map(|(_, v)| v.as_slice())
    }

    /// `objects.get(kind, [])`
    pub fn get_or_empty(&self, kind: &str) -> Vec<Obj> {
        self.get(kind).map(<[Obj]>::to_vec).unwrap_or_default()
    }

    /// `objects.keys()`
    pub fn keys(&self) -> impl Iterator<Item = &PyStr> {
        self.kinds.iter().map(|(k, _)| k)
    }

    /// `objects.items()`
    pub fn iter(&self) -> impl Iterator<Item = (&PyStr, &[Obj])> {
        self.kinds.iter().map(|(k, v)| (k, v.as_slice()))
    }

    pub fn len(&self) -> usize {
        self.kinds.len()
    }

    pub fn is_empty(&self) -> bool {
        self.kinds.is_empty()
    }

    /// `objects[kind] = objs` (a new kind goes last, an existing one keeps
    /// its place).
    pub fn insert(&mut self, kind: PyStr, objs: Vec<Obj>) {
        match self.kinds.iter_mut().find(|(k, _)| *k == kind) {
            Some((_, v)) => *v = objs,
            None => self.kinds.push((kind, objs)),
        }
    }

    /// `if objects.get(kind) is None: objects[kind] = []`, then
    /// `objects[kind].append(obj)`
    /// (pdfplumber/page.py:441 `Page.parse_objects`).
    pub fn push(&mut self, kind: PyStr, obj: Obj) {
        match self.kinds.iter_mut().find(|(k, _)| *k == kind) {
            Some((_, v)) => v.push(obj),
            None => self.kinds.push((kind, vec![obj])),
        }
    }
}

/// The cached properties of `Container` (`_rect_edges`, `_curve_edges`,
/// `_edges`; `_objects` is the implementor's, since `Page` and `PDF`
/// compute it differently).
#[derive(Default)]
pub struct ContainerCache {
    pub rect_edges: Option<Rc<Vec<Obj>>>,
    pub curve_edges: Option<Rc<Vec<Obj>>>,
    pub edges: Option<Rc<Vec<Obj>>>,
}

/// The options of `to_json` and `to_csv` (`stream` is not one: the port
/// returns the text, which is what `stream=None` does).
#[derive(Clone, Debug, Default)]
pub struct SerializeOptions {
    /// `object_types` (`None`: every kind of `objects`, and `"annot"`)
    pub object_types: Option<Vec<PyStr>>,
    pub include_attrs: Option<Vec<PyStr>>,
    pub exclude_attrs: Option<Vec<PyStr>>,
    /// `precision`, as given (`round(x, precision)`)
    pub precision: Option<PyValue>,
    /// `indent` of `json.dumps` (`to_json` only), as given
    pub indent: Option<PyValue>,
}

/// pdfplumber/container.py:12 `Container`
pub trait Container {
    // pdfplumber/container.py:16 Container.pages
    /// `self.pages`: `None` for a page (pdfplumber/page.py:186 `Page`:
    /// `pages = None`), the pages for a `PDF`.
    fn pages(&self) -> PyResult<Option<Vec<Rc<Page>>>>;

    // pdfplumber/container.py:20 Container.objects
    fn objects(&self) -> PyResult<Rc<Objects>>;

    // pdfplumber/container.py:23 Container.to_dict
    fn to_dict(&self, object_types: Option<&[PyStr]>) -> PyResult<Obj>;

    /// The cached properties of [`ContainerCache`].
    fn container_cache(&self) -> &RefCell<ContainerCache>;

    /// `getattr(self, name)` for the attribute names `to_dict` and `to_csv`
    /// build (`t + "s"`, any `t` a user gives): every attribute of the
    /// implementor's class, `AttributeError("'Page' object has no attribute
    /// 'xs'")` otherwise.
    fn attribute(&self, name: &PyStr) -> PyResult<PyValue>;

    // pdfplumber/container.py:28 Container.flush_cache
    /// Deletes the cached properties named in `properties` (`None`: the
    /// class's `cached_properties`) that are set.
    fn flush_cache(&self, properties: Option<&[&str]>);

    // pdfplumber/container.py:35 Container.rects
    fn rects(&self) -> PyResult<Vec<Obj>> {
        todo!("pdfplumber/container.py:35 Container.rects")
    }

    // pdfplumber/container.py:39 Container.lines
    fn lines(&self) -> PyResult<Vec<Obj>> {
        todo!("pdfplumber/container.py:39 Container.lines")
    }

    // pdfplumber/container.py:43 Container.curves
    fn curves(&self) -> PyResult<Vec<Obj>> {
        todo!("pdfplumber/container.py:43 Container.curves")
    }

    // pdfplumber/container.py:47 Container.images
    fn images(&self) -> PyResult<Vec<Obj>> {
        todo!("pdfplumber/container.py:47 Container.images")
    }

    // pdfplumber/container.py:51 Container.chars
    fn chars(&self) -> PyResult<Vec<Obj>> {
        todo!("pdfplumber/container.py:51 Container.chars")
    }

    // pdfplumber/container.py:55 Container.textboxverticals
    fn textboxverticals(&self) -> PyResult<Vec<Obj>> {
        todo!("pdfplumber/container.py:55 Container.textboxverticals")
    }

    // pdfplumber/container.py:59 Container.textboxhorizontals
    fn textboxhorizontals(&self) -> PyResult<Vec<Obj>> {
        todo!("pdfplumber/container.py:59 Container.textboxhorizontals")
    }

    // pdfplumber/container.py:63 Container.textlineverticals
    fn textlineverticals(&self) -> PyResult<Vec<Obj>> {
        todo!("pdfplumber/container.py:63 Container.textlineverticals")
    }

    // pdfplumber/container.py:67 Container.textlinehorizontals
    fn textlinehorizontals(&self) -> PyResult<Vec<Obj>> {
        todo!("pdfplumber/container.py:67 Container.textlinehorizontals")
    }

    // pdfplumber/container.py:71 Container.rect_edges
    /// `list(chain(*(utils.rect_to_edges(r) for r in self.rects)))`, cached.
    fn rect_edges(&self) -> PyResult<Rc<Vec<Obj>>> {
        todo!("pdfplumber/container.py:71 Container.rect_edges")
    }

    // pdfplumber/container.py:79 Container.curve_edges
    fn curve_edges(&self) -> PyResult<Rc<Vec<Obj>>> {
        todo!("pdfplumber/container.py:79 Container.curve_edges")
    }

    // pdfplumber/container.py:87 Container.edges
    /// The edges of the lines, then `rect_edges`, then `curve_edges`,
    /// cached.
    fn edges(&self) -> PyResult<Rc<Vec<Obj>>> {
        todo!("pdfplumber/container.py:87 Container.edges")
    }

    // pdfplumber/container.py:95 Container.horizontal_edges
    fn horizontal_edges(&self) -> PyResult<Vec<Obj>> {
        todo!("pdfplumber/container.py:95 Container.horizontal_edges")
    }

    // pdfplumber/container.py:102 Container.vertical_edges
    fn vertical_edges(&self) -> PyResult<Vec<Obj>> {
        todo!("pdfplumber/container.py:102 Container.vertical_edges")
    }

    // pdfplumber/container.py:108 Container.to_json
    /// `json.dumps(Serializer(...).serialize(self.to_dict(object_types)),
    /// indent=indent)`: CPython's encoder (`pycompat::json`, to write:
    /// `ensure_ascii`, `float.__repr__`, `NaN`/`Infinity`, the separators
    /// of `indent`, the `TypeError` of a key or value it cannot encode).
    fn to_json(&self, options: &SerializeOptions) -> PyResult<PyStr> {
        let _ = options;
        todo!("pdfplumber/container.py:108 Container.to_json")
    }

    // pdfplumber/container.py:132 Container.to_csv
    /// `csv.DictWriter(stream, fieldnames=cols, extrasaction="ignore",
    /// quoting=csv.QUOTE_MINIMAL, escapechar="\\")`, header then rows:
    /// CPython's `csv` writer (`pycompat::csv`, to write: `str()` of each
    /// value, quoting, `\r\n` line ends).
    fn to_csv(&self, options: &SerializeOptions) -> PyResult<PyStr> {
        let _ = options;
        todo!("pdfplumber/container.py:132 Container.to_csv")
    }
}
