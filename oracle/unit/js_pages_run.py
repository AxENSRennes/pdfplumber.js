"""The statements the JavaScript boundary runs, as a Python script runs them
(``oracle/unit/js_runtime.py``, ``pages.jsonl``).

Each operation of ``crates/wasm/src/pages.rs`` is a statement at the top
level of a script (``crates/wasm/src/ops.rs``, ``ENTRY_DEPTH``): this file
is that script. Run as ``python -I js_pages_run.py IN OUT``, it reads the
cases of one session (``IN``: a JSON list, values in typed JSON) and runs
them in order, in one process, at the top level of the module (the loop
and the ``if`` chain below take no frame), each operation as the boundary
runs it: a property as a load (``obj.objects``: the getter from C the first
time a site runs, ``pycompat::specialize::AttrSite``), a method with
``*args, **kwargs`` (``CALL_FUNCTION_EX``: the bound method from C), an
attribute by name with ``getattr``. ``OUT`` gets, for each case, its result
(the value written by the boundary, handles as ``"#<k>"``: the ``k``-th
object the session made) or its exception, and the depth of the top level
(``depth``: frames, C units left), which ``ENTRY_DEPTH`` must be.

It imports nothing but the standard library and pdfplumber (whose import
interns the symbols the port starts from), and decodes and encodes typed
JSON itself (``oracle/typed_json.py``, without interning literals), so the
process is a pdfplumber user's.

A case is ``[op, target, args, path or bytes, ...]`` (the generator may
append what the replays need): ``target`` the index of
the object the operation applies to (a document, page or table this
session made), ``args`` as ``pages.rs`` takes them, the PDF of a
``doc_open`` as a path or its bytes. Two operations differ from the
boundary's: ``search`` (``[pattern, options]``, the JavaScript pattern
being the same: ``js_runtime.py`` picks patterns that match alike) is one
statement, ``page.search(pattern, **options)``, written with the text it
ran over and the spans of its matches, which the boundary takes in two
calls (``page_search_text``, then ``page_search`` with the spans, and the
spans of the named groups of each match, as JavaScript's ``indices.groups``);
``page_filter`` names a predicate (``PREDICATES``), whose truth on each of
the parent's objects is written with the result: the mask the boundary
takes.
"""

import io
import json
import logging
import struct
import sys

import pdfplumber
from pdfminer.pdftypes import PDFObjRef, PDFStream
from pdfminer.psparser import PSKeyword, PSLiteral
from pdfplumber.page import Page, tuplify_list_kwargs

_SHORT = ("builtins", "pdfminer", "pdfplumber", "oracle")


def decode(j):
    """Typed JSON to a value (no literals: the cases have none)."""
    if j is None or type(j) in (bool, str):
        return j
    if type(j) is list:
        return [decode(x) for x in j]
    (k, v), = j.items()
    if k == "i":
        return int(v)
    if k == "f":
        return struct.unpack(">d", bytes.fromhex(v))[0]
    if k == "b":
        return bytes.fromhex(v)
    if k == "s":
        return "".join(chr(c) for c in v)
    if k == "t":
        return tuple(decode(x) for x in v)
    if k == "d":
        return {decode(a): decode(b) for a, b in v}
    raise ValueError(j)


def _str(s):
    return {"s": [ord(c) for c in s]} if any(0xD800 <= ord(c) <= 0xDFFF for c in s) else s


def encode(v):
    """A value as typed JSON (``oracle/typed_json.py``; a ``Page`` as the
    boundary writes it, ``{"page_number": n}``)."""
    t = type(v)
    if v is None or t is bool:
        return v
    if t is str:
        return _str(v)
    if t is int:
        return {"i": str(v)}
    if t is float:
        return {"f": struct.pack(">d", v).hex()}
    if t is bytes:
        return {"b": v.hex()}
    if t is list:
        return [encode(x) for x in v]
    if t is tuple:
        return {"t": [encode(x) for x in v]}
    if t is dict:
        return {"d": [[encode(k), encode(x)] for k, x in v.items()]}
    if isinstance(v, Page):
        return {"d": [["page_number", encode(v.page_number)]]}
    if t is PSLiteral:
        name = v.name
        return {"lit": _str(name) if type(name) is str else {"b": name.hex()}}
    if t is PSKeyword:
        return {"kw": v.name.hex()}
    if t is PDFObjRef:
        return {"ref": v.objid if type(v.objid) is int else encode(v.objid)}
    if t is PDFStream:
        fields = [("objid", v.objid), ("genno", v.genno), ("attrs", v.attrs), ("rawdata", v.rawdata), ("data", v.data)]
        return {"stream": {k: encode(x) for k, x in fields}}
    if isinstance(v, BaseException):
        module = type(v).__module__ or ""
        name = type(v).__name__ if module.split(".")[0] in _SHORT else f"{module}.{type(v).__name__}"
        return {"exc": name, "msg": _str(str(v))}
    raise TypeError(f"cannot encode {t.__qualname__}")


