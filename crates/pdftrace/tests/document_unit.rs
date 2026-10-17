//! Replays `oracle/unit/document.py`: the stage traces of the document
//! layer (`pdfparser`, `pdfdocument`, `pdfpage`, pdfplumber's page boxes)
//! on the adversarial corpora `adv-objects` and `adv-pages`, `micro` and
//! small files of `quick`; the other methods of the document (`api`:
//! outlines, page labels, names, destinations, `get_pages`); `bytes %
//! int`; the fallback scan's object header pattern.
//!
//! A difference prints the case and the first differing line of the
//! trace. `DOCUMENT_ONLY=<substring>` restricts the trace cases to the
//! file names containing it.
//!
//! The generator computes each trace in a process forked for it, whose
//! state (`py_process.jsonl`: the interned symbols) each
//! replayed trace starts from (`stages::py_process`).

use std::collections::HashSet;
use std::io::{self, Write};
use std::rc::Rc;

use pdfminer::pdfdocument::{pdfobj_cue, PDFDocument, PyVal};
use pdfminer::pdfpage::PDFPage;
use pdfminer::pdfparser::FileKind;
use pdfminer::pdftypes::PDFObject;
use pdftrace::canon::{canon, canon_exc, canon_obj};
use pdftrace::stages::common::{terminated, Lines};
use pdftrace::stages::py_process::{self, PyProcess};
use pdftrace::stages::{self, objects, StageInput};
use pdftrace::unit::UnitData;
use pdftrace::value::{PyStr, Value};
use pycompat::bytes_format::bytes_mod_int;
use pycompat::{ExcKind, PyResult};

/// `OUTLINES_MAX`, `LABELS_MAX`, `PAGES_MAX` of the generator.
const OUTLINES_MAX: usize = 60;
const LABELS_MAX: usize = 12;
const PAGES_MAX: usize = 12;

fn str_value(s: &str) -> Value {
    Value::Str(PyStr::from(s))
}

/// `api_trace` of `oracle/unit/document.py`.
fn api_trace(input: &StageInput<'_>, out: &mut dyn Write) -> io::Result<()> {
    terminated(out, |lines: &mut Lines| {
        let doc = open_api_document(input)?;
        match doc.get_outlines() {
            Err(e) => lines.push(format!("outlines {}", canon_exc(&e))),
            Ok(outlines) => {
                let mut error = None;
                for entry in outlines.take(OUTLINES_MAX) {
                    match entry {
                        Ok((level, title, dest, action, se)) => lines.line(
                            "outline",
                            &[
                                &level.to_string(),
                                &canon(&str_value(&title)),
                                &canon_obj(&dest),
                                &canon_obj(&action),
                                &canon_obj(&se),
                            ],
                        ),
                        Err(e) => {
                            error = Some(e);
                            break;
                        }
                    }
                }
                lines.push(match error {
                    Some(e) => format!("outlines {}", canon_exc(&e)),
                    None => "outlines end".to_owned(),
                });
            }
        }
        match doc.get_page_labels() {
            Err(e) => lines.push(format!("labels {}", canon_exc(&e))),
            Ok(labels) => {
                let mut error = None;
                for label in labels.take(LABELS_MAX) {
                    match label {
                        Ok(label) => lines.line("label", &[&canon(&str_value(&label))]),
                        Err(e) => {
                            error = Some(e);
                            break;
                        }
                    }
                }
                lines.push(match error {
                    Some(e) => format!("labels {}", canon_exc(&e)),
                    None => "labels end".to_owned(),
                });
            }
        }
        let bytes = |b: &[u8]| PyVal::Obj(PDFObject::Bytes(Rc::from(b)));
        let text = |s: &str| PyVal::Str(Rc::from(s));
        let key_canon = |k: &PyVal| match k {
            PyVal::Obj(o) => canon_obj(o),
            PyVal::Str(s) => canon(&str_value(s)),
        };
        for (cat, key) in [
            ("Dests", bytes(b"a")),
            ("Dests", bytes(b"b")),
            ("Dests", text("a")),
            ("Dests", bytes(b"zz")),
            ("JavaScript", bytes(b"a")),
        ] {
            let value = match doc.lookup_name(cat, &key) {
                Ok(v) => canon_obj(&v),
                Err(e) => canon_exc(&e),
            };
            lines.line("lookup", &[&canon(&str_value(cat)), &key_canon(&key), &value]);
        }
        for name in [bytes(b"a"), text("a"), text("b"), bytes(b"zz"), text("zz")] {
            let value = match doc.get_dest(&name) {
                Ok(v) => canon_obj(&v),
                Err(e) => canon_exc(&e),
            };
            lines.line("dest", &[&key_canon(&name), &value]);
        }
        for (pagenos, maxpages) in [(None, 0), (Some(vec![1, 2]), 0), (None, 2), (Some(vec![0]), 1)] {
            let set: Option<HashSet<usize>> = pagenos.as_ref().map(|p: &Vec<usize>| p.iter().copied().collect());
            let ids = (|| -> PyResult<Vec<Value>> {
                let pages = PDFPage::get_pages(
                    Rc::from(input.pdf),
                    FileKind::BufferedReader,
                    "<_io.BufferedReader>",
                    set.as_ref(),
                    maxpages,
                    input.password,
                    true,
                    false,
                )?;
                pages.map(|p| p.map(|p| Value::from(&p.pageid))).collect()
            })();
            let value = match ids {
                Ok(ids) => canon(&Value::List(ids)),
                Err(e) => canon_exc(&e),
            };
            let pagenos = match &pagenos {
                Some(p) => canon(&Value::List(p.iter().map(|&n| Value::Int((n as i64).into())).collect())),
                None => "None".to_owned(),
            };
            lines.line("getpages", &[&pagenos, &maxpages.to_string(), &value]);
        }
        let checked = (|| -> PyResult<Vec<Value>> {
            let pages = PDFPage::get_pages(
                Rc::from(input.pdf),
                FileKind::BufferedReader,
                "<_io.BufferedReader>",
                None,
                0,
                input.password,
                true,
                true,
            )?;
            pages.map(|p| p.map(|p| Value::from(&p.pageid))).collect()
        })();
        let value = match checked {
            Ok(ids) => canon(&Value::List(ids)),
            // the class alone (the message holds the file name)
            Err(e) if e.is_instance(ExcKind::PDFTextExtractionNotAllowed) => "PDFTextExtractionNotAllowed".to_owned(),
            Err(e) => canon_exc(&e),
        };
        lines.push(format!("getpages-checked {value}"));
        let pages = PDFPage::get_pages(
            Rc::from(input.pdf),
            FileKind::BufferedReader,
            "<_io.BufferedReader>",
            None,
            0,
            input.password,
            true,
            false,
        );
        let mut error = None;
        match pages {
            Err(e) => error = Some(e),
            Ok(pages) => {
                for page in pages.take(PAGES_MAX) {
                    match page.and_then(|p| p.repr()) {
                        Ok(r) => lines.line("page", &[&canon(&str_value(&r))]),
                        Err(e) => {
                            error = Some(e);
                            break;
                        }
                    }
                }
            }
        }
        lines.push(match error {
            Some(e) => format!("pages {}", canon_exc(&e)),
            None => "pages end".to_owned(),
        });
        Ok(())
    })
}

