//! What the four `plumber_*` stages share, the Rust side of the helpers
//! of `oracle/stages/plumber_objects.py` (`open_pdf`, `exc`, `select`,
//! `digest`, `Names`, `Objects`) and of `plumber_text.py` (`out_line`,
//! `derive_bbox`, `PREDICATES`), with the option tables of
//! [`super::plumber_ops`].
//!
//! Wave 4 (base): `select`, `exc`, `digest`, `bounded` and the tables are
//! here; the rest is written with the stages (workstreams PAGE and DOC,
//! `docs/architecture.md`, "Vague 4"), against the library:
//!
//! - `open_pdf(input, options)`: `pdfplumber::pdf::PDF::open(Source::Path
//!   { .. }, OpenOptions { pages: Some(list 1..=max_pages), password, .. })`
//!   from `STAGE_ENTRY` (the stage function's frame is `terminated`'s
//!   body), as `pdfplumber.open(path, ...)`: the oracle opens a path
//!   (`io.BufferedReader`, not external);
//! - with `laparams` (the second opening of `plumber_objects`):
//!   `OpenOptions::ids` gives each page's analysis an `IdSource` that
//!   ranks boxes and groups by the `ids` lines of the oracle's trace
//!   (`StageInput::oracle`), keyed by content as the Python stage keys them
//!   (`content_ids`: a box's chars written `LTChar{...}`, sorted, hashed);
//! - `Names`: inline names `inline-<k>` (`pdfinterp::inline_image_name`)
//!   written `inline{"n": k}` as Python writes `str(id(stream))`, a
//!   `PyValue::Page` as `Page{"page_number": n}` (`crate::plumber::to_value`);
//! - search ops: the spans of the oracle's `spans` lines (in order), given
//!   to `Page::search` as its matches, and the `spans` line copied.

use std::collections::HashMap;
use std::rc::Rc;

use pdfminer::pdftypes::PDFObject;
use pdfplumber::container::Objects;
use pdfplumber::obj::{Obj, PyValue};
use pdfplumber::page::Page;
use pdfplumber::pdf::{OpenOptions, Source, PDF};
use pycompat::recursion::py_frame;
use pycompat::{ExcKind, PyErr, PyInt, PyResult, PyStr};

use super::common::Lines;
use super::StageInput;
use crate::canon::{canon, canon_exc, sha256_hex};
use crate::plumber::to_value;
use crate::typed_json;
use crate::value::Value;

/// `exc(e)` of `plumber_objects.py`: `canon(e)`, then ` cause <canon>` for
/// each exception a `PdfminerException` or `MalformedPDFException` wraps.
/// (The order of the set of directions in `validate_directions`' messages
/// is the port's, which the Python stage writes too.)
pub fn exc(e: &PyErr) -> String {
    let mut out = canon_exc(e);
    let mut e = e;
    while matches!(e.kind, ExcKind::PdfminerException | ExcKind::MalformedPDFException) {
        let Some(cause) = e.cause() else { break };
        out.push_str(" cause ");
        out.push_str(&canon_exc(cause));
        e = cause;
    }
    out
}

/// `select(sha256, index, variants, k)` of `plumber_objects.py`: `k`
/// consecutive entries of `variants` (cyclically) from
/// `(int(sha256[:8], 16) + index * k) % len(variants)`, `sha256` the hex
/// SHA-256 of the PDF file.
pub fn select<'a, T>(sha256: &str, index: usize, variants: &'a [T], k: usize) -> Vec<&'a T> {
    if variants.is_empty() {
        return Vec::new();
    }
    let head = u64::from_str_radix(&sha256[..8], 16).expect("a hex SHA-256");
    let n = variants.len() as u64;
    let start = (head + (index as u64) * (k as u64)) % n;
    (0..k.min(variants.len())).map(|j| &variants[((start + j as u64) % n) as usize]).collect()
}

/// `digest(value)`: the first 16 hex digits of the SHA-256 of the
/// canonical text.
pub fn digest(value: &Value) -> String {
    sha256_hex(canon(value).as_bytes())[..16].to_owned()
}

/// `label i text`, or `labeld i length digest` beyond `OUT_LIMIT`
/// characters (`out_line` of `plumber_text.py`, `bounded` of
/// `plumber_tables.py`; `plumber_text`'s label `out` becomes `outd`).
pub fn bounded(label: &str, i: &str, text: &str) -> String {
    if text.len() > super::plumber_ops::OUT_LIMIT {
        format!("{label}d {i} {} {}", text.len(), &sha256_hex(text.as_bytes())[..16])
    } else {
        format!("{label} {i} {text}")
    }
}

