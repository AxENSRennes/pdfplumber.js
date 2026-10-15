"""Regenerate the unit case data that is missing or stale.

    PDFPLUMBER_ROOT=<main checkout> <root>/.venv/bin/python -m oracle.unit [--check] [--force] [MODULE ...]

For each generator (all by default), the data is *current* when a directory
``<cache>/unit/<module>/<fingerprint>/`` (``oracle/unit/_common.py``) has a
``meta.json`` (and, for ``psparser``, ``fuzz.json`` and ``corpus.json``)
recording the SHA-256 of oracle sources that are unchanged in this checkout,
the check ``pdftrace::unit::UnitData`` makes before replaying it, and the
current environment (``oracle/environment.py``), which only Python can see.
The current directory's ``meta.json`` is touched, so that it is the most
recent of those whose sources match: the one the Rust side picks. Each stale
module is regenerated in its own process.

``--check`` only reports (exit status 1 if something is stale); ``--force``
regenerates everything.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
import sys
import time
from pathlib import Path

from ..corpus import REPO_DIR, cache_dir
from ..environment import environment

#: Generator -> (data files carrying ``sources``, runs as argument lists).
GENERATORS: dict[str, tuple[tuple[str, ...], tuple[tuple[str, ...], ...]]] = {
    "canon": (("meta.json",), ((),)),
    "pycompat": (("meta.json",), ((),)),
    "exceptions": (("meta.json",), ((),)),
    "pdftypes": (("meta.json",), ((),)),
    "psparser_base": (("meta.json",), ((),)),
    "utils": (("meta.json",), ((),)),
    # generated layouts and every configuration of LAParams on the corpus pages (minutes)
    "layout": (("meta.json",), ((),)),
    # the adversarial corpus (seconds), then the real one (minutes: every PDF)
    "psparser": (("meta.json", "fuzz.json", "corpus.json"), (("--fuzz", "4000"), ())),
    "zlib": (("meta.json",), ((),)),
    # generated cases, then every stream of the real corpus (minutes)
    "filters": (("meta.json", "corpus.json"), ((),)),
}


def _directory_reason(module: str, directory: Path) -> str | None:
    """Why the data in ``directory`` cannot be replayed here, or None."""
    for name in GENERATORS[module][0]:
        path = directory / name
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            return f"no {path}"
        sources = data.get("sources")
        if not isinstance(sources, dict) or f"oracle/unit/{module}.py" not in sources:
            return f"{path} comes from an older generator"
        for rel, sha in sources.items():
            try:
                current = hashlib.sha256((REPO_DIR / rel).read_bytes()).hexdigest()
            except OSError:
                return f"{rel} no longer exists"
            if current != sha:
                return f"{rel} changed since {path.name} was written"
        if name == "meta.json" and data.get("environment") != json.loads(json.dumps(environment())):
            return f"the oracle environment changed since {path} was written (oracle/environment.py)"
    return None


def stale_reason(module: str) -> str | None:
    """Why the data of ``module`` cannot be replayed, or None if it is current
    (its directory is then marked as the most recent one)."""
    base = cache_dir() / "unit" / module
    candidates = sorted(
        (d for d in (base.iterdir() if base.is_dir() else []) if d.is_dir() and (d / "meta.json").is_file()),
        key=lambda d: (d / "meta.json").stat().st_mtime_ns,
        reverse=True,
    )
    reasons = []
    for directory in candidates:
        reason = _directory_reason(module, directory)
        if reason is None:
            os.utime(directory / "meta.json")  # the one UnitData picks
            return None
        reasons.append(reason)
    return reasons[0] if reasons else f"no data in {base}"


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(prog="python -m oracle.unit", description=__doc__.split("\n\n")[0])
    ap.add_argument("modules", nargs="*", metavar="MODULE", help=f"default: all ({', '.join(GENERATORS)})")
    ap.add_argument("--check", action="store_true", help="only report what is stale")
    ap.add_argument("--force", action="store_true", help="regenerate even current data")
    args = ap.parse_args(argv)
    unknown = [m for m in args.modules if m not in GENERATORS]
    if unknown:
        ap.error(f"unknown module(s) {', '.join(unknown)} (known: {', '.join(GENERATORS)})")
    stale = 0
    for module in args.modules or list(GENERATORS):
        reason = "--force" if args.force else stale_reason(module)
        if reason is None:
            print(f"unit/{module}: current")
            continue
        stale += 1
        print(f"unit/{module}: {reason}")
        if args.check:
            continue
        for run in GENERATORS[module][1]:
            started = time.monotonic()
            cmd = [sys.executable, "-m", f"oracle.unit.{module}", *run]
            print(f"  $ python -m oracle.unit.{module} {' '.join(run)}".rstrip(), flush=True)
            if subprocess.run(cmd, cwd=REPO_DIR).returncode != 0:
                print(f"unit/{module}: generation failed", file=sys.stderr)
                return 1
            print(f"  ({time.monotonic() - started:.0f} s)", flush=True)
    return 1 if args.check and stale else 0


if __name__ == "__main__":
    sys.exit(main())
