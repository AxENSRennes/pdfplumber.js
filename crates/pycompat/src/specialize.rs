//! The state of CPython 3.12's adaptive interpreter at a call site, where it
//! changes how many C recursion units a call takes ([`crate::recursion`]):
//! process state, like the logger caches ([`crate::logging`]) and the
//! compiled patterns ([`crate::re_cache`]).
//!
//! A code object is quickened when it is created: each `CALL` instruction
//! starts with its adaptive counter at the warm-up value (1). Its first
//! execution decrements the counter and runs the generic `CALL`; the second
//! finds the counter at zero, specializes the instruction and runs the
//! specialized form from then on (`Python/bytecodes.c`, `CALL`;
//! `Python/specialize.c`, `_Py_Specialize_Call`). For a call of the type
//! `str` with one argument, the generic path is `PyObject_Vectorcall` ->
//! `_PyObject_MakeTpCall` (`str` has no `tp_vectorcall` in 3.12), which
//! holds one C unit (`" while calling a Python object"`) for the whole
//! call, `PyObject_Str` included; the specialized `CALL_NO_KW_STR_1` calls
//! `PyObject_Str` directly and holds none. So the first `str(x)` a process
//! runs at a site raises `RecursionError` for an `x` nested one level less
//! deep than every later one (measured: `oracle/depth.py`,
//! `executed_call_sites`, reads the counter).
//!
//! Only the sites whose call can reach the C recursion limit are modeled:
//! a `str()` of an arbitrary PDF object ([`CallSite`] statics next to the
//! ported code, `pdfminer::psparser::LITERAL_NAME_STR`). Elsewhere the
//! difference is one unit for the duration of a call that does not recurse
//! (a builtin such as `isinstance`, generic before it is specialized to
//! `CALL_NO_KW_ISINSTANCE`), while the units in use stay far below the
//! limit (`recursion`, three units at most per Python frame); the calls of
//! a Python class or function are never specialized differently in 3.12
//! (`_PyObject_MakeTpCall` every time, or a frame pushed inline).
//!
//! The state is per thread, as the recursion counters are: [`reset`] is a
//! new Python process, [`set_executed`] the state a unit generator
//! recorded.

use std::cell::RefCell;

use crate::err::PyResult;
use crate::recursion::{c_call, Guard};

/// A call site of the ported code (named by its upstream `file:line` and
/// call), whose first execution in a process differs.
#[derive(Debug)]
pub struct CallSite {
    pub name: &'static str,
}

thread_local! {
    /// The sites executed at least once in this process.
    static EXECUTED: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
}

impl CallSite {
    pub const fn new(name: &'static str) -> CallSite {
        CallSite { name }
    }

    /// Whether the process has executed this site.
    pub fn executed(&self) -> bool {
        EXECUTED.with(|e| e.borrow().contains(&self.name))
    }

    /// The C unit of a one-argument call of `str` at this site, held by
    /// the caller for the duration of the call: the generic `CALL`'s
    /// (`_PyObject_MakeTpCall`) the first time the process executes the
    /// site, none once it is specialized (`CALL_NO_KW_STR_1`). The site
    /// counts as executed even when the call raises: the counter is
    /// decremented before the call.
    pub fn str_call(&self) -> PyResult<Option<Guard>> {
        if self.executed() {
            return Ok(None);
        }
        set_executed(self.name, true);
        c_call(" while calling a Python object").map(Some)
    }
}

/// A new process: no site executed.
pub fn reset() {
    EXECUTED.with(|e| e.borrow_mut().clear());
}

/// Records whether the site named `name` was executed.
pub fn set_executed(name: &'static str, executed: bool) {
    EXECUTED.with(|e| {
        let mut e = e.borrow_mut();
        e.retain(|n| *n != name);
        if executed {
            e.push(name);
        }
    });
}

/// The sites executed so far (a snapshot of the state, see [`restore`]).
pub fn snapshot() -> Vec<&'static str> {
    EXECUTED.with(|e| e.borrow().clone())
}

/// Puts back a [`snapshot`].
pub fn restore(executed: Vec<&'static str>) {
    EXECUTED.with(|e| *e.borrow_mut() = executed);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recursion::{with_depth, Depth, C_RECURSION_LIMIT};

    static SITE: CallSite = CallSite::new("test.py:1 str(x)");

    #[test]
    fn the_first_execution_takes_a_unit() {
        reset();
        with_depth(Depth { py: 5, c: C_RECURSION_LIMIT }, || {
            let err = SITE.str_call().err().expect("no unit left for the generic call");
            assert_eq!(err.message, "maximum recursion depth exceeded while calling a Python object");
            assert!(SITE.executed(), "executed even though it raised");
            assert!(SITE.str_call().expect("specialized").is_none());
        });
        reset();
        with_depth(Depth { py: 5, c: C_RECURSION_LIMIT - 1 }, || {
            let unit = SITE.str_call().expect("one unit left").expect("generic");
            assert_eq!(crate::recursion::current().c, C_RECURSION_LIMIT);
            drop(unit);
        });
        let saved = snapshot();
        reset();
        assert!(!SITE.executed());
        restore(saved);
        assert!(SITE.executed());
        set_executed(SITE.name, false);
        assert!(!SITE.executed());
    }
}
