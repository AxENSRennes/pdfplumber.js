"""Stage ``plumber_text``: pdfplumber's text extraction on real pages, with
option sets, and on cropped, filtered and deduplicated pages.

Exercises pdfplumber/page.py (``Page.extract_text``, ``extract_text_simple``,
``extract_words``, ``extract_text_lines``, ``search``, ``dedupe_chars``,
``get_textmap`` and its ``lru_cache``, ``tuplify_list_kwargs``, ``crop``,
``within_bbox``, ``outside_bbox``, ``filter``, ``CroppedPage``,
``FilteredPage``, ``DerivedPage``, ``test_proposed_bbox``),
pdfplumber/utils/text.py (``WordExtractor``, ``WordMap``, ``TextMap``,
``chars_to_textmap``, ``extract_text``, ``collate_line``,
``extract_text_simple``, ``dedupe_chars``), pdfplumber/utils/geometry.py
(``crop_to_bbox``, ``within_bbox``, ``outside_bbox``, ``clip_obj``...) and,
below them, everything ``page.chars`` needs (``plumber_objects``).

Traversal (``open_pdf`` and ``exc`` of ``plumber_objects``)::

    pdf = open_pdf(path, password, max_pages)
    for index, page in enumerate(pdf.pages):
        page.chars
        each op of BASE_OPS + select(sha, index, VARIANT_OPS, VARIANTS_PER_PAGE)
        for steps in select(sha, index, DERIVED, DERIVED_PER_PAGE):
            derived = the page derived by steps
            each op of DERIVED_OPS on derived

``sha`` is the SHA-256 of the PDF file; ``select`` (``plumber_objects``)
picks a different slice of the variants for each file and page, so that
the option sets are covered across the corpus while each trace stays
bounded. The tables are data, shared with the Rust stage through
``oracle/codegen/plumber_ops.py`` (``crates/pdftrace/src/stages/plumber_ops.rs``).

An op is a list, run by ``run_op`` on a page:

- ``["extract_text", kwargs]``, ``["extract_text_simple", kwargs]``,
  ``["extract_words", kwargs]``, ``["extract_text_lines", kwargs]``:
  ``page.<name>(**kwargs)``;
- ``["search", pattern, kwargs]``: ``page.search(pattern, **kwargs)``;
- ``["dedupe_chars", kwargs]``: ``page.dedupe_chars(**kwargs)``, written as
  the indices in ``page.chars`` of the chars of the returned page (by
  identity: they are the same dicts).

A derivation step is a list:

- ``["crop" | "within_bbox" | "outside_bbox", [f0, f1, f2, f3], kwargs]``:
  ``page.<name>(bbox, **kwargs)`` with, for ``(x0, top, x1, bottom) =
  page.bbox``, ``bbox = (x0 + (x1 - x0) * f0, top + (bottom - top) * f1,
  x0 + (x1 - x0) * f2, top + (bottom - top) * f3)``, or, when
  ``kwargs["relative"]`` is true, ``((x1 - x0) * f0, (bottom - top) * f1,
  (x1 - x0) * f2, (bottom - top) * f3)`` (Python's arithmetic on the box's
  numbers; a tuple);
- ``["filter", name]``: ``page.filter(PREDICATES[name])``, the predicates
  being functions the Rust stage defines alike;
- ``["dedupe_chars", kwargs]``: ``page.dedupe_chars(**kwargs)``.

Search: the port does not run Python's ``re`` (``docs/architecture.md``,
"JavaScript Boundary"): ``TextMap.search`` takes the matches. Before the
result of a ``search`` op, the ``spans`` line gives the matches of Python's
``re`` on the page's text map (``re.finditer`` of ``re.compile(pattern if
regex else re.escape(pattern), re.I if case is False else 0)`` on
``page.get_textmap(**tuplify_list_kwargs(textmap_kwargs)).as_string``, each
match the spans of its groups, ``None`` for a group that did not take part;
``None`` when the text map raises), computed before the call (``get_textmap``
caches the text map, which the call then reuses). ``oracle.verify`` hands
the trace to ``pdftrace`` (``ORACLE_INPUT``), which reads these spans and
writes the same line.

Lines::

    open <exc>                       pdfplumber.open raised (nothing else follows)
    pages <exc>                      pdf.pages raised
    page <page_number>
    chars <exc>                      page.chars raised (the page ends there)
    op <i> <op>                      i counts the ops of the page, derived pages included
    spans <i> <spans>                search ops only
    out <i> <value>                  the result, or the exception (``exc``)
    outd <i> <length> <digest>       in place of ``out`` when ``canon`` of the
                                     result is longer than OUT_LIMIT characters
    derived <steps>                  then the ops of DERIVED_OPS on that page
    derive <exc>                     a step raised (no ops follow)
    !end
"""

