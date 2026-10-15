"""Differential cases for crates/pdfminer/src/cmapdb.rs, encodingdb.rs,
glyphlist.rs, latin_enc.rs and fontmetrics.rs, computed by pdfminer.

Writes typed JSON lines (docs/verification.md) to
``<cache>/unit/cmaps/<fingerprint>/<name>.jsonl`` (``oracle/unit/_common.py``);
the Rust replay is crates/pdftrace/tests/cmaps_unit.rs. The predefined CMaps
are read by the Rust side from the compact files of
``oracle/codegen/cmaps.py``, whose directory (relative to the cache) is
``cmap_dir`` in ``meta.json``.

Default run (seconds):

- ``tables``: ``glyphname2unicode``, ``ENCODING`` and ``FONT_METRICS`` in full.
- ``name2unicode``: every glyph name, with suffixes and ``_`` ligatures, and
  generated names: ``uniXXXX...`` and ``uXXXX[XX]`` with every boundary
  (surrogates, U+10FFFF, ``str.strip`` quirks, ``int()`` accepting ``0x``,
  ``_``, spaces and non-ASCII digits), random names, and ``bytes`` names.
- ``encoding``: ``EncodingDB.get_encoding(name, diff)`` with generated
  ``Differences`` arrays (codes, names, bad names, other objects); the result
  as its items (keys through ``int()``, see cmapdb.rs) and whether it is the
  class dict itself.
- ``methods``: scripts of method calls on each class (``add_code2cid``,
  ``add_cid2unichr`` with every kind of object, ``use_cmap``, ``set_attr``,
  ``decode``, ``get_unichr``, ``is_vertical``, ``repr``, ``dump``), each
  followed by the state of the object.
- ``predefined``: every predefined CMap and unicode map through ``CMapDB``
  (attributes, full maps, decoding of every single byte, of every code of
  the map, of every two-byte code and of random strings, ``get_unichr``),
  the identity CMaps, missing names and names that are paths
  (``../cmap/H``), and the identity of cached objects.
- ``parse``: ``CMapParser(target, BytesIO(data)).run()`` on generated
  adversarial CMaps (``--fuzz N`` jobs, then fixed jobs: each construct that
  raises, alone), into every kind of target: the exception, the state, the
  warnings, and ``decode``/``get_unichr`` of samples.

``--corpus`` (minutes, every PDF of the set ``all``, ``-j`` workers): every
embedded CMap stream (``ToUnicode`` and ``Encoding`` streams of fonts, and
``/Type /CMap`` streams), de-duplicated, parsed into a ``FileUnicodeMap``
and a ``FileCMap``. Writes ``corpus.jsonl`` and ``corpus.json``.

Large outputs (``parse``, ``corpus``, ``predefined``) are compared through
their canonical text (``oracle.canon``): the text itself when it is short,
else ``"sha256:<hex>:<length>"``. ``--dump NAME INDEX`` prints the full
canonical text of one such case.

Usage: PDFPLUMBER_ROOT=<main checkout> <root>/.venv/bin/python -m oracle.unit.cmaps [--fuzz 3000] [--corpus]
"""

from __future__ import annotations

import argparse
import collections
import hashlib
import io
import json
import multiprocessing
import random
import signal
import sys
import time

from pdfminer.cmapdb import (
    CMap,
    CMapBase,
    CMapDB,
    CMapParser,
    FileCMap,
    FileUnicodeMap,
    IdentityCMap,
    IdentityCMapByte,
    IdentityUnicodeMap,
    UnicodeMap,
)
from pdfminer.encodingdb import EncodingDB, name2unicode
from pdfminer.fontmetrics import FONT_METRICS
from pdfminer.glyphlist import glyphname2unicode
from pdfminer.latin_enc import ENCODING
from pdfminer.pdftypes import PDFStream, resolve1
from pdfminer.psparser import KWD, LIT

from ..canon import canon
from ..codegen import cmaps as codegen_cmaps
from ..corpus import cache_dir, load_set
from ..stages._common import open_document
from ._common import depth_here, oracle_sources, require_oracle_python, unit_dir, write_cases, write_meta

MODULE = "cmaps"
FILE_TIMEOUT_S = 120
#: Outputs longer than this are compared through their SHA-256.
INLINE_LIMIT = 4000

CLASSES = {
    "CMapBase": CMapBase,
    "CMap": CMap,
    "FileCMap": FileCMap,
    "IdentityCMap": IdentityCMap,
    "IdentityCMapByte": IdentityCMapByte,
    "UnicodeMap": UnicodeMap,
    "IdentityUnicodeMap": IdentityUnicodeMap,
    "FileUnicodeMap": FileUnicodeMap,
}
CMAP_CLASSES = ("CMap", "FileCMap")
UMAP_CLASSES = ("UnicodeMap", "IdentityUnicodeMap", "FileUnicodeMap")


def attempt(f, *args):
    try:
        return f(*args)
    except Exception as e:  # the exception is the expected result
        return e


def canon_or_hash(v) -> str:
    text = canon(v)
    if len(text) <= INLINE_LIMIT:
        return text
    return f"sha256:{hashlib.sha256(text.encode('ascii')).hexdigest()}:{len(text)}"


# -- state of a map -------------------------------------------------------------


def tree(d):
    """``code2cid`` as nested ``[key, cid | tree]`` lists, in dict order."""
    return [[k, tree(v) if isinstance(v, dict) else v] for k, v in d.items()]


def state(m):
    """``[attrs items, code2cid tree or None, cid2unichr items or None]``;
    keys of ``cid2unichr`` through ``int()`` (a ``bool`` key is ``0``/``1``
    in the port)."""
    attrs = [[k, v] for k, v in m.attrs.items()]
    code2cid = tree(m.code2cid) if hasattr(m, "code2cid") else None
    cid2unichr = [[int(k), v] for k, v in m.cid2unichr.items()] if hasattr(m, "cid2unichr") else None
    return [attrs, code2cid, cid2unichr]


