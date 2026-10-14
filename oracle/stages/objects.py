"""Stage ``objects``: cross-reference tables, trailers and every indirect object.

Exercises pdfminer/pdfdocument.py (``PDFDocument.__init__``: ``find_xref``,
``read_xref_from``, ``PDFXRef``, ``PDFXRefFallback``, ``PDFXRefStream``,
security handlers; ``getobj`` with object streams, ``_getobj_parse`` and its
"objid mismatch" hack, ``decipher_all``), pdfminer/pdfparser.py
(``PDFParser`` object syntax, stream ``/Length`` handling, ``PDFStreamParser``
for object streams) and pdfminer/psparser.py underneath.

Traversal::

    doc = PDFDocument(PDFParser(open(path, "rb")), password=password or "")
    for i, xref in enumerate(doc.xrefs):
        type(xref).__name__, xref.get_trailer()
        for objid in xref.get_objids(): xref.get_pos(objid)
    doc.catalog, doc.info, doc.encryption,
    doc.is_printable, doc.is_modifiable, doc.is_extractable
    for objid in fetch_order(every objid listed above):
        doc.getobj(objid)

``fetch_order``: the distinct objids (distinct under Python ``==``/``hash``,
so ``5`` and ``5.0`` are one key, and the first one seen is kept), ``int``
objids in increasing order first, then the others (a ``PDFXRefFallback`` takes
whatever object an object stream holds at an id position: float, name...) in
the order they were first seen.

Order matters because ``getobj`` caches and has side effects: an object
stream is decoded (``rawdata`` becomes None, see ``oracle/canon.py``) when
the first object stored in it is fetched, and ``_parsed_objs`` keeps its
parsed content. Objects are fetched exactly once each, in ``fetch_order``, after the xref listing; nothing else calls ``getobj`` in between
(``get_objids``/``get_pos`` do not). ``max_pages`` is ignored.

Lines::

    xref <i> <kind>                     kind: PDFXRef | PDFXRefFallback | PDFXRefStream
    trailer <i> <trailer dict>
    entry <i> <objid> <get_pos(objid)>  (strmid, index, genno), in get_objids() order;
                                        the value is an exception line if get_pos raises
    entries <i> <exception>             get_objids() raised after the entries above
    catalog <dict>
    info <list of dicts>
    encryption <None | (id list, Encrypt dict)>
    permissions <is_printable> <is_modifiable> <is_extractable>
    obj <objid> <object>                or <exception> when getobj raises
    !end
"""

from __future__ import annotations

from collections.abc import Iterator

from ..canon import canon
from ._common import line, open_document, terminated

PAGED = False
DEPENDS = ()


def fetch_order(objids: dict[object, None]) -> list[object]:
    ints = sorted(k for k in objids if type(k) is int)
    return ints + [k for k in objids if type(k) is not int]


@terminated
def run(path: str, password: str | None, max_pages: int) -> Iterator[str]:
    with open_document(path, password) as doc:
        objids: dict[object, None] = {}  # insertion-ordered set
        for i, xref in enumerate(doc.xrefs):
            yield line("xref", i) + " " + type(xref).__name__
            yield line("trailer", i, xref.get_trailer())
            try:
                for objid in xref.get_objids():
                    objids.setdefault(objid, None)
                    try:
                        pos = canon(xref.get_pos(objid))
                    except Exception as e:
                        pos = canon(e)
                    yield line("entry", i, objid) + " " + pos
            except Exception as e:
                yield line("entries", i) + " " + canon(e)
        yield line("catalog", doc.catalog)
        yield line("info", doc.info)
        yield line("encryption", doc.encryption)
        yield line("permissions", doc.is_printable, doc.is_modifiable, doc.is_extractable)
        for objid in fetch_order(objids):
            try:
                obj = doc.getobj(objid)
            except Exception as e:
                yield line("obj", objid) + " " + canon(e)
                continue
            yield line("obj", objid, obj)
