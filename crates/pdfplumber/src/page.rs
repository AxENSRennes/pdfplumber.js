//! Port of `pdfplumber/page.py` (partial).
//!
//! Ported: `_normalize_box`, `_invert_box` and the box attributes that
//! `Page.__init__` computes from the `PDFPage` ([`PageBoxes`]: `rotation`,
//! `mediabox`, `cropbox`, `trimbox`, `bleedbox`, `artbox`, `bbox`,
//! `initial_doctop`, and the `width`/`height` properties), and the device
//! `Page.layout` drives, `PDFPageAggregatorWithMarkedContent`
//! ([`PDFPageAggregatorWithMarkedContent`], its methods' bodies
//! [`marked_content`], the layout payload [`PlumberPayload`]; wave 3,
//! `docs/architecture.md`, "Vague 3"). Not ported yet: the rest of `Page`
//! (objects, layout, annotations, cropping) and `CroppedPage` (wave 4).
//!
//! A box is a 4-tuple of the numbers the file holds (`int`, `float` or
//! `bool`, which `numbers.Number` accepts), kept as [`PDFObject`]s so that
//! an `int` stays an `int` and a `bool` a `bool`; the arithmetic on them is
//! Python's ([`Num`]).

use std::rc::Rc;

use pdfminer::converter::{
    layout_analyzer, page_aggregator, InterpPayload, LayoutAnalyzer, PDFLayoutAnalyzer, PDFPageAggregator,
    PageAggregator,
};
use pdfminer::layout::{LAParams, LTPage, LTPayload};
use pdfminer::pdfcolor::PDFColorSpace;
use pdfminer::pdfdevice::{text_device, ObjSeq, PDFDevice, PDFDeviceAttrs, PDFTextDevice, PDFTextSeq};
use pdfminer::pdffont::{FontName, PDFFont};
use pdfminer::pdfinterp::{Color, PDFGraphicState, PDFResourceManager, PDFStackT, PDFTextState};
use pdfminer::pdfpage::PDFPage;
use pdfminer::pdftypes::{PDFObject, PDFStream};
use pdfminer::psparser::{LiteralName, PSLiteral};
use pdfminer::utils::{Matrix, PathSegment};
use pycompat::bytes_format::bytes_mod_int;
use pycompat::recursion::{eval_entry, py_frame};
use pycompat::{exc, ExcKind, Num, PyErr, PyInt, PyResult};

use crate::utils::pdfinternals::{decode_text, resolve_all, TextArg};

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

// pdfplumber/page.py:161 _normalize_box
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
        PDFObject::Stream(_) => return Err(box_raw.py_getitem_index0().expect_err("a stream has no attribute 0")),
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

// pdfplumber/page.py:181 _invert_box
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

/// The boxes of a pdfplumber `Page`, as `Page.__init__` sets them.
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
        // `get_attr`, nested in pdfplumber/page.py:191 Page.__init__
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

// ---------------------------------------------------------------------------
// pdfplumber's device. See pdfminer's `converter` and `pdfdevice` for the
// pattern (a struct of attributes, accessor traits, the methods' bodies as
// free functions usable as `super()`).

/// The value of an attribute pdfplumber sets on a layout object
/// (pdfplumber/page.py:130 `PDFPageAggregatorWithMarkedContent.tag_cur_item`):
/// `mcid`, the `MCID`
/// of the marked-content properties (any object; `None` is
/// [`PDFObject::Null`]), or `tag`, `decode_text(tag.name)` (a `str`) or
/// `None`.
#[derive(Clone, Debug)]
pub enum MarkedContentAttr {
    Mcid(PDFObject),
    Tag(Option<String>),
}

/// The layout payload of pdfplumber's pages: the interpreter's types, and
/// the `mcid`/`tag` attributes.
#[derive(Clone)]
pub struct PlumberPayload;

impl LTPayload for PlumberPayload {
    type FontName = FontName;
    type GraphicState = Rc<PDFGraphicState>;
    type Color = Color;
    type Attr = MarkedContentAttr;
}

impl InterpPayload for PlumberPayload {}

