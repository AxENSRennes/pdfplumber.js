//! `io.BytesIO` over shared, immutable bytes (CPython 3.12
//! `Modules/_io/bytesio.c`), the file object pdfminer's parsers read.
//!
//! `read()` returns a range of the shared buffer instead of a copy, so a
//! parser can keep its current chunk without allocating.
//!
//! pdfplumber opens a path with `open(path, "rb")`, a `BufferedReader`,
//! which only differs from `BytesIO` in the exception raised by a negative
//! `seek()` (`OSError` instead of `ValueError`); the parser that seeks to
//! computed offsets (`pdfparser.py`) must take this into account.

use std::ops::Range;
use std::rc::Rc;

use crate::err::{exc, PyResult};

#[derive(Clone, Debug)]
pub struct BytesIO {
    data: Rc<[u8]>,
    pos: usize,
}

impl BytesIO {
    pub fn new(data: Rc<[u8]>) -> BytesIO {
        BytesIO { data, pos: 0 }
    }

    /// The whole underlying buffer (`getvalue()`, without copying).
    pub fn data(&self) -> &Rc<[u8]> {
        &self.data
    }

    /// `tell()`
    pub fn tell(&self) -> usize {
        self.pos
    }

    /// `seek(pos)` (whence 0): a negative position raises `ValueError`;
    /// any other position is accepted, even past the end.
    pub fn seek(&mut self, pos: i64) -> PyResult<usize> {
        self.seek_whence(pos, 0)
    }

    /// `seek(pos, whence)` (`_io_BytesIO_seek_impl`).
    pub fn seek_whence(&mut self, pos: i64, whence: i32) -> PyResult<usize> {
        let mut pos = i128::from(pos);
        match whence {
            0 => {
                if pos < 0 {
                    return Err(exc::ValueError(format!("negative seek value {pos}")));
                }
            }
            1 => pos += self.pos as i128,
            2 => pos += self.data.len() as i128,
            _ => return Err(exc::ValueError(format!("invalid whence ({whence}, should be 0, 1 or 2)"))),
        }
        self.pos = usize::try_from(pos.max(0)).unwrap_or(usize::MAX);
        Ok(self.pos)
    }

    /// `read(n)` (`None`, like a negative `n` in Python, reads to the end):
    /// the range of [`Self::data`] that Python would return. Reading at or
    /// past the end returns an empty range and leaves the position
    /// unchanged.
    pub fn read(&mut self, n: Option<usize>) -> Range<usize> {
        let avail = self.data.len().saturating_sub(self.pos);
        let n = n.map_or(avail, |n| n.min(avail));
        let start = self.pos.min(self.data.len());
        if n == 0 {
            return start..start;
        }
        self.pos += n;
        start..start + n
    }

    /// `read(n)` returning the bytes.
    pub fn read_bytes(&mut self, n: Option<usize>) -> &[u8] {
        let range = self.read(n);
        &self.data[range]
    }
}
