//! The C library's `pow`, as the oracle runs it.
//!
//! CPython's `float ** float` (`float_pow`) ends in a call to the platform
//! `pow`, so its last bits are those of the C library. The oracle is CPython
//! 3.12 on x86-64 glibc 2.39, whose `pow` is the ARM optimized-routines
//! algorithm (`sysdeps/ieee754/dbl-64/e_pow.c`, from
//! `upstream/arm-optimized-routines/math/pow.c`, MIT). Rust's `f64::powf` is
//! not that function everywhere: on `wasm32-unknown-unknown` it is FreeBSD's
//! `e_pow.c` (the `libm` crate), which differs in the last bit for about one
//! input in ten (`10 ** -5` gives `9.999999999999999e-06`). This module ports
//! glibc's `pow` with basic IEEE operations and fused multiply-adds
//! (`f64::mul_add`, correctly rounded on every target), so the result is the
//! same on every platform.
//!
//! # Which variant
//!
//! glibc 2.39 builds `e_pow.c` three times on x86-64 and picks one at load
//! time (`sysdeps/x86_64/fpu/multiarch/e_pow.c`): `__pow_fma` (compiled with
//! `-mfma -mavx2`) when the CPU has FMA and AVX2, as the oracle machine does
//! (`oracle/environment.py` records its CPU flags), else `__pow_fma4` or
//! `__pow_sse2`. The FMA build takes the `__FP_FAST_FMA` branches of the C
//! source, and GCC also contracts other `a * b + c` expressions into fused
//! operations (`-ffp-contract=fast`, its default). Which ones it fused was
//! read from the disassembly of `__pow_fma` in the oracle's `libm.so.6`;
//! each is marked "fused" below, and every other operation is rounded
//! separately, in the C order. (On aarch64, glibc defines `__FP_FAST_FMA`
//! too.) The tables are generated from the ARM sources
//! (`libm_tables.rs`, `oracle/codegen/libm_tables.py`); they are
//! byte-identical to those of the oracle's `libm.so.6`.
//!
//! Verified against the oracle's `pow` through CPython (`oracle/unit/pycompat.py`,
//! `pow` cases) and, natively, against glibc's `pow` itself
//! (`tests::matches_the_platform_pow`, on an x86-64 glibc host with FMA).

use crate::libm_tables::{
    EXP_INVLN2N, EXP_NEGLN2HIN, EXP_NEGLN2LON, EXP_POLY, EXP_SHIFT, EXP_TAB, POW_LN2HI, POW_LN2LO, POW_POLY, POW_TAB,
};

#[inline]
fn asuint64(x: f64) -> u64 {
    x.to_bits()
}

#[inline]
fn asdouble(i: u64) -> f64 {
    f64::from_bits(i)
}

/// Top 12 bits of a double (sign and exponent bits).
#[inline]
fn top12(x: f64) -> u32 {
    (asuint64(x) >> 52) as u32
}

const POW_LOG_TABLE_BITS: u32 = 7;
const EXP_TABLE_BITS: u32 = 7;
const OFF: u64 = 0x3fe6_9555_0000_0000;
const SIGN_BIAS: u32 = 0x800 << EXP_TABLE_BITS;
const ONE_BITS: u64 = 0x3ff0_0000_0000_0000;
const INF_BITS: u64 = 0x7ff0_0000_0000_0000;

/// `__math_oflow(sign)`: `±0x1p769 * 0x1p769`, an infinity.
fn math_oflow(sign: u32) -> f64 {
    if sign != 0 {
        f64::NEG_INFINITY
    } else {
        f64::INFINITY
    }
}

/// `__math_uflow(sign)`: `±0x1p-767 * 0x1p-767`, a zero.
fn math_uflow(sign: u32) -> f64 {
    if sign != 0 {
        -0.0
    } else {
        0.0
    }
}

