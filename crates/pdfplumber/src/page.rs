//! Port of `pdfplumber/page.py`.
//!
//! Ported: `_normalize_box`, `_invert_box` and the box attributes that
//! `Page.__init__` computes from the `PDFPage` ([`PageBoxes`]: `rotation`,
//! `mediabox`, `cropbox`, `trimbox`, `bleedbox`, `artbox`, `bbox`,
//! `initial_doctop`, and the `width`/`height` properties), the device
//! `Page.layout` drives, `PDFPageAggregatorWithMarkedContent`
//! ([`PDFPageAggregatorWithMarkedContent`], its methods' bodies
//! [`marked_content`], the layout payload [`PlumberPayload`]; wave 3,
//! `docs/architecture.md`, "Vague 3") and the body of `Page.layout`
//! ([`page_layout`], moved here from `pdftrace` in wave 4).
//!
//! Wave 4 (base): the interface of [`Page`] (`Page`, `DerivedPage`,
//! `CroppedPage`, `FilteredPage`), bodies `todo!()` (see the section
//! "`Page`" below and `docs/architecture.md`, "Vague 4"). Out of scope:
//! `Page.to_image` (pdfplumber/display.py: rendering with pypdfium2 and
//! PIL).
//!
//! A box is a 4-tuple of the numbers the file holds (`int`, `float` or
//! `bool`, which `numbers.Number` accepts), kept as [`PDFObject`]s so that
//! an `int` stays an `int` and a `bool` a `bool`; the arithmetic on them is
//! Python's ([`Num`]).

use std::cell::RefCell;
use std::rc::Rc;

use pdfminer::converter::{
    layout_analyzer, page_aggregator, InterpPayload, LayoutAnalyzer, PDFLayoutAnalyzer, PDFPageAggregator,
    PageAggregator,
};
use pdfminer::layout::{ExtraAttrs, LAParams, LTAnno, LTComponent, LTItem, LTPage, LTPayload, TextLineElement};
use pdfminer::pdfcolor::PDFColorSpace;
use pdfminer::pdfdevice::{text_device, ObjSeq, PDFDevice, PDFDeviceAttrs, PDFTextDevice, PDFTextSeq};
use pdfminer::pdffont::{FontName, PDFFont};
use pdfminer::pdfinterp::{
    Color, PDFGraphicState, PDFPageInterpreter, PDFResourceManager, PDFStackT, PDFTextState, StandardColor,
};
use pdfminer::pdfpage::PDFPage;
use pdfminer::pdftypes::{PDFDict, PDFList, PDFObject, PDFStream};
use pdfminer::psparser::{LiteralName, PSLiteral};
use pdfminer::utils::{Matrix, PathSegment, Point};
use pycompat::bytes_format::bytes_mod_int;
use pycompat::call::Signature;
use pycompat::codecs;
use pycompat::recursion::{c_call, c_called_leaf, class_call, eval_entry, leaf, py_frame, py_frames};
use pycompat::repr::bytes_repr;
use pycompat::specialize::{AttrSite, CallSite};
use pycompat::typing_cache::Subscription;
use pycompat::{exc, ExcKind, Num, PyErr, PyInt, PyResult, PyStr};

use crate::container::{Container, ContainerCache, Objects};
use crate::obj::{bbox_from_value, bbox_value, Bbox, HashKey, Hot, Obj, PageRef, PyValue};
use crate::pdf::PdfCore;
use crate::structure::{self, PDFStructTree, Scope};
use crate::table::{Table, TableFinder, TablePage, TableSettings};
use crate::utils::exceptions::pdfminer_exception;
use crate::utils::geometry::{calculate_area, crop_to_bbox, get_bbox_overlap, outside_bbox, within_bbox};
use crate::utils::pdfinternals::{decode_text, resolve_all, resolve_and_decode, TextArg};
use crate::utils::text::{
    chars_to_textmap, dedupe_chars, extract_text_simple, extract_words_kwargs, Kwargs, RegexMatch, TextMap,
};

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
        PDFObject::Str(s) => s
            .code_points()
            .nth(i)
            .map(|c| PDFObject::Str(PyStr::from_code_point(c)))
            .ok_or_else(|| exc::IndexError("string index out of range")),
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
        // a str (left by the metadata decoding, or an attribute decoded by
        // the structure tree) iterates over its characters: str
        PDFObject::Str(s) => s.is_empty(),
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
    Tag(Option<PyStr>),
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
    pub cur_tag: Option<PyStr>,
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
        marked.cur_tag = Some(PyStr::from(cur_tag));
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

// ---------------------------------------------------------------------------
// `Page.layout`'s driving (wave 3 code, moved from `pdftrace` in wave 4).

// pdfplumber/page.py:256 Page.layout
/// The body of `Page.layout`, on the document's resource manager, with
/// the device `make` builds (pdfplumber's: [`PDFPageAggregatorWithMarkedContent::create`];
/// the stages of `pdftrace` give subclasses that record the calls): one
/// frame (the property's), `device = make()`, `PDFPageInterpreter(rsrcmgr,
/// device)`, `interpreter.process_page(page)`. Returns the device, the
/// interpreter (the `fonts` stage reads its `fontmap`, the `layout` stage
/// its `inline_images`) and what `process_page` raised (unwrapped:
/// [`Page::layout`] wraps it in `PdfminerException`).
pub fn page_layout<D: PDFDevice + 'static>(
    rsrcmgr: &Rc<PDFResourceManager>,
    page: &PDFPage,
    make: impl FnOnce() -> PyResult<D>,
) -> PyResult<(Rc<RefCell<D>>, PDFPageInterpreter, Option<PyErr>)> {
    let _frame = py_frame()?;
    let device = Rc::new(RefCell::new(make()?));
    let shared: Rc<RefCell<dyn PDFDevice>> = device.clone();
    let mut interpreter = PDFPageInterpreter::new(rsrcmgr.clone(), shared)?;
    let error = interpreter.process_page(page).err();
    Ok((device, interpreter, error))
}

// ---------------------------------------------------------------------------
// `Page`, `DerivedPage`, `CroppedPage`, `FilteredPage` (wave 4).
//
// Wave 4 (base): the interface, bodies `todo!()`; workstream PAGE, except
// `structure_tree`, `annots`, `hyperlinks` (workstream DOC):
// `docs/architecture.md`, "Vague 4".
//
// # Classes
//
// One struct for the four classes: [`Page`] holds the attributes every
// page has (in `__init__`'s order) and [`PageKind`] says which class it is
// and what a derived page adds (`parent_page`, `_crop_fn`, `filter_fn`).
// Methods that a subclass overrides (`objects`) dispatch on the kind.
// `is_original` (no use in the library) is `matches!(kind, PageKind::Original)`.
//
// # Laziness and caching
//
// - `layout` (`_layout`), `objects` (`_objects`), the edges of
//   `Container` are computed on first use and kept ([`PageCache`],
//   `ContainerCache`); `close` and `flush_cache` drop them. A derived
//   page computes its own (`DerivedPage.__init__` flushes
//   `Container.cached_properties`) from its parent's `objects`.
// - `get_textmap` is `lru_cache()(self._get_textmap)` (maxsize 128, keyed by
//   the keyword arguments in the order given, after `tuplify_list_kwargs`;
//   an unhashable value raises `TypeError` before anything is computed; a
//   result is shared: `extract_text`, `search` and `extract_text_lines`
//   with the same arguments reuse one `TextMap`).
// - `dedupe_chars` returns a `FilteredPage` whose `_objects` is set at
//   once (the parent's dict copied, `"char"` replaced).
//
// # Recursion (frame-counting plan)
//
// pdfplumber's calls, each a Python frame unless said otherwise (counted
// with `pycompat::recursion`, measured with `oracle/depth_probe.py` where
// the standard library is involved):
// - `Page(...)`: class call, `__init__`, `get_attr` (nested) and
//   `resolve_all` for each box (`PageBoxes::new` already counts them),
//   `_normalize_box`/`_invert_box`, then `lru_cache()` (`functools.lru_cache`,
//   `decorating_function`, `update_wrapper`: Python frames of the standard
//   library, to measure);
// - `objects` -> `parse_objects` -> `layout` (see [`page_layout`]) ->
//   `iter_layout_objects` (a generator: `yield from` itself for each
//   `LTContainer` level, so the chain of suspended generators is resumed at
//   each object: one frame per level of figure nesting) -> `process_object`
//   -> `re.sub` (Python frames: `re.sub`, `re._compile`), `dict(filter(None,
//   map(process_attr, ...)))` (each `process_attr` called from C:
//   `eval_entry`), `resolve_all` (recursive, `pdfinternals.rs`: a value
//   nested deep, an `mcid` from a property list or an image's
//   `colorspace`, raises `RecursionError` there, unwrapped),
//   `resolve_and_decode`, `get_text` (`LTChar.get_text`, or
//   `LTTextContainer.get_text`: `"".join` of a generator over the
//   children, recursive), `normalize_unicode` (C), `map(self.point2coord,
//   ...)` (each call from C);
// - text and tables: `utils/text.rs`, `table.rs`, `utils/geometry.rs`,
//   `utils/clustering.rs` (wave 1) count no frames: they never recurse,
//   and pdfplumber calls them from a user's level; their only
//   depth-dependent behavior is C recursion inside values (`==`, `<`,
//   `hash`, `repr` of nested `tuple`/`list` values: an `mcid` list nested
//   10^4 deep compared by `extract_words(extra_attrs=["mcid"])` or
//   `dedupe_chars(extra_attrs=("mcid",))`), which `obj.rs` does not count
//   yet: to add (C units per level, as `PDFObject::repr` counts them) with
//   cases of `adv-plumber` at the limit;
// - `annots` -> `resolve_all(self.page_obj.annots)` -> `parse` (from C, a
//   `map`), `rotate_point`, `_normalize_box`, `_invert_box`; the
//   `warnings.warn` of `raise_unicode_errors=False` runs Python frames of
//   `warnings` (`_showwarnmsg`, `_formatwarnmsg`...: to measure);
// - `to_json`/`to_csv`: `crate::container`, `crate::convert`.
// `decode_text` of a `str` or `bytes` (`begin_tag`) is counted
// (`pdfinternals.rs`).

/// The class of a page and what a derived page adds.
pub enum PageKind {
    /// pdfplumber/page.py:186 `Page`
    Original,
    /// pdfplumber/page.py:677 `CroppedPage`: `crop_bbox` as given (or
    /// offset by the parent's origin when `relative`), `_crop_fn` the
    /// function given.
    Cropped { parent: Rc<Page>, crop_bbox: PyValue, crop_fn: CropFn },
    /// pdfplumber/page.py:717 `FilteredPage`
    Filtered { parent: Rc<Page>, filter_fn: FilterFn },
}

impl PageKind {
    /// The parent of a derived page, taken out (the kind becomes
    /// `Original`).
    fn take_parent(&mut self) -> Option<Rc<Page>> {
        match std::mem::replace(self, PageKind::Original) {
            PageKind::Original => None,
            PageKind::Cropped { parent, .. } | PageKind::Filtered { parent, .. } => Some(parent),
        }
    }
}

/// Frees a chain of derived pages (each the `crop`, `filter` or
/// `dedupe_chars` of the previous one) without recursion: the parent each
/// page holds is unlinked in a loop, as CPython frees such a chain without
/// recursing (the "trashcan" of `subtype_dealloc`); a recursive drop would
/// overflow the native stack after some 10^4 pages under WebAssembly.
impl Drop for Page {
    fn drop(&mut self) {
        let mut next = self.kind.take_parent();
        while let Some(parent) = next {
            next = match Rc::try_unwrap(parent) {
                // the last owner: its own parent next, then it drops with
                // no parent left
                Ok(mut page) => page.kind.take_parent(),
                Err(_) => None,
            };
        }
    }
}

