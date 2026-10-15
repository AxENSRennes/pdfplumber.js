//! The cross-reference tables of `pdfminer/pdfdocument.py`:
//! `PDFBaseXRef`, `PDFXRef`, `PDFXRefFallback` and `PDFXRefStream`.
//!
//! The three classes are the variants of [`PDFBaseXRef`];
//! `PDFXRefFallback` is a `PDFXRef` loaded by [`PDFXRef::load_fallback`].
//! A `load` reads the file with the document's parser, passed in as
//! upstream does; a resolution on the way (`dict_value` of a trailer given
//! by reference, `get_data` of a stream with indirect filters) lends the
//! parser back to the document ([`crate::pdfparser::lend`]).
//!
//! Keys of `offsets` and the arguments of `get_pos` are [`ObjId`]s: a
//! fallback xref stores the object an object stream holds at an id
//! position as it is, and `PDFXRefStream` computes its ids and positions
//! with Python's operators on whatever `/Index` and `/W` hold
//! ([`super::pyval`]).

use std::rc::Rc;

use indexmap::IndexMap;
use pycompat::recursion::py_frame;
use pycompat::text::str_isspace;
use pycompat::{exc, ExcKind, PyErr, PyInt, PyResult};
use rustc_hash::FxBuildHasher;

use super::pyval::{entry_bounds, slice, PyVal};
use super::LITERAL_OBJSTM;
use super::LITERAL_XREF;
use crate::casting::safe_int;
use crate::pdfparser::{lend, PDFParser, PDFStreamParser};
use crate::pdftypes::{dict_value, ObjId, PDFDict, PDFObject, PyBytes};
use crate::psparser::KWD;
use crate::utils::nunpack;

/// `repr(parser)` in error messages: the default repr, whose address the
/// traces hide (`at 0x?>`).
const PARSER_REPR: &str = "<pdfminer.pdfparser.PDFParser object at 0x0>";

/// A position returned by `get_pos`: `(strmid, index, genno)` for an
/// object in an object stream, `(None, pos, genno)` for an object at a
/// file offset.
#[derive(Clone, Debug, PartialEq)]
pub struct XRefPos {
    pub strmid: Option<PyInt>,
    pub index: PyInt,
    pub genno: PyInt,
}

/// pdfminer/pdfdocument.py:107 `PDFBaseXRef`, as its three subclasses.
pub enum PDFBaseXRef {
    /// `PDFXRef`
    XRef(PDFXRef),
    /// `PDFXRefFallback`
    Fallback(PDFXRef),
    /// `PDFXRefStream`
    Stream(PDFXRefStream),
}

/// What `get_objids()` yields: ids, then possibly the error that stopped
/// the generator.
pub type ObjIds<'a> = Box<dyn Iterator<Item = PyResult<ObjId>> + 'a>;

impl PDFBaseXRef {
    /// `type(xref).__name__`
    pub fn class_name(&self) -> &'static str {
        match self {
            PDFBaseXRef::XRef(_) => "PDFXRef",
            PDFBaseXRef::Fallback(_) => "PDFXRefFallback",
            PDFBaseXRef::Stream(_) => "PDFXRefStream",
        }
    }

    /// `get_trailer()` (pdfminer/pdfdocument.py:196 `PDFXRef.get_trailer`,
    /// :294 `PDFXRefStream.get_trailer`)
    pub fn get_trailer(&self) -> PDFDict {
        match self {
            PDFBaseXRef::XRef(x) | PDFBaseXRef::Fallback(x) => x.get_trailer(),
            PDFBaseXRef::Stream(x) => x.get_trailer(),
        }
    }

    /// `xref.get_objids()` for a generator that outlives the borrow of the
    /// xref (the generator holds it): the same ids.
    pub fn objids(self: &Rc<Self>) -> ObjIds<'static> {
        match &**self {
            PDFBaseXRef::XRef(x) | PDFBaseXRef::Fallback(x) => {
                // the offsets never change once the xref is loaded
                Box::new(x.offsets.keys().cloned().collect::<Vec<_>>().into_iter().map(Ok))
            }
            PDFBaseXRef::Stream(_) => Box::new(XRefStreamIds {
                xref: StreamRef::Owned(self.clone()),
                range: 0,
                current: None,
                done: false,
            }),
        }
    }

    /// `get_objids()`
    pub fn get_objids(&self) -> ObjIds<'_> {
        match self {
            PDFBaseXRef::XRef(x) | PDFBaseXRef::Fallback(x) => x.get_objids(),
            PDFBaseXRef::Stream(x) => x.get_objids(),
        }
    }

    /// `get_pos(objid)`: raises a `KeyError` for an id the table does not
    /// have.
    pub fn get_pos(&self, objid: &ObjId) -> PyResult<XRefPos> {
        match self {
            PDFBaseXRef::XRef(x) | PDFBaseXRef::Fallback(x) => x.get_pos(objid),
            PDFBaseXRef::Stream(x) => x.get_pos(objid),
        }
    }
}

