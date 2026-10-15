//! A raised Python exception, as JavaScript receives it.
//!
//! JavaScript gets the class (`type(e).__name__`, `__module__`,
//! `__qualname__`), its method resolution order (`type(e).__mro__`, each
//! class as `module.qualname`, so that `error.isInstance("PDFException")`
//! answers like `isinstance(e, PDFException)`) and `str(e)`, and builds an
//! `Error` whose `name` is the Python class name (`js/src/errors.ts`).

use pycompat::{ExcKind, PyErr};

use crate::wire::Writer;

/// `module.qualname` of a class (`"builtins.ValueError"`,
/// `"pdfminer.cmapdb.CMapDB.CMapNotFound"`).
pub fn qualified_name(kind: ExcKind) -> String {
    format!("{}.{}", kind.module(), kind.qualname())
}

/// `type.__mro__` of an exception class, by C3 linearization of the
/// bases `ExcKind::bases` lists (Python's own algorithm), without `object`.
pub fn mro(kind: ExcKind) -> Vec<ExcKind> {
    let mut seqs: Vec<Vec<ExcKind>> = kind.bases().iter().map(|&b| mro(b)).collect();
    seqs.push(kind.bases().to_vec());
    let mut out = vec![kind];
    loop {
        seqs.retain(|s| !s.is_empty());
        if seqs.is_empty() {
            return out;
        }
        // the first head that is in no tail
        let head = seqs
            .iter()
            .map(|s| s[0])
            .find(|h| !seqs.iter().any(|s| s[1..].contains(h)))
            .expect("the exception hierarchy has a consistent MRO");
        out.push(head);
        for s in &mut seqs {
            if s[0] == head {
                s.remove(0);
            }
        }
    }
}

/// Writes the error as the dict JavaScript reads.
pub fn write_error<W: Writer>(w: &mut W, err: &PyErr) {
    let kind = err.kind;
    w.dict(5);
    w.str("name");
    w.str(kind.name());
    w.str("module");
    w.str(kind.module());
    w.str("qualname");
    w.str(&kind.qualname());
    w.str("mro");
    let mro = mro(kind);
    // `object` closes every Python MRO; NotPorted is not a Python class
    let object = kind != ExcKind::NotPorted;
    w.list(mro.len() + usize::from(object));
    for k in mro {
        w.str(&qualified_name(k));
    }
    if object {
        w.str("builtins.object");
    }
    w.str("message");
    w.str(&err.message);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The MRO of every class is compared with CPython's by
    /// `tests/js_runtime_unit.rs`; here, its structure: the class first,
    /// each class once, every base after the classes derived from it.
    #[test]
    fn mro_is_a_linearization() {
        for &k in ExcKind::ALL {
            let m = mro(k);
            assert_eq!(m[0], k);
            for (i, c) in m.iter().enumerate() {
                assert!(!m[i + 1..].contains(c), "{k:?}: {c:?} twice");
                for b in c.bases() {
                    assert!(m[i + 1..].contains(b), "{k:?}: {b:?} before {c:?}");
                }
            }
        }
    }
}
