"""Differential cases for crates/pdfminer/src/pdftypes.rs (and the parts of
psparser.py it relies on), computed by pdfminer.

Writes typed JSON lines (docs/verification.md) to
``<cache>/unit/pdftypes/<name>.jsonl`` (see ``oracle/unit/_common.py``), one
case per line, ``{"in": [...], "out": ...}``; the Rust replay is
crates/pdftrace/tests/pdftypes_unit.rs. In the inputs, ``{"ref": n}`` is a
``PDFObjRef`` to a document whose objects are given by the case (a list of
``[objid, object]`` pairs; a missing id raises ``PDFObjectNotFound``, except
object 13, ``BROKEN_OBJID``, which raises ``PDFSyntaxError`` as a broken
object does in ``PDFDocument.getobj``).

- ``objects``: ``repr``, ``literal_name``, ``keyword_name``, ``bool``, the
  ``*_value`` accessors, ``"Predictor" in x`` and ``x[0]`` of random objects.
- ``eq``: ``a == b`` for pairs of related objects; references and streams
  compare by identity (``"same"``: the object itself; ``"shallow"``:
  ``list(x)``/``dict(x)``, a new container sharing the elements).
- ``resolve``: ``resolve1``, ``resolve_all`` (which updates dicts in place:
  the case re-reads the object afterwards) and the accessors through
  references.
- ``streams``: ``PDFStream`` ``repr``, ``get_filters``, ``get_data`` (filters
  that need no unported decoder), ``s.data`` and ``__getitem__``;
  ``DecodeParms`` may be a stream (an indirect object).
- ``decipher``: ``decipher_all`` (lists rebuilt, dicts updated in place) and
  ``PDFStream.get_data`` with a decipher callback, the test callback
  ``test_decipher`` below (the Rust test implements the same function).
- ``refs``: ``PDFObjRef.resolve`` without a document (``AssertionError``)
  and with one (a missing or broken object), and ``doc``.
- ``containers``: sequences of ``list``/``dict`` operations (append, item
  assignment, pop, keys, copy, identity), the Python behavior ``PDFList`` and
  ``PDFDict`` reproduce.
- ``uint``: ``uint_value(x, n_bits)`` (two's complement, a ``True`` kept).
- ``deep``: objects nested close to Python's recursion limits, called at a
  measured depth (``_common.at_depth``; the Rust test starts from the same
  depth): ``repr`` and ``literal_name`` (``str``) around the C recursion
  limit, for every kind of innermost object, and ``resolve_all`` and
  ``decipher_all`` around the Python frame limit, and on self-referencing
  lists (``RecursionError``). Deep objects are given
  as ``["deep", shape, n, leaf]`` (``deep_object``), results of
  ``resolve_all``/``decipher_all`` as the SHA-256 of their ``canon``.
  ``literal_name``'s ``str(x)`` has run before (``oracle.depth``,
  ``warm_call_sites``): the cost of every execution but a process's first.
- ``deep_first``: ``literal_name`` of the same deep objects, each case in a
  child process forked before anything ran ``literal_name``'s ``str(x)``
  (written first): the first execution of that call site in a process,
  generic, one C unit more (``crates/pycompat/src/specialize.rs``).

Usage: PDFPLUMBER_ROOT=<main checkout> <root>/.venv/bin/python -m oracle.unit.pdftypes
"""

import builtins
import hashlib
import math
import random
import struct

from pdfminer.pdfexceptions import PDFObjectNotFound
from pdfminer.pdfparser import PDFSyntaxError
from pdfminer.pdftypes import (
    PDFObjRef,
    PDFStream,
    decipher_all,
    dict_value,
    float_value,
    int_value,
    list_value,
    num_value,
    resolve1,
    resolve_all,
    str_value,
    uint_value,
)
from pdfminer.psparser import KWD, LIT, keyword_name, literal_name

from ..canon import canon
from ..depth import executed_call_sites, warm_call_sites
from ._common import at_depth, depth_here, require_oracle_python, write_cases, write_meta
from .document import in_child


# An object whose parsing fails: PDFObjRef.resolve lets the error through.
BROKEN_OBJID = 13


