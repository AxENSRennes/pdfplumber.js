//! Stage `objects`: xref, trailer, every indirect object resolved
//! (`getobj`), errors included. The traversal and the line format are
//! those of `oracle/stages/objects.py`.

use std::io::{self, Write};

use std::rc::Rc;

use pdfminer::pdfdocument::{DocId, PDFDocument, XRefPos};
use pdfminer::pdfparser::FileKind;
use pdfminer::pdftypes::{ObjId, PDFObject};
use pycompat::PyResult;

use super::common::{canon_id, fetch_order, open_document, open_file, terminated, Lines};
use super::{Stage, StageInput};
use crate::canon::{canon, canon_exc, canon_obj};
use crate::value::Value;

pub const STAGE: Stage = Stage { name: "objects", run: Some(run) };

/// `canon(xref.get_pos(objid))`: `(strmid, index, genno)`.
pub fn canon_pos(pos: &XRefPos) -> String {
    let strmid = pos.strmid.clone().map_or(Value::None, Value::Int);
    canon(&Value::Tuple(vec![strmid, Value::Int(pos.index.clone()), Value::Int(pos.genno.clone())]))
}

/// `canon(doc.encryption)`
pub fn canon_encryption(doc: &PDFDocument) -> String {
    match doc.encryption() {
        None => "None".to_owned(),
        Some((id, param)) => {
            let id = match id {
                DocId::List(l) => Value::from(&PDFObject::List(l)),
                DocId::Default => Value::Tuple(vec![Value::Bytes(Vec::new()), Value::Bytes(Vec::new())]),
            };
            canon(&Value::Tuple(vec![id, Value::from(&PDFObject::Dict(param))]))
        }
    }
}

fn py_bool(b: bool) -> &'static str {
    if b {
        "True"
    } else {
        "False"
    }
}

fn run(input: &StageInput<'_>, out: &mut dyn Write) -> io::Result<()> {
    trace(input, out, open_document)
}

/// The traversal of the stage on a parser reading an `io.BytesIO` (what
/// `oracle/unit/document.py` `objects_bytesio` writes).
pub fn run_bytesio(input: &StageInput<'_>, out: &mut dyn Write) -> io::Result<()> {
    trace(input, out, |input| open_file(input, FileKind::BytesIO))
}

fn trace(
    input: &StageInput<'_>,
    out: &mut dyn Write,
    open: impl FnOnce(&StageInput<'_>) -> PyResult<Rc<PDFDocument>>,
) -> io::Result<()> {
    terminated(out, |lines: &mut Lines| {
        let doc = open(input)?;
        let mut objids: Vec<ObjId> = Vec::new();
        for (i, xref) in doc.xrefs().iter().enumerate() {
            let i = i.to_string();
            lines.line("xref", &[&i, xref.class_name()]);
            lines.line("trailer", &[&i, &canon_obj(&PDFObject::Dict(xref.get_trailer()))]);
            for objid in xref.get_objids() {
                let objid = match objid {
                    Ok(objid) => objid,
                    Err(e) => {
                        lines.line("entries", &[&i, &canon_exc(&e)]);
                        break;
                    }
                };
                objids.push(objid.clone());
                let pos = match xref.get_pos(&objid) {
                    Ok(pos) => canon_pos(&pos),
                    Err(e) => canon_exc(&e),
                };
                lines.line("entry", &[&i, &canon_id(&objid), &pos]);
            }
        }
        lines.line("catalog", &[&canon_obj(&PDFObject::Dict(doc.catalog()))]);
        let info = doc.info().into_iter().map(|d| Value::from(&PDFObject::Dict(d))).collect();
        lines.line("info", &[&canon(&Value::List(info))]);
        lines.line("encryption", &[&canon_encryption(&doc)]);
        lines.line(
            "permissions",
            &[py_bool(doc.is_printable()), py_bool(doc.is_modifiable()), py_bool(doc.is_extractable())],
        );
        for objid in fetch_order(&objids) {
            match doc.getobj(&objid) {
                Ok(obj) => lines.line("obj", &[&canon_id(&objid), &canon_obj(&obj)]),
                Err(e) => lines.line("obj", &[&canon_id(&objid), &canon_exc(&e)]),
            }
        }
        Ok(())
    })
}
