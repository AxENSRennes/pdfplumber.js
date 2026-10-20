"""Adversarial PDFs for pdfplumber's document layer: the structure tree
(``pdfplumber/structure.py``), the metadata (``PDF.__init__``,
``resolve_and_decode``, ``strict_metadata``) and the annotations
(``Page.annots``, ``Page.hyperlinks``). Verified by the ``plumber_doc`` and
``plumber_objects`` stages.

    PDFPLUMBER_ROOT=<main checkout> python -m oracle.adversarial.structure
    PDFPLUMBER_ROOT=<main checkout> python -m oracle.adversarial.structure --probe

writes ``<cache>/corpus/adv-structure/`` (same bytes on every run);
``--probe`` searches, by dichotomy over the ``plumber_doc`` stage, the first
failing depth of each window of ``cases_depth`` and prints it (the windows
are written around the values found, noted next to them).

The cases (``tagged`` builds a document with a structure tree, a parent
tree, marked content on its pages):

- the root: ``/StructTreeRoot`` absent, of every type, without ``/K``,
  ``/K`` of every kind (int, dict, list, reference, null, name, nested);
- the elements' ``/K``: MCIDs of every type, MCR and OBJR dicts (to an
  annotation, to an element, on another page), direct dicts, numbers,
  strings, references to missing objects, DAGs, cycles (document scope:
  ``prune`` runs out of frames), elements on pages beyond ``pages=``;
- ``/Pg`` absent, odd, on an unparsed page; ``/RoleMap`` and ``/ClassMap``
  of every type, attribute objects ``/A``/``/C`` in lists with revisions,
  class names missing, values that are references (the deep copy of
  ``asdict`` refuses the file object);
- ``/ID``, ``/T``, ``/Lang``, ``/Alt``, ``/ActualText`` of every type
  (PDFDocEncoding, UTF-16 with BOM, lists of ints, past 255, negative,
  nested: ``decode_text``'s ``str(s)``);
- the parent tree: absent, without the page's ``/StructParents``, nested,
  with nulls, parents that never reach the root or are missing;
- ``/BBox`` attributes and MCIDs found on the page or not
  (``element_bbox``);
- metadata: ``/Info`` values of every type, shared, nested, referring to
  themselves, to the catalog, to a page, to a structure element (the
  in-place decoding is then seen by the readers that follow), two
  ``/Info`` dicts (an incremental update);
- annotations: ``/Rect``, ``/A``, ``/URI``, ``/T``, ``/Contents`` of every
  type, undecodable strings, ``/P``, streams, rotated pages;
- depth windows (``cases_depth``) around the first depth that fails in
  Python for each recursive path: ``to_dict`` (``asdict``), ``repr``,
  ``prune``, the C units of ``str(s)`` and ``repr``, ``resolve_and_decode``.
"""

from __future__ import annotations

import random
import sys
from typing import Any, Callable

from ._common import write_corpus
from ._pdf import Doc, Name, Raw, Ref, classic, flate, standard_encryption

Case = tuple[str, bytes, "str | None"]

FONT = {"Type": Name("Font"), "Subtype": Name("Type1"), "BaseFont": Name("Helvetica")}
BOX = [0, 0, 200, 200]

#: Marked content of a page: MCID 0 (text), MCID 1 (a rectangle), MCID 2
#: (text again), then unmarked text.
CONTENT = (
    b"/P <</MCID 0>> BDC BT /F1 12 Tf 10 150 Td (Hello) Tj ET EMC\n"
    b"/Figure <</MCID 1>> BDC 20 20 50 30 re f EMC\n"
    b"/Span <</MCID 2>> BDC BT /F1 10 Tf 100 100 Td (World) Tj ET EMC\n"
    b"BT /F1 8 Tf 5 5 Td (free) Tj ET\n"
)

#: Object numbers: 1 catalog, 2 pages, 3.. pages (PAGE0 + i), 9 font, 10
#: root, 11.. elements, 100 parent tree, 200 info.
PAGE0 = 3
CONTENT0 = 60
ROOT = 10
PARENT_TREE = 100
INFO = 200
ENCRYPT = 199

MISSING = object()


def tagged(
    elems: dict[int, Any] | None = None,
    root: Any = MISSING,
    root_extra: dict | None = None,
    npages: int = 1,
    page_extra: dict[int, dict] | None = None,
    catalog_extra: dict | None = None,
    parent_tree: Any = MISSING,
    info: Any = MISSING,
    extra: dict[int, Any] | None = None,
    contents: dict[int, bytes] | None = None,
    streams: dict[int, bytes] | None = None,
    struct_root: Any = MISSING,
    trailer_extra: dict | None = None,
    encrypted: bool = False,
) -> bytes:
    """A tagged document. Defaults: one page, the tree ``Document`` (11)
    -> ``P`` (12, MCID 0) and ``Span`` (13, MCIDs 1, 2), a parent tree
    ``{0: [12, 13, 13]}``. ``None`` as a value of ``elems``, ``extra``,
    ``page_extra`` removes it."""
    doc = Doc()
    catalog: dict = {"Type": Name("Catalog"), "Pages": Ref(2)}
    if struct_root is MISSING:
        catalog["StructTreeRoot"] = Ref(ROOT)
    elif struct_root is not None:
        catalog["StructTreeRoot"] = struct_root
    catalog.update(catalog_extra or {})
    doc.add(1, catalog)
    kids = [Ref(PAGE0 + i) for i in range(npages)]
    doc.add(2, {"Type": Name("Pages"), "Kids": kids, "Count": npages})
    for i in range(npages):
        p: dict = {
            "Type": Name("Page"), "Parent": Ref(2), "MediaBox": BOX, "Contents": Ref(CONTENT0 + i),
            "Resources": {"Font": {"F1": Ref(9)}}, "StructParents": i,
        }
        for k, v in ((page_extra or {}).get(i) or {}).items():
            if v is None:
                p.pop(k, None)
            else:
                p[k] = v
        doc.add(PAGE0 + i, p)
        doc.add(CONTENT0 + i, {}, stream=(contents or {}).get(i, CONTENT))
    doc.add(9, FONT)
    if root is MISSING:
        r: dict = {"Type": Name("StructTreeRoot"), "K": [Ref(11)], "ParentTree": Ref(PARENT_TREE)}
        r.update(root_extra or {})
        r = {k: v for k, v in r.items() if v is not None}
        doc.add(ROOT, r, stream=(streams or {}).get(ROOT))
    elif root is not None:
        doc.add(ROOT, root, stream=(streams or {}).get(ROOT))
    base: dict[int, Any] = {
        11: {"Type": Name("StructElem"), "S": Name("Document"), "P": Ref(ROOT), "K": [Ref(12), Ref(13)]},
        12: {"Type": Name("StructElem"), "S": Name("P"), "P": Ref(11), "Pg": Ref(PAGE0), "K": 0},
        13: {"Type": Name("StructElem"), "S": Name("Span"), "P": Ref(11), "Pg": Ref(PAGE0), "K": [1, 2]},
    }
    base.update(elems or {})
    for num, value in sorted(base.items()):
        if value is None:
            continue
        if isinstance(value, Raw):
            doc.add_raw(num, b"%d 0 obj\n" % num + bytes(value) + b"\nendobj\n")
        else:
            doc.add(num, value, stream=(streams or {}).get(num))
    if parent_tree is MISSING:
        doc.add(PARENT_TREE, {"Nums": [0, [Ref(12), Ref(13), Ref(13)]]})
    elif parent_tree is not None:
        doc.add(PARENT_TREE, parent_tree)
    for num, value in sorted((extra or {}).items()):
        if value is None:
            continue
        if isinstance(value, Raw):
            doc.add_raw(num, b"%d 0 obj\n" % num + bytes(value) + b"\nendobj\n")
        else:
            doc.add(num, value, stream=(streams or {}).get(num))
    trailer: dict = {"Root": Ref(1)}
    if info is not MISSING:
        if isinstance(info, Ref):
            trailer["Info"] = info
        else:
            doc.add(INFO, info)
            trailer["Info"] = Ref(INFO)
    trailer.update(trailer_extra or {})
    enc = None
    if encrypted:
        # RC4, empty user password (the stages open it without one)
        docid = b"0123456789abcdef"
        enc = standard_encryption(random.Random(1), r=3, user="", owner="o", docid=docid)
        doc.add(ENCRYPT, dict(enc.param), encrypt=False)
        trailer["Encrypt"] = Ref(ENCRYPT)
        trailer["ID"] = [docid, docid]
    return classic(doc, trailer, enc)


