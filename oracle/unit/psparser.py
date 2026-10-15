"""Content-stream corpora and Python oracle for psparser + PDFContentParser.

A job is a list of
content streams, each given by a kind and bytes (see ``STREAM_KINDS``: a
plain stream of decoded data, or the cases where ``PDFContentParser.fillfp``
meets an unsupported filter, a pass-through filter or an object that is not
a stream). The oracle runs pdfminer's real ``PDFContentParser``
over each job and records the SHA-256 of its canonical serialization: one line
``<pos> <canon(obj)>`` per object yielded by ``nextobject()``, then the line of
the exception that ended the job (``!PSEOF("Unexpected EOF")`` normally), with
``oracle.canon.canon``. The Rust replay is crates/pdftrace/tests/psparser_unit.rs.

Modes (outputs in ``<cache>/unit/psparser/<fingerprint>/``, see
``oracle/unit/_common.py``):

- default: the real corpus. Decoded streams of the first ``--max-pages`` pages
  (and of the Form XObjects of their resources) of every PDF of the corpus set
  ``all`` (``oracle/corpus.py``: de-duplicated, opened with their known
  password). Writes ``corpus.bin`` and ``corpus.json``.
- ``--fuzz N``: N generated adversarial jobs (tokens across 4096-byte read
  blocks and stream boundaries, escapes, inline images, exceptions), then the
  fixed jobs of ``fixed_jobs`` (every raising fragment alone, every
  chunk-edge fragment on the edge, objects nested around the recursion
  limit of ``repr``/``str`` and 10^6 deep). Writes ``fuzz.bin`` and
  ``fuzz.json``.
- ``--dump INDEX [--corpus fuzz]``: prints the serialization of one job.

``*.bin``: little-endian ``b"PSC2" u32 njobs``, then per job ``u32 nstreams``
and per stream ``u8 kind``, ``u32 len`` + bytes. ``*.json``: ``{"sources",
"records": [{"id", "bytes", "objects", "outcome", "sha256", "depth"}]}``, where
``sources`` is the ``sources`` of the ``meta.json`` written with it (the Rust
replay checks that both corpora come from the current generator), and
``depth`` the recursion depth ``parse_job`` ran at (``_common.depth_here``).

Usage: PDFPLUMBER_ROOT=<main checkout> <root>/.venv/bin/python -m oracle.unit.psparser [--fuzz 4000]
"""

import argparse
import collections
import hashlib
import json
import multiprocessing
import random
import signal
import struct
import sys

from pdfminer.pdfinterp import PDFContentParser
from pdfminer.pdfpage import PDFPage
from pdfminer.pdftypes import PDFStream, resolve1, stream_value
from pdfminer.psparser import LIT

from ..canon import canon
from ..corpus import load_set
from ..stages._common import open_document
from ._common import depth_here, oracle_sources, require_oracle_python, unit_dir, write_meta

FILE_TIMEOUT_S = 120


def out_dir():
    """Where this generator's files go (``oracle/unit/_common.py``)."""
    return unit_dir("psparser")


# -- the job ------------------------------------------------------------------


# Stream kinds of a job (the Rust replay builds the same objects).
STREAM_KINDS = {
    0: lambda data: PDFStream({}, data),  # decoded content
    1: lambda data: PDFStream({"Filter": LIT("FooDecode")}, data),  # get_data raises PDFNotImplementedError
    2: lambda data: PDFStream({"Filter": LIT("DCTDecode")}, data),  # get_data returns the raw data
    3: lambda data: data,  # not a stream: stream_value gives PDFStream({}, b"")
    4: lambda data: PDFStream({"Filter": LIT("Crypt")}, data),  # get_data raises PDFNotImplementedError
}


def parse_job(streams):
    """Like PDFPageInterpreter.execute: the (pos, obj) pairs, the exception,
    and the recursion depth the parser was called at (``_common.depth_here``:
    the Rust replay starts from it, so that a ``RecursionError`` of ``repr``
    or ``str`` on a deep object happens at the same depth)."""
    out = []
    depth = depth_here()
    try:
        parser = PDFContentParser([STREAM_KINDS[kind](data) for kind, data in streams])
    except Exception as e:  # PSEOF for a job without streams
        return out, e, depth
    while True:
        try:
            out.append(parser.nextobject())
        except Exception as e:  # PSEOF at the end; pdfminer lets the others escape
            return out, e, depth


