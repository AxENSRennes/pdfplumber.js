//! The binary value format (`PJB1`) of the JavaScript boundary, specified
//! in `docs/js-runtime.md` ("Format binaire"); `js/src/core/wire.ts`
//! decodes and encodes it.
//!
//! A block is a 16-byte header (`"PJB1"`, flags, the offset of the string
//! table, reserved), one tagged value, then the string table. All integers
//! are little-endian; counts and lengths are unsigned LEB128 varints.
//!
//! - Every `str` written by [`BinWriter`] goes to the string table once
//!   (keys and repeated values such as font names are decoded once by
//!   JavaScript). Strings without surrogates are one UTF-8 blob that
//!   `TextDecoder` decodes in one call, cut by their UTF-16 end offsets;
//!   strings with a lone surrogate are stored as UTF-16 code units.
//! - A list of at least two dicts with the same keys (page objects) is a
//!   *table*: the keys once, then one column per key, typed when all its
//!   cells allow it (numbers as `f64`, strings as indices...), so that
//!   JavaScript builds the objects column by column.
//! - Integers within ±2^53 are JavaScript numbers (`I32` when they fit 32
//!   bits, else `IntF64`); larger ones are written in decimal and become a
//!   `BigInt`. Floats are their 8 IEEE-754 bytes: NaN, infinities and
//!   `-0.0` cross unchanged. A float and an integer of the same value are
//!   the same JavaScript number.

use std::rc::Rc;

use pdfminer::pdftypes::PDFObject;
use pdfminer::psparser::{KWD, LIT, LIT_bytes};
use pdfplumber::obj::{Obj, PyValue};
use pycompat::{exc, PyErr, PyInt, PyResult, PyStr};
use rustc_hash::FxHashMap;

use super::{safe_int, write_value, Shapes, Writer};
use crate::strings::{decode_generalized_utf8, PyStrRef};

pub const MAGIC: &[u8; 4] = b"PJB1";
pub const HEADER_LEN: usize = 16;

pub const TAG_NONE: u8 = 0x00;
pub const TAG_FALSE: u8 = 0x01;
pub const TAG_TRUE: u8 = 0x02;
pub const TAG_I32: u8 = 0x03;
pub const TAG_INT_F64: u8 = 0x04;
pub const TAG_BIGINT: u8 = 0x05;
pub const TAG_F64: u8 = 0x06;
pub const TAG_STR: u8 = 0x07;
pub const TAG_BYTES: u8 = 0x08;
pub const TAG_LIST: u8 = 0x09;
pub const TAG_TUPLE: u8 = 0x0a;
pub const TAG_DICT: u8 = 0x0b;
pub const TAG_LITERAL: u8 = 0x0c;
pub const TAG_KEYWORD: u8 = 0x0d;
pub const TAG_REF: u8 = 0x0e;
pub const TAG_STREAM: u8 = 0x0f;
pub const TAG_TABLE: u8 = 0x10;
pub const TAG_INLINE_STR: u8 = 0x11;
pub const TAG_PAGE: u8 = 0x12;
/// A `PDFStream` by reference: its handle, `objid`, `genno`, `attrs`
/// ([`Writer::stream_ref`]).
pub const TAG_STREAM_REF: u8 = 0x13;

pub const COL_GENERIC: u8 = 0;
pub const COL_F64: u8 = 1;
pub const COL_STR: u8 = 2;
pub const COL_OPT_STR: u8 = 3;
pub const COL_BOOL: u8 = 4;
pub const COL_NONE: u8 = 5;
pub const COL_NUM_SEQ: u8 = 6;
pub const COL_OPT_F64: u8 = 7;

/// The `u32` index of a `None` cell in a [`COL_OPT_STR`] column.
pub const NO_STR: u32 = u32::MAX;
/// The length byte of a `None` cell in a [`COL_NUM_SEQ`] column.
pub const NO_SEQ: u8 = 0xff;

/// Writes a `PJB1` block (see the module documentation).
pub struct BinWriter {
    buf: Vec<u8>,
    index: FxHashMap<Box<str>, u32>,
    count: u32,
    /// UTF-8 of the strings without surrogates, in index order.
    blob: Vec<u8>,
    /// UTF-16 end offset of each string without surrogates, in the
    /// concatenation of those strings.
    ends: Vec<u32>,
    utf16_len: u32,
    /// `(index, UTF-16 code units)` of the strings with a surrogate.
    special: Vec<(u32, Vec<u16>)>,
    special_index: FxHashMap<Vec<u16>, u32>,
    /// Whether uniform lists of dicts are written as tables.
    tables: bool,
}

