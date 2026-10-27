"""Stage registry and the common trace runner.

Each module ``oracle/stages/<name>.py`` (not starting with ``_``) is a stage.
It documents its lines at the top and exposes:

- ``PAGED``: True if ``max_pages`` changes its output (it is then part of the
  cache fingerprint);
- ``DEPENDS``: the other stage modules it imports code from (their sources are
  part of its cache fingerprint);
- ``run(path, password, max_pages) -> Iterator[str]``: the trace lines, without
  newlines, ending with ``!end`` or with the exception line that interrupted
  the stage (``_common.terminated`` adds that last line).

``trace_lines`` is the only entry point used by the CLI (``oracle.trace``),
the cache builder and the tests. It runs the stage in a fresh thread so that
pdfminer always starts at the same Python stack depth: a ``RecursionError``
(deep page trees, nested arrays) then happens at the same place whatever the
caller (CLI, worker process, unit test). The stage generator's frame sits at
depth ``STAGE_ENTRY_DEPTH`` (frames counted from the thread's first frame,
``threading.Thread._bootstrap``). The thread gets a 256 MiB C stack so that a
deep C-level recursion fails with Python's ``RecursionError``, as for a user,
rather than a segmentation fault; Python's recursion limit is left at its
default (1000) and counts frames, not bytes, so the larger C stack cannot
change a trace.

Logging is disabled while a stage runs (``logging.disable``), which keeps
stderr quiet. pdfminer never reads its log level, but a logging call costs
Python frames: ``log.debug(...)`` runs ``Logger.debug`` and
``Logger.isEnabledFor`` (two frames below the caller; the first call of a
level on a logger, after ``logging.disable`` cleared the caches, a third:
``_acquireLock``), which decides where a ``RecursionError`` happens in deep
recursions (the page tree of ``PDFPage.create_pages`` logs at each node).
The traces are those of a user whose configuration leaves pdfminer's
warnings off: ``logging.disable``, or the common
``logging.getLogger("pdfminer").setLevel(logging.ERROR)``, whose calls take
the same frames (``getEffectiveLevel`` fills the cache one frame below
``isEnabledFor``, as ``_acquireLock`` does). The default configuration (no
handler, level WARNING) costs the same for ``log.debug`` and ``log.info``,
but not for ``log.warning``, which it emits through ``logging.lastResort``:
eleven frames below the caller at the deepest (``Formatter.format`` ->
``PercentStyle._format``), and the ``%r`` of the arguments formatted, whose
``RecursionError`` on an operand nested near the C recursion limit
``StreamHandler.emit`` raises again (the page fails). A user with that
configuration can therefore see a ``RecursionError`` one Form XObject level
earlier, or on a deep operand that warns; a DEBUG configuration does the
same for every ``log.debug``. Not modeled (``docs/verification.md``,
"Stages", "Logging Disabled"). The Rust side counts the frames of disabled
logging like the others (``pycompat::logging``, ``pycompat::recursion``).
Warnings keep their default filters.

``entry_depth()`` measures the recursion depth a stage function's body runs
at (``oracle.depth.depth_here``); the Rust stages start from it
(``pdftrace::stages::STAGE_ENTRY``, checked against it by the unit tests).
"""

from __future__ import annotations

import importlib
import logging
import sys
import threading
from collections.abc import Callable, Iterator
from pathlib import Path
from types import ModuleType

from ..depth import depth_here
from ._common import terminated

DEFAULT_MAX_PAGES = 20
STACK_SIZE = 256 << 20
STAGE_DIR = Path(__file__).resolve().parent

#: Python frames below the stage generator's frame, see ``trace_lines``:
#: Thread._bootstrap, Thread._bootstrap_inner, Thread.run, _target, and the
#: ``terminated`` wrapper generator.
STAGE_ENTRY_DEPTH = 5


def available_stages() -> list[str]:
    return sorted(p.stem for p in STAGE_DIR.glob("*.py") if not p.stem.startswith("_"))


def get_stage(name: str) -> ModuleType:
    if name not in available_stages():
        raise KeyError(f"unknown stage {name!r} (known: {', '.join(available_stages())})")
    return importlib.import_module(f"{__name__}.{name}")


def stage_source_files(name: str) -> list[Path]:
    """Files whose content defines the stage's output (cache fingerprint)."""
    pkg = STAGE_DIR.parent
    stages: list[str] = []
    pending = [name]
    while pending:  # the stage and, transitively, the stages it DEPENDS on
        stage = pending.pop()
        if stage not in stages:
            stages.append(stage)
            pending.extend(getattr(get_stage(stage), "DEPENDS", ()))
    return [
        *(STAGE_DIR / f"{s}.py" for s in stages),
        STAGE_DIR / "__init__.py",
        STAGE_DIR / "_common.py",
        pkg / "canon.py",
        pkg / "typed_json.py",
        pkg / "depth.py",
        pkg / "trace.py",  # writes the cached bytes
        pkg / "environment.py",
    ]


def trace_lines(name: str, path: str | Path, password: str | None = None, max_pages: int = DEFAULT_MAX_PAGES) -> list[str]:
    """Run stage ``name`` on one PDF and return its trace lines."""
    stage = get_stage(name)
    return _in_stage_thread(lambda: stage.run(str(path), password, max_pages))


def entry_depth() -> dict[str, int]:
    """The recursion depth (``oracle.depth.depth_here``) at which the body of
    a stage function runs: a generator decorated with ``terminated``, run by
    the stage thread like ``trace_lines`` runs a stage."""

    @terminated
    def probe(path: str, password: str | None, max_pages: int) -> Iterator[str]:
        depth = depth_here()
        yield f"{depth['py']} {depth['c_left']}"

    py, c_left = _in_stage_thread(lambda: probe("", None, 0))[0].split()
    return {"py": int(py), "c_left": int(c_left)}


def _in_stage_thread(make: Callable[[], Iterator[str]]) -> list[str]:
    """The lines of the generator ``make()`` returns, consumed by ``list()``
    in a fresh thread with a 256 MiB stack and logging disabled."""
    box: dict[str, object] = {}

    def _target() -> None:
        try:
            box["lines"] = list(make())
        except BaseException as e:  # harness error (CanonError...): re-raised below
            box["error"] = e

    previous_stack = threading.stack_size(STACK_SIZE)
    logging.disable(logging.CRITICAL)
    try:
        thread = threading.Thread(target=_target, name="oracle-stage")
        thread.start()
        thread.join()
    finally:
        logging.disable(logging.NOTSET)
        threading.stack_size(previous_stack)
    if "error" in box:
        raise box["error"]  # type: ignore[misc]
    return box["lines"]  # type: ignore[return-value]


def trace_text(name: str, path: str | Path, password: str | None = None, max_pages: int = DEFAULT_MAX_PAGES) -> str:
    """The trace as written to files: each line followed by ``\\n``, ASCII."""
    return "".join(line + "\n" for line in trace_lines(name, path, password, max_pages))


__all__ = [
    "DEFAULT_MAX_PAGES",
    "STAGE_ENTRY_DEPTH",
    "available_stages",
    "entry_depth",
    "get_stage",
    "stage_source_files",
    "trace_lines",
    "trace_text",
]

if sys.version_info < (3, 12):  # pragma: no cover
    raise RuntimeError(
        'the oracle runs on CPython >= 3.12 (sum() semantics: docs/architecture.md, "Upstream Sources And Python Version")'
    )
