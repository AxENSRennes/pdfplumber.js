//! Port of `pdfminer/pdfdevice.py`: the device interface the interpreter
//! drives (`PDFDevice`) and the text device that turns a shown string into
//! characters (`PDFTextDevice`).
//!
//! The signatures are the contract between the interpreter and the devices
//! (`docs/architecture.md`, "Classes, Overrides And super()").
//!
//! Not ported (out of scope, pdfplumber never uses them): `TagExtractor`
//! (an XML-like dump of the tags), `PDFDevice.__repr__`, and the context
//! manager methods `__enter__`/`__exit__` (only `close` is kept).
//!
//! # Classes, overrides and `super()`
//!
//! A Python device class is a struct holding its attributes (and those of
//! its bases) plus an implementation of [`PDFDevice`] (and of
//! [`PDFTextDevice`] for a text device). Python's dynamic dispatch is
//! kept: upstream methods call other methods on `self`
//! (`PDFTextDevice.render_string_horizontal` calls `self.render_char`,
//! `PDFLayoutAnalyzer.paint_path` calls `self.paint_path` for each
//! subpath), and these calls reach the most derived override. So:
//!
//! - every method is a trait method whose default is the base class's body;
//! - each class's body of a method is also a free generic function of the
//!   module named after the class ([`device`], [`text_device`],
//!   `converter::layout_analyzer`, `converter::page_aggregator`,
//!   `pdfplumber::page::marked_content`), taking `dev: &mut D` for `self`:
//!   an override calls it as Python calls `super().method(...)`, and its own
//!   calls on `dev` dispatch to the most derived class;
//! - a subclass that does not override a method delegates to its nearest
//!   base's free function. Rust has no inheritance between trait defaults,
//!   so a type implementing [`PDFTextDevice`] writes its
//!   [`PDFDevice::render_string`] as `text_device::render_string(self, ...)`.
//!
//! A device accesses its attributes through accessor traits
//! ([`PDFDevice::attrs`] here, `converter::LayoutAnalyzer::analyzer` for
//! the analyzer's), so the free functions work on any subclass, a tracing
//! device of `pdftrace` included.
//!
//! # Values
//!
//! `PDFTextDevice` runs Python's arithmetic on the text state's numbers
//! ([`Num`]: ints until an operator sets a float) and on the items of the
//! shown sequence, which nothing checked: `TJ`'s operand is any object
//! ([`PDFTextSeq`]), iterated as Python iterates it ([`text_seq_items`]).
//!
//! # Recursion depth
//!
//! Each method takes its Python frame (`pycompat::recursion::py_frame`) for
//! its duration, in the free function of its class (the methods of
//! `PDFDevice`, which only `pass` or assign, check theirs with `leaf(1)`); a
//! call to a free function standing for `super().m(...)` is a frame of `m`,
//! taken by the function. The `pdfminer.utils` functions called here
//! (`mult_matrix`, `translate_matrix`) and the font's `is_multibyte` and
//! `is_vertical` are Python functions that do not count their frame in the
//! port: their callers check it (`leaf(1)`). `logger.warning` of
//! `render_string_horizontal` and `render_string_vertical` take their
//! frames through `pycompat::logging` (`Logger::new("pdfminer.pdfdevice")`).
//! `font.decode(obj)` of a CID font is a generator (`CMap.decode`) resumed
//! by the `for` loop: `pdffont` takes the frames of its first step where the
//! loop resumes it; the later resumptions take a frame at the depth of the
//! loop's, shallower than that first step (which logs), so they can never
//! raise where it did not, and are not taken. See `pdfinterp` for the
//! interpreter's recursion.

use std::rc::Rc;

use pycompat::logging::Logger;
use pycompat::recursion::{c_called_leaf, leaf, py_frame};
use pycompat::{exc, Num, PyInt, PyResult};

