//! Port of `pdfminer/pdfinterp.py` (partial).
//!
//! Ported: `PDFContentParser` (the content stream parser, with inline
//! images). **Wave 3 skeleton** (signatures and state, bodies `todo!()`,
//! workstream INTERP, `docs/architecture.md`, "Vague 3"): `PDFTextState`,
//! `PDFGraphicState` and the colors, `PDFResourceManager`,
//! `PDFPageInterpreter`. The exception classes `PDFResourceError` and
//! `PDFInterpreterError` are
//! `pycompat::ExcKind::{PDFResourceError, PDFInterpreterError}`.
//!
//! # How pdfplumber drives it
//!
//! One [`PDFResourceManager`] per document
//! (pdfplumber/pdf.py:28 `PDF.__init__`, at its line 53; `caching=True`:
//! fonts are cached by object id across pages); for each page a new device
//! (`pdfplumber::page::PDFPageAggregatorWithMarkedContent`) and a new
//! [`PDFPageInterpreter`] (pdfplumber/page.py:256 `Page.layout`), then
//! [`PDFPageInterpreter::process_page`]. The resource manager belongs to
//! the document's state: a call of the JavaScript boundary that a missing
//! predefined CMap made void (`crates/wasm/src/cmaps.rs`) drops that state
//! with the fonts it cached (a font built while a CMap was missing holds
//! an empty `CMap()`); the interpreter never catches or hides that
//! condition (it is not an exception: the provider records the name).
//!
//! # Values
//!
//! The operands are unchecked PDF objects ([`PDFStackT`]); the operators
//! convert them as upstream does (`safe_float`, `safe_matrix`...: floats,
//! or a logged warning and no change). The states hold Python's numbers
//! ([`Num`]: `PDFTextState()` starts with ints, `fontsize = 0`,
//! `scaling = 100`; an operator sets floats), and the colors their Python
//! shapes ([`Color`]).
//!
//! # Recursion depth
//!
//! Form XObjects recurse: `execute` -> `do_Do` (from `func(*args)`) ->
//! `subinterp()` -> `render_contents` -> `execute`. Every ported method
//! takes its frame (`py_frame`) for its duration; `func(*args)` in
//! `execute` is a call from C in CPython 3.12 (`CALL_FUNCTION_EX`), so an
//! evaluation loop entry (`eval_entry`) before the operator's frame
//! (measure it with `oracle/depth_probe.py` rather than assume it);
//! class instantiations (`PDFTextState()`, `PDFGraphicState()`,
//! `self.__class__(...)` in `dup`, the device's objects) take the entry and
//! the `__init__` frame (`c_called_leaf` when it returns at once);
//! `log.debug(...)` of `render_contents`, `execute` (one per operator),
//! `init_resources`, `do_Do`, and every `log.warning(...)` take theirs
//! through `pycompat::logging` (`Logger::new("pdfminer.pdfinterp")`,
//! whose cache it shares with `pdffont::frames`, by name). The parser
//! (`PDFContentParser`), `pdftypes`, `pdffont` and `cmapdb` count their
//! own. The device's methods are called with their own frames
//! (`pdfdevice`). The `fonts`, `interp` and `layout` stages and the
//! `adv-interp` corpus check the boundary: windows of nesting depths
//! around the first one that fails in Python, the deepest point being each
//! time another path (an operator's warning, a font created in the deepest
//! form, a char, an inline image, a path with several subpaths, a
//! circular reference refused).

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::LazyLock;

use indexmap::IndexMap;
use pycompat::io::{BytesIO, Pos};
use pycompat::recursion::{c_called_leaf, eval_entry, leaf, py_frame};
use pycompat::{exc, ExcKind, Num, PyInt, PyResult};
use rustc_hash::FxBuildHasher;

use crate::cmapdb::CMapBase;
use crate::pdfcolor::PDFColorSpace;
use crate::pdfdevice::PDFDevice;
use crate::pdffont::PDFFont;
use crate::pdfpage::PDFPage;
use crate::pdftypes::{
    in_literals, resolve1_none, stream_value, ObjId, PDFDict, PDFObject, PDFStream, LITERALS_ASCII85_DECODE,
};
use crate::psparser::{literal_name, Buf, PSKeyword, PSLiteral, PSStackParser, PSStackParserExt, BUFSIZ, KWD, LIT};
use crate::settings;
use crate::utils::{choplist, Matrix, PathSegment, Point};

