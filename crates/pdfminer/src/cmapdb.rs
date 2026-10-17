//! Port of `pdfminer/cmapdb.py`: CMaps (character codes to CIDs), unicode
//! maps (CIDs to text), the database of predefined CMaps and the parser of
//! embedded CMaps.
//!
//! # Classes
//!
//! Python's `CMapBase` is the trait [`CMapBase`]; its subclasses are four
//! structs, the class being a field where subclasses share their data:
//!
//! | Python | Rust |
//! |---|---|
//! | `CMapBase()` | [`PlainCMapBase`] |
//! | `CMap`, `FileCMap`, `PyCMap` | [`CMap`] with [`CMapClass`] |
//! | `IdentityCMap`, `IdentityCMapByte` | [`IdentityCMap`] |
//! | `UnicodeMap`, `IdentityUnicodeMap`, `FileUnicodeMap`, `PyUnicodeMap` | [`UnicodeMap`] with [`UnicodeMapClass`] |
//!
//! `attrs` values are the objects of a `def` in a CMap stream ([`PDFObject`])
//! or the `str`/`int` the constructors set ([`AttrValue`]).
//!
//! Dict keys are Python ints ([`PyInt`]); a `bool` used as a key (a
//! `begincidchar` entry `true <0041>`) is stored as `0`/`1`. Python keeps
//! `True` as the key object when the dict has no `1` yet, which only `dump`
//! and `repr` of the dict can show: `True == 1` and both hash alike, so every
//! lookup is the same. Values of `code2cid` are the `int` or `dict`
//! ([`Code2CidValue`]) `FileCMap.add_code2cid` and the predefined data
//! hold; `add_code2cid` is never called by pdfminer itself.
//!
//! # Text with surrogates
//!
//! `chr(cid)` (`IdentityUnicodeMap.get_unichr`, an integer in a
//! `beginbfrange` array) can be a lone surrogate, which a Rust `String`
//! cannot hold: unicode map values are [`Unichr`], text or code points.
//!
//! # Predefined CMaps (`CMapDB`)
//!
//! pdfminer reads `pdfminer/cmap/<name>.json.gz` (about 8 MB of gzipped
//! JSON). The port does not embed them: [`CMapDB`] asks a [`CMapProvider`]
//! for a name and gets the file in a compact binary form
//! (`oracle/codegen/cmaps.py` converts every pdfminer file and documents
//! the format; [`DirCMapProvider`] reads the converted files from a
//! directory, natively and under WASI; the JavaScript side will provide
//! them by `fetch`). A process-wide default provider
//! ([`CMapDB::set_default_provider`]) serves every thread that has not set
//! its own ([`CMapDB::set_provider`]); the caches are per thread
//! (`CMapDB._cmap_cache`, `_umap_cache`: class attributes in Python, so
//! process-wide and never emptied). A thread whose provider is `None` finds
//! no name (`CMapNotFound`), as with an installation without the data. A
//! thread with neither its own provider nor a default gets a `RuntimeError`
//! (no Python counterpart) when it needs a predefined CMap: forgetting to
//! install a provider must not look like that installation, since pdffont
//! turns `CMapNotFound` into an empty CMap and a font without text.
//!
//! `CMapDB._load_data` looks for the file in `$CMAP_PATH` (default
//! `/usr/share/pdfminer/`) then in the package's `cmap` directory, after
//! `os.path.realpath`, and only accepts a path inside the directory. The
//! port models the package directory, whose last components are
//! `pdfminer/cmap` ([`resolve_cmap_name`]): a name such as `../cmap/H` or
//! `x/../H` finds `H` as in Python; a name that leaves `pdfminer/` and comes
//! back through the absolute install path, or a file installed in
//! `$CMAP_PATH`, depends on the machine and is not found.
//!
//! # Recursion depth
//!
//! A font is built at the depth of the page or Form XObject that uses it
//! (`pdffont`), so a `RecursionError` can happen while a CMap is loaded or
//! a `/ToUnicode` stream parsed. These count their Python frames
//! (`docs/architecture.md`, "Profondeur de récursion"):
//!
//! - [`CMapDB::get_cmap`], [`CMapDB::get_unicode_map`] and
//!   [`CMapDB::load_data`], `_load_data` with the frames of the standard
//!   library it runs, which are those of the oracle's CPython 3.12 on its
//!   files: when the name is not found, `os.path.realpath` ->
//!   `_joinrealpath` -> `isabs` (or `join`) -> `_get_sep`, 4 frames below
//!   `_load_data` (`$CMAP_PATH` unset, no symbolic link on the paths:
//!   a name whose path goes through one depends on the machine, as
//!   [`resolve_cmap_name`] says); when it is found (the first load of a
//!   predefined CMap), `json.load` -> `GzipFile.read` -> `_compression`'s
//!   `readall` -> `GzipFile.read` -> `_read_gzip_header` (twice) ->
//!   `_read_exact` -> `GzipFile.read`, 8 frames below, for every file of
//!   pdfminer's data (measured by a profile hook that counts the depth of
//!   each call, `oracle/depth_probe.py`); the `log.debug`
//!   calls take their frames where they are, for the logger cache;
//! - the `CMapParser` methods: its construction (its `__init__`; the
//!   `PSStackParser.__init__` it calls counts its own, as every `psparser`
//!   method does: `nextobject`, `nexttoken`, `popall`, `pop`, `KWD`/`LIT`
//!   interning, their `log.debug`), `run`, `do_keyword`, `_warn_once` and
//!   its `log.warning`.
//!
//! Not modeled: the frames of `UnicodeMap.add_cid2unichr` and the
//! `name2unicode` it calls, and of the `choplist` generator of
//! `do_keyword`: they reach at most 7 frames below the caller of `run`,
//! never deeper than the parser's construction (7 frames, 8 the first time
//! the `psparser` logger is asked since its cache was cleared), which comes
//! first; only `usecmap` (through `get_cmap`) goes deeper. Also not modeled: the
//! import of the UTF-16 codec by the first `bytes.decode("UTF-16BE")` of a
//! Python process (`importlib`, 15 frames below `run`), a property of the
//! process's history. The query methods (`get_unichr`, `decode`) do not
//! count theirs: their caller, `pdffont`, does (`pdffont::frames`).
//! `CMap.use_cmap` (`usecmap` in a stream parsed into a `CMap`, which
//! `pdffont` never does: its parser fills a `FileUnicodeMap`, whose
//! `use_cmap` does nothing) does not count its recursive `copy`, nor does
//! `add_code2cid`, which pdfminer never calls.

use std::cell::RefCell;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, LazyLock, PoisonError, RwLock};

use indexmap::IndexMap;
use num_bigint::BigUint;
use pycompat::io::{BytesIO, Pos};
use pycompat::logging::Logger;
use pycompat::recursion;
use pycompat::repr::str_repr;
use pycompat::{exc, ExcKind, PyErr, PyInt, PyResult};
use rustc_hash::{FxBuildHasher, FxHashMap, FxHashSet};

use crate::encodingdb::{name2unicode, py_chr};
use crate::pdftypes::PDFObject;
use crate::psparser::{literal_name, LiteralName, PSKeyword, PSStackParser, PSStackParserExt, KWD};
use crate::utils::choplist;

/// `log = logging.getLogger(__name__)` of `pdfminer/cmapdb.py`.
static LOG: Logger = Logger::new("pdfminer.cmapdb");

