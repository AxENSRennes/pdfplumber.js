//! Python `str` values on the way to JavaScript.
//!
//! A Python `str` is a sequence of code points, any of them possibly a lone
//! surrogate (U+D800..U+DFFF, `chr(0xd800)`), which a Rust `String` cannot
//! hold. A JavaScript string is a sequence of UTF-16 code units, which can
//! hold a lone surrogate. The transport therefore accepts a `str` in any of
//! the forms a port may keep it in ([`PyStrRef`]) and writes it as UTF-16
//! code units, one unit per BMP code point (a lone surrogate included) and a
//! surrogate pair per supplementary code point.
//!
//! The one loss is JavaScript's: a Python `str` holding a high surrogate
//! followed by a low surrogate, as two code points, arrives as the same
//! two units as the supplementary character they would encode
//! (`"😀"` and `"😀"` are one JS string). `canon` writes both the
//! same way too (`docs/verification.md`, "Canonical Trace Format").

/// A Python `str`, borrowed in one of the forms a port keeps it in.
#[derive(Clone, Copy, Debug)]
pub enum PyStrRef<'a> {
    /// No surrogate.
    Utf8(&'a str),
    /// Generalized UTF-8: UTF-8 in which a surrogate code point is encoded
    /// like any other BMP code point (three bytes `ED A0..BF 80..BF`), each
    /// on its own: what Python's `s.encode("utf-8", "surrogatepass")`
    /// writes, and WTF-8 when no surrogate pair is split.
    Wtf8(&'a [u8]),
    /// UTF-16 code units, read as code points: a valid pair is one
    /// supplementary code point, any other surrogate unit a lone surrogate.
    Utf16(&'a [u16]),
    /// The code points (`[ord(c) for c in s]`), each at most U+10FFFF.
    CodePoints(&'a [u32]),
}

/// Why a [`PyStrRef`] is not a `str`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidStr(pub String);

/// Whether `c` is a surrogate code point.
pub fn is_surrogate(c: u32) -> bool {
    (0xd800..0xe000).contains(&c)
}

impl PyStrRef<'_> {
    /// The text when the `str` holds no surrogate, else `None`.
    pub fn as_text(&self) -> Result<Option<String>, InvalidStr> {
        match *self {
            // (a shortcut: the general arm gives the same text)
            PyStrRef::Utf8(s) => Ok(Some(s.to_owned())),
            _ => {
                let cps = self.code_points()?;
                if cps.iter().any(|&c| is_surrogate(c)) {
                    return Ok(None);
                }
                Ok(Some(cps.into_iter().filter_map(char::from_u32).collect()))
            }
        }
    }

    /// The code points.
    pub fn code_points(&self) -> Result<Vec<u32>, InvalidStr> {
        match *self {
            PyStrRef::Utf8(s) => Ok(s.chars().map(u32::from).collect()),
            PyStrRef::Wtf8(b) => decode_generalized_utf8(b),
            PyStrRef::Utf16(u) => Ok(utf16_code_points(u)),
            PyStrRef::CodePoints(c) => match c.iter().find(|&&c| c > 0x10ffff) {
                Some(c) => Err(InvalidStr(format!("code point {c:#x} above U+10FFFF"))),
                None => Ok(c.to_vec()),
            },
        }
    }

    /// The UTF-16 code units JavaScript receives.
    pub fn utf16(&self) -> Result<Vec<u16>, InvalidStr> {
        match *self {
            // (shortcuts: through the code points, the units are the same)
            PyStrRef::Utf8(s) => Ok(s.encode_utf16().collect()),
            PyStrRef::Utf16(u) => Ok(u.to_vec()),
            _ => Ok(code_points_utf16(&self.code_points()?)),
        }
    }
}

/// UTF-16 code units of code points (each at most U+10FFFF).
pub fn code_points_utf16(cps: &[u32]) -> Vec<u16> {
    let mut out = Vec::with_capacity(cps.len());
    for &c in cps {
        if c >= 0x10000 {
            let v = c - 0x10000;
            // (`|` and `^` are the same here: the 10-bit halves do not
            // overlap the surrogate bases)
            out.push(0xd800 | (v >> 10) as u16);
            out.push(0xdc00 | (v & 0x3ff) as u16);
        } else {
            out.push(c as u16);
        }
    }
    out
}

/// The code points of UTF-16 code units: a high surrogate followed by a
/// low one is a supplementary code point, any other surrogate unit is a
/// lone surrogate.
pub fn utf16_code_points(units: &[u16]) -> Vec<u32> {
    let mut out = Vec::with_capacity(units.len());
    let mut i = 0;
    while i < units.len() {
        let u = u32::from(units[i]);
        if (0xd800..0xdc00).contains(&u) && i + 1 < units.len() && (0xdc00..0xe000).contains(&u32::from(units[i + 1])) {
            out.push(0x10000 + ((u - 0xd800) << 10) + (u32::from(units[i + 1]) - 0xdc00));
            i += 2;
        } else {
            out.push(u);
            i += 1;
        }
    }
    out
}

/// Decodes generalized UTF-8 ([`PyStrRef::Wtf8`]): strict UTF-8 (shortest
/// forms only, at most U+10FFFF) in which `ED A0..BF xx` is a surrogate.
pub fn decode_generalized_utf8(b: &[u8]) -> Result<Vec<u32>, InvalidStr> {
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    let bad = |i: usize| InvalidStr(format!("invalid generalized UTF-8 at byte {i}"));
    while i < b.len() {
        let b0 = b[i];
        let (len, min, init) = match b0 {
            0x00..=0x7f => (1, 0, u32::from(b0)),
            0xc2..=0xdf => (2, 0x80, u32::from(b0 & 0x1f)),
            0xe0..=0xef => (3, 0x800, u32::from(b0 & 0x0f)),
            0xf0..=0xf4 => (4, 0x10000, u32::from(b0 & 0x07)),
            _ => return Err(bad(i)),
        };
        if i + len > b.len() {
            return Err(bad(i));
        }
        let mut c = init;
        for k in 1..len {
            let bk = b[i + k];
            if bk & 0xc0 != 0x80 {
                return Err(bad(i + k));
            }
            // (`|` and `^` are the same: the shift leaves the low 6 bits 0)
            c = (c << 6) | u32::from(bk & 0x3f);
        }
        if c < min || c > 0x10ffff {
            return Err(bad(i));
        }
        out.push(c);
        i += len;
    }
    Ok(out)
}

/// Encodes code points as generalized UTF-8 (each surrogate on its own,
/// as Python's `surrogatepass`). (Each `|` below joins disjoint bits: `^`
/// would write the same bytes.)
pub fn encode_generalized_utf8(cps: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(cps.len());
    for &c in cps {
        match c {
            0..=0x7f => out.push(c as u8),
            0x80..=0x7ff => out.extend([0xc0 | (c >> 6) as u8, 0x80 | (c & 0x3f) as u8]),
            0x800..=0xffff => {
                out.extend([0xe0 | (c >> 12) as u8, 0x80 | ((c >> 6) & 0x3f) as u8, 0x80 | (c & 0x3f) as u8])
            }
            _ => out.extend([
                0xf0 | (c >> 18) as u8,
                0x80 | ((c >> 12) & 0x3f) as u8,
                0x80 | ((c >> 6) & 0x3f) as u8,
                0x80 | (c & 0x3f) as u8,
            ]),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generalized_utf8_round_trips_surrogates() {
        let cps = [0x41, 0xd800, 0xdc00, 0xd83d, 0xde00, 0x1f600, 0x7ff, 0x800, 0xffff, 0x10ffff, 0];
        let b = encode_generalized_utf8(&cps);
        assert_eq!(decode_generalized_utf8(&b).unwrap(), cps);
        // a lone high surrogate, as surrogatepass writes it
        assert_eq!(decode_generalized_utf8(&[0xed, 0xa0, 0x80]).unwrap(), [0xd800]);
        assert_eq!(PyStrRef::Wtf8(&[0xed, 0xa0, 0x80, 0x61]).utf16().unwrap(), [0xd800, 0x61]);
        assert_eq!(PyStrRef::Wtf8(b"ab").as_text().unwrap().as_deref(), Some("ab"));
        assert_eq!(PyStrRef::Wtf8(&[0xed, 0xa0, 0x80]).as_text().unwrap(), None);
    }

    #[test]
    fn generalized_utf8_rejects_invalid_input() {
        for bad in [
            &[0x80][..],
            &[0xc0, 0x80],             // overlong
            &[0xe0, 0x80, 0x80],       // overlong
            &[0xf0, 0x80, 0x80, 0x80], // overlong
            &[0xf4, 0x90, 0x80, 0x80], // above U+10FFFF
            &[0xf5, 0x80, 0x80, 0x80],
            &[0xe2, 0x82],       // truncated
            &[0xe2, 0x28, 0xa1], // bad continuation
            &[0xff],
        ] {
            assert!(decode_generalized_utf8(bad).is_err(), "{bad:x?}");
        }
        assert_eq!(decode_generalized_utf8(&[0xc2, 0x80]).unwrap(), [0x80]);
        let at = |b: &[u8]| decode_generalized_utf8(b).unwrap_err().0;
        assert_eq!(at(&[0x61, 0xe2, 0x28, 0xa1]), "invalid generalized UTF-8 at byte 2");
        assert_eq!(at(&[0x61, 0xe2, 0x82]), "invalid generalized UTF-8 at byte 1");
        assert_eq!(at(&[0x61, 0xc0, 0x80]), "invalid generalized UTF-8 at byte 1");
        assert_eq!(decode_generalized_utf8(&[0xf4, 0x8f, 0xbf, 0xbf]).unwrap(), [0x10ffff]);
    }

    #[test]
    fn utf16_pairs_and_lone_surrogates() {
        assert_eq!(
            utf16_code_points(&[0xd83d, 0xde00, 0xd800, 0x61, 0xdc00, 0xd800]),
            [0x1f600, 0xd800, 0x61, 0xdc00, 0xd800]
        );
        assert_eq!(
            code_points_utf16(&[0x1f600, 0xd800, 0x10ffff, 0x10000]),
            [0xd83d, 0xde00, 0xd800, 0xdbff, 0xdfff, 0xd800, 0xdc00]
        );
        assert!(PyStrRef::CodePoints(&[0x110000]).utf16().is_err());
        assert_eq!(PyStrRef::CodePoints(&[0x10ffff]).utf16().unwrap(), [0xdbff, 0xdfff]);
        assert_eq!(PyStrRef::CodePoints(&[0xdc00]).utf16().unwrap(), [0xdc00]);
        assert_eq!(PyStrRef::Utf8("é😀").utf16().unwrap(), [0xe9, 0xd83d, 0xde00]);
        assert_eq!(PyStrRef::Utf16(&[0xd800]).code_points().unwrap(), [0xd800]);
        assert!(is_surrogate(0xd800) && is_surrogate(0xdfff) && !is_surrogate(0xe000) && !is_surrogate(0xd7ff));
    }
}