/// The state `PDFContentParser` adds to `PSStackParser`.
pub struct ContentStreams {
    /// `self.streams`: the content streams (any objects; each goes through
    /// `stream_value`).
    pub streams: Vec<PDFObject>,
    /// `self.istream`: index of the next stream to open.
    pub istream: usize,
    /// `KEYWORD_BI`, `KEYWORD_ID`, `KEYWORD_EI`.
    keywords: [PSKeyword; 3],
}

/// pdfminer/pdfinterp.py:256 `PDFContentParser`.
pub type PDFContentParser = PSStackParser<ContentStreams>;

// pdfminer/pdfinterp.py:327-329 PDFContentParser.KEYWORD_BI, PDFContentParser.KEYWORD_ID, PDFContentParser.KEYWORD_EI
pub static KEYWORD_BI: LazyLock<PSKeyword> = LazyLock::new(|| KWD(b"BI"));
pub static KEYWORD_ID: LazyLock<PSKeyword> = LazyLock::new(|| KWD(b"ID"));
pub static KEYWORD_EI: LazyLock<PSKeyword> = LazyLock::new(|| KWD(b"EI"));

impl PSStackParser<ContentStreams> {
    // pdfminer/pdfinterp.py:257 PDFContentParser.__init__
    /// `PDFContentParser(streams)`; raises `PSEOF` when there is no stream.
    pub fn new(streams: Vec<PDFObject>) -> PyResult<PDFContentParser> {
        // the class is called (from C)
        let _entry = eval_entry()?;
        let _frame = py_frame()?;
        // PSStackParser.__init__(fp=None) is safe only because we've
        // overloaded all the methods that would attempt to access self.fp
        // without first calling self.fillfp().
        PSStackParser::init(
            None,
            ContentStreams { streams, istream: 0, keywords: [*KEYWORD_BI, *KEYWORD_ID, *KEYWORD_EI] },
        )
    }

    // pdfminer/pdfinterp.py:265 PDFContentParser.fillfp
    /// Opens the next stream when none is open; returns whether it did.
    pub fn fillfp(&mut self) -> PyResult<bool> {
        let _frame = py_frame()?;
        if self.fp.is_none() {
            let strm = if self.ext.istream < self.ext.streams.len() {
                let strm = stream_value(&self.ext.streams[self.ext.istream])?;
                self.ext.istream += 1;
                strm
            } else {
                return Err(exc::PSEOF("Unexpected EOF, file truncated?"));
            };
            self.fp = Some(BytesIO::new(strm.get_data()?));
            return Ok(true);
        }
        Ok(false)
    }

    // pdfminer/pdfinterp.py:294 PDFContentParser.get_inline_data
    /// Reads the data of an inline image up to `target` followed by a
    /// whitespace (`EI`), or up to `target` (`~>`, for ASCII85 data).
    pub fn get_inline_data(&mut self, pos: Pos, target: &[u8]) -> PyResult<(Pos, Vec<u8>)> {
        let _frame = py_frame()?;
        self.seek(pos)?;
        let mut i = 0;
        let mut data: Vec<u8> = Vec::new();
        while i <= target.len() {
            self.fillbuf()?;
            if i != 0 {
                let c = self.buf[self.charpos];
                data.push(c);
                self.charpos += 1;
                if (target.len() <= i && pycompat::text::bytes_isspace(c)) || (i < target.len() && c == target[i]) {
                    i += 1;
                } else {
                    i = 0;
                }
            } else {
                match self.buf[self.charpos..].iter().position(|&b| b == target[0]) {
                    Some(k) => {
                        let j = self.charpos + k;
                        data.extend_from_slice(&self.buf[self.charpos..=j]);
                        self.charpos = j + 1;
                        i = 1;
                    }
                    None => {
                        data.extend_from_slice(&self.buf[self.charpos..]);
                        self.charpos = self.buf.len();
                    }
                }
            }
            self.check_progress("get_inline_data");
        }
        // strip the last part
        data.truncate(data.len().saturating_sub(target.len() + 1));
        let end = strip_trailing_eol(&data);
        data.truncate(end);
        Ok((pos, data))
    }
}

