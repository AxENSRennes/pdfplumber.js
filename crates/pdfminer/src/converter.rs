//! Port of `pdfminer/converter.py` (partial): the devices that build the
//! layout objects, `PDFLayoutAnalyzer` and `PDFPageAggregator`.
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
//! (`paint_path` for each subpath, `handle_undefined_char` from
//! `render_char`, `receive_layout` from `end_page`).
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
//! # Errors
//!
//! The interpreter never catches what a device raises (only `PSEOF` from
//! its parser): an exception of a device method ends `process_page`, and
//! pdfplumber drops the device with the interpreter (`Page.layout` raises
//! `PdfminerException`). So the device's state after an exception is never
//! read again, and the port does not reproduce it where it would cost a
//! copy: `begin_figure` pushes the current item once the figure is built
//! (Python pushes it first), `end_page` moves the page to `receive_layout`
//! (Python keeps it as `cur_item`; nothing calls the device after
//! `end_page`). The exception itself, and every frame taken before it, are
//! Python's.
//!
//! Calls the interpreter never makes (`render_image` outside a figure,
//! `end_figure` without `begin_figure`, `end_page` inside a figure, any
//! call before `begin_page`) raise Python's `AssertionError` (or the
//! `AttributeError` of a missing `cur_item`, whose message names
//! pdfplumber's class).
//!
//! # Analysis
//!
//! `end_page` analyzes the page when the device has `laparams`, with the
//! device's [`IdSource`] (`CreationOrder` by default, the library's and
//! JavaScript's; the `layout` stage and the unit replay give Python's ids,
//! see `layout.rs`, "Analysis and object identity").
//!
//! # Recursion depth
//!
//! Each method takes its frame; the layout objects are built by class
//! calls, whose frames `layout.rs` counts (its constructors), and
//! `self.cur_item.add(obj)` is `LTContainer.add`'s leaf frame. In
//! `paint_path`, `"".join(x[0] for x in path)` creates a generator (a frame
//! check) that `str.join` resumes from C once per segment and once more
//! (an evaluation loop entry and a frame each), `re.finditer` and its
//! `_compile` are two frames (more the first time a process compiles the
//! pattern: [`SUBPATH_COMPILE_DEPTH`], `pycompat::re_cache`), the list
//! comprehensions take no frame (PEP 709) but their calls of `typing.cast`
//! and `apply_matrix_pt` are leaves, and `self.paint_path(...)` of a
//! subpath recurses once (a subpath has one `m`). `log.debug` of
//! `handle_undefined_char` takes its frames through `pycompat::logging`
//! (`Logger::new("pdfminer.converter")`). A device's class call is
//! [`PDFPageAggregator::create`] (and pdfplumber's).

use std::cell::RefCell;
use std::rc::Rc;

use pycompat::logging::Logger;
use pycompat::recursion::{c_called_leaf, leaf, py_frame};
use pycompat::{exc, Num, PyErr, PyInt, PyResult, PyStr};

use crate::layout::{class_call, CreationOrder, IdSource, LAParams, LTCurve, LTFigure, LTImage, LTItem, LTPage, LTPayload};
use crate::layout::{apply_matrix_rect_counted, LTChar, OriginalPathSegment};
use crate::pdfcolor::PDFColorSpace;
use crate::pdfdevice::{text_device, ObjSeq, PDFDevice, PDFDeviceAttrs, PDFTextDevice, PDFTextSeq};
use crate::pdffont::{FontName, PDFFont};
use crate::pdfinterp::{Color, PDFGraphicState, PDFResourceManager};
use crate::pdfpage::PDFPage;
use crate::pdftypes::{PDFObject, PDFStream};
use crate::utils::{apply_matrix_pt, mult_matrix_obj, unpack, Matrix, PathSegment, Point};
use pycompat::re_cache;

/// `log = logging.getLogger(__name__)` of pdfminer/converter.py.
const LOG: Logger = Logger::new("pdfminer.converter");

/// The pattern `PDFLayoutAnalyzer.paint_path` splits a shape with.
pub const SUBPATH_PATTERN: &str = "m[^m]+";

/// How deep below `PDFLayoutAnalyzer.paint_path` the first
/// `re.finditer(r"m[^m]+", shape)` of a process goes, compiling the
/// pattern (`re.finditer` -> `_compile` -> `_compiler.compile` ->
/// `_parser.parse` -> `_parse_sub` -> `_parse` -> `get` -> `__next`),
/// measured on CPython 3.12 by `oracle/unit/converter.py` (its probe, and
/// the `depth` cases that cross the boundary there).
pub const SUBPATH_COMPILE_DEPTH: u32 = 8;

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