def _upright(obj):
    return obj.get("upright", True)


def _not_rect(obj):
    return obj["object_type"] != "rect"


def _small(obj):
    return obj.get("size", 0) < 10


#: The predicates of ``page_filter`` (``oracle/stages/plumber_text.py``).
PREDICATES = {"upright": _upright, "not-rect": _not_rect, "small": _small}


def mask_of(page, name):
    """The truth of the predicate on each of the page's objects, by kind,
    and the test function looking it up by identity (``pages.rs``
    ``mask_filter``)."""
    predicate = PREDICATES[name]
    mask = {k: [bool(predicate(o)) for o in v] for k, v in page.objects.items()}
    table = {id(o): t for k, v in page.objects.items() for o, t in zip(v, mask[k])}

    def test(o):
        return table[id(o)]

    return mask, test


def span_of(text, m, g):
    """A group's span as JavaScript gives it (UTF-16 offsets), ``None`` if
    it did not take part."""
    if m.start(g) == -1:
        return None
    u = lambda i: len(text[:i].encode("utf-16-le", "surrogatepass")) // 2  # noqa: E731
    return [u(m.start(g)), u(m.end(g))]


def depth():
    """Frames and C units left here (``oracle/depth.py`` ``depth_here``)."""
    frame, py = sys._getframe(1), 0
    while frame is not None:
        py += 1
        frame = frame.f_back

    def nested(n):
        x = []
        for _ in range(n - 1):
            x = [x]
        return x

    lo, hi = 0, 1 << 15
    while lo < hi:
        mid = (lo + hi + 1) // 2
        try:
            ok = nested(mid) == nested(mid)
        except RecursionError:
            ok = False
        lo, hi = (mid, hi) if ok else (lo, mid - 1)
    return {"py": py, "c_left": lo}


def handle(objs, x):
    objs.append(x)
    return f"#{len(objs) - 1}"


