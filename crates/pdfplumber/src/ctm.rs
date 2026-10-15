//! Port of `pdfplumber/ctm.py`: the decomposition of a transformation
//! matrix (PDF Reference, 6th ed., section 4.2.2).
//!
//! `CTM` is a `NamedTuple` of six numbers (pdfplumber builds it from a
//! char's `matrix`, whose entries may be ints); the properties compute with
//! Python's arithmetic:
//!
//! - `scale_x`/`scale_y`: `math.sqrt(pow(a, 2) + pow(b, 2))`. `pow` is
//!   Python's (`int ** 2` stays an int; a float goes through the oracle's
//!   C `pow`, `pycompat::libm`), `math.sqrt` is correctly rounded on every
//!   target (IEEE-754 square root).
//! - `skew_x`/`skew_y`: `math.atan2`, CPython's `m_atan2` (its own handling
//!   of NaNs, infinities and zeros) and then the C library's `atan2`. No
//!   port of glibc's `atan2` exists in `pycompat::libm` yet, so the finite
//!   case calls Rust's `f64::atan2`: the C library's function natively
//!   (glibc on Linux: the oracle's results), but musl's algorithm on
//!   WebAssembly (`compiler-builtins`' `libm` and WASI's libc), which
//!   differs from glibc in the last bit for about 17 % of random inputs
//!   (measured on 10^6 pairs). That is the risk until `atan2` joins
//!   `pycompat::libm`; pdfplumber itself never calls these properties.

use pycompat::{exc, Num, PyInt, PyResult};

/// pdfplumber/ctm.py:8 `CTM`.
#[derive(Clone, Debug)]
pub struct CTM {
    pub a: Num,
    pub b: Num,
    pub c: Num,
    pub d: Num,
    pub e: Num,
    pub f: Num,
}

/// `math.sqrt(x)`: `float(x)`, then the square root (`ValueError` below 0).
fn math_sqrt(x: &Num) -> PyResult<f64> {
    let x = x.to_f64()?;
    if x < 0.0 {
        return Err(exc::ValueError("math domain error"));
    }
    Ok(x.sqrt())
}

/// `math.atan2(y, x)` (`m_atan2` of CPython's `mathmodule.c`).
fn math_atan2(y: &Num, x: &Num) -> PyResult<f64> {
    let (y, x) = (y.to_f64()?, x.to_f64()?);
    let pi = std::f64::consts::PI;
    if x.is_nan() || y.is_nan() {
        return Ok(f64::NAN);
    }
    if y.is_infinite() {
        if x.is_infinite() {
            return Ok(if x.is_sign_positive() {
                // atan2(+-inf, +inf) == +-pi/4
                (0.25 * pi).copysign(y)
            } else {
                // atan2(+-inf, -inf) == +-pi*3/4
                (0.75 * pi).copysign(y)
            });
        }
        // atan2(+-inf, x) == +-pi/2 for finite x
        return Ok((0.5 * pi).copysign(y));
    }
    if x.is_infinite() || y == 0.0 {
        return Ok(if x.is_sign_positive() {
            // atan2(+-y, +inf) = atan2(+-0, +x) = +-0.
            0.0f64.copysign(y)
        } else {
            // atan2(+-y, -inf) = atan2(+-0., -x) = +-pi.
            pi.copysign(y)
        });
    }
    Ok(y.atan2(x))
}

/// `pow(v, 2)`
fn square(v: &Num) -> PyResult<Num> {
    v.pow(&Num::Int(PyInt::Small(2)))
}

impl CTM {
    pub fn new(a: Num, b: Num, c: Num, d: Num, e: Num, f: Num) -> CTM {
        CTM { a, b, c, d, e, f }
    }

    // pdfplumber/ctm.py:17 CTM.scale_x
    pub fn scale_x(&self) -> PyResult<f64> {
        math_sqrt(&square(&self.a)?.add(&square(&self.b)?)?)
    }

    // pdfplumber/ctm.py:21 CTM.scale_y
    pub fn scale_y(&self) -> PyResult<f64> {
        math_sqrt(&square(&self.c)?.add(&square(&self.d)?)?)
    }

    // pdfplumber/ctm.py:25 CTM.skew_x
    pub fn skew_x(&self) -> PyResult<f64> {
        Ok((math_atan2(&self.d, &self.c)? * 180.0 / std::f64::consts::PI) - 90.0)
    }

    // pdfplumber/ctm.py:29 CTM.skew_y
    pub fn skew_y(&self) -> PyResult<f64> {
        Ok(math_atan2(&self.b, &self.a)? * 180.0 / std::f64::consts::PI)
    }

    // pdfplumber/ctm.py:33 CTM.translation_x
    pub fn translation_x(&self) -> Num {
        self.e.clone()
    }

    // pdfplumber/ctm.py:37 CTM.translation_y
    pub fn translation_y(&self) -> Num {
        self.f.clone()
    }
}
