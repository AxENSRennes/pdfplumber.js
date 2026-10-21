//! The operations JavaScript calls, and the response they produce
//! (`docs/js-runtime.md`, "ABI").
//!
//! A request is a `PJB1` list `[op, handle, args, min_level]` plus an
//! optional byte buffer (`data`: the PDF, a CMap file). [`Core::call`] runs
//! the operation inside a diagnostics collector (`min_level` 0: none) and
//! returns the response:
//!
//! ```text
//! 0  "PJR1"
//! 4  u8 status       0 ok, 1 Python exception, 2 CMaps missing
//! 5  u8 format       of the payload: 0 PJB1, 1 JSON
//! 6  u8 flags        bit 0: the JSON payload needs the fixup walk
//! 7  u8 0
//! 8  u32 payload offset, u32 payload length
//! 16 u32 diagnostics offset, u32 diagnostics length (a PJB1 block)
//! 24 payload, diagnostics (each at a multiple of 8)
//! ```
//!
//! The payload of an exception is the dict of `errors.rs` and that of
//! missing CMaps the list of their names, both in `PJB1`; the payload of a
//! result is in the format chosen by `set_format` (`PJB1` by default).
//!
//! The operations: documents (`pdfplumber.open`, the `PDF` the handle
//! keeps) and their pages, original and derived, whose operations are
//! pdfplumber's (`crate::pages`); the parts of the core exposed for the
//! JavaScript tests over objects given by JavaScript (text extraction,
//! words, search spans, tables, predefined CMaps); the transport tests and
//! the encoding benchmark.
//!
//! **Depth.** Every request runs as a statement at the top level of a
//! Python script ([`ENTRY_DEPTH`]: one Python frame, two C units in use,
//! measured by `oracle/unit/js_runtime.py`), so that a `RecursionError`
//! happens where it happens in such a script.
//!
//! **Void calls.** A call that asked for a predefined CMap JavaScript has
//! not handed over yet ([`cmaps`]) ran without it, as if pdfminer had no
//! such file: whatever it computed or cached may differ from Python's run
//! with every CMap installed (`PDFCIDFont.get_cmap_from_spec` caches an
//! empty `CMap()` in the document's font cache). The call is void: its
//! result and diagnostics are discarded, the handles it created are
//! removed, and the modeled state of the Python process is put back as it
//! was before the call (`pdfminer::process_state`: symbols interned, `re`
//! patterns compiled, `typing` subscriptions cached, logger caches,
//! specialized call sites, the CMaps `CMapDB` cached), so that the re-run
//! reaches Python's recursion limits where Python's single run with every
//! CMap installed does. The documents the call reached are put back as
//! they were before it too: each drops its state (its `PDF`, the pages of
//! its page handles, [`Object::reset`]), which [`Core`] then rebuilds by
//! replaying, results and diagnostics discarded, every earlier call that
//! reached the document ([`Entry`], from `doc_open` on), each refilling
//! the handles it had created; the process state is put back after the
//! replay too. So the document is the object Python's single run has at
//! that point, caches, decoded streams and in-place changes included (the
//! metadata decoding, `Page.annots`), not a document opened again: the
//! re-run then sees what Python's next statement sees.

use std::collections::VecDeque;
use std::rc::Rc;

use pdfminer::cmapdb::{Attrs, CMap, CMapBase, CMapDB, Unichr};
use pdfminer::process_state::ProcessState;
use pdfplumber::obj::{bbox_from_value, bbox_value, Bbox, HashKey, Obj, PyValue};
use pdfplumber::table::{TableFinder, TablePage, TableSettings};
use pdfplumber::utils::text::{
    chars_to_textmap, dedupe_chars, extract_text_kwargs, extract_words_kwargs, Kwargs, RegexMatch, TextMap,
};
use pycompat::diagnostics::{self, Level};
use pycompat::recursion::{with_depth, Depth, C_RECURSION_LIMIT};
use pycompat::{exc, ExcKind, PyErr, PyInt, PyResult, PyStr};
use rustc_hash::FxHashMap;

use crate::cmaps;
use crate::errors::write_error;
use crate::handles::Handles;
use crate::pages::{PageObj, TableObj};
use crate::strings::PyStrRef;
use crate::wire::binary::BinWriter;
use crate::wire::json::JsonWriter;
use crate::wire::{binary, write_value, Writer};

pub const STATUS_OK: u8 = 0;
pub const STATUS_ERROR: u8 = 1;
pub const STATUS_NEED_CMAPS: u8 = 2;

pub const FORMAT_BINARY: u8 = 0;
pub const FORMAT_JSON: u8 = 1;

/// The most diagnostics one call keeps (the others are counted).
pub const MAX_DIAGNOSTICS: usize = 1000;

/// A result encoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Binary,
    Json,
}

impl Format {
    fn parse(v: &PyValue) -> PyResult<Format> {
        match v.as_str() {
            Some("binary") => Ok(Format::Binary),
            Some("json") => Ok(Format::Json),
            _ => Err(exc::ValueError(format!("unknown result format {}", v.repr()))),
        }
    }
}

/// Where every request runs: a statement at the top level of a Python
/// script (`python script.py`: the module's frame, and the two C units of
/// the evaluation loop the interpreter entered; `depth_here()` there gives
/// 1 frame and 9 998 units left, `oracle/unit/js_runtime.py`, `depth.json`).
pub const ENTRY_DEPTH: Depth = Depth { py: 1, c: C_RECURSION_LIMIT - 9998 };

/// A document: the bytes and the options of `open()`, and the state
/// derived from them ([`DocState`]), dropped by [`Document::reset`] when a
/// call that reached it is void, and rebuilt by the replay of the calls
/// that reached it before (see the module documentation).
pub struct Document {
    pub data: Rc<[u8]>,
    pub options: PyValue,
    pub state: DocState,
}

