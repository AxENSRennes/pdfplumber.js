//! Port of `pdfplumber/container.py`: `Container`, the base of `PDF` and
//! `Page` (object lists by kind, edges, JSON and CSV output).
//!
//! Wave 4, workstream PAGE (`docs/architecture.md`, "Vague 4").
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

use pycompat::json::JsonIndent;
use pycompat::recursion::{eval_entry, leaf, py_frame};
use pycompat::specialize::{AttrSite, CallSite};
use pycompat::{ExcKind, PyErr, PyResult, PyStr};

use crate::convert::{Serializer, CSV_COLS_REQUIRED, CSV_COLS_TO_PREPEND};
use crate::obj::{Obj, PyValue};
use crate::page::Page;
use crate::utils::geometry::{curve_to_edges, line_to_edge, rect_to_edges};

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

    /// `objects.get(kind)` for a `str` key.
    pub fn get_key(&self, kind: &PyStr) -> Option<&[Obj]> {
        self.kinds.iter().find(|(k, _)| k == kind).map(|(_, v)| v.as_slice())
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

    /// The Python class of the object (`type(self).__name__`): the class a
    /// property load specializes for (`pycompat::specialize::AttrSite`).
    fn class_name(&self) -> &'static str;

    /// `getattr(self, name)` for the attribute names `to_dict` and `to_csv`
    /// build (`t + "s"`, any `t` a user gives): every attribute of the
    /// implementor's class, `AttributeError("'Page' object has no attribute
    /// 'xs'")` otherwise.
    fn attribute(&self, name: &PyStr) -> PyResult<PyValue>;

    // pdfplumber/container.py:28 Container.flush_cache
    /// Deletes the cached properties named in `properties` (`None`: the
    /// class's `cached_properties`) that are set.
    fn flush_cache(&self, properties: Option<&[&str]>);

    /// `self.objects.get(kind, [])`, in the frame of the property (`site`:
    /// its load of `self.objects`).
    fn kind_list(&self, kind: &str, site: &AttrSite) -> PyResult<Vec<Obj>> {
        let _frame = py_frame()?;
        Ok(site.get(self.class_name(), || self.objects())?.get_or_empty(kind))
    }

    // pdfplumber/container.py:35 Container.rects
    fn rects(&self) -> PyResult<Vec<Obj>> {
        self.kind_list("rect", &RECTS_OBJECTS)
    }

    // pdfplumber/container.py:39 Container.lines
    fn lines(&self) -> PyResult<Vec<Obj>> {
        self.kind_list("line", &LINES_OBJECTS)
    }

    // pdfplumber/container.py:43 Container.curves
    fn curves(&self) -> PyResult<Vec<Obj>> {
        self.kind_list("curve", &CURVES_OBJECTS)
    }

    // pdfplumber/container.py:47 Container.images
    fn images(&self) -> PyResult<Vec<Obj>> {
        self.kind_list("image", &IMAGES_OBJECTS)
    }

    // pdfplumber/container.py:51 Container.chars
    fn chars(&self) -> PyResult<Vec<Obj>> {
        self.kind_list("char", &CHARS_OBJECTS)
    }

    // pdfplumber/container.py:55 Container.textboxverticals
    fn textboxverticals(&self) -> PyResult<Vec<Obj>> {
        self.kind_list("textboxvertical", &TEXTBOXVERTICALS_OBJECTS)
    }

    // pdfplumber/container.py:59 Container.textboxhorizontals
    fn textboxhorizontals(&self) -> PyResult<Vec<Obj>> {
        self.kind_list("textboxhorizontal", &TEXTBOXHORIZONTALS_OBJECTS)
    }

    // pdfplumber/container.py:63 Container.textlineverticals
    fn textlineverticals(&self) -> PyResult<Vec<Obj>> {
        self.kind_list("textlinevertical", &TEXTLINEVERTICALS_OBJECTS)
    }

    // pdfplumber/container.py:67 Container.textlinehorizontals
    fn textlinehorizontals(&self) -> PyResult<Vec<Obj>> {
        self.kind_list("textlinehorizontal", &TEXTLINEHORIZONTALS_OBJECTS)
    }

    // pdfplumber/container.py:71 Container.rect_edges
    /// `list(chain(*(utils.rect_to_edges(r) for r in self.rects)))`, cached.
    fn rect_edges(&self) -> PyResult<Rc<Vec<Obj>>> {
        let _frame = py_frame()?;
        if let Some(edges) = &self.container_cache().borrow().rect_edges {
            return Ok(edges.clone());
        }
        let rects = RECT_EDGES_RECTS.get(self.class_name(), || self.rects())?;
        let edges = Rc::new(chain_edges(&rects, rect_to_edges)?);
        self.container_cache().borrow_mut().rect_edges = Some(edges.clone());
        Ok(edges)
    }

    // pdfplumber/container.py:79 Container.curve_edges
    /// `list(chain(*(utils.curve_to_edges(r) for r in self.curves)))`,
    /// cached.
    fn curve_edges(&self) -> PyResult<Rc<Vec<Obj>>> {
        let _frame = py_frame()?;
        if let Some(edges) = &self.container_cache().borrow().curve_edges {
            return Ok(edges.clone());
        }
        let curves = CURVE_EDGES_CURVES.get(self.class_name(), || self.curves())?;
        let edges = Rc::new(chain_edges(&curves, curve_to_edges)?);
        self.container_cache().borrow_mut().curve_edges = Some(edges.clone());
        Ok(edges)
    }

    // pdfplumber/container.py:87 Container.edges
    /// The edges of the lines (`list(map(utils.line_to_edge, self.lines))`),
    /// then `rect_edges`, then `curve_edges`, cached.
    fn edges(&self) -> PyResult<Rc<Vec<Obj>>> {
        let _frame = py_frame()?;
        if let Some(edges) = &self.container_cache().borrow().edges {
            return Ok(edges.clone());
        }
        let lines = EDGES_LINES.get(self.class_name(), || self.lines())?;
        let mut edges = Vec::with_capacity(lines.len());
        for line in &lines {
            // utils.line_to_edge, called by map() from C
            let _entry = eval_entry()?;
            leaf(1)?;
            edges.push(line_to_edge(line)?);
        }
        edges.extend(EDGES_RECT_EDGES.get(self.class_name(), || self.rect_edges())?.iter().cloned());
        edges.extend(EDGES_CURVE_EDGES.get(self.class_name(), || self.curve_edges())?.iter().cloned());
        let edges = Rc::new(edges);
        self.container_cache().borrow_mut().edges = Some(edges.clone());
        Ok(edges)
    }

    // pdfplumber/container.py:95 Container.horizontal_edges
    fn horizontal_edges(&self) -> PyResult<Vec<Obj>> {
        let _frame = py_frame()?;
        oriented_edges(&HORIZONTAL_EDGES_EDGES.get(self.class_name(), || self.edges())?, "h")
    }

    // pdfplumber/container.py:102 Container.vertical_edges
    fn vertical_edges(&self) -> PyResult<Vec<Obj>> {
        let _frame = py_frame()?;
        oriented_edges(&VERTICAL_EDGES_EDGES.get(self.class_name(), || self.edges())?, "v")
    }

    // pdfplumber/container.py:108 Container.to_json
    /// `json.dumps(Serializer(...).serialize(self.to_dict(object_types)),
    /// indent=indent)`: CPython's encoder (`pycompat::json`: the C encoder
    /// without an indent, the Python one with).
    fn to_json(&self, options: &SerializeOptions) -> PyResult<PyStr> {
        let _frame = py_frame()?;
        let data = self.to_dict(options.object_types.as_deref())?;
        let serialized = Serializer::new(
            options.precision.clone(),
            options.include_attrs.as_deref(),
            options.exclude_attrs.as_deref(),
        )?
        .serialize(&PyValue::Dict(data))?;
        let indent = match &options.indent {
            None => JsonIndent::None,
            Some(v) if v.is_none() => JsonIndent::None,
            // ' ' * indent (a bool is an int)
            Some(PyValue::Str(s)) => JsonIndent::Str(s.to_string_lossy()),
            Some(v @ (PyValue::Int(_) | PyValue::Bool(_))) => {
                let n = match v.as_num() {
                    Some(pycompat::Num::Int(i)) => i.as_i64(),
                    _ => None,
                };
                match n {
                    Some(n) if n <= MAX_INDENT => JsonIndent::Str(" ".repeat(n.max(0) as usize)),
                    Some(_) => {
                        return Err(PyErr::new(ExcKind::NotPorted, "not modelled: a JSON indent of more than 2**20"))
                    }
                    None => return Err(pycompat::exc::OverflowError("cannot fit 'int' into an index-sized integer")),
                }
            }
            Some(v) => return pycompat::json::dumps_indent_error(v.type_name()).map(PyStr::from),
        };
        Ok(PyStr::from(pycompat::json::dumps(&serialized, &indent)?))
    }

    // pdfplumber/container.py:132 Container.to_csv
    /// `csv.DictWriter(stream, fieldnames=cols, extrasaction="ignore",
    /// quoting=csv.QUOTE_MINIMAL, escapechar="\\")`, header then rows:
    /// CPython's `csv` writer (`pycompat::csv`), `str()` of each value that
    /// is not a `str` (`PyValue::str_counted`), `\r\n` line ends.
    fn to_csv(&self, options: &SerializeOptions) -> PyResult<PyStr> {
        let _frame = py_frame()?;
        let object_types: Vec<PyStr> = match &options.object_types {
            Some(t) => t.clone(),
            None => {
                let mut t: Vec<PyStr> =
                    TO_CSV_OBJECTS.get(self.class_name(), || self.objects())?.keys().cloned().collect();
                t.push(PyStr::new("annot"));
                t
            }
        };
        let mut serialized: Vec<PyValue> = Vec::new();
        let mut fields: Vec<PyStr> = Vec::new();
        // [self] if self.pages is None else self.pages: a property of a
        // `PDF`, a class attribute (`None`) of a `Page`
        let class = self.class_name();
        let pages: Vec<Option<Rc<Page>>> = if class == "PDF" {
            TO_CSV_PAGES.get(class, || self.pages())?;
            match TO_CSV_PAGES_2.get(class, || self.pages())? {
                None => vec![None],
                Some(pages) => pages.into_iter().map(Some).collect(),
            }
        } else {
            TO_CSV_PAGES.plain(class);
            match self.pages()? {
                None => vec![None],
                Some(pages) => pages.into_iter().map(Some).collect(),
            }
        };
        let serializer = Serializer::new(
            options.precision.clone(),
            options.include_attrs.as_deref(),
            options.exclude_attrs.as_deref(),
        )?;
        for page in &pages {
            for t in &object_types {
                let name = t.concat(&PyStr::new("s"));
                // getattr(page, t + "s")
                let objs = match page {
                    None => getattr(self, &name, &TO_CSV_GETATTR)?,
                    Some(p) => getattr(&**p, &name, &TO_CSV_GETATTR)?,
                };
                if py_len(&objs)? > 0 {
                    // serialized += serializer.serialize(objs)
                    let s = serializer.serialize(&objs)?;
                    match crate::obj::py_iter(&s) {
                        Some(items) => serialized.extend(items?),
                        None => {
                            return Err(pycompat::exc::TypeError(format!("'{}' object is not iterable", s.type_name())))
                        }
                    }
                    // [k for k, v in objs[0].items() if type(v) is not dict]
                    let first = crate::obj::py_getitem_index(&objs, 0)?;
                    let Some(first) = dict_items(&first) else {
                        return Err(pycompat::exc::AttributeError(format!(
                            "'{}' object has no attribute 'items'",
                            first.type_name()
                        )));
                    };
                    for (k, v) in first {
                        if v.type_name() != "dict" && !fields.contains(&k) {
                            fields.push(k);
                        }
                    }
                }
            }
        }
        let required: Vec<PyStr> = CSV_COLS_REQUIRED.iter().map(|c| PyStr::new(c)).collect();
        let prepend: Vec<PyStr> = CSV_COLS_TO_PREPEND.iter().map(|c| PyStr::new(c)).collect();
        let mut rest: Vec<PyStr> =
            fields.into_iter().filter(|f| !required.contains(f) && !prepend.contains(f)).collect();
        rest.sort();
        let mut cols = required;
        for c in prepend.into_iter().chain(rest) {
            // filter(serializer.attr_filter, non_req_cols): the lambda from C
            let _entry = eval_entry()?;
            leaf(1)?;
            if serializer.attr_filter.accepts(&c) {
                cols.push(c);
            }
        }
        // csv.DictWriter(...): the class call, __init__
        pycompat::recursion::class_call_leaf()?;
        let mut out: Vec<u32> = Vec::new();
        {
            // w.writeheader() -> writerow -> _dict_to_list, whose generator
            // expression the C writer resumes
            let _writeheader = py_frame()?;
            let _writerow = py_frame()?;
            leaf(1)?;
            let _entry = eval_entry()?;
            leaf(1)?;
            let header: Vec<Option<PyStr>> = cols.iter().map(|c| Some(c.clone())).collect();
            pycompat::csv::write_row(&mut out, &header);
        }
        {
            // w.writerows(serialized): map(self._dict_to_list, rowdicts)
            // from C, each generator expression resumed from C
            let _writerows = py_frame()?;
            for row in &serialized {
                let _entry = eval_entry()?;
                leaf(1)?;
                let mut fields = Vec::with_capacity(cols.len());
                {
                    let _entry = eval_entry()?;
                    let _genexpr = py_frame()?;
                    let Some(items) = dict_items(row) else {
                        return Err(pycompat::exc::AttributeError(format!(
                            "'{}' object has no attribute 'get'",
                            row.type_name()
                        )));
                    };
                    for c in &cols {
                        // rowdict.get(key, self.restval): restval is ""
                        let value = items.iter().find(|(k, _)| k == c).map(|(_, v)| v.clone());
                        fields.push(match value {
                            None => Some(PyStr::empty()),
                            Some(v) if v.is_none() => None,
                            Some(v) => Some(v.str_counted()?),
                        });
                    }
                }
                pycompat::csv::write_row(&mut out, &fields);
            }
        }
        Ok(PyStr::from_code_points(out).expect("code points"))
    }
}

