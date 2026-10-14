"""Cache of oracle traces.

Layout::

    <cache>/oracle/<stage>/<fingerprint>/<pdf sha256>.txt
    <cache>/oracle/<stage>/<fingerprint>/<pdf sha256>.pw-<12 hex>.txt   (with a password)
    <cache>/oracle/<stage>/<fingerprint>/fingerprint.json              (what was hashed)
    <cache>/oracle/<stage>/<fingerprint>/_builds.jsonl                 (one record per build)

``<cache>`` is ``PDFPLUMBER_CACHE`` or ``<PDFPLUMBER_ROOT>/.cache``.

The fingerprint (16 hex digits of a SHA-256) covers everything that decides a
trace: the sources of the stage module and of the stages it ``DEPENDS`` on,
of ``stages/__init__.py`` (runner), ``stages/_common.py``, ``canon.py`` and
``typed_json.py``; the Python, pdfminer.six and pdfplumber versions; and
``max_pages`` for a paged stage.
When a password is used, its SHA-256 is part of the file name, so changing a
file's password in a manifest rebuilds its trace.

Each trace is built in its own process (``python -m oracle.trace``, same
interpreter), ``jobs`` at a time, so a crash or a runaway file cannot take the
others down. A file that does not finish within ``timeout`` seconds gets the
trace ``!Timeout`` (cached; ``retry_timeouts`` rebuilds them); verify.py
excludes and counts these. A process that fails (non-zero exit) is a harness
error: nothing is cached and the error is reported. Writes are atomic
(temporary file in the same directory, then ``os.replace``).
"""

from __future__ import annotations

import hashlib
import json
import os
import platform
import subprocess
import sys
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
from pathlib import Path

from .corpus import REPO_DIR, CorpusFile, cache_dir
from .stages import get_stage, stage_source_files

TIMEOUT_TEXT = "!Timeout\n"
DEFAULT_TIMEOUT = 120.0
DEFAULT_JOBS = 3


def versions() -> dict[str, str]:
    import pdfminer
    import pdfplumber

    return {
        "python": platform.python_implementation() + " " + sys.version.split()[0],
        "pdfminer.six": pdfminer.__version__,
        "pdfplumber": pdfplumber.__version__,
    }


def fingerprint_inputs(stage: str, max_pages: int) -> dict[str, object]:
    module = get_stage(stage)
    sources = {}
    for path in stage_source_files(stage):
        sources[path.relative_to(REPO_DIR).as_posix()] = hashlib.sha256(path.read_bytes()).hexdigest()
    inputs: dict[str, object] = {"stage": stage, "sources": sources, "versions": versions()}
    if getattr(module, "PAGED", True):
        inputs["max_pages"] = max_pages
    return inputs


def fingerprint(stage: str, max_pages: int) -> str:
    blob = json.dumps(fingerprint_inputs(stage, max_pages), sort_keys=True).encode()
    return hashlib.sha256(blob).hexdigest()[:16]


@dataclass
class OracleResult:
    file: CorpusFile
    path: Path  # cache file
    status: str  # "ok" | "timeout" | "error"
    seconds: float | None = None  # build time, None when read from the cache
    error: str = ""


