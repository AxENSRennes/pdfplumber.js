//! Replays the cases of `oracle/unit/js_runtime.py` natively, through the
//! requests and responses of the boundary (`Core::call`), in both result
//! formats: what the JavaScript tests check in Node and every browser
//! (`js/test/shared/tests.mjs`), here without JavaScript, so that mutation
//! testing of `crates/wasm` runs on Python-generated cases.
//!
//! - `exceptions`: the MRO of every exception class (`errors.rs`), directly
//!   and through the `raise` operation;
//! - `cmaps`: predefined CMaps through the lazy protocol (`cmaps.rs`): a
//!   call that misses a CMap reports it, the test hands the file over
//!   (inflated, or zlib-compressed) and runs the call again;
//! - `text`: text, words, `dedupe_chars`, text maps (search with the spans
//!   of the literal patterns, computed here as `RegExp` gives them),
//!   lines and tables over pdfplumber's objects;
//! - `search_groups`: `main_group` read for each match as `m.group()` reads
//!   it, with the spans the oracle recorded;
//! - void calls: a request that reaches a document through an argument,
//!   caches a CMap in it and misses that CMap, re-run once it is handed
//!   over, gives Python's result with every CMap installed, leaves no
//!   handle behind, and reports the diagnostics of the final run only.
//!
//! Regenerate the data with
//! `PDFPLUMBER_ROOT=<main checkout> <root>/.venv/bin/python -m oracle.unit.js_runtime`.

mod common;

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;

use common::*;
use pdfplumber::obj::PyValue;
use pdfplumber_wasm::errors::{mro, qualified_name};
use pdfplumber_wasm::ops::Core;
use pdftrace::plumber::from_value;
use pdftrace::typed_json::{self, Json};
use pdftrace::unit::{cache_dir, UnitData};
use pdftrace::value::Value;
use pycompat::ExcKind;

fn data() -> UnitData {
    UnitData::open("js_runtime")
}

/// The directory of the compact CMaps (recorded by `oracle/unit/cmaps.py`).
fn cmap_dir() -> PathBuf {
    let data = UnitData::open("cmaps");
    let meta = typed_json::parse_json(&fs::read_to_string(data.path("meta.json")).unwrap()).unwrap();
    let Some(Json::String(rel)) = meta.get("cmap_dir") else { panic!("meta.json has no cmap_dir") };
    cache_dir().join(rel)
}

fn py(v: &Value) -> PyValue {
    from_value(v).unwrap_or_else(|e| panic!("{e}: {v:?}"))
}

fn str_list(v: &Value) -> Vec<String> {
    let Value::List(items) = v else { panic!("a list: {v:?}") };
    items.iter().map(|x| x.as_str().expect("a str").to_owned()).collect()
}

/// The JavaScript view of an exception response, as the tests compare it.
fn as_exception(r: &Resp) -> Js {
    let Js::Obj(items) = &r.value else { panic!("an error dict") };
    let get = |k: &str| items.iter().find(|(kk, _)| *kk == k.encode_utf16().collect::<Vec<_>>()).unwrap().1.clone();
    obj(vec![("exception", get("name")), ("message", get("message"))])
}

/// The result of a request, or its exception.
fn result(r: Resp) -> Js {
    match r.status {
        0 => r.value,
        1 => as_exception(&r),
        s => panic!("status {s}: {:?}", r.value),
    }
}

