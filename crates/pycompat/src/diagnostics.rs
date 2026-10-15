//! The diagnostics channel: where the text of pdfminer's `logging` calls
//! and of pdfplumber's `warnings.warn` goes (`docs/architecture.md`,
//! "Avertissements").
//!
//! Their text never changes a result, so the port does not print it: an
//! entry point that wants it (the Wasm boundary forwards it to the `onWarning`
//! option of the JavaScript API) runs the core inside [`collect`], which
//! gathers the entries emitted by this thread at or above a level. Outside
//! [`collect`] nothing is recorded and [`enabled`] is false, so a port can
//! skip formatting a message nobody reads:
//!
//! ```
//! use pycompat::diagnostics::{self, Level};
//! let x = 3;
//! diagnostics::emit("pdfminer.pdfdocument", Level::Warning, || format!("bad xref entry {x}"));
//! ```
//!
//! This module only carries text. The cost of a logging call in Python
//! frames (which decides where a `RecursionError` falls, see
//! `docs/verification.md`) is the business of the code that ports the call,
//! whether or not anything is collected.
//!
//! The number of entries one [`collect`] keeps is bounded ([`Collected::dropped`]
//! counts the others): a warning inside a loop over a hostile document must
//! not grow without limit.

use std::cell::RefCell;

/// A `logging` level (the numeric values of Python's `logging`). A
/// `warnings.warn` is reported at [`Level::Warning`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Level {
    Debug = 10,
    Info = 20,
    Warning = 30,
    Error = 40,
    Critical = 50,
}

impl Level {
    /// The level's name, as `logging.getLevelName` writes it.
    pub fn name(self) -> &'static str {
        match self {
            Level::Debug => "DEBUG",
            Level::Info => "INFO",
            Level::Warning => "WARNING",
            Level::Error => "ERROR",
            Level::Critical => "CRITICAL",
        }
    }
}

/// One emitted diagnostic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    /// The logger name (`"pdfminer.pdfdocument"`, the `__name__` of the
    /// module that calls `logging.getLogger(__name__)`), or, for a warning,
    /// its category (`"UserWarning"`).
    pub source: String,
    pub level: Level,
    /// The formatted message (`record.getMessage()`, `str(warning)`).
    pub message: String,
}

/// What [`collect`] gathered.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Collected {
    pub entries: Vec<Diagnostic>,
    /// Entries at or above the level that were not kept (over the limit).
    pub dropped: usize,
}

struct Sink {
    min_level: Level,
    limit: usize,
    collected: Collected,
}

thread_local! {
    static SINKS: RefCell<Vec<Sink>> = const { RefCell::new(Vec::new()) };
}

/// Whether an entry at `level` would be kept by the innermost [`collect`]
/// of this thread.
pub fn enabled(level: Level) -> bool {
    SINKS.with(|s| s.borrow().last().is_some_and(|sink| level >= sink.min_level))
}

/// Emits a diagnostic; `message` is called only when [`enabled`].
pub fn emit(source: &str, level: Level, message: impl FnOnce() -> String) {
    if !enabled(level) {
        return;
    }
    let message = message();
    SINKS.with(|s| {
        let mut sinks = s.borrow_mut();
        let Some(sink) = sinks.last_mut() else { return };
        if sink.collected.entries.len() < sink.limit {
            sink.collected.entries.push(Diagnostic { source: source.to_owned(), level, message });
        } else {
            sink.collected.dropped += 1;
        }
    });
}

/// Runs `f`, collecting the diagnostics this thread emits at or above
/// `min_level` (at most `limit` of them). Calls nest: an inner `collect`
/// takes the entries emitted while it runs.
pub fn collect<T>(min_level: Level, limit: usize, f: impl FnOnce() -> T) -> (T, Collected) {
    SINKS.with(|s| s.borrow_mut().push(Sink { min_level, limit, collected: Collected::default() }));
    // Pops the sink even if `f` panics (natively, where panics unwind).
    struct Pop;
    impl Drop for Pop {
        fn drop(&mut self) {
            SINKS.with(|s| s.borrow_mut().pop());
        }
    }
    let guard = Pop;
    let value = f();
    let collected = SINKS.with(|s| s.borrow_mut().last_mut().map(|sink| std::mem::take(&mut sink.collected)));
    drop(guard);
    let collected = collected.unwrap_or_default();
    (value, collected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_enabled_outside_collect() {
        assert!(!enabled(Level::Critical));
        let mut called = false;
        emit("x", Level::Critical, || {
            called = true;
            String::new()
        });
        assert!(!called);
    }

    #[test]
    fn collect_filters_by_level_and_bounds_the_entries() {
        let ((), got) = collect(Level::Warning, 2, || {
            emit("a", Level::Debug, || "debug".into());
            assert!(enabled(Level::Warning) && !enabled(Level::Info));
            emit("a", Level::Warning, || "one".into());
            emit("b", Level::Error, || "two".into());
            emit("b", Level::Error, || "three".into());
        });
        assert_eq!(
            got.entries,
            vec![
                Diagnostic { source: "a".into(), level: Level::Warning, message: "one".into() },
                Diagnostic { source: "b".into(), level: Level::Error, message: "two".into() },
            ]
        );
        assert_eq!(got.dropped, 1);
        assert!(!enabled(Level::Critical));
    }

    #[test]
    fn collect_nests() {
        let (inner, outer) = collect(Level::Debug, 10, || {
            emit("o", Level::Info, || "outer".into());
            let ((), inner) = collect(Level::Warning, 10, || {
                emit("i", Level::Info, || "dropped by level".into());
                emit("i", Level::Warning, || "inner".into());
            });
            inner
        });
        assert_eq!(inner.entries.len(), 1);
        assert_eq!(inner.entries[0].message, "inner");
        assert_eq!(outer.entries.len(), 1);
        assert_eq!(outer.entries[0].message, "outer");
        assert_eq!(Level::Warning.name(), "WARNING");
        assert_eq!(Level::Debug as u8, 10);
    }
}
