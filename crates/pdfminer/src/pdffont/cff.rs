//! `NIBBLES`, `getdict` and `CFFFont` of `pdfminer/pdffont.py`: the parser
//! of Compact Font Format programs.
//!
//! pdfminer itself never builds a `CFFFont` (no font class reads
//! `/FontFile3`); the class is ported for completeness with all its
//! quirks: every `INDEX` reads the font's one file object, so reading a
//! string (`getstr`) in the middle of a charset of format 1 moves the
//! position the next range is read from, as upstream.
//!
//! `struct.unpack` and `BytesIO` semantics: a short read raises
//! `struct.error("unpack requires a buffer of N bytes")`, a negative size
//! reads to the end, a seek to a float raises `TypeError`, a negative one
//! `ValueError` and one beyond `Py_ssize_t` `OverflowError`
//! ([`super::unpack`]).

use indexmap::IndexMap;
use pycompat::io::{BytesIO, Pos};
use pycompat::repr::bytes_repr;
use pycompat::{exc, Num, PyInt, PyResult};
use rustc_hash::FxBuildHasher;

use super::frames;
use super::tables::{NIBBLES, STANDARD_STRINGS};
use super::unpack::{read_n, read_size, seek_int, unpack_be, unpack_error};
use crate::utils::nunpack;

/// A value of a CFF DICT: an `int` or a `float` (a real number).
pub type DictValue = Num;

/// `getdict`'s result: operator -> operands, in the order of first
/// assignment.
pub type CFFDict = IndexMap<u8, Vec<DictValue>, FxBuildHasher>;

/// `ord(fp.read(1))`: `TypeError` at the end of the data.
fn ord1(fp: &mut BytesIO) -> PyResult<u8> {
    match fp.read_bytes(Some(1)) {
        [b] => Ok(*b),
        _ => Err(exc::TypeError("ord() expected a character, but string of length 0 found")),
    }
}

// pdfminer/pdffont.py:174 getdict
/// Parses a CFF DICT: operands are pushed on a stack that each operator
/// (a byte up to 21) takes as its value.
pub fn getdict(data: &[u8]) -> PyResult<CFFDict> {
    let _frame = frames::frame()?;
    let mut d = CFFDict::default();
    let mut fp = BytesIO::new(data.into());
    let mut stack: Vec<DictValue> = Vec::new();
    loop {
        let c = fp.read_bytes(Some(1));
        let Some(&b0) = c.first() else { break };
        if b0 <= 21 {
            d.insert(b0, std::mem::take(&mut stack));
            continue;
        }
        let value = if b0 == 30 {
            let mut s = String::new();
            let mut more = true;
            while more {
                let b = ord1(&mut fp)?;
                for n in [b >> 4, b & 15] {
                    if n == 15 {
                        more = false;
                    } else {
                        let Some(nibble) = NIBBLES[usize::from(n)] else {
                            return Err(exc::AssertionError(""));
                        };
                        s.push_str(nibble);
                    }
                }
            }
            Num::Float(pycompat::float::float_from_str(&s)?)
        } else if (32..=246).contains(&b0) {
            Num::from(i64::from(b0) - 139)
        } else {
            let b1 = i64::from(ord1(&mut fp)?);
            let b0 = i64::from(b0);
            if (247..=250).contains(&b0) {
                Num::from(((b0 - 247) << 8) + b1 + 108)
            } else if (251..=254).contains(&b0) {
                Num::from(-((b0 - 251) << 8) - b1 - 108)
            } else {
                let b2 = i64::from(ord1(&mut fp)?);
                let b1 = if b1 >= 128 { b1 - 256 } else { b1 };
                if b0 == 28 {
                    Num::from((b1 << 8) | b2)
                } else {
                    let low = unpack_be(read_n(&mut fp, 2)?, 2);
                    Num::from((b1 << 24) | (b2 << 16) | low)
                }
            }
        };
        stack.push(value);
    }
    Ok(d)
}

/// pdfminer/pdffont.py:615 `CFFFont.INDEX`: the offsets of the items of an
/// INDEX of the font's file object.
#[derive(Clone, Debug)]
pub struct INDEX {
    pub offsets: Vec<PyInt>,
    pub base: Pos,
}

