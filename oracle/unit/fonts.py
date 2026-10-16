"""Differential cases for crates/pdfminer/src/pdffont/ (``pdfminer/pdffont.py``),
computed by pdfminer.

Writes typed JSON lines (docs/verification.md) to
``<cache>/unit/fonts/<fingerprint>/<name>.jsonl`` (``oracle/unit/_common.py``);
the Rust replay is crates/pdftrace/tests/fonts_unit.rs. The predefined CMaps
are read by the Rust side from the compact files of ``oracle/codegen/cmaps.py``
(``cmap_dir`` in ``meta.json``, relative to the cache).

Default run (about ten minutes: through ``scripts/heavy.sh``):

- ``widths``: ``get_widths`` and ``get_widths2`` on generated arrays (numbers of
  every type, runs, lists, references to numbers, lists and missing or broken
  objects, other objects).
- ``getdict``: ``getdict`` on generated CFF DICT data (every operand encoding,
  real numbers with every nibble, truncations).
- ``cff``: ``CFFFont(name, BytesIO(data))`` on generated CFF programs (INDEXes
  of every offset size, encodings and charsets of every format, strings of the
  string INDEX read in the middle of a charset) and their mutations
  (truncations, byte changes): the attributes, the items of every INDEX.
- ``truetype``: ``TrueTypeFont(name, BytesIO(data))`` then
  ``create_unicode_map()`` on generated TrueType programs (``cmap`` subtables
  of formats 0, 2, 4, 6, 10, 12 and others, non-Unicode platforms) and their
  mutations: the tables, the map or the exception.
- ``type1``: ``Type1FontHeaderParser(BytesIO(data)).get_encoding()`` on
  generated font program headers.
- ``fonts``: font dictionaries (``PDFResourceManager().get_font(None, spec)``,
  every class, ``Type0`` chains, missing or malformed entries of every kind,
  embedded programs, ``/ToUnicode`` streams, every predefined CMap name and
  unknown ones), with the font's attributes and, for sample strings and codes,
  ``decode``, ``to_unichr``, ``char_width``, ``char_disp``, ``string_width``.
- ``depth``: for fonts of every kind, the largest number of extra Python frames
  under which building the font (and each query) still succeeds, with the
  outcome there and one frame deeper (a ``RecursionError``); the font is built
  once first, which loads the predefined CMaps it uses (``get_font``), and,
  for a font that loads some, the same search with ``CMapDB``'s caches
  emptied before each build (``get_font_cold``: the first loads, as in a new
  process). Fonts whose ``/ToUnicode`` uses ``usecmap`` (found, missing,
  identity) are among them.

The first ``bytes.decode("UTF-16BE")`` of a process imports the codec, deep
below the caller (not modeled by the port): the module imports it first.

``--corpus`` (minutes, every PDF of the set ``all``, the first 20 pages, ``-j``
workers): every font a page or a Form XObject uses (the ``spec`` given to
``PDFResourceManager.get_font``), with every indirect object it reaches (and
the error of those that do not load), and every string shown with it; the
embedded font programs (``FontFile``, ``FontFile2``, ``FontFile3``) through
``Type1FontHeaderParser``, ``TrueTypeFont`` and ``CFFFont``. Writes
``corpus_fonts.jsonl``, ``corpus_programs.jsonl`` and ``corpus.json``.

A font case is ``[spec, table, errors, strings, probes, depth]``: the font
dictionary (typed JSON, its references by number), the objects of the
document it reaches (``[[objid, object], ...]``, streams with their raw
data), the object numbers whose loading raised (``[[objid, exception],
...]``), the strings to decode, the extra codes to query, and the recursion
depth of the call. The case is run on objects decoded from that dump (so the
Rust side sees exactly the same input); for the real corpus, the font built
from the dump is checked against the one pdfminer built in the document
(``corpus.json`` counts the differences, which only come from what the dump
cannot carry). Streams the font does not read are emptied (``b""``) when that
does not change the outcome.

Outputs are compared through their canonical text (``oracle.canon``): the
text itself when it is short, else ``"sha256:<hex>:<length>"``.
``--dump NAME INDEX`` prints the full canonical text of one case.

Usage: PDFPLUMBER_ROOT=<main checkout> <root>/.venv/bin/python -m oracle.unit.fonts [--corpus] [-j N]
"""

from __future__ import annotations

import argparse
import codecs
import collections
import contextlib
import hashlib
import io
import json
import logging
import math
import multiprocessing
import random
import signal
import struct
import sys
import time
from typing import Any

from pdfminer import pdfexceptions, pdffont, psexceptions
from pdfminer.cmapdb import CMap, CMapDB, IdentityCMap, PyCMap, UnicodeMap
from pdfminer.pdfdevice import PDFDevice
from pdfminer.pdfdocument import PDFStandardSecurityHandlerV5
from pdfminer.pdffont import (
    CFFFont,
    PDFCIDFont,
    PDFFont,
    TrueTypeFont,
    Type1FontHeaderParser,
    get_widths,
    get_widths2,
    getdict,
)
from pdfminer.pdfinterp import PDFPageInterpreter, PDFResourceManager
from pdfminer.pdfparser import PDFSyntaxError
from pdfminer.pdfpage import PDFPage
from pdfminer.pdftypes import PDFObjRef, PDFStream
from pdfminer.psparser import KWD, LIT, PSKeyword, PSLiteral, PSStackParser

from ..canon import Named, canon, exception_message, exception_name
from ..codegen import cmaps as codegen_cmaps
from ..corpus import cache_dir, load_set
from ..stages._common import open_document
from ..typed_json import decode as typed_decode
from ..typed_json import encode as typed_encode
from ._common import at_depth, depth_here, oracle_sources, require_oracle_python, unit_dir, write_cases, write_meta
from .crypto import aes_cbc, deep_handlers, factory_of

MODULE = "fonts"
FILE_TIMEOUT_S = 180
#: Outputs longer than this are compared through their SHA-256.
INLINE_LIMIT = 4000
#: The codes every font is also queried with.
PROBES = [0, 1, 32, 65, 97, 160, 255, 256, 0x4E00, 0xD800, 0xFFFF, 0x10000, 0x10FFFF, 0x110000]
#: The strings every generated font decodes.
SAMPLE_STRINGS = [b"", b"A", b"Hello", b"\x00A\x00B", b"\x00", b"\xff\xfe\x01", b"\x81\x40\x82\xa0", bytes(range(32, 48))]
#: The recursion limit while dumping or decoding deep values (never while
#: pdfminer runs).
DEEP_LIMIT = 100_000

# The first ``bytes.decode("UTF-16BE")`` of a process imports the codec
# (``importlib``, 15 frames below ``CMapParser.run``), which the port does
# not model (``crates/pdfminer/src/cmapdb.rs``, "Recursion depth"): imported
# here, so that no set depends on the order the sets run in.
codecs.lookup("UTF-16BE")


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


@contextlib.contextmanager
def deep_recursion():
    old = sys.getrecursionlimit()
    sys.setrecursionlimit(DEEP_LIMIT)
    try:
        yield
    finally:
        sys.setrecursionlimit(old)


def clear_logger_caches() -> None:
    """``logging.disable`` clears the cache of every logger: the first call of
    each level takes one more frame (``_acquireLock``); the Rust replay clears
    its model of the cache at the same points (``frames::clear_logger_cache``)."""
    logging.disable(logging.CRITICAL)


# -- documents ----------------------------------------------------------------


#: Object numbers from this one up are looked up ``DEEP_GETOBJ_CALLS``
#: calls further down (``FontDoc._deep``).
DEEP_OBJIDS = 200_000
DEEP_GETOBJ_CALLS = 8


class FontDoc:
    """A document whose ``getobj`` reads a table of objects, or raises the
    exception recorded for an object number (``PDFObjectNotFound`` for one
    missing from both); from ``DEEP_OBJIDS`` up, through more frames.

    An encrypted document also has ``cipher``, ``[docid, param, password]``
    (what ``PDFDocument._initialize_password`` builds its security handler
    from), and ``decipher``, that handler's ``decrypt``: every stream of the
    case with an object number gets it, as ``PDFParser`` gives it the
    document's (``load_doc``, ``bind``)."""

    def __init__(self) -> None:
        self.table: dict[Any, Any] = {}
        #: objid -> (exception class, args)
        self.errors: dict[Any, tuple[type, tuple]] = {}
        self.cipher: list | None = None
        self.decipher = None

    def getobj(self, objid):
        if type(objid) is int and objid >= DEEP_OBJIDS:
            return self._deep(DEEP_GETOBJ_CALLS, objid)
        if objid in self.table:
            return self.table[objid]
        if objid in self.errors:
            cls, args = self.errors[objid]
            raise cls(*args)
        raise pdfexceptions.PDFObjectNotFound(objid)

    def _deep(self, n: int, objid):
        """The lookup of an object number from ``DEEP_OBJIDS`` up, ``n``
        frames further down (as a real document's ``getobj`` goes down
        through its parser: deeper than what calls it)."""
        if n:
            return self._deep(n - 1, objid)
        if objid in self.table:
            return self.table[objid]
        raise pdfexceptions.PDFObjectNotFound(objid)


#: Exception classes by the name ``canon`` gives them (for the recorded
#: ``getobj`` errors of the real corpus).
def _exception_classes() -> dict[str, type]:
    import builtins
    import zlib

    from pdfminer import ccitt, cmapdb, lzw, pdfdocument, pdfinterp, pdfparser

    found: dict[str, type] = {}
    for module in (builtins, psexceptions, pdfexceptions, pdfparser, pdfdocument, pdffont, pdfinterp, cmapdb, lzw, ccitt):
        for value in vars(module).values():
            if isinstance(value, type) and issubclass(value, BaseException):
                found.setdefault(value.__name__, value)
    found["zlib.error"] = zlib.error
    found["struct.error"] = struct.error
    return found


EXCEPTION_CLASSES = _exception_classes()


def bind(value, doc):
    """A copy of a decoded value whose references resolve through ``doc``
    (``typed_json.decode`` gives them no document)."""
    if isinstance(value, PDFObjRef):
        return PDFObjRef(doc, value.objid)
    if isinstance(value, list):
        return [bind(x, doc) for x in value]
    if isinstance(value, dict):
        return {k: bind(x, doc) for k, x in value.items()}
    if isinstance(value, PDFStream):
        decipher = doc.decipher if value.objid is not None else None
        s = PDFStream(bind(value.attrs, doc), value.rawdata, decipher)
        s.data = value.data
        s.objid = value.objid
        s.genno = value.genno
        return s
    return value


def dump_doc(doc: FontDoc) -> tuple[list, list]:
    """``table`` (``[[objid, object], ...]``) and ``errors`` (``[[objid,
    class name, args, str(exception)], ...]``) of a case."""
    table = [[objid, obj] for objid, obj in doc.table.items()]
    errors = []
    for objid, (cls, args) in doc.errors.items():
        e = cls(*args)
        errors.append([objid, exception_name(e), tuple(args), exception_message(e)])
    return table, errors


def security_handler(cipher):
    """The security handler of ``[docid, param, password]``, built as
    ``PDFDocument._initialize_password`` builds it."""
    docid, param, password = cipher
    return factory_of(param)(docid, param, password)


