"""Differential cases for crates/pycompat/src/zlib/ (zlib 1.3 inflate under
CPython 3.12's ``zlib`` module), computed by CPython.

Writes typed JSON lines (docs/verification.md) to
``<cache>/unit/zlib/<fingerprint>/<name>.jsonl`` (``oracle/unit/_common.py``);
the Rust replay is crates/pdftrace/tests/zlib_unit.rs. A result of more than
``BLOB_MAX`` bytes is written ``("sha256", len, hexdigest)``.

- ``decompress``: ``zlib.decompress(data, wbits, bufsize)`` -> bytes or
  exception; ``in``: ``[data, wbits, bufsize]``.
- ``decompressobj``: ``in``: ``[data, wbits, feeds]``; for each feed, a new
  ``d = zlib.decompressobj(wbits)`` gets the calls of the feed, a list of
  ``[n, max_length, repeat]``: ``repeat`` calls each passing
  ``d.unconsumed_tail + data[pos:pos + n]`` (``pos`` advancing by ``n``)
  and ``max_length``; ``out``: per feed, ``[results, unused_data,
  unconsumed_tail, eof]``, ``results`` holding each call's output up to the
  first exception (included, then no more calls).

Inputs (seed fixed):

- real compressors: ``zlib.compressobj`` with every level, strategy,
  ``wbits`` (zlib and raw) and memory level, on random, text-like, run-heavy
  and incompressible data, sizes around the 16 KiB / 32 KiB window / 64 KiB
  / 256 KiB output-block boundaries of CPython's output buffer; with a
  ``zdict`` (``Z_NEED_DICT``); trailing bytes after the stream;
- every truncation of small streams, bit flips and byte changes;
- every 2-byte zlib header in front of one body;
- crafted deflate bit streams (``rand_deflate``): stored blocks with bad
  lengths, block type 3, fixed blocks using the invalid symbols 286/287 and
  distances 30/31, dynamic blocks with oversubscribed or incomplete code
  length / literal / distance codes, too many symbols, repeat codes at the
  start or past the end, a missing end-of-block code, a single distance
  code, distances exactly at and one past the output so far, matches
  crossing the window, and wrong Adler-32 trailers; headers with small
  windows decoded with ``wbits=0`` (the window then is smaller than the
  output blocks);
- feeding: whole, 1 byte at a time (what ``pdftypes.decompress_corrupted``
  does), random chunk sizes, small ``max_length`` (``unconsumed_tail``);
  ``bufsize`` 0, 1, 7, default, 64 KiB; invalid ``wbits``.

Usage: PDFPLUMBER_ROOT=<main checkout> <root>/.venv/bin/python -m oracle.unit.zlib
"""

from __future__ import annotations

import hashlib
import random
import zlib

from ._common import require_oracle_python, write_cases, write_meta

BLOB_MAX = 2048
#: streams up to this size are also fed byte by byte, truncated, flipped
SMALL = 20000


def blob(b: bytes):
    if len(b) <= BLOB_MAX:
        return b
    return ("sha256", len(b), hashlib.sha256(b).hexdigest())


def call(f, *args):
    try:
        return blob(f(*args))
    except Exception as e:  # noqa: BLE001 - the exception is the result
        return e


# -- the two operations ------------------------------------------------------


def run_decompress(data, wbits=15, bufsize=zlib.DEF_BUF_SIZE):
    return [data, wbits, bufsize], call(zlib.decompress, data, wbits, bufsize)


def feed(data, wbits, calls):
    """One decompressor fed by ``calls`` (see the module docstring)."""
    try:
        d = zlib.decompressobj(wbits)
    except Exception as e:  # noqa: BLE001
        return [[e], b"", b"", False]
    results = []
    pos = 0
    for n, max_length, repeat in calls:
        for _ in range(repeat):
            chunk = d.unconsumed_tail + data[pos : pos + n]
            pos += n
            try:
                results.append(blob(d.decompress(chunk, max_length)))
            except Exception as e:  # noqa: BLE001
                results.append(e)
                return [results, blob(d.unused_data), blob(d.unconsumed_tail), d.eof]
    return [results, blob(d.unused_data), blob(d.unconsumed_tail), d.eof]


