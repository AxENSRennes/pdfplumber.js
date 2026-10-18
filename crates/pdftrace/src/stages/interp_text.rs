//! Stage `interp_text`: the `interp` stage on pdfminer's text device, so
//! that the interpreter is checked without pdfplumber's device. The
//! traversal, the device and the lines are those of
//! `oracle/stages/interp_text.py`: [`TextDevice`] is `PDFTextDevice` with
//! the constructor of pdfplumber's device class and the check of its
//! `begin_figure` (a figure matrix whose product with the CTM is not
//! numbers raises `TypeError`, as pdfplumber's `LTFigure` would), and
//! [`TracingDevice`] records its calls as `interp::TracingDevice` does
//! (the same records and formatter, `stages::interp`).

use std::io::{self, Write};
use std::rc::Rc;

use pdfminer::pdfcolor::PDFColorSpace;
use pdfminer::pdfdevice::{device, text_device, ObjSeq, PDFDevice, PDFDeviceAttrs, PDFTextDevice, PDFTextSeq};
use pdfminer::pdffont::PDFFont;
use pdfminer::pdfinterp::{PDFGraphicState, PDFResourceManager, PDFStackT, PDFTextState};
use pdfminer::pdfpage::PDFPage;
use pdfminer::pdftypes::PDFStream;
use pdfminer::psparser::PSLiteral;
use pdfminer::utils::{mult_matrix_obj, Matrix, PathSegment};
use pycompat::recursion::{eval_entry, leaf, py_frame};
use pycompat::{exc, Num, PyInt, PyResult};

use super::interp::{run_with, traced, traced_render_char, traced_render_string, Call, Traced};
use super::{Stage, StageInput};

pub const STAGE: Stage = Stage { name: "interp_text", run: Some(run), oracle_input: false };

/// `interp_text.TextDevice`: `PDFTextDevice` with the constructor of
/// pdfplumber's device class.
pub struct TextDevice {
    pub attrs: PDFDeviceAttrs,
}

impl TextDevice {
    /// `TextDevice(rsrcmgr, pageno=..., laparams=None)`: the class call,
    /// `TextDevice.__init__` and `PDFDevice.__init__`.
    pub fn new(rsrcmgr: Rc<PDFResourceManager>) -> PyResult<TextDevice> {
        let _entry = eval_entry()?;
        let _frame = py_frame()?;
        Ok(TextDevice { attrs: device::init(rsrcmgr)? })
    }
}

/// The bodies of `TextDevice`'s own methods.
pub mod text_base {
    use super::*;

    /// `TextDevice.begin_figure`: `mult_matrix(matrix, self.ctm)` (its
    /// frame), and `TypeError` when an item of the result is not a
    /// number.
    pub fn begin_figure<D: PDFDevice + ?Sized>(
        dev: &mut D,
        _name: &str,
        _bbox: &ObjSeq,
        matrix: &ObjSeq,
    ) -> PyResult<()> {
        let _frame = py_frame()?;
        // utils.mult_matrix: a Python function
        leaf(1)?;
        let Some(ctm) = dev.attrs().ctm.clone() else {
            return Err(exc::TypeError("cannot unpack non-iterable NoneType object"));
        };
        if mult_matrix_obj(&matrix.items(), &ctm)?.to_matrix().is_none() {
            return Err(exc::TypeError("figure matrix is not numbers"));
        }
        Ok(())
    }
}

impl PDFDevice for TextDevice {
    fn attrs(&self) -> &PDFDeviceAttrs {
        &self.attrs
    }

    fn attrs_mut(&mut self) -> &mut PDFDeviceAttrs {
        &mut self.attrs
    }

    fn begin_figure(&mut self, name: &str, bbox: &ObjSeq, matrix: &ObjSeq) -> PyResult<()> {
        text_base::begin_figure(self, name, bbox, matrix)
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

impl PDFTextDevice for TextDevice {}

/// `interp_text.TracingDevice`
pub struct TracingDevice {
    pub inner: TextDevice,
    /// `self.calls`
    pub calls: Vec<Call>,
}

impl Traced for TracingDevice {
    fn calls_mut(&mut self) -> &mut Vec<Call> {
        &mut self.calls
    }
}

impl PDFDevice for TracingDevice {
    fn attrs(&self) -> &PDFDeviceAttrs {
        &self.inner.attrs
    }

    fn attrs_mut(&mut self) -> &mut PDFDeviceAttrs {
        &mut self.inner.attrs
    }

    fn set_ctm(&mut self, ctm: Matrix) -> PyResult<()> {
        traced(self, Call::SetCtm(ctm.clone()), |d| device::set_ctm(d, ctm))
    }

    fn begin_tag(&mut self, tag: &PSLiteral, props: Option<&PDFStackT>) -> PyResult<()> {
        traced(self, Call::BeginTag(*tag, props.cloned()), |d| device::begin_tag(d, tag, props))
    }

    fn end_tag(&mut self) -> PyResult<()> {
        traced(self, Call::EndTag, device::end_tag)
    }

    fn do_tag(&mut self, tag: &PSLiteral, props: Option<&PDFStackT>) -> PyResult<()> {
        traced(self, Call::DoTag(*tag, props.cloned()), |d| device::do_tag(d, tag, props))
    }

    fn begin_page(&mut self, page: &PDFPage, ctm: &Matrix) -> PyResult<()> {
        traced(self, Call::BeginPage(page.pageid.clone(), ctm.clone()), |d| device::begin_page(d, page, ctm))
    }

    fn end_page(&mut self, page: &PDFPage) -> PyResult<()> {
        traced(self, Call::EndPage(page.pageid.clone()), |d| device::end_page(d, page))
    }

    fn begin_figure(&mut self, name: &str, bbox: &ObjSeq, matrix: &ObjSeq) -> PyResult<()> {
        let record = Call::BeginFigure(name.to_owned(), bbox.clone(), matrix.clone());
        traced(self, record, |d| text_base::begin_figure(d, name, bbox, matrix))
    }

    fn end_figure(&mut self, name: &str) -> PyResult<()> {
        traced(self, Call::EndFigure(name.to_owned()), |d| device::end_figure(d, name))
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
        traced(self, record, |d| device::paint_path(d, graphicstate, stroke, fill, evenodd, path))
    }

    fn render_image(&mut self, name: &str, stream: &Rc<PDFStream>) -> PyResult<()> {
        traced(self, Call::RenderImage(name.to_owned(), stream.clone()), |d| device::render_image(d, name, stream))
    }

    fn render_string(
        &mut self,
        textstate: &mut PDFTextState,
        seq: &PDFTextSeq,
        ncs: &Rc<PDFColorSpace>,
        graphicstate: &Rc<PDFGraphicState>,
    ) -> PyResult<()> {
        traced_render_string(self, textstate, seq, ncs, graphicstate, |d, ts| {
            text_device::render_string(d, ts, seq, ncs, graphicstate)
        })
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
        traced_render_char(self, matrix, font, fontsize, scaling, rise, cid, ncs, graphicstate, |d, m| {
            text_device::render_char(d, m, font, fontsize, scaling, rise, cid, ncs, graphicstate)
        })
    }
}

fn run(input: &StageInput<'_>, out: &mut dyn Write) -> io::Result<()> {
    run_with(
        input,
        out,
        |rsrcmgr, _pageno| Ok(TracingDevice { inner: TextDevice::new(rsrcmgr.clone())?, calls: Vec::new() }),
        |d| std::mem::take(&mut d.calls),
    )
}
