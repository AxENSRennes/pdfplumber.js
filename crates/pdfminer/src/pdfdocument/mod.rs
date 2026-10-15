//! Port of `pdfminer/pdfdocument.py` (partial; a folder, as
//! `docs/architecture.md` allows for a large module).
//!
//! Ported: the standard security handlers and their registry
//! (`security.rs`). Not ported yet: `PDFXRef`, `PDFXRefFallback`,
//! `PDFXRefStream`, `PDFDocument` (whose `_initialize_password` will pick a
//! handler with `security_handler_registry`), the outlines and page labels.

mod security;
pub use security::*;
