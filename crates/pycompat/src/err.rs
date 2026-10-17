//! Python exceptions as values: `PyErr { kind, message }`.
//!
//! Upstream code raises and catches exceptions by class, and `except C:`
//! also catches every subclass of `C`. [`ExcKind`] lists every exception
//! class the port can raise or catch, with its Python bases, so that
//! `except C:` becomes `if err.is_instance(ExcKind::C)` (see
//! [`ExcKind::is_instance`]). `message` is exactly `str(e)`: for `KeyError`
//! and its subclasses that is the `repr()` of the argument
//! (`str(KeyError("a"))` is `"'a'"`), so their constructors take the repr.
//!
//! The classes come from CPython 3.12 builtins and the stdlib modules the
//! ported upstream code imports (`struct.error`, `zlib.error`,
//! `binascii.Error`, `re.error`; `crates/pdftrace/tests/pycompat_unit.rs`
//! checks this list against every exception class of those modules, the
//! others with the reason they cannot be raised), from every `Exception`
//! subclass defined in
//! pdfminer.six 20260107 (`psexceptions.py`, `pdfexceptions.py`,
//! `pdfparser.py`, `pdfdocument.py`, `pdffont.py`, `pdfinterp.py`,
//! `cmapdb.py`, `lzw.py`, `ccitt.py`; `pdftypes.py` and `layout.py` only
//! re-export or raise them) and from pdfplumber 0.11.9
//! (`utils/exceptions.py`, `structure.py`).
//!
//! [`ExcKind::NotPorted`] is not a Python class: it marks a code path whose
//! upstream module is not ported yet. It is deliberately outside the Python
//! hierarchy (not even an `Exception`), so no ported `except Exception:`
//! swallows it and a trace that reaches it fails loudly.

use std::fmt;

/// `Result` of an operation that can raise a Python exception.
pub type PyResult<T> = Result<T, PyErr>;

