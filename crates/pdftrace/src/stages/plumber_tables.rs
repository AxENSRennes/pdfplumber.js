//! Stage `plumber_tables`: the table finder for the `lines`,
//! `lines_strict`, `text` and `explicit` strategies and setting variants,
//! on the page and on derived pages (`debug_tablefinder`, `find_table`,
//! `extract_tables`, `extract_table`). The traversal and the line format
//! are those of `oracle/stages/plumber_tables.py`.
//!
//! `page.edges()`, `horizontal_edges()`, `vertical_edges()` (digests), then
//! the ops of `plumber_ops::TABLES_BASE_OPS` and a `select` of
//! `TABLES_VARIANT_OPS` (`TABLES_VARIANTS_PER_PAGE`), the settings of an
//! `explicit` op built from the page (`settings_of`), each through
//! `Page::debug_tablefinder` (`TableFinder`: edges, intersections, cells,
//! tables with their rows, columns and `Table::extract`), `find_table`,
//! `extract_tables`, `extract_table`; then one of `TABLES_DERIVED_OPS` on
//! its derived page (`plumber_text::derive`). The Python stage's
//! generators and helpers (`op_lines`, `settings_of`, `finder_lines`) take
//! a frame each.

use std::io::{self, Write};
use std::rc::Rc;

use pdfplumber::container::Container;
use pdfplumber::obj::{bbox_from_value, Bbox, Obj, PyValue};
use pdfplumber::page::Page;
use pdfplumber::table::TableSettings;
use pdfplumber::utils::geometry::obj_to_bbox;
use pycompat::recursion::py_frame;
use pycompat::{PyResult, PyStr};

use super::common::{terminated, Lines};
use super::plumber_common::{bounded, digest, exc, open_pdf, select, table};
use super::plumber_ops::{TABLES_BASE_OPS, TABLES_DERIVED_OPS, TABLES_VARIANTS_PER_PAGE, TABLES_VARIANT_OPS};
use super::plumber_text::{derive, kwargs_of};
use super::{Stage, StageInput};
use crate::canon::{canon, sha256_hex};
use crate::plumber::{obj_value, to_value};
use crate::value::Value;

pub const STAGE: Stage = Stage { name: "plumber_tables", run: Some(run), oracle_input: false };

fn bbox_v(b: &Bbox) -> Value {
    Value::Tuple(b.iter().map(to_value).collect())
}

fn objs_v(objs: &[Obj]) -> Value {
    Value::List(objs.iter().map(obj_value).collect())
}

fn opt_str(s: &Option<PyStr>) -> Value {
    s.as_ref().map_or(Value::None, Value::py_str)
}

fn rows_v(rows: &[Vec<Option<PyStr>>]) -> Value {
    Value::List(rows.iter().map(|r| Value::List(r.iter().map(opt_str).collect())).collect())
}

/// `settings_of(page, op)`: a frame.
pub fn settings_of(page: &Page, op: &[Value]) -> PyResult<pdfplumber::utils::text::Kwargs> {
    let _frame = py_frame()?;
    let name = op[0].as_str().expect("an op name");
    let settings = kwargs_of(&op[1]);
    if name == "settings" {
        return Ok(settings);
    }
    let mut out = pdfplumber::utils::text::Kwargs::new();
    for (k, v) in settings.items() {
        if !["v", "h", "v_objects", "h_objects"].iter().any(|n| *k == **n) {
            out.set(k, v.clone());
        }
    }
    let [x0, top, x1, bottom] = bbox_from_value(&page.bbox)?;
    let objects = page.objects()?;
    let mut objs: Vec<PyValue> = Vec::new();
    for kind in ["rect", "line", "curve"] {
        objs.extend(objects.get_or_empty(kind).into_iter().map(PyValue::Dict));
    }
    let lines_of = |fracs: &PyValue, n: &PyValue, origin: &PyValue, size: &PyValue| -> PyResult<PyValue> {
        let mut out = Vec::new();
        for f in fracs.as_seq().expect("fractions") {
            out.push(origin.add(&size.mul(f)?)?);
        }
        let n = n.as_num().and_then(|n| match n {
            pycompat::Num::Int(i) => i.as_i64(),
            _ => None,
        });
        out.extend(objs.iter().take(n.expect("a count") as usize).cloned());
        Ok(PyValue::List(out.into()))
    };
    let (v, v_objects) = (settings.item("v")?.clone(), settings.item("v_objects")?.clone());
    if v.truthy() || v_objects.truthy() {
        out.set("explicit_vertical_lines", lines_of(&v, &v_objects, &x0, &x1.sub(&x0)?)?);
    }
    let (h, h_objects) = (settings.item("h")?.clone(), settings.item("h_objects")?.clone());
    if h.truthy() || h_objects.truthy() {
        out.set("explicit_horizontal_lines", lines_of(&h, &h_objects, &top, &bottom.sub(&top)?)?);
    }
    Ok(out)
}

