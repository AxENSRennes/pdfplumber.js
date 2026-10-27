"""Classify, by their content, the traces of a ``plumber_*`` stage through
the public JavaScript API that differ from Python's.

    python -m oracle.api_parity --stage S --set NAME [-j N] [--timeout T]
                                [--max-pages N] [--files F ...] [--json]

Runs, on the files of the set, the Python oracle (cached, as
``oracle.verify`` builds it), the Rust library (``pdftrace``, release) and
the public API (``js/trace/pdftrace-js``, the build of ``dist/`` with
``dist/pdfplumber-oracle.wasm``), then sorts each file whose API trace is
not Python's (``docs/verification.md``,
"Parity Through The JavaScript API"):

- ``core``: the API's trace is the Rust library's byte for byte, and the
  Rust library's differs from Python's: a known difference of the core
  (``oracle.verify`` of the stage shows it), to which the API adds
  nothing;
- ``filter``: stage ``plumber_doc``; once every derived page made by a
  ``filter`` step whose objects raise is aligned (Python writes the
  ``dbox``, ``dobjects <exc>`` and ``dannots`` lines of the filtered
  page, the API rejects ``filter()`` with that same exception: a
  ``derive <exc>`` line, the documented gap of ``filter()``), the traces
  are identical;
- ``window``: a depth window (``depth-*``, ``climit-*``) of an adversarial
  set that has its session in ``depth.jsonl`` and ``depth-bytes.jsonl``
  of ``oracle/unit/js_api.py`` (where the parity of the API at the
  recursion limit is checked), whose traces are identical up to a first
  difference where one side's line is an exception that is, or is caused
  by, ``RecursionError`` (the stages' oracle calls pdfplumber from nested
  generators, the API from the top level of a script: the limit is
  reached elsewhere, by construction);
- ``defect``: anything else, including an API run that crashed or ran out
  of time while the Rust library did not; the exit status is then 1.

``--files`` classifies explicit files (their set, for ``window``, named
by ``--set`` too). The outputs of both engines go to a scratch directory
under ``<cache>/tmp``, removed at the end.
"""

from __future__ import annotations

import argparse
import json
import re
import shutil
import subprocess
import sys
import tempfile
from collections.abc import Callable
from pathlib import Path

from .cache import DEFAULT_JOBS, OracleCache
from .corpus import SET_NAMES, CorpusError, CorpusFile, cache_dir, files_from_paths, load_set
from .stages import DEFAULT_MAX_PAGES
from .verify import (
    DEFAULT_BATCH,
    DEFAULT_PDFTRACE,
    JS_API_BUILT,
    JS_API_TRACE,
    REPO_DIR,
    cmap_provider,
    first_difference,
    oracle_input,
    run_rust,
)

STAGES = ("plumber_objects", "plumber_text", "plumber_tables", "plumber_doc")
CLASSES = ("identical", "core", "filter", "window", "defect")
WINDOW_NAME = re.compile(r"(^|-)(depth|climit)-")
#: an exception line (``exc`` of the stages) that is, or is caused by, RecursionError
RECURSION = re.compile(r"!RecursionError\(")
#: the frames of recursion limit added to Python for a window whose
#: RecursionError pdfplumber caught: more than the levels by which the
#: API's statements are above the stages' (0 to 7 levels of nesting,
#: docs/verification.md, "Parity Through The JavaScript API")
HEADROOM = 200


def _lines(data: bytes) -> list[str]:
    return data.decode("utf-8", "replace").split("\n")


def _first(a: list[str], b: list[str], start: int = 0) -> int | None:
    """The index of the first differing line from ``start``, ``None`` when
    the lists are equal."""
    n = min(len(a), len(b))
    for k in range(start, n):
        if a[k] != b[k]:
            return k
    return None if len(a) == len(b) else n


