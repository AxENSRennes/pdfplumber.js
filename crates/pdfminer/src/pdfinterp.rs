//! Port of `pdfminer/pdfinterp.py`: the resource manager
//! (`PDFResourceManager`, with its font cache), the content stream parser
//! (`PDFContentParser`, with inline images), the text and graphic states
//! (`PDFTextState`, `PDFGraphicState`, the colors) and the page
//! interpreter (`PDFPageInterpreter`: every `do_*` operator, color spaces
//! and patterns, inline images, marked content, Form and image XObjects,
//! `render_contents`, `execute`, `process_page`). The exception classes
//! `PDFResourceError` and `PDFInterpreterError` are
//! `pycompat::ExcKind::{PDFResourceError, PDFInterpreterError}`; with
//! `settings.STRICT = False`, the interpreter never raises them.
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
//! (`pdfdevice`). The `pdfminer.casting` functions are Python functions
//! that the port of `casting` does not count: the operators check their
//! frames (`leaf`: `safe_float`, `safe_int` one, `safe_matrix` two,
//! `safe_rgb` and `safe_cmyk` three, through `_safe_float_triple` and
//! `_safe_float_quadruple`), as they check those of `utils.mult_matrix`,
//! `keyword_name` and the dunder methods of `PDFStream` run from C
//! (`__contains__`: `c_called_leaf`). The `fonts`, `interp` and `layout`
//! stages (and their variants on `PDFTextDevice`, `fonts_text` and
//! `interp_text`) and the `adv-interp` corpus check the boundary: windows
//! of nesting depths
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
use pycompat::logging::Logger;
use pycompat::recursion::{c_called_leaf, eval_entry, leaf, py_frame};
use pycompat::text::decode_utf8_ignore;
use pycompat::{exc, ExcKind, Num, PyInt, PyResult};
use rustc_hash::FxBuildHasher;

use crate::casting::{safe_cmyk, safe_float, safe_int, safe_matrix, safe_rgb};
use crate::cmapdb::{Attrs, CMap, CMapBase, CMapDB};
use crate::pdfcolor::{predefined_colorspace, predefined_colorspaces, PDFColorSpace};
use crate::pdfdevice::{ObjSeq, PDFDevice};
use crate::pdffont::PDFFont;
use crate::pdfpage::PDFPage;
use crate::pdftypes::{
    dict_value, in_literals, list_value, resolve1_none, stream_value, ObjId, PDFDict, PDFList, PDFObject, PDFStream,
    LITERALS_ASCII85_DECODE,
};
use crate::psparser::{literal_name, Buf, PSKeyword, PSLiteral, PSStackParser, PSStackParserExt, BUFSIZ, KWD, LIT};
use crate::settings;
use crate::utils::{choplist, matrix_identity, mult_matrix, mult_matrix_obj, Matrix, PathSegment, Point};

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
// The states, the resource manager, the page interpreter

/// `log = logging.getLogger(__name__)`: shares its cache with
/// `pdffont::frames` (the `log.debug` of `get_font`), by name.
static LOG: Logger = Logger::new("pdfminer.pdfinterp");

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

/// An int.
fn int(v: i64) -> Num {
    Num::Int(PyInt::Small(v))
}

/// `(0, 0)`
fn origin() -> Point {
    [int(0), int(0)]
}

/// `MATRIX_IDENTITY` as PDF objects (the tuple `do_Do` and `do_EI` hand to
/// `begin_figure`).
fn identity_objs() -> Vec<PDFObject> {
    [1, 0, 0, 1, 0, 0].into_iter().map(|v| PDFObject::Int(PyInt::Small(v))).collect()
}

