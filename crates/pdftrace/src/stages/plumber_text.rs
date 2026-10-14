//! Stage `plumber_text`: `extract_text` (simple, `layout=True`, options), `extract_words`, `extract_text_lines`, `search`, `dedupe_chars`.
//!
//! Not implemented yet. The line format is documented in
//! `oracle/stages/plumber_text.py`.

use super::Stage;

pub const STAGE: Stage = Stage { name: "plumber_text", run: None };