from __future__ import annotations

import hashlib
import re
from collections.abc import Iterator
from typing import Any

from pdfplumber.page import Page, tuplify_list_kwargs

from ..canon import canon
from ._common import terminated
from .plumber_objects import exc, file_sha256, open_pdf, select

PAGED = True
DEPENDS = ("plumber_objects",)
#: ``oracle.verify`` gives ``pdftrace`` the oracle's trace (the ``spans`` lines).
ORACLE_INPUT = True

#: Longer results are written as their length and digest.
OUT_LIMIT = 1 << 16

#: The ``page.search`` arguments that are not text map arguments.
SEARCH_ARGS = ("regex", "case", "main_group", "return_chars", "return_groups")

BASE_OPS: list[list[Any]] = [
    ["extract_text", {}],
    ["extract_text", {"layout": True}],
    ["extract_text_simple", {}],
    ["extract_words", {}],
    ["extract_text_lines", {"return_chars": False}],
    ["search", "the", {"regex": False, "case": False, "return_chars": False}],
    ["dedupe_chars", {}],
]

VARIANT_OPS: list[list[Any]] = [
    ["extract_text", {"x_tolerance": 1.5, "y_tolerance": 1}],
    ["extract_text", {"x_tolerance": 0, "y_tolerance": 0}],
    ["extract_text", {"y_tolerance": 10}],
    ["extract_text", {"keep_blank_chars": True}],
    ["extract_text", {"use_text_flow": True}],
    ["extract_text", {"extra_attrs": ["fontname", "size"]}],
    ["extract_text", {"split_at_punctuation": True}],
    ["extract_text", {"split_at_punctuation": "!?.,"}],
    ["extract_text", {"expand_ligatures": False}],
    ["extract_text", {"x_tolerance_ratio": 0.15}],
    ["extract_text", {"layout": True, "x_density": 5, "y_density": 10.5}],
    ["extract_text", {"layout": True, "layout_width_chars": 80, "layout_height_chars": 30}],
    ["extract_text", {"layout": True, "use_text_flow": True, "keep_blank_chars": True}],
    ["extract_text", {"layout": True, "x_shift": 3, "y_shift": -2}],
    ["extract_text", {"line_dir": "btt", "char_dir": "rtl"}],
    ["extract_text", {"line_dir_render": "rtl", "char_dir_render": "btt"}],
    ["extract_text", {"layout": True, "line_dir": "ltr", "char_dir": "ttb"}],
    ["extract_text", {"line_dir_rotated": "ttb", "char_dir_rotated": "rtl"}],
    ["extract_text", {"layout": True, "layout_bbox": [0, 0, 300, 300]}],
    ["extract_text", {"line_dir": "up"}],
    ["extract_text", {"x_tolerance": "1"}],
    ["extract_text", {"extra_attrs": {"fontname": 1}}],
    ["extract_text_simple", {"x_tolerance": 1, "y_tolerance": 0}],
    ["extract_text_simple", {"x_tolerance": 5.5, "y_tolerance": 8}],
    ["extract_words", {"keep_blank_chars": True}],
    ["extract_words", {"use_text_flow": True}],
    ["extract_words", {"extra_attrs": ["fontname", "size"]}],
    ["extract_words", {"extra_attrs": ["non_stroking_color", "mcid"]}],
    ["extract_words", {"split_at_punctuation": True}],
    ["extract_words", {"expand_ligatures": False}],
    ["extract_words", {"return_chars": True}],
    ["extract_words", {"x_tolerance": 1, "y_tolerance": 1}],
    ["extract_words", {"x_tolerance_ratio": 0.3, "y_tolerance_ratio": 0.5}],
    ["extract_words", {"horizontal_ltr": False}],
    ["extract_words", {"vertical_ttb": False}],
    ["extract_words", {"line_dir": "rtl", "char_dir": "btt"}],
    ["extract_words", {"line_dir_rotated": "ltr"}],
    ["extract_words", {"extra_attrs": ["nonexistent"]}],
    ["extract_text_lines", {"strip": False, "return_chars": False}],
    ["extract_text_lines", {"layout": True, "return_chars": False}],
    ["extract_text_lines", {}],
    ["extract_text_lines", {"x_tolerance": 1, "return_chars": False}],
    ["search", r"\d+", {}],
    ["search", r"(\w)(\w)", {"main_group": 1, "return_chars": False}],
    ["search", "e", {"regex": False, "return_groups": False}],
    ["search", r"[A-Z]\w*", {"case": False, "return_chars": False}],
    ["search", r"\s+", {}],
    ["search", r"x*", {}],
    ["search", r"(a)|(b)", {"return_chars": False}],
    ["search", r"\w+", {"layout": True, "return_chars": False}],
    ["search", r"(\d+)", {"main_group": 2}],
    ["search", ".", {"regex": False, "case": False}],
    ["dedupe_chars", {"tolerance": 2}],
    ["dedupe_chars", {"extra_attrs": None}],
    ["dedupe_chars", {"tolerance": 0, "extra_attrs": ["fontname", "size", "non_stroking_color"]}],
    ["dedupe_chars", {"extra_attrs": ["nonexistent"]}],
]