// ---------------------------------------------------------------------------
// PDFXRef

/// pdfminer/pdfdocument.py:124 `PDFXRef` (and :206 `PDFXRefFallback`,
/// which only overrides `load`).
#[derive(Default)]
pub struct PDFXRef {
    /// `self.offsets`
    pub offsets: IndexMap<ObjId, XRefPos, FxBuildHasher>,
    /// `self.trailer`
    pub trailer: PDFDict,
}

/// `bytes.strip()`: ASCII whitespace at both ends.
pub(crate) fn bytes_strip(s: &[u8]) -> &[u8] {
    let ws = |c: &u8| matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c);
    let start = s.iter().position(|c| !ws(c)).unwrap_or(s.len());
    let end = s.iter().rposition(|c| !ws(c)).map_or(start, |e| e + 1);
    &s[start..end]
}

/// `bytes.split(b" ")`
fn split_space(s: &[u8]) -> Vec<&[u8]> {
    s.split(|&c| c == b' ').collect()
}

fn no_valid_xref(msg: impl Into<String>) -> PyErr {
    exc::PDFNoValidXRef(msg)
}

/// `nextline()` with `PSEOF` turned into `PDFNoValidXRef` (`PDFXRef.load`).
fn nextline_or_corrupted(parser: &mut PDFParser) -> PyResult<(pycompat::io::Pos, Vec<u8>)> {
    parser.nextline().map_err(|e| {
        if e.is_instance(ExcKind::PSEOF) {
            no_valid_xref("Unexpected EOF - file corrupted?")
        } else {
            e
        }
    })
}

impl PDFXRef {
    // pdfminer/pdfdocument.py:125 PDFXRef.__init__
    pub fn new() -> PDFXRef {
        PDFXRef::default()
    }

    // pdfminer/pdfdocument.py:132 PDFXRef.load
    pub fn load(&mut self, parser: &mut PDFParser) -> PyResult<()> {
        let _frame = py_frame()?;
        loop {
            let (pos, line) = nextline_or_corrupted(parser)?;
            let line = bytes_strip(&line);
            if line.is_empty() {
                continue;
            }
            if line.starts_with(b"trailer") {
                parser.seek(pos)?;
                break;
            }
            let f = split_space(line);
            if f.len() != 2 {
                let error_msg = format!("Trailer not found: {PARSER_REPR}: line={}", pycompat::repr::bytes_repr(line));
                return Err(no_valid_xref(error_msg));
            }
            let parsed = PyInt::parse_bytes(f[0], 10).and_then(|start| Ok((start, PyInt::parse_bytes(f[1], 10)?)));
            let (start, nobjs) = match parsed {
                Ok(v) => v,
                Err(e) if e.is_instance(ExcKind::ValueError) => {
                    let error_msg = format!("Invalid line: {PARSER_REPR}: line={}", pycompat::repr::bytes_repr(line));
                    return Err(no_valid_xref(error_msg));
                }
                Err(e) => return Err(e),
            };
            let end = start.add(&nobjs);
            let mut objid = start;
            while objid < end {
                let (_, line) = nextline_or_corrupted(parser)?;
                let line = bytes_strip(&line);
                let f = split_space(line);
                if f.len() != 3 {
                    let error_msg =
                        format!("Invalid XRef format: {PARSER_REPR}, line={}", pycompat::repr::bytes_repr(line));
                    return Err(no_valid_xref(error_msg));
                }
                let (pos_b, genno_b, use_b) = (f[0], f[1], f[2]);
                if use_b == b"n" {
                    let pos_i = safe_int(&PDFObject::Bytes(pos_b.into()))?;
                    let genno_i = safe_int(&PDFObject::Bytes(genno_b.into()))?;
                    if let (Some(pos_i), Some(genno_i)) = (pos_i, genno_i) {
                        self.offsets
                            .insert(ObjId::int(objid.clone()), XRefPos { strmid: None, index: pos_i, genno: genno_i });
                    }
                    // else: log.warning("Not adding object %s to xref ...")
                }
                objid = objid.add(&PyInt::Small(1));
            }
        }
        // (log.debug("xref objects: %r", self.offsets))
        self.load_trailer(parser)
    }