impl<P: LTPayload> CurItem<P> {
    /// `str(type(self.cur_item))`
    pub fn type_str(&self) -> &'static str {
        match self {
            CurItem::Page(_) => "<class 'pdfminer.layout.LTPage'>",
            CurItem::Figure(_) => "<class 'pdfminer.layout.LTFigure'>",
        }
    }

    /// `self.cur_item.add(obj)`, from Python code (`LTContainer.add`'s
    /// frame).
    pub fn add(&mut self, obj: LTItem<P>) -> PyResult<()> {
        match self {
            CurItem::Page(page) => page.layout.add_counted(obj),
            CurItem::Figure(fig) => fig.borrow_mut().layout.add_counted(obj),
        }
    }

    /// `self.cur_item._objs[-1]`, if any.
    pub fn last(&self) -> Option<LTItem<P>> {
        match self {
            CurItem::Page(page) => page.layout._objs.last().cloned(),
            CurItem::Figure(fig) => fig.borrow().layout._objs.last().cloned(),
        }
    }
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
    /// `self.cur_item`: `None` before `begin_page` (unset upstream), and
    /// after `end_page` (see the module documentation).
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

    // pdfminer/converter.py:65 PDFLayoutAnalyzer.__init__
    /// The frames of `PDFLayoutAnalyzer.__init__` called by a subclass's
    /// `__init__`: its own and `PDFDevice.__init__`'s (a leaf, through
    /// `PDFTextDevice.__init__`, which is `PDFDevice`'s).
    pub fn init_frames() -> PyResult<()> {
        let _frame = py_frame()?;
        leaf(1) // PDFDevice.__init__
    }

    /// `self.cur_item`, which upstream is unset before `begin_page`.
    fn cur_item(&mut self) -> PyResult<&mut CurItem<P>> {
        self.cur_item.as_mut().ok_or_else(no_cur_item)
    }
}

/// The `AttributeError` of `self.cur_item` before `begin_page`.
fn no_cur_item() -> PyErr {
    exc::AttributeError("'PDFPageAggregatorWithMarkedContent' object has no attribute 'cur_item'")
}

