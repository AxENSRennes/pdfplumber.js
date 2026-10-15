//! Stage `psparser`: objects `(pos, obj)` produced by `PDFContentParser` on
//! the content streams of each page and Form XObject. The traversal and
//! the line format are those of `oracle/stages/psparser.py`.

use std::collections::HashSet;
use std::io::{self, Write};

use pdfminer::pdfinterp::PDFContentParser;
use pdfminer::pdfpage::PDFPage;
use pdfminer::pdftypes::{dict_value, list_value, stream_value, ObjId, PDFObject};
use pdfminer::psparser::LIT;
use pycompat::PyResult;

use super::common::{canon_id, open_document, terminated, Lines};
use super::{Stage, StageInput};
use crate::canon::{canon, canon_exc, canon_obj};
use crate::value::Value;

pub const STAGE: Stage = Stage { name: "psparser", run: Some(run) };

/// `_parse(streams)`: every object until the exception that ends the job.
fn parse(lines: &mut Lines, streams: Vec<PDFObject>) {
    let mut parser = match PDFContentParser::new(streams) {
        Ok(parser) => parser,
        Err(e) => {
            lines.push(canon_exc(&e));
            return;
        }
    };
    loop {
        match parser.nextobject() {
            Ok((pos, obj)) => lines.line(&pos.to_string(), &[&canon_obj(&obj)]),
            Err(e) => {
                lines.push(canon_exc(&e));
                return;
            }
        }
    }
}

/// `_page_job(page)`
fn page_job(lines: &mut Lines, page: &PDFPage) {
    let streams = (|| -> PyResult<Vec<PDFObject>> {
        let mut streams = Vec::new();
        for obj in list_value(&PDFObject::List(page.contents.clone()))?.to_vec() {
            let stream = stream_value(&obj)?;
            if stream.objid().is_none() {
                continue;
            }
            streams.push(PDFObject::Stream(stream));
        }
        Ok(streams)
    })();
    let streams = match streams {
        Ok(streams) => streams,
        Err(e) => {
            lines.push(canon_exc(&e));
            return;
        }
    };
    let ids = streams
        .iter()
        .map(|s| match s {
            PDFObject::Stream(s) => s.objid().map_or(Value::None, |id| Value::from(id.obj())),
            _ => unreachable!("streams"),
        })
        .collect();
    lines.line("streams", &[&canon(&Value::List(ids))]);
    parse(lines, streams);
}

/// `_xobject_entries(resources)`
fn xobject_entries(resources: &PDFObject) -> PyResult<Vec<(String, PDFObject)>> {
    if !resources.is_truthy() {
        return Ok(Vec::new());
    }
    let mut entries = Vec::new();
    for (k, v) in dict_value(resources)?.items() {
        if &*k == "XObject" {
            entries.extend(dict_value(&v)?.items().into_iter().map(|(k, v)| (k.to_string(), v)));
        }
    }
    Ok(entries)
}

fn path_value(path: &[String]) -> String {
    canon(&Value::List(path.iter().map(|p| Value::str(p.clone())).collect()))
}

/// `_form_jobs(page)`
fn form_jobs(lines: &mut Lines, page: &PDFPage) {
    // pdfminer/pdfinterp.py LITERAL_FORM
    let literal_form = PDFObject::Literal(LIT("Form"));
    let mut visited: HashSet<ObjId> = HashSet::new();
    let mut pending: Vec<(Vec<String>, PDFObject)> = Vec::new();
    let push_children = |lines: &mut Lines, pending: &mut Vec<(Vec<String>, PDFObject)>, resources: &PDFObject, path: &[String]| {
        match xobject_entries(resources) {
            Ok(entries) => {
                for (name, value) in entries.into_iter().rev() {
                    let mut child = path.to_vec();
                    child.push(name);
                    pending.push((child, value));
                }
            }
            Err(e) => lines.line("xobjects", &[&path_value(path), &canon_exc(&e)]),
        }
    };
    push_children(lines, &mut pending, &page.resources, &[]);
    while let Some((path, value)) = pending.pop() {
        let checked = (|| -> PyResult<_> {
            let xobj = stream_value(&value)?;
            let is_form = xobj.get("Subtype", &PDFObject::Null) == literal_form && xobj.contains("BBox");
            Ok((xobj, is_form))
        })();
        let (xobj, is_form) = match checked {
            Ok(v) => v,
            Err(e) => {
                lines.line("form", &[&path_value(&path), &canon_exc(&e)]);
                continue;
            }
        };
        let Some(objid) = xobj.objid() else { continue };
        if !is_form || visited.contains(&objid) {
            continue;
        }
        visited.insert(objid.clone());
        lines.line("form", &[&path_value(&path), &canon_id(&objid)]);
        parse(lines, vec![PDFObject::Stream(xobj.clone())]);
        let resources = (|| -> PyResult<Option<PDFObject>> {
            let xobjres = xobj.get("Resources", &PDFObject::Null);
            if xobjres.is_truthy() {
                Ok(Some(PDFObject::Dict(dict_value(&xobjres)?)))
            } else {
                Ok(None)
            }
        })();
        match resources {
            Ok(Some(resources)) => push_children(lines, &mut pending, &resources, &path),
            Ok(None) => {}
            Err(e) => lines.line("xobjects", &[&path_value(&path), &canon_exc(&e)]),
        }
    }
}

fn run(input: &StageInput<'_>, out: &mut dyn Write) -> io::Result<()> {
    terminated(out, |lines: &mut Lines| {
        let doc = open_document(input)?;
        for (pageno, page) in PDFPage::create_pages(&doc).enumerate() {
            let pageno = pageno + 1;
            let page = page?;
            lines.line("page", &[&pageno.to_string(), &canon_obj(&page.pageid)]);
            page_job(lines, &page);
            form_jobs(lines, &page);
            if input.max_pages.is_some_and(|max| pageno >= max) {
                break;
            }
        }
        Ok(())
    })
}
