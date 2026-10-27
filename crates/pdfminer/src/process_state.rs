//! Not a pdfminer module: the state of the Python process that the port
//! models because it decides how deep a later call goes, so where a
//! `RecursionError` happens (`docs/architecture.md`, "Recursion Depth",
//! "Process State"), taken and put back as a whole.
//!
//! A Python process remembers what it has done: the symbols it interned
//! (`psparser::py_intern`), the patterns `re` compiled
//! (`pycompat::re_cache`), the subscriptions of `typing` aliases cached
//! (`pycompat::typing_cache`), the logger caches (`pycompat::logging`), the
//! call sites the adaptive interpreter specialized (`pycompat::specialize`)
//! and the CMaps `CMapDB` loaded (`cmapdb`). Each is per thread in the
//! port, as the recursion counters are.
//!
//! The JavaScript boundary voids a call that asked for a predefined CMap
//! not handed over yet and runs it again (`crates/wasm/src/cmaps.rs`): the
//! re-run must start from the process state the voided call started from,
//! or it would find, say, a pattern compiled or a name interned that
//! Python's single run with every CMap installed compiles or interns at a
//! deeper point. [`ProcessState::capture`] before the call and
//! [`ProcessState::restore`] when it is void give that.

use std::cell::RefCell;
use std::rc::Rc;

use crate::cmapdb::{CMapDB, CMapDBCaches};
use crate::psparser::{py_symbols_restore, py_symbols_snapshot, PyInterned};

/// The modeled state of the process (see the module documentation).
///
/// A snapshot is cheap to keep: the interned symbols are shared with the
/// other snapshots ([`PyInterned`]), and the rest, whose size is bounded
/// by the port's static sites, is shared with the previous snapshot when
/// it did not change. The boundary keeps the state each call started
/// from (`crates/wasm/src/ops.rs`, "Void calls").
#[derive(Clone)]
pub struct ProcessState {
    interned: PyInterned,
    rest: Rc<Rest>,
}

/// The parts of a [`ProcessState`] besides the interned symbols.
struct Rest {
    compiled: Vec<&'static str>,
    subscriptions: Vec<&'static str>,
    loggers: pycompat::logging::LoggerCaches,
    call_sites: pycompat::specialize::Snapshot,
    cmaps: CMapDBCaches,
    warnings: Vec<pycompat::warnings::Key>,
}

impl Rest {
    fn same(&self, other: &Rest) -> bool {
        self.compiled == other.compiled
            && self.subscriptions == other.subscriptions
            && self.loggers == other.loggers
            && self.call_sites == other.call_sites
            && self.cmaps.same(&other.cmaps)
            && self.warnings == other.warnings
    }
}

thread_local! {
    /// The [`Rest`] of the last snapshot, which the next one shares when
    /// nothing changed.
    static LAST: RefCell<Option<Rc<Rest>>> = const { RefCell::new(None) };
}

impl ProcessState {
    /// The state of this thread's process now.
    pub fn capture() -> ProcessState {
        let rest = Rest {
            compiled: pycompat::re_cache::snapshot(),
            subscriptions: pycompat::typing_cache::snapshot(),
            loggers: pycompat::logging::snapshot(),
            call_sites: pycompat::specialize::snapshot(),
            cmaps: CMapDB::caches_snapshot(),
            warnings: pycompat::warnings::snapshot(),
        };
        let rest = LAST.with(|last| {
            let mut last = last.borrow_mut();
            match &*last {
                Some(r) if r.same(&rest) => r.clone(),
                _ => {
                    let r = Rc::new(rest);
                    *last = Some(r.clone());
                    r
                }
            }
        });
        ProcessState { interned: py_symbols_snapshot(), rest }
    }

