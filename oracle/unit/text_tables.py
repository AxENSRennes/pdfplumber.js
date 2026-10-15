"""Differential cases for the text and table modules of the port
(crates/pdfplumber/src/utils/{clustering,geometry,generic,text}.rs,
table.rs, obj.rs and crates/pycompat/src/listsort.rs), computed by
pdfplumber 0.11.9. The Rust replay is crates/pdftrace/tests/text_tables_unit.rs.

Writes typed JSON lines (docs/verification.md) to
``<cache>/unit/text_tables/<fingerprint>/<name>.jsonl``:

- ``sort``: ``sorted(range(n), key=xs.__getitem__)`` (CPython's timsort,
  ``list_sort_impl``) on lists with NaNs, ties, runs, mixed types
  (``TypeError``), up to a few thousand items (galloping merges).
- ``ops``: the Python operators the object model reproduces (``+ - * /``,
  unary ``-``, ``abs``, ``< <= > >=``, ``==``, ``repr``, ``min``/``max``,
  ``sum``, ``bool``) on pairs of values of every type, with their results
  or exceptions.
- ``cluster``: ``cluster_list`` and ``cluster_objects`` on numbers on
  tolerance boundaries, ints vs floats, duplicates.
- ``geometry``: every function of ``utils/geometry.py`` on generated
  objects (asserts and errors included).
- ``textlines``: the spans ``re.finditer`` gives for the two patterns of
  ``TextMap.extract_text_lines`` over generated strings (the Rust port
  matches them natively).
- ``settings``: ``TableSettings.resolve(dict)``: resolved fields, or the
  validation error.
- ``pages_fuzz`` (``--fuzz N`` adversarial pages, seed 1): page cases
  (``_plumber_ops.py``) on synthetic pages: chars and edges on exact
  tolerance boundaries, ints vs floats, rotated (``upright`` false) chars,
  ligatures, blank and empty texts, duplicates, degenerate tables, explicit
  lines, invalid options. The real-PDF cases are written by
  ``text_tables_real.py``.

Usage: PDFPLUMBER_ROOT=<main checkout> <root>/.venv/bin/python -m oracle.unit.text_tables [--fuzz 800]
       ... -m oracle.unit.text_tables --dump pages_fuzz:LINE:OP   (the full canon of one operation)
"""

from __future__ import annotations

import argparse
import math
import random
import re
import sys

from pdfminer.pdftypes import PDFObjRef
from pdfminer.psparser import KWD, LIT
from pdfplumber import table
from pdfplumber.table import TableSettings
from pdfplumber.utils import clustering, geometry
from pdfplumber.utils import text as ptext

from ..canon import canon
from ._common import require_oracle_python, unit_dir, write_cases, write_meta
from ._plumber_ops import FakePage, exc_value, guarded, page_case, run_op

MODULE = "text_tables"


# -- synthetic pages ------------------------------------------------------------------


TEXTS = list("abcdefghijklmnopqrstuvwxyzABCDE0123456789") + [
    " ", " ", "\t", " ", ".", ",", "-", ";", "!", "(", ")", "'",
    "ﬁ", "ﬀ", "ﬃ", "ﬆ", "", "ab", "é", "中", "  ", "א",
]


def num(rng, x, int_ok=True):
    """x as an int (when integral and drawn so) or a float."""
    if int_ok and float(x).is_integer() and rng.random() < 0.35:
        return int(x)
    return float(x)


def synth_char(rng, x0, top, w, h, text, upright, doctop_off, fontname, size, color):
    x0n, topn = num(rng, x0), num(rng, top)
    x1n = num(rng, x0 + w)
    bottom = num(rng, top + h)
    page_h = 800
    c = {
        "matrix": (1.0, 0.0, 0.0, 1.0, float(x0), float(page_h - top - h)),
        "fontname": fontname,
        "adv": float(w),
        "upright": upright,
        "x0": x0n,
        "y0": num(rng, page_h - (top + h)),
        "x1": x1n,
        "y1": num(rng, page_h - top),
        "width": num(rng, w),
        "height": num(rng, h),
        "size": size,
        "mcid": None,
        "tag": None,
        "object_type": "char",
        "page_number": 1,
        "ncs": "DeviceGray",
        "text": text,
        "stroking_color": color,
        "non_stroking_color": (0,),
        "top": topn,
        "bottom": bottom,
        "doctop": topn + doctop_off if rng.random() < 0.9 else num(rng, top + 800),
    }
    return c


