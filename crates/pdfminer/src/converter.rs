//! Port of `pdfminer/converter.py` (partial): the devices that build the
//! layout objects, `PDFLayoutAnalyzer` and `PDFPageAggregator`.
//!
//! **Wave 3 skeleton** (signatures and state, bodies `todo!()`, workstream
//! CONVERTER, `docs/architecture.md`, "Vague 3").
//!
//! Not ported (out of scope: pdfplumber never uses them, see
//! `docs/architecture.md`, "Règles de port"): `PDFConverter`,
//! `TextConverter`, `HTMLConverter`, `XMLConverter`, `HOCRConverter` (text,
//! HTML, XML and hOCR outputs, with `ImageWriter`).
//!
//! # Classes
//!
//! As in `pdfdevice` (see "Classes, overrides and `super()`" there): the
//! attributes of `PDFLayoutAnalyzer` are [`PDFLayoutAnalyzer`], reached
//! through [`LayoutAnalyzer::analyzer`]; its methods' bodies are the free
//! functions of [`layout_analyzer`], generic over any [`LayoutAnalyzer`],
//! so that a subclass (`PDFPageAggregator`, pdfplumber's
//! `PDFPageAggregatorWithMarkedContent`, a tracing device of `pdftrace`)
//! reuses them as `super()` and receives the calls they make on `self`
//! (`paint_path` for each subpath, `receive_layout` from `end_page`).
//! [`PDFPageAggregator`] is the concrete device; its result is
//! [`page_aggregator::get_result`].
//!
//! # Layout objects
//!
//! The objects are `layout`'s, with a payload ([`InterpPayload`]) whose
//! types are the interpreter's: the font name of `pdffont`, the
//! `Rc<PDFGraphicState>` that `do_TJ` copied for a string (one per string,
//! shared by its chars, as Python shares the object), and the interpreter's
//! [`Color`]. The font reaches `LTChar.__init__` through
//! `layout::LTCharFont`, implemented by `PDFFont` (`pdffont`). `cur_item`
//! is an `LTPage` or the `Rc` of an `LTFigure`, so that the figure on the
//! stack is the object `end_figure` adds to its parent (`LTItem::Figure`).
//!
//! # Analysis
//!
//! `end_page` analyzes the page when the device has `laparams`, with the
//! device's [`IdSource`] (`CreationOrder` by default, the library's and
//! JavaScript's; the `layout` stage gives Python's ids, see
//! `layout.rs`, "Analysis and object identity").
//!
//! # Recursion depth
//!
//! Each method and function takes its frame; the layout objects are built
//! by class calls (the evaluation loop entry and the `__init__` frames,
//! `LTChar.__init__` -> `LTComponent.__init__` -> `set_bbox`...), which
//! `layout.rs` counts for the constructors the analyzer calls (to add in
//! wave 3: `layout.rs` counts its analysis today, not its constructors).
//! `"".join(x[0] for x in path)` resumes a generator from C (an entry and
//! a frame per item, leaves); the list comprehensions take no frame
//! (PEP 709); `re.finditer` is C; `paint_path` recurses once (a subpath has
//! one `m`). `log.debug` of `handle_undefined_char` takes its frames
//! through `pycompat::logging` (`Logger::new("pdfminer.converter")`).

#![allow(unused_variables)] // wave 3 skeleton: the bodies are todo!()

use std::cell::RefCell;
use std::rc::Rc;

use pycompat::{Num, PyInt, PyResult, PyStr};

use crate::layout::{CreationOrder, IdSource, LAParams, LTFigure, LTPage, LTPayload};
use crate::pdfcolor::PDFColorSpace;
use crate::pdfdevice::{text_device, ObjSeq, PDFDevice, PDFDeviceAttrs, PDFTextDevice, PDFTextSeq};
use crate::pdffont::{FontName, PDFFont};
use crate::pdfinterp::{Color, PDFGraphicState, PDFResourceManager};
use crate::pdfpage::PDFPage;
use crate::pdftypes::{PDFObject, PDFStream};
use crate::utils::{Matrix, PathSegment};