// sysdeps/ieee754/dbl-64/e_pow.c log_inline
/// `log(x) = hi + lo` for the bit pattern `ix` of a positive normal `x` (or
/// of a subnormal one normalized with the exponent below zero).
fn log_inline(ix: u64) -> (f64, f64) {
    let a = POW_POLY.map(asdouble);
    let ln2hi = asdouble(POW_LN2HI);
    let ln2lo = asdouble(POW_LN2LO);
    // x = 2^k z; where z is in range [OFF,2*OFF) and exact.
    let tmp = ix.wrapping_sub(OFF);
    let i = ((tmp >> (52 - POW_LOG_TABLE_BITS)) % (1 << POW_LOG_TABLE_BITS)) as usize;
    let k = ((tmp as i64) >> 52) as i32; // arithmetic shift
    let iz = ix.wrapping_sub(tmp & (0xfff << 52));
    let z = asdouble(iz);
    let kd = f64::from(k);
    let (invc, logc, logctail) = POW_TAB[i];
    let (invc, logc, logctail) = (asdouble(invc), asdouble(logc), asdouble(logctail));
    // log(x) = k*Ln2 + log(c) + log1p(z/c-1); r = z/c - 1 is exact.
    let r = z.mul_add(invc, -1.0); // __builtin_fma
                                   // k*Ln2 + log(c) + r.
    let t1 = kd.mul_add(ln2hi, logc); // fused
    let t2 = t1 + r;
    let lo1 = kd.mul_add(ln2lo, logctail); // fused
    let lo2 = t1 - t2 + r;
    let ar = a[0] * r; // A[0] = -0.5
    let ar2 = r * ar;
    let ar3 = r * ar2;
    // k*Ln2 + log(c) + r + A[0]*r*r.
    let hi = t2 + ar2;
    let lo3 = ar.mul_add(r, -ar2); // __builtin_fma
    let lo4 = t2 - hi + ar2;
    // p = log1p(r) - r - A[0]*r*r, fused into the sum of the low parts:
    // ar3 * (A[1] + r*A[2] + ar2*(A[3] + r*A[4] + ar2*(A[5] + r*A[6]))).
    let q = ar2.mul_add(r.mul_add(a[6], a[5]), r.mul_add(a[4], a[3]));
    let q = ar2.mul_add(q, r.mul_add(a[2], a[1]));
    let lo = ar3.mul_add(q, lo1 + lo2 + lo3 + lo4);
    let y = hi + lo;
    let tail = hi - y + lo;
    (y, tail)
}

// sysdeps/ieee754/dbl-64/e_pow.c specialcase
/// `scale * (1 + tmp)` when it may overflow or underflow; `sbits` is the bit
/// pattern of scale with an exponent that may have overflowed, `ki` the `k`
/// of the argument reduction.
fn specialcase(tmp: f64, sbits: u64, ki: u64) -> f64 {
    if ki & 0x8000_0000 == 0 {
        // k > 0, the exponent of scale might have overflowed by <= 460.
        let scale = asdouble(sbits.wrapping_sub(1009 << 52));
        return f64::from_bits(0x7f00_0000_0000_0000) * scale.mul_add(tmp, scale);
        // 0x1p1009 * fused
    }
    // k < 0, need special care in the subnormal range.
    let sbits = sbits.wrapping_add(1022 << 52);
    // Note: sbits is signed scale.
    let scale = asdouble(sbits);
    let st = scale * tmp; // not fused: the product is used twice
    let mut y = scale + st;
    if y.abs() < 1.0 {
        // Round y to the right precision before scaling it into the subnormal
        // range to avoid double rounding.
        let one = if y < 0.0 { -1.0 } else { 1.0 };
        let lo = scale - y + st;
        let hi = one + y;
        let lo = one - hi + y + lo;
        y = (hi + lo) - one;
        // Fix the sign of 0.
        if y == 0.0 {
            y = asdouble(sbits & 0x8000_0000_0000_0000);
        }
    }
    f64::from_bits(0x0010_0000_0000_0000) * y // 0x1p-1022 * y
}

