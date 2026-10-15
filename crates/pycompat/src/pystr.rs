//! Python's `str`: a sequence of code points, lone surrogates included.
//!
//! A CPython `str` holds any code point from U+0000 to U+10FFFF, the
//! surrogates U+D800..U+DFFF included (`chr(0xd800)`), and pdfminer makes
//! such strings: `IdentityUnicodeMap.get_unichr` is `chr(cid)` and a CID can
//! be any number, a `ToUnicode` CMap can map a code to `chr(n)`. A Rust
//! `String` cannot hold a surrogate, a JavaScript string can (one UTF-16
//! unit), so the port keeps such text exactly with [`PyStr`].
//!
//! # Representation
//!
//! [`PyStr`] holds the bytes of `s.encode("utf-8", "surrogatepass")`: every
//! code point encoded on its own by the UTF-8 algorithm, a surrogate as three
//! bytes (`ED A0 80`..`ED BF BF`). Two surrogates that would form a pair stay
//! two code points (`"\ud83d" + "\ude00"` has length 2 in Python and is not
//! `"\U0001f600"`), unlike WTF-8, which would join them. A `str` without
//! surrogates is therefore stored as its UTF-8 bytes, and:
//!
//! - `==`, `<` and `hash` compare the bytes: UTF-8 order is code point
//!   order, surrogates included (`ED 9F BF` < `ED A0 80` < `EE 80 80`), so
//!   this is Python's `str` comparison;
//! - concatenation is byte concatenation, and a substring test (`in`) is a
//!   byte substring test (every code point has one encoding, and a lead
//!   byte never matches inside another sequence);
//! - [`PyStr::as_str`] gives the `&str` of a string without surrogates at the
//!   cost of a UTF-8 validation (Rust rejects exactly the surrogate
//!   sequences), and every constructor from Rust text is a copy.
//!
//! Cost, measured on the text and table operations of the real pages
//! (`TEXT_TABLES_TIME=1 cargo test --release --test text_tables_unit
//! pages_real`, five alternating rounds against the former `Rc<str>`
//! values on a loaded shared machine): new/old time ratios 0.87 to 1.09,
//! 0.96 on average, i.e. no difference beyond the noise. The layout is that
//! of `Rc<str>` (a fat pointer), and every text without surrogates is the
//! same bytes. A code-point vector (`Rc<[u32]>`) was not kept: it
//! quadruples the memory of every text and needs a conversion at every
//! boundary with Rust text.
//!
//! # JavaScript
//!
//! [`PyStr::to_utf16`] writes each code point as UTF-16 (a surrogate as its
//! single unit) and [`PyStr::from_utf16`] reads a JS string back, joining
//! valid pairs. Every Python `str` is thus exact in JavaScript; the only
//! strings that do not come back identical are those holding a high
//! surrogate followed by a low one, which JavaScript cannot tell from the
//! character they encode (`json.dumps` writes both the same way too).
//!
//! # Character classes and normalization
//!
//! [`str_isspace`](crate::text::str_isspace) and the other classes of
//! [`crate::text`] take a code point; a surrogate is in no class (not
//! space, not printable, not alphanumeric, no decimal value), as in
//! CPython. [`PyStr::normalize`] is `unicodedata.normalize` (see
//! [`crate::unicodedata`]).

use std::fmt;
use std::rc::Rc;

use crate::text::str_isspace_cp;

/// A Python `str` (see the module documentation).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PyStr(Rc<[u8]>);

/// The highest code point (`sys.maxunicode`).
pub const MAX_CODE_POINT: u32 = 0x10ffff;

/// Whether `c` is a surrogate code point (U+D800..U+DFFF).
#[inline]
pub fn is_surrogate(c: u32) -> bool {
    (0xd800..0xe000).contains(&c)
}

