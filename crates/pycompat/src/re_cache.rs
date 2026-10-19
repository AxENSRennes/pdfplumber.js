//! The Python frames of a call into CPython 3.12's `re` module with a
//! pattern string, which depend on the module's cache of compiled patterns
//! (`Lib/re/__init__.py`, `_compile`): process state that decides where a
//! `RecursionError` happens in a deep recursion, as the logger caches do
//! ([`crate::logging`]).
//!
//! `re.finditer(pattern, string)` is a Python function that calls
//! `_compile(pattern, flags)`: two frames below the caller when the pattern
//! is in `_cache2` (or in `_cache`, from which `_compile` moves it back to
//! `_cache2`). The first call of a process with a pattern compiles it
//! (`_compiler.compile`, `_parser.parse`...), deeper; the depth of that
//! compilation is the pattern's, measured by the caller's oracle (for
//! pdfminer's `PDFLayoutAnalyzer.paint_path`, `oracle/unit/converter.py`).
//! A compilation interrupted by a `RecursionError` caches nothing.
//! Eviction (more than 256 then 512 distinct patterns in a process) is not
//! modeled: pdfminer and pdfplumber use far fewer.
//!
//! The cache is per thread, as the recursion counters are; [`reset`] is a
//! new Python process (whose imports compiled none of the patterns the
//! port asks for) and [`set_compiled`] the state a unit generator recorded.

use std::cell::RefCell;

use crate::err::PyResult;
use crate::recursion::leaf;

thread_local! {
    /// The patterns compiled so far.
    static COMPILED: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
}

/// A new process: no pattern compiled.
pub fn reset() {
    COMPILED.with(|c| c.borrow_mut().clear());
}

/// The patterns compiled so far: a snapshot of the state, to put back with
/// [`restore`].
pub fn snapshot() -> Vec<&'static str> {
    COMPILED.with(|c| c.borrow().clone())
}

/// Puts back a [`snapshot`].
pub fn restore(compiled: Vec<&'static str>) {
    COMPILED.with(|c| *c.borrow_mut() = compiled);
}

/// Whether `pattern` is compiled (in `re`'s caches).
pub fn is_compiled(pattern: &str) -> bool {
    COMPILED.with(|c| c.borrow().contains(&pattern))
}

/// Records whether `pattern` is compiled.
pub fn set_compiled(pattern: &'static str, compiled: bool) {
    COMPILED.with(|c| {
        let mut c = c.borrow_mut();
        c.retain(|p| *p != pattern);
        if compiled {
            c.push(pattern);
        }
    });
}

/// The frames of `re.finditer(pattern, ...)` (or another function that
/// calls `_compile` the same way): `re.finditer` and `_compile`, two frames
/// below the caller, when `pattern` is compiled; else the compilation,
/// `compile_depth` frames below the caller at its deepest, after which the
/// pattern is compiled. Raises `RecursionError` when they do not fit.
pub fn compile_frames(pattern: &'static str, compile_depth: u32) -> PyResult<()> {
    if is_compiled(pattern) {
        return leaf(2);
    }
    leaf(compile_depth)?;
    set_compiled(pattern, true);
    Ok(())
}
