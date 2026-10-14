//! Stage `psparser`: objects `(pos, obj)` produced by `PDFContentParser` on the content streams of each page and Form XObject.
//!
//! Not implemented yet. The line format is documented in
//! `oracle/stages/psparser.py`.

use super::Stage;

pub const STAGE: Stage = Stage { name: "psparser", run: None };
