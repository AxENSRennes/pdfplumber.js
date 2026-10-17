//! The JSON result encoding (measured against the binary one in
//! `docs/js-runtime.md`; kept as a readable debugging form).
//!
//! Plain values are plain JSON, so `JSON.parse` alone decodes a result
//! without the values JSON lacks. Numbers are written in their shortest
//! round-trip form, which `JSON.parse` reads back exactly (`-0` included).
//! A value JSON lacks is an object whose only key is `"\u0000"`:
//!
//! | Value | JSON |
//! |---|---|
//! | NaN, `inf`, `-inf` | `{"\u0000":["nan"]}`, `["inf"]`, `["-inf"]` |
//! | `int` beyond ±2^53 | `{"\u0000":["int","<decimal>"]}` |
//! | `bytes` | `{"\u0000":["bytes","<hex>"]}` |
//! | `PSLiteral` | `{"\u0000":["lit",<name>]}` |
//! | `PSKeyword` | `{"\u0000":["kw","<hex>"]}` |
//! | `PDFObjRef` | `{"\u0000":["ref",<objid>]}` |
//! | `PDFStream` | `{"\u0000":["stream",<objid>,<genno>,<attrs>,<rawdata>,<data>]}` |
//!
//! and a real dict key that starts with U+0000 gets one more U+0000 in
//! front. [`JsonWriter::needs_fixup`] tells JavaScript whether to walk the
//! parsed value to undo these escapes. Lone surrogates are written as
//! `\udXXX` escapes, which `JSON.parse` turns back into them.

use std::io::Write as _;

use pycompat::{exc, PyInt, PyResult};

use super::{safe_int, Writer};
use crate::strings::PyStrRef;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Array,
    Object,
    /// `{"\u0000":[tag, ...]}`
    Special,
}

struct Frame {
    kind: Kind,
    remaining: usize,
    first: bool,
    expect_key: bool,
}

/// Writes one value as JSON text.
pub struct JsonWriter {
    out: Vec<u8>,
    frames: Vec<Frame>,
    fixup: bool,
}

impl Default for JsonWriter {
    fn default() -> Self {
        JsonWriter::new()
    }
}

const HEX: &[u8; 16] = b"0123456789abcdef";

impl JsonWriter {
    pub fn new() -> JsonWriter {
        JsonWriter { out: Vec::with_capacity(4096), frames: Vec::new(), fixup: false }
    }

    /// Whether the text holds escapes that JavaScript must undo.
    pub fn needs_fixup(&self) -> bool {
        self.fixup
    }

    pub fn finish(self) -> Vec<u8> {
        debug_assert!(self.frames.is_empty(), "unfinished JSON value");
        self.out
    }

    /// Before a value: the separator, and whether the value is a key.
    fn begin(&mut self) -> bool {
        let Some(f) = self.frames.last_mut() else { return false };
        if f.kind == Kind::Object && !f.expect_key {
            return false;
        }
        if !f.first {
            self.out.push(b',');
        }
        f.first = false;
        f.kind == Kind::Object
    }

    /// After a complete value: counts it, closing the full containers.
    fn end(&mut self) {
        while let Some(f) = self.frames.last_mut() {
            if f.kind == Kind::Object {
                if f.expect_key {
                    // a key was written: its value comes next
                    f.expect_key = false;
                    return;
                }
                f.expect_key = true;
            }
            f.remaining -= 1;
            if f.remaining > 0 {
                return;
            }
            let kind = f.kind;
            self.frames.pop();
            self.out.extend_from_slice(match kind {
                Kind::Array => b"]",
                Kind::Object => b"}",
                Kind::Special => b"]}",
            });
        }
    }

    fn open(&mut self, kind: Kind, n: usize) {
        if n == 0 {
            self.out.extend_from_slice(if kind == Kind::Array { b"[]" } else { b"{}" });
            self.end();
            return;
        }
        self.out.push(if kind == Kind::Array { b'[' } else { b'{' });
        self.frames.push(Frame { kind, remaining: n, first: true, expect_key: kind == Kind::Object });
    }

    /// A value with no children: `{"\u0000":[tag, payload]}`.
    fn special(&mut self, tag: &str, payload: Option<&str>) {
        self.begin_value();
        self.fixup = true;
        self.out.extend_from_slice(b"{\"\\u0000\":[\"");
        self.out.extend_from_slice(tag.as_bytes());
        self.out.push(b'"');
        if let Some(p) = payload {
            self.out.extend_from_slice(b",\"");
            self.out.extend_from_slice(p.as_bytes());
            self.out.push(b'"');
        }
        self.out.extend_from_slice(b"]}");
        self.end();
    }

    /// A special value followed by `n` child values.
    fn special_open(&mut self, tag: &str, n: usize) {
        self.begin_value();
        self.fixup = true;
        self.out.extend_from_slice(b"{\"\\u0000\":[\"");
        self.out.extend_from_slice(tag.as_bytes());
        self.out.push(b'"');
        self.frames.push(Frame { kind: Kind::Special, remaining: n, first: false, expect_key: false });
    }