#[test]
fn exception_classes_have_python_s_mro() {
    let cases = data().cases("exceptions");
    let mut seen = BTreeSet::new();
    let mut core = Core::new();
    for (input, out) in &cases {
        let q = input[0].as_str().unwrap();
        let expected = str_list(out);
        let Some(&kind) = ExcKind::ALL.iter().find(|k| qualified_name(**k) == q) else { continue };
        let mut names: Vec<String> = mro(kind).into_iter().map(qualified_name).collect();
        names.push("builtins.object".into());
        assert_eq!(names, expected, "{q}");
        seen.insert(q.to_owned());
        // through the boundary
        let r = call(&mut core, "raise", 0, list(vec![PyValue::str(q), PyValue::str("m")]), None);
        assert_eq!(r.status, 1);
        let Js::Obj(items) = &r.value else { panic!() };
        let expected_js = obj(vec![
            ("name", s(kind.name())),
            ("module", s(kind.module())),
            ("qualname", s(&kind.qualname())),
            ("mro", Js::Arr(expected.iter().map(|n| s(n)).collect())),
            ("message", s("m")),
        ]);
        assert_eq!(Js::Obj(items.clone()), expected_js, "{q}");
    }
    let missing: Vec<String> = ExcKind::ALL
        .iter()
        .filter(|k| **k != ExcKind::NotPorted)
        .map(|k| qualified_name(*k))
        .filter(|q| !seen.contains(q))
        .collect();
    assert!(missing.is_empty(), "classes without Python data: {missing:?}");
    // NotPorted is not a Python class: no `object` in its MRO
    let r = call(&mut core, "raise", 0, list(vec![PyValue::str("pycompat.NotPorted"), PyValue::str("x")]), None);
    let Js::Obj(items) = r.value else { panic!() };
    assert_eq!(items[3].1, Js::Arr(vec![s("pycompat.NotPorted")]));
}

/// zlib's stored form of `data` (no compression), what `deflate` level 0
/// writes: the core inflates it like any zlib stream.
fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    let chunks: Vec<&[u8]> = data.chunks(65535).collect();
    for (i, c) in chunks.iter().enumerate() {
        out.push(u8::from(i + 1 == chunks.len()));
        let n = c.len() as u16;
        out.extend_from_slice(&n.to_le_bytes());
        out.extend_from_slice(&(!n).to_le_bytes());
        out.extend_from_slice(c);
    }
    let adler = pycompat::zlib::adler32::adler32(1, data) as u32;
    out.extend_from_slice(&adler.to_be_bytes());
    out
}

/// Runs a request, handing over the CMaps it misses (each once), as
/// `WasmCore.call` does; returns the result and the names loaded.
fn call_with_cmaps(core: &mut Core, op: &str, args: PyValue, dir: &std::path::Path, loaded: &mut Vec<String>) -> Js {
    for _ in 0..8 {
        let r = call(core, op, 0, args.clone(), None);
        if r.status != 2 {
            return result(r);
        }
        let Js::Arr(names) = r.value else { panic!("missing names") };
        assert!(!names.is_empty());
        for n in names {
            let Js::Str(n) = n else { panic!() };
            let name = String::from_utf16(&n).unwrap();
            assert!(!loaded.contains(&name), "{name} asked for twice");
            let raw = fs::read(dir.join(format!("{name}.pdfcmap"))).unwrap();
            // alternately as zlib and as already inflated
            let file = if loaded.len().is_multiple_of(2) { zlib_stored(&raw) } else { raw };
            assert_eq!(ok(call(core, "cmap_add", 0, PyValue::str(&name), Some(file))), Js::Null);
            loaded.push(name);
        }
    }
    panic!("{op}: still missing CMaps")
}