/// The attributes `PDFPageAggregatorWithMarkedContent` adds: the class
/// attributes `cur_mcid = None`, `cur_tag = None`, which `begin_tag` and
/// `end_tag` set on the instance.
#[derive(Clone, Debug)]
pub struct MarkedContentState {
    pub cur_mcid: PDFObject,
    pub cur_tag: Option<String>,
}

impl Default for MarkedContentState {
    fn default() -> MarkedContentState {
        MarkedContentState { cur_mcid: PDFObject::Null, cur_tag: None }
    }
}

/// A device that is a `PDFPageAggregatorWithMarkedContent` (or a
/// subclass, such as the tracing devices of `pdftrace`).
pub trait MarkedContent: PageAggregator<PlumberPayload> {
    fn marked(&self) -> &MarkedContentState;
    fn marked_mut(&mut self) -> &mut MarkedContentState;
}

/// The bodies of `PDFPageAggregatorWithMarkedContent`'s methods. The
/// overrides of `render_char`, `render_image` and `paint_path` take
/// `*args, **kwargs` and call `super().<method>(*args, **kwargs)`: a call
/// from C (`CALL_FUNCTION_EX` on a bound method: `PyObject_Call`, then the
/// evaluation loop entered for the method's frame, two C units measured by
/// `oracle/unit/converter.py`'s probe).
pub mod marked_content {
    use super::*;

    // pdfplumber/page.py:117 PDFPageAggregatorWithMarkedContent.begin_tag
    /// Handle beginning of tag, setting current MCID if any:
    /// `cur_tag = decode_text(tag.name)`, `cur_mcid = props["MCID"]` when
    /// `props` is a dict that has it, else `None`.
    pub fn begin_tag<D: MarkedContent + ?Sized>(
        dev: &mut D,
        tag: &PSLiteral,
        props: Option<&PDFStackT>,
    ) -> PyResult<()> {
        let _frame = py_frame()?;
        let cur_tag = match tag.name() {
            LiteralName::Str(s) => decode_text(TextArg::Str(s))?,
            LiteralName::Bytes(b) => decode_text(TextArg::Bytes(b))?,
        };
        let marked = dev.marked_mut();
        marked.cur_tag = Some(cur_tag);
        marked.cur_mcid = match props {
            Some(PDFObject::Dict(d)) => d.get("MCID").unwrap_or(PDFObject::Null),
            _ => PDFObject::Null,
        };
        Ok(())
    }

    // pdfplumber/page.py:125 PDFPageAggregatorWithMarkedContent.end_tag
    /// Handle beginning of tag, clearing current MCID.
    pub fn end_tag<D: MarkedContent + ?Sized>(dev: &mut D) -> PyResult<()> {
        let _frame = py_frame()?;
        let marked = dev.marked_mut();
        marked.cur_tag = None;
        marked.cur_mcid = PDFObject::Null;
        Ok(())
    }

    // pdfplumber/page.py:130 PDFPageAggregatorWithMarkedContent.tag_cur_item
    /// Add current MCID to what we hope to be the most recent object
    /// created by pdfminer.six: sets `mcid`, then `tag`, on the last object
    /// of `cur_item`, if any (`layout::LTItem::set_attr`).
    pub fn tag_cur_item<D: MarkedContent + ?Sized>(dev: &mut D) -> PyResult<()> {
        let _frame = py_frame()?;
        let Some(item) = dev.analyzer().cur_item.as_ref() else {
            return Err(exc::AttributeError("'PDFPageAggregatorWithMarkedContent' object has no attribute 'cur_item'"));
        };
        if let Some(cur_obj) = item.last() {
            let marked = dev.marked();
            cur_obj.set_attr("mcid", MarkedContentAttr::Mcid(marked.cur_mcid.clone()));
            cur_obj.set_attr("tag", MarkedContentAttr::Tag(marked.cur_tag.clone()));
        }
        Ok(())
    }

