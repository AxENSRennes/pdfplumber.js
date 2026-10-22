"""Sessions of the public JavaScript API (``docs/public-api.md``): the spec
the JavaScript replayer (``js/test/shared/api-sessions.mjs``) checks the
API against, in Node, in the browsers (in the page and in worker mode) and
on the dist build.

A session is a sequence of calls of the public API, written in a neutral
JSON form. This generator turns each call into the statements of
``oracle/unit/js_pages_run.py`` the API runs for it (``docs/public-api.md``,
"Operations": every member, its boundary operation, its statement), runs
the statements of the session in a fresh Python process (a session, a new
core: a new Python process), and derives from their results what each call
must give: a value, a Python exception (with the exceptions it wraps,
``cause``), or an error of the API itself (``DocumentReleased``). The
expected values are Python's; nothing here is written by hand but the
calls.

Writes ``<cache>/unit/js_api/<fingerprint>/sessions.jsonl``
(``oracle/unit/_common.py``), one session per line::

    {"in": [session], "out": [expectation, ...]}

    session     = {"name": str, "node_only": bool, "steps": [step, ...]}
    step        = {"calls": [call, ...], "await": [k, ...]}
                  the calls are made in this order without waiting, then
                  their promises awaited in the order of "await" (indices
                  in "calls"); a value that is not a promise is awaited too
    call        = {"id": int, "on": ref or null, "get" | "call": member,
                   "args": [typed JSON, ...],
                   "input": {"bytes_of": rel} | {"bytes": hex} | {"path": rel}
                                                         (open only)
                   "predicate": name                     (filter only)}
    ref         = [call id, index, ...]: the result of that call, then items
    expectation = {"id": int, "value": typed JSON}

``bytes_of`` opens the bytes of a file of ``PDFPLUMBER_ROOT``
(``pdfplumber.open(io.BytesIO(...))``); ``path`` opens the path
(``pdfplumber.open(path)``: ``close()`` closes the file), which only Node
does (``node_only`` sessions). ``search`` takes its pattern as a string or
``{"regexp": source, "flags": flags}`` (a ``RegExp``); ``filter`` takes the
predicate named ``predicate`` (``js_pages_run.PREDICATES``, written alike
by the replayer).

The values are typed JSON; an object of the API in a value is a ``Named``
value: a page ``PDFPlumberPage{page_number, initial_doctop, rotation,
mediabox, cropbox, [trimbox, bleedbox, artbox], bbox, width, height}``
(``width`` and ``height`` an ``Error`` when they throw), a table
``Table{bbox, cells, rows, columns}``, a document ``PDFPlumberDocument{}``,
a ``Page`` held in a value ``Page{page_number}``, an error
``Error{name, message, cause}`` (``message`` ``None``: any; ``cause`` the
wrapped exception's ``Error``, or ``None``). A call on an object that was
not made (the call that makes it raised, or made fewer) is
``Unresolved{}``: neither side runs it.

Usage: PDFPLUMBER_ROOT=<main checkout> <root>/.venv/bin/python -m oracle.unit.js_api
"""

from __future__ import annotations

import json
import re
import subprocess
import sys
import tempfile
from collections.abc import Callable
from pathlib import Path
from typing import Any

from pdfminer.pdftypes import PDFStream

from .. import typed_json
from ..canon import ExcValue, Named
from ..corpus import cache_dir, corpus_root
from ..stages.plumber_objects import VALID_DIRS_REPR
from . import js_pages_run
from ._common import require_oracle_python, unit_dir, write_meta

MODULE = "js_api"

#: Alias -> the member it is (the API has one implementation of both).
ALIASES = {
    "rectEdges": "rect_edges",
    "curveEdges": "curve_edges",
    "horizontalEdges": "horizontal_edges",
    "verticalEdges": "vertical_edges",
    "structureTree": "structure_tree",
    "pageNumber": "page_number",
    "initialDoctop": "initial_doctop",
    "to_dict": "toDict",
    "to_json": "toJson",
    "to_csv": "toCsv",
    "extract_text": "extractText",
    "extract_text_simple": "extractTextSimple",
    "extract_words": "extractWords",
    "extract_text_lines": "extractTextLines",
    "within_bbox": "withinBbox",
    "outside_bbox": "outsideBbox",
    "dedupe_chars": "dedupeChars",
    "find_tables": "findTables",
    "find_table": "findTable",
    "extract_tables": "extractTables",
    "extract_table": "extractTable",
}

#: The object lists read with ``getattr(container, name)`` (``doc_attr``,
#: ``page_attr``).
LISTS = ["chars", "rects", "lines", "curves", "images", "edges", "rect_edges", "curve_edges", "horizontal_edges",
         "vertical_edges", "textboxhorizontals", "textboxverticals", "textlinehorizontals", "textlineverticals"]

#: The geometry of a page, in this order.
GEOMETRY = ["page_number", "initial_doctop", "rotation", "mediabox", "cropbox", "trimbox", "bleedbox", "artbox", "bbox",
            "width", "height"]

UNRESOLVED = Named("Unresolved", {})


