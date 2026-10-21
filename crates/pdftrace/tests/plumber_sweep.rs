//! Replays `oracle/unit/plumber_sweep.py`: operations of pdfplumber's page
//! layer (text, tables, derived pages, edges, serialization, annotations,
//! the document's pages and objects, `pdfplumber.open`) run from every depth
//! that moves the recursion limit through their frames, from the room of
//! their deepest frame down to none. Each trace is compared with the
//! oracle's by its SHA-256; a difference prints the case, the room, and
//! the first trace's first differing line when it is the first room.
//!
//! `PLUMBER_ONLY=<substring>[,<substring>...]` restricts the cases to the
//! file names or operations containing one of them; `PLUMBER_DUMP=<dir>`
//! writes each differing trace there; `PLUMBER_FAIL_FAST=1` stops at the
//! first difference; `PLUMBER_THREADS` (4 by default, one under WASI) is the
//! number of replay threads.

use std::fs;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use pdfminer::cmapdb::{CMapDB, DirCMapProvider};
use pdfplumber::container::{Container, SerializeOptions};
use pdfplumber::obj::{Obj, PyValue};
use pdfplumber::page::Page;
use pdfplumber::pdf::PDF;
use pdfplumber::utils::text::Kwargs;
use pdftrace::canon::{canon, sha256_hex};
use pdftrace::plumber::{from_value, obj_from_value, to_value};
use pdftrace::stages::common::{terminated, Lines};
use pdftrace::stages::plumber_common::{exc, open_pdf};
use pdftrace::stages::plumber_objects::{content_ids, ids_factory, oracle_ids, IdsRegistry};
use pdftrace::stages::plumber_tables::settings_of;
use pdftrace::stages::plumber_text::{derive, parse_spans, run_op};
use pdftrace::stages::py_process::{self, PyProcess};
use pdftrace::stages::StageInput;
use pdftrace::typed_json::{self, Json};
use pdftrace::unit::{cache_dir, UnitData};
use pdftrace::value::Value;
use pycompat::recursion::{leaf, py_frame, with_depth, Depth, C_RECURSION_LIMIT};
use pycompat::{PyInt, PyResult, PyStr};

/// `LIMIT` of the generator: Python's recursion limit.
const LIMIT: u32 = 1000;
/// `MAX_PAGES` of the generator.
const MAX_PAGES: usize = 20;

fn open() -> UnitData {
    let data = UnitData::open("plumber_sweep");
    let meta = fs::read_to_string(data.path("meta.json")).expect("meta.json");
    let meta = typed_json::parse_json(&meta).expect("meta.json is JSON");
    let Some(Json::String(rel)) = meta.get("cmap_dir") else { panic!("meta.json has no cmap_dir") };
    let dir = cache_dir().join(rel);
    let provider = DirCMapProvider::new(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
    CMapDB::set_default_provider(Some(Arc::new(provider)));
    data
}

fn field(v: &Value, name: &str) -> u32 {
    let Value::Dict(items) = v else { panic!("a depth: {v:?}") };
    let v = items.iter().find(|(k, _)| k.as_str() == Some(name)).map(|(_, v)| v).expect("a depth field");
    let Value::Int(i) = v else { panic!("an int: {v:?}") };
    u32::try_from(i.as_i64().expect("small")).expect("a depth")
}

fn int(n: usize) -> Value {
    Value::Int(PyInt::Small(n as i64))
}

fn str_of(v: &Value) -> &str {
    v.as_str().expect("a str")
}

fn str_list(v: &PyValue) -> Vec<PyStr> {
    v.as_seq().expect("a list").iter().map(|s| s.as_pystr().expect("a str").clone()).collect()
}

/// What `deep_apply` returns.
enum Deep {
    Value(Value),
    Objects(Rc<pdfplumber::container::Objects>),
    Layout(usize),
    Edges(Vec<Obj>),
    Tables(Vec<(Value, Value)>),
    Finder([usize; 4]),
    Annots(Vec<Obj>),
    Pages(Vec<Rc<Page>>),
    Opened(PDF),
}

fn rows_value(rows: &[Vec<Option<PyStr>>]) -> Value {
    Value::List(
        rows.iter()
            .map(|r| Value::List(r.iter().map(|c| c.as_ref().map_or(Value::None, Value::py_str)).collect()))
            .collect(),
    )
}

/// The context of a trace.
struct Case<'a> {
    input: StageInput<'a>,
    op: &'a [Value],
    spans: Option<&'a str>,
}

