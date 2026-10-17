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
//! - `skew_x`/`skew_y`: `math.atan2`, CPython's `m_atan2` and then the C
//!   library's `atan2`, both in `pycompat::libm` (a port of the oracle's
//!   glibc `atan2`: the same bits on every target, WebAssembly included).

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

/// `math.atan2(y, x)`: `float()` of each argument, then `m_atan2`.
fn math_atan2(y: &Num, x: &Num) -> PyResult<f64> {
    let (y, x) = (y.to_f64()?, x.to_f64()?);
    Ok(pycompat::libm::math_atan2(y, x))
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
