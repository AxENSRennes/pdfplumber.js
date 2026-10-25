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
//! CMaps its fonts name: the `/Encoding` of a `Type0` font, and the unicode
//! map of each of its descendants' `/CIDSystemInfo`
//! (`to-unicode-<Registry>-<Ordering>`, what
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
use pycompat::{PyResult, PyStr};
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

/// A dict.
fn dict_of(o: &PDFObject) -> Option<PDFDict> {
    match o {
        PDFObject::Dict(d) => Some(d.clone()),
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

/// The CMaps a font names (see the module documentation): a `Type0`
/// font (its `/Subtype`, as `PDFResourceManager.get_font` reads it), its
/// `/Encoding` name and the unicode map of each descendant font.
fn font_cmaps(obj: &PDFObject, found: &mut BTreeSet<String>) {
    let Some(d) = dict_of(obj) else { return };
    if d.get("Subtype").as_ref().and_then(name).as_deref() != Some("Type0") {
        return;
    }
    if let Some(n) = d.get("Encoding").as_ref().and_then(resolved).as_ref().and_then(name) {
        found.insert(n);
    }
    if let Some(PDFObject::List(fonts)) = d.get("DescendantFonts").as_ref().and_then(resolved) {
        for f in fonts.to_vec() {
            if let Some(f) = resolved(&f).as_ref().and_then(dict_of) {
                system_info(&f, found);
            }
        }
    }
}

/// The unicode map of a CID font's `/CIDSystemInfo`
/// (pdfminer/pdffont.py:1102 PDFCIDFont, `get_unicode_map(cidcoding)`:
/// the registry and ordering are `bytes`, decoded as Latin-1 and
/// stripped).
fn system_info(font: &PDFDict, found: &mut BTreeSet<String>) {
    let Some(info) = font.get("CIDSystemInfo").as_ref().and_then(resolved).as_ref().and_then(dict_of) else {
        return;
    };
    let text = |k: &str| match info.get(k).as_ref().and_then(resolved) {
        Some(PDFObject::Bytes(b)) => {
            let text = b.iter().map(|&c| char::from(c)).collect::<String>();
            Some(PyStr::from(text.as_str()).strip().to_string_lossy())
        }
        _ => None,
    };
    if let (Some(registry), Some(ordering)) = (text("Registry"), text("Ordering")) {
        found.insert(format!("to-unicode-{registry}-{ordering}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfplumber::obj::Obj;

    /// What the scan finds in a page tree (no xref table: pdfminer's
    /// fallback reads the objects): the `/Encoding` name of each `Type0`
    /// font, resolved, and the unicode map of each descendant's
    /// `/CIDSystemInfo` (Latin-1, stripped), in the fonts of every node
    /// (the tree's own resources too, a node met twice read once); nothing
    /// from a font of another subtype, an encoding that is not a name, a
    /// system info whose registry is not a string.
    #[test]
    fn the_scan_lists_the_names_the_fonts_of_the_page_tree_give() {
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R 4 0 R 3 0 R] /Count 2 /Resources << /Font << /T 9 0 R >> >> >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources << /Font << /A 5 0 R /B 6 0 R >> >> >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources 10 0 R >>",
            "<< /Type /Font /Subtype /Type0 /Encoding 11 0 R /DescendantFonts [7 0 R] >>",
            "<< /Type /Font /Subtype /Type1 /Encoding /UniJIS-UCS2-H /DescendantFonts [7 0 R] >>",
            "<< /Subtype /CIDFontType0 /CIDSystemInfo << /Registry (\\240Adobe ) /Ordering ( GB1\\n) >> >>",
            "<< /Subtype /CIDFontType2 /CIDSystemInfo << /Registry /Adobe /Ordering (Korea1) >> >>",
            "<< /Type /Font /Subtype /Type0 /Encoding (UniCNS-UCS2-H) /DescendantFonts [8 0 R] >>",
            "<< /Font << /C 12 0 R >> >>",
            "/UniGB-UCS2-H",
            "<< /Type /Font /Subtype /Type0 /Encoding /Identity-V /DescendantFonts 13 0 R >>",
            "[<< /CIDSystemInfo << /Registry (Adobe) /Ordering (Japan1) >> >>]",
        ];
        let mut pdf = String::from("%PDF-1.4\n");
        for (k, o) in objects.iter().enumerate() {
            pdf.push_str(&format!("{} 0 obj\n{o}\nendobj\n", k + 1));
        }
        pdf.push_str("trailer\n<< /Root 1 0 R /Size 14 >>\n%%EOF\n");
        let data: Rc<[u8]> = Rc::from(pdf.into_bytes());
        let mut found = BTreeSet::new();
        assert!(scan_objects(&data, &PyValue::None, &mut found).is_ok());
        let found: Vec<&str> = found.iter().map(String::as_str).collect();
        assert_eq!(found, ["Identity-V", "UniGB-UCS2-H", "to-unicode-Adobe-GB1", "to-unicode-Adobe-Japan1"], "found");
        // `known` filters them, sorted
        assert_eq!(
            scan(&data, &PyValue::None, &|n| n.starts_with("to-")),
            ["to-unicode-Adobe-GB1", "to-unicode-Adobe-Japan1"]
        );
    }

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