def load_doc(table, errors, cipher=None) -> FontDoc:
    """The document of decoded ``table``, ``errors`` and ``cipher``."""
    doc = FontDoc()
    if cipher is not None:
        doc.cipher = cipher
        doc.decipher = security_handler(cipher).decrypt
    for objid, obj in table:
        doc.table[objid] = bind(obj, doc)
    for objid, name, args, _text in errors:
        doc.errors[objid] = (EXCEPTION_CLASSES[name], tuple(args))
    return doc


def roundtrip(*values):
    """The values as the Rust side decodes them: typed JSON and back."""
    with deep_recursion():
        return typed_decode(json.loads(json.dumps(typed_encode(list(values)))))


# -- reports --------------------------------------------------------------------


def seqval(x):
    """A value that may be a repeated ``bytes``/``list`` (Type 3 scales):
    the port keeps its type and length only."""
    if isinstance(x, (bytes, list)):
        return Named("seq", {"type": type(x).__name__, "len": len(x)})
    return x


def call_seq(f, *args):
    r = attempt(f, *args)
    return r if isinstance(r, Exception) else seqval(r)


def umap_report(m):
    if m is None:
        return None
    return Named(type(m).__name__, {"attrs": dict(m.attrs), "items": [(int(k), v) for k, v in m.cid2unichr.items()]})


def cmap_report(m):
    fields = {"attrs": dict(m.attrs)}
    if hasattr(m, "code2cid"):
        fields["code2cid"] = len(m.code2cid)
    return Named(type(m).__name__, fields)


def font_attrs(font: PDFFont) -> Named:
    """The attributes of a font, in a fixed order (absent ones omitted)."""
    v = vars(font)
    fields: dict[str, Any] = {}
    for name in ("basefont", "fontname", "flags", "ascent", "descent", "italic_angle", "default_width", "leading", "bbox"):
        if name in v:
            fields[name] = v[name]
    fields["hscale"] = seqval(font.hscale)
    fields["vscale"] = seqval(font.vscale)
    fields["widths"] = list(font.widths.items())
    if "cid2unicode" in v:
        fields["cid2unicode"] = [(int(k), x) for k, x in font.cid2unicode.items()]
    if "unicode_map" in v:
        fields["unicode_map"] = umap_report(font.unicode_map)
    if "fontfile" in v:
        fields["fontfile"] = True
    if "matrix" in v:
        fields["matrix"] = font.matrix
    if isinstance(font, PDFCIDFont):
        fields["cidcoding"] = font.cidcoding
        fields["cmap"] = cmap_report(font.cmap)
        fields["vertical"] = font.vertical
        fields["disps"] = list(font.disps.items())
        fields["default_disp"] = font.default_disp
    fields["is_vertical"] = font.is_vertical()
    fields["is_multibyte"] = font.is_multibyte()
    fields["repr"] = repr(font)
    for method in ("get_ascent", "get_descent", "get_width", "get_height"):
        fields[method] = call_seq(getattr(font, method))
    return Named(type(font).__name__, fields)


def decode_loop(font, s):
    """``for cid in font.decode(s)``, as the interpreter iterates it."""
    out = []
    for cid in font.decode(s):
        out.append(cid)
    return out


def font_queries(font: PDFFont, strings, probes) -> list:
    """``[[s, decode, string_width], ...]`` then ``[[cid, to_unichr,
    char_width, char_disp], ...]`` for the codes of the strings (first
    appearance order) and the probes. Each call is made from a frame at the
    depth ``query_depth`` measures."""
    out_strings = []
    cids: dict[int, None] = {}
    for s in strings:
        decoded = attempt(decode_loop, font, s)
        if isinstance(decoded, list):
            cids.update(dict.fromkeys(decoded))
        out_strings.append([s, decoded, call_seq(font.string_width, s)])
    cids.update(dict.fromkeys(probes))
    out_cids = []
    for cid in cids:
        out_cids.append([cid, attempt(font.to_unichr, cid), call_seq(font.char_width, cid), attempt(font.char_disp, cid)])
    return [out_strings, out_cids]


def query_depth():
    """The depth of the frame ``font_queries`` makes its calls from
    (``attempt``'s, ``call_seq``'s)."""
    return at_depth(lambda: None)[0]


def run_font(spec, table, errors, strings, probes, cipher=None):
    """``(depth, query depth, outcome)``: the font built from the decoded
    dump, its attributes and the queries; ``outcome`` is the canonical
    text."""
    clear_logger_caches()
    doc = load_doc(table, errors, cipher)
    spec = bind(spec, doc)
    rsrcmgr = PDFResourceManager()
    depth, font = at_depth(rsrcmgr.get_font, None, spec)
    qdepth = query_depth()
    if isinstance(font, Exception):
        return depth, qdepth, canon_or_hash(font)
    return depth, qdepth, canon_or_hash([font_attrs(font), font_queries(font, strings, probes)])


def font_case(spec, doc: FontDoc, strings=SAMPLE_STRINGS, probes=PROBES):
    """A case of the set ``fonts``: dump, decode, run."""
    table, errors = dump_doc(doc)
    spec_j, table_j, errors_j, cipher_j = roundtrip(spec, table, errors, doc.cipher)
    depth, qdepth, out = run_font(spec_j, table_j, errors_j, strings, probes, cipher_j)
    return [spec, table, errors, list(strings), list(probes), depth, qdepth, doc.cipher], out


# -- widths ---------------------------------------------------------------------

MISSING_OBJID = 99999
BROKEN_OBJID = 99998
BROKEN = (PDFSyntaxError, ("broken object",))


class Numbers:
    """The numbers of one generated array: all ints within a window (small,
    or around a large base), so that every run ``c1 c2 w`` the parser may
    form, however the items group, has a short range."""

    def __init__(self, rng):
        self.rng = rng
        self.base = rng.choice([0, 0, 0, 2**63 - 3, 2**70, -(2**64)])

    def code(self):
        rng = self.rng
        r = rng.random()
        if self.base == 0 and r < 0.08:
            return rng.choice([True, False])
        if r < 0.16:
            return rng.choice([1.5, 0.0, -0.0, 2.0, math.inf, 1e300, -3.25])
        return self.base + rng.randint(-4, 300)

    def width(self):
        rng = self.rng
        if rng.random() < 0.3:
            return rng.choice([1.5, 0.0, -0.0, 250.0, 1e300, math.inf, -math.inf])
        if self.base == 0 and rng.random() < 0.1:
            return rng.choice([True, False])
        return self.base + rng.randint(-4, 300)


def junk(rng, doc):
    """An object that is not a number."""
    return rng.choice(
        [
            None,
            b"12",
            b"",
            LIT("F1"),
            KWD(b"Tj"),
            {"A": 1},
            PDFStream({"Length": 0}, b""),
            PDFObjRef(doc, MISSING_OBJID),
            "x",
        ][:-1]
    )


def width_value(rng, doc, nums):
    """An item of a widths list: any object."""
    r = rng.random()
    if r < 0.6:
        return nums.width()
    if r < 0.7:
        return [nums.width(), [nums.width()]]
    if r < 0.8:
        objid = len(doc.table) + 1
        doc.table[objid] = nums.width()
        return PDFObjRef(doc, objid)
    return junk(rng, doc)


def ref_to(doc, obj):
    objid = 1 + max([0, *[k for k in doc.table if isinstance(k, int) and k < MISSING_OBJID - 10]])
    doc.table[objid] = obj
    return PDFObjRef(doc, objid)


def widths_array(rng, doc, vertical):
    nums = Numbers(rng)
    seq = []
    for _ in range(rng.randint(0, 8)):
        r = rng.random()
        if r < 0.3:
            c1 = nums.code()
            c2 = c1 + rng.randint(-2, 12) if type(c1) is int else nums.code()
            seq += [c1, c2, *[nums.width() for _ in range(3 if vertical else 1)]]
        elif r < 0.6:
            n = rng.randint(0, 7)
            seq += [nums.code(), [width_value(rng, doc, nums) for _ in range(n)]]
        elif r < 0.7:
            seq.append(nums.code())
        elif r < 0.78:
            seq.append([width_value(rng, doc, nums)])
        elif r < 0.86:
            seq.append(ref_to(doc, rng.choice([nums.code(), [nums.width(), nums.width()]])))
        elif r < 0.9:
            seq.append(PDFObjRef(doc, MISSING_OBJID))
        elif r < 0.93:
            doc.errors[BROKEN_OBJID] = BROKEN
            seq.append(PDFObjRef(doc, BROKEN_OBJID))
        else:
            seq.append(junk(rng, doc))
    return seq


def widths_cases(rng):
    for k in range(1500):
        vertical = k % 2 == 1
        doc = FontDoc()
        seq = widths_array(rng, doc, vertical)
        table, errors = dump_doc(doc)
        seq_j, table_j, errors_j = roundtrip(seq, table, errors)
        doc2 = load_doc(table_j, errors_j)
        seq2 = bind(seq_j, doc2)
        clear_logger_caches()
        depth, out = at_depth(get_widths2 if vertical else get_widths, seq2)
        out = out if isinstance(out, Exception) else list(out.items())
        yield ["get_widths2" if vertical else "get_widths", seq, table, errors, depth], canon_or_hash(out)


# -- getdict --------------------------------------------------------------------


def cff_int(rng, v=None):
    """A CFF DICT integer operand in one of its encodings."""
    if v is None:
        v = rng.choice([0, 1, -1, 107, -107, 108, 1131, -1131, 32767, -32768, 100000, -(2**31), 2**31 - 1])
    if -107 <= v <= 107 and rng.random() < 0.5:
        return bytes([v + 139])
    if 108 <= v <= 1131 and rng.random() < 0.5:
        v -= 108
        return bytes([(v >> 8) + 247, v & 0xFF])
    if -1131 <= v <= -108 and rng.random() < 0.5:
        v = -v - 108
        return bytes([(v >> 8) + 251, v & 0xFF])
    if -32768 <= v <= 32767 and rng.random() < 0.5:
        return b"\x1c" + struct.pack(">h", v)
    return b"\x1d" + struct.pack(">i", max(-(2**31), min(2**31 - 1, v)))


def cff_real(rng):
    nibbles = []
    for _ in range(rng.randint(0, 8)):
        nibbles.append(rng.choice([0, 1, 2, 5, 9, 10, 11, 12, 14, 13] if rng.random() < 0.1 else [0, 1, 2, 3, 5, 7, 9, 10, 11, 12, 14]))
    nibbles.append(15)
    if len(nibbles) % 2:
        nibbles.append(rng.choice([15, 0, 3]))
    return b"\x1e" + bytes((nibbles[i] << 4) | nibbles[i + 1] for i in range(0, len(nibbles), 2))


def dict_data(rng):
    out = b""
    for _ in range(rng.randint(0, 6)):
        r = rng.random()
        if r < 0.5:
            out += cff_int(rng)
        elif r < 0.65:
            out += cff_real(rng)
        elif r < 0.9:
            out += bytes([rng.randint(0, 21)])
        else:
            out += bytes([rng.choice([22, 23, 27, 31, 255, 247, 251, 28, 29, 30])])
    if rng.random() < 0.2 and out:
        out = out[: rng.randrange(len(out))]
    return out


