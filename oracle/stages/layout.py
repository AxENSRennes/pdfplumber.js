"""Stage ``layout``: the ``LTPage`` of each page, raw (pdfplumber's default,
``laparams=None``) and analyzed with ``LAParams()``, every object with all
its attributes, ``mcid`` and ``tag`` included.

Exercises pdfminer/converter.py (``PDFLayoutAnalyzer``: ``begin_page``,
``end_page``, ``begin_figure``, ``end_figure``, ``render_image``,
``paint_path`` and its shapes, ``render_char``, ``handle_undefined_char``;
``PDFPageAggregator``),
pdfplumber/page.py:110 ``PDFPageAggregatorWithMarkedContent`` (``mcid``, ``tag``), pdfminer/layout.py
(the objects, and ``LTLayoutContainer.analyze`` under ``LAParams()``), and
the interpreter under them.

Traversal (pages as in the ``interp`` stage: one ``PDFResourceManager`` for
the document)::

    for pageno, page in enumerate(PDFPage.create_pages(doc), start=1):
        _page(rsrcmgr, page, pageno)

``_page`` is the body of pdfplumber/page.py:256 ``Page.layout`` (one frame)
with ``laparams=None``: ``PDFPageAggregatorWithMarkedContent(rsrcmgr,
pageno=pageno, laparams=None)``, ``PDFPageInterpreter(rsrcmgr,
device).process_page(page)``, ``device.get_result()``, exactly pdfplumber's
calls, no override. It writes the raw page, then analyzes that same
``LTPage`` with ``LAParams()`` from two frames below (``_process_page`` and
``_end_page``, which stand for ``PDFPageInterpreter.process_page`` and
``PDFLayoutAnalyzer.end_page``): this is the call
pdfminer/converter.py:81 ``PDFLayoutAnalyzer.end_page`` makes when the
device has ``laparams``, ``self.cur_item.analyze(self.laparams)``, on the
same objects (the analysis reads nothing else: the rest of ``end_page``
and of ``process_page`` touches no object), at the same depth, so the
result is the one ``pdfplumber.open(f, laparams={})`` computes, without a
second interpretation. A ``RecursionError`` in the analysis happens where
it happens in pdfplumber (pdfplumber would raise it, wrapped, from
``page.layout``).

The analysis orders pairs of text boxes at equal distance by Python's
``id()`` (``crates/pdfminer/src/layout.rs``, "Analysis and object
identity"). For the run it traces, the stage writes the ``id()`` ranking of
the boxes and groups of the result (``ids``, the keying of
``oracle/unit/layout.py`` ``id_ranks``: ``[min_char, n_boxes, rank]``, the
smallest raw index of its chars and its number of boxes, ``rank`` the order
of the ids). ``oracle.verify`` hands the oracle's trace to ``pdftrace``
(``ORACLE_INPUT``), which analyzes with these ids (an injected
``IdSource``) and writes the ``ids`` of its own result. The ids are memory
addresses: the hash seed and every allocation of the process move them, so
on pages with such ties the analysis is a draw that differs from one run of
the stage to the next (and from ``pdfplumber.open(f, laparams={})``, which
allocates differently: no raw dump before the analysis); parity holds for
whichever draw the cache holds, and the trace is not reproducible across
cache builds (``docs/architecture.md``, "Statistique ``CreationOrder``").

Raw objects are numbered depth-first in ``_objs`` order (figures before
their contents), from 0: the raw index. A raw object is written as its
class name and the tuple of its ``__dict__`` values, in order; a ``class``
line gives the attribute names, before the first object of the class on the
page and again whenever they change (``mcid``/``tag`` are set on some
objects only). Everywhere, ``bbox`` is left out: upstream it is always the
tuple ``(x0, y0, x1, y1)`` of the same values (``LTComponent.set_bbox``),
and ``layout.rs`` does not store it. In the values, ``_objs`` is the number
of children (they follow on their own lines); a graphic state is
``gs{"n": k}`` and a color space ``cs{"n": k}``, ``k`` numbering the
distinct values (canonical texts) of the page, which a ``gstate`` or
``colorspace`` line gives before their first use (by value: the identity
of these objects is not observable through pdfplumber); the name of an
inline image (``str(id(stream))``, pdfminer/pdfinterp.py:1309 ``PDFPageInterpreter.do_EI``, a
memory address) is ``inline{"n": k}``, ``k`` counting the page's inline
images in raw order (an ``LTImage`` whose name is ``str(id(stream))``, and
its ``LTFigure``, which has its name).

In the analyzed page, a raw object (``LTChar``, ``LTCurve``, ``LTImage``,
``LTFigure``: ``LAParams()`` has ``all_texts=False``, figures are not
analyzed) is ``#{"i": n}``, its raw index; a text line is written as
``Named(class, __dict__)`` on one line, its ``_objs`` given in full (raw
chars and ``LTAnno{"_text": ..}``), then ``get_text()``; a text box with
its ``_objs`` count and ``get_text()``, its lines on the following lines;
``groups`` is the number of top-level groups (or ``None``), written after
the objects, depth-first, a box as ``box#{"i": n}`` (its position in the
page's ``_objs``), a group with ``get_text()`` when its tree is at most
``GROUP_TEXT_HEIGHT`` high (a deeper one would recurse near the limit).

Lines (per page)::

    page <pageno> <pageid>
    error <exception>                    process_page raised it (no layout lines follow)
    raw LTPage{...}                      the raw page (``_objs``: count, ``groups``: None)
    gstate <k> <PDFGraphicState{...}>    its __dict__, color spaces as PDFColorSpace{...}
    colorspace <k> <PDFColorSpace{"name": .., "ncomponents": ..}>
    class <name> <[attribute, ...]>
    raw <i> <depth> <name> <(value, ...)>
                                         depth: 0 for an object of the page, 1 inside a
                                         figure of the page...
    analyze <exception>                  the analysis raised (no analyzed lines follow)
    analyzed LTPage{...}                 the analyzed page (``_objs``: count, ``groups``:
                                         count or None)
    obj <path> <object>                  path: the positions from the page, as a tuple
    group <path> <object>
    ids [[min_char, n_boxes, rank], ...] after an analysis, even one that raised
    !end

The ``gstate``, ``colorspace`` and ``class`` lines a ``raw`` line needs come
right before it, in the order its values need them (``class`` last).
"""