/// `deep_apply(path, pdf, page, op)`: a frame; the operation's value or
/// the exception it raised.
fn deep_apply(case: &Case<'_>, pdf: &PDF, page: Option<&Rc<Page>>) -> PyResult<Deep> {
    let _frame = py_frame()?;
    let op = case.op;
    let page = || page.expect("a page");
    let kwargs = |v: &Value| -> Kwargs { obj_from_value(v).expect("kwargs") };
    Ok(match str_of(&op[0]) {
        "text" => {
            let Value::List(inner) = &op[1] else { panic!("an op") };
            let spans = match case.spans {
                Some(text) => parse_spans(text).expect("spans"),
                None => None,
            };
            Deep::Value(run_op(page(), inner, spans)?)
        }
        "derived" => {
            let Value::List(inner) = &op[2] else { panic!("an op") };
            Deep::Value(run_op(&derive(page(), &op[1])?, inner, None)?)
        }
        "derive" => Deep::Objects(derive(page(), &op[1])?.objects()?),
        "objects" => Deep::Objects(page().objects()?),
        "layout" => Deep::Layout(page().layout()?.layout._objs.len()),
        "edges" => Deep::Edges(match str_of(&op[1]) {
            "edges" => page().edges()?.to_vec(),
            "horizontal_edges" => page().horizontal_edges()?,
            _ => page().vertical_edges()?,
        }),
        "edges_identity" => {
            let p = page();
            let a = p.edges()?;
            let b = p.edges()?;
            let r1 = p.rect_edges()?;
            let c1 = p.curve_edges()?;
            let o1 = p.objects()?;
            let l1 = p.layout()?;
            // page.flush_cache(): its frame, then hasattr/delattr from C
            leaf(1)?;
            Container::flush_cache(&**p, None);
            let c = p.edges()?;
            let r2 = p.rect_edges()?;
            let c2 = p.curve_edges()?;
            let o2 = p.objects()?;
            let l2 = p.layout()?;
            Deep::Value(Value::List(vec![
                Value::Bool(Rc::ptr_eq(&a, &b)),
                Value::Bool(Rc::ptr_eq(&a, &c)),
                int(c.len()),
                Value::Bool(Rc::ptr_eq(&r1, &r2)),
                Value::Bool(Rc::ptr_eq(&c1, &c2)),
                Value::Bool(Rc::ptr_eq(&o1, &o2)),
                Value::Bool(Rc::ptr_eq(&l1, &l2)),
            ]))
        }
        "pdf_edges_identity" => {
            let a = pdf.edges()?;
            let b = pdf.edges()?;
            let o1 = pdf.objects()?;
            let p1 = pdf.pages()?;
            // pdf.flush_cache(): its frame, then hasattr/delattr from C
            leaf(1)?;
            Container::flush_cache(pdf, None);
            let c = pdf.edges()?;
            let o2 = pdf.objects()?;
            let p2 = pdf.pages()?;
            // `p1 is p2`: the same list object (the pages, in the same order)
            let same_pages = p1.len() == p2.len() && p1.iter().zip(&p2).all(|(x, y)| Rc::ptr_eq(x, y));
            Deep::Value(Value::List(vec![
                Value::Bool(Rc::ptr_eq(&a, &b)),
                Value::Bool(Rc::ptr_eq(&a, &c)),
                int(c.len()),
                Value::Bool(Rc::ptr_eq(&o1, &o2)),
                Value::Bool(same_pages),
            ]))
        }
        "close_identity" => {
            let o1 = page().objects()?;
            pdf.close()?;
            let o2 = page().objects()?;
            let chars = o2.get("char").map_or(0, |c| c.len());
            Deep::Value(Value::List(vec![Value::Bool(Rc::ptr_eq(&o1, &o2)), int(chars)]))
        }
        "pdf_to_csv" => {
            let k = kwargs(&op[1]);
            let options = SerializeOptions {
                object_types: k.get("object_types").map(str_list),
                include_attrs: k.get("include_attrs").map(str_list),
                exclude_attrs: k.get("exclude_attrs").map(str_list),
                precision: k.get("precision").cloned(),
                indent: k.get("indent").cloned(),
            };
            Deep::Value(Value::py_str(&pdf.to_csv(&options)?))
        }
        "find_tables" => {
            let Value::List(s) = &op[1] else { panic!("settings") };
            let settings = settings_of(page(), s)?;
            let mut out = Vec::new();
            for t in page().find_tables(Some(&settings))? {
                leaf(1)?; // t.bbox: a property, a leaf
                let bbox = t.bbox()?;
                let rows = t.extract(&**page(), &Kwargs::new())?;
                out.push((Value::List(bbox.iter().map(to_value).collect()), rows_value(&rows)));
            }
            Deep::Tables(out)
        }
        "extract_tables" => {
            let Value::List(s) = &op[1] else { panic!("settings") };
            let tables = page().extract_tables(Some(&settings_of(page(), s)?))?;
            Deep::Value(Value::List(tables.iter().map(|t| rows_value(t)).collect()))
        }
        "extract_table" => {
            let Value::List(s) = &op[1] else { panic!("settings") };
            let table = page().extract_table(Some(&settings_of(page(), s)?))?;
            Deep::Value(table.map_or(Value::None, |t| rows_value(&t)))
        }
        "debug_tablefinder" => {
            let Value::List(s) = &op[1] else { panic!("settings") };
            let tf = page().debug_tablefinder(Some(&settings_of(page(), s)?))?;
            Deep::Finder([tf.edges.len(), tf.intersections.points.len(), tf.cells.len(), tf.tables.len()])
        }
        method @ ("to_json" | "to_csv") => {
            let k = kwargs(&op[1]);
            let options = SerializeOptions {
                object_types: k.get("object_types").map(str_list),
                include_attrs: k.get("include_attrs").map(str_list),
                exclude_attrs: k.get("exclude_attrs").map(str_list),
                precision: k.get("precision").cloned(),
                indent: k.get("indent").cloned(),
            };
            let text = if method == "to_json" { page().to_json(&options)? } else { page().to_csv(&options)? };
            Deep::Value(Value::py_str(&text))
        }
        "annots" => Deep::Annots(page().annots()?),
        "hyperlinks" => Deep::Annots(page().hyperlinks()?),
        "pdf_pages" => Deep::Pages(pdf.pages()?),
        "pdf_objects" => Deep::Objects(pdf.objects()?),
        "pdf_annots" => Deep::Annots(pdf.annots()?),
        "pdf_hyperlinks" => Deep::Annots(pdf.hyperlinks()?),
        "open" => {
            let options = kwargs(&op[1]);
            Deep::Opened(open_pdf(&case.input, |o| set_options(o, &options, None))?)
        }
        other => panic!("unknown op {other}"),
    })
}

