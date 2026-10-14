"""Build the content-stream corpus and the Python oracle for the psparser benchmark.

For every selected page we collect the decoded page content streams (one job) and
each Form XObject referenced from the page resources (one job each). The oracle
runs pdfminer's real ``PDFContentParser`` over each job and records:

- a SHA-256 of a canonical serialization of every ``(pos, obj)`` it yields,
- the number of objects,
- the CPython time spent in ``nextobject()`` (best of N runs).

Outputs (in ``out/``):
- ``corpus.bin``: little-endian ``b"PSC1" u32 njobs``, then per job
  ``u32 nstreams`` and per stream ``u32 len`` + bytes.
- ``oracle.json``: per-job id, hash, object count, outcome, python timing.

Usage: python dump_corpus.py [--max-pages N] | --fuzz N [--seed S] | --dump JOB_INDEX [--corpus fuzz]
"""

import argparse
import glob
import hashlib
import json
import os
import struct
import sys
import time

from pdfminer.pdfdocument import PDFDocument
from pdfminer.pdfinterp import PDFContentParser
from pdfminer.pdfpage import PDFPage
from pdfminer.pdfparser import PDFParser
from pdfminer.pdftypes import PDFStream, resolve1, stream_value
from pdfminer.psparser import PSEOF, PSKeyword, PSLiteral

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
OUT = os.path.join(HERE, "out")

SOURCES = [
    "test/fixtures/external-pdfs/*.pdf",
    "test/fixtures/external-holdout-pdfs/*.pdf",
    "pdfplumber-python/tests/pdfs/*.pdf",
    "pdfjs/test/pdfs/*.pdf",
]


def canon(obj) -> str:
    """Canonical, type-tagged serialization shared with the TS and Rust ports.

    Numbers: integer-valued numbers that are safe in a JS double are written as
    ``d<decimal>`` whether Python saw an int or a float (JS has one number type);
    Python ints outside that range stay exact (``d``), other floats are written as
    raw IEEE-754 bits (``f<hex>``).
    """
    if obj is True:
        return "T"
    if obj is False:
        return "F"
    if isinstance(obj, int):
        return f"d{obj}"
    if isinstance(obj, float):
        if obj.is_integer() and abs(obj) <= 2**53 - 1:
            return f"d{int(obj)}"
        return "f" + struct.pack(">d", obj).hex()
    if isinstance(obj, bytes):
        return "s" + obj.hex()
    if isinstance(obj, PSLiteral):
        if isinstance(obj.name, str):
            return "n" + obj.name.encode("utf-8").hex()
        return "N" + obj.name.hex()
    if isinstance(obj, PSKeyword):
        return "k" + obj.name.hex()
    if isinstance(obj, list):
        return "[" + ",".join(canon(x) for x in obj) + "]"
    if isinstance(obj, dict):
        return "{" + ",".join(k.encode("utf-8").hex() + "=" + canon(v) for k, v in obj.items()) + "}"
    if isinstance(obj, PDFStream):
        return "S" + canon(obj.attrs) + "|" + obj.rawdata.hex()
    raise TypeError(f"unexpected object {type(obj)!r}")


def parse_job(streams):
    """Yield (pos, obj) like PDFPageInterpreter.execute does, then the outcome."""
    out = []
    try:
        parser = PDFContentParser([PDFStream({}, s) for s in streams])
    except PSEOF:
        return out, "eof"
    while True:
        try:
            out.append(parser.nextobject())
        except PSEOF:
            return out, "eof"
        except Exception as e:  # pdfminer lets these escape to the caller
            return out, "error:" + type(e).__name__


def time_job(streams, repeat: int) -> float:
    best = float("inf")
    for _ in range(repeat):
        t0 = time.perf_counter()
        parse_job(streams)
        best = min(best, time.perf_counter() - t0)
    return best


def serialize(objs, outcome: str) -> bytes:
    lines = [f"{pos}:{canon(obj)}" for pos, obj in objs]
    lines.append("!" + outcome)
    return ("\n".join(lines) + "\n").encode("ascii")


def collect_jobs(path: str, max_pages: int):
    with open(path, "rb") as fp:
        doc = PDFDocument(PDFParser(fp))
        for pageno, page in enumerate(PDFPage.create_pages(doc), start=1):
            if pageno > max_pages:
                break
            contents = page.contents if isinstance(page.contents, list) else [page.contents]
            datas = [stream_value(c).get_data() for c in contents if c is not None]
            if datas:
                yield f"{pageno}:page", datas
            xobjects = resolve1((page.resources or {}).get("XObject")) or {}
            if isinstance(xobjects, dict):
                for name, ref in xobjects.items():
                    xobj = resolve1(ref)
                    if isinstance(xobj, PDFStream) and getattr(xobj.get("Subtype"), "name", None) == "Form":
                        yield f"{pageno}:form:{name}", [xobj.get_data()]