/// `(0, 0, 1, 1)`, the bounding box of an image's figure.
fn unit_rect_objs() -> Vec<PDFObject> {
    [0, 0, 1, 1].into_iter().map(|v| PDFObject::Int(PyInt::Small(v))).collect()
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

impl PDFTextState {
    /// `__dict__` keys, in order (`reset` sets `matrix`, then `linematrix`).
    pub const DICT_KEYS: [&'static str; 10] =
        ["font", "fontsize", "charspace", "wordspace", "scaling", "leading", "render", "rise", "matrix", "linematrix"];

    /// The attributes of `__init__`, without its frames.
    fn initial() -> PDFTextState {
        PDFTextState {
            font: None,
            fontsize: int(0),
            charspace: int(0),
            wordspace: int(0),
            scaling: int(100),
            leading: int(0),
            render: PyInt::ZERO,
            rise: int(0),
            matrix: matrix_identity(),
            linematrix: origin(),
        }
    }

    // pdfminer/pdfinterp.py:74 PDFTextState.__init__
    /// `PDFTextState()`: ints (`fontsize = 0`, `scaling = 100`...), then
    /// `reset()`. Takes the frames of the class call.
    pub fn new() -> PyResult<PDFTextState> {
        // the class is called: __init__ runs from C
        let _entry = eval_entry()?;
        let _frame = py_frame()?;
        let mut state = PDFTextState::initial();
        state.reset()?;
        // self.matrix is set
        // self.linematrix is set
        Ok(state)
    }

    // pdfminer/pdfinterp.py:101 PDFTextState.copy
    pub fn copy(&self) -> PyResult<PDFTextState> {
        let _frame = py_frame()?;
        let mut obj = PDFTextState::new()?;
        obj.font = self.font.clone();
        obj.fontsize = self.fontsize.clone();
        obj.charspace = self.charspace.clone();
        obj.wordspace = self.wordspace.clone();
        obj.scaling = self.scaling.clone();
        obj.leading = self.leading.clone();
        obj.render = self.render.clone();
        obj.rise = self.rise.clone();
        obj.matrix = self.matrix.clone();
        obj.linematrix = self.linematrix.clone();
        Ok(obj)
    }

    // pdfminer/pdfinterp.py:115 PDFTextState.reset
    /// `matrix = MATRIX_IDENTITY`, `linematrix = (0, 0)` (ints).
    pub fn reset(&mut self) -> PyResult<()> {
        leaf(1)?;
        self.matrix = matrix_identity();
        self.linematrix = origin();
        Ok(())
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

    /// The attributes of `__init__`, without its frames.
    fn initial() -> PDFGraphicState {
        let gray = predefined_colorspace("DeviceGray").expect("DeviceGray is predefined");
        PDFGraphicState {
            linewidth: int(0),
            linecap: PDFObject::Null,
            linejoin: PDFObject::Null,
            miterlimit: PDFObject::Null,
            dash: None,
            intent: PDFObject::Null,
            flatness: PDFObject::Null,
            // stroking color
            scolor: Color::Standard(StandardColor::Gray(int(0))),
            scs: gray.clone(),
            // non stroking color
            ncolor: Color::Standard(StandardColor::Gray(int(0))),
            ncs: gray,
        }
    }

    // pdfminer/pdfinterp.py:138 PDFGraphicState.__init__
    /// `PDFGraphicState()`: `DeviceGray` (the shared predefined color
    /// space, `pdfcolor::predefined_colorspace`) and the gray `0`.
    pub fn new() -> PyResult<PDFGraphicState> {
        // the class is called: __init__ runs from C and returns
        c_called_leaf()?;
        Ok(PDFGraphicState::initial())
    }

    // pdfminer/pdfinterp.py:155 PDFGraphicState.copy
    pub fn copy(&self) -> PyResult<PDFGraphicState> {
        let _frame = py_frame()?;
        let mut obj = PDFGraphicState::new()?;
        obj.linewidth = self.linewidth.clone();
        obj.linecap = self.linecap.clone();
        obj.linejoin = self.linejoin.clone();
        obj.miterlimit = self.miterlimit.clone();
        obj.dash = self.dash.clone();
        obj.intent = self.intent.clone();
        obj.flatness = self.flatness.clone();
        obj.scolor = self.scolor.clone();
        obj.scs = self.scs.clone();
        obj.ncolor = self.ncolor.clone();
        obj.ncs = self.ncs.clone();
        Ok(obj)
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

impl PDFResourceManager {
    // pdfminer/pdfinterp.py:193 PDFResourceManager.__init__
    /// The resource manager (its class call happens where pdfplumber opens
    /// the document, far from any recursion limit: not counted).
    pub fn new(caching: bool) -> PDFResourceManager {
        PDFResourceManager { caching, _cached_fonts: RefCell::new(IndexMap::default()) }
    }

    // pdfminer/pdfinterp.py:197 PDFResourceManager.get_procset
    /// Does nothing with the items (`list_value(v)` of `/ProcSet`): its
    /// loop only compares them to `LITERAL_PDF` and `LITERAL_TEXT`.
    pub fn get_procset(&self, procs: &[PDFObject]) -> PyResult<()> {
        leaf(1)?;
        for proc in procs {
            let _ = matches!(proc, PDFObject::Literal(l) if *l == *LITERAL_PDF || *l == *LITERAL_TEXT);
        }
        Ok(())
    }

    // pdfminer/pdfinterp.py:204 PDFResourceManager.get_cmap
    /// The predefined CMap `cmapname`, or an empty `CMap()` when it does
    /// not exist (unless `strict`). pdfminer itself never calls it.
    pub fn get_cmap(&self, cmapname: &str, strict: bool) -> PyResult<Rc<dyn CMapBase>> {
        let _frame = py_frame()?;
        match CMapDB::get_cmap(cmapname) {
            Err(e) if e.is_instance(ExcKind::CMapDB_CMapNotFound) => {
                if strict {
                    return Err(e);
                }
                // CMap(): CMap.__init__ (from C), then CMapBase.__init__
                let _entry = eval_entry()?;
                let _init = py_frame()?;
                leaf(1)?;
                Ok(Rc::new(CMap::new(Attrs::default())))
            }
            other => other,
        }
    }

    // pdfminer/pdfinterp.py:212 PDFResourceManager.get_font
    /// The cached font of `objid` (when truthy and cached), else
    /// `pdffont::get_font(spec)` (which takes `get_font`'s frame and
    /// creates the font, `Type0` included), cached when `objid` is truthy
    /// and `caching` is on. `init_resources` passes the `PDFObjRef`'s
    /// `objid` of a font given by reference, `None` for a direct
    /// dictionary; `do_Tf` passes `(None, {})` for an unknown font name.
    pub fn get_font(&self, objid: Option<&PyInt>, spec: &PDFDict) -> PyResult<Rc<PDFFont>> {
        let objid = objid.filter(|id| !id.is_zero());
        if let Some(id) = objid {
            let cached = self._cached_fonts.borrow().get(id).cloned();
            if let Some(font) = cached {
                // the frame of get_font, which returns the cached font
                leaf(1)?;
                return Ok(font);
            }
        }
        // log.debug("get_font: create: ...") and the creation are
        // pdffont::get_font's, which takes this method's frame
        let font = Rc::new(crate::pdffont::get_font(spec)?);
        if let Some(id) = objid {
            if self.caching {
                self._cached_fonts.borrow_mut().insert(id.clone(), font.clone());
            }
        }
        Ok(font)
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

/// `self.resources.copy()` of `do_Do` for a form without `/Resources`:
/// the resources the interpreter was given are any object (a page's
/// `/Resources`, resolved, unchecked): a dict or a list copies, anything
/// else has no `copy` (`AttributeError`).
fn copy_resources(resources: &PDFObject) -> PyResult<PDFObject> {
    match resources {
        PDFObject::Dict(d) => Ok(PDFObject::Dict(d.copy())),
        PDFObject::List(l) => Ok(PDFObject::List(PDFList::new(l.to_vec()))),
        other => Err(exc::AttributeError(format!("'{}' object has no attribute 'copy'", other.type_name()))),
    }
}

// pdfminer/pdfinterp.py:397 PDFPageInterpreter.init_resources (its nested get_colorspace, at line 406)
/// The color space of a `/ColorSpace` resource (resolved): an `ICCBased`
/// space with the stream's raw `/N`, a `DeviceN` space with as many
/// components as its names, or a predefined space by name (`None` when
/// unknown). A list's first item and a stream's `/N` are read unchecked:
/// `IndexError`, `PDFKeyError`.
fn get_colorspace(spec: &PDFObject) -> PyResult<Option<Rc<PDFColorSpace>>> {
    let _frame = py_frame()?;
    let name = match spec {
        PDFObject::List(l) => literal_name(&l.get(0).ok_or_else(|| exc::IndexError("list index out of range"))?)?,
        other => literal_name(other)?,
    };
    if let PDFObject::List(l) = spec {
        if l.len() >= 2 {
            let second = l.get(1).expect("two items");
            if name == "ICCBased" {
                let ncomponents = stream_value(&second)?.getitem("N")?;
                // PDFColorSpace(...): __init__ called from C
                c_called_leaf()?;
                return Ok(Some(Rc::new(PDFColorSpace::new(name, ncomponents))));
            }
            if name == "DeviceN" {
                let n = list_value(&second)?.len();
                c_called_leaf()?;
                return Ok(Some(Rc::new(PDFColorSpace::new(name, PDFObject::Int(PyInt::Small(n as i64))))));
            }
        }
    }
    Ok(predefined_colorspace(&name))
}

/// An operator: a `do_*` method of `PDFPageInterpreter` (the name after
/// `do_`, as `execute` builds it from a keyword).
#[allow(non_camel_case_types)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    q,
    Q,
    cm,
    w,
    J,
    j,
    M,
    d,
    ri,
    i,
    gs,
    m,
    l,
    c,
    v,
    y,
    h,
    re,
    S,
    s,
    f,
    F,
    f_a,
    B,
    B_a,
    b,
    b_a,
    n,
    W,
    W_a,
    CS,
    cs,
    G,
    g,
    RG,
    rg,
    K,
    k,
    SCN,
    scn,
    SC,
    sc,
    sh,
    BT,
    ET,
    BX,
    EX,
    MP,
    DP,
    BMC,
    BDC,
    EMC,
    Tc,
    Tw,
    Tz,
    TL,
    Tf,
    Tr,
    Ts,
    Td,
    TD,
    Tm,
    T_a,
    TJ,
    Tj,
    _q,
    _w,
    BI,
    ID,
    EI,
    Do,
}

impl Op {
    /// `hasattr(self, "do_" + method)`: the operator whose method is
    /// `do_<method>`.
    fn from_method(method: &str) -> Option<Op> {
        Some(match method {
            "q" => Op::q,
            "Q" => Op::Q,
            "cm" => Op::cm,
            "w" => Op::w,
            "J" => Op::J,
            "j" => Op::j,
            "M" => Op::M,
            "d" => Op::d,
            "ri" => Op::ri,
            "i" => Op::i,
            "gs" => Op::gs,
            "m" => Op::m,
            "l" => Op::l,
            "c" => Op::c,
            "v" => Op::v,
            "y" => Op::y,
            "h" => Op::h,
            "re" => Op::re,
            "S" => Op::S,
            "s" => Op::s,
            "f" => Op::f,
            "F" => Op::F,
            "f_a" => Op::f_a,
            "B" => Op::B,
            "B_a" => Op::B_a,
            "b" => Op::b,
            "b_a" => Op::b_a,
            "n" => Op::n,
            "W" => Op::W,
            "W_a" => Op::W_a,
            "CS" => Op::CS,
            "cs" => Op::cs,
            "G" => Op::G,
            "g" => Op::g,
            "RG" => Op::RG,
            "rg" => Op::rg,
            "K" => Op::K,
            "k" => Op::k,
            "SCN" => Op::SCN,
            "scn" => Op::scn,
            "SC" => Op::SC,
            "sc" => Op::sc,
            "sh" => Op::sh,
            "BT" => Op::BT,
            "ET" => Op::ET,
            "BX" => Op::BX,
            "EX" => Op::EX,
            "MP" => Op::MP,
            "DP" => Op::DP,
            "BMC" => Op::BMC,
            "BDC" => Op::BDC,
            "EMC" => Op::EMC,
            "Tc" => Op::Tc,
            "Tw" => Op::Tw,
            "Tz" => Op::Tz,
            "TL" => Op::TL,
            "Tf" => Op::Tf,
            "Tr" => Op::Tr,
            "Ts" => Op::Ts,
            "Td" => Op::Td,
            "TD" => Op::TD,
            "Tm" => Op::Tm,
            "T_a" => Op::T_a,
            "TJ" => Op::TJ,
            "Tj" => Op::Tj,
            "_q" => Op::_q,
            "_w" => Op::_w,
            "BI" => Op::BI,
            "ID" => Op::ID,
            "EI" => Op::EI,
            "Do" => Op::Do,
            _ => return None,
        })
    }

    /// `func.__code__.co_argcount - 1`: the method's arguments.
    fn nargs(self) -> usize {
        match self {
            Op::cm | Op::c | Op::Tm => 6,
            Op::v | Op::y | Op::re | Op::K | Op::k => 4,
            Op::RG | Op::rg | Op::_w => 3,
            Op::d | Op::m | Op::l | Op::DP | Op::BDC | Op::Tf | Op::Td | Op::TD => 2,
            Op::w
            | Op::J
            | Op::j
            | Op::M
            | Op::ri
            | Op::i
            | Op::gs
            | Op::CS
            | Op::cs
            | Op::G
            | Op::g
            | Op::sh
            | Op::MP
            | Op::BMC
            | Op::Tc
            | Op::Tw
            | Op::Tz
            | Op::TL
            | Op::Tr
            | Op::Ts
            | Op::TJ
            | Op::Tj
            | Op::_q
            | Op::EI
            | Op::Do => 1,
            Op::q
            | Op::Q
            | Op::h
            | Op::S
            | Op::s
            | Op::f
            | Op::F
            | Op::f_a
            | Op::B
            | Op::B_a
            | Op::b
            | Op::b_a
            | Op::n
            | Op::W
            | Op::W_a
            | Op::SCN
            | Op::scn
            | Op::SC
            | Op::sc
            | Op::BT
            | Op::ET
            | Op::BX
            | Op::EX
            | Op::EMC
            | Op::T_a
            | Op::BI
            | Op::ID => 0,
        }
    }
}

/// `safe_float(o)`, a Python function (its frame is checked: it calls only
/// `float()`).
fn safe_float_counted(o: &PDFObject) -> PyResult<Option<f64>> {
    leaf(1)?;
    Ok(safe_float(o))
}

/// `safe_matrix(...)`: its frame, and those of the `safe_float` calls it
/// makes one after the other.
fn safe_matrix_counted(m: [&PDFObject; 6]) -> PyResult<Option<[f64; 6]>> {
    leaf(2)?;
    Ok(safe_matrix(m[0], m[1], m[2], m[3], m[4], m[5]))
}

/// `safe_rgb(...)`: its frame, `_safe_float_triple`'s and those of the
/// `safe_float` calls.
fn safe_rgb_counted(r: &PDFObject, g: &PDFObject, b: &PDFObject) -> PyResult<Option<[f64; 3]>> {
    leaf(3)?;
    Ok(safe_rgb(r, g, b))
}

/// `safe_cmyk(...)`: its frame, `_safe_float_quadruple`'s and those of the
/// `safe_float` calls.
fn safe_cmyk_counted(c: &PDFObject, m: &PDFObject, y: &PDFObject, k: &PDFObject) -> PyResult<Option<[f64; 4]>> {
    leaf(3)?;
    Ok(safe_cmyk(c, m, y, k))
}

/// The stroking or non-stroking color, for the methods that do the same on
/// either (`G`/`g`, `RG`/`rg`, `K`/`k`, `SCN`/`scn`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Paint {
    Stroke,
    NonStroke,
}

/// The path segment `(op, *coords)` of floats.
fn segment(op: char, coords: &[f64]) -> PathSegment {
    (op, coords.iter().map(|&x| Num::Float(x)).collect())
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
///
/// The CTM is a [`Matrix`] of numbers. Upstream, `do_Do` multiplies the
/// CTM by the form's `/Matrix` unchecked, and a `bytes` or a `list` item
/// would make sequences of the result (repeated by the CTM's ints); the
/// device's `begin_figure` sees the same `/Matrix` first, and pdfplumber's
/// (`LTFigure`) raises `TypeError` for every such matrix
/// (`utils::ObjMatrix`), so the CTM is numbers wherever pdfplumber runs.
/// For a device that would let such a matrix through, the port raises
/// `TypeError` in `do_Do` where Python would go on with a CTM of
/// sequences (a documented limit).
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

#[allow(non_snake_case)] // the operators keep their upstream names
impl PDFPageInterpreter {
    // pdfminer/pdfinterp.py:373 PDFPageInterpreter.__init__
    /// `PDFPageInterpreter(rsrcmgr, device)`: takes the frames of the class
    /// call.
    pub fn new(rsrcmgr: Rc<PDFResourceManager>, device: Rc<RefCell<dyn PDFDevice>>) -> PyResult<PDFPageInterpreter> {
        // the class is called: __init__ runs from C and only assigns
        c_called_leaf()?;
        Ok(PDFPageInterpreter {
            rsrcmgr,
            device,
            // Track stream IDs currently being executed to detect circular references
            stream_ids: HashSet::new(),
            // Track stream IDs from parent interpreters in the call stack
            parent_stream_ids: HashSet::new(),
            inline_images: Rc::new(Cell::new(0)),
            resources: PDFObject::Null,
            fontmap: None,
            xobjmap: IndexMap::default(),
            csmap: IndexMap::default(),
            gstack: Vec::new(),
            ctm: matrix_identity(),
            textstate: PDFTextState::initial(),
            graphicstate: PDFGraphicState::initial(),
            curpath: Vec::new(),
            argstack: Vec::new(),
        })
    }

    // pdfminer/pdfinterp.py:381 PDFPageInterpreter.dup
    /// `self.__class__(self.rsrcmgr, self.device)`, which shares the
    /// inline images' counter (not upstream, see [`inline_image_name`]).
    pub fn dup(&self) -> PyResult<PDFPageInterpreter> {
        let _frame = py_frame()?;
        let mut interp = PDFPageInterpreter::new(self.rsrcmgr.clone(), self.device.clone())?;
        interp.inline_images = self.inline_images.clone();
        Ok(interp)
    }

    // pdfminer/pdfinterp.py:384 PDFPageInterpreter.subinterp
    /// A sub-interpreter for a Form XObject: `dup()`, with the parent's
    /// stream ids.
    pub fn subinterp(&self) -> PyResult<PDFPageInterpreter> {
        let _frame = py_frame()?;
        let mut interp = self.dup()?;
        interp.parent_stream_ids.extend(self.parent_stream_ids.iter().cloned());
        interp.parent_stream_ids.extend(self.stream_ids.iter().cloned());
        Ok(interp)
    }

    // pdfminer/pdfinterp.py:397 PDFPageInterpreter.init_resources
    /// Prepares the fonts (`rsrcmgr.get_font`), color spaces and XObjects
    /// listed in `resources`.
    pub fn init_resources(&mut self, resources: &PDFObject) -> PyResult<()> {
        let _frame = py_frame()?;
        self.resources = resources.clone();
        self.fontmap = Some(IndexMap::default());
        self.xobjmap = IndexMap::default();
        self.csmap = predefined_colorspaces().into_iter().map(|cs| (Rc::from(cs.name.as_str()), cs)).collect();
        if !resources.is_truthy() {
            return Ok(());
        }
        for (k, v) in dict_value(resources)?.items() {
            // log.debug("Resource: %r: %r", k, v)
            LOG.debug()?;
            match &*k {
                "Font" => {
                    for (fontid, spec) in dict_value(&v)?.items() {
                        let objid = match &spec {
                            PDFObject::Ref(r) => Some(r.objid().clone()),
                            _ => None,
                        };
                        let spec = dict_value(&spec)?;
                        let font = self.rsrcmgr.get_font(objid.as_ref(), &spec)?;
                        self.fontmap.as_mut().expect("set above").insert(fontid, font);
                    }
                }
                "ColorSpace" => {
                    for (csid, spec) in dict_value(&v)?.items() {
                        let colorspace = get_colorspace(&resolve1_none(&spec)?)?;
                        if let Some(colorspace) = colorspace {
                            self.csmap.insert(csid, colorspace);
                        }
                    }
                }
                "ProcSet" => self.rsrcmgr.get_procset(&list_value(&v)?.to_vec())?,
                "XObject" => {
                    for (xobjid, xobjstrm) in dict_value(&v)?.items() {
                        self.xobjmap.insert(xobjid, xobjstrm);
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    // pdfminer/pdfinterp.py:438 PDFPageInterpreter.init_state
    /// New text and graphic states, `ctm`, `device.set_ctm(ctm)`.
    pub fn init_state(&mut self, ctm: Matrix) -> PyResult<()> {
        let _frame = py_frame()?;
        // gstack: stack for graphical states.
        self.gstack = Vec::new();
        self.ctm = ctm.clone();
        self.device.borrow_mut().set_ctm(ctm)?;
        self.textstate = PDFTextState::new()?;
        self.graphicstate = PDFGraphicState::new()?;
        self.curpath = Vec::new();
        // argstack: stack for command arguments.
        self.argstack = Vec::new();
        Ok(())
    }

    // pdfminer/pdfinterp.py:450 PDFPageInterpreter.push
    pub fn push(&mut self, obj: PDFStackT) -> PyResult<()> {
        leaf(1)?;
        self.argstack.push(obj);
        Ok(())
    }

    // pdfminer/pdfinterp.py:453 PDFPageInterpreter.pop
    /// The last `n` operands (all of them when fewer), removed.
    pub fn pop(&mut self, n: usize) -> PyResult<Vec<PDFStackT>> {
        leaf(1)?;
        if n == 0 {
            return Ok(Vec::new());
        }
        let at = self.argstack.len().saturating_sub(n);
        Ok(self.argstack.split_off(at))
    }

    // pdfminer/pdfinterp.py:453 PDFPageInterpreter.pop
    /// `self.pop(n)` for the `n` of `do_SCN`/`do_scn`: a color space's
    /// `ncomponents`, which is any object for an `ICCBased` space (its raw
    /// `/N`). Python's operators apply: `n == 0` returns `[]` (`0`, `0.0`,
    /// `False`), `-n` raises `TypeError` for a non-number, a float is not
    /// a slice index (`TypeError`), and an int, a bool included, slices
    /// `argstack[-n:]` and `argstack[:-n]` with Python's clamping (a
    /// negative `n` takes the operands from index `-n` on).
    fn pop_obj(&mut self, n: &PDFObject) -> PyResult<Vec<PDFStackT>> {
        leaf(1)?;
        let neg = match n {
            PDFObject::Int(i) if i.is_zero() => return Ok(Vec::new()),
            PDFObject::Float(f) if *f == 0.0 => return Ok(Vec::new()),
            PDFObject::Bool(false) => return Ok(Vec::new()),
            PDFObject::Int(i) => i.neg(),
            PDFObject::Bool(true) => PyInt::Small(-1),
            PDFObject::Float(_) => {
                return Err(exc::TypeError("slice indices must be integers or None or have an __index__ method"))
            }
            other => return Err(exc::TypeError(format!("bad operand type for unary -: '{}'", other.type_name()))),
        };
        // the slice index -n, normalized as list slicing does
        let len = self.argstack.len();
        let at = match neg.as_i64() {
            Some(v) if v < 0 => (len as i64 + v).max(0) as usize,
            Some(v) => (v as u64).min(len as u64) as usize,
            None if neg < PyInt::ZERO => 0,
            None => len,
        };
        Ok(self.argstack.split_off(at))
    }

    // pdfminer/pdfinterp.py:460 PDFPageInterpreter.get_current_state
    pub fn get_current_state(&self) -> PyResult<(Matrix, PDFTextState, PDFGraphicState)> {
        let _frame = py_frame()?;
        Ok((self.ctm.clone(), self.textstate.copy()?, self.graphicstate.copy()?))
    }

    // pdfminer/pdfinterp.py:463 PDFPageInterpreter.set_current_state
    pub fn set_current_state(&mut self, state: (Matrix, PDFTextState, PDFGraphicState)) -> PyResult<()> {
        let _frame = py_frame()?;
        (self.ctm, self.textstate, self.graphicstate) = state;
        self.device.borrow_mut().set_ctm(self.ctm.clone())
    }

    // pdfminer/pdfinterp.py:470 PDFPageInterpreter.do_q
    /// Save graphics state
    pub fn do_q(&mut self) -> PyResult<()> {
        let _frame = py_frame()?;
        let state = self.get_current_state()?;
        self.gstack.push(state);
        Ok(())
    }

    // pdfminer/pdfinterp.py:474 PDFPageInterpreter.do_Q
    /// Restore graphics state
    pub fn do_Q(&mut self) -> PyResult<()> {
        let _frame = py_frame()?;
        if let Some(state) = self.gstack.pop() {
            self.set_current_state(state)?;
        }
        Ok(())
    }

    // pdfminer/pdfinterp.py:479 PDFPageInterpreter.do_cm
    /// Concatenate matrix to current transformation matrix
    pub fn do_cm(
        &mut self,
        a1: &PDFStackT,
        b1: &PDFStackT,
        c1: &PDFStackT,
        d1: &PDFStackT,
        e1: &PDFStackT,
        f1: &PDFStackT,
    ) -> PyResult<()> {
        let _frame = py_frame()?;
        match safe_matrix_counted([a1, b1, c1, d1, e1, f1])? {
            // log.warning("Cannot concatenate matrix to current transformation matrix ...")
            None => LOG.warning()?,
            Some(matrix) => {
                // utils.mult_matrix: a Python function
                leaf(1)?;
                self.ctm = mult_matrix(&matrix.map(Num::Float), &self.ctm)?;
                self.device.borrow_mut().set_ctm(self.ctm.clone())?;
            }
        }
        Ok(())
    }

    // pdfminer/pdfinterp.py:501 PDFPageInterpreter.do_w
    /// Set line width
    pub fn do_w(&mut self, linewidth: &PDFStackT) -> PyResult<()> {
        let _frame = py_frame()?;
        match safe_float_counted(linewidth)? {
            // log.warning("Cannot set line width because %r is an invalid float value", ...)
            None => LOG.warning()?,
            Some(linewidth_f) => {
                let two = int(2);
                let scale = self.ctm[0].pow(&two)?.add(&self.ctm[1].pow(&two)?)?.pow(&Num::Float(0.5))?;
                self.graphicstate.linewidth = Num::Float(linewidth_f).mul(&scale)?;
            }
        }
        Ok(())
    }

    // pdfminer/pdfinterp.py:513 PDFPageInterpreter.do_J
    /// Set line cap style
    pub fn do_J(&mut self, linecap: &PDFStackT) -> PyResult<()> {
        leaf(1)?;
        self.graphicstate.linecap = linecap.clone();
        Ok(())
    }

    // pdfminer/pdfinterp.py:517 PDFPageInterpreter.do_j
    /// Set line join style
    pub fn do_j(&mut self, linejoin: &PDFStackT) -> PyResult<()> {
        leaf(1)?;
        self.graphicstate.linejoin = linejoin.clone();
        Ok(())
    }

    // pdfminer/pdfinterp.py:521 PDFPageInterpreter.do_M
    /// Set miter limit
    pub fn do_M(&mut self, miterlimit: &PDFStackT) -> PyResult<()> {
        leaf(1)?;
        self.graphicstate.miterlimit = miterlimit.clone();
        Ok(())
    }

    // pdfminer/pdfinterp.py:525 PDFPageInterpreter.do_d
    /// Set line dash pattern
    pub fn do_d(&mut self, dash: &PDFStackT, phase: &PDFStackT) -> PyResult<()> {
        leaf(1)?;
        self.graphicstate.dash = Some((dash.clone(), phase.clone()));
        Ok(())
    }

    // pdfminer/pdfinterp.py:529 PDFPageInterpreter.do_ri
    /// Set color rendering intent
    pub fn do_ri(&mut self, intent: &PDFStackT) -> PyResult<()> {
        leaf(1)?;
        self.graphicstate.intent = intent.clone();
        Ok(())
    }

    // pdfminer/pdfinterp.py:533 PDFPageInterpreter.do_i
    /// Set flatness tolerance
    pub fn do_i(&mut self, flatness: &PDFStackT) -> PyResult<()> {
        leaf(1)?;
        self.graphicstate.flatness = flatness.clone();
        Ok(())
    }

    // pdfminer/pdfinterp.py:537 PDFPageInterpreter.do_gs
    /// Set parameters from graphics state parameter dictionary (a `TODO`
    /// upstream: nothing)
    pub fn do_gs(&mut self, _name: &PDFStackT) -> PyResult<()> {
        leaf(1)
    }

    /// `do_m`, `do_l`, `do_c`, `do_v`, `do_y`: the operands as floats
    /// appended as the segment `(op, ...)`, or a warning.
    fn path_segment(&mut self, op: char, operands: &[&PDFStackT]) -> PyResult<()> {
        let mut coords = Vec::with_capacity(operands.len());
        for x in operands {
            coords.push(safe_float_counted(x)?);
        }
        match coords.into_iter().collect::<Option<Vec<f64>>>() {
            // log.warning("Cannot ... because not all values in %r can be parsed as floats", point)
            None => LOG.warning()?,
            Some(coords) => self.curpath.push(segment(op, &coords)),
        }
        Ok(())
    }

    // pdfminer/pdfinterp.py:541 PDFPageInterpreter.do_m
    /// Begin new subpath
    pub fn do_m(&mut self, x: &PDFStackT, y: &PDFStackT) -> PyResult<()> {
        let _frame = py_frame()?;
        self.path_segment('m', &[x, y])
    }

    // pdfminer/pdfinterp.py:557 PDFPageInterpreter.do_l
    /// Append straight line segment to path
    pub fn do_l(&mut self, x: &PDFStackT, y: &PDFStackT) -> PyResult<()> {
        let _frame = py_frame()?;
        self.path_segment('l', &[x, y])
    }

    // pdfminer/pdfinterp.py:572 PDFPageInterpreter.do_c
    /// Append curved segment to path (three control points)
    pub fn do_c(
        &mut self,
        x1: &PDFStackT,
        y1: &PDFStackT,
        x2: &PDFStackT,
        y2: &PDFStackT,
        x3: &PDFStackT,
        y3: &PDFStackT,
    ) -> PyResult<()> {
        let _frame = py_frame()?;
        self.path_segment('c', &[x1, y1, x2, y2, x3, y3])
    }

    // pdfminer/pdfinterp.py:606 PDFPageInterpreter.do_v
    /// Append curved segment to path (initial point replicated)
    pub fn do_v(&mut self, x2: &PDFStackT, y2: &PDFStackT, x3: &PDFStackT, y3: &PDFStackT) -> PyResult<()> {
        let _frame = py_frame()?;
        self.path_segment('v', &[x2, y2, x3, y3])
    }

    // pdfminer/pdfinterp.py:623 PDFPageInterpreter.do_y
    /// Append curved segment to path (final point replicated)
    pub fn do_y(&mut self, x1: &PDFStackT, y1: &PDFStackT, x3: &PDFStackT, y3: &PDFStackT) -> PyResult<()> {
        let _frame = py_frame()?;
        self.path_segment('y', &[x1, y1, x3, y3])
    }

    // pdfminer/pdfinterp.py:640 PDFPageInterpreter.do_h
    /// Close subpath
    pub fn do_h(&mut self) -> PyResult<()> {
        leaf(1)?;
        self.curpath.push(('h', Vec::new()));
        Ok(())
    }

    // pdfminer/pdfinterp.py:644 PDFPageInterpreter.do_re
    /// Append rectangle to path
    pub fn do_re(&mut self, x: &PDFStackT, y: &PDFStackT, w: &PDFStackT, h: &PDFStackT) -> PyResult<()> {
        let _frame = py_frame()?;
        let x_f = safe_float_counted(x)?;
        let y_f = safe_float_counted(y)?;
        let w_f = safe_float_counted(w)?;
        let h_f = safe_float_counted(h)?;
        match (x_f, y_f, w_f, h_f) {
            (Some(x_f), Some(y_f), Some(w_f), Some(h_f)) => {
                self.curpath.push(segment('m', &[x_f, y_f]));
                self.curpath.push(segment('l', &[x_f + w_f, y_f]));
                self.curpath.push(segment('l', &[x_f + w_f, y_f + h_f]));
                self.curpath.push(segment('l', &[x_f, y_f + h_f]));
                self.curpath.push(('h', Vec::new()));
            }
            // log.warning("Cannot append rectangle to path because ...")
            _ => LOG.warning()?,
        }
        Ok(())
    }

    /// `self.device.paint_path(self.graphicstate, stroke, fill, evenodd,
    /// self.curpath)`, then `self.curpath = []`.
    fn paint(&mut self, stroke: bool, fill: bool, evenodd: bool) -> PyResult<()> {
        self.device.borrow_mut().paint_path(&self.graphicstate, stroke, fill, evenodd, &self.curpath)?;
        self.curpath = Vec::new();
        Ok(())
    }

    // pdfminer/pdfinterp.py:665 PDFPageInterpreter.do_S
    /// Stroke path
    pub fn do_S(&mut self) -> PyResult<()> {
        let _frame = py_frame()?;
        self.paint(true, false, false)
    }

    // pdfminer/pdfinterp.py:670 PDFPageInterpreter.do_s
    /// Close and stroke path
    pub fn do_s(&mut self) -> PyResult<()> {
        let _frame = py_frame()?;
        self.do_h()?;
        self.do_S()
    }

    // pdfminer/pdfinterp.py:676 PDFPageInterpreter.do_f
    /// Fill path using nonzero winding number rule
    pub fn do_f(&mut self) -> PyResult<()> {
        let _frame = py_frame()?;
        self.paint(false, true, false)
    }

    // pdfminer/pdfinterp.py:680 PDFPageInterpreter.do_F
    /// Fill path using nonzero winding number rule (obsolete): does
    /// nothing upstream.
    pub fn do_F(&mut self) -> PyResult<()> {
        leaf(1)
    }

    // pdfminer/pdfinterp.py:683 PDFPageInterpreter.do_f_a
    /// Fill path using even-odd rule
    pub fn do_f_a(&mut self) -> PyResult<()> {
        let _frame = py_frame()?;
        self.paint(false, true, true)
    }

    // pdfminer/pdfinterp.py:688 PDFPageInterpreter.do_B
    /// Fill and stroke path using nonzero winding number rule
    pub fn do_B(&mut self) -> PyResult<()> {
        let _frame = py_frame()?;
        self.paint(true, true, false)
    }

    // pdfminer/pdfinterp.py:693 PDFPageInterpreter.do_B_a
    /// Fill and stroke path using even-odd rule
    pub fn do_B_a(&mut self) -> PyResult<()> {
        let _frame = py_frame()?;
        self.paint(true, true, true)
    }

    // pdfminer/pdfinterp.py:698 PDFPageInterpreter.do_b
    /// Close, fill, and stroke path using nonzero winding number rule
    pub fn do_b(&mut self) -> PyResult<()> {
        let _frame = py_frame()?;
        self.do_h()?;
        self.do_B()
    }

    // pdfminer/pdfinterp.py:703 PDFPageInterpreter.do_b_a
    /// Close, fill, and stroke path using even-odd rule
    pub fn do_b_a(&mut self) -> PyResult<()> {
        let _frame = py_frame()?;
        self.do_h()?;
        self.do_B_a()
    }

    // pdfminer/pdfinterp.py:708 PDFPageInterpreter.do_n
    /// End path without filling or stroking
    pub fn do_n(&mut self) -> PyResult<()> {
        leaf(1)?;
        self.curpath = Vec::new();
        Ok(())
    }

    // pdfminer/pdfinterp.py:712 PDFPageInterpreter.do_W
    /// Set clipping path using nonzero winding number rule
    pub fn do_W(&mut self) -> PyResult<()> {
        leaf(1)
    }

    // pdfminer/pdfinterp.py:715 PDFPageInterpreter.do_W_a
    /// Set clipping path using even-odd rule
    pub fn do_W_a(&mut self) -> PyResult<()> {
        leaf(1)
    }

    /// `do_CS`/`do_cs`: `self.csmap[literal_name(name)]`, nothing when it
    /// is not there (`KeyError`, `STRICT = False`).
    fn set_colorspace(&mut self, paint: Paint, name: &PDFStackT) -> PyResult<()> {
        let _frame = py_frame()?;
        let name = literal_name(name)?;
        if let Some(cs) = self.csmap.get(name.as_str()).cloned() {
            match paint {
                Paint::Stroke => self.graphicstate.scs = cs,
                Paint::NonStroke => self.graphicstate.ncs = cs,
            }
        }
        Ok(())
    }

    // pdfminer/pdfinterp.py:718 PDFPageInterpreter.do_CS
    /// Set color space for stroking operations (PDF 1.1)
    pub fn do_CS(&mut self, name: &PDFStackT) -> PyResult<()> {
        self.set_colorspace(Paint::Stroke, name)
    }

    // pdfminer/pdfinterp.py:729 PDFPageInterpreter.do_cs
    /// Set color space for nonstroking operations
    pub fn do_cs(&mut self, name: &PDFStackT) -> PyResult<()> {
        self.set_colorspace(Paint::NonStroke, name)
    }

    /// `self.csmap[name]` of a predefined name, which `init_resources`
    /// never removes (a resource may replace it).
    fn predefined(&self, name: &str) -> Rc<PDFColorSpace> {
        self.csmap.get(name).cloned().expect("csmap holds the predefined color spaces")
    }

    /// Sets the stroking or non-stroking color and color space.
    fn set_color(&mut self, paint: Paint, color: StandardColor, csname: &str) {
        let cs = self.predefined(csname);
        let color = Color::Standard(color);
        match paint {
            Paint::Stroke => {
                self.graphicstate.scolor = color;
                self.graphicstate.scs = cs;
            }
            Paint::NonStroke => {
                self.graphicstate.ncolor = color;
                self.graphicstate.ncs = cs;
            }
        }
    }

    /// `do_G`/`do_g`.
    fn set_gray(&mut self, paint: Paint, gray: &PDFStackT) -> PyResult<()> {
        let _frame = py_frame()?;
        match safe_float_counted(gray)? {
            // log.warning("Cannot set gray level because %r is an invalid float value", gray)
            None => LOG.warning()?,
            Some(gray_f) => self.set_color(paint, StandardColor::Gray(Num::Float(gray_f)), "DeviceGray"),
        }
        Ok(())
    }

    // pdfminer/pdfinterp.py:737 PDFPageInterpreter.do_G
    /// Set gray level for stroking operations
    pub fn do_G(&mut self, gray: &PDFStackT) -> PyResult<()> {
        self.set_gray(Paint::Stroke, gray)
    }

    // pdfminer/pdfinterp.py:750 PDFPageInterpreter.do_g
    /// Set gray level for nonstroking operations
    pub fn do_g(&mut self, gray: &PDFStackT) -> PyResult<()> {
        self.set_gray(Paint::NonStroke, gray)
    }

    /// `do_RG`/`do_rg`.
    fn set_rgb(&mut self, paint: Paint, r: &PDFStackT, g: &PDFStackT, b: &PDFStackT) -> PyResult<()> {
        let _frame = py_frame()?;
        match safe_rgb_counted(r, g, b)? {
            // log.warning("Cannot set RGB stroke color because ...")
            None => LOG.warning()?,
            Some(rgb) => self.set_color(paint, StandardColor::Rgb(rgb), "DeviceRGB"),
        }
        Ok(())
    }

    // pdfminer/pdfinterp.py:763 PDFPageInterpreter.do_RG
    /// Set RGB color for stroking operations
    pub fn do_RG(&mut self, r: &PDFStackT, g: &PDFStackT, b: &PDFStackT) -> PyResult<()> {
        self.set_rgb(Paint::Stroke, r, g, b)
    }

    // pdfminer/pdfinterp.py:777 PDFPageInterpreter.do_rg
    /// Set RGB color for nonstroking operations
    pub fn do_rg(&mut self, r: &PDFStackT, g: &PDFStackT, b: &PDFStackT) -> PyResult<()> {
        self.set_rgb(Paint::NonStroke, r, g, b)
    }

    /// `do_K`/`do_k`.
    fn set_cmyk(&mut self, paint: Paint, c: &PDFStackT, m: &PDFStackT, y: &PDFStackT, k: &PDFStackT) -> PyResult<()> {
        let _frame = py_frame()?;
        match safe_cmyk_counted(c, m, y, k)? {
            // log.warning("Cannot set CMYK stroke color because ...")
            None => LOG.warning()?,
            Some(cmyk) => self.set_color(paint, StandardColor::Cmyk(cmyk), "DeviceCMYK"),
        }
        Ok(())
    }

    // pdfminer/pdfinterp.py:791 PDFPageInterpreter.do_K
    /// Set CMYK color for stroking operations
    pub fn do_K(&mut self, c: &PDFStackT, m: &PDFStackT, y: &PDFStackT, k: &PDFStackT) -> PyResult<()> {
        self.set_cmyk(Paint::Stroke, c, m, y, k)
    }

    // pdfminer/pdfinterp.py:805 PDFPageInterpreter.do_k
    /// Set CMYK color for nonstroking operations
    pub fn do_k(&mut self, c: &PDFStackT, m: &PDFStackT, y: &PDFStackT, k: &PDFStackT) -> PyResult<()> {
        self.set_cmyk(Paint::NonStroke, c, m, y, k)
    }

    // pdfminer/pdfinterp.py:819 PDFPageInterpreter._parse_color_components
    /// Parse color components into StandardColor (gray, RGB, or CMYK): 1,
    /// 3 or 4 numeric components, else (or when one is not a number) a
    /// warning and `None`.
    fn _parse_color_components(&self, components: &[PDFStackT]) -> PyResult<Option<StandardColor>> {
        let _frame = py_frame()?;
        let color = match components {
            [gray] => safe_float_counted(gray)?.map(|g| StandardColor::Gray(Num::Float(g))),
            [r, g, b] => safe_rgb_counted(r, g, b)?.map(StandardColor::Rgb),
            [c, m, y, k] => safe_cmyk_counted(c, m, y, k)?.map(StandardColor::Cmyk),
            _ => None,
        };
        if color.is_none() {
            // log.warning("Cannot set %s color: ...")
            LOG.warning()?;
        }
        Ok(color)
    }

    /// The body of `do_SCN` (stroking) and `do_scn` (non-stroking), the
    /// same code upstream but for the state's fields and the messages.
    fn set_scn(&mut self, paint: Paint) -> PyResult<()> {
        let cs = match paint {
            Paint::Stroke => self.graphicstate.scs.clone(),
            Paint::NonStroke => self.graphicstate.ncs.clone(),
        };
        let n = cs.ncomponents.clone();
        let components = self.pop_obj(&n)?;
        // only a number (an int or a bool, or a zero) gets past pop()
        let n_num = n.as_num().expect("pop() raises for a non-number");
        let set = |interp: &mut PDFPageInterpreter, color: Color| match paint {
            Paint::Stroke => interp.graphicstate.scolor = color,
            Paint::NonStroke => interp.graphicstate.ncolor = color,
        };
        if !int(components.len() as i64).py_eq(&n_num) {
            // log.warning("Cannot set stroke color because expected %d components but got %r", ...)
            LOG.warning()?;
        } else if cs.name != "Pattern" {
            // Standard colors (gray, RGB, CMYK) - common case
            if let Some(color) = self._parse_color_components(&components)? {
                set(self, Color::Standard(color));
            }
        } else if let Some(pattern_component) = components.last() {
            // Pattern color space (ISO 32000 8.7.3.2-3)
            // Last component is always the pattern name
            // Per spec: pattern name must be a name object (PSLiteral)
            if !matches!(pattern_component, PDFObject::Literal(_)) {
                // log.warning("Pattern color space requires name object (PSLiteral), ...")
                LOG.warning()?;
                return Ok(());
            }
            let pattern_name = literal_name(pattern_component)?;
            if components.len() == 1 {
                // Colored tiling pattern (PaintType=1): just pattern name
                set(self, Color::Pattern(pattern_name));
                // log.debug("Set stroke pattern (colored): %s", pattern_name)
                LOG.debug()?;
            } else {
                // Uncolored tiling pattern (PaintType=2):
                // color components + pattern name
                let base_color_components = &components[..components.len() - 1];
                // Parse base color using shared logic
                let Some(base_color) = self._parse_color_components(base_color_components)? else {
                    return Ok(());
                };
                // Store as tuple: (base_color, pattern_name)
                set(self, Color::UncoloredPattern(base_color, pattern_name));
                // log.debug("Set stroke pattern (uncolored): %s + %s", base_color, pattern_name)
                LOG.debug()?;
            }
        }
        Ok(())
    }

    // pdfminer/pdfinterp.py:870 PDFPageInterpreter.do_SCN
    /// Set color for stroking operations (patterns: ISO 32000-1:2008 4.5.5,
    /// ISO 32000-2:2020 8.7.3).
    pub fn do_SCN(&mut self) -> PyResult<()> {
        let _frame = py_frame()?;
        self.set_scn(Paint::Stroke)
    }

    // pdfminer/pdfinterp.py:936 PDFPageInterpreter.do_scn
    /// Set color for nonstroking operations.
    pub fn do_scn(&mut self) -> PyResult<()> {
        let _frame = py_frame()?;
        self.set_scn(Paint::NonStroke)
    }

    // pdfminer/pdfinterp.py:1004 PDFPageInterpreter.do_SC
    /// Set color for stroking operations
    pub fn do_SC(&mut self) -> PyResult<()> {
        let _frame = py_frame()?;
        self.do_SCN()
    }

    // pdfminer/pdfinterp.py:1008 PDFPageInterpreter.do_sc
    /// Set color for nonstroking operations
    pub fn do_sc(&mut self) -> PyResult<()> {
        let _frame = py_frame()?;
        self.do_scn()
    }

    // pdfminer/pdfinterp.py:1012 PDFPageInterpreter.do_sh
    /// Paint area defined by shading pattern (nothing)
    pub fn do_sh(&mut self, _name: &PDFStackT) -> PyResult<()> {
        leaf(1)
    }

    // pdfminer/pdfinterp.py:1015 PDFPageInterpreter.do_BT
    /// Begin text object: the text matrix and the text line matrix are
    /// reset to the identity.
    pub fn do_BT(&mut self) -> PyResult<()> {
        let _frame = py_frame()?;
        self.textstate.reset()
    }

    // pdfminer/pdfinterp.py:1024 PDFPageInterpreter.do_ET
    /// End a text object
    pub fn do_ET(&mut self) -> PyResult<()> {
        leaf(1)
    }

    // pdfminer/pdfinterp.py:1027 PDFPageInterpreter.do_BX
    /// Begin compatibility section
    pub fn do_BX(&mut self) -> PyResult<()> {
        leaf(1)
    }

    // pdfminer/pdfinterp.py:1030 PDFPageInterpreter.do_EX
    /// End compatibility section
    pub fn do_EX(&mut self) -> PyResult<()> {
        leaf(1)
    }

    // pdfminer/pdfinterp.py:1033 PDFPageInterpreter.do_MP
    /// Define marked-content point
    pub fn do_MP(&mut self, tag: &PDFStackT) -> PyResult<()> {
        let _frame = py_frame()?;
        match tag {
            PDFObject::Literal(tag) => self.device.borrow_mut().do_tag(tag, None)?,
            // log.warning("Cannot define marked-content point because %r is not a PSLiteral", tag)
            _ => LOG.warning()?,
        }
        Ok(())
    }

    // pdfminer/pdfinterp.py:1043 PDFPageInterpreter.do_DP
    /// Define marked-content point with property list
    pub fn do_DP(&mut self, tag: &PDFStackT, props: &PDFStackT) -> PyResult<()> {
        let _frame = py_frame()?;
        match tag {
            PDFObject::Literal(tag) => self.device.borrow_mut().do_tag(tag, Some(props))?,
            _ => LOG.warning()?,
        }
        Ok(())
    }

    // pdfminer/pdfinterp.py:1054 PDFPageInterpreter.do_BMC
    /// Begin marked-content sequence
    pub fn do_BMC(&mut self, tag: &PDFStackT) -> PyResult<()> {
        let _frame = py_frame()?;
        match tag {
            PDFObject::Literal(tag) => self.device.borrow_mut().begin_tag(tag, None)?,
            _ => LOG.warning()?,
        }
        Ok(())
    }

    // pdfminer/pdfinterp.py:1064 PDFPageInterpreter.do_BDC
    /// Begin marked-content sequence with property list
    pub fn do_BDC(&mut self, tag: &PDFStackT, props: &PDFStackT) -> PyResult<()> {
        let _frame = py_frame()?;
        match tag {
            PDFObject::Literal(tag) => self.device.borrow_mut().begin_tag(tag, Some(props))?,
            _ => LOG.warning()?,
        }
        Ok(())
    }

    // pdfminer/pdfinterp.py:1075 PDFPageInterpreter.do_EMC
    /// End marked-content sequence
    pub fn do_EMC(&mut self) -> PyResult<()> {
        let _frame = py_frame()?;
        self.device.borrow_mut().end_tag()
    }

    /// `safe_float(operand)`, or `None` after the warning of the operator.
    fn text_float(&self, operand: &PDFStackT) -> PyResult<Option<f64>> {
        let value = safe_float_counted(operand)?;
        if value.is_none() {
            // log.warning("Could not set ... because %r is an invalid float value", operand)
            LOG.warning()?;
        }
        Ok(value)
    }

    // pdfminer/pdfinterp.py:1079 PDFPageInterpreter.do_Tc
    /// Set character spacing (unscaled text space units; used by `Tj`,
    /// `TJ` and `'`).
    pub fn do_Tc(&mut self, space: &PDFStackT) -> PyResult<()> {
        let _frame = py_frame()?;
        if let Some(charspace) = self.text_float(space)? {
            self.textstate.charspace = Num::Float(charspace);
        }
        Ok(())
    }

    // pdfminer/pdfinterp.py:1095 PDFPageInterpreter.do_Tw
    /// Set the word spacing (unscaled text space units).
    pub fn do_Tw(&mut self, space: &PDFStackT) -> PyResult<()> {
        let _frame = py_frame()?;
        if let Some(wordspace) = self.text_float(space)? {
            self.textstate.wordspace = Num::Float(wordspace);
        }
        Ok(())
    }

    // pdfminer/pdfinterp.py:1111 PDFPageInterpreter.do_Tz
    /// Set the horizontal scaling (a percentage of the normal width).
    pub fn do_Tz(&mut self, scale: &PDFStackT) -> PyResult<()> {
        let _frame = py_frame()?;
        if let Some(scale_f) = self.text_float(scale)? {
            self.textstate.scaling = Num::Float(scale_f);
        }
        Ok(())
    }

    // pdfminer/pdfinterp.py:1126 PDFPageInterpreter.do_TL
    /// Set the text leading (unscaled text space units; used by `T*`, `'`
    /// and `"`): stored negated.
    pub fn do_TL(&mut self, leading: &PDFStackT) -> PyResult<()> {
        let _frame = py_frame()?;
        if let Some(leading_f) = self.text_float(leading)? {
            self.textstate.leading = Num::Float(-leading_f);
        }
        Ok(())
    }

    // pdfminer/pdfinterp.py:1142 PDFPageInterpreter.do_Tf
    /// Set the text font: the font resource `fontid` (an unknown one is
    /// `rsrcmgr.get_font(None, {})`, a new font each time), and the size.
    pub fn do_Tf(&mut self, fontid: &PDFStackT, fontsize: &PDFStackT) -> PyResult<()> {
        let _frame = py_frame()?;
        let name = literal_name(fontid)?;
        let font = self.fontmap.as_ref().and_then(|m| m.get(name.as_str()).cloned());
        self.textstate.font = Some(match font {
            Some(font) => font,
            None => self.rsrcmgr.get_font(None, &PDFDict::new())?,
        });
        if let Some(fontsize_f) = self.text_float(fontsize)? {
            self.textstate.fontsize = Num::Float(fontsize_f);
        }
        Ok(())
    }

    // pdfminer/pdfinterp.py:1165 PDFPageInterpreter.do_Tr
    /// Set the text rendering mode (`safe_int`, whose `OverflowError`
    /// propagates).
    pub fn do_Tr(&mut self, render: &PDFStackT) -> PyResult<()> {
        let _frame = py_frame()?;
        // safe_int: its frame
        leaf(1)?;
        match safe_int(render)? {
            // log.warning("Could not set text rendering mode because %r is an invalid int value", render)
            None => LOG.warning()?,
            Some(render_i) => self.textstate.render = render_i,
        }
        Ok(())
    }

    // pdfminer/pdfinterp.py:1177 PDFPageInterpreter.do_Ts
    /// Set the text rise (unscaled text space units).
    pub fn do_Ts(&mut self, rise: &PDFStackT) -> PyResult<()> {
        let _frame = py_frame()?;
        if let Some(rise_f) = self.text_float(rise)? {
            self.textstate.rise = Num::Float(rise_f);
        }
        Ok(())
    }

    /// `(a, b, c, d, e, f) = textstate.matrix` translated by `(tx, ty)`:
    /// `e = tx * a + ty * c + e`, `f = tx * b + ty * d + f`.
    fn move_text(&mut self, tx: f64, ty: f64) -> PyResult<()> {
        let [a, b, c, d, e, f] = self.textstate.matrix.clone();
        let (tx, ty) = (Num::Float(tx), Num::Float(ty));
        let e_new = tx.mul(&a)?.add(&ty.mul(&c)?)?.add(&e)?;
        let f_new = tx.mul(&b)?.add(&ty.mul(&d)?)?.add(&f)?;
        self.textstate.matrix = [a, b, c, d, e_new, f_new];
        Ok(())
    }

    // pdfminer/pdfinterp.py:1192 PDFPageInterpreter.do_Td
    /// Move to the start of the next line, offset from the start of the
    /// current line by `(tx, ty)`.
    pub fn do_Td(&mut self, tx: &PDFStackT, ty: &PDFStackT) -> PyResult<()> {
        let _frame = py_frame()?;
        let tx_ = safe_float_counted(tx)?;
        let ty_ = safe_float_counted(ty)?;
        if let (Some(tx_), Some(ty_)) = (tx_, ty_) {
            self.move_text(tx_, ty_)?;
        }
        // elif settings.STRICT: raise PDFValueError(...) (STRICT is False)
        self.textstate.linematrix = origin();
        Ok(())
    }

    // pdfminer/pdfinterp.py:1210 PDFPageInterpreter.do_TD
    /// Move to the start of the next line, offset by `(tx, ty)`; sets the
    /// leading to `ty` (not negated, as upstream).
    pub fn do_TD(&mut self, tx: &PDFStackT, ty: &PDFStackT) -> PyResult<()> {
        let _frame = py_frame()?;
        let tx_ = safe_float_counted(tx)?;
        let ty_ = safe_float_counted(ty)?;
        if let (Some(tx_), Some(ty_)) = (tx_, ty_) {
            self.move_text(tx_, ty_)?;
        }
        if let Some(ty_) = ty_ {
            self.textstate.leading = Num::Float(ty_);
        }
        self.textstate.linematrix = origin();
        Ok(())
    }

    // pdfminer/pdfinterp.py:1233 PDFPageInterpreter.do_Tm
    /// Set text matrix and text line matrix
    pub fn do_Tm(
        &mut self,
        a: &PDFStackT,
        b: &PDFStackT,
        c: &PDFStackT,
        d: &PDFStackT,
        e: &PDFStackT,
        f: &PDFStackT,
    ) -> PyResult<()> {
        let _frame = py_frame()?;
        match safe_matrix_counted([a, b, c, d, e, f])? {
            // log.warning("Could not set text matrix because ...")
            None => LOG.warning()?,
            Some(matrix) => {
                self.textstate.matrix = matrix.map(Num::Float);
                self.textstate.linematrix = origin();
            }
        }
        Ok(())
    }

    // pdfminer/pdfinterp.py:1256 PDFPageInterpreter.do_T_a
    /// Move to start of next text line
    pub fn do_T_a(&mut self) -> PyResult<()> {
        let _frame = py_frame()?;
        let [a, b, c, d, e, f] = self.textstate.matrix.clone();
        let leading = self.textstate.leading.clone();
        let e_new = leading.mul(&c)?.add(&e)?;
        let f_new = leading.mul(&d)?.add(&f)?;
        self.textstate.matrix = [a, b, c, d, e_new, f_new];
        self.textstate.linematrix = origin();
        Ok(())
    }

    // pdfminer/pdfinterp.py:1269 PDFPageInterpreter.do_TJ
    /// Show text, allowing individual glyph positioning: nothing without a
    /// font, else `device.render_string` with a copy of the graphic state.
    pub fn do_TJ(&mut self, seq: &PDFStackT) -> PyResult<()> {
        let _frame = py_frame()?;
        if self.textstate.font.is_none() {
            // if settings.STRICT: raise PDFInterpreterError("No font specified!")
            return Ok(());
        }
        let ncs = self.graphicstate.ncs.clone();
        let graphicstate = Rc::new(self.graphicstate.copy()?);
        self.device.borrow_mut().render_string(&mut self.textstate, seq, &ncs, &graphicstate)
    }

    /// `[s]`
    fn singleton(s: &PDFStackT) -> PDFObject {
        PDFObject::List(PDFList::new(vec![s.clone()]))
    }

    // pdfminer/pdfinterp.py:1282 PDFPageInterpreter.do_Tj
    /// Show text
    pub fn do_Tj(&mut self, s: &PDFStackT) -> PyResult<()> {
        let _frame = py_frame()?;
        self.do_TJ(&Self::singleton(s))
    }

    // pdfminer/pdfinterp.py:1286 PDFPageInterpreter.do__q
    /// Move to next line and show text (the `'` operator)
    pub fn do__q(&mut self, s: &PDFStackT) -> PyResult<()> {
        let _frame = py_frame()?;
        self.do_T_a()?;
        self.do_TJ(&Self::singleton(s))
    }

    // pdfminer/pdfinterp.py:1294 PDFPageInterpreter.do__w
    /// Set word and character spacing, move to next line, and show text
    /// (the `"` operator; upstream does not move to the next line).
    pub fn do__w(&mut self, aw: &PDFStackT, ac: &PDFStackT, s: &PDFStackT) -> PyResult<()> {
        let _frame = py_frame()?;
        self.do_Tw(aw)?;
        self.do_Tc(ac)?;
        self.do_TJ(&Self::singleton(s))
    }

    // pdfminer/pdfinterp.py:1303 PDFPageInterpreter.do_BI
    /// Begin inline image object
    pub fn do_BI(&mut self) -> PyResult<()> {
        leaf(1)
    }

    // pdfminer/pdfinterp.py:1306 PDFPageInterpreter.do_ID
    /// Begin inline image data
    pub fn do_ID(&mut self) -> PyResult<()> {
        leaf(1)
    }

    /// `begin_figure(name, (0, 0, 1, 1), MATRIX_IDENTITY)`,
    /// `render_image(name, stream)`, `end_figure(name)`.
    fn image_figure(&mut self, name: &str, stream: &Rc<PDFStream>) -> PyResult<()> {
        let mut device = self.device.borrow_mut();
        device.begin_figure(name, &ObjSeq::Tuple(unit_rect_objs()), &ObjSeq::Tuple(identity_objs()))?;
        device.render_image(name, stream)?;
        device.end_figure(name)
    }

    // pdfminer/pdfinterp.py:1309 PDFPageInterpreter.do_EI
    /// End inline image object: an image's figure for a stream with `/W`
    /// and `/H`, named [`inline_image_name`] where upstream names it
    /// `str(id(obj))`.
    pub fn do_EI(&mut self, obj: &PDFStackT) -> PyResult<()> {
        let _frame = py_frame()?;
        let PDFObject::Stream(stream) = obj else {
            return Ok(());
        };
        for key in ["W", "H"] {
            // `key in obj`: PDFStream.__contains__, called from C
            c_called_leaf()?;
            if !stream.contains(key) {
                return Ok(());
            }
        }
        let n = self.inline_images.get();
        self.inline_images.set(n + 1);
        let iobjid = inline_image_name(n);
        self.image_figure(&iobjid, stream)
    }

    // pdfminer/pdfinterp.py:1317 PDFPageInterpreter.do_Do
    /// Invoke named XObject: a Form XObject with a `/BBox` is rendered by a
    /// sub-interpreter (with its own `/Resources`, or a copy of the
    /// current ones), an image with `/Width` and `/Height` is an image's
    /// figure, anything else (or an unknown name) is ignored.
    pub fn do_Do(&mut self, xobjid_arg: &PDFStackT) -> PyResult<()> {
        let _frame = py_frame()?;
        let xobjid = literal_name(xobjid_arg)?;
        let Some(entry) = self.xobjmap.get(xobjid.as_str()).cloned() else {
            // if settings.STRICT: raise PDFInterpreterError(...)
            return Ok(());
        };
        let xobj = match stream_value(&entry) {
            Ok(xobj) => xobj,
            // the `except KeyError` around the lookup also catches what
            // resolving the reference raises
            Err(e) if e.is_instance(ExcKind::KeyError) => return Ok(()),
            Err(e) => return Err(e),
        };
        // log.debug("Processing xobj: %r", xobj)
        LOG.debug()?;
        let subtype = xobj.get_counted("Subtype", &PDFObject::Null)?;
        let is = |lit: PSLiteral| matches!(&subtype, PDFObject::Literal(l) if *l == lit);
        // `name in xobj`: PDFStream.__contains__, called from C
        let contains = |name: &str| -> PyResult<bool> {
            c_called_leaf()?;
            Ok(xobj.contains(name))
        };
        if is(*LITERAL_FORM) && contains("BBox")? {
            let mut interpreter = self.subinterp()?;
            let bbox = ObjSeq::List(list_value(&xobj.getitem("BBox")?)?);
            // xobj.get("Matrix", MATRIX_IDENTITY), then list_value() of it
            leaf(1)?;
            let matrix = match xobj.attrs.get("Matrix") {
                Some(matrix) => ObjSeq::List(list_value(&matrix)?),
                None => {
                    // list_value(MATRIX_IDENTITY): its frame and resolve1's
                    leaf(2)?;
                    ObjSeq::Tuple(identity_objs())
                }
            };
            // According to PDF reference 1.7 section 4.9.1, XObjects in
            // earlier PDFs (prior to v1.2) use the page's Resources entry
            // instead of having their own Resources entry.
            let xobjres = xobj.get_counted("Resources", &PDFObject::Null)?;
            let resources =
                if xobjres.is_truthy() { PDFObject::Dict(dict_value(&xobjres)?) } else { copy_resources(&self.resources)? };
            self.device.borrow_mut().begin_figure(&xobjid, &bbox, &matrix)?;
            let streams = PDFObject::List(PDFList::new(vec![PDFObject::Stream(xobj.clone())]));
            // utils.mult_matrix(matrix, self.ctm): a Python function
            leaf(1)?;
            let ctm = mult_matrix_obj(&matrix.items(), &self.ctm)?.to_matrix().ok_or_else(|| {
                exc::TypeError("the port runs a Form XObject only with a CTM of numbers (see PDFPageInterpreter)")
            })?;
            interpreter.render_contents(&resources, &streams, ctm)?;
            self.device.borrow_mut().end_figure(&xobjid)?;
        } else if is(*LITERAL_IMAGE) && contains("Width")? && contains("Height")? {
            self.image_figure(&xobjid, &xobj)?;
        }
        // else: unsupported xobject type.
        Ok(())
    }

    // pdfminer/pdfinterp.py:1352 PDFPageInterpreter.process_page
    /// The page's CTM from its mediabox and rotation, then
    /// `device.begin_page`, `render_contents(page.resources,
    /// page.contents, ctm)`, `device.end_page`.
    pub fn process_page(&mut self, page: &PDFPage) -> PyResult<()> {
        let _frame = py_frame()?;
        // log.debug("Processing page: %r", page)
        LOG.debug()?;
        let [x0, y0, x1, y1] = page.mediabox;
        let float = Num::Float;
        let rotate = page.rotate.as_i64();
        let ctm = if rotate == Some(90) {
            [int(0), int(-1), int(1), int(0), float(-y0), float(x1)]
        } else if rotate == Some(180) {
            [int(-1), int(0), int(0), int(-1), float(x1), float(y1)]
        } else if rotate == Some(270) {
            [int(0), int(1), int(-1), int(0), float(y1), float(-x0)]
        } else {
            [int(1), int(0), int(0), int(1), float(-x0), float(-y0)]
        };
        self.device.borrow_mut().begin_page(page, &ctm)?;
        self.render_contents(&page.resources, &PDFObject::List(page.contents.clone()), ctm)?;
        self.device.borrow_mut().end_page(page)
    }

    // pdfminer/pdfinterp.py:1367 PDFPageInterpreter.render_contents
    /// `init_resources(resources)`, `init_state(ctm)`,
    /// `execute(list_value(streams))`. Called recursively for Form
    /// XObjects (on a sub-interpreter, from `do_Do`).
    pub fn render_contents(&mut self, resources: &PDFObject, streams: &PDFObject, ctm: Matrix) -> PyResult<()> {
        let _frame = py_frame()?;
        // log.debug("render_contents: resources=%r, streams=%r, ctm=%r", ...)
        LOG.debug()?;
        self.init_resources(resources)?;
        self.init_state(ctm)?;
        let streams = list_value(streams)?.to_vec();
        self.execute(&streams)
    }

    /// `func(*args)` (or `func()`): the operator's method.
    fn call(&mut self, op: Op, args: &[PDFStackT]) -> PyResult<()> {
        let a = |i: usize| &args[i];
        match op {
            Op::q => self.do_q(),
            Op::Q => self.do_Q(),
            Op::cm => self.do_cm(a(0), a(1), a(2), a(3), a(4), a(5)),
            Op::w => self.do_w(a(0)),
            Op::J => self.do_J(a(0)),
            Op::j => self.do_j(a(0)),
            Op::M => self.do_M(a(0)),
            Op::d => self.do_d(a(0), a(1)),
            Op::ri => self.do_ri(a(0)),
            Op::i => self.do_i(a(0)),
            Op::gs => self.do_gs(a(0)),
            Op::m => self.do_m(a(0), a(1)),
            Op::l => self.do_l(a(0), a(1)),
            Op::c => self.do_c(a(0), a(1), a(2), a(3), a(4), a(5)),
            Op::v => self.do_v(a(0), a(1), a(2), a(3)),
            Op::y => self.do_y(a(0), a(1), a(2), a(3)),
            Op::h => self.do_h(),
            Op::re => self.do_re(a(0), a(1), a(2), a(3)),
            Op::S => self.do_S(),
            Op::s => self.do_s(),
            Op::f => self.do_f(),
            Op::F => self.do_F(),
            Op::f_a => self.do_f_a(),
            Op::B => self.do_B(),
            Op::B_a => self.do_B_a(),
            Op::b => self.do_b(),
            Op::b_a => self.do_b_a(),
            Op::n => self.do_n(),
            Op::W => self.do_W(),
            Op::W_a => self.do_W_a(),
            Op::CS => self.do_CS(a(0)),
            Op::cs => self.do_cs(a(0)),
            Op::G => self.do_G(a(0)),
            Op::g => self.do_g(a(0)),
            Op::RG => self.do_RG(a(0), a(1), a(2)),
            Op::rg => self.do_rg(a(0), a(1), a(2)),
            Op::K => self.do_K(a(0), a(1), a(2), a(3)),
            Op::k => self.do_k(a(0), a(1), a(2), a(3)),
            Op::SCN => self.do_SCN(),
            Op::scn => self.do_scn(),
            Op::SC => self.do_SC(),
            Op::sc => self.do_sc(),
            Op::sh => self.do_sh(a(0)),
            Op::BT => self.do_BT(),
            Op::ET => self.do_ET(),
            Op::BX => self.do_BX(),
            Op::EX => self.do_EX(),
            Op::MP => self.do_MP(a(0)),
            Op::DP => self.do_DP(a(0), a(1)),
            Op::BMC => self.do_BMC(a(0)),
            Op::BDC => self.do_BDC(a(0), a(1)),
            Op::EMC => self.do_EMC(),
            Op::Tc => self.do_Tc(a(0)),
            Op::Tw => self.do_Tw(a(0)),
            Op::Tz => self.do_Tz(a(0)),
            Op::TL => self.do_TL(a(0)),
            Op::Tf => self.do_Tf(a(0), a(1)),
            Op::Tr => self.do_Tr(a(0)),
            Op::Ts => self.do_Ts(a(0)),
            Op::Td => self.do_Td(a(0), a(1)),
            Op::TD => self.do_TD(a(0), a(1)),
            Op::Tm => self.do_Tm(a(0), a(1), a(2), a(3), a(4), a(5)),
            Op::T_a => self.do_T_a(),
            Op::TJ => self.do_TJ(a(0)),
            Op::Tj => self.do_Tj(a(0)),
            Op::_q => self.do__q(a(0)),
            Op::_w => self.do__w(a(0), a(1), a(2)),
            Op::BI => self.do_BI(),
            Op::ID => self.do_ID(),
            Op::EI => self.do_EI(a(0)),
            Op::Do => self.do_Do(a(0)),
        }
    }

    // pdfminer/pdfinterp.py:1387 PDFPageInterpreter.execute
    /// Skips the streams without an objid and those a parent interpreter
    /// is executing (circular references), then runs the operators of
    /// `PDFContentParser(valid_streams)` until `PSEOF`.
    pub fn execute(&mut self, streams: &[PDFObject]) -> PyResult<()> {
        let _frame = py_frame()?;
        // Detect and prevent circular references in content streams
        // (including Form XObjects): the streams being executed in this
        // interpreter and in its parents (CWE-835).
        let mut valid_streams: Vec<PDFObject> = Vec::new();
        self.stream_ids.clear();
        for obj in streams {
            let stream = stream_value(obj)?;
            let objid = stream.objid().filter(|id| !matches!(id.obj(), PDFObject::Null));
            let Some(objid) = objid else {
                // Inline streams without object IDs can't be tracked for circular refs
                // log.warning("Execute called on non-indirect object (inline image?) %r", stream)
                LOG.warning()?;
                continue;
            };
            if self.parent_stream_ids.contains(&objid) {
                // log.warning("Refusing to execute circular reference to content stream %d", ...)
                LOG.warning()?;
            } else {
                valid_streams.push(PDFObject::Stream(stream));
                self.stream_ids.insert(objid);
            }
        }
        let mut parser = match PDFContentParser::new(valid_streams) {
            Ok(parser) => parser,
            // empty page
            Err(e) if e.is_instance(ExcKind::PSEOF) => return Ok(()),
            Err(e) => return Err(e),
        };
        loop {
            let obj = match parser.nextobject() {
                Ok((_, obj)) => obj,
                Err(e) if e.is_instance(ExcKind::PSEOF) => break,
                Err(e) => return Err(e),
            };
            let PDFObject::Keyword(kwd) = obj else {
                self.push(obj)?;
                continue;
            };
            // keyword_name(obj): its frame
            leaf(1)?;
            let name = decode_utf8_ignore(kwd.name());
            let method = name.replace('*', "_a").replace('"', "_w").replace('\'', "_q");
            // hasattr(self, "do_" + method); elif settings.STRICT: raise
            // PDFInterpreterError(f"Unknown operator: {name!r}")
            let Some(op) = Op::from_method(&method) else {
                continue;
            };
            let nargs = op.nargs();
            if nargs > 0 {
                let args = self.pop(nargs)?;
                // log.debug("exec: %s %r", name, args)
                LOG.debug()?;
                if args.len() == nargs {
                    // func(*args): the bound method called from C
                    // (CALL_FUNCTION_EX), an evaluation loop entry
                    let _entry = eval_entry()?;
                    self.call(op, &args)?;
                }
            } else {
                // log.debug("exec: %s", name)
                LOG.debug()?;
                self.call(op, &[])?;
            }
        }
        Ok(())
    }
}
