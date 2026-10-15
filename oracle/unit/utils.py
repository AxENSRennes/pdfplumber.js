"""Differential cases for crates/pdfminer/src/{utils,casting,data_structures,pdfcolor}.rs
and crates/pdfplumber/src/ctm.rs, computed by pdfminer and pdfplumber.

Writes typed JSON lines (docs/verification.md) to
``<cache>/unit/utils/<fingerprint>/<name>.jsonl`` (see
``oracle/unit/_common.py``), one case per line, ``{"in": [...], "out": ...}``;
the Rust replay is crates/pdftrace/tests/utils_unit.rs. Inputs that are PDF
objects use the typed form; a ``{"ref": n}`` resolves through the case's
table of objects (``TableDoc`` of ``oracle/unit/pdftypes.py``: a missing id
raises ``PDFObjectNotFound``, object 13 ``PDFSyntaxError``).

- ``matrix``: ``mult_matrix``, ``translate_matrix``, ``apply_matrix_pt``,
  ``apply_matrix_rect``, ``apply_matrix_norm`` on matrices and points of
  ints, floats, huge ints, infinities, NaNs and signed zeros.
- ``geometry``: ``get_bound`` (no point: ints; NaN and huge coordinates),
  ``drange``, ``bbox2str``/``matrix2str`` (rounding ties, ``-0.0``,
  ``inf``, huge ints), ``parse_rect`` (wrong counts, ``float()`` errors that
  are and are not caught, ``str`` items such as the keys of a dict box,
  which ``float()`` reads with Unicode digits and spaces).
- ``iter``: ``uniq``, ``fsplit``, ``pick`` on numbers (``1 == 1.0 ==
  True``, NaN scores).
- ``numbers``: ``isnumber``, ``nunpack`` (random bytes, and 8 bytes on
  both sides of the int64 bound), ``format_int_roman``,
  ``format_int_alpha`` (assertions, big ints).
- ``decode_text``: every single byte, random strings, and UTF-16BE strings
  after the BOM (odd lengths, lone and paired surrogates).
- ``casting``: ``safe_int``, ``safe_float`` (``int(inf)`` raises),
  ``safe_matrix``, ``safe_rgb``, ``safe_cmyk``, ``safe_rect``,
  ``safe_rect_list`` (lists, dict keys, bytes, streams) on PDF objects.
- ``plane``: sequences of ``Plane`` operations (``add``, ``remove``,
  ``find``, iteration, ``len`` and ``bool``, ``in``) on boxes of random sizes, some on
  cell boundaries, some outside the plane, with several grid sizes.
- ``numbertree``: ``NumberTree(obj).values`` (and ``nums``/``kids``/
  ``limits``) on the page label and structure parent trees of the corpus
  set ``upstream`` and on generated malformed trees (odd ``Nums``,
  non-int keys, references, cycles), plus chains of ``Kids`` around the
  Python frame limit, called at a measured depth (``_common.at_depth``).
- ``pdfcolor``: ``PREDEFINED_COLORSPACE`` (names, components, ``repr``).
- ``ctm``: the ``CTM`` properties on matrices of ints, floats and special
  values.

Usage: PDFPLUMBER_ROOT=<main checkout> <root>/.venv/bin/python -m oracle.unit.utils
"""

from __future__ import annotations

import math
import random

from pdfminer import utils
from pdfminer.casting import safe_cmyk, safe_float, safe_int, safe_matrix, safe_rect, safe_rect_list, safe_rgb
from pdfminer.data_structures import NumberTree
from pdfminer.pdfcolor import PREDEFINED_COLORSPACE
from pdfminer.pdfexceptions import PDFObjectNotFound
from pdfminer.pdfpage import PDFPage
from pdfminer.pdfparser import PDFSyntaxError
from pdfminer.pdftypes import PDFObjRef, PDFStream, resolve1
from pdfminer.psparser import KWD, LIT
from pdfplumber.ctm import CTM

from ..corpus import load_set
from ..stages._common import open_document
from ._common import at_depth, depth_here, require_oracle_python, write_cases, write_meta

BROKEN_OBJID = 13


class TableDoc:
    """A document whose getobj() reads a table (as in oracle/unit/pdftypes.py)."""

    def __init__(self):
        self.table = {}

    def getobj(self, objid):
        if objid in self.table:
            return self.table[objid]
        if objid == BROKEN_OBJID:
            raise PDFSyntaxError(f"broken {objid}")
        raise PDFObjectNotFound(objid)


