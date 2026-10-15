"""Unit cases of the document layer (``pdfparser``, ``pdfdocument``,
``pdfpage``), replayed by ``crates/pdftrace/tests/document_unit.rs``.

The stage traces of ``oracle/stages/`` over files are the main check of
these modules (``python -m oracle.verify``); this generator makes the same
checks runnable by ``cargo test`` (and so by ``cargo mutants``), and adds
what the stages do not reach:

- ``stages.jsonl``: ``{"in": [name, pdf, password, what, max_pages],
  "out": trace}``, the trace of stage ``what`` (``objects``, ``streams``,
  ``pages``, ``psparser``) of each PDF of the adversarial corpora
  ``adv-objects`` and ``adv-pages`` (regenerated first, from
  ``oracle/adversarial/``), of ``micro`` and of the small files of the
  ``quick`` set (``objects`` and ``pages`` only); ``what`` is also
  ``objects-bytesio`` (the ``objects`` traversal on a parser reading an
  ``io.BytesIO``, whose ``seek``/``read`` errors differ from a file's) and
  ``api`` (``api_trace``: ``get_outlines``, ``get_page_labels``,
  ``lookup_name``, ``get_dest``, ``PDFPage.get_pages``).
- ``bytes_format.jsonl``: ``fmt % arg`` for generated formats and ints
  (``pycompat::bytes_format``, what pdfplumber's ``rotation % 360`` runs
  on a string ``/Rotate``).
- ``pdfobj_cue.jsonl``: ``PDFXRefFallback.PDFOBJ_CUE.match(line)`` on
  generated lines (every Latin-1 character around the pattern).

Every trace runs in a fresh stage thread, from the stage entry depth, as
``oracle/stages/__init__.py`` runs a stage.

Usage: PDFPLUMBER_ROOT=<main checkout> <root>/.venv/bin/python -m oracle.unit.document
"""

from __future__ import annotations

import io
import random
import sys
from collections.abc import Iterator

from pdfminer.pdfdocument import PDFDocument, PDFXRefFallback
from pdfminer.pdfpage import PDFPage
from pdfminer.pdfparser import PDFParser

from ..adversarial import objects as adv_objects
from ..adversarial import pages as adv_pages
from ..canon import canon
from ..corpus import load_set
from ..stages import _in_stage_thread, trace_lines
from ..stages import objects as _objects_stage  # noqa: F401  (loaded before require_oracle_python)
from ..stages import pages as _pages_stage  # noqa: F401
from ..stages import psparser as _psparser_stage  # noqa: F401
from ..stages import streams as _streams_stage  # noqa: F401
from ..stages._common import line, terminated
from ..stages.objects import fetch_order
from ._common import require_oracle_python, write_cases, write_meta

MODULE = "document"
#: Files of the quick set at most this large are included.
QUICK_MAX_BYTES = 60_000
SEED = 7
OUTLINES_MAX = 60
LABELS_MAX = 12


@terminated
def objects_bytesio(path: str, password: str | None, max_pages: int) -> Iterator[str]:
    """The ``objects`` stage on ``PDFParser(io.BytesIO(data))``."""
    with open(path, "rb") as f:
        data = f.read()
    doc = PDFDocument(PDFParser(io.BytesIO(data)), password=password or "")
    objids: dict[object, None] = {}
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


@terminated
def api_trace(path: str, password: str | None, max_pages: int) -> Iterator[str]:
    """The other methods of ``PDFDocument`` and ``PDFPage.get_pages``::

        outline <level> <title> <dest> <action> <se>   (at most OUTLINES_MAX)
        outlines <exception or "end">
        label <label>                                  (at most LABELS_MAX)
        labels <exception or "end">
        lookup <cat> <key> <value or exception>
        dest <name> <value or exception>
        getpages <pagenos> <maxpages> <[pageid, ...] or exception>
    """
    with open(path, "rb") as fp:
        doc = PDFDocument(PDFParser(fp), password=password or "")
        try:
            n = 0
            for entry in doc.get_outlines():
                yield line("outline", *entry)
                n += 1
                if n >= OUTLINES_MAX:
                    break
            yield "outlines end"
        except Exception as e:
            yield "outlines " + canon(e)
        try:
            n = 0
            for label in doc.get_page_labels():
                yield line("label", label)
                n += 1
                if n >= LABELS_MAX:
                    break
            yield "labels end"
        except Exception as e:
            yield "labels " + canon(e)
        for cat, key in [("Dests", b"a"), ("Dests", b"b"), ("Dests", "a"), ("Dests", b"zz"), ("JavaScript", b"a")]:
            try:
                value = canon(doc.lookup_name(cat, key))
            except Exception as e:
                value = canon(e)
            yield line("lookup", cat, key) + " " + value
        for name in [b"a", "a", "b", b"zz", "zz"]:
            try:
                value = canon(doc.get_dest(name))
            except Exception as e:
                value = canon(e)
            yield line("dest", name) + " " + value
        for pagenos, maxpages in [(None, 0), ({1, 2}, 0), (None, 2), ({0}, 1)]:
            fp.seek(0)
            try:
                ids = [p.pageid for p in PDFPage.get_pages(fp, pagenos, maxpages, password or "")]
                value = canon(ids)
            except Exception as e:
                value = canon(e)
            yield line("getpages", sorted(pagenos) if pagenos else None, maxpages) + " " + value


