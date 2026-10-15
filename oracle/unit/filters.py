"""Differential cases for the stream filters: crates/pdfminer/src/{lzw,
ascii85,runlength}.rs, the predictors of utils.rs, and
``PDFStream.decode``/``get_data``/``get_filters`` and
``decompress_corrupted`` of pdftypes.rs, computed by pdfminer.

Outputs in ``<cache>/unit/filters/<fingerprint>/`` (``oracle/unit/_common.py``);
the Rust replay is crates/pdftrace/tests/filters_unit.rs. A result of more
than ``BLOB_MAX`` bytes is written ``("sha256", len, hexdigest)``.

Generated cases (typed JSON lines, seed fixed), ``{"in": [...], "out": ...}``:

- ``lzw``: ``lzwdecode(data)``: code sequences packed with the decoder's own
  code widths (clear codes, early change at 511/1023/2047 entries, codes
  equal to the table length, past it, before any clear code, the EOD code
  in the middle, tables past 4096 entries), random bytes, truncations.
- ``ascii85``: ``ascii85decode(data)``: ``a85encode`` output framed by
  ``<~``/``~``/``~>`` with whitespace (``\\f`` and ``\\v`` too, which
  ``\\s`` strips but ``a85decode`` rejects), ``z`` in and out of groups,
  overflowing groups, final groups of 1 to 4 characters, characters out of
  the alphabet (non-ASCII included).
- ``asciihex``: ``asciihexdecode(data)``: digits of both cases, whitespace,
  ``>`` after an odd or even number of digits, invalid characters.
- ``runlength``: ``rldecode(data)``: literal and repeat runs, EOD, data
  running out inside a literal run (``RuntimeError``) or a repeat run
  (``StopIteration``).
- ``paeth``: ``paeth_predictor(a, b, c)``.
- ``tiff``, ``png``: ``apply_tiff_predictor(colors, columns, bpc, data)``,
  ``apply_png_predictor(pred, colors, columns, bpc, data)``: every PNG
  filter type and invalid ones, 1 to 4 colors, 1 and 8 bits (and 2, 16,
  ``True``, ``False``), short last rows, ``Columns`` smaller than a row
  (``Colors`` > 1: the first row's ``Average``/``Paeth`` read past
  ``line_above``), zero, negative and huge parameters.
- ``corrupted``: ``decompress_corrupted(data)`` on truncated or damaged
  zlib streams (an error before the last 4 bytes raises, after returns
  what was decoded).
- ``streams``: ``PDFStream(attrs, raw)``: ``[get_filters(), get_data(),
  get_rawdata()]`` for chains of every filter (names and abbreviations,
  lists, parameters as dict, list, stream, of the wrong length or type),
  predictors through ``DecodeParms`` (values int, bool, float, huge),
  pass-through and unsupported filters, data encoded for the chain then
  sometimes damaged; any of the filter entry, its elements, the parameters
  entry, its elements and the predictor values (``Predictor``, ``Colors``,
  ``Columns``, ``BitsPerComponent``, ``K``) may be an indirect reference
  (``refify``): to an object of the case's table, through a chain of
  references, to a missing object (``None``) or to a broken one
  (``PDFSyntaxError``). ``in``: ``[attrs, raw, table]``, the references
  (``{"ref": n}``) resolving through the table, ``[[objid, object], ...]``
  (``oracle/unit/pdftypes.py``'s ``TableDoc``: object ``BROKEN_OBJID``
  raises ``PDFSyntaxError``, another missing one ``PDFObjectNotFound``).

Real corpus (``corpus.bin`` + ``corpus.json``): every stream object of the
PDFs of the set ``all`` (``oracle/corpus.py``), as ``getobj`` returns it:
its filter entries (``F``, ``Filter``, ``DP``, ``DecodeParms``,
``FDecodeParms``) with indirect references resolved (copies; a stream as
parameters keeps its attributes and no data), and its raw data, deciphered
with the document's own ``decipher`` for encrypted documents (decryption is
not what is tested here). Python's ``get_data()`` of a new
``PDFStream(filter entries, raw data)`` is recorded. Streams already
decoded by pdfminer (object streams), pass-through-only streams above 4 KB
and raw data above ``MAX_RAW`` are skipped, a file stops after
``MAX_FILE_RAW`` bytes of raw data; identical cases are kept once.
``corpus.bin``: little-endian ``b"FLT1" u32 n``, then per case ``u32 len``
+ typed JSON of ``attrs`` and ``u32 len`` + raw bytes. ``corpus.json``:
``{"sources", "records": [{"id", "out"}]}``, ``out`` typed JSON of the result
(always ``("sha256", len, hex)`` for bytes, or the exception).

Usage: PDFPLUMBER_ROOT=<main checkout> <root>/.venv/bin/python -m oracle.unit.filters [--no-corpus] [-j 3]
"""

