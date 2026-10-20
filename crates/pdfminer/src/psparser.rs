//! Port of `pdfminer/psparser.py`: PostScript literals and keywords, and the
//! tokenizer/object parser every pdfminer parser derives from.
//!
//! # Interning (`PSLiteralTable`, `PSKeywordTable`, `LIT`, `KWD`)
//!
//! Python interns literals and keywords so that `is` compares them.
//! [`PSLiteral`] and [`PSKeyword`] are `Copy` handles to interned, never
//! freed names (Python's tables are never emptied either); `==` compares the
//! handles, which is Python's identity. A literal name is a `str` or, when
//! the bytes were not valid UTF-8, `bytes`, and `LIT("F")` and
//! `LIT(b"F")` are different literals, as in Python. Module-level constants
//! such as `KEYWORD_ARRAY_BEGIN` are `LazyLock` statics interned on first
//! use: compare with `token == *KEYWORD_ARRAY_BEGIN`. (The parsers copy the
//! ones they test for every token into their own state, which avoids the
//! `LazyLock` check in the hot loop.)
//!
//! # Parsers and their Python subclasses
//!
//! `PSBaseParser` and `PSStackParser` are one generic struct,
//! [`PSStackParser<X>`]. It holds the state of both Python classes (`fp`,
//! `buf`, `bufpos`, `charpos`, `eof`, the tokenizer state, `context`,
//! `curtype`, `curstack`, `results`) as public fields, and `ext: X`, the
//! state a Python subclass adds (`PDFContentParser.streams`,
//! `PDFParser.doc`, `CMapParser.cmap`, ...).
//!
//! `X` implements [`PSStackParserExt`], whose associated functions are the
//! methods Python subclasses override; each receives the whole parser:
//!
//! | Python method | hook | default (Python base implementation) |
//! |---|---|---|
//! | `seek(pos)` | [`PSStackParserExt::seek`] | [`PSStackParser::psstackparser_seek`] |
//! | `fillbuf()` | [`PSStackParserExt::fillbuf`] | [`PSStackParser::psbaseparser_fillbuf`] |
//! | `do_keyword(pos, token)` | [`PSStackParserExt::do_keyword`] | nothing |
//! | `flush()` | [`PSStackParserExt::flush`] | nothing |
//! | (none) | [`PSStackParserExt::read_position`] | offset in `fp` |
//!
//! `read_position` is not a Python method: it feeds [`ProgressGuard`], a
//! debug-assertion check that the reading loops move forward between two
//! seeks. A subclass that reads several files in turn (`PDFContentParser`)
//! returns `(file ordinal, offset)`; a subclass reading one `fp` keeps the
//! default. A subclass method that loops over `fillbuf` calls
//! [`PSStackParser::check_progress`] after each step, as
//! `PDFContentParser::get_inline_data` does.
//!
//! The parser's own methods (`nexttoken`, `nextline`, `nextobject`, ...)
//! call the hooks wherever Python calls `self.seek()`, `self.fillbuf()`,
//! `self.do_keyword()` or `self.flush()`, so a subclass changes behavior
//! exactly where Python's dynamic dispatch would. An override that calls the
//! base class (`PSStackParser.seek(self, pos)`) calls the `psbaseparser_*` or
//! `psstackparser_*` method of the same name. Methods a subclass adds are an
//! `impl PSStackParser<TheirExt>` block in their module, and the Python class
//! name is a type alias:
//!
//! ```ignore
//! pub struct ContentStreams { pub streams: Vec<PDFObject>, pub istream: usize }
//! impl PSStackParserExt for ContentStreams {
//!     fn seek(p: &mut PSStackParser<Self>, pos: Pos) -> PyResult<()> {
//!         p.fillfp()?;                     // PDFContentParser.seek
//!         p.psstackparser_seek(pos)        // PSStackParser.seek(self, pos)
//!     }
//!     fn do_keyword(p: &mut PSStackParser<Self>, pos: Pos, token: PSKeyword) -> PyResult<()> { ... }
//! }
//! impl PSStackParser<ContentStreams> { pub fn fillfp(&mut self) -> PyResult<bool> { ... } }
//! pub type PDFContentParser = PSStackParser<ContentStreams>;
//! ```
//!
//! `pdfinterp::PDFContentParser` is the reference implementation of the
//! pattern. `pdfparser::PDFParser`/`PDFStreamParser`, `cmapdb::CMapParser`
//! and `pdffont::Type1FontHeaderParser` follow it.
//!
//! Stack entries are `(pos, obj)` pairs ([`PSStackEntry`]) whose object is
//! a [`PDFObject`] (the union of what any subclass pushes: Python's
//! `ExtraT`). `Type1FontHeaderParser.do_keyword` is the one place where
//! upstream puts something else in `results` (a `(cid, name)` tuple); its
//! port encodes that pair as a `PDFObject` and decodes it in
//! `get_encoding`.
//!
//! The file object `fp` is a [`BytesIO`] over shared bytes; `buf` is a
//! window on the same bytes ([`Buf`]), so reading a chunk does not copy.
//! Reads are done in `BUFSIZ` (4096-byte) blocks exactly like Python, since
//! some results depend on where a block ends.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::ops::Deref;
use std::rc::Rc;
use std::sync::{LazyLock, Mutex, OnceLock};

use pycompat::float::float_from_bytes;
use pycompat::io::{BytesIO, Pos};
use pycompat::logging::Logger;
use pycompat::recursion::{c_called_leaf, leaf, py_frame};
use pycompat::repr::{bytes_repr, str_repr};
use pycompat::{exc, ExcKind, PyInt, PyResult};
use rustc_hash::FxHashMap;

use crate::pdftypes::{PDFDict, PDFList, PDFObject};
use crate::settings;
use crate::utils::choplist;

pub use crate::psexceptions::{PSException, PSSyntaxError, PSTypeError, PSValueError, PSEOF};

// ---------------------------------------------------------------------------
// PSLiteral, PSKeyword and their intern tables

/// The name of a [`PSLiteral`]: a `str`, or `bytes` when the name was not
/// valid UTF-8 (`PSLiteral.NameType = Union[str, bytes]`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LiteralName {
    Str(&'static str),
    Bytes(&'static [u8]),
}

/// pdfminer/psparser.py:33 `PSLiteral`: an interned PostScript name
/// (`/Name`). Equality is identity, like `is` on interned Python objects.
#[derive(Clone, Copy)]
pub struct PSLiteral(&'static LiteralName);

/// pdfminer/psparser.py:55 `PSKeyword`: an interned PostScript keyword
/// (operator). Equality is identity.
#[derive(Clone, Copy)]
pub struct PSKeyword(&'static [u8]);

impl PartialEq for PSLiteral {
    fn eq(&self, other: &PSLiteral) -> bool {
        std::ptr::eq(self.0, other.0)
    }
}
impl Eq for PSLiteral {}
impl Hash for PSLiteral {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::ptr::hash(self.0, state)
    }
}

impl PartialEq for PSKeyword {
    fn eq(&self, other: &PSKeyword) -> bool {
        std::ptr::eq(self.0.as_ptr(), other.0.as_ptr())
    }
}
impl Eq for PSKeyword {}
impl Hash for PSKeyword {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::ptr::hash(self.0.as_ptr(), state)
    }
}

impl PSLiteral {
    /// `self.name`
    pub fn name(self) -> LiteralName {
        *self.0
    }

    // pdfminer/psparser.py:50 PSLiteral.__repr__
    pub fn repr(self) -> String {
        match self.name() {
            LiteralName::Str(s) => format!("/{}", str_repr(s)),
            LiteralName::Bytes(b) => format!("/{}", bytes_repr(b)),
        }
    }
}

impl PSKeyword {
    /// `self.name`
    pub fn name(self) -> &'static [u8] {
        self.0
    }

    // pdfminer/psparser.py:69 PSKeyword.__repr__
    pub fn repr(self) -> String {
        format!("/{}", bytes_repr(self.0))
    }
}

impl fmt::Debug for PSLiteral {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.repr())
    }
}

impl fmt::Debug for PSKeyword {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.repr())
    }
}