/// The state a document derives from its bytes.
#[derive(Default)]
pub struct DocState {
    /// pdfplumber's `PDF` (`pdfplumber.open(io.BytesIO(data),
    /// **options)`): its `PDFDocument`, its `PDFResourceManager` (whose
    /// font cache holds the fonts, with the CMaps they loaded), its pages
    /// and their caches. `None` only between a reset and the replay that
    /// rebuilds it.
    pub pdf: Option<Rc<pdfplumber::pdf::PDF>>,
    /// CMaps by name, as fonts cache them (an empty `CMap()` for a name
    /// `CMapDB` did not find): the test operation `doc_cmap_decode`.
    pub cmaps: FxHashMap<String, Rc<dyn CMapBase>>,
}

impl Document {
    /// Drops the state derived from the bytes.
    pub fn reset(&mut self) {
        self.state = DocState::default();
    }
}

/// An object JavaScript holds a handle to.
pub enum Object {
    Document(Document),
    /// A page of a document, or a page derived from one (`crop`,
    /// `filter`...).
    Page(PageObj),
    /// A table a page found (`Table.extract` runs on it).
    Table(TableObj),
    TextMap(TextMap),
    /// A value kept for the benchmark, or derived by a test operation.
    Value(PyValue),
}

impl Object {
    /// The document whose state the object holds (`None`: none).
    pub fn document(&self, handle: u32) -> Option<u32> {
        match self {
            Object::Document(_) => Some(handle),
            Object::Page(p) => Some(p.doc),
            Object::Table(t) => Some(t.doc),
            Object::TextMap(_) | Object::Value(_) => None,
        }
    }

    /// Drops what the object derived from its document (see the module
    /// documentation, "Void calls"). Every variant says what it keeps.
    pub fn reset(&mut self) {
        match self {
            Object::Document(d) => d.reset(),
            Object::Page(p) => p.page = None,
            Object::Table(t) => t.table = None,
            // computed from the arguments of the call that made it, with
            // no CMap: nothing to drop
            Object::TextMap(_) => {}
            Object::Value(_) => {}
        }
    }
}

/// A call that reached a document (one operation of a request; those of
/// a `sequence` each), replayed to rebuild the document after a void call.
#[derive(Clone)]
pub struct Entry {
    pub op: String,
    pub handle: u32,
    pub args: PyValue,
    pub data: Option<Rc<[u8]>>,
    /// The handles it created, in order: the replay refills them.
    pub created: Vec<u32>,
    pub doc: u32,
    /// The call it belongs to ([`Core::calls`]).
    pub call: u64,
}

/// A value written by hand.
pub type WriteFn = Box<dyn Fn(&mut dyn Writer) -> PyResult<()>>;

/// What an operation returns.
pub enum Out {
    Value(PyValue),
    /// A value written by hand (values a [`PyValue`] cannot hold).
    Written(WriteFn),
    /// A value in a given format (the benchmark).
    Encoded(Format, PyValue),
}

/// The state of the boundary: the handle table, the result format, and
/// the calls that reached each document.
pub struct Core {
    pub handles: Handles<Object>,
    pub format: Format,
    /// The handles created by the running call (removed if it is void).
    pub(crate) created: Vec<u32>,
    /// Every call that reached a live document, in order (see the module
    /// documentation, "Void calls").
    pub(crate) history: Vec<Entry>,
    /// The document the running operation reached.
    pub(crate) op_doc: Option<u32>,
    /// The documents the running call reached.
    pub(crate) touched: Vec<u32>,
    /// While replaying: the handles the replayed operation created, which
    /// it refills instead of creating new ones.
    pub(crate) replay: Option<VecDeque<u32>>,
    /// The number of calls so far (the running one included).
    pub(crate) calls: u64,
}

impl Default for Core {
    fn default() -> Self {
        Core::new()
    }
}

pub(crate) fn arg(args: &[PyValue], i: usize) -> PyResult<&PyValue> {
    args.get(i).ok_or_else(|| exc::TypeError(format!("missing argument {i}")))
}

pub(crate) fn seq(v: &PyValue) -> PyResult<&[PyValue]> {
    v.as_seq().ok_or_else(|| exc::TypeError(format!("expected a list, got {}", v.type_name())))
}

pub(crate) fn objs(v: &PyValue) -> PyResult<Vec<Obj>> {
    seq(v)?
        .iter()
        .map(|o| match o {
            PyValue::Dict(d) => Ok(d.clone()),
            other => Err(exc::TypeError(format!("expected a dict, got {}", other.type_name()))),
        })
        .collect()
}

pub(crate) fn kwargs(v: &PyValue) -> PyResult<Kwargs> {
    match v {
        PyValue::None => Ok(Obj::new()),
        PyValue::Dict(d) => Ok(d.clone()),
        other => Err(exc::TypeError(format!("expected keyword arguments, got {}", other.type_name()))),
    }
}

pub(crate) fn int(v: &PyValue) -> PyResult<i64> {
    match v {
        PyValue::Int(i) => i.as_i64().ok_or_else(|| exc::OverflowError("int too large")),
        other => Err(exc::TypeError(format!("expected an int, got {}", other.type_name()))),
    }
}

pub(crate) fn str_arg(v: &PyValue) -> PyResult<&str> {
    v.as_str().ok_or_else(|| exc::TypeError(format!("expected a str, got {}", v.type_name())))
}

pub(crate) fn handle(v: &PyValue) -> PyResult<u32> {
    u32::try_from(int(v)?).map_err(|_| exc::ValueError("invalid handle"))
}

pub(crate) fn list_of_objs(objs: Vec<Obj>) -> PyValue {
    PyValue::list(objs.into_iter().map(PyValue::Dict))
}