#: ``str(valid_dirs)`` in the messages of
#: pdfplumber/utils/text.py:78 validate_directions: a ``set``, whose order changes with
#: ``PYTHONHASHSEED``; written in the port's order, as the ``plumber_*``
#: stages write it (``plumber_objects.VALID_DIRS_REPR``)
_DIRS = re.compile(r"\{'(?:ttb|btt|ltr|rtl)', '(?:ttb|btt|ltr|rtl)', '(?:ttb|btt|ltr|rtl)', '(?:ttb|btt|ltr|rtl)'\}")


def _message(m: str) -> str:
    return _DIRS.sub(VALID_DIRS_REPR, m)


def error(e: Any, causes: list[Any] | None = None) -> Named:
    """``Error{name, message, cause}`` of an exception of a result."""
    out = None
    for c in reversed(list(causes or [])):
        out = Named("Error", {"name": c.name, "message": _message(c.message), "cause": out})
    return Named("Error", {"name": e.name, "message": _message(e.message), "cause": out})


def inline_names(v: Any, inline: Callable[[dict[str, Any]], bool]) -> Any:
    """``v`` with the name of each inline image (an image dict for which
    ``inline`` holds) written ``"<inline k>"``, ``k`` numbering the
    distinct names of a page (``page_number``) in the order they appear:
    pdfminer names an inline image ``str(id(stream))``, the port
    ``inline-<k>`` (``docs/public-api.md``, "Not Ported"). The replayer
    writes the API's names alike."""
    seen: dict[Any, dict[str, int]] = {}

    def go(x: Any) -> Any:
        if type(x) is dict:
            if x.get("object_type") == "image" and type(x.get("name")) is str and inline(x):
                names = seen.setdefault(x.get("page_number"), {})
                k = names.setdefault(x["name"], len(names))
                return {key: (f"<inline {k}>" if key == "name" else go(i)) for key, i in x.items()}
            return {key: go(i) for key, i in x.items()}
        if type(x) is list:
            return [go(i) for i in x]
        if type(x) is tuple:
            return tuple(go(i) for i in x)
        return x

    return go(v)


def _python_inline(o: dict[str, Any]) -> bool:
    s = o.get("stream")
    return isinstance(s, PDFStream) and s.objid is None and o["name"].isdigit()


def js_error(name: str) -> Named:
    """An error of the API itself (no Python counterpart)."""
    return Named("Error", {"name": name, "message": None, "cause": None})


def neutral(v: Any) -> Any:
    """A result value as the API gives it: a ``Page`` held in it by its
    ``page_number`` (its handle is the core's), inline image names as
    ``inline_names`` writes them."""

    def go(x: Any) -> Any:
        if isinstance(x, Named) and x.name == "Page":
            return Named("Page", {"page_number": x.fields["page_number"]})
        if type(x) is list:
            return [go(i) for i in x]
        if type(x) is tuple:
            return tuple(go(i) for i in x)
        if type(x) is dict:
            return {k: go(i) for k, i in x.items()}
        return x

    return inline_names(go(v), _python_inline)


def page_geometry(g: dict[str, Any]) -> dict[str, Any]:
    """The geometry of a page value (``doc_pages``, ``page_info``) in the
    order of ``GEOMETRY``, without its handle."""
    return {k: g[k] for k in GEOMETRY if k in g}


def table_value(t: dict[str, Any]) -> Named:
    return Named("Table", {k: t[k] for k in ("bbox", "cells", "rows", "columns")})


class Results:
    """The results of a session's statements."""

    def __init__(self, results: list[Any], causes: list[Any]) -> None:
        self.results = results
        self.causes = causes

    def value(self, case: int) -> Any:
        return self.results[case][0]

    def unresolved(self, case: int) -> bool:
        v = self.value(case)
        return type(v) is dict and v.get("$unresolved") is True

    def failed(self, case: int) -> bool:
        return isinstance(self.value(case), ExcValue)

    def error(self, case: int) -> Named:
        return error(self.value(case), self.causes[case])

    def plain(self, case: int) -> Any:
        """The value of a call made of one statement."""
        if self.unresolved(case):
            return UNRESOLVED
        return self.error(case) if self.failed(case) else neutral(self.value(case))


class Doc:
    def __init__(self) -> None:
        self.pages: list[Obj] | None = None
        self.pages_value: Callable[[Results], Any] | None = None
        self.released = False


class Obj:
    """An object of the API in a session: its JavaScript reference, its
    target in ``js_pages_run.py``, and whether it was made (``alive``)."""

    def __init__(self, kind: str, ref: list[Any], target: Any, doc: Doc, alive: Callable[[Results], bool]) -> None:
        self.kind = kind
        self.ref = ref
        self.target = target
        self.doc = doc
        self.alive = alive
        #: a page: its geometry from the results (once alive)
        self.geometry: Callable[[Results], dict[str, Any]] | None = None


