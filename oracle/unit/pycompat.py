"""Differential cases for crates/pycompat, computed by CPython.

Exercises only the standard library. Writes ``<cache>/unit/pycompat/<name>.jsonl``
(see ``oracle/unit/_common.py``): one case per line, ``{"in": [arguments...],
"out": result}`` in typed JSON, where the result is the value CPython returns
or the exception it raises (``{"exc": ..., "msg": ...}``). The Rust replay is
crates/pdftrace/tests/pycompat_unit.rs.

Usage: PDFPLUMBER_ROOT=<main checkout> <root>/.venv/bin/python -m oracle.unit.pycompat [--seed N]

The oracle is CPython 3.12 (``.python-version``); the script refuses to run
on another minor version.
"""

import argparse
import io
import json
import math
import random
import re
import struct
import unicodedata

from ._common import require_oracle_python, write_cases, write_meta


def call(f, *args):
    try:
        return f(*args)
    except Exception as e:  # the exception is the expected result
        return e


# -- value generators ---------------------------------------------------------

SPECIAL_FLOATS = [0.0, -0.0, 1.0, -1.0, 0.5, 1.5, 2.5, -2.5, 0.1, 0.2, 0.3, 1e16, 1e-4, 1e-5, 9.999999999999999e15,
                  123456789.0, 2.0**52, 2.0**53, 2.0**53 + 2, 2.0**63, 2.0**64, -(2.0**63), 1e308, 1.7976931348623157e308,
                  2.2250738585072014e-308, 2.225073858507201e-308, 5e-324, -5e-324, math.inf, -math.inf, math.nan,
                  0.30000000000000004, 1e22, 1e23, 5e-5, 0.0001, 100.0, 1234.5678, 2.675, 0.125, 0.375, 1e15, 1e17]


def rand_float(rng):
    r = rng.random()
    if r < 0.15:
        return rng.choice(SPECIAL_FLOATS)
    if r < 0.35:
        # uniformly random finite bit patterns
        while True:
            f = struct.unpack(">d", rng.getrandbits(64).to_bytes(8, "big"))[0]
            if math.isfinite(f):
                return f
    if r < 0.6:
        # PDF-like coordinates
        return round(rng.uniform(-1000, 1000), rng.randint(0, 6))
    if r < 0.75:
        return float(rng.randint(-10**6, 10**6)) + rng.choice([0.0, 0.5, 0.25, 0.125])
    if r < 0.85:
        return rng.uniform(-1, 1) * 10.0 ** rng.randint(-320, 308)
    return rng.choice([1, -1]) * rng.random() * 2.0 ** rng.randint(-1074, 1023)


def rand_int(rng):
    r = rng.random()
    if r < 0.3:
        return rng.randint(-1000, 1000)
    if r < 0.5:
        return rng.randint(-(2**62), 2**62)
    if r < 0.65:
        return rng.choice([1, -1]) * (2**63 + rng.randint(-3, 3))
    if r < 0.75:
        return rng.choice([1, -1]) * (2**53 + rng.randint(-3, 3))
    if r < 0.9:
        return rng.randint(-(2**200), 2**200)
    return rng.choice([1, -1]) * rng.getrandbits(rng.randint(64, 1100))


def rand_num(rng):
    return rand_int(rng) if rng.random() < 0.4 else rand_float(rng)


# -- case generators ------------------------------------------------------------


def exc_classes():
    """Every exception class of the ported modules, with its bases."""
    import builtins
    import struct as struct_mod
    import zlib

    classes = [c for c in vars(builtins).values() if isinstance(c, type) and issubclass(c, BaseException)]
    classes += [struct_mod.error, zlib.error]
    return classes


def gen_exc_hierarchy(rng):
    for cls in exc_classes():
        yield [cls.__module__, cls.__qualname__], [cls.__name__, [[b.__module__, b.__qualname__] for b in cls.__bases__]]


