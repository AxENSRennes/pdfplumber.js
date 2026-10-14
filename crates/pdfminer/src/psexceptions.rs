//! Port of `pdfminer/psexceptions.py`.
//!
//! The classes are kinds of `pycompat::PyErr` (`ExcKind`), so that
//! `except PSException:` also catches `PSEOF` (`err.is_instance(...)`).

pub use pycompat::ExcKind::{PSException, PSSyntaxError, PSTypeError, PSValueError, PSEOF};