def elem(s: Any, parent: int, k: Any = MISSING, **attrs: Any) -> dict:
    """A structure element dict (``/Pg`` the first page unless given)."""
    d: dict = {"Type": Name("StructElem"), "S": s if not isinstance(s, str) or isinstance(s, Name) else Name(s),
               "P": Ref(parent)}
    d["Pg"] = Ref(PAGE0)
    if k is not MISSING:
        d["K"] = k
    for key, v in attrs.items():
        if v is None:
            d.pop(key, None)
        else:
            d[key] = v
    return d


# ------------------------------------------------------------------------------------- root


def cases_root() -> list[Case]:
    out: list[Case] = [("basic", tagged(), None)]
    out.append(("no-struct-tree-root", tagged(struct_root=None), None))
    for label, value in [("int", 5), ("null", None), ("name", Name("S")), ("list", [Ref(11)]), ("string", b"root"),
                         ("missing-object", Ref(77)), ("direct-dict", {"K": [Ref(11)]})]:
        if value is None:
            data = tagged(struct_root=Raw(b"null"))
        else:
            data = tagged(struct_root=value)
        out.append((f"root-{label}", data, None))
    out.append(("root-stream", tagged(root={"Type": Name("StructTreeRoot"), "K": [Ref(11)], "ParentTree": Ref(PARENT_TREE)},
                                      streams={ROOT: b"q Q"}), None))
    out.append(("root-without-k", tagged(root_extra={"K": None}), None))
    for label, k in [("int", 0), ("dict", Ref(11)), ("direct-dict", {"S": Name("P"), "K": 0, "Pg": Ref(PAGE0)}),
                     ("ref-to-list", Ref(14)), ("empty", []), ("null", Raw(b"null")), ("name", Name("K")),
                     ("string", b"ab"), ("nested", [[Ref(11)]]), ("float", 1.5), ("true", True),
                     ("mcr", [{"Type": Name("MCR"), "MCID": 0, "Pg": Ref(PAGE0)}]),
                     ("objr-element", [{"Type": Name("OBJR"), "Obj": Ref(11), "Pg": Ref(PAGE0)}]),
                     ("objr-other-page", [{"Type": Name("OBJR"), "Obj": Ref(11), "Pg": Ref(77)}]),
                     ("duplicate", [Ref(11), Ref(11)]), ("two-roots", [Ref(12), Ref(13)]),
                     ("ref-to-ref", Ref(15)), ("missing", [Ref(77), Ref(11)])]:
        out.append((f"root-k-{label}", tagged(root_extra={"K": k}, extra={14: [Ref(11)], 15: Ref(14)}), None))
    out.append(("root-k-stream-ref", tagged(root_extra={"K": Ref(16)}, extra={16: {"Length": 3}}, streams={16: b"abc"}), None))
    out.append(("root-type-missing", tagged(root_extra={"Type": None}), None))
    for label, value in [("ref", Ref(17)), ("list", [Name("A")]), ("int", 3), ("stream", Ref(16))]:
        out.append((f"rolemap-{label}", tagged(root_extra={"RoleMap": value}, extra={16: {"P": Name("Q")}, 17: {"P": Name("Para")}},
                                               streams={16: b"x"}), None))
        out.append((f"classmap-{label}", tagged(root_extra={"ClassMap": value},
                                                elems={12: elem("P", 11, 0, C=Name("P"))},
                                                extra={16: {"P": {"O": Name("Layout")}}, 17: {"P": {"O": Name("List")}}},
                                                streams={16: b"x"}), None))
    out.append(("rolemap-chain", tagged(root_extra={"RoleMap": {"P": Name("Q"), "Q": Name("R"), "Span": Name("Span")}}), None))
    out.append(("rolemap-value-string", tagged(root_extra={"RoleMap": {"P": b"Q"}}), None))
    out.append(("rolemap-value-ref", tagged(root_extra={"RoleMap": {"P": Ref(18)}}, extra={18: Name("Q")}), None))
    out.append(("rolemap-value-keyword", tagged(root_extra={"RoleMap": {"P": Raw(b"Qkw")}}), None))
    out.append(("rolemap-non-ascii", tagged(root_extra={"RoleMap": {"P": Name("élément")}}), None))
    return out


# ---------------------------------------------------------------------------------- children