macro_rules! exc_kinds {
    ($( $(#[$doc:meta])* $kind:ident = $name:literal in $module:literal : [$($base:ident),*] ),* $(,)?) => {
        /// A Python exception class (see the module documentation).
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        #[allow(non_camel_case_types)]
        pub enum ExcKind {
            $( $(#[$doc])* $kind, )*
        }

        impl ExcKind {
            /// Every kind, in declaration order.
            pub const ALL: &'static [ExcKind] = &[$(ExcKind::$kind),*];

            /// `type(e).__name__`, as written in traces (`!PSEOF("...")`).
            pub fn name(self) -> &'static str {
                match self { $(ExcKind::$kind => $name,)* }
            }

            /// `type(e).__module__` (`"builtins"` for builtins).
            pub fn module(self) -> &'static str {
                match self { $(ExcKind::$kind => $module,)* }
            }

            /// `type(e).__bases__`, in Python order.
            pub fn bases(self) -> &'static [ExcKind] {
                match self { $(ExcKind::$kind => &[$(ExcKind::$base),*],)* }
            }
        }
    };
}

exc_kinds! {
    // --- builtins (CPython 3.12) ---------------------------------------
    BaseException = "BaseException" in "builtins": [],
    Exception = "Exception" in "builtins": [BaseException],
    ArithmeticError = "ArithmeticError" in "builtins": [Exception],
    ZeroDivisionError = "ZeroDivisionError" in "builtins": [ArithmeticError],
    OverflowError = "OverflowError" in "builtins": [ArithmeticError],
    AssertionError = "AssertionError" in "builtins": [Exception],
    AttributeError = "AttributeError" in "builtins": [Exception],
    LookupError = "LookupError" in "builtins": [Exception],
    IndexError = "IndexError" in "builtins": [LookupError],
    KeyError = "KeyError" in "builtins": [LookupError],
    ValueError = "ValueError" in "builtins": [Exception],
    UnicodeError = "UnicodeError" in "builtins": [ValueError],
    UnicodeDecodeError = "UnicodeDecodeError" in "builtins": [UnicodeError],
    UnicodeEncodeError = "UnicodeEncodeError" in "builtins": [UnicodeError],
    TypeError = "TypeError" in "builtins": [Exception],
    RuntimeError = "RuntimeError" in "builtins": [Exception],
    RecursionError = "RecursionError" in "builtins": [RuntimeError],
    NotImplementedError = "NotImplementedError" in "builtins": [RuntimeError],
    EOFError = "EOFError" in "builtins": [Exception],
    /// `next()` on an exhausted iterator (`runlength.rldecode`)
    StopIteration = "StopIteration" in "builtins": [Exception],
    /// `OSError` (`IOError` is an alias of it).
    OSError = "OSError" in "builtins": [Exception],
    ImportError = "ImportError" in "builtins": [Exception],
    MemoryError = "MemoryError" in "builtins": [Exception],
    /// Raised by a dead `weakref.proxy`; see `pdftypes::PDFObjRef`.
    ReferenceError = "ReferenceError" in "builtins": [Exception],
    Warning = "Warning" in "builtins": [Exception],
    UserWarning = "UserWarning" in "builtins": [Warning],
    SyntaxWarning = "SyntaxWarning" in "builtins": [Warning],
    DeprecationWarning = "DeprecationWarning" in "builtins": [Warning],
    /// `struct.error`
    StructError = "error" in "struct": [Exception],
    /// `zlib.error`
    ZlibError = "error" in "zlib": [Exception],
    /// `binascii.Error` (`ascii85.asciihexdecode` calls `binascii.unhexlify`)
    BinasciiError = "Error" in "binascii": [ValueError],
    /// `re.error` (pdfplumber compiles the patterns given to `search`)
    ReError = "error" in "re": [Exception],

    // --- pdfminer/psexceptions.py ----------------------------------------
    PSException = "PSException" in "pdfminer.psexceptions": [Exception],
    PSEOF = "PSEOF" in "pdfminer.psexceptions": [PSException],
    PSSyntaxError = "PSSyntaxError" in "pdfminer.psexceptions": [PSException],
    PSTypeError = "PSTypeError" in "pdfminer.psexceptions": [PSException],
    PSValueError = "PSValueError" in "pdfminer.psexceptions": [PSException],

    // --- pdfminer/pdfexceptions.py ---------------------------------------
    PDFException = "PDFException" in "pdfminer.pdfexceptions": [PSException],
    PDFTypeError = "PDFTypeError" in "pdfminer.pdfexceptions": [PDFException, TypeError],
    PDFValueError = "PDFValueError" in "pdfminer.pdfexceptions": [PDFException, ValueError],
    PDFObjectNotFound = "PDFObjectNotFound" in "pdfminer.pdfexceptions": [PDFException],
    PDFNotImplementedError = "PDFNotImplementedError" in "pdfminer.pdfexceptions": [PDFException, NotImplementedError],
    PDFKeyError = "PDFKeyError" in "pdfminer.pdfexceptions": [PDFException, KeyError],
    PDFEOFError = "PDFEOFError" in "pdfminer.pdfexceptions": [PDFException, EOFError],
    PDFIOError = "PDFIOError" in "pdfminer.pdfexceptions": [PDFException, OSError],

    // --- pdfminer/pdfparser.py -------------------------------------------
    PDFSyntaxError = "PDFSyntaxError" in "pdfminer.pdfparser": [PDFException],

    // --- pdfminer/pdfdocument.py -----------------------------------------
    PDFNoValidXRef = "PDFNoValidXRef" in "pdfminer.pdfdocument": [PDFSyntaxError],
    PDFNoValidXRefWarning = "PDFNoValidXRefWarning" in "pdfminer.pdfdocument": [SyntaxWarning],
    PDFNoOutlines = "PDFNoOutlines" in "pdfminer.pdfdocument": [PDFException],
    PDFNoPageLabels = "PDFNoPageLabels" in "pdfminer.pdfdocument": [PDFException],
    PDFDestinationNotFound = "PDFDestinationNotFound" in "pdfminer.pdfdocument": [PDFException],
    PDFEncryptionError = "PDFEncryptionError" in "pdfminer.pdfdocument": [PDFException],
    PDFPasswordIncorrect = "PDFPasswordIncorrect" in "pdfminer.pdfdocument": [PDFEncryptionError],
    PDFEncryptionWarning = "PDFEncryptionWarning" in "pdfminer.pdfdocument": [UserWarning],
    PDFTextExtractionNotAllowedWarning = "PDFTextExtractionNotAllowedWarning" in "pdfminer.pdfdocument": [UserWarning],
    PDFTextExtractionNotAllowed = "PDFTextExtractionNotAllowed" in "pdfminer.pdfdocument": [PDFEncryptionError],

    // --- pdfminer/pdffont.py ---------------------------------------------
    /// `TrueTypeFont.CMapNotFound`
    TrueTypeFont_CMapNotFound = "CMapNotFound" in "pdfminer.pdffont": [PDFException],
    PDFFontError = "PDFFontError" in "pdfminer.pdffont": [PDFException],
    PDFUnicodeNotDefined = "PDFUnicodeNotDefined" in "pdfminer.pdffont": [PDFFontError],

    // --- pdfminer/pdfinterp.py -------------------------------------------
    PDFResourceError = "PDFResourceError" in "pdfminer.pdfinterp": [PDFException],
    PDFInterpreterError = "PDFInterpreterError" in "pdfminer.pdfinterp": [PDFException],

    // --- pdfminer/cmapdb.py ----------------------------------------------
    CMapError = "CMapError" in "pdfminer.cmapdb": [PDFException],
    /// `CMapDB.CMapNotFound`
    CMapDB_CMapNotFound = "CMapNotFound" in "pdfminer.cmapdb": [CMapError],

    // --- pdfminer/lzw.py -------------------------------------------------
    CorruptDataError = "CorruptDataError" in "pdfminer.lzw": [PDFException],

    // --- pdfminer/ccitt.py (classes nested in CCITTG4Parser) --------------
    /// `CCITTG4Parser.CCITTException`
    CCITTException = "CCITTException" in "pdfminer.ccitt": [PDFException],
    /// `CCITTG4Parser.EOFB`
    EOFB = "EOFB" in "pdfminer.ccitt": [CCITTException],
    /// `CCITTG4Parser.InvalidData`
    InvalidData = "InvalidData" in "pdfminer.ccitt": [CCITTException],
    /// `CCITTG4Parser.ByteSkip`
    ByteSkip = "ByteSkip" in "pdfminer.ccitt": [CCITTException],

    // --- pdfplumber ------------------------------------------------------
    MalformedPDFException = "MalformedPDFException" in "pdfplumber.utils.exceptions": [Exception],
    PdfminerException = "PdfminerException" in "pdfplumber.utils.exceptions": [Exception],
    StructTreeMissing = "StructTreeMissing" in "pdfplumber.structure": [ValueError],

    // --- not a Python class ----------------------------------------------
    /// A path whose upstream module is not ported yet (see module docs).
    NotPorted = "NotPorted" in "pycompat": [],
}

impl ExcKind {
    /// `issubclass(self, base)`: true when `base` is `self` or one of its
    /// ancestors (multiple inheritance included).
    pub fn is_instance(self, base: ExcKind) -> bool {
        self == base || self.bases().iter().any(|b| b.is_instance(base))
    }

    /// The class as traces name it (`oracle/canon.py` `exception_name`):
    /// `type(e).__name__` for builtins, pdfminer and pdfplumber classes,
    /// `module.name` for the others (`zlib.error`, `binascii.Error`).
    pub fn trace_name(self) -> String {
        match self.module().split('.').next() {
            Some("builtins" | "pdfminer" | "pdfplumber" | "pycompat") => self.name().to_owned(),
            _ => format!("{}.{}", self.module(), self.name()),
        }
    }

    /// `type(e).__qualname__`: the name, prefixed by the enclosing class for
    /// the classes pdfminer nests inside another class.
    pub fn qualname(self) -> String {
        let outer = match self {
            ExcKind::TrueTypeFont_CMapNotFound => "TrueTypeFont.",
            ExcKind::CMapDB_CMapNotFound => "CMapDB.",
            ExcKind::CCITTException | ExcKind::EOFB | ExcKind::InvalidData | ExcKind::ByteSkip => "CCITTG4Parser.",
            _ => "",
        };
        format!("{outer}{}", self.name())
    }
}

/// What a [`PyErr`] holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PyErrInfo {
    pub kind: ExcKind,
    /// `str(e)`, or, when `str(e)` holds a lone surrogate (a user's `str`
    /// formatted into the message), that text with U+FFFD in its place:
    /// for Rust-side checks and logs only. Whatever shows `str(e)` to a
    /// user (traces, the JavaScript boundary, whose strings hold lone
    /// surrogates) reads [`PyErr::py_message`].
    pub message: String,
    /// `str(e)` as `surrogatepass` UTF-8 bytes (see [`crate::PyStr`]) when
    /// it holds a lone surrogate; `None` otherwise. Bytes rather than a
    /// `PyStr` keep a `PyErr` `Send`.
    pub surrogate_message: Option<Box<[u8]>>,
}