def _gap(out: list[str], api: list[str], k: int, window: bool) -> bool:
    """Whether line ``k`` starts a gap of ``filter()``: after the same
    ``derived <steps>`` line, whose steps have a ``filter`` step, Python
    writes ``dbox ...``, ``dobjects <exc>`` and ``dannots ...`` where the
    API writes ``derive <exc>``: the same exception, or, in a depth window
    (``window``), two exceptions one of which is a ``RecursionError`` (the
    API evaluates the predicate on the parent's objects from another
    depth)."""
    if not (
        0 < k
        and k + 2 < len(out)
        and k < len(api)
        and out[k - 1].startswith("derived ")
        and '["filter", ' in out[k - 1]
        and out[k].startswith("dbox ")
        and out[k + 1].startswith("dobjects !")
        and out[k + 2].startswith("dannots ")
        and api[k].startswith("derive !")
    ):
        return False
    python_exc, api_exc = out[k + 1][len("dobjects ") :], api[k][len("derive ") :]
    return python_exc == api_exc or (window and bool(RECURSION.search(python_exc) or RECURSION.search(api_exc)))


def align_filters(oracle: list[str], api: list[str], window: bool = False) -> tuple[list[str], int, int | None]:
    """``oracle`` with each gap of ``filter()`` (``_gap``) before the first
    other difference replaced by the API's ``derive <exc>`` line, the number
    of gaps, and the index of that other difference (``None``: the traces
    are then identical)."""
    out = list(oracle)
    gaps = 0
    k = _first(out, api)
    while k is not None and _gap(out, api, k, window):
        out[k : k + 3] = [api[k]]
        gaps += 1
        k = _first(out, api, k + 1)
    return out, gaps, k


def unit_data_dir(module: str) -> Path:
    """The unit data of ``module`` for this checkout: the most recent
    ``<cache>/unit/<module>/<fingerprint>/`` whose ``meta.json`` lists the
    oracle sources as they are (as ``js/test/oracle-data.mjs`` and
    ``pdftrace::unit::UnitData`` pick it)."""
    import hashlib

    best: tuple[float, Path] | None = None
    base = cache_dir() / "unit" / module
    for meta in base.glob("*/meta.json") if base.is_dir() else []:
        sources = json.loads(meta.read_text()).get("sources", {})
        if all((REPO_DIR / p).is_file() and hashlib.sha256((REPO_DIR / p).read_bytes()).hexdigest() == sha
               for p, sha in sources.items()):
            if best is None or meta.stat().st_mtime > best[0]:
                best = (meta.stat().st_mtime, meta.parent)
    if best is None:
        raise SystemExit(f"api_parity: no unit data of {module} for this checkout (python -m oracle.unit.{module})")
    return best[1]


def window_sessions(aset: str | None) -> set[str]:
    """The names of the files of ``aset`` that have a session in both
    depth files of ``oracle/unit/js_api.py`` (the unit data of this
    checkout)."""
    if aset is None:
        return set()
    d = unit_data_dir("js_api")
    names: list[set[str]] = []
    for name, prefix in (("depth", f"depth-{aset}-"), ("depth-bytes", f"depth-bytes-{aset}-")):
        found = set()
        with open(d / f"{name}.jsonl", encoding="ascii") as f:
            for line in f:
                # the name is the first field of the session: no need to
                # parse the (deeply nested) line
                m = re.match(r'\{"in":\[\{"name":"([^"]+)"', line)
                if m and m.group(1).startswith(prefix):
                    found.add(m.group(1)[len(prefix) :])
        names.append(found)
    return names[0] & names[1]


