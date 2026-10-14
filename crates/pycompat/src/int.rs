//! Python `int`: exact integers of any size.
//!
//! [`PyInt`] stores an `i64` when the value fits, and falls back to a
//! `BigInt` otherwise (the representation is normalized, so equal values have
//! equal representations). Arithmetic follows CPython 3.12
//! (`Objects/longobject.c`): floor division and modulo round toward negative
//! infinity, true division and `float(int)` are correctly rounded and raise
//! `OverflowError` beyond the float range, and `int()` parsing implements
//! `PyLong_FromString` with the 4 300-digit limit of
//! `sys.int_info.default_max_str_digits`.

use std::cmp::Ordering;
use std::fmt;

use num_bigint::{BigInt, BigUint, Sign};
use num_integer::Integer;
use num_traits::{One, Signed, ToPrimitive, Zero};

use crate::err::{exc, PyResult};
use crate::repr::{bytes_repr, str_repr};
use crate::text;

/// `sys.int_info.default_max_str_digits`.
pub const MAX_STR_DIGITS: usize = 4300;

/// A Python `int`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum PyInt {
    Small(i64),
    /// Only values outside the `i64` range.
    Big(Box<BigInt>),
}

impl From<i64> for PyInt {
    fn from(v: i64) -> PyInt {
        PyInt::Small(v)
    }
}

impl From<i32> for PyInt {
    fn from(v: i32) -> PyInt {
        PyInt::Small(v.into())
    }
}

impl From<u32> for PyInt {
    fn from(v: u32) -> PyInt {
        PyInt::Small(v.into())
    }
}

impl From<usize> for PyInt {
    fn from(v: usize) -> PyInt {
        match i64::try_from(v) {
            Ok(v) => PyInt::Small(v),
            Err(_) => PyInt::from(BigInt::from(v)),
        }
    }
}

impl From<u64> for PyInt {
    fn from(v: u64) -> PyInt {
        match i64::try_from(v) {
            Ok(v) => PyInt::Small(v),
            Err(_) => PyInt::from(BigInt::from(v)),
        }
    }
}

impl From<i128> for PyInt {
    fn from(v: i128) -> PyInt {
        match i64::try_from(v) {
            Ok(v) => PyInt::Small(v),
            Err(_) => PyInt::from(BigInt::from(v)),
        }
    }
}

impl From<BigInt> for PyInt {
    fn from(v: BigInt) -> PyInt {
        match v.to_i64() {
            Some(small) => PyInt::Small(small),
            None => PyInt::Big(Box::new(v)),
        }
    }
}

impl Ord for PyInt {
    fn cmp(&self, other: &PyInt) -> Ordering {
        match (self, other) {
            (PyInt::Small(a), PyInt::Small(b)) => a.cmp(b),
            // A Big value is outside the i64 range: its sign decides.
            (PyInt::Small(_), PyInt::Big(b)) => {
                if b.is_negative() {
                    Ordering::Greater
                } else {
                    Ordering::Less
                }
            }
            (PyInt::Big(a), PyInt::Small(_)) => {
                if a.is_negative() {
                    Ordering::Less
                } else {
                    Ordering::Greater
                }
            }
            (PyInt::Big(a), PyInt::Big(b)) => a.cmp(b),
        }
    }
}

impl PartialOrd for PyInt {
    fn partial_cmp(&self, other: &PyInt) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for PyInt {
    /// Decimal digits, without the `str()` digit limit (see [`PyInt::py_str`]).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PyInt::Small(v) => write!(f, "{v}"),
            PyInt::Big(v) => write!(f, "{v}"),
        }
    }
}

impl PyInt {
    pub const ZERO: PyInt = PyInt::Small(0);

    pub fn to_bigint(&self) -> BigInt {
        match self {
            PyInt::Small(v) => BigInt::from(*v),
            PyInt::Big(v) => (**v).clone(),
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            PyInt::Small(v) => Some(*v),
            PyInt::Big(_) => None,
        }
    }

    pub fn is_zero(&self) -> bool {
        matches!(self, PyInt::Small(0))
    }

    pub fn is_negative(&self) -> bool {
        match self {
            PyInt::Small(v) => *v < 0,
            PyInt::Big(v) => v.is_negative(),
        }
    }

