//! `struct.unpack` and `io.BytesIO` as the font program parsers of
//! `pdffont` use them (CPython 3.12 `Modules/_struct.c`,
//! `Modules/_io/bytesio.c`).

use pycompat::io::BytesIO;
use pycompat::{exc, PyInt, PyResult};

/// `fp.read(n)` for `struct.unpack` of a format of `n` bytes: the bytes, or
/// `struct.error` when fewer were left (the short read still moves the
/// position).
pub fn read_n(fp: &mut BytesIO, n: usize) -> PyResult<&[u8]> {
    let data = fp.read_bytes(Some(n));
    if data.len() != n {
        return Err(unpack_error(n as u64));
    }
    Ok(data)
}

/// `fp.read(n)` for a format of `n` bytes where `n` may exceed the address
/// space (`f">{count}H"` with a 32-bit count): the read stops at the end of
/// the data anyway.
pub fn read_n_u64(fp: &mut BytesIO, n: u64) -> PyResult<&[u8]> {
    let data = fp.read_bytes(Some(usize::try_from(n).unwrap_or(usize::MAX)));
    if data.len() as u64 != n {
        return Err(unpack_error(n));
    }
    Ok(data)
}

/// `struct.error("unpack requires a buffer of N bytes")`.
pub fn unpack_error(n: u64) -> pycompat::PyErr {
    exc::StructError(format!("unpack requires a buffer of {n} bytes"))
}

/// A big-endian unsigned integer of `width` bytes (`>H`, `>L`...).
pub fn unpack_be(b: &[u8], width: usize) -> i64 {
    b[..width].iter().fold(0i64, |acc, &x| (acc << 8) | i64::from(x))
}

/// A big-endian signed 16-bit integer (`>h`).
pub fn unpack_be_i16(b: &[u8]) -> i64 {
    i64::from(i16::from_be_bytes([b[0], b[1]]))
}

/// `fp.seek(pos)` for a Python int: `ValueError` when negative,
/// `OverflowError` beyond `Py_ssize_t`.
pub fn seek_int(fp: &mut BytesIO, pos: &PyInt) -> PyResult<()> {
    let Some(pos) = pos.as_i64() else {
        return Err(exc::OverflowError("Python int too large to convert to C ssize_t"));
    };
    fp.seek(pos)?;
    Ok(())
}

/// The size argument of `fp.read(size)` for a Python int: a negative size
/// reads to the end (`None`), one beyond `Py_ssize_t` raises
/// `OverflowError`.
pub fn read_size(size: &PyInt) -> PyResult<Option<usize>> {
    let Some(size) = size.as_i64() else {
        return Err(exc::OverflowError("Python int too large to convert to C ssize_t"));
    };
    Ok(usize::try_from(size).ok())
}
