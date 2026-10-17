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
//!
//! `HashKey` holds `Rc`-shared values (no interior mutability reaches its
//! hash), hence the allowed `mutable_key_type`.
#![allow(clippy::mutable_key_type)]

use std::fs::File;
use std::io::{BufRead, BufReader};

use pdfplumber::obj::{bbox_from_value, bbox_value, Bbox, Obj, PyValue};
use pdfplumber::table::{
    self, cells_to_tables, CellGroup, Intersections, Table, TableFinder, TablePage, TableSettings,
};
use pdfplumber::utils::clustering::{cluster_list, cluster_objects, make_cluster_dict};
use pdfplumber::utils::geometry::{self, Axis, SnapAttr};
use pdfplumber::utils::text::{
    chars_to_textmap, dedupe_chars, extract_text_kwargs, extract_text_simple, extract_words_kwargs, text_line_matches,
    Kwargs, RegexMatch, TextMap, TextMapOptions, WordExtractor,
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
        // The same sort through sort_by_key (keys computed first, then the
        // items moved by the permutation).
        let mut items: Vec<usize> = (0..xs.len()).collect();
        let r = pycompat::listsort::sort_by_key(&mut items, |&k| Ok(xs[k].clone()), |a, b| a.lt(b));
        let got = result(r, |()| {
            Value::List(items.into_iter().map(|k| Value::Int(pycompat::PyInt::Small(k as i64))).collect())
        });
        c.check(|| format!("case {i} (sort_by_key)"), &got, &out);
    });
    c.finish(1000);
}

// -- operators --------------------------------------------------------------------

/// One operator of the generator's `ops_cases` on two values.
fn run_value_op(op: &str, a: &PyValue, b: &PyValue) -> Value {
    let v = |r: PyResult<PyValue>| result(r, |v| to_value(&v));
    match op {
        "add" => v(a.add(b)),
        "sub" => v(a.sub(b)),
        "mul" => v(a.mul(b)),
        "truediv" => v(a.truediv(b)),
        "lt" => v(a.lt(b).map(PyValue::Bool)),
        "le" => v(a.le(b).map(PyValue::Bool)),
        "gt" => v(a.gt(b).map(PyValue::Bool)),
        "ge" => v(a.ge(b).map(PyValue::Bool)),
        "eq" => v(Ok(PyValue::Bool(a.py_eq(b)))),
        "neg" => v(a.neg()),
        "abs" => v(a.abs()),
        "repr" => v(Ok(PyValue::from(a.repr()))),
        "bool" => v(Ok(PyValue::Bool(a.truthy()))),
        "min" => v(pdfplumber::obj::py_min([a, b])),
        "max" => v(pdfplumber::obj::py_max([a, b])),
        "sum" => v(pdfplumber::obj::py_sum([a, b])),
        "dictget" => v((|| {
            let mut d = std::collections::HashMap::new();
            d.insert(pdfplumber::obj::HashKey::new(a.clone())?, PyValue::str("found"));
            Ok(d.get(&pdfplumber::obj::HashKey::new(b.clone())?).cloned().unwrap_or(PyValue::None))
        })()),
        "unpack4" => v(pdfplumber::obj::bbox_from_value(a).map(|b| pdfplumber::obj::bbox_value(&b))),
        "float" => v(a.to_float()),
        "in" => v(pdfplumber::obj::py_contains(b, a).map(PyValue::Bool)),
        "iter" => v(match pdfplumber::obj::py_iter(a) {
            Some(items) => items.map(PyValue::list),
            None => Err(exc::TypeError(format!("'{}' object is not iterable", a.type_name()))),
        }),
        other => panic!("unknown op {other}"),
    }
}

