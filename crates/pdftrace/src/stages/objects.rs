//! Stage `objects`: xref, trailer, every indirect object resolved (`getobj`), errors included.
//!
//! Not implemented yet. The line format is documented in
//! `oracle/stages/objects.py`.

use super::Stage;

pub const STAGE: Stage = Stage { name: "objects", run: None };
