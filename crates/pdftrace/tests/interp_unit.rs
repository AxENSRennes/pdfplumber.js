//! Replays `oracle/unit/interp.py`: the traces of the `interp`, `fonts`
//! and `layout` stages (the interpreter, the fonts it obtains and the
//! layout, on pdfplumber's device; `layout` is given its Python trace as
//! its oracle input, the `id()` order of the analysis) and of
//! `interp_text` and `fonts_text` (on pdfminer's `PDFTextDevice`) on the
//! adversarial corpus `adv-interp` (its depth windows included, also from
//! deeper stage entries), `micro` and small files of `quick`; and
//! `oracle/unit/interp_deep.py` (`deep_sweeps`): `interp_text` with the
//! device of `DeepTracingDevice` (each method descends `descent` frames
//! first, `stages::interp_text::run_deep`) on `adv-interp`, each file from
//! a range of entries that moves the recursion limit one frame at a time
//! through the paths to the device calls.
//!
//! `resource_manager_get_cmap` replays `PDFResourceManager.get_cmap`
//! (`api.jsonl`), which no page calls, by name and from every depth near
//! the recursion limit.
//!
//! The depth windows and the sweeps are replayed from the deeper stage
//! entries the generators traced them from (`with_stage_entry`).
//!
//! A difference prints the case and the first differing line of the
//! trace (`INTERP_DUMP=<dir>`: the differing traces of `deep_sweeps` are
//! written there). `INTERP_ONLY=<substring>[,<substring>...]` restricts the cases to the file names
//! containing one of them.
//!
//! The generator computes each trace in a process forked for it, whose
//! state (`py_process.jsonl`: the interned symbols) each replayed trace
//! starts from (`stages::py_process`), and names the directory of the
//! predefined CMaps (`cmap_dir` of `meta.json`) the replay's provider reads.

use std::fs;
use std::sync::Arc;

use pdfminer::cmapdb::{CMapDB, DirCMapProvider};
use pdfminer::pdfinterp::PDFResourceManager;
use pdftrace::canon::{canon, canon_exc, sha256_hex};
use pdftrace::stages::common::with_stage_entry;
use pdftrace::stages::interp_text::run_deep;
use pdftrace::stages::py_process::{self, PyProcess};
use pdftrace::stages::STAGE_ENTRY;
use pdftrace::stages::{self, StageInput};
use pdftrace::typed_json::{self, Json};
use pdftrace::unit::{cache_dir, UnitData};
use pdftrace::value::Value;
use pycompat::recursion::{with_depth, Depth, C_RECURSION_LIMIT};

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
    let [(_, state)] = &cases[..] else { panic!("one process state") };
    PyProcess::from_value(state)
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
        let input = StageInput { pdf, password: &password, password_given, max_pages, oracle: stage.oracle_input.then_some(expected) };
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
            failures.push(format!("{name} [{what}]: {}", first_difference(expected, &got)));
        }
    }
    if only.is_none() {
        assert!(n > 500, "only {n} traces");
        for what in ["interp_text", "fonts_text", "interp", "fonts", "layout"] {
            assert!(per_stage.get(what).is_some_and(|&k| k > 300), "{what}: {per_stage:?}");
        }
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
        let [Value::Str(name), Value::Bytes(pdf), password, Value::Int(descent), base, Value::List(frames)] = &args[..]
        else {
            panic!("bad case {args:?}");
        };
        let name = name.as_str().expect("a file name");
        if only.as_ref().is_some_and(|o| !o.split(',').any(|s| name.contains(s))) {
            continue;
        }
        assert_eq!(depth(base), STAGE_ENTRY, "the stage body's depth");
        let Value::List(out) = &expected else { panic!("bad output") };
        let [Value::Str(first), Value::List(digests)] = &out[..] else { panic!("bad output {out:?}") };
        let first = first.as_str().expect("ASCII");
        let password_given = matches!(password, Value::Str(_));
        let password = match password {
            Value::Str(p) => p.as_str().expect("a password").to_owned(),
            _ => String::new(),
        };
        let descent = u32::try_from(descent.as_i64().expect("small")).expect("a descent");
        let input = StageInput { pdf, password: &password, password_given, max_pages: Some(20), oracle: None };
        assert_eq!(frames.len(), digests.len(), "{name}: one digest per depth");
        files += 1;
        for (i, (n, digest)) in frames.iter().zip(digests).enumerate() {
            let Value::Int(n) = n else { panic!("a number of frames") };
            let n = u32::try_from(n.as_i64().expect("small")).expect("frames");
            let mut out = Vec::new();
            // down(n): n + 1 frames, and its lambda's
            run_deep(&input, &mut out, descent, n + 2).expect("write to memory");
            let got = String::from_utf8(out).expect("ASCII");
            traces += 1;
            if got.contains("RecursionError") {
                limited += 1;
            }
            let Value::Str(digest) = digest else { panic!("a digest") };
            if sha256_hex(got.as_bytes()) != digest.as_str().expect("hex") {
                if let Some(dir) = std::env::var_os("INTERP_DUMP") {
                    let path = std::path::Path::new(&dir).join(format!("{name}-{descent}-{n}.txt"));
                    fs::write(path, &got).expect("INTERP_DUMP is a writable directory");
                }
                let at = if i == 0 { first_difference(first, &got) } else { "trace digest".to_owned() };
                failures.push(format!("{name} [descent {descent}, {n} frames deeper]: {at}"));
            }
        }
    }
    if only.is_none() {
        assert!(files > 300, "only {files} sweeps");
        assert!(limited > 3000, "only {limited} of {traces} traces reach the recursion limit");
    }
    assert!(
        failures.is_empty(),
        "{} of {traces} traces differ:\n{}",
        failures.len(),
        failures[..failures.len().min(20)].join("\n")
    );
}

