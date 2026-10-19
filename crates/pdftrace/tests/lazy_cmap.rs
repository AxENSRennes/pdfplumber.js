//! The lazy-CMap protocol of the boundary (`crates/wasm/src/cmaps.rs`,
//! `docs/architecture.md`, "Vague 3", "Pilotage") through the interpreter:
//! a predefined CMap that the provider does not have yet is recorded as
//! asked for even though pdfminer handles its absence (`CMapNotFound`,
//! caught by `PDFCIDFont.get_cmap_from_spec` and `get_unicode_map`, which
//! fall back on an empty `CMap()` and no unicode map), so the call can be
//! voided and re-run; the re-run, with the CMap handed over, a new
//! resource manager (the voided call's fonts thrown away) and the process
//! state the voided call started from (`pdfminer::process_state`), gives
//! what a run with every CMap installed from the start gives.
//!
//! The runs go on in one process, as the boundary's calls do
//! (`stages::py_process::continuing`); the complete run, the reference
//! (Python's, checked by `oracle.verify` and the `adv-interp` depth
//! windows), is a new process's. The documents are generated here:
//!
//! - one page showing a Shift-JIS code with a Type 0 font encoded by the
//!   predefined `90ms-RKSJ-H` (`Adobe-Japan1`, no `/ToUnicode`, so the
//!   unicode map `to-unicode-Adobe-Japan1` too);
//! - a page with a chain of nested Form XObjects whose deepest form shows
//!   the process's first inline image (`get_inline_data`'s `re.sub`
//!   compiles its pattern there, at depth), then a shallow page with an
//!   inline image and a Type 0 font encoded by `UniJIS-UCS2-H`: the voided
//!   run compiles the pattern on its second page, so a re-run that kept
//!   that state would not fail where the complete run does, at the chain
//!   lengths around the limit.
//!
//! The CMaps are those of the `interp` unit data (`cmap_dir` of its
//! `meta.json`), the provider of the replays.

use std::cell::RefCell;
use std::collections::HashSet;
use std::fs;
use std::rc::Rc;

use pdfminer::cmapdb::{CMapDB, CMapProvider, DirCMapProvider};
use pdfminer::process_state::ProcessState;
use pdftrace::stages::{self, py_process, StageInput};
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

/// A PDF of the given objects (numbered from 1; 1 is the catalog), its
/// xref table computed.
fn build(objects: &[Vec<u8>]) -> Vec<u8> {
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

/// A stream object: `dict` (without `/Length`) and `data`.
fn stream(dict: &str, data: &[u8]) -> Vec<u8> {
    [format!("<< {dict} /Length {} >>\nstream\n", data.len()).as_bytes(), data, b"\nendstream"].concat()
}

/// One page showing `<82A082A2>` with a Type 0 font encoded by `90ms-RKSJ-H`.
fn rksj_pdf() -> Vec<u8> {
    build(&[
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>"
            .to_vec(),
        stream("", b"BT /F1 12 Tf 10 20 Td <82A082A2> Tj ET"),
        b"<< /Type /Font /Subtype /Type0 /BaseFont /HeiseiMin-W3 /Encoding /90ms-RKSJ-H /DescendantFonts [6 0 R] >>"
            .to_vec(),
        b"<< /Type /Font /Subtype /CIDFontType0 /BaseFont /HeiseiMin-W3 /CIDSystemInfo << /Registry (Adobe) /Ordering (Japan1) /Supplement 2 >> >>"
            .to_vec(),
    ])
}

/// Page 1: a chain of `n` nested Form XObjects, the deepest showing an
/// inline image; page 2: an inline image, then `<3042>` with a Type 0 font
/// encoded by `UniJIS-UCS2-H`.
fn chain_pdf(n: usize) -> Vec<u8> {
    const FIRST_FORM: usize = 9;
    let image = b"BI /W 1 /H 1 ID x EI\n";
    let mut objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".to_vec(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << /XObject << /X {FIRST_FORM} 0 R >> >> /Contents 5 0 R >>"
        )
        .into_bytes(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << /Font << /F 7 0 R >> >> /Contents 6 0 R >>"
            .to_vec(),
        stream("", b"/X Do"),
        stream("", &[&image[..], b" BT /F 10 Tf <3042> Tj ET"].concat()),
        b"<< /Type /Font /Subtype /Type0 /BaseFont /Ryumin-Light /Encoding /UniJIS-UCS2-H /DescendantFonts [8 0 R] >>"
            .to_vec(),
        b"<< /Type /Font /Subtype /CIDFontType0 /BaseFont /Ryumin-Light /CIDSystemInfo << /Registry (Adobe) /Ordering (Japan1) /Supplement 2 >> >>"
            .to_vec(),
    ];
    for k in 0..n {
        let form = "/Type /XObject /Subtype /Form /BBox [0 0 10 10]";
        objects.push(if k + 1 < n {
            stream(
                &format!("{form} /Resources << /XObject << /X {} 0 R >> >>", FIRST_FORM + k + 1),
                b"0 0 m 1 1 l /X Do",
            )
        } else {
            stream(form, image)
        });
    }
    build(&objects)
}

