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
import json
import pkgutil
import random
import re
import subprocess
import sys
import tempfile
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import pdfplumber
from pdfminer.cmapdb import CMapDB
from pdfplumber.table import TableFinder, TableSettings
from pdfplumber.utils import text as utext

from .. import typed_json
from ..adversarial import interp as adv_interp
from ..adversarial import plumber as adv_plumber
from ..adversarial import structure as adv_structure
from ..canon import ExcValue
from ..corpus import corpus_root
from . import js_pages_run
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
TABLE_SETTINGS = [
    None,
    {"vertical_strategy": "text", "horizontal_strategy": "text"},
    {"snap_tolerance": 1},
    # explicit lines given as numbers span the page's bbox
    {"vertical_strategy": "explicit", "explicit_vertical_lines": [72, 144.5, 216, 400], "horizontal_strategy": "lines"},
    {"horizontal_strategy": "explicit", "explicit_horizontal_lines": [100, 200.25, 300], "vertical_strategy": "text"},
]

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


# ------------------------------------------------------------------------- pages


#: Search options and patterns of ``page_search`` (Python's pattern, the
#: JavaScript one: the same matches, as ``SEARCHES``).
PAGE_SEARCHES = [
    ("the", {}, "the"),
    ("[A-Z][a-z]+", {"return_chars": False}, "[A-Z][a-z]+"),
    ("a.b", {"regex": False}, "a.b"),
    ("THE", {"case": False, "return_groups": False}, "THE"),
    (r"([0-9]+)[.,]([0-9]+)", {"main_group": 2, "return_chars": False}, r"([0-9]+)[.,]([0-9]+)"),
    ("(?P<w>e)(x)?", {"main_group": "w", "layout": True}, "(?<w>e)(x)?"),
]


def _op(op: str, target: int | None, *args: Any, data: Any = None, js: Any = None) -> list[Any]:
    return [op, target, list(args), data, js]


class Session:
    """The cases of one session: ``objects`` counts the objects the
    operations make (handles ``#k``), as the script does."""

    def __init__(self) -> None:
        self.cases: list[list[Any]] = []
        self.count = 0

    def add(self, op: str, target: int | None, *args: Any, data: Any = None, js: Any = None, makes: int = 0) -> int:
        """Adds a case; ``makes`` objects it makes (their first index)."""
        self.cases.append(_op(op, target, *args, data=data, js=js))
        first = self.count
        self.count += makes
        return first

    def open(self, data: Any, options: dict[str, Any] | None = None, pages: int = 0) -> tuple[int, list[int]]:
        """``doc_open`` then ``doc_pages`` (``pages``: how many it makes)."""
        doc = self.add("doc_open", None, options or {}, data=data, makes=1)
        first = self.add("doc_pages", doc, makes=pages)
        return doc, list(range(first, first + pages))


def _page_count(rel: str, options: dict[str, Any] | None = None) -> int:
    with pdfplumber.open(corpus_root() / rel, **(options or {})) as pdf:
        return len(pdf.pages)


def _text_ops(s: Session, page: int, searches: bool = True) -> None:
    for kw in ({}, {"layout": True}, {"x_tolerance": 1.5, "y_tolerance": 1}):
        s.add("page_extract_text", page, kw)
    s.add("page_extract_text_simple", page, {})
    for kw in ({}, {"keep_blank_chars": True, "use_text_flow": True}, {"extra_attrs": ["fontname", "size"]}):
        s.add("page_extract_words", page, kw)
    s.add("page_extract_text_lines", page, True, False, {})
    s.add("page_extract_text_lines", page, False, True, {"layout": True})
    if searches:
        for pattern, options, js in PAGE_SEARCHES:
            s.add("search", page, pattern, options, js=js)


