"""Check the upstream ``file:line`` references of the port.

Every ported function carries a comment naming the upstream code it ports
(``docs/architecture.md``, "Règles de port")::

    // pdfminer/pdfinterp.py:276 PDFContentParser.seek
    /// pdfminer/psparser.py:161 `PSBaseParser` and :522 `PSStackParser`, ...
    // pdfminer/pdfinterp.py:327-329 PDFContentParser.KEYWORD_BI, ... (class attributes)

    python -m oracle.upstream_refs [PATH ...]

scans the Rust sources and the oracle modules (``crates/``, ``oracle/``,
default) or the given files, and checks
each ``pdfminer/<module>.py:<line>`` / ``pdfplumber/<module>.py:<line>``
reference against the upstream sources the oracle runs
(``upstream/pdfminer-six``, ``upstream/pdfplumber-python``, read under
``PDFPLUMBER_ROOT``): the file must exist, the line (or ``<first>-<last>``
range) must be in it, and it must be followed by the qualified name of what
is there (``Class.method``, ``function``, ``CONSTANT``, ``Class.ATTRIBUTE``;
optionally in backquotes): the ``def``, ``class`` or assignment of that name
must be on that line. A range lists every name it covers, separated by commas
(``:327-329 C.A, C.B, C.C``), each defined within the range. Continuations
``+ :523 Name`` and ``and :522 Name`` refer to the same file.

Exit status 1 if a reference is wrong (each is listed with the place where
the name actually is, when it exists elsewhere in the file).
"""

from __future__ import annotations

import argparse
import ast
import functools
import re
import sys
from dataclasses import dataclass
from pathlib import Path

from .corpus import REPO_DIR, corpus_root

UPSTREAM = {
    "pdfminer": Path("upstream/pdfminer-six/pdfminer"),
    "pdfplumber": Path("upstream/pdfplumber-python/pdfplumber"),
}

_NAME = r"`{0,2}[A-Za-z_][\w.]*\w`{0,2}"
_NAMES = r"(?P<names>" + _NAME + r"(?:,[ \t]*" + _NAME + r")*)"
REF_RE = re.compile(
    r"(?P<pkg>pdfminer|pdfplumber)/(?P<file>[\w/]+\.py):(?P<first>\d+)(?:-(?P<last>\d+))?(?:[ \t]+" + _NAMES + r")?"
)
CONT_RE = re.compile(r"[ \t]*(?:\+|and)[ \t]*:(?P<first>\d+)[ \t]+" + _NAMES)


@dataclass(frozen=True)
class Definition:
    qualname: str
    line: int


@functools.lru_cache(maxsize=None)
def definitions(path: Path) -> tuple[tuple[Definition, ...], int]:
    """The ``def``/``class``/assignment names of a module, and its line count."""
    source = path.read_text(encoding="utf-8")
    tree = ast.parse(source)
    found: list[Definition] = []

    def visit(body: list[ast.stmt], prefix: str) -> None:
        for node in body:
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
                found.append(Definition(prefix + node.name, node.lineno))
                if isinstance(node, ast.ClassDef):
                    visit(node.body, prefix + node.name + ".")
            elif isinstance(node, (ast.Assign, ast.AnnAssign)):
                targets = node.targets if isinstance(node, ast.Assign) else [node.target]
                for target in targets:
                    for name in ast.walk(target):
                        if isinstance(name, ast.Name):
                            found.append(Definition(prefix + name.id, node.lineno))
            elif isinstance(node, (ast.If, ast.Try)):  # module-level `if TYPE_CHECKING:`, try/except imports
                visit(node.body, prefix)
                visit(getattr(node, "orelse", []), prefix)

    visit(tree.body, "")
    return tuple(found), len(source.splitlines())


def _matches(defn: Definition, name: str) -> bool:
    return defn.qualname == name


@dataclass
class Problem:
    where: str
    reference: str
    message: str

    def __str__(self) -> str:
        return f"{self.where}: {self.reference}: {self.message}"


def _split_names(names: str | None) -> list[str]:
    return [n.strip().strip("`") for n in names.split(",")] if names else []


def check_reference(root: Path, pkg: str, file: str, first: int, last: int | None, names: list[str]) -> str | None:
    """Why the reference is wrong, or None."""
    path = root / UPSTREAM[pkg] / file
    if not path.is_file():
        return f"no file {path.relative_to(root)}"
    defs, nlines = definitions(path)
    last_line = last if last is not None else first
    if not (1 <= first <= last_line <= nlines):
        return f"line {first}{'-' + str(last) if last else ''} is outside {pkg}/{file} ({nlines} lines)"
    lines = range(first, last_line + 1)
    at = [d.qualname for d in defs if d.line in lines]
    if not names:
        return "no name after the reference" + (f" (it points at {', '.join(at)})" if at else "")
    if last is None:
        names = names[:1]  # a single line names one thing; a comma after it is prose
    for name in names:
        if any(_matches(d, name) and d.line in lines for d in defs):
            continue
        elsewhere = sorted({d.line for d in defs if _matches(d, name)})
        if elsewhere:
            return f"{name} is at line {', '.join(map(str, elsewhere))}, not {first}" + (f"-{last}" if last else "")
        return f"no definition of {name} in {pkg}/{file}" + (f" ({first} defines {', '.join(at)})" if at else "")
    return None


def check_text(root: Path, text: str, where: str) -> list[Problem]:
    problems = []
    for lineno, line in enumerate(text.splitlines(), 1):
        for m in REF_RE.finditer(line):
            refs = [(int(m["first"]), int(m["last"]) if m["last"] else None, _split_names(m["names"]), m.group(0))]
            pos = m.end()
            while (c := CONT_RE.match(line, pos)) is not None:
                refs.append((int(c["first"]), None, _split_names(c["names"]), c.group(0).strip()))
                pos = c.end()
            for first, last, names, shown in refs:
                why = check_reference(root, m["pkg"], m["file"], first, last, names)
                if why is not None:
                    problems.append(Problem(f"{where}:{lineno}", shown, why))
    return problems


def default_paths() -> list[Path]:
    """The Rust sources and the oracle's own modules (stage docstrings cite upstream lines too)."""
    rust = sorted((REPO_DIR / "crates").rglob("*.rs"))
    oracle = sorted(p for p in (REPO_DIR / "oracle").rglob("*.py") if "tests" not in p.relative_to(REPO_DIR).parts)
    return rust + oracle


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(prog="python -m oracle.upstream_refs", description=__doc__.split("\n\n")[0])
    ap.add_argument("paths", nargs="*", type=Path, help="files to check (default: crates/**/*.rs and oracle/**/*.py)")
    args = ap.parse_args(argv)
    root = corpus_root()
    paths = args.paths or default_paths()
    problems: list[Problem] = []
    count = 0
    for path in paths:
        text = path.read_text(encoding="utf-8")
        count += sum(1 for line in text.splitlines() for _ in REF_RE.finditer(line))
        rel = path.resolve().relative_to(REPO_DIR) if path.resolve().is_relative_to(REPO_DIR) else path
        problems += check_text(root, text, str(rel))
    for p in problems:
        print(p)
    print(f"upstream references: {count} checked, {len(problems)} wrong", file=sys.stderr)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
