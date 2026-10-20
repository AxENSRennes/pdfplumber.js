"""Stage ``plumber_doc``: what pdfplumber gives about the document beyond
the page objects: metadata, the structure tree, derived pages and their
objects, serialization, the ``open`` options.

Exercises pdfplumber/pdf.py (``PDF.__init__``: metadata and
``strict_metadata``, ``unicode_norm``, ``raise_unicode_errors``,
``password``, ``pages``; ``PDF.structure_tree``, ``PDF.to_dict``,
``PDF.close``), pdfplumber/structure.py (``PDFStructTree`` for the
document and for a page, with and without a parent tree;
``PDFStructElement`` and ``to_dict``, ``all_mcids``, ``find``,
``find_all``, ``element_bbox``), pdfplumber/page.py (``Page.structure_tree``,
``crop``/``within_bbox``/``outside_bbox`` with ``relative`` and ``strict``,
``filter``, ``dedupe_chars``, ``CroppedPage.objects``, cropped annotations,
``to_dict``, ``__repr__``, ``close``), pdfplumber/container.py (``to_json``,
``to_csv``, ``flush_cache``), pdfplumber/convert.py (``Serializer``,
``get_attr_filter``, ``to_b64``), pdfplumber/utils/pdfinternals.py
(``resolve_and_decode``, ``decode_text``, ``decode_psl_list``).

Not traced: page labels (pdfplumber 0.11.9 has no page-label API;
pdfminer's labels are in the ``pages`` stage), ``to_image`` (``display.py``,
not ported), ``repair`` (Ghostscript, not ported).

Traversal (``open_pdf``, ``exc``, ``select``, ``digest`` of
``plumber_objects``; ``derive``, ``PREDICATES`` of ``plumber_text``)::

    pdf = open_pdf(path, password, max_pages)
    metadata, structure tree of the document (``pdf.structure_tree``, then a
        ``PDFStructTree(pdf)``: ``find_all``, ``find``, ``all_mcids`` and
        ``element_bbox`` of its first elements, ``repr`` of its first one)
    for index, page in enumerate(pdf.pages):
        repr, page.structure_tree and a ``PDFStructTree(pdf, page)``
        for steps in DERIVED_BASE + select(sha, index, DERIVED, DERIVED_PER_PAGE):
            the derived page: bbox, repr, objects (each kind: count and digest),
            annots
        for op in select(sha, index, SERIALIZE, SERIALIZE_PER_PAGE): the output
    pdf.to_dict() (its digest)
    pdf.close() (``flush_cache``: ``pdf.pages`` is built again, with new
        ``Page`` objects, which ``close`` closes), then the objects of the
        first page of ``pdf.pages`` (built again: the file is closed)
    for options in OPEN_OPTIONS (each a new ``open_pdf(path, password,
        max_pages, **options)``): what the option changes (metadata, char
        texts, annotations, page numbers and ``initial_doctop``)
    the password variants

``strict_metadata``: ``PDF.__init__`` resolves and decodes each value of
the document's ``/Info`` dicts (``resolve_and_decode``); without
``strict_metadata`` an exception there is logged and the value kept as
is. ``unicode_norm``: the ``text`` of chars and text containers.
``raise_unicode_errors=False``: annotations whose ``/T``, ``/Contents`` or
``/URI`` decodes neither as UTF-8 nor as UTF-16 keep their bytes (with a
``warnings.warn``) instead of raising ``UnicodeDecodeError``. Passwords:
``password=None`` when a password is given (the document's own may be
needed), and a wrong one.

Structure elements are written as ``to_dict()`` (``dataclasses.asdict``,
which deep-copies the attribute values: a ``PDFObjRef`` copies its
document, whose file object refuses to be copied, ``TypeError``), at most
``MAX_ELEMENTS`` top-level elements per tree in full (then digests).

Lines::

    open <exc>                           pdfplumber.open raised (nothing else follows,
                                         except for OPEN_OPTIONS and passwords, where it
                                         ends that variant)
    metadata <dict | exc>
    structure_tree <n> <digest> | structure_tree <exc>
    struct <i> <dict>                    the first MAX_ELEMENTS elements of the list
    tree <exc>                           PDFStructTree(...) raised
    find_all <matcher> <[type, ...] | exc>   (at most MAX_FOUND types, then "...")
    find <matcher> <type | None | exc>
    mcids <path> <[(page_number, mcid), ...] | exc>
    element_bbox <path> <bbox | exc>     path: the indices from the tree's children
    element_repr <length> <digest> | element_repr <exc>
                                         repr() of the tree's first element: its length
                                         (code points) and the first 16 hex digits of the
                                         SHA-256 of its UTF-8 (surrogatepass) bytes
    pages <exc>                          pdf.pages raised
    page <page_number> <repr>
    page_structure_tree ...              as structure_tree, struct, tree, find_all, ...
    derived <steps>
    derive <exc>
    dbox <bbox> <width> <height> <repr>
    dobjects <[[kind, n, digest], ...]> | dobjects <exc>
    dannots <n> <digest> | dannots <exc>
    serialize <op> <value | exc>         (outd-style ``serialized <op> <length> <digest>``
                                         beyond OUT_LIMIT); in the text of ``to_json`` and
                                         ``to_csv``, each inline image name of the page
                                         (``Names``) is replaced by ``<inline k>``, in a
                                         ``to_dict`` written as in ``plumber_objects``
    pdf_to_dict <digest> | pdf_to_dict <exc>
    close <exc> | close ok
    closed_objects <[[kind, n, digest], ...]> | closed_objects <exc>
    options <dict>                       then, for that opening, lines of its kind:
    metadata ...
    texts <page_number> <[text, ...]>    unicode_norm: the text of each char (a digest
                                         beyond OUT_LIMIT: textsd)
    annots <page_number> <n> <digest> | annots <page_number> <exc>
    selected <[(page_number, initial_doctop), ...]> | selected <exc>
    password <repr> ok | password <repr> <exc>
    !end
"""

