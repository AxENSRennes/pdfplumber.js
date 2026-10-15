"""Adversarial PDFs for the ``pages`` stage (and ``psparser``): the page
tree of ``pdfminer/pdfpage.py``, page labels (``PageLabels``) and the
boxes of pdfplumber's ``Page``.

    PDFPLUMBER_ROOT=<main checkout> python -m oracle.adversarial.pages

writes ``<cache>/corpus/adv-pages/`` (same bytes on every run). The cases:
inheritance of ``/Resources``, ``/MediaBox``, ``/CropBox`` and ``/Rotate``
(from ``/Pages`` nodes and from the catalog), page trees with loops,
shared kids, missing or odd ``/Kids``, kids that are ints, dicts, streams
or names, the lowercase ``/type`` fallback, a missing ``/Pages`` (the scan
of the xrefs), deep trees (around the recursion limit), every kind of
``/Rotate``, ``/MediaBox``, ``/CropBox``, ``/TrimBox``, ``/BleedBox``,
``/ArtBox`` and ``/Contents`` value (references, cycles, a box that is the
page itself, strings, names, short or long arrays, ``bytes % 360`` for a
string ``/Rotate``), and page labels of every style, with prefixes,
``/St`` values, ranges out of order or not starting at 0, nested number
trees and invalid entries.
"""

from __future__ import annotations

from typing import Any

from ._common import write_corpus
from ._pdf import Doc, Name, Raw, Ref, classic

CONTENT = b"BT /F1 12 Tf 10 10 Td (Hi) Tj ET"
FONT = {"Type": Name("Font"), "Subtype": Name("Type1"), "BaseFont": Name("Helvetica")}

Case = tuple[str, bytes, "str | None"]


def build(objects: dict[int, Any], catalog_extra: dict | None = None, root: Any = Ref(2),
          streams: dict[int, bytes] | None = None, trailer_extra: dict | None = None) -> bytes:
    """A file with object 1 the catalog (``/Pages root``), objects 50
    (content stream) and 51 (font), and ``objects``."""
    doc = Doc()
    catalog: dict = {"Type": Name("Catalog")}
    if root is not None:
        catalog["Pages"] = root
    catalog.update(catalog_extra or {})
    doc.add(1, catalog)
    assert not {50, 51} & set(objects), "objects 50 and 51 are the content stream and the font"
    for num, value in sorted(objects.items()):
        if isinstance(value, Raw):
            doc.add_raw(num, b"%d 0 obj\n" % num + bytes(value) + b"\nendobj\n")
        else:
            doc.add(num, value, stream=(streams or {}).get(num))
    doc.add(50, {}, stream=CONTENT)
    doc.add(51, FONT)
    return classic(doc, {"Root": Ref(1), **(trailer_extra or {})})


def page(**attrs: Any) -> dict:
    d: dict = {"Type": Name("Page")}
    d.update(attrs)
    return d


def pages(kids: list, **attrs: Any) -> dict:
    d: dict = {"Type": Name("Pages"), "Kids": kids, "Count": len(kids) if isinstance(kids, list) else 1}
    d.update(attrs)
    return d


RES = {"Font": {"F1": Ref(51)}}
BOX = [0, 0, 300, 400]


def one_page(**attrs: Any) -> bytes:
    """A page tree of one page with these attributes (plus /Contents)."""
    p = page(Parent=Ref(2), Contents=Ref(50), Resources=RES)
    p.update(attrs)
    p = {k: v for k, v in p.items() if v is not None}
    return build({2: pages([Ref(3)], MediaBox=BOX), 3: p})


