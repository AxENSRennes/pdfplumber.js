//! Replays the cases of `oracle/unit/text_tables.py` (pdfplumber results)
//! through `pdfplumber::{obj, utils::{clustering, geometry, text}, table}`
//! and `pycompat::listsort`.
//!
//! Every result (or exception) is compared through its canonical text
//! (`canon`), or the SHA-256 of it for long results of real pages. The page
//! operations go through the functions of `pdfplumber/page.py` that the
//! generator calls (`Page._get_textmap`, `extract_text`, `search`,
//! `extract_text_lines`, `find_tables`, `find_table`, `extract_tables`,
//! `extract_table`), mirrored here (`page.rs` is not ported yet).
//!
//! To investigate a mismatch: `python -m oracle.unit.text_tables --dump
//! pages_fuzz:LINE:OP` prints Python's full canon of one operation, and
//! `TEXT_TABLES_DUMP=1` prints the Rust one.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::rc::Rc;

use pdfplumber::obj::{bbox_from_value, bbox_value, Bbox, Obj, PyValue};
use pdfplumber::table::{
    cells_to_tables, CellGroup, Table, TableFinder, TablePage, TableSettings,
};
use pdfplumber::utils::clustering::{cluster_list, cluster_objects};
use pdfplumber::utils::geometry::{self, Axis, SnapAttr};
use pdfplumber::utils::text::{
    chars_to_textmap, dedupe_chars, extract_text_kwargs, extract_text_simple, extract_words_kwargs,
    text_line_matches, Kwargs, RegexMatch, TextMap,
};
use pdftrace::canon::{canon, sha256_hex};
use pdftrace::plumber::{from_value, obj_from_value, to_value};
use pdftrace::typed_json;
use pdftrace::unit::UnitData;
use pdftrace::value::Value;
use pycompat::{exc, PyResult};