def _table_ops(s: Session, page: int, tables: int, found: bool) -> None:
    """Table operations; ``tables``: how many ``find_tables`` finds, ``found``:
    whether ``find_table`` (text strategies) finds one."""
    text = {"vertical_strategy": "text", "horizontal_strategy": "text"}
    first = s.add("page_find_tables", page, makes=tables)
    for t in range(first, first + tables):
        s.add("table_extract", t, {})
        s.add("table_extract", t, {"x_tolerance": 1})
    table = s.add("page_find_table", page, text, makes=1 if found else 0)
    if found:
        s.add("table_extract", table, {"layout": True})
    s.add("page_extract_tables", page)
    s.add("page_extract_table", page, text)
    s.add("page_extract_tables", page, {"vertical_strategy": "nope"})


def documents_session() -> list[list[Any]]:
    """Documents opened one after another in one process, each operation of
    the boundary on their pages and on pages derived from them."""
    s = Session()
    # a small page: every list, text, search, derived pages, tables, serializations
    doc, pages = s.open("corpus/micro/text-operators.pdf", pages=_page_count("corpus/micro/text-operators.pdf"))
    p = pages[0]
    s.add("doc_metadata", doc)
    s.add("page_info", p)
    s.add("page_objects", p)
    for name in ("chars", "rects", "lines", "curves", "images", "edges", "horizontal_edges", "vertical_edges",
                 "annots", "hyperlinks", "objects", "nope"):
        s.add("page_attr", p, name)
    _text_ops(s, p)
    crop = s.add("page_crop", p, [10, 10, 150, 150], {}, makes=1)
    s.add("page_info", crop)
    s.add("page_attr", crop, "chars")
    s.add("page_extract_text", crop, {})
    within = s.add("page_within_bbox", p, (0, 0, 100.5, 100), {"relative": True}, makes=1)
    s.add("page_extract_words", within, {})
    outside = s.add("page_outside_bbox", p, [0, 0, 50, 50], {"strict": False}, makes=1)
    s.add("page_extract_text", outside, {})
    s.add("page_crop", p, [0, 0, 0, 0], {})
    s.add("page_crop", p, [0, 0, 10, 10], {"bogus": 1})
    s.add("page_crop", p, ["a", 0, 10, 10], {})
    crop2 = s.add("page_crop", crop, [5, 5, 50, 50], {"relative": True, "strict": False}, makes=1)
    s.add("page_info", crop2)
    s.add("page_extract_text", crop2, {})
    filtered = s.add("page_filter", p, "upright", makes=1)
    s.add("page_extract_text", filtered, {})
    s.add("page_attr", filtered, "chars")
    deduped = s.add("page_dedupe_chars", p, {}, makes=1)
    s.add("page_attr", deduped, "chars")
    s.add("page_dedupe_chars", p, {"nope": 1})
    rel = "corpus/micro/text-operators.pdf"
    _table_ops(s, p, tables=_tables(rel, 0), found=_found(rel, 0))
    s.add("page_to_json", p, {"object_types": ["char"], "precision": 2})
    s.add("page_to_json", p, {"object_types": ["rect", "line"], "indent": 1})
    s.add("page_to_csv", p, {"object_types": ["char", "rect"]})
    s.add("page_to_dict", p, {"object_types": ["line", "annot"]})
    s.add("page_to_json", p, {"bogus": 1})
    s.add("page_to_dict", crop, {})
    s.add("page_close", p)
    s.add("page_attr", p, "chars")
    s.add("doc_objects", doc)
    s.add("doc_attr", doc, "edges")
    s.add("doc_attr", doc, "pages")
    s.add("doc_annots", doc)
    s.add("doc_hyperlinks", doc)
    s.add("doc_structure_tree", doc)
    s.add("doc_to_json", doc, {"object_types": ["rect"]})
    s.add("doc_to_csv", doc, {"object_types": ["line"]})
    s.add("doc_to_dict", doc, {"object_types": ["image"]})
    s.add("doc_close", doc)
    s.add("page_extract_text", p, {})
    # lines and curves: edges, tables
    rel = "corpus/micro/vector-objects.pdf"
    doc, pages = s.open(rel, pages=_page_count(rel))
    s.add("page_attr", pages[0], "edges")
    s.add("page_objects", pages[0])
    _table_ops(s, pages[0], tables=_tables(rel, 0), found=_found(rel, 0))
    # filters whose truth differs from object to object
    for predicate in ("not-rect", "small"):
        filtered = s.add("page_filter", pages[0], predicate, makes=1)
        s.add("page_objects", filtered)
    # a ruled table
    rel = "upstream/pdfplumber-python/tests/pdfs/nics-background-checks-2015-11.pdf"
    doc, pages = s.open(rel, {"pages": [1]}, pages=1)
    _table_ops(s, pages[0], tables=_tables(rel, 0), found=_found(rel, 0))
    s.add("page_extract_text", pages[0], {"layout": True})
    for pattern, options, js in PAGE_SEARCHES[:2]:
        s.add("search", pages[0], pattern, options, js=js)
    # annotations and links
    for rel in ("corpus/micro/annotations.pdf", "upstream/pdfplumber-python/tests/pdfs/annotations-rotated-90.pdf",
                "upstream/pdfplumber-python/tests/pdfs/pdffill-demo.pdf"):
        doc, pages = s.open(rel, pages=_page_count(rel))
        s.add("page_attr", pages[0], "annots")
        s.add("page_attr", pages[0], "hyperlinks")
        s.add("doc_annots", doc)
        s.add("doc_hyperlinks", doc)
        s.add("page_extract_words", pages[0], {})
    # structure trees
    for rel in ("upstream/pdfplumber-python/tests/pdfs/hello_structure.pdf",
                "upstream/pdfplumber-python/tests/pdfs/figure_structure.pdf"):
        doc, pages = s.open(rel, pages=_page_count(rel))
        s.add("doc_structure_tree", doc)
        s.add("page_structure_tree", pages[0])
        s.add("page_objects", pages[0])
    # duplicate chars
    rel = "upstream/pdfplumber-python/tests/pdfs/issue-71-duplicate-chars.pdf"
    doc, pages = s.open(rel, pages=_page_count(rel))
    deduped = s.add("page_dedupe_chars", pages[0], {"tolerance": 2, "extra_attrs": ["fontname"]}, makes=1)
    s.add("page_extract_text", deduped, {})
    _text_ops(s, pages[0], searches=False)
    # page boxes, rotation
    for rel in ("upstream/pdfplumber-python/tests/pdfs/page-boxes-example.pdf", "corpus/micro/page-boxes-rotate.pdf"):
        doc, pages = s.open(rel, pages=_page_count(rel))
        for p in pages:
            s.add("page_info", p)
    # open options
    rel = "upstream/pdfplumber-python/tests/pdfs/password-example.pdf"
    doc, pages = s.open(rel, {"password": "test"}, pages=_page_count(rel, {"password": "test"}))
    s.add("page_extract_text", pages[0], {})
    s.add("doc_open", None, {"password": "wrong"}, data=rel)
    s.add("doc_open", None, {"password": None}, data=rel)
    rel = "upstream/pdfplumber-python/tests/pdfs/issue-71-duplicate-chars.pdf"
    doc, pages = s.open(rel, {"unicode_norm": "NFKC", "raise_unicode_errors": False}, pages=_page_count(rel))
    s.add("page_extract_text", pages[0], {})
    doc, pages = s.open(rel, {"unicode_norm": "X"}, pages=_page_count(rel))
    s.add("page_extract_text", pages[0], {})
    rel = "upstream/pdfplumber-python/tests/pdfs/nics-background-checks-2015-11-rotated.pdf"
    doc, pages = s.open(rel, {"pages": [1], "strict_metadata": True}, pages=1)
    s.add("doc_metadata", doc)
    s.add("page_info", pages[0])
    s.add("doc_open", None, {"pages": "1"}, data="corpus/micro/text-operators.pdf", makes=1)
    s.add("doc_pages", s.count - 1)
    s.add("doc_open", None, {"nope": 1}, data="corpus/micro/text-operators.pdf")
    s.add("doc_open", None, {"laparams": {"bogus": 1}}, data="corpus/micro/text-operators.pdf")
    s.add("doc_open", None, {"gs_path": "gs", "repair_setting": "default"}, data="corpus/micro/text-operators.pdf",
          makes=1)
    s.add("doc_open", None, {"password": b"x"}, data="corpus/micro/text-operators.pdf", makes=1)
    s.add("doc_open", None, {}, data=b"%PDF-1.4 nothing")
    return s.cases


