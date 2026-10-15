//! `repr()` of floats, bytes and str, and `json.dumps(str)`.

use std::fmt::Write;

use crate::text::str_isprintable;

const HEX: &[u8; 16] = b"0123456789abcdef";

/// Shortest round-trip decimal digits of a finite, non-negative double and
/// the position of the decimal point (`_Py_dg_dtoa` mode 0): the value is
/// `0.DIGITS * 10**decpt`.
///
/// Rust's float formatting also produces the shortest digit string that
/// reads back to the same double, and the closest one to the double when
/// several have that length. The two differ only when the double lies
/// exactly halfway between the two closest candidates: dtoa then takes the
/// one with an even last digit, Rust the upper one. That case is detected
/// exactly and corrected.
pub fn shortest_digits(x: f64) -> (String, i32) {
    debug_assert!(x.is_finite() && x >= 0.0);
    let sci = format!("{x:e}");
    let (mantissa, exp) = sci.split_once('e').expect("LowerExp has an exponent");
    let exp: i32 = exp.parse().expect("integer exponent");
    let digits: String = mantissa.chars().filter(|&c| c != '.').collect();
    let decpt = exp + 1;
    if digits.as_bytes()[digits.len() - 1] % 2 == 1 {
        if let Some(even) = even_tie_alternative(x, &digits, decpt) {
            return (even, decpt);
        }
    }
    (digits, decpt)
}

/// When `x` is exactly halfway between `digits` (odd last digit) and its
/// neighbor with one less or one more in the last digit, and that neighbor
/// also reads back as `x`, returns the neighbor (whose last digit is even).
fn even_tie_alternative(x: f64, digits: &str, decpt: i32) -> Option<String> {
    use num_bigint::BigUint;

    // x = m * 2**e with m odd.
    let bits = x.to_bits();
    let biased = ((bits >> 52) & 0x7ff) as i32;
    let frac = bits & ((1u64 << 52) - 1);
    let (m, e) = if biased == 0 { (frac, -1074) } else { (frac | (1u64 << 52), biased - 1075) };
    if m == 0 {
        return None;
    }
    let tz = m.trailing_zeros() as i32;
    let (m, e) = (m >> tz, e + tz);
    let n = digits.len() as i32;
    // The midpoint has n+1 significant digits ending in 5, hence exactly
    // n+1-decpt decimal places; x has exactly -e of them when e < 0, and
    // none when x is an integer.
    let places = n + 1 - decpt;
    let possible = if e < 0 { -e == places } else { places <= 0 };
    if !possible {
        return None;
    }
    let d: BigUint = digits.parse().expect("decimal digits");
    let k = decpt - n; // the candidates are D * 10**k
    for down in [true, false] {
        let last = digits.as_bytes()[digits.len() - 1];
        if (down && last == b'0') || (!down && last == b'9') {
            continue;
        }
        // Is 2x == (2D -+ 1) * 10**k exactly?
        let twice_mid = if down { &d * 2u32 - 1u32 } else { &d * 2u32 + 1u32 };
        let (mut lhs, mut rhs) = (BigUint::from(m), twice_mid);
        let a = e + 1; // 2x = m * 2**a
        let shift = a.unsigned_abs() as usize;
        if a >= 0 {
            lhs <<= shift;
        } else {
            rhs <<= shift;
        }
        let scale = num_traits::pow(BigUint::from(10u32), k.unsigned_abs() as usize);
        if k >= 0 {
            rhs *= scale;
        } else {
            lhs *= scale;
        }
        if lhs != rhs {
            continue;
        }
        let alt = if down { &d - 1u32 } else { &d + 1u32 }.to_string();
        let reread: f64 = format!("{alt}e{k}").parse().expect("decimal number");
        if reread == x && alt.len() == digits.len() {
            return Some(alt);
        }
    }
    None
}

/// `repr(x)` / `str(x)` for a float (`float_repr`, format code `'r'`):
/// shortest round-trip digits, exponent form when the decimal exponent is
/// below -4 or at least 16.
pub fn float_repr(x: f64) -> String {
    if x.is_nan() {
        return "nan".to_owned();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf" } else { "-inf" }.to_owned();
    }
    let (digits, decpt) = shortest_digits(x.abs());
    let mut out = String::with_capacity(digits.len() + 8);
    if x.is_sign_negative() {
        out.push('-');
    }
    let n = digits.len() as i32;
    if decpt <= -4 || decpt > 16 {
        // format_float_short: d[.ddd]e+XX, at least two exponent digits.
        out.push_str(&digits[..1]);
        if n > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        let exp = decpt - 1;
        let _ = write!(out, "e{}{:02}", if exp < 0 { '-' } else { '+' }, exp.abs());
    } else if decpt <= 0 {
        out.push_str("0.");
        out.extend(std::iter::repeat_n('0', decpt.unsigned_abs() as usize));
        out.push_str(&digits);
    } else if decpt >= n {
        out.push_str(&digits);
        out.extend(std::iter::repeat_n('0', (decpt - n).unsigned_abs() as usize));
        out.push_str(".0");
    } else {
        out.push_str(&digits[..decpt as usize]);
        out.push('.');
        out.push_str(&digits[decpt as usize..]);
    }
    out
}