/// The frames below `CMapDB._load_data` of the deepest call of a search
/// that finds nothing (module documentation): `realpath` -> `_joinrealpath`
/// -> `isabs` -> `_get_sep`.
const SEARCH_FRAMES: u32 = 4;

/// The frames below `CMapDB._load_data` of the deepest call of a load
/// (module documentation): `json.load` -> `GzipFile.read` -> `readall` ->
/// `GzipFile.read` -> `_read_gzip_header` -> `_read_gzip_header` ->
/// `_read_exact` -> `GzipFile.read`.
const LOAD_FRAMES: u32 = 8;

// ---------------------------------------------------------------------------
// Values

/// A Python `str` held by a unicode map: text, or a lone surrogate, which
/// UTF-8 cannot carry. A surrogate only comes from `chr()` of one code point
/// (`IdentityUnicodeMap.get_unichr`, an integer in a `beginbfrange` array):
/// `name2unicode` rejects them, the UTF-16 decoder skips them and the
/// predefined maps hold none (`oracle/codegen/cmaps.py` checks).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Unichr {
    Text(Rc<str>),
    /// `chr(c)` for a surrogate code point `c` (U+D800..U+DFFF).
    Surrogate(u32),
}

impl Unichr {
    /// `chr(c)` for a code point below 0x110000.
    pub fn from_code_point(c: u32) -> Unichr {
        match char::from_u32(c) {
            Some(ch) => Unichr::Text(ch.to_string().into()),
            None => Unichr::Surrogate(c),
        }
    }

    /// The text, when it is not a surrogate.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Unichr::Text(s) => Some(s),
            Unichr::Surrogate(_) => None,
        }
    }

    /// `repr(s)` (a surrogate is not printable: `'\udxxx'`).
    pub fn repr(&self) -> String {
        match self {
            Unichr::Text(s) => str_repr(s),
            Unichr::Surrogate(c) => format!("'\\u{c:04x}'"),
        }
    }
}

impl From<String> for Unichr {
    fn from(s: String) -> Unichr {
        Unichr::Text(s.into())
    }
}

impl From<&str> for Unichr {
    fn from(s: &str) -> Unichr {
        Unichr::Text(s.into())
    }
}

/// A value of `CMapBase.attrs`: a `str` or an `int` set by a constructor
/// (`CMapName=name`, `WMode=1`), or an object of a `def` in a CMap stream.
#[derive(Clone, Debug)]
pub enum AttrValue {
    Str(String),
    Obj(PDFObject),
}

impl AttrValue {
    /// `str(v)`.
    pub fn py_str(&self) -> PyResult<String> {
        match self {
            AttrValue::Str(s) => Ok(s.clone()),
            AttrValue::Obj(o) => o.py_str(),
        }
    }
}

impl From<i64> for AttrValue {
    fn from(v: i64) -> AttrValue {
        AttrValue::Obj(PDFObject::Int(PyInt::Small(v)))
    }
}

/// `CMapBase.attrs`: `str` keys, insertion-ordered.
pub type Attrs = IndexMap<String, AttrValue, FxBuildHasher>;

/// `CMap.code2cid`: `dict[int, int | dict]`.
pub type Code2Cid = IndexMap<u32, Code2CidValue, FxBuildHasher>;

/// A value of [`Code2Cid`]: a CID, or the dict of the next byte.
#[derive(Clone, Debug, PartialEq)]
pub enum Code2CidValue {
    Cid(PyInt),
    Map(Code2Cid),
}

/// `UnicodeMap.cid2unichr`: `dict[int, str]`.
pub type Cid2Unichr = IndexMap<PyInt, Unichr, FxBuildHasher>;

/// `str(type(x))` of a class of this module.
fn type_str(class_name: &str) -> String {
    format!("<class 'pdfminer.cmapdb.{class_name}'>")
}

// pdfminer/utils.py:412 nunpack
/// Unpacks variable-length unsigned integers (big endian); `default` (0)
/// for no bytes.
fn nunpack(s: &[u8]) -> PyInt {
    if s.is_empty() {
        return PyInt::ZERO;
    }
    if s.len() <= 7 {
        return PyInt::Small(s.iter().fold(0i64, |acc, &b| (acc << 8) + i64::from(b)));
    }
    PyInt::from(num_bigint::BigInt::from(BigUint::from_bytes_be(s)))
}

/// The size of a unicode map that [`check_growth`] calls runaway: far more
/// than any real or tested map (the predefined ones hold about 20 000
/// entries).
const MAX_CHECKED_ENTRIES: usize = 1 << 22;

/// With debug assertions (`--profile mutants`), panics when a result outgrows
/// `limit`: a mutant that loops while it allocates then fails at once instead
/// of exhausting the memory (`docs/verification.md`, "Mutations"; like
/// `psparser::ProgressGuard`). Release builds check nothing.
#[inline]
fn check_growth(len: usize, limit: usize) {
    debug_assert!(len <= limit, "runaway growth: {len} > {limit}");
}

/// A UTF-16 surrogate code point (U+D800..U+DFFF).
fn is_surrogate(c: u32) -> bool {
    (0xd800..0xe000).contains(&c)
}

/// `bytes.decode("UTF-16BE", "ignore")`: CPython's UTF-16 decoder
/// (`Objects/stringlib/codecs.h` `utf16_decode`) skipping what it reports:
/// a lone low surrogate, a high surrogate not followed by a low one (only
/// the high surrogate is skipped), and at the end a high surrogate (with
/// any odd byte after it) or an odd byte.
pub fn decode_utf16be_ignore(b: &[u8]) -> String {
    let unit = |i: usize| u32::from(u16::from_be_bytes([b[i], b[i + 1]]));
    let mut out = String::with_capacity(b.len() / 2);
    // UTF-8 bytes of the result: at most 4 per 2 input bytes
    let limit = 2 * b.len();
    let mut i = 0;
    while i + 1 < b.len() {
        let ch = unit(i);
        i += 2;
        if !is_surrogate(ch) {
            out.push(char::from_u32(ch).expect("not a surrogate"));
            check_growth(out.len(), limit);
            continue;
        }
        if ch >= 0xdc00 {
            continue; // illegal encoding: a low surrogate first
        }
        if i + 1 >= b.len() {
            break; // unexpected end of data
        }
        let ch2 = unit(i);
        if !(0xdc00..0xe000).contains(&ch2) {
            continue; // illegal UTF-16 surrogate: skip the high one only
        }
        i += 2;
        out.push(char::from_u32(0x10000 + ((ch - 0xd800) << 10) + (ch2 - 0xdc00)).expect("a supplementary char"));
        check_growth(out.len(), limit);
    }
    out
}

// ---------------------------------------------------------------------------
// CMapBase and its subclasses

/// pdfminer/cmapdb.py:43 `CMapBase`: the interface of every CMap and
/// unicode map. The default methods are the base class's.
pub trait CMapBase {
    /// `self.attrs`
    fn attrs(&self) -> &Attrs;
    fn attrs_mut(&mut self) -> &mut Attrs;
    /// `type(self).__name__`
    fn class_name(&self) -> &'static str;

    // pdfminer/cmapdb.py:49 CMapBase.is_vertical
    /// `self.attrs.get("WMode", 0) != 0`.
    fn is_vertical(&self) -> bool {
        match self.attrs().get("WMode") {
            None => false,
            Some(AttrValue::Str(_)) => true,
            Some(AttrValue::Obj(v)) => *v != PDFObject::Int(PyInt::ZERO),
        }
    }