// The property loads of pdfplumber/container.py (`pycompat::specialize`).
static RECTS_OBJECTS: AttrSite = AttrSite::new("Container.rects self.objects");
static LINES_OBJECTS: AttrSite = AttrSite::new("Container.lines self.objects");
static CURVES_OBJECTS: AttrSite = AttrSite::new("Container.curves self.objects");
static IMAGES_OBJECTS: AttrSite = AttrSite::new("Container.images self.objects");
static CHARS_OBJECTS: AttrSite = AttrSite::new("Container.chars self.objects");
static TEXTBOXVERTICALS_OBJECTS: AttrSite = AttrSite::new("Container.textboxverticals self.objects");
static TEXTBOXHORIZONTALS_OBJECTS: AttrSite = AttrSite::new("Container.textboxhorizontals self.objects");
static TEXTLINEVERTICALS_OBJECTS: AttrSite = AttrSite::new("Container.textlineverticals self.objects");
static TEXTLINEHORIZONTALS_OBJECTS: AttrSite = AttrSite::new("Container.textlinehorizontals self.objects");
static RECT_EDGES_RECTS: AttrSite = AttrSite::new("Container.rect_edges self.rects");
static CURVE_EDGES_CURVES: AttrSite = AttrSite::new("Container.curve_edges self.curves");
static EDGES_LINES: AttrSite = AttrSite::new("Container.edges self.lines");
static EDGES_RECT_EDGES: AttrSite = AttrSite::new("Container.edges self.rect_edges");
static EDGES_CURVE_EDGES: AttrSite = AttrSite::new("Container.edges self.curve_edges");
static HORIZONTAL_EDGES_EDGES: AttrSite = AttrSite::new("Container.horizontal_edges self.edges");
static VERTICAL_EDGES_EDGES: AttrSite = AttrSite::new("Container.vertical_edges self.edges");
static TO_CSV_OBJECTS: AttrSite = AttrSite::new("Container.to_csv self.objects");
static TO_CSV_PAGES: AttrSite = AttrSite::new("Container.to_csv self.pages is None");
static TO_CSV_PAGES_2: AttrSite = AttrSite::new("Container.to_csv else self.pages");
static TO_CSV_GETATTR: CallSite = CallSite::new("Container.to_csv getattr(page, t + 's')");