/// The keyword arguments of `pdfplumber.open` the cases use.
fn set_options(o: &mut pdfplumber::pdf::OpenOptions, options: &Obj, ids: Option<pdfplumber::pdf::IdsFactory>) {
    for (k, v) in options.items() {
        match k.as_str() {
            Some("laparams") => o.laparams = Some(v.clone()),
            Some("unicode_norm") => o.unicode_norm = Some(v.clone()),
            other => panic!("an option of pdfplumber.open: {other:?}"),
        }
    }
    if ids.is_some() {
        o.ids = ids;
    }
}

fn obj_item(o: &Obj, k: &str) -> Value {
    to_value(o.item(k).expect("an attribute"))
}

/// `summarize(op, value)`.
fn summarize(deep: Deep) -> PyResult<Value> {
    Ok(match deep {
        Deep::Value(v) => v,
        Deep::Objects(objects) => {
            Value::List(objects.iter().map(|(k, v)| Value::List(vec![Value::py_str(k), int(v.len())])).collect())
        }
        Deep::Layout(n) => int(n),
        Deep::Edges(edges) => {
            let mut out = vec![int(edges.len())];
            for e in edges.iter().take(4) {
                out.push(Value::List(
                    ["x0", "top", "x1", "bottom", "orientation"].iter().map(|k| obj_item(e, k)).collect(),
                ));
            }
            Value::List(out)
        }
        Deep::Tables(tables) => Value::List(tables.into_iter().map(|(b, r)| Value::List(vec![b, r])).collect()),
        Deep::Finder(n) => Value::List(n.iter().map(|&k| int(k)).collect()),
        Deep::Annots(annots) => Value::List(
            annots
                .iter()
                .map(|a| {
                    Value::List(["page_number", "x0", "top", "x1", "bottom"].iter().map(|k| obj_item(a, k)).collect())
                })
                .collect(),
        ),
        Deep::Pages(pages) => Value::List(
            pages
                .iter()
                .map(|p| Value::List(vec![Value::Int(p.page_number.clone()), to_value(&p.initial_doctop)]))
                .collect(),
        ),
        Deep::Opened(pdf) => int(pdf.pages()?.len()),
    })
}

