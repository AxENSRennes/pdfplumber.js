//! Stage `interp`: every call pdfminer's interpreter makes to the device,
//! with the graphic and text states, as pdfplumber drives it. The
//! traversal and the line format are those of `oracle/stages/interp.py`.
//!
//! [`TracingDevice`] is the Python `TracingDevice`: a subclass of
//! `PDFPageAggregatorWithMarkedContent` whose every method takes its own
//! frame (the override's), records a snapshot of its arguments and calls
//! the method of `PDFPageAggregatorWithMarkedContent` (the free functions
//! of `marked_content`, `layout_analyzer`, `pdfdevice::device` and
//! `text_device`, `super()`), so that the calls the analyzer makes on
//! `self` (`paint_path` for a subpath, `render_char` from
//! `render_string_horizontal`) are recorded too. Nothing is formatted while
//! the interpreter runs ([`PageFormatter`] writes the lines of a page
//! afterwards, shared with the `interp_text` stage). The Python
//! `TracingDevice` classifies an inline image's name by its call site
//! (`do_EI`'s frame); here the names the interpreter gave inline images are
//! known (`pdfinterp::inline_image_name`,
//! `PDFPageInterpreter::inline_images`): the two agree but on a PDF that
//! names an XObject like the port names inline images (`inline-0`), which
//! no corpus does (`docs/architecture.md`, "Vague 3").
//!
//! Wave 3: this stage needs the converter (`PDFLayoutAnalyzer`,
//! `PDFPageAggregatorWithMarkedContent`), ported by the CONVERTER
//! workstream; until it is merged, its bodies are `todo!()` and the
//! `interp_text` stage checks the interpreter on `PDFTextDevice`.

use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use std::rc::Rc;

use pdfminer::converter::{layout_analyzer, LayoutAnalyzer, PDFLayoutAnalyzer, PDFPageAggregator, PageAggregator};
use pdfminer::layout::LTPage;
use pdfminer::pdfcolor::PDFColorSpace;
use pdfminer::pdfdevice::{device, text_device, ObjSeq, PDFDevice, PDFDeviceAttrs, PDFTextDevice, PDFTextSeq};
use pdfminer::pdffont::PDFFont;
use pdfminer::pdfinterp::{
    inline_image_name, Color, PDFGraphicState, PDFResourceManager, PDFStackT, PDFTextState, StandardColor,
};
use pdfminer::pdfpage::PDFPage;
use pdfminer::pdftypes::{PDFObject, PDFStream};
use pdfminer::psparser::PSLiteral;
use pdfminer::utils::{Matrix, PathSegment, Point};
use pdfplumber::page::{
    marked_content, MarkedContent, MarkedContentState, PDFPageAggregatorWithMarkedContent, PlumberPayload,
};
use pycompat::recursion::{py_frame, py_frames};
use pycompat::{Num, PyErr, PyInt, PyResult};

use super::common::{open_document, page_layout, terminated, Lines};
use super::{Stage, StageInput};
use crate::canon::{canon, canon_exc, canon_obj};
use crate::fonts_report::num;
use crate::value::Value;

pub const STAGE: Stage = Stage { name: "interp", run: Some(run), oracle_input: false };

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

/// A device that records [`Call`]s.
pub trait Traced {
    fn calls_mut(&mut self) -> &mut Vec<Call>;
}

/// The pattern of every override: the override's frame, the record on
/// entry, `super()`, and a `Raised` record if it raised.
pub fn traced<D: Traced + ?Sized, T>(
    dev: &mut D,
    record: Call,
    call: impl FnOnce(&mut D) -> PyResult<T>,
) -> PyResult<T> {
    let _frame = py_frame()?;
    dev.calls_mut().push(record);
    let result = call(dev);
    if let Err(e) = &result {
        dev.calls_mut().push(Call::Raised(e.clone()));
    }
    result
}