from __future__ import annotations

import hashlib
from collections.abc import Iterator
from typing import Any

import pdfplumber
from pdfplumber.page import Page
from pdfplumber.structure import PDFStructTree

from ..canon import canon
from ._common import terminated
from .plumber_objects import Names, annot_value, digest, exc, file_sha256, open_pdf, page_names, select
from .plumber_text import OUT_LIMIT, PREDICATES, derive

PAGED = True
DEPENDS = ("plumber_objects", "plumber_text")

MAX_ELEMENTS = 50
MAX_FOUND = 50
#: Elements whose ``all_mcids``/``element_bbox`` are written: the first
#: ones of the tree, depth-first, by path.
MAX_PROBED = 8

DERIVED_BASE: list[list[list[Any]]] = [[["crop", [0.1, 0.1, 0.9, 0.9], {}]]]

DERIVED: list[list[list[Any]]] = [
    [["crop", [0.0, 0.0, 0.5, 0.5], {}]],
    [["crop", [0.0, 0.0, 0.5, 0.5], {"relative": True}]],
    [["crop", [-0.5, -0.5, 0.5, 0.5], {"strict": False}]],
    [["crop", [-0.5, -0.5, 0.5, 0.5], {}]],
    [["crop", [0.2, 0.2, 0.2, 0.8], {}]],
    [["crop", [1.5, 1.5, 2.0, 2.0], {}]],
    [["crop", [1.5, 1.5, 2.0, 2.0], {"strict": False}]],
    [["within_bbox", [0.0, 0.0, 1.0, 0.5], {}]],
    [["within_bbox", [0.25, 0.25, 0.75, 0.75], {"relative": True, "strict": False}]],
    [["outside_bbox", [0.0, 0.0, 1.0, 0.5], {}]],
    [["outside_bbox", [0.3, 0.3, 0.6, 0.6], {"strict": False}]],
    [["filter", "upright"]],
    [["filter", "not-rect"]],
    [["filter", "small"]],
    [["dedupe_chars", {}]],
    [["dedupe_chars", {"tolerance": 3, "extra_attrs": None}]],
    [["crop", [0.0, 0.0, 1.0, 1.0], {}], ["crop", [0.0, 0.0, 1.0, 1.0], {}]],
    [["crop", [0.1, 0.1, 0.9, 0.9], {}], ["crop", [0.0, 0.0, 0.5, 0.5], {"relative": True}]],
    [["outside_bbox", [0.0, 0.0, 0.5, 0.5], {}], ["within_bbox", [0.0, 0.0, 1.0, 0.5], {}]],
    [["filter", "small"], ["crop", [0.0, 0.0, 1.0, 0.5], {}]],
]

