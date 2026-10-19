//! Stage `layout`: the `LTPage` of each page, raw (`laparams=None`) and
//! analyzed with `LAParams()`, every attribute included. The traversal and
//! the line format are those of `oracle/stages/layout.py`; the lines are
//! written by [`crate::layout_dump`], which `tests/converter_unit.rs`
//! checks against Python on device-call sequences replayed without the
//! interpreter (`oracle/unit/converter.py`).
//!
//! For each page, the body of pdfplumber/page.py:256 `Page.layout` with
//! `laparams=None` (`common::page_layout`: one frame;
//! `PDFPageAggregatorWithMarkedContent::create`, `PDFPageInterpreter`,
//! `process_page`), `get_result()`, the raw lines; then the analysis of
//! that page with `LAParams()` two frames further down (`_process_page`,
//! `_end_page` of the Python stage, standing for `process_page` and
//! `end_page`), with the `id()` order of the oracle's trace when
//! `--oracle-dir` gives it ([`oracle_ids`], one entry per `ids` line, taken
//! in order by the pages whose analysis runs; `RecordedIds`), else the
//! library's `CreationOrder`; the analyzed lines (or the `analyze` line);
//! the `ids` line of the Rust analysis (ranked by the ids it used).
//!
//! Inline images: Python's trace names them `inline{"n": k}` by their
//! name `str(id(stream))`; here they are the names the interpreter gave
//! (`pdfinterp::inline_image_name(k)`, `k` below
//! `PDFPageInterpreter::inline_images`): the two agree but on a PDF that
//! names an XObject like the port names inline images (`inline-0`)
//! (`docs/architecture.md`, "Vague 3").
//!
//! The statistic of `docs/architecture.md` ("Vague 3": how often the
//! library's default order, `CreationOrder`, gives Python's result) is
//! this stage run without `--oracle-dir`, compared page by page with the
//! oracle's trace, the `ids` lines left out.

use std::collections::HashSet;
use std::io::{self, Write};
use std::rc::Rc;

use pdfminer::layout::{CreationOrder, IdSource, LAParams};
use pdfminer::pdfinterp::{inline_image_name, PDFResourceManager};
use pdfminer::pdfpage::PDFPage;
use pdfplumber::page::{PDFPageAggregatorWithMarkedContent, PlumberPayload};
use pycompat::recursion::{leaf, py_frames};
use pycompat::{PyInt, PyResult};

use super::common::{open_document, page_layout, terminated, Lines};
use super::{Stage, StageInput};
use crate::canon::{canon_exc, canon_obj};
use crate::layout_dump::{analyzed, id_ranks, ids_line, raw_index, RawDump, RecordedIds};
use crate::typed_json::{parse_json, Json};

pub const STAGE: Stage = Stage { name: "layout", run: Some(run), oracle_input: true };

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

/// `_page(rsrcmgr, page, pageno)` of the Python stage: the lines of one
/// page.
fn page_lines(
    lines: &mut Lines,
    rsrcmgr: &Rc<PDFResourceManager>,
    page: &PDFPage,
    pageno: usize,
    oracle: &mut Option<std::vec::IntoIter<IdRanks>>,
) -> PyResult<()> {
    let pageno_int = PyInt::Small(i64::try_from(pageno).expect("a page number"));
    let (device, interpreter, error) = page_layout(rsrcmgr, page, || {
        PDFPageAggregatorWithMarkedContent::create(rsrcmgr.clone(), pageno_int.clone(), None)
    })?;
    if let Some(e) = error {
        lines.push(format!("error {}", canon_exc(&e)));
        return Ok(());
    }
    // ltpage = device.get_result(), from _page
    leaf(2)?; // _page, get_result
    let Some(mut ltpage) = device.borrow_mut().aggregator.result.take() else {
        unreachable!("process_page returned: end_page gave the result")
    };
    let inline: HashSet<String> = (0..interpreter.inline_images.get()).map(inline_image_name).collect();
    let is_inline = |name: &str| inline.contains(name);
    let raw = RawDump::new(&ltpage.layout._objs, &is_inline);
    let index = raw.index.clone();
    for l in RawDump::new(&ltpage.layout._objs, &is_inline).write(&ltpage) {
        lines.push(l);
    }
    let mut ids: Box<dyn IdSource<PlumberPayload>> = match oracle {
        Some(pages) => Box::new(RecordedIds::new(&pages.next().unwrap_or_default(), raw_index(&ltpage.layout._objs))),
        None => Box::new(CreationOrder::default()),
    };
    // _page -> _process_page -> _end_page -> ltpage.analyze(LAParams())
    let analysis = {
        let _frames = py_frames(3)?;
        ltpage.analyze_with_ids(&LAParams::default(), &mut *ids)
    };
    match analysis {
        Err(e) => lines.push(format!("analyze {}", canon_exc(&e))),
        Ok(()) => {
            for l in analyzed(&ltpage, &raw) {
                lines.push(l);
            }
        }
    }
    lines.push(ids_line(&id_ranks(&ltpage, &index, &mut *ids)));
    Ok(())
}

fn run(input: &StageInput<'_>, out: &mut dyn Write) -> io::Result<()> {
    let mut oracle = match input.oracle.map(oracle_ids).transpose() {
        Ok(pages) => pages.map(Vec::into_iter),
        Err(e) => return Err(io::Error::new(io::ErrorKind::InvalidData, format!("the oracle's trace: {e}"))),
    };
    terminated(out, |lines: &mut Lines| {
        let doc = open_document(input)?;
        let rsrcmgr = Rc::new(PDFResourceManager::new(true));
        for (pageno, page) in PDFPage::create_pages(&doc).enumerate() {
            let pageno = pageno + 1;
            let page = page?;
            lines.line("page", &[&pageno.to_string(), &canon_obj(&page.pageid)]);
            page_lines(lines, &rsrcmgr, &page, pageno, &mut oracle)?;
            if input.max_pages.is_some_and(|max| pageno >= max) {
                break;
            }
        }
        Ok(())
    })
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
