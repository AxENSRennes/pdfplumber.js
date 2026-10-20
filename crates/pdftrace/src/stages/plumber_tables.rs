//! Stage `plumber_tables`: the table finder for the `lines`,
//! `lines_strict`, `text` and `explicit` strategies and setting variants,
//! on the page and on derived pages (`debug_tablefinder`, `find_table`,
//! `extract_tables`, `extract_table`). The traversal and the line format
//! are those of `oracle/stages/plumber_tables.py`.
//!
//! Not implemented yet (wave 4, workstream PAGE). The runner, with
//! `plumber_common`: `page.edges()`, `horizontal_edges()`,
//! `vertical_edges()` (digests), then the ops of
//! `plumber_ops::TABLES_BASE_OPS` and a `select` of `TABLES_VARIANT_OPS`
//! (`TABLES_VARIANTS_PER_PAGE`), the settings of an `explicit` op built
//! from the page (`settings_of`), each through `Page::debug_tablefinder`
//! (`TableFinder`: edges, intersections, cells, tables with their rows,
//! columns and `Table::extract`), `find_table`, `extract_tables`,
//! `extract_table`; then one of `TABLES_DERIVED_OPS` on its derived page.

use super::Stage;

pub const STAGE: Stage = Stage { name: "plumber_tables", run: None, oracle_input: false };
