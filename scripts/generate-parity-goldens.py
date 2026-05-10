#!/usr/bin/env python3
from __future__ import annotations

import hashlib
import json
import math
import pathlib
import subprocess
import sys
from datetime import datetime, timezone
from typing import Any, Callable, Dict, Iterable, List, Optional, Sequence

ROOT = pathlib.Path(__file__).resolve().parents[1]
PY_REF = ROOT / "pdfplumber-python"
PDF_DIR = PY_REF / "tests" / "pdfs"
OUT = ROOT / "test" / "fixtures" / "goldens" / "pdfplumber-parity.json"

sys.path.insert(0, str(PY_REF))

import pdfplumber  # noqa: E402

try:
    import pdfminer  # noqa: E402
except Exception:  # pragma: nocover
    pdfminer = None


JSONScalar = Optional[str | int | float | bool]
CheckBuilder = Callable[[Any], List[Dict[str, Any]]]
HASH_NUMERIC_DIGITS = 3


def finite_number(value: Any, digits: int = 6) -> JSONScalar:
    if isinstance(value, bool):
        return value
    if isinstance(value, int):
        return value
    if isinstance(value, float):
        if not math.isfinite(value):
            return None
        if digits < 6:
            sign = -1 if value < 0 else 1
            fixed = f"{abs(value):.6f}"
            whole, _, fraction = fixed.partition(".")
            kept = fraction[:digits].ljust(digits, "0")
            next_digit = fraction[digits] if digits < len(fraction) else "0"
            rest = fraction[digits + 1 :]
            last_kept = int(kept[-1] if digits > 0 and kept else whole[-1])
            round_up = next_digit > "5" or (
                next_digit == "5" and (any(ch != "0" for ch in rest) or last_kept % 2 == 1)
            )
            scale = 10**digits
            scaled = int(whole) * scale + (int(kept) if kept else 0)
            if round_up:
                scaled += 1
            rounded = sign * scaled / scale
            if abs(rounded - round(rounded)) < 1e-9:
                return int(round(rounded))
            return 0 if rounded == 0 else rounded
        rounded = round(value, digits)
        if abs(rounded - round(rounded)) < 1e-9:
            return int(round(rounded))
        return rounded
    return None


def clean(value: Any, digits: int = 6) -> Any:
    number = finite_number(value, digits)
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
        return [clean(v, digits) for v in value]
    if isinstance(value, dict):
        cleaned: Dict[str, Any] = {}
        for key, item in value.items():
            if key in {"stream", "data", "graphicstate"}:
                continue
            if isinstance(item, (bytes, str, int, float, bool, list, tuple, dict)) or item is None:
                cleaned[str(key)] = clean(item, digits)
        return cleaned
    if isinstance(value, str):
        return value
    return repr(value)


def stable_json(value: Any, digits: int = 6) -> str:
    return json.dumps(clean(value, digits), ensure_ascii=False, sort_keys=True, separators=(",", ":"))


def sha256_value(value: Any) -> str:
    return hashlib.sha256(stable_json(value, HASH_NUMERIC_DIGITS).encode("utf-8")).hexdigest()


def clipped_text(text: str, count: int = 240) -> Dict[str, str]:
    return {
        "head": text[:count],
        "tail": text[-count:] if len(text) > count else text,
    }


def error_summary(exc: BaseException) -> Dict[str, Any]:
    return {
        "status": "error",
        "errorType": type(exc).__name__,
        "message": str(exc)[:240],
    }


def with_status(fn: Callable[[], Any], summarize: Callable[[Any], Any] = clean) -> Dict[str, Any]:
    try:
        return {"status": "ok", "value": summarize(fn())}
    except Exception as exc:
        return error_summary(exc)


