//! Shared helpers of the boundary tests: what JavaScript sees of a value
//! ([`Js`]), decoders of the result formats that follow `js/src/core/wire.ts`
//! (the PJB1 decoder and the JSON fixups), and requests to [`Core`].

#![allow(dead_code)]

use std::rc::Rc;

use pdfminer::pdftypes::PDFObject;
use pdfminer::psparser::LiteralName;
use pdfplumber::obj::PyValue;
use pdfplumber_wasm::ops::Core;
use pdfplumber_wasm::wire::binary::*;
use pdfplumber_wasm::wire::{write_value, MAX_SAFE};
use pdftrace::value::{NameValue, PyStr, Value};
use pycompat::PyInt;

/// A JavaScript value, as the decoders of `js/src/core/wire.ts` build it.
#[derive(Clone, Debug)]
pub enum Js {
    Null,
    Bool(bool),
    Num(f64),
    Big(String),
    Str(Vec<u16>),
    Bytes(Vec<u8>),
    Arr(Vec<Js>),
    Obj(Vec<(Vec<u16>, Js)>),
    Lit(Box<Js>),
    Kw(Vec<u8>),
    Ref(Box<Js>),
    Stream(Vec<Js>),
}

impl PartialEq for Js {
    /// `Object.is` for numbers (NaN is NaN, -0 is not 0), key order of
    /// objects included.
    fn eq(&self, other: &Js) -> bool {
        use Js::*;
        match (self, other) {
            (Num(a), Num(b)) => (a.is_nan() && b.is_nan()) || a.to_bits() == b.to_bits(),
            (Null, Null) => true,
            (Bool(a), Bool(b)) => a == b,
            (Big(a), Big(b)) => a == b,
            (Str(a), Str(b)) => a == b,
            (Bytes(a), Bytes(b)) | (Kw(a), Kw(b)) => a == b,
            (Arr(a), Arr(b)) | (Stream(a), Stream(b)) => a == b,
            (Obj(a), Obj(b)) => a == b,
            (Lit(a), Lit(b)) | (Ref(a), Ref(b)) => a == b,
            _ => false,
        }
    }
}

pub fn s(text: &str) -> Js {
    Js::Str(text.encode_utf16().collect())
}

pub fn obj(pairs: Vec<(&str, Js)>) -> Js {
    Js::Obj(pairs.into_iter().map(|(k, v)| (k.encode_utf16().collect(), v)).collect())
}

pub fn int_view(i: &PyInt) -> Js {
    match i.as_i64().filter(|v| (-MAX_SAFE..=MAX_SAFE).contains(v)) {
        Some(v) => Js::Num(v as f64),
        None => Js::Big(i.to_string()),
    }
}

/// What JavaScript receives for a pdfplumber value.
pub fn js_view(v: &PyValue) -> Js {
    match v {
        PyValue::None => Js::Null,
        PyValue::Bool(b) => Js::Bool(*b),
        PyValue::Int(i) => int_view(i),
        PyValue::Float(x) => Js::Num(x.get()),
        PyValue::Str(t) => Js::Str(t.to_utf16()),
        PyValue::Bytes(b) => Js::Bytes(b.to_vec()),
        PyValue::List(items) | PyValue::Tuple(items) => Js::Arr(items.iter().map(js_view).collect()),
        PyValue::Dict(d) => Js::Obj(d.items().iter().map(|(k, v)| (k.to_utf16(), js_view(v))).collect()),
        PyValue::Pdf(o) => pdf_view(o),
        PyValue::Page(p) => Js::Obj(vec![("page_number".encode_utf16().collect(), int_view(p.page_number()))]),
    }
}

