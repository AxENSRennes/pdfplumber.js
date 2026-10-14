"""Stage ``psparser``: what ``PDFContentParser`` yields on page and form streams.

Exercises pdfminer/psparser.py (``PSBaseParser``, ``PSStackParser``) through
pdfminer/pdfinterp.py:256 ``PDFContentParser`` (multi-stream reading,
4096-byte buffers, inline images), plus the stream decoding it triggers
(``PDFStream.get_data`` in ``fillfp``).

Traversal (``max_pages`` pages, 0 = all)::

    doc = PDFDocument(PDFParser(open(path, "rb")), password=password or "")
    for pageno, page in enumerate(PDFPage.create_pages(doc), start=1):
        page job, then the form jobs of the page
        stop after page ``max_pages``

Pages are created lazily: a page's jobs run before the next page is created.

Page job (mirrors pdfinterp.py:1387 ``PDFPageInterpreter.execute`` called by
``process_page`` with ``page.contents``)::

    streams = [stream_value(c) for c in list_value(page.contents)]
    keep the streams whose objid is not None (execute's filter)
    parser = PDFContentParser(streams); parser.nextobject() until an exception

Form jobs: the Form XObjects reachable from the page resources, depth-first,
pre-order, each stream at most once per page (by objid). The XObject entries
of a resources dict are found as ``PDFPageInterpreter.init_resources`` finds
them (pdfinterp.py:397: nothing if ``not resources``, else
``dict_value(resources)``, key ``"XObject"``, ``dict_value`` of its value, in
dict order). An entry is a form if ``do_Do`` (pdfinterp.py:1317) would render
it as one: ``xobj = stream_value(entry)``, ``xobj.get("Subtype") is
LITERAL_FORM and "BBox" in xobj``. A form's own resources are
``dict_value(xobj.get("Resources"))`` when that value is truthy; otherwise
the form uses the page resources, already traversed, and adds nothing. Its
job is ``PDFContentParser([xobj])``. (This walks resources, not ``Do``
operators: unused forms are included, and a form reached through several
names is parsed once. The ``interp`` stage covers actual invocations.)

Lines::

    page <pageno> <pageid>
    streams <[objid, ...]>               streams given to PDFContentParser
    <pos> <obj>                          one per nextobject() result
    <exception>                          what ended the job, normally a PSEOF:
                                         !PSEOF("Unexpected EOF, file truncated?")
                                         (fillfp, the streams are exhausted) or
                                         !PSEOF("Unexpected EOF") (nexttoken,
                                         when a token was pending at the end)
    form <[name, ...]> <objid>           path of XObject names from the page
    <pos> <obj> ... <exception>          same as a page job
    xobjects <[name, ...]> <exception>   listing the XObjects of the resources
                                         of the form at that path ([] = page) failed
    form <[name, ...]> <exception>       stream_value() of that entry failed
    !end

A job that fails before parsing (``stream_value`` on a content entry raising)
has its header followed by the exception line only. ``pos`` is the offset in
the stream being read (it restarts at 0 on each stream of a page).
"""

from __future__ import annotations

from collections.abc import Iterator
from typing import Any

from pdfminer.pdfinterp import LITERAL_FORM, PDFContentParser
from pdfminer.pdfpage import PDFPage
from pdfminer.pdftypes import PDFStream, dict_value, list_value, stream_value

from ..canon import canon, int_to_decimal
from ._common import line, open_document, terminated

PAGED = True
DEPENDS = ()


def _parse(streams: list[Any]) -> Iterator[str]:
    try:
        parser = PDFContentParser(streams)
    except Exception as e:
        yield canon(e)
        return
    while True:
        try:
            pos, obj = parser.nextobject()
        except Exception as e:
            yield canon(e)
            return
        yield int_to_decimal(pos) + " " + canon(obj)


def _page_job(page: PDFPage) -> Iterator[str]:
    try:
        streams: list[PDFStream] = []
        for obj in list_value(page.contents):
            stream = stream_value(obj)
            if stream.objid is None:
                continue
            streams.append(stream)
    except Exception as e:
        yield canon(e)
        return
    yield line("streams", [s.objid for s in streams])
    yield from _parse(streams)


def _xobject_entries(resources: Any) -> list[tuple[str, Any]]:
    if not resources:
        return []
    entries: list[tuple[str, Any]] = []
    for k, v in dict_value(resources).items():
        if k == "XObject":
            entries.extend(dict_value(v).items())
    return entries


def _form_jobs(page: PDFPage) -> Iterator[str]:
    visited: set[int] = set()
    # Pending entries: (path, xobject value); popped last-in first-out, so
    # children are pushed in reverse to come out in dict order.
    pending: list[tuple[list[str], Any]] = []

    def push_children(resources: Any, path: list[str]) -> Iterator[str]:
        try:
            entries = _xobject_entries(resources)
        except Exception as e:
            yield line("xobjects", path) + " " + canon(e)
            return
        for name, value in reversed(entries):
            pending.append(([*path, name], value))

    yield from push_children(page.resources, [])
    while pending:
        path, value = pending.pop()
        try:
            xobj = stream_value(value)
            is_form = xobj.get("Subtype") is LITERAL_FORM and "BBox" in xobj
        except Exception as e:
            yield line("form", path) + " " + canon(e)
            continue
        if not is_form or xobj.objid is None or xobj.objid in visited:
            continue
        visited.add(xobj.objid)
        yield line("form", path, xobj.objid)
        yield from _parse([xobj])
        try:
            xobjres = xobj.get("Resources")
            resources = dict_value(xobjres) if xobjres else None
        except Exception as e:
            yield line("xobjects", path) + " " + canon(e)
            continue
        if resources is not None:
            yield from push_children(resources, path)


@terminated
def run(path: str, password: str | None, max_pages: int) -> Iterator[str]:
    with open_document(path, password) as doc:
        for pageno, page in enumerate(PDFPage.create_pages(doc), start=1):
            yield line("page", pageno, page.pageid)
            yield from _page_job(page)
            yield from _form_jobs(page)
            if max_pages and pageno >= max_pages:
                break