def serialize(objs, exc) -> bytes:
    lines = [f"{pos} {canon(obj)}" for pos, obj in objs]
    lines.append(canon(exc))
    return ("\n".join(lines) + "\n").encode("ascii")


def record(job_id, streams):
    objs, exc, depth = parse_job(streams)
    return {
        "id": job_id,
        "bytes": sum(len(data) for _, data in streams),
        "objects": len(objs),
        "outcome": type(exc).__name__,
        "sha256": hashlib.sha256(serialize(objs, exc)).hexdigest(),
        "depth": depth,
    }


# -- real corpus --------------------------------------------------------------


def collect_jobs(path: str, password: str | None, max_pages: int):
    """(job id, streams) for each page and Form XObject; a job that fails to
    load (decoding error...) is skipped, not the whole file."""
    with open_document(path, password) as doc:
        for pageno, page in enumerate(PDFPage.create_pages(doc), start=1):
            if pageno > max_pages:
                break
            try:
                contents = page.contents if isinstance(page.contents, list) else [page.contents]
                datas = [stream_value(c).get_data() for c in contents if c is not None]
            except Exception:
                datas = []
            if datas:
                yield f"{pageno}:page", [(0, d) for d in datas]
            try:
                xobjects = resolve1((page.resources or {}).get("XObject")) or {}
                items = list(xobjects.items()) if isinstance(xobjects, dict) else []
            except Exception:
                items = []
            for name, ref in items:
                try:
                    xobj = resolve1(ref)
                    if isinstance(xobj, PDFStream) and getattr(xobj.get("Subtype"), "name", None) == "Form":
                        yield f"{pageno}:form:{name}", [(0, xobj.get_data())]
                except Exception:
                    continue


class Timeout(Exception):
    pass


def on_alarm(signum, frame):
    raise Timeout()


def process_file(args):
    path, rel, password, max_pages = args
    signal.signal(signal.SIGALRM, on_alarm)
    signal.alarm(FILE_TIMEOUT_S)
    out = []
    try:
        for job_id, streams in collect_jobs(path, password, max_pages):
            out.append((record(f"{rel}#{job_id}", streams), streams))
        status = None
    except Timeout:
        status = "timeout"
    except Exception as e:  # unreadable document: not a parser case
        status = type(e).__name__
    finally:
        signal.alarm(0)
    return rel, status, out


def write_corpus(name: str, payload) -> None:
    with open(out_dir() / f"{name}.bin", "wb") as f:
        f.write(b"PSC2" + struct.pack("<I", len(payload)))
        for streams in payload:
            f.write(struct.pack("<I", len(streams)))
            for kind, data in streams:
                f.write(struct.pack("<BI", kind, len(data)) + data)


def read_corpus(name: str):
    data = (out_dir() / f"{name}.bin").read_bytes()
    assert data[:4] == b"PSC2"
    (n,) = struct.unpack_from("<I", data, 4)
    off, jobs = 8, []
    for _ in range(n):
        (ns,) = struct.unpack_from("<I", data, off)
        off += 4
        streams = []
        for _ in range(ns):
            kind, ln = struct.unpack_from("<BI", data, off)
            off += 5
            streams.append((kind, data[off : off + ln]))
            off += ln
        jobs.append(streams)
    return jobs


def write_oracle(name: str, records) -> None:
    with open(out_dir() / f"{name}.json", "w", encoding="ascii") as f:
        json.dump({"sources": oracle_sources(), "records": records}, f)
    write_meta("psparser")


