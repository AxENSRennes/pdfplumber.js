#!/usr/bin/env python3
from __future__ import annotations

import json
import math
import os
import pathlib
import subprocess
import sys
from datetime import datetime, timezone
from typing import Any, Callable, Dict, Iterable, List, Optional

ROOT = pathlib.Path(__file__).resolve().parents[1]
PY_REF = ROOT / "pdfplumber-python"
PDF_DIR = PY_REF / "tests" / "pdfs"
OUT = ROOT / "test" / "fixtures" / "goldens" / "pdfplumber-compat.json"

sys.path.insert(0, str(PY_REF))

import pdfplumber  # noqa: E402
from pdfplumber import table  # noqa: E402
from pdfplumber import utils  # noqa: E402
from pdfplumber.ctm import CTM  # noqa: E402

try:
    import pdfminer  # noqa: E402
except Exception:  # pragma: nocover
    pdfminer = None


JSONScalar = Optional[str | int | float | bool]


def finite_number(value: Any) -> JSONScalar:
    if isinstance(value, bool):
        return value
    if isinstance(value, int):
        return value
    if isinstance(value, float):
        if math.isfinite(value):
            return round(value, 6)
        return None
    return None


def clean(value: Any) -> Any:
    number = finite_number(value)
    if number is not None or value is None:
        return number
    if isinstance(value, bytes):
        try:
            return value.decode("utf-8")
        except UnicodeDecodeError:
            return value.hex()
    if isinstance(value, pathlib.Path):
        return str(value)
    if isinstance(value, (list, tuple)):
        return [clean(v) for v in value]
    if isinstance(value, dict):
        cleaned: Dict[str, Any] = {}
        for key, item in value.items():
            if key in {"stream", "data", "graphicstate"}:
                continue
            if isinstance(item, (bytes, str, int, float, bool, list, tuple, dict)) or item is None:
                cleaned[str(key)] = clean(item)
        return cleaned
    if isinstance(value, str):
        return value
    return repr(value)


def slim_obj(obj: Dict[str, Any]) -> Dict[str, Any]:
    keys = [
        "object_type",
        "page_number",
        "text",
        "x0",
        "x1",
        "y0",
        "y1",
        "top",
        "bottom",
        "doctop",
        "width",
        "height",
        "fontname",
        "size",
        "adv",
        "upright",
        "direction",
        "non_stroking_color",
        "stroking_color",
        "orientation",
        "mcid",
        "tag",
        "uri",
        "title",
        "contents",
    ]
    return clean({k: obj[k] for k in keys if k in obj})


def slim_words(words: Iterable[Dict[str, Any]], limit: Optional[int] = None) -> List[Dict[str, Any]]:
    selected = list(words)
    if limit is not None:
        selected = selected[:limit]
    return [slim_obj(word) for word in selected]


def metadata_subset(metadata: Dict[str, Any]) -> Dict[str, Any]:
    keys = ["Author", "Creator", "Producer", "Title", "Subject", "Keywords", "CreationDate", "ModDate", "Copies"]
    return clean({k: metadata[k] for k in keys if k in metadata})


def object_counts(objects: Dict[str, List[Dict[str, Any]]]) -> Dict[str, int]:
    return {key: len(value) for key, value in sorted(objects.items())}


def page_geometry(page: Any) -> Dict[str, Any]:
    return clean(
        {
            "page_number": page.page_number,
            "width": page.width,
            "height": page.height,
            "bbox": page.bbox,
            "mediabox": page.mediabox,
            "cropbox": page.cropbox,
        }
    )


def text_line(text: str, index: int = -1) -> str:
    return text.split("\n")[index]


def table_cell_line(table_data: List[List[Optional[str]]], row: int, col: int, index: int = -1) -> Optional[str]:
    value = table_data[row][col]
    return None if value is None else value.split("\n")[index]


def table_sample_summary(table_data: List[List[Optional[str]]], cells: List[List[int]]) -> Dict[str, Any]:
    return clean(
        {
            "row_count": len(table_data),
            "column_count": len(table_data[0]) if table_data else 0,
            "cells": {f"{row},{col}": table_data[row][col] for row, col in cells},
        }
    )


def nics_plain_table_summary(table_data: List[List[Optional[str]]]) -> Dict[str, Any]:
    def parse_value(index: int, value: Optional[str]) -> Optional[int | str]:
        if index == 0:
            return value
        if value in (None, ""):
            return None
        return int(value.replace(",", ""))

    parsed = [[parse_value(index, value) for index, value in enumerate(row)] for row in table_data]
    column_checks = {}
    for index in range(1, len(parsed[0])):
        total = parsed[-1][index] or 0
        colsum = sum((row[index] or 0) for row in parsed)
        column_checks[str(index)] = {
            "total": total,
            "colsum": colsum,
            "matches_double_total": colsum == total * 2,
        }
    return clean(
        {
            "row_count": len(table_data),
            "column_count": len(table_data[0]) if table_data else 0,
            "all_columns_match_double_total": all(value["matches_double_total"] for value in column_checks.values()),
            "sample_column_checks": {key: column_checks[key] for key in ["1", "22", "24"] if key in column_checks},
        }
    )


def ca_warn_fix_row_spaces(row: List[Optional[str]]) -> List[Optional[str]]:
    return [(x or "").replace(" ", "") for x in row[:3]] + row[3:]


def ca_warn_parse_summary(pdf: Any) -> Dict[str, Any]:
    rect_x0_clusters = utils.cluster_list([r["x0"] for r in pdf.pages[1].rects], tolerance=3)
    v_lines = [x[0] for x in rect_x0_clusters]
    table_data = pdf.pages[0].extract_table({"vertical_strategy": "explicit", "explicit_vertical_lines": v_lines})
    return clean(
        {
            "v_lines": v_lines,
            "row_count": len(table_data),
            "column_count": len(table_data[0]) if table_data else 0,
            "header": ca_warn_fix_row_spaces(table_data[0]),
            "first_data_row": ca_warn_fix_row_spaces(table_data[1]),
            "last_data_row": ca_warn_fix_row_spaces(table_data[-1]),
        }
    )


