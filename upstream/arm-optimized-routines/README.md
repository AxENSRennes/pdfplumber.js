# ARM optimized-routines: `pow` sources

Files copied from <https://github.com/ARM-software/optimized-routines> at
commit `97362b2eab91d35d30c4ac2c8c79dcd4dae4331b` (2026-10-13), under its
license (`LICENSE`: MIT OR Apache-2.0 WITH LLVM-exception):

- `math/pow.c`, `math/pow_log_data.c`, `math/exp_data.c`, `math/math_config.h`

Why: CPython's `float ** float` calls the C library's `pow`. The oracle
(CPython 3.12 on x86-64 glibc 2.39) runs glibc's `pow`, which is this
algorithm (glibc `sysdeps/ieee754/dbl-64/e_pow.c`, contributed by Arm), and
`crates/pycompat/src/libm.rs` ports it so that the Rust core gives the
oracle's bits on every target, including WebAssembly, whose `f64::powf` is
another algorithm.

- `oracle/codegen/libm_tables.py` reads the tables from `pow_log_data.c` and
  `exp_data.c` (with glibc's configuration: 128-entry tables, a degree-8 log
  polynomial, a degree-5 exp polynomial) and writes
  `crates/pycompat/src/libm_tables.rs`. When these files were copied, the
  extracted tables were compared with glibc 2.39's `e_pow_log_data.c` and
  `e_exp_data.c` (identical) and with the bytes of the oracle machine's
  `/usr/lib/x86_64-linux-gnu/libm.so.6` at the addresses `__pow_fma` reads
  them from (identical).
- `pow.c` is the reference for the algorithm; `libm.rs` follows glibc 2.39's
  `e_pow.c` where the two differ in form (they compute the same values). The
  fused multiply-adds of the oracle's `__pow_fma` build were read from its
  disassembly; see the module documentation of `libm.rs`.
