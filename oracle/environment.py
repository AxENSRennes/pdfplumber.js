"""What decides an oracle result besides the oracle's own sources.

A trace or a unit case is a function of the code that ran: the ``oracle/``
modules (hashed separately by their users), the interpreter, the pdfminer and
pdfplumber code actually installed (an editable install or a local edit does
not change a version string), the other packages they call (``cryptography``
writes the messages of AES errors, ``charset-normalizer``...), and native
libraries whose output reaches traces: zlib (error messages and the partial
output of ``decompress_corrupted``) and the C library (``float.__pow__`` calls
glibc's ``pow``, whose x86-64 build picks an FMA variant when the CPU has
FMA and AVX2, see ``crates/pycompat/src/libm.rs``).

``environment()`` describes all of this as a JSON-able dict. The trace cache
(``oracle/cache.py``) puts it in its fingerprint and the unit generators
(``oracle/unit/_common.py``) in their ``meta.json``, so that a change of any
of it invalidates the results instead of silently keeping stale ones.
"""

from __future__ import annotations

import functools
import hashlib
import importlib.metadata
import platform
import sys
import zlib
from pathlib import Path
from typing import Any

__all__ = ["environment", "package_files_sha256"]


def package_files_sha256(directory: Path) -> str:
    """SHA-256 over the relative paths and contents of every file of a
    package directory (compiled ``__pycache__`` files excluded)."""
    h = hashlib.sha256()
    for path in sorted(p for p in directory.rglob("*") if p.is_file() and "__pycache__" not in p.parts):
        h.update(path.relative_to(directory).as_posix().encode() + b"\0")
        h.update(hashlib.sha256(path.read_bytes()).digest())
    return h.hexdigest()


def _cpu_flags() -> list[str]:
    """The CPU features that select a glibc libm variant (Linux only)."""
    try:
        text = Path("/proc/cpuinfo").read_text(encoding="ascii", errors="replace")
    except OSError:
        return []
    for line in text.splitlines():
        if line.startswith(("flags", "Features")):
            flags = set(line.partition(":")[2].split())
            return sorted(flags & {"fma", "fma4", "avx2", "asimd"})
    return []


@functools.lru_cache(maxsize=1)
def environment() -> dict[str, Any]:
    import pdfminer
    import pdfplumber

    return {
        "python": platform.python_implementation() + " " + sys.version.split()[0],
        "packages": dict(sorted((d.metadata["Name"], d.version) for d in importlib.metadata.distributions())),
        "pdfminer_files": package_files_sha256(Path(pdfminer.__file__).parent),
        "pdfplumber_files": package_files_sha256(Path(pdfplumber.__file__).parent),
        "zlib": {"version": zlib.ZLIB_VERSION, "runtime": zlib.ZLIB_RUNTIME_VERSION},
        "libc": " ".join(platform.libc_ver()),
        "machine": platform.machine(),
        "cpu": _cpu_flags(),
    }
