"""Unit cases of the interpreter (``pdfinterp``, ``pdfdevice``), replayed by
``crates/pdftrace/tests/interp_unit.rs``.

The ``interp_text`` and ``fonts_text`` stages (the ``interp`` and ``fonts``
stages on pdfminer's ``PDFTextDevice``, ``oracle/stages/interp_text.py``)
over files are the main check of these modules (``python -m oracle.verify
--set all``); this generator makes the same checks runnable by ``cargo
test`` (and so by ``cargo mutants``):

- ``stages.jsonl``: ``{"in": [name, pdf, password, what, max_pages],
  "out": trace}``, the trace of stage ``what`` (``interp_text``,
  ``fonts_text``) of each PDF of the adversarial corpus ``adv-interp``
  (regenerated first, from ``oracle/adversarial/interp.py``, its depth
  windows included), of ``micro`` and of the small files of the ``quick``
  set.

Every trace runs in a process forked for it (``document.in_child``), from
the state ``py_process.jsonl`` records (the interned symbols), as
``oracle/unit/document.py`` does. ``meta.json`` names the directory of the
predefined CMaps (``cmap_dir``, relative to the cache) that the replay's
provider reads.

Usage: PDFPLUMBER_ROOT=<main checkout> <root>/.venv/bin/python -m oracle.unit.interp
"""

from __future__ import annotations

import sys
from collections.abc import Iterator

from ..adversarial import interp as adv_interp
from ..codegen import cmaps as codegen_cmaps
from ..corpus import cache_dir, load_set
from ..stages import fonts_text as _fonts_text_stage  # noqa: F401  (loaded before the children fork)
from ..stages import interp_text as _interp_text_stage  # noqa: F401
from ..stages import trace_lines
from ._common import require_oracle_python, write_cases, write_meta
from .document import in_child, process_state

MODULE = "interp"
#: Files of the quick set at most this large are included.
QUICK_MAX_BYTES = 60_000
STAGES = ("interp_text", "fonts_text")
MAX_PAGES = 20


def stage_cases() -> Iterator[tuple[list, str]]:
    seen: set[str] = set()
    for set_name in ("adv-interp", "micro", "quick"):
        for f in load_set(set_name):
            if f.sha256 in seen or (set_name == "quick" and f.size > QUICK_MAX_BYTES):
                continue
            seen.add(f.sha256)
            data = f.path.read_bytes()
            for what in STAGES:
                lines = in_child(lambda: trace_lines(what, str(f.path), f.password, MAX_PAGES))
                yield [f.path.name, data, f.password, what, MAX_PAGES], "".join(x + "\n" for x in lines)


def main() -> None:
    require_oracle_python()
    cmap_dir = codegen_cmaps.ensure()
    rel_cmap_dir = cmap_dir.relative_to(cache_dir()).as_posix()
    # the adversarial corpus this module replays, as its generator writes it
    adv_interp.generate()
    state = process_state()
    write_cases(MODULE, "py_process", [([], state)])
    n = write_cases(MODULE, "stages", stage_cases())
    if process_state() != state:
        raise RuntimeError("the process state changed while the traces ran in children")
    print(f"{MODULE}: {n} stage traces", file=sys.stderr)
    write_meta(MODULE, cmap_dir=rel_cmap_dir)


if __name__ == "__main__":
    main()