/// `sweep_trace(path, op, pre, options, room)`.
fn sweep_trace(case: &Case<'_>, pre: &str, options: &Obj, here: &Value, room: u32, ids: &str) -> String {
    let mut out = Vec::new();
    let oracle = oracle_ids(ids).expect("the ids of the trace");
    terminated(&mut out, |lines: &mut Lines| {
        let (factory, registry): (_, IdsRegistry) = ids_factory(&oracle);
        let pdf = open_pdf(&case.input, |o| set_options(o, options, Some(factory)))?;
        let mut page = None;
        if pre != "pdf" {
            let p = pdf.pages()?.into_iter().next().expect("a first page");
            match pre {
                "layout" => drop(p.layout()?),
                "objects" => drop(p.objects()?),
                "edges" => {
                    drop(p.objects()?);
                    drop(p.edges()?);
                }
                _ => {}
            }
            page = Some(p);
        }
        // down(n, lambda: deep_apply(...)): the lambda's frame at LIMIT - room
        let at = Depth { py: LIMIT - room, c: C_RECURSION_LIMIT - field(here, "c_left") };
        match with_depth(at, || deep_apply(case, &pdf, page.as_ref())) {
            Err(e) => lines.push(format!("raised {}", exc(&e))),
            Ok(deep) => lines.push(format!("ok {}", canon(&summarize(deep)?))),
        }
        if let (Some(p), true) = (&page, pdf.core.laparams.is_some()) {
            // the analysis's id() ranking (computed again here if the
            // operation raised before)
            let layout = p.layout()?;
            leaf(1)?;
            let n = p.page_number.as_i64().expect("a page number");
            let ids = registry.borrow().get(&n).cloned().expect("the page's ids");
            let value = content_ids(&layout, &mut ids.borrow_mut());
            lines.push(format!("ids {n} {}", canon(&value)));
        }
        Ok(())
    })
    .expect("write to memory");
    String::from_utf8(out).expect("ASCII")
}

fn first_difference(expected: &str, got: &str) -> String {
    for (i, (e, g)) in expected.lines().zip(got.lines()).enumerate() {
        if e != g {
            let clip = |s: &str| s.chars().take(400).collect::<String>();
            return format!("line {}:\n  oracle: {}\n  rust:   {}", i + 1, clip(e), clip(g));
        }
    }
    format!("line counts {} (oracle) and {} (rust)", expected.lines().count(), got.lines().count())
}

