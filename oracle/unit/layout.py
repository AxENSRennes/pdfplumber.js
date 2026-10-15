"""Differential cases for crates/pdfminer/src/layout.rs, computed by pdfminer.

Writes typed JSON lines (docs/verification.md) to
``<cache>/unit/layout/<fingerprint>/<name>.jsonl`` (see
``oracle/unit/_common.py``); the Rust replay is
crates/pdftrace/tests/layout_unit.rs.

Layout analysis cases (``real``, ``adversarial``)
-------------------------------------------------
A case is a raw page, as pdfminer's ``PDFPageAggregator`` builds it without
``laparams`` (the objects the analysis starts from; for the real pages,
pdfplumber's ``PDFPageAggregatorWithMarkedContent``, which also sets
``mcid`` and ``tag`` on them), and its analysis under
several ``LAParams``. Input: ``[name, page, runs]`` where ``page`` is the raw
``LTPage`` dumped by ``dump_raw`` (every object as ``Named(class, __dict__)``,
in ``__dict__`` order; ``graphicstate`` is left out (``None``), the color
space is ``Named("PDFColorSpace", ...)``, an image's stream keeps its
attributes and no data), and each run is ``[laparams, depth, ids]``:

- ``laparams``: the ``LAParams`` keyword arguments (``CONFIGS``);
- ``depth``: the recursion depth ``page.analyze`` was called at
  (``_common.at_depth``);
- ``ids``: Python's ``id()`` of the text boxes and groups, which
  ``group_textboxes`` uses to order pairs at equal distance (see
  ``layout.rs``): ``[min_char, n_boxes, rank]`` for each box and group of
  the result, keyed by the smallest raw index of its chars and its number of
  boxes, ``rank`` being the order of the ids.

Each run analyzes a fresh copy of the page rebuilt from the dump
(``rebuild``: the same classes, ``__dict__`` restored in order; checked to
dump back identically). Output: for each run, ``canon`` of the analyzed
tree (``dump_tree``): containers with all their attributes, plus
``get_text()`` of lines, boxes and groups (those of a tree no higher than
``GROUP_TEXT_HEIGHT``); the raw objects (chars, curves, images)
by their index in the raw page (``Named("#", {"i": n})``, depth-first
order); the boxes of a group by their position in the container's
``_objs`` (``Named("box#", ...)``). An analysis that raises gives the
exception.

- ``real``: the first ``--max-pages`` pages of every PDF of the corpus set
  ``upstream`` and the first page of every PDF of ``real`` (pages of more
  than ``MAX_CHARS`` objects are skipped), with every configuration of
  ``CONFIGS`` on pages of at most ``SMALL`` objects (but ``ZERO``, which
  makes a box of each char, so ``n**2`` heap entries, only up to
  ``ZERO_MAX_CHARS``), and ``BIG_CONFIGS`` on larger pages (the data stays
  around 150 MB).
- ``adversarial``: generated pages (``adversarial_pages``): chars on exact
  margin boundaries (``char_margin``, ``line_overlap``, ``word_margin``,
  ``line_margin``), overlapping and duplicated chars, vertical text, blank
  and zero-size chars, grids of equal boxes (ties in the heap), NaN, infinite
  and huge coordinates, chars outside the page, lines of different lengths
  aligned on one edge or on the center only, figures (nested, with
  chars), curves and images among chars, many boxes, chains of groups
  whose depth reaches Python's recursion limit, and objects (figures
  included) given ``mcid``/``tag`` attributes as pdfplumber's
  ``tag_cur_item`` does, once or again, of every kind of value.

Object cases
------------
- ``ltchar``: ``LTChar(...)`` with a test font (``fontname``,
  ``is_vertical()``, ``get_descent()``) on random arguments: the dumped
  attributes or the exception; then vertical chars whose ``(vx, vy)`` are
  any value ``pdffont`` can hand over (PDF objects of every type, ``str``
  dict keys) with float and int font sizes (``TypeError``/``OverflowError``
  of the arithmetic, sequence repetition).
- ``objects``: the constructors of ``LTCurve``, ``LTLine``, ``LTRect``,
  ``LTImage``, ``LTFigure``, ``LTPage``, ``LAParams`` (validation), the
  ``LTComponent`` distance methods on random boxes, and the figure of a Form
  XObject from its unchecked ``/Matrix`` and ``/BBox`` items (any PDF
  object): ``mult_matrix(matrix, ctm)`` (numbers, ``[type, len]`` of a
  sequence, or the exception) and ``LTFigure(name, bbox, that)``; and
  ``list.sort`` on the keys the layout sorts by, with NaN keys in lists
  shorter than 64 (``list_sort``, ``list_sort_tuple``: the sorted indices).
- ``textline``: ``LTTextLineHorizontal``/``LTTextLineVertical`` built by
  ``add`` on random chars and word margins: the line, ``get_text()``,
  ``is_empty()``.

Usage: PDFPLUMBER_ROOT=<main checkout> <root>/.venv/bin/python -m oracle.unit.layout [--max-pages N] [-j N]
"""

from __future__ import annotations

import argparse
import math
import multiprocessing
import random
import signal
import sys

