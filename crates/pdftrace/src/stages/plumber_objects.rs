//! Stage `plumber_objects`: pdfplumber `page.objects`, every attribute, plus boxes and `doctop`.
//!
//! Not implemented yet. The line format is documented in
//! `oracle/stages/plumber_objects.py`.

use super::Stage;

pub const STAGE: Stage = Stage { name: "plumber_objects", run: None, oracle_input: false };