DERIVED: list[list[list[Any]]] = [
    [["crop", [0.1, 0.1, 0.9, 0.6], {}]],
    [["crop", [0.0, 0.0, 0.5, 1.0], {"relative": True}]],
    [["crop", [0.0, 0.5, 1.0, 1.0], {"relative": True, "strict": False}]],
    [["within_bbox", [0.2, 0.2, 0.8, 0.8], {}]],
    [["outside_bbox", [0.25, 0.25, 0.75, 0.75], {}]],
    [["crop", [-0.1, -0.1, 0.5, 0.5], {"strict": False}]],
    [["crop", [-0.1, -0.1, 0.5, 0.5], {}]],
    [["crop", [0.5, 0.5, 0.5, 0.9], {}]],
    [["within_bbox", [0.5, 0.0, 1.5, 0.5], {"strict": False}]],
    [["filter", "upright"]],
    [["filter", "not-rect"]],
    [["filter", "small"]],
    [["dedupe_chars", {"tolerance": 1}]],
    [["crop", [0.05, 0.05, 0.95, 0.95], {}], ["crop", [0.1, 0.1, 0.9, 0.9], {}]],
    [["crop", [0.0, 0.0, 1.0, 0.5], {}], ["within_bbox", [0.0, 0.0, 0.5, 1.0], {}]],
    [["filter", "upright"], ["crop", [0.0, 0.25, 1.0, 0.75], {}]],
    [["dedupe_chars", {}], ["outside_bbox", [0.0, 0.0, 0.3, 0.3], {"relative": True}]],
]

DERIVED_OPS: list[list[Any]] = [
    ["extract_text", {}],
    ["extract_text", {"layout": True}],
    ["extract_words", {}],
    ["search", r"\w+", {"return_chars": False}],
]

VARIANTS_PER_PAGE = 8
DERIVED_PER_PAGE = 2


def _upright(obj: dict[str, Any]) -> Any:
    return obj.get("upright", True)


def _not_rect(obj: dict[str, Any]) -> Any:
    return obj["object_type"] != "rect"


def _small(obj: dict[str, Any]) -> Any:
    return obj.get("size", 0) < 10


#: The predicates of ``filter`` steps.
PREDICATES = {"upright": _upright, "not-rect": _not_rect, "small": _small}