/// Appends the bytes of code point `c` (at most U+10FFFF; a surrogate as
/// three bytes).
#[inline]
pub fn encode_code_point(out: &mut Vec<u8>, c: u32) {
    debug_assert!(c <= MAX_CODE_POINT, "not a code point: {c:#x}");
    if c < 0x80 {
        out.push(c as u8);
    } else if c < 0x800 {
        out.extend_from_slice(&[0xc0 | (c >> 6) as u8, 0x80 | (c & 0x3f) as u8]);
    } else if c < 0x10000 {
        out.extend_from_slice(&[0xe0 | (c >> 12) as u8, 0x80 | ((c >> 6) & 0x3f) as u8, 0x80 | (c & 0x3f) as u8]);
    } else {
        out.extend_from_slice(&[
            0xf0 | (c >> 18) as u8,
            0x80 | ((c >> 12) & 0x3f) as u8,
            0x80 | ((c >> 6) & 0x3f) as u8,
            0x80 | (c & 0x3f) as u8,
        ]);
    }
}

/// The code point at `bytes[i]` and its length in bytes (`bytes` holds
/// whole code points).
#[inline]
fn decode_at(bytes: &[u8], i: usize) -> (u32, usize) {
    let b0 = bytes[i];
    if b0 < 0x80 {
        return (u32::from(b0), 1);
    }
    let cont = |k: usize| u32::from(bytes[i + k] & 0x3f);
    if b0 < 0xe0 {
        (u32::from(b0 & 0x1f) << 6 | cont(1), 2)
    } else if b0 < 0xf0 {
        (u32::from(b0 & 0x0f) << 12 | cont(1) << 6 | cont(2), 3)
    } else {
        (u32::from(b0 & 0x07) << 18 | cont(1) << 12 | cont(2) << 6 | cont(3), 4)
    }
}

/// The code points of a [`PyStr`] (`iter(s)`, as `ord` values).
#[derive(Clone)]
pub struct CodePoints<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Iterator for CodePoints<'_> {
    type Item = u32;

    #[inline]
    fn next(&mut self) -> Option<u32> {
        if self.pos >= self.bytes.len() {
            return None;
        }
        let (c, n) = decode_at(self.bytes, self.pos);
        self.pos += n;
        Some(c)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let rest = self.bytes.len() - self.pos;
        (rest.div_ceil(4), Some(rest))
    }
}

thread_local! {
    static EMPTY: PyStr = PyStr(Rc::from(&b""[..]));
}

impl PyStr {
    /// `""`
    pub fn empty() -> PyStr {
        EMPTY.with(PyStr::clone)
    }

    /// The `str` of Rust text.
    pub fn new(s: &str) -> PyStr {
        if s.is_empty() {
            return PyStr::empty();
        }
        PyStr(Rc::from(s.as_bytes()))
    }

    /// From bytes that hold whole encoded code points (`surrogatepass`
    /// UTF-8, each code point in its shortest form, at most U+10FFFF), as
    /// [`PyStrBuilder`] and [`PyStr::as_bytes`] give them; `None` otherwise.
    pub fn from_surrogatepass_utf8(bytes: &[u8]) -> Option<PyStr> {
        let mut i = 0;
        while i < bytes.len() {
            let b0 = bytes[i];
            let (n, min) = match b0 {
                0x00..=0x7f => (1, 0),
                0xc2..=0xdf => (2, 0x80),
                0xe0..=0xef => (3, 0x800),
                0xf0..=0xf4 => (4, 0x10000),
                _ => return None,
            };
            if i + n > bytes.len() || !bytes[i + 1..i + n].iter().all(|&b| b & 0xc0 == 0x80) {
                return None;
            }
            let (c, _) = decode_at(bytes, i);
            if c < min || c > MAX_CODE_POINT {
                return None;
            }
            i += n;
        }
        Some(PyStr(Rc::from(bytes)))
    }

    /// The `str` of these code points (`"".join(map(chr, code_points))`):
    /// `None` if one is above U+10FFFF (`chr` raises `ValueError`).
    pub fn from_code_points(code_points: impl IntoIterator<Item = u32>) -> Option<PyStr> {
        let mut b = PyStrBuilder::new();
        for c in code_points {
            if c > MAX_CODE_POINT {
                return None;
            }
            b.push(c);
        }
        Some(b.finish())
    }