// sysdeps/ieee754/dbl-64/e_pow.c exp_inline
/// `sign * exp(x + xtail)`, where `|xtail| < 2^-8/N` and `|xtail| <= |x|`;
/// `sign_bias` is `SIGN_BIAS` (negative) or 0.
fn exp_inline(x: f64, xtail: f64, sign_bias: u32) -> f64 {
    const N: u64 = 1 << EXP_TABLE_BITS;
    let c = EXP_POLY.map(asdouble); // C2, C3, C4, C5
    let shift = asdouble(EXP_SHIFT);
    let tiny = top12(f64::from_bits(0x3c90_0000_0000_0000)); // top12(0x1p-54)
    let mut abstop = top12(x) & 0x7ff;
    if abstop.wrapping_sub(tiny) >= top12(512.0).wrapping_sub(tiny) {
        if abstop.wrapping_sub(tiny) >= 0x8000_0000 {
            // Avoid spurious underflow for tiny x.
            let one = 1.0 + x;
            return if sign_bias != 0 { -one } else { one };
        }
        if abstop >= top12(1024.0) {
            // Note: inf and nan are already handled.
            return if asuint64(x) >> 63 != 0 { math_uflow(sign_bias) } else { math_oflow(sign_bias) };
        }
        // Large x is special cased below.
        abstop = 0;
    }
    // exp(x) = 2^(k/N) * exp(r), with exp(r) in [2^(-1/2N),2^(1/2N)].
    // x = ln2/N*k + r, with int k and r in [-ln2/2N, ln2/2N].
    let kd = x.mul_add(asdouble(EXP_INVLN2N), shift); // z = InvLn2N * x; kd = z + Shift, fused
    let ki = asuint64(kd);
    let kd = kd - shift;
    let r = kd.mul_add(asdouble(EXP_NEGLN2LON), kd.mul_add(asdouble(EXP_NEGLN2HIN), x)); // fused twice
                                                                                         // The code assumes 2^-200 < |xtail| < 2^-8/N.
    let r = r + xtail;
    // 2^(k/N) ~= scale * (1 + tail).
    let idx = (2 * (ki % N)) as usize;
    let top = ki.wrapping_add(u64::from(sign_bias)) << (52 - EXP_TABLE_BITS);
    let tail = asdouble(EXP_TAB[idx]);
    // This is only a valid scale when -1023*N < k < 1024*N.
    let sbits = EXP_TAB[idx + 1].wrapping_add(top);
    // exp(x) = 2^(k/N) * exp(r) ~= scale + scale * (tail + exp(r) - 1).
    let r2 = r * r;
    // tail + r + r2 * (C2 + r * C3) + r2 * r2 * (C4 + r * C5), fused
    let tmp = (r2 * r2).mul_add(r.mul_add(c[3], c[2]), r2.mul_add(r.mul_add(c[1], c[0]), tail + r));
    if abstop == 0 {
        return specialcase(tmp, sbits, ki);
    }
    let scale = asdouble(sbits);
    scale.mul_add(tmp, scale) // scale + scale * tmp, fused
}

// sysdeps/ieee754/dbl-64/e_pow.c checkint
/// 0 if not int, 1 if odd int, 2 if even int. The argument is the bit
/// representation of a non-zero finite floating-point value.
fn checkint(iy: u64) -> u32 {
    let e = (iy >> 52 & 0x7ff) as i32;
    if e < 0x3ff {
        return 0;
    }
    if e > 0x3ff + 52 {
        return 2;
    }
    if iy & ((1u64 << (0x3ff + 52 - e)) - 1) != 0 {
        return 0;
    }
    if iy & (1u64 << (0x3ff + 52 - e)) != 0 {
        return 1;
    }
    2
}

// sysdeps/ieee754/dbl-64/e_pow.c zeroinfnan
/// The bit representation of 0, infinity or nan.
fn zeroinfnan(i: u64) -> bool {
    (i << 1).wrapping_sub(1) >= (INF_BITS << 1) - 1
}

/// `issignaling_inline(x)`: a signaling NaN.
fn issignaling(x: f64) -> bool {
    let ix = asuint64(x);
    (ix ^ 0x0008_0000_0000_0000).wrapping_mul(2) > 0xfff0_0000_0000_0000
}

