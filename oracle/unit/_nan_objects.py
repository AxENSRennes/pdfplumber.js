"""NaN objects whose place in a CPython ``set`` is known, and a check that
every set pdfplumber's clustering builds lists them where the port does.

``clustering.make_cluster_dict`` sorts ``list(set(values))``. With NaN
values, the order of that list decides the clusters, and CPython lists a set
by hash slot (``Objects/setobject.c``), where a NaN's hash is its address
(``_Py_HashPointer``: the address shifted right by 4 bits, rotated). Python's
result then depends on the memory layout. The port
(``crates/pdfplumber/src/utils/clustering.rs``) gives the result of the
layout where every NaN comes after the other values, NaNs in the order of
their first occurrence. The generators build their NaN coordinates from
``NanPool``, whose objects hash to chosen low bits, and run each case under
``SetOrderCheck``, which keeps the case only if CPython's set gives this
order, for any value of the hash bits not chosen.

- ``NanPool().take(rank, m, salt)``: a NaN object for the ``rank``-th of
  ``m`` NaN items (``rank`` 0 first, ``m`` at most 4), whose hash ends with
  the 16 bits ``salt << 12 | (0x1FF - 2 * (m - 1 - rank))``, and that this
  pool did not give before (one pool per case). In a table of 8 to 512
  slots, NaN ranks land in the top odd slots, in rank order (a float's
  address is 16 bytes past a multiple of 32 in its pymalloc pool, so its
  hash is odd). ``salt`` (0-15) tells apart the coordinates of one item,
  which never share a set.
- ``SetOrderCheck``: a profiler (``sys.setprofile``, observation only) that
  sees each ``cluster_list`` call made by ``make_cluster_dict`` (from
  ``cluster_objects``, or directly on a list), recomputes the keys of that
  ``cluster_objects`` call,
  and replays the set insertions (``set_order``, a transcription of
  ``set_add_entry``, ``set_table_resize`` and ``set_insert_clean`` of
  CPython 3.12). ``ok`` stays true when every set without NaN is any order,
  and every set with NaNs holds only ``NanPool`` objects (a NaN computed by
  pdfplumber has an address nobody chose), each once, among ``int``,
  ``float`` and ``bool`` values (``str`` and ``None`` hash by seed and
  address), and lists them last in their order of first occurrence under
  each of ``VARIANTS`` (other values of the hash bits above the 16 chosen).
  A NaN of a computed key (``-x["bottom"]``: ``key_fn`` gives a new object
  at each call) is never found again whatever its place: such a set is
  kept when its other values are all equal (their cluster indices are then
  compared with each other and with ``None`` along the same path).
  The replay with the real hashes must give the very list the call
  receives, and that list must satisfy the same order; anything else is an
  error of this module.
"""

from __future__ import annotations

import sys

from pdfplumber.utils import clustering

M64 = (1 << 64) - 1
LINEAR_PROBES = 9
PERTURB_SHIFT = 5
POOL_SIZE = 1 << 18

#: bits 16-59 of a NaN's hash in the replays (the top 4 bits of a pointer
#: hash are those of a 16-byte aligned address: zero)
VARIANTS = (0, 0xFFFFFFFFFFF, 0x55555555555, 0xAAAAAAAAAAA, 0x7D5262C1F3A)


def _signed(u: int) -> int:
    u &= M64
    return u - (1 << 64) if u >= 1 << 63 else u


class NanPool:
    """NaN float objects indexed by the low 16 bits of their hash."""

    _by_low: dict[int, list[float]] | None = None

    def __init__(self):
        if NanPool._by_low is None:
            by_low: dict[int, list[float]] = {}
            for _ in range(POOL_SIZE):
                o = float("nan")
                by_low.setdefault(hash(o) & 0xFFFF, []).append(o)
            NanPool._by_low = by_low
        self.used: dict[int, int] = {}

    def take(self, rank: int, m: int, salt: int) -> float:
        assert 0 <= rank < m <= 4 and 0 <= salt < 16, (rank, m, salt)
        low = salt << 12 | (0x1FF - 2 * (m - 1 - rank))
        k = self.used.get(low, 0)
        objs = NanPool._by_low.get(low, [])
        if k >= len(objs):
            raise RuntimeError(f"NanPool: no NaN object left whose hash ends with {low:#06x}")
        self.used[low] = k + 1
        return objs[k]

    @staticmethod
    def owns(o) -> bool:
        objs = NanPool._by_low.get(hash(o) & 0xFFFF, []) if NanPool._by_low else []
        return any(x is o for x in objs)


def _insert_clean(table, mask, key, h):
    perturb = h & M64
    i = perturb & mask
    while True:
        if table[i] is None:
            table[i] = (key, h)
            return
        if i + LINEAR_PROBES <= mask:
            for j in range(1, LINEAR_PROBES + 1):
                if table[i + j] is None:
                    table[i + j] = (key, h)
                    return
        perturb >>= PERTURB_SHIFT
        i = (i * 5 + 1 + perturb) & mask


