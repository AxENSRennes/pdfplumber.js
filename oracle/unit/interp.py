"""Unit cases of the interpreter (``pdfinterp``, ``pdfdevice``), replayed by
``crates/pdftrace/tests/interp_unit.rs``.

The stages that drive the pages (``interp``, ``fonts``, ``layout``, as
pdfplumber does, and ``interp_text``, ``fonts_text``, the first two on
pdfminer's ``PDFTextDevice``, ``oracle/stages/interp_text.py``) over files
are the main check of these modules (``python -m oracle.verify --set
all``); this generator makes the same checks runnable by ``cargo test``
(and so by ``cargo mutants``):

- ``stages.jsonl``: ``{"in": [name, pdf, password, what, max_pages,
  entry], "out": trace}``, the trace of stage ``what`` (``STAGES``; the
  replay hands a ``layout`` trace to the Rust stage as its oracle input,
  for the ``id()`` order of its analysis) of each PDF of the adversarial corpus ``adv-interp``
  (regenerated first, from ``oracle/adversarial/interp.py``, its depth
  windows included), of ``micro`` and of the small files of the ``quick``
  set. ``entry`` is ``None`` for a trace run as ``oracle.verify`` runs it;
  the depth windows (``depth-*`` files) are also traced from one and two
  frames deeper (``entry``: the ``{"py", "c_left"}`` depth of the stage
  body, ``oracle.depth.depth_here``), which moves the recursion limit to
  each of the three positions it can have within a nesting level (a level
  of Form XObjects is three frames): a frame miscounted on a path near the
  deepest of its level shows at one of them.

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
from ..stages import fonts as _fonts_stage  # noqa: F401  (loaded before the children fork)
from ..stages import fonts_text as _fonts_text_stage  # noqa: F401
from ..stages import interp as _interp_stage  # noqa: F401
from ..stages import interp_text as _interp_text_stage  # noqa: F401
from ..stages import layout as _layout_stage  # noqa: F401
from ..depth import depth_here
from ..stages import _in_stage_thread, get_stage, trace_lines
from ..stages._common import terminated
from ._common import require_oracle_python, write_cases, write_meta
from .document import in_child, process_state

MODULE = "interp"
#: Files of the quick set at most this large are included.
QUICK_MAX_BYTES = 60_000
STAGES = ("interp_text", "fonts_text", "interp", "fonts", "layout")
MAX_PAGES = 20
#: Extra frames below the stage body for the depth windows.
DEPTH_OFFSETS = (1, 2)


def nested(k: int, make):
    """The generator ``make()`` resumed through ``k`` more generator frames."""
    if k == 0:
        yield from make()
    else:
        yield from nested(k - 1, make)


def entry_at(k: int) -> dict[str, int]:
    """The depth of a stage body run through ``nested(k, ...)``."""

    @terminated
    def probe(path: str, password: str | None, max_pages: int):
        depth = depth_here()
        yield f"{depth['py']} {depth['c_left']}"

    py, c_left = _in_stage_thread(lambda: nested(k, lambda: probe("", None, 0)))[0].split()
    return {"py": int(py), "c_left": int(c_left)}


def deeper_trace(what: str, path: str, password: str | None, k: int) -> list[str]:
    stage = get_stage(what)
    return _in_stage_thread(lambda: nested(k, lambda: stage.run(path, password, MAX_PAGES)))


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