from pdfminer.layout import (
    LAParams,
    LTAnno,
    LTChar,
    LTComponent,
    LTCurve,
    LTFigure,
    LTImage,
    LTLine,
    LTPage,
    LTRect,
    LTTextBox,
    LTTextBoxHorizontal,
    LTTextBoxVertical,
    LTTextGroup,
    LTTextGroupLRTB,
    LTTextGroupTBRL,
    LTTextLine,
    LTTextLineHorizontal,
    LTTextLineVertical,
)
from pdfminer.pdfcolor import PREDEFINED_COLORSPACE, PDFColorSpace
from pdfminer.pdfinterp import PDFPageInterpreter, PDFResourceManager
from pdfminer.pdfpage import PDFPage
from pdfminer.pdftypes import PDFObjRef, PDFStream
from pdfminer.psparser import KWD, LIT
from pdfminer.utils import mult_matrix
from pdfplumber.page import PDFPageAggregatorWithMarkedContent

from ..canon import Named, canon
from ..corpus import load_set
from ..stages._common import open_document
from ..typed_json import dumps, loads
from ._common import at_depth, require_oracle_python, write_cases, write_meta

MAX_CHARS = 1500
SMALL = 800
ZERO_MAX_CHARS = 600
FILE_TIMEOUT_S = 300
#: The highest group whose get_text() the tree dump includes.
GROUP_TEXT_HEIGHT = 100

#: LAParams keyword arguments of the runs.
CONFIGS = [
    {},
    {"boxes_flow": None},
    {"detect_vertical": True},
    {"all_texts": True},
    (ZERO := {"line_overlap": 0.0, "char_margin": 0.0, "line_margin": 0.0, "word_margin": 0.0}),
    {"char_margin": 1e6, "line_margin": 1e3, "boxes_flow": 1.0, "detect_vertical": True, "all_texts": True},
    {"line_overlap": 1, "char_margin": 3, "line_margin": 1, "word_margin": 1, "boxes_flow": -1},
    {"boxes_flow": None, "detect_vertical": True, "all_texts": True, "word_margin": 0.5},
]

#: The configurations of the real pages of more than SMALL objects.
BIG_CONFIGS = [CONFIGS[0], CONFIGS[1], CONFIGS[2]]

CLASSES = {
    c.__name__: c
    for c in (LTChar, LTCurve, LTLine, LTRect, LTImage, LTFigure, LTPage, LTAnno, LTTextLineHorizontal,
              LTTextLineVertical, LTTextBoxHorizontal, LTTextBoxVertical, LTTextGroupLRTB, LTTextGroupTBRL)
}


# -- raw pages ----------------------------------------------------------------


def dump_raw(obj):
    """``Named(class, __dict__)`` of a raw layout object (see the module docstring)."""
    fields = {}
    for k, v in obj.__dict__.items():
        if k == "_objs":
            v = [dump_raw(c) for c in v]
        elif k == "ncs":
            v = Named("PDFColorSpace", {"name": v.name, "ncomponents": v.ncomponents})
        elif k == "graphicstate":
            v = None
        elif k == "stream":
            v = PDFStream(dict(v.attrs), b"")
        elif k == "groups" and v is not None:
            raise ValueError("an analyzed page")
        fields[k] = v
    return Named(type(obj).__name__, fields)


def rebuild(named):
    """The layout object of a ``dump_raw`` dump (constructors bypassed)."""
    obj = object.__new__(CLASSES[named.name])
    for k, v in named.fields.items():
        if k == "_objs":
            v = [rebuild(c) for c in v]
        elif k == "ncs":
            v = PDFColorSpace(v.fields["name"], v.fields["ncomponents"])
        elif k == "stream":
            v = PDFStream(dict(v.attrs), b"")
        setattr(obj, k, v)
    return obj


def raw_index(page):
    """``id(obj) -> n`` of the raw objects, depth first."""
    index = {}

    def walk(objs):
        for obj in objs:
            index[id(obj)] = len(index)
            if isinstance(obj, LTFigure):
                walk(obj._objs)

    walk(page._objs)
    return index


# -- analyzed trees -----------------------------------------------------------


def dump_tree(container, index):
    """The analyzed tree of a page or figure (see the module docstring)."""
    positions = {id(o): i for i, o in enumerate(container._objs)}

    def item(obj):
        if id(obj) in index:
            if isinstance(obj, LTFigure):
                return figure(obj)
            return Named("#", {"i": index[id(obj)]})
        if isinstance(obj, LTAnno):
            return Named("LTAnno", {"_text": obj._text})
        if isinstance(obj, (LTTextLine, LTTextBox)):
            fields = {}
            for k, v in obj.__dict__.items():
                fields[k] = [item(c) for c in v] if k == "_objs" else v
            fields["get_text()"] = obj.get_text()
            return Named(type(obj).__name__, fields)
        raise TypeError(type(obj))

    def group(g):
        return group_height(g)[0]

    def group_height(g):
        """The dump of a group and the height of its tree (a box is 0). A
        group no higher than GROUP_TEXT_HEIGHT also gets its ``get_text()``
        (a higher one would recurse near the limit, at the dump's depth)."""
        if isinstance(g, LTTextBox):
            return Named("box#", {"i": positions[id(g)]}), 0
        fields = {}
        height = 0
        for k, v in g.__dict__.items():
            if k == "_objs":
                children = [group_height(c) for c in v]
                height = 1 + max((h for _, h in children), default=0)
                v = [d for d, _ in children]
            fields[k] = v
        if height <= GROUP_TEXT_HEIGHT:
            fields["get_text()"] = g.get_text()
        return Named(type(g).__name__, fields), height

    def figure(fig):
        return dump_tree(fig, index)

    fields = {}
    for k, v in container.__dict__.items():
        if k == "_objs":
            v = [item(c) for c in v]
        elif k == "groups" and v is not None:
            v = [group(g) for g in v]
        fields[k] = v
    return Named(type(container).__name__, fields)