def gen_int_parse(rng):
    fixed = ["0", "-0", "+12", "007", " 12 ", "\t-3\n", "1_000", "1__0", "_1", "1_", "", " ", "-", "+", "--5", "0x1f",
             "0X1F", "0o17", "0b101", "0_1", "00", "08", "0x", "0x_1", "1e5", "12.5", "١٢٣", "３４", " 12 ",
             "1\x00", "9" * 4300, "9" * 4301, "0" * 4301, "-" + "1" * 4301, "1_" * 2200 + "1", "abc", "zz", "Z",
             "12" * 150 + "x", "\x7f", "１_２", "12\u0085"]
    for s in fixed:
        for base in (10, 0, 16, 8, 2, 36):
            yield [s, base], call(int, s, base)
            yield [s.encode("utf-8"), base], call(int, s.encode("utf-8"), base)
    for base in (1, 37, -1):
        yield ["1", base], call(int, "1", base)
    alphabet = "0123456789abcdefxXoObB_ +-\t\n"
    for _ in range(3000):
        n = rng.randint(0, 30)
        s = "".join(rng.choice(alphabet) for _ in range(n))
        base = rng.choice([10, 10, 10, 0, 16, 8, 2, 36, 7])
        if rng.random() < 0.5:
            yield [s, base], call(int, s, base)
        else:
            yield [s.encode(), base], call(int, s.encode(), base)
    for _ in range(500):
        s = str(rand_int(rng))
        yield [s.encode(), 10], call(int, s.encode(), 10)
        yield [s, 10], call(int, s, 10)


def gen_int_str(rng):
    for v in [0, -1, 10**4299, 10**4300 - 1, 10**4300, -(10**4300), 2**14300]:
        yield [v], call(str, v)
    for _ in range(1000):
        v = rand_int(rng)
        yield [v], call(str, v)


def gen_int_to_float(rng):
    fixed = [0, 1, -1, 2**53, 2**53 + 1, 2**53 + 2, 2**53 + 3, 2**63 - 1, -(2**63), 2**64 + 1,
             (2**53 - 1) * 2**971, 2**1024 - 2**970, 2**1024 - 2**970 - 1, 2**1024, -(2**1024 - 2**970 - 1)]
    for v in fixed:
        yield [v], call(float, v)
    for _ in range(3000):
        k = rng.randint(54, 1030)
        mant = rng.getrandbits(k) | (1 << (k - 1))
        if rng.random() < 0.3:
            # halfway cases: 53 significant bits then a 1 then zeros (+/- 1)
            shift = k - 54
            mant = ((mant >> shift) | 1) << shift
            mant += rng.choice([-1, 0, 0, 1])
        v = mant * rng.choice([1, -1])
        yield [v], call(float, v)


BINOPS = {
    "+": lambda a, b: a + b,
    "-": lambda a, b: a - b,
    "*": lambda a, b: a * b,
    "/": lambda a, b: a / b,
    "//": lambda a, b: a // b,
    "%": lambda a, b: a % b,
    "**": lambda a, b: a**b,
}


def gen_binop(rng):
    for _ in range(12000):
        op = rng.choice(list(BINOPS))
        a, b = rand_num(rng), rand_num(rng)
        if rng.random() < 0.05:
            b = rng.choice([0, 0.0, -0.0])
        if op == "**":
            if isinstance(a, int) and isinstance(b, int):
                a = rng.randint(-50, 50) if abs(a) > 2**64 else a
                b = rng.randint(-5, 40)
            elif isinstance(b, int) and abs(b) > 2000:
                b = rng.randint(-40, 40)
            if a < 0 and isinstance(b, float) and math.isfinite(b) and b != math.floor(b):
                a = -a  # a negative number to a fractional power is a complex
            if rng.random() < 0.3:
                b = rng.choice([0.5, 2, 2.0, -1, 3, 0.25, -0.5])
                a = -a if a < 0 and isinstance(b, float) and b != math.floor(b) else a
        if op in ("/",) and isinstance(a, int) and isinstance(b, int) and rng.random() < 0.3:
            a, b = rng.choice([1, -1]) * rng.getrandbits(rng.randint(1, 1200)), rng.getrandbits(rng.randint(1, 1200)) or 1
        yield [op, a, b], call(BINOPS[op], a, b)
    # true division into the subnormal range and around overflow
    for k in range(300, 330):
        yield ["/", 1, 10**k], call(BINOPS["/"], 1, 10**k)
        yield ["/", -3, 10**k], call(BINOPS["/"], -3, 10**k)
    for k in range(1020, 1030):
        yield ["/", 2**k, 3], call(BINOPS["/"], 2**k, 3)
        yield ["/", 2**k + 1, 2], call(BINOPS["/"], 2**k + 1, 2)
    for a, b in [(0, -5), (-0, 5), (2**2000, 2**2000 + 1), (1, 2**1074), (1, 2**1075), (3, 2**1076), (2**1100, 2**76)]:
        yield ["/", a, b], call(BINOPS["/"], a, b)


