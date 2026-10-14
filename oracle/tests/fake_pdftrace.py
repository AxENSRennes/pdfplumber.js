"""A stand-in for the Rust ``pdftrace`` binary, to test ``oracle/verify.py``.

Same command line as the real binary::

    fake_pdftrace.py --stage S [--password P] [--max-pages N] --out DIR F1.pdf ...

It writes the oracle's own trace to ``DIR/<sha256>.txt``, then misbehaves as
``FAKE_PDFTRACE_MODE`` says, for the files whose name contains
``FAKE_PDFTRACE_TARGET`` (every file if unset):

- ``copy`` (default): nothing, the outputs are exact;
- ``perturb``: line ``FAKE_PDFTRACE_LINE`` (1-based, default 2) gets `` !perturbed``
  appended;
- ``truncate``: the trace loses its last line;
- ``crash``: the process dies (``os._exit(101)``) when it reaches the file;
- ``unimplemented``: exit status 2 before doing anything.
"""

from __future__ import annotations

import argparse
import hashlib
import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))

from oracle.stages import trace_text  # noqa: E402


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--stage", required=True)
    ap.add_argument("--password")
    ap.add_argument("--max-pages", type=int, default=20)
    ap.add_argument("--out", required=True)
    ap.add_argument("files", nargs="+")
    args = ap.parse_args()
    mode = os.environ.get("FAKE_PDFTRACE_MODE", "copy")
    target = os.environ.get("FAKE_PDFTRACE_TARGET", "")
    if mode == "unimplemented":
        print(f"pdftrace: stage {args.stage} is not implemented", file=sys.stderr)
        return 2
    for name in args.files:
        hit = target in Path(name).name
        if mode == "crash" and hit:
            print(f"pdftrace: crashing on {name}", file=sys.stderr)
            sys.stderr.flush()
            os._exit(101)
        data = Path(name).read_bytes()
        lines = trace_text(args.stage, name, args.password, args.max_pages).split("\n")[:-1]
        if mode == "perturb" and hit:
            k = int(os.environ.get("FAKE_PDFTRACE_LINE", "2")) - 1
            lines[k] += " !perturbed"
        if mode == "truncate" and hit:
            lines = lines[:-1]
        out = Path(args.out) / f"{hashlib.sha256(data).hexdigest()}.txt"
        out.write_bytes("".join(x + "\n" for x in lines).encode("ascii"))
    return 0


if __name__ == "__main__":
    sys.exit(main())