# -- adversarial corpus ---------------------------------------------------------

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
    b"(\\377)", b"(\\0)", b"(a\\1234)", b"(\\37)", b"(\\3778)",
    b"(lone \\\rcr)", b"(\\\r)", b"(q \\q\nz)", b"(\\\r\r\n)",
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
    # inline image data ending with line breaks: re.sub(rb"(\x0d\x0a|[\x0d\x0a])$", b"", data)
    b"BI /W 1 ID x\n\nEI ", b"BI /W 1 ID y\r\n\nEI\n", b"BI ID \n\r\nEI ", b"BI ID z\r\r\nEI ", b"BI ID \n\rEI ",
    b"BI /F /A85 ID ab\n\n~>", b"BI /F /AHx ID 4\r\nEI ",
    # dicts whose keys go through literal_name(): str() of non-literals
    b"<<1.5 2 true 3 [/A (b) 0.1 1e-5] 4 /x#c2#a0 5 /y#ff 6>>", b"<<-0. 1 <a\'\"> 2 (q') 3>>",
]
# Fragments that make pdfminer raise (AssertionError, PSSyntaxError, IndexError,
# TypeError, KeyError); rare so most jobs run to the end of their streams.
FUZZ_ERRORS = [b"(bad oct \\777)", b"(\\400)", b"(\\4000)", b"(x\\401y)", b"<</A>>", b"BI /F [] ID x EI ", b"BI /F 5 ID x EI ", b"BI /F (s) ID x EI ",
               b"BI /F <<>> ID x EI ", b"1" * 4301, b"-" + b"0" * 4301, b"+" + b"9" * 4300,
               # repr() in the PSSyntaxError message, and other subscript errors
               b"<</x#c2#a0 (it's\"q) 1.5 /bad#ff -0. true /k 0.1 [/caf\xc3\xa9]>>",
               b"BI /F Tj ID x EI ", b"BI /F true ID x EI ", b"BI /F 1.5 ID x EI ", b"BI /F () ID x EI "]
# Fragments whose byte at `offset` should land exactly on a 4096-byte read-chunk edge.
FUZZ_EDGE = [(b"(cont \\\r\nline)", 7), (b"(a\\101)", 3), (b"/N#41x", 2), (b"<4142>", 0), (b"BI /W 1 ID x E\nEI\r\n", 13),
             (b"(e\\\rx)", 3), (b"(q\\q\nz)", 3), (b"(a\\377)", 5), (b"(a\\40)", 4)]
# Chunk-edge fragments that raise (an octal escape of 256 split across the
# edge): only in the fixed jobs, so that random jobs run to their end.
FUZZ_EDGE_ERRORS = [(b"(a\\400)", 3), (b"(a\\400)", 4), (b"(a\\400)", 5)]
FUZZ_SEPARATORS = [b" ", b" ", b"\n", b"\r\n", b"\r", b"\t", b"\x0c", b"\x0b", b"", b"  \n "]
# Fragments that swallow the rest of a job: an unclosed string, comment,
# array or procedure (whose objects never reach nextobject's caller), or an
# inline image without its end. Half of the jobs leave them out, so that
# most of their tokens come out of the parser.
FUZZ_SWALLOWING = {b"(unbalanced (", b"%eof-comment", b"[", b"{", b"<<", b"BI /W 1 ID nothing-after"}
FUZZ_FRAGMENTS_CLEAN = [f for f in FUZZ_FRAGMENTS if f not in FUZZ_SWALLOWING]


def fuzz_job(rng):
    parts = []
    fragments = FUZZ_FRAGMENTS if rng.random() < 0.5 else FUZZ_FRAGMENTS_CLEAN
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
            frag = rng.choice(fragments)
        parts.append(frag + rng.choice(FUZZ_SEPARATORS))
        size += len(parts[-1])
    data = b"".join(parts)
    cuts = sorted(rng.sample(range(1, len(data)), k=min(len(data) - 1, rng.choice([0, 0, 1, 2, 3]))))
    streams = [(0, data[a:b]) for a, b in zip([0] + cuts, cuts + [len(data)])]
    if rng.random() < 0.1:
        streams.insert(rng.randint(0, len(streams)), (0, b""))
    if rng.random() < 0.05:
        # the same data through a filter that returns it unchanged
        k = rng.randrange(len(streams))
        streams[k] = (2, streams[k][1])
    if rng.random() < 0.1:
        # a stream that fails to decode, or an object that is not a stream,
        # often right after a token cut by a stream boundary
        kind = rng.choice([1, 3, 4])
        streams.insert(rng.randint(1, len(streams)), (kind, rng.choice(FUZZ_FRAGMENTS)))
    return streams


def at_chunk_edge(fragment, offset, shift=0):
    """``fragment`` placed so that its byte ``offset`` is the last byte of the
    first 4096-byte read (``shift`` bytes later)."""
    gap = 4096 - offset - 1 + shift
    return b"%" + b"y" * (gap - 2) + b"\n" + fragment


