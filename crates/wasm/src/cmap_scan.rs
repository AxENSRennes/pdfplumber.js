//! CMap preloading (`docs/js-runtime.md`, "CMaps"): the predefined CMaps a
//! document will probably ask for, so that its first void call (`crate::ops`,
//! "Void calls") asks JavaScript for all of them at once, instead of one void
//! call per CMap, and a document that never misses one costs nothing.
//!
//! At the first void call of a document, the core reads it again, apart
//! from its `PDF` (a new `PDFDocument` of the same bytes and password, its
//! page tree and the fonts of each node's resources: no content stream or
//! XObject, whose data would be copied for nothing; a font used only in a
//! Form XObject is loaded lazily), and lists the names of the predefined
//! CMaps its fonts name: the `/Encoding` of a `Type0` font
//! (or the `/UseCMap` of an embedded CMap), and the unicode map of a CID
//! font's `/CIDSystemInfo` (`to-unicode-<Registry>-<Ordering>`, what
//! `CMapDB.get_unicode_map` loads), among those JavaScript can provide
//! and has not handed over yet. A guess: a name it misses is still loaded
//! lazily, a name it adds is loaded for nothing.
//!
//! The scan has no Python counterpart and no Python effect: the modeled state of
//! the process (`pdfminer::process_state`: names interned, sites
//! specialized, caches) is put back after it, its diagnostics are
//! dropped, its errors (a document that cannot be read again, a
//! `RecursionError`) end the scan with what it found, it records nothing in
//! the history of the document, and it loads no CMap. So a CMap present
//! from the start is Python with every CMap installed (the reference):
//! results and recursion are those of the lazy loading.

use std::collections::BTreeSet;
use std::rc::Rc;

use pdfminer::pdfdocument::PDFDocument;
use pdfminer::pdfparser::FileKind;
use pdfminer::pdftypes::{PDFDict, PDFObject};
use pdfminer::process_state::ProcessState;
use pdfminer::psparser::LiteralName;
use pdfplumber::obj::PyValue;
use pycompat::diagnostics::{self, Level};
use pycompat::PyResult;
use rustc_hash::FxHashSet;

/// The most nodes of the page tree a scan reads.
pub const MAX_OBJECTS: usize = 1 << 20;

/// The names of the predefined CMaps the fonts of the document `data`
/// (opened with `options`, those of `pdfplumber.open`) name, among `known`
/// (see the module documentation), sorted.
pub fn scan(data: &Rc<[u8]>, options: &PyValue, known: &dyn Fn(&str) -> bool) -> Vec<String> {
    let process = ProcessState::capture();
    let mut found = BTreeSet::new();
    // its diagnostics are collected here, and dropped
    let (_, _) = diagnostics::collect(Level::Critical, 0, || drop(scan_objects(data, options, &mut found)));
    process.restore();
    found.into_iter().filter(|n| known(n)).collect()
}

/// The password `pdfplumber.open` gives `PDFDocument` (`password or ""`);
/// a password that is not a `str` reads as `""`.
fn password(options: &PyValue) -> String {
    match options {
        PyValue::Dict(d) => match d.get("password") {
            Some(PyValue::Str(s)) => s.to_string_lossy(),
            _ => String::new(),
        },
        _ => String::new(),
    }
}

fn scan_objects(data: &Rc<[u8]>, options: &PyValue, found: &mut BTreeSet<String>) -> PyResult<()> {
    let doc = PDFDocument::open(data.clone(), FileKind::BytesIO, &password(options))?;
    // the page tree, from the catalog: the fonts of each node's resources
    // (no content stream, no XObject is read: their data would be copied
    // for nothing)
    let mut stack: Vec<PDFObject> = doc.catalog().get("Pages").into_iter().collect();
    let mut seen: FxHashSet<String> = FxHashSet::default();
    let mut nodes = 0;
    while let Some(node) = stack.pop() {
        if let PDFObject::Ref(r) = &node {
            if !seen.insert(r.objid().to_string()) {
                continue;
            }
        }
        nodes += 1;
        if nodes > MAX_OBJECTS {
            break;
        }
        let Some(d) = resolved(&node).as_ref().and_then(dict_of) else { continue };
        if let Some(fonts) = d.get("Resources").as_ref().and_then(resolved).as_ref().and_then(dict_of) {
            if let Some(fonts) = fonts.get("Font").as_ref().and_then(resolved).as_ref().and_then(dict_of) {
                for (_, font) in fonts.items() {
                    if let Some(font) = resolved(&font) {
                        font_cmaps(&font, found);
                    }
                }
            }
        }
        if let Some(PDFObject::List(kids)) = d.get("Kids").as_ref().and_then(resolved) {
            stack.extend(kids.to_vec().into_iter().rev());
        }
    }
    Ok(())
}