from __future__ import annotations

import argparse
import base64
import collections
import hashlib
import json
import multiprocessing
import random
import signal
import struct
import sys
import zlib

from pdfminer.ascii85 import ascii85decode, asciihexdecode
from pdfminer.lzw import lzwdecode
from pdfminer.pdftypes import PDFObjRef, PDFStream, decompress_corrupted
from pdfminer.psparser import LIT, PSKeyword, PSLiteral
from pdfminer.runlength import rldecode
from pdfminer.utils import apply_png_predictor, apply_tiff_predictor, paeth_predictor

from ..corpus import load_set
from ..stages._common import open_document
from ..stages.objects import fetch_order
from ..typed_json import dumps
from ._common import oracle_sources, require_oracle_python, unit_dir, write_cases, write_meta
from .pdftypes import BROKEN_OBJID, TableDoc

BLOB_MAX = 2048
MAX_RAW = 4 * 1024 * 1024
MAX_FILE_RAW = 48 * 1024 * 1024
FILE_TIMEOUT_S = 120


def blob(b: bytes):
    if len(b) <= BLOB_MAX:
        return b
    return ("sha256", len(b), hashlib.sha256(b).hexdigest())


def call(f, *args):
    try:
        return blob(f(*args))
    except Exception as e:  # noqa: BLE001 - the exception is the result
        return e


# -- encoders --------------------------------------------------------------------


def lzw_pack(codes):
    """Packs ``codes`` MSB first with the widths ``LZWDecoder`` reads them
    with (tracking its table length as it would)."""
    acc, nacc, out = 0, 0, bytearray()
    table_len, prev = None, None  # None: no clear code yet
    nbits = 9
    for code in codes:
        acc = (acc << nbits) | (code & ((1 << nbits) - 1))
        nacc += nbits
        while nacc >= 8:
            out.append((acc >> (nacc - 8)) & 0xFF)
            nacc -= 8
        acc &= (1 << nacc) - 1
        # the decoder's table after this code
        if code == 256:
            table_len, prev, nbits = 258, False, 9
        elif code == 257 or table_len is None:
            pass
        elif not prev:
            prev = True
        elif code <= table_len:
            table_len += 1
            nbits = {511: 10, 1023: 11, 2047: 12}.get(table_len, nbits)
    if nacc:
        out.append((acc << (8 - nacc)) & 0xFF)
    return bytes(out)


def lzw_encode(data, early=True):
    """A standard LZW encoder (clear code first, EOD last)."""
    table = {bytes([i]): i for i in range(256)}
    codes = [256]
    w = b""
    for c in data:
        wc = w + bytes([c])
        if wc in table:
            w = wc
            continue
        codes.append(table[w])
        if len(table) + 2 < 4094:
            table[wc] = len(table) + 2
        else:
            codes.append(256)
            table = {bytes([i]): i for i in range(256)}
        w = bytes([c])
    if w:
        codes.append(table[w])
    codes.append(257)
    return codes


def rand_lzw_codes(rng):
    codes = [] if rng.random() < 0.05 else [256]
    table_len = 258
    first = True
    for _ in range(rng.choice([1, 5, 50, 600, 3000, 5000])):
        r = rng.random()
        if r < 0.01:
            codes.append(256)
            table_len, first = 258, True
            continue
        if r < 0.015:
            codes.append(257)
            continue
        hi = 255 if first else table_len
        if r < 0.03:
            code = rng.randint(hi, min(hi + 3, 4095))  # equal to or past the table length
        else:
            code = rng.randint(0, min(hi, 4095))
            if code in (256, 257):
                code = 65
        codes.append(code)
        if first:
            first = False
        elif code <= table_len:
            table_len += 1
    return codes


def rl_encode(rng, data):
    out = bytearray()
    i = 0
    while i < len(data):
        run = 1
        while i + run < len(data) and data[i + run] == data[i] and run < 128:
            run += 1
        if run >= 2 and rng.random() < 0.9:
            out += bytes([257 - run, data[i]])
            i += run
        else:
            n = min(rng.randint(1, 128), len(data) - i)
            out += bytes([n - 1]) + data[i : i + n]
            i += n
    if rng.random() < 0.7:
        out.append(128)
    return bytes(out)


