//! Stage `plumber_doc`: metadata (`strict_metadata` both ways), the
//! structure tree (document and pages: `to_dict`, `find_all`, `find`,
//! `all_mcids`, `element_bbox`, `repr`), derived pages (`crop`,
//! `within_bbox`, `outside_bbox`, strict or not, relative or not,
//! `filter`, `dedupe_chars`: boxes, objects, annotations), serialization
//! (`to_dict`, `to_json`, `to_csv`), `close`, the `open` options
//! (`unicode_norm`, `raise_unicode_errors`, `pages`) and passwords. The
//! traversal and the line format are those of
//! `oracle/stages/plumber_doc.py`.
//!
//! Not implemented yet (wave 4, workstream DOC; derived pages and
//! serialization call workstream PAGE's `Page` and `Container`). The
//! runner, with `plumber_common`: `PDF::open` (and again for each of
//! `plumber_ops::DOC_OPEN_OPTIONS` and the passwords), `pdf.metadata`,
//! `pdf.structure_tree()`, `PDFStructTree::new(doc, Scope::Document(..))`
//! and `Scope::Page(..)` (`Matcher::Tag` of the first element's type and
//! of `Span`, `Matcher::Func` for `starts-with-H`), `DOC_DERIVED_BASE` and
//! a `select` of `DOC_DERIVED`, a `select` of `DOC_SERIALIZE`
//! (`Container::to_dict`, `to_json`, `to_csv` with `SerializeOptions`),
//! `pdf.to_dict(...)`, `pdf.close()` and the objects of the first page of
//! `pdf.pages()` built again.

use super::Stage;

pub const STAGE: Stage = Stage { name: "plumber_doc", run: None, oracle_input: false };