    /// `chr(c)` for `c <= 0x10ffff`.
    pub fn from_code_point(c: u32) -> PyStr {
        assert!(c <= MAX_CODE_POINT, "chr() arg not in range(0x110000)");
        let mut out = Vec::with_capacity(4);
        encode_code_point(&mut out, c);
        PyStr(Rc::from(out))
    }

    /// A JavaScript string (UTF-16 units) as a Python `str`: a valid
    /// surrogate pair is one code point, any other surrogate a lone one.
    pub fn from_utf16(units: &[u16]) -> PyStr {
        let mut b = PyStrBuilder::with_capacity(units.len());
        for r in char::decode_utf16(units.iter().copied()) {
            match r {
                Ok(ch) => b.push_char(ch),
                Err(e) => b.push(u32::from(e.unpaired_surrogate())),
            }
        }
        b.finish()
    }

    /// The UTF-16 units of the `str` for JavaScript: each code point above
    /// U+FFFF as a pair, every other one (a surrogate included) as one unit.
    pub fn to_utf16(&self) -> Vec<u16> {
        let mut out = Vec::with_capacity(self.0.len());
        for c in self.code_points() {
            if c >= 0x10000 {
                let c = c - 0x10000;
                out.push(0xd800 | (c >> 10) as u16);
                out.push(0xdc00 | (c & 0x3ff) as u16);
            } else {
                out.push(c as u16);
            }
        }
        out
    }

