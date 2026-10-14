"""Cases for cross-testing ``canon.rs`` / ``typed_json.rs`` against ``oracle/canon.py``.

    PYTHONPATH=. .venv/bin/python -m oracle.unit.canon [--seed 1] [--random 6000] [--out FILE]

Writes ``<cache>/unit/canon/cases.jsonl`` (``<cache>`` = ``PDFPLUMBER_CACHE``
or ``<PDFPLUMBER_ROOT>/.cache``): one JSON object per line,

    {"value": <typed JSON of the value>, "canon": "<canon(value)>"}

The Rust side decodes ``value`` with its typed JSON reader, writes it with its
``canon`` and must produce ``canon`` exactly; it can also re-encode ``value``
and compare with the line (``dumps`` output is canonical: compact, ASCII, key
order fixed).

Cases: a fixed list of edge cases for every type of the canon table (ints
around 2**31/2**53/2**63/2**64 and past CPython's 4300-digit ``str()`` limit;
floats at the ``repr`` exponent thresholds, subnormals, min/max normal,
signed zeros, infinities, NaNs with sign and payload bits; strings with
quotes, controls, DEL, U+2028, BOM, non-BMP, lone and reversed surrogates;
bytes of 0/255/256/257 bytes; names with str and bytes names; references;
streams with and without rawdata; real exceptions, including ones whose
message holds a memory address; ``Named`` objects; nested containers), then
``--random`` values drawn with ``random.Random(seed)``: random IEEE-754 bit
patterns, random decimals, random strings over a mix of code point classes,
and random nested structures of all of these. Every case is checked to
round-trip through typed JSON with the same canon before it is written.
"""

from __future__ import annotations

import argparse
import json
import random
import struct
import sys
import zlib
from collections import Counter
from collections.abc import Iterator
from pathlib import Path
from typing import Any

from pdfminer.pdfdocument import PDFPasswordIncorrect
from pdfminer.pdfexceptions import PDFObjectNotFound
from pdfminer.pdfparser import PDFSyntaxError
from pdfminer.pdftypes import PDFObjRef, PDFStream
from pdfminer.psparser import KWD, LIT, PSEOF

from ..canon import Named, canon
from ..corpus import cache_dir
from ..typed_json import dumps, loads


def _f(bits: int) -> float:
    return struct.unpack(">d", struct.pack(">Q", bits))[0]


def _stream(attrs: dict[str, Any], rawdata: bytes | None, objid: Any = None, genno: Any = None, data: bytes | None = None) -> PDFStream:
    s = PDFStream(attrs, b"")
    s.rawdata = rawdata
    s.data = data
    s.objid = objid
    s.genno = genno
    return s


def _exc(factory: Any) -> BaseException:
    try:
        factory()
    except BaseException as e:  # noqa: BLE001 - we want the real object
        return e
    raise AssertionError("factory did not raise")


def _recursion() -> None:
    def f() -> None:
        f()

    f()


INTS = [
    0, 1, -1, 7, -7, 255, 256, 65535, 2**31 - 1, 2**31, -(2**31), -(2**31) - 1, 2**32,
    2**53 - 1, 2**53, 2**53 + 1, -(2**53) - 1, 2**63 - 1, 2**63, -(2**63), -(2**63) - 1,
    2**64 - 1, 2**64, 10**18, 10**19, -(10**19), 10**100, 10**4299, 10**4300 - 1, 10**4300,
    -(10**4300), 10**5000 + 123456789, -(10**5000) - 1, 2**20000,
]  # fmt: skip

FLOAT_BITS = [
    0x0000000000000000, 0x8000000000000000,  # +0.0, -0.0
    0x0000000000000001, 0x8000000000000001,  # smallest subnormals
    0x000FFFFFFFFFFFFF, 0x0010000000000000,  # largest subnormal, smallest normal
    0x7FEFFFFFFFFFFFFF, 0xFFEFFFFFFFFFFFFF,  # +-max
    0x7FF0000000000000, 0xFFF0000000000000,  # +-inf
    0x7FF8000000000000, 0xFFF8000000000000,  # quiet NaNs, both signs
    0x7FF0000000000001, 0x7FF8000000000001, 0xFFFFFFFFFFFFFFFF,  # NaN payloads
    0x3FF0000000000000, 0x3FF0000000000001, 0x3FEFFFFFFFFFFFFF,  # around 1.0
]  # fmt: skip

FLOATS = [
    1.0, -1.0, 0.1, 0.2, 0.1 + 0.2, 1 / 3, 2 / 3, 0.5, 1.5, 2.5, -2.5, 100.0, 123.456, 1e-7,
    1e16, 1e16 - 2, 9999999999999998.0, 1e15, 123456789012345.6, 1e17, 1e22, 1e23, 1e100, 1e308,
    1e-4, 0.0001, 9.999999999999999e-05, 0.00011, 1e-5, 1.5e-5, 1e-300, 5e-324, 2.5e-323,
    float(2**53), float(2**53 + 2), float(2**63), float(2**64), float(10**18),
    612.0, 792.0, 0.001, 72.0, 595.276, 841.89, 11.955, -0.000001, 3.14159, 1e-320,
]  # fmt: skip

