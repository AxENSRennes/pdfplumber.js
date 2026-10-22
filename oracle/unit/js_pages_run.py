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
(``depth``: frames, C units left), which ``ENTRY_DEPTH`` must be, and how
many objects each case made (``made``).

It imports nothing but the standard library and pdfplumber (whose import
interns the symbols the port starts from), and decodes and encodes typed
JSON itself (``oracle/typed_json.py``, without interning literals), so the
process is a pdfplumber user's.

A case is ``[op, target, args, path or bytes, ...]`` (the generator may
append what the replays need): ``target`` the index of
the object the operation applies to (a document, page or table this
session made), or ``[c, i]``, the ``i``-th object case ``c`` made (the
sessions of the public API, ``oracle/unit/js_api.py``), ``args`` as
``pages.rs`` takes them, the PDF of a ``doc_open`` as a path or its bytes;
``doc_open_file`` (``[path, options]``) opens the path itself
(``pdfplumber.open(path)``: the ``PDF`` owns the file). ``OUT`` also
gets, for each case, the exceptions its exception wraps (``causes``:
``e.args[0]``, recursively), what a JavaScript error has as ``cause``. Two operations differ from the
boundary's: ``search`` (``[pattern, options]``, the JavaScript pattern
being the same: ``js_runtime.py`` picks patterns that match alike) is one
statement, ``page.search(pattern, **options)``, written with the text it
ran over and the spans of its matches, which the boundary takes in two
calls (``page_search_text``, then ``page_search`` with the spans, and the
spans of the named groups of each match, as JavaScript's ``indices.groups``);
``page_filter`` names a predicate (``PREDICATES``), whose truth on each of
the parent's objects is written with the result: the mask the boundary
takes.

The sessions of the API also write, for the harness only (the boundary
never sees them): a target ``{"at": [c, path...]}``, the object a value of
case ``c``'s result names (the page of a ``Page`` value, with the handle the
boundary labelled it with: ``pdf.page(ref)``); an argument ``{"$cross": [c,
path...]}``, the value at that path given back as the API gives it back
(``cross``: an object of a result as an argument); and ``api_page``
(``pdf.page(ref)``: ``page_info`` unless the API holds an object of the
same page, ``args[0]`` the targets of the pages it holds). A value that does not exist makes the
case unresolved, as a target that was not made.
"""

import dataclasses
import io
import json
import logging
import struct
import sys

import pdfplumber
from pdfminer.pdftypes import PDFObjRef, PDFStream
from pdfminer.psparser import KWD, LIT, PSKeyword, PSLiteral
from pdfplumber.page import Page, tuplify_list_kwargs
from pdfplumber.structure import PDFStructElement, PDFStructTree

_SHORT = ("builtins", "pdfminer", "pdfplumber", "oracle")


class _Ref:
    """A ``PDFObjRef`` of an argument, bound to the document of the case's
    target before the statement (``bind_args``), as the boundary binds it."""

    def __init__(self, objid):
        self.objid = objid


def decode(j):
    """Typed JSON to a value; the pdfminer objects of an argument as
    pdfminer makes them (``LIT``, ``KWD``: interned; a ``PDFObjRef``,
    bound by ``bind_args``)."""
    if j is None or type(j) in (bool, str):
        return j
    if type(j) is list:
        return [decode(x) for x in j]
    (k, v), = j.items()
    if k == "lit":
        return LIT(decode(v)) if type(v) is not dict or "b" not in v else LIT(bytes.fromhex(v["b"]))
    if k == "kw":
        return KWD(bytes.fromhex(v))
    if k == "ref":
        return _Ref(v if type(v) is int else decode(v))
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


#: The objects the session made (handles ``#k``), and those it freed.
OBJS = []
FREED = set()
#: Whether results hold their streams by reference (``set_stream_refs``,
#: ``crates/wasm/src/streams.rs``).
STREAM_REFS = [False]


def page_handle(page):
    """The handle the boundary names a ``Page`` value by: the first object
    of the session that is this page and was not freed, else a new one."""
    for k, o in enumerate(OBJS):
        if o is page and k not in FREED:
            return f"#{k}"
    return handle(OBJS, page)


def document_of(obj):
    """The ``PDFDocument`` of a case's target (``None``: none)."""
    for get in (lambda o: o.doc, lambda o: o.pdf.doc, lambda o: o.page.pdf.doc):
        try:
            doc = get(obj)
        except AttributeError:
            continue
        if doc is not None and hasattr(doc, "getobj"):
            return doc
    return None


