"""Page operations shared by the text/table unit generators
(``text_tables.py``: adversarial pages, ``text_tables_real.py``: real PDFs).

A page case is ``{"in": [record, ops], "out": [canon...]}``: ``record`` is
``{"chars": page.chars, "edges": page.edges, "bbox": page.bbox}``; each op
runs through ``run_op`` (pdfplumber's own functions and ``Page`` methods),
and its output is the ``canon`` of its result or exception, or
``"sha256:<hex>:<len>"`` of it when longer than ``FULL_CANON_LIMIT``. A
``search`` op is replayed by Rust with the spans of its matches (computed
here, ``search_spans``) instead of its pattern. Synthetic pages run on
``FakePage``, a ``Page`` whose chars, edges and bbox are given.

``ValueError`` messages of ``validate_directions`` print a ``set`` of
strings, whose order changes with ``PYTHONHASHSEED``: they are normalized to
the order of ``POSITION_KEYS`` (``VALID_DIRS_REPR``), which the port prints.
"""

from __future__ import annotations

import hashlib
import re
from functools import lru_cache

from pdfplumber import utils
from pdfplumber.page import Page, tuplify_list_kwargs
from pdfplumber.table import TableFinder, TableSettings
from pdfplumber.utils import geometry
from pdfplumber.utils import text as ptext

from ..canon import ExcValue, canon, exception_name
from ._nan_objects import SetOrderCheck

FULL_CANON_LIMIT = 4096

#: str(valid_dirs) of validate_directions, as the port prints it
VALID_DIRS_REPR = "{'ttb', 'btt', 'ltr', 'rtl'}"
_PROCESS_DIRS_REPR = str(set(ptext.POSITION_KEYS.keys()))


def exc_value(e: BaseException) -> ExcValue:
    message = str(e).replace(_PROCESS_DIRS_REPR, VALID_DIRS_REPR)
    return ExcValue(exception_name(e), message)


def guarded(fn, *args, **kwargs):
    try:
        return fn(*args, **kwargs)
    except Exception as e:
        return exc_value(e)


def canon_out(value) -> str:
    text = canon(value)
    if len(text) > FULL_CANON_LIMIT:
        return f"sha256:{hashlib.sha256(text.encode('ascii')).hexdigest()}:{len(text)}"
    return text




class FakePage(Page):
    """A Page whose chars, edges and bbox are given (synthetic pages)."""

    def __init__(self, chars, edges, bbox):  # noqa: D107 (no super().__init__: no PDF)
        self.pdf = None
        self.root_page = self
        self.page_number = 1
        self.initial_doctop = 0
        self.rotation = 0
        self.bbox = bbox
        self.mediabox = self.cropbox = bbox
        self._objects = {"char": chars}
        self._edges = edges
        self.get_textmap = lru_cache()(self._get_textmap)


def describe_finder(finder: TableFinder):
    bbox = geometry.obj_to_bbox
    return {
        "edges": finder.edges,
        "intersections": [(pt, [bbox(e) for e in d["v"]], [bbox(e) for e in d["h"]]) for pt, d in finder.intersections.items()],
        "cells": finder.cells,
        "tables": [
            {
                "bbox": t.bbox,
                "cells": t.cells,
                "rows": [(r.cells, r.bbox) for r in t.rows],
                "columns": [(c.cells, c.bbox) for c in t.columns],
            }
            for t in finder.tables
        ],
    }


def search_spans(page, op):
    """The matches of a search op, as spans (None if the textmap raises)."""
    _, kwargs, pattern, regex, case = op[:5]
    try:
        tm = page.get_textmap(**tuplify_list_kwargs(kwargs))
    except Exception:
        return None
    compiled = re.compile(pattern if regex else re.escape(pattern), re.I if case is False else 0)
    return [
        [(m.start(g), m.end(g)) if m.start(g) >= 0 else None for g in range(compiled.groups + 1)]
        for m in re.finditer(compiled, tm.as_string)
    ]


def run_op(page, op):
    name = op[0]
    if name == "text":
        return page.extract_text(**op[1])
    if name == "utext":
        return utils.extract_text(page.chars, **op[1])
    if name == "simple":
        return utils.extract_text_simple(page.chars, **op[1])
    if name == "words":
        return utils.extract_words(page.chars, **op[1])
    if name == "lines":
        return page.extract_text_lines(strip=op[1], return_chars=op[2], **op[3])
    if name == "search":
        _, kwargs, pattern, regex, case, main_group, return_groups, return_chars = op
        return page.search(
            pattern,
            regex=regex,
            case=case,
            main_group=main_group,
            return_chars=return_chars,
            return_groups=return_groups,
            **kwargs,
        )
    if name == "dedupe":
        return utils.dedupe_chars(page.chars, **op[1])
    if name == "tables":
        return describe_finder(TableFinder(page, TableSettings.resolve(op[1])))
    if name == "extract_tables":
        return page.extract_tables(op[1])
    if name == "extract_table":
        return page.extract_table(op[1])
    if name == "wordmap":
        # WordMap.to_textmap called directly (presorted may be false)
        wordmap = ptext.WordExtractor(**op[1]).extract_wordmap(page.chars)
        tm = wordmap.to_textmap(**op[2])
        return (tm.as_string, [(t, c) for t, c in tm.tuples])
    raise ValueError(f"unknown op {name}")


def rust_op(page, op):
    """The op as Rust replays it: a search carries its spans instead of the
    pattern."""
    if op[0] == "search":
        _, kwargs, pattern, regex, case, main_group, return_groups, return_chars = op
        return ["search", kwargs, main_group, return_groups, return_chars, search_spans(page, op)]
    return list(op)


def page_case(page, record, ops, nan_sets=None):
    """(args, out) of one page: the record, the ops as Rust replays them,
    and the canon of each result.

    ``nan_sets`` (a dict of counts) is for pages whose NaN coordinates come
    from ``NanPool``: each op runs under ``SetOrderCheck``, with an empty
    textmap cache, and is left out when a set it builds does not list its
    NaNs as the port does (``_nan_objects.py``)."""
    outs = []
    rops = []
    for op in ops:
        if nan_sets is None:
            rops.append(rust_op(page, op))
            outs.append(canon_out(guarded(run_op, page, op)))
            continue
        page.get_textmap.cache_clear()
        with SetOrderCheck() as check:
            rop = rust_op(page, op)
            out = canon_out(guarded(run_op, page, op))
        key = "kept" if check.ok else "left out"
        nan_sets[key] = nan_sets.get(key, 0) + 1
        nan_sets["sets with NaNs"] = nan_sets.get("sets with NaNs", 0) + check.nan_sets
        if check.ok:
            rops.append(rop)
            outs.append(out)
    return [record, rops], outs


def has_surrogate(objs) -> bool:
    for o in objs:
        for v in o.values():
            if isinstance(v, str) and any(0xD800 <= ord(c) <= 0xDFFF for c in v):
                return True
    return False