class TableDoc:
    """A document whose getobj() reads a table."""

    def __init__(self, pairs):
        self.table = {}
        self.pairs = pairs

    def getobj(self, objid):
        if objid in self.table:
            return self.table[objid]
        if objid == BROKEN_OBJID:
            raise PDFSyntaxError(f"broken {objid}")
        raise PDFObjectNotFound(objid)


def build(spec, doc):
    """A pdfminer object from a generator spec (the spec is its typed form)."""
    kind = spec[0]
    if kind == "v":
        if isinstance(spec[1], float):
            # a new float object each time: a parsed NaN is never another one
            return struct.unpack(">d", struct.pack(">d", spec[1]))[0]
        return spec[1]
    if kind == "lit":
        return LIT(spec[1])
    if kind == "kw":
        return KWD(spec[1])
    if kind == "ref":
        return PDFObjRef(doc, spec[1])
    if kind == "list":
        return [build(x, doc) for x in spec[1]]
    if kind == "dict":
        return {k: build(x, doc) for k, x in spec[1]}
    if kind == "stream":
        return PDFStream({k: build(x, doc) for k, x in spec[1]}, b"raw")
    raise ValueError(kind)


def spec_value(spec):
    """The object of a spec as a case input: its references have no document
    (typed JSON ``{"ref": n}``; the Rust test binds them to the case's table)."""
    return build(spec, None)


def call(f, *args):
    try:
        return f(*args)
    except Exception as e:  # the exception is the expected result
        return e


# -- random objects -------------------------------------------------------------

NAMES = ["Type", "Page", "F", "Predictor", "A85", "a'b", 'q"', "x\xa0y", "café", " ", "", "1", "\U0001f600"]
BYTES_NAMES = [b"bad\xff", b"\xfe", b"\xc3"]
KEYWORDS = [b"BT", b"Tj", b"'", b'"', b"EI", b"\xff", b"T*"]
FLOATS = [0.0, -0.0, 1.0, 0.1, 1.5, -2.5, 1e16, 1e-5, 123.456, math.inf, -math.inf, math.nan, 1e300]


def rand_leaf(rng, refs, streams=False):
    r = rng.random()
    if r < 0.1:
        return ["v", None]
    if r < 0.18:
        return ["v", rng.choice([True, False])]
    if r < 0.35:
        return ["v", rng.choice([0, 1, -1, 2, 7, 2**63, -(2**70), rng.randint(-1000, 1000)])]
    if r < 0.5:
        return ["v", rng.choice(FLOATS)]
    if r < 0.62:
        return ["v", bytes(rng.choice([0, 39, 34, 92, 10, 65, 200, 255]) for _ in range(rng.randint(0, 5)))]
    if r < 0.75:
        return ["lit", rng.choice(NAMES + BYTES_NAMES)]
    if r < 0.85:
        return ["kw", rng.choice(KEYWORDS)]
    if streams and r < 0.9:
        return ["stream", []]
    if refs:
        return ["ref", rng.choice(refs)]
    return ["v", 3]


def rand_spec(rng, depth=0, refs=(), streams=False):
    r = rng.random()
    if depth < 3 and r < 0.2:
        return ["list", [rand_spec(rng, depth + 1, refs, streams) for _ in range(rng.randint(0, 4))]]
    if depth < 3 and r < 0.4:
        keys = [rng.choice(NAMES) for _ in range(rng.randint(0, 4))]
        return ["dict", [[k, rand_spec(rng, depth + 1, refs, streams)] for k in keys]]
    return rand_leaf(rng, refs, streams)


def contains_nan(spec):
    kind = spec[0]
    if kind == "v":
        return isinstance(spec[1], float) and math.isnan(spec[1])
    if kind == "list":
        return any(contains_nan(x) for x in spec[1])
    if kind in ("dict", "stream"):
        return any(contains_nan(x) for _, x in spec[1])
    return False


# -- cases ----------------------------------------------------------------------


def accessors(x):
    return [
        repr(x),
        literal_name(x),
        keyword_name(x),
        bool(x),
        int_value(x),
        float_value(x),
        num_value(x),
        str_value(x),
        list_value(x),
        dict_value(x),
        call(lambda: "Predictor" in x),
        call(lambda: x[0]),
    ]


def gen_objects(rng):
    for _ in range(4000):
        spec = rand_spec(rng)
        doc = TableDoc([])
        x = build(spec, doc)
        yield [spec_value(spec)], accessors(x)


