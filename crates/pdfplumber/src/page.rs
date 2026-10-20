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
use pdfminer::layout::{LAParams, LTItem, LTPage, LTPayload};
use pdfminer::pdfcolor::PDFColorSpace;
use pdfminer::pdfdevice::{text_device, ObjSeq, PDFDevice, PDFDeviceAttrs, PDFTextDevice, PDFTextSeq};
use pdfminer::pdffont::{FontName, PDFFont};
use pdfminer::pdfinterp::{Color, PDFGraphicState, PDFPageInterpreter, PDFResourceManager, PDFStackT, PDFTextState};
use pdfminer::pdfpage::PDFPage;
use pdfminer::pdftypes::{PDFObject, PDFStream};
use pdfminer::psparser::{LiteralName, PSLiteral};
use pdfminer::utils::{Matrix, PathSegment};
use pycompat::bytes_format::bytes_mod_int;
use pycompat::recursion::{eval_entry, py_frame};
use pycompat::{exc, ExcKind, Num, PyErr, PyInt, PyResult, PyStr};

use crate::container::{Container, ContainerCache, Objects};
use crate::obj::{bbox_from_value, Bbox, HashKey, Obj, PageRef, PyValue};
use crate::pdf::PdfCore;
use crate::table::{Table, TableFinder, TablePage};
use crate::utils::pdfinternals::{decode_text, resolve_all, TextArg};
use crate::utils::text::{Kwargs, RegexMatch, TextMap};

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
// `is_original` is `matches!(kind, PageKind::Original)`.
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

// pdfplumber/page.py:83 CP936_FONTNAMES
pub const CP936_FONTNAMES: [(&[u8], &str); 5] = [
    (b"\xcb\xce\xcc\xe5", "SimSun,Regular"),
    (b"\xba\xda\xcc\xe5", "SimHei,Regular"),
    (b"\xbf\xac\xcc\xe5_GB2312", "SimKai,Regular"),
    (b"\xb7\xc2\xcb\xce_GB2312", "SimFang,Regular"),
    (b"\xc1\xa5\xca\xe9", "SimLi,Regular"),
];

// pdfplumber/page.py:92 fix_fontname_bytes
pub fn fix_fontname_bytes(fontname: &[u8]) -> PyResult<PyStr> {
    let _ = fontname;
    todo!("pdfplumber/page.py:92 fix_fontname_bytes")
}

// pdfplumber/page.py:103 tuplify_list_kwargs
/// A `list` value becomes a `tuple` (top level only).
pub fn tuplify_list_kwargs(kwargs: &Kwargs) -> PyResult<Kwargs> {
    let _ = kwargs;
    todo!("pdfplumber/page.py:103 tuplify_list_kwargs")
}

// pdfplumber/page.py:657 test_proposed_bbox
/// `ValueError` for a box of zero area, outside the parent's box, or not
/// fully within it (each message prints both boxes as given).
pub fn test_proposed_bbox(bbox: &PyValue, parent_bbox: &PyValue) -> PyResult<()> {
    let _ = (bbox, parent_bbox);
    todo!("pdfplumber/page.py:657 test_proposed_bbox")
}

/// The keyword arguments of `Page.search` besides the text map's.
#[derive(Clone, Copy, Debug)]
pub struct SearchArgs {
    pub main_group: usize,
    pub return_chars: bool,
    pub return_groups: bool,
}

impl Page {
    // pdfplumber/page.py:191 Page.__init__
    /// `Page(pdf, page_obj, page_number=page_number,
    /// initial_doctop=initial_doctop)`: the class call, `__init__`.
    pub fn new(pdf: Rc<PdfCore>, page_obj: Rc<PDFPage>, page_number: PyInt, initial_doctop: PyValue) -> PyResult<Page> {
        let _ = (pdf, page_obj, page_number, initial_doctop);
        todo!("pdfplumber/page.py:191 Page.__init__ (PageBoxes::new)")
    }

    /// This page as a value (the same object each time).
    pub fn as_value(&self) -> PageRef {
        self.value.clone()
    }

    // pdfplumber/page.py:235 Page.close
    pub fn close(&self) {
        todo!("pdfplumber/page.py:235 Page.close")
    }

    // pdfplumber/page.py:240 Page.width
    pub fn width(&self) -> PyResult<PyValue> {
        todo!("pdfplumber/page.py:240 Page.width")
    }