#[test]
fn predefined_cmaps_load_lazily_and_match_pdfminer() {
    let dir = cmap_dir();
    let names: Vec<String> = fs::read_to_string(dir.join("names.txt")).unwrap().lines().map(str::to_owned).collect();
    for &format in FORMATS {
        // each format on its own thread: a fresh provider and CMap cache
        let dir = dir.clone();
        let names = names.clone();
        on_thread(64 << 20, move || {
            let names: Vec<PyValue> = names.iter().map(|n| PyValue::str(n)).collect();
            let mut core = Core::new();
            ok(call(&mut core, "set_format", 0, PyValue::str(format), None));
            ok(call(&mut core, "cmap_configure", 0, list(names), None));
            let mut loaded = Vec::new();
            for (input, out) in data().cases("cmaps") {
                let op = input[0].as_str().unwrap();
                let args = list(input[1..].iter().map(py).collect());
                let got = match op {
                    "decode" => call_with_cmaps(&mut core, "cmap_decode", args, &dir, &mut loaded),
                    _ => call_with_cmaps(&mut core, "cmap_unichr", args, &dir, &mut loaded),
                };
                assert_eq!(got, value_view(&out), "{format} {input:?}");
            }
            assert!(
                loaded.contains(&"UniJIS-UCS2-H".to_owned()) && loaded.contains(&"to-unicode-Adobe-Korea1".to_owned())
            );
            // a name outside the list is not found, without a request for it
            let r = call(
                &mut core,
                "cmap_decode",
                0,
                list(vec![PyValue::str("No-Such"), PyValue::Bytes(Rc::from(&[1u8][..]))]),
                None,
            );
            assert_eq!(as_exception(&r), obj(vec![("exception", s("CMapNotFound")), ("message", s("No-Such"))]));
            // a file that is not a compact CMap
            let r = call(&mut core, "cmap_add", 0, PyValue::str("UniGB-UCS2-H"), Some(zlib_stored(b"nope")));
            assert_eq!(r.status, 1);
            let r = call(&mut core, "cmap_add", 0, PyValue::str("UniGB-UCS2-H"), Some(b"nope".to_vec()));
            assert_eq!(
                as_exception(&r).clone(),
                obj(vec![
                    ("exception", s("error")),
                    ("message", s("Error -3 while decompressing data: incorrect header check"))
                ])
            );
        });
    }
}

/// Under WebAssembly there is no fresh thread per test: the CMaps a test
/// loads would stay in the provider and `CMapDB`'s caches of the next one,
/// which must miss them (natively, each test has its own thread-local
/// state). In the shared run, `predefined_cmaps_load_lazily_and_match_pdfminer`
/// is the only test that loads CMaps; the two void-call tests are ignored
/// there, so the skip shows in the counts, and `scripts/check.sh` runs each
/// alone, one process per test (`--ignored --exact NAME`).
#[test]
#[cfg_attr(target_family = "wasm", ignore = "loads CMaps: run alone, one process per test, by scripts/check.sh")]
fn a_void_call_leaves_no_state_and_its_rerun_matches_pdfminer() {
    let dir = cmap_dir();
    let names: Vec<String> = fs::read_to_string(dir.join("names.txt")).unwrap().lines().map(str::to_owned).collect();
    on_thread(64 << 20, move || {
        let mut core = Core::new();
        ok(call(&mut core, "cmap_configure", 0, list(names.iter().map(|n| PyValue::str(n)).collect()), None));
        let Js::Num(doc) = ok(call(&mut core, "doc_open", 0, PyValue::None, Some(b"%PDF".to_vec()))) else {
            panic!("a handle")
        };
        let doc = PyValue::from(doc as i64);
        let mut loaded = Vec::new();
        let mut decoded = 0;
        for (input, out) in data().cases("cmaps") {
            if input[0].as_str() != Some("decode") {
                continue;
            }
            // the document is an argument, not the request's handle
            let args = list(vec![doc.clone(), py(&input[1]), py(&input[2])]);
            let Js::Arr(got) = call_with_cmaps(&mut core, "doc_cmap_decode", args, &dir, &mut loaded) else {
                panic!("a pair")
            };
            assert_eq!(got[0], value_view(&out), "{input:?}");
            decoded += 1;
        }
        assert!(decoded >= 400 && loaded.len() >= 10, "{decoded} cases, {loaded:?}");
        // the void runs' derived handles are gone: the document and one
        // value per decode
        let Js::Obj(info) = ok(call(&mut core, "info", 0, PyValue::None, None)) else { panic!() };
        assert_eq!(info[3].1, Js::Num(f64::from(1 + decoded)));
        // a request that warns, then misses a CMap: no diagnostics from the
        // void run, the warning once from the re-run
        let name = names.iter().find(|n| !n.starts_with("to-unicode-") && !loaded.contains(n)).unwrap().clone();
        let warn = list(vec![
            PyValue::str("warn"),
            PyValue::from(0_i64),
            list(vec![list(vec![PyValue::str("pdfminer.pdfinterp"), PyValue::from(30_i64), PyValue::str("w")])]),
        ]);
        let decode = list(vec![
            PyValue::str("doc_cmap_decode"),
            PyValue::from(0_i64),
            list(vec![doc.clone(), PyValue::str(&name), PyValue::Bytes(Rc::from(&[0u8, 0x41][..]))]),
        ]);
        let sequence = list(vec![warn, decode]);
        let no_diagnostics = obj(vec![("entries", Js::Arr(vec![])), ("dropped", Js::Num(0.0))]);
        let r = call_level(&mut core, "sequence", 0, sequence.clone(), None, 10);
        assert_eq!((r.status, &r.diagnostics), (2, &no_diagnostics));
        assert_eq!(r.value, Js::Arr(vec![s(&name)]));
        let raw = fs::read(dir.join(format!("{name}.pdfcmap"))).unwrap();
        ok(call(&mut core, "cmap_add", 0, PyValue::str(&name), Some(raw)));
        let r = call_level(&mut core, "sequence", 0, sequence, None, 10);
        assert_eq!(r.status, 0);
        let warning = Js::Arr(vec![s("pdfminer.pdfinterp"), Js::Num(30.0), s("w")]);
        assert_eq!(r.diagnostics, obj(vec![("entries", Js::Arr(vec![warning])), ("dropped", Js::Num(0.0))]));
        // a nested sequence, and an operation whose value is written by hand
        let nested = list(vec![list(vec![PyValue::str("sequence"), PyValue::from(0_i64), list(vec![])])]);
        assert_eq!(as_exception(&call(&mut core, "sequence", 0, nested, None)), exc("ValueError", "nested sequence"));
        let written = list(vec![list(vec![PyValue::str("test_values"), PyValue::from(0_i64), PyValue::None])]);
        assert_eq!(
            as_exception(&call(&mut core, "sequence", 0, written, None)),
            exc("ValueError", "test_values cannot run in a sequence")
        );
    });
}

