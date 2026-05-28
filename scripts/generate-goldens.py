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
from pdfplumber import utils  # noqa: E402

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