/// The `TypeError` of unpacking `self.ctm` while it is `None`.
fn unpack_none() -> PyErr {
    exc::TypeError("cannot unpack non-iterable NoneType object")
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
        let _frame = py_frame()?;
        let mediabox = page.mediabox.map(Num::Float);
        let [x0, y0, x1, y1] = apply_matrix_rect_counted(ctm, &mediabox)?;
        let zero = Num::Int(PyInt::ZERO);
        let mediabox = [zero.clone(), zero.clone(), x0.sub(&x1)?.abs(), y0.sub(&y1)?.abs()];
        let pageno = dev.analyzer().pageno.clone();
        let page = LTPage::new(pageno, mediabox, zero)?;
        dev.analyzer_mut().cur_item = Some(CurItem::Page(page));
        Ok(())
    }

    // pdfminer/converter.py:81 PDFLayoutAnalyzer.end_page
    /// Asserts (an empty stack, an `LTPage`), analyzes with `laparams` when
    /// set (with `ids`), increments `pageno`, calls `dev.receive_layout`.
    pub fn end_page<P: InterpPayload, D: LayoutAnalyzer<P> + ?Sized>(dev: &mut D, _page: &PDFPage) -> PyResult<()> {
        let _frame = py_frame()?;
        let a = dev.analyzer_mut();
        if !a._stack.is_empty() {
            return Err(exc::AssertionError(a._stack.len().to_string()));
        }
        let ltpage = match a.cur_item.as_mut() {
            Some(CurItem::Page(ltpage)) => ltpage,
            Some(other) => return Err(exc::AssertionError(other.type_str())),
            None => return Err(no_cur_item()),
        };
        if let Some(laparams) = &a.laparams {
            ltpage.analyze_with_ids(laparams, &mut *a.ids)?;
        }
        a.pageno = a.pageno.add(&PyInt::Small(1));
        let Some(CurItem::Page(ltpage)) = a.cur_item.take() else { unreachable!("checked above") };
        dev.receive_layout(ltpage)
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
        let _frame = py_frame()?;
        let a = dev.analyzer_mut();
        // self._stack.append(self.cur_item): pushed below, once the figure
        // is built (see the module documentation, "Errors")
        a.cur_item()?;
        let matrix = {
            let _frame = py_frame()?; // mult_matrix
            let m1 = matrix.items();
            match a.device.ctm.as_ref() {
                Some(ctm) => mult_matrix_obj(&m1, ctm)?,
                None => {
                    unpack::<_, 6>(&m1)?;
                    return Err(unpack_none());
                }
            }
        };
        let figure = LTFigure::new_obj(name.to_owned(), &bbox.items(), &matrix)?;
        let parent = a.cur_item.replace(CurItem::Figure(Rc::new(RefCell::new(figure)))).expect("checked above");
        a._stack.push(parent);
        Ok(())
    }

    // pdfminer/converter.py:93 PDFLayoutAnalyzer.end_figure
    pub fn end_figure<P: InterpPayload, D: LayoutAnalyzer<P> + ?Sized>(dev: &mut D, _name: &str) -> PyResult<()> {
        let _frame = py_frame()?;
        let a = dev.analyzer_mut();
        let fig = match a.cur_item()? {
            CurItem::Figure(fig) => fig.clone(),
            other => return Err(exc::AssertionError(other.type_str())),
        };
        let Some(parent) = a._stack.pop() else {
            return Err(exc::IndexError("pop from empty list"));
        };
        a.cur_item = Some(parent);
        a.cur_item()?.add(LTItem::Figure(fig))
    }

    // pdfminer/converter.py:99 PDFLayoutAnalyzer.render_image
    /// Asserts a figure; adds `LTImage(name, stream, figure's bbox)`.
    pub fn render_image<P: InterpPayload, D: LayoutAnalyzer<P> + ?Sized>(
        dev: &mut D,
        name: &str,
        stream: &Rc<PDFStream>,
    ) -> PyResult<()> {
        let _frame = py_frame()?;
        let item = dev.analyzer_mut().cur_item()?;
        let bbox = match &*item {
            CurItem::Figure(fig) => fig.borrow().layout.comp.bbox(),
            other => return Err(exc::AssertionError(other.type_str())),
        };
        let image = LTImage::new(name.to_owned(), stream.clone(), bbox)?;
        item.add(LTItem::Image(Rc::new(image)))
    }

    // pdfminer/converter.py:108 PDFLayoutAnalyzer.paint_path
    /// Paint paths described in section 4.4 of the PDF reference manual.
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
        let _frame = py_frame()?;
        // "".join(x[0] for x in path): the generator is created, then
        // resumed by str.join, from C, once per segment and once more
        leaf(1)?;
        for _ in 0..=path.len() {
            c_called_leaf()?;
        }
        let mut shape: String = path.iter().map(|x| x.0).collect();

        if !shape.starts_with('m') {
            // Per PDF Reference Section 4.4.1, "path construction operators
            // may be invoked in any sequence, but the first one invoked must
            // be m or re to begin a new subpath." Since pdfminer.six already
            // converts all `re` (rectangle) operators to their equivalent
            // `mlllh` representation, paths ingested by `.paint_path(...)`
            // that do not begin with the `m` operator are invalid.
            return Ok(());
        }
        if shape.matches('m').count() > 1 {
            // recurse if there are multiple m's in this shape
            re_cache::compile_frames(SUBPATH_PATTERN, SUBPATH_COMPILE_DEPTH)?;
            for (start, end) in subpaths(&shape) {
                dev.paint_path(gstate, stroke, fill, evenodd, &path[start..end])?;
            }
            return Ok(());
        }

        // Although the 'h' command does not not literally provide a
        // point-position, its position is (by definition) equal to the
        // subpath's starting point.
        //
        // And, per Section 4.4's Table 4.9, all other path commands place
        // their point-position in their final two arguments. (Any preceding
        // arguments represent control points on Bézier curves.)
        let a = dev.analyzer_mut();
        let mut raw_pts: Vec<Point> = Vec::with_capacity(path.len());
        for p in path {
            leaf(1)?; // cast
            let src = if p.0 != 'h' { p } else { &path[0] };
            raw_pts.push(last_point(src));
        }
        let mut pts: Vec<Point> = Vec::with_capacity(raw_pts.len());
        for pt in &raw_pts {
            pts.push(apply_pt(a.device.ctm.as_ref(), pt)?);
        }

        let mut transformed_path: Vec<OriginalPathSegment> = Vec::with_capacity(path.len());
        for (op, coords) in path {
            let mut points = Vec::with_capacity(coords.len() / 2);
            for pair in coords.chunks_exact(2) {
                let pt = [Num::Float(pair[0].to_f64()?), Num::Float(pair[1].to_f64()?)];
                points.push(apply_pt(a.device.ctm.as_ref(), &pt)?);
            }
            transformed_path.push((*op, points));
        }
        for _ in &transformed_path {
            leaf(1)?; // cast
        }

        // Drop a redundant "l" on a path closed with "h"
        if shape.len() > 3 && shape.ends_with("lh") && point_eq(&pts[pts.len() - 2], &pts[0]) {
            shape.truncate(shape.len() - 2);
            shape.push('h');
            pts.pop();
        }

        let linewidth = gstate.linewidth.clone();
        let scolor = Some(gstate.scolor.clone());
        let ncolor = Some(gstate.ncolor.clone());
        let dash = gstate.dash.clone();
        let curve = if shape == "mlh" || shape == "ml" {
            // single line segment
            //
            // Note: 'ml', in conditional above, is a frequent anomaly
            // that we want to support.
            LTCurve::new_line(
                linewidth,
                pts[0].clone(),
                pts[1].clone(),
                stroke,
                fill,
                evenodd,
                scolor,
                ncolor,
                Some(transformed_path),
                dash,
            )?
        } else if shape == "mlllh" || shape == "mllll" {
            let [[x0, y0], [x1, y1], [x2, y2], [x3, y3], _] = unpack::<_, 5>(&pts)?;
            let is_closed_loop = point_eq(&pts[0], &pts[4]);
            let has_square_coordinates = (x0.py_eq(&x1) && y1.py_eq(&y2) && x2.py_eq(&x3) && y3.py_eq(&y0))
                || (y0.py_eq(&y1) && x1.py_eq(&x2) && y2.py_eq(&y3) && x3.py_eq(&x0));
            if is_closed_loop && has_square_coordinates {
                LTCurve::new_rect(
                    linewidth,
                    [x0, y0, x2, y2],
                    stroke,
                    fill,
                    evenodd,
                    scolor,
                    ncolor,
                    Some(transformed_path),
                    dash,
                )?
            } else {
                LTCurve::new(linewidth, pts, stroke, fill, evenodd, scolor, ncolor, Some(transformed_path), dash)?
            }
        } else {
            LTCurve::new(linewidth, pts, stroke, fill, evenodd, scolor, ncolor, Some(transformed_path), dash)?
        };
        a.cur_item()?.add(LTItem::Curve(Rc::new(curve)))
    }

    /// The spans of `re.finditer(r"m[^m]+", shape)`: an `m` followed by at
    /// least one other operator, as many as follow it (the search goes on
    /// after the match, or after an `m` followed by another `m`).
    pub(super) fn subpaths(shape: &str) -> Vec<(usize, usize)> {
        let ops = shape.as_bytes();
        let mut spans = Vec::new();
        let mut i = 0;
        while i < ops.len() {
            if ops[i] == b'm' {
                let end = i + 1 + ops[i + 1..].iter().take_while(|&&c| c != b'm').count();
                if end > i + 1 {
                    spans.push((i, end));
                    i = end;
                    continue;
                }
            }
            i += 1;
        }
        spans
    }

    /// `p[-2:]` of a segment of the interpreter's path (an operator and
    /// its coordinates, at least two but for `h`).
    fn last_point(p: &PathSegment) -> Point {
        let c = &p.1;
        let n = c.len();
        assert!(n >= 2, "a path segment {:?} without coordinates", p.0);
        [c[n - 2].clone(), c[n - 1].clone()]
    }

    /// `apply_matrix_pt(self.ctm, pt)`: a leaf call.
    fn apply_pt(ctm: Option<&Matrix>, pt: &Point) -> PyResult<Point> {
        leaf(1)?;
        apply_matrix_pt(ctm.ok_or_else(unpack_none)?, pt)
    }

    /// `p == q` of two points (tuples of new number objects: Python's
    /// `==` on the numbers).
    fn point_eq(p: &Point, q: &Point) -> bool {
        p[0].py_eq(&q[0]) && p[1].py_eq(&q[1])
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
        let _frame = py_frame()?;
        let text = match font.to_unichr(cid) {
            Ok(text) => PyStr::from(text),
            Err(e) if e.is_instance(pycompat::ExcKind::PDFUnicodeNotDefined) => dev.handle_undefined_char(font, cid)?,
            Err(e) => return Err(e),
        };
        let textwidth = font.char_width(cid)?;
        let textdisp = font.char_disp(cid)?;
        let item = LTChar::new(
            matrix,
            &**font,
            fontsize.clone(),
            scaling.clone(),
            rise.clone(),
            text,
            textwidth,
            textdisp,
            ncs.clone(),
            graphicstate.clone(),
        )?;
        let adv = item.adv.clone();
        dev.analyzer_mut().cur_item()?.add(LTItem::Char(Rc::new(item)))?;
        Ok(adv)
    }

    // pdfminer/converter.py:266 PDFLayoutAnalyzer.handle_undefined_char
    /// `f"(cid:{cid})"`, after `log.debug("undefined: %r, %r", font, cid)`.
    pub fn handle_undefined_char<P: InterpPayload, D: LayoutAnalyzer<P> + ?Sized>(
        _dev: &mut D,
        _font: &PDFFont,
        cid: &PyInt,
    ) -> PyResult<PyStr> {
        let _frame = py_frame()?;
        LOG.debug()?;
        Ok(PyStr::from(format!("(cid:{})", cid.py_str()?)))
    }

    // pdfminer/converter.py:270 PDFLayoutAnalyzer.receive_layout
    pub fn receive_layout<P: InterpPayload, D: LayoutAnalyzer<P> + ?Sized>(
        _dev: &mut D,
        _ltpage: LTPage<P>,
    ) -> PyResult<()> {
        leaf(1)
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
    /// The attributes (no frame: see [`Self::create`]).
    pub fn new(rsrcmgr: Rc<PDFResourceManager>, pageno: PyInt, laparams: Option<LAParams>) -> PDFPageAggregator<P> {
        PDFPageAggregator { analyzer: PDFLayoutAnalyzer::new(rsrcmgr, pageno, laparams), result: None }
    }

    // pdfminer/converter.py:275 PDFPageAggregator.__init__
    /// `PDFPageAggregator(rsrcmgr, pageno=pageno, laparams=laparams)`: the
    /// class call (its C unit and evaluation loop entry), `__init__`'s
    /// frame and those of the base `__init__`s, then the attributes.
    pub fn create(
        rsrcmgr: Rc<PDFResourceManager>,
        pageno: PyInt,
        laparams: Option<LAParams>,
    ) -> PyResult<PDFPageAggregator<P>> {
        let _call = class_call()?;
        Self::init_frames()?;
        Ok(PDFPageAggregator::new(rsrcmgr, pageno, laparams))
    }

    /// The frames of `PDFPageAggregator.__init__` (its own, then
    /// `PDFLayoutAnalyzer.__init__`'s), for a subclass's class call.
    pub fn init_frames() -> PyResult<()> {
        let _frame = py_frame()?;
        PDFLayoutAnalyzer::<P>::init_frames()
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
        leaf(1)?;
        dev.aggregator_mut().result = Some(ltpage);
        Ok(())
    }

    // pdfminer/converter.py:287 PDFPageAggregator.get_result
    /// `assert self.result is not None` (`AssertionError`).
    pub fn get_result<P: InterpPayload, D: PageAggregator<P> + ?Sized>(dev: &D) -> PyResult<&LTPage<P>> {
        leaf(1)?;
        dev.aggregator().result.as_ref().ok_or_else(|| exc::AssertionError(""))
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

#[cfg(test)]
mod tests {
    use super::layout_analyzer::subpaths;

    /// `[m.span() for m in re.finditer(r"m[^m]+", shape)]`, computed by
    /// CPython 3.12.
    #[test]
    fn subpaths_are_the_matches_of_the_pattern() {
        let cases: &[(&str, &[(usize, usize)])] = &[
            ("", &[]),
            ("m", &[]),
            ("mm", &[]),
            ("ml", &[(0, 2)]),
            ("mlml", &[(0, 2), (2, 4)]),
            ("mmlh", &[(1, 4)]),
            ("lmlhm", &[(1, 4)]),
            ("mlmmc", &[(0, 2), (3, 5)]),
            ("hhmlllhmc", &[(2, 7), (7, 9)]),
        ];
        for (shape, expected) in cases {
            assert_eq!(subpaths(shape), *expected, "{shape}");
        }
    }
}
