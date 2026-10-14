//! Builtin functions over numbers: `sum()`, `min()`, `max()`, `round()`
//! (CPython 3.12 `Python/bltinmodule.c`).

use crate::err::{exc, PyResult};
use crate::float::{round_float, round_float_ndigits, round_int_ndigits};
use crate::int::PyInt;
use crate::num::Num;

/// `sum(items, start)` exactly as CPython 3.12 `builtin_sum_impl`.
///
/// While the running total is an int that fits a C `long`, ints are added
/// exactly. The first float switches to a double accumulator: following
/// floats are added with Neumaier compensation, following ints that fit a
/// `long` are added as `(double)value` without compensation, and the
/// compensation is added at the end when it is finite and non-zero. Any
/// other item (an int beyond `long`) ends the fast paths, and generic `+`
/// takes over for the rest.
pub fn py_sum(items: impl IntoIterator<Item = Num>, start: Num) -> PyResult<Num> {
    let mut iter = items.into_iter();
    let mut result = start;

    if let Num::Int(PyInt::Small(mut i_result)) = result {
        loop {
            match iter.next() {
                None => return Ok(Num::Int(PyInt::Small(i_result))),
                Some(Num::Int(PyInt::Small(b))) if i_result.checked_add(b).is_some() => i_result += b,
                Some(item) => {
                    result = Num::Int(PyInt::Small(i_result)).add(&item)?;
                    break;
                }
            }
        }
    }

    if let Num::Float(mut f_result) = result {
        let mut c = 0.0f64;
        loop {
            match iter.next() {
                None => {
                    // Avoid losing the sign on a negative result, and don't
                    // let the compensation turn an infinite sum into a NaN.
                    if c != 0.0 && c.is_finite() {
                        f_result += c;
                    }
                    return Ok(Num::Float(f_result));
                }
                Some(Num::Float(x)) => {
                    let t = f_result + x;
                    if f_result.abs() >= x.abs() {
                        c += (f_result - t) + x;
                    } else {
                        c += (x - t) + f_result;
                    }
                    f_result = t;
                }
                Some(Num::Int(PyInt::Small(value))) => f_result += value as f64,
                Some(item) => {
                    if c != 0.0 && c.is_finite() {
                        f_result += c;
                    }
                    result = Num::Float(f_result).add(&item)?;
                    break;
                }
            }
        }
    }

    for item in iter {
        result = result.add(&item)?;
    }
    Ok(result)
}

/// `sum(items)`: [`py_sum`] with the default start `0`.
pub fn py_sum0(items: impl IntoIterator<Item = Num>) -> PyResult<Num> {
    py_sum(items, Num::Int(PyInt::ZERO))
}

/// `min(items)` with a strict "less than": the first of equal items wins,
/// and an item replaces the current minimum only when `lt(item, current)`
/// (so a NaN compared with anything never replaces nor is replaced).
pub fn py_min_by<T>(items: impl IntoIterator<Item = T>, mut lt: impl FnMut(&T, &T) -> bool) -> PyResult<T> {
    let mut iter = items.into_iter();
    let mut best = iter.next().ok_or_else(|| exc::ValueError("min() iterable argument is empty"))?;
    for item in iter {
        if lt(&item, &best) {
            best = item;
        }
    }
    Ok(best)
}

/// `max(items)`: like [`py_min_by`] with `gt(item, current)`.
pub fn py_max_by<T>(items: impl IntoIterator<Item = T>, mut gt: impl FnMut(&T, &T) -> bool) -> PyResult<T> {
    let mut iter = items.into_iter();
    let mut best = iter.next().ok_or_else(|| exc::ValueError("max() iterable argument is empty"))?;
    for item in iter {
        if gt(&item, &best) {
            best = item;
        }
    }
    Ok(best)
}

/// `min(items)` over numbers.
pub fn py_min(items: impl IntoIterator<Item = Num>) -> PyResult<Num> {
    py_min_by(items, |a, b| a.py_lt(b))
}

/// `max(items)` over numbers.
pub fn py_max(items: impl IntoIterator<Item = Num>) -> PyResult<Num> {
    py_max_by(items, |a, b| a.py_gt(b))
}

/// `min(a, b)` for floats: `b` only when `b < a`.
#[inline]
pub fn min_f64(a: f64, b: f64) -> f64 {
    if b < a {
        b
    } else {
        a
    }
}

/// `max(a, b)` for floats: `b` only when `b > a`.
#[inline]
pub fn max_f64(a: f64, b: f64) -> f64 {
    if b > a {
        b
    } else {
        a
    }
}

/// `round(x)`: an int, half to even.
pub fn py_round(x: &Num) -> PyResult<PyInt> {
    match x {
        Num::Int(i) => Ok(i.clone()),
        Num::Float(f) => round_float(*f),
    }
}

/// `round(x, ndigits)`: same type as `x`.
pub fn py_round_ndigits(x: &Num, ndigits: i64) -> PyResult<Num> {
    match x {
        Num::Int(i) => Ok(Num::Int(round_int_ndigits(i, ndigits))),
        Num::Float(f) => Ok(Num::Float(round_float_ndigits(*f, ndigits)?)),
    }
}
