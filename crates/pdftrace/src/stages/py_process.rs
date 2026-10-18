//! The state of the Python process a trace starts from, which the port
//! sets at the start of each trace ([`start`], called by
//! `common::terminated`).
//!
//! Some Python state outlives a call and changes how deep a later call
//! goes, so where a `RecursionError` happens (`docs/architecture.md`,
//! "Profondeur de récursion"): the symbols interned so far (interning a
//! name for the first time instantiates it, a frame more:
//! `pdfminer::psparser::py_intern`). The oracle computes every trace of
//! `oracle.verify` in a new process (`python -m oracle.trace`,
//! `oracle/cache.py`): a trace starts from the symbols that process
//! imports ([`super::py_symbols`]), which [`set_for_stage`] selects (the
//! `pdftrace` binary, for each file). The
//! unit generator `oracle/unit/document.py` forks its process for each
//! trace and records the state of that process, which its replay sets with
//! [`set`].

use std::cell::RefCell;

use pdfminer::psparser::{py_symbols_reset, LIT_bytes, Symbol, Symbols, KWD, LIT};

/// A process state.
#[derive(Clone, Debug, Default)]
pub struct PyProcess {
    /// `str` names of the interned literals
    pub literals: Vec<String>,
    /// `bytes` names of the interned literals
    pub bytes_literals: Vec<Vec<u8>>,
    /// names of the interned keywords
    pub keywords: Vec<Vec<u8>>,
}

impl PyProcess {
    /// A new process that imported what `symbols` lists.
    pub fn new(symbols: &Symbols) -> PyProcess {
        PyProcess {
            literals: symbols.literals.iter().map(|s| (*s).to_owned()).collect(),
            bytes_literals: symbols.bytes_literals.iter().map(|b| b.to_vec()).collect(),
            keywords: symbols.keywords.iter().map(|k| k.to_vec()).collect(),
        }
    }

    fn symbols(&self) -> Vec<Symbol> {
        let literals = self.literals.iter().map(|s| Symbol::Literal(LIT(s)));
        let bytes_literals = self.bytes_literals.iter().map(|b| Symbol::Literal(LIT_bytes(b)));
        let keywords = self.keywords.iter().map(|k| Symbol::Keyword(KWD(k)));
        literals.chain(bytes_literals).chain(keywords).collect()
    }
}

thread_local! {
    static PROCESS: RefCell<PyProcess> =
        RefCell::new(PyProcess::new(&pdfminer::psparser_symbols::IMPORT_PDFPLUMBER));
}

/// The traces of this thread start from `process`.
pub fn set(process: PyProcess) {
    PROCESS.with(|p| *p.borrow_mut() = process);
}

/// The traces of this thread start from the oracle's process for `stage`.
pub fn set_for_stage(stage: &str) {
    let symbols = super::py_symbols::for_stage(stage).unwrap_or(pdfminer::psparser_symbols::IMPORT_PDFPLUMBER);
    set(PyProcess::new(&symbols));
}

/// Starts a trace from the process state of this thread: its interned
/// symbols, and what a new process has not done yet (the pattern of
/// `get_inline_data` not compiled, `CMapDB`'s caches empty: a predefined
/// CMap loaded by an earlier trace would otherwise be found in the cache,
/// fewer frames down than Python's first load).
pub fn start() {
    PROCESS.with(|p| {
        py_symbols_reset(p.borrow().symbols());
        pdfminer::pdfinterp::reset_process_state();
        pdfminer::cmapdb::CMapDB::clear_caches();
    });
}
