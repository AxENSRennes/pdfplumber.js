//! Port of `pdfminer/pdfdocument.py`, a folder (`docs/architecture.md`
//! allows it for a large module):
//!
//! - `mod.rs`: `PDFDocument` (xref discovery and fallback, password,
//!   `getobj` with object streams, outlines, page labels, names and
//!   destinations) and `PageLabels`;
//! - `xref.rs`: `PDFBaseXRef`, `PDFXRef`, `PDFXRefFallback`, `PDFXRefStream`;
//! - `security.rs`: the standard security handlers and their registry;
//! - `pyval.rs`: the Python operators upstream applies to file values
//!   (`/Index`, `/W`, `/N`, name tree keys).
//!
//! The exception classes are `pycompat::ExcKind::{PDFNoValidXRef,
//! PDFNoOutlines, PDFNoPageLabels, PDFDestinationNotFound,
//! PDFEncryptionError, PDFPasswordIncorrect, PDFTextExtractionNotAllowed}`
//! (the warning classes are never raised upstream).
//!
//! # The document and its parser
//!
//! `PDFDocument.__init__` resolves references (`/Encrypt`, `/Root` of the
//! trailers) while it runs, and the parser resolves `/Length` through the
//! document: [`PDFDocument::new`] creates the document in an `Rc` whose
//! state is interior first, then runs `__init__` (`docs/architecture.md`,
//! "Résolution des références et ré-entrance du parseur"). The document
//! owns its parser in a slot: an operation that reads the file takes it
//! ([`PDFDocument::with_parser`]), and a resolution started while the
//! parser runs lends it back ([`crate::pdfparser::lend`]), so a nested
//! `getobj` acts on the same parser state as in Python.
//!
//! Object ids are [`ObjId`]s (Python's `hash`/`==` on any hashable value),
//! as keys of the caches and of the xref tables.
//!
//! # Recursion depth
//!
//! The recursive methods (`read_xref_from` over `/Prev` and `/XRefStm`,
//! `getobj` over object streams, the outline search, the name tree lookup)
//! and every Python function they call take their frames
//! (`pycompat::recursion`): the parser's methods (`psparser.rs`), the
//! accessors of `pdftypes`, the logging calls (`pycompat::logging`), the
//! class instantiations (an `__init__` called from C) and the generators,
//! so a loop in the file ends in `RecursionError("maximum recursion depth
//! exceeded")` and a long finite chain fails at the same level as in Python
//! (`docs/architecture.md`, "Profondeur de récursion").

mod pyval;
mod security;
mod xref;

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::LazyLock;

use pycompat::logging::Logger;
use pycompat::recursion::{c_called_leaf, eval_entry, leaf, py_frame};
use pycompat::repr::{bytes_repr, str_repr};
use pycompat::{exc, ExcKind, PyErr, PyInt, PyResult};
use rustc_hash::FxHashMap;

pub use pyval::PyVal;
pub use security::*;
pub use xref::{pdfobj_cue, ObjIds, PDFBaseXRef, PDFXRef, PDFXRefStream, XRefPos};

use crate::data_structures::NumberTree;
use crate::pdfparser::{FileKind, PDFParser, PDFStreamParser};
use crate::pdftypes::{
    decipher_all, dict_value, int_value, list_value, str_value, stream_value, DecipherCallable, ObjId, PDFDict,
    PDFList, PDFObject, PDFResolver, PDFStream,
};
use crate::psparser::{literal_name, py_intern, PSLiteral, Symbol, LIT};
use crate::utils::{choplist, decode_text, format_int_alpha, format_int_roman};

/// `log = logging.getLogger(__name__)` of `pdfminer/pdfdocument.py`.
pub(crate) static LOG: Logger = Logger::new("pdfminer.pdfdocument");

/// `decode_text(s)` (`utils`, whose port does not count its frames): its
/// frame and, for PDFDocEncoding, the generator expression that
/// `"".join(...)` runs (from C).
pub(crate) fn decode_text_counted(s: &[u8]) -> PyResult<String> {
    let _frame = py_frame()?;
    if !s.starts_with(b"\xfe\xff") {
        let _entry = eval_entry()?;
        leaf(1)?;
    }
    Ok(decode_text(s))
}

// pdfminer/pdfdocument.py:102-104 LITERAL_OBJSTM, LITERAL_XREF, LITERAL_CATALOG
pub static LITERAL_OBJSTM: LazyLock<PSLiteral> = LazyLock::new(|| LIT("ObjStm"));
pub static LITERAL_XREF: LazyLock<PSLiteral> = LazyLock::new(|| LIT("XRef"));
pub static LITERAL_CATALOG: LazyLock<PSLiteral> = LazyLock::new(|| LIT("Catalog"));

/// The first element of `PDFDocument.encryption`: the trailer's `/ID` as
/// `list_value` returns it (the list object itself, or a new empty list),
/// or `(b"", b"")` when the trailer has no `/ID`.
#[derive(Clone)]
pub enum DocId {
    List(PDFList),
    /// `(b"", b"")`, a tuple
    Default,
}

impl DocId {
    /// The ids, as the security handler reads them.
    pub fn to_vec(&self) -> Vec<PDFObject> {
        match self {
            DocId::List(l) => l.to_vec(),
            DocId::Default => vec![PDFObject::Bytes(Rc::from(&b""[..])), PDFObject::Bytes(Rc::from(&b""[..]))],
        }
    }
}

/// What `_parsed_objs` keeps for an object stream: its objects and its
/// `/N` (any object).
type ParsedObjs = (Rc<Vec<PDFObject>>, PDFObject);