class Session:
    """A session being written: its calls (neutral form), the statements
    they run, and how each call's expectation follows from their results."""

    def __init__(self, name: str, node_only: bool = False) -> None:
        self.name = name
        self.node_only = node_only
        self.steps: list[dict[str, Any]] = []
        self.cases: list[list[Any]] = []
        self.expect: list[tuple[int, Callable[[Results], Any]]] = []
        self.next_id = 0
        self.batch: list[dict[str, Any]] | None = None

    # -------------------------------------------------------------- plumbing

    def case(self, op: str, target: Any, args: list[Any], data: Any = None, js: Any = None) -> int:
        self.cases.append([op, target, args, data, js])
        return len(self.cases) - 1

    def _call(self, on: Obj | None, kind: str, name: str, args: list[Any], **extra: Any) -> int:
        cid = self.next_id
        self.next_id += 1
        call: dict[str, Any] = {"id": cid, "on": None if on is None else on.ref, kind: name,
                                "args": [typed_json.encode(a) for a in args]}
        call.update(extra)
        if self.batch is not None:
            self.batch.append(call)
        else:
            self.steps.append({"calls": [call], "await": [0]})
        return cid

    def _expect(self, cid: int, on: Obj | None, f: Callable[[Results], Any]) -> None:
        """The expectation of call ``cid``: ``Unresolved`` when the object
        it is made on was not made."""
        if on is None:
            self.expect.append((cid, f))
        else:
            self.expect.append((cid, lambda r: f(r) if on.alive(r) else UNRESOLVED))

    def _async(self, obj: Obj, kind: str, name: str, args: list[Any], **extra: Any) -> tuple[int, bool]:
        """An asynchronous member: the call, and whether its document was
        released (``DocumentReleased``: no statement)."""
        cid = self._call(obj, kind, name, args, **extra)
        if obj.doc.released:
            self._expect(cid, obj, lambda r: js_error("DocumentReleased"))
            return cid, True
        return cid, False

    def concurrently(self, order: list[int] | None = None) -> "_Batch":
        """``with s.concurrently(order):`` the calls made inside are made
        without waiting, then awaited in ``order`` (reversed by default)."""
        return _Batch(self, order)

    # ------------------------------------------------------------ the module

    def open(self, rel: str | None = None, options: dict[str, Any] | None = None, *, data: bytes | None = None,
             by_path: bool = False) -> Obj:
        """``open(input, options)``: ``doc_open`` (bytes), or
        ``doc_open_file`` (a path, Node: ``node_only`` sessions). Every
        option but ``logLevel`` and ``onWarning`` is a keyword argument of
        ``pdfplumber.open``."""
        options = dict(options or {})
        if by_path:
            assert self.node_only and rel is not None
            inp: dict[str, Any] = {"path": rel}
            c = self.case("doc_open_file", None, [rel, options], rel)
        elif rel is not None:
            inp = {"bytes_of": rel}
            c = self.case("doc_open", None, [options], rel)
        else:
            assert data is not None
            inp = {"bytes": data.hex()}
            c = self.case("doc_open", None, [options], data)
        cid = self._call(None, "call", "open", [options] if options else [], input=inp)
        obj = Obj("document", [cid], [c, 0], Doc(), lambda r: not r.failed(c))
        self._expect(cid, None, lambda r: r.error(c) if r.failed(c) else Named("PDFPlumberDocument", {}))
        return obj

    # ---------------------------------------------------------- the document

    def pages(self, doc: Obj) -> list[Obj]:
        """``await pdf.pages``: ``doc_pages`` at the first read (and after
        ``close()``), the same array then (the sessions read again only
        pages that loaded: a rejected read is not kept by the API)."""
        d = doc.doc
        cid, released = self._async(doc, "get", "pages", [])
        if released:
            d.pages = None
            return []
        if d.pages is not None:
            self._expect(cid, doc, d.pages_value)  # type: ignore[arg-type]
            return d.pages
        c = self.case("doc_pages", doc.target, [])

        def value(r: Results) -> Any:
            if r.unresolved(c):
                return UNRESOLVED
            if r.failed(c):
                return r.error(c)
            return [Named("PDFPlumberPage", page_geometry(g)) for g in r.value(c)]

        pages = []
        # the sessions read the first pages (the count is the result's)
        for i in range(4):
            p = Obj("page", [cid, i], [c, i], d, lambda r, i=i: not r.unresolved(c) and not r.failed(c) and i < len(r.value(c)))
            p.geometry = lambda r, i=i: page_geometry(r.value(c)[i])
            pages.append(p)
        d.pages = pages
        d.pages_value = value
        self._expect(cid, doc, value)
        return pages

    def doc_get(self, doc: Obj, name: str) -> None:
        """A getter of the document (a promise)."""
        cid, released = self._async(doc, "get", name, [])
        if released:
            return
        member = ALIASES.get(name, name)
        if member in LISTS:
            c = self.case("doc_attr", doc.target, [member])
        else:
            op = {"metadata": "doc_metadata", "objects": "doc_objects", "annots": "doc_annots",
                  "hyperlinks": "doc_hyperlinks", "structure_tree": "doc_structure_tree"}[member]
            c = self.case(op, doc.target, [])
        self._expect(cid, doc, lambda r: r.plain(c))

    def serialize(self, obj: Obj, name: str, *args: Any) -> None:
        """``toDict(objectTypes)``, ``toJson(options)``, ``toCsv(options)``
        of a document or a page."""
        cid, released = self._async(obj, "call", name, list(args))
        if released:
            return
        member = ALIASES.get(name, name)
        prefix = "doc" if obj.kind == "document" else "page"
        if member == "toDict":
            c = self.case(f"{prefix}_to_dict", obj.target, [{} if not args else {"object_types": args[0]}])
        else:
            op = {"toJson": "to_json", "toCsv": "to_csv"}[member]
            c = self.case(f"{prefix}_{op}", obj.target, [args[0] if args else {}])
        self._expect(cid, obj, lambda r: r.plain(c))

    def close(self, obj: Obj) -> None:
        """``pdf.close()`` (``PDF.close()``: the pages are made again at the
        next read) or ``page.close()``."""
        cid, released = self._async(obj, "call", "close", [])
        if released:
            return
        c = self.case("doc_close" if obj.kind == "document" else "page_close", obj.target, [])
        if obj.kind == "document":
            obj.doc.pages = None
        self._expect(cid, obj, lambda r: r.plain(c))

    def release(self, doc: Obj) -> None:
        """``pdf.release()``: the document leaves the core (no statement)."""
        cid = self._call(doc, "call", "release", [])
        doc.doc.released = True
        doc.doc.pages = None
        self._expect(cid, doc, lambda r: None)

    # --------------------------------------------------------------- the page

    def page_get(self, page: Obj, name: str) -> None:
        """A getter of a page: the geometry (synchronous, known since the
        page was made) or an object list (a promise)."""
        member = ALIASES.get(name, name)
        if member in GEOMETRY:
            cid = self._call(page, "get", name, [])
            geometry = page.geometry
            assert geometry is not None
            self._expect(cid, page, lambda r: geometry(r).get(member))
            return
        cid, released = self._async(page, "get", name, [])
        if released:
            return
        if member in LISTS or member in ("annots", "hyperlinks"):
            c = self.case("page_attr", page.target, [member])
        else:
            c = self.case({"objects": "page_objects", "structure_tree": "page_structure_tree"}[member], page.target, [])
        self._expect(cid, page, lambda r: r.plain(c))

    def to_string(self, page: Obj) -> None:
        """``String(page)``: ``repr(page)``."""
        cid = self._call(page, "call", "toString", [])
        geometry = page.geometry
        assert geometry is not None
        self._expect(cid, page, lambda r: f"<Page:{geometry(r)['page_number']}>")

    def text(self, page: Obj, name: str, options: dict[str, Any] | None = None) -> None:
        """``extractText``, ``extractTextSimple``, ``extractWords``,
        ``extractTextLines`` (``strip`` and ``return_chars`` positional)."""
        cid, released = self._async(page, "call", name, [] if options is None else [options])
        if released:
            return
        member = ALIASES.get(name, name)
        kw = dict(options or {})
        if member == "extractTextLines":
            strip = kw.pop("strip", True)
            chars = kw.pop("return_chars", True)
            c = self.case("page_extract_text_lines", page.target, [strip, chars, kw])
        else:
            op = {"extractText": "page_extract_text", "extractTextSimple": "page_extract_text_simple",
                  "extractWords": "page_extract_words"}[member]
            c = self.case(op, page.target, [kw])
        self._expect(cid, page, lambda r: r.plain(c))

    def search(self, page: Obj, py_pattern: Any, js_pattern: Any, options: dict[str, Any] | None = None) -> None:
        """``search(pattern, options)``: one statement,
        ``page.search(pattern, **options)`` (``js_pattern`` matches as
        ``py_pattern`` does under Python's ``re``; ``{"$compiled": source}``
        a compiled pattern, the ``RegExp`` of the API)."""
        cid, released = self._async(page, "call", "search", [js_pattern] + ([] if options is None else [options]))
        if released:
            return
        c = self.case("search", page.target, [py_pattern, dict(options or {})], js=js_pattern)
        self._expect(cid, page, lambda r: r.plain(c))

    def _derived(self, page: Obj, cid: int, made: int, before: int | None = None) -> Obj:
        """The derived page made by case ``made``, then its ``page_info``."""
        parent = page.geometry
        assert parent is not None
        info = self.case("page_info", [made, 0], [])

        def made_ok(r: Results) -> bool:
            return all(not r.unresolved(c) and not r.failed(c) for c in (before, made) if c is not None)

        derived = Obj("page", [cid], [made, 0], page.doc, lambda r: page.alive(r) and made_ok(r))

        def geometry(r: Results) -> dict[str, Any]:
            if not r.failed(info):
                return page_geometry(r.value(info))
            # page_info raised (width or height): the derivation's geometry,
            # width and height throwing
            g = dict(parent(r))
            g["page_number"] = r.value(made)["page_number"]
            g["bbox"] = r.value(made)["bbox"]
            g["width"] = g["height"] = r.error(info)
            return {k: g[k] for k in GEOMETRY if k in g}

        derived.geometry = geometry

        def value(r: Results) -> Any:
            for c in (before, made):
                if c is not None and r.failed(c):
                    return r.error(c)
            return Named("PDFPlumberPage", geometry(r))

        self._expect(cid, page, value)
        return derived

    def derive(self, page: Obj, name: str, bbox: Any, options: dict[str, Any] | None = None) -> Obj | None:
        """``crop``, ``withinBbox``, ``outsideBbox``: the operation, then
        ``page_info`` of the page it made."""
        cid, released = self._async(page, "call", name, [bbox] + ([] if options is None else [options]))
        if released:
            return None
        op = {"crop": "page_crop", "withinBbox": "page_within_bbox", "outsideBbox": "page_outside_bbox"}[ALIASES.get(name, name)]
        made = self.case(op, page.target, [bbox, dict(options or {})])
        return self._derived(page, cid, made)

    def filter(self, page: Obj, predicate: str) -> Obj | None:
        """``filter(test)``: ``page_objects``, the mask (``page_filter``),
        ``page_info``."""
        cid, released = self._async(page, "call", "filter", [], predicate=predicate)
        if released:
            return None
        objects = self.case("page_objects", page.target, [])
        made = self.case("page_filter", page.target, [predicate])
        return self._derived(page, cid, made, before=objects)

    def dedupe(self, page: Obj, name: str = "dedupeChars", options: dict[str, Any] | None = None) -> Obj | None:
        cid, released = self._async(page, "call", name, [] if options is None else [options])
        if released:
            return None
        made = self.case("page_dedupe_chars", page.target, [dict(options or {})])
        return self._derived(page, cid, made)

    def tables(self, page: Obj, name: str, settings: Any = "absent") -> list[Obj]:
        """``findTables``, ``findTable``, ``extractTables``,
        ``extractTable`` (``settings`` ``"absent"``: no argument)."""
        args = [] if settings == "absent" else [settings]
        cid, released = self._async(page, "call", name, args)
        if released:
            return []
        member = ALIASES.get(name, name)
        op = {"findTables": "page_find_tables", "findTable": "page_find_table", "extractTables": "page_extract_tables",
              "extractTable": "page_extract_table"}[member]
        c = self.case(op, page.target, list(args))

        def value(r: Results) -> Any:
            if r.failed(c):
                return r.error(c)
            v = r.value(c)
            if member == "findTables":
                return [table_value(t) for t in v]
            if member == "findTable":
                return None if v is None else table_value(v)
            return neutral(v)

        self._expect(cid, page, value)

        def made(r: Results, i: int) -> bool:
            if not page.alive(r) or r.unresolved(c) or r.failed(c):
                return False
            v = r.value(c)
            return i < len(v) if member == "findTables" else v is not None

        if member == "findTables":
            return [Obj("table", [cid, i], [c, i], page.doc, lambda r, i=i: made(r, i)) for i in range(2)]
        if member == "findTable":
            return [Obj("table", [cid], [c, 0], page.doc, lambda r: made(r, 0))]
        return []

    def table_get(self, table: Obj, name: str) -> None:
        """``bbox``, ``cells``, ``rows``, ``columns`` of a table (known
        since it was found)."""
        cid = self._call(table, "get", name, [])
        ref = table.ref
        found = ref[0]
        index = ref[1] if len(ref) > 1 else None

        def value(r: Results) -> Any:
            f = next(f for c, f in self.expect if c == found)
            t = f(r)
            t = t[index] if index is not None else t
            return t.fields[name]

        self._expect(cid, table, value)

    def extract(self, table: Obj, options: dict[str, Any] | None = None) -> None:
        cid, released = self._async(table, "call", "extract", [] if options is None else [options])
        if released:
            return
        c = self.case("table_extract", table.target, [dict(options or {})])
        self._expect(cid, table, lambda r: r.plain(c))

    # ---------------------------------------------------------------- output

    def run(self) -> dict[str, Any]:
        """Runs the statements; the session and its expectations."""
        if self.batch is not None:
            raise AssertionError("a batch left open")
        r = run_cases(self.cases)
        out = [{"id": cid, "value": typed_json.encode(f(r))} for cid, f in sorted(self.expect, key=lambda e: e[0])]
        assert [e["id"] for e in out] == list(range(self.next_id)), self.name
        return {"session": {"name": self.name, "node_only": self.node_only, "steps": self.steps}, "expect": out}


