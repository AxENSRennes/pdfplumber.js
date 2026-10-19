"""Stage ``interp_text``: the ``interp`` stage on pdfminer's text device, so
that the interpreter is checked without pdfplumber's device.

Exercises pdfminer/pdfinterp.py (everything the ``interp`` stage does) and
pdfminer/pdfdevice.py (``PDFDevice``, ``PDFTextDevice``: ``render_string``,
``render_string_horizontal``, ``render_string_vertical``, ``render_char``
returning ``0``), with no converter: the calls and states the interpreter
produces do not depend on what pdfplumber's device builds, except for the
advance ``render_char`` returns (here ``0``, so a string's chars all start
at its origin, moved by the char spacing, the word spacing and the numbers
of ``TJ``) and for what that device raises (``LTFigure`` of a figure whose
``/BBox`` or ``/Matrix`` it cannot use, an ``LTChar`` whose font fails):
here the interpretation goes on.

Traversal and lines: those of the ``interp`` stage (``oracle/stages/interp.py``),
with ``TracingDevice`` built on ``TextDevice`` instead of
``PDFPageAggregatorWithMarkedContent``; the same overrides, the same
snapshots, the same formatter.

``TextDevice`` is ``PDFTextDevice`` with the constructor pdfplumber's
device class has (``TextDevice(rsrcmgr, pageno=..., laparams=None)``,
which calls ``PDFDevice.__init__``) and one check where pdfplumber's
device would raise and the interpreter's own arithmetic depends on it: its
``begin_figure`` computes ``mult_matrix(matrix, self.ctm)``, as
``PDFLayoutAnalyzer.begin_figure`` does for its ``LTFigure``, and raises
``TypeError("figure matrix is not numbers")`` when an item of the result is
not a number (a ``bytes`` or a ``list`` in the form's ``/Matrix`` repeated
by the CTM's ints), where ``LTFigure`` raises ``TypeError`` too. Otherwise
the interpreter would go on with a CTM made of sequences, which the port does
not represent (``crates/pdfminer/src/pdfinterp.rs``, ``PDFPageInterpreter``).
"""

from __future__ import annotations

import sys
from collections.abc import Iterator
from typing import Any

from pdfminer.pdfdevice import PDFTextDevice
from pdfminer.pdfinterp import PDFResourceManager
from pdfminer.pdfpage import PDFPage
from pdfminer.utils import mult_matrix

from ._common import line, open_document, terminated
from .interp import _DO_EI, Fonts, _Page, page_layout

PAGED = True
DEPENDS = ("interp",)


class TextDevice(PDFTextDevice):
    """``PDFTextDevice`` with the constructor of pdfplumber's device class
    and the figure check of the module docstring."""

    def __init__(self, rsrcmgr: PDFResourceManager, pageno: int = 1, laparams: Any = None) -> None:
        PDFTextDevice.__init__(self, rsrcmgr)

    def begin_figure(self, name, bbox, matrix):
        m = mult_matrix(matrix, self.ctm)
        if [v for v in m if not isinstance(v, (int, float))]:
            raise TypeError("figure matrix is not numbers")


class TracingDevice(TextDevice):
    """``interp.TracingDevice`` on ``TextDevice`` (the same overrides)."""

    calls: list[tuple[Any, ...]]

    def set_ctm(self, ctm):
        self.calls.append(("set_ctm", ctm))
        try:
            super().set_ctm(ctm)
        except Exception as e:
            self.calls.append(("raised", e))
            raise

    def begin_tag(self, tag, props=None):
        self.calls.append(("begin_tag", tag, props))
        try:
            super().begin_tag(tag, props)
        except Exception as e:
            self.calls.append(("raised", e))
            raise

    def end_tag(self):
        self.calls.append(("end_tag",))
        try:
            super().end_tag()
        except Exception as e:
            self.calls.append(("raised", e))
            raise

    def do_tag(self, tag, props=None):
        self.calls.append(("do_tag", tag, props))
        try:
            super().do_tag(tag, props)
        except Exception as e:
            self.calls.append(("raised", e))
            raise

    def begin_page(self, page, ctm):
        self.calls.append(("begin_page", page.pageid, ctm))
        try:
            super().begin_page(page, ctm)
        except Exception as e:
            self.calls.append(("raised", e))
            raise

    def end_page(self, page):
        self.calls.append(("end_page", page.pageid))
        try:
            super().end_page(page)
        except Exception as e:
            self.calls.append(("raised", e))
            raise

    def begin_figure(self, name, bbox, matrix):
        self.calls.append(("begin_figure", name, sys._getframe(1).f_code is _DO_EI, bbox, matrix))
        try:
            super().begin_figure(name, bbox, matrix)
        except Exception as e:
            self.calls.append(("raised", e))
            raise

    def end_figure(self, name):
        self.calls.append(("end_figure", name, sys._getframe(1).f_code is _DO_EI))
        try:
            super().end_figure(name)
        except Exception as e:
            self.calls.append(("raised", e))
            raise

    def render_image(self, name, stream):
        self.calls.append(("render_image", name, sys._getframe(1).f_code is _DO_EI, stream))
        try:
            super().render_image(name, stream)
        except Exception as e:
            self.calls.append(("raised", e))
            raise

    def paint_path(self, graphicstate, stroke, fill, evenodd, path):
        self.calls.append(("paint_path", dict(vars(graphicstate)), stroke, fill, evenodd, list(path)))
        try:
            super().paint_path(graphicstate, stroke, fill, evenodd, path)
        except Exception as e:
            self.calls.append(("raised", e))
            raise

    def render_string(self, textstate, seq, ncs, graphicstate):
        self.calls.append(("render_string", dict(vars(textstate)), seq, ncs, dict(vars(graphicstate))))
        try:
            super().render_string(textstate, seq, ncs, graphicstate)
        except Exception as e:
            self.calls.append(("raised", e))
            raise
        self.calls.append(("end_string", textstate.linematrix))

    def render_char(self, matrix, font, fontsize, scaling, rise, cid, ncs, graphicstate):
        try:
            adv = super().render_char(matrix, font, fontsize, scaling, rise, cid, ncs, graphicstate)
        except Exception as e:
            self.calls.append(("render_char", matrix, font, fontsize, scaling, rise, cid, ncs, graphicstate, e))
            raise
        self.calls.append(("render_char", matrix, font, fontsize, scaling, rise, cid, ncs, graphicstate, adv))
        return adv


@terminated
def run(path: str, password: str | None, max_pages: int) -> Iterator[str]:
    with open_document(path, password) as doc:
        rsrcmgr = PDFResourceManager()
        fonts = Fonts()
        for pageno, page in enumerate(PDFPage.create_pages(doc), start=1):
            yield line("page", pageno, page.pageid)
            device, _, error = page_layout(rsrcmgr, page, pageno, TracingDevice)
            fmt = _Page(fonts)
            for c in device.calls:
                fmt.call(c)
            yield from fmt.lines
            if error is not None:
                yield line("error", error)
            if max_pages and pageno >= max_pages:
                break


__all__ = ["TextDevice", "TracingDevice"]