/// pdfminer/psparser.py:77 `PSSymbolTable`, for both kinds of symbols.
#[derive(Default)]
struct SymbolTables {
    literals_str: FxHashMap<&'static str, PSLiteral>,
    literals_bytes: FxHashMap<&'static [u8], PSLiteral>,
    keywords: FxHashMap<&'static [u8], PSKeyword>,
}

impl SymbolTables {
    fn literal_str(&mut self, name: &str) -> PSLiteral {
        if let Some(&lit) = self.literals_str.get(name) {
            return lit;
        }
        let name: &'static str = Box::leak(name.into());
        let lit = PSLiteral(Box::leak(Box::new(LiteralName::Str(name))));
        self.literals_str.insert(name, lit);
        lit
    }

    fn literal_bytes(&mut self, name: &[u8]) -> PSLiteral {
        if let Some(&lit) = self.literals_bytes.get(name) {
            return lit;
        }
        let name: &'static [u8] = Box::leak(name.into());
        let lit = PSLiteral(Box::leak(Box::new(LiteralName::Bytes(name))));
        self.literals_bytes.insert(name, lit);
        lit
    }

    fn keyword(&mut self, name: &[u8]) -> PSKeyword {
        if let Some(&kwd) = self.keywords.get(name) {
            return kwd;
        }
        // Each name is its own allocation, so identity compares addresses
        // (the single empty keyword, if any, has a dangling but unique one).
        let name: &'static [u8] = Box::leak(name.into());
        let kwd = PSKeyword(name);
        self.keywords.insert(name, kwd);
        kwd
    }
}

/// The process-wide tables, so that identity holds across threads.
fn global_tables() -> &'static Mutex<SymbolTables> {
    static TABLES: OnceLock<Mutex<SymbolTables>> = OnceLock::new();
    TABLES.get_or_init(Default::default)
}

fn global<T>(f: impl FnOnce(&mut SymbolTables) -> T) -> T {
    f(&mut global_tables().lock().unwrap_or_else(|e| e.into_inner()))
}

/// A thread's lock-free cache of the symbols it has interned. Names of at
/// most 7 bytes (nearly every operator and resource name) are looked up by
/// a packed integer key, which is much cheaper to hash than a slice.
struct LocalTables {
    short_keywords: FxHashMap<u64, PSKeyword>,
    short_literals: FxHashMap<u64, PSLiteral>,
    tables: SymbolTables,
}

thread_local! {
    static LOCAL_TABLES: RefCell<LocalTables> = const {
        RefCell::new(LocalTables {
            short_keywords: FxHashMap::with_hasher(rustc_hash::FxBuildHasher),
            short_literals: FxHashMap::with_hasher(rustc_hash::FxBuildHasher),
            tables: SymbolTables {
                literals_str: FxHashMap::with_hasher(rustc_hash::FxBuildHasher),
                literals_bytes: FxHashMap::with_hasher(rustc_hash::FxBuildHasher),
                keywords: FxHashMap::with_hasher(rustc_hash::FxBuildHasher),
            },
        })
    };
}

/// The name bytes and length packed in a `u64`, for names of 0 to 7 bytes.
#[inline]
fn short_key(name: &[u8]) -> Option<u64> {
    if name.len() > 7 {
        return None;
    }
    let mut key = (name.len() as u64) << 56;
    for (i, &c) in name.iter().enumerate() {
        key |= u64::from(c) << (8 * i);
    }
    Some(key)
}

/// `PSLiteralTable.intern(name)` for a `str` name: `LIT("Name")`.
#[allow(non_snake_case)]
pub fn LIT(name: &str) -> PSLiteral {
    LOCAL_TABLES.with(|local| {
        let short = short_key(name.as_bytes());
        {
            let local = local.borrow();
            let cached = match short {
                Some(key) => local.short_literals.get(&key),
                None => local.tables.literals_str.get(name),
            };
            if let Some(&lit) = cached {
                return lit;
            }
        }
        let lit = global(|g| g.literal_str(name));
        let LiteralName::Str(key) = lit.name() else { unreachable!() };
        let mut local = local.borrow_mut();
        match short {
            Some(short) => local.short_literals.insert(short, lit),
            None => local.tables.literals_str.insert(key, lit),
        };
        lit
    })
}

/// `PSLiteralTable.intern(name)` for a `bytes` name: `LIT(b"Name")`.
#[allow(non_snake_case)]
pub fn LIT_bytes(name: &[u8]) -> PSLiteral {
    LOCAL_TABLES.with(|local| {
        if let Some(&lit) = local.borrow().tables.literals_bytes.get(name) {
            return lit;
        }
        let lit = global(|g| g.literal_bytes(name));
        let LiteralName::Bytes(key) = lit.name() else { unreachable!() };
        local.borrow_mut().tables.literals_bytes.insert(key, lit);
        lit
    })
}

/// `PSKeywordTable.intern(name)`: `KWD(b"name")`.
#[allow(non_snake_case)]
pub fn KWD(name: &[u8]) -> PSKeyword {
    LOCAL_TABLES.with(|local| {
        let short = short_key(name);
        {
            let local = local.borrow();
            let cached = match short {
                Some(key) => local.short_keywords.get(&key),
                None => local.tables.keywords.get(name),
            };
            if let Some(&kwd) = cached {
                return kwd;
            }
        }
        let kwd = global(|g| g.keyword(name));
        let mut local = local.borrow_mut();
        match short {
            Some(short) => local.short_keywords.insert(short, kwd),
            None => local.tables.keywords.insert(kwd.name(), kwd),
        };
        kwd
    })
}

// ---------------------------------------------------------------------------
// The frames of interning

/// Names of symbols, as a Python process has them interned (generated
/// tables: `psparser_symbols.rs`, and the stage harness's).
#[derive(Clone, Copy, Debug)]
pub struct Symbols {
    /// `str` names of literals
    pub literals: &'static [&'static str],
    /// `bytes` names of literals (not valid UTF-8)
    pub bytes_literals: &'static [&'static [u8]],
    /// names of keywords
    pub keywords: &'static [&'static [u8]],
}

impl Symbols {
    /// The interned symbols.
    pub fn symbols(&self) -> impl Iterator<Item = Symbol> + '_ {
        let literals = self.literals.iter().map(|&s| Symbol::Literal(LIT(s)));
        let bytes_literals = self.bytes_literals.iter().map(|&b| Symbol::Literal(LIT_bytes(b)));
        let keywords = self.keywords.iter().map(|&k| Symbol::Keyword(KWD(k)));
        literals.chain(bytes_literals).chain(keywords)
    }
}

thread_local! {
    /// The symbols the Python process has interned so far (addresses of
    /// their names): see [`py_intern`]. A thread starts as the process of
    /// a pdfplumber user, after `import pdfplumber`.
    static PY_INTERNED: RefCell<rustc_hash::FxHashSet<usize>> = RefCell::new(
        crate::psparser_symbols::IMPORT_PDFPLUMBER.symbols().map(Symbol::address).collect(),
    );
}

/// A symbol as [`py_intern`] identifies it.
#[derive(Clone, Copy, Debug)]
pub enum Symbol {
    Literal(PSLiteral),
    Keyword(PSKeyword),
}

impl Symbol {
    /// The address of the interned name: literals and keywords are
    /// distinct leaked allocations, so no two symbols share one.
    fn address(self) -> usize {
        match self {
            Symbol::Literal(lit) => std::ptr::from_ref(lit.0) as usize,
            Symbol::Keyword(kwd) => kwd.0.as_ptr() as usize,
        }
    }
}

/// The symbols the process has interned so far ([`py_intern`]): a
/// snapshot to put back with [`py_symbols_restore`].
#[derive(Clone, Debug)]
pub struct PyInterned(rustc_hash::FxHashSet<usize>);

/// A snapshot of the interned symbols.
pub fn py_symbols_snapshot() -> PyInterned {
    PY_INTERNED.with(|s| PyInterned(s.borrow().clone()))
}

/// Puts back a [`py_symbols_snapshot`].
pub fn py_symbols_restore(interned: PyInterned) {
    PY_INTERNED.with(|s| *s.borrow_mut() = interned.0);
}