def gen_compare(rng):
    edge = [2**53, 2**53 + 1, 2**63, 2**63 - 1, -(2**63), 2**64, 10**400, -(10**400), 0, -1, 1]
    for _ in range(4000):
        if rng.random() < 0.5:
            a = rng.choice(edge) + rng.randint(-2, 2) if rng.random() < 0.5 else rand_int(rng)
            b = float(a) if rng.random() < 0.5 and abs(a) < 2**1000 else rand_float(rng)
            if rng.random() < 0.1:
                b = rng.choice([math.inf, -math.inf, math.nan])
        else:
            a, b = rand_num(rng), rand_num(rng)
        if rng.random() < 0.5:
            a, b = b, a
        yield [a, b], [a < b, a <= b, a == b, a != b, a > b, a >= b]


def gen_sum(rng):
    for _ in range(4000):
        n = rng.randint(0, 25)
        mode = rng.random()
        items = []
        for _ in range(n):
            if mode < 0.4:
                items.append(round(rng.uniform(-1000, 1000), rng.randint(0, 5)))
            elif mode < 0.6:
                items.append(rng.choice([0.1, 0.2, 0.3, 1e16, -1e16, 1.0, 1e-16, 3.0]))
            elif mode < 0.75:
                items.append(rng.choice([rng.randint(-10, 10), round(rng.uniform(-10, 10), 3)]))
            elif mode < 0.85:
                items.append(rng.choice([2**63 - 1, 2**63, -(2**63), rng.randint(-5, 5), 0.5, 2**70, 1e300, -0.0]))
            else:
                items.append(rand_num(rng))
        if rng.random() < 0.05:
            items.append(rng.choice([math.inf, -math.inf, math.nan, 1e308, -1e308]))
        if rng.random() < 0.2:
            start = rng.choice([0, 0.0, -0.0, 5, 2**64, 1.5])
            yield [items, start], call(sum, items, start)
        else:
            yield [items], call(sum, items)


def gen_minmax(rng):
    pool = [0, 0.0, -0.0, 1, 1.0, -1, -1.0, 2**53, float(2**53), 2**53 + 1, math.nan, math.inf, -math.inf, 0.5, 3]
    for _ in range(3000):
        n = rng.randint(0, 6)
        items = [rng.choice(pool) if rng.random() < 0.8 else rand_num(rng) for _ in range(n)]
        yield [items], [call(min, items), call(max, items)]


def gen_float_repr(rng):
    for f in SPECIAL_FLOATS:
        yield [f], repr(f)
    for k in range(-330, 309):
        f = float(f"1e{k}")
        yield [f], repr(f)
        yield [f * 1.5], repr(f * 1.5)
    for _ in range(20000):
        f = rand_float(rng)
        yield [f], repr(f)
    # doubles exactly halfway between two shortest candidates (dtoa takes the
    # even digit), and large integers whose shortest digits are rounded
    for _ in range(3000):
        f = float(rng.randint(2**47, 2**52)) + rng.choice([0.25, 0.75, 0.5, 0.125, 0.375])
        yield [f], repr(f)
        g = float(rng.randint(10**16, 10**23)) * rng.choice([1, 2, 5, 25, 125])
        yield [g], repr(g)


