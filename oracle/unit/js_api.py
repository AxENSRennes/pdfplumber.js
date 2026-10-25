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

and, in the same form, ``depth.jsonl``: a session for each depth window
(``depth-*``, ``climit-*``) of the adversarial sets ``DEPTH_SETS``, where
Python's ``RecursionError`` happens through the API's statements (replayed
in Node by ``js/test/node/api-depth.test.ts``).

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
    "debug_tablefinder": "debugTablefinder",
    "find_all": "findAll",
    "element_bbox": "elementBbox",
    "all_mcids": "allMcids",
    "get_data": "getData",
    "get_rawdata": "getRawdata",
}

#: The object lists read with ``getattr(container, name)`` (``doc_attr``,
#: ``page_attr``).
LISTS = ["chars", "rects", "lines", "curves", "images", "edges", "rect_edges", "curve_edges", "horizontal_edges",
         "vertical_edges", "textboxhorizontals", "textboxverticals", "textlinehorizontals", "textlineverticals"]

#: The geometry of a page, in this order.
GEOMETRY = ["page_number", "initial_doctop", "rotation", "mediabox", "cropbox", "trimbox", "bleedbox", "artbox", "bbox",
            "width", "height"]

UNRESOLVED = Named("Unresolved", {})


# -------------------------------------------------- arguments in JavaScript forms
#
# An argument of a call is a Python value, written in typed JSON: the
# replayer gives a ``float`` as a ``PyFloat`` and a ``tuple`` as a
# ``PyTuple`` (the Python type crosses as it is). These classes write the
# other JavaScript forms an argument takes (``docs/public-api.md``,
# "Arguments"), each with the Python value the statement receives.


class JSForm:
    """An argument in a JavaScript form of its own."""

    def json(self, enc: Callable[[Any], Any]) -> Any:
        raise NotImplementedError

    def py(self, conv: Callable[[Any], Any]) -> Any:
        raise NotImplementedError


class Num(JSForm):
    """A plain JavaScript number: a safe integer (``-0`` excepted) crosses
    as an ``int``, any other number as a ``float``."""

    def __init__(self, x: float) -> None:
        self.x = float(x)

    def json(self, enc: Callable[[Any], Any]) -> Any:
        return {"$num": typed_json.encode(self.x)}

    def py(self, conv: Callable[[Any], Any]) -> Any:
        x = self.x
        safe = x == x and abs(x) <= 2**53 - 1 and x.is_integer() and not (x == 0 and str(x).startswith("-"))
        return int(x) if safe else x


class Big(JSForm):
    """A ``bigint``: an ``int``."""

    def __init__(self, n: int) -> None:
        self.n = n

    def json(self, enc: Callable[[Any], Any]) -> Any:
        return {"$bigint": str(self.n)}

    def py(self, conv: Callable[[Any], Any]) -> Any:
        return self.n


class JSMap(JSForm):
    """A ``Map`` with string keys: a ``dict``."""

    def __init__(self, d: dict[str, Any]) -> None:
        self.d = d

    def json(self, enc: Callable[[Any], Any]) -> Any:
        return {"$map": [[k, enc(v)] for k, v in self.d.items()]}

    def py(self, conv: Callable[[Any], Any]) -> Any:
        return {k: conv(v) for k, v in self.d.items()}


class Undefined(JSForm):
    """``undefined`` as a property: the option is not given."""

    def json(self, enc: Callable[[Any], Any]) -> Any:
        return {"$undefined": True}

    def py(self, conv: Callable[[Any], Any]) -> Any:
        raise AssertionError("undefined is a property of an object only")


class Ref(JSForm):
    """A value of an earlier call's result given back (``[call id, index
    or key...]``): the API gives it back as it is, the statement receives
    it as it crosses (``js_pages_run.cross``)."""

    def __init__(self, cid: int, *path: Any) -> None:
        self.cid = cid
        self.path = list(path)

    def json(self, enc: Callable[[Any], Any]) -> Any:
        return {"$ref": [self.cid, *self.path]}

    def py(self, conv: Callable[[Any], Any]) -> Any:
        raise AssertionError("a Ref is converted by its session")


class Refused(JSForm):
    """An argument the JavaScript encoder refuses (``TypeError``, before any
    statement): a container holding itself, a function."""

    def __init__(self, kind: str) -> None:
        self.kind = kind

    def json(self, enc: Callable[[Any], Any]) -> Any:
        return {f"${self.kind}": True}

    def py(self, conv: Callable[[Any], Any]) -> Any:
        raise AssertionError("a refused argument reaches no statement")


class Collector(JSForm):
    """An ``onWarning`` function of ``open()`` (the API's own option: no
    keyword of ``pdfplumber.open``)."""

    def json(self, enc: Callable[[Any], Any]) -> Any:
        return {"$warnings": True}

    def py(self, conv: Callable[[Any], Any]) -> Any:
        raise AssertionError("onWarning is not given to Python")


class ObjArg(JSForm):
    """An object of the API given as an argument (a page to
    ``structTree``): the object itself, the session's object in Python
    (``{"$obj": target}``, ``js_pages_run.arg_object``)."""

    def __init__(self, obj: "Obj") -> None:
        self.obj = obj

    def json(self, enc: Callable[[Any], Any]) -> Any:
        return {"$ref": self.obj.ref}

    def py(self, conv: Callable[[Any], Any]) -> Any:
        return {"$obj": self.obj.target}


#: The fields of the dataclass, in order, ``children`` last
#: (pdfplumber/structure.py:102 ``PDFStructElement``).
ELEMENT_FIELDS = ["type", "revision", "id", "lang", "alt_text", "actual_text", "title", "page_number", "attributes",
                  "mcids"]


def element_named(v: dict[str, Any]) -> Named:
    """An element as ``struct_tree`` writes it, as the API gives it:
    ``PDFStructElement{fields..., children}`` (without recursion)."""
    out: list[Named] = []
    stack: list[tuple[dict[str, Any], list[Any]]] = [(v, out)]
    while stack:
        e, into = stack.pop()
        children: list[Any] = []
        fields = {k: neutral(e[k]) for k in ELEMENT_FIELDS}
        fields["children"] = children
        into.append(Named("PDFStructElement", fields))
        # (filled in order: the children pushed last first)
        slots: list[list[Any]] = []
        for c in e["children"]:
            slot: list[Any] = []
            slots.append(slot)
            stack.append((c, slot))
        children.extend(_Deferred(s) for s in slots)
    return _resolve_deferred(out[0])


class _Deferred:
    def __init__(self, slot: list[Any]) -> None:
        self.slot = slot


def _resolve_deferred(root: Named) -> Named:
    """``element_named``'s children, put in place (without recursion)."""
    stack = [root]
    while stack:
        n = stack.pop()
        kids = n.fields["children"]
        for i, k in enumerate(kids):
            if isinstance(k, _Deferred):
                kids[i] = k.slot[0]
            stack.append(kids[i])
    return root