def fixed_jobs(c_left):
    """Jobs every corpus gets, after the random ones: each fragment that makes
    pdfminer raise, alone; each chunk-edge fragment exactly on the edge; and
    objects nested around the depth where ``repr``/``str`` run out of C
    recursion units (``c_left`` at ``parse_job``), and far deeper."""
    jobs = [[(0, frag)] for frag in FUZZ_ERRORS]
    for frag, offset in FUZZ_EDGE + FUZZ_EDGE_ERRORS:
        jobs += [[(0, at_chunk_edge(frag, offset, shift))] for shift in (-1, 0, 1)]
    deep = lambda n, leaf=b"": b"[" * n + leaf + b"]" * n  # noqa: E731
    jobs.append([(0, deep(10**6) + b" Q")])
    jobs.append([(0, b"{" * 300000 + b"}" * 300000 + b" Q")])
    jobs.append([(0, b"<< /A " + deep(200000) + b" /B >> Q")])
    for n in range(c_left - 6, c_left + 2):
        jobs.append([(0, b"<< " + deep(n) + b" 1 >> Q")])  # str() of a key
        jobs.append([(0, b"<< " + deep(n, b"/F") + b" 1 >> Q")])
        jobs.append([(0, b"<< " + deep(n, b"<a1>") + b" 1 >> Q")])
        jobs.append([(0, b"<< /A " + deep(n) + b" /B >> Q")])  # repr() in the PSSyntaxError message
        jobs.append([(0, b"<< " + b"<< /K " * n + b"1" + b" >>" * n + b" 1 >> Q")])
        jobs.append([(0, b"BI /A " + deep(n) + b" ID x EI Q")])  # repr() in the swallowed PSTypeError
        jobs.append([(0, b"BI " + deep(n, b"1.5") + b" 1 ID x EI Q")])
    return jobs


# -- main -----------------------------------------------------------------------


def main() -> None:
    ap = argparse.ArgumentParser(prog="python -m oracle.unit.psparser")
    ap.add_argument("--max-pages", type=int, default=20)
    ap.add_argument("--fuzz", type=int, help="generate N adversarial jobs instead of the real corpus")
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--dump", type=int, help="print the serialization of one job")
    ap.add_argument("--corpus", default="corpus", choices=["corpus", "fuzz"])
    ap.add_argument("-j", "--jobs", type=int, default=3, help="worker processes for the real corpus")
    args = ap.parse_args()
    require_oracle_python()

    if args.dump is not None:
        jobs = read_corpus(args.corpus)
        objs, exc, _ = parse_job(jobs[args.dump])
        sys.stdout.buffer.write(serialize(objs, exc))
        return

    if args.fuzz:
        rng = random.Random(args.seed)
        records, payload = [], []
        for k in range(args.fuzz):
            streams = fuzz_job(rng)
            records.append(record(f"fuzz#{k}", streams))
            payload.append(streams)
        # parse_job is called from here through record(): Python-to-Python
        # calls, so with the C units left here
        for k, streams in enumerate(fixed_jobs(depth_here()["c_left"])):
            records.append(record(f"fixed#{k}", streams))
            payload.append(streams)
        write_corpus("fuzz", payload)
        write_oracle("fuzz", records)
        print(len(records), "fuzz jobs", dict(collections.Counter(r["outcome"] for r in records)))
        return

    files = load_set("all")
    tasks = [(str(f.path), f.rel, f.password, args.max_pages) for f in files]
    records, payload, skipped = [], [], collections.Counter()
    with multiprocessing.Pool(args.jobs) as pool:
        for rel, status, out in pool.imap(process_file, tasks, chunksize=4):
            if status is not None:
                skipped[status] += 1
                print(f"skip {rel}: {status}", file=sys.stderr)
            for rec, streams in out:
                records.append(rec)
                payload.append(streams)
    write_corpus("corpus", payload)
    write_oracle("corpus", records)
    total = sum(r["bytes"] for r in records)
    print(f"{len(records)} jobs from {len(files)} files ({dict(skipped)} skipped), {total / 1e6:.1f} MB,",
          dict(collections.Counter(r["outcome"] for r in records)))


if __name__ == "__main__":
    main()