def decode_list(m, code):
    return list(m.decode(code))


# -- tables ---------------------------------------------------------------------


def table_cases():
    yield ["glyphname2unicode"], [[k, v] for k, v in glyphname2unicode.items()]
    yield ["ENCODING"], [list(row) for row in ENCODING]
    ids = {}
    for name, entry in FONT_METRICS.items():
        props, widths = entry
        yield ["FONT_METRICS", name], [[[k, v] for k, v in props.items()], [[k, v] for k, v in widths.items()], ids.setdefault(id(entry), name)]
    yield ["FONT_METRICS"], list(FONT_METRICS)
    # EncodingDB.encodings: which class dict each name gives
    tables = {"std": EncodingDB.std2unicode, "mac": EncodingDB.mac2unicode, "win": EncodingDB.win2unicode, "pdf": EncodingDB.pdf2unicode}
    found = [[name, next((k for k, t in tables.items() if EncodingDB.encodings.get(name) is t), None)] for name in ENCODING_NAMES]
    yield ["encodings", ENCODING_NAMES], found


# -- name2unicode ---------------------------------------------------------------

HEX_EDGES = [0x0000, 0x0041, 0x00A0, 0x0100, 0xD7FF, 0xD800, 0xDBFF, 0xDC00, 0xDFFF, 0xE000, 0xFEFF, 0xFFFF]


def generated_names(rng):
    names = []
    for v in HEX_EDGES:
        names += [f"uni{v:04X}", f"uni{v:04x}", f"u{v:04X}", f"u{v:05X}", f"u{v:06x}", f"uni{v:04X}.sc", f"uni{v:04X}_uni0041"]
    for v in [0x10000, 0x1F600, 0xFFFFF, 0x10FFFF, 0x110000, 0xFFFFFF, 0xD800, 0xDFFF]:
        names += [f"u{v:X}", f"u{v:06X}", f"uni{v:08X}"]
    for k in range(0, 6):
        names.append("uni" + "".join(f"{rng.choice(HEX_EDGES + [rng.randrange(0x10000)]):04X}" for _ in range(k)))
    names += [
        "uni", "u", "", ".", "_", "__", "a.", ".a", "A.sc", "A.alt.1", "A_B", "A_B_C.sc", "f_f_i", "A__B", "A_",
        "uniuni0041", "uni0041i", "uniu0041", "unin0041", "uni0041uni", "u0041u", "uu0041", "u0041uu", "unii0041",
        "uni0041zz00", "uni00zz", "u00zz", "uni+041", "uni-001", "uni0041-001", "u-0041", "u+0041", "u0x41", "uni0x41",
        "uni0X41", "u0X41", "uni0_41", "u0_41", "u00_41", "uni 041", "u 0041", "uni041 ", "u0041 ", "uni٠٠٤١",
        "u٠٠٤١", "uni００４１", "uni00410042", "uni004100", "uni0041004", "u41", "u041",
        "u0041", "u00041", "u000041", "u0000041", "uD800", "uDFFF", "uE000", "ud83d", "uniD83DDE00", "uniDE00", "Uni0041",
        "U0041", "uniG041", "unié0041", "space", "nbspace", "nonbreakingspace", "a1", "g1234", "Euro", "euro",
        "café", "é", "uni0041.", "u0041.sc", "uni004.1", "uni0041_", "_uni0041", "uni0041__u0042", "a_uni00410042_u1F600",
        "uni0041_bad", "bad_uni0041", "uni0041_uniD800", "u110000_A", "A_u110000",
    ]
    alphabet = "uni0123456789abcdefABCDEFxX_.+- ٤１g"
    for _ in range(3000):
        n = rng.randint(0, 12)
        prefix = rng.choice(["", "u", "uni", "uni", "u"])
        names.append(prefix + "".join(rng.choice(alphabet) for _ in range(n)))
    return names


def name2unicode_cases(rng):
    names = list(glyphname2unicode)
    glyphs = list(glyphname2unicode)
    for _ in range(600):
        k = rng.randint(1, 3)
        parts = [rng.choice(glyphs) for _ in range(k)]
        sep = rng.choice(["_", "_", "."])
        names.append(sep.join(parts) + rng.choice(["", ".sc", ".alt", "._x"]))
    names += generated_names(rng)
    seen = set()
    for name in names:
        if name in seen:
            continue
        seen.add(name)
        yield [name], attempt(name2unicode, name)
    for b in [b"A", b"\xff", b"uni0041", b"", b"caf\xe9"]:
        yield [b], attempt(name2unicode, b)


# -- get_encoding -----------------------------------------------------------------

ENCODING_NAMES = ["StandardEncoding", "MacRomanEncoding", "WinAnsiEncoding", "PDFDocEncoding", "Unknown", "", "standardencoding"]


def diff_entry(rng, glyphs):
    r = rng.random()
    if r < 0.25:
        return rng.choice([0, 1, 32, 65, 127, 128, 160, 200, 255, 256, 1000, -1, -100, 2**63, 2**70, -(2**70)])
    if r < 0.65:
        return LIT(rng.choice(glyphs))
    if r < 0.75:
        return LIT(rng.choice(["uni0041", "uniD800", "u110000", "uni00zz", "bad", "", "a_b", "uni0041.sc", "u1F600", "uni004"]))
    if r < 0.78:
        return LIT(rng.choice([b"\xff", b"caf\xe9"]))
    return rng.choice([True, False, 1.5, -0.0, b"A", KWD(b"null"), None, [LIT("A")], {"A": 1}, float("nan")])


def encoding_cases(rng):
    glyphs = list(glyphname2unicode)
    std = [EncodingDB.std2unicode, EncodingDB.mac2unicode, EncodingDB.win2unicode, EncodingDB.pdf2unicode]

    def run(name, diff):
        d = EncodingDB.get_encoding(name, diff)
        return [any(d is s for s in std), [[int(k), v] for k, v in d.items()]]

    for name in ENCODING_NAMES:
        for diff in (None, []):
            yield [name, diff], attempt(run, name, diff)
    for _ in range(800):
        name = rng.choice(ENCODING_NAMES)
        diff = [diff_entry(rng, glyphs) for _ in range(rng.randint(1, 12))]
        yield [name, diff], attempt(run, name, diff)


