//! Whether CPython's allocation of an object of a given size succeeds, for
//! the code paths where pdfminer asks for memory sized by a value of the
//! file (`BufferedReader.read(n)`, `bytes * n`): CPython raises
//! `MemoryError` when `malloc` fails, so the port asks for the same amount
//! ([`probe`]) and raises it in the same cases, never aborting.
//!
//! Natively the memory is requested and given back untouched (virtual
//! memory: no page is used, as CPython's object is not filled before the
//! allocation succeeds). On wasm32, growing the memory cannot be undone,
//! so only a size beyond the address space (`isize::MAX`) fails there; in
//! between, the limit is the machine's, in Python as in the port.

use crate::err::{exc, PyResult};

/// `MemoryError` unless `bytes` bytes could be allocated now.
pub fn probe(bytes: u128) -> PyResult<()> {
    let n = usize::try_from(bytes).ok().filter(|&n| n <= isize::MAX as usize).ok_or_else(|| exc::MemoryError(""))?;
    if cfg!(target_family = "wasm") {
        return Ok(());
    }
    let mut probe: Vec<u8> = Vec::new();
    probe.try_reserve_exact(n).map_err(|_| exc::MemoryError(""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_beyond_any_machine_fail() {
        assert!(probe(1 << 20).is_ok());
        assert!(probe(u128::from(u64::MAX)).is_err());
        assert!(probe((isize::MAX as u128) + 1).is_err());
    }
}
