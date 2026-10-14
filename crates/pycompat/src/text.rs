//! Character classes of CPython 3.12.
//!
//! Bytes: `Py_ISSPACE`, which is also `bytes.isspace()` and the `\s` class
//! of a `bytes` regular expression: `[\t\n\v\f\r ]`.
//! Text: `str.isspace()`, `str.isprintable()` and decimal digit values, from
//! tables generated out of the oracle's `unicodedata` (Unicode 15.0) by
//! `oracle/codegen/unicode_tables.py`.

use std::cmp::Ordering;

pub use crate::unicode_tables::UNICODE_VERSION;

/// The generated ranges behind the classes (used by the differential tests).
pub mod tables {
    pub use crate::unicode_tables::{DECIMAL, NONPRINTABLE, SPACE};
}

/// `Py_ISSPACE(c)`, `bytes([c]).isspace()`, `re.match(rb"\s", bytes([c]))`.
#[inline]
pub fn bytes_isspace(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

fn in_ranges(ranges: &[(u32, u32)], c: u32) -> bool {
    ranges
        .binary_search_by(|&(lo, hi)| {
            if hi < c {
                Ordering::Less
            } else if lo > c {
                Ordering::Greater
            } else {
                Ordering::Equal
            }
        })
        .is_ok()
}

/// `ch.isspace()` (`Py_UNICODE_ISSPACE`).
pub fn str_isspace(ch: char) -> bool {
    in_ranges(tables::SPACE, ch as u32)
}

/// `ch.isprintable()` (`Py_UNICODE_ISPRINTABLE`).
pub fn str_isprintable(ch: char) -> bool {
    !in_ranges(tables::NONPRINTABLE, ch as u32)
}

/// `unicodedata.decimal(ch, None)` (`Py_UNICODE_TODECIMAL`).
pub fn decimal_value(ch: char) -> Option<u32> {
    let c = ch as u32;
    let i = tables::DECIMAL.partition_point(|&(_, hi, _)| hi < c);
    match tables::DECIMAL.get(i) {
        Some(&(lo, _, first)) if lo <= c => Some(first + (c - lo)),
        _ => None,
    }
}

/// `str(b, "utf-8", "ignore")`: decodes UTF-8, dropping each invalid
/// sequence. CPython and Rust delimit an invalid sequence the same way (the
/// "maximal subpart" of Unicode §3.9), and a truncated sequence at the end is
/// dropped whole.
pub fn decode_utf8_ignore(b: &[u8]) -> String {
    let mut out = String::with_capacity(b.len());
    let mut rest = b;
    loop {
        match std::str::from_utf8(rest) {
            Ok(s) => {
                out.push_str(s);
                return out;
            }
            Err(e) => {
                let (valid, after) = rest.split_at(e.valid_up_to());
                out.push_str(std::str::from_utf8(valid).expect("valid prefix"));
                match e.error_len() {
                    Some(n) => rest = &after[n..],
                    None => return out,
                }
            }
        }
    }
}

/// `_PyUnicode_TransformDecimalAndSpaceToASCII` (Objects/unicodeobject.c):
/// the ASCII buffer that `int(str)` and `float(str)` parse. Characters below
/// U+007F are kept, other whitespace becomes `' '` and other decimal digits
/// their ASCII digit; the first other character becomes `'?'` and ends the
/// buffer.
pub fn transform_decimal_and_space_to_ascii(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    for ch in s.chars() {
        if (ch as u32) < 127 {
            out.push(ch as u8);
        } else if str_isspace(ch) {
            out.push(b' ');
        } else if let Some(d) = decimal_value(ch) {
            out.push(b'0' + d as u8);
        } else {
            out.push(b'?');
            break;
        }
    }
    out
}
