"""Depth sweeps of pdfplumber's page layer, replayed by
``crates/pdftrace/tests/plumber_sweep.rs``.

The ``plumber_*`` stages run every page method a few frames below the
stage body, far from the recursion limit, and the depth windows of
``adv-plumber`` (``oracle/unit/plumber.py``) only reach it inside the
interpretation, ``resolve_all`` and the ``Serializer``: a frame of
``Page.extract_text``, ``iter_layout_objects``, ``Container.to_json`` or
``PDF.pages`` miscounted would show in neither. Here each operation
(``OPS``) runs on a document opened as the stages open it, from every depth
that moves the recursion limit through its frames: from the room of its
deepest frame (measured with a bounded profile hook, ``deepest``) down to
none, one frame at a time. Where a ``RecursionError`` happens is the first
moment, in time, the operation goes deeper than the limit, so each frame on
the way to each depth the operation reaches is the first one past the
limit in one of the traces.

A case is an operation on a document after a preparation (``pre``):
``"none"`` (the first page, fresh: the operation's first property access
interprets it, as a call through the JavaScript boundary does), ``"edges"``
(``page.objects`` and ``page.edges`` computed first: the cached edges),
``"pdf"`` (the document only). On a fresh page the deepest frames are the
interpretation's and the frames above it are the operation's: the sweep
checks the frames from the operation down to the interpretation, where a
``RecursionError`` can happen through the API. The code that runs after
(``utils/text.py``, ``table.py``: no frame counted, ``docs/architecture.md``)
stays shallower than what the interpretation reached before it. The preparation runs in the stage body;
then ``down(n, lambda: deep_apply(...))`` puts the lambda's frame at
``LIMIT - room``; ``deep_apply`` catches what the operation raises, and
what it returns is summarized (``summarize``) back in the stage body, where
``canon`` and the summary's own code run far from the limit.

Cases (``sweeps.jsonl``): ``{"in": [name, pdf, op, pre, options, here,
rooms, spans], "out": [first, digests, ids]}``: ``options`` the keyword
arguments of ``pdfplumber.open`` (``laparams``), ``here`` the depth of the
stage body (``depth_here``), ``rooms`` the rooms swept (the first one
holds the deepest frame: no ``RecursionError``), ``spans`` the matches of
Python's ``re`` for a ``search`` op (computed in another process, as
``plumber_text`` writes them, ``None`` for other ops), the first trace in
full and the SHA-256 of each, and the ``ids`` line of each: a document with
``laparams`` writes the ``id()`` ranking of its analysis (``plumber_objects``,
after the operation), which the replay hands to its analysis (the ranking
of an analysis run again after a ``RecursionError`` is that trace's). Every trace runs in a process forked for
it (``document.in_child``), from the state ``py_process.jsonl`` records.

Usage: PDFPLUMBER_ROOT=<main checkout> <root>/.venv/bin/python -m oracle.unit.plumber_sweep
"""

from __future__ import annotations

import hashlib
import sys
from collections.abc import Iterator
from typing import Any

from ..canon import canon
from ..codegen import cmaps as codegen_cmaps
from ..corpus import cache_dir, load_set
from ..depth import depth_here
from ..stages import _in_stage_thread
from ..stages._common import terminated
from ..stages.plumber_objects import content_ids, exc, open_pdf
from ..stages.plumber_tables import settings_of
from ..stages.plumber_text import SEARCH_ARGS, derive, run_op
from ..stages.plumber_text import search_spans as _search_spans
from ._common import require_oracle_python, write_cases, write_meta
from .document import in_child, process_state

MODULE = "plumber_sweep"
MAX_PAGES = 20
#: Python's recursion limit (``sys.getrecursionlimit()``, never changed).
LIMIT = 1000
#: Files of ``adv-plumber`` (by the name after the number): text, a ruled
#: table, figures, duplicated chars, annotations and links.
FILES = ("table-grid-lines.pdf", "layout-figures.pdf", "text-dedupe.pdf", "annot-malformed.pdf", "font-kinds.pdf")

_GRID = {"vertical_strategy": "lines", "horizontal_strategy": "lines"}
_TEXT = {"vertical_strategy": "text", "horizontal_strategy": "text"}
_EXPLICIT = {"v": [0.1, 0.5, 0.9], "h": [0.1, 0.5, 0.9], "v_objects": 1, "h_objects": 1,
             "vertical_strategy": "explicit", "horizontal_strategy": "explicit"}

