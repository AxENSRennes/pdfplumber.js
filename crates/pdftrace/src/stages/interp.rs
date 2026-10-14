//! Stage `interp`: device calls (`begin_figure`, `paint_path`, `render_char`, `render_image`, tags) with the graphic state.
//!
//! Not implemented yet. The line format is documented in
//! `oracle/stages/interp.py`.

use super::Stage;

pub const STAGE: Stage = Stage { name: "interp", run: None };