def png_encode(rng, raw_rows, bpp):
    """Rows filtered with random PNG filter types (the encoder of the spec)."""
    out = bytearray()
    prior = None
    for row in raw_rows:
        ft = rng.randrange(5)
        enc = bytearray()
        for j, x in enumerate(row):
            a = row[j - bpp] if j >= bpp else 0
            b = prior[j] if prior is not None and j < len(prior) else 0
            c = prior[j - bpp] if prior is not None and j >= bpp and j - bpp < len(prior) else 0
            pred = [0, a, b, (a + b) // 2, paeth_predictor(a, b, c)][ft]
            enc.append((x - pred) & 255)
        out += bytes([ft]) + enc
        prior = row
    return bytes(out)


def sample(rng, n):
    kind = rng.randrange(3)
    if kind == 0:
        return rng.randbytes(n)
    if kind == 1:
        return bytes(rng.choice(b"\x00\x00\x00\xff\x10\x11") for _ in range(n))
    return bytes((i * rng.randint(1, 5)) & 0xFF for i in range(n))


# -- generated cases ---------------------------------------------------------------


def gen_lzw(rng):
    for _ in range(1500):
        codes = rand_lzw_codes(rng)
        data = lzw_pack(codes)
        yield [data], call(lzwdecode, data)
        if rng.random() < 0.1 and data:
            k = rng.randrange(len(data))
            yield [data[:k]], call(lzwdecode, data[:k])
    for _ in range(400):
        src = sample(rng, rng.choice([0, 1, 10, 300, 3000, 20000]))
        codes = lzw_encode(src)
        if rng.random() < 0.3:
            codes = codes[1:]  # no clear code first
        data = lzw_pack(codes)
        yield [data], call(lzwdecode, data)
    for _ in range(300):
        data = rng.randbytes(rng.choice([0, 1, 2, 3, 50, 1000]))
        yield [data], call(lzwdecode, data)


A85_EXTRA = [b" ", b"\n", b"\r\n", b"\t", b"\x0b", b"\x0c", b"z", b"y", b"~", b"~>", b"<~", b"v", b"{", b"\x00", b"\xff",
             b"s8W-\"", b"uuuuu", b"s8W-!", b"!", b"!!", b"!!!", b"!!!!", b">", b"<", b"\x80"]


def gen_ascii85(rng):
    for _ in range(3000):
        raw = sample(rng, rng.choice([0, 1, 2, 3, 4, 5, 8, 50, 400]))
        enc = bytearray(base64.a85encode(raw, foldspaces=rng.random() < 0.1, wrapcol=rng.choice([0, 0, 10, 75])))
        for _ in range(rng.choice([0, 0, 1, 2, 4])):
            k = rng.randint(0, len(enc))
            enc[k:k] = rng.choice(A85_EXTRA)
        pre = rng.choice([b"", b"", b"<~", b"~", b" <~", b"< ~", b" ~ ", b"<\n~\n", b"<<~", b"~~", b"\x0c~"])
        post = rng.choice([b"", b"", b"~>", b"~", b"~> ", b"~ >", b"~>\n", b"~>x", b"~\n>\n", b"~~>", b" ~> \x0b", b">"])
        data = pre + bytes(enc) + post
        if rng.random() < 0.1 and data:
            data = data[: rng.randrange(len(data))]
        yield [data], call(ascii85decode, data)


AHX_EXTRA = [b" ", b"\n", b"\x0c", b"\x0b", b">", b"g", b"G", b"x", b"\x00", b"\xff", b"0", b"Ab"]


def gen_asciihex(rng):
    for _ in range(2000):
        raw = sample(rng, rng.choice([0, 1, 2, 5, 50]))
        enc = bytearray(raw.hex().encode() if rng.random() < 0.5 else raw.hex().upper().encode())
        for _ in range(rng.choice([0, 0, 1, 2, 3])):
            k = rng.randint(0, len(enc))
            enc[k:k] = rng.choice(AHX_EXTRA)
        if rng.random() < 0.3 and enc:
            del enc[rng.randrange(len(enc))]
        data = bytes(enc) + rng.choice([b"", b">", b" >", b">>", b"> 12", b"\n"])
        yield [data], call(asciihexdecode, data)


def gen_runlength(rng):
    for _ in range(2000):
        raw = sample(rng, rng.choice([0, 1, 2, 5, 130, 300, 1000]))
        data = rl_encode(rng, raw)
        r = rng.random()
        if r < 0.2 and data:
            data = data[: rng.randrange(len(data))]
        elif r < 0.25:
            data = rng.randbytes(rng.choice([1, 2, 10, 100]))
        yield [data], call(rldecode, data)
    for data in [b"", b"\x80", b"\x00", b"\x01a", b"\x81", b"\xff", b"\x7f" + b"a" * 127, b"\x7f" + b"a" * 128, b"\x81a\x80zz"]:
        yield [data], call(rldecode, data)


def gen_paeth(rng):
    for _ in range(2000):
        a, b, c = rng.randrange(256), rng.randrange(256), rng.randrange(256)
        yield [a, b, c], paeth_predictor(a, b, c)
    for a in range(0, 256, 51):
        for b in range(0, 256, 51):
            for c in range(0, 256, 51):
                yield [a, b, c], paeth_predictor(a, b, c)


ODD_INTS = [0, -1, -3, 2**70, -(2**70), 2**63, 2**63 - 1, -(2**63), True, False]


def rand_param(rng, usual):
    return rng.choice(ODD_INTS) if rng.random() < 0.05 else rng.choice(usual)


def gen_tiff(rng):
    for _ in range(2500):
        colors = rand_param(rng, [1, 1, 2, 3, 4])
        columns = rand_param(rng, [1, 2, 3, 5, 8, 17])
        bpc = rng.choice([8] * 12 + [1, 16, True, 0, 2**70])
        nbytes = colors * columns if isinstance(colors, int) and isinstance(columns, int) else 1
        rows = rng.choice([0, 1, 2, 5])
        n = max(0, min(abs(nbytes), 300)) * rows + rng.choice([0, 0, 0, 1, -1, 3])
        data = sample(rng, max(n, 0))
        yield [colors, columns, bpc, data], call(apply_tiff_predictor, colors, columns, bpc, data)


def gen_png(rng):
    for _ in range(5000):
        colors = rand_param(rng, [1, 1, 2, 3, 4])
        columns = rand_param(rng, [1, 2, 3, 5, 8, 17, 100])
        bpc = rng.choice([8] * 6 + [1] * 4 + [2, 16, True, False, 2**70])
        pred = rng.choice([10, 11, 12, 13, 14, 15, 2**70, True])
        try:
            nbytes = max(0, min(colors * columns * bpc // 8, 400))
            bpp = colors * bpc // 8
        except TypeError:
            nbytes, bpp = 3, 1
        rows = [sample(rng, nbytes) for _ in range(rng.choice([0, 1, 2, 4]))]
        if rng.random() < 0.7 and isinstance(bpp, int) and 0 < bpp < 10:
            data = png_encode(rng, rows, bpp)
        else:
            data = b"".join(bytes([rng.choice([0, 1, 2, 3, 4, 4, 5, 255])]) + r for r in rows)
        if rng.random() < 0.2 and data:
            data = data[: rng.randrange(len(data))]
        elif rng.random() < 0.1:
            data += rng.randbytes(rng.randint(1, 5))
        yield [pred, colors, columns, bpc, data], call(apply_png_predictor, pred, colors, columns, bpc, data)
    # a /Columns too large for `list(b"\x00" * columns)` on the oracle
    # machine (MemoryError; MEMORY_LIMIT in utils.rs): 2**32 builds the 4 GiB
    # bytes object (about a second), then the 32 GiB list is refused; from
    # 2**35 on the bytes object is refused at once
    for columns in [2**32, 2**32 + 1, 2**35, 2**36, 2**40, 2**46, 2**47, 2**62]:
        for colors, bpc, data in [(1, 8, b"\x00\x01\x02"), (1, 1, b"\x01\x05"), (3, 8, b""), (-1, 8, b"\x02\x07"),
                                  (1, 2, b"\x00\x01")]:
            yield [10, colors, columns, bpc, data], call(apply_png_predictor, 10, colors, columns, bpc, data)


def gen_corrupted(rng):
    for _ in range(1500):
        raw = sample(rng, rng.choice([0, 1, 10, 100, 1000, 5000]))
        z = zlib.compress(raw, rng.choice([0, 1, 6, 9]))
        r = rng.random()
        if r < 0.3:
            z = z[: rng.randrange(len(z) + 1)]
        elif r < 0.6:
            b = bytearray(z)
            b[rng.randrange(len(b))] ^= 1 << rng.randrange(8)
            z = bytes(b)
        elif r < 0.8:
            b = bytearray(z)  # damage in the Adler-32 trailer
            b[-rng.randint(1, 4)] ^= 0xFF
            z = bytes(b)
        else:
            z += rng.randbytes(rng.randint(1, 6))
        yield [z], call(decompress_corrupted, z)
    for z in [b"", b"x", b"x\x9c", b"x\x9c\x03", b"x\x9c\x03\x00", b"\x00\x00\x00\x00", b"\x00\x00\x00\x00\x00"]:
        yield [z], call(decompress_corrupted, z)


# -- generated streams ---------------------------------------------------------------

FILTER_NAMES = {
    "flate": [LIT("FlateDecode"), LIT("Fl")],
    "lzw": [LIT("LZWDecode"), LIT("LZW")],
    "a85": [LIT("ASCII85Decode"), LIT("A85")],
    "ahx": [LIT("ASCIIHexDecode"), LIT("AHx")],
    "rl": [LIT("RunLengthDecode"), LIT("RL")],
    "dct": [LIT("DCTDecode"), LIT("DCT")],
    "jpx": [LIT("JPXDecode")],
    "jbig2": [LIT("JBIG2Decode")],
}


def encode(rng, kind, data):
    if kind == "flate":
        return zlib.compress(data, rng.choice([0, 6, 9]))
    if kind == "lzw":
        return lzw_pack(lzw_encode(data))
    if kind == "a85":
        return base64.a85encode(data, adobe=rng.random() < 0.5)
    if kind == "ahx":
        return data.hex().encode() + rng.choice([b">", b""])
    if kind == "rl":
        return rl_encode(rng, data)
    return data


def rand_params(rng, kind, rows_info):
    """DecodeParms for one filter: often a predictor."""
    r = rng.random()
    if r < 0.4:
        return rng.choice([None, {}, {"K": -1}])
    colors, columns, bpc = rows_info
    p = {"Predictor": rng.choice([1, 2, 10, 11, 12, 13, 14, 15, 15, 3, 0, True, 1.5, 2**70, -1])}
    if rng.random() < 0.8:
        p["Colors"] = colors
    if rng.random() < 0.8:
        p["Columns"] = columns
    if rng.random() < 0.7:
        p["BitsPerComponent"] = bpc
    if rng.random() < 0.05:
        p["Colors"] = rng.choice([1.5, b"3", LIT("X"), None])
    return p


#: a missing object (resolves to None)
MISSING_OBJID = 999
PARAM_KEYS = ("Predictor", "Colors", "Columns", "BitsPerComponent", "K")


def refify(rng, attrs, doc):
    """Replaces some filter entries of ``attrs`` (in place, see the module
    docstring) by references to new objects of ``doc``'s table."""

    def ref_to(x):
        if rng.random() < 0.1:
            return PDFObjRef(doc, rng.choice([MISSING_OBJID, BROKEN_OBJID]))
        objid = len(doc.table) + 1
        doc.table[objid] = x
        ref = PDFObjRef(doc, objid)
        while rng.random() < 0.2:  # a chain
            objid = len(doc.table) + 1
            doc.table[objid] = ref
            ref = PDFObjRef(doc, objid)
        return ref

    def params(p):
        d = p.attrs if isinstance(p, PDFStream) else p
        if isinstance(d, dict):
            for k in PARAM_KEYS:
                if k in d and rng.random() < 0.2:
                    d[k] = ref_to(d[k])
        return ref_to(p) if rng.random() < 0.15 else p

    for key in list(attrs):
        if key in ("F", "Filter"):
            v = attrs[key]
            if isinstance(v, list):
                v = [ref_to(x) if rng.random() < 0.15 else x for x in v]
            attrs[key] = ref_to(v) if rng.random() < 0.15 else v
        elif key in ("DP", "DecodeParms", "FDecodeParms"):
            v = attrs[key]
            if isinstance(v, list):
                v = [params(x) for x in v]
                attrs[key] = ref_to(v) if rng.random() < 0.15 else v
            else:
                attrs[key] = params(v)


def gen_streams(rng):
    for _ in range(4000):
        kinds = [rng.choice(["flate", "flate", "lzw", "a85", "ahx", "rl", "dct", "jpx", "jbig2"]) for _ in range(rng.choice([1, 1, 1, 2, 3]))]
        colors = rng.choice([1, 1, 3, 4])
        columns = rng.choice([1, 4, 10, 33])
        bpc = rng.choice([8, 8, 1, 16])
        rows_info = (colors, columns, bpc)
        params = [rand_params(rng, k, rows_info) for k in kinds]
        # the data: rows predicted for the last predictor in decoding order
        data = sample(rng, rng.choice([0, 3, 40, 500, 3000]))
        for k, p in zip(kinds, params):
            if isinstance(p, dict) and p.get("Predictor") in (10, 11, 12, 13, 14, 15) and bpc == 8:
                bpp = colors
                nbytes = colors * columns
                rows = [data[i : i + nbytes] for i in range(0, len(data), nbytes)]
                data = png_encode(rng, rows, bpp)
        for k in reversed(kinds):
            data = encode(rng, k, data)
        if rng.random() < 0.15 and data:
            b = bytearray(data)
            b[rng.randrange(len(b))] ^= 1 << rng.randrange(8)
            data = bytes(b)
        if rng.random() < 0.1 and data:
            data = data[: rng.randrange(len(data))]
        names = [rng.choice(FILTER_NAMES[k]) for k in kinds]
        if rng.random() < 0.05:
            names[rng.randrange(len(names))] = rng.choice([LIT("Crypt"), LIT("Foo"), LIT("CCITTFaxDecode"), 5, b"x", None])
        attrs = {}
        fkey = rng.choice(["Filter", "Filter", "F"])
        attrs[fkey] = names[0] if len(names) == 1 and rng.random() < 0.6 else names
        if any(p is not None for p in params) or rng.random() < 0.1:
            pkey = rng.choice(["DecodeParms", "DecodeParms", "DP", "FDecodeParms"])
            pv = params[0] if len(params) == 1 and rng.random() < 0.6 else params
            r = rng.random()
            if r < 0.05:
                pv = params[:-1]  # fewer parameters than filters
            elif r < 0.1:
                pv = params + [{}]
            elif r < 0.15 and isinstance(pv, dict):
                pv = PDFStream(pv, b"")
            elif r < 0.18:
                pv = rng.choice([5, b"Predictor", LIT("X"), True, [5]])
            attrs[pkey] = pv
        if rng.random() < 0.1:
            attrs["Length"] = len(data)
        doc = TableDoc(None)
        if rng.random() < 0.4:
            refify(rng, attrs, doc)
        s = PDFStream(attrs, data)
        filters = call(s.get_filters)
        out = call(s.get_data)
        yield [attrs, data, sorted(doc.table.items())], [filters, out, s.get_rawdata()]


GENERATORS = {
    "lzw": gen_lzw,
    "ascii85": gen_ascii85,
    "asciihex": gen_asciihex,
    "runlength": gen_runlength,
    "paeth": gen_paeth,
    "tiff": gen_tiff,
    "png": gen_png,
    "corrupted": gen_corrupted,
    "streams": gen_streams,
}


# -- real corpus ------------------------------------------------------------------

FILTER_KEYS = ("F", "Filter", "DP", "DecodeParms", "FDecodeParms")
PASS_THROUGH = {"DCTDecode", "DCT", "JPXDecode", "JBIG2Decode"}


def snapshot(x, depth=0):
    """A copy of ``x`` with references resolved (a missing object is None);
    a stream becomes a stream with its attributes and no data."""
    if depth > 30:
        return None
    while isinstance(x, PDFObjRef):
        try:
            x = x.resolve()
        except Exception:  # noqa: BLE001
            return None
        depth += 1
        if depth > 30:
            return None
    if isinstance(x, list):
        return [snapshot(v, depth + 1) for v in x]
    if isinstance(x, dict):
        return {k: snapshot(v, depth + 1) for k, v in x.items()}
    if isinstance(x, PDFStream):
        return PDFStream({k: snapshot(v, depth + 1) for k, v in x.attrs.items()}, b"")
    if isinstance(x, (PSLiteral, PSKeyword, bytes, int, float, str)) or x is None:
        return x
    return None


def filter_names(attrs):
    f = attrs.get("F", attrs.get("Filter"))
    f = f if isinstance(f, list) else [f]
    return {getattr(x, "name", None) for x in f}


def collect(path, password):
    with open_document(path, password) as doc:
        objids = {}
        for xref in doc.xrefs:
            try:
                for objid in xref.get_objids():
                    objids.setdefault(objid, None)
            except Exception:  # noqa: BLE001
                pass
        for objid in fetch_order(objids):
            try:
                obj = doc.getobj(objid)
            except Exception:  # noqa: BLE001
                continue
            if not isinstance(obj, PDFStream) or obj.rawdata is None:
                continue
            raw = obj.rawdata
            if obj.decipher is not None:
                try:
                    raw = obj.decipher(obj.objid, obj.genno, raw, obj.attrs)
                except Exception:  # noqa: BLE001
                    continue
            if len(raw) > MAX_RAW:
                continue
            attrs = {k: snapshot(v) for k, v in obj.attrs.items() if k in FILTER_KEYS}
            names = filter_names(attrs)
            if names and names <= PASS_THROUGH and len(raw) > 4096:
                continue
            yield objid, attrs, raw


class Timeout(Exception):
    pass


def on_alarm(signum, frame):
    raise Timeout()


def process_file(args):
    path, rel, password = args
    signal.signal(signal.SIGALRM, on_alarm)
    signal.alarm(FILE_TIMEOUT_S)
    out = []
    status = None
    budget = MAX_FILE_RAW
    try:
        for objid, attrs, raw in collect(path, password):
            budget -= len(raw)
            if budget < 0:
                break
            a = dumps(attrs).encode("ascii")
            key = hashlib.sha256(struct.pack("<I", len(a)) + a + raw).hexdigest()
            try:
                result = PDFStream(attrs, raw).get_data()
                result = ("sha256", len(result), hashlib.sha256(result).hexdigest())
            except Timeout:
                raise
            except Exception as e:  # noqa: BLE001
                result = e
            out.append((key, f"{rel}#{objid}", a, raw, dumps(result)))
    except Timeout:
        status = "timeout"
    except Exception as e:  # noqa: BLE001 - unreadable document
        status = type(e).__name__
    finally:
        signal.alarm(0)
    return rel, status, out


def build_corpus(jobs):
    files = load_set("all")
    tasks = [(str(f.path), f.rel, f.password) for f in files]
    seen = set()
    records, total, skipped = [], 0, collections.Counter()
    out_dir = unit_dir("filters")
    tmp = out_dir / ".corpus.bin.tmp"
    with open(tmp, "wb") as f, multiprocessing.Pool(jobs) as pool:
        f.write(b"FLT1" + struct.pack("<I", 0))
        for k, (rel, status, cases) in enumerate(pool.imap(process_file, tasks, chunksize=1), 1):
            if k % 50 == 0:
                print(f"{k}/{len(tasks)} files, {len(records)} streams, {total / 1e6:.1f} MB", file=sys.stderr, flush=True)
            if status is not None:
                skipped[status] += 1
                print(f"skip {rel}: {status}", file=sys.stderr)
            for key, case_id, a, raw, result in cases:
                if key in seen:
                    continue
                seen.add(key)
                f.write(struct.pack("<I", len(a)) + a + struct.pack("<I", len(raw)) + raw)
                records.append({"id": case_id, "out": json.loads(result)})
                total += len(raw)
        f.seek(4)
        f.write(struct.pack("<I", len(records)))
    tmp.replace(out_dir / "corpus.bin")
    with open(out_dir / "corpus.json", "w", encoding="ascii") as f:
        json.dump({"sources": oracle_sources(), "records": records}, f)
    outcomes = collections.Counter(r["out"]["exc"] if isinstance(r["out"], dict) and "exc" in r["out"] else "bytes" for r in records)
    print(f"{len(records)} streams from {len(files)} files ({dict(skipped)} skipped), {total / 1e6:.1f} MB raw, {dict(outcomes)}")


def main():
    ap = argparse.ArgumentParser(prog="python -m oracle.unit.filters")
    ap.add_argument("--no-corpus", action="store_true", help="only the generated cases")
    ap.add_argument("-j", "--jobs", type=int, default=3)
    args = ap.parse_args()
    require_oracle_python()
    counts = {name: write_cases("filters", name, gen(random.Random(f"20261015:{name}"))) for name, gen in GENERATORS.items()}
    if not args.no_corpus:
        build_corpus(args.jobs)
    write_meta("filters", counts=counts)
    print(f"wrote {counts}")


if __name__ == "__main__":
    main()