/// The exception class of a name (`type(e).__name__`, or the trace name
/// for the classes that share one: `zlib.error`).
fn exc_kind(name: &str) -> PyResult<ExcKind> {
    ExcKind::ALL
        .iter()
        .copied()
        .find(|k| k.trace_name() == name || crate::errors::qualified_name(*k) == name)
        .ok_or_else(|| exc::ValueError(format!("unknown exception class {name:?}")))
}

fn level(n: i64) -> Option<Level> {
    [Level::Debug, Level::Info, Level::Warning, Level::Error, Level::Critical].into_iter().find(|l| *l as i64 == n)
}

/// A page made of objects given by JavaScript (what `TableFinder` reads).
struct ObjectsPage {
    chars: Vec<Obj>,
    edges: Vec<Obj>,
    bbox: Bbox,
}

impl TablePage for ObjectsPage {
    fn chars(&self) -> PyResult<Vec<Obj>> {
        Ok(self.chars.clone())
    }
    fn edges(&self) -> PyResult<Vec<Obj>> {
        Ok(self.edges.clone())
    }
    fn bbox(&self) -> PyResult<Bbox> {
        Ok(self.bbox.clone())
    }
}

pub(crate) fn opt_bbox(b: &Option<Bbox>) -> PyValue {
    b.as_ref().map_or(PyValue::None, bbox_value)
}

/// The number of the group `m.group(index)` reads (CPython 3.12
/// `Modules/_sre/sre.c` `match_getindex`): an int (a bool is one) is the
/// number itself, clipped to the `Py_ssize_t` range
/// (`PyNumber_AsSsize_t(index, NULL)`); any other value is looked up in the
/// pattern's named groups (`self->pattern->groupindex`, only set when the
/// pattern has some), after being hashed (`TypeError` for an unhashable
/// one); a number outside `0..m.groups` or a name not found is
/// `IndexError("no such group")` (a number past the last group: from
/// `TextMap::search_with`, which reads the group's span).
///
/// JavaScript gives the named groups of each match (`indices.groups`:
/// name → span, `None` for a group that did not take part), not their
/// numbers: a name resolves to a group with that span (or that did not take
/// part), whose `m.group`, `m.start` and `m.end`, all `search` reads, are
/// those of the named group (`named` is `None` for a pattern without named
/// groups).
pub(crate) fn group_index(index: &PyValue, m: &RegexMatch, named: Option<&NamedGroups>) -> PyResult<usize> {
    let no_such_group = || exc::IndexError("no such group");
    match index {
        PyValue::Bool(b) => Ok(usize::from(*b)),
        PyValue::Int(i) => i.as_i64().and_then(|i| usize::try_from(i).ok()).ok_or_else(no_such_group),
        other => {
            let Some(named) = named else { return Err(no_such_group()) };
            HashKey::new(other.clone())?;
            let PyValue::Str(name) = other else { return Err(no_such_group()) };
            let (_, span) = named.iter().find(|(n, _)| n == name).ok_or_else(no_such_group)?;
            m.spans.iter().position(|s| s == span).ok_or_else(|| {
                exc::ValueError(format!("the span of group {:?} is no group's span", name.to_string_lossy()))
            })
        }
    }
}

/// The named groups of a match: name → span (`None`: did not take part).
pub(crate) type NamedGroups = Vec<(PyStr, Option<(usize, usize)>)>;

/// UTF-16 offsets of a JavaScript match → code point offsets of `s`.
pub(crate) struct Utf16Index {
    /// `cp[u]`: the code point offset at UTF-16 offset `u`, `None` inside
    /// a surrogate pair.
    cp: Vec<Option<usize>>,
}

impl Utf16Index {
    /// The offsets of `s` as JavaScript holds it (`PyStr::to_utf16`: a
    /// lone surrogate is one unit, a supplementary code point two).
    pub(crate) fn new(s: &PyStr) -> Utf16Index {
        let mut cp = Vec::with_capacity(s.as_bytes().len() + 1);
        let mut n = 0;
        for (i, c) in s.code_points().enumerate() {
            cp.push(Some(i));
            if c >= 0x10000 {
                cp.push(None);
            }
            n = i + 1;
        }
        cp.push(Some(n));
        Utf16Index { cp }
    }

    /// A JavaScript span (`[start, end]`, or `None`).
    pub(crate) fn span(&self, s: &PyValue) -> PyResult<Option<(usize, usize)>> {
        match s {
            PyValue::None => Ok(None),
            s => {
                let s = seq(s)?;
                Ok(Some((self.convert(arg(s, 0)?)?, self.convert(arg(s, 1)?)?)))
            }
        }
    }

    fn convert(&self, u: &PyValue) -> PyResult<usize> {
        let u = usize::try_from(int(u)?).map_err(|_| exc::ValueError("negative match offset"))?;
        match self.cp.get(u) {
            Some(Some(i)) => Ok(*i),
            Some(None) => Err(exc::ValueError(format!("match offset {u} splits a surrogate pair"))),
            None => Err(exc::ValueError(format!("match offset {u} outside the text"))),
        }
    }
}

impl Core {
    pub fn new() -> Core {
        cmaps::install();
        Core {
            handles: Handles::new(),
            format: Format::Binary,
            created: Vec::new(),
            history: Vec::new(),
            op_doc: None,
            touched: Vec::new(),
            replay: None,
            calls: 0,
        }
    }

