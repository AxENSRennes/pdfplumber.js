//! CPython 3.12's two recursion limits, where the ported code recurses.
//!
//! CPython raises `RecursionError` from two counters of the running thread
//! (`Python/ceval.c`, `Include/internal/pycore_ceval.h`):
//!
//! - the Python frames on the stack, at most `sys.getrecursionlimit()`
//!   ([`PY_RECURSION_LIMIT`]): pushing one more frame raises
//!   `RecursionError("maximum recursion depth exceeded")`;
//! - C-level recursion units, at most `Py_C_RECURSION_LIMIT`
//!   ([`C_RECURSION_LIMIT`]): `repr()`, `str()`, rich comparison and the
//!   other C functions that may recurse take one unit for their duration
//!   (`Py_EnterRecursiveCall(" while getting the repr of an object")`...),
//!   and every entry of the evaluation loop from C (a Python function called
//!   by C code, such as a `__repr__` method, or a generator resumed by
//!   `list()`) takes two. Running out raises `RecursionError("maximum
//!   recursion depth exceeded" + where)`. A Python-to-Python call takes no
//!   unit.
//!
//! [`Depth`] is the state of both counters. The port keeps it per thread
//! and counts what the ported Python code would count: a ported Python
//! function enters a [`py_frame`] for its duration, a ported C operation a
//! [`c_call`], and a call from C into Python code (a `__repr__` method run
//! by `repr()`, a function run by `f(*args)`) an [`eval_entry`] before the
//! callee's frame; a guard leaves on drop, as CPython leaves on return or
//! on error. Calling a builtin function (`repr(x)`, `len(x)`) takes a unit
//! too (`" while calling a Python object"`), but neither `str(x)` (a type)
//! nor the `!r` of an f-string, which call `PyObject_Str`/`PyObject_Repr`
//! directly. So a
//! `RecursionError` happens at the same place as in Python, provided the
//! starting depth is Python's: the harness sets it with [`with_depth`] to
//! the depth the oracle measured where it calls the same code
//! (`oracle/stages/__init__.py` `entry_depth`, and the unit generators,
//! which record the depth of their call sites). The ported code itself
//! never sets it.
//!
//! The limits are those of CPython 3.12 release builds on Linux and macOS
//! (`Py_C_RECURSION_LIMIT` is lower on Windows, WASI and in debug builds);
//! `oracle/unit/pycompat.py` checks both against the oracle.

use std::cell::Cell;
use std::marker::PhantomData;

use crate::err::{exc, PyErr, PyResult};

/// `sys.getrecursionlimit()` (default, never changed by pdfminer or pdfplumber).
pub const PY_RECURSION_LIMIT: u32 = 1000;

/// `Py_C_RECURSION_LIMIT` of CPython 3.12 (`Include/cpython/pystate.h`).
pub const C_RECURSION_LIMIT: u32 = 10000;

/// `PY_EVAL_C_STACK_UNITS`: the C units an evaluation loop entry takes.
const EVAL_C_UNITS: u32 = 2;

/// The use of both counters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Depth {
    /// Python frames on the stack (`limit - py_recursion_remaining`).
    pub py: u32,
    /// C units in use (`Py_C_RECURSION_LIMIT - c_recursion_remaining`).
    pub c: u32,
}

thread_local! {
    static DEPTH: Cell<Depth> = const { Cell::new(Depth { py: 0, c: 0 }) };
}

/// The current depth of this thread.
pub fn current() -> Depth {
    DEPTH.with(Cell::get)
}

/// Runs `f` with the counters starting at `base` (where the oracle calls the
/// same code), then restores them.
pub fn with_depth<R>(base: Depth, f: impl FnOnce() -> R) -> R {
    struct Restore(Depth);
    impl Drop for Restore {
        fn drop(&mut self) {
            DEPTH.with(|d| d.set(self.0));
        }
    }
    let _restore = Restore(DEPTH.with(|d| d.replace(base)));
    f()
}

