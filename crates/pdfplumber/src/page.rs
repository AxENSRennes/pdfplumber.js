//! Port of `pdfplumber/page.py` (partial).
//!
//! Ported: `_normalize_box`, `_invert_box` and the box attributes that
//! `Page.__init__` computes from the `PDFPage` ([`PageBoxes`]: `rotation`,
//! `mediabox`, `cropbox`, `trimbox`, `bleedbox`, `artbox`, `bbox`,
//! `initial_doctop`, and the `width`/`height` properties). Not ported yet:
//! the rest of `Page` (objects, layout, annotations, cropping) and
//! `CroppedPage`.
//!
//! A box is a 4-tuple of the numbers the file holds (`int`, `float` or
//! `bool`, which `numbers.Number` accepts), kept as [`PDFObject`]s so that
//! an `int` stays an `int` and a `bool` a `bool`; the arithmetic on them is
//! Python's ([`Num`]).

use pdfminer::pdfpage::PDFPage;
use pdfminer::pdftypes::PDFObject;
use pycompat::bytes_format::bytes_mod_int;
use pycompat::recursion::py_frame;
use pycompat::{exc, ExcKind, Num, PyErr, PyInt, PyResult};

use crate::utils::pdfinternals::resolve_all;

/// `T_bbox`: four numbers.
pub type BBox = [PDFObject; 4];

/// A number as an object (an int stays an int).
fn num_obj(n: Num) -> PDFObject {
    match n {
        Num::Int(i) => PDFObject::Int(i),
        Num::Float(f) => PDFObject::Float(f),
    }
}

fn num(o: &PDFObject) -> Num {
    o.as_num().expect("a box holds numbers")
}

/// `box_raw[i]`, as a sequence index raises.
fn item(box_raw: &PDFObject, i: usize) -> PyResult<PDFObject> {
    match box_raw {
        PDFObject::List(l) => l.get(i).ok_or_else(|| exc::IndexError("list index out of range")),
        PDFObject::Bytes(b) => b
            .get(i)
            .map(|&c| PDFObject::Int(PyInt::Small(i64::from(c))))
            .ok_or_else(|| exc::IndexError("index out of range")),
        // a dict: d[0] (its keys are str)
        PDFObject::Dict(_) => Err(exc::KeyError(i.to_string())),
        other => Err(exc::TypeError(format!("'{}' object is not subscriptable", other.type_name()))),
    }
}

/// `sorted((a, b))` of two numbers: `[b, a]` when `b < a`.
fn sorted2(a: PDFObject, b: PDFObject) -> (PDFObject, PDFObject) {
    if num(&b).py_lt(&num(&a)) {
        (b, a)
    } else {
        (a, b)
    }
}

// pdfplumber/page.py:162 _normalize_box
/// Per PDF Reference 3.8.4: "Note: Although rectangles are conventionally
/// specified by their lower-left and upperright corners, it is acceptable
/// to specify any two diagonally opposite corners."
pub fn _normalize_box(box_raw: &PDFObject, rotation: &PDFObject) -> PyResult<BBox> {
    let _frame = py_frame()?;
    // all(isinstance(x, numbers.Number) for x in box_raw)
    let all_numbers = match box_raw {
        PDFObject::List(l) => l.to_vec().iter().all(|x| x.as_num().is_some()),
        // a dict iterates over its keys: str
        PDFObject::Dict(d) => d.is_empty(),
        PDFObject::Bytes(_) => true,
        PDFObject::Stream(_) => return Err(box_raw.py_getitem_index0().err().expect("a stream has no attribute 0")),
        other => return Err(exc::TypeError(format!("'{}' object is not iterable", other.type_name()))),
    };
    if !all_numbers {
        return Err(PyErr::new(
            ExcKind::MalformedPDFException,
            format!("Bounding box contains non-number coordinate(s): {}", box_raw.py_str()?),
        ));
    }
    let (x0, x1) = {
        let a = item(box_raw, 0)?;
        let b = item(box_raw, 2)?;
        sorted2(a, b)
    };
    let (y0, y1) = {
        let a = item(box_raw, 1)?;
        let b = item(box_raw, 3)?;
        sorted2(a, b)
    };
    let in_90_270 = rotation.as_num().is_some_and(|r| r.py_eq(&Num::from(90)) || r.py_eq(&Num::from(270)));
    if in_90_270 {
        Ok([y0, x0, y1, x1])
    } else {
        Ok([x0, y0, x1, y1])
    }
}