/// `repr(b)` for `bytes` (`PyBytes_Repr` with smartquotes).
pub fn bytes_repr(b: &[u8]) -> String {
    let quote = if b.contains(&b'\'') && !b.contains(&b'"') { b'"' } else { b'\'' };
    let mut out = String::with_capacity(b.len() + 3);
    out.push('b');
    out.push(quote as char);
    for &c in b {
        if c == quote || c == b'\\' {
            out.push('\\');
            out.push(c as char);
        } else if c == b'\t' {
            out.push_str("\\t");
        } else if c == b'\n' {
            out.push_str("\\n");
        } else if c == b'\r' {
            out.push_str("\\r");
        } else if !(b' '..0x7f).contains(&c) {
            out.push_str("\\x");
            out.push(HEX[(c >> 4) as usize] as char);
            out.push(HEX[(c & 15) as usize] as char);
        } else {
            out.push(c as char);
        }
    }
    out.push(quote as char);
    out
}

/// `repr(s)` for `str` (`unicode_repr`).
pub fn str_repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for ch in s.chars() {
        let c = ch as u32;
        if ch == quote || ch == '\\' {
            out.push('\\');
            out.push(ch);
        } else if ch == '\t' {
            out.push_str("\\t");
        } else if ch == '\n' {
            out.push_str("\\n");
        } else if ch == '\r' {
            out.push_str("\\r");
        } else if c < 0x20 || c == 0x7f {
            let _ = write!(out, "\\x{c:02x}");
        } else if c < 0x7f || str_isprintable(ch) {
            out.push(ch);
        } else if c <= 0xff {
            let _ = write!(out, "\\x{c:02x}");
        } else if c <= 0xffff {
            let _ = write!(out, "\\u{c:04x}");
        } else {
            let _ = write!(out, "\\U{c:08x}");
        }
    }
    out.push(quote);
    out
}

/// [`str_repr`] of a Python `str` given by its code points, which may include
/// surrogates (not printable: `\udxxx`). The quote is chosen as in
/// `unicode_repr`.
pub fn str_repr_code_points(code_points: &[u32]) -> String {
    let has = |c: char| code_points.contains(&(c as u32));
    let quote = if has('\'') && !has('"') { '"' } else { '\'' };
    let mut out = String::with_capacity(code_points.len() + 2);
    out.push(quote);
    for &c in code_points {
        match char::from_u32(c) {
            Some(ch) if ch == quote || ch == '\\' => {
                out.push('\\');
                out.push(ch);
            }
            Some('\t') => out.push_str("\\t"),
            Some('\n') => out.push_str("\\n"),
            Some('\r') => out.push_str("\\r"),
            _ if c < 0x20 || c == 0x7f => {
                let _ = write!(out, "\\x{c:02x}");
            }
            Some(ch) if c < 0x7f || str_isprintable(ch) => out.push(ch),
            _ if c <= 0xff => {
                let _ = write!(out, "\\x{c:02x}");
            }
            _ if c <= 0xffff => {
                let _ = write!(out, "\\u{c:04x}");
            }
            _ => {
                let _ = write!(out, "\\U{c:08x}");
            }
        }
    }
    out.push(quote);
    out
}

/// `json.dumps(s)` with the default `ensure_ascii=True` (`ascii_escape_unicode`
/// in Modules/_json.c): ASCII printable characters except `"` and `\` as is,
/// the short escapes `\" \\ \n \r \t \b \f`, everything else as `\uXXXX`
/// (lowercase hex, UTF-16 surrogate pairs above U+FFFF).
pub fn json_dumps_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    write_json_str(&mut out, s);
    out
}

/// [`json_dumps_str`] appending to `out`.
pub fn write_json_str(out: &mut String, s: &str) {
    out.push('"');
    for ch in s.chars() {
        write_json_char(out, ch);
    }
    out.push('"');
}

/// [`json_dumps_str`] of a Python `str` given by its code points, which may
/// include surrogates (U+D800..U+DFFF, written as one `\uXXXX` escape each,
/// as `ascii_escape_unichar` does). Code points above U+10FFFF are not
/// Python characters and are skipped.
pub fn write_json_code_points(out: &mut String, code_points: &[u32]) {
    out.push('"');
    for &c in code_points {
        match char::from_u32(c) {
            Some(ch) => write_json_char(out, ch),
            None if c < 0x10000 => {
                let _ = write!(out, "\\u{c:04x}");
            }
            None => {}
        }
    }
    out.push('"');
}

#[inline]
fn write_json_char(out: &mut String, ch: char) {
    match ch {
        '"' => out.push_str("\\\""),
        '\\' => out.push_str("\\\\"),
        '\n' => out.push_str("\\n"),
        '\r' => out.push_str("\\r"),
        '\t' => out.push_str("\\t"),
        '\u{8}' => out.push_str("\\b"),
        '\u{c}' => out.push_str("\\f"),
        ' '..='~' => out.push(ch),
        _ => {
            let mut units = [0u16; 2];
            for unit in ch.encode_utf16(&mut units) {
                let _ = write!(out, "\\u{unit:04x}");
            }
        }
    }
}