def cases_tree() -> list[Case]:
    out: list[Case] = []
    out.append(("basic", one_page(), None))
    # inheritance from /Pages nodes and from the catalog
    out.append(("inherit-all", build({
        2: pages([Ref(3)], MediaBox=[0, 0, 500, 500], CropBox=[10, 10, 490, 490], Rotate=90, Resources=RES),
        3: pages([Ref(4)], Parent=Ref(2), MediaBox=[0, 0, 600, 600]),
        4: page(Parent=Ref(3), Contents=Ref(50)),
    }), None))
    out.append(("inherit-from-catalog", build({2: pages([Ref(3)]), 3: page(Contents=Ref(50))},
                                              catalog_extra={"MediaBox": [0, 0, 111, 222], "Rotate": 180, "Resources": RES}), None))
    out.append(("inherit-not-inheritable", build({2: pages([Ref(3)], Annots=[Ref(51)], Contents=Ref(50), Foo=1), 3: page()}), None))
    out.append(("inherit-null-overrides", build({2: pages([Ref(3)], Rotate=90), 3: Raw(b"<</Type /Page /Rotate null>>")}), None))
    out.append(("inherit-ref-values", build({2: pages([Ref(3)], MediaBox=Ref(4), Resources=Ref(5)), 3: page(), 4: [0, 0, 70, 80], 5: RES}), None))
    # tree shapes
    out.append(("tree-loop-self", build({2: pages([Ref(3), Ref(2)]), 3: page(MediaBox=BOX)}), None))
    out.append(("tree-loop-parent", build({2: pages([Ref(3)]), 3: pages([Ref(4), Ref(2)]), 4: page(MediaBox=BOX)}), None))
    out.append(("tree-shared-kid", build({2: pages([Ref(3), Ref(4)]), 3: pages([Ref(5)]), 4: pages([Ref(5)]), 5: page(MediaBox=BOX)}), None))
    out.append(("tree-page-twice", build({2: pages([Ref(3), Ref(3), Ref(4)]), 3: page(MediaBox=BOX), 4: page(MediaBox=BOX)}), None))
    out.append(("tree-kids-missing", build({2: {"Type": Name("Pages")}, 3: page(MediaBox=BOX)}), None))
    out.append(("tree-kids-not-list", build({2: pages(Ref(3)), 3: page(MediaBox=BOX)}), None))
    out.append(("tree-kids-ref-to-list", build({2: pages(Ref(4)), 3: page(MediaBox=BOX), 4: [Ref(3)]}), None))
    out.append(("tree-kids-ints", build({2: pages([3, Raw(b"3.0"), True]), 3: page(MediaBox=BOX), 1 + 0: {"Type": Name("Catalog"), "Pages": Ref(2)}}), None))
    out.append(("tree-kid-direct-dict", build({2: pages([page(MediaBox=BOX)])}), None))
    out.append(("tree-kid-name", build({2: pages([Name("P")])}), None))
    out.append(("tree-kid-null", build({2: pages([None, Ref(3)]), 3: page(MediaBox=BOX)}), None))
    out.append(("tree-kid-float", build({2: pages([Raw(b"3.5"), Ref(3)]), 3: page(MediaBox=BOX)}), None))
    out.append(("tree-kid-stream", build({2: pages([Ref(4), Ref(3)]), 3: page(MediaBox=BOX), 4: {"Type": Name("Page")}},
                                         streams={4: b"q Q"}), None))
    out.append(("tree-kid-missing-object", build({2: pages([Ref(77), Ref(3)]), 3: page(MediaBox=BOX)}), None))
    out.append(("tree-kid-int-missing", build({2: pages([77, Ref(3)]), 3: page(MediaBox=BOX)}), None))
    out.append(("tree-lowercase-type", build({2: {"type": Name("Pages"), "Kids": [Ref(3)]}, 3: {"type": Name("Page"), "MediaBox": BOX}}), None))
    out.append(("tree-type-wrong", build({2: pages([Ref(3), Ref(4)]), 3: {"Type": Name("Pagex"), "MediaBox": BOX}, 4: page(MediaBox=BOX)}), None))
    out.append(("tree-type-string", build({2: pages([Ref(3)]), 3: {"Type": b"Page", "MediaBox": BOX}}), None))
    out.append(("tree-root-int", build({2: pages([Ref(3)]), 3: page(MediaBox=BOX)}, root=2), None))
    out.append(("tree-root-bool", build({1 + 1: pages([Ref(3)]), 3: page(MediaBox=BOX)}, root=True), None))
    out.append(("tree-root-dict", build({3: page(MediaBox=BOX)}, root=pages([Ref(3)])), None))
    out.append(("tree-root-name", build({3: page(MediaBox=BOX)}, root=Name("Pages")), None))
    out.append(("tree-root-page", build({2: page(MediaBox=BOX)}), None))
    out.append(("tree-root-stream", build({2: pages([Ref(3)]), 3: page(MediaBox=BOX)}, streams={2: b"x"}), None))
    # no /Pages: the scan of the xrefs; pages in object order
    out.append(("no-pages-scan", build({3: page(MediaBox=BOX), 4: page(MediaBox=[0, 0, 10, 10]), 5: {"Type": Name("Pages")}}, root=None), None))
    out.append(("pages-empty-scan", build({2: pages([]), 3: page(MediaBox=BOX)}), None))
    out.append(("pages-missing-object-scan", build({3: page(MediaBox=BOX)}, root=Ref(77)), None))
    # deep trees: a chain of /Pages nodes, a page at each level (numbers
    # from 1000, away from the objects build() adds)
    # (a page right at the frame limit is left out: its outcome depends on the
    # frames of the parser's own methods, not counted yet, see
    # docs/architecture.md "Profondeur de récursion")
    for depth in (40, 160, 400, 960):
        objs: dict[int, Any] = {}
        node = [2] + [1000 + 2 * i for i in range(1, depth)]
        for i in range(depth):
            kids = [Ref(node[i] + 1 if i else 1001)] + ([Ref(node[i + 1])] if i + 1 < depth else [])
            objs[node[i]] = pages(kids, Rotate=90 * (i % 4))
            objs[node[i] + 1 if i else 1001] = page(MediaBox=[0, 0, 100 + i, 100])
        out.append((f"tree-deep-{depth}", build(objs), None))
    # only /Pages nodes down to the page: RecursionError before any page
    chain = [2] + [1000 + i for i in range(1, 1100)]
    objs = {chain[i]: pages([Ref(chain[i + 1] if i + 1 < len(chain) else 5000)]) for i in range(len(chain))}
    objs[5000] = page(MediaBox=BOX)
    out.append(("tree-deep-chain-1100", build(objs), None))
    return out