def _tables(rel: str, index: int) -> int:
    with pdfplumber.open(corpus_root() / rel) as pdf:
        return len(pdf.pages[index].find_tables())


def _found(rel: str, index: int) -> bool:
    with pdfplumber.open(corpus_root() / rel) as pdf:
        return pdf.pages[index].find_table({"vertical_strategy": "text", "horizontal_strategy": "text"}) is not None


def cjk_session() -> list[list[Any]]:
    """CJK documents whose fonts use predefined CMaps: the boundary loads
    them lazily, voids the calls that missed one and replays the history of
    their document (``crates/wasm/src/ops.rs``)."""
    s = Session()
    for rel, _ in DOCUMENTS:
        doc, pages = s.open(rel, pages=_page_count(rel))
        s.add("page_info", pages[0])
        s.add("page_extract_text", pages[0], {})
        s.add("page_extract_words", pages[0], {})
        crop = s.add("page_crop", pages[0], [0, 0, 300, 300], {"strict": False}, makes=1)
        s.add("page_extract_text", crop, {})
        if len(pages) > 1:
            s.add("page_extract_text", pages[-1], {})
    return s.cases


def run_session(cases: list[list[Any]]) -> dict[str, Any]:
    """The results of the cases in a fresh process at the top level of
    ``js_pages_run.py`` (paths made absolute)."""
    root = corpus_root()
    runnable = [[op, target, args, str(root / data) if isinstance(data, str) else data, js]
                for op, target, args, data, js in cases]
    with tempfile.TemporaryDirectory() as d:
        inp, out = Path(d) / "in.json", Path(d) / "out.json"
        inp.write_text(json.dumps(typed_json.encode(runnable)))
        subprocess.run([sys.executable, "-I", js_pages_run.__file__, str(inp), str(out)], check=True, timeout=1200)
        raw = json.loads(out.read_text())
    return {"depth": raw["depth"], "results": [[typed_json.decode(r), typed_json.decode(x)] for r, x in raw["results"]]}