use crate::pdfcolor::PDFColorSpace;
use crate::pdffont::PDFFont;
use crate::pdfinterp::{PDFGraphicState, PDFResourceManager, PDFStackT, PDFTextState};
use crate::pdfpage::PDFPage;
use crate::pdftypes::{PDFList, PDFObject, PDFStream};
use crate::psparser::PSLiteral;
use crate::utils::{mult_matrix, translate_matrix, Matrix, PathSegment, Point};

/// `logger = logging.getLogger(__name__)`
static LOG: Logger = Logger::new("pdfminer.pdfdevice");

// pdfminer/pdfdevice.py:27 PDFTextSeq
/// `PDFTextSeq`: what `render_string` iterates. The interpreter does not
/// check it: the operand of `TJ` (any object: a list in practice, but a
/// number, a `bytes`, a dict or a stream are iterated as Python iterates
/// them, or raise `TypeError`/`KeyError`), or the new list `[s]` of `Tj`,
/// `'` and `"`.
pub type PDFTextSeq = PDFObject;

/// An item of a [`PDFTextSeq`], as `for obj in seq` gives it: a PDF object
/// (an item of a list; a `bytes` gives its byte values, ints), or a `str`
/// key of a dict (neither a number nor `bytes`).
#[derive(Clone, Debug)]
pub enum SeqItem {
    Obj(PDFObject),
    Str(Rc<str>),
}

/// The items `for obj in seq` (or `list(seq)`) gets from a sequence of
/// any type, or the exception Python raises: a list gives its items, a
/// `bytes` its byte values, a dict its keys; a stream is iterated through
/// `PDFStream.__getitem__(0)` (a Python method called from C: the
/// evaluation loop entry and its frame), whose `PDFKeyError` propagates;
/// anything else is not iterable (`TypeError`). Nothing can raise once the
/// first item is had, so the items are collected at once.
pub fn text_seq_items(seq: &PDFTextSeq) -> PyResult<Vec<SeqItem>> {
    match seq {
        PDFObject::List(l) => Ok(l.to_vec().into_iter().map(SeqItem::Obj).collect()),
        PDFObject::Bytes(b) => {
            Ok(b.iter().map(|&c| SeqItem::Obj(PDFObject::Int(PyInt::Small(i64::from(c))))).collect())
        }
        PDFObject::Dict(d) => Ok(d.keys().into_iter().map(SeqItem::Str).collect()),
        PDFObject::Stream(s) => {
            c_called_leaf()?;
            Err(s.missing_attribute("0"))
        }
        other => Err(exc::TypeError(format!("'{}' object is not iterable", other.type_name()))),
    }
}

/// A sequence the interpreter hands to `begin_figure` unchecked: the
/// result of `list_value(...)` on an XObject's `/BBox` or `/Matrix` (the
/// PDF's own list, or `[]` for a value that is not a list), or one of the
/// interpreter's tuples (`(0, 0, 1, 1)`, `MATRIX_IDENTITY`, and
/// `MATRIX_IDENTITY` again when a form has no `/Matrix`). A list and a
/// tuple differ in the traces (`[..]`, `(..)`) and in Python's
/// operators.
#[derive(Clone)]
pub enum ObjSeq {
    List(PDFList),
    Tuple(Vec<PDFObject>),
}

impl ObjSeq {
    /// The items.
    pub fn items(&self) -> Vec<PDFObject> {
        match self {
            ObjSeq::List(l) => l.to_vec(),
            ObjSeq::Tuple(t) => t.clone(),
        }
    }
}

/// The attributes `PDFDevice.__init__` sets.
pub struct PDFDeviceAttrs {
    /// `self.rsrcmgr`
    pub rsrcmgr: Rc<PDFResourceManager>,
    /// `self.ctm`: `None` until the interpreter's first `set_ctm`.
    pub ctm: Option<Matrix>,
}

impl PDFDeviceAttrs {
    // pdfminer/pdfdevice.py:35 PDFDevice.__init__
    /// The attributes (the frame of `__init__` is its caller's to count:
    /// [`device::init`]).
    pub fn new(rsrcmgr: Rc<PDFResourceManager>) -> PDFDeviceAttrs {
        PDFDeviceAttrs { rsrcmgr, ctm: None }
    }
}

