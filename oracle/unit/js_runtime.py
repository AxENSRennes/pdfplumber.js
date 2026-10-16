"""Cases of the JavaScript runtime (crates/wasm, js/): what the JavaScript
tests check through the WebAssembly boundary, in Node and in every browser.

Writes into ``<cache>/unit/js_runtime/`` (see ``oracle/unit/_common.py``), in
typed JSON:

- ``exceptions.jsonl``: ``{"in": ["module.qualname"], "out": [mro...]}`` for
  every exception class the port names (pdfminer, pdfplumber, and the
  standard library ones of ``oracle.unit.pycompat.exc_classes``), the MRO as
  ``module.qualname`` strings (``type(e).__mro__``, ``object`` included):
  what a JavaScript error carries as ``pythonMro``;
- ``cmaps.jsonl``: predefined CMaps through ``CMapDB`` (the lazily loaded
  files of ``dist/cmaps/``): ``{"in": ["decode", name, code], "out": [cids]}``
  for ``list(CMapDB.get_cmap(name).decode(code))`` on random codes, and
  ``{"in": ["unichr", collection, vertical, [cids]], "out": [str or
  ["KeyError", str(e)]]}`` for ``CMapDB.get_unicode_map(...).get_unichr``;
- ``documents.jsonl``: CJK documents whose fonts use predefined CMaps:
  ``{"in": [path, page_number], "out": {"text": page.extract_text(),
  "cmaps": [...], "umaps": [...]}}``, the names of the CMaps and unicode
  maps pdfminer loaded for the page (from empty ``CMapDB`` caches): what
  the lazy loading of ``dist/cmaps/`` must fetch (replayed once the page
  layer lands);
- ``text.jsonl``: pages of real PDFs (``chars``, ``edges``, ``bbox`` of
  pdfplumber) and the results of the pdfplumber functions the boundary
  exposes before the page layer: ``utils.extract_text``,
  ``utils.extract_words``, ``utils.dedupe_chars`` (the indices of the
  chars it keeps), ``TextMap.search`` (on
  ``utils.chars_to_textmap``) with patterns whose matches are the same under
  JavaScript ``RegExp`` semantics, ``TextMap.extract_text_lines``, and
  ``TableFinder`` (``bbox``, ``cells``, ``rows``, ``columns``,
  ``extract``). One line per page: ``{"in": [path, page_number, chars,
  edges, bbox], "out": {"ops": [[op, args, result], ...]}}``;
- ``search_groups.jsonl``: ``TextMap.search`` with every kind of
  ``main_group`` (an int, a bool, a group name, values ``m.group()``
  refuses), evaluated for each match: ``{"in": [chars, pattern,
  main_group, matches], "out": result}``, the pattern in JavaScript syntax
  (``(?<name>...)``; Python's ``(?P<name>...)`` was run), ``matches`` the
  spans of each match (as the UTF-16 offsets of a ``RegExp`` with the
  ``d`` flag, which finds the same matches) as ``[spans, named]``, ``named``
  the spans of the named groups (``indices.groups``), ``None`` for a
  pattern without any;
- ``open.jsonl``: ``pdfplumber.open(path, **options)`` with options
  pdfplumber accepts at ``open()`` whatever their value (they fail later,
  or never): ``{"in": [path, options], "out": None or the exception}``.

The JavaScript tests (js/test/shared/) replay them: Node through vitest,
browsers through Playwright, which read them with ``js/test/oracle-data.mjs``.

Usage: PDFPLUMBER_ROOT=<main checkout> <root>/.venv/bin/python -m oracle.unit.js_runtime
"""

from __future__ import annotations

import importlib
import pkgutil
import random
import re

import pdfplumber
from pdfminer.cmapdb import CMapDB
from pdfplumber.table import TableFinder, TableSettings
from pdfplumber.utils import text as utext

from ..corpus import corpus_root
from ._common import require_oracle_python, write_cases, write_meta
from .exceptions import SKIP, classes_in
from .pycompat import exc_classes