/// The `crop_fn` of a `CroppedPage` (`crop` and `within_bbox`/
/// `outside_bbox`): `utils.crop_to_bbox`, `utils.within_bbox`,
/// `utils.outside_bbox` (`crate::utils::geometry`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CropFn {
    CropToBbox,
    WithinBbox,
    OutsideBbox,
}

/// `test_function` of `Page.filter`: called on each object (from C:
/// `filter()`), its result taken for its truthiness. The JavaScript
/// boundary evaluates a JavaScript function on the objects already made
/// and gives a mask (`docs/architecture.md`, "Frontière JavaScript").
pub type FilterFn = Rc<dyn Fn(&Obj) -> PyResult<PyValue>>;

/// The `lru_cache` of `get_textmap`: the text maps by keyword arguments
/// (in the order given, values after `tuplify_list_kwargs`), at most 128,
/// least recently used dropped first.
#[derive(Default)]
pub struct TextMapCache {
    pub entries: Vec<(TextMapKey, Rc<TextMap>)>,
}

/// The key of a [`TextMapCache`] entry: the keyword arguments, in order.
pub type TextMapKey = Vec<(PyStr, HashKey)>;

/// The cached properties of a page besides `ContainerCache`.
#[derive(Default)]
pub struct PageCache {
    /// `_layout`
    pub layout: Option<Rc<LTPage<PlumberPayload>>>,
    /// `_objects`
    pub objects: Option<Rc<Objects>>,
    /// `get_textmap`'s cache
    pub textmaps: TextMapCache,
}

/// pdfplumber/page.py:186 `Page` (and its subclasses, [`PageKind`]).
pub struct Page {
    /// `self.pdf`
    pub pdf: Rc<PdfCore>,
    /// `self.root_page`: `None` for an original page (itself).
    pub root_page: Option<Rc<Page>>,
    /// `self.page_obj`
    pub page_obj: Rc<PDFPage>,
    /// `self.page_number`
    pub page_number: PyInt,
    /// `self.initial_doctop`: the sum of the heights of the pages before
    /// it (`PDF.pages`), a number.
    pub initial_doctop: PyValue,
    /// `self.rotation`
    pub rotation: PyValue,
    /// `self.mediabox`, `self.cropbox`: tuples of four numbers.
    pub mediabox: PyValue,
    pub cropbox: PyValue,
    /// `self.trimbox`, `self.bleedbox`, `self.artbox`: set only when the
    /// page has the box (`hasattr`).
    pub trimbox: Option<PyValue>,
    pub bleedbox: Option<PyValue>,
    pub artbox: Option<PyValue>,
    /// `self.bbox`: the mediabox, or the crop box as given (a `tuple` or a
    /// `list`: messages print it).
    pub bbox: PyValue,
    pub kind: PageKind,
    /// This page as a value ([`PageRef`]: `annot["data"]["P"]`).
    value: PageRef,
    cache: RefCell<PageCache>,
    container: RefCell<ContainerCache>,
}

// pdfplumber/page.py:44 ALL_ATTRS
/// The attributes of a layout object that `process_object` keeps (a
/// `set`: membership only).
pub const ALL_ATTRS: [&str; 27] = [
    "adv",
    "height",
    "linewidth",
    "pts",
    "size",
    "srcsize",
    "width",
    "x0",
    "x1",
    "y0",
    "y1",
    "bits",
    "matrix",
    "upright",
    "fontname",
    "text",
    "imagemask",
    "colorspace",
    "evenodd",
    "fill",
    "non_stroking_color",
    "stroke",
    "stroking_color",
    "stream",
    "name",
    "mcid",
    "tag",
];

// The subscriptions of `typing` aliases in the annotations of the nested
// functions, evaluated each time the enclosing function runs, with their
// depths below its frame (measured: `python -m oracle.depth_probe typing`;
// `pycompat::typing_cache`).

// `process_attr`, nested in pdfplumber/page.py:353 Page.process_object:
// `item: Tuple[str, Any]`
const PROCESS_ATTR_ITEM: Subscription = Subscription::new("Tuple[str, Any]", 7, 1);
// `process_attr`, nested in pdfplumber/page.py:353 Page.process_object:
// `-> Optional[Tuple[str, Any]]` (its
// `Tuple[str, Any]` cached by the parameter's)
const PROCESS_ATTR_RETURN: Subscription = Subscription::new("Optional[Tuple[str, Any]]", 10, 2);
// `sorter`, nested in pdfplumber/page.py:464 Page.find_table: `->
// Tuple[int, T_num, T_num]`
const SORTER_RETURN: Subscription = Subscription::new("Tuple[int, T_num, T_num]", 7, 2);
// `rotate_point`, nested in pdfplumber/page.py:273 Page.annots: `pt:
// Tuple[float, float]` and `-> Tuple[float, float]`
const ROTATE_POINT_TUPLE: Subscription = Subscription::new("Tuple[float, float]", 7, 1);

// pdfplumber/page.py:83 CP936_FONTNAMES
pub const CP936_FONTNAMES: [(&[u8], &str); 5] = [
    (b"\xcb\xce\xcc\xe5", "SimSun,Regular"),
    (b"\xba\xda\xcc\xe5", "SimHei,Regular"),
    (b"\xbf\xac\xcc\xe5_GB2312", "SimKai,Regular"),
    (b"\xb7\xc2\xcb\xce_GB2312", "SimFang,Regular"),
    (b"\xc1\xa5\xca\xe9", "SimLi,Regular"),
];

// pdfplumber/page.py:92 fix_fontname_bytes
/// The name of a font given as `bytes` (a `/FontName` that is a string):
/// the subset prefix up to `+`, and the suffix through `CP936_FONTNAMES`,
/// both written as `str(b)[2:-1]` (the `repr` of the bytes without `b'`
/// and `'`: `\xcb`, `\\`...).
pub fn fix_fontname_bytes(fontname: &[u8]) -> PyResult<PyStr> {
    let _frame = py_frame()?;
    let (prefix, suffix) = match fontname.iter().position(|&b| b == b'+') {
        Some(i) => (&fontname[..=i], &fontname[i + 1..]),
        None => (&b""[..], fontname),
    };
    // str(b)[2:-1]: PyObject_Str of the bytes (a C unit)
    let inner = |b: &[u8]| -> PyResult<String> {
        drop(c_call(WHILE_STR)?);
        let r = bytes_repr(b);
        Ok(r[2..r.len() - 1].to_owned())
    };
    // CP936_FONTNAMES.get(suffix, str(suffix)[2:-1]): the default is
    // computed first
    let default = inner(suffix)?;
    let suffix_new = match CP936_FONTNAMES.iter().find(|(k, _)| *k == suffix) {
        Some((_, name)) => (*name).to_owned(),
        None => default,
    };
    Ok(PyStr::from(inner(prefix)? + &suffix_new))
}

// pdfplumber/page.py:103 tuplify_list_kwargs
/// A `list` value becomes a `tuple` (top level only).
pub fn tuplify_list_kwargs(kwargs: &Kwargs) -> PyResult<Kwargs> {
    let _frame = py_frame()?;
    let mut out = Kwargs::new();
    for (k, v) in kwargs.items() {
        let v = match v {
            PyValue::List(items) => PyValue::Tuple(items.clone()),
            PyValue::Pdf(PDFObject::List(l)) => PyValue::tuple(l.to_vec().into_iter().map(PyValue::from_pdf)),
            other => other.clone(),
        };
        out.set(k, v);
    }
    Ok(out)
}

// pdfplumber/page.py:657 test_proposed_bbox
/// `ValueError` for a box of zero area, outside the parent's box, or not
/// fully within it (each message prints both boxes as given).
pub fn test_proposed_bbox(bbox: &PyValue, parent_bbox: &PyValue) -> PyResult<()> {
    let _frame = py_frame()?;
    let message = |text: &str, with_parent: bool| -> PyErr {
        let mut m = PyStr::new("Bounding box ").concat(&bbox.py_str()).concat(&PyStr::new(text));
        if with_parent {
            m = m.concat(&parent_bbox.py_str());
        }
        PyErr::with_message(ExcKind::ValueError, &m)
    };
    leaf(1)?; // utils.calculate_area
    let bbox_area = calculate_area(bbox)?;
    if bbox_area.py_eq(&PyValue::from(0i64)) {
        return Err(message(" has an area of zero.", false));
    }
    leaf(1)?; // utils.get_bbox_overlap
    let overlap = get_bbox_overlap(&bbox_from_value(bbox)?, &bbox_from_value(parent_bbox)?)?;
    let Some(overlap) = overlap else {
        return Err(message(" is entirely outside parent page bounding box ", true));
    };
    leaf(1)?; // utils.calculate_area
    let overlap_area = calculate_area(&bbox_value(&overlap))?;
    if overlap_area.lt(&bbox_area)? {
        return Err(message(" is not fully within parent page bounding box ", true));
    }
    Ok(())
}

// pdfplumber/page.py:161 _normalize_box
/// `_normalize_box(box)` of the 4-tuple of values `Page.annots` builds
/// (`rotation` 0): every coordinate a number (`numbers.Number`: `bool`,
/// `int`, `float`), else `MalformedPDFException` with the tuple's `str()`;
/// each pair sorted.
fn normalize_box_values(b: [PyValue; 4]) -> PyResult<[PyValue; 4]> {
    let _frame = py_frame()?;
    if !b.iter().all(|x| x.as_num().is_some() && !matches!(x, PyValue::Pdf(_))) {
        let text = PyValue::tuple(b).str_counted()?;
        return Err(PyErr::new(
            ExcKind::MalformedPDFException,
            format!("Bounding box contains non-number coordinate(s): {}", text.to_string_lossy()),
        ));
    }
    let [a, b0, c, d] = b;
    let (x0, x1) = if c.lt(&a)? { (c, a) } else { (a, c) };
    let (y0, y1) = if d.lt(&b0)? { (d, b0) } else { (b0, d) };
    Ok([x0, y0, x1, y1])
}

// pdfplumber/page.py:181 _invert_box
/// `_invert_box(box, mb_height)` of values.
fn invert_box_values(b: [PyValue; 4], mb_height: &PyValue) -> PyResult<[PyValue; 4]> {
    let _frame = py_frame()?;
    let [x0, y0, x1, y1] = b;
    Ok([x0, mb_height.sub(&y1)?, x1, mb_height.sub(&y0)?])
}

/// The keyword arguments of `Page.search` besides the text map's.
#[derive(Clone, Copy, Debug)]
pub struct SearchArgs {
    pub main_group: usize,
    pub return_chars: bool,
    pub return_groups: bool,
}

const WHILE_STR: &str = " while getting the str of an object";
const WHILE_CALLING: &str = " while calling a Python object";

