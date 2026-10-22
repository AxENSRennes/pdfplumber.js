//! The Python frames of a `logging` call, which decide where a
//! `RecursionError` happens in a deep recursion (`docs/architecture.md`,
//! "Profondeur de récursion").
//!
//! pdfminer logs with `log = logging.getLogger(__name__)` and
//! `log.debug(...)`, `log.warning(...)`... It never reads its log level, so
//! a call changes nothing but the stack: with logging disabled
//! (`logging.disable(logging.CRITICAL)`, which the oracle runs before a
//! stage and before the unit cases), `Logger.debug` (CPython 3.12
//! `Lib/logging/__init__.py`) calls `Logger.isEnabledFor`, two frames below
//! the caller, which answers from `Logger._cache`. When the level is not in
//! that cache (the first call of a level on a logger since
//! `logging.disable()` cleared every cache), `isEnabledFor` calls
//! `_acquireLock` (a third frame), fills the cache and calls
//! `_releaseLock`: the cache is filled only once the third frame was
//! entered. The arguments are not formatted (no `repr()` is run).
//!
//! [`Logger::call`] takes these frames and leaves them at once (the call
//! returns before the caller goes on); the cache is per thread, as the
//! recursion counters are ([`crate::recursion`]), and [`disable`] is
//! `logging.disable(...)`: the harness calls it where the oracle does.

use std::cell::RefCell;

use crate::err::PyResult;
use crate::recursion::py_frames;

/// The level of a logging call (`Logger.debug`, `.info`, `.warning`,
/// `.error`): each has its own entry in a logger's cache.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Level {
    Debug = 1,
    Info = 2,
    Warning = 4,
    Error = 8,
}

/// A logger, `logging.getLogger(name)`.
#[derive(Debug)]
pub struct Logger {
    pub name: &'static str,
}

thread_local! {
    /// `Logger._cache` of each logger: the levels it holds, by logger name.
    static CACHE: RefCell<Vec<(&'static str, u8)>> = const { RefCell::new(Vec::new()) };
}

/// `logging.disable(level)`: clears the cache of every logger (the next
/// call of each level takes three frames again).
pub fn disable() {
    CACHE.with(|c| c.borrow_mut().clear());
}

/// The cache of every logger: a snapshot of the state, to put back with
/// [`restore`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoggerCaches(Vec<(&'static str, u8)>);

/// A snapshot of the logger caches.
pub fn snapshot() -> LoggerCaches {
    CACHE.with(|c| LoggerCaches(c.borrow().clone()))
}

/// Puts back a [`snapshot`].
pub fn restore(caches: LoggerCaches) {
    CACHE.with(|c| *c.borrow_mut() = caches.0);
}

impl Logger {
    /// `logging.getLogger(name)`.
    pub const fn new(name: &'static str) -> Logger {
        Logger { name }
    }

    /// Whether `level` is in this logger's cache.
    fn cached(&self, level: Level) -> bool {
        CACHE.with(|c| c.borrow().iter().any(|&(name, levels)| name == self.name && levels & level as u8 != 0))
    }

    fn fill(&self, level: Level) {
        CACHE.with(|c| {
            let mut c = c.borrow_mut();
            match c.iter_mut().find(|(name, _)| *name == self.name) {
                Some((_, levels)) => *levels |= level as u8,
                None => c.push((self.name, level as u8)),
            }
        });
    }

    /// The frames of `self.<level>(...)` with logging disabled: two, three
    /// when the level is not cached yet; raises `RecursionError` when they
    /// do not fit.
    pub fn call(&self, level: Level) -> PyResult<()> {
        if self.cached(level) {
            drop(py_frames(2)?);
        } else {
            drop(py_frames(3)?);
            self.fill(level);
        }
        Ok(())
    }

    /// `self.debug(...)`
    pub fn debug(&self) -> PyResult<()> {
        self.call(Level::Debug)
    }

    /// `self.info(...)`
    pub fn info(&self) -> PyResult<()> {
        self.call(Level::Info)
    }

    /// `self.warning(...)`
    pub fn warning(&self) -> PyResult<()> {
        self.call(Level::Warning)
    }

    /// `self.error(...)`
    pub fn error(&self) -> PyResult<()> {
        self.call(Level::Error)
    }

    /// `self.<level>(msg, *args)` with the text of its record: the frames
    /// of the call ([`Logger::call`], logging disabled: the reference),
    /// then `message()`, the record's `getMessage()` (`msg % args`), to the
    /// diagnostics channel ([`crate::diagnostics::emit`], under the
    /// logger's name), formatted only when someone collects it. A message
    /// that cannot be formatted (a `repr` that raises) is not emitted, as
    /// `logging` drops a record whose formatting raises
    /// (`Handler.handleError`). The text never changes a result.
    pub fn log(&self, level: Level, message: impl FnOnce() -> PyResult<String>) -> PyResult<()> {
        self.call(level)?;
        let lvl = match level {
            Level::Debug => crate::diagnostics::Level::Debug,
            Level::Info => crate::diagnostics::Level::Info,
            Level::Warning => crate::diagnostics::Level::Warning,
            Level::Error => crate::diagnostics::Level::Error,
        };
        if crate::diagnostics::enabled(lvl) {
            if let Ok(text) = message() {
                crate::diagnostics::emit(self.name, lvl, || text);
            }
        }
        Ok(())
    }

    /// `self.warning(msg, *args)` with its text ([`Logger::log`]).
    pub fn warning_with(&self, message: impl FnOnce() -> PyResult<String>) -> PyResult<()> {
        self.log(Level::Warning, message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recursion::{with_depth, Depth, PY_RECURSION_LIMIT};

    static A: Logger = Logger::new("a");
    static B: Logger = Logger::new("b");

    #[test]
    fn first_call_of_a_level_takes_three_frames() {
        disable();
        with_depth(Depth { py: PY_RECURSION_LIMIT - 2, c: 0 }, || {
            // not cached: needs three frames, does not fill the cache
            assert!(A.debug().is_err());
            assert!(A.debug().is_err());
        });
        with_depth(Depth { py: PY_RECURSION_LIMIT - 3, c: 0 }, || {
            A.debug().expect("three frames fit");
            B.debug().expect("another logger");
        });
        with_depth(Depth { py: PY_RECURSION_LIMIT - 2, c: 0 }, || {
            A.debug().expect("cached: two frames");
            B.debug().expect("each logger has its cache");
            assert!(A.warning().is_err(), "another level is not cached");
            assert!(B.warning().is_err());
        });
        with_depth(Depth { py: PY_RECURSION_LIMIT - 3, c: 0 }, || A.warning().expect("three frames fit"));
        with_depth(Depth { py: PY_RECURSION_LIMIT - 2, c: 0 }, || {
            A.warning().expect("cached");
            A.debug().expect("the levels of a logger add up");
            assert!(B.warning().is_err(), "the cache of another logger");
        });
        disable();
        with_depth(Depth { py: PY_RECURSION_LIMIT - 2, c: 0 }, || {
            assert!(A.debug().is_err(), "disable() cleared the cache");
            assert!(A.info().is_err());
            assert!(A.error().is_err());
        });
        with_depth(Depth { py: PY_RECURSION_LIMIT - 3, c: 0 }, || {
            A.info().expect("three frames fit");
            A.error().expect("three frames fit");
        });
        with_depth(Depth { py: PY_RECURSION_LIMIT - 1, c: 0 }, || {
            assert!(A.info().is_err(), "a cached level takes two frames");
            assert!(A.error().is_err());
        });
    }
}