    // pdfplumber/page.py:144 PDFPageAggregatorWithMarkedContent.render_char
    /// Hook for rendering characters, adding the `mcid` attribute:
    /// `super().render_char(...)` (`converter::layout_analyzer::render_char`),
    /// then `tag_cur_item`.
    pub fn render_char<D: MarkedContent + ?Sized>(
        dev: &mut D,
        matrix: Matrix,
        font: &Rc<PDFFont>,
        fontsize: &Num,
        scaling: &Num,
        rise: &Num,
        cid: &PyInt,
        ncs: &Rc<PDFColorSpace>,
        graphicstate: &Rc<PDFGraphicState>,
    ) -> PyResult<Num> {
        let _frame = py_frame()?;
        let adv = {
            let _entry = eval_entry()?;
            layout_analyzer::render_char(dev, matrix, font, fontsize, scaling, rise, cid, ncs, graphicstate)?
        };
        tag_cur_item(dev)?;
        Ok(adv)
    }

    // pdfplumber/page.py:150 PDFPageAggregatorWithMarkedContent.render_image
    /// Hook for rendering images, adding the `mcid` attribute.
    pub fn render_image<D: MarkedContent + ?Sized>(dev: &mut D, name: &str, stream: &Rc<PDFStream>) -> PyResult<()> {
        let _frame = py_frame()?;
        {
            let _entry = eval_entry()?;
            layout_analyzer::render_image(dev, name, stream)?;
        }
        tag_cur_item(dev)
    }

    // pdfplumber/page.py:155 PDFPageAggregatorWithMarkedContent.paint_path
    /// Hook for rendering lines and curves, adding the `mcid` attribute:
    /// `super().paint_path(...)`, whose subpaths come back through
    /// `dev.paint_path` (this function again: each subpath is tagged), then
    /// `tag_cur_item`.
    pub fn paint_path<D: MarkedContent + ?Sized>(
        dev: &mut D,
        graphicstate: &PDFGraphicState,
        stroke: bool,
        fill: bool,
        evenodd: bool,
        path: &[PathSegment],
    ) -> PyResult<()> {
        let _frame = py_frame()?;
        {
            let _entry = eval_entry()?;
            layout_analyzer::paint_path(dev, graphicstate, stroke, fill, evenodd, path)?;
        }
        tag_cur_item(dev)
    }
}

/// pdfplumber/page.py:110 `PDFPageAggregatorWithMarkedContent`: extracts
/// the layout of a page, adding marked-content ids to the objects
/// (pdfplumber/page.py:256 `Page.layout` makes one per page:
/// `PDFPageAggregatorWithMarkedContent(pdf.rsrcmgr,
/// pageno=page_number, laparams=pdf.laparams)`).
pub struct PDFPageAggregatorWithMarkedContent {
    pub aggregator: PDFPageAggregator<PlumberPayload>,
    pub marked: MarkedContentState,
}

impl PDFPageAggregatorWithMarkedContent {
    /// The attributes of `PDFPageAggregatorWithMarkedContent(rsrcmgr,
    /// pageno=pageno, laparams=laparams)` (`PDFPageAggregator.__init__`),
    /// without the frames of the class call ([`Self::create`]).
    pub fn new(rsrcmgr: Rc<PDFResourceManager>, pageno: PyInt, laparams: Option<LAParams>) -> Self {
        PDFPageAggregatorWithMarkedContent {
            aggregator: PDFPageAggregator::new(rsrcmgr, pageno, laparams),
            marked: MarkedContentState::default(),
        }
    }

    /// `PDFPageAggregatorWithMarkedContent(rsrcmgr, pageno=pageno,
    /// laparams=laparams)`, as pdfplumber/page.py:256 `Page.layout` calls
    /// it: the class call's C unit and evaluation loop entry, the frames of
    /// `PDFPageAggregator.__init__` -> `PDFLayoutAnalyzer.__init__` ->
    /// `PDFDevice.__init__` (no `__init__` of its own), then the
    /// attributes.
    pub fn create(rsrcmgr: Rc<PDFResourceManager>, pageno: PyInt, laparams: Option<LAParams>) -> PyResult<Self> {
        let _call = pycompat::recursion::class_call()?;
        PDFPageAggregator::<PlumberPayload>::init_frames()?;
        Ok(PDFPageAggregatorWithMarkedContent::new(rsrcmgr, pageno, laparams))
    }
}