/// The failures of one case (`None` when `PLUMBER_ONLY` leaves it out),
/// and the number of its traces that reach the recursion limit.
fn replay(args: &[Value], expected: &Value, only: Option<&str>, stop: &AtomicBool) -> Option<(Vec<String>, usize)> {
    if stop.load(Ordering::Relaxed) {
        return None;
    }
    let [Value::Str(name), Value::Bytes(pdf), Value::List(op), Value::Str(pre), options, here, Value::List(rooms), spans] =
        args
    else {
        panic!("bad case {args:?}");
    };
    let name = name.as_str().expect("a file name");
    let label = format!("{name} {}", canon(&Value::List(op.clone())));
    if only.is_some_and(|o| !o.split(',').any(|s| label.contains(s))) {
        return None;
    }
    let Value::List(out) = expected else { panic!("bad output") };
    let [Value::Str(first), Value::List(digests), Value::List(ids)] = &out[..] else { panic!("bad output {out:?}") };
    let first = first.as_str().expect("ASCII");
    let options = match from_value(options) {
        Ok(PyValue::Dict(ref d)) => d.clone(),
        other => panic!("options: {other:?}"),
    };
    let spans = match spans {
        Value::Str(s) => Some(s.as_str().expect("ASCII")),
        _ => None,
    };
    let case = Case {
        input: StageInput { pdf, password: "", password_given: false, max_pages: Some(MAX_PAGES), oracle: None },
        op,
        spans,
    };
    let (mut failures, mut limited) = (Vec::new(), 0);
    let mut failed_rooms = Vec::new();
    for (i, ((room, digest), ids)) in rooms.iter().zip(digests).zip(ids).enumerate() {
        let Value::Int(room) = room else { panic!("a room") };
        let room = u32::try_from(room.as_i64().expect("small")).expect("a room");
        let Value::Str(ids) = ids else { panic!("an ids line") };
        let got = sweep_trace(&case, pre.as_str().expect("a pre"), &options, here, room, ids.as_str().expect("ASCII"));
        limited += usize::from(got.contains("RecursionError"));
        let Value::Str(digest) = digest else { panic!("a digest") };
        if sha256_hex(got.as_bytes()) != digest.as_str().expect("hex") {
            if let Some(dir) = std::env::var_os("PLUMBER_DUMP") {
                let file = format!("{name}-{}-{room}.txt", sha256_hex(label.as_bytes()).get(..8).unwrap_or(""));
                fs::write(std::path::Path::new(&dir).join(file), &got).expect("PLUMBER_DUMP is a writable directory");
            }
            failed_rooms.push(room);
            if failures.is_empty() {
                let at = if i == 0 { first_difference(first, &got) } else { format!("trace digest:\n{got}") };
                failures.push(format!("{label} [pre {}, room {room}]: {at}", pre.as_str().unwrap_or("?")));
            }
            if std::env::var_os("PLUMBER_FAIL_FAST").is_some() {
                stop.store(true, Ordering::Relaxed);
                break;
            }
        }
    }
    if let Some(f) = failures.first_mut() {
        // one failure per case: the rooms that differ, the first one's trace
        *f = format!("{f}\n  (rooms that differ: {failed_rooms:?})");
    }
    Some((failures, limited))
}

#[test]
fn sweeps() {
    let data = open();
    let cases = data.cases("py_process");
    let [(_, state)] = &cases[..] else { panic!("one process state") };
    let process = PyProcess::from_value(state);
    let only = std::env::var("PLUMBER_ONLY").ok();
    let default_threads = if cfg!(target_family = "wasm") { 1 } else { 4 };
    let threads: usize =
        std::env::var("PLUMBER_THREADS").ok().and_then(|t| t.parse().ok()).unwrap_or(default_threads).max(1);
    let all = data.cases("sweeps");
    let stop = AtomicBool::new(false);
    let results: Vec<(Vec<String>, usize)> = if threads == 1 {
        py_process::set(process);
        all.iter().filter_map(|(args, expected)| replay(args, expected, only.as_deref(), &stop)).collect()
    } else {
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..threads)
                .map(|k| {
                    let (all, only, process, stop) = (&all, only.as_deref(), process.clone(), &stop);
                    std::thread::Builder::new()
                        .stack_size(64 << 20)
                        .spawn_scoped(scope, move || {
                            py_process::set(process);
                            all.iter()
                                .enumerate()
                                .filter(|(i, _)| i % threads == k)
                                .filter_map(|(_, (args, expected))| replay(args, expected, only, stop))
                                .collect::<Vec<_>>()
                        })
                        .expect("a thread")
                })
                .collect();
            handles.into_iter().flat_map(|h| h.join().expect("a replay thread")).collect()
        })
    };
    let n = results.len();
    let limited: usize = results.iter().map(|r| r.1).sum();
    let failures: Vec<String> = results.into_iter().flat_map(|r| r.0).collect();
    if only.is_none() && failures.is_empty() {
        assert!(n > 200, "only {n} sweeps");
        assert!(limited > 2000, "only {limited} traces reach the recursion limit");
    }
    assert!(failures.is_empty(), "{} cases differ:\n{}", failures.len(), failures[..failures.len().min(80)].join("\n"));
}
