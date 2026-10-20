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

- ``open.jsonl``: ``{"in": [name, pdf, options], "out": trace}``, a few
  small documents opened with each ``pdfplumber.open`` keyword arguments of
  ``OPEN_VARIANTS`` (``laparams``, ``pages``, ``password``... of every
  type): the pages' ``initial_doctop``, objects and the ``id()`` ranking of
  their analysis (``open_lines``).

Every trace runs in a process forked for it (``document.in_child``), from
the state ``py_process.jsonl`` records, as ``oracle/unit/interp.py`` does;
``meta.json`` names the directory of the predefined CMaps (``cmap_dir``).

Usage: PDFPLUMBER_ROOT=<main checkout> <root>/.venv/bin/python -m oracle.unit.plumber
"""

from __future__ import annotations

import os
import sys
import tempfile
from collections.abc import Iterator
from typing import Any

import pdfplumber

from ..adversarial import plumber as adv_plumber
from ..codegen import cmaps as codegen_cmaps
from ..corpus import cache_dir, load_set
from ..stages import plumber_doc as _plumber_doc_stage  # noqa: F401  (loaded before the children fork)
from ..stages import plumber_objects as _plumber_objects_stage  # noqa: F401
from ..stages import plumber_tables as _plumber_tables_stage  # noqa: F401
from ..stages import plumber_text as _plumber_text_stage  # noqa: F401
from ..canon import canon
from ..stages import _in_stage_thread, trace_lines
from ..stages._common import terminated
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


#: ``pdfplumber.open`` keyword arguments of every type (the stages only use
#: ``pages=[1..20]``, ``password`` and ``laparams={}``). A layout margin
#: that is not a number is not modelled (``crates/pdfplumber/src/pdf.rs``).
OPEN_VARIANTS: list[dict[str, Any]] = [
    {},
    {"laparams": {}},
    {"laparams": None},
    {"laparams": {"char_margin": 0}},
    {"laparams": {"char_margin": 100.5}},
    {"laparams": {"line_margin": 0, "word_margin": 0}},
    {"laparams": {"line_overlap": 1, "line_margin": 3}},
    {"laparams": {"line_overlap": -1}},
    {"laparams": {"boxes_flow": None}},
    {"laparams": {"boxes_flow": -1}},
    {"laparams": {"boxes_flow": 1}},
    {"laparams": {"boxes_flow": 0.25}},
    {"laparams": {"boxes_flow": 1.5}},
    {"laparams": {"boxes_flow": -1.0000001}},
    {"laparams": {"boxes_flow": "x"}},
    {"laparams": {"boxes_flow": [0.5]}},
    {"laparams": {"boxes_flow": True}},
    {"laparams": {"detect_vertical": True}},
    {"laparams": {"detect_vertical": True, "boxes_flow": None}},
    {"laparams": {"all_texts": True}},
    {"laparams": {"all_texts": "yes", "detect_vertical": []}},
    {"laparams": {"unknown": 1}},
    {"laparams": {"char_margin": 1, "nope": 2}},
    {"laparams": {"char_margin": True, "word_margin": False}},
    {"laparams": {"char_margin": 2 ** 70}},
    {"laparams": {"char_margin": float("inf"), "line_margin": float("inf")}},
    {"laparams": {"word_margin": float("nan")}},
    {"laparams": [1]},
    {"laparams": "x"},
    {"laparams": 0},
    {"pages": [1]},
    {"pages": [2, 1]},
    {"pages": (1,)},
    {"pages": []},
    {"pages": [0, -1, 3]},
    {"pages": [1.0, 2.5]},
    {"pages": [True]},
    {"pages": ["1"]},
    {"pages": 1},
    {"pages": "1"},
    {"pages": [[1]]},
    {"pages": None},
    {"pages": [1, 2], "laparams": {"char_margin": 5}},
    {"password": ""},
    {"password": "not used"},
    {"strict_metadata": True},
    {"unicode_norm": "NFKD"},
    {"unicode_norm": "bad"},
    {"unicode_norm": 3},
    {"raise_unicode_errors": False},
]


def open_lines(path: str, options: dict[str, Any]) -> Iterator[str]:
    """``pdfplumber.open(path, **options)`` and, for each page, its
    ``initial_doctop``, objects and (with ``laparams``) the ``id()``
    ranking of its analysis, as ``plumber_objects`` writes them."""
    from ..stages.plumber_objects import content_ids, exc, objects_lines

    try:
        pdf = pdfplumber.open(path, **options)
    except Exception as e:
        yield "open " + exc(e)
        return
    try:
        pages = pdf.pages
    except Exception as e:
        yield "pages " + exc(e)
        return
    for page in pages:
        yield f"page {page.page_number} " + canon(page.initial_doctop)
        lines = list(objects_lines(page))
        yield from lines
        if pdf.laparams is not None and not lines[0].startswith("objects "):
            yield f"ids {page.page_number} " + canon(content_ids(page.layout))


def open_cases() -> Iterator[tuple[list, str]]:
    """``open.jsonl``: ``{"in": [name, pdf, options], "out": trace}``, each
    variant of ``OPEN_VARIANTS`` on a few small documents (the trace of
    ``open_lines`` run as a stage: a fresh thread, ``terminated``)."""
    docs = [("scene.pdf", adv_plumber.doc([(adv_plumber.SCENE, {})] * 3)),
            ("layout.pdf", adv_plumber.doc([(adv_plumber.text(20, 180, b"one two") + adv_plumber.text(20, 160, b"three")
                                             + adv_plumber.text(120, 180, b"right") + adv_plumber.text(20, 20, b"foot"), {})] * 2))]
    for name, data in docs:
        with tempfile.NamedTemporaryFile(suffix=".pdf", delete=False) as f:
            f.write(data)
            path = f.name
        try:
            for options in OPEN_VARIANTS:
                run = terminated(lambda path, password, max_pages, options=options: open_lines(path, options))
                lines = in_child(lambda: _in_stage_thread(lambda: run(path, None, 0)))
                yield [name, data, options], "".join(x + "\n" for x in lines)
        finally:
            os.unlink(path)


def main() -> None:
    require_oracle_python()
    cmap_dir = codegen_cmaps.ensure()
    rel_cmap_dir = cmap_dir.relative_to(cache_dir()).as_posix()
    # the adversarial corpus this module replays, as its generator writes it
    adv_plumber.generate()
    state = process_state()
    write_cases(MODULE, "py_process", [([], state)])
    n = write_cases(MODULE, "stages", stage_cases())
    n_open = write_cases(MODULE, "open", open_cases())
    if process_state() != state:
        raise RuntimeError("the process state changed while the traces ran in children")
    print(f"{MODULE}: {n} stage traces, {n_open} openings", file=sys.stderr)
    write_meta(MODULE, cmap_dir=rel_cmap_dir)


if __name__ == "__main__":
    main()
