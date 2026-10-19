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
``oracle/stages/__init__.py`` runs a stage, in a process forked from this
one for that trace alone (``in_child``): as each trace of ``oracle.verify``
runs in a new process, no trace sees what another one left in the
process-wide state that decides how deep a call goes (the symbols interned
so far, the call sites executed). ``py_process.jsonl`` records that state as the
children start (``process_state``); the replay starts each trace from it
(``pdftrace::stages::py_process``).

Usage: PDFPLUMBER_ROOT=<main checkout> <root>/.venv/bin/python -m oracle.unit.document
"""

from __future__ import annotations

import io
import json
import os
import random
import sys
from collections.abc import Callable, Iterator
from typing import Any

from pdfminer.pdfdocument import PDFDocument, PDFTextExtractionNotAllowed, PDFXRefFallback
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

#: Formats whose result needs more memory than any machine has (a width of
#: at least 10**15 bytes, up to PY_SSIZE_T_MAX): MemoryError wherever they
#: run, so the expected outcome. (Smaller huge widths and precisions fit or
#: not depending on the machine; they are not generated.)
UNALLOCATABLE_FORMATS = [
    b"%999999999999999999d", b"%-999999999999999999d|", b"%0999999999999999999d", b"%+999999999999999d",
    b"%1000000000000000x", b"%#1000000000000000o", b"%9223372036854775806d", b"%9223372036854775807d",
    b"%1000000000000000.3f", b"%1000000000000000c", b"%1000000000000000r", b"abc%1000000000000000d",
]

MODULE = "document"
#: Files of the quick set at most this large are included.
QUICK_MAX_BYTES = 60_000
SEED = 7
OUTLINES_MAX = 60
LABELS_MAX = 12
PAGES_MAX = 12


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
        getpages-checked <[pageid, ...] or exception>  (check_extractable=True; the
                                                        class alone for PDFTextExtractionNotAllowed,
                                                        whose message holds the file name)
        page <f"{page!r}">                             (PDFPage.__repr__, at most PAGES_MAX)
        pages <exception or "end">
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
        fp.seek(0)
        try:
            value = canon([p.pageid for p in PDFPage.get_pages(fp, None, 0, password or "", check_extractable=True)])
        except PDFTextExtractionNotAllowed:
            value = "PDFTextExtractionNotAllowed"
        except Exception as e:
            value = canon(e)
        yield "getpages-checked " + value
        fp.seek(0)
        try:
            n = 0
            for page in PDFPage.get_pages(fp, None, 0, password or ""):
                yield line("page", f"{page!r}")
                n += 1
                if n >= PAGES_MAX:
                    break
            yield "pages end"
        except Exception as e:
            yield "pages " + canon(e)


def in_child(fn: Callable[[], Any]) -> Any:
    """``fn()`` (a JSON value) computed in a child process forked from this
    one, which exits after it: what ``fn`` changes in the process (symbols
    interned, caches) is gone with it. A harness error in the child is
    raised again here."""
    r, w = os.pipe()
    pid = os.fork()
    if pid == 0:  # the child
        os.close(r)
        try:
            data, code = json.dumps({"value": fn()}).encode(), 0
        except BaseException as e:  # noqa: BLE001  (reported to the parent)
            data, code = json.dumps({"error": repr(e)}).encode(), 1
        with os.fdopen(w, "wb") as f:
            f.write(data)
        os._exit(code)
    os.close(w)
    with os.fdopen(r, "rb") as f:
        data = f.read()
    os.waitpid(pid, 0)
    result = json.loads(data)
    if "error" in result:
        raise RuntimeError(f"in a forked child: {result['error']}")
    return result["value"]


def process_state() -> dict[str, Any]:
    """What a child forked now starts with: the names of the interned
    literals (``str`` or ``bytes``) and keywords, and the call sites the
    adaptive interpreter has executed (``oracle.depth.executed_call_sites``:
    the first ``str(x)`` of ``literal_name`` takes one C unit more)."""
    from pdfminer.psparser import PSKeywordTable, PSLiteralTable

    from ..depth import executed_call_sites

    return {
        "literals": sorted(k for k in PSLiteralTable.dict if isinstance(k, str)),
        "bytes_literals": sorted(k for k in PSLiteralTable.dict if isinstance(k, bytes)),
        "keywords": sorted(PSKeywordTable.dict),
        "call_sites": executed_call_sites(),
    }


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
                lines = in_child(lambda: one_trace(what, str(f.path), f.password, max_pages))
                yield [f.path.name, data, f.password, what, max_pages], "".join(x + "\n" for x in lines)


def one_trace(what: str, path: str, password: str | None, max_pages: int) -> list[str]:
    if what == "objects-bytesio":
        return _in_stage_thread(lambda: objects_bytesio(path, password, max_pages))
    if what == "api":
        return _in_stage_thread(lambda: api_trace(path, password, max_pages))
    return trace_lines(what, path, password, max_pages)


def bytes_format_cases(rng: random.Random) -> Iterator[tuple[list, object]]:
    pieces = ["%", "%%", "d", "i", "u", "o", "x", "X", "e", "E", "f", "F", "g", "G", "c", "r", "a", "s", "b", "y", "(k)",
              "-", "+", " ", "#", "0", "*", ".", ".*", "5", "12", "0", "h", "l", "L", "|", "ab", "9999999999999999999999"]
    fixed = ["%d", "%5d", "%-5d|", "%05d", "%+d", "% d", "%x", "%#x", "%#X", "%#o", "%o", "%#010x", "%.5d", "%10.5d",
             "%e", "%E", "%.0e", "%#.0e", "%f", "%.0f", "%#.0f", "%.3f", "%g", "%G", "%.1g", "%.2g", "%.3g", "%#.1g",
             "%#g", "%.10g", "%c", "%5c", "%r", "%a", "%.1r", "%s", "%b", "%%", "%%d", "%d%%", "%d%d", "%*d", "%.*d",
             "%", "%-", "%5", "%.", "%l", "%ld", "%(a)d", "%y", "%-08d", "%+08d", "% 08d", "%#08o", "%-#8x|", "x", "",
             "%99999999999999999999d", "%.99999999999d", "%.2147483647d", "%.2147483648d", "abc%dxyz", "%08.3f", "%+.2e",
             # widths of several digits, and around the last digit of PY_SSIZE_T_MAX
             "%12d", "%-15d|", "%021x", "%13.4f", "%9223372036854775808d", "%9223372036854775809d", "%.0r", "%.0a", "%.3r",
             # float precisions past the digits of the float of an int
             "%.41f", "%.50f", "%.45e", "%.45E", "%#.60g", "%.100g", "%#.41G", "%60.45f|", "%-+60.42e|",
             "%.2147483649d", "%.2147483645d"]
    for fmt in fixed:
        for arg in (360, 0, -360, 65, 255, 256, -7):
            yield [fmt.encode("latin-1"), arg], _bytes_mod(fmt.encode("latin-1"), arg)
    for fmt_b in UNALLOCATABLE_FORMATS:
        for arg in (360, -7):
            yield [fmt_b, arg], _bytes_mod(fmt_b, arg)
    # a * width or precision taken from the argument: C int bounds
    for fmt_b in (b"%.*d", b"%*d", b"%-*d"):
        for arg in (2**31 - 1, 2**31, -(2**31), -(2**31) - 1):
            yield [fmt_b, arg], _bytes_mod(fmt_b, arg)
    for _ in range(4000):
        fmt = "".join(rng.choice(pieces) for _ in range(rng.randrange(1, 6)))
        arg = rng.choice([360, 0, 1, -1, 97, 2**40, -(2**40)])
        yield [fmt.encode("latin-1"), arg], _bytes_mod(fmt.encode("latin-1"), arg)


def _bytes_mod(fmt: bytes, arg: int) -> object:
    try:
        return fmt % arg
    except MemoryError as e:
        if fmt in UNALLOCATABLE_FORMATS:
            return e
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
    state = process_state()
    write_cases(MODULE, "py_process", [([], state)])
    n = write_cases(MODULE, "stages", stage_cases())
    if process_state() != state:
        raise RuntimeError("the process state changed while the traces ran in children")
    print(f"{MODULE}: {n} stage traces", file=sys.stderr)
    write_cases(MODULE, "bytes_format", bytes_format_cases(rng))
    write_cases(MODULE, "pdfobj_cue", pdfobj_cue_cases(rng))
    write_meta(MODULE)


if __name__ == "__main__":
    main()
