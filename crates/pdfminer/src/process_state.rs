//! Not a pdfminer module: the state of the Python process that the port
//! models because it decides how deep a later call goes, so where a
//! `RecursionError` happens (`docs/architecture.md`, "Profondeur de
//! récursion"), taken and put back as a whole.
//!
//! A Python process remembers what it has done: the symbols it interned
//! (`psparser::py_intern`), the patterns `re` compiled
//! (`pycompat::re_cache`), the logger caches (`pycompat::logging`), the
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
            loggers: pycompat::logging::snapshot(),
            call_sites: pycompat::specialize::snapshot(),
            cmaps: CMapDB::caches_snapshot(),
        }
    }

    /// Puts this state back.
    pub fn restore(self) {
        py_symbols_restore(self.interned);
        pycompat::re_cache::restore(self.compiled);
        pycompat::logging::restore(self.loggers);
        pycompat::specialize::restore(self.call_sites);
        CMapDB::restore_caches(self.cmaps);
    }
}