#: ``(op, pre, options)``.
OPS: list[tuple[list[Any], str, dict[str, Any]]] = [
    (["objects"], "none", {}),
    (["layout"], "none", {}),
    (["objects"], "none", {"laparams": {}}),
    (["objects"], "layout", {}),
    (["objects"], "layout", {"laparams": {}}),
    (["objects"], "layout", {"laparams": {"all_texts": True, "detect_vertical": True}}),
    (["objects"], "none", {"laparams": {"all_texts": True, "detect_vertical": True}}),
    (["text", ["extract_words", {}]], "none", {"laparams": {}}),
    (["text", ["extract_text", {}]], "none", {}),
    (["text", ["extract_text", {"layout": True}]], "none", {}),
    (["text", ["extract_text", {"use_text_flow": True, "keep_blank_chars": True}]], "none", {}),
    (["text", ["extract_text_simple", {}]], "none", {}),
    (["text", ["extract_words", {"extra_attrs": ["size"], "split_at_punctuation": True}]], "none", {}),
    (["text", ["extract_words", {"return_chars": True, "expand_ligatures": False}]], "none", {}),
    (["text", ["extract_text_lines", {}]], "none", {}),
    (["text", ["extract_text_lines", {"strip": False, "return_chars": False, "layout": True}]], "none", {}),
    (["text", ["search", "e", {}]], "none", {}),
    (["text", ["search", "(a)(b)?", {"return_chars": False, "layout": True}]], "none", {}),
    (["text", ["search", "X", {"regex": False, "case": False}]], "none", {}),
    (["text", ["dedupe_chars", {}]], "none", {}),
    (["text", ["dedupe_chars", {"extra_attrs": ["fontname"], "tolerance": 2}]], "none", {}),
    (["derived", [["crop", [0.1, 0.1, 0.9, 0.9], {}]], ["extract_text", {}]], "none", {}),
    (["derived", [["within_bbox", [0.0, 0.0, 0.6, 0.6], {}]], ["extract_words", {}]], "none", {}),
    (["derived", [["outside_bbox", [0.0, 0.0, 0.5, 0.5], {}]], ["extract_text", {}]], "none", {}),
    (["derived", [["filter", "upright"]], ["extract_text", {}]], "none", {}),
    (["derived", [["crop", [0.0, 0.0, 0.8, 0.8], {}], ["crop", [0.1, 0.1, 0.5, 0.5], {"relative": True}],
                  ["dedupe_chars", {}]], ["extract_text_simple", {}]], "none", {}),
    (["derive", [["crop", [0.2, 0.2, 0.8, 0.8], {}]]], "none", {}),
    (["derive", [["within_bbox", [0.2, 0.2, 0.8, 0.8], {}], ["filter", "not-rect"]]], "none", {}),
    (["derive", [["crop", [0.0, 0.0, 2.0, 2.0], {}]]], "none", {}),
    (["derive", [["crop", [0.5, 0.5, 0.5, 0.9], {}]]], "none", {}),
    (["edges", "edges"], "none", {}),
    (["edges", "horizontal_edges"], "none", {}),
    (["edges", "vertical_edges"], "none", {}),
    (["edges_identity"], "none", {}),
    (["find_tables", ["settings", _GRID]], "none", {}),
    (["find_tables", ["settings", _TEXT]], "none", {}),
    (["find_tables", ["explicit", _EXPLICIT]], "none", {}),
    (["extract_tables", ["settings", {}]], "none", {}),
    (["extract_table", ["settings", _TEXT]], "none", {}),
    (["debug_tablefinder", ["settings", {}]], "none", {}),
    (["to_json", {}], "none", {}),
    (["to_json", {"object_types": ["char", "rect"], "precision": 2, "indent": 1}], "none", {}),
    (["to_json", {"object_types": ["cached_propertie", "hyperlink", "page", "annot", "_object", "object"]}], "edges", {}),
    (["to_json", {"object_types": ["rect_edge", "curve_edge", "edge", "horizontal_edge", "vertical_edge"]}], "none", {}),
    (["to_json", {"indent": None, "object_types": ["line"]}], "none", {}),
    (["to_csv", {"object_types": ["textboxhorizontal", "textlinehorizontal", "textboxvertical", "textlinevertical"]}],
     "none", {"laparams": {"detect_vertical": True}}),
    (["to_json", {"object_types": ["_rect_edge", "_edge", "_curve_edge"]}], "edges", {}),
    (["to_json", {"object_types": ["_rect_edge"]}], "none", {}),
    (["to_csv", {}], "none", {}),
    (["to_csv", {"object_types": ["line", "char"], "precision": 1}], "none", {}),
    (["annots"], "none", {}),
    (["hyperlinks"], "none", {}),
    (["pdf_pages"], "pdf", {}),
    (["pdf_objects"], "pdf", {}),
    (["pdf_annots"], "pdf", {}),
    (["pdf_hyperlinks"], "pdf", {}),
    (["pdf_to_csv", {"object_types": ["annot", "hyperlink"]}], "pdf", {}),
    (["pdf_to_csv", {"object_types": ["page"]}], "pdf", {}),
    (["pdf_edges_identity"], "pdf", {}),
    (["open", {}], "pdf", {}),
    (["open", {"laparams": {"char_margin": 2}, "unicode_norm": "NFC"}], "pdf", {}),
]


