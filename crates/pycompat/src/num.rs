//! `Num`: a Python number that is either an `int` or a `float`.
//!
//! Python keeps the two types apart, and the difference shows: `1 + 1` is
//! `2` but `1 + 1.0` is `2.0`, `sum()` adds ints exactly, `repr` differs.
//! Binary operations follow CPython's dispatch: two ints use int arithmetic
//! (`long_*`), otherwise the int operand is converted with `float(int)`
//! (which can raise `OverflowError`) and float arithmetic applies
//! (`float_*`). Comparisons between an int and a float are exact.

use std::cmp::Ordering;
use std::fmt;

use crate::err::PyResult;
use crate::float::{float_div, float_floordiv, float_mod, float_pow};
use crate::int::{cmp_int_float, PyInt};
use crate::repr::float_repr;

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
}