    /// `begin` for a value that cannot be a key.
    fn begin_value(&mut self) {
        assert!(!self.begin(), "a JSON object key must be a str");
    }

    fn hex(b: &[u8]) -> String {
        let mut s = String::with_capacity(b.len() * 2);
        for &x in b {
            s.push(HEX[usize::from(x >> 4)] as char);
            s.push(HEX[usize::from(x & 15)] as char);
        }
        s
    }

    fn escape_unit(&mut self, u: u16) {
        self.out.extend_from_slice(b"\\u");
        for shift in [12, 8, 4, 0] {
            self.out.push(HEX[usize::from((u >> shift) & 15)]);
        }
    }

    /// A JSON string of text (no surrogates).
    fn string(&mut self, s: &str, key: bool) {
        if key && s.starts_with('\0') {
            self.fixup = true;
            self.out.extend_from_slice(b"\"\\u0000");
        } else {
            self.out.push(b'"');
        }
        let bytes = s.as_bytes();
        let mut start = 0;
        for (i, &b) in bytes.iter().enumerate() {
            if b >= 0x20 && b != b'"' && b != b'\\' {
                continue;
            }
            self.out.extend_from_slice(&bytes[start..i]);
            match b {
                b'"' => self.out.extend_from_slice(b"\\\""),
                b'\\' => self.out.extend_from_slice(b"\\\\"),
                b'\n' => self.out.extend_from_slice(b"\\n"),
                _ => self.escape_unit(u16::from(b)),
            }
            start = i + 1;
        }
        self.out.extend_from_slice(&bytes[start..]);
        self.out.push(b'"');
    }

    fn key_or_value_str(&mut self, s: &str) {
        let key = self.begin();
        self.string(s, key);
        if key {
            self.out.push(b':');
        }
        self.end();
    }

    fn number(&mut self, x: f64) {
        let a = x.abs();
        if a == 0.0 || (1e-6..1e21).contains(&a) {
            write!(self.out, "{x}").expect("write to a Vec");
        } else {
            write!(self.out, "{x:e}").expect("write to a Vec");
        }
    }
}

impl Writer for JsonWriter {
    fn none(&mut self) {
        self.begin_value();
        self.out.extend_from_slice(b"null");
        self.end();
    }

    fn bool(&mut self, b: bool) {
        self.begin_value();
        self.out.extend_from_slice(if b { b"true" } else { b"false" });
        self.end();
    }

    fn int(&mut self, i: &PyInt) {
        match safe_int(i) {
            Some(v) => {
                self.begin_value();
                write!(self.out, "{v}").expect("write to a Vec");
                self.end();
            }
            None => self.special("int", Some(&i.to_string())),
        }
    }

    fn float(&mut self, x: f64) {
        if x.is_nan() {
            self.special("nan", None);
        } else if x.is_infinite() {
            // (x is ±inf: `>` and `>=` are the same here)
            self.special(if x > 0.0 { "inf" } else { "-inf" }, None);
        } else {
            self.begin_value();
            self.number(x);
            self.end();
        }
    }

    fn str(&mut self, s: &str) {
        self.key_or_value_str(s);
    }

    fn py_str(&mut self, s: PyStrRef<'_>) -> PyResult<()> {
        let units = s.utf16().map_err(|e| exc::ValueError(e.0))?;
        if let Ok(text) = String::from_utf16(&units) {
            self.key_or_value_str(&text);
            return Ok(());
        }
        let key = self.begin();
        if key && units.first() == Some(&0) {
            self.fixup = true;
            self.out.extend_from_slice(b"\"\\u0000");
        } else {
            self.out.push(b'"');
        }
        for c in char::decode_utf16(units.iter().copied()) {
            match c {
                Ok(c) if c >= ' ' && c != '"' && c != '\\' => {
                    let mut buf = [0; 4];
                    self.out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                }
                Ok(c) => self.escape_unit(c as u16),
                Err(e) => self.escape_unit(e.unpaired_surrogate()),
            }
        }
        self.out.push(b'"');
        if key {
            self.out.push(b':');
        }
        self.end();
        Ok(())
    }

    fn bytes(&mut self, b: &[u8]) {
        self.special("bytes", Some(&Self::hex(b)));
    }

    fn list(&mut self, n: usize) {
        self.begin_value();
        self.open(Kind::Array, n);
    }

    fn tuple(&mut self, n: usize) {
        self.list(n);
    }

    fn dict(&mut self, n: usize) {
        self.begin_value();
        self.open(Kind::Object, n);
    }

    fn literal(&mut self) {
        self.special_open("lit", 1);
    }

    fn keyword(&mut self, name: &[u8]) {
        self.special("kw", Some(&Self::hex(name)));
    }

    fn objref(&mut self) {
        self.special_open("ref", 1);
    }

    fn stream(&mut self) {
        self.special_open("stream", 5);
    }
}

/// Encodes a value as JSON; the flag says whether it needs the fixup walk.
pub fn encode(v: &pdfplumber::obj::PyValue) -> PyResult<(Vec<u8>, bool)> {
    let mut w = JsonWriter::new();
    super::write_value(&mut w, v)?;
    let fixup = w.needs_fixup();
    Ok((w.finish(), fixup))
}
