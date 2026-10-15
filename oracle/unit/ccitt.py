"""Differential cases for pdfminer/ccitt.py (CCITT Group 4 fax decoding).

The Rust replay is crates/pdftrace/tests/ccitt_unit.rs. Files (typed JSON
lines ``{"in": [...], "out": ...}``, see ``oracle/unit/_common.py``):

- ``trees.jsonl``: ``[name]`` -> the class tree ``CCITTG4Parser.<name>``
  (nested ``[x, y]`` lists) for ``MODE``, ``WHITE``, ``BLACK``,
  ``UNCOMPRESSED``.
- ``add.jsonl``: ``[[[value, bits], ...]]`` -> ``[[result of each
  BitParser.add], tree]`` on a fresh ``[None, None]``: random codes,
  overwritten and prefix codes, codes through a leaf (errors), empty codes.
- ``ops.jsonl``: ``[width, bytealign, ops]`` -> ``[result of each op]``:
  a ``CCITTG4Parser`` (whose ``output_line`` prints: the printed text is
  part of the ``state`` results) driven through its internal methods
  (``_do_vertical``, ``_do_pass``, ``_do_horizontal``, ``_do_uncompressed``,
  ``_flush_line``, ``_reset_line``, ``reset``, the ``_parse_*`` methods
  called directly, ``_parse_bit``, ``feedbytes`` of ``CCITTG4Parser`` and of
  ``BitParser``, ``_get_bits``, ``_get_refline``), see ``run_ops``. Includes
  the cases of upstream ``tests/test_pdfminer_ccitt.py`` (with an
  ``array('b')`` reference line where the tests use a ``list``: the same
  values, the type only changes the text of an ``IndexError``).
- ``decode.jsonl``: ``[data, params]`` -> ``ccittfaxdecode(data, params)``
  (``output``): G4 streams of random bitmaps from a small T.6 encoder
  (``g4_encode``, test generation only: plain and byte-aligned lines,
  uncompressed-mode and invalid codes spliced in, end-of-block or not), from
  libtiff through Pillow (``compression="group4"`` and ``"group3"``),
  random bytes, truncated and bit-flipped streams, and every parameter
  value upstream reads (``K``, ``Columns``, ``EncodedByteAlign``,
  ``BlackIs1``) with ints, floats, bools, ``None``, literals, bytes,
  containers, references, and widths raising ``MemoryError``/
  ``OverflowError``.
- ``obj.jsonl``: ``[data, params]`` -> ``output``, for parameters that are
  not a ``dict`` (``None``, a list, a number, a literal, a ``PDFStream``
  whose ``get`` reads its attributes).
- ``deep.jsonl``: ``[depth, kind, data, params]`` -> ``output``, called
  at a measured depth (``_common.at_depth``) up to Python's frame limit:
  ``kind`` is ``dict`` or ``obj``.
- ``corpus.jsonl``: ``[source, data, params]`` -> ``output``, every
  ``ccittfaxdecode`` call made while decoding (``PDFStream.get_data``) the
  streams of the corpus set ``all`` whose filters include
  ``/CCITTFaxDecode``, and the inline images of the content streams that
  name it, with the data and the parameters pdfminer passes (one case per
  distinct pair). A call that runs more than ``CALL_TIMEOUT_S`` seconds in
  Python is left out (listed in ``meta.json``).

``output`` is the result bytes when at most 512 long, else ``("sha256",
len, hex digest)``, or the exception.

Usage: PDFPLUMBER_ROOT=<main checkout> <root>/.venv/bin/python -m oracle.unit.ccitt [-j 3]
"""

from __future__ import annotations

import argparse
import array
import contextlib
import hashlib
import io
import multiprocessing
import random
import re
import signal
import sys

from PIL import Image

import pdfminer.pdftypes
from pdfminer.ccitt import BitParser, CCITTG4Parser, ccittfaxdecode
from pdfminer.pdfinterp import PDFContentParser
from pdfminer.pdftypes import LITERALS_CCITTFAX_DECODE, PDFObjRef, PDFStream
from pdfminer.psparser import KWD, LIT

from ..corpus import load_set
from ..stages._common import open_document
from ._common import depth_here, require_oracle_python, write_cases, write_meta