def bind_args(args, obj):
    """The arguments of a case with the objects they name: ``{"$obj": k}``
    the ``k``-th object of the session (a page, a stream), a ``PDFObjRef``
    bound to the document of the target (``crates/wasm/src/ops.rs``
    ``object_arg``)."""
    def go(v):
        if isinstance(v, _Ref):
            return PDFObjRef(document_of(obj), v.objid)
        if type(v) is list:
            return [go(x) for x in v]
        if type(v) is tuple:
            return tuple(go(x) for x in v)
        if type(v) is dict:
            if set(v) == {"$obj"} and type(v["$obj"]) is int:
                return OBJS[v["$obj"]]
            return {k: go(x) for k, x in v.items()}
        return v
    return go(args)


def stream_handle(stream):
    """The handle the boundary names a stream by (``set_stream_refs``), as
    ``page_handle``."""
    for k, o in enumerate(OBJS):
        if o is stream and k not in FREED:
            return f"#{k}"
    return handle(OBJS, stream)


def encode(v):
    """A value as typed JSON (``oracle/typed_json.py``; a ``Page`` as the
    boundary writes it, its handle and ``page_number``, as a named object
    ``Page``)."""
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
        return {"named": "Page", "d": [["handle", page_handle(v)], ["page_number", encode(v.page_number)]]}
    if t is PSLiteral:
        name = v.name
        return {"lit": _str(name) if type(name) is str else {"b": name.hex()}}
    if t is PSKeyword:
        return {"kw": v.name.hex()}
    if t is PDFObjRef:
        return {"ref": v.objid if type(v.objid) is int else encode(v.objid)}
    if t is PDFStream:
        if STREAM_REFS[0]:
            # by reference: its handle, objid, genno and attrs, in this order
            label = stream_handle(v)
            return {"named": "StreamRef", "d": [["handle", label], ["objid", encode(v.objid)], ["genno", encode(v.genno)],
                                                ["attrs", encode(v.attrs)]]}
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


def _text(obj):
    return obj.get("text", "")


def _raises(obj):
    raise ValueError("boom")


#: The predicates of ``page_filter`` (``oracle/stages/plumber_text.py``;
#: ``text`` and ``raises``: the sessions of the API).
PREDICATES = {"upright": _upright, "not-rect": _not_rect, "small": _small, "text": _text, "raises": _raises}


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


def table_value(objs, t):
    """A table as the boundary gives it (``find_tables``,
    ``debug_tablefinder``): its handle, ``bbox``, ``cells``, ``rows``,
    ``columns``."""
    return {"handle": handle(objs, t), "bbox": t.bbox, "cells": t.cells,
            "rows": [{"bbox": r.bbox, "cells": r.cells} for r in t.rows],
            "columns": [{"bbox": c.bbox, "cells": c.cells} for c in t.columns]}


def preorder(tree):
    """The elements of a structure tree in ``_find_all``'s order (an element
    reached twice listed twice)."""
    out, stack = [], list(reversed(tree.children))
    while stack:
        el = stack.pop()
        out.append(el)
        stack.extend(reversed(el.children))
    return out


