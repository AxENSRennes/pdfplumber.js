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
//! A `LOAD_ATTR` of a property is the same story with two units
//! ([`AttrSite`]): the generic instruction finds the property through
//! `PyObject_GetAttr`, whose `property.__get__` calls the getter from C, an
//! entry of the evaluation loop (`PY_EVAL_C_STACK_UNITS`, two units held
//! while the getter runs); the specialized `LOAD_ATTR_PROPERTY` pushes the
//! getter's frame inline. A property's getter can run the deepest code of
//! pdfplumber (`Page.layout` interprets the page under `page.chars` ->
//! `Page.objects` -> `parse_objects`), so every property load of the
//! ported pdfplumber code is a site, with the adaptive counter of
//! CPython 3.12 (`Include/internal/pycore_code.h`; `Python/bytecodes.c`,
//! `LOAD_ATTR`, `LOAD_ATTR_PROPERTY`; `Python/specialize.c`,
//! `_Py_Specialize_LoadAttr`): the specialized form keeps the class it was
//! specialized for, a load on another class (`Container.chars` of a `Page`,
//! then of a `CroppedPage`) deoptimizes to the generic one, and the
//! instruction specializes again for the new class after 52 such misses.
//!
//! The state is per thread, as the recursion counters are: [`reset`] is a
//! new Python process, [`set_executed`] the state a unit generator
//! recorded.

use std::cell::RefCell;

use crate::err::PyResult;
use crate::recursion::{c_call, eval_entry, Guard};

/// A call site of the ported code (named by its upstream `file:line` and
/// call), whose first execution in a process differs.
#[derive(Debug)]
pub struct CallSite {
    pub name: &'static str,
}

thread_local! {
    /// The sites executed at least once in this process.
    static EXECUTED: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
    /// The state of each [`AttrSite`] executed in this process.
    static ATTRS: RefCell<Vec<(&'static str, AttrState)>> = const { RefCell::new(Vec::new()) };
}

/// The adaptive counter of a quickened instruction, 3.12
/// (`Include/internal/pycore_code.h`): a value, decremented by each
/// execution of the generic (adaptive) form, which tries to specialize
/// when it is zero, and the exponent of its backoff.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AttrState {
    value: u16,
    backoff: u8,
    /// The class the instruction is specialized for (`LOAD_ATTR_PROPERTY`
    /// caches its type version), `None` for the generic form.
    specialized: Option<&'static str>,
}

/// `adaptive_counter_warmup()`: what `_PyCode_Quicken` gives each
/// instruction (`ADAPTIVE_WARMUP_VALUE` 1, `ADAPTIVE_WARMUP_BACKOFF` 1).
const WARMUP: AttrState = AttrState { value: 1, backoff: 1, specialized: None };
/// `adaptive_counter_cooldown()`: after a successful specialization
/// (`ADAPTIVE_COOLDOWN_VALUE` 52, backoff 0).
const COOLDOWN_VALUE: u16 = 52;
/// `MAX_BACKOFF_VALUE`: `16 - ADAPTIVE_BACKOFF_BITS`.
const MAX_BACKOFF: u8 = 12;

/// A `LOAD_ATTR` instruction of the ported code (named by its upstream
/// `file:line` and expression): `owner.attr`, whose attribute is a property
/// of the owner's class (or, at a few sites, a plain class attribute for
/// some classes: [`AttrSite::plain`]).
#[derive(Debug)]
pub struct AttrSite {
    pub name: &'static str,
}

impl AttrSite {
    pub const fn new(name: &'static str) -> AttrSite {
        AttrSite { name }
    }

    // The state is found by the address of the site's name (each site's
    // name is its own literal): a load is on the hot path of
    // `process_object`, once or twice per object of a page.
    fn state(&self) -> AttrState {
        ATTRS.with(|a| a.borrow().iter().find(|(n, _)| std::ptr::eq(*n, self.name)).map_or(WARMUP, |(_, s)| *s))
    }

    fn set_state(&self, state: AttrState) {
        ATTRS.with(|a| {
            let mut a = a.borrow_mut();
            match a.iter_mut().find(|(n, _)| std::ptr::eq(*n, self.name)) {
                Some((_, s)) => *s = state,
                None => a.push((self.name, state)),
            }
        });
    }

    /// The load of a property of `class` whose getter is a Python
    /// function: the C units the caller holds while the getter runs, those
    /// of the evaluation loop entry when the generic instruction runs
    /// (`property.__get__` calls the getter from C), none when the
    /// instruction is specialized for `class` (`LOAD_ATTR_PROPERTY`). The
    /// counter moves before the getter runs (it counts even when the
    /// getter raises).
    pub fn property(&self, class: &'static str) -> PyResult<Option<Guard>> {
        let mut st = self.state();
        if st.specialized == Some(class) {
            return Ok(None);
        }
        // LOAD_ATTR: the generic form, or the deoptimized specialized one
        if st.value == 0 {
            // _Py_Specialize_LoadAttr succeeds for a property whose getter
            // is a Python function taking one argument; DISPATCH_SAME_OPARG
            // then runs LOAD_ATTR_PROPERTY
            st = AttrState { value: COOLDOWN_VALUE, backoff: 0, specialized: Some(class) };
            self.set_state(st);
            return Ok(None);
        }
        st.value -= 1;
        self.set_state(st);
        eval_entry().map(Some)
    }

    /// `owner.attr` for a property of `class`: `getter` runs with the units
    /// of the load ([`AttrSite::property`]) held.
    pub fn get<T>(&self, class: &'static str, getter: impl FnOnce() -> PyResult<T>) -> PyResult<T> {
        let _units = self.property(class)?;
        getter()
    }

