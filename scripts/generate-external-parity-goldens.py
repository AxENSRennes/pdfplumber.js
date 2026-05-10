#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
import json
import pathlib
import sys
from datetime import datetime, timezone
from typing import Any, Dict, List

ROOT = pathlib.Path(__file__).resolve().parents[1]
BASE_SCRIPT = ROOT / "scripts" / "generate-parity-goldens.py"
MANIFEST = ROOT / pathlib.Path(
    __import__("os").environ.get("EXTERNAL_PARITY_MANIFEST", "test/fixtures/external-pdfs/manifest.json")
)
OUT = ROOT / pathlib.Path(
    __import__("os").environ.get("EXTERNAL_PARITY_OUT", "test/fixtures/goldens/pdfplumber-external-parity.json")
)

spec = importlib.util.spec_from_file_location("parity_goldens", BASE_SCRIPT)
if spec is None or spec.loader is None:
    raise RuntimeError(f"Unable to load {BASE_SCRIPT}")
parity = importlib.util.module_from_spec(spec)
sys.modules["parity_goldens"] = parity
spec.loader.exec_module(parity)


def selected_page_indices(page_count: int) -> List[int]:
    if page_count <= 10:
        return list(range(page_count))
    middle = page_count // 2
    return sorted(
        {
            0,
            1,
            middle - 1,
            middle,
            middle + 1,
            page_count - 2,
            page_count - 1,
        }
    )


def page_count(path: pathlib.Path) -> int:
    try:
        with parity.pdfplumber.open(path) as pdf:
            return len(pdf.pages)
    except Exception:
        return 0


def make_snapshot_check(path: pathlib.Path) -> Dict[str, Any]:
    count = page_count(path)
    indices = selected_page_indices(count) if count else []
    return parity.make_check(
        "document.snapshot",
        parity.document_snapshot(path, page_indices=indices),
        pageIndices=indices,
    )


def operation_scenario(entry: Dict[str, Any], operations: List[Dict[str, Any]]) -> Dict[str, Any]:
    path = ROOT / entry["localPath"]
    checks = [make_snapshot_check(path)]
    if page_count(path):
        with parity.pdfplumber.open(path) as pdf:
            checks.extend(parity.operation_checks(pdf, operations))
    return {
        "id": f"external/{entry['id']}",
        "pdf": entry["localPath"],
        "checks": checks,
    }


def default_operations(entry: Dict[str, Any], count: int) -> List[Dict[str, Any]]:
    categories = set(entry["categories"])
    ops: List[Dict[str, Any]] = [
        {"type": "text", "page": 0},
        {"type": "words", "page": 0},
    ]

    if (
        "unicode" in categories
        or "font" in categories
        or "cid-font" in categories
        or "type3-font" in categories
        or "cjk" in categories
        or "rtl" in categories
        or "arabic" in categories
        or "chinese" in categories
        or "cyrillic" in categories
        or "multilingual" in categories
    ):
        ops.extend(
            [
                {"type": "words", "page": 0, "options": {"extra_attrs": ["fontname", "size"]}},
                {"type": "text", "page": 0, "options": {"layout": True}},
            ]
        )

    if "annotations" in categories or "acroform" in categories or "form-widget" in categories or "forms" in categories:
        ops.extend(
            [
                {"type": "search", "page": 0, "pattern": "Text", "options": {"regex": False}},
                {"type": "words", "page": 0, "options": {"keep_blank_chars": True}},
            ]
        )

    if "tables" in categories or "table-extraction" in categories:
        ops.extend(
            [
                {"type": "findTables", "page": 0},
                {"type": "extractTables", "page": 0},
                {"type": "extractTables", "page": 0, "options": {"vertical_strategy": "text", "horizontal_strategy": "text"}},
            ]
        )

    if "scientific-paper" in categories:
        middle = min(max(count // 2, 0), count - 1)
        ops.extend(
            [
                {"type": "text", "page": middle, "options": {"layout": True}},
                {"type": "words", "page": middle, "options": {"use_text_flow": True}},
                {"type": "search", "page": middle, "pattern": "Table", "options": {"regex": False}},
                {"type": "findTables", "page": middle},
            ]
        )

    if "legal" in categories:
        middle = min(max(count // 2, 0), count - 1)
        last = max(count - 1, 0)
        ops.extend(
            [
                {"type": "text", "page": middle},
                {"type": "text", "page": last, "options": {"layout": True}},
                {"type": "words", "page": middle, "options": {"use_text_flow": True}},
                {"type": "search", "page": 0, "pattern": "Federal Register|JUSTICE|Court", "options": {"regex": True}},
            ]
        )

    if "long-document" in categories:
        middle = min(max(count // 2, 0), count - 1)
        ops.extend(
            [
                {"type": "text", "page": middle},
                {"type": "words", "page": middle},
                {"type": "findTables", "page": middle},
                {"type": "extractTables", "page": middle},
            ]
        )

    if (
        "financial-report" in categories
        or "annual-report" in categories
        or "sec-filing" in categories
        or "bank-regulatory" in categories
    ):
        middle = min(max(count // 2, 0), count - 1)
        last = max(count - 1, 0)
        ops.extend(
            [
                {"type": "search", "page": middle, "pattern": "Assets|Revenue|Net income|Balance Sheet|Consolidated", "options": {"regex": True}},
                {"type": "words", "page": middle, "options": {"use_text_flow": True}},
                {"type": "text", "page": last, "options": {"layout": True}},
                {"type": "findTables", "page": last},
            ]
        )

    if "scan" in categories or "image-heavy" in categories or "bitmap" in categories or "image" in categories:
        middle = min(max(count // 2, 0), count - 1)
        ops.extend(
            [
                {"type": "text", "page": middle},
                {"type": "words", "page": middle},
            ]
        )

    return ops


def build_scenarios() -> List[Dict[str, Any]]:
    manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
    scenarios = []
    for entry in manifest:
        print(f"Generating external parity scenario: {entry['id']}", flush=True)
        path = ROOT / entry["localPath"]
        count = page_count(path)
        operations = default_operations(entry, count) if count else []
        scenarios.append(operation_scenario(entry, operations))
    return scenarios


def main() -> None:
    scenarios = build_scenarios()
    OUT.parent.mkdir(parents=True, exist_ok=True)
    payload = {
        "reference": {
            "generatedAt": datetime.now(timezone.utc).isoformat(),
            "pdfplumberVersion": getattr(parity.pdfplumber, "__version__", "unknown"),
            "pdfminerVersion": getattr(parity.pdfminer, "__version__", "unknown") if parity.pdfminer else "unknown",
            "pdfplumberCommit": parity.git_head(),
            "source": str(MANIFEST.relative_to(ROOT)),
        },
        "coverage": {
            "pdfCount": len(json.loads(MANIFEST.read_text(encoding="utf-8"))),
            "scenarioCount": len(scenarios),
            "manifest": str(MANIFEST.relative_to(ROOT)),
        },
        "scenarios": scenarios,
    }
    OUT.write_text(json.dumps(payload, indent=2, ensure_ascii=False, sort_keys=True) + "\n", encoding="utf-8")
    print(f"Wrote {OUT.relative_to(ROOT)} with {len(scenarios)} scenarios")


if __name__ == "__main__":
    main()