def call(f, *args):
    try:
        return f(*args)
    except Exception as e:  # the exception is the expected result
        return e


NUMS = [0, 1, -1, 2, 3, 7, 100, 2**31 - 1, -(2**31), 2**53 + 1, 2**63, 10**30, -(10**400), True, False,
        0.0, -0.0, 0.5, -0.5, 1.0, 1.5, 0.1, 2.5, 1e16, 1e-5, 1e308, -1e308, 5e-324, math.inf, -math.inf, math.nan]


def rand_num(rng):
    r = rng.random()
    if r < 0.35:
        return rng.choice(NUMS)
    if r < 0.6:
        return rng.randint(-1000, 1000)
    if r < 0.85:
        return rng.uniform(-1000, 1000)
    return float(rng.randint(-50, 50)) / 8


def no_bool(x):
    """Matrices and points of the port are numbers: a bool becomes its int."""
    return int(x) if isinstance(x, bool) else x


def rand_matrix(rng):
    if rng.random() < 0.2:
        return (1, 0, 0, 1, 0, 0)
    return tuple(no_bool(rand_num(rng)) for _ in range(6))


def gen_matrix(rng):
    for _ in range(3000):
        m1, m0 = rand_matrix(rng), rand_matrix(rng)
        v = (no_bool(rand_num(rng)), no_bool(rand_num(rng)))
        rect = tuple(no_bool(rand_num(rng)) for _ in range(4))
        yield ["mult_matrix", m1, m0], call(utils.mult_matrix, m1, m0)
        yield ["translate_matrix", m1, v], call(utils.translate_matrix, m1, v)
        yield ["apply_matrix_pt", m1, v], call(utils.apply_matrix_pt, m1, v)
        yield ["apply_matrix_rect", m1, rect], call(utils.apply_matrix_rect, m1, rect)
        yield ["apply_matrix_norm", m1, v], call(utils.apply_matrix_norm, m1, v)


def gen_geometry(rng):
    for _ in range(1500):
        pts = [(no_bool(rand_num(rng)), no_bool(rand_num(rng))) for _ in range(rng.randint(0, 5))]
        yield ["get_bound", pts], call(utils.get_bound, pts)
    grid = [0, 1, 49, 50, 51, 99.9, 100, -0.5, -50, -50.5, 1e18, 2.0**63, math.inf, math.nan, 10**30]
    for _ in range(1500):
        v0 = no_bool(rng.choice(grid) if rng.random() < 0.5 else rand_num(rng))
        v1 = no_bool(rng.choice(grid) if rng.random() < 0.5 else rand_num(rng))
        d = rng.choice([1, 7, 50, 64])
        out = call(utils.drange, v0, v1, d)
        if isinstance(out, range):
            if abs(out.start) >= 2**63 or abs(out.stop) >= 2**63:
                continue  # the port raises instead of iterating for ages
            out = (out.start, out.stop)
        yield ["drange", v0, v1, d], out
    ties = [0.125, 0.375, 2.5, 0.0625, 1.0005, 0.0005, 2.675, -0.125, -0.0, 0.0, 1e300, 123456789.0125]
    for _ in range(1000):
        bbox = tuple(no_bool(rng.choice(ties) if rng.random() < 0.4 else rand_num(rng)) for _ in range(4))
        yield ["bbox2str", bbox], call(utils.bbox2str, bbox)
        m = tuple(no_bool(rng.choice(ties) if rng.random() < 0.4 else rand_num(rng)) for _ in range(6))
        yield ["matrix2str", m], call(utils.matrix2str, m)
    items = [1, 2.5, -3, True, None, b"7", b" 1.5 ", b"x", b"1e400", b"nan", LIT("F"), [1], {"A": 1}, 10**400,
             math.inf, PDFStream({}, b"")]
    for _ in range(1500):
        seq = [rng.choice(items) for _ in range(rng.choice([3, 4, 4, 4, 5]))]
        yield ["parse_rect", seq], call(utils.parse_rect, iter(seq))
    # str items: pdfpage iterates a resolved /MediaBox of any type, and a
    # dict yields its str keys, which float() reads as str (Unicode digits
    # and spaces, underscores), unlike bytes
    strs = ["0", "0.5", "612", "792", "-3", "١٢", " 1_0 ", " 1", "　-2.5　", "nan", "-inf",
            "Infinity", "iNf", "1e400", "-1e400", "", " ", "x", "0x10", "1__0", "_1", "1_", "٣.٥",
            "१२३", "\U0001d7d9\U0001d7da", "1\x00", "+.5", "5.", "1e5", "²", "Ⅷ", "\xa012\x85"]
    for _ in range(800):
        seq = [rng.choice(strs) if rng.random() < 0.6 else rng.choice(items)
               for _ in range(rng.choice([3, 4, 4, 4, 5]))]
        yield ["parse_rect", seq], call(utils.parse_rect, iter(seq))
    for _ in range(200):
        d = {rng.choice(strs): 0 for _ in range(rng.choice([3, 4, 4, 4, 5, 6]))}
        yield ["parse_rect", list(d)], call(lambda: utils.parse_rect(resolve1(x) for x in resolve1(d)))