STRINGS = [
    "", "a", "F1", '"', "\\", "/", "'", "\x00", "\x01", "\b\f\n\r\t", "\x1f", " ", "\x7f", "\x80", "\x9f",
    "\xa0", "\xff", "é", "café", "naïve", "Ω", "中文", "\u2028", "\u2029", "\ufeff", "\ufffd", "\uffff",
    "\U0001f600", "\U00010000", "\U0010ffff", "a\U0001f600b", "\ud800", "\udbff", "\udc00", "\udfff",
    "\ud83d\ude00", "\ude00\ud83d", "x\ud800y", "\ud800\ud800", "<>&", "</script>", "(cid:12)",
    "a" * 300, "é" * 100,
]  # fmt: skip

BYTES = [
    b"", b"\x00", b"\xff", b"BT", b"\r\n", b"\\", b'"', bytes(range(16)), bytes(range(255)),
    bytes(range(256)), bytes(range(256)) + b"\x00", bytes(255 - i for i in range(256)) * 2, b"\x00" * 4096, b"x" * 100000,
]  # fmt: skip


def edge_cases() -> Iterator[Any]:
    yield from (None, True, False)
    yield from INTS
    yield from (_f(b) for b in FLOAT_BITS)
    yield from FLOATS
    yield from (-x for x in FLOATS)
    yield from STRINGS
    yield from BYTES
    for name in ["F1", "", "café", "a b", "Name#20", "\ud800", "\U0001f600", "x" * 300]:
        yield LIT(name)
    for bname in [b"F1", b"", b"\xff\xfe", b"caf\xc3\xa9", b"\x00", bytes(range(256)), b"y" * 300]:
        yield LIT(bname)
    for kw in [b"BT", b"Tj", b"T*", b"'", b'"', b"", b"\xff", b"<<", b"z" * 257]:
        yield KWD(kw)
    for objid in [0, 1, 12, 65535, 2**31, 2**40, -1, 2**63 - 1, 2**63, 2**70, -(2**64)]:
        yield PDFObjRef(None, objid)
    yield _stream({}, b"", 1, 0)
    yield _stream({"Length": 42}, b"x" * 42, 5, 0)
    yield _stream({"Length": 300, "Filter": LIT("FlateDecode")}, bytes(300), 7, 0)
    yield _stream({"Filter": [LIT("A85"), LIT("Fl")], "DecodeParms": [None, {"Predictor": 12}]}, None, 9, 3, data=b"decoded")
    yield _stream({"W": 1, "H": 1, "BPC": 8, "CS": LIT("RGB")}, b"\xff\x00\x00")  # inline image
    yield _stream({"Length": PDFObjRef(None, 4)}, b"abc", 2**40, 65535)
    yield _stream({}, None, None, None)
    exceptions = [
        PSEOF("Unexpected EOF"),
        PSEOF("Unexpected EOF, file truncated?"),
        PDFSyntaxError("objid mismatch: 3=4"),
        PDFObjectNotFound(5),
        PDFPasswordIncorrect(),
        KeyError("Length"),
        KeyError(5),
        _exc(lambda: zlib.decompress(b"not zlib")),
        _exc(lambda: struct.unpack(">L", b"ab")),
        _exc(lambda: b"\xff".decode("utf-8")),
        _exc(lambda: int("x" * 5)),
        _exc(lambda: [][1]),
        _exc(lambda: None + 1),  # type: ignore[operator]
        _exc(lambda: 1 / 0),
        _exc(_recursion),
        AssertionError(),
        ValueError("Trailer not found: <pdfminer.pdfparser.PDFParser object at 0x7f3a2b1c0d10>: line=b'x'"),
        ValueError("two <a.B object at 0xDEADbeef> and <c.D object at 0x1>, not <at 0x12>"),
        TypeError("caf\xe9 \U0001f600 \"quoted\" \\ \n"),
        Exception("a", 2),
        Exception(),
    ]
    yield from exceptions
    yield Named("LTChar", {"x0": 1.0, "text": "a", "fontname": "Helvetica", "upright": True})
    yield Named("Cell", {})
    yield [Named("LTLine", {"pts": [(0.0, 1.0), (2.0, 3.0)]}), Named("LTRect", {"fill": False})]
    yield from ([], (), {}, (1,), (1, 2), [None], [[]], [()], ((),), {"a": []}, {"": None})
    yield {1: 2, (1, 2.5): 3, b"k": 4, None: 5, "s": 6, 1.5: 7, LIT("L"): 8}
    yield {"Type": LIT("Page"), "MediaBox": [0, 0, 612.0, 792], "Contents": PDFObjRef(None, 4), "Annots": None}
    yield [exceptions[0], (exceptions[3], [exceptions[5]])]
    deep: Any = 0
    for _ in range(300):
        deep = [deep]
    yield deep
    deep = "leaf"
    for k in range(200):
        deep = {"k": deep} if k % 2 else (deep,)
    yield deep


# ---------------------------------------------------------------------- random values