    // pdfminer/cmapdb.py:52 CMapBase.set_attr
    fn set_attr(&mut self, k: String, v: AttrValue) {
        self.attrs_mut().insert(k, v);
    }

    // pdfminer/cmapdb.py:55 CMapBase.add_code2cid
    fn add_code2cid(&mut self, _code: &str, _cid: PyInt) -> PyResult<()> {
        Ok(())
    }

    // pdfminer/cmapdb.py:58 CMapBase.add_cid2unichr
    fn add_cid2unichr(&mut self, _cid: PyInt, _code: &PDFObject) -> PyResult<()> {
        Ok(())
    }

    // pdfminer/cmapdb.py:61 CMapBase.use_cmap
    fn use_cmap(&mut self, _cmap: &dyn CMapBase) -> PyResult<()> {
        Ok(())
    }

    // pdfminer/cmapdb.py:64 CMapBase.decode
    /// The CIDs of the character codes `code` (the generator, run to its
    /// end).
    fn decode(&self, _code: &[u8]) -> PyResult<Vec<PyInt>> {
        Err(exc::NotImplementedError(""))
    }

    /// `self` when it is a `CMap` (`isinstance(self, CMap)`).
    fn as_cmap(&self) -> Option<&CMap> {
        None
    }

    /// `self` when it is a `UnicodeMap`.
    fn as_unicode_map(&self) -> Option<&UnicodeMap> {
        None
    }
}

/// An instance of `CMapBase` itself.
#[derive(Clone, Debug, Default)]
pub struct PlainCMapBase {
    pub attrs: Attrs,
}

impl PlainCMapBase {
    // pdfminer/cmapdb.py:46 CMapBase.__init__
    pub fn new(attrs: Attrs) -> PlainCMapBase {
        PlainCMapBase { attrs }
    }
}

impl CMapBase for PlainCMapBase {
    fn attrs(&self) -> &Attrs {
        &self.attrs
    }
    fn attrs_mut(&mut self) -> &mut Attrs {
        &mut self.attrs
    }
    fn class_name(&self) -> &'static str {
        "CMapBase"
    }
}

/// The class of a [`CMap`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CMapClass {
    CMap,
    FileCMap,
    PyCMap,
}

/// pdfminer/cmapdb.py:68 `CMap`, :163 `FileCMap` and :202 `PyCMap`.
#[derive(Clone, Debug)]
pub struct CMap {
    pub class: CMapClass,
    pub attrs: Attrs,
    pub code2cid: Code2Cid,
}

impl CMap {
    // pdfminer/cmapdb.py:69 CMap.__init__
    /// `CMap(**attrs)`.
    pub fn new(attrs: Attrs) -> CMap {
        CMap { class: CMapClass::CMap, attrs, code2cid: Code2Cid::default() }
    }

    // pdfminer/cmapdb.py:69 CMap.__init__
    /// `FileCMap(**attrs)`.
    pub fn new_file(attrs: Attrs) -> CMap {
        CMap { class: CMapClass::FileCMap, ..CMap::new(attrs) }
    }

    // pdfminer/cmapdb.py:203 PyCMap.__init__
    /// `PyCMap(name, module)`: the data of a predefined CMap, or the
    /// `AttributeError` of a module that has none.
    pub fn new_py(name: &str, module: &CMapModule) -> PyResult<CMap> {
        let mut attrs = Attrs::default();
        attrs.insert("CMapName".into(), AttrValue::Str(name.into()));
        let code2cid = module.code2cid.clone().ok_or_else(|| module.missing("CODE2CID"))?;
        let is_vertical = module.is_vertical.ok_or_else(|| module.missing("IS_VERTICAL"))?;
        if is_vertical {
            attrs.insert("WMode".into(), AttrValue::from(1));
        }
        Ok(CMap { class: CMapClass::PyCMap, attrs, code2cid })
    }

    // pdfminer/cmapdb.py:73 CMap.__repr__
    pub fn repr(&self) -> PyResult<String> {
        let name = match self.attrs.get("CMapName") {
            Some(v) => v.py_str()?,
            None => "None".into(),
        };
        Ok(format!("<CMap: {name}>"))
    }

    // pdfminer/cmapdb.py:90 CMap.decode
    /// The CIDs of `code`: the codes that reach a CID in `code2cid`; a byte
    /// without an entry restarts at the root.
    pub fn decode_cids(&self, code: &[u8]) -> Vec<PyInt> {
        let mut out = Vec::new();
        let mut d = &self.code2cid;
        for &i in code {
            match d.get(&u32::from(i)) {
                Some(Code2CidValue::Cid(x)) => {
                    out.push(x.clone());
                    d = &self.code2cid;
                }
                Some(Code2CidValue::Map(x)) => d = x,
                None => d = &self.code2cid,
            }
        }
        out
    }

    // pdfminer/cmapdb.py:104 CMap.dump
    /// What `dump(out)` writes: one line per code, keys sorted.
    pub fn dump(&self) -> String {
        fn walk(out: &mut String, code2cid: &Code2Cid, code: &mut Vec<u32>) {
            let mut items: Vec<_> = code2cid.iter().collect();
            items.sort_by_key(|(k, _)| **k);
            for (k, v) in items {
                code.push(*k);
                match v {
                    Code2CidValue::Cid(cid) => {
                        let c = code.iter().map(u32::to_string).collect::<Vec<_>>();
                        let c = if c.len() == 1 { format!("({},)", c[0]) } else { format!("({})", c.join(", ")) };
                        let _ = writeln!(out, "code {c} = cid {cid}");
                    }
                    Code2CidValue::Map(d) => walk(out, d, code),
                }
                code.pop();
            }
        }
        let mut out = String::new();
        walk(&mut out, &self.code2cid, &mut Vec::new());
        out
    }

    // pdfminer/cmapdb.py:164 FileCMap.add_code2cid
    fn file_add_code2cid(&mut self, code: &str, cid: PyInt) -> PyResult<()> {
        let chars: Vec<char> = code.chars().collect();
        let Some((last, prefix)) = chars.split_last() else {
            return Err(exc::IndexError("string index out of range"));
        };
        let mut d = &mut self.code2cid;
        for (k, c) in prefix.iter().enumerate() {
            match d.entry(u32::from(*c)).or_insert_with(|| Code2CidValue::Map(Code2Cid::default())) {
                Code2CidValue::Map(t) => d = t,
                // `d` is now an int: the next `ci in d`, or the final
                // `d[ci] = cid`, fails
                Code2CidValue::Cid(_) if k + 1 < prefix.len() => {
                    return Err(exc::TypeError("argument of type 'int' is not iterable"));
                }
                Code2CidValue::Cid(_) => return Err(exc::TypeError("'int' object does not support item assignment")),
            }
        }
        d.insert(u32::from(*last), Code2CidValue::Cid(cid));
        Ok(())
    }
}