#: ``[method, kwargs]``: ``page.<method>(**kwargs)``.
SERIALIZE: list[list[Any]] = [
    ["to_dict", {}],
    ["to_dict", {"object_types": ["rect", "line"]}],
    ["to_json", {}],
    ["to_json", {"precision": 2}],
    ["to_json", {"object_types": ["char"], "include_attrs": ["text", "x0", "top", "fontname"], "precision": 1}],
    ["to_json", {"object_types": ["rect", "image"], "exclude_attrs": ["stream", "path", "pts"], "indent": 2}],
    ["to_json", {"object_types": ["annot"]}],
    ["to_json", {"include_attrs": ["x0"], "exclude_attrs": ["x1"]}],
    ["to_json", {"object_types": ["char"], "exclude_attrs": ["object_type"]}],
    ["to_csv", {}],
    ["to_csv", {"precision": 3}],
    ["to_csv", {"object_types": ["char"], "include_attrs": ["text", "x0", "matrix", "non_stroking_color"]}],
    ["to_csv", {"object_types": ["line", "rect", "curve"], "exclude_attrs": ["pts", "path"]}],
    ["to_csv", {"object_types": ["annot"]}],
    ["to_csv", {"object_types": ["nothing"]}],
]

DERIVED_PER_PAGE = 3
SERIALIZE_PER_PAGE = 3

#: Each a new opening of the document.
OPEN_OPTIONS: list[dict[str, Any]] = [
    {"strict_metadata": True},
    {"unicode_norm": "NFC"},
    {"unicode_norm": "NFKC"},
    {"unicode_norm": "NFD"},
    {"unicode_norm": "NFKD"},
    {"raise_unicode_errors": False},
    {"pages": [2, 1]},
    {"pages": (3,)},
    {"pages": []},
]

#: The pages whose texts the ``unicode_norm`` openings write.
NORM_PAGES = 2

WRONG_PASSWORD = "\x00not the password"


def bounded(label: str, value: Any) -> str:
    text = canon(value)
    if len(text) > OUT_LIMIT:
        return f"{label}d {len(text)} " + hashlib.sha256(text.encode("ascii")).hexdigest()[:16]
    return f"{label} {text}"


def objects_digest(page: Page) -> list[list[Any]]:
    names = Names()
    objects = page.objects
    names.learn(objects)
    return [[kind, len(objs), digest([names.obj(o) for o in objs])] for kind, objs in objects.items()]


# ------------------------------------------------------------------------------- structure


def _paths(children: list[Any]) -> list[tuple[tuple[int, ...], Any]]:
    """The first MAX_PROBED elements under ``children``, depth-first, with
    their paths (no recursion)."""
    out: list[tuple[tuple[int, ...], Any]] = []
    stack = [((i,), el) for i, el in reversed(list(enumerate(children)))]
    while stack and len(out) < MAX_PROBED:
        path, el = stack.pop()
        out.append((path, el))
        stack.extend(((*path, j), c) for j, c in reversed(list(enumerate(el.children))))
    return out