/// `open_pdf(path, password, max_pages, **options)` of `plumber_objects.py`:
/// a frame (the helper's), then `pdfplumber.open(path, pages=PAGES,
/// password=password, **options)` on the file the oracle opens by path
/// (`PAGES`: `[1, ..., max_pages]`, `None` for every page). `options`
/// sets the other keyword arguments.
pub fn open_pdf(input: &StageInput<'_>, options: impl FnOnce(&mut OpenOptions)) -> PyResult<PDF> {
    let _frame = py_frame()?;
    let mut o = OpenOptions::new();
    o.pages = input.max_pages.map(|n| PyValue::list((1..=n as i64).map(PyValue::from)));
    o.password = if input.password.is_empty() { None } else { Some(PyValue::str(input.password)) };
    options(&mut o);
    PDF::open(Source::Path { data: Rc::from(input.pdf), path: PyStr::new("") }, o)
}

/// `Names` of `plumber_objects.py`: the values of a page as written, an
/// inline image name (`inline-<k>` in the port, `str(id(stream))` in
/// Python) as `inline{"n": j}`, `j` numbering the distinct inline names of
/// the page's images in the order of its objects; a `Page` as
/// `Page{"page_number": n}`.
#[derive(Default)]
pub struct Names {
    inline: Vec<PyStr>,
}

/// Whether `name` is the port's name of an inline image
/// (`pdfinterp::inline_image_name`).
fn is_inline_name(name: &PyStr) -> bool {
    name.as_str()
        .and_then(|n| n.strip_prefix("inline-"))
        .is_some_and(|k| !k.is_empty() && k.bytes().all(|b| b.is_ascii_digit()) && (k == "0" || !k.starts_with('0')))
}

impl Names {
    /// `learn(objects)`: the inline names of the images, in order.
    pub fn learn(&mut self, objects: &Objects) {
        for (_, objs) in objects.iter() {
            for o in objs {
                if o.get("object_type").and_then(PyValue::as_str) != Some("image") || !o.contains("stream") {
                    continue;
                }
                // Python: `name == str(id(stream))`; the port's inline image
                // streams are the content parser's, without an object id
                let inline_stream =
                    matches!(o.get("stream"), Some(PyValue::Pdf(PDFObject::Stream(s))) if s.objid().is_none());
                if let Some(PyValue::Str(name)) = o.get("name") {
                    if inline_stream && is_inline_name(name) && !self.inline.contains(name) {
                        self.inline.push(name.clone());
                    }
                }
            }
        }
    }

    /// `value(v)`
    pub fn value(&self, v: &PyValue) -> Value {
        if let PyValue::Str(s) = v {
            if let Some(n) = self.inline.iter().position(|x| x == s) {
                return Value::Named {
                    name: "inline".into(),
                    fields: vec![(Value::str("n"), Value::Int(PyInt::Small(n as i64)))],
                };
            }
        }
        to_value(v)
    }

    /// `obj(o)`: the dict with its `name` and `P` written as above.
    pub fn obj(&self, o: &Obj) -> Value {
        Value::Dict(
            o.items()
                .iter()
                .map(|(k, v)| {
                    let v = if *k == "name" || *k == "P" { self.value(v) } else { to_value(v) };
                    (Value::py_str(k), v)
                })
                .collect(),
        )
    }

    /// `text(s)`: each inline name replaced by `<inline k>`.
    pub fn text(&self, s: &PyStr) -> PyStr {
        let mut s = s.clone();
        for (k, name) in self.inline.iter().enumerate() {
            s = s.replace(name, &PyStr::new(&format!("<inline {k}>")));
        }
        s
    }
}

/// `Objects` of `plumber_objects.py`: the `kinds`/`keys`/`obj` (and
/// `lchars`) lines of a page's objects, the canonical text of each char
/// written (`chars`: its first index on the page).
#[derive(Default)]
pub struct ObjectsWriter {
    pub names: Names,
    keys: Vec<(PyStr, Vec<PyStr>)>,
    pub chars: HashMap<String, usize>,
}

