//! The state of the Python process a trace starts from, which the port
//! sets at the start of each trace ([`start`], called by
//! `common::terminated`).
//!
//! Some Python state outlives a call and changes how deep a later call
//! goes, so where a `RecursionError` happens (`docs/architecture.md`,
//! "Process State"): the symbols interned so far (interning a
//! name for the first time instantiates it, a frame more:
//! `pdfminer::psparser::py_intern`), the patterns `re` compiled
//! (`pycompat::re_cache`: none at the start of a trace), the CMaps
//! `CMapDB` loaded, and the call sites the adaptive interpreter has
//! executed (`pycompat::specialize`: the first `str(x)` of `literal_name`
//! takes one C unit more). The oracle computes every trace of
//! `oracle.verify` in a new process (`python -m oracle.trace`,
//! `oracle/cache.py`): a trace starts from the symbols that process
//! imports ([`super::py_symbols`]), which [`set_for_stage`] selects (the
//! `pdftrace` binary, for each file), and with no site executed. The unit
//! generators `oracle/unit/document.py` and `oracle/unit/interp.py` fork
//! their process for each trace and record the state of that process
//! (`py_process.jsonl`, [`PyProcess::from_value`]), which their replays
//! set with [`set`].

use std::cell::{Cell, RefCell};

use pdfminer::psparser::{py_symbols_reset, LIT_bytes, Symbol, Symbols, KWD, LIT};
use pycompat::specialize::{self, CallSite};

use crate::value::Value;

/// The call sites of the port whose first execution differs
/// (`pycompat::specialize`), as `oracle/depth.py` (`CALL_SITES`) names
/// them.
pub const CALL_SITES: &[&CallSite] =
    &[&pdfminer::psparser::LITERAL_NAME_STR, &pdfplumber::utils::pdfinternals::DECODE_TEXT_STR];

/// A process state.
#[derive(Clone, Debug, Default)]
pub struct PyProcess {
    /// `str` names of the interned literals
    pub literals: Vec<String>,
    /// `bytes` names of the interned literals
    pub bytes_literals: Vec<Vec<u8>>,
    /// names of the interned keywords
    pub keywords: Vec<Vec<u8>>,
    /// the call sites of [`CALL_SITES`] executed so far
    pub call_sites: Vec<&'static str>,
}

impl PyProcess {
    /// A new process that imported what `symbols` lists.
    pub fn new(symbols: &Symbols) -> PyProcess {
        PyProcess {
            literals: symbols.literals.iter().map(|s| (*s).to_owned()).collect(),
            bytes_literals: symbols.bytes_literals.iter().map(|b| b.to_vec()).collect(),
            keywords: symbols.keywords.iter().map(|k| k.to_vec()).collect(),
            call_sites: Vec::new(),
        }
    }

    /// The state `process_state()` of `oracle/unit/document.py` recorded:
    /// `{"literals": [..], "bytes_literals": [..], "keywords": [..],
    /// "call_sites": [..]}`.
    pub fn from_value(v: &Value) -> PyProcess {
        let Value::Dict(items) = v else { panic!("a process state: {v:?}") };
        let get = |name: &str| {
            items.iter().find(|(k, _)| k.as_str() == Some(name)).map(|(_, v)| v).unwrap_or_else(|| panic!("{name}"))
        };
        let list = |name: &str| match get(name) {
            Value::List(l) => l.clone(),
            other => panic!("{name}: {other:?}"),
        };
        let bytes = |v: &Value| match v {
            Value::Bytes(b) => b.clone(),
            other => panic!("bytes: {other:?}"),
        };
        let site = |v: &Value| {
            let name = v.as_str().expect("a call site name");
            CALL_SITES.iter().find(|s| s.name == name).unwrap_or_else(|| panic!("unknown call site {name:?}")).name
        };
        PyProcess {
            literals: list("literals").iter().map(|v| v.as_str().expect("a str name").to_owned()).collect(),
            bytes_literals: list("bytes_literals").iter().map(bytes).collect(),
            keywords: list("keywords").iter().map(bytes).collect(),
            call_sites: list("call_sites").iter().map(site).collect(),
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
    /// Inside [`continuing`]: traces go on in the process as it is.
    static CONTINUING: Cell<bool> = const { Cell::new(false) };
}

/// Runs `f`, whose traces go on in the current process instead of each
/// starting a new one ([`start`] and `logging.disable()` are skipped): the
/// calls of one Python process, as the JavaScript boundary runs them
/// (`tests/lazy_cmap.rs`, with `pdfminer::process_state` around a voided
/// call).
pub fn continuing<R>(f: impl FnOnce() -> R) -> R {
    struct Reset(bool);
    impl Drop for Reset {
        fn drop(&mut self) {
            CONTINUING.with(|c| c.set(self.0));
        }
    }
    let _reset = Reset(CONTINUING.with(|c| c.replace(true)));
    f()
}

/// Whether the traces go on in the current process ([`continuing`]).
pub fn is_continuing() -> bool {
    CONTINUING.with(Cell::get)
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
/// symbols and executed call sites, and what a new process has not done
/// yet (no pattern of `re` compiled, `pycompat::re_cache`; `CMapDB`'s
/// caches empty: a predefined CMap loaded by an earlier trace would
/// otherwise be found in the cache, fewer frames down than Python's first
/// load).
pub fn start() {
    PROCESS.with(|p| {
        let p = p.borrow();
        py_symbols_reset(p.symbols());
        specialize::reset();
        for name in &p.call_sites {
            specialize::set_executed(name, true);
        }
        pdfminer::cmapdb::CMapDB::clear_caches();
    });
    pycompat::re_cache::reset();
    pycompat::typing_cache::reset();
    pycompat::warnings::reset();
}

#[cfg(test)]
mod tests {
    use pdfminer::psparser::{py_intern, LITERAL_NAME_STR};
    use pycompat::recursion::{with_depth, Depth, PY_RECURSION_LIMIT};

    use super::*;

    #[test]
    fn a_recorded_state_is_the_state_a_trace_starts_from() {
        let state = Value::Dict(vec![
            (Value::str("literals"), Value::List(vec![Value::str("Zz recorded")])),
            (Value::str("bytes_literals"), Value::List(vec![Value::Bytes(b"\xff\xfe".to_vec())])),
            (Value::str("keywords"), Value::List(vec![Value::Bytes(b"zzkw".to_vec())])),
            (Value::str("call_sites"), Value::List(vec![Value::str(LITERAL_NAME_STR.name)])),
        ]);
        let process = PyProcess::from_value(&state);
        assert_eq!(process.literals, ["Zz recorded"]);
        assert_eq!(process.bytes_literals, [b"\xff\xfe".to_vec()]);
        assert_eq!(process.keywords, [b"zzkw".to_vec()]);
        assert_eq!(process.call_sites, [LITERAL_NAME_STR.name]);
        set(process);
        start();
        assert!(LITERAL_NAME_STR.executed());
        // an interned name takes one frame (no class call) at the limit
        let last = Depth { py: PY_RECURSION_LIMIT - 1, c: 0 };
        let recorded = [
            Symbol::Literal(LIT("Zz recorded")),
            Symbol::Literal(LIT_bytes(b"\xff\xfe")),
            Symbol::Keyword(KWD(b"zzkw")),
        ];
        for symbol in recorded {
            assert!(with_depth(last, || py_intern(symbol)).is_ok());
        }
        assert!(with_depth(last, || py_intern(Symbol::Literal(LIT("Zz not recorded")))).is_err());
        set_for_stage("objects");
        start();
        assert!(!LITERAL_NAME_STR.executed(), "a new process");
    }
}
