//! `bytes % arg` for one int argument (`Objects/bytesobject.c`
//! `_PyBytes_FormatEx` of CPython 3.12): printf-style formatting, with
//! CPython's errors.
//!
//! pdfplumber computes `rotation % 360` on whatever `/Rotate` holds; a
//! PDF string there makes it a bytes formatting, whose result (or error)
//! is the page's rotation. The argument is a single `int` (not a tuple,
//! not a mapping): one conversion may consume it, a second one raises
//! "not enough arguments", an unused argument raises "not all arguments
//! converted".
//!
//! A width or a precision as large as the format says (up to
//! `PY_SSIZE_T_MAX` and `INT_MAX - 3`) asks for that much memory: CPython
//! raises `MemoryError` when the allocation fails (`_PyBytesWriter_Prepare`,
//! `PyUnicode_New`), and so does the port ([`reserve`]): never an abort,
//! and on a 32-bit target (wasm32) a size that does not fit the address
//! space fails like an allocation. Between "fits in memory" and "does not"
//! the limit is the machine's, in Python as here.

use crate::err::{exc, PyResult};

/// Makes room for `extra` more bytes in `out`, or `MemoryError` (Python's
/// failed allocation): a size beyond the address space, or an allocation
/// the allocator refuses.
fn reserve(out: &mut Vec<u8>, extra: i64) -> PyResult<()> {
    let extra = usize::try_from(extra).map_err(|_| exc::MemoryError(""))?;
    out.try_reserve_exact(extra).map_err(|_| exc::MemoryError(""))
}

const F_LJUST: u8 = 1;
const F_SIGN: u8 = 2;
const F_BLANK: u8 = 4;
const F_ALT: u8 = 8;
const F_ZERO: u8 = 16;

/// The format being read: `fmt` and `fmtcnt` of the C code.
struct Cursor<'a> {
    fmt: &'a [u8],
    i: usize,
    /// `fmtcnt`: the bytes left after the last one read (negative once a
    /// read went past the end).
    cnt: i64,
}

impl Cursor<'_> {
    /// `if (--fmtcnt >= 0) c = *fmt++;`: the next byte, if any.
    fn next(&mut self) -> Option<u8> {
        self.cnt -= 1;
        if self.cnt < 0 {
            return None;
        }
        let c = self.fmt[self.i];
        self.i += 1;
        Some(c)
    }
}