def sample_indices(length: int, limit: int = 8) -> List[int]:
    if length <= limit:
        return list(range(length))
    raw = [0, 1, length // 2 - 1, length // 2, length // 2 + 1, length - 2, length - 1]
    return sorted({i for i in raw if 0 <= i < length})


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
        "matrix",
        "non_stroking_color",
        "stroking_color",
        "non_stroking_pattern",
        "stroking_pattern",
        "ncs",
        "linewidth",
        "dash",
        "fill",
        "stroke",
        "evenodd",
        "orientation",
        "mcid",
        "tag",
        "uri",
        "title",
        "contents",
        "name",
        "srcsize",
        "colorspace",
        "bits",
    ]
    return clean({key: obj[key] for key in keys if key in obj})


def slim_search_result(obj: Dict[str, Any]) -> Dict[str, Any]:
    result = slim_obj(obj)
    chars = obj.get("chars")
    if isinstance(chars, list):
        result["chars"] = object_list_summary(chars, limit=5)
    return result


def object_list_summary(objects: Iterable[Dict[str, Any]], limit: int = 8) -> Dict[str, Any]:
    slimmed = [slim_obj(obj) for obj in objects]
    indices = sample_indices(len(slimmed), limit)
    return {
        "count": len(slimmed),
        "sha256": sha256_value(slimmed),
        "sampleIndices": indices,
        "samples": [slimmed[i] for i in indices],
    }


def search_summary(results: Iterable[Dict[str, Any]]) -> Dict[str, Any]:
    slimmed = [slim_search_result(result) for result in results]
    indices = sample_indices(len(slimmed), 8)
    return {
        "count": len(slimmed),
        "sha256": sha256_value(slimmed),
        "sampleIndices": indices,
        "samples": [slimmed[i] for i in indices],
    }


def text_summary(text: Optional[str]) -> Dict[str, Any]:
    value = text or ""
    return {
        "length": len(value),
        "lineCount": value.count("\n") + 1 if value else 0,
        "sha256": hashlib.sha256(value.encode("utf-8")).hexdigest(),
        **clipped_text(value),
    }


def table_shapes(value: Any) -> Any:
    if value is None:
        return None
    if not isinstance(value, list):
        return None
    if not value:
        return []
    if all(isinstance(row, list) and (not row or not isinstance(row[0], list)) for row in value):
        return [len(value), max((len(row) for row in value), default=0)]
    return [table_shapes(table) for table in value]


def clipped_table(value: Any, limit: int = 3) -> Any:
    if value is None or not isinstance(value, list):
        return clean(value)
    if len(value) <= limit * 2:
        return clean(value)
    return clean(value[:limit] + [["..."]] + value[-limit:])


def json_value_summary(value: Any) -> Dict[str, Any]:
    cleaned = clean(value)
    encoded = stable_json(cleaned)
    return {
        "kind": "null" if cleaned is None else type(cleaned).__name__,
        "shape": table_shapes(cleaned),
        "jsonLength": len(encoded),
        "sha256": hashlib.sha256(encoded.encode("utf-8")).hexdigest(),
        "sample": clipped_table(cleaned),
    }


def find_tables_summary(tables: Sequence[Any]) -> Dict[str, Any]:
    summaries = []
    for table in tables:
        rows = getattr(table, "rows", [])
        columns = getattr(table, "columns", [])
        summaries.append(
            clean(
                {
                    "bbox": table.bbox,
                    "cellCount": len(table.cells),
                    "rowCount": len(rows),
                    "columnCount": len(columns),
                    "rowShapes": [len(row.cells) for row in rows[:6]],
                }
            )
        )
    return {
        "count": len(summaries),
        "sha256": sha256_value(summaries),
        "tables": summaries,
    }


def metadata_subset(metadata: Dict[str, Any]) -> Dict[str, Any]:
    keys = ["Author", "Creator", "Producer", "Title", "Subject", "Keywords", "CreationDate", "ModDate", "Copies"]
    return clean({key: metadata[key] for key in keys if key in metadata})


def object_counts(objects: Dict[str, List[Dict[str, Any]]]) -> Dict[str, int]:
    return {key: len(value) for key, value in sorted(objects.items())}


