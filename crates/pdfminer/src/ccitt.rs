//! Port of `pdfminer/ccitt.py`: not ported yet.
//!
//! Only the signature `pdftypes::PDFStream.decode` calls is here, as a stub
//! (the module is ported separately and replaces this file).

use pycompat::{PyErr, PyResult};

use crate::pdftypes::PDFDict;

// pdfminer/ccitt.py:556 ccittfaxdecode
/// Stub: raises `NotPorted` until `ccitt.py` is ported.
pub fn ccittfaxdecode(_data: &[u8], _params: &PDFDict) -> PyResult<Vec<u8>> {
    Err(PyErr::not_ported("pdfminer.ccitt.ccittfaxdecode"))
}