def synth_chars(rng):
    n_lines = rng.randint(0, 7)
    step = rng.choice([0.5, 1, 0.25, 2, 1 / 3, 3])
    tol = rng.choice([3, 1, 0.5, 2.5])
    chars = []
    y = rng.choice([0, 5, 10.5, 20])
    fonts = ["F1", "F2", "Helvetica-Bold"]
    for _ in range(n_lines):
        x = rng.choice([0, 2, 10, 50.5])
        h = rng.choice([10, 12, 8.5, 9.75])
        upright = rng.random() > 0.15
        n = rng.randint(0, 14)
        for _ in range(n):
            w = rng.choice([5, 6, 4.5, step * 7])
            text = rng.choice(TEXTS)
            size = rng.choice([10.0, 12.0, 9.5, float(h)])
            color = rng.choice([(0,), None, (0.5, 0.5, 0.5), [1, 0, 0]])
            dy = rng.choice([0, 0, 0, step, -step, tol, tol + step, h / 2])
            chars.append(synth_char(rng, x, y + dy, w, h, text, upright, rng.choice([0, 800, 1000.5]), rng.choice(fonts), size, color))
            r = rng.random()
            if r < 0.08:
                chars.append(dict(chars[-1]))  # exact duplicate
            elif r < 0.14:
                d = dict(chars[-1])
                d["x0"] = d["x0"] + rng.choice([0.5, 1, 1.5])
                d["doctop"] = d["doctop"] + rng.choice([0, 1, 0.25])
                chars.append(d)
            gap = rng.choice([0, 0, step, tol, tol + step, -step, 3, 3.0001, 2.9999])
            x += w + gap
        y += h + rng.choice([0, step, tol, 1, 2, 3, 13])
    if rng.random() < 0.2:
        rng.shuffle(chars)
    return chars


def synth_edges(rng, bbox):
    x0, top, x1, bottom = bbox
    edges = []
    kind = rng.random()
    step = rng.choice([1, 0.5, 3, 2.5])
    if kind < 0.6:
        # a grid, with jitter on tolerance boundaries
        nx, ny = rng.randint(1, 6), rng.randint(1, 6)
        gx0, gy0 = rng.choice([10, 20.5, 0]), rng.choice([15, 30, 0.25])
        cw, ch = rng.choice([20, 35.5, 50]), rng.choice([12, 15, 20.25])
        jit = [0, 0, 0, step, -step, 1, 3, 3.5, 0.5]
        for i in range(nx + 1):
            x = gx0 + i * cw + rng.choice(jit)
            for j in range(ny if rng.random() < 0.5 else 1):
                t = gy0 + (j * ch if j else 0) + rng.choice(jit)
                b = gy0 + ((j + 1) * ch if rng.random() < 0.5 else ny * ch) + rng.choice(jit)
                edges.append(edge(rng, x, t, x, b, "v"))
        for j in range(ny + 1):
            y = gy0 + j * ch + rng.choice(jit)
            for i in range(nx if rng.random() < 0.5 else 1):
                l = gx0 + (i * cw if i else 0) + rng.choice(jit)
                r = gx0 + ((i + 1) * cw if rng.random() < 0.5 else nx * cw) + rng.choice(jit)
                edges.append(edge(rng, l, y, r, y, "h"))
    else:
        for _ in range(rng.randint(0, 12)):
            if rng.random() < 0.5:
                x = rng.choice([10, 20, 30, 30.5, 33, 50])
                t, b = sorted([rng.choice([0, 10, 20, 25, 40]), rng.choice([10, 20, 30, 60])])
                edges.append(edge(rng, x, t, x, b, "v"))
            else:
                y = rng.choice([10, 20, 30, 30.5, 33, 50])
                l, r = sorted([rng.choice([0, 10, 20, 25, 40]), rng.choice([10, 20, 30, 60])])
                edges.append(edge(rng, l, y, r, y, "h"))
    if rng.random() < 0.15 and edges:
        edges.append(dict(rng.choice(edges)))
    if rng.random() < 0.1:
        e = edge(rng, 10, 10, 20, 25, None)
        e["object_type"] = "curve_edge"
        edges.append(e)
    if rng.random() < 0.2:
        rng.shuffle(edges)
    return edges


def edge(rng, x0, top, x1, bottom, orientation):
    lo_x, hi_x = min(x0, x1), max(x0, x1)
    lo_y, hi_y = min(top, bottom), max(top, bottom)
    otype = rng.choice(["line", "rect_edge", "rect_edge", "curve_edge", "line"])
    return {
        "x0": num(rng, lo_x),
        "y0": num(rng, 800 - hi_y),
        "x1": num(rng, hi_x),
        "y1": num(rng, 800 - lo_y),
        "width": num(rng, hi_x - lo_x),
        "height": num(rng, hi_y - lo_y),
        "linewidth": rng.choice([1, 0.5]),
        "stroke": True,
        "fill": False,
        "object_type": otype,
        "page_number": 1,
        "top": num(rng, lo_y),
        "bottom": num(rng, hi_y),
        "doctop": num(rng, lo_y),
        "orientation": orientation,
    }