MODULE = "ccitt"
CALL_TIMEOUT_S = 300
FILE_TIMEOUT_S = 900
TREES = {"MODE": CCITTG4Parser.MODE, "WHITE": CCITTG4Parser.WHITE, "BLACK": CCITTG4Parser.BLACK,
         "UNCOMPRESSED": CCITTG4Parser.UNCOMPRESSED}


# -- results ------------------------------------------------------------------


def output(f, *args):
    """``f(*args)`` as a case result: bytes (hashed beyond 512 bytes) or the
    exception."""
    try:
        return bytes_result(f(*args))
    except Exception as e:  # the exception is the result
        return e


def bytes_result(b):
    if len(b) <= 512:
        return b
    return ("sha256", len(b), hashlib.sha256(b).hexdigest())


# -- the class trees and BitParser.add ------------------------------------------


def node_paths():
    """``{id(list): "TREE:bits"}`` for every list of the class trees."""
    paths = {}
    for name, root in TREES.items():
        stack = [(root, "")]
        while stack:
            node, prefix = stack.pop()
            paths[id(node)] = f"{name}:{prefix}"
            for b in (0, 1):
                if isinstance(node[b], list):
                    stack.append((node[b], prefix + str(b)))
    return paths


def gen_trees():
    for name, root in TREES.items():
        yield [name], root


def gen_add(rng):
    values = [0, 1, -1, 2, 7, 64, 2560, "h", "e", "T00", "x", "ab", ""]
    for k in range(600):
        codes = []
        for _ in range(rng.randint(1, 12)):
            n = rng.choice([0, 1, 1, 2, 3, 4, 5, 8]) if k % 50 else rng.choice([0, 1])
            bits = "".join(rng.choice("01") for _ in range(n))
            if rng.random() < 0.03:
                bits = bits + "2"  # any other character is a 0
            codes.append([rng.choice(values), bits])
        root = [None, None]
        results = []
        for v, bits in codes:
            try:
                BitParser.add(root, v, bits)
                results.append(None)
            except Exception as e:  # the exception is the result
                results.append(e)
        yield [codes], [results, root]


# -- the parser through its methods -------------------------------------------------


ACCEPT_NAMES = {"_parse_mode", "_parse_horiz1", "_parse_horiz2", "_parse_uncompressed"}


def run_ops(width, bytealign, ops):
    """Applies ops to a fresh ``CCITTG4Parser(width, bytealign)``; each op
    returns a result (the exception it raised, if any). ``state`` gives
    ``[_curpos, _color, _y, _pos, _accept name, path of _state, _refline,
    _curline, _n1, _n2 (None while unset), text printed since the last
    state]``."""
    paths = node_paths()
    out = []
    printed = io.StringIO()
    with contextlib.redirect_stdout(printed):
        try:
            parser = CCITTG4Parser(width, bytealign=bytealign)
        except Exception as e:  # the exception is the result
            return [e]
        for op in ops:
            kind = op[0]
            try:
                if kind == "state":
                    text = printed.getvalue()
                    printed.seek(0)
                    printed.truncate()
                    name = parser._accept.__name__
                    assert name in ACCEPT_NAMES
                    out.append([parser._curpos, parser._color, parser._y, parser._pos, name,
                                paths[id(parser._state)], "".join(str(b) for b in parser._refline),
                                "".join(str(b) for b in parser._curline), getattr(parser, "_n1", None),
                                getattr(parser, "_n2", None), text])
                    continue
                if kind == "setcurline":
                    parser._curline = array.array("b", [int(c) for c in op[1]])
                    r = None
                elif kind == "setrefline":
                    parser._refline = array.array("b", [int(c) for c in op[1]])
                    r = None
                elif kind == "set":
                    setattr(parser, op[1], op[2])
                    r = None
                elif kind == "vertical":
                    r = parser._do_vertical(op[1])
                elif kind == "pass":
                    r = parser._do_pass()
                elif kind == "horizontal":
                    r = parser._do_horizontal(op[1], op[2])
                elif kind == "uncompressed":
                    r = parser._do_uncompressed(op[1])
                elif kind == "flush":
                    r = parser._flush_line()
                elif kind == "reset_line":
                    r = parser._reset_line()
                elif kind == "reset":
                    r = parser.reset()
                elif kind in ("mode", "horiz1", "horiz2", "uncomp"):
                    tree = getattr(parser, {"mode": "_parse_mode", "horiz1": "_parse_horiz1", "horiz2": "_parse_horiz2",
                                            "uncomp": "_parse_uncompressed"}[kind])(op[1])
                    r = paths[id(tree)]
                elif kind == "bit":
                    r = parser._parse_bit(op[1])
                elif kind == "feed":
                    r = parser.feedbytes(op[1])
                elif kind == "bitfeed":
                    r = BitParser.feedbytes(parser, op[1])
                elif kind == "bits":
                    r = parser._get_bits()
                elif kind == "refline_at":
                    r = parser._get_refline(op[1])
                else:
                    raise ValueError(kind)
                out.append(r)
            except Exception as e:  # the exception is the result
                out.append(e)
    return out