    /// Stores a new object; while replaying, refills the handle the
    /// replayed call created instead (a page freed since stays freed).
    pub(crate) fn insert(&mut self, o: Object) -> PyResult<PyValue> {
        if let Some(queue) = &mut self.replay {
            let h = queue
                .pop_front()
                .ok_or_else(|| PyErr::new(ExcKind::RuntimeError, "replay: more handles than the call made"))?;
            if let Some(slot) = self.handles.get_mut(h) {
                let freed = matches!(
                    slot,
                    Object::Page(PageObj { freed: true, .. }) | Object::Table(TableObj { freed: true, .. })
                );
                *slot = o;
                match slot {
                    Object::Page(p) => p.freed = freed,
                    Object::Table(t) => t.freed = freed,
                    _ => {}
                }
            }
            return Ok(PyValue::Int(PyInt::from(h)));
        }
        let h = self.handles.insert(o).ok_or_else(|| exc::MemoryError("too many handles"))?;
        self.created.push(h);
        Ok(PyValue::Int(PyInt::from(h)))
    }

    /// Notes that the running operation reached the document `doc`. An
    /// operation reaches one document, that of the handle it acts on (no
    /// operation takes two documents' handles; a `sequence` records each of
    /// its operations): its replay rebuilds that document only.
    pub(crate) fn reach(&mut self, doc: u32) -> PyResult<()> {
        self.op_doc = Some(doc);
        if !self.touched.contains(&doc) {
            self.touched.push(doc);
        }
        Ok(())
    }

    fn document(&mut self, h: &PyValue) -> PyResult<&mut Document> {
        let h = handle(h)?;
        if !matches!(self.handles.get(h), Some(Object::Document(_))) {
            return Err(exc::ValueError("invalid handle"));
        }
        self.reach(h)?;
        match self.handles.get_mut(h) {
            Some(Object::Document(d)) => Ok(d),
            _ => Err(exc::ValueError("invalid handle")),
        }
    }

    fn textmap(&self, h: &PyValue) -> PyResult<&TextMap> {
        match self.handles.get(handle(h)?) {
            Some(Object::TextMap(t)) => Ok(t),
            _ => Err(exc::ValueError("invalid handle")),
        }
    }

