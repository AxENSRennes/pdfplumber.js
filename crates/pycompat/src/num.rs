//! `Num`: a Python number that is either an `int` or a `float`.
//!
//! Python keeps the two types apart, and the difference shows: `1 + 1` is
//! `2` but `1 + 1.0` is `2.0`, `sum()` adds ints exactly, `repr` differs.
//! Binary operations follow CPython's dispatch: two ints use int arithmetic
//! (`long_*`), otherwise the int operand is converted with `float(int)`
//! (which can raise `OverflowError`) and float arithmetic applies
//! (`float_*`). Comparisons between an int and a float are exact.
//!
//! [`NumKey`] is a number used as a `dict` key, with CPython's numeric
//! `hash()` and `==` (`hash(3.0) == hash(3)`, `-0.0 == 0`), so that an
//! `IndexMap<NumKey, V>` behaves as a Python dict with number keys: a lookup
//! with `3` finds the key `3.0`, and assigning to an existing key keeps the
//! key first inserted (and its place), as `dict.__setitem__` does.

use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

use num_bigint::Sign;

use crate::err::PyResult;
use crate::float::{float_div, float_floordiv, float_mod, float_pow};
use crate::int::{cmp_int_float, PyInt};
use crate::repr::float_repr;

// -- hash() -----------------------------------------------------------------
// CPython 3.12 Python/pyhash.c and Objects/longobject.c (long_hash), 64-bit.

const HASH_BITS: u32 = 61;
/// `_PyHASH_MODULUS`: the Mersenne prime 2**61 - 1.
const HASH_MODULUS: u64 = (1 << HASH_BITS) - 1;
/// `_PyHASH_INF`
const HASH_INF: i64 = 314159;

/// `hash(i)` (`long_hash`): `i` modulo 2**61 - 1 with its sign, -1 being
/// replaced by -2.
pub fn int_hash(i: &PyInt) -> i64 {
    let (sign, digits) = match i {
        // |v| mod P directly (performance: no byte vector for the ints
        // every width or cluster lookup hashes; the same value as the
        // loop below)
        PyInt::Small(v) => return signed_hash(*v < 0, v.unsigned_abs() % HASH_MODULUS),
        PyInt::Big(b) => {
            let (sign, bytes) = b.to_bytes_be();
            (sign, bytes)
        }
    };
    // |i| mod P, from the most significant byte (P = 2**61 - 1, so
    // 2**61 = 1 mod P and a multiplication by 256 is a rotation).
    let mut x: u64 = 0;
    for byte in digits {
        x = ((x << 8) & HASH_MODULUS) | (x >> (HASH_BITS - 8));
        x += u64::from(byte);
        if x >= HASH_MODULUS {
            x -= HASH_MODULUS;
        }
    }
    signed_hash(sign == Sign::Minus, x)
}

/// The hash of an int whose magnitude modulo P is `x`: negated for a
/// negative int, -1 being replaced by -2.
fn signed_hash(negative: bool, x: u64) -> i64 {
    let x = if negative { (x as i64).wrapping_neg() } else { x as i64 };
    if x == -1 {
        -2
    } else {
        x
    }
}

/// `hash(x)` for a float (`_Py_HashDouble`): equal to the hash of the
/// number's exact rational value, so equal to `hash(int(x))` when `x` is
/// integral. `None` for a NaN, whose hash is its object's identity.
pub fn float_hash(v: f64) -> Option<i64> {
    if v.is_nan() {
        return None;
    }
    if v.is_infinite() {
        return Some(if v > 0.0 { HASH_INF } else { -HASH_INF });
    }
    let (mut m, mut e) = frexp(v);
    let sign: i64 = if m < 0.0 {
        m = -m;
        -1
    } else {
        1
    };
    // process 28 bits at a time
    let mut x: u64 = 0;
    while m != 0.0 {
        x = ((x << 28) & HASH_MODULUS) | (x >> (HASH_BITS - 28));
        m *= 268435456.0; // 2**28
        e -= 28;
        let y = m as u64; // integer part
        m -= y as f64;
        x += y;
        if x >= HASH_MODULUS {
            x -= HASH_MODULUS;
        }
    }
    // adjust for the exponent; first reduce it modulo _PyHASH_BITS
    let bits = HASH_BITS as i32;
    let e = if e >= 0 { e % bits } else { bits - 1 - ((-1 - e) % bits) } as u32;
    x = ((x << e) & HASH_MODULUS) | (x >> (HASH_BITS - e));
    let x = (x as i64).wrapping_mul(sign);
    Some(if x == -1 { -2 } else { x })
}

