//! Not a pdfminer module: the state of the Python process that the port
//! models because it decides how deep a later call goes, so where a
//! `RecursionError` happens (`docs/architecture.md`, "Profondeur de
//! récursion"), taken and put back as a whole.
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

use crate::cmapdb::{CMapDB, CMapDBCaches};
use crate::psparser::{py_symbols_restore, py_symbols_snapshot, PyInterned};

/// The modeled state of the process (see the module documentation).
#[derive(Clone)]
pub struct ProcessState {
    interned: PyInterned,
    compiled: Vec<&'static str>,
    subscriptions: Vec<&'static str>,
    loggers: pycompat::logging::LoggerCaches,
    call_sites: Vec<&'static str>,
    cmaps: CMapDBCaches,
}

impl ProcessState {
    /// The state of this thread's process now.
    pub fn capture() -> ProcessState {
        ProcessState {
            interned: py_symbols_snapshot(),
            compiled: pycompat::re_cache::snapshot(),
            subscriptions: pycompat::typing_cache::snapshot(),
            loggers: pycompat::logging::snapshot(),
            call_sites: pycompat::specialize::snapshot(),
            cmaps: CMapDB::caches_snapshot(),
        }
    }

    /// Puts this state back.
    pub fn restore(self) {
        py_symbols_restore(self.interned);
        pycompat::re_cache::restore(self.compiled);
        pycompat::typing_cache::restore(self.subscriptions);
        pycompat::logging::restore(self.loggers);
        pycompat::specialize::restore(self.call_sites);
        CMapDB::restore_caches(self.cmaps);
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
    fn parts_done(symbol: Symbol) -> [bool; 5] {
        let at = |py: u32, c: u32, f: &mut dyn FnMut() -> bool| with_depth(Depth { py, c }, f);
        [
            at(PY_RECURSION_LIMIT - 1, 0, &mut || py_intern(symbol).is_ok()),
            at(PY_RECURSION_LIMIT - 2, 0, &mut || LOG.warning().is_ok()),
            at(PY_RECURSION_LIMIT - 3, 0, &mut || pycompat::re_cache::compile_frames(PATTERN, 5).is_ok()),
            at(0, C_RECURSION_LIMIT, &mut || SITE.str_call().is_ok()),
            CMapDB::caches_snapshot().names().0 == ["process_state test"],
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
        };
        done_once();
        let done = ProcessState::capture();
        fresh.clone().restore();
        assert_eq!(parts_done(symbol), [false; 5], "the fresh state, every part");
        // (parts_done did them, at depth, failing: still not done but for
        // the call site, executed even when its call raises)
        fresh.restore();
        done.restore();
        assert_eq!(parts_done(symbol), [true; 5], "the state after them, every part");
        CMapDB::clear_provider();
    }
}