/// `fmt % arg` for an `int` argument `arg`.
pub fn bytes_mod_int(fmt: &[u8], arg: i64) -> PyResult<Vec<u8>> {
    let mut out: Vec<u8> = Vec::new();
    // getnextarg() on a non-tuple: argidx goes from -2 to -1
    let mut arg_used = false;
    let mut next_arg = || -> PyResult<i64> {
        if arg_used {
            return Err(exc::TypeError("not enough arguments for format string"));
        }
        arg_used = true;
        Ok(arg)
    };
    let mut i = 0;
    while i < fmt.len() {
        if fmt[i] != b'%' {
            out.push(fmt[i]);
            i += 1;
            continue;
        }
        // Got a format specifier
        i += 1;
        if fmt.get(i) == Some(&b'%') {
            out.push(b'%');
            i += 1;
            continue;
        }
        if fmt.get(i) == Some(&b'(') {
            return Err(exc::TypeError("format requires a mapping"));
        }
        let mut cur = Cursor { fmt, i, cnt: (fmt.len() - i) as i64 };
        let mut flags = 0u8;
        let mut width: i64 = -1;
        let mut prec: i64 = -1;
        let mut c: u8 = 0;
        // Parse flags. Example: "%+i" => flags=F_SIGN.
        while let Some(ch) = cur.next() {
            c = ch;
            match ch {
                b'-' => flags |= F_LJUST,
                b'+' => flags |= F_SIGN,
                b' ' => flags |= F_BLANK,
                b'#' => flags |= F_ALT,
                b'0' => flags |= F_ZERO,
                _ => break,
            }
        }
        // Parse width. Example: "%10s" => width=10
        if c == b'*' {
            width = next_arg()?;
            if width < 0 {
                flags |= F_LJUST;
                width = -width;
            }
            if let Some(ch) = cur.next() {
                c = ch;
            }
        } else if c.is_ascii_digit() {
            width = i64::from(c - b'0');
            while let Some(ch) = cur.next() {
                c = ch;
                if !c.is_ascii_digit() {
                    break;
                }
                if width > (i64::MAX - i64::from(c - b'0')) / 10 {
                    return Err(exc::ValueError("width too big"));
                }
                width = width * 10 + i64::from(c - b'0');
            }
        }
        // Parse precision. Example: "%.3f" => prec=3
        if c == b'.' {
            prec = 0;
            if let Some(ch) = cur.next() {
                c = ch;
            }
            if c == b'*' {
                let v = next_arg()?;
                if v > i64::from(i32::MAX) || v < i64::from(i32::MIN) {
                    return Err(exc::OverflowError("Python int too large to convert to C int"));
                }
                prec = v.max(0);
                if let Some(ch) = cur.next() {
                    c = ch;
                }
            } else if c.is_ascii_digit() {
                prec = i64::from(c - b'0');
                while let Some(ch) = cur.next() {
                    c = ch;
                    if !c.is_ascii_digit() {
                        break;
                    }
                    if prec > (i64::from(i32::MAX) - i64::from(c - b'0')) / 10 {
                        return Err(exc::ValueError("prec too big"));
                    }
                    prec = prec * 10 + i64::from(c - b'0');
                }
            }
        }
        if cur.cnt >= 0 && matches!(c, b'h' | b'l' | b'L') {
            if let Some(ch) = cur.next() {
                c = ch;
            }
        }
        if cur.cnt < 0 {
            return Err(exc::ValueError("incomplete format"));
        }
        i = cur.i;
        let v = next_arg()?;
        // the converted text, whether it is a number (sign, '0' fill)
        let (text, numeric): (Vec<u8>, bool) = match c {
            b'r' | b'a' => {
                let mut t = v.to_string().into_bytes();
                if prec >= 0 {
                    // at most `prec` characters
                    t.truncate(usize::try_from(prec).unwrap_or(usize::MAX));
                }
                (t, false)
            }
            b's' | b'b' => {
                return Err(exc::TypeError(
                    "%b requires a bytes-like object, or an object that implements __bytes__, not 'int'",
                ))
            }
            b'i' | b'd' | b'u' | b'o' | b'x' | b'X' => (format_long(v, flags & F_ALT != 0, prec, c)?, true),
            b'e' | b'E' | b'f' | b'F' | b'g' | b'G' => (format_float(v as f64, flags & F_ALT != 0, prec, c)?, true),
            b'c' => {
                if !(0..256).contains(&v) {
                    return Err(exc::OverflowError("%c arg not in range(256)"));
                }
                (vec![v as u8], false)
            }
            _ => {
                return Err(exc::ValueError(format!(
                    "unsupported format character '{}' (0x{:x}) at index {}",
                    char::from(c),
                    c,
                    i - 1
                )))
            }
        };
        let fill = if numeric && flags & F_ZERO != 0 { b'0' } else { b' ' };
        let mut pbuf: &[u8] = &text;
        let mut len = pbuf.len() as i64;
        let mut sign: u8 = 0;
        if numeric {
            if pbuf.first().is_some_and(|&s| s == b'-' || s == b'+') {
                sign = pbuf[0];
                pbuf = &pbuf[1..];
                len -= 1;
            } else if flags & F_SIGN != 0 {
                sign = b'+';
            } else if flags & F_BLANK != 0 {
                sign = b' ';
            }
        }
        if width < len {
            width = len;
        }
        // alloc = width (+ 1 for a sign when len == width): the writer
        // allocates it before writing (_PyBytesWriter_Prepare)
        reserve(&mut out, width.saturating_add(i64::from(sign != 0 && len == width)))?;
        // Write the sign if needed
        if sign != 0 {
            if fill != b' ' {
                out.push(sign);
            }
            if width > len {
                width -= 1;
            }
        }
        // Write the numeric prefix for "x", "X" and "o" formats if the
        // alternate form is used. For example, write "0x" for the "%#x"
        // format.
        let alt_prefix = flags & F_ALT != 0 && matches!(c, b'o' | b'x' | b'X');
        if alt_prefix {
            if fill != b' ' {
                out.extend_from_slice(&pbuf[..2]);
                pbuf = &pbuf[2..];
            }
            width = (width - 2).max(0);
            len -= 2;
        }
        // Pad left with the fill character if needed
        if width > len && flags & F_LJUST == 0 {
            out.extend(std::iter::repeat_n(fill, (width - len) as usize));
            width = len;
        }
        // If padding with spaces: write sign if needed and/or numeric
        // prefix if the alternate form is used
        if fill == b' ' {
            if sign != 0 {
                out.push(sign);
            }
            if alt_prefix {
                out.extend_from_slice(&pbuf[..2]);
                pbuf = &pbuf[2..];
            }
        }
        // Copy bytes
        out.extend_from_slice(&pbuf[..len as usize]);
        // Pad right with the fill character if needed
        if width > len {
            out.extend(std::iter::repeat_n(b' ', (width - len) as usize));
        }
    }
    if !arg_used {
        return Err(exc::TypeError("not all arguments converted during bytes formatting"));
    }
    Ok(out)
}

