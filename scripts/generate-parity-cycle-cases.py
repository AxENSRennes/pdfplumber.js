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


def to_unicode_cmap(mapping: dict[int, str]) -> str:
    rows = []
    for code, text in mapping.items():
        src = f"<{code:04X}>"
        dst = "<" + "".join(f"{ord(char):04X}" for char in text) + ">"
        rows.append(f"{src} {dst}")
    return "\n".join(
        [
            "/CIDInit /ProcSet findresource begin",
            "12 dict begin",
            "begincmap",
            "/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def",
            "/CMapName /CycleToUnicode def",
            "/CMapType 2 def",
            "1 begincodespacerange",
            "<0000> <FFFF>",
            "endcodespacerange",
            f"{len(rows)} beginbfchar",
            *rows,
            "endbfchar",
            "endcmap",
            "CMapName currentdict /CMap defineresource pop",
            "end",
            "end",
        ]
    )


def type0_font(pdf: PDF, mapping: dict[int, str], wmode: int = 0, name: str = "CycleType0") -> int:
    cmap = pdf.stream("", to_unicode_cmap(mapping))
    descendant = pdf.add(
        f"<< /Type /Font /Subtype /CIDFontType2 /BaseFont /{name} "
        f"/CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> "
        f"/FontDescriptor << /Type /FontDescriptor /FontName /{name} /Flags 32 /Ascent 718 /Descent -207 /CapHeight 718 /ItalicAngle 0 /StemV 80 >> "
        f"/DW 600 >>"
    )
    encoding = "/Identity-V" if wmode else "/Identity-H"
    return pdf.add(f"<< /Type /Font /Subtype /Type0 /BaseFont /{name} /Encoding {encoding} /DescendantFonts [{descendant} 0 R] /ToUnicode {cmap} 0 R >>")


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