    /// Runs one operation.
    pub fn op(&mut self, name: &str, h: u32, args: &PyValue, data: Option<Rc<[u8]>>) -> PyResult<Out> {
        let a = || seq(args);
        Ok(Out::Value(match name {
            "info" => PyValue::Dict(Obj::from_pairs([
                ("version", PyValue::str(env!("CARGO_PKG_VERSION"))),
                ("pdfminer", PyValue::str("20260107")),
                ("pdfplumber", PyValue::str("0.11.9")),
                ("handles", PyValue::from(self.handles.len() as i64)),
                // the calls the documents' replays would run
                ("history", PyValue::from(self.history.len() as i64)),
            ])),
            "set_format" => {
                self.format = Format::parse(args)?;
                PyValue::None
            }
            "echo" => args.clone(),
            "free" => {
                self.free(h)?;
                PyValue::None
            }
            "test_values" => return Ok(Out::Written(Box::new(write_test_values))),
            "echo_wtf8" => {
                // a `str` given as generalized UTF-8 bytes, returned as a `str`
                let PyValue::Bytes(b) = args else { return Err(exc::TypeError("echo_wtf8 takes bytes")) };
                let b = b.clone();
                return Ok(Out::Written(Box::new(move |w| w.py_str(PyStrRef::Wtf8(&b)))));
            }
            "exception_kinds" => {
                PyValue::list(ExcKind::ALL.iter().map(|k| PyValue::str(&crate::errors::qualified_name(*k))))
            }
            "raise" => {
                let a = a()?;
                // the message is any str (str(e) may hold a lone surrogate)
                let PyValue::Str(message) = arg(a, 1)? else {
                    return Err(exc::TypeError(format!("expected a str, got {}", arg(a, 1)?.type_name())));
                };
                return Err(PyErr::with_message(exc_kind(str_arg(arg(a, 0)?)?)?, message));
            }
            "warn" => {
                for w in a()? {
                    let w = seq(w)?;
                    let lvl = level(int(arg(w, 1)?)?).ok_or_else(|| exc::ValueError("unknown level"))?;
                    let msg = str_arg(arg(w, 2)?)?.to_owned();
                    diagnostics::emit(str_arg(arg(w, 0)?)?, lvl, || msg);
                }
                PyValue::None
            }
            "cmap_configure" => {
                let names = a()?.iter().map(|n| str_arg(n).map(str::to_owned)).collect::<PyResult<Vec<_>>>()?;
                cmaps::configure(names);
                PyValue::None
            }
            "cmap_add" => {
                let data = data.ok_or_else(|| exc::TypeError("cmap_add needs the file"))?;
                cmaps::add(str_arg(args)?, &data)?;
                PyValue::None
            }
            "cmap_decode" => {
                let a = a()?;
                let PyValue::Bytes(code) = arg(a, 1)? else { return Err(exc::TypeError("code must be bytes")) };
                let cmap = CMapDB::get_cmap(str_arg(arg(a, 0)?)?)?;
                PyValue::list(cmap.decode(code)?.into_iter().map(PyValue::Int))
            }
            "cmap_unichr" => {
                let a = a()?;
                let umap = CMapDB::get_unicode_map(str_arg(arg(a, 0)?)?, arg(a, 1)?.truthy())?;
                let mut out = Vec::new();
                for cid in seq(arg(a, 2)?)? {
                    let PyValue::Int(cid) = cid else { return Err(exc::TypeError("a cid must be an int")) };
                    // (a `UnicodeMap` from `get_unicode_map` only raises
                    // KeyError: the guard is for an `IdentityUnicodeMap`'s
                    // `chr()` errors, which no request reaches today)
                    out.push(match umap.get_unichr(cid) {
                        Ok(u) => Ok(u),
                        Err(e) if e.is_instance(ExcKind::KeyError) => Err(e.message.clone()),
                        Err(e) => return Err(e),
                    });
                }
                return Ok(Out::Written(Box::new(move |w| {
                    w.list(out.len());
                    for u in &out {
                        match u {
                            Ok(Unichr::Text(s)) => w.str(s),
                            Ok(Unichr::Surrogate(c)) => w.py_str(PyStrRef::CodePoints(&[*c]))?,
                            Err(key) => {
                                w.tuple(2);
                                w.str("KeyError");
                                w.str(key);
                            }
                        }
                    }
                    Ok(())
                })));
            }
            "doc_open" => {
                // pdfplumber.open(io.BytesIO(data), **options)
                let data = data.ok_or_else(|| exc::TypeError("doc_open needs the PDF bytes"))?;
                // pdfplumber.open(...) with **kwargs: the classmethod from C
                let pdf = {
                    let _call = pycompat::recursion::eval_entry()?;
                    crate::pages::open(&data, args)?
                };
                let state = DocState { pdf: Some(Rc::new(pdf)), ..DocState::default() };
                let h = self.insert(Object::Document(Document { data, options: args.clone(), state }))?;
                self.reach(handle(&h)?)?;
                h
            }
            "doc_cmap_decode" => {
                // [document, name, code]: `cmap.decode(code)` with the CMap
                // the document caches for `name`, as a font of the document
                // gets it, cached with the font:
                // pdfminer/pdffont.py:1182 PDFCIDFont.get_cmap_from_spec
                // pdfminer/pdfinterp.py:212 PDFResourceManager.get_font
                // A test of void calls: it reaches its document through an
                // argument, and returns a handle to a value derived from it.
                let a = a()?;
                let PyValue::Bytes(code) = arg(a, 2)? else { return Err(exc::TypeError("code must be bytes")) };
                let name = str_arg(arg(a, 1)?)?;
                let doc = self.document(arg(a, 0)?)?;
                let cmap = match doc.state.cmaps.get(name) {
                    Some(c) => c.clone(),
                    None => {
                        let c = match CMapDB::get_cmap(name) {
                            Ok(c) => c,
                            Err(e) if e.is_instance(ExcKind::CMapDB_CMapNotFound) => {
                                Rc::new(CMap::new(Attrs::default()))
                            }
                            Err(e) => return Err(e),
                        };
                        doc.state.cmaps.insert(name.to_owned(), c.clone());
                        c
                    }
                };
                let cids = PyValue::list(cmap.decode(code)?.into_iter().map(PyValue::Int));
                let derived = self.insert(Object::Value(cids.clone()))?;
                PyValue::tuple([cids, derived])
            }
            "sequence" => {
                // [[op, handle, args], ...]: the operations in turn, in one
                // call (a void call voids them all); their values, in order
                let mut out = Vec::new();
                for r in a()? {
                    let r = seq(r)?;
                    let op = str_arg(arg(r, 0)?)?;
                    if op == "sequence" {
                        return Err(exc::ValueError("nested sequence"));
                    }
                    // (a void call does not undo what `free` did)
                    if op == "free" {
                        return Err(exc::ValueError("free cannot run in a sequence"));
                    }
                    match self.run_op(op, handle(arg(r, 1)?)?, arg(r, 2)?, None)? {
                        Out::Value(v) => out.push(v),
                        _ => return Err(exc::ValueError(format!("{op} cannot run in a sequence"))),
                    }
                }
                PyValue::list(out)
            }
            "doc_info" => match self.handles.get(h) {
                Some(Object::Document(d)) => PyValue::Dict(Obj::from_pairs([
                    ("size", PyValue::from(d.data.len() as i64)),
                    ("options", d.options.clone()),
                ])),
                _ => return Err(exc::ValueError("invalid handle")),
            },
            "extract_text" => {
                let a = a()?;
                PyValue::from(extract_text_kwargs(&objs(arg(a, 0)?)?, &kwargs(arg(a, 1)?)?)?)
            }
            "extract_words" => {
                let a = a()?;
                list_of_objs(extract_words_kwargs(&objs(arg(a, 0)?)?, &kwargs(arg(a, 1)?)?)?)
            }
            "dedupe_chars" => {
                let a = a()?;
                // extra_attrs as given (None: none)
                list_of_objs(dedupe_chars(&objs(arg(a, 0)?)?, arg(a, 1)?, arg(a, 2)?)?)
            }
            "textmap" => {
                let a = a()?;
                let tm = chars_to_textmap(&objs(arg(a, 0)?)?, &kwargs(arg(a, 1)?)?)?;
                let text = PyValue::Str(tm.as_string.clone());
                let h = self.insert(Object::TextMap(tm))?;
                PyValue::tuple([h, text])
            }
            "textmap_lines" => {
                let a = a()?;
                let tm = self.textmap(arg(a, 0)?)?;
                list_of_objs(tm.extract_text_lines(arg(a, 1)?.truthy(), arg(a, 2)?.truthy())?)
            }
            "textmap_search" => {
                // [handle, matches, return_groups, return_chars, main_group,
                // named]: each match is a list of [start, end] or None
                // (UTF-16 offsets); `named`, for a pattern with named
                // groups, has a dict of each match's named groups' spans
                // (`indices.groups`)
                let a = a()?;
                let tm = self.textmap(arg(a, 0)?)?;
                let index = Utf16Index::new(&tm.as_string);
                let mut matches = Vec::new();
                for m in seq(arg(a, 1)?)? {
                    let spans = seq(m)?.iter().map(|s| index.span(s)).collect::<PyResult<Vec<_>>>()?;
                    matches.push(RegexMatch { spans });
                }
                let named = match a.get(5).unwrap_or(&PyValue::None) {
                    PyValue::None => None,
                    v => Some(
                        seq(v)?
                            .iter()
                            .map(|g| {
                                kwargs(g)?
                                    .items()
                                    .iter()
                                    .map(|(k, s)| Ok((k.clone(), index.span(s)?)))
                                    .collect::<PyResult<NamedGroups>>()
                            })
                            .collect::<PyResult<Vec<_>>>()?,
                    ),
                };
                if named.as_ref().is_some_and(|n| n.len() != matches.len()) {
                    return Err(exc::ValueError("one dict of named groups per match"));
                }
                let main_group = arg(a, 4)?;
                let group = |k: usize, m: &RegexMatch| group_index(main_group, m, named.as_ref().map(|n| &n[k]));
                list_of_objs(tm.search_with(&matches, arg(a, 2)?.truthy(), arg(a, 3)?.truthy(), group)?)
            }
            "find_tables" => {
                // [chars, edges, bbox, table_settings, extract_kwargs]
                let a = a()?;
                let page = ObjectsPage {
                    chars: objs(arg(a, 0)?)?,
                    edges: objs(arg(a, 1)?)?,
                    bbox: bbox_from_value(arg(a, 2)?)?,
                };
                let settings = match arg(a, 3)? {
                    PyValue::None => TableSettings::resolve(None)?,
                    v => TableSettings::resolve(Some(&kwargs(v)?))?,
                };
                let extract_kwargs = kwargs(arg(a, 4)?)?;
                let finder = TableFinder::new(&page, settings)?;
                let mut tables = Vec::new();
                for t in &finder.tables {
                    let group = |g: pdfplumber::table::CellGroup| {
                        PyValue::Dict(Obj::from_pairs([
                            ("bbox", bbox_value(&g.bbox)),
                            ("cells", PyValue::list(g.cells.iter().map(opt_bbox))),
                        ]))
                    };
                    let text = t.extract(&page, &extract_kwargs)?;
                    tables.push(PyValue::Dict(Obj::from_pairs([
                        ("bbox", bbox_value(&t.bbox()?)),
                        ("cells", PyValue::list(t.cells.iter().map(bbox_value))),
                        ("rows", PyValue::list(t.rows()?.into_iter().map(group))),
                        ("columns", PyValue::list(t.columns()?.into_iter().map(group))),
                        (
                            "text",
                            PyValue::list(text.into_iter().map(|row| {
                                PyValue::list(row.into_iter().map(|c| c.map_or(PyValue::None, PyValue::from)))
                            })),
                        ),
                    ])));
                }
                PyValue::list(tables)
            }
            "bench_load" => {
                let data = data.ok_or_else(|| exc::TypeError("bench_load needs a PJB1 block"))?;
                let v = binary::decode(&data)?;
                self.insert(Object::Value(v))?
            }
            "bench_encode" => {
                // [format, index]: item `index` of the stored list, or all of it (None)
                let a = a()?;
                let format = Format::parse(arg(a, 0)?)?;
                let Some(Object::Value(v)) = self.handles.get(h) else {
                    return Err(exc::ValueError("invalid handle"));
                };
                let v = match arg(a, 1)? {
                    PyValue::None => v.clone(),
                    i => {
                        let i = usize::try_from(int(i)?).map_err(|_| exc::IndexError("list index out of range"))?;
                        seq(v)?.get(i).cloned().ok_or_else(|| exc::IndexError("list index out of range"))?
                    }
                };
                return Ok(Out::Encoded(format, v));
            }
            _ => match self.page_op(name, h, args)? {
                Some(out) => return Ok(out),
                None => return Err(exc::ValueError(format!("unknown operation {name:?}"))),
            },
        }))
    }