def nics_explicit_horizontal_summary(page: Any) -> Dict[str, Any]:
    cropped = page.crop((0, 80, page.width, 475))
    table = cropped.find_tables({"horizontal_strategy": "text", "vertical_strategy": "text"})[0]
    h_positions = [row.cells[0][1] for row in table.rows] + [table.rows[-1].cells[0][3]]
    explicit = cropped.find_tables(
        {
            "horizontal_strategy": "explicit",
            "vertical_strategy": "text",
            "explicit_horizontal_lines": h_positions,
        }
    )[0]
    h_objects = [
        {
            "x0": 0,
            "x1": page.width,
            "width": page.width,
            "top": h,
            "bottom": h,
            "object_type": "line",
        }
        for h in h_positions
    ]
    explicit_objects = cropped.find_tables(
        {
            "horizontal_strategy": "explicit",
            "vertical_strategy": "text",
            "explicit_horizontal_lines": h_objects,
        }
    )[0]
    base = table.extract()
    return clean(
        {
            "h_count": len(h_positions),
            "first_h": h_positions[0],
            "last_h": h_positions[-1],
            "shape": [len(base), len(base[0]) if base else 0],
            "numbers_equal": base == explicit.extract(),
            "objects_equal": base == explicit_objects.extract(),
            "samples": {
                "0,0": base[0][0],
                "0,22": base[0][22],
                "-1,0": base[-1][0],
                "-1,22": base[-1][22],
            },
        }
    )


def table_rows_columns_summary(page: Any) -> Dict[str, Any]:
    table = page.find_table()
    row = [page.crop(bbox).extract_text() for bbox in table.rows[0].cells]
    col = [page.crop(bbox).extract_text() for bbox in table.columns[1].cells]
    return clean(
        {
            "cell_count": len(table.cells),
            "row_count": len(table.rows),
            "column_count": len(table.columns),
            "row0": row,
            "column1": col,
        }
    )


def capture_error_name(fn: Callable[[], Any]) -> Optional[str]:
    try:
        fn()
    except Exception as exc:
        return type(exc).__name__
    return None


def table_settings_error_summary(page: Any) -> Dict[str, Optional[str]]:
    return {
        "non_mapping": capture_error_name(lambda: table.TableFinder(page, tuple())),
        "unknown_setting": capture_error_name(lambda: table.TableFinder(page, {"strategy": "x"}).get_edges()),
        "invalid_vertical_strategy": capture_error_name(lambda: table.TableFinder(page, {"vertical_strategy": "x"})),
        "explicit_vertical_lines": capture_error_name(lambda: table.TableFinder(page, {"vertical_strategy": "explicit", "explicit_vertical_lines": []})),
        "negative_join_tolerance": capture_error_name(lambda: table.TableFinder(page, {"join_tolerance": -1}).get_edges()),
    }


def dedupe_extra_attrs_lines(page: Any) -> Dict[str, List[str]]:
    specs = [
        ("no_dedupe", None),
        ("none", ()),
        ("size", ("size",)),
        ("fontname", ("fontname",)),
        ("size_fontname", ("size", "fontname")),
    ]
    out: Dict[str, List[str]] = {}
    for name, keys in specs:
        filtered_page = page if keys is None else page.dedupe_chars(tolerance=2, extra_attrs=keys)
        out[name] = filtered_page.extract_text(y_tolerance=5).splitlines()
    return out


def layout_objects_summary(page: Any) -> Dict[str, Any]:
    props = [
        "textboxhorizontals",
        "textlinehorizontals",
        "textboxverticals",
        "textlineverticals",
    ]
    return clean(
        {
            "object_counts": object_counts(page.objects),
            "properties": {
                prop: {
                    "count": len(getattr(page, prop, [])),
                    "first_has_text": bool(getattr(page, prop, [])) and "text" in getattr(page, prop)[0],
                    "first_text": getattr(page, prop)[0].get("text") if getattr(page, prop, []) else None,
                }
                for prop in props
            },
        }
    )


def first_word_chars_summary(words: List[Dict[str, Any]]) -> Dict[str, Any]:
    first = words[0]
    chars = first.get("chars")
    return clean(
        {
            "first_has_chars": "chars" in first,
            "first_text": first.get("text"),
            "first_chars_text": "".join(char["text"] for char in chars) if isinstance(chars, list) else None,
            "first_chars_count": len(chars) if isinstance(chars, list) else None,
        }
    )


def ctm_summary(char: Dict[str, Any]) -> Dict[str, Any]:
    ctm = CTM(*char["matrix"])
    return clean(
        {
            "matrix": char["matrix"],
            "translation_x": ctm.translation_x,
            "translation_y": ctm.translation_y,
            "skew_x": ctm.skew_x,
            "skew_y": ctm.skew_y,
            "scale_x": ctm.scale_x,
            "scale_y": ctm.scale_y,
        }
    )


def make_check(type_: str, expected: Any, **kwargs: Any) -> Dict[str, Any]:
    check = {"type": type_, "expected": clean(expected)}
    check.update({key: value for key, value in kwargs.items() if value is not None})
    return check


def error_name(fn: Callable[[], Any]) -> Optional[str]:
    try:
        fn()
    except Exception as exc:
        return type(exc).__name__
    return None


def scenario(
    id_: str,
    pdf_name: str,
    build: Callable[[Any], List[Dict[str, Any]]],
    open_options: Optional[Dict[str, Any]] = None,
) -> Dict[str, Any]:
    with pdfplumber.open(PDF_DIR / pdf_name, **(open_options or {})) as pdf:
        checks = build(pdf)
    data: Dict[str, Any] = {"id": id_, "pdf": pdf_name, "checks": checks}
    if open_options:
        data["openOptions"] = open_options
    return data


