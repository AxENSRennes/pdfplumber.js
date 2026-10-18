//! Stage `interp`: every call pdfminer's interpreter makes to the device,
//! with the graphic and text states, as pdfplumber drives it.
//!
//! Not implemented yet (wave 3, workstream INTERP). The traversal and the
//! line format are those of `oracle/stages/interp.py`; this file holds the
//! tracing device the runner will use, which compiles against the wave 3
//! interfaces (`pdfminer::pdfdevice`, `pdfminer::converter`,
//! `pdfplumber::page`).
//!
//! # The runner, once the interpreter is ported
//!
//! ```text
//! terminated(out, |lines| {
//!     let doc = open_document(input)?;
//!     let rsrcmgr = Rc::new(PDFResourceManager::new(true));
//!     for (pageno, page) in PDFPage::create_pages(&doc).enumerate() {  // from 1
//!         page line
//!         let (device, interpreter, error) = common::page_layout(&rsrcmgr, &page,
//!             || Ok(TracingDevice::new(rsrcmgr.clone(), pageno)))?;
//!         format device.calls (fonts by identity: Rc::as_ptr; inline images:
//!             the names inline_image_name(k), k < interpreter.inline_images.get())
//!         error line; stop after max_pages
//!     }
//! })
//! ```
//!
//! [`TracingDevice`] is the Python `TracingDevice`: a subclass of
//! `PDFPageAggregatorWithMarkedContent` whose every method takes its own
//! frame (the override's), records a snapshot of its arguments and calls
//! the method of `PDFPageAggregatorWithMarkedContent` (the free functions
//! of `marked_content`, `layout_analyzer`, `pdfdevice::device` and
//! `text_device`, `super()`), so that the calls the analyzer makes on
//! `self` (`paint_path` for a subpath, `render_char` from
//! `render_string_horizontal`) are recorded too. Nothing is formatted while
//! the interpreter runs. The Python `TracingDevice` classifies an inline
//! image's name by its call site (`do_EI`'s frame); here the names the
//! interpreter gave inline images are known
//! (`pdfinterp::inline_image_name`, `PDFPageInterpreter::inline_images`):
//! the two agree but on a PDF that names an XObject like the port names
//! inline images (`inline-0`), which no corpus does
//! (`docs/architecture.md`, "Vague 3").

#![allow(unused_variables)] // wave 3 skeleton: the bodies are todo!()

use std::rc::Rc;

use pdfminer::converter::{layout_analyzer, LayoutAnalyzer, PDFLayoutAnalyzer, PDFPageAggregator, PageAggregator};
use pdfminer::layout::LTPage;
use pdfminer::pdfcolor::PDFColorSpace;
use pdfminer::pdfdevice::{device, ObjSeq, PDFDevice, PDFDeviceAttrs, PDFTextDevice, PDFTextSeq};
use pdfminer::pdffont::PDFFont;
use pdfminer::pdfinterp::{PDFGraphicState, PDFResourceManager, PDFStackT, PDFTextState};
use pdfminer::pdfpage::PDFPage;
use pdfminer::pdftypes::{PDFObject, PDFStream};
use pdfminer::psparser::PSLiteral;
use pdfminer::utils::{Matrix, PathSegment, Point};
use pdfplumber::page::{
    marked_content, MarkedContent, MarkedContentState, PDFPageAggregatorWithMarkedContent, PlumberPayload,
};
use pycompat::{Num, PyErr, PyInt, PyResult};

use super::Stage;

pub const STAGE: Stage = Stage { name: "interp", run: None, oracle_input: false };

/// A device call, as `TracingDevice` records it (`device.calls` of
/// `oracle/stages/interp.py`): the arguments on entry (the states are
/// snapshots: the interpreter's live states are copied).
pub enum Call {
    SetCtm(Matrix),
    BeginTag(PSLiteral, Option<PDFStackT>),
    EndTag,
    DoTag(PSLiteral, Option<PDFStackT>),
    /// The page's `pageid`, the CTM.
    BeginPage(PDFObject, Matrix),
    EndPage(PDFObject),
    BeginFigure(String, ObjSeq, ObjSeq),
    EndFigure(String),
    RenderImage(String, Rc<PDFStream>),
    PaintPath {
        gstate: Box<PDFGraphicState>,
        stroke: bool,
        fill: bool,
        evenodd: bool,
        path: Vec<PathSegment>,
    },
    RenderString {
        textstate: Box<PDFTextState>,
        seq: PDFTextSeq,
        ncs: Rc<PDFColorSpace>,
        gstate: Box<PDFGraphicState>,
    },
    /// `textstate.linematrix` when `render_string` returned.
    EndString(Point),
    /// One record, written when the call returned or raised.
    RenderChar {
        matrix: Matrix,
        font: Rc<PDFFont>,
        fontsize: Num,
        scaling: Num,
        rise: Num,
        cid: PyInt,
        ncs: Rc<PDFColorSpace>,
        gstate: Rc<PDFGraphicState>,
        result: Result<Num, PyErr>,
    },
    /// The call recorded before (and not a `render_char`) raised.
    Raised(PyErr),
}

/// `TracingDevice` of `oracle/stages/interp.py`.
pub struct TracingDevice {
    /// The attributes of `PDFPageAggregatorWithMarkedContent`.
    pub inner: PDFPageAggregatorWithMarkedContent,
    /// `self.calls`
    pub calls: Vec<Call>,
}

