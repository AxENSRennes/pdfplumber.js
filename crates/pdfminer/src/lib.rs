//! Faithful Rust port of pdfminer.six 20260107 (the code the oracle runs).
//!
//! One Rust module per Python module, with the same names; every ported
//! function carries its upstream `file:line` reference. See
//! `docs/architecture.md` for the porting rules and the value model, and the
//! module documentation of [`psparser`] (parser extension points) and
//! [`pdftypes`] (object model, references) for the core design.
//!
//! Modules marked "not ported yet" only hold their header. Out of scope
//! (see `docs/architecture.md`): `high_level.py`, `image.py`, `jbig2.py`
//! (used only by `image.py`), and the HTML/XML/hOCR parts of
//! `converter.py`.

pub mod arcfour;
pub mod ascii85;
pub mod casting;
pub mod ccitt;
pub mod cmapdb;
pub mod converter;
pub mod data_structures;
pub mod encodingdb;
pub mod fontmetrics;
pub mod glyphlist;
pub mod latin_enc;
pub mod layout;
pub mod lzw;
pub mod pdfcolor;
pub mod pdfdevice;
pub mod pdfdocument;
pub mod pdfexceptions;
pub mod pdffont;
pub mod pdfinterp;
pub mod pdfpage;
pub mod pdfparser;
pub mod pdftypes;
pub mod psexceptions;
pub mod psparser;
pub mod runlength;
/// `pdfminer/_saslprep.py`
pub mod saslprep;
pub mod settings;
pub mod utils;
