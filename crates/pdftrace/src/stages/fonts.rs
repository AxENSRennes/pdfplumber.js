//! Stage `fonts`: the fonts the interpreter obtains for the pages and, for
//! every string it shows with them, `decode`, `to_unichr`, `char_width`,
//! `char_disp`.
//!
//! Not implemented yet (wave 3, workstream INTERP). The traversal and the
//! line format are those of `oracle/stages/fonts.py`.
//!
//! # The runner, once the interpreter is ported
//!
//! ```text
//! terminated(out, |lines| {
//!     let doc = open_document(input)?;
//!     let rsrcmgr = Rc::new(PDFResourceManager::new(true));
//!     for (pageno, page) in PDFPage::create_pages(&doc).enumerate() {   // from 1
//!         page line
//!         let ncached = rsrcmgr.cached_fonts().len();
//!         let (device, interpreter, error) = common::page_layout(&rsrcmgr, &page,
//!             || Ok(FontsDevice::new(rsrcmgr.clone(), pageno)))?;
//!         fontmap lines: interpreter.fontmap (None: init_resources not reached)
//!         cached lines: rsrcmgr.cached_fonts()[ncached..]
//!         string/cid lines: the bytes items of each recorded seq, as Python
//!         font lines first, fonts numbered by identity (Rc::as_ptr, the Rc kept)
//!         error line; stop after max_pages
//!     }
//! })
//! ```
//!
//! `FontsDevice` is a subclass of `PDFPageAggregatorWithMarkedContent`
//! (like `interp::TracingDevice`) whose `render_string` takes its frame,
//! records `(textstate.font, seq)` and calls
//! `pdfdevice::text_device::render_string`. The font report (`font_attrs`)
//! is the one `tests/fonts_unit.rs` writes for `oracle/unit/fonts.py`: move
//! it into the library and share it.

use super::Stage;

pub const STAGE: Stage = Stage { name: "fonts", run: None, oracle_input: false };
