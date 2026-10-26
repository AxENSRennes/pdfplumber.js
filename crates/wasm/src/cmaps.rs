//! Predefined CMaps served by JavaScript, loaded lazily
//! (`docs/js-runtime.md`, "CMaps").
//!
//! The core reads a predefined CMap synchronously (`CMapDB.get_cmap`, from
//! `CMapParser.do_keyword` for `usecmap`, from the font code), and a
//! browser can only fetch asynchronously. JavaScript therefore tells the
//! core which names exist ([`configure`], the names of pdfminer's `cmap`
//! directory) and hands it the files it has fetched ([`add`]). When the core
//! asks for a name that exists but has not been handed over yet, the
//! provider records it and answers as if it did not exist; the call that
//! asked is then void ([`take_missing`]): the boundary discards its result
//! and the state it touched (the documents' derived state, and the modeled
//! state of the Python process, `pdfminer::process_state`, put back as it
//! was before the call), reports the names, and JavaScript re-runs the
//! call once it has added them. The re-run sees the CMap from the start, so
//! its result is the one Python computes with every CMap installed, down to
//! where a `RecursionError` happens.
//!
//! The files are the compact form of `oracle/codegen/cmaps.py`, compressed
//! with zlib (`scripts/build-cmaps.mjs`); they are inflated here by the
//! port of zlib (`pycompat::zlib`), so every runtime decodes them with the
//! same code. A file that is already inflated (a server that applied its
//! own `Content-Encoding`) is accepted as is.

use std::cell::RefCell;
use std::rc::Rc;

use pdfminer::cmapdb::{CMapDB, CMapProvider};
use pycompat::{exc, PyResult};
use rustc_hash::{FxHashMap, FxHashSet};

#[derive(Default)]
struct State {
    /// Whether [`configure`] ran.
    configured: bool,
    /// The names JavaScript can provide.
    known: FxHashSet<String>,
    /// Inflated files, by name.
    loaded: FxHashMap<String, Rc<[u8]>>,
    /// Names asked for and not loaded, in order, since the last
    /// [`take_missing`].
    missing: Vec<String>,
    /// Names JavaScript could not provide when it handed over every CMap
    /// ([`mark_unavailable`]), until one is handed over after all.
    unavailable: FxHashSet<String>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

/// The provider the boundary installs for its thread.
pub struct JsCMapProvider;

impl CMapProvider for JsCMapProvider {
    fn load(&self, name: &str) -> Option<Vec<u8>> {
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            if let Some(data) = s.loaded.get(name) {
                return Some(data.to_vec());
            }
            if s.known.contains(name) && !s.missing.iter().any(|m| m == name) {
                s.missing.push(name.to_owned());
            }
            None
        })
    }
}

/// Installs the provider for this thread (idempotent).
pub fn install() {
    CMapDB::set_provider(Some(Rc::new(JsCMapProvider)));
}

/// Sets the names JavaScript can provide (an empty list: an installation
/// without the CMaps, every name is `CMapNotFound`).
pub fn configure(names: impl IntoIterator<Item = String>) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.known = names.into_iter().collect();
        s.configured = true;
    });
}

/// Whether every name JavaScript can provide was handed over (none, for an
/// installation without CMaps), or found unavailable ([`mark_unavailable`]):
/// then no call can miss one that JavaScript will hand over later, and the
/// history of the documents is no longer kept (`crate::ops`, "Void calls").
pub fn complete() -> bool {
    STATE.with(|s| {
        let s = s.borrow();
        s.configured && s.known.iter().all(|n| s.loaded.contains_key(n) || s.unavailable.contains(n))
    })
}

/// Marks names JavaScript failed to provide when it handed over every CMap
/// (a file its server or its loader does not have): [`complete`] holds
/// without them, so the history of the documents goes and stays bounded
/// (the core would otherwise keep it, and JavaScript fetch every missing
/// file again, at every call). A call that asks for one is still void,
/// and JavaScript fetches it then; a document whose history went cannot be
/// rebuilt by that void call (`crate::ops`, "Void calls": every later
/// operation on it raises), where without the file its call could only
/// fail. A name handed over later ([`add`]) is available again.
pub fn mark_unavailable(names: impl IntoIterator<Item = String>) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let names: Vec<String> = names.into_iter().filter(|n| s.known.contains(n) && !s.loaded.contains_key(n)).collect();
        s.unavailable.extend(names);
    });
}