/// Whether the attribute `name` of an object of class `class` is a
/// property (its getter a Python function), as `getattr` finds it: the
/// lists and edges of `Container`, `objects`, `annots`, `hyperlinks`,
/// `structure_tree` of both classes, `pages` of a `PDF` (a class attribute
/// of a `Page`), `width`, `height`, `layout` of a page.
pub fn is_property(class: &str, name: &PyStr) -> bool {
    match name.as_str() {
        Some(
            "rects"
            | "lines"
            | "curves"
            | "images"
            | "chars"
            | "textboxverticals"
            | "textboxhorizontals"
            | "textlineverticals"
            | "textlinehorizontals"
            | "rect_edges"
            | "curve_edges"
            | "edges"
            | "horizontal_edges"
            | "vertical_edges"
            | "objects"
            | "annots"
            | "hyperlinks"
            | "structure_tree",
        ) => true,
        Some("pages") => class == "PDF",
        Some("width" | "height" | "layout") => class != "PDF",
        _ => false,
    }
}

/// `getattr(c, name)` at a call site of the builtin: its generic call's C
/// unit (`site`, the first execution), and, for a property, the getter
/// called from C (`property.__get__`: an entry of the evaluation loop),
/// whatever the state of the site (`getattr` is no `LOAD_ATTR`).
pub fn getattr<C: Container + ?Sized>(c: &C, name: &PyStr, site: &CallSite) -> PyResult<PyValue> {
    let _call = site.builtin_call()?;
    let _entry = if is_property(c.class_name(), name) { Some(eval_entry()?) } else { None };
    c.attribute(name)
}