def element_at_path(top: list[Any], path: list[int]) -> dict[str, Any]:
    """The element value at a path (indices from the tree's children)."""
    children, el = top, None
    for i in path:
        el = children[i]
        children = el["children"]
    assert el is not None
    return el


def encode_arg(a: Any) -> Any:
    """An argument as the session writes it (typed JSON, JavaScript forms)."""
    if isinstance(a, JSForm):
        return a.json(encode_arg)
    if type(a) is list:
        return [encode_arg(x) for x in a]
    if type(a) is tuple:
        return {"t": [encode_arg(x) for x in a]}
    if type(a) is dict:
        return {"d": [[typed_json.encode(k), encode_arg(v)] for k, v in a.items()]}
    return typed_json.encode(a)


def refs_in(a: Any) -> list[Ref]:
    """The ``Ref`` arguments in ``a``."""
    if isinstance(a, Ref):
        return [a]
    if isinstance(a, JSMap):
        return refs_in(list(a.d.values()))
    if type(a) in (list, tuple):
        return [x for i in a for x in refs_in(i)]
    if type(a) is dict:
        return refs_in(list(a.values()))
    return []


def refused(a: Any) -> bool:
    """Whether the JavaScript encoder refuses the argument."""
    if isinstance(a, Refused):
        return True
    if isinstance(a, JSMap):
        return any(refused(v) for v in a.d.values())
    if type(a) in (list, tuple):
        return any(refused(x) for x in a)
    if type(a) is dict:
        return any(refused(x) for x in a.values())
    return False


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
    if isinstance(s, Named) and s.name == "PDFStream":
        return s.fields["objid"] is None and o["name"].isdigit()
    return isinstance(s, PDFStream) and s.objid is None and o["name"].isdigit()


def js_error(name: str) -> Named:
    """An error of the API itself (no Python counterpart)."""
    return Named("Error", {"name": name, "message": None, "cause": None})


