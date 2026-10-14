//! Port of `pdfminer/pdfexceptions.py`.
//!
//! The classes are kinds of `pycompat::PyErr` (`ExcKind`), with their
//! Python bases (`PDFTypeError` is both a `PDFException` and a
//! `TypeError`).

pub use pycompat::ExcKind::{
    PDFEOFError, PDFException, PDFIOError, PDFKeyError, PDFNotImplementedError, PDFObjectNotFound, PDFTypeError,
    PDFValueError,
};
