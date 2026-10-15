"""Differential cases for pdfminer/ccitt.py on the real corpus.

Every ``ccittfaxdecode`` call made by pdfminer while decoding
(``PDFStream.get_data``) the streams of the corpus set ``all`` whose filters
include ``/CCITTFaxDecode``, and the inline images of the content streams
that name the filter, with the data and the parameters pdfminer passes (one
case per distinct pair). The generated and adversarial cases are written by
``oracle/unit/ccitt.py``; this generator is separate because it takes
minutes. The Rust replay is the ``corpus`` test of
crates/pdftrace/tests/ccitt_unit.rs.

Writes ``corpus.jsonl``: ``[source, data, params]`` -> ``output``, the
result bytes when at most 512 long, else ``("sha256", len, hex digest)``, or
the exception (as ``oracle/unit/ccitt.py``). A call that runs more than
``CALL_TIMEOUT_S`` seconds in Python, and a file whose streams take more
than ``FILE_TIMEOUT_S`` to read, are left out (listed in ``meta.json``).

Usage: PDFPLUMBER_ROOT=<main checkout> <root>/.venv/bin/python -m oracle.unit.ccitt_corpus [-j 3]
"""

from __future__ import annotations

import argparse
import contextlib
import hashlib
import multiprocessing
import re
import signal
import sys

import pdfminer.pdftypes
from pdfminer.ccitt import ccittfaxdecode
from pdfminer.pdfinterp import PDFContentParser
from pdfminer.pdftypes import LITERALS_CCITTFAX_DECODE, PDFStream
from pdfminer.psparser import LIT

from ..corpus import load_set
from ..stages._common import open_document
from ._common import require_oracle_python, write_cases, write_meta

MODULE = "ccitt_corpus"
CALL_TIMEOUT_S = 300
FILE_TIMEOUT_S = 900


def output(f, *args):
    """``f(*args)`` as a case result: bytes (hashed beyond 512 bytes) or the
    exception (``output`` of ``oracle/unit/ccitt.py``)."""
    try:
        b = f(*args)
    except Exception as e:  # the exception is the result
        return e
    if len(b) <= 512:
        return b
    return ("sha256", len(b), hashlib.sha256(b).hexdigest())


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
    ap = argparse.ArgumentParser(prog="python -m oracle.unit.ccitt_corpus")
    ap.add_argument("-j", "--jobs", type=int, default=3, help="worker processes")
    args = ap.parse_args()
    require_oracle_python()
    skipped = []
    n = write_cases(MODULE, "corpus", gen_corpus(args.jobs, skipped))
    write_meta(MODULE, cases=n, skipped=skipped)
    print(f"wrote {n} cases ({len(skipped)} skipped)")


if __name__ == "__main__":
    main()