def id_ranks(page, index):
    """``[min_char, n_boxes, rank]`` of every box and group in the groups of
    the page and of its figures."""
    found = []

    def chars_of(obj):
        if isinstance(obj, LTTextBox):
            return min(index[id(c)] for line in obj for c in line if isinstance(c, LTChar)), 1
        lo, n = None, 0
        for c in obj:
            m, k = chars_of(c)
            lo = m if lo is None else min(lo, m)
            n += k
        return lo, n

    def walk_group(g):
        m, n = chars_of(g)
        found.append((id(g), m, n))
        if isinstance(g, LTTextGroup):
            for c in g:
                walk_group(c)

    def walk(container):
        for obj in container._objs:
            if isinstance(obj, LTFigure):
                walk(obj)
        for g in container.groups or []:
            walk_group(g)

    walk(page)
    found.sort()
    return [[m, n, rank] for rank, (_, m, n) in enumerate(found)]


def analyze_runs(raw, configs):
    """The runs of a case and their outputs."""
    runs, outs = [], []
    for cfg in configs:
        page = rebuild(raw)
        index = raw_index(page)
        lp = LAParams(**cfg)
        depth, err = at_depth(page.analyze, lp)
        if isinstance(err, BaseException):
            # the groups are set before their analysis can fail
            runs.append([cfg, depth, id_ranks(page, index)])
            outs.append(canon(err))
            continue
        runs.append([cfg, depth, id_ranks(page, index)])
        outs.append(canon(dump_tree(page, index)))
    return runs, outs


def check_dump(raw):
    again = dump_raw(rebuild(raw))
    if canon(again) != canon(raw):
        raise AssertionError("dump_raw(rebuild(page)) differs")


# -- real pages ---------------------------------------------------------------


class Timeout(Exception):
    pass


def on_alarm(signum, frame):
    raise Timeout()


def process_file(args):
    path, rel, password, max_pages = args
    signal.signal(signal.SIGALRM, on_alarm)
    signal.alarm(FILE_TIMEOUT_S)
    out = []
    try:
        with open_document(path, password) as doc:
            rsrcmgr = PDFResourceManager()
            # pdfplumber's aggregator: pdfminer's, plus the mcid and tag
            # attributes it sets on the objects
            device = PDFPageAggregatorWithMarkedContent(rsrcmgr, laparams=None)
            interpreter = PDFPageInterpreter(rsrcmgr, device)
            for pageno, page in enumerate(PDFPage.create_pages(doc), start=1):
                if pageno > max_pages:
                    break
                try:
                    interpreter.process_page(page)
                    layout = device.get_result()
                except Exception:
                    continue
                nobjs = len(raw_index(layout))
                if nobjs > MAX_CHARS:
                    continue
                raw = dump_raw(layout)
                check_dump(raw)
                if nobjs > SMALL:
                    configs = BIG_CONFIGS
                else:
                    configs = [c for c in CONFIGS if c is not ZERO or nobjs <= ZERO_MAX_CHARS]
                runs, outs = analyze_runs(raw, configs)
                # sent back as typed JSON: the objects hold references to the
                # document, which do not pickle
                out.append(dumps([[f"{rel}#{pageno}", raw, runs], outs]))
    except Timeout:
        pass
    except Exception:
        pass
    finally:
        signal.alarm(0)
    return out


def gen_real(max_pages, jobs):
    files = [(str(f.path), f.rel, f.password, max_pages) for f in load_set("upstream")]
    files += [(str(f.path), f.rel, f.password, 1) for f in load_set("real")]
    with multiprocessing.Pool(jobs) as pool:
        for cases in pool.imap(process_file, files):
            for text in cases:
                args, outs = loads(text)
                yield args, outs


# -- generated pages ------------------------------------------------------------


class TestFont:
    """The three PDFFont members LTChar reads."""

    def __init__(self, fontname="F", vertical=False, descent=-0.2):
        self.fontname = fontname
        self.vertical = vertical
        self.descent = descent

    def is_vertical(self):
        return self.vertical

    def get_descent(self):
        return self.descent


GRAY = PREDEFINED_COLORSPACE["DeviceGray"]
HFONT = TestFont()
VFONT = TestFont("V", True, -0.1)


def char_at(x, y, size=10.0, width=0.6, text="a", font=HFONT, matrix=None, disp=None, scaling=1.0, rise=0.0):
    """A horizontal (or vertical) char of ``size`` at ``(x, y)``."""
    m = matrix if matrix is not None else (size, 0.0, 0.0, size, x, y)
    fontsize = 1.0 if matrix is None else size
    if font.vertical and disp is None:
        disp = (None, 880)
    return LTChar(m, font, fontsize, scaling, rise, text, width, disp if font.vertical else 0, GRAY, None)


def page_of(objs, bbox=(0, 0, 612, 792), pageid=1):
    page = LTPage(pageid, bbox)
    for o in objs:
        page.add(o)
    return page