/// Starts a new run of the Python process, in which exactly `symbols` are
/// interned (those its imported modules intern at import time): the
/// harness calls it where the oracle starts a process.
pub fn py_symbols_reset(symbols: impl IntoIterator<Item = Symbol>) {
    PY_INTERNED.with(|s| {
        let mut s = s.borrow_mut();
        s.clear();
        s.extend(symbols.into_iter().map(Symbol::address));
    });
}

// pdfminer/psparser.py:87 PSSymbolTable.intern
/// The frames of a call `LIT(name)` or `KWD(name)` that Python runs (not a
/// module constant) and that gave `symbol`: the frame of `intern`, and, the
/// first time the process interns that name, the class instantiation
/// (`self.klass(name)`: `PSLiteral.__init__`/`PSKeyword.__init__` called
/// from C), after which the name is in the table.
pub fn py_intern(symbol: Symbol) -> PyResult<()> {
    let address = symbol.address();
    let _intern = py_frame()?;
    if !PY_INTERNED.with(|s| s.borrow().contains(&address)) {
        c_called_leaf()?;
        PY_INTERNED.with(|s| s.borrow_mut().insert(address));
    }
    Ok(())
}

// pdfminer/psparser.py:102-107 KEYWORD_PROC_BEGIN, KEYWORD_PROC_END, KEYWORD_ARRAY_BEGIN, KEYWORD_ARRAY_END,
// KEYWORD_DICT_BEGIN, KEYWORD_DICT_END
pub static KEYWORD_PROC_BEGIN: LazyLock<PSKeyword> = LazyLock::new(|| KWD(b"{"));
pub static KEYWORD_PROC_END: LazyLock<PSKeyword> = LazyLock::new(|| KWD(b"}"));
pub static KEYWORD_ARRAY_BEGIN: LazyLock<PSKeyword> = LazyLock::new(|| KWD(b"["));
pub static KEYWORD_ARRAY_END: LazyLock<PSKeyword> = LazyLock::new(|| KWD(b"]"));
pub static KEYWORD_DICT_BEGIN: LazyLock<PSKeyword> = LazyLock::new(|| KWD(b"<<"));
pub static KEYWORD_DICT_END: LazyLock<PSKeyword> = LazyLock::new(|| KWD(b">>"));

// pdfminer/psparser.py:110 literal_name
/// `literal_name(x)` with `settings.STRICT = False`: the literal's name as a
/// `str` (a `bytes` name is decoded as UTF-8, or else written as its repr),
/// and `str(x)` for any other object, which raises `RecursionError` for an
/// object nested too deep (see `pdftypes`, "Depth"; one level less deep the
/// first time a process runs that `str(x)`, [`LITERAL_NAME_STR`]). Takes its
/// Python frame.
pub fn literal_name(x: &PDFObject) -> PyResult<String> {
    let _frame = py_frame()?;
    if let PDFObject::Literal(lit) = x {
        return Ok(match lit.name() {
            LiteralName::Str(s) => s.to_owned(),
            LiteralName::Bytes(b) => match std::str::from_utf8(b) {
                Ok(s) => s.to_owned(),
                Err(_) => {
                    // str(x.name): PyObject_Str of the bytes
                    let _unit = pycompat::recursion::c_call(" while getting the str of an object")?;
                    bytes_repr(b)
                }
            },
        });
    }
    if settings::STRICT {
        unreachable!("settings.STRICT is False");
    }
    // `return str(x)`: one C unit more the first time the process runs it
    let _call = LITERAL_NAME_STR.str_call()?;
    x.py_str()
}

// pdfminer/psparser.py:110 literal_name
/// The call site of `return str(x)` in `literal_name` (psparser.py:121),
/// whose first execution in a process takes one C unit more
/// (`pycompat::specialize`): the first non-literal `literal_name` raises
/// `RecursionError` for an object nested one level less deep (a `Tf`, `Do`,
/// `CS`/`cs` operand, a `/ColorSpace` resource, a dict key of the parsers).
pub static LITERAL_NAME_STR: pycompat::specialize::CallSite =
    pycompat::specialize::CallSite::new("psparser.literal_name str(x)");

/// Result of [`keyword_name`]: Python returns either a `str` or, for a
/// non-keyword with `STRICT = False`, the object itself.
#[derive(Clone, Debug)]
pub enum KeywordName {
    Name(String),
    NotKeyword(PDFObject),
}

// pdfminer/psparser.py:124 keyword_name
/// `keyword_name(x)` with `settings.STRICT = False`.
pub fn keyword_name(x: &PDFObject) -> KeywordName {
    match x {
        PDFObject::Keyword(kwd) => KeywordName::Name(pycompat::text::decode_utf8_ignore(kwd.name())),
        _ => KeywordName::NotKeyword(x.clone()),
    }
}

// ---------------------------------------------------------------------------
// Byte classes of the module-level regular expressions (bytes patterns, so
// `\s` is `[\t\n\v\f\r ]`)

const C_SPACE: u8 = 0x01; // SPC
const C_DIGIT: u8 = 0x02; // [0-9], END_NUMBER is its complement
const C_HEX: u8 = 0x04; // HEX
const C_END_LITERAL: u8 = 0x08; // END_LITERAL, END_KEYWORD: [#/%\[\]()<>{}\s]
const C_END_STRING: u8 = 0x10; // END_STRING: [()\\]
const C_EOL: u8 = 0x20; // EOL: [\r\n]
const C_OCT: u8 = 0x40; // OCT_STRING: [0-7]

const fn byte_classes() -> [u8; 256] {
    let mut t = [0u8; 256];
    let mut c = 0;
    while c < 256 {
        let b = c as u8;
        let mut k = 0;
        if matches!(b, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r') {
            k |= C_SPACE | C_END_LITERAL;
        }
        if b.is_ascii_digit() {
            k |= C_DIGIT | C_HEX;
        }
        if matches!(b, b'a'..=b'f' | b'A'..=b'F') {
            k |= C_HEX;
        }
        if matches!(b, b'#' | b'/' | b'%' | b'[' | b']' | b'(' | b')' | b'<' | b'>' | b'{' | b'}') {
            k |= C_END_LITERAL;
        }
        if matches!(b, b'(' | b')' | b'\\') {
            k |= C_END_STRING;
        }
        if matches!(b, b'\r' | b'\n') {
            k |= C_EOL;
        }
        if matches!(b, b'0'..=b'7') {
            k |= C_OCT;
        }
        t[c] = k;
        c += 1;
    }
    t
}

static CLASSES: [u8; 256] = byte_classes();

#[inline]
fn has(c: u8, class: u8) -> bool {
    CLASSES[c as usize] & class != 0
}

/// `PATTERN.search(s, i)`: index of the first byte at or after `i` in
/// (`want = true`) or outside (`want = false`) `class`.
#[inline]
fn search(s: &[u8], i: usize, class: u8, want: bool) -> Option<usize> {
    s[i.min(s.len())..].iter().position(|&c| has(c, class) == want).map(|k| i + k)
}

/// pdfminer/psparser.py:146 ESC_STRING
fn esc_string(c: u8) -> Option<u8> {
    Some(match c {
        b'b' => 8,
        b't' => 9,
        b'n' => 10,
        b'f' => 12,
        b'r' => 13,
        b'(' => 40,
        b')' => 41,
        b'\\' => 92,
        _ => return None,
    })
}

fn hex_digit(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        _ => c - b'A' + 10,
    }
}

// ---------------------------------------------------------------------------
// PSBaseParser tokenizer

/// pdfminer/psparser.py:164 `PSBaseParser.BUFSIZ`
pub const BUFSIZ: usize = 4096;

/// A `(pos, token)` pair from `nexttoken()`; the token is an int, float,
/// bool, bytes, literal or keyword. Positions are file offsets
/// ([`Pos`], Python's `Py_ssize_t`).
pub type PSToken = (Pos, PDFObject);

/// A `(pos, obj)` stack entry (`PSStackEntry`).
pub type PSStackEntry = (Pos, PDFObject);

/// `self._parse1`: the bound method that consumes the next bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Parse1 {
    Main,
    Comment,
    Literal,
    LiteralHex,
    Number,
    Float,
    Keyword,
    String,
    String1,
    WOpen,
    WClose,
    HexString,
}

