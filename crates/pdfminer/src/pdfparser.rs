//! Port of `pdfminer/pdfparser.py`: `PDFParser` (the object parser of a
//! PDF file: indirect references, `endobj`, streams and their `/Length`)
//! and `PDFStreamParser` (the objects of an object stream).
//!
//! Both are `PSStackParser`s with their own state (`psparser.rs`, "Parsers
//! and their Python subclasses"): [`PDFParserExt`] and
//! [`PDFStreamParserExt`]. `PDFSyntaxError` is
//! `pycompat::ExcKind::PDFSyntaxError`.
//!
//! # The file object
//!
//! `PDFParser` seeks and reads its file directly (`self.fp.seek(pos)`,
//! `self.fp.read(objlen)`) with values computed from the file (`/Length`,
//! `/Prev`, xref offsets), so the errors of the file object are part of
//! the behavior. pdfplumber opens a path with `open(path, "rb")`, an
//! `io.BufferedReader`, and a file object given by the caller is often an
//! `io.BytesIO`: they differ for a negative or huge position or length
//! ([`FileKind`]). Both read the same bytes from [`BytesIO`].
//!
//! A `BufferedReader.read(n)` beyond its buffer allocates `n` bytes before
//! reading (`_bufferedreader_read_generic`), so a huge `/Length` raises
//! `MemoryError` when that allocation fails: the port makes the same
//! allocation ([`FileKind::read_size`], `pycompat::memory::probe`). Not modeled: a `seek`
//! beyond the largest file size of the file system (2**44 on ext4) raises
//! `OSError` in Python only.
//!
//! # Re-entrance
//!
//! `do_keyword(stream)` resolves the stream dictionary and its `/Length`
//! through the document (`dict_value`, `int_value`), and resolving a
//! reference parses another object with the document's parser, which is
//! this very parser (`docs/architecture.md`, "Résolution des références et
//! ré-entrance du parseur"). The document keeps its parser in a slot that a
//! `getobj` takes it from; [`lend`] puts the running parser back in that
//! slot for the duration of a resolution, so that the nested `seek`/
//! `reset`/`nextobject` act on the same state, as in Python, and takes it
//! back after.

use std::rc::{Rc, Weak};
use std::sync::LazyLock;

use pycompat::io::{BytesIO, Pos};
use pycompat::logging::Logger;
use pycompat::recursion::{c_called_leaf, eval_entry, leaf, py_frame};
use pycompat::{exc, ExcKind, PyInt, PyResult};

use crate::casting::safe_int;
use crate::pdfdocument::PDFDocument;
use crate::pdftypes::{dict_value, int_value_obj, PDFObjRef, PDFObject, PDFResolver, PDFStream};
use crate::psparser::{PSKeyword, PSStackEntry, PSStackParser, PSStackParserExt, KWD};

/// `log = logging.getLogger(__name__)` of `pdfminer/pdfparser.py`.
static LOG: Logger = Logger::new("pdfminer.pdfparser");

// pdfminer/pdfparser.py:48-53 PDFParser.KEYWORD_R, PDFParser.KEYWORD_NULL, PDFParser.KEYWORD_ENDOBJ,
// PDFParser.KEYWORD_STREAM, PDFParser.KEYWORD_XREF, PDFParser.KEYWORD_STARTXREF
pub static KEYWORD_R: LazyLock<PSKeyword> = LazyLock::new(|| KWD(b"R"));
pub static KEYWORD_NULL: LazyLock<PSKeyword> = LazyLock::new(|| KWD(b"null"));
pub static KEYWORD_ENDOBJ: LazyLock<PSKeyword> = LazyLock::new(|| KWD(b"endobj"));
pub static KEYWORD_STREAM: LazyLock<PSKeyword> = LazyLock::new(|| KWD(b"stream"));
pub static KEYWORD_XREF: LazyLock<PSKeyword> = LazyLock::new(|| KWD(b"xref"));
pub static KEYWORD_STARTXREF: LazyLock<PSKeyword> = LazyLock::new(|| KWD(b"startxref"));
// pdfminer/pdfparser.py:153 PDFStreamParser.KEYWORD_OBJ
pub static KEYWORD_OBJ: LazyLock<PSKeyword> = LazyLock::new(|| KWD(b"obj"));

/// The Python class of the file object a parser reads (see the module
/// documentation).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    /// `open(path, "rb")`: `io.BufferedReader`
    BufferedReader,
    /// `io.BytesIO`
    BytesIO,
}