def _recursion(results: dict[str, Any], last: bool = False) -> bool:
    """Whether an operation raised ``RecursionError`` (``last``: the last
    one)."""
    rs = results["results"][-1:] if last else results["results"]
    return any(isinstance(r, ExcValue) and "recursion" in r.message for r, _ in rs)


#: Explicit table lines: ``find_tables`` reads no object of the page, so
#: ``Table.extract`` computes them (``self.page.chars``).
EXPLICIT = {"vertical_strategy": "explicit", "horizontal_strategy": "explicit",
            "explicit_vertical_lines": [1, 50, 100], "explicit_horizontal_lines": [1, 50, 100]}


def depth_session(make: Any, ops: list[tuple[str, tuple[Any, ...]]], n: int) -> list[list[Any]]:
    """A document of nesting ``n`` and the operations, on its page (``doc:``
    before a name: on the document; ``table:``: on the first table the
    previous operation found)."""
    s = Session()
    doc, pages = s.open(make(n), pages=1)
    for op, args in ops:
        target = pages[0]
        if op.startswith("doc:"):
            op, target = op[4:], doc
        elif op.startswith("table:") or op.startswith("crop:"):
            op, target = op.split(":", 1)[1], s.count - 1
        makes = 1 if op in ("page_find_tables", "page_find_table", "page_crop") else 0
        s.add(op, target, *args, js=args[0] if op == "search" else None, makes=makes)
    return s.cases