/// The value of a name (`/Identity-H`), `None` for anything else.
fn name(o: &PDFObject) -> Option<String> {
    match o {
        PDFObject::Literal(l) => match l.name() {
            LiteralName::Str(s) => Some(s.to_owned()),
            LiteralName::Bytes(_) => None,
        },
        _ => None,
    }
}

/// A dict of an object (the dict itself, or a stream's).
fn dict_of(o: &PDFObject) -> Option<PDFDict> {
    match o {
        PDFObject::Dict(d) => Some(d.clone()),
        PDFObject::Stream(s) => Some(s.attrs.clone()),
        _ => None,
    }
}

/// `x`, resolved if it is a reference (an error: `None`).
fn resolved(x: &PDFObject) -> Option<PDFObject> {
    match x {
        PDFObject::Ref(r) => r.resolve(&PDFObject::Null).ok(),
        other => Some(other.clone()),
    }
}

/// The CMaps a font object names (see the module documentation).
fn font_cmaps(obj: &PDFObject, found: &mut BTreeSet<String>) {
    let Some(d) = dict_of(obj) else { return };
    // an embedded CMap that uses a predefined one
    if let Some(n) = d.get("UseCMap").as_ref().and_then(name) {
        found.insert(n);
    }
    if d.get("Type").as_ref().and_then(name).as_deref() != Some("Font") {
        return;
    }
    match d.get("Subtype").as_ref().and_then(name).as_deref() {
        Some("Type0") => {
            if let Some(enc) = d.get("Encoding").as_ref().and_then(resolved) {
                match name(&enc) {
                    Some(n) => {
                        found.insert(n);
                    }
                    None => {
                        if let Some(n) = dict_of(&enc).and_then(|e| e.get("UseCMap")).as_ref().and_then(name) {
                            found.insert(n);
                        }
                    }
                }
            }
            if let Some(PDFObject::List(fonts)) = d.get("DescendantFonts").as_ref().and_then(resolved) {
                for f in fonts.to_vec() {
                    if let Some(f) = resolved(&f).as_ref().and_then(dict_of) {
                        system_info(&f, found);
                    }
                }
            }
        }
        Some("CIDFontType0" | "CIDFontType2") => system_info(&d, found),
        _ => {}
    }
}

/// The unicode map of a CID font's `/CIDSystemInfo`.
fn system_info(font: &PDFDict, found: &mut BTreeSet<String>) {
    let Some(info) = font.get("CIDSystemInfo").as_ref().and_then(resolved).as_ref().and_then(dict_of) else {
        return;
    };
    let text = |k: &str| match info.get(k).as_ref().and_then(resolved) {
        Some(PDFObject::Bytes(b)) => Some(b.iter().map(|&c| char::from(c)).collect::<String>()),
        Some(o) => name(&o),
        None => None,
    };
    if let (Some(registry), Some(ordering)) = (text("Registry"), text("Ordering")) {
        found.insert(format!("to-unicode-{registry}-{ordering}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfplumber::obj::Obj;

    /// The scan opens an encrypted document with the password of `open()`
    /// (`password-example.pdf`, upstream's: password `test`).
    #[test]
    fn the_scan_reads_an_encrypted_document_with_its_password() {
        let root = std::path::PathBuf::from(std::env::var("PDFPLUMBER_ROOT").expect("PDFPLUMBER_ROOT"));
        let data: Rc<[u8]> =
            Rc::from(std::fs::read(root.join("upstream/pdfplumber-python/tests/pdfs/password-example.pdf")).unwrap());
        let options = |p: PyValue| PyValue::Dict(Obj::from_pairs([("password", p)]));
        let mut found = BTreeSet::new();
        assert!(scan_objects(&data, &options(PyValue::str("test")), &mut found).is_ok());
        assert!(scan_objects(&data, &options(PyValue::str("wrong")), &mut found).is_err());
        assert!(scan_objects(&data, &options(PyValue::from(1_i64)), &mut found).is_err(), "not a str: \"\"");
        assert!(scan_objects(&data, &PyValue::None, &mut found).is_err());
    }
}