def word(x, y, text, size=10.0, width=0.6, gap=0.0):
    out = []
    for ch in text:
        out.append(char_at(x, y, size, width, ch))
        x += size * width + gap
    return out


def curve_objs(rng):
    out = []
    for _ in range(rng.randint(0, 3)):
        x, y = rng.uniform(0, 600), rng.uniform(0, 780)
        kind = rng.random()
        if kind < 0.3:
            out.append(LTLine(1.0, (x, y), (x + 50, y), True, False, False, (0.0,), None, [("m", (x, y)), ("l", (x + 50, y))], None))
        elif kind < 0.6:
            out.append(LTRect(0.5, (x, y, x + 20, y + 10), True, True, False, None, (1.0, 0.0, 0.0), None, ([3], 0)))
        elif kind < 0.8:
            out.append(LTCurve(2, [(x, y), (x + 5, y + 7), (x + 1, y + 3)], False, True, True))
        else:
            out.append(LTCurve(0, [(math.nan, y), (x, math.nan)], True))
    return out


def adversarial_pages(rng):
    """(name, page) of the generated layouts."""
    # margins hit exactly: char width 6 (size 10, width .6), char_margin 2:
    # a gap of 12 (exactly max(w) * char_margin) does not join, below joins
    for gap in (11.999, 12.0, 12.001, 0.6, 0.6000001, 0.5999999, 0.0, -3.0):
        objs = word(10, 700, "abcd", gap=gap) + word(10, 680, "efgh", gap=gap / 2)
        yield f"gap {gap}", page_of(objs)
    # line_overlap: overlap exactly half the min height
    for dy in (5.0, 4.999, 5.001, 10.0, 0.0, -10.0):
        objs = [char_at(10, 700), char_at(16, 700 + dy), char_at(22, 700 + 2 * dy, size=20.0)]
        yield f"overlap {dy}", page_of(objs)
    # line_margin: lines exactly line_margin * height apart, aligned or not
    for sep in (5.0, 4.999, 5.001, 15.0):
        for shift in (0.0, 4.99, 5.0, 7.5, 40.0):
            objs = word(10, 700, "line one") + word(10 + shift, 700 - 10 - sep, "line two") + word(10, 600, "far")
            yield f"line margin {sep} {shift}", page_of(objs)
    # vertical text, columns
    for col_gap in (5.0, 12.0, 30.0):
        objs = []
        for c in range(3):
            for r in range(5):
                objs.append(char_at(100 + c * (10 + col_gap), 700 - r * 10, font=VFONT, text="v"))
        yield f"vertical {col_gap}", page_of(objs)
    # two lines of different lengths aligned on one edge only, or on the
    # center only, or not at all (find_neighbors: one alignment suffices)
    for align, offset in (("start", 0), ("end", 4), ("center", 2), ("none", 1)):
        for vertical in (False, True):
            objs = []
            for r in range(6):
                objs.append(char_at(100, 700 - r * 10, font=VFONT, text="v") if vertical else char_at(10 + r * 6, 700))
            for r in range(2):
                objs.append(char_at(113, 700 - (r + offset) * 10, font=VFONT, text="w") if vertical
                            else char_at(10 + (r + offset) * 6, 687, text="b"))
            yield f"aligned {align} {'vertical' if vertical else 'horizontal'}", page_of(objs)
    # mixed horizontal and vertical, overlapping, duplicates
    for k in range(20):
        objs = []
        for _ in range(rng.randint(2, 40)):
            x, y = rng.choice([100.0, 106.0, 112.0, rng.uniform(0, 600)]), rng.choice([700.0, 690.0, rng.uniform(0, 780)])
            font = VFONT if rng.random() < 0.3 else HFONT
            objs.append(char_at(x, y, size=rng.choice([10.0, 10.0, 5.0, 20.0]), font=font, text=rng.choice("ab ")))
            if rng.random() < 0.2:
                objs.append(objs[-1])  # the same object twice
        rng.shuffle(objs)
        yield f"mixed {k}", page_of(objs + curve_objs(rng))
    # blank and zero-size chars
    objs = word(10, 700, "a  b") + [char_at(50, 700, size=0.0), char_at(60, 700, width=0.0, text=" "),
                                     char_at(70, 700, text="\t"), char_at(80, 700, text="")]
    objs += word(10, 650, "   ")
    yield "blank", page_of(objs)
    # grids of equal boxes: equal distances, the heap orders them by id()
    for n, step in ((3, 40.0), (5, 30.0), (8, 25.0), (12, 40.0)):
        objs = []
        for i in range(n):
            for j in range(n):
                objs += word(20 + i * step, 700 - j * step, "xy")
        yield f"grid {n} {step}", page_of(objs)
    # many boxes
    objs = []
    for i in range(300):
        objs += word(rng.uniform(0, 590), rng.uniform(0, 780), rng.choice(["a", "bb", "ccc"]))
    yield "many", page_of(objs)
    # NaN, infinite, huge and out-of-page coordinates
    specials = [math.nan, math.inf, -math.inf, 3e9, -3e9, 1e300, 2147483647.0, 2147483648.0]
    for k in range(25):
        objs = word(10, 700, "ok") + word(10, 600, "fine")
        for _ in range(rng.randint(1, 6)):
            m = [10.0, 0.0, 0.0, 10.0, rng.uniform(0, 600), rng.uniform(0, 780)]
            m[rng.choice([0, 3, 4, 5, 4, 5])] = rng.choice(specials)
            objs.append(char_at(0, 0, matrix=tuple(m), text="s"))
        rng.shuffle(objs)
        bbox = rng.choice([(0, 0, 612, 792), (0.0, 0.0, 612.0, 792.0), (-100.5, -50, 300, 400)])
        yield f"special {k}", page_of(objs, bbox)
    # chars outside the page
    objs = word(10, 700, "in") + word(700, 900, "out") + word(-50, -40, "neg") + word(605, 700, "edge")
    yield "outside", page_of(objs)
    # figures with chars, nested, and an image
    for k in range(6):
        fig = LTFigure("Fm1", (0, 0, 300, 300), (1, 0, 0, 1, 50.0, 60.0))
        for o in word(60, 100, "in fig") + word(60, 85, "second"):
            fig.add(o)
        inner = LTFigure("Fm2", (0, 0, 10, 10), (2.0, 0, 0, 2.0, 100.0, 200.0))
        for o in word(100, 200, "deep"):
            inner.add(o)
        if k % 2:
            inner.add(LTImage("Im1", PDFStream({"Width": 4, "Height": 2, "ColorSpace": LIT("DeviceRGB")}, b""),
                              (inner.x0, inner.y0, inner.x1, inner.y1)))
        fig.add(inner)
        objs = word(10, 700, "page text") + [fig] + curve_objs(rng)
        if k >= 3:
            objs = [fig] + curve_objs(rng)  # no char on the page itself
        yield f"figure {k}", page_of(objs)
    # random pages
    for k in range(60):
        objs = []
        for _ in range(rng.randint(1, 12)):
            x, y = rng.uniform(0, 550), rng.uniform(0, 780)
            size = rng.choice([8.0, 10.0, 12.0, 24.0])
            objs += word(x, y, rng.choice(["the", "quick brown", "fox", "a b  c", "x"]), size=size,
                         gap=rng.choice([0.0, 0.0, 1.0, 3.0, size * 1.3]))
        if rng.random() < 0.3:
            for _ in range(rng.randint(1, 8)):
                objs.append(char_at(rng.uniform(0, 550), rng.uniform(0, 780), font=VFONT, text="v"))
        if rng.random() < 0.3:
            rng.shuffle(objs)
        yield f"random {k}", page_of(objs + curve_objs(rng))
    yield "empty", page_of([])
    yield "curves only", page_of(curve_objs(rng) + curve_objs(rng))
    # objects tagged after their creation, as pdfplumber's aggregator does
    for k in range(30):
        fig = LTFigure("Fm1", (0, 0, 300, 300), (1, 0, 0, 1, 50.0, 60.0))
        for o in word(60, 100, "fig") + curve_objs(rng):
            fig.add(o)
        inner = LTFigure("Fm2", (0, 0, 10, 10), (2.0, 0, 0, 2.0, 100.0, 200.0))
        inner.add(LTImage("Im1", PDFStream({"Width": 4, "Height": 2}, b""), (inner.x0, inner.y0, inner.x1, inner.y1)))
        fig.add(inner)
        objs = word(10, 700, "tagged text") + word(10, 650, "more") + [fig] + curve_objs(rng)
        rng.shuffle(objs)
        tag_objects(rng, objs, rng.randint(1, 25))
        yield f"tagged {k}", page_of(objs)