impl FileKind {
    /// The position `fp.seek(pos)` moves to, or its error.
    pub fn seek_pos(self, pos: &PyInt) -> PyResult<Pos> {
        match (pos.as_i64(), self) {
            (Some(p), _) if p >= 0 => Ok(p),
            (Some(p), FileKind::BytesIO) => Err(exc::ValueError(format!("negative seek value {p}"))),
            (Some(_), FileKind::BufferedReader) => Err(exc::OSError("[Errno 22] Invalid argument")),
            (None, FileKind::BytesIO) => Err(exc::OverflowError("Python int too large to convert to C ssize_t")),
            (None, FileKind::BufferedReader) => Err(exc::ValueError("cannot fit 'int' into an offset-sized integer")),
        }
    }

    /// The size `fp.read(n)` asks for (`None`: to the end), or its error.
    pub fn read_size(self, n: &PyInt) -> PyResult<Option<usize>> {
        // PY_SSIZE_T_MAX - PyBytesObject_SIZE: the largest bytes object
        const MAX_BYTES: i64 = i64::MAX - 33;
        match n.as_i64() {
            None => Err(exc::OverflowError("cannot fit 'int' into an index-sized integer")),
            Some(-1) => Ok(None),
            Some(n) if n < 0 => match self {
                FileKind::BytesIO => Ok(None),
                FileKind::BufferedReader => Err(exc::ValueError("read length must be non-negative or -1")),
            },
            Some(n) if n > MAX_BYTES && self == FileKind::BufferedReader => {
                Err(exc::OverflowError("byte string is too large"))
            }
            Some(n) => {
                if self == FileKind::BufferedReader && n > BUFFER_SIZE {
                    // PyBytes_FromStringAndSize(NULL, n): MemoryError when
                    // the allocation fails
                    pycompat::memory::probe(n as u128)?;
                }
                Ok(Some(usize::try_from(n).unwrap_or(usize::MAX)))
            }
        }
    }
}

/// `io.DEFAULT_BUFFER_SIZE`, the buffer of `open(path, "rb")`: a read of
/// more bytes than it holds allocates the whole result first.
const BUFFER_SIZE: i64 = 8192;

/// The keywords `do_keyword` tests, copied from the statics once.
#[derive(Clone, Copy)]
struct Keywords {
    r: PSKeyword,
    null: PSKeyword,
    endobj: PSKeyword,
    stream: PSKeyword,
    xref: PSKeyword,
    startxref: PSKeyword,
    obj: PSKeyword,
}

impl Keywords {
    fn new() -> Keywords {
        Keywords {
            r: *KEYWORD_R,
            null: *KEYWORD_NULL,
            endobj: *KEYWORD_ENDOBJ,
            stream: *KEYWORD_STREAM,
            xref: *KEYWORD_XREF,
            startxref: *KEYWORD_STARTXREF,
            obj: *KEYWORD_OBJ,
        }
    }
}

/// The state `PDFParser` adds to `PSStackParser`.
pub struct PDFParserExt {
    /// `self.doc`
    pub doc: Option<Weak<PDFDocument>>,
    /// `self.fallback`
    pub fallback: bool,
    /// The class of `self.fp`.
    pub file: FileKind,
    keywords: Keywords,
}

/// pdfminer/pdfparser.py:23 `PDFParser`.
pub type PDFParser = PSStackParser<PDFParserExt>;

/// The state `PDFStreamParser` adds to `PSStackParser` (its `fallback`,
/// inherited from `PDFParser`, is never read).
pub struct PDFStreamParserExt {
    /// `self.doc`
    pub doc: Option<Weak<PDFDocument>>,
    keywords: Keywords,
}

/// pdfminer/pdfparser.py:139 `PDFStreamParser`.
pub type PDFStreamParser = PSStackParser<PDFStreamParserExt>;

/// `self.doc` as the `doc` of a new `PDFObjRef`.
fn resolver(doc: &Option<Weak<PDFDocument>>) -> Option<Weak<dyn PDFResolver>> {
    doc.as_ref().map(|d| d.clone() as Weak<dyn PDFResolver>)
}

impl PSStackParser<PDFParserExt> {
    // pdfminer/pdfparser.py:39 PDFParser.__init__
    /// `PDFParser(fp)`, `fp` being a file object of class `file`.
    pub fn new(fp: BytesIO, file: FileKind) -> PyResult<PDFParser> {
        // the class is called (from C), then its __init__
        let _entry = eval_entry()?;
        let _frame = py_frame()?;
        let ext = PDFParserExt { doc: None, fallback: false, file, keywords: Keywords::new() };
        PSStackParser::init(Some(fp), ext)
    }

