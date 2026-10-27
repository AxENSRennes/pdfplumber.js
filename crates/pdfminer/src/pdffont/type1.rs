//! `Type1FontHeaderParser` of `pdfminer/pdffont.py`: the encoding of an
//! embedded Type 1 font program (its clear-text header, `/FontFile` up to
//! `/Length1`).
//!
//! Upstream adds `(cid, name)` tuples to `results`, which no other parser
//! does (`docs/architecture.md`, "Reference Resolution And Parser
//! Re-entrance"): the generic parser's results are `(pos, PDFObject)`
//! entries, so each tuple is kept in the parser's own state ([`Type1Header::pairs`], in the same order) and a
//! placeholder entry stands for it in `results`. Only `do_keyword` adds
//! results (`flush` does nothing) and the parser seeks only once, at its
//! creation, so the two queues stay in step.
//!
//! Frames: the construction takes the frame of `__init__`, and
//! `PSStackParser.__init__` takes its own (7 below the caller, 8 the first
//! time the `psparser` logger is asked since its cache was cleared: the
//! seek of every parser fills it); `get_encoding` and `do_keyword` take
//! theirs, and `psparser` counts those of `nextobject` and of everything
//! below it (`docs/architecture.md`, "Recursion Depth").

use std::collections::VecDeque;
use std::sync::LazyLock;

use pycompat::io::{BytesIO, Pos};
use pycompat::{exc, ExcKind, PyInt, PyResult};

use super::frames::{self, Level, Logger};
use crate::encodingdb::Cid2Unicode;
use crate::pdftypes::PDFObject;
use crate::psparser::{literal_name, PSKeyword, PSStackParser, PSStackParserExt, KWD};

// pdfminer/pdffont.py:119-126 Type1FontHeaderParser.KEYWORD_BEGIN, Type1FontHeaderParser.KEYWORD_END,
// Type1FontHeaderParser.KEYWORD_DEF, Type1FontHeaderParser.KEYWORD_PUT, Type1FontHeaderParser.KEYWORD_DICT,
// Type1FontHeaderParser.KEYWORD_ARRAY, Type1FontHeaderParser.KEYWORD_READONLY, Type1FontHeaderParser.KEYWORD_FOR
/// The keywords of the class (only `put` is acted upon).
pub static KEYWORDS: LazyLock<[PSKeyword; 8]> = LazyLock::new(|| {
    [KWD(b"begin"), KWD(b"end"), KWD(b"def"), KWD(b"put"), KWD(b"dict"), KWD(b"array"), KWD(b"readonly"), KWD(b"for")]
});

/// The state `Type1FontHeaderParser` adds to `PSStackParser`.
pub struct Type1Header {
    /// `self._cid2unicode`
    pub cid2unicode: Cid2Unicode,
    /// The `(key, literal_name(value))` tuples added to `results`, in
    /// order: `key` is an `int` (a `bool` passes `isinstance(key, int)`
    /// too, kept as its int, see `encodingdb`).
    pub pairs: VecDeque<(PyInt, String)>,
    /// `KEYWORD_PUT`
    put: PSKeyword,
}

/// pdfminer/pdffont.py:118 `Type1FontHeaderParser`.
pub type Type1FontHeaderParser = PSStackParser<Type1Header>;

impl PSStackParser<Type1Header> {
    // pdfminer/pdffont.py:128 Type1FontHeaderParser.__init__
    /// `Type1FontHeaderParser(data)`: takes the frame of `__init__` and
    /// those of the `PSStackParser.__init__` it calls (a caller
    /// instantiating the class from Python code takes the evaluation loop
    /// entry of `type.__call__`).
    pub fn new(data: BytesIO) -> PyResult<Type1FontHeaderParser> {
        let _frame = frames::frame()?;
        PSStackParser::init(
            Some(data),
            Type1Header { cid2unicode: Cid2Unicode::default(), pairs: VecDeque::new(), put: KEYWORDS[3] },
        )
    }

    // pdfminer/pdffont.py:132 Type1FontHeaderParser.get_encoding
    /// Parses the font encoding: the codes `put` into the encoding array,
    /// mapped to the text of their glyph name (a name that is not a glyph
    /// name is skipped; a malformed `uniXXXX` name raises).
    pub fn get_encoding(&mut self) -> PyResult<Cid2Unicode> {
        let _frame = frames::frame()?;
        loop {
            match self.nextobject() {
                Ok(_) => {}
                Err(e) if e.is_instance(ExcKind::PSEOF) => break,
                Err(e) => return Err(e),
            }
            let (cid, name) = self.ext.pairs.pop_front().expect("a pair for each result");
            let result = frames::name2unicode_counted(&name);
            match result {
                Ok(text) => {
                    self.ext.cid2unicode.insert(cid, text);
                }
                Err(e) if e.is_instance(ExcKind::KeyError) => {
                    // log.debug(str(e))
                    frames::log_frames(Logger::PdfFont, Level::Debug)?;
                }
                Err(e) => return Err(e),
            }
        }
        Ok(std::mem::take(&mut self.ext.cid2unicode))
    }
}

impl PSStackParserExt for Type1Header {
    // pdfminer/pdffont.py:157 Type1FontHeaderParser.do_keyword
    /// Called by `PSStackParser.nextobject` (which counts its frame and its
    /// `log.debug` before the call).
    fn do_keyword(p: &mut PSStackParser<Self>, _pos: Pos, token: PSKeyword) -> PyResult<()> {
        let _frame = frames::frame()?;
        if token == p.ext.put {
            let entries = p.pop(2)?;
            let [(_, key), (_, value)] = <[_; 2]>::try_from(entries).map_err(|entries: Vec<_>| {
                exc::ValueError(format!("not enough values to unpack (expected 2, got {})", entries.len()))
            })?;
            let key = match key {
                PDFObject::Int(i) => i,
                PDFObject::Bool(b) => PyInt::Small(i64::from(b)),
                _ => return Ok(()),
            };
            if let PDFObject::Literal(_) = value {
                let name = literal_name(&value)?;
                p.add_results([(0, PDFObject::Null)])?;
                p.ext.pairs.push_back((key, name));
            }
        }
        Ok(())
    }
}
