"""Build the table-finding corpus and the Python oracle.

Each job is one page's ``page.edges`` (what TableFinder reads with the default
"lines" strategy) plus table settings. The oracle runs pdfplumber's real
``TableFinder`` and serializes its whole trace: merged edges, intersection points,
cells, tables and table rows. Python ints are flagged per coordinate because
``sum()`` in ``snap_objects`` treats ints and floats differently (Python 3.12
Neumaier summation applies to floats only).

Outputs (in ``out/``): ``<name>.bin`` corpus, ``<name>_oracle.json``.

Corpus layout (little-endian): b"TBC1", u32 njobs; per job: 8 x f64 settings
(snap_x, snap_y, join_x, join_y, isect_x, isect_y, edge_min_length,
edge_min_length_prefilter), u32 nedges, then per edge 6 x f64 (x0, top, x1,
bottom, width, height), u8 int-flags (bit k set when field k is a Python int),
u8 orientation (0 None, 1 v, 2 h), u8 object_type (0 line, 1 rect_edge,
2 curve_edge).

Usage: python dump_tables.py [--max-pages N] | --fuzz N [--seed S] | --dump JOB [--corpus fuzz]
"""

import argparse
import glob
import hashlib
import json
import math
import os
import random
import struct
import sys
import time

import pdfplumber
from pdfplumber.table import TableFinder

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
OUT = os.path.join(HERE, "out")
SOURCES = [
    "test/fixtures/external-pdfs/*.pdf",
    "test/fixtures/external-holdout-pdfs/*.pdf",
    "pdfplumber-python/tests/pdfs/*.pdf",
    "pdfjs/test/pdfs/*.pdf",
]
FIELDS = ("x0", "top", "x1", "bottom", "width", "height")
ORIENT = {None: 0, "v": 1, "h": 2}
ORIENT_BACK = {0: None, 1: "v", 2: "h"}
TYPES = {"line": 0, "rect_edge": 1, "curve_edge": 2}
TYPES_BACK = {v: k for k, v in TYPES.items()}
SETTING_KEYS = (
    "snap_x_tolerance",
    "snap_y_tolerance",
    "join_x_tolerance",
    "join_y_tolerance",
    "intersection_x_tolerance",
    "intersection_y_tolerance",
    "edge_min_length",
    "edge_min_length_prefilter",
)
DEFAULT_SETTINGS = (3, 3, 3, 3, 3, 3, 3, 1)


class FakePage:
    """The only page attributes TableFinder touches with line strategies."""

    def __init__(self, edges):
        self.edges = edges
        self.bbox = (0, 0, 0, 0)
        self.chars = []


def num(v) -> str:
    if isinstance(v, bool):
        raise TypeError("bool coordinate")
    if isinstance(v, int) or (isinstance(v, float) and v.is_integer() and abs(v) <= 2**53 - 1):
        return f"d{int(v)}"
    return "f" + struct.pack(">d", v).hex()


def bbox(b) -> str:
    return ",".join("-" if v is None else num(v) for v in b)


def serialize(tf: TableFinder) -> bytes:
    out = []
    for e in tf.edges:
        out.append(f"E {bbox([e[k] for k in FIELDS])} {e['orientation']}")
    for (x, y), d in sorted(tf.intersections.items()):
        out.append(f"I {num(x)},{num(y)} {len(d['v'])} {len(d['h'])}")
    for c in tf.cells:
        out.append(f"C {bbox(c)}")
    for t in tf.tables:
        out.append(f"T {bbox(t.bbox)} {len(t.cells)}")
        for c in t.cells:
            out.append(f"  c {bbox(c)}")
        for r in t.rows:
            out.append("  r " + " ".join("-" if c is None else bbox(c) for c in r.cells))
    return ("\n".join(out) + "\n").encode("ascii")


def run_job(edges, settings):
    tf = TableFinder(FakePage(edges), dict(zip(SETTING_KEYS, settings)))
    for t in tf.tables:
        t.rows  # part of the measured work (needed by Table.extract)
    return tf


def outcome(edges, settings):
    try:
        return serialize(run_job(edges, settings)), "ok"
    except Exception as e:  # pdfplumber lets these escape to the caller
        return b"", "error:" + type(e).__name__


def time_job(edges, settings, repeat: int) -> float:
    best = float("inf")
    for _ in range(repeat):
        t0 = time.perf_counter()
        try:
            run_job(edges, settings)
        except Exception:
            pass
        best = min(best, time.perf_counter() - t0)
    return best