/// Calls `f(index, args, out)` on each case of `<name>.jsonl`, reading the
/// file line by line (the page corpora are large). Returns the count.
fn for_each_case(data: &UnitData, name: &str, mut f: impl FnMut(usize, Vec<Value>, Value)) -> usize {
    let path = data.path(&format!("{name}.jsonl"));
    let file = File::open(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let mut n = 0;
    for (i, line) in BufReader::new(file).lines().enumerate() {
        let line = line.expect("a line");
        let json = typed_json::parse_json(&line).unwrap_or_else(|e| panic!("{}:{}: {e}", path.display(), i + 1));
        let mut args = typed_json::from_json(json.get("in").expect("\"in\"")).expect("typed \"in\"");
        let args = match &mut args {
            Value::List(args) => std::mem::take(args),
            other => panic!("{name}: \"in\" is not a list: {other:?}"),
        };
        let out = typed_json::from_json(json.get("out").expect("\"out\"")).expect("typed \"out\"");
        drop(json);
        f(i, args, out);
        n += 1;
    }
    n
}

fn data() -> UnitData {
    UnitData::open("text_tables")
}

fn py(v: &Value) -> PyValue {
    from_value(v).unwrap_or_else(|e| panic!("{e}"))
}

fn obj(v: &Value) -> Obj {
    obj_from_value(v).unwrap_or_else(|e| panic!("{e}"))
}

fn objs(v: &Value) -> Vec<Obj> {
    match v {
        Value::List(items) => items.iter().map(obj).collect(),
        other => panic!("not a list of objects: {other:?}"),
    }
}

fn result<T>(r: PyResult<T>, f: impl FnOnce(T) -> Value) -> Value {
    match r {
        Ok(v) => f(v),
        Err(e) => Value::from(&e),
    }
}

fn list_of_objs(v: Vec<Obj>) -> Value {
    Value::List(v.iter().map(|o| to_value(&PyValue::Dict(o.clone()))).collect())
}

fn bbox_v(b: &Bbox) -> Value {
    to_value(&bbox_value(b))
}

/// Compares and counts mismatches (prints the first ones).
struct Checker {
    name: &'static str,
    failures: usize,
    total: usize,
}

impl Checker {
    fn new(name: &'static str) -> Checker {
        Checker { name, failures: 0, total: 0 }
    }

    fn check(&mut self, what: impl FnOnce() -> String, got: &Value, want: &Value) {
        self.check_text(what, &canon(got), &canon(want));
    }

    fn check_text(&mut self, what: impl FnOnce() -> String, got: &str, want: &str) {
        self.total += 1;
        let same = match want.strip_prefix("sha256:") {
            Some(rest) => {
                let hex = rest.split(':').next().expect("sha");
                sha256_hex(got.as_bytes()) == hex
            }
            None => got == want,
        };
        if !same {
            self.failures += 1;
            if self.failures <= 10 {
                let clip = |s: &str| s.chars().take(1500).collect::<String>();
                eprintln!("{} mismatch: {}\n  rust:   {}\n  python: {}", self.name, what(), clip(got), clip(want));
            }
            if std::env::var_os("TEXT_TABLES_DUMP").is_some() {
                eprintln!("{} rust canon: {got}", self.name);
            }
        }
    }

    fn finish(self, min_cases: usize) {
        assert!(self.total >= min_cases, "{}: only {} cases", self.name, self.total);
        assert_eq!(self.failures, 0, "{}: {} of {} cases differ", self.name, self.failures, self.total);
    }
}

// -- sort -----------------------------------------------------------------------

#[test]
fn sort_cases() {
    let data = data();
    let mut c = Checker::new("sort");
    for_each_case(&data, "sort", |i, args, out| {
        let xs: Vec<PyValue> = match &args[0] {
            Value::List(items) => items.iter().map(py).collect(),
            other => panic!("{other:?}"),
        };
        let got = result(pycompat::listsort::sort_permutation(xs.len(), |a, b| xs[a].lt(&xs[b])), |perm| {
            Value::List(perm.into_iter().map(|k| Value::Int(pycompat::PyInt::Small(k as i64))).collect())
        });
        c.check(|| format!("case {i}"), &got, &out);
    });
    c.finish(1000);
}

// -- operators --------------------------------------------------------------------

#[test]
fn ops_cases() {
    let data = data();
    let mut c = Checker::new("ops");
    for_each_case(&data, "ops", |i, args, out| {
        let op = args[0].as_str().expect("op").to_owned();
        let (a, b) = (py(&args[1]), py(&args[2]));
        let v = |r: PyResult<PyValue>| result(r, |v| to_value(&v));
        let got = match op.as_str() {
            "add" => v(a.add(&b)),
            "sub" => v(a.sub(&b)),
            "mul" => v(a.mul(&b)),
            "truediv" => v(a.truediv(&b)),
            "lt" => v(a.lt(&b).map(PyValue::Bool)),
            "le" => v(a.le(&b).map(PyValue::Bool)),
            "gt" => v(a.gt(&b).map(PyValue::Bool)),
            "ge" => v(a.ge(&b).map(PyValue::Bool)),
            "eq" => v(Ok(PyValue::Bool(a.py_eq(&b)))),
            "neg" => v(a.neg()),
            "abs" => v(a.abs()),
            "repr" => v(Ok(PyValue::from(a.repr()))),
            "bool" => v(Ok(PyValue::Bool(a.truthy()))),
            "min" => v(pdfplumber::obj::py_min([&a, &b])),
            "max" => v(pdfplumber::obj::py_max([&a, &b])),
            "sum" => v(pdfplumber::obj::py_sum([&a, &b])),
            other => panic!("unknown op {other}"),
        };
        c.check(|| format!("case {i}: {op} {a:?} {b:?}"), &got, &out);
    });
    c.finish(5000);
}

// -- clustering -------------------------------------------------------------------

#[test]
fn cluster_cases() {
    let data = data();
    let mut c = Checker::new("cluster");
    for_each_case(&data, "cluster", |i, args, out| {
        let name = args[0].as_str().expect("fn").to_owned();
        let got = match name.as_str() {
            "cluster_list" => {
                let xs = match &args[1] {
                    Value::List(items) => items.iter().map(py).collect(),
                    other => panic!("{other:?}"),
                };
                result(cluster_list(xs, &py(&args[2])), |groups| {
                    Value::List(groups.iter().map(|g| Value::List(g.iter().map(to_value).collect())).collect())
                })
            }
            "cluster_objects" => {
                let xs = objs(&args[1]);
                let preserve = matches!(args[3], Value::Bool(true));
                result(cluster_objects(&xs, |o| Ok(o.item("v")?.clone()), &py(&args[2]), preserve), |groups| {
                    Value::List(groups.into_iter().map(list_of_objs).collect())
                })
            }
            other => panic!("{other}"),
        };
        c.check(|| format!("case {i}: {name}"), &got, &out);
    });
    c.finish(2000);
}

// -- geometry ---------------------------------------------------------------------

fn snap_attr(v: &Value) -> SnapAttr {
    match v.as_str() {
        Some("x0") => SnapAttr::X0,
        Some("x1") => SnapAttr::X1,
        Some("top") => SnapAttr::Top,
        Some("bottom") => SnapAttr::Bottom,
        other => panic!("{other:?}"),
    }
}

fn opt_obj(r: PyResult<Option<Obj>>) -> Value {
    result(r, |o| o.map_or(Value::None, |o| to_value(&PyValue::Dict(o))))
}

#[test]
fn geometry_cases() {
    let data = data();
    let mut c = Checker::new("geometry");
    for_each_case(&data, "geometry", |i, args, out| {
        let name = args[0].as_str().expect("fn").to_owned();
        let bbox = |v: &Value| bbox_from_value(&py(v)).expect("a bbox");
        let objs_v = |r: PyResult<Vec<Obj>>| result(r, list_of_objs);
        let got = match name.as_str() {
            "objects_to_bbox" => result(geometry::objects_to_bbox(&objs(&args[1])), |b| bbox_v(&b)),
            "objects_to_rect" => result(geometry::objects_to_rect(&objs(&args[1])), |o| to_value(&PyValue::Dict(o))),
            "get_bbox_overlap" => {
                result(geometry::get_bbox_overlap(&bbox(&args[1]), &bbox(&args[2])), |b| b.map_or(Value::None, |b| bbox_v(&b)))
            }
            "calculate_area" => result(geometry::calculate_area(&bbox(&args[1])), |v| to_value(&v)),
            "clip_obj" => opt_obj(geometry::clip_obj(&obj(&args[1]), &bbox(&args[2]))),
            "intersects_bbox" => objs_v(geometry::intersects_bbox(&objs(&args[1]), &bbox(&args[2]))),
            "within_bbox" => objs_v(geometry::within_bbox(&objs(&args[1]), &bbox(&args[2]))),
            "outside_bbox" => objs_v(geometry::outside_bbox(&objs(&args[1]), &bbox(&args[2]))),
            "crop_to_bbox" => objs_v(geometry::crop_to_bbox(&objs(&args[1]), &bbox(&args[2]))),
            "move_object" => {
                let axis = if args[2].as_str() == Some("h") { Axis::H } else { Axis::V };
                result(geometry::move_object(&obj(&args[1]), axis, &py(&args[3])), |o| to_value(&PyValue::Dict(o)))
            }
            "snap_objects" => objs_v(geometry::snap_objects(&objs(&args[1]), snap_attr(&args[2]), &py(&args[3]))),
            "resize_object" => result(geometry::resize_object(&obj(&args[1]), snap_attr(&args[2]), &py(&args[3])), |o| {
                to_value(&PyValue::Dict(o))
            }),
            "curve_to_edges" => objs_v(geometry::curve_to_edges(&obj(&args[1]))),
            "rect_to_edges" => objs_v(geometry::rect_to_edges(&obj(&args[1]))),
            "line_to_edge" => result(geometry::line_to_edge(&obj(&args[1])), |o| to_value(&PyValue::Dict(o))),
            "obj_to_edges" => objs_v(geometry::obj_to_edges(&obj(&args[1]))),
            "filter_edges" => {
                let edge_type = py(&args[3]);
                objs_v(geometry::filter_edges(
                    &objs(&args[1]),
                    args[2].as_str(),
                    if edge_type.is_none() { None } else { Some(&edge_type) },
                    &py(&args[4]),
                ))
            }
            other => panic!("{other}"),
        };
        c.check(|| format!("case {i}: {name}"), &got, &out);
    });
    c.finish(3000);
}

// -- text lines -------------------------------------------------------------------

#[test]
fn textline_cases() {
    let data = data();
    let mut c = Checker::new("textlines");
    for_each_case(&data, "textlines", |i, args, out| {
        let s: Vec<char> = args[0].as_str().expect("a str").chars().collect();
        let strip = matches!(args[1], Value::Bool(true));
        let got = Value::List(text_line_matches(&s, strip).iter().map(spans_value).collect());
        c.check(|| format!("case {i}: {:?} strip={strip}", args[0]), &got, &out);
    });
    c.finish(5000);
}

fn spans_value(m: &RegexMatch) -> Value {
    let int = |k: usize| Value::Int(pycompat::PyInt::Small(k as i64));
    Value::List(m.spans.iter().map(|s| s.map_or(Value::None, |(a, b)| Value::Tuple(vec![int(a), int(b)]))).collect())
}

// -- table settings -----------------------------------------------------------------

fn settings_value(s: &TableSettings) -> Value {
    let r = |v: &Option<PyValue>| to_value(v.as_ref().expect("resolved"));
    Value::str_dict([
        ("vertical_strategy".to_owned(), to_value(&s.vertical_strategy)),
        ("horizontal_strategy".to_owned(), to_value(&s.horizontal_strategy)),
        ("explicit_vertical_lines".to_owned(), to_value(&s.explicit_vertical_lines)),
        ("explicit_horizontal_lines".to_owned(), to_value(&s.explicit_horizontal_lines)),
        ("snap_tolerance".to_owned(), to_value(&s.snap_tolerance)),
        ("snap_x_tolerance".to_owned(), r(&s.snap_x_tolerance)),
        ("snap_y_tolerance".to_owned(), r(&s.snap_y_tolerance)),
        ("join_tolerance".to_owned(), to_value(&s.join_tolerance)),
        ("join_x_tolerance".to_owned(), r(&s.join_x_tolerance)),
        ("join_y_tolerance".to_owned(), r(&s.join_y_tolerance)),
        ("edge_min_length".to_owned(), to_value(&s.edge_min_length)),
        ("edge_min_length_prefilter".to_owned(), to_value(&s.edge_min_length_prefilter)),
        ("min_words_vertical".to_owned(), to_value(&s.min_words_vertical)),
        ("min_words_horizontal".to_owned(), to_value(&s.min_words_horizontal)),
        ("intersection_tolerance".to_owned(), to_value(&s.intersection_tolerance)),
        ("intersection_x_tolerance".to_owned(), r(&s.intersection_x_tolerance)),
        ("intersection_y_tolerance".to_owned(), r(&s.intersection_y_tolerance)),
        ("text_settings".to_owned(), to_value(&s.text_settings)),
    ])
}

#[test]
fn settings_cases() {
    let data = data();
    let mut c = Checker::new("settings");
    for_each_case(&data, "settings", |i, args, out| {
        let settings = match &args[0] {
            Value::None => None,
            v => Some(obj(v)),
        };
        let got = result(TableSettings::resolve(settings.as_ref()), |s| settings_value(&s));
        c.check(|| format!("case {i}"), &got, &out);
    });
    c.finish(1000);
}

// -- pages ------------------------------------------------------------------------

/// The page of a case: `page.chars`, `page.edges`, `page.bbox`.
struct CasePage {
    chars: Vec<Obj>,
    edges: Vec<Obj>,
    bbox: Bbox,
}

impl TablePage for CasePage {
    fn chars(&self) -> PyResult<Vec<Obj>> {
        Ok(self.chars.clone())
    }
    fn edges(&self) -> PyResult<Vec<Obj>> {
        Ok(self.edges.clone())
    }
    fn bbox(&self) -> PyResult<Bbox> {
        Ok(self.bbox.clone())
    }
}

/// `tuplify_list_kwargs` (pdfplumber/page.py:103)
fn tuplify_list_kwargs(kwargs: &Kwargs) -> Kwargs {
    Obj::from_pairs(kwargs.items().iter().map(|(k, v)| {
        let v = match v {
            PyValue::List(items) => PyValue::Tuple(items.clone()),
            other => other.clone(),
        };
        (k.clone(), v)
    }))
}

impl CasePage {
    /// pdfplumber/page.py:498 Page._get_textmap (through `get_textmap`,
    /// with `tuplify_list_kwargs`)
    fn get_textmap(&self, kwargs: &Kwargs) -> PyResult<TextMap> {
        let kwargs = tuplify_list_kwargs(kwargs);
        let mut full = Obj::from_pairs([("layout_bbox", bbox_value(&self.bbox))]);
        if !kwargs.contains("layout_width_chars") {
            full.set("layout_width", self.bbox[2].sub(&self.bbox[0])?);
        }
        if !kwargs.contains("layout_height_chars") {
            full.set("layout_height", self.bbox[3].sub(&self.bbox[1])?);
        }
        full.update(kwargs.items().iter().map(|(k, v)| (k.clone(), v.clone())));
        chars_to_textmap(&self.chars, &full)
    }

    /// pdfplumber/page.py:458 Page.find_tables
    fn find_tables(&self, tset: &TableSettings) -> PyResult<Vec<Table>> {
        Ok(TableFinder::new(self, tset.clone())?.tables)
    }

    /// pdfplumber/page.py:464 Page.find_table
    fn find_table(&self, tset: &TableSettings) -> PyResult<Option<Table>> {
        let mut tables = self.find_tables(tset)?;
        if tables.is_empty() {
            return Ok(None);
        }
        let mut keyed = Vec::new();
        for t in tables.drain(..) {
            let b = t.bbox()?;
            keyed.push(((-(t.cells.len() as i64), b[1].clone(), b[0].clone()), t));
        }
        let key_lt = |a: &(i64, PyValue, PyValue), b: &(i64, PyValue, PyValue)| -> PyResult<bool> {
            if a.0 != b.0 {
                return Ok(a.0 < b.0);
            }
            if !a.1.py_eq(&b.1) {
                return a.1.lt(&b.1);
            }
            a.2.lt(&b.2)
        };
        pycompat::listsort::sort_by_key(&mut keyed, |(k, _)| Ok(k.clone()), |a, b| key_lt(a, b))?;
        Ok(Some(keyed.swap_remove(0).1))
    }
}

fn table_text(rows: Vec<Vec<Option<String>>>) -> Value {
    Value::List(
        rows.into_iter()
            .map(|r| Value::List(r.into_iter().map(|c| c.map_or(Value::None, Value::str)).collect()))
            .collect(),
    )
}

fn cell_group_value(g: &CellGroup) -> Value {
    Value::Tuple(vec![
        Value::List(g.cells.iter().map(|c| c.as_ref().map_or(Value::None, bbox_v)).collect()),
        bbox_v(&g.bbox),
    ])
}

/// `describe_finder` of the generator.
fn describe_finder(f: &TableFinder) -> PyResult<Value> {
    let bboxes = |edges: &[Obj]| -> PyResult<Value> {
        Ok(Value::List(edges.iter().map(|e| Ok(bbox_v(&geometry::obj_to_bbox(e)?))).collect::<PyResult<_>>()?))
    };
    let mut intersections = Vec::new();
    for ((x, y), v, h) in &f.intersections.points {
        intersections.push(Value::Tuple(vec![Value::Tuple(vec![to_value(x), to_value(y)]), bboxes(v)?, bboxes(h)?]));
    }
    let mut tables = Vec::new();
    for t in &f.tables {
        tables.push(Value::str_dict([
            ("bbox".to_owned(), bbox_v(&t.bbox()?)),
            ("cells".to_owned(), Value::List(t.cells.iter().map(bbox_v).collect())),
            ("rows".to_owned(), Value::List(t.rows()?.iter().map(cell_group_value).collect())),
            ("columns".to_owned(), Value::List(t.columns()?.iter().map(cell_group_value).collect())),
        ]));
    }
    Ok(Value::str_dict([
        ("edges".to_owned(), list_of_objs(f.edges.clone())),
        ("intersections".to_owned(), Value::List(intersections)),
        ("cells".to_owned(), Value::List(f.cells.iter().map(bbox_v).collect())),
        ("tables".to_owned(), Value::List(tables)),
    ]))
}

fn kw(v: &Value) -> Kwargs {
    obj(v)
}

fn truthy(v: &Value) -> bool {
    py(v).truthy()
}

fn settings_arg(v: &Value) -> Option<Obj> {
    match v {
        Value::None => None,
        v => Some(obj(v)),
    }
}

/// One operation of `run_op` (generator), as a value.
fn run_op(page: &CasePage, op: &[Value]) -> Value {
    let name = op[0].as_str().expect("op name");
    let text = |r: PyResult<String>| result(r, Value::str);
    match name {
        "text" => text(page.get_textmap(&kw(&op[1])).map(|tm| tm.as_string)),
        "utext" => text(extract_text_kwargs(&page.chars, &kw(&op[1]))),
        "simple" => {
            let k = kw(&op[1]);
            let tol = |key: &str| k.get(key).cloned().unwrap_or(PyValue::from(3i64));
            text(extract_text_simple(&page.chars, &tol("x_tolerance"), &tol("y_tolerance")))
        }
        "words" => result(extract_words_kwargs(&page.chars, &kw(&op[1])), list_of_objs),
        "lines" => {
            let r = page.get_textmap(&kw(&op[3])).and_then(|tm| tm.extract_text_lines(truthy(&op[1]), truthy(&op[2])));
            result(r, list_of_objs)
        }
        "search" => {
            // ["search", kwargs, main_group, return_groups, return_chars, spans]
            let main_group = match &op[2] {
                Value::Int(i) => i.as_i64().expect("small") as usize,
                other => panic!("{other:?}"),
            };
            let r = page.get_textmap(&kw(&op[1])).and_then(|tm| {
                let matches: Vec<RegexMatch> = match &op[5] {
                    Value::List(ms) => ms
                        .iter()
                        .map(|m| match m {
                            Value::List(spans) => RegexMatch {
                                spans: spans
                                    .iter()
                                    .map(|s| match s {
                                        Value::Tuple(ab) => {
                                            let n = |v: &Value| match v {
                                                Value::Int(i) => i.as_i64().expect("small") as usize,
                                                other => panic!("{other:?}"),
                                            };
                                            Some((n(&ab[0]), n(&ab[1])))
                                        }
                                        Value::None => None,
                                        other => panic!("{other:?}"),
                                    })
                                    .collect(),
                            },
                            other => panic!("{other:?}"),
                        })
                        .collect(),
                    other => panic!("spans of a textmap that did not raise: {other:?}"),
                };
                tm.search(&matches, truthy(&op[3]), truthy(&op[4]), main_group)
            });
            result(r, list_of_objs)
        }
        "dedupe" => {
            let k = kw(&op[1]);
            let tolerance = k.get("tolerance").cloned().unwrap_or(PyValue::from(1i64));
            let extra: Vec<Rc<str>> = match k.get("extra_attrs") {
                None => vec![Rc::from("fontname"), Rc::from("size")],
                Some(v) => v
                    .as_seq()
                    .map(|items| items.iter().map(|x| Rc::from(x.as_str().expect("a str"))).collect())
                    .unwrap_or_default(),
            };
            result(dedupe_chars(&page.chars, &tolerance, &extra), list_of_objs)
        }
        "tables" => {
            let r = TableSettings::resolve(settings_arg(&op[1]).as_ref())
                .and_then(|s| TableFinder::new(page, s))
                .and_then(|f| describe_finder(&f));
            result(r, |v| v)
        }
        "extract_tables" => {
            // pdfplumber/page.py:481 Page.extract_tables
            let r = TableSettings::resolve(settings_arg(&op[1]).as_ref()).and_then(|tset| {
                let tables = page.find_tables(&tset)?;
                tables.iter().map(|t| t.extract(page, &tset.text_settings())).collect::<PyResult<Vec<_>>>()
            });
            result(r, |ts| Value::List(ts.into_iter().map(table_text).collect()))
        }
        "extract_table" => {
            // pdfplumber/page.py:488 Page.extract_table
            let r = TableSettings::resolve(settings_arg(&op[1]).as_ref()).and_then(|tset| match page.find_table(&tset)? {
                None => Ok(None),
                Some(t) => Ok(Some(t.extract(page, &tset.text_settings())?)),
            });
            result(r, |t| t.map_or(Value::None, table_text))
        }
        other => panic!("unknown op {other}"),
    }
}

fn replay_pages(name: &'static str, min_pages: usize) {
    let data = data();
    let mut c = Checker::new(name);
    let pages = for_each_case(&data, name, |i, args, out| {
        let record = obj(&args[0]);
        let get = |k: &str| record.get(k).unwrap_or_else(|| panic!("record without {k}"));
        let as_objs = |v: &PyValue| match v {
            PyValue::List(items) => items
                .iter()
                .map(|x| match x {
                    PyValue::Dict(d) => d.clone(),
                    other => panic!("{other:?}"),
                })
                .collect::<Vec<Obj>>(),
            other => panic!("{other:?}"),
        };
        let page = CasePage {
            chars: as_objs(get("chars")),
            edges: as_objs(get("edges")),
            bbox: bbox_from_value(get("bbox")).expect("a bbox"),
        };
        let (Value::List(ops), Value::List(outs)) = (&args[1], &out) else { panic!("ops/outs") };
        assert_eq!(ops.len(), outs.len());
        for (k, (op, want)) in ops.iter().zip(outs.iter()).enumerate() {
            let Value::List(op) = op else { panic!("{op:?}") };
            let got = run_op(&page, op);
            let want = want.as_str().expect("a canon text");
            c.check_text(|| format!("{name}:{i}:{k} {}", canon(&op[0])), &canon(&got), want);
        }
    });
    assert!(pages >= min_pages, "{name}: only {pages} pages");
    c.finish(min_pages);
}

#[test]
fn pages_fuzz() {
    replay_pages("pages_fuzz", 300);
}

#[test]
fn pages_real() {
    replay_pages("pages_real", 100);
}

/// `cells_to_tables` and `Table` on a hand-made grid share the code paths
/// the page cases exercise; this only checks the plumbing of the helpers.
#[test]
fn table_helpers_smoke() {
    let n = |x: i64| PyValue::from(x);
    let cells = vec![[n(0), n(0), n(1), n(1)], [n(1), n(0), n(2), n(1)]];
    let tables = cells_to_tables(&cells).unwrap();
    assert_eq!(tables.len(), 1);
    let t = Table::new(tables[0].clone());
    assert_eq!(t.rows().unwrap().len(), 1);
    assert!(exc::ValueError("x").message == "x");
}