class OracleCache:
    def __init__(self, stage: str, max_pages: int) -> None:
        self.stage = stage
        self.max_pages = max_pages
        self.fingerprint = fingerprint(stage, max_pages)
        self.dir = cache_dir() / "oracle" / stage / self.fingerprint
        self._log_lock = threading.Lock()

    def path_for(self, f: CorpusFile) -> Path:
        if f.password is None:
            return self.dir / f"{f.sha256}.txt"
        pw = hashlib.sha256(f.password.encode("utf-8", "surrogatepass")).hexdigest()[:12]
        return self.dir / f"{f.sha256}.pw-{pw}.txt"

    def _ensure_dir(self) -> None:
        self.dir.mkdir(parents=True, exist_ok=True)
        meta = self.dir / "fingerprint.json"
        if not meta.exists():
            _atomic_write(meta, json.dumps(fingerprint_inputs(self.stage, self.max_pages), indent=2, sort_keys=True).encode())

    def lookup(self, f: CorpusFile) -> OracleResult | None:
        path = self.path_for(f)
        try:
            with open(path, "rb") as fh:
                head = fh.read(len(TIMEOUT_TEXT) + 1)
        except FileNotFoundError:
            return None
        status = "timeout" if head == TIMEOUT_TEXT.encode() else "ok"
        return OracleResult(f, path, status)

    def build(
        self,
        files: list[CorpusFile],
        jobs: int = DEFAULT_JOBS,
        timeout: float = DEFAULT_TIMEOUT,
        retry_timeouts: bool = False,
        progress: bool = True,
    ) -> list[OracleResult]:
        """Make sure every file has a trace; return one result per file, in order."""
        results: list[OracleResult | None] = []
        todo: list[tuple[int, CorpusFile]] = []
        for i, f in enumerate(files):
            hit = self.lookup(f)
            if hit is None or (retry_timeouts and hit.status == "timeout"):
                todo.append((i, f))
                results.append(None)
            else:
                results.append(hit)
        if todo:
            self._ensure_dir()
            done = 0
            started = time.monotonic()
            lock = threading.Lock()

            def work(item: tuple[int, CorpusFile]) -> None:
                nonlocal done
                i, f = item
                results[i] = self._build_one(f, timeout)
                with lock:
                    done += 1
                    if progress and (done % 25 == 0 or done == len(todo)):
                        elapsed = time.monotonic() - started
                        print(f"oracle {self.stage}: {done}/{len(todo)} built ({elapsed:.0f} s)", file=sys.stderr, flush=True)

            with ThreadPoolExecutor(max_workers=max(1, jobs)) as pool:
                list(pool.map(work, todo))
        return [r for r in results if r is not None]

    def _build_one(self, f: CorpusFile, timeout: float) -> OracleResult:
        path = self.path_for(f)
        cmd = [sys.executable, "-m", "oracle.trace", "--stage", self.stage, "--max-pages", str(self.max_pages)]
        if f.password is not None:
            cmd += ["--password", f.password]
        cmd.append(str(f.path))
        env = dict(os.environ)
        env["PYTHONPATH"] = str(REPO_DIR) + (os.pathsep + env["PYTHONPATH"] if env.get("PYTHONPATH") else "")
        started = time.monotonic()
        try:
            proc = subprocess.run(cmd, env=env, capture_output=True, timeout=timeout, cwd=REPO_DIR)
        except subprocess.TimeoutExpired:
            seconds = time.monotonic() - started
            _atomic_write(path, TIMEOUT_TEXT.encode())
            self._log(f, "timeout", seconds, 0)
            return OracleResult(f, path, "timeout", seconds)
        seconds = time.monotonic() - started
        if proc.returncode != 0:
            tail = proc.stderr.decode("utf-8", "replace").strip().splitlines()[-5:]
            self._log(f, "error", seconds, 0)
            return OracleResult(f, path, "error", seconds, f"exit {proc.returncode}: " + " | ".join(tail))
        _atomic_write(path, proc.stdout)
        self._log(f, "ok", seconds, len(proc.stdout))
        return OracleResult(f, path, "ok", seconds)

    def _log(self, f: CorpusFile, status: str, seconds: float, size: int) -> None:
        record = {"sha256": f.sha256, "path": f.rel, "status": status, "seconds": round(seconds, 3), "bytes": size}
        with self._log_lock, open(self.dir / "_builds.jsonl", "a") as fh:
            fh.write(json.dumps(record) + "\n")


def _atomic_write(path: Path, data: bytes) -> None:
    tmp = path.with_name(f".{path.name}.{os.getpid()}.{threading.get_ident()}.tmp")
    with open(tmp, "wb") as fh:
        fh.write(data)
    os.replace(tmp, path)