/// `_PyUnicode_FormatLong(v, alt, prec, type)`
fn format_long(v: i64, alt: bool, prec: i64, conv: u8) -> PyResult<Vec<u8>> {
    if prec > i64::from(i32::MAX) - 3 {
        return Err(exc::OverflowError("precision too large"));
    }
    let magnitude = i128::from(v).unsigned_abs();
    let (prefix, digits) = match conv {
        b'o' => ("0o", format!("{magnitude:o}")),
        b'x' | b'X' => ("0x", format!("{magnitude:x}")),
        _ => ("", magnitude.to_string()),
    };
    let mut out: Vec<u8> = Vec::new();
    // the new str of numnondigits + max(prec, numdigits) characters
    // (PyUnicode_New)
    let zeros = (prec - digits.len() as i64).max(0);
    reserve(&mut out, 3 + digits.len() as i64 + zeros)?;
    if v < 0 {
        out.push(b'-');
    }
    if alt {
        out.extend_from_slice(prefix.as_bytes());
    }
    // Fill with leading zeroes to meet minimum width.
    out.resize(out.len() + zeros as usize, b'0');
    out.extend_from_slice(digits.as_bytes());
    if conv == b'X' {
        out.make_ascii_uppercase();
    }
    Ok(out)
}

/// The precision past which the digits of the float of an `i64` are all
/// zeros: such a float is an integer of at most 19 significant digits.
const EXACT_PREC: i64 = 40;

/// `PyOS_double_to_string(x, type, prec, alt ? Py_DTSF_ALT : 0)` for a
/// finite `x` (the float of an int): C's `%e`, `%f`, `%g` and their upper
/// case forms, with exact decimal rounding (Rust's formatting rounds the
/// exact value, as glibc's printf and Python's dtoa do).
///
/// `x` is an integer, so beyond [`EXACT_PREC`] a larger precision only
/// adds zeros (after the point for `%f` and the `#` form of `%g`, in the
/// mantissa for `%e`; `%g` strips them): the text is computed at that
/// precision and the zeros added, after making room for them ([`reserve`]:
/// `MemoryError` where the allocation fails).
fn format_float(x: f64, alt: bool, prec: i64, conv: u8) -> PyResult<Vec<u8>> {
    let prec = if prec < 0 { 6 } else { prec };
    let extra = (prec - EXACT_PREC).max(0);
    let mut text = format_float_exact(x, alt, prec.min(EXACT_PREC) as usize, conv);
    // where the zeros go (none for `%g`; `extra` may be 0)
    let zeros_at = match conv.to_ascii_lowercase() {
        b'f' => Some(text.len()),
        b'e' => Some(text.iter().position(|&b| b == b'e' || b == b'E').expect("an exponent")),
        _ if alt => Some(text.len()),
        _ => None,
    };
    if let Some(at) = zeros_at {
        let mut out = Vec::new();
        reserve(&mut out, text.len() as i64 + extra)?;
        out.extend_from_slice(&text[..at]);
        out.resize(at + extra as usize, b'0');
        out.extend_from_slice(&text[at..]);
        text = out;
    }
    Ok(text)
}

/// [`format_float`] at a precision of at most [`EXACT_PREC`].
fn format_float_exact(x: f64, alt: bool, prec: usize, conv: u8) -> Vec<u8> {
    let mut text = match conv.to_ascii_lowercase() {
        b'f' => {
            let mut t = format!("{x:.prec$}");
            if alt && prec == 0 {
                t.push('.');
            }
            t
        }
        b'e' => exp_form(x, prec, alt),
        _ => {
            // %g: precision 0 is 1
            let p = prec.max(1);
            let exponent = exp_of(x, p - 1);
            let t = if exponent < -4 || exponent >= p as i32 {
                exp_form(x, p - 1, alt)
            } else {
                let decimals = (p as i32 - 1 - exponent) as usize;
                let mut t = format!("{x:.decimals$}");
                if alt && !t.contains('.') {
                    t.push('.');
                }
                t
            };
            if alt {
                t
            } else {
                // remove the trailing zeros (and point) of the mantissa
                let (mantissa, rest) = t.split_at(t.find('e').unwrap_or(t.len()));
                let mantissa = if mantissa.contains('.') {
                    mantissa.trim_end_matches('0').trim_end_matches('.')
                } else {
                    mantissa
                };
                format!("{mantissa}{rest}")
            }
        }
    };
    if conv.is_ascii_uppercase() {
        text = text.to_uppercase();
    }
    text.into_bytes()
}

/// The decimal exponent of `x` written with `decimals` digits after the
/// point in exponential form.
fn exp_of(x: f64, decimals: usize) -> i32 {
    let t = format!("{x:.decimals$e}");
    t[t.find('e').expect("an exponent") + 1..].parse().expect("an int")
}

/// C's `%.<decimals>e`: an exponent with a sign and at least two digits.
fn exp_form(x: f64, decimals: usize, alt: bool) -> String {
    let t = format!("{x:.decimals$e}");
    let k = t.find('e').expect("an exponent");
    let exponent: i32 = t[k + 1..].parse().expect("an int");
    let mut mantissa = t[..k].to_owned();
    if alt && !mantissa.contains('.') {
        mantissa.push('.');
    }
    let sign = if exponent < 0 { '-' } else { '+' };
    format!("{mantissa}e{sign}{:02}", exponent.abs())
}