/// pdfminer/pdfdocument.py:673 `PDFDocument`.
pub struct PDFDocument {
    /// `self.caching`
    pub caching: bool,
    xrefs: RefCell<Vec<Rc<PDFBaseXRef>>>,
    info: RefCell<Vec<PDFDict>>,
    catalog: RefCell<PDFDict>,
    encryption: RefCell<Option<(DocId, PDFDict)>>,
    decipher: RefCell<Option<DecipherCallable>>,
    /// `self._parser`, while no operation uses it.
    parser: RefCell<Option<Box<PDFParser>>>,
    /// A stand-in kept for [`crate::pdfparser::lend`].
    spare_parser: RefCell<Option<Box<PDFParser>>>,
    cached_objs: RefCell<FxHashMap<ObjId, (PDFObject, PyInt)>>,
    parsed_objs: RefCell<FxHashMap<ObjId, ParsedObjs>>,
    is_printable: Cell<bool>,
    is_modifiable: Cell<bool>,
    is_extractable: Cell<bool>,
    /// `self`, for the parsers and references the document creates.
    this: Weak<PDFDocument>,
}

impl PDFDocument {
    // pdfminer/pdfdocument.py:693 PDFDocument.__init__
    /// `PDFDocument(parser, password, caching, fallback)`: reads the cross
    /// reference tables (falling back to a scan of the file when
    /// `fallback` and none is valid), the trailers, the encryption and the
    /// catalog.
    pub fn new(parser: PDFParser, password: &str, caching: bool, fallback: bool) -> PyResult<Rc<PDFDocument>> {
        PDFDocument::new_with(parser, &|| Ok(password.to_owned()), caching, fallback)
    }

    /// [`PDFDocument::new`] with a password read only when the document is
    /// encrypted (`_initialize_password(password)` is the only use of
    /// `password` in `__init__`): pdfplumber passes any object its caller
    /// gave, which an unencrypted document never looks at, and the port
    /// models only a `str` (`password()` raises `NotPorted` for another).
    pub fn new_with(
        parser: PDFParser,
        password: &dyn Fn() -> PyResult<String>,
        caching: bool,
        fallback: bool,
    ) -> PyResult<Rc<PDFDocument>> {
        // the class is called (from C), then its __init__
        let _entry = eval_entry()?;
        let _frame = py_frame()?;
        let doc = Rc::new_cyclic(|this| PDFDocument {
            caching,
            xrefs: RefCell::new(Vec::new()),
            info: RefCell::new(Vec::new()),
            catalog: RefCell::new(PDFDict::new()),
            encryption: RefCell::new(None),
            decipher: RefCell::new(None),
            parser: RefCell::new(None),
            spare_parser: RefCell::new(None),
            cached_objs: RefCell::new(FxHashMap::default()),
            parsed_objs: RefCell::new(FxHashMap::default()),
            is_printable: Cell::new(true),
            is_modifiable: Cell::new(true),
            is_extractable: Cell::new(true),
            this: this.clone(),
        });
        let mut parser = Box::new(parser);
        parser.set_document(Rc::downgrade(&doc))?;
        doc.put_parser(parser);
        doc.init(password, fallback)?;
        Ok(doc)
    }

    /// `PDFDocument(PDFParser(fp), password)` as pdfplumber opens a file
    /// (`caching=True`, `fallback=True`): `data` is the content of `fp`, an
    /// object of class `file`.
    pub fn open(data: Rc<[u8]>, file: FileKind, password: &str) -> PyResult<Rc<PDFDocument>> {
        PDFDocument::open_with(data, file, &|| Ok(password.to_owned()))
    }

    /// [`PDFDocument::open`] with a password read only when the document is
    /// encrypted (see [`PDFDocument::new_with`]).
    pub fn open_with(
        data: Rc<[u8]>,
        file: FileKind,
        password: &dyn Fn() -> PyResult<String>,
    ) -> PyResult<Rc<PDFDocument>> {
        let parser = PDFParser::new(pycompat::io::BytesIO::new(data), file)?;
        PDFDocument::new_with(parser, password, true, true)
    }

    /// The rest of `__init__`, once the document exists (in the frame of
    /// `__init__`, taken by [`PDFDocument::new`]).
    fn init(&self, password: &dyn Fn() -> PyResult<String>, fallback: bool) -> PyResult<()> {
        // Retrieve the information of each header that was appended
        // (maybe multiple times) at the end of the document.
        let found = self.with_parser(|p| {
            let pos = self.find_xref(p)?;
            self.read_xref_from(p, &pos)
        });
        match found {
            Ok(()) => {}
            Err(e) if e.is_instance(ExcKind::PDFNoValidXRef) => {
                if fallback {
                    self.with_parser(|p| {
                        p.ext.fallback = true;
                        Ok(())
                    })?;
                    // PDFXRefFallback(): PDFXRef.__init__, called from C
                    c_called_leaf()?;
                    let mut newxref = PDFXRef::new();
                    self.with_parser(|p| newxref.load_fallback(p))?;
                    self.xrefs.borrow_mut().push(Rc::new(PDFBaseXRef::Fallback(newxref)));
                }
            }
            Err(e) => return Err(e),
        }
        let mut i = 0;
        loop {
            let Some(xref) = self.xrefs.borrow().get(i).cloned() else {
                return Err(exc::PDFSyntaxError("No /Root object! - Is this really a PDF?"));
            };
            i += 1;
            leaf(1)?; // xref.get_trailer()
            let trailer = xref.get_trailer();
            if trailer.is_empty() {
                continue;
            }
            // If there's an encryption info, remember it.
            if let Some(encrypt) = trailer.get("Encrypt") {
                // Some documents may not have a /ID, use two empty byte
                // strings instead.
                let id_value = match trailer.get("ID") {
                    Some(id) => DocId::List(list_value(&id)?),
                    None => DocId::Default,
                };
                let param = dict_value(&encrypt)?;
                *self.encryption.borrow_mut() = Some((id_value, param));
                self._initialize_password(&password()?)?;
            }
            if let Some(info) = trailer.get("Info") {
                let info = dict_value(&info)?;
                self.info.borrow_mut().push(info);
            }
            if let Some(root) = trailer.get("Root") {
                // Every PDF file must have exactly one /Root dictionary.
                let catalog = dict_value(&root)?;
                *self.catalog.borrow_mut() = catalog;
                break;
            }
        }
        // (self.catalog.get("Type") is not LITERAL_CATALOG and settings.STRICT)
        Ok(())
    }