def set_order(keys, hashes) -> list:
    """``list(set(iter(keys)))`` for keys of these hashes (no deletion, so no
    dummy entry): ``set_add_entry`` (equality: ``is``, then ``==``),
    ``set_table_resize``, ``set_insert_clean``."""
    mask = 7
    table = [None] * 8
    fill = used = 0
    for key, h in zip(keys, hashes):
        i = (h & M64) & mask
        perturb = h & M64
        while True:
            probes = LINEAR_PROBES if i + LINEAR_PROBES <= mask else 0
            j = i
            result = None
            while True:
                e = table[j]
                if e is None:
                    result = j
                    break
                if e[1] == h and (e[0] is key or e[0] == key):
                    result = "found"
                    break
                if probes == 0:
                    break
                probes -= 1
                j += 1
            if result is not None:
                break
            perturb >>= PERTURB_SHIFT
            i = (i * 5 + 1 + perturb) & mask
        if result == "found":
            continue
        table[result] = (key, h)
        fill += 1
        used += 1
        if fill * 5 >= mask * 3:
            minused = used * 2 if used > 50000 else used * 4
            newsize = 8
            while newsize <= minused:
                newsize <<= 1
            old, table, mask = table, [None] * newsize, newsize - 1
            for e in old:
                if e is not None:
                    _insert_clean(table, mask, e[0], e[1])
    return [e[0] for e in table if e is not None]


def _is_nan(v) -> bool:
    return isinstance(v, float) and v != v


def _model_order(listed, nans) -> bool:
    """Every NaN after every other value, NaNs in the order ``nans``."""
    at = [k for k, v in enumerate(listed) if _is_nan(v)]
    return at == list(range(len(listed) - len(at), len(listed))) and all(
        a is b for a, b in zip([listed[k] for k in at], nans)
    )


class SetOrderCheck:
    """``with SetOrderCheck() as check: ...`` then ``check.ok`` (see the
    module documentation)."""

    def __init__(self):
        self.ok = True
        self.sets = 0
        self.nan_sets = 0
        self.error: BaseException | None = None

    def __enter__(self):
        self._previous = sys.getprofile()
        sys.setprofile(self._hook)
        return self

    def __exit__(self, *exc):
        sys.setprofile(self._previous)
        if self.error is not None:
            raise RuntimeError("SetOrderCheck") from self.error
        return False

    def _hook(self, frame, event, arg):
        # an exception raised here would reach the profiled code (and its
        # ``except``): kept for __exit__
        if event != "call" or frame.f_code is not clustering.cluster_list.__code__ or self.error:
            return
        try:
            self._check(frame)
        except BaseException as e:  # noqa: BLE001
            self.error = e

    def _check(self, frame):
        parent = frame.f_back
        if parent is None or parent.f_code is not clustering.make_cluster_dict.__code__:
            return
        caller = parent.f_back
        if caller is not None and caller.f_code is clustering.cluster_objects.__code__:
            key_fn, xs = caller.f_locals["key_fn"], caller.f_locals["xs"]
        else:
            # make_cluster_dict(values, tolerance) called directly, on a list
            key_fn, xs = (lambda v: v), parent.f_locals["values"]
            if not isinstance(xs, list):
                raise RuntimeError("make_cluster_dict called on an iterator: its values are gone")
        self.sets += 1
        listed = list(frame.f_locals["xs"])
        values = [key_fn(x) for x in xs]
        if not any(_is_nan(v) for v in values):
            return
        self.nan_sets += 1
        if any(_is_nan(v) and key_fn(x) is not v for x, v in zip(xs, values)):
            # a computed key (a new object at each call): its NaNs are never
            # found again (cluster None), and their places in the set only
            # decide the cluster indices of the other values, which are
            # compared with each other and with None. When those values are
            # all equal, the result does not depend on the places.
            others = [v for v in values if not _is_nan(v)]
            if any(type(v) not in (int, float, bool) for v in others) or len(set(others)) > 1:
                self.ok = False
            elif any(key_fn(x) is v for x, v in zip(xs, values) if _is_nan(v)):
                self.ok = False  # stored and computed NaNs in one set
            return
        nans = []
        for v in values:
            if _is_nan(v):
                if any(v is n for n in nans) or not NanPool.owns(v):
                    self.ok = False
                    return
                nans.append(v)
            elif type(v) not in (int, float, bool):
                self.ok = False
                return
        real = set_order(values, [hash(v) for v in values])
        if len(real) != len(listed) or not all(a is b for a, b in zip(real, listed)):
            raise RuntimeError("set_order does not replay CPython's set")
        for high in VARIANTS:
            hashes = [_signed((hash(v) & 0xFFFF) | high << 16) if _is_nan(v) else hash(v) for v in values]
            if not _model_order(set_order(values, hashes), nans):
                self.ok = False
                return
        if not _model_order(listed, nans):
            raise RuntimeError("a NaN order that no variant predicted")
