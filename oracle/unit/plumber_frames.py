"""The frames and C units of ``Page.process_object``, replayed by
``crates/pdftrace/tests/plumber_frames.rs``.

The stage traces never reach the recursion limit inside
``process_object``: on a fresh page the interpretation goes deeper first,
and the sweeps of ``oracle/unit/plumber_sweep.py`` only see whether an
operation raises, so only the deepest chain of frames of ``page.objects``
counts there. Yet a page whose layout is already computed (``page.layout``,
then ``page.objects`` from deep in a program) runs ``process_object`` as
the deepest code, each kind of object along its own chain: ``process_attr``
called by ``map`` from C, ``resolve_all`` and its generator expressions,
``decode_text`` of a char's colour space name, ``point2coord`` of a curve's
points and path, ``Page.height``. This generator pins each of them: for
each layout object of the first page (``laparams`` None: the leaves, the
figures flattened, as ``iter_layout_objects`` yields them), after one pass
over all of them (the call sites the adaptive interpreter specializes on a
first execution are then executed), the deepest entry from which
``page.process_object(obj)`` still runs without ``RecursionError``, along
each counter:

- ``py``: the caller of ``process_object`` under ``k`` more Python frames
  (``down``: Python-to-Python calls, no C unit);
- ``c``: the caller, called by ``Leaf.__eq__``, reached through ``n``
  nested one-item lists compared with ``==`` (one C unit per level, then
  the ``__eq__`` called from C).

``k`` and ``n`` are bisected, then ``k + 1`` and ``n + 1`` checked to
raise. Each attempt runs in a process forked for it from the same frame
(every attempt has the same base and the same history: what an attempt
specializes or raises is gone with its process). The depth of the caller
(``depth_here``) is measured at ``k = 0`` and ``n = 0`` through the same
path and moved by ``k`` frames or ``n`` units (checked at a few levels):
never measured near the limit, where the measure itself would raise.

``functions.jsonl`` holds the same for ``decode_text`` and ``resolve_all``
on their own (``FUNCTION_ARGS``: the arguments ``process_object`` gives
them, whose paths the port speeds up), where their frames are the deepest:
one case, ``{"in": [], "out": [[function, typed JSON of the argument, py
depth, py outcome, c depth, c outcome], ...]}``.

Cases (``objects.jsonl``): ``{"in": [name, pdf, password], "out": [[class
name, py depth, py outcome, c depth, c outcome], ...]}``, each depth
``{"py", "c_left"}`` of the caller at the deepest entry from which no
``RecursionError`` escapes, each outcome ``"ok"`` or the canonical line of
the exception ``process_object`` raised there (a malformed MediaBox; a
``RecursionError`` pdfminer wraps, such as the ``MalformedPDFException``
of an image's colour space at the C limit). Every file runs in a process forked for it
(``document.in_child``), from the state ``py_process.jsonl`` records,
inside ``_in_stage_thread`` as the stages run.

Usage: PDFPLUMBER_ROOT=<main checkout> <root>/.venv/bin/python -m oracle.unit.plumber_frames
"""

from __future__ import annotations

import json
import os
import sys
import warnings
from collections.abc import Iterator
from typing import Any

from ..codegen import cmaps as codegen_cmaps
from ..corpus import cache_dir, load_set
from ..depth import depth_here
from ..stages import _in_stage_thread
from ..stages._common import terminated
from ..stages.plumber_objects import exc, open_pdf
from ._common import require_oracle_python, write_cases, write_meta
from .document import in_child, process_state

MODULE = "plumber_frames"
#: Python's recursion limit and CPython 3.12's ``Py_C_RECURSION_LIMIT``.
PY_LIMIT, C_LIMIT = 1000, 10000
#: Files of ``micro`` (curves, rects, lines, images, figures, colour spaces,
#: text of every kind).
MICRO = ("vector-objects.pdf", "graphics-state-colors.pdf", "image-xobject.pdf", "images-advanced.pdf",
         "form-xobject-matrix.pdf", "text-operators.pdf", "page-boxes-rotate.pdf", "type3-simple.pdf",
         "lone-surrogates.pdf", "encodings-cmap.pdf", "vertical-rtl-text.pdf")
#: Files of ``adv-plumber`` (by the name after the number): MediaBoxes of
#: every kind a page opens with (``point2coord``, the ``x0`` shift), dashes,
#: colours, images, marked content.
ADV = ("box-media-origin.pdf", "box-media-inverted.pdf", "box-media-mixed.pdf", "box-media-bools.pdf",
       "box-media-refs.pdf", "box-media-inherited.pdf", "geom-boundaries.pdf",
       "geom-infinite.pdf", "paint-dash.pdf", "paint-colors.pdf", "image-kinds.pdf", "mc-mcid-kinds.pdf",
       "mc-props.pdf", "mc-figure.pdf", "font-kinds.pdf")