    // ---- the parser slot

    /// Takes the parser out of its slot.
    pub(crate) fn take_parser(&self) -> Option<Box<PDFParser>> {
        self.parser.borrow_mut().take()
    }

    /// Puts the parser back in its slot.
    pub(crate) fn put_parser(&self, parser: Box<PDFParser>) {
        *self.parser.borrow_mut() = Some(parser);
    }

    pub(crate) fn take_spare_parser(&self) -> Option<Box<PDFParser>> {
        self.spare_parser.borrow_mut().take()
    }

    pub(crate) fn put_spare_parser(&self, parser: Box<PDFParser>) {
        *self.spare_parser.borrow_mut() = Some(parser);
    }

    /// Runs `f` on `self._parser`, taken from its slot for the duration.
    pub fn with_parser<T>(&self, f: impl FnOnce(&mut PDFParser) -> PyResult<T>) -> PyResult<T> {
        let Some(mut parser) = self.take_parser() else {
            // Every use of the parser that may resolve a reference lends it
            // back (pdfparser::lend): an empty slot is a bug of the port.
            return Err(PyErr::not_ported("a nested use of the document's parser that was not lent"));
        };
        let result = f(&mut parser);
        self.put_parser(parser);
        result
    }

    /// `fp.close()` of the file the document reads (pdfplumber's
    /// `PDF.close` closes the file it opened): every later read of the
    /// document raises the `ValueError` of a closed file
    /// (`pycompat::io::Closed`).
    pub fn close_file(&self) -> PyResult<()> {
        self.with_parser(|p| {
            let class = match p.ext.file {
                FileKind::BufferedReader => pycompat::io::Closed::BufferedReader,
                FileKind::BytesIO => pycompat::io::Closed::BytesIO,
            };
            if let Some(fp) = &p.fp {
                fp.close(class);
            }
            Ok(())
        })
    }

    // ---- attributes

    /// The class of the file object the document reads (`self._parser.fp`):
    /// what `copy.deepcopy` of an object holding the document does
    /// (pdfplumber/structure.py:132 `PDFStructElement.to_dict`).
    pub fn file_kind(&self) -> PyResult<FileKind> {
        self.with_parser(|p| Ok(p.ext.file))
    }

    /// `self.xrefs`
    pub fn xrefs(&self) -> Vec<Rc<PDFBaseXRef>> {
        self.xrefs.borrow().clone()
    }

    /// `self.catalog`
    pub fn catalog(&self) -> PDFDict {
        self.catalog.borrow().clone()
    }

    /// `self.info`
    pub fn info(&self) -> Vec<PDFDict> {
        self.info.borrow().clone()
    }

    /// `self.encryption`
    pub fn encryption(&self) -> Option<(DocId, PDFDict)> {
        self.encryption.borrow().clone()
    }

    /// `self.decipher`
    pub fn decipher(&self) -> Option<DecipherCallable> {
        self.decipher.borrow().clone()
    }

    /// `self.is_printable`
    pub fn is_printable(&self) -> bool {
        self.is_printable.get()
    }

    /// `self.is_modifiable`
    pub fn is_modifiable(&self) -> bool {
        self.is_modifiable.get()
    }

    /// `self.is_extractable`
    pub fn is_extractable(&self) -> bool {
        self.is_extractable.get()
    }

    /// A weak reference to the document (what `PDFObjRef.doc` holds).
    pub fn weak(&self) -> Weak<PDFDocument> {
        self.this.clone()
    }

    // pdfminer/pdfdocument.py:752 PDFDocument._initialize_password
    /// Perform the initialization with a given password.
    fn _initialize_password(&self, password: &str) -> PyResult<()> {
        let _frame = py_frame()?;
        let (docid, param) = self.encryption.borrow().clone().expect("set by the caller");
        let filter = param.get("Filter").unwrap_or(PDFObject::Null);
        if literal_name(&filter)? != "Standard" {
            let msg = format!("Unknown filter: param={}", PDFObject::Dict(param.clone()).repr()?);
            return Err(PyErr::new(ExcKind::PDFEncryptionError, msg));
        }
        let v = int_value(&param.get("V").unwrap_or(PDFObject::Int(PyInt::ZERO)))?;
        let Some(factory) = security_handler_registry(&v) else {
            let msg = format!("Unknown algorithm: param={}", PDFObject::Dict(param.clone()).repr()?);
            return Err(PyErr::new(ExcKind::PDFEncryptionError, msg));
        };
        let handler = Rc::new(PDFStandardSecurityHandler::new(factory, docid.to_vec(), param, password)?);
        *self.decipher.borrow_mut() = Some(handler.decipher());
        self.is_printable.set(handler.is_printable()?);
        self.is_modifiable.set(handler.is_modifiable()?);
        self.is_extractable.set(handler.is_extractable()?);
        // need to read streams with exact length
        self.with_parser(|p| {
            p.ext.fallback = false;
            Ok(())
        })
    }

