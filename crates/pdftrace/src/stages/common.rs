//! Helpers shared by the stages, the Rust side of
//! `oracle/stages/_common.py`.

use std::cell::Cell;
use std::collections::HashSet;
use std::io::{self, Write};
use std::rc::Rc;

use pdfminer::pdfdocument::PDFDocument;
use pdfminer::pdfparser::FileKind;
use pdfminer::pdftypes::ObjId;
use pycompat::recursion::{py_frames, with_depth, Depth};
use pycompat::PyResult;

use super::{StageInput, STAGE_ENTRY};
use crate::canon::{canon_exc, canon_obj};

/// The lines of a trace being written.
#[derive(Default)]
pub struct Lines(pub String);

impl Lines {
    /// Appends `text` as a line.
    pub fn push(&mut self, text: impl AsRef<str>) {
        self.0.push_str(text.as_ref());
        self.0.push('\n');
    }

    /// `line(label, *values)`: the label and the values, canonical texts
    /// already, joined by spaces.
    pub fn line(&mut self, label: &str, values: &[&str]) {
        self.0.push_str(label);
        for v in values {
            self.0.push(' ');
            self.0.push_str(v);
        }
        self.0.push('\n');
    }
}

thread_local! {
    /// The depth [`terminated`] runs a stage body from on this thread
    /// ([`with_stage_entry`]), `None` for the oracle's (`STAGE_ENTRY`).
    static ENTRY: Cell<Option<Depth>> = const { Cell::new(None) };
}

/// Runs `f` with the stage bodies starting at `entry` instead of the
/// oracle's stage entry depth: a unit replay of a trace that its generator
/// computed from a deeper call site (`oracle/unit/interp.py`, the depth
/// windows from 1 or 2 more frames).
pub fn with_stage_entry<R>(entry: Depth, f: impl FnOnce() -> R) -> R {
    let previous = ENTRY.with(|e| e.replace(Some(entry)));
    let result = f();
    ENTRY.with(|e| e.set(previous));
    result
}

/// `terminated`: runs a stage body from the oracle's stage entry depth and
/// ends the trace with `!end`, or with the line of the exception that
/// escaped the body. As the oracle, it starts from a new Python process
/// ([`super::py_process`]) with logging disabled (`logging.disable`),
/// unless the trace goes on in the current one
/// ([`super::py_process::continuing`]).
pub fn terminated(out: &mut dyn Write, body: impl FnOnce(&mut Lines) -> PyResult<()>) -> io::Result<()> {
    if !super::py_process::is_continuing() {
        super::py_process::start();
        pycompat::logging::disable();
    }
    let mut lines = Lines::default();
    let entry = ENTRY.with(Cell::get).unwrap_or(STAGE_ENTRY);
    let result = with_depth(entry, || body(&mut lines));
    match result {
        Ok(()) => lines.push("!end"),
        Err(e) => lines.push(canon_exc(&e)),
    }
    out.write_all(lines.0.as_bytes())
}

/// `open_document(path, password)`: `PDFDocument(PDFParser(open(path,
/// "rb")), password=password or "")`, entered through the context
/// manager's `__enter__` and its generator (two frames).
pub fn open_document(input: &StageInput<'_>) -> PyResult<Rc<PDFDocument>> {
    let _frames = py_frames(2)?;
    open_file(input, FileKind::BufferedReader)
}

/// `PDFDocument(PDFParser(fp), password=password or "")` on a file object
/// of class `file` holding the PDF.
pub fn open_file(input: &StageInput<'_>, file: FileKind) -> PyResult<Rc<PDFDocument>> {
    PDFDocument::open(Rc::from(input.pdf), file, input.password)
}

/// `fetch_order(objids)` of `oracle/stages/objects.py`: the distinct ids
/// (in the order first seen), `int` ids sorted first, then the others in
/// the order they were seen.
pub fn fetch_order(objids: &[ObjId]) -> Vec<ObjId> {
    let mut seen: HashSet<ObjId> = HashSet::new();
    let distinct: Vec<ObjId> = objids.iter().filter(|id| seen.insert((*id).clone())).cloned().collect();
    let mut ints: Vec<ObjId> = distinct.iter().filter(|id| id.as_int().is_some()).cloned().collect();
    ints.sort_by(|a, b| a.as_int().cmp(&b.as_int()));
    ints.extend(distinct.into_iter().filter(|id| id.as_int().is_none()));
    ints
}

/// `canon(objid)`
pub fn canon_id(id: &ObjId) -> String {
    canon_obj(id.obj())
}

/// `interp.page_layout` of `oracle/stages/interp.py`, shared by the
/// `fonts`, `interp` and `layout` stages: the body of
/// pdfplumber/page.py:256 `Page.layout` with `laparams=None`, which the
/// library has (`pdfplumber::page::page_layout`: `Page::layout` drives
/// pages with it).
pub use pdfplumber::page::page_layout;
