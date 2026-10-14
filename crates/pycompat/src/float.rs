//! Python `float` operations that differ from plain IEEE arithmetic:
//! `float()` parsing, floor division and modulo, `**`, and `round()`.
//! References are to CPython 3.12 `Objects/floatobject.c` and
//! `Python/pystrtod.c`.

use num_bigint::BigInt;
use num_integer::Integer;
use num_traits::{Signed, Zero};

use crate::err::{exc, PyErr, PyResult};
use crate::int::{bigint_from_integral_f64, PyInt};
use crate::repr::{bytes_repr, str_repr};
use crate::text::{bytes_isspace, transform_decimal_and_space_to_ascii};

// -- float() parsing ------------------------------------------------------

/// `float(b)` for a `bytes` argument (`PyFloat_FromString`).
#[inline]
pub fn float_from_bytes(b: &[u8]) -> PyResult<f64> {
    match fast_decimal(b) {
        Some(v) => Ok(v),
        None => float_from_bytes_general(b),
    }
}

fn float_from_bytes_general(b: &[u8]) -> PyResult<f64> {
    string_to_float(b).ok_or_else(|| exc::ValueError(format!("could not convert string to float: {}", bytes_repr(b))))
}

/// `float(s)` for a `str` argument: Unicode digits and whitespace count as
/// their ASCII forms (`_PyUnicode_TransformDecimalAndSpaceToASCII`).
pub fn float_from_str(s: &str) -> PyResult<f64> {
    string_to_float(&transform_decimal_and_space_to_ascii(s))
        .ok_or_else(|| exc::ValueError(format!("could not convert string to float: {}", str_repr(s))))
}

/// Powers of ten that are exact doubles.
const POW10: [f64; 23] = [
    1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10, 1e11, 1e12, 1e13, 1e14, 1e15, 1e16, 1e17, 1e18, 1e19, 1e20,
    1e21, 1e22,
];

/// Fast path for the common `[+-]?digits[.digits]` forms: when the digits
/// read as an integer `m < 2**53` with `k <= 22` decimals, `m` and `10**k`
/// are exact doubles and the IEEE division `m / 10**k` is the correctly
/// rounded value (Clinger), equal to what `_Py_dg_strtod` returns. `None`
/// sends anything else to the general path.
#[inline]
fn fast_decimal(s: &[u8]) -> Option<f64> {
    let (negative, body) = match s.first()? {
        b'-' => (true, &s[1..]),
        b'+' => (false, &s[1..]),
        _ => (false, s),
    };
    let mut mantissa: u64 = 0;
    let mut ndigits = 0usize;
    let mut decimals: Option<usize> = None;
    for &c in body {
        match c {
            b'0'..=b'9' => {
                mantissa = mantissa * 10 + u64::from(c - b'0');
                if mantissa >= 1 << 53 {
                    return None;
                }
                ndigits += 1;
                if let Some(k) = decimals.as_mut() {
                    *k += 1;
                }
            }
            b'.' if decimals.is_none() => decimals = Some(0),
            _ => return None,
        }
    }
    let k = decimals.unwrap_or(0);
    if ndigits == 0 || k >= POW10.len() {
        return None;
    }
    let v = mantissa as f64 / POW10[k];
    Some(if negative { -v } else { v })
}

/// `_Py_string_to_number_with_underscores` + `float_from_string_inner`;
/// `None` where CPython raises "could not convert string to float".
fn string_to_float(s: &[u8]) -> Option<f64> {
    if let Some(v) = fast_decimal(s) {
        return Some(v);
    }
    let owned;
    let s = if s.contains(&b'_') {
        // Underscores are only allowed between digits.
        let mut out = Vec::with_capacity(s.len());
        let mut prev = 0u8;
        for &c in s {
            if c == b'_' {
                if !prev.is_ascii_digit() {
                    return None;
                }
            } else {
                if prev == b'_' && !c.is_ascii_digit() {
                    return None;
                }
                out.push(c);
            }
            prev = c;
        }
        if prev == b'_' {
            return None;
        }
        owned = out;
        &owned[..]
    } else {
        s
    };
    let mut start = 0;
    let mut end = s.len();
    while start < end && bytes_isspace(s[start]) {
        start += 1;
    }
    if start == end {
        return None;
    }
    while start < end - 1 && bytes_isspace(s[end - 1]) {
        end -= 1;
    }
    let s = &s[start..end];
    match strtod_prefix(s) {
        Some(n) if n == s.len() => {
            let text = std::str::from_utf8(s).expect("ASCII number");
            // Rust's parser is correctly rounded like _Py_dg_strtod, and
            // saturates to 0 or infinity on underflow and overflow.
            Some(text.parse::<f64>().expect("validated number"))
        }
        Some(_) => None,
        None => parse_inf_or_nan(s),
    }
}

