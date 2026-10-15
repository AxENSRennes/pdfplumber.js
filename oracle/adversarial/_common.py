"""Output of the adversarial generators: ``<cache>/corpus/adv-<stage>/``.

``write_corpus(stage, cases)`` replaces the directory's PDFs with the
generated ones (``NNN-<name>.pdf``, in case order) and writes
``manifest.json`` with the password of each file that needs one, which
``oracle/corpus.py`` reads (``password`` keyed by ``sha256``). Every file
is distinct (a case that wants several passwords for one document writes
variants that differ by a comment).
"""

from __future__ import annotations

import hashlib
import json
import sys
from pathlib import Path

from ..corpus import cache_dir


def corpus_dir(stage: str) -> Path:
    return cache_dir() / "corpus" / f"adv-{stage}"


def write_corpus(stage: str, cases: list[tuple[str, bytes, str | None]]) -> Path:
    """Writes ``(name, data, password)`` cases; returns the directory."""
    out = corpus_dir(stage)
    out.mkdir(parents=True, exist_ok=True)
    for old in out.glob("*.pdf"):
        old.unlink()
    manifest = []
    seen: dict[str, str] = {}
    for i, (name, data, password) in enumerate(cases):
        sha = hashlib.sha256(data).hexdigest()
        if sha in seen:
            raise ValueError(f"case {name!r} has the same bytes as {seen[sha]!r}")
        seen[sha] = name
        path = out / f"{i:03d}-{name}.pdf"
        path.write_bytes(data)
        entry: dict[str, object] = {"id": f"adv-{stage}/{path.name}", "sha256": sha, "size": len(data)}
        if password is not None:
            entry["password"] = password
        manifest.append(entry)
    (out / "manifest.json").write_text(json.dumps(manifest, indent=1) + "\n")
    print(f"adv-{stage}: {len(cases)} PDFs in {out}", file=sys.stderr)
    return out
