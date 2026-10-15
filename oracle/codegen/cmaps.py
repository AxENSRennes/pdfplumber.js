"""Convert pdfminer's predefined CMaps (``pdfminer/cmap/*.json.gz``) to the
compact files the Rust ``CMapDB`` loads through a ``CMapProvider``.

    PDFPLUMBER_ROOT=<main checkout> <root>/.venv/bin/python -m oracle.codegen.cmaps [--out DIR] [--check]

The ~8 MB of gzipped JSON are not embedded in the Rust core: the core asks a
provider for a CMap by name (``crates/pdfminer/src/cmapdb.rs``), and only the
documents that need one load it. This script writes one file per CMap,
``<name>.pdfcmap``, plus ``names.txt`` (one name per line, the names the
provider knows), into ``<cache>/cmap/<fingerprint>/`` (``<cache>`` as in
``oracle/corpus.py``: ``PDFPLUMBER_CACHE`` or ``$PDFPLUMBER_ROOT/.cache``),
or ``--out DIR``. The fingerprint hashes this script and every input file,
so a new pdfminer or format gets a new directory; the files are not
committed (about 4 MB in all, regenerated in seconds). It prints the
directory. ``--check`` regenerates in memory and exits 1 unless every file
of the directory is byte-identical to it.

The data is what the oracle loads: each file goes through pdfminer's own
``CMapDB._load_data`` (gzip, JSON, key conversion), and the attributes of
the class it returns are written in their order (``IS_VERTICAL``,
``CODE2CID`` or ``CID2UNICHR_H``, ``CID2UNICHR_V``). Format (integers are
unsigned LEB128 varints, ``str`` UTF-8 with a varint length)::

    file      = b"PMCM" version=1 (u8) nattrs (varint) attr*
    attr      = name (str) tag (u8) value
      tag b"B": a bool (u8 0/1)                       (IS_VERTICAL)
      tag b"C": a code2cid tree (node)                (CODE2CID)
      tag b"U": a cid2unichr dict (umap)              (CID2UNICHR_H / _V)
    node      = nitems (varint) item*   -- dict[int, int | node], in dict order (a run is one item)
      item    = h (varint) ...
        h even, h >= 2: a run of h/2 leaves: key0 cid0 (varints): key0+j -> cid0+j
        h == 1: key (varint) node: key -> a nested dict
    umap      = nitems (varint) item*   -- dict[int, str], in dict order (a run is one item)
      item    = h (varint) ...
        h even, h >= 2: a run of h/2 one-character values: cid0 cp0: cid0+j -> chr(cp0+j)
        h == 1: cid (varint) text (str)

The script refuses data the format cannot carry exactly (negative or
non-int keys and values, ``bool`` values, other attributes, surrogates).
"""

from __future__ import annotations

import argparse
import hashlib
import os
import shutil
import sys
from pathlib import Path

import pdfminer
from pdfminer.cmapdb import CMapDB

from ..corpus import cache_dir

FORMAT_VERSION = 1
CMAP_DIR = Path(pdfminer.__file__).resolve().parent / "cmap"
SUFFIX = ".json.gz"


def varint(n: int) -> bytes:
    if type(n) is not int or n < 0:
        raise ValueError(f"not a non-negative int: {n!r}")
    out = bytearray()
    while True:
        b = n & 0x7F
        n >>= 7
        if n:
            out.append(b | 0x80)
        else:
            out.append(b)
            return bytes(out)


def text(s: str) -> bytes:
    if any(0xD800 <= ord(c) < 0xE000 for c in s):
        raise ValueError(f"a surrogate in {s!r}")
    b = s.encode("utf-8")
    return varint(len(b)) + b


def node(d: dict) -> bytes:
    items = list(d.items())
    out = []
    i = 0
    while i < len(items):
        k, v = items[i]
        if type(k) is not int:
            raise ValueError(f"not an int key: {k!r}")
        if isinstance(v, dict):
            out.append(varint(1) + varint(k) + node(v))
            i += 1
            continue
        if type(v) is not int:
            raise ValueError(f"not an int value: {v!r}")
        j = i + 1
        while j < len(items):
            k2, v2 = items[j]
            if type(k2) is not int or type(v2) is not int or k2 != k + (j - i) or v2 != v + (j - i):
                break
            j += 1
        out.append(varint(2 * (j - i)) + varint(k) + varint(v))
        i = j
    return varint(len(out)) + b"".join(out)


