#!/usr/bin/env python3
from __future__ import annotations

import hashlib
import importlib.util
import json
import pathlib
import sys
from datetime import datetime, timezone
from typing import Any, Dict, Iterable, List, Optional

ROOT = pathlib.Path(__file__).resolve().parents[1]
BASE_SCRIPT = ROOT / "scripts" / "generate-parity-goldens.py"
OUT_DIR = ROOT / "test" / "fixtures" / "parity-cycles"
GOLDEN_DIR = ROOT / "test" / "fixtures" / "goldens" / "parity-cycles"

spec = importlib.util.spec_from_file_location("parity_goldens", BASE_SCRIPT)
if spec is None or spec.loader is None:
    raise RuntimeError(f"Unable to load {BASE_SCRIPT}")
parity = importlib.util.module_from_spec(spec)
sys.modules["parity_goldens"] = parity
spec.loader.exec_module(parity)


class PDF:
    def __init__(self) -> None:
        self.objects: list[bytes] = []

    def add(self, body: str | bytes) -> int:
        if isinstance(body, str):
            body = body.encode("latin1")
        self.objects.append(body)
        return len(self.objects)

    def stream(self, dictionary: str, data: str | bytes) -> int:
        if isinstance(data, str):
            data = data.encode("latin1")
        body = f"<< {dictionary} /Length {len(data)} >>\nstream\n".encode("latin1") + data + b"\nendstream"
        return self.add(body)

    def write(self, path: pathlib.Path, root_obj: int, info_obj: Optional[int] = None) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        out = bytearray(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n")
        offsets = [0]
        for index, body in enumerate(self.objects, start=1):
            offsets.append(len(out))
            out.extend(f"{index} 0 obj\n".encode("latin1"))
            out.extend(body)
            out.extend(b"\nendobj\n")
        xref = len(out)
        out.extend(f"xref\n0 {len(self.objects) + 1}\n".encode("latin1"))
        out.extend(b"0000000000 65535 f\n")
        for offset in offsets[1:]:
            out.extend(f"{offset:010d} 00000 n\n".encode("latin1"))
        info = f" /Info {info_obj} 0 R" if info_obj else ""
        out.extend(
            f"trailer\n<< /Size {len(self.objects) + 1} /Root {root_obj} 0 R{info} >>\nstartxref\n{xref}\n%%EOF\n".encode(
                "latin1"
            )
        )
        path.write_bytes(bytes(out))


def pdf_string(value: str) -> str:
    escaped = value.replace("\\", "\\\\").replace("(", "\\(").replace(")", "\\)")
    return f"({escaped})"


def sha256_file(path: pathlib.Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def page_tree(pdf: PDF, page_obj: int, extra_catalog: str = "") -> int:
    pages = pdf.add(f"<< /Type /Pages /Kids [{page_obj} 0 R] /Count 1 >>")
    return pdf.add(f"<< /Type /Catalog /Pages {pages} 0 R {extra_catalog} >>")


def text_case(cycle: int, index: int) -> tuple[PDF, str, list[dict[str, Any]]]:
    pdf = PDF()
    width = 240 + (index % 5) * 12
    height = 190 + (index % 4) * 15
    font_size = 9 + (index % 7)
    x0 = 18 + (index % 6) * 3
    y0 = height - 28 - (index % 5) * 4
    lines = [
        f"Cycle {cycle} Case {index:03d}",
        f"Alpha {index * 7} Beta {cycle + index}",
        f"Mix-{index % 11} page text",
    ]
    content = ["BT", f"/F1 {font_size} Tf", f"{x0} {y0} Td"]
    for line_index, line in enumerate(lines):
        if line_index:
            content.append(f"0 -{font_size + 5} Td")
        content.append(f"{pdf_string(line)} Tj")
    content.append("ET")
    font = pdf.add("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
    content_obj = pdf.stream("", "\n".join(content))
    page = pdf.add(
        f"<< /Type /Page /MediaBox [0 0 {width} {height}] /Resources << /Font << /F1 {font} 0 R >> >> /Contents {content_obj} 0 R >>"
    )
    root = page_tree(pdf, page)
    pdf.root = root  # type: ignore[attr-defined]
    ops = [
        {"type": "text", "page": 0},
        {"type": "words", "page": 0},
        {"type": "search", "page": 0, "pattern": f"Case {index:03d}", "options": {"regex": False}},
    ]
    return pdf, "text-basic", ops


def positioned_text_case(cycle: int, index: int) -> tuple[PDF, str, list[dict[str, Any]]]:
    pdf = PDF()
    width = 260 + (index % 3) * 20
    height = 210 + (index % 5) * 10
    font_size = 10 + (index % 4)
    x = 22 + (index % 7) * 2
    y = height - 34
    gap = 18 + (index % 5)
    content = [
        "BT",
        f"/F1 {font_size} Tf",
        f"1 0 0 1 {x} {y} Tm",
        f"{pdf_string(f'Left {cycle}-{index}')} Tj",
        f"{gap} 0 Td",
        f"{pdf_string('Right')} Tj",
        f"{-gap} -{font_size + 8} Td",
        f"{pdf_string('Below words')} Tj",
        "ET",
    ]
    font = pdf.add("<< /Type /Font /Subtype /Type1 /BaseFont /Times-Roman >>")
    content_obj = pdf.stream("", "\n".join(content))
    page = pdf.add(
        f"<< /Type /Page /MediaBox [0 0 {width} {height}] /Resources << /Font << /F1 {font} 0 R >> >> /Contents {content_obj} 0 R >>"
    )
    root = page_tree(pdf, page)
    pdf.root = root  # type: ignore[attr-defined]
    ops = [
        {"type": "text", "page": 0, "options": {"x_tolerance": 2 + (index % 3)}},
        {"type": "words", "page": 0, "options": {"use_text_flow": bool(index % 2)}},
        {"type": "text", "page": 0, "bbox": [0, 0, width, height / 2]},
    ]
    return pdf, "text-positioned", ops


def vector_case(cycle: int, index: int) -> tuple[PDF, str, list[dict[str, Any]]]:
    pdf = PDF()
    width = 220 + (index % 4) * 15
    height = 180 + (index % 6) * 10
    x = 18 + (index % 8) * 4
    y = 18 + (index % 5) * 3
    rect_w = 38 + (index % 6) * 5
    rect_h = 22 + (index % 4) * 4
    content = "\n".join(
        [
            f"{0.5 + (index % 3) * 0.5} w",
            f"{(index % 10) / 10:.1f} G",
            f"{x} {y} m {x + 80} {y + 18} l S",
            f"{(index % 4) / 4:.2f} g",
            f"{x + 8} {y + 38} {rect_w} {rect_h} re f",
            f"{x} {y + 90} m {x + 30} {y + 124} {x + 70} {y + 114} {x + 98} {y + 86} c S",
            "BT /F1 10 Tf 20 160 Td (Vector case) Tj ET",
        ]
    )
    font = pdf.add("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
    content_obj = pdf.stream("", content)
    page = pdf.add(
        f"<< /Type /Page /MediaBox [0 0 {width} {height}] /Resources << /Font << /F1 {font} 0 R >> >> /Contents {content_obj} 0 R >>"
    )
    root = page_tree(pdf, page)
    pdf.root = root  # type: ignore[attr-defined]
    ops = [
        {"type": "text", "page": 0},
        {"type": "words", "page": 0},
        {"type": "findTables", "page": 0},
    ]
    return pdf, "vector-paths", ops


def image_case(cycle: int, index: int) -> tuple[PDF, str, list[dict[str, Any]]]:
    pdf = PDF()
    width = 180 + (index % 4) * 20
    height = 160 + (index % 5) * 10
    red = bytes([220 - (index % 7) * 8, 20 + (index % 5) * 20, 40 + (cycle * 20)])
    image = pdf.stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8", red)
    font = pdf.add("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
    scale = 10 + (index % 8) * 2
    content = "\n".join(
        [
            f"q {scale} 0 0 {scale} {30 + index % 20} {35 + index % 18} cm /Im1 Do Q",
            f"BT /F1 11 Tf 20 {height - 28} Td {pdf_string(f'Image {cycle}-{index}')} Tj ET",
        ]
    )
    content_obj = pdf.stream("", content)
    page = pdf.add(
        f"<< /Type /Page /MediaBox [0 0 {width} {height}] /Resources << /Font << /F1 {font} 0 R >> /XObject << /Im1 {image} 0 R >> >> /Contents {content_obj} 0 R >>"
    )
    root = page_tree(pdf, page)
    pdf.root = root  # type: ignore[attr-defined]
    ops = [
        {"type": "text", "page": 0},
        {"type": "words", "page": 0},
    ]
    return pdf, "image-xobject", ops


def annotation_case(cycle: int, index: int) -> tuple[PDF, str, list[dict[str, Any]]]:
    pdf = PDF()
    width = 240
    height = 180 + (index % 4) * 10
    font = pdf.add("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
    text = f"Link {cycle}-{index}"
    content_obj = pdf.stream("", f"BT /F1 12 Tf 30 {height - 45} Td {pdf_string(text)} Tj ET")
    link = pdf.add(
        f"<< /Type /Annot /Subtype /Link /Rect [28 {height - 52} 120 {height - 34}] /Border [0 0 0] /A << /S /URI /URI {pdf_string(f'https://example.test/c{cycle}/{index}')} >> >>"
    )
    page = pdf.add(
        f"<< /Type /Page /MediaBox [0 0 {width} {height}] /Resources << /Font << /F1 {font} 0 R >> >> /Contents {content_obj} 0 R /Annots [{link} 0 R] >>"
    )
    root = page_tree(pdf, page)
    pdf.root = root  # type: ignore[attr-defined]
    ops = [
        {"type": "text", "page": 0},
        {"type": "search", "page": 0, "pattern": "Link", "options": {"regex": False}},
    ]
    return pdf, "annotation-link", ops


BUILDERS = [text_case, positioned_text_case, vector_case, image_case, annotation_case]


def case_builder(cycle: int, ordinal: int):
    return BUILDERS[(cycle * 17 + ordinal) % len(BUILDERS)]


def make_check(path: pathlib.Path, operations: list[dict[str, Any]]) -> list[dict[str, Any]]:
    checks = [
        parity.make_check(
            "document.snapshot",
            parity.document_snapshot(path, page_indices=[0]),
            pageIndices=[0],
        )
    ]
    with parity.pdfplumber.open(path) as pdf:
        checks.extend(parity.operation_checks(pdf, operations))
    return checks


def scenario_for(path: pathlib.Path, case_id: str, operations: list[dict[str, Any]]) -> Dict[str, Any]:
    return {
        "id": case_id,
        "pdf": str(path.relative_to(ROOT)),
        "checks": make_check(path, operations),
    }


def write_golden(path: pathlib.Path, cycle: int, partition: str, scenarios: list[dict[str, Any]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    payload = {
        "reference": {
            "generatedAt": datetime.now(timezone.utc).isoformat(),
            "pdfplumberVersion": getattr(parity.pdfplumber, "__version__", "unknown"),
            "pdfminerVersion": getattr(parity.pdfminer, "__version__", "unknown") if parity.pdfminer else "unknown",
            "pdfplumberCommit": parity.git_head(),
            "source": "scripts/generate-parity-cycle-cases.py",
        },
        "coverage": {
            "cycle": cycle,
            "partition": partition,
            "scenarioCount": len(scenarios),
            "pdfCount": len(scenarios),
        },
        "scenarios": scenarios,
    }
    path.write_text(json.dumps(payload, indent=2, ensure_ascii=False, sort_keys=True) + "\n", encoding="utf-8")


def build_cycle(cycle: int) -> None:
    manifests: dict[str, list[dict[str, Any]]] = {"working": [], "holdout": []}
    scenarios: dict[str, list[dict[str, Any]]] = {"working": [], "holdout": []}

    for ordinal in range(1, 121):
        partition = "working" if ordinal <= 100 else "holdout"
        builder = case_builder(cycle, ordinal)
        pdf, category, operations = builder(cycle, ordinal)
        case_id = f"cycle-{cycle:02d}/{partition}/c{cycle:02d}-{ordinal:03d}-{category}"
        filename = f"c{cycle:02d}-{ordinal:03d}-{category}.pdf"
        path = OUT_DIR / f"cycle-{cycle:02d}" / partition / filename
        info = pdf.add(
            f"<< /Title {pdf_string(case_id)} /Creator {pdf_string('pdfplumber.js parity cycle generator')} /Producer {pdf_string('pdfplumber.js tests')} >>"
        )
        pdf.write(path, getattr(pdf, "root"), info)
        manifests[partition].append(
            {
                "id": case_id,
                "localPath": str(path.relative_to(ROOT)),
                "source": "Generated micro-PDF",
                "generator": "scripts/generate-parity-cycle-cases.py",
                "categories": [category, "micro-pdf", f"cycle-{cycle:02d}", partition],
                "size": path.stat().st_size,
                "sha256": sha256_file(path),
            }
        )
        scenarios[partition].append(scenario_for(path, case_id, operations))

    cycle_dir = OUT_DIR / f"cycle-{cycle:02d}"
    for partition in ["working", "holdout"]:
        manifest_path = cycle_dir / f"{partition}-manifest.json"
        manifest_path.write_text(json.dumps(manifests[partition], indent=2, sort_keys=True) + "\n", encoding="utf-8")
        write_golden(GOLDEN_DIR / f"pdfplumber-cycle-{cycle:02d}-{partition}.json", cycle, partition, scenarios[partition])
    print(f"Wrote cycle {cycle}: working={len(scenarios['working'])} holdout={len(scenarios['holdout'])}")


def parse_cycles(args: Iterable[str]) -> list[int]:
    cycles = [int(arg) for arg in args]
    return cycles or [1, 2, 3]


def main() -> None:
    for cycle in parse_cycles(sys.argv[1:]):
        if cycle < 1:
            raise ValueError("Cycle numbers are 1-based")
        build_cycle(cycle)


if __name__ == "__main__":
    main()
