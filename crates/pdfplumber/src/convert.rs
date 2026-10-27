//! Port of `pdfplumber/convert.py`: the `Serializer` of `to_json` and
//! `to_csv` (`crate::container::Container`).
//!
//! See `docs/architecture.md`, "Serialization".
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

use pdfminer::pdftypes::PDFObject;
use pdfminer::psparser::LiteralName;
use pycompat::json::{JsonKind, JsonValue};
use pycompat::recursion::{class_call, eval_entry, leaf, py_frame};
use pycompat::{exc, Num, PyResult, PyStr};

use crate::obj::{Obj, PyValue};
use crate::utils::pdfinternals::{decode_text, TextArg};

// pdfplumber/convert.py:8 ENCODINGS_TO_TRY
pub const ENCODINGS_TO_TRY: [&str; 4] = ["utf-8", "latin-1", "utf-16", "utf-16le"];

// pdfplumber/convert.py:15 CSV_COLS_REQUIRED
pub const CSV_COLS_REQUIRED: [&str; 1] = ["object_type"];

// pdfplumber/convert.py:19 CSV_COLS_TO_PREPEND
pub const CSV_COLS_TO_PREPEND: [&str; 10] =
    ["page_number", "x0", "x1", "y0", "y1", "doctop", "top", "bottom", "width", "height"];

/// The function `get_attr_filter` returns (a lambda over a set; only the
/// `str` items of the set can equal an attribute name).
#[derive(Clone, Debug)]
pub enum AttrFilter {
    /// `lambda attr: True`
    All,
    /// `lambda attr: attr in incl`, `incl = set(CSV_COLS_REQUIRED + include_attrs)`
    Include(Vec<PyStr>),
    /// `lambda attr: attr not in excl`, `excl = set(exclude_attrs)`
    Exclude(Vec<PyStr>),
}

/// `set(items)`: the items of an iterable, each hashable (`TypeError`
/// otherwise), as the `str` among them.
fn str_set(items: &PyValue) -> PyResult<Vec<PyStr>> {
    let items = match crate::obj::py_iter(items) {
        Some(items) => items?,
        None => return Err(exc::TypeError(format!("'{}' object is not iterable", items.type_name()))),
    };
    let mut set = Vec::new();
    for item in &items {
        crate::obj::check_hashable(item)?;
        if let PyValue::Str(s) = item {
            set.push(s.clone());
        }
    }
    Ok(set)
}

impl AttrFilter {
    /// `attr_filter(attr)`
    pub fn accepts(&self, attr: &PyStr) -> bool {
        match self {
            AttrFilter::All => true,
            AttrFilter::Include(incl) => incl.contains(attr),
            AttrFilter::Exclude(excl) => !excl.contains(attr),
        }
    }
}

// pdfplumber/convert.py:33 get_attr_filter
/// `ValueError` for both lists, or for an excluded required attribute
/// (`"Cannot exclude these required properties: ['object_type']"`); any
/// value as Python takes it: `CSV_COLS_REQUIRED + include_attrs` (a list
/// only), `set(exclude_attrs)` (any iterable), their items hashable.
pub fn get_attr_filter(include_attrs: &PyValue, exclude_attrs: &PyValue) -> PyResult<AttrFilter> {
    let _frame = py_frame()?;
    match (!include_attrs.is_none(), !exclude_attrs.is_none()) {
        (true, true) => Err(exc::ValueError("Cannot specify `include_attrs` and `exclude_attrs` at the same time.")),
        (true, false) => {
            let required = PyValue::list(CSV_COLS_REQUIRED.iter().map(|c| PyValue::str(c)));
            Ok(AttrFilter::Include(str_set(&required.add(include_attrs)?)?))
        }
        (false, true) => {
            let excl = str_set(exclude_attrs)?;
            // set(exclude_attrs).intersection(set(CSV_COLS_REQUIRED)): a
            // set of one item at most, printed as a list
            let required: Vec<PyStr> = CSV_COLS_REQUIRED.iter().map(|c| PyStr::new(c)).collect();
            let nonexcludable: Vec<&PyStr> = required.iter().filter(|r| excl.contains(r)).collect();
            if !nonexcludable.is_empty() {
                let list = nonexcludable.iter().map(|r| r.repr()).collect::<Vec<_>>().join(", ");
                return Err(exc::ValueError(format!("Cannot exclude these required properties: [{list}]")));
            }
            Ok(AttrFilter::Exclude(excl))
        }
        (false, false) => Ok(AttrFilter::All),
    }
}

