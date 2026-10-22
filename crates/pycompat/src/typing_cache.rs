//! The Python frames of a subscription of a `typing` alias (`Tuple[str,
//! Any]`), which depend on the caches of `typing._tp_cache`: process state
//! that decides where a `RecursionError` happens in a deep recursion, as
//! the compiled patterns of `re` do ([`crate::re_cache`]).
//!
//! A function definition evaluates its annotations each time it runs,
//! unless its module imports `annotations` from `__future__`: pdfplumber's
//! nested functions (`process_attr` of `Page.process_object`, `sorter` of
//! `Page.find_table`, `iter_pages` of `PDF.pages`, the helpers of
//! `table.py`...) evaluate theirs each time the enclosing function runs.
//! Each subscription calls the `__getitem__` of the alias's class, wrapped
//! by `_tp_cache` (`Lib/typing.py`): its `inner` wrapper (a frame) calls a
//! `functools.lru_cache` (C), which hashes the arguments (the `__hash__`
//! of an argument that is itself an alias is a Python method, called from
//! C) and, the first time in a process (a miss), calls the `__getitem__`,
//! which builds the alias (`_GenericAlias.__init__`... several frames
//! deeper). The depths are measured by the oracle (`python -m
//! oracle.depth_probe typing`), below the frame that evaluates the
//! annotation; a subscription that the imports of pdfplumber already made
//! (`Optional[T_bbox]`...) hits from the start and is a constant
//! [`Subscription`] whose miss and hit depths are equal.
//!
//! A subscription interrupted by a `RecursionError` caches nothing (the
//! `lru_cache` stores a result only when the call returns; the inner
//! subscriptions it completed before are not modeled). Eviction (more than
//! 128 distinct subscriptions of one `__getitem__` in a process) is not
//! modeled: pdfplumber makes a few.
//!
//! The cache is per thread, as the recursion counters are; [`reset`] is a
//! new Python process (whose imports made none of the subscriptions whose
//! miss differs from their hit).

use std::cell::RefCell;

use crate::err::PyResult;
use crate::recursion::leaf;

thread_local! {
    /// The subscriptions cached so far.
    static CACHED: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
}

/// A subscription of a `typing` alias in an annotation, named by its text
/// (the cache key: two annotations with the same text in pdfplumber
/// subscript the same objects), with its depths below the evaluating
/// frame.
#[derive(Debug)]
pub struct Subscription {
    pub key: &'static str,
    /// The deepest frame of the first evaluation in a process.
    pub miss: u32,
    /// The deepest frame of the next ones.
    pub hit: u32,
}

impl Subscription {
    pub const fn new(key: &'static str, miss: u32, hit: u32) -> Subscription {
        Subscription { key, miss, hit }
    }

    /// Evaluates the subscription: its frames, `miss` deep the first time
    /// in a process, `hit` deep after. Raises `RecursionError` when they
    /// do not fit (and caches nothing).
    pub fn eval(&self) -> PyResult<()> {
        if self.miss == self.hit || is_cached(self.key) {
            return leaf(self.hit);
        }
        leaf(self.miss)?;
        CACHED.with(|c| c.borrow_mut().push(self.key));
        Ok(())
    }
}

/// A new process: no subscription cached.
pub fn reset() {
    CACHED.with(|c| c.borrow_mut().clear());
}

/// Whether the subscription `key` is cached.
pub fn is_cached(key: &str) -> bool {
    CACHED.with(|c| c.borrow().contains(&key))
}

/// The subscriptions cached so far: a snapshot of the state, to put back
/// with [`restore`].
pub fn snapshot() -> Vec<&'static str> {
    CACHED.with(|c| c.borrow().clone())
}

/// Puts back a [`snapshot`].
pub fn restore(cached: Vec<&'static str>) {
    CACHED.with(|c| *c.borrow_mut() = cached);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recursion::{with_depth, Depth, PY_RECURSION_LIMIT};

    #[test]
    fn a_miss_is_deeper_once_per_process() {
        reset();
        let s = Subscription::new("Tuple[test, Any]", 7, 1);
        let room = |n: u32| Depth { py: PY_RECURSION_LIMIT - n, c: 0 };
        // the miss does not fit: nothing cached
        assert!(with_depth(room(6), || s.eval()).is_err());
        assert!(!is_cached(s.key));
        assert!(with_depth(room(7), || s.eval()).is_ok());
        assert!(is_cached(s.key));
        assert!(with_depth(room(1), || s.eval()).is_ok());
        assert!(with_depth(room(0), || s.eval()).is_err());
        let saved = snapshot();
        reset();
        assert!(with_depth(room(6), || s.eval()).is_err());
        restore(saved);
        assert!(with_depth(room(1), || s.eval()).is_ok());
        // a constant subscription
        let c = Subscription::new("Optional[test]", 3, 3);
        assert!(with_depth(room(2), || c.eval()).is_err());
        assert!(with_depth(room(3), || c.eval()).is_ok());
        assert!(!is_cached(c.key));
        reset();
    }
}