MODULE = "js_runtime"

PAGES = [
    ("corpus/micro/text-operators.pdf", 1),
    ("corpus/micro/vertical-rtl-text.pdf", 1),
    ("corpus/micro/vector-objects.pdf", 1),
    ("upstream/pdfplumber-python/tests/pdfs/pdffill-demo.pdf", 1),
    ("upstream/pdfplumber-python/tests/pdfs/scotus-transcript-p1.pdf", 1),
    ("upstream/pdfplumber-python/tests/pdfs/issue-71-duplicate-chars.pdf", 1),
    ("upstream/pdfplumber-python/tests/pdfs/nics-background-checks-2015-11.pdf", 1),
]

TEXT_KWARGS = [{}, {"layout": True}, {"x_tolerance": 1.5, "y_tolerance": 1}, {"keep_blank_chars": True}]
WORD_KWARGS = [{}, {"keep_blank_chars": True, "use_text_flow": True}, {"split_at_punctuation": True}, {"extra_attrs": ["fontname", "size"]}]
# (pattern, regex, case, return_groups, return_chars, main_group): the same
# matches under Python's re and JavaScript's RegExp (ASCII classes, no empty
# matches next to a non-empty one, no lookbehind)
SEARCHES = [
    ("the", True, True, True, True, 0),
    ("THE", True, False, True, False, 0),
    ("[A-Z][a-z]+", True, True, False, False, 0),
    (r"([0-9]+)[.,]([0-9]+)", True, True, True, False, 0),
    (r"([0-9]+)[.,]([0-9]+)", True, True, True, False, 2),
    (r"(x)?([a-z]+)ion", True, True, True, False, 0),
    ("a.b", False, True, True, False, 0),
    ("(", False, True, False, False, 0),
    ("e", True, True, False, False, 0),
]
TEXTMAP_KWARGS = [{}, {"layout": True}]
# (Python pattern, JavaScript pattern) and main_group values: any value
# m.group() takes, read for each match, on the text map of "ab cd":
# pdfplumber/utils/text.py:172 TextMap.search
GROUP_PATTERNS = [
    ("zzz", "zzz"),
    ("(?P<w>ab)", "(?<w>ab)"),
    ("(?P<w>x)?(?P<v>ab)", "(?<w>x)?(?<v>ab)"),
    ("(a)(b)", "(a)(b)"),
    ("b|(?P<w>c)", "b|(?<w>c)"),
]
MAIN_GROUPS = [0, 1, 2, -1, 5, True, False, "w", "v", "zz", None, 1.5, 2**70, -(2**70), b"w", [], {}]
# and on a text with an astral character (UTF-16 offsets differ from
# Python's code point offsets after it)
ASTRAL_TEXT = "a\U0001F600b ab"
ASTRAL_PATTERNS = [("b", "b"), ("(?P<w>\U0001F600)(b)", r"(?<w>\u{1F600})(b)"), ("(?P<w>b) (a)", "(?<w>b) (a)")]
ASTRAL_GROUPS = [0, 1, 2, "w"]
OPEN_PATH = "corpus/micro/text-operators.pdf"
OPEN_OPTIONS = [
    {"unicode_norm": "X"},
    {"unicode_norm": None},
    {"pages": "1"},
    {"pages": [1.0]},
    {"pages": [1]},
    {"password": b"x"},
    {"password": "x"},
    {"strict_metadata": 1},
    {"laparams": {"line_margin": 0.3}},
]
TABLE_SETTINGS = [None, {"vertical_strategy": "text", "horizontal_strategy": "text"}, {"snap_tolerance": 1}]

DOCUMENTS = [
    ("upstream/pdfjs/test/pdfs/noembed-sjis.pdf", 1),
    ("upstream/pdfplumber-python/tests/pdfs/pr-136-example.pdf", 1),
]

