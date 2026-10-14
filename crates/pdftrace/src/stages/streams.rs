//! Stage `streams`: every stream: filters, parameters, decoded data or exception.
//!
//! Not implemented yet. The line format is documented in
//! `oracle/stages/streams.py`.

use super::Stage;

pub const STAGE: Stage = Stage { name: "streams", run: None };