/// `_edge(e)`: `(x0, top, x1, bottom, orientation, object_type)`.
fn edge_v(e: &Obj) -> PyResult<Value> {
    let mut items = Vec::new();
    for k in ["x0", "top", "x1", "bottom", "orientation"] {
        items.push(to_value(e.item(k)?));
    }
    items.push(e.get("object_type").map_or(Value::None, to_value));
    Ok(Value::Tuple(items))
}

/// `finder_lines(page, i, settings)`: a generator (a frame).
fn finder_lines(lines: &mut Lines, page: &Page, i: usize, settings: &pdfplumber::utils::text::Kwargs) -> PyResult<()> {
    let _gen = py_frame()?;
    match page.debug_tablefinder(Some(settings)) {
        Err(e) => lines.push(format!("finder {i} {}", exc(&e))),
        Ok(finder) => {
            let is = i.to_string();
            let edges = finder.edges.iter().map(edge_v).collect::<PyResult<Vec<_>>>()?;
            lines.push(bounded("fedges", &is, &canon(&Value::List(edges))));
            lines.push(format!("fedges_all {i} {} {}", finder.edges.len(), digest(&objs_v(&finder.edges))));
            let mut inters = Vec::new();
            for (pt, v, h) in &finder.intersections.points {
                let bboxes = |es: &[Obj]| -> PyResult<Value> {
                    Ok(Value::List(es.iter().map(|e| obj_to_bbox(e).map(|b| bbox_v(&b))).collect::<PyResult<_>>()?))
                };
                inters.push(Value::Tuple(vec![
                    Value::Tuple(vec![to_value(&pt.0), to_value(&pt.1)]),
                    bboxes(v)?,
                    bboxes(h)?,
                ]));
            }
            lines.push(bounded("intersections", &is, &canon(&Value::List(inters))));
            lines.push(bounded("cells", &is, &canon(&Value::List(finder.cells.iter().map(bbox_v).collect()))));
            let text_settings =
                TableSettings::resolve(Some(settings))?.text_settings_kwargs("pdfplumber.table.Table.extract");
            for (t, table) in finder.tables.iter().enumerate() {
                lines.push(format!("table {i} {t} {} {}", canon(&bbox_v(&table.bbox()?)), table.cells.len()));
                let groups = |gs: Vec<pdfplumber::table::CellGroup>| {
                    Value::List(
                        gs.iter()
                            .map(|g| {
                                Value::Tuple(vec![
                                    Value::List(
                                        g.cells.iter().map(|c| c.as_ref().map_or(Value::None, bbox_v)).collect(),
                                    ),
                                    bbox_v(&g.bbox),
                                ])
                            })
                            .collect(),
                    )
                };
                let it = format!("{i} {t}");
                lines.push(bounded("rows", &it, &canon(&groups(table.rows()?))));
                lines.push(bounded("columns", &it, &canon(&groups(table.columns()?))));
                match text_settings.clone().and_then(|kw| table.extract(page, &kw)) {
                    Err(e) => lines.push(format!("extract {it} {}", exc(&e))),
                    Ok(rows) => lines.push(bounded("extract", &it, &canon(&rows_v(&rows)))),
                }
            }
        }
    }
    match page.find_table(Some(settings)) {
        Err(e) => lines.push(format!("find_table {i} {}", exc(&e))),
        Ok(found) => lines.push(format!(
            "find_table {i} {}",
            canon(&found.map_or(Ok(Value::None), |t| t.bbox().map(|b| bbox_v(&b)))?)
        )),
    }
    match page.extract_tables(Some(settings)) {
        Err(e) => lines.push(format!("extract_tables {i} {}", exc(&e))),
        Ok(tables) => lines.push(bounded(
            "extract_tables",
            &i.to_string(),
            &canon(&Value::List(tables.iter().map(|t| rows_v(t)).collect())),
        )),
    }
    match page.extract_table(Some(settings)) {
        Err(e) => lines.push(format!("extract_table {i} {}", exc(&e))),
        Ok(table) => {
            lines.push(bounded("extract_table", &i.to_string(), &canon(&table.map_or(Value::None, |t| rows_v(&t)))))
        }
    }
    Ok(())
}

