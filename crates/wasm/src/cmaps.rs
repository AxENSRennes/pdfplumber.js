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
    /// The names JavaScript can provide.
    known: FxHashSet<String>,
    /// Inflated files, by name.
    loaded: FxHashMap<String, Rc<[u8]>>,
    /// Names asked for and not loaded, in order, since the last
    /// [`take_missing`].
    missing: Vec<String>,
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
    STATE.with(|s| s.borrow_mut().known = names.into_iter().collect());
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
    STATE.with(|s| s.borrow_mut().loaded.insert(name.to_owned(), Rc::from(inflated)));
    Ok(())
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
        configure([]);
        assert!(JsCMapProvider.load("B").is_none());
        assert!(take_missing().is_empty());
    }
}