/// Length of the longest prefix `_Py_dg_strtod` converts: an optional sign,
/// digits with at most one `.` and at least one digit, then an exponent only
/// if it has digits. `None` when no number starts here.
fn strtod_prefix(s: &[u8]) -> Option<usize> {
    let mut i = 0;
    if matches!(s.first(), Some(b'+' | b'-')) {
        i += 1;
    }
    let mut ndigits = 0;
    while i < s.len() && s[i].is_ascii_digit() {
        i += 1;
        ndigits += 1;
    }
    if i < s.len() && s[i] == b'.' {
        i += 1;
        while i < s.len() && s[i].is_ascii_digit() {
            i += 1;
            ndigits += 1;
        }
    }
    if ndigits == 0 {
        return None;
    }
    if i < s.len() && matches!(s[i], b'e' | b'E') {
        let mut j = i + 1;
        if j < s.len() && matches!(s[j], b'+' | b'-') {
            j += 1;
        }
        let digits_start = j;
        while j < s.len() && s[j].is_ascii_digit() {
            j += 1;
        }
        if j > digits_start {
            i = j;
        }
    }
    Some(i)
}

/// `_Py_parse_inf_or_nan`, required to consume the whole string.
fn parse_inf_or_nan(s: &[u8]) -> Option<f64> {
    let (negate, rest) = match s.first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let value = if rest.eq_ignore_ascii_case(b"inf") || rest.eq_ignore_ascii_case(b"infinity") {
        f64::INFINITY
    } else if rest.eq_ignore_ascii_case(b"nan") {
        f64::NAN
    } else {
        return None;
    };
    Some(if negate { -value } else { value })
}

// -- arithmetic -------------------------------------------------------------

/// `a / b` (`float_div`).
pub fn float_div(a: f64, b: f64) -> PyResult<f64> {
    if b == 0.0 {
        return Err(exc::ZeroDivisionError("float division by zero"));
    }
    Ok(a / b)
}

/// `a % b` (`float_rem`): the result has the sign of `b`.
pub fn float_mod(a: f64, b: f64) -> PyResult<f64> {
    if b == 0.0 {
        return Err(exc::ZeroDivisionError("float modulo"));
    }
    let mut m = a % b; // C fmod
    if m != 0.0 {
        if (b < 0.0) != (m < 0.0) {
            m += b;
        }
    } else {
        m = 0.0f64.copysign(b);
    }
    Ok(m)
}

/// `a // b` (`float_floor_div` via `_float_div_mod`).
pub fn float_floordiv(a: f64, b: f64) -> PyResult<f64> {
    if b == 0.0 {
        return Err(exc::ZeroDivisionError("float floor division by zero"));
    }
    let m = a % b;
    let mut div = (a - m) / b;
    if m != 0.0 && (b < 0.0) != (m < 0.0) {
        div -= 1.0;
    }
    Ok(if div != 0.0 {
        let mut floordiv = div.floor();
        if div - floordiv > 0.5 {
            floordiv += 1.0;
        }
        floordiv
    } else {
        0.0f64.copysign(a / b)
    })
}

fn is_odd_integer(x: f64) -> bool {
    x.abs() % 2.0 == 1.0
}

/// `a ** b` (`float_pow`). A negative base with a non-integral exponent
/// gives a `complex` in Python, which the port does not model.
pub fn float_pow(iv: f64, iw: f64) -> PyResult<f64> {
    if iw == 0.0 {
        return Ok(1.0);
    }
    if iv.is_nan() {
        return Ok(iv);
    }
    if iw.is_nan() {
        return Ok(if iv == 1.0 { 1.0 } else { iw });
    }
    if iw.is_infinite() {
        let av = iv.abs();
        return Ok(if av == 1.0 {
            1.0
        } else if (iw > 0.0) == (av > 1.0) {
            iw.abs()
        } else {
            0.0
        });
    }
    if iv.is_infinite() {
        let odd = is_odd_integer(iw);
        return Ok(if iw > 0.0 {
            if odd {
                iv
            } else {
                iv.abs()
            }
        } else if odd {
            0.0f64.copysign(iv)
        } else {
            0.0
        });
    }
    if iv == 0.0 {
        if iw < 0.0 {
            return Err(exc::ZeroDivisionError("0.0 cannot be raised to a negative power"));
        }
        return Ok(if is_odd_integer(iw) { iv } else { 0.0 });
    }
    let mut base = iv;
    let mut negate = false;
    if base < 0.0 {
        if iw != iw.floor() {
            return Err(PyErr::not_ported("complex result of a negative float raised to a fractional power"));
        }
        base = -base;
        negate = is_odd_integer(iw);
    }
    if base == 1.0 {
        return Ok(if negate { -1.0 } else { 1.0 });
    }
    // The platform pow(): glibc natively, the same correctly-rounded-in-
    // practice algorithm (ARM optimized-routines) in Rust's wasm libm.
    let ix = base.powf(iw);
    if ix.is_infinite() {
        return Err(exc::OverflowError("(34, 'Numerical result out of range')"));
    }
    Ok(if negate { -ix } else { ix })
}