fn exc(name: &str, message: &str) -> Js {
    obj(vec![("exception", s(name)), ("message", s(message))])
}

/// A void call puts the modeled state of the Python process back as it was
/// before it (`pdfminer::process_state`): here the CMap `CMapDB` cached
/// from a file already handed over, before the call missed another one.
/// Python's single run, every CMap installed, loads that CMap during the
/// call, deeper than a re-run that found it cached would.
#[test]
#[cfg_attr(target_family = "wasm", ignore = "loads CMaps: run alone, one process per test, by scripts/check.sh")]
fn a_void_call_puts_the_process_state_back() {
    let dir = cmap_dir();
    let names: Vec<String> = fs::read_to_string(dir.join("names.txt")).unwrap().lines().map(str::to_owned).collect();
    on_thread(64 << 20, move || {
        use pdfminer::cmapdb::CMapDB;
        let mut core = Core::new();
        ok(call(&mut core, "cmap_configure", 0, list(names.iter().map(|n| PyValue::str(n)).collect()), None));
        let Js::Num(doc) = ok(call(&mut core, "doc_open", 0, PyValue::None, Some(b"%PDF".to_vec()))) else {
            panic!("a handle")
        };
        let doc = PyValue::from(doc as i64);
        let (present, missing) = ("UniJIS-UCS2-H", "UniGB-UCS2-H");
        let raw = |name: &str| fs::read(dir.join(format!("{name}.pdfcmap"))).unwrap();
        ok(call(&mut core, "cmap_add", 0, PyValue::str(present), Some(raw(present))));
        let decode = |name: &str| {
            list(vec![
                PyValue::str("doc_cmap_decode"),
                PyValue::from(0_i64),
                list(vec![doc.clone(), PyValue::str(name), PyValue::Bytes(Rc::from(&[0u8, 0x41][..]))]),
            ])
        };
        let sequence = list(vec![decode(present), decode(missing)]);
        let cached = || CMapDB::caches_snapshot().names().0.into_iter().map(str::to_owned).collect::<Vec<_>>();
        assert!(cached().is_empty());
        let r = call(&mut core, "sequence", 0, sequence.clone(), None);
        assert_eq!((r.status, r.value), (2, Js::Arr(vec![s(missing)])));
        assert!(cached().is_empty(), "the void call's CMapDB cache entries are dropped: {:?}", cached());
        ok(call(&mut core, "cmap_add", 0, PyValue::str(missing), Some(raw(missing))));
        assert_eq!(call(&mut core, "sequence", 0, sequence, None).status, 0);
        assert_eq!(cached(), [missing, present]);
    });
}