def variant(rng, spec):
    """A spec related to `spec`: equal, or differing in one place."""
    kind = spec[0]
    r = rng.random()
    if r < 0.4:
        return spec
    if kind == "v" and isinstance(spec[1], (bool, int, float)) and not isinstance(spec[1], bool) or kind == "v" and isinstance(spec[1], bool):
        v = spec[1]
        alts = [v, float(v) if not (isinstance(v, float) and not math.isfinite(v)) and abs(v) < 2**1000 else v, -v if v else 0.0,
                bool(v) if isinstance(v, int) and v in (0, 1) else v, int(v) if isinstance(v, float) and v.is_integer() else v]
        return ["v", rng.choice(alts)]
    if kind == "lit" and isinstance(spec[1], str):
        return ["lit", rng.choice([spec[1], spec[1].encode("utf-8"), spec[1] + "x"])]
    if kind == "list":
        items = [variant(rng, x) for x in spec[1]]
        if rng.random() < 0.2:
            items = items[:-1]
        return ["list", items]
    if kind == "dict":
        items = [[k, variant(rng, x)] for k, x in spec[1]]
        if rng.random() < 0.2:
            items = list(reversed(items))
        if rng.random() < 0.1:
            items = items + [["Extra", ["v", 1]]]
        return ["dict", items]
    return rand_spec(rng)


def gen_eq(rng):
    for _ in range(4000):
        a = rand_spec(rng)
        b = variant(rng, a)
        doc = TableDoc([])
        x, y = build(a, doc), build(b, doc)
        yield [spec_value(a), spec_value(b)], [x == y, y == x]
    # references and streams compare by identity: two built from the same
    # spec differ, the same object (shared by a shallow copy) is equal
    for _ in range(1000):
        a = rand_spec(rng, refs=[1, 2], streams=True)
        doc = TableDoc([])
        x, y = build(a, doc), build(a, doc)
        yield [spec_value(a), spec_value(a)], [x == y, y == x]
    # one object compared with itself (identity shortcut of containers)
    for _ in range(300):
        a = rand_spec(rng, refs=[1, 2], streams=True)
        x = build(a, TableDoc([]))
        yield [spec_value(a), "same"], [x == x, x == x]
    # A shallow copy shares the elements: Python compares them by identity
    # first. Floats have no identity in the port (they are values), so a NaN
    # shared by two containers would compare unequal: the parser never
    # produces NaN, and these cases leave it out.
    for _ in range(1000):
        a = rand_spec(rng, refs=[1, 2], streams=True)
        if a[0] not in ("list", "dict") or contains_nan(a):
            continue
        x = build(a, TableDoc([]))
        y = list(x) if isinstance(x, list) else dict(x)
        yield [spec_value(a), "shallow"], [x == y, y == x]


def gen_resolve(rng):
    for _ in range(3000):
        n = rng.randint(0, 5)
        ids = list(range(1, n + 1))
        missing = [n + 1, 0, BROKEN_OBJID]
        refs = ids + missing
        table_specs = [[i, rand_spec(rng, 1, refs=[j for j in refs if j > i or j not in ids])] for i in ids]
        if rng.random() < 0.3 and ids:
            # a chain of references (acyclic: Python would loop forever)
            table_specs[0][1] = ["ref", rng.choice([j for j in refs if j != 1])]
        x_spec = rand_spec(rng, 0, refs=refs) if rng.random() < 0.6 else ["ref", rng.choice(refs)]
        doc = TableDoc(None)
        for i, s in table_specs:
            doc.table[i] = build(s, doc)
        x = build(x_spec, doc)
        r1 = call(resolve1, x)
        ra = call(resolve_all, x)
        again = call(resolve1, x)
        out = [r1, ra, again, call(int_value, x), call(num_value, x), call(dict_value, x), call(list_value, x),
               call(str_value, x), repr(x)]
        yield [[[i, spec_value(s)] for i, s in table_specs], spec_value(x_spec)], out


STREAM_FILTERS = [None, LIT("DCTDecode"), LIT("DCT"), LIT("JPXDecode"), LIT("JBIG2Decode"), [LIT("DCT"), LIT("JPXDecode")],
                  LIT("Crypt"), LIT("Foo"), 5, [], [LIT("DCT"), LIT("Foo")], b"s", [LIT("Crypt")], [LIT("DCT"), 1.5],
                  {"A": 1}, True]