def neutral(v: Any) -> Any:
    """A result value as the API gives it: a ``Page`` held in it by its
    ``page_number`` (its handle is the core's), a stream by reference
    ``PDFStream{objid, genno, attrs}`` (its data is read on demand), inline
    image names as ``inline_names`` writes them."""

    def go(x: Any) -> Any:
        if isinstance(x, Named) and x.name == "Page":
            return Named("Page", {"page_number": x.fields["page_number"]})
        if isinstance(x, Named) and x.name == "StreamRef":
            return Named("PDFStream", {k: go(x.fields[k]) for k in ("objid", "genno", "attrs")})
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
    order of ``GEOMETRY``, without its handle; ``page_info`` gives the
    exception of ``width`` or ``height`` as ``width_error``/``height_error``
    (each read in its own statement): an ``Error``."""
    out: dict[str, Any] = {}
    for k in GEOMETRY:
        if k in g:
            out[k] = g[k]
        elif f"{k}_error" in g:
            out[k] = error(g[f"{k}_error"])
    return out


def table_value(t: dict[str, Any]) -> Named:
    return Named("Table", {k: t[k] for k in ("bbox", "cells", "rows", "columns")})


class Results:
    """The results of a session's statements."""

    def __init__(self, results: list[Any], causes: list[Any]) -> None:
        self.results = results
        self.causes = causes

    def value(self, case: int) -> Any:
        return self.results[case][0]

    def extra(self, case: int) -> Any:
        return self.results[case][1]

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
        #: the case of the read that made ``pages``
        self.pages_case = -1
        self.released = False
        #: the pages the API holds objects of (``js_pages_run.held_by_api``
        #: entries), with whether each is a page of ``pdf.pages`` (which
        #: ``close()`` drops) or a derived page (which it keeps)
        self.held: list[tuple[list[Any], bool]] = []


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
        #: a derived page: the page it derives from; a table: its page
        self.parent: Obj | None = None


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
        #: the case of each call made of one statement whose value is the
        #: statement's (``Ref`` to its result)
        self.case_of: dict[int, int] = {}
        #: the cases that made pages, to find a page's geometry by its
        #: handle: ``doc_pages`` cases, derived pages (the case that made
        #: it, its geometry), ``api_page`` cases
        self.pages_cases: list[int] = []
        self.derived_cases: list[tuple[int, Callable[[Results], dict[str, Any]]]] = []
        self.api_page_cases: list[int] = []
        # the core of the API holds the streams of its results by reference
        # (runtime.ts: set_stream_refs, once, before any document)
        self.case("set_stream_refs", None, [True])

    # -------------------------------------------------------------- plumbing

    def py(self, a: Any) -> Any:
        """An argument as the statement receives it."""
        if isinstance(a, Ref):
            return {"$cross": [self.case_of[a.cid], *a.path]}
        if isinstance(a, JSForm):
            return a.py(self.py)
        if type(a) is list:
            return [self.py(x) for x in a]
        if type(a) is tuple:
            return tuple(self.py(x) for x in a)
        if type(a) is dict:
            return {k: self.py(v) for k, v in a.items() if not isinstance(v, (Undefined, Collector))}
        return a

    def case(self, op: str, target: Any, args: list[Any], data: Any = None, js: Any = None) -> int:
        self.cases.append([op, target, self.py(args), data, js])
        return len(self.cases) - 1

    def plain_case(self, cid: int, case: int) -> int:
        """Call ``cid``'s value is case ``case``'s (a ``Ref`` reads it)."""
        self.case_of[cid] = case
        return case

    def geometry_of(self, r: Results, handle: str) -> dict[str, Any]:
        """The geometry of the page the API holds under ``handle``."""
        for c in self.pages_cases:
            if not r.unresolved(c) and not r.failed(c) and type(r.value(c)) is list:
                for g in r.value(c):
                    if g["handle"] == handle:
                        return page_geometry(g)
        for c, geometry in self.derived_cases:
            if not r.unresolved(c) and not r.failed(c) and r.value(c)["handle"] == handle:
                return geometry(r)
        for c in self.api_page_cases:
            extra = r.extra(c)
            if extra is not None and extra["handle"] == handle:
                return page_geometry(r.value(c))
        raise AssertionError(f"{self.name}: no page {handle}")

    def _call(self, on: Obj | None, kind: str, name: str, args: list[Any], **extra: Any) -> int:
        cid = self.next_id
        self.next_id += 1
        call: dict[str, Any] = {"id": cid, "on": None if on is None else on.ref, kind: name,
                                "args": [encode_arg(a) for a in args]}
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
        """An asynchronous member: the call, and whether it runs no
        statement: its document was released (``DocumentReleased``), or an
        argument is one the JavaScript encoder refuses (``TypeError``, at
        the job's first request)."""
        cid = self._call(obj, kind, name, args, **extra)
        refs = refs_in(args)

        def fails(name: str) -> Callable[[Results], Any]:
            # (the replayer reads the values an argument gives back first:
            # one that does not exist leaves the call unmade)
            return lambda r: UNRESOLVED if any(self.ref_missing(r, x) for x in refs) else js_error(name)

        if obj.doc.released:
            self._expect(cid, obj, fails("DocumentReleased"))
            return cid, True
        if refused(args):
            self._expect(cid, obj, fails("TypeError"))
            return cid, True
        return cid, False

    def ref_missing(self, r: Results, ref: Ref) -> bool:
        """Whether the value a ``Ref`` names does not exist."""
        c = self.case_of[ref.cid]
        if r.unresolved(c) or r.failed(c):
            return True
        v = r.value(c)
        for k in ref.path:
            if type(v) in (list, tuple) and type(k) is int and 0 <= k < len(v):
                v = v[k]
            elif type(v) is dict and k in v:
                v = v[k]
            else:
                return True
        return False

    def concurrently(self, order: list[int] | None = None) -> "_Batch":
        """``with s.concurrently(order):`` the calls made inside are made
        without waiting, then awaited in ``order`` (reversed by default)."""
        return _Batch(self, order)

    # ------------------------------------------------------------ the module

    def open(self, rel: str | None = None, options: dict[str, Any] | None = None, *, data: bytes | None = None,
             by_path: bool = False, as_: str | None = None) -> Obj:
        """``open(input, options)``: ``doc_open`` (bytes), or
        ``doc_open_file`` (a path, Node: ``node_only`` sessions). Every
        option but ``logLevel`` and ``onWarning`` is a keyword argument of
        ``pdfplumber.open``. ``as_``: the JavaScript form of the bytes
        (``blob``, ``arraybuffer``, ``dataview``: a view at an offset of a
        larger buffer), or ``file-url`` (Node: a ``file:`` URL, opened as
        its path)."""
        given: Any = options
        options = dict(options.d if isinstance(options, JSMap) else options or {})
        api_options = {k: v for k, v in options.items() if k not in ("logLevel", "onWarning")}
        if by_path or as_ == "file-url":
            assert self.node_only and rel is not None
            inp: dict[str, Any] = {"path": rel} if as_ is None else {"path": rel, "as": as_}
            c = self.case("doc_open_file", None, [rel, api_options], rel)
        elif rel is not None:
            inp = {"bytes_of": rel} if as_ is None else {"bytes_of": rel, "as": as_}
            c = self.case("doc_open", None, [api_options], rel)
        else:
            assert data is not None
            inp = {"bytes": data.hex()}
            c = self.case("doc_open", None, [api_options], data)
        cid = self._call(None, "call", "open", [given] if options else [], input=inp)
        obj = Obj("document", [cid], [c, 0], Doc(), lambda r: not r.failed(c))
        self._expect(cid, None, lambda r: r.error(c) if r.failed(c) else Named("PDFPlumberDocument", {}))
        return obj

    def options(self, doc: Obj, options: dict[str, Any]) -> None:
        """``pdf.options``: the options of ``open()``, as given."""
        cid = self._call(doc, "get", "options", [])
        self._expect(cid, doc, lambda r: options)

    # ---------------------------------------------------------- the document

    def pages(self, doc: Obj) -> list[Obj]:
        """``await pdf.pages``: ``doc_pages`` at the first read (and after
        ``close()``), the same array then; a rejected read is not kept by
        the API: the next read runs ``doc_pages`` again (``api_pages``:
        ``doc_pages`` if the read of before raised, nothing otherwise)."""
        d = doc.doc
        cid, released = self._async(doc, "get", "pages", [])
        if released:
            d.pages = None
            return []
        if d.pages is not None:
            first = d.pages_case
            again = self.case("api_pages", doc.target, [first])
            kept = d.pages_value

            def value_again(r: Results) -> Any:
                if r.unresolved(first) or not r.failed(first):
                    return kept(r)  # type: ignore[misc]
                if r.failed(again):
                    return r.error(again)
                return [Named("PDFPlumberPage", page_geometry(g)) for g in r.value(again)]

            self._expect(cid, doc, value_again)
            self.pages_cases.append(again)
            d.held.append(([again, "*"], True))
            d.pages_case = again
            d.pages_value = value_again
            return d.pages
        c = self.case("doc_pages", doc.target, [])
        d.pages_case = c
        self.pages_cases.append(c)
        d.held.append(([c, "*"], True))

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
        self.plain_case(cid, c)
        self._expect(cid, doc, lambda r: r.plain(c))
        return cid
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
        self.plain_case(cid, c)
        self._expect(cid, obj, lambda r: r.plain(c))

    def close(self, obj: Obj) -> None:
        """``pdf.close()`` (``PDF.close()``: the pages are made again at the
        next read) or ``page.close()``."""
        cid, released = self._async(obj, "call", "close", [])
        if released:
            return
        c = self.case("doc_close" if obj.kind == "document" else "page_close", obj.target, [])
        if obj.kind == "document":
            self._closed(obj.doc)
        self._expect(cid, obj, lambda r: r.plain(c))

    @staticmethod
    def _closed(d: Doc) -> None:
        """``pdf.close()``: the pages are made again at the next read; the
        API still names the pages of before while they live (the session
        holds them all)."""
        d.pages = None

    def release(self, doc: Obj) -> None:
        """``pdf.release()``: the document leaves the core (no statement);
        idempotent."""
        cid = self._call(doc, "call", "release", [])
        doc.doc.released = True
        doc.doc.pages = None
        doc.doc.held = []
        self._expect(cid, doc, lambda r: None)

    def dispose(self, doc: Obj) -> None:
        """``await pdf[Symbol.asyncDispose]()`` (``await using``):
        ``close()``, then ``release()``; ``close()``'s error, if any."""
        cid = self._call(doc, "call", "asyncDispose", [])
        if doc.doc.released:
            self._expect(cid, doc, lambda r: None)
            return
        c = self.case("doc_close", doc.target, [])
        self._closed(doc.doc)
        doc.doc.released = True
        doc.doc.held = []
        self._expect(cid, doc, lambda r: r.error(c) if r.failed(c) else None)

    def stream(self, doc: Obj, cid: int, *path: Any) -> Obj:
        """The ``PDFStream`` at ``path`` in call ``cid``'s value (an image's
        ``stream``): a reference to the stream in the core."""
        ref = Ref(cid, *path)
        return Obj("stream", [cid, *path], {"at": [self.case_of[cid], *path]}, doc.doc,
                   lambda r: not self.ref_missing(r, ref))

    def stream_read(self, stream: Obj, name: str) -> int:
        """``rawdata``, ``data`` (promises: the stream as it is when they
        are read), ``getData()``, ``getRawdata()`` (and their snake_case
        names): ``stream_rawdata``... in the document's job."""
        member = {"get_data": "getData", "get_rawdata": "getRawdata"}.get(name, name)
        kind = "get" if member in ("rawdata", "data") else "call"
        cid, skipped = self._async(stream, kind, name, [])
        if skipped:
            return cid
        op = {"rawdata": "stream_rawdata", "data": "stream_data", "getData": "stream_get_data",
              "getRawdata": "stream_get_rawdata"}[member]
        c = self.case(op, stream.target, [])
        self._expect(cid, stream, lambda r: r.plain(c))
        return cid

    # ------------------------------------------------------ the structure tree

    def struct_tree(self, doc: Obj, page: Obj | None = None) -> Obj | None:
        """``pdf.structTree(page?)``: ``struct_tree`` (``PDFStructTree(pdf,
        page)``), a ``PDFStructTree{page, children}`` of
        ``PDFStructElement{fields..., children}``."""
        cid, skipped = self._async(doc, "call", "structTree", [] if page is None else [ObjArg(page)])
        if skipped:
            return None
        c = self.case("struct_tree", doc.target, [None if page is None else ObjArg(page)])
        self.plain_case(cid, c)

        def value(r: Results) -> Any:
            if page is not None and not page.alive(r):
                return UNRESOLVED
            if r.unresolved(c):
                return UNRESOLVED
            if r.failed(c):
                return r.error(c)
            g = None if page is None else Named("PDFPlumberPage", page.geometry(r))  # type: ignore[misc]
            return Named("PDFStructTree", {"page": g, "children": [element_named(e) for e in r.value(c)["children"]]})

        self._expect(cid, doc, value)
        alive = (lambda r: doc.alive(r) and (page is None or page.alive(r)) and not r.unresolved(c)
                 and not r.failed(c))
        return Obj("tree", [cid], [c, 0], doc.doc, alive)

    def element(self, tree: Obj, *path: int) -> Obj:
        """The element of a tree at ``path`` (indices from its children)."""
        steps: list[Any] = []
        for i in path:
            steps += ["children", i]
        ref = Ref(tree.ref[0], *steps)
        el = Obj("element", [tree.ref[0], *steps], tree.target, tree.doc, lambda r: tree.alive(r) and not self.ref_missing(r, ref))
        el.parent = tree
        el.path = list(path)  # type: ignore[attr-defined]
        return el

    def struct_find(self, on: Obj, name: str, matcher: tuple[str, Any]) -> None:
        """``findAll``/``find`` (and ``find_all``) on a tree or an element:
        ``struct_find_all``/``struct_find`` with the path of the element
        (``None`` for the tree); ``matcher`` ``("tag", type)``, ``("regexp",
        source)`` (a ``RegExp``), ``("predicate", name)`` (a function,
        ``js_pages_run.ELEMENT_PREDICATES``) or ``("value", v)`` (any
        value, as it is)."""
        kind, m = matcher
        extra: dict[str, Any] = {}
        if kind == "tag" or kind == "value":
            js_args, py_m = [m], m
        elif kind == "regexp":
            js_args, py_m = [{"regexp": m, "flags": ""}], {"regexp": m}
        else:
            js_args, py_m = [], {"predicate": m}
            extra["element_predicate"] = m
        cid, skipped = self._async(on, "call", name, js_args, **extra)
        if skipped:
            return
        tree = on if on.kind == "tree" else on.parent
        assert tree is not None
        path = getattr(on, "path", None)
        member = ALIASES.get(name, name)
        c = self.case("struct_find_all" if member == "findAll" else "struct_find", tree.target, [path, py_m])
        tree_case = self.case_of[tree.ref[0]]

        def value(r: Results) -> Any:
            if r.unresolved(c):
                return UNRESOLVED
            if r.failed(c):
                return r.error(c)
            top = r.value(tree_case)["children"]
            v = r.value(c)
            if member == "findAll":
                return [element_named(element_at_path(top, p)) for p in v]
            return None if v is None else element_named(element_at_path(top, v))

        self._expect(cid, on, value)

    def element_bbox(self, tree: Obj, el: Obj, name: str = "elementBbox") -> None:
        """``tree.elementBbox(el)``: ``struct_element_bbox [path]``."""
        steps = el.ref[1:]
        ref = Ref(el.ref[0], *steps)
        cid, skipped = self._async(tree, "call", name, [ref])
        if skipped:
            return
        c = self.case("struct_element_bbox", tree.target, [el.path])  # type: ignore[attr-defined]
        self._expect(cid, tree, lambda r: UNRESOLVED if self.ref_missing(r, ref) else r.plain(c))

    def element_get(self, el: Obj, name: str) -> None:
        """A field of an element (``type``, ``alt_text``..., or the getters
        ``altText``, ``actualText``, ``pageNumber``): synchronous, the
        dataclass's field in the tree's value."""
        cid = self._call(el, "get", name, [])
        field = {"altText": "alt_text", "actualText": "actual_text", "pageNumber": "page_number"}.get(name, name)
        tree = el.parent
        assert tree is not None
        tree_case = self.case_of[tree.ref[0]]
        path = el.path  # type: ignore[attr-defined]
        self._expect(cid, el, lambda r: element_at_path(r.value(tree_case)["children"], path)[field])

    def element_call(self, el: Obj, name: str) -> None:
        """``el.allMcids()`` (``all_mcids``), ``el.repr()``."""
        cid, skipped = self._async(el, "call", name, [])
        if skipped:
            return
        op = {"allMcids": "struct_all_mcids", "repr": "struct_element_repr"}[ALIASES.get(name, name)]
        c = self.case(op, el.target, [el.path])  # type: ignore[attr-defined]
        self._expect(cid, el, lambda r: r.plain(c))

    def page_ref(self, doc: Obj, cid: int, *path: Any) -> Obj | None:
        """``pdf.page(ref)``, the ``PageRef`` at ``path`` in call ``cid``'s
        value (an annotation's ``data.P``): the page the API holds for its
        handle (no statement), else ``page_info`` (``api_page``)."""
        ref = Ref(cid, *path)
        call, released = self._async(doc, "call", "page", [ref])
        if released:
            return None
        target = {"at": [self.case_of[cid], *path]}
        c = self.case("api_page", target, [[t for t, _ in doc.doc.held]])
        self.api_page_cases.append(c)
        doc.doc.held.append(([c, "label"], True))

        def geometry(r: Results) -> dict[str, Any]:
            v = r.value(c)
            return self.geometry_of(r, v["known"]) if "known" in v else page_geometry(v)

        def value(r: Results) -> Any:
            if r.unresolved(c):
                return UNRESOLVED
            return r.error(c) if r.failed(c) else Named("PDFPlumberPage", geometry(r))

        self._expect(call, doc, value)
        page = Obj("page", [call], target, doc.doc, lambda r: doc.alive(r) and not r.unresolved(c) and not r.failed(c))
        page.geometry = geometry
        return page

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
            return cid
        if member == "pdf":
            cid = self._call(page, "get", name, [])
            self._expect(cid, page, lambda r: Named("PDFPlumberDocument", {}))
            return cid
        if member == "parent":
            # the page a derived page derives from (DerivedPage.parent_page)
            cid = self._call(page, "get", name, [])
            parent = page.parent
            self._expect(cid, page, lambda r: None if parent is None else Named("PDFPlumberPage", parent.geometry(r)))  # type: ignore[misc]
            return cid
        cid, released = self._async(page, "get", name, [])
        if released:
            return cid
        if member in LISTS or member in ("annots", "hyperlinks"):
            c = self.case("page_attr", page.target, [member])
        else:
            c = self.case({"objects": "page_objects", "structure_tree": "page_structure_tree"}[member], page.target, [])
        self.plain_case(cid, c)
        self._expect(cid, page, lambda r: r.plain(c))
        return cid

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
        kw = dict(options.d if isinstance(options, JSMap) else options or {})
        if member == "extractTextLines":
            # (an option undefined is not given: its default)
            strip = kw.pop("strip", True)
            chars = kw.pop("return_chars", True)
            strip = True if isinstance(strip, Undefined) else strip
            chars = True if isinstance(chars, Undefined) else chars
            c = self.case("page_extract_text_lines", page.target, [strip, chars, kw])
        else:
            op = {"extractText": "page_extract_text", "extractTextSimple": "page_extract_text_simple",
                  "extractWords": "page_extract_words"}[member]
            c = self.case(op, page.target, [kw])
        self.plain_case(cid, c)
        self._expect(cid, page, lambda r: r.plain(c))
        return cid

    def search(self, page: Obj, py_pattern: Any, js_pattern: Any, options: dict[str, Any] | None = None) -> None:
        """``search(pattern, options)``: one statement,
        ``page.search(pattern, **options)`` (``js_pattern`` matches as
        ``py_pattern`` does under Python's ``re``; ``{"$compiled": source}``
        a compiled pattern, the ``RegExp`` of the API)."""
        cid, released = self._async(page, "call", "search", [js_pattern] + ([] if options is None else [options]))
        if released:
            return
        c = self.case("search", page.target, [py_pattern, dict(options.d if isinstance(options, JSMap) else options or {})],
                      js=js_pattern)
        self.plain_case(cid, c)
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
        derived.parent = page
        self.derived_cases.append((made, geometry))
        page.doc.held.append(([made, 0], False))

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
              "extractTable": "page_extract_table", "debugTablefinder": "page_debug_tablefinder"}[member]
        c = self.case(op, page.target, list(args))

        def value(r: Results) -> Any:
            if r.unresolved(c):
                return UNRESOLVED
            if r.failed(c):
                return r.error(c)
            v = r.value(c)
            if member == "findTables":
                return [table_value(t) for t in v]
            if member == "findTable":
                return None if v is None else table_value(v)
            if member == "debugTablefinder":
                return {k: [table_value(t) for t in x] if k == "tables" else neutral(x) for k, x in v.items()}
            return neutral(v)

        self._expect(cid, page, value)

        def made(r: Results, i: int) -> bool:
            if not page.alive(r) or r.unresolved(c) or r.failed(c):
                return False
            v = r.value(c)
            if member == "debugTablefinder":
                return i < len(v["tables"])
            return i < len(v) if member == "findTables" else v is not None

        if member in ("extractTables", "extractTable"):
            self.plain_case(cid, c)
        found: list[Obj] = []
        if member == "findTables":
            found = [Obj("table", [cid, i], [c, i], page.doc, lambda r, i=i: made(r, i)) for i in range(2)]
        if member == "debugTablefinder":
            found = [Obj("table", [cid, "tables", i], [c, i], page.doc, lambda r, i=i: made(r, i)) for i in range(2)]
        if member == "findTable":
            found = [Obj("table", [cid], [c, 0], page.doc, lambda r: made(r, 0))]
        for t in found:
            t.parent = page
        return found

    def table_get(self, table: Obj, name: str) -> None:
        """``bbox``, ``cells``, ``rows``, ``columns`` of a table (known
        since it was found), ``page`` (the page it was found on)."""
        cid = self._call(table, "get", name, [])
        if name == "page":
            page = table.parent
            assert page is not None and page.geometry is not None
            geometry = page.geometry
            self._expect(cid, table, lambda r: Named("PDFPlumberPage", geometry(r)))
            return
        ref = table.ref
        found = ref[0]

        def value(r: Results) -> Any:
            f = next(f for c, f in self.expect if c == found)
            t = f(r)
            for k in ref[1:]:
                t = t[k]
            return t.fields[name]

        self._expect(cid, table, value)

    def extract(self, table: Obj, options: dict[str, Any] | None = None) -> None:
        cid, released = self._async(table, "call", "extract", [] if options is None else [options])
        if released:
            return
        c = self.case("table_extract", table.target, [dict(options or {})])
        self.plain_case(cid, c)
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
        for t in s.tables(page, alias(k, "debugTablefinder"), settings)[:1]:
            for name in ("bbox", "rows", "page"):
                s.table_get(t, name)
            s.extract(t, {"x_tolerance": 2})
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
    # calls made before release() run (the core frees the document after
    # them), calls made after reject
    again = s.pages(doc)[0]
    with s.concurrently([2, 1, 0]):
        s.page_get(again, "chars")
        s.release(doc)
        s.text(again, "extractText", None)
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
    # read again: the same pages, or doc_pages again after a read that raised
    s.pages(doc)
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


