//! The Python frames of the calls `pdffont` makes into code that does not
//! count its own (`docs/architecture.md`, "Profondeur de récursion").
//!
//! A font is built and queried by the interpreter at any depth: a Form
//! XObject's resources are initialized (fonts created) and its text shown
//! (`decode`, `to_unichr`, `char_width`, `char_disp`) at the depth of its
//! nesting, so a `RecursionError` can happen inside a font method. Every
//! ported function of `pdffont` takes its frame; the helpers here take the
//! frames of the functions it calls that do not count theirs:
//!
//! - the accessors of `pdftypes` (`dict_value`, `list_value`, `int_value`,
//!   `num_value`, `stream_value`) and `resolve1` (their Rust ports leave
//!   their frames to the caller, `PDFObjRef.resolve` and the document's
//!   `getobj` count their own);
//! - `literal_name` (`psparser`; `str()` of a non-literal counts its C
//!   units itself);
//! - the `casting` functions;
//! - `cmapdb` (`UnicodeMap.get_unichr`, `CMap.decode` and their
//!   `log.debug`, the constructors) and `encodingdb.EncodingDB.get_encoding`;
//! - logging calls: `log.debug(...)` runs `Logger.debug` and
//!   `Logger.isEnabledFor` (two frames below the caller) and, the first
//!   time a level is asked of a logger since its cache was cleared
//!   (`logging.disable`, which the oracle runs before a stage or a unit
//!   case), `_acquireLock` (a third). [`log_frames`] keeps that cache per
//!   thread; [`clear_logger_cache`] is `logging.disable()`.
//!
//! When one of those modules starts counting its own frames, the matching
//! helper here must go.

use std::borrow::Cow;

use pycompat::recursion::{self, eval_entry, py_frame, py_frames, Guard};
use pycompat::{ExcKind, PyInt, PyResult};

use crate::encodingdb::{name2unicode, Cid2Unicode, EncodingDB};
use crate::glyphlist::glyphname2unicode;
use crate::pdftypes::PDFObject;
use crate::psparser::LiteralName;

/// The pdfminer loggers `pdffont` and the code it calls write to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Logger {
    /// `pdfminer.pdffont`
    PdfFont,
    /// `pdfminer.pdfinterp` (`PDFResourceManager.get_font`)
    PdfInterp,
    /// `pdfminer.cmapdb`
    CMapDb,
    /// `pdfminer.encodingdb`
    EncodingDb,
}

impl Logger {
    /// The logger's name (`logging.getLogger(__name__)`).
    pub fn name(self) -> &'static str {
        match self {
            Logger::PdfFont => "pdfminer.pdffont",
            Logger::PdfInterp => "pdfminer.pdfinterp",
            Logger::CMapDb => "pdfminer.cmapdb",
            Logger::EncodingDb => "pdfminer.encodingdb",
        }
    }
}

/// The levels of the logging calls made on these paths.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Debug,
    Warning,
}

impl Level {
    /// `logging.DEBUG`, `logging.WARNING`
    pub fn number(self) -> u32 {
        match self {
            Level::Debug => recursion::DEBUG,
            Level::Warning => recursion::WARNING,
        }
    }
}

/// `logging.disable(...)`: clears the cache of every logger
/// ([`recursion::clear_logger_cache`]).
pub use recursion::clear_logger_cache;

/// The frames of `log.<level>(...)` ([`recursion::log_frames`]): two, three
/// the first time since the cache was cleared.
pub fn log_frames(logger: Logger, level: Level) -> PyResult<()> {
    recursion::log_frames(logger.name(), level.number())
}

/// One Python frame for the duration of a call (a ported Python function,
/// or a callee whose port does not count it).
pub fn frame() -> PyResult<Guard> {
    py_frame()
}

/// The frame of a Python function called from C (a class's `__init__` run
/// by `type.__call__`, a generator resumed by `sum()`): the evaluation loop
/// entry (two C units), then the frame.
pub fn c_called() -> PyResult<(Guard, Guard)> {
    let entry = eval_entry()?;
    Ok((entry, py_frame()?))
}

/// `n` nested frames of a call that returns at once (only checked).
pub fn leaf(n: u32) -> PyResult<()> {
    drop(py_frames(n)?);
    Ok(())
}

// ---------------------------------------------------------------------------
// encodingdb

/// Whether `name2unicode(name)` of a name without `.` or `_` enters
/// `raise_key_error_for_invalid_unicode`: not a glyph name, and a
/// `uniXXXX...` or `uXXXX[XX]` name whose digits all read as hex (the
/// branches of pdfminer/encodingdb.py:16 `name2unicode` before the call).
fn reaches_raise_key_error(name: &str) -> bool {
    if glyphname2unicode.contains_key(name) {
        return false;
    }
    let strip = |chars: &str| name.trim_matches(|c| chars.contains(c)).to_owned();
    let hex_start = |s: &str| s.chars().next().is_some_and(|c| c.is_ascii_hexdigit());
    if name.starts_with("uni") {
        let s = strip("uni");
        let digits: Vec<char> = s.chars().collect();
        hex_start(&s)
            && digits.len().is_multiple_of(4)
            && digits.chunks(4).all(|c| PyInt::parse_str(&c.iter().collect::<String>(), 16).is_ok())
    } else if name.starts_with('u') {
        let s = strip("u");
        hex_start(&s) && (4..=6).contains(&s.chars().count()) && PyInt::parse_str(&s, 16).is_ok()
    } else {
        false
    }
}

/// `name2unicode(name)` (`encodingdb`, which does not count its frames)
/// with its frames: its own, `raise_key_error_for_invalid_unicode`'s, and
/// for a ligature (`a_b`) the frame of each component's call, made by
/// `str.join` through `map` (from C), until a component raises.
pub fn name2unicode_counted(name: &str) -> PyResult<String> {
    let _frame = frame()?;
    let base = name.split('.').next().expect("split yields a part");
    let components: Vec<&str> = base.split('_').collect();
    if components.len() > 1 {
        let mut out = String::new();
        for component in components {
            let _call = c_called()?;
            if reaches_raise_key_error(component) {
                leaf(1)?;
            }
            out.push_str(&name2unicode(component)?);
        }
        return Ok(out);
    }
    if reaches_raise_key_error(base) {
        leaf(1)?;
    }
    name2unicode(name)
}

/// `EncodingDB.get_encoding(name, diff)` (`encodingdb`, which does not
/// count its frames) with its frames: its own, then for each name of `diff`
/// those of `name2unicode` and, when it raises, of `log.debug(str(e))`.
/// `name2unicode` raises only `KeyError` and `ValueError` (caught there),
/// so only a `RecursionError` comes out.
pub fn get_encoding_counted(name: &str, diff: Option<&[PDFObject]>) -> PyResult<Cow<'static, Cid2Unicode>> {
    {
        let _frame = frame()?;
        for x in diff.unwrap_or_default() {
            let PDFObject::Literal(lit) = x else { continue };
            let failed = match lit.name() {
                LiteralName::Str(s) => match name2unicode_counted(s) {
                    Ok(_) => false,
                    Err(e) if e.is_instance(ExcKind::RecursionError) => return Err(e),
                    Err(_) => true,
                },
                // name2unicode's isinstance check raises at once
                LiteralName::Bytes(_) => {
                    leaf(1)?;
                    true
                }
            };
            if failed {
                log_frames(Logger::EncodingDb, Level::Debug)?;
            }
        }
    }
    EncodingDB::get_encoding(name, diff)
}
