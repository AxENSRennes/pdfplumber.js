//! Stage `plumber_doc`: metadata (`strict_metadata` both ways), the
//! structure tree (document and pages: `to_dict`, `find_all`, `find`,
//! `all_mcids`, `element_bbox`, `repr`), derived pages (`crop`,
//! `within_bbox`, `outside_bbox`, strict or not, relative or not,
//! `filter`, `dedupe_chars`: boxes, objects, annotations), serialization
//! (`to_dict`, `to_json`, `to_csv`), `close`, the `open` options
//! (`unicode_norm`, `raise_unicode_errors`, `pages`) and passwords. The
//! traversal and the line format are those of
//! `oracle/stages/plumber_doc.py`.
//!
//! # Depth
//!
//! The Python stage is a generator that delegates to others (`yield from
//! tree_lines(...)`, `page_lines`, `option_lines`): each is a frame above
//! the stage body, and the lambdas it is given (`lambda:
//! pdf.structure_tree`) one more. `SEND` runs a generator inline (no C
//! unit), as a call of a Python function does; the `LOAD_ATTR` of a
//! property does not the first time its instruction runs in the process
//! (the getter is called from C, two units: [`PDF_STRUCTURE_TREE`],
//! [`PAGE_STRUCTURE_TREE`]). The structure trees are the only deep paths of
//! the stage that depend on it (`adv-structure`, "depth" windows).

use std::io::{self, Write};
use std::rc::Rc;

use pdfplumber::container::{Container, Objects, SerializeOptions};
use pdfplumber::obj::{Obj, PyValue};
use pdfplumber::page::{CropFn, FilterFn, Page};
use pdfplumber::pdf::{OpenOptions, Source, PDF};
use pdfplumber::structure::{Matcher, PDFStructElement, PDFStructTree, Scope};
use pycompat::recursion::{c_call, leaf, py_frame};
use pycompat::specialize::CallSite;
use pycompat::{exc, PyErr, PyInt, PyResult, PyStr};

use super::common::{terminated, Lines};
use super::plumber_common::{digest, exc as exc_line, select, table};
use super::plumber_ops::{
    DOC_DERIVED, DOC_DERIVED_BASE, DOC_DERIVED_PER_PAGE, DOC_MAX_ELEMENTS, DOC_MAX_FOUND, DOC_MAX_PROBED,
    DOC_NORM_PAGES, DOC_OPEN_OPTIONS, DOC_SERIALIZE, DOC_SERIALIZE_PER_PAGE, DOC_WRONG_PASSWORD, OUT_LIMIT,
};
use super::{Stage, StageInput};
use crate::canon::{canon, sha256_hex};
use crate::plumber::{from_value, to_value};
use crate::typed_json;
use crate::value::{self, Value};

pub const STAGE: Stage = Stage { name: "plumber_doc", run: Some(run), oracle_input: false };

/// `lambda: pdf.structure_tree` of `run`: the property's `LOAD_ATTR`.
pub static PDF_STRUCTURE_TREE: CallSite = CallSite::new("plumber_doc run pdf.structure_tree");
/// `lambda: page.structure_tree` of `page_lines`.
pub static PAGE_STRUCTURE_TREE: CallSite = CallSite::new("plumber_doc page_lines page.structure_tree");

const WHILE_CALLING: &str = " while calling a Python object";

fn s(text: &str) -> Value {
    Value::Str(value::PyStr::Text(text.to_owned()))
}

/// `canon(x)` of a pdfplumber value.
fn canon_py(v: &PyValue) -> String {
    canon(&to_value(v))
}

/// `exc(e)` of `plumber_objects.py`.
fn exc_text(e: &PyErr) -> String {
    exc_line(e)
}

/// `bounded(label, value)` of `plumber_doc.py`: `label text`, or `labeld
/// length digest` beyond `OUT_LIMIT` characters.
fn bounded(label: &str, value: &Value) -> String {
    let text = canon(value);
    if text.len() > OUT_LIMIT {
        format!("{label}d {} {}", text.len(), &sha256_hex(text.as_bytes())[..16])
    } else {
        format!("{label} {text}")
    }
}