/// `re.sub(rb"(\x0d\x0a|[\x0d\x0a])$", b"", data)`, as the length to keep.
///
/// `$` matches at the end and just before a final `\n`, so a match starts
/// in the last three bytes; the leftmost match either reaches the end or
/// stops before a final `\n`, which the next match then removes. Removing
/// every match therefore cuts the data at the start of the leftmost one.
fn strip_trailing_eol(data: &[u8]) -> usize {
    let n = data.len();
    let at_dollar = |q: usize| q == n || (q + 1 == n && data[n - 1] == b'\n');
    for p in n.saturating_sub(3)..n {
        if data[p] == b'\r' && p + 1 < n && data[p + 1] == b'\n' && at_dollar(p + 2) {
            return p;
        }
        if (data[p] == b'\r' || data[p] == b'\n') && at_dollar(p + 1) {
            return p;
        }
    }
    n
}

impl PSStackParserExt for ContentStreams {
    // pdfminer/pdfinterp.py:276 PDFContentParser.seek
    fn seek(p: &mut PSStackParser<Self>, pos: Pos) -> PyResult<()> {
        let _frame = py_frame()?;
        p.fillfp()?;
        p.psstackparser_seek(pos)
    }

    // pdfminer/pdfinterp.py:280 PDFContentParser.fillbuf
    fn fillbuf(p: &mut PSStackParser<Self>) -> PyResult<bool> {
        let _frame = py_frame()?;
        if p.charpos < p.buf.len() {
            return Ok(false);
        }
        let mut new_stream;
        loop {
            new_stream = p.fillfp()?;
            let fp = p.fp.as_mut().expect("fillfp() opened a stream");
            let bufpos = fp.tell();
            let range = fp.read(Some(BUFSIZ));
            let data = fp.data().clone();
            p.bufpos = bufpos;
            p.buf = Buf::new(data, range);
            if !p.buf.is_empty() {
                break;
            }
            p.fp = None;
        }
        p.charpos = 0;
        Ok(new_stream)
    }

    /// The ordinal of the current stream, then the offset in it.
    fn read_position(p: &PSStackParser<Self>) -> (usize, Pos) {
        (p.ext.istream, p.position())
    }

    // pdfminer/pdfinterp.py:324 PDFContentParser.flush
    fn flush(p: &mut PSStackParser<Self>) -> PyResult<()> {
        let _frame = py_frame()?;
        p.add_results_popall()
    }

    // pdfminer/pdfinterp.py:331 PDFContentParser.do_keyword
    fn do_keyword(p: &mut PSStackParser<Self>, pos: Pos, token: PSKeyword) -> PyResult<()> {
        let _frame = py_frame()?;
        let [keyword_bi, keyword_id, _] = p.ext.keywords;
        if token == keyword_bi {
            // inline image within a content stream
            p.start_type(pos, "inline")?;
        } else if token == keyword_id {
            match inline_image(p, pos) {
                Err(e) if e.is_instance(ExcKind::PSTypeError) && !settings::STRICT => {}
                other => other?,
            }
        } else {
            p.push((pos, PDFObject::Keyword(token)))?;
        }
        Ok(())
    }
}