def getdict_cases(rng):
    fixed = [b"", b"\x00", b"\x8b\x00", b"\x1e\x1f", b"\x1e\xff", b"\x1e\xdf", b"\x1e", b"\x1c\x01", b"\x1d\x00\x00",
             b"\xf7", b"\xff\x00\x00\x00\x00\x05", b"\x1e\xa1\x2f\x0c", b"\x1e\xb1\xff", b"\x1e\xe1\xff", b"\x1e\xc1\xff",
             b"\x1e\x1a\x2b\xff", b"\x8b\x8c\x00\x8d\x00", b"\x1e\xf0"]
    for data in fixed + [dict_data(rng) for _ in range(1500)]:
        depth, out = at_depth(getdict, data)
        yield [data, depth], canon_or_hash(out if isinstance(out, Exception) else list(out.items()))


# -- CFF ------------------------------------------------------------------------


def cff_index(items, offsize=None, count=None):
    offsets = [1]
    for it in items:
        offsets.append(offsets[-1] + len(it))
    if offsize is None:
        offsize = 1 if offsets[-1] < 256 else 2 if offsets[-1] < 65536 else 4
    head = struct.pack(">HB", len(items) if count is None else count, offsize)
    return head + b"".join(o.to_bytes(offsize, "big") if offsize else b"" for o in offsets) + b"".join(items)


def cff_program(rng):
    """A CFF program: header, Name, Top DICT, String and Global Subr INDEXes,
    CharStrings, encoding, charset; with generated variations."""
    nglyphs = rng.choice([0, 1, 2, 5, 12, 40])
    strings = [f"glyph{i}".encode() for i in range(rng.randint(0, 6))]
    names = [b"Font" + bytes([65 + i]) for i in range(rng.choice([0, 1, 1, 1, 2]))]
    charstrings = [bytes([14]) * rng.randint(1, 3) for _ in range(nglyphs)]
    offsize = rng.choice([None, None, None, 1, 2, 3, 4, 4, 0, 5, 8] if rng.random() < 0.3 else [None])
    hdrsize = rng.choice([4] * 12 + [5, 2, 0])
    header = bytes([1, 0, hdrsize, 4]) + b"\x00" * max(0, hdrsize - 4)
    name_index = cff_index(names, offsize if rng.random() < 0.3 else None)
    string_index = cff_index(strings, offsize if rng.random() < 0.3 else None)
    gsubr_index = cff_index([b"\x0b"] * rng.randint(0, 2))
    # encoding
    efmt = rng.choice([0, 0, 1, 1, 2, None])
    if efmt == 0:
        codes = [rng.randint(0, 255) for _ in range(rng.randint(0, 10))]
        encoding = bytes([0, len(codes)]) + bytes(codes)
    elif efmt == 1:
        ranges = [(rng.randint(0, 250), rng.randint(0, 5)) for _ in range(rng.randint(0, 4))]
        encoding = bytes([1, len(ranges)]) + b"".join(bytes(r) for r in ranges)
    elif efmt == 2:
        encoding = b"\x02\x00"
    else:
        encoding = None
    # charset
    cfmt = rng.choice([0, 0, 1, 1, 2, 3, None])
    nstd = len(pdffont.CFFFont.STANDARD_STRINGS)
    sid = lambda: rng.choice([rng.randint(0, 390), nstd + rng.randint(0, len(strings) + 1)])  # noqa: E731
    if cfmt == 0:
        charset = b"\x00" + b"".join(struct.pack(">H", sid()) for _ in range(max(0, nglyphs - 1 + rng.choice([0, 0, 0, -1, 1]))))
    elif cfmt == 1:
        ranges = [(rng.randint(0, 40), rng.randint(0, 3)) for _ in range(rng.randint(0, 3))]
        charset = bytes([1, len(ranges)]) + b"".join(bytes(r) for r in ranges)
    elif cfmt is not None:
        charset = bytes([cfmt, 0, 0])
    else:
        charset = None
    # layout: fixed-size Top DICT (5-byte integers)
    top_ops = []
    if charset is not None:
        top_ops.append(15)
    if encoding is not None:
        top_ops.append(16)
    top_ops.append(17)
    if rng.random() < 0.15:
        top_ops.remove(17)
    extra = b""
    if rng.random() < 0.2:
        extra = cff_real(rng) + b"\x0c"
    top_len = len(extra) + 6 * len(top_ops)
    top_index_len = len(cff_index([b"x" * top_len]))
    start = len(header) + len(name_index) + top_index_len + len(string_index) + len(gsubr_index)
    blobs = []
    pos = start
    where = {}
    cs_index = cff_index(charstrings)
    for op, blob in ((17, cs_index), (16, encoding), (15, charset)):
        if blob is None:
            continue
        where[op] = pos
        blobs.append(blob)
        pos += len(blob)
    top = extra
    for op in top_ops:
        value = where.get(op, rng.randint(0, pos))
        if rng.random() < 0.02:
            top += cff_real(rng)
        elif rng.random() < 0.02:
            top += b"\x1d" + struct.pack(">i", -rng.randint(1, 5))
        else:
            top += b"\x1d" + struct.pack(">i", value)
        top += bytes([op])
    top = top.ljust(top_len, b"\x00")[:top_len] if len(top) != top_len else top
    data = header + name_index + cff_index([top]) + string_index + gsubr_index + b"".join(blobs)
    return data


def mutate(rng, data):
    data = bytearray(data)
    r = rng.random()
    if r < 0.35 and data:
        del data[rng.randrange(len(data)) :]
    elif r < 0.75 and data:
        for _ in range(rng.randint(1, 3)):
            data[rng.randrange(len(data))] = rng.randrange(256)
    return bytes(data)


def cff_report(font):
    fp = font.fp
    def items(index):
        return attempt(lambda: [index[i] for i in range(len(index))])
    return Named("CFFFont", {
        "name": font.name,
        "indexes": [(ix.offsets, ix.base) for ix in (font.name_index, font.dict_index, font.string_index, font.subr_index, font.charstring)],
        "top_dict": list(font.top_dict.items()),
        "nglyphs": font.nglyphs,
        "code2gid": list(font.code2gid.items()),
        "gid2code": list(font.gid2code.items()),
        "name2gid": list(font.name2gid.items()),
        "gid2name": list(font.gid2name.items()),
        "items": [items(ix) for ix in (font.name_index, font.dict_index, font.string_index, font.subr_index, font.charstring)],
        "iters": [attempt(lambda ix=ix: list(ix)) for ix in (font.name_index, font.dict_index, font.string_index, font.subr_index, font.charstring)],
        "reprs": [repr(ix) for ix in (font.name_index, font.dict_index, font.string_index, font.subr_index, font.charstring)],
        "tell": fp.tell(),
    })


def cff_case(data):
    depth, font = at_depth(CFFFont, "F", io.BytesIO(data))
    return [data, depth], canon_or_hash(font if isinstance(font, Exception) else cff_report(font))


def cff_cases(rng):
    for k in range(1500):
        data = cff_program(rng)
        if k % 3 == 2:
            data = mutate(rng, data)
        yield cff_case(data)


# -- TrueType -------------------------------------------------------------------


def tt_fmt0(rng):
    return struct.pack(">HHH", 0, 262, 0) + bytes(rng.randrange(256) for _ in range(256))


def tt_fmt2(rng):
    keys = [0] * 256
    nsub = rng.randint(1, 3)
    for b in rng.sample(range(256), rng.randint(0, 4)):
        keys[b] = 8 * rng.randint(0, nsub - 1)
    subs = []
    glyphs = []
    for j in range(nsub):
        n = rng.randint(0, 6)
        subs.append((rng.randint(0, 250), n, rng.randint(-5, 5), len(glyphs)))
        glyphs += [rng.choice([0, rng.randint(1, 500)]) for _ in range(n)]
    out = struct.pack(">HHH", 2, 0, 0) + b"".join(struct.pack(">H", k) for k in keys)
    head_len = len(out)
    for j, (first, n, delta, gstart) in enumerate(subs):
        # idRangeOffset from its own position (2 bytes before the end of the subheader)
        here = head_len + 8 * j + 6
        target = head_len + 8 * nsub + 2 * gstart
        out += struct.pack(">HHhH", first, n, delta, target - here)
    out += b"".join(struct.pack(">H", g) for g in glyphs)
    return out


def tt_fmt4(rng):
    segs = []
    start = 0
    for _ in range(rng.randint(0, 5)):
        sc = start + rng.randint(0, 40)
        ec = sc + rng.randint(-1, 30)
        segs.append((sc, max(ec, 0), rng.randint(-300, 300), rng.random() < 0.4))
        start = max(sc, ec) + 1
    if rng.random() < 0.8:
        segs.append((0xFFFF, 0xFFFF, 1, False))
    n = len(segs)
    glyphs = []
    idrs = []
    for k, (sc, ec, d, use) in enumerate(segs):
        if use and ec >= sc:
            idrs.append(2 * n + 2 * len(glyphs))  # pdfminer seeks from the start of the array
            glyphs += [rng.randint(0, 600) for _ in range(ec - sc + 1)]
        else:
            idrs.append(0)
    body = struct.pack(">HHHH", 2 * n, 0, 0, 0)
    body += b"".join(struct.pack(">H", s[1]) for s in segs) + b"\x00\x00"
    body += b"".join(struct.pack(">H", s[0]) for s in segs)
    body += b"".join(struct.pack(">h", s[2]) for s in segs)
    body += b"".join(struct.pack(">H", i) for i in idrs)
    body += b"".join(struct.pack(">H", g) for g in glyphs)
    return struct.pack(">HHH", 4, 14 + len(body) - 8, 0) + body


def tt_fmt6(rng):
    n = rng.randint(0, 20)
    return struct.pack(">HHHHH", 6, 10 + 2 * n, 0, rng.randint(0, 60000), n) + b"".join(
        struct.pack(">H", rng.randint(0, 900)) for _ in range(n)
    )


def tt_fmt10(rng):
    n = rng.randint(0, 20)
    return struct.pack(">HHIIII", 10, 0, 20 + 2 * n, 0, rng.choice([0, 0x10000, 0x1F600, 0x10FFF0, 0xFFFFFFF0]), n) + b"".join(
        struct.pack(">H", rng.randint(0, 900)) for _ in range(n)
    )


def tt_fmt12(rng):
    groups = []
    for _ in range(rng.randint(0, 4)):
        sc = rng.choice([0x20, 0x4E00, 0x1F600, 0x10FFF0, 0x10FFFE, 0xD7F0, 0x7FFFFFF0, 0xFFFFFFF0])
        groups.append((sc, min(0xFFFFFFFF, sc + rng.randint(-1, 20)), rng.randint(0, 1000)))
    return struct.pack(">HHIII", 12, 0, 16 + 12 * len(groups), 0, len(groups)) + b"".join(struct.pack(">III", *g) for g in groups)


def tt_other(rng):
    fmt = rng.choice([8, 13, 14, 1, 3])
    return struct.pack(">HHI", fmt, 0, 16) + bytes(8)


SUBTABLES = [tt_fmt0, tt_fmt2, tt_fmt4, tt_fmt4, tt_fmt6, tt_fmt10, tt_fmt12, tt_other]


def tt_cmap(rng):
    subs = []
    for _ in range(rng.choice([0, 1, 1, 1, 2, 2, 3])):
        platform = rng.choice([(0, 3), (0, 4), (3, 1), (3, 10), (3, 1), (0, 3), (3, 0), (1, 0)])
        subs.append((platform, rng.choice(SUBTABLES)(rng)))
    head = struct.pack(">HH", 0, len(subs))
    offset = 4 + 8 * len(subs)
    records = b""
    bodies = b""
    for (p, e), body in subs:
        records += struct.pack(">HHI", p, e, offset + len(bodies))
        bodies += body
    return head + records + bodies


