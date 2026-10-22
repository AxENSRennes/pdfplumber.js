//! `warnings.warn` under CPython 3.12's default filters
//! (`Lib/warnings.py`, `warn_explicit`): a category none of the default
//! filters names (`UserWarning`...) takes the `"default"` action, which
//! shows a warning the first time its key — its text, its category and
//! the line that issued it — is seen, and records the key in the
//! `__warningregistry__` of the issuing module: the same warning from the
//! same line is shown once per process. The registry is process state
//! (`pdfminer::process_state`): a call the JavaScript boundary voids puts
//! it back. The filters never change in the port (the registry would be
//! cleared when they do).
//!
//! A shown warning goes to the diagnostics channel ([`crate::diagnostics`],
//! source: the category's name). The Python frames of `warnings.warn` are
//! not modelled (`docs/architecture.md`, "Avertissements").

use std::cell::RefCell;

use crate::diagnostics::{self, Level};

/// A registry key: the category, the line that issued the warning
/// (module and line: `"pdfplumber.page:305"`) and the text.
pub type Key = (&'static str, &'static str, String);

thread_local! {
    /// The keys shown so far (the `__warningregistry__` of every module).
    static REGISTRY: RefCell<Vec<Key>> = const { RefCell::new(Vec::new()) };
}

/// `warnings.warn(message, category)` issued at `location`: shown (sent to
/// the diagnostics channel) unless the same key was shown before.
pub fn warn(category: &'static str, location: &'static str, message: String) {
    let new = REGISTRY.with(|r| {
        let mut r = r.borrow_mut();
        if r.iter().any(|(c, l, m)| *c == category && *l == location && *m == message) {
            return None;
        }
        r.push((category, location, message.clone()));
        Some(message)
    });
    if let Some(message) = new {
        diagnostics::emit(category, Level::Warning, || message);
    }
}

/// A new process: nothing shown yet.
pub fn reset() {
    REGISTRY.with(|r| r.borrow_mut().clear());
}

/// The registry: a snapshot of the state, to put back with [`restore`].
pub fn snapshot() -> Vec<Key> {
    REGISTRY.with(|r| r.borrow().clone())
}

/// Puts back a [`snapshot`].
pub fn restore(keys: Vec<Key>) {
    REGISTRY.with(|r| *r.borrow_mut() = keys);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_warning_is_shown_once_per_text_category_and_line() {
        reset();
        let ((), shown) = diagnostics::collect(Level::Debug, 10, || {
            warn("UserWarning", "a.py:1", "x".to_owned());
            warn("UserWarning", "a.py:1", "x".to_owned());
            warn("UserWarning", "a.py:1", "y".to_owned());
            warn("UserWarning", "a.py:2", "x".to_owned());
            warn("RuntimeWarning", "a.py:1", "x".to_owned());
        });
        let texts: Vec<(&str, &str)> = shown.entries.iter().map(|d| (d.source.as_str(), d.message.as_str())).collect();
        assert_eq!(texts, [("UserWarning", "x"), ("UserWarning", "y"), ("UserWarning", "x"), ("RuntimeWarning", "x")]);
        assert_eq!(snapshot().len(), 4);
        let shown = snapshot();
        // a new process has shown nothing
        reset();
        assert!(snapshot().is_empty());
        restore(shown);
        let (_, collected) = diagnostics::collect(Level::Debug, 10, || warn("UserWarning", "a.py:1", "x".to_owned()));
        assert!(collected.entries.is_empty());
        restore(Vec::new());
        let (_, collected) = diagnostics::collect(Level::Debug, 10, || warn("UserWarning", "a.py:1", "x".to_owned()));
        assert_eq!(collected.entries.len(), 1);
        reset();
    }
}