    // pdfplumber/page.py:244 Page.height
    pub fn height(&self) -> PyResult<PyValue> {
        todo!("pdfplumber/page.py:244 Page.height")
    }

    // pdfplumber/page.py:248 Page.structure_tree
    /// Workstream DOC (`crate::structure`): the `to_dict()` of each element
    /// of `PDFStructTree(self.pdf, self)`, `[]` without a structure tree.
    pub fn structure_tree(self: &Rc<Self>) -> PyResult<Vec<PyValue>> {
        todo!("pdfplumber/page.py:248 Page.structure_tree")
    }

    // pdfplumber/page.py:256 Page.layout
    /// The page's `LTPage` (analyzed when the `PDF` has `laparams`, with
    /// the `IdSource` of [`PdfCore::ids`]), cached; `PdfminerException(e)`
    /// when the interpretation raises (nothing cached). See [`page_layout`].
    pub fn layout(&self) -> PyResult<Rc<LTPage<PlumberPayload>>> {
        todo!("pdfplumber/page.py:256 Page.layout")
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
        todo!("pdfplumber/page.py:273 Page.annots")
    }

    // pdfplumber/page.py:339 Page.hyperlinks
    /// Workstream DOC: the annotations whose `uri` is not `None`
    /// (`self.annots` computed again).
    pub fn hyperlinks(&self) -> PyResult<Vec<Obj>> {
        todo!("pdfplumber/page.py:339 Page.hyperlinks")
    }

    // pdfplumber/page.py:343 Page.objects
    // pdfplumber/page.py:708 CroppedPage.objects
    // pdfplumber/page.py:724 FilteredPage.objects
    /// The objects by kind, cached: `parse_objects` for an original page,
    /// the parent's through `_crop_fn` or `filter_fn` for a derived one.
    pub fn objects(&self) -> PyResult<Rc<Objects>> {
        todo!("pdfplumber/page.py:343 Page.objects")
    }

