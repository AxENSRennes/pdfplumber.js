"""Stage ``pages``: the page tree as ``PDFPage.create_pages`` builds it, and
pdfplumber's page boxes.

Exercises pdfminer/pdfpage.py (``create_pages``: depth-first walk with
inheritance of Resources/MediaBox/CropBox/Rotate, the ``visited`` guard, the
``type`` fallback, the no-``/Pages`` fallback over the xrefs;
``PDFPage.__init__``: ``_parse_mediabox``, ``_parse_cropbox``,
``_parse_contents``, rotation), pdfminer/pdfdocument.py
``get_page_labels`` + ``PageLabels`` (pdfminer/utils.py ``NumberTree``), and
pdfplumber/page.py:191 ``Page.__init__`` (``_normalize_box``, ``_invert_box``,
rotation, MediaBox/CropBox/TrimBox/BleedBox/ArtBox).

pdfminer's ``PDFPage`` has no TrimBox/BleedBox/ArtBox attributes: these
boxes exist only as raw entries of ``attrs`` (written on the ``attrs`` line)
and as pdfplumber ``Page`` attributes (the ``plumber`` lines).

Traversal (``max_pages`` pages, 0 = all; pages are created lazily, the
generator is not resumed after page ``max_pages``)::

    doc = PDFDocument(PDFParser(open(path, "rb")), password=password or "")
    doctop = 0
    for pageno, page in enumerate(PDFPage.create_pages(doc), start=1):
        the pdfminer attributes of ``page``
        p = pdfplumber.page.Page(None, page, page_number=pageno, initial_doctop=doctop)
        the box attributes of ``p``; doctop += p.height

as in pdfplumber/pdf.py:145 ``PDF.pages``. ``Page.__init__`` only reads
``page_obj.attrs`` (it stores ``pdf`` and never uses it there), so passing
``None`` for the ``PDF`` cannot change a value. It reads each box through
pdfplumber's resolve_all (pdfplumber/utils/pdfinternals.py:66 ``resolve_all``), which
builds resolved copies (stopping at ``/Page`` dicts) and turns a
``RecursionError`` on a reference cycle into ``MalformedPDFException``: that
error depends on the stack depth, fixed by the runner (``STAGE_ENTRY_DEPTH``).
The pdfminer lines of a page are written before ``Page`` is built, so the
``getobj`` calls of ``resolve_all`` come after them. If ``Page.__init__``
raises (a box that is not four numbers...), the exception is written in place
of the ``plumber`` lines and ``doctop`` is not advanced; pdfplumber itself
would fail on ``pdf.pages``.

Lines (per page)::

    page <pageno> <pageid>
    label <label>                        str or None
    attrs <dict>                         page dict plus inherited entries
    lastmod <value>
    resources <value>
    mediabox <tuple>
    cropbox <tuple>
    rotate <int>
    contents <list>
    annots <value>
    beads <value>
    plumber rotation <value>
    plumber mediabox <tuple>
    plumber cropbox <tuple>
    plumber trimbox <tuple>              only if the page has /TrimBox
    plumber bleedbox <tuple>             only if the page has /BleedBox
    plumber artbox <tuple>               only if the page has /ArtBox
    plumber bbox <tuple>
    plumber initial_doctop <number>
    plumber width <number>
    plumber height <number>
    plumber <exception>                  Page.__init__ (or width/height) raised
    !end
"""

from __future__ import annotations

from collections.abc import Iterator

from pdfminer.pdfpage import PDFPage
from pdfplumber.page import Page

from ..canon import canon
from ._common import line, open_document, terminated

PAGED = True
DEPENDS = ()

_PDFMINER_FIELDS = ("label", "attrs", "lastmod", "resources", "mediabox", "cropbox", "rotate", "contents", "annots", "beads")
_PLUMBER_BOXES = ("rotation", "mediabox", "cropbox", "trimbox", "bleedbox", "artbox", "bbox", "initial_doctop")


@terminated
def run(path: str, password: str | None, max_pages: int) -> Iterator[str]:
    with open_document(path, password) as doc:
        doctop: object = 0
        for pageno, page in enumerate(PDFPage.create_pages(doc), start=1):
            yield line("page", pageno, page.pageid)
            for field in _PDFMINER_FIELDS:
                yield line(field, getattr(page, field))
            try:
                p = Page(None, page, page_number=pageno, initial_doctop=doctop)  # type: ignore[arg-type]
                values = [(name, getattr(p, name)) for name in _PLUMBER_BOXES if hasattr(p, name)]
                values += [("width", p.width), ("height", p.height)]
            except Exception as e:
                yield "plumber " + canon(e)
            else:
                for name, value in values:
                    yield line("plumber " + name, value)
                doctop += p.height  # type: ignore[operator]
            if max_pages and pageno >= max_pages:
                break