/// A raised Python exception: its class and `str(e)` (`err.kind`,
/// `err.message`).
///
/// The information is boxed so that a `PyResult<T>` stays small: results
/// of the parser's hot paths are returned in registers, and an error, the
/// rare case, pays for an allocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PyErr(Box<PyErrInfo>);

impl std::ops::Deref for PyErr {
    type Target = PyErrInfo;
    fn deref(&self) -> &PyErrInfo {
        &self.0
    }
}

impl PyErr {
    pub fn new(kind: ExcKind, message: impl Into<String>) -> PyErr {
        PyErr(Box::new(PyErrInfo { kind, message: message.into(), surrogate_message: None }))
    }

    /// An error whose `str(e)` is a Python `str` (it may hold a lone
    /// surrogate).
    pub fn with_message(kind: ExcKind, message: &crate::PyStr) -> PyErr {
        match message.as_str() {
            Some(text) => PyErr::new(kind, text),
            None => PyErr(Box::new(PyErrInfo {
                kind,
                message: message.to_string_lossy(),
                surrogate_message: Some(Box::from(message.as_bytes())),
            })),
        }
    }

    /// `str(e)`, exactly.
    pub fn py_message(&self) -> crate::PyStr {
        match &self.surrogate_message {
            None => crate::PyStr::new(&self.message),
            Some(bytes) => crate::PyStr::from_surrogatepass_utf8(bytes).expect("the bytes of a str"),
        }
    }