/// pdfminer/pdfdevice.py:32 `PDFDevice`: translates the output of
/// `PDFPageInterpreter` to the output that is needed. Every method returns
/// `PyResult` (a device may raise, and every call takes its frame).
///
/// The interpreter holds its device as `Rc<RefCell<dyn PDFDevice>>`,
/// shared with the sub-interpreters of Form XObjects
/// (pdfminer/pdfinterp.py:381 `PDFPageInterpreter.dup`); no device method
/// calls back into the interpreter, so a call never finds the device
/// borrowed.
pub trait PDFDevice {
    /// The attributes of `PDFDevice.__init__`.
    fn attrs(&self) -> &PDFDeviceAttrs;
    fn attrs_mut(&mut self) -> &mut PDFDeviceAttrs;

    // pdfminer/pdfdevice.py:48 PDFDevice.close
    fn close(&mut self) -> PyResult<()> {
        device::close(self)
    }

    // pdfminer/pdfdevice.py:51 PDFDevice.set_ctm
    fn set_ctm(&mut self, ctm: Matrix) -> PyResult<()> {
        device::set_ctm(self, ctm)
    }

    // pdfminer/pdfdevice.py:54 PDFDevice.begin_tag
    /// `props` is `None` when the interpreter passes none (`BMC`), the
    /// unchecked operand otherwise (`BDC`: a dict, or a name of the
    /// `/Properties` resources, which pdfminer does not resolve).
    fn begin_tag(&mut self, tag: &PSLiteral, props: Option<&PDFStackT>) -> PyResult<()> {
        device::begin_tag(self, tag, props)
    }

    // pdfminer/pdfdevice.py:57 PDFDevice.end_tag
    fn end_tag(&mut self) -> PyResult<()> {
        device::end_tag(self)
    }

    // pdfminer/pdfdevice.py:60 PDFDevice.do_tag
    fn do_tag(&mut self, tag: &PSLiteral, props: Option<&PDFStackT>) -> PyResult<()> {
        device::do_tag(self, tag, props)
    }

    // pdfminer/pdfdevice.py:63 PDFDevice.begin_page
    fn begin_page(&mut self, page: &PDFPage, ctm: &Matrix) -> PyResult<()> {
        device::begin_page(self, page, ctm)
    }

    // pdfminer/pdfdevice.py:66 PDFDevice.end_page
    fn end_page(&mut self, page: &PDFPage) -> PyResult<()> {
        device::end_page(self, page)
    }

    // pdfminer/pdfdevice.py:69 PDFDevice.begin_figure
    /// `name` is the XObject's name (`literal_name` of the `Do` operand),
    /// or the deterministic name the port gives an inline image
    /// (`pdfinterp`, `do_EI`).
    fn begin_figure(&mut self, name: &str, bbox: &ObjSeq, matrix: &ObjSeq) -> PyResult<()> {
        device::begin_figure(self, name, bbox, matrix)
    }

    // pdfminer/pdfdevice.py:72 PDFDevice.end_figure
    fn end_figure(&mut self, name: &str) -> PyResult<()> {
        device::end_figure(self, name)
    }

    // pdfminer/pdfdevice.py:75 PDFDevice.paint_path
    /// `graphicstate` is the interpreter's live state (not a copy: a device
    /// that keeps values copies them), `path` its current path (or, from
    /// `PDFLayoutAnalyzer.paint_path`, a slice of it: one subpath).
    fn paint_path(
        &mut self,
        graphicstate: &PDFGraphicState,
        stroke: bool,
        fill: bool,
        evenodd: bool,
        path: &[PathSegment],
    ) -> PyResult<()> {
        device::paint_path(self, graphicstate, stroke, fill, evenodd, path)
    }

