"""Python's recursion depth where the oracle calls pdfminer.

CPython raises ``RecursionError`` from two counters: Python frames (at most
``sys.getrecursionlimit()``) and C recursion units (``repr``, ``str``,
comparisons, calls of builtins and from C into Python take some; see
``crates/pycompat/src/recursion.rs``). Where a ``RecursionError`` happens
therefore depends on the depth the code is called at. The Rust replays start
from the depth measured here, at the same call site.
"""

from __future__ import annotations

import dis
import sys
from typing import Any

__all__ = ["at_depth", "depth_here", "executed_call_sites", "nested_list", "warm_call_sites"]


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


def _call_sites() -> dict[str, tuple[Any, int, tuple]]:
    """The call sites whose first execution in a process takes one C unit
    more (``crates/pycompat/src/specialize.rs``), as the port names them
    (``pdftrace::stages::py_process::CALL_SITES``): ``name -> (function,
    line of the call, arguments that run it)``."""
    from pdfminer.psparser import literal_name

    return {"psparser.literal_name str(x)": (literal_name, 121, (0,))}


def executed_call_sites() -> list[str]:
    """The call sites of ``_call_sites`` this process has executed. CPython
    3.12 quickens a code object when it creates it, each ``CALL`` with its
    adaptive counter at the warm-up value (``1 << 4 | 1``: value 1, backoff
    1); the first execution decrements the value to 0 and runs the generic
    ``CALL``, the second specializes the instruction (``CALL_NO_KW_STR_1``).
    A site is executed when its instruction is specialized or the value of
    its counter (the bits above the 4 backoff bits) is 0."""
    out = []
    for name, (func, line, _) in _call_sites().items():
        code = func.__code__
        raw = code._co_code_adaptive
        calls = [i for i in dis.get_instructions(code, adaptive=True)
                 if i.positions.lineno == line and i.opname.startswith("CALL")]
        if len(calls) != 1:
            raise RuntimeError(f"{name}: {len(calls)} calls on line {line}")
        counter = int.from_bytes(raw[calls[0].offset + 2:calls[0].offset + 4], "little")
        if calls[0].opname != "CALL" or counter >> 4 == 0:
            out.append(name)
    return out


def warm_call_sites() -> None:
    """Executes each call site of ``_call_sites``: the state of a process
    that ran them, which a generator's cases then share (its Rust replay
    sets it with ``pycompat::specialize::set_executed``)."""
    for func, _, args in _call_sites().values():
        func(*args)
    missing = set(_call_sites()) - set(executed_call_sites())
    if missing:
        raise RuntimeError(f"not executed: {sorted(missing)}")