def gen_iter(rng):
    for _ in range(800):
        # no NaN: uniq() finds a NaN by identity, which a number cannot carry
        seq = [x for x in (rand_num(rng) for _ in range(rng.randint(0, 8))) if x == x]
        if rng.random() < 0.5:
            seq += [rng.choice(seq or [1]) for _ in range(3)]
        yield ["uniq", seq], [x for x in utils.uniq(seq) if True]
        yield ["fsplit", seq], utils.fsplit(lambda x: x > 0, seq)
        yield ["pick", seq], utils.pick(seq, lambda x: -x if isinstance(x, int) else x * 2, "none")


def gen_numbers(rng):
    objs = [None, True, False, 0, 1, -5, 2**70, 0.0, 1.5, math.nan, b"1", LIT("A"), KWD(b"x"), [1], {}, PDFStream({}, b"")]
    for x in objs:
        yield ["isnumber", x], utils.isnumber(x)
    for _ in range(600):
        s = bytes(rng.randrange(256) for _ in range(rng.randint(0, 20)))
        default = rng.choice([0, 1, 5])
        yield ["nunpack", s, default], utils.nunpack(s, default)
    # 8 bytes: an int64 when the high bit is clear, beyond it otherwise
    for s in [b"\x7f" + b"\xff" * 7, b"\x80" + b"\x00" * 7, b"\x80" + b"\xff" * 7, b"\xff" * 8, b"\x00" * 8]:
        yield ["nunpack", s, 0], utils.nunpack(s, 0)
    for v in list(range(-3, 4010, 1)) + [2**70, -(2**70)]:
        yield ["format_int_roman", v], call(utils.format_int_roman, v)
    for v in list(range(-3, 2000)) + [26**2, 26**2 + 26, 26**3 + 26**2 + 26, 2**64, 10**40]:
        yield ["format_int_alpha", v], call(utils.format_int_alpha, v)


def gen_decode_text(rng):
    for c in range(256):
        yield [bytes([c])], utils.decode_text(bytes([c]))
    yield [bytes(range(256))], utils.decode_text(bytes(range(256)))
    for _ in range(2000):
        n = rng.randint(0, 12)
        r = rng.random()
        if r < 0.3:
            s = bytes(rng.randrange(256) for _ in range(n))
        else:
            units = []
            for _ in range(n):
                k = rng.random()
                if k < 0.4:
                    units.append(rng.randrange(0x20, 0x7F))
                elif k < 0.6:
                    units.append(rng.randrange(0xD800, 0xDC00))
                elif k < 0.8:
                    units.append(rng.randrange(0xDC00, 0xE000))
                else:
                    units.append(rng.randrange(0x10000))
            s = b"\xfe\xff" + b"".join(u.to_bytes(2, "big") for u in units)
            if rng.random() < 0.3:
                s += bytes([rng.randrange(256)])
            if rng.random() < 0.1:
                s = b"\xff\xfe" + s[2:]
        yield [s], utils.decode_text(s)
    for s in [b"", b"\xfe", b"\xfe\xff", b"\xfe\xff\x00", b"\xfe\xff\xd8\x00", b"\xfe\xff\xd8\x00\xdc\x00",
              b"\xfe\xff\xdc\x00\xd8\x00", b"\xfe\xff\xd8\x00\x00A", b"\xfe\xff\xd8\x3d\xde\x00\x00"]:
        yield [s], utils.decode_text(s)