def umap(d: dict) -> bytes:
    items = list(d.items())
    out = []
    i = 0
    while i < len(items):
        k, v = items[i]
        if type(k) is not int or type(v) is not str:
            raise ValueError(f"not an int -> str item: {k!r}: {v!r}")
        if len(v) != 1:
            out.append(varint(1) + varint(k) + text(v))
            i += 1
            continue
        text(v)  # checks for surrogates
        j = i + 1
        while j < len(items):
            k2, v2 = items[j]
            if type(k2) is not int or type(v2) is not str or len(v2) != 1 or k2 != k + (j - i):
                break
            if ord(v2) != ord(v) + (j - i) or 0xD800 <= ord(v2) < 0xE000:
                break
            j += 1
        out.append(varint(2 * (j - i)) + varint(k) + varint(ord(v)))
        i = j
    return varint(len(out)) + b"".join(out)


def encode(name: str) -> bytes:
    """The compact file of the CMap ``name``, from ``CMapDB._load_data``."""
    cls = CMapDB._load_data(name)
    attrs = {k: v for k, v in vars(cls).items() if not (k.startswith("__") and k.endswith("__"))}
    out = [b"PMCM", bytes([FORMAT_VERSION]), varint(len(attrs))]
    for key, value in attrs.items():
        out.append(text(key))
        if key == "IS_VERTICAL" and type(value) is bool:
            out.append(b"B" + bytes([value]))
        elif key == "CODE2CID" and isinstance(value, dict):
            out.append(b"C" + node(value))
        elif key in ("CID2UNICHR_H", "CID2UNICHR_V") and isinstance(value, dict):
            out.append(b"U" + umap(value))
        else:
            raise ValueError(f"{name}: unexpected attribute {key}={type(value).__name__}")
    return b"".join(out)


def names() -> list[str]:
    """The CMap names of pdfminer's cmap directory (files ``<name>.json.gz``)."""
    return sorted(p.name[: -len(SUFFIX)] for p in CMAP_DIR.iterdir() if p.name.endswith(SUFFIX))


def fingerprint() -> str:
    h = hashlib.sha256()
    h.update(Path(__file__).read_bytes())
    for name in names():
        h.update(name.encode() + b"\0" + hashlib.sha256((CMAP_DIR / (name + SUFFIX)).read_bytes()).digest())
    return h.hexdigest()[:16]


def render() -> dict[str, bytes]:
    files = {f"{name}.pdfcmap": encode(name) for name in names()}
    files["names.txt"] = "".join(f"{name}\n" for name in names()).encode("ascii")
    return files


def default_dir() -> Path:
    return cache_dir() / "cmap" / fingerprint()


def ensure(out: Path | None = None) -> Path:
    """The directory of the compact CMaps, written first if it is missing."""
    out = out or default_dir()
    if (out / "names.txt").is_file():
        return out
    tmp = out.with_name(f".{out.name}.{os.getpid()}.tmp")
    shutil.rmtree(tmp, ignore_errors=True)
    tmp.mkdir(parents=True)
    for fname, data in render().items():
        (tmp / fname).write_bytes(data)
    try:
        tmp.rename(out)
    except OSError:
        if not (out / "names.txt").is_file():
            raise
        shutil.rmtree(tmp, ignore_errors=True)  # written concurrently by another process
    return out


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(prog="python -m oracle.codegen.cmaps", description=__doc__.split("\n\n")[0])
    ap.add_argument("--out", type=Path, help="output directory (default: <cache>/cmap/<fingerprint>)")
    ap.add_argument("--check", action="store_true", help="exit 1 unless the directory holds exactly what is generated")
    args = ap.parse_args(argv)
    out = args.out or default_dir()
    if args.check:
        files = render()
        present = sorted(p.name for p in out.iterdir()) if out.is_dir() else []
        bad = [f for f in files if f not in present or (out / f).read_bytes() != files[f]]
        bad += [f for f in present if f not in files]
        if bad:
            print(f"{out}: {len(bad)} files differ or are missing/extra, e.g. {bad[:3]}", file=sys.stderr)
            return 1
        print(f"{out}: {len(files)} files identical")
        return 0
    ensure(out)
    total = sum(p.stat().st_size for p in out.iterdir())
    print(f"{out} ({len(names())} CMaps, {total / 1e6:.1f} MB)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
