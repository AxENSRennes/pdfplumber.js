//! Stage `fonts_text`: the `fonts` stage on pdfminer's text device
//! (`interp_text::TextDevice`), so that the fonts the interpreter obtains
//! are checked without pdfplumber's device. The traversal and the lines
//! are those of `oracle/stages/fonts_text.py` (and `fonts.py`, whose
//! runner `stages::fonts::run_with` is shared).

use std::io::{self, Write};
use std::rc::Rc;

use pdfminer::pdfcolor::PDFColorSpace;
use pdfminer::pdfdevice::{text_device, ObjSeq, PDFDevice, PDFDeviceAttrs, PDFTextDevice, PDFTextSeq};
use pdfminer::pdfinterp::{PDFGraphicState, PDFTextState};
use pycompat::recursion::py_frame;
use pycompat::PyResult;

use super::fonts::{run_with, ShownString};
use super::interp_text::{text_base, TextDevice};
use super::{Stage, StageInput};

pub const STAGE: Stage = Stage { name: "fonts_text", run: Some(run), oracle_input: false };

/// `fonts_text.FontsDevice`: `fonts.FontsDevice` on `TextDevice`.
pub struct FontsDevice {
    pub inner: TextDevice,
    /// `self.calls`
    pub calls: Vec<ShownString>,
}

impl PDFDevice for FontsDevice {
    fn attrs(&self) -> &PDFDeviceAttrs {
        &self.inner.attrs
    }

    fn attrs_mut(&mut self) -> &mut PDFDeviceAttrs {
        &mut self.inner.attrs
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
        let _frame = py_frame()?;
        self.calls.push((textstate.font.clone(), seq.clone()));
        text_device::render_string(self, textstate, seq, ncs, graphicstate)
    }
}

impl PDFTextDevice for FontsDevice {}

fn run(input: &StageInput<'_>, out: &mut dyn Write) -> io::Result<()> {
    run_with(
        input,
        out,
        |rsrcmgr, _pageno| Ok(FontsDevice { inner: TextDevice::new(rsrcmgr.clone())?, calls: Vec::new() }),
        |d| std::mem::take(&mut d.calls),
    )
}