def page_geometry(page: Any) -> Dict[str, Any]:
    data = {
        "page_number": page.page_number,
        "width": page.width,
        "height": page.height,
        "bbox": page.bbox,
        "mediabox": page.mediabox,
        "cropbox": page.cropbox,
    }
    for key in ["artbox", "bleedbox", "trimbox"]:
        if hasattr(page, key):
            data[key] = getattr(page, key)
    return clean(data)


def page_snapshot(page: Any) -> Dict[str, Any]:
    objects = page.objects
    object_samples = {
        key: object_list_summary(objects.get(key, []))
        for key in [
            "char",
            "line",
            "rect",
            "curve",
            "image",
            "annot",
            "textboxhorizontal",
            "textlinehorizontal",
            "textboxvertical",
            "textlinevertical",
        ]
        if key in objects
    }
    return {
        "geometry": page_geometry(page),
        "objectCounts": object_counts(objects),
        "edgeCounts": {
            "rect_edges": len(page.rect_edges),
            "curve_edges": len(page.curve_edges),
            "edges": len(page.edges),
        },
        "objectSamples": object_samples,
        "annots": object_list_summary(page.annots),
        "hyperlinks": object_list_summary(page.hyperlinks),
        "extractText": with_status(lambda: page.extract_text(), text_summary),
        "extractWords": with_status(lambda: page.extract_words(), object_list_summary),
    }


def document_snapshot(
    pdf_path: pathlib.Path,
    open_options: Optional[Dict[str, Any]] = None,
    page_indices: Optional[List[int]] = None,
) -> Dict[str, Any]:
    try:
        with pdfplumber.open(pdf_path, **(open_options or {})) as pdf:
            selected_indices = page_indices if page_indices is not None else list(range(len(pdf.pages)))
            pages = [page_snapshot(pdf.pages[index]) for index in selected_indices]
            payload = {
                "status": "ok",
                "metadata": metadata_subset(pdf.metadata),
                "pageCount": len(pdf.pages),
                **({"pageIndices": selected_indices} if page_indices is not None else {}),
                "pages": pages,
            }
            if page_indices is None:
                payload.update(
                    {
                        "objectCounts": object_counts(pdf.objects),
                        "annots": object_list_summary(pdf.annots),
                        "hyperlinks": object_list_summary(pdf.hyperlinks),
                    }
                )
            return payload
    except Exception as exc:
        return error_summary(exc)


def make_check(type_: str, expected: Any, **kwargs: Any) -> Dict[str, Any]:
    check = {"type": type_, "expected": clean(expected)}
    check.update({key: value for key, value in kwargs.items() if value is not None})
    return check