#: The adversarial sets whose depth windows (``depth-*``, ``climit-*``
#: files) ``depth.jsonl`` holds: where Python's ``RecursionError`` happens
#: through the API's statements, which the stage traces cannot show (their
#: oracle calls pdfplumber from nested generators: docs/verification.md).
DEPTH_SETS = ["adv-plumber", "adv-structure", "adv-interp", "adv-objects", "adv-pages"]


def depth_session(aset: str, path: Path) -> Session:
    """A depth window of an adversarial set: the members of
    ``adversarial_session``, the metadata, and the structure tree (the
    document's tree, an element's ``repr``, ``allMcids``,
    ``elementBbox``)."""
    s = Session(f"depth-{aset}-{path.stem}")
    root = corpus_root()
    try:
        doc = s.open(str(path.relative_to(root)))
    except ValueError:
        doc = s.open(data=path.read_bytes())
    s.doc_get(doc, "metadata")
    page = s.pages(doc)[0]
    s.pages(doc)
    s.page_get(page, "objects")
    s.text(page, "extractText", None)
    s.text(page, "extractWords", None)
    s.search(page, "e", "e", {"return_chars": False})
    s.tables(page, "findTables")
    s.page_get(page, "annots")
    s.derive(page, "crop", [0, 0, 100, 100], {"strict": False})
    s.serialize(page, "toDict")
    s.page_get(page, "structure_tree")
    s.doc_get(doc, "structure_tree")
    tree = s.struct_tree(doc)
    if tree is not None:
        s.struct_find(tree, "findAll", ("tag", "P"))
        el = s.element(tree, 0)
        s.element_call(el, "repr")
        s.element_call(el, "allMcids")
        s.element_bbox(tree, el)
    return s