#[test]
fn ops_cases() {
    let data = data();
    let mut c = Checker::new("ops");
    for_each_case(&data, "ops", |i, args, out| {
        let op = args[0].as_str().expect("op").to_owned();
        let a = py(&args[1]);
        let identical = matches!(args[3], Value::Bool(true));
        let b = if identical { a.clone() } else { py(&args[2]) };
        c.check(|| format!("case {i}: {op} {a:?} {b:?}"), &run_value_op(&op, &a, &b), &out);
        // The same values as PDF objects (pdfminer's objects are Python
        // values: a PDF list is a list): same results. Not for repr (a PDF
        // object has its own), nor for a value compared with itself (a
        // reference has an identity).
        if op == "repr" || identical {
            return;
        }
        let pdf = |x: &Value| pdftrace::plumber::pdf_object(x).ok().map(PyValue::Pdf);
        if let Some(pa) = pdf(&args[1]) {
            c.check(|| format!("case {i}: {op} {pa:?} {b:?} (PDF object)"), &run_value_op(&op, &pa, &b), &out);
        }
        if let Some(pb) = pdf(&args[2]) {
            c.check(|| format!("case {i}: {op} {a:?} {pb:?} (PDF object)"), &run_value_op(&op, &a, &pb), &out);
        }
    });
    c.finish(5000);
}

// -- dicts ------------------------------------------------------------------------