def upstream_vectors():
    """tests/test_pdfminer_ccitt.py: ``get_parser(bits)`` is a parser of
    width ``len(bits)`` whose reference line is ``bits``; each assert reads
    ``_curpos``, ``_color`` or ``_get_bits()`` (``state`` and ``bits``)."""
    def parser(bits, *ops):
        return [len(bits), False, [["setcurline", bits], ["reset_line"], *ops]]

    s, g = ["state"], ["bits"]
    return [
        parser("00000", ["vertical", 0], s),  # test_b1
        parser("10000", ["vertical", -1], s),  # test_b2
        parser("000111", ["pass"], s, g),  # test_b3
        parser("00000", ["vertical", 2], s, g),  # test_b4
        parser("11111111100", ["horizontal", 0, 3], s, ["vertical", 1], s, g),  # test_b5
        parser("10000", ["vertical", 0], s, ["vertical", 0], s, g),  # test_e1
        parser("10011", ["vertical", 0], s, ["vertical", 2], s, g),  # test_e2
        parser("011111", ["set", "_color", 0], ["vertical", 0], s, ["vertical", -2], s, ["vertical", 0], s, g),  # test_e3
        parser("10000", ["vertical", 0], s, ["vertical", -2], s, ["vertical", 0], s, g),  # test_e4
        parser("011000", ["set", "_color", 0], ["vertical", 0], s, ["vertical", 3], s, g),  # test_e5
        parser("11001", ["pass"], s, ["vertical", 0], s, g),  # test_e6
        parser("0000000000", ["set", "_curpos", 2], ["set", "_color", 1], ["horizontal", 2, 6], s, g),  # test_e7
        parser("001100000", ["set", "_curpos", 1], ["set", "_color", 0], ["vertical", 0], s, ["horizontal", 7, 0], s,
               g),  # test_e8
        parser("10101", ["pass"], s, ["pass"], s, g),  # test_m1
        parser("101011", ["vertical", -1], ["vertical", -1], ["vertical", 1], ["horizontal", 1, 1], g),  # test_m2
        parser("10111011", ["vertical", -1], ["pass"], ["vertical", 1], ["vertical", 1], g),  # test_m3
    ]


def random_bits(rng, n):
    return "".join(rng.choice("01") for _ in range(n))