def cases_attrs() -> list[Case]:
    out: list[Case] = []
    for label, value in [
        ("90", 90), ("-90", -90), ("450", 450), ("45", 45), ("float", 90.0), ("float-odd", 1.5), ("true", True),
        ("huge", 10**30), ("name", Name("R")), ("list", [90]), ("dict", {"A": 1}), ("ref", Ref(4)), ("ref-to-page", Ref(3)),
        ("string-d", b"%d"), ("string-plain", b"90"), ("string-x", b"%#x"), ("string-o", b"%#o"), ("string-X", b"%X"),
        ("string-f", b"%f"), ("string-e", b"%.2e"), ("string-g", b"%g"), ("string-G", b"%#.1G"), ("string-c", b"%c"),
        ("string-c-width", b"%5c|"), ("string-s", b"%s"), ("string-r", b"%r"), ("string-a", b"%.2a"), ("string-pct", b"%%"),
        ("string-pct-d", b"%%%d"), ("string-two", b"%d%d"), ("string-star", b"%*d"), ("string-star-prec", b"%.*d"),
        ("string-width", b"%010.5d"), ("string-left", b"%-8d|"), ("string-plus", b"%+d"), ("string-space", b"% d"),
        ("string-incomplete", b"%"), ("string-incomplete-flags", b"%-5"), ("string-mapping", b"%(a)d"),
        ("string-bad-conv", b"%y"), ("string-length-mod", b"%ld"), ("string-prec-big", b"%.99999999999d"),
        ("string-hex-alt-zero", b"%#010x"), ("string-e-zero-prec", b"%#.0e"), ("string-g-prec", b"%.10g"),
    ]:
        extra = {4: 180} if label == "ref" else {}
        p = page(Parent=Ref(2), Contents=Ref(50), Resources=RES, Rotate=value, MediaBox=BOX)
        out.append((f"rotate-{label}", build({2: pages([Ref(3)]), 3: p, **extra}), None))
    for label, value in [
        ("ints", [0, 0, 612, 792]), ("floats", [0.5, 1.25, 300.75, 400.5]), ("swapped", [300, 400, 0, 0]),
        ("negative", [-100, -50, 100, 50]), ("bools", [False, False, True, True]), ("huge", [0, 0, 10**20, 10**20]),
        ("short", [0, 0, 300]), ("long", [0, 0, 300, 400, 500]), ("empty", []), ("strings", [b"0", b"0", b"300", b"400"]),
        ("names", [Name("a"), 0, 1, 2]), ("nested", [[0], 0, 1, 2]), ("null-item", [0, None, 1, 2]),
        ("refs-inside", [Ref(4), 0, Ref(5), 400]), ("ref", Ref(6)), ("ref-to-page", Ref(3)), ("string", b"abcd"),
        ("string-short", b"ab"), ("name", Name("Letter")), ("int", 5), ("dict", {"a": 1, "b": 2, "c": 3, "d": 4}),
        ("dict-empty", {}), ("cycle", Ref(7)), ("ref-chain", Ref(8)), ("stream", Ref(9)), ("float-strings", [b"1.5", b"2", b"3e2", b"nan"]),
    ]:
        objs: dict[int, Any] = {2: pages([Ref(3)]), 4: 0, 5: 300, 6: [0, 0, 250, 350], 7: [Ref(7), 0, 1, 2], 8: Ref(6)}
        objs[3] = page(Parent=Ref(2), Contents=Ref(50), Resources=RES, MediaBox=value)
        objs[9] = {"N": 1}
        out.append((f"mediabox-{label}", build(objs, streams={9: b"0 0 1 1"}), None))
    out.append(("mediabox-missing", build({2: pages([Ref(3)]), 3: page(Contents=Ref(50))}), None))
    for name in ("CropBox", "TrimBox", "BleedBox", "ArtBox"):
        for label, value in [("normal", [10, 10, 200, 300]), ("swapped", [200, 300, 10, 10]), ("bad", [Name("x"), 1, 2, 3]),
                             ("short", [1, 2]), ("null", None), ("rotated", [10, 20, 30, 40])]:
            attrs = {"Parent": Ref(2), "Contents": Ref(50), "MediaBox": BOX, name: value}
            if label == "rotated":
                attrs["Rotate"] = 270
            if value is None:
                p = Raw(b"<</Type /Page /MediaBox [0 0 300 400] /%s null>>" % name.encode())
            else:
                p = page(**attrs)
            out.append((f"{name.lower()}-{label}", build({2: pages([Ref(3)]), 3: p}), None))
    out.append(("all-boxes", one_page(MediaBox=[0, 0, 600, 800], CropBox=[5, 5, 595, 795], TrimBox=[10, 10, 590, 790],
                                      BleedBox=[1, 1, 599, 799], ArtBox=[20, 20, 580, 780], Rotate=90), None))
    for label, value in [("ref", Ref(50)), ("list", [Ref(50), Ref(50)]), ("ref-to-list", Ref(4)), ("direct-dict", {"A": 1}),
                         ("null", None), ("missing-object", Ref(77)), ("int", 5), ("list-of-junk", [5, Name("x"), Ref(77), Ref(51)]),
                         ("empty-list", [])]:
        p = page(Parent=Ref(2), Resources=RES, MediaBox=BOX, Contents=value)
        p = {k: v for k, v in p.items() if v is not None}
        out.append((f"contents-{label}", build({2: pages([Ref(3)]), 3: p, 4: [Ref(50)]}), None))
    for label, value in [("ref", Ref(4)), ("missing", None), ("not-dict", 5), ("missing-object", Ref(77)), ("xobject-forms", Ref(5))]:
        p = page(Parent=Ref(2), Contents=Ref(50), MediaBox=BOX)
        if value is not None:
            p["Resources"] = value
        objs = {2: pages([Ref(3)]), 3: p, 4: RES, 5: {"XObject": {"Fm1": Ref(6), "Fm2": Ref(6), "Im": Ref(7)}, "Font": {"F1": Ref(51)}},
                6: {"Type": Name("XObject"), "Subtype": Name("Form"), "BBox": [0, 0, 10, 10], "Resources": {"XObject": {"Fm3": Ref(8)}}},
                7: {"Type": Name("XObject"), "Subtype": Name("Image"), "Width": 1, "Height": 1, "ColorSpace": Name("DeviceGray"), "BitsPerComponent": 8},
                8: {"Subtype": Name("Form"), "BBox": [0, 0, 1, 1], "Resources": {"XObject": {"Back": Ref(6)}}}}
        out.append((f"resources-{label}", build(objs, streams={6: b"0 0 m 1 1 l S", 7: b"\x80", 8: b"q Q"}), None))
    out.append(("page-misc-attrs", one_page(Annots=[Ref(51)], B=[Ref(3)], LastModified=b"D:20200101", Thumb=Ref(50)), None))
    out.append(("page-attr-refs", one_page(LastModified=Ref(4), Annots=Ref(4)) .replace(b"endobj\n50 0 obj", b"endobj\n4 0 obj\n(D:x)\nendobj\n50 0 obj", 1), None))
    return out