def depth_sessions() -> list[Session]:
    out = []
    for aset in DEPTH_SETS:
        d = cache_dir() / "corpus" / aset
        if not d.is_dir():
            raise SystemExit(f"js_api: {d} is missing: generate {aset} first (python -m oracle.unit)")
        out += [depth_session(aset, p) for p in sorted(d.glob("*.pdf")) if re.search(r"(^|-)(depth|climit)-", p.stem)]
    return out


def inputs_session() -> Session:
    """The forms of ``open()``'s input (bytes as a ``Blob``, an
    ``ArrayBuffer``, a view at an offset of a larger buffer), ``null``
    options, the API's own options (no keyword of ``pdfplumber.open``),
    ``pdf.options``."""
    s = Session("open-inputs")
    rel = "corpus/micro/text-operators.pdf"
    for as_ in ("blob", "arraybuffer", "dataview"):
        doc = s.open(rel, as_=as_)
        s.text(s.pages(doc)[0], "extractText", None)
        s.release(doc)
    for options in [{"logLevel": "silent"}, {"logLevel": "warning", "pages": [1]}, {"onWarning": Collector()},
                    JSMap({"pages": [1], "logLevel": "silent"}), {"pages": Undefined()}]:
        doc = s.open(rel, options)  # type: ignore[arg-type]
        if type(options) is dict and not any(isinstance(v, JSForm) for v in options.values()):
            s.options(doc, options)
        s.text(s.pages(doc)[0], "extractWords", None)
        s.release(doc)
    # an option that cannot cross: TypeError, no statement
    cid = s._call(None, "call", "open", [{"bogus": Refused("function")}], input={"bytes_of": rel})
    s._expect(cid, None, lambda r: js_error("TypeError"))
    # null options: no option (pdfplumber.open(x))
    cid = s._call(None, "call", "open", [None], input={"bytes_of": rel})
    c = s.case("doc_open", None, [None], rel)
    s._expect(cid, None, lambda r: r.error(c) if r.failed(c) else Named("PDFPlumberDocument", {}))
    doc = Obj("document", [cid], [c, 0], Doc(), lambda r: not r.failed(c))
    s.text(s.pages(doc)[0], "extractText", None)
    s.release(doc)
    return s


