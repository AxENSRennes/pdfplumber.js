//! The stage registry: one file per stage of `docs/verification.md`
//! ("Étapes"), each with the same name as its Python twin
//! `oracle/stages/<name>.py`.
//!
//! Every stage file exports `pub const STAGE: Stage`. To implement a stage,
//! edit only its file: write a `run` function and set `run: Some(run)`. A
//! new stage is a new file plus one line in [`STAGES`].
//!
//! A stage writes its trace lines to `out` (UTF-8, each ending with `\n`)
//! with [`crate::canon`], and ends with `!end`, or with the line of the
//! exception that interrupted it (`canon::canon_exc`).

use std::io::Write;

pub mod fonts;
pub mod interp;
pub mod layout;
pub mod objects;
pub mod pages;
pub mod plumber_doc;
pub mod plumber_objects;
pub mod plumber_tables;
pub mod plumber_text;
pub mod psparser;
pub mod streams;

/// What a stage reads: the PDF and the trace options.
pub struct StageInput<'a> {
    /// The bytes of the PDF file.
    pub pdf: &'a [u8],
    /// `--password` (empty by default, like `pdfplumber.open`).
    pub password: &'a str,
    /// `--max-pages`: page-by-page stages stop after this many pages
    /// (`None`: every page).
    pub max_pages: Option<usize>,
}

/// Writes the trace of one PDF.
pub type StageFn = fn(&StageInput<'_>, &mut dyn Write) -> std::io::Result<()>;

/// A registered stage.
pub struct Stage {
    pub name: &'static str,
    /// `None` until the stage is implemented.
    pub run: Option<StageFn>,
}

/// Every stage, in pipeline order.
pub const STAGES: &[Stage] = &[
    psparser::STAGE,
    objects::STAGE,
    streams::STAGE,
    pages::STAGE,
    fonts::STAGE,
    interp::STAGE,
    layout::STAGE,
    plumber_objects::STAGE,
    plumber_text::STAGE,
    plumber_tables::STAGE,
    plumber_doc::STAGE,
];

/// The stage called `name`.
pub fn lookup(name: &str) -> Option<&'static Stage> {
    STAGES.iter().find(|s| s.name == name)
}