def labels_doc(labels: Any, npages: int = 6, extra: dict[int, Any] | None = None) -> bytes:
    kids = [Ref(10 + i) for i in range(npages)]
    objs: dict[int, Any] = {2: pages(kids, MediaBox=BOX)}
    for i in range(npages):
        objs[10 + i] = page(Parent=Ref(2))
    objs.update(extra or {})
    return build(objs, catalog_extra={"PageLabels": labels})


def cases_labels() -> list[Case]:
    out: list[Case] = []
    for style in ("D", "R", "r", "A", "a", "X"):
        out.append((f"labels-style-{style}", labels_doc({"Nums": [0, {"S": Name(style)}]}), None))
    out.append(("labels-all-styles", labels_doc({"Nums": [0, {"S": Name("r")}, 2, {"S": Name("D"), "P": b"P-"}, 3, {"S": Name("A"), "St": 26},
                                                           4, {"P": b"\xfe\xff\x00X"}, 5, {"S": Name("a"), "St": 700}]}, 8), None))
    out.append(("labels-no-style", labels_doc({"Nums": [0, {"P": b"Cover"}]}), None))
    out.append(("labels-not-from-0", labels_doc({"Nums": [2, {"S": Name("D")}]}), None))
    out.append(("labels-unsorted", labels_doc({"Nums": [3, {"S": Name("a")}, 0, {"S": Name("R")}]}), None))
    out.append(("labels-kids", labels_doc({"Kids": [Ref(4), Ref(5)]}, extra={4: {"Nums": [0, {"S": Name("D")}], "Limits": [0, 0]},
                                                                             5: {"Nums": [3, {"S": Name("r"), "St": 5}]}}), None))
    for label, st in [("st-zero-roman", {"S": Name("R"), "St": 0}), ("st-negative-alpha", {"S": Name("a"), "St": -1}),
                      ("st-zero-decimal", {"S": Name("D"), "St": 0}), ("st-big-roman", {"S": Name("r"), "St": 3998}),
                      ("st-float", {"S": Name("D"), "St": 2.5}), ("st-bool", {"S": Name("D"), "St": True}),
                      ("st-huge-alpha", {"S": Name("A"), "St": 10**15}), ("prefix-not-string", {"S": Name("D"), "P": Name("x")}),
                      ("style-string", {"S": b"D"}), ("dict-ref", Ref(4)), ("not-dict", 5)]:
        out.append((f"labels-{label}", labels_doc({"Nums": [0, st]}, extra={4: {"S": Name("R")}}), None))
    for label, value in [("ref", Ref(4)), ("not-dict", 5), ("empty", {}), ("nums-odd", {"Nums": [0, {"S": Name("D")}, 2]}),
                         ("nums-key-name", {"Nums": [Name("a"), {"S": Name("D")}]}), ("nums-not-list", {"Nums": 5}),
                         ("missing-object", Ref(77)), ("kids-loop", {"Kids": [Ref(4)]})]:
        extra = {4: {"Nums": [0, {"S": Name("a")}]}} if label == "ref" else {4: {"Kids": [Ref(4)]}}
        out.append((f"labels-{label}", labels_doc(value, extra=extra), None))
    out.append(("labels-no-pages-scan", build({3: page(MediaBox=BOX), 4: page(MediaBox=BOX)}, root=None,
                                              catalog_extra={"PageLabels": {"Nums": [0, {"S": Name("A")}]}}), None))
    return out