    // pdfplumber/page.py:349 Page.point2coord
    pub fn point2coord(&self, pt: &PyValue) -> PyResult<PyValue> {
        let _ = pt;
        todo!("pdfplumber/page.py:349 Page.point2coord")
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
    pub fn process_object(&self, obj: &LTItem<PlumberPayload>) -> PyResult<Obj> {
        let _ = obj;
        todo!("pdfplumber/page.py:353 Page.process_object")
    }

    // pdfplumber/page.py:427 Page.iter_layout_objects
    /// The objects of `layout_objects` and, recursively, of their
    /// children (a container itself only with `laparams`), each through
    /// `process_object`, in the generator's order; `visit` gets each one
    /// (the generator's consumer, `parse_objects`).
    pub fn iter_layout_objects(
        &self,
        layout_objects: &[LTItem<PlumberPayload>],
        visit: &mut dyn FnMut(Obj) -> PyResult<()>,
    ) -> PyResult<()> {
        let _ = (layout_objects, visit);
        todo!("pdfplumber/page.py:427 Page.iter_layout_objects")
    }

    // pdfplumber/page.py:441 Page.parse_objects
    /// The objects of the layout by kind (`anno` left out).
    pub fn parse_objects(&self) -> PyResult<Objects> {
        todo!("pdfplumber/page.py:441 Page.parse_objects")
    }

    // pdfplumber/page.py:452 Page.debug_tablefinder
    pub fn debug_tablefinder(&self, table_settings: Option<&Kwargs>) -> PyResult<TableFinder> {
        let _ = table_settings;
        todo!("pdfplumber/page.py:452 Page.debug_tablefinder")
    }

    // pdfplumber/page.py:458 Page.find_tables
    pub fn find_tables(&self, table_settings: Option<&Kwargs>) -> PyResult<Vec<Table>> {
        let _ = table_settings;
        todo!("pdfplumber/page.py:458 Page.find_tables")
    }

    // pdfplumber/page.py:464 Page.find_table
    /// The largest table (most cells, then top, then left: `sorter`,
    /// nested), or `None`.
    pub fn find_table(&self, table_settings: Option<&Kwargs>) -> PyResult<Option<Table>> {
        let _ = table_settings;
        todo!("pdfplumber/page.py:464 Page.find_table")
    }

    // pdfplumber/page.py:481 Page.extract_tables
    pub fn extract_tables(&self, table_settings: Option<&Kwargs>) -> PyResult<Vec<Vec<Vec<Option<PyStr>>>>> {
        let _ = table_settings;
        todo!("pdfplumber/page.py:481 Page.extract_tables")
    }

    // pdfplumber/page.py:488 Page.extract_table
    pub fn extract_table(&self, table_settings: Option<&Kwargs>) -> PyResult<Option<Vec<Vec<Option<PyStr>>>>> {
        let _ = table_settings;
        todo!("pdfplumber/page.py:488 Page.extract_table")
    }

    // pdfplumber/page.py:498 Page._get_textmap
    /// `get_textmap(**kwargs)`: the cached text map of the chars
    /// (`TextMapCache`), `layout_bbox`, `layout_width`, `layout_height`
    /// defaulting to the page's.
    pub fn get_textmap(&self, kwargs: &Kwargs) -> PyResult<Rc<TextMap>> {
        let _ = kwargs;
        todo!("pdfplumber/page.py:498 Page._get_textmap (and its lru_cache)")
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
        let _ = (matches, args, kwargs);
        todo!("pdfplumber/page.py:509 Page.search")
    }

    // pdfplumber/page.py:529 Page.extract_text
    pub fn extract_text(&self, kwargs: &Kwargs) -> PyResult<PyStr> {
        let _ = kwargs;
        todo!("pdfplumber/page.py:529 Page.extract_text")
    }

    // pdfplumber/page.py:532 Page.extract_text_simple
    pub fn extract_text_simple(&self, kwargs: &Kwargs) -> PyResult<PyStr> {
        let _ = kwargs;
        todo!("pdfplumber/page.py:532 Page.extract_text_simple")
    }

    // pdfplumber/page.py:535 Page.extract_words
    pub fn extract_words(&self, kwargs: &Kwargs) -> PyResult<Vec<Obj>> {
        let _ = kwargs;
        todo!("pdfplumber/page.py:535 Page.extract_words")
    }

    // pdfplumber/page.py:538 Page.extract_text_lines
    pub fn extract_text_lines(&self, strip: bool, return_chars: bool, kwargs: &Kwargs) -> PyResult<Vec<Obj>> {
        let _ = (strip, return_chars, kwargs);
        todo!("pdfplumber/page.py:538 Page.extract_text_lines")
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
        let _ = (bbox, crop_fn, relative, strict);
        todo!("pdfplumber/page.py:545 Page.crop, :678 CroppedPage.__init__")
    }

    // pdfplumber/page.py:570 Page.filter
    /// A `FilteredPage` (pdfplumber/page.py:718 `FilteredPage.__init__`).
    pub fn filter(self: &Rc<Self>, test_function: FilterFn) -> PyResult<Rc<Page>> {
        let _ = test_function;
        todo!("pdfplumber/page.py:570 Page.filter, :718 FilteredPage.__init__")
    }

    // pdfplumber/page.py:573 Page.dedupe_chars
    /// A `FilteredPage` whose objects are this page's with the chars of
    /// `utils.dedupe_chars(self.chars, **kwargs)`, computed now.
    pub fn dedupe_chars(self: &Rc<Self>, kwargs: &Kwargs) -> PyResult<Rc<Page>> {
        let _ = kwargs;
        todo!("pdfplumber/page.py:573 Page.dedupe_chars")
    }

    // pdfplumber/page.py:636 Page.__repr__
    /// `<Page:n>` (every kind of page).
    pub fn repr(&self) -> String {
        self.value.repr()
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
    fn to_dict(&self, object_types: Option<&[PyStr]>) -> PyResult<Obj> {
        let _ = object_types;
        todo!("pdfplumber/page.py:617 Page.to_dict")
    }

    fn container_cache(&self) -> &RefCell<ContainerCache> {
        &self.container
    }

    fn attribute(&self, name: &PyStr) -> PyResult<PyValue> {
        let _ = name;
        todo!("getattr(page, name) for pdfplumber/page.py:617 Page.to_dict")
    }

    fn flush_cache(&self, properties: Option<&[&str]>) {
        let _ = (properties, &self.cache);
        todo!("pdfplumber/container.py:28 Container.flush_cache (Page: + _layout)")
    }
}

impl TablePage for Page {
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