/// A layout payload whose types are the interpreter's (see the module
/// documentation); `Attr` is free (pdfminer never sets an attribute on a
/// layout object, pdfplumber sets `mcid` and `tag`).
pub trait InterpPayload: LTPayload<FontName = FontName, GraphicState = Rc<PDFGraphicState>, Color = Color> {}

/// The payload of pdfminer's own devices: no attribute is ever set
/// (`Attr` is never built).
#[derive(Clone)]
pub struct MinerPayload;

impl LTPayload for MinerPayload {
    type FontName = FontName;
    type GraphicState = Rc<PDFGraphicState>;
    type Color = Color;
    type Attr = PDFObject;
}

impl InterpPayload for MinerPayload {}

/// `self.cur_item`: the container being filled, an `LTPage` or an
/// `LTFigure` (`PDFLayoutAnalyzer.cur_item`).
pub enum CurItem<P: LTPayload> {
    Page(LTPage<P>),
    Figure(Rc<RefCell<LTFigure<P>>>),
}

/// The attributes of pdfminer/converter.py:61 `PDFLayoutAnalyzer` (and of
/// `PDFDevice`).
pub struct PDFLayoutAnalyzer<P: InterpPayload> {
    /// `PDFDevice.__init__`'s `rsrcmgr` and `ctm`.
    pub device: PDFDeviceAttrs,
    /// `self.pageno`: the `LTPage`'s `pageid`; `end_page` increments it.
    pub pageno: PyInt,
    /// `self.laparams`
    pub laparams: Option<LAParams>,
    /// `self._stack`: the containers of the figures being filled.
    pub _stack: Vec<CurItem<P>>,
    /// `self.cur_item`: `None` before `begin_page` (unset upstream).
    pub cur_item: Option<CurItem<P>>,
    /// Not upstream: the `id()` of the boxes and groups for the analysis of
    /// `end_page` (`layout.rs`, "Analysis and object identity").
    pub ids: Box<dyn IdSource<P>>,
}

impl<P: InterpPayload> PDFLayoutAnalyzer<P> {
    // pdfminer/converter.py:65 PDFLayoutAnalyzer.__init__
    /// The attributes, with the default ids ([`CreationOrder`]).
    pub fn new(rsrcmgr: Rc<PDFResourceManager>, pageno: PyInt, laparams: Option<LAParams>) -> PDFLayoutAnalyzer<P> {
        PDFLayoutAnalyzer {
            device: PDFDeviceAttrs::new(rsrcmgr),
            pageno,
            laparams,
            _stack: Vec::new(),
            cur_item: None,
            ids: Box::new(CreationOrder::default()),
        }
    }
}

/// A device that is a `PDFLayoutAnalyzer` (or a subclass): access to its
/// attributes, and the methods the analyzer calls on `self` that a
/// subclass may override.
pub trait LayoutAnalyzer<P: InterpPayload>: PDFTextDevice {
    fn analyzer(&self) -> &PDFLayoutAnalyzer<P>;
    fn analyzer_mut(&mut self) -> &mut PDFLayoutAnalyzer<P>;

    // pdfminer/converter.py:266 PDFLayoutAnalyzer.handle_undefined_char
    fn handle_undefined_char(&mut self, font: &PDFFont, cid: &PyInt) -> PyResult<PyStr> {
        layout_analyzer::handle_undefined_char(self, font, cid)
    }

    // pdfminer/converter.py:270 PDFLayoutAnalyzer.receive_layout
    /// Receives the finished page (`end_page` hands it over; the page is
    /// moved, `cur_item` is left `None`, which nothing reads again).
    fn receive_layout(&mut self, ltpage: LTPage<P>) -> PyResult<()> {
        layout_analyzer::receive_layout(self, ltpage)
    }
}

/// The bodies of `PDFLayoutAnalyzer`'s methods (`super()` for its
/// subclasses). A device implements its `PDFDevice` and `PDFTextDevice`
/// methods with them: `begin_page`, `end_page`, `begin_figure`,
/// `end_figure`, `render_image`, `paint_path`, `render_char`, and
/// `render_string` as `pdfdevice::text_device::render_string`.
pub mod layout_analyzer {
    use super::*;

