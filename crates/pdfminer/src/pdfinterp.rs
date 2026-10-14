//! Port of `pdfminer/pdfinterp.py` (partial).
//!
//! Ported: `PDFContentParser` (the content stream parser, with inline
//! images). Not ported yet: `PDFTextState`, `PDFGraphicState`,
//! `PDFResourceManager`, `PDFPageInterpreter`. The exception classes
//! `PDFResourceError` and `PDFInterpreterError` are
//! `pycompat::ExcKind::{PDFResourceError, PDFInterpreterError}`.

use std::rc::Rc;
use std::sync::LazyLock;

use pycompat::io::BytesIO;
use pycompat::{exc, ExcKind, PyResult};

use crate::pdftypes::{
    in_literals, resolve1_none, stream_value, PDFDict, PDFObject, PDFStream, LITERALS_ASCII85_DECODE,
};
use crate::psparser::{literal_name, Buf, PSKeyword, PSStackParser, PSStackParserExt, BUFSIZ, KWD};
use crate::settings;
use crate::utils::choplist;

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

// pdfminer/pdfinterp.py:330-332 (class attributes)
pub static KEYWORD_BI: LazyLock<PSKeyword> = LazyLock::new(|| KWD(b"BI"));
pub static KEYWORD_ID: LazyLock<PSKeyword> = LazyLock::new(|| KWD(b"ID"));
pub static KEYWORD_EI: LazyLock<PSKeyword> = LazyLock::new(|| KWD(b"EI"));

impl PSStackParser<ContentStreams> {
    // pdfminer/pdfinterp.py:257 PDFContentParser.__init__
    /// `PDFContentParser(streams)`; raises `PSEOF` when there is no stream.
    pub fn new(streams: Vec<PDFObject>) -> PyResult<PDFContentParser> {
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

    // pdfminer/pdfinterp.py:288 PDFContentParser.get_inline_data
    /// Reads the data of an inline image up to `target` followed by a
    /// whitespace (`EI`), or up to `target` (`~>`, for ASCII85 data).
    pub fn get_inline_data(&mut self, pos: usize, target: &[u8]) -> PyResult<(usize, Vec<u8>)> {
        self.seek(pos as i64)?;
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
    // pdfminer/pdfinterp.py:278 PDFContentParser.seek
    fn seek(p: &mut PSStackParser<Self>, pos: i64) -> PyResult<()> {
        p.fillfp()?;
        p.psstackparser_seek(pos)
    }

    // pdfminer/pdfinterp.py:282 PDFContentParser.fillbuf
    fn fillbuf(p: &mut PSStackParser<Self>) -> PyResult<bool> {
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

    // pdfminer/pdfinterp.py:327 PDFContentParser.flush
    fn flush(p: &mut PSStackParser<Self>) -> PyResult<()> {
        // self.add_results(*self.popall()). When `results` is empty (always,
        // from nextobject), the two buffers trade places instead of moving
        // the entries; otherwise the entries are appended. Either way
        // `curstack` keeps a buffer for the next objects.
        let PSStackParser { curstack, results, .. } = p;
        if results.is_empty() {
            let empty: Vec<_> = std::mem::take(results).into();
            *results = std::mem::replace(curstack, empty).into();
        } else {
            results.extend(curstack.drain(..));
        }
        Ok(())
    }

    // pdfminer/pdfinterp.py:334 PDFContentParser.do_keyword
    fn do_keyword(p: &mut PSStackParser<Self>, pos: usize, token: PSKeyword) -> PyResult<()> {
        let [keyword_bi, keyword_id, _] = p.ext.keywords;
        if token == keyword_bi {
            // inline image within a content stream
            p.start_type(pos, "inline");
        } else if token == keyword_id {
            match inline_image(p, pos) {
                Err(e) if e.is_instance(ExcKind::PSTypeError) && !settings::STRICT => {}
                other => other?,
            }
        } else {
            p.push((pos, PDFObject::Keyword(token)));
        }
        Ok(())
    }
}

/// The `try:` block of `do_keyword` for `ID`.
fn inline_image(p: &mut PDFContentParser, pos: usize) -> PyResult<()> {
    let (_, objs) = p.end_type("inline")?;
    if !objs.len().is_multiple_of(2) {
        let error_msg = format!("Invalid dictionary construct: {}", PDFObject::list_repr(&objs));
        return Err(exc::PSTypeError(error_msg));
    }
    let d = PDFDict::new();
    for pair in choplist(2, &objs) {
        d.insert(literal_name(&pair[0]).into(), resolve1_none(&pair[1])?);
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
    let (pos, mut data) = p.get_inline_data(pos + b"ID ".len(), eos)?;
    if eos != b"EI" {
        // it may be necessary for decoding
        data.extend_from_slice(eos);
    }
    let obj = PDFStream::new(d, Rc::from(data), None);
    p.push((pos, PDFObject::Stream(Rc::new(obj))));
    if eos == b"EI" {
        // otherwise it is still in the stream
        p.push((pos, PDFObject::Keyword(p.ext.keywords[2])));
    }
    Ok(())
}