/// The `try:` block of `do_keyword` for `ID`.
fn inline_image(p: &mut PDFContentParser, pos: Pos) -> PyResult<()> {
    let (_, objs) = p.end_type("inline")?;
    if !objs.len().is_multiple_of(2) {
        let error_msg = format!("Invalid dictionary construct: {}", PDFObject::list_repr(&objs)?);
        return Err(exc::PSTypeError(error_msg));
    }
    // {literal_name(k): resolve1(v) for (k, v) in choplist(2, objs)}: the
    // generator's frame one level down (created, then resumed)
    leaf(1)?;
    let d = PDFDict::new();
    for pair in choplist(2, &objs) {
        d.insert(literal_name(&pair[0])?.into(), resolve1_none(&pair[1])?);
    }
    let mut eos: &[u8] = b"EI";
    if let Some(filter) = d.get("F") {
        if !matches!(filter, PDFObject::Null) {
            let first = match &filter {
                PDFObject::Literal(_) => filter.clone(),
                other => other.py_getitem_index0()?,
            };
            if in_literals(&first, &*LITERALS_ASCII85_DECODE) {
                eos = b"~>";
            }
        }
    }
    let (pos, mut data) = p.get_inline_data(pos + b"ID ".len() as Pos, eos)?;
    if eos != b"EI" {
        // it may be necessary for decoding
        data.extend_from_slice(eos);
    }
    // PDFStream(d, data): its __init__, called from C
    c_called_leaf()?;
    let obj = PDFStream::new(d, Rc::from(data), None);
    p.push((pos, PDFObject::Stream(Rc::new(obj))))?;
    if eos == b"EI" {
        // otherwise it is still in the stream
        p.push((pos, PDFObject::Keyword(p.ext.keywords[2])))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Wave 3 skeleton: the states, the resource manager, the page interpreter
// (workstream INTERP).

// pdfminer/pdfinterp.py:63-67 LITERAL_PDF, LITERAL_TEXT, LITERAL_FONT, LITERAL_FORM, LITERAL_IMAGE
pub static LITERAL_PDF: LazyLock<PSLiteral> = LazyLock::new(|| LIT("PDF"));
pub static LITERAL_TEXT: LazyLock<PSLiteral> = LazyLock::new(|| LIT("Text"));
pub static LITERAL_FONT: LazyLock<PSLiteral> = LazyLock::new(|| LIT("Font"));
pub static LITERAL_FORM: LazyLock<PSLiteral> = LazyLock::new(|| LIT("Form"));
pub static LITERAL_IMAGE: LazyLock<PSLiteral> = LazyLock::new(|| LIT("Image"));

// pdfminer/pdfinterp.py:364 PDFStackT
/// `PDFStackT`: an operand on the argument stack, whatever the content
/// parser yields (a `PDFStream` for an inline image).
pub type PDFStackT = PDFObject;

// pdfminer/pdfinterp.py:121 StandardColor
/// `StandardColor`: a gray level, `(r, g, b)` or `(c, m, y, k)`. The gray
/// is the int `0` of a new `PDFGraphicState`, else a float (`safe_float`).
#[derive(Clone, Debug)]
pub enum StandardColor {
    Gray(Num),
    Rgb([f64; 3]),
    Cmyk([f64; 4]),
}

// pdfminer/pdfinterp.py:128 Color
/// `Color`: a standard color, a pattern name (a colored pattern,
/// `PaintType=1`: `literal_name` of the operand), or `(base_color,
/// pattern_name)` (an uncolored pattern, `PaintType=2`).
#[derive(Clone, Debug)]
pub enum Color {
    Standard(StandardColor),
    Pattern(String),
    UncoloredPattern(StandardColor, String),
}

/// pdfminer/pdfinterp.py:70 `PDFTextState`. The fields are the instance
/// attributes, in `__dict__` order ([`Self::DICT_KEYS`]).
#[derive(Clone)]
pub struct PDFTextState {
    pub font: Option<Rc<PDFFont>>,
    pub fontsize: Num,
    pub charspace: Num,
    pub wordspace: Num,
    pub scaling: Num,
    pub leading: Num,
    /// `safe_int` of the `Tr` operand.
    pub render: PyInt,
    pub rise: Num,
    pub matrix: Matrix,
    pub linematrix: Point,
}

#[allow(unused_variables)] // wave 3 skeleton: the bodies are todo!()
impl PDFTextState {
    /// `__dict__` keys, in order (`reset` sets `matrix`, then `linematrix`).
    pub const DICT_KEYS: [&'static str; 10] =
        ["font", "fontsize", "charspace", "wordspace", "scaling", "leading", "render", "rise", "matrix", "linematrix"];

    // pdfminer/pdfinterp.py:74 PDFTextState.__init__
    /// `PDFTextState()`: ints (`fontsize = 0`, `scaling = 100`...), then
    /// `reset()`. Takes the frames of the class call.
    pub fn new() -> PyResult<PDFTextState> {
        todo!("wave 3 INTERP")
    }

    // pdfminer/pdfinterp.py:101 PDFTextState.copy
    pub fn copy(&self) -> PyResult<PDFTextState> {
        todo!("wave 3 INTERP")
    }

    // pdfminer/pdfinterp.py:115 PDFTextState.reset
    /// `matrix = MATRIX_IDENTITY`, `linematrix = (0, 0)` (ints).
    pub fn reset(&mut self) -> PyResult<()> {
        todo!("wave 3 INTERP")
    }
}

/// pdfminer/pdfinterp.py:137 `PDFGraphicState`. The fields are the
/// instance attributes, in `__dict__` order ([`Self::DICT_KEYS`]); the
/// line style operands are kept unchecked (`None` is
/// [`PDFObject::Null`]).
#[derive(Clone)]
pub struct PDFGraphicState {
    /// The int `0`, then `safe_float(w) * scale` (a float).
    pub linewidth: Num,
    pub linecap: PDFStackT,
    pub linejoin: PDFStackT,
    pub miterlimit: PDFStackT,
    /// `(dash, phase)`, the raw operands of `d`.
    pub dash: Option<(PDFStackT, PDFStackT)>,
    pub intent: PDFStackT,
    pub flatness: PDFStackT,
    pub scolor: Color,
    pub scs: Rc<PDFColorSpace>,
    pub ncolor: Color,
    pub ncs: Rc<PDFColorSpace>,
}

#[allow(unused_variables)] // wave 3 skeleton: the bodies are todo!()
impl PDFGraphicState {
    /// `__dict__` keys, in order.
    pub const DICT_KEYS: [&'static str; 11] = [
        "linewidth",
        "linecap",
        "linejoin",
        "miterlimit",
        "dash",
        "intent",
        "flatness",
        "scolor",
        "scs",
        "ncolor",
        "ncs",
    ];

    // pdfminer/pdfinterp.py:138 PDFGraphicState.__init__
    /// `PDFGraphicState()`: `DeviceGray` (the shared predefined color
    /// space, `pdfcolor::predefined_colorspace`) and the gray `0`.
    pub fn new() -> PyResult<PDFGraphicState> {
        todo!("wave 3 INTERP")
    }

    // pdfminer/pdfinterp.py:155 PDFGraphicState.copy
    pub fn copy(&self) -> PyResult<PDFGraphicState> {
        todo!("wave 3 INTERP")
    }
}

/// pdfminer/pdfinterp.py:185 `PDFResourceManager`: the repository of the
/// fonts of a document (one per document, shared by its pages: see the
/// module documentation).
pub struct PDFResourceManager {
    /// `self.caching`
    pub caching: bool,
    /// `self._cached_fonts`: `objid -> font`, in insertion order (the
    /// `fonts` stage writes the entries each page adds). The `objid` is a
    /// `PDFObjRef`'s (an int); `get_font` only caches a truthy one.
    _cached_fonts: RefCell<IndexMap<PyInt, Rc<PDFFont>, FxBuildHasher>>,
}

#[allow(unused_variables)] // wave 3 skeleton: the bodies are todo!()
impl PDFResourceManager {
    // pdfminer/pdfinterp.py:193 PDFResourceManager.__init__
    pub fn new(caching: bool) -> PDFResourceManager {
        PDFResourceManager { caching, _cached_fonts: RefCell::new(IndexMap::default()) }
    }

    // pdfminer/pdfinterp.py:197 PDFResourceManager.get_procset
    /// Does nothing with the items (`list_value(v)` of `/ProcSet`).
    pub fn get_procset(&self, procs: &[PDFObject]) -> PyResult<()> {
        todo!("wave 3 INTERP")
    }

    // pdfminer/pdfinterp.py:204 PDFResourceManager.get_cmap
    pub fn get_cmap(&self, cmapname: &str, strict: bool) -> PyResult<Rc<dyn CMapBase>> {
        todo!("wave 3 INTERP")
    }

    // pdfminer/pdfinterp.py:212 PDFResourceManager.get_font
    /// The cached font of `objid` (when truthy and cached), else
    /// `pdffont::get_font(spec)` (which takes `get_font`'s frame and
    /// creates the font, `Type0` included), cached when `objid` is truthy
    /// and `caching` is on. `init_resources` passes the `PDFObjRef`'s
    /// `objid` of a font given by reference, `None` for a direct
    /// dictionary; `do_Tf` passes `(None, {})` for an unknown font name.
    pub fn get_font(&self, objid: Option<&PyInt>, spec: &PDFDict) -> PyResult<Rc<PDFFont>> {
        todo!("wave 3 INTERP")
    }

    /// `list(self._cached_fonts.items())`, in insertion order.
    pub fn cached_fonts(&self) -> Vec<(PyInt, Rc<PDFFont>)> {
        self._cached_fonts.borrow().iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }
}

/// The name the port gives the `n`-th inline image (from 0) of a page's
/// interpretation, where upstream's `do_EI` gives `str(id(obj))`, a
/// memory address (pdfminer/pdfinterp.py:1309 `PDFPageInterpreter.do_EI`):
/// deterministic, and unique among the
/// inline images of a page (`docs/architecture.md`, "Vague 3"). The name
/// reaches `LTFigure.name`, `LTImage.name` and pdfplumber's image `name`.
pub fn inline_image_name(n: usize) -> String {
    format!("inline-{n}")
}

/// pdfminer/pdfinterp.py:367 `PDFPageInterpreter`: the processor of the
/// content of a page (PDF Reference, Appendix A, Operator Summary).
///
/// Each operator is a method with its upstream name and arity
/// (`do_q`, `do_cm(a1, b1, c1, d1, e1, f1)`, `do_T_a` for `T*`, `do__q`
/// for `'`, `do__w` for `"`, ...: every `do_*` of the class, the empty ones
/// included); `execute` maps a keyword's name (`keyword_name`, then the
/// replacements of `*`, `"` and `'`) to it, so a keyword spelled like a
/// method (`T_a`, `_q`) runs it too, as in Python, and pops its arity
/// (`func.__code__.co_argcount - 1`) of operands, calling it only when that
/// many were there.
///
/// The fields set by `init_resources` and `init_state` are not set before
/// upstream (an attribute error): here they hold placeholders until then,
/// which nothing reads (`process_page` and `render_contents` set them
/// first), except `fontmap`, `None` until `init_resources` ran (the
/// `fonts` stage reads it).
pub struct PDFPageInterpreter {
    /// `self.rsrcmgr`
    pub rsrcmgr: Rc<PDFResourceManager>,
    /// `self.device`, shared with the sub-interpreters.
    pub device: Rc<RefCell<dyn PDFDevice>>,
    /// `self.stream_ids`: the objids of the streams being executed.
    pub stream_ids: HashSet<ObjId>,
    /// `self.parent_stream_ids`: those of the parent interpreters.
    pub parent_stream_ids: HashSet<ObjId>,
    /// Not upstream: the number of inline images this interpreter and the
    /// interpreters it was made from or made have named
    /// ([`inline_image_name`]); shared by `dup`.
    pub inline_images: Rc<Cell<usize>>,
    /// `self.resources`: what `init_resources` was given.
    pub resources: PDFObject,
    /// `self.fontmap`: `fontid -> font`, in the order of the `/Font` dict.
    pub fontmap: Option<IndexMap<Rc<str>, Rc<PDFFont>, FxBuildHasher>>,
    /// `self.xobjmap`: `xobjid -> object`, unresolved.
    pub xobjmap: IndexMap<Rc<str>, PDFObject, FxBuildHasher>,
    /// `self.csmap`: `PREDEFINED_COLORSPACE.copy()` and the page's color
    /// spaces (new objects for each `init_resources`, as upstream).
    pub csmap: IndexMap<Rc<str>, Rc<PDFColorSpace>, FxBuildHasher>,
    /// `self.gstack`: the saved `(ctm, textstate, graphicstate)`.
    pub gstack: Vec<(Matrix, PDFTextState, PDFGraphicState)>,
    /// `self.ctm`
    pub ctm: Matrix,
    /// `self.textstate`
    pub textstate: PDFTextState,
    /// `self.graphicstate`
    pub graphicstate: PDFGraphicState,
    /// `self.curpath`: the segments of the path being built (floats).
    pub curpath: Vec<PathSegment>,
    /// `self.argstack`
    pub argstack: Vec<PDFStackT>,
}

#[allow(unused_variables)] // wave 3 skeleton: the bodies are todo!()
impl PDFPageInterpreter {
    // pdfminer/pdfinterp.py:373 PDFPageInterpreter.__init__
    /// `PDFPageInterpreter(rsrcmgr, device)`: takes the frames of the class
    /// call.
    pub fn new(rsrcmgr: Rc<PDFResourceManager>, device: Rc<RefCell<dyn PDFDevice>>) -> PyResult<PDFPageInterpreter> {
        todo!("wave 3 INTERP")
    }

    // pdfminer/pdfinterp.py:381 PDFPageInterpreter.dup
    pub fn dup(&self) -> PyResult<PDFPageInterpreter> {
        todo!("wave 3 INTERP")
    }

    // pdfminer/pdfinterp.py:384 PDFPageInterpreter.subinterp
    /// A sub-interpreter for a Form XObject: `dup()`, with the parent's
    /// stream ids.
    pub fn subinterp(&self) -> PyResult<PDFPageInterpreter> {
        todo!("wave 3 INTERP")
    }

    // pdfminer/pdfinterp.py:397 PDFPageInterpreter.init_resources
    /// Prepares the fonts (`rsrcmgr.get_font`), color spaces and XObjects
    /// listed in `resources`.
    pub fn init_resources(&mut self, resources: &PDFObject) -> PyResult<()> {
        todo!("wave 3 INTERP")
    }

    // pdfminer/pdfinterp.py:438 PDFPageInterpreter.init_state
    /// New text and graphic states, `ctm`, `device.set_ctm(ctm)`.
    pub fn init_state(&mut self, ctm: Matrix) -> PyResult<()> {
        todo!("wave 3 INTERP")
    }

    // pdfminer/pdfinterp.py:450 PDFPageInterpreter.push
    pub fn push(&mut self, obj: PDFStackT) -> PyResult<()> {
        todo!("wave 3 INTERP")
    }

    // pdfminer/pdfinterp.py:453 PDFPageInterpreter.pop
    pub fn pop(&mut self, n: usize) -> PyResult<Vec<PDFStackT>> {
        todo!("wave 3 INTERP")
    }

    // pdfminer/pdfinterp.py:460 PDFPageInterpreter.get_current_state
    pub fn get_current_state(&self) -> PyResult<(Matrix, PDFTextState, PDFGraphicState)> {
        todo!("wave 3 INTERP")
    }

    // pdfminer/pdfinterp.py:463 PDFPageInterpreter.set_current_state
    pub fn set_current_state(&mut self, state: (Matrix, PDFTextState, PDFGraphicState)) -> PyResult<()> {
        todo!("wave 3 INTERP")
    }

    // pdfminer/pdfinterp.py:1352 PDFPageInterpreter.process_page
    /// The page's CTM from its mediabox and rotation, then
    /// `device.begin_page`, `render_contents(page.resources,
    /// page.contents, ctm)`, `device.end_page`.
    pub fn process_page(&mut self, page: &PDFPage) -> PyResult<()> {
        todo!("wave 3 INTERP")
    }

    // pdfminer/pdfinterp.py:1367 PDFPageInterpreter.render_contents
    /// `init_resources(resources)`, `init_state(ctm)`,
    /// `execute(list_value(streams))`. Called recursively for Form
    /// XObjects (on a sub-interpreter, from `do_Do`).
    pub fn render_contents(&mut self, resources: &PDFObject, streams: &PDFObject, ctm: Matrix) -> PyResult<()> {
        todo!("wave 3 INTERP")
    }

    // pdfminer/pdfinterp.py:1387 PDFPageInterpreter.execute
    /// Skips the streams without an objid and those a parent interpreter
    /// is executing (circular references), then runs the operators of
    /// `PDFContentParser(valid_streams)` until `PSEOF`.
    pub fn execute(&mut self, streams: &[PDFObject]) -> PyResult<()> {
        todo!("wave 3 INTERP")
    }
}
