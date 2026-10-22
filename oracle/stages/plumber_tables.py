"""Stage ``plumber_tables``: pdfplumber's table finder on real pages, for
every strategy (``lines``, ``lines_strict``, ``text``, ``explicit``) and
setting variants, on the page and on cropped pages.

Exercises pdfplumber/table.py (``TableSettings`` and ``resolve``,
``TableFinder``: ``get_edges``, ``edges_to_intersections``,
``intersections_to_cells``, ``cells_to_tables``; ``Table``: ``bbox``,
``rows``, ``columns``, ``extract``; ``snap_edges``, ``join_edge_group``,
``merge_edges``, ``words_to_edges_h``/``_v``), pdfplumber/page.py
(``debug_tablefinder``, ``find_tables``, ``find_table``, ``extract_tables``,
``extract_table``), pdfplumber/container.py (``edges``, ``rect_edges``,
``curve_edges``, ``horizontal_edges``, ``vertical_edges``),
pdfplumber/utils/geometry.py (``rect_to_edges``, ``curve_to_edges``,
``line_to_edge``, ``obj_to_edges``, ``filter_edges``, ``snap_objects``...)
and pdfplumber/utils/text.py (words of the ``text`` strategy, cell text).

Traversal (``open_pdf``, ``exc``, ``select`` of ``plumber_objects``;
``derive``, ``out_line`` of ``plumber_text``)::

    pdf = open_pdf(path, password, max_pages)
    for index, page in enumerate(pdf.pages):
        page.edges, page.horizontal_edges, page.vertical_edges
        for op in BASE_OPS + select(sha, index, VARIANT_OPS, VARIANTS_PER_PAGE):
            settings = the table settings of op
            finder = page.debug_tablefinder(settings): edges, intersections,
                cells, tables (bbox, cells, rows, columns, extract(**text_settings))
            page.find_table(settings).bbox, page.extract_tables(settings),
            page.extract_table(settings)
        for op in select(sha, index, DERIVED_OPS, 1):
            derived = derive(page, op[0]); the same lines for op[1] on it

An op is ``[name, settings]``. ``name`` ``"settings"``: ``settings`` as
given (a dict). ``name`` ``"explicit"``: ``settings`` plus the explicit
lines built from the page: for ``(x0, top, x1, bottom) = page.bbox``,
``explicit_vertical_lines`` is ``[x0 + (x1 - x0) * f for f in v]`` followed
by the first ``v_objects`` objects of ``objs``, and
``explicit_horizontal_lines`` ``[top + (bottom - top) * f for f in h]``
followed by the first ``h_objects`` of ``objs``, where ``objs =
page.objects.get("rect", []) + page.objects.get("line", []) +
page.objects.get("curve", [])`` (the dicts themselves: pdfplumber turns
them into edges); ``v``, ``h``, ``v_objects``, ``h_objects`` are removed
from the settings. A ``DERIVED_OPS`` entry is ``[steps, op]``: ``steps``
as in ``plumber_text``.

Values longer than ``OUT_LIMIT`` characters (``canon``) are written as
their length and digest (``outd``-style: ``<label> <i> <length> <digest>``
with the label suffixed by ``d``). An edge in ``fedges`` is
``(x0, top, x1, bottom, orientation, object_type)``; an edge in
``intersections`` its bbox (``utils.obj_to_bbox``); every edge in full is in
the digest of ``edges``.

Lines::

    open <exc>                         pdfplumber.open raised (nothing else follows)
    pages <exc>                        pdf.pages raised
    page <page_number>
    edges <exc> | edges <n> <digest>   page.edges (the page ends on an exception)
    hedges <n> <digest>                page.horizontal_edges
    vedges <n> <digest>                page.vertical_edges
    op <i> <op>                        i counts the ops of the page, derived pages included
    finder <i> <exc>                   debug_tablefinder raised (the find_table ... lines follow)
    fedges <i> <[edge, ...]>           finder.edges
    fedges_all <i> <n> <digest>        finder.edges in full
    intersections <i> <[(point, [v bbox, ...], [h bbox, ...]), ...]>
    cells <i> <[bbox, ...]>
    table <i> <t> <bbox> <n cells>     each of finder.tables
    rows <i> <t> <[(cells, bbox), ...]>
    columns <i> <t> <[(cells, bbox), ...]>
    extract <i> <t> <rows of cell texts | exc>
    find_table <i> <bbox | None | exc>
    extract_tables <i> <value | exc>
    extract_table <i> <value | exc>
    derived <steps>                    then the lines of its op
    derive <exc>
    !end
"""