def elements_value(children):
    """The elements as the boundary writes them: each dataclass field, in
    order, ``children`` their values (no recursion; an element holding an
    ancestor raises, as the boundary)."""
    names = [f.name for f in dataclasses.fields(PDFStructElement)]
    top = []
    for root in children:
        stack = [(root, list(reversed(root.children)), [])]
        active = {id(root)}
        while stack:
            el, todo, done = stack[-1]
            if todo:
                child = todo.pop()
                if id(child) in active:
                    raise ValueError("Circular reference detected")
                active.add(id(child))
                stack.append((child, list(reversed(child.children)), []))
                continue
            stack.pop()
            active.discard(id(el))
            value = {n: getattr(el, n) for n in names if n != "children"}
            value["children"] = done
            (stack[-1][2] if stack else top).append(value)
    return top


def element_at(tree, path):
    """The element of a path (indices from ``tree.children``)."""
    if not path:
        raise ValueError("an element's path is not empty")
    children, el = tree.children, None
    for i in path:
        if type(i) is not int:
            raise TypeError(f"expected an int, got {type(i).__name__}")
        if i < 0:
            raise IndexError("list index out of range")
        el = children[i]
        children = el.children
    return el


def element_paths(tree):
    """The path of each element of a tree (its first one), by identity."""
    paths = {}
    stack = [([i], el) for i, el in reversed(list(enumerate(tree.children)))]
    while stack:
        path, el = stack.pop()
        paths.setdefault(id(el), path)
        stack.extend(([*path, j], c) for j, c in reversed(list(enumerate(el.children))))
    return paths


#: The predicates of the structure trees' ``find_all``/``find``
#: (``oracle/stages/plumber_doc.py``), on elements.
ELEMENT_PREDICATES = {"starts-with-H": lambda el: el.type.startswith("H"), "has-mcids": lambda el: bool(el.mcids)}


def matcher_of(tree, m):
    """The matcher of a case and the mask the boundary takes (the truth of
    the matcher on each element, ``preorder``): ``{"predicate": name}`` (a
    function), ``{"regexp": source}`` (``re.compile(source)``, whose
    ``match`` decides, as a JavaScript ``RegExp`` at the start of the
    type); anything else as it is (a tag, or a value Python calls)."""
    import re

    if type(m) is dict and set(m) == {"predicate"}:
        f = ELEMENT_PREDICATES[m["predicate"]]
        return f, [bool(f(el)) for el in preorder(tree)]
    if type(m) is dict and set(m) == {"regexp"}:
        pattern = re.compile(m["regexp"])
        return pattern, [pattern.match(el.type) is not None for el in preorder(tree)]
    return m, None


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


# ------------------------------------------------------- the sessions of the API
#
# What the sessions of the public API (``oracle/unit/js_api.py``) need besides
# the operations: values of earlier results given back as arguments, and
# pages named by a value (``pdf.page(ref)``). The harness writes them, the
# boundary never sees them.


class Unresolved(Exception):
    """A value of an earlier result that does not exist (its case raised or
    gave less): the call is not made, as the API makes no call with an
    argument it does not have."""


#: The value of each case (the Python object) and its encoded form.
RAW = []
ENCODED = []


def _at(case, path):
    """The value at ``path`` (indices and keys) in the result of case
    ``case``, and its encoded form."""
    if case >= len(RAW) or isinstance(RAW[case], BaseException):
        raise Unresolved
    raw, enc = RAW[case], ENCODED[case]
    for k in path:
        if isinstance(raw, Page):
            raise Unresolved
        if type(raw) in (list, tuple) and type(k) is int and 0 <= k < len(raw):
            raw = raw[k]
            enc = enc[k] if type(enc) is list else enc["t"][k]
        elif type(raw) is dict and k in raw:
            enc = next(v for ek, v in enc["d"] if decode(ek) == k)
            raw = raw[k]
        else:
            raise Unresolved
    return raw, enc


def _is_index(k):
    """Whether a key is an array index of a JavaScript object (``"7"``),
    which the object puts first, in ascending order."""
    return k.isdigit() and (k == "0" or k[0] != "0") and int(k) < 2**32 - 1