def down(n: int, f):
    """``f()`` called through ``n + 1`` more frames (``down`` itself)."""
    return f() if n <= 0 else down(n - 1, f)


def deep_apply(path: str, pdf: Any, page: Any, op: list[Any]) -> tuple[str, Any]:
    """The operation, its value or the exception it raised."""
    name = op[0]
    try:
        if name == "text":
            return "ok", run_op(page, op[1])
        if name == "derived":
            return "ok", run_op(derive(page, op[1]), op[2])
        if name == "derive":
            return "ok", derive(page, op[1]).objects
        if name == "objects":
            return "ok", page.objects
        if name == "layout":
            return "ok", page.layout
        if name == "edges":
            if op[1] == "edges":
                return "ok", page.edges
            if op[1] == "horizontal_edges":
                return "ok", page.horizontal_edges
            return "ok", page.vertical_edges
        if name == "edges_identity":
            a = page.edges
            b = page.edges
            r1 = page.rect_edges
            c1 = page.curve_edges
            o1 = page.objects
            l1 = page.layout
            page.flush_cache()
            c = page.edges
            r2 = page.rect_edges
            c2 = page.curve_edges
            o2 = page.objects
            l2 = page.layout
            return "ok", [a is b, a is c, len(c), r1 is r2, c1 is c2, o1 is o2, l1 is l2]
        if name == "pdf_edges_identity":
            a = pdf.edges
            b = pdf.edges
            o1 = pdf.objects
            p1 = pdf.pages
            pdf.flush_cache()
            c = pdf.edges
            o2 = pdf.objects
            p2 = pdf.pages
            return "ok", [a is b, a is c, len(c), o1 is o2, p1 is p2]
        if name == "pdf_to_csv":
            return "ok", pdf.to_csv(**op[1])
        if name == "find_tables":
            return "ok", [(t.bbox, t.extract()) for t in page.find_tables(settings_of(page, op[1]))]
        if name == "extract_tables":
            return "ok", page.extract_tables(settings_of(page, op[1]))
        if name == "extract_table":
            return "ok", page.extract_table(settings_of(page, op[1]))
        if name == "debug_tablefinder":
            return "ok", page.debug_tablefinder(settings_of(page, op[1]))
        if name == "to_json":
            return "ok", page.to_json(**op[1])
        if name == "to_csv":
            return "ok", page.to_csv(**op[1])
        if name == "annots":
            return "ok", page.annots
        if name == "hyperlinks":
            return "ok", page.hyperlinks
        if name == "pdf_pages":
            return "ok", pdf.pages
        if name == "pdf_objects":
            return "ok", pdf.objects
        if name == "pdf_annots":
            return "ok", pdf.annots
        if name == "pdf_hyperlinks":
            return "ok", pdf.hyperlinks
        if name == "open":
            return "ok", open_pdf(path, None, MAX_PAGES, **op[1])
        raise AssertionError(op)
    except Exception as e:
        return "raised", e


def _annot(a: dict[str, Any]) -> list[Any]:
    return [a["page_number"], a["x0"], a["top"], a["x1"], a["bottom"]]


def summarize(op: list[Any], value: Any) -> Any:
    """What a trace writes of an operation's value (in the stage body)."""
    name = op[0]
    if name in ("text", "derived", "extract_tables", "extract_table", "to_json", "to_csv", "edges_identity",
                "pdf_edges_identity", "pdf_to_csv"):
        return value
    if name in ("objects", "derive", "pdf_objects"):
        return [[k, len(v)] for k, v in value.items()]
    if name == "layout":
        return len(value)
    if name == "edges":
        return [len(value)] + [[e["x0"], e["top"], e["x1"], e["bottom"], e["orientation"]] for e in value[:4]]
    if name == "find_tables":
        return [[list(bbox), rows] for bbox, rows in value]
    if name == "debug_tablefinder":
        return [len(value.edges), len(value.intersections), len(value.cells), len(value.tables)]
    if name in ("annots", "hyperlinks", "pdf_annots", "pdf_hyperlinks"):
        return [_annot(a) for a in value]
    if name == "pdf_pages":
        return [[p.page_number, p.initial_doctop] for p in value]
    if name == "open":
        return len(value.pages)
    raise AssertionError(op)


