//! Port of `pdfplumber/convert.py`: the `Serializer` of `to_json` and
//! `to_csv` (`crate::container::Container`).
//!
//! Wave 4 (base): the interface, bodies `todo!()` (workstream PAGE,
//! `docs/architecture.md`, "Vague 4").
//!
//! `Serializer.serialize` dispatches on `type(obj)` (`do_<type name>`), so
//! the values it meets in pdfplumber's dicts decide the output:
//! `int`/`str` as they are, `float` rounded (`round(x, precision)`,
//! `pycompat`'s), `bool` as `int`, `list`/`tuple`/`dict` element-wise (a
//! dict with an `object_type` key filtered by the attribute filter),
//! `PDFStream` as `{"rawdata": base64 or None}` (its `rawdata`, `None` once
//! decoded: the state of the stream's cache shows), `PSLiteral` as
//! `decode_text(name)`, `bytes` decoded as UTF-8 or `None`; every other
//! type through `str()`: `PDFObjRef` (`<PDFObjRef:12>`), `PSKeyword`, a
//! `Page` (`<Page:1>`, the `/P` of annotations, [`crate::obj::PageRef`]),
//! a `PDFObject` `dict` or `list` reached through `resolve_all` stays a
//! Python `dict`/`list` (it is one). `None` stays `None`.
//!
//! Recursion: `serialize` and `do_<type>` are a frame each, and
//! `do_list`/`do_tuple` resume a generator expression from C (`list(...)`,
//! `tuple(...)`): three frames per level of a nested list (two for a
//! dict, a comprehension). See `crate::container` ("Recursion").

use pycompat::{PyResult, PyStr};

use crate::obj::PyValue;

// pdfplumber/convert.py:8 ENCODINGS_TO_TRY
pub const ENCODINGS_TO_TRY: [&str; 4] = ["utf-8", "latin-1", "utf-16", "utf-16le"];

// pdfplumber/convert.py:15 CSV_COLS_REQUIRED
pub const CSV_COLS_REQUIRED: [&str; 1] = ["object_type"];

// pdfplumber/convert.py:19 CSV_COLS_TO_PREPEND
pub const CSV_COLS_TO_PREPEND: [&str; 10] =
    ["page_number", "x0", "x1", "y0", "y1", "doctop", "top", "bottom", "width", "height"];

/// The function `get_attr_filter` returns (a lambda over a set).
#[derive(Clone, Debug)]
pub enum AttrFilter {
    /// `lambda attr: True`
    All,
    /// `lambda attr: attr in incl`, `incl = set(CSV_COLS_REQUIRED + include_attrs)`
    Include(Vec<PyStr>),
    /// `lambda attr: attr not in excl`, `excl = set(exclude_attrs)`
    Exclude(Vec<PyStr>),
}

impl AttrFilter {
    /// `attr_filter(attr)`
    pub fn accepts(&self, attr: &PyStr) -> bool {
        let _ = attr;
        todo!("the lambdas of pdfplumber/convert.py:33 get_attr_filter")
    }
}

// pdfplumber/convert.py:33 get_attr_filter
/// `ValueError` for both lists, or for an excluded required attribute
/// (`"Cannot exclude these required properties: ['object_type']"`).
pub fn get_attr_filter(include_attrs: Option<&[PyStr]>, exclude_attrs: Option<&[PyStr]>) -> PyResult<AttrFilter> {
    let _ = (include_attrs, exclude_attrs);
    todo!("pdfplumber/convert.py:33 get_attr_filter")
}

// pdfplumber/convert.py:58 to_b64
pub fn to_b64(data: &[u8]) -> PyStr {
    let _ = data;
    todo!("pdfplumber/convert.py:58 to_b64 (pycompat::base64)")
}

/// pdfplumber/convert.py:62 `Serializer`
#[derive(Clone, Debug)]
pub struct Serializer {
    /// `self.precision`, as given
    pub precision: Option<PyValue>,
    pub attr_filter: AttrFilter,
}

impl Serializer {
    // pdfplumber/convert.py:63 Serializer.__init__
    pub fn new(
        precision: Option<PyValue>,
        include_attrs: Option<&[PyStr]>,
        exclude_attrs: Option<&[PyStr]>,
    ) -> PyResult<Serializer> {
        let _ = (precision, include_attrs, exclude_attrs);
        todo!("pdfplumber/convert.py:63 Serializer.__init__")
    }

    // pdfplumber/convert.py:75 Serializer.serialize
    /// The serialized value: `None`, `bool`... as described in the module
    /// documentation; a result made of `None`, `int`, `float`, `str`,
    /// `list`, `tuple` and `dict` values.
    pub fn serialize(&self, obj: &PyValue) -> PyResult<PyValue> {
        let _ = obj;
        todo!("pdfplumber/convert.py:75 Serializer.serialize")
    }

    // pdfplumber/convert.py:94 Serializer.do_float
    pub fn do_float(&self, x: &PyValue) -> PyResult<PyValue> {
        let _ = x;
        todo!("pdfplumber/convert.py:94 Serializer.do_float")
    }

    // pdfplumber/convert.py:118 Serializer.do_bytes
    /// `obj.decode("utf-8")`, `None` when that raises (the loop returns at
    /// its first encoding either way).
    pub fn do_bytes(&self, obj: &[u8]) -> PyResult<PyValue> {
        let _ = obj;
        todo!("pdfplumber/convert.py:118 Serializer.do_bytes")
    }
}
