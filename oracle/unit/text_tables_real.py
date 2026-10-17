"""Real-PDF cases for the text and table modules of the port: the page
operations of ``_plumber_ops.py`` (``REAL_OPS`` and explicit-lines tables
built from each page) on the first ``--max-pages`` pages of every PDF of the
``quick`` set, and on all pages (up to ``SAMPLE_PAGES``) of ``SAMPLE_DOCS``,
table- and text-heavy documents. Writes ``pages_real.jsonl`` (one page per
line) to ``<cache>/unit/text_tables_real/<fingerprint>/``; the Rust replay is
crates/pdftrace/tests/text_tables_unit.rs (``pages_real``). Pages whose
chars hold lone surrogates are cases like the others (``pycompat::PyStr``).

Usage: PDFPLUMBER_ROOT=<main checkout> <root>/.venv/bin/python -m oracle.unit.text_tables_real [--max-pages 2] [-j 3]
"""

from __future__ import annotations

import argparse
import multiprocessing
import signal
import sys

import pdfplumber

from ..corpus import REPO_DIR, corpus_root, load_set
from ._common import require_oracle_python, write_cases, write_meta
from ._plumber_ops import page_case

MODULE = "text_tables_real"
FILE_TIMEOUT_S = 600
SAMPLE_PAGES = 6
SAMPLE_DOCS = [
    # table-heavy
    "upstream/pdfplumber-python/tests/pdfs/nics-background-checks-2015-11.pdf",
    "upstream/pdfplumber-python/tests/pdfs/150109DSP-Milw-505-90D.pdf",
    "upstream/pdfplumber-python/tests/pdfs/senate-expenditures.pdf",
    "upstream/pdfplumber-python/tests/pdfs/WARN-Report-for-7-1-2015-to-03-25-2016.pdf",
    "upstream/pdfplumber-python/tests/pdfs/pdffill-demo.pdf",
    "upstream/pdfplumber-python/tests/pdfs/table-curves-example.pdf",
    "upstream/pdfplumber-python/tests/pdfs/issue-140-example.pdf",
    "upstream/pdfplumber-python/tests/pdfs/cupertino_usd_4-6-16.pdf",
    "upstream/pdfplumber-python/tests/pdfs/chelsea_pdta.pdf",
    "upstream/pdfplumber-python/tests/pdfs/2023-06-20-PV.pdf",
    "upstream/pdfplumber-python/tests/pdfs/issue-466-example.pdf",
    "upstream/pdfplumber-python/tests/pdfs/nics-background-checks-2015-11-rotated.pdf",
    # text-heavy
    "upstream/pdfplumber-python/tests/pdfs/federal-register-2020-17221.pdf",
    "upstream/pdfplumber-python/tests/pdfs/scotus-transcript-p1.pdf",
    "upstream/pdfplumber-python/tests/pdfs/issue-33-lorem-ipsum.pdf",
    "upstream/pdfplumber-python/tests/pdfs/issue-71-duplicate-chars.pdf",
    "upstream/pdfplumber-python/tests/pdfs/issue-1114-dedupe-chars.pdf",
    "upstream/pdfplumber-python/tests/pdfs/extra-attrs-example.pdf",
    "upstream/pdfplumber-python/tests/pdfs/line-char-render-example.pdf",
    "upstream/pdfplumber-python/tests/pdfs/issue-1147-example.pdf",
    "corpus/external/arxiv-publaynet-1908.07836.pdf",
    "corpus/external/irs-f1040.pdf",
    "corpus/external/federal-register-2024-01-02.pdf",
]
#: Generated PDFs of this checkout (``oracle/generate_micro_pdfs.py``), all
#: pages: chars whose text is a lone surrogate (``chr(cid)``).
LOCAL_DOCS = ["corpus/micro/lone-surrogates.pdf"]