/// The magic of an inflated compact CMap.
const COMPACT_MAGIC: &[u8] = b"PMCM";

/// Hands over the file of `name`: zlib-compressed, or already inflated.
pub fn add(name: &str, data: &[u8]) -> PyResult<()> {
    let known = STATE.with(|s| s.borrow().known.contains(name));
    if !known {
        return Err(exc::ValueError(format!("unknown predefined CMap {name:?}")));
    }
    let inflated = if data.starts_with(COMPACT_MAGIC) { data.to_vec() } else { pycompat::zlib::decompress(data)? };
    if !inflated.starts_with(COMPACT_MAGIC) {
        return Err(exc::ValueError(format!("the file of CMap {name:?} is not a compact CMap")));
    }
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.unavailable.remove(name);
        s.loaded.insert(name.to_owned(), Rc::from(inflated));
    });
    Ok(())
}

/// Whether JavaScript can provide `name` (`configure`).
pub fn is_known(name: &str) -> bool {
    STATE.with(|s| s.borrow().known.contains(name))
}

/// Whether the file of `name` was handed over.
pub fn is_loaded(name: &str) -> bool {
    STATE.with(|s| s.borrow().loaded.contains_key(name))
}

/// The names asked for and not loaded since the last call, emptied.
pub fn take_missing() -> Vec<String> {
    STATE.with(|s| std::mem::take(&mut s.borrow_mut().missing))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_names_are_recorded_once_and_cleared() {
        install();
        configure(["A".to_owned(), "B".to_owned()]);
        assert!(JsCMapProvider.load("A").is_none());
        assert!(JsCMapProvider.load("A").is_none());
        assert!(JsCMapProvider.load("C").is_none());
        assert!(JsCMapProvider.load("B").is_none());
        assert_eq!(take_missing(), ["A", "B"]);
        assert!(take_missing().is_empty());
        assert!(add("C", b"PMCM\x01\x00").is_err());
        assert!(add("A", b"not zlib").is_err());
        add("A", b"PMCM\x01\x00").unwrap();
        assert!(is_loaded("A") && !is_loaded("B"));
        assert_eq!(JsCMapProvider.load("A").as_deref(), Some(&b"PMCM\x01\x00"[..]));
        assert!(take_missing().is_empty());
        assert!(!complete(), "B is missing");
        add("B", b"PMCM\x01\x00").unwrap();
        assert!(complete());
        configure([]);
        assert!(complete(), "no CMap at all");
        assert!(JsCMapProvider.load("B").is_some(), "a file handed over stays");
        configure(["Z".to_owned()]);
        assert!(JsCMapProvider.load("Z").is_none());
        assert!(!complete());
        assert_eq!(take_missing(), ["Z"]);
    }

    #[test]
    fn unavailable_names_complete_the_set_until_one_is_handed_over() {
        install();
        configure(["U".to_owned(), "V".to_owned(), "W".to_owned()]);
        add("U", b"PMCM\x01\x00").unwrap();
        assert!(!complete());
        // an unknown name and a loaded one are not marked
        mark_unavailable(["V".to_owned(), "X".to_owned(), "U".to_owned()]);
        assert!(!complete(), "W is neither loaded nor unavailable");
        mark_unavailable(["W".to_owned()]);
        assert!(complete());
        // still asked for (the call is void: JavaScript fetches it again)
        assert!(JsCMapProvider.load("V").is_none());
        assert_eq!(take_missing(), ["V"]);
        add("V", b"PMCM\x01\x00").unwrap();
        assert!(complete());
        assert!(STATE.with(|s| !s.borrow().unavailable.contains("V")), "handed over: available again");
        assert!(STATE.with(|s| !s.borrow().unavailable.contains("U") && !s.borrow().unavailable.contains("X")));
    }
}