/// `PDFDocument(PDFParser(fp), password=...)` on an open file, inside
/// `with open(path, "rb") as fp:` (no frame).
fn open_api_document(input: &StageInput<'_>) -> PyResult<Rc<PDFDocument>> {
    pdftrace::stages::common::open_file(input, FileKind::BufferedReader)
}

/// Runs `what` on one PDF, as the generator did.
fn trace(what: &str, input: &StageInput<'_>) -> String {
    let mut out = Vec::new();
    match what {
        "objects-bytesio" => objects::run_bytesio(input, &mut out),
        "api" => api_trace(input, &mut out),
        stage => (stages::lookup(stage).and_then(|s| s.run).expect("an implemented stage"))(input, &mut out),
    }
    .expect("write to memory");
    String::from_utf8(out).expect("ASCII")
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
    &s[..s.len().min(400)]
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

#[test]
fn stage_traces() {
    let data = UnitData::open("document");
    py_process::set(generator_process(&data));
    let only = std::env::var("DOCUMENT_ONLY").ok();
    let mut failures = Vec::new();
    let mut n = 0;
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
        let input = StageInput { pdf, password: &password, max_pages };
        let what = what.as_str().expect("a stage");
        let got = trace(what, &input);
        let Value::Str(expected) = &expected else { panic!("bad output") };
        let expected = expected.as_str().expect("ASCII");
        n += 1;
        if got != expected {
            failures.push(format!("{name} [{what}]: {}", first_difference(expected, &got)));
        }
    }
    assert!(n > 1000, "only {n} traces");
    assert!(
        failures.is_empty(),
        "{} of {n} traces differ:\n{}",
        failures.len(),
        failures[..failures.len().min(20)].join("\n")
    );
}

#[test]
fn bytes_format() {
    let data = UnitData::open("document");
    let mut n = 0;
    for (args, expected) in data.cases("bytes_format") {
        let [Value::Bytes(fmt), Value::Int(arg)] = &args[..] else { panic!("bad case {args:?}") };
        let got = match bytes_mod_int(fmt, arg.as_i64().expect("a small int")) {
            Ok(b) => Value::Bytes(b),
            Err(e) => Value::from(&e),
        };
        assert_eq!(canon(&got), canon(&expected), "{:?} % {arg}", String::from_utf8_lossy(fmt));
        n += 1;
    }
    assert!(n > 4000);
}

#[test]
fn pdfobj_cue_matches() {
    let data = UnitData::open("document");
    let mut n = 0;
    for (args, expected) in data.cases("pdfobj_cue") {
        let [Value::Bytes(line)] = &args[..] else { panic!("bad case {args:?}") };
        let got = match pdfobj_cue(line) {
            Some((a, b)) => Value::List(vec![Value::Bytes(a.to_vec()), Value::Bytes(b.to_vec())]),
            None => Value::None,
        };
        assert_eq!(canon(&got), canon(&expected), "{line:?}");
        n += 1;
    }
    assert!(n > 3000);
}
