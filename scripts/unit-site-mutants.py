#!/usr/bin/env python3
"""Hand mutants of the C-unit sites (docs/verification.md, "Mutation Testing",
"Hand Mutants"): each site added since a base revision is
removed in turn (an ``AttrSite`` load becomes its getter, ``page_load`` its
getter, ``kind_list`` the list without the load, ``getattr`` the attribute,
an ``eval_entry``/``method_call``/``builtin_call``/``plain`` statement
nothing), the tree is rebuilt and the checks that can see it run:
``oracle.verify`` of the four ``plumber_*`` stages on the ``climit-*``
windows of ``adv-interp`` and ``adv-plumber`` (library and stage code),
``js_runtime_unit`` (library and boundary code). cargo-mutants mutates
none of these.

    PDFPLUMBER_ROOT=<main checkout> HEAVY_SLOTS=1 scripts/heavy.sh \\
      python3 scripts/unit-site-mutants.py --base <rev> --out <tsv> [--list] [--only N,...] [--no-verify]

One line per mutant appended to ``--out`` (index, file, line, kind,
verdict, detail, time); indices already there are skipped. The working
tree is restored after each mutant (do not edit or commit meanwhile).
"""
from __future__ import annotations

import argparse
import os
import re
import subprocess
import time
from pathlib import Path

WT = Path(__file__).resolve().parent.parent
FILES = [
    "crates/pdfplumber/src/page.rs", "crates/pdfplumber/src/container.rs", "crates/pdfplumber/src/pdf.rs",
    "crates/pdfplumber/src/structure.rs", "crates/pdfplumber/src/table.rs",
    "crates/pdftrace/src/stages/plumber_objects.rs", "crates/pdftrace/src/stages/plumber_text.rs",
    "crates/pdftrace/src/stages/plumber_tables.rs", "crates/pdftrace/src/stages/plumber_doc.rs",
    "crates/pdftrace/src/stages/plumber_common.rs",
    "crates/wasm/src/pages.rs", "crates/wasm/src/ops.rs",
]
STAGES = ("plumber_objects", "plumber_text", "plumber_tables", "plumber_doc")


def root() -> Path:
    return Path(os.environ["PDFPLUMBER_ROOT"])


def env() -> dict[str, str]:
    return dict(os.environ, PATH=os.path.expanduser("~/.cargo/bin") + ":" + os.environ["PATH"],
                CARGO_BUILD_JOBS=os.environ.get("CARGO_BUILD_JOBS", "2"), PYTHONPATH=str(WT))


def added_lines(base: str, path: str) -> set[int]:
    out = subprocess.run(["git", "-C", str(WT), "diff", "-U0", base, "--", path], capture_output=True, text=True,
                         check=True).stdout
    lines: set[int] = set()
    for m in re.finditer(r"^@@ -\S+ \+(\d+)(?:,(\d+))? @@", out, re.M):
        start, n = int(m.group(1)), int(m.group(2) or 1)
        lines.update(range(start, start + n))
    return lines


def matching_paren(text: str, i: int) -> int:
    """Index of the ``)`` closing the ``(`` at ``i``."""
    depth = 0
    for j in range(i, len(text)):
        if text[j] == "(":
            depth += 1
        elif text[j] == ")":
            depth -= 1
            if depth == 0:
                return j
    raise ValueError("unbalanced")


def top_comma(s: str) -> int:
    depth = 0
    for j, c in enumerate(s):
        if c in "([{":
            depth += 1
        elif c in ")]}":
            depth -= 1
        elif c == "," and depth == 0:
            return j
    return -1