#: Values pdfplumber's aggregator gives ``mcid`` (the ``MCID`` of the
#: marked-content properties: any object) and ``tag`` (a ``str``), or None.
ATTR_VALUES = [None, 0, 1, 7, -3, 2**70, True, 1.5, math.nan, "P", "Span", "", "café", b"x", LIT("Artifact"),
               [1, 2], {"MCID": 3}]


def tag_objects(rng, objs, n):
    """``n`` rounds of pdfplumber's ``tag_cur_item`` (``obj.mcid = ...``, then
    ``obj.tag = ...``) on objects of ``objs`` and of their figures, the same
    object possibly again; some rounds set one attribute, or ``tag`` first."""
    flat = []

    def walk(os):
        for o in os:
            flat.append(o)
            if isinstance(o, LTFigure):
                walk(o._objs)

    walk(objs)
    for _ in range(n):
        obj = rng.choice(flat)
        r = rng.random()
        names = ["mcid", "tag"] if r < 0.8 else (["tag"] if r < 0.9 else ["tag", "mcid"])
        for name in names:
            setattr(obj, name, rng.choice(ATTR_VALUES))


def chain_page(n):
    """``n`` one-char boxes in a row, the gaps growing: the groups chain, as
    deep as there are boxes."""
    objs = []
    x = 10.0
    for i in range(n):
        objs.append(char_at(x, 100.0, text="c"))
        x += 6.0 + 13.0 + i * 0.25
    return page_of(objs, (0, 0, x + 100.0, 300))


def gen_adversarial(rng):
    for name, page in adversarial_pages(rng):
        raw = dump_raw(page)
        check_dump(raw)
        runs, outs = analyze_runs(raw, CONFIGS)
        yield [name, raw, runs], outs
    # group chains around the recursion limit: find where it fails
    lo, hi = 50, 900
    while lo < hi:
        mid = (lo + hi + 1) // 2
        _, outs = analyze_runs(dump_raw(chain_page(mid)), [{}])
        if outs[0].startswith("!RecursionError"):
            hi = mid - 1
        else:
            lo = mid
    for n in range(lo - 2, lo + 3):
        raw = dump_raw(chain_page(n))
        runs, outs = analyze_runs(raw, [{}, {"boxes_flow": -1.0}])
        yield [f"chain {n}", raw, runs], outs


