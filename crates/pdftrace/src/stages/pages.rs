//! Stage `pages`: page tree (inherited attributes, boxes, rotation,
//! resources, contents, labels) and pdfplumber's page boxes. The traversal
//! and the line format are those of `oracle/stages/pages.py`.

use std::io::{self, Write};

use pdfminer::pdfpage::{FloatRect, PDFPage};
use pdfminer::pdftypes::PDFObject;
use pdfplumber::page::{BBox, PageBoxes};
use pycompat::{Num, PyInt, PyResult};

use super::common::{open_document, terminated, Lines};
use super::{Stage, StageInput};
use crate::canon::{canon, canon_exc, canon_obj};
use crate::value::{PyStr, Value};

pub const STAGE: Stage = Stage { name: "pages", run: Some(run) };

fn float_rect(r: &FloatRect) -> String {
    canon(&Value::Tuple(r.iter().map(|&f| Value::Float(f)).collect()))
}

fn bbox(b: &BBox) -> String {
    canon(&Value::Tuple(b.iter().map(Value::from).collect()))
}

fn num(n: &Num) -> String {
    canon(&match n {
        Num::Int(i) => Value::Int(i.clone()),
        Num::Float(f) => Value::Float(*f),
    })
}

/// The pdfminer lines of a page.
fn pdfminer_lines(lines: &mut Lines, page: &PDFPage) {
    let label = page.label.as_ref().map_or(Value::None, |l| Value::Str(PyStr::from(l.as_str())));
    lines.line("label", &[&canon(&label)]);
    lines.line("attrs", &[&canon_obj(&PDFObject::Dict(page.attrs.clone()))]);
    lines.line("lastmod", &[&canon_obj(&page.lastmod)]);
    lines.line("resources", &[&canon_obj(&page.resources)]);
    lines.line("mediabox", &[&float_rect(&page.mediabox)]);
    lines.line("cropbox", &[&float_rect(&page.cropbox)]);
    lines.line("rotate", &[&page.rotate.to_string()]);
    lines.line("contents", &[&canon_obj(&PDFObject::List(page.contents.clone()))]);
    lines.line("annots", &[&canon_obj(&page.annots)]);
    lines.line("beads", &[&canon_obj(&page.beads)]);
}

/// The values of the `plumber` lines, or the exception of `Page(...)`.
fn plumber_values(page: &PDFPage, doctop: &Num) -> PyResult<(Vec<(&'static str, String)>, Num)> {
    let p = PageBoxes::new(page, doctop.clone())?;
    let mut values =
        vec![("rotation", canon_obj(&p.rotation)), ("mediabox", bbox(&p.mediabox)), ("cropbox", bbox(&p.cropbox))];
    for (name, b) in [("trimbox", &p.trimbox), ("bleedbox", &p.bleedbox), ("artbox", &p.artbox)] {
        if let Some(b) = b {
            values.push((name, bbox(b)));
        }
    }
    values.push(("bbox", bbox(&p.bbox)));
    values.push(("initial_doctop", num(&p.initial_doctop)));
    values.push(("width", num(&p.width()?)));
    let height = p.height()?;
    values.push(("height", num(&height)));
    Ok((values, height))
}

fn run(input: &StageInput<'_>, out: &mut dyn Write) -> io::Result<()> {
    terminated(out, |lines: &mut Lines| {
        let doc = open_document(input)?;
        let mut doctop = Num::Int(PyInt::ZERO);
        for (pageno, page) in PDFPage::create_pages(&doc).enumerate() {
            let pageno = pageno + 1;
            let page = page?;
            lines.line("page", &[&pageno.to_string(), &canon_obj(&page.pageid)]);
            pdfminer_lines(lines, &page);
            match plumber_values(&page, &doctop) {
                Ok((values, height)) => {
                    for (name, value) in values {
                        lines.line(&format!("plumber {name}"), &[&value]);
                    }
                    doctop = doctop.add(&height)?;
                }
                Err(e) => lines.push(format!("plumber {}", canon_exc(&e))),
            }
            if input.max_pages.is_some_and(|max| pageno >= max) {
                break;
            }
        }
        Ok(())
    })
}
