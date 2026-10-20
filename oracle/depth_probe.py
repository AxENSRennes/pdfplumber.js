"""Bounded measurement of the Python frames a call goes down to.

Where a ``RecursionError`` happens depends on the deepest frame a call
reaches, standard library included (``docs/architecture.md``, "Profondeur de
récursion"). The port models those frames as constants where they do not
depend on the input (``crates/pdfminer/src/cmapdb.rs``: ``_load_data``); this
module measures them, so that the constants can be checked again when the
oracle's environment changes.

A profile hook (``sys.setprofile``) keeps two integers, the current depth
and the deepest one, relative to the caller of the measured function: memory
stays constant whatever the call does. ``deepest_path`` also copies the
names of the frames on the stack, but only when a new maximum is reached
(at most a few dozen times per call). Never record the stack on every call:
an unbounded probe that did grew to 18 GB and froze the machine
(``AGENTS.md``). Run it under explicit limits::

    timeout 15m prlimit --as=4294967296 -- .venv/bin/python -m oracle.depth_probe cmaps

(``prlimit`` takes a number of bytes: ``--as=4G`` is not 4 GiB with
util-linux 2.39, every command run that way crashes at once.)

``cmaps``: for each predefined CMap and unicode map of pdfminer's data, and
for missing names, the deepest frame of ``CMapDB._load_data(name)`` (the
logger caches cleared before each), as ``depth path`` lines grouped by
depth. ``_load_data`` itself is depth 1.
"""

from __future__ import annotations

import argparse
import collections
import logging
import pathlib
import sys
from typing import Any

_state = {"depth": 0, "max": 0, "path": None, "record": False}


def _hook(frame, event, arg) -> None:
    if event == "call":
        _state["depth"] += 1
        if _state["depth"] > _state["max"]:
            _state["max"] = _state["depth"]
            if _state["record"]:
                names = []
                f = frame
                for _ in range(_state["depth"]):
                    names.append(f"{f.f_code.co_filename.rsplit('/', 1)[-1]}:{f.f_code.co_name}")
                    f = f.f_back
                _state["path"] = names[::-1]
    elif event == "return":
        _state["depth"] -= 1


def _measure(record: bool, f: Any, *args: Any) -> tuple[Any, int, list[str] | None]:
    _state.update(depth=0, max=0, path=None, record=record)
    sys.setprofile(_hook)
    try:
        out = f(*args)
    except Exception as e:  # the exception is a result
        out = e
    finally:
        sys.setprofile(None)
    return out, _state["max"], _state["path"]


def max_depth(f: Any, *args: Any) -> tuple[Any, int]:
    """``(f(*args) or its exception, deepest frame)``: ``f``'s own frame is
    1 (so is a class's ``__init__``, called from C)."""
    out, depth, _ = _measure(False, f, *args)
    return out, depth


def deepest_path(f: Any, *args: Any) -> tuple[int, list[str]]:
    """The deepest frame of ``f(*args)`` and the names of the frames down to
    it (``file:function``)."""
    _, depth, path = _measure(True, f, *args)
    return depth, path or []


def cmaps() -> None:
    import pdfminer
    from pdfminer.cmapdb import CMapDB

    directory = pathlib.Path(pdfminer.__file__).parent / "cmap"
    names = sorted(p.name[: -len(".json.gz")] for p in directory.glob("*.json.gz"))
    groups: dict[tuple[int, str, tuple[str, ...]], list[str]] = collections.defaultdict(list)
    for name in [*names, "unknown", "Foo-H", "to-unicode-Adobe-Foo", "x/../Foo"]:
        logging.disable(logging.CRITICAL)
        out, _ = max_depth(CMapDB._load_data, name)
        logging.disable(logging.CRITICAL)
        depth, path = deepest_path(CMapDB._load_data, name)
        outcome = type(out).__name__ if isinstance(out, Exception) else "loaded"
        groups[(depth, outcome, tuple(path))].append(name)
    for (depth, outcome, path), group in sorted(groups.items()):
        print(f"depth {depth} ({outcome}, {len(group)} names, e.g. {group[:3]}): {' -> '.join(path)}")


def structure() -> None:
    """The standard library under ``PDFStructElement.to_dict`` (the
    ``copy.deepcopy`` of ``asdict``'s leaves, each kind twice: the first
    deep copy of a class in a process fills ``copyreg``'s cache) and
    ``Page.annots`` (``warnings.warn``, twice)."""
    import copy
    import dataclasses
    import io
    import warnings

    from pdfminer.pdfdocument import PDFDocument
    from pdfminer.pdfparser import PDFParser
    from pdfminer.pdftypes import PDFObjRef
    from pdfminer.psparser import KWD, LIT
    from pdfplumber.structure import PDFStructElement

    path = pathlib.Path(__file__).resolve().parent.parent / "corpus" / "micro"
    sample = next(iter(sorted(path.glob("*.pdf"))), None)
    if sample is None:
        sample = pathlib.Path(sys.argv[2])
    fp = open(sample, "rb")
    doc = PDFDocument(PDFParser(fp))
    bio_doc = PDFDocument(PDFParser(io.BytesIO(sample.read_bytes())))
    el = PDFStructElement(type="P", revision=None, id=None, lang=None, alt_text=None, actual_text=None, title=None,
                          page_number=None)
    cases = [
        ("deepcopy PSLiteral (first)", copy.deepcopy, LIT("probe-literal")),
        ("deepcopy PSLiteral (again)", copy.deepcopy, LIT("probe-literal-2")),
        ("deepcopy PSKeyword (first)", copy.deepcopy, KWD(b"probe-keyword")),
        ("deepcopy PSKeyword (again)", copy.deepcopy, KWD(b"probe-keyword-2")),
        ("deepcopy PDFObjRef BufferedReader (first)", copy.deepcopy, PDFObjRef(doc, 1)),
        ("deepcopy PDFObjRef BufferedReader (again)", copy.deepcopy, PDFObjRef(doc, 2)),
        ("deepcopy PDFObjRef BytesIO", copy.deepcopy, PDFObjRef(bio_doc, 1)),
        ("deepcopy int", copy.deepcopy, 5),
        ("fields", dataclasses.fields, el),
        ("_is_dataclass_instance", dataclasses._is_dataclass_instance, el),
        ("warnings.warn (first)", warnings.warn, "Could not decode uri of annotation. uri will be missing."),
        ("warnings.warn (again)", warnings.warn, "Could not decode uri of annotation. uri will be missing."),
        ("warnings.warn (other)", warnings.warn, "Could not decode title of annotation. title will be missing."),
    ]
    for label, f, arg in cases:
        out, depth, path = _measure(True, f, arg)
        outcome = type(out).__name__ if isinstance(out, Exception) else "ok"
        print(f"{label}: depth {depth} ({outcome}): {' -> '.join(path or [])}")


def main() -> None:
    ap = argparse.ArgumentParser(prog="python -m oracle.depth_probe")
    ap.add_argument("what", choices=["cmaps", "structure"])
    args = ap.parse_args()
    {"cmaps": cmaps, "structure": structure}[args.what]()


if __name__ == "__main__":
    main()
