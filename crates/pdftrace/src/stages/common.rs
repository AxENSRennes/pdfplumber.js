//! Helpers shared by the stages, the Rust side of
//! `oracle/stages/_common.py`.

use std::collections::HashSet;
use std::io::{self, Write};
use std::rc::Rc;

use pdfminer::pdfdocument::PDFDocument;
use pdfminer::pdfparser::FileKind;
use pdfminer::pdftypes::{ObjId, PDFObject};
use pycompat::recursion::{py_frames, with_depth};
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

/// `terminated`: runs a stage body from the oracle's stage entry depth and
/// ends the trace with `!end`, or with the line of the exception that
/// escaped the body.
pub fn terminated(out: &mut dyn Write, body: impl FnOnce(&mut Lines) -> PyResult<()>) -> io::Result<()> {
    let mut lines = Lines::default();
    let result = with_depth(STAGE_ENTRY, || body(&mut lines));
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
    PDFDocument::open(Rc::from(input.pdf), FileKind::BufferedReader, input.password)
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

/// `canon(obj)` of an optional object (`None` when absent).
pub fn canon_opt(obj: Option<&PDFObject>) -> String {
    obj.map_or_else(|| "None".to_owned(), canon_obj)
}