    // pdfminer/pdfdocument.py:769 PDFDocument._getobj_objstm
    fn _getobj_objstm(&self, stream: &Rc<PDFStream>, index: &PyInt, _objid: &ObjId) -> PyResult<PDFObject> {
        let _frame = py_frame()?;
        let strmid = stream.objid();
        let cached = strmid.as_ref().and_then(|id| self.parsed_objs.borrow().get(id).cloned());
        let (objs, n) = match cached {
            Some(parsed) => parsed,
            None => {
                let (objs, n) = self._get_objects(stream)?;
                let parsed = (Rc::new(objs), n);
                if self.caching {
                    let Some(strmid) = strmid else {
                        return Err(exc::AssertionError(""));
                    };
                    self.parsed_objs.borrow_mut().insert(strmid, parsed.clone());
                }
                parsed
            }
        };
        // i = n * 2 + index
        let i = n_times_2_plus(&n, index)?;
        match list_index(&objs, &i)? {
            Some(obj) => Ok(obj),
            None => Err(exc::PDFSyntaxError(format!("index too big: {index}"))),
        }
    }

    // pdfminer/pdfdocument.py:784 PDFDocument._get_objects
    fn _get_objects(&self, stream: &Rc<PDFStream>) -> PyResult<(Vec<PDFObject>, PDFObject)> {
        let _frame = py_frame()?;
        // (stream.get("Type") is not LITERAL_OBJSTM and settings.STRICT)
        stream.get_counted("Type", &PDFObject::Null)?;
        let n = match stream.getitem("N") {
            // cast(int, stream["N"])
            Ok(n) => {
                leaf(1)?;
                n
            }
            // with settings.STRICT: PDFSyntaxError(f"N is not defined: {stream!r}")
            Err(e) if e.is_instance(ExcKind::KeyError) => PDFObject::Int(PyInt::ZERO),
            Err(e) => return Err(e),
        };
        let mut parser = PDFStreamParser::new(stream.get_data()?)?;
        parser.set_document(self.weak())?;
        let mut objs = Vec::new();
        loop {
            match parser.nextobject() {
                Ok((_, obj)) => objs.push(obj),
                Err(e) if e.is_instance(ExcKind::PSEOF) => break,
                Err(e) => return Err(e),
            }
        }
        Ok((objs, n))
    }

    // pdfminer/pdfdocument.py:804 PDFDocument._getobj_parse
    fn _getobj_parse(&self, pos: &PyInt, objid: &ObjId) -> PyResult<PDFObject> {
        let _frame = py_frame()?;
        self.with_parser(|p| {
            p.seek_int(pos)?;
            let (_, mut objid1) = p.nexttoken()?; // objid
            p.nexttoken()?; // genno
            let (_, mut kwd) = p.nexttoken()?;
            // hack around malformed pdf files
            // copied from https://github.com/jaepil/pdfminer3k/blob/master/
            // pdfminer/pdfparser.py#L399
            // to solve https://github.com/pdfminer/pdfminer.six/issues/56
            // assert objid1 == objid, str((objid1, objid))
            let keyword_obj = PDFObject::Keyword(*crate::pdfparser::KEYWORD_OBJ);
            if objid1 != *objid.obj() {
                let mut x = Vec::new();
                while !is_same_keyword(&kwd, &keyword_obj) {
                    let (_, k) = p.nexttoken()?;
                    kwd = k.clone();
                    x.push(k);
                }
                if x.len() >= 2 {
                    objid1 = x[x.len() - 2].clone();
                }
            }
            // #### end hack around malformed pdf files
            if objid1 != *objid.obj() {
                return Err(exc::PDFSyntaxError(format!("objid mismatch: {}={}", objid1.repr()?, objid.repr()?)));
            }
            // kwd != KWD(b"obj")
            py_intern(Symbol::Keyword(*crate::pdfparser::KEYWORD_OBJ))?;
            if !is_same_keyword(&kwd, &keyword_obj) {
                return Err(exc::PDFSyntaxError(format!("Invalid object spec: offset={pos}")));
            }
            let (_, obj) = p.nextobject()?;
            Ok(obj)
        })
    }

    // pdfminer/pdfdocument.py:832 PDFDocument.getobj
    /// Get object from PDF: raises `PDFException` if the document is not
    /// initialized, `PDFObjectNotFound` if `objid` does not exist.
    pub fn getobj(&self, objid: &ObjId) -> PyResult<PDFObject> {
        let _frame = py_frame()?;
        if self.xrefs.borrow().is_empty() {
            return Err(exc::PDFException("PDFDocument is not initialized"));
        }
        // log.debug("getobj: objid=%r", objid)
        LOG.debug()?;
        if let Some((obj, _)) = self.cached_objs.borrow().get(objid) {
            return Ok(obj.clone());
        }
        let mut i = 0;
        let (obj, genno) = loop {
            let Some(xref) = self.xrefs.borrow().get(i).cloned() else {
                return Err(exc::PDFObjectNotFound(objid.py_str()?));
            };
            i += 1;
            let pos = match xref.get_pos(objid) {
                Ok(pos) => pos,
                Err(e) if e.is_instance(ExcKind::KeyError) => continue,
                Err(e) => return Err(e),
            };
            match self.getobj_at(objid, &pos) {
                Ok(obj) => break (obj, pos.genno),
                Err(e) if e.is_instance(ExcKind::PSEOF) || e.is_instance(ExcKind::PDFSyntaxError) => continue,
                Err(e) => return Err(e),
            }
        };
        // log.debug("register: objid=%r: %r", objid, obj)
        LOG.debug()?;
        if self.caching {
            self.cached_objs.borrow_mut().insert(objid.clone(), (obj.clone(), genno));
        }
        Ok(obj)
    }

