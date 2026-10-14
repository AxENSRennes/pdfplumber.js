"""Stage ``streams``: filters, parameters and decoded data of every stream object.

Exercises pdfminer/pdftypes.py ``PDFStream.get_filters`` / ``decode`` and
everything a stream goes through there: decryption (``decipher``: RC4, AES),
Flate (``zlib`` with the ``decompress_corrupted`` fallback), LZW, ASCII85,
ASCIIHex, RunLength, CCITTFax, predictors (TIFF 2, PNG), pass-through of
DCT/JBIG2/JPX, and the "unsupported filter" errors.

Traversal: the document is opened and the objids are listed exactly as in
the ``objects`` stage (same ``fetch_order``), without writing the xref
lines; then::

    for objid in fetch_order(...):
        obj = doc.getobj(objid)
        if isinstance(obj, PDFStream):
            canon(obj)            # before get_data(): raw is the stored rawdata,
                                  # or None if an earlier getobj already decoded
                                  # it (object streams)
            obj.get_filters()
            obj.get_data()

``max_pages`` is ignored.

Lines::

    getobj <objid> <exception>           getobj raised (non-streams are skipped)
    stream <objid> <Stream(...)>
    filters <objid> <[(filter, params), ...]>   or <exception>
    data <objid> <bytes>                 x"..." or x#len:sha; or <exception>
    !end
"""

from __future__ import annotations

from collections.abc import Iterator

from pdfminer.pdftypes import PDFStream

from ..canon import canon
from ._common import line, open_document, terminated
from .objects import fetch_order

PAGED = False
DEPENDS = ("objects",)  # fetch_order


@terminated
def run(path: str, password: str | None, max_pages: int) -> Iterator[str]:
    with open_document(path, password) as doc:
        objids: dict[object, None] = {}
        for xref in doc.xrefs:
            try:
                for objid in xref.get_objids():
                    objids.setdefault(objid, None)
            except Exception:
                pass  # written by the objects stage; the ids listed so far are kept
        for objid in fetch_order(objids):
            try:
                obj = doc.getobj(objid)
            except Exception as e:
                yield line("getobj", objid) + " " + canon(e)
                continue
            if not isinstance(obj, PDFStream):
                continue
            yield line("stream", objid, obj)
            try:
                filters = canon(obj.get_filters())
            except Exception as e:
                filters = canon(e)
            yield line("filters", objid) + " " + filters
            try:
                data = canon(obj.get_data())
            except Exception as e:
                data = canon(e)
            yield line("data", objid) + " " + data