// ------------------------------------------------------------------------------ names

/// `Names` of `plumber_objects.py`: the values of a page as written, the
/// inline image names numbered (`inline{"n": k}`, the port names them
/// `inline-<k>`), a `Page` as `Page{"page_number": n}`.
#[derive(Default)]
struct Names {
    inline: Vec<PyStr>,
}

/// Whether `name` is a name the port gives an inline image
/// (`pdfminer::pdfinterp::inline_image_name`).
fn is_inline_name(name: &PyStr) -> bool {
    name.as_str()
        .and_then(|n| n.strip_prefix("inline-"))
        .is_some_and(|k| !k.is_empty() && k.bytes().all(|c| c.is_ascii_digit()))
}

impl Names {
    /// Records the inline names of the images of `objects`, in order.
    fn learn(&mut self, objects: &Objects) {
        for (_, objs) in objects.iter() {
            for o in objs {
                let image = o.get("object_type").and_then(PyValue::as_str) == Some("image");
                // Python: `name == str(id(stream))`; the port's inline image
                // streams are the content parser's, without an object id
                let inline_stream = matches!(o.get("stream"),
                    Some(PyValue::Pdf(pdfminer::pdftypes::PDFObject::Stream(st))) if st.objid().is_none());
                if image && inline_stream {
                    if let Some(PyValue::Str(name)) = o.get("name") {
                        if is_inline_name(name) && !self.inline.contains(name) {
                            self.inline.push(name.clone());
                        }
                    }
                }
            }
        }
    }

    fn value(&self, v: &PyValue) -> Value {
        if let PyValue::Str(name) = v {
            if let Some(k) = self.inline.iter().position(|n| n == name) {
                return Value::Named {
                    name: "inline".into(),
                    fields: vec![(s("n"), Value::Int(PyInt::from(k as i64)))],
                };
            }
        }
        to_value(v)
    }

    /// An object dict with its `name` and top-level `Page` values written
    /// as above; `AttributeError` for a value without `items()`.
    fn obj(&self, o: &PyValue) -> PyResult<Value> {
        let PyValue::Dict(o) = o else {
            return Err(exc::AttributeError(format!("'{}' object has no attribute 'items'", o.type_name())));
        };
        Ok(self.dict(o))
    }

    fn dict(&self, o: &Obj) -> Value {
        Value::Dict(
            o.items()
                .iter()
                .map(|(k, v)| {
                    let key = k.as_str();
                    let v = if key == Some("name") || key == Some("P") { self.value(v) } else { to_value(v) };
                    (Value::py_str(k), v)
                })
                .collect(),
        )
    }

    /// A text that may hold inline names (`to_json`, `to_csv`): each one
    /// replaced by `<inline k>`.
    fn text(&self, text: &PyStr) -> PyStr {
        let mut out = text.clone();
        // Python's names (addresses) all have the same length; the port's
        // are prefixes of one another (`inline-1`, `inline-10`): the longer
        // ones are replaced first
        let mut order: Vec<(usize, &PyStr)> = self.inline.iter().enumerate().collect();
        order.sort_by_key(|(_, name)| std::cmp::Reverse(name.len()));
        for (k, name) in order {
            let parts = out.split(name);
            let mut joined = PyStr::empty();
            for (i, part) in parts.iter().enumerate() {
                if i > 0 {
                    joined = joined.concat(&PyStr::from(format!("<inline {k}>")));
                }
                joined = joined.concat(part);
            }
            out = joined;
        }
        out
    }
}

/// `annot_value(a, names)`: an annotation as written (its `data` dict holds
/// the `Page` in place of `/P`).
fn annot_value(a: &Obj, names: &Names) -> PyResult<Value> {
    let data = names.obj(a.get("data").unwrap_or(&PyValue::None))?;
    let mut items = Vec::with_capacity(a.len());
    for (k, v) in a.items() {
        let key = k.as_str();
        let v = if key == Some("data") {
            data.clone()
        } else if key == Some("name") || key == Some("P") {
            names.value(v)
        } else {
            to_value(v)
        };
        items.push((Value::py_str(k), v));
    }
    Ok(Value::Dict(items))
}