class _Batch:
    def __init__(self, s: Session, order: list[int] | None) -> None:
        self.s = s
        self.order = order

    def __enter__(self) -> None:
        self.s.batch = []

    def __exit__(self, *exc: Any) -> None:
        calls = self.s.batch or []
        self.s.batch = None
        order = self.order if self.order is not None else list(reversed(range(len(calls))))
        assert sorted(order) == list(range(len(calls))), order
        self.s.steps.append({"calls": calls, "await": order})


def run_cases(cases: list[list[Any]]) -> Results:
    """The results of the statements in a fresh process at the top level of
    ``js_pages_run.py`` (paths made absolute)."""
    root = corpus_root()
    runnable = [[op, target, args, str(root / data) if isinstance(data, str) else data, js]
                for op, target, args, data, js in cases]
    with tempfile.TemporaryDirectory() as d:
        inp, out = Path(d) / "in.json", Path(d) / "out.json"
        inp.write_text(json.dumps(typed_json.encode(runnable)))
        subprocess.run([sys.executable, "-I", js_pages_run.__file__, str(inp), str(out)], check=True, timeout=1800)
        raw = json.loads(out.read_text())
    results = [[typed_json.decode(v), typed_json.decode(x)] for v, x in raw["results"]]
    causes = [[typed_json.decode(c) for c in chain] for chain in raw["causes"]]
    return Results(results, causes)