/// `PDFResourceManager.get_cmap`, which nothing calls on a page: each
/// case from the recorded depth (`api.jsonl` of `oracle/unit/interp_deep.py`),
/// from a new process's state (the generator forks one per case: `CMapDB`'s
/// caches empty).
#[test]
fn resource_manager_get_cmap() {
    let data = open("interp_deep");
    py_process::set(generator_process(&data));
    let mut failures = Vec::new();
    let (mut n, mut limited) = (0, 0);
    for (args, expected) in data.cases("api") {
        let [Value::Str(method), Value::Str(name), Value::Bool(strict), entry] = &args[..] else {
            panic!("bad case {args:?}");
        };
        assert_eq!(method.as_str(), Some("get_cmap"));
        let name = name.as_str().expect("a CMap name");
        py_process::start();
        pycompat::logging::disable();
        CMapDB::clear_caches();
        let rsrcmgr = PDFResourceManager::new(true);
        let got = match with_depth(depth(entry), || rsrcmgr.get_cmap(name, *strict)) {
            Ok(cmap) => {
                let cids = cmap.decode(b"\x00\x01\x30\x42\xff").expect("decode");
                Value::List(vec![
                    Value::Str(cmap.class_name().into()),
                    Value::Bool(cmap.is_vertical()),
                    Value::List(cids.into_iter().map(Value::Int).collect()),
                ])
            }
            Err(e) => {
                limited += usize::from(e.message.contains("recursion"));
                Value::Str(canon_exc(&e).as_str().into())
            }
        };
        n += 1;
        if canon(&got) != canon(&expected) {
            failures.push(format!(
                "get_cmap({name:?}, {strict}) at {entry:?}: oracle {}, rust {}",
                canon(&expected),
                canon(&got)
            ));
        }
    }
    assert!(n > 100 && limited > 20, "{n} cases, {limited} at the recursion limit");
    assert!(failures.is_empty(), "{} of {n} cases differ:\n{}", failures.len(), failures.join("\n"));
}