def url_session() -> Session:
    """Node: a ``file:`` URL is opened as its path (``pdfplumber.open(path)``)."""
    s = Session("open-file-url", node_only=True)
    doc = s.open("corpus/micro/annotations.pdf", as_="file-url")
    s.page_get(s.pages(doc)[0], "annots")
    s.close(doc)
    s.page_get(s.pages(doc)[0], "chars")
    s.release(doc)
    return s


def arguments_session() -> Session:
    """Arguments in their JavaScript forms (``docs/public-api.md``,
    "Arguments"): plain numbers (an integral one crosses as an ``int``,
    visible in ``to_json``), ``PyFloat``, ``PyTuple``, ``bigint``, ``Map``,
    ``undefined`` options, objects of a result given back (explicit table
    lines), values the encoder refuses; options that are not a mapping."""
    s = Session("arguments")
    doc = s.open("upstream/pdfplumber-python/tests/pdfs/nics-background-checks-2015-11.pdf")
    page = s.pages(doc)[0]
    for bbox in ([Num(0), Num(0), Num(300.0), Num(300.5)], (0.0, 0, 300, 300.5), [Big(0), 0, Big(300), 300],
                 [Num(-0.0), 0, 200, 200], [Num(float("nan")), 0, 100, 100]):
        crop = s.derive(page, "crop", bbox)
        if crop is not None:
            s.serialize(crop, "toJson", {"object_types": ["rect"], "precision": 2})
    rects = s.page_get(page, "rects")
    lines = s.page_get(page, "lines")
    s.page_get(page, "edges")
    explicit = [
        {"vertical_strategy": "explicit", "horizontal_strategy": "lines",
         "explicit_vertical_lines": [Ref(rects, 0), Ref(rects, 1), Num(100), 200.5, Num(400.0)]},
        {"vertical_strategy": "lines", "horizontal_strategy": "explicit",
         "explicit_horizontal_lines": [Ref(lines, 0), Ref(lines, 3), 100, Big(150)]},
        {"vertical_strategy": "explicit", "horizontal_strategy": "explicit",
         "explicit_vertical_lines": [Ref(rects, 5), 300, 400], "explicit_horizontal_lines": [Ref(rects, 6), 100, 200]},
        {"vertical_strategy": "explicit", "explicit_vertical_lines": [Ref(rects, 100000)]},
    ]
    for settings in explicit:
        for t in s.tables(page, "findTables", settings)[:1]:
            s.table_get(t, "bbox")
            s.extract(t)
        s.tables(page, "extractTables", settings)
    s.tables(page, "extractTable", JSMap({"snap_tolerance": Num(4.0), "intersection_tolerance": 2.0}))
    words = s.text(page, "extractWords", JSMap({"x_tolerance": Num(2.0), "extra_attrs": ["size"]}))  # type: ignore[arg-type]
    s.text(page, "extractWords", {"x_tolerance": Undefined(), "y_tolerance": 2.0})
    s.text(page, "extractTextLines", JSMap({"strip": False, "return_chars": False, "layout": True}))  # type: ignore[arg-type]
    s.text(page, "extractTextLines", {"strip": Undefined(), "return_chars": False})
    s.search(page, "Alabama", "Alabama", JSMap({"return_chars": False, "x_tolerance": 1}))  # type: ignore[arg-type]
    s.text(page, "extractText", {"layout": True, "layout_bbox": (0, 0, 400.0, 400), "x_density": Num(7.0)})
    s.text(page, "extractText", {"x_tolerance": Refused("function")})
    s.text(page, "extractText", {"extra_attrs": Refused("cyclic")})
    # options that are not a mapping: Python's TypeError (argument after **)
    for name in ("extractText", "extractWords", "extractTextLines"):
        cid = s._call(page, "call", name, [None])
        member = ALIASES.get(name, name)
        if member == "extractTextLines":
            c = s.case("page_extract_text_lines", page.target, [True, True, None])
        else:
            c = s.case({"extractText": "page_extract_text", "extractWords": "page_extract_words"}[member], page.target, [None])
        s._expect(cid, page, lambda r, c=c: r.plain(c))
    for options in (None, [1]):
        cid = s._call(page, "call", "search", ["a", options])
        c = s.case("search", page.target, ["a", options], js="a")
        s._expect(cid, page, lambda r, c=c: r.plain(c))
    cid = s._call(page, "call", "extractTextLines", [[]])
    c = s.case("page_extract_text_lines", page.target, [True, True, []])
    s._expect(cid, page, lambda r: r.plain(c))
    # a word given back to a call that reads objects (dedupe's extra_attrs
    # are names; crop's bbox from a word's coordinates)
    s.derive(page, "crop", [Ref(words, 0, "x0"), Ref(words, 0, "top"), Ref(words, 3, "x1"), Ref(words, 3, "bottom")])
    s.release(doc)
    return s