DIRS = ["ttb", "btt", "ltr", "rtl"]
VALID_PAIRS = [("ttb", "ltr"), ("ttb", "rtl"), ("btt", "ltr"), ("btt", "rtl"), ("ltr", "ttb"), ("rtl", "btt"), ("ltr", "btt"), ("rtl", "ttb")]


def rand_text_kwargs(rng, layout_ok=True):
    kw = {}
    pick = rng.random

    def maybe(p, key, values):
        if pick() < p:
            kw[key] = rng.choice(values)

    maybe(0.3, "x_tolerance", [0, 1, 1.5, 3, 3.0, 5, 0.5])
    maybe(0.3, "y_tolerance", [0, 1, 2.5, 3, 3.0, 10])
    maybe(0.15, "x_tolerance_ratio", [None, 0.1, 0.5, 1])
    maybe(0.1, "y_tolerance_ratio", [None, 0.2, 1])
    maybe(0.2, "keep_blank_chars", [True, False, 1, ""])
    maybe(0.15, "use_text_flow", [True, False])
    maybe(0.15, "horizontal_ltr", [False, True])
    maybe(0.15, "vertical_ttb", [False, True])
    if pick() < 0.25:
        ld, cd = rng.choice(VALID_PAIRS)
        kw["line_dir"], kw["char_dir"] = ld, cd
    if pick() < 0.03:
        kw["line_dir"] = rng.choice(["ltr", "foo", None, "ttb"])
        kw["char_dir"] = rng.choice(["ltr", "rtl", "x"])
    if pick() < 0.12:
        kw["line_dir_rotated"], kw["char_dir_rotated"] = rng.choice(VALID_PAIRS + [(None, None)])
    elif pick() < 0.03:
        kw[rng.choice(["line_dir_rotated", "char_dir_rotated"])] = rng.choice(DIRS + [None])
    maybe(0.15, "extra_attrs", [("fontname",), ("size", "fontname"), (), ("stroking_color",), ("upright",), ("missing",)])
    maybe(0.15, "split_at_punctuation", [True, False, ".,", "-", "", "ab"])
    maybe(0.15, "expand_ligatures", [False, True])
    if layout_ok and pick() < 0.4:
        kw["layout"] = rng.choice([True, True, False, 1])
        maybe(0.3, "x_density", [7.25, 5, 3.5, 10, 1])
        maybe(0.3, "y_density", [13, 10, 6.5, 20])
        maybe(0.2, "x_shift", [0, 5, -3.5, 10])
        maybe(0.2, "y_shift", [0, 5, -2, 12.5])
        maybe(0.15, "layout_width_chars", [0, 20, 80, 20.0, True])
        maybe(0.15, "layout_height_chars", [0, 5, 40, 5.0, True])
        maybe(0.1, "layout_width", [0, 100, 300.5])
        maybe(0.1, "layout_height", [0, 50, 200])
        maybe(0.1, "layout_bbox", [(0, 0, 200, 300), (10, 5, 100.5, 80), (0.0, 0.0, 612.0, 792.0), None, ()])
        maybe(0.08, "presorted", [True, False])
    if pick() < 0.12:
        kw["line_dir_render"], kw["char_dir_render"] = rng.choice(VALID_PAIRS + [(None, None)])
    elif pick() < 0.02:
        kw[rng.choice(["line_dir_render", "char_dir_render"])] = rng.choice(DIRS + [None])
    if pick() < 0.01:
        kw["x_density"] = 0
    if pick() < 0.005:
        kw["chars"] = []  # binds to the named parameter: TypeError
    return kw


PATTERNS = [
    (r"\w+", True, True),
    (r"[a-e]+", True, False),
    (r"(\w)(\w)?", True, True),
    (r"(a)|(b)", True, True),
    (r"\s*", True, True),
    (r"", True, True),
    (r" ", True, True),
    (r"A.", False, False),
    (r".", True, True),
    (r"ﬁ|fi", True, True),
    (r"(?P<x>\d)\.?", True, True),
]


