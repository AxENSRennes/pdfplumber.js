//! CPython 3.12 `Lib/base64.py`: `b64encode` (pdfplumber's `to_b64`) and
//! `a85decode`, the decoder `pdfminer.ascii85.ascii85decode` calls.

use crate::err::{exc, PyResult};

// cpython-3.12.3/Lib/base64.py:369 a85decode
/// `base64.a85decode(b)` with the default arguments (`foldspaces=False`,
/// `adobe=False`, `ignorechars=b' \t\n\r\v'`). Raises `ValueError`.
pub fn a85decode(b: &[u8]) -> PyResult<Vec<u8>> {
    const IGNORECHARS: &[u8] = b" \t\n\r\x0b";
    let mut decoded: Vec<u8> = Vec::with_capacity(b.len() / 5 * 4 + 4);
    let mut curr: Vec<u8> = Vec::with_capacity(5);
    for &x in b.iter().chain(b"uuuu") {
        if (b'!'..=b'u').contains(&x) {
            curr.push(x);
            if curr.len() == 5 {
                let mut acc: u64 = 0;
                for &c in &curr {
                    acc = 85 * acc + u64::from(c - 33);
                }
                // struct.pack('!I', acc)
                let Ok(acc) = u32::try_from(acc) else {
                    return Err(exc::ValueError("Ascii85 overflow"));
                };
                decoded.extend_from_slice(&acc.to_be_bytes());
                curr.clear();
            }
        } else if x == b'z' {
            if !curr.is_empty() {
                return Err(exc::ValueError("z inside Ascii85 5-tuple"));
            }
            decoded.extend_from_slice(b"\0\0\0\0");
        } else if IGNORECHARS.contains(&x) {
            // Skip whitespace
        } else {
            // '%c' % x: the character of code point x
            return Err(exc::ValueError(format!("Non-Ascii85 digit found: {}", char::from(x))));
        }
    }
    let padding = 4 - curr.len();
    if padding != 0 {
        // Throw away the extra padding
        decoded.truncate(decoded.len().saturating_sub(padding));
    }
    Ok(decoded)
}

// cpython-3.12.3/Lib/base64.py:51 b64encode
/// `base64.b64encode(s).decode("ascii")`: the standard alphabet, with
/// padding, no newline (`binascii.b2a_base64(s, newline=False)`).
pub fn b64encode(s: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(s.len().div_ceil(3) * 4);
    for chunk in s.chunks(3) {
        let b = [chunk[0], chunk.get(1).copied().unwrap_or(0), chunk.get(2).copied().unwrap_or(0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod b64_tests {
    #[test]
    fn b64encode() {
        assert_eq!(super::b64encode(b""), "");
        assert_eq!(super::b64encode(b"f"), "Zg==");
        assert_eq!(super::b64encode(b"fo"), "Zm8=");
        assert_eq!(super::b64encode(b"foo"), "Zm9v");
        assert_eq!(super::b64encode(b"\xff\xfe\xfd\x00"), "//79AA==");
    }
}