def edge_record(e) -> bytes:
    flags = 0
    for k, f in enumerate(FIELDS):
        if isinstance(e[f], int):
            flags |= 1 << k
    return struct.pack("<6d3B", *[float(e[f]) for f in FIELDS], flags, ORIENT[e["orientation"]], TYPES[e["object_type"]])


def write_corpus(name, jobs) -> None:
    with open(os.path.join(OUT, name + ".bin"), "wb") as f:
        f.write(b"TBC1" + struct.pack("<I", len(jobs)))
        for edges, settings in jobs:
            f.write(struct.pack("<8d", *[float(s) for s in settings]))
            f.write(struct.pack("<I", len(edges)))
            for e in edges:
                f.write(edge_record(e))


def read_corpus(name):
    data = open(os.path.join(OUT, name + ".bin"), "rb").read()
    assert data[:4] == b"TBC1"
    (n,) = struct.unpack_from("<I", data, 4)
    off, jobs = 8, []
    for _ in range(n):
        settings = struct.unpack_from("<8d", data, off)
        off += 64
        (ne,) = struct.unpack_from("<I", data, off)
        off += 4
        edges = []
        for _ in range(ne):
            *vals, flags, o, t = struct.unpack_from("<6d3B", data, off)
            off += 51
            e = {f: (int(v) if flags >> k & 1 else v) for k, (f, v) in enumerate(zip(FIELDS, vals))}
            e["orientation"] = ORIENT_BACK[o]
            e["object_type"] = TYPES_BACK[t]
            e["doctop"] = e["top"]
            edges.append(e)
        # settings are plain numbers; ints where integral, like user-provided values
        jobs.append((edges, tuple(int(s) if float(s).is_integer() else s for s in settings)))
    return jobs


# ---------------------------------------------------------------------------
# Adversarial generator: grids whose coordinates sit on the snap/join/
# intersection tolerance boundaries, int/float mixes that exercise sum(),
# duplicates, broken segments, diagonal curve edges, varied settings.


def fuzz_job(rng: random.Random):
    tol = rng.choice([3, 3, 3, 0, 1, 2.5, 0.5, 5])
    settings = [tol] * 6 + [rng.choice([3, 3, 0, 1, 2.9999999999999996, 10]), rng.choice([1, 1, 0, 3])]
    for k in range(6):
        if rng.random() < 0.2:
            settings[k] = rng.choice([0, 1, 3, 0.1, 4.000000000000001])

    def coord(base):
        r = rng.random()
        if r < 0.25:
            return int(round(base))  # Python int
        if r < 0.35:
            return float(int(round(base)))  # integral float
        if r < 0.5:
            return round(base, 1) + rng.choice([0.1, 0.2, 0.30000000000000004, 1e-12, -1e-12])
        if r < 0.6:
            return base + rng.choice([-1, 1]) * rng.choice([tol, tol + 1e-9, tol - 1e-9, 1.0, 3.0])
        return base + rng.uniform(-0.4, 0.4)

    edges = []

    def add(x0, top, x1, bottom, orientation, otype, width=None, height=None):
        if x1 < x0:
            x0, x1 = x1, x0
        if bottom < top:
            top, bottom = bottom, top
        edges.append(
            {
                "x0": x0,
                "x1": x1,
                "top": top,
                "bottom": bottom,
                "width": (x1 - x0) if width is None else width,
                "height": (bottom - top) if height is None else height,
                "orientation": orientation,
                "object_type": otype,
                "doctop": top,
            }
        )

    for _ in range(rng.choice([1, 1, 2, 3])):
        ox, oy = rng.uniform(0, 400), rng.uniform(0, 600)
        cols = sorted(ox + rng.uniform(5, 300) for _ in range(rng.randint(1, 9)))
        rows = sorted(oy + rng.uniform(5, 300) for _ in range(rng.randint(1, 12)))
        xs, ys = [ox] + cols, [oy] + rows
        style = rng.choice(["lines", "rects", "cells", "mixed"])
        for y in ys:
            if style in ("lines", "mixed") or rng.random() < 0.3:
                pieces = rng.choice([1, 1, 2, 3])
                cuts = sorted(rng.uniform(xs[0], xs[-1]) for _ in range(pieces - 1))
                bounds = [xs[0]] + cuts + [xs[-1]]
                for a, b in zip(bounds, bounds[1:]):
                    gap = rng.choice([0, 0, tol, tol + 1e-9, 1, 6]) if rng.random() < 0.3 else 0
                    yy = coord(y)
                    add(coord(a), yy, coord(b) - gap, yy, "h", "line")
        for x in xs:
            if style in ("lines", "mixed") or rng.random() < 0.3:
                xx = coord(x)
                add(xx, coord(ys[0]), xx, coord(ys[-1]), "v", "line")
        if style in ("rects", "cells", "mixed"):
            for i in range(len(xs) - 1):
                for j in range(len(ys) - 1):
                    if rng.random() < 0.85:
                        x0, x1, t, b = coord(xs[i]), coord(xs[i + 1]), coord(ys[j]), coord(ys[j + 1])
                        if x1 < x0:
                            x0, x1 = x1, x0
                        if b < t:
                            t, b = b, t
                        w, h = x1 - x0, b - t
                        add(x0, t, x1, t, "h", "rect_edge", height=0, width=w)
                        add(x0, b, x1, b, "h", "rect_edge", height=0, width=w)
                        add(x0, t, x0, b, "v", "rect_edge", width=0, height=h)
                        add(x1, t, x1, b, "v", "rect_edge", width=0, height=h)
    for _ in range(rng.choice([0, 0, 3, 10])):
        x, y = rng.uniform(0, 600), rng.uniform(0, 800)
        kind = rng.random()
        if kind < 0.3:
            add(x, y, x + rng.uniform(-5, 5), y + rng.uniform(-5, 5), None, "curve_edge")
        elif kind < 0.6:
            L = rng.choice([0, 0.5, 1, 2.9999, 3, 3.0000001, 50])
            add(x, y, x + L, y, "h", "line")
        else:
            L = rng.choice([0, 0.5, 1, 2.9999, 3, 3.0000001, 50])
            add(x, y, x, y + L, "v", "line")
    if edges and rng.random() < 0.3:
        for _ in range(rng.randint(1, 5)):
            edges.append(dict(rng.choice(edges)))  # exact duplicates
    rng.shuffle(edges)
    return edges, tuple(settings)