    /// Puts this state back.
    pub fn restore(self) {
        py_symbols_restore(self.interned);
        let rest = &*self.rest;
        pycompat::re_cache::restore(rest.compiled.clone());
        pycompat::typing_cache::restore(rest.subscriptions.clone());
        pycompat::logging::restore(rest.loggers.clone());
        pycompat::specialize::restore(rest.call_sites.clone());
        CMapDB::restore_caches(rest.cmaps.clone());
        pycompat::warnings::restore(rest.warnings.clone());
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use pycompat::logging::Logger;
    use pycompat::recursion::{with_depth, Depth, C_RECURSION_LIMIT, PY_RECURSION_LIMIT};
    use pycompat::specialize::CallSite;

    use super::*;
    use crate::cmapdb::CMapProvider;
    use crate::psparser::{py_intern, Symbol, LIT};

    static LOG: Logger = Logger::new("process_state.test");
    static SITE: CallSite = CallSite::new("process_state.test str(x)");
    const PATTERN: &str = "process_state test";
    static SUBSCRIPTION: pycompat::typing_cache::Subscription =
        pycompat::typing_cache::Subscription::new("process_state[test]", 6, 1);

    /// Serves a compact horizontal CMap with an empty `CODE2CID` for every
    /// name.
    struct Empty;
    impl CMapProvider for Empty {
        fn load(&self, _name: &str) -> Option<Vec<u8>> {
            Some(b"PMCM\x01\x02\x0bIS_VERTICALB\x00\x08CODE2CIDC\x00".to_vec())
        }
    }

    /// Whether each part of the state holds what `done_once` did, read from
    /// how deep the same calls go (one frame or unit more the first time)
    /// and from `CMapDB`'s cache.
    fn parts_done(symbol: Symbol) -> [bool; 6] {
        let at = |py: u32, c: u32, f: &mut dyn FnMut() -> bool| with_depth(Depth { py, c }, f);
        [
            at(PY_RECURSION_LIMIT - 1, 0, &mut || py_intern(symbol).is_ok()),
            at(PY_RECURSION_LIMIT - 2, 0, &mut || LOG.warning().is_ok()),
            at(PY_RECURSION_LIMIT - 3, 0, &mut || pycompat::re_cache::compile_frames(PATTERN, 5).is_ok()),
            at(0, C_RECURSION_LIMIT, &mut || SITE.str_call().is_ok()),
            CMapDB::caches_snapshot().names().0 == ["process_state test"],
            at(PY_RECURSION_LIMIT - 5, 0, &mut || SUBSCRIPTION.eval().is_ok()),
        ]
    }

    #[test]
    fn restore_puts_back_every_part() {
        CMapDB::set_provider(Some(Rc::new(Empty)));
        let symbol = Symbol::Literal(LIT("process_state test"));
        let fresh = ProcessState::capture();
        // each done once, shallow
        let done_once = || {
            py_intern(symbol).unwrap();
            LOG.warning().unwrap();
            pycompat::re_cache::compile_frames(PATTERN, 5).unwrap();
            drop(SITE.str_call().unwrap());
            CMapDB::get_cmap("process_state test").unwrap();
            SUBSCRIPTION.eval().unwrap();
        };
        done_once();
        let done = ProcessState::capture();
        fresh.clone().restore();
        assert_eq!(parts_done(symbol), [false; 6], "the fresh state, every part");
        // (parts_done did them, at depth, failing: still not done but for
        // the call site, executed even when its call raises)
        fresh.restore();
        done.restore();
        assert_eq!(parts_done(symbol), [true; 6], "the state after them, every part");
        CMapDB::clear_provider();
    }

    /// A snapshot shares the parts besides the interned symbols with the
    /// previous one only when none of them changed: each part changed alone
    /// makes a new one, which puts that part back (the warnings registry
    /// included).
    #[test]
    fn snapshots_share_only_unchanged_parts() {
        CMapDB::set_provider(Some(Rc::new(Empty)));
        // a new process (under WebAssembly the tests share one thread)
        pycompat::re_cache::reset();
        pycompat::typing_cache::reset();
        pycompat::logging::disable();
        pycompat::specialize::reset();
        CMapDB::clear_caches();
        pycompat::warnings::reset();
        let changes: [(&str, &dyn Fn()); 6] = [
            ("re", &|| pycompat::re_cache::set_compiled(PATTERN, true)),
            ("typing", &|| drop(SUBSCRIPTION.eval())),
            ("logging", &|| drop(LOG.warning())),
            ("sites", &|| pycompat::specialize::set_executed("process_state.test site", true)),
            ("cmaps", &|| drop(CMapDB::get_cmap("process_state shared"))),
            ("warnings", &|| pycompat::warnings::warn("UserWarning", "process_state.rs", "w".to_owned())),
        ];
        for (name, change) in changes {
            let before = ProcessState::capture();
            assert!(Rc::ptr_eq(&before.rest, &ProcessState::capture().rest), "{name}: nothing changed");
            change();
            let after = ProcessState::capture();
            assert!(!Rc::ptr_eq(&before.rest, &after.rest), "{name}: a new snapshot");
            before.clone().restore();
            assert!(ProcessState::capture().rest.same(&before.rest), "{name}: put back");
        }
        assert!(pycompat::warnings::snapshot().is_empty());
        CMapDB::clear_provider();
    }
}
