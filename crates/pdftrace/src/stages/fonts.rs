//! Stage `fonts`: the fonts the interpreter obtains for the pages and, for
//! every string it shows with them, `decode`, `to_unichr`, `char_width`,
//! `char_disp`. The traversal and the line format are those of
//! `oracle/stages/fonts.py`.
//!
//! [`FontsDevice`] is a subclass of `PDFPageAggregatorWithMarkedContent`
//! (like `interp::TracingDevice`) whose `render_string` takes its frame,
//! records `(textstate.font, seq)` and calls
//! `pdfdevice::text_device::render_string`. The font report (`font_attrs`)
//! is the one `tests/fonts_unit.rs` writes for `oracle/unit/fonts.py`
//! (`crate::fonts_report`). [`run_with`] is shared with the `fonts_text`
//! stage.
//!
//! Wave 3: this stage needs the converter (`PDFLayoutAnalyzer`,
//! `PDFPageAggregatorWithMarkedContent`), ported by the CONVERTER
//! workstream; until it is merged, the `fonts_text` stage checks the same
//! lines on `PDFTextDevice`.

use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use std::rc::Rc;

use pdfminer::converter::{layout_analyzer, LayoutAnalyzer, PDFLayoutAnalyzer, PDFPageAggregator, PageAggregator};
use pdfminer::layout::LTPage;
use pdfminer::pdfcolor::PDFColorSpace;
use pdfminer::pdfdevice::{
    device, text_device, text_seq_items, ObjSeq, PDFDevice, PDFDeviceAttrs, PDFTextDevice, PDFTextSeq, SeqItem,
};
use pdfminer::pdffont::PDFFont;
use pdfminer::pdfinterp::{PDFGraphicState, PDFResourceManager, PDFStackT, PDFTextState};
use pdfminer::pdfpage::PDFPage;
use pdfminer::pdftypes::{PDFObject, PDFStream};
use pdfminer::psparser::PSLiteral;
use pdfminer::utils::{Matrix, PathSegment};
use pdfplumber::page::{
    marked_content, MarkedContent, MarkedContentState, PDFPageAggregatorWithMarkedContent, PlumberPayload,
};
use pycompat::recursion::py_frame;
use pycompat::{Num, PyInt, PyResult};

use super::common::{open_document, page_layout, terminated, Lines};
use super::interp::Fonts;
use super::{Stage, StageInput};
use crate::canon::{canon, canon_exc, canon_obj};
use crate::fonts_report::{call_seq, char_disp, font_attrs, in_frame, res, unichr};
use crate::value::Value;

pub const STAGE: Stage = Stage { name: "fonts", run: Some(run), oracle_input: false };

/// `MAX_STRINGS`: strings queried per font (the first ones shown in the
/// document).
pub const MAX_STRINGS: usize = 2000;

/// What a fonts device records: `(textstate.font, seq)` of each
/// `render_string`.
pub type ShownString = (Option<Rc<PDFFont>>, PDFTextSeq);

/// `fonts.FontsDevice`
pub struct FontsDevice {
    /// The attributes of `PDFPageAggregatorWithMarkedContent`.
    pub inner: PDFPageAggregatorWithMarkedContent,
    /// `self.calls`
    pub calls: Vec<ShownString>,
}

impl FontsDevice {
    /// `FontsDevice(rsrcmgr, pageno=pageno, laparams=None)`.
    pub fn new(rsrcmgr: Rc<PDFResourceManager>, pageno: PyInt) -> FontsDevice {
        FontsDevice { inner: PDFPageAggregatorWithMarkedContent::new(rsrcmgr, pageno, None), calls: Vec::new() }
    }
}

impl PDFDevice for FontsDevice {
    fn attrs(&self) -> &PDFDeviceAttrs {
        self.inner.attrs()
    }

    fn attrs_mut(&mut self) -> &mut PDFDeviceAttrs {
        self.inner.attrs_mut()
    }

    fn begin_tag(&mut self, tag: &PSLiteral, props: Option<&PDFStackT>) -> PyResult<()> {
        marked_content::begin_tag(self, tag, props)
    }

    fn end_tag(&mut self) -> PyResult<()> {
        marked_content::end_tag(self)
    }

    fn do_tag(&mut self, tag: &PSLiteral, props: Option<&PDFStackT>) -> PyResult<()> {
        device::do_tag(self, tag, props)
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
        let _frame = py_frame()?;
        self.calls.push((textstate.font.clone(), seq.clone()));
        text_device::render_string(self, textstate, seq, ncs, graphicstate)
    }
}

impl PDFTextDevice for FontsDevice {
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

impl LayoutAnalyzer<PlumberPayload> for FontsDevice {
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

impl PageAggregator<PlumberPayload> for FontsDevice {
    fn aggregator(&self) -> &PDFPageAggregator<PlumberPayload> {
        self.inner.aggregator()
    }

    fn aggregator_mut(&mut self) -> &mut PDFPageAggregator<PlumberPayload> {
        self.inner.aggregator_mut()
    }
}

impl MarkedContent for FontsDevice {
    fn marked(&self) -> &MarkedContentState {
        self.inner.marked()
    }