def _first_type(children: list[Any]) -> Any:
    return children[0].type if children else "P"


def tree_lines(label: str, get_list: Any, make_tree: Any) -> Iterator[str]:
    try:
        elements = get_list()
    except Exception as e:
        yield f"{label} " + exc(e)
    else:
        yield f"{label} {len(elements)} " + digest(elements)
        for i, el in enumerate(elements[:MAX_ELEMENTS]):
            yield bounded(f"struct {i}", el)
    try:
        tree = make_tree()
    except Exception as e:
        yield "tree " + exc(e)
        return
    tag = _first_type(tree.children)
    matchers: list[tuple[Any, Any]] = [
        (["tag", tag], tag),
        (["tag", "Span"], "Span"),
        (["function", "starts-with-H"], lambda el: el.type.startswith("H")),
    ]
    for desc, matcher in matchers:
        try:
            found = []
            for el in tree.find_all(matcher):
                if len(found) == MAX_FOUND:
                    found.append("...")
                    break
                found.append(el.type)
        except Exception as e:
            yield "find_all " + canon(desc) + " " + exc(e)
        else:
            yield "find_all " + canon(desc) + " " + canon(found)
        try:
            el = tree.find(matcher)
        except Exception as e:
            yield "find " + canon(desc) + " " + exc(e)
        else:
            yield "find " + canon(desc) + " " + canon(None if el is None else el.type)
    for path, el in _paths(tree.children):
        try:
            yield "mcids " + canon(path) + " " + canon(list(el.all_mcids()))
        except Exception as e:
            yield "mcids " + canon(path) + " " + exc(e)
        try:
            yield "element_bbox " + canon(path) + " " + canon(tree.element_bbox(el))
        except Exception as e:
            yield "element_bbox " + canon(path) + " " + exc(e)
    if tree.children:
        try:
            text = repr(tree.children[0])
        except Exception as e:
            yield "element_repr " + exc(e)
        else:
            yield f"element_repr {len(text)} " + hashlib.sha256(text.encode("utf-8", "surrogatepass")).hexdigest()[:16]


# ----------------------------------------------------------------------------------- pages


def derived_lines(page: Page, steps: list[list[Any]]) -> Iterator[str]:
    yield "derived " + canon(steps)
    try:
        d = derive(page, steps)
    except Exception as e:
        yield "derive " + exc(e)
        return
    yield "dbox " + " ".join(canon(v) for v in (d.bbox, d.width, d.height, repr(d)))
    try:
        yield "dobjects " + canon(objects_digest(d))
    except Exception as e:
        yield "dobjects " + exc(e)
    try:
        annots = d.annots
    except Exception as e:
        yield "dannots " + exc(e)
    else:
        names = Names()
        yield f"dannots {len(annots)} " + digest([annot_value(a, names) for a in annots])


def page_dict(value: dict[str, Any], names: Names) -> dict[str, Any]:
    """A ``Page.to_dict`` value as written: its object lists through
    ``names`` (the values are those of ``page.objects`` and ``page.annots``)."""
    return {k: ([annot_value(a, names) if k == "annots" else names.obj(a) for a in v] if isinstance(v, list) else v) for k, v in value.items()}


def serialize_lines(page: Page, op: list[Any]) -> Iterator[str]:
    method, kwargs = op
    label = "serialize " + canon(op)
    try:
        value = getattr(page, method)(**kwargs)
    except Exception as e:
        yield label + " " + exc(e)
        return
    names = page_names(page)
    if method == "to_dict":
        value = page_dict(value, names)
    else:
        value = names.text(value)
    text = canon(value)
    if len(text) > OUT_LIMIT:
        yield f"serialized {canon(op)} {len(text)} " + hashlib.sha256(text.encode("ascii")).hexdigest()[:16]
    else:
        yield f"{label} {text}"