def gen_ops(rng):
    for case in upstream_vectors():
        yield case, run_ops(*case)
    mode_values = [0, 1, -1, 2, -2, 3, -3, 9, -9, 100, -100, "p", "h", "u", "e", "x1", "x7", "zz", None]
    unc_values = ["1", "01", "001", "0001", "00001", "00000", "T00", "T10", "T000", "T100", "T0000", "T1000", "T00000",
                  "T10000", "", None, "T", "T1", "T2", "Tx", "0x", "0120"]
    for k in range(1500):
        width = rng.choice([-3, -1, 0, 1, 2, 3, 4, 5, 7, 8, 9, 11, 16]) if k % 7 else rng.choice([1, 5, 20])
        bytealign = rng.random() < 0.3
        ops = []
        if rng.random() < 0.6:
            w = max(width, 0)
            ops.append(["setcurline", random_bits(rng, w if rng.random() < 0.9 else rng.randint(0, w + 2))])
            ops.append(["reset_line"])
        for _ in range(rng.randint(1, 10)):
            r = rng.random()
            if r < 0.18:
                ops.append(["vertical", rng.choice([0, 0, 1, -1, 2, -2, 3, -3, 7, -7])])
            elif r < 0.28:
                ops.append(["pass"])
            elif r < 0.38:
                ops.append(["horizontal", rng.randint(-1, 6), rng.randint(-1, 6)])
            elif r < 0.44:
                ops.append(["uncompressed", random_bits(rng, rng.randint(0, 4))])
            elif r < 0.48:
                ops.append(["flush"])
            elif r < 0.5:
                ops.append([rng.choice(["reset_line", "reset"])])
            elif r < 0.56:
                field = rng.choice(["_color", "_color", "_curpos", "_curpos", "_y", "_n1", "_n2"])
                value = rng.choice([0, 1]) if field == "_color" else rng.randint(-2, max(width, 0) + 2)
                if rng.random() < 0.05:
                    value = rng.choice([2, -1, 200, -200])
                ops.append(["set", field, value])
            elif r < 0.6:
                ops.append(["setrefline", random_bits(rng, max(width, 0) if rng.random() < 0.8 else rng.randint(0, 6))])
            elif r < 0.68:
                ops.append(["mode", rng.choice(mode_values)])
            elif r < 0.71:
                ops.append([rng.choice(["horiz1", "horiz2"]), rng.choice([None, 0, 3, 63, 64, 128, 2560])])
            elif r < 0.74:
                ops.append(["uncomp", rng.choice(unc_values)])
            elif r < 0.78:
                ops.append(["bit", rng.choice([0, 1])])
            elif r < 0.86:
                ops.append(["feed", rng.randbytes(rng.randint(0, 4))])
            elif r < 0.88:
                ops.append(["bitfeed", rng.randbytes(rng.randint(0, 3))])
            elif r < 0.92:
                ops.append(["bits"])
            elif r < 0.95:
                ops.append(["refline_at", rng.randint(-2, max(width, 0) + 2)])
            else:
                ops.append(["state"])
        ops.append(["state"])
        yield [width, bytealign, ops], run_ops(width, bytealign, ops)


# -- a G4 (T.6) encoder, for test generation only ---------------------------------


def code_table(tree):
    """``{value: code}`` of a class tree."""
    codes = {}
    stack = [(tree, "")]
    while stack:
        node, prefix = stack.pop()
        for b in (0, 1):
            x = node[b]
            if isinstance(x, list):
                stack.append((x, prefix + str(b)))
            elif x is not None:
                codes[x] = prefix + str(b)
    return codes


MODE_CODES = code_table(CCITTG4Parser.MODE)
RUN_CODES = (code_table(CCITTG4Parser.WHITE), code_table(CCITTG4Parser.BLACK))
UNC_CODES = code_table(CCITTG4Parser.UNCOMPRESSED)
EOFB = MODE_CODES["e"]