def classify(stem: str, oracle: bytes, api: bytes | None, rust: bytes | None, windows: set[str], stage: str,
             headroom: Callable[[], bytes | None] = lambda: None) -> tuple[str, str]:
    """The class of a file and a note. ``headroom()``: Python's trace of
    the file with HEADROOM more frames of recursion limit (``None``: not
    available), asked only for a depth window whose first difference shows
    no ``RecursionError`` (pdfplumber caught it: the metadata, decoded in
    a ``try``)."""
    if api is None:
        return "defect", "the API's run crashed or ran out of time"
    if api == oracle:
        return "identical", ""
    if rust is not None and rust != oracle and api == rust:
        return "core", "API = pdftrace"
    a, b = _lines(oracle), _lines(api)
    if stage == "plumber_doc":
        _, gaps, k = align_filters(a, b)
        if k is None:
            return "filter", f"{gaps} filtered page(s)"
    if WINDOW_NAME.search(stem):
        if stem not in windows:
            return "defect", "a depth window without its sessions in depth.jsonl and depth-bytes.jsonl"
        aligned, gaps, k = align_filters(a, b, window=True) if stage == "plumber_doc" else (a, 0, _first(a, b))
        if k is None:
            return "window", f"{gaps} filtered page(s), a RecursionError on one side of each"
        la = aligned[k] if k < len(aligned) else ""
        lb = b[k] if k < len(b) else ""
        after = f" (after {gaps} filtered page(s))" if gaps else ""
        if RECURSION.search(la) or RECURSION.search(lb):
            return "window", f"line {k + 1}{after}: RecursionError on the {'oracle' if RECURSION.search(la) else 'API'} side"
        more = headroom()
        if more is not None:
            m = _lines(more)
            if gaps == 0 and m[: k + 1] == b[: k + 1]:
                return "window", f"line {k + 1}: Python's line with {HEADROOM} more frames of recursion limit"
        return "defect", f"a depth window whose first difference (line {k + 1}{after}) is not a RecursionError"
    return "defect", ""


def headroom_trace(stage: str, f: CorpusFile, max_pages: int, timeout: float) -> bytes | None:
    """Python's trace of ``f`` for ``stage`` with ``sys.getrecursionlimit()
    + HEADROOM`` (a fresh interpreter, as the cache builds its traces):
    where the trace then gives the API's line, the difference was the
    recursion limit's."""
    script = (
        "import sys\n"
        "root, extra, stage, path, max_pages, password = sys.argv[1:7]\n"
        "sys.path.insert(0, root)\n"
        "sys.setrecursionlimit(sys.getrecursionlimit() + int(extra))\n"
        "from oracle.stages import trace_text\n"
        "text = trace_text(stage, path, None if password == '-' else password[1:], int(max_pages))\n"
        "sys.stdout.buffer.write(text.encode('ascii'))\n"
    )
    password = "-" if f.password is None else "=" + f.password
    try:
        proc = subprocess.run([sys.executable, "-I", "-c", script, str(REPO_DIR), str(HEADROOM), stage, str(f.path),
                               str(max_pages), password], capture_output=True, timeout=timeout)
    except subprocess.TimeoutExpired:
        return None
    return proc.stdout if proc.returncode == 0 else None