from __future__ import annotations

import hashlib
from collections.abc import Iterator
from typing import Any

from pdfplumber.page import Page
from pdfplumber.table import TableSettings
from pdfplumber.utils import geometry

from ..canon import canon
from ._common import terminated
from .plumber_objects import digest, exc, file_sha256, open_pdf, select
from .plumber_text import OUT_LIMIT, derive

PAGED = True
DEPENDS = ("plumber_objects", "plumber_text")

TEXT = {"vertical_strategy": "text", "horizontal_strategy": "text"}
STRICT = {"vertical_strategy": "lines_strict", "horizontal_strategy": "lines_strict"}
EXPLICIT = {"vertical_strategy": "explicit", "horizontal_strategy": "explicit"}
FRACTIONS = [0.0, 0.25, 0.5, 0.75, 1.0]

BASE_OPS: list[list[Any]] = [
    ["settings", {}],
    ["settings", TEXT],
    ["settings", STRICT],
    ["explicit", {**EXPLICIT, "v": FRACTIONS, "h": [0.0, 0.2, 0.4, 0.6, 0.8, 1.0], "v_objects": 0, "h_objects": 0}],
]

VARIANT_OPS: list[list[Any]] = [
    ["settings", {"snap_tolerance": 1}],
    ["settings", {"snap_x_tolerance": 0, "snap_y_tolerance": 6}],
    ["settings", {"join_tolerance": 8}],
    ["settings", {"join_x_tolerance": 0.5, "join_y_tolerance": 10}],
    ["settings", {"edge_min_length": 10}],
    ["settings", {"edge_min_length": 0, "edge_min_length_prefilter": 0}],
    ["settings", {"edge_min_length_prefilter": 20}],
    ["settings", {"intersection_tolerance": 8}],
    ["settings", {"intersection_x_tolerance": 0, "intersection_y_tolerance": 1}],
    ["settings", {"vertical_strategy": "lines", "horizontal_strategy": "text"}],
    ["settings", {"vertical_strategy": "text", "horizontal_strategy": "lines_strict"}],
    ["settings", {**TEXT, "min_words_vertical": 1, "min_words_horizontal": 2}],
    ["settings", {**TEXT, "text_x_tolerance": 1, "text_y_tolerance": 1}],
    ["settings", {**TEXT, "text_keep_blank_chars": True, "text_use_text_flow": True}],
    ["settings", {**TEXT, "text_layout": True}],
    ["settings", {"text_x_tolerance": 1, "text_keep_blank_chars": True, "snap_y_tolerance": 0}],
    ["settings", {"text_settings": {"layout": True, "x_density": 4}}],
    ["settings", {"text_expand_ligatures": False, "text_split_at_punctuation": True}],
    ["settings", {"vertical_strategy": "unknown"}],
    ["settings", {"snap_tolerance": -1}],
    ["settings", {"text_tolerance": 2}],
    ["settings", {"no_such_setting": 1}],
    ["settings", {"explicit_vertical_lines": [100, 200], "explicit_horizontal_lines": [100, 200]}],
    ["explicit", {**EXPLICIT, "v": [0.1, 0.5, 0.9], "h": [0.1, 0.3, 0.5, 0.7], "v_objects": 6, "h_objects": 6}],
    ["explicit", {"vertical_strategy": "explicit", "horizontal_strategy": "lines", "v": FRACTIONS, "h": [], "v_objects": 6, "h_objects": 6}],
    ["explicit", {"vertical_strategy": "lines", "horizontal_strategy": "explicit", "v": [], "h": [0.0, 0.5, 1.0], "v_objects": 0, "h_objects": 3}],
    ["explicit", {**EXPLICIT, "v": [0.5], "h": [0.5], "v_objects": 0, "h_objects": 0}],
]

DERIVED_OPS: list[list[Any]] = [
    [[["crop", [0.1, 0.1, 0.9, 0.9], {}]], ["settings", {}]],
    [[["crop", [0.0, 0.0, 1.0, 0.5], {}]], ["settings", TEXT]],
    [[["within_bbox", [0.0, 0.25, 1.0, 0.75], {}]], ["settings", {}]],
    [[["outside_bbox", [0.0, 0.0, 0.5, 0.5], {}]], ["settings", STRICT]],
    [[["filter", "not-rect"]], ["settings", {}]],
]

VARIANTS_PER_PAGE = 4


