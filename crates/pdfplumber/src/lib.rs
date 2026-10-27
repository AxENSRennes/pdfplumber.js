//! Faithful Rust port of pdfplumber 0.11.9 (the code the oracle runs).
//!
//! One Rust module per Python module, with the same names (`_typing.py` is
//! `typing`). Out of scope (see `docs/architecture.md`, "Porting Rules",
//! rule 5): `display.py`, `repair.py`, `cli.py`.

pub mod container;
pub mod convert;
pub mod ctm;
pub mod obj;
pub mod page;
pub mod pdf;
pub mod structure;
pub mod table;
/// `pdfplumber/_typing.py`
pub mod typing;
pub mod utils;