def parse_args(argv: list[str] | None) -> argparse.Namespace:
    ap = argparse.ArgumentParser(prog="python -m oracle.api_parity", description=__doc__.split("\n\n")[0])
    ap.add_argument("--stage", required=True, choices=STAGES)
    ap.add_argument("--set", help=f"corpus set: {', '.join(SET_NAMES)}, adv-<name>")
    ap.add_argument("--files", nargs="+", metavar="PDF", help="explicit PDF files (of the set --set names, if any)")
    ap.add_argument("-j", "--jobs", type=int, default=DEFAULT_JOBS)
    ap.add_argument("--max-pages", type=int, default=DEFAULT_MAX_PAGES)
    ap.add_argument("--timeout", type=float, default=600.0, help="seconds per file (default 600)")
    ap.add_argument("--batch-size", type=int, default=DEFAULT_BATCH)
    ap.add_argument("--cmap-dir", type=Path)
    ap.add_argument("--json", action="store_true", help="a JSON summary on stdout")
    args = ap.parse_args(argv)
    if not args.set and not args.files:
        ap.error("--set or --files is required")
    return args


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    missing = [str(p.relative_to(REPO_DIR)) for p in (*JS_API_BUILT, DEFAULT_PDFTRACE) if not p.is_file()]
    if missing:
        print(f"api_parity: {', '.join(missing)} missing (node scripts/build.mjs; node scripts/build-wasm.mjs --oracle; "
              "cargo build --release -p pdftrace)", file=sys.stderr)
        return 2
    try:
        files = files_from_paths(args.files) if args.files else load_set(args.set)
    except CorpusError as e:
        print(f"api_parity: {e}", file=sys.stderr)
        return 2
    cmap_dir = cmap_provider(args.cmap_dir)
    if isinstance(cmap_dir, str):
        print(f"api_parity: {cmap_dir}", file=sys.stderr)
        return 2
    windows = window_sessions(args.set if args.set and args.set.startswith("adv-") else None)
    cache = OracleCache(args.stage, args.max_pages)
    oracle = cache.build(files, jobs=args.jobs, timeout=args.timeout, progress=not args.json)
    comparable = [r.file for r in oracle if r.status == "ok"]
    oracle_dir = cache.dir if oracle_input(args.stage) else None
    tmp_root = cache_dir() / "tmp"
    tmp_root.mkdir(parents=True, exist_ok=True)
    out_root = Path(tempfile.mkdtemp(prefix=f"api-parity-{args.stage}-", dir=tmp_root))
    records = []
    try:
        runs = {}
        for engine, binary in (("pdftrace", DEFAULT_PDFTRACE), ("js-api", JS_API_TRACE)):
            runs[engine] = run_rust(binary, args.stage, args.max_pages, comparable, out_root / engine, args.jobs,
                                    args.timeout, args.batch_size, oracle_dir, cmap_dir)
        for r in oracle:
            f = r.file
            rec: dict[str, object] = {"path": f.rel}
            if r.status != "ok":
                rec["class"] = f"oracle-{r.status}"
                records.append(rec)
                continue

            def output(engine: str) -> bytes | None:
                run = runs[engine]
                if f.sha256 in run.failures or f.sha256 in run.timeouts or f.sha256 not in run.outputs:
                    return None
                return run.outputs[f.sha256].read_bytes()

            expected = r.path.read_bytes()
            api, rust = output("js-api"), output("pdftrace")
            cls, note = classify(Path(f.rel).stem, expected, api, rust, windows, args.stage,
                                 lambda f=f: headroom_trace(args.stage, f, args.max_pages, args.timeout))
            rec["class"] = cls
            if note:
                rec["note"] = note
            if cls == "defect" and api is not None:
                rec["difference"] = first_difference(expected, api)
            if cls == "defect" and api is None:
                run = runs["js-api"]
                rec["reason"] = run.failures.get(f.sha256) or run.timeouts.get(f.sha256) or "no output"
            records.append(rec)
    finally:
        shutil.rmtree(out_root, ignore_errors=True)
    counts = {c: sum(1 for rec in records if rec["class"] == c) for c in CLASSES}
    counts["oracle-timeout"] = sum(1 for rec in records if rec["class"] == "oracle-timeout")
    counts["oracle-error"] = sum(1 for rec in records if rec["class"] == "oracle-error")
    counts["total"] = len(records)
    if args.json:
        print(json.dumps({"stage": args.stage, "set": args.set, "counts": counts,
                          "files": [rec for rec in records if rec["class"] != "identical"]}, indent=2))
    else:
        for rec in records:
            if rec["class"] not in ("identical",):
                extra = rec.get("note") or rec.get("reason") or ""
                print(f"{rec['class']:<15} {rec['path']}  {extra}")
                if "difference" in rec:
                    d = rec["difference"]
                    print(f"    line {d.get('line')}: oracle {str(d.get('oracle'))[:200]!r}")
                    print(f"    line {d.get('line')}:    api {str(d.get('rust'))[:200]!r}")
        print(f"api_parity {args.stage} on {args.set or 'files'}: " + " / ".join(f"{v} {k}" for k, v in counts.items()))
    # an oracle error (the oracle of adv-interp's 096-text-TL.pdf beyond
    # 5 GB) leaves its file unverified, listed above: not the API's
    return 1 if counts["defect"] else 0


if __name__ == "__main__":
    sys.exit(main())
