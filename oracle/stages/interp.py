"""Stage ``interp``: every call pdfminer's interpreter makes to the device,
with the graphic and text states, as pdfplumber drives it.

Exercises pdfminer/pdfinterp.py (``PDFResourceManager`` and its font cache,
``PDFPageInterpreter``: ``process_page``, ``render_contents``,
``init_resources``, ``init_state``, ``execute`` and every ``do_*`` operator,
``subinterp`` and the circular-reference guard of Form XObjects, inline
images), pdfminer/pdfdevice.py (``PDFTextDevice.render_string``,
``render_string_horizontal``, ``render_string_vertical``),
pdfminer/converter.py (``PDFLayoutAnalyzer``, whose ``render_char`` returns
the advance the interpreter adds) and
pdfplumber/page.py:110 ``PDFPageAggregatorWithMarkedContent``, plus everything they call (fonts,
CMaps, ``PDFContentParser``).

Traversal (``max_pages`` pages, 0 = all; pages are created lazily), the one
of pdfplumber: pdfplumber/pdf.py:28 ``PDF.__init__`` creates (at its line 53) one
``PDFResourceManager()`` for the document (``caching=True``: fonts are
cached by object id across pages), and pdfplumber/page.py:256 ``Page.layout``
creates a device and an interpreter for each page::

    doc = PDFDocument(PDFParser(open(path, "rb")), password=password or "")
    rsrcmgr = PDFResourceManager()
    for pageno, page in enumerate(PDFPage.create_pages(doc), start=1):
        page_layout(rsrcmgr, page, pageno, TracingDevice)
        the lines of the recorded calls

``page_layout`` is the body of ``Page.layout`` (one frame, like the
property's)::

    device = TracingDevice(rsrcmgr, pageno=pageno, laparams=None)
    interpreter = PDFPageInterpreter(rsrcmgr, device)
    interpreter.process_page(page)

with ``laparams=None``, pdfplumber's default (the analysis is the ``layout``
stage's). ``TracingDevice`` subclasses ``PDFPageAggregatorWithMarkedContent``
(an extension point, not a patch): each device method appends a snapshot of
its arguments to ``device.calls`` and calls ``super()``. Nothing is formatted
while the interpreter runs; the lines are written after ``process_page``
returns or raises. Each override is one Python frame more than pdfplumber
runs at that call (the Rust tracing device takes it too), and the snapshots
are native calls (``vars``, ``dict``, ``list``, ``list.append``), which
never reach their C limit there. The analyzer's own calls of device
methods go through the overrides too: ``PDFLayoutAnalyzer.paint_path``
calls ``self.paint_path`` for each subpath of a path with several ``m``,
``PDFTextDevice.render_string_horizontal`` calls ``self.render_char``.

The fonts, by identity, are numbered in order of first reference in the
document's trace. Inline images are named ``str(id(obj))`` by
pdfminer/pdfinterp.py:1309 ``PDFPageInterpreter.do_EI``, a memory address: a name given by
``do_EI`` (``begin_figure``, ``render_image``, ``end_figure`` called from
``do_EI``'s frame) is written ``inline{"n": k}``, ``k`` counting the inline
images of the page from 0 in order of appearance (the port names them
deterministically, see ``docs/architecture.md``, "Inline Image Names").

The module imports the UTF-16BE codec first: the first
``bytes.decode("UTF-16BE")`` of a process imports it, deep below the caller,
which the port does not model (``crates/pdfminer/src/cmapdb.rs``); the
``fonts`` and ``layout`` stages import this module.

Lines (per page)::

    page <pageno> <pageid>
    font <n> <class> <repr>              before the first line that refers to font n
                                         (a call's font, gstate and char_args lines
                                         come in this order, before the call's line)
    gstate <PDFGraphicState{...}>        before a call that carries a graphic state
                                         (paint_path, render_string, render_char) whose
                                         canonical text differs from the last one written
                                         on the page; its __dict__, color spaces as
                                         PDFColorSpace{"name": .., "ncomponents": ..}
    char_args <font> <fontsize> <scaling> <rise> <ncs>
                                         before a render_char whose other arguments
                                         differ from the last ones written on the page
    set_ctm <ctm>
    begin_page <pageid> <ctm>
    end_page <pageid>
    begin_figure <name> <bbox> <matrix>  bbox, matrix as given: a list (the PDF's) or a tuple
    end_figure <name>
    render_image <name> <stream>
    paint_path <stroke> <fill> <evenodd> <path>
    render_string <textstate> <seq> <ncs>
                                         textstate: PDFTextState{...} (its __dict__ on
                                         entry, the font as font{"n": n})
    render_char <matrix> <cid> <result>  result: the returned advance, or the exception
    end_string <linematrix>              textstate.linematrix when render_string returned
    begin_tag <tag> <props>              props None when the interpreter passed none
    end_tag
    do_tag <tag> <props>
    raised <exception>                   the call whose lines precede (and not a
                                         render_char) raised it, after the lines of the
                                         calls it made
    error <exception>                    process_page raised it (pdfplumber raises
                                         PdfminerException(e) from page.layout)
    !end

A call's line is written at its entry, so the calls it makes follow it; a
``render_char`` is one line, written with its result. A font is
``font{"n": n}`` and a color space ``PDFColorSpace{...}`` wherever they
appear.
"""