// The property loads of pdfplumber/page.py (`pycompat::specialize`):
// each `LOAD_ATTR` of a property, by the function it is in.
static ROTATE_POINT_WIDTH: AttrSite = AttrSite::new("Page.annots.rotate_point self.width");
static ROTATE_POINT_HEIGHT: AttrSite = AttrSite::new("Page.annots.rotate_point self.height");
static PARSE_ROOT_HEIGHT: AttrSite = AttrSite::new("Page.annots.parse self.root_page.height");
static HYPERLINKS_ANNOTS: AttrSite = AttrSite::new("Page.hyperlinks self.annots");
static POINT2COORD_HEIGHT: AttrSite = AttrSite::new("Page.point2coord self.height");
static PROCESS_OBJECT_HEIGHT_TOP: AttrSite = AttrSite::new("Page.process_object self.height (top)");
static PROCESS_OBJECT_HEIGHT_BOTTOM: AttrSite = AttrSite::new("Page.process_object self.height (bottom)");
static PARSE_OBJECTS_LAYOUT: AttrSite = AttrSite::new("Page.parse_objects self.layout");
static SORTER_BBOX_1: AttrSite = AttrSite::new("Page.find_table.sorter x.bbox[1]");
static SORTER_BBOX_0: AttrSite = AttrSite::new("Page.find_table.sorter x.bbox[0]");
static GET_TEXTMAP_WIDTH: AttrSite = AttrSite::new("Page._get_textmap self.width");
static GET_TEXTMAP_HEIGHT: AttrSite = AttrSite::new("Page._get_textmap self.height");
static GET_TEXTMAP_CHARS: AttrSite = AttrSite::new("Page._get_textmap self.chars");
static EXTRACT_TEXT_SIMPLE_CHARS: AttrSite = AttrSite::new("Page.extract_text_simple self.chars");
static EXTRACT_WORDS_CHARS: AttrSite = AttrSite::new("Page.extract_words self.chars");
static DEDUPE_CHARS_OBJECTS: AttrSite = AttrSite::new("Page.dedupe_chars self.objects");
static DEDUPE_CHARS_CHARS: AttrSite = AttrSite::new("Page.dedupe_chars self.chars");
static TO_DICT_OBJECTS: AttrSite = AttrSite::new("Page.to_dict self.objects");
static TO_DICT_WIDTH: AttrSite = AttrSite::new("Page.to_dict self.width");
static TO_DICT_HEIGHT: AttrSite = AttrSite::new("Page.to_dict self.height");
static TO_DICT_GETATTR: CallSite = CallSite::new("Page.to_dict getattr(self, t + 's')");
static CROPPED_PARENT_OBJECTS: AttrSite = AttrSite::new("CroppedPage.objects self.parent_page.objects");
static FILTERED_PARENT_OBJECTS: AttrSite = AttrSite::new("FilteredPage.objects self.parent_page.objects");

/// `maxsize` of `lru_cache()`.
const LRU_MAXSIZE: usize = 128;

/// An object `iter_layout_objects` walks: an item of a page, a figure, a
/// text box or a text line (an `LTAnno` only in a line).
#[derive(Clone)]
pub enum LayoutObj {
    Item(LTItem<PlumberPayload>),
    Anno(LTAnno),
}

impl LayoutObj {
    /// `isinstance(obj, LTContainer)`: its children, `None` for a leaf.
    fn children(&self) -> Option<Vec<LayoutObj>> {
        match self {
            LayoutObj::Item(LTItem::Figure(f)) => {
                Some(f.borrow().layout._objs.iter().cloned().map(LayoutObj::Item).collect())
            }
            LayoutObj::Item(LTItem::TextBox(b)) => {
                Some(b.borrow()._objs.iter().map(|l| LayoutObj::Item(LTItem::TextLine(l.clone()))).collect())
            }
            LayoutObj::Item(LTItem::TextLine(l)) => Some(
                l.borrow()
                    ._objs
                    .iter()
                    .map(|e| match e {
                        TextLineElement::Char(c) => LayoutObj::Item(LTItem::Char(c.clone())),
                        TextLineElement::Anno(a) => LayoutObj::Anno(a.clone()),
                    })
                    .collect(),
            ),
            _ => None,
        }
    }

    /// The Python class name.
    fn class_name(&self) -> &'static str {
        match self {
            LayoutObj::Item(item) => item.class_name(),
            LayoutObj::Anno(_) => "LTAnno",
        }
    }
}

/// An item of an object's `__dict__` as `process_attr` sees it: `None`
/// for an attribute that is not in `ALL_ATTRS` (it is only looked at),
/// else the key (shared: [`AttrKeys`]) and the value.
type DictItem = Option<(PyStr, PyValue)>;

/// The keys `process_object` writes, made once per thread (performance:
/// `intern` hashes the name; these are cloned): the same shared `str`s as
/// [`crate::obj::intern`] gives.
struct AttrKeys {
    matrix: PyStr,
    fontname: PyStr,
    adv: PyStr,
    upright: PyStr,
    x0: PyStr,
    y0: PyStr,
    x1: PyStr,
    y1: PyStr,
    width: PyStr,
    height: PyStr,
    size: PyStr,
    pts: PyStr,
    linewidth: PyStr,
    stroke: PyStr,
    fill: PyStr,
    evenodd: PyStr,
    stroking_color: PyStr,
    non_stroking_color: PyStr,
    name: PyStr,
    stream: PyStr,
    srcsize: PyStr,
    imagemask: PyStr,
    bits: PyStr,
    colorspace: PyStr,
    object_type: PyStr,
    page_number: PyStr,
    ncs: PyStr,
    text: PyStr,
    path: PyStr,
    dash: PyStr,
    top: PyStr,
    bottom: PyStr,
    doctop: PyStr,
    mcid: PyStr,
    tag: PyStr,
}

impl AttrKeys {
    fn new() -> AttrKeys {
        let k = crate::obj::intern;
        AttrKeys {
            matrix: k("matrix"),
            fontname: k("fontname"),
            adv: k("adv"),
            upright: k("upright"),
            x0: k("x0"),
            y0: k("y0"),
            x1: k("x1"),
            y1: k("y1"),
            width: k("width"),
            height: k("height"),
            size: k("size"),
            pts: k("pts"),
            linewidth: k("linewidth"),
            stroke: k("stroke"),
            fill: k("fill"),
            evenodd: k("evenodd"),
            stroking_color: k("stroking_color"),
            non_stroking_color: k("non_stroking_color"),
            name: k("name"),
            stream: k("stream"),
            srcsize: k("srcsize"),
            imagemask: k("imagemask"),
            bits: k("bits"),
            colorspace: k("colorspace"),
            object_type: k("object_type"),
            page_number: k("page_number"),
            ncs: k("ncs"),
            text: k("text"),
            path: k("path"),
            dash: k("dash"),
            top: k("top"),
            bottom: k("bottom"),
            doctop: k("doctop"),
            mcid: k("mcid"),
            tag: k("tag"),
        }
    }
}

