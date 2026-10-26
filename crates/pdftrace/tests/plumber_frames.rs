//! Replays `oracle/unit/plumber_frames.py`: `Page.process_object` of each
//! layout object of a few files (the first page, `laparams` None, the
//! layout computed first), from the deepest entry along each recursion
//! counter from which Python raises no `RecursionError`: there it gives
//! Python's outcome, and from one frame (`py`) or one C unit (`c`) deeper
//! it raises `RecursionError`. This pins every frame and C unit of
//! `process_object` (`process_attr` called by `map`, `resolve_all` and its
//! generator expressions, `decode_text`, `point2coord`, `Page.height`),
//! which the stage traces and the sweeps of `plumber_sweep` cannot see (the
//! interpretation goes deeper first, and a sweep sees only whether an
//! operation raised); the performance paths of `process_object`
//! (`docs/architecture.md`, "Chantier PERF") must keep them.
//!
//! The Python frames are checked. The C units are checked only with
//! `PLUMBER_FRAMES_C=1`: the port does not model all of them yet (a known
//! gap, `docs/architecture.md`, "Chantier PERF : réalisation"): the unit
//! `isinstance` takes while it checks a tuple of classes (`isinstance(x,
//! (list, tuple))` in `resolve_all`, `(LTChar, LTTextContainer)` and
//! `(LTCurve,)` in `process_object`), and the C units of the `typing`
//! subscriptions of `process_attr`'s annotations (`pycompat::typing_cache`
//! counts their frames only: `Optional[Tuple[str, Any]]` takes 6 units
//! when it hits). With both, Python raises one C unit before the port for
//! every object.

use std::fs;
use std::sync::Arc;

use pdfminer::cmapdb::{CMapDB, DirCMapProvider};
use pdfplumber::obj::Obj;
use pdfplumber::page::{LayoutObj, Page};
use pdftrace::stages::common::terminated;
use pdftrace::stages::plumber_common::{exc, open_pdf};
use pdftrace::stages::py_process::{self, PyProcess};
use pdftrace::stages::StageInput;
use pdftrace::typed_json::{self, Json};
use pdftrace::unit::{cache_dir, UnitData};
use pdftrace::value::Value;
use pycompat::recursion::{with_depth, Depth, C_RECURSION_LIMIT};
use pycompat::{ExcKind, PyResult};

/// `MAX_OBJECTS` of the generator.
const MAX_OBJECTS: usize = 20;

