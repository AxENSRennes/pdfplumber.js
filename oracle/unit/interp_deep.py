"""Depth sweeps of the interpreter's device paths (``pdfinterp``,
``pdfdevice``), replayed by ``crates/pdftrace/tests/interp_unit.rs``
(``deep_sweeps``).

Where a ``RecursionError`` happens is the earliest point, in time, deeper
than the recursion limit. In the ``interp_text`` stage, the text device
returns at once, and every operator path of an ``execute`` stays shallower
than the frames the content parser reached before that ``execute``'s first
operator (8 frames below ``execute`` at least, measured on ``adv-interp``
and on its depth chains): a frame of ``do_TJ``, ``render_string``,
``render_string_horizontal``, ``do_S``... miscounted would never show
there. With pdfplumber's device, the converter's objects (``LTChar``,
``LTCurve``, ``LTFigure``, ``LTImage``) go deeper than the parser, and those
frames decide where pdfplumber raises.

So this generator traces the files of ``adv-interp`` (all but the
``depth-*`` chains) with ``DeepTracingDevice``: the ``TracingDevice`` of
``oracle/stages/interp_text.py`` whose every method first descends
``DESCENT`` frames (``descend``), which makes each device call deeper than
anything before it on its page (each page is interpreted by its own device
and interpreter, a ``RecursionError`` stops only its page). Each file is
traced from a range of deeper entries (``nested`` generator frames, the
stage body's depth recorded per trace, ``oracle.depth.depth_here``) that
moves the recursion limit one frame at a time from just above the deepest
frame of the file's run (``max_depth``, measured with a bounded profile
hook) down ``SWEEP`` frames: every frame on a path to a device call, and on
the paths deeper than those, is then the first one past the limit in one of
the traces.

Cases (``deep.jsonl``): ``{"in": [name, pdf, password, descent, entries],
"out": [first, digests]}``: the trace of each entry (as ``interp_text``
writes it, with ``DeepTracingDevice``), the first one in full (the limit
above the deepest frame: no ``RecursionError``), and the SHA-256 of each.
Every trace runs in a process forked for it (``document.in_child``), from
the state ``py_process.jsonl`` records.

Usage: PDFPLUMBER_ROOT=<main checkout> <root>/.venv/bin/python -m oracle.unit.interp_deep
"""

from __future__ import annotations

import hashlib
import sys
from collections.abc import Iterator
from typing import Any

from pdfminer.pdfinterp import PDFResourceManager
from pdfminer.pdfpage import PDFPage

from ..codegen import cmaps as codegen_cmaps
from ..corpus import cache_dir, load_set
from ..depth import depth_here
from ..stages import _in_stage_thread
from ..stages._common import line, open_document, terminated
from ..stages.interp import _DO_EI, Fonts, _Page, page_layout
from ..stages.interp_text import TextDevice
from ._common import require_oracle_python, write_cases, write_meta
from .document import in_child, process_state

MODULE = "interp_deep"
MAX_PAGES = 20
#: Frames each method of ``DeepTracingDevice`` descends before its body.
DESCENT = 60
#: Frames the sweep moves the limit down from above a file's deepest frame.
SWEEP = 16
#: Python's recursion limit (``sys.getrecursionlimit()``, never changed).
LIMIT = 1000


def descend(n: int) -> None:
    """``n`` nested frames that return at once."""
    if n > 1:
        descend(n - 1)


class DeepTracingDevice(TextDevice):
    """``interp_text.TracingDevice``, each method descending ``DESCENT``
    frames first (its body is copied rather than called through ``super()``,
    so that ``sys._getframe(1)`` is still the interpreter's method)."""

    calls: list[tuple[Any, ...]]

    def set_ctm(self, ctm):
        descend(DESCENT)
        self.calls.append(("set_ctm", ctm))
        try:
            super().set_ctm(ctm)
        except Exception as e:
            self.calls.append(("raised", e))
            raise

    def begin_tag(self, tag, props=None):
        descend(DESCENT)
        self.calls.append(("begin_tag", tag, props))
        try:
            super().begin_tag(tag, props)
        except Exception as e:
            self.calls.append(("raised", e))
            raise

    def end_tag(self):
        descend(DESCENT)
        self.calls.append(("end_tag",))
        try:
            super().end_tag()
        except Exception as e:
            self.calls.append(("raised", e))
            raise

    def do_tag(self, tag, props=None):
        descend(DESCENT)
        self.calls.append(("do_tag", tag, props))
        try:
            super().do_tag(tag, props)
        except Exception as e:
            self.calls.append(("raised", e))
            raise

    def begin_page(self, page, ctm):
        descend(DESCENT)
        self.calls.append(("begin_page", page.pageid, ctm))
        try:
            super().begin_page(page, ctm)
        except Exception as e:
            self.calls.append(("raised", e))
            raise

    def end_page(self, page):
        descend(DESCENT)
        self.calls.append(("end_page", page.pageid))
        try:
            super().end_page(page)
        except Exception as e:
            self.calls.append(("raised", e))
            raise

    def begin_figure(self, name, bbox, matrix):
        descend(DESCENT)
        self.calls.append(("begin_figure", name, sys._getframe(1).f_code is _DO_EI, bbox, matrix))
        try:
            super().begin_figure(name, bbox, matrix)
        except Exception as e:
            self.calls.append(("raised", e))
            raise

    def end_figure(self, name):
        descend(DESCENT)
        self.calls.append(("end_figure", name, sys._getframe(1).f_code is _DO_EI))
        try:
            super().end_figure(name)
        except Exception as e:
            self.calls.append(("raised", e))
            raise

    def render_image(self, name, stream):
        descend(DESCENT)
        self.calls.append(("render_image", name, sys._getframe(1).f_code is _DO_EI, stream))
        try:
            super().render_image(name, stream)
        except Exception as e:
            self.calls.append(("raised", e))
            raise

    def paint_path(self, graphicstate, stroke, fill, evenodd, path):
        descend(DESCENT)
        self.calls.append(("paint_path", dict(vars(graphicstate)), stroke, fill, evenodd, list(path)))
        try:
            super().paint_path(graphicstate, stroke, fill, evenodd, path)
        except Exception as e:
            self.calls.append(("raised", e))
            raise

    def render_string(self, textstate, seq, ncs, graphicstate):
        descend(DESCENT)
        self.calls.append(("render_string", dict(vars(textstate)), seq, ncs, dict(vars(graphicstate))))
        try:
            super().render_string(textstate, seq, ncs, graphicstate)
        except Exception as e:
            self.calls.append(("raised", e))
            raise
        self.calls.append(("end_string", textstate.linematrix))

    def render_char(self, matrix, font, fontsize, scaling, rise, cid, ncs, graphicstate):
        descend(DESCENT)
        try:
            adv = super().render_char(matrix, font, fontsize, scaling, rise, cid, ncs, graphicstate)
        except Exception as e:
            self.calls.append(("render_char", matrix, font, fontsize, scaling, rise, cid, ncs, graphicstate, e))
            raise
        self.calls.append(("render_char", matrix, font, fontsize, scaling, rise, cid, ncs, graphicstate, adv))
        return adv