/// A small direct-mapped cache in front of [`KWD`], private to a lexer:
/// content streams use a few operators over and over, and a hit costs one
/// multiplication and one comparison instead of a thread-local hash lookup.
/// It only speeds up interning; the keywords are those of the tables.
struct KeywordCache {
    slots: Box<[(u64, Option<PSKeyword>); 256]>,
}

impl KeywordCache {
    fn new() -> KeywordCache {
        KeywordCache { slots: Box::new([(0, None); 256]) }
    }

    #[inline]
    fn get(&mut self, name: &[u8]) -> PSKeyword {
        let Some(key) = short_key(name) else {
            return KWD(name);
        };
        let slot = &mut self.slots[(key.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 56) as usize];
        match slot {
            (k, Some(kwd)) if *k == key => *kwd,
            _ => {
                let kwd = KWD(name);
                *slot = (key, Some(kwd));
                kwd
            }
        }
    }
}

impl fmt::Debug for KeywordCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("KeywordCache")
    }
}

/// The tokenizer state of `PSBaseParser` (`_parse1`, `_curtoken`,
/// `_curtokenpos`, `_tokens` and the attributes the `_parse_*` methods
/// create: `paren`, `hex`, `oct`).
#[derive(Debug)]
pub struct Lexer {
    pub parse1: Parse1,
    pub curtoken: Vec<u8>,
    pub curtokenpos: Pos,
    pub tokens: VecDeque<PSToken>,
    pub paren: i64,
    pub hex: Vec<u8>,
    pub oct: Vec<u8>,
    /// `KEYWORD_DICT_BEGIN` and `KEYWORD_DICT_END`.
    dict_keywords: (PSKeyword, PSKeyword),
    keyword_cache: KeywordCache,
}

impl Lexer {
    fn new() -> Lexer {
        Lexer {
            parse1: Parse1::Main,
            curtoken: Vec::new(),
            curtokenpos: 0,
            tokens: VecDeque::new(),
            paren: 0,
            hex: Vec::new(),
            oct: Vec::new(),
            dict_keywords: (*KEYWORD_DICT_BEGIN, *KEYWORD_DICT_END),
            keyword_cache: KeywordCache::new(),
        }
    }

    // pdfminer/psparser.py:302 PSBaseParser._add_token
    /// `self._add_token(obj)`: its frame, then the token.
    fn add_token(&mut self, obj: PDFObject) -> PyResult<()> {
        leaf(1)?;
        self.tokens.push_back((self.curtokenpos, obj));
        Ok(())
    }

    /// `self._parse1(s, i)`; `bufpos` is `self.bufpos`. The method called
    /// takes its frame.
    pub fn parse1(&mut self, s: &[u8], i: usize, bufpos: Pos) -> PyResult<usize> {
        let _frame = py_frame()?;
        match self.parse1 {
            Parse1::Main => self.parse_main(s, i, bufpos),
            Parse1::Comment => Ok(self.parse_comment(s, i)),
            Parse1::Literal => self.parse_literal(s, i),
            Parse1::LiteralHex => Ok(self.parse_literal_hex(s, i)),
            Parse1::Number => self.parse_number(s, i),
            Parse1::Float => self.parse_float(s, i),
            Parse1::Keyword => self.parse_keyword(s, i),
            Parse1::String => self.parse_string(s, i),
            Parse1::String1 => self.parse_string_1(s, i),
            Parse1::WOpen => self.parse_wopen(s, i),
            Parse1::WClose => self.parse_wclose(s, i),
            Parse1::HexString => self.parse_hexstring(s, i),
        }
    }

    // pdfminer/psparser.py:256 PSBaseParser._parse_main
    fn parse_main(&mut self, s: &[u8], i: usize, bufpos: Pos) -> PyResult<usize> {
        let Some(j) = search(s, i, C_SPACE, false) else {
            return Ok(s.len());
        };
        let c = s[j];
        self.curtokenpos = bufpos + j as Pos;
        match c {
            b'%' => {
                self.curtoken.clear();
                self.curtoken.push(b'%');
                self.parse1 = Parse1::Comment;
            }
            b'/' => {
                self.curtoken.clear();
                self.parse1 = Parse1::Literal;
            }
            b'-' | b'+' | b'0'..=b'9' => {
                self.curtoken.clear();
                self.curtoken.push(c);
                self.parse1 = Parse1::Number;
            }
            b'.' => {
                self.curtoken.clear();
                self.curtoken.push(c);
                self.parse1 = Parse1::Float;
            }
            b'a'..=b'z' | b'A'..=b'Z' => {
                self.curtoken.clear();
                self.curtoken.push(c);
                self.parse1 = Parse1::Keyword;
            }
            b'(' => {
                self.curtoken.clear();
                self.paren = 1;
                self.parse1 = Parse1::String;
            }
            b'<' => {
                self.curtoken.clear();
                self.parse1 = Parse1::WOpen;
            }
            b'>' => {
                self.curtoken.clear();
                self.parse1 = Parse1::WClose;
            }
            b'\x00' => {}
            _ => {
                // self._add_token(KWD(c))
                let kwd = self.keyword_cache.get(&s[j..j + 1]);
                py_intern(Symbol::Keyword(kwd))?;
                self.add_token(PDFObject::Keyword(kwd))?;
            }
        }
        Ok(j + 1)
    }

    // pdfminer/psparser.py:305 PSBaseParser._parse_comment
    fn parse_comment(&mut self, s: &[u8], i: usize) -> usize {
        let Some(j) = search(s, i, C_EOL, true) else {
            self.curtoken.extend_from_slice(&s[i..]);
            return s.len();
        };
        self.curtoken.extend_from_slice(&s[i..j]);
        self.parse1 = Parse1::Main;
        // We ignore comments.
        j
    }

    // pdfminer/psparser.py:317 PSBaseParser._parse_literal
    fn parse_literal(&mut self, s: &[u8], i: usize) -> PyResult<usize> {
        let Some(j) = search(s, i, C_END_LITERAL, true) else {
            self.curtoken.extend_from_slice(&s[i..]);
            return Ok(s.len());
        };
        self.curtoken.extend_from_slice(&s[i..j]);
        if s[j] == b'#' {
            self.hex.clear();
            self.parse1 = Parse1::LiteralHex;
            return Ok(j + 1);
        }
        // str(self._curtoken, "utf-8"), else the bytes themselves.
        let lit = match std::str::from_utf8(&self.curtoken) {
            Ok(name) => LIT(name),
            Err(_) => LIT_bytes(&self.curtoken),
        };
        py_intern(Symbol::Literal(lit))?;
        self.add_token(PDFObject::Literal(lit))?;
        self.parse1 = Parse1::Main;
        Ok(j)
    }

    // pdfminer/psparser.py:337 PSBaseParser._parse_literal_hex
    fn parse_literal_hex(&mut self, s: &[u8], i: usize) -> usize {
        if let Some(&c) = s.get(i) {
            if has(c, C_HEX) && self.hex.len() < 2 {
                self.hex.push(c);
                return i + 1;
            }
        }
        if !self.hex.is_empty() {
            let value = self.hex.iter().fold(0u8, |acc, &c| acc * 16 + hex_digit(c));
            self.curtoken.push(value);
        }
        self.parse1 = Parse1::Literal;
        i
    }

    // pdfminer/psparser.py:347 PSBaseParser._parse_number
    fn parse_number(&mut self, s: &[u8], i: usize) -> PyResult<usize> {
        let Some(j) = search(s, i, C_DIGIT, false) else {
            self.curtoken.extend_from_slice(&s[i..]);
            return Ok(s.len());
        };
        self.curtoken.extend_from_slice(&s[i..j]);
        if s[j] == b'.' {
            self.curtoken.push(b'.');
            self.parse1 = Parse1::Float;
            return Ok(j + 1);
        }
        // with contextlib.suppress(ValueError): self._add_token(int(...)):
        // suppress.__init__ (called from C), __enter__ (called from C by
        // BEFORE_WITH), _add_token and __exit__ (called inline, or from C
        // when int() raised) each take a frame one level down: the first
        // one decides.
        c_called_leaf()?;
        if let Ok(v) = PyInt::parse_bytes(&self.curtoken, 10) {
            self.add_token(PDFObject::Int(v))?;
        }
        self.parse1 = Parse1::Main;
        Ok(j)
    }