    // pdfminer/pdfdevice.py:85 PDFDevice.render_image
    fn render_image(&mut self, name: &str, stream: &Rc<PDFStream>) -> PyResult<()> {
        device::render_image(self, name, stream)
    }

    // pdfminer/pdfdevice.py:88 PDFDevice.render_string
    /// `textstate` is the interpreter's live text state (`render_string` of
    /// a text device sets its `linematrix`); `ncs` the non-stroking color
    /// space; `graphicstate` the copy `do_TJ` made for this string, which
    /// every char of the string keeps (one `Rc`, shared as Python shares the
    /// object).
    fn render_string(
        &mut self,
        textstate: &mut PDFTextState,
        seq: &PDFTextSeq,
        ncs: &Rc<PDFColorSpace>,
        graphicstate: &Rc<PDFGraphicState>,
    ) -> PyResult<()> {
        device::render_string(self, textstate, seq, ncs, graphicstate)
    }
}

/// The bodies of `PDFDevice`'s methods (`super()` for a direct subclass):
/// `set_ctm` sets `self.ctm`, the others only `pass` (each only checks its
/// frame: nothing is called under it).
#[allow(unused_variables)] // the methods that only `pass`
pub mod device {
    use super::*;

    // pdfminer/pdfdevice.py:35 PDFDevice.__init__
    /// The frames of `PDFDevice.__init__(self, rsrcmgr)` called by a
    /// subclass's `__init__` (only checked: it assigns and returns).
    pub fn init(rsrcmgr: Rc<PDFResourceManager>) -> PyResult<PDFDeviceAttrs> {
        leaf(1)?;
        Ok(PDFDeviceAttrs::new(rsrcmgr))
    }

    // pdfminer/pdfdevice.py:48 PDFDevice.close
    pub fn close<D: PDFDevice + ?Sized>(dev: &mut D) -> PyResult<()> {
        leaf(1)
    }

    // pdfminer/pdfdevice.py:51 PDFDevice.set_ctm
    pub fn set_ctm<D: PDFDevice + ?Sized>(dev: &mut D, ctm: Matrix) -> PyResult<()> {
        leaf(1)?;
        dev.attrs_mut().ctm = Some(ctm);
        Ok(())
    }

    // pdfminer/pdfdevice.py:54 PDFDevice.begin_tag
    pub fn begin_tag<D: PDFDevice + ?Sized>(dev: &mut D, tag: &PSLiteral, props: Option<&PDFStackT>) -> PyResult<()> {
        leaf(1)
    }

    // pdfminer/pdfdevice.py:57 PDFDevice.end_tag
    pub fn end_tag<D: PDFDevice + ?Sized>(dev: &mut D) -> PyResult<()> {
        leaf(1)
    }

    // pdfminer/pdfdevice.py:60 PDFDevice.do_tag
    pub fn do_tag<D: PDFDevice + ?Sized>(dev: &mut D, tag: &PSLiteral, props: Option<&PDFStackT>) -> PyResult<()> {
        leaf(1)
    }

    // pdfminer/pdfdevice.py:63 PDFDevice.begin_page
    pub fn begin_page<D: PDFDevice + ?Sized>(dev: &mut D, page: &PDFPage, ctm: &Matrix) -> PyResult<()> {
        leaf(1)
    }

    // pdfminer/pdfdevice.py:66 PDFDevice.end_page
    pub fn end_page<D: PDFDevice + ?Sized>(dev: &mut D, page: &PDFPage) -> PyResult<()> {
        leaf(1)
    }

    // pdfminer/pdfdevice.py:69 PDFDevice.begin_figure
    pub fn begin_figure<D: PDFDevice + ?Sized>(
        dev: &mut D,
        name: &str,
        bbox: &ObjSeq,
        matrix: &ObjSeq,
    ) -> PyResult<()> {
        leaf(1)
    }

    // pdfminer/pdfdevice.py:72 PDFDevice.end_figure
    pub fn end_figure<D: PDFDevice + ?Sized>(dev: &mut D, name: &str) -> PyResult<()> {
        leaf(1)
    }