fn open() -> UnitData {
    let data = UnitData::open("plumber_frames");
    let meta = fs::read_to_string(data.path("meta.json")).expect("meta.json");
    let meta = typed_json::parse_json(&meta).expect("meta.json is JSON");
    let Some(Json::String(rel)) = meta.get("cmap_dir") else { panic!("meta.json has no cmap_dir") };
    let dir = cache_dir().join(rel);
    let provider = DirCMapProvider::new(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
    CMapDB::set_default_provider(Some(Arc::new(provider)));
    data
}

/// The `{"py": .., "c_left": ..}` depth of a case.
fn depth(v: &Value) -> Depth {
    let Value::Dict(items) = v else { panic!("a depth: {v:?}") };
    let field = |name: &str| {
        let v = items.iter().find(|(k, _)| k.as_str() == Some(name)).map(|(_, v)| v).expect("a depth field");
        let Value::Int(i) = v else { panic!("an int: {v:?}") };
        u32::try_from(i.as_i64().expect("small")).expect("a depth")
    };
    Depth { py: field("py"), c: C_RECURSION_LIMIT - field("c_left") }
}

/// `leaves` of the generator: the objects `iter_layout_objects` processes
/// without `laparams`, the containers' children in order.
fn leaves(objs: Vec<LayoutObj>, out: &mut Vec<LayoutObj>) {
    for obj in objs {
        match obj.children() {
            Some(children) => leaves(children, out),
            None => out.push(obj),
        }
    }
}

/// `sample` of the generator: the first objects, then the first of each
/// class not seen.
fn sample(objs: Vec<LayoutObj>) -> Vec<LayoutObj> {
    let mut seen: Vec<&'static str> = objs.iter().take(MAX_OBJECTS).map(LayoutObj::class_name).collect();
    let mut out: Vec<LayoutObj> = objs.iter().take(MAX_OBJECTS).cloned().collect();
    for o in objs.into_iter().skip(MAX_OBJECTS) {
        if !seen.contains(&o.class_name()) {
            seen.push(o.class_name());
            out.push(o);
        }
    }
    out
}

/// The outcome of `process_object` as the generator writes it.
fn outcome(r: &PyResult<Obj>) -> String {
    match r {
        Ok(_) => "ok".to_owned(),
        Err(e) => exc(e),
    }
}

fn str_of(v: &Value) -> &str {
    let Value::Str(s) = v else { panic!("a str: {v:?}") };
    s.as_str().expect("ASCII")
}

/// The differences of one file's objects.
fn replay_file(page: &Page, expected: &[Value], failures: &mut Vec<String>, name: &str) -> PyResult<usize> {
    let layout = page.layout()?;
    let mut objs = Vec::new();
    leaves(layout.layout._objs.iter().cloned().map(LayoutObj::Item).collect(), &mut objs);
    let objs = sample(objs);
    assert_eq!(objs.len(), expected.len(), "{name}: the objects sampled");
    // one pass: every call site executed once
    for o in &objs {
        let _ = page.process_object(o);
    }
    for (i, (o, case)) in objs.iter().zip(expected).enumerate() {
        let Value::List(case) = case else { panic!("a case: {case:?}") };
        let [class, py_depth, py_outcome, c_depth, c_outcome] = &case[..] else { panic!("a case: {case:?}") };
        assert_eq!(o.class_name(), str_of(class), "{name} object {i}");
        let mut counters = vec![("py", depth(py_depth), str_of(py_outcome))];
        if std::env::var_os("PLUMBER_FRAMES_C").is_some() {
            counters.push(("c", depth(c_depth), str_of(c_outcome)));
        }
        for (counter, at, expected) in counters {
            let got = outcome(&with_depth(at, || page.process_object(o)));
            if got != expected {
                failures.push(format!(
                    "{name} {i} {}: at the {counter} limit {at:?}: oracle {expected}, rust {got}",
                    o.class_name()
                ));
            }
            let deeper = if counter == "py" { Depth { py: at.py + 1, ..at } } else { Depth { c: at.c + 1, ..at } };
            match with_depth(deeper, || page.process_object(o)) {
                Err(e) if e.kind == ExcKind::RecursionError => {}
                r => failures.push(format!(
                    "{name} {i} {}: one {counter} deeper {deeper:?}: oracle RecursionError, rust {}",
                    o.class_name(),
                    outcome(&r)
                )),
            }
        }
    }
    Ok(objs.len())
}

#[test]
fn process_object_frames() {
    let data = open();
    let cases = data.cases("py_process");
    let [(_, state)] = &cases[..] else { panic!("one process state") };
    let process = PyProcess::from_value(state);
    let all = data.cases("objects");
    let run = move || {
        py_process::set(process);
        let mut failures = Vec::new();
        let (mut n_files, mut n_objects) = (0, 0);
        for (args, expected) in all {
            let [Value::Str(name), Value::Bytes(pdf), password] = &args[..] else { panic!("bad case {args:?}") };
            let name = name.as_str().expect("ASCII").to_owned();
            let password = match password {
                Value::Str(p) => p.as_str().expect("a password").to_owned(),
                _ => String::new(),
            };
            let Value::List(expected) = &expected else { panic!("bad output") };
            let input =
                StageInput { pdf, password: &password, password_given: false, max_pages: Some(1), oracle: None };
            let mut out = Vec::new();
            let mut n = 0;
            terminated(&mut out, |_| {
                let pdf = open_pdf(&input, |_| {})?;
                let pages = pdf.pages()?;
                n = replay_file(&pages[0], expected, &mut failures, &name)?;
                Ok(())
            })
            .expect("write to memory");
            let out = String::from_utf8(out).expect("ASCII");
            assert_eq!(out, "!end\n", "{name}: the file did not run");
            n_files += 1;
            n_objects += n;
        }
        (n_files, n_objects, failures)
    };
    // the generator's stage thread: a large stack, the oracle's process
    // state (WASI has no threads: the test's own, whose stack is the module's)
    let (n_files, n_objects, failures) = if cfg!(target_family = "wasm") {
        run()
    } else {
        std::thread::Builder::new()
            .stack_size(64 << 20)
            .spawn(run)
            .expect("a thread")
            .join()
            .expect("the replay thread")
    };
    assert!(n_files > 20 && n_objects > 200, "only {n_files} files, {n_objects} objects");
    assert!(failures.is_empty(), "{} differences:\n{}", failures.len(), failures[..failures.len().min(40)].join("\n"));
}

/// `functions.jsonl`: `decode_text` of a `str` and `resolve_all` of the
/// values `process_object` makes, on their own (where their frames are the
/// deepest), through each path of the port: `decode_text_value`, and both
/// `resolve_all_value` and `resolve_all_owned` (a value only it holds).
#[test]
fn function_frames() {
    let data = open();
    let cases = data.cases("py_process");
    let [(_, state)] = &cases[..] else { panic!("one process state") };
    py_process::set(PyProcess::from_value(state));
    let all = data.cases("functions");
    let [(_, Value::List(rows))] = &all[..] else { panic!("one case of functions") };
    py_process::start();
    let mut failures = Vec::new();
    let check_c = std::env::var_os("PLUMBER_FRAMES_C").is_some();
    type Path<'a> = (&'a str, Box<dyn Fn() -> PyResult<()> + 'a>);
    // one pass: every call site executed once
    let parsed: Vec<(String, pdfplumber::obj::PyValue, &Vec<Value>)> = rows
        .iter()
        .map(|row| {
            let Value::List(row) = row else { panic!("a row: {row:?}") };
            let [Value::Str(name), Value::Str(arg), ..] = &row[..] else { panic!("a row: {row:?}") };
            let arg = typed_json::parse(arg.as_str().expect("ASCII")).expect("typed JSON");
            let value = pdftrace::plumber::from_value(&arg).expect("a value");
            (name.as_str().expect("ASCII").to_owned(), value, row)
        })
        .collect();
    for (name, value, _) in &parsed {
        let _ = match name.as_str() {
            "decode_text" => pdfplumber::utils::pdfinternals::decode_text_value(value).map(drop),
            _ => pdfplumber::page::resolve_all_value(value).map(drop),
        };
    }
    for (name, value, row) in &parsed {
        let [_, _, py_depth, py_outcome, c_depth, c_outcome] = &row[..] else { panic!("a row: {row:?}") };
        let paths: Vec<Path<'_>> = match name.as_str() {
            "decode_text" => {
                vec![(
                    "decode_text_value",
                    Box::new(|| pdfplumber::utils::pdfinternals::decode_text_value(value).map(drop)),
                )]
            }
            "resolve_all" => vec![
                ("resolve_all_value", Box::new(|| pdfplumber::page::resolve_all_value(value).map(drop))),
                // a copy only the call holds (from_value makes new containers)
                ("resolve_all_owned", Box::new(|| pdfplumber::page::resolve_all_owned(deep_copy(value)).map(drop))),
            ],
            other => panic!("unknown function {other}"),
        };
        let mut counters = vec![("py", depth(py_depth), str_of(py_outcome))];
        if check_c {
            counters.push(("c", depth(c_depth), str_of(c_outcome)));
        }
        for (path, f) in &paths {
            for (counter, at, expected) in &counters {
                let got = match with_depth(*at, f) {
                    Ok(()) => "ok".to_owned(),
                    Err(e) => exc(&e),
                };
                if got != *expected {
                    failures.push(format!(
                        "{path}({value:?}): at the {counter} limit {at:?}: oracle {expected}, rust {got}"
                    ));
                }
                let deeper =
                    if *counter == "py" { Depth { py: at.py + 1, ..*at } } else { Depth { c: at.c + 1, ..*at } };
                match with_depth(deeper, f) {
                    Err(e) if e.kind == ExcKind::RecursionError => {}
                    r => failures.push(format!(
                        "{path}({value:?}): one {counter} deeper {deeper:?}: oracle RecursionError, rust {}",
                        r.map_or_else(|e| exc(&e), |()| "ok".to_owned())
                    )),
                }
            }
        }
    }
    assert!(parsed.len() > 10, "only {} cases", parsed.len());
    assert!(failures.is_empty(), "{} differences:\n{}", failures.len(), failures.join("\n"));
}

/// A copy of `v` with containers of its own (`Rc::get_mut` succeeds on
/// them).
fn deep_copy(v: &pdfplumber::obj::PyValue) -> pdfplumber::obj::PyValue {
    use pdfplumber::obj::PyValue;
    match v {
        PyValue::Tuple(items) => PyValue::tuple(items.iter().map(deep_copy)),
        PyValue::List(items) => PyValue::list(items.iter().map(deep_copy)),
        other => other.clone(),
    }
}
