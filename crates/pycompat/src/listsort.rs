//! `list.sort()` / `sorted()` exactly as CPython 3.12 (`Objects/listobject.c`,
//! `list_sort_impl` and its helpers: `count_run`, `binarysort`,
//! `gallop_left`/`gallop_right`, `merge_lo`/`merge_hi`, `merge_at`, the
//! "powersort" merge policy of `found_new_run`/`powerloop`, and
//! `merge_force_collapse`).
//!
//! With a consistent total order, any stable sort gives Python's result. The
//! port matters when the order is not total or when a comparison raises:
//! a NaN key (`nan < x` and `x < nan` are both false), or keys whose `<`
//! raises `TypeError` (`None < 1.0`). The result (or the first exception)
//! then depends on which pairs are compared and in which order; this port
//! makes the same comparisons, in the same order, as CPython.
//!
//! Only `<` is used (`IFLT`), as in CPython; keys are computed once per
//! item, in order, before any comparison (`list.sort(key=...)`).
//!
//! The sort works on a permutation of indices, so items never move until
//! the end and need not be `Clone`.

use crate::err::PyResult;

/// `MIN_GALLOP`
const MIN_GALLOP: usize = 7;

/// A run waiting to be merged (`struct s_slice` + its `power`).
#[derive(Clone, Copy)]
struct Run {
    base: usize,
    len: usize,
    power: u32,
}

struct MergeState<'a, F: FnMut(usize, usize) -> PyResult<bool>> {
    /// The permutation being sorted: `keys[i]` is an index into the items.
    keys: Vec<usize>,
    /// Scratch space of `merge_lo`/`merge_hi` (`ms->a`).
    tmp: Vec<usize>,
    pending: Vec<Run>,
    min_gallop: usize,
    listlen: usize,
    lt: &'a mut F,
}

