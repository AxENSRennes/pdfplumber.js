"""Stage ``fonts``: the fonts the interpreter obtains for the pages, and,
for every string it shows with them, ``decode``, ``to_unichr``,
``char_width`` and ``char_disp``.

Exercises pdfminer/pdfinterp.py:212 ``PDFResourceManager.get_font`` (the
font cache of the document, ``Type0`` fonts), :397
``PDFPageInterpreter.init_resources`` (the ``fontmap``) and :1142 ``do_Tf``
(the ``get_font(None, {})`` of an unknown font name), and
pdfminer/pdffont.py (every font class, with cmapdb and encodingdb under
it), in the documents of the corpus.

Traversal: the pages of the ``interp`` stage, driven as pdfplumber drives
them (``interp.page_layout``: one ``PDFResourceManager`` for the document, a
``PDFPageAggregatorWithMarkedContent`` and a ``PDFPageInterpreter`` for each
page, ``laparams=None``), with ``FontsDevice``, which records the font and
the ``seq`` of each ``render_string`` call on entry (one Python frame more
than pdfplumber at that call, as the Rust side). After ``process_page``
returns or raises, the stage writes, for the page:

1. the page interpreter's ``fontmap`` (``fontid -> font``, in dict order:
   the page's ``/Font`` resources, nothing if ``init_resources`` was not
   reached), from the stage's frame;
2. the entries the page added to the resource manager's cache
   (``rsrcmgr._cached_fonts``, in insertion order: ``objid -> font``, the
   fonts of the page and of its Form XObjects that have an object id);
3. each byte string of a ``seq`` (items that are ``bytes``) the page showed
   with a font for the first time in the document, in order of display, at
   most ``MAX_STRINGS`` per font: ``[cid for cid in font.decode(s)]`` (as
   the interpreter iterates it), then each code of that string not queried
   yet with that font: ``to_unichr``, ``char_width``, ``char_disp``, each
   one's result or exception.

Fonts are numbered by identity in order of first appearance in these lines
(``interp.Fonts``); a font's ``font`` line comes before the first line that
refers to it. Its attributes are those of ``oracle/unit/fonts.py``
``font_attrs`` (the same report as the unit cases, so that the Rust report
can be shared): a fixed order, absent ones omitted, a ``bytes``/``list``
``hscale``/``vscale`` of a Type 3 font as ``seq{"type": .., "len": ..}``.

Lines (per page)::

    page <pageno> <pageid>
    font <n> <attrs>                     <Class>{"basefont": .., ..., "get_height": ..}
    fontmap <fontid> <font>              font: font{"n": n}
    cached <objid> <font>
    string <font> <bytes> <cids>         cids: the decoded list, or the exception
    cid <font> <cid> <to_unichr> <char_width> <char_disp>
    error <exception>                    process_page raised it
    !end
"""

from __future__ import annotations

from collections.abc import Iterator
from typing import Any

from pdfminer.pdffont import PDFCIDFont, PDFFont
from pdfminer.pdfinterp import PDFResourceManager
from pdfminer.pdfpage import PDFPage
from pdfplumber.page import PDFPageAggregatorWithMarkedContent

from ..canon import Named
from ._common import line, open_document, terminated
from .interp import Fonts, page_layout

PAGED = True
DEPENDS = ("interp",)

#: Strings queried per font (the first ones shown in the document).
MAX_STRINGS = 2000


class FontsDevice(PDFPageAggregatorWithMarkedContent):
    """pdfplumber's device, recording the font and ``seq`` of each
    ``render_string`` call."""

    calls: list[tuple[Any, Any]]

    def render_string(self, textstate, seq, ncs, graphicstate):
        self.calls.append((textstate.font, seq))
        super().render_string(textstate, seq, ncs, graphicstate)