def cross(v):
    """A value of a result given back as an argument: what the API gives
    for it (``docs/js-runtime.md``, "Valeurs"), encoded again as a
    JavaScript argument is: an integral ``float`` a JavaScript safe integer
    crosses as an ``int``, a ``tuple`` as a ``list``, a ``dict`` with its
    integer-like keys first, two surrogates as the astral character they
    make, and a pdfminer object as the plain object of its fields (a
    ``PSLiteral`` as ``{"name": ...}``...) as the wave 5 base encodes it.
    Iterative: values of any depth."""
    out = []
    stack = [(v, out.append)]
    while stack:
        x, put = stack.pop()
        t = type(x)
        if x is None or t in (bool, int, bytes):
            put(x)
        elif t is float:
            safe = x == x and abs(x) <= 2**53 - 1 and x.is_integer() and not (x == 0 and str(x).startswith("-"))
            put(int(x) if safe else x)
        elif t is str:
            put(x.encode("utf-16-le", "surrogatepass").decode("utf-16-le", "surrogatepass"))
        elif t in (list, tuple):
            items = [None] * len(x)
            put(items)
            for i, item in enumerate(x):
                stack.append((item, lambda y, i=i, items=items: items.__setitem__(i, y)))
        elif t is dict or t in (PSLiteral, PSKeyword, PDFObjRef, PDFStream):
            if t is PSLiteral or t is PSKeyword:
                pairs = [("name", x.name)]
            elif t is PDFObjRef:
                pairs = [("objid", x.objid)]
            elif t is PDFStream:
                pairs = [("objid", x.objid), ("genno", x.genno), ("attrs", x.attrs), ("rawdata", x.rawdata),
                         ("data", x.data)]
            else:
                pairs = [(str(k), i) for k, i in x.items()]
            index = sorted((k for k, _ in pairs if _is_index(k)), key=int)
            order = index + [k for k, _ in pairs if not _is_index(k)]
            values = dict(pairs)
            d = {}
            put(d)
            for k in order:
                d[k] = None
                stack.append((values[k], lambda y, k=k, d=d: d.__setitem__(k, y)))
        else:
            # a Page: a PageRef, whose handle the core gives
            raise TypeError(f"cannot cross {t.__qualname__}")
    return out[0]


def substitute(v):
    """``v`` with each ``{"$cross": [case, path...]}`` replaced by the
    crossed value at that path (``cross``)."""
    if type(v) is list:
        return [substitute(x) for x in v]
    if type(v) is tuple:
        return tuple(substitute(x) for x in v)
    if type(v) is dict:
        if len(v) == 1 and "$cross" in v:
            case, *path = v["$cross"]
            return cross(_at(case, path)[0])
        return {k: substitute(x) for k, x in v.items()}
    return v


#: The handle of the page each ``api_page`` case that ran ``page_info``
#: made an object of (the API holds it from then on).
LABELS = {}


def held_by_api(entries, made):
    """The handles of the pages the API holds objects of: ``[c, i]`` the
    ``i``-th object case ``c`` made, ``[c, "*"]`` every object it made,
    ``[c, "label"]`` the page of ``api_page`` case ``c``."""
    out = set()
    for c, i in entries:
        if i == "*":
            out.update(made[c])
        elif i == "label":
            if c in LABELS:
                out.add(LABELS[c])
        elif i < len(made[c]):
            out.add(made[c][i])
    return out


def page_at(case, path):
    """The object a value names and its handle: the page a ``Page`` value
    is and the handle the boundary labelled it with (the ``PageRef``'s),
    else the value and ``None``."""
    raw, enc = _at(case, path)
    if isinstance(raw, Page):
        k = int(dict((decode(a), b) for a, b in enc["d"])["handle"][1:])
        return OBJS[k], k
    return raw, None


