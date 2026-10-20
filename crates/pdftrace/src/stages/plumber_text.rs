//! Stage `plumber_text`: `extract_text` (simple, `layout=True`, option
//! sets), `extract_text_simple`, `extract_words`, `extract_text_lines`,
//! `search`, `dedupe_chars`, on the page and on derived pages. The
//! traversal and the line format are those of
//! `oracle/stages/plumber_text.py`.
//!
//! Not implemented yet (wave 4, workstream PAGE). The runner, with
//! `plumber_common`: the ops of `plumber_ops::TEXT_BASE_OPS` and a
//! `select` of `TEXT_VARIANT_OPS` (`TEXT_VARIANTS_PER_PAGE`), then
//! `TEXT_DERIVED_OPS` on a `select` of `TEXT_DERIVED`
//! (`TEXT_DERIVED_PER_PAGE`), each op through the `Page` methods
//! (`extract_text(kwargs)`, `search(matches, args, kwargs)`,
//! `dedupe_chars(kwargs)` written as the indices of its chars in
//! `page.chars` by identity, `Obj::ptr_eq`), a derivation through `crop`,
//! `filter` (the predicates `upright`, `not-rect`, `small` of the Python
//! stage, as `FilterFn`s) and `dedupe_chars`, the box of a step computed
//! with `PyValue` arithmetic as `derive_bbox` does. `search`: the matches
//! are the spans of the oracle's `spans <i>` line for that op
//! (`StageInput::oracle`, `ORACLE_INPUT`), written back as they are.

use super::Stage;

pub const STAGE: Stage = Stage { name: "plumber_text", run: None, oracle_input: true };