def run_code(n, color):
    """The makeup and terminating codes of a run of ``n`` pixels of
    ``color`` (0 white, 1 black)."""
    codes = RUN_CODES[color]
    out = []
    while n >= 2560:
        out.append(codes[2560])
        n -= 2560
    if n >= 64:
        out.append(codes[n // 64 * 64])
        n %= 64
    out.append(codes[n])
    return "".join(out)


def g4_line(row, ref, width):
    """The T.6 codes of ``row`` (pixels, 1 black) under the reference line
    ``ref``."""
    def color_at(line, i):
        return line[i] if 0 <= i < width else 0

    def next_change(line, a, c):
        """First ``i > a`` (``i >= 0``) whose pixel is not ``c``."""
        i = max(a + 1, 0)
        while i < width and line[i] == c:
            i += 1
        return i

    def ref_change(a, c):
        """b1: first changing element of ``ref`` right of ``a`` whose color is
        not ``c``."""
        i = max(a + 1, 0)
        while i < width and not (ref[i] != c and color_at(ref, i - 1) == c):
            i += 1
        return i

    bits = []
    a0, c = -1, 0
    while a0 < width:
        a1 = next_change(row, a0, c)
        b1 = ref_change(a0, c)
        b2 = next_change(ref, b1, 1 - c)
        if b2 < a1:
            bits.append(MODE_CODES["p"])
            a0 = b2
        elif abs(a1 - b1) <= 3:
            bits.append(MODE_CODES[a1 - b1])
            a0, c = a1, 1 - c
        else:
            a2 = next_change(row, a1, 1 - c)
            bits.append(MODE_CODES["h"] + run_code(a1 - max(a0, 0), c) + run_code(a2 - a1, 1 - c))
            a0 = a2
    return "".join(bits)


def pack(bits):
    bits += "0" * (-len(bits) % 8)
    return bytes(int(bits[i:i + 8], 2) for i in range(0, len(bits), 8))


def random_rows(rng, width, height):
    rows = []
    prev = [0] * width
    style = rng.choice(["noise", "runs", "shift", "blank", "black", "mixed"])
    for _ in range(height):
        s = rng.choice(["noise", "runs", "shift", "blank", "black"]) if style == "mixed" else style
        if s == "noise":
            p = rng.choice([0.05, 0.3, 0.5, 0.9])
            row = [int(rng.random() < p) for _ in range(width)]
        elif s == "runs":
            row, c = [], rng.randint(0, 1)
            while len(row) < width:
                row += [c] * rng.choice([1, 2, 5, 63, 64, 65, 200, 2559, 2560, 2561, 2700])
                c = 1 - c
            row = row[:width]
        elif s == "shift":
            k = rng.randint(-4, 4)
            row = [prev[i - k] if 0 <= i - k < width else 0 for i in range(width)]
            for _ in range(rng.randint(0, 2)):
                if width:
                    row[rng.randrange(width)] ^= 1
        elif s == "blank":
            row = [0] * width
        else:
            row = [1] * width
        rows.append(row)
        prev = row
    return rows


def g4_encode(rng, rows, width, bytealign, eofb, splice):
    """Codes of the rows, each padded to a byte when ``bytealign``; with
    ``splice``, uncompressed-mode runs or random mode codes replace some
    lines."""
    out = []
    ref = [0] * width
    for row in rows:
        if splice and rng.random() < 0.2:
            if rng.random() < 0.6:
                codes = [MODE_CODES["u"]] + [UNC_CODES[rng.choice(["1", "01", "001", "0001", "00001", "00000"])]
                                              for _ in range(rng.randint(0, 6))]
                codes.append(UNC_CODES[rng.choice([k for k in UNC_CODES if k.startswith("T")])])
            else:
                codes = [MODE_CODES[rng.choice(list(MODE_CODES))] for _ in range(rng.randint(1, 4))]
            line = "".join(codes)
        else:
            line = g4_line(row, ref, width)
        out.append(line)
        if bytealign:
            out.append("0" * (-sum(len(x) for x in out) % 8))
        ref = row
    if eofb:
        out.append(EOFB)
    return pack("".join(out))


def pillow_g4(rng, width, height, compression):
    """The strip of a TIFF written by libtiff (Pillow)."""
    im = Image.new("1", (width, height), 1)
    px = im.load()
    for y, row in enumerate(random_rows(rng, width, height)):
        for x, v in enumerate(row):
            if v:
                px[x, y] = 0
    buf = io.BytesIO()
    im.save(buf, "TIFF", compression=compression, tiffinfo={278: height})
    tif = Image.open(io.BytesIO(buf.getvalue()))
    (off,), (n,) = tif.tag_v2[273], tif.tag_v2[279]
    return buf.getvalue()[off:off + n]


# -- ccittfaxdecode -------------------------------------------------------------------

WIDTHS = [1, 2, 3, 7, 8, 9, 15, 16, 17, 31, 32, 33, 63, 64, 65, 100, 127, 128, 200, 1728, 2560, 2600, 5200]
K_VALUES = [0, 1, 4, -2, True, False, -1.5, 1.0, LIT("K"), b"-1", [-1], {"K": -1}, 2**70, None, PDFObjRef(None, 7)]
COLUMN_VALUES = [None, 0, -1, -8, True, False, 8.0, LIT("C"), [8], b"8", 2**50, 2**55, 2**60 - 1, 2**60, 2**62,
                 2**63, -(2**63) - 1, 2**64, PDFObjRef(None, 3), KWD(b"x")]
FLAG_VALUES = [True, False, 0, 1, 2, 0.0, 0.5, None, LIT("x"), b"", b"0", [], [0], {}, {"a": 1}, PDFObjRef(None, 1)]


def params_for(rng, width, bytealign, k=-1):
    """Parameters as a PDF dictionary: present keys in a random order."""
    items = [("K", k), ("Columns", width)]
    if bytealign or rng.random() < 0.3:
        items.append(("EncodedByteAlign", bytealign))
    if rng.random() < 0.5:
        items.append(("BlackIs1", rng.random() < 0.5))
    for key in ("Rows", "EndOfBlock", "EndOfLine", "DamagedRowsBeforeError"):
        if rng.random() < 0.1:
            items.append((key, rng.choice([0, 1, True, False, 7])))
    rng.shuffle(items)
    return dict(items)


def mutate(rng, data):
    r = rng.random()
    if not data:
        return data
    if r < 0.3:
        return data[: rng.randrange(len(data))]
    if r < 0.6:
        b = bytearray(data)
        for _ in range(rng.randint(1, 3)):
            i = rng.randrange(len(b) * 8)
            b[i // 8] ^= 0x80 >> (i % 8)
        return bytes(b)
    if r < 0.8:
        return data + rng.randbytes(rng.randint(1, 8))
    return rng.randbytes(rng.randint(1, 4)) + data


def gen_decode(rng):
    def case(data, params):
        return [data, params], output(ccittfaxdecode, data, params)

    # encoder streams, with the parameters they were made for or others
    for k in range(2400):
        width = rng.choice(WIDTHS[:20]) if k % 10 else rng.choice(WIDTHS)
        height = rng.choice([0, 1, 1, 2, 3, 5, 8, 13]) if width <= 200 else rng.choice([1, 2, 3])
        bytealign = rng.random() < 0.3
        rows = random_rows(rng, width, height)
        data = g4_encode(rng, rows, width, bytealign, eofb=rng.random() < 0.7, splice=rng.random() < 0.25)
        if rng.random() < 0.3:
            data = mutate(rng, data)
        cols = width
        if rng.random() < 0.1:
            cols = max(width + rng.choice([-9, -1, 1, 8]), 0)
        ba = bytealign if rng.random() < 0.9 else not bytealign
        yield case(data, params_for(rng, cols, ba))
    # libtiff's encoders: G4 (K -1) and G3 (K 0 and K 4: PDFValueError)
    for k in range(300):
        width = rng.choice(WIDTHS[:20])
        height = rng.choice([1, 2, 5, 13])
        if k % 5 == 4:
            data = pillow_g4(rng, width, height, "group3")
            yield case(data, params_for(rng, width, False, rng.choice([0, 4])))
            continue
        data = pillow_g4(rng, width, height, "group4")
        if rng.random() < 0.2:
            data = mutate(rng, data)
        yield case(data, params_for(rng, width, False))
    # random bytes, and code soups
    for k in range(600):
        width = rng.choice(WIDTHS[:16])
        if k % 2:
            data = rng.randbytes(rng.choice([0, 1, 2, 5, 20, 100]))
        else:
            pool = list(MODE_CODES.values()) + [EOFB[:12], "0000000", "1" * 8]
            data = pack("".join(rng.choice(pool) for _ in range(rng.randint(1, 30))))
        yield case(data, params_for(rng, width, rng.random() < 0.3))
    # every parameter value upstream reads
    base = g4_encode(rng, random_rows(rng, 17, 3), 17, False, True, False)
    aligned = g4_encode(rng, random_rows(rng, 17, 3), 17, True, True, False)
    for k in K_VALUES + [-1, -1.0]:
        yield case(base, {"K": k, "Columns": 17})
    yield case(base, {"Columns": 17})
    yield case(base, {})
    for cols in COLUMN_VALUES:
        yield case(base, {"K": -1, "Columns": cols})
        yield case(b"", {"K": -1, "Columns": cols})
    yield case(base, {"K": -1})
    for flag in FLAG_VALUES:
        for data in (base, aligned):
            yield case(data, {"K": -1, "Columns": 17, "EncodedByteAlign": flag})
            yield case(data, {"K": -1, "Columns": 17, "BlackIs1": flag})
    # widths near the 8-pixel packing and byte-aligned lines of every width
    for width in range(0, 26):
        for bytealign in (False, True):
            rows = random_rows(rng, width, 4)
            data = g4_encode(rng, rows, width, bytealign, True, False)
            yield case(data, {"K": -1, "Columns": width, "EncodedByteAlign": bytealign, "BlackIs1": width % 2 == 0})


def gen_obj(rng):
    data = g4_encode(rng, random_rows(rng, 9, 2), 9, False, True, False)
    stream = PDFStream({"K": -1, "Columns": 9, "BlackIs1": True}, b"")
    for params in [None, [], [{"K": -1}], 0, -1, 1.5, True, LIT("P"), KWD(b"P"), b"K", PDFObjRef(None, 4), stream,
                   PDFStream({"K": 0}, b"x"), PDFStream({}, b""), PDFStream({"K": -1, "Columns": None}, b""),
                   {"K": -1, "Columns": 9}]:
        yield [data, params], output(ccittfaxdecode, data, params)


# -- recursion depth ------------------------------------------------------------------


def call_at(n, c_left, f, *args):
    """``f(*args)`` (or the exception it raised) called ``n`` frames below
    this one, with the depth it was called at, as ``_common.at_depth``
    gives it. ``at_depth`` cannot be used here: its ``depth_here``
    measures the C units left with nested calls, which run out of Python
    frames this close to the limit. The C units left are those of the first
    ``call_at`` (``c_left``): a Python-to-Python call takes none."""
    if n:
        return call_at(n - 1, c_left, f, *args)
    frame, py = sys._getframe(), 0
    while frame is not None:
        py += 1
        frame = frame.f_back
    depth = {"py": py, "c_left": c_left}
    try:
        return depth, f(*args)
    except Exception as e:  # the exception is the expected result
        return depth, e


def gen_deep(rng):
    line = g4_encode(rng, [[0, 1, 1, 0, 0, 0, 1, 0]], 8, False, False, False)
    unc = pack(MODE_CODES["u"] + UNC_CODES["1"] * 9 + UNC_CODES["T00"])
    jobs = [
        ("dict", b"", {"K": -1, "Columns": 8}),
        ("dict", b"\x00", {"K": -1, "Columns": 8}),
        ("dict", b"\x80", {"K": -1, "Columns": 8}),  # V0: a line at once
        ("dict", b"\x08", {"K": -1, "Columns": 8}),  # VL2: no line
        ("dict", b"\x20\x00", {"K": -1, "Columns": 8}),  # H then white runs
        ("dict", line, {"K": -1, "Columns": 8}),
        ("dict", line, {"K": -1, "Columns": 8, "EncodedByteAlign": True}),
        ("dict", pack(EOFB), {"K": -1, "Columns": 8}),
        ("dict", b"\x02\x00", {"K": -1, "Columns": 8}),  # x1: InvalidData
        ("dict", unc, {"K": -1, "Columns": 8}),
        ("dict", unc, {"K": -1, "Columns": 4}),
        ("dict", b"", {"K": 0}),
        ("dict", b"", {"K": -1, "Columns": None}),
        ("dict", b"", {"K": -1, "Columns": 2**60}),
        ("obj", b"\x80", PDFStream({"K": -1, "Columns": 8}, b"")),
        ("obj", b"", PDFStream({"K": 1}, b"")),
        ("obj", b"", None),
    ]
    here = depth_here()
    base = here["py"] + 1  # call_at's first frame
    for kind, data, params in jobs:
        for top in range(990, 1001):  # the frame number ccittfaxdecode gets
            # call_at(n) takes n + 1 frames, ccittfaxdecode its own
            depth, out = call_at(top - base - 1, here["c_left"], ccittfaxdecode, data, params)
            assert depth["py"] == top - 1, (depth, top)
            yield [depth, kind, data, params], (bytes_result(out) if isinstance(out, bytes) else out)


# -- real corpus ----------------------------------------------------------------------


class Timeout(BaseException):
    """The time limit (a ``BaseException``: pdfminer's ``except Exception``
    must not swallow it)."""


class Recorded(Exception):
    """Raised by the recording wrapper: the call is replayed afterwards."""


def on_alarm(signum, frame):
    raise Timeout()


def corpus_calls(path, password):
    """``(source, data, params)`` of every ``ccittfaxdecode`` call made by
    pdfminer while decoding the CCITT streams of a file and the inline
    images of its content streams (the function is wrapped only to record
    its arguments)."""
    calls = []
    real = pdfminer.pdftypes.ccittfaxdecode
    where = [""]

    def record(data, params):
        calls.append((where[0], bytes(data), params))
        raise Recorded()  # stop: the case is computed afterwards, with a time limit

    def has_ccitt(stream):
        try:
            return any(f in LITERALS_CCITTFAX_DECODE for f, _ in stream.get_filters())
        except Exception:  # unreadable filter list: pdfminer would fail too
            return False

    pdfminer.pdftypes.ccittfaxdecode = record
    try:
        with open_document(path, password) as doc:
            for xref in doc.xrefs:
                for objid in xref.get_objids():
                    try:
                        obj = doc.getobj(objid)
                    except Exception:
                        continue
                    if not isinstance(obj, PDFStream):
                        continue
                    if obj.get("Subtype") is LIT("Image") and not has_ccitt(obj):
                        continue  # no inline image in an image XObject
                    where[0] = f"obj {objid}"
                    if has_ccitt(obj):
                        with contextlib.suppress(Exception):
                            obj.get_data()
                        continue
                    try:
                        data = obj.get_data()
                    except Exception:
                        continue
                    if not re.search(rb"/CCF\b|/CCITTFaxDecode", data) or b"ID" not in data:
                        continue
                    parser = PDFContentParser([PDFStream({}, data)])
                    while True:
                        try:
                            _, x = parser.nextobject()
                        except Exception:
                            break
                        if isinstance(x, PDFStream) and has_ccitt(x):
                            where[0] = f"obj {objid} inline image"
                            with contextlib.suppress(Exception):
                                x.get_data()
    finally:
        pdfminer.pdftypes.ccittfaxdecode = real
    return calls


def process_file(args):
    path, rel, password = args
    signal.signal(signal.SIGALRM, on_alarm)
    signal.alarm(FILE_TIMEOUT_S)
    try:
        calls = corpus_calls(path, password)
    except Timeout:
        return rel, "timeout", []
    except Exception as e:  # unreadable document
        return rel, type(e).__name__, []
    finally:
        signal.alarm(0)
    out = []
    for source, data, params in calls:
        signal.alarm(CALL_TIMEOUT_S)
        try:
            result = output(ccittfaxdecode, data, params)
        except Timeout:
            result = None
        finally:
            signal.alarm(0)
        out.append((f"{rel}#{source}", data, params, result))
    return rel, None, out


def gen_corpus(jobs, skipped):
    files = load_set("all")
    tasks = []
    for f in files:
        with open(f.path, "rb") as fp:
            raw = fp.read()
        # a stream dictionary is never compressed; inline images in a
        # compressed content stream are found by decoding the streams of
        # the files that name the filter anywhere
        if re.search(rb"/CCF\b|CCITTFax", raw) or b"/ObjStm" in raw or b"/FlateDecode" in raw:
            tasks.append((str(f.path), f.rel, f.password))
    seen = set()
    with multiprocessing.Pool(jobs) as pool:
        for rel, status, out in pool.imap(process_file, tasks):
            if status is not None:
                skipped.append([rel, status])
                print(f"skip {rel}: {status}", file=sys.stderr)
            for source, data, params, result in out:
                key = hashlib.sha256(data).hexdigest() + repr(params)
                if key in seen:
                    continue
                seen.add(key)
                if result is None:
                    skipped.append([source, "call timeout"])
                    print(f"skip {source}: call timeout", file=sys.stderr)
                    continue
                yield [source, data, params], result


def main():
    ap = argparse.ArgumentParser(prog="python -m oracle.unit.ccitt")
    ap.add_argument("-j", "--jobs", type=int, default=3, help="worker processes for the real corpus")
    ap.add_argument("--no-corpus", action="store_true", help="skip the real corpus (for a quick look)")
    args = ap.parse_args()
    require_oracle_python()
    rng = random.Random("20261015:ccitt")
    counts = {}
    counts["trees"] = write_cases(MODULE, "trees", gen_trees())
    counts["add"] = write_cases(MODULE, "add", gen_add(rng))
    counts["ops"] = write_cases(MODULE, "ops", gen_ops(rng))
    counts["decode"] = write_cases(MODULE, "decode", gen_decode(rng))
    counts["obj"] = write_cases(MODULE, "obj", gen_obj(rng))
    counts["deep"] = write_cases(MODULE, "deep", gen_deep(rng))
    skipped = []
    if not args.no_corpus:
        counts["corpus"] = write_cases(MODULE, "corpus", gen_corpus(args.jobs, skipped))
    write_meta(MODULE, cases=counts, skipped=skipped)
    print("wrote", counts, f"({len(skipped)} skipped)")


if __name__ == "__main__":
    main()