from __future__ import annotations

import codecs
import sys
from collections.abc import Iterator
from typing import Any

from pdfminer.pdfcolor import PDFColorSpace
from pdfminer.pdffont import PDFFont
from pdfminer.pdfinterp import PDFPageInterpreter, PDFResourceManager
from pdfminer.pdfpage import PDFPage
from pdfplumber.page import PDFPageAggregatorWithMarkedContent

from ..canon import Named, canon
from ._common import line, open_document, terminated

PAGED = True
DEPENDS = ()

codecs.lookup("UTF-16BE")

#: The code of the method that names inline images ``str(id(obj))``.
_DO_EI = PDFPageInterpreter.do_EI.__code__


class TracingDevice(PDFPageAggregatorWithMarkedContent):
    """pdfplumber's device, recording every call (see the module docstring)."""

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


def page_layout(rsrcmgr: PDFResourceManager, page: PDFPage, pageno: int, device_class: type) -> tuple[Any, Any, Exception | None]:
    """The body of pdfplumber/page.py:256 ``Page.layout`` with
    ``laparams=None``: ``(device, interpreter, e)``, ``e`` being what
    ``process_page`` raised (or ``None``). ``device.calls`` is set before
    the interpreter runs (no ``__init__`` override)."""
    device = device_class(rsrcmgr, pageno=pageno, laparams=None)
    device.calls = []
    interpreter = PDFPageInterpreter(rsrcmgr, device)
    try:
        interpreter.process_page(page)
    except Exception as e:
        return device, interpreter, e
    return device, interpreter, None


def colorspace(cs: PDFColorSpace) -> Named:
    return Named("PDFColorSpace", {"name": cs.name, "ncomponents": cs.ncomponents})


def state_fields(fields: dict[str, Any], font_ref: Any = None) -> dict[str, Any]:
    """A graphic or text state ``__dict__`` with its color spaces (and font)
    written as values."""
    out = {}
    for k, v in fields.items():
        if isinstance(v, PDFColorSpace):
            v = colorspace(v)
        elif isinstance(v, PDFFont):
            v = font_ref(v)
        out[k] = v
    return out


class Fonts:
    """Fonts numbered by identity in order of first reference (kept alive so
    that an ``id()`` is never reused)."""

    def __init__(self) -> None:
        self.index: dict[int, int] = {}
        self.fonts: list[PDFFont] = []

    def number(self, font: PDFFont) -> tuple[int, bool]:
        """The font's number, and whether it is new."""
        n = self.index.get(id(font))
        if n is not None:
            return n, False
        n = self.index[id(font)] = len(self.fonts)
        self.fonts.append(font)
        return n, True


class _Page:
    """The formatter of one page's calls."""

    def __init__(self, fonts: Fonts) -> None:
        self.fonts = fonts
        self.lines: list[str] = []
        self.inline: dict[str, int] = {}
        self.gstate: str | None = None
        self.char_args: str | None = None

    def font(self, font: PDFFont) -> Named:
        n, new = self.fonts.number(font)
        if new:
            self.lines.append(line("font", n, type(font).__name__, repr(font)))
        return Named("font", {"n": n})

    def name(self, name: str, inline: bool) -> Any:
        if not inline:
            return name
        if name not in self.inline:
            self.inline[name] = len(self.inline)
        return Named("inline", {"n": self.inline[name]})

    def graphic_state(self, fields: dict[str, Any]) -> None:
        text = canon(Named("PDFGraphicState", state_fields(fields)))
        if text != self.gstate:
            self.gstate = text
            self.lines.append("gstate " + text)

    def call(self, c: tuple[Any, ...]) -> None:
        kind = c[0]
        out = self.lines
        if kind in ("set_ctm", "begin_page", "end_page", "end_string", "begin_tag", "do_tag"):
            out.append(line(kind, *c[1:]))
        elif kind == "end_tag":
            out.append(kind)
        elif kind == "begin_figure":
            _, name, inline, bbox, matrix = c
            out.append(line(kind, self.name(name, inline), bbox, matrix))
        elif kind == "end_figure":
            _, name, inline = c
            out.append(line(kind, self.name(name, inline)))
        elif kind == "render_image":
            _, name, inline, stream = c
            out.append(line(kind, self.name(name, inline), stream))
        elif kind == "paint_path":
            _, gs, stroke, fill, evenodd, path = c
            self.graphic_state(gs)
            out.append(line(kind, stroke, fill, evenodd, path))
        elif kind == "render_string":
            _, ts, seq, ncs, gs = c
            textstate = Named("PDFTextState", state_fields(ts, self.font))
            self.graphic_state(gs)
            out.append(line(kind, textstate, seq, colorspace(ncs)))
        elif kind == "render_char":
            _, matrix, font, fontsize, scaling, rise, cid, ncs, gs, result = c
            args = " ".join(canon(v) for v in (self.font(font), fontsize, scaling, rise, colorspace(ncs)))
            self.graphic_state(vars(gs))
            if args != self.char_args:
                self.char_args = args
                out.append("char_args " + args)
            out.append(line(kind, matrix, cid, result))
        elif kind == "raised":
            out.append(line(kind, c[1]))
        else:  # pragma: no cover
            raise AssertionError(kind)


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


__all__ = ["Fonts", "TracingDevice", "colorspace", "page_layout", "state_fields"]