# -- objects --------------------------------------------------------------------


NUMS = [0, 1, -1, 2, 1000, 0.0, -0.0, 0.5, 1.0, -1.5, 12.0, 0.001, 1e300, math.inf, -math.inf, math.nan, 10**30]


def rand_num(rng):
    return rng.choice(NUMS) if rng.random() < 0.3 else rng.choice([rng.uniform(-50, 50), rng.randint(-20, 20)])


def dump_attrs(obj):
    return [obj.__dict__.get(k) if k != "ncs" else obj.ncs.name for k in obj.__dict__]


def call(f, *args):
    try:
        return f(*args)
    except Exception as e:  # the exception is the expected result
        return e


def gen_ltchar(rng):
    for _ in range(3000):
        matrix = tuple(rand_num(rng) for _ in range(6)) if rng.random() < 0.5 else (
            rng.uniform(-20, 20), rng.choice([0.0, 0, rng.uniform(-5, 5)]), rng.choice([0.0, 0, rng.uniform(-5, 5)]),
            rng.uniform(-20, 20), rng.uniform(0, 600), rng.uniform(0, 800))
        vertical = rng.random() < 0.3
        font = TestFont(rng.choice(["F1", b"F\xff", LIT("X")]), vertical, rand_num(rng))
        fontsize, scaling, rise, textwidth = (rand_num(rng) for _ in range(4))
        if rng.random() < 0.5:
            fontsize, scaling, rise = rng.uniform(1, 30), 1.0, 0.0
        r = rng.random()
        if r < 0.15:
            textdisp = rand_num(rng)
        elif r < 0.5:
            textdisp = (None, rand_num(rng))
        else:
            textdisp = (rand_num(rng), rand_num(rng))
        args = [matrix, [font.fontname, vertical, font.descent], fontsize, scaling, rise, "t", textwidth, textdisp]
        out = call(LTChar, matrix, font, fontsize, scaling, rise, "t", textwidth, textdisp, GRAY, None)
        yield args, out if isinstance(out, BaseException) else dump_raw(out)
    # vertical fonts: pdffont hands over (vx, vy) unchecked, from /W2 items
    # (any object) or `(vy, w) = resolve1(DW2)` (a dict key is a str); the
    # fontsize is a float, or the int 0 of a text state without Tf (ints that
    # repeat a sequence too, but not into an allocation Python would attempt)
    for _ in range(1500):
        vx, vy = rng.choice(DISP_ITEMS), rng.choice(DISP_ITEMS)
        if rng.random() < 0.3:
            vx = None
        fontsize = rng.choice([10.0, 0, 0.0, rng.uniform(1, 30), 2, -1, 2**62, 2**63, 10**30, math.nan])
        if fontsize == 2**62 and isinstance(vx, (bytes, str)) and len(vx) == 1:
            continue  # a 4 EiB allocation: MemoryError, depending on the machine
        textdisp = (vx, vy)
        font = TestFont("V", True, rand_num(rng))
        matrix = (1.0, 0.0, 0.0, 1.0, rng.uniform(0, 600), rng.uniform(0, 800))
        args = [matrix, [font.fontname, True, font.descent], fontsize, 1.0, 0.0, "t", 0.5, textdisp]
        out = call(LTChar, matrix, font, fontsize, 1.0, 0.0, "t", 0.5, textdisp, GRAY, None)
        yield args, out if isinstance(out, BaseException) else dump_raw(out)


DISP_ITEMS = [None, True, False, 0, 880, -1000, 1000, 0.5, -0.0, math.nan, math.inf, 1e308, 10**400, b"9", b"", b"ab",
              "880", "a", "", "ab", [1], [], [1, 2], {"a": 1}, LIT("x"), KWD(b"Tj"), PDFStream({}, b""),
              PDFObjRef(None, 3)]


#: Items of an unchecked /Matrix or /BBox. The ints that can repeat a
#: sequence are small, or beyond CPython's size checks (OverflowError,
#: MemoryError before any allocation; a bytes of length 1 is never repeated
#: 2**62 times): no repetition Python would try to allocate.
RAW_NUMS = [0, 1, -1, 2, 3, 7, 1000, -3, 2**62, 2**63, -2**63, 10**30, 0.0, -0.0, 0.5, 1.5, -2.25, 72.0, 1e308,
            math.inf, -math.inf, math.nan, True, False]
RAW_SEQS = [b"", b"ab", b"xyz", [], [1], [1, 2], [LIT("a"), 2.5]]
RAW_OTHERS = RAW_SEQS + [None, {}, {"a": 1}, LIT("x"), KWD(b"q"), PDFStream({}, b""), PDFObjRef(None, 3)]
#: CTMs: the page's (ints for the rotations, floats after a cm), and others.
CTMS = [(1, 0, 0, 1, 0, 0), (1.0, 0.0, 0.0, 1.0, 0.0, 0.0), (0, -1, 1, 0, -0.0, 612.0), (-1, 0, 0, -1, 612.0, 792.0),
        (2, 0, 0, 3, 5, 7), (0.5, 0.25, -0.25, 0.5, 10.5, 20.0), (1, 0, 0, 1, -36.0, -18.5), (0, 1, -1, 0, 792, 0)]
