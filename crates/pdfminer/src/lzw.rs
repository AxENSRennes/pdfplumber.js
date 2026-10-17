//! Port of `pdfminer/lzw.py`: the LZW decoder of `/LZWDecode`.
//!
//! Python's `LZWDecoder` reads bits from a `BytesIO`; the port reads the
//! same bits from the slice. One optimization, behavior-preserving: the
//! table keeps growing past 4096 entries in Python (it is only reset by a
//! clear code), but a code has at most 12 bits, so an entry at index 4096
//! or above can never be read; the port counts those entries
//! (`table_len`) without storing their bytes, which Python keeps (up to
//! ~4 KB each).

use pycompat::logging::Logger;
use pycompat::recursion::{c_called_leaf, eval_entry, leaf, py_frame};
use pycompat::{exc, PyResult};

/// `logger = logging.getLogger(__name__)` of `pdfminer/lzw.py`.
static LOG: Logger = Logger::new("pdfminer.lzw");

/// Codes are at most 12 bits: entries at this index or above are never
/// read.
const READABLE: usize = 4096;

// pdfminer/lzw.py:15 LZWDecoder
pub struct LZWDecoder<'a> {
    fp: &'a [u8],
    pos: usize,
    buff: u32,
    bpos: u32,
    nbits: u32,
    /// `self.table[:4096]`; `None` only at 256 and 257
    table: Vec<Option<Vec<u8>>>,
    /// `len(self.table)`
    table_len: usize,
    /// `self.prevbuf`: `None` before the first clear code
    prevbuf: Option<Vec<u8>>,
}

/// Why `run` stops reading codes, or an error that escapes it.
enum Stop {
    /// `PDFEOFError` from `readbits`, caught as `EOFError`
    Eof,
}

impl<'a> LZWDecoder<'a> {
    // pdfminer/lzw.py:16 LZWDecoder.__init__
    pub fn new(fp: &'a [u8]) -> LZWDecoder<'a> {
        LZWDecoder { fp, pos: 0, buff: 0, bpos: 8, nbits: 9, table: Vec::new(), table_len: 0, prevbuf: None }
    }

    // pdfminer/lzw.py:25 LZWDecoder.readbits
    fn readbits(&mut self, mut bits: u32) -> Result<u32, Stop> {
        let mut v: u32 = 0;
        loop {
            // the number of remaining bits we can get from the current buffer.
            let r = 8 - self.bpos;
            if bits <= r {
                v = (v << bits) | ((self.buff >> (r - bits)) & ((1 << bits) - 1));
                self.bpos += bits;
                break;
            }
            v = (v << r) | (self.buff & ((1 << r) - 1));
            bits -= r;
            let Some(&x) = self.fp.get(self.pos) else {
                return Err(Stop::Eof); // raise PDFEOFError
            };
            self.pos += 1;
            self.buff = u32::from(x);
            self.bpos = 0;
        }
        Ok(v)
    }

    /// `self.table[code]`: `IndexError` past the end (an entry at 4096 or
    /// above is never asked for, see the module documentation).
    fn entry(&self, code: usize) -> PyResult<Vec<u8>> {
        match self.table.get(code) {
            // (a None entry, 256 or 257, is never asked for: feed handles
            // those codes first)
            Some(e) => Ok(e.clone().unwrap_or_default()),
            None => Err(exc::IndexError("list index out of range")),
        }
    }

    /// `self.table.append(entry)`
    fn append(&mut self, entry: Vec<u8>) {
        if self.table_len < READABLE {
            self.table.push(Some(entry));
        }
        self.table_len += 1;
    }

    // pdfminer/lzw.py:50 LZWDecoder.feed
    /// The bytes of `code`; `Ok(None)` for `CorruptDataError`.
    fn feed(&mut self, code: usize) -> PyResult<Option<Vec<u8>>> {
        let mut x = Vec::new();
        if code == 256 {
            self.table = (0..=255u8).map(|c| Some(vec![c])).collect(); // 0-255
            self.table.push(None); // 256
            self.table.push(None); // 257
            self.table_len = 258;
            self.prevbuf = Some(Vec::new());
            self.nbits = 9;
        } else if code == 257 {
        } else if self.prevbuf.as_ref().is_none_or(|p| p.is_empty()) {
            x = self.entry(code)?; // assume not None
            leaf(1)?; // cast(bytes, ...)
            self.prevbuf = Some(x.clone());
        } else {
            let prevbuf = self.prevbuf.take().expect("checked above");
            if code < self.table_len {
                x = self.entry(code)?; // assume not None
                leaf(1)?; // cast(bytes, ...)
                let mut e = prevbuf.clone();
                e.extend(x.first());
                self.append(e);
            } else if code == self.table_len {
                let mut e = prevbuf.clone();
                e.extend(prevbuf.first());
                self.append(e);
                x = self.entry(code)?;
                leaf(1)?; // cast(bytes, ...)
            } else {
                self.prevbuf = Some(prevbuf);
                return Ok(None); // raise CorruptDataError
            }
            match self.table_len {
                511 => self.nbits = 10,
                1023 => self.nbits = 11,
                2047 => self.nbits = 12,
                _ => {}
            }
            self.prevbuf = Some(x.clone());
        }
        Ok(Some(x))
    }

    // pdfminer/lzw.py:81 LZWDecoder.run
    /// The concatenation of what the generator yields (`b"".join(...)`).
    /// The generator consumed by `b"".join(...)`: each step resumes it from
    /// C (its frame), logs the previous output (after the first), reads a
    /// code and feeds it, each a frame one level down. (An exception ends
    /// `lzwdecode` with it: the decoder's state no longer matters then.)
    pub fn run(&mut self) -> PyResult<Vec<u8>> {
        let mut out = Vec::new();
        let mut after_yield = false;
        loop {
            let _entry = eval_entry()?;
            let _frame = py_frame()?;
            if after_yield {
                // logger.debug("nbits=%d, code=%d, output=%r, table=%r", ...)
                LOG.debug()?;
            }
            // (readbits raises PDFEOFError at the end, caught as EOFError)
            leaf(1)?; // self.readbits(self.nbits)
            let Ok(code) = self.readbits(self.nbits) else { break };
            let _feed = py_frame()?;
            match self.feed(code as usize)? {
                // just ignore corrupt data and stop yielding there
                None => break,
                Some(x) => out.extend_from_slice(&x),
            }
            after_yield = true;
        }
        Ok(out)
    }
}

// pdfminer/lzw.py:103 lzwdecode
pub fn lzwdecode(data: &[u8]) -> PyResult<Vec<u8>> {
    let _frame = py_frame()?;
    // LZWDecoder(fp) (its __init__ called from C), then .run() creates the
    // generator
    c_called_leaf()?;
    leaf(1)?;
    LZWDecoder::new(data).run()
}