STREAM_PARAMS = [None, {}, {"Predictor": 1}, {"Predictor": 5}, {"Predictor": True}, {"Predictor": False},
                 {"Predictor": 1.5}, {"Predictor": 0}, {"Predictor": -3}, [{"Predictor": 3}], [{}, {"Predictor": 1}], b"x",
                 5, [LIT("X")], {"Predictor": -(2**70)}, [], {"Colors": 3}]


def gen_streams(rng):
    for _ in range(3000):
        attrs = {}
        if rng.random() < 0.8:
            f = rng.choice(STREAM_FILTERS)
            if f is not None:
                attrs[rng.choice(["Filter", "F"])] = f
        if rng.random() < 0.7:
            p = rng.choice(STREAM_PARAMS)
            if p is not None:
                attrs[rng.choice(["DecodeParms", "DP", "FDecodeParms"])] = p
        if rng.random() < 0.3:
            attrs["Length"] = rng.randint(0, 10)
        raw = bytes(rng.randint(0, 255) for _ in range(rng.randint(0, 6)))
        objid = rng.choice([None, 0, 7, 12345])
        # DecodeParms may be a stream (an indirect object): `in` and `[]`
        # then read its attributes. It is added last, so attrs keep the order.
        params_stream = rng.choice([1, 5, True, 0, -(2**70)]) if rng.random() < 0.1 else None
        full_attrs = dict(attrs)
        if params_stream is not None:
            full_attrs["DecodeParms"] = PDFStream({"Predictor": params_stream}, b"")
        s = PDFStream(full_attrs, raw)
        if objid is not None:
            s.set_objid(objid, 0)
        before = repr(s)
        filters = call(s.get_filters)
        data = call(s.get_data)
        out = [before, filters, data, repr(s), s.get_rawdata(), s.data, call(lambda: s["Missing"]),
               call(lambda: s["Length"]), s.get("Length", -1), s.get_any(["F", "Filter"], "none"), "Length" in s]
        yield [attrs, raw, objid, params_stream], out


def test_decipher(objid, genno, data, attrs=None):
    """A deterministic stand-in for the document's decryption callback."""
    key = (objid + 7 * genno + (len(attrs) if attrs is not None else 100)) & 0xFF
    return bytes(b ^ key for b in reversed(data)) + bytes([len(data) & 0xFF])


def gen_decipher(rng):
    for _ in range(2000):
        spec = rand_spec(rng)
        objid, genno = rng.choice([1, 7, 300]), rng.choice([0, 1, 65535])
        x = build(spec, TableDoc([]))
        out = call(decipher_all, test_decipher, objid, genno, x)
        yield [spec_value(spec), objid, genno], [out, x]
    for _ in range(1000):
        attrs = {}
        if rng.random() < 0.5:
            attrs["Filter"] = rng.choice([LIT("DCT"), LIT("Foo"), [LIT("JPXDecode")]])
        if rng.random() < 0.5:
            attrs["Length"] = rng.randint(0, 9)
        raw = bytes(rng.randint(0, 255) for _ in range(rng.randint(0, 6)))
        s = PDFStream(dict(attrs), raw, test_decipher)
        objid = rng.choice([None, 4, 9])
        if objid is not None:
            s.set_objid(objid, rng.choice([0, 2]))
        yield [attrs, raw, objid, s.genno], [call(s.get_data), repr(s)]


def gen_refs(rng):
    for objid in [0, 1, 5, 2**70]:
        r = PDFObjRef(None, objid)
        yield [objid], [repr(r), call(r.resolve), call(resolve1, r), call(resolve_all, [r]), r.doc is None]
    for objid in [0, 5, BROKEN_OBJID]:
        r = PDFObjRef(TableDoc([]), objid)
        yield [objid, "bound"], [repr(r), call(r.resolve), call(r.resolve, 7), call(resolve1, r, 8), r.doc is None]


def rand_scalar(rng):
    return rng.choice([0, 1, -5, 2**64, 1.5, -0.0, b"", b"ab", None, True, LIT("F"), KWD(b"Tj")])