    // pdfminer/pdfdocument.py:183 PDFXRef.load_trailer
    pub fn load_trailer(&mut self, parser: &mut PDFParser) -> PyResult<()> {
        let _frame = py_frame()?;
        let attempt = |parser: &mut PDFParser| -> PyResult<PDFObject> {
            let (_, kwd) = parser.nexttoken()?;
            if kwd != PDFObject::Keyword(KWD(b"trailer")) {
                return Err(exc::AssertionError(kwd.py_str()?));
            }
            let (_, dic) = parser.nextobject()?;
            Ok(dic)
        };
        let dic = match attempt(parser) {
            Ok(dic) => dic,
            Err(e) if e.is_instance(ExcKind::PSEOF) => {
                let x = parser.pop(1);
                let Some((_, dic)) = x.into_iter().next() else {
                    return Err(no_valid_xref("Unexpected EOF - file corrupted"));
                };
                dic
            }
            Err(e) => return Err(e),
        };
        let dic = lend(parser, |_| dict_value(&dic))?;
        // self.trailer.update(...)
        for (k, v) in dic.items() {
            self.trailer.insert(k, v);
        }
        // (log.debug("trailer=%r", self.trailer))
        Ok(())
    }

    // pdfminer/pdfdocument.py:196 PDFXRef.get_trailer
    pub fn get_trailer(&self) -> PDFDict {
        self.trailer.clone()
    }