FUZZ_FRAGMENTS = [
    # numbers, including forms Python's int()/float() reject or round specially
    b"0", b"-0", b"+12", b"007", b"-", b"+", b".", b"-.", b"+.5", b"5.", b"1.2.3", b"--5", b"-.5e3",
    b"123456789012345678", b"9007199254740993", b"0.1000000000000000055511151231257827",
    b"3.14159265358979323846264338327950288", b"1" * 40 + b".5", b"1" * 330 + b".", b"-" + b"9" * 30,
    # names
    b"/F", b"/A85", b"/ASCII85Decode", b"/Name#20With#41", b"/Bad#G1", b"/Half#4", b"/#", b"/",
    b"/caf\xc3\xa9", b"/bad\xff\xfe", b"/a#e9", b"//double",
    # strings and escapes
    b"(plain)", b"(nested (paren) ok)", b"(unbalanced (", b"(esc \\n\\t\\b\\f\\r\\( \\) \\\\)", b"(oct \\101\\7\\0123)",
    b"(cont \\\r\nline)", b"(cont \\\nline)", b"(unknown \\q)", b"(cr\rlf\n)", b")",
    # hex strings and dict delimiters
    b"<48656c6c6f>", b"<4 8 6>", b"<ABC>", b"<zz>", b"<<", b">>", b">", b"<", b"<</A 1>>",
    b"<</A 1 /B [1 2 <</C (x)>>]>>", b"<<1 2>>", b"<<(k) 1>>", b"<<[1] 2>>",
    # arrays, procs, keywords
    b"[", b"]", b"[1 2 [3]]", b"{", b"}", b"{1 {2} }", b"true", b"false", b"trueish", b"BT", b"ET", b"Tj", b"TJ",
    b"T*", b"'", b'"', b"d0", b"q", b"Q", b"re", b"f*", b"\x00", b"\x80", b"\xff", b"#", b"%comment\n", b"%eof-comment",
    # inline images
    b"BI /W 2 /H 2 /BPC 8 /CS /G ID \x01\x02EI\x03\x04 EI Q",
    b"BI /F /A85 ID abc~>xyz EI",
    b"BI /F [/A85] ID 9jqo^~> ",
    b"BI /W 1 ID EEI \r\nEI\n", b"BI /W ID x EI ", b"BI ID \r\n\n EI ", b"BI /W 1 ID nothing-after",
    b"[BI /W 1 ID x EI ]", b"ID", b"EI",
]
# Fragments that make pdfminer raise (AssertionError, PSSyntaxError, IndexError,
# TypeError, KeyError); rare so most jobs run to the end of their streams.
FUZZ_ERRORS = [b"(bad oct \\777)", b"<</A>>", b"BI /F [] ID x EI ", b"BI /F 5 ID x EI ", b"BI /F (s) ID x EI ", b"BI /F <<>> ID x EI ", b"1" * 4301]
# Fragments whose byte at `offset` should land exactly on a 4096-byte read-chunk edge.
FUZZ_EDGE = [(b"(cont \\\r\nline)", 7), (b"(a\\101)", 3), (b"/N#41x", 2), (b"<4142>", 0), (b"BI /W 1 ID x E\nEI\r\n", 13)]
FUZZ_SEPARATORS = [b" ", b" ", b"\n", b"\r\n", b"\r", b"\t", b"\x0c", b"\x0b", b"", b"  \n "]