def settings_of(page: Page, op: list[Any]) -> dict[str, Any]:
    """The table settings of ``op`` on ``page`` (see the module docstring)."""
    name, settings = op
    if name == "settings":
        return dict(settings)
    out = {k: v for k, v in settings.items() if k not in ("v", "h", "v_objects", "h_objects")}
    x0, top, x1, bottom = page.bbox
    objs = page.objects.get("rect", []) + page.objects.get("line", []) + page.objects.get("curve", [])
    if settings["v"] or settings["v_objects"]:
        out["explicit_vertical_lines"] = [x0 + (x1 - x0) * f for f in settings["v"]] + objs[: settings["v_objects"]]
    if settings["h"] or settings["h_objects"]:
        out["explicit_horizontal_lines"] = [top + (bottom - top) * f for f in settings["h"]] + objs[: settings["h_objects"]]
    return out


def bounded(label: str, i: Any, value: Any) -> str:
    text = canon(value)
    if len(text) > OUT_LIMIT:
        return f"{label}d {i} {len(text)} " + hashlib.sha256(text.encode("ascii")).hexdigest()[:16]
    return f"{label} {i} {text}"


def _edge(e: dict[str, Any]) -> tuple[Any, ...]:
    return (e["x0"], e["top"], e["x1"], e["bottom"], e["orientation"], e.get("object_type"))


def finder_lines(page: Page, i: int, settings: dict[str, Any]) -> Iterator[str]:
    try:
        finder = page.debug_tablefinder(settings)
    except Exception as e:
        yield f"finder {i} " + exc(e)
    else:
        bbox = geometry.obj_to_bbox
        yield bounded("fedges", i, [_edge(e) for e in finder.edges])
        yield f"fedges_all {i} {len(finder.edges)} " + digest(finder.edges)
        yield bounded("intersections", i, [(pt, [bbox(e) for e in d["v"]], [bbox(e) for e in d["h"]]) for pt, d in finder.intersections.items()])
        yield bounded("cells", i, finder.cells)
        text_settings = TableSettings.resolve(settings).text_settings or {}
        for t, table in enumerate(finder.tables):
            yield f"table {i} {t} " + canon(table.bbox) + f" {len(table.cells)}"
            yield bounded("rows", f"{i} {t}", [(r.cells, r.bbox) for r in table.rows])
            yield bounded("columns", f"{i} {t}", [(c.cells, c.bbox) for c in table.columns])
            try:
                yield bounded("extract", f"{i} {t}", table.extract(**text_settings))
            except Exception as e:
                yield f"extract {i} {t} " + exc(e)
    try:
        found = page.find_table(settings)
    except Exception as e:
        yield f"find_table {i} " + exc(e)
    else:
        yield f"find_table {i} " + canon(None if found is None else found.bbox)
    for name in ("extract_tables", "extract_table"):
        try:
            value = getattr(page, name)(settings)
        except Exception as e:
            yield f"{name} {i} " + exc(e)
        else:
            yield bounded(name, i, value)


def op_lines(page: Page, op: list[Any], counter: list[int]) -> Iterator[str]:
    i = counter[0]
    counter[0] += 1
    yield f"op {i} " + canon(op)
    try:
        settings = settings_of(page, op)
    except Exception as e:  # page.objects raised: the finder would too
        yield f"finder {i} " + exc(e)
        return
    yield from finder_lines(page, i, settings)


@terminated
def run(path: str, password: str | None, max_pages: int) -> Iterator[str]:
    sha = file_sha256(path)
    try:
        pdf = open_pdf(path, password, max_pages)
    except Exception as e:
        yield "open " + exc(e)
        return
    try:
        pages = pdf.pages
    except Exception as e:
        yield "pages " + exc(e)
        return
    for index, page in enumerate(pages):
        yield f"page {page.page_number}"
        try:
            edges = page.edges
        except Exception as e:
            yield "edges " + exc(e)
            continue
        yield f"edges {len(edges)} " + digest(edges)
        h, v = page.horizontal_edges, page.vertical_edges
        yield f"hedges {len(h)} " + digest(h)
        yield f"vedges {len(v)} " + digest(v)
        counter = [0]
        for op in BASE_OPS + select(sha, index, VARIANT_OPS, VARIANTS_PER_PAGE):
            yield from op_lines(page, op, counter)
        for steps, op in select(sha, index, DERIVED_OPS, 1):
            yield "derived " + canon(steps)
            try:
                derived = derive(page, steps)
            except Exception as e:
                yield "derive " + exc(e)
                continue
            yield from op_lines(derived, op, counter)


__all__ = ["BASE_OPS", "DERIVED_OPS", "VARIANT_OPS", "settings_of"]
