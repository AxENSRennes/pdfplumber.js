//! Stage `layout`: the whole `LTPage` tree, with and without `laparams`.
//!
//! Not implemented yet. The line format is documented in
//! `oracle/stages/layout.py`.

use super::Stage;

pub const STAGE: Stage = Stage { name: "layout", run: None };