def stage_cases() -> Iterator[tuple[list, str]]:
    sets = [("adv-objects", ("objects", "objects-bytesio", "streams", "pages", "psparser", "api")),
            ("adv-pages", ("objects", "streams", "pages", "psparser", "api")),
            ("micro", ("objects", "objects-bytesio", "streams", "pages", "psparser", "api")),
            ("quick", ("objects", "pages"))]
    seen: set[str] = set()
    for set_name, stages in sets:
        for f in load_set(set_name):
            if f.sha256 in seen or (set_name == "quick" and f.size > QUICK_MAX_BYTES):
                continue
            seen.add(f.sha256)
            data = f.path.read_bytes()
            for what in stages:
                max_pages = 0 if set_name.startswith("adv") else 20
                if what == "objects-bytesio":
                    lines = _in_stage_thread(lambda: objects_bytesio(str(f.path), f.password, max_pages))
                elif what == "api":
                    lines = _in_stage_thread(lambda: api_trace(str(f.path), f.password, max_pages))
                else:
                    lines = trace_lines(what, f.path, f.password, max_pages)
                yield [f.path.name, data, f.password, what, max_pages], "".join(x + "\n" for x in lines)


def bytes_format_cases(rng: random.Random) -> Iterator[tuple[list, object]]:
    pieces = ["%", "%%", "d", "i", "u", "o", "x", "X", "e", "E", "f", "F", "g", "G", "c", "r", "a", "s", "b", "y", "(k)",
              "-", "+", " ", "#", "0", "*", ".", ".*", "5", "12", "0", "h", "l", "L", "|", "ab", "9999999999999999999999"]
    fixed = ["%d", "%5d", "%-5d|", "%05d", "%+d", "% d", "%x", "%#x", "%#X", "%#o", "%o", "%#010x", "%.5d", "%10.5d",
             "%e", "%E", "%.0e", "%#.0e", "%f", "%.0f", "%#.0f", "%.3f", "%g", "%G", "%.1g", "%.2g", "%.3g", "%#.1g",
             "%#g", "%.10g", "%c", "%5c", "%r", "%a", "%.1r", "%s", "%b", "%%", "%%d", "%d%%", "%d%d", "%*d", "%.*d",
             "%", "%-", "%5", "%.", "%l", "%ld", "%(a)d", "%y", "%-08d", "%+08d", "% 08d", "%#08o", "%-#8x|", "x", "",
             "%99999999999999999999d", "%.99999999999d", "%.2147483647d", "%.2147483648d", "abc%dxyz", "%08.3f", "%+.2e"]
    for fmt in fixed:
        for arg in (360, 0, -360, 65, 255, 256, -7):
            yield [fmt.encode("latin-1"), arg], _bytes_mod(fmt.encode("latin-1"), arg)
    for _ in range(4000):
        fmt = "".join(rng.choice(pieces) for _ in range(rng.randrange(1, 6)))
        arg = rng.choice([360, 0, 1, -1, 97, 2**40, -(2**40)])
        yield [fmt.encode("latin-1"), arg], _bytes_mod(fmt.encode("latin-1"), arg)


def _bytes_mod(fmt: bytes, arg: int) -> object:
    try:
        return fmt % arg
    except MemoryError:
        raise
    except Exception as e:
        return e


def pdfobj_cue_cases(rng: random.Random) -> Iterator[tuple[list, object]]:
    def cue(line: bytes) -> object:
        m = PDFXRefFallback.PDFOBJ_CUE.match(line.decode("latin-1"))
        return None if m is None else [g.encode("latin-1") for g in m.groups()]

    for c in range(256):
        b = bytes([c])
        for line_ in (b"1 0 obj" + b, b"1" + b + b"0 obj", b"1 0" + b + b"obj", b + b"1 0 obj", b"1" + b + b" 0 obj\n"):
            yield [line_], cue(line_)
    parts = [b"1", b"23", b"0", b" ", b"\t", b"\r", b"\n", b"\x85", b"\xa0", b"obj", b"ob", b"j", b"x", b"_", b"\xe9", b"R"]
    for _ in range(3000):
        line_ = b"".join(rng.choice(parts) for _ in range(rng.randrange(1, 9)))
        yield [line_], cue(line_)


def main() -> None:
    require_oracle_python()
    rng = random.Random(SEED)
    # the adversarial corpora this module replays, as their generators write them
    adv_objects.main()
    adv_pages.main()
    n = write_cases(MODULE, "stages", stage_cases())
    print(f"{MODULE}: {n} stage traces", file=sys.stderr)
    write_cases(MODULE, "bytes_format", bytes_format_cases(rng))
    write_cases(MODULE, "pdfobj_cue", pdfobj_cue_cases(rng))
    write_meta(MODULE)


if __name__ == "__main__":
    main()