def run_decompressobj(data, wbits, feeds):
    return [data, wbits, feeds], [feed(data, wbits, calls) for calls in feeds]


def feedings(rng, data):
    """Call lists for one stream: whole, byte by byte, random chunks, and
    small max_length."""
    n = len(data)
    out = [[[n, 0, 1]]]
    if n <= SMALL:
        out.append([[1, 0, n], [0, 0, 1]])
    calls, left = [], n
    while left > 0:
        k = rng.choice([1, 2, 3, 5, 7, 64, 300, 5000, 40000])
        calls.append([k, 0, 1])
        left -= k
    out.append(calls)
    ml = rng.choice([1, 2, 100, 258, 1000, 40000])
    out.append([[n, ml, 1], [0, ml, rng.randint(1, 6)]])
    return out


# -- data and real compressors -----------------------------------------------

WORDS = [b"the", b"stream", b"obj", b"endobj", b"BT", b"ET", b"/F1", b"12", b"Tf", b"0.5", b"Td", b"(Hello)", b"Tj", b"\n", b" "]


def sample_data(rng, size):
    kind = rng.randrange(5)
    if kind == 0 and size > 40000:
        kind = 1  # keeps the case files small: large outputs compress well
    if kind == 0:
        return rng.randbytes(size)
    if kind == 1:
        return b"".join(rng.choice(WORDS) for _ in range(size // 3 + 1))[:size]
    if kind == 2:
        out = bytearray()
        while len(out) < size:
            out += bytes([rng.randrange(4)]) * rng.randint(1, 300)
        return bytes(out[:size])
    if kind == 3:
        unit = rng.randbytes(rng.randint(1, 40))
        return (unit * (size // len(unit) + 1))[:size]
    # long-distance repeats: a block repeated after a gap near 32 KiB
    block = rng.randbytes(rng.randint(3, 400))
    gap = rng.randbytes(rng.choice([32000, 32765, 32766, 32767, 32768, 33000]))
    return (block + gap + block + rng.randbytes(10))[:size] if size > len(block) else block[:size]


SIZES = [0, 1, 2, 3, 10, 100, 257, 258, 259, 1000, 4096, 16383, 16384, 16385, 32767, 32768, 32769, 65536, 81919, 81920,
         81921, 98304, 100000, 262144, 360447, 360448, 360449, 400000]


def compress(rng, data, wbits=None, level=None, strategy=None, memlevel=None, zdict=None):
    wbits = wbits if wbits is not None else rng.choice([15, 15, 15, 9, 10, 12, 14, -15, -9, -12])
    level = level if level is not None else rng.choice([-1, 0, 1, 2, 5, 6, 9])
    strategy = strategy if strategy is not None else rng.choice(
        [zlib.Z_DEFAULT_STRATEGY, zlib.Z_FILTERED, zlib.Z_HUFFMAN_ONLY, zlib.Z_RLE, zlib.Z_FIXED])
    memlevel = memlevel if memlevel is not None else rng.choice([1, 8, 9])
    if zdict is not None:
        c = zlib.compressobj(level, zlib.DEFLATED, wbits, memlevel, strategy, zdict)
    else:
        c = zlib.compressobj(level, zlib.DEFLATED, wbits, memlevel, strategy)
    out = bytearray()
    for k in range(0, max(len(data), 1), 7000):
        out += c.compress(data[k : k + 7000])
        if rng.random() < 0.1:
            out += c.flush(rng.choice([zlib.Z_SYNC_FLUSH, zlib.Z_FULL_FLUSH]))
    out += c.flush()
    return bytes(out), wbits


def real_streams(rng):
    """(stream, wbits to decode with) from zlib's compressor."""
    for size in SIZES:
        for _ in range(3):
            data = sample_data(rng, size)
            yield compress(rng, data)
    for _ in range(400):
        data = sample_data(rng, rng.choice([rng.randint(0, 300), rng.randint(0, 5000), rng.randint(0, 70000)]))
        yield compress(rng, data)
    for _ in range(20):  # a dictionary: Z_NEED_DICT for the decompressor
        data = sample_data(rng, rng.randint(0, 2000))
        yield compress(rng, data, wbits=15, zdict=b"the stream obj")


# -- crafted deflate streams ---------------------------------------------------


class BitWriter:
    def __init__(self):
        self.acc = 0
        self.n = 0
        self.out = bytearray()

    def bits(self, value, n):
        """``n`` bits of ``value``, least significant first."""
        self.acc |= (value & ((1 << n) - 1)) << self.n
        self.n += n
        while self.n >= 8:
            self.out.append(self.acc & 0xFF)
            self.acc >>= 8
            self.n -= 8

    def code(self, code, length):
        """A Huffman code: most significant bit first."""
        rev = 0
        for i in range(length):
            rev = (rev << 1) | ((code >> i) & 1)
        self.bits(rev, length)

    def align(self):
        if self.n:
            self.bits(0, 8 - self.n)

    def getvalue(self):
        out = bytes(self.out)
        if self.n:
            out += bytes([self.acc & 0xFF])
        return out


def canonical(lengths):
    """Canonical codes (RFC 1951 3.2.2) of ``lengths``; for an invalid set
    the codes are whatever the algorithm gives (masked to the length)."""
    max_len = max(lengths, default=0)
    bl_count = [0] * (max_len + 2)
    for ln in lengths:
        if ln:
            bl_count[ln] += 1
    code, next_code = 0, [0] * (max_len + 2)
    for b in range(1, max_len + 1):
        code = (code + bl_count[b - 1]) << 1
        next_code[b] = code
    codes = []
    for ln in lengths:
        if ln:
            codes.append(next_code[ln] & ((1 << ln) - 1))
            next_code[ln] += 1
        else:
            codes.append(None)
    return codes


def rand_lengths(rng, n_symbols, used, max_bits):
    """Lengths of a complete prefix code over ``used`` (a random full binary
    tree of depth <= max_bits), 0 for the other symbols."""
    used = list(used)
    if len(used) == 1:
        lengths = [0] * n_symbols
        lengths[used[0]] = 1
        return lengths
    leaves = [1, 1]
    while len(leaves) < len(used):
        candidates = [i for i, d in enumerate(leaves) if d < max_bits]
        i = rng.choice(candidates)
        d = leaves.pop(i)
        leaves += [d + 1, d + 1]
    rng.shuffle(leaves)
    lengths = [0] * n_symbols
    for s, d in zip(used, leaves):
        lengths[s] = d
    return lengths


def mutate_lengths(rng, lengths, max_bits):
    lengths = list(lengths)
    k = rng.randrange(len(lengths))
    r = rng.random()
    if r < 0.4:
        lengths[k] = rng.randint(1, max_bits)  # often oversubscribed
    elif r < 0.8:
        nz = [i for i, x in enumerate(lengths) if x]
        if nz:
            lengths[rng.choice(nz)] = 0  # incomplete
    else:
        lengths[k] = 0
    return lengths


LEN_BASE = [3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258]
LEN_EXTRA = [0] * 8 + [1] * 4 + [2] * 4 + [3] * 4 + [4] * 4 + [5] * 4 + [0]
DIST_BASE = [1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097,
             6145, 8193, 12289, 16385, 24577]
DIST_EXTRA = [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13]


def len_symbol(length):
    for i in range(len(LEN_BASE) - 1, -1, -1):
        if LEN_BASE[i] <= length:
            if i == 28 and length != 258:
                continue
            return 257 + i, length - LEN_BASE[i], LEN_EXTRA[i]
    raise ValueError(length)


def dist_symbol(dist):
    for i in range(len(DIST_BASE) - 1, -1, -1):
        if DIST_BASE[i] <= dist:
            return i, dist - DIST_BASE[i], DIST_EXTRA[i]
    raise ValueError(dist)


FIXED_LIT = [8] * 144 + [9] * 112 + [7] * 24 + [8] * 8
FIXED_DIST = [5] * 32


def rand_tokens(rng, produced, wsize, n):
    """Random literals and matches; mostly valid distances (<= output so
    far and the window), sometimes exactly at or one past the limit."""
    tokens = []
    for _ in range(n):
        if produced == 0 or rng.random() < 0.5:
            tokens.append(("lit", rng.randrange(256)))
            produced += 1
            continue
        length = rng.choice([3, 4, 10, 18, 66, 130, 257, 258, rng.randint(3, 258)])
        limit = min(produced, wsize)
        r = rng.random()
        if r < 0.03:
            dist = min(limit + 1, 32768)
        elif r < 0.08:
            dist = limit
        else:
            dist = rng.randint(1, limit)
        tokens.append(("match", length, dist))
        produced += length
    return tokens, produced


def write_tokens(rng, w, tokens, lit_codes, lit_lens, dist_codes, dist_lens, fixed):
    for t in tokens:
        if t[0] == "lit":
            sym = t[1]
            if lit_codes[sym] is None:
                continue
            w.code(lit_codes[sym], lit_lens[sym])
        else:
            _, length, dist = t
            sym, extra, nextra = len_symbol(length)
            dsym, dextra, dnextra = dist_symbol(dist)
            if lit_codes[sym] is None or dist_codes[dsym] is None:
                continue
            w.code(lit_codes[sym], lit_lens[sym])
            w.bits(extra, nextra)
            w.code(dist_codes[dsym], dist_lens[dsym])
            w.bits(dextra, dnextra)
    if fixed and rng.random() < 0.03:
        # an invalid symbol of the fixed codes: 286/287, or distance 30/31
        if rng.random() < 0.5:
            sym = rng.choice([286, 287])
            w.code(lit_codes[sym], lit_lens[sym])
        else:
            w.code(lit_codes[257], lit_lens[257])
            d = rng.choice([30, 31])
            w.code(dist_codes[d], dist_lens[d])
        w.bits(rng.getrandbits(16), 16)
    if lit_codes[256] is not None and rng.random() < 0.97:
        w.code(lit_codes[256], lit_lens[256])


def write_dynamic_header(rng, w, lit_lens, dist_lens):
    hlit, hdist = len(lit_lens), len(dist_lens)
    lens = lit_lens + dist_lens
    # run-length encode with 16/17/18, sometimes wrongly
    symbols = []
    i = 0
    while i < len(lens):
        run = 1
        while i + run < len(lens) and lens[i + run] == lens[i]:
            run += 1
        if lens[i] == 0 and run >= 3 and rng.random() < 0.8:
            k = min(run, 138)
            symbols.append((18, k - 11, 7) if k >= 11 else (17, k - 3, 3))
            i += k
        elif i > 0 and lens[i] == lens[i - 1] and run >= 3 and rng.random() < 0.8:
            k = min(run, 6)
            symbols.append((16, k - 3, 2))
            i += k
        else:
            symbols.append((lens[i], 0, 0))
            i += 1
    if rng.random() < 0.05:
        symbols.insert(0, (16, rng.randrange(4), 2))  # repeat with no previous length
    if rng.random() < 0.05:
        symbols.append((18, 127, 7))  # repeat past the end
    used_cl = sorted({s for s, _, _ in symbols})
    cl_lens = rand_lengths(rng, 19, used_cl, 7)
    if rng.random() < 0.05:
        cl_lens = mutate_lengths(rng, cl_lens, 7)
    order = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15]
    hclen = 19
    while hclen > 4 and cl_lens[order[hclen - 1]] == 0:
        hclen -= 1
    if rng.random() < 0.03:
        hclen = rng.randint(4, 19)
    w.bits(hlit - 257, 5)
    w.bits(hdist - 1, 5)
    w.bits(hclen - 4, 4)
    for k in range(hclen):
        w.bits(cl_lens[order[k]], 3)
    cl_codes = canonical(cl_lens)
    for s, extra, n in symbols:
        if cl_codes[s] is None:
            continue
        w.code(cl_codes[s], cl_lens[s])
        w.bits(extra, n)


def rand_deflate(rng, wsize=32768):
    """A random raw deflate stream (often invalid somewhere)."""
    w = BitWriter()
    produced = 0
    nblocks = rng.choice([1, 1, 2, 3, 5])
    for b in range(nblocks):
        last = b == nblocks - 1
        btype = rng.choices([0, 1, 2, 3], [3, 4, 6, 0.2])[0]
        w.bits(1 if last else 0, 1)
        w.bits(btype, 2)
        if btype == 3:
            w.bits(rng.getrandbits(24), 24)
            break
        if btype == 0:
            w.align()
            n = rng.choice([0, 1, 5, 100, rng.randint(0, 1500)])
            nlen = (~n) & 0xFFFF if rng.random() < 0.95 else rng.getrandbits(16)
            w.bits(n, 16)
            w.bits(nlen, 16)
            for x in rng.randbytes(n):
                w.bits(x, 8)
            produced += n
            continue
        tokens, produced = rand_tokens(rng, produced, wsize, rng.choice([0, 1, 5, 40, 400, 1500]))
        if btype == 1:
            lit_lens, dist_lens = FIXED_LIT, FIXED_DIST
        else:
            used_lit = {256} | {t[1] for t in tokens if t[0] == "lit"} | {len_symbol(t[1])[0] for t in tokens if t[0] == "match"}
            used_dist = {dist_symbol(t[2])[0] for t in tokens if t[0] == "match"}
            nlit = max(used_lit) + 1
            if rng.random() < 0.3:
                nlit = rng.randint(nlit, 286)
            if rng.random() < 0.02:
                nlit = rng.choice([287, 288])
            if not used_dist and rng.random() < 0.5:
                used_dist = set()
                ndist = rng.randint(1, 30)
                dist_lens = [0] * ndist  # no distance codes at all
            else:
                used_dist = used_dist or {rng.randrange(30)}
                ndist = max(used_dist) + 1
                if rng.random() < 0.3:
                    ndist = rng.randint(ndist, 30)
                dist_lens = rand_lengths(rng, ndist, sorted(used_dist), 15)
            if rng.random() < 0.02:
                ndist = rng.choice([31, 32])
                dist_lens = dist_lens + [0] * (ndist - len(dist_lens))
            lit_lens = rand_lengths(rng, nlit, sorted(used_lit), 15)
            if rng.random() < 0.08:
                lit_lens = mutate_lengths(rng, lit_lens, 15)
            if rng.random() < 0.08 and dist_lens:
                dist_lens = mutate_lengths(rng, dist_lens, 15)
            if rng.random() < 0.02:
                lit_lens[256] = 0  # no end-of-block code
            write_dynamic_header(rng, w, lit_lens, dist_lens)
        write_tokens(rng, w, tokens, canonical(lit_lens), lit_lens, canonical(dist_lens), dist_lens, btype == 1)
    if rng.random() < 0.3:
        w.bits(rng.getrandbits(8), 8)
    return w.getvalue()


def zlib_header(cinfo=7, fdict=False, level=2, bad=False):
    cmf = (cinfo << 4) | 8
    flg = (level << 6) | (0x20 if fdict else 0)
    flg += (31 - ((cmf << 8) + flg) % 31) % 31
    if bad:
        flg ^= 1
    return bytes([cmf, flg])


def wrap_zlib(rng, raw):
    """``raw`` in a zlib container; the Adler-32 is that of what zlib
    decodes when it decodes, sometimes wrong."""
    cinfo = rng.choice([7, 7, 7, 0, 1, 4])
    try:
        check = zlib.adler32(zlib.decompress(raw, -15))
    except zlib.error:
        check = rng.getrandbits(32)
    if rng.random() < 0.1:
        check ^= 1 << rng.randrange(32)
    return zlib_header(cinfo) + raw + check.to_bytes(4, "big"), (0 if cinfo != 7 and rng.random() < 0.7 else 15)


# -- cases ---------------------------------------------------------------------


def variants(rng, stream):
    """Truncations, bit flips, byte changes, trailing data."""
    n = len(stream)
    yield stream + rng.randbytes(rng.choice([1, 3, 10]))
    if n <= 200:
        for k in range(n):
            yield stream[:k]
    else:
        for k in sorted(rng.sample(range(n), 8)) + [n - 1, n - 2, n - 3, n - 4, n - 5]:
            yield stream[:k]
    if n:
        for _ in range(4):
            b = bytearray(stream)
            i = rng.randrange(n)
            b[i] ^= 1 << rng.randrange(8)
            yield bytes(b)
        b = bytearray(stream)
        b[rng.randrange(n)] = rng.randrange(256)
        yield bytes(b)


def gen_decompress(rng):
    for stream, wbits in real_streams(rng):
        yield run_decompress(stream, wbits)
        if rng.random() < 0.2:
            yield run_decompress(stream, wbits, rng.choice([0, 1, 7, 65536]))
        if len(stream) <= SMALL and rng.random() < 0.3:
            for v in variants(rng, stream):
                yield run_decompress(v, wbits)
    for _ in range(6000):
        raw = rand_deflate(rng)
        if rng.random() < 0.5:
            yield run_decompress(raw, -15)
            if rng.random() < 0.1:
                for v in variants(rng, raw):
                    yield run_decompress(v, -15)
        else:
            stream, wbits = wrap_zlib(rng, raw)
            yield run_decompress(stream, wbits, rng.choice([16384, 16384, 16384, 1, 300, 0]))
            if rng.random() < 0.1:
                for v in variants(rng, stream):
                    yield run_decompress(v, wbits)
    # every 2-byte header in front of one body
    body = zlib.compress(b"header test " * 20)[2:]
    for h in range(65536):
        if h % 31 == 0 or h % 97 == 0:
            yield run_decompress(h.to_bytes(2, "big") + body)
    # a small-window header in front of a stream using far distances,
    # decoded with wbits=0 (window of the header) across output blocks
    for cinfo in range(8):
        data = sample_data(rng, 70000) if cinfo % 2 else (rng.randbytes(300) * 300)
        raw, _ = compress(rng, data, wbits=-15, level=6, strategy=zlib.Z_DEFAULT_STRATEGY)
        stream = zlib_header(cinfo) + raw + zlib.adler32(data).to_bytes(4, "big")
        for bufsize in (1, 300, 16384, 65536):
            yield run_decompress(stream, 0, bufsize)
        yield run_decompress(stream, 15)
        yield run_decompress(zlib_header(cinfo, fdict=True) + b"\0\0\0\1" + raw, 15)
    # wbits and bufsize arguments
    ok = zlib.compress(b"abc" * 100)
    for wbits in [-16, -15, -9, -8, -7, 0, 7, 8, 9, 14, 15, 48, 64, 100, -100]:
        yield run_decompress(ok, wbits)
    for bufsize in [-1, 0, 1, 2, 299, 300, 301]:
        yield run_decompress(ok, 15, bufsize)


def gen_decompressobj(rng):
    for stream, wbits in real_streams(rng):
        if len(stream) > 20000 and rng.random() < 0.8:
            continue
        yield run_decompressobj(stream, wbits, feedings(rng, stream))
        if len(stream) <= SMALL and rng.random() < 0.3:
            for v in variants(rng, stream):
                yield run_decompressobj(v, wbits, [[[1, 0, len(v)]]])
    for _ in range(4000):
        raw = rand_deflate(rng)
        if rng.random() < 0.3:
            stream, wbits = raw, -15
        else:
            stream, wbits = wrap_zlib(rng, raw)
        yield run_decompressobj(stream, wbits, feedings(rng, stream))
    for wbits in [-16, -7, 7, 48, 100]:
        yield run_decompressobj(b"x", wbits, [[[1, 0, 1]]])
    ok = zlib.compress(b"abc" * 100)
    yield run_decompressobj(ok, 15, [[[5, -1, 1]]])


GENERATORS = {"decompress": gen_decompress, "decompressobj": gen_decompressobj}


def main():
    require_oracle_python()
    counts = {name: write_cases("zlib", name, gen(random.Random(f"20261015:{name}"))) for name, gen in GENERATORS.items()}
    write_meta("zlib", counts=counts, zlib_version=zlib.ZLIB_RUNTIME_VERSION)
    print(f"wrote {counts}")


if __name__ == "__main__":
    main()
