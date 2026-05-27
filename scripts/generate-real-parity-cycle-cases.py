#!/usr/bin/env python3
from __future__ import annotations

import hashlib
import importlib.util
import json
import pathlib
import sys
from collections import defaultdict, deque
from datetime import datetime, timezone
from typing import Any, Dict, Iterable, List

ROOT = pathlib.Path(__file__).resolve().parents[1]
BASE_SCRIPT = ROOT / "scripts" / "generate-parity-goldens.py"
CYCLE_DIR = ROOT / "test" / "fixtures" / "parity-cycles"
GOLDEN_DIR = ROOT / "test" / "fixtures" / "goldens" / "parity-cycles"

SOURCE_MANIFESTS = [
    ROOT / "test" / "fixtures" / "external-pdfs" / "manifest.json",
    ROOT / "test" / "fixtures" / "external-holdout-pdfs" / "manifest.json",
]

SLOW_COUNTED_DOC_IDS = {
    # Keep the scanned book available for targeted image/performance diagnostics,
    # but do not use it as a counted parity-cycle case until selected-page image
    # extraction can run without overwhelming the fast validation loop.
    "commons-alice-scan",
}

spec = importlib.util.spec_from_file_location("parity_goldens", BASE_SCRIPT)
if spec is None or spec.loader is None:
    raise RuntimeError(f"Unable to load {BASE_SCRIPT}")
parity = importlib.util.module_from_spec(spec)
sys.modules["parity_goldens"] = parity
spec.loader.exec_module(parity)