/// What JavaScript receives for a PDF object.
pub fn pdf_view(o: &PDFObject) -> Js {
    match o {
        PDFObject::Null => Js::Null,
        PDFObject::Bool(b) => Js::Bool(*b),
        PDFObject::Int(i) => int_view(i),
        PDFObject::Float(x) => Js::Num(*x),
        PDFObject::Bytes(b) => Js::Bytes(b.to_vec()),
        PDFObject::Str(t) => Js::Str(t.to_utf16()),
        PDFObject::Literal(l) => Js::Lit(Box::new(match l.name() {
            LiteralName::Str(t) => s(t),
            LiteralName::Bytes(b) => Js::Bytes(b.to_vec()),
        })),
        PDFObject::Keyword(k) => Js::Kw(k.name().to_vec()),
        PDFObject::List(l) => Js::Arr(l.borrow().iter().map(pdf_view).collect()),
        PDFObject::Dict(d) => {
            Js::Obj(d.borrow().iter().map(|(k, v)| (k.encode_utf16().collect(), pdf_view(v))).collect())
        }
        PDFObject::Ref(r) => Js::Ref(Box::new(int_view(r.objid()))),
        PDFObject::Stream(st) => Js::Stream(vec![
            st.objid().map_or(Js::Null, |i| pdf_view(i.obj())),
            st.genno().map_or(Js::Null, |i| int_view(&i)),
            pdf_view(&PDFObject::Dict(st.attrs.clone())),
            st.rawdata().map_or(Js::Null, |b| Js::Bytes(b.to_vec())),
            st.data().map_or(Js::Null, |b| Js::Bytes(b.to_vec())),
        ]),
    }
}

/// What JavaScript receives for a value of the oracle (typed JSON).
pub fn value_view(v: &Value) -> Js {
    match v {
        Value::None => Js::Null,
        Value::Bool(b) => Js::Bool(*b),
        Value::Int(i) => int_view(i),
        Value::Float(x) => Js::Num(*x),
        Value::Str(t) => Js::Str(pystr_units(t)),
        Value::Bytes(b) => Js::Bytes(b.clone()),
        Value::List(items) | Value::Tuple(items) => Js::Arr(items.iter().map(value_view).collect()),
        Value::Dict(items) => Js::Obj(
            items
                .iter()
                .map(|(k, v)| match k {
                    Value::Str(t) => (pystr_units(t), value_view(v)),
                    other => panic!("a dict key that is not a str: {other:?}"),
                })
                .collect(),
        ),
        Value::Literal(NameValue::Str(t)) => Js::Lit(Box::new(Js::Str(pystr_units(t)))),
        Value::Literal(NameValue::Bytes(b)) => Js::Lit(Box::new(Js::Bytes(b.clone()))),
        Value::Keyword(k) => Js::Kw(k.clone()),
        Value::Ref(i) => Js::Ref(Box::new(int_view(i))),
        Value::Exception { class, message } => {
            obj(vec![("exception", s(class)), ("message", Js::Str(pystr_units(message)))])
        }
        other => panic!("no JavaScript view for {other:?}"),
    }
}

pub fn pystr_units(t: &PyStr) -> Vec<u16> {
    pdfplumber_wasm::strings::code_points_utf16(&t.code_points())
}

// ---------------------------------------------------------------------------
// PJB1 decoding, as js/src/core/wire.ts does it

pub struct Dec<'a> {
    pub b: &'a [u8],
    pub pos: usize,
    pub strings: Vec<Vec<u16>>,
}

fn utf16_of_utf8(b: &[u8]) -> Vec<u16> {
    std::str::from_utf8(b).expect("UTF-8 blob").encode_utf16().collect()
}