    // pdfminer/pdfdevice.py:75 PDFDevice.paint_path
    pub fn paint_path<D: PDFDevice + ?Sized>(
        dev: &mut D,
        graphicstate: &PDFGraphicState,
        stroke: bool,
        fill: bool,
        evenodd: bool,
        path: &[PathSegment],
    ) -> PyResult<()> {
        leaf(1)
    }

    // pdfminer/pdfdevice.py:85 PDFDevice.render_image
    pub fn render_image<D: PDFDevice + ?Sized>(dev: &mut D, name: &str, stream: &Rc<PDFStream>) -> PyResult<()> {
        leaf(1)
    }

    // pdfminer/pdfdevice.py:88 PDFDevice.render_string
    pub fn render_string<D: PDFDevice + ?Sized>(
        dev: &mut D,
        textstate: &mut PDFTextState,
        seq: &PDFTextSeq,
        ncs: &Rc<PDFColorSpace>,
        graphicstate: &Rc<PDFGraphicState>,
    ) -> PyResult<()> {
        leaf(1)
    }
}

/// pdfminer/pdfdevice.py:98 `PDFTextDevice`: a device that shows strings
/// as characters, one `render_char` call per code of the font's `decode`.
/// Its implementors also write [`PDFDevice::render_string`] as
/// [`text_device::render_string`]`(self, ...)`.
///
/// Values: `fontsize`, `charspace`, `wordspace`, `rise` are the text
/// state's numbers (an `int` before any `Tf`/`Tc`/`Tw`/`Ts`, a `float`
/// after); `scaling` and `dxscale` are floats; `wordspace` is the int `0`
/// for a multibyte font; `cid` is an item of `font.decode(obj)`.
pub trait PDFTextDevice: PDFDevice {
    // pdfminer/pdfdevice.py:149 PDFTextDevice.render_string_horizontal
    /// The new `linematrix`: `(x, y)` after the string.
    fn render_string_horizontal(
        &mut self,
        seq: &PDFTextSeq,
        matrix: &Matrix,
        pos: &Point,
        font: &Rc<PDFFont>,
        fontsize: &Num,
        scaling: &Num,
        charspace: &Num,
        wordspace: &Num,
        rise: &Num,
        dxscale: &Num,
        ncs: &Rc<PDFColorSpace>,
        graphicstate: &Rc<PDFGraphicState>,
    ) -> PyResult<Point> {
        text_device::render_string_horizontal(
            self,
            seq,
            matrix,
            pos,
            font,
            fontsize,
            scaling,
            charspace,
            wordspace,
            rise,
            dxscale,
            ncs,
            graphicstate,
        )
    }

    // pdfminer/pdfdevice.py:195 PDFTextDevice.render_string_vertical
    fn render_string_vertical(
        &mut self,
        seq: &PDFTextSeq,
        matrix: &Matrix,
        pos: &Point,
        font: &Rc<PDFFont>,
        fontsize: &Num,
        scaling: &Num,
        charspace: &Num,
        wordspace: &Num,
        rise: &Num,
        dxscale: &Num,
        ncs: &Rc<PDFColorSpace>,
        graphicstate: &Rc<PDFGraphicState>,
    ) -> PyResult<Point> {
        text_device::render_string_vertical(
            self,
            seq,
            matrix,
            pos,
            font,
            fontsize,
            scaling,
            charspace,
            wordspace,
            rise,
            dxscale,
            ncs,
            graphicstate,
        )
    }

    // pdfminer/pdfdevice.py:241 PDFTextDevice.render_char
    /// The advance the string's position moves by (`0`, an int, here; the
    /// char's `adv` for `PDFLayoutAnalyzer`). `matrix` is
    /// `translate_matrix(matrix, (x, y))`, a new tuple per char.
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
        text_device::render_char(self, matrix, font, fontsize, scaling, rise, cid, ncs, graphicstate)
    }
}

