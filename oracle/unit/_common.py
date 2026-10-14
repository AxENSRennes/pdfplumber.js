"""Shared plumbing of the unit case generators (``oracle/unit/<module>.py``).

Every generator runs as a module from the checkout whose Rust code will
replay its cases::

    PDFPLUMBER_ROOT=<main checkout> <root>/.venv/bin/python -m oracle.unit.<module>

and writes into ``<cache>/unit/<module>/`` (``<cache>`` is ``PDFPLUMBER_CACHE``
or ``<PDFPLUMBER_ROOT>/.cache``, see ``oracle.corpus.cache_dir``):

- ``<name>.jsonl``: one case per line, ``{"in": [arguments...], "out": result}``
  in typed JSON (``oracle.typed_json.dumps``, compact ASCII);
- ``meta.json``: ``{"sources": {<repo path>: <sha256>, ...}, "python": ..., ...}``
  where ``sources`` lists every ``oracle/`` module loaded by the generator
  (the generator itself, ``canon.py``, ``typed_json.py``...). The Rust side
  (``pdftrace::unit::UnitData``) refuses the data when one of these files has
  changed since, and prints the command that regenerates it.
"""

from __future__ import annotations

import hashlib
import json
import logging
import os
import sys
from collections.abc import Iterable
from pathlib import Path
from typing import Any

from ..corpus import REPO_DIR, cache_dir
from ..typed_json import dumps

__all__ = ["oracle_sources", "require_oracle_python", "unit_dir", "write_cases", "write_meta"]

ORACLE_DIR = REPO_DIR / "oracle"


_snapshot: dict[str, str] = {}


def require_oracle_python() -> None:
    """Start of a generator: exit unless this is CPython 3.12, the oracle
    (``.python-version``), and record the hashes of the oracle sources
    loaded so far (``oracle_sources`` refuses to go on if they change
    during the run).

    Also silences ``logging``: pdfminer never reads its log level, so this
    changes no result, only the noise of malformed inputs.
    """
    if sys.implementation.name != "cpython" or sys.version_info[:2] != (3, 12):
        sys.exit(f"the oracle is CPython 3.12, not {sys.implementation.name} {sys.version.split()[0]}")
    logging.disable(logging.CRITICAL)
    _snapshot.update(_loaded_sources())


def unit_dir(module: str) -> Path:
    """``<cache>/unit/<module>/``, created if needed."""
    path = cache_dir() / "unit" / module
    path.mkdir(parents=True, exist_ok=True)
    return path


def _sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def oracle_sources() -> dict[str, str]:
    """SHA-256 of every module of ``oracle/`` loaded so far, by repo path.

    Raises if a file hashed by ``require_oracle_python`` has changed since:
    the data would be attributed to code that did not produce it.
    """
    now = _loaded_sources()
    changed = [path for path, sha in _snapshot.items() if now.get(path) != sha]
    if changed:
        raise RuntimeError(f"oracle sources changed during the run: {', '.join(changed)}; run the generator again")
    return now


def _loaded_sources() -> dict[str, str]:
    found: dict[str, str] = {}
    for module in list(sys.modules.values()):
        file = getattr(module, "__file__", None)
        if not file:
            continue
        path = Path(file).resolve()
        if path.suffix == ".py" and path.is_relative_to(ORACLE_DIR):
            found[path.relative_to(REPO_DIR).as_posix()] = _sha256(path)
    return dict(sorted(found.items()))


def _atomic_write_text(path: Path, text: str) -> None:
    tmp = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    tmp.write_text(text, encoding="ascii")
    tmp.replace(path)


def write_cases(module: str, name: str, cases: Iterable[tuple[list[Any], Any]]) -> int:
    """Writes ``<name>.jsonl``, one ``{"in": args, "out": result}`` line per case."""
    path = unit_dir(module) / f"{name}.jsonl"
    tmp = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    n = 0
    with open(tmp, "w", encoding="ascii") as f:
        for args, out in cases:
            f.write('{"in":' + dumps(list(args)) + ',"out":' + dumps(out) + "}\n")
            n += 1
    tmp.replace(path)
    return n


def write_meta(module: str, **extra: Any) -> None:
    """Writes ``meta.json``; call it last, once every case file is written."""
    meta = {"sources": oracle_sources(), "python": sys.version.split()[0], **extra}
    _atomic_write_text(unit_dir(module) / "meta.json", json.dumps(meta, indent=1) + "\n")