def gen_round(rng):
    for _ in range(4000):
        x = rand_float(rng)
        yield [x], call(round, x)
    for x in [0.5, 1.5, 2.5, -0.5, -2.5, 2.675, 0.125, 0.375, 1e16 + 2, 4503599627370497.0, math.inf, math.nan]:
        yield [x], call(round, x)
    for _ in range(8000):
        x = rand_float(rng)
        n = rng.choice([rng.randint(-5, 15), rng.randint(-330, 330)])
        yield [x, n], call(round, x, n)
    halfway = [2.675, 1.005, 0.125, 0.375, 2.5, 0.5, -0.5, 1.45, 5e-324, 1e-320, 1.7976931348623157e308, 1e300]
    for x in halfway:
        for n in [-400, -309, -308, -307, -300, -1, 0, 1, 2, 3, 300, 320, 323, 324, 400]:
            yield [x, n], call(round, x, n)
    for _ in range(2000):
        i = rand_int(rng)
        n = rng.randint(-25, 5)
        yield [i, n], call(round, i, n)
    for i, n in [(5, -1), (15, -1), (25, -1), (-25, -1), (10**400, -399), (5 * 10**399, -400), (5, -10)]:
        yield [i, n], call(round, i, n)


def gen_float_parse(rng):
    fixed = ["1", "-1", "+1.5", ".5", "5.", ".", "-.", "+.", "1e5", "1E-5", "1e", "1e+", "1e+5x", "inf", "-inf", "+inf",
             "Infinity", "-INFINITY", "infinit", "nan", "-nan", "+NaN", "nan(1)", " 1 ", "\t1.5\n", "1_0.5", "1__0", "_1",
             "1_", "1_e5", "1e_5", "1.5_5", "0x10", "1.2.3", "--1", "", " ", "1\x00", "1" * 400 + "." + "9" * 400,
             "9007199254740993", "9007199254740993.000000000000000000001", "2.4703282292062327e-324",
             "2.4703282292062328e-324", "1e-400", "-1e-400", "1e400", "1e99999999999999999999", "0e99999999999",
             "١٫٥", "١.٥", "３.５", " 1.5 ", "1.5\u0085", "1.5é", "4.9406564584124654e-324",
             "1.7976931348623157e308", "1.7976931348623158e308", "1.7976931348623159e308"]
    for s in fixed:
        yield [s], call(float, s)
        try:
            b = s.encode("ascii")
        except UnicodeEncodeError:
            continue
        yield [b], call(float, b)
    alphabet = "0123456789.eE+-_ \tinfaINFxy"
    for _ in range(4000):
        n = rng.randint(0, 20)
        s = "".join(rng.choice(alphabet) for _ in range(n))
        yield [s.encode()], call(float, s.encode())
    for _ in range(4000):
        # well-formed decimals, some long enough to need exact rounding
        digits = "".join(rng.choice("0123456789") for _ in range(rng.choice([1, 5, 17, 20, 40, 800])))
        k = rng.randint(0, len(digits))
        s = rng.choice(["", "-", "+"]) + digits[:k] + "." + digits[k:]
        if rng.random() < 0.4:
            s += "e" + str(rng.randint(-400, 400))
        yield [s.encode()], call(float, s.encode())


STR_POOL = (
    [chr(c) for c in range(0, 0x80)]
    + [chr(c) for c in range(0x80, 0x100)]
    + [" ", " ", "​", "﻿", "̀", "א", "中", "", "￿", "\U0001f600",
       "\U000e0001", "\U0010ffff", "\U0001d400", "͸", "　", " ", "᠎", "\u0085", "'", '"', "\\"]
)


def rand_str(rng, n):
    return "".join(rng.choice(STR_POOL) for _ in range(n))


def gen_str_repr(rng):
    for s in ["", "'", '"', "'\"", "a'b", 'a"b', "\\", "café", "­", "\x7f", "\U0001f600"]:
        yield [s], repr(s)
    for _ in range(4000):
        s = rand_str(rng, rng.randint(0, 12))
        yield [s], repr(s)
    for _ in range(500):
        s = "".join(chr(rng.choice([rng.randint(0, 0xD7FF), rng.randint(0xE000, 0x10FFFF)])) for _ in range(8))
        yield [s], repr(s)