/// C `frexp`: `v = m * 2**e` with `0.5 <= |m| < 1` (`m = v`, `e = 0` for 0).
fn frexp(v: f64) -> (f64, i32) {
    if v == 0.0 {
        return (v, 0);
    }
    let bits = v.to_bits();
    let exp = ((bits >> 52) & 0x7ff) as i32;
    if exp == 0 {
        // subnormal: scale into the normal range first
        let (m, e) = frexp(v * f64::from_bits(0x4350_0000_0000_0000)); // 2**54
        return (m, e - 54);
    }
    let m = f64::from_bits((bits & !(0x7ff << 52)) | (1022 << 52));
    (m, exp - 1022)
}

#[derive(Clone, Debug)]
pub enum Num {
    Int(PyInt),
    Float(f64),
}

impl From<PyInt> for Num {
    fn from(v: PyInt) -> Num {
        Num::Int(v)
    }
}

impl From<i64> for Num {
    fn from(v: i64) -> Num {
        Num::Int(PyInt::Small(v))
    }
}

impl From<f64> for Num {
    fn from(v: f64) -> Num {
        Num::Float(v)
    }
}

impl fmt::Display for Num {
    /// `repr(x)` (equal to `str(x)` for numbers).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Num::Int(i) => write!(f, "{i}"),
            Num::Float(x) => f.write_str(&float_repr(*x)),
        }
    }
}

/// Converts both operands to floats in CPython's order (left first).
fn both_f64(a: &Num, b: &Num) -> PyResult<(f64, f64)> {
    Ok((a.to_f64()?, b.to_f64()?))
}

impl Num {
    pub fn is_float(&self) -> bool {
        matches!(self, Num::Float(_))
    }

    /// `float(x)`.
    pub fn to_f64(&self) -> PyResult<f64> {
        match self {
            Num::Int(i) => i.to_f64(),
            Num::Float(x) => Ok(*x),
        }
    }

    /// `a + b`
    pub fn add(&self, other: &Num) -> PyResult<Num> {
        if let (Num::Int(a), Num::Int(b)) = (self, other) {
            return Ok(Num::Int(a.add(b)));
        }
        let (a, b) = both_f64(self, other)?;
        Ok(Num::Float(a + b))
    }

    /// `a - b`
    pub fn sub(&self, other: &Num) -> PyResult<Num> {
        if let (Num::Int(a), Num::Int(b)) = (self, other) {
            return Ok(Num::Int(a.sub(b)));
        }
        let (a, b) = both_f64(self, other)?;
        Ok(Num::Float(a - b))
    }

    /// `a * b`
    pub fn mul(&self, other: &Num) -> PyResult<Num> {
        if let (Num::Int(a), Num::Int(b)) = (self, other) {
            return Ok(Num::Int(a.mul(b)));
        }
        let (a, b) = both_f64(self, other)?;
        Ok(Num::Float(a * b))
    }

    /// `a / b`: always a float.
    pub fn truediv(&self, other: &Num) -> PyResult<Num> {
        if let (Num::Int(a), Num::Int(b)) = (self, other) {
            return Ok(Num::Float(a.truediv(b)?));
        }
        let (a, b) = both_f64(self, other)?;
        Ok(Num::Float(float_div(a, b)?))
    }

    /// `a // b`
    pub fn floordiv(&self, other: &Num) -> PyResult<Num> {
        if let (Num::Int(a), Num::Int(b)) = (self, other) {
            return Ok(Num::Int(a.floordiv(b)?));
        }
        let (a, b) = both_f64(self, other)?;
        Ok(Num::Float(float_floordiv(a, b)?))
    }

    /// `a % b`
    pub fn modulo(&self, other: &Num) -> PyResult<Num> {
        if let (Num::Int(a), Num::Int(b)) = (self, other) {
            return Ok(Num::Int(a.modulo(b)?));
        }
        let (a, b) = both_f64(self, other)?;
        Ok(Num::Float(float_mod(a, b)?))
    }