    fn marked_mut(&mut self) -> &mut MarkedContentState {
        self.inner.marked_mut()
    }
}

// ---------------------------------------------------------------------------
// The lines

/// `font{"n": n}`
fn font_ref(n: usize) -> Value {
    Value::Named { name: "font".into(), fields: vec![(Value::str("n"), Value::Int(PyInt::Small(n as i64)))] }
}

/// `line(label, *values)`
fn line(label: &str, values: &[Value]) -> String {
    let mut out = label.to_owned();
    for v in values {
        out.push(' ');
        out.push_str(&canon(v));
    }
    out
}

/// `fonts._Doc`: what the document's lines have shown so far.
#[derive(Default)]
struct Doc {
    fonts: Fonts,
    /// The strings queried, per font.
    strings: HashMap<usize, HashSet<Vec<u8>>>,
    /// The codes queried, per font.
    cids: HashMap<usize, HashSet<PyInt>>,
    lines: Vec<String>,
}

impl Doc {
    /// `font{"n": n}`, after the font's `font` line if it is new.
    fn font(&mut self, font: &Rc<PDFFont>) -> (usize, Value) {
        let (n, new) = self.fonts.number(font);
        if new {
            let report = font_attrs(font);
            self.lines.push(line("font", &[Value::Int(PyInt::Small(n as i64)), report]));
            self.strings.insert(n, HashSet::new());
            self.cids.insert(n, HashSet::new());
        }
        (n, font_ref(n))
    }

    /// `shown(font, s)`
    fn shown(&mut self, font: &Rc<PDFFont>, s: &[u8]) {
        let (n, fref) = self.font(font);
        let strings = self.strings.get_mut(&n).expect("a known font");
        if strings.contains(s) || strings.len() >= MAX_STRINGS {
            return;
        }
        strings.insert(s.to_vec());
        // _call(_decode, font, s): `for cid in font.decode(s)` in a frame
        let cids = (|| {
            let _call = py_frame()?;
            let _decode = py_frame()?;
            font.decode(s)
        })();
        let bytes = Value::Bytes(s.to_vec());
        let cids = match cids {
            Err(e) => {
                self.lines.push(line("string", &[fref, bytes, Value::from(&e)]));
                return;
            }
            Ok(cids) => cids,
        };
        self.lines
            .push(line("string", &[fref.clone(), bytes, Value::List(cids.iter().cloned().map(Value::Int).collect())]));
        for cid in cids {
            let seen = self.cids.get_mut(&n).expect("a known font");
            if !seen.insert(cid.clone()) {
                continue;
            }
            // _call(f, cid), _call_seq(f, cid): their frames
            let to_unichr = res(in_frame(|| font.to_unichr(&cid)), |u| unichr(&u));
            let char_width = call_seq(|| font.char_width(&cid));
            let disp = res(in_frame(|| font.char_disp(&cid)), |d| char_disp(&d));
            self.lines.push(line("cid", &[fref.clone(), Value::Int(cid.clone()), to_unichr, char_width, disp]));
        }
    }
}

/// The runner of `fonts` and `fonts_text`: `make` builds the page's
/// device, `calls` takes its records.
pub fn run_with<D: PDFDevice + 'static>(
    input: &StageInput<'_>,
    out: &mut dyn Write,
    make: impl Fn(&Rc<PDFResourceManager>, PyInt) -> PyResult<D>,
    calls: impl Fn(&mut D) -> Vec<ShownString>,
) -> io::Result<()> {
    terminated(out, |lines: &mut Lines| {
        let doc = open_document(input)?;
        let rsrcmgr = Rc::new(PDFResourceManager::new(true));
        let mut state = Doc::default();
        for (pageno, page) in PDFPage::create_pages(&doc).enumerate() {
            let pageno = pageno + 1;
            let page = page?;
            lines.line("page", &[&pageno.to_string(), &canon_obj(&page.pageid)]);
            let ncached = rsrcmgr.cached_fonts().len();
            let pageno_int = PyInt::Small(pageno as i64);
            let (device, interpreter, error) = page_layout(&rsrcmgr, &page, || make(&rsrcmgr, pageno_int))?;
            for (fontid, font) in interpreter.fontmap.iter().flatten() {
                let (_, fref) = state.font(font);
                state.lines.push(line("fontmap", &[Value::str(fontid.to_string()), fref]));
            }
            for (objid, font) in rsrcmgr.cached_fonts().into_iter().skip(ncached) {
                let (_, fref) = state.font(&font);
                state.lines.push(line("cached", &[Value::Int(objid), fref]));
            }
            for (font, seq) in calls(&mut device.borrow_mut()) {
                // list(seq): render_string raised on it too when it fails
                let Ok(items) = text_seq_items(&seq) else { continue };
                let Some(font) = font else { continue };
                for s in items {
                    if let SeqItem::Obj(PDFObject::Bytes(s)) = s {
                        state.shown(&font, &s);
                    }
                }
            }
            for l in state.lines.drain(..) {
                lines.push(l);
            }
            if let Some(e) = error {
                lines.push(format!("error {}", canon_exc(&e)));
            }
            if input.max_pages.is_some_and(|max| pageno >= max) {
                break;
            }
        }
        Ok(())
    })
}

fn run(input: &StageInput<'_>, out: &mut dyn Write) -> io::Result<()> {
    run_with(
        input,
        out,
        |rsrcmgr, pageno| Ok(FontsDevice::new(rsrcmgr.clone(), pageno)),
        |d| std::mem::take(&mut d.calls),
    )
}