from __future__ import annotations

from collections.abc import Iterator
from typing import Any

from pdfminer.layout import LAParams, LTChar, LTFigure, LTImage, LTPage, LTTextBox, LTTextLine
from pdfminer.pdfcolor import PDFColorSpace
from pdfminer.pdfinterp import PDFPageInterpreter, PDFResourceManager
from pdfminer.pdfpage import PDFPage
from pdfplumber.page import PDFPageAggregatorWithMarkedContent

from ..canon import Named, canon
from ._common import line, open_document, terminated
from .interp import colorspace, state_fields

PAGED = True
DEPENDS = ("interp",)
#: ``oracle.verify`` gives ``pdftrace`` the oracle's trace (the ``ids`` lines).
ORACLE_INPUT = True

#: The highest group whose get_text() is written (oracle/unit/layout.py).
GROUP_TEXT_HEIGHT = 100


def _raw_objects(page: LTPage) -> list[tuple[Any, int]]:
    """``(obj, depth)`` of the raw objects, depth-first (no recursion)."""
    out: list[tuple[Any, int]] = []
    stack: list[tuple[Any, int]] = [(o, 0) for o in reversed(page._objs)]
    while stack:
        obj, depth = stack.pop()
        out.append((obj, depth))
        if isinstance(obj, LTFigure):
            stack.extend((o, depth + 1) for o in reversed(obj._objs))
    return out