    // pdfminer/psparser.py:364 PSBaseParser._parse_float
    fn parse_float(&mut self, s: &[u8], i: usize) -> PyResult<usize> {
        let Some(j) = search(s, i, C_DIGIT, false) else {
            self.curtoken.extend_from_slice(&s[i..]);
            return Ok(s.len());
        };
        self.curtoken.extend_from_slice(&s[i..j]);
        // with contextlib.suppress(ValueError): float(self._curtoken)
        // (the frames of the `with`, as in parse_number)
        c_called_leaf()?;
        if let Ok(v) = float_from_bytes(&self.curtoken) {
            self.add_token(PDFObject::Float(v))?;
        }
        self.parse1 = Parse1::Main;
        Ok(j)
    }

    // pdfminer/psparser.py:376 PSBaseParser._parse_keyword
    fn parse_keyword(&mut self, s: &[u8], i: usize) -> PyResult<usize> {
        let Some(j) = search(s, i, C_END_LITERAL, true) else {
            self.curtoken.extend_from_slice(&s[i..]);
            return Ok(s.len());
        };
        self.curtoken.extend_from_slice(&s[i..j]);
        let token = match &self.curtoken[..] {
            b"true" => PDFObject::Bool(true),
            b"false" => PDFObject::Bool(false),
            name => {
                let kwd = self.keyword_cache.get(name);
                py_intern(Symbol::Keyword(kwd))?;
                PDFObject::Keyword(kwd)
            }
        };
        self.add_token(token)?;
        self.parse1 = Parse1::Main;
        Ok(j)
    }

    // pdfminer/psparser.py:394 PSBaseParser._parse_string
    fn parse_string(&mut self, s: &[u8], i: usize) -> PyResult<usize> {
        let Some(j) = search(s, i, C_END_STRING, true) else {
            self.curtoken.extend_from_slice(&s[i..]);
            return Ok(s.len());
        };
        self.curtoken.extend_from_slice(&s[i..j]);
        let c = s[j];
        if c == b'\\' {
            self.oct.clear();
            self.parse1 = Parse1::String1;
            return Ok(j + 1);
        }
        if c == b'(' {
            self.paren += 1;
            self.curtoken.push(c);
            return Ok(j + 1);
        }
        if c == b')' {
            self.paren -= 1;
            if self.paren != 0 {
                // WTF, they said balanced parens need no special treatment.
                self.curtoken.push(c);
                return Ok(j + 1);
            }
        }
        let token = PDFObject::Bytes(Rc::from(&self.curtoken[..]));
        self.add_token(token)?;
        self.parse1 = Parse1::Main;
        Ok(j + 1)
    }

    // pdfminer/psparser.py:420 PSBaseParser._parse_string_1
    fn parse_string_1(&mut self, s: &[u8], mut i: usize) -> PyResult<usize> {
        let c = s.get(i).copied();
        if let Some(c) = c.filter(|&c| has(c, C_OCT) && self.oct.len() < 3) {
            self.oct.push(c);
            return Ok(i + 1);
        } else if !self.oct.is_empty() {
            let chrcode = self.oct.iter().fold(0u32, |acc, &d| acc * 8 + u32::from(d - b'0'));
            if chrcode >= 256 {
                return Err(exc::AssertionError(format!("Invalid octal {} ({chrcode})", bytes_repr(&self.oct))));
            }
            self.curtoken.push(chrcode as u8);
            self.parse1 = Parse1::String;
            return Ok(i);
        } else if let Some(e) = c.and_then(esc_string) {
            self.curtoken.push(e);
        } else if c == Some(b'\r') && s.len() > i + 1 && s[i + 1] == b'\n' {
            // If current and next character is \r\n skip both because enters
            // after a \ are ignored
            i += 1;
        }
        // default action
        self.parse1 = Parse1::String;
        Ok(i + 1)
    }

    // pdfminer/psparser.py:449 PSBaseParser._parse_wopen
    fn parse_wopen(&mut self, s: &[u8], mut i: usize) -> PyResult<usize> {
        if s.get(i) == Some(&b'<') {
            self.add_token(PDFObject::Keyword(self.dict_keywords.0))?;
            self.parse1 = Parse1::Main;
            i += 1;
        } else {
            self.parse1 = Parse1::HexString;
        }
        Ok(i)
    }

    // pdfminer/psparser.py:459 PSBaseParser._parse_wclose
    fn parse_wclose(&mut self, s: &[u8], mut i: usize) -> PyResult<usize> {
        if s.get(i) == Some(&b'>') {
            self.add_token(PDFObject::Keyword(self.dict_keywords.1))?;
            i += 1;
        }
        self.parse1 = Parse1::Main;
        Ok(i)
    }

    // pdfminer/psparser.py:467 PSBaseParser._parse_hexstring
    fn parse_hexstring(&mut self, s: &[u8], i: usize) -> PyResult<usize> {
        let Some(j) = search(s, i, C_SPACE | C_HEX, false) else {
            self.curtoken.extend_from_slice(&s[i..]);
            return Ok(s.len());
        };
        self.curtoken.extend_from_slice(&s[i..j]);
        // HEX_PAIR.sub(lambda m: bytes((int(m.group(0), 16),)),
        //              SPC.sub(b"", self._curtoken)):
        // the token holds only hex digits once spaces are removed; each pair
        // gives a byte, a trailing single digit (matched by `.`) its value.
        let mut token = Vec::with_capacity(self.curtoken.len() / 2 + 1);
        let mut pending: Option<u8> = None;
        for &c in self.curtoken.iter().filter(|&&c| !has(c, C_SPACE)) {
            match pending.take() {
                None => pending = Some(hex_digit(c)),
                Some(hi) => token.push((hi << 4) | hex_digit(c)),
            }
        }
        if let Some(d) = pending {
            token.push(d);
        }
        if !token.is_empty() {
            // re.sub calls the lambda (from C) for each match, one level
            // down: the first call decides.
            c_called_leaf()?;
        }
        self.add_token(PDFObject::Bytes(token.into()))?;
        self.parse1 = Parse1::Main;
        Ok(j)
    }
}

// ---------------------------------------------------------------------------
// The parser

/// `self.buf`: the current chunk, a window on the bytes of `fp`.
#[derive(Clone, Debug)]
pub struct Buf {
    data: Rc<[u8]>,
    start: usize,
    end: usize,
}

impl Buf {
    pub fn empty() -> Buf {
        Buf { data: Rc::from(&[][..]), start: 0, end: 0 }
    }

    pub fn new(data: Rc<[u8]>, range: std::ops::Range<usize>) -> Buf {
        Buf { data, start: range.start, end: range.end }
    }
}

impl Deref for Buf {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.data[self.start..self.end]
    }
}

/// A debug-build watchdog over the read position of a parser.
///
/// Between two `seek`s, a parser only reads forward: the position returned
/// by [`PSStackParserExt::read_position`] grows, except when the tokenizer
/// switches state without consuming a byte, which never happens more than
/// twice in a row. (Something else moving `fp`, as `revreadlines` and
/// `PDFParser` do, restarts the guard.) The reading loops (`nexttoken`,
/// `nextline`, `get_inline_data`) report the position after each step; a
/// broken port (or a mutant, see `docs/verification.md`, "Mutations") that
/// stops consuming its input, or rereads it, panics after a few steps
/// instead of looping, often while allocating, until a timeout or the
/// memory limit. Without `debug_assertions` it is empty and costs nothing.
#[derive(Default)]
pub struct ProgressGuard {
    /// The furthest position reached since the last seek.
    #[cfg(debug_assertions)]
    high: Option<(usize, Pos)>,
    #[cfg(debug_assertions)]
    stalls: u32,
}

impl ProgressGuard {
    /// Consecutive steps without progress tolerated (correct code needs two).
    #[cfg(debug_assertions)]
    const MAX_STALLS: u32 = 8;

