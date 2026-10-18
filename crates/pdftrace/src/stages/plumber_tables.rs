//! Stage `plumber_tables`: `find_tables`/`extract_tables` for the `lines`, `lines_strict`, `text` and `explicit` strategies.
//!
//! Not implemented yet. The line format is documented in
//! `oracle/stages/plumber_tables.py`.

use super::Stage;

pub const STAGE: Stage = Stage { name: "plumber_tables", run: None, oracle_input: false };