class _RawDump:
    """The ``raw`` lines of a page and the raw index of its objects."""

    def __init__(self, page: LTPage) -> None:
        self.objects = _raw_objects(page)
        self.index = {id(o): i for i, (o, _) in enumerate(self.objects)}
        self.inline: dict[str, int] = {}
        for obj, _ in self.objects:
            if isinstance(obj, LTImage) and obj.name == str(id(obj.stream)) and obj.name not in self.inline:
                self.inline[obj.name] = len(self.inline)
        #: canonical text -> number, for each kind of shared value
        self.shared: dict[str, dict[str, int]] = {"gstate": {}, "colorspace": {}}
        self.keys: dict[str, list[str]] = {}
        self.lines: list[str] = []

    def name(self, obj: Any, name: Any) -> Any:
        if isinstance(obj, LTImage) and name == str(id(obj.stream)):
            return Named("inline", {"n": self.inline[name]})
        if isinstance(obj, LTFigure) and name in self.inline:
            for child in obj._objs:
                if isinstance(child, LTImage) and child.name == name and name == str(id(child.stream)):
                    return Named("inline", {"n": self.inline[name]})
        return name

    def ref(self, kind: str, value: Named, short: str) -> Named:
        """``short{"n": k}``, ``k`` numbering the distinct values of ``kind``
        on the page; a ``kind`` line defines it at its first use."""
        text = canon(value)
        numbers = self.shared[kind]
        k = numbers.get(text)
        if k is None:
            k = numbers[text] = len(numbers)
            self.lines.append(f"{kind} {k} {text}")
        return Named(short, {"n": k})

    def values(self, obj: Any) -> tuple[list[str], list[Any]]:
        """The attribute names (``bbox`` left out) and values of ``obj``."""
        keys: list[str] = []
        values: list[Any] = []
        for k, v in vars(obj).items():
            if k == "bbox":
                continue
            if k == "_objs":
                v = len(v)
            elif k == "graphicstate":
                v = self.ref("gstate", Named("PDFGraphicState", state_fields(vars(v))), "gs")
            elif k == "name":
                v = self.name(obj, v)
            elif isinstance(v, PDFColorSpace):
                v = self.ref("colorspace", colorspace(v), "cs")
            keys.append(k)
            values.append(v)
        return keys, values

    def write(self, page: LTPage) -> list[str]:
        keys, values = self.values(page)
        self.lines.append(line("raw", Named("LTPage", dict(zip(keys, values)))))
        for i, (obj, depth) in enumerate(self.objects):
            cls = type(obj).__name__
            keys, values = self.values(obj)
            if self.keys.get(cls) != keys:
                self.keys[cls] = keys
                self.lines.append(line("class", cls, keys))
            self.lines.append(line("raw", i, depth, cls, tuple(values)))
        return self.lines


def _heights(groups: list[Any]) -> dict[int, int]:
    """The height of the tree of each group (a box is 0), without recursion."""
    heights: dict[int, int] = {}
    stack: list[tuple[Any, bool]] = [(g, False) for g in groups]
    while stack:
        g, done = stack.pop()
        if isinstance(g, LTTextBox):
            heights[id(g)] = 0
        elif done:
            heights[id(g)] = 1 + max((heights[id(c)] for c in g._objs), default=0)
        else:
            stack.append((g, True))
            stack.extend((c, False) for c in g._objs)
    return heights


def _analyzed(page: LTPage, raw: _RawDump) -> list[str]:
    out: list[str] = []
    positions = {id(o): i for i, o in enumerate(page._objs)}

    def ref(obj: Any) -> Named:
        return Named("#", {"i": raw.index[id(obj)]})

    def container(obj: Any, extra: dict[str, Any]) -> Named:
        fields: dict[str, Any] = {}
        for k, v in vars(obj).items():
            if k != "bbox":
                fields[k] = len(v) if k == "_objs" and not isinstance(obj, LTTextLine) else v
        if isinstance(obj, LTTextLine):
            fields["_objs"] = [ref(c) if isinstance(c, LTChar) else Named("LTAnno", {"_text": c._text}) for c in obj._objs]
        fields.update(extra)
        return Named(type(obj).__name__, fields)

    fields: dict[str, Any] = {}
    for k, v in vars(page).items():
        if k == "bbox":
            continue
        if k == "_objs" or (k == "groups" and v is not None):
            v = len(v)
        fields[k] = v
    out.append(line("analyzed", Named("LTPage", fields)))
    for i, obj in enumerate(page._objs):
        if id(obj) in raw.index:
            out.append(line("obj", (i,), ref(obj)))
        elif isinstance(obj, LTTextLine):
            out.append(line("obj", (i,), container(obj, {"get_text()": obj.get_text()})))
        elif isinstance(obj, LTTextBox):
            out.append(line("obj", (i,), container(obj, {"get_text()": obj.get_text()})))
            for j, tl in enumerate(obj._objs):
                out.append(line("obj", (i, j), container(tl, {"get_text()": tl.get_text()})))
        else:  # pragma: no cover
            raise AssertionError(type(obj))
    if page.groups is not None:
        heights = _heights(page.groups)
        stack: list[tuple[Any, tuple[int, ...]]] = [(g, (i,)) for i, g in reversed(list(enumerate(page.groups)))]
        while stack:
            g, path = stack.pop()
            if isinstance(g, LTTextBox):
                out.append(line("group", path, Named("box#", {"i": positions[id(g)]})))
                continue
            extra = {"get_text()": g.get_text()} if heights[id(g)] <= GROUP_TEXT_HEIGHT else {}
            out.append(line("group", path, container(g, extra)))
            stack.extend((c, (*path, j)) for j, c in reversed(list(enumerate(g._objs))))
    return out