# --------------------------------------------------------------------- sessions

#: (Python pattern, JavaScript pattern, options): the same matches under
#: Python's ``re`` and JavaScript's ``RegExp`` (ASCII classes, no empty
#: match next to a non-empty one)
SEARCHES: list[tuple[str, Any, dict[str, Any] | None]] = [
    ("the", "the", None),
    ("[A-Z][a-z]+", "[A-Z][a-z]+", {"return_chars": False}),
    ("a.b", "a.b", {"regex": False}),
    ("THE", "THE", {"case": False, "return_groups": False}),
    (r"([0-9]+)[.,]([0-9]+)", r"([0-9]+)[.,]([0-9]+)", {"main_group": 2, "return_chars": False}),
    ("(?P<w>e)(x)?", "(?<w>e)(x)?", {"main_group": "w", "layout": True}),
    ("(?i)[a-z]+", {"regexp": "[a-z]+", "flags": "i"}, {"return_chars": False}),
    ("e", "e", {"main_group": 3}),
]

TEXT_OPTIONS: list[dict[str, Any] | None] = [None, {"layout": True}, {"x_tolerance": 1.5, "y_tolerance": 1},
                                             {"keep_blank_chars": True, "use_text_flow": True}, {"x_tolerance": "1"}]
WORD_OPTIONS: list[dict[str, Any] | None] = [None, {"extra_attrs": ["fontname", "size"]}, {"return_chars": True},
                                             {"split_at_punctuation": True}, {"line_dir": "up"}]