def gen_containers(rng):
    """Random operation sequences on a list and on a dict."""
    for _ in range(1500):
        lst, ops, out = [], [], []
        alias = lst
        for _ in range(rng.randint(1, 10)):
            r = rng.random()
            if r < 0.35:
                v = rand_scalar(rng)
                lst.append(v)
                ops.append(["append", v])
                out.append(None)
            elif r < 0.5 and lst:
                k = rng.randrange(len(lst))
                v = rand_scalar(rng)
                lst[k] = v
                ops.append(["set", k, v])
                out.append(None)
            elif r < 0.6:
                ops.append(["len"])
                out.append(len(lst))
            elif r < 0.7 and lst:
                k = rng.randrange(len(lst))
                ops.append(["get", k])
                out.append(lst[k])
            elif r < 0.8:
                ops.append(["items"])
                out.append(list(lst))
            elif r < 0.9:
                ops.append(["is_alias"])
                out.append(alias is lst)
            else:
                ops.append(["is_copy"])
                out.append(list(lst) is lst)
        yield ["list", ops], out
    for _ in range(1500):
        d, ops, out = {}, [], []
        alias = d
        for _ in range(rng.randint(1, 10)):
            r = rng.random()
            k = rng.choice(["A", "B", "C", "Type"])
            if r < 0.35:
                v = rand_scalar(rng)
                d[k] = v
                ops.append(["set", k, v])
                out.append(None)
            elif r < 0.5:
                ops.append(["pop", k])
                out.append(d.pop(k, None))
            elif r < 0.6:
                ops.append(["keys"])
                out.append(list(d.keys()))
            elif r < 0.7:
                ops.append(["len"])
                out.append(len(d))
            elif r < 0.8:
                # a copy is a new dict: changing it leaves the original alone
                c = d.copy()
                v = rand_scalar(rng)
                c[k] = v
                ops.append(["copy_set", k, v])
                out.append([list(c.items()), list(d.items()), c is d])
            elif r < 0.9:
                ops.append(["get", k])
                out.append(d.get(k))
            else:
                ops.append(["is_alias"])
                out.append(alias is d)
        yield ["dict", ops], out


def gen_uint(rng):
    values = [0, -1, 1, 2, 255, 256, -128, -(2**31), 2**31, 2**32, -(2**40), 2**63, -(2**63), 2**70, -(2**70), True, False,
              1.5, -1.5, 0.0, None, b"x", LIT("A"), [1], {"A": 1}]
    for v in values:
        for n_bits in (0, 1, 8, 16, 32, 64, 100):
            yield [v, n_bits], call(uint_value, v, n_bits)
    for v in (5, -5, True, 2.5):  # through a reference
        doc = TableDoc(None)
        doc.table = {1: v}
        yield [[[1, ["v", v]]], 8], call(uint_value, PDFObjRef(doc, 1), 8)


def deep_object(shape, n, leaf, doc):
    """``n`` containers deep around ``leaf``: lists, dicts (``{"K": ...}``),
    both alternating, or a list holding itself (``n`` ignored)."""
    if shape == "cycle":
        x = [build(leaf, doc)]
        x.append(x)
        return x
    x = build(leaf, doc)
    for i in range(n):
        x = {"K": x} if shape == "dict" or (shape == "mixed" and i % 2) else [x]
    return x


def canon_sha(x):
    if isinstance(x, BaseException):
        return x
    return hashlib.sha256(canon(x).encode("ascii")).hexdigest()


def text_sha(x):
    """The SHA-256 of a (long) str result, or the exception."""
    if isinstance(x, BaseException):
        return x
    return hashlib.sha256(x.encode("utf-8")).hexdigest()


DEEP_LEAVES = [["list", []], ["dict", []], ["v", 7], ["v", 2**80], ["v", 1.5], ["v", None], ["v", b"x'y"], ["lit", "F1"],
               ["lit", b"\xff"], ["kw", b"Tj"], ["ref", 5], ["stream", [["Length", ["v", 3]], ["A", ["list", [["v", 1]]]]]],
               ["dict", [["Q", ["lit", "R"]]]]]


