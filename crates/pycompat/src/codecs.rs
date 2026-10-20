//! `bytes.decode("utf-8")` and `bytes.decode("utf-16")` with the `strict`
//! error handler: CPython 3.12's decoders (`Objects/unicodeobject.c`,
//! `unicode_decode_utf8` and `PyUnicode_DecodeUTF16Stateful`), down to the
//! `UnicodeDecodeError` they raise (its encoding, the range of bytes it
//! reports and its reason make up `str(e)`).
//!
//! pdfplumber decodes the `/URI`, `/T` and `/Contents` of annotations with
//! them (pdfplumber/page.py:273 `Page.annots`). Both names are fast paths
//! of `PyUnicode_Decode` (no codec lookup, no Python frame).

use crate::err::{ExcKind, PyErr, PyResult};
use crate::pystr::{PyStr, PyStrBuilder};

/// `UnicodeDecodeError(encoding, object, start, end, reason)`, as `str()`
/// writes it (`Objects/exceptions.c`, `UnicodeDecodeError_str`).
pub fn unicode_decode_error(encoding: &str, object: &[u8], start: usize, end: usize, reason: &str) -> PyErr {
    let message = if end == start + 1 {
        format!("'{encoding}' codec can't decode byte 0x{:02x} in position {start}: {reason}", object[start])
    } else {
        format!("'{encoding}' codec can't decode bytes in position {start}-{}: {reason}", end - 1)
    };
    PyErr::new(ExcKind::UnicodeDecodeError, message)
}

/// Whether `c` can start a UTF-8 sequence (`\xC2`-`\xF4`, or ASCII).
fn is_start_byte(c: u8) -> bool {
    c < 0x80 || (0xC2..=0xF4).contains(&c)
}

/// `b.decode("utf-8")`. CPython and Rust delimit an invalid sequence the
/// same way (the "maximal subpart" of Unicode §3.9, `stringlib/codecs.h`):
/// the first one is reported as `invalid start byte` (one byte that cannot
/// start a sequence), `invalid continuation byte` (the valid prefix of a
/// sequence) or, when the input ends inside a sequence, `unexpected end of
/// data` (up to the end).
pub fn decode_utf8(b: &[u8]) -> PyResult<PyStr> {
    match std::str::from_utf8(b) {
        Ok(s) => Ok(PyStr::new(s)),
        Err(e) => {
            let start = e.valid_up_to();
            Err(match e.error_len() {
                None => unicode_decode_error("utf-8", b, start, b.len(), "unexpected end of data"),
                Some(n) if !is_start_byte(b[start]) => {
                    debug_assert_eq!(n, 1);
                    unicode_decode_error("utf-8", b, start, start + 1, "invalid start byte")
                }
                Some(n) => unicode_decode_error("utf-8", b, start, start + n, "invalid continuation byte"),
            })
        }
    }
}

/// `b.decode("utf-16")`: a byte order mark (`\xff\xfe` little-endian,
/// `\xfe\xff` big-endian) is consumed, otherwise the native order
/// (little-endian on the oracle's machine and in Wasm); the encoding in the
/// error is `utf-16-le` or `utf-16-be` from the order chosen, positions
/// count from the start of `b` (BOM included). Reasons: `truncated data` (an
/// odd byte at the end), `illegal encoding` (a low surrogate first),
/// `unexpected end of data` (a high surrogate last), `illegal UTF-16
/// surrogate` (a high surrogate not followed by a low one, reported on the
/// high one).
pub fn decode_utf16(b: &[u8]) -> PyResult<PyStr> {
    let (big, mut q) = match b {
        [0xff, 0xfe, ..] => (false, 2),
        [0xfe, 0xff, ..] => (true, 2),
        _ => (false, 0),
    };
    let encoding = if big { "utf-16-be" } else { "utf-16-le" };
    let unit = |i: usize| -> u32 {
        let pair = [b[i], b[i + 1]];
        u32::from(if big { u16::from_be_bytes(pair) } else { u16::from_le_bytes(pair) })
    };
    let mut out = PyStrBuilder::with_capacity(b.len() / 2);
    while q < b.len() {
        if b.len() - q < 2 {
            return Err(unicode_decode_error(encoding, b, q, b.len(), "truncated data"));
        }
        let ch = unit(q);
        q += 2;
        if !(0xD800..=0xDFFF).contains(&ch) {
            out.push(ch);
            continue;
        }
        if ch >= 0xDC00 {
            return Err(unicode_decode_error(encoding, b, q - 2, q, "illegal encoding"));
        }
        if b.len() - q < 2 {
            return Err(unicode_decode_error(encoding, b, q - 2, b.len(), "unexpected end of data"));
        }
        let ch2 = unit(q);
        q += 2;
        if !(0xDC00..=0xDFFF).contains(&ch2) {
            return Err(unicode_decode_error(encoding, b, q - 4, q - 2, "illegal UTF-16 surrogate"));
        }
        out.push(0x10000 + (((ch & 0x3FF) << 10) | (ch2 & 0x3FF)));
    }
    Ok(out.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(r: PyResult<PyStr>) -> String {
        r.expect_err("an error").message
    }

    #[test]
    fn utf8_errors() {
        assert_eq!(decode_utf8(b"caf\xc3\xa9").unwrap().as_str(), Some("caf\u{e9}"));
        assert_eq!(msg(decode_utf8(b"a\xff")), "'utf-8' codec can't decode byte 0xff in position 1: invalid start byte");
        assert_eq!(
            msg(decode_utf8(b"\xe1\x80A")),
            "'utf-8' codec can't decode bytes in position 0-1: invalid continuation byte"
        );
        assert_eq!(msg(decode_utf8(b"\xe1\x80")), "'utf-8' codec can't decode bytes in position 0-1: unexpected end of data");
    }

    #[test]
    fn utf16_errors() {
        assert_eq!(decode_utf16(b"\xff\xfeA\x00").unwrap().as_str(), Some("A"));
        assert_eq!(decode_utf16(b"\xfe\xff\x00A").unwrap().as_str(), Some("A"));
        assert_eq!(decode_utf16(b"\x3d\xd8\x00\xde").unwrap().as_str(), Some("\u{1f600}"));
        assert_eq!(msg(decode_utf16(b"A\x00B")), "'utf-16-le' codec can't decode byte 0x42 in position 2: truncated data");
        assert_eq!(msg(decode_utf16(b"\xfe\xff\xdc\x00")), "'utf-16-be' codec can't decode bytes in position 2-3: illegal encoding");
        assert_eq!(
            msg(decode_utf16(b"\x00\xd8A\x00")),
            "'utf-16-le' codec can't decode bytes in position 0-1: illegal UTF-16 surrogate"
        );
        assert_eq!(msg(decode_utf16(b"\x00\xd8")), "'utf-16-le' codec can't decode bytes in position 0-1: unexpected end of data");
    }
}
