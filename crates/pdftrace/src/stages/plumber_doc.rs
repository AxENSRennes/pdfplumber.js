//! Stage `plumber_doc`: metadata, annotations, links, structure tree, `crop`/`within_bbox`/`outside_bbox`.
//!
//! Not implemented yet. The line format is documented in
//! `oracle/stages/plumber_doc.py`.

use super::Stage;

pub const STAGE: Stage = Stage { name: "plumber_doc", run: None, oracle_input: false };