/// The largest indent `' ' * indent` builds here (beyond, `NotPorted`:
/// Python builds it, a case no caller of the port makes).
const MAX_INDENT: i64 = 1 << 20;

/// `list(chain(*(f(r) for r in objs)))`: the generator expression is
/// created, then unpacked from C (an evaluation loop entry and its frame
/// per item, `f`'s frame inside, and once more at the end).
fn chain_edges(objs: &[Obj], f: fn(&Obj) -> PyResult<Vec<Obj>>) -> PyResult<Vec<Obj>> {
    leaf(1)?;
    let mut out = Vec::new();
    for obj in objs {
        let _entry = eval_entry()?;
        let _genexpr = py_frame()?;
        leaf(1)?;
        out.extend(f(obj)?);
    }
    let _entry = eval_entry()?;
    leaf(1)?;
    Ok(out)
}

/// `list(filter(test, edges))` with `test(x) = bool(x["orientation"] ==
/// orientation)`, called from C.
fn oriented_edges(edges: &[Obj], orientation: &str) -> PyResult<Vec<Obj>> {
    let wanted = PyValue::str(orientation);
    let mut out = Vec::new();
    for e in edges {
        let _entry = eval_entry()?;
        leaf(1)?;
        if e.item("orientation")?.py_eq(&wanted) {
            out.push(e.clone());
        }
    }
    Ok(out)
}