def fuzz_job(rng):
    parts = []
    target = rng.choice([200, 2000, 4090, 4096, 4100, 8200, 12300])
    size = 0
    while size < target:
        if rng.random() < 0.05:
            frag, offset = rng.choice(FUZZ_EDGE)
            gap = (4096 - size % 4096) - offset - 1 + rng.choice([-1, 0, 0, 1])
            frag = (b"%" + b"y" * (gap - 2) + b"\n" if gap > 2 else b"") + frag
        elif rng.random() < 0.05:
            # filler that pushes the next fragment onto a 4096-byte chunk edge
            gap = (4096 - size % 4096) - rng.randint(0, 4)
            if gap > 2:
                frag = b"%" + b"x" * (gap - 2) + b"\n"
            else:
                frag = b" "
        elif rng.random() < 0.002:
            frag = rng.choice(FUZZ_ERRORS)
        else:
            frag = rng.choice(FUZZ_FRAGMENTS)
        parts.append(frag + rng.choice(FUZZ_SEPARATORS))
        size += len(parts[-1])
    data = b"".join(parts)
    cuts = sorted(rng.sample(range(1, len(data)), k=min(len(data) - 1, rng.choice([0, 0, 1, 2, 3]))))
    streams = [data[a:b] for a, b in zip([0] + cuts, cuts + [len(data)])]
    if rng.random() < 0.1:
        streams.insert(rng.randint(0, len(streams)), b"")
    return streams


def write_corpus(name: str, payload) -> None:
    with open(os.path.join(OUT, name + ".bin"), "wb") as f:
        f.write(b"PSC1" + struct.pack("<I", len(payload)))
        for streams in payload:
            f.write(struct.pack("<I", len(streams)))
            for s in streams:
                f.write(struct.pack("<I", len(s)) + s)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--max-pages", type=int, default=20)
    ap.add_argument("--repeat", type=int, default=3)
    ap.add_argument("--fuzz", type=int, help="generate N adversarial jobs instead of the real corpus")
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--dump", type=int, help="print the serialization of one job")
    ap.add_argument("--corpus", default="corpus")
    args = ap.parse_args()
    os.makedirs(OUT, exist_ok=True)

    if args.dump is not None:
        jobs = read_corpus(args.corpus)
        objs, outcome = parse_job(jobs[args.dump])
        sys.stdout.buffer.write(serialize(objs, outcome))
        return

    import logging

    logging.disable(logging.CRITICAL)  # malformed files are noisy; parsing is unchanged

    if args.fuzz:
        import random

        rng = random.Random(args.seed)
        records, payload = [], []
        for k in range(args.fuzz):
            streams = fuzz_job(rng)
            objs, outcome = parse_job(streams)
            records.append(
                {
                    "id": f"fuzz#{k}",
                    "bytes": sum(len(s) for s in streams),
                    "objects": len(objs),
                    "outcome": outcome,
                    "sha256": hashlib.sha256(serialize(objs, outcome)).hexdigest(),
                }
            )
            payload.append(streams)
        write_corpus("fuzz", payload)
        with open(os.path.join(OUT, "fuzz_oracle.json"), "w") as f:
            json.dump(records, f)
        import collections

        print(len(records), "fuzz jobs", collections.Counter(r["outcome"] for r in records))
        return

    files = sorted(f for pattern in SOURCES for f in glob.glob(os.path.join(ROOT, pattern)))
    records, payload = [], []
    for path in files:
        rel = os.path.relpath(path, ROOT)
        try:
            jobs = list(collect_jobs(path, args.max_pages))
        except Exception as e:  # unreadable document: not a parser benchmark case
            print(f"skip {rel}: {type(e).__name__}", file=sys.stderr)
            continue
        for job_id, streams in jobs:
            objs, outcome = parse_job(streams)
            records.append(
                {
                    "id": f"{rel}#{job_id}",
                    "bytes": sum(len(s) for s in streams),
                    "objects": len(objs),
                    "outcome": outcome,
                    "sha256": hashlib.sha256(serialize(objs, outcome)).hexdigest(),
                    "python_s": time_job(streams, args.repeat),
                }
            )
            payload.append(streams)

    write_corpus("corpus", payload)
    with open(os.path.join(OUT, "oracle.json"), "w") as f:
        json.dump(records, f)
    total = sum(r["bytes"] for r in records)
    py = sum(r["python_s"] for r in records)
    print(f"{len(records)} jobs from {len(files)} files, {total / 1e6:.1f} MB, python {py:.2f}s ({total / 1e6 / py:.2f} MB/s)")


def read_corpus(name: str = "corpus"):
    with open(os.path.join(OUT, name + ".bin"), "rb") as f:
        data = f.read()
    assert data[:4] == b"PSC1"
    (n,) = struct.unpack_from("<I", data, 4)
    off, jobs = 8, []
    for _ in range(n):
        (ns,) = struct.unpack_from("<I", data, off)
        off += 4
        streams = []
        for _ in range(ns):
            (ln,) = struct.unpack_from("<I", data, off)
            off += 4
            streams.append(data[off : off + ln])
            off += ln
        jobs.append(streams)
    return jobs


if __name__ == "__main__":
    main()