impl<F: FnMut(usize, usize) -> PyResult<bool>> MergeState<'_, F> {
    /// `ISLT(X, Y)` on item indices.
    #[inline]
    fn islt(&mut self, x: usize, y: usize) -> PyResult<bool> {
        (self.lt)(x, y)
    }

    /// `count_run` (3.12): length of the run at `lo`, and whether it is
    /// strictly descending.
    fn count_run(&mut self, lo: usize, hi: usize) -> PyResult<(usize, bool)> {
        let mut lo = lo + 1;
        if lo == hi {
            return Ok((1, false));
        }
        let mut n = 2;
        if self.islt(self.keys[lo], self.keys[lo - 1])? {
            lo += 1;
            while lo < hi {
                if !self.islt(self.keys[lo], self.keys[lo - 1])? {
                    break;
                }
                lo += 1;
                n += 1;
            }
            Ok((n, true))
        } else {
            lo += 1;
            while lo < hi {
                if self.islt(self.keys[lo], self.keys[lo - 1])? {
                    break;
                }
                lo += 1;
                n += 1;
            }
            Ok((n, false))
        }
    }

    /// `binarysort(ms, lo, hi, start)`: `[lo, start)` is sorted.
    fn binarysort(&mut self, lo: usize, hi: usize, mut start: usize) -> PyResult<()> {
        if lo == start {
            start += 1;
        }
        while start < hi {
            let mut l = lo;
            let mut r = start;
            let pivot = self.keys[r];
            loop {
                let p = l + ((r - l) >> 1);
                if self.islt(pivot, self.keys[p])? {
                    r = p;
                } else {
                    l = p + 1;
                }
                if l >= r {
                    break;
                }
            }
            self.keys.copy_within(l..start, l + 1);
            self.keys[l] = pivot;
            start += 1;
        }
        Ok(())
    }

    /// `gallop_left(ms, key, a, n, hint)` over `src[a..a + n]`.
    fn gallop_left(&mut self, key: usize, src: Src, a: usize, n: usize, hint: usize) -> PyResult<usize> {
        let at = |s: &Self, i: usize| match src {
            Src::Keys => s.keys[i],
            Src::Tmp => s.tmp[i],
        };
        let mut lastofs: isize;
        let mut ofs: isize;
        let ah = a + hint;
        let (h, nn) = (hint as isize, n as isize);
        lastofs = 0;
        ofs = 1;
        if self.islt(at(self, ah), key)? {
            let maxofs = nn - h;
            while ofs < maxofs {
                if self.islt(at(self, (ah as isize + ofs) as usize), key)? {
                    lastofs = ofs;
                    ofs = (ofs << 1) + 1;
                } else {
                    break;
                }
            }
            if ofs > maxofs {
                ofs = maxofs;
            }
            lastofs += h;
            ofs += h;
        } else {
            let maxofs = h + 1;
            while ofs < maxofs {
                if self.islt(at(self, (ah as isize - ofs) as usize), key)? {
                    break;
                }
                lastofs = ofs;
                ofs = (ofs << 1) + 1;
            }
            if ofs > maxofs {
                ofs = maxofs;
            }
            let k = lastofs;
            lastofs = h - ofs;
            ofs = h - k;
        }
        lastofs += 1;
        while lastofs < ofs {
            let m = lastofs + ((ofs - lastofs) >> 1);
            if self.islt(at(self, a + m as usize), key)? {
                lastofs = m + 1;
            } else {
                ofs = m;
            }
        }
        Ok(ofs as usize)
    }

    /// `gallop_right(ms, key, a, n, hint)` over `src[a..a + n]`.
    fn gallop_right(&mut self, key: usize, src: Src, a: usize, n: usize, hint: usize) -> PyResult<usize> {
        let at = |s: &Self, i: usize| match src {
            Src::Keys => s.keys[i],
            Src::Tmp => s.tmp[i],
        };
        let mut lastofs: isize;
        let mut ofs: isize;
        let ah = a + hint;
        let (h, nn) = (hint as isize, n as isize);
        lastofs = 0;
        ofs = 1;
        if self.islt(key, at(self, ah))? {
            let maxofs = h + 1;
            while ofs < maxofs {
                if self.islt(key, at(self, (ah as isize - ofs) as usize))? {
                    lastofs = ofs;
                    ofs = (ofs << 1) + 1;
                } else {
                    break;
                }
            }
            if ofs > maxofs {
                ofs = maxofs;
            }
            let k = lastofs;
            lastofs = h - ofs;
            ofs = h - k;
        } else {
            let maxofs = nn - h;
            while ofs < maxofs {
                if self.islt(key, at(self, (ah as isize + ofs) as usize))? {
                    break;
                }
                lastofs = ofs;
                ofs = (ofs << 1) + 1;
            }
            if ofs > maxofs {
                ofs = maxofs;
            }
            lastofs += h;
            ofs += h;
        }
        lastofs += 1;
        while lastofs < ofs {
            let m = lastofs + ((ofs - lastofs) >> 1);
            if self.islt(key, at(self, a + m as usize))? {
                ofs = m;
            } else {
                lastofs = m + 1;
            }
        }
        Ok(ofs as usize)
    }

    /// `merge_lo(ms, ssa, na, ssb, nb)`: `na <= nb`, A copied to `tmp`.
    fn merge_lo(&mut self, pa: usize, mut na: usize, pb: usize, mut nb: usize) -> PyResult<()> {
        self.tmp.clear();
        self.tmp.extend_from_slice(&self.keys[pa..pa + na]);
        let mut dest = pa;
        let mut a = 0usize; // into tmp
        let mut b = pb; // into keys
        let result: PyResult<()> = 'merge: {
            self.keys[dest] = self.keys[b];
            dest += 1;
            b += 1;
            nb -= 1;
            if nb == 0 {
                break 'merge Ok(());
            }
            if na == 1 {
                // CopyB
                self.keys.copy_within(b..b + nb, dest);
                self.keys[dest + nb] = self.tmp[a];
                return Ok(());
            }
            let mut min_gallop = self.min_gallop;
            loop {
                let mut acount = 0usize;
                let mut bcount = 0usize;
                loop {
                    let k = match self.islt(self.keys[b], self.tmp[a]) {
                        Ok(k) => k,
                        Err(e) => break 'merge Err(e),
                    };
                    if k {
                        self.keys[dest] = self.keys[b];
                        dest += 1;
                        b += 1;
                        bcount += 1;
                        acount = 0;
                        nb -= 1;
                        if nb == 0 {
                            break 'merge Ok(());
                        }
                        if bcount >= min_gallop {
                            break;
                        }
                    } else {
                        self.keys[dest] = self.tmp[a];
                        dest += 1;
                        a += 1;
                        acount += 1;
                        bcount = 0;
                        na -= 1;
                        if na == 1 {
                            self.keys.copy_within(b..b + nb, dest);
                            self.keys[dest + nb] = self.tmp[a];
                            return Ok(());
                        }
                        if acount >= min_gallop {
                            break;
                        }
                    }
                }
                min_gallop += 1;
                loop {
                    min_gallop -= usize::from(min_gallop > 1);
                    self.min_gallop = min_gallop;
                    let k = match self.gallop_right(self.keys[b], Src::Tmp, a, na, 0) {
                        Ok(k) => k,
                        Err(e) => break 'merge Err(e),
                    };
                    acount = k;
                    if k != 0 {
                        self.keys[dest..dest + k].copy_from_slice(&self.tmp[a..a + k]);
                        dest += k;
                        a += k;
                        na -= k;
                        if na == 1 {
                            self.keys.copy_within(b..b + nb, dest);
                            self.keys[dest + nb] = self.tmp[a];
                            return Ok(());
                        }
                        if na == 0 {
                            break 'merge Ok(());
                        }
                    }
                    self.keys[dest] = self.keys[b];
                    dest += 1;
                    b += 1;
                    nb -= 1;
                    if nb == 0 {
                        break 'merge Ok(());
                    }
                    let k = match self.gallop_left(self.tmp[a], Src::Keys, b, nb, 0) {
                        Ok(k) => k,
                        Err(e) => break 'merge Err(e),
                    };
                    bcount = k;
                    if k != 0 {
                        self.keys.copy_within(b..b + k, dest);
                        dest += k;
                        b += k;
                        nb -= k;
                        if nb == 0 {
                            break 'merge Ok(());
                        }
                    }
                    self.keys[dest] = self.tmp[a];
                    dest += 1;
                    a += 1;
                    na -= 1;
                    if na == 1 {
                        self.keys.copy_within(b..b + nb, dest);
                        self.keys[dest + nb] = self.tmp[a];
                        return Ok(());
                    }
                    if !(acount >= MIN_GALLOP || bcount >= MIN_GALLOP) {
                        break;
                    }
                }
                min_gallop += 1;
                self.min_gallop = min_gallop;
            }
        };
        // Succeed / Fail: what is left of A goes to dest.
        if na != 0 {
            self.keys[dest..dest + na].copy_from_slice(&self.tmp[a..a + na]);
        }
        result
    }

    /// `merge_hi(ms, ssa, na, ssb, nb)`: `na > nb`, B copied to `tmp`.
    fn merge_hi(&mut self, pa: usize, mut na: usize, pb: usize, mut nb: usize) -> PyResult<()> {
        self.tmp.clear();
        self.tmp.extend_from_slice(&self.keys[pb..pb + nb]);
        // Positions are of the *last* element of each part (signed, they
        // reach -1 when a part is exhausted).
        let basea = pa as isize;
        let mut dest = (pb + nb - 1) as isize;
        let mut b = nb as isize - 1; // into tmp
        let mut a = (pa + na - 1) as isize; // into keys
        let kk = |i: isize| i as usize;
        let result: PyResult<()> = 'merge: {
            self.keys[kk(dest)] = self.keys[kk(a)];
            dest -= 1;
            a -= 1;
            na -= 1;
            if na == 0 {
                break 'merge Ok(());
            }
            if nb == 1 {
                // CopyA
                let n = na as isize;
                self.keys.copy_within(kk(a + 1 - n)..kk(a + 1), kk(dest + 1 - n));
                dest -= n;
                self.keys[kk(dest)] = self.tmp[kk(b)];
                return Ok(());
            }
            let mut min_gallop = self.min_gallop;
            loop {
                let mut acount = 0usize;
                let mut bcount = 0usize;
                loop {
                    let k = match self.islt(self.tmp[kk(b)], self.keys[kk(a)]) {
                        Ok(k) => k,
                        Err(e) => break 'merge Err(e),
                    };
                    if k {
                        self.keys[kk(dest)] = self.keys[kk(a)];
                        dest -= 1;
                        a -= 1;
                        acount += 1;
                        bcount = 0;
                        na -= 1;
                        if na == 0 {
                            break 'merge Ok(());
                        }
                        if acount >= min_gallop {
                            break;
                        }
                    } else {
                        self.keys[kk(dest)] = self.tmp[kk(b)];
                        dest -= 1;
                        b -= 1;
                        bcount += 1;
                        acount = 0;
                        nb -= 1;
                        if nb == 1 {
                            let n = na as isize;
                            self.keys.copy_within(kk(a + 1 - n)..kk(a + 1), kk(dest + 1 - n));
                            dest -= n;
                            self.keys[kk(dest)] = self.tmp[kk(b)];
                            return Ok(());
                        }
                        if bcount >= min_gallop {
                            break;
                        }
                    }
                }
                min_gallop += 1;
                loop {
                    min_gallop -= usize::from(min_gallop > 1);
                    self.min_gallop = min_gallop;
                    let k = match self.gallop_right(self.tmp[kk(b)], Src::Keys, kk(basea), na, na - 1) {
                        Ok(k) => k,
                        Err(e) => break 'merge Err(e),
                    };
                    let k = na - k;
                    acount = k;
                    if k != 0 {
                        let ki = k as isize;
                        dest -= ki;
                        a -= ki;
                        self.keys.copy_within(kk(a + 1)..kk(a + 1 + ki), kk(dest + 1));
                        na -= k;
                        if na == 0 {
                            break 'merge Ok(());
                        }
                    }
                    self.keys[kk(dest)] = self.tmp[kk(b)];
                    dest -= 1;
                    b -= 1;
                    nb -= 1;
                    if nb == 1 {
                        let n = na as isize;
                        self.keys.copy_within(kk(a + 1 - n)..kk(a + 1), kk(dest + 1 - n));
                        dest -= n;
                        self.keys[kk(dest)] = self.tmp[kk(b)];
                        return Ok(());
                    }
                    let k = match self.gallop_left(self.keys[kk(a)], Src::Tmp, 0, nb, nb - 1) {
                        Ok(k) => k,
                        Err(e) => break 'merge Err(e),
                    };
                    let k = nb - k;
                    bcount = k;
                    if k != 0 {
                        let ki = k as isize;
                        dest -= ki;
                        b -= ki;
                        let (from, to) = (kk(b + 1), kk(dest + 1));
                        self.keys[to..to + k].copy_from_slice(&self.tmp[from..from + k]);
                        nb -= k;
                        if nb == 1 {
                            let n = na as isize;
                            self.keys.copy_within(kk(a + 1 - n)..kk(a + 1), kk(dest + 1 - n));
                            dest -= n;
                            self.keys[kk(dest)] = self.tmp[kk(b)];
                            return Ok(());
                        }
                        if nb == 0 {
                            break 'merge Ok(());
                        }
                    }
                    self.keys[kk(dest)] = self.keys[kk(a)];
                    dest -= 1;
                    a -= 1;
                    na -= 1;
                    if na == 0 {
                        break 'merge Ok(());
                    }
                    if !(acount >= MIN_GALLOP || bcount >= MIN_GALLOP) {
                        break;
                    }
                }
                min_gallop += 1;
                self.min_gallop = min_gallop;
            }
        };
        // Succeed / Fail: what is left of B goes to dest.
        if nb != 0 {
            let to = kk(dest + 1 - nb as isize);
            self.keys[to..to + nb].copy_from_slice(&self.tmp[..nb]);
        }
        result
    }

    /// `merge_at(ms, i)`: merges the runs `i` and `i + 1` of the stack.
    fn merge_at(&mut self, i: usize) -> PyResult<()> {
        let n = self.pending.len();
        let Run { base: pa, len: mut na, .. } = self.pending[i];
        let Run { base: pb, len: mut nb, .. } = self.pending[i + 1];
        self.pending[i].len = na + nb;
        if i + 3 == n {
            self.pending[i + 1] = self.pending[i + 2];
        }
        self.pending.pop();

        let k = self.gallop_right(self.keys[pb], Src::Keys, pa, na, 0)?;
        let pa = pa + k;
        na -= k;
        if na == 0 {
            return Ok(());
        }
        nb = self.gallop_left(self.keys[pa + na - 1], Src::Keys, pb, nb, nb - 1)?;
        if nb == 0 {
            return Ok(());
        }
        if na <= nb {
            self.merge_lo(pa, na, pb, nb)
        } else {
            self.merge_hi(pa, na, pb, nb)
        }
    }

    /// `found_new_run(ms, n2)`: the powersort merge policy.
    fn found_new_run(&mut self, n2: usize) -> PyResult<()> {
        if let Some(last) = self.pending.last().copied() {
            let power = powerloop(last.base, last.len, n2, self.listlen);
            while self.pending.len() > 1 && self.pending[self.pending.len() - 2].power > power {
                self.merge_at(self.pending.len() - 2)?;
            }
            let n = self.pending.len();
            self.pending[n - 1].power = power;
        }
        Ok(())
    }

    /// `merge_force_collapse(ms)`
    fn merge_force_collapse(&mut self) -> PyResult<()> {
        while self.pending.len() > 1 {
            let mut n = self.pending.len() - 2;
            if n > 0 && self.pending[n - 1].len < self.pending[n + 1].len {
                n -= 1;
            }
            self.merge_at(n)?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum Src {
    Keys,
    Tmp,
}

/// `powerloop(s1, n1, n2, n)`
fn powerloop(s1: usize, n1: usize, n2: usize, n: usize) -> u32 {
    let mut result = 0;
    let mut a = 2 * s1 + n1;
    let mut b = a + n1 + n2;
    loop {
        result += 1;
        if a >= n {
            a -= n;
            b -= n;
        } else if b >= n {
            break;
        }
        a <<= 1;
        b <<= 1;
    }
    result
}

/// `merge_compute_minrun(n)`
fn merge_compute_minrun(mut n: usize) -> usize {
    let mut r = 0;
    while n >= 64 {
        r |= n & 1;
        n >>= 1;
    }
    n + r
}

/// The permutation `sorted(range(n), key=...)` gives, where `lt(i, j)` is
/// `key(i) < key(j)` (`list_sort_impl`). On an exception, the sort stops
/// and returns it, like `list.sort`.
pub fn sort_permutation(n: usize, mut lt: impl FnMut(usize, usize) -> PyResult<bool>) -> PyResult<Vec<usize>> {
    let keys: Vec<usize> = (0..n).collect();
    if n < 2 {
        return Ok(keys);
    }
    let mut ms =
        MergeState { keys, tmp: Vec::new(), pending: Vec::new(), min_gallop: MIN_GALLOP, listlen: n, lt: &mut lt };
    let minrun = merge_compute_minrun(n);
    let mut lo = 0;
    let mut nremaining = n;
    loop {
        let (mut run, descending) = ms.count_run(lo, lo + nremaining)?;
        if descending {
            ms.keys[lo..lo + run].reverse();
        }
        if run < minrun {
            let force = if nremaining <= minrun { nremaining } else { minrun };
            ms.binarysort(lo, lo + force, lo + run)?;
            run = force;
        }
        ms.found_new_run(run)?;
        // at most log_phi(n) runs are pending (MAX_MERGE_PENDING = 85)
        debug_assert!(ms.pending.len() < 85, "progress guard (debug assertions: the `mutants` profile)");
        ms.pending.push(Run { base: lo, len: run, power: 0 });
        lo += run;
        nremaining -= run;
        if nremaining == 0 {
            break;
        }
    }
    ms.merge_force_collapse()?;
    Ok(ms.keys)
}

/// `items.sort(key=key)` with `lt` as `<` on keys: keys are computed first,
/// in order (the first error of `key` is raised before any comparison).
pub fn sort_by_key<T, K>(
    items: &mut Vec<T>,
    mut key: impl FnMut(&T) -> PyResult<K>,
    mut lt: impl FnMut(&K, &K) -> PyResult<bool>,
) -> PyResult<()> {
    let keys = items.iter().map(&mut key).collect::<PyResult<Vec<K>>>()?;
    let perm = sort_permutation(items.len(), |i, j| lt(&keys[i], &keys[j]))?;
    apply_permutation(items, &perm);
    Ok(())
}

/// `items.sort()` with `lt` as `<` on the items.
pub fn sort_by_lt<T>(items: &mut Vec<T>, mut lt: impl FnMut(&T, &T) -> PyResult<bool>) -> PyResult<()> {
    let perm = sort_permutation(items.len(), |i, j| lt(&items[i], &items[j]))?;
    apply_permutation(items, &perm);
    Ok(())
}

/// Reorders `items` so that position `k` holds the old `items[perm[k]]`.
fn apply_permutation<T>(items: &mut Vec<T>, perm: &[usize]) {
    let mut slots: Vec<Option<T>> = items.drain(..).map(Some).collect();
    items.extend(perm.iter().map(|&i| slots[i].take().expect("a permutation")));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorts_and_is_stable() {
        let mut v: Vec<(i32, usize)> = (0..1000).map(|i| ((i * 7919 % 13) as i32, i)).collect();
        sort_by_lt(&mut v, |a, b| Ok(a.0 < b.0)).unwrap();
        for w in v.windows(2) {
            assert!(w[0].0 < w[1].0 || (w[0].0 == w[1].0 && w[0].1 < w[1].1));
        }
    }

    #[test]
    fn minrun() {
        assert_eq!(merge_compute_minrun(63), 63);
        assert_eq!(merge_compute_minrun(64), 32);
        assert_eq!(merge_compute_minrun(65), 33);
    }
}