impl CMapBase for CMap {
    fn attrs(&self) -> &Attrs {
        &self.attrs
    }
    fn attrs_mut(&mut self) -> &mut Attrs {
        &mut self.attrs
    }
    fn class_name(&self) -> &'static str {
        match self.class {
            CMapClass::CMap => "CMap",
            CMapClass::FileCMap => "FileCMap",
            CMapClass::PyCMap => "PyCMap",
        }
    }

    fn add_code2cid(&mut self, code: &str, cid: PyInt) -> PyResult<()> {
        match self.class {
            CMapClass::FileCMap => self.file_add_code2cid(code, cid),
            CMapClass::CMap | CMapClass::PyCMap => Ok(()),
        }
    }

    // pdfminer/cmapdb.py:76 CMap.use_cmap
    /// Copies the `code2cid` of `cmap` (a `CMap`) into this one: each of its
    /// top-level entries replaces this one's (nested dicts are copied).
    fn use_cmap(&mut self, cmap: &dyn CMapBase) -> PyResult<()> {
        let Some(cmap) = cmap.as_cmap() else {
            return Err(exc::AssertionError(type_str(cmap.class_name())));
        };
        for (k, v) in &cmap.code2cid {
            self.code2cid.insert(*k, v.clone());
        }
        Ok(())
    }

    fn decode(&self, code: &[u8]) -> PyResult<Vec<PyInt>> {
        Ok(self.decode_cids(code))
    }

    fn as_cmap(&self) -> Option<&CMap> {
        Some(self)
    }
}

/// pdfminer/cmapdb.py:121 `IdentityCMap` and :130 `IdentityCMapByte`
/// (`byte`).
#[derive(Clone, Debug)]
pub struct IdentityCMap {
    pub attrs: Attrs,
    pub byte: bool,
}

impl IdentityCMap {
    // pdfminer/cmapdb.py:46 CMapBase.__init__
    /// `IdentityCMap(**attrs)`.
    pub fn new(attrs: Attrs) -> IdentityCMap {
        IdentityCMap { attrs, byte: false }
    }

    // pdfminer/cmapdb.py:46 CMapBase.__init__
    /// `IdentityCMapByte(**attrs)`.
    pub fn new_byte(attrs: Attrs) -> IdentityCMap {
        IdentityCMap { attrs, byte: true }
    }

    // pdfminer/cmapdb.py:122 IdentityCMap.decode
    // pdfminer/cmapdb.py:131 IdentityCMapByte.decode
    /// The big-endian 16-bit units of `code` (an odd last byte is ignored),
    /// or its bytes for `IdentityCMapByte`.
    pub fn decode_cids(&self, code: &[u8]) -> Vec<PyInt> {
        if self.byte {
            return code.iter().map(|&b| PyInt::Small(i64::from(b))).collect();
        }
        code.as_chunks::<2>().0.iter().map(|u| PyInt::Small(i64::from(u16::from_be_bytes(*u)))).collect()
    }
}

impl CMapBase for IdentityCMap {
    fn attrs(&self) -> &Attrs {
        &self.attrs
    }
    fn attrs_mut(&mut self) -> &mut Attrs {
        &mut self.attrs
    }
    fn class_name(&self) -> &'static str {
        if self.byte {
            "IdentityCMapByte"
        } else {
            "IdentityCMap"
        }
    }

    fn decode(&self, code: &[u8]) -> PyResult<Vec<PyInt>> {
        Ok(self.decode_cids(code))
    }
}

/// The class of a [`UnicodeMap`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnicodeMapClass {
    UnicodeMap,
    IdentityUnicodeMap,
    FileUnicodeMap,
    PyUnicodeMap,
}

/// pdfminer/cmapdb.py:139 `UnicodeMap`, :156 `IdentityUnicodeMap`, :181
/// `FileUnicodeMap` and :210 `PyUnicodeMap`.
#[derive(Clone, Debug)]
pub struct UnicodeMap {
    pub class: UnicodeMapClass,
    pub attrs: Attrs,
    pub cid2unichr: Cid2Unichr,
}

impl UnicodeMap {
    // pdfminer/cmapdb.py:140 UnicodeMap.__init__
    /// `UnicodeMap(**attrs)`.
    pub fn new(attrs: Attrs) -> UnicodeMap {
        UnicodeMap { class: UnicodeMapClass::UnicodeMap, attrs, cid2unichr: Cid2Unichr::default() }
    }

    // pdfminer/cmapdb.py:140 UnicodeMap.__init__
    /// `IdentityUnicodeMap(**attrs)`.
    pub fn new_identity(attrs: Attrs) -> UnicodeMap {
        UnicodeMap { class: UnicodeMapClass::IdentityUnicodeMap, ..UnicodeMap::new(attrs) }
    }

    // pdfminer/cmapdb.py:140 UnicodeMap.__init__
    /// `FileUnicodeMap(**attrs)`.
    pub fn new_file(attrs: Attrs) -> UnicodeMap {
        UnicodeMap { class: UnicodeMapClass::FileUnicodeMap, ..UnicodeMap::new(attrs) }
    }

    // pdfminer/cmapdb.py:211 PyUnicodeMap.__init__
    /// `PyUnicodeMap(name, module, vertical)`, or the `AttributeError` of a
    /// module without the dict.
    pub fn new_py(name: &str, module: &CMapModule, vertical: bool) -> PyResult<UnicodeMap> {
        let mut attrs = Attrs::default();
        attrs.insert("CMapName".into(), AttrValue::Str(name.into()));
        let cid2unichr = if vertical {
            let d = module.cid2unichr_v.clone().ok_or_else(|| module.missing("CID2UNICHR_V"))?;
            attrs.insert("WMode".into(), AttrValue::from(1));
            d
        } else {
            module.cid2unichr_h.clone().ok_or_else(|| module.missing("CID2UNICHR_H"))?
        };
        Ok(UnicodeMap { class: UnicodeMapClass::PyUnicodeMap, attrs, cid2unichr })
    }

    // pdfminer/cmapdb.py:144 UnicodeMap.__repr__
    pub fn repr(&self) -> PyResult<String> {
        let name = match self.attrs.get("CMapName") {
            Some(v) => v.py_str()?,
            None => "None".into(),
        };
        Ok(format!("<UnicodeMap: {name}>"))
    }

    // pdfminer/cmapdb.py:147 UnicodeMap.get_unichr
    // pdfminer/cmapdb.py:157 IdentityUnicodeMap.get_unichr
    /// The text of `cid`: `KeyError` when the map has none; `chr(cid)` for
    /// `IdentityUnicodeMap` (interpret character id as unicode codepoint).
    pub fn get_unichr(&self, cid: &PyInt) -> PyResult<Unichr> {
        if self.class == UnicodeMapClass::IdentityUnicodeMap {
            return Ok(Unichr::from_code_point(py_chr(cid)?));
        }
        match self.cid2unichr.get(cid) {
            Some(u) => Ok(u.clone()),
            None => Err(exc::KeyError(cid.to_string())),
        }
    }

    // pdfminer/cmapdb.py:151 UnicodeMap.dump
    /// What `dump(out)` writes: one line per CID, sorted.
    pub fn dump(&self) -> String {
        let mut items: Vec<_> = self.cid2unichr.iter().collect();
        items.sort_by(|a, b| a.0.cmp(b.0));
        let mut out = String::new();
        for (k, v) in items {
            let _ = writeln!(out, "cid {k} = unicode {}", v.repr());
        }
        out
    }