    /// Runs one operation of a request and, when it reached a document,
    /// records it in the history (a replay runs [`Core::op`] itself: it
    /// records nothing; a `sequence` is not recorded, its operations are).
    pub(crate) fn run_op(&mut self, name: &str, h: u32, args: &PyValue, data: Option<Rc<[u8]>>) -> PyResult<Out> {
        let start = self.created.len();
        let outer = self.op_doc.take();
        let result = self.op(name, h, args, data.clone());
        if let Some(doc) = self.op_doc.take() {
            let created = self.created[start..].to_vec();
            let call = self.calls;
            self.history.push(Entry { op: name.to_owned(), handle: h, args: args.clone(), data, created, doc, call });
        }
        self.op_doc = outer;
        result
    }

    /// `free`: a document goes with its pages, tables and history; a page
    /// or a table becomes a tombstone (refused to JavaScript, its page or
    /// table dropped, but kept for the replays of the calls that used it
    /// before); any other object goes.
    fn free(&mut self, h: u32) -> PyResult<()> {
        match self.handles.get_mut(h) {
            None => return Err(exc::ValueError("invalid handle")),
            Some(Object::Page(PageObj { freed: true, .. }) | Object::Table(TableObj { freed: true, .. })) => {
                return Err(exc::ValueError("invalid handle"))
            }
            Some(Object::Page(p)) => {
                p.freed = true;
                p.page = None;
                return Ok(());
            }
            Some(Object::Table(t)) => {
                t.freed = true;
                t.table = None;
                return Ok(());
            }
            Some(Object::Document(_)) => {
                let owned: Vec<u32> =
                    self.handles.iter().filter(|(k, o)| *k != h && o.document(*k) == Some(h)).map(|(k, _)| k).collect();
                for k in owned {
                    self.handles.remove(k);
                }
                self.history.retain(|e| e.doc != h);
            }
            Some(_) => {}
        }
        self.handles.remove(h);
        Ok(())
    }