// sysdeps/ieee754/dbl-64/e_pow.c __pow
/// `pow(x, y)`, bit for bit as glibc 2.39's `__pow_fma` (the oracle's
/// `pow`). Errno is not modeled: CPython's `float_pow` only calls `pow` with
/// a finite positive `x` other than 1 and a finite non-zero `y`, and its
/// only observable errno case (overflow) is an infinite result.
pub fn pow(x: f64, y: f64) -> f64 {
    let mut sign_bias = 0u32;
    let mut ix = asuint64(x);
    let iy = asuint64(y);
    let mut topx = top12(x);
    let topy = top12(y);
    if topx.wrapping_sub(0x001) >= 0x7ff - 0x001 || (topy & 0x7ff).wrapping_sub(0x3be) >= 0x43e - 0x3be {
        // Special cases: (x < 0x1p-126 or inf or nan) or
        // (|y| < 0x1p-65 or |y| >= 0x1p63 or nan).
        if zeroinfnan(iy) {
            if iy << 1 == 0 {
                return if issignaling(x) { x + y } else { 1.0 };
            }
            if ix == ONE_BITS {
                return if issignaling(y) { x + y } else { 1.0 };
            }
            if ix << 1 > INF_BITS << 1 || iy << 1 > INF_BITS << 1 {
                return x + y;
            }
            if ix << 1 == ONE_BITS << 1 {
                return 1.0;
            }
            if (ix << 1 < ONE_BITS << 1) == (iy >> 63 == 0) {
                return 0.0; // |x|<1 && y==inf or |x|>1 && y==-inf.
            }
            return y * y;
        }
        if zeroinfnan(ix) {
            let mut x2 = x * x;
            if ix >> 63 != 0 && checkint(iy) == 1 {
                x2 = -x2;
                sign_bias = 1;
            }
            if ix << 1 == 0 && iy >> 63 != 0 {
                // __math_divzero(sign_bias)
                return if sign_bias != 0 { f64::NEG_INFINITY } else { f64::INFINITY };
            }
            return if iy >> 63 != 0 { 1.0 / x2 } else { x2 };
        }
        // Here x and y are non-zero finite.
        if ix >> 63 != 0 {
            // Finite x < 0.
            let yint = checkint(iy);
            if yint == 0 {
                return f64::NAN; // __math_invalid(x)
            }
            if yint == 1 {
                sign_bias = SIGN_BIAS;
            }
            ix &= 0x7fff_ffff_ffff_ffff;
            topx &= 0x7ff;
        }
        if (topy & 0x7ff).wrapping_sub(0x3be) >= 0x43e - 0x3be {
            // Note: sign_bias == 0 here because y is not odd.
            if ix == ONE_BITS {
                return 1.0;
            }
            if (topy & 0x7ff) < 0x3be {
                // |y| < 2^-65, x^y ~= 1 + y*log(x).
                return if ix > ONE_BITS { 1.0 + y } else { 1.0 - y };
            }
            return if (ix > ONE_BITS) == (topy < 0x800) { math_oflow(0) } else { math_uflow(0) };
        }
        if topx == 0 {
            // Normalize subnormal x so exponent becomes negative.
            ix = asuint64(x * f64::from_bits(0x4330_0000_0000_0000)); // x * 0x1p52
            ix &= 0x7fff_ffff_ffff_ffff;
            ix = ix.wrapping_sub(52 << 52);
        }
    }
    let (hi, lo) = log_inline(ix);
    let ehi = y * hi;
    let elo = y.mul_add(lo, y.mul_add(hi, -ehi)); // y * lo + __builtin_fma (y, hi, -ehi), fused
    exp_inline(ehi, elo, sign_bias)
}

#[cfg(test)]
mod tests {
    use super::pow;