    // pdfminer/cmapdb.py:182 FileUnicodeMap.add_cid2unichr
    fn file_add_cid2unichr(&mut self, cid: PyInt, code: &PDFObject) -> PyResult<()> {
        let unichr = match code {
            // Interpret as an Adobe glyph name.
            PDFObject::Literal(lit) => match lit.name() {
                LiteralName::Str(name) => Unichr::from(name2unicode(name)?),
                LiteralName::Bytes(_) => return Err(exc::AssertionError("")),
            },
            // Interpret as UTF-16BE.
            PDFObject::Bytes(b) => Unichr::from(decode_utf16be_ignore(b)),
            PDFObject::Int(i) => Unichr::from_code_point(py_chr(i)?),
            PDFObject::Bool(b) => Unichr::from_code_point(u32::from(*b)),
            other => return Err(exc::PDFTypeError(other.py_str()?)),
        };
        // A0 = non-breaking space, some weird fonts can have a collision on a cid here.
        if unichr.as_str() == Some("\u{a0}") && self.cid2unichr.get(&cid).and_then(Unichr::as_str) == Some(" ") {
            return Ok(());
        }
        self.cid2unichr.insert(cid, unichr);
        check_growth(self.cid2unichr.len(), MAX_CHECKED_ENTRIES);
        Ok(())
    }
}

impl CMapBase for UnicodeMap {
    fn attrs(&self) -> &Attrs {
        &self.attrs
    }
    fn attrs_mut(&mut self) -> &mut Attrs {
        &mut self.attrs
    }
    fn class_name(&self) -> &'static str {
        match self.class {
            UnicodeMapClass::UnicodeMap => "UnicodeMap",
            UnicodeMapClass::IdentityUnicodeMap => "IdentityUnicodeMap",
            UnicodeMapClass::FileUnicodeMap => "FileUnicodeMap",
            UnicodeMapClass::PyUnicodeMap => "PyUnicodeMap",
        }
    }

    fn add_cid2unichr(&mut self, cid: PyInt, code: &PDFObject) -> PyResult<()> {
        match self.class {
            UnicodeMapClass::FileUnicodeMap => self.file_add_cid2unichr(cid, code),
            _ => Ok(()),
        }
    }

    fn as_unicode_map(&self) -> Option<&UnicodeMap> {
        Some(self)
    }
}

// ---------------------------------------------------------------------------
// Predefined CMaps

/// The data of a predefined CMap file: the class `CMapDB._load_data`
/// returns, whose attributes are the keys of the JSON file (a CMap has
/// `IS_VERTICAL` and `CODE2CID`, a unicode map `CID2UNICHR_H` and
/// `CID2UNICHR_V`).
#[derive(Clone, Debug, Default)]
pub struct CMapModule {
    /// The class name: the name given to `_load_data`, without NULs.
    pub name: String,
    pub is_vertical: Option<bool>,
    pub code2cid: Option<Code2Cid>,
    pub cid2unichr_h: Option<Cid2Unichr>,
    pub cid2unichr_v: Option<Cid2Unichr>,
}

impl CMapModule {
    /// The `AttributeError` of reading a missing attribute of the class:
    /// `type_getattro` (Objects/typeobject.c) formats it with
    /// `"type object '%.100s' has no attribute '%U'"`. The class name is not
    /// quoted by `repr`; `%.100s` keeps its first 100 UTF-8 bytes and decodes
    /// them with the `replace` handler, so a character cut in the middle
    /// becomes one U+FFFD (as `from_utf8_lossy` does with a truncated
    /// sequence at the end).
    fn missing(&self, attr: &str) -> PyErr {
        let name = self.name.as_bytes();
        let name = String::from_utf8_lossy(&name[..name.len().min(100)]);
        exc::AttributeError(format!("type object '{name}' has no attribute '{attr}'"))
    }

    /// Reads the compact form written by `oracle/codegen/cmaps.py` (see its
    /// documentation for the format).
    pub fn parse(name: &str, data: &[u8]) -> Result<CMapModule, String> {
        let mut r = Reader { data, pos: 0 };
        if r.take(4)? != b"PMCM" || r.byte()? != 1 {
            return Err("not a version 1 compact CMap".into());
        }
        let mut module = CMapModule { name: name.into(), ..CMapModule::default() };
        for _ in 0..r.varint()? {
            let key = r.text()?;
            match (key.as_str(), r.byte()?) {
                ("IS_VERTICAL", b'B') => module.is_vertical = Some(r.byte()? != 0),
                ("CODE2CID", b'C') => module.code2cid = Some(r.node(0)?),
                ("CID2UNICHR_H", b'U') => module.cid2unichr_h = Some(r.umap()?),
                ("CID2UNICHR_V", b'U') => module.cid2unichr_v = Some(r.umap()?),
                (key, tag) => return Err(format!("unexpected attribute {key} (tag {tag})")),
            }
        }
        if r.pos != data.len() {
            return Err("trailing bytes".into());
        }
        Ok(module)
    }
}