    /// The bytes (`s.encode("utf-8", "surrogatepass")`).
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// The text, when the `str` holds no surrogate.
    #[inline]
    pub fn as_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.0).ok()
    }

    /// Whether the `str` holds a surrogate.
    pub fn has_surrogate(&self) -> bool {
        self.as_str().is_none()
    }

    /// The text with each surrogate replaced by U+FFFD (for messages of the
    /// port itself, never for a Python value).
    pub fn to_string_lossy(&self) -> String {
        match self.as_str() {
            Some(s) => s.to_owned(),
            None => self.code_points().map(|c| char::from_u32(c).unwrap_or('\u{fffd}')).collect(),
        }
    }

    /// The code points.
    #[inline]
    pub fn code_points(&self) -> CodePoints<'_> {
        CodePoints { bytes: &self.0, pos: 0 }
    }

    /// `len(s)`: the number of code points.
    pub fn len(&self) -> usize {
        self.0.iter().filter(|&&b| b & 0xc0 != 0x80).count()
    }

    /// `not s`
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// `s[start:end]` for `0 <= start`, `0 <= end` (code point indices,
    /// clamped to the length like a slice).
    pub fn slice(&self, start: usize, end: usize) -> PyStr {
        if start >= end {
            return PyStr::empty();
        }
        let mut offsets = self.0.iter().enumerate().filter(|(_, &b)| b & 0xc0 != 0x80).map(|(i, _)| i);
        let Some(a) = offsets.nth(start) else { return PyStr::empty() };
        let b = offsets.nth(end - start - 1).unwrap_or(self.0.len());
        PyStr(Rc::from(&self.0[a..b]))
    }

    /// `a + b`
    pub fn concat(&self, other: &PyStr) -> PyStr {
        if other.is_empty() {
            return self.clone();
        }
        if self.is_empty() {
            return other.clone();
        }
        let mut out = Vec::with_capacity(self.0.len() + other.0.len());
        out.extend_from_slice(&self.0);
        out.extend_from_slice(&other.0);
        PyStr(Rc::from(out))
    }

    /// `s * n` (`n <= 0`: the empty string).
    pub fn repeat(&self, n: usize) -> PyStr {
        if n == 0 || self.is_empty() {
            return PyStr::empty();
        }
        PyStr(Rc::from(self.0.repeat(n)))
    }

    /// `sep.join(items)`
    pub fn join<'a>(&self, items: impl IntoIterator<Item = &'a PyStr>) -> PyStr {
        let mut b = PyStrBuilder::new();
        for (i, s) in items.into_iter().enumerate() {
            if i > 0 {
                b.push_str(self);
            }
            b.push_str(s);
        }
        b.finish()
    }

    /// The rest of the string after `prefix`, if it starts with it
    /// (`s[len(prefix):]` when `s[:len(prefix)] == prefix`).
    pub fn strip_prefix(&self, prefix: &str) -> Option<PyStr> {
        self.0.strip_prefix(prefix.as_bytes()).map(|rest| PyStr(Rc::from(rest)))
    }

    /// `sub in s` (a substring test).
    pub fn contains(&self, sub: &PyStr) -> bool {
        let (h, n) = (&self.0[..], &sub.0[..]);
        n.is_empty() || h.windows(n.len()).any(|w| w == n)
    }

    /// `s.isspace()`: not empty and only whitespace (`Py_UNICODE_ISSPACE`).
    pub fn isspace(&self) -> bool {
        !self.is_empty() && self.code_points().all(str_isspace_cp)
    }

    /// `s.strip()` (whitespace by `Py_UNICODE_ISSPACE`).
    pub fn strip(&self) -> PyStr {
        let cps: Vec<u32> = self.code_points().collect();
        let start = cps.iter().position(|&c| !str_isspace_cp(c)).unwrap_or(cps.len());
        let end = cps.iter().rposition(|&c| !str_isspace_cp(c)).map_or(start, |i| i + 1);
        PyStr::from_code_points(cps[start..end].iter().copied()).expect("code points of a str")
    }

    /// `s.split()`: the runs of non-whitespace.
    pub fn split_whitespace(&self) -> Vec<PyStr> {
        let mut out = Vec::new();
        let mut cur = PyStrBuilder::new();
        for c in self.code_points() {
            if str_isspace_cp(c) {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur).finish());
                }
            } else {
                cur.push(c);
            }
        }
        if !cur.is_empty() {
            out.push(cur.finish());
        }
        out
    }

    /// `s.split(sep)` for a non-empty `sep`.
    pub fn split(&self, sep: &PyStr) -> Vec<PyStr> {
        assert!(!sep.is_empty(), "empty separator");
        let (h, n) = (&self.0[..], &sep.0[..]);
        let mut out = Vec::new();
        let mut start = 0;
        let mut i = 0;
        while i + n.len() <= h.len() {
            if &h[i..i + n.len()] == n {
                out.push(PyStr(Rc::from(&h[start..i])));
                i += n.len();
                start = i;
            } else {
                i += 1;
            }
        }
        out.push(PyStr(Rc::from(&h[start..])));
        out
    }

    /// `repr(s)` (`unicode_repr`): always printable ASCII or printable
    /// text, so a Rust `String` (a surrogate is written `\udxxx`).
    pub fn repr(&self) -> String {
        match self.as_str() {
            Some(s) => crate::repr::str_repr(s),
            None => crate::repr::str_repr_code_points(&self.code_points().collect::<Vec<_>>()),
        }
    }

    /// `json.dumps(s)` (`ensure_ascii=True`): ASCII.
    pub fn json(&self) -> String {
        let mut out = String::with_capacity(self.0.len() + 2);
        match self.as_str() {
            Some(s) => crate::repr::write_json_str(&mut out, s),
            None => crate::repr::write_json_code_points(&mut out, &self.code_points().collect::<Vec<_>>()),
        }
        out
    }

    /// `unicodedata.normalize(form, s)`.
    pub fn normalize(&self, form: crate::unicodedata::NormalizationForm) -> PyStr {
        crate::unicodedata::normalize(form, self)
    }
}

impl Default for PyStr {
    fn default() -> PyStr {
        PyStr::empty()
    }
}

/// `repr(s)`
impl fmt::Debug for PyStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.repr())
    }
}

impl From<&str> for PyStr {
    fn from(s: &str) -> PyStr {
        PyStr::new(s)
    }
}

impl From<String> for PyStr {
    fn from(s: String) -> PyStr {
        PyStr::new(&s)
    }
}