    /// The `try:` block of `getobj` for one xref entry.
    fn getobj_at(&self, objid: &ObjId, pos: &XRefPos) -> PyResult<PDFObject> {
        let obj = match &pos.strmid {
            Some(strmid) => {
                let stream = stream_value(&self.getobj(&ObjId::int(strmid.clone()))?)?;
                self._getobj_objstm(&stream, &pos.index, objid)?
            }
            None => {
                let obj = self._getobj_parse(&pos.index, objid)?;
                match self.decipher() {
                    Some(decipher) => decipher_all(&decipher, objid, &pos.genno, &obj)?,
                    None => obj,
                }
            }
        };
        if let PDFObject::Stream(s) = &obj {
            leaf(1)?; // obj.set_objid(objid, genno)
            s.set_objid(objid.clone(), pos.genno.clone());
        }
        Ok(obj)
    }

    // pdfminer/pdfdocument.py:874 PDFDocument.get_outlines
    /// The outline entries `(level, title, dest, action, se)`, depth first;
    /// `PDFNoOutlines` when the catalog has no `/Outlines`.
    pub fn get_outlines(&self) -> PyResult<Outlines> {
        let _frame = py_frame()?;
        let Some(outlines) = self.catalog().get("Outlines") else {
            return Err(PyErr::new(ExcKind::PDFNoOutlines, ""));
        };
        // search(self.catalog["Outlines"], 0): the generator is created
        leaf(1)?;
        Ok(Outlines { stack: vec![OutlineFrame::new(outlines, PyInt::ZERO)], done: false })
    }

    // pdfminer/pdfdocument.py:893 PDFDocument.get_page_labels
    /// Generate page label strings for the PDF document: one per page,
    /// without end. `PDFNoPageLabels` when the document has none.
    pub fn get_page_labels(&self) -> PyResult<PageLabelsIter> {
        let _frame = py_frame()?;
        let page_labels = match self.catalog().get("PageLabels") {
            None => Err(exc::KeyError("'PageLabels'")),
            Some(obj) => {
                // PageLabels(...): NumberTree.__init__, called from C
                let _entry = eval_entry()?;
                PageLabels::new(&obj)
            }
        };
        match page_labels {
            Ok(page_labels) => {
                // page_labels.labels: the property (called from C) creates
                // the generator
                let _entry = eval_entry()?;
                let _property = py_frame()?;
                leaf(1)?;
                Ok(page_labels.labels())
            }
            Err(e) if e.is_instance(ExcKind::PDFTypeError) || e.is_instance(ExcKind::KeyError) => {
                Err(PyErr::new(ExcKind::PDFNoPageLabels, ""))
            }
            Err(e) => Err(e),
        }
    }

    // pdfminer/pdfdocument.py:910 PDFDocument.lookup_name
    /// The value of `key` (a `str` or `bytes`) in the name tree `cat` of the
    /// catalog's `/Names`.
    pub fn lookup_name(&self, cat: &str, key: &PyVal) -> PyResult<PDFObject> {
        let _frame = py_frame()?;
        let not_found = |key: &PyVal| -> PyResult<PyErr> { Ok(exc::PDFKeyError(cat_key_repr(cat, key)?)) };
        let names = match self.catalog().get("Names") {
            Some(names) => dict_value(&names),
            None => Err(exc::KeyError("'Names'")),
        };
        let names = match names {
            Ok(names) => names,
            Err(e) if e.is_instance(ExcKind::PDFTypeError) || e.is_instance(ExcKind::KeyError) => {
                return Err(not_found(key)?)
            }
            Err(e) => return Err(e),
        };
        // may raise KeyError
        let Some(d0) = names.get(cat) else {
            return Err(exc::KeyError(str_repr(cat)));
        };
        let d0 = dict_value(&d0)?;
        lookup(&d0, cat, key)
    }

    // pdfminer/pdfdocument.py:938 PDFDocument.get_dest
    pub fn get_dest(&self, name: &PyVal) -> PyResult<PDFObject> {
        let _frame = py_frame()?;
        match self.lookup_name("Dests", name) {
            // PDF-1.2 or later
            Ok(obj) => Ok(obj),
            Err(e) if e.is_instance(ExcKind::KeyError) => {
                // PDF-1.1 or prior
                let not_found =
                    || -> PyResult<PyErr> { Ok(PyErr::new(ExcKind::PDFDestinationNotFound, pyval_str(name)?)) };
                let catalog = self.catalog();
                let Some(dests) = catalog.get("Dests") else {
                    return Err(not_found()?);
                };
                let d0 = dict_value(&dests)?;
                // name in d0: the keys are str, a bytes name is never one
                let PyVal::Str(name) = name else {
                    return Err(not_found()?);
                };
                match d0.get(name) {
                    Some(obj) => Ok(obj),
                    None => Err(not_found()?),
                }
            }
            Err(e) => Err(e),
        }
    }