/// The bodies of `PDFTextDevice`'s methods.
pub mod text_device {
    use super::*;

    /// `0.01`, `0.001`, `0`
    fn float(x: f64) -> Num {
        Num::Float(x)
    }

    // pdfminer/pdfdevice.py:99 PDFTextDevice.render_string
    /// `matrix = mult_matrix(textstate.matrix, self.ctm)`, the font's
    /// scales, then `render_string_vertical` or `render_string_horizontal`
    /// (on `dev`: overridable), whose result becomes
    /// `textstate.linematrix`. Asserts `self.ctm` and `textstate.font`.
    pub fn render_string<D: PDFTextDevice + ?Sized>(
        dev: &mut D,
        textstate: &mut PDFTextState,
        seq: &PDFTextSeq,
        ncs: &Rc<PDFColorSpace>,
        graphicstate: &Rc<PDFGraphicState>,
    ) -> PyResult<()> {
        let _frame = py_frame()?;
        let Some(ctm) = dev.attrs().ctm.clone() else {
            return Err(exc::AssertionError(""));
        };
        // utils.mult_matrix: a Python function
        leaf(1)?;
        let matrix = mult_matrix(&textstate.matrix, &ctm)?;
        let font = textstate.font.clone();
        let fontsize = textstate.fontsize.clone();
        let scaling = textstate.scaling.mul(&float(0.01))?;
        let charspace = textstate.charspace.mul(&scaling)?;
        let mut wordspace = textstate.wordspace.mul(&scaling)?;
        let rise = textstate.rise.clone();
        let Some(font) = font else {
            return Err(exc::AssertionError(""));
        };
        // font.is_multibyte(): its frame
        leaf(1)?;
        if font.is_multibyte() {
            wordspace = Num::Int(PyInt::ZERO);
        }
        let dxscale = float(0.001).mul(&fontsize)?.mul(&scaling)?;
        // font.is_vertical(): its frame
        leaf(1)?;
        let pos = textstate.linematrix.clone();
        textstate.linematrix = if font.is_vertical() {
            dev.render_string_vertical(
                seq,
                &matrix,
                &pos,
                &font,
                &fontsize,
                &scaling,
                &charspace,
                &wordspace,
                &rise,
                &dxscale,
                ncs,
                graphicstate,
            )?
        } else {
            dev.render_string_horizontal(
                seq,
                &matrix,
                &pos,
                &font,
                &fontsize,
                &scaling,
                &charspace,
                &wordspace,
                &rise,
                &dxscale,
                ncs,
                graphicstate,
            )?
        };
        Ok(())
    }