def cases_children() -> list[Case]:
    out: list[Case] = []

    def k_case(label: str, k: Any, extra: dict | None = None, **kw: Any) -> None:
        out.append((f"k-{label}", tagged(elems={13: elem("Span", 11, k)}, extra=extra, **kw), None))

    for label, k in [("int", 1), ("zero", 0), ("negative", -1), ("bool", True), ("float", 1.0), ("huge", 10**20),
                     ("list-floats", [1.5, 2.0]), ("list-strings", [b"1"]), ("list-names", [Name("x")]),
                     ("list-nested", [[1], 2]), ("list-null", [None, 1]), ("string", b"12"), ("name", Name("x")),
                     ("null", Raw(b"null")), ("empty", []), ("direct-element", [{"S": Name("Q"), "K": 1}]),
                     ("direct-element-only", {"S": Name("Q"), "K": 1}), ("direct-dict-no-s", {"K": 1}),
                     ("ref-int", [Ref(20)]), ("ref-list", Ref(21)), ("ref-missing", [Ref(77), 1]),
                     ("ref-chain", [Ref(22)]), ("ref-dict-mcr", [Ref(23)])]:
        k_case(label, k, extra={20: 2, 21: [1, Ref(20)], 22: Ref(20), 23: {"Type": Name("MCR"), "MCID": 2, "Pg": Ref(PAGE0)}})
    for label, mcid in [("int", 1), ("float", 1.0), ("float-half", 1.5), ("string", b"1"), ("name", Name("m")),
                        ("list", [1]), ("nested", [[1]]), ("bool", True), ("null", Raw(b"null")), ("ref", Ref(20)),
                        ("negative", -3), ("huge", 10**30), ("dict", {"a": 1})]:
        mcr = {"Type": Name("MCR"), "MCID": mcid, "Pg": Ref(PAGE0)}
        k_case(f"mcr-{label}", [mcr, 2], extra={20: 1})
    k_case("mcr-no-pg", [{"Type": Name("MCR"), "MCID": 1}, 2])
    k_case("mcr-pg-other", [{"Type": Name("MCR"), "MCID": 1, "Pg": Ref(77)}, 2])
    k_case("mcr-pg-int", [{"Type": Name("MCR"), "MCID": 1, "Pg": 3}, 2])
    k_case("mcr-pg-dict", [{"Type": Name("MCR"), "MCID": 1, "Pg": {"Type": Name("Page")}}, 2])
    k_case("mcr-pg-string", [{"Type": Name("MCR"), "MCID": 1, "Pg": b"3"}, 2])
    k_case("mcr-pg-stream", [{"Type": Name("MCR"), "MCID": 1, "Pg": Ref(24)}, 2], extra={24: {"Length": 1}}, streams={24: b"x"})
    k_case("mcr-ref-on-page", [Ref(23), 1])
    # OBJR: to an annotation of the page, to an element, to a missing object
    annot = {"Type": Name("Annot"), "Subtype": Name("Link"), "Rect": [10, 10, 50, 30], "P": Ref(PAGE0),
             "A": {"S": Name("URI"), "URI": b"http://example.com/"}, "StructParent": 1}
    for label, obj in [("annot", Ref(30)), ("element", Ref(12)), ("missing", Ref(77)), ("int", 5), ("direct", {"S": Name("Q")})]:
        objr = {"Type": Name("OBJR"), "Obj": obj, "Pg": Ref(PAGE0)}
        out.append((f"objr-{label}", tagged(elems={13: elem("Link", 11, [objr, 1])}, extra={30: annot},
                                            page_extra={0: {"Annots": [Ref(30)]}}), None))
    objr = {"Type": Name("OBJR"), "Obj": Ref(30), "Pg": Ref(77)}
    out.append(("objr-other-page", tagged(elems={13: elem("Link", 11, [objr, 1])}, extra={30: annot}), None))
    out.append(("objr-top-level", tagged(root_extra={"K": [{"Type": Name("OBJR"), "Obj": Ref(11), "Pg": Ref(PAGE0)}]}), None))
    out.append(("objr-ref-dict", tagged(elems={13: elem("Link", 11, [Ref(31)])},
                                        extra={30: annot, 31: {"Type": Name("OBJR"), "Obj": Ref(12)}}), None))
    # DAGs and repeats
    out.append(("dag-shared", tagged(elems={11: elem("Document", ROOT, [Ref(12), Ref(13)]), 12: elem("P", 11, [Ref(14)]),
                                            13: elem("Div", 11, [Ref(14)]), 14: elem("Span", 12, [0, 1])}), None))
    out.append(("dag-repeat", tagged(elems={11: elem("Document", ROOT, [Ref(12), Ref(12), Ref(13)])}), None))
    out.append(("dag-shared-empty", tagged(elems={11: elem("Document", ROOT, [Ref(12), Ref(13)]), 12: elem("P", 11, [Ref(14), 0]),
                                                  13: elem("Div", 11, [Ref(14), 1]), 14: elem("Span", 12, [])}), None))
    # chains of DAG levels: each level reached twice. _resolve_children appends
    # the children of an element each time it reaches it: level i gets
    # 2**(i + 2) children, 2 * 2**(2 + 3 + ... + (n + 1)) paths to the
    # leaf for n levels (4 here: 1024)
    dag: dict[int, Any] = {11: elem("Document", ROOT, [Ref(20), Ref(20)])}
    for i in range(4):
        dag[20 + i] = elem(f"L{i}", 11, [Ref(21 + i), Ref(21 + i)] if i < 3 else [0])
    out.append(("dag-doubling", tagged(elems=dag), None))
    # cycles: the document scope runs out of frames in prune, the page scope
    # without a parent tree too. Through a page's parent tree, Python's
    # _resolve_children loops for ever (the port raises MemoryError once
    # the walk only repeats itself): the pages here have no
    # /StructParents, so their trees stop before.
    no_parents = {0: {"StructParents": None}}
    out.append(("cycle-self", tagged(elems={13: elem("Span", 11, [1, Ref(13)])}, page_extra=no_parents), None))
    out.append(("cycle-parent", tagged(elems={13: elem("Span", 11, [1, Ref(11)])}, page_extra=no_parents), None))
    out.append(("cycle-objr", tagged(elems={13: elem("Span", 11, [1, {"Type": Name("OBJR"), "Obj": Ref(11)}])},
                                     page_extra=no_parents), None))
    out.append(("cycle-self-no-parent-tree", tagged(elems={13: elem("Span", 11, [1, Ref(13)])}, root_extra={"ParentTree": None}), None))
    # elements without content are pruned; elements on unparsed pages
    out.append(("empty-element", tagged(elems={11: elem("Document", ROOT, [Ref(12), Ref(13), Ref(14)]), 14: elem("Empty", 11, [])}), None))
    out.append(("empty-subtree", tagged(elems={11: elem("Document", ROOT, [Ref(12), Ref(14)]), 14: elem("Div", 11, [Ref(15)]),
                                               15: elem("Span", 14)}), None))
    out.append(("pg-missing", tagged(elems={12: elem("P", 11, 0, Pg=None)}), None))
    out.append(("pg-int", tagged(elems={12: elem("P", 11, 0, Pg=3)}), None))
    out.append(("pg-other-object", tagged(elems={12: elem("P", 11, 0, Pg=Ref(9))}), None))
    out.append(("pg-missing-object", tagged(elems={12: elem("P", 11, 0, Pg=Ref(77))}), None))
    out.append(("pg-direct-dict", tagged(elems={12: elem("P", 11, 0, Pg={"Type": Name("Page")})}), None))
    out.append(("pg-null", tagged(elems={12: elem("P", 11, 0, Pg=Raw(b"null"))}), None))
    # 22 pages: the stage opens pages 1 to 20; elements on pages 21, 22
    many = {11: elem("Document", ROOT, [Ref(12), Ref(13), Ref(14)]), 14: elem("P", 11, 0, Pg=Ref(PAGE0 + 21))}
    out.append(("pg-beyond-pages", tagged(npages=22, elems=many), None))
    many = {11: elem("Document", ROOT, [Ref(12), Ref(14)]), 14: elem("Div", 11, [Ref(15)], Pg=None),
            15: elem("P", 14, [{"Type": Name("MCR"), "MCID": 0, "Pg": Ref(PAGE0 + 20)}, {"Type": Name("MCR"), "MCID": 1, "Pg": Ref(PAGE0)}])}
    out.append(("mcr-beyond-pages", tagged(npages=22, elems=many), None))
    # elements and MCIDs on several pages, MCIDs duplicated or absent
    two = {11: elem("Document", ROOT, [Ref(12), Ref(13), Ref(14)]), 14: elem("P", 11, [0, 2], Pg=Ref(PAGE0 + 1))}
    out.append(("two-pages", tagged(npages=2, elems=two,
                                    parent_tree={"Nums": [0, [Ref(12), Ref(13), Ref(13)], 1, [Ref(14), None, Ref(14)]]}), None))
    out.append(("mcid-duplicated", tagged(elems={12: elem("P", 11, [0, 0, 1]), 13: elem("Span", 11, [1, 2])}), None))
    out.append(("mcid-absent", tagged(elems={12: elem("P", 11, [7]), 13: elem("Span", 11, [8, 9])}), None))
    out.append(("mcid-unmarked-content", tagged(contents={0: b"BT /F1 12 Tf 10 150 Td (Hello) Tj ET"}), None))
    out.append(("element-not-dict", tagged(elems={13: Raw(b"[1 2 3]")}), None))
    out.append(("element-int", tagged(elems={13: 7}), None))
    # an object that fails to parse is read again after pdf.close(): the
    # closed file raises (closed_objects)
    out.append(("closed-file-reread", tagged(extra={9: Raw(b"<< /Type /Font /Subtype /Type1 /BaseFont >>")}), None))
    out.append(("element-stream", tagged(elems={13: {"S": Name("Span"), "K": [1, 2], "P": Ref(11), "Pg": Ref(PAGE0)}},
                                         streams={13: b"stream"}), None))
    out.append(("element-mcr-direct", tagged(elems={13: {"Type": Name("MCR"), "MCID": 1, "Pg": Ref(PAGE0)}}), None))
    out.append(("element-objr-direct", tagged(elems={13: {"Type": Name("OBJR"), "Obj": Ref(12)}}), None))
    out.append(("element-s-missing", tagged(elems={13: {"P": Ref(11), "K": [1, 2], "Pg": Ref(PAGE0)}}), None))
    for label, s in [("string", b"Span"), ("int", 3), ("ref", Ref(25)), ("keyword", Raw(b"Spankw")), ("utf8-name", Name("été")),
                     ("hex-name", Raw(b"/Sp#61n")), ("bytes-name", Raw(b"/S#ffpan"))]:
        out.append((f"s-{label}", tagged(elems={13: elem(s, 11, [1, 2])}, extra={25: Name("Span")}), None))
    return out


