"""Helpers shared by the stages.

Opening a document
------------------
``open_document`` does exactly what pdfplumber/pdf.py:28 ``PDF.__init__``
does (at its line 50), minus the ``PdfminerException`` wrapper::

    fp = open(path, "rb")
    doc = PDFDocument(PDFParser(fp), password=password or "")

so ``caching=True`` and ``fallback=True`` (the defaults), ``settings.STRICT``
untouched (False). An exception raised there ends the trace (it is the
exception a pdfplumber user gets, unwrapped).

Trace termination
-----------------
``terminated`` turns a stage generator into a full trace: it appends ``!end``
if the generator finishes, or the canonical line of the exception that
escaped it (``!PDFSyntaxError("...")``) and stops. Exceptions that a stage
catches itself, around one item of a loop, are written by the stage in place
of that item and the loop goes on.
"""

from __future__ import annotations

import functools
from collections.abc import Callable, Iterator
from contextlib import contextmanager

from pdfminer.pdfdocument import PDFDocument
from pdfminer.pdfparser import PDFParser

from ..canon import canon

StageFn = Callable[[str, "str | None", int], Iterator[str]]


def terminated(fn: StageFn) -> StageFn:
    @functools.wraps(fn)
    def run(path: str, password: str | None, max_pages: int) -> Iterator[str]:
        try:
            yield from fn(path, password, max_pages)
        except Exception as e:
            yield canon(e)
            return
        yield "!end"

    return run


@contextmanager
def open_document(path: str, password: str | None) -> Iterator[PDFDocument]:
    with open(path, "rb") as fp:
        yield PDFDocument(PDFParser(fp), password=password or "")


def line(label: str, *values: object) -> str:
    """``label v1 v2 ...`` with every value through ``canon``."""
    return " ".join([label, *(canon(v) for v in values)])