def operation_checks(pdf: Any, operations: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
    checks = []
    for operation in operations:
        attrs = {key: value for key, value in operation.items() if key != "type"}
        page = pdf.pages[int(operation.get("page", 0))]
        bbox = operation.get("bbox")
        target = page
        if bbox is not None:
            method = operation.get("bboxMethod", "crop")
            if method == "outside":
                target = page.outside_bbox(tuple(bbox))
            elif method == "within":
                target = page.within_bbox(tuple(bbox))
            else:
                target = page.crop(tuple(bbox))
        if operation.get("dedupe"):
            target = target.dedupe_chars(**operation.get("dedupeOptions", {}))

        options = operation.get("options", {})
        check_type = operation["type"]
        if check_type == "text":
            expected = with_status(lambda target=target, options=options: target.extract_text(**options), text_summary)
            checks.append(make_check("page.textSummary", expected, **attrs))
        elif check_type == "words":
            expected = with_status(lambda target=target, options=options: target.extract_words(**options), object_list_summary)
            checks.append(make_check("page.wordsSummary", expected, **attrs))
        elif check_type == "search":
            pattern = operation["pattern"]
            expected = with_status(lambda target=target, pattern=pattern, options=options: target.search(pattern, **options), search_summary)
            checks.append(make_check("page.searchSummary", expected, **attrs))
        elif check_type == "extractTable":
            expected = with_status(lambda target=target, options=options: target.extract_table(options), json_value_summary)
            checks.append(make_check("page.extractTableSummary", expected, **attrs))
        elif check_type == "extractTables":
            expected = with_status(lambda target=target, options=options: target.extract_tables(options), json_value_summary)
            checks.append(make_check("page.extractTablesSummary", expected, **attrs))
        elif check_type == "findTables":
            expected = with_status(lambda target=target, options=options: target.find_tables(options), find_tables_summary)
            checks.append(make_check("page.findTablesSummary", expected, **attrs))
        else:
            raise ValueError(f"Unknown operation type: {check_type}")
    return checks


def scenario(
    id_: str,
    pdf_name: str,
    build: CheckBuilder,
    open_options: Optional[Dict[str, Any]] = None,
) -> Dict[str, Any]:
    with pdfplumber.open(PDF_DIR / pdf_name, **(open_options or {})) as pdf:
        checks = build(pdf)
    data: Dict[str, Any] = {"id": id_, "pdf": pdf_name, "checks": checks}
    if open_options:
        data["openOptions"] = open_options
    return data


def corpus_scenarios() -> List[Dict[str, Any]]:
    scenarios = []
    for pdf_path in sorted(PDF_DIR.glob("*.pdf")):
        expected = document_snapshot(pdf_path)
        scenarios.append(
            {
                "id": f"corpus/default/{pdf_path.name}",
                "pdf": pdf_path.name,
                "checks": [make_check("document.snapshot", expected)],
            }
        )
    return scenarios


def targeted_scenarios() -> List[Dict[str, Any]]:
    scenarios: List[Dict[str, Any]] = []

    for id_, pdf_name, open_options in [
        ("variant/password-open", "password-example.pdf", {"password": "test"}),
        ("variant/unicode-normalization-nfc", "issue-905.pdf", {"unicode_norm": "NFC"}),
        ("variant/laparams-empty", "issue-13-151201DSP-Fond-581-90D.pdf", {"laparams": {}}),
        ("variant/laparams-custom-layout", "cupertino_usd_4-6-16.pdf", {"laparams": {"line_margin": 0.2}}),
        ("variant/laparams-detect-vertical", "issue-192-example.pdf", {"laparams": {"detect_vertical": True}}),
    ]:
        scenarios.append(
            {
                "id": id_,
                "pdf": pdf_name,
                "openOptions": open_options,
                "checks": [make_check("document.snapshot", document_snapshot(PDF_DIR / pdf_name, open_options))],
            }
        )

    text_word_ops = [
        (
            "text/x-tolerance-ratio",
            "issue-987-test.pdf",
            [
                {"type": "text"},
                {"type": "text", "options": {"x_tolerance": 4}},
                {"type": "text", "options": {"x_tolerance_ratio": 0.15}},
                {"type": "words", "options": {"x_tolerance_ratio": 0.15}},
            ],
        ),
        (
            "text/ligatures",
            "issue-598-example.pdf",
            [
                {"type": "text"},
                {"type": "text", "options": {"expand_ligatures": False}},
                {"type": "words"},
                {"type": "words", "options": {"expand_ligatures": False}},
            ],
        ),
        (
            "text/punctuation",
            "test-punkt.pdf",
            [
                {"type": "words", "options": {"split_at_punctuation": True}},
                {"type": "words", "options": {"split_at_punctuation": False}},
                {"type": "text", "options": {"layout": True, "split_at_punctuation": True}},
            ],
        ),
        (
            "text/use-text-flow",
            "issue-982-example.pdf",
            [
                {"type": "text"},
                {"type": "text", "options": {"use_text_flow": True}},
                {"type": "words", "options": {"use_text_flow": True}},
            ],
        ),
        (
            "text/directions-and-extra-attrs",
            "issue-192-example.pdf",
            [
                {"type": "words", "options": {"vertical_ttb": False}},
                {"type": "words", "options": {"vertical_ttb": False, "extra_attrs": ["size"]}},
                {"type": "words", "options": {"horizontal_ltr": False}},
            ],
        ),
        (
            "search/scotus",
            "scotus-transcript-p1.pdf",
            [
                {"type": "search", "pattern": "Roberts", "options": {"regex": False}},
                {"type": "search", "pattern": r"\b[A-Z][a-z]+,\sJ\.", "options": {"regex": True}},
                {"type": "text", "bbox": [0, 0, 612, 120]},
            ],
        ),
        (
            "crop/inside-outside-relative",
            "nics-background-checks-2015-11.pdf",
            [
                {"type": "text", "bbox": [0, 0, 400, 120]},
                {"type": "text", "bbox": [0, 0, 400, 120], "bboxMethod": "outside"},
                {"type": "words", "bbox": [0, 0, 400, 120]},
            ],
        ),
    ]

    for id_, pdf_name, operations in text_word_ops:
        scenarios.append(scenario(id_, pdf_name, lambda pdf, operations=operations: operation_checks(pdf, operations)))

    dedupe_ops = [
        (
            "dedupe/issue-71",
            "issue-71-duplicate-chars.pdf",
            [
                {"type": "text"},
                {"type": "text", "dedupe": True},
                {"type": "words", "dedupe": True},
                {"type": "extractTable"},
                {"type": "extractTable", "dedupe": True},
            ],
        ),
        (
            "dedupe/issue-71-large",
            "issue-71-duplicate-chars-2.pdf",
            [
                {"type": "text", "dedupe": True, "options": {"y_tolerance": 6}},
                {"type": "search", "pattern": r"\d+", "options": {"regex": True}, "dedupe": True},
            ],
        ),
        (
            "dedupe/extra-attrs",
            "issue-1114-dedupe-chars.pdf",
            [
                {"type": "text", "options": {"y_tolerance": 5}},
                {"type": "text", "options": {"y_tolerance": 5}, "dedupe": True, "dedupeOptions": {"tolerance": 2, "extra_attrs": []}},
                {"type": "text", "options": {"y_tolerance": 5}, "dedupe": True, "dedupeOptions": {"tolerance": 2, "extra_attrs": ["size"]}},
                {"type": "text", "options": {"y_tolerance": 5}, "dedupe": True, "dedupeOptions": {"tolerance": 2, "extra_attrs": ["fontname"]}},
                {"type": "text", "options": {"y_tolerance": 5}, "dedupe": True, "dedupeOptions": {"tolerance": 2, "extra_attrs": ["size", "fontname"]}},
            ],
        ),
    ]

    for id_, pdf_name, operations in dedupe_ops:
        scenarios.append(scenario(id_, pdf_name, lambda pdf, operations=operations: operation_checks(pdf, operations)))

    table_ops = [
        (
            "table/nics-default-and-text",
            "nics-background-checks-2015-11.pdf",
            [
                {"type": "findTables"},
                {"type": "extractTable"},
                {
                    "type": "extractTable",
                    "options": {
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
                },
                {"type": "extractTable", "options": {"vertical_strategy": "text", "horizontal_strategy": "text"}},
            ],
        ),
        (
            "table/lines-strict",
            "issue-140-example.pdf",
            [
                {"type": "findTables"},
                {"type": "extractTable", "options": {"vertical_strategy": "lines_strict", "horizontal_strategy": "lines_strict"}},
                {"type": "extractTable", "bbox": [0, 0, 700, 122]},
            ],
        ),
        (
            "table/text-strategy-and-tolerance",
            "senate-expenditures.pdf",
            [
                {
                    "type": "extractTable",
                    "bbox": [70.332, 130.986, 420, 509.106],
                    "options": {"horizontal_strategy": "text", "vertical_strategy": "text", "min_words_vertical": 20},
                },
                {
                    "type": "extractTable",
                    "bbox": [70.332, 130.986, 420, 509.106],
                    "options": {"horizontal_strategy": "text", "vertical_strategy": "text", "min_words_vertical": 20, "text_x_tolerance": 1},
                },
                {
                    "type": "extractTables",
                    "bbox": [70.332, 130.986, 420, 509.106],
                    "options": {"horizontal_strategy": "text", "vertical_strategy": "text", "min_words_vertical": 20, "text_x_tolerance": 1},
                },
            ],
        ),
        ("table/text-layout", "issue-53-example.pdf", [{"type": "extractTable", "options": {"text_layout": True}}]),
        ("table/order-issue-336", "issue-336-example.pdf", [{"type": "extractTables"}]),
        (
            "table/mixed-strategy-issue-466",
            "issue-466-example.pdf",
            [
                {
                    "type": "extractTables",
                    "options": {
                        "vertical_strategy": "lines",
                        "horizontal_strategy": "text",
                        "snap_tolerance": 8,
                        "intersection_tolerance": 4,
                    },
                }
            ],
        ),
        (
            "table/curves",
            "table-curves-example.pdf",
            [
                {"type": "findTables"},
                {"type": "extractTables"},
                {"type": "extractTables", "options": {"vertical_strategy": "lines_strict"}},
            ],
        ),
    ]

    for id_, pdf_name, operations in table_ops:
        scenarios.append(scenario(id_, pdf_name, lambda pdf, operations=operations: operation_checks(pdf, operations)))

    scenarios.append(
        scenario(
            "table/mediabox-offset",
            "issue-1181.pdf",
            lambda pdf: operation_checks(
                pdf,
                [
                    {"type": "extractTable", "page": 0, "bbox": list(pdf.pages[0].bbox)},
                    {"type": "extractTable", "page": 1, "bbox": list(pdf.pages[1].bbox)},
                ],
            ),
        )
    )

    scenarios.append(
        scenario(
            "marked-content/mcid-example",
            "mcid_example.pdf",
            lambda pdf: [
                make_check("document.snapshot", document_snapshot(PDF_DIR / "mcid_example.pdf")),
                make_check(
                    "page.mcidText",
                    clean(
                        [
                            {"mcid": i, "text": text}
                            for i, text in enumerate(
                                build_mcid_text(pdf.pages[0])
                            )
                        ]
                    ),
                    page=0,
                ),
            ],
        )
    )

    return scenarios


def build_mcid_text(page: Any) -> List[str]:
    mcids: List[str] = []
    for char in page.chars:
        if "mcid" not in char:
            continue
        while len(mcids) <= char["mcid"]:
            mcids.append("")
        if not mcids[char["mcid"]]:
            mcids[char["mcid"]] = char["tag"] + ": "
        mcids[char["mcid"]] += char["text"]
    return mcids


def git_head() -> str:
    try:
        return subprocess.check_output(["git", "-C", str(PY_REF), "rev-parse", "HEAD"], text=True).strip()
    except Exception:
        return "unknown"


def main() -> None:
    OUT.parent.mkdir(parents=True, exist_ok=True)
    scenarios = corpus_scenarios() + targeted_scenarios()
    payload = {
        "reference": {
            "generatedAt": datetime.now(timezone.utc).isoformat(),
            "pdfplumberVersion": getattr(pdfplumber, "__version__", "unknown"),
            "pdfminerVersion": getattr(pdfminer, "__version__", "unknown") if pdfminer else "unknown",
            "pdfplumberCommit": git_head(),
            "source": "pdfplumber-python/tests/pdfs plus targeted upstream regression options",
        },
        "coverage": {
            "pdfCount": len(list(PDF_DIR.glob("*.pdf"))),
            "scenarioCount": len(scenarios),
            "corpusScenarioCount": len(list(PDF_DIR.glob("*.pdf"))),
        },
        "scenarios": scenarios,
    }
    OUT.write_text(json.dumps(payload, indent=2, ensure_ascii=False, sort_keys=True) + "\n", encoding="utf-8")
    print(f"Wrote {OUT.relative_to(ROOT)} with {len(scenarios)} scenarios")


if __name__ == "__main__":
    main()