impl PDFDevice for PDFPageAggregatorWithMarkedContent {
    fn attrs(&self) -> &PDFDeviceAttrs {
        &self.aggregator.analyzer.device
    }

    fn attrs_mut(&mut self) -> &mut PDFDeviceAttrs {
        &mut self.aggregator.analyzer.device
    }

    fn begin_tag(&mut self, tag: &PSLiteral, props: Option<&PDFStackT>) -> PyResult<()> {
        marked_content::begin_tag(self, tag, props)
    }

    fn end_tag(&mut self) -> PyResult<()> {
        marked_content::end_tag(self)
    }

    fn begin_page(&mut self, page: &PDFPage, ctm: &Matrix) -> PyResult<()> {
        layout_analyzer::begin_page(self, page, ctm)
    }

    fn end_page(&mut self, page: &PDFPage) -> PyResult<()> {
        layout_analyzer::end_page(self, page)
    }

    fn begin_figure(&mut self, name: &str, bbox: &ObjSeq, matrix: &ObjSeq) -> PyResult<()> {
        layout_analyzer::begin_figure(self, name, bbox, matrix)
    }

    fn end_figure(&mut self, name: &str) -> PyResult<()> {
        layout_analyzer::end_figure(self, name)
    }

    fn paint_path(
        &mut self,
        graphicstate: &PDFGraphicState,
        stroke: bool,
        fill: bool,
        evenodd: bool,
        path: &[PathSegment],
    ) -> PyResult<()> {
        marked_content::paint_path(self, graphicstate, stroke, fill, evenodd, path)
    }

    fn render_image(&mut self, name: &str, stream: &Rc<PDFStream>) -> PyResult<()> {
        marked_content::render_image(self, name, stream)
    }

    fn render_string(
        &mut self,
        textstate: &mut PDFTextState,
        seq: &PDFTextSeq,
        ncs: &Rc<PDFColorSpace>,
        graphicstate: &Rc<PDFGraphicState>,
    ) -> PyResult<()> {
        text_device::render_string(self, textstate, seq, ncs, graphicstate)
    }
}

impl PDFTextDevice for PDFPageAggregatorWithMarkedContent {
    fn render_char(
        &mut self,
        matrix: Matrix,
        font: &Rc<PDFFont>,
        fontsize: &Num,
        scaling: &Num,
        rise: &Num,
        cid: &PyInt,
        ncs: &Rc<PDFColorSpace>,
        graphicstate: &Rc<PDFGraphicState>,
    ) -> PyResult<Num> {
        marked_content::render_char(self, matrix, font, fontsize, scaling, rise, cid, ncs, graphicstate)
    }
}

impl LayoutAnalyzer<PlumberPayload> for PDFPageAggregatorWithMarkedContent {
    fn analyzer(&self) -> &PDFLayoutAnalyzer<PlumberPayload> {
        &self.aggregator.analyzer
    }

    fn analyzer_mut(&mut self) -> &mut PDFLayoutAnalyzer<PlumberPayload> {
        &mut self.aggregator.analyzer
    }

    fn receive_layout(&mut self, ltpage: LTPage<PlumberPayload>) -> PyResult<()> {
        page_aggregator::receive_layout(self, ltpage)
    }
}

impl PageAggregator<PlumberPayload> for PDFPageAggregatorWithMarkedContent {
    fn aggregator(&self) -> &PDFPageAggregator<PlumberPayload> {
        &self.aggregator
    }

    fn aggregator_mut(&mut self) -> &mut PDFPageAggregator<PlumberPayload> {
        &mut self.aggregator
    }
}

impl MarkedContent for PDFPageAggregatorWithMarkedContent {
    fn marked(&self) -> &MarkedContentState {
        &self.marked
    }

    fn marked_mut(&mut self) -> &mut MarkedContentState {
        &mut self.marked
    }
}
