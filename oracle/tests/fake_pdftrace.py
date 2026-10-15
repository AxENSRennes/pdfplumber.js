"""A stand-in for the Rust ``pdftrace`` binary, to test ``oracle/verify.py``.

Same command line as the real binary::

    fake_pdftrace.py --stage S [--password P] [--max-pages N] --out DIR F1.pdf ...
    fake_pdftrace.py --list-stages

It writes the oracle's own trace to ``DIR/<sha256>.txt`` and appends
``<sha256>\t<seconds>`` to ``DIR/_times.tsv``, like the real binary, then
misbehaves as ``FAKE_PDFTRACE_MODE`` says, for the files whose name contains
``FAKE_PDFTRACE_TARGET`` (every file if unset):

- ``copy`` (default): nothing, the outputs are exact;
- ``perturb``: line ``FAKE_PDFTRACE_LINE`` (1-based, default 2) gets `` !perturbed``
  appended;
- ``truncate``: the trace loses its last line;
- ``crash``: the process dies (``os._exit(101)``) when it reaches the file;
- ``hang``: the process sleeps ``FAKE_PDFTRACE_SLEEP`` seconds (default 30)
  before the file;
- ``slow``: the file's trace is right but ``_times.tsv`` says it took
  ``FAKE_PDFTRACE_SLEEP`` seconds;
- ``unimplemented``: exit status 2 before doing anything;
- ``unlisted``: like ``unimplemented``, and ``--list-stages`` lists every
  stage as not implemented (otherwise it lists them as implemented).
"""

from __future__ import annotations

import argparse
import hashlib
import os
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))

from oracle.stages import available_stages, trace_text  # noqa: E402


def main() -> int:
    mode = os.environ.get("FAKE_PDFTRACE_MODE", "copy")
    if sys.argv[1:] == ["--list-stages"]:
        for name in available_stages():
            print(name + (" (not implemented)" if mode == "unlisted" else ""))
        return 0
    ap = argparse.ArgumentParser()
    ap.add_argument("--stage", required=True)
    ap.add_argument("--password")
    ap.add_argument("--max-pages", type=int, default=20)
    ap.add_argument("--out", required=True)
    ap.add_argument("files", nargs="+")
    args = ap.parse_args()
    target = os.environ.get("FAKE_PDFTRACE_TARGET", "")
    if mode in ("unimplemented", "unlisted"):
        print(f"pdftrace: stage {args.stage} is not implemented", file=sys.stderr)
        return 2
    sleep = float(os.environ.get("FAKE_PDFTRACE_SLEEP", "30"))
    for name in args.files:
        started = time.monotonic()
        hit = target in Path(name).name
        if mode == "crash" and hit:
            print(f"pdftrace: crashing on {name}", file=sys.stderr)
            sys.stderr.flush()
            os._exit(101)
        if mode == "hang" and hit:
            time.sleep(sleep)
        data = Path(name).read_bytes()
        lines = trace_text(args.stage, name, args.password, args.max_pages).split("\n")[:-1]
        if mode == "perturb" and hit:
            k = int(os.environ.get("FAKE_PDFTRACE_LINE", "2")) - 1
            lines[k] += " !perturbed"
        if mode == "truncate" and hit:
            lines = lines[:-1]
        sha = hashlib.sha256(data).hexdigest()
        (Path(args.out) / f"{sha}.txt").write_bytes("".join(x + "\n" for x in lines).encode("ascii"))
        seconds = sleep if mode == "slow" and hit else time.monotonic() - started
        with open(Path(args.out) / "_times.tsv", "a") as times:
            times.write(f"{sha}\t{seconds:.3f}\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