    // pdfminer/pdfdocument.py:953 PDFDocument.find_xref
    /// Internal function used to locate the first XRef.
    fn find_xref(&self, parser: &mut PDFParser) -> PyResult<PyInt> {
        let _frame = py_frame()?;
        // search the last xref table by scanning the file backwards.
        let mut prev: Vec<u8> = Vec::new();
        for line in parser.revreadlines()? {
            let line = xref::bytes_strip(&line?).to_vec();
            // log.debug("find_xref: %r", line)
            LOG.debug()?;
            if line == b"startxref" {
                // log.debug("xref found: pos=%r", prev)
                LOG.debug()?;
                if prev.is_empty() || !prev.iter().all(u8::is_ascii_digit) {
                    return Err(exc::PDFNoValidXRef(format!("Invalid xref position, no digit: {}", bytes_repr(&prev))));
                }
                let start = PyInt::parse_bytes(&prev, 10)?;
                if start.is_negative() {
                    return Err(exc::PDFNoValidXRef(format!("Invalid xref position, negative: {start}")));
                }
                // The xref start needs to fit in a C ssize_t to be a proper file offset
                if start >= PyInt::Small(1 << 31) {
                    return Err(exc::PDFNoValidXRef(format!("Invalid xref position, too large: {start}")));
                }
                return Ok(start);
            }
            if !line.is_empty() {
                prev = line;
            }
        }
        Err(exc::PDFNoValidXRef("Unexpected EOF"))
    }

    // pdfminer/pdfdocument.py:984 PDFDocument.read_xref_from
    /// Reads XRefs from the given location (into `self.xrefs`).
    fn read_xref_from(&self, parser: &mut PDFParser, start: &PyInt) -> PyResult<()> {
        let _frame = py_frame()?;
        parser.seek_int(start)?;
        parser.reset()?;
        let (pos, token) = match parser.nexttoken() {
            Ok(t) => t,
            Err(e) if e.is_instance(ExcKind::PSEOF) => return Err(exc::PDFNoValidXRef("Unexpected EOF")),
            Err(e) => return Err(e),
        };
        // log.debug("read_xref_from: start=%d, token=%r", start, token)
        LOG.debug()?;
        let xref = if matches!(token, PDFObject::Int(_) | PDFObject::Bool(_)) {
            // XRefStream: PDF-1.5
            parser.seek(pos)?;
            parser.reset()?;
            c_called_leaf()?; // PDFXRefStream()
            let mut xref = PDFXRefStream::new();
            xref.load(parser)?;
            PDFBaseXRef::Stream(xref)
        } else {
            if token == PDFObject::Keyword(*crate::pdfparser::KEYWORD_XREF) {
                parser.nextline()?;
            }
            c_called_leaf()?; // PDFXRef()
            let mut xref = PDFXRef::new();
            xref.load(parser)?;
            PDFBaseXRef::XRef(xref)
        };
        let xref = Rc::new(xref);
        self.xrefs.borrow_mut().push(xref.clone());
        leaf(1)?; // xref.get_trailer()
        let trailer = xref.get_trailer();
        // log.debug("trailer: %r", trailer)
        LOG.debug()?;
        if let Some(xrefstm) = trailer.get("XRefStm") {
            let pos = crate::pdfparser::lend(parser, |_| int_value(&xrefstm))?;
            self.read_xref_from(parser, &pos)?;
        }
        if let Some(prev) = trailer.get("Prev") {
            // find previous xref
            let pos = crate::pdfparser::lend(parser, |_| int_value(&prev))?;
            self.read_xref_from(parser, &pos)?;
        }
        Ok(())
    }
}

/// `kwd is KEYWORD`, `kwd != KWD(b"obj")`: keywords compare by identity.
fn is_same_keyword(a: &PDFObject, b: &PDFObject) -> bool {
    matches!((a, b), (PDFObject::Keyword(x), PDFObject::Keyword(y)) if x == y)
}

/// `repr((cat, key))`, the message of `PDFKeyError((cat, key))`.
fn cat_key_repr(cat: &str, key: &PyVal) -> PyResult<String> {
    Ok(format!("({}, {})", str_repr(cat), pyval_repr(key)?))
}

/// `repr(x)` of a name tree key or value.
fn pyval_repr(x: &PyVal) -> PyResult<String> {
    match x {
        PyVal::Obj(o) => o.repr(),
        PyVal::Str(s) => Ok(str_repr(s)),
    }
}

/// `str(x)`
fn pyval_str(x: &PyVal) -> PyResult<String> {
    match x {
        PyVal::Obj(o) => o.py_str(),
        PyVal::Str(s) => Ok(s.to_string()),
    }
}

/// `n * 2 + index` (`_getobj_objstm`), `n` being any object.
fn n_times_2_plus(n: &PDFObject, index: &PyInt) -> PyResult<PyVal> {
    let index = PyVal::from(index.clone());
    let doubled = match n {
        PDFObject::Int(_) | PDFObject::Bool(_) | PDFObject::Float(_) => {
            let n = n.as_num().expect("a number");
            match n.mul(&pycompat::Num::from(2)) {
                Ok(pycompat::Num::Int(i)) => PyVal::from(i),
                Ok(pycompat::Num::Float(f)) => PyVal::Obj(PDFObject::Float(f)),
                Err(e) => return Err(e),
            }
        }
        // a sequence repeated: then sequence + int raises
        PDFObject::Bytes(_) | PDFObject::List(_) => PyVal::Obj(n.clone()),
        other => {
            return Err(exc::TypeError(format!("unsupported operand type(s) for *: '{}' and 'int'", other.type_name())))
        }
    };
    doubled.add(&index)
}

/// `objs[i]`: `None` for an `IndexError`, a `TypeError` for a non-int.
fn list_index(objs: &[PDFObject], i: &PyVal) -> PyResult<Option<PDFObject>> {
    let Some(i) = i.as_index() else {
        return Err(exc::TypeError(format!("list indices must be integers or slices, not {}", i.type_name())));
    };
    let Some(i) = i.as_i64() else {
        // IndexError("cannot fit 'int' into an index-sized integer")
        return Ok(None);
    };
    let len = objs.len() as i64;
    let i = if i < 0 { i + len } else { i };
    Ok(usize::try_from(i).ok().and_then(|i| objs.get(i)).cloned())
}