thread_local! {
    static ATTR_KEYS: AttrKeys = AttrKeys::new();
    /// The buffer of [`dict_items`], kept between objects.
    static DICT_ITEMS: RefCell<Vec<DictItem>> = const { RefCell::new(Vec::new()) };
    /// The last color space whose name [`colorspace_name`] made, and that
    /// name (the chars of a page share a few color spaces).
    static LAST_COLORSPACE: RefCell<Option<(Rc<PDFColorSpace>, PyStr)>> = const { RefCell::new(None) };
    /// `object_type` of each class (`re.sub(lt_pat, "", name).lower()`),
    /// made once per thread: a `str` is immutable and its identity is not
    /// modeled.
    static OBJECT_TYPES: RefCell<Vec<(&'static str, PyStr)>> = const { RefCell::new(Vec::new()) };
}

/// The `str` of a path operator (`cmd` of `obj.original_path`), made once
/// per thread.
fn path_command(cmd: char) -> PyStr {
    thread_local! {
        static COMMANDS: RefCell<Vec<(char, PyStr)>> = const { RefCell::new(Vec::new()) };
    }
    COMMANDS.with(|cmds| {
        let mut cmds = cmds.borrow_mut();
        if let Some((_, s)) = cmds.iter().find(|(c, _)| *c == cmd) {
            return s.clone();
        }
        let s = PyStr::from(cmd);
        cmds.push((cmd, s.clone()));
        s
    })
}

/// `cs.name`, a `str`, made once for consecutive chars of one color space
/// (performance; the cache holds the color space, whose address cannot
/// name another one meanwhile).
fn colorspace_name(cs: &Rc<PDFColorSpace>) -> PyStr {
    LAST_COLORSPACE.with(|last| {
        let mut last = last.borrow_mut();
        if let Some((c, name)) = &*last {
            if Rc::ptr_eq(c, cs) {
                return name.clone();
            }
        }
        let name = PyStr::new(&cs.name);
        *last = Some((cs.clone(), name.clone()));
        name
    })
}

/// `re.sub(lt_pat, "", class_name).lower()` (`lt_pat = re.compile(r"^LT")`).
fn object_type(class_name: &'static str) -> PyStr {
    OBJECT_TYPES.with(|types| {
        let mut types = types.borrow_mut();
        if let Some((_, t)) = types.iter().find(|(c, _)| *c == class_name) {
            return t.clone();
        }
        let t = PyStr::new(&class_name.strip_prefix("LT").unwrap_or(class_name).to_lowercase());
        types.push((class_name, t.clone()));
        t
    })
}

fn numv(n: &Num) -> PyValue {
    PyValue::from(n.clone())
}

fn point_value(p: &Point) -> PyValue {
    PyValue::tuple(p.iter().map(numv))
}

fn standard_color(c: &StandardColor) -> PyValue {
    match c {
        StandardColor::Gray(g) => numv(g),
        StandardColor::Rgb(rgb) => PyValue::tuple(rgb.iter().map(|f| PyValue::float(*f))),
        StandardColor::Cmyk(cmyk) => PyValue::tuple(cmyk.iter().map(|f| PyValue::float(*f))),
    }
}

/// A color of the interpreter as Python holds it (a number, a tuple, a
/// pattern name, or `(base_color, pattern_name)`).
pub fn color_value(c: &Color) -> PyValue {
    match c {
        Color::Standard(c) => standard_color(c),
        Color::Pattern(name) => PyValue::str(name),
        Color::UncoloredPattern(c, name) => PyValue::tuple([standard_color(c), PyValue::str(name)]),
    }
}

/// `gs.scolor if isinstance(gs.scolor, tuple) else (gs.scolor,)`
fn color_tuple(c: &Color) -> PyValue {
    match color_value(c) {
        t @ PyValue::Tuple(_) => t,
        other => PyValue::tuple([other]),
    }
}

fn marked(v: &MarkedContentAttr) -> PyValue {
    match v {
        MarkedContentAttr::Mcid(o) => PyValue::from_pdf(o.clone()),
        MarkedContentAttr::Tag(t) => t.as_ref().map_or(PyValue::None, |t| PyValue::Str(t.clone())),
    }
}

/// The items of `LTComponent.__init__`: `x0`, `y0`, `x1`, `y1`, `width`,
/// `height`, then `bbox` (not in `ALL_ATTRS`).
fn comp_items(out: &mut Vec<DictItem>, k: &AttrKeys, c: &LTComponent) {
    out.extend([
        Some((k.x0.clone(), numv(&c.x0))),
        Some((k.y0.clone(), numv(&c.y0))),
        Some((k.x1.clone(), numv(&c.x1))),
        Some((k.y1.clone(), numv(&c.y1))),
        Some((k.width.clone(), numv(&c.width))),
        Some((k.height.clone(), numv(&c.height))),
        None,
    ]);
}

/// The attributes pdfplumber set after `__init__` (`mcid`, `tag`): never
/// one of the class's own.
fn extra_items(out: &mut Vec<DictItem>, k: &AttrKeys, attrs: &ExtraAttrs<MarkedContentAttr>) {
    attrs.with_items(|items| {
        out.extend(items.iter().map(|(name, v)| {
            let key = match *name {
                "mcid" => k.mcid.clone(),
                "tag" => k.tag.clone(),
                other => crate::obj::intern(other),
            };
            Some((key, marked(v)))
        }));
    });
}

/// `obj.__dict__.items()` into `out`, the values of `ALL_ATTRS` made (see
/// [`DictItem`]): the class's attributes in `__init__` order, then those
/// pdfplumber set later (`mcid`, `tag`). The keys are distinct (a
/// `__dict__`'s).
fn dict_items(out: &mut Vec<DictItem>, k: &AttrKeys, obj: &LayoutObj) {
    match obj {
        LayoutObj::Anno(_) => {
            out.push(None); // _text
        }
        LayoutObj::Item(LTItem::Char(c)) => {
            out.push(None); // _text
            out.push(Some((k.matrix.clone(), PyValue::tuple(c.matrix.iter().map(numv)))));
            out.push(Some((
                k.fontname.clone(),
                match &c.fontname {
                    FontName::Str(s) => PyValue::Str(s.clone()),
                    FontName::Obj(o) => PyValue::from_pdf(o.clone()),
                },
            )));
            out.push(None); // ncs
            out.push(None); // graphicstate
            out.push(Some((k.adv.clone(), numv(&c.adv))));
            out.push(Some((k.upright.clone(), PyValue::Bool(c.upright))));
            comp_items(out, k, &c.comp);
            out.push(Some((k.size.clone(), numv(&c.size))));
            extra_items(out, k, &c.attrs);
        }
        LayoutObj::Item(LTItem::Curve(c)) => {
            comp_items(out, k, &c.comp);
            out.push(Some((k.pts.clone(), PyValue::list(c.pts.iter().map(point_value)))));
            out.push(Some((k.linewidth.clone(), numv(&c.linewidth))));
            out.push(Some((k.stroke.clone(), PyValue::Bool(c.stroke))));
            out.push(Some((k.fill.clone(), PyValue::Bool(c.fill))));
            out.push(Some((k.evenodd.clone(), PyValue::Bool(c.evenodd))));
            let color = |c: &Option<Color>| c.as_ref().map_or(PyValue::None, color_value);
            out.push(Some((k.stroking_color.clone(), color(&c.stroking_color))));
            out.push(Some((k.non_stroking_color.clone(), color(&c.non_stroking_color))));
            out.push(None); // original_path
            out.push(None); // dashing_style
            extra_items(out, k, &c.attrs);
        }
        LayoutObj::Item(LTItem::Image(i)) => {
            comp_items(out, k, &i.comp);
            out.push(Some((k.name.clone(), PyValue::str(&i.name))));
            out.push(Some((k.stream.clone(), PyValue::Pdf(PDFObject::Stream(i.stream.clone())))));
            out.push(Some((
                k.srcsize.clone(),
                PyValue::tuple([PyValue::from_pdf(i.srcsize.0.clone()), PyValue::from_pdf(i.srcsize.1.clone())]),
            )));
            out.push(Some((k.imagemask.clone(), PyValue::from_pdf(i.imagemask.clone()))));
            out.push(Some((k.bits.clone(), PyValue::from_pdf(i.bits.clone()))));
            out.push(Some((k.colorspace.clone(), PyValue::from_pdf(i.colorspace.clone()))));
            extra_items(out, k, &i.attrs);
        }
        LayoutObj::Item(LTItem::Figure(f)) => {
            let f = f.borrow();
            out.push(Some((k.name.clone(), PyValue::str(&f.name))));
            out.push(Some((k.matrix.clone(), PyValue::tuple(f.matrix.iter().map(numv)))));
            comp_items(out, k, &f.layout.comp);
            out.push(None); // _objs
            out.push(None); // groups
            extra_items(out, k, &f.attrs);
        }
        LayoutObj::Item(LTItem::TextLine(l)) => {
            let l = l.borrow();
            comp_items(out, k, &l.comp);
            out.push(None); // _objs
            out.push(None); // word_margin
            out.push(None); // _x1 (horizontal) or _y0 (vertical)
        }
        LayoutObj::Item(LTItem::TextBox(b)) => {
            comp_items(out, k, &b.borrow().comp);
            out.push(None); // _objs
            out.push(None); // index
        }
    }
}

// pdfplumber/utils/pdfinternals.py:66 resolve_all
/// `resolve_all(x)` of a value of the object model: a PDF object through
/// `pdfinternals::resolve_all` (its frame), a Python `tuple` or `list`
/// item by item (`type(x)(resolve_all(v) for v in x)`: the generator
/// expression resumed from C), a `dict` value by value, anything else as
/// it is.
pub fn resolve_all_value(x: &PyValue) -> PyResult<PyValue> {
    if let PyValue::Pdf(o) = x {
        return Ok(PyValue::from_pdf(resolve_all(o)?));
    }
    let _frame = py_frame()?;
    match x {
        PyValue::Tuple(items) | PyValue::List(items) => {
            let _entry = eval_entry()?;
            let _genexpr = py_frame()?;
            let mut out = Vec::with_capacity(items.len());
            for v in items.iter() {
                out.push(resolve_all_value(v)?);
            }
            Ok(if matches!(x, PyValue::Tuple(_)) { PyValue::Tuple(out.into()) } else { PyValue::List(out.into()) })
        }
        PyValue::Dict(d) => {
            // get_dict_type(x) == "Annot": the decoded name of a literal /Type
            let annot = {
                let _frame = py_frame()?;
                match d.get("Type") {
                    Some(PyValue::Pdf(PDFObject::Literal(lit))) => {
                        let name = match lit.name() {
                            LiteralName::Str(s) => decode_text(TextArg::Str(s))?,
                            LiteralName::Bytes(b) => decode_text(TextArg::Bytes(b))?,
                        };
                        name == "Annot"
                    }
                    Some(t) => t.py_eq(&PyValue::str("Annot")),
                    None => false,
                }
            };
            let mut out = Obj::new();
            for (k, v) in d.items() {
                let value = if annot && *k == "Parent" { v.clone() } else { resolve_all_value(v)? };
                out.set(k, value);
            }
            Ok(PyValue::Dict(out))
        }
        other => Ok(other.clone()),
    }
}

// pdfplumber/utils/pdfinternals.py:66 resolve_all
/// [`resolve_all_value`] of a value nothing else holds (an attribute
/// value `process_object` has just made): the same frames, and the same
/// result, but a `tuple` or `list` that only `x` holds is resolved in
/// place instead of copied (performance: Python makes a new one, whose
/// identity no one can compare with the old one's, which only it held).
pub fn resolve_all_owned(mut x: PyValue) -> PyResult<PyValue> {
    if let PyValue::Tuple(items) | PyValue::List(items) = &mut x {
        if let Some(items) = Rc::get_mut(items) {
            let _frame = py_frame()?;
            let _entry = eval_entry()?;
            let _genexpr = py_frame()?;
            for v in items.iter_mut() {
                *v = resolve_all_owned(std::mem::take(v))?;
            }
            return Ok(x);
        }
    }
    resolve_all_value(&x)
}

/// `x[i]`, `IndexError`... as Python's subscription of a sequence raises.
fn index(v: &PyValue, i: usize) -> PyResult<PyValue> {
    crate::obj::py_getitem_index(v, i)
}

impl Page {
    // pdfplumber/page.py:191 Page.__init__
    /// `Page(pdf, page_obj, page_number=page_number,
    /// initial_doctop=initial_doctop)`: the class call, `__init__`.
    pub fn new(pdf: Rc<PdfCore>, page_obj: Rc<PDFPage>, page_number: PyInt, initial_doctop: PyValue) -> PyResult<Page> {
        let _call = class_call()?;
        let doctop = initial_doctop.as_num().expect("PDF.pages accumulates numbers");
        // the boxes, in __init__'s frame (PageBoxes::new takes it)
        let boxes = PageBoxes::new(&page_obj, doctop)?;
        {
            // self.get_textmap = lru_cache()(self._get_textmap): lru_cache,
            // then decorating_function -> update_wrapper, in __init__'s frame
            let _init = py_frame()?;
            leaf(1)?;
            leaf(2)?;
        }
        let tuple = |b: &BBox| PyValue::tuple(b.iter().cloned().map(PyValue::from_pdf));
        let mediabox = tuple(&boxes.mediabox);
        let cropbox = if page_obj.attrs.contains_key("CropBox") { tuple(&boxes.cropbox) } else { mediabox.clone() };
        Ok(Page {
            pdf,
            root_page: None,
            page_number: page_number.clone(),
            initial_doctop: PyValue::from(boxes.initial_doctop.clone()),
            rotation: PyValue::from_pdf(boxes.rotation.clone()),
            bbox: mediabox.clone(),
            mediabox,
            cropbox,
            trimbox: boxes.trimbox.as_ref().map(tuple),
            bleedbox: boxes.bleedbox.as_ref().map(tuple),
            artbox: boxes.artbox.as_ref().map(tuple),
            page_obj,
            kind: PageKind::Original,
            value: PageRef::new(page_number),
            cache: RefCell::new(PageCache::default()),
            container: RefCell::new(ContainerCache::default()),
        })
    }

    // pdfplumber/page.py:643 DerivedPage.__init__
    /// A derived page of `parent`: `DerivedPage.__init__` (a frame) copies
    /// what the parent's `__init__` set but the optional boxes, flushes
    /// `Container.cached_properties` (none is set yet) and makes its
    /// `get_textmap` cache (`lru_cache`).
    fn derived(parent: &Rc<Page>, kind: PageKind, bbox: PyValue) -> PyResult<Page> {
        let _frame = py_frame()?;
        leaf(1)?; // self.flush_cache(Container.cached_properties)
        leaf(1)?; // lru_cache()
        leaf(2)?; // decorating_function -> update_wrapper
        Ok(Page {
            pdf: parent.pdf.clone(),
            root_page: Some(parent.root_page.clone().unwrap_or_else(|| parent.clone())),
            page_obj: parent.page_obj.clone(),
            page_number: parent.page_number.clone(),
            initial_doctop: parent.initial_doctop.clone(),
            rotation: parent.rotation.clone(),
            mediabox: parent.mediabox.clone(),
            cropbox: parent.cropbox.clone(),
            trimbox: None,
            bleedbox: None,
            artbox: None,
            bbox,
            kind,
            value: PageRef::new(parent.page_number.clone()),
            cache: RefCell::new(PageCache::default()),
            container: RefCell::new(ContainerCache::default()),
        })
    }

    /// This page as a value (the same object each time).
    pub fn as_value(&self) -> PageRef {
        self.value.clone()
    }

    /// `self.root_page`: the original page this one derives from (itself
    /// for an original page).
    pub fn root_page<'a>(&'a self, this: &'a Rc<Page>) -> &'a Rc<Page> {
        self.root_page.as_ref().unwrap_or(this)
    }

    /// The Python class name (`Page`, `CroppedPage`, `FilteredPage`).
    pub fn class_name(&self) -> &'static str {
        match self.kind {
            PageKind::Original => "Page",
            PageKind::Cropped { .. } => "CroppedPage",
            PageKind::Filtered { .. } => "FilteredPage",
        }
    }

    // pdfplumber/page.py:235 Page.close
    /// `self.flush_cache()`, then `self.get_textmap.cache_clear()`.
    pub fn close(&self) -> PyResult<()> {
        let _frame = py_frame()?;
        leaf(1)?; // flush_cache
        Container::flush_cache(self, None);
        self.cache.borrow_mut().textmaps.entries.clear();
        Ok(())
    }

    /// `self.bbox[i]`
    fn bbox_item(&self, i: usize) -> PyResult<PyValue> {
        index(&self.bbox, i)
    }

    // pdfplumber/page.py:240 Page.width
    pub fn width(&self) -> PyResult<PyValue> {
        let _frame = py_frame()?;
        self.bbox_item(2)?.sub(&self.bbox_item(0)?)
    }

    // pdfplumber/page.py:244 Page.height
    pub fn height(&self) -> PyResult<PyValue> {
        let _frame = py_frame()?;
        self.bbox_item(3)?.sub(&self.bbox_item(1)?)
    }

    // pdfplumber/page.py:248 Page.structure_tree
    /// Workstream DOC (`crate::structure`): the `to_dict()` of each element
    /// of `PDFStructTree(self.pdf, self)`, `[]` without a structure tree.
    pub fn structure_tree(self: &Rc<Self>) -> PyResult<Vec<PyValue>> {
        let _frame = py_frame()?;
        let tree = match PDFStructTree::new(&self.pdf.doc, Scope::Page(self)) {
            Ok(tree) => tree,
            Err(e) if e.is_instance(ExcKind::StructTreeMissing) => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        // [elem.to_dict() for elem in tree]: PDFStructTree.__iter__, run by C
        c_called_leaf()?;
        let file = self.pdf.doc.file_kind()?;
        tree.iter().iter().map(|elem| elem.to_dict(file)).collect()
    }

    // pdfplumber/page.py:256 Page.layout
    /// The page's `LTPage` (analyzed when the `PDF` has `laparams`, with
    /// the `IdSource` of [`PdfCore::ids`]), cached; `PdfminerException(e)`
    /// when the interpretation raises (nothing cached). See [`page_layout`].
    pub fn layout(&self) -> PyResult<Rc<LTPage<PlumberPayload>>> {
        if let Some(layout) = &self.cache.borrow().layout {
            leaf(1)?;
            return Ok(layout.clone());
        }
        let rsrcmgr = &self.pdf.rsrcmgr;
        let (device, _interpreter, error) = page_layout(rsrcmgr, &self.page_obj, || {
            let mut device = PDFPageAggregatorWithMarkedContent::create(
                rsrcmgr.clone(),
                self.page_number.clone(),
                self.pdf.laparams.clone(),
            )?;
            if let Some(ids) = &self.pdf.ids {
                device.aggregator.analyzer.ids = ids(&self.page_number);
            }
            Ok(device)
        })?;
        if let Some(e) = error {
            // raise PdfminerException(e), in the property's frame
            let _frame = py_frame()?;
            drop(c_call(WHILE_CALLING)?);
            return Err(pdfminer_exception(e));
        }
        // self._layout = device.get_result(), in the property's frame
        leaf(2)?;
        let result = device.borrow_mut().aggregator.result.take();
        let Some(ltpage) = result else { return Err(exc::AssertionError("")) };
        let ltpage = Rc::new(ltpage);
        self.cache.borrow_mut().layout = Some(ltpage.clone());
        Ok(ltpage)
    }

    // pdfplumber/page.py:273 Page.annots
    /// Workstream DOC. The annotations of the page (`resolve_all` of
    /// `/Annots`), each parsed (`rotate_point`, `parse`, nested): its
    /// geometry, `uri`/`title`/`contents` decoded (UTF-8, then UTF-16; with
    /// `raise_unicode_errors` false, a `warnings.warn` and the bytes kept),
    /// and `data`, the annotation's dict as a [`PyValue::Dict`] whose `P`,
    /// when present, is this page ([`Page::as_value`]); a `CroppedPage`
    /// crops them with its `_crop_fn`.
    pub fn annots(&self) -> PyResult<Vec<Obj>> {
        let _frame = py_frame()?;
        // def rotate_point(pt: Tuple[float, float], r: int) -> Tuple[float,
        // float]: its annotations, evaluated at each call (the second one
        // hits the cache the first one filled)
        ROTATE_POINT_TUPLE.eval()?;
        ROTATE_POINT_TUPLE.eval()?;
        let raw = resolve_all(&self.page_obj.annots)?;
        let raw = if raw.is_truthy() { raw } else { PDFObject::List(PDFList::default()) };
        // list(map(parse, raw)): parse is called from C for each item
        let mut parsed = Vec::new();
        for annot in structure::py_iter(&raw)? {
            let _entry = eval_entry()?;
            parsed.push(self.parse_annot(annot)?);
        }
        match &self.kind {
            PageKind::Cropped { crop_bbox, crop_fn, .. } => apply_crop_fn(*crop_fn, crop_bbox, &parsed),
            _ => Ok(parsed),
        }
    }

    // `rotate_point`, nested in pdfplumber/page.py:273 Page.annots
    /// The nested `rotate_point(pt, r)`: `r // 90` quarter turns, each
    /// `(x, y) -> (y, comp - x)`, `comp` this page's width or height.
    fn rotate_point(&self, pt: (PyValue, PyValue), r: &PyValue) -> PyResult<(PyValue, PyValue)> {
        let _frame = py_frame()?;
        let turns = match r.as_num() {
            Some(n) => n.floordiv(&Num::from(90))?,
            None => {
                return Err(exc::TypeError(format!(
                    "unsupported operand type(s) for //: '{}' and 'int'",
                    r.type_name()
                )))
            }
        };
        // range(turns)
        let turns = match turns {
            Num::Int(i) => {
                i.as_i64().ok_or_else(|| exc::OverflowError("Python int too large to convert to C ssize_t"))?
            }
            Num::Float(_) => return Err(exc::TypeError("'float' object cannot be interpreted as an integer")),
        };
        let mut pt = pt;
        for i in 0..turns.max(0) {
            let (x, y) = pt;
            // `i == turns % 2`
            let comp = if i == turns.rem_euclid(2) {
                ROTATE_POINT_WIDTH.get(self.class_name(), || self.width())?
            } else {
                ROTATE_POINT_HEIGHT.get(self.class_name(), || self.height())?
            };
            pt = (y, comp.sub(&x)?);
        }
        Ok(pt)
    }

    // `parse`, nested in pdfplumber/page.py:273 Page.annots
    /// The nested `parse(annot)`: an annotation's geometry (its `/Rect`
    /// turned with the page and flipped), `uri` (`/A`'s `/URI`), `title`
    /// (`/T`) and `contents` (`/Contents`) decoded as UTF-8 then UTF-16, and
    /// `data`, the annotation, whose `/P` becomes this page.
    fn parse_annot(&self, annot: PDFObject) -> PyResult<Obj> {
        let _frame = py_frame()?;
        let [a, b, c, d] = bbox_from_value(&PyValue::from_pdf(structure::py_getitem(&annot, "Rect")?))?;
        let pt0 = self.rotate_point((a, b), &self.rotation)?;
        let pt1 = self.rotate_point((c, d), &self.rotation)?;
        let rh = PARSE_ROOT_HEIGHT.get(self.root().class_name(), || self.root().height())?;
        let [x0, top, x1, bottom] = invert_box_values(normalize_box_values([pt0.0, pt0.1, pt1.0, pt1.1])?, &rh)?;

        let a = structure::py_get(&annot, "A", PDFObject::Dict(PDFDict::new()))?;
        let mut extras = [
            ("uri", structure::py_get(&a, "URI", PDFObject::Null)?),
            ("title", structure::py_get(&annot, "T", PDFObject::Null)?),
            ("contents", structure::py_get(&annot, "Contents", PDFObject::Null)?),
        ]
        .map(|(k, v)| (k, PyValue::from_pdf(v)));
        for (k, v) in extras.iter_mut() {
            if v.is_none() {
                continue;
            }
            let PyValue::Bytes(bytes) = &*v else {
                return Err(exc::AttributeError(format!("'{}' object has no attribute 'decode'", v.type_name())));
            };
            match codecs::decode_utf8(bytes) {
                Ok(text) => *v = PyValue::Str(text),
                Err(_) => match codecs::decode_utf16(bytes) {
                    Ok(text) => *v = PyValue::Str(text),
                    Err(e) => {
                        if self.pdf.raise_unicode_errors {
                            return Err(e);
                        }
                        // warn(...): shown once per text (the default
                        // filters' registry, pycompat::warnings)
                        pycompat::warnings::warn(
                            "UserWarning",
                            "pdfplumber.page:305",
                            format!("Could not decode {k} of annotation. {k} will be missing."),
                        );
                    }
                },
            }
        }

        let mut parsed = Obj::new();
        parsed.set("page_number", PyValue::Int(self.page_number.clone()));
        parsed.set("object_type", PyValue::str("annot"));
        parsed.set("x0", x0.clone());
        parsed.set("y0", rh.sub(&bottom)?);
        parsed.set("x1", x1.clone());
        parsed.set("y1", rh.sub(&top)?);
        parsed.set("doctop", self.initial_doctop.add(&top)?);
        parsed.set("top", top.clone());
        parsed.set("bottom", bottom.clone());
        parsed.set("width", x1.sub(&x0)?);
        parsed.set("height", bottom.sub(&top)?);
        for (k, v) in extras {
            parsed.set(k, v);
        }
        // Replace the indirect reference to the page dictionary with a
        // pointer to our actual page
        let as_obj = |d: &PDFDict| Obj::from_pairs(d.items().into_iter().map(|(k, v)| (k, PyValue::from_pdf(v))));
        let data = if structure::py_in(&annot, "P")? {
            match &annot {
                PDFObject::Dict(d) => {
                    let mut data = as_obj(d);
                    data.set("P", PyValue::Page(self.as_value()));
                    PyValue::Dict(data)
                }
                other => {
                    return Err(exc::TypeError(format!(
                        "'{}' object does not support item assignment",
                        other.type_name()
                    )))
                }
            }
        } else {
            match &annot {
                PDFObject::Dict(d) => PyValue::Dict(as_obj(d)),
                other => PyValue::from_pdf(other.clone()),
            }
        };
        parsed.set("data", data);
        Ok(parsed)
    }

    /// `self.root_page`
    fn root(&self) -> &Page {
        self.root_page.as_deref().unwrap_or(self)
    }

    // pdfplumber/page.py:339 Page.hyperlinks
    /// Workstream DOC: the annotations whose `uri` is not `None`
    /// (`self.annots` computed again).
    pub fn hyperlinks(&self) -> PyResult<Vec<Obj>> {
        let _frame = py_frame()?;
        let annots = HYPERLINKS_ANNOTS.get(self.class_name(), || self.annots())?;
        Ok(annots.into_iter().filter(|a| !a.get("uri").is_some_and(PyValue::is_none)).collect())
    }

    // pdfplumber/page.py:343 Page.objects
    // pdfplumber/page.py:708 CroppedPage.objects
    // pdfplumber/page.py:724 FilteredPage.objects
    /// The objects by kind, cached: `parse_objects` for an original page,
    /// the parent's through `_crop_fn` or `filter_fn` for a derived one.
    pub fn objects(&self) -> PyResult<Rc<Objects>> {
        let _frame = py_frame()?;
        if let Some(objects) = &self.cache.borrow().objects {
            return Ok(objects.clone());
        }
        let objects = match &self.kind {
            PageKind::Original => self.parse_objects()?,
            PageKind::Cropped { parent, crop_bbox, crop_fn } => {
                let mut out = Objects::new();
                // a dict comprehension: no frame of its own
                for (k, v) in CROPPED_PARENT_OBJECTS.get(parent.class_name(), || parent.objects())?.iter() {
                    out.insert(k.clone(), apply_crop_fn(*crop_fn, crop_bbox, v)?);
                }
                out
            }
            PageKind::Filtered { parent, filter_fn } => {
                let mut out = Objects::new();
                for (k, v) in FILTERED_PARENT_OBJECTS.get(parent.class_name(), || parent.objects())?.iter() {
                    // list(filter(self.filter_fn, v)): each call from C
                    let mut kept = Vec::new();
                    for obj in v {
                        let _entry = eval_entry()?;
                        let _frame = py_frame()?;
                        if filter_fn(obj)?.truthy() {
                            kept.push(obj.clone());
                        }
                    }
                    out.insert(k.clone(), kept);
                }
                out
            }
        };
        let objects = Rc::new(objects);
        self.cache.borrow_mut().objects = Some(objects.clone());
        Ok(objects)
    }

    // pdfplumber/page.py:349 Page.point2coord
    pub fn point2coord(&self, pt: &PyValue) -> PyResult<PyValue> {
        let _frame = py_frame()?;
        // See note below re. #1181 and mediabox-adjustment reversions
        let x = index(&self.mediabox, 0)?.add(&index(pt, 0)?)?;
        let height = POINT2COORD_HEIGHT.get(self.class_name(), || self.height())?;
        let y = index(&self.mediabox, 1)?.add(&height)?.sub(&index(pt, 1)?)?;
        Ok(PyValue::tuple([x, y]))
    }

    /// `map(self.point2coord, pts)` consumed from C.
    fn points2coords(&self, pts: &[PyValue]) -> PyResult<Vec<PyValue>> {
        let mut out = Vec::with_capacity(pts.len());
        for pt in pts {
            let _entry = eval_entry()?;
            out.push(self.point2coord(pt)?);
        }
        Ok(out)
    }

    // pdfplumber/page.py:353 Page.process_object
    /// The dict of a layout object: the attributes of `ALL_ATTRS` in the
    /// order of its `__dict__` (through `resolve_all`), `object_type` (the
    /// class name without `LT`, lowercased), `page_number`, `ncs`/`scs`,
    /// `text` (`unicode_norm`), the colors of a char's graphic state,
    /// `fontname` bytes fixed, a curve's `pts`, `path`, `dash`, then `top`,
    /// `bottom`, `doctop`, and `x0`/`x1` shifted by a mediabox that does
    /// not start at 0. Inline image names are `inline-<k>`
    /// (`docs/architecture.md`, "Vague 3").
    fn process_layout_obj(&self, obj: &LayoutObj) -> PyResult<Obj> {
        ATTR_KEYS.with(|k| self.process_layout_obj_with(obj, k))
    }

    /// [`Page::process_layout_obj`] with the shared keys. Performance
    /// (`docs/architecture.md`, "Vague 5", PERF): the dict is made with
    /// room for its items, and a key it cannot hold yet (an attribute of
    /// the `__dict__`, whose keys are distinct; a key `process_object`
    /// adds that no layout class has) is appended without a look-up
    /// ([`Obj::push_new`]): the same dict, in the same order, as `obj[k]
    /// = v` for each.
    fn process_layout_obj_with(&self, obj: &LayoutObj, k: &AttrKeys) -> PyResult<Obj> {
        let _frame = py_frame()?;
        // re.sub(lt_pat, "", obj.__class__.__name__).lower(): re.sub ->
        // re._compile (a compiled pattern: returned as it is)
        leaf(2)?;
        // def process_attr(item: Tuple[str, Any]) -> Optional[Tuple[str,
        // Any]]: its annotations, evaluated at each call
        PROCESS_ATTR_ITEM.eval()?;
        PROCESS_ATTR_RETURN.eval()?;
        let kind = object_type(obj.class_name());

        // dict(filter(None, map(process_attr, obj.__dict__.items()))): each
        // process_attr called from C
        let mut items: Vec<DictItem> = DICT_ITEMS.with(|v| std::mem::take(&mut *v.borrow_mut()));
        dict_items(&mut items, k, obj);
        // (room for what is added below: at most 9 keys, a curve's)
        let mut attr = Obj::with_capacity(items.len() + 9);
        for item in items.drain(..) {
            let _entry = eval_entry()?;
            let _process_attr = py_frame()?;
            if let Some((key, v)) = item {
                // k in ALL_ATTRS
                let res = resolve_all_owned(v)?;
                attr.push_new(key, res);
            }
        }
        // (the buffer is used again: an error above drops it)
        DICT_ITEMS.with(|v| *v.borrow_mut() = items);

        attr.set(&k.object_type, PyValue::Str(kind));
        attr.push_new(k.page_number.clone(), PyValue::Int(self.page_number.clone()));

        // Note: As of pdfminer.six v20221105, that library only exposes ncs
        // for LTChars, and neither attribute for other objects.
        if let LayoutObj::Item(LTItem::Char(c)) = obj {
            let name = PyValue::Str(colorspace_name(&c.ncs));
            attr.push_new(k.ncs.clone(), resolve_and_decode(&name)?);
        }

        let text = match obj {
            LayoutObj::Item(LTItem::Char(c)) => {
                leaf(1)?; // LTChar.get_text
                Some(c.get_text().clone())
            }
            LayoutObj::Item(LTItem::TextLine(l)) => Some(l.borrow().get_text()?),
            LayoutObj::Item(LTItem::TextBox(b)) => Some(b.borrow().get_text()?),
            _ => None,
        };
        if let Some(text) = text {
            let text = match &self.pdf.unicode_norm {
                Some(form) => normalize_unicode(form, &text)?,
                None => text,
            };
            attr.set(&k.text, PyValue::Str(text));
        }

        match obj {
            LayoutObj::Item(LTItem::Char(c)) => {
                // pdfminer.six (at least as of v20221105) does not directly
                // expose .stroking_color and .non_stroking_color for LTChar
                // objects (unlike, e.g., LTRect objects).
                let gs = &c.graphicstate;
                attr.push_new(k.stroking_color.clone(), color_tuple(&gs.scolor));
                attr.push_new(k.non_stroking_color.clone(), color_tuple(&gs.ncolor));
                // Handle (rare) byte-encoded fontnames
                if let PyValue::Bytes(b) = attr.item(&k.fontname)? {
                    let fixed = fix_fontname_bytes(b)?;
                    attr.set(&k.fontname, PyValue::Str(fixed));
                }
            }
            LayoutObj::Item(LTItem::Curve(c)) => {
                // list(map(self.point2coord, attr["pts"]))
                let pts = attr.item(&k.pts)?.clone();
                let pts = match crate::obj::py_iter(&pts) {
                    Some(items) => items?,
                    None => return Err(exc::TypeError(format!("'{}' object is not iterable", pts.type_name()))),
                };
                attr.set(&k.pts, PyValue::List(self.points2coords(&pts)?.into()));
                // [(cmd, *map(self.point2coord, pts)) for cmd, *pts in obj.original_path]
                let Some(original_path) = &c.original_path else {
                    return Err(exc::TypeError("'NoneType' object is not iterable"));
                };
                let mut path = Vec::with_capacity(original_path.len());
                for (cmd, pts) in original_path {
                    let mut items = Vec::with_capacity(pts.len() + 1);
                    items.push(PyValue::Str(path_command(*cmd)));
                    let pts: Vec<PyValue> = pts.iter().map(point_value).collect();
                    items.extend(self.points2coords(&pts)?);
                    path.push(PyValue::Tuple(items.into()));
                }
                attr.push_new(k.path.clone(), PyValue::List(path.into()));
                attr.push_new(
                    k.dash.clone(),
                    match &c.dashing_style {
                        None => PyValue::None,
                        Some((d, p)) => PyValue::tuple([PyValue::from_pdf(d.clone()), PyValue::from_pdf(p.clone())]),
                    },
                );
            }
            _ => {}
        }

        // As noted in #1181, `pdfminer.six` adjusts objects' coordinates
        // relative to the MediaBox.
        let mb_x0 = index(&self.mediabox, 0)?;
        let mb_top = index(&self.mediabox, 1)?;

        if attr.contains(&k.y0) {
            let top = PROCESS_OBJECT_HEIGHT_TOP.get(self.class_name(), || self.height())?;
            let top = top.sub(attr.hot(Hot::Y1)?)?.add(&mb_top)?;
            attr.set(&k.top, top);
            let bottom = PROCESS_OBJECT_HEIGHT_BOTTOM.get(self.class_name(), || self.height())?;
            let bottom = bottom.sub(attr.hot(Hot::Y0)?)?.add(&mb_top)?;
            attr.set(&k.bottom, bottom);
            let doctop = self.initial_doctop.add(attr.hot(Hot::Top)?)?;
            attr.set(&k.doctop, doctop);
        }

        if attr.contains(&k.x0) && !mb_x0.py_eq(&PyValue::from(0i64)) {
            let x0 = attr.hot(Hot::X0)?.add(&mb_x0)?;
            attr.set(&k.x0, x0);
            let x1 = attr.hot(Hot::X1)?.add(&mb_x0)?;
            attr.set(&k.x1, x1);
        }

        Ok(attr)
    }

    // pdfplumber/page.py:427 Page.iter_layout_objects
    /// The objects of `layout_objects` and, recursively, of their
    /// children (a container itself only with `laparams`), each through
    /// `process_object`, in the generator's order; `visit` gets each one
    /// (the generator's consumer, `parse_objects`, whose frame is the
    /// current one).
    pub fn iter_layout_objects(
        &self,
        layout_objects: &[LTItem<PlumberPayload>],
        visit: &mut dyn FnMut(Obj) -> PyResult<()>,
    ) -> PyResult<()> {
        leaf(1)?; // the generator is created
        let objs: Vec<LayoutObj> = layout_objects.iter().cloned().map(LayoutObj::Item).collect();
        self.iter_level(&objs, 0, visit)
    }

    /// The generator of level `level` (0: the one `parse_objects` made),
    /// resumed through the chain of `yield from` above it: `level + 1`
    /// generator frames on top of the consumer's while it runs.
    fn iter_level(&self, objs: &[LayoutObj], level: u32, visit: &mut dyn FnMut(Obj) -> PyResult<()>) -> PyResult<()> {
        for obj in objs {
            match obj.children() {
                // If object is, like LTFigure, a higher-level object ...
                Some(children) => {
                    // and LAParams is passed, process the object itself.
                    if self.pdf.laparams.is_some() {
                        let o = {
                            let _chain = py_frames(level + 1)?;
                            self.process_layout_obj(obj)?
                        };
                        visit(o)?;
                    }
                    // Regardless, iterate through its children: the
                    // generator `yield from` creates, one level down
                    leaf(level + 2)?;
                    self.iter_level(&children, level + 1, visit)?;
                }
                None => {
                    let o = {
                        let _chain = py_frames(level + 1)?;
                        self.process_layout_obj(obj)?
                    };
                    visit(o)?;
                }
            }
        }
        // the last resumption, which ends the generator
        leaf(level + 1)
    }

    // pdfplumber/page.py:441 Page.parse_objects
    /// The objects of the layout by kind (`anno` left out).
    pub fn parse_objects(&self) -> PyResult<Objects> {
        let _frame = py_frame()?;
        let layout = PARSE_OBJECTS_LAYOUT.get(self.class_name(), || self.layout())?;
        let mut objects = Objects::new();
        self.iter_layout_objects(&layout.layout._objs, &mut |obj| {
            let kind = obj.item("object_type")?.clone();
            let PyValue::Str(kind) = &kind else { unreachable!("process_object sets a str") };
            if kind != "anno" {
                objects.push(kind.clone(), obj);
            }
            Ok(())
        })?;
        Ok(objects)
    }

    // pdfplumber/page.py:452 Page.debug_tablefinder
    pub fn debug_tablefinder(&self, table_settings: &PyValue) -> PyResult<TableFinder> {
        let _frame = py_frame()?;
        let tset = TableSettings::resolve_value(table_settings)?;
        // TableFinder(self, tset): the class call, __init__
        let _call = class_call()?;
        let _init = py_frame()?;
        TableFinder::new(self, tset)
    }

    // pdfplumber/page.py:458 Page.find_tables
    pub fn find_tables(&self, table_settings: &PyValue) -> PyResult<Vec<Table>> {
        let _frame = py_frame()?;
        let tset = TableSettings::resolve_value(table_settings)?;
        self.tables_of(tset)
    }

    /// `TableFinder(self, tset).tables`
    fn tables_of(&self, tset: TableSettings) -> PyResult<Vec<Table>> {
        let _call = class_call()?;
        let _init = py_frame()?;
        Ok(TableFinder::new(self, tset)?.tables)
    }

    /// `self.find_tables(tset)` for an already resolved `tset`
    /// (`TableSettings.resolve` returns it as it is).
    fn find_tables_resolved(&self, tset: TableSettings) -> PyResult<Vec<Table>> {
        let _frame = py_frame()?;
        leaf(1)?; // TableSettings.resolve(tset)
        self.tables_of(tset)
    }

    // pdfplumber/page.py:464 Page.find_table
    /// The largest table (most cells, then top, then left: `sorter`,
    /// nested), or `None`.
    pub fn find_table(&self, table_settings: &PyValue) -> PyResult<Option<Table>> {
        let _frame = py_frame()?;
        let tset = TableSettings::resolve_value(table_settings)?;
        self.find_table_resolved(tset)
    }

    fn find_table_resolved(&self, tset: TableSettings) -> PyResult<Option<Table>> {
        let mut tables = self.find_tables_resolved(tset)?;
        if tables.is_empty() {
            return Ok(None);
        }
        // Return the largest table, as measured by number of cells.
        // def sorter(x: Table) -> Tuple[int, T_num, T_num]: its annotation
        SORTER_RETURN.eval()?;
        let mut keys = Vec::with_capacity(tables.len());
        for t in &tables {
            // sorter(x) = (-len(x.cells), x.bbox[1], x.bbox[0]), called from C
            let _entry = eval_entry()?;
            let _sorter = py_frame()?;
            // x.bbox[1], x.bbox[0]: Table.bbox, two loads
            SORTER_BBOX_1.get("Table", || leaf(1))?;
            let bbox = t.bbox()?;
            SORTER_BBOX_0.get("Table", || leaf(1))?;
            keys.push(PyValue::tuple([PyValue::from(-(t.cells.len() as i64)), bbox[1].clone(), bbox[0].clone()]));
        }
        let mut indexed: Vec<usize> = (0..tables.len()).collect();
        pycompat::listsort::sort_by_key(&mut indexed, |&i| Ok(keys[i].clone()), |a, b| a.lt(b))?;
        Ok(Some(tables.swap_remove(indexed[0])))
    }

    // pdfplumber/page.py:481 Page.extract_tables
    pub fn extract_tables(&self, table_settings: &PyValue) -> PyResult<Vec<Vec<Vec<Option<PyStr>>>>> {
        let _frame = py_frame()?;
        let tset = TableSettings::resolve_value(table_settings)?;
        let kwargs = tset.text_settings_kwargs("pdfplumber.table.Table.extract");
        let tables = self.find_tables_resolved(tset)?;
        let mut out = Vec::with_capacity(tables.len());
        for table in &tables {
            out.push(table.extract(self, &kwargs.clone()?)?);
        }
        Ok(out)
    }

    // pdfplumber/page.py:488 Page.extract_table
    pub fn extract_table(&self, table_settings: &PyValue) -> PyResult<Option<Vec<Vec<Option<PyStr>>>>> {
        let _frame = py_frame()?;
        let tset = TableSettings::resolve_value(table_settings)?;
        let kwargs = tset.text_settings_kwargs("pdfplumber.table.Table.extract");
        let table = {
            let _frame = py_frame()?;
            leaf(1)?; // TableSettings.resolve(tset)
            self.find_table_resolved(tset)?
        };
        match table {
            None => Ok(None),
            Some(table) => Ok(Some(table.extract(self, &kwargs?)?)),
        }
    }

    // pdfplumber/page.py:498 Page._get_textmap
    /// `get_textmap(**kwargs)`: the cached text map of the chars (the
    /// `lru_cache` of `__init__`: by the keyword arguments in the order
    /// given, `TypeError` for one that is not hashable, at most 128 maps,
    /// the least recently used dropped first), `layout_bbox`,
    /// `layout_width`, `layout_height` defaulting to the page's.
    pub fn get_textmap(&self, kwargs: &Kwargs) -> PyResult<Rc<TextMap>> {
        // the lru_cache wrapper, a C object called by Python code
        let _call = c_call(WHILE_CALLING)?;
        let mut key: TextMapKey = Vec::with_capacity(kwargs.len());
        for (k, v) in kwargs.items() {
            key.push((k.clone(), HashKey::new(v.clone())?));
        }
        {
            let mut cache = self.cache.borrow_mut();
            let entries = &mut cache.textmaps.entries;
            if let Some(i) = entries.iter().position(|(k, _)| *k == key) {
                let hit = entries.remove(i);
                let textmap = hit.1.clone();
                entries.push(hit);
                return Ok(textmap);
            }
        }
        // a miss: self._get_textmap(**kwargs), called from C
        let textmap = {
            let _entry = eval_entry()?;
            Rc::new(self._get_textmap(kwargs)?)
        };
        let mut cache = self.cache.borrow_mut();
        let entries = &mut cache.textmaps.entries;
        if entries.len() >= LRU_MAXSIZE {
            entries.remove(0);
        }
        entries.push((key, textmap.clone()));
        Ok(textmap)
    }

    // pdfplumber/page.py:498 Page._get_textmap
    fn _get_textmap(&self, kwargs: &Kwargs) -> PyResult<TextMap> {
        let _frame = py_frame()?;
        let mut full_kwargs = Kwargs::new();
        full_kwargs.set("layout_bbox", self.bbox.clone());
        if !kwargs.contains("layout_width_chars") {
            full_kwargs.set("layout_width", GET_TEXTMAP_WIDTH.get(self.class_name(), || self.width())?);
        }
        if !kwargs.contains("layout_height_chars") {
            full_kwargs.set("layout_height", GET_TEXTMAP_HEIGHT.get(self.class_name(), || self.height())?);
        }
        // {**defaults, **kwargs}
        for (k, v) in kwargs.items() {
            full_kwargs.set(k, v.clone());
        }
        let chars = GET_TEXTMAP_CHARS.get(self.class_name(), || Container::chars(self))?;
        leaf(1)?; // utils.chars_to_textmap
        chars_to_textmap(&chars, &full_kwargs)
    }

    // pdfplumber/page.py:509 Page.search
    /// `page.search(pattern, regex, case, main_group, return_chars,
    /// return_groups, **kwargs)`: the matches of `pattern` are the
    /// caller's (`matches` gets `as_string` of the text map and returns
    /// the spans of each match, as Python's `re.finditer` of
    /// `re.compile(pattern if regex else re.escape(pattern), re.I if case
    /// is False else 0)` would: `docs/architecture.md`, "Frontière
    /// JavaScript").
    pub fn search(
        &self,
        matches: &mut dyn FnMut(&PyStr) -> PyResult<Vec<RegexMatch>>,
        args: SearchArgs,
        kwargs: &Kwargs,
    ) -> PyResult<Vec<Obj>> {
        let main_group = args.main_group;
        self.search_with(matches, args.return_groups, args.return_chars, &|_, _| Ok(main_group), kwargs)
    }

    // pdfplumber/page.py:509 Page.search
    /// [`Page::search`] whose `main_group` is any value `m.group()` takes
    /// (`TextMap::search_with`): the JavaScript boundary resolves it for
    /// each match.
    pub fn search_with(
        &self,
        matches: &mut dyn FnMut(&PyStr) -> PyResult<Vec<RegexMatch>>,
        return_groups: bool,
        return_chars: bool,
        main_group: &dyn Fn(usize, &RegexMatch) -> PyResult<usize>,
        kwargs: &Kwargs,
    ) -> PyResult<Vec<Obj>> {
        bind(SEARCH, kwargs)?;
        let _frame = py_frame()?;
        let textmap = self.get_textmap(&tuplify_list_kwargs(kwargs)?)?;
        // TextMap.search
        let _search = py_frame()?;
        let found = matches(&textmap.as_string)?;
        textmap.search_with(&found, return_groups, return_chars, main_group)
    }

    // pdfplumber/page.py:509 Page.search
    /// The text `page.search(pattern, **kwargs)` runs its pattern over:
    /// `self.get_textmap(**tuplify_list_kwargs(kwargs)).as_string`, in the
    /// frame of `search`. The JavaScript boundary searches in two calls
    /// (`docs/architecture.md`, "Frontière JavaScript"): this text, which
    /// JavaScript's `RegExp` runs over, then [`Page::search_with`] with its
    /// matches, which finds the text map in the cache this call filled; so
    /// the text map is computed (the page interpreted, if it was not) at
    /// the depth Python's `search` computes it.
    pub fn search_text(&self, kwargs: &Kwargs) -> PyResult<PyStr> {
        bind(SEARCH, kwargs)?;
        let _frame = py_frame()?;
        Ok(self.get_textmap(&tuplify_list_kwargs(kwargs)?)?.as_string.clone())
    }

    // pdfplumber/page.py:529 Page.extract_text
    pub fn extract_text(&self, kwargs: &Kwargs) -> PyResult<PyStr> {
        bind(method("Page.extract_text"), kwargs)?;
        let _frame = py_frame()?;
        Ok(self.get_textmap(&tuplify_list_kwargs(kwargs)?)?.as_string.clone())
    }

    // pdfplumber/page.py:532 Page.extract_text_simple
    /// `utils.extract_text_simple(self.chars, **kwargs)`: its keyword
    /// arguments are `x_tolerance` and `y_tolerance` (3 by default).
    pub fn extract_text_simple(&self, kwargs: &Kwargs) -> PyResult<PyStr> {
        bind(method("Page.extract_text_simple"), kwargs)?;
        let _frame = py_frame()?;
        let chars = EXTRACT_TEXT_SIMPLE_CHARS.get(self.class_name(), || Container::chars(self))?;
        let values = bind_kwargs("extract_text_simple", kwargs, &["x_tolerance", "y_tolerance"])?;
        let three = PyValue::from(3i64);
        leaf(1)?;
        extract_text_simple(&chars, values[0].as_ref().unwrap_or(&three), values[1].as_ref().unwrap_or(&three))
    }

    // pdfplumber/page.py:535 Page.extract_words
    pub fn extract_words(&self, kwargs: &Kwargs) -> PyResult<Vec<Obj>> {
        bind(method("Page.extract_words"), kwargs)?;
        let _frame = py_frame()?;
        let chars = EXTRACT_WORDS_CHARS.get(self.class_name(), || Container::chars(self))?;
        leaf(1)?;
        extract_words_kwargs(&chars, kwargs)
    }

    // pdfplumber/page.py:538 Page.extract_text_lines
    pub fn extract_text_lines(&self, strip: bool, return_chars: bool, kwargs: &Kwargs) -> PyResult<Vec<Obj>> {
        bind(EXTRACT_TEXT_LINES, kwargs)?;
        let _frame = py_frame()?;
        let textmap = self.get_textmap(&tuplify_list_kwargs(kwargs)?)?;
        leaf(1)?; // TextMap.extract_text_lines
        textmap.extract_text_lines(strip, return_chars)
    }

    // pdfplumber/page.py:545 Page.crop
    // pdfplumber/page.py:550 Page.within_bbox
    // pdfplumber/page.py:560 Page.outside_bbox
    /// `page.crop(bbox, relative, strict)` (`CropFn::CropToBbox`),
    /// `within_bbox` (`WithinBbox`), `outside_bbox` (`OutsideBbox`): a
    /// `CroppedPage` (pdfplumber/page.py:678 `CroppedPage.__init__`, then
    /// :643 `DerivedPage.__init__`). `bbox` as given (any value: it is
    /// unpacked, tested, printed).
    pub fn crop(self: &Rc<Self>, bbox: PyValue, crop_fn: CropFn, relative: bool, strict: bool) -> PyResult<Rc<Page>> {
        let _frame = py_frame()?;
        // CroppedPage(self, bbox, ...): the class call, __init__
        let _call = class_call()?;
        let _init = py_frame()?;
        let mut crop_bbox = bbox;
        if relative {
            let [o_x0, o_top, _, _] = bbox_from_value(&self.bbox)?;
            let [x0, top, x1, bottom] = bbox_from_value(&crop_bbox)?;
            crop_bbox = PyValue::tuple([x0.add(&o_x0)?, top.add(&o_top)?, x1.add(&o_x0)?, bottom.add(&o_top)?]);
        }
        if strict {
            test_proposed_bbox(&crop_bbox, &self.bbox)?;
        }
        // Note: testing for original function passed, not _crop_fn
        let bbox = if crop_fn == CropFn::OutsideBbox { self.bbox.clone() } else { crop_bbox.clone() };
        let kind = PageKind::Cropped { parent: self.clone(), crop_bbox, crop_fn };
        Ok(Rc::new(Page::derived(self, kind, bbox)?))
    }

    // pdfplumber/page.py:570 Page.filter
    /// A `FilteredPage` (pdfplumber/page.py:718 `FilteredPage.__init__`).
    pub fn filter(self: &Rc<Self>, test_function: FilterFn) -> PyResult<Rc<Page>> {
        let _frame = py_frame()?;
        self.filtered(test_function)
    }

    // pdfplumber/page.py:718 FilteredPage.__init__
    fn filtered(self: &Rc<Self>, filter_fn: FilterFn) -> PyResult<Rc<Page>> {
        let _call = class_call()?;
        let _init = py_frame()?;
        let kind = PageKind::Filtered { parent: self.clone(), filter_fn };
        Ok(Rc::new(Page::derived(self, kind, self.bbox.clone())?))
    }

    // pdfplumber/page.py:573 Page.dedupe_chars
    /// A `FilteredPage` whose objects are this page's with the chars of
    /// `utils.dedupe_chars(self.chars, **kwargs)`, computed now.
    pub fn dedupe_chars(self: &Rc<Self>, kwargs: &Kwargs) -> PyResult<Rc<Page>> {
        bind(method("Page.dedupe_chars"), kwargs)?;
        let _frame = py_frame()?;
        // FilteredPage(self, lambda x: True)
        let p = self.filtered(Rc::new(|_: &Obj| Ok(PyValue::Bool(true))))?;
        let mut objects = Objects::new();
        for (kind, objs) in DEDUPE_CHARS_OBJECTS.get(self.class_name(), || self.objects())?.iter() {
            objects.insert(kind.clone(), objs.to_vec());
        }
        let chars = DEDUPE_CHARS_CHARS.get(self.class_name(), || Container::chars(&**self))?;
        let values = bind_kwargs("dedupe_chars", kwargs, &["tolerance", "extra_attrs"])?;
        let tolerance = values[0].clone().unwrap_or(PyValue::from(1i64));
        let extra_attrs =
            values[1].clone().unwrap_or_else(|| PyValue::tuple([PyValue::str("fontname"), PyValue::str("size")]));
        leaf(1)?; // utils.dedupe_chars
        objects.insert(PyStr::new("char"), dedupe_chars(&chars, &tolerance, &extra_attrs)?);
        p.cache.borrow_mut().objects = Some(Rc::new(objects));
        Ok(p)
    }

    // pdfplumber/page.py:636 Page.__repr__
    /// `<Page:n>` (every kind of page).
    pub fn repr(&self) -> String {
        self.value.repr()
    }
}

/// `self._crop_fn(objs)`: `crop_fn(objs, crop_bbox)` (`_crop_fn`, nested
/// in `CroppedPage.__init__`, and the function of `utils`: a frame each).
/// The box is unpacked by the function for the first object (an empty list
/// never looks at it).
pub(crate) fn apply_crop_fn(crop_fn: CropFn, crop_bbox: &PyValue, objs: &[Obj]) -> PyResult<Vec<Obj>> {
    let _crop_fn = py_frame()?;
    let _utils = py_frame()?;
    if objs.is_empty() {
        return Ok(Vec::new());
    }
    let bbox = bbox_from_value(crop_bbox)?;
    match crop_fn {
        CropFn::CropToBbox => crop_to_bbox(objs, &bbox),
        CropFn::WithinBbox => within_bbox(objs, &bbox),
        CropFn::OutsideBbox => outside_bbox(objs, &bbox),
    }
}

/// The values of the keyword parameters `names` of `func(chars, **kwargs)`
/// (`None` when not given), with the `TypeError`s of a keyword that is
/// `chars` or none of `names`, for the first such key in order.
fn bind_kwargs(func: &str, kwargs: &Kwargs, names: &[&str]) -> PyResult<Vec<Option<PyValue>>> {
    Signature { qualname: func, bound: &["chars"], keywords: Some(names) }
        .bind(kwargs.items().iter().map(|(k, _)| k))?;
    Ok(names.iter().map(|n| kwargs.get(n).cloned()).collect())
}

/// The signature of a method `Class.name(self, **kwargs)`.
const fn method(qualname: &str) -> Signature<'_> {
    Signature { qualname, bound: &["self"], keywords: None }
}