/// `len(v)`, with the `TypeError` of a value that has none.
pub fn py_len(v: &PyValue) -> PyResult<usize> {
    let no_len = || pycompat::exc::TypeError(format!("object of type '{}' has no len()", v.type_name()));
    // PDFStream has __getitem__ (iterable) but no __len__
    if let PyValue::Pdf(pdfminer::pdftypes::PDFObject::Stream(_)) = v {
        return Err(no_len());
    }
    match crate::obj::py_iter(v) {
        Some(items) => Ok(items?.len()),
        None => Err(no_len()),
    }
}

/// The `(key, value)` items of a `dict` value (a PDF dict's values as
/// values), `None` for any other value.
fn dict_items(v: &PyValue) -> Option<Vec<(PyStr, PyValue)>> {
    match v {
        PyValue::Dict(d) => Some(d.items().to_vec()),
        PyValue::Pdf(pdfminer::pdftypes::PDFObject::Dict(d)) => {
            Some(d.items().into_iter().map(|(k, v)| (PyStr::new(&k), PyValue::from_pdf(v))).collect())
        }
        _ => None,
    }
}

/// A list of object dicts as a value.
pub fn obj_list(objs: Vec<Obj>) -> PyValue {
    PyValue::List(objs.into_iter().map(PyValue::Dict).collect())
}