    /// Undoes what a void call did (see the module documentation): removes
    /// the handles it created and its calls from the history, puts the
    /// process state back, and rebuilds the documents it reached by
    /// replaying their history.
    fn void(&mut self, process: ProcessState) {
        for h in std::mem::take(&mut self.created) {
            self.handles.remove(h);
        }
        let call = self.calls;
        self.history.retain(|e| e.call != call);
        process.restore();
        let docs: Vec<u32> = std::mem::take(&mut self.touched)
            .into_iter()
            .filter(|d| matches!(self.handles.get(*d), Some(Object::Document(_))))
            .collect();
        if docs.is_empty() {
            return;
        }
        for (k, o) in self.handles.iter_mut() {
            if o.document(k).is_some_and(|d| docs.contains(&d)) {
                o.reset();
            }
        }
        let process = ProcessState::capture();
        let entries: Vec<Entry> = self.history.iter().filter(|e| docs.contains(&e.doc)).cloned().collect();
        for e in entries {
            self.replay = Some(e.created.iter().copied().collect());
            // the result is the one the call returned before
            let _ = with_depth(ENTRY_DEPTH, || self.op(&e.op, e.handle, &e.args, e.data.clone()));
            self.replay = None;
            self.op_doc = None;
        }
        for (_, o) in self.handles.iter_mut() {
            match o {
                Object::Page(p) if p.freed => p.page = None,
                Object::Table(t) if t.freed => t.table = None,
                _ => {}
            }
        }
        self.touched.clear();
        // every CMap the replayed calls need was handed over before them
        let missed = cmaps::take_missing();
        debug_assert!(missed.is_empty(), "a replay missed a CMap: {missed:?}");
        process.restore();
    }

    /// Runs a request (see the module documentation) and returns the
    /// response.
    pub fn call(&mut self, request: &[u8], data: Option<Rc<[u8]>>) -> Vec<u8> {
        let mut diag_level = None;
        let mut h = 0;
        self.created.clear();
        self.touched.clear();
        self.op_doc = None;
        self.calls += 1;
        let process = ProcessState::capture();
        let (result, collected) = {
            let parsed = binary::decode(request).and_then(|req| {
                let r = seq(&req)?.to_vec();
                let op = str_arg(arg(&r, 0)?)?.to_owned();
                h = match arg(&r, 1)? {
                    PyValue::None => 0,
                    v => handle(v)?,
                };
                diag_level = level(int(arg(&r, 3)?)?);
                Ok((op, arg(&r, 2)?.clone()))
            });
            match parsed {
                Err(e) => (Err(e), diagnostics::Collected::default()),
                Ok((op, args)) => {
                    let run = || with_depth(ENTRY_DEPTH, || self.run_op(&op, h, &args, data));
                    match diag_level {
                        // above every level: nothing is enabled, nothing formatted
                        None => (run(), diagnostics::Collected::default()),
                        Some(l) => diagnostics::collect(l, MAX_DIAGNOSTICS, run),
                    }
                }
            }
        };
        let missing = cmaps::take_missing();
        if !missing.is_empty() {
            // the re-run emits its own diagnostics: Python's, once
            drop((result, collected));
            self.void(process);
            let none = diagnostics::Collected::default();
            return response(STATUS_NEED_CMAPS, FORMAT_BINARY, false, &encode_names(&missing), &none);
        }
        match result {
            Ok(out) => match self.encode(out) {
                Ok((format, fixup, payload)) => response(STATUS_OK, format, fixup, &payload, &collected),
                Err(e) => response(STATUS_ERROR, FORMAT_BINARY, false, &encode_error(&e), &collected),
            },
            Err(e) => response(STATUS_ERROR, FORMAT_BINARY, false, &encode_error(&e), &collected),
        }
    }

    fn encode(&self, out: Out) -> PyResult<(u8, bool, Vec<u8>)> {
        let (format, value) = match out {
            Out::Value(v) => (self.format, v),
            Out::Encoded(f, v) => (f, v),
            Out::Written(f) => {
                return match self.format {
                    Format::Binary => {
                        let mut w = BinWriter::new();
                        f(&mut w)?;
                        Ok((FORMAT_BINARY, false, w.finish()))
                    }
                    Format::Json => {
                        let mut w = JsonWriter::new();
                        f(&mut w)?;
                        let fixup = w.needs_fixup();
                        Ok((FORMAT_JSON, fixup, w.finish()))
                    }
                };
            }
        };
        match format {
            Format::Binary => Ok((FORMAT_BINARY, false, binary::encode(&value)?)),
            Format::Json => {
                let (text, fixup) = crate::wire::json::encode(&value)?;
                Ok((FORMAT_JSON, fixup, text))
            }
        }
    }
}

fn encode_names(names: &[String]) -> Vec<u8> {
    let mut w = BinWriter::new();
    w.list(names.len());
    for n in names {
        w.str(n);
    }
    w.finish()
}

fn encode_error(e: &PyErr) -> Vec<u8> {
    let mut w = BinWriter::new();
    write_error(&mut w, e);
    w.finish()
}

fn encode_diagnostics(c: &diagnostics::Collected) -> Vec<u8> {
    let mut w = BinWriter::new();
    w.dict(2);
    w.str("entries");
    w.list(c.entries.len());
    for d in &c.entries {
        w.tuple(3);
        w.str(&d.source);
        w.int(&PyInt::from(d.level as i64));
        w.str(&d.message);
    }
    w.str("dropped");
    w.int(&PyInt::from(c.dropped as i64));
    w.finish()
}

fn pad8(buf: &mut Vec<u8>) {
    while !buf.len().is_multiple_of(8) {
        buf.push(0);
    }
}

