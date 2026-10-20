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

use std::rc::Rc;

use pycompat::{ExcKind, PyErr};

use crate::canon::{canon, canon_exc, sha256_hex};
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