/// One run of `stage` on `pdf` with `provider` (a new resource manager, as
/// each run of a stage makes), in the current process.
fn run_once(stage: &str, pdf: &[u8], provider: Rc<dyn CMapProvider>) -> String {
    CMapDB::set_provider(Some(provider));
    let input = StageInput { pdf, password: "", max_pages: Some(20), oracle: None };
    let run = stages::lookup(stage).and_then(|s| s.run).expect("an implemented stage");
    let mut out = Vec::new();
    run(&input, &mut out).expect("write to memory");
    CMapDB::clear_provider();
    String::from_utf8(out).expect("an ASCII trace")
}

/// The trace of `stage` in a new process with every CMap.
fn complete(stage: &str, pdf: &[u8]) -> String {
    run_once(stage, pdf, Rc::new(cmap_provider()))
}

/// What the boundary does in a new process: a run with `withheld` held
/// back (void), the state of the process put back, the re-run with every
/// CMap. Returns the voided trace, the names asked for, the re-run's trace,
/// and the trace of the same re-run from the state the voided run left.
struct Boundary {
    voided: String,
    asked: Vec<String>,
    rerun: String,
    rerun_unrestored: String,
}

fn void_and_rerun(stage: &str, pdf: &[u8], withheld: &[&str]) -> Boundary {
    py_process::start();
    pycompat::logging::disable();
    let lazy = Rc::new(Lazy {
        inner: cmap_provider(),
        withheld: RefCell::new(withheld.iter().map(|s| (*s).to_owned()).collect()),
        missing: RefCell::new(Vec::new()),
    });
    py_process::continuing(|| {
        let before = ProcessState::capture();
        let voided = run_once(stage, pdf, lazy.clone());
        let asked = lazy.missing.take();
        let after_void = ProcessState::capture();
        // JavaScript hands the files over; the call is run again
        lazy.withheld.borrow_mut().clear();
        before.restore();
        let rerun = run_once(stage, pdf, lazy.clone());
        assert!(lazy.missing.borrow().is_empty(), "{stage}: nothing missing on the re-run");
        after_void.restore();
        let rerun_unrestored = run_once(stage, pdf, lazy.clone());
        Boundary { voided, asked, rerun, rerun_unrestored }
    })
}

#[test]
fn a_missing_predefined_cmap_is_reported_and_the_rerun_is_complete() {
    let pdf = rksj_pdf();
    for stage in ["layout", "interp", "fonts"] {
        let complete = complete(stage, &pdf);
        let b = void_and_rerun(stage, &pdf, &["90ms-RKSJ-H", "to-unicode-Adobe-Japan1"]);
        assert_eq!(b.asked, ["90ms-RKSJ-H", "to-unicode-Adobe-Japan1"], "{stage}: the names asked for");
        assert_ne!(b.voided, complete, "{stage}: the CMaps change the page");
        assert_eq!(b.rerun, complete, "{stage}: the re-run is the complete run");
    }
}

#[test]
fn the_rerun_starts_from_the_process_state_of_the_voided_call() {
    for stage in ["layout", "interp", "fonts"] {
        let mut sensitive = Vec::new();
        for n in 318..=336 {
            let pdf = chain_pdf(n);
            let complete = complete(stage, &pdf);
            let b = void_and_rerun(stage, &pdf, &["UniJIS-UCS2-H"]);
            assert_eq!(b.asked, ["UniJIS-UCS2-H"], "{stage} {n}: the names asked for");
            assert_eq!(b.rerun, complete, "{stage} {n}: the re-run is the complete run");
            if b.rerun_unrestored != complete {
                sensitive.push(n);
            }
        }
        // the case checks something: at some lengths, the state the voided
        // run left (the pattern compiled on its second page) changes the
        // re-run
        assert!(!sensitive.is_empty(), "{stage}: no chain length depends on the process state");
    }
}
