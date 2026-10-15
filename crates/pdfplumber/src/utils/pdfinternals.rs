//! Port of `pdfplumber/utils/pdfinternals.py` (partial).
//!
//! Ported: `decode_text`, `resolve`, `get_dict_type`, `resolve_all` (what
//! `Page.__init__` reads its boxes through). Not ported yet:
//! `resolve_and_decode` and `decode_psl_list` (metadata and structure
//! tree, whose values are not PDF objects).
//!
//! `resolve_all` recurses as upstream does, one Python frame per level
//! (and the generator expression's for a list), so a reference cycle ends
//! in `RecursionError`, which it turns into `MalformedPDFException`, as in
//! Python.

use pdfminer::pdftypes::{PDFDict, PDFList, PDFObject};
use pdfminer::psparser::LiteralName;
use pdfminer::utils::PDF_DOC_ENCODING;
use pycompat::recursion::{eval_entry, py_frame};
use pycompat::{ExcKind, PyErr, PyResult};

/// `decode_text(s)` of a `str` or `bytes` (a literal's name).
pub enum TextArg<'a> {
    Str(&'a str),
    Bytes(&'a [u8]),
}

// pdfplumber/utils/pdfinternals.py:10 decode_text
/// Decodes a PDFDocEncoding string to Unicode. Adds py3 compatibility to
/// pdfminer's version.
pub fn decode_text(s: TextArg<'_>) -> PyResult<String> {
    let _frame = py_frame()?;
    match s {
        TextArg::Bytes(b) => {
            if let Some(body) = b.strip_prefix(b"\xfe\xff") {
                // str(s[2:], "utf-16be", "ignore")
                let units = body.chunks_exact(2).map(|p| u16::from_be_bytes([p[0], p[1]]));
                return Ok(char::decode_utf16(units).filter_map(Result::ok).collect());
            }
            Ok(b.iter().map(|&c| doc_char(u32::from(c))).collect())
        }
        TextArg::Str(s) => {
            // PDFDocEncoding[ord(c)]: an IndexError past 255, then str(s)
            if s.chars().any(|c| u32::from(c) > 255) {
                return Ok(s.to_owned());
            }
            Ok(s.chars().map(|c| doc_char(u32::from(c))).collect())
        }
    }
}

/// `PDFDocEncoding[o]` for `o < 256`.
fn doc_char(o: u32) -> char {
    char::from_u32(u32::from(PDF_DOC_ENCODING[o as usize])).expect("no surrogate in PDFDocEncoding")
}

// pdfplumber/utils/pdfinternals.py:48 resolve
pub fn resolve(x: &PDFObject) -> PyResult<PDFObject> {
    let _frame = py_frame()?;
    match x {
        PDFObject::Ref(r) => r.resolve(&PDFObject::Null),
        other => Ok(other.clone()),
    }
}

/// The result of [`get_dict_type`]: `None`, the decoded name of a literal
/// `/Type`, or the `/Type` value itself.
pub enum DictType {
    None,
    Name(String),
    Other(PDFObject),
}

impl DictType {
    /// `get_dict_type(d) == name`
    pub fn is(&self, name: &str) -> bool {
        matches!(self, DictType::Name(n) if n == name)
    }
}

// pdfplumber/utils/pdfinternals.py:55 get_dict_type
pub fn get_dict_type(d: &PDFObject) -> PyResult<DictType> {
    let _frame = py_frame()?;
    let PDFObject::Dict(d) = d else {
        return Ok(DictType::None);
    };
    Ok(match d.get("Type") {
        None => DictType::None,
        Some(PDFObject::Literal(lit)) => DictType::Name(match lit.name() {
            LiteralName::Str(s) => decode_text(TextArg::Str(s))?,
            LiteralName::Bytes(b) => decode_text(TextArg::Bytes(b))?,
        }),
        Some(t) => DictType::Other(t),
    })
}

// pdfplumber/utils/pdfinternals.py:66 resolve_all
/// Recursively resolves the given object and all the internals: new lists
/// and dicts, a reference to a `/Page` dict left as it is, and the
/// `/Parent` of an `/Annot` dict left unresolved.
pub fn resolve_all(x: &PDFObject) -> PyResult<PDFObject> {
    let _frame = py_frame()?;
    match x {
        PDFObject::Ref(r) => {
            let resolved = r.resolve(&PDFObject::Null)?;
            // Avoid infinite recursion
            if get_dict_type(&resolved)?.is("Page") {
                return Ok(x.clone());
            }
            match resolve_all(&resolved) {
                Err(e) if e.is_instance(ExcKind::RecursionError) => {
                    Err(PyErr::new(ExcKind::MalformedPDFException, e.message.clone()))
                }
                other => other,
            }
        }
        PDFObject::List(l) => {
            // type(x)(resolve_all(v) for v in x): list() runs the generator
            // expression from C, a frame of its own
            let _entry = eval_entry()?;
            let _genexpr = py_frame()?;
            let mut items = Vec::with_capacity(l.len());
            let mut i = 0;
            while let Some(v) = l.get(i) {
                items.push(resolve_all(&v)?);
                i += 1;
            }
            Ok(PDFObject::List(PDFList::new(items)))
        }
        PDFObject::Dict(d) => {
            let exceptions: &[&str] = if get_dict_type(x)?.is("Annot") { &["Parent"] } else { &[] };
            // a dict comprehension (inlined, PEP 709: no frame)
            let out = PDFDict::new();
            let mut i = 0;
            while let Some((k, v)) = d.get_index(i) {
                let v = if exceptions.contains(&&*k) { v } else { resolve_all(&v)? };
                out.insert(k, v);
                i += 1;
            }
            Ok(PDFObject::Dict(out))
        }
        other => Ok(other.clone()),
    }
}