/// Every name an instance of `PDF` or `Page` (and its subclasses) has as a
/// method: `getattr` gives a bound method, which is not a value of the
/// object model (`NotPorted`).
const METHODS: [&str; 30] = [
    "flush_cache",
    "to_json",
    "to_csv",
    "to_dict",
    "close",
    "point2coord",
    "process_object",
    "iter_layout_objects",
    "parse_objects",
    "debug_tablefinder",
    "find_tables",
    "find_table",
    "extract_tables",
    "extract_table",
    "get_textmap",
    "_get_textmap",
    "search",
    "extract_text",
    "extract_text_simple",
    "extract_words",
    "extract_text_lines",
    "crop",
    "within_bbox",
    "outside_bbox",
    "filter",
    "dedupe_chars",
    "to_image",
    "open",
    "__enter__",
    "__exit__",
];

/// `getattr(container, name)` for what `PDF` and `Page` share: the lists
/// by kind, the edges, `objects`, the cached properties when they are set,
/// `cached_properties`; `class` names the class in the `AttributeError`.
pub fn container_attribute<C: Container + ?Sized>(c: &C, class: &str, name: &PyStr) -> PyResult<PyValue> {
    let edges = |e: Rc<Vec<Obj>>| obj_list(e.to_vec());
    Ok(match name.as_str() {
        Some("rects") => obj_list(c.rects()?),
        Some("lines") => obj_list(c.lines()?),
        Some("curves") => obj_list(c.curves()?),
        Some("images") => obj_list(c.images()?),
        Some("chars") => obj_list(c.chars()?),
        Some("textboxverticals") => obj_list(c.textboxverticals()?),
        Some("textboxhorizontals") => obj_list(c.textboxhorizontals()?),
        Some("textlineverticals") => obj_list(c.textlineverticals()?),
        Some("textlinehorizontals") => obj_list(c.textlinehorizontals()?),
        Some("rect_edges") => edges(c.rect_edges()?),
        Some("curve_edges") => edges(c.curve_edges()?),
        Some("edges") => edges(c.edges()?),
        Some("horizontal_edges") => obj_list(c.horizontal_edges()?),
        Some("vertical_edges") => obj_list(c.vertical_edges()?),
        Some("objects") => objects_value(&*c.objects()?),
        Some(cached @ ("_rect_edges" | "_curve_edges" | "_edges")) => {
            let cache = c.container_cache().borrow();
            let set = match cached {
                "_rect_edges" => cache.rect_edges.clone(),
                "_curve_edges" => cache.curve_edges.clone(),
                _ => cache.edges.clone(),
            };
            match set {
                Some(e) => edges(e),
                None => return Err(no_attribute(class, name)),
            }
        }
        Some(m) if METHODS.contains(&m) => {
            return Err(PyErr::new(
                ExcKind::NotPorted,
                format!("not modelled: the method {class}.{m} as a value (getattr in to_dict/to_csv)"),
            ))
        }
        _ => return Err(no_attribute(class, name)),
    })
}

/// `self.objects` as a value: a dict of lists.
pub fn objects_value(objects: &Objects) -> PyValue {
    let mut d = Obj::new();
    for (k, v) in objects.iter() {
        d.set(k, obj_list(v.to_vec()));
    }
    PyValue::Dict(d)
}

/// `AttributeError: 'Page' object has no attribute 'xs'`
pub fn no_attribute(class: &str, name: &PyStr) -> PyErr {
    PyErr::with_message(
        ExcKind::AttributeError,
        &PyStr::new(&format!("'{class}' object has no attribute '")).concat(name).concat(&PyStr::new("'")),
    )
}

/// `flush_cache` of the properties of [`ContainerCache`] named in
/// `props`.
pub fn flush_container_cache(cache: &RefCell<ContainerCache>, props: &[&str]) {
    let mut cache = cache.borrow_mut();
    for p in props {
        match *p {
            "_rect_edges" => cache.rect_edges = None,
            "_curve_edges" => cache.curve_edges = None,
            "_edges" => cache.edges = None,
            _ => {}
        }
    }
}
