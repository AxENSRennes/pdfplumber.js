//! Stage `layout`: the `LTPage` of each page, raw (`laparams=None`) and
//! analyzed with `LAParams()`, every attribute included.
//!
//! Not implemented yet (wave 3, workstream CONVERTER). The traversal and
//! the line format are those of `oracle/stages/layout.py`. Implemented
//! here: reading Python's `id()` order from the oracle's trace
//! ([`oracle_ids`]).
//!
//! # The runner, once the converter is ported
//!
//! ```text
//! let ids = input.oracle.map(oracle_ids);          // one entry per `ids` line
//! terminated(out, |lines| {
//!     let doc = open_document(input)?;
//!     let rsrcmgr = Rc::new(PDFResourceManager::new(true));
//!     for (pageno, page) in PDFPage::create_pages(&doc).enumerate() {   // from 1
//!         page line
//!         _page(...): one frame (Page.layout); PDFPageAggregatorWithMarkedContent
//!             (laparams None), PDFPageInterpreter::new, process_page (error line
//!             and next page if it raised), the raw lines; then two frames
//!             (_process_page, _end_page) and
//!             ltpage.analyze_with_ids(&LAParams::default(), &mut recorded)
//!             where `recorded` is the IdSource of the next `ids` entry (the
//!             ids of this page's analysis), or CreationOrder without
//!             --oracle-dir; the analyzed lines (or the analyze line); the
//!             ids line of the Rust result
//!         stop after max_pages
//!     }
//! })
//! ```
//!
//! The `IdSource` keyed by `(min_char, n_boxes)` is the replay's
//! `RecordedIds` (`tests/layout_unit.rs`), to move here and share; a key
//! that Python's ranking does not have (the Rust analysis built another
//! box or group) takes an id after every recorded one, in creation order,
//! and the trace then differs at the latest on the `ids` line. The dump of
//! a raw or analyzed object is shared with `layout_unit.rs` where it can
//! be (the same `__dict__` order: `LTItem::dict_keys`).
//!
//! The statistic of `docs/architecture.md` ("Vague 3": how often the
//! library's default order, `CreationOrder`, gives Python's result) is
//! this stage run without `--oracle-dir` on the set `all`, compared page by
//! page with the oracle once the stage is identical with it.

use super::Stage;
use crate::typed_json::{parse_json, Json};

pub const STAGE: Stage = Stage { name: "layout", run: None, oracle_input: true };

/// The `[min_char, n_boxes, rank]` items of an `ids` line.
pub type IdRanks = Vec<[i64; 3]>;

/// The `ids` lines of an oracle trace of the stage, in order (one per page
/// whose analysis ran).
pub fn oracle_ids(trace: &str) -> Result<Vec<IdRanks>, String> {
    let int = |j: &Json| match j {
        Json::Number(n) => n.parse::<i64>().map_err(|e| format!("ids: {n}: {e}")),
        other => Err(format!("ids: not a number: {other:?}")),
    };
    let mut pages = Vec::new();
    for line in trace.lines() {
        let Some(list) = line.strip_prefix("ids ") else { continue };
        let json = parse_json(list)?;
        let Json::Array(items) = &json else { return Err(format!("ids: not a list: {line}")) };
        let mut ranks = Vec::with_capacity(items.len());
        for item in items {
            let Json::Array(triple) = item else { return Err(format!("ids: not a list: {item:?}")) };
            let [m, n, r] = triple.as_slice() else { return Err(format!("ids: not a triple: {triple:?}")) };
            ranks.push([int(m)?, int(n)?, int(r)?]);
        }
        pages.push(ranks);
    }
    Ok(pages)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_ids_lines_in_order() {
        let trace = "page 1 3\nids [[0, 1, 0]]\npage 2 4\nobj (0) #{\"i\": 0}\nids []\npage 3 5\nids [[4, 2, 1], [0, 1, 0]]\n!end\n";
        assert_eq!(oracle_ids(trace).unwrap(), vec![vec![[0, 1, 0]], vec![], vec![[4, 2, 1], [0, 1, 0]]]);
        assert!(oracle_ids("ids [[0, 1]]\n").is_err());
        assert!(oracle_ids("ids {}\n").is_err());
    }
}