# -------------------------------------------------------------------------------- attributes


def cases_attributes() -> list[Case]:
    out: list[Case] = []

    def a_case(label: str, extra: dict | None = None, root_extra: dict | None = None, **attrs: Any) -> None:
        out.append((f"attr-{label}", tagged(elems={12: elem("P", 11, 0, **attrs)}, extra=extra, root_extra=root_extra), None))

    a_case("dict", A={"O": Name("Layout"), "Placement": Name("Block"), "Width": 100})
    a_case("list", A=[{"O": Name("Layout"), "Width": 1}, {"O": Name("List"), "ListNumbering": Name("Decimal")}])
    for r in (0, 1, 2, 1.0, True, None):
        label = "none" if r is None else repr(r).lower()
        a_case(f"revision-{label}", R=r, A=[{"O": Name("Layout"), "Width": 1}, 0, {"O": Name("Layout"), "Width": 2}, 1, {"Width": 3}])
    a_case("revision-list", R=[1], A=[{"Width": 1}, 1])
    a_case("revision-name", R=Name("R"), A=[{"Width": 1}, 1])
    a_case("revision-empty-list", R=[], A=[{"Width": 1}])
    a_case("revision-empty-dict", R={}, A=[{"Width": 1}])
    a_case("int-first", A=[0, {"Width": 1}])
    a_case("null-in-list", A=[None, {"Width": 1}, Ref(77)])
    a_case("ref", A=Ref(40), extra={40: {"Width": 9, "BBox": [10, 10, 60, 40]}})
    a_case("ref-in-list", A=[Ref(40), Ref(41)], extra={40: {"Width": 9}, 41: [{"Height": 1}]})
    a_case("value-ref", A={"Width": Ref(40)}, extra={40: 7})
    a_case("value-ref-nested", A={"Widths": [1, [Ref(40)]]}, extra={40: 7})
    a_case("value-literal", A={"Placement": Name("Block"), "L": [Name("a"), {"b": Name("c")}]})
    a_case("value-string", A={"T": b"\xfe\xff\x00h\x00i"})
    # a stream among the attribute values: the metadata decoding resolved
    # the reference in place (/Info refers to the attribute dict); the deep
    # copy of to_dict copies the stream, its attributes (a reference there:
    # the document's file, TypeError) and the security handler of an
    # encrypted document (its decipher method)
    for label, encrypted, length, items in (("plain", False, 1, [Name("n"), [1]]), ("encrypted", True, 1, [Name("n"), [1]]),
                                            ("ref-length", False, Ref(73), [Name("n"), [1]]),
                                            ("ref-in-list", False, 1, [Name("n"), [Ref(73)]])):
        out.append((f"attr-stream-via-info-{label}", tagged(
            elems={12: elem("P", 11, 0, A=Ref(70))}, info={"X": Ref(70)},
            extra={70: {"O": Name("Layout"), "S": Ref(71)}, 71: {"Length": length, "L": items}, 73: 1},
            streams={71: b"x"}, encrypted=encrypted), None))
    a_case("not-dict-int", A=5)
    a_case("not-dict-string", A=b"x")
    a_case("not-dict-stream", A=Ref(42), extra={42: {"Length": 1}})
    a_case("class", C=Name("Cl"), root_extra={"ClassMap": {"Cl": {"O": Name("Layout"), "Width": 5}}})
    a_case("class-missing", C=Name("Nope"), root_extra={"ClassMap": {"Cl": {"Width": 5}}})
    a_case("class-and-a", C=[Name("Cl"), Name("Cl2")], A={"Width": 6},
           root_extra={"ClassMap": {"Cl": {"Width": 5, "Height": 1}, "Cl2": {"Height": 2}}})
    a_case("class-ref", C=Name("Cl"), root_extra={"ClassMap": {"Cl": Ref(43)}}, extra={43: {"Width": 5}})
    a_case("class-value-list", C=Name("Cl"), root_extra={"ClassMap": {"Cl": [{"Width": 5}]}})
    a_case("class-revisions", C=[Name("Cl"), 1, Name("Cl2"), 0], R=1, root_extra={"ClassMap": {"Cl": {"W": 1}, "Cl2": {"W": 2}}})
    a_case("class-no-classmap", C=Name("Cl"))
    a_case("class-classmap-int", C=Name("Cl"), root_extra={"ClassMap": 3})
    a_case("class-non-ascii", C=Name("é"), root_extra={"ClassMap": {"é": {"Width": 5}}})
    for label, bbox in [("ints", [10, 20, 60, 50]), ("floats", [10.5, 20.25, 60.0, 50.0]), ("swapped", [60, 50, 10, 20]),
                        ("short", [1, 2, 3]), ("names", [Name("a"), 1, 2, 3]), ("name", Name("Box")), ("string", b"abcd"),
                        ("empty-string", b""), ("ref", Ref(44)), ("bools", [False, False, True, True]), ("int", 5),
                        ("outside", [500, 500, 600, 600])]:
        a_case(f"bbox-{label}", A={"O": Name("Layout"), "BBox": bbox}, extra={44: [0, 0, 100, 100]})
    return out