// `lookup`, nested in pdfminer/pdfdocument.py:910 PDFDocument.lookup_name
fn lookup(d: &PDFDict, cat: &str, key: &PyVal) -> PyResult<PDFObject> {
    let _frame = py_frame()?;
    if let Some(limits) = d.get("Limits") {
        let limits = list_value(&limits)?.to_vec();
        let [k1, k2]: [PDFObject; 2] = match limits.len() {
            2 => limits.try_into().map_err(|_| exc::ValueError(""))?,
            n @ (0 | 1) => return Err(exc::ValueError(format!("not enough values to unpack (expected 2, got {n})"))),
            _ => return Err(exc::ValueError("too many values to unpack (expected 2)")),
        };
        if key.compare(&PyVal::Obj(k1), "<")? || PyVal::Obj(k2).compare(key, "<")? {
            return Ok(PDFObject::Null);
        }
    }
    if let Some(names) = d.get("Names") {
        let objs = list_value(&names)?.to_vec();
        // names = dict(choplist(2, objs)); return names[key]: the generator
        // takes a frame one level down when created and at each step (from
        // C, dict())
        leaf(1)?;
        let mut found: Option<PDFObject> = None;
        for pair in choplist(2, &objs) {
            let k = ObjId::new(pair[0].clone())?;
            if let PyVal::Obj(key) = key {
                if ObjId::new(key.clone())? == k {
                    found = Some(pair[1].clone());
                }
            }
        }
        return found.ok_or_else(|| match pyval_repr(key) {
            Ok(r) => exc::KeyError(r),
            Err(e) => e,
        });
    }
    if let Some(kids) = d.get("Kids") {
        for c in list_value(&kids)?.to_vec() {
            let v = lookup(&dict_value(&c)?, cat, key)?;
            if v.is_truthy() {
                return Ok(v);
            }
        }
    }
    Err(exc::PDFKeyError(cat_key_repr(cat, key)?))
}

impl PDFResolver for PDFDocument {
    fn getobj(&self, objid: &PyInt) -> PyResult<PDFObject> {
        PDFDocument::getobj(self, &ObjId::int(objid.clone()))
    }
}

// ---------------------------------------------------------------------------
// Outlines

/// One `search(entry, level)` generator of `get_outlines`.
struct OutlineFrame {
    entry: PDFObject,
    level: PyInt,
    /// 0: not started; 1: `First` searched; 2: `Next` searched.
    step: u8,
    dict: Option<PDFDict>,
}

impl OutlineFrame {
    fn new(entry: PDFObject, level: PyInt) -> OutlineFrame {
        OutlineFrame { entry, level, step: 0, dict: None }
    }
}

/// An outline entry: `(level, title, dest, action, se)`.
pub type OutlineEntry = (PyInt, String, PDFObject, PDFObject, PDFObject);

/// The generator returned by `get_outlines()`.
pub struct Outlines {
    stack: Vec<OutlineFrame>,
    done: bool,
}

impl Outlines {
    // `search`, nested in pdfminer/pdfdocument.py:874 PDFDocument.get_outlines
    fn step(&mut self) -> PyResult<Option<OutlineEntry>> {
        loop {
            // The generators of the `yield from` chain are resumed one
            // inside the other: a frame each.
            let _frames = pycompat::recursion::py_frames(self.stack.len() as u32)?;
            let Some(top) = self.stack.last_mut() else {
                return Ok(None);
            };
            match top.step {
                0 => {
                    let entry = dict_value(&top.entry)?;
                    top.step = 1;
                    top.dict = Some(entry.clone());
                    if entry.contains_key("Title") && (entry.contains_key("A") || entry.contains_key("Dest")) {
                        let title = decode_text_counted(&str_value(&entry.get("Title").expect("checked"))?)?;
                        let dest = entry.get("Dest").unwrap_or(PDFObject::Null);
                        let action = entry.get("A").unwrap_or(PDFObject::Null);
                        let se = entry.get("SE").unwrap_or(PDFObject::Null);
                        return Ok(Some((top.level.clone(), title, dest, action, se)));
                    }
                }
                1 => {
                    top.step = 2;
                    let entry = top.dict.clone().expect("set at step 0");
                    if entry.contains_key("First") && entry.contains_key("Last") {
                        let level = top.level.add(&PyInt::Small(1));
                        let first = entry.get("First").expect("checked");
                        self.stack.push(OutlineFrame::new(first, level));
                    }
                }
                2 => {
                    top.step = 3;
                    let entry = top.dict.clone().expect("set at step 0");
                    if let Some(next) = entry.get("Next") {
                        let level = top.level.clone();
                        self.stack.push(OutlineFrame::new(next, level));
                    }
                }
                _ => {
                    self.stack.pop();
                }
            }
        }
    }
}

impl Iterator for Outlines {
    type Item = PyResult<OutlineEntry>;