impl INDEX {
    // pdfminer/pdffont.py:616 CFFFont.INDEX.__init__
    /// Reads the INDEX at the position of `fp` and moves past its data.
    pub fn new(fp: &mut BytesIO) -> PyResult<INDEX> {
        let (_entry, _frame) = frames::c_called()?;
        let header = read_n(fp, 3)?;
        let (count, offsize) = (unpack_be(&header[..2], 2), header[2]);
        let mut offsets = Vec::new();
        for _ in 0..=count {
            let range = fp.read(Some(usize::from(offsize)));
            offsets.push(nunpack(&fp.data()[range], 0));
        }
        let base = fp.tell() - 1;
        let index = INDEX { offsets, base };
        seek_int(fp, &PyInt::Small(base).add(index.offsets.last().expect("count + 1 offsets")))?;
        Ok(index)
    }

    // pdfminer/pdffont.py:625 CFFFont.INDEX.__repr__
    pub fn repr(&self) -> String {
        format!("<INDEX: size={}>", self.len())
    }

    // pdfminer/pdffont.py:628 CFFFont.INDEX.__len__
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        self.offsets.len() - 1
    }

    /// `self.offsets[i]`
    fn offset(&self, i: usize) -> PyResult<&PyInt> {
        self.offsets.get(i).ok_or_else(|| exc::IndexError("list index out of range"))
    }

    // pdfminer/pdffont.py:631 CFFFont.INDEX.__getitem__
    /// `self[i]`, read from `fp` (the font's file object, which it moves).
    pub fn get(&self, fp: &mut BytesIO, i: usize) -> PyResult<Vec<u8>> {
        let _frame = frames::frame()?;
        seek_int(fp, &PyInt::Small(self.base).add(self.offset(i)?))?;
        let size = self.offset(i + 1)?.sub(self.offset(i)?);
        let n = read_size(&size)?;
        Ok(fp.read_bytes(n).to_vec())
    }

    // pdfminer/pdffont.py:635 CFFFont.INDEX.__iter__
    /// The items, in order.
    pub fn items(&self, fp: &mut BytesIO) -> PyResult<Vec<Vec<u8>>> {
        (0..self.len()).map(|i| self.get(fp, i)).collect()
    }
}

/// A name of a charset: a standard string (`str`) or an item of the string
/// INDEX (`bytes`), as `getstr` returns them.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SidName {
    Str(&'static str),
    Bytes(Vec<u8>),
}

/// pdfminer/pdffont.py:220 `CFFFont`.
pub struct CFFFont {
    pub name: String,
    /// `self.fp`, shared by the INDEXes.
    pub fp: BytesIO,
    pub name_index: INDEX,
    pub dict_index: INDEX,
    pub string_index: INDEX,
    pub subr_index: INDEX,
    pub top_dict: CFFDict,
    pub charstring: INDEX,
    pub nglyphs: usize,
    pub code2gid: IndexMap<i64, i64, FxBuildHasher>,
    pub gid2code: IndexMap<i64, i64, FxBuildHasher>,
    pub name2gid: IndexMap<SidName, i64, FxBuildHasher>,
    pub gid2name: IndexMap<i64, SidName, FxBuildHasher>,
}

/// `(x,) = top_dict.get(op, [0])`.
fn single(d: &CFFDict, op: u8) -> PyResult<Num> {
    match d.get(&op).map(Vec::as_slice) {
        None => Ok(Num::from(0)),
        Some([x]) => Ok(x.clone()),
        Some([]) => Err(exc::ValueError("not enough values to unpack (expected 1, got 0)")),
        Some(_) => Err(exc::ValueError("too many values to unpack (expected 1)")),
    }
}

/// `fp.seek(x)` of a DICT value: a float is not an index.
fn seek_num(fp: &mut BytesIO, x: &Num) -> PyResult<()> {
    match x {
        Num::Int(i) => seek_int(fp, i),
        Num::Float(_) => Err(exc::TypeError("'float' object cannot be interpreted as an integer")),
    }
}