    // pdfminer/pdfdocument.py:199 PDFXRef.get_objids
    pub fn get_objids(&self) -> ObjIds<'_> {
        Box::new(self.offsets.keys().cloned().map(Ok))
    }

    // pdfminer/pdfdocument.py:202 PDFXRef.get_pos
    pub fn get_pos(&self, objid: &ObjId) -> PyResult<XRefPos> {
        match self.offsets.get(objid) {
            Some(pos) => Ok(pos.clone()),
            // str(KeyError(objid)) is repr(objid)
            None => Err(exc::KeyError(objid.repr()?)),
        }
    }

    // pdfminer/pdfdocument.py:212 PDFXRefFallback.load
    /// `PDFXRefFallback.load`: scans the whole file for `n g obj` lines and
    /// the objects of the object streams found there.
    pub fn load_fallback(&mut self, parser: &mut PDFParser) -> PyResult<()> {
        let _frame = py_frame()?;
        parser.seek(0)?;
        loop {
            let (pos, line_bytes) = match parser.nextline() {
                Ok(v) => v,
                Err(e) if e.is_instance(ExcKind::PSEOF) => break,
                Err(e) => return Err(e),
            };
            if line_bytes.starts_with(b"trailer") {
                parser.seek(pos)?;
                self.load_trailer(parser)?;
                // (log.debug("trailer: %r", self.trailer))
                break;
            }
            // line = line_bytes.decode("latin-1"); m = self.PDFOBJ_CUE.match(line)
            let Some((objid_s, genno_s)) = pdfobj_cue(&line_bytes) else {
                continue;
            };
            let objid = PyInt::parse_bytes(objid_s, 10)?;
            let genno = PyInt::parse_bytes(genno_s, 10)?;
            self.offsets
                .insert(ObjId::int(objid.clone()), XRefPos { strmid: None, index: PyInt::Small(pos), genno });
            // expand ObjStm.
            parser.seek(pos)?;
            let (_, obj) = parser.nextobject()?;
            let PDFObject::Stream(stream) = obj else { continue };
            if stream.get("Type", &PDFObject::Null) != PDFObject::Literal(*LITERAL_OBJSTM) {
                continue;
            }
            // stream = stream_value(obj): obj itself
            let n = match stream.getitem("N") {
                Ok(n) => n,
                // with settings.STRICT: PDFSyntaxError(f"N is not defined: {stream!r}")
                Err(e) if e.is_instance(ExcKind::KeyError) => PDFObject::Int(PyInt::ZERO),
                Err(e) => return Err(e),
            };
            let data = lend(parser, |_| stream.get_data())?;
            let mut parser1 = PDFStreamParser::new(data)?;
            let mut objs: Vec<PDFObject> = Vec::new();
            loop {
                match parser1.nextobject() {
                    Ok((_, obj)) => objs.push(obj),
                    Err(e) if e.is_instance(ExcKind::PSEOF) => break,
                    Err(e) => return Err(e),
                }
            }
            // n = min(n, len(objs) // 2): min() keeps n unless half < n
            let half = PyVal::from(PyInt::from(objs.len() / 2));
            let n = PyVal::Obj(n);
            let n = if half.compare(&n, "<")? { half } else { n };
            let n = n.range_arg()?;
            let mut index = PyInt::ZERO;
            while index < n {
                let i = index.as_i64().expect("index < len(objs) // 2") as usize;
                let objid1 = ObjId::new(objs[i * 2].clone())?;
                self.offsets.insert(objid1, XRefPos { strmid: Some(objid.clone()), index: index.clone(), genno: PyInt::ZERO });
                index = index.add(&PyInt::Small(1));
            }
        }
        Ok(())
    }
}

/// `\w` of a `str` pattern on a Latin-1 character: `isalnum()` or `_`.
fn is_word_char(c: u8) -> bool {
    c == b'_' || char::from(c).is_alphanumeric()
}

// pdfminer/pdfdocument.py:210 PDFXRefFallback.PDFOBJ_CUE
/// `PDFOBJ_CUE.match(line_bytes.decode("latin-1"))`, the pattern
/// `^(\d+)\s+(\d+)\s+obj\b` on the Latin-1 characters of the line (one per
/// byte; `\d` is `[0-9]` there, `\s` is `str.isspace()`, `\w` is
/// `isalnum()` or `_`): the two groups. The classes are disjoint, so the
/// greedy runs never backtrack.
pub fn pdfobj_cue(line: &[u8]) -> Option<(&[u8], &[u8])> {
    let digits = |i: usize| i + line[i..].iter().take_while(|c| c.is_ascii_digit()).count();
    let spaces = |i: usize| i + line[i..].iter().take_while(|&&c| str_isspace(char::from(c))).count();
    let a = digits(0);
    if a == 0 {
        return None;
    }
    let s1 = spaces(a);
    if s1 == a {
        return None;
    }
    let b = digits(s1);
    if b == s1 {
        return None;
    }
    let s2 = spaces(b);
    if s2 == b || !line[s2..].starts_with(b"obj") {
        return None;
    }
    // \b after "obj": the next character is not a word character
    if line.get(s2 + 3).is_some_and(|&c| is_word_char(c)) {
        return None;
    }
    Some((&line[..a], &line[s1..b]))
}

// ---------------------------------------------------------------------------
// PDFXRefStream