    /// Reads `-?[0-9]+` without `int()`'s digit limit (for data exchange,
    /// not a Python operation).
    pub fn from_decimal(text: &str) -> Option<PyInt> {
        let digits = text.strip_prefix('-').unwrap_or(text);
        if digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        text.parse::<i64>().ok().map(PyInt::Small).or_else(|| text.parse::<BigInt>().ok().map(PyInt::from))
    }

    /// `str(i)`: `ValueError` above 4 300 decimal digits
    /// (`long_to_decimal_string_internal`).
    pub fn py_str(&self) -> PyResult<String> {
        let s = self.to_string();
        let ndigits = s.len() - usize::from(self.is_negative());
        if ndigits > MAX_STR_DIGITS {
            return Err(exc::ValueError(format!(
                "Exceeds the limit ({MAX_STR_DIGITS} digits) for integer string conversion; \
                 use sys.set_int_max_str_digits() to increase the limit"
            )));
        }
        Ok(s)
    }

    // -- arithmetic (Objects/longobject.c) --------------------------------

    /// `a + b`
    pub fn add(&self, other: &PyInt) -> PyInt {
        if let (PyInt::Small(a), PyInt::Small(b)) = (self, other) {
            if let Some(r) = a.checked_add(*b) {
                return PyInt::Small(r);
            }
        }
        PyInt::from(self.to_bigint() + other.to_bigint())
    }

    /// `a - b`
    pub fn sub(&self, other: &PyInt) -> PyInt {
        if let (PyInt::Small(a), PyInt::Small(b)) = (self, other) {
            if let Some(r) = a.checked_sub(*b) {
                return PyInt::Small(r);
            }
        }
        PyInt::from(self.to_bigint() - other.to_bigint())
    }

    /// `a * b`
    pub fn mul(&self, other: &PyInt) -> PyInt {
        if let (PyInt::Small(a), PyInt::Small(b)) = (self, other) {
            if let Some(r) = a.checked_mul(*b) {
                return PyInt::Small(r);
            }
        }
        PyInt::from(self.to_bigint() * other.to_bigint())
    }

    /// `-a`
    pub fn neg(&self) -> PyInt {
        match self {
            PyInt::Small(v) => match v.checked_neg() {
                Some(r) => PyInt::Small(r),
                None => PyInt::from(-BigInt::from(*v)),
            },
            PyInt::Big(v) => PyInt::from(-(**v).clone()),
        }
    }

    /// `abs(a)`
    pub fn abs(&self) -> PyInt {
        if self.is_negative() {
            self.neg()
        } else {
            self.clone()
        }
    }

    /// `a // b` (`long_div`): rounds toward negative infinity.
    pub fn floordiv(&self, other: &PyInt) -> PyResult<PyInt> {
        if other.is_zero() {
            return Err(exc::ZeroDivisionError("integer division or modulo by zero"));
        }
        if let (PyInt::Small(a), PyInt::Small(b)) = (self, other) {
            if let (Some(q), Some(r)) = (a.checked_div(*b), a.checked_rem(*b)) {
                // Truncated quotient, moved down when the signs differ.
                return Ok(PyInt::Small(if r != 0 && (r < 0) != (*b < 0) { q - 1 } else { q }));
            }
        }
        Ok(PyInt::from(self.to_bigint().div_floor(&other.to_bigint())))
    }

    /// `a % b` (`long_mod`): the result has the sign of `b`.
    pub fn modulo(&self, other: &PyInt) -> PyResult<PyInt> {
        if other.is_zero() {
            return Err(exc::ZeroDivisionError("integer modulo by zero"));
        }
        if let (PyInt::Small(a), PyInt::Small(b)) = (self, other) {
            if let Some(r) = a.checked_rem(*b) {
                return Ok(PyInt::Small(if r != 0 && (r < 0) != (*b < 0) { r + b } else { r }));
            }
        }
        Ok(PyInt::from(self.to_bigint().mod_floor(&other.to_bigint())))
    }