def rand_table_settings(rng, page_bbox, chars, edges):
    s = {}
    pick = rng.random

    def maybe(p, key, values):
        if pick() < p:
            s[key] = rng.choice(values)

    strategies = ["lines", "lines_strict", "text", "explicit"]
    maybe(0.5, "vertical_strategy", strategies)
    maybe(0.5, "horizontal_strategy", strategies)
    x0, top, x1, bottom = page_bbox
    pool_v = [10, 20.5, 30, 45, 60, 80.25, 100, 3]
    pool_h = [5, 15, 20, 30.5, 40, 60, 75]
    objs = [e for e in edges][:4]
    if pick() < 0.3:
        # rects and curves as explicit lines (obj_to_edges)
        objs += [rand_obj(rng, rng.choice(["rect", "curve", "line"])) for _ in range(rng.randint(1, 3))]
    if s.get("vertical_strategy") == "explicit" or pick() < 0.15:
        lines = rng.sample(pool_v, rng.randint(0, 5))
        if objs and pick() < 0.4:
            lines += rng.sample(objs, min(len(objs), 2))
        s["explicit_vertical_lines"] = lines if pick() < 0.9 else tuple(lines)
    if s.get("horizontal_strategy") == "explicit" or pick() < 0.15:
        lines = rng.sample(pool_h, rng.randint(0, 5))
        if objs and pick() < 0.4:
            lines += rng.sample(objs, min(len(objs), 2))
        s["explicit_horizontal_lines"] = lines
    if pick() < 0.05:
        s["explicit_vertical_lines"] = None
    tol = [0, 1, 2, 3, 3.0, 0.5, 5, 10]
    maybe(0.3, "snap_tolerance", tol)
    maybe(0.15, "snap_x_tolerance", tol)
    maybe(0.15, "snap_y_tolerance", tol)
    maybe(0.3, "join_tolerance", tol)
    maybe(0.15, "join_x_tolerance", tol)
    maybe(0.15, "join_y_tolerance", tol)
    maybe(0.3, "edge_min_length", [0, 1, 3, 5, 10.5, 25])
    maybe(0.2, "edge_min_length_prefilter", [0, 1, 2, 20])
    maybe(0.3, "min_words_vertical", [0, 1, 2, 3, 5])
    maybe(0.3, "min_words_horizontal", [0, 1, 2, 4])
    maybe(0.3, "intersection_tolerance", tol)
    maybe(0.15, "intersection_x_tolerance", tol)
    maybe(0.15, "intersection_y_tolerance", tol)
    if pick() < 0.3:
        for k, v in rand_text_kwargs(rng, layout_ok=pick() < 0.3).items():
            s["text_" + k] = v
    maybe(0.05, "text_tolerance", [1, 5])
    if pick() < 0.03:
        s[rng.choice(["snap_tolerance", "join_y_tolerance", "min_words_vertical"])] = rng.choice([-1, -0.5, None, "a"])
    if pick() < 0.02:
        s["vertical_strategy"] = "nope"
    if pick() < 0.02:
        s["foo"] = 1
    return s


def synth_page_case(rng):
    bbox = rng.choice([(0, 0, 612, 792), (0, 0, 200.5, 300), (10, 20, 300, 400), (0.0, 0.0, 595.0, 842.0)])
    chars = synth_chars(rng)
    if chars and rng.random() < 0.03:
        # a char whose text is not a str (unhashable or not)
        rng.choice(chars)["text"] = rng.choice([["x"], ("x",), 5])
    edges = synth_edges(rng, bbox) if rng.random() < 0.8 else []
    page = FakePage(chars, edges, bbox)
    ops = []
    for _ in range(rng.randint(12, 26)):
        r = rng.random()
        if rng.random() < 0.08:
            kw = rand_text_kwargs(rng)
            ex = {k: v for k, v in kw.items() if k in ptext.WORD_EXTRACTOR_KWARGS}
            tm = {k: v for k, v in kw.items() if k in ptext.TEXTMAP_KWARGS}
            tm["presorted"] = rng.choice([False, False, True, 0])
            if rng.random() < 0.03:
                (ex if rng.random() < 0.5 else tm)["bogus"] = 1
            ops.append(("wordmap", ex, tm))
        elif r < 0.14:
            ops.append(("text", rand_text_kwargs(rng)))
        elif r < 0.24:
            ops.append(("utext", rand_text_kwargs(rng)))
        elif r < 0.30:
            kw = {}
            if rng.random() < 0.6:
                kw["x_tolerance"] = rng.choice([0, 1, 3, 2.5])
            if rng.random() < 0.6:
                kw["y_tolerance"] = rng.choice([0, 1, 3, 2.5])
            ops.append(("simple", kw))
        elif r < 0.44:
            kw = rand_text_kwargs(rng, layout_ok=False)
            if rng.random() < 0.9:
                kw.pop("line_dir_render", None)
                kw.pop("char_dir_render", None)
            if rng.random() < 0.4:
                kw["return_chars"] = rng.choice([True, False])
            ops.append(("words", kw))
        elif r < 0.52:
            ops.append(("lines", rng.choice([True, False]), rng.choice([True, False]), rand_text_kwargs(rng)))
        elif r < 0.62:
            pattern, regex, case = rng.choice(PATTERNS)
            main_group = rng.choice([0, 0, 1]) if "(" in pattern or rng.random() < 0.1 else 0
            ops.append(("search", rand_text_kwargs(rng), pattern, regex, case, main_group, rng.choice([True, False]), rng.choice([True, False])))
        elif r < 0.70:
            kw = {}
            if rng.random() < 0.6:
                kw["tolerance"] = rng.choice([0, 1, 0.5, 2, 1.0])
            if rng.random() < 0.5:
                kw["extra_attrs"] = rng.choice([None, (), ("fontname",), ("size",), ("stroking_color",), ("fontname", "size")])
            ops.append(("dedupe", kw))
        elif r < 0.84:
            ops.append(("tables", rand_table_settings(rng, bbox, chars, edges)))
        elif r < 0.94:
            ops.append(("extract_tables", rand_table_settings(rng, bbox, chars, edges)))
        else:
            ops.append(("extract_table", rand_table_settings(rng, bbox, chars, edges)))
    record = {"chars": chars, "edges": edges, "bbox": bbox}
    return page_case(page, record, ops)