# -- method scripts -----------------------------------------------------------------

LIT_NAMES = ["A", "space", "nbspace", "uni00A0", "uni0020", "uniD800", "u110000", "uni00zz", "bad", "a_b", "f_f_i", "u1F600", "Euro"]


def pdf_object(rng, depth=0):
    """A random object of the kinds a CMap stream can hold."""
    r = rng.random()
    if r < 0.3:
        return bytes(rng.choice([0, 0x20, 0x41, 0xA0, 0xD8, 0xDC, 0xFE, 0xFF, rng.randrange(256)]) for _ in range(rng.randint(0, 6)))
    if r < 0.45:
        return LIT(rng.choice(LIT_NAMES))
    if r < 0.47:
        return LIT(b"\xff\xfe")
    if r < 0.65:
        return rng.choice([0, 32, 65, 0xA0, 0xD800, 0xDFFF, 0x10FFFF, 0x110000, -1, 2**31, 2**40, -(2**31) - 1, 2**70])
    if r < 0.72:
        return rng.choice([True, False])
    if r < 0.78:
        return rng.choice([1.5, 0.0, -0.0, float("nan"), 1e300])
    if r < 0.84 and depth < 2:
        return [pdf_object(rng, depth + 1) for _ in range(rng.randint(0, 3))]
    if r < 0.88 and depth < 2:
        return {"K": pdf_object(rng, depth + 1)}
    if r < 0.94:
        return KWD(rng.choice([b"null", b"def", b"BT"]))
    return None


def code_string(rng):
    n = rng.choice([0, 1, 1, 2, 2, 3, 4])
    return "".join(chr(rng.choice([0x00, 0x01, 0x41, 0x81, 0xFF, 0x100, 0x3042, 0x10FFFF, rng.randrange(0x300)])) for _ in range(n))


def method_op(rng, cls_name):
    r = rng.random()
    if r < 0.2:
        return ["add_code2cid", code_string(rng), rng.choice([0, 1, 7, 65, 1000, 2**40, -5])]
    if r < 0.45:
        return ["add_cid2unichr", rng.choice([0, 1, 2, 65, 2**40, -1]), pdf_object(rng)]
    if r < 0.55:
        src = rng.random()
        if src < 0.4:
            return ["use_cmap", ["predefined", rng.choice(["H", "78-H", "Identity-H", "OneByteIdentityV", "NoSuch", "to-unicode-Adobe-GB1"])]]
        cls = rng.choice(list(CLASSES))
        return ["use_cmap", ["new", cls, [[code_string(rng), rng.randrange(100)] for _ in range(rng.randint(0, 4))]]]
    if r < 0.65:
        return ["set_attr", rng.choice(["WMode", "CMapName", "X"]), pdf_object(rng)]
    if r < 0.8:
        return ["decode", bytes(rng.choice([0, 1, 0x41, 0x81, 0xFF]) for _ in range(rng.randint(0, 7)))]
    if r < 0.88 and cls_name in UMAP_CLASSES:
        return ["get_unichr", rng.choice([0, 1, 2, 65, 0xD800, 0x10FFFF, 0x110000, -1, 2**31, 2**40])]
    if r < 0.94:
        return ["is_vertical"]
    if cls_name in CMAP_CLASSES or cls_name in UMAP_CLASSES:
        return [rng.choice(["repr", "dump"])]
    return ["is_vertical"]


def build_source(spec):
    if spec[0] == "predefined":
        return CMapDB.get_cmap(spec[1])
    _, cls, codes = spec
    m = CLASSES[cls]()
    for code, cid in codes:
        m.add_code2cid(code, cid)
    return m


def apply_op(m, op):
    name = op[0]
    if name == "add_code2cid":
        return m.add_code2cid(op[1], op[2])
    if name == "add_cid2unichr":
        return m.add_cid2unichr(op[1], op[2])
    if name == "use_cmap":
        return m.use_cmap(build_source(op[1]))
    if name == "set_attr":
        return m.set_attr(op[1], op[2])
    if name == "decode":
        return decode_list(m, op[1])
    if name == "get_unichr":
        return m.get_unichr(op[1])
    if name == "is_vertical":
        return m.is_vertical()
    if name == "repr":
        return repr(m)
    if name == "dump":
        out = io.StringIO()
        m.dump(out)
        return out.getvalue()
    raise ValueError(name)


def run_script(cls_name, kwargs, ops):
    m = CLASSES[cls_name](**kwargs)
    results = [attempt(apply_op, m, op) for op in ops]
    results.append(state(m))
    return results