def sha256_file(path: pathlib.Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def load_entries() -> List[Dict[str, Any]]:
    entries: List[Dict[str, Any]] = []
    for manifest_path in SOURCE_MANIFESTS:
        for entry in json.loads(manifest_path.read_text(encoding="utf-8")):
            if entry["id"] in SLOW_COUNTED_DOC_IDS:
                continue
            local_path = ROOT / entry["localPath"]
            if not local_path.exists():
                continue
            try:
                with parity.pdfplumber.open(local_path) as pdf:
                    page_count = len(pdf.pages)
                    metadata = dict(pdf.metadata or {})
            except Exception:
                continue
            if page_count <= 0:
                continue
            entries.append(
                {
                    **entry,
                    "pageCount": page_count,
                    "producer": metadata.get("Producer"),
                    "creator": metadata.get("Creator"),
                    "title": metadata.get("Title"),
                    "manifest": str(manifest_path.relative_to(ROOT)),
                }
            )
    return entries


def selected_pages(page_count: int) -> List[int]:
    if page_count <= 40:
        return list(range(page_count))
    anchors = {0, 1, 2, page_count - 3, page_count - 2, page_count - 1}
    interior_slots = 34
    step = (page_count - 1) / (interior_slots + 1)
    candidates = sorted(anchors | {round(step * i) for i in range(1, interior_slots + 1)})
    out: List[int] = []
    for page in candidates:
        if page not in out:
            out.append(page)
    return out


def behavior(entry: Dict[str, Any], page_index: int) -> str:
    categories = set(entry.get("categories", []))
    if categories & {"acroform", "annotations", "form", "form-widget", "forms"}:
        return "forms-annotations"
    if categories & {"financial-report", "annual-report", "sec-filing", "bank-regulatory"}:
        return "financial-report"
    if categories & {"scientific-paper", "technical-paper"}:
        return "scientific-technical"
    if categories & {"legal", "regulatory", "federal-register", "court"}:
        return "legal-regulatory"
    if categories & {"cjk", "rtl", "arabic", "cyrillic", "multilingual", "unicode", "cid-font"}:
        return "multilingual-text"
    if categories & {"pdfua", "tagged-pdf", "accessibility"}:
        return "tagged-accessibility"
    if categories & {"tables", "table-extraction"}:
        return "tables"
    if categories & {"image-heavy", "scan", "bitmap", "image"}:
        return "images-graphics"
    if "pdfjs-regression" in categories:
        return "upstream-regression"
    return "text-geometry"


def rationale(entry: Dict[str, Any], page_index: int) -> str:
    b = behavior(entry, page_index)
    page_part = "first page" if page_index == 0 else "last page" if page_index == entry["pageCount"] - 1 else "interior page"
    return f"{page_part} selected from a real {b.replace('-', ' ')} document to exercise pdfplumber page geometry, objects, text, words, colors, images, annotations, and table summaries when present."


def operations_for(entry: Dict[str, Any], page_index: int) -> List[Dict[str, Any]]:
    categories = set(entry.get("categories", []))
    ops: List[Dict[str, Any]] = [
        {"type": "text", "page": page_index},
        {"type": "words", "page": page_index},
    ]
    if categories & {"unicode", "font", "cid-font", "cjk", "rtl", "arabic", "chinese", "cyrillic", "multilingual"}:
        ops.append({"type": "text", "page": page_index, "options": {"layout": True}})
    if categories & {"tables", "table-extraction", "financial-report", "annual-report", "scientific-paper"}:
        ops.append({"type": "findTables", "page": page_index})
        ops.append({"type": "extractTables", "page": page_index})
    if categories & {"annotations", "acroform", "form", "form-widget", "forms"}:
        ops.append({"type": "words", "page": page_index, "options": {"keep_blank_chars": True}})
        ops.append({"type": "search", "page": page_index, "pattern": "Text|Name|Date|Form", "options": {"regex": True}})
    return ops


def candidates(entries: Iterable[Dict[str, Any]]) -> List[Dict[str, Any]]:
    out: List[Dict[str, Any]] = []
    for entry in entries:
        for page_index in selected_pages(entry["pageCount"]):
            out.append(
                {
                    "entry": entry,
                    "pageIndex": page_index,
                    "behavior": behavior(entry, page_index),
                    "source": entry.get("source", "unknown"),
                }
            )
    return out


def choose_cases(
    pool: deque[Dict[str, Any]],
    count: int,
    excluded_docs: set[str] | None = None,
    max_per_doc: int | None = None,
    max_per_source: int = 25,
    max_per_behavior: int | None = None,
) -> List[Dict[str, Any]]:
    selected: List[Dict[str, Any]] = []
    doc_counts: Dict[str, int] = defaultdict(int)
    source_counts: Dict[str, int] = defaultdict(int)
    behavior_counts: Dict[str, int] = defaultdict(int)
    excluded_docs = excluded_docs or set()
    target_behavior_count = min(8, len({item["behavior"] for item in pool}))

    attempts = 0
    while len(selected) < count and attempts < len(pool) * 8:
        attempts += 1
        candidate = pool.popleft()
        pool.append(candidate)
        entry = candidate["entry"]
        doc_id = entry["id"]
        source = candidate["source"]
        behavior_name = candidate["behavior"]
        doc_limit = min(case_limit_for(entry), max_per_doc) if max_per_doc is not None else case_limit_for(entry)
        if doc_id in excluded_docs:
            continue
        if doc_counts[doc_id] >= doc_limit:
            continue
        if source_counts[source] >= max_per_source:
            continue
        if max_per_behavior is not None and behavior_counts[behavior_name] >= max_per_behavior:
            continue
        if behavior_counts[behavior_name] >= 24 and len({item["behavior"] for item in selected}) < target_behavior_count:
            continue
        key = (doc_id, candidate["pageIndex"])
        if any((item["entry"]["id"], item["pageIndex"]) == key for item in selected):
            continue
        selected.append(candidate)
        doc_counts[doc_id] += 1
        source_counts[source] += 1
        behavior_counts[behavior_name] += 1

    if len(selected) != count:
        raise RuntimeError(f"Unable to choose {count} independent real-document cases; selected {len(selected)}")
    return selected


def case_limit_for(entry: Dict[str, Any]) -> int:
    categories = set(entry.get("categories", []))
    if categories & {"scan", "image-heavy", "bitmap"}:
        return 2
    return 10


def choose_holdout(pool: deque[Dict[str, Any]], excluded_docs: set[str]) -> List[Dict[str, Any]]:
    plans = [
        {"max_per_doc": 5, "max_per_source": 5, "max_per_behavior": 4},
        {"max_per_doc": 5, "max_per_source": 10, "max_per_behavior": 5},
        {"max_per_doc": 10, "max_per_source": 10, "max_per_behavior": 8},
        {"max_per_doc": 10, "max_per_source": 25, "max_per_behavior": None},
    ]
    errors: List[str] = []
    for plan in plans:
        try:
            return choose_cases(deque(pool), 20, excluded_docs=excluded_docs, **plan)
        except RuntimeError as exc:
            errors.append(str(exc))
    raise RuntimeError("; ".join(errors))


def scenario_for(cycle: int, phase: str, ordinal: int, case: Dict[str, Any]) -> Dict[str, Any]:
    entry = case["entry"]
    page_index = int(case["pageIndex"])
    case_id = f"cycle-{cycle:02d}/{phase}/c{cycle:02d}-{ordinal:03d}-{case['behavior']}-{entry['id']}-p{page_index + 1}"
    pdf_path = ROOT / entry["localPath"]
    with parity.pdfplumber.open(pdf_path) as pdf:
        checks = []
        snapshot = parity.document_snapshot(pdf_path, page_indices=[page_index])
        if snapshot.get("status") != "error":
            checks.append(
                parity.make_check(
                    "document.snapshot",
                    snapshot,
                    pageIndices=[page_index],
                )
            )
        checks.extend(parity.operation_checks(pdf, operations_for(entry, page_index)))
    return {"id": case_id, "pdf": entry["localPath"], "checks": checks}


def manifest_entry(cycle: int, phase: str, ordinal: int, case: Dict[str, Any]) -> Dict[str, Any]:
    entry = case["entry"]
    path = ROOT / entry["localPath"]
    page_index = int(case["pageIndex"])
    return {
        "id": f"cycle-{cycle:02d}/{phase}/c{cycle:02d}-{ordinal:03d}-{case['behavior']}-{entry['id']}-p{page_index + 1}",
        "sourceDocumentId": entry["id"],
        "localPath": entry["localPath"],
        "sourceUrl": entry.get("sourceUrl"),
        "source": entry.get("source"),
        "sourceManifest": entry.get("manifest"),
        "licenseOrTerms": entry.get("license") or entry.get("licenseOrTerms"),
        "sha256": entry.get("sha256") or sha256_file(path),
        "size": entry.get("size") or path.stat().st_size,
        "categories": sorted(set(entry.get("categories", []) + [case["behavior"], f"cycle-{cycle:02d}", phase, "real-document"])),
        "selectedPages": [page_index],
        "pageCount": entry["pageCount"],
        "producer": entry.get("producer"),
        "creator": entry.get("creator"),
        "title": entry.get("title"),
        "rationale": rationale(entry, page_index),
        "counted": True,
    }


def write_cycle(cycle: int, working: List[Dict[str, Any]], holdout: List[Dict[str, Any]]) -> None:
    for phase, cases in [("working", working), ("holdout", holdout)]:
        scenarios = []
        manifest = []
        for index, case in enumerate(cases, 1 if phase == "working" else 101):
            print(f"Generating cycle-{cycle:02d} {phase} case {index}: {case['entry']['id']} page {case['pageIndex'] + 1}", flush=True)
            scenarios.append(scenario_for(cycle, phase, index, case))
            manifest.append(manifest_entry(cycle, phase, index, case))

        cycle_path = CYCLE_DIR / f"cycle-{cycle:02d}"
        cycle_path.mkdir(parents=True, exist_ok=True)
        (cycle_path / f"{phase}-manifest.json").write_text(json.dumps(manifest, indent=2, ensure_ascii=False, sort_keys=True) + "\n", encoding="utf-8")
        payload = {
            "reference": {
                "generatedAt": datetime.now(timezone.utc).isoformat(),
                "pdfplumberVersion": getattr(parity.pdfplumber, "__version__", "unknown"),
                "pdfminerVersion": getattr(parity.pdfminer, "__version__", "unknown") if parity.pdfminer else "unknown",
                "pdfplumberCommit": parity.git_head(),
                "source": "scripts/generate-real-parity-cycle-cases.py",
            },
            "coverage": {
                "cycle": cycle,
                "phase": phase,
                "caseCount": len(scenarios),
                "realDocumentOnly": True,
                "sourceDocumentCount": len({case["entry"]["id"] for case in cases}),
                "behaviorFamilies": sorted({case["behavior"] for case in cases}),
            },
            "scenarios": scenarios,
        }
        GOLDEN_DIR.mkdir(parents=True, exist_ok=True)
        (GOLDEN_DIR / f"pdfplumber-cycle-{cycle:02d}-{phase}.json").write_text(
            json.dumps(payload, indent=2, ensure_ascii=False, sort_keys=True) + "\n",
            encoding="utf-8",
        )


def parse_cycle_list(value: str) -> List[int]:
    cycles = [int(item.strip()) for item in value.split(",") if item.strip()]
    if not cycles:
        raise ValueError("At least one cycle number is required")
    return cycles


def arg_value(name: str, default: str) -> str:
    if name not in sys.argv:
        return default
    index = sys.argv.index(name)
    try:
        return sys.argv[index + 1]
    except IndexError as exc:
        raise ValueError(f"Missing value after {name}") from exc


def existing_case_keys(cycles: Iterable[int]) -> set[tuple[str, int]]:
    used: set[tuple[str, int]] = set()
    for cycle in cycles:
        for phase in ["working", "holdout"]:
            manifest_path = CYCLE_DIR / f"cycle-{cycle:02d}" / f"{phase}-manifest.json"
            if not manifest_path.exists():
                continue
            for entry in json.loads(manifest_path.read_text(encoding="utf-8")):
                pages = entry.get("selectedPages") or []
                if entry.get("counted") is not True or not pages:
                    continue
                used.add((entry["sourceDocumentId"], int(pages[0])))
    return used


def main() -> None:
    cycles = parse_cycle_list(arg_value("--cycles", "10,11,12"))
    exclude_cycles = parse_cycle_list(arg_value("--exclude-cycles", "7,8,9"))
    all_candidates = candidates(load_entries())
    if len(all_candidates) < 360:
        raise RuntimeError(f"Need at least 360 real selected-page candidates, found {len(all_candidates)}")
    pool = deque(sorted(all_candidates, key=lambda item: (item["behavior"], item["source"], item["entry"]["id"], item["pageIndex"])))
    used_global = existing_case_keys(exclude_cycles)
    for cycle in cycles:
        working_pool = deque(item for item in pool if (item["entry"]["id"], item["pageIndex"]) not in used_global)
        working = choose_cases(working_pool, 100)
        used_global.update((case["entry"]["id"], case["pageIndex"]) for case in working)
        working_docs = {case["entry"]["id"] for case in working}
        holdout_pool = deque(item for item in pool if (item["entry"]["id"], item["pageIndex"]) not in used_global)
        holdout = choose_holdout(holdout_pool, excluded_docs=working_docs)
        used_global.update((case["entry"]["id"], case["pageIndex"]) for case in holdout)
        write_cycle(cycle, working, holdout)


if __name__ == "__main__":
    main()