#: Objects per file: the first ones, then the first of each class not seen.
MAX_OBJECTS = 20


def leaves(objs: list[Any]) -> Iterator[Any]:
    """The objects ``iter_layout_objects`` processes without ``laparams``:
    the leaves, the containers' children in order."""
    from pdfminer.layout import LTContainer

    for obj in objs:
        if isinstance(obj, LTContainer):
            yield from leaves(obj._objs)
        else:
            yield obj


def sample(objs: list[Any]) -> list[Any]:
    out = objs[:MAX_OBJECTS]
    seen = {type(o) for o in out}
    for o in objs[MAX_OBJECTS:]:
        if type(o) not in seen:
            seen.add(type(o))
            out.append(o)
    return out


def down(k: int, f: Any) -> Any:
    """``f()`` under ``k`` more Python frames."""
    return f() if k == 0 else down(k - 1, f)


class Leaf:
    """Compared with ``==`` at the bottom of nested lists: its ``__eq__``,
    called from C, runs ``self.f`` (the caller of ``process_object``)."""

    def __init__(self, f: Any) -> None:
        self.f = f
        self.result = None

    def __eq__(self, other: object) -> bool:
        self.result = self.f()
        return True

    __hash__ = None  # type: ignore[assignment]


def nested(n: int, leaf: Any) -> list[Any]:
    x: Any = leaf
    for _ in range(n):
        x = [x]
    return x


def under_c(n: int, f: Any) -> Any:
    """``f()`` from ``Leaf.__eq__`` under ``n`` list comparisons."""
    leaf = Leaf(f)
    nested(n, leaf) == nested(n, Leaf(None))  # noqa: B015  (the comparison runs f)
    return leaf.result


def forked(f: Any) -> Any:
    """``f()`` (a JSON value) in a process forked for it, from this thread:
    each attempt starts from the same state. Python's own history matters
    near the limit (in one process, an attempt that raised moves where the
    next ones raise by a frame), so no attempt sees another's."""
    r, w = os.pipe()
    pid = os.fork()
    if pid == 0:
        os.close(r)
        try:
            data = json.dumps({"value": f()}).encode()
        except BaseException as e:  # noqa: BLE001  (reported to the parent)
            data = json.dumps({"error": repr(e)}).encode()
        with os.fdopen(w, "wb") as out:
            out.write(data)
        os._exit(0)
    os.close(w)
    with os.fdopen(r, "rb") as inp:
        data = inp.read()
    os.waitpid(pid, 0)
    result = json.loads(data)
    if "error" in result:
        raise RuntimeError(f"in a forked attempt: {result['error']}")
    return result["value"]


def limits(f: Any, arg: Any, what: str) -> list[Any]:
    """``[py depth, py outcome, c depth, c outcome]`` of ``f(arg)`` (see the
    module docstring): the depth of ``f``'s caller at the deepest entry from
    which no ``RecursionError`` escapes, along each counter, and the outcome
    there."""
    mode = {"measure": False}

    def call() -> Any:
        """The caller of ``f``: the outcome, or (when measuring, far from
        the limit) its depth."""
        if mode["measure"]:
            return depth_here(1)
        try:
            f(arg)
            return "ok"
        except RecursionError:
            raise
        except Exception as e:  # noqa: BLE001  (the outcome)
            return exc(e)

    def attempt(counter: str, x: int, measure: bool = False) -> Any:
        """``call`` ``x`` frames (``py``) or list levels (``c``) down, in a
        process forked for it; ``None`` for a RecursionError. Always made
        from this frame: every attempt has the same base."""

        def run() -> Any:
            mode["measure"] = measure
            try:
                return down(x, call) if counter == "py" else under_c(x, call)
            except RecursionError:
                return None

        return forked(run)

    row: list[Any] = []
    for counter, hi in (("py", PY_LIMIT), ("c", C_LIMIT)):
        # the caller's depth at x = 0, moved by one frame (py) or one unit
        # (c) per level: checked at a few levels
        base = attempt(counter, 0, measure=True)

        def at(x: int) -> dict[str, int]:
            if counter == "py":
                return {"py": base["py"] + x, "c_left": base["c_left"]}
            return {"py": base["py"], "c_left": base["c_left"] - x}

        for x in (1, 7, 30):
            if attempt(counter, x, measure=True) != at(x):
                raise RuntimeError(f"{what}: {counter} {x} levels down is not {at(x)}")
        lo = 0
        if attempt(counter, 0) is None:
            raise RuntimeError(f"{what}: RecursionError with no descent")
        while lo < hi:
            mid = (lo + hi + 1) // 2
            if attempt(counter, mid) is not None:
                lo = mid
            else:
                hi = mid - 1
        if attempt(counter, lo + 1) is not None:
            raise RuntimeError(f"{what}: {counter} not monotone at {lo}")
        row += [at(lo), attempt(counter, lo)]
    return row