impl Default for BinWriter {
    fn default() -> Self {
        BinWriter::new()
    }
}

impl BinWriter {
    pub fn new() -> BinWriter {
        let mut buf = Vec::with_capacity(4096);
        buf.extend_from_slice(MAGIC);
        buf.extend_from_slice(&[0; HEADER_LEN - 4]);
        BinWriter {
            buf,
            index: FxHashMap::default(),
            count: 0,
            blob: Vec::new(),
            ends: Vec::new(),
            utf16_len: 0,
            special: Vec::new(),
            special_index: FxHashMap::default(),
            tables: true,
        }
    }

    /// A writer that writes lists as lists, never as tables (the argument
    /// form JavaScript writes and [`decode`] reads).
    pub fn without_tables() -> BinWriter {
        BinWriter { tables: false, ..BinWriter::new() }
    }

    /// The block: header, value, string table.
    pub fn finish(mut self) -> Vec<u8> {
        self.align(4);
        let table_offset = self.buf.len() as u32;
        self.buf[8..12].copy_from_slice(&table_offset.to_le_bytes());
        let n_special = self.special.len() as u32;
        self.u32(self.count);
        self.u32(n_special);
        self.u32(self.blob.len() as u32);
        let blob = std::mem::take(&mut self.blob);
        self.buf.extend_from_slice(&blob);
        self.align(4);
        for e in std::mem::take(&mut self.ends) {
            self.u32(e);
        }
        for (i, units) in std::mem::take(&mut self.special) {
            self.u32(i);
            self.u32(units.len() as u32);
            for u in units {
                self.buf.extend_from_slice(&u.to_le_bytes());
            }
            self.align(4);
        }
        self.buf
    }

    fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn varint(&mut self, mut v: usize) {
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                self.buf.push(b);
                return;
            }
            // (`|` and `^` are the same here: `b` has no high bit)
            self.buf.push(b | 0x80);
        }
    }

    /// Pads with zeros to a multiple of `n` from the start of the block.
    fn align(&mut self, n: usize) {
        while !self.buf.len().is_multiple_of(n) {
            self.buf.push(0);
        }
    }

    /// The index of a string without surrogates.
    fn intern(&mut self, s: &str) -> u32 {
        if let Some(&i) = self.index.get(s) {
            return i;
        }
        let i = self.count;
        self.count += 1;
        self.blob.extend_from_slice(s.as_bytes());
        self.utf16_len += s.encode_utf16().count() as u32;
        self.ends.push(self.utf16_len);
        self.index.insert(Box::from(s), i);
        i
    }

    /// The index of a Python `str` (a lone surrogate included).
    fn intern_str(&mut self, s: &PyStr) -> u32 {
        match s.as_str() {
            Some(text) => self.intern(text),
            None => self.intern_units(s.to_utf16()),
        }
    }

    /// The index of the string of these UTF-16 units.
    fn intern_units(&mut self, units: Vec<u16>) -> u32 {
        // valid UTF-16 (pairs included) is text; only lone surrogates go on
        if let Ok(s) = String::from_utf16(&units) {
            return self.intern(&s);
        }
        if let Some(&i) = self.special_index.get(&units) {
            return i;
        }
        let i = self.count;
        self.count += 1;
        self.special_index.insert(units.clone(), i);
        self.special.push((i, units));
        i
    }

    fn f64(&mut self, x: f64) {
        self.buf.extend_from_slice(&x.to_le_bytes());
    }

    fn column(&mut self, rows: &[&Obj], c: usize) -> PyResult<()> {
        let cells: Vec<&PyValue> = rows.iter().map(|d| &d.items()[c].1).collect();
        let kind = column_kind(&cells);
        self.buf.push(kind);
        match kind {
            COL_NONE => {}
            COL_BOOL => {
                for v in &cells {
                    self.buf.push(u8::from(matches!(v, PyValue::Bool(true))));
                }
            }
            COL_F64 => {
                self.align(8);
                for v in &cells {
                    self.f64(as_f64(v).expect("a number column"));
                }
            }
            COL_OPT_F64 => {
                for v in &cells {
                    self.buf.push(u8::from(!v.is_none()));
                }
                self.align(8);
                for v in &cells {
                    self.f64(as_f64(v).unwrap_or(0.0));
                }
            }
            COL_STR | COL_OPT_STR => {
                self.align(4);
                for v in &cells {
                    let i = match v {
                        PyValue::Str(s) => self.intern_str(s),
                        _ => NO_STR,
                    };
                    self.u32(i);
                }
            }
            COL_NUM_SEQ => {
                // the lengths, then every number, aligned (a Float64Array view)
                for v in &cells {
                    self.buf.push(v.as_seq().map_or(NO_SEQ, |items| items.len() as u8));
                }
                self.align(8);
                for v in &cells {
                    for x in v.as_seq().unwrap_or(&[]) {
                        self.f64(as_f64(x).expect("a number sequence"));
                    }
                }
            }
            _ => {
                for v in &cells {
                    write_value(self, v)?;
                }
            }
        }
        Ok(())
    }
}