CAST_OBJS = [None, True, False, 0, 7, -3, 2**63, 10**400, -(10**400), 0.0, -0.0, 1.5, math.inf, -math.inf, math.nan,
             b"", b"12", b" 12 ", b"+5", b"-0", b"1.5", b"1e400", b"-1e400", b"nan", b"inf", b"-Infinity", b"1_000",
             b"0x10", b"1.5e3", b"\xff", b"12abc", b"\t3\n", b"9" * 5000, LIT("A"), LIT("12"), KWD(b"Tj"), [1, 2],
             [], {"A": 1}, PDFStream({"A": 1}, b"x"), PDFObjRef(None, 5)]


def gen_casting(rng):
    for x in CAST_OBJS:
        yield ["safe_int", x], call(safe_int, x)
        yield ["safe_float", x], call(safe_float, x)
    for _ in range(1500):
        args = [rng.choice(CAST_OBJS) if rng.random() < 0.3 else rand_num(rng) for _ in range(6)]
        yield ["safe_matrix", *args], safe_matrix(*args)
        yield ["safe_rgb", *args[:3]], safe_rgb(*args[:3])
        yield ["safe_cmyk", *args[:4]], safe_cmyk(*args[:4])
        yield ["safe_rect", *args[:4]], safe_rect(*args[:4])
    rect_values = [[1, 2, 3, 4], [1, 2, 3], [1, 2, 3, 4, 5], [1.5, b"2", True, 4], [1, None, 3, 4], [10**400, 1, 2, 3],
                   {"1": 0, "2": 0, "3": 0, "4.5": 0}, {"1": 0, "B": 0, "3": 0, "4": 0}, {" 1 ": 0, "2": 0, "3": 0},
                   b"abcd", b"abcdef", b"ab", PDFStream({"A": 1}, b"x"), None, 5, 1.5, LIT("A"), KWD(b"x"),
                   PDFObjRef(None, 3), [b"1", b"2", b"3", b"inf"], [math.nan, 1, 2, 3], {}, []]
    for v in rect_values:
        yield ["safe_rect_list", v], call(safe_rect_list, v)


class Box:
    """An object of a plane: a box with identity."""

    def __init__(self, i, bbox):
        self.i = i
        (self.x0, self.y0, self.x1, self.y1) = bbox


def rand_box(rng, size):
    edges = [0, 50, 100, 150, 49.999, 50.0001, -10, size, size + 10]
    def coord():
        if rng.random() < 0.3:
            return rng.choice(edges)
        if rng.random() < 0.05:
            return rng.choice([math.inf, -math.inf, math.nan])
        return rng.uniform(-20, size + 20)
    x0, x1 = sorted([coord(), coord()]) if rng.random() < 0.9 else (coord(), coord())
    y0, y1 = sorted([coord(), coord()]) if rng.random() < 0.9 else (coord(), coord())
    return (x0, y0, x1, y1)


def plane_job(rng):
    size = rng.choice([100, 200, 333.5, 612])
    bbox = (0, 0, size, size) if rng.random() < 0.5 else (-5.5, -3.25, size, size * 1.5)
    gridsize = rng.choice([50, 50, 10, 7, 64])
    ops = []
    boxes = []
    live = []
    for _ in range(rng.randint(1, 40)):
        r = rng.random()
        if r < 0.45 or not live:
            b = rand_box(rng, size)
            ops.append(["add", len(boxes), b])
            boxes.append(b)
            live.append(len(boxes) - 1)
        elif r < 0.6:
            i = rng.choice(live)
            live.remove(i)
            ops.append(["remove", i])
        elif r < 0.85:
            ops.append(["find", rand_box(rng, size)])
        elif r < 0.9:
            ops.append(["iter"])
        elif r < 0.95:
            ops.append(["len"])
        else:
            ops.append(["contains", rng.randrange(len(boxes))])
    return bbox, gridsize, ops


def run_plane(bbox, gridsize, ops):
    plane = utils.Plane(bbox, gridsize)
    objs = {}
    out = []
    for op in ops:
        try:
            if op[0] == "add":
                objs[op[1]] = Box(op[1], op[2])
                plane.add(objs[op[1]])
                out.append(None)
            elif op[0] == "remove":
                plane.remove(objs[op[1]])
                out.append(None)
            elif op[0] == "find":
                out.append([o.i for o in plane.find(op[1])])
            elif op[0] == "iter":
                out.append([o.i for o in plane])
            elif op[0] == "len":
                out.append([len(plane), bool(plane)])
            else:
                out.append(objs[op[1]] in plane)
        except Exception as e:  # the exception is the expected result; the job stops
            out.append(e)
            break
    return out


