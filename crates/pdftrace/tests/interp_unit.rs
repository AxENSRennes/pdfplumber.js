//! Replays `oracle/unit/interp.py`: the traces of the `interp_text` and
//! `fonts_text` stages (the interpreter and the fonts it obtains, on
//! pdfminer's `PDFTextDevice`) on the adversarial corpus `adv-interp` (its
//! depth windows included), `micro` and small files of `quick`; and
//! `oracle/unit/interp_deep.py` (`deep_sweeps`): `interp_text` with the
//! device of `DeepTracingDevice` (each method descends `descent` frames
//! first, `stages::interp_text::run_deep`) on `adv-interp`, each file from
//! a range of entries that moves the recursion limit one frame at a time
//! through the paths to the device calls.
//!
//! The depth windows and the sweeps are replayed from the deeper stage
//! entries the generators traced them from (`with_stage_entry`).
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
use pdftrace::canon::sha256_hex;
use pdftrace::stages::common::with_stage_entry;
use pdftrace::stages::interp_text::run_deep;
use pdftrace::stages::py_process::{self, PyProcess};
use pdftrace::stages::{self, StageInput};
use pdftrace::typed_json::{self, Json};
use pdftrace::unit::{cache_dir, UnitData};
use pdftrace::value::Value;
use pycompat::recursion::{Depth, C_RECURSION_LIMIT};

/// Opens the data of `module` and installs the provider of the predefined
/// CMaps.
fn open(module: &str) -> UnitData {
    let data = UnitData::open(module);
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
    let data = open("interp");
    py_process::set(generator_process(&data));
    let only = std::env::var("INTERP_ONLY").ok();
    let mut failures = Vec::new();
    let mut n = 0;
    let mut depth_cases = 0;
    let mut deeper_entries = 0;
    for (args, expected) in data.cases("stages") {
        let [Value::Str(name), Value::Bytes(pdf), password, Value::Str(what), Value::Int(max_pages), entry] = &args[..]
        else {
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
        match entry {
            Value::None => run(&input, &mut out).expect("write to memory"),
            entry => {
                deeper_entries += 1;
                with_stage_entry(depth(entry), || run(&input, &mut out)).expect("write to memory")
            }
        }
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
        assert!(deeper_entries > 100, "only {deeper_entries} traces from a deeper entry");
    }
    assert!(
        failures.is_empty(),
        "{} of {n} traces differ:\n{}",
        failures.len(),
        failures[..failures.len().min(20)].join("\n")
    );
}

#[test]
fn deep_sweeps() {
    let data = open("interp_deep");
    py_process::set(generator_process(&data));
    let only = std::env::var("INTERP_ONLY").ok();
    let mut failures = Vec::new();
    let (mut files, mut traces, mut limited) = (0, 0, 0);
    for (args, expected) in data.cases("deep") {
        let [Value::Str(name), Value::Bytes(pdf), password, Value::Int(descent), Value::List(entries)] = &args[..]
        else {
            panic!("bad case {args:?}");
        };
        let name = name.as_str().expect("a file name");
        if only.as_ref().is_some_and(|o| !name.contains(o.as_str())) {
            continue;
        }
        let Value::List(out) = &expected else { panic!("bad output") };
        let [Value::Str(first), Value::List(digests)] = &out[..] else { panic!("bad output {out:?}") };
        let first = first.as_str().expect("ASCII");
        let password = match password {
            Value::Str(p) => p.as_str().expect("a password").to_owned(),
            _ => String::new(),
        };
        let descent = u32::try_from(descent.as_i64().expect("small")).expect("a descent");
        let input = StageInput { pdf, password: &password, max_pages: Some(20), oracle: None };
        assert_eq!(entries.len(), digests.len(), "{name}: one digest per entry");
        files += 1;
        for (i, (entry, digest)) in entries.iter().zip(digests).enumerate() {
            let mut out = Vec::new();
            with_stage_entry(depth(entry), || run_deep(&input, &mut out, descent)).expect("write to memory");
            let got = String::from_utf8(out).expect("ASCII");
            traces += 1;
            if got.contains("RecursionError") {
                limited += 1;
            }
            let Value::Str(digest) = digest else { panic!("a digest") };
            if sha256_hex(got.as_bytes()) != digest.as_str().expect("hex") {
                let at = if i == 0 { first_difference(first, &got) } else { "trace digest".to_owned() };
                failures.push(format!("{name} [entry {i}: {entry:?}]: {at}"));
            }
        }
    }
    if only.is_none() {
        assert!(files > 100, "only {files} files");
        assert!(limited > 2000, "only {limited} of {traces} traces reach the recursion limit");
    }
    assert!(
        failures.is_empty(),
        "{} of {traces} traces differ:\n{}",
        failures.len(),
        failures[..failures.len().min(20)].join("\n")
    );
}
