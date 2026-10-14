"""Differential cases for crates/pdfminer/src/pdftypes.rs (and the parts of
psparser.py it relies on), computed by pdfminer.

Writes typed JSON lines (docs/verification.md) to
``$PDFPLUMBER_ROOT/.cache/unit/pdftypes/<name>.jsonl``, one case per line,
``{"in": [...], "out": ...}``; the Rust replay is
crates/pdftrace/tests/pdftypes_unit.rs. In the inputs, ``{"ref": n}`` is a
``PDFObjRef`` to a document whose objects are given by the case (a list of
``[objid, object]`` pairs; a missing id raises ``PDFObjectNotFound``).

- ``objects``: ``repr``, ``literal_name``, ``keyword_name``, ``bool``, the
  ``*_value`` accessors, ``"Predictor" in x`` and ``x[0]`` of random objects.
- ``eq``: ``a == b`` for pairs of related objects.
- ``resolve``: ``resolve1``, ``resolve_all`` (which updates dicts in place:
  the case re-reads the object afterwards) and the accessors through
  references.
- ``streams``: ``PDFStream`` ``repr``, ``get_filters``, ``get_data`` (filters
  that need no unported decoder) and ``__getitem__``.

Usage: PDFPLUMBER_ROOT=<main checkout> <root>/.venv/bin/python oracle/unit/pdftypes.py
"""

import hashlib
import json
import math
import os
import random
import struct
import sys

from pdfminer.pdfexceptions import PDFObjectNotFound
from pdfminer.pdftypes import (
    PDFObjRef,
    PDFStream,
    dict_value,
    float_value,
    int_value,
    list_value,
    num_value,
    resolve1,
    resolve_all,
    str_value,
)
from pdfminer.psparser import KWD, LIT, PSKeyword, PSLiteral, keyword_name, literal_name

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", ".."))


# -- typed JSON -----------------------------------------------------------------


def typed(v):
    if v is None or isinstance(v, (bool, str)):
        return v
    if isinstance(v, int):
        return {"i": str(v)}
    if isinstance(v, float):
        return {"f": struct.pack(">d", v).hex()}
    if isinstance(v, bytes):
        return {"b": v.hex()}
    if isinstance(v, list):
        return [typed(x) for x in v]
    if isinstance(v, tuple):
        return {"t": [typed(x) for x in v]}
    if isinstance(v, dict):
        return {"d": [[typed(k), typed(x)] for k, x in v.items()]}
    if isinstance(v, PSLiteral):
        return {"lit": v.name if isinstance(v.name, str) else {"b": v.name.hex()}}
    if isinstance(v, PSKeyword):
        return {"kw": v.name.hex()}
    if isinstance(v, PDFObjRef):
        return {"ref": v.objid}
    if isinstance(v, BaseException):
        return {"exc": type(v).__name__, "msg": str(v)}
    raise TypeError(f"no typed JSON form for {type(v).__name__}")


class TableDoc:
    """A document whose getobj() reads a table."""

    def __init__(self, pairs):
        self.table = {}
        self.pairs = pairs

    def getobj(self, objid):
        if objid in self.table:
            return self.table[objid]
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
    raise ValueError(kind)


def spec_typed(spec):
    kind = spec[0]
    if kind == "v":
        return typed(spec[1])
    if kind == "lit":
        return typed(LIT(spec[1]))
    if kind == "kw":
        return {"kw": spec[1].hex()}
    if kind == "ref":
        return {"ref": spec[1]}
    if kind == "list":
        return [spec_typed(x) for x in spec[1]]
    if kind == "dict":
        return {"d": [[k, spec_typed(x)] for k, x in spec[1]]}
    raise ValueError(kind)


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


def rand_leaf(rng, refs):
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
    if refs:
        return ["ref", rng.choice(refs)]
    return ["v", 3]


def rand_spec(rng, depth=0, refs=()):
    r = rng.random()
    if depth < 3 and r < 0.2:
        return ["list", [rand_spec(rng, depth + 1, refs) for _ in range(rng.randint(0, 4))]]
    if depth < 3 and r < 0.4:
        keys = [rng.choice(NAMES) for _ in range(rng.randint(0, 4))]
        return ["dict", [[k, rand_spec(rng, depth + 1, refs)] for k in keys]]
    return rand_leaf(rng, refs)


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
        yield [spec_typed(spec)], accessors(x)


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
        yield [spec_typed(a), spec_typed(b)], [x == y, y == x]
    # one object compared with itself (identity shortcut of containers)
    for _ in range(300):
        a = rand_spec(rng)
        x = build(a, TableDoc([]))
        yield [spec_typed(a), "same"], [x == x, x == x]


def gen_resolve(rng):
    for _ in range(3000):
        n = rng.randint(0, 5)
        ids = list(range(1, n + 1))
        missing = [n + 1, 0]
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
        yield [[[typed(i), spec_typed(s)] for i, s in table_specs], spec_typed(x_spec)], out


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
        s = PDFStream(dict(attrs), raw)
        if objid is not None:
            s.set_objid(objid, 0)
        before = repr(s)
        filters = call(s.get_filters)
        data = call(s.get_data)
        out = [before, filters, data, repr(s), s.get_rawdata(), call(lambda: s["Missing"]), call(lambda: s["Length"]),
               s.get("Length", -1), s.get_any(["F", "Filter"], "none"), "Length" in s]
        yield [typed(attrs), typed(raw), typed(objid)], out


GENERATORS = {"objects": gen_objects, "eq": gen_eq, "resolve": gen_resolve, "streams": gen_streams}


def main():
    if sys.version_info[:2] != (3, 12):
        sys.exit(f"the oracle is CPython 3.12, not {sys.version.split()[0]}")
    root = os.environ.get("PDFPLUMBER_ROOT") or REPO
    outdir = os.path.join(root, ".cache", "unit", "pdftypes")
    os.makedirs(outdir, exist_ok=True)
    counts = {}
    for name, gen in GENERATORS.items():
        rng = random.Random(f"20261014:{name}")
        n = 0
        with open(os.path.join(outdir, name + ".jsonl"), "w", encoding="ascii") as f:
            for args, out in gen(rng):
                f.write(json.dumps({"in": args, "out": typed(out)}) + "\n")
                n += 1
        counts[name] = n
    with open(os.path.abspath(__file__), "rb") as f:
        digest = hashlib.sha256(f.read()).hexdigest()
    with open(os.path.join(outdir, "meta.json"), "w") as f:
        json.dump({"generator_sha256": digest, "python": sys.version.split()[0], "counts": counts}, f)
    print(f"wrote {counts} to {outdir}")


if __name__ == "__main__":
    main()