impl Dec<'_> {
    fn u8(&mut self) -> u8 {
        let v = self.b[self.pos];
        self.pos += 1;
        v
    }
    fn u32(&mut self) -> u32 {
        let v = u32::from_le_bytes(self.b[self.pos..self.pos + 4].try_into().unwrap());
        self.pos += 4;
        v
    }
    fn f64(&mut self) -> f64 {
        let v = f64::from_le_bytes(self.b[self.pos..self.pos + 8].try_into().unwrap());
        self.pos += 8;
        v
    }
    fn varint(&mut self) -> usize {
        let mut v = 0usize;
        let mut shift = 0;
        loop {
            let b = self.u8();
            v |= usize::from(b & 0x7f) << shift;
            if b < 0x80 {
                return v;
            }
            shift += 7;
        }
    }
    fn align(&mut self, n: usize) {
        while !self.pos.is_multiple_of(n) {
            assert_eq!(self.b[self.pos], 0, "padding is zero");
            self.pos += 1;
        }
    }
    fn take(&mut self, n: usize) -> Vec<u8> {
        let v = self.b[self.pos..self.pos + n].to_vec();
        self.pos += n;
        v
    }

    pub fn value(&mut self) -> Js {
        match self.u8() {
            TAG_NONE => Js::Null,
            TAG_FALSE => Js::Bool(false),
            TAG_TRUE => Js::Bool(true),
            TAG_I32 => Js::Num(f64::from(i32::from_le_bytes(self.take(4).try_into().unwrap()))),
            TAG_INT_F64 => {
                let x = self.f64();
                assert!(x.fract() == 0.0 && x.abs() <= MAX_SAFE as f64 && x.abs() > f64::from(i32::MAX) - 1.0);
                Js::Num(x)
            }
            TAG_BIGINT => {
                let n = self.varint();
                Js::Big(String::from_utf8(self.take(n)).unwrap())
            }
            TAG_F64 => Js::Num(self.f64()),
            TAG_STR => {
                let i = self.varint();
                Js::Str(self.strings[i].clone())
            }
            TAG_INLINE_STR => {
                let n = self.varint();
                let cps = pdfplumber_wasm::strings::decode_generalized_utf8(&self.take(n)).unwrap();
                Js::Str(pdfplumber_wasm::strings::code_points_utf16(&cps))
            }
            TAG_BYTES => {
                let n = self.varint();
                Js::Bytes(self.take(n))
            }
            TAG_LIST | TAG_TUPLE => {
                let n = self.varint();
                Js::Arr((0..n).map(|_| self.value()).collect())
            }
            TAG_DICT => {
                let n = self.varint();
                Js::Obj(
                    (0..n)
                        .map(|_| {
                            let Js::Str(k) = self.value() else { panic!("a key is a str") };
                            (k, self.value())
                        })
                        .collect(),
                )
            }
            TAG_LITERAL => Js::Lit(Box::new(self.value())),
            TAG_KEYWORD => {
                let n = self.varint();
                Js::Kw(self.take(n))
            }
            TAG_REF => Js::Ref(Box::new(self.value())),
            TAG_STREAM => Js::Stream((0..5).map(|_| self.value()).collect()),
            TAG_TABLE => self.table(),
            t => panic!("unknown tag {t}"),
        }
    }

    fn table(&mut self) -> Js {
        let n = self.varint();
        let nshapes = self.varint();
        assert!((1..=pdfplumber_wasm::wire::MAX_SHAPES).contains(&nshapes) && 2 * nshapes <= n);
        let mut groups: Vec<Vec<Js>> = Vec::new();
        for _ in 0..nshapes {
            let rows = self.varint();
            let ncols = self.varint();
            let mut keys: Vec<Vec<u16>> = Vec::new();
            for _ in 0..ncols {
                let k = self.varint();
                keys.push(self.strings[k].clone());
            }
            let mut cols: Vec<Vec<Js>> = Vec::new();
            for _ in 0..ncols {
                cols.push(self.column(rows));
            }
            groups.push(
                (0..rows)
                    .map(|i| Js::Obj(keys.iter().zip(&cols).map(|(k, c)| (k.clone(), c[i].clone())).collect()))
                    .collect(),
            );
        }
        assert_eq!(groups.iter().map(Vec::len).sum::<usize>(), n);
        if nshapes == 1 {
            return Js::Arr(groups.pop().unwrap());
        }
        let order = self.take(n);
        let mut next = vec![0; nshapes];
        Js::Arr(
            order
                .iter()
                .map(|&sh| {
                    let sh = usize::from(sh);
                    next[sh] += 1;
                    groups[sh][next[sh] - 1].clone()
                })
                .collect(),
        )
    }

    fn column(&mut self, n: usize) -> Vec<Js> {
        match self.u8() {
            COL_NONE => vec![Js::Null; n],
            COL_BOOL => (0..n).map(|_| Js::Bool(self.u8() != 0)).collect(),
            COL_F64 => {
                self.align(8);
                (0..n).map(|_| Js::Num(self.f64())).collect()
            }
            COL_OPT_F64 => {
                let mask = self.take(n);
                self.align(8);
                mask.iter().map(|&m| (m, self.f64())).map(|(m, x)| if m != 0 { Js::Num(x) } else { Js::Null }).collect()
            }
            kind @ (COL_STR | COL_OPT_STR) => {
                self.align(4);
                (0..n)
                    .map(|_| match self.u32() {
                        NO_STR if kind == COL_OPT_STR => Js::Null,
                        i => Js::Str(self.strings[i as usize].clone()),
                    })
                    .collect()
            }
            COL_NUM_SEQ => {
                let lens = self.take(n);
                self.align(8);
                lens.iter()
                    .map(|&len| {
                        if len == NO_SEQ {
                            Js::Null
                        } else {
                            Js::Arr((0..len).map(|_| Js::Num(self.f64())).collect())
                        }
                    })
                    .collect()
            }
            COL_GENERIC => (0..n).map(|_| self.value()).collect(),
            k => panic!("unknown column {k}"),
        }
    }
}

