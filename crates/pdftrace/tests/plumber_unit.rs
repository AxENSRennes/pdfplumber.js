//! Replays `oracle/unit/plumber.py`: the traces of the `plumber_objects`,
//! `plumber_text`, `plumber_tables` and `plumber_doc` stages (pdfplumber's
//! page layer: `PDF`, `Page` and its derived pages, `Container`, the text
//! and table methods, serialization, the structure tree) on the adversarial corpus `adv-plumber` (its depth windows
//! included, also from deeper stage entries), `micro` and small files of
//! `quick`. An `ORACLE_INPUT` stage is given its Python trace (the `id()`
//! order of the analysis, the matches of `search`).
//!
//! A difference prints the case and the first differing line of the trace.
//! `PLUMBER_ONLY=<substring>[,<substring>...]` restricts the cases to the
//! file names containing one of them.
//!
//! The generator computes each trace in a process forked for it, whose
//! state (`py_process.jsonl`) each replayed trace starts from, and names the
//! directory of the predefined CMaps (`cmap_dir` of `meta.json`) the
//! replay's provider reads.

use std::fs;
use std::sync::Arc;

use pdfminer::cmapdb::{CMapDB, DirCMapProvider};
use pdftrace::stages::common::with_stage_entry;
use pdftrace::stages::py_process::{self, PyProcess};
use pdftrace::stages::{self, StageInput};
use pdftrace::typed_json::{self, Json};
use pdftrace::unit::{cache_dir, UnitData};
use pdftrace::value::Value;
use pycompat::recursion::{Depth, C_RECURSION_LIMIT};

/// Opens the data and installs the provider of the predefined CMaps.
fn open() -> UnitData {
    let data = UnitData::open("plumber");
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

fn clip(s: &str) -> &str {
    let mut end = s.len().min(400);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

fn first_difference(expected: &str, got: &str) -> String {
    for (i, (e, g)) in expected.lines().zip(got.lines()).enumerate() {
        if e != g {
            return format!("line {}:\n  oracle: {}\n  rust:   {}", i + 1, clip(e), clip(g));
        }
    }
    format!("line counts {} (oracle) and {} (rust)", expected.lines().count(), got.lines().count())
}

#[test]
fn stage_traces() {
    let data = open();
    let cases = data.cases("py_process");
    let [(_, state)] = &cases[..] else { panic!("one process state") };
    py_process::set(PyProcess::from_value(state));
    let only = std::env::var("PLUMBER_ONLY").ok();
    let mut failures = Vec::new();
    let (mut n, mut depth_cases, mut deeper_entries) = (0, 0, 0);
    let mut per_stage = std::collections::BTreeMap::new();
    for (args, expected) in data.cases("stages") {
        let [Value::Str(name), Value::Bytes(pdf), password, Value::Str(what), Value::Int(max_pages), entry] = &args[..]
        else {
            panic!("bad case {args:?}");
        };
        let name = name.as_str().expect("a file name");
        if only.as_ref().is_some_and(|o| !o.split(',').any(|s| name.contains(s))) {
            continue;
        }
        let password_given = matches!(password, Value::Str(_));
        let password = match password {
            Value::Str(p) => p.as_str().expect("a password").to_owned(),
            _ => String::new(),
        };
        let max_pages = max_pages.as_i64().filter(|&m| m > 0).map(|m| m as usize);
        let what = what.as_str().expect("a stage");
        let Value::Str(expected) = &expected else { panic!("bad output") };
        let expected = expected.as_str().expect("ASCII");
        let stage = stages::lookup(what).expect("a stage");
        let input = StageInput {
            pdf,
            password: &password,
            password_given,
            max_pages,
            oracle: stage.oracle_input.then_some(expected),
        };
        let run = stage.run.expect("an implemented stage");
        let mut out = Vec::new();
        match entry {
            Value::None => run(&input, &mut out).expect("write to memory"),
            entry => {
                deeper_entries += 1;
                with_stage_entry(depth(entry), || run(&input, &mut out)).expect("write to memory")
            }
        }
        let got = String::from_utf8(out).expect("ASCII");
        n += 1;
        *per_stage.entry(what.to_owned()).or_insert(0) += 1;
        if expected.contains("RecursionError") {
            depth_cases += 1;
        }
        if got != expected {
            let at = if matches!(entry, Value::None) { String::new() } else { format!(" from {}", pdftrace::canon::canon(entry)) };
            if let Some(dir) = std::env::var_os("PLUMBER_DUMP") {
                let base = std::path::Path::new(&dir).join(format!("{name}-{what}{}", at.replace(' ', "_")));
                fs::write(base.with_extension("py.txt"), expected).expect("PLUMBER_DUMP is a writable directory");
                fs::write(base.with_extension("rs.txt"), &got).expect("PLUMBER_DUMP is a writable directory");
            }
            failures.push(format!("{name} [{what}]{at}: {}", first_difference(expected, &got)));
        }
    }
    if only.is_none() {
        assert!(n > 300, "only {n} traces");
        for what in ["plumber_objects", "plumber_text", "plumber_tables", "plumber_doc"] {
            assert!(per_stage.get(what).is_some_and(|&k| k > 100), "{what}: {per_stage:?}");
        }
        assert!(depth_cases > 20, "only {depth_cases} traces reach the recursion limit");
        assert!(deeper_entries > 40, "only {deeper_entries} traces from a deeper entry");
    }
    assert!(
        failures.is_empty(),
        "{} of {n} traces differ:\n{}",
        failures.len(),
        failures[..failures.len().min(20)].join("\n")
    );
}

/// `open.jsonl`: `pdfplumber.open(path, **options)` with options of every
/// type (`laparams`, `pages`, `password`, ...), the pages' objects and the
/// ranking of their analysis (`plumber_objects::open_trace`).
#[test]
fn open_options() {
    let data = open();
    let cases = data.cases("py_process");
    let [(_, state)] = &cases[..] else { panic!("one process state") };
    py_process::set(PyProcess::from_value(state));
    let mut failures = Vec::new();
    let mut n = 0;
    for (args, expected) in data.cases("open") {
        let [Value::Str(name), Value::Bytes(pdf), options] = &args[..] else { panic!("bad case {args:?}") };
        let options = match pdftrace::plumber::from_value(options) {
            Ok(pdfplumber::obj::PyValue::Dict(ref d)) => d.clone(),
            other => panic!("options: {other:?}"),
        };
        let Value::Str(expected) = &expected else { panic!("bad output") };
        let expected = expected.as_str().expect("ASCII");
        let input = StageInput { pdf, password: "", password_given: false, max_pages: None, oracle: Some(expected) };
        let mut out = Vec::new();
        pdftrace::stages::plumber_objects::open_trace(&input, &options, &mut out).expect("write to memory");
        let got = String::from_utf8(out).expect("ASCII");
        n += 1;
        if got != expected {
            failures.push(format!(
                "{} {}: {}",
                name.as_str().unwrap_or("?"),
                pdftrace::canon::canon(&args[2]),
                first_difference(expected, &got)
            ));
        }
    }
    assert!(n > 80, "only {n} openings");
    assert!(
        failures.is_empty(),
        "{} of {n} openings differ:\n{}",
        failures.len(),
        failures[..failures.len().min(20)].join("\n")
    );
}