def tt_program(rng):
    tables = {}
    if rng.random() < 0.9:
        tables[b"cmap"] = tt_cmap(rng)
    for tag in rng.sample([b"head", b"glyf", b"loca", b"name"], rng.randint(0, 2)):
        tables[tag] = bytes(rng.randrange(256) for _ in range(rng.randint(0, 12)))
    version = rng.choice([b"\x00\x01\x00\x00", b"true", b"OTTO"])
    offset = 12 + 16 * len(tables)
    directory = b""
    data = b""
    for tag, body in tables.items():
        directory += struct.pack(">4sIII", tag, 0, offset + len(data), len(body))
        data += body
    return version + struct.pack(">HHHH", len(tables) + rng.choice([0, 0, 0, 1, -1 if tables else 0]), 0, 0, 0) + directory + data


def tt_cost(data) -> int:
    """The number of codes ``create_unicode_map`` would loop over without
    reading (format 4 segments without glyph array, format 12 groups), so
    that generated mutations never ask pdfminer for billions of steps."""
    try:
        f = TrueTypeFont("cost", io.BytesIO(data))
        if b"cmap" not in f.tables:
            return 0
        base, _ = f.tables[b"cmap"]
        fp = f.fp
        fp.seek(base)
        _, n = struct.unpack(">HH", fp.read(4))
        subs = [struct.unpack(">HHL", fp.read(8)) for _ in range(n)]
        cost = 0
        for p, e, off in subs:
            fp.seek(base + off)
            (fmt,) = struct.unpack(">H", fp.read(2))
            if fmt == 4:
                fp.read(4)
                (seg, _, _, _) = struct.unpack(">HHHH", fp.read(8))
                seg //= 2
                ecs = struct.unpack(f">{seg}H", fp.read(2 * seg))
                fp.read(2)
                scs = struct.unpack(f">{seg}H", fp.read(2 * seg))
                cost += sum(max(0, ec - sc + 1) for ec, sc in zip(ecs, scs))
            elif fmt == 12:
                fp.read(10)
                (ng,) = struct.unpack(">I", fp.read(4))
                for _ in range(min(ng, 10000)):
                    sc, ec, _ = struct.unpack(">III", fp.read(12))
                    cost += max(0, ec - sc + 1)
        return cost
    except (struct.error, ValueError, OverflowError):
        return 0


def tt_report(font):
    return Named("TrueTypeFont", {"name": font.name, "fonttype": font.fonttype, "tables": list(font.tables.items())})


def tt_case(data):
    clear_logger_caches()
    depth, font = at_depth(TrueTypeFont, "F", io.BytesIO(data))
    if isinstance(font, Exception):
        return [data, depth], canon_or_hash(font)
    _, umap = at_depth(font.create_unicode_map)
    return [data, depth], canon_or_hash([tt_report(font), umap if isinstance(umap, Exception) else umap_report(umap)])


def truetype_cases(rng):
    k = 0
    while k < 2000:
        data = tt_program(rng)
        if k % 3:
            data = mutate(rng, data)
        if tt_cost(data) > 200_000:
            continue
        k += 1
        yield tt_case(data)


# -- Type 1 font program headers ------------------------------------------------


GLYPH_NAMES = ["space", "A", "a", "Aacute", "uni0041", "uni00410042", "u1F600", "f_i", "a.sc", "A_B_space",
               ".notdef", "nonexistentglyph", "u0041_u0042", "g123", "uni004", "uniXYZW", "uniD800", "uniDBFF0041"]
#: Names whose ``name2unicode`` raises something else than ``KeyError``.
BAD_GLYPH_NAMES = ["u110000", "uni0041_u110000", "uniFFFFF"]


def glyph_name(rng) -> str:
    return rng.choice(BAD_GLYPH_NAMES) if rng.random() < 0.03 else rng.choice(GLYPH_NAMES)


def type1_token(rng):
    r = rng.random()
    if r < 0.45:
        key = rng.choice([str(rng.randint(0, 255)), str(rng.randint(0, 255)), "true", "1.5", "99999999999999999999", "-1",
                          "/a", "(s)"])
        value = rng.choice(["/" + glyph_name(rng)] * 4 + ["/#ff#fe", "1", "(x)", "[/a]"])
        return f"dup {key} {value} put"
    if r < 0.55:
        return rng.choice(["0 1 255 {1 index exch /.notdef put} for", "readonly def", "/Encoding 256 array", "begin",
                           "end", "currentdict end", "/FontName /Foo def"])
    if r < 0.62:
        return rng.choice(["put", "1 put", "[1 2] put", "<< /a 1 >>", "<< /a >>", "{ 1 2 put }", "]", ">>", "}"])
    if r < 0.7:
        return rng.choice(["% comment", "<41> /A put", "(paren\\) string) pop", "<<", "["])
    return f"{rng.randint(0, 300)} /{glyph_name(rng)} put"


def type1_program(rng):
    head = "%!PS-AdobeFont-1.0: Foo 001.000\n/Encoding 256 array\n0 1 255 {1 index exch /.notdef put} for\n"
    body = "\n".join(type1_token(rng) for _ in range(rng.randint(0, 25)))
    data = (head if rng.random() < 0.8 else "") + body + "\nreadonly def\ncurrentfile eexec\n"
    data = data.encode("latin-1")
    if rng.random() < 0.3:
        data += bytes(rng.randrange(256) for _ in range(rng.randint(0, 40)))
    return data


def type1_encoding(data):
    """``Type1FontHeaderParser(BytesIO(data)).get_encoding()``, as
    ``PDFType1Font.__init__`` calls it."""
    parser = Type1FontHeaderParser(io.BytesIO(data))
    return parser.get_encoding()


def type1_case(data):
    clear_logger_caches()
    depth, out = at_depth(type1_encoding, data)
    return [data, depth], canon_or_hash(out if isinstance(out, Exception) else [(int(k), v) for k, v in out.items()])


def type1_cases(rng):
    for k in range(1500):
        data = type1_program(rng)
        if k % 4 == 3:
            data = mutate(rng, data)
        yield type1_case(data)


# -- font dictionaries ----------------------------------------------------------


def predefined_names() -> list[str]:
    """Every predefined CMap of pdfminer's data, unicode maps excluded."""
    import pdfminer

    directory = __import__("pathlib").Path(pdfminer.__file__).parent / "cmap"
    return sorted(p.name[: -len(".json.gz")] for p in directory.glob("*.json.gz") if not p.name.startswith("to-unicode-"))


PREDEFINED = predefined_names()
ORDERINGS = [b"Japan1", b"CNS1", b"GB1", b"Korea1", b"Identity", b"UCS", b"Unknown"]
STANDARD_FONTS = list(pdffont.FONT_METRICS)
ENCODING_NAMES = ["StandardEncoding", "WinAnsiEncoding", "MacRomanEncoding", "PDFDocEncoding", "MacExpertEncoding",
                  "Symbol", "Identity-H", ""]