def derive_bbox(page: Page, fractions: list[float], relative: Any) -> tuple[Any, Any, Any, Any]:
    x0, top, x1, bottom = page.bbox
    w, h = x1 - x0, bottom - top
    f0, f1, f2, f3 = fractions
    if relative:
        return (w * f0, h * f1, w * f2, h * f3)
    return (x0 + w * f0, top + h * f1, x0 + w * f2, top + h * f3)


def derive(page: Page, steps: list[list[Any]]) -> Page:
    """The page derived from ``page`` by ``steps``."""
    for step in steps:
        name = step[0]
        if name in ("crop", "within_bbox", "outside_bbox"):
            kwargs = step[2]
            page = getattr(page, name)(derive_bbox(page, step[1], kwargs.get("relative")), **kwargs)
        elif name == "filter":
            page = page.filter(PREDICATES[step[1]])
        elif name == "dedupe_chars":
            page = page.dedupe_chars(**step[1])
        else:  # pragma: no cover
            raise AssertionError(step)
    return page


def search_spans(page: Page, pattern: str, kwargs: dict[str, Any]) -> Any:
    """The matches of Python's ``re`` for ``page.search(pattern, **kwargs)``
    (see the module docstring)."""
    regex = kwargs.get("regex", True)
    case = kwargs.get("case", True)
    textmap_kwargs = {k: v for k, v in kwargs.items() if k not in SEARCH_ARGS}
    try:
        text = page.get_textmap(**tuplify_list_kwargs(textmap_kwargs)).as_string
    except Exception:
        return None
    compiled = re.compile(pattern if regex else re.escape(pattern), re.I if case is False else 0)
    return [
        [(m.start(g), m.end(g)) if m.start(g) >= 0 else None for g in range(compiled.groups + 1)]
        for m in re.finditer(compiled, text)
    ]


def run_op(page: Page, op: list[Any]) -> Any:
    name = op[0]
    if name == "search":
        return page.search(op[1], **op[2])
    if name == "dedupe_chars":
        deduped = page.dedupe_chars(**op[1])
        index = {id(c): i for i, c in enumerate(page.chars)}
        return [index[id(c)] for c in deduped.chars]
    return getattr(page, name)(**op[1])


def out_line(i: int, value: Any) -> str:
    text = canon(value)
    if len(text) > OUT_LIMIT:
        return f"outd {i} {len(text)} " + hashlib.sha256(text.encode("ascii")).hexdigest()[:16]
    return f"out {i} {text}"


def ops_lines(page: Page, ops: list[list[Any]], counter: list[int]) -> Iterator[str]:
    for op in ops:
        i = counter[0]
        counter[0] += 1
        yield f"op {i} " + canon(op)
        if op[0] == "search":
            yield f"spans {i} " + canon(search_spans(page, op[1], op[2]))
        try:
            value = run_op(page, op)
        except Exception as e:
            yield f"out {i} " + exc(e)
        else:
            yield out_line(i, value)


@terminated
def run(path: str, password: str | None, max_pages: int) -> Iterator[str]:
    sha = file_sha256(path)
    try:
        pdf = open_pdf(path, password, max_pages)
    except Exception as e:
        yield "open " + exc(e)
        return
    try:
        pages = pdf.pages
    except Exception as e:
        yield "pages " + exc(e)
        return
    for index, page in enumerate(pages):
        yield f"page {page.page_number}"
        try:
            page.chars
        except Exception as e:
            yield "chars " + exc(e)
            continue
        counter = [0]
        yield from ops_lines(page, BASE_OPS + select(sha, index, VARIANT_OPS, VARIANTS_PER_PAGE), counter)
        for steps in select(sha, index, DERIVED, DERIVED_PER_PAGE):
            yield "derived " + canon(steps)
            try:
                derived = derive(page, steps)
            except Exception as e:
                yield "derive " + exc(e)
                continue
            yield from ops_lines(derived, DERIVED_OPS, counter)


__all__ = [
    "BASE_OPS",
    "DERIVED",
    "DERIVED_OPS",
    "OUT_LIMIT",
    "PREDICATES",
    "VARIANT_OPS",
    "derive",
    "derive_bbox",
    "out_line",
]
