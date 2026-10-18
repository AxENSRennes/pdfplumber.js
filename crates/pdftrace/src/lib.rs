//! Differential testing against the Python oracle.
//!
//! - [`canon`]: the canonical trace writer (`docs/verification.md`).
//! - [`typed_json`]: the typed JSON exchange format of unit cases.
//! - [`value`]: [`value::Value`], a dynamic Python value both are written
//!   from.
//! - [`stages`]: the registry of trace stages behind the `pdftrace`
//!   binary.
//! - [`unit`]: locating and checking the unit test data written by
//!   `oracle/unit/*.py` (the replay tests are in `tests/`).

pub mod canon;
pub mod font_case;
pub mod layout_dump;
pub mod plumber;
pub mod stages;
pub mod typed_json;
pub mod unit;
pub mod value;