def mutants(base: str) -> list[tuple[str, int, str, int, int, str]]:
    """``(file, line, kind, start, end, replacement)`` of each site."""
    out = []
    for path in FILES:
        text = (WT / path).read_text()
        added = added_lines(base, path)
        line_of = lambda i: text.count("\n", 0, i) + 1  # noqa: E731
        # SITE.get(class, || expr) -> (expr)
        for m in re.finditer(r"(?:\b\w+::)*\b([A-Za-z_][A-Za-z0-9_]*)\.get\(", text):
            if line_of(m.start()) not in added:
                continue
            open_i = m.end() - 1
            close_i = matching_paren(text, open_i)
            inner = text[open_i + 1:close_i]
            body = inner[top_comma(inner) + 1:].strip()
            if body.startswith("||"):
                out.append((path, line_of(m.start()), "attr-site " + m.group(1), m.start(), close_i + 1,
                            "(" + body[2:].strip() + ")"))
        # page_load(page, &SITE, || expr) -> (expr)
        for m in re.finditer(r"\bpage_load\(page, &([A-Z0-9_]+), ", text):
            if line_of(m.start()) not in added:
                continue
            close_i = matching_paren(text, m.end() - len("page, &" + m.group(1) + ", ") - 1)
            inner = text[m.end():close_i].strip()
            out.append((path, line_of(m.start()), "page-load " + m.group(1), m.start(), close_i + 1,
                        "(" + inner[2:].strip() + ")"))
        # self.kind_list("kind", &SITE) -> the list without the load's units
        for m in re.finditer(r'self\.kind_list\("(\w+)", &([A-Z_]+)\)', text):
            new = '{ let _frame = py_frame()?; Ok(self.objects()?.get_or_empty("%s")) }' % m.group(1)
            out.append((path, line_of(m.start()), "kind-list " + m.group(2), m.start(), m.end(), new))
        # getattr(obj, &name, &SITE) -> the attribute
        for m in re.finditer(r"(?:crate::container::|pdfplumber::container::)?\bgetattr\(([^,]+), &name, &([A-Z_]+)\)",
                             text):
            if line_of(m.start()) in added:
                out.append((path, line_of(m.start()), "getattr " + m.group(2), m.start(), m.end(),
                            "Container::attribute(%s, &name)" % m.group(1)))
        # statements holding units
        for m in re.finditer(r"[ \t]*(?:let _\w+ = [\w:]*(?:eval_entry|method_call)\(\)\?;|drop\(\w+\.builtin_call\(\)\?\);"
                             r"|let _\w+ = \w+\.builtin_call\(\)\?;|\w+\.plain\(\w+\);)\n", text):
            if line_of(m.start()) in added:
                out.append((path, line_of(m.start()), "statement " + m.group(0).strip(), m.start(), m.end(), ""))
    return out


def run(cmd: list[str], timeout: int = 3000) -> tuple[int, str]:
    p = subprocess.run(cmd, cwd=WT, env=env(), capture_output=True, text=True, timeout=timeout)
    return p.returncode, (p.stdout + p.stderr)[-3000:]


def climit_files() -> list[str]:
    return sorted(str(p) for d in ("adv-interp", "adv-plumber") for p in (root() / ".cache/corpus" / d).glob("*climit*"))


def verify() -> str | None:
    for stage in STAGES:
        code, log = run([str(root() / ".venv/bin/python"), "-m", "oracle.verify", "--stage", stage, "--files",
                         *climit_files(), "--pdftrace", str(WT / "target/release/pdftrace"), "-j", "3",
                         "--timeout", "600"])
        if code != 0:
            return f"verify {stage}: " + log.strip().splitlines()[-1]
    return None


def boundary() -> str | None:
    code, log = run(["cargo", "test", "--release", "-p", "pdfplumber-wasm", "--test", "js_runtime_unit",
                     "page_operations"])
    if code != 0:
        lines = [x for x in log.splitlines() if "panicked" in x or "left" in x or "error" in x][:2]
        return "js_runtime_unit: " + " | ".join(lines)[:300]
    return None


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--base", required=True, help="the revision whose later sites are mutated")
    ap.add_argument("--out", type=Path, help="the results (tab-separated, appended)")
    ap.add_argument("--list", action="store_true", help="list the mutants and exit")
    ap.add_argument("--only", help="comma-separated indices")
    ap.add_argument("--no-verify", action="store_true", help="skip oracle.verify (boundary code only)")
    args = ap.parse_args()
    ms = mutants(args.base)
    if args.list:
        for i, m in enumerate(ms):
            print(i, m[0], m[1], m[2], repr(m[5])[:60])
        return
    assert args.out, "--out"
    only = {int(x) for x in args.only.split(",")} if args.only else None
    done = {int(line.split("\t")[0]) for line in args.out.read_text().splitlines() if line} if args.out.exists() else set()
    for i, (path, line, kind, a, b, new) in enumerate(ms):
        if (only is not None and i not in only) or i in done:
            continue
        full = WT / path
        original = full.read_text()
        full.write_text(original[:a] + new + original[b:])
        started = time.time()
        try:
            code, log = run(["cargo", "build", "--release", "-p", "pdftrace"])
            if code != 0:
                verdict, detail = "unviable", log.strip().splitlines()[-1][:200]
            else:
                detail = None
                if not path.startswith("crates/wasm") and not args.no_verify:
                    detail = verify()
                if detail is None and not path.startswith("crates/pdftrace"):
                    detail = boundary()
                verdict = "caught" if detail else "survived"
        finally:
            full.write_text(original)
        with args.out.open("a") as f:
            f.write(f"{i}\t{path}\t{line}\t{kind}\t{verdict}\t{detail or ''}\t{time.time() - started:.0f}s\n")
    run(["cargo", "build", "--release", "-p", "pdftrace"])


if __name__ == "__main__":
    main()