// pdfplumber/convert.py:58 to_b64
pub fn to_b64(data: &[u8]) -> PyResult<PyStr> {
    let _frame = py_frame()?;
    // base64.b64encode(data): a Python function of the standard library
    leaf(1)?;
    Ok(PyStr::from(pycompat::base64::b64encode(data)))
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
    /// `Serializer(precision=..., include_attrs=..., exclude_attrs=...)`:
    /// the class call and `__init__`.
    pub fn new(precision: Option<PyValue>, include_attrs: &PyValue, exclude_attrs: &PyValue) -> PyResult<Serializer> {
        let _call = class_call()?;
        let _frame = py_frame()?;
        let attr_filter = get_attr_filter(include_attrs, exclude_attrs)?;
        Ok(Serializer { precision: precision.filter(|p| !p.is_none()), attr_filter })
    }

    // pdfplumber/convert.py:75 Serializer.serialize
    /// The serialized value: `None` and `int`/`str` as they are, then the
    /// `do_<type>` method of the value's type (`float`, `bool`, `list`,
    /// `tuple`, `dict`, `PDFStream`, `PSLiteral`, `bytes`), else `str(obj)`.
    /// The result is made of `None`, `int`, `float`, `str`, `list`, `tuple`
    /// and `dict` values (PDF lists and dicts become Python ones).
    pub fn serialize(&self, obj: &PyValue) -> PyResult<PyValue> {
        let _frame = py_frame()?;
        match obj {
            PyValue::None | PyValue::Pdf(PDFObject::Null) => Ok(PyValue::None),
            PyValue::Int(_) | PyValue::Str(_) => Ok(obj.clone()),
            PyValue::Pdf(PDFObject::Int(i)) => Ok(PyValue::Int(i.clone())),
            PyValue::Float(_) => self.do_float(obj),
            PyValue::Pdf(PDFObject::Float(x)) => self.do_float(&PyValue::float(*x)),
            PyValue::Bool(b) | PyValue::Pdf(PDFObject::Bool(b)) => {
                // do_bool: int(x)
                leaf(1)?;
                Ok(PyValue::from(i64::from(*b)))
            }
            PyValue::List(items) => self.do_seq(items, false),
            PyValue::Tuple(items) => self.do_seq(items, true),
            PyValue::Pdf(PDFObject::List(l)) => {
                let items: Vec<PyValue> = l.to_vec().into_iter().map(PyValue::from_pdf).collect();
                self.do_seq(&items, false)
            }
            PyValue::Dict(d) => self.do_dict(d.items().iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
            PyValue::Pdf(PDFObject::Dict(d)) => {
                self.do_dict(d.items().into_iter().map(|(k, v)| (PyStr::new(&k), PyValue::from_pdf(v))).collect())
            }
            PyValue::Pdf(PDFObject::Stream(s)) => {
                // pdfplumber/convert.py:112 Serializer.do_PDFStream
                let _frame = py_frame()?;
                let rawdata = s.rawdata();
                let value = match rawdata {
                    Some(raw) if !raw.is_empty() => PyValue::Str(to_b64(&raw)?),
                    _ => PyValue::None,
                };
                Ok(PyValue::Dict(Obj::from_pairs([("rawdata", value)])))
            }
            PyValue::Pdf(PDFObject::Literal(lit)) => {
                // pdfplumber/convert.py:115 Serializer.do_PSLiteral
                let _frame = py_frame()?;
                let text = match lit.name() {
                    LiteralName::Str(s) => decode_text(TextArg::Str(s))?,
                    LiteralName::Bytes(b) => decode_text(TextArg::Bytes(b))?,
                };
                Ok(PyValue::Str(PyStr::from(text)))
            }
            PyValue::Bytes(b) | PyValue::Pdf(PDFObject::Bytes(b)) => self.do_bytes(b),
            // str(obj): PDFObjRef, PSKeyword, a Page...
            other => Ok(PyValue::Str(other.str_counted()?)),
        }
    }

    // pdfplumber/convert.py:94 Serializer.do_float
    /// `x if self.precision is None else round(x, self.precision)`
    pub fn do_float(&self, x: &PyValue) -> PyResult<PyValue> {
        let _frame = py_frame()?;
        let Some(precision) = &self.precision else {
            return Ok(x.clone());
        };
        let Some(Num::Float(f)) = x.as_num() else { unreachable!("a float") };
        // round(x, ndigits): ndigits through __index__, clipped to Py_ssize_t
        let ndigits = match precision {
            PyValue::Int(i) | PyValue::Pdf(PDFObject::Int(i)) => {
                i.as_i64().unwrap_or(if i.is_negative() { i64::MIN } else { i64::MAX })
            }
            PyValue::Bool(b) => i64::from(*b),
            other => {
                return Err(exc::TypeError(format!(
                    "'{}' object cannot be interpreted as an integer",
                    other.type_name()
                )))
            }
        };
        Ok(PyValue::float(pycompat::float::round_float_ndigits(f, ndigits)?))
    }

    // pdfplumber/convert.py:100 Serializer.do_list
    // pdfplumber/convert.py:103 Serializer.do_tuple
    /// `list(self.serialize(x) for x in obj)` (or `tuple(...)`): the
    /// generator expression resumed from C, once per item and once more.
    fn do_seq(&self, items: &[PyValue], tuple: bool) -> PyResult<PyValue> {
        let _frame = py_frame()?;
        leaf(1)?; // the generator expression is created
        let mut out = Vec::with_capacity(items.len());
        for x in items {
            let _entry = eval_entry()?;
            let _genexpr = py_frame()?;
            out.push(self.serialize(x)?);
        }
        let _entry = eval_entry()?;
        leaf(1)?;
        Ok(if tuple { PyValue::Tuple(out.into()) } else { PyValue::List(out.into()) })
    }

    // pdfplumber/convert.py:106 Serializer.do_dict
    /// A dict comprehension (no frame of its own): with an `object_type`
    /// key, the items whose key `self.attr_filter` accepts (the lambda, a
    /// frame per key).
    fn do_dict(&self, items: Vec<(PyStr, PyValue)>) -> PyResult<PyValue> {
        let _frame = py_frame()?;
        let filtered = items.iter().any(|(k, _)| *k == "object_type");
        let mut out = Obj::new();
        for (k, v) in &items {
            if filtered {
                leaf(1)?;
                if !self.attr_filter.accepts(k) {
                    continue;
                }
            }
            let value = self.serialize(v)?;
            out.set(k, value);
        }
        Ok(PyValue::Dict(out))
    }

    // pdfplumber/convert.py:118 Serializer.do_bytes
    /// `obj.decode("utf-8")`, `None` when that raises (the loop returns at
    /// its first encoding either way).
    pub fn do_bytes(&self, obj: &[u8]) -> PyResult<PyValue> {
        let _frame = py_frame()?;
        Ok(match std::str::from_utf8(obj) {
            Ok(s) => PyValue::str(s),
            Err(_) => PyValue::None,
        })
    }
}

/// The values `Serializer.serialize` produces, for `json.dumps`
/// (`pycompat::json`); any other value is not JSON serializable.
impl JsonValue for PyValue {
    fn json_kind(&self) -> JsonKind<'_, PyValue> {
        match self {
            PyValue::None => JsonKind::None,
            PyValue::Bool(b) => JsonKind::Bool(*b),
            PyValue::Int(i) => JsonKind::Int(i),
            PyValue::Float(x) => JsonKind::Float(x.get()),
            PyValue::Str(s) => JsonKind::Str(s),
            PyValue::List(items) | PyValue::Tuple(items) => JsonKind::List(items),
            PyValue::Dict(d) => JsonKind::Dict(d.items()),
            other => JsonKind::Other(other.type_name()),
        }
    }
}