    /// `isinstance(e, base)`: the test behind `except base:`.
    pub fn is_instance(&self, base: ExcKind) -> bool {
        self.kind.is_instance(base)
    }

    /// An error for a path whose upstream code is not ported yet;
    /// `what` names the upstream function (`"pdfminer.lzw.lzwdecode"`).
    pub fn not_ported(what: &str) -> PyErr {
        PyErr::new(ExcKind::NotPorted, format!("not ported yet: {what}"))
    }
}

impl fmt::Display for PyErr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.message.is_empty() {
            f.write_str(self.kind.name())
        } else {
            write!(f, "{}: {}", self.kind.name(), self.message)
        }
    }
}

impl std::error::Error for PyErr {}

/// Shorthand constructors, one per kind, named like the Python class:
/// `exc::ValueError("...")`.
#[allow(non_snake_case)]
pub mod exc {
    use super::{ExcKind, PyErr};

    macro_rules! ctors {
        ($($kind:ident),* $(,)?) => {
            $(
                #[doc = concat!("`", stringify!($kind), "(message)`")]
                pub fn $kind(message: impl Into<String>) -> PyErr {
                    PyErr::new(ExcKind::$kind, message)
                }
            )*
        };
    }

    ctors! {
        Exception, ZeroDivisionError, OverflowError, AssertionError, AttributeError,
        IndexError, KeyError, ValueError, UnicodeDecodeError, TypeError, RecursionError,
        NotImplementedError, EOFError, OSError, MemoryError, ReferenceError, StructError, ZlibError, BinasciiError,
        ReError,
        PSException, PSEOF, PSSyntaxError, PSTypeError, PSValueError,
        PDFException, PDFTypeError, PDFValueError, PDFObjectNotFound, PDFNotImplementedError,
        PDFKeyError, PDFEOFError, PDFIOError, PDFSyntaxError, PDFNoValidXRef,
        PDFFontError, PDFInterpreterError, PDFResourceError, CMapError,
    }
}