#[test]
fn search_reads_main_group_for_each_match_like_m_group() {
    for &format in FORMATS {
        on_thread(64 << 20, move || {
            let mut core = Core::new();
            ok(call(&mut core, "set_format", 0, PyValue::str(format), None));
            let mut checked = 0;
            for (input, out) in data().cases("search_groups") {
                let r = ok(call(&mut core, "textmap", 0, list(vec![py(&input[0]), PyValue::None]), None));
                let Js::Arr(pair) = r else { panic!() };
                let Js::Num(h) = pair[0] else { panic!() };
                let Value::List(matches) = &input[3] else { panic!("matches") };
                let pairs: Vec<(PyValue, PyValue)> = matches
                    .iter()
                    .map(|m| match m {
                        Value::List(m) => (py(&m[0]), py(&m[1])),
                        other => panic!("a match: {other:?}"),
                    })
                    .collect();
                let named = if pairs.first().is_some_and(|(_, n)| !n.is_none()) {
                    list(pairs.iter().map(|(_, n)| n.clone()).collect())
                } else {
                    PyValue::None
                };
                let spans = list(pairs.into_iter().map(|(s, _)| s).collect());
                let args = list(vec![
                    PyValue::from(h as i64),
                    spans,
                    PyValue::Bool(true),
                    PyValue::Bool(false),
                    py(&input[2]),
                    named,
                ]);
                let got = result(call(&mut core, "textmap_search", 0, args, None));
                assert_eq!(got, value_view(&out), "{format} {:?} main_group={:?}", input[1], input[2]);
                ok(call(&mut core, "free", h as u32, PyValue::None, None));
                checked += 1;
            }
            assert!(checked >= 80, "{checked} cases");
        });
    }
}

/// The spans of every match of a literal pattern in `text` (UTF-16
/// offsets, non-overlapping, left to right): what `matchAll` gives for an
/// escaped string.
fn literal_spans(text: &[u16], pattern: &[u16]) -> Vec<Vec<Option<(usize, usize)>>> {
    let mut out = Vec::new();
    let mut i = 0;
    while !pattern.is_empty() && i + pattern.len() <= text.len() {
        if text[i..i + pattern.len()] == *pattern {
            out.push(vec![Some((i, i + pattern.len()))]);
            i += pattern.len();
        } else {
            i += 1;
        }
    }
    out
}

fn spans_value(spans: &[Vec<Option<(usize, usize)>>]) -> PyValue {
    list(
        spans
            .iter()
            .map(|m| {
                list(
                    m.iter()
                        .map(|sp| match sp {
                            None => PyValue::None,
                            Some((a, b)) => list(vec![PyValue::from(*a as i64), PyValue::from(*b as i64)]),
                        })
                        .collect(),
                )
            })
            .collect(),
    )
}