    fn next(&mut self) -> Option<PyResult<OutlineEntry>> {
        if self.done {
            return None;
        }
        match self.step() {
            Ok(Some(entry)) => Some(Ok(entry)),
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

// ---------------------------------------------------------------------------
// PageLabels

/// pdfminer/pdfdocument.py:1021 `PageLabels`: PageLabels from the document
/// catalog (section 8.3.1 of the PDF Reference), a `NumberTree`.
pub struct PageLabels(pub NumberTree);

impl PageLabels {
    /// `PageLabels(obj)` (`NumberTree.__init__`)
    pub fn new(obj: &PDFObject) -> PyResult<PageLabels> {
        Ok(PageLabels(NumberTree::new(obj)?))
    }

    // pdfminer/pdfdocument.py:1028 PageLabels.labels
    /// The `labels` generator (its body runs at the first `next()`).
    pub fn labels(self) -> PageLabelsIter {
        PageLabelsIter { tree: self, state: LabelsState::NotStarted }
    }

    // pdfminer/pdfdocument.py:1059 PageLabels._format_page_label
    /// Format page label value in a specific style
    pub fn _format_page_label(value: &PyInt, style: &PDFObject) -> PyResult<String> {
        let _frame = py_frame()?;
        // style is LIT(name): LIT is called, then the identity compared
        let is = |name: &str| -> PyResult<bool> {
            let lit = LIT(name);
            py_intern(Symbol::Literal(lit))?;
            Ok(matches!(style, PDFObject::Literal(s) if *s == lit))
        };
        Ok(if matches!(style, PDFObject::Null) {
            String::new()
        } else if is("D")? {
            // Decimal arabic numerals
            value.py_str()?
        } else if is("R")? {
            // Uppercase roman numerals
            leaf(1)?; // format_int_roman
            format_int_roman(value)?.to_uppercase()
        } else if is("r")? {
            // Lowercase roman numerals
            leaf(1)?; // format_int_roman
            format_int_roman(value)?
        } else if is("A")? {
            // Uppercase letters A-Z, AA-ZZ...
            leaf(1)?; // format_int_alpha
            format_int_alpha(value)?.to_uppercase()
        } else if is("a")? {
            // Lowercase letters a-z, aa-zz...
            leaf(1)?; // format_int_alpha
            format_int_alpha(value)?
        } else {
            LOG.warning_with(|| Ok(format!("Unknown page label style: {}", style.repr()?)))?;
            String::new()
        })
    }
}

enum LabelsState {
    NotStarted,
    /// In range `next - 1` of `ranges`: the current value and its end
    /// (`None`: `itertools.count`).
    Running {
        ranges: Vec<(PDFObject, PDFObject)>,
        next: usize,
        value: PyInt,
        end: Option<PyInt>,
        style: PDFObject,
        prefix: String,
    },
    Done,
}

/// The generator `PageLabels.labels`.
pub struct PageLabelsIter {
    tree: PageLabels,
    state: LabelsState,
}

impl PageLabelsIter {
    /// Enters range number `next` (1-based, `enumerate(ranges, 1)`).
    fn enter(ranges: Vec<(PDFObject, PDFObject)>, next: usize) -> PyResult<LabelsState> {
        let (start, label_dict_unchecked) = ranges[next - 1].clone();
        let label_dict = dict_value(&label_dict_unchecked)?;
        let style = label_dict.get("S").unwrap_or(PDFObject::Null);
        let prefix = decode_text_counted(&str_value(
            &label_dict.get("P").unwrap_or_else(|| PDFObject::Bytes(Rc::from(&b""[..]))),
        )?)?;
        let first_value = int_value(&label_dict.get("St").unwrap_or(PDFObject::Int(PyInt::Small(1))))?;
        let end = if next == ranges.len() {
            // This is the last specified range. It continues until the end
            // of the document.
            None
        } else {
            let end = ranges[next].0.as_num().expect("number tree keys are ints");
            let start = start.as_num().expect("number tree keys are ints");
            let range_length = match end.sub(&start)? {
                pycompat::Num::Int(i) => i,
                pycompat::Num::Float(_) => unreachable!("ints"),
            };
            Some(first_value.add(&range_length))
        };
        Ok(LabelsState::Running { ranges, next, value: first_value, end, style, prefix })
    }

    fn step(&mut self) -> PyResult<Option<String>> {
        // the generator's frame, resumed
        let _frame = py_frame()?;
        loop {
            match &mut self.state {
                LabelsState::Done => return Ok(None),
                LabelsState::NotStarted => {
                    let mut ranges = self.tree.0.values()?;
                    // The tree must begin with page index 0
                    let starts_at_0 = ranges
                        .first()
                        .is_some_and(|(k, _)| k.as_num().is_some_and(|k| k.py_eq(&pycompat::Num::from(0))));
                    if !starts_at_0 {
                        // (with settings.STRICT: PDFSyntaxError("PageLabels is missing page index 0"))
                        // Try to cope, by assuming empty labels for the initial pages
                        ranges.insert(0, (PDFObject::Int(PyInt::ZERO), PDFObject::Dict(PDFDict::new())));
                    }
                    self.state = Self::enter(ranges, 1)?;
                }
                LabelsState::Running { ranges, next, value, end, style, prefix } => {
                    if end.as_ref().is_some_and(|end| *value >= *end) {
                        let (ranges, next) = (std::mem::take(ranges), *next + 1);
                        if next > ranges.len() {
                            self.state = LabelsState::Done;
                            continue;
                        }
                        self.state = Self::enter(ranges, next)?;
                        continue;
                    }
                    let label = PageLabels::_format_page_label(value, style)?;
                    *value = value.add(&PyInt::Small(1));
                    return Ok(Some(format!("{prefix}{label}")));
                }
            }
        }
    }
}

impl Iterator for PageLabelsIter {
    type Item = PyResult<String>;

    fn next(&mut self) -> Option<PyResult<String>> {
        match self.step() {
            Ok(Some(label)) => Some(Ok(label)),
            Ok(None) => None,
            Err(e) => {
                self.state = LabelsState::Done;
                Some(Err(e))
            }
        }
    }
}