fn recursion_error(what: &str) -> PyErr {
    exc::RecursionError(format!("maximum recursion depth exceeded{what}"))
}

/// Units held until dropped (see the module documentation). Not `Send`:
/// the counters belong to the thread.
#[must_use = "the frame or call ends when the guard is dropped"]
pub struct Guard {
    py: u32,
    c: u32,
    _thread: PhantomData<*const ()>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        DEPTH.with(|d| {
            let Depth { py, c } = d.get();
            debug_assert!(py >= self.py && c >= self.c, "a recursion guard outlived its with_depth()");
            d.set(Depth { py: py.saturating_sub(self.py), c: c.saturating_sub(self.c) });
        });
    }
}

fn take(py: u32, c: u32) -> Guard {
    DEPTH.with(|d| {
        let now = d.get();
        d.set(Depth { py: now.py + py, c: now.c + c });
    });
    Guard { py, c, _thread: PhantomData }
}

/// A Python frame (`_Py_EnterRecursivePy`): raises `RecursionError` when the
/// stack already has [`PY_RECURSION_LIMIT`] frames.
pub fn py_frame() -> PyResult<Guard> {
    if current().py >= PY_RECURSION_LIMIT {
        return Err(recursion_error(""));
    }
    Ok(take(1, 0))
}

/// `n` nested Python frames entered at once, as a helper function and the
/// ones it calls (`Logger.debug` -> `Logger.isEnabledFor`): raises if any
/// of them would exceed the limit.
pub fn py_frames(n: u32) -> PyResult<Guard> {
    if current().py + n > PY_RECURSION_LIMIT {
        return Err(recursion_error(""));
    }
    Ok(take(n, 0))
}

/// `Py_EnterRecursiveCall(what)`: one C unit, or `RecursionError` with
/// `what` appended to the message (`" while getting the repr of an
/// object"`) when all are in use.
pub fn c_call(what: &str) -> PyResult<Guard> {
    if current().c >= C_RECURSION_LIMIT {
        return Err(recursion_error(what));
    }
    Ok(take(0, 1))
}

/// The evaluation loop entered from C (`_PyEval_EvalFrameDefault`, to run
/// a Python function called by C code): two C units, before the function
/// takes its frame ([`py_frame`]). Raises `RecursionError("maximum
/// recursion depth exceeded")` when fewer than two units are left.
pub fn eval_entry() -> PyResult<Guard> {
    if current().c + 1 >= C_RECURSION_LIMIT {
        return Err(recursion_error(""));
    }
    Ok(take(0, EVAL_C_UNITS))
}

// ---------------------------------------------------------------------------
// logging calls

/// `logging.DEBUG`
pub const DEBUG: u32 = 10;
/// `logging.WARNING`
pub const WARNING: u32 = 30;