/// Decodes a PJB1 block as JavaScript does (strings table first).
pub fn decode_block(b: &[u8]) -> Js {
    assert_eq!(&b[..4], MAGIC);
    let table = u32::from_le_bytes(b[8..12].try_into().unwrap()) as usize;
    let mut d = Dec { b, pos: table, strings: Vec::new() };
    if table != 0 {
        let count = d.u32() as usize;
        let n_special = d.u32() as usize;
        let blob_len = d.u32() as usize;
        let blob = utf16_of_utf8(&d.take(blob_len));
        d.align(4);
        let ends: Vec<usize> = (0..count - n_special).map(|_| d.u32() as usize).collect();
        let mut strings: Vec<Option<Vec<u16>>> = vec![None; count];
        for _ in 0..n_special {
            let i = d.u32() as usize;
            let n = d.u32() as usize;
            let units: Vec<u16> =
                (0..n).map(|k| u16::from_le_bytes([d.b[d.pos + 2 * k], d.b[d.pos + 2 * k + 1]])).collect();
            d.pos += 2 * n;
            d.align(4);
            assert!(strings[i].is_none());
            strings[i] = Some(units);
        }
        assert_eq!(d.pos, b.len(), "the string table ends the block");
        let mut start = 0;
        let mut ends = ends.into_iter();
        for slot in strings.iter_mut().filter(|x| x.is_none()) {
            let end = ends.next().unwrap();
            *slot = Some(blob[start..end].to_vec());
            start = end;
        }
        assert_eq!(start, blob.len());
        d.strings = strings.into_iter().map(Option::unwrap).collect();
    }
    d.pos = HEADER_LEN;
    let v = d.value();
    let end = if table != 0 { table } else { b.len() };
    assert!(d.pos <= end && end - d.pos < 4, "the value fills the body");
    v
}

// ---------------------------------------------------------------------------
// JSON results, as JSON.parse and the fixup of js/src/core/wire.ts give them

pub struct JsonDec<'a> {
    b: &'a [u8],
    pos: usize,
}

