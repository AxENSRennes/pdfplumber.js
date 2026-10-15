//! Port of `pdfminer/arcfour.py`: the RC4 stream cipher.
//!
//! Each method takes the Python frame of its upstream method (a security
//! handler decrypts from `decipher_all`, whose recursion can run close to
//! the frame limit); see `docs/architecture.md`, "Profondeur de récursion".

use pycompat::recursion;
use pycompat::{exc, PyResult};

/// pdfminer/arcfour.py:10 `Arcfour`
#[derive(Clone)]
pub struct Arcfour {
    s: [u8; 256],
    i: u8,
    j: u8,
}

impl Arcfour {
    // pdfminer/arcfour.py:11 Arcfour.__init__
    /// `Arcfour(key)`. An empty key raises `ZeroDivisionError` (`i % klen`).
    pub fn new(key: &[u8]) -> PyResult<Arcfour> {
        let _frame = recursion::py_frame()?;
        if key.is_empty() {
            return Err(exc::ZeroDivisionError("integer modulo by zero"));
        }
        // s = list(range(256)): every entry stays in 0..=255 (a permutation)
        let mut s = [0u8; 256];
        for (i, x) in s.iter_mut().enumerate() {
            *x = i as u8;
        }
        let mut j: u8 = 0;
        for i in 0..256 {
            // j = (j + s[i] + key[i % klen]) % 256
            j = j.wrapping_add(s[i]).wrapping_add(key[i % key.len()]);
            s.swap(i, usize::from(j));
        }
        Ok(Arcfour { s, i: 0, j: 0 })
    }

    // pdfminer/arcfour.py:22 Arcfour.process
    /// Encrypts or decrypts `data`, continuing the key stream of earlier
    /// calls (`encrypt` and `decrypt` are this method).
    pub fn process(&mut self, data: &[u8]) -> PyResult<Vec<u8>> {
        let _frame = recursion::py_frame()?;
        let (mut i, mut j) = (self.i, self.j);
        let s = &mut self.s;
        let mut r = Vec::with_capacity(data.len());
        for &c in data {
            i = i.wrapping_add(1);
            j = j.wrapping_add(s[usize::from(i)]);
            s.swap(usize::from(i), usize::from(j));
            let k = s[usize::from(s[usize::from(i)].wrapping_add(s[usize::from(j)]))];
            r.push(c ^ k);
        }
        (self.i, self.j) = (i, j);
        Ok(r)
    }

    /// `Arcfour.encrypt` (the same function as `process`).
    pub fn encrypt(&mut self, data: &[u8]) -> PyResult<Vec<u8>> {
        self.process(data)
    }

    /// `Arcfour.decrypt` (the same function as `process`).
    pub fn decrypt(&mut self, data: &[u8]) -> PyResult<Vec<u8>> {
        self.process(data)
    }
}