/// The value of a number cell as a JavaScript number: a float, or an
/// integer within ±2^53.
fn as_f64(v: &PyValue) -> Option<f64> {
    match v {
        PyValue::Float(x) => Some(x.get()),
        PyValue::Int(i) => safe_int(i).map(|i| i as f64),
        _ => None,
    }
}

/// Whether a cell is a sequence that a [`COL_NUM_SEQ`] column holds.
fn is_num_seq(v: &PyValue) -> bool {
    matches!(v, PyValue::Tuple(items) | PyValue::List(items)
        if items.len() < usize::from(NO_SEQ) && items.iter().all(|x| as_f64(x).is_some()))
}

/// The narrowest column type that holds every cell exactly, in one pass:
/// the kinds of cell seen, then the column type that allows them all.
pub fn column_kind(cells: &[&PyValue]) -> u8 {
    const NONE: u8 = 1;
    const BOOL: u8 = 2;
    const NUM: u8 = 4;
    const STR: u8 = 8;
    const SEQ: u8 = 16;
    let mut seen = 0u8;
    for v in cells {
        seen |= match v {
            PyValue::None => NONE,
            PyValue::Bool(_) => BOOL,
            v if as_f64(v).is_some() => NUM,
            PyValue::Str(_) => STR,
            v if is_num_seq(v) => SEQ,
            _ => return COL_GENERIC,
        };
    }
    // (`|` and `^` are the same in these patterns: the flags are distinct bits)
    match seen {
        NONE => COL_NONE,
        BOOL => COL_BOOL,
        NUM => COL_F64,
        x if x == NUM | NONE => COL_OPT_F64,
        STR => COL_STR,
        x if x == STR | NONE => COL_OPT_STR,
        x if x & !NONE == SEQ => COL_NUM_SEQ,
        _ => COL_GENERIC,
    }
}

impl Writer for BinWriter {
    fn none(&mut self) {
        self.buf.push(TAG_NONE);
    }

    fn bool(&mut self, b: bool) {
        self.buf.push(if b { TAG_TRUE } else { TAG_FALSE });
    }

    fn int(&mut self, i: &PyInt) {
        match safe_int(i) {
            Some(v) => match i32::try_from(v) {
                Ok(v32) => {
                    self.buf.push(TAG_I32);
                    self.buf.extend_from_slice(&v32.to_le_bytes());
                }
                Err(_) => {
                    self.buf.push(TAG_INT_F64);
                    self.f64(v as f64);
                }
            },
            None => {
                let text = i.to_string();
                self.buf.push(TAG_BIGINT);
                self.varint(text.len());
                self.buf.extend_from_slice(text.as_bytes());
            }
        }
    }

    fn float(&mut self, x: f64) {
        self.buf.push(TAG_F64);
        self.f64(x);
    }

    fn str(&mut self, s: &str) {
        let i = self.intern(s);
        self.buf.push(TAG_STR);
        self.varint(i as usize);
    }

    fn py_str(&mut self, s: PyStrRef<'_>) -> PyResult<()> {
        let units = s.utf16().map_err(|e| exc::ValueError(e.0))?;
        let i = self.intern_units(units);
        self.buf.push(TAG_STR);
        self.varint(i as usize);
        Ok(())
    }

    fn bytes(&mut self, b: &[u8]) {
        self.buf.push(TAG_BYTES);
        self.varint(b.len());
        self.buf.extend_from_slice(b);
    }

    fn list(&mut self, n: usize) {
        self.buf.push(TAG_LIST);
        self.varint(n);
    }

    fn tuple(&mut self, n: usize) {
        self.buf.push(TAG_TUPLE);
        self.varint(n);
    }

    fn dict(&mut self, n: usize) {
        self.buf.push(TAG_DICT);
        self.varint(n);
    }