impl JsonDec<'_> {
    fn peek(&self) -> u8 {
        self.b[self.pos]
    }
    fn eat(&mut self, c: u8) {
        assert_eq!(self.b[self.pos], c, "JSON at {}", self.pos);
        self.pos += 1;
    }
    fn string(&mut self) -> Vec<u16> {
        self.eat(b'"');
        let mut out = Vec::new();
        loop {
            let c = self.b[self.pos];
            self.pos += 1;
            match c {
                b'"' => return out,
                b'\\' => {
                    let e = self.b[self.pos];
                    self.pos += 1;
                    match e {
                        b'"' => out.push(0x22),
                        b'\\' => out.push(0x5c),
                        b'/' => out.push(0x2f),
                        b'n' => out.push(10),
                        b't' => out.push(9),
                        b'r' => out.push(13),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'u' => {
                            let h = std::str::from_utf8(&self.b[self.pos..self.pos + 4]).unwrap();
                            out.push(u16::from_str_radix(h, 16).unwrap());
                            self.pos += 4;
                        }
                        _ => panic!("bad escape"),
                    }
                }
                c if c < 0x20 => panic!("a raw control character in a JSON string"),
                _ => {
                    let start = self.pos - 1;
                    let len = match c {
                        0..=0x7f => 1,
                        0xc0..=0xdf => 2,
                        0xe0..=0xef => 3,
                        _ => 4,
                    };
                    self.pos = start + len;
                    out.extend(std::str::from_utf8(&self.b[start..start + len]).unwrap().encode_utf16());
                }
            }
        }
    }
    pub fn value(&mut self) -> Js {
        match self.peek() {
            b'n' => {
                self.pos += 4;
                Js::Null
            }
            b't' => {
                self.pos += 4;
                Js::Bool(true)
            }
            b'f' => {
                self.pos += 5;
                Js::Bool(false)
            }
            b'"' => Js::Str(self.string()),
            b'[' => {
                self.pos += 1;
                let mut items = Vec::new();
                if self.peek() == b']' {
                    self.pos += 1;
                    return Js::Arr(items);
                }
                loop {
                    items.push(self.value());
                    if self.peek() == b',' {
                        self.pos += 1;
                    } else {
                        self.eat(b']');
                        return Js::Arr(items);
                    }
                }
            }
            b'{' => {
                self.pos += 1;
                let mut items = Vec::new();
                if self.peek() == b'}' {
                    self.pos += 1;
                    return Js::Obj(items);
                }
                loop {
                    let k = self.string();
                    self.eat(b':');
                    items.push((k, self.value()));
                    if self.peek() == b',' {
                        self.pos += 1;
                    } else {
                        self.eat(b'}');
                        return Js::Obj(items);
                    }
                }
            }
            _ => {
                let start = self.pos;
                while self.pos < self.b.len() && b"+-0123456789.eE".contains(&self.b[self.pos]) {
                    self.pos += 1;
                }
                let text = std::str::from_utf8(&self.b[start..self.pos]).unwrap();
                Js::Num(text.parse::<f64>().unwrap_or_else(|_| panic!("number {text}")))
            }
        }
    }
}

fn hex(units: &[u16]) -> Vec<u8> {
    let t = String::from_utf16(units).unwrap();
    (0..t.len() / 2).map(|i| u8::from_str_radix(&t[2 * i..2 * i + 2], 16).unwrap()).collect()
}

/// The fixup walk of `decodeJson`.
pub fn fixup(v: Js) -> Js {
    match v {
        Js::Arr(items) => Js::Arr(items.into_iter().map(fixup).collect()),
        Js::Obj(items) if items.len() == 1 && items[0].0 == [0] => {
            let Js::Arr(sp) = items.into_iter().next().unwrap().1 else { panic!("special") };
            let Js::Str(tag) = &sp[0] else { panic!("special tag") };
            let tag = String::from_utf16(tag).unwrap();
            let arg = |i: usize| match &sp[i] {
                Js::Str(u) => u.clone(),
                _ => panic!("special argument"),
            };
            match tag.as_str() {
                "nan" => Js::Num(f64::NAN),
                "inf" => Js::Num(f64::INFINITY),
                "-inf" => Js::Num(f64::NEG_INFINITY),
                "int" => Js::Big(String::from_utf16(&arg(1)).unwrap()),
                "bytes" => Js::Bytes(hex(&arg(1))),
                "kw" => Js::Kw(hex(&arg(1))),
                "lit" => Js::Lit(Box::new(fixup(sp[1].clone()))),
                "ref" => Js::Ref(Box::new(fixup(sp[1].clone()))),
                "stream" => Js::Stream(sp[1..].iter().cloned().map(fixup).collect()),
                t => panic!("unknown special {t}"),
            }
        }
        Js::Obj(items) => Js::Obj(
            items
                .into_iter()
                .map(|(k, v)| (if k.first() == Some(&0) { k[1..].to_vec() } else { k }, fixup(v)))
                .collect(),
        ),
        other => other,
    }
}