/// The override of `render_string`: the record (the text state and the
/// string's graphic state as `vars()` snapshots), `super()`, then
/// `end_string` with the new `linematrix`.
pub fn traced_render_string<D: Traced + ?Sized>(
    dev: &mut D,
    textstate: &mut PDFTextState,
    seq: &PDFTextSeq,
    ncs: &Rc<PDFColorSpace>,
    graphicstate: &Rc<PDFGraphicState>,
    call: impl FnOnce(&mut D, &mut PDFTextState) -> PyResult<()>,
) -> PyResult<()> {
    let record = Call::RenderString {
        textstate: Box::new(textstate.clone()),
        seq: seq.clone(),
        ncs: ncs.clone(),
        gstate: Box::new((**graphicstate).clone()),
    };
    traced(dev, record, |d| call(d, textstate))?;
    dev.calls_mut().push(Call::EndString(textstate.linematrix.clone()));
    Ok(())
}

/// The override of `render_char`: its frame, `super()`, one record with the
/// result or the exception.
#[allow(clippy::too_many_arguments)]
pub fn traced_render_char<D: Traced + ?Sized>(
    dev: &mut D,
    matrix: Matrix,
    font: &Rc<PDFFont>,
    fontsize: &Num,
    scaling: &Num,
    rise: &Num,
    cid: &PyInt,
    ncs: &Rc<PDFColorSpace>,
    graphicstate: &Rc<PDFGraphicState>,
    call: impl FnOnce(&mut D, Matrix) -> PyResult<Num>,
) -> PyResult<Num> {
    let _frame = py_frame()?;
    let result = call(dev, matrix.clone());
    dev.calls_mut().push(Call::RenderChar {
        matrix,
        font: font.clone(),
        fontsize: fontsize.clone(),
        scaling: scaling.clone(),
        rise: rise.clone(),
        cid: cid.clone(),
        ncs: ncs.clone(),
        gstate: graphicstate.clone(),
        result: result.clone(),
    });
    result
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
}

impl Traced for TracingDevice {
    fn calls_mut(&mut self) -> &mut Vec<Call> {
        &mut self.calls
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
        traced(self, Call::SetCtm(ctm.clone()), |d| device::set_ctm(d, ctm))
    }

    fn begin_tag(&mut self, tag: &PSLiteral, props: Option<&PDFStackT>) -> PyResult<()> {
        traced(self, Call::BeginTag(*tag, props.cloned()), |d| marked_content::begin_tag(d, tag, props))
    }

    fn end_tag(&mut self) -> PyResult<()> {
        traced(self, Call::EndTag, marked_content::end_tag)
    }

    fn do_tag(&mut self, tag: &PSLiteral, props: Option<&PDFStackT>) -> PyResult<()> {
        traced(self, Call::DoTag(*tag, props.cloned()), |d| device::do_tag(d, tag, props))
    }

    fn begin_page(&mut self, page: &PDFPage, ctm: &Matrix) -> PyResult<()> {
        traced(self, Call::BeginPage(page.pageid.clone(), ctm.clone()), |d| layout_analyzer::begin_page(d, page, ctm))
    }

    fn end_page(&mut self, page: &PDFPage) -> PyResult<()> {
        traced(self, Call::EndPage(page.pageid.clone()), |d| layout_analyzer::end_page(d, page))
    }

    fn begin_figure(&mut self, name: &str, bbox: &ObjSeq, matrix: &ObjSeq) -> PyResult<()> {
        let record = Call::BeginFigure(name.to_owned(), bbox.clone(), matrix.clone());
        traced(self, record, |d| layout_analyzer::begin_figure(d, name, bbox, matrix))
    }

    fn end_figure(&mut self, name: &str) -> PyResult<()> {
        traced(self, Call::EndFigure(name.to_owned()), |d| layout_analyzer::end_figure(d, name))
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
        traced(self, record, |d| marked_content::paint_path(d, graphicstate, stroke, fill, evenodd, path))
    }