def lifetime_session() -> Session:
    """``pdf.page(ref)`` (the page of a ``PageRef``: before and after
    ``pdf.pages``, of a derived page, after ``close()``), ``page.pdf``,
    ``page.parent``, ``table.page``, ``close()`` twice, ``release()`` twice,
    ``await using`` (``Symbol.asyncDispose``), and every kind of object
    after ``release()``."""
    s = Session("lifetime")
    # (annotations whose /P names their page, on the first two pages)
    rel = "upstream/pdfplumber-python/tests/pdfs/issue-463-example.pdf"
    doc = s.open(rel)
    annots = s.doc_get(doc, "annots")
    first = s.page_ref(doc, annots, 0, "data", "P")
    if first is not None:
        s.page_get(first, "chars")
        s.page_get(first, "pdf")
        s.page_get(first, "parent")
    for i in (1, 2):
        s.page_ref(doc, annots, i, "data", "P")
    pages = s.pages(doc)
    page = pages[0]
    s.page_get(pages[1], "annots")
    # annotations given back as explicit table lines: their data (names,
    # references, the page) cross as the objects they stand for
    s.tables(page, "extractTables", {"vertical_strategy": "explicit", "horizontal_strategy": "explicit",
                                     "explicit_vertical_lines": [Ref(annots, 0), Ref(annots, 1), 10, 600],
                                     "explicit_horizontal_lines": [Ref(annots, 0), 10, 700]})
    # the page of pdf.pages is the page made above (the same object)
    again = s.page_get(page, "annots")
    s.page_ref(doc, again, 0, "data", "P")
    s.page_ref(doc, again, 1, "data", "P")
    crop = s.derive(page, "crop", [0, 0, 400, 400])
    if crop is not None:
        s.page_get(crop, "parent")
        s.page_get(crop, "pdf")
        cropped = s.page_get(crop, "annots")
        derived = s.page_ref(doc, cropped, 0, "data", "P")
        if derived is not None:
            s.page_get(derived, "bbox")
            s.text(derived, "extractText", None)
        s.serialize(crop, "toDict", ["annot"])
    for t in s.tables(page, "findTables", {"vertical_strategy": "text", "horizontal_strategy": "text"})[:1]:
        s.table_get(t, "page")
    s.close(doc)
    s.close(doc)
    # after close(): the pages are made again, a reference of before names
    # the page it named (a page of the flushed list)
    s.page_ref(doc, again, 0, "data", "P")
    later = s.pages(doc)[0]
    s.page_ref(doc, s.page_get(later, "annots"), 0, "data", "P")
    s.release(doc)
    s.release(doc)
    s.page_ref(doc, again, 0, "data", "P")
    s.doc_get(doc, "metadata")
    if crop is not None:
        s.page_get(crop, "bbox")
        s.page_get(crop, "chars")
        s.dedupe(crop)
    s.page_get(page, "rects")
    s.serialize(page, "toJson")
    s.dispose(doc)
    # await using: close(), then release()
    other = s.open("upstream/pdfplumber-python/tests/pdfs/pdffill-demo.pdf")
    p = s.pages(other)[0]
    tables = s.tables(p, "findTables")
    s.dispose(other)
    s.page_get(p, "chars")
    s.page_get(p, "width")
    for t in tables[:1]:
        s.table_get(t, "cells")
        s.extract(t)
    s.dispose(other)
    return s


def calls_session() -> Session:
    """The members and options the file sessions leave out: every search
    option and error of ``main_group``, crop options and boxes out of the
    page, filter predicates (a truthy value, an error), dedupe options,
    ``extractTextSimple`` options, serializations' options, the document's
    ``toCsv``."""
    s = Session("calls")
    doc = s.open("upstream/pdfplumber-python/tests/pdfs/scotus-transcript-p1.pdf")
    page = s.pages(doc)[0]
    for py, js, options in [
        ("(?P<a>J)(?P<b>U)?", "(?<a>J)(?<b>U)?", {"main_group": "b"}),
        ("(?P<a>J)", "(?<a>J)", {"main_group": "nope"}),
        ("(J)(U)", "(J)(U)", {"main_group": True, "return_groups": False}),
        ("(J)(U)", "(J)(U)", {"main_group": False}),
        ("(J)(x)?", "(J)(x)?", {"main_group": 2}),
        ("(J)", "(J)", {"main_group": -1}),
        ("J.S", "J.S", {"regex": False, "case": False}),
        ("[.]", "[.]", {"regex": False}),
        ("justice", "justice", {"case": False, "return_chars": False, "return_groups": False}),
        ("zzzz", "zzzz", None),
        # a string pattern is compiled with `u` (and `i`): \u{...} escapes
        ("J", r"\u{4A}", None),
        ("j", r"\u{6A}", {"case": False}),
        # a name, for a pattern without named groups: IndexError
        ("(J)", "(J)", {"main_group": "w"}),
    ]:
        s.search(page, py, js, options)
    s.search(page, {"$compiled": "J"}, {"regexp": "J", "flags": "y"}, {"return_chars": False})
    s.search(page, {"$compiled": "J"}, {"regexp": "J", "flags": ""}, {"case": False})
    for bbox, options in [([0, 0, 2000, 2000], None), ([0, 0, 2000, 2000], {"strict": False}),
                          ([-10, -10, 100, 100], {"relative": True}), ([50, 50, 10, 10], None),
                          ([0, 0, 100], None), ([10, 10, 100, 100], {"relative": True, "strict": True})]:
        for name in ("crop", "within_bbox", "outsideBbox"):
            d = s.derive(page, name, bbox, options)
            if d is not None and name == "crop":
                s.text(d, "extractTextSimple", {"x_tolerance": 1})
    for predicate in ("text", "raises"):
        f = s.filter(page, predicate)
        if f is not None:
            s.page_get(f, "chars")
    for options in [{"tolerance": 2.5}, {"extra_attrs": ["fontname"]}, {"tolerance": "x"}, {"extra_attrs": None},
                    {"bogus": 1}]:
        d = s.dedupe(page, "dedupe_chars", options)
        if d is not None:
            s.text(d, "extractText", None)
    for options in [{}, {"x_tolerance": 1, "y_tolerance": 1}, {"keep_blank_chars": True}, {"layout": True}]:
        s.text(page, "extract_text_simple", options)
    for options in [{"include_attrs": ["text", "size"]}, {"exclude_attrs": ["fontname"]}, {"precision": 0},
                    {"object_types": ["char"], "precision": 1, "indent": 1}, {"bogus": True}]:
        s.serialize(page, "toJson", options)
        s.serialize(page, "toCsv", options)
    s.serialize(page, "toDict", None)
    s.serialize(page, "toDict", ["char", "rect", "line", "curve", "image", "annot"])
    s.serialize(doc, "toCsv", {"precision": 2})
    s.serialize(doc, "to_csv")
    s.serialize(doc, "toDict", ["char"])
    for name in ["rectEdges", "curveEdges", "horizontalEdges", "verticalEdges", "structureTree"]:
        s.page_get(page, name)
        s.doc_get(doc, name)
    for name in ["textboxhorizontals", "textlinehorizontals", "curves", "images", "rect_edges", "curve_edges"]:
        s.doc_get(doc, name)
    s.close(page)
    s.close(page)
    s.text(page, "extractText", None)
    s.release(doc)
    return s