def method_cases(rng):
    """Scripts of method calls; the output is ``canon_or_hash`` text (it holds
    whole predefined CMaps copied by ``use_cmap``)."""
    for cls_name in CLASSES:
        for _ in range(250):
            kwargs = rng.choice([{}, {}, {"WMode": 1}, {"WMode": 0}, {"CMapName": "X"}, {"CMapName": "Y", "WMode": 1}])
            ops = [method_op(rng, cls_name) for _ in range(rng.randint(1, 12))]
            yield [cls_name, kwargs, ops], (run_script(cls_name, kwargs, ops))
    # the A0 rule and overwrites, in both orders
    for first, second in [(b"\x00 ", b"\x00\xa0"), (b"\x00\xa0", b"\x00 "), (b"\x00 ", b"\x00A"), (b"\x00A", b"\x00\xa0")]:
        ops = [["add_cid2unichr", 5, first], ["add_cid2unichr", 5, second], ["add_cid2unichr", 6, 0xA0], ["get_unichr", 5]]
        yield ["FileUnicodeMap", {}, ops], (run_script("FileUnicodeMap", {}, ops))
    # UTF-16BE with errors="ignore"
    for data in [b"", b"\x00", b"\x00A\x00", b"\xd8\x00", b"\xd8\x00\x00", b"\xd8\x00\xdc\x00", b"\xd8\x00\x00A", b"\xdc\x00\x00A",
                 b"\xd8\x00\xd8\x00\xdc\x00", b"\xdb\xff\xdf\xff", b"\xfe\xff\x00A", b"\x00A\xd8", b"\xd8\x00\xdc", b"\x00\xa0",
                 b"\xd8\x3d\xde\x00\xd8\x3d", b"\xdc\x00\xd8\x00", b"\xd8\x00\xdb\xff\xdc\x00\x00B"]:
        ops = [["add_cid2unichr", 1, data]]
        yield ["FileUnicodeMap", {}, ops], (run_script("FileUnicodeMap", {}, ops))
    # is_vertical for every kind of WMode
    for v in [0, 1, -1, 0.0, -0.0, 1e-300, float("nan"), True, False, None, b"", b"0", LIT("0"), [], {}, 2**70, KWD(b"0")]:
        ops = [["set_attr", "WMode", v], ["is_vertical"]]
        yield ["CMap", {}, ops], (run_script("CMap", {}, ops))
    ops = [["is_vertical"], ["repr"]]
    yield ["CMap", {"WMode": "0"}, ops], (run_script("CMap", {"WMode": "0"}, ops))
    # add_code2cid through an int
    for codes in [["AB", "A"], ["A", "AB"], ["A", "ABC"], ["AB", "ABC"], ["", "A"], ["ABC", "AB", "ABD"]]:
        ops = [["add_code2cid", c, i] for i, c in enumerate(codes)] + [["decode", b"ABCD"]]
        yield ["FileCMap", {}, ops], (run_script("FileCMap", {}, ops))


# -- predefined CMaps -----------------------------------------------------------------


def leaf_paths(d, prefix=()):
    for k, v in d.items():
        if isinstance(v, dict):
            yield from leaf_paths(v, (*prefix, k))
        else:
            yield bytes((*prefix, k)) if all(0 <= x < 256 for x in (*prefix, k)) else None


def decode_samples(rng):
    """Random byte strings for ``decode``, biased to the bytes CMaps use."""
    alphabet = [0x00, 0x20, 0x41, 0x7F, 0x80, 0x81, 0x8E, 0x8F, 0xA1, 0xFE, 0xFF]
    return [bytes(rng.choice(alphabet + [rng.randrange(256)]) for _ in range(rng.randint(1, 8))) for _ in range(100)]


def cmap_report(samples, m):
    """What is compared for a CMap object."""
    out = [type(m).__name__, [[k, v] for k, v in m.attrs.items()], m.is_vertical()]
    if hasattr(m, "code2cid"):
        out.append(tree(m.code2cid))
        paths = [p for p in leaf_paths(m.code2cid) if p is not None]
        out.append(decode_list(m, b"".join(paths)))
    else:
        out += [None, None]
    out.append([decode_list(m, bytes([b])) for b in range(256)])
    out.append(decode_list(m, b"".join(bytes([a, b]) for a in range(256) for b in range(256))))
    out.append([[s, decode_list(m, s)] for s in samples])
    return out


def umap_report(m):
    keys = list(m.cid2unichr)
    probes = keys[::97] + [-1, 0, max(keys, default=0) + 1, 2**40]
    return [type(m).__name__, [[k, v] for k, v in m.attrs.items()], m.is_vertical(),
            [[int(k), v] for k, v in m.cid2unichr.items()], [[c, attempt(m.get_unichr, c)] for c in probes]]


PATH_NAMES = [
    "../cmap/H", "./H", "x/../H", "H.json.gz/../H", "/H", "//H", "", "NoSuch", "H\0", "\0H\0", "\0", "Identity-H\0",
    "../../pdfminer/cmap/H", "../H", "a/b/../../H", "H/", "H/.", "..", ".", "cmap/H", "../cmap/../cmap/V", "./../cmap/./78-H",
    "../../../H", "h", "README.txt", "README", "to-unicode-Adobe-Japan1", "identity-h", "Identity-H ", "UniJIS-UCS2-H",
    "x" * 60 + "/../to-unicode-Adobe-GB1", "\u00e9" * 25 + "/../to-unicode-Adobe-GB1",
    # the AttributeError message of a class read as the wrong kind: "type object '%.100s' has no attribute '%U'"
    # (the class name unquoted, cut to 100 UTF-8 bytes, a cut character decoded as one U+FFFD)
    "it's/../to-unicode-Adobe-GB1", 'a"b/../to-unicode-Adobe-GB1', "a'\"b/../to-unicode-Adobe-GB1",
    "a\\b/../to-unicode-Adobe-GB1", "a\nb\tc\x7f/../to-unicode-Adobe-GB1", "\u00e9\u20ac\U0001F600/../to-unicode-Adobe-GB1",
    "x" * 200 + "/../to-unicode-Adobe-GB1", "x" * 76 + "/../to-unicode-Adobe-GB1", "x" * 77 + "/../to-unicode-Adobe-GB1",
    "x" * 99 + "\u00e9/../to-unicode-Adobe-GB1", "x" * 98 + "\u00e9/../to-unicode-Adobe-GB1",
    "x" * 98 + "\u20ac/../to-unicode-Adobe-GB1", "x" * 99 + "\u20ac/../to-unicode-Adobe-GB1",
    "x" * 97 + "\U0001F600/../to-unicode-Adobe-GB1", "x" * 96 + "\U0001F600/../to-unicode-Adobe-GB1",
    "x" * 99 + "\U0001F600/../to-unicode-Adobe-GB1", "\u20ac" * 40 + "/../to-unicode-Adobe-GB1",
]
UMAP_NAMES = ["Adobe-Japan1", "Adobe-GB1", "Adobe-CNS1", "Adobe-Korea1", "Adobe-Identity", "Adobe-UCS", "Unknown",
              "x/../../cmap/H", "x/../Adobe-Korea1", "../cmap/to-unicode-Adobe-GB1", "Adobe-Japan1\0", "",
              # AttributeError messages show the class name "to-unicode-<name>" unquoted, cut to 100 UTF-8 bytes
              # ("%.100s"), a cut character decoded as one U+FFFD
              "x" * 60 + "/../../cmap/V", "x" * 34 + "\u00e9/../../cmap/V", "x" * 35 + "\u00e9/../../cmap/V",
              "x" * 34 + "\u20ac/../../cmap/V", "\u00e9" * 30 + "/../../cmap/V",
              "it's/../../cmap/V", 'a"b/../../cmap/V', "a\\b/../../cmap/V", "a\nb\r/../../cmap/V",
              "x" * 200 + "/../../cmap/V", "x" * 76 + "/../../cmap/V", "x" * 77 + "/../../cmap/V",
              "x" * 88 + "\u00e9/../../cmap/V", "x" * 87 + "\u00e9/../../cmap/V", "x" * 87 + "\u20ac/../../cmap/V",
              "x" * 88 + "\u20ac/../../cmap/V", "x" * 86 + "\U0001F600/../../cmap/V", "x" * 88 + "\U0001F600/../../cmap/V",
              "\u20ac" * 40 + "/../../cmap/V"]


