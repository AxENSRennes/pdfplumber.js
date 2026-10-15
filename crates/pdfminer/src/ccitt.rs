//! Port of `pdfminer/ccitt.py`: not ported yet.
//!
//! Only the signatures `pdftypes::PDFStream.decode` calls are here, as a
//! stub (the module is ported separately and replaces this file). The
//! dispatch on the type of `params` lives in `ccittfaxdecode_obj`, so that
//! `decode` keeps calling it unchanged once the port lands.

use pycompat::recursion::py_frame;
use pycompat::{exc, PyErr, PyResult};

use crate::pdftypes::{PDFDict, PDFObject};

// pdfminer/ccitt.py:556 ccittfaxdecode
/// Stub: raises `NotPorted` until `ccitt.py` is ported.
pub fn ccittfaxdecode(_data: &[u8], _params: &PDFDict) -> PyResult<Vec<u8>> {
    let _frame = py_frame()?;
    Err(PyErr::not_ported("pdfminer.ccitt.ccittfaxdecode"))
}

// pdfminer/ccitt.py:556 ccittfaxdecode
/// `ccittfaxdecode(data, params)` for any object, as `PDFStream.decode`
/// passes it (the resolved `DecodeParms` entry of the filter): a `dict`; a
/// `PDFStream`, whose `get` reads its attributes; any other object raises
/// `AttributeError` on `params.get("K")`. Stub: `NotPorted` for a `dict`
/// or a stream.
pub fn ccittfaxdecode_obj(data: &[u8], params: &PDFObject) -> PyResult<Vec<u8>> {
    match params {
        PDFObject::Dict(d) => ccittfaxdecode(data, d),
        PDFObject::Stream(s) => ccittfaxdecode(data, &s.attrs),
        other => {
            let _frame = py_frame()?;
            Err(exc::AttributeError(format!("'{}' object has no attribute 'get'", other.type_name())))
        }
    }
}