#: Files with a structure tree (real, then ``adv-structure``).
STRUCTURE = ["upstream/pdfplumber-python/tests/pdfs/hello_structure.pdf",
             "upstream/pdfplumber-python/tests/pdfs/word365_structure.pdf",
             "upstream/pdfplumber-python/tests/pdfs/figure_structure.pdf",
             "upstream/pdfplumber-python/tests/pdfs/image_structure.pdf",
             "upstream/pdfplumber-python/tests/pdfs/pdf_structure.pdf"]
ADV_STRUCTURE = ["000-basic.pdf", "001-no-struct-tree-root.pdf", "023-root-k-objr-element.pdf",
                 "024-root-k-objr-other-page.pdf", "025-root-k-duplicate.pdf", "026-root-k-two-roots.pdf"]

#: The matchers of the structure sessions (``Session.struct_find``).
MATCHERS: list[tuple[str, Any]] = [("tag", "P"), ("tag", "Span"), ("regexp", "H[0-9]"), ("regexp", "S"), ("regexp", "an"),
                                   ("regexp", ""), ("predicate", "starts-with-H"), ("predicate", "has-mcids"),
                                   ("value", 5)]


def structure_session(name: str, rel: str | None = None, data: bytes | None = None) -> Session:
    """``pdf.structTree(page?)``: the document's tree, a page's, a cropped
    page's; ``findAll``/``find`` with each kind of matcher on the tree and
    on elements; ``elementBbox``, ``allMcids``, ``repr`` of the first
    elements; after ``release()``."""
    s = Session(name)
    doc = s.open(rel, data=data)
    trees = []
    t = s.struct_tree(doc)
    if t is not None:
        trees.append(t)
    page = s.pages(doc)[0]
    t = s.struct_tree(doc, page)
    if t is not None:
        trees.append(t)
    crop = s.derive(page, "crop", [0, 0, 300, 300])
    if crop is not None:
        t = s.struct_tree(doc, crop)
        if t is not None:
            trees.append(t)
    for k, tree in enumerate(trees):
        for matcher in MATCHERS:
            s.struct_find(tree, "findAll" if k % 2 == 0 else "find_all", matcher)
            s.struct_find(tree, "find", matcher)
        for path in [(0,), (0, 0), (0, 1), (1,), (0, 0, 0)]:
            el = s.element(tree, *path)
            s.element_call(el, "allMcids" if len(path) % 2 else "all_mcids")
            s.element_call(el, "repr")
            for name in pick(["altText", "actualText", "pageNumber", "alt_text", "page_number", "type", "mcids"], len(path) + k, 3):
                s.element_get(el, name)
            s.element_bbox(tree, el, "elementBbox" if len(path) % 2 else "element_bbox")
            s.struct_find(el, "findAll", ("predicate", "has-mcids"))
            s.struct_find(el, "find", ("tag", "Span"))
    s.release(doc)
    if trees:
        s.struct_find(trees[0], "find", ("tag", "P"))
        s.element_call(s.element(trees[0], 0), "repr")
    s.struct_tree(doc)
    return s


def streams_session(files: list[tuple[str, bytes]]) -> Session:
    """Streams by reference (``docs/public-api.md``, "PDFStream"): the
    data of an image's stream read as it is when read (``rawdata``, then
    ``getData()`` decodes it: ``rawdata`` is then ``None``), the same stream
    reached from ``objects``, the document's ``images``, after ``close()``
    and after ``release()``."""
    s = Session("streams")
    for name, data in files:
        doc = s.open(data=data)
        page = s.pages(doc)[0]
        images = s.page_get(page, "images")
        for i in range(3):
            st = s.stream(doc, images, i, "stream")
            for member in ("rawdata", "data", "getRawdata", "get_data", "rawdata", "data", "get_rawdata", "getData"):
                s.stream_read(st, member)
        # images given back as explicit table lines: their streams (and
        # colour space names) cross as the objects they stand for
        s.tables(page, "extractTables", {"vertical_strategy": "explicit", "horizontal_strategy": "explicit",
                                         "explicit_vertical_lines": [Ref(images, 0), Ref(images, 1), Ref(images, 2)],
                                         "explicit_horizontal_lines": [Ref(images, 0), Ref(images, 2)]})
        objects = s.page_get(page, "objects")
        s.stream_read(s.stream(doc, objects, "image", 0, "stream"), "data")
        s.stream_read(s.stream(doc, objects, "image", 3, "stream"), "getData")
        every = s.doc_get(doc, "images")
        s.close(doc)
        s.stream_read(s.stream(doc, every, 1, "stream"), "rawdata")
        s.stream_read(s.stream(doc, every, 4, "stream"), "getData")
        s.release(doc)
        s.stream_read(s.stream(doc, every, 0, "stream"), "data")
        s.stream_read(s.stream(doc, images, 0, "stream"), "getData")
    return s


def sessions() -> list[Session]:
    out = [file_session(k, rel) for k, rel in enumerate(FILES)]
    out += [open_session(), path_session(), order_session(), cjk_session()]
    out += [inputs_session(), url_session(), arguments_session(), lifetime_session(), calls_session()]
    adv = cache_dir() / "corpus" / "adv-plumber"
    for name in ADV:
        path = adv / name
        if not path.is_file():
            raise SystemExit(f"js_api: {path} is missing: generate adv-plumber first (python -m oracle.unit plumber)")
        out.append(adversarial_session(name, path.read_bytes()))
    out.append(streams_session([(n, (adv / n).read_bytes()) for n in ("078-image-kinds.pdf", "079-image-inline-many.pdf")]))
    out += [structure_session(f"structure-{Path(rel).stem}", rel) for rel in STRUCTURE]
    adv_structure = cache_dir() / "corpus" / "adv-structure"
    for name in ADV_STRUCTURE:
        path = adv_structure / name
        if not path.is_file():
            raise SystemExit(f"js_api: {path} is missing: generate adv-structure first (python -m oracle.unit structure)")
        out.append(structure_session(f"structure-adv-{Path(name).stem}", data=path.read_bytes()))
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
    # the depth windows of the adversarial sets (Node only: js/test/node/api-depth.test.ts)
    depth = unit_dir(MODULE) / "depth.jsonl"
    tmp = depth.with_name(f".{depth.name}.tmp")
    n_depth = n_depth_calls = 0
    with open(tmp, "w", encoding="ascii") as f:
        for s in depth_sessions():
            out = s.run()
            f.write(json.dumps({"in": [out["session"]], "out": out["expect"]}, ensure_ascii=True, separators=(",", ":")) + "\n")
            n_depth += 1
            n_depth_calls += len(out["expect"])
    tmp.replace(depth)
    write_meta(MODULE, cases={"sessions": n_sessions, "calls": n_calls, "depth_sessions": n_depth, "depth_calls": n_depth_calls})
    print(f"{MODULE}: {n_sessions} sessions, {n_calls} calls; depth windows: {n_depth} sessions, {n_depth_calls} calls")


if __name__ == "__main__":
    main()