def id_ranks(page: LTPage, index: dict[int, int]) -> list[list[int]]:
    """oracle/unit/layout.py ``id_ranks``: ``[min_char, n_boxes, rank]`` of
    every box and group in the groups of the page and of its figures (no
    recursion)."""
    found: list[tuple[int, int, int]] = []
    keys: dict[int, tuple[int, int]] = {}

    def box_key(box: Any) -> tuple[int, int]:
        return min(index[id(c)] for tl in box for c in tl if isinstance(c, LTChar)), 1

    containers: list[Any] = [page]
    roots: list[Any] = []
    while containers:
        c = containers.pop(0)
        containers[:0] = [o for o in c._objs if isinstance(o, LTFigure)]
        roots.extend(c.groups or [])
    for root in roots:
        stack: list[tuple[Any, bool]] = [(root, False)]
        while stack:
            g, done = stack.pop()
            if isinstance(g, LTTextBox):
                keys[id(g)] = box_key(g)
                found.append((id(g), *keys[id(g)]))
            elif done:
                ks = [keys[id(c)] for c in g._objs]
                keys[id(g)] = (min(k[0] for k in ks), sum(k[1] for k in ks))
                found.append((id(g), *keys[id(g)]))
            else:
                stack.append((g, True))
                stack.extend((c, False) for c in reversed(list(g._objs)))
    found.sort()
    return [[m, n, rank] for rank, (_, m, n) in enumerate(found)]


def _end_page(ltpage: LTPage, laparams: LAParams) -> Exception | None:
    """Stands for pdfminer/converter.py:81 ``PDFLayoutAnalyzer.end_page``'s
    frame: ``self.cur_item.analyze(self.laparams)``."""
    try:
        ltpage.analyze(laparams)
    except Exception as e:
        return e
    return None


def _process_page(ltpage: LTPage, laparams: LAParams) -> Exception | None:
    """Stands for the frame of
    pdfminer/pdfinterp.py:1352 ``PDFPageInterpreter.process_page``, which calls ``end_page``."""
    return _end_page(ltpage, laparams)


def _page(rsrcmgr: PDFResourceManager, page: PDFPage, pageno: int) -> list[str]:
    """pdfplumber/page.py:256 ``Page.layout`` (see the module docstring)."""
    device = PDFPageAggregatorWithMarkedContent(rsrcmgr, pageno=pageno, laparams=None)
    interpreter = PDFPageInterpreter(rsrcmgr, device)
    try:
        interpreter.process_page(page)
    except Exception as e:
        return [line("error", e)]
    ltpage = device.get_result()
    raw = _RawDump(ltpage)
    out = raw.write(ltpage)
    error = _process_page(ltpage, LAParams())
    if error is not None:
        out.append(line("analyze", error))
    else:
        out.extend(_analyzed(ltpage, raw))
    out.append(line("ids", id_ranks(ltpage, raw.index)))
    return out


@terminated
def run(path: str, password: str | None, max_pages: int) -> Iterator[str]:
    with open_document(path, password) as doc:
        rsrcmgr = PDFResourceManager()
        for pageno, page in enumerate(PDFPage.create_pages(doc), start=1):
            yield line("page", pageno, page.pageid)
            yield from _page(rsrcmgr, page, pageno)
            if max_pages and pageno >= max_pages:
                break


__all__ = ["GROUP_TEXT_HEIGHT", "ORACLE_INPUT", "id_ranks"]
