//! Port of `pdfplumber/utils/exceptions.py`: pdfplumber's two exception
//! classes, `MalformedPDFException` and `PdfminerException` (both
//! `Exception` subclasses; `pycompat::ExcKind` has them, with their
//! module, for the error model and the JavaScript MRO).
//!
//! Both are raised with another exception as their single argument
//! (`raise PdfminerException(e)`, `raise MalformedPDFException(e)`): `str()`
//! of the wrapper is `str(e)`, and the wrapped exception is what a user
//! inspects (`e.args[0]`, in JavaScript `error.cause`). [`wrap`] keeps it
//! ([`pycompat::PyErr::cause`]); the stage traces write it after the
//! wrapper (`exc` of `oracle/stages/plumber_objects.py`: `!PdfminerException("...")
//! cause !PDFSyntaxError("...")`).
//!
//! `MalformedPDFException(msg)` raised with a message (`_normalize_box`) is
//! `PyErr::new(ExcKind::MalformedPDFException, msg)`; `resolve_all`
//! (`pdfinternals.rs`) raises `MalformedPDFException(e)` with [`wrap`] (the
//! cause is only written by the `plumber_*` stages: the traces of the
//! earlier stages do not change).

use pycompat::{ExcKind, PyErr};

// pdfplumber/utils/exceptions.py:1 MalformedPDFException
// pdfplumber/utils/exceptions.py:5 PdfminerException
/// `kind(e)`: the exception `kind` raised with `e` as its single argument
/// (`str()` is `str(e)`; the cause is kept).
pub fn wrap(kind: ExcKind, e: PyErr) -> PyErr {
    PyErr::wrapping(kind, e)
}

/// `PdfminerException(e)` (pdfplumber/pdf.py:28 `PDF.__init__`, :145
/// `PDF.pages`, pdfplumber/page.py:256 `Page.layout`).
pub fn pdfminer_exception(e: PyErr) -> PyErr {
    wrap(ExcKind::PdfminerException, e)
}

/// `MalformedPDFException(e)`
/// (pdfplumber/utils/pdfinternals.py:66 `resolve_all`).
pub fn malformed_pdf_exception(e: PyErr) -> PyErr {
    wrap(ExcKind::MalformedPDFException, e)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pycompat::exc;

    #[test]
    fn a_wrapper_keeps_the_message_and_the_cause() {
        let e = pdfminer_exception(exc::ValueError("bad"));
        assert_eq!(e.kind, ExcKind::PdfminerException);
        assert_eq!(e.message, "bad");
        assert_eq!(e.cause().map(|c| c.kind), Some(ExcKind::ValueError));
        let m = malformed_pdf_exception(e.clone());
        assert_eq!(m.cause().and_then(PyErr::cause).map(|c| c.kind), Some(ExcKind::ValueError));
    }
}