    /// `a ** b` for `b >= 0` (`long_pow` without modulus); `Ok(None)` when
    /// `b` is negative (Python then computes a float power, see `Num::pow`).
    ///
    /// For an exponent beyond `u32` with a base other than 0, 1 and -1,
    /// Python computes until memory or time runs out; the port raises
    /// `MemoryError` at once.
    pub fn pow(&self, exp: &PyInt) -> PyResult<Option<PyInt>> {
        if exp.is_negative() {
            return Ok(None);
        }
        let base = self.to_bigint();
        if base.is_zero() || base.is_one() {
            return Ok(Some(if exp.is_zero() { PyInt::Small(1) } else { self.clone() }));
        }
        if base == BigInt::from(-1) {
            let odd = match exp {
                PyInt::Small(e) => e % 2 == 1,
                PyInt::Big(e) => e.is_odd(),
            };
            return Ok(Some(PyInt::Small(if odd { -1 } else { 1 })));
        }
        match exp.as_i64().and_then(|e| u32::try_from(e).ok()) {
            Some(e) => Ok(Some(PyInt::from(num_traits::Pow::pow(base, e)))),
            None => Err(exc::MemoryError("")),
        }
    }

    /// `float(a)` (`PyLong_AsDouble`): correctly rounded, half to even.
    pub fn to_f64(&self) -> PyResult<f64> {
        match self {
            // `as` rounds to nearest, ties to even, like PyLong_AsDouble.
            PyInt::Small(v) => Ok(*v as f64),
            PyInt::Big(v) => {
                let (sign, mag) = (v.sign(), v.magnitude());
                match ratio_to_f64(mag, &BigUint::one()) {
                    Some(f) => Ok(if sign == Sign::Minus { -f } else { f }),
                    None => Err(exc::OverflowError("int too large to convert to float")),
                }
            }
        }
    }

    /// `a / b` (`long_true_divide`): the correctly rounded quotient.
    pub fn truediv(&self, other: &PyInt) -> PyResult<f64> {
        if other.is_zero() {
            return Err(exc::ZeroDivisionError("division by zero"));
        }
        let negate = self.is_negative() != other.is_negative();
        let (a, b) = (self.to_bigint(), other.to_bigint());
        match ratio_to_f64(a.magnitude(), b.magnitude()) {
            Some(f) => Ok(if negate { -f } else { f }),
            None => Err(exc::OverflowError("integer division result too large for a float")),
        }
    }

    // -- int() parsing (PyLong_FromString) --------------------------------

    /// `int(b, base)` for a `bytes` argument (`_PyLong_FromBytes`).
    #[inline]
    pub fn parse_bytes(s: &[u8], base: u32) -> PyResult<PyInt> {
        // Fast path: an optional sign and 1 to 18 decimal digits (always an
        // i64), which is all a content stream holds in practice.
        if base == 10 {
            let (negative, digits) = match s.first() {
                Some(b'-') => (true, &s[1..]),
                Some(b'+') => (false, &s[1..]),
                _ => (false, s),
            };
            if (1..=18).contains(&digits.len()) && digits.iter().all(u8::is_ascii_digit) {
                let v = digits.iter().fold(0i64, |acc, &c| acc * 10 + i64::from(c - b'0'));
                return Ok(PyInt::Small(if negative { -v } else { v }));
            }
        }
        PyInt::parse_bytes_general(s, base)
    }

    fn parse_bytes_general(s: &[u8], base: u32) -> PyResult<PyInt> {
        parse_ascii(s, base).map_err(|e| match e {
            ParseError::Syntax => exc::ValueError(format!(
                "invalid literal for int() with base {base}: {}",
                truncate_chars(&bytes_repr(&s[..s.len().min(200)]), 200)
            )),
            ParseError::Other(err) => err,
        })
    }

    /// `int(s, base)` for a `str` argument (`PyLong_FromUnicodeObject`):
    /// Unicode decimal digits and whitespace count as their ASCII forms.
    pub fn parse_str(s: &str, base: u32) -> PyResult<PyInt> {
        let ascii = text::transform_decimal_and_space_to_ascii(s);
        parse_ascii(&ascii, base).map_err(|e| match e {
            ParseError::Syntax => exc::ValueError(format!(
                "invalid literal for int() with base {base}: {}",
                truncate_chars(&str_repr(s), 200)
            )),
            ParseError::Other(err) => err,
        })
    }
}