// pdfplumber/page.py:509 Page.search: its named parameters, which the
// port takes apart from the keyword arguments
const SEARCH: Signature<'static> = Signature {
    qualname: "Page.search",
    bound: &["self", "pattern", "regex", "case", "main_group", "return_chars", "return_groups"],
    keywords: None,
};

// pdfplumber/page.py:538 Page.extract_text_lines
const EXTRACT_TEXT_LINES: Signature<'static> =
    Signature { qualname: "Page.extract_text_lines", bound: &["self", "strip", "return_chars"], keywords: None };

/// Binds the keyword arguments of a call (`**kwargs`) to `signature`
/// (`pycompat::call`): a name of a parameter the call bound already
/// (`self`...) is a `TypeError` before the method runs.
fn bind(signature: Signature<'_>, kwargs: &Kwargs) -> PyResult<()> {
    signature.bind(kwargs.items().iter().map(|(k, _)| k))
}

/// `normalize_unicode(form, text)` (`unicodedata.normalize`): `TypeError`
/// for a form that is not a `str`, `ValueError` for a `str` that is not a
/// form (not even checked for an empty text).
fn normalize_unicode(form: &PyValue, text: &PyStr) -> PyResult<PyStr> {
    let PyValue::Str(form) = form else {
        return Err(exc::TypeError(format!("normalize() argument 1 must be str, not {}", form.type_name())));
    };
    match form.as_str() {
        Some(f) => pycompat::unicodedata::normalize_named(f, text),
        None if text.is_empty() => Ok(text.clone()),
        None => Err(exc::ValueError("invalid normalization form")),
    }
}