def page_lines(pdf: pdfplumber.PDF, page: Page, sha: str, index: int) -> Iterator[str]:
    yield f"page {page.page_number} " + canon(repr(page))
    yield from tree_lines("page_structure_tree", lambda: page.structure_tree, lambda: PDFStructTree(pdf, page))
    for steps in DERIVED_BASE + select(sha, index, DERIVED, DERIVED_PER_PAGE):
        yield from derived_lines(page, steps)
    for op in select(sha, index, SERIALIZE, SERIALIZE_PER_PAGE):
        yield from serialize_lines(page, op)


# --------------------------------------------------------------------------------- options


def option_lines(path: str, password: str | None, max_pages: int, options: dict[str, Any]) -> Iterator[str]:
    yield "options " + canon(options)
    if "pages" in options:
        try:
            pdf = pdfplumber.open(path, password=password, **options)
        except Exception as e:
            yield "open " + exc(e)
            return
        try:
            yield "selected " + canon([(p.page_number, p.initial_doctop) for p in pdf.pages])
        except Exception as e:
            yield "selected " + exc(e)
        return
    try:
        pdf = open_pdf(path, password, max_pages, **options)
    except Exception as e:
        yield "open " + exc(e)
        return
    if "strict_metadata" in options:
        yield "metadata " + canon(pdf.metadata)
        return
    try:
        pages = pdf.pages
    except Exception as e:
        yield "pages " + exc(e)
        return
    if "unicode_norm" in options:
        for page in pages[:NORM_PAGES]:
            try:
                yield bounded(f"texts {page.page_number}", [c["text"] for c in page.chars])
            except Exception as e:
                yield f"texts {page.page_number} " + exc(e)
        return
    for page in pages:
        try:
            annots = page.annots
        except Exception as e:
            yield f"annots {page.page_number} " + exc(e)
        else:
            names = Names()
            yield f"annots {page.page_number} {len(annots)} " + digest([annot_value(a, names) for a in annots])


def password_lines(path: str, password: str | None, max_pages: int) -> Iterator[str]:
    for other in ([None] if password is not None else []) + [WRONG_PASSWORD]:
        try:
            open_pdf(path, other, max_pages)
        except Exception as e:
            yield "password " + canon(other) + " " + exc(e)
        else:
            yield "password " + canon(other) + " ok"


# ----------------------------------------------------------------------------------- stage


@terminated
def run(path: str, password: str | None, max_pages: int) -> Iterator[str]:
    sha = file_sha256(path)
    try:
        pdf = open_pdf(path, password, max_pages)
    except Exception as e:
        yield "open " + exc(e)
        return
    yield "metadata " + canon(pdf.metadata)
    yield from tree_lines("structure_tree", lambda: pdf.structure_tree, lambda: PDFStructTree(pdf))
    try:
        pages = pdf.pages
    except Exception as e:
        yield "pages " + exc(e)
        pages = []
    for index, page in enumerate(pages):
        yield from page_lines(pdf, page, sha, index)
    try:
        value = pdf.to_dict(["char", "rect", "line", "curve", "image"])
        value["pages"] = [page_dict(d, page_names(p)) for p, d in zip(pdf.pages, value["pages"])]
        yield "pdf_to_dict " + digest(value)
    except Exception as e:
        yield "pdf_to_dict " + exc(e)
    try:
        pdf.close()
    except Exception as e:
        yield "close " + exc(e)
    else:
        yield "close ok"
    try:
        reopened = pdf.pages
        if reopened:
            yield "closed_objects " + canon(objects_digest(reopened[0]))
    except Exception as e:
        yield "closed_objects " + exc(e)
    for options in OPEN_OPTIONS:
        yield from option_lines(path, password, max_pages, options)
    yield from password_lines(path, password, max_pages)


__all__ = ["DERIVED", "OPEN_OPTIONS", "SERIALIZE"]