def outline_doc(outlines: dict[int, Any], catalog_extra: dict) -> bytes:
    objs: dict[int, Any] = {2: pages([Ref(3)], MediaBox=BOX), 3: page(Parent=Ref(2))}
    objs.update(outlines)
    return build(objs, catalog_extra=catalog_extra)


NAMES = {30: {"Kids": [Ref(32), Ref(33)]}, 32: {"Limits": [b"a", b"b"], "Names": [b"a", [Ref(3), Name("Fit")], b"b", 5]},
         33: {"Limits": [b"c", b"z"], "Names": [b"c", 1, b"zz", {"D": [Ref(3), Name("XYZ")]}]}, 31: {"a": [Ref(3), Name("XYZ")], "zz": 7}}
OUTLINE = {20: {"Type": Name("Outlines"), "First": Ref(21), "Last": Ref(23)},
           21: {"Title": b"One", "Dest": [Ref(3), Name("Fit")], "Next": Ref(22)},
           22: {"Title": b"\xfe\xff\x00T\x00w\x00o\xd8\x00", "A": {"S": Name("URI"), "URI": b"http://x"}, "Next": Ref(23),
                "First": Ref(24), "Last": Ref(24)},
           23: {"Title": b"Thr\x18ee", "Dest": b"a", "SE": Ref(3)},
           24: {"Title": b"Sub", "Dest": Name("b")}}


