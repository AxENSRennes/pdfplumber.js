# glibc 2.39: `atan2` sources

Files copied from the GNU C Library at tag `glibc-2.39` (the oracle machine's
C library: Ubuntu's glibc 2.39), fetched on 2026-10-15 from
<https://raw.githubusercontent.com/bminor/glibc/glibc-2.39/> (a mirror of
<https://sourceware.org/git/glibc.git>), under their license, the GNU Lesser
General Public License 2.1 or later (`COPYING.LIB`):

- `sysdeps/ieee754/dbl-64/e_atan2.c`: `__ieee754_atan2` (IBM Accurate
  Mathematical Library), the algorithm;
- `sysdeps/ieee754/dbl-64/atnat2.h`, `uatan.tbl`: its constants and the
  `cij` table;
- `sysdeps/ieee754/dbl-64/dla.h`, `mydefs.h`: the macros it uses (`EADD`,
  `ESUB`, `EMULV` with `DLA_FMS`);
- `sysdeps/x86_64/fpu/multiarch/e_atan2.c`, `e_atan2-fma.c`,
  `ifunc-avx-fma4.h`: the x86-64 variants and how one is chosen (`-mfma
  -mavx2` build when the CPU has FMA and AVX2, as the oracle machine does);
- `math/w_atan2_compat.c`: the exported `atan2`, which returns
  `__ieee754_atan2` unchanged (it only sets `errno`).

Why: CPython's `math.atan2` calls the C library's `atan2`, and pdfplumber's
`CTM.skew_x`/`skew_y` call `math.atan2`. Rust's `f64::atan2` is musl's
algorithm on WebAssembly, which differs from glibc's in the last bit for
about 17 % of inputs. `crates/pycompat/src/libm.rs` ports this code so that
the Rust core gives the oracle's bits on every target.

- `oracle/codegen/atan2_tables.py` reads the constants of `atnat2.h` and the
  `cij` table of `uatan.tbl` and writes `crates/pycompat/src/atan2_tables.rs`.
  When these files were copied, the generated values were compared with the
  bytes of the oracle machine's `/usr/lib/x86_64-linux-gnu/libm.so.6` at the
  addresses `__ieee754_atan2_fma` reads them from (identical), and the
  little-endian and big-endian variants of the table with each other.
- The fused multiply-adds of the `-mfma` build were read from the
  disassembly of `__ieee754_atan2_fma` (the target of the `__atan2_finite`
  ifunc for a CPU with FMA and AVX2); see the module documentation of
  `libm.rs`.

License note: `libm.rs`'s `atan2` is a translation of `e_atan2.c` and its
tables are data of `uatan.tbl`/`atnat2.h`, so that part of `pycompat` is
derived from LGPL-2.1-or-later code.