    fn literal(&mut self) {
        self.buf.push(TAG_LITERAL);
    }

    fn keyword(&mut self, name: &[u8]) {
        self.buf.push(TAG_KEYWORD);
        self.varint(name.len());
        self.buf.extend_from_slice(name);
    }

    fn objref(&mut self) {
        self.buf.push(TAG_REF);
    }

    fn stream(&mut self) {
        self.buf.push(TAG_STREAM);
    }

    fn stream_ref(&mut self) {
        self.buf.push(TAG_STREAM_REF);
    }

    fn page(&mut self) {
        self.buf.push(TAG_PAGE);
    }

    fn table(&mut self, rows: &[PyValue], shapes: &Shapes<'_>) -> PyResult<bool> {
        if !self.tables {
            return Ok(false);
        }
        self.buf.push(TAG_TABLE);
        self.varint(rows.len());
        self.varint(shapes.first.len());
        for (s, first) in shapes.first.iter().enumerate() {
            let group = shapes.rows(rows, s);
            // a capacity hint (about one 8-byte cell and a tag per cell):
            // any other value writes the same bytes
            self.buf.reserve(group.len() * first.len() * 9);
            self.varint(group.len());
            self.varint(first.len());
            for k in first.keys() {
                let i = self.intern_str(k);
                self.varint(i as usize);
            }
            for c in 0..first.len() {
                self.column(&group, c)?;
            }
        }
        if shapes.first.len() > 1 {
            self.buf.extend_from_slice(&shapes.order);
        }
        Ok(true)
    }
}

/// Encodes a value as a `PJB1` block.
pub fn encode(v: &PyValue) -> PyResult<Vec<u8>> {
    let mut w = BinWriter::new();
    write_value(&mut w, v)?;
    Ok(w.finish())
}

// ---------------------------------------------------------------------------
// Reading (arguments sent by JavaScript)

fn malformed(what: &str) -> PyErr {
    exc::ValueError(format!("malformed PJB1 block: {what}"))
}

struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
    strings: Vec<PyStr>,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> PyResult<&[u8]> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.b.len()).ok_or_else(|| malformed("truncated"))?;
        let s = &self.b[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    fn u8(&mut self) -> PyResult<u8> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> PyResult<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().expect("4 bytes")))
    }

    fn f64(&mut self) -> PyResult<f64> {
        Ok(f64::from_le_bytes(self.take(8)?.try_into().expect("8 bytes")))
    }

    fn varint(&mut self) -> PyResult<usize> {
        let mut v: u64 = 0;
        for shift in (0..64).step_by(7) {
            let b = self.u8()?;
            v |= u64::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                return usize::try_from(v).map_err(|_| malformed("varint out of range"));
            }
        }
        Err(malformed("varint too long"))
    }

    /// A `str` in generalized UTF-8 (Python's `surrogatepass`, what
    /// JavaScript writes): a lone surrogate is kept.
    fn text(&mut self, n: usize) -> PyResult<PyStr> {
        let bytes = self.take(n)?;
        if let Ok(s) = std::str::from_utf8(bytes) {
            return Ok(PyStr::new(s));
        }
        let cps = decode_generalized_utf8(bytes).map_err(|e| malformed(&e.0))?;
        Ok(PyStr::from_code_points(cps).expect("decoded code points are at most U+10FFFF"))
    }

    /// The string table at `offset`, as [`BinWriter::finish`] writes it.
    fn read_table(&mut self, offset: usize) -> PyResult<()> {
        let body_end = self.pos;
        self.pos = offset;
        let count = self.u32()? as usize;
        let n_special = self.u32()? as usize;
        let blob_len = self.u32()? as usize;
        let blob = self.take(blob_len)?.to_vec();
        let blob = String::from_utf8(blob).map_err(|_| malformed("string table is not UTF-8"))?;
        self.pos = self.pos.next_multiple_of(4);
        let n_plain = count.checked_sub(n_special).ok_or_else(|| malformed("string counts"))?;
        // every string has a 4-byte offset: the counts cannot exceed the block
        // and a string with a surrogate at least 8 bytes (index, length)
        if n_plain > (self.b.len() - self.pos) / 4 || n_special > (self.b.len() - self.pos) / 8 {
            return Err(malformed("string counts"));
        }
        let mut plain = Vec::with_capacity(n_plain);
        let units: Vec<u16> = blob.encode_utf16().collect();
        let mut start = 0usize;
        for _ in 0..n_plain {
            let end = self.u32()? as usize;
            let s = units.get(start..end).ok_or_else(|| malformed("string offsets"))?;
            plain.push(PyStr::new(&String::from_utf16(s).map_err(|_| malformed("string cut in a pair"))?));
            start = end;
        }
        // the strings with a lone surrogate, as BinWriter::finish writes
        // them: (index, UTF-16 units), in index order; the plain ones fill
        // the other indices in order
        let mut strings: Vec<Option<PyStr>> = vec![None; count];
        for _ in 0..n_special {
            let i = self.u32()? as usize;
            let n = self.u32()? as usize;
            let bytes = self.take(n.checked_mul(2).ok_or_else(|| malformed("string length"))?)?;
            let units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|&u| u16::from_le_bytes(u)).collect();
            let slot = strings.get_mut(i).ok_or_else(|| malformed("string index"))?;
            if slot.is_some() {
                return Err(malformed("string index"));
            }
            *slot = Some(PyStr::from_utf16(&units));
            self.pos = self.pos.next_multiple_of(4);
        }
        let mut plain = plain.into_iter();
        for slot in strings.iter_mut().filter(|s| s.is_none()) {
            *slot = Some(plain.next().ok_or_else(|| malformed("string counts"))?);
        }
        self.strings = strings.into_iter().map(|s| s.expect("every index filled")).collect();
        self.pos = body_end;
        Ok(())
    }
}