/// pdfminer/pdfdocument.py:257 `PDFXRefStream`.
#[derive(Default)]
pub struct PDFXRefStream {
    /// `self.data`
    pub data: Option<PyBytes>,
    /// `self.entlen`
    pub entlen: Option<PyVal>,
    /// `self.fl1`, `self.fl2`, `self.fl3`
    pub fl1: Option<PyVal>,
    pub fl2: Option<PyVal>,
    pub fl3: Option<PyVal>,
    /// `self.ranges`
    pub ranges: Vec<(PyVal, PyVal)>,
    /// `self.trailer` (set by `load`)
    pub trailer: Option<PDFDict>,
}

impl PDFXRefStream {
    // pdfminer/pdfdocument.py:258 PDFXRefStream.__init__
    pub fn new() -> PDFXRefStream {
        PDFXRefStream::default()
    }

    // pdfminer/pdfdocument.py:269 PDFXRefStream.load
    pub fn load(&mut self, parser: &mut PDFParser) -> PyResult<()> {
        let _frame = py_frame()?;
        parser.nexttoken()?; // ignored
        parser.nexttoken()?; // ignored
        parser.nexttoken()?;
        let (_, stream) = parser.nextobject()?;
        let stream = match stream {
            PDFObject::Stream(s) if s.get("Type", &PDFObject::Null) == PDFObject::Literal(*LITERAL_XREF) => s,
            _ => return Err(no_valid_xref("Invalid PDF stream spec.")),
        };
        let size = stream.getitem("Size")?;
        let index_array = match stream.attrs.get("Index") {
            Some(index) => PyVal::Obj(index),
            // (0, size): a tuple, iterated as such
            None => PyVal::Obj(PDFObject::List(vec![PDFObject::Int(PyInt::ZERO), size].into())),
        };
        if index_array.len()? % 2 != 0 {
            return Err(exc::PDFSyntaxError("Invalid index number"));
        }
        let items = index_array.items(|t| format!("'{t}' object is not iterable"))?;
        let mut items = items.into_iter();
        while let (Some(start), Some(nobjs)) = (items.next(), items.next()) {
            self.ranges.push((start, nobjs));
        }
        let w = PyVal::Obj(stream.getitem("W")?);
        let fields = w.items(|t| format!("cannot unpack non-iterable {t} object"))?;
        let [fl1, fl2, fl3]: [PyVal; 3] = match fields.len() {
            3 => fields.try_into().expect("three fields"),
            n if n < 3 => return Err(exc::ValueError(format!("not enough values to unpack (expected 3, got {n})"))),
            _ => return Err(exc::ValueError("too many values to unpack (expected 3)")),
        };
        if [&fl1, &fl2, &fl3].iter().any(|f| matches!(f, PyVal::Obj(PDFObject::Null))) {
            return Err(exc::AssertionError(""));
        }
        self.data = Some(lend(parser, |_| stream.get_data())?);
        let entlen = fl1.add(&fl2)?.add(&fl3)?;
        (self.fl1, self.fl2, self.fl3, self.entlen) = (Some(fl1), Some(fl2), Some(fl3), Some(entlen));
        self.trailer = Some(stream.attrs.clone());
        // (log.debug("xref stream: objid=%s, fields=%d,%d,%d", ...))
        Ok(())
    }

    // pdfminer/pdfdocument.py:294 PDFXRefStream.get_trailer
    pub fn get_trailer(&self) -> PDFDict {
        self.trailer.clone().expect("a loaded xref stream has a trailer")
    }

    /// `(f1, f2, f3)` of the entry at `index` (`get_pos`'s slicing, and
    /// `get_objids`'s for `f1`); `f2`/`f3` only when `all_fields`.
    fn fields(&self, index: &PyVal, all_fields: bool) -> PyResult<(PyInt, PyInt, PyInt)> {
        let entlen = self.entlen.as_ref().expect("loaded");
        let data = self.data.as_ref().expect("loaded");
        let (offset, end) = entry_bounds(entlen, index)?;
        let ent = slice(data, Some(&offset), Some(&end));
        // entlen is an int, so are the fields (an int or a bool each)
        let fl = |f: &Option<PyVal>| f.as_ref().and_then(PyVal::as_index).expect("int fields");
        let (fl1, fl2) = (fl(&self.fl1), fl(&self.fl2));
        let f1 = nunpack(slice(ent, None, Some(&fl1)), 1);
        if !all_fields {
            return Ok((f1, PyInt::ZERO, PyInt::ZERO));
        }
        let fl12 = fl1.add(&fl2);
        let f2 = nunpack(slice(ent, Some(&fl1), Some(&fl12)), 0);
        let f3 = nunpack(slice(ent, Some(&fl12), None), 0);
        Ok((f1, f2, f3))
    }

