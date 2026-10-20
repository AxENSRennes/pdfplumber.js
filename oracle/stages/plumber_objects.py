"""Stage ``plumber_objects``: pdfplumber's page objects, every kind with every
attribute, the page boxes and ``doctop``, the annotations and hyperlinks,
``pdf.objects``; then the same document opened with ``laparams={}`` (the
text boxes and lines of pdfminer's analysis become objects).

Exercises pdfplumber/pdf.py (``PDF.open``, ``PDF.__init__``: one
``PDFResourceManager``, ``pages=`` selection, ``PDF.pages`` with the
``doctop`` accumulation, ``PDF.objects``, ``PDF.annots``,
``PDF.hyperlinks``), pdfplumber/page.py (``Page.__init__``, ``Page.layout``
on ``PDFPageAggregatorWithMarkedContent``, ``Page.objects`` /
``parse_objects`` / ``iter_layout_objects`` / ``process_object`` /
``point2coord``, ``fix_fontname_bytes``, ``Page.annots`` and its
``rotate_point``/``parse``, ``Page.hyperlinks``),
pdfplumber/utils/pdfinternals.py (``resolve_all``, ``resolve_and_decode``,
``decode_text``) and everything below them (the interpreter, the converter,
pdfminer's analysis under ``laparams={}``).

Opening (shared by the four ``plumber_*`` stages, ``open_pdf``): exactly
what a user writes::

    pdf = pdfplumber.open(path, pages=PAGES, password=password, **options)

``PAGES`` is ``list(range(1, max_pages + 1))`` (``None`` when ``max_pages``
is 0): pdfplumber/pdf.py:145 ``PDF.pages`` walks the whole page tree
(``PDFPage.create_pages``, an exception there is ``PdfminerException``) and
builds a ``Page`` for the selected numbers only, accumulating ``doctop``
over them. Pages are built lazily: on the first ``pdf.pages``, then cached
(``_pages``). ``pdf.close()`` is not called here (``plumber_doc`` does).
The stages run with the trace runner's settings (fresh thread, logging
disabled: ``oracle/stages/__init__.py``), no patching.

Exceptions: ``exc(e)`` writes ``canon(e)``, followed, for an exception of
pdfplumber that wraps another one (``PdfminerException(e)``,
``MalformedPDFException(e)``: a single argument that is an exception), by
`` cause `` and the wrapped exception, recursively: the class of the
cause is what a JavaScript user reads in ``error.cause``. The ``set`` of
directions that ``validate_directions`` prints, whose order depends on
``PYTHONHASHSEED``, is written in the order of ``POSITION_KEYS``
(``VALID_DIRS_REPR``, the port's).

Inline images: their ``name`` (an image's, and with ``laparams`` its
figure's) is ``str(id(stream))`` (pdfminer/pdfinterp.py:1309
``PDFPageInterpreter.do_EI``), an address; ``Names`` writes such a name
``inline{"n": k}``, ``k`` numbering the distinct inline names of the page
in the order the lines first write them (the port names them
``inline-<j>``, ``docs/architecture.md``, "Vague 3"). A value that is a
pdfplumber ``Page`` (``annot["P"]``, which ``Page.annots`` replaces by the
page itself) is written ``Page{"page_number": n}``.

Objects (``Objects.write``): for each kind of ``page.objects``, in the
dict's order, each object as the tuple of its values; a ``keys`` line gives
the attribute names before the first object of a kind and again whenever
they change (``mcid``/``tag`` are not on every object).

Traversal::

    pdf = open_pdf(path, password, max_pages)
    for page in pdf.pages:
        the boxes, page.objects, page.annots, page.hyperlinks
    pdf.objects, pdf.annots, pdf.hyperlinks
    pdf = open_pdf(path, password, max_pages, laparams={})
    for page in pdf.pages:
        page.objects, and the ids of the analysis

``laparams={}`` is ``LAParams()``: ``Page.layout`` is the analyzed page
(pdfminer/converter.py:81 ``PDFLayoutAnalyzer.end_page`` analyzes it), and
``iter_layout_objects`` yields each container (text box, text line,
figure) before its children: ``textboxhorizontal``, ``textlinehorizontal``
(and vertical) objects with their ``text``, and the chars in the order of
the analysis. That order depends on Python's ``id()`` of the boxes and
groups where boxes are at equal distance (``crates/pdfminer/src/layout.rs``,
"Analysis and object identity"): for each page whose ``objects`` were
computed, the ``ids`` line gives the ``id()`` ranking of the boxes and
groups of its analysis (in ``page.layout.groups`` and, for an analysis that
reaches them, its figures', as ``oracle/stages/layout.py`` ``id_ranks``),
keyed by content, since the raw order of the chars is not visible through
pdfplumber: a box's key is the first 16 hex digits of the SHA-256 of
``"box\\n"`` followed by the sorted canonical texts of its chars, joined by
``"\\n"``, each char written ``LTChar{...}`` with its ``__dict__`` but
``bbox`` (graphic state and color space as in the ``layout`` stage's
``gstate``/``colorspace`` lines, in full); a group's key, that of
``"group\\n"`` and the sorted keys of its elements. Elements with the same
key are interchangeable (same chars, same geometry): the port gives a key's
ranks, in increasing order, to its elements in the order it asks for their
ids. ``oracle.verify`` hands this trace to ``pdftrace`` (``ORACLE_INPUT``),
whose library takes these ranks through an injected ``IdSource``; the
library and JavaScript default stay ``CreationOrder``.

Lines::

    open <exc>                           pdfplumber.open raised (nothing else follows)
    pages <exc>                          pdf.pages raised (the pdf lines follow)
    page <page_number>
    box <name> <value>                   rotation, mediabox, cropbox, [trimbox, bleedbox,
                                         artbox: when set], bbox, initial_doctop, width,
                                         height
    objects <exc>                        page.objects raised
    kinds <[kind, ...]>                  the keys of page.objects
    keys <kind> <[attr, ...]>
    obj <kind> <i> <(value, ...)>
    annots <exc> | annots <n>            page.annots, then each:
    annot <i> <dict>
    hyperlinks <exc> | hyperlinks <n>    page.hyperlinks, then each:
    hyperlink <i> <dict>
    pdf objects <exc> | pdf objects <[[kind, n], ...]>
    pdf annots <exc> | pdf annots <n> <digest>
    pdf hyperlinks <exc> | pdf hyperlinks <n> <digest>
    laparams <dict>                      the second opening (its own open/pages/page/
                                         objects/kinds/keys/obj lines follow)
    ids <page_number> <[[key, rank], ...]>
                                         laparams only, after the objects of a page
                                         whose layout was computed
    !end

``<digest>`` is ``digest(value)``: the first 16 hex digits of the SHA-256
of ``canon`` of the value (after ``Names``).
"""

