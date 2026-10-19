//! The lazy-CMap protocol of the boundary (`crates/wasm/src/cmaps.rs`,
//! `docs/architecture.md`, "Vague 3", "Pilotage") through the interpreter:
//! a predefined CMap that the provider does not have yet is recorded as
//! asked for even though pdfminer handles its absence (`CMapNotFound`,
//! caught by `PDFCIDFont.get_cmap_from_spec` and `get_unicode_map`, which
//! fall back on an empty `CMap()` and no unicode map), so the call can be
//! voided and re-run; the re-run, with the CMap handed over and a new
//! resource manager (the voided call's fonts thrown away), gives what a run
//! with every CMap installed from the start gives.
//!
//! The document is generated here: one page showing a Shift-JIS code with
//! a Type 0 font encoded by the predefined `90ms-RKSJ-H` (`Adobe-Japan1`,
//! no `/ToUnicode`, so the unicode map `to-unicode-Adobe-Japan1` too).
//! The CMaps are those of the `interp` unit data (`cmap_dir` of its
//! `meta.json`), the provider of the replays.

use std::cell::RefCell;
use std::collections::HashSet;
use std::fs;
use std::rc::Rc;

use pdfminer::cmapdb::{CMapDB, CMapProvider, DirCMapProvider};
use pdftrace::stages::{self, StageInput};
use pdftrace::typed_json::{self, Json};
use pdftrace::unit::{cache_dir, UnitData};

/// A provider that withholds some names as the JavaScript provider does
/// before they are fetched: it records them, in order, and answers as if
/// they did not exist.
struct Lazy {
    inner: DirCMapProvider,
    withheld: RefCell<HashSet<String>>,
    missing: RefCell<Vec<String>>,
}

impl CMapProvider for Lazy {
    fn load(&self, name: &str) -> Option<Vec<u8>> {
        if self.withheld.borrow().contains(name) {
            let mut missing = self.missing.borrow_mut();
            if !missing.iter().any(|m| m == name) {
                missing.push(name.to_owned());
            }
            return None;
        }
        self.inner.load(name)
    }
}

fn cmap_provider() -> DirCMapProvider {
    let data = UnitData::open("interp");
    let meta = fs::read_to_string(data.path("meta.json")).expect("meta.json");
    let meta = typed_json::parse_json(&meta).expect("meta.json is JSON");
    let Some(Json::String(rel)) = meta.get("cmap_dir") else { panic!("meta.json has no cmap_dir") };
    let dir = cache_dir().join(rel);
    DirCMapProvider::new(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
}

/// A one-page PDF, its xref table computed.
fn pdf() -> Vec<u8> {
    let content = b"BT /F1 12 Tf 10 20 Td <82A082A2> Tj ET";
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>"
            .to_vec(),
        [format!("<< /Length {} >>\nstream\n", content.len()).as_bytes(), content, b"\nendstream"].concat(),
        b"<< /Type /Font /Subtype /Type0 /BaseFont /HeiseiMin-W3 /Encoding /90ms-RKSJ-H /DescendantFonts [6 0 R] >>"
            .to_vec(),
        b"<< /Type /Font /Subtype /CIDFontType0 /BaseFont /HeiseiMin-W3 /CIDSystemInfo << /Registry (Adobe) /Ordering (Japan1) /Supplement 2 >> >>"
            .to_vec(),
    ];
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes(),
    );
    out
}

/// The trace of `stage` with `provider` (a new resource manager, as each
/// run of a stage makes, and `CMapDB`'s caches emptied first: a new
/// process).
fn run(stage: &str, pdf: &[u8], provider: Rc<dyn CMapProvider>) -> String {
    CMapDB::clear_caches();
    CMapDB::set_provider(Some(provider));
    let input = StageInput { pdf, password: "", max_pages: Some(20), oracle: None };
    let run = stages::lookup(stage).and_then(|s| s.run).expect("an implemented stage");
    let mut out = Vec::new();
    run(&input, &mut out).expect("write to memory");
    CMapDB::clear_provider();
    String::from_utf8(out).expect("an ASCII trace")
}

#[test]
fn a_missing_predefined_cmap_is_reported_and_the_rerun_is_complete() {
    let pdf = pdf();
    for stage in ["layout", "interp", "fonts"] {
        let complete = run(stage, &pdf, Rc::new(cmap_provider()));
        let lazy = Rc::new(Lazy {
            inner: cmap_provider(),
            withheld: RefCell::new(["90ms-RKSJ-H", "to-unicode-Adobe-Japan1"].map(String::from).into()),
            missing: RefCell::new(Vec::new()),
        });
        let voided = run(stage, &pdf, lazy.clone());
        assert_eq!(*lazy.missing.borrow(), ["90ms-RKSJ-H", "to-unicode-Adobe-Japan1"], "{stage}: the names asked for");
        assert_ne!(voided, complete, "{stage}: the CMaps change the page");
        // JavaScript hands the files over; the call is run again
        lazy.withheld.borrow_mut().clear();
        lazy.missing.borrow_mut().clear();
        let rerun = run(stage, &pdf, lazy.clone());
        assert!(lazy.missing.borrow().is_empty(), "{stage}: nothing missing on the re-run");
        assert_eq!(rerun, complete, "{stage}: the re-run is the complete run");
    }
}