INT_CTMS = [m for m in CTMS if all(isinstance(x, int) for x in m[:4])]


def operands(m):
    """A matrix computed from unchecked items: numbers, and ``[type, len]``
    for a sequence."""
    return [x if isinstance(x, (int, float)) else [type(x).__name__, len(x)] for x in m]


def figure_obj(m1, m0, bbox):
    # PDFLayoutAnalyzer.begin_figure
    return dump_raw(LTFigure("Fm0", bbox, mult_matrix(m1, m0)))


def gen_figure_obj(rng):
    def item(p):
        return rng.choice(RAW_NUMS) if rng.random() < p else rng.choice(RAW_OTHERS)

    for _ in range(3000):
        m1 = [item(0.85) for _ in range(rng.choice([6] * 10 + [0, 4, 5, 7]))]
        bbox = [item(0.85) for _ in range(rng.choice([4] * 10 + [0, 3, 5]))]
        if rng.random() < 0.3:
            m0 = tuple(rand_num(rng) for _ in range(6))
        else:
            m0 = rng.choice(CTMS)
        yield ["mult_matrix_obj", m1, m0], call(lambda: operands(mult_matrix(m1, m0)))
        yield ["LTFigure_obj", m1, m0, bbox], call(figure_obj, m1, m0, bbox)
    # sequences that survive mult_matrix (repeated by the int CTM, added to
    # their own kind), then meet the box
    for _ in range(1000):
        kind = rng.choice([bytes, list])
        seqs = [s for s in RAW_SEQS if isinstance(s, kind)]
        m1 = [rng.choice(seqs) if rng.random() < 0.8 else item(1) for _ in range(4)] + [item(1), item(1)]
        m0 = rng.choice(INT_CTMS)
        bbox = [rng.choice([0, 1, 2, 5, -1, 2**62, 2**63, 0.5, True, b"ab", [1]]) for _ in range(4)]
        yield ["mult_matrix_obj", m1, m0], call(lambda: operands(mult_matrix(m1, m0)))
        yield ["LTFigure_obj", m1, m0, bbox], call(figure_obj, m1, m0, bbox)


SORT_VALS = [math.nan, math.inf, -math.inf, 0.0, -0.0, 0, 1, -1, 1.5, 2, 2.0, 3, 1e300, 2**53, 2**53 + 1, 2**70]


def gen_list_sort(rng):
    """``list.sort(key=...)`` as the layout calls it: keys are new objects
    (``-x``, ``(kind, -a, -b)``), NaN among them only in lists shorter than
    64 (the exact count_run + binarysort path), totally ordered keys in
    longer ones (the stable sort). Output: the sorted indices."""
    for _ in range(1500):
        n = rng.choice([2, 3, 4, 5, 8, 16, 31, 32, 33, 62, 63, rng.randint(2, 63), rng.randint(64, 200)])
        pool = SORT_VALS if n < 64 else SORT_VALS[1:]
        vals = [rng.choice(pool) for _ in range(n)]
        order = list(range(n))
        order.sort(key=lambda i: -vals[i])
        yield ["list_sort", vals], order
    for _ in range(800):
        n = rng.choice([2, 3, 5, 10, 63, rng.randint(2, 63), rng.randint(64, 150)])
        pool = SORT_VALS if n < 64 else SORT_VALS[1:]
        rows = [[rng.choice([0, 1]), rng.choice(pool), rng.choice(pool)] for _ in range(n)]
        order = list(range(n))
        order.sort(key=lambda i: (rows[i][0], -rows[i][1], -rows[i][2]))
        yield ["list_sort_tuple", rows], order