// -- round() ------------------------------------------------------------------

/// `int(x)` for a float that is already integral (`PyLong_FromDouble`).
pub fn float_to_int(x: f64) -> PyResult<PyInt> {
    if x.is_infinite() {
        return Err(exc::OverflowError("cannot convert float infinity to integer"));
    }
    if x.is_nan() {
        return Err(exc::ValueError("cannot convert float NaN to integer"));
    }
    Ok(PyInt::from(bigint_from_integral_f64(x.trunc())))
}

/// `round(x)` for a float (`float___round___impl` without ndigits): half to
/// even, as an int.
pub fn round_float(x: f64) -> PyResult<PyInt> {
    let mut rounded = x.round(); // C round(): half away from zero
    if (x - rounded).abs() == 0.5 {
        rounded = 2.0 * (x / 2.0).round();
    }
    float_to_int(rounded)
}

/// `round(x, ndigits)` for a float: the double nearest to the decimal value
/// of `x` rounded half to even at `ndigits` decimals (`double_round`, which
/// rounds the exact binary value with `_Py_dg_dtoa` mode 3).
pub fn round_float_ndigits(x: f64, ndigits: i64) -> PyResult<f64> {
    const NDIGITS_MAX: i64 = 323; // (DBL_MANT_DIG - DBL_MIN_EXP) * 0.30103
    const NDIGITS_MIN: i64 = -308; // -(DBL_MAX_EXP + 1) * 0.30103
    if !x.is_finite() || ndigits > NDIGITS_MAX {
        return Ok(x);
    }
    if ndigits < NDIGITS_MIN {
        return Ok(0.0 * x);
    }
    // x = mant * 2**exp exactly; scale by 10**ndigits as a fraction num/den.
    let bits = x.abs().to_bits();
    let biased = ((bits >> 52) & 0x7ff) as i64;
    let frac = bits & ((1u64 << 52) - 1);
    let (mant, exp) = if biased == 0 { (frac, -1074) } else { (frac | (1u64 << 52), biased - 1075) };
    let mut num = BigInt::from(mant);
    let mut den = BigInt::from(1);
    if exp >= 0 {
        num <<= exp as usize;
    } else {
        den <<= (-exp) as usize;
    }
    let ten = BigInt::from(10);
    if ndigits >= 0 {
        num *= num_traits::pow(ten, ndigits as usize);
    } else {
        den *= num_traits::pow(ten, (-ndigits) as usize);
    }
    let (mut q, r) = num.div_rem(&den);
    let twice = &r * 2;
    if twice > den || (twice == den && q.is_odd()) {
        q += 1;
    }
    let negative = x.is_sign_negative();
    if q.is_zero() {
        return Ok(if negative { -0.0 } else { 0.0 });
    }
    // Back to binary through the correctly rounded decimal parser, like
    // CPython's "<digits>e<exp>" string and _Py_dg_strtod.
    let text = format!("{}{}e{}", if negative { "-" } else { "" }, q.abs(), -ndigits);
    let rounded: f64 = text.parse().expect("decimal number");
    if rounded.is_infinite() {
        return Err(exc::OverflowError("rounded value too large to represent"));
    }
    Ok(rounded)
}

/// `round(i, ndigits)` for an int (`long_round`): unchanged when
/// `ndigits >= 0`, else rounded half to even to a multiple of
/// `10**-ndigits`.
pub fn round_int_ndigits(i: &PyInt, ndigits: i64) -> PyInt {
    if ndigits >= 0 {
        return i.clone();
    }
    let v = i.to_bigint();
    let k = ndigits.unsigned_abs();
    // |v| < 10**ndigits(v) <= 10**(k-1) < 10**k / 2 rounds to zero.
    if k > v.abs().to_string().len() as u64 {
        return PyInt::ZERO;
    }
    let pow = num_traits::pow(BigInt::from(10), k as usize);
    let (q, r) = v.div_mod_floor(&pow);
    // _PyLong_DivmodNear: nearest quotient, ties to even.
    let twice = &r * 2;
    let q = if twice > pow || (twice == pow && q.is_odd()) { q + 1 } else { q };
    PyInt::from(q * pow)
}
