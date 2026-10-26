"""The Python twin of ``plumber_phases.rs``: times the phases of pdfplumber's
page layer, without any trace, in the oracle (CPython 3.12, after the
imports). For each file (``path<TAB>password`` lines on stdin):
``pdfplumber.open(path, pages=[1..20], password=...)`` and ``pdf.pages``,
then for each page ``page.layout`` (the interpretation and the device),
``page.objects`` (``process_object`` of each layout object),
``extract_text()``, ``extract_words()``, ``find_tables()``,
``extract_tables()`` (which finds the tables again: pdfplumber keeps no
table). Writes, per file, ``path pages open layout objects text words
tables extract_tables`` (seconds), the columns of the Rust example. An exception ends the phase it happens in
(the Rust example drops errors alike).

    .venv/bin/python crates/pdftrace/examples/plumber_phases.py < list.tsv
"""

from __future__ import annotations

import logging
import sys
import time

import pdfplumber


def main() -> None:
    logging.disable(logging.CRITICAL)
    sys.setrecursionlimit(1000)
    for line in sys.stdin:
        line = line.rstrip("\n")
        if not line or line.startswith("#"):
            continue
        path, _, password = line.partition("\t")
        t = [0.0] * 7
        started = time.perf_counter()
        try:
            pdf = pdfplumber.open(path, pages=list(range(1, 21)), password=password or None)
            pages = pdf.pages
        except Exception:
            print(f"{path}\t0\t{time.perf_counter() - started:.4f}\t0\t0\t0\t0\t0\t0", flush=True)
            continue
        t[0] = time.perf_counter() - started
        for p in pages:
            for i, f in enumerate(
                (
                    lambda: p.layout,
                    lambda: p.objects,
                    lambda: p.extract_text(),
                    lambda: p.extract_words(),
                    lambda: p.find_tables(),
                    lambda: p.extract_tables(),
                ),
                start=1,
            ):
                s = time.perf_counter()
                try:
                    f()
                except Exception:
                    pass
                t[i] += time.perf_counter() - s
        pdf.close()
        print(f"{path}\t{len(pages)}\t" + "\t".join(f"{x:.4f}" for x in t), flush=True)


if __name__ == "__main__":
    main()
