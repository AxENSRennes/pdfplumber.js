//! Stage `fonts`: every font used: type, metrics, and `decode`, `to_unichr`, `char_width`, `char_disp` for each code met.
//!
//! Not implemented yet. The line format is documented in
//! `oracle/stages/fonts.py`.

use super::Stage;

pub const STAGE: Stage = Stage { name: "fonts", run: None };