def fuzz_cases(n: int):
    rng = random.Random(1)
    for _ in range(n):
        yield synth_page_case(rng)
    yield from rounding_cases()


def rounding_triples():
    """(h_top, tolerance, v_end) where the rounding of ``h_top + tol`` (or
    ``h_top - tol``) decides an intersection: ``h_top`` is the float just
    beyond ``v_top - tol`` (``v_bottom + tol``), yet the predicate of
    ``edges_to_intersections`` holds. Deterministic search."""
    rng = random.Random(8)
    top, bottom = [], []
    while len(top) < 6 or len(bottom) < 6:
        b = rng.choice([0.3, 0.7, 1.1, 10.3, 100.7, 33.33, 612.9]) + rng.randint(0, 50) * 0.1
        t = rng.choice([0.1, 0.3, 1, 3, 0.5, 2.7])
        a = math.nextafter(b - t, -math.inf)
        if len(top) < 6 and a + t >= b:
            top.append((a, t, b))
        a = math.nextafter(b + t, math.inf)
        if len(bottom) < 6 and a - t <= b:
            bottom.append((a, t, b))
    return top, bottom


def line_edge(x0, top, x1, bottom):
    return {
        "x0": x0, "y0": 800 - bottom, "x1": x1, "y1": 800 - top, "width": x1 - x0, "height": bottom - top,
        "object_type": "line", "top": top, "bottom": bottom, "doctop": top,
        "orientation": "v" if x0 == x1 else "h",
    }


def rounding_cases():
    """Pages with one vertical and one horizontal edge meeting only through
    the rounding of the tolerance arithmetic (the search window of the
    port's edges_to_intersections must keep them)."""
    top, bottom = rounding_triples()
    for (a, t, b), side in [(x, "top") for x in top] + [(x, "bottom") for x in bottom]:
        v = line_edge(50, b, 50, b + 20) if side == "top" else line_edge(50, b - 20, 50, b)
        h = line_edge(0, a, 100, a)
        page = FakePage([], [v, h], (0, 0, 612, 792))
        base = {"snap_tolerance": 0, "join_tolerance": 0, "edge_min_length": 0, "edge_min_length_prefilter": 0}
        ops = [
            ("tables", {**base, "intersection_tolerance": t}),
            ("tables", {**base, "intersection_y_tolerance": t, "intersection_x_tolerance": 0}),
        ]
        yield page_case(page, {"chars": [], "edges": [v, h], "bbox": (0, 0, 612, 792)}, ops)


# -- unit sets ------------------------------------------------------------------------


def sort_cases():
    rng = random.Random(2)
    nan = float("nan")
    for i in range(1500):
        n = rng.choice([0, 1, 2, 3, 5, 10, 31, 64, 65, 100, 200, 500, 1000, 2500])
        kind = rng.random()
        if kind < 0.3:
            xs = [rng.choice([nan, 1.0, 2.0, 3.0, 0.5, -1.0, 2]) for _ in range(n)]
        elif kind < 0.55:
            # runs (ascending, descending) with NaNs sprinkled: galloping merges
            xs = []
            while len(xs) < n:
                run = sorted(rng.random() * 100 for _ in range(rng.randint(1, 80)))
                if rng.random() < 0.5:
                    run.reverse()
                xs.extend(run)
            xs = xs[:n]
            for _ in range(rng.randint(0, 4)):
                if xs:
                    xs[rng.randrange(len(xs))] = nan
        elif kind < 0.75:
            xs = [rng.randint(0, 5) for _ in range(n)]
        elif kind < 0.9:
            xs = [rng.choice([nan, rng.random(), rng.randint(0, 3)]) for _ in range(n)]
        else:
            xs = [rng.choice([1, 2.5, None, "a", "b", (1, 2), (1, nan)]) if rng.random() < 0.05 else rng.random() for _ in range(n)]
        try:
            out = sorted(range(n), key=xs.__getitem__)
        except Exception as e:
            out = exc_value(e)
        yield [xs], out