def rich_text_options_case(cycle: int, index: int) -> tuple[PDF, str, list[dict[str, Any]]]:
    pdf = PDF()
    width = 320
    height = 230
    font_size = 10 + (index % 4)
    content = "\n".join(
        [
            "BT",
            f"/F1 {font_size} Tf",
            "1 0 0 1 24 190 Tm",
            f"{pdf_string(f'Alpha-{cycle}.{index}, beta/gamma')} Tj",
            f"0 -{font_size + 6} Td {pdf_string('word:one;word two')} Tj",
            f"0 -{font_size + 6} Td {pdf_string('loose')} Tj 26 0 Td {pdf_string('spacing')} Tj",
            f"0 -{font_size + 6} Td {pdf_string('dup')} Tj",
            f"0 0 Td {pdf_string('dup')} Tj",
            "ET",
        ]
    )
    font = pdf.add("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
    content_obj = pdf.stream("", content)
    page = pdf.add(
        f"<< /Type /Page /MediaBox [0 0 {width} {height}] /Resources << /Font << /F1 {font} 0 R >> >> /Contents {content_obj} 0 R >>"
    )
    pdf.root = page_tree(pdf, page)  # type: ignore[attr-defined]
    ops = [
        {"type": "text", "page": 0},
        {"type": "text", "page": 0, "options": {"layout": True, "x_tolerance": 2}},
        {"type": "words", "page": 0, "options": {"split_at_punctuation": True}},
        {"type": "text", "page": 0, "dedupe": True},
        {"type": "search", "page": 0, "pattern": r"Alpha-\d+\.\d+", "options": {"regex": True}},
    ]
    return pdf, "text-options", ops


def table_grid_case(cycle: int, index: int) -> tuple[PDF, str, list[dict[str, Any]]]:
    pdf = PDF()
    width = 280
    height = 220
    x0 = 24
    y0 = 54
    cols = [0, 58, 132, 220]
    rows = [0, 28, 56, 84, 112]
    path_ops = ["0.75 w"]
    for x in cols:
        path_ops.append(f"{x0 + x} {y0} m {x0 + x} {y0 + rows[-1]} l S")
    for y in rows:
        path_ops.append(f"{x0} {y0 + y} m {x0 + cols[-1]} {y0 + y} l S")
    text_ops = ["BT", "/F1 9 Tf"]
    labels = [
        ["Item", "Qty", "Amount"],
        [f"A{cycle}", str(index % 9 + 1), f"{index * 3}.00"],
        [f"B{index}", str(index % 5 + 2), f"{index * 7}.50"],
        ["Total", "", f"{index * 10}.50"],
    ]
    for row_index, row in enumerate(labels):
        for col_index, label in enumerate(row):
            text_ops.append(f"1 0 0 1 {x0 + cols[col_index] + 4} {y0 + rows[-1] - 18 - row_index * 28} Tm {pdf_string(label)} Tj")
    text_ops.append("ET")
    font = pdf.add("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
    content_obj = pdf.stream("", "\n".join(path_ops + text_ops))
    page = pdf.add(
        f"<< /Type /Page /MediaBox [0 0 {width} {height}] /Resources << /Font << /F1 {font} 0 R >> >> /Contents {content_obj} 0 R >>"
    )
    pdf.root = page_tree(pdf, page)  # type: ignore[attr-defined]
    ops = [
        {"type": "findTables", "page": 0},
        {"type": "extractTable", "page": 0},
        {"type": "extractTables", "page": 0, "options": {"vertical_strategy": "lines", "horizontal_strategy": "lines"}},
    ]
    return pdf, "table-grid", ops


def text_table_case(cycle: int, index: int) -> tuple[PDF, str, list[dict[str, Any]]]:
    pdf = PDF()
    width = 330
    height = 220
    font = pdf.add("<< /Type /Font /Subtype /Type1 /BaseFont /Courier >>")
    rows = [
        ("Name", "Region", "Value"),
        (f"Ann{index}", "North", f"{cycle * index}"),
        (f"Bo{index}", "South", f"{cycle + index}"),
        ("Total", "", f"{cycle * index + cycle + index}"),
    ]
    content = ["BT", "/F1 10 Tf"]
    for row_index, row in enumerate(rows):
        y = 176 - row_index * 18
        for x, text in zip([28, 128, 230], row):
            content.append(f"1 0 0 1 {x} {y} Tm {pdf_string(text)} Tj")
    content.append("ET")
    content_obj = pdf.stream("", "\n".join(content))
    page = pdf.add(
        f"<< /Type /Page /MediaBox [0 0 {width} {height}] /Resources << /Font << /F1 {font} 0 R >> >> /Contents {content_obj} 0 R >>"
    )
    pdf.root = page_tree(pdf, page)  # type: ignore[attr-defined]
    ops = [
        {"type": "extractTable", "page": 0, "options": {"vertical_strategy": "text", "horizontal_strategy": "text"}},
        {"type": "extractTables", "page": 0, "options": {"vertical_strategy": "text", "horizontal_strategy": "text", "min_words_vertical": 2}},
        {"type": "words", "page": 0},
    ]
    return pdf, "table-text", ops


def page_boxes_nonzero_case(cycle: int, index: int) -> tuple[PDF, str, list[dict[str, Any]]]:
    pdf = PDF()
    font = pdf.add("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
    content_obj = pdf.stream("", f"0 0 60 24 re S\nBT /F1 11 Tf 22 62 Td {pdf_string(f'Box {cycle}-{index}')} Tj ET")
    page = pdf.add(
        f"<< /Type /Page /MediaBox [-12 -8 240 170] /CropBox [8 12 220 150] /BleedBox [0 0 230 160] "
        f"/TrimBox [14 18 214 144] /ArtBox [20 24 208 138] "
        f"/Resources << /Font << /F1 {font} 0 R >> >> /Contents {content_obj} 0 R >>"
    )
    pdf.root = page_tree(pdf, page)  # type: ignore[attr-defined]
    ops = [
        {"type": "text", "page": 0},
        {"type": "words", "page": 0},
    ]
    return pdf, "page-boxes", ops


def path_color_case(cycle: int, index: int) -> tuple[PDF, str, list[dict[str, Any]]]:
    pdf = PDF()
    font = pdf.add("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
    content = "\n".join(
        [
            f"[{2 + index % 4} 2] {index % 3} d",
            f"{0.5 + (index % 4) * 0.25} w",
            "0.1 0.2 0.7 RG 20 30 m 130 42 l S",
            "0.2 g 28 58 42 24 re f",
            "0.7 0.1 0.2 rg 86 58 46 24 re B",
            "0 1 0 0 k 145 58 46 24 re f*",
            "0.3 0.6 0.1 rg 35 110 50 28 re f",
            "20 150 m 40 184 92 176 120 146 c s",
            "0 g",
            f"BT /F1 10 Tf 24 202 Td {pdf_string('Color paths')} Tj ET",
        ]
    )
    content_obj = pdf.stream("", content)
    page = pdf.add(
        f"<< /Type /Page /MediaBox [0 0 230 230] /Resources << /Font << /F1 {font} 0 R >> >> /Contents {content_obj} 0 R >>"
    )
    pdf.root = page_tree(pdf, page)  # type: ignore[attr-defined]
    return pdf, "paths-colors", [{"type": "text", "page": 0}, {"type": "findTables", "page": 0}]


def image_mask_case(cycle: int, index: int) -> tuple[PDF, str, list[dict[str, Any]]]:
    pdf = PDF()
    mask = pdf.stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ImageMask true /BitsPerComponent 1", b"\x80")
    image = pdf.stream(f"/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8 /SMask {mask} 0 R", bytes([index % 255, 180, 40]))
    font = pdf.add("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
    content_obj = pdf.stream("", f"q 14 0 0 14 25 45 cm /Im1 Do Q\nBT /F1 10 Tf 20 110 Td {pdf_string('Images and mask')} Tj ET")
    page = pdf.add(
        f"<< /Type /Page /MediaBox [0 0 180 140] /Resources << /Font << /F1 {font} 0 R >> /XObject << /Im1 {image} 0 R >> >> /Contents {content_obj} 0 R >>"
    )
    pdf.root = page_tree(pdf, page)  # type: ignore[attr-defined]
    return pdf, "images-masks", [{"type": "text", "page": 0}, {"type": "words", "page": 0}]


def form_content_case(cycle: int, index: int) -> tuple[PDF, str, list[dict[str, Any]]]:
    pdf = PDF()
    font = pdf.add("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
    inner = pdf.stream("/Type /XObject /Subtype /Form /BBox [0 0 82 42] /Matrix [1 0 0 1 0 0] /Resources << >>", "0 0 42 20 re f")
    c1 = pdf.stream("", "q /Fm1 Do Q")
    c2 = pdf.stream("", f"BT /F1 11 Tf 24 115 Td {pdf_string(f'Array {cycle}-{index}')} Tj ET")
    array = pdf.add(f"[{c1} 0 R {c2} 0 R]")
    page = pdf.add(
        f"<< /Type /Page /MediaBox [0 0 190 150] /Resources << /Font << /F1 {font} 0 R >> /XObject << /Fm1 {inner} 0 R >> >> /Contents {array} 0 R >>"
    )
    pdf.root = page_tree(pdf, page)  # type: ignore[attr-defined]
    return pdf, "forms-content", [{"type": "text", "page": 0}, {"type": "words", "page": 0}, {"type": "search", "page": 0, "pattern": "Array", "options": {"regex": False}}]


def annotation_widget_case(cycle: int, index: int) -> tuple[PDF, str, list[dict[str, Any]]]:
    pdf = PDF()
    font = pdf.add("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
    content_obj = pdf.stream("", f"BT /F1 12 Tf 24 180 Td {pdf_string(f'Widget link {cycle}-{index}')} Tj ET")
    link = pdf.add(f"<< /Type /Annot /Subtype /Link /Rect [22 172 132 192] /Border [0 0 0] /A << /S /URI /URI {pdf_string(f'https://example.test/cycle/{cycle}/{index}')} >> >>")
    text = pdf.add(f"<< /Type /Annot /Subtype /Text /Rect [24 132 44 152] /Contents {pdf_string('Note contents')} /T {pdf_string('Reviewer')} >>")
    widget = pdf.add("<< /Type /Annot /Subtype /Widget /FT /Tx /T (field) /V (value) /Rect [24 92 128 114] >>")
    page = pdf.add(
        f"<< /Type /Page /MediaBox [0 0 220 220] /Resources << /Font << /F1 {font} 0 R >> >> /Contents {content_obj} 0 R /Annots [{link} 0 R {text} 0 R {widget} 0 R] >>"
    )
    pdf.root = page_tree(pdf, page, f"/AcroForm << /Fields [{widget} 0 R] >>")  # type: ignore[attr-defined]
    return pdf, "annots-widgets", [{"type": "text", "page": 0}, {"type": "search", "page": 0, "pattern": "Widget", "options": {"regex": False}}]


def cmap_type3_case(cycle: int, index: int) -> tuple[PDF, str, list[dict[str, Any]]]:
    pdf = PDF()
    mapping = {1: "A", 2: "\ufb01", 3: "\u0416", 4: "?"}
    cid = type0_font(pdf, mapping, name=f"CycleCID{cycle}{index}")
    charproc = pdf.stream("", "500 0 0 0 500 500 d1\n50 50 400 400 re f")
    type3 = pdf.add(
        f"<< /Type /Font /Subtype /Type3 /Name /T3 /FontBBox [0 0 500 500] /FontMatrix [0.001 0 0 0.001 0 0] "
        f"/CharProcs << /A {charproc} 0 R >> /Encoding << /Type /Encoding /Differences [65 /A] >> /FirstChar 65 /LastChar 65 /Widths [500] /Resources << >> >>"
    )
    content = "BT /F1 16 Tf 24 120 Td <0001000200030004> Tj ET\nBT /F2 22 Tf 24 78 Td (A) Tj ET"
    content_obj = pdf.stream("", content)
    page = pdf.add(
        f"<< /Type /Page /MediaBox [0 0 210 160] /Resources << /Font << /F1 {cid} 0 R /F2 {type3} 0 R >> >> /Contents {content_obj} 0 R >>"
    )
    pdf.root = page_tree(pdf, page)  # type: ignore[attr-defined]
    return pdf, "cmap-type3", [{"type": "text", "page": 0}, {"type": "words", "page": 0}, {"type": "search", "page": 0, "pattern": "A", "options": {"regex": False}}]


def vertical_rtl_case(cycle: int, index: int) -> tuple[PDF, str, list[dict[str, Any]]]:
    pdf = PDF()
    vertical = type0_font(pdf, {1: "\u7e26", 2: "\u66f8"}, wmode=1, name=f"CycleVertical{cycle}{index}")
    rtl = type0_font(pdf, {1: "\u0627", 2: "\u0628", 3: "\u062c"}, name=f"CycleRTL{cycle}{index}")
    content = "\n".join(
        [
            "BT /Fv 18 Tf 48 142 Td <00010002> Tj ET",
            "BT /Fr 18 Tf 142 86 Td <000300020001> Tj ET",
        ]
    )
    content_obj = pdf.stream("", content)
    page = pdf.add(
        f"<< /Type /Page /MediaBox [0 0 210 180] /Resources << /Font << /Fv {vertical} 0 R /Fr {rtl} 0 R >> >> /Contents {content_obj} 0 R >>"
    )
    pdf.root = page_tree(pdf, page)  # type: ignore[attr-defined]
    return pdf, "vertical-rtl", [{"type": "text", "page": 0}, {"type": "words", "page": 0, "options": {"vertical_ttb": False}}, {"type": "words", "page": 0, "options": {"horizontal_ltr": False}}]


def marked_content_case(cycle: int, index: int) -> tuple[PDF, str, list[dict[str, Any]]]:
    pdf = PDF()
    font = pdf.add("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
    content = "\n".join(
        [
            "/P <</MCID 0>> BDC",
            f"BT /F1 12 Tf 26 120 Td {pdf_string(f'Marked {cycle}-{index}')} Tj ET",
            "EMC",
            "/Span <</MCID 1>> BDC",
            f"BT /F1 10 Tf 26 96 Td {pdf_string('Tagged span')} Tj ET",
            "EMC",
        ]
    )
    content_obj = pdf.stream("", content)
    page = pdf.add(
        f"<< /Type /Page /MediaBox [0 0 210 160] /Resources << /Font << /F1 {font} 0 R >> >> /Contents {content_obj} 0 R /StructParents 0 >>"
    )
    pdf.root = page_tree(pdf, page, "/MarkInfo << /Marked true >>")  # type: ignore[attr-defined]
    return pdf, "marked-content", [{"type": "text", "page": 0}, {"type": "words", "page": 0}]


HIGH_SIGNAL_BUILDERS = [
    rich_text_options_case,
    table_grid_case,
    text_table_case,
    page_boxes_nonzero_case,
    path_color_case,
    image_mask_case,
    form_content_case,
    annotation_widget_case,
    cmap_type3_case,
    marked_content_case,
    positioned_text_case,
]


REAL_CASES: list[dict[str, Any]] = [
    {"pdf": "nics-background-checks-2015-11.pdf", "page": 0, "categories": ["real-pdf", "government-form", "tables"], "operations": [{"type": "findTables", "page": 0}, {"type": "extractTable", "page": 0}]},
    {"pdf": "senate-expenditures.pdf", "page": 0, "categories": ["real-pdf", "financial-table", "text-table"], "operations": [{"type": "extractTable", "page": 0, "bbox": [70.332, 130.986, 420, 509.106], "options": {"horizontal_strategy": "text", "vertical_strategy": "text", "min_words_vertical": 20}}]},
    {"pdf": "scotus-transcript-p1.pdf", "page": 0, "categories": ["real-pdf", "legal", "search"], "operations": [{"type": "search", "page": 0, "pattern": "Roberts", "options": {"regex": False}}, {"type": "search", "page": 0, "pattern": r"\b[A-Z][a-z]+,\sJ\.", "options": {"regex": True}}]},
    {"pdf": "mcid_example.pdf", "page": 0, "categories": ["real-pdf", "tagged-pdf", "mcid"], "operations": [{"type": "text", "page": 0}, {"type": "words", "page": 0}]},
    {"pdf": "page-boxes-example.pdf", "page": 0, "categories": ["real-pdf", "page-boxes", "geometry"], "operations": [{"type": "text", "page": 0}, {"type": "words", "page": 0}]},
    {"pdf": "issue-598-example.pdf", "page": 0, "categories": ["real-pdf", "ligatures", "fonts"], "operations": [{"type": "text", "page": 0}, {"type": "text", "page": 0, "options": {"expand_ligatures": False}}, {"type": "words", "page": 0, "options": {"expand_ligatures": False}}]},
    {"pdf": "issue-192-example.pdf", "page": 0, "categories": ["real-pdf", "vertical-text", "laparams"], "operations": [{"type": "words", "page": 0, "options": {"vertical_ttb": False}}, {"type": "words", "page": 0, "options": {"horizontal_ltr": False}}]},
    {"pdf": "annotations-rotated-90.pdf", "page": 0, "categories": ["real-pdf", "annotations", "rotation"], "operations": [{"type": "text", "page": 0}, {"type": "search", "page": 0, "pattern": "Link", "options": {"regex": False}}]},
    {"pdf": "image_structure.pdf", "page": 0, "categories": ["real-pdf", "images", "tagged-pdf"], "operations": [{"type": "text", "page": 0}, {"type": "words", "page": 0}]},
    {"pdf": "pdf_structure.pdf", "page": 0, "categories": ["real-pdf", "tagged-pdf", "structure"], "operations": [{"type": "text", "page": 0}, {"type": "words", "page": 0}]},
    {"pdf": "word365_structure.pdf", "page": 0, "categories": ["real-pdf", "tagged-pdf", "office"], "operations": [{"type": "text", "page": 0}, {"type": "words", "page": 0}]},
    {"pdf": "chelsea_pdta.pdf", "page": 32, "categories": ["real-pdf", "long-document", "selected-middle-page"], "operations": [{"type": "text", "page": 32}, {"type": "words", "page": 32}]},
    {"pdf": "WARN-Report-for-7-1-2015-to-03-25-2016.pdf", "page": 15, "categories": ["real-pdf", "long-document", "selected-last-page"], "operations": [{"type": "text", "page": 15}, {"type": "words", "page": 15}]},
    {"pdf": "issue-71-duplicate-chars.pdf", "page": 0, "categories": ["real-pdf", "dedupe", "tables"], "operations": [{"type": "text", "page": 0}, {"type": "text", "page": 0, "dedupe": True}, {"type": "extractTable", "page": 0, "dedupe": True}]},
    {"pdf": "table-curves-example.pdf", "page": 0, "categories": ["real-pdf", "curves", "tables"], "operations": [{"type": "findTables", "page": 0}, {"type": "extractTables", "page": 0}]},
    {"pdf": "issue-987-test.pdf", "page": 0, "categories": ["real-pdf", "word-tolerance", "layout"], "operations": [{"type": "text", "page": 0, "options": {"x_tolerance_ratio": 0.15}}, {"type": "words", "page": 0, "options": {"x_tolerance_ratio": 0.15}}]},
    {"pdf": "password-example.pdf", "page": 0, "categories": ["real-pdf", "encrypted", "password"], "openOptions": {"password": "test"}, "operations": [{"type": "text", "page": 0}, {"type": "words", "page": 0}]},
    {"pdf": "malformed-from-issue-932.pdf", "page": 0, "categories": ["real-pdf", "malformed", "recovery"], "operations": [{"type": "text", "page": 0}, {"type": "words", "page": 0}]},
]


def case_builder(cycle: int, ordinal: int):
    if cycle >= 4:
        return HIGH_SIGNAL_BUILDERS[(cycle * 31 + ordinal) % len(HIGH_SIGNAL_BUILDERS)]
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


def scenario_for_real(path: pathlib.Path, case_id: str, page_index: int, operations: list[dict[str, Any]], open_options: Optional[dict[str, Any]] = None) -> Dict[str, Any]:
    checks = [
        parity.make_check(
            "document.snapshot",
            parity.document_snapshot(path, open_options=open_options, page_indices=[page_index]),
            pageIndices=[page_index],
        )
    ]
    with parity.pdfplumber.open(path, **(open_options or {})) as pdf:
        checks.extend(parity.operation_checks(pdf, operations))
    data: Dict[str, Any] = {
        "id": case_id,
        "pdf": str(path.relative_to(ROOT)),
        "checks": checks,
    }
    if open_options:
        data["openOptions"] = open_options
    return data


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
        if cycle >= 4 and ordinal % 5 == 0:
            real = REAL_CASES[(cycle * 13 + ordinal) % len(REAL_CASES)]
            category = "-".join(str(real["categories"][1]).split("-")[:2])
            case_id = f"cycle-{cycle:02d}/{partition}/c{cycle:02d}-{ordinal:03d}-real-{category}"
            path = ROOT / "pdfplumber-python" / "tests" / "pdfs" / str(real["pdf"])
            page_index = int(real["page"])
            open_options = real.get("openOptions")
            manifests[partition].append(
                {
                    "id": case_id,
                    "localPath": str(path.relative_to(ROOT)),
                    "source": "Vendored upstream pdfplumber regression PDF",
                    "sourceUrl": "https://github.com/jsvine/pdfplumber/tree/stable/tests/pdfs",
                    "licenseOrTerms": "Vendored with pdfplumber test corpus; retained for parity testing under repository test-fixture terms.",
                    "categories": [*real["categories"], f"cycle-{cycle:02d}", partition, "selected-page"],
                    "selectedPages": [page_index],
                    "size": path.stat().st_size,
                    "sha256": sha256_file(path),
                }
            )
            scenarios[partition].append(scenario_for_real(path, case_id, page_index, real["operations"], open_options))
        else:
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
                    "licenseOrTerms": "Generated locally for parity testing; no external license.",
                    "categories": [category, "micro-pdf", f"cycle-{cycle:02d}", partition],
                    "selectedPages": [0],
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
