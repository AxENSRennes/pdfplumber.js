//! Replays `oracle/unit/interp.py`: the traces of the `interp_text` and
//! `fonts_text` stages (the interpreter and the fonts it obtains, on
//! pdfminer's `PDFTextDevice`) on the adversarial corpus `adv-interp` (its
//! depth windows included), `micro` and small files of `quick`.
//!
//! A difference prints the case and the first differing line of the
//! trace. `INTERP_ONLY=<substring>` restricts the cases to the file names
//! containing it.
//!
//! The generator computes each trace in a process forked for it, whose
//! state (`py_process.jsonl`: the interned symbols) each replayed trace
//! starts from (`stages::py_process`), and names the directory of the
//! predefined CMaps (`cmap_dir` of `meta.json`) the replay's provider reads.

use std::fs;
use std::sync::Arc;

use pdfminer::cmapdb::{CMapDB, DirCMapProvider};
use pdftrace::stages::py_process::{self, PyProcess};
use pdftrace::stages::{self, StageInput};
use pdftrace::typed_json::{self, Json};
use pdftrace::unit::{cache_dir, UnitData};
use pdftrace::value::Value;

/// Opens the data and installs the provider of the predefined CMaps.
fn open() -> UnitData {
    let data = UnitData::open("interp");
    let meta = fs::read_to_string(data.path("meta.json")).expect("meta.json");
    let meta = typed_json::parse_json(&meta).expect("meta.json is JSON");
    let Some(Json::String(rel)) = meta.get("cmap_dir") else { panic!("meta.json has no cmap_dir") };
    let dir = cache_dir().join(rel);
    let provider = DirCMapProvider::new(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
    CMapDB::set_default_provider(Some(Arc::new(provider)));
    data
}

/// The state of the generator's process when it forks a trace
/// (`process_state` of `oracle/unit/document.py`).
fn generator_process(data: &UnitData) -> PyProcess {
    let cases = data.cases("py_process");
    let [(_, Value::Dict(items))] = &cases[..] else { panic!("one process state") };
    let get = |name: &str| {
        items.iter().find(|(k, _)| k.as_str() == Some(name)).map(|(_, v)| v).unwrap_or_else(|| panic!("{name}"))
    };
    let list = |name: &str| match get(name) {
        Value::List(l) => l.clone(),
        other => panic!("{name}: {other:?}"),
    };
    let bytes = |v: &Value| match v {
        Value::Bytes(b) => b.clone(),
        other => panic!("bytes: {other:?}"),
    };
    PyProcess {
        literals: list("literals").iter().map(|v| v.as_str().expect("a str name").to_owned()).collect(),
        bytes_literals: list("bytes_literals").iter().map(bytes).collect(),
        keywords: list("keywords").iter().map(bytes).collect(),
    }
}

fn first_difference(expected: &str, got: &str) -> String {
    for (i, (e, g)) in expected.lines().zip(got.lines()).enumerate() {
        if e != g {
            return format!("line {}:\n  oracle: {}\n  rust:   {}", i + 1, clip(e), clip(g));
        }
    }
    format!("line counts {} (oracle) and {} (rust)", expected.lines().count(), got.lines().count())
}

fn clip(s: &str) -> &str {
    let mut end = s.len().min(400);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[test]
fn stage_traces() {
    let data = open();
    py_process::set(generator_process(&data));
    let only = std::env::var("INTERP_ONLY").ok();
    let mut failures = Vec::new();
    let mut n = 0;
    let mut depth_cases = 0;
    for (args, expected) in data.cases("stages") {
        let [Value::Str(name), Value::Bytes(pdf), password, Value::Str(what), Value::Int(max_pages)] = &args[..] else {
            panic!("bad case {args:?}");
        };
        let name = name.as_str().expect("a file name");
        if only.as_ref().is_some_and(|o| !name.contains(o.as_str())) {
            continue;
        }
        let password = match password {
            Value::Str(p) => p.as_str().expect("a password").to_owned(),
            _ => String::new(),
        };
        let max_pages = max_pages.as_i64().filter(|&m| m > 0).map(|m| m as usize);
        let input = StageInput { pdf, password: &password, max_pages, oracle: None };
        let what = what.as_str().expect("a stage");
        let run = stages::lookup(what).and_then(|s| s.run).expect("an implemented stage");
        let mut out = Vec::new();
        run(&input, &mut out).expect("write to memory");
        let got = String::from_utf8(out).expect("ASCII");
        let Value::Str(expected) = &expected else { panic!("bad output") };
        let expected = expected.as_str().expect("ASCII");
        n += 1;
        if expected.contains("RecursionError") {
            depth_cases += 1;
        }
        if got != expected {
            failures.push(format!("{name} [{what}]: {}", first_difference(expected, &got)));
        }
    }
    if only.is_none() {
        assert!(n > 500, "only {n} traces");
        assert!(depth_cases > 50, "only {depth_cases} traces reach the recursion limit");
    }
    assert!(
        failures.is_empty(),
        "{} of {n} traces differ:\n{}",
        failures.len(),
        failures[..failures.len().min(20)].join("\n")
    );
}