impl From<&String> for PyStr {
    fn from(s: &String) -> PyStr {
        PyStr::new(s)
    }
}

impl From<char> for PyStr {
    fn from(c: char) -> PyStr {
        PyStr::new(c.encode_utf8(&mut [0; 4]))
    }
}

impl From<&PyStr> for PyStr {
    fn from(s: &PyStr) -> PyStr {
        s.clone()
    }
}

impl PartialEq<str> for PyStr {
    fn eq(&self, other: &str) -> bool {
        &self.0[..] == other.as_bytes()
    }
}

impl PartialEq<&str> for PyStr {
    fn eq(&self, other: &&str) -> bool {
        &self.0[..] == other.as_bytes()
    }
}

impl PartialEq<PyStr> for str {
    fn eq(&self, other: &PyStr) -> bool {
        self.as_bytes() == &other.0[..]
    }
}

impl PartialEq<PyStr> for &str {
    fn eq(&self, other: &PyStr) -> bool {
        self.as_bytes() == &other.0[..]
    }
}

/// Builds a [`PyStr`] code point by code point (`"".join(...)`).
#[derive(Clone, Default)]
pub struct PyStrBuilder(Vec<u8>);

impl PyStrBuilder {
    pub fn new() -> PyStrBuilder {
        PyStrBuilder(Vec::new())
    }

    /// Room for about `n` code points.
    pub fn with_capacity(n: usize) -> PyStrBuilder {
        PyStrBuilder(Vec::with_capacity(n))
    }

    /// Appends code point `c` (at most U+10FFFF).
    #[inline]
    pub fn push(&mut self, c: u32) {
        encode_code_point(&mut self.0, c);
    }

    #[inline]
    pub fn push_char(&mut self, ch: char) {
        let mut buf = [0; 4];
        self.0.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
    }

    #[inline]
    pub fn push_str(&mut self, s: &PyStr) {
        self.0.extend_from_slice(&s.0);
    }

    #[inline]
    pub fn push_rust_str(&mut self, s: &str) {
        self.0.extend_from_slice(s.as_bytes());
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn finish(self) -> PyStr {
        if self.0.is_empty() {
            return PyStr::empty();
        }
        PyStr(Rc::from(self.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surrogates_stay_code_points() {
        let hi = PyStr::from_code_point(0xd83d);
        let lo = PyStr::from_code_point(0xde00);
        let pair = hi.concat(&lo);
        assert_eq!(pair.len(), 2);
        assert_ne!(pair, PyStr::from('\u{1f600}'));
        assert_eq!(pair.as_bytes(), b"\xed\xa0\xbd\xed\xb8\x80");
        assert!(pair.as_str().is_none());
        assert_eq!(pair.code_points().collect::<Vec<_>>(), [0xd83d, 0xde00]);
        assert_eq!(pair.to_utf16(), [0xd83d, 0xde00]);
        assert_eq!(PyStr::from_utf16(&[0xd83d, 0xde00]), PyStr::from('\u{1f600}'));
        assert_eq!(
            PyStr::from_utf16(&[0x61, 0xdc00, 0xd800]).code_points().collect::<Vec<_>>(),
            [0x61, 0xdc00, 0xd800]
        );
        let (below, above) = (PyStr::from_code_point(0xd7ff), PyStr::from_code_point(0xe000));
        assert!(below < PyStr::from_code_point(0xd800));
        assert!(PyStr::from_code_point(0xdfff) < above);
        assert_eq!(pair.repr(), "'\\ud83d\\ude00'");
        assert_eq!(pair.json(), "\"\\ud83d\\ude00\"");
        assert_eq!(PyStr::from_surrogatepass_utf8(pair.as_bytes()), Some(pair.clone()));
        assert_eq!(PyStr::from_surrogatepass_utf8(b"\xc0\x80"), None);
        assert_eq!(PyStr::from("a\u{e9}\u{1f600}b").slice(1, 3), PyStr::from("\u{e9}\u{1f600}"));
    }
}
