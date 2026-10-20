"""Unit cases of pdfplumber's page layer (``pdf``, ``page``, ``container``,
``convert``, ``utils/pdfinternals``, and ``structure`` through
``plumber_doc``), replayed by
``crates/pdftrace/tests/plumber_unit.rs``.

The ``plumber_*`` stages over files are the main check of these modules
(``python -m oracle.verify --set all``); this generator makes the same
checks runnable by ``cargo test`` (and so by ``cargo mutants``):

- ``stages.jsonl``: ``{"in": [name, pdf, password, what, max_pages,
  entry], "out": trace}``, the trace of stage ``what`` (``STAGES``; the
  replay hands an ``ORACLE_INPUT`` stage its Python trace, for the ``id()``
  order of the analysis and the matches of ``search``) of each PDF of the
  adversarial corpus ``adv-plumber`` (regenerated first, from
  ``oracle/adversarial/plumber.py``, its depth windows included), of
  ``micro`` and of the small files of the ``quick`` set. ``entry`` is
  ``None`` for a trace run as ``oracle.verify`` runs it; the depth windows
  (``depth-*`` files) are also traced from one and two frames deeper
  (``entry``: the ``{"py", "c_left"}`` depth of the stage body), which
  moves the recursion limit within the nesting levels of their paths (a
  Form XObject is three frames, a list level of ``resolve_all`` two).

Every trace runs in a process forked for it (``document.in_child``), from
the state ``py_process.jsonl`` records, as ``oracle/unit/interp.py`` does;
``meta.json`` names the directory of the predefined CMaps (``cmap_dir``).

Usage: PDFPLUMBER_ROOT=<main checkout> <root>/.venv/bin/python -m oracle.unit.plumber
"""

from __future__ import annotations

import sys
from collections.abc import Iterator

from ..adversarial import plumber as adv_plumber
from ..codegen import cmaps as codegen_cmaps
from ..corpus import cache_dir, load_set
from ..stages import plumber_doc as _plumber_doc_stage  # noqa: F401  (loaded before the children fork)
from ..stages import plumber_objects as _plumber_objects_stage  # noqa: F401
from ..stages import plumber_tables as _plumber_tables_stage  # noqa: F401
from ..stages import plumber_text as _plumber_text_stage  # noqa: F401
from ..stages import trace_lines
from ._common import require_oracle_python, write_cases, write_meta
from .document import in_child, process_state
from .interp import deeper_trace, entry_at

MODULE = "plumber"
#: Files of the quick set at most this large are included.
QUICK_MAX_BYTES = 60_000
STAGES = ("plumber_objects", "plumber_text", "plumber_tables", "plumber_doc")
MAX_PAGES = 20
#: Extra frames below the stage body for the depth windows.
DEPTH_OFFSETS = (1, 2)


def stage_cases() -> Iterator[tuple[list, str]]:
    seen: set[str] = set()
    for set_name in ("adv-plumber", "micro", "quick"):
        for f in load_set(set_name):
            if f.sha256 in seen or (set_name == "quick" and f.size > QUICK_MAX_BYTES):
                continue
            seen.add(f.sha256)
            data = f.path.read_bytes()
            for what in STAGES:
                lines = in_child(lambda: trace_lines(what, str(f.path), f.password, MAX_PAGES))
                yield [f.path.name, data, f.password, what, MAX_PAGES, None], "".join(x + "\n" for x in lines)
                if not f.path.name[4:].startswith("depth-"):
                    continue
                for k in DEPTH_OFFSETS:
                    entry = in_child(lambda: entry_at(k))
                    lines = in_child(lambda: deeper_trace(what, str(f.path), f.password, k))
                    yield [f.path.name, data, f.password, what, MAX_PAGES, entry], "".join(x + "\n" for x in lines)


def main() -> None:
    require_oracle_python()
    cmap_dir = codegen_cmaps.ensure()
    rel_cmap_dir = cmap_dir.relative_to(cache_dir()).as_posix()
    # the adversarial corpus this module replays, as its generator writes it
    adv_plumber.generate()
    state = process_state()
    write_cases(MODULE, "py_process", [([], state)])
    n = write_cases(MODULE, "stages", stage_cases())
    if process_state() != state:
        raise RuntimeError("the process state changed while the traces ran in children")
    print(f"{MODULE}: {n} stage traces", file=sys.stderr)
    write_meta(MODULE, cmap_dir=rel_cmap_dir)


if __name__ == "__main__":
    main()