/// A cursor over compact CMap data.
struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], String> {
        let end = self.pos.checked_add(n).filter(|&end| end <= self.data.len()).ok_or("truncated data")?;
        let s = &self.data[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    fn byte(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    fn varint(&mut self) -> Result<u64, String> {
        let mut v: u64 = 0;
        for shift in (0..64).step_by(7) {
            let b = self.byte()?;
            v |= u64::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                return Ok(v);
            }
        }
        Err("varint too long".into())
    }

    fn text(&mut self) -> Result<String, String> {
        let n = usize::try_from(self.varint()?).map_err(|_| "length out of range")?;
        String::from_utf8(self.take(n)?.to_vec()).map_err(|_| "invalid UTF-8".to_string())
    }

    fn node(&mut self, depth: usize) -> Result<Code2Cid, String> {
        if depth > 64 {
            return Err("tree too deep".into());
        }
        let mut d = Code2Cid::default();
        for _ in 0..self.varint()? {
            let h = self.varint()?;
            let key = self.varint()?;
            if h == 1 {
                let k = u32::try_from(key).map_err(|_| "key out of range")?;
                let sub = self.node(depth + 1)?;
                d.insert(k, Code2CidValue::Map(sub));
                continue;
            }
            let cid = self.varint()?;
            for j in 0..h / 2 {
                let k = key.checked_add(j).and_then(|k| u32::try_from(k).ok()).ok_or("key out of range")?;
                let v = cid.checked_add(j).ok_or("value out of range")?;
                d.insert(k, Code2CidValue::Cid(PyInt::from(v)));
            }
        }
        Ok(d)
    }

    fn umap(&mut self) -> Result<Cid2Unichr, String> {
        let mut d = Cid2Unichr::default();
        for _ in 0..self.varint()? {
            let h = self.varint()?;
            let cid = self.varint()?;
            if h == 1 {
                let text = self.text()?;
                d.insert(PyInt::from(cid), Unichr::from(text));
                continue;
            }
            let cp = self.varint()?;
            for j in 0..h / 2 {
                let k = cid.checked_add(j).ok_or("value out of range")?;
                let ch = cp
                    .checked_add(j)
                    .and_then(|c| u32::try_from(c).ok())
                    .and_then(char::from_u32)
                    .ok_or("not a character")?;
                d.insert(PyInt::from(k), Unichr::Text(ch.to_string().into()));
            }
        }
        Ok(d)
    }
}

/// The source of the predefined CMaps: pdfminer's `cmap` directory.
pub trait CMapProvider {
    /// The compact form (`oracle/codegen/cmaps.py`) of `<name>.json.gz`,
    /// or `None` when there is no such file. `name` is any string without
    /// `/` (the last component of the path pdfminer builds).
    fn load(&self, name: &str) -> Option<Vec<u8>>;
}

/// A [`CMapProvider`] reading the files `oracle/codegen/cmaps.py` writes:
/// `<dir>/names.txt` (the names, one per line) and `<dir>/<name>.pdfcmap`.
pub struct DirCMapProvider {
    dir: PathBuf,
    names: FxHashSet<String>,
}

impl DirCMapProvider {
    /// Reads `<dir>/names.txt`.
    pub fn new(dir: impl Into<PathBuf>) -> std::io::Result<DirCMapProvider> {
        let dir = dir.into();
        let names = std::fs::read_to_string(dir.join("names.txt"))?.lines().map(str::to_owned).collect();
        Ok(DirCMapProvider { dir, names })
    }
}

impl CMapProvider for DirCMapProvider {
    fn load(&self, name: &str) -> Option<Vec<u8>> {
        if !self.names.contains(name) {
            return None;
        }
        std::fs::read(self.dir.join(format!("{name}.pdfcmap"))).ok()
    }
}

/// Where `os.path.realpath(os.path.join(directory, name + ".json.gz"))`
/// leads, for pdfminer's `cmap` directory: the CMap name (the file stem)
/// when the path is a file directly in the directory, which `_load_data`
/// accepts; `None` otherwise.
///
/// The directory is `<install>/pdfminer/cmap`. The path is normalized like
/// `realpath` (lexically: the directory has no symbolic links): empty and
/// `.` components are skipped, `..` goes up. Going up through
/// `<install>`, or an absolute name, can only come back through the
/// install path, which depends on the machine: such a path is not found.
pub fn resolve_cmap_name(name: &str) -> Option<String> {
    // the path below `<install>`, or None once it went above it
    let mut path: Option<Vec<&str>> = if name.starts_with('/') { None } else { Some(vec!["pdfminer", "cmap"]) };
    let file = format!("{name}.json.gz");
    let mut components: Vec<&str> = file.split('/').collect();
    let last = components.pop().expect("split yields at least one part");
    for c in components {
        match c {
            "" | "." => {}
            ".." => {
                if let Some(p) = &mut path {
                    if p.pop().is_none() {
                        path = None;
                    }
                }
            }
            c => {
                if let Some(p) = &mut path {
                    p.push(c);
                }
            }
        }
    }
    match path.as_deref() {
        Some(["pdfminer", "cmap"]) => Some(last.strip_suffix(".json.gz").expect("the suffix just added").to_owned()),
        _ => None,
    }
}

/// A provider shared by the threads of the process.
pub type SharedCMapProvider = Arc<dyn CMapProvider + Send + Sync>;

/// The process-wide default provider ([`CMapDB::set_default_provider`]).
static DEFAULT_PROVIDER: RwLock<Option<SharedCMapProvider>> = RwLock::new(None);

/// The process-wide default provider, as a provider of this thread.
struct DefaultProvider(SharedCMapProvider);

impl CMapProvider for DefaultProvider {
    fn load(&self, name: &str) -> Option<Vec<u8>> {
        self.0.load(name)
    }
}

thread_local! {
    /// The choice of this thread: `None` (no choice) uses the process
    /// default; `Some(None)` is an installation without the data.
    static PROVIDER: RefCell<Option<Option<Rc<dyn CMapProvider>>>> = const { RefCell::new(None) };
    /// `CMapDB._cmap_cache`
    static CMAP_CACHE: RefCell<FxHashMap<String, Rc<CMap>>> = RefCell::new(FxHashMap::default());
    /// `CMapDB._umap_cache`
    static UMAP_CACHE: RefCell<FxHashMap<String, [Rc<UnicodeMap>; 2]>> = RefCell::new(FxHashMap::default());
}

/// pdfminer/cmapdb.py:220 `CMapDB`: the predefined CMaps (see the module
/// documentation).
pub struct CMapDB;

impl CMapDB {
    /// Sets the provider of the whole process, used by every thread that
    /// has not chosen its own with [`CMapDB::set_provider`] (`None`: no
    /// default). The caches are kept, as Python's are.
    pub fn set_default_provider(provider: Option<SharedCMapProvider>) {
        *DEFAULT_PROVIDER.write().unwrap_or_else(PoisonError::into_inner) = provider;
    }

    /// Sets the provider of this thread, which overrides the process
    /// default (`None`: every CMap is missing, as with an installation
    /// without the data). The caches are kept, as Python's are.
    pub fn set_provider(provider: Option<Rc<dyn CMapProvider>>) {
        PROVIDER.with(|p| *p.borrow_mut() = Some(provider));
    }

    /// Makes this thread use the process default again.
    pub fn clear_provider() {
        PROVIDER.with(|p| *p.borrow_mut() = None);
    }

    /// The provider of this thread, else the process default. With
    /// neither, the core is misconfigured: a `RuntimeError` (no Python
    /// counterpart) rather than `CMapNotFound`, which pdffont would silently
    /// turn into an empty CMap or no unicode map.
    fn provider() -> PyResult<Option<Rc<dyn CMapProvider>>> {
        if let Some(choice) = PROVIDER.with(|p| p.borrow().clone()) {
            return Ok(choice);
        }
        match DEFAULT_PROVIDER.read().unwrap_or_else(PoisonError::into_inner).clone() {
            Some(provider) => Ok(Some(Rc::new(DefaultProvider(provider)))),
            None => Err(PyErr::new(
                ExcKind::RuntimeError,
                "no CMap provider installed: call CMapDB::set_default_provider or CMapDB::set_provider",
            )),
        }
    }

    /// Empties the caches of this thread (`CMapDB._cmap_cache.clear()`,
    /// `CMapDB._umap_cache.clear()`): the state of a new Python process,
    /// for tests of the first load.
    pub fn clear_caches() {
        CMAP_CACHE.with(|c| c.borrow_mut().clear());
        UMAP_CACHE.with(|c| c.borrow_mut().clear());
    }

    // pdfminer/cmapdb.py:249 CMapDB._load_data
    /// The data of the predefined CMap `name`, or `CMapNotFound(name)`;
    /// with the frames of the standard library calls of a search that fails
    /// or a file that loads (see the module documentation).
    pub fn load_data(name: &str) -> PyResult<CMapModule> {
        let _frame = recursion::py_frame()?;
        let name = name.replace('\0', "");
        // log.debug("loading: %r", name)
        LOG.debug()?;
        let provider = CMapDB::provider()?;
        if let (Some(provider), Some(stem)) = (provider, resolve_cmap_name(&name)) {
            if let Some(data) = provider.load(&stem) {
                // log.debug("loading JSON: %r", json_path), then the
                // deepest frames: json.load reading the gzip header
                LOG.debug()?;
                drop(recursion::py_frames(LOAD_FRAMES)?);
                return CMapModule::parse(&name, &data)
                    .map_err(|e| exc::ValueError(format!("corrupt compact CMap {stem:?}: {e}")));
            }
        }
        // the deepest frames of the search: os.path.realpath
        drop(recursion::py_frames(SEARCH_FRAMES)?);
        Err(PyErr::new(ExcKind::CMapDB_CMapNotFound, name))
    }

    // pdfminer/cmapdb.py:288 CMapDB.get_cmap
    /// The CMap `name`: an identity CMap (a new one each time), or the
    /// predefined CMap, loaded once and cached.
    pub fn get_cmap(name: &str) -> PyResult<Rc<dyn CMapBase>> {
        let _frame = recursion::py_frame()?;
        let wmode = |w: i64| Attrs::from_iter([("WMode".to_owned(), AttrValue::from(w))]);
        let identity = |cmap: IdentityCMap| -> PyResult<Rc<dyn CMapBase>> {
            // IdentityCMap(WMode=...): CMapBase.__init__, run from C
            let _entry = recursion::eval_entry()?;
            drop(recursion::py_frame()?);
            Ok(Rc::new(cmap))
        };
        match name {
            "Identity-H" => return identity(IdentityCMap::new(wmode(0))),
            "Identity-V" => return identity(IdentityCMap::new(wmode(1))),
            "OneByteIdentityH" => return identity(IdentityCMap::new_byte(wmode(0))),
            "OneByteIdentityV" => return identity(IdentityCMap::new_byte(wmode(1))),
            _ => {}
        }
        if let Some(cmap) = CMAP_CACHE.with(|c| c.borrow().get(name).cloned()) {
            return Ok(cmap);
        }
        let data = CMapDB::load_data(name)?;
        // (PyCMap(name, data) and the __init__ it chains to are shallower
        // than load_data's frames)
        let cmap = Rc::new(CMap::new_py(name, &data)?);
        CMAP_CACHE.with(|c| c.borrow_mut().insert(name.to_owned(), cmap.clone()));
        Ok(cmap)
    }

    // pdfminer/cmapdb.py:306 CMapDB.get_unicode_map
    /// The predefined unicode map of the character collection `name`
    /// (`Adobe-Japan1`...), horizontal or vertical; both are loaded once and
    /// cached.
    pub fn get_unicode_map(name: &str, vertical: bool) -> PyResult<Rc<UnicodeMap>> {
        let _frame = recursion::py_frame()?;
        if let Some(umap) = UMAP_CACHE.with(|c| c.borrow().get(name).map(|maps| maps[usize::from(vertical)].clone())) {
            return Ok(umap);
        }
        let data = CMapDB::load_data(&format!("to-unicode-{name}"))?;
        let maps = [Rc::new(UnicodeMap::new_py(name, &data, false)?), Rc::new(UnicodeMap::new_py(name, &data, true)?)];
        let umap = maps[usize::from(vertical)].clone();
        UMAP_CACHE.with(|c| c.borrow_mut().insert(name.to_owned(), maps));
        Ok(umap)
    }
}

// ---------------------------------------------------------------------------
// CMapParser

// pdfminer/cmapdb.py:328-343 CMapParser.KEYWORD_BEGINCMAP, CMapParser.KEYWORD_ENDCMAP,
// CMapParser.KEYWORD_USECMAP, CMapParser.KEYWORD_DEF, CMapParser.KEYWORD_BEGINCODESPACERANGE,
// CMapParser.KEYWORD_ENDCODESPACERANGE, CMapParser.KEYWORD_BEGINCIDRANGE, CMapParser.KEYWORD_ENDCIDRANGE,
// CMapParser.KEYWORD_BEGINCIDCHAR, CMapParser.KEYWORD_ENDCIDCHAR, CMapParser.KEYWORD_BEGINBFRANGE,
// CMapParser.KEYWORD_ENDBFRANGE, CMapParser.KEYWORD_BEGINBFCHAR, CMapParser.KEYWORD_ENDBFCHAR,
// CMapParser.KEYWORD_BEGINNOTDEFRANGE, CMapParser.KEYWORD_ENDNOTDEFRANGE
/// The keywords `CMapParser.do_keyword` handles, in this order.
static KEYWORDS: LazyLock<[PSKeyword; 16]> = LazyLock::new(|| {
    [
        KWD(b"begincmap"),
        KWD(b"endcmap"),
        KWD(b"usecmap"),
        KWD(b"def"),
        KWD(b"begincodespacerange"),
        KWD(b"endcodespacerange"),
        KWD(b"begincidrange"),
        KWD(b"endcidrange"),
        KWD(b"begincidchar"),
        KWD(b"endcidchar"),
        KWD(b"beginbfrange"),
        KWD(b"endbfrange"),
        KWD(b"beginbfchar"),
        KWD(b"endbfchar"),
        KWD(b"beginnotdefrange"),
        KWD(b"endnotdefrange"),
    ]
});

/// The state `CMapParser` adds to `PSStackParser`.
pub struct CMapParserExt<'a> {
    /// `self.cmap`: the CMap or unicode map being filled.
    pub cmap: &'a mut dyn CMapBase,
    /// `self._in_cmap`: some ToUnicode maps don't have "begincmap" keyword.
    pub in_cmap: bool,
    /// `self._warnings`: the messages warned about, once each, in order (a
    /// set in Python).
    pub warnings: Vec<String>,
    keywords: [PSKeyword; 16],
}

