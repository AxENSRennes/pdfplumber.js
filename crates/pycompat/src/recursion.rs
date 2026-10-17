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

/// `n` nested frames of calls that return at once (a leaf function and
/// the ones it calls): only checked, as the call would raise
/// `RecursionError` before doing anything.
pub fn leaf(n: u32) -> PyResult<()> {
    drop(py_frames(n)?);
    Ok(())
}

/// A leaf Python function called from C (a class's `__init__` run by
/// `type.__call__`, a lambda run by `re.sub`): the evaluation loop entry
/// and its frame, only checked.
pub fn c_called_leaf() -> PyResult<()> {
    let _entry = eval_entry()?;
    leaf(1)
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

#[cfg(test)]
mod tests {
    use super::*;

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