    // pdfminer/pdfdocument.py:297 PDFXRefStream.get_objids
    pub fn get_objids(&self) -> ObjIds<'_> {
        Box::new(XRefStreamIds { xref: StreamRef::Borrowed(self), range: 0, current: None, done: false })
    }

    // pdfminer/pdfdocument.py:308 PDFXRefStream.get_pos
    pub fn get_pos(&self, objid: &ObjId) -> PyResult<XRefPos> {
        let id = PyVal::Obj(objid.obj().clone());
        let mut index = PyVal::from(PyInt::ZERO);
        let mut found = false;
        for (start, nobjs) in &self.ranges {
            if start.compare(&id, "<=")? && id.compare(&start.add(nobjs)?, "<")? {
                index = index.add(&id.sub(start)?)?;
                found = true;
                break;
            }
            index = index.add(nobjs)?;
        }
        if !found {
            return Err(exc::PDFKeyError(objid.repr()?));
        }
        let (f1, f2, f3) = self.fields(&index, true)?;
        if f1 == PyInt::Small(1) {
            Ok(XRefPos { strmid: None, index: f2, genno: f3 })
        } else if f1 == PyInt::Small(2) {
            Ok(XRefPos { strmid: Some(f2), index: f3, genno: PyInt::ZERO })
        } else {
            // this is a free object
            Err(exc::PDFKeyError(objid.repr()?))
        }
    }
}

/// The xref stream a [`XRefStreamIds`] reads.
enum StreamRef<'a> {
    Borrowed(&'a PDFXRefStream),
    /// An xref of the `Stream` variant, kept alive by the generator.
    Owned(Rc<PDFBaseXRef>),
}

impl StreamRef<'_> {
    fn get(&self) -> &PDFXRefStream {
        match self {
            StreamRef::Borrowed(x) => x,
            StreamRef::Owned(x) => match &**x {
                PDFBaseXRef::Stream(x) => x,
                _ => unreachable!("an xref stream"),
            },
        }
    }
}

/// The generator `PDFXRefStream.get_objids()`.
struct XRefStreamIds<'a> {
    xref: StreamRef<'a>,
    /// The range being walked.
    range: usize,
    /// `(i, nobjs)` in that range.
    current: Option<(PyInt, PyInt)>,
    done: bool,
}

impl XRefStreamIds<'_> {
    fn step(&mut self) -> PyResult<Option<ObjId>> {
        loop {
            let xref = self.xref.get();
            let Some((start, nobjs)) = xref.ranges.get(self.range) else {
                return Ok(None);
            };
            let (i, n) = match &mut self.current {
                Some(state) => state,
                None => self.current.insert((PyInt::ZERO, nobjs.range_arg()?)),
            };
            if *i >= *n {
                self.range += 1;
                self.current = None;
                continue;
            }
            let this = i.clone();
            *i = i.add(&PyInt::Small(1));
            let (f1, _, _) = xref.fields(&PyVal::from(this.clone()), false)?;
            if f1 == PyInt::Small(1) || f1 == PyInt::Small(2) {
                return Ok(Some(start.add(&PyVal::from(this))?.to_objid()?));
            }
        }
    }
}

impl Iterator for XRefStreamIds<'_> {
    type Item = PyResult<ObjId>;

    fn next(&mut self) -> Option<PyResult<ObjId>> {
        if self.done {
            return None;
        }
        match self.step() {
            Ok(Some(id)) => Some(Ok(id)),
            Ok(None) => {
                self.done = true;
                None
            }
            Err(e) => {
                self.done = true;
                Some(Err(e))
            }
        }
    }
}
