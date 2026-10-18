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
traced with its pages interpreted from a range of depths (``down``: ``n``
more frames around ``page_layout``, recorded per trace) that moves the
recursion limit one frame at a time from just above the deepest frame of
the pages' interpretation (``max_depth``, measured with a bounded profile
hook) down ``SWEEP`` frames: every frame on a path to a device call, and on
the paths deeper than those, is then the first one past the limit in one of
the traces. Each file is also swept with ``descent`` 0 (the stage's own
device) from above its deepest frame down to ``page_layout``'s own frame,
one frame at a time: what a page does before its first device call (``init_resources``,
fonts, color spaces, ``/ProcSet``) is then reached too, which the descent
of ``begin_page`` hides.

``api.jsonl``: ``PDFResourceManager.get_cmap(name, strict)``, which
pdfminer and pdfplumber never call on a page: the class, ``is_vertical``
and the CIDs of a few codes of the result, or the exception, for
predefined, identity and missing names; and the same call from every
depth down to the recursion limit (a process forked for each:
``CMapDB``'s caches start empty).

Cases (``deep.jsonl``): ``{"in": [name, pdf, password, descent, base,
frames], "out": [first, digests]}``: ``base`` the depth of the stage body
(``STAGE_ENTRY`` of the replay), the trace for each ``n`` of ``frames`` (as
``interp_text`` writes it, with ``DeepTracingDevice``, pages ``n + 2``
frames deeper), the first one in full (the limit above the deepest frame:
no ``RecursionError``), and the SHA-256 of each.
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
from ..canon import canon
from ..corpus import cache_dir, load_set
from ..depth import depth_here
from ..stages import _in_stage_thread
from ..stages._common import line, open_document, terminated
from ..stages.interp import _DO_EI, Fonts, _Page, page_layout
from ..stages.interp_text import TextDevice, TracingDevice
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
    descent = DESCENT

    def set_ctm(self, ctm):
        descend(self.descent)
        self.calls.append(("set_ctm", ctm))
        try:
            super().set_ctm(ctm)
        except Exception as e:
            self.calls.append(("raised", e))
            raise

    def begin_tag(self, tag, props=None):
        descend(self.descent)
        self.calls.append(("begin_tag", tag, props))
        try:
            super().begin_tag(tag, props)
        except Exception as e:
            self.calls.append(("raised", e))
            raise

    def end_tag(self):
        descend(self.descent)
        self.calls.append(("end_tag",))
        try:
            super().end_tag()
        except Exception as e:
            self.calls.append(("raised", e))
            raise

    def do_tag(self, tag, props=None):
        descend(self.descent)
        self.calls.append(("do_tag", tag, props))
        try:
            super().do_tag(tag, props)
        except Exception as e:
            self.calls.append(("raised", e))
            raise

    def begin_page(self, page, ctm):
        descend(self.descent)
        self.calls.append(("begin_page", page.pageid, ctm))
        try:
            super().begin_page(page, ctm)
        except Exception as e:
            self.calls.append(("raised", e))
            raise

    def end_page(self, page):
        descend(self.descent)
        self.calls.append(("end_page", page.pageid))
        try:
            super().end_page(page)
        except Exception as e:
            self.calls.append(("raised", e))
            raise

    def begin_figure(self, name, bbox, matrix):
        descend(self.descent)
        self.calls.append(("begin_figure", name, sys._getframe(1).f_code is _DO_EI, bbox, matrix))
        try:
            super().begin_figure(name, bbox, matrix)
        except Exception as e:
            self.calls.append(("raised", e))
            raise

    def end_figure(self, name):
        descend(self.descent)
        self.calls.append(("end_figure", name, sys._getframe(1).f_code is _DO_EI))
        try:
            super().end_figure(name)
        except Exception as e:
            self.calls.append(("raised", e))
            raise

    def render_image(self, name, stream):
        descend(self.descent)
        self.calls.append(("render_image", name, sys._getframe(1).f_code is _DO_EI, stream))
        try:
            super().render_image(name, stream)
        except Exception as e:
            self.calls.append(("raised", e))
            raise

    def paint_path(self, graphicstate, stroke, fill, evenodd, path):
        descend(self.descent)
        self.calls.append(("paint_path", dict(vars(graphicstate)), stroke, fill, evenodd, list(path)))
        try:
            super().paint_path(graphicstate, stroke, fill, evenodd, path)
        except Exception as e:
            self.calls.append(("raised", e))
            raise

    def render_string(self, textstate, seq, ncs, graphicstate):
        descend(self.descent)
        self.calls.append(("render_string", dict(vars(textstate)), seq, ncs, dict(vars(graphicstate))))
        try:
            super().render_string(textstate, seq, ncs, graphicstate)
        except Exception as e:
            self.calls.append(("raised", e))
            raise
        self.calls.append(("end_string", textstate.linematrix))

    def render_char(self, matrix, font, fontsize, scaling, rise, cid, ncs, graphicstate):
        descend(self.descent)
        try:
            adv = super().render_char(matrix, font, fontsize, scaling, rise, cid, ncs, graphicstate)
        except Exception as e:
            self.calls.append(("render_char", matrix, font, fontsize, scaling, rise, cid, ncs, graphicstate, e))
            raise
        self.calls.append(("render_char", matrix, font, fontsize, scaling, rise, cid, ncs, graphicstate, adv))
        return adv


#: ``[n]``: ``deep_run`` interprets each page from ``n + 2`` frames deeper
#: than the stage does (``down``), set in the process forked for a trace.
PAGE_FRAMES = [-1]


def set_descent(descent: int, n: int = -1) -> None:
    """The device of ``deep_run`` (in the process forked for a trace):
    ``DeepTracingDevice`` descending ``descent`` frames, or for ``0`` the
    stage's ``TracingDevice``; and the depth of its pages (``n``: below,
    ``-1``: the stage's)."""
    DeepTracingDevice.descent = descent
    PAGE_FRAMES[0] = n


def down(n: int, f):
    """``f()`` called through ``n + 1`` more frames (``down`` itself)."""
    return f() if n <= 0 else down(n - 1, f)


@terminated
def deep_run(path: str, password: str | None, max_pages: int) -> Iterator[str]:
    """``interp_text.run`` with the device ``set_descent`` chose."""
    device_class = DeepTracingDevice if DeepTracingDevice.descent else TracingDevice
    with open_document(path, password) as doc:
        rsrcmgr = PDFResourceManager()
        fonts = Fonts()
        for pageno, page in enumerate(PDFPage.create_pages(doc), start=1):
            yield line("page", pageno, page.pageid)
            if PAGE_FRAMES[0] < 0:
                device, _, error = page_layout(rsrcmgr, page, pageno, device_class)
            else:
                # n + 1 frames of down(), one of the lambda: page_layout's
                # frame is n + 2 frames deeper than the stage's
                device, _, error = down(PAGE_FRAMES[0], lambda: page_layout(rsrcmgr, page, pageno, device_class))
            fmt = _Page(fonts)
            for c in device.calls:
                fmt.call(c)
            yield from fmt.lines
            if error is not None:
                yield line("error", error)
            if max_pages and pageno >= max_pages:
                break


def entry_at() -> dict[str, int]:
    """The depth of the stage body (``oracle.stages.entry_depth``)."""

    @terminated
    def probe(path: str, password: str | None, max_pages: int):
        depth = depth_here()
        yield f"{depth['py']} {depth['c_left']}"

    py, c_left = _in_stage_thread(lambda: probe("", None, 0))[0].split()
    return {"py": int(py), "c_left": int(c_left)}


def max_depth(path: str, password: str | None, descent: int) -> int:
    """The deepest frame of the pages' interpretation (``page_layout`` and
    below) in ``deep_run`` on ``path``, counted from the stage body
    (``page_layout`` is 1): a profile hook keeping a few integers."""
    set_descent(descent)
    state = {"depth": 0, "max": 0, "inside": 0}
    target = page_layout.__code__

    def hook(frame, event, arg) -> None:
        if event == "call":
            state["depth"] += 1
            if frame.f_code is target:
                state["inside"] = state["depth"]
            if state["inside"] and state["depth"] > state["max"]:
                state["max"] = state["depth"]
        elif event == "return":
            if state["depth"] == state["inside"]:
                state["inside"] = 0
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


def traced_from(path: str, password: str | None, n: int, descent: int) -> str:
    set_descent(descent, n)
    lines = _in_stage_thread(lambda: deep_run(path, password, MAX_PAGES))
    return "".join(x + "\n" for x in lines)


def deep_cases() -> Iterator[tuple[list, Any]]:
    base = in_child(entry_at)
    for f in load_set("adv-interp"):
        if f.path.name[4:].startswith("depth-"):
            continue
        for descent in (DESCENT, 0):
            deepest = in_child(lambda: max_depth(str(f.path), f.password, descent))
            frames, first, digests = [], None, []
            # a frame r below the body runs at base["py"] + r + n + 2: n such
            # that the frame at `deepest` is the first past the limit (shift
            # 0), then one frame higher each time, down to page_layout's own
            # frame without descent
            for shift in range(-1, SWEEP if descent else deepest):
                n = LIMIT - 1 - base["py"] - deepest + shift
                if n < 0:
                    continue
                text = in_child(lambda: traced_from(str(f.path), f.password, n, descent))
                frames.append(n)
                if first is None:
                    first = text
                digests.append(hashlib.sha256(text.encode("ascii")).hexdigest())
            yield [f.path.name, f.path.read_bytes(), f.password, descent, base, frames], [first, digests]


#: Names given to ``get_cmap``: identity CMaps (made, not loaded), predefined
#: ones (loaded), unicode maps and missing names (``CMapNotFound``).
CMAP_NAMES = ("Identity-H", "Identity-V", "OneByteIdentityH", "OneByteIdentityV", "UniJIS-UCS2-H",
              "UniJIS-UCS2-V", "78-EUC-H", "Adobe-Japan1-UCS2", "NoSuchCMap", "", "../cmap/x", "Identity")
#: Names of the depth sweeps of ``get_cmap``.
CMAP_DEPTH_NAMES = ("Identity-H", "UniJIS-UCS2-H", "NoSuchCMap")
#: Codes decoded by the result.
CODES = b"\x00\x01\x30\x42\xff"


def get_cmap_case(name: str, strict: bool, room: int | None) -> tuple[dict[str, int], Any]:
    """``PDFResourceManager().get_cmap(name, strict)`` called from the
    depth ``LIMIT - room`` (``get_cmap``'s own frame is the next one; or
    from the stage thread's depth for ``None``): the depth of the calling
    frame, and the result (``[class, is_vertical, cids]``, built back at
    the stage's depth) or the exception."""
    rsrcmgr = PDFResourceManager()

    def call():
        try:
            return rsrcmgr.get_cmap(name, strict)
        except Exception as e:  # the exception is the expected result
            return e

    def down(n: int):
        return call() if n <= 0 else down(n - 1)

    out: dict[str, Any] = {}

    @terminated
    def body(path, password, max_pages):
        here = depth_here()
        # down(n) takes n + 1 frames, then call() one: its frame is the
        # (here + n + 2)th, and a Python call takes no C unit
        n = LIMIT - here["py"] - room - 2 if room is not None else -1
        out["depth"] = {"py": here["py"] + max(n, 0) + 2 if n >= 0 else here["py"] + 1, "c_left": here["c_left"]}
        out["cmap"] = down(n) if n >= 0 else call()
        yield ""

    _in_stage_thread(lambda: body("", None, 0))
    cmap = out["cmap"]
    if isinstance(cmap, Exception):
        return out["depth"], canon(cmap)
    return out["depth"], [type(cmap).__name__, cmap.is_vertical(), list(cmap.decode(CODES))]


def api_cases() -> Iterator[tuple[list, Any]]:
    for name in CMAP_NAMES:
        for strict in (False, True):
            depth, result = in_child(lambda: get_cmap_case(name, strict, None))
            yield ["get_cmap", name, strict, depth], result
    for name in CMAP_DEPTH_NAMES:
        for room in range(0, 31):
            depth, result = in_child(lambda: get_cmap_case(name, False, room))
            yield ["get_cmap", name, False, depth], result


def main() -> None:
    require_oracle_python()
    cmap_dir = codegen_cmaps.ensure()
    rel_cmap_dir = cmap_dir.relative_to(cache_dir()).as_posix()
    state = process_state()
    write_cases(MODULE, "py_process", [([], state)])
    write_cases(MODULE, "api", api_cases())
    n = write_cases(MODULE, "deep", deep_cases())
    if process_state() != state:
        raise RuntimeError("the process state changed while the traces ran in children")
    print(f"{MODULE}: {n} files swept", file=sys.stderr)
    write_meta(MODULE, cmap_dir=rel_cmap_dir, descent=DESCENT, sweep=SWEEP)


if __name__ == "__main__":
    main()