/// pdfminer/cmapdb.py:316 `CMapParser`.
pub type CMapParser<'a> = PSStackParser<CMapParserExt<'a>>;

/// What `_warn_once` writes (`log.warning`) before the message.
pub const WARNING_PREFIX: &str = "Ignoring (part of) ToUnicode map because the PDF data does not conform to the \
                                  format. This could result in (cid) values in the output. ";

impl<'a> PSStackParser<CMapParserExt<'a>> {
    // pdfminer/cmapdb.py:317 CMapParser.__init__
    /// `CMapParser(cmap, fp)`: takes the frame of `__init__` (a caller
    /// instantiating the class from Python code takes the evaluation loop
    /// entry of `type.__call__`).
    pub fn new(cmap: &'a mut dyn CMapBase, fp: BytesIO) -> PyResult<CMapParser<'a>> {
        let _frame = recursion::py_frame()?;
        PSStackParser::init(Some(fp), CMapParserExt { cmap, in_cmap: true, warnings: Vec::new(), keywords: *KEYWORDS })
    }

    // pdfminer/cmapdb.py:324 CMapParser.run
    /// Parses the whole stream into the map (until `PSEOF`, the only error
    /// it ignores).
    pub fn run(&mut self) -> PyResult<()> {
        let _frame = recursion::py_frame()?;
        match self.nextobject() {
            Err(e) if e.is_instance(ExcKind::PSEOF) => Ok(()),
            Err(e) => Err(e),
            Ok(_) => Ok(()),
        }
    }

    // pdfminer/cmapdb.py:498 CMapParser._warn_once
    /// Warn once for each unique message (`log.warning`).
    fn warn_once(&mut self, msg: &str) -> PyResult<()> {
        let _frame = recursion::py_frame()?;
        if !self.ext.warnings.iter().any(|w| w == msg) {
            self.ext.warnings.push(msg.to_owned());
            LOG.warning()?;
        }
        Ok(())
    }

    /// The objects of the stack, emptied (`[obj for (__, obj) in self.popall()]`).
    fn popall_objects(&mut self) -> PyResult<Vec<PDFObject>> {
        Ok(self.popall()?.into_iter().map(|(_, obj)| obj).collect())
    }

    /// The `KEYWORD_ENDCIDRANGE` branch of `do_keyword`.
    fn end_cid_range(&mut self) -> PyResult<()> {
        let objs = self.popall_objects()?;
        for triple in choplist(3, &objs) {
            let PDFObject::Bytes(start_byte) = &triple[0] else {
                self.warn_once("The start object of begincidrange is not a byte.")?;
                continue;
            };
            let PDFObject::Bytes(end_byte) = &triple[1] else {
                self.warn_once("The end object of begincidrange is not a byte.")?;
                continue;
            };
            let cid = match &triple[2] {
                PDFObject::Int(i) => i.clone(),
                PDFObject::Bool(b) => PyInt::Small(i64::from(*b)),
                _ => {
                    self.warn_once("The cid object of begincidrange is not a byte.")?;
                    continue;
                }
            };
            if start_byte.len() != end_byte.len() {
                self.warn_once("The start and end byte of begincidrange have different lengths.")?;
                continue;
            }
            let split = start_byte.len().saturating_sub(4);
            let (start_prefix, svar) = start_byte.split_at(split);
            let (end_prefix, evar) = end_byte.split_at(split);
            if start_prefix != end_prefix {
                self.warn_once("The prefix of the start and end byte of begincidrange are not the same.")?;
                continue;
            }
            let start = nunpack(svar).as_i64().expect("at most 4 bytes");
            let end = nunpack(evar).as_i64().expect("at most 4 bytes");
            let vlen = svar.len();
            for i in 0..(end - start + 1).max(0) {
                let x = pack_suffix(start_prefix, start + i, vlen);
                self.ext.cmap.add_cid2unichr(cid.add(&PyInt::Small(i)), &PDFObject::Bytes(x.into()))?;
            }
        }
        Ok(())
    }

    /// The `KEYWORD_ENDBFRANGE` branch of `do_keyword`.
    fn end_bf_range(&mut self) -> PyResult<()> {
        let objs = self.popall_objects()?;
        for triple in choplist(3, &objs) {
            let PDFObject::Bytes(start_byte) = &triple[0] else {
                self.warn_once("The start object is not a byte.")?;
                continue;
            };
            let PDFObject::Bytes(end_byte) = &triple[1] else {
                self.warn_once("The end object is not a byte.")?;
                continue;
            };
            if start_byte.len() != end_byte.len() {
                self.warn_once("The start and end byte have different lengths.")?;
                continue;
            }
            let start = nunpack(start_byte);
            let end = nunpack(end_byte);
            // range(end - start + 1): its length (an int of any size)
            let count = end.sub(&start).add(&PyInt::Small(1));
            match &triple[2] {
                PDFObject::List(code) => {
                    let code = code.to_vec();
                    if PyInt::from(code.len()) != count {
                        self.warn_once(
                            "The difference between the start and end offsets does not match the code length.",
                        )?;
                    }
                    // zip(range(start, end + 1), code)
                    let mut cid = start;
                    for unicode_value in &code {
                        if cid > end {
                            break;
                        }
                        self.ext.cmap.add_cid2unichr(cid.clone(), unicode_value)?;
                        cid = cid.add(&PyInt::Small(1));
                    }
                }
                PDFObject::Bytes(code) => {
                    let split = code.len().saturating_sub(4);
                    let (prefix, var) = code.split_at(split);
                    let base = nunpack(var).as_i64().expect("at most 4 bytes");
                    let vlen = var.len();
                    let mut i: i64 = 0;
                    while PyInt::Small(i) < count {
                        let value = base + i;
                        if value > i64::from(u32::MAX) {
                            return Err(exc::StructError("'L' format requires 0 <= number <= 4294967295"));
                        }
                        let x = pack_suffix(prefix, value, vlen);
                        self.ext.cmap.add_cid2unichr(start.add(&PyInt::Small(i)), &PDFObject::Bytes(x.into()))?;
                        i += 1;
                    }
                }
                _ => return Err(exc::AssertionError("")),
            }
        }
        Ok(())
    }
}

/// `prefix + struct.pack(">L", value)[-vlen:]` for `0 <= value < 2**32`
/// (`[-0:]` is the whole packed value).
fn pack_suffix(prefix: &[u8], value: i64, vlen: usize) -> Vec<u8> {
    let packed = u32::try_from(value).expect("checked by the caller").to_be_bytes();
    let tail = if vlen == 0 { &packed[..] } else { &packed[4 - vlen..] };
    [prefix, tail].concat()
}

impl PSStackParserExt for CMapParserExt<'_> {
    // pdfminer/cmapdb.py:345 CMapParser.do_keyword
    /// ToUnicode CMaps: see Section 5.9.2 - ToUnicode CMaps of the PDF
    /// Reference.
    fn do_keyword(p: &mut PSStackParser<Self>, pos: Pos, token: PSKeyword) -> PyResult<()> {
        let _frame = recursion::py_frame()?;
        let [begincmap, endcmap, usecmap, def, begincodespacerange, endcodespacerange, begincidrange, endcidrange, begincidchar, endcidchar, beginbfrange, endbfrange, beginbfchar, endbfchar, beginnotdefrange, endnotdefrange] =
            p.ext.keywords;
        if token == begincmap {
            p.ext.in_cmap = true;
            p.popall()?;
            return Ok(());
        } else if token == endcmap {
            p.ext.in_cmap = false;
            return Ok(());
        }

        if !p.ext.in_cmap {
            return Ok(());
        }

        if token == def {
            // (the `except PSSyntaxError` of upstream catches nothing: with
            // STRICT off, literal_name never raises it)
            let entries = p.pop(2)?;
            let [(_, k), (_, v)] = <[_; 2]>::try_from(entries).map_err(|entries: Vec<_>| {
                exc::ValueError(format!("not enough values to unpack (expected 2, got {})", entries.len()))
            })?;
            p.ext.cmap.set_attr(literal_name(&k)?, AttrValue::Obj(v));
            return Ok(());
        }

        if token == usecmap {
            // (as for `def`, the `except PSSyntaxError` catches nothing)
            let Some((_, cmapname)) = p.pop(1)?.pop() else {
                return Err(exc::ValueError("not enough values to unpack (expected 1, got 0)"));
            };
            match CMapDB::get_cmap(&literal_name(&cmapname)?) {
                Ok(cmap) => p.ext.cmap.use_cmap(&*cmap)?,
                Err(e) if e.is_instance(ExcKind::CMapDB_CMapNotFound) => {}
                Err(e) => return Err(e),
            }
            return Ok(());
        }

        if [begincodespacerange, endcodespacerange, begincidrange].contains(&token) {
            p.popall()?;
            return Ok(());
        }

        if token == endcidrange {
            return p.end_cid_range();
        }

        if token == begincidchar {
            p.popall()?;
            return Ok(());
        }

        if token == endcidchar {
            let objs = p.popall_objects()?;
            for pair in choplist(2, &objs) {
                let cid = match &pair[0] {
                    PDFObject::Int(i) => i.clone(),
                    PDFObject::Bool(b) => PyInt::Small(i64::from(*b)),
                    _ => continue,
                };
                if let PDFObject::Bytes(_) = &pair[1] {
                    p.ext.cmap.add_cid2unichr(cid, &pair[1])?;
                }
            }
            return Ok(());
        }

        if token == beginbfrange {
            p.popall()?;
            return Ok(());
        }

        if token == endbfrange {
            return p.end_bf_range();
        }

        if token == beginbfchar {
            p.popall()?;
            return Ok(());
        }

        if token == endbfchar {
            let objs = p.popall_objects()?;
            for pair in choplist(2, &objs) {
                if let (PDFObject::Bytes(cid), PDFObject::Bytes(_)) = (&pair[0], &pair[1]) {
                    p.ext.cmap.add_cid2unichr(nunpack(cid), &pair[1])?;
                }
            }
            return Ok(());
        }

        if token == beginnotdefrange || token == endnotdefrange {
            p.popall()?;
            return Ok(());
        }

        p.push((pos, PDFObject::Keyword(token)))?;
        Ok(())
    }
}