/// `page_names(page)`: the `Names` of a page's objects (none when
/// `page.objects` raises).
fn page_names(page: &Page) -> PyResult<Names> {
    // a Python function: its frame, then `Names()` (a class call and its
    // __init__), then `page.objects` in a try block
    let _frame = py_frame()?;
    pycompat::recursion::class_call_leaf()?;
    let mut names = Names::default();
    if let Ok(objects) = page.objects() {
        names.learn(&objects);
    }
    Ok(names)
}

/// `objects_digest(page)`: `[kind, n, digest]` of each kind.
fn objects_digest(page: &Page) -> PyResult<Value> {
    let _frame = py_frame()?;
    let mut names = Names::default();
    let objects = page.objects()?;
    names.learn(&objects);
    let mut out = Vec::new();
    for (kind, objs) in objects.iter() {
        let values = Value::List(objs.iter().map(|o| names.dict(o)).collect());
        out.push(Value::List(vec![
            Value::py_str(kind),
            Value::Int(PyInt::from(objs.len() as i64)),
            s(&digest(&values)),
        ]));
    }
    Ok(Value::List(out))
}

/// `[annot_value(a, names) for a in annots]`, digested.
fn annots_digest(annots: &[Obj]) -> PyResult<String> {
    let names = Names::default();
    let values = annots.iter().map(|a| annot_value(a, &names)).collect::<PyResult<Vec<_>>>()?;
    Ok(digest(&Value::List(values)))
}

// ---------------------------------------------------------------------------- opening

/// `open_pdf(path, password, max_pages, **options)` of
/// `plumber_objects.py`: `pdfplumber.open(path, pages=PAGES,
/// password=password, **options)`.
fn open_pdf(input: &StageInput<'_>, password: Option<&str>, options: OpenOptions) -> PyResult<PDF> {
    let _frame = py_frame()?;
    let pages = input.max_pages.map(|n| PyValue::list((1..=n as i64).map(|i| PyValue::Int(PyInt::from(i)))));
    open_with(input, password, OpenOptions { pages, ..options })
}

/// `pdfplumber.open(path, password=password, **options)`.
fn open_with(input: &StageInput<'_>, password: Option<&str>, options: OpenOptions) -> PyResult<PDF> {
    let source = Source::Path { data: Rc::from(input.pdf), path: PyStr::new("input.pdf") };
    let password = password.map(PyValue::str);
    PDF::open(source, OpenOptions { password, ..options })
}

/// The stage's own password: `None` when none was given.
fn password_of<'a>(input: &StageInput<'a>) -> Option<&'a str> {
    input.password_given.then_some(input.password)
}

/// The `OpenOptions` of a dict of keyword arguments (`OPEN_OPTIONS`).
fn open_options(options: &Value) -> OpenOptions {
    let Value::Dict(items) = options else { panic!("an options dict") };
    let mut out = OpenOptions { raise_unicode_errors: true, ..OpenOptions::default() };
    for (k, v) in items {
        let v = from_value(v).expect("an option value");
        match k.as_str().expect("an option name") {
            "strict_metadata" => out.strict_metadata = v.truthy(),
            "unicode_norm" => out.unicode_norm = Some(v),
            "raise_unicode_errors" => out.raise_unicode_errors = v.truthy(),
            "pages" => out.pages = Some(v),
            other => panic!("an option the table does not have: {other}"),
        }
    }
    out
}

fn has_option(options: &Value, name: &str) -> bool {
    let Value::Dict(items) = options else { return false };
    items.iter().any(|(k, _)| k.as_str() == Some(name))
}

// ------------------------------------------------------------------------- structure