enum Frame {
    List(Vec<PyValue>, usize),
    Tuple(Vec<PyValue>, usize),
    Dict(Obj, usize, Option<PyStr>),
    /// The values of an object argument ([`ObjectArg`]) being read.
    Object(u8, Vec<PyValue>, usize),
}

/// A pdfminer or pdfplumber object JavaScript gives back as an argument
/// (an object of a result: `PDFObjRef`, a `PDFStream` by reference, a
/// `PageRef`), which only the core can make the object it stands for
/// ([`decode_args`]).
pub enum ObjectArg {
    /// `PDFObjRef`: its `objid`.
    Ref(PyValue),
    /// A pdfplumber `Page`: its handle and `page_number`.
    Page(PyValue, PyValue),
    /// A `PDFStream` by reference: its handle (then `objid`, `genno`,
    /// `attrs`, which the handle decides).
    Stream(PyValue),
}

/// Reads the value of a `PJB1` block sent by JavaScript. Only plain Python
/// values are accepted (`None`, `bool`, `int`, `float`, `str`, `bytes`,
/// `list`, `tuple`, `dict` with `str` keys), and `PSLiteral`s and
/// `PSKeyword`s (interned, as pdfminer's `LIT` and `KWD` make them).
pub fn decode(block: &[u8]) -> PyResult<PyValue> {
    decode_args(block, &mut |_| Err(exc::TypeError("unsupported argument value (an object of a document)")))
}

