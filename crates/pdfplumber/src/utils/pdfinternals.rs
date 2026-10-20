//! Port of `pdfplumber/utils/pdfinternals.py` (partial).
//!
//! Ported: `decode_text` of a `str` or `bytes`, `resolve`, `get_dict_type`,
//! `resolve_all` (what `Page.__init__` reads its boxes through).
//!
//! Wave 4 (base): the interface of the rest, bodies `todo!()` (workstream
//! PAGE, `docs/architecture.md`, "Vague 4"): `decode_text` of any value
//! ([`decode_text_value`]: the structure tree decodes whatever `/ID`,
//! `/T`, `/Lang`, `/Alt`, `/ActualText` resolve to), `resolve_and_decode`
//! (metadata, `ncs` names) and `decode_psl_list`.
//!
//! `resolve_all` recurses as upstream does, one Python frame per level
//! (and the generator expression's for a list), so a reference cycle ends
//! in `RecursionError`, which it turns into `MalformedPDFException`, as in
//! Python.

use pdfminer::pdftypes::{PDFDict, PDFList, PDFObject};
use pdfminer::psparser::LiteralName;
use pdfminer::utils::PDF_DOC_ENCODING;
use pycompat::recursion::{eval_entry, py_frame};
use pycompat::{ExcKind, PyErr, PyResult, PyStr};

use crate::obj::PyValue;

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
    if let TextArg::Bytes(b) = &s {
        if let Some(body) = b.strip_prefix(b"\xfe\xff") {
            // str(s[2:], "utf-16be", "ignore"): not one of PyUnicode_Decode's
            // fast paths, so the codec registry's decoder runs,
            // encodings.utf_16_be.decode, a Python function called from C
            pycompat::recursion::c_called_leaf()?;
            let units = body.as_chunks::<2>().0.iter().map(|p| u16::from_be_bytes(*p));
            return Ok(char::decode_utf16(units).filter_map(Result::ok).collect());
        }
    }
    // `ords` and the generator `"".join` runs are created (a frame check
    // each), then join resumes the second from C, which resumes `ords`, two
    // frames down, once per character and once more (or up to the
    // IndexError): every check is at most two frames down, and nothing
    // between them can raise, so the deepest decides.
    let _entry = eval_entry()?;
    pycompat::recursion::leaf(2)?;
    match s {
        TextArg::Bytes(b) => Ok(b.iter().map(|&c| doc_char(u32::from(c))).collect()),
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

// pdfplumber/utils/pdfinternals.py:49 resolve
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

// pdfplumber/utils/pdfinternals.py:56 get_dict_type
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

// pdfplumber/utils/pdfinternals.py:10 decode_text
/// `decode_text(s)` of any value: a `bytes` starting with `\xfe\xff` as
/// UTF-16BE; otherwise each item of `s` (characters of a `str`, ints of
/// `bytes`, items of a `list`, keys of a `dict`...: `TypeError` for a value
/// that is not iterable) through `PDFDocEncoding[ord(c) if isinstance(c,
/// str) else c]` (`TypeError` for an index that is not an `int`), and on
/// the first `IndexError` (an int past 255, or negative past -256) `str(s)`,
/// which recurses into a nested value with C units: a call site of
/// `pycompat::specialize` (its first execution in a process takes one unit
/// more), to declare with `oracle/depth.py` `_call_sites` (which every
/// oracle cache's fingerprint covers: add it before building caches) and
/// `pdftrace::stages::py_process`.
pub fn decode_text_value(s: &PyValue) -> PyResult<PyStr> {
    let _ = s;
    todo!("pdfplumber/utils/pdfinternals.py:10 decode_text")
}

// pdfplumber/utils/pdfinternals.py:24 resolve_and_decode
/// Recursively resolve the metadata values: a reference resolved, a
/// `list` element by element (`map` from C), a `PSLiteral` or `str`/`bytes`
/// through `decode_text`, a `dict` value by value **in place** (Python
/// mutates the PDF's dict and returns it: a dict reached twice is decoded
/// twice), anything else as it is.
pub fn resolve_and_decode(obj: &PyValue) -> PyResult<PyValue> {
    let _ = obj;
    todo!("pdfplumber/utils/pdfinternals.py:24 resolve_and_decode")
}

// pdfplumber/utils/pdfinternals.py:42 decode_psl_list
pub fn decode_psl_list(list: &[PyValue]) -> PyResult<Vec<PyValue>> {
    let _ = list;
    todo!("pdfplumber/utils/pdfinternals.py:42 decode_psl_list")
}