/// The first `DOC_MAX_PROBED` elements under `children`, depth-first, with
/// their paths (`_paths`, no recursion).
fn paths(children: &[Rc<PDFStructElement>]) -> Vec<(Vec<usize>, Rc<PDFStructElement>)> {
    let mut out = Vec::new();
    let mut stack: Vec<(Vec<usize>, Rc<PDFStructElement>)> =
        children.iter().enumerate().rev().map(|(i, el)| (vec![i], el.clone())).collect();
    while let Some((path, el)) = stack.pop() {
        if out.len() >= DOC_MAX_PROBED {
            break;
        }
        let kids = el.children.borrow().clone();
        for (j, c) in kids.iter().enumerate().rev() {
            let mut p = path.clone();
            p.push(j);
            stack.push((p, c.clone()));
        }
        out.push((path, el));
    }
    out
}

/// `tree_lines(label, get_list, make_tree)`: the generator's frame, the
/// lambdas' frames, then the lines.
fn tree_lines(
    lines: &mut Lines,
    label: &str,
    get_list: impl FnOnce() -> PyResult<Vec<PyValue>>,
    make_tree: impl FnOnce() -> PyResult<PDFStructTree>,
) -> PyResult<()> {
    let _generator = py_frame()?;
    let elements = {
        let _lambda = py_frame()?;
        get_list()
    };
    match elements {
        Err(e) => lines.push(format!("{label} {}", exc_text(&e))),
        Ok(elements) => {
            let values = Value::List(elements.iter().map(to_value).collect());
            lines.push(format!("{label} {} {}", elements.len(), digest(&values)));
            for (i, el) in elements.iter().take(DOC_MAX_ELEMENTS).enumerate() {
                lines.push(bounded(&format!("struct {i}"), &to_value(el)));
            }
        }
    }
    let tree = {
        let _lambda = py_frame()?;
        make_tree()
    };
    let tree = match tree {
        Ok(tree) => tree,
        Err(e) => {
            lines.push(format!("tree {}", exc_text(&e)));
            return Ok(());
        }
    };
    // _first_type(tree.children)
    leaf(1)?;
    let tag = match tree.children.first() {
        Some(el) => el.r#type.clone(),
        None => PyStr::new("P"),
    };
    let span = PyStr::new("Span");
    let starts_with_h = |el: &PDFStructElement| -> PyResult<bool> {
        // lambda el: el.type.startswith("H")
        leaf(1)?;
        Ok(el.r#type.as_bytes().starts_with(b"H"))
    };
    let matchers: [(Value, Matcher<'_>, Matcher<'_>); 3] = [
        (Value::List(vec![s("tag"), Value::py_str(&tag)]), Matcher::Tag(&tag), Matcher::Tag(&tag)),
        (Value::List(vec![s("tag"), s("Span")]), Matcher::Tag(&span), Matcher::Tag(&span)),
        (
            Value::List(vec![s("function"), s("starts-with-H")]),
            Matcher::Func(&starts_with_h),
            Matcher::Func(&starts_with_h),
        ),
    ];
    for (desc, for_all, for_find) in matchers {
        let desc = canon(&desc);
        let found = (|| -> PyResult<Vec<Value>> {
            let mut found = Vec::new();
            for el in tree.find_all(for_all)? {
                let el = el?;
                if found.len() == DOC_MAX_FOUND {
                    found.push(s("..."));
                    break;
                }
                found.push(Value::py_str(&el.r#type));
            }
            Ok(found)
        })();
        match found {
            Ok(found) => lines.push(format!("find_all {desc} {}", canon(&Value::List(found)))),
            Err(e) => lines.push(format!("find_all {desc} {}", exc_text(&e))),
        }
        match tree.find(for_find) {
            Ok(el) => {
                let v = el.map_or(Value::None, |el| Value::py_str(&el.r#type));
                lines.push(format!("find {desc} {}", canon(&v)));
            }
            Err(e) => lines.push(format!("find {desc} {}", exc_text(&e))),
        }
    }
    // _paths(tree.children): a function
    let probed = {
        let _frame = py_frame()?;
        paths(&tree.children)
    };
    for (path, el) in probed {
        let path = canon(&Value::Tuple(path.iter().map(|&i| Value::Int(PyInt::from(i as i64))).collect()));
        match el.all_mcids() {
            Ok(mcids) => {
                let v = Value::List(
                    mcids
                        .iter()
                        .map(|(n, m)| Value::Tuple(vec![n.clone().map_or(Value::None, Value::Int), to_value(m)]))
                        .collect(),
                );
                lines.push(format!("mcids {path} {}", canon(&v)));
            }
            Err(e) => lines.push(format!("mcids {path} {}", exc_text(&e))),
        }
        match tree.element_bbox(&el) {
            Ok(bbox) => {
                lines.push(format!("element_bbox {path} {}", canon(&Value::Tuple(bbox.iter().map(to_value).collect()))))
            }
            Err(e) => lines.push(format!("element_bbox {path} {}", exc_text(&e))),
        }
    }
    if let Some(first) = tree.children.first() {
        // repr(tree.children[0]): the builtin's unit, then PyObject_Repr
        let text = {
            let _call = c_call(WHILE_CALLING)?;
            first.repr()
        };
        match text {
            Ok(text) => lines.push(format!("element_repr {} {}", text.len(), &sha256_hex(text.as_bytes())[..16])),
            Err(e) => lines.push(format!("element_repr {}", exc_text(&e))),
        }
    }
    Ok(())
}

// ------------------------------------------------------------------------------ pages

/// `PREDICATES` of `plumber_text.py`: Python functions, called by `filter`
/// from C.
fn predicate(name: &str) -> FilterFn {
    match name {
        // obj.get("upright", True)
        "upright" => Rc::new(|o: &Obj| {
            leaf(1)?;
            Ok(o.get("upright").cloned().unwrap_or(PyValue::Bool(true)))
        }),
        // obj["object_type"] != "rect"
        "not-rect" => Rc::new(|o: &Obj| {
            leaf(1)?;
            Ok(PyValue::Bool(!o.item("object_type")?.py_eq(&PyValue::str("rect"))))
        }),
        // obj.get("size", 0) < 10
        "small" => Rc::new(|o: &Obj| {
            leaf(1)?;
            let size = o.get("size").cloned().unwrap_or(PyValue::Int(PyInt::ZERO));
            Ok(PyValue::Bool(size.lt(&PyValue::Int(PyInt::from(10)))?))
        }),
        other => panic!("no predicate {other}"),
    }
}

/// `derive_bbox(page, fractions, relative)`.
fn derive_bbox(page: &Page, fractions: &[Value], relative: bool) -> PyResult<PyValue> {
    let _frame = py_frame()?;
    let [x0, top, x1, bottom] = pdfplumber::obj::bbox_from_value(&page.bbox)?;
    let w = x1.sub(&x0)?;
    let h = bottom.sub(&top)?;
    let f: Vec<PyValue> = fractions.iter().map(|v| from_value(v).expect("a number")).collect();
    let [f0, f1, f2, f3] = &f[..] else { panic!("four fractions") };
    Ok(if relative {
        PyValue::tuple([w.mul(f0)?, h.mul(f1)?, w.mul(f2)?, h.mul(f3)?])
    } else {
        PyValue::tuple([x0.add(&w.mul(f0)?)?, top.add(&h.mul(f1)?)?, x0.add(&w.mul(f2)?)?, top.add(&h.mul(f3)?)?])
    })
}

/// `derive(page, steps)` of `plumber_text.py`.
fn derive(page: &Rc<Page>, steps: &[Value]) -> PyResult<Rc<Page>> {
    let _frame = py_frame()?;
    let mut page = page.clone();
    for step in steps {
        let Value::List(step) = step else { panic!("a step") };
        let name = step[0].as_str().expect("a step name");
        match name {
            "crop" | "within_bbox" | "outside_bbox" => {
                let Value::List(fractions) = &step[1] else { panic!("fractions") };
                let kwargs = crate::plumber::obj_from_value(&step[2]).expect("a kwargs dict");
                let flag = |k: &str, default: bool| kwargs.get(k).map_or(default, PyValue::truthy);
                let relative = flag("relative", false);
                let bbox = derive_bbox(&page, fractions, kwargs.get("relative").is_some_and(PyValue::truthy))?;
                let crop_fn = match name {
                    "crop" => CropFn::CropToBbox,
                    "within_bbox" => CropFn::WithinBbox,
                    _ => CropFn::OutsideBbox,
                };
                page = page.crop(bbox, crop_fn, relative, flag("strict", true))?;
            }
            "filter" => page = page.filter(predicate(step[1].as_str().expect("a predicate name")))?,
            "dedupe_chars" => {
                let kwargs = crate::plumber::obj_from_value(&step[1]).expect("a kwargs dict");
                page = page.dedupe_chars(&kwargs)?;
            }
            other => panic!("no step {other}"),
        }
    }
    Ok(page)
}

/// `derived_lines(page, steps)`.
fn derived_lines(lines: &mut Lines, page: &Rc<Page>, steps: &Value) -> PyResult<()> {
    let _generator = py_frame()?;
    lines.push(format!("derived {}", canon(steps)));
    let Value::List(list) = steps else { panic!("steps") };
    let d = match derive(page, list) {
        Ok(d) => d,
        Err(e) => {
            lines.push(format!("derive {}", exc_text(&e)));
            return Ok(());
        }
    };
    let width = d.width()?;
    let height = d.height()?;
    lines.push(format!(
        "dbox {} {} {} {}",
        canon_py(&d.bbox),
        canon_py(&width),
        canon_py(&height),
        canon(&s(&d.repr()))
    ));
    match objects_digest(&d) {
        Ok(v) => lines.push(format!("dobjects {}", canon(&v))),
        Err(e) => lines.push(format!("dobjects {}", exc_text(&e))),
    }
    match d.annots() {
        Ok(annots) => lines.push(format!("dannots {} {}", annots.len(), annots_digest(&annots)?)),
        Err(e) => lines.push(format!("dannots {}", exc_text(&e))),
    }
    Ok(())
}

/// The string list of a keyword argument.
fn str_list(v: &PyValue) -> Vec<PyStr> {
    v.as_seq().expect("a list").iter().map(|x| x.as_pystr().expect("a str").clone()).collect()
}

/// `page_dict(value, names)`: a `to_dict` value as written.
fn page_dict(value: &Obj, names: &Names) -> PyResult<Value> {
    let mut items = Vec::new();
    for (k, v) in value.items() {
        let v = match v {
            PyValue::List(objs) => {
                let annots = k.as_str() == Some("annots");
                Value::List(
                    objs.iter()
                        .map(|a| match (annots, a) {
                            (true, PyValue::Dict(a)) => annot_value(a, names),
                            (_, a) => names.obj(a),
                        })
                        .collect::<PyResult<_>>()?,
                )
            }
            other => to_value(other),
        };
        items.push((Value::py_str(k), v));
    }
    Ok(Value::Dict(items))
}

/// `serialize_lines(page, op)`.
fn serialize_lines(lines: &mut Lines, page: &Rc<Page>, op: &Value) -> PyResult<()> {
    let _generator = py_frame()?;
    let Value::List(parts) = op else { panic!("an op") };
    let method = parts[0].as_str().expect("a method");
    let kwargs = crate::plumber::obj_from_value(&parts[1]).expect("a kwargs dict");
    let label = format!("serialize {}", canon(op));
    let options = SerializeOptions {
        object_types: kwargs.get("object_types").map(str_list),
        include_attrs: kwargs.get("include_attrs").map(str_list),
        exclude_attrs: kwargs.get("exclude_attrs").map(str_list),
        precision: kwargs.get("precision").cloned(),
        indent: kwargs.get("indent").cloned(),
    };
    let names = |page: &Page| page_names(page);
    let value = match method {
        "to_dict" => page.to_dict(options.object_types.as_deref()).map(|d| {
            let names = names(page)?;
            page_dict(&d, &names)
        }),
        "to_json" => page.to_json(&options).map(|t| names(page).map(|n| Value::py_str(&n.text(&t)))),
        "to_csv" => page.to_csv(&options).map(|t| names(page).map(|n| Value::py_str(&n.text(&t)))),
        other => panic!("no method {other}"),
    };
    let value = match value {
        Ok(v) => v?,
        Err(e) => {
            lines.push(format!("{label} {}", exc_text(&e)));
            return Ok(());
        }
    };
    let text = canon(&value);
    if text.len() > OUT_LIMIT {
        lines.push(format!("serialized {} {} {}", canon(op), text.len(), &sha256_hex(text.as_bytes())[..16]));
    } else {
        lines.push(format!("{label} {text}"));
    }
    Ok(())
}

/// `page_lines(pdf, page, sha, index)`.
fn page_lines(lines: &mut Lines, pdf: &PDF, page: &Rc<Page>, sha: &str, index: usize) -> PyResult<()> {
    let _generator = py_frame()?;
    lines.push(format!("page {} {}", page.page_number, canon(&s(&page.repr()))));
    tree_lines(
        lines,
        "page_structure_tree",
        || {
            let _load = PAGE_STRUCTURE_TREE.property_load()?;
            page.structure_tree()
        },
        || PDFStructTree::new(&pdf.core.doc, Scope::Page(page)),
    )?;
    let base = table(DOC_DERIVED_BASE);
    let derived = table(DOC_DERIVED);
    let mut all: Vec<&Value> = base.iter().collect();
    all.extend(select(sha, index, &derived, DOC_DERIVED_PER_PAGE));
    for steps in all {
        derived_lines(lines, page, steps)?;
    }
    let serialize = table(DOC_SERIALIZE);
    for op in select(sha, index, &serialize, DOC_SERIALIZE_PER_PAGE) {
        serialize_lines(lines, page, op)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------- options

/// `option_lines(path, password, max_pages, options)`.
fn option_lines(lines: &mut Lines, input: &StageInput<'_>, options: &Value) -> PyResult<()> {
    let _generator = py_frame()?;
    lines.push(format!("options {}", canon(options)));
    let opts = open_options(options);
    if has_option(options, "pages") {
        let pdf = match open_with(input, password_of(input), opts) {
            Ok(pdf) => pdf,
            Err(e) => {
                lines.push(format!("open {}", exc_text(&e)));
                return Ok(());
            }
        };
        let selected = pdf.pages().map(|pages| {
            Value::List(
                pages
                    .iter()
                    .map(|p| Value::Tuple(vec![Value::Int(p.page_number.clone()), to_value(&p.initial_doctop)]))
                    .collect(),
            )
        });
        match selected {
            Ok(v) => lines.push(format!("selected {}", canon(&v))),
            Err(e) => lines.push(format!("selected {}", exc_text(&e))),
        }
        return Ok(());
    }
    let pdf = match open_pdf(input, password_of(input), opts) {
        Ok(pdf) => pdf,
        Err(e) => {
            lines.push(format!("open {}", exc_text(&e)));
            return Ok(());
        }
    };
    if has_option(options, "strict_metadata") {
        lines.push(format!("metadata {}", canon(&crate::plumber::obj_value(&pdf.metadata))));
        return Ok(());
    }
    let pages = match pdf.pages() {
        Ok(pages) => pages,
        Err(e) => {
            lines.push(format!("pages {}", exc_text(&e)));
            return Ok(());
        }
    };
    if has_option(options, "unicode_norm") {
        for page in pages.iter().take(DOC_NORM_PAGES) {
            let texts = page
                .chars()
                .and_then(|chars| chars.iter().map(|c| c.item("text").map(to_value)).collect::<PyResult<Vec<_>>>());
            match texts {
                Ok(texts) => lines.push(bounded(&format!("texts {}", page.page_number), &Value::List(texts))),
                Err(e) => lines.push(format!("texts {} {}", page.page_number, exc_text(&e))),
            }
        }
        return Ok(());
    }
    for page in &pages {
        match page.annots() {
            Ok(annots) => {
                lines.push(format!("annots {} {} {}", page.page_number, annots.len(), annots_digest(&annots)?))
            }
            Err(e) => lines.push(format!("annots {} {}", page.page_number, exc_text(&e))),
        }
    }
    Ok(())
}

/// `password_lines(path, password, max_pages)`.
fn password_lines(lines: &mut Lines, input: &StageInput<'_>) -> PyResult<()> {
    let _generator = py_frame()?;
    let wrong = match typed_json::parse(DOC_WRONG_PASSWORD) {
        Ok(Value::Str(ref p)) => p.as_str().expect("a password without surrogates").to_owned(),
        other => panic!("a password: {other:?}"),
    };
    let mut others: Vec<Option<&str>> = Vec::new();
    if input.password_given {
        others.push(None);
    }
    others.push(Some(&wrong));
    for other in others {
        let label = canon(&other.map_or(Value::None, s));
        match open_pdf(input, other, OpenOptions { raise_unicode_errors: true, ..OpenOptions::default() }) {
            Ok(_) => lines.push(format!("password {label} ok")),
            Err(e) => lines.push(format!("password {label} {}", exc_text(&e))),
        }
    }
    Ok(())
}

// ------------------------------------------------------------------------------ stage

fn run(input: &StageInput<'_>, out: &mut dyn Write) -> io::Result<()> {
    terminated(out, |lines| body(input, lines))
}

fn body(input: &StageInput<'_>, lines: &mut Lines) -> PyResult<()> {
    // file_sha256(path): a function
    let sha = {
        leaf(1)?;
        sha256_hex(input.pdf)
    };
    let defaults = OpenOptions { raise_unicode_errors: true, ..OpenOptions::default() };
    let pdf = match open_pdf(input, password_of(input), defaults) {
        Ok(pdf) => pdf,
        Err(e) => {
            lines.push(format!("open {}", exc_text(&e)));
            return Ok(());
        }
    };
    lines.push(format!("metadata {}", canon(&crate::plumber::obj_value(&pdf.metadata))));
    tree_lines(
        lines,
        "structure_tree",
        || {
            let _load = PDF_STRUCTURE_TREE.property_load()?;
            pdf.structure_tree()
        },
        || PDFStructTree::new(&pdf.core.doc, Scope::Document(&pdf)),
    )?;
    let pages = match pdf.pages() {
        Ok(pages) => pages,
        Err(e) => {
            lines.push(format!("pages {}", exc_text(&e)));
            Vec::new()
        }
    };
    for (index, page) in pages.iter().enumerate() {
        page_lines(lines, &pdf, page, &sha, index)?;
    }
    let to_dict = (|| -> PyResult<Value> {
        let object_types: Vec<PyStr> =
            ["char", "rect", "line", "curve", "image"].iter().map(|t| PyStr::new(t)).collect();
        let value = pdf.to_dict(Some(&object_types))?;
        let pages = pdf.pages()?;
        let mut items = Vec::new();
        for (k, v) in value.items() {
            if k.as_str() == Some("pages") {
                let dicts = v.as_seq().expect("a list of page dicts");
                let mut out = Vec::new();
                // zip(pdf.pages, value["pages"])
                for (p, d) in pages.iter().zip(dicts) {
                    let PyValue::Dict(d) = d else { panic!("a page dict") };
                    out.push(page_dict(d, &page_names(p)?)?);
                }
                items.push((Value::py_str(k), Value::List(out)));
            } else {
                items.push((Value::py_str(k), to_value(v)));
            }
        }
        Ok(Value::Dict(items))
    })();
    match to_dict {
        Ok(v) => lines.push(format!("pdf_to_dict {}", digest(&v))),
        Err(e) => lines.push(format!("pdf_to_dict {}", exc_text(&e))),
    }
    match pdf.close() {
        Ok(()) => lines.push("close ok"),
        Err(e) => lines.push(format!("close {}", exc_text(&e))),
    }
    match pdf.pages().and_then(|reopened| reopened.first().map(|p| objects_digest(p)).transpose()) {
        Ok(Some(v)) => lines.push(format!("closed_objects {}", canon(&v))),
        Ok(None) => {}
        Err(e) => lines.push(format!("closed_objects {}", exc_text(&e))),
    }
    for options in table(DOC_OPEN_OPTIONS).iter() {
        option_lines(lines, input, options)?;
    }
    password_lines(lines, input)?;
    Ok(())
}