OP_VALUES = [
    None, True, False, 0, 1, -1, 3, 2**70, 0.0, -0.0, 1.0, 1.5, -2.5, 3.0, float("inf"), float("nan"),
    float(2**70), 2**63, float(2**63), -(2**63), float(-(2**63)), (1, [2]),
    "", "a", "ab", "b", b"x", (), (1,), (1, 2), (1.0, 2.5), (1, None), [1], [1, 2], [], {}, {"a": 1}, {"a": 1, "b": (2,)},
    # pdfminer objects found in pdfplumber dicts (pattern color names, ...)
    LIT("F1"), KWD(b"BT"), PDFObjRef(None, 7),
]


def ops_cases():
    def run(op, a, b):
        if op == "add":
            return a + b
        if op == "sub":
            return a - b
        if op == "mul":
            return a * b
        if op == "truediv":
            return a / b
        if op == "lt":
            return a < b
        if op == "le":
            return a <= b
        if op == "gt":
            return a > b
        if op == "ge":
            return a >= b
        if op == "eq":
            return a == b
        if op == "neg":
            return -a
        if op == "abs":
            return abs(a)
        if op == "repr":
            return repr(a)
        if op == "bool":
            return bool(a)
        if op == "min":
            return min(a, b)
        if op == "max":
            return max(a, b)
        if op == "sum":
            return sum([a, b])
        if op == "dictget":
            # a dict keyed by a, looked up with b (hash() and == of keys)
            return {a: "found"}.get(b)
        if op == "unpack4":
            x0, top, x1, bottom = a
            return (x0, top, x1, bottom)
        raise ValueError(op)

    seq = (str, bytes, tuple, list)
    for op in ["add", "sub", "mul", "truediv", "lt", "le", "gt", "ge", "eq", "min", "max", "sum", "dictget"]:
        for a in OP_VALUES:
            for b in OP_VALUES:
                # sequence concatenation and repetition: outside the model
                if op == "add" and isinstance(a, seq) and type(a) is type(b):
                    continue
                if op == "mul" and (isinstance(a, seq) or isinstance(b, seq)) and (type(a) in (int, bool) or type(b) in (int, bool)):
                    continue
                # a NaN key is found through its identity only, which the port does not model
                if op == "dictget" and a is b and a != a:
                    continue
                # a is b: the Rust replay passes the same value (identity of PDF objects)
                yield [op, a, b, a is b], guarded(run, op, a, b)
    for op in ["neg", "abs", "repr", "bool", "unpack4"]:
        for a in OP_VALUES:
            # unpacking iterates: strings, bytes and dicts are outside the model
            if op == "unpack4" and isinstance(a, (str, bytes, dict)):
                continue
            yield [op, a, None, False], guarded(run, op, a, None)
    for items in [(1, 2, 3, 4), [1.0, 2, 3, 4.5], (1, 2, 3), (1, 2, 3, 4, 5), [None] * 4, ()]:
        yield ["unpack4", items, None, False], guarded(run, "unpack4", items, None)


DICT_KEYS = ["x0", "top", "x1", "bottom", "doctop", "width", "height", "text", "upright", "size",
             "orientation", "object_type", "y0", "y1", "fontname", "k", "chars"]


def dict_cases():
    """Sequences of dict operations (the Obj model): assignment of a new or
    existing key, del (KeyError when absent), get, in, keys, ==, copy."""
    rng = random.Random(7)
    for i in range(1500):
        d = {}
        other = {}
        steps, results = [], []
        for _ in range(rng.randint(1, 30)):
            op = rng.choice(["set", "set", "set", "del", "get", "in", "keys", "eq", "copyset"])
            k = rng.choice(DICT_KEYS)
            v = rng.choice([0, 1, 1.0, 2.5, "a", None, (1,)])
            try:
                if op == "set":
                    d[k] = v
                    r = None
                elif op == "del":
                    del d[k]
                    r = None
                elif op == "get":
                    r = d.get(k)
                elif op == "in":
                    r = k in d
                elif op == "keys":
                    r = list(d.keys())
                elif op == "eq":
                    r = d == other
                    other = dict(d)  # compared with the next eq
                else:  # copyset: a copy changed does not change the original
                    c = dict(d)
                    c[k] = v
                    r = (list(c.items()), list(d.items()))
            except Exception as e:
                r = exc_value(e)
            steps.append((op, k, v))
            results.append(r)
        yield [steps], results


