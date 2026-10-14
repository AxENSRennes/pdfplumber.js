"""Write the Python trace of one stage for one PDF on stdout.

    python -m oracle.trace --stage S [--password P] [--max-pages N] FILE.pdf

Same contract as ``pdftrace --stage S [--password P] [--max-pages N] FILE.pdf``
(docs/verification.md). ``--max-pages`` defaults to 20; 0 means every page.
The output is ASCII, one ``\\n`` after each line, ending with ``!end`` or with
the exception line that stopped the stage. Exit status: 0 when a trace was
written (whatever it ends with), 1 on a harness error, 2 on bad usage.
"""

from __future__ import annotations

import argparse
import sys

from .stages import DEFAULT_MAX_PAGES, available_stages, trace_text


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(prog="python -m oracle.trace", description=__doc__.split("\n\n")[0])
    ap.add_argument("--stage", required=True, choices=available_stages())
    ap.add_argument("--password", default=None)
    ap.add_argument("--max-pages", type=int, default=DEFAULT_MAX_PAGES, help="pages per document (0 = all)")
    ap.add_argument("file")
    args = ap.parse_args(argv)
    text = trace_text(args.stage, args.file, args.password, args.max_pages)
    out = sys.stdout.buffer
    out.write(text.encode("ascii"))
    out.flush()
    return 0


if __name__ == "__main__":
    sys.exit(main())