def sweep_trace(path: str, op: list[Any], pre: str, options: dict[str, Any], room: int | None) -> dict[str, Any]:
    """The trace of ``op`` with the lambda's frame at ``LIMIT - room``
    (``"lines"``), and the depth of the stage body (``"here"``); for
    ``room`` ``None``, run without a descent under a profile hook, the
    room of its deepest frame (``"deepest"``)."""
    out: dict[str, Any] = {}

    @terminated
    def body(path_: str, password: str | None, max_pages: int) -> Iterator[str]:
        pdf = open_pdf(path, None, MAX_PAGES, **options)
        page = None
        if pre != "pdf":
            page = pdf.pages[0]
            if pre == "layout":
                page.layout
            elif pre == "objects":
                page.objects
            elif pre == "edges":
                page.objects
                page.edges
        here = depth_here()
        out["here"] = here
        if room is None:
            state = {"depth": 0, "max": 0}

            def hook(frame, event, arg) -> None:
                if event == "call":
                    state["depth"] += 1
                    state["max"] = max(state["max"], state["depth"])
                elif event == "return":
                    state["depth"] -= 1

            sys.setprofile(hook)
            try:
                result = (lambda: deep_apply(path, pdf, page, op))()
            finally:
                sys.setprofile(None)
            # the lambda is the first frame: its room is max - 1
            out["deepest"] = state["max"] - 1
        else:
            n = LIMIT - here["py"] - room - 2
            try:
                result = down(n, lambda: deep_apply(path, pdf, page, op))
            except RecursionError as e:
                result = ("raised", e)
        kind, value = result
        if kind == "raised":
            yield "raised " + exc(value)
        else:
            yield "ok " + canon(summarize(op, value))
        if page is not None and pdf.laparams is not None:
            # the analysis's id() ranking (computed again here if the
            # operation raised before)
            yield f"ids {page.page_number} " + canon(content_ids(page.layout))

    out["lines"] = _in_stage_thread(lambda: body(path, None, 0))
    return out


def spans_of(path: str, op: list[Any], options: dict[str, Any]) -> Any:
    """The matches of Python's ``re`` for a ``search`` op (``plumber_text``'s
    ``spans`` line), ``None`` for another op."""
    if op[0] != "text" or op[1][0] != "search":
        return None
    pdf = open_pdf(path, None, MAX_PAGES, **options)
    return canon(_search_spans(pdf.pages[0], op[1][1], op[1][2]))


def sweep_cases() -> Iterator[tuple[list, Any]]:
    files = {f.path.name[4:]: f for f in load_set("adv-plumber")}
    for fname in FILES:
        f = files[fname]
        path, data = str(f.path), f.path.read_bytes()
        for op, pre, options in OPS:
            measured = in_child(lambda: sweep_trace(path, op, pre, options, None))
            deepest, here = measured["deepest"], measured["here"]
            spans = in_child(lambda: spans_of(path, op, options))
            rooms = list(range(deepest, -1, -1))
            first, digests, ids = None, [], []
            for room in rooms:
                traced = in_child(lambda: sweep_trace(path, op, pre, options, room))
                if traced["here"] != here:
                    raise RuntimeError(f"{fname} {op}: the stage body moved ({traced['here']} != {here})")
                text = "".join(x + "\n" for x in traced["lines"])
                if first is None:
                    first = text
                    if "RecursionError" in text:
                        raise RuntimeError(f"{fname} {op}: RecursionError at the deepest room:\n{text[:2000]}")
                digests.append(hashlib.sha256(text.encode("ascii")).hexdigest())
                ids.append("".join(x + "\n" for x in traced["lines"] if x.startswith("ids ")))
            yield [fname, data, op, pre, options, here, rooms, spans], [first, digests, ids]


def main() -> None:
    require_oracle_python()
    cmap_dir = codegen_cmaps.ensure()
    rel_cmap_dir = cmap_dir.relative_to(cache_dir()).as_posix()
    assert SEARCH_ARGS
    state = process_state()
    write_cases(MODULE, "py_process", [([], state)])
    n = write_cases(MODULE, "sweeps", sweep_cases())
    if process_state() != state:
        raise RuntimeError("the process state changed while the traces ran in children")
    print(f"{MODULE}: {n} sweeps", file=sys.stderr)
    write_meta(MODULE, cmap_dir=rel_cmap_dir)


if __name__ == "__main__":
    main()