def predefined_cases(rng):
    names = [n for n in codegen_cmaps.names() if not n.startswith("to-unicode-")]
    for name in names + ["Identity-H", "Identity-V", "OneByteIdentityH", "OneByteIdentityV"] + PATH_NAMES:
        samples = decode_samples(rng)
        m = attempt(CMapDB.get_cmap, name)
        out = m if isinstance(m, Exception) else [cmap_report(samples, m), CMapDB.get_cmap(name) is m]
        yield ["get_cmap", name, samples], out
    for name in [n[len("to-unicode-"):] for n in codegen_cmaps.names() if n.startswith("to-unicode-")] + UMAP_NAMES:
        for vertical in (False, True):
            m = attempt(CMapDB.get_unicode_map, name, vertical)
            out = m if isinstance(m, Exception) else [umap_report(m), CMapDB.get_unicode_map(name, vertical) is m]
            yield ["get_unicode_map", name, vertical], out


# -- CMapParser jobs ----------------------------------------------------------------------

UNICODE_SAMPLES = [0, 1, 32, 65, 0xA0, 0xD800, 0x10FFFF, 0x110000, -1, 2**40]


def parse_job(target, data):
    """``CMapParser(target, BytesIO(data)).run()``: the outcome, the state,
    the warnings, and samples of ``decode``/``get_unichr``; with the
    recursion depth it was called at (``_common.depth_here``)."""
    depth = depth_here()
    m = CLASSES[target]()
    parser = CMapParser(m, io.BytesIO(data))
    # called inline: attempt(parser.run) would enter the evaluation loop from
    # C (CALL_FUNCTION_EX), two C recursion units more
    try:
        parser.run()
        error = None
    except Exception as e:  # the exception is the expected result
        error = e
    out = [error, state(m), sorted(parser._warnings)]
    samples = [b"", b"\x00", b"A", b"\x00A", b"\x81\x40\x82", b"\xff\xff", b"\x00\x01\x02\x03\x04"]
    out.append([attempt(decode_list, m, s) for s in samples])
    if isinstance(m, UnicodeMap):
        keys = [int(k) for k in m.cid2unichr]  # a bool key is 0/1 in the port
        probes = keys[:3] + keys[-1:] + UNICODE_SAMPLES
        out.append([[c, attempt(m.get_unichr, c)] for c in probes])
    else:
        out.append(None)
    return depth, out


def h(rng, n=None, values=None):
    n = rng.randint(0, 6) if n is None else n
    if values is not None:
        return b"<" + values.hex().encode() + b">"
    return b"<" + bytes(rng.choice([0x00, 0x01, 0x20, 0x41, 0x7F, 0xA0, 0xD8, 0xDC, 0xFE, 0xFF, rng.randrange(256)]) for _ in range(n)).hex().upper().encode() + b">"


def obj_token(rng, errors, depth=0):
    """A token for a value position (bfrange array element, bfchar
    destination...). ``errors``: may produce what makes pdfminer raise."""
    r = rng.random()
    if r < 0.35:
        return h(rng)
    if r < 0.5:
        names = ["A", "space", "nbspace", "uni00A0", "uni0041", "a_b", "f_f_i", "u1F600", "Euro", "g123x", "uni0041.sc"]
        if errors:
            names += ["bad", "uniD800", "u110000", "uni00zz", "#ff#fe"]
        return b"/" + rng.choice(names).encode()
    if r < 0.62:
        values = [0, 32, 65, 0xA0, 0x3042, 0x10FFFF]
        if errors:
            values += [0x110000, -1, 2**40]
        return str(rng.choice(values)).encode()
    if r < 0.67:
        return rng.choice([b"true", b"false"])
    if r < 0.72:
        return rng.choice([b"(A)", b"(\\000A)", b"()", b"(\\376\\377)"])
    if r < 0.76 and depth < 2:
        return b"[" + b" ".join(obj_token(rng, errors, depth + 1) for _ in range(rng.randint(0, 3))) + b"]"
    if errors and r < 0.8:
        return rng.choice([b"1.5", b"null", b"<< /K 1 >>", b"{ 1 }", b"-0.0", b"[1 [2]]"])
    return h(rng, 2)


def range_ends(rng, n):
    """``(start, end)`` hex tokens of ``n`` bytes, mostly a short range."""
    start = rng.choice([0, 1, 0x20, 0xFE, 0xFF, 0x100, 0xFFFE, 0x1FF, rng.randrange(1 << (8 * n)) if n else 0])
    start &= (1 << (8 * n)) - 1 if n else 0
    span = rng.choice([0, 1, 2, 5, 30, 300, -1])
    end = max(0, min((1 << (8 * n)) - 1 if n else 0, start + span))
    sb = start.to_bytes(n, "big") if n else b""
    eb = end.to_bytes(n, "big") if n else b""
    return h(rng, values=sb), h(rng, values=eb)