impl TracingDevice {
    /// `TracingDevice(rsrcmgr, pageno=pageno, laparams=None)`.
    pub fn new(rsrcmgr: Rc<PDFResourceManager>, pageno: PyInt) -> TracingDevice {
        TracingDevice { inner: PDFPageAggregatorWithMarkedContent::new(rsrcmgr, pageno, None), calls: Vec::new() }
    }

    /// The pattern of every override: the override's frame, the record on
    /// entry, `super()`, and a `Raised` record if it raised.
    fn traced<T>(&mut self, record: Call, call: impl FnOnce(&mut TracingDevice) -> PyResult<T>) -> PyResult<T> {
        todo!("wave 3 INTERP: py_frame(), self.calls.push(record), call(self), Raised on error")
    }
}

impl PDFDevice for TracingDevice {
    fn attrs(&self) -> &PDFDeviceAttrs {
        self.inner.attrs()
    }

    fn attrs_mut(&mut self) -> &mut PDFDeviceAttrs {
        self.inner.attrs_mut()
    }

    fn set_ctm(&mut self, ctm: Matrix) -> PyResult<()> {
        self.traced(Call::SetCtm(ctm.clone()), |d| device::set_ctm(d, ctm))
    }

    fn begin_tag(&mut self, tag: &PSLiteral, props: Option<&PDFStackT>) -> PyResult<()> {
        self.traced(Call::BeginTag(*tag, props.cloned()), |d| marked_content::begin_tag(d, tag, props))
    }

    fn end_tag(&mut self) -> PyResult<()> {
        self.traced(Call::EndTag, marked_content::end_tag)
    }

    fn do_tag(&mut self, tag: &PSLiteral, props: Option<&PDFStackT>) -> PyResult<()> {
        self.traced(Call::DoTag(*tag, props.cloned()), |d| device::do_tag(d, tag, props))
    }

    fn begin_page(&mut self, page: &PDFPage, ctm: &Matrix) -> PyResult<()> {
        self.traced(Call::BeginPage(page.pageid.clone(), ctm.clone()), |d| layout_analyzer::begin_page(d, page, ctm))
    }

    fn end_page(&mut self, page: &PDFPage) -> PyResult<()> {
        self.traced(Call::EndPage(page.pageid.clone()), |d| layout_analyzer::end_page(d, page))
    }

    fn begin_figure(&mut self, name: &str, bbox: &ObjSeq, matrix: &ObjSeq) -> PyResult<()> {
        let record = Call::BeginFigure(name.to_owned(), bbox.clone(), matrix.clone());
        self.traced(record, |d| layout_analyzer::begin_figure(d, name, bbox, matrix))
    }

    fn end_figure(&mut self, name: &str) -> PyResult<()> {
        self.traced(Call::EndFigure(name.to_owned()), |d| layout_analyzer::end_figure(d, name))
    }

    fn paint_path(
        &mut self,
        graphicstate: &PDFGraphicState,
        stroke: bool,
        fill: bool,
        evenodd: bool,
        path: &[PathSegment],
    ) -> PyResult<()> {
        let record =
            Call::PaintPath { gstate: Box::new(graphicstate.clone()), stroke, fill, evenodd, path: path.to_vec() };
        self.traced(record, |d| marked_content::paint_path(d, graphicstate, stroke, fill, evenodd, path))
    }

    fn render_image(&mut self, name: &str, stream: &Rc<PDFStream>) -> PyResult<()> {
        self.traced(Call::RenderImage(name.to_owned(), stream.clone()), |d| {
            marked_content::render_image(d, name, stream)
        })
    }

    fn render_string(
        &mut self,
        textstate: &mut PDFTextState,
        seq: &PDFTextSeq,
        ncs: &Rc<PDFColorSpace>,
        graphicstate: &Rc<PDFGraphicState>,
    ) -> PyResult<()> {
        todo!("wave 3 INTERP: traced(RenderString {{ .. }}, pdfdevice::text_device::render_string), then EndString")
    }
}

impl PDFTextDevice for TracingDevice {
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
        todo!("wave 3 INTERP: frame, marked_content::render_char, one RenderChar record with the result")
    }
}

impl LayoutAnalyzer<PlumberPayload> for TracingDevice {
    fn analyzer(&self) -> &PDFLayoutAnalyzer<PlumberPayload> {
        self.inner.analyzer()
    }

    fn analyzer_mut(&mut self) -> &mut PDFLayoutAnalyzer<PlumberPayload> {
        self.inner.analyzer_mut()
    }

    fn receive_layout(&mut self, ltpage: LTPage<PlumberPayload>) -> PyResult<()> {
        pdfminer::converter::page_aggregator::receive_layout(self, ltpage)
    }
}

impl PageAggregator<PlumberPayload> for TracingDevice {
    fn aggregator(&self) -> &PDFPageAggregator<PlumberPayload> {
        self.inner.aggregator()
    }

    fn aggregator_mut(&mut self) -> &mut PDFPageAggregator<PlumberPayload> {
        self.inner.aggregator_mut()
    }
}

impl MarkedContent for TracingDevice {
    fn marked(&self) -> &MarkedContentState {
        self.inner.marked()
    }

    fn marked_mut(&mut self) -> &mut MarkedContentState {
        self.inner.marked_mut()
    }
}