LINE_OPTIONS: list[dict[str, Any] | None] = [None, {"strip": False, "return_chars": False}, {"layout": True, "return_chars": False}]
TABLE_SETTINGS: list[Any] = ["absent", None, {"vertical_strategy": "text", "horizontal_strategy": "text"},
                             {"snap_tolerance": 1}, {"vertical_strategy": "nope"}, {"no_such_setting": 1}]

#: Files of the real corpus (``PDFPLUMBER_ROOT``-relative): every member on
#: the document and its first page.
FILES = [
    "corpus/micro/text-operators.pdf",
    "corpus/micro/vector-objects.pdf",
    "corpus/micro/annotations.pdf",
    "corpus/micro/annotations-extended.pdf",
    "corpus/micro/vertical-rtl-text.pdf",
    "upstream/pdfplumber-python/tests/pdfs/pdffill-demo.pdf",
    "upstream/pdfplumber-python/tests/pdfs/nics-background-checks-2015-11.pdf",
    "upstream/pdfplumber-python/tests/pdfs/scotus-transcript-p1.pdf",
    "upstream/pdfplumber-python/tests/pdfs/issue-71-duplicate-chars.pdf",
    "upstream/pdfplumber-python/tests/pdfs/table-curves-example.pdf",
]

#: CJK documents (lazy CMaps).
CJK = ["upstream/pdfjs/test/pdfs/noembed-sjis.pdf", "upstream/pdfplumber-python/tests/pdfs/pr-136-example.pdf"]

#: Adversarial PDFs of ``adv-plumber`` (embedded): boxes, rotations,
#: degenerate geometry, images, marked content, tables, annotations, and
#: depth windows (where Python's ``RecursionError`` happens through the
#: API's statements).
ADV = ["000-box-media-origin.pdf", "007-box-media-bools.pdf", "015-box-media-bad-string.pdf", "027-box-crop-inverted.pdf",
       "051-box-all.pdf", "055-box-rotate-3.pdf", "073-geom-degenerate.pdf", "074-geom-infinite.pdf",
       "078-image-kinds.pdf", "079-image-inline-many.pdf", "080-mc-mcid-kinds.pdf", "085-text-dedupe.pdf",
       "091-table-grid-lines.pdf", "097-table-nested.pdf", "100-annot-malformed.pdf",
       "105-depth-figures-324.pdf", "106-depth-figures-325.pdf", "107-depth-figures-326.pdf",
       "122-depth-mcid-list-491.pdf", "123-depth-mcid-list-492.pdf", "124-depth-mcid-list-493.pdf",
       "131-climit-tf-list-9984.pdf", "139-climit-tf-list-9992.pdf", "143-climit-tf-list-9996.pdf",
       "146-climit-tf-list-9999.pdf", "147-climit-tf-list-10000.pdf"]


def pick(seq: list[Any], k: int, n: int) -> list[Any]:
    """``n`` entries of ``seq`` from the ``k``-th, cyclically (each file
    takes another slice)."""
    return [seq[(k + j) % len(seq)] for j in range(min(n, len(seq)))]


def alias(k: int, member: str) -> str:
    """The member's name or its alias, alternately."""
    other = next((a for a, m in ALIASES.items() if m == member), None)
    if other is None:
        other = next((m for a, m in ALIASES.items() if a == member), None)
    return other if other is not None and k % 2 else member