fn replay_text(format: &'static str) -> usize {
    let mut core = Core::new();
    ok(call(&mut core, "set_format", 0, PyValue::str(format), None));
    let mut checked = 0;
    for (input, out) in data().cases("text") {
        let path = input[0].as_str().unwrap().to_owned();
        let chars = py(&input[2]);
        let edges = py(&input[3]);
        let bbox = py(&input[4]);
        let chars_js = js_view(&chars);
        let Value::Dict(items) = &out else { panic!("out is a dict") };
        let Value::List(ops) = &items[0].1 else { panic!("ops") };
        let mut textmap: Option<(u32, Vec<u16>)> = None;
        for op in ops {
            let Value::List(op) = op else { panic!() };
            let name = op[0].as_str().unwrap();
            let (args, expected) = (&op[1], &op[2]);
            let where_ = format!("{format} {path} {name} {args:?}");
            let got = match name {
                "extract_text" | "extract_words" => {
                    result(call(&mut core, name, 0, list(vec![chars.clone(), py(args)]), None))
                }
                "dedupe_chars" => {
                    let Value::List(a) = args else { panic!() };
                    let got = result(call(&mut core, name, 0, list(vec![chars.clone(), py(&a[0]), py(&a[1])]), None));
                    // the oracle gives the indices of the kept chars
                    if let Value::List(indices) = expected {
                        let Js::Arr(all) = &chars_js else { panic!() };
                        let kept: Vec<Js> = indices
                            .iter()
                            .map(|i| match i {
                                Value::Int(i) => all[i.as_i64().unwrap() as usize].clone(),
                                other => panic!("an index: {other:?}"),
                            })
                            .collect();
                        assert_eq!(got, Js::Arr(kept), "{where_}");
                        checked += 1;
                        continue;
                    }
                    got
                }
                "textmap" => {
                    if let Some((h, _)) = textmap.take() {
                        ok(call(&mut core, "free", h, PyValue::None, None));
                    }
                    let r = result(call(&mut core, name, 0, list(vec![chars.clone(), py(args)]), None));
                    match r {
                        Js::Arr(pair) => {
                            let (Js::Num(h), Js::Str(text)) = (&pair[0], &pair[1]) else { panic!() };
                            textmap = Some((*h as u32, text.clone()));
                            pair[1].clone()
                        }
                        other => other,
                    }
                }
                "search" => {
                    let Value::List(a) = args else { panic!() };
                    let (pattern, regex, case) =
                        (a[1].as_str().unwrap(), matches!(a[2], Value::Bool(true)), matches!(a[3], Value::Bool(true)));
                    let literal = !regex || !pattern.contains(|c: char| "\\^$.*+?()[]{}|/".contains(c));
                    let Some((h, text)) = &textmap else { panic!("search without a textmap") };
                    if !(literal && case) {
                        continue; // a regular expression: replayed by the JavaScript tests
                    }
                    let spans = literal_spans(text, &pattern.encode_utf16().collect::<Vec<_>>());
                    let args =
                        list(vec![PyValue::from(i64::from(*h)), spans_value(&spans), py(&a[4]), py(&a[5]), py(&a[6])]);
                    result(call(&mut core, "textmap_search", 0, args, None))
                }
                "lines" => {
                    let Value::List(a) = args else { panic!() };
                    let (h, _) = textmap.as_ref().expect("lines without a textmap");
                    let args = list(vec![PyValue::from(i64::from(*h)), py(&a[1]), py(&a[2])]);
                    result(call(&mut core, "textmap_lines", 0, args, None))
                }
                "find_tables" => {
                    let kwargs = PyValue::Dict(pdfplumber::obj::Obj::from_pairs([
                        ("x_tolerance", PyValue::from(3_i64)),
                        ("y_tolerance", PyValue::from(3_i64)),
                    ]));
                    let args = list(vec![chars.clone(), edges.clone(), bbox.clone(), py(args), kwargs]);
                    result(call(&mut core, name, 0, args, None))
                }
                other => panic!("unknown op {other}"),
            };
            assert_eq!(got, value_view(expected), "{where_}");
            checked += 1;
        }
        if let Some((h, _)) = textmap {
            ok(call(&mut core, "free", h, PyValue::None, None));
        }
    }
    let Js::Obj(info) = ok(call(&mut core, "info", 0, PyValue::None, None)) else { panic!() };
    assert_eq!(info[3].1, Js::Num(0.0), "every handle freed");
    checked
}

#[test]
fn text_words_tables_over_objects_match_pdfplumber() {
    for &format in FORMATS {
        let checked = on_thread(256 << 20, move || replay_text(format));
        assert!(checked > 150, "{format}: {checked} operations");
    }
}