/// The response envelope (see the module documentation).
pub fn response(status: u8, format: u8, fixup: bool, payload: &[u8], diags: &diagnostics::Collected) -> Vec<u8> {
    let diag = encode_diagnostics(diags);
    let mut out = Vec::with_capacity(32 + payload.len() + diag.len());
    out.extend_from_slice(b"PJR1");
    out.extend_from_slice(&[status, format, u8::from(fixup), 0]);
    out.extend_from_slice(&[0; 16]);
    pad8(&mut out);
    let payload_off = out.len() as u32;
    out.extend_from_slice(payload);
    pad8(&mut out);
    let diag_off = out.len() as u32;
    out.extend_from_slice(&diag);
    out[8..12].copy_from_slice(&payload_off.to_le_bytes());
    out[12..16].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    out[16..20].copy_from_slice(&diag_off.to_le_bytes());
    out[20..24].copy_from_slice(&(diag.len() as u32).to_le_bytes());
    out
}

/// A [`WriteFn`] that cannot fail.
fn plain(f: impl Fn(&mut dyn Writer) + 'static) -> WriteFn {
    Box::new(move |w| {
        f(w);
        Ok(())
    })
}

/// The adversarial values of the transport tests (`js/test/shared/`), as
/// written by hand: the strings come from hand-made buffers in each form a
/// port may keep a `str` in.
fn write_test_values(w: &mut dyn Writer) -> PyResult<()> {
    let big = PyInt::from_decimal("-123456789012345678901234567890").expect("a decimal");
    let pairs: Vec<(&str, WriteFn)> = vec![
        ("nan", plain(|w| w.float(f64::NAN))),
        ("inf", plain(|w| w.float(f64::INFINITY))),
        ("-inf", plain(|w| w.float(f64::NEG_INFINITY))),
        ("-0.0", plain(|w| w.float(-0.0))),
        ("0.1", plain(|w| w.float(0.1))),
        ("5e-324", plain(|w| w.float(5e-324))),
        ("1.7976931348623157e308", plain(|w| w.float(f64::MAX))),
        ("1e21", plain(|w| w.float(1e21))),
        ("float 3.0", plain(|w| w.float(3.0))),
        ("int 3", plain(|w| w.int(&PyInt::from(3)))),
        ("int -2147483649", plain(|w| w.int(&PyInt::from(-2_147_483_649_i64)))),
        ("int 2**53", plain(|w| w.int(&PyInt::from(1_i64 << 53)))),
        ("int 2**53+1", plain(|w| w.int(&PyInt::from((1_i64 << 53) + 1)))),
        ("int big", plain(move |w| w.int(&big))),
        ("str wtf8 lone high", Box::new(|w| w.py_str(PyStrRef::Wtf8(&[b'a', 0xed, 0xa0, 0x80, b'b'])))),
        ("str wtf8 lone low", Box::new(|w| w.py_str(PyStrRef::Wtf8(&[0xed, 0xb0, 0x80])))),
        // two code points that JavaScript cannot tell from the pair U+1F600
        ("str wtf8 split pair", Box::new(|w| w.py_str(PyStrRef::Wtf8(&[0xed, 0xa0, 0xbd, 0xed, 0xb8, 0x80])))),
        ("str utf16 lone", Box::new(|w| w.py_str(PyStrRef::Utf16(&[0xdfff, 0x41, 0xd800])))),
        ("str code points", Box::new(|w| w.py_str(PyStrRef::CodePoints(&[0x1f600, 0xdc00, 0xe9, 0])))),
        ("str nul", plain(|w| w.str("\0x"))),
        ("str escapes", plain(|w| w.str("\"\\\n\t\u{1}\u{7f}\u{2028}é😀"))),
        ("bytes", plain(|w| w.bytes(&[0, 255, 128]))),
        (
            "dict keys",
            Box::new(|w| {
                w.dict(4);
                w.str("__proto__");
                w.int(&PyInt::from(1));
                w.str("\0");
                w.int(&PyInt::from(2));
                w.py_str(PyStrRef::Utf16(&[0xd800]))?;
                w.int(&PyInt::from(3));
                w.str("constructor");
                w.none();
                Ok(())
            }),
        ),
        (
            "pdf objects",
            Box::new(|w| {
                w.list(4);
                w.literal();
                w.str("DeviceRGB");
                w.literal();
                w.bytes(&[0xff]);
                w.keyword(b"BT");
                w.objref();
                w.int(&PyInt::from(12));
                Ok(())
            }),
        ),
        (
            "stream",
            Box::new(|w| {
                w.stream();
                w.int(&PyInt::from(5));
                w.int(&PyInt::from(0));
                w.dict(1);
                w.str("Length");
                w.int(&PyInt::from(3));
                w.bytes(b"abc");
                w.none();
                Ok(())
            }),
        ),
        (
            "deep",
            Box::new(|w| {
                // a list nested 100 000 levels deep
                for _ in 0..100_000 {
                    w.list(1);
                }
                w.none();
                Ok(())
            }),
        ),
    ];
    w.dict(pairs.len() + 1);
    for (k, f) in &pairs {
        w.str(k);
        f(w)?;
    }
    // a table: uniform dicts written by columns, every column type
    w.str("table");
    let row = |i: i64| {
        let f = i as f64;
        PyValue::Dict(Obj::from_pairs([
            ("n", if i == 1 { PyValue::Int(PyInt::from(7)) } else { PyValue::float(f + 0.5) }),
            ("s", PyValue::str(if i % 2 == 0 { "even" } else { "odd" })),
            ("os", if i == 1 { PyValue::None } else { PyValue::str("x") }),
            ("b", PyValue::Bool(i % 2 == 0)),
            ("none", PyValue::None),
            ("seq", if i == 2 { PyValue::None } else { PyValue::tuple([PyValue::from(f), PyValue::from(1_i64)]) }),
            ("on", if i == 0 { PyValue::None } else { PyValue::float(-0.0) }),
            ("g", if i == 0 { PyValue::Int(PyInt::from(1_i64 << 60)) } else { PyValue::list([PyValue::str("a")]) }),
            ("__proto__", PyValue::from(i)),
        ]))
    };
    write_value(w, &PyValue::list((0..3).map(row)))
}