def file_session(k: int, rel: str) -> Session:
    """Every member of the document and its first page, on one file."""
    s = Session(f"file-{Path(rel).stem}")
    doc = s.open(rel)
    s.doc_get(doc, "metadata")
    page = s.pages(doc)[0]
    for name in GEOMETRY + ["pageNumber", "initialDoctop"]:
        s.page_get(page, name)
    s.to_string(page)
    s.page_get(page, "objects")
    for name in pick(LISTS + ["annots", "hyperlinks", "rectEdges", "curveEdges", "horizontalEdges", "verticalEdges"], k, 8):
        s.page_get(page, name)
    s.page_get(page, alias(k, "structure_tree"))
    for options in pick(TEXT_OPTIONS, k, 3):
        s.text(page, alias(k, "extractText"), options)
    s.text(page, alias(k + 1, "extractTextSimple"), None)
    for options in pick(WORD_OPTIONS, k, 3):
        s.text(page, alias(k, "extractWords"), options)
    for options in pick(LINE_OPTIONS, k, 2):
        s.text(page, alias(k + 1, "extractTextLines"), options)
    for py, js, options in pick(SEARCHES, k, 4):
        s.search(page, py, js, options)
    # a RegExp with regex=False or case=False: pdfplumber's ValueError
    s.search(page, {"$compiled": "e"}, {"regexp": "e", "flags": ""}, pick([{"regex": False}, {"case": False}], k, 1)[0])
    s.search(page, {"$compiled": "[a-z]"}, {"regexp": "[a-z]", "flags": ""}, {"return_chars": False})
    # derived pages: each kind, a chain, the getters of a derived page
    crop = s.derive(page, "crop", [10, 10, 200, 200])
    if crop is not None:
        s.page_get(crop, "bbox")
        s.page_get(crop, "chars")
        s.text(crop, "extractText", None)
        chained = s.derive(crop, "crop", [5, 5, 50, 50], {"relative": True, "strict": False})
        if chained is not None:
            s.page_get(chained, "width")
            s.text(chained, "extractWords", None)
    within = s.derive(page, alias(k, "withinBbox"), (0, 0, 300.5, 300), {"relative": True})
    if within is not None:
        s.page_get(within, "objects")
    outside = s.derive(page, alias(k + 1, "outsideBbox"), [0, 0, 50, 50], {"strict": False})
    if outside is not None:
        s.text(outside, "extractText", None)
    s.derive(page, "crop", [0, 0, 0, 0])
    s.derive(page, "crop", [0, 0, 10, 10], {"bogus": 1})
    odd = s.derive(page, "crop", ["a", 0, 10, 10], {"strict": False})
    if odd is not None:
        s.page_get(odd, "width")
        s.page_get(odd, "height")
    for predicate in pick(["upright", "not-rect", "small"], k, 2):
        filtered = s.filter(page, predicate)
        if filtered is not None:
            s.page_get(filtered, "chars")
            s.text(filtered, "extractText", None)
    deduped = s.dedupe(page, alias(k, "dedupeChars"), pick([None, {"tolerance": 2}, {"extra_attrs": None}], k, 1)[0])
    if deduped is not None:
        s.page_get(deduped, "chars")
    # tables
    for settings in pick(TABLE_SETTINGS, k, 3):
        for t in s.tables(page, alias(k, "findTables"), settings)[:1]:
            for name in ("bbox", "cells", "rows", "columns"):
                s.table_get(t, name)
            s.extract(t)
            s.extract(t, {"x_tolerance": 1})
        s.tables(page, alias(k + 1, "findTable"), settings)
        s.tables(page, alias(k, "extractTables"), settings)
        s.tables(page, alias(k + 1, "extractTable"), settings)
    # serialization
    s.serialize(page, alias(k, "toDict"))
    s.serialize(page, alias(k + 1, "toDict"), ["rect", "line"])
    s.serialize(page, alias(k, "toJson"), pick([{}, {"precision": 2}, {"object_types": ["char"], "include_attrs": ["text", "x0"]}], k, 1)[0])
    s.serialize(page, alias(k + 1, "toCsv"), pick([{}, {"precision": 3}, {"object_types": ["nothing"]}], k, 1)[0])
    s.serialize(page, "toJson", {"stream": None, "indent": 2})
    # the document's members, close (the pages are made again), release
    for name in pick(["objects", "annots", "hyperlinks", "structure_tree"] + LISTS, k, 5):
        s.doc_get(doc, name)
    s.serialize(doc, alias(k, "toDict"))
    s.serialize(doc, alias(k + 1, "toJson"), {"precision": 1})
    s.close(page)
    s.text(page, "extractText", None)
    s.close(doc)
    again = s.pages(doc)
    s.page_get(again[0], "chars")
    s.pages(doc)
    s.release(doc)
    s.pages(doc)
    s.page_get(page, "chars")
    s.page_get(page, "width")
    s.text(page, "extractText", None)
    return s