    /// `a ** b`: an int for an int raised to a non-negative int, else
    /// `float_pow` on both operands converted to float (`long_pow` hands a
    /// negative int exponent to `float_pow` too).
    pub fn pow(&self, other: &Num) -> PyResult<Num> {
        if let (Num::Int(a), Num::Int(b)) = (self, other) {
            if let Some(r) = a.pow(b)? {
                return Ok(Num::Int(r));
            }
        }
        let (a, b) = both_f64(self, other)?;
        Ok(Num::Float(float_pow(a, b)?))
    }

    /// `-a`
    pub fn neg(&self) -> Num {
        match self {
            Num::Int(i) => Num::Int(i.neg()),
            Num::Float(x) => Num::Float(-x),
        }
    }

    /// `abs(a)`
    pub fn abs(&self) -> Num {
        match self {
            Num::Int(i) => Num::Int(i.abs()),
            Num::Float(x) => Num::Float(x.abs()),
        }
    }

    /// Exact ordering of two numbers; `None` when a NaN is involved (every
    /// comparison is then false, except `!=`).
    pub fn py_cmp(&self, other: &Num) -> Option<Ordering> {
        match (self, other) {
            (Num::Int(a), Num::Int(b)) => Some(a.cmp(b)),
            (Num::Float(a), Num::Float(b)) => a.partial_cmp(b),
            (Num::Int(a), Num::Float(b)) => cmp_int_float(a, *b),
            (Num::Float(a), Num::Int(b)) => cmp_int_float(b, *a).map(Ordering::reverse),
        }
    }

    /// `a == b`
    pub fn py_eq(&self, other: &Num) -> bool {
        self.py_cmp(other) == Some(Ordering::Equal)
    }

    /// `a < b`
    pub fn py_lt(&self, other: &Num) -> bool {
        self.py_cmp(other) == Some(Ordering::Less)
    }

    /// `a <= b`
    pub fn py_le(&self, other: &Num) -> bool {
        matches!(self.py_cmp(other), Some(Ordering::Less | Ordering::Equal))
    }

    /// `a > b`
    pub fn py_gt(&self, other: &Num) -> bool {
        self.py_cmp(other) == Some(Ordering::Greater)
    }

    /// `a >= b`
    pub fn py_ge(&self, other: &Num) -> bool {
        matches!(self.py_cmp(other), Some(Ordering::Greater | Ordering::Equal))
    }

    /// `hash(a)`; `None` for a NaN (its hash is its identity, see
    /// [`NumKey`]).
    pub fn py_hash(&self) -> Option<i64> {
        match self {
            Num::Int(i) => Some(int_hash(i)),
            Num::Float(x) => float_hash(*x),
        }
    }
}

/// A number as a `dict` (or `set`) key, compared and hashed as CPython
/// does (see the module documentation). The key keeps the number it was made
/// from (`3.0` stays a float in a `repr` of the dict).
///
/// A NaN is only equal to itself in Python, by identity: `{nan: 1}[nan]`
/// finds the entry only with the same object. Each `NumKey` made from a NaN
/// is therefore a new object, equal to its clones only.
#[derive(Clone, Debug)]
pub struct NumKey {
    num: Num,
    /// The identity of a NaN key (0 for any other number).
    nan_id: u64,
}

impl NumKey {
    pub fn new(num: Num) -> NumKey {
        static NEXT_NAN: AtomicU64 = AtomicU64::new(1);
        let nan_id = match num {
            Num::Float(x) if x.is_nan() => NEXT_NAN.fetch_add(1, AtomicOrdering::Relaxed),
            _ => 0,
        };
        NumKey { num, nan_id }
    }

    /// The number, as it was given.
    pub fn num(&self) -> &Num {
        &self.num
    }
}

impl From<Num> for NumKey {
    fn from(num: Num) -> NumKey {
        NumKey::new(num)
    }
}

impl PartialEq for NumKey {
    fn eq(&self, other: &NumKey) -> bool {
        if self.nan_id != 0 || other.nan_id != 0 {
            return self.nan_id == other.nan_id;
        }
        self.num.py_eq(&other.num)
    }
}

impl Eq for NumKey {}

impl Hash for NumKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self.num.py_hash() {
            Some(h) => state.write_i64(h),
            None => state.write_u64(self.nan_id),
        }
    }
}

impl fmt::Display for NumKey {
    /// `repr(key)`
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.num.fmt(f)
    }
}