/// `%.200R`: the repr cut to its first 200 characters.
fn truncate_chars(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

enum ParseError {
    /// "invalid literal": the caller formats the message with its argument.
    Syntax,
    Other(crate::err::PyErr),
}

/// `_PyLong_DigitValue`: digit value of an ASCII byte, 37 when not a digit.
fn digit_value(c: u8) -> u32 {
    match c {
        b'0'..=b'9' => u32::from(c - b'0'),
        b'a'..=b'z' => u32::from(c - b'a') + 10,
        b'A'..=b'Z' => u32::from(c - b'A') + 10,
        _ => 37,
    }
}

/// `PyLong_FromString` + `long_from_string_base` on an ASCII buffer.
fn parse_ascii(s: &[u8], base: u32) -> Result<PyInt, ParseError> {
    if (base != 0 && base < 2) || base > 36 {
        return Err(ParseError::Other(exc::ValueError("int() base must be >= 2 and <= 36, or 0")));
    }
    let at = |i: usize| s.get(i).copied().unwrap_or(0);
    let mut p = 0;
    while p < s.len() && text::bytes_isspace(s[p]) {
        p += 1;
    }
    let mut negative = false;
    if at(p) == b'+' {
        p += 1;
    } else if at(p) == b'-' {
        p += 1;
        negative = true;
    }
    let mut base = base;
    let mut error_if_nonzero = false;
    if base == 0 {
        base = if at(p) != b'0' {
            10
        } else {
            match at(p + 1) {
                b'x' | b'X' => 16,
                b'o' | b'O' => 8,
                b'b' | b'B' => 2,
                _ => {
                    // "old" C-style octal literal: only zero is valid.
                    error_if_nonzero = true;
                    10
                }
            }
        };
    }
    if at(p) == b'0'
        && ((base == 16 && matches!(at(p + 1), b'x' | b'X'))
            || (base == 8 && matches!(at(p + 1), b'o' | b'O'))
            || (base == 2 && matches!(at(p + 1), b'b' | b'B')))
    {
        p += 2;
        if at(p) == b'_' {
            p += 1;
        }
    }

    // long_from_string_base
    let start = p;
    if at(p) == b'_' {
        return Err(ParseError::Syntax);
    }
    let mut digits = 0usize;
    let mut prev = 0u8;
    while p < s.len() && (digit_value(s[p]) < base || s[p] == b'_') {
        if s[p] == b'_' {
            if prev == b'_' {
                return Err(ParseError::Syntax);
            }
        } else {
            digits += 1;
        }
        prev = s[p];
        p += 1;
    }
    if prev == b'_' {
        return Err(ParseError::Syntax);
    }
    let end = p;
    if start == end {
        return Err(ParseError::Syntax);
    }
    while p < s.len() && text::bytes_isspace(s[p]) {
        p += 1;
    }
    if p != s.len() {
        return Err(ParseError::Syntax);
    }
    let is_binary_base = base & (base - 1) == 0;
    if !is_binary_base && digits > MAX_STR_DIGITS {
        return Err(ParseError::Other(exc::ValueError(format!(
            "Exceeds the limit ({MAX_STR_DIGITS} digits) for integer string conversion: \
             value has {digits} digits; use sys.set_int_max_str_digits() to increase the limit"
        ))));
    }

    let body = &s[start..end];
    let value = if digits <= 18 && base == 10 {
        // At most 18 decimal digits always fit in an i64.
        let mut v: i64 = 0;
        for &c in body.iter().filter(|&&c| c != b'_') {
            v = v * 10 + i64::from(c - b'0');
        }
        PyInt::Small(if negative { -v } else { v })
    } else {
        let clean: Vec<u8> = body.iter().copied().filter(|&c| c != b'_').collect();
        let mag = BigInt::parse_bytes(&clean, base).expect("validated digits");
        PyInt::from(if negative { -mag } else { mag })
    };
    if error_if_nonzero && !value.is_zero() {
        return Err(ParseError::Syntax);
    }
    Ok(value)
}

/// Number of significant bits of a non-zero magnitude.
fn bits(x: &BigUint) -> i64 {
    x.bits() as i64
}

/// `2.0 ** e` for `-1074 <= e <= 1023`, built from its bit pattern.
fn pow2(e: i64) -> f64 {
    debug_assert!((-1074..=1023).contains(&e));
    if e >= -1022 {
        f64::from_bits(((e + 1023) as u64) << 52)
    } else {
        f64::from_bits(1u64 << (e + 1074))
    }
}

/// The correctly rounded (half to even) double nearest to `n / d`, with
/// gradual underflow; `None` when it overflows the double range.
///
/// This is the result `long_true_divide` and `PyLong_AsDouble` produce
/// (both are correctly rounded, so any correct algorithm agrees with them).
pub(crate) fn ratio_to_f64(n: &BigUint, d: &BigUint) -> Option<f64> {
    debug_assert!(!d.is_zero());
    if n.is_zero() {
        return Some(0.0);
    }
    // Scale so that the integer quotient q = n / (d * 2**e) has 55 or 56
    // bits: two guard bits beyond the 53-bit significand plus a sticky
    // remainder decide the rounding exactly.
    let e = bits(n) - bits(d) - 55;
    let scale = e.unsigned_abs() as usize;
    let (q, r) = if e >= 0 { n.div_rem(&(d << scale)) } else { (n << scale).div_rem(d) };
    // Binary exponent of the quotient and weight of its last kept bit.
    let top = bits(&q) - 1 + e;
    let lsb = (top - 52).max(-1074);
    let shift = usize::try_from(lsb - e).expect("q has at least 55 bits, so shift >= 2");
    let mut m = (&q >> shift).to_u64().expect("at most 54 bits");
    let rest = &q - (BigUint::from(m) << shift);
    let half = BigUint::one() << (shift - 1);
    let round_up = match rest.cmp(&half) {
        Ordering::Greater => true,
        Ordering::Equal => !r.is_zero() || m & 1 == 1,
        Ordering::Less => false,
    };
    if round_up {
        m += 1;
    }
    if m == 0 {
        return Some(0.0);
    }
    // m * 2**lsb, exactly representable by construction.
    let m_bits = 64 - i64::from(m.leading_zeros());
    if m_bits - 1 + lsb > 1023 {
        return None;
    }
    // Split the scaling so that each factor is a representable power of two.
    if lsb > 1023 - 53 {
        Some(m as f64 * pow2(lsb - 53) * pow2(53))
    } else {
        Some(m as f64 * pow2(lsb))
    }
}

/// Exact integer value of a finite, integral double.
pub(crate) fn bigint_from_integral_f64(x: f64) -> BigInt {
    debug_assert!(x.is_finite() && x.fract() == 0.0);
    let bits = x.to_bits();
    let exp = ((bits >> 52) & 0x7ff) as i64;
    let frac = bits & ((1u64 << 52) - 1);
    let (mant, e) = if exp == 0 { (frac, -1074) } else { (frac | (1u64 << 52), exp - 1075) };
    let shift = e.unsigned_abs(); // at most 1074
    let mag =
        if e >= 0 { BigInt::from(mant) << shift } else { BigInt::from(mant.checked_shr(shift as u32).unwrap_or(0)) };
    if x < 0.0 {
        -mag
    } else {
        mag
    }
}

/// Exact comparison of an int with a float (`float_richcompare`); `None`
/// when `f` is NaN.
pub fn cmp_int_float(i: &PyInt, f: f64) -> Option<Ordering> {
    if f.is_nan() {
        return None;
    }
    if f.is_infinite() {
        return Some(if f > 0.0 { Ordering::Less } else { Ordering::Greater });
    }
    if let PyInt::Small(v) = i {
        // Both sides are exact doubles when |v| <= 2**53.
        if v.unsigned_abs() <= 1 << 53 {
            return (*v as f64).partial_cmp(&f);
        }
    }
    let floor = f.floor();
    match i.to_bigint().cmp(&bigint_from_integral_f64(floor)) {
        Ordering::Equal if f > floor => Some(Ordering::Less),
        ord => Some(ord),
    }
}