def gen_deep(rng):
    warm_call_sites()
    # repr() and str() (literal_name) around the C recursion limit
    for op_name, op in (("repr", repr), ("literal_name", literal_name)):
        for shape in ("list", "dict", "mixed"):
            for leaf in DEEP_LEAVES:
                left = depth_here()["c_left"]
                for n in range(left - 5, left + 2):
                    spec = ["deep", shape, n, leaf]
                    depth, out = at_depth(op, deep_object(shape, n, leaf, TableDoc([])))
                    yield [op_name, depth, spec, []], text_sha(out)
        depth, out = at_depth(op, deep_object("cycle", 0, ["v", 1], None))
        yield [op_name, depth, ["deep", "cycle", 0, ["v", 1]], []], text_sha(out)
    # resolve_all and decipher_all around the Python frame limit (the leaf
    # reference resolves through PDFObjRef.resolve and TableDoc.getobj, the
    # leaf bytes through test_decipher and its generator expression)
    table = [[5, ["list", [["v", 1], ["ref", 6]]]], [6, ["v", b"z"]]]
    for shape in ("list", "dict", "mixed"):
        for leaf in (["ref", 5], ["v", b"ab"], ["v", 3], ["list", []]):
            frames = depth_here()["py"] + 1  # at_depth's frame
            for n in range(1000 - frames - 8, 1000 - frames + 2):
                spec = ["deep", shape, n, leaf]
                doc = TableDoc(None)
                for i, s in table:
                    doc.table[i] = build(s, doc)
                depth, out = at_depth(resolve_all, deep_object(shape, n, leaf, doc))
                yield ["resolve_all", depth, spec, table], canon_sha(out)
                depth, out = at_depth(decipher_all, test_decipher, 1, 0, deep_object(shape, n, leaf, TableDoc([])))
                yield ["decipher_all", depth, spec, []], canon_sha(out)
    # a list holding itself: RecursionError, whatever the depth
    for leaf in (["ref", 5], ["v", b"ab"]):
        spec = ["deep", "cycle", 0, leaf]
        doc = TableDoc(None)
        for i, s in table:
            doc.table[i] = build(s, doc)
        depth, out = at_depth(resolve_all, deep_object("cycle", 0, leaf, doc))
        yield ["resolve_all", depth, spec, table], canon_sha(out)
        depth, out = at_depth(decipher_all, test_decipher, 1, 0, deep_object("cycle", 0, leaf, TableDoc([])))
        yield ["decipher_all", depth, spec, []], canon_sha(out)


def _first_literal_name(shape, n, leaf):
    """In a forked child: ``literal_name`` of a deep object as the first
    ``str(x)`` of its call site, ``[depth, out]`` (JSON)."""
    if executed_call_sites():
        raise RuntimeError(f"call sites already executed: {executed_call_sites()}")
    depth, out = at_depth(literal_name, deep_object(shape, n, leaf, TableDoc([])))
    out = text_sha(out)
    return [depth, [type(out).__name__, str(out)] if isinstance(out, BaseException) else out]


def gen_deep_first(rng):
    # literal_name's str(x) executed for the first time in the process, around
    # the C recursion limit (each case in its own forked child)
    for shape in ("list", "dict", "mixed"):
        for leaf in (["list", []], ["v", 7], ["dict", [["Q", ["lit", "R"]]]], ["ref", 5]):
            left = depth_here()["c_left"]
            for n in range(left - 4, left + 2):
                depth, out = in_child(lambda: _first_literal_name(shape, n, leaf))
                if isinstance(out, list):
                    cls, msg = out
                    out = getattr(builtins, cls)(msg)
                yield ["literal_name", depth, ["deep", shape, n, leaf], []], out


GENERATORS = {
    # first: forked before anything executes literal_name's str(x)
    "deep_first": gen_deep_first,
    "objects": gen_objects,
    "eq": gen_eq,
    "resolve": gen_resolve,
    "streams": gen_streams,
    "decipher": gen_decipher,
    "refs": gen_refs,
    "uint": gen_uint,
    "containers": gen_containers,
    "deep": gen_deep,
}


def main():
    require_oracle_python()
    counts = {name: write_cases("pdftypes", name, gen(random.Random(f"20261014:{name}"))) for name, gen in GENERATORS.items()}
    write_meta("pdftypes", counts=counts)
    print(f"wrote {counts}")

if __name__ == "__main__":
    main()