def open_session() -> Session:
    """``open()`` options (and their errors), passwords, inputs that are
    not PDFs."""
    s = Session("open-options")
    rel = "corpus/micro/text-operators.pdf"
    for options in [{"pages": [1]}, {"pages": "1"}, {"pages": [1.0]}, {"laparams": {}},
                    {"laparams": {"line_margin": 0.3}}, {"unicode_norm": "NFKC"}, {"unicode_norm": "X"},
                    {"strict_metadata": True}, {"raise_unicode_errors": False}, {"password": "x"},
                    {"repair": False}, {"bogus": 1}]:
        doc = s.open(rel, options)
        page = s.pages(doc)[0]
        s.page_get(page, "objects")
        s.text(page, "extractText", None)
        s.release(doc)
    enc = "corpus/micro/encrypted-password.pdf"
    # (a bytes password is not modelled for an encrypted document:
    # docs/public-api.md, "Not Ported")
    for options in [None, {"password": "user"}, {"password": "wrong"}]:
        doc = s.open(enc, options)
        s.text(s.pages(doc)[0], "extractText", None)
        s.release(doc)
    s.open(data=b"%PDF-1.4\nnot a pdf")
    s.open(data=b"")
    return s


def path_session() -> Session:
    """Documents opened by path (Node: ``pdfplumber.open(path)``): after
    ``close()``, an object not read before raises ``ValueError``."""
    s = Session("open-path", node_only=True)
    for rel in ["upstream/pdfplumber-python/tests/pdfs/pdffill-demo.pdf", "corpus/micro/annotations.pdf"]:
        doc = s.open(rel, by_path=True)
        s.page_get(s.pages(doc)[0], "chars")
        s.close(doc)
        again = s.pages(doc)[0]
        s.page_get(again, "objects")
        s.page_get(again, "annots")
        s.doc_get(doc, "structure_tree")
        s.release(doc)
    return s


def order_session() -> Session:
    """Calls made without waiting run in the order they are made, whatever
    the order their promises are awaited in."""
    s = Session("await-order")
    doc = s.open("upstream/pdfplumber-python/tests/pdfs/nics-background-checks-2015-11.pdf")
    page = s.pages(doc)[0]
    with s.concurrently():
        s.page_get(page, "chars")
        s.text(page, "extractText", None)
        s.search(page, "Alabama", "Alabama", None)
        s.derive(page, "crop", [0, 0, 300, 300])
        s.filter(page, "upright")
        s.tables(page, "extractTables")
        s.close(page)
        s.text(page, "extractWords", None)
    with s.concurrently([0, 2, 1]):
        s.doc_get(doc, "objects")
        s.close(doc)
        s.pages(doc)
    return s


def cjk_session() -> Session:
    """CJK pages: their CMaps load lazily, the calls that missed one run
    again; the results are those of Python with every CMap."""
    s = Session("cjk")
    for rel in CJK:
        doc = s.open(rel)
        page = s.pages(doc)[0]
        s.text(page, "extractText", None)
        s.text(page, "extractWords", None)
        crop = s.derive(page, "crop", [0, 0, 300, 300], {"strict": False})
        if crop is not None:
            s.text(crop, "extractText", None)
        s.release(doc)
    return s


def adversarial_session(name: str, data: bytes) -> Session:
    """An adversarial PDF, the members that interpret its page."""
    s = Session(f"adv-{Path(name).stem}")
    doc = s.open(data=data)
    page = s.pages(doc)[0]
    s.page_get(page, "objects")
    s.text(page, "extractText", None)
    s.text(page, "extractWords", None)
    s.search(page, "e", "e", {"return_chars": False})
    s.tables(page, "findTables")
    s.page_get(page, "annots")
    s.derive(page, "crop", [0, 0, 100, 100], {"strict": False})
    # (to_dict, not to_json: inline image names are normalized in values,
    # not in texts)
    s.serialize(page, "toDict")
    s.doc_get(doc, "structure_tree")
    return s


def sessions() -> list[Session]:
    out = [file_session(k, rel) for k, rel in enumerate(FILES)]
    out += [open_session(), path_session(), order_session(), cjk_session()]
    adv = cache_dir() / "corpus" / "adv-plumber"
    for name in ADV:
        path = adv / name
        if not path.is_file():
            raise SystemExit(f"js_api: {path} is missing: generate adv-plumber first (python -m oracle.unit plumber)")
        out.append(adversarial_session(name, path.read_bytes()))
    return out


def main() -> None:
    require_oracle_python()
    path = unit_dir(MODULE) / "sessions.jsonl"
    tmp = path.with_name(f".{path.name}.tmp")
    n_sessions = n_calls = 0
    with open(tmp, "w", encoding="ascii") as f:
        for s in sessions():
            out = s.run()
            f.write(json.dumps({"in": [out["session"]], "out": out["expect"]}, ensure_ascii=True, separators=(",", ":")) + "\n")
            n_sessions += 1
            n_calls += len(out["expect"])
    tmp.replace(path)
    write_meta(MODULE, cases={"sessions": n_sessions, "calls": n_calls})
    print(f"{MODULE}: {n_sessions} sessions, {n_calls} calls")


if __name__ == "__main__":
    main()