def gen_json_str(rng):
    for _ in range(4000):
        s = rand_str(rng, rng.randint(0, 12))
        yield [s], json.dumps(s, ensure_ascii=True)


def gen_bytes_repr(rng):
    for b in [b"", b"'", b'"', b"'\"", b"\\", bytes(range(256))]:
        yield [b], repr(b)
    for _ in range(4000):
        b = bytes(rng.choice([rng.randint(0, 255), 39, 34, 92, 10, 13, 9]) for _ in range(rng.randint(0, 12)))
        yield [b], repr(b)


def gen_utf8_ignore(rng):
    for b in [b"", b"\xff", b"\xc3", b"\xc3\xa9", b"\xe2\x82", b"\xe2\x82\xac", b"\xed\xa0\x80", b"\xf4\x90\x80\x80",
              b"\xf0\x9f\x98", b"\xc0\xaf", b"a\x80b"]:
        yield [b], b.decode("utf-8", "ignore")
    for _ in range(4000):
        b = bytes(rng.choice([rng.randint(0, 255), rng.randint(0x80, 0xBF), rng.randint(0xC0, 0xF7), 0x41])
                  for _ in range(rng.randint(0, 10)))
        yield [b], b.decode("utf-8", "ignore")


def ranges(pred):
    out, start = [], None
    for c in range(0x110000):
        if pred(c):
            if start is None:
                start = c
        elif start is not None:
            out.append((start, c - 1))
            start = None
    if start is not None:
        out.append((start, 0x10FFFF))
    return out


def gen_unicode(rng):
    yield ["unidata_version"], unicodedata.unidata_version
    yield ["nonprintable"], [list(r) for r in ranges(lambda c: not chr(c).isprintable())]
    yield ["space"], [list(r) for r in ranges(lambda c: chr(c).isspace())]
    yield ["decimal"], [[c, unicodedata.decimal(chr(c))] for c in range(0x110000) if chr(c).isdecimal()]
    yield ["bytes_isspace"], [c for c in range(256) if bytes([c]).isspace()]
    yield ["bytes_re_s"], [c for c in range(256) if re.match(rb"\s", bytes([c]))]


def gen_bytesio(rng):
    for _ in range(1000):
        data = bytes(rng.randint(0, 255) for _ in range(rng.randint(0, 20)))
        f = io.BytesIO(data)
        ops, results = [], []
        for _ in range(rng.randint(1, 8)):
            r = rng.random()
            if r < 0.4:
                op = ["seek", rng.randint(-5, 30), rng.choice([0, 0, 1, 2, 3])]
                res = call(f.seek, op[1], op[2])
            elif r < 0.8:
                op = ["read", rng.choice([None, -1, 0, 1, 3, 10, 100])]
                res = call(f.read, op[1])
            else:
                op = ["tell"]
                res = f.tell()
            ops.append(op)
            results.append(res)
        yield [data, ops], results


GENERATORS = {
    "exc_hierarchy": gen_exc_hierarchy,
    "int_parse": gen_int_parse,
    "int_str": gen_int_str,
    "int_to_float": gen_int_to_float,
    "binop": gen_binop,
    "compare": gen_compare,
    "sum": gen_sum,
    "minmax": gen_minmax,
    "float_repr": gen_float_repr,
    "round": gen_round,
    "float_parse": gen_float_parse,
    "str_repr": gen_str_repr,
    "json_str": gen_json_str,
    "bytes_repr": gen_bytes_repr,
    "utf8_ignore": gen_utf8_ignore,
    "unicode": gen_unicode,
    "bytesio": gen_bytesio,
}


def main():
    ap = argparse.ArgumentParser(prog="python -m oracle.unit.pycompat")
    ap.add_argument("--seed", type=int, default=20261014)
    args = ap.parse_args()
    require_oracle_python()
    counts = {name: write_cases("pycompat", name, gen(random.Random(f"{args.seed}:{name}"))) for name, gen in GENERATORS.items()}
    write_meta("pycompat", seed=args.seed, counts=counts)
    print(f"wrote {sum(counts.values())} cases: {counts}")


if __name__ == "__main__":
    main()