def base_document_checks(pdf: Any) -> List[Dict[str, Any]]:
    page = pdf.pages[0]
    return [
        make_check("pdf.pageCount", len(pdf.pages)),
        make_check("pdf.metadata", metadata_subset(pdf.metadata)),
        make_check("pdf.objectCounts", object_counts(pdf.objects)),
        make_check("pdf.annots.count", len(pdf.annots)),
        make_check("pdf.hyperlinks.count", len(pdf.hyperlinks)),
        make_check("page.geometry", page_geometry(page), page=0),
        make_check("page.objectCounts", object_counts(page.objects), page=0),
        make_check(
            "page.edgeCounts",
            {"rect_edges": len(page.rect_edges), "curve_edges": len(page.curve_edges), "edges": len(page.edges)},
            page=0,
        ),
        make_check("page.chars.sample", [slim_obj(c) for c in page.chars[:8]], page=0, args={"count": 8}),
        make_check("page.extractText", page.extract_text(), page=0),
        make_check("page.extractWords", slim_words(page.extract_words(), 20), page=0, args={"limit": 20}),
    ]


def build_scenarios() -> List[Dict[str, Any]]:
    scenarios: List[Dict[str, Any]] = []

    scenarios.append(
        scenario(
            "open-basic-objects-text-and-edges",
            "nics-background-checks-2015-11.pdf",
            lambda pdf: base_document_checks(pdf)
            + [
                make_check("page.extractTable", pdf.pages[0].extract_table(), page=0),
                make_check("page.outsideBbox.extractText", pdf.pages[0].outside_bbox(pdf.pages[0].find_tables()[0].bbox).extract_text(), page=0, bbox=pdf.pages[0].find_tables()[0].bbox),
            ],
        )
    )

    scenarios.append(
        scenario(
            "basic-colors",
            "nics-background-checks-2015-11.pdf",
            lambda pdf: [
                make_check("page.object", slim_obj(pdf.pages[0].rects[0]), page=0, args={"objectType": "rect", "index": 0}),
                make_check("page.object", slim_obj(pdf.pages[0].chars[3358]), page=0, args={"objectType": "char", "index": 3358}),
            ],
        )
    )

    scenarios.append(
        scenario(
            "list-metadata-load",
            "cupertino_usd_4-6-16.pdf",
            lambda pdf: [
                make_check("pdf.metadata", metadata_subset(pdf.metadata)),
            ],
        )
    )

    scenarios.append(
        scenario(
            "pages-option-load",
            "WARN-Report-for-7-1-2015-to-03-25-2016.pdf",
            lambda pdf: [
                make_check("pdf.pageCount", len(pdf.pages)),
                make_check("page.geometry", page_geometry(pdf.pages[0]), page=0),
                make_check("page.geometry", page_geometry(pdf.pages[1]), page=1),
            ],
            open_options={"pages": [1, 3]},
        )
    )

    def crop_filter_checks(pdf: Any) -> List[Dict[str, Any]]:
        page = pdf.pages[0]
        crop_bbox = (0, 0, 200, 200)
        cropped = page.crop(crop_bbox)
        within = page.within_bbox(crop_bbox)
        filtered = cropped.filter(lambda obj: obj["object_type"] == "char")
        bottom_bbox = (0, 0.8 * float(page.height), page.width, page.height)
        small_crop = page.crop((10, 10, 40, 40))
        relative_bbox = (10, 15, 20, 25)
        bottom = page.crop(bottom_bbox)
        crop_right_bbox = (page.width / 2, 0, page.width, page.height)
        crop_right = page.crop(crop_right_bbox)
        invalid_bboxes = [
            (0, 0, 0, 0),
            (0, 0, 10000, 10),
            (-10, 0, 10, 10),
            (100, 0, 0, 100),
            (0, 100, 100, 0),
        ]
        return [
            make_check("page.crop.geometry", page_geometry(cropped), page=0, bbox=crop_bbox),
            make_check("page.crop.objectCounts", object_counts(cropped.objects), page=0, bbox=crop_bbox),
            make_check("page.withinBbox.objectCounts", object_counts(within.objects), page=0, bbox=crop_bbox),
            make_check("page.crop.filter.objectCounts", object_counts(filtered.objects), page=0, bbox=crop_bbox, args={"objectType": "char"}),
            make_check("page.crop.crop.geometry", page_geometry(small_crop.crop(relative_bbox, relative=True)), page=0, bbox=relative_bbox, relative=True, args={"baseBbox": (10, 10, 40, 40)}),
            make_check("page.crop.withinBbox.geometry", page_geometry(small_crop.within_bbox(relative_bbox, relative=True)), page=0, bbox=relative_bbox, relative=True, args={"baseBbox": (10, 10, 40, 40)}),
            make_check("page.crop.crop.objectCounts", object_counts(bottom.crop((0, 0, 0.5 * float(bottom.width), bottom.height), relative=True).objects), page=0, bbox=(0, 0, 0.5 * float(bottom.width), bottom.height), relative=True, args={"baseBbox": bottom_bbox}),
            make_check("page.crop.crop.objectCounts", object_counts(bottom.crop((0.5 * float(bottom.width), 0, bottom.width, bottom.height), relative=True).objects), page=0, bbox=(0.5 * float(bottom.width), 0, bottom.width, bottom.height), relative=True, args={"baseBbox": bottom_bbox}),
            make_check("page.crop.crop.objectCounts", object_counts(crop_right.crop((0, 0, crop_right.width / 2, page.height), relative=True).objects), page=0, bbox=(0, 0, crop_right.width / 2, page.height), relative=True, args={"baseBbox": crop_right_bbox}),
            *[
                make_check("page.crop.error", error_name(lambda bbox=bbox: page.crop(bbox)), page=0, bbox=bbox)
                for bbox in invalid_bboxes
            ],
            make_check("page.crop.crop.error", error_name(lambda: bottom.crop((0, 0, 0.5 * float(bottom.width), bottom.height))), page=0, bbox=(0, 0, 0.5 * float(bottom.width), bottom.height), args={"baseBbox": bottom_bbox}),
            make_check("page.crop.crop.error", error_name(lambda: bottom.crop((0.5 * float(bottom.width), 0, bottom.width, bottom.height))), page=0, bbox=(0.5 * float(bottom.width), 0, bottom.width, bottom.height), args={"baseBbox": bottom_bbox}),
            make_check("page.crop.error", error_name(lambda: page.crop((0, 0, page.width + 10, page.height + 10))), page=0, bbox=(0, 0, page.width + 10, page.height + 10)),
            make_check("page.crop.geometry", page_geometry(page.crop((0, 0, page.width + 10, page.height + 10), strict=False)), page=0, bbox=(0, 0, page.width + 10, page.height + 10), strict=False),
        ]

    scenarios.append(
        scenario(
            "crop-filter-and-validation",
            "nics-background-checks-2015-11.pdf",
            crop_filter_checks,
        )
    )

    def nics_filter_checks(pdf: Any) -> List[Dict[str, Any]]:
        page = pdf.pages[0]
        filtered = page.filter(lambda obj: not (obj["object_type"] == "char" and obj["size"] < 15))
        return [
            make_check("page.filterMinCharSize.extractText", filtered.extract_text(), page=0, args={"minSize": 15}),
            make_check("page.filterMinCharSize.objectCounts", object_counts(filtered.objects), page=0, args={"minSize": 15}),
        ]

    scenarios.append(
        scenario(
            "nics-filter-min-char-size",
            "nics-background-checks-2015-11.pdf",
            nics_filter_checks,
        )
    )

    scenarios.append(
        scenario(
            "nics-document-edges",
            "nics-background-checks-2015-11.pdf",
            lambda pdf: [
                make_check(
                    "pdf.edgeCounts",
                    {
                        "vertical_edges": len(pdf.vertical_edges),
                        "horizontal_edges": len(pdf.horizontal_edges),
                        "edges": len(pdf.edges),
                    },
                )
            ],
        )
    )

    def nics_plain_checks(pdf: Any) -> List[Dict[str, Any]]:
        page = pdf.pages[0]
        cropped = page.crop((0, 80, page.width, 485))
        table_data = cropped.extract_table(
            {
                "horizontal_strategy": "text",
                "explicit_vertical_lines": [min(c["x0"] for c in cropped.chars)],
                "intersection_tolerance": 5,
            }
        )
        return [
            make_check(
                "page.crop.extractTableNumericSummary",
                nics_plain_table_summary(table_data),
                page=0,
                bbox=(0, 80, page.width, 485),
                args={
                    "horizontal_strategy": "text",
                    "explicit_vertical_lines": [min(c["x0"] for c in cropped.chars)],
                    "intersection_tolerance": 5,
                },
            ),
            make_check("page.withinBbox.extractText", page.within_bbox((0, 35, page.width, 65)).extract_text(), page=0, bbox=(0, 35, page.width, 65)),
        ]

    scenarios.append(
        scenario(
            "nics-plain-table-and-month",
            "nics-background-checks-2015-11.pdf",
            nics_plain_checks,
        )
    )

    scenarios.append(
        scenario(
            "ca-warn-objects-and-parse",
            "WARN-Report-for-7-1-2015-to-03-25-2016.pdf",
            lambda pdf: [
                make_check("page.objectCounts", object_counts(pdf.pages[0].objects), page=0),
                make_check("page.edgeCounts", {"rect_edges": len(pdf.pages[0].rect_edges), "curve_edges": len(pdf.pages[0].curve_edges), "edges": len(pdf.pages[0].edges)}, page=0),
                make_check("pdf.caWarnParseSummary", ca_warn_parse_summary(pdf)),
            ],
        )
    )

    scenarios.append(
        scenario(
            "issue-14-objects",
            "cupertino_usd_4-6-16.pdf",
            lambda pdf: [make_check("pdf.objectCounts", object_counts(pdf.objects))],
        )
    )

    scenarios.append(
        scenario(
            "issue-21-objects",
            "150109DSP-Milw-505-90D.pdf",
            lambda pdf: [make_check("pdf.objectCounts", object_counts(pdf.objects))],
        )
    )

    scenarios.append(
        scenario(
            "issue-33-metadata-present",
            "issue-33-lorem-ipsum.pdf",
            lambda pdf: [make_check("pdf.metadataHasKeys", len(pdf.metadata.keys()) > 0)],
        )
    )

    scenarios.append(
        scenario(
            "issue-67-metadata-present",
            "issue-67-example.pdf",
            lambda pdf: [make_check("pdf.metadataHasKeys", len(pdf.metadata.keys()) > 0)],
        )
    )

    scenarios.append(
        scenario(
            "pr-88-word-count",
            "pr-88-example.pdf",
            lambda pdf: [make_check("page.extractWords.count", len(pdf.pages[0].extract_words()), page=0)],
        )
    )

    scenarios.append(
        scenario(
            "issue-90-extract-words-no-error",
            "issue-90-example.pdf",
            lambda pdf: [make_check("page.extractWords.error", error_name(lambda: pdf.pages[0].extract_words()), page=0)],
        )
    )

    scenarios.append(
        scenario(
            "pr-136-extract-words-no-error",
            "pr-136-example.pdf",
            lambda pdf: [make_check("page.extractWords.error", error_name(lambda: pdf.pages[0].extract_words()), page=0)],
        )
    )

    scenarios.append(
        scenario(
            "issue-203-objects",
            "issue-203-decimalize.pdf",
            lambda pdf: [make_check("pdf.objectCounts", object_counts(pdf.objects))],
        )
    )

    scenarios.append(
        scenario(
            "issue-216-empty-crop-table",
            "issue-140-example.pdf",
            lambda pdf: [
                make_check("page.crop.extractTable", pdf.pages[0].crop((0, 0, 1, 1)).extract_table(), page=0, bbox=(0, 0, 1, 1)),
            ],
        )
    )

    scenarios.append(
        scenario(
            "issue-297-integer-metadata",
            "issue-297-example.pdf",
            lambda pdf: [make_check("pdf.metadata", pdf.metadata)],
        )
    )

    scenarios.append(
        scenario(
            "issue-1147-extract-text",
            "issue-1147-example.pdf",
            lambda pdf: [make_check("page.extractText.line", text_line(pdf.pages[0].extract_text(), 0), page=0, args={"line": 0})],
        )
    )

    scenarios.append(
        scenario(
            "nics-table-text-only-strategy",
            "nics-background-checks-2015-11.pdf",
            lambda pdf: [
                make_check(
                    "page.crop.extractTableSummary",
                    table_sample_summary(
                        pdf.pages[0]
                        .crop((0, 80, pdf.pages[0].width, 475))
                        .extract_table({"horizontal_strategy": "text", "vertical_strategy": "text"}),
                        [[0, 0], [0, 22], [-1, 0], [-1, 22]],
                    ),
                    page=0,
                    bbox=(0, 80, pdf.pages[0].width, 475),
                    args={"horizontal_strategy": "text", "vertical_strategy": "text", "cells": [[0, 0], [0, 22], [-1, 0], [-1, 22]]},
                )
            ],
        )
    )

    scenarios.append(
        scenario(
            "nics-explicit-horizontal",
            "nics-background-checks-2015-11.pdf",
            lambda pdf: [
                make_check("page.nicsExplicitHorizontalSummary", nics_explicit_horizontal_summary(pdf.pages[0]), page=0),
            ],
        )
    )

    scenarios.append(
        scenario(
            "rotation-page-boxes",
            "nics-background-checks-2015-11-rotated.pdf",
            lambda pdf: [
                make_check("pdf.pageCount", len(pdf.pages)),
                make_check("page.geometry", page_geometry(pdf.pages[0]), page=0),
                make_check("page.objectCounts", object_counts(pdf.pages[0].objects), page=0),
            ],
        )
    )

    scenarios.append(
        scenario(
            "annotations-and-hyperlinks",
            "pdffill-demo.pdf",
            lambda pdf: [
                make_check("pdf.annots.count", len(pdf.annots)),
                make_check("pdf.hyperlinks.count", len(pdf.hyperlinks)),
                make_check("page.annots", [slim_obj(a) for a in pdf.pages[0].annots[:5]], page=0),
                make_check("page.hyperlinks", [slim_obj(a) for a in pdf.pages[0].hyperlinks], page=0),
                make_check("page.edgeCounts", {"rect_edges": len(pdf.pages[0].rect_edges), "curve_edges": len(pdf.pages[0].curve_edges), "edges": len(pdf.pages[0].edges)}, page=0),
            ],
        )
    )

    scenarios.append(
        scenario(
            "char-ctm-matrix",
            "pdffill-demo.pdf",
            lambda pdf: [
                make_check("page.ctmSummary", ctm_summary(pdf.pages[3].chars[97]), page=3, args={"index": 97}),
                make_check("page.ctmSummary", ctm_summary(pdf.pages[3].chars[105]), page=3, args={"index": 105}),
            ],
        )
    )

    def annotations_cropped_checks(pdf: Any) -> List[Dict[str, Any]]:
        page = pdf.pages[0]
        hyperlink_bbox = utils.obj_to_bbox(page.hyperlinks[0])
        return [
            make_check("page.annots.count", len(page.annots), page=0),
            make_check("page.hyperlinks.count", len(page.hyperlinks), page=0),
            make_check("page.crop.annots", [slim_obj(a) for a in page.crop(page.bbox).annots], page=0, bbox=page.bbox),
            make_check("page.crop.hyperlinks", [slim_obj(a) for a in page.crop(page.bbox).hyperlinks], page=0, bbox=page.bbox),
            make_check("page.crop.annots", [slim_obj(a) for a in page.crop(hyperlink_bbox).annots], page=0, bbox=hyperlink_bbox),
            make_check("page.crop.hyperlinks", [slim_obj(a) for a in page.crop(hyperlink_bbox).hyperlinks], page=0, bbox=hyperlink_bbox),
        ]

    scenarios.append(
        scenario(
            "annotations-cropped",
            "pdffill-demo.pdf",
            annotations_cropped_checks,
        )
    )

    for rotation_name, pdf_name in [
        ("annotations-rotated-base", "annotations.pdf"),
        ("annotations-rotated-180", "annotations-rotated-180.pdf"),
        ("annotations-rotated-90", "annotations-rotated-90.pdf"),
        ("annotations-rotated-270", "annotations-rotated-270.pdf"),
    ]:
        scenarios.append(
            scenario(
                rotation_name,
                pdf_name,
                lambda pdf: [
                    make_check("page.annots", [slim_obj(a) for a in pdf.pages[0].annots], page=0),
                ],
            )
        )

    scenarios.append(
        scenario(
            "password-open",
            "password-example.pdf",
            lambda pdf: [
                make_check("pdf.pageCount", len(pdf.pages)),
                make_check("page.extractText", pdf.pages[0].extract_text(), page=0),
                make_check("page.objectCounts", object_counts(pdf.pages[0].objects), page=0),
            ],
            open_options={"password": "test"},
        )
    )

    scenarios.append(
        scenario(
            "unicode-normalization",
            "issue-905.pdf",
            lambda pdf: [
                make_check("page.extractText", pdf.pages[0].extract_text(), page=0),
                make_check("page.chars.sample", [slim_obj(c) for c in pdf.pages[0].chars[:2]], page=0, args={"count": 2}),
            ],
            open_options={"unicode_norm": "NFC"},
        )
    )

    scenarios.append(
        scenario(
            "uncommon-page-boxes",
            "page-boxes-example.pdf",
            lambda pdf: [
                make_check(
                    "page.geometry",
                    {
                        **page_geometry(pdf.pages[0]),
                        "artbox": pdf.pages[0].artbox,
                        "bleedbox": pdf.pages[0].bleedbox,
                        "trimbox": pdf.pages[0].trimbox,
                    },
                    page=0,
                )
            ],
        )
    )

    scenarios.append(
        scenario(
            "laparams-custom-layout",
            "cupertino_usd_4-6-16.pdf",
            lambda pdf: [
                make_check("page.chars.sample", [slim_obj(c) for c in pdf.pages[0].chars[:5]], page=0, args={"count": 5}),
                make_check("page.objectCounts", object_counts(pdf.pages[0].objects), page=0),
            ],
            open_options={"laparams": {"line_margin": 0.2}},
        )
    )

    scenarios.append(
        scenario(
            "laparams-layout-objects-none",
            "issue-13-151201DSP-Fond-581-90D.pdf",
            lambda pdf: [
                make_check("page.layoutObjectsSummary", layout_objects_summary(pdf.pages[0]), page=0),
            ],
            open_options={"laparams": None},
        )
    )

    scenarios.append(
        scenario(
            "laparams-layout-objects-default",
            "issue-13-151201DSP-Fond-581-90D.pdf",
            lambda pdf: [
                make_check("page.layoutObjectsSummary", layout_objects_summary(pdf.pages[0]), page=0),
                make_check("page.crop.layoutObjectsSummary", layout_objects_summary(pdf.pages[0].crop((0, 0, 100, 100))), page=0, bbox=(0, 0, 100, 100)),
            ],
            open_options={"laparams": {}},
        )
    )

    scenarios.append(
        scenario(
            "laparams-layout-objects-vertical",
            "issue-192-example.pdf",
            lambda pdf: [
                make_check("page.layoutObjectsSummary", layout_objects_summary(pdf.pages[0]), page=0),
            ],
            open_options={"laparams": {"detect_vertical": True}},
        )
    )

    scenarios.append(
        scenario(
            "words-directions-and-extra-attrs",
            "issue-192-example.pdf",
            lambda pdf: [
                make_check("page.extractWords", slim_words(pdf.pages[0].extract_words(vertical_ttb=False), 30), page=0, args={"vertical_ttb": False, "limit": 30}),
                make_check("page.extractWords", slim_words(pdf.pages[0].extract_words(vertical_ttb=False, extra_attrs=["size"]), 30), page=0, args={"vertical_ttb": False, "extra_attrs": ["size"], "limit": 30}),
                make_check("page.extractWords", slim_words(pdf.pages[0].extract_words(horizontal_ltr=False), 30), page=0, args={"horizontal_ltr": False, "limit": 30}),
            ],
        )
    )

    scenarios.append(
        scenario(
            "extract-words-return-chars",
            "extra-attrs-example.pdf",
            lambda pdf: [
                make_check("page.extractWords.firstCharsSummary", first_word_chars_summary(pdf.pages[0].extract_words()), page=0),
                make_check("page.extractWords.firstCharsSummary", first_word_chars_summary(pdf.pages[0].extract_words(return_chars=True)), page=0, args={"return_chars": True}),
            ],
        )
    )

    scenarios.append(
        scenario(
            "x-tolerance-ratio",
            "issue-987-test.pdf",
            lambda pdf: [
                make_check("page.extractText", pdf.pages[0].extract_text(), page=0),
                make_check("page.extractText", pdf.pages[0].extract_text(x_tolerance=4), page=0, args={"x_tolerance": 4}),
                make_check("page.extractText", pdf.pages[0].extract_text(x_tolerance_ratio=0.15), page=0, args={"x_tolerance_ratio": 0.15}),
                make_check("page.extractWords", slim_words(pdf.pages[0].extract_words(x_tolerance_ratio=0.15)), page=0, args={"x_tolerance_ratio": 0.15}),
            ],
        )
    )

    scenarios.append(
        scenario(
            "ligatures",
            "issue-598-example.pdf",
            lambda pdf: [
                make_check("page.extractText", pdf.pages[0].extract_text(), page=0),
                make_check("page.extractText", pdf.pages[0].extract_text(expand_ligatures=False), page=0, args={"expand_ligatures": False}),
                make_check("page.extractWords", slim_words(pdf.pages[0].extract_words()[50:56]), page=0, args={"slice": [50, 56]}),
                make_check("page.extractWords", slim_words(pdf.pages[0].extract_words(expand_ligatures=False)[50:56]), page=0, args={"expand_ligatures": False, "slice": [50, 56]}),
            ],
        )
    )

    scenarios.append(
        scenario(
            "punctuation-splitting",
            "test-punkt.pdf",
            lambda pdf: [
                make_check("page.extractWords", slim_words(pdf.pages[0].extract_words(split_at_punctuation=True), 12), page=0, args={"split_at_punctuation": True, "limit": 12}),
                make_check("page.extractWords", slim_words(pdf.pages[0].extract_words(split_at_punctuation=False), 12), page=0, args={"split_at_punctuation": False, "limit": 12}),
                make_check("page.extractText", pdf.pages[0].extract_text(layout=True, split_at_punctuation=True), page=0, args={"layout": True, "split_at_punctuation": True}),
            ],
        )
    )

    scenarios.append(
        scenario(
            "search-and-text-lines",
            "scotus-transcript-p1.pdf",
            lambda pdf: [
                make_check("page.extractText", pdf.pages[0].extract_text(), page=0),
                make_check("page.search", [clean(r) for r in pdf.pages[0].search("Roberts", regex=False)[:5]], page=0, args={"pattern": "Roberts", "regex": False}),
                make_check("page.crop.extractText", pdf.pages[0].crop((0, 0, pdf.pages[0].width, 120)).extract_text(), page=0, bbox=(0, 0, pdf.pages[0].width, 120)),
            ],
        )
    )

    scenarios.append(
        scenario(
            "table-lines-strict",
            "issue-140-example.pdf",
            lambda pdf: [
                make_check(
                    "page.extractTable",
                    pdf.pages[0].extract_table({"vertical_strategy": "lines_strict", "horizontal_strategy": "lines_strict"}),
                    page=0,
                    args={"vertical_strategy": "lines_strict", "horizontal_strategy": "lines_strict"},
                ),
                make_check("page.crop.extractTable", pdf.pages[0].crop((0, 0, pdf.pages[0].width, 122)).extract_table(), page=0, bbox=(0, 0, pdf.pages[0].width, 122)),
            ],
        )
    )

    scenarios.append(
        scenario(
            "table-rows-and-columns",
            "issue-140-example.pdf",
            lambda pdf: [
                make_check("page.tableRowsColumnsSummary", table_rows_columns_summary(pdf.pages[0]), page=0),
            ],
        )
    )

    scenarios.append(
        scenario(
            "table-settings-errors",
            "pdffill-demo.pdf",
            lambda pdf: [
                make_check("page.tableSettingsErrorSummary", table_settings_error_summary(pdf.pages[0]), page=0),
            ],
        )
    )

    scenarios.append(
        scenario(
            "table-explicit-desc-decimalization",
            "pdffill-demo.pdf",
            lambda pdf: [
                make_check(
                    "page.extractTables",
                    pdf.pages[0].extract_tables(
                        {
                            "vertical_strategy": "explicit",
                            "explicit_vertical_lines": [100, 200, 300],
                            "horizontal_strategy": "explicit",
                            "explicit_horizontal_lines": [100, 200, 300],
                        }
                    ),
                    page=0,
                    args={
                        "vertical_strategy": "explicit",
                        "explicit_vertical_lines": [100, 200, 300],
                        "horizontal_strategy": "explicit",
                        "explicit_horizontal_lines": [100, 200, 300],
                    },
                ),
            ],
        )
    )

    scenarios.append(
        scenario(
            "table-text-strategy-and-tolerance",
            "senate-expenditures.pdf",
            lambda pdf: [
                make_check(
                    "page.crop.extractTable",
                    pdf.pages[0]
                    .crop((70.332, 130.986, 420, 509.106))
                    .extract_table({"horizontal_strategy": "text", "vertical_strategy": "text", "min_words_vertical": 20}),
                    page=0,
                    bbox=(70.332, 130.986, 420, 509.106),
                    args={"horizontal_strategy": "text", "vertical_strategy": "text", "min_words_vertical": 20},
                ),
                make_check(
                    "page.crop.extractTable",
                    pdf.pages[0]
                    .crop((70.332, 130.986, 420, 509.106))
                    .extract_table({"horizontal_strategy": "text", "vertical_strategy": "text", "min_words_vertical": 20, "text_x_tolerance": 1}),
                    page=0,
                    bbox=(70.332, 130.986, 420, 509.106),
                    args={"horizontal_strategy": "text", "vertical_strategy": "text", "min_words_vertical": 20, "text_x_tolerance": 1},
                ),
            ],
        )
    )

    scenarios.append(
        scenario(
            "table-text-layout",
            "issue-53-example.pdf",
            lambda pdf: [
                make_check("page.extractTable", pdf.pages[0].extract_table({"text_layout": True}), page=0, args={"text_layout": True})
            ],
        )
    )

    scenarios.append(
        scenario(
            "table-text-without-words",
            "pdffill-demo.pdf",
            lambda pdf: [
                make_check(
                    "page.crop.extractTables",
                    pdf.pages[0].crop((0, 0, 10, 10)).extract_tables({"vertical_strategy": "text", "horizontal_strategy": "text"}),
                    page=0,
                    bbox=(0, 0, 10, 10),
                    args={"vertical_strategy": "text", "horizontal_strategy": "text"},
                ),
                make_check(
                    "page.crop.extractTable",
                    pdf.pages[0].crop((0, 0, 10, 10)).extract_table({"vertical_strategy": "text", "horizontal_strategy": "text"}),
                    page=0,
                    bbox=(0, 0, 10, 10),
                    args={"vertical_strategy": "text", "horizontal_strategy": "text"},
                ),
            ],
        )
    )

    scenarios.append(
        scenario(
            "table-order",
            "issue-336-example.pdf",
            lambda pdf: [
                make_check("page.extractTables", pdf.pages[0].extract_tables(), page=0),
            ],
        )
    )

    scenarios.append(
        scenario(
            "table-mixed-strategy-issue-466",
            "issue-466-example.pdf",
            lambda pdf: [
                make_check(
                    "page.extractTables",
                    pdf.pages[0].extract_tables(
                        {
                            "vertical_strategy": "lines",
                            "horizontal_strategy": "text",
                            "snap_tolerance": 8,
                            "intersection_tolerance": 4,
                        }
                    ),
                    page=0,
                    args={
                        "vertical_strategy": "lines",
                        "horizontal_strategy": "text",
                        "snap_tolerance": 8,
                        "intersection_tolerance": 4,
                    },
                ),
            ],
        )
    )

    scenarios.append(
        scenario(
            "table-null-value-discussion-539",
            "nics-background-checks-2015-11.pdf",
            lambda pdf: [
                make_check(
                    "page.extractTable",
                    pdf.pages[0].extract_table(
                        {
                            "vertical_strategy": "lines",
                            "horizontal_strategy": "lines",
                            "explicit_vertical_lines": [],
                            "explicit_horizontal_lines": [],
                            "snap_tolerance": 3,
                            "join_tolerance": 3,
                            "edge_min_length": 3,
                            "min_words_vertical": 3,
                            "min_words_horizontal": 1,
                            "text_keep_blank_chars": False,
                            "text_tolerance": 3,
                            "intersection_tolerance": 3,
                        }
                    ),
                    page=0,
                    args={
                        "vertical_strategy": "lines",
                        "horizontal_strategy": "lines",
                        "explicit_vertical_lines": [],
                        "explicit_horizontal_lines": [],
                        "snap_tolerance": 3,
                        "join_tolerance": 3,
                        "edge_min_length": 3,
                        "min_words_vertical": 3,
                        "min_words_horizontal": 1,
                        "text_keep_blank_chars": False,
                        "text_tolerance": 3,
                        "intersection_tolerance": 3,
                    },
                ),
                make_check(
                    "page.extractTables",
                    pdf.pages[0].extract_tables(
                        {
                            "vertical_strategy": "lines",
                            "horizontal_strategy": "lines",
                            "explicit_vertical_lines": [],
                            "explicit_horizontal_lines": [],
                            "snap_tolerance": 3,
                            "join_tolerance": 3,
                            "edge_min_length": 3,
                            "min_words_vertical": 3,
                            "min_words_horizontal": 1,
                            "text_keep_blank_chars": False,
                            "text_tolerance": 3,
                            "intersection_tolerance": 3,
                        }
                    ),
                    page=0,
                    args={
                        "vertical_strategy": "lines",
                        "horizontal_strategy": "lines",
                        "explicit_vertical_lines": [],
                        "explicit_horizontal_lines": [],
                        "snap_tolerance": 3,
                        "join_tolerance": 3,
                        "edge_min_length": 3,
                        "min_words_vertical": 3,
                        "min_words_horizontal": 1,
                        "text_keep_blank_chars": False,
                        "text_tolerance": 3,
                        "intersection_tolerance": 3,
                    },
                ),
            ],
        )
    )

    scenarios.append(
        scenario(
            "table-curves",
            "table-curves-example.pdf",
            lambda pdf: [
                make_check("page.objectCounts", object_counts(pdf.pages[0].objects), page=0),
                make_check("page.extractTables", pdf.pages[0].extract_tables(), page=0),
                make_check("page.extractTables", pdf.pages[0].extract_tables({"vertical_strategy": "lines_strict"}), page=0, args={"vertical_strategy": "lines_strict"}),
            ],
        )
    )

    scenarios.append(
        scenario(
            "mediabox-offset-table-coordinates",
            "issue-1181.pdf",
            lambda pdf: [
                make_check("page.geometry", page_geometry(pdf.pages[0]), page=0),
                make_check("page.geometry", page_geometry(pdf.pages[1]), page=1),
                make_check("page.crop.extractTable", pdf.pages[0].crop(pdf.pages[0].bbox).extract_table(), page=0, bbox=pdf.pages[0].bbox),
                make_check("page.crop.extractTable", pdf.pages[1].crop(pdf.pages[1].bbox).extract_table(), page=1, bbox=pdf.pages[1].bbox),
            ],
        )
    )

    scenarios.append(
        scenario(
            "dedupe-chars",
            "issue-71-duplicate-chars-2.pdf",
            lambda pdf: [
                make_check("page.objectCounts", object_counts(pdf.pages[0].objects), page=0),
                make_check("page.extractText", pdf.pages[0].extract_text(), page=0),
                make_check("page.extractText", pdf.pages[0].dedupe_chars().extract_text(), page=0, args={"dedupe_chars": True}),
                make_check("page.search", [clean(r) for r in pdf.pages[0].search(r"\\d+", regex=True)[:5]], page=0, args={"pattern": r"\\d+", "regex": True}),
            ],
        )
    )

    scenarios.append(
        scenario(
            "dedupe-chars-primary",
            "issue-71-duplicate-chars.pdf",
            lambda pdf: [
                make_check("page.extractTable.cellLine", table_cell_line(pdf.pages[0].extract_table(), 1, 1), page=0, args={"row": 1, "col": 1, "line": -1}),
                make_check("page.dedupe.extractTable.cellLine", table_cell_line(pdf.pages[0].dedupe_chars().extract_table(), 1, 1), page=0, args={"row": 1, "col": 1, "line": -1}),
                make_check("page.extractWords", slim_words(pdf.pages[0].extract_words()[-1:]), page=0, args={"slice": [-1]}),
                make_check("page.dedupe.extractWords", slim_words(pdf.pages[0].dedupe_chars().extract_words()[-1:]), page=0, args={"slice": [-1]}),
                make_check("page.extractText.line", text_line(pdf.pages[0].extract_text()), page=0, args={"line": -1}),
                make_check("page.dedupe.extractText.line", text_line(pdf.pages[0].dedupe_chars().extract_text()), page=0, args={"line": -1}),
            ],
        )
    )

    scenarios.append(
        scenario(
            "dedupe-extra-attrs",
            "issue-1114-dedupe-chars.pdf",
            lambda pdf: [
                make_check("page.dedupeExtraAttrsLines", dedupe_extra_attrs_lines(pdf.pages[0]), page=0),
            ],
        )
    )

    scenarios.append(
        scenario(
            "marked-content-ids",
            "mcid_example.pdf",
            lambda pdf: [
                make_check("page.chars.sample", [slim_obj(c) for c in pdf.pages[0].chars[:20]], page=0, args={"count": 20}),
                make_check("page.objectCounts", object_counts(pdf.pages[0].objects), page=0),
            ],
        )
    )

    return scenarios


def git_head() -> str:
    try:
        return subprocess.check_output(["git", "-C", str(PY_REF), "rev-parse", "HEAD"], text=True).strip()
    except Exception:
        return "unknown"


def main() -> None:
    OUT.parent.mkdir(parents=True, exist_ok=True)
    payload = {
        "reference": {
            "generatedAt": datetime.now(timezone.utc).isoformat(),
            "pdfplumberVersion": getattr(pdfplumber, "__version__", "unknown"),
            "pdfminerVersion": getattr(pdfminer, "__version__", "unknown") if pdfminer else "unknown",
            "pdfplumberCommit": git_head(),
        },
        "scenarios": build_scenarios(),
    }
    OUT.write_text(json.dumps(payload, indent=2, ensure_ascii=False, sort_keys=True) + "\n", encoding="utf-8")
    print(f"Wrote {OUT.relative_to(ROOT)} with {len(payload['scenarios'])} scenarios")


if __name__ == "__main__":
    main()
