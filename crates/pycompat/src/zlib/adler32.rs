//! zlib 1.3 `adler32.c`: the Adler-32 checksum of the zlib container.
//!
//! The checksum has a single result for a given input; the port keeps
//! zlib's structure (two component sums, one modulo per `NMAX` bytes)
//! without its manual unrolling.

/// Largest prime smaller than 65536.
const BASE: u64 = 65521;
/// NMAX is the largest n such that 255n(n+1)/2 + (n+1)(BASE-1) <= 2^32-1.
const NMAX: usize = 5552;

// zlib-1.3/adler32.c:59 adler32_z
/// `adler32(adler, buf, len)`: `adler` updated with the bytes of `buf`
/// (the initial value, `adler32(0L, Z_NULL, 0)`, is 1).
pub fn adler32(adler: u64, buf: &[u8]) -> u64 {
    // split Adler-32 into component sums
    let mut sum2 = (adler >> 16) & 0xffff;
    let mut adler = adler & 0xffff;
    // do length NMAX blocks -- requires just one modulo operation
    for block in buf.chunks(NMAX) {
        for &b in block {
            adler += u64::from(b);
            sum2 += adler;
        }
        adler %= BASE;
        sum2 %= BASE;
    }
    // return recombined sums
    adler | (sum2 << 16)
}

#[cfg(test)]
mod tests {
    use super::adler32;

    #[test]
    fn incremental_is_one_shot() {
        // (the values themselves are checked against CPython through the
        // "incorrect data check" cases of oracle/unit/zlib.py)
        assert_eq!(adler32(1, b""), 1);
        let data: Vec<u8> = (0..20_000u32).map(|i| (i * 7 + 3) as u8 | 0x80).collect();
        let whole = adler32(1, &data);
        let (a, b) = data.split_at(7_777);
        assert_eq!(adler32(adler32(1, a), b), whole);
    }
}