/// `op_lines(page, op, counter)`: a generator (a frame).
fn op_lines(lines: &mut Lines, page: &Page, op: &Value, counter: &mut usize) -> PyResult<()> {
    let _gen = py_frame()?;
    let i = *counter;
    *counter += 1;
    lines.push(format!("op {i} {}", canon(op)));
    let Value::List(op) = op else { panic!("op: {op:?}") };
    match settings_of(page, op) {
        Err(e) => {
            lines.push(format!("finder {i} {}", exc(&e)));
            Ok(())
        }
        Ok(settings) => finder_lines(lines, page, i, &settings),
    }
}

fn run(input: &StageInput<'_>, out: &mut dyn Write) -> io::Result<()> {
    let sha = sha256_hex(input.pdf);
    let base_ops = table(TABLES_BASE_OPS);
    let variant_ops = table(TABLES_VARIANT_OPS);
    let derived_ops = table(TABLES_DERIVED_OPS);
    terminated(out, |lines: &mut Lines| {
        let pdf = match open_pdf(input, |_| {}) {
            Ok(pdf) => pdf,
            Err(e) => {
                lines.push(format!("open {}", exc(&e)));
                return Ok(());
            }
        };
        let pages = match pdf.pages() {
            Ok(pages) => pages,
            Err(e) => {
                lines.push(format!("pages {}", exc(&e)));
                return Ok(());
            }
        };
        for (index, page) in pages.iter().enumerate() {
            lines.push(format!("page {}", page.page_number));
            let edges = match Container::edges(&**page) {
                Ok(e) => e,
                Err(e) => {
                    lines.push(format!("edges {}", exc(&e)));
                    continue;
                }
            };
            lines.push(format!("edges {} {}", edges.len(), digest(&objs_v(&edges))));
            let h = Container::horizontal_edges(&**page)?;
            let v = Container::vertical_edges(&**page)?;
            lines.push(format!("hedges {} {}", h.len(), digest(&objs_v(&h))));
            lines.push(format!("vedges {} {}", v.len(), digest(&objs_v(&v))));
            let mut counter = 0;
            let mut ops: Vec<&Value> = base_ops.iter().collect();
            ops.extend(select(&sha, index, &variant_ops, TABLES_VARIANTS_PER_PAGE));
            for op in ops {
                op_lines(lines, page, op, &mut counter)?;
            }
            for entry in select(&sha, index, &derived_ops, 1) {
                let Value::List(entry) = entry else { panic!("derived op: {entry:?}") };
                let (steps, op) = (&entry[0], &entry[1]);
                lines.push(format!("derived {}", canon(steps)));
                let derived: Rc<Page> = match derive(page, steps) {
                    Ok(d) => d,
                    Err(e) => {
                        lines.push(format!("derive {}", exc(&e)));
                        continue;
                    }
                };
                op_lines(lines, &derived, op, &mut counter)?;
            }
        }
        Ok(())
    })
}