def gen_objects(rng):
    for _ in range(1000):
        pts = [(rand_num(rng), rand_num(rng)) for _ in range(rng.randint(0, 4))]
        lw = rand_num(rng)
        yield ["LTCurve", lw, pts], call(lambda: dump_raw(LTCurve(lw, pts, True, False, True, (0.5,), "P1", [("h",)], ([1], 2))))
        if len(pts) >= 2:
            yield ["LTLine", lw, pts[:2]], call(lambda: dump_raw(LTLine(lw, pts[0], pts[1])))
        bbox = tuple(rand_num(rng) for _ in range(4))
        yield ["LTRect", lw, bbox], call(lambda: dump_raw(LTRect(lw, bbox)))
        matrix = tuple(rand_num(rng) for _ in range(6))
        yield ["LTFigure", bbox, matrix], call(lambda: dump_raw(LTFigure("F", bbox, matrix)))
        rotate = rng.choice([0, 90, 1.5])
        yield ["LTPage", bbox, rotate], call(lambda: dump_raw(LTPage(3, bbox, rotate)))
    attrs_choices = [{}, {"W": 5, "H": 7}, {"Width": 5.5, "Height": LIT("x"), "W": 1}, {"IM": True, "BPC": 8},
                     {"ImageMask": False, "BitsPerComponent": 1, "CS": LIT("DeviceGray")},
                     {"ColorSpace": [LIT("Indexed"), LIT("DeviceRGB"), 255, b"x"]}, {"CS": [], "ColorSpace": 3}]
    for attrs in attrs_choices:
        bbox = (1.0, 2.0, 3.0, 4.5)
        yield ["LTImage", attrs, bbox], dump_raw(LTImage("Im0", PDFStream(dict(attrs), b""), bbox))
    for bf in [None, -1, 1, 0, -1.0, 1.0, 1.0000001, -2, 0.5, math.nan, math.inf, True, False, 10**30]:
        yield ["LAParams", bf], call(lambda: LAParams(boxes_flow=bf).boxes_flow)
    for _ in range(2000):
        a = tuple(rand_num(rng) for _ in range(4))
        b = tuple(rand_num(rng) for _ in range(4))
        c1, c2 = LTComponent(a), LTComponent(b)
        yield ["component", a, b], [c1.is_empty(), c1.is_hoverlap(c2), call(c1.hdistance, c2), call(c1.hoverlap, c2),
                                    c1.is_voverlap(c2), call(c1.vdistance, c2), call(c1.voverlap, c2)]
    for _ in range(300):
        pts = [(rand_num(rng), rand_num(rng)) for _ in range(rng.randint(0, 4))]
        yield ["get_pts", pts], call(LTCurve(1, pts).get_pts)
    # the figure of a Form XObject from its unchecked /Matrix and /BBox
    # (pdfinterp.do_Do -> PDFLayoutAnalyzer.begin_figure)
    yield from gen_figure_obj(rng)
    yield from gen_list_sort(rng)
    # attributes set after creation (pdfplumber's tag_cur_item): a new one
    # comes last in __dict__, one set again keeps its place
    def fresh():
        fig = LTFigure("Fm0", (0, 0, 10, 10), (1, 0, 0, 1, 5.0, 5.0))
        fig.add(char_at(6.0, 6.0))
        return rng.choice([lambda: char_at(1.0, 2.0), lambda: LTCurve(1, [(0, 0), (1, 2)]),
                           lambda: LTLine(1, (0, 0), (1, 1)), lambda: LTRect(1, (0, 0, 1, 1)),
                           lambda: LTImage("I", PDFStream({"W": 1}, b""), (0, 0, 1, 1)), lambda: fig])()

    for _ in range(400):
        obj = fresh()
        raw = dump_raw(obj)
        ops = []
        for _ in range(rng.randint(1, 6)):
            name, value = rng.choice(["mcid", "tag"]), rng.choice(ATTR_VALUES)
            setattr(obj, name, value)
            ops.append([name, value])
        yield ["setattr", raw, ops], [dump_raw(obj), list(obj.__dict__)]
    # the __dict__ keys of every class, in the order pdfplumber reads them
    stream = PDFStream({}, b"")
    samples = [char_at(1.0, 2.0), LTCurve(1, [(0, 0)]), LTLine(1, (0, 0), (1, 1)), LTRect(1, (0, 0, 1, 1)),
               LTImage("I", stream, (0, 0, 1, 1)), LTFigure("F", (0, 0, 1, 1), (1, 0, 0, 1, 0, 0)), LTPage(1, (0, 0, 1, 1)),
               LTAnno(" "), LTTextLineHorizontal(0.1), LTTextLineVertical(0.1), LTTextBoxHorizontal(), LTTextBoxVertical(),
               LTTextGroupLRTB([]), LTTextGroupTBRL([])]
    for obj in samples:
        yield ["dict_keys", type(obj).__name__], list(obj.__dict__)
    for box in (LTTextBoxHorizontal(), LTTextBoxVertical()):
        yield ["writing_mode", type(box).__name__], box.get_writing_mode()


def gen_textline(rng):
    for _ in range(1500):
        vertical = rng.random() < 0.4
        wm = rng.choice([0.1, 0, 0.0, 1, math.nan, -0.5, 2.0])
        chars = []
        for _ in range(rng.randint(1, 8)):
            chars.append(char_at(rng.choice([10.0, 16.0, 30.0, rng.uniform(0, 100)]),
                                 rng.choice([700.0, 690.0, rng.uniform(600, 800)]),
                                 size=rng.choice([10.0, 5.0]), font=VFONT if vertical else HFONT,
                                 text=rng.choice(["a", " ", "　", "b"])))
        raws = [dump_raw(c) for c in chars]
        line = LTTextLineVertical(wm) if vertical else LTTextLineHorizontal(wm)
        for c in chars:
            line.add(c)
        index = {id(c): i for i, c in enumerate(chars)}
        page = LTPage(1, (0, 0, 1, 1))
        page._objs = [line]
        tree = dump_tree(page, index)
        yield [vertical, wm, raws], [canon(tree), line.is_empty()]


GENERATORS = {
    "ltchar": gen_ltchar,
    "objects": gen_objects,
    "textline": gen_textline,
    "adversarial": gen_adversarial,
}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--max-pages", type=int, default=3)
    ap.add_argument("-j", "--jobs", type=int, default=3)
    ap.add_argument("--no-real", action="store_true", help="skip the real corpus (for a quick run)")
    args = ap.parse_args()
    require_oracle_python()
    sys.setrecursionlimit(1000)
    counts = {name: write_cases("layout", name, gen(random.Random(f"20261015:{name}"))) for name, gen in GENERATORS.items()}
    if not args.no_real:
        counts["real"] = write_cases("layout", "real", gen_real(args.max_pages, min(args.jobs, 3)))
    write_meta("layout", counts=counts, max_pages=args.max_pages)
    print(f"wrote {counts}")


if __name__ == "__main__":
    main()