impl Container for Page {
    fn pages(&self) -> PyResult<Option<Vec<Rc<Page>>>> {
        Ok(None)
    }

    fn objects(&self) -> PyResult<Rc<Objects>> {
        Page::objects(self)
    }

    // pdfplumber/page.py:617 Page.to_dict
    fn to_dict(&self, object_types: &PyValue) -> PyResult<Obj> {
        let _frame = py_frame()?;
        let types = if object_types.is_none() {
            let objects = TO_DICT_OBJECTS.get(self.class_name(), || Page::objects(self))?;
            PyValue::list(objects.keys().cloned().map(PyValue::Str).chain([PyValue::str("annot")]))
        } else {
            object_types.clone()
        };
        let mut d = Obj::from_pairs([
            ("page_number", PyValue::Int(self.page_number.clone())),
            ("initial_doctop", self.initial_doctop.clone()),
            ("rotation", self.rotation.clone()),
            ("cropbox", self.cropbox.clone()),
            ("mediabox", self.mediabox.clone()),
            ("bbox", self.bbox.clone()),
        ]);
        d.set("width", TO_DICT_WIDTH.get(self.class_name(), || self.width())?);
        d.set("height", TO_DICT_HEIGHT.get(self.class_name(), || self.height())?);
        // for t in _object_types: d[t + "s"] = getattr(self, t + "s")
        for t in &crate::container::object_types_iter(&types)? {
            let name = crate::container::object_type_name(t)?;
            let value = crate::container::getattr(self, &name, &TO_DICT_GETATTR)?;
            d.set(&name, value);
        }
        Ok(d)
    }

