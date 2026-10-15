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

use pdfplumber::obj::{Obj, PyValue};
use pycompat::{exc, ExcKind, PyErr, PyInt, PyResult};
use rustc_hash::FxHashMap;

use super::{safe_int, write_value, Writer};
use crate::strings::{decode_generalized_utf8, is_surrogate, PyStrRef};

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
        }
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

    fn column(&mut self, rows: &[PyValue], c: usize) -> PyResult<()> {
        let cell = |r: &PyValue| match r {
            PyValue::Dict(d) => d.items()[c].1.clone(),
            _ => unreachable!("table rows are dicts"),
        };
        let cells: Vec<PyValue> = rows.iter().map(cell).collect();
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
                let idx: Vec<u32> = cells
                    .iter()
                    .map(|v| match v {
                        PyValue::Str(s) => self.intern(s),
                        _ => NO_STR,
                    })
                    .collect();
                self.align(4);
                for i in idx {
                    self.u32(i);
                }
            }
            COL_NUM_SEQ => {
                for v in &cells {
                    match v.as_seq() {
                        Some(items) => {
                            self.buf.push(items.len() as u8);
                            for x in items {
                                self.f64(as_f64(x).expect("a number sequence"));
                            }
                        }
                        None => self.buf.push(NO_SEQ),
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
        PyValue::Float(x) => Some(*x),
        PyValue::Int(i) => safe_int(i).map(|i| i as f64),
        _ => None,
    }
}

/// The narrowest column type that holds every cell exactly.
pub fn column_kind(cells: &[PyValue]) -> u8 {
    let all = |f: &dyn Fn(&PyValue) -> bool| cells.iter().all(f);
    let any_none = cells.iter().any(PyValue::is_none);
    if all(&PyValue::is_none) {
        COL_NONE
    } else if all(&|v| matches!(v, PyValue::Bool(_))) {
        COL_BOOL
    } else if all(&|v| as_f64(v).is_some()) {
        COL_F64
    } else if all(&|v| as_f64(v).is_some() || v.is_none()) {
        debug_assert!(any_none);
        COL_OPT_F64
    } else if all(&|v| matches!(v, PyValue::Str(_))) {
        COL_STR
    } else if all(&|v| matches!(v, PyValue::Str(_)) || v.is_none()) {
        COL_OPT_STR
    } else if all(&|v| {
        v.is_none()
            || matches!(v, PyValue::Tuple(items) | PyValue::List(items)
                if items.len() < usize::from(NO_SEQ) && items.iter().all(|x| as_f64(x).is_some()))
    }) {
        COL_NUM_SEQ
    } else {
        COL_GENERIC
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

    fn table(&mut self, rows: &[PyValue], keys: &[Rc<str>]) -> PyResult<bool> {
        self.buf.push(TAG_TABLE);
        self.varint(rows.len());
        self.varint(keys.len());
        for k in keys {
            let i = self.intern(k);
            self.varint(i as usize);
        }
        for c in 0..keys.len() {
            self.column(rows, c)?;
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

fn lone_surrogate() -> PyErr {
    PyErr::new(
        ExcKind::UnicodeEncodeError,
        "a str argument holds a lone surrogate, which the core's str cannot hold yet",
    )
}

fn malformed(what: &str) -> PyErr {
    exc::ValueError(format!("malformed PJB1 block: {what}"))
}

struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
    strings: Vec<Rc<str>>,
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

    fn text(&mut self, n: usize) -> PyResult<Rc<str>> {
        let bytes = self.take(n)?;
        if let Ok(s) = std::str::from_utf8(bytes) {
            return Ok(Rc::from(s));
        }
        let cps = decode_generalized_utf8(bytes).map_err(|e| malformed(&e.0))?;
        if cps.iter().any(|&c| is_surrogate(c)) {
            return Err(lone_surrogate());
        }
        Err(malformed("invalid UTF-8"))
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
        let mut plain = Vec::with_capacity(n_plain);
        let units: Vec<u16> = blob.encode_utf16().collect();
        let mut start = 0usize;
        for _ in 0..n_plain {
            let end = self.u32()? as usize;
            let s = units.get(start..end).ok_or_else(|| malformed("string offsets"))?;
            plain.push(Rc::from(String::from_utf16(s).map_err(|_| malformed("string cut in a pair"))?));
            start = end;
        }
        if n_special > 0 {
            return Err(lone_surrogate());
        }
        self.strings = plain;
        self.pos = body_end;
        Ok(())
    }
}

enum Frame {
    List(Vec<PyValue>, usize),
    Tuple(Vec<PyValue>, usize),
    Dict(Obj, usize, Option<Rc<str>>),
}

/// Reads the value of a `PJB1` block sent by JavaScript. Only plain Python
/// values are accepted (`None`, `bool`, `int`, `float`, `str`, `bytes`,
/// `list`, `tuple`, `dict` with `str` keys).
pub fn decode(block: &[u8]) -> PyResult<PyValue> {
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
            TAG_F64 => PyValue::Float(r.f64()?),
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
            other => return Err(exc::TypeError(format!("unsupported argument value (PJB1 tag {other:#04x})"))),
        };
        // attach the value to its container, closing the full ones
        loop {
            match stack.last_mut() {
                None => {
                    if r.pos != body_end {
                        return Err(malformed("trailing bytes"));
                    }
                    return Ok(value);
                }
                Some(Frame::List(items, n) | Frame::Tuple(items, n)) => {
                    items.push(value);
                    if items.len() < *n {
                        break;
                    }
                }
                Some(Frame::Dict(obj, n, key)) => match key.take() {
                    None => {
                        let PyValue::Str(k) = value else {
                            return Err(exc::TypeError("dict keys must be str"));
                        };
                        *key = Some(k);
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
            };
        }
    }
}