# ------------------------------------------------------------------------------------- texts


TEXTS: list[tuple[str, Any]] = [
    ("ascii", b"hello"),
    ("pdfdoc", bytes(range(0x10, 0x40)) + bytes(range(0x7f, 0xb0))),
    ("pdfdoc-high", bytes(range(0xb0, 0x100))),
    ("utf16", b"\xfe\xff\x00h\x00\xe9\xd8=\xde\x00"),
    ("utf16-odd", b"\xfe\xff\x00h\x00"),
    ("utf16-lone-surrogate", b"\xfe\xff\xd8\x00\x00a\xdc\x00"),
    ("utf16-bom-only", b"\xfe\xff"),
    ("utf16le-bom", b"\xff\xfeh\x00i\x00"),
    ("empty", b""),
    ("name", Name("Nm")),
    ("int", 7),
    ("float", 1.5),
    ("bool", True),
    ("null", Raw(b"null")),
    ("list-ints", [104, 105, 0x80]),
    ("list-past-255", [104, 300]),
    ("list-negative", [-1, -256]),
    ("list-very-negative", [104, -257]),
    ("list-huge", [104, 10**20]),
    ("list-bools", [True, False]),
    ("list-strings", [b"h"]),
    ("list-floats", [1.0]),
    ("list-nested", [[104]]),
    ("list-past-255-nested", [300, [1, [2]]]),
    ("list-past-255-names", [256, Name("n"), b"s", {"k": 1}]),
    ("list-empty", []),
    ("dict-short-keys", {"a": 1, "b": 2}),
    ("dict-long-keys", {"ab": 1}),
    ("dict-empty", {}),
    ("ref-stream", Ref(45)),
    ("ref-list", Ref(46)),
    ("ref-missing", Ref(77)),
]


def cases_texts() -> list[Case]:
    out: list[Case] = []
    for key in ("ID", "T", "Lang", "Alt", "ActualText"):
        for label, value in TEXTS:
            if key != "ActualText" and label not in ("pdfdoc", "utf16", "list-past-255", "name", "int", "list-strings",
                                                       "dict-short-keys", "ref-stream", "null"):
                continue
            out.append((f"text-{key.lower()}-{label}",
                        tagged(elems={12: elem("P", 11, 0, **{key: value})}, extra={45: {"Length": 1}, 46: [104, 105]},
                               streams={45: b"x"}), None))
    return out


# ------------------------------------------------------------------------------- parent tree


def cases_parent_tree() -> list[Case]:
    out: list[Case] = []

    def pt(label: str, tree: Any = MISSING, **kw: Any) -> None:
        out.append((f"parent-tree-{label}", tagged(parent_tree=tree, **kw), None))

    pt("missing", None, root_extra={"ParentTree": None})
    pt("ref-missing", None)
    pt("no-struct-parents", page_extra={0: {"StructParents": None}})
    pt("struct-parents-other", page_extra={0: {"StructParents": 5}})
    pt("struct-parents-float", page_extra={0: {"StructParents": 0.0}})
    pt("struct-parents-bool", page_extra={0: {"StructParents": False}})
    pt("struct-parents-ref", page_extra={0: {"StructParents": Ref(47)}}, extra={47: 0})
    pt("struct-parents-name", page_extra={0: {"StructParents": Name("x")}})
    pt("kids", {"Kids": [Ref(48), Ref(49)]}, extra={48: {"Nums": [5, [Ref(12)]], "Limits": [5, 5]},
                                                    49: {"Nums": [0, [Ref(12), Ref(13)]], "Limits": [0, 0]}})
    pt("kids-loop", {"Kids": [Ref(PARENT_TREE)]})
    pt("nums-unsorted", {"Nums": [3, [Ref(13)], 0, [Ref(12), Ref(13)]]})
    pt("nums-duplicate", {"Nums": [0, [Ref(13)], 0, [Ref(12)]]})
    pt("nums-key-string", {"Nums": [b"0", [Ref(13)], 0, [Ref(12)]]})
    pt("nums-odd", {"Nums": [0, [Ref(12)], 1]})
    pt("value-ref", {"Nums": [0, Ref(50)]}, extra={50: [Ref(12), Ref(13)]})
    pt("value-dict", {"Nums": [0, Ref(13)]})
    pt("value-int", {"Nums": [0, 5]})
    pt("value-null", {"Nums": [0, None]})
    pt("value-empty", {"Nums": [0, []]})
    pt("value-nested", {"Nums": [0, [[Ref(12)]]]})
    pt("value-with-null", {"Nums": [0, [Ref(12), None, Ref(13)]]})
    pt("value-with-int", {"Nums": [0, [Ref(12), 3]]})
    pt("value-repeated", {"Nums": [0, [Ref(12), Ref(12), Ref(13), Ref(12)]]})
    pt("value-root", {"Nums": [0, [Ref(ROOT)]]})
    pt("value-mcr", {"Nums": [0, [Ref(51)]]}, extra={51: {"Type": Name("MCR"), "MCID": 0, "Pg": Ref(PAGE0)}})
    pt("not-dict", Raw(b"[0 [12 0 R]]"))
    # parents
    pt("parent-missing", elems={12: elem("P", 11, 0, P=None)})
    pt("parent-not-root", elems={11: elem("Document", 52, [Ref(12), Ref(13)])}, extra={52: {"S": Name("X"), "P": Ref(53)}, 53: {"S": Name("Y")}})
    pt("parent-loop", elems={11: elem("Document", 12, [Ref(12), Ref(13)]), 12: elem("P", 11, 0)})
    pt("parent-root-type-string", root_extra={"Type": b"StructTreeRoot"})
    pt("parent-root-type-other", root_extra={"Type": Name("Other")})
    pt("parent-direct", elems={12: elem("P", 11, 0, P={"Type": Name("StructTreeRoot")})})
    pt("parent-int", elems={12: elem("P", 11, 0, P=4)})
    # a cycle among the elements the parent tree reaches, and an element
    # whose /K is a stream: _resolve_children reaches the stream (its
    # PDFKeyError) before it could loop for ever on the cycle
    pt("cycle-then-error", {"Nums": [0, [Ref(12), Ref(15)]]},
       elems={11: elem("Document", ROOT, [Ref(12), Ref(15)]), 12: elem("P", 11, [0, Ref(11)]),
              13: None, 15: elem("Span", 11, Ref(16))},
       extra={16: {"Length": 1}}, streams={16: b"x"})
    pt("parent-dag", {"Nums": [0, [Ref(14), Ref(14)]]},
       elems={11: elem("Document", ROOT, [Ref(12), Ref(13)]), 12: elem("P", 11, [Ref(14)]),
              13: elem("Div", 11, [Ref(14)]), 14: elem("Span", 12, [0, 1])})
    return out


