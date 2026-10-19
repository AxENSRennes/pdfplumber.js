//! Stage `streams`: each stream's filters, parameters and decoded data, or
//! the exception. The traversal and the line format are those of
//! `oracle/stages/streams.py`.

use std::io::{self, Write};

use pdfminer::pdftypes::{ObjId, PDFObject};

use super::common::{canon_id, fetch_order, open_document, terminated, Lines};
use super::{Stage, StageInput};
use crate::canon::{canon, canon_exc, canon_obj, write_bytes};
use crate::value::Value;

pub const STAGE: Stage = Stage { name: "streams", run: Some(run), oracle_input: false };

fn run(input: &StageInput<'_>, out: &mut dyn Write) -> io::Result<()> {
    terminated(out, |lines: &mut Lines| {
        let doc = open_document(input)?;
        let mut objids: Vec<ObjId> = Vec::new();
        for xref in doc.xrefs() {
            // an error ends the listing of that xref (written by the
            // objects stage); the ids listed so far are kept
            objids.extend(xref.get_objids().map_while(Result::ok));
        }
        for objid in fetch_order(&objids) {
            let obj = match doc.getobj(&objid) {
                Ok(obj) => obj,
                Err(e) => {
                    lines.line("getobj", &[&canon_id(&objid), &canon_exc(&e)]);
                    continue;
                }
            };
            let PDFObject::Stream(stream) = &obj else { continue };
            let id = canon_id(&objid);
            lines.line("stream", &[&id, &canon_obj(&obj)]);
            let filters = match stream.get_filters() {
                Ok(filters) => canon(&Value::List(
                    filters.iter().map(|(f, p)| Value::Tuple(vec![Value::from(f), Value::from(p)])).collect(),
                )),
                Err(e) => canon_exc(&e),
            };
            lines.line("filters", &[&id, &filters]);
            let data = match stream.get_data() {
                Ok(data) => {
                    let mut text = String::new();
                    write_bytes(&mut text, &data);
                    text
                }
                Err(e) => canon_exc(&e),
            };
            lines.line("data", &[&id, &data]);
        }
        Ok(())
    })
}
