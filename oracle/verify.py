"""Compare the Rust ``pdftrace`` with the Python oracle, file by file.

    python -m oracle.verify --stage S --set NAME [-j N] [--max-pages N]
                            [--pdftrace PATH] [--limit K] [--files F ...]
                            [--json] [--oracle-only]

1. Lists the files of the set (``oracle/corpus.py``), or ``--files``; keeps the
   first ``--limit`` ones.
2. Builds the missing oracle traces in the cache (``oracle/cache.py``):
   ``-j`` worker processes (default 3), ``--timeout`` seconds per file
   (default 120, recorded as ``!Timeout``; ``--retry-timeouts`` rebuilds them).
3. Runs the Rust binary (default ``<repo>/target/release/pdftrace``) in
   batches of files sharing a password::

       pdftrace --stage S --max-pages N [--password P] --out DIR F1.pdf F2.pdf ...

   which writes ``DIR/<sha256 of the PDF>.txt`` for each file. Exit status 2
   means "stage not implemented" and stops verify (status 2). If a batch fails
   otherwise (non-zero status, signal, time limit of ``--timeout`` per file),
   each of its files is run again alone, and a file that still fails is
   counted as ``rust-crashed``.
4. Compares the traces byte for byte and prints the counts (identical /
   different / rust-crashed / oracle-timeout / oracle-error / total) and, for
   each differing file, its path and the first differing line with 3 lines of
   context from each side. ``--json`` prints a machine-readable summary
   instead.

Exit status: 0 if every compared file is identical (oracle timeouts are
excluded), 1 otherwise (difference, Rust crash, oracle error), 2 if the stage
is not implemented on the Rust side or the binary is missing.

``--oracle-only`` stops after step 2 and reports the oracle build (counts,
time, slowest files); it is how the oracle cache is warmed.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
from pathlib import Path

from .cache import DEFAULT_JOBS, DEFAULT_TIMEOUT, OracleCache, OracleResult
from .corpus import REPO_DIR, SET_NAMES, CorpusFile, cache_dir, files_from_paths, load_set
from .stages import DEFAULT_MAX_PAGES, available_stages

DEFAULT_PDFTRACE = REPO_DIR / "target" / "release" / "pdftrace"
DEFAULT_BATCH = 64
CONTEXT = 3
SHOW_WIDTH = 240

STATUSES = ("identical", "different", "rust-crashed", "oracle-timeout", "oracle-error")


class StageNotImplemented(Exception):
    pass


@dataclass
class RustRun:
    outputs: dict[str, Path] = field(default_factory=dict)  # sha256 -> trace file
    failures: dict[str, str] = field(default_factory=dict)  # sha256 -> reason


# ------------------------------------------------------------------------- Rust side


def _run_pdftrace(pdftrace: Path, stage: str, max_pages: int, password: str | None, files: list[CorpusFile], out: Path, timeout: float) -> tuple[int | None, str]:
    cmd = [str(pdftrace), "--stage", stage, "--max-pages", str(max_pages)]
    if password is not None:
        cmd += ["--password", password]
    cmd += ["--out", str(out), *(str(f.path) for f in files)]
    try:
        proc = subprocess.run(cmd, capture_output=True, timeout=timeout * len(files))
    except subprocess.TimeoutExpired:
        return None, f"time limit ({timeout:g} s per file) exceeded"
    stderr = proc.stderr.decode("utf-8", "replace").strip().splitlines()
    return proc.returncode, " | ".join(stderr[-5:])


def run_rust(pdftrace: Path, stage: str, max_pages: int, files: list[CorpusFile], out_root: Path, jobs: int, timeout: float, batch_size: int) -> RustRun:
    """Run pdftrace over ``files``; raise StageNotImplemented on exit status 2."""
    groups: dict[str | None, list[CorpusFile]] = {}
    for f in files:
        groups.setdefault(f.password, []).append(f)
    batches = [(pw, fs[i : i + batch_size]) for pw, fs in groups.items() for i in range(0, len(fs), batch_size)]
    result = RustRun()
    counter = iter(range(1 << 30))

    def one_batch(item: tuple[str | None, list[CorpusFile]]) -> None:
        password, batch = item
        out = out_root / f"batch-{next(counter)}"
        out.mkdir(parents=True)
        code, err = _run_pdftrace(pdftrace, stage, max_pages, password, batch, out, timeout)
        if code == 2:
            raise StageNotImplemented(err or f"pdftrace: stage {stage!r} not implemented")
        if code == 0:
            for f in batch:
                p = out / f"{f.sha256}.txt"
                if p.is_file():
                    result.outputs[f.sha256] = p
                else:
                    result.failures[f.sha256] = f"exit 0 but no {p.name}"
            return
        if len(batch) == 1:
            result.failures[batch[0].sha256] = f"exit {code}: {err}" if code is not None else err
            return
        for f in batch:  # isolate the culprit(s): every file of the batch alone
            one_batch((password, [f]))

    with ThreadPoolExecutor(max_workers=max(1, jobs)) as pool:
        for _ in pool.map(one_batch, batches):
            pass
    return result


# ------------------------------------------------------------------------ comparison


def _split(data: bytes) -> list[str]:
    return data.decode("utf-8", "replace").split("\n")


def _clip(s: str, col: int | None = None) -> str:
    if len(s) <= SHOW_WIDTH:
        return s
    if col is None or col < SHOW_WIDTH - 40:
        return s[:SHOW_WIDTH] + f"... [{len(s)} chars]"
    start = col - SHOW_WIDTH // 2
    return f"[col {start}] ..." + s[start : start + SHOW_WIDTH] + f"... [{len(s)} chars]"


def first_difference(oracle: bytes, rust: bytes) -> dict[str, object]:
    """Locate the first differing line (1-based) and column (0-based)."""
    a, b = _split(oracle), _split(rust)
    n = min(len(a), len(b))
    i = next((k for k in range(n) if a[k] != b[k]), n)
    la = a[i] if i < len(a) else None
    lb = b[i] if i < len(b) else None
    col = None
    if la is not None and lb is not None:
        col = next((k for k in range(min(len(la), len(lb))) if la[k] != lb[k]), min(len(la), len(lb)))
    lo, hi = max(0, i - CONTEXT), i + CONTEXT + 1
    return {
        "line": i + 1,
        "column": col,
        "oracle": [_clip(x, col if j == i else None) for j, x in enumerate(a[lo:hi], lo)],
        "rust": [_clip(x, col if j == i else None) for j, x in enumerate(b[lo:hi], lo)],
        "first_context_line": lo + 1,
        "oracle_lines": len(a) - (1 if a and a[-1] == "" else 0),
        "rust_lines": len(b) - (1 if b and b[-1] == "" else 0),
    }


def _format_difference(f: CorpusFile, d: dict[str, object]) -> str:
    out = [f"--- {f.rel}  (sha256 {f.sha256[:16]}, first difference at line {d['line']}" + (f", column {d['column']}" if d["column"] is not None else "") + ")"]
    first = int(d["first_context_line"])  # type: ignore[arg-type]
    for side in ("oracle", "rust"):
        out.append(f"  {side}:")
        for k, text in enumerate(d[side], first):  # type: ignore[arg-type]
            mark = ">" if k == d["line"] else " "
            out.append(f"  {mark}{k:>6} {text}")
        if first + len(d[side]) <= int(d["line"]):  # type: ignore[arg-type]
            out.append(f"  >{d['line']:>6} <end of file>")
    return "\n".join(out)


# ------------------------------------------------------------------------------ main


def parse_args(argv: list[str] | None) -> argparse.Namespace:
    ap = argparse.ArgumentParser(prog="python -m oracle.verify", description=__doc__.split("\n\n")[0])
    ap.add_argument("--stage", required=True, choices=available_stages())
    src = ap.add_mutually_exclusive_group(required=True)
    src.add_argument("--set", help=f"corpus set: {', '.join(SET_NAMES)}, adv-<stage>")
    src.add_argument("--files", nargs="+", metavar="PDF", help="explicit PDF files")
    ap.add_argument("-j", "--jobs", type=int, default=DEFAULT_JOBS, help="parallel oracle processes and pdftrace batches (default 3)")
    ap.add_argument("--max-pages", type=int, default=DEFAULT_MAX_PAGES, help="pages per document for paged stages (0 = all; default 20)")
    ap.add_argument("--pdftrace", type=Path, default=Path(os.environ.get("PDFTRACE", DEFAULT_PDFTRACE)), help="Rust trace binary")
    ap.add_argument("--limit", type=int, help="only the first K files of the set")
    ap.add_argument("--timeout", type=float, default=DEFAULT_TIMEOUT, help="seconds per file (oracle and pdftrace; default 120)")
    ap.add_argument("--retry-timeouts", action="store_true", help="rebuild oracle traces cached as !Timeout")
    ap.add_argument("--batch-size", type=int, default=DEFAULT_BATCH, help="files per pdftrace invocation (default 64)")
    ap.add_argument("--json", action="store_true", help="print a JSON summary on stdout instead of the report")
    ap.add_argument("--oracle-only", action="store_true", help="only build the oracle cache and report on it")
    ap.add_argument("--keep", action="store_true", help="keep the pdftrace output directory")
    return ap.parse_args(argv)


def _oracle_report(args: argparse.Namespace, cache: OracleCache, results: list[OracleResult], seconds: float) -> int:
    counts = {s: sum(1 for r in results if r.status == s) for s in ("ok", "timeout", "error")}
    built = [r for r in results if r.seconds is not None]
    summary = {
        "stage": args.stage,
        "set": args.set,
        "max_pages": args.max_pages,
        "fingerprint": cache.fingerprint,
        "cache_dir": str(cache.dir),
        "counts": {**counts, "total": len(results)},
        "built": len(built),
        "build_seconds_cpu": round(sum(r.seconds for r in built), 1),  # type: ignore[misc]
        "wall_seconds": round(seconds, 1),
        "slowest": [
            {"path": r.file.rel, "seconds": round(r.seconds, 2), "status": r.status}  # type: ignore[arg-type]
            for r in sorted(built, key=lambda r: -r.seconds)[:10]  # type: ignore[operator]
        ],
        "timeouts": [r.file.rel for r in results if r.status == "timeout"],
        "errors": [{"path": r.file.rel, "error": r.error} for r in results if r.status == "error"],
    }
    if args.json:
        print(json.dumps(summary, indent=2))
    else:
        print(f"oracle {args.stage} [{cache.fingerprint}] {len(results)} files: {counts['ok']} ok, {counts['timeout']} timeout, {counts['error']} error")
        print(f"built {len(built)} traces in {summary['wall_seconds']} s wall, {summary['build_seconds_cpu']} s summed")
        for s in summary["slowest"]:  # type: ignore[union-attr]
            print(f"  {s['seconds']:>8.2f} s  {s['status']:<7} {s['path']}")
        for path in summary["timeouts"]:  # type: ignore[union-attr]
            print(f"  timeout: {path}")
        for e in summary["errors"]:  # type: ignore[union-attr]
            print(f"  ERROR: {e['path']}: {e['error']}")
    return 1 if counts["error"] else 0


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    files = load_set(args.set) if args.set else files_from_paths(args.files)
    if args.limit is not None:
        files = files[: args.limit]
    cache = OracleCache(args.stage, args.max_pages)
    started = time.monotonic()
    oracle = cache.build(files, jobs=args.jobs, timeout=args.timeout, retry_timeouts=args.retry_timeouts, progress=not args.json)
    if args.oracle_only:
        return _oracle_report(args, cache, oracle, time.monotonic() - started)

    if not (args.pdftrace.is_file() and os.access(args.pdftrace, os.X_OK)):
        print(f"verify: no executable pdftrace at {args.pdftrace} (build it: cargo build --release -p pdftrace)", file=sys.stderr)
        return 2
    comparable = [r.file for r in oracle if r.status == "ok"]
    tmp_root = cache_dir() / "tmp"
    tmp_root.mkdir(parents=True, exist_ok=True)
    out_root = Path(tempfile.mkdtemp(prefix=f"pdftrace-{args.stage}-", dir=tmp_root))
    try:
        try:
            rust = run_rust(args.pdftrace, args.stage, args.max_pages, comparable, out_root, args.jobs, args.timeout, args.batch_size)
        except StageNotImplemented as e:
            print(f"verify: pdftrace does not implement stage {args.stage!r} (exit status 2): {e}", file=sys.stderr)
            return 2
        records = []
        for r in oracle:
            f = r.file
            rec: dict[str, object] = {"path": f.rel, "sha256": f.sha256}
            if r.status == "timeout":
                rec["status"] = "oracle-timeout"
            elif r.status == "error":
                rec["status"] = "oracle-error"
                rec["reason"] = r.error
            elif f.sha256 in rust.failures:
                rec["status"] = "rust-crashed"
                rec["reason"] = rust.failures[f.sha256]
            else:
                expected = r.path.read_bytes()
                actual = rust.outputs[f.sha256].read_bytes()
                if expected == actual:
                    rec["status"] = "identical"
                else:
                    rec["status"] = "different"
                    rec["difference"] = first_difference(expected, actual)
            records.append((f, rec))
    finally:
        if args.keep:
            print(f"verify: pdftrace outputs kept in {out_root}", file=sys.stderr)
        else:
            shutil.rmtree(out_root, ignore_errors=True)

    counts = {s: sum(1 for _, rec in records if rec["status"] == s) for s in STATUSES}
    counts["total"] = len(records)
    failed = counts["different"] + counts["rust-crashed"] + counts["oracle-error"]
    if args.json:
        summary = {
            "stage": args.stage,
            "set": args.set,
            "max_pages": args.max_pages,
            "fingerprint": cache.fingerprint,
            "counts": counts,
            "files": [rec for _, rec in records if rec["status"] != "identical"],
        }
        print(json.dumps(summary, indent=2))
    else:
        for f, rec in records:
            if rec["status"] == "different":
                print(_format_difference(f, rec["difference"]))  # type: ignore[arg-type]
            elif rec["status"] in ("rust-crashed", "oracle-error"):
                print(f"--- {f.rel}  (sha256 {f.sha256[:16]}) {rec['status']}: {rec.get('reason', '')}")
        timeouts = [f.rel for f, rec in records if rec["status"] == "oracle-timeout"]
        for path in timeouts:
            print(f"    oracle timeout (excluded): {path}")
        print(
            f"{args.stage} on {args.set or 'files'}: {counts['identical']} identical / {counts['different']} different / "
            f"{counts['rust-crashed']} rust-crashed / {counts['oracle-timeout']} oracle-timeout / "
            f"{counts['oracle-error']} oracle-error / {counts['total']} total"
        )
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