    // pdfminer/pdfparser.py:44 PDFParser.set_document
    /// Associates the parser with a PDFDocument object.
    pub fn set_document(&mut self, doc: Weak<PDFDocument>) -> PyResult<()> {
        leaf(1)?;
        self.ext.doc = Some(doc);
        Ok(())
    }

    /// `self.seek(pos)` for a Python int `pos` (not a Python method): the
    /// error of the file object for a position it refuses, raised where
    /// `self.fp.seek(pos)` raises it.
    pub fn seek_int(&mut self, pos: &PyInt) -> PyResult<()> {
        match self.ext.file.seek_pos(pos) {
            Ok(pos) => self.seek(pos),
            Err(e) => self.seek_error(e),
        }
    }

    /// The stand-in left in place of this parser while it is lent.
    fn stand_in(&self) -> PDFParser {
        let ext = PDFParserExt {
            doc: self.ext.doc.clone(),
            fallback: self.ext.fallback,
            file: self.ext.file,
            keywords: self.ext.keywords,
        };
        PSStackParser::placeholder(ext)
    }

    /// The body of `do_keyword` for `stream`; `pos` is the keyword's.
    fn do_stream(&mut self, mut pos: Pos) -> PyResult<()> {
        // stream object
        let popped_data = self.pop(1)?;
        let Some((_, dic)) = popped_data.into_iter().next() else {
            // pop(1) returned []: the unpacking failed
            return Err(exc::PDFSyntaxError("Invalid stream dictionary: []"));
        };
        let dic = lend(self, |_| dict_value(&dic))?;
        let mut objlen = PyInt::ZERO;
        if !self.ext.fallback {
            // try: objlen = int_value(dic["Length"]) except KeyError: (STRICT)
            if let Some(length) = dic.get("Length") {
                objlen = match lend(self, |_| int_value_obj(&length))? {
                    PDFObject::Int(i) => i,
                    PDFObject::Bool(b) => PyInt::Small(i64::from(b)),
                    _ => unreachable!("int_value returns an int or a bool"),
                };
            }
        }
        self.seek(pos)?;
        let line = match self.nextline() {
            Ok((_, line)) => line,
            // 'stream': with settings.STRICT, PDFSyntaxError("Unexpected EOF")
            Err(e) if e.is_instance(ExcKind::PSEOF) => return Ok(()),
            Err(e) => return Err(e),
        };
        pos += line.len() as Pos;
        let file = self.ext.file;
        let fp = self.fp.as_mut().expect("a PDFParser has a file");
        fp.seek(pos)?;
        let range = fp.read(file.read_size(&objlen)?);
        let mut data = fp.data()[range].to_vec();
        self.seek_int(&PyInt::Small(pos).add(&objlen))?;
        loop {
            let line = match self.nextline() {
                Ok((_, line)) => line,
                // with settings.STRICT, PDFSyntaxError("Unexpected EOF")
                Err(e) if e.is_instance(ExcKind::PSEOF) => break,
                Err(e) => return Err(e),
            };
            if let Some(i) = find(&line, b"endstream") {
                objlen = objlen.add(&PyInt::from(i));
                if self.ext.fallback {
                    data.extend_from_slice(&line[..i]);
                }
                break;
            }
            objlen = objlen.add(&PyInt::from(line.len()));
            if self.ext.fallback {
                data.extend_from_slice(&line);
            }
        }
        self.seek_int(&PyInt::Small(pos).add(&objlen))?;
        // XXX limit objlen not to exceed object boundary
        // log.debug("Stream: pos=%d, objlen=%d, dic=%r, data=%r...", ...)
        LOG.debug()?;
        let Some(doc) = self.ext.doc.as_ref().and_then(Weak::upgrade) else {
            return Err(exc::AssertionError(""));
        };
        // PDFStream(dic, bytes(data), self.doc.decipher): its __init__,
        // called from C
        c_called_leaf()?;
        let stream = PDFStream::new(dic, Rc::from(data), doc.decipher());
        self.push((pos, PDFObject::Stream(Rc::new(stream))))?;
        Ok(())
    }
}

/// `needle in haystack` and `haystack.index(needle)` for bytes.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Runs `f`, which may resolve references through the document of `p`,
/// with `p` lent to that document (see the module documentation): `p` is
/// the document's parser (`PDFDocument::new` sets the document of the
/// parser it takes, and only that parser, or a stand-in for it, is ever
/// lent), so it goes back to the document's slot for the duration of `f`,
/// where a nested `getobj` finds it, and `p` is whatever that nesting left
/// in it afterwards. `f` gets the document.
pub fn lend<T>(p: &mut PDFParser, f: impl FnOnce(Option<&Rc<PDFDocument>>) -> PyResult<T>) -> PyResult<T> {
    let Some(doc) = p.ext.doc.as_ref().and_then(Weak::upgrade) else {
        return f(None);
    };
    let mut lent = doc.take_spare_parser().unwrap_or_else(|| Box::new(p.stand_in()));
    std::mem::swap(p, &mut lent);
    doc.put_parser(lent);
    let result = f(Some(&doc));
    let mut back = doc.take_parser().expect("a lent parser comes back to its slot");
    std::mem::swap(p, &mut back);
    doc.put_spare_parser(back);
    result
}