def seqval(x: Any) -> Any:
    """A value that may be a repeated ``bytes``/``list`` (Type 3 scales):
    its type and length (oracle/unit/fonts.py ``seqval``)."""
    if isinstance(x, (bytes, list)):
        return Named("seq", {"type": type(x).__name__, "len": len(x)})
    return x


def _call(f: Any, *args: Any) -> Any:
    try:
        return f(*args)
    except Exception as e:  # the exception is the result
        return e


def _call_seq(f: Any, *args: Any) -> Any:
    r = _call(f, *args)
    return r if isinstance(r, Exception) else seqval(r)


def _umap(m: Any) -> Any:
    if m is None:
        return None
    return Named(type(m).__name__, {"attrs": dict(m.attrs), "items": [(int(k), v) for k, v in m.cid2unichr.items()]})


def _cmap(m: Any) -> Named:
    fields: dict[str, Any] = {"attrs": dict(m.attrs)}
    if hasattr(m, "code2cid"):
        fields["code2cid"] = len(m.code2cid)
    return Named(type(m).__name__, fields)


def font_attrs(font: PDFFont) -> Named:
    """oracle/unit/fonts.py ``font_attrs``: the attributes of a font, in a
    fixed order (absent ones omitted)."""
    v = vars(font)
    fields: dict[str, Any] = {}
    for name in ("basefont", "fontname", "flags", "ascent", "descent", "italic_angle", "default_width", "leading", "bbox"):
        if name in v:
            fields[name] = v[name]
    fields["hscale"] = seqval(font.hscale)
    fields["vscale"] = seqval(font.vscale)
    fields["widths"] = list(font.widths.items())
    if "cid2unicode" in v:
        fields["cid2unicode"] = [(int(k), x) for k, x in font.cid2unicode.items()]
    if "unicode_map" in v:
        fields["unicode_map"] = _umap(font.unicode_map)
    if "fontfile" in v:
        fields["fontfile"] = True
    if "matrix" in v:
        fields["matrix"] = font.matrix
    if isinstance(font, PDFCIDFont):
        fields["cidcoding"] = font.cidcoding
        fields["cmap"] = _cmap(font.cmap)
        fields["vertical"] = font.vertical
        fields["disps"] = list(font.disps.items())
        fields["default_disp"] = font.default_disp
    fields["is_vertical"] = font.is_vertical()
    fields["is_multibyte"] = font.is_multibyte()
    fields["repr"] = repr(font)
    for method in ("get_ascent", "get_descent", "get_width", "get_height"):
        fields[method] = _call_seq(getattr(font, method))
    return Named(type(font).__name__, fields)


def _decode(font: PDFFont, s: bytes) -> list[int]:
    """``for cid in font.decode(s)``, as the interpreter iterates it."""
    out = []
    for cid in font.decode(s):
        out.append(cid)
    return out


class _Doc:
    """What the document's lines have shown so far."""

    def __init__(self) -> None:
        self.fonts = Fonts()
        self.strings: dict[int, dict[bytes, None]] = {}
        self.cids: dict[int, set[int]] = {}
        self.lines: list[str] = []

    def font(self, font: PDFFont) -> Named:
        n, new = self.fonts.number(font)
        if new:
            self.lines.append(line("font", n, font_attrs(font)))
            self.strings[n] = {}
            self.cids[n] = set()
        return Named("font", {"n": n})

    def shown(self, font: PDFFont, s: bytes) -> None:
        ref = self.font(font)
        n = ref.fields["n"]
        strings = self.strings[n]
        if s in strings or len(strings) >= MAX_STRINGS:
            return
        strings[s] = None
        cids = _call(_decode, font, s)
        self.lines.append(line("string", ref, s, cids))
        if isinstance(cids, Exception):
            return
        for cid in cids:
            if cid in self.cids[n]:
                continue
            self.cids[n].add(cid)
            self.lines.append(
                line("cid", ref, cid, _call(font.to_unichr, cid), _call_seq(font.char_width, cid), _call(font.char_disp, cid))
            )


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