    /// The load of an attribute of `class` that is no property (a class
    /// attribute, `Page.pages = None`): no getter runs; the instruction's
    /// counter moves, and an attempt to specialize fails in 3.12 (a
    /// non-descriptor class attribute read through an instance), which
    /// backs the counter off (`adaptive_counter_backoff`) and runs the
    /// generic form.
    pub fn plain(&self, _class: &'static str) {
        let mut st = self.state();
        if st.value == 0 {
            let backoff = (st.backoff + 1).min(MAX_BACKOFF);
            st = AttrState { value: (1 << backoff) - 1, backoff, specialized: None };
        }
        st.value -= 1;
        self.set_state(st);
    }
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

    /// The C unit of a call of a builtin function at this site (`getattr`,
    /// `hasattr`...: `METH_FASTCALL` functions), held by the caller for the
    /// duration of the call: the generic `CALL`'s
    /// (`cfunction_vectorcall_FASTCALL` -> `cfunction_enter_call`) the
    /// first time the process executes the site, none once it is
    /// specialized (`CALL_NO_KW_BUILTIN_FAST` calls the C function
    /// directly). The same counter as [`CallSite::str_call`].
    pub fn builtin_call(&self) -> PyResult<Option<Guard>> {
        self.str_call()
    }

    /// The C units of a `LOAD_ATTR` of a property at this site, held by
    /// the caller while the getter runs: the generic instruction's the
    /// first time the process executes the site (`PyObject_GetAttr` ->
    /// `property.__get__` calls the getter from C, an entry of the
    /// evaluation loop: two units), none once it is specialized
    /// (`LOAD_ATTR_PROPERTY` pushes the getter's frame inline).
    pub fn property_load(&self) -> PyResult<Option<Guard>> {
        if self.executed() {
            return Ok(None);
        }
        set_executed(self.name, true);
        eval_entry().map(Some)
    }
}

/// A new process: no site executed.
pub fn reset() {
    EXECUTED.with(|e| e.borrow_mut().clear());
    ATTRS.with(|a| a.borrow_mut().clear());
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

/// The state of every site (see [`snapshot`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    executed: Vec<&'static str>,
    attrs: Vec<(&'static str, AttrState)>,
}

/// The state of the sites so far (a snapshot, see [`restore`]).
pub fn snapshot() -> Snapshot {
    Snapshot { executed: EXECUTED.with(|e| e.borrow().clone()), attrs: ATTRS.with(|a| a.borrow().clone()) }
}

/// Puts back a [`snapshot`].
pub fn restore(snapshot: Snapshot) {
    EXECUTED.with(|e| *e.borrow_mut() = snapshot.executed);
    ATTRS.with(|a| *a.borrow_mut() = snapshot.attrs);
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

    static ATTR: AttrSite = AttrSite::new("test.py:2 self.objects");
    static PLAIN: AttrSite = AttrSite::new("test.py:3 self.pages");

    /// The units of a load: `Some(2)` for the generic form.
    fn units(site: &AttrSite, class: &'static str) -> Option<u32> {
        with_depth(Depth { py: 5, c: 0 }, || site.property(class).unwrap().map(|_| crate::recursion::current().c))
    }

    #[test]
    fn a_property_load_specializes_on_its_second_execution_and_deoptimizes_for_another_class() {
        reset();
        // first execution: generic, the getter from C (two units)
        assert_eq!(units(&ATTR, "Page"), Some(2));
        // second: the counter is zero, the instruction specializes for Page
        assert_eq!(units(&ATTR, "Page"), None);
        assert_eq!(units(&ATTR, "Page"), None);
        // another class: 52 misses run the generic form, then it
        // specializes for that class
        for _ in 0..52 {
            assert_eq!(units(&ATTR, "CroppedPage"), Some(2));
        }
        assert_eq!(units(&ATTR, "CroppedPage"), None);
        assert_eq!(units(&ATTR, "Page"), Some(2), "Page now misses");
        let saved = snapshot();
        reset();
        assert_eq!(units(&ATTR, "CroppedPage"), Some(2), "a new process");
        restore(saved.clone());
        assert_eq!(snapshot(), saved);
        assert_eq!(units(&ATTR, "CroppedPage"), None);
        // at the limit, the generic load raises, and still counts
        reset();
        with_depth(Depth { py: 5, c: C_RECURSION_LIMIT - 1 }, || assert!(ATTR.property("Page").is_err()));
        assert_eq!(units(&ATTR, "Page"), None);
    }

    #[test]
    fn a_plain_attribute_backs_the_counter_off() {
        reset();
        PLAIN.plain("Page"); // 1 -> 0
        PLAIN.plain("Page"); // zero: the attempt fails, backoff 2: 3, then 2
        PLAIN.plain("Page"); // 1
        PLAIN.plain("Page"); // 0
                             // a property of another class at the same site: the counter is
                             // zero, it specializes
        assert_eq!(units(&PLAIN, "PDF"), None);
        reset();
        PLAIN.plain("Page");
        assert_eq!(units(&PLAIN, "PDF"), None, "the counter reached zero: specializes");
        reset();
        for _ in 0..5 {
            PLAIN.plain("Page");
        }
        // 0 -> fail (backoff 2: 3 - 1 = 2), 1, 0, fail (backoff 3: 7 - 1 = 6)
        for _ in 0..6 {
            assert_eq!(units(&PLAIN, "PDF"), Some(2));
        }
        assert_eq!(units(&PLAIN, "PDF"), None);
    }
}