def gen_plane(rng):
    for _ in range(1500):
        bbox, gridsize, ops = plane_job(rng)
        yield [bbox, gridsize, ops], run_plane(bbox, gridsize, ops)
    # a plane of inf/nan bounds
    for bbox in [(0, 0, math.inf, 10), (math.nan, 0, 10, 10), (0, 0, 10**30, 10), (-math.inf, -math.inf, 5, 5)]:
        ops = [["add", 0, (1, 1, 2, 2)], ["find", (0, 0, 3, 3)], ["iter"]]
        yield [bbox, 50, ops], run_plane(bbox, 50, ops)
    # len and bool of an empty plane (the random jobs add a box whenever
    # none is left)
    ops = [["len"], ["add", 0, (1, 1, 2, 2)], ["len"], ["remove", 0], ["len"], ["iter"]]
    yield [(0, 0, 100, 100), 50, ops], run_plane((0, 0, 100, 100), 50, ops)


# -- NumberTree -------------------------------------------------------------------


def tree_table(root, doc_getobj):
    """The objects a NumberTree reads: the nodes, Nums/Kids/Limits and keys
    that are references (values are not resolved by NumberTree)."""
    table = {}

    def fetch(x):
        while isinstance(x, PDFObjRef):
            if x.objid in table:
                x = table[x.objid]
                continue
            try:
                v = doc_getobj(x.objid)
            except Exception:
                return None
            table[x.objid] = v
            x = v
        return x

    stack = [root]
    seen = set()
    while stack:
        node = fetch(stack.pop())
        if not isinstance(node, dict) or id(node) in seen:
            continue
        seen.add(id(node))
        for key in ("Nums", "Kids", "Limits"):
            if key in node:
                lst = fetch(node[key])
                if isinstance(lst, list):
                    if key == "Kids":
                        stack.extend(lst)
                    elif key == "Nums":
                        for k in lst[::2]:
                            fetch(k)
    return table


def numbertree_case(root, table, depth_fn=None):
    doc = TableDoc()
    doc.table.update(table)

    def bind(x):
        # references in the case's objects point to this document
        if isinstance(x, PDFObjRef):
            return PDFObjRef(doc, x.objid)
        if isinstance(x, list):
            return [bind(v) for v in x]
        if isinstance(x, dict):
            return {k: bind(v) for k, v in x.items()}
        return x

    for k in list(doc.table):
        doc.table[k] = bind(doc.table[k])
    root_b = bind(root)

    def run():
        tree = NumberTree(root_b)
        return [tree.nums, tree.kids, tree.limits, tree.values]

    depth, out = at_depth(run)
    return depth, out


def spec_table(table):
    return [[k, unbind(v)] for k, v in sorted(table.items())]


def unbind(x):
    if isinstance(x, PDFObjRef):
        return PDFObjRef(None, x.objid)
    if isinstance(x, list):
        return [unbind(v) for v in x]
    if isinstance(x, dict):
        return {k: unbind(v) for k, v in x.items()}
    return x


def real_trees():
    """(name, root, table) of the page label trees and structure parent
    trees of the corpus set ``upstream``."""
    for f in load_set("upstream"):
        try:
            with open_document(str(f.path), f.password) as doc:
                catalog = doc.catalog
                roots = []
                if "PageLabels" in catalog:
                    roots.append(("PageLabels", catalog["PageLabels"]))
                st = resolve1(catalog.get("StructTreeRoot"))
                if isinstance(st, dict) and "ParentTree" in st:
                    roots.append(("ParentTree", st["ParentTree"]))
                for name, root in roots:
                    table = tree_table(root, doc.getobj)
                    yield f"{f.rel}:{name}", unbind(root), {k: unbind(v) for k, v in table.items()}
        except Exception:
            continue