    fn container_cache(&self) -> &RefCell<ContainerCache> {
        &self.container
    }

    fn class_name(&self) -> &'static str {
        Page::class_name(self)
    }

    /// `getattr(page, name)`: the lists and edges of `Container`,
    /// `annots`, `hyperlinks`, `pages` (`None`), `objects`, the cached
    /// properties when set (`_objects`...), `cached_properties`; methods
    /// are `NotPorted`, any other name `AttributeError`.
    fn attribute(&self, name: &PyStr) -> PyResult<PyValue> {
        match name.as_str() {
            Some("annots") => Ok(crate::container::obj_list(self.annots()?)),
            Some("hyperlinks") => Ok(crate::container::obj_list(self.hyperlinks()?)),
            Some("pages") => Ok(PyValue::None),
            Some("_objects") => match &self.cache.borrow().objects {
                Some(objects) => Ok(crate::container::objects_value(objects)),
                None => Err(crate::container::no_attribute(self.class_name(), name)),
            },
            Some("cached_properties") => Ok(PyValue::list(
                ["_rect_edges", "_curve_edges", "_edges", "_objects", "_layout"].iter().map(|p| PyValue::str(p)),
            )),
            _ => crate::container::container_attribute(self, self.class_name(), name),
        }
    }

    fn flush_cache(&self, properties: Option<&[&str]>) {
        // cached_properties = Container.cached_properties + ["_layout"]
        let props = properties.unwrap_or(&["_rect_edges", "_curve_edges", "_edges", "_objects", "_layout"]);
        crate::container::flush_container_cache(&self.container, props);
        let mut cache = self.cache.borrow_mut();
        for p in props {
            match *p {
                "_objects" => cache.objects = None,
                "_layout" => cache.layout = None,
                _ => {}
            }
        }
    }
}

impl TablePage for Page {
    fn property_class(&self) -> Option<&'static str> {
        Some(Page::class_name(self))
    }

    fn chars(&self) -> PyResult<Vec<Obj>> {
        Container::chars(self)
    }

    fn edges(&self) -> PyResult<Vec<Obj>> {
        Container::edges(self).map(|e| e.to_vec())
    }

    fn bbox(&self) -> PyResult<Bbox> {
        bbox_from_value(&self.bbox)
    }

    fn extract_words(&self, kwargs: &Kwargs) -> PyResult<Vec<Obj>> {
        Page::extract_words(self, kwargs)
    }
}
