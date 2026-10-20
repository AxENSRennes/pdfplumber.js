//! Stage `plumber_objects`: pdfplumber's page objects (every kind, every
//! attribute), boxes, `doctop`, annotations, hyperlinks, `pdf.objects`,
//! then the same with `laparams={}`. The traversal and the line format are
//! those of `oracle/stages/plumber_objects.py`.
//!
//! Not implemented yet (wave 4, workstream PAGE; `annots`/`hyperlinks`
//! lines call workstream DOC's `Page::annots`/`hyperlinks`). The runner
//! (`run`, then `run: Some(run)`), with `plumber_common`:
//!
//! - `terminated` from `STAGE_ENTRY`; `open_pdf`
//!   (`pdfplumber::pdf::PDF::open`), an error written `open <exc>`;
//!   `pdf.pages()`; for each page the `box` lines (`Page` fields,
//!   `width()`, `height()`), `objects()` written by an `Objects` writer
//!   (`kinds`, `keys` when they change, `obj` with `crate::plumber::to_value`
//!   and `Names`), which records each char's canonical text for the
//!   `laparams` variant's `lchars`;
//! - `annots()`/`hyperlinks()` (`annot <i> <dict>`), `pdf.objects()`,
//!   `pdf.annots()`, `pdf.hyperlinks()` (digests);
//! - the second opening with `OpenOptions { laparams: Some({}), ids }`,
//!   `ids` built from the oracle's `ids <page_number> [[key, rank], ...]`
//!   lines (`StageInput::oracle`, required: `ORACLE_INPUT`); after each
//!   page's objects, the `ids` line of the Rust analysis (the ranks it
//!   used, keyed as Python keys them).

use super::Stage;

pub const STAGE: Stage = Stage { name: "plumber_objects", run: None, oracle_input: true };