#[test]
fn dict_cases() {
    let data = data();
    let mut c = Checker::new("dicts");
    for_each_case(&data, "dicts", |i, args, out| {
        let Value::List(steps) = &args[0] else { panic!("steps") };
        let mut d = Obj::new();
        let mut other = Obj::new();
        let mut results = Vec::new();
        for step in steps {
            let Value::Tuple(step) = step else { panic!("step") };
            let (op, k, v) = (step[0].as_str().expect("op"), step[1].as_str().expect("key"), py(&step[2]));
            let items = |o: &Obj| {
                PyValue::list(o.items().iter().map(|(k, v)| PyValue::tuple([PyValue::Str(k.clone()), v.clone()])))
            };
            let r: PyResult<PyValue> = match op {
                "set" => {
                    d.set(k, v);
                    Ok(PyValue::None)
                }
                "del" => d.remove(k).map(|_| PyValue::None),
                "get" => Ok(d.get(k).cloned().unwrap_or(PyValue::None)),
                "in" => Ok(PyValue::Bool(d.contains(k))),
                "keys" => Ok(PyValue::list(d.keys().cloned().map(PyValue::Str))),
                "eq" => Ok(PyValue::Bool(d.py_eq(&other))),
                "copyset" => {
                    let mut copy = d.clone();
                    copy.set(k, v);
                    Ok(PyValue::tuple([items(&copy), items(&d)]))
                }
                other => panic!("unknown dict op {other}"),
            };
            results.push(result(r, |v| to_value(&v)));
            if op == "eq" {
                other = d.clone();
            }
        }
        c.check(|| format!("case {i}"), &Value::List(results), &out);
    });
    c.finish(1000);
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
            // key "v" (itemgetter: the stored object), or lambda o: -o["v"]
            // (a new object)
            "cluster_objects" | "cluster_objects_neg" => {
                let xs = objs(&args[1]);
                let preserve = matches!(args[3], Value::Bool(true));
                let r = if name == "cluster_objects" {
                    cluster_objects(&xs, |o| Ok(o.item("v")?.clone()), &py(&args[2]), preserve)
                } else {
                    cluster_objects(&xs, |o| o.item("v")?.neg(), &py(&args[2]), preserve)
                };
                result(r, |groups| Value::List(groups.into_iter().map(list_of_objs).collect()))
            }
            // list(make_cluster_dict(values, tolerance).items())
            "make_cluster_dict" => {
                let values = match &args[1] {
                    Value::List(items) => items.iter().map(py).collect::<Vec<_>>(),
                    other => panic!("{other:?}"),
                };
                result(make_cluster_dict(values.into_iter().map(Ok), &py(&args[2])), |items| {
                    Value::List(
                        items
                            .iter()
                            .map(|(v, i)| Value::Tuple(vec![to_value(v), to_value(&PyValue::from(*i as i64))]))
                            .collect(),
                    )
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
            "get_bbox_overlap" => result(geometry::get_bbox_overlap(&bbox(&args[1]), &bbox(&args[2])), |b| {
                b.map_or(Value::None, |b| bbox_v(&b))
            }),
            "calculate_area" => result(geometry::calculate_area(&py(&args[1])), |v| to_value(&v)),
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
            "resize_object" => {
                result(geometry::resize_object(&obj(&args[1]), snap_attr(&args[2]), &py(&args[3])), |o| {
                    to_value(&PyValue::Dict(o))
                })
            }
            "curve_to_edges" => objs_v(geometry::curve_to_edges(&obj(&args[1]))),
            "rect_to_edges" => objs_v(geometry::rect_to_edges(&obj(&args[1]))),
            "line_to_edge" => result(geometry::line_to_edge(&obj(&args[1])), |o| to_value(&PyValue::Dict(o))),
            "obj_to_edges" => objs_v(geometry::obj_to_edges(&obj(&args[1]))),
            "snap_edges" => objs_v(pdfplumber::table::snap_edges(&objs(&args[1]), &py(&args[2]), &py(&args[3]))),
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

// -- table.py functions -----------------------------------------------------------

fn intersections_v(i: &Intersections) -> Value {
    Value::List(
        i.points
            .iter()
            .map(|((x, y), v, h)| {
                Value::Tuple(vec![
                    Value::Tuple(vec![to_value(x), to_value(y)]),
                    list_of_objs(v.clone()),
                    list_of_objs(h.clone()),
                ])
            })
            .collect(),
    )
}

#[test]
fn table_cases() {
    let data = data();
    let mut c = Checker::new("tables");
    for_each_case(&data, "tables", |i, args, out| {
        let name = args[0].as_str().expect("fn").to_owned();
        let got = match name.as_str() {
            "edges_to_intersections" => {
                result(table::edges_to_intersections(&objs(&args[1]), &py(&args[2]), &py(&args[3])), |i| {
                    intersections_v(&i)
                })
            }
            "merge_edges" => result(
                table::merge_edges(objs(&args[1]), &py(&args[2]), &py(&args[3]), &py(&args[4]), &py(&args[5])),
                list_of_objs,
            ),
            "table" => {
                let Value::List(cells) = &args[1] else { panic!("cells") };
                let t = Table::new(cells.iter().map(|c| bbox_from_value(&py(c)).expect("a bbox")).collect());
                let groups = if args[2].as_str() == Some("rows") { t.rows() } else { t.columns() };
                Value::List(vec![
                    result(t.bbox(), |b| bbox_v(&b)),
                    result(groups, |g| Value::List(g.iter().map(cell_group_value).collect())),
                ])
            }
            other => panic!("{other}"),
        };
        c.check(|| format!("case {i}: {name}"), &got, &out);
    });
    c.finish(1500);
}

// -- text lines -------------------------------------------------------------------

#[test]
fn textline_cases() {
    let data = data();
    let mut c = Checker::new("textlines");
    for_each_case(&data, "textlines", |i, args, out| {
        let Value::Str(s) = &args[0] else { panic!("a str") };
        let s: Vec<u32> = pycompat::PyStr::from(s).code_points().collect();
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
        if args[0].as_str() == Some("new") {
            let got = result(TableSettings::new(&obj(&args[1])), |s| settings_value(&s));
            c.check(|| format!("case {i}: new {:?}", args[1]), &got, &out);
            return;
        }
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

/// pdfplumber/page.py:103 tuplify_list_kwargs
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
        // get_textmap is an lru_cache: its key hashes every value
        for (_, v) in kwargs.items() {
            pdfplumber::obj::check_hashable(v)?;
        }
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

fn table_text(rows: Vec<Vec<Option<pycompat::PyStr>>>) -> Value {
    Value::List(
        rows.into_iter()
            .map(|r| Value::List(r.into_iter().map(|c| c.map_or(Value::None, |s| Value::py_str(&s))).collect()))
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
    let text = |r: PyResult<pycompat::PyStr>| result(r, |s| Value::py_str(&s));
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
            let extra = match k.get("extra_attrs") {
                None => PyValue::tuple([PyValue::str("fontname"), PyValue::str("size")]),
                Some(v) => v.clone(),
            };
            result(dedupe_chars(&page.chars, &tolerance, &extra), list_of_objs)
        }
        "tables" => {
            let r = TableSettings::resolve(settings_arg(&op[1]).as_ref())
                .and_then(|s| TableFinder::new(page, s))
                .and_then(|f| describe_finder(&f));
            result(r, |v| v)
        }
        "tables_new" => {
            let r = TableSettings::new(&kw(&op[1]))
                .and_then(|s| TableFinder::new(page, s))
                .and_then(|f| describe_finder(&f));
            result(r, |v| v)
        }
        "extract_tables" | "extract_tables_new" => {
            // pdfplumber/page.py:481 Page.extract_tables (a TableSettings
            // resolves to itself)
            let tset = match name {
                "extract_tables" => TableSettings::resolve(settings_arg(&op[1]).as_ref()),
                _ => TableSettings::new(&kw(&op[1])),
            };
            let r = tset.and_then(|tset| {
                let tables = page.find_tables(&tset)?;
                let kwargs = tset.text_settings_kwargs("pdfplumber.table.Table.extract")?;
                tables.iter().map(|t| t.extract(page, &kwargs)).collect::<PyResult<Vec<_>>>()
            });
            result(r, |ts| Value::List(ts.into_iter().map(table_text).collect()))
        }
        "extract_table" => {
            // pdfplumber/page.py:488 Page.extract_table
            let r = TableSettings::resolve(settings_arg(&op[1]).as_ref()).and_then(|tset| {
                match page.find_table(&tset)? {
                    None => Ok(None),
                    Some(t) => {
                        Ok(Some(t.extract(page, &tset.text_settings_kwargs("pdfplumber.table.Table.extract")?)?))
                    }
                }
            });
            result(r, |t| t.map_or(Value::None, table_text))
        }
        "wordmap" => {
            // WordExtractor(**op[1]).extract_wordmap(chars).to_textmap(**op[2])
            let r = WordExtractor::from_kwargs(&kw(&op[1]), true)
                .and_then(|ex| ex.extract_wordmap(&page.chars))
                .and_then(|wm| wm.to_textmap(&TextMapOptions::from_kwargs(&kw(&op[2]), true)?));
            result(r, |tm| {
                let tuples = tm
                    .tuples
                    .iter()
                    .map(|(t, c)| {
                        let c = c.as_ref().map_or(Value::None, |c| to_value(&PyValue::Dict(c.clone())));
                        let t = pycompat::PyStr::from_code_point(*t);
                        Value::Tuple(vec![Value::py_str(&t), c])
                    })
                    .collect();
                Value::Tuple(vec![Value::py_str(&tm.as_string), Value::List(tuples)])
            })
        }
        other => panic!("unknown op {other}"),
    }
}

/// With `TEXT_TABLES_TIME` set, [`replay_pages`] prints the time spent in
/// each operation (for performance comparisons of the object model).
fn replay_pages(module: &str, name: &'static str, min_pages: usize) {
    let data = UnitData::open(module);
    let mut c = Checker::new(name);
    let timing = std::env::var_os("TEXT_TABLES_TIME").is_some();
    let mut times: Vec<(String, std::time::Duration)> = Vec::new();
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
            let start = std::time::Instant::now();
            let got = run_op(&page, op);
            if timing {
                let op_name = op[0].as_str().expect("op name").to_owned();
                match times.iter_mut().find(|(n, _)| *n == op_name) {
                    Some((_, t)) => *t += start.elapsed(),
                    None => times.push((op_name, start.elapsed())),
                }
            }
            let want = want.as_str().expect("a canon text");
            c.check_text(|| format!("{name}:{i}:{k} {}", canon(&op[0])), &canon(&got), want);
        }
    });
    assert!(pages >= min_pages, "{name}: only {pages} pages");
    if timing {
        let total: std::time::Duration = times.iter().map(|(_, t)| *t).sum();
        eprintln!("{name}: {pages} pages, operations {total:.3?}: {times:.3?}");
    }
    c.finish(min_pages);
}

#[test]
fn pages_fuzz() {
    replay_pages("text_tables", "pages_fuzz", 800);
}

#[test]
fn pages_real() {
    replay_pages("text_tables_real", "pages_real", 150);
}

/// Spans come from the caller's regular expression engine: a span that
/// cannot come from a match over `as_string` is refused (not a Python
/// behavior: Python computes the spans itself).
#[test]
fn search_refuses_spans_outside_the_string() {
    let tm = TextMap::new(vec![(0x61, None), (0x62, None)], &PyValue::str("ttb"), &PyValue::str("ltr")).unwrap();
    for span in [(0, 3), (2, 1), (3, 3)] {
        let m = RegexMatch { spans: vec![Some(span)] };
        let e = tm.search(&[m], true, true, 0).unwrap_err();
        assert_eq!(e.message, "match span outside TextMap.as_string", "{span:?}");
    }
    for span in [(0, 0), (2, 2), (1, 1)] {
        let m = RegexMatch { spans: vec![Some(span)] };
        assert!(tm.search(&[m], true, true, 0).unwrap().is_empty(), "{span:?}");
    }
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

/// The address-space limit of sequence repetition: a result beyond
/// `MAX_REPEAT_BYTES` (4 GiB of the port's representation, the wasm32
/// address space) raises `MemoryError` where CPython on the oracle machine
/// would allocate it (no oracle case: the generator leaves these out, see
/// `architecture.md`). Smaller results are built as in Python (oracle
/// cases: a 20 MB `str` in `pages_fuzz`, `user_arg_cases`); beyond
/// `PY_SSIZE_T_MAX`, CPython's own errors are oracle cases (`ops`).
#[test]
fn sequence_repeat_address_space_limit() {
    let limit = pdfplumber::obj::MAX_REPEAT_BYTES as i64;
    let item = std::mem::size_of::<PyValue>() as i64;
    for (seq, n) in [
        (PyValue::str("a"), limit + 1),
        (PyValue::str("ab"), limit / 2 + 1),
        (PyValue::str("\u{e9}"), limit / 2 + 1), // two UTF-8 bytes, one code point
        (PyValue::Bytes(std::rc::Rc::from(&b"a"[..])), limit + 1),
        (PyValue::tuple([PyValue::None]), limit / item + 1),
        (PyValue::list([PyValue::None, PyValue::None]), limit / item / 2 + 1),
    ] {
        let n = PyValue::from(n);
        let e = seq.mul(&n).unwrap_err();
        assert_eq!((e.kind, e.message.as_str()), (pycompat::ExcKind::MemoryError, ""), "{seq:?}");
        let e = n.mul(&seq).unwrap_err();
        assert_eq!(e.kind, pycompat::ExcKind::MemoryError);
    }
    let ok = PyValue::str("ab").mul(&PyValue::from(10_000_000i64)).unwrap();
    assert_eq!(ok.as_str().map(str::len), Some(20_000_000));
    let ok = PyValue::tuple([PyValue::None]).mul(&PyValue::from(10_000_001i64)).unwrap();
    assert_eq!(ok.as_seq().map(<[PyValue]>::len), Some(10_000_001));
    drop(ok);
    // a str or bytes item is one byte (not a PyValue): one more item than a
    // tuple may hold is built (about 180 MB, freed at once)
    let n = limit / item + 1;
    let ok = PyValue::str("a").mul(&PyValue::from(n)).unwrap();
    assert_eq!(ok.as_pystr().map(pycompat::PyStr::len), Some(n as usize));
    drop(ok);
    let ok = PyValue::Bytes(std::rc::Rc::from(&b"a"[..])).mul(&PyValue::from(n)).unwrap();
    assert!(matches!(&ok, PyValue::Bytes(b) if b.len() == n as usize));
}

/// The resource guard of layout text: `for i in range(n)` that appends a
/// line per iteration, with `n` beyond `MAX_REPEAT` (10**7) or beyond 64
/// bits, raises `MemoryError` where Python would run until memory is
/// exhausted (no oracle case can exist). A huge *negative* `n` is an empty
/// range (oracle cases in `pages_fuzz`).
#[test]
fn layout_range_resource_guard() {
    let big = |s: &str| PyValue::Int(pycompat::PyInt::from_decimal(s).expect("an int"));
    let bbox = PyValue::tuple([0i64, 0, 612, 792].map(PyValue::from));
    let n = |x: i64| PyValue::from(x);
    let chars = [Obj::from_pairs([
        ("text", PyValue::str("a")),
        ("x0", n(10)),
        ("x1", n(15)),
        ("top", n(20)),
        ("bottom", n(30)),
        ("doctop", n(20)),
        ("upright", PyValue::from(true)),
        ("size", n(10)),
        ("fontname", PyValue::str("F")),
    ])];
    for n in [big("1180591620717411303424"), PyValue::from(100_000_000i64)] {
        let kwargs = Obj::from_pairs([
            ("layout", PyValue::from(true)),
            ("layout_bbox", bbox.clone()),
            ("layout_height_chars", n.clone()),
        ]);
        let e = chars_to_textmap(&chars, &kwargs).unwrap_err();
        assert_eq!((e.kind, e.message.as_str()), (pycompat::ExcKind::MemoryError, ""), "{n:?}");
    }
    let kwargs = Obj::from_pairs([
        ("layout", PyValue::from(true)),
        ("layout_bbox", bbox),
        ("layout_height_chars", PyValue::from(3i64)),
    ]);
    assert_eq!(chars_to_textmap(&chars, &kwargs).unwrap().as_string, "\n\n a"); // Python: chars_to_textmap([c], layout=True, ...)
}

/// `edges_to_intersections` never makes a vertex holding a NaN, whatever
/// coordinate of an edge is a NaN and whatever the tolerances (a NaN
/// included): each coordinate of a vertex takes part in a comparison that a
/// NaN makes false. So `Table.rows`/`Table.columns` never look a NaN cell
/// coordinate up through `TableFinder` (see `edges_to_intersections`).
#[test]
fn no_nan_vertex() {
    let n = |x: f64| PyValue::from(x);
    let coords = ["x0", "x1", "top", "bottom"];
    let mut seen = 0;
    for nan_mask in 0u32..256 {
        let mut edges = Vec::new();
        for (k, orientation) in ["v", "h"].into_iter().enumerate() {
            let base = [10.0, 10.0, 0.0, 100.0];
            let base = if orientation == "h" { [0.0, 100.0, 50.0, 50.0] } else { base };
            let mut e = Obj::from_pairs([("orientation", PyValue::str(orientation))]);
            for (i, c) in coords.iter().enumerate() {
                let nan = nan_mask & (1 << (4 * k + i)) != 0;
                e.set(*c, if nan { n(f64::NAN) } else { n(base[i]) });
            }
            edges.push(e);
        }
        for tol in [0.0, 3.0, f64::INFINITY, f64::NAN] {
            let inter = table::edges_to_intersections(&edges, &n(tol), &n(tol)).expect("numbers");
            for ((x, y), _, _) in &inter.points {
                assert!(x.py_eq(x) && y.py_eq(y), "NaN vertex: mask {nan_mask:#x}, tolerance {tol}");
                seen += 1;
            }
        }
    }
    assert!(seen > 0, "some masks give vertices");
}

/// A NaN that comes from outside (a JavaScript number: `f64::NAN` or any
/// payload) is a new object, as the values of a decoder are (`obj.rs`,
/// "NaN identity"; `PyFloat`'s private field leaves no other way to make
/// a `float`): two decoded NaNs are two objects (distinct clusters, `[a]
/// == [b]` false, as the oracle's `nan_cluster_cases` check for distinct
/// NaN objects), and each is found again as itself (one cluster, `[a] ==
/// [a]`), whatever the bits it arrived with.
#[test]
fn outside_nans_are_new_objects() {
    let item = |v: PyValue| Obj::from_pairs([("v", v)]);
    let sizes = |a: PyValue, b: PyValue| -> Vec<usize> {
        let xs = [item(a), item(b)];
        let clusters =
            cluster_objects(&xs, |o: &Obj| Ok(o.get("v").expect("v").clone()), &PyValue::from(0i64), false).unwrap();
        clusters.iter().map(Vec::len).collect()
    };
    let same = |a: &PyValue, b: &PyValue| PyValue::list([a.clone()]).py_eq(&PyValue::list([b.clone()]));
    let outside_bits =
        [f64::NAN, -f64::NAN, f64::from_bits(0x7ff8_0000_0000_0001), f64::from_bits(0x7ff4_0000_0000_0000)];
    for x in outside_bits {
        let payload = x.to_bits() & ((1 << 51) - 1);
        type Decode = fn(f64) -> PyValue;
        let decoders: [(Decode, bool); 3] =
            [(PyValue::from, false), (PyValue::float, false), (PyValue::float_with_identity, true)];
        for (decode, keeps_payload) in decoders {
            let (a, b) = (decode(x), decode(x));
            // (float_with_identity keeps the identity of a test case's NaN:
            // a payload without the port's own bit 50)
            if !keeps_payload || payload == 0 || payload & (1 << 50) != 0 {
                // a NaN without an identity payload: two new objects
                assert_eq!(sizes(a.clone(), b.clone()), [1, 1], "{:#x}", x.to_bits());
                assert!(!same(&a, &b));
            }
            assert_eq!(sizes(a.clone(), a.clone()), [2], "{:#x}", x.to_bits());
            assert!(same(&a, &a) && a.eq_bool(&a.clone()));
        }
    }
}

/// A dict key given as `&str`, `String`, `Rc<str>` or `PyStr` (with or
/// without a surrogate) is the same `str` key: set by one, found by the
/// others, hot (`x0`, `text`) or not. (The port's key types, which the
/// oracle cases reach only through `&str` and `PyStr`.)
#[test]
fn key_kinds() {
    use pycompat::PyStr;
    use std::rc::Rc;
    for name in ["x0", "fontname", "text"] {
        let as_string = String::from(name);
        let as_rc: Rc<str> = Rc::from(name);
        let as_pystr = PyStr::new(name);
        let mut o = Obj::from_pairs([("k", PyValue::None)]);
        o.set(&as_string, PyValue::from(1i64));
        assert!(o.get(name).is_some() && o.get(&as_rc).is_some() && o.get(&as_pystr).is_some());
        o.set(&as_rc, PyValue::from(2i64));
        o.set(&as_pystr, PyValue::from(3i64));
        assert_eq!(o.keys().count(), 2, "{name}");
        assert!(o.get(&as_string).unwrap().py_eq(&PyValue::from(3i64)));
        assert!(o.get("x1").is_none() && o.get(&String::from("x")).is_none() && o.get(&Rc::<str>::from("y")).is_none());
        assert!(o.get(&Rc::<str>::from("k")).is_some() && o.get(&String::from("k")).is_some());
        assert_eq!(*o.keys().nth(1).unwrap(), as_pystr);
    }
    let surrogate = PyStr::from_code_points([0x61, 0xd800]).unwrap();
    let mut o = Obj::from_pairs([("a", PyValue::None)]);
    o.set(&surrogate, PyValue::from(1i64));
    assert!(o.get(&surrogate).is_some() && o.get("a").is_some() && o.get(&String::from("a\u{fffd}")).is_none());
    assert_eq!(o.item(&surrogate.clone()).unwrap().repr(), "1");
}

/// `{:?}` of a value or a dict is its Python `repr` (the replay's mismatch
/// messages and the port's panics print it); the reprs themselves are
/// oracle cases (`ops`).
#[test]
fn debug_is_repr() {
    let v = PyValue::tuple([PyValue::from(1i64), PyValue::from(0.5), PyValue::str("a")]);
    assert_eq!(format!("{v:?}"), v.repr());
    let o = Obj::from_pairs([("x0", PyValue::from(1.0)), ("k", v.clone())]);
    assert_eq!(format!("{o:?}"), PyValue::Dict(o.clone()).repr());
    assert_eq!(format!("{o:?}"), "{'x0': 1.0, 'k': (1, 0.5, 'a')}");
    // the From conversions of the API
    assert_eq!(PyValue::from("a").repr(), "'a'");
    assert_eq!(PyValue::from(pycompat::PyStr::from_code_points([0x61, 0xd800]).unwrap()).repr(), "'a\\ud800'");
    assert_eq!(PyValue::from(String::from("b")).repr(), "'b'");
    assert!(matches!(PyValue::from(o.clone()), PyValue::Dict(d) if d.ptr_eq(&o)));
}