    // pdfminer/converter.py:76 PDFLayoutAnalyzer.begin_page
    /// `cur_item = LTPage(pageno, (0, 0, |x0 - x1|, |y0 - y1|))` of the
    /// mediabox mapped by `ctm`.
    pub fn begin_page<P: InterpPayload, D: LayoutAnalyzer<P> + ?Sized>(
        dev: &mut D,
        page: &PDFPage,
        ctm: &Matrix,
    ) -> PyResult<()> {
        todo!("wave 3 CONVERTER")
    }

    // pdfminer/converter.py:81 PDFLayoutAnalyzer.end_page
    /// Asserts (an empty stack, an `LTPage`), analyzes with `laparams` when
    /// set (with `ids`), increments `pageno`, calls `dev.receive_layout`.
    pub fn end_page<P: InterpPayload, D: LayoutAnalyzer<P> + ?Sized>(dev: &mut D, page: &PDFPage) -> PyResult<()> {
        todo!("wave 3 CONVERTER")
    }

    // pdfminer/converter.py:89 PDFLayoutAnalyzer.begin_figure
    /// Pushes `cur_item`; `cur_item = LTFigure(name, bbox,
    /// mult_matrix(matrix, self.ctm))` on the unchecked items
    /// (`LTFigure::new_obj`, `utils::mult_matrix_obj`).
    pub fn begin_figure<P: InterpPayload, D: LayoutAnalyzer<P> + ?Sized>(
        dev: &mut D,
        name: &str,
        bbox: &ObjSeq,
        matrix: &ObjSeq,
    ) -> PyResult<()> {
        todo!("wave 3 CONVERTER")
    }

    // pdfminer/converter.py:93 PDFLayoutAnalyzer.end_figure
    pub fn end_figure<P: InterpPayload, D: LayoutAnalyzer<P> + ?Sized>(dev: &mut D, name: &str) -> PyResult<()> {
        todo!("wave 3 CONVERTER")
    }

    // pdfminer/converter.py:99 PDFLayoutAnalyzer.render_image
    /// Asserts a figure; adds `LTImage(name, stream, figure's bbox)`.
    pub fn render_image<P: InterpPayload, D: LayoutAnalyzer<P> + ?Sized>(
        dev: &mut D,
        name: &str,
        stream: &Rc<PDFStream>,
    ) -> PyResult<()> {
        todo!("wave 3 CONVERTER")
    }

    // pdfminer/converter.py:108 PDFLayoutAnalyzer.paint_path
    /// The shape of the path: nothing unless it starts with `m`; each
    /// subpath again through `dev.paint_path` when it has several `m`
    /// (`re.finditer(r"m[^m]+", shape)`); else an `LTLine`, `LTRect` or
    /// `LTCurve` with the points mapped by `ctm`.
    pub fn paint_path<P: InterpPayload, D: LayoutAnalyzer<P> + ?Sized>(
        dev: &mut D,
        gstate: &PDFGraphicState,
        stroke: bool,
        fill: bool,
        evenodd: bool,
        path: &[PathSegment],
    ) -> PyResult<()> {
        todo!("wave 3 CONVERTER")
    }

    // pdfminer/converter.py:233 PDFLayoutAnalyzer.render_char
    /// `font.to_unichr(cid)` (or `dev.handle_undefined_char` on
    /// `PDFUnicodeNotDefined`), `char_width`, `char_disp`, then `LTChar`
    /// added to `cur_item`; returns its `adv`.
    pub fn render_char<P: InterpPayload, D: LayoutAnalyzer<P> + ?Sized>(
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
        todo!("wave 3 CONVERTER")
    }

    // pdfminer/converter.py:266 PDFLayoutAnalyzer.handle_undefined_char
    /// `f"(cid:{cid})"`
    pub fn handle_undefined_char<P: InterpPayload, D: LayoutAnalyzer<P> + ?Sized>(
        dev: &mut D,
        font: &PDFFont,
        cid: &PyInt,
    ) -> PyResult<PyStr> {
        todo!("wave 3 CONVERTER")
    }

    // pdfminer/converter.py:270 PDFLayoutAnalyzer.receive_layout
    pub fn receive_layout<P: InterpPayload, D: LayoutAnalyzer<P> + ?Sized>(
        dev: &mut D,
        ltpage: LTPage<P>,
    ) -> PyResult<()> {
        todo!("wave 3 CONVERTER")
    }
}