CMAP_NAMES = ["UniJIS-UCS2-H", "UniJIS-UCS2-V", "90ms-RKSJ-H", "GBK-EUC-H", "UniGB-UCS2-H", "B5pc-H", "UniCNS-UCS2-H", "KSC-EUC-H", "UniKS-UCS2-H", "EUC-V"]
COLLECTIONS = ["Adobe-Japan1", "Adobe-GB1", "Adobe-CNS1", "Adobe-Korea1"]


def exception_cases():
    found = {}
    for package_name in ("pdfminer", "pdfplumber"):
        package = importlib.import_module(package_name)
        names = [package_name] + [m.name for m in pkgutil.walk_packages(package.__path__, package_name + ".")]
        for name in names:
            if name in SKIP:
                continue
            for cls in classes_in(importlib.import_module(name), set()):
                if issubclass(cls, Exception) and cls.__module__.split(".")[0] in ("pdfminer", "pdfplumber"):
                    found[f"{cls.__module__}.{cls.__qualname__}"] = cls
    for cls in exc_classes():
        found[f"{cls.__module__}.{cls.__qualname__}"] = cls
    for qualified, cls in sorted(found.items()):
        yield [qualified], [f"{c.__module__}.{c.__qualname__}" for c in cls.__mro__]


def document_cases():
    root = corpus_root()
    for rel, number in DOCUMENTS:
        CMapDB._cmap_cache.clear()
        CMapDB._umap_cache.clear()
        with pdfplumber.open(root / rel) as pdf:
            text = pdf.pages[number - 1].extract_text()
        out = {"text": text, "cmaps": sorted(CMapDB._cmap_cache), "umaps": sorted(CMapDB._umap_cache)}
        yield [rel, number], out


def cmap_cases():
    rng = random.Random(20261015)
    for name in CMAP_NAMES:
        cmap = CMapDB.get_cmap(name)
        for _ in range(40):
            n = rng.choice([1, 2, 2, 4, 6, 9])
            code = bytes(rng.choice([rng.randrange(256), rng.randrange(0x81, 0xA0), rng.randrange(0x20, 0x7F)]) for _ in range(n))
            yield ["decode", name, code], list(cmap.decode(code))
    for coll in COLLECTIONS:
        for vertical in (False, True):
            umap = CMapDB.get_unicode_map(coll, vertical)
            cids = sorted({rng.randrange(0, 24000) for _ in range(60)} | {0, 1, 7716, 8720})
            out = []
            for cid in cids:
                try:
                    out.append(umap.get_unichr(cid))
                except KeyError as e:
                    out.append(["KeyError", str(e)])
            yield ["unichr", coll, vertical, cids], out


class FakePage:
    """What TableFinder and Table.extract read from a page."""

    def __init__(self, chars, edges, bbox):
        self.chars = chars
        self.edges = edges
        self.bbox = bbox

    def extract_words(self, **kwargs):
        return utext.extract_words(self.chars, **kwargs)


def guarded(f):
    try:
        return f()
    except Exception as e:  # the exception is the result (typed JSON writes it)
        return e