/// [`decode`], the objects of a document ([`ObjectArg`]) made by `object`.
pub fn decode_args(block: &[u8], object: &mut dyn FnMut(ObjectArg) -> PyResult<PyValue>) -> PyResult<PyValue> {
    if block.len() < HEADER_LEN || &block[..4] != MAGIC {
        return Err(malformed("bad header"));
    }
    let table_offset = u32::from_le_bytes(block[8..12].try_into().expect("4 bytes")) as usize;
    let mut r = Reader { b: block, pos: HEADER_LEN, strings: Vec::new() };
    if table_offset != 0 {
        r.read_table(table_offset)?;
    }
    let body_end = if table_offset != 0 { table_offset } else { block.len() };
    let mut stack: Vec<Frame> = Vec::new();
    loop {
        let tag = r.u8()?;
        let mut value = match tag {
            TAG_NONE => PyValue::None,
            TAG_FALSE => PyValue::Bool(false),
            TAG_TRUE => PyValue::Bool(true),
            TAG_I32 => PyValue::Int(PyInt::from(i32::from_le_bytes(r.take(4)?.try_into().expect("4 bytes")))),
            TAG_INT_F64 => {
                let x = r.f64()?;
                if x.fract() != 0.0 || x.abs() > (1u64 << 53) as f64 {
                    return Err(malformed("IntF64 is not a safe integer"));
                }
                PyValue::Int(PyInt::from(x as i64))
            }
            TAG_BIGINT => {
                let n = r.varint()?;
                let text = std::str::from_utf8(r.take(n)?).map_err(|_| malformed("BigInt"))?;
                PyValue::Int(PyInt::from_decimal(text).ok_or_else(|| malformed("BigInt"))?)
            }
            // a new float object (a NaN from JavaScript has no identity)
            TAG_F64 => PyValue::float(r.f64()?),
            TAG_STR => {
                let i = r.varint()?;
                PyValue::Str(r.strings.get(i).cloned().ok_or_else(|| malformed("string index"))?)
            }
            TAG_INLINE_STR => {
                let n = r.varint()?;
                PyValue::Str(r.text(n)?)
            }
            TAG_BYTES => {
                let n = r.varint()?;
                PyValue::Bytes(Rc::from(r.take(n)?))
            }
            TAG_LIST | TAG_TUPLE | TAG_DICT => {
                let n = r.varint()?;
                // every item takes at least one byte
                if n > body_end.saturating_sub(r.pos) {
                    return Err(malformed("container longer than its block"));
                }
                if n == 0 {
                    match tag {
                        TAG_LIST => PyValue::List(Rc::from(Vec::new())),
                        TAG_TUPLE => PyValue::Tuple(Rc::from(Vec::new())),
                        _ => PyValue::Dict(Obj::new()),
                    }
                } else {
                    stack.push(match tag {
                        TAG_LIST => Frame::List(Vec::with_capacity(n.min(4096)), n),
                        TAG_TUPLE => Frame::Tuple(Vec::with_capacity(n.min(4096)), n),
                        _ => Frame::Dict(Obj::new(), n, None),
                    });
                    continue;
                }
            }
            TAG_KEYWORD => {
                let n = r.varint()?;
                PyValue::Pdf(PDFObject::Keyword(KWD(r.take(n)?)))
            }
            TAG_LITERAL | TAG_REF | TAG_PAGE | TAG_STREAM_REF => {
                let n = match tag {
                    TAG_LITERAL | TAG_REF => 1,
                    TAG_PAGE => 2,
                    _ => 4,
                };
                stack.push(Frame::Object(tag, Vec::with_capacity(n), n));
                continue;
            }
            other => return Err(exc::TypeError(format!("unsupported argument value (PJB1 tag {other:#04x})"))),
        };
        // attach the value to its container, closing the full ones
        loop {
            match stack.last_mut() {
                None => {
                    // only the zero padding that aligns the string table may follow
                    let rest =
                        block.get(r.pos..body_end).ok_or_else(|| malformed("value overlaps the string table"))?;
                    let padding = if table_offset == 0 { 0 } else { 3 };
                    if rest.len() > padding || rest.iter().any(|&b| b != 0) {
                        return Err(malformed("trailing bytes"));
                    }
                    return Ok(value);
                }
                Some(Frame::List(items, n) | Frame::Tuple(items, n) | Frame::Object(_, items, n)) => {
                    items.push(value);
                    if items.len() < *n {
                        break;
                    }
                }
                Some(Frame::Dict(obj, n, key)) => match key.take() {
                    None => {
                        let PyValue::Str(k) = &value else {
                            return Err(exc::TypeError("dict keys must be str"));
                        };
                        *key = Some(k.clone());
                        break;
                    }
                    Some(k) => {
                        obj.set(&k, value);
                        *n -= 1;
                        if *n > 0 {
                            break;
                        }
                    }
                },
            }
            value = match stack.pop().expect("a frame") {
                Frame::List(items, _) => PyValue::List(Rc::from(items)),
                Frame::Tuple(items, _) => PyValue::Tuple(Rc::from(items)),
                Frame::Dict(obj, _, _) => PyValue::Dict(obj),
                Frame::Object(tag, items, _) => {
                    let mut items = items.into_iter();
                    let mut next = || items.next().expect("the values of the object");
                    match tag {
                        // PSLiteralTable.intern(name): `str` or `bytes`
                        TAG_LITERAL => match &next() {
                            PyValue::Str(name) => match name.as_str() {
                                Some(name) => PyValue::Pdf(PDFObject::Literal(LIT(name))),
                                None => return Err(exc::TypeError("a PSLiteral name with a lone surrogate")),
                            },
                            PyValue::Bytes(name) => PyValue::Pdf(PDFObject::Literal(LIT_bytes(name))),
                            other => {
                                return Err(exc::TypeError(format!("a PSLiteral name is a str, not {}", other.type_name())))
                            }
                        },
                        TAG_REF => object(ObjectArg::Ref(next()))?,
                        TAG_PAGE => {
                            let handle = next();
                            object(ObjectArg::Page(handle, next()))?
                        }
                        _ => object(ObjectArg::Stream(next()))?,
                    }
                }
            };
        }
    }
}