    /// Records a step that ended at `position`; panics after too many steps
    /// in a row that did not go beyond the furthest position.
    #[inline]
    pub fn check(&mut self, position: (usize, Pos), what: &str) {
        #[cfg(debug_assertions)]
        {
            if self.high.is_some_and(|high| position <= high) {
                self.stalls += 1;
                assert!(self.stalls < Self::MAX_STALLS, "{what}: no progress at {position:?}");
            } else {
                self.high = Some(position);
                self.stalls = 0;
            }
        }
        #[cfg(not(debug_assertions))]
        let _ = (position, what);
    }

    /// Restarts the guard when `fp` (now at `tell`) is not where the parser
    /// left it, right after the chunk `buf` read at `bufpos`: something
    /// else moved it. The chunk's raw range is used, not its contents.
    #[inline]
    pub fn restart_if_moved(&mut self, bufpos: Pos, buf: &Buf, tell: Pos) {
        #[cfg(debug_assertions)]
        if bufpos + (buf.end - buf.start) as Pos != tell {
            *self = ProgressGuard::default();
        }
        #[cfg(not(debug_assertions))]
        let _ = (bufpos, buf, tell);
    }
}

/// Methods a Python subclass of `PSStackParser` overrides (see the module
/// documentation). Every default is the base-class behavior, and takes the
/// Python frames of the base-class method (an override takes its own).
pub trait PSStackParserExt: Sized {
    /// `seek(pos)`
    fn seek(p: &mut PSStackParser<Self>, pos: Pos) -> PyResult<()> {
        p.psstackparser_seek(pos)
    }

    /// `fillbuf()`: returns whether the source changed to a new stream.
    fn fillbuf(p: &mut PSStackParser<Self>) -> PyResult<bool> {
        p.psbaseparser_fillbuf()
    }

    // pdfminer/psparser.py:570 PSStackParser.do_keyword
    /// `do_keyword(pos, token)`
    fn do_keyword(_p: &mut PSStackParser<Self>, _pos: Pos, _token: PSKeyword) -> PyResult<()> {
        leaf(1)
    }

    // pdfminer/psparser.py:174 PSBaseParser.flush
    /// `flush()`
    fn flush(_p: &mut PSStackParser<Self>) -> PyResult<()> {
        leaf(1)
    }

    /// The position of the next byte to read, for [`ProgressGuard`] (not a
    /// Python method): it only grows between two seeks. The default is the
    /// offset in `fp`; a parser that reads several files one after the
    /// other puts the file's ordinal first.
    fn read_position(p: &PSStackParser<Self>) -> (usize, Pos) {
        (0, p.position())
    }
}

/// A plain `PSStackParser` (or `PSBaseParser`): no overrides.
impl PSStackParserExt for () {}

/// pdfminer/psparser.py:161 `PSBaseParser` and :522 `PSStackParser`, with the
/// state of a subclass in `ext` (see the module documentation).
pub struct PSStackParser<X: PSStackParserExt = ()> {
    // PSBaseParser
    /// `self.fp`; `None` only for `PDFContentParser` between streams.
    pub fp: Option<BytesIO>,
    pub eof: bool,
    pub bufpos: Pos,
    pub buf: Buf,
    pub charpos: usize,
    pub lexer: Lexer,
    // PSStackParser
    pub context: Vec<(Pos, Option<&'static str>, Vec<PSStackEntry>)>,
    pub curtype: Option<&'static str>,
    pub curstack: Vec<PSStackEntry>,
    pub results: VecDeque<PSStackEntry>,
    /// The subclass state.
    pub ext: X,
    /// `KEYWORD_ARRAY_BEGIN`, `_END`, `KEYWORD_DICT_BEGIN`, `_END`,
    /// `KEYWORD_PROC_BEGIN`, `_END`.
    structure_keywords: [PSKeyword; 6],
    /// Reset by every seek.
    progress: ProgressGuard,
}

/// pdfminer/psparser.py:161 `PSBaseParser`.
pub type PSBaseParser = PSStackParser<()>;

/// `log = logging.getLogger(__name__)` of `pdfminer/psparser.py`.
static LOG: Logger = Logger::new("pdfminer.psparser");

impl<X: PSStackParserExt> PSStackParser<X> {
    // pdfminer/psparser.py:166 PSBaseParser.__init__ + :523 PSStackParser.__init__
    /// `PSStackParser.__init__(self, fp)`, with the subclass state `ext`
    /// (Python subclasses set their attributes before or after calling it;
    /// the call itself only uses `fp` and the `seek` override). A subclass
    /// constructor (`PDFContentParser::new`) calls this, after taking its
    /// own frames. The frames of both `__init__`s are taken here.
    pub fn init(fp: Option<BytesIO>, ext: X) -> PyResult<Self> {
        let _stack_init = py_frame()?;
        let mut p = PSStackParser {
            fp,
            eof: false,
            bufpos: 0,
            buf: Buf::empty(),
            charpos: 0,
            lexer: Lexer::new(),
            context: Vec::new(),
            curtype: None,
            curstack: Vec::new(),
            results: VecDeque::new(),
            ext,
            structure_keywords: [
                *KEYWORD_ARRAY_BEGIN,
                *KEYWORD_ARRAY_END,
                *KEYWORD_DICT_BEGIN,
                *KEYWORD_DICT_END,
                *KEYWORD_PROC_BEGIN,
                *KEYWORD_PROC_END,
            ],
            progress: ProgressGuard::default(),
        };
        {
            // PSBaseParser.__init__(self, fp)
            let _base_init = py_frame()?;
            p.seek(0)?;
        }
        p.reset()?;
        Ok(p)
    }

    /// `self.fp`, which Python would dereference.
    pub fn fp_mut(&mut self, attr: &str) -> PyResult<&mut BytesIO> {
        let fp = self
            .fp
            .as_mut()
            .ok_or_else(|| exc::AttributeError(format!("'NoneType' object has no attribute '{attr}'")))?;
        // a file the PDF closed (pdfplumber/pdf.py:124 PDF.close)
        fp.check_open(attr)?;
        Ok(fp)
    }

    /// `self.seek(pos)`, dispatched to the subclass.
    pub fn seek(&mut self, pos: Pos) -> PyResult<()> {
        X::seek(self, pos)
    }

    /// `self.fillbuf()`, dispatched to the subclass.
    pub fn fillbuf(&mut self) -> PyResult<bool> {
        X::fillbuf(self)
    }

    /// `self.flush()`, dispatched to the subclass.
    pub fn flush(&mut self) -> PyResult<()> {
        X::flush(self)
    }

    /// `self.do_keyword(pos, token)`, dispatched to the subclass.
    pub fn do_keyword(&mut self, pos: Pos, token: PSKeyword) -> PyResult<()> {
        X::do_keyword(self, pos, token)
    }

    /// `self.bufpos + self.charpos`: the file offset of the next byte.
    pub fn position(&self) -> Pos {
        self.bufpos + self.charpos as Pos
    }

    // pdfminer/psparser.py:177 PSBaseParser.seek
    /// `PSBaseParser.seek(self, pos)`.
    pub fn psbaseparser_seek(&mut self, pos: Pos) -> PyResult<()> {
        let _frame = py_frame()?;
        LOG.debug()?;
        self.fp_mut("seek")?.seek(pos)?;
        // reset the status for nextline()
        self.bufpos = pos;
        self.buf = Buf::empty();
        self.charpos = 0;
        // reset the status for nexttoken()
        self.lexer.parse1 = Parse1::Main;
        self.lexer.curtoken.clear();
        self.lexer.curtokenpos = 0;
        self.lexer.tokens.clear();
        self.eof = false;
        self.progress = ProgressGuard::default();
        Ok(())
    }

    /// The frames `self.seek(pos)` enters before `self.fp.seek(pos)` raises
    /// for a position the file object refuses (`PSStackParser.seek`,
    /// `PSBaseParser.seek` and its `log.debug`), then `err`: a parser whose
    /// `seek` is the base class's, given a Python int
    /// (`pdfparser::PDFParser::seek_int`, which knows the file's errors:
    /// `pdfparser::FileKind`).
    pub fn seek_error(&mut self, err: pycompat::PyErr) -> PyResult<()> {
        let _stack_seek = py_frame()?;
        let _base_seek = py_frame()?;
        LOG.debug()?;
        Err(err)
    }

