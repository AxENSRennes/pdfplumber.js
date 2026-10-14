//! Faithful port of pdfminer.six psparser.py (PSBaseParser + PSStackParser) and
//! pdfinterp.py PDFContentParser, compiled to wasm32 without wasm-bindgen.
//!
//! Same state machine as ../ts/psparser.ts. Results reach JavaScript either as a
//! compact binary tape (decoded into the same JS objects the TS port builds) or
//! not at all (`parse_count`, the "everything stays in Rust" lower bound).

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

// ---------------------------------------------------------------------------
// Values

#[derive(Debug)]
pub enum Obj {
    Int(i64),
    /// Python int outside i64, as a normalized decimal string.
    Big(Box<[u8]>),
    Float(f64),
    Bool(bool),
    Str(Box<[u8]>),
    Lit(u32),
    Kw(u32),
    Array(Vec<Obj>),
    Dict(Dict),
    Stream(Box<(Dict, Box<[u8]>)>),
}

/// Python dict with str keys: insertion ordered, later duplicates replace in place.
pub type Dict = Vec<(Box<str>, Obj)>;

fn dict_set(d: &mut Dict, k: String, v: Obj) {
    if let Some(slot) = d.iter_mut().find(|(key, _)| **key == *k) {
        slot.1 = v;
    } else {
        d.push((k.into_boxed_str(), v));
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PErr {
    Eof,
    Assertion,
    Syntax,
    Index,
    Type,
    Key,
}

impl PErr {
    fn code(self) -> i32 {
        match self {
            PErr::Eof => 0,
            PErr::Assertion => 1,
            PErr::Syntax => 2,
            PErr::Index => 3,
            PErr::Type => 4,
            PErr::Key => 5,
        }
    }
}

// ---------------------------------------------------------------------------
// Interning (PSLiteralTable / PSKeywordTable)

#[derive(Default)]
struct Fx(u64);
impl Hasher for Fx {
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0.rotate_left(5) ^ b as u64).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
        }
    }
    fn finish(&self) -> u64 {
        self.0
    }
}
type FxMap<K, V> = HashMap<K, V, BuildHasherDefault<Fx>>;

pub struct Interner {
    /// (name bytes, name is a valid UTF-8 str)
    lits: Vec<(Box<[u8]>, bool)>,
    lit_str: FxMap<Vec<u8>, u32>,
    lit_bytes: FxMap<Vec<u8>, u32>,
    kws: Vec<Box<[u8]>>,
    kw_map: FxMap<Vec<u8>, u32>,
    /// Direct tables for 1- and 2-byte operators (u32::MAX = not interned yet).
    kw1: Box<[u32; 256]>,
    kw2: Box<[u32]>,
}

const KW_PROC_BEGIN: u32 = 0;
const KW_PROC_END: u32 = 1;
const KW_ARRAY_BEGIN: u32 = 2;
const KW_ARRAY_END: u32 = 3;
const KW_DICT_BEGIN: u32 = 4;
const KW_DICT_END: u32 = 5;
const KW_BI: u32 = 6;
const KW_ID: u32 = 7;
const KW_EI: u32 = 8;
const LIT_ASCII85: u32 = 0;
const LIT_A85: u32 = 1;

impl Interner {
    pub fn new() -> Self {
        let mut t = Interner {
            lits: Vec::new(),
            lit_str: FxMap::default(),
            lit_bytes: FxMap::default(),
            kws: Vec::new(),
            kw_map: FxMap::default(),
            kw1: Box::new([u32::MAX; 256]),
            kw2: vec![u32::MAX; 65536].into_boxed_slice(),
        };
        for k in [&b"{"[..], b"}", b"[", b"]", b"<<", b">>", b"BI", b"ID", b"EI"] {
            t.kwd(k);
        }
        t.lit(b"ASCII85Decode");
        t.lit(b"A85");
        t
    }

    #[inline]
    fn kwd(&mut self, name: &[u8]) -> u32 {
        let slot = match name.len() {
            1 => Some(&mut self.kw1[name[0] as usize]),
            2 => Some(&mut self.kw2[(name[0] as usize) << 8 | name[1] as usize]),
            _ => None,
        };
        if let Some(slot) = slot {
            if *slot != u32::MAX {
                return *slot;
            }
            let id = self.kwd_slow(name);
            match name.len() {
                1 => self.kw1[name[0] as usize] = id,
                _ => self.kw2[(name[0] as usize) << 8 | name[1] as usize] = id,
            }
            return id;
        }
        self.kwd_slow(name)
    }

    fn kwd_slow(&mut self, name: &[u8]) -> u32 {
        if let Some(&id) = self.kw_map.get(name) {
            return id;
        }
        let id = self.kws.len() as u32;
        self.kws.push(name.into());
        self.kw_map.insert(name.to_vec(), id);
        id
    }