// pdfplumber/page.py:182 _invert_box
/// PDFs coordinate spaces refer to an origin in the bottom-left of the
/// page; pdfplumber flips this vertically, so that the origin is in the
/// top-left.
pub fn _invert_box(box_raw: &BBox, mb_height: &Num) -> PyResult<BBox> {
    let _frame = py_frame()?;
    let [x0, y0, x1, y1] = box_raw;
    Ok([x0.clone(), num_obj(mb_height.sub(&num(y1))?), x1.clone(), num_obj(mb_height.sub(&num(y0))?)])
}

/// `x % 360` for whatever `/Rotate` resolved to.
fn modulo_360(x: &PDFObject) -> PyResult<PDFObject> {
    match x {
        PDFObject::Bytes(b) => Ok(PDFObject::Bytes(bytes_mod_int(b, 360)?.into())),
        x => match x.as_num() {
            Some(n) => Ok(num_obj(n.modulo(&Num::from(360))?)),
            None => Err(exc::TypeError(format!("unsupported operand type(s) for %: '{}' and 'int'", x.type_name()))),
        },
    }
}

/// The boxes of a pdfplumber `Page`, as pdfplumber/page.py:191
/// `Page.__init__` sets them.
pub struct PageBoxes {
    /// `self.rotation`: `_rotation % 360` (a number, or the bytes of a
    /// string `/Rotate`).
    pub rotation: PDFObject,
    pub mediabox: BBox,
    pub cropbox: BBox,
    /// Set only when the page has the box.
    pub trimbox: Option<BBox>,
    pub bleedbox: Option<BBox>,
    pub artbox: Option<BBox>,
    /// `self.bbox`: the mediabox (until `crop`).
    pub bbox: BBox,
    pub initial_doctop: Num,
}

impl PageBoxes {
    // pdfplumber/page.py:191 Page.__init__
    /// The boxes of `Page(pdf, page_obj, page_number, initial_doctop)`.
    pub fn new(page_obj: &PDFPage, initial_doctop: Num) -> PyResult<PageBoxes> {
        let _frame = py_frame()?;
        // pdfplumber/page.py:205 Page.__init__.get_attr
        let get_attr = |key: &str, default: PDFObject| -> PyResult<PDFObject> {
            let _frame = py_frame()?;
            let value = resolve_all(&page_obj.attrs.get(key).unwrap_or(PDFObject::Null))?;
            Ok(if matches!(value, PDFObject::Null) { default } else { value })
        };
        // Per PDF Reference Table 3.27: "The number of degrees by which the
        // page should be rotated clockwise when displayed or printed. The
        // value must be a multiple of 90. Default value: 0"
        let _rotation = get_attr("Rotate", PDFObject::Int(PyInt::ZERO))?;
        let rotation = modulo_360(&_rotation)?;
        let mb_raw = _normalize_box(&get_attr("MediaBox", PDFObject::Null)?, &rotation)?;
        let mb_height = num(&mb_raw[3]).sub(&num(&mb_raw[1]))?;
        let mediabox = _invert_box(&mb_raw, &mb_height)?;
        let mut boxes: [Option<BBox>; 4] = [None, None, None, None];
        for (slot, box_name) in boxes.iter_mut().zip(["CropBox", "TrimBox", "BleedBox", "ArtBox"]) {
            if page_obj.attrs.contains_key(box_name) {
                let box_normalized =
                    _invert_box(&_normalize_box(&get_attr(box_name, PDFObject::Null)?, &rotation)?, &mb_height)?;
                *slot = Some(box_normalized);
            }
        }
        let [cropbox, trimbox, bleedbox, artbox] = boxes;
        let cropbox = cropbox.unwrap_or_else(|| mediabox.clone());
        // Page.bbox defaults to self.mediabox, but can be altered by Page.crop(...)
        let bbox = mediabox.clone();
        Ok(PageBoxes { rotation, mediabox, cropbox, trimbox, bleedbox, artbox, bbox, initial_doctop })
    }

    // pdfplumber/page.py:240 Page.width
    pub fn width(&self) -> PyResult<Num> {
        num(&self.bbox[2]).sub(&num(&self.bbox[0]))
    }

    // pdfplumber/page.py:244 Page.height
    pub fn height(&self) -> PyResult<Num> {
        num(&self.bbox[3]).sub(&num(&self.bbox[1]))
    }
}