# Operations on every real page.
REAL_OPS = [
    ("text", {}),
    ("text", {"layout": True}),
    ("text", {"layout": True, "x_density": 5, "y_density": 10.5}),
    ("text", {"x_tolerance": 1.5, "y_tolerance": 1}),
    ("text", {"x_tolerance_ratio": 0.15, "keep_blank_chars": True}),
    ("text", {"use_text_flow": True}),
    ("text", {"line_dir_render": "btt", "char_dir_render": "rtl"}),
    ("text", {"line_dir": "ltr", "char_dir": "ttb", "layout": True}),
    ("utext", {}),
    ("utext", {"layout": True, "x_shift": 3, "y_shift": -2}),
    ("simple", {}),
    ("simple", {"x_tolerance": 1, "y_tolerance": 0}),
    ("words", {}),
    ("words", {"return_chars": True, "extra_attrs": ("fontname", "size")}),
    ("words", {"split_at_punctuation": True, "keep_blank_chars": True}),
    ("words", {"horizontal_ltr": False, "vertical_ttb": False, "expand_ligatures": False}),
    ("words", {"x_tolerance_ratio": 0.3, "y_tolerance_ratio": 0.5, "use_text_flow": True}),
    ("lines", True, True, {}),
    ("lines", False, False, {"layout": True}),
    ("search", {}, r"\w+", True, True, 0, True, False),
    ("search", {"layout": True}, r"(\d+)[.,](\d+)", True, True, 1, True, True),
    ("search", {}, "THE", False, False, 0, False, False),
    ("dedupe", {"tolerance": 2, "extra_attrs": None}),
    ("tables", {}),
    ("tables", {"vertical_strategy": "lines_strict", "horizontal_strategy": "lines_strict"}),
    ("tables", {"vertical_strategy": "text", "horizontal_strategy": "text"}),
    ("tables", {"vertical_strategy": "text", "horizontal_strategy": "lines", "snap_tolerance": 1, "join_x_tolerance": 5}),
    ("tables", {"intersection_tolerance": 5, "edge_min_length": 10, "text_x_tolerance": 2, "min_words_vertical": 2}),
    ("extract_tables", {}),
    ("extract_tables", {"vertical_strategy": "text", "horizontal_strategy": "text", "text_layout": True}),
    ("extract_tables", {"text_x_tolerance": 1, "text_keep_blank_chars": True, "snap_y_tolerance": 0}),
    ("extract_table", {}),
    ("extract_table", {"vertical_strategy": "text", "text_use_text_flow": True}),
]


class Timeout(Exception):
    pass


def on_alarm(signum, frame):
    raise Timeout()


def page_record(page):
    return {"chars": page.chars, "edges": page.edges, "bbox": page.bbox}


def explicit_ops(page):
    """Explicit-lines ops built from the page's own objects and bbox."""
    x0, top, x1, bottom = page.bbox
    w, h = x1 - x0, bottom - top
    v_lines = [x0 + w * k / 4 for k in range(5)]
    h_lines = [top + h * k / 6 for k in range(7)]
    objs = (page.objects.get("rect", []) + page.objects.get("line", []) + page.objects.get("curve", []))[:6]
    return [
        ("tables", {"vertical_strategy": "explicit", "horizontal_strategy": "explicit", "explicit_vertical_lines": v_lines, "explicit_horizontal_lines": h_lines}),
        ("extract_tables", {"vertical_strategy": "explicit", "explicit_vertical_lines": v_lines + objs, "horizontal_strategy": "lines", "explicit_horizontal_lines": objs}),
    ]


def process_file(args):
    path, rel, password, max_pages = args
    signal.signal(signal.SIGALRM, on_alarm)
    signal.alarm(FILE_TIMEOUT_S)
    out, skipped = [], []
    try:
        with pdfplumber.open(path, password=password or "") as pdf:
            for pageno, page in enumerate(pdf.pages, start=1):
                if pageno > max_pages:
                    break
                try:
                    record = page_record(page)
                except Exception as e:  # unreadable page: not a text/table case
                    skipped.append(f"{rel}#{pageno}: {type(e).__name__}")
                    continue
                out.append(page_case(page, record, REAL_OPS + explicit_ops(page)))
                page.close()
        status = None
    except Timeout:
        status = "timeout"
    except Exception as e:
        status = type(e).__name__
    finally:
        signal.alarm(0)
    return rel, status, out, skipped


def real_cases(max_pages: int, workers: int, meta: dict):
    root = corpus_root()
    jobs = [(str(f.path), f.rel, f.password, max_pages) for f in load_set("quick")]
    seen = {str(f.path) for f in load_set("quick")}
    for rel in SAMPLE_DOCS:
        p = root / rel
        if p.is_file() and str(p) not in seen:
            jobs.append((str(p), rel, None, SAMPLE_PAGES))
        elif p.is_file():
            jobs = [(j[0], j[1], j[2], SAMPLE_PAGES) if j[0] == str(p) else j for j in jobs]
    for rel in LOCAL_DOCS:
        p = REPO_DIR / rel
        if str(p) not in seen:
            jobs.append((str(p), rel, None, SAMPLE_PAGES))
    statuses, skipped = {}, []
    with multiprocessing.Pool(workers, maxtasksperchild=8) as pool:
        for rel, status, cases, sk in pool.imap(process_file, jobs):
            if status:
                statuses[rel] = status
            skipped.extend(sk)
            yield from cases
    meta["real_files"] = len(jobs)
    meta["real_file_errors"] = statuses
    meta["real_skipped_pages"] = skipped



def main() -> int:
    ap = argparse.ArgumentParser(prog="python -m oracle.unit.text_tables_real", description=__doc__.split("\n\n")[0])
    ap.add_argument("--max-pages", type=int, default=2, help="pages per PDF of the quick set (default 2)")
    ap.add_argument("-j", "--workers", type=int, default=3)
    args = ap.parse_args()
    require_oracle_python()
    meta: dict = {}
    n = write_cases(MODULE, "pages_real", real_cases(args.max_pages, args.workers, meta))
    write_meta(MODULE, pages=n, max_pages=args.max_pages, **meta)
    print(n, meta.get("real_file_errors"), len(meta.get("real_skipped_pages", [])), flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