    /// Reports the read position after a step of a reading loop (see
    /// [`ProgressGuard`]).
    #[inline]
    pub fn check_progress(&mut self, what: &str) {
        let position = X::read_position(self);
        self.progress.check(position, what);
    }

    // pdfminer/psparser.py:533 PSStackParser.seek
    /// `PSStackParser.seek(self, pos)`.
    pub fn psstackparser_seek(&mut self, pos: Pos) -> PyResult<()> {
        let _frame = py_frame()?;
        self.psbaseparser_seek(pos)?;
        self.reset()
    }

    // pdfminer/psparser.py:192 PSBaseParser.fillbuf
    /// `PSBaseParser.fillbuf(self)`.
    pub fn psbaseparser_fillbuf(&mut self) -> PyResult<bool> {
        let _frame = py_frame()?;
        if self.charpos < self.buf.len() {
            return Ok(false);
        }
        // fetch next chunk.
        let fp = self.fp_mut("tell")?;
        let bufpos = fp.tell();
        let range = fp.read(Some(BUFSIZ));
        let data = fp.data().clone();
        self.progress.restart_if_moved(self.bufpos, &self.buf, bufpos);
        self.bufpos = bufpos;
        self.buf = Buf::new(data, range);
        if self.buf.is_empty() {
            return Err(exc::PSEOF("Unexpected EOF"));
        }
        self.charpos = 0;
        Ok(false)
    }

    // pdfminer/psparser.py:203 PSBaseParser.nextline
    /// Fetches a next line that ends either with `\r` or `\n`.
    pub fn nextline(&mut self) -> PyResult<(Pos, Vec<u8>)> {
        let _frame = py_frame()?;
        let mut linebuf = Vec::new();
        let linepos = self.position();
        let mut eol = false;
        loop {
            self.fillbuf()?;
            if eol {
                // handle b'\r\n'
                if self.buf.get(self.charpos) == Some(&b'\n') {
                    linebuf.push(b'\n');
                    self.charpos += 1;
                }
                break;
            }
            match search(&self.buf, self.charpos, C_EOL, true) {
                Some(m) => {
                    linebuf.extend_from_slice(&self.buf[self.charpos..=m]);
                    self.charpos = m + 1;
                    if linebuf.last() == Some(&b'\r') {
                        eol = true;
                    } else {
                        break;
                    }
                }
                None => {
                    linebuf.extend_from_slice(&self.buf[self.charpos..]);
                    self.charpos = self.buf.len();
                }
            }
            self.check_progress("nextline");
        }
        LOG.debug()?;
        Ok((linepos, linebuf))
    }

    // pdfminer/psparser.py:232 PSBaseParser.revreadlines
    /// Fetches the lines backward, from the end of `fp` (a lazy iterator,
    /// like the Python generator, whose frame each step takes).
    pub fn revreadlines(&mut self) -> PyResult<RevReadLines<'_>> {
        leaf(1)?; // the generator is created (its frame starts and suspends)
        Ok(RevReadLines { fp: self.fp_mut("seek")?, started: false, done: false, pos: 0, buf: Vec::new(), s: None })
    }

    // pdfminer/psparser.py:482 PSBaseParser.nexttoken
    pub fn nexttoken(&mut self) -> PyResult<PSToken> {
        let _frame = py_frame()?;
        if self.eof {
            // It's not really unexpected, come on now...
            return Err(exc::PSEOF("Unexpected EOF"));
        }
        while self.lexer.tokens.is_empty() {
            match self.nexttoken_step() {
                Ok(()) => self.check_progress("nexttoken"),
                Err(e) if e.is_instance(ExcKind::PSEOF) => {
                    // If we hit EOF in the middle of a token, try to parse
                    // it by tacking on whitespace, and delay raising PSEOF
                    // until next time around
                    self.charpos = self.lexer.parse1(b"\n", 0, self.bufpos)?;
                    self.eof = true;
                    // Oh, so there wasn't actually a token there? OK.
                    if self.lexer.tokens.is_empty() {
                        return Err(e);
                    }
                }
                Err(e) => return Err(e),
            }
        }
        let token = self.lexer.tokens.pop_front().expect("a token is ready");
        LOG.debug()?;
        Ok(token)
    }

    /// The body of the `try:` block of `nexttoken`.
    fn nexttoken_step(&mut self) -> PyResult<()> {
        let changed_stream = self.fillbuf()?;
        if changed_stream && !self.lexer.curtoken.is_empty() {
            // Fixes #1157: if the stream is changed in the middle of a
            // token, try to parse it by tacking on whitespace.
            self.lexer.parse1(b"\n", 0, self.bufpos)?;
        } else {
            self.charpos = self.lexer.parse1(&self.buf, self.charpos, self.bufpos)?;
        }
        Ok(())
    }

    // pdfminer/psparser.py:527 PSStackParser.reset
    pub fn reset(&mut self) -> PyResult<()> {
        leaf(1)?;
        self.context.clear();
        self.curtype = None;
        self.curstack.clear();
        self.results.clear();
        Ok(())
    }

    // pdfminer/psparser.py:537 PSStackParser.push
    pub fn push(&mut self, obj: PSStackEntry) -> PyResult<()> {
        leaf(1)?;
        self.curstack.push(obj);
        Ok(())
    }

    // pdfminer/psparser.py:540 PSStackParser.pop
    /// `self.pop(n)`: the last `n` entries (`curstack[-n:]`, so `pop(0)`
    /// takes the whole stack, like Python's slice).
    pub fn pop(&mut self, n: usize) -> PyResult<Vec<PSStackEntry>> {
        leaf(1)?;
        let start = if n == 0 { 0 } else { self.curstack.len().saturating_sub(n) };
        Ok(self.curstack.split_off(start))
    }

    // pdfminer/psparser.py:545 PSStackParser.popall
    pub fn popall(&mut self) -> PyResult<Vec<PSStackEntry>> {
        leaf(1)?;
        Ok(std::mem::take(&mut self.curstack))
    }

    // pdfminer/psparser.py:550 PSStackParser.add_results
    /// `self.add_results(*objs)`. (Its `log.debug` is in a `try:` whose
    /// `except Exception:` calls `log.debug` again: a `RecursionError` of
    /// the first is raised again by the second, at the same depth.)
    pub fn add_results(&mut self, objs: impl IntoIterator<Item = PSStackEntry>) -> PyResult<()> {
        let _frame = py_frame()?;
        LOG.debug()?;
        self.results.extend(objs);
        Ok(())
    }

    /// `self.add_results(*self.popall())` (`PDFStreamParser.flush`,
    /// `PDFContentParser.flush`): the frames of both calls. When `results`
    /// is empty (always, from `nextobject`), the two buffers trade places
    /// instead of moving the entries; otherwise the entries are appended.
    /// Either way `curstack` keeps a buffer for the next objects.
    pub fn add_results_popall(&mut self) -> PyResult<()> {
        leaf(1)?; // popall
        let _frame = py_frame()?; // add_results
        LOG.debug()?;
        let PSStackParser { curstack, results, .. } = self;
        if results.is_empty() {
            let empty: Vec<_> = std::mem::take(results).into();
            *results = std::mem::replace(curstack, empty).into();
        } else {
            results.extend(curstack.drain(..));
        }
        Ok(())
    }

    // pdfminer/psparser.py:557 PSStackParser.start_type
    pub fn start_type(&mut self, pos: Pos, type_: &'static str) -> PyResult<()> {
        let _frame = py_frame()?;
        let stack = std::mem::take(&mut self.curstack);
        self.context.push((pos, self.curtype, stack));
        self.curtype = Some(type_);
        LOG.debug()
    }

    // pdfminer/psparser.py:562 PSStackParser.end_type
    pub fn end_type(&mut self, type_: &'static str) -> PyResult<(Pos, Vec<PDFObject>)> {
        let _frame = py_frame()?;
        if self.curtype != Some(type_) {
            let cur = self.curtype.map_or_else(|| "None".to_owned(), str_repr);
            return Err(exc::PSTypeError(format!("Type mismatch: {cur} != {}", str_repr(type_))));
        }
        let objs = std::mem::take(&mut self.curstack).into_iter().map(|(_, obj)| obj).collect();
        let (pos, curtype, curstack) = self.context.pop().expect("curtype is set only with a context");
        self.curtype = curtype;
        self.curstack = curstack;
        LOG.debug()?;
        Ok((pos, objs))
    }