    /// LIT(str(curtoken, "utf-8")) or LIT(curtoken) when it is not valid UTF-8.
    fn lit(&mut self, name: &[u8]) -> u32 {
        let is_str = std::str::from_utf8(name).is_ok();
        let map = if is_str { &mut self.lit_str } else { &mut self.lit_bytes };
        if let Some(&id) = map.get(name) {
            return id;
        }
        let id = self.lits.len() as u32;
        map.insert(name.to_vec(), id);
        self.lits.push((name.into(), is_str));
        id
    }
}

// ---------------------------------------------------------------------------
// Python str()/repr() for non-literal dictionary keys (literal_name, STRICT off)

fn py_bytes_repr(b: &[u8], out: &mut String) {
    let quote = if b.contains(&b'\'') && !b.contains(&b'"') { '"' } else { '\'' };
    out.push('b');
    out.push(quote);
    for &c in b {
        match c {
            b'\\' => out.push_str("\\\\"),
            b'\t' => out.push_str("\\t"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            _ if c as char == quote => {
                out.push('\\');
                out.push(quote);
            }
            _ if c < 0x20 || c >= 0x7f => out.push_str(&format!("\\x{c:02x}")),
            _ => out.push(c as char),
        }
    }
    out.push(quote);
}

fn py_str_repr(s: &str, out: &mut String) {
    let quote = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    out.push(quote);
    for ch in s.chars() {
        let c = ch as u32;
        match ch {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ if ch == quote => {
                out.push('\\');
                out.push(quote);
            }
            _ if c < 0x20 || c == 0x7f || (0x80..0xa0).contains(&c) => out.push_str(&format!("\\x{c:02x}")),
            _ => out.push(ch),
        }
    }
    out.push(quote);
}

/// Python float repr: shortest round-trip digits, exponent form outside [1e-4, 1e16).
pub fn py_float_repr(x: f64) -> String {
    if x.is_nan() {
        return "nan".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf".into() } else { "-inf".into() };
    }
    if x == 0.0 {
        return if x.is_sign_negative() { "-0.0".into() } else { "0.0".into() };
    }
    let sci = format!("{:e}", x.abs()); // e.g. "1.5e-5", shortest round-trip digits
    let (mant, exp) = sci.split_once('e').unwrap();
    let exp: i32 = exp.parse().unwrap();
    let digits: String = mant.chars().filter(|c| *c != '.').collect();
    let nd = digits.len() as i32;
    let body = if (-5..16).contains(&exp) {
        if exp >= nd - 1 {
            format!("{}{}.0", digits, "0".repeat((exp - nd + 1) as usize))
        } else if exp >= 0 {
            format!("{}.{}", &digits[..(exp + 1) as usize], &digits[(exp + 1) as usize..])
        } else {
            format!("0.{}{}", "0".repeat((-exp - 1) as usize), digits)
        }
    } else {
        let m = if nd > 1 { format!("{}.{}", &digits[..1], &digits[1..]) } else { digits };
        format!("{}e{}{:02}", m, if exp < 0 { '-' } else { '+' }, exp.abs())
    };
    if x < 0.0 {
        format!("-{body}")
    } else {
        body
    }
}

fn py_repr(t: &Interner, v: &Obj, out: &mut String) {
    match v {
        Obj::Int(i) => out.push_str(&i.to_string()),
        Obj::Big(d) => out.push_str(std::str::from_utf8(d).unwrap()),
        Obj::Float(f) => out.push_str(&py_float_repr(*f)),
        Obj::Bool(b) => out.push_str(if *b { "True" } else { "False" }),
        Obj::Str(b) => py_bytes_repr(b, out),
        Obj::Lit(id) => {
            let (name, is_str) = &t.lits[*id as usize];
            out.push('/');
            if *is_str {
                py_str_repr(std::str::from_utf8(name).unwrap(), out)
            } else {
                py_bytes_repr(name, out)
            }
        }
        Obj::Kw(id) => {
            out.push('/');
            py_bytes_repr(&t.kws[*id as usize], out)
        }
        Obj::Array(items) => {
            out.push('[');
            for (k, x) in items.iter().enumerate() {
                if k > 0 {
                    out.push_str(", ");
                }
                py_repr(t, x, out);
            }
            out.push(']');
        }
        Obj::Dict(d) => {
            out.push('{');
            for (k, (key, x)) in d.iter().enumerate() {
                if k > 0 {
                    out.push_str(", ");
                }
                py_str_repr(key, out);
                out.push_str(": ");
                py_repr(t, x, out);
            }
            out.push('}');
        }
        Obj::Stream(_) => out.push_str("<PDFStream>"),
    }
}

/// pdfminer.psparser.literal_name with settings.STRICT == False.
fn literal_name(t: &Interner, x: &Obj) -> String {
    let mut s = String::new();
    match x {
        Obj::Lit(id) => {
            let (name, is_str) = &t.lits[*id as usize];
            if *is_str {
                s.push_str(std::str::from_utf8(name).unwrap());
            } else {
                py_bytes_repr(name, &mut s);
            }
        }
        _ => py_repr(t, x, &mut s),
    }
    s
}

// ---------------------------------------------------------------------------
// Byte classes (Python bytes regex semantics: \s == [\t\n\v\f\r ])

const SPACE: u8 = 1;
const DIGIT: u8 = 2;
const HEXD: u8 = 4;
const ALPHA: u8 = 8;
const END_LITERAL: u8 = 16; // [#/%\[\]()<>{}\s], also END_KEYWORD
const END_STRING: u8 = 32; // [()\\]
const EOL: u8 = 64;
const OCT: u8 = 128;

const CLS: [u8; 256] = {
    let mut t = [0u8; 256];
    let spaces = [0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x20];
    let mut k = 0;
    while k < spaces.len() {
        t[spaces[k]] |= SPACE | END_LITERAL;
        k += 1;
    }
    let mut c = b'0';
    while c <= b'9' {
        t[c as usize] |= DIGIT | HEXD;
        if c <= b'7' {
            t[c as usize] |= OCT;
        }
        c += 1;
    }
    let mut c = b'A';
    while c <= b'Z' {
        t[c as usize] |= ALPHA;
        t[(c + 32) as usize] |= ALPHA;
        if c <= b'F' {
            t[c as usize] |= HEXD;
            t[(c + 32) as usize] |= HEXD;
        }
        c += 1;
    }
    let delims = b"#/%[]()<>{}";
    let mut k = 0;
    while k < delims.len() {
        t[delims[k] as usize] |= END_LITERAL;
        k += 1;
    }
    t[b'(' as usize] |= END_STRING;
    t[b')' as usize] |= END_STRING;
    t[b'\\' as usize] |= END_STRING;
    t[b'\n' as usize] |= EOL;
    t[b'\r' as usize] |= EOL;
    t
};

fn esc_string(c: u8) -> Option<u8> {
    match c {
        b'b' => Some(8),
        b't' => Some(9),
        b'n' => Some(10),
        b'f' => Some(12),
        b'r' => Some(13),
        b'(' => Some(40),
        b')' => Some(41),
        b'\\' => Some(92),
        _ => None,
    }
}

#[inline]
fn hexval(c: u8) -> u32 {
    if c <= b'9' {
        (c - b'0') as u32
    } else {
        ((c | 0x20) - b'a' + 10) as u32
    }
}

const BUFSIZ: usize = 4096;
const NEWLINE: &[u8] = b"\n";
const POW10: [f64; 23] = [
    1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10, 1e11, 1e12, 1e13, 1e14, 1e15, 1e16, 1e17, 1e18, 1e19, 1e20, 1e21, 1e22,
];

#[derive(Clone, Copy, PartialEq)]
enum State {
    Main,
    Comment,
    Literal,
    LiteralHex,
    Number,
    Float,
    Keyword,
    Str,
    Str1,
    WOpen,
    WClose,
    HexString,
}

#[derive(Clone, Copy, PartialEq)]
enum CurType {
    Array,
    Dict,
    Proc,
    Inline,
}

type Entry = (usize, Obj);

/// pdfminer.pdfinterp.PDFContentParser (with its PSStackParser/PSBaseParser bases).
pub struct Parser<'a, 't> {
    t: &'t mut Interner,
    streams: Vec<&'a [u8]>,
    istream: usize,
    fp: Option<&'a [u8]>,
    fp_pos: usize,
    buf: &'a [u8],
    bufpos: usize,
    charpos: usize,
    eof: bool,

    state: State,
    tok: Vec<u8>, // _curtoken
    curtokenpos: usize,
    paren: i64,
    hex: u32,
    hexlen: u32,
    oct: u32,
    octlen: u32,
    tokens: Vec<Entry>,
    tok_head: usize,

    context: Vec<(usize, Option<CurType>, Vec<Entry>)>,
    curtype: Option<CurType>,
    curstack: Vec<Entry>,
    results: Vec<Entry>,
    res_head: usize,
}

impl<'a, 't> Parser<'a, 't> {
    pub fn new(t: &'t mut Interner, streams: Vec<&'a [u8]>) -> Result<Self, PErr> {
        let mut p = Parser {
            t,
            streams,
            istream: 0,
            fp: None,
            fp_pos: 0,
            buf: &[],
            bufpos: 0,
            charpos: 0,
            eof: false,
            state: State::Main,
            tok: Vec::with_capacity(256),
            curtokenpos: 0,
            paren: 0,
            hex: 0,
            hexlen: 0,
            oct: 0,
            octlen: 0,
            tokens: Vec::new(),
            tok_head: 0,
            context: Vec::new(),
            curtype: None,
            curstack: Vec::new(),
            results: Vec::new(),
            res_head: 0,
        };
        p.seek(0)?;
        p.reset();
        Ok(p)
    }

    // -- PDFContentParser stream plumbing --------------------------------------

    fn fillfp(&mut self) -> Result<bool, PErr> {
        if self.fp.is_none() {
            if self.istream < self.streams.len() {
                self.fp = Some(self.streams[self.istream]);
                self.istream += 1;
                self.fp_pos = 0;
            } else {
                return Err(PErr::Eof);
            }
            return Ok(true);
        }
        Ok(false)
    }

    fn seek(&mut self, pos: usize) -> Result<(), PErr> {
        self.fillfp()?;
        self.fp_pos = pos;
        self.bufpos = pos;
        self.buf = &[];
        self.charpos = 0;
        self.state = State::Main;
        self.tok.clear();
        self.curtokenpos = 0;
        self.tokens.clear();
        self.tok_head = 0;
        self.eof = false;
        self.reset();
        Ok(())
    }

    fn reset(&mut self) {
        self.context.clear();
        self.curtype = None;
        self.curstack.clear();
        self.results.clear();
        self.res_head = 0;
    }

    fn fillbuf(&mut self) -> Result<bool, PErr> {
        if self.charpos < self.buf.len() {
            return Ok(false);
        }
        let mut new_stream;
        loop {
            new_stream = self.fillfp()?;
            let fp = self.fp.unwrap();
            self.bufpos = self.fp_pos;
            let end = fp.len().min(self.fp_pos + BUFSIZ);
            self.buf = if self.fp_pos < end { &fp[self.fp_pos..end] } else { &[] };
            self.fp_pos = self.fp_pos.max(end);
            if !self.buf.is_empty() {
                break;
            }
            self.fp = None;
        }
        self.charpos = 0;
        Ok(new_stream)
    }

    // -- tokenizer helpers ---------------------------------------------------

    #[inline]
    fn add_token(&mut self, obj: Obj) {
        self.tokens.push((self.curtokenpos, obj));
    }

    /// int(self._curtoken); None when Python raises ValueError.
    fn parse_int(&self) -> Option<Obj> {
        let tok = &self.tok[..];
        let (neg, digits) = match tok.first() {
            Some(b'-') => (true, &tok[1..]),
            Some(b'+') => (false, &tok[1..]),
            _ => (false, tok),
        };
        if digits.is_empty() || digits.len() > 4300 {
            return None; // ValueError, or sys.int_info.default_max_str_digits
        }
        let first = digits.iter().position(|&c| c != b'0').unwrap_or(digits.len());
        let sig = &digits[first..];
        if sig.len() <= 18 {
            let mut v: i64 = 0;
            for &c in sig {
                v = v * 10 + (c - b'0') as i64;
            }
            return Some(Obj::Int(if neg { -v } else { v }));
        }
        let mut s = Vec::with_capacity(sig.len() + 1);
        if neg {
            s.push(b'-');
        }
        s.extend_from_slice(sig);
        Some(Obj::Big(s.into_boxed_slice()))
    }

    /// float(self._curtoken); None when Python raises ValueError.
    fn parse_float(&self) -> Option<f64> {
        let tok = &self.tok[..];
        let (neg, body) = match tok.first() {
            Some(b'-') => (true, &tok[1..]),
            Some(b'+') => (false, &tok[1..]),
            _ => (false, tok),
        };
        let mut mant: u64 = 0;
        let mut ndigits = 0usize;
        let mut frac: i32 = -1;
        for &c in body {
            if c == b'.' {
                frac = 0;
            } else {
                if ndigits < 19 {
                    mant = mant * 10 + (c - b'0') as u64;
                }
                ndigits += 1;
                if frac >= 0 {
                    frac += 1;
                }
            }
        }
        if ndigits == 0 {
            return None;
        }
        // Clinger fast path: exact mantissa and power of ten give a correctly rounded result.
        let v = if ndigits <= 15 && frac <= 22 {
            if frac > 0 {
                mant as f64 / POW10[frac as usize]
            } else {
                mant as f64
            }
        } else {
            std::str::from_utf8(body).ok()?.parse::<f64>().ok()?
        };
        Some(if neg { -v } else { v })
    }

    // -- PSBaseParser._parse_* -----------------------------------------------

    fn parse1(&mut self, s: &[u8], mut i: usize) -> Result<usize, PErr> {
        let n = s.len();
        match self.state {
            State::Main => {
                while i < n && CLS[s[i] as usize] & SPACE != 0 {
                    i += 1;
                }
                if i >= n {
                    return Ok(n);
                }
                let j = i;
                let c = s[j];
                self.curtokenpos = self.bufpos + j;
                match c {
                    b'%' => {
                        self.tok.clear();
                        self.tok.push(c);
                        self.state = State::Comment;
                    }
                    b'/' => {
                        self.tok.clear();
                        self.state = State::Literal;
                    }
                    b'-' | b'+' | b'0'..=b'9' => {
                        self.tok.clear();
                        self.tok.push(c);
                        self.state = State::Number;
                    }
                    b'.' => {
                        self.tok.clear();
                        self.tok.push(c);
                        self.state = State::Float;
                    }
                    b'A'..=b'Z' | b'a'..=b'z' => {
                        self.tok.clear();
                        self.tok.push(c);
                        self.state = State::Keyword;
                    }
                    b'(' => {
                        self.tok.clear();
                        self.paren = 1;
                        self.state = State::Str;
                    }
                    b'<' => {
                        self.tok.clear();
                        self.state = State::WOpen;
                    }
                    b'>' => {
                        self.tok.clear();
                        self.state = State::WClose;
                    }
                    0 => {}
                    _ => {
                        let id = self.t.kwd(&s[j..j + 1]);
                        self.add_token(Obj::Kw(id));
                    }
                }
                Ok(j + 1)
            }
            State::Comment => {
                let mut j = i;
                while j < n && CLS[s[j] as usize] & EOL == 0 {
                    j += 1;
                }
                self.tok.extend_from_slice(&s[i..j]);
                if j >= n {
                    return Ok(n);
                }
                self.state = State::Main;
                Ok(j)
            }
            State::Literal => {
                let mut j = i;
                while j < n && CLS[s[j] as usize] & END_LITERAL == 0 {
                    j += 1;
                }
                self.tok.extend_from_slice(&s[i..j]);
                if j >= n {
                    return Ok(n);
                }
                if s[j] == b'#' {
                    self.hex = 0;
                    self.hexlen = 0;
                    self.state = State::LiteralHex;
                    return Ok(j + 1);
                }
                let id = self.t.lit(&self.tok);
                self.add_token(Obj::Lit(id));
                self.state = State::Main;
                Ok(j)
            }
            State::LiteralHex => {
                if i < n && CLS[s[i] as usize] & HEXD != 0 && self.hexlen < 2 {
                    self.hex = self.hex * 16 + hexval(s[i]);
                    self.hexlen += 1;
                    return Ok(i + 1);
                }
                if self.hexlen > 0 {
                    self.tok.push(self.hex as u8);
                }
                self.state = State::Literal;
                Ok(i)
            }
            State::Number => {
                let mut j = i;
                while j < n && CLS[s[j] as usize] & DIGIT != 0 {
                    j += 1;
                }
                self.tok.extend_from_slice(&s[i..j]);
                if j >= n {
                    return Ok(n);
                }
                if s[j] == b'.' {
                    self.tok.push(b'.');
                    self.state = State::Float;
                    return Ok(j + 1);
                }
                if let Some(v) = self.parse_int() {
                    self.add_token(v);
                }
                self.state = State::Main;
                Ok(j)
            }
            State::Float => {
                let mut j = i;
                while j < n && CLS[s[j] as usize] & DIGIT != 0 {
                    j += 1;
                }
                self.tok.extend_from_slice(&s[i..j]);
                if j >= n {
                    return Ok(n);
                }
                if let Some(v) = self.parse_float() {
                    self.add_token(Obj::Float(v));
                }
                self.state = State::Main;
                Ok(j)
            }
            State::Keyword => {
                let mut j = i;
                while j < n && CLS[s[j] as usize] & END_LITERAL == 0 {
                    j += 1;
                }
                self.tok.extend_from_slice(&s[i..j]);
                if j >= n {
                    return Ok(n);
                }
                let token = match &self.tok[..] {
                    b"true" => Obj::Bool(true),
                    b"false" => Obj::Bool(false),
                    name => Obj::Kw(self.t.kwd(name)),
                };
                self.add_token(token);
                self.state = State::Main;
                Ok(j)
            }
            State::Str => {
                let mut j = i;
                while j < n && CLS[s[j] as usize] & END_STRING == 0 {
                    j += 1;
                }
                self.tok.extend_from_slice(&s[i..j]);
                if j >= n {
                    return Ok(n);
                }
                let c = s[j];
                if c == b'\\' {
                    self.oct = 0;
                    self.octlen = 0;
                    self.state = State::Str1;
                    return Ok(j + 1);
                }
                if c == b'(' {
                    self.paren += 1;
                    self.tok.push(c);
                    return Ok(j + 1);
                }
                self.paren -= 1;
                if self.paren != 0 {
                    self.tok.push(c);
                    return Ok(j + 1);
                }
                let token = Obj::Str(self.tok.as_slice().into());
                self.add_token(token);
                self.state = State::Main;
                Ok(j + 1)
            }
            State::Str1 => {
                let c = if i < n { Some(s[i]) } else { None };
                if let Some(c) = c.filter(|&c| CLS[c as usize] & OCT != 0 && self.octlen < 3) {
                    self.oct = self.oct * 8 + (c - b'0') as u32;
                    self.octlen += 1;
                    return Ok(i + 1);
                } else if self.octlen > 0 {
                    if self.oct >= 256 {
                        return Err(PErr::Assertion);
                    }
                    self.tok.push(self.oct as u8);
                    self.state = State::Str;
                    return Ok(i);
                } else if let Some(e) = c.and_then(esc_string) {
                    self.tok.push(e);
                } else if c == Some(b'\r') && n > i + 1 && s[i + 1] == b'\n' {
                    i += 1;
                }
                self.state = State::Str;
                Ok(i + 1)
            }
            State::WOpen => {
                if i < n && s[i] == b'<' {
                    self.add_token(Obj::Kw(KW_DICT_BEGIN));
                    self.state = State::Main;
                    return Ok(i + 1);
                }
                self.state = State::HexString;
                Ok(i)
            }
            State::WClose => {
                if i < n && s[i] == b'>' {
                    self.add_token(Obj::Kw(KW_DICT_END));
                    i += 1;
                }
                self.state = State::Main;
                Ok(i)
            }
            State::HexString => {
                let mut j = i;
                while j < n && CLS[s[j] as usize] & (SPACE | HEXD) != 0 {
                    j += 1;
                }
                self.tok.extend_from_slice(&s[i..j]);
                if j >= n {
                    return Ok(n);
                }
                // HEX_PAIR.sub(..., SPC.sub(b"", curtoken)): pairs, then a lone trailing digit.
                let mut out = Vec::with_capacity((self.tok.len() + 1) / 2);
                let mut pending: Option<u32> = None;
                for &c in &self.tok {
                    if CLS[c as usize] & SPACE != 0 {
                        continue;
                    }
                    let v = hexval(c);
                    match pending.take() {
                        None => pending = Some(v),
                        Some(h) => out.push(((h << 4) | v) as u8),
                    }
                }
                if let Some(h) = pending {
                    out.push(h as u8);
                }
                self.add_token(Obj::Str(out.into_boxed_slice()));
                self.state = State::Main;
                Ok(j)
            }
        }
    }

    fn nexttoken(&mut self) -> Result<(), PErr> {
        if self.eof {
            return Err(PErr::Eof);
        }
        while self.tok_head >= self.tokens.len() {
            let step = (|| -> Result<(), PErr> {
                let changed = self.fillbuf()?;
                if changed && !self.tok.is_empty() {
                    self.parse1(NEWLINE, 0)?;
                } else {
                    let buf = self.buf;
                    self.charpos = self.parse1(buf, self.charpos)?;
                }
                Ok(())
            })();
            match step {
                Ok(()) => {}
                Err(PErr::Eof) => {
                    self.charpos = self.parse1(NEWLINE, 0)?;
                    self.eof = true;
                    if self.tok_head >= self.tokens.len() {
                        return Err(PErr::Eof);
                    }
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    // -- PSStackParser -------------------------------------------------------

    fn start_type(&mut self, pos: usize, ty: CurType) {
        let stack = std::mem::take(&mut self.curstack);
        self.context.push((pos, self.curtype, stack));
        self.curtype = Some(ty);
    }

    /// end_type(); None where pdfminer raises PSTypeError (swallowed when not STRICT).
    fn end_type(&mut self, ty: CurType) -> Option<(usize, Vec<Obj>)> {
        if self.curtype != Some(ty) {
            return None;
        }
        let objs = std::mem::take(&mut self.curstack).into_iter().map(|(_, o)| o).collect();
        let (pos, curtype, stack) = self.context.pop().unwrap();
        self.curtype = curtype;
        self.curstack = stack;
        Some((pos, objs))
    }

    fn make_dict(&self, objs: Vec<Obj>) -> Dict {
        let mut d = Dict::new();
        let mut it = objs.into_iter();
        while let (Some(k), Some(v)) = (it.next(), it.next()) {
            dict_set(&mut d, literal_name(self.t, &k), v);
        }
        d
    }

    fn do_keyword(&mut self, pos: usize, id: u32) -> Result<(), PErr> {
        if id == KW_BI {
            self.start_type(pos, CurType::Inline);
        } else if id == KW_ID {
            let Some((_, objs)) = self.end_type(CurType::Inline) else { return Ok(()) };
            if objs.len() % 2 != 0 {
                return Ok(()); // PSTypeError, swallowed
            }
            let d = self.make_dict(objs);
            let mut eos: &[u8] = b"EI";
            if let Some((_, filter)) = d.iter().find(|(k, _)| &**k == "F") {
                // Python: filter = [filter] if PSLiteral; filter[0] in LITERALS_ASCII85_DECODE
                let first_is_a85 = match filter {
                    Obj::Lit(l) => *l == LIT_ASCII85 || *l == LIT_A85,
                    Obj::Array(items) => match items.first() {
                        None => return Err(PErr::Index),
                        Some(Obj::Lit(l)) => *l == LIT_ASCII85 || *l == LIT_A85,
                        Some(_) => false,
                    },
                    Obj::Str(b) => {
                        if b.is_empty() {
                            return Err(PErr::Index);
                        }
                        false
                    }
                    Obj::Dict(_) => return Err(PErr::Key),
                    _ => return Err(PErr::Type),
                };
                if first_is_a85 {
                    eos = b"~>";
                }
            }
            let (data_pos, mut data) = self.get_inline_data(pos + 3, eos)?;
            if eos != b"EI" {
                data.extend_from_slice(eos);
            }
            self.curstack.push((data_pos, Obj::Stream(Box::new((d, data.into_boxed_slice())))));
            if eos == b"EI" {
                self.curstack.push((data_pos, Obj::Kw(KW_EI)));
            }
        } else {
            self.curstack.push((pos, Obj::Kw(id)));
        }
        Ok(())
    }

    fn get_inline_data(&mut self, pos: usize, target: &[u8]) -> Result<(usize, Vec<u8>), PErr> {
        self.seek(pos)?;
        let mut i = 0;
        let mut data: Vec<u8> = Vec::new();
        while i <= target.len() {
            self.fillbuf()?;
            if i > 0 {
                let c = self.buf[self.charpos];
                data.push(c);
                self.charpos += 1;
                if (target.len() <= i && CLS[c as usize] & SPACE != 0) || (i < target.len() && c == target[i]) {
                    i += 1;
                } else {
                    i = 0;
                }
            } else if let Some(off) = self.buf[self.charpos..].iter().position(|&b| b == target[0]) {
                let j = self.charpos + off;
                data.extend_from_slice(&self.buf[self.charpos..=j]);
                self.charpos = j + 1;
                i = 1;
            } else {
                data.extend_from_slice(&self.buf[self.charpos..]);
                self.charpos = self.buf.len();
            }
        }
        let end = data.len().saturating_sub(target.len() + 1);
        data.truncate(strip_trailing_eol(&data, end));
        Ok((pos, data))
    }

    /// PSStackParser.nextobject().
    pub fn nextobject(&mut self) -> Result<Entry, PErr> {
        while self.res_head >= self.results.len() {
            self.nexttoken()?;
            let (pos, token) = std::mem::replace(&mut self.tokens[self.tok_head], (0, Obj::Bool(false)));
            self.tok_head += 1;
            if self.tok_head == self.tokens.len() {
                self.tokens.clear();
                self.tok_head = 0;
            }
            match token {
                Obj::Kw(KW_ARRAY_BEGIN) => self.start_type(pos, CurType::Array),
                Obj::Kw(KW_ARRAY_END) => {
                    if let Some((p, objs)) = self.end_type(CurType::Array) {
                        self.curstack.push((p, Obj::Array(objs)));
                    }
                }
                Obj::Kw(KW_DICT_BEGIN) => self.start_type(pos, CurType::Dict),
                Obj::Kw(KW_DICT_END) => {
                    if let Some((p, objs)) = self.end_type(CurType::Dict) {
                        if objs.len() % 2 != 0 {
                            return Err(PErr::Syntax);
                        }
                        let d = self.make_dict(objs);
                        self.curstack.push((p, Obj::Dict(d)));
                    }
                }
                Obj::Kw(KW_PROC_BEGIN) => self.start_type(pos, CurType::Proc),
                Obj::Kw(KW_PROC_END) => {
                    if let Some((p, objs)) = self.end_type(CurType::Proc) {
                        self.curstack.push((p, Obj::Array(objs)));
                    }
                }
                Obj::Kw(id) => self.do_keyword(pos, id)?,
                other => self.curstack.push((pos, other)),
            }
            if !self.context.is_empty() {
                continue;
            }
            // flush(): results is empty here, so curstack becomes results in O(1)
            std::mem::swap(&mut self.results, &mut self.curstack);
        }
        let entry = std::mem::replace(&mut self.results[self.res_head], (0, Obj::Bool(false)));
        self.res_head += 1;
        if self.res_head == self.results.len() {
            self.results.clear();
            self.res_head = 0;
        }
        Ok(entry)
    }
}

/// re.sub(rb"(\x0d\x0a|[\x0d\x0a])$", b"", data[:n]); see the TS port for the derivation.
fn strip_trailing_eol(data: &[u8], n: usize) -> usize {
    let is_end = |q: usize| q == n || (q + 1 == n && data[n - 1] == b'\n');
    for p in n.saturating_sub(3)..n {
        if data[p] == b'\r' && p + 1 < n && data[p + 1] == b'\n' && is_end(p + 2) {
            return p;
        }
        if (data[p] == b'\r' || data[p] == b'\n') && is_end(p + 1) {
            return p;
        }
    }
    n
}

// ---------------------------------------------------------------------------
// Tape encoding for JavaScript (little-endian)

const T_NUM: u8 = 0;
const T_BIG: u8 = 1;
const T_TRUE: u8 = 2;
const T_FALSE: u8 = 3;
const T_STR: u8 = 4;
const T_LIT: u8 = 5;
const T_LIT_DEF: u8 = 6;
const T_KW: u8 = 7;
const T_KW_DEF: u8 = 8;
const T_ARRAY: u8 = 9;
const T_DICT: u8 = 10;
const T_STREAM: u8 = 11;
const MAX_SAFE: i64 = (1 << 53) - 1;

struct Tape {
    out: Vec<u8>,
    lit_sent: Vec<bool>,
    kw_sent: Vec<bool>,
}

impl Tape {
    fn u32(&mut self, v: u32) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }
    fn bytes(&mut self, b: &[u8]) {
        self.u32(b.len() as u32);
        self.out.extend_from_slice(b);
    }
    fn dict(&mut self, t: &Interner, d: &Dict) {
        self.u32(d.len() as u32);
        for (k, v) in d {
            self.bytes(k.as_bytes());
            self.value(t, v);
        }
    }
    fn value(&mut self, t: &Interner, v: &Obj) {
        match v {
            Obj::Int(i) if (-MAX_SAFE..=MAX_SAFE).contains(i) => {
                self.out.push(T_NUM);
                self.out.extend_from_slice(&(*i as f64).to_le_bytes());
            }
            Obj::Int(i) => {
                self.out.push(T_BIG);
                self.bytes(i.to_string().as_bytes());
            }
            Obj::Big(d) => {
                self.out.push(T_BIG);
                self.bytes(d);
            }
            Obj::Float(f) => {
                self.out.push(T_NUM);
                self.out.extend_from_slice(&f.to_le_bytes());
            }
            Obj::Bool(b) => self.out.push(if *b { T_TRUE } else { T_FALSE }),
            Obj::Str(b) => {
                self.out.push(T_STR);
                self.bytes(b);
            }
            Obj::Lit(id) => {
                let k = *id as usize;
                if self.lit_sent.len() <= k {
                    self.lit_sent.resize(k + 1, false);
                }
                if self.lit_sent[k] {
                    self.out.push(T_LIT);
                    self.u32(*id);
                } else {
                    self.lit_sent[k] = true;
                    let (name, is_str) = &t.lits[k];
                    self.out.push(T_LIT_DEF);
                    self.u32(*id);
                    self.out.push(*is_str as u8);
                    self.bytes(name);
                }
            }
            Obj::Kw(id) => {
                let k = *id as usize;
                if self.kw_sent.len() <= k {
                    self.kw_sent.resize(k + 1, false);
                }
                if self.kw_sent[k] {
                    self.out.push(T_KW);
                    self.u32(*id);
                } else {
                    self.kw_sent[k] = true;
                    self.out.push(T_KW_DEF);
                    self.u32(*id);
                    self.bytes(&t.kws[k]);
                }
            }
            Obj::Array(items) => {
                self.out.push(T_ARRAY);
                self.u32(items.len() as u32);
                for x in items {
                    self.value(t, x);
                }
            }
            Obj::Dict(d) => {
                self.out.push(T_DICT);
                self.dict(t, d);
            }
            Obj::Stream(s) => {
                self.out.push(T_STREAM);
                self.dict(t, &s.0);
                self.bytes(&s.1);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// wasm exports (single-threaded; state lives in thread-locals)

struct Global {
    interner: Interner,
    input: Vec<u8>,
    tape: Tape,
}

thread_local! {
    static G: RefCell<Global> = RefCell::new(Global {
        interner: Interner::new(),
        input: Vec::new(),
        tape: Tape { out: Vec::new(), lit_sent: Vec::new(), kw_sent: Vec::new() },
    });
}

/// Job layout written by JS: u32 nstreams, then per stream u32 len + bytes.
fn split_job(input: &[u8]) -> Vec<&[u8]> {
    let rd = |o: usize| u32::from_le_bytes(input[o..o + 4].try_into().unwrap()) as usize;
    let n = rd(0);
    let mut off = 4;
    let mut streams = Vec::with_capacity(n);
    for _ in 0..n {
        let len = rd(off);
        off += 4;
        streams.push(&input[off..off + len]);
        off += len;
    }
    streams
}

/// Returns a pointer to an input buffer of at least `len` bytes.
#[no_mangle]
pub extern "C" fn input_buffer(len: usize) -> *mut u8 {
    G.with(|g| {
        let g = &mut *g.borrow_mut();
        g.input.resize(len, 0);
        g.input.as_mut_ptr()
    })
}

#[no_mangle]
pub extern "C" fn tape_ptr() -> *const u8 {
    G.with(|g| g.borrow().tape.out.as_ptr())
}

#[no_mangle]
pub extern "C" fn tape_len() -> usize {
    G.with(|g| g.borrow().tape.out.len())
}

/// Parse the job in the input buffer, encoding every (pos, obj) onto the tape.
/// Returns the outcome code (0 = PSEOF, else the Python exception, see PErr).
#[no_mangle]
pub extern "C" fn parse_tape() -> i32 {
    G.with(|g| {
        let g = &mut *g.borrow_mut();
        let Global { interner, input, tape } = g;
        tape.out.clear();
        let streams = split_job(input);
        let mut parser = match Parser::new(interner, streams) {
            Ok(p) => p,
            Err(e) => return e.code(),
        };
        loop {
            match parser.nextobject() {
                Ok((pos, obj)) => {
                    tape.u32(pos as u32);
                    tape.value(parser.t, &obj);
                }
                Err(e) => return e.code(),
            }
        }
    })
}

/// Parse the job without sending objects to JS; returns the object count
/// (outcome in the high bits: count | code << 28).
#[no_mangle]
pub extern "C" fn parse_count() -> u32 {
    G.with(|g| {
        let g = &mut *g.borrow_mut();
        let Global { interner, input, .. } = g;
        let streams = split_job(input);
        let mut parser = match Parser::new(interner, streams) {
            Ok(p) => p,
            Err(e) => return (e.code() as u32) << 28,
        };
        let mut count = 0u32;
        loop {
            match parser.nextobject() {
                Ok(_) => count += 1,
                Err(e) => return count | (e.code() as u32) << 28,
            }
        }
    })
}