def depth_windows() -> Iterator[tuple[str, list[list[Any]]]]:
    """Sessions around the first nesting at which an operation of the
    boundary raises ``RecursionError`` (bisected on the script: the
    boundary's entry depth is not a stage's)."""



    figures = adv_plumber.depth_kinds()["figures"][0]
    dash = adv_plumber.depth_kinds()["dash-list"][0]
    climit = adv_interp.climit_kinds()
    kinds = [
        ("figures-objects", figures, [("page_objects", ())], 2, 1200),
        ("figures-text", figures, [("page_extract_text", ({},))], 2, 1200),
        ("figures-chars", figures, [("page_attr", ("chars",))], 2, 1200),
        ("dash-json", dash, [("page_to_json", ({"object_types": ["rect"]},))], 2, 1200),
        ("climit-tf-list-objects", climit["tf-list"], [("page_objects", ())], 9000, 10100),
        ("climit-tf-list-chars", climit["tf-list"], [("page_attr", ("chars",))], 9000, 10100),
        ("climit-tf-list-text", climit["tf-list"], [("page_extract_text", ({},))], 9000, 10100),
        ("climit-do-list-search", climit["do-list"], [("search", ("a", {}))], 9000, 10100),
        # every way into the interpretation from the boundary
        ("climit-tf-list-words", climit["tf-list"], [("page_extract_words", ({},))], 9000, 10100),
        ("climit-tf-list-simple", climit["tf-list"], [("page_extract_text_simple", ({},))], 9000, 10100),
        ("climit-tf-list-lines", climit["tf-list"], [("page_extract_text_lines", (True, False, {}))], 9000, 10100),
        ("climit-tf-list-edges", climit["tf-list"], [("page_attr", ("edges",))], 9000, 10100),
        ("climit-tf-list-hedges", climit["tf-list"], [("page_attr", ("horizontal_edges",))], 9000, 10100),
        ("climit-tf-list-rect-edges", climit["tf-list"], [("page_attr", ("rect_edges",))], 9000, 10100),
        ("climit-tf-list-curve-edges", climit["tf-list"], [("page_attr", ("curve_edges",))], 9000, 10100),
        ("climit-tf-list-tables", climit["tf-list"], [("page_find_tables", ())], 9000, 10100),
        ("climit-tf-list-strict", climit["tf-list"], [("page_extract_tables", ({"vertical_strategy": "lines_strict", "horizontal_strategy": "lines_strict"},))], 9000, 10100),
        ("climit-tf-list-table-extract", climit["tf-list"], [("page_find_tables", (EXPLICIT,)), ("table:table_extract", ({},))], 9000, 10100),
        ("climit-tf-list-dedupe", climit["tf-list"], [("page_dedupe_chars", ({},))], 9000, 10100),
        ("climit-tf-list-csv", climit["tf-list"], [("page_to_csv", ({},))], 9000, 10100),
        ("climit-tf-list-dict", climit["tf-list"], [("page_to_dict", ({"object_types": ["line", "char"]},))], 9000, 10100),
        ("climit-tf-list-doc-objects", climit["tf-list"], [("doc:doc_objects", ())], 9000, 10100),
        ("climit-tf-list-doc-csv", climit["tf-list"], [("doc:doc_to_csv", ({},))], 9000, 10100),
        ("climit-tf-list-doc-dict", climit["tf-list"], [("doc:doc_to_dict", ({"object_types": ["char"]},))], 9000, 10100),
        ("climit-tf-list-doc-json", climit["tf-list"], [("doc:doc_to_json", ({"object_types": ["char"]},))], 9000, 10100),
        ("climit-tf-list-doc-edges", climit["tf-list"], [("doc:doc_attr", ("edges",))], 9000, 10100),
        ("climit-tf-list-json", climit["tf-list"], [("page_to_json", ({"object_types": ["char"]},))], 9000, 10100),
        ("climit-tf-list-find-table", climit["tf-list"], [("page_find_table", ())], 9000, 10100),
        ("climit-tf-list-extract-table", climit["tf-list"], [("page_extract_table", ())], 9000, 10100),
        ("climit-tf-list-vedges", climit["tf-list"], [("page_attr", ("vertical_edges",))], 9000, 10100),
        *[(f"climit-tf-list-{name}", climit["tf-list"], [("page_attr", (name,))], 9000, 10100)
          for name in ("images", "textboxverticals", "textboxhorizontals", "textlineverticals", "textlinehorizontals")],
        # the structure trees (adv-structure's windows from this depth)
        ("struct-chain-doc", adv_structure.WINDOWS["chain-to-dict"][0], [("doc:doc_structure_tree", ())], 2, 1200),
        ("struct-repr-doc", adv_structure.WINDOWS["k-nested-list-repr"][0], [("doc:doc_structure_tree", ())], 9000, 10100),
        ("struct-str-doc", adv_structure.WINDOWS["actualtext-str-first"][0], [("doc:doc_structure_tree", ())], 9000, 10100),
        ("struct-str-page", adv_structure.WINDOWS["actualtext-str-first"][0], [("page_structure_tree", ())], 9000, 10100),
        # a site run for one class, specialized for it, then for another
        # (the first runs raised: nothing was cached); bisected on the last
        ("class-text", climit["tf-list"], [("page_extract_text", ({},)), ("page_extract_text", ({},)),
                                           ("page_crop", ([0, 0, 200, 200], {})), ("crop:page_extract_text", ({},))],
         9000, 10100),
        ("class-table", climit["tf-list"], [("page_find_tables", (EXPLICIT,)), ("table:table_extract", ({},)),
                                            ("table:table_extract", ({},)), ("page_crop", ([0, 0, 200, 200], {})),
                                            ("crop:page_find_tables", (EXPLICIT,)), ("table:table_extract", ({},))],
         9000, 10100),
        ("class-doc", climit["tf-list"], [("page_attr", ("chars",)), ("page_attr", ("chars",)),
                                          ("doc:doc_attr", ("chars",))], 9000, 10100),
    ]
    for label, make, ops, lo, hi in kinds:
        last = label.startswith("class-")
        fails = lambda n: _recursion(run_session(depth_session(make, ops, n)), last)  # noqa: E731
        assert not fails(lo) and fails(hi), (label, lo, hi)
        while hi - lo > 1:
            mid = (lo + hi) // 2
            if fails(mid):
                hi = mid
            else:
                lo = mid
        print(f"pages depth-{label}: first failing {hi}", file=sys.stderr)
        for n in range(hi - 2, hi + 2):
            yield f"depth-{label}-{n}", depth_session(make, ops, n)


