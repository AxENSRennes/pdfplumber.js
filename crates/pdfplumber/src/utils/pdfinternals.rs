//! Port of `pdfplumber/utils/pdfinternals.py`.
//!
//! `decode_text` of a `str` or `bytes` ([`decode_text`]: a literal's name)
//! and of any value ([`decode_text_value`]: the structure tree decodes
//! whatever `/ID`, `/T`, `/Lang`, `/Alt`, `/ActualText` resolve to),
//! `resolve_and_decode` (metadata, `ncs` names), `decode_psl_list`,
//! `resolve`, `get_dict_type`, `resolve_all` (what `Page.__init__` reads its
//! boxes through, and `process_object` the attributes of layout objects).
//!
//! `resolve_all` recurses as upstream does, one Python frame per level
//! (and the generator expression's for a list), so a reference cycle ends
//! in `RecursionError`, which it turns into `MalformedPDFException(e)`
//! (the `RecursionError` kept as its cause), as in Python.

use pdfminer::pdftypes::{PDFDict, PDFList, PDFObject};
use pdfminer::psparser::LiteralName;
use pdfminer::utils::PDF_DOC_ENCODING;
use pycompat::pystr::PyStrBuilder;
use pycompat::recursion::{eval_entry, py_frame};
use pycompat::specialize::CallSite;
use pycompat::{exc, ExcKind, PyResult, PyStr};

use crate::obj::{py_iter, Obj, PyValue};

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
                    Err(crate::utils::exceptions::malformed_pdf_exception(e))
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
/// more: [`DECODE_TEXT_STR`]).
pub fn decode_text_value(s: &PyValue) -> PyResult<PyStr> {
    let _frame = py_frame()?;
    let bytes = match s {
        PyValue::Bytes(b) | PyValue::Pdf(PDFObject::Bytes(b)) => Some(b.clone()),
        _ => None,
    };
    if let Some(b) = &bytes {
        if let Some(body) = b.strip_prefix(b"\xfe\xff") {
            // str(s[2:], "utf-16be", "ignore"), from C (see decode_text)
            pycompat::recursion::c_called_leaf()?;
            let units = body.as_chunks::<2>().0.iter().map(|p| u16::from_be_bytes(*p));
            let text: String = char::decode_utf16(units).filter_map(Result::ok).collect();
            return Ok(PyStr::from(text));
        }
    }
    // `ords = (... for c in s)`: iter(s) runs where the generator
    // expression is created
    let items = match py_iter(s) {
        Some(items) => items?,
        None => return Err(exc::TypeError(format!("'{}' object is not iterable", s.type_name()))),
    };
    // the two generators created, then resumed by "".join from C (see
    // decode_text)
    let _entry = eval_entry()?;
    pycompat::recursion::leaf(2)?;
    // "".join materializes the generator first (PySequence_Fast): every
    // item is looked up before anything is joined
    let mut out = PyStrBuilder::with_capacity(items.len());
    for c in &items {
        let o = match c {
            // ord(c)
            PyValue::Str(ch) => {
                let n = ch.len();
                if n != 1 {
                    return Err(exc::TypeError(format!("ord() expected a character, but string of length {n} found")));
                }
                i64::from(ch.code_points().next().expect("one character"))
            }
            PyValue::Bool(b) | PyValue::Pdf(PDFObject::Bool(b)) => i64::from(*b),
            PyValue::Int(i) | PyValue::Pdf(PDFObject::Int(i)) => match i.as_i64() {
                Some(i) => i,
                // `cannot fit 'int' into an index-sized integer`: an IndexError
                None => return decode_text_str(s),
            },
            other => {
                return Err(exc::TypeError(format!("string indices must be integers, not '{}'", other.type_name())))
            }
        };
        // PDFDocEncoding[o]: a str of 256 characters
        let index = if o < 0 { o + 256 } else { o };
        if !(0..256).contains(&index) {
            return decode_text_str(s);
        }
        out.push(u32::from(PDF_DOC_ENCODING[index as usize]));
    }
    Ok(out.finish())
}

/// `except IndexError: return str(s)` of `decode_text`.
fn decode_text_str(s: &PyValue) -> PyResult<PyStr> {
    let _call = DECODE_TEXT_STR.str_call()?;
    s.str_counted()
}