from __future__ import annotations

import hashlib
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import pdfplumber
from pdfminer.layout import LTChar, LTFigure, LTTextBox
from pdfminer.pdfcolor import PDFColorSpace
from pdfplumber.page import Page
from pdfplumber.utils.exceptions import MalformedPDFException, PdfminerException
from pdfplumber.utils.text import POSITION_KEYS

from ..canon import ExcValue, Named, canon, exception_message, exception_name
from ._common import terminated
from .interp import colorspace, state_fields

PAGED = True
DEPENDS = ("interp",)
#: ``oracle.verify`` gives ``pdftrace`` the oracle's trace (the ``ids`` lines).
ORACLE_INPUT = True

BOXES = ("rotation", "mediabox", "cropbox", "trimbox", "bleedbox", "artbox", "bbox", "initial_doctop", "width", "height")


# --------------------------------------------------------------------------- shared by the plumber_* stages


def open_pdf(path: str, password: str | None, max_pages: int, **options: Any) -> pdfplumber.PDF:
    """``pdfplumber.open(path, pages=PAGES, password=password, **options)``
    (see the module docstring)."""
    pages = list(range(1, max_pages + 1)) if max_pages else None
    return pdfplumber.open(path, pages=pages, password=password, **options)


#: ``str(valid_dirs)`` in the messages of pdfplumber/utils/text.py:132
#: ``validate_directions``: a ``set`` of ``str``, whose order changes with
#: ``PYTHONHASHSEED``; written in the order of ``POSITION_KEYS``, as the port
#: prints it (``oracle/unit/_plumber_ops.py``).
VALID_DIRS_REPR = "{'ttb', 'btt', 'ltr', 'rtl'}"
_PROCESS_DIRS_REPR = str(set(POSITION_KEYS.keys()))