    // pdfminer/psparser.py:573 PSStackParser.nextobject
    /// Yields the next object: keywords, literals, strings, numbers, arrays
    /// and dictionaries (arrays and dictionaries as `PDFObject::List` and
    /// `PDFObject::Dict`).
    pub fn nextobject(&mut self) -> PyResult<PSStackEntry> {
        let _frame = py_frame()?;
        while self.results.is_empty() {
            let (pos, token) = self.nexttoken()?;
            match token {
                PDFObject::Keyword(kwd) => {
                    let [array_begin, array_end, dict_begin, dict_end, proc_begin, proc_end] = self.structure_keywords;
                    if kwd == array_begin {
                        // begin array
                        self.start_type(pos, "a")?;
                    } else if kwd == array_end {
                        // end array
                        match self.end_type("a") {
                            Ok((pos, objs)) => self.push((pos, PDFObject::List(PDFList::new(objs))))?,
                            Err(e) if e.is_instance(ExcKind::PSTypeError) && !settings::STRICT => {}
                            Err(e) => return Err(e),
                        }
                    } else if kwd == dict_begin {
                        // begin dictionary
                        self.start_type(pos, "d")?;
                    } else if kwd == dict_end {
                        // end dictionary
                        match self.end_type("d") {
                            Ok((pos, objs)) => {
                                if !objs.len().is_multiple_of(2) {
                                    let error_msg =
                                        format!("Invalid dictionary construct: {}", PDFObject::list_repr(&objs)?);
                                    return Err(exc::PSSyntaxError(error_msg));
                                }
                                // {literal_name(k): v for (k, v) in choplist(2, objs) if v is not None}:
                                // the comprehension runs in this frame (PEP 709); the
                                // generator choplist() takes a frame one level down when
                                // created and at each step, before literal_name's.
                                leaf(1)?;
                                let d = PDFDict::new();
                                for pair in choplist(2, &objs) {
                                    let (k, v) = (&pair[0], &pair[1]);
                                    if !matches!(v, PDFObject::Null) {
                                        d.insert(literal_name(k)?.into(), v.clone());
                                    }
                                }
                                self.push((pos, PDFObject::Dict(d)))?;
                            }
                            Err(e) if e.is_instance(ExcKind::PSTypeError) && !settings::STRICT => {}
                            Err(e) => return Err(e),
                        }
                    } else if kwd == proc_begin {
                        // begin proc
                        self.start_type(pos, "p")?;
                    } else if kwd == proc_end {
                        // end proc
                        match self.end_type("p") {
                            Ok((pos, objs)) => self.push((pos, PDFObject::List(PDFList::new(objs))))?,
                            Err(e) if e.is_instance(ExcKind::PSTypeError) && !settings::STRICT => {}
                            Err(e) => return Err(e),
                        }
                    } else {
                        // log.debug("do_keyword: pos=%r, token=%r, stack=%r", ...)
                        LOG.debug()?;
                        self.do_keyword(pos, kwd)?;
                    }
                }
                // normal token (int, float, bool, bytes, literal); tokens are
                // never of another type, so upstream's "unknown token" branch
                // (log.error + raise PSException) cannot be reached.
                token => self.push((pos, token))?,
            }
            if !self.context.is_empty() {
                continue;
            }
            self.flush()?;
        }
        let obj = self.results.pop_front().expect("results is not empty");
        // log.debug("nextobject: %r", obj), in a try: like add_results'
        LOG.debug()?;
        Ok(obj)
    }
}

/// The generator returned by [`PSStackParser::revreadlines`]: each step
/// resumes it (a frame), and a `RecursionError` ends it.
pub struct RevReadLines<'a> {
    fp: &'a mut BytesIO,
    started: bool,
    done: bool,
    pos: Pos,
    buf: Vec<u8>,
    /// The chunk being split (`s`), once read.
    s: Option<Vec<u8>>,
}

impl RevReadLines<'_> {
    fn step(&mut self) -> Option<Vec<u8>> {
        if !self.started {
            self.started = true;
            self.pos = self.fp.seek_whence(0, 2).expect("seek to the end");
        }
        loop {
            if let Some(s) = self.s.as_mut() {
                let n = s.iter().rposition(|&c| c == b'\r' || c == b'\n');
                match n {
                    None => {
                        let mut line = self.s.take().expect("chunk");
                        line.extend_from_slice(&self.buf);
                        self.buf = line;
                    }
                    Some(n) => {
                        let mut line = s.split_off(n);
                        line.append(&mut self.buf);
                        return Some(line);
                    }
                }
                continue;
            }
            if self.pos == 0 {
                return None;
            }
            let prevpos = self.pos;
            self.pos = (self.pos - BUFSIZ as Pos).max(0);
            self.fp.seek(self.pos).expect("non-negative position");
            let s = self.fp.read_bytes(Some((prevpos - self.pos) as usize)).to_vec();
            if s.is_empty() {
                self.pos = 0;
                return None;
            }
            self.s = Some(s);
        }
    }
}

impl Iterator for RevReadLines<'_> {
    type Item = PyResult<Vec<u8>>;

    fn next(&mut self) -> Option<PyResult<Vec<u8>>> {
        if self.done {
            return None;
        }
        let _frame = match py_frame() {
            Ok(frame) => frame,
            Err(e) => {
                self.done = true;
                return Some(Err(e));
            }
        };
        let line = self.step();
        self.done = line.is_none();
        line.map(Ok)
    }
}

impl<X: PSStackParserExt> PSStackParser<X> {
    /// A parser in the state `__init__` leaves before its `seek(0)`: no
    /// file, empty buffers and stacks. Not a Python state: it only stands
    /// in for a parser moved elsewhere for a while (`pdfparser.rs`, where a
    /// nested `getobj` uses the parser that is running `do_keyword`).
    pub fn placeholder(ext: X) -> Self {
        PSStackParser {
            fp: None,
            eof: false,
            bufpos: 0,
            buf: Buf::empty(),
            charpos: 0,
            lexer: Lexer::new(),
            context: Vec::new(),
            curtype: None,
            curstack: Vec::new(),
            results: VecDeque::new(),
            ext,
            structure_keywords: [
                *KEYWORD_ARRAY_BEGIN,
                *KEYWORD_ARRAY_END,
                *KEYWORD_DICT_BEGIN,
                *KEYWORD_DICT_END,
                *KEYWORD_PROC_BEGIN,
                *KEYWORD_PROC_END,
            ],
            progress: ProgressGuard::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pycompat::recursion::{with_depth, Depth, PY_RECURSION_LIMIT};

    /// A thread starts as a process that imported pdfplumber: interning a
    /// name of its modules' constants takes one frame, a new name two (once).
    /// libtest runs each test on a new thread (natively; on wasm32-wasip1,
    /// on the main thread, which no other test of this crate uses).
    #[test]
    fn a_thread_starts_with_the_names_pdfplumber_interns() {
        let last = Depth { py: PY_RECURSION_LIMIT - 1, c: 0 };
        with_depth(last, || {
            py_intern(Symbol::Keyword(KWD(b"endobj"))).expect("interned by pdfminer.pdfparser");
            py_intern(Symbol::Literal(LIT("Pages"))).expect("interned by pdfminer.pdfpage");
            assert!(py_intern(Symbol::Keyword(KWD(b"zq_never_interned"))).is_err());
            assert!(py_intern(Symbol::Literal(LIT("ZqNeverInterned"))).is_err());
        });
        with_depth(Depth { py: PY_RECURSION_LIMIT - 2, c: 0 }, || {
            py_intern(Symbol::Literal(LIT("ZqNeverInterned"))).expect("two frames fit");
        });
        with_depth(last, || {
            py_intern(Symbol::Literal(LIT("ZqNeverInterned"))).expect("interned now");
            assert!(py_intern(Symbol::Keyword(KWD(b"ZqNeverInterned"))).is_err(), "a keyword is another symbol");
        });
        py_symbols_reset([]);
        with_depth(last, || assert!(py_intern(Symbol::Keyword(KWD(b"endobj"))).is_err(), "a new process"));
    }
}