def text_cases():
    root = corpus_root()
    for rel, number in PAGES:
        with pdfplumber.open(root / rel) as pdf:
            page = pdf.pages[number - 1]
            chars, edges, bbox = page.chars, page.edges, page.bbox
        ops = []
        for kw in TEXT_KWARGS:
            ops.append(["extract_text", kw, guarded(lambda: utext.extract_text(chars, **kw))])
        for kw in WORD_KWARGS:
            ops.append(["extract_words", kw, guarded(lambda: utext.extract_words(chars, **kw))])
        index = {id(c): i for i, c in enumerate(chars)}
        for args in [(1, ("fontname", "size")), (0.5, None), (3, ("fontname",))]:
            # the kept chars, as indices of `chars` (they are the input dicts)
            ops.append(["dedupe_chars", list(args), guarded(lambda: [index[id(c)] for c in utext.dedupe_chars(chars, args[0], args[1])])])
        for kw in TEXTMAP_KWARGS:
            tm = guarded(lambda: utext.chars_to_textmap(chars, **kw))
            if isinstance(tm, Exception):
                ops.append(["textmap", kw, tm])
                continue
            ops.append(["textmap", kw, tm.as_string])
            for pattern, regex, case, groups, rchars, main in SEARCHES:
                result = guarded(
                    lambda: tm.search(pattern, regex=regex, case=case, return_groups=groups, return_chars=rchars, main_group=main)
                )
                ops.append(["search", [kw, pattern, regex, case, groups, rchars, main], result])
            for strip, rchars in [(True, False), (False, len(chars) < 500)]:
                ops.append(["lines", [kw, strip, rchars], guarded(lambda: tm.extract_text_lines(strip=strip, return_chars=rchars))])
        for settings in TABLE_SETTINGS:

            def tables():
                tset = TableSettings.resolve(settings)
                fake = FakePage(chars, edges, bbox)
                finder = TableFinder(fake, tset)
                return [
                    {
                        "bbox": t.bbox,
                        "cells": t.cells,
                        "rows": [{"bbox": r.bbox, "cells": r.cells} for r in t.rows],
                        "columns": [{"bbox": c.bbox, "cells": c.cells} for c in t.columns],
                        "text": t.extract(x_tolerance=3, y_tolerance=3),
                    }
                    for t in finder.tables
                ]

            ops.append(["find_tables", settings, guarded(tables)])
        yield [rel, number, chars, edges, bbox], {"ops": ops}


def search_group_cases():
    for text, patterns, mains in [("ab cd", GROUP_PATTERNS, MAIN_GROUPS), (ASTRAL_TEXT, ASTRAL_PATTERNS, ASTRAL_GROUPS)]:
        chars = [
            {"text": c, "x0": i * 10.0, "x1": i * 10 + 8.0, "top": 0.0, "bottom": 10.0, "doctop": 0.0,
             "upright": True, "height": 10.0, "width": 8.0, "size": 10.0}
            for i, c in enumerate(text)
        ]
        tm = utext.chars_to_textmap(chars)
        assert tm.as_string == text
        # a code point offset as a UTF-16 one (what RegExp's indices give)
        utf16 = lambda i: len(tm.as_string[:i].encode("utf-16-le")) // 2  # noqa: E731
        span = lambda m, g: None if m.start(g) == -1 else [utf16(m.start(g)), utf16(m.end(g))]  # noqa: E731
        for main in mains:
            for py_pattern, js_pattern in patterns:
                compiled = re.compile(py_pattern)
                matches = [
                    [
                        [span(m, g) for g in range(compiled.groups + 1)],
                        {name: span(m, name) for name in compiled.groupindex} if compiled.groupindex else None,
                    ]
                    for m in compiled.finditer(tm.as_string)
                ]
                result = guarded(lambda: tm.search(py_pattern, return_chars=False, main_group=main))
                yield [chars, js_pattern, main, matches], result


def open_only(path, options):
    """``pdfplumber.open`` alone: ``pdf.close()`` would load the pages."""
    pdf = pdfplumber.open(path, **options)
    pdf.stream.close()


def open_cases():
    root = corpus_root()
    for options in OPEN_OPTIONS:
        yield [OPEN_PATH, options], guarded(lambda: open_only(root / OPEN_PATH, options))


def main():
    require_oracle_python()
    n_exc = write_cases(MODULE, "exceptions", exception_cases())
    n_doc = write_cases(MODULE, "documents", document_cases())
    n_cmap = write_cases(MODULE, "cmaps", cmap_cases())
    n_text = write_cases(MODULE, "text", text_cases())
    n_groups = write_cases(MODULE, "search_groups", search_group_cases())
    n_open = write_cases(MODULE, "open", open_cases())
    write_meta(
        MODULE,
        cases={"exceptions": n_exc, "documents": n_doc, "cmaps": n_cmap, "text": n_text, "search_groups": n_groups, "open": n_open},
    )
    print(f"{MODULE}: {n_exc} exception classes, {n_cmap} CMap cases, {n_text} pages")


if __name__ == "__main__":
    main()