impl PSStackParserExt for PDFParserExt {
    // pdfminer/pdfparser.py:55 PDFParser.do_keyword
    /// Handles PDF-related keywords.
    fn do_keyword(p: &mut PDFParser, pos: Pos, token: PSKeyword) -> PyResult<()> {
        let _frame = py_frame()?;
        let k = p.ext.keywords;
        if token == k.xref || token == k.startxref {
            let objs = p.pop(1)?;
            p.add_results(objs)?;
        } else if token == k.endobj {
            let objs = p.pop(4)?;
            p.add_results(objs)?;
        } else if token == k.null {
            // null object
            p.push((pos, PDFObject::Null))?;
        } else if token == k.r {
            // reference to indirect object
            if p.curstack.len() >= 2 {
                let popped = p.pop(2)?;
                let object_id = safe_int_counted(&popped[0].1)?;
                if let Some(object_id) = object_id {
                    // PDFObjRef(self.doc, object_id): its __init__, called from C
                    c_called_leaf()?;
                    let obj = PDFObjRef::new(resolver(&p.ext.doc), object_id);
                    p.push((pos, PDFObject::Ref(obj)))?;
                }
            }
        } else if token == k.stream {
            p.do_stream(pos)?;
        } else {
            // others
            p.push((pos, PDFObject::Keyword(token)))?;
        }
        Ok(())
    }
}

impl PSStackParser<PDFStreamParserExt> {
    // pdfminer/pdfparser.py:147 PDFStreamParser.__init__
    /// `PDFStreamParser(data)`: the class is called (from C), its
    /// `__init__` calls `PDFParser.__init__`.
    pub fn new(data: Rc<[u8]>) -> PyResult<PDFStreamParser> {
        let _entry = eval_entry()?;
        let _frame = py_frame()?;
        let _parser_init = py_frame()?;
        PSStackParser::init(Some(BytesIO::new(data)), PDFStreamParserExt { doc: None, keywords: Keywords::new() })
    }

    // pdfminer/pdfparser.py:44 PDFParser.set_document
    pub fn set_document(&mut self, doc: Weak<PDFDocument>) -> PyResult<()> {
        leaf(1)?;
        self.ext.doc = Some(doc);
        Ok(())
    }
}

/// `(_, a), _ = entries`: the first of exactly two entries.
fn unpack_two(entries: Vec<PSStackEntry>) -> PyResult<PDFObject> {
    match entries.len() {
        2 => Ok(entries.into_iter().next().expect("two entries").1),
        n => Err(exc::ValueError(format!("not enough values to unpack (expected 2, got {n})"))),
    }
}

impl PSStackParserExt for PDFStreamParserExt {
    // pdfminer/pdfparser.py:150 PDFStreamParser.flush
    fn flush(p: &mut PDFStreamParser) -> PyResult<()> {
        let _frame = py_frame()?;
        p.add_results_popall()
    }

    // pdfminer/pdfparser.py:155 PDFStreamParser.do_keyword
    fn do_keyword(p: &mut PDFStreamParser, pos: Pos, token: PSKeyword) -> PyResult<()> {
        let _frame = py_frame()?;
        let k = p.ext.keywords;
        if token == k.r {
            // reference to indirect object
            let popped = p.pop(2)?;
            let object_id = safe_int_counted(&unpack_two(popped)?)?;
            if let Some(object_id) = object_id {
                // PDFObjRef(self.doc, object_id): its __init__, called from C
                c_called_leaf()?;
                let obj = PDFObjRef::new(resolver(&p.ext.doc), object_id);
                p.push((pos, PDFObject::Ref(obj)))?;
            }
            return Ok(());
        } else if token == k.obj || token == k.endobj {
            // with settings.STRICT: PDFSyntaxError("Keyword endobj found in stream")
            return Ok(());
        }
        // others
        p.push((pos, PDFObject::Keyword(token)))?;
        Ok(())
    }
}

/// `safe_int(o)` (`casting`, whose port does not count its frame): its
/// frame, then the value.
fn safe_int_counted(o: &PDFObject) -> PyResult<Option<PyInt>> {
    leaf(1)?;
    safe_int(o)
}