impl ObjectsWriter {
    /// `write(objects, refer)`
    pub fn write(&mut self, lines: &mut Lines, objects: &Objects, refer: Option<&HashMap<String, usize>>) {
        self.names.learn(objects);
        lines.push(format!("kinds {}", canon(&Value::List(objects.keys().map(Value::py_str).collect()))));
        for (kind, objs) in objects.iter() {
            let mut refs: Vec<Value> = Vec::new();
            for (i, o) in objs.iter().enumerate() {
                let obj = self.names.obj(o);
                let Value::Dict(items) = &obj else { unreachable!("a dict") };
                let keys: Vec<PyStr> = o.keys().cloned().collect();
                let values = canon(&Value::Tuple(items.iter().map(|(_, v)| v.clone()).collect()));
                if *kind == "char" {
                    let text = canon(&Value::List(keys.iter().map(Value::py_str).collect())) + " " + &values;
                    match refer {
                        Some(refer) => {
                            let j = refer.get(&text);
                            refs.push(j.map_or(Value::None, |j| Value::Int(PyInt::Small(*j as i64))));
                            if j.is_some() {
                                continue;
                            }
                        }
                        None => {
                            self.chars.entry(text).or_insert(i);
                        }
                    }
                }
                let known = self.keys.iter_mut().find(|(k, _)| k == kind);
                let changed = match known {
                    Some((_, k)) if *k == keys => false,
                    Some((_, k)) => {
                        *k = keys.clone();
                        true
                    }
                    None => {
                        self.keys.push((kind.clone(), keys.clone()));
                        true
                    }
                };
                if changed {
                    lines.push(format!(
                        "keys {} {}",
                        canon(&Value::py_str(kind)),
                        canon(&Value::List(keys.iter().map(Value::py_str).collect()))
                    ));
                }
                lines.push(format!("obj {} {i} {values}", canon(&Value::py_str(kind))));
            }
            if refer.is_some() && *kind == "char" {
                lines.push(format!("lchars {}", canon(&Value::List(refs))));
            }
        }
    }
}

/// The values of `boxes(page)`: `rotation`, `mediabox`, `cropbox`, the
/// optional boxes when set, `bbox`, `initial_doctop`, `width`, `height`.
pub fn box_lines(lines: &mut Lines, page: &Page) -> PyResult<()> {
    let mut boxes: Vec<(&str, PyValue)> = vec![
        ("rotation", page.rotation.clone()),
        ("mediabox", page.mediabox.clone()),
        ("cropbox", page.cropbox.clone()),
    ];
    for (name, b) in [("trimbox", &page.trimbox), ("bleedbox", &page.bleedbox), ("artbox", &page.artbox)] {
        if let Some(b) = b {
            boxes.push((name, b.clone()));
        }
    }
    boxes.push(("bbox", page.bbox.clone()));
    boxes.push(("initial_doctop", page.initial_doctop.clone()));
    for (name, v) in boxes {
        lines.push(format!("box {name} {}", canon(&to_value(&v))));
    }
    // hasattr(page, "width"): the property runs
    lines.push(format!("box width {}", canon(&to_value(&page.width()?))));
    lines.push(format!("box height {}", canon(&to_value(&page.height()?))));
    Ok(())
}

/// A table of [`super::plumber_ops`], decoded (a `list`).
pub fn table(text: &str) -> Rc<[Value]> {
    match typed_json::parse(text) {
        Ok(Value::List(ref items)) => items.clone().into(),
        other => panic!("not a list table: {other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::super::plumber_ops::*;
    use super::*;

    #[test]
    fn the_tables_decode() {
        for t in [
            TEXT_BASE_OPS,
            TEXT_VARIANT_OPS,
            TEXT_DERIVED,
            TEXT_DERIVED_OPS,
            TABLES_BASE_OPS,
            TABLES_VARIANT_OPS,
            TABLES_DERIVED_OPS,
            DOC_DERIVED_BASE,
            DOC_DERIVED,
            DOC_SERIALIZE,
            DOC_OPEN_OPTIONS,
        ] {
            assert!(!table(t).is_empty());
        }
        assert!(matches!(typed_json::parse(DOC_WRONG_PASSWORD), Ok(Value::Str(_))));
    }

    #[test]
    fn select_takes_a_cyclic_slice() {
        let v = [0, 1, 2, 3, 4];
        let sha = "00000003ffff";
        assert_eq!(select(sha, 0, &v, 2), [&3, &4]);
        assert_eq!(select(sha, 1, &v, 2), [&0, &1]);
        assert_eq!(select(sha, 0, &v, 9).len(), 5);
        assert!(select::<i32>(sha, 0, &[], 2).is_empty());
    }

    #[test]
    fn exc_writes_the_causes() {
        let inner = pycompat::exc::ValueError("x");
        let e = PyErr::wrapping(ExcKind::PdfminerException, PyErr::wrapping(ExcKind::MalformedPDFException, inner));
        assert_eq!(exc(&e), r#"!PdfminerException("x") cause !MalformedPDFException("x") cause !ValueError("x")"#);
    }
}