    fn render_image(&mut self, name: &str, stream: &Rc<PDFStream>) -> PyResult<()> {
        traced(self, Call::RenderImage(name.to_owned(), stream.clone()), |d| {
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
            marked_content::render_char(d, m, font, fontsize, scaling, rise, cid, ncs, graphicstate)
        })
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

// ---------------------------------------------------------------------------
// The lines

/// `interp.Fonts`: fonts numbered by identity in order of first reference
/// (kept alive, so that an address is never reused).
#[derive(Default)]
pub struct Fonts {
    index: HashMap<*const PDFFont, usize>,
    fonts: Vec<Rc<PDFFont>>,
}

impl Fonts {
    /// The font's number, and whether it is new.
    pub fn number(&mut self, font: &Rc<PDFFont>) -> (usize, bool) {
        if let Some(&n) = self.index.get(&Rc::as_ptr(font)) {
            return (n, false);
        }
        let n = self.fonts.len();
        self.index.insert(Rc::as_ptr(font), n);
        self.fonts.push(font.clone());
        (n, true)
    }
}

/// `Named(name, {fields})` with `str` keys.
fn named(name: &str, fields: Vec<(&str, Value)>) -> Value {
    Value::Named { name: name.to_owned(), fields: fields.into_iter().map(|(k, v)| (Value::str(k), v)).collect() }
}

/// A tuple of numbers (a matrix, a point).
fn nums(items: &[Num]) -> Value {
    Value::Tuple(items.iter().map(num).collect())
}

/// `colorspace(cs)`: `PDFColorSpace{"name": .., "ncomponents": ..}`.
pub fn colorspace(cs: &PDFColorSpace) -> Value {
    named("PDFColorSpace", vec![("name", Value::str(cs.name.clone())), ("ncomponents", Value::from(&cs.ncomponents))])
}

fn standard_color(c: &StandardColor) -> Value {
    match c {
        StandardColor::Gray(g) => num(g),
        StandardColor::Rgb(rgb) => Value::Tuple(rgb.iter().map(|&x| Value::Float(x)).collect()),
        StandardColor::Cmyk(cmyk) => Value::Tuple(cmyk.iter().map(|&x| Value::Float(x)).collect()),
    }
}

/// A `Color` as Python holds it.
pub fn color(c: &Color) -> Value {
    match c {
        Color::Standard(s) => standard_color(s),
        Color::Pattern(name) => Value::str(name.clone()),
        Color::UncoloredPattern(base, name) => Value::Tuple(vec![standard_color(base), Value::str(name.clone())]),
    }
}

/// `Named("PDFGraphicState", state_fields(vars(gs)))`
pub fn graphic_state(gs: &PDFGraphicState) -> Value {
    let dash = match &gs.dash {
        None => Value::None,
        Some((dash, phase)) => Value::Tuple(vec![Value::from(dash), Value::from(phase)]),
    };
    named(
        "PDFGraphicState",
        vec![
            ("linewidth", num(&gs.linewidth)),
            ("linecap", Value::from(&gs.linecap)),
            ("linejoin", Value::from(&gs.linejoin)),
            ("miterlimit", Value::from(&gs.miterlimit)),
            ("dash", dash),
            ("intent", Value::from(&gs.intent)),
            ("flatness", Value::from(&gs.flatness)),
            ("scolor", color(&gs.scolor)),
            ("scs", colorspace(&gs.scs)),
            ("ncolor", color(&gs.ncolor)),
            ("ncs", colorspace(&gs.ncs)),
        ],
    )
}

/// A sequence handed to `begin_figure`: a list or a tuple.
fn obj_seq(s: &ObjSeq) -> Value {
    match s {
        ObjSeq::List(l) => Value::from(&PDFObject::List(l.clone())),
        ObjSeq::Tuple(t) => Value::Tuple(t.iter().map(Value::from).collect()),
    }
}

/// `list(path)`: the segments as tuples `(op, *coords)`.
fn path_value(path: &[PathSegment]) -> Value {
    Value::List(
        path.iter()
            .map(|(op, coords)| {
                let mut items = vec![Value::str(op.to_string())];
                items.extend(coords.iter().map(num));
                Value::Tuple(items)
            })
            .collect(),
    )
}

/// An optional operand (`None` when the interpreter passed none).
fn opt_obj(o: &Option<PDFObject>) -> Value {
    o.as_ref().map_or(Value::None, Value::from)
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

/// `interp._Page`: the formatter of one page's calls.
pub struct PageFormatter<'a> {
    fonts: &'a mut Fonts,
    /// The lines of the page.
    pub lines: Vec<String>,
    /// The names the interpreter gave the page's inline images.
    inline_names: HashSet<String>,
    inline: HashMap<String, usize>,
    gstate: Option<String>,
    char_args: Option<String>,
}

impl<'a> PageFormatter<'a> {
    /// The formatter of a page whose interpretation named `inline_images`
    /// inline images.
    pub fn new(fonts: &'a mut Fonts, inline_images: usize) -> PageFormatter<'a> {
        PageFormatter {
            fonts,
            lines: Vec::new(),
            inline_names: (0..inline_images).map(inline_image_name).collect(),
            inline: HashMap::new(),
            gstate: None,
            char_args: None,
        }
    }

    /// `font{"n": n}`, after the font's `font` line if it is new.
    fn font(&mut self, font: &Rc<PDFFont>) -> Value {
        let (n, new) = self.fonts.number(font);
        if new {
            let values = [Value::Int(PyInt::Small(n as i64)), Value::str(font.class_name()), Value::str(font.repr())];
            self.lines.push(line("font", &values));
        }
        named("font", vec![("n", Value::Int(PyInt::Small(n as i64)))])
    }

    /// A figure's or image's name: `inline{"n": k}` for an inline image.
    fn name(&mut self, name: &str) -> Value {
        if !self.inline_names.contains(name) {
            return Value::str(name.to_owned());
        }
        let k = self.inline.len();
        let k = *self.inline.entry(name.to_owned()).or_insert(k);
        named("inline", vec![("n", Value::Int(PyInt::Small(k as i64)))])
    }

    /// The `gstate` line, when the state differs from the last one written.
    fn graphic_state(&mut self, gs: &PDFGraphicState) {
        let text = canon(&graphic_state(gs));
        if self.gstate.as_deref() != Some(text.as_str()) {
            self.lines.push(format!("gstate {text}"));
            self.gstate = Some(text);
        }
    }

    /// The lines of one call.
    pub fn call(&mut self, c: &Call) {
        match c {
            Call::SetCtm(ctm) => self.lines.push(line("set_ctm", &[nums(ctm)])),
            Call::BeginPage(pageid, ctm) => self.lines.push(line("begin_page", &[Value::from(pageid), nums(ctm)])),
            Call::EndPage(pageid) => self.lines.push(line("end_page", &[Value::from(pageid)])),
            Call::EndString(linematrix) => self.lines.push(line("end_string", &[nums(linematrix)])),
            Call::BeginTag(tag, props) => {
                self.lines.push(line("begin_tag", &[Value::from(&PDFObject::Literal(*tag)), opt_obj(props)]))
            }
            Call::DoTag(tag, props) => {
                self.lines.push(line("do_tag", &[Value::from(&PDFObject::Literal(*tag)), opt_obj(props)]))
            }
            Call::EndTag => self.lines.push("end_tag".to_owned()),
            Call::BeginFigure(name, bbox, matrix) => {
                let name = self.name(name);
                self.lines.push(line("begin_figure", &[name, obj_seq(bbox), obj_seq(matrix)]));
            }
            Call::EndFigure(name) => {
                let name = self.name(name);
                self.lines.push(line("end_figure", &[name]));
            }
            Call::RenderImage(name, stream) => {
                let name = self.name(name);
                self.lines.push(line("render_image", &[name, Value::from(&PDFObject::Stream(stream.clone()))]));
            }
            Call::PaintPath { gstate, stroke, fill, evenodd, path } => {
                self.graphic_state(gstate);
                let values = [Value::Bool(*stroke), Value::Bool(*fill), Value::Bool(*evenodd), path_value(path)];
                self.lines.push(line("paint_path", &values));
            }
            Call::RenderString { textstate, seq, ncs, gstate } => {
                let ts = &**textstate;
                let font = match &ts.font {
                    Some(f) => self.font(f),
                    None => Value::None,
                };
                let textstate = named(
                    "PDFTextState",
                    vec![
                        ("font", font),
                        ("fontsize", num(&ts.fontsize)),
                        ("charspace", num(&ts.charspace)),
                        ("wordspace", num(&ts.wordspace)),
                        ("scaling", num(&ts.scaling)),
                        ("leading", num(&ts.leading)),
                        ("render", Value::Int(ts.render.clone())),
                        ("rise", num(&ts.rise)),
                        ("matrix", nums(&ts.matrix)),
                        ("linematrix", nums(&ts.linematrix)),
                    ],
                );
                self.graphic_state(gstate);
                self.lines.push(line("render_string", &[textstate, Value::from(seq), colorspace(ncs)]));
            }
            Call::RenderChar { matrix, font, fontsize, scaling, rise, cid, ncs, gstate, result } => {
                let font = self.font(font);
                let args = [font, num(fontsize), num(scaling), num(rise), colorspace(ncs)]
                    .iter()
                    .map(canon)
                    .collect::<Vec<_>>()
                    .join(" ");
                self.graphic_state(gstate);
                if self.char_args.as_deref() != Some(args.as_str()) {
                    self.lines.push(format!("char_args {args}"));
                    self.char_args = Some(args);
                }
                let result = match result {
                    Ok(adv) => num(adv),
                    Err(e) => Value::from(e),
                };
                self.lines.push(line("render_char", &[nums(matrix), Value::Int(cid.clone()), result]));
            }
            Call::Raised(e) => self.lines.push(format!("raised {}", canon_exc(e))),
        }
    }
}

/// The runner of `interp` and `interp_text`: `make` builds the page's
/// device, `calls` takes its records.
pub fn run_with<D: PDFDevice + 'static>(
    input: &StageInput<'_>,
    out: &mut dyn Write,
    make: impl Fn(&Rc<PDFResourceManager>, PyInt) -> PyResult<D>,
    calls: impl Fn(&mut D) -> Vec<Call>,
) -> io::Result<()> {
    run_with_page_frames(input, out, make, calls, 0)
}

/// [`run_with`], each page interpreted (`page_layout`) `page_frames`
/// frames deeper than the stage does: the depth sweeps of
/// `oracle/unit/interp_deep.py` (`down`).
pub fn run_with_page_frames<D: PDFDevice + 'static>(
    input: &StageInput<'_>,
    out: &mut dyn Write,
    make: impl Fn(&Rc<PDFResourceManager>, PyInt) -> PyResult<D>,
    calls: impl Fn(&mut D) -> Vec<Call>,
    page_frames: u32,
) -> io::Result<()> {
    terminated(out, |lines: &mut Lines| {
        let doc = open_document(input)?;
        let rsrcmgr = Rc::new(PDFResourceManager::new(true));
        let mut fonts = Fonts::default();
        for (pageno, page) in PDFPage::create_pages(&doc).enumerate() {
            let pageno = pageno + 1;
            let page = page?;
            lines.line("page", &[&pageno.to_string(), &canon_obj(&page.pageid)]);
            let pageno_int = PyInt::Small(pageno as i64);
            let (device, interpreter, error) = {
                let _deeper = if page_frames > 0 { Some(py_frames(page_frames)?) } else { None };
                page_layout(&rsrcmgr, &page, || make(&rsrcmgr, pageno_int))?
            };
            let records = calls(&mut device.borrow_mut());
            let mut fmt = PageFormatter::new(&mut fonts, interpreter.inline_images.get());
            for c in &records {
                fmt.call(c);
            }
            for l in &fmt.lines {
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
        |rsrcmgr, pageno| Ok(TracingDevice::new(rsrcmgr.clone(), pageno)),
        |d| std::mem::take(&mut d.calls),
    )
}