def _canon_exc(e: BaseException) -> str:
    return canon(ExcValue(exception_name(e), exception_message(e).replace(_PROCESS_DIRS_REPR, VALID_DIRS_REPR)))


def exc(e: BaseException) -> str:
    """``canon(e)``, and `` cause <exc>`` for a pdfplumber exception that
    wraps another one. The set of directions in the message of
    ``validate_directions`` is written in a fixed order
    (``VALID_DIRS_REPR``)."""
    parts = [_canon_exc(e)]
    while isinstance(e, (PdfminerException, MalformedPDFException)) and len(e.args) == 1 and isinstance(e.args[0], BaseException):
        e = e.args[0]
        parts.append("cause " + _canon_exc(e))
    return " ".join(parts)


def file_sha256(path: str) -> str:
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def select(sha256: str, index: int, variants: list[Any], k: int) -> list[Any]:
    """``k`` consecutive entries of ``variants`` (cyclically) for the
    ``index``-th item (a page, from 0) of the file whose SHA-256 is
    ``sha256``: from ``(int(sha256[:8], 16) + index * k) % len(variants)``.
    Deterministic, different from file to file and from page to page."""
    if not variants:
        return []
    start = (int(sha256[:8], 16) + index * k) % len(variants)
    return [variants[(start + j) % len(variants)] for j in range(min(k, len(variants)))]


def digest(value: Any) -> str:
    return hashlib.sha256(canon(value).encode("ascii")).hexdigest()[:16]


class Names:
    """The values of a page as written: inline image names numbered, a
    ``Page`` as ``Page{"page_number": n}``."""

    def __init__(self) -> None:
        self.inline: dict[str, int] = {}

    def learn(self, objects: dict[str, list[dict[str, Any]]]) -> None:
        """Records the inline names of the images of ``objects``, in order."""
        for objs in objects.values():
            for o in objs:
                if o.get("object_type") == "image" and "stream" in o and o.get("name") == str(id(o["stream"])):
                    self.inline.setdefault(o["name"], len(self.inline))

    def value(self, v: Any) -> Any:
        if isinstance(v, Page):
            return Named("Page", {"page_number": v.page_number})
        if isinstance(v, str) and v in self.inline:
            return Named("inline", {"n": self.inline[v]})
        return v

    def obj(self, o: dict[str, Any]) -> dict[str, Any]:
        """An object dict with its ``name`` and top-level ``Page`` values
        written as above (``Page`` only appears as ``annot["P"]``)."""
        return {k: (self.value(v) if k in ("name", "P") else v) for k, v in o.items()}


class Objects:
    """The ``kinds``/``keys``/``obj`` lines of a page's objects."""

    def __init__(self, names: Names) -> None:
        self.names = names
        self.keys: dict[str, list[str]] = {}

    def write(self, objects: dict[str, list[dict[str, Any]]], label: str = "") -> Iterator[str]:
        self.names.learn(objects)
        yield label + "kinds " + canon(list(objects.keys()))
        for kind, objs in objects.items():
            for i, o in enumerate(objs):
                o = self.names.obj(o)
                keys = list(o.keys())
                if self.keys.get(kind) != keys:
                    self.keys[kind] = keys
                    yield label + "keys " + canon(kind) + " " + canon(keys)
                yield label + "obj " + canon(kind) + " " + str(i) + " " + canon(tuple(o.values()))


def boxes(page: Page) -> Iterator[str]:
    for name in BOXES:
        if hasattr(page, name):
            yield "box " + name + " " + canon(getattr(page, name))


# ------------------------------------------------------------------------------------- ids


def _char_text(c: LTChar) -> str:
    fields: dict[str, Any] = {}
    for k, v in vars(c).items():
        if k == "bbox":
            continue
        if k == "graphicstate":
            v = Named("PDFGraphicState", state_fields(vars(v)))
        elif isinstance(v, PDFColorSpace):
            v = colorspace(v)
        fields[k] = v
    return canon(Named("LTChar", fields))


def _key(prefix: str, texts: list[str]) -> str:
    return hashlib.sha256((prefix + "\n" + "\n".join(sorted(texts))).encode("ascii")).hexdigest()[:16]