pub fn decode_json(b: &[u8], needs_fixup: bool) -> Js {
    let mut d = JsonDec { b, pos: 0 };
    let v = d.value();
    assert_eq!(d.pos, b.len(), "the JSON text is one value");
    if needs_fixup {
        fixup(v)
    } else {
        v
    }
}

// ---------------------------------------------------------------------------
// Requests

/// A decoded response.
#[derive(Debug)]
pub struct Resp {
    pub status: u8,
    pub value: Js,
    pub diagnostics: Js,
}

/// The request block of an operation, as JavaScript writes it.
pub fn request(op: &str, handle: u32, args: &PyValue, min_level: i64) -> Vec<u8> {
    let mut w = BinWriter::without_tables();
    let req =
        PyValue::list([PyValue::str(op), PyValue::from(i64::from(handle)), args.clone(), PyValue::from(min_level)]);
    write_value(&mut w, &req).unwrap();
    w.finish()
}

/// Decodes a response envelope.
pub fn response(r: &[u8]) -> Resp {
    assert_eq!(&r[..4], b"PJR1");
    let u32_at = |i: usize| u32::from_le_bytes(r[i..i + 4].try_into().unwrap()) as usize;
    let (po, pl, dof, dl) = (u32_at(8), u32_at(12), u32_at(16), u32_at(20));
    assert!(po % 8 == 0 && dof % 8 == 0 && po >= 24 && dof >= po + pl && dof + dl == r.len());
    let payload = &r[po..po + pl];
    let value = match r[5] {
        0 => {
            assert_eq!(r[6], 0);
            decode_block(payload)
        }
        1 => decode_json(payload, r[6] & 1 != 0),
        f => panic!("format {f}"),
    };
    Resp { status: r[4], value, diagnostics: decode_block(&r[dof..dof + dl]) }
}

pub fn call(core: &mut Core, op: &str, handle: u32, args: PyValue, data: Option<Vec<u8>>) -> Resp {
    call_level(core, op, handle, args, data, 0)
}

pub fn call_level(core: &mut Core, op: &str, handle: u32, args: PyValue, data: Option<Vec<u8>>, level: i64) -> Resp {
    response(&core.call(&request(op, handle, &args, level), data.map(Rc::from)))
}

/// The value of a successful response.
pub fn ok(r: Resp) -> Js {
    assert_eq!(r.status, 0, "{:?}", r.value);
    r.value
}

pub fn list(items: Vec<PyValue>) -> PyValue {
    PyValue::List(Rc::from(items))
}

/// Runs `f` on a new thread with a stack of `stack` bytes (fresh
/// thread-local state: CMap caches, providers); under WebAssembly, which has
/// no threads here, on the current one.
pub fn on_thread<T: Send + 'static>(stack: usize, f: impl FnOnce() -> T + Send + 'static) -> T {
    #[cfg(target_family = "wasm")]
    {
        let _ = stack;
        f()
    }
    #[cfg(not(target_family = "wasm"))]
    {
        std::thread::Builder::new().stack_size(stack).spawn(f).unwrap().join().unwrap()
    }
}

/// The result formats a test runs in turn, each on a fresh thread: both
/// natively, one under WebAssembly (no fresh thread for the second).
pub const FORMATS: &[&str] = if cfg!(target_family = "wasm") { &["binary"] } else { &["binary", "json"] };

/// A minimal valid PDF: one 200 x 200 page whose content stream is
/// `content` (uncompressed), with a correct cross-reference table.
pub fn tiny_pdf(content: &[u8]) -> Vec<u8> {
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Resources << >> >>".to_vec(),
        [format!("<< /Length {} >>\nstream\n", content.len()).as_bytes(), content, b"\nendstream"].concat(),
    ];
    for (i, o) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes(),
    );
    out
}