def non_bytes(rng):
    """A token that is not a string (a range end that pdfminer rejects)."""
    return rng.choice([b"/A", b"5", b"true", b"[<00>]", b"1.5", b"null", b"<< >>"])


def section_bfrange(rng, errors):
    lines = []
    for _ in range(rng.randint(1, 6)):
        n = rng.choice([1, 2, 2, 2, 3, 4, 5, 0])
        s, e = range_ends(rng, n)
        if rng.random() < 0.1:
            e = h(rng, n + rng.choice([-1, 1]) if n else 1)
        if rng.random() < 0.05:
            s = non_bytes(rng)
        r = rng.random()
        if r < 0.55:
            k = rng.choice([1, 2, 2, 3, 4, 6])
            base = rng.choice([0, 0x41, 0xFE, 0xFF, 0xFFFE, 0xFFFF, 0xFFFFFE, 0xD7FF, 0xDBFF, rng.randrange(1 << (8 * min(k, 4)))])
            if errors and k >= 4 and rng.random() < 0.3:
                base = 0xFFFFFFFF - rng.randrange(3)
            base &= (1 << (8 * min(k, 4))) - 1
            dest = bytes(rng.randrange(256) for _ in range(max(0, k - 4))) + base.to_bytes(min(k, 4), "big")
            d = h(rng, values=dest)
        elif r < 0.9:
            d = b"[" + b" ".join(obj_token(rng, errors) for _ in range(rng.randint(0, 6))) + b"]"
        elif errors:
            d = obj_token(rng, True)
        else:
            d = h(rng, 2)
        lines.append(s + b" " + e + b" " + d)
    return b"%d beginbfrange\n" % len(lines) + b"\n".join(lines) + b"\nendbfrange\n"


def section_bfchar(rng, errors):
    lines = []
    for _ in range(rng.randint(1, 8)):
        src = h(rng, rng.choice([1, 2, 2, 3, 0, 9])) if rng.random() < 0.9 else non_bytes(rng)
        dst = h(rng, rng.choice([2, 2, 4, 1, 0, 3])) if rng.random() < 0.7 else obj_token(rng, errors)
        lines.append(src + b" " + dst)
    return b"%d beginbfchar\n" % len(lines) + b"\n".join(lines) + b"\nendbfchar\n"


def section_cidrange(rng, errors):
    lines = []
    for _ in range(rng.randint(1, 5)):
        n = rng.choice([1, 2, 2, 3, 4, 5, 6, 0])
        s, e = range_ends(rng, n)
        if rng.random() < 0.15:
            prefix_s = bytes(rng.randrange(2) for _ in range(max(0, n - 4)))
            s = h(rng, values=prefix_s + bytes.fromhex(s[1:-1].decode())[len(prefix_s):]) if n > 4 else s
        if rng.random() < 0.1:
            e = h(rng, n + 1)
        if rng.random() < 0.05:
            s = non_bytes(rng)
        if rng.random() < 0.05:
            e = non_bytes(rng)
        cid = rng.choice([b"0", b"1", b"100", b"65535", b"true", b"-5", b"99999999999999999999", b"1.5", b"/A", b"<01>"])
        lines.append(s + b" " + e + b" " + cid)
    return b"%d begincidrange\n" % len(lines) + b"\n".join(lines) + b"\nendcidrange\n"


def section_cidchar(rng, errors):
    lines = []
    for _ in range(rng.randint(1, 6)):
        cid = rng.choice([b"0", b"7", b"65", b"true", b"false", b"1.5", b"<41>", b"-3", b"2147483648"])
        code = h(rng, rng.choice([1, 2, 4, 0])) if rng.random() < 0.8 else obj_token(rng, False)
        lines.append(cid + b" " + code)
    return b"%d begincidchar\n" % len(lines) + b"\n".join(lines) + b"\nendcidchar\n"


def section_misc(rng, errors):
    choices = [
        b"1 begincodespacerange\n<00> <FF>\nendcodespacerange\n",
        b"2 begincodespacerange <0000> <80FF> <8140> <FFFF> endcodespacerange\n",
        b"1 beginnotdefrange <00> <1F> 1 endnotdefrange\n",
        b"/CMapName /Test def\n", b"/WMode 1 def\n", b"/WMode 0 def\n", b"/WMode true def\n", b"/WMode /V def\n",
        b"/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n", b"/CMapType 2 def\n",
        b"(key) 5 def\n", b"5 /v def\n", b"[/a] 1 def\n", b"<< /K 1 >> 2 def\n", b"/x#ff 3 def\n",
        b"/H usecmap\n", b"/78-H usecmap\n", b"/NoSuch usecmap\n", b"/Identity-H usecmap\n", b"/..#2Fcmap#2FV usecmap\n",
        b"(H) usecmap\n", b"/OneByteIdentityH usecmap\n", b"/UniJIS-UCS2-H usecmap\n",
        b"endcmap\n", b"begincmap\n", b"CMapName currentdict /CMap defineresource pop\n", b"end end\n", b"12 dict begin\n",
        b"]", b">>", b"}", b"{ 1 2 }", b"[ 1 2 ]", b"%comment\n", b"1 2 3", b"/A /B /C", b"true", b"null", b"<< /A 1 >>",
    ]
    if errors:
        choices += [b"def\n", b"/WMode def\n", b"usecmap\n", b"<< /A >>\n", b"[1 2] 3 4 begincidrange [5] endcidrange\n"]
    return rng.choice(choices)


HEADER = (b"/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n"
          b"/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n"
          b"/CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n")
FOOTER = b"endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n"
SECTIONS = [section_bfrange, section_bfrange, section_bfchar, section_bfchar, section_cidrange, section_cidchar, section_misc, section_misc]