def content_ids(ltpage: Any) -> list[list[Any]]:
    """``[key, rank]`` of the boxes and groups of the analysis of
    ``ltpage`` (its groups and its figures', breadth-first as
    ``oracle/stages/layout.py`` ``id_ranks``), sorted by rank (no
    recursion)."""
    found: list[tuple[int, str]] = []
    keys: dict[int, str] = {}
    containers: list[Any] = [ltpage]
    roots: list[Any] = []
    while containers:
        c = containers.pop(0)
        containers[:0] = [o for o in c._objs if isinstance(o, LTFigure)]
        roots.extend(getattr(c, "groups", None) or [])
    for root in roots:
        stack: list[tuple[Any, bool]] = [(root, False)]
        while stack:
            g, done = stack.pop()
            if isinstance(g, LTTextBox):
                keys[id(g)] = _key("box", [_char_text(c) for tl in g for c in tl if isinstance(c, LTChar)])
                found.append((id(g), keys[id(g)]))
            elif done:
                keys[id(g)] = _key("group", [keys[id(c)] for c in g._objs])
                found.append((id(g), keys[id(g)]))
            else:
                stack.append((g, True))
                stack.extend((c, False) for c in reversed(list(g._objs)))
    found.sort()
    return [[key, rank] for rank, (_, key) in enumerate(found)]


# ----------------------------------------------------------------------------------- stage


def annot_value(a: dict[str, Any], names: Names) -> dict[str, Any]:
    """An annotation of ``Page.annots`` as written (its ``data`` dict
    holds the ``Page`` in place of ``/P``)."""
    return names.obj({**a, "data": names.obj(a["data"])})


def annots_lines(label: str, item: str, get: Any, names: Names) -> Iterator[str]:
    try:
        annots = get()
    except Exception as e:
        yield label + " " + exc(e)
        return
    yield f"{label} {len(annots)}"
    for i, a in enumerate(annots):
        yield f"{item} {i} " + canon(annot_value(a, names))


def objects_lines(page: Page) -> Iterator[str]:
    """The ``objects`` or ``kinds``/``keys``/``obj`` lines of a page."""
    names = Names()
    try:
        objs = page.objects
    except Exception as e:
        yield "objects " + exc(e)
        return
    yield from Objects(names).write(objs)


def page_lines(page: Page) -> Iterator[str]:
    yield f"page {page.page_number}"
    yield from boxes(page)
    yield from objects_lines(page)
    names = Names()
    yield from annots_lines("annots", "annot", lambda: page.annots, names)
    yield from annots_lines("hyperlinks", "hyperlink", lambda: page.hyperlinks, names)


def _pdf_lines(pdf: pdfplumber.PDF) -> Iterator[str]:
    try:
        objs = pdf.objects
    except Exception as e:
        yield "pdf objects " + exc(e)
    else:
        yield "pdf objects " + canon([[k, len(v)] for k, v in objs.items()])
    names = Names()
    for what in ("annots", "hyperlinks"):
        try:
            values = getattr(pdf, what)
        except Exception as e:
            yield f"pdf {what} " + exc(e)
        else:
            yield f"pdf {what} {len(values)} " + digest([annot_value(a, names) for a in values])


def _laparams_lines(path: str, password: str | None, max_pages: int) -> Iterator[str]:
    yield "laparams {}"
    try:
        pdf = open_pdf(path, password, max_pages, laparams={})
    except Exception as e:
        yield "open " + exc(e)
        return
    try:
        pages = pdf.pages
    except Exception as e:
        yield "pages " + exc(e)
        return
    for page in pages:
        yield f"page {page.page_number}"
        lines = list(objects_lines(page))
        yield from lines
        if not lines[0].startswith("objects "):
            yield f"ids {page.page_number} " + canon(content_ids(page.layout))


@terminated
def run(path: str, password: str | None, max_pages: int) -> Iterator[str]:
    try:
        pdf = open_pdf(path, password, max_pages)
    except Exception as e:
        yield "open " + exc(e)
        return
    try:
        pages = pdf.pages
    except Exception as e:
        yield "pages " + exc(e)
        pages = []
    for page in pages:
        yield from page_lines(page)
    yield from _pdf_lines(pdf)
    yield from _laparams_lines(path, password, max_pages)


__all__ = [
    "BOXES",
    "Names",
    "Objects",
    "annot_value",
    "annots_lines",
    "boxes",
    "content_ids",
    "digest",
    "exc",
    "file_sha256",
    "objects_lines",
    "open_pdf",
    "page_lines",
    "select",
]