# ---------------------------------------------------------------------------------- metadata


def cases_metadata() -> list[Case]:
    out: list[Case] = []

    def md(label: str, info: Any, **kw: Any) -> None:
        out.append((f"info-{label}", tagged(info=info, **kw), None))

    md("strings", {"Title": b"T\x80\x9f\xad", "Author": b"\xfe\xff\x00A\xd8=\xde\x00", "Subject": b"\xfe\xff\x00\x16\x00\x7f"})
    md("every-type", {"N": Name("Nm"), "I": 3, "F": 2.5, "B": True, "Z": Raw(b"null"), "K": Raw(b"kw"), "L": [b"a", Name("b"), 1],
                      "D": {"x": b"\xfe\xff\x00y", "n": {"m": Name("z")}}, "E": [], "ED": {}})
    md("refs", {"Title": Ref(54), "Missing": Ref(77), "List": Ref(55), "Dict": Ref(56), "Stream": Ref(57)},
       extra={54: b"t", 55: [Ref(54), 1], 56: {"a": Ref(54)}, 57: {"Length": 3}}, streams={57: b"abc"})
    md("shared-dict", {"A": Ref(58), "B": Ref(58), "C": [Ref(58)]}, extra={58: {"u": b"\xfe\xff\x00\x16\x00\x7f", "v": [b"\x16"]}})
    md("self-reference", {"Title": b"x", "Self": Ref(INFO), "After": b"\xfe\xff\x00z"})
    md("self-reference-deeper", {"A": Ref(59)}, extra={59: {"a": b"1", "b": Ref(59), "c": b"3"}})
    md("refers-to-catalog", {"Cat": Ref(1)})
    md("refers-to-pages", {"Pages": Ref(2)})
    md("refers-to-page", {"Page": Ref(PAGE0)})
    md("refers-to-element", {"El": Ref(13)})
    md("refers-to-root", {"Root": Ref(ROOT)})
    md("refers-to-annot", {"Annot": Ref(30)}, extra={30: {"Type": Name("Annot"), "Subtype": Name("Link"), "Rect": [1, 2, 3, 4],
                                                         "A": {"S": Name("URI"), "URI": b"http://x/"}, "T": b"\xff"}},
       page_extra={0: {"Annots": [Ref(30)]}})
    # the page dict decoded in place up to a value that refers to itself
    # (RecursionError, logged): its /Annots, a name, is a str from then on,
    # which page.annots iterates (its /Type, after, is left a name)
    for label, annots in (("name", b"/A"), ("empty-name", b"/")):
        page = (b"<< /Annots " + annots + b" /Bomb 90 0 R /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] "
                b"/Contents 60 0 R /Resources << /Font << /F1 9 0 R >> >> /StructParents 0 >>")
        out.append((f"info-mutates-page-annots-{label}", tagged(
            info={"P": Ref(PAGE0)}, extra={PAGE0: Raw(page), 90: {"Self": Ref(90)}}, page_extra={0: {}}), None))
    md("is-catalog", Ref(1))
    md("is-page", Ref(PAGE0))
    md("not-dict", Raw(b"[1 2]"))
    md("int", Raw(b"5"))
    md("missing-object", Ref(77))
    nested: Any = b"\xfe\xff\x00x"
    for i in range(40):
        nested = {"n": nested, "l": [b"%d" % i, Name("x")]} if i % 2 else [nested, b"\x16"]
    md("nested", {"Deep": nested})
    # two /Info dicts: the update's trailer and the first one's
    base = tagged(info={"Title": b"first", "Only1": b"a"})
    prev = int(base[base.rindex(b"startxref"):].split()[1])
    upd = bytearray(base)
    off = len(upd)
    upd += b"201 0 obj\n<< /Title (second) /Only2 (b) >>\nendobj\n"
    x = len(upd)
    upd += b"xref\n201 1\n%010d 00000 n \ntrailer\n<< /Size 202 /Root 1 0 R /Info 201 0 R /Prev %d >>\nstartxref\n%d\n%%%%EOF\n" % (off, prev, x)
    out.append(("info-two-trailers", bytes(upd), None))
    return out


# -------------------------------------------------------------------------------- annotations