def cluster_cases():
    rng = random.Random(3)
    for i in range(3000):
        n = rng.randint(0, 25)
        step = rng.choice([0.5, 1, 0.1, 1 / 3, 3])
        xs = [num(rng, rng.randint(0, 20) * step) for _ in range(n)]
        tol = rng.choice([0, 0.0, 1, 0.5, 1.5, 3, 0.1, step, 2 * step])
        if rng.random() < 0.5:
            yield ["cluster_list", xs, tol], guarded(clustering.cluster_list, xs, tol)
        else:
            objs = [{"v": x, "i": k} for k, x in enumerate(xs)]
            preserve = rng.random() < 0.3
            yield ["cluster_objects", objs, tol, preserve], guarded(clustering.cluster_objects, objs, "v", tol, preserve)
    for xs, tol in [([1, None], 1), ([None], 1), ([1.0, "a"], 0), ([(1, 2), (1, 3)], 0), ([[1]], 1)]:
        yield ["cluster_list", xs, tol], guarded(clustering.cluster_list, xs, tol)


def rand_obj(rng, kind=None):
    step = rng.choice([0.5, 1, 0.25])
    x0 = num(rng, rng.randint(0, 40) * step)
    top = num(rng, rng.randint(0, 40) * step)
    w = num(rng, rng.randint(0, 20) * step)
    h = num(rng, rng.randint(0, 20) * step)
    kind = kind or rng.choice(["rect", "line", "curve", "char", "rect_edge"])
    o = {
        "x0": x0,
        "y0": num(rng, 800 - top - h),
        "x1": x0 + w,
        "y1": num(rng, 800 - top),
        "width": w,
        "height": h,
        "object_type": kind,
        "top": top,
        "bottom": top + h,
        "doctop": top + rng.choice([0, 800, 1000.5]),
    }
    if kind == "line" and rng.random() < 0.5:
        o["bottom"] = o["top"]
    if kind == "curve":
        pts = [(num(rng, rng.randint(0, 40) * step), num(rng, rng.randint(0, 40) * step)) for _ in range(rng.randint(0, 5))]
        if rng.random() < 0.3 and pts:
            pts.append((pts[-1][0], pts[-1][1] + 3))
        o["pts"] = pts if rng.random() < 0.8 else [list(p) for p in pts]
    if kind == "rect_edge":
        o["orientation"] = rng.choice(["v", "h", None])
    if rng.random() < 0.1:
        del o["doctop"]
    if rng.random() < 0.05:
        del o[rng.choice(["x0", "top", "y0", "height"])]
    return o


def rand_bbox(rng):
    step = rng.choice([0.5, 1, 0.25])
    x0 = num(rng, rng.randint(0, 40) * step)
    top = num(rng, rng.randint(0, 40) * step)
    return (x0, top, x0 + num(rng, rng.randint(-2, 20) * step), top + num(rng, rng.randint(-2, 20) * step))


def geometry_cases():
    rng = random.Random(4)
    for i in range(4000):
        fn = rng.choice(
            ["objects_to_bbox", "get_bbox_overlap", "calculate_area", "clip_obj", "intersects_bbox", "within_bbox",
             "outside_bbox", "crop_to_bbox", "move_object", "snap_objects", "resize_object", "curve_to_edges",
             "rect_to_edges", "line_to_edge", "obj_to_edges", "filter_edges", "objects_to_rect"]
        )
        if fn in ("objects_to_bbox", "objects_to_rect"):
            args = [[rand_obj(rng) for _ in range(rng.randint(0, 4))]]
        elif fn == "get_bbox_overlap":
            args = [rand_bbox(rng), rand_bbox(rng)]
        elif fn == "calculate_area":
            args = [rand_bbox(rng)]
        elif fn == "clip_obj":
            args = [rand_obj(rng), rand_bbox(rng)]
        elif fn in ("intersects_bbox", "within_bbox", "outside_bbox", "crop_to_bbox"):
            args = [[rand_obj(rng) for _ in range(rng.randint(0, 5))], rand_bbox(rng)]
        elif fn == "move_object":
            args = [rand_obj(rng), rng.choice(["h", "v"]), num(rng, rng.choice([0, 1, -2.5, 3]))]
        elif fn == "snap_objects":
            args = [[rand_obj(rng) for _ in range(rng.randint(0, 8))], rng.choice(["x0", "x1", "top", "bottom"]), rng.choice([0, 1, 0.5, 3, 2.0])]
        elif fn == "resize_object":
            o = rand_obj(rng)
            value = num(rng, rng.randint(0, 40) * 0.5)
            bounds = [o[k] for k in ("x0", "x1", "top", "bottom") if k in o]
            if bounds and rng.random() < 0.5:
                value = rng.choice(bounds)  # on the bound of the assert
            args = [o, rng.choice(["x0", "x1", "top", "bottom"]), value]
        elif fn == "curve_to_edges":
            args = [rand_obj(rng, "curve")]
        elif fn == "rect_to_edges":
            args = [rand_obj(rng, "rect")]
        elif fn == "line_to_edge":
            args = [rand_obj(rng, "line")]
        elif fn == "obj_to_edges":
            o = rand_obj(rng)
            if rng.random() < 0.05:
                o["object_type"] = rng.choice(["image", 3, "foo_edge"])
            args = [o]
        else:  # filter_edges
            edges = [rand_obj(rng, "rect_edge") for _ in range(rng.randint(0, 6))]
            for e in edges:
                e["object_type"] = rng.choice(["line", "rect_edge", "curve_edge"])
            args = [edges, rng.choice(["v", "h", None, None, "x"]), rng.choice([None, "line", "rect_edge"]), rng.choice([0, 1, 3, 2.5])]
        yield [fn, *args], guarded(getattr(geometry, fn), *args)
    # table.snap_edges on edges of any orientation (TableFinder only gives it
    # "v" and "h" edges)
    for i in range(300):
        edges = [rand_obj(rng, "rect_edge") for _ in range(rng.randint(0, 6))]
        for e in edges:
            if rng.random() < 0.15:
                e["orientation"] = rng.choice([None, "x", ["v"], ("h",)])
        args = [edges, rng.choice([0, 1, 2.5]), rng.choice([0, 1, 2.5])]
        yield ["snap_edges", *args], guarded(table.snap_edges, *args)