// pdfplumber/utils/pdfinternals.py:10 decode_text
/// The call site of `return str(s)` in `decode_text`
/// (line 21 of pdfinternals.py), whose first execution in a
/// process takes one C unit more (`pycompat::specialize`): `oracle/depth.py`
/// `_call_sites` names it so, `pdftrace::stages::py_process::CALL_SITES`
/// lists it.
pub static DECODE_TEXT_STR: CallSite = CallSite::new("pdfinternals.decode_text str(s)");

// pdfplumber/utils/pdfinternals.py:24 resolve_and_decode
/// Recursively resolve the metadata values: a reference resolved (one
/// level), a `list` element by element (`list(map(resolve_and_decode,
/// obj))`: each call from C), a `PSLiteral` or `str`/`bytes` through
/// `decode_text`, a `dict` value by value, anything else as it is.
///
/// Python decodes a `dict` **in place** (`obj[k] = ...` on the PDF's own
/// dict, which it returns): a dict reached again (twice in the metadata,
/// or later by other code reading the document) holds the decoded values.
/// The port returns a new `dict` and leaves the PDF's as it is (a
/// `PDFObject` cannot hold a Python `str`): a dict reached twice is
/// decoded from its PDF values each time, where Python decodes what the
/// first pass left (the same text but for characters PDFDocEncoding maps
/// again: `\x16`, `\x7f`... of a UTF-16 string), and code reading the
/// document afterwards sees the PDF values (deviation noted in
/// `docs/architecture.md`, "Vague 4"; the metadata is workstream DOC's).
pub fn resolve_and_decode(obj: &PyValue) -> PyResult<PyValue> {
    let _frame = py_frame()?;
    // if hasattr(obj, "resolve"): obj = obj.resolve()
    let resolved;
    let obj = match obj {
        PyValue::Pdf(PDFObject::Ref(r)) => {
            resolved = PyValue::from_pdf(r.resolve(&PDFObject::Null)?);
            &resolved
        }
        other => other,
    };
    match obj {
        PyValue::List(items) => resolve_and_decode_list(items),
        PyValue::Pdf(PDFObject::List(l)) => {
            let items: Vec<PyValue> = l.to_vec().into_iter().map(PyValue::from_pdf).collect();
            resolve_and_decode_list(&items)
        }
        PyValue::Pdf(PDFObject::Literal(lit)) => Ok(PyValue::Str(PyStr::from(literal_text(lit.name())?))),
        PyValue::Str(_) | PyValue::Bytes(_) => Ok(PyValue::Str(decode_text_value(obj)?)),
        PyValue::Dict(d) => {
            let mut out = d.clone();
            for (k, v) in d.items() {
                let decoded = resolve_and_decode(v)?;
                out.set(k, decoded);
            }
            Ok(PyValue::Dict(out))
        }
        PyValue::Pdf(PDFObject::Dict(d)) => {
            let mut out = Obj::new();
            for (k, v) in d.items() {
                out.set(&*k, PyValue::from_pdf(v));
            }
            let items = out.items().to_vec();
            for (k, v) in &items {
                let decoded = resolve_and_decode(v)?;
                out.set(k, decoded);
            }
            Ok(PyValue::Dict(out))
        }
        other => Ok(other.clone()),
    }
}

/// `list(map(resolve_and_decode, obj))`
fn resolve_and_decode_list(items: &[PyValue]) -> PyResult<PyValue> {
    let mut out = Vec::with_capacity(items.len());
    for v in items {
        let _entry = eval_entry()?;
        out.push(resolve_and_decode(v)?);
    }
    Ok(PyValue::List(out.into()))
}

/// `decode_text(lit.name)`
fn literal_text(name: LiteralName) -> PyResult<String> {
    match name {
        LiteralName::Str(s) => decode_text(TextArg::Str(s)),
        LiteralName::Bytes(b) => decode_text(TextArg::Bytes(b)),
    }
}

// pdfplumber/utils/pdfinternals.py:42 decode_psl_list
/// `[decode_text(value.name) if isinstance(value, PSLiteral) else value for
/// value in _list]` (a comprehension: no frame of its own).
pub fn decode_psl_list(list: &[PyValue]) -> PyResult<Vec<PyValue>> {
    let _frame = py_frame()?;
    list.iter()
        .map(|value| match value {
            PyValue::Pdf(PDFObject::Literal(lit)) => Ok(PyValue::Str(PyStr::from(literal_text(lit.name())?))),
            other => Ok(other.clone()),
        })
        .collect()
}
