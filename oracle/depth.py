"""Python's recursion depth where the oracle calls pdfminer.

CPython raises ``RecursionError`` from two counters: Python frames (at most
``sys.getrecursionlimit()``) and C recursion units (``repr``, ``str``,
comparisons, calls of builtins and from C into Python take some; see
``crates/pycompat/src/recursion.rs``). Where a ``RecursionError`` happens
therefore depends on the depth the code is called at. The Rust replays start
from the depth measured here, at the same call site.
"""

from __future__ import annotations

import sys
from typing import Any

__all__ = ["at_depth", "depth_here", "nested_list"]


def nested_list(n: int, leaf: Any = None) -> Any:
    """``[[...[leaf]...]]``, ``n`` lists deep (``[]`` at the bottom when
    ``leaf`` is None), built without recursion."""
    x = [] if leaf is None else [leaf]
    for _ in range(n - 1):
        x = [x]
    return x


_c_left_guess = 0


def depth_here(skip: int = 1) -> dict[str, int]:
    """The recursion depth of the caller (``skip`` frames up):
    ``{"py": <Python frames on the stack, the caller's included>,
    "c_left": <C recursion units left>}``, which the Rust replay gives to
    ``pycompat::recursion::with_depth``.

    ``c_left`` is measured: comparing two lists nested ``n`` deep takes ``n``
    C units (one ``PyObject_RichCompare`` per level, the unit ``repr()``
    takes too, without its quadratic string building), so it is the largest
    ``n`` whose comparison succeeds. A Python-to-Python call takes no C
    unit, so the caller has the same number left as this function.
    """
    global _c_left_guess
    frame = sys._getframe(skip)
    py = 0
    while frame is not None:
        py += 1
        frame = frame.f_back

    def fits(n: int) -> bool:
        try:
            return nested_list(n) == nested_list(n)
        except RecursionError:
            return False

    guess = _c_left_guess
    if guess and fits(guess) and not fits(guess + 1):
        return {"py": py, "c_left": guess}
    lo, hi = 0, 1 << 15
    while lo < hi:
        mid = (lo + hi + 1) // 2
        if fits(mid):
            lo = mid
        else:
            hi = mid - 1
    _c_left_guess = lo
    return {"py": py, "c_left": lo}


def at_depth(f: Any, *args: Any) -> tuple[dict[str, int], Any]:
    """``f(*args)``, or the exception it raised, with the depth it was called
    at (``depth_here`` of this frame). ``f(*args)`` runs a Python function
    inline (its frame is one more) and takes one C unit for a builtin."""
    depth = depth_here(1)
    try:
        return depth, f(*args)
    except Exception as e:  # the exception is the expected result
        return depth, e