def object_cases(page: Any, objs: list[Any]) -> list[list[Any]]:
    out = []
    for obj in objs:
        out.append([type(obj).__name__, *limits(page.process_object, obj, type(obj).__name__)])
    return out


#: Arguments of the functions ``process_object`` calls whose performance
#: paths the port has: ``decode_text`` of a ``str`` (one PDFDocEncoding maps
#: to itself, one it changes, one past 255: ``str(s)``, the empty one) and
#: ``resolve_all`` of the values ``process_object`` makes (numbers, a
#: ``str``, tuples and lists of them, nested).
FUNCTION_ARGS: list[tuple[str, Any]] = [
    ("decode_text", "DeviceGray"),
    ("decode_text", "\x80\x9fab"),
    ("decode_text", "a\u0100b"),
    ("decode_text", ""),
    ("resolve_all", 1.5),
    ("resolve_all", 3),
    ("resolve_all", "x"),
    ("resolve_all", None),
    ("resolve_all", (1.0, 0.0, 0.0, 1.0, 2.5, 3.5)),
    ("resolve_all", [(1.0, 2.0), (3.0, 4.0)]),
    ("resolve_all", [[[1.0]]]),
    ("resolve_all", ()),
]


def function_cases() -> list[list[Any]]:
    """``[[function, typed JSON of the argument, py depth, py outcome, c
    depth, c outcome], ...]`` (``FUNCTION_ARGS``), in the stage thread."""
    from pdfplumber.utils import decode_text, resolve_all

    from ..typed_json import dumps

    functions = {"decode_text": decode_text, "resolve_all": resolve_all}
    box: dict[str, Any] = {}

    @terminated
    def body(path_: str, password_: str | None, max_pages: int) -> Iterator[str]:
        # one pass: every call site executed once
        for name, arg in FUNCTION_ARGS:
            functions[name](arg)
        box["cases"] = [[name, dumps(arg), *limits(functions[name], arg, f"{name}({arg!r})")]
                        for name, arg in FUNCTION_ARGS]
        yield "!done"

    warnings.filterwarnings("ignore", "This process .* is multi-threaded", DeprecationWarning)
    lines = _in_stage_thread(lambda: body("", None, 0))
    if "cases" not in box:
        raise RuntimeError(f"functions: {lines}")
    return box["cases"]


def file_case(path: str, password: str | None) -> list[list[Any]]:
    """The cases of a file, computed in the stage thread (a fresh process
    forked by the caller)."""
    box: dict[str, Any] = {}

    @terminated
    def body(path_: str, password_: str | None, max_pages: int) -> Iterator[str]:
        pdf = open_pdf(path, password, 1)
        page = pdf.pages[0]
        objs = sample(list(leaves(page.layout._objs)))
        # one pass: every call site executed once
        for obj in objs:
            try:
                page.process_object(obj)
            except Exception:  # noqa: BLE001, S110  (the outcome is measured below)
                pass
        box["cases"] = object_cases(page, objs)
        yield "!done"

    # (the attempts fork from the stage thread: the process has two threads)
    warnings.filterwarnings("ignore", "This process .* is multi-threaded", DeprecationWarning)
    lines = _in_stage_thread(lambda: body(path, password, 0))
    if "cases" not in box:
        raise RuntimeError(f"{path}: {lines}")
    return box["cases"]


def cases() -> Iterator[tuple[list, Any]]:
    micro = {f.path.name: f for f in load_set("micro")}
    adv = {f.path.name[4:]: f for f in load_set("adv-plumber")}
    files = [micro[n] for n in MICRO] + [adv[n] for n in ADV]
    for f in files:
        out = in_child(lambda: file_case(str(f.path), f.password))
        if not out:
            raise RuntimeError(f"{f.path.name}: no object")
        yield [f.path.name, f.path.read_bytes(), f.password], out


def main() -> None:
    require_oracle_python()
    cmap_dir = codegen_cmaps.ensure()
    rel_cmap_dir = cmap_dir.relative_to(cache_dir()).as_posix()
    state = process_state()
    write_cases(MODULE, "py_process", [([], state)])
    n = write_cases(MODULE, "objects", cases())
    write_cases(MODULE, "functions", [([], in_child(function_cases))])
    if process_state() != state:
        raise RuntimeError("the process state changed while the cases ran in children")
    print(f"{MODULE}: {n} files", file=sys.stderr)
    write_meta(MODULE, cmap_dir=rel_cmap_dir)


if __name__ == "__main__":
    main()