def cases_outlines() -> list[Case]:
    """Outlines, name trees and destinations (``api_trace`` of
    ``oracle/unit/document.py``)."""
    cat = {"Outlines": Ref(20), "Names": {"Dests": Ref(30)}, "Dests": Ref(31)}
    out: list[Case] = [("outlines-names-dests", outline_doc({**OUTLINE, **NAMES}, cat), None)]

    def variant(label: str, changes: dict[int, Any], catalog: dict | None = None) -> None:
        objs = {**OUTLINE, **NAMES, **changes}
        objs = {k: v for k, v in objs.items() if v is not None}
        out.append((f"outlines-{label}", outline_doc(objs, catalog if catalog is not None else cat), None))

    variant("loop", {23: {"Title": b"Three", "Dest": b"a", "Next": Ref(21)}})
    variant("no-title", {21: {"Dest": [Ref(3)], "Next": Ref(22)}})
    variant("title-not-string", {21: {"Title": Name("T"), "Dest": [Ref(3)], "Next": Ref(22)}})
    variant("first-without-last", {20: {"First": Ref(21)}})
    variant("root-not-dict", {}, {**cat, "Outlines": 5})
    variant("missing-objects", {22: None, 24: None})
    variant("names-limits-int", {32: {"Limits": [1, 2], "Names": [b"a", 1]}})
    variant("names-limits-three", {32: {"Limits": [b"a", b"b", b"c"], "Names": [b"a", 1]}})
    variant("names-key-list", {32: {"Names": [[1], 2, b"a", 3]}})
    variant("names-odd", {32: {"Names": [b"a", 1, b"b"]}})
    variant("names-kids-loop", {30: {"Kids": [Ref(30)]}})
    variant("names-not-dict", {}, {**cat, "Names": 5})
    variant("names-dests-missing", {}, {**cat, "Names": {"JavaScript": Ref(30)}})
    variant("dests-missing", {}, {"Outlines": Ref(20), "Names": {"Dests": Ref(30)}})
    variant("names-value-falsy", {32: {"Names": [b"a", 0, b"b", []]}})
    chain = {20: {"First": Ref(100), "Last": Ref(100)}}
    for i in range(1200):
        # no title before the depth where Python runs out of frames: the
        # outcome does not depend on where exactly that happens
        chain[100 + i] = {"Dest": [i], "Next": Ref(101 + i)}
        if i >= 1150:
            chain[100 + i]["Title"] = b"t%d" % i
    variant("deep-chain", chain)
    return out


def main() -> None:
    write_corpus("pages", cases_tree() + cases_attrs() + cases_labels() + cases_outlines())


if __name__ == "__main__":
    main()