class Spec:
    """Helpers to build the objects of one font dictionary in a document."""

    def __init__(self, rng, doc: FontDoc):
        self.rng = rng
        self.doc = doc

    def ref(self, obj):
        if self.rng.random() < 0.1:
            return self.deep_ref(obj)
        return ref_to(self.doc, obj)

    def deep_ref(self, obj):
        """A reference whose ``getobj`` goes further down (``FontDoc._deep``)."""
        deep = [k for k in self.doc.table if type(k) is int and k >= DEEP_OBJIDS]
        objid = max([DEEP_OBJIDS, *deep]) + 1
        self.doc.table[objid] = obj
        return PDFObjRef(self.doc, objid)

    def maybe_ref(self, obj, p=0.15):
        return self.ref(obj) if self.rng.random() < p else obj

    def missing_ref(self):
        return PDFObjRef(self.doc, MISSING_OBJID)

    def broken_ref(self):
        self.doc.errors[BROKEN_OBJID] = BROKEN
        return PDFObjRef(self.doc, BROKEN_OBJID)

    def junk(self):
        return self.rng.choice([None, b"x", 7, 1.5, True, LIT("X"), KWD(b"Tj"), [1], {"A": 1},
                                PDFStream({"Length": 0}, b""), self.missing_ref()])

    def number(self):
        return self.rng.choice([0, 1, -1, 250, 1000, -250, 0.5, -0.0, 1e300, math.inf, True, False, 2**70, 600.25])

    def stream(self, data: bytes, attrs=None, compress=None):
        attrs = dict(attrs or {})
        if compress is None:
            compress = self.rng.random() < 0.4
        if compress:
            attrs["Filter"] = LIT("FlateDecode")
            data = __import__("zlib").compress(data)
            r = self.rng.random()
            if r < 0.2:
                # filters given by references, with a predictor that
                # changes nothing, or ASCIIHex above Flate
                attrs["Filter"] = self.ref(self.rng.choice([LIT("FlateDecode"), [LIT("FlateDecode")]]))
                attrs["DecodeParms"] = self.maybe_ref(self.rng.choice([{"Predictor": 1}, {"Predictor": self.ref(1)}, {}]), 0.5)
            elif r < 0.3:
                attrs["Filter"] = [LIT("ASCIIHexDecode"), self.maybe_ref(LIT("FlateDecode"), 0.5)]
                data = data.hex().encode() + b">"
        attrs["Length"] = len(data)
        return PDFStream(attrs, data)

    def tounicode_data(self):
        rng = self.rng
        lines = ["/CIDInit /ProcSet findresource begin", "12 dict begin", "begincmap",
                 "1 begincodespacerange <00> <FF> endcodespacerange"]
        for _ in range(rng.randint(0, 3)):
            entries = []
            for _ in range(rng.randint(1, 4)):
                c = rng.randint(0, 260)
                entries.append(f"<{c:04x}> " + rng.choice(["<0041>", "<00410042>", "<D83DDE00>", "<D800>", "/space",
                                                            "/A", "<00A0>", "<0020>", "65", "/uniD800"]))
            lines.append(f"{len(entries)} beginbfchar " + " ".join(entries) + " endbfchar")
        if rng.random() < 0.5:
            a = rng.randint(0, 200)
            lines.append(f"1 beginbfrange <{a:04x}> <{a + rng.randint(0, 5):04x}> " +
                         rng.choice(["<0061>", "[<0041> <0042> /C]", "<D7FF>"]) + " endbfrange")
        if rng.random() < 0.1:
            lines.append("1 beginbfchar <01> endbfchar <<")
        r = rng.random()
        if r < 0.15:
            # usecmap: CMapDB.get_cmap from the parser (an identity, a
            # predefined CMap, a missing one, a unicode map's data)
            name = rng.choice(["Identity-H", "Foo-H", rng.choice(PREDEFINED), "to-unicode-Adobe-Japan1"])
            lines.insert(3, f"/{name} usecmap")
        elif r < 0.25:
            # names through name2unicode (ligatures, invalid code points)
            # and the warnings of _warn_once
            lines.append(rng.choice(["1 beginbfrange <10> <11> [/f_i /uniD800] endbfrange",
                                     "1 beginbfrange <10> <11> [/A_B /f_uniD800] endbfrange",
                                     "2 beginbfrange /x <01> <0041> <01> <0002> <0041> endbfrange"]))
        lines += ["endcmap", "CMapName currentdict /CMap defineresource pop", "end", "end"]
        return "\n".join(lines).encode()

    def tounicode(self):
        r = self.rng.random()
        if r < 0.75:
            return self.maybe_ref(self.stream(self.tounicode_data()), 0.4)
        return self.rng.choice([LIT("Identity-H"), LIT("Foo"), self.missing_ref(), b"x", self.junk()])

    def bbox(self):
        rng = self.rng
        r = rng.random()
        if r < 0.6:
            return [self.maybe_ref(rng.choice([0, -50, 1000, 0.5, -0.0, True])) for _ in range(4)]
        return rng.choice([[0, 0, 0], [1, 2, 3, 4, 5], b"1234", {"a": 1, "b": 2, "1": 3, "2.5": 4},
                           PDFStream({"0": 1}, b""), None, [b"1", b"2", b" 3 ", b"4e1"], [[1], 2, 3, 4], self.missing_ref()])

    def descriptor(self, fontfile=None):
        rng = self.rng
        d = {"Type": LIT("FontDescriptor")}
        if rng.random() < 0.8:
            d["FontName"] = rng.choice([LIT("ABCDEF+Foo"), LIT(b"bad\xff"), b"Foo", 5, [LIT("A")], self.missing_ref()])
        for key in ("Flags", "Ascent", "Descent", "ItalicAngle", "MissingWidth", "Leading", "CapHeight"):
            if rng.random() < 0.6:
                d[key] = self.maybe_ref(self.number() if rng.random() < 0.85 else self.junk())
        if rng.random() < 0.85:
            d["FontBBox"] = self.maybe_ref(self.bbox())
        if fontfile is not None:
            d.update(fontfile)
        return d

    def widths(self):
        rng = self.rng
        r = rng.random()
        if r < 0.7:
            nums = Numbers(rng)
            return [width_value(rng, self.doc, nums) for _ in range(rng.randint(0, 12))]
        return rng.choice([None, 5, LIT("W"), self.missing_ref(), {"A": 1}])

    def differences(self):
        rng = self.rng
        diff = []
        for _ in range(rng.randint(0, 10)):
            r = rng.random()
            if r < 0.3:
                diff.append(rng.choice([0, 32, 65, 128, 255, 300, True, -1, 2**70]))
            elif r < 0.8:
                diff.append(LIT(glyph_name(rng)))
            else:
                diff.append(rng.choice([LIT(b"\xff\xfe"), 1.5, b"A", None, self.missing_ref(), [65, LIT("A")]]))
        return diff

    def simple_encoding(self):
        rng = self.rng
        r = rng.random()
        if r < 0.4:
            return LIT(rng.choice(ENCODING_NAMES))
        if r < 0.85:
            enc = {"Type": LIT("Encoding")}
            if rng.random() < 0.6:
                enc["BaseEncoding"] = self.maybe_ref(rng.choice([LIT(rng.choice(ENCODING_NAMES)), b"WinAnsiEncoding", 3]))
            if rng.random() < 0.8:
                enc["Differences"] = self.maybe_ref(self.differences() if rng.random() < 0.9 else self.junk())
            return self.maybe_ref(enc, 0.3)
        return self.junk()

    def type1_fontfile(self):
        rng = self.rng
        data = type1_program(rng)
        attrs = {}
        if rng.random() < 0.9:
            attrs["Length1"] = rng.choice([len(data), len(data) // 2, 0, -10, True, 2**70, 1.5, self.missing_ref()])
        return self.stream(data, attrs)

    def simple(self, subtype):
        rng = self.rng
        spec = {"Type": LIT("Font")}
        if subtype is not None:
            spec["Subtype"] = subtype
        r = rng.random()
        if r < 0.35:
            spec["BaseFont"] = LIT(rng.choice(STANDARD_FONTS))
        elif r < 0.85:
            spec["BaseFont"] = LIT(rng.choice(["ABCDEF+Foo", "Foo", "Helvetica-Narrow", "Arial,Bold"]))
        elif r < 0.9:
            spec["BaseFont"] = rng.choice([b"Helvetica", 3, LIT(b"\xff"), [LIT("Courier")], self.ref(LIT("Courier"))])
        if rng.random() < 0.8:
            spec["FirstChar"] = self.maybe_ref(rng.choice([0, 32, 65, True, 1.5, -3, 2**70]))
        if rng.random() < 0.8:
            spec["Widths"] = self.maybe_ref(self.widths())
        if rng.random() < 0.7:
            spec["Encoding"] = self.simple_encoding()
        if rng.random() < 0.3:
            spec["ToUnicode"] = self.tounicode()
        if rng.random() < 0.8:
            fontfile = None
            if rng.random() < 0.4:
                fontfile = {rng.choice(["FontFile", "FontFile", "FontFile2", "FontFile3"]): self.maybe_ref(self.type1_fontfile(), 0.5)}
            spec["FontDescriptor"] = self.maybe_ref(self.descriptor(fontfile) if rng.random() < 0.95 else self.junk(), 0.5)
        return spec

    def matrix(self):
        rng = self.rng
        r = rng.random()
        if r < 0.6:
            return [self.maybe_ref(rng.choice([0.001, 0, 1, 0.5, -0.001, True, 2, 1e-300])) for _ in range(6)]
        if r < 0.8:
            return rng.choice([[0.001, 0, 0, 0.001, 0], [1, 0, 0, 1, 0, 0, 0], [], None, b"abcdef"])
        items = [0.001, 0, 0, 0.001, 0, 0]
        k = rng.randrange(6)
        items[k] = rng.choice([b"ab", [1, 2], LIT("x"), None, {"a": 1}, self.missing_ref(), b""])
        if k in (0, 1) and rng.random() < 0.5:
            items[k + 2] = rng.choice([b"cd", [3], 0, 1])
        return items

    def type3(self):
        rng = self.rng
        spec = self.simple(LIT("Type3"))
        spec.pop("BaseFont", None)
        if rng.random() < 0.9:
            spec["FontMatrix"] = self.maybe_ref(self.matrix())
        if rng.random() < 0.8:
            spec["FontBBox"] = self.maybe_ref(self.bbox())
        if rng.random() < 0.5:
            spec.pop("FontDescriptor", None)
        if rng.random() < 0.5:
            spec["CharProcs"] = {"a": self.stream(b"0 0 d0", compress=False)}
            spec["Resources"] = {"ProcSet": [LIT("PDF")]}
        return spec

    def cidsysteminfo(self):
        rng = self.rng
        info = {}
        if rng.random() < 0.9:
            info["Registry"] = self.maybe_ref(rng.choice([b"Adobe"] * 30 + [b" Adobe\xa0", b"\x85Adobe", b"Foo", 5, LIT("Adobe"),
                                                         PDFStream({}, b"")]))
        if rng.random() < 0.9:
            info["Ordering"] = self.maybe_ref(rng.choice(ORDERINGS * 6 + [b"Identity ", b"\tUCS", 3, None]))
        info["Supplement"] = 0
        return self.maybe_ref(info) if rng.random() < 0.95 else self.junk()

    def cid_encoding(self, name=None):
        rng = self.rng
        if name is not None:
            return LIT(name)
        r = rng.random()
        if r < 0.5:
            return LIT(rng.choice(["Identity-H", "Identity-V", "DLIdent-H", "DLIdent-V", "OneByteIdentityH",
                                   "OneByteIdentityV", "Foo-H", "unknown", rng.choice(PREDEFINED)]))
        if r < 0.65:
            d = {"Type": LIT("CMap")}
            if rng.random() < 0.8:
                d["CMapName"] = rng.choice([LIT("Identity-V"), LIT(rng.choice(PREDEFINED)), b"Identity-H", 3])
            return self.maybe_ref(d)
        if r < 0.8:
            attrs = {"Type": LIT("CMap")}
            if rng.random() < 0.7:
                attrs["CMapName"] = rng.choice([LIT("Identity-H"), LIT("Custom"), LIT(rng.choice(PREDEFINED))])
            return self.maybe_ref(self.stream(b"%!PS-Adobe-3.0 Resource-CMap\nbegincmap endcmap", attrs))
        return rng.choice([[LIT("Identity-H")], b"Identity-H", 3, KWD(b"Identity-H"), None, self.missing_ref(), LIT("Identity-H ")])

    def w(self):
        rng = self.rng
        if rng.random() < 0.85:
            return widths_array(rng, self.doc, vertical=False)
        return self.junk()

    def w2(self):
        rng = self.rng
        if rng.random() < 0.85:
            return widths_array(rng, self.doc, vertical=True)
        return self.junk()

    def truetype_fontfile(self):
        rng = self.rng
        while True:
            data = tt_program(rng)
            if rng.random() < 0.3:
                data = mutate(rng, data)
            if tt_cost(data) <= 200_000:
                return self.stream(data, {"Length1": len(data)})

    def cid(self, subtype=None, encoding=None, ordering=None):
        rng = self.rng
        spec = {"Type": LIT("Font"), "Subtype": subtype or LIT(rng.choice(["CIDFontType0", "CIDFontType2"]))}
        if rng.random() < 0.9:
            spec["BaseFont"] = rng.choice([LIT("ABCDEF+MSMincho"), LIT("Foo"), b"Foo", self.ref(LIT("Bar"))])
        if ordering is not None:
            spec["CIDSystemInfo"] = {"Registry": b"Adobe", "Ordering": ordering, "Supplement": 2}
        elif rng.random() < 0.9:
            spec["CIDSystemInfo"] = self.cidsysteminfo()
        if encoding is not None or rng.random() < 0.85:
            spec["Encoding"] = self.cid_encoding(encoding)
        if rng.random() < 0.85:
            fontfile = None
            if rng.random() < 0.5:
                fontfile = {"FontFile2": self.maybe_ref(self.truetype_fontfile(), 0.5)}
            spec["FontDescriptor"] = self.maybe_ref(self.descriptor(fontfile) if rng.random() < 0.95 else self.junk(), 0.4)
        if rng.random() < 0.4:
            spec["ToUnicode"] = self.tounicode()
        if rng.random() < 0.7:
            spec["W"] = self.maybe_ref(self.w())
        if rng.random() < 0.5:
            spec["W2"] = self.maybe_ref(self.w2())
        if rng.random() < 0.4:
            spec["DW"] = self.maybe_ref(rng.choice([1000, 500.5, True, None, [1], LIT("x"), b"12", self.missing_ref()]))
        if rng.random() < 0.4:
            spec["DW2"] = self.maybe_ref(rng.choice([[880, -1000], [900], [1, 2, 3], {"a": 1, "b": 2}, b"ab", 5, None,
                                                      PDFStream({}, b""), [self.missing_ref(), 500], [None, 700],
                                                      [LIT("x"), b"y"], self.missing_ref()]))
        return spec

    def type0(self, depth=1):
        rng = self.rng
        spec = {"Type": LIT("Font"), "Subtype": LIT("Type0")}
        if rng.random() < 0.9:
            spec["BaseFont"] = LIT("ABCDEF+Foo")
        if rng.random() < 0.85:
            spec["Encoding"] = self.maybe_ref(self.cid_encoding(), 0.3)
        if rng.random() < 0.4:
            spec["ToUnicode"] = self.tounicode()
        r = rng.random()
        if r < 0.85:
            if depth < 3 and rng.random() < 0.15:
                child = self.type0(depth + 1)
            elif rng.random() < 0.85:
                child = self.cid()
            else:
                child = self.simple(LIT(rng.choice(["Type1", "TrueType"])))
            spec["DescendantFonts"] = self.maybe_ref([self.maybe_ref(child, 0.6), *([self.junk()] if rng.random() < 0.2 else [])], 0.3)
        elif r < 0.95:
            spec["DescendantFonts"] = rng.choice([[], [5], 5, LIT("x"), [self.missing_ref()], [[{}]]])
        return spec

    def any(self):
        rng = self.rng
        r = rng.random()
        if r < 0.25:
            return self.simple(LIT(rng.choice(["Type1", "Type1", "MMType1", "TrueType", "TrueType"])))
        if r < 0.32:
            return self.simple(rng.choice([None, LIT("Foo"), b"Type1", 3, [LIT("Type3")], self.ref(LIT("TrueType"))]))
        if r < 0.47:
            return self.type3()
        if r < 0.7:
            return self.cid()
        return self.type0()


# -- encrypted documents ----------------------------------------------------------


def cipher_specs(rng) -> list[list]:
    """``[docid, param, password]`` of a handler of every decryption path
    (``oracle.unit.crypto.deep_handlers``: RC4 40 and 128 bits, V4 with RC4,
    AES-128 and identity crypt filters, AES-256, and an AES-256 handler whose
    48-byte key AES refuses)."""
    return [[docid, param, password] for _name, docid, param, password in deep_handlers(rng)]


def encrypt_data(h, objid, genno, plain: bytes, attrs, rng) -> bytes:
    """Raw data that ``h.decrypt(objid, genno, raw, attrs)`` deciphers to
    ``plain``: ``plain`` itself for an identity filter, RC4 being its own
    inverse, AES-CBC under the key of the object (PKCS#7 padding, which
    ``unpad_aes`` removes); for a handler whose decryption raises, and for
    one stream in ten, a 5-byte stream (an IV that ``modes.CBC`` refuses, or
    RC4 data)."""
    if rng.random() < 0.1:
        return rng.randbytes(5)
    once = attempt(h.decrypt, objid, genno, plain, attrs)
    if once == plain:
        return plain
    if isinstance(once, bytes) and attempt(h.decrypt, objid, genno, once, attrs) == plain:
        return once
    if isinstance(h, PDFStandardSecurityHandlerV5):
        key = h.key
    else:
        salted = h.key + struct.pack("<L", objid)[:3] + struct.pack("<L", genno)[:2] + b"sAlT"
        key = hashlib.md5(salted).digest()[:16]
    iv = rng.randbytes(16)
    n = 16 - len(plain) % 16
    try:
        raw = iv + aes_cbc(key, iv, plain + bytes([n]) * n)
    except ValueError:  # a key of a size AES refuses: the decryption raises
        return iv + bytes(32)
    assert h.decrypt(objid, genno, raw, attrs) == plain
    return raw


def encrypt_case(rng, spec, doc: FontDoc, cipher) -> None:
    """Makes the document of a generated case an encrypted one: every stream
    of the case gets an object number (``1000 + k``) and a generation, and
    its raw data is encrypted for the handler of ``cipher`` (the streams of
    an encrypted document, deciphered by ``PDFStream.decode``)."""
    h = security_handler(cipher)
    doc.cipher = cipher
    for k, s in enumerate(streams_of(spec, doc)):
        s.objid, s.genno = 1000 + k, rng.choice([0, 0, 0, 1, 7, 65535])
        s.rawdata = encrypt_data(h, s.objid, s.genno, s.rawdata, s.attrs, rng)


def font_spec_cases(rng):
    for _ in range(3000):
        doc = FontDoc()
        spec = Spec(rng, doc).any()
        yield font_case(spec, doc)
    # fonts of encrypted documents: their streams deciphered by a security
    # handler of every kind
    ciphers = cipher_specs(rng)
    for k in range(400):
        doc = FontDoc()
        spec = Spec(rng, doc).any()
        encrypt_case(rng, spec, doc, ciphers[k % len(ciphers)])
        yield font_case(spec, doc)
    # every predefined CMap, as the encoding of a CID font and of a Type 0
    # font, with its character collection's unicode map
    for name in PREDEFINED:
        for ordering in (rng.choice(ORDERINGS[:4]), b"Identity"):
            doc = FontDoc()
            spec = Spec(rng, doc).cid(encoding=name, ordering=ordering)
            spec.pop("ToUnicode", None)
            yield font_case(spec, doc, SAMPLE_STRINGS + [b"\x30\x42\x8b\xa0", b"\xa4\xa2\x41"])
    # the encoding of an embedded Type 1 program read up to /Length1 (every
    # kind of int, small or large, of either sign)
    for k in range(12):
        doc = FontDoc()
        s = Spec(rng, doc)
        data = b"/Encoding 256 array\ndup 65 /A put\ndup 66 /B put\n" + type1_program(rng)
        length1 = [len(data), len(data) // 2, 0, -10, -len(data) - 5, -(2**70), 2**70, True, 1.5, s.missing_ref(),
                   s.ref(len(data) - 3), None][k]
        attrs = {} if length1 is None else {"Length1": length1}
        spec = {"Type": LIT("Font"), "Subtype": LIT("Type1"), "BaseFont": LIT("ABCDEF+Foo"),
                "FontDescriptor": s.descriptor({"FontFile": s.stream(data, attrs)})}
        yield font_case(spec, doc)


# -- recursion depth --------------------------------------------------------------


def _nested(extra: int, f, *args):
    if extra == 0:
        try:
            return f(*args)
        except Exception as e:  # the exception is the expected result
            return e
    return _nested(extra - 1, f, *args)


def nested(extra: int, f, *args):
    """``(depth, f(*args))`` with ``f`` called under ``extra`` more Python
    frames; ``depth`` is that of the frame calling ``f`` (as ``at_depth``
    gives it), computed rather than measured there, so that the harness
    takes no frame below the call."""
    base = depth_here()
    out = _nested(extra, f, *args)
    return {"py": base["py"] + extra + 1, "c_left": base["c_left"]}, out


def is_recursion(out) -> bool:
    return isinstance(out, RecursionError)


def threshold(run):
    """``(run(e), run(e + 1))`` where ``e`` is the largest number of extra
    frames under which ``run(extra)`` does not raise ``RecursionError``,
    run from this frame (``None`` if it raises without extra frames, or
    never)."""
    if is_recursion(run(0)[1]):
        return None
    top = sys.getrecursionlimit() - depth_here()["py"] - 3
    lo, hi = 0, top
    while lo < hi:
        mid = (lo + hi + 1) // 2
        if is_recursion(run(mid)[1]):
            hi = mid - 1
        else:
            lo = mid
    if lo >= top:
        return None
    # run again from this frame: the depth of the caller decides
    return run(lo), run(lo + 1)


def built_font(spec_j, table_j, errors_j, cipher_j=None):
    """A fresh font of a decoded dump, built at the generator's depth."""
    doc = load_doc(table_j, errors_j, cipher_j)
    return PDFResourceManager().get_font(None, bind(spec_j, doc))


def chain_spec(doc: FontDoc, rng, length: int, leaf) -> dict:
    """A ``Type0`` font whose descendant is a ``Type0`` font ... ``length``
    levels down to ``leaf``, each level an indirect object."""
    spec = leaf
    for _ in range(length):
        spec = {"Subtype": LIT("Type0"), "Encoding": LIT("Identity-H"), "DescendantFonts": [ref_to(doc, spec)]}
    return spec


def deep_list(n: int, leaf):
    x = leaf
    for _ in range(n):
        x = [x]
    return x


def depth_specs(rng):
    """``(spec, doc)`` of fonts of every kind, chains and deep values."""
    for _ in range(150):
        doc = FontDoc()
        yield Spec(rng, doc).any(), doc
    for length in (1, 3, 40, 250, 600):
        for kind in ("cid", "simple", "type3"):
            doc = FontDoc()
            s = Spec(rng, doc)
            leaf = {"cid": s.cid, "simple": lambda: s.simple(LIT("Type1")), "type3": s.type3}[kind]()
            yield chain_spec(doc, rng, length, leaf), doc
    for n in (50, 300, 700):
        doc = FontDoc()
        spec = Spec(rng, doc).simple(LIT("TrueType"))
        spec["Widths"] = [1, deep_list(n, 2), 3]
        yield spec, doc
        doc = FontDoc()
        spec = Spec(rng, doc).cid()
        spec["FontDescriptor"] = {"FontBBox": [deep_list(n, 0), 1, 2, 3]}
        yield spec, doc
    # CMaps loaded from the font (its encoding, its collection's unicode
    # map) and from its /ToUnicode stream (usecmap), found or not
    for name in ("UniJIS-UCS2-H", "GBK-EUC-V", "Foo-H", "Identity-V"):
        for ordering in (b"Japan1", b"Korea1", b"Foo"):
            doc = FontDoc()
            spec = Spec(rng, doc).cid(encoding=name, ordering=ordering)
            spec.pop("ToUnicode", None)
            yield spec, doc
    for name in ("Identity-H", "Foo-H", "UniCNS-UCS2-V", "to-unicode-Adobe-Japan1"):
        for subtype in ("TrueType", "CIDFontType2"):
            doc = FontDoc()
            s = Spec(rng, doc)
            spec = s.simple(LIT(subtype)) if subtype == "TrueType" else s.cid()
            data = f"begincmap\n/{name} usecmap\n1 beginbfchar <41> <0042> endbfchar\nendcmap".encode()
            spec["ToUnicode"] = s.stream(data, compress=False)
            yield spec, doc
    # /ToUnicode streams whose filters and parameters are references
    # (resolved by get_filters, deeper than get_any)
    zlib = __import__("zlib")
    data = zlib.compress(b"begincmap 1 beginbfchar <41> <0042> endbfchar endcmap")
    for filters, params in ((LIT("FlateDecode"), None), ([LIT("FlateDecode")], {"Predictor": 1}),
                            ([LIT("ASCIIHexDecode"), LIT("FlateDecode")], None)):
        for subtype in ("TrueType", "CIDFontType2"):
            doc = FontDoc()
            s = Spec(rng, doc)
            spec = s.simple(LIT(subtype)) if subtype == "TrueType" else s.cid()
            raw = data.hex().encode() + b">" if isinstance(filters, list) and len(filters) == 2 else data
            attrs = {"Filter": s.deep_ref(filters), "Length": len(raw)}
            if params is not None:
                attrs["DecodeParms"] = s.deep_ref(params)
            spec["ToUnicode"] = PDFStream(attrs, raw)
            yield spec, doc
    # embedded programs whose filter is such a reference, or not (then the
    # construction of the Type 1 header parser is the deepest point)
    for key, deep in (("FontFile", True), ("FontFile2", True), ("FontFile", False)):
        doc = FontDoc()
        s = Spec(rng, doc)
        program = type1_program(rng) if key == "FontFile" else tt_program(rng)
        packed = zlib.compress(program)
        flate = s.deep_ref(LIT("FlateDecode")) if deep else LIT("FlateDecode")
        attrs = {"Filter": flate, "Length": len(packed), "Length1": len(program)}
        if key == "FontFile":
            spec = {"Type": LIT("Font"), "Subtype": LIT("Type1"), "BaseFont": LIT("ABCDEF+Foo")}
        else:
            spec = s.cid(ordering=b"Identity")
            spec.pop("ToUnicode", None)
        spec["FontDescriptor"] = {"FontBBox": [0, 0, 1000, 1000], key: PDFStream(attrs, packed)}
        yield spec, doc
    # fonts of encrypted documents: the security handler deciphers each
    # stream inside PDFStream.decode, under get_data and decode (AES goes
    # deeper than the filters), and in front of a failing filter
    for cipher in cipher_specs(rng):
        for kind in ("tounicode-simple", "tounicode-cid", "fontfile", "fontfile2", "fontfile-deep"):
            doc = FontDoc()
            s = Spec(rng, doc)
            if kind.startswith("tounicode"):
                spec = s.simple(LIT("TrueType")) if kind == "tounicode-simple" else s.cid()
                spec["ToUnicode"] = s.stream(s.tounicode_data(), compress=rng.random() < 0.5)
            else:
                program = tt_program(rng) if kind == "fontfile2" else type1_program(rng)
                packed = zlib.compress(program)
                flate = s.deep_ref(LIT("FlateDecode")) if kind == "fontfile-deep" else LIT("FlateDecode")
                attrs = {"Filter": flate, "Length": len(packed), "Length1": len(program)}
                if kind == "fontfile2":
                    spec = s.cid(ordering=b"Identity")
                    spec.pop("ToUnicode", None)
                else:
                    spec = {"Type": LIT("Font"), "Subtype": LIT("Type1"), "BaseFont": LIT("ABCDEF+Foo")}
                key = "FontFile2" if kind == "fontfile2" else "FontFile"
                spec["FontDescriptor"] = {"FontBBox": [0, 0, 1000, 1000], key: PDFStream(attrs, packed)}
            encrypt_case(rng, spec, doc, cipher)
            yield spec, doc


def clear_cmap_caches() -> None:
    """``CMapDB``'s caches emptied, as in a new Python process: the next
    ``get_cmap``/``get_unicode_map`` of a predefined name loads its file
    (the Rust replay calls ``CMapDB::clear_caches``)."""
    CMapDB._cmap_cache.clear()
    CMapDB._umap_cache.clear()


QUERIES = ("decode", "string_width", "to_unichr", "char_width", "char_disp", "get_descent", "get_width")


def query_call(font, op, arg):
    """One query, as ``font_queries`` makes it (``decode`` through the loop
    of ``decode_loop``)."""
    if op == "decode":
        return decode_loop(font, arg)
    r = getattr(font, op)(*([] if op.startswith("get_") else [arg]))
    return seqval(r)


def depth_cases(rng):
    for k, (spec, doc) in enumerate(depth_specs(rng)):
        if k % 20 == 0:
            print(f"depth: font {k}", file=sys.stderr, flush=True)
        table, errors = dump_doc(doc)
        yield from font_depth_cases(spec, table, errors, doc.cipher, QUERIES)


def font_depth_cases(spec, table, errors, cipher, queries):
    """The cases of the set ``depth`` for one dumped font: ``[spec, table,
    errors, op, arg, depth, depth + 1, cipher]`` and the outcomes at both
    depths, for its creation (``get_font``; ``get_font_cold``, the first
    loads of its predefined CMaps; ``get_font@parsed``, after a
    ``PSStackParser`` was created, as a document's parser is before any
    font) and for each of ``queries``."""
    spec_j, table_j, errors_j, cipher_j = roundtrip(spec, table, errors, cipher)
    clear_logger_caches()
    font = attempt(built_font, spec_j, table_j, errors_j, cipher_j)  # loads the predefined CMaps once

    def build(extra, parsed=False):
        clear_logger_caches()
        if parsed:
            # PSBaseParser.seek's log.debug fills the cache of the
            # pdfminer.psparser logger
            PSStackParser(io.BytesIO(b""))
        doc2 = load_doc(table_j, errors_j, cipher_j)
        spec2 = bind(spec_j, doc2)
        depth, out = nested(extra, PDFResourceManager().get_font, None, spec2)
        return depth, out if isinstance(out, Exception) else font_attrs(out)

    def case(op, arg, found):
        (d0, out0), (d1, out1) = found
        assert is_recursion(out1) and not is_recursion(out0), (op, out0, out1)
        return [spec, table, errors, op, arg, d0, d1, cipher], [canon_or_hash(out0), canon_or_hash(out1)]

    found = threshold(build)
    if found is not None:
        yield case("get_font", None, found)

    # the same after a generic parser was created (only a font with a
    # stream can create a parser itself)
    if streams_of(spec_j, load_doc(table_j, errors_j)):
        found = threshold(lambda extra: build(extra, parsed=True))
        if found is not None:
            yield case("get_font@parsed", None, found)

    # the same where the font loads its predefined CMaps (first loads)
    clear_cmap_caches()
    clear_logger_caches()
    attempt(built_font, spec_j, table_j, errors_j, cipher_j)
    if CMapDB._cmap_cache or CMapDB._umap_cache:

        def build_cold(extra):
            clear_cmap_caches()
            return build(extra)

        found = threshold(build_cold)
        if found is not None:
            yield case("get_font_cold", None, found)
    if isinstance(font, Exception):
        return
    cids = attempt(decode_loop, font, b"\x00A\x00B")
    cid = cids[0] if isinstance(cids, list) and cids else 65
    for op in queries:
        arg = {"decode": b"\x00A\x00B", "string_width": b"\x00A\x00B"}.get(op, cid)

        def query(extra, op=op, arg=arg):
            clear_logger_caches()
            return nested(extra, query_call, font, op, arg)

        found = threshold(query)
        if found is not None:
            yield case(op, arg, found)

        # the same query right after the font's creation, the logger
        # caches as the creation left them (as the interpreter queries
        # a font it just created: "<op>@build")
        def query_built(extra, op=op, arg=arg):
            clear_logger_caches()
            fresh = built_font(spec_j, table_j, errors_j, cipher_j)
            return nested(extra, query_call, fresh, op, arg)

        found = threshold(query_built)
        if found is not None:
            yield case(op + "@build", arg, found)


# -- real corpus ------------------------------------------------------------------

MAX_PAGES = 20
#: Strings kept per font (the first ones shown).
MAX_STRINGS = 2000
FILTER_KEYS = ("F", "Filter", "DP", "DecodeParms", "FDecodeParms")


class Timeout(Exception):
    pass


def on_alarm(signum, frame):
    raise Timeout()


class RecordingManager(PDFResourceManager):
    """The resource manager of the interpreter, recording the fonts it
    creates for the pages and Form XObjects (``get_font`` calls of the
    interpreter, not the recursive ones of ``Type0`` fonts)."""

    def __init__(self) -> None:
        super().__init__(caching=True)
        self.level = 0
        self.fonts: list[tuple[dict, Any]] = []
        self.seen: set[int] = set()

    def get_font(self, objid, spec):
        self.level += 1
        try:
            font = super().get_font(objid, spec)
        except Exception as e:
            if self.level == 1:
                self.fonts.append((spec, e))
            raise
        finally:
            self.level -= 1
        if self.level == 0 and id(font) not in self.seen:
            self.seen.add(id(font))
            self.fonts.append((spec, font))
        return font


class RecordingDevice(PDFDevice):
    """Records the strings shown with each font."""

    def __init__(self, rsrcmgr) -> None:
        super().__init__(rsrcmgr)
        self.strings: dict[int, dict[bytes, None]] = collections.defaultdict(dict)

    def render_string(self, textstate, seq, ncs, graphicstate) -> None:
        font = textstate.font
        if font is None:
            return
        shown = self.strings[id(font)]
        for obj in seq:
            if isinstance(obj, bytes) and len(shown) < MAX_STRINGS:
                shown[obj] = None


class DecodedStream(Exception):
    """A stream of an encrypted case that was already decoded: its raw
    data, which the case's handler would decipher, is gone."""


def dump_stream(s: PDFStream, encrypted: bool = False) -> PDFStream:
    """A stream as the case carries it: its raw data when it was not decoded
    yet (deciphered, unless the case is ``encrypted``: then the case's
    handler deciphers it when the font decodes it); else its decoded data,
    its filters dropped."""
    if s.rawdata is not None:
        raw = s.rawdata
        if s.decipher and not encrypted:
            raw = s.decipher(s.objid, s.genno, raw, s.attrs)
        out = PDFStream(dict(s.attrs), raw)
    elif encrypted:
        raise DecodedStream(s.objid)
    else:
        out = PDFStream({k: v for k, v in s.attrs.items() if k not in FILTER_KEYS}, s.data)
    out.objid, out.genno = s.objid, s.genno
    return out


def safe_args(e: BaseException) -> tuple:
    """The arguments of an exception, when typed JSON can carry them (else
    its message: the class and, mostly, ``str(e)`` are what the font code
    sees)."""
    if all(a is None or type(a) in (str, bytes, int, float, bool) for a in e.args):
        return tuple(e.args)
    return (str(e),)


def copy_graph(spec, real_doc, doc: FontDoc):
    """A copy of ``spec`` and, in ``doc``, of every object it reaches through
    references (or the error of ``getobj``), streams through ``dump_stream``
    (still encrypted when ``doc`` has a cipher)."""
    memo: dict[int, Any] = {}

    def copy(x):
        if isinstance(x, PDFObjRef):
            visit(x.objid)
            return PDFObjRef(doc, x.objid)
        if isinstance(x, (list, dict, PDFStream)):
            if id(x) in memo:
                return memo[id(x)]
            if isinstance(x, list):
                out = memo[id(x)] = []
                out.extend(copy(v) for v in x)
            elif isinstance(x, dict):
                out = memo[id(x)] = {}
                for k, v in x.items():
                    out[k] = copy(v)
            else:
                out = memo[id(x)] = dump_stream(x, doc.cipher is not None)
                out.attrs = {k: copy(v) for k, v in out.attrs.items()}
            return out
        return x

    pending: list[Any] = []

    def visit(objid):
        if objid in doc.table or objid in doc.errors or objid in queued:
            return
        queued.add(objid)
        pending.append(objid)

    queued: set[Any] = set()
    out = copy(spec)
    while pending:
        objid = pending.pop()
        try:
            obj = real_doc.getobj(objid)
        except pdfexceptions.PDFObjectNotFound:
            continue
        except Exception as e:
            doc.errors[objid] = (type(e), safe_args(e))
            continue
        doc.table[objid] = copy(obj)
    return out


def streams_of(spec, doc: FontDoc):
    """Every stream object of a decoded case (spec and table)."""
    found = []
    stack = [spec, *doc.table.values()]
    seen = set()
    while stack:
        x = stack.pop()
        if id(x) in seen:
            continue
        seen.add(id(x))
        if isinstance(x, list):
            stack.extend(x)
        elif isinstance(x, dict):
            stack.extend(x.values())
        elif isinstance(x, PDFStream):
            found.append(x)
            stack.extend(x.attrs.values())
    return found


def prune(spec, doc: FontDoc, read: set) -> None:
    """Empties the streams of the case whose position (in ``streams_of``
    order) is not in ``read``."""
    for k, s in enumerate(streams_of(spec, doc)):
        if k not in read:
            for key in FILTER_KEYS:
                s.attrs.pop(key, None)
            s.rawdata = b""
            s.data = None


def corpus_font_case(spec, real_doc, original, strings, cipher=None):
    """The case of one font of a document, pruned when the outcome allows;
    and whether the dump reproduces pdfminer's font. With ``cipher`` (an
    encrypted document, ``real_doc`` a fresh instance of it whose streams
    were not decoded yet), the streams stay encrypted."""
    doc = FontDoc()
    doc.cipher = cipher
    spec_copy = copy_graph(spec, real_doc, doc)
    strings = list(strings)
    expected = canon_or_hash(
        original if isinstance(original, Exception) else [font_attrs(original), font_queries(original, strings, PROBES)]
    )
    table, errors = dump_doc(doc)
    spec_j, table_j, errors_j, cipher_j = roundtrip(spec_copy, table, errors, cipher)
    full = run_font(spec_j, table_j, errors_j, strings, PROBES, cipher_j)
    # the streams the font reads (decoded by its creation)
    probe_doc = load_doc(table_j, errors_j, cipher_j)
    probe_spec = bind(spec_j, probe_doc)
    attempt(PDFResourceManager().get_font, None, probe_spec)
    read = {k for k, s in enumerate(streams_of(probe_spec, probe_doc)) if s.data is not None and s.rawdata is None}
    pruned_doc = load_doc(table_j, errors_j, cipher_j)
    pruned_spec = bind(spec_j, pruned_doc)
    prune(pruned_spec, pruned_doc, read)
    table_p, errors_p = dump_doc(pruned_doc)
    spec_pj, table_pj, errors_pj = roundtrip(pruned_spec, table_p, errors_p)
    pruned = run_font(spec_pj, table_pj, errors_pj, strings, PROBES, cipher_j)
    if pruned[2] == full[2]:
        args, (depth, qdepth, out) = [pruned_spec, table_p, errors_p], pruned
    else:
        args, (depth, qdepth, out) = [spec_copy, table, errors], full
    return [*args, strings, list(PROBES), depth, qdepth, cipher], out, out == expected


def plain_value(x) -> bool:
    """Whether typed JSON carries ``x`` with no document (no reference, no
    stream)."""
    if isinstance(x, (PDFObjRef, PDFStream)):
        return False
    if isinstance(x, (list, tuple)):
        return all(plain_value(v) for v in x)
    if isinstance(x, dict):
        return all(plain_value(v) for v in x.values())
    return True


def document_cipher(doc, password) -> list | None:
    """``[docid, param, password]`` of an encrypted document whose
    encryption dictionary typed JSON carries as it is, else ``None``."""
    if doc.encryption is None:
        return None
    docid, param = doc.encryption
    if not plain_value([docid, param]):
        return None
    return [list(docid), dict(param), password or ""]


#: Fonts per encrypted document whose recursion boundary is searched
#: (``corpus_depth``).
DEPTH_FONTS_PER_FILE = 3


def program_cases(spec, real_doc):
    """The embedded programs of a font (and of its descendant): ``(kind,
    data)`` with ``kind`` ``type1`` (``FontFile`` up to ``Length1``),
    ``truetype`` (``FontFile2``) or ``cff`` (``FontFile3``)."""
    from pdfminer.pdftypes import dict_value, int_value, list_value, resolve1

    specs = [spec]
    with contextlib.suppress(Exception):
        specs += [dict_value(d) for d in list_value(spec.get("DescendantFonts"))[:1]]
    out = []
    for s in specs:
        try:
            descriptor = dict_value(s.get("FontDescriptor"))
        except Exception:
            continue
        for key, kind in (("FontFile", "type1"), ("FontFile2", "truetype"), ("FontFile3", "cff")):
            try:
                strm = resolve1(descriptor.get(key))
                if not isinstance(strm, PDFStream):
                    continue
                data = strm.get_data()
                if kind == "type1":
                    data = data[: int_value(strm.get("Length1"))]
            except Exception:
                continue
            out.append((kind, data))
    return out


def collect_file(path, password):
    """``(font cases, program cases, depth cases, stats)`` of one document.
    The fonts of an encrypted document are copied from a second instance of
    it, whose streams were not decoded yet: they stay encrypted, with the
    document's handler in the case (``document_cipher``), and the boundary
    of the first ones that read a stream is searched (``corpus_depth``)."""
    fonts, programs, depths, stats = [], [], [], collections.Counter()
    with open_document(path, password) as doc, open_document(path, password) as fresh:
        cipher = document_cipher(doc, password)
        if doc.encryption is not None:
            stats["encrypted_files"] += 1
            stats["encrypted_files_cipher"] += cipher is not None
        rsrcmgr = RecordingManager()
        device = RecordingDevice(rsrcmgr)
        interpreter = PDFPageInterpreter(rsrcmgr, device)
        for i, page in enumerate(PDFPage.create_pages(doc)):
            if i >= MAX_PAGES:
                break
            try:
                interpreter.process_page(page)
            except Exception:
                stats["page_errors"] += 1
        for spec, font in rsrcmgr.fonts:
            strings = device.strings.get(id(font), {}) if not isinstance(font, Exception) else {}
            try:
                if cipher is not None:
                    case, out, same = corpus_font_case(spec, fresh, font, strings, cipher)
                    stats["encrypted_fonts"] += 1
                else:
                    case, out, same = corpus_font_case(spec, doc, font, strings)
            except Timeout:
                raise
            except Exception as e:  # a harness limitation, counted
                stats[f"skipped:{type(e).__name__}"] += 1
                continue
            stats["fonts"] += 1
            stats["dump_differs"] += not same
            fonts.append((case, out))
            spec_c, table_c, errors_c = case[:3]
            if cipher is not None and len(depths) < DEPTH_FONTS_PER_FILE and streams_of(spec_c, load_doc(table_c, errors_c)):
                depths.append(case)
        for spec, _ in rsrcmgr.fonts:
            programs += program_cases(spec, doc)
    depth_out = []
    for case in depths:
        for args, out in font_depth_cases(case[0], case[1], case[2], case[7], ()):
            if args[3] in ("get_font", "get_font@parsed"):
                depth_out.append((args, out))
    return fonts, programs, depth_out, stats


def process_file(args):
    path, rel, password = args
    signal.signal(signal.SIGALRM, on_alarm)
    signal.alarm(FILE_TIMEOUT_S)
    try:
        fonts, programs, depths, stats = collect_file(path, password)
        with deep_recursion():
            fonts = [(json.dumps(typed_encode(case)), out) for case, out in fonts]
            depths = [(json.dumps(typed_encode(case)), out) for case, out in depths]
        return rel, None, fonts, programs, depths, stats
    except Timeout:
        return rel, "timeout", [], [], [], collections.Counter()
    except Exception as e:  # unreadable document: no font case
        return rel, type(e).__name__, [], [], [], collections.Counter()
    finally:
        signal.alarm(0)


def program_case(kind, data):
    """``[kind, data, depth]`` and the outcome of the case of that kind."""
    run = {"type1": type1_case, "truetype": tt_case, "cff": cff_case}[kind]
    args, out = run(data)
    return [kind, *args], out


def corpus_sets(jobs):
    files = load_set("all")
    tasks = [(str(f.path), f.rel, f.password) for f in files]
    fonts: dict[str, tuple[str, str]] = {}
    depths: dict[str, tuple[str, str]] = {}
    programs: dict[str, tuple[str, bytes]] = {}
    status, stats = collections.Counter(), collections.Counter()
    with multiprocessing.Pool(jobs) as pool:
        for rel, err, file_fonts, file_programs, file_depths, file_stats in pool.imap(process_file, tasks, chunksize=2):
            if err is not None:
                status[err] += 1
            stats.update(file_stats)
            for text, out in file_fonts:
                fonts.setdefault(hashlib.sha256(text.encode()).hexdigest(), (text, out))
            for text, out in file_depths:
                depths.setdefault(hashlib.sha256(text.encode()).hexdigest(), (text, out))
            for kind, data in file_programs:
                programs.setdefault(hashlib.sha256(kind.encode() + data).hexdigest(), (kind, data))
    print(f"{len(fonts)} fonts, {len(depths)} depth cases, {len(programs)} programs from {len(files)} files; "
          f"{dict(status)}; {dict(stats)}", file=sys.stderr)
    for name, cases in (("corpus_fonts", fonts), ("corpus_depth", depths)):
        path = unit_dir(MODULE) / f"{name}.jsonl"
        tmp = path.with_name(f".{path.name}.tmp")
        with open(tmp, "w", encoding="ascii") as f:
            for sha in sorted(cases):
                text, out = cases[sha]
                f.write('{"in":' + text + ',"out":' + json.dumps(typed_encode(out)) + "}\n")
        tmp.replace(path)
    n_programs = write_set("corpus_programs", (program_case(*programs[sha]) for sha in sorted(programs)))
    return {"corpus_fonts": len(fonts), "corpus_depth": len(depths), "corpus_programs": n_programs,
            "dump_differs": stats["dump_differs"], "files_failed": dict(status), "stats": dict(stats)}


# -- main -----------------------------------------------------------------------


def write_marker(name: str, count: int, **extra) -> None:
    """``<name>.json``: the sources that wrote the set (checked by the Rust
    replay)."""
    with open(unit_dir(MODULE) / f"{name}.json", "w", encoding="ascii") as f:
        json.dump({"sources": oracle_sources(), "cases": count, **extra}, f)


GENERATED = {
    "widths": widths_cases,
    "getdict": getdict_cases,
    "cff": cff_cases,
    "truetype": truetype_cases,
    "type1": type1_cases,
    "fonts": font_spec_cases,
}


def write_set(name: str, cases) -> int:
    """Runs the cases (at the normal recursion limit), then writes them."""
    started = time.monotonic()
    cases = list(cases)
    with deep_recursion():
        n = write_cases(MODULE, name, cases)
    print(f"{name}: {n} cases, {time.monotonic() - started:.0f} s", file=sys.stderr)
    return n


def dump(args) -> None:
    """Prints the full canonical output of case INDEX of a generated set."""
    name, index = args.dump[0], int(args.dump[1])
    rng = random.Random(f"{args.seed}:{name}")
    capture = {}

    def keep(v):
        capture["text"] = v
        return "x"

    global canon_or_hash
    original = canon_or_hash
    canon_or_hash = lambda v: keep(canon(v))  # noqa: E731
    try:
        for k, _ in enumerate(GENERATED[name](rng)):
            if k == index:
                print(capture["text"])
                return
    finally:
        canon_or_hash = original
    raise SystemExit(f"--dump {name}: no case {index}")


def main() -> None:
    ap = argparse.ArgumentParser(prog="python -m oracle.unit.fonts")
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--corpus", action="store_true", help="the fonts of the corpus set all")
    ap.add_argument("-j", "--jobs", type=int, default=3, help="worker processes for --corpus")
    ap.add_argument("--only", nargs="*", help="generated sets to write (default: all)")
    ap.add_argument("--dump", nargs=2, metavar=("NAME", "INDEX"))
    args = ap.parse_args()
    require_oracle_python()
    cmap_dir = codegen_cmaps.ensure()
    rel_cmap_dir = cmap_dir.relative_to(cache_dir()).as_posix()

    if args.dump:
        dump(args)
        return

    if args.corpus:
        counts = corpus_sets(args.jobs)
        write_marker("corpus", sum(counts[k] for k in ("corpus_fonts", "corpus_depth", "corpus_programs")), **counts)
        write_meta(MODULE, cmap_dir=rel_cmap_dir)
        print(counts)
        return

    counts = {}
    for name, gen in GENERATED.items():
        if args.only and name not in args.only:
            continue
        counts[name] = write_set(name, gen(random.Random(f"{args.seed}:{name}")))
    if not args.only or "depth" in args.only:
        counts["depth"] = write_set("depth", depth_cases(random.Random(f"{args.seed}:depth")))
    write_marker("generated", sum(counts.values()), **counts)
    write_meta(MODULE, cmap_dir=rel_cmap_dir)
    print(counts)


if __name__ == "__main__":
    main()