/// pdfminer/converter.py:274 `PDFPageAggregator`: the analyzer that keeps
/// the last page (`get_result`).
pub struct PDFPageAggregator<P: InterpPayload> {
    pub analyzer: PDFLayoutAnalyzer<P>,
    /// `self.result`
    pub result: Option<LTPage<P>>,
}

impl<P: InterpPayload> PDFPageAggregator<P> {
    // pdfminer/converter.py:275 PDFPageAggregator.__init__
    pub fn new(rsrcmgr: Rc<PDFResourceManager>, pageno: PyInt, laparams: Option<LAParams>) -> PDFPageAggregator<P> {
        PDFPageAggregator { analyzer: PDFLayoutAnalyzer::new(rsrcmgr, pageno, laparams), result: None }
    }
}

/// A device that is a `PDFPageAggregator` (or a subclass).
pub trait PageAggregator<P: InterpPayload>: LayoutAnalyzer<P> {
    fn aggregator(&self) -> &PDFPageAggregator<P>;
    fn aggregator_mut(&mut self) -> &mut PDFPageAggregator<P>;
}

/// The bodies of `PDFPageAggregator`'s methods.
pub mod page_aggregator {
    use super::*;

    // pdfminer/converter.py:284 PDFPageAggregator.receive_layout
    pub fn receive_layout<P: InterpPayload, D: PageAggregator<P> + ?Sized>(
        dev: &mut D,
        ltpage: LTPage<P>,
    ) -> PyResult<()> {
        todo!("wave 3 CONVERTER")
    }

    // pdfminer/converter.py:287 PDFPageAggregator.get_result
    /// `assert self.result is not None` (`AssertionError`).
    pub fn get_result<P: InterpPayload, D: PageAggregator<P> + ?Sized>(dev: &D) -> PyResult<&LTPage<P>> {
        todo!("wave 3 CONVERTER")
    }
}

/// The device: `PDFDevice`'s defaults where `PDFLayoutAnalyzer` keeps
/// them (`set_ctm`, the tags, `close`), the analyzer's methods elsewhere.
impl<P: InterpPayload> PDFDevice for PDFPageAggregator<P> {
    fn attrs(&self) -> &PDFDeviceAttrs {
        &self.analyzer.device
    }

    fn attrs_mut(&mut self) -> &mut PDFDeviceAttrs {
        &mut self.analyzer.device
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
        layout_analyzer::paint_path(self, graphicstate, stroke, fill, evenodd, path)
    }

    fn render_image(&mut self, name: &str, stream: &Rc<PDFStream>) -> PyResult<()> {
        layout_analyzer::render_image(self, name, stream)
    }

    fn render_string(
        &mut self,
        textstate: &mut crate::pdfinterp::PDFTextState,
        seq: &PDFTextSeq,
        ncs: &Rc<PDFColorSpace>,
        graphicstate: &Rc<PDFGraphicState>,
    ) -> PyResult<()> {
        text_device::render_string(self, textstate, seq, ncs, graphicstate)
    }
}

impl<P: InterpPayload> PDFTextDevice for PDFPageAggregator<P> {
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
        layout_analyzer::render_char(self, matrix, font, fontsize, scaling, rise, cid, ncs, graphicstate)
    }
}

impl<P: InterpPayload> LayoutAnalyzer<P> for PDFPageAggregator<P> {
    fn analyzer(&self) -> &PDFLayoutAnalyzer<P> {
        &self.analyzer
    }

    fn analyzer_mut(&mut self) -> &mut PDFLayoutAnalyzer<P> {
        &mut self.analyzer
    }

    fn receive_layout(&mut self, ltpage: LTPage<P>) -> PyResult<()> {
        page_aggregator::receive_layout(self, ltpage)
    }
}

impl<P: InterpPayload> PageAggregator<P> for PDFPageAggregator<P> {
    fn aggregator(&self) -> &PDFPageAggregator<P> {
        self
    }

    fn aggregator_mut(&mut self) -> &mut PDFPageAggregator<P> {
        self
    }
}