@terminated
def deep_run(path: str, password: str | None, max_pages: int) -> Iterator[str]:
    """``interp_text.run`` with ``DeepTracingDevice``."""
    with open_document(path, password) as doc:
        rsrcmgr = PDFResourceManager()
        fonts = Fonts()
        for pageno, page in enumerate(PDFPage.create_pages(doc), start=1):
            yield line("page", pageno, page.pageid)
            device, _, error = page_layout(rsrcmgr, page, pageno, DeepTracingDevice)
            fmt = _Page(fonts)
            for c in device.calls:
                fmt.call(c)
            yield from fmt.lines
            if error is not None:
                yield line("error", error)
            if max_pages and pageno >= max_pages:
                break


def nested(k: int, make):
    """The generator ``make()`` resumed through ``k`` more generator frames
    (built without recursion: each level is a generator frame on the stack
    while it runs)."""

    def level(inner):
        yield from inner

    gen = make()
    for _ in range(k):
        gen = level(gen)
    return gen


def entry_at(k: int) -> dict[str, int]:
    """The depth of a stage body run through ``nested(k, ...)``."""

    @terminated
    def probe(path: str, password: str | None, max_pages: int):
        depth = depth_here()
        yield f"{depth['py']} {depth['c_left']}"

    py, c_left = _in_stage_thread(lambda: nested(k, lambda: probe("", None, 0)))[0].split()
    return {"py": int(py), "c_left": int(c_left)}


def max_depth(path: str, password: str | None) -> int:
    """The deepest frame of ``deep_run`` on ``path``, counted from its body
    (the body's callees are 1): a profile hook keeping two integers."""
    state = {"depth": 0, "max": 0}

    def hook(frame, event, arg) -> None:
        if event == "call":
            state["depth"] += 1
            if state["depth"] > state["max"]:
                state["max"] = state["depth"]
        elif event == "return":
            state["depth"] -= 1

    @terminated
    def probe(path: str, password: str | None, max_pages: int):
        sys.setprofile(hook)
        try:
            # deep_run's body is one frame below this one
            yield from deep_run.__wrapped__(path, password, max_pages)
        finally:
            sys.setprofile(None)

    _in_stage_thread(lambda: probe(path, password, MAX_PAGES))
    return state["max"] - 1


def traced_from(path: str, password: str | None, k: int) -> tuple[dict[str, int], str]:
    entry = entry_at(k)
    lines = _in_stage_thread(lambda: nested(k, lambda: deep_run(path, password, MAX_PAGES)))
    return entry, "".join(x + "\n" for x in lines)


def deep_cases() -> Iterator[tuple[list, Any]]:
    base = in_child(lambda: entry_at(0))["py"]
    for f in load_set("adv-interp"):
        if f.path.name[4:].startswith("depth-"):
            continue
        deepest = in_child(lambda: max_depth(str(f.path), f.password))
        entries, first, digests = [], None, []
        # k such that the frame at `deepest` below the body is the first
        # past the limit (shift 0), then one frame higher each time
        for shift in range(-1, SWEEP):
            k = LIMIT + 1 - base - deepest + shift
            if k < 0:
                continue
            entry, text = in_child(lambda: traced_from(str(f.path), f.password, k))
            entries.append(entry)
            if first is None:
                first = text
            digests.append(hashlib.sha256(text.encode("ascii")).hexdigest())
        yield [f.path.name, f.path.read_bytes(), f.password, DESCENT, entries], [first, digests]


def main() -> None:
    require_oracle_python()
    cmap_dir = codegen_cmaps.ensure()
    rel_cmap_dir = cmap_dir.relative_to(cache_dir()).as_posix()
    state = process_state()
    write_cases(MODULE, "py_process", [([], state)])
    n = write_cases(MODULE, "deep", deep_cases())
    if process_state() != state:
        raise RuntimeError("the process state changed while the traces ran in children")
    print(f"{MODULE}: {n} files swept", file=sys.stderr)
    write_meta(MODULE, cmap_dir=rel_cmap_dir, descent=DESCENT, sweep=SWEEP)


if __name__ == "__main__":
    main()