def cases_annots() -> list[Case]:
    out: list[Case] = []
    link = {"Type": Name("Annot"), "Subtype": Name("Link"), "Rect": [10, 20, 110, 40], "P": Ref(PAGE0),
            "A": {"S": Name("URI"), "URI": b"http://example.com/"}, "T": b"title", "Contents": b"contents"}

    def an(label: str, annots: Any, extra: dict | None = None, page: dict | None = None, **kw: Any) -> None:
        pe = {"Annots": annots}
        pe.update(page or {})
        out.append((f"annot-{label}", tagged(page_extra={0: pe}, extra={30: link, **(extra or {})}, **kw), None))

    an("link", [Ref(30)])
    for rot in (90, 180, 270, -90, 450, 90.0):
        an(f"rotate-{rot}", [Ref(30)], page={"Rotate": rot})
    an("rotate-string", [Ref(30)], page={"Rotate": b"%d"})
    # a page that is not square: rotate_point takes its width or its height
    for rot in (90, 180, 270):
        an(f"rotate-{rot}-portrait", [Ref(30)], page={"Rotate": rot, "MediaBox": [0, 0, 200, 300]})
    an("rotate-90-cropbox", [Ref(30)], page={"Rotate": 90, "MediaBox": [0, 0, 200, 300], "CropBox": [10, 20, 150, 250]})
    an("cropbox", [Ref(30)], page={"CropBox": [5, 5, 150, 150]})
    an("mediabox-offset", [Ref(30)], page={"MediaBox": [50, 50, 250, 300]})
    for label, rect in [("swapped", [110, 40, 10, 20]), ("floats", [1.5, 2.5, 3.5, 4.5]), ("short", [1, 2, 3]),
                        ("long", [1, 2, 3, 4, 5]), ("names", [Name("a"), 1, 2, 3]), ("strings", [b"1", 2, 3, 4]),
                        ("string-4", b"abcd"), ("dict-4", {"a": 1, "b": 2, "c": 3, "d": 4}), ("int", 5), ("missing", None),
                        ("ref", Ref(31)), ("bools", [True, False, True, True]), ("nested", [[1], 2, 3, 4]), ("null-item", [None, 1, 2, 3])]:
        a = dict(link)
        if rect is None:
            del a["Rect"]
        else:
            a["Rect"] = rect
        an(f"rect-{label}", [Ref(32)], extra={31: [0, 0, 10, 10], 32: a})
    for key in ("T", "Contents"):
        for label, value in [("utf8", "héllo".encode()), ("utf16-bom", b"\xff\xfeh\x00i\x00"), ("utf16-be-bom", b"\xfe\xff\x00h"),
                             ("utf16-no-bom", b"h\x00i\x00\xff"[:4] + b"\xd8\x00\xdc"), ("undecodable", b"\xff\xd8"),
                             ("undecodable-odd", b"\xff"), ("lone-surrogate", b"\x00\xd8a\x00"), ("name", Name("n")),
                             ("int", 3), ("null", Raw(b"null")), ("list", [b"a"]), ("empty", b"")]:
            a = dict(link)
            a[key] = value
            an(f"{key.lower()}-{label}", [Ref(32)], extra={32: a})
    for label, act in [("uri-undecodable", {"S": Name("URI"), "URI": b"\xff\xfe\x00\xd8"}), ("uri-name", {"URI": Name("u")}),
                       ("no-uri", {"S": Name("GoTo")}), ("a-list", [1]), ("a-ref", Ref(33)), ("a-string", b"x"),
                       ("a-stream", Ref(34)), ("uri-ref", {"URI": Ref(35)})]:
        a = dict(link)
        a["A"] = act
        an(label, [Ref(32)], extra={32: a, 33: {"URI": b"http://ref/"}, 34: {"URI": b"http://stream/"}, 35: b"http://r/"},
           streams={34: b"x"})
    a = dict(link)
    del a["P"]
    an("no-p", [Ref(32)], extra={32: a})
    a = dict(link)
    a["P"] = Ref(9)
    an("p-other", [Ref(32)], extra={32: a})
    an("stream", [Ref(36)], extra={36: {"Rect": [1, 2, 3, 4], "P": Ref(PAGE0)}}, streams={36: b"x"})
    an("stream-no-p", [Ref(36)], extra={36: {"Rect": [1, 2, 3, 4]}}, streams={36: b"x"})
    an("items-int", [5])
    an("items-null", [None])
    an("items-direct", [{"Rect": [1, 2, 3, 4], "Subtype": Name("Text")}])
    an("dict", {"Rect": [1, 2, 3, 4]})
    an("ref-list", Ref(37), extra={37: [Ref(30)]})
    an("name", Name("A"))
    an("empty", [])
    an("int", 5)
    an("cycle", [Ref(38)], extra={38: {"Rect": [1, 2, 3, 4], "Next": Ref(38)}})
    an("popup-parent", [Ref(39)], extra={39: {"Type": Name("Annot"), "Rect": [1, 2, 3, 4], "Parent": Ref(39), "P": Ref(PAGE0)}})
    return out


# ------------------------------------------------------------------------------------ depth


def chain(n: int, leaf: dict | None = None, leaf_k: Any = 0, parent_tree: bool = True, extra: dict | None = None,
          s: str = "Div") -> bytes:
    """A chain of ``n`` nested elements (``CHAIN0`` -> ``CHAIN0 + 1`` ->
    ...), the last with ``/K leaf_k`` and the attributes ``leaf``; the
    parent tree lists the last one."""
    elems: dict[int, Any] = {11: None, 12: None, 13: None}
    for i in range(n):
        num = CHAIN0 + i
        parent = ROOT if i == 0 else num - 1
        if i == n - 1:
            e = elem(s, parent, leaf_k)
            e.update(leaf or {})
        else:
            e = elem(s, parent, [Ref(num + 1)])
        elems[num] = e
    pt: Any = {"Nums": [0, [Ref(CHAIN0 + n - 1)]]} if parent_tree else None
    root_extra: dict = {"K": [Ref(CHAIN0)]}
    if not parent_tree:
        root_extra["ParentTree"] = None
    return tagged(elems=elems, parent_tree=pt, root_extra=root_extra, extra=extra)


#: The first object of a chain of elements (after every other object).
CHAIN0 = 2000


def nested_list(depth: int, inner: Any) -> Any:
    """``inner`` nested in ``depth`` lists, written without recursion."""
    return Raw(b"[" * depth + _ser(inner) + b"]" * depth)


def _ser(v: Any) -> bytes:
    from ._pdf import ser
    return ser(v)


#: The windows of ``cases_depth``: name -> (builder of the PDF for a depth
#: n, first failing n found by ``--probe``).
WINDOWS: dict[str, tuple[Callable[[int], bytes], int]] = {}


def window(name: str, first_failing: int) -> Callable[[Callable[[int], bytes]], Callable[[int], bytes]]:
    def register(make: Callable[[int], bytes]) -> Callable[[int], bytes]:
        WINDOWS[name] = (make, first_failing)
        return make
    return register


# to_dict (asdict) of the document's tree: structure_tree
@window("chain-to-dict", 330)
def _w1(n: int) -> bytes:
    return chain(n)


@window("chain-to-dict-literal-revision", 329)
def _w2(n: int) -> bytes:
    return chain(n, leaf={"R": Name("Rev")})