_CODEPOINT_CLASSES = [
    (0x20, 0x7E), (0x00, 0x1F), (0x7F, 0x9F), (0xA0, 0x2FF), (0x370, 0x3FF), (0x4E00, 0x9FFF),
    (0x2000, 0x206F), (0xD800, 0xDFFF), (0xE000, 0xFFFF), (0x10000, 0x10FFFF),
]  # fmt: skip


def random_float(rng: random.Random) -> float:
    k = rng.randrange(6)
    if k == 0:
        return _f(rng.getrandbits(64))
    if k == 1:  # subnormal
        return _f(rng.getrandbits(52) | (rng.getrandbits(1) << 63))
    if k == 2:  # decimal with a few digits, as in PDF coordinates
        return round(rng.uniform(-2000, 2000), rng.randrange(0, 6))
    if k == 3:  # integer-valued
        return float(rng.randrange(-(2**60), 2**60) >> rng.randrange(0, 60))
    if k == 4:  # around the repr exponent thresholds 1e-4 and 1e16
        return rng.choice([1e-4, 1e16, 1e-5, 1e15, 1e17]) * rng.uniform(0.5, 2)
    return rng.uniform(-1, 1) * 10.0 ** rng.randrange(-320, 309)


def random_int(rng: random.Random) -> int:
    bits = rng.choice([1, 4, 8, 16, 31, 32, 33, 53, 54, 63, 64, 65, 100, 200])
    return rng.getrandbits(bits) * rng.choice([1, -1])


def random_str(rng: random.Random) -> str:
    chars = []
    for _ in range(rng.randrange(0, 24)):
        lo, hi = rng.choice(_CODEPOINT_CLASSES)
        chars.append(chr(rng.randint(lo, hi)))
    return "".join(chars)


def random_bytes(rng: random.Random) -> bytes:
    n = rng.choice([0, 1, 2, 5, 16, 100, 255, 256, 257, 258, 300, 1000, rng.randrange(0, 600)])
    return rng.randbytes(n)


def random_leaf(rng: random.Random) -> Any:
    k = rng.randrange(11)
    if k == 0:
        return rng.choice([None, True, False])
    if k == 1:
        return random_int(rng)
    if k in (2, 3):
        return random_float(rng)
    if k == 4:
        return random_str(rng)
    if k == 5:
        return random_bytes(rng)
    if k == 6:
        return LIT(random_str(rng) if rng.random() < 0.6 else rng.randbytes(rng.randrange(0, 8)))
    if k == 7:
        return KWD(rng.randbytes(rng.randrange(0, 6)))
    if k == 8:
        return PDFObjRef(None, rng.randrange(0, 100000))
    if k == 9:
        return ValueError(random_str(rng))
    return _stream({"Length": rng.randrange(0, 400)}, random_bytes(rng) if rng.random() < 0.8 else None, rng.randrange(1, 999), 0)


def random_value(rng: random.Random, depth: int = 0) -> Any:
    if depth >= 4 or rng.random() < 0.45:
        return random_leaf(rng)
    k = rng.randrange(4)
    n = rng.randrange(0, 5)
    if k == 0:
        return [random_value(rng, depth + 1) for _ in range(n)]
    if k == 1:
        return tuple(random_value(rng, depth + 1) for _ in range(n))
    if k == 2:
        return {random_str(rng) if rng.random() < 0.9 else random_int(rng): random_value(rng, depth + 1) for _ in range(n)}
    return Named(rng.choice(["LTChar", "LTCurve", "Cell", "Edge"]), {random_str(rng): random_value(rng, depth + 1) for _ in range(n)})


def random_cases(seed: int, count: int) -> Iterator[Any]:
    rng = random.Random(seed)
    makers = [random_float, random_float, random_int, random_str, random_bytes, random_value, random_value]
    for i in range(count):
        yield makers[i % len(makers)](rng)


def kind(v: Any) -> str:
    return type(v).__name__ if not isinstance(v, BaseException) else "exception"


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(prog="python -m oracle.unit.canon", description=__doc__.split("\n\n")[0])
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--random", type=int, default=6000, help="number of random values after the edge cases")
    ap.add_argument("--out", type=Path, default=None)
    args = ap.parse_args(argv)
    out = args.out or cache_dir() / "unit" / "canon" / "cases.jsonl"
    out.parent.mkdir(parents=True, exist_ok=True)
    kinds: Counter[str] = Counter()
    tmp = out.with_name(out.name + ".tmp")
    with open(tmp, "w", encoding="ascii") as fh:
        for v in [*edge_cases(), *random_cases(args.seed, args.random)]:
            text = canon(v)
            encoded = dumps(v)
            again = loads(encoded)
            if canon(again) != text or dumps(again) != encoded:
                raise AssertionError(f"typed JSON round trip changes {v!r}")
            fh.write('{"value":' + encoded + ',"canon":' + json.dumps(text) + "}\n")
            kinds[kind(v)] += 1
    tmp.replace(out)
    total = sum(kinds.values())
    print(f"{total} cases -> {out}", file=sys.stderr)
    print("  " + ", ".join(f"{k} {n}" for k, n in kinds.most_common()), file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