def page_jobs(path, max_pages):
    with pdfplumber.open(path) as pdf:
        for page in pdf.pages[:max_pages]:
            edges = page.edges
            if not edges:
                continue
            if any(not math.isfinite(e[f]) for e in edges for f in FIELDS):
                continue
            yield page.page_number, [dict(e) for e in edges]


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--max-pages", type=int, default=20)
    ap.add_argument("--repeat", type=int, default=3)
    ap.add_argument("--fuzz", type=int)
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--dump", type=int)
    ap.add_argument("--corpus", default="corpus")
    args = ap.parse_args()
    os.makedirs(OUT, exist_ok=True)

    if args.dump is not None:
        edges, settings = read_corpus(args.corpus)[args.dump]
        text, result = outcome(edges, settings)
        sys.stdout.buffer.write(text + f"!{result}\n".encode())
        return

    import logging

    logging.disable(logging.CRITICAL)
    jobs, records = [], []
    if args.fuzz:
        rng = random.Random(args.seed)
        source = ((f"fuzz#{k}", *fuzz_job(rng)) for k in range(args.fuzz))
    else:
        files = sorted(f for p in SOURCES for f in glob.glob(os.path.join(ROOT, p)))

        def real():
            for path in files:
                rel = os.path.relpath(path, ROOT)
                try:
                    for pageno, edges in page_jobs(path, args.max_pages):
                        yield f"{rel}#{pageno}", edges, DEFAULT_SETTINGS
                except Exception as e:
                    print(f"skip {rel}: {type(e).__name__}", file=sys.stderr)

        source = real()

    for job_id, edges, settings in source:
        jobs.append((edges, settings))
    write_corpus(args.corpus if not args.fuzz else "fuzz", jobs)
    # Re-read so the oracle sees exactly what the ports will see.
    name = "fuzz" if args.fuzz else args.corpus
    for k, (edges, settings) in enumerate(read_corpus(name)):
        text, result = outcome(edges, settings)
        tf_tables = text.count(b"\nT ") + text.startswith(b"T ")
        rec = {
            "edges": len(edges),
            "tables": tf_tables,
            "outcome": result,
            "sha256": hashlib.sha256(text + f"!{result}\n".encode()).hexdigest(),
            "int_coords": sum(1 for e in edges for f in ("x0", "top") if isinstance(e[f], int)),
        }
        rec["python_s"] = time_job(edges, settings, args.repeat)
        records.append(rec)
    with open(os.path.join(OUT, f"{name}_oracle.json"), "w") as f:
        json.dump(records, f)
    import collections

    print(
        f"{len(records)} jobs, {sum(r['edges'] for r in records)} edges, {sum(r['tables'] for r in records)} tables,",
        collections.Counter(r["outcome"] for r in records),
        f"jobs with int coords: {sum(1 for r in records if r['int_coords'])}",
        f"python {sum(r['python_s'] for r in records):.2f}s",
    )


if __name__ == "__main__":
    main()