def textline_cases():
    rng = random.Random(5)
    alphabet = [" ", " ", " ", "\n", "a", "b", "\t", "\r", " ", "x"]
    pats = {True: re.compile(r" *([^\n]+?) *(\n|$)"), False: re.compile(r"([^\n]+)")}
    for i in range(6000):
        s = "".join(rng.choice(alphabet) for _ in range(rng.randint(0, 30)))
        strip = rng.random() < 0.7
        p = pats[strip]
        spans = [[(m.start(g), m.end(g)) if m.start(g) >= 0 else None for g in range(p.groups + 1)] for m in p.finditer(s)]
        yield [s, strip], spans


def settings_value(s: TableSettings):
    return {k: getattr(s, k) for k in s.__dataclass_fields__}


def settings_cases():
    rng = random.Random(6)
    yield [None], settings_value(TableSettings.resolve(None))
    for i in range(1500):
        s = rand_table_settings(rng, (0, 0, 100, 100), [], [])
        if rng.random() < 0.1:
            s["text_settings"] = {"x_tolerance": 1}
        if rng.random() < 0.1:
            s[rng.choice(list(TableSettings.__dataclass_fields__))] = rng.choice([None, -1, "lines", 0.0, False, 2])
        yield [s], guarded(lambda: settings_value(TableSettings.resolve(s)))


# -- main -----------------------------------------------------------------------------


def dump(spec: str) -> None:
    """Prints the full canon of one page operation: NAME:LINE:OP (0-based)."""
    from ..typed_json import loads

    name, line, op = spec.split(":")
    path = unit_dir(MODULE) / f"{name}.jsonl"
    with open(path, encoding="ascii") as f:
        for k, text in enumerate(f):
            if k == int(line):
                case = loads(text)
                record, ops = case["in"]
                page = FakePage(record["chars"], record["edges"], record["bbox"])
                o = ops[int(op)]
                print("op:", canon(o)[:2000])
                print(canon(guarded(run_op, page, tuple(o)) if o[0] != "search" else "search: see the generator"))
                return
    sys.exit(f"no line {line} in {path}")


def main() -> int:
    ap = argparse.ArgumentParser(prog="python -m oracle.unit.text_tables", description=__doc__.split("\n\n")[0])
    ap.add_argument("--fuzz", type=int, default=800, help="number of adversarial pages (default 800)")
    ap.add_argument("--dump", metavar="NAME:LINE:OP")
    args = ap.parse_args()
    require_oracle_python()
    if args.dump:
        dump(args.dump)
        return 0
    counts = {}
    counts["sort"] = write_cases(MODULE, "sort", sort_cases())
    counts["ops"] = write_cases(MODULE, "ops", ops_cases())
    counts["cluster"] = write_cases(MODULE, "cluster", cluster_cases())
    counts["geometry"] = write_cases(MODULE, "geometry", geometry_cases())
    counts["textlines"] = write_cases(MODULE, "textlines", textline_cases())
    counts["settings"] = write_cases(MODULE, "settings", settings_cases())
    counts["dicts"] = write_cases(MODULE, "dicts", dict_cases())
    counts["pages_fuzz"] = write_cases(MODULE, "pages_fuzz", fuzz_cases(args.fuzz))
    write_meta(MODULE, counts=counts, fuzz=args.fuzz)
    print(counts, flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