if __name__ == "__main__":
    # the reference configuration: logging disabled (docs/verification.md,
    # "Étapes"), as the boundary runs without onWarning
    logging.disable(logging.CRITICAL)
    with open(sys.argv[1]) as f:
        cases = decode(json.load(f))
    objs = OBJS
    results = []
    #: the exception each exception wraps (``e.args[0]``, pdfplumber's
    #: ``PdfminerException(e)``), as ``[[name, message], ...]``: what a
    #: JavaScript error carries as ``cause``
    causes = []
    #: the objects each case made (their indices in ``objs``): a target
    #: ``[c, i]`` is the ``i``-th object case ``c`` made
    made = []
    top = depth()

    def arg_object(a):
        """An object of the session given as an argument: ``{"$obj": k}``
        (``k``: its index, or ``[c, i]``, the ``i``-th object case ``c``
        made), else the value itself."""
        if type(a) is dict and set(a) == {"$obj"}:
            k = a["$obj"]
            return objs[made[k[0]][k[1]]] if type(k) is list else objs[k]
        return a

    for case in cases:
        op, target, args, data = case[:4]
        labelled = None
        try:
            if type(target) is dict:
                # {"at": [case, path...]}: the page a value names (the API's
                # pdf.page(ref)), or the value
                obj, labelled = page_at(target["at"][0], target["at"][1:])
            elif type(target) is list:
                if target[1] >= len(made[target[0]]):
                    raise Unresolved
                obj = objs[made[target[0]][target[1]]]
            elif type(target) is int and target >= len(objs):
                # (a depth window's operation on the object an operation
                # that raised did not make)
                raise Unresolved
            else:
                obj = None if target is None else objs[target]
            args = substitute(args)
            if op == "api_page":
                # pdf.page(ref) of the API (harness only): no statement for
                # a page the API has an object of (args[0]: the targets of
                # the pages it holds), else page_info on it
                # (the API holds an object for this page if it holds one
                # for its handle, or one for the same page of pdf.pages: a
                # Page value names its page by its first live handle, not
                # necessarily the one pdf.pages gave)
                same = sorted(h for h in held_by_api(args[0], made) if objs[h] is obj)
                known = f"#{same[0]}" if same else None
                op = "api_known" if same else "page_info"
                if op == "page_info":
                    LABELS[len(results)] = labelled
        except Unresolved:
            # the object was not made (the case that makes it raised, or
            # made fewer), or a value an argument gives back: nothing runs,
            # as the API makes no call on an object it does not have
            results.append([encode({"$unresolved": True}), None])
            causes.append([])
            made.append([])
            RAW.append(Unresolved())
            ENCODED.append(None)
            continue
        first = len(objs)
        extra = None
        if op != "struct_tree":
            args = bind_args(args, obj)
        try:
            if op == "doc_open":
                if type(data) is str:
                    with open(data, "rb") as f:
                        data = f.read()
                result = pdfplumber.open(io.BytesIO(data), **(args[0] or {}))
                result = handle(objs, result)
            elif op == "doc_open_file":
                # pdfplumber.open(path, **options): the file opened by the PDF
                result = pdfplumber.open(data, **(args[1] or {}))
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
                # width and height each in its own statement: either may
                # raise (a crop box kept as given)
                try:
                    d["width"] = obj.width
                except Exception as e:
                    d["width_error"] = e
                try:
                    d["height"] = obj.height
                except Exception as e:
                    d["height_error"] = e
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
                if type(pattern) is dict:
                    # {"$compiled": source}: a compiled pattern (a RegExp
                    # of the API), compiled before the statement
                    import re

                    pattern = re.compile(pattern["$compiled"])
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
                result = [table_value(objs, t) for t in tables]
                if op == "page_find_table":
                    result = result[0] if result else None
            elif op == "page_debug_tablefinder":
                finder = obj.debug_tablefinder(*args)
                settings = finder.settings
                result = {"edges": finder.edges,
                          "intersections": [[pt, d] for pt, d in finder.intersections.items()],
                          "cells": finder.cells,
                          "tables": [table_value(objs, t) for t in finder.tables],
                          "settings": {f.name: getattr(settings, f.name) for f in dataclasses.fields(settings)}}
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
            elif op == "struct_tree":
                # [page]: a page of the session ({"$obj": k}) or None
                page = arg_object(args[0])
                tree = PDFStructTree(obj, page)
                children = elements_value(tree.children)
                result = {"handle": handle(objs, tree), "children": children}
            elif op in ("struct_find_all", "struct_find"):
                # [path or None, matcher]: on the tree or on an element
                path, m = args
                x = obj if path is None else element_at(obj, path)
                m, extra = matcher_of(obj, m)
                if op == "struct_find_all":
                    found = list(x.find_all(m))
                else:
                    found = x.find(m)
                paths = element_paths(obj)
                if op == "struct_find_all":
                    result = [paths[id(el)] for el in found]
                else:
                    result = None if found is None else paths[id(found)]
            elif op == "struct_element_bbox":
                el = element_at(obj, args[0])
                result = obj.element_bbox(el)
            elif op == "struct_all_mcids":
                el = element_at(obj, args[0])
                result = list(el.all_mcids())
            elif op == "struct_element_repr":
                el = element_at(obj, args[0])
                result = repr(el)
            elif op == "set_stream_refs":
                # the streams of the following results by reference
                STREAM_REFS[0] = bool(args[0])
                result = None
            elif op == "stream_rawdata":
                result = obj.rawdata
            elif op == "stream_data":
                result = obj.data
            elif op == "stream_get_data":
                result = obj.get_data(*args) if args else obj.get_data()
            elif op == "stream_get_rawdata":
                result = obj.get_rawdata(*args) if args else obj.get_rawdata()
            elif op == "free":
                FREED.add(target)
                result = None
            elif op == "api_known":
                result = {"known": known}
            else:
                raise AssertionError(op)
        except Exception as e:
            result = e
        if len(results) in LABELS:
            # api_page: the handle of the page the API made an object of
            extra = {"handle": f"#{LABELS[len(results)]}"}
        if op == "search" and not isinstance(result, Exception):
            # the text the pattern ran over (the text map search found in
            # the cache), and the spans of its matches
            import re

            pattern, options = args
            if type(pattern) is dict:
                pattern = pattern["$compiled"]
            kw = {k: v for k, v in options.items()
                  if k not in ("regex", "case", "main_group", "return_chars", "return_groups")}
            text = obj.get_textmap(**tuplify_list_kwargs(kw)).as_string
            regex, case = options.get("regex", True), options.get("case", True)
            compiled = re.compile(pattern if regex else re.escape(pattern), re.I if case is False else 0)
            found = list(compiled.finditer(text))
            spans = [[span_of(text, m, g) for g in range(compiled.groups + 1)] for m in found]
            named = [{k: span_of(text, m, k) for k in compiled.groupindex} for m in found] if compiled.groupindex else None
            extra = {"text": text, "spans": spans, "named": named}
        # values as deep as the limit allows (a structure tree's to_dict):
        # encoded with room, the limit put back before the next statement
        sys.setrecursionlimit(20000)
        results.append([encode(result), encode(extra)])
        RAW.append(result)
        ENCODED.append(results[-1][0])
        chain = []
        while isinstance(result, BaseException) and len(result.args) == 1 and isinstance(result.args[0], BaseException):
            result = result.args[0]
            chain.append(encode(result))
        causes.append(chain)
        made.append(list(range(first, len(objs))))
        sys.setrecursionlimit(1000)
    sys.setrecursionlimit(20000)
    with open(sys.argv[2], "w") as f:
        json.dump({"depth": top, "results": results, "causes": causes, "made": [len(m) for m in made]}, f)
