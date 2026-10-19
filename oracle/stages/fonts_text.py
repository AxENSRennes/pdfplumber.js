"""Stage ``fonts_text``: the ``fonts`` stage on pdfminer's text device
(``interp_text.TextDevice``), so that the fonts the interpreter obtains are
checked without pdfplumber's device.

Exercises what the ``fonts`` stage does (the font cache of
pdfminer/pdfinterp.py:212 ``PDFResourceManager.get_font``, :397
``PDFPageInterpreter.init_resources``, :1142 ``do_Tf``, pdfminer/pdffont.py),
on pages that pdfplumber's device does not stop (an ``LTChar`` or an
``LTFigure`` it cannot build): see ``oracle/stages/interp_text.py``.

Traversal and lines: those of ``oracle/stages/fonts.py``, with ``FontsDevice``
built on ``interp_text.TextDevice``.
"""

from __future__ import annotations

from collections.abc import Iterator
from typing import Any

from pdfminer.pdfinterp import PDFResourceManager
from pdfminer.pdfpage import PDFPage

from ._common import line, open_document, terminated
from .fonts import _Doc
from .interp import page_layout
from .interp_text import TextDevice

PAGED = True
DEPENDS = ("fonts", "interp_text")


class FontsDevice(TextDevice):
    """``fonts.FontsDevice`` on ``TextDevice``."""

    calls: list[tuple[Any, Any]]

    def render_string(self, textstate, seq, ncs, graphicstate):
        self.calls.append((textstate.font, seq))
        super().render_string(textstate, seq, ncs, graphicstate)


@terminated
def run(path: str, password: str | None, max_pages: int) -> Iterator[str]:
    with open_document(path, password) as doc:
        rsrcmgr = PDFResourceManager()
        state = _Doc()
        for pageno, page in enumerate(PDFPage.create_pages(doc), start=1):
            yield line("page", pageno, page.pageid)
            ncached = len(rsrcmgr._cached_fonts)
            device, interpreter, error = page_layout(rsrcmgr, page, pageno, FontsDevice)
            for fontid, font in getattr(interpreter, "fontmap", {}).items():
                state.lines.append(line("fontmap", fontid, state.font(font)))
            for objid, font in list(rsrcmgr._cached_fonts.items())[ncached:]:
                state.lines.append(line("cached", objid, state.font(font)))
            for font, seq in device.calls:
                try:
                    items = list(seq)
                except Exception:  # render_string raised on it too
                    continue
                for s in items:
                    if isinstance(s, bytes):
                        state.shown(font, s)
            yield from state.lines
            state.lines.clear()
            if error is not None:
                yield line("error", error)
            if max_pages and pageno >= max_pages:
                break