    /// A small deterministic generator (SplitMix64).
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^ (z >> 31)
        }

        fn unit(&mut self) -> f64 {
            (self.next() >> 11) as f64 / (1u64 << 53) as f64
        }
    }

    /// Inputs of every kind: random bit patterns, ordinary magnitudes, ints
    /// raised to small integral powers, results near overflow and in the
    /// subnormal range.
    fn inputs(n: usize) -> Vec<(f64, f64)> {
        let mut rng = Rng(20261014);
        let mut out = Vec::with_capacity(n);
        let sign = |rng: &mut Rng| if rng.next() & 1 == 0 { 1.0 } else { -1.0 };
        for i in 0..n {
            let (x, y) = match i % 12 {
                0 => (f64::from_bits(rng.next()), f64::from_bits(rng.next())),
                1 => (rng.unit() * 1e4, rng.unit() * 20.0 - 10.0),
                2 => ((rng.next() % 400) as f64 + 2.0, -((rng.next() % 6) as f64) - 1.0),
                3 => (rng.unit() * 2.0, (rng.unit() - 0.5) * 2000.0),
                4 => (1.0 + (rng.unit() - 0.5) * 1e-6, (rng.unit() - 0.5) * 1e9),
                5 => (f64::from_bits(rng.next() & 0x000f_ffff_ffff_ffff), rng.unit() * 2.0 - 0.5),
                // |y| < 2**-65: 1 + y*log(x), on both sides of 1
                6 => (rng.unit() * 4.0, sign(&mut rng) * rng.unit() * 2f64.powi(-65 - (rng.next() % 20) as i32)),
                // |y| >= 2**63: overflow or underflow, bases on both sides of 1
                7 => {
                    (rng.unit() * 2.0, sign(&mut rng) * 2f64.powi(63 + (rng.next() % 900) as i32) * (1.0 + rng.unit()))
                }
                // negative bases to integral powers (odd: negative results), and
                // results in the subnormal range, of both signs
                8 => (-(rng.unit() * 0.9 + 0.05), ((rng.next() % 1200) as f64) + 900.0),
                9 => (-(rng.unit() * 100.0), ((rng.next() % 81) as f64) - 40.0),
                // exp(x + xtail) with |x| < 2**-54: bases next to 1, small powers
                10 => (
                    1.0 + sign(&mut rng) * f64::EPSILON * (1 + rng.next() % 4) as f64,
                    sign(&mut rng) * rng.unit() * 0.4,
                ),
                // zero, infinite, nan (signaling ones included) and subnormal operands
                _ => {
                    const ODD: [u64; 6] = [
                        0x7ff0_0000_0000_0001, // signaling NaN
                        0xfff4_0000_0000_0000, // negative signaling NaN
                        0x7ff8_0000_0000_0000, // quiet NaN
                        0x7ff0_0000_0000_0000, // inf
                        0x0000_0000_0000_0003, // subnormal
                        0x8000_0000_0000_0000, // -0.0
                    ];
                    let special = f64::from_bits(ODD[(rng.next() % 6) as usize]);
                    let other = [1.0, -1.0, 0.0, 3.0, -3.0, 0.5, 2.0, -0.5][(rng.next() % 8) as usize];
                    if rng.next() & 1 == 0 {
                        (special, other)
                    } else {
                        (other, special)
                    }
                }
            };
            out.push((x, y));
        }
        out
    }

    /// Bitwise equality, any NaN equal to any NaN.
    fn same(a: f64, b: f64) -> bool {
        a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
    }

    const SPECIALS: [f64; 16] = [
        0.0,
        -0.0,
        1.0,
        -1.0,
        0.5,
        -0.5,
        2.0,
        -2.0,
        3.0,
        -3.0,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NAN,
        5e-324,
        1e308,
        -1e308,
    ];

    #[test]
    fn special_values() {
        // C99 Annex F values (glibc follows them).
        assert!(same(pow(1.0, f64::NAN), 1.0));
        assert!(same(pow(f64::NAN, 0.0), 1.0));
        assert!(same(pow(-1.0, f64::INFINITY), 1.0));
        assert!(same(pow(f64::NEG_INFINITY, -3.0), -0.0));
        assert!(same(pow(f64::NEG_INFINITY, 3.0), f64::NEG_INFINITY));
        assert!(same(pow(-0.0, -3.0), f64::NEG_INFINITY));
        assert!(same(pow(-2.0, 0.5), f64::NAN));
        assert!(same(pow(10.0, -5.0), 1e-05));
        assert!(same(pow(1e308, 2.0), f64::INFINITY));
        assert!(same(pow(5e-324, 0.5), 2.2227587494850775e-162));
    }

    /// The port against the platform's `pow`, which is the oracle's own
    /// function on an x86-64 glibc host whose CPU has FMA and AVX2 (the
    /// oracle machine): every input must give the same bits. Elsewhere
    /// (another libm, or glibc's non-FMA variant) the test is skipped.
    #[test]
    fn matches_the_platform_pow() {
        let host_is_oracle = cfg!(all(target_os = "linux", target_env = "gnu", target_arch = "x86_64"))
            && std::fs::read_to_string("/proc/cpuinfo").is_ok_and(|info| {
                info.split_whitespace().any(|f| f == "fma") && info.split_whitespace().any(|f| f == "avx2")
            });
        if !host_is_oracle {
            return;
        }
        let mut differ = Vec::new();
        let specials = SPECIALS.iter().flat_map(|&x| SPECIALS.iter().map(move |&y| (x, y)));
        for (x, y) in inputs(2_000_000).into_iter().chain(specials) {
            let (a, b) = (pow(x, y), x.powf(y));
            if !same(a, b) {
                differ.push(format!("pow({x:e}, {y:e}) = {a:e}, glibc {b:e}"));
            }
        }
        assert!(differ.is_empty(), "{} differences, first: {:?}", differ.len(), &differ[..differ.len().min(5)]);
    }
}