@window("chain-to-dict-ref-attribute", 323)
def _w3(n: int) -> bytes:
    return chain(n, leaf={"A": {"X": Ref(90)}}, extra={90: 1})


@window("chain-to-dict-nested-attribute", 327)
def _w4(n: int) -> bytes:
    return chain(n, leaf={"A": {"X": [[[[1]]]]}})


@window("chain-no-parent-tree", 330)
def _w5(n: int) -> bytes:
    return chain(n, parent_tree=False)


# prune: the tree line (PDFStructTree(pdf)) fails there
@window("chain-prune", 986)
def _w6(n: int) -> bytes:
    return chain(n)


# repr of the first element
@window("chain-repr", 497)
def _w7(n: int) -> bytes:
    return chain(n)


# a nested attribute value: asdict recursion (2 frames a level)
@window("attribute-nested-list", 492)
def _w8(n: int) -> bytes:
    return tagged(elems={12: elem("P", 11, 0, A={"X": nested_list(n, 1)})})


# C units: repr(ref) of a nested list in /K (prune's s[repr(ref)])
@window("k-nested-list-repr", 9990)
def _w9(n: int) -> bytes:
    return tagged(elems={13: elem("Span", 11, Raw(b"[1 2 " + bytes(_ser(nested_list(n, 5))) + b"]"))})


# C units: decode_text's str(s), first execution of the site
@window("actualtext-str-first", 9989)
def _w10(n: int) -> bytes:
    return tagged(elems={12: elem("P", 11, 0, ActualText=Raw(b"[300 " + bytes(_ser(nested_list(n, 1))) + b"]"))})


# the same, the site executed once before (element 12's shallow str(s))
@window("actualtext-str-second", 9990)
def _w11(n: int) -> bytes:
    return tagged(elems={12: elem("P", 11, 0, ActualText=[300]),
                         13: elem("Span", 11, [1, 2], ActualText=Raw(b"[300 " + bytes(_ser(nested_list(n, 1))) + b"]"))})


# C units: the repr of an element's nested attribute (element_repr)
@window("attribute-repr", 9991)
def _w12(n: int) -> bytes:
    return tagged(elems={11: elem("Document", ROOT, [Ref(12), Ref(13)], A={"X": nested_list(n, 1)})})


# resolve_and_decode of nested metadata dicts (a frame a level)
@window("metadata-nested-dicts", 987)
def _w13(n: int) -> bytes:
    return tagged(info=Raw(b"<< /D " + b"<< /D " * n + b"(x) " + b">> " * n + b">>"))


# resolve_and_decode of nested metadata lists (a frame and an entry from C a level)
@window("metadata-nested-lists", 987)
def _w14(n: int) -> bytes:
    return tagged(info=Raw(b"<< /L " + bytes(_ser(nested_list(n, b"x"))) + b" >>"))


# the parent tree's number tree: NumberTree._parse recursion through /Kids
@window("parent-tree-kids", 978)
def _w15(n: int) -> bytes:
    extra = {}
    for i in range(n):
        extra[CHAIN0 + i] = {"Kids": [Ref(CHAIN0 + 1 + i)]} if i < n - 1 else {"Nums": [0, [Ref(12), Ref(13), Ref(13)]]}
    return tagged(parent_tree={"Kids": [Ref(CHAIN0)]}, extra=extra)


def cases_depth() -> list[Case]:
    out: list[Case] = []
    for name, (make, first) in WINDOWS.items():
        if first <= 0:
            continue
        for n in range(first - 2, first + 2):
            out.append((f"depth-{name}-{n}", make(n), None))
    return out


# ------------------------------------------------------------------------------------ probe


def probe() -> None:
    """Prints the first failing depth of each window: the smallest n whose
    ``plumber_doc`` trace has the window's failure (dichotomy)."""
    import os
    import subprocess
    import tempfile

    checks: dict[str, Callable[[list[str]], bool]] = {
        "chain-to-dict": lambda t: _line(t, "structure_tree ").startswith("structure_tree !"),
        "chain-to-dict-literal-revision": lambda t: _line(t, "structure_tree ").startswith("structure_tree !"),
        "chain-to-dict-ref-attribute": lambda t: "RecursionError" in _line(t, "structure_tree "),
        "chain-to-dict-nested-attribute": lambda t: _line(t, "structure_tree ").startswith("structure_tree !"),
        "chain-no-parent-tree": lambda t: _line(t, "page_structure_tree ").startswith("page_structure_tree !"),
        "chain-prune": lambda t: _line(t, "tree ").startswith("tree !"),
        "chain-repr": lambda t: _line(t, "element_repr ").startswith("element_repr !"),
        "attribute-nested-list": lambda t: _line(t, "structure_tree ").startswith("structure_tree !"),
        "k-nested-list-repr": lambda t: "RecursionError" in _line(t, "structure_tree "),
        "actualtext-str-first": lambda t: "RecursionError" in _line(t, "structure_tree "),
        "actualtext-str-second": lambda t: "RecursionError" in _line(t, "structure_tree "),
        "attribute-repr": lambda t: _line(t, "element_repr ").startswith("element_repr !"),
        "metadata-nested-dicts": lambda t: _line(t, "open !RecursionError") != "",
        "metadata-nested-lists": lambda t: _line(t, "open !RecursionError") != "",
        "parent-tree-kids": lambda t: "RecursionError" in _line(t, "page_structure_tree "),
    }
    only = sys.argv[2:] if len(sys.argv) > 2 else list(WINDOWS)
    with tempfile.TemporaryDirectory() as tmp:
        def fails(name: str, n: int) -> bool:
            path = os.path.join(tmp, f"{name}-{n}.pdf")
            with open(path, "wb") as f:
                f.write(WINDOWS[name][0](n))
            r = subprocess.run([sys.executable, "-m", "oracle.trace", "--stage", "plumber_doc", path],
                               capture_output=True, text=True, check=True, timeout=600)
            return checks[name](r.stdout.splitlines())

        for name in only:
            lo, hi = 1, 1
            while not fails(name, hi):
                lo, hi = hi, hi * 2
                if hi > 1 << 16:
                    break
            while lo + 1 < hi:
                mid = (lo + hi) // 2
                if fails(name, mid):
                    hi = mid
                else:
                    lo = mid
            print(f"{name}: first failing {hi}", flush=True)


def _line(trace: list[str], prefix: str) -> str:
    for line in trace:
        if line.startswith(prefix):
            return line
    return ""


def generate() -> None:
    write_corpus("structure", cases_root() + cases_children() + cases_attributes() + cases_texts() + cases_parent_tree()
                 + cases_metadata() + cases_annots() + cases_depth())


def main() -> None:
    if sys.argv[1:2] == ["--probe"]:
        probe()
    else:
        generate()


if __name__ == "__main__":
    main()