def fuzz_job(rng):
    errors = rng.random() < 0.15
    parts = [HEADER] if rng.random() < 0.8 else []
    for _ in range(rng.randint(1, 8)):
        parts.append(rng.choice(SECTIONS)(rng, errors))
    if rng.random() < 0.8:
        parts.append(FOOTER)
    if rng.random() < 0.1:
        parts.append(rng.choice(SECTIONS)(rng, errors))
    data = b"".join(parts)
    if rng.random() < 0.05:
        # tokens across the 4096-byte read blocks
        data = b"%" + b"x" * rng.randint(3900, 4094) + b"\n" + data
    r = rng.random()
    target = "FileUnicodeMap" if r < 0.6 else "FileCMap" if r < 0.75 else rng.choice(list(CLASSES))
    return target, data


FIXED = [
    b"<00> <02> [/A /bad /B] endbfrange", b"1 beginbfrange <00> <01> [/uniD800] endbfrange",
    b"1 beginbfrange <00> <01> [/u110000] endbfrange", b"1 beginbfrange <00> <01> [/uni00zz] endbfrange",
    b"1 beginbfrange <00> <01> [/#ff] endbfrange", b"1 beginbfrange <00> <01> [1.5] endbfrange",
    b"1 beginbfrange <00> <01> [[1]] endbfrange", b"1 beginbfrange <00> <01> [null] endbfrange",
    b"1 beginbfrange <00> <01> [<< /A 1 >>] endbfrange", b"1 beginbfrange <00> <01> [1114112] endbfrange",
    b"1 beginbfrange <00> <01> [-1] endbfrange", b"1 beginbfrange <00> <01> [1099511627776] endbfrange",
    b"1 beginbfrange <00> <01> /A endbfrange", b"1 beginbfrange <00> <01> 5 endbfrange",
    b"1 beginbfrange <00> <01> (AB) endbfrange", b"1 beginbfrange <00> <05> <FFFFFFFE> endbfrange",
    b"1 beginbfrange <00> <05> <0000FFFFFFFE> endbfrange", b"1 beginbfrange <00> <03> <FFFFFFFF> endbfrange",
    b"1 beginbfrange <00> <05> <FE> endbfrange", b"1 beginbfrange <00> <05> <00FE> endbfrange", b"1 beginbfrange <00> <05> <> endbfrange",
    b"1 beginbfrange <> <> <0041> endbfrange", b"1 beginbfrange <0000000000000041> <0000000000000043> <0041> endbfrange",
    b"1 beginbfrange <FFFFFFFFFFFFFFFFFF> <FFFFFFFFFFFFFFFFFF> [/A] endbfrange",
    b"1 beginbfrange <05> <00> <0041> endbfrange", b"1 beginbfrange <05> <00> [/A] endbfrange",
    b"1 beginbfchar <000000000000000001> <0041> endbfchar", b"1 beginbfchar <> <0041> endbfchar",
    b"1 begincidrange <> <> 5 endcidrange", b"1 begincidrange <0000000001> <0000000003> 5 endcidrange",
    b"1 begincidrange <0100000001> <0000000003> 5 endcidrange", b"1 begincidrange <00> <02> 99999999999999999999 endcidrange",
    b"1 begincidrange <00> <02> true endcidrange", b"1 begincidchar true <0041> endcidchar",
    b"def", b"/WMode def", b"usecmap", b"/Identity-H usecmap", b"/Identity-V usecmap", b"/H usecmap /V usecmap",
    b"<< /A >>", b"endcmap /WMode 1 def begincmap /WMode 1 def",
    b"1 beginbfchar <0001> <0020> endbfchar 1 beginbfchar <0001> <00A0> endbfchar",
    b"1 beginbfchar <0001> <00A0> endbfchar 1 beginbfchar <0001> <0020> endbfchar",
    b"[1 2] [3 4] /K def /x [5] def", b"1 2 3 begincodespacerange 4 endcodespacerange 5",
    # usecmap of a unicode map file: AttributeError, not CMapNotFound
    b"/to-unicode-Adobe-GB1 usecmap", b"/..#2Fcmap#2Fto-unicode-Adobe-CNS1 usecmap",
    # ... whose message shows the class name unquoted, cut to 100 UTF-8 bytes ("%.100s")
    b"/it#27s#2F..#2Fto-unicode-Adobe-GB1 usecmap", b"/a#5Cb#2F..#2Fto-unicode-Adobe-GB1 usecmap",
    b"/a#0Ab#22#2F..#2Fto-unicode-Adobe-GB1 usecmap", b"/" + b"x" * 200 + b"#2F..#2Fto-unicode-Adobe-GB1 usecmap",
    b"/" + b"x" * 99 + b"#C3#A9#2F..#2Fto-unicode-Adobe-GB1 usecmap",
    b"/" + b"x" * 98 + b"#E2#82#AC#2F..#2Fto-unicode-Adobe-GB1 usecmap",
    b"/" + b"x" * 97 + b"#F0#9F#98#80#2F..#2Fto-unicode-Adobe-GB1 usecmap",
    # each "begin..."/"end..." empties the stack: what def pops next shows it
    b"1 beginnotdefrange <00> <1F> 1 endnotdefrange /CMapName def",
    b"1 beginnotdefrange <00> <1F> 1 /CMapName def", b"<00> <1F> 1 endnotdefrange /CMapName def",
    b"1 begincodespacerange <00> /CMapName def", b"<00> <FF> endcodespacerange /CMapName def",
    b"1 begincidrange <00> /CMapName def", b"1 begincidchar <00> /CMapName def", b"1 beginbfrange <00> /CMapName def",
    b"1 beginbfchar <00> /CMapName def", b"1 begincmap /CMapName def",
]