thread_local! {
    /// `Logger._cache` of each logger: the (logger name, level) pairs whose
    /// answer is cached.
    static LOGGER_CACHE: std::cell::RefCell<Vec<(&'static str, u32)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// `logging.disable(...)`: clears the cache of every logger (the next call
/// of each level on each logger takes the third frame again). The oracle
/// runs it before a stage or a unit case.
pub fn clear_logger_cache() {
    LOGGER_CACHE.with(|c| c.borrow_mut().clear());
}

/// The Python frames of `log.<level>(...)` on the logger `logger`
/// (`"pdfminer.cmapdb"`) while the level is disabled (`docs/verification.md`,
/// "Étapes": the reference is `logging.disable(logging.CRITICAL)`, as the
/// oracle runs): `Logger.<level>` and `Logger.isEnabledFor`, two frames
/// below the caller; three the first time the level is asked of the logger
/// since its cache was cleared (`isEnabledFor` fills `Logger._cache` under
/// `_acquireLock`). The cache is filled only when the third frame was
/// entered.
///
/// An enabled level costs more: at Python's default configuration (no
/// handler, level `WARNING`), `log.debug` costs the same, but `log.warning`
/// runs `_log`, `findCaller`, `makeRecord` and the `lastResort` handler,
/// whose formatting of the arguments may go deeper still (12 frames below
/// the caller of `PDFFont._parse_bbox`'s warning, against 4 disabled):
/// not modeled, the port emits no log records.
pub fn log_frames(logger: &'static str, level: u32) -> PyResult<()> {
    let cached = LOGGER_CACHE.with(|c| c.borrow().contains(&(logger, level)));
    if cached {
        drop(py_frames(2)?);
    } else {
        drop(py_frames(3)?);
        LOGGER_CACHE.with(|c| c.borrow_mut().push((logger, level)));
    }
    Ok(())
}

/// The effect of `log.<level>(...)` on the cache of `logger` alone, for a
/// logging call whose frames the caller does not count yet (`psparser`'s
/// `log.debug` in `PSBaseParser.seek`): the next [`log_frames`] of that
/// level on that logger takes two frames, as in Python after any such call.
pub fn fill_logger_cache(logger: &'static str, level: u32) {
    LOGGER_CACHE.with(|c| {
        let mut cache = c.borrow_mut();
        if !cache.contains(&(logger, level)) {
            cache.push((logger, level));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_frames_and_the_logger_cache() {
        clear_logger_cache();
        with_depth(Depth { py: PY_RECURSION_LIMIT - 2, c: 0 }, || {
            // the first call needs three frames, and fails without filling
            // the cache
            assert!(log_frames("a", DEBUG).is_err());
        });
        with_depth(Depth { py: PY_RECURSION_LIMIT - 3, c: 0 }, || {
            log_frames("a", DEBUG).expect("three frames left");
            assert_eq!(current().py, PY_RECURSION_LIMIT - 3);
        });
        with_depth(Depth { py: PY_RECURSION_LIMIT - 2, c: 0 }, || {
            log_frames("a", DEBUG).expect("cached: two frames");
            // per logger and per level
            assert!(log_frames("a", WARNING).is_err());
            assert!(log_frames("b", DEBUG).is_err());
            clear_logger_cache();
            assert!(log_frames("a", DEBUG).is_err());
            // a call whose frames are not counted fills the cache all the same
            fill_logger_cache("a", DEBUG);
            fill_logger_cache("a", DEBUG);
            log_frames("a", DEBUG).expect("cached: two frames");
            assert!(log_frames("a", WARNING).is_err());
        });
        clear_logger_cache();
    }

    #[test]
    fn frames_up_to_the_limit() {
        with_depth(Depth { py: PY_RECURSION_LIMIT - 2, c: 0 }, || {
            let a = py_frame().expect("999th frame");
            let b = py_frame().expect("1000th frame");
            let err = py_frame().err().expect("1001st frame");
            assert_eq!(err.message, "maximum recursion depth exceeded");
            drop((a, b));
            assert_eq!(current().py, PY_RECURSION_LIMIT - 2);
            assert!(py_frames(3).is_err());
            assert!(py_frames(2).is_ok());
        });
        assert_eq!(current(), Depth::default());
    }

    #[test]
    fn c_units_and_eval_entries() {
        with_depth(Depth { py: 5, c: C_RECURSION_LIMIT - 2 }, || {
            // an evaluation loop entry needs two units
            let entry = eval_entry().expect("two units left");
            assert_eq!(current(), Depth { py: 5, c: C_RECURSION_LIMIT });
            drop(entry);
            let one = c_call(" while getting the repr of an object").expect("one unit left");
            assert_eq!(eval_entry().err().expect("one unit left").message, "maximum recursion depth exceeded");
            let _last = c_call("").expect("the last unit");
            let err = c_call(" while getting the repr of an object").err().expect("none left");
            assert_eq!(err.message, "maximum recursion depth exceeded while getting the repr of an object");
            drop(one);
        });
    }
}