    /// The axis a string advances along.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Axis {
        Horizontal,
        Vertical,
    }

    /// The loop of `render_string_horizontal` (moving `x`) and
    /// `render_string_vertical` (moving `y`), which are the same code but
    /// for the coordinate and the warning's text; the caller took the
    /// method's frame.
    #[allow(clippy::too_many_arguments)]
    fn render_string_along<D: PDFTextDevice + ?Sized>(
        dev: &mut D,
        axis: Axis,
        seq: &PDFTextSeq,
        matrix: &Matrix,
        pos: &Point,
        font: &Rc<PDFFont>,
        fontsize: &Num,
        scaling: &Num,
        charspace: &Num,
        wordspace: &Num,
        rise: &Num,
        dxscale: &Num,
        ncs: &Rc<PDFColorSpace>,
        graphicstate: &Rc<PDFGraphicState>,
    ) -> PyResult<Point> {
        let [mut x, mut y] = pos.clone();
        let mut needcharspace = false;
        for obj in text_seq_items(seq)? {
            match obj {
                SeqItem::Obj(PDFObject::Bytes(b)) => {
                    for cid in font.decode(&b)? {
                        if needcharspace {
                            let c = coord(axis, &mut x, &mut y);
                            *c = c.add(charspace)?;
                        }
                        // utils.translate_matrix: a Python function
                        leaf(1)?;
                        let char_matrix = translate_matrix(matrix, &[x.clone(), y.clone()])?;
                        let adv =
                            dev.render_char(char_matrix, font, fontsize, scaling, rise, &cid, ncs, graphicstate)?;
                        let c = coord(axis, &mut x, &mut y);
                        *c = c.add(&adv)?;
                        if cid == PyInt::Small(32) && is_truthy(wordspace) {
                            *c = c.add(wordspace)?;
                        }
                        needcharspace = true;
                    }
                }
                SeqItem::Obj(o) if o.as_num().is_some() => {
                    let shift = o.as_num().expect("a number").mul(dxscale)?;
                    let c = coord(axis, &mut x, &mut y);
                    *c = c.sub(&shift)?;
                    needcharspace = true;
                }
                other => {
                    LOG.warning_with(|| {
                        let direction = match axis {
                            Axis::Horizontal => "horizontal",
                            Axis::Vertical => "vertical",
                        };
                        let obj = match &other {
                            SeqItem::Obj(o) => o.repr()?,
                            SeqItem::Str(s) => pycompat::repr::str_repr(s),
                        };
                        Ok(format!(
                            "Cannot render {direction} string because {obj} is not a valid int, float or bytes."
                        ))
                    })?;
                }
            }
        }
        Ok([x, y])
    }

    /// The coordinate a string advances along: `x`, or `y`.
    fn coord<'a>(axis: Axis, x: &'a mut Num, y: &'a mut Num) -> &'a mut Num {
        match axis {
            Axis::Horizontal => x,
            Axis::Vertical => y,
        }
    }

    /// `bool(x)` of a number: a NaN is true.
    fn is_truthy(x: &Num) -> bool {
        match x {
            Num::Int(i) => !i.is_zero(),
            Num::Float(f) => *f != 0.0,
        }
    }

    // pdfminer/pdfdevice.py:149 PDFTextDevice.render_string_horizontal
    pub fn render_string_horizontal<D: PDFTextDevice + ?Sized>(
        dev: &mut D,
        seq: &PDFTextSeq,
        matrix: &Matrix,
        pos: &Point,
        font: &Rc<PDFFont>,
        fontsize: &Num,
        scaling: &Num,
        charspace: &Num,
        wordspace: &Num,
        rise: &Num,
        dxscale: &Num,
        ncs: &Rc<PDFColorSpace>,
        graphicstate: &Rc<PDFGraphicState>,
    ) -> PyResult<Point> {
        let _frame = py_frame()?;
        render_string_along(
            dev,
            Axis::Horizontal,
            seq,
            matrix,
            pos,
            font,
            fontsize,
            scaling,
            charspace,
            wordspace,
            rise,
            dxscale,
            ncs,
            graphicstate,
        )
    }

    // pdfminer/pdfdevice.py:195 PDFTextDevice.render_string_vertical
    pub fn render_string_vertical<D: PDFTextDevice + ?Sized>(
        dev: &mut D,
        seq: &PDFTextSeq,
        matrix: &Matrix,
        pos: &Point,
        font: &Rc<PDFFont>,
        fontsize: &Num,
        scaling: &Num,
        charspace: &Num,
        wordspace: &Num,
        rise: &Num,
        dxscale: &Num,
        ncs: &Rc<PDFColorSpace>,
        graphicstate: &Rc<PDFGraphicState>,
    ) -> PyResult<Point> {
        let _frame = py_frame()?;
        render_string_along(
            dev,
            Axis::Vertical,
            seq,
            matrix,
            pos,
            font,
            fontsize,
            scaling,
            charspace,
            wordspace,
            rise,
            dxscale,
            ncs,
            graphicstate,
        )
    }

    // pdfminer/pdfdevice.py:241 PDFTextDevice.render_char
    #[allow(unused_variables)]
    pub fn render_char<D: PDFTextDevice + ?Sized>(
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
        leaf(1)?;
        Ok(Num::Int(PyInt::ZERO))
    }
}