def fixed_jobs():
    for data in FIXED:
        for target in ("FileUnicodeMap", "FileCMap", "CMap", "UnicodeMap", "IdentityUnicodeMap", "CMapBase", "IdentityCMap"):
            yield target, data
    # the def key nested around the C recursion limit of str() (literal_name)
    c_left = depth_here()["c_left"]
    for n in range(c_left - 8, c_left + 8):
        yield "FileUnicodeMap", b"[" * n + b"]" * n + b" 1 def"
    # a long map across many read blocks
    lines = b"\n".join(b"<%04X> <%04X>" % (i, 0x4E00 + i) for i in range(3000))
    yield "FileUnicodeMap", HEADER + b"3000 beginbfchar\n" + lines + b"\nendbfchar\n" + FOOTER


def parse_cases(n_fuzz, seed):
    rng = random.Random(seed)
    jobs = [fuzz_job(rng) for _ in range(n_fuzz)] + list(fixed_jobs())
    for target, data in jobs:
        depth, out = parse_job(target, data)
        yield [target, data, depth], out


# -- real corpus ----------------------------------------------------------------------------


class Timeout(Exception):
    pass


def on_alarm(signum, frame):
    raise Timeout()


def collect_streams(path, password):
    """The decoded data of every embedded CMap stream of a document."""
    found = {}
    with open_document(path, password) as doc:
        for xref in doc.xrefs:
            for objid in xref.get_objids():
                try:
                    obj = doc.getobj(objid)
                except Exception:
                    continue
                candidates = []
                if isinstance(obj, PDFStream) and getattr(obj.get("Type"), "name", None) == "CMap":
                    candidates.append(obj)
                if isinstance(obj, dict):
                    for key in ("ToUnicode", "Encoding"):
                        try:
                            v = resolve1(obj.get(key))
                        except Exception:
                            continue
                        if isinstance(v, PDFStream):
                            candidates.append(v)
                for strm in candidates:
                    try:
                        data = strm.get_data()
                    except Exception:
                        continue
                    found.setdefault(hashlib.sha256(data).hexdigest(), data)
    return list(found.values())


def process_file(args):
    path, rel, password = args
    signal.signal(signal.SIGALRM, on_alarm)
    signal.alarm(FILE_TIMEOUT_S)
    try:
        return rel, None, collect_streams(path, password)
    except Timeout:
        return rel, "timeout", []
    except Exception as e:  # unreadable document: not a CMap case
        return rel, type(e).__name__, []
    finally:
        signal.alarm(0)


def corpus_cases(jobs):
    files = load_set("all")
    tasks = [(str(f.path), f.rel, f.password) for f in files]
    streams, skipped = {}, collections.Counter()
    with multiprocessing.Pool(jobs) as pool:
        for rel, status, datas in pool.imap(process_file, tasks, chunksize=4):
            if status is not None:
                skipped[status] += 1
            for data in datas:
                streams.setdefault(hashlib.sha256(data).hexdigest(), (rel, data))
    print(f"{len(streams)} distinct CMap streams from {len(files)} files ({dict(skipped)} skipped)", file=sys.stderr)
    cases = []
    for sha in sorted(streams):
        rel, data = streams[sha]
        for target in ("FileUnicodeMap", "FileCMap"):
            depth, out = parse_job(target, data)
            cases.append(([target, data, depth], out))
    return cases


# -- main -----------------------------------------------------------------------------------


def write_marker(name: str, count: int) -> None:
    """``<name>.json``: the sources that wrote ``<name>.jsonl`` (checked by
    ``python -m oracle.unit`` and the Rust replay)."""
    with open(unit_dir(MODULE) / f"{name}.json", "w", encoding="ascii") as f:
        json.dump({"sources": oracle_sources(), "cases": count}, f)


HASHED = {
    "methods": lambda args: method_cases(random.Random(args.seed)),
    "predefined": lambda args: predefined_cases(random.Random(args.seed)),
    "parse": lambda args: parse_cases(args.fuzz, args.seed),
}


def hashed(cases):
    """The cases with their output as ``canon_or_hash`` text."""
    for args, out in cases:
        yield args, canon_or_hash(out)


def dump(args) -> None:
    """Prints the full canonical output of case INDEX of a hashed set."""
    name, index = args.dump[0], int(args.dump[1])
    if name not in HASHED:
        raise SystemExit(f"--dump {name}: one of {', '.join(HASHED)}")
    for k, (_, out) in enumerate(HASHED[name](args)):
        if k == index:
            print(canon(out))
            return
    raise SystemExit(f"--dump {name}: no case {index}")


def main() -> None:
    ap = argparse.ArgumentParser(prog="python -m oracle.unit.cmaps")
    ap.add_argument("--fuzz", type=int, default=3000, help="number of generated CMap streams")
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--corpus", action="store_true", help="the embedded CMaps of the corpus set all")
    ap.add_argument("-j", "--jobs", type=int, default=3, help="worker processes for --corpus")
    ap.add_argument("--dump", nargs=2, metavar=("NAME", "INDEX"))
    args = ap.parse_args()
    require_oracle_python()
    cmap_dir = codegen_cmaps.ensure()
    rel_cmap_dir = cmap_dir.relative_to(cache_dir()).as_posix()

    if args.dump:
        dump(args)
        return

    if args.corpus:
        n = write_cases(MODULE, "corpus", hashed(corpus_cases(args.jobs)))
        write_marker("corpus", n)
        write_meta(MODULE, cmap_dir=rel_cmap_dir)
        print(f"corpus: {n} cases")
        return

    counts = {}
    rng = random.Random(args.seed)
    counts["tables"] = write_cases(MODULE, "tables", table_cases())
    counts["name2unicode"] = write_cases(MODULE, "name2unicode", name2unicode_cases(rng))
    counts["encoding"] = write_cases(MODULE, "encoding", encoding_cases(rng))
    for name, cases in HASHED.items():
        started = time.monotonic()
        counts[name] = write_cases(MODULE, name, hashed(cases(args)))
        print(f"{name}: {counts[name]} cases, {time.monotonic() - started:.0f} s", file=sys.stderr)
    write_marker("synthetic", sum(counts.values()))
    write_meta(MODULE, cmap_dir=rel_cmap_dir)
    print(counts)


if __name__ == "__main__":
    main()
