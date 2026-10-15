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

/// A file position: Python's `Py_ssize_t`, 64 bits on every target (a seek
/// past 4 GiB keeps its value on wasm32 too).
pub type Pos = i64;

#[derive(Clone, Debug)]
pub struct BytesIO {
    data: Rc<[u8]>,
    /// `self->pos`; never negative, may be past the end.
    pos: Pos,
}

impl BytesIO {
    pub fn new(data: Rc<[u8]>) -> BytesIO {
        BytesIO { data, pos: 0 }
    }

    /// The whole underlying buffer (`getvalue()`, without copying).
    pub fn data(&self) -> &Rc<[u8]> {
        &self.data
    }

    /// The length of the data, as a position.
    fn size(&self) -> Pos {
        Pos::try_from(self.data.len()).expect("a buffer is shorter than 2**63 bytes")
    }

    /// `tell()`
    pub fn tell(&self) -> Pos {
        self.pos
    }

    /// `seek(pos)` (whence 0): a negative position raises `ValueError`;
    /// any other position is accepted, even past the end.
    pub fn seek(&mut self, pos: Pos) -> PyResult<Pos> {
        self.seek_whence(pos, 0)
    }

    /// `seek(pos, whence)` (`_io_BytesIO_seek_impl`).
    pub fn seek_whence(&mut self, pos: Pos, whence: i32) -> PyResult<Pos> {
        let too_large = || exc::OverflowError("new position too large");
        let pos = match whence {
            0 if pos < 0 => return Err(exc::ValueError(format!("negative seek value {pos}"))),
            0 => pos,
            1 => pos.checked_add(self.pos).ok_or_else(too_large)?,
            2 => pos.checked_add(self.size()).ok_or_else(too_large)?,
            _ => return Err(exc::ValueError(format!("invalid whence ({whence}, should be 0, 1 or 2)"))),
        };
        self.pos = pos.max(0);
        Ok(self.pos)
    }

    /// `read(n)` (`None`, like a negative `n` in Python, reads to the end):
    /// the range of [`Self::data`] that Python would return. Reading at or
    /// past the end returns an empty range and leaves the position
    /// unchanged.
    pub fn read(&mut self, n: Option<usize>) -> Range<usize> {
        // The position is within the data whenever there is anything to read.
        let start = usize::try_from(self.pos).map_or(self.data.len(), |p| p.min(self.data.len()));
        let avail = self.data.len() - start;
        let n = n.map_or(avail, |n| n.min(avail));
        if n == 0 {
            return start..start;
        }
        self.pos += n as Pos;
        start..start + n
    }

    /// `read(n)` returning the bytes.
    pub fn read_bytes(&mut self, n: Option<usize>) -> &[u8] {
        let range = self.read(n);
        &self.data[range]
    }
}