impl CFFFont {
    // pdfminer/pdffont.py:638 CFFFont.__init__
    pub fn new(name: &str, fp: BytesIO) -> PyResult<CFFFont> {
        let (_entry, _frame) = frames::c_called()?;
        let mut fp = fp;
        // Header
        let header = read_n(&mut fp, 4)?;
        let hdrsize = i64::from(header[2]);
        fp.read(usize::try_from(hdrsize - 4).ok());
        // Name INDEX
        let name_index = INDEX::new(&mut fp)?;
        // Top DICT INDEX
        let dict_index = INDEX::new(&mut fp)?;
        // String INDEX
        let string_index = INDEX::new(&mut fp)?;
        // Global Subr INDEX
        let subr_index = INDEX::new(&mut fp)?;
        // Top DICT DATA
        let top_dict = getdict(&dict_index.get(&mut fp, 0)?)?;
        let charset_pos = single(&top_dict, 15)?;
        let encoding_pos = single(&top_dict, 16)?;
        let charstring_pos = single(&top_dict, 17)?;
        // CharStrings
        seek_num(&mut fp, &charstring_pos)?;
        let charstring = INDEX::new(&mut fp)?;
        let nglyphs = charstring.len();
        let mut font = CFFFont {
            name: name.to_owned(),
            fp,
            name_index,
            dict_index,
            string_index,
            subr_index,
            top_dict,
            charstring,
            nglyphs,
            code2gid: IndexMap::default(),
            gid2code: IndexMap::default(),
            name2gid: IndexMap::default(),
            gid2name: IndexMap::default(),
        };
        // Encodings
        seek_num(&mut font.fp, &encoding_pos)?;
        let format = font.fp.read_bytes(Some(1)).to_vec();
        if format == b"\x00" {
            // Format 0
            let n = read_n(&mut font.fp, 1)?[0];
            let gids = read_n(&mut font.fp, usize::from(n))?.to_vec();
            for (code, gid) in gids.into_iter().enumerate() {
                font.code2gid.insert(code as i64, i64::from(gid));
                font.gid2code.insert(i64::from(gid), code as i64);
            }
        } else if format == b"\x01" {
            // Format 1
            let n = read_n(&mut font.fp, 1)?[0];
            let mut code = 0;
            for _ in 0..n {
                let pair = read_n(&mut font.fp, 2)?;
                let (first, nleft) = (i64::from(pair[0]), i64::from(pair[1]));
                for gid in first..first + nleft + 1 {
                    font.code2gid.insert(code, gid);
                    font.gid2code.insert(gid, code);
                    code += 1;
                }
            }
        } else {
            return Err(exc::PDFValueError(format!("unsupported encoding format: {}", bytes_repr(&format))));
        }
        // Charsets
        seek_num(&mut font.fp, &charset_pos)?;
        let format = font.fp.read_bytes(Some(1)).to_vec();
        if format == b"\x00" {
            // Format 0
            let n = font.nglyphs as i64 - 1;
            // struct.unpack(">" + "H" * n, fp.read(2 * n)): n == -1 reads to
            // the end and expects nothing
            let data = match usize::try_from(2 * n) {
                Ok(size) => read_n(&mut font.fp, size)?.to_vec(),
                Err(_) => {
                    if !font.fp.read_bytes(None).is_empty() {
                        return Err(unpack_error(0));
                    }
                    Vec::new()
                }
            };
            for (gid, sid) in data.chunks_exact(2).enumerate() {
                let gid = gid as i64 + 1;
                let sidname = font.getstr(unpack_be(sid, 2))?;
                font.name2gid.insert(sidname.clone(), gid);
                font.gid2name.insert(gid, sidname);
            }
        } else if format == b"\x01" {
            // Format 1
            let n = read_n(&mut font.fp, 1)?[0];
            let mut sid = 0;
            for _ in 0..n {
                let pair = read_n(&mut font.fp, 2)?;
                let (first, nleft) = (i64::from(pair[0]), i64::from(pair[1]));
                for gid in first..first + nleft + 1 {
                    let sidname = font.getstr(sid)?;
                    font.name2gid.insert(sidname.clone(), gid);
                    font.gid2name.insert(gid, sidname);
                    sid += 1;
                }
            }
        } else if format == b"\x02" {
            // Format 2
            return Err(exc::AssertionError(format!("('Unhandled', {})", bytes_repr(&format))));
        } else {
            return Err(exc::PDFValueError(format!("unsupported charset format: {}", bytes_repr(&format))));
        }
        Ok(font)
    }

    // pdfminer/pdffont.py:718 CFFFont.getstr
    /// The standard string `sid`, else the item `sid - 391` of the string
    /// INDEX (which moves the file position).
    pub fn getstr(&mut self, sid: i64) -> PyResult<SidName> {
        let _frame = frames::frame()?;
        let nstd = STANDARD_STRINGS.len() as i64;
        if sid < nstd {
            return Ok(SidName::Str(STANDARD_STRINGS[usize::try_from(sid).expect("a SID is unsigned")]));
        }
        let i = usize::try_from(sid - nstd).expect("non-negative");
        Ok(SidName::Bytes(self.string_index.get(&mut self.fp, i)?))
    }
}