def gen_numbertree(rng):
    for name, root, table in real_trees():
        depth, out = numbertree_case(root, table)
        yield [name, depth, root, spec_table(table)], out
    keys = [0, 1, 5, -2, 2**40, True, False, 1.5, b"3", None, LIT("K"), PDFObjRef(None, 30), PDFObjRef(None, 99),
            PDFObjRef(None, BROKEN_OBJID)]
    for _ in range(1500):
        table = {30: rng.choice([4, 7.5, True, b"x"])}
        n_nodes = rng.randint(1, 6)
        for i in range(n_nodes):
            node = {}
            if rng.random() < 0.7:
                nums = []
                for _ in range(rng.randint(0, 6)):
                    nums.append(rng.choice(keys) if rng.random() < 0.4 else rng.randint(-5, 30))
                    nums.append(rng.choice([LIT("V"), PDFObjRef(None, 100 + i), b"v", 3]))
                if rng.random() < 0.2:
                    nums.append(7)  # odd length
                node["Nums"] = nums if rng.random() < 0.8 else PDFObjRef(None, 200 + i)
                table[200 + i] = nums
            if rng.random() < 0.5 and i + 1 < n_nodes:
                kids = [PDFObjRef(None, 1 + j) for j in range(i + 1, n_nodes) if rng.random() < 0.6]
                if rng.random() < 0.1:
                    kids.append(PDFObjRef(None, 1 + i))  # a cycle
                if rng.random() < 0.1:
                    kids.append(rng.choice([5, None, PDFObjRef(None, 77), PDFObjRef(None, BROKEN_OBJID)]))
                node["Kids"] = kids if rng.random() < 0.9 else rng.choice([5, LIT("X"), {}])
            if rng.random() < 0.3:
                node["Limits"] = rng.choice([[0, 9], PDFObjRef(None, 300), 4])
                table[300] = [1, 2]
            table[1 + i] = node
        root = PDFObjRef(None, 1) if rng.random() < 0.9 else rng.choice([None, 5, [1], table[1]])
        depth, out = numbertree_case(root, table)
        yield ["generated", depth, root, spec_table(table)], out
    # chains of Kids around the frame limit (each level: _parse, then
    # NumberTree.__init__ -> dict_value -> resolve1 -> PDFObjRef.resolve ->
    # getobj while it is built)
    py = depth_here()["py"] + 1
    for n in list(range(max(1, (1000 - py) // 2 - 6), (1000 - py) // 2 + 4)) + list(range(1000 - py - 8, 1000 - py + 2)):
        for leaf in ([3, LIT("a"), 1, LIT("b")], []):
            table = {}
            for i in range(1, n + 1):
                table[i] = {"Kids": [PDFObjRef(None, i + 1)]} if i < n else {"Nums": leaf}
            depth, out = numbertree_case(PDFObjRef(None, 1), table)
            yield ["chain", depth, PDFObjRef(None, 1), spec_table(table)], out
    # a node that is its own kid
    table = {1: {"Kids": [PDFObjRef(None, 1)], "Nums": [1, 2]}}
    depth, out = numbertree_case(PDFObjRef(None, 1), table)
    yield ["cycle", depth, PDFObjRef(None, 1), spec_table(table)], out


def gen_pdfcolor(rng):
    for name, cs in PREDEFINED_COLORSPACE.items():
        yield [name], [cs.name, cs.ncomponents, repr(cs)]


def gen_ctm(rng):
    for _ in range(3000):
        m = rand_matrix(rng)
        if rng.random() < 0.3:
            m = tuple(rng.choice([0, 1, -1, 0.0, -0.0, math.inf, -math.inf, math.nan, 1e200, 10**400, 2]) for _ in range(6))
        ctm = CTM(*m)
        yield [m], [call(lambda: ctm.scale_x), call(lambda: ctm.scale_y), call(lambda: ctm.skew_x),
                    call(lambda: ctm.skew_y), ctm.translation_x, ctm.translation_y]


GENERATORS = {
    "matrix": gen_matrix,
    "geometry": gen_geometry,
    "iter": gen_iter,
    "numbers": gen_numbers,
    "decode_text": gen_decode_text,
    "casting": gen_casting,
    "plane": gen_plane,
    "numbertree": gen_numbertree,
    "pdfcolor": gen_pdfcolor,
    "ctm": gen_ctm,
}


def main():
    require_oracle_python()
    counts = {name: write_cases("utils", name, gen(random.Random(f"20261015:{name}"))) for name, gen in GENERATORS.items()}
    write_meta("utils", counts=counts)
    print(f"wrote {counts}")


if __name__ == "__main__":
    main()
