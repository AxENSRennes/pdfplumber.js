//! CPython 3.12 `Modules/binascii.c`: `unhexlify`, which
//! `pdfminer.ascii85.asciihexdecode` calls.

use crate::err::{exc, PyResult};

// cpython-3.12.3/Modules/binascii.c:887 binascii_a2b_hex_impl
/// `binascii.unhexlify(hexstr)`: raises `binascii.Error` for an odd length
/// (checked first) or a character that is not a hexadecimal digit.
pub fn unhexlify(hexstr: &[u8]) -> PyResult<Vec<u8>> {
    if !hexstr.len().is_multiple_of(2) {
        return Err(exc::BinasciiError("Odd-length string"));
    }
    let digit = |c: u8| (c as char).to_digit(16);
    hexstr
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| match (digit(pair[0]), digit(pair[1])) {
            (Some(top), Some(bot)) => Ok(((top << 4) + bot) as u8),
            _ => Err(exc::BinasciiError("Non-hexadecimal digit found")),
        })
        .collect()
}