def page_cases() -> Iterator[tuple[list[Any], Any]]:
    """``pages.jsonl``: ``{"in": [name, cases], "out": {"depth": ...,
    "results": [[result, extra], ...]}}``, a session each (see
    ``js_pages_run.py``)."""
    sessions = [("documents", documents_session()), ("cjk", cjk_session())]
    sessions += list(depth_windows())
    for name, cases in sessions:
        out = run_session(cases)
        yield [name, cases], out


def main():
    require_oracle_python()
    n_exc = write_cases(MODULE, "exceptions", exception_cases())
    n_doc = write_cases(MODULE, "documents", document_cases())
    n_cmap = write_cases(MODULE, "cmaps", cmap_cases())
    n_text = write_cases(MODULE, "text", text_cases())
    n_groups = write_cases(MODULE, "search_groups", search_group_cases())
    n_open = write_cases(MODULE, "open", open_cases())
    n_pages = write_cases(MODULE, "pages", page_cases())
    write_meta(
        MODULE,
        cases={"exceptions": n_exc, "documents": n_doc, "cmaps": n_cmap, "text": n_text, "search_groups": n_groups,
               "open": n_open, "pages": n_pages},
    )
    print(f"{MODULE}: {n_exc} exception classes, {n_cmap} CMap cases, {n_text} pages")


if __name__ == "__main__":
    main()