if __name__ == "__main__":
    # the reference configuration: logging disabled (docs/verification.md,
    # "Étapes"), as the boundary runs without onWarning
    logging.disable(logging.CRITICAL)
    with open(sys.argv[1]) as f:
        cases = decode(json.load(f))
    objs = []
    results = []
    top = depth()
    for case in cases:
        op, target, args, data = case[:4]
        obj = None if target is None else objs[target]
        extra = None
        try:
            if op == "doc_open":
                if type(data) is str:
                    with open(data, "rb") as f:
                        data = f.read()
                result = pdfplumber.open(io.BytesIO(data), **(args[0] or {}))
                result = handle(objs, result)
            elif op == "doc_metadata":
                result = obj.metadata
            elif op == "doc_pages":
                pages = obj.pages
                result = []
                for p in pages:
                    d = {"handle": handle(objs, p), "page_number": p.page_number, "initial_doctop": p.initial_doctop,
                         "rotation": p.rotation, "mediabox": p.mediabox, "cropbox": p.cropbox}
                    for name in ("trimbox", "bleedbox", "artbox"):
                        if hasattr(p, name):
                            d[name] = getattr(p, name)
                    d["bbox"] = p.bbox
                    d["width"] = p.width
                    d["height"] = p.height
                    result.append(d)
            elif op == "doc_objects":
                result = obj.objects
            elif op == "doc_attr":
                result = getattr(obj, args[0])
            elif op == "doc_annots":
                result = obj.annots
            elif op == "doc_hyperlinks":
                result = obj.hyperlinks
            elif op == "doc_structure_tree":
                result = obj.structure_tree
            elif op == "doc_to_dict":
                result = obj.to_dict(**args[0])
            elif op == "doc_to_json":
                result = obj.to_json(**args[0])
            elif op == "doc_to_csv":
                result = obj.to_csv(**args[0])
            elif op == "doc_close":
                result = obj.close(*args)
            elif op == "page_info":
                d = {"page_number": obj.page_number, "initial_doctop": obj.initial_doctop, "rotation": obj.rotation,
                     "mediabox": obj.mediabox, "cropbox": obj.cropbox}
                for name in ("trimbox", "bleedbox", "artbox"):
                    if hasattr(obj, name):
                        d[name] = getattr(obj, name)
                d["bbox"] = obj.bbox
                d["width"] = obj.width
                d["height"] = obj.height
                result = d
            elif op == "page_objects":
                result = obj.objects
            elif op == "page_attr":
                result = getattr(obj, args[0])
            elif op == "page_structure_tree":
                result = obj.structure_tree
            elif op == "page_extract_text":
                result = obj.extract_text(**args[0])
            elif op == "page_extract_text_simple":
                result = obj.extract_text_simple(**args[0])
            elif op == "page_extract_words":
                result = obj.extract_words(**args[0])
            elif op == "page_extract_text_lines":
                result = obj.extract_text_lines(*args[:2], **args[2])
            elif op == "search":
                pattern, options = args
                result = obj.search(pattern, **options)
            elif op == "page_crop":
                derived = obj.crop(args[0], **args[1])
                result = {"handle": handle(objs, derived), "page_number": derived.page_number, "bbox": derived.bbox}
            elif op == "page_within_bbox":
                derived = obj.within_bbox(args[0], **args[1])
                result = {"handle": handle(objs, derived), "page_number": derived.page_number, "bbox": derived.bbox}
            elif op == "page_outside_bbox":
                derived = obj.outside_bbox(args[0], **args[1])
                result = {"handle": handle(objs, derived), "page_number": derived.page_number, "bbox": derived.bbox}
            elif op == "page_filter":
                extra, test = mask_of(obj, args[0])
                derived = obj.filter(test)
                result = {"handle": handle(objs, derived), "page_number": derived.page_number, "bbox": derived.bbox}
            elif op == "page_dedupe_chars":
                derived = obj.dedupe_chars(**args[0])
                result = {"handle": handle(objs, derived), "page_number": derived.page_number, "bbox": derived.bbox}
            elif op in ("page_find_tables", "page_find_table"):
                if op == "page_find_tables":
                    tables = obj.find_tables(*args)
                else:
                    found = obj.find_table(*args)
                    tables = [] if found is None else [found]
                result = []
                for t in tables:
                    result.append({"handle": handle(objs, t), "bbox": t.bbox, "cells": t.cells,
                                   "rows": [{"bbox": r.bbox, "cells": r.cells} for r in t.rows],
                                   "columns": [{"bbox": c.bbox, "cells": c.cells} for c in t.columns]})
                if op == "page_find_table":
                    result = result[0] if result else None
            elif op == "page_extract_tables":
                result = obj.extract_tables(*args)
            elif op == "page_extract_table":
                result = obj.extract_table(*args)
            elif op == "table_extract":
                result = obj.extract(**args[0])
            elif op == "page_to_dict":
                result = obj.to_dict(**args[0])
            elif op == "page_to_json":
                result = obj.to_json(**args[0])
            elif op == "page_to_csv":
                result = obj.to_csv(**args[0])
            elif op == "page_close":
                result = obj.close(*args)
            elif op == "free":
                result = None
            else:
                raise AssertionError(op)
        except Exception as e:
            result = e
        if op == "search" and not isinstance(result, Exception):
            # the text the pattern ran over (the text map search found in
            # the cache), and the spans of its matches
            import re

            pattern, options = args
            kw = {k: v for k, v in options.items()
                  if k not in ("regex", "case", "main_group", "return_chars", "return_groups")}
            text = obj.get_textmap(**tuplify_list_kwargs(kw)).as_string
            regex, case = options.get("regex", True), options.get("case", True)
            compiled = re.compile(pattern if regex else re.escape(pattern), re.I if case is False else 0)
            found = list(compiled.finditer(text))
            spans = [[span_of(text, m, g) for g in range(compiled.groups + 1)] for m in found]
            named = [{k: span_of(text, m, k) for k in compiled.groupindex} for m in found] if compiled.groupindex else None
            extra = {"text": text, "spans": spans, "named": named}
        results.append([encode(result), encode(extra)])
    with open(sys.argv[2], "w") as f:
        json.dump({"depth": top, "results": results}, f)
