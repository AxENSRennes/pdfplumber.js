"""Adversarial PDFs for pdfplumber's page layer: the ``plumber_objects``,
``plumber_text``, ``plumber_tables`` and ``plumber_doc`` stages
(``--set adv-plumber``).

    PDFPLUMBER_ROOT=<main checkout> python -m oracle.adversarial.plumber

writes ``<cache>/corpus/adv-plumber/`` (same bytes on every run, but for the
depth windows, whose bounds are measured on the oracle: ``cases_depth``).
Documents have several pages where they can: each page of a file takes
another slice of the stages' option tables (``select``), so that the
variants of the text, table, derivation and serialization ops meet the
odd objects. The cases (pdfplumber/pdf.py, page.py, container.py,
convert.py, utils/pdfinternals.py and what they call):

- ``box-*``: page boxes (``Page.__init__``: ``_normalize_box``,
  ``_invert_box``): a ``/MediaBox`` that does not start at 0, inverted,
  negative, huge, fractional, degenerate (no area: a crop raises), of
  booleans, of references, inherited, missing, too short, non-numeric, a
  dict, a string; ``/CropBox`` (larger, inverted, partly outside, odd),
  ``/TrimBox``, ``/BleedBox``, ``/ArtBox`` set or odd; every ``/Rotate``
  (0 to 270, negative, past 360, not a multiple of 90, a float, a string,
  a name), each page with text, rects, lines, curves and an annotation
  (``rotate_point``);
- ``geom-*``: objects exactly on the crop boundaries the stages' derived
  pages use (fractions of the page box) and partly outside, outside the
  page, zero-size and negative-size rects, lines and curves, every path
  operator, infinite coordinates (an infinite ``cm`` or operand, whose
  differences are NaN);
- ``paint-*``: dash patterns (empty, odd values, nested arrays), every color
  setter on paths and text (gray, RGB, CMYK, patterns colored and
  uncolored, ``Separation``, ``DeviceN``), line widths;
- ``image-*``: image XObjects whose ``/ColorSpace``, ``/BitsPerComponent``,
  ``/Width``, ``/ImageMask`` are references, arrays, names, missing;
  inline images (named ``inline-<k>`` by the port), several per page and in
  forms (no XObject named like that: the port's names would collide, the
  documented deviation of ``docs/public-api.md``);
- ``mc-*``: marked content whose ``/MCID`` is every kind of value (int,
  float, name, string, array, dict, reference, null, bool, a nested
  array), properties by resource name, ``BMC``, nested and unbalanced
  ``BDC``/``EMC``, odd tag names (bytes names);
- ``font-*``: chars of every font kind (standard Type 1, Type 3, embedded
  Type 1, ``ToUnicode``, ``Type0`` horizontal and vertical, a predefined
  CMap), font names given as strings (the five ``CP936_FONTNAMES``, with and
  without a subset prefix, other bytes), missing fonts;
- ``text-*``: duplicate and overlapping chars for ``dedupe_chars`` (same
  place, offsets at the tolerances, other fonts, sizes and colors),
  ligatures (U+FB00..U+FB06), lone surrogates, words and lines at the word
  extractor's tolerances, rotated and vertical text, text outside the page,
  zero, negative and huge font sizes, empty strings and spaces;
- ``table-*``: ruled grids at the snapping, joining, intersection and edge
  length tolerances, grids of rects, lines and curves, text columns for the
  ``text`` strategy, tables in figures, a page of thousands of tiny objects
  (bounded);
- ``annot-*``: annotations without ``/Rect``, with a short or non-numeric
  ``/Rect``, ``/A`` and ``/URI`` of every type, ``/T`` and ``/Contents`` in
  UTF-8, UTF-16 and neither, ``/P``, ``/Annots`` that is not an array, on
  rotated pages;
- ``layout-*``: text boxes at equal distances (the analysis' ``id()``
  ties), nested figures with text (``laparams`` figure objects);
- ``depth-*`` (``cases_depth``): around the first level at which Python
  raises ``RecursionError``: chains of Form XObjects (the interpreter, then
  ``iter_layout_objects`` over the nested figures), an image ``/ColorSpace``
  and an ``/MCID`` nested in arrays or through chains of references
  (``resolve_all``), a dash pattern nested in arrays (``Serializer`` of
  ``to_json``/``to_csv``, ``plumber_doc``).
"""

from __future__ import annotations

import argparse
import sys
from typing import Any

from ._common import write_corpus
from ._pdf import Name, Raw, Ref
from .interp import HELVETICA, Builder, first_failing, form_chain, type0, type3

Case = tuple[str, bytes, "str | None"]

#: An operand ``float()`` makes infinite (content stream syntax).
INF = b"1" + b"0" * 400 + b".5"

FRACTIONS = (0.05, 0.1, 0.2, 0.25, 0.3, 0.5, 0.6, 0.75, 0.8, 0.9, 0.95, 1.0)


def num(x: float) -> bytes:
    if float(x).is_integer():
        return b"%d" % int(x)
    return repr(float(x)).encode()


def text(x: float, y: float, s: bytes, font: bytes = b"F1", size: float = 10) -> bytes:
    return b"BT /%s %s Tf %s %s Td (%s) Tj ET\n" % (font, num(size), num(x), num(y), s)


#: A page of everything: text lines, rects filled and stroked, lines, a
#: curve, a small ruled table (in a 200 x 200 box).
SCENE = (
    text(20, 180, b"Hello world, the quick fox.")
    + text(20, 165, b"Second line: 12 34 56")
    + text(120, 165, b"right side")
    + b"q 0.5 g 20 120 60 20 re f Q 1 w 100 120 60 20 re S\n"
    + b"20 100 m 180 100 l S 30 30 m 60 90 l S\n"
    + b"40 60 m 50 80 70 80 80 60 c S\n"
    + b"20 20 m 180 20 l 20 40 m 180 40 l 20 55 m 180 55 l S\n"
    + b"20 20 m 20 55 l 100 20 m 100 55 l 180 20 m 180 55 l S\n"
    + text(25, 45, b"cell A") + text(105, 45, b"cell B") + text(25, 25, b"12") + text(105, 25, b"34")
)


def doc(pages: list[tuple[bytes, dict]], resources: Any = None, page_attrs: dict | None = None,
        pages_attrs: dict | None = None) -> bytes:
    """A document of ``(content, page dict attributes)`` pages, sharing
    ``resources`` (Helvetica as ``F1`` by default). ``pages_attrs``: the
    page tree's (inherited)."""
    b = Builder()
    font = b.obj(HELVETICA)
    res = resources(b) if callable(resources) else (resources if resources is not None else {"Font": {"F1": font}})
    for content, attrs in pages:
        a = dict(page_attrs or {})
        a.update(attrs)
        drop = [k for k, v in a.items() if v is DROP]
        ref = b.page(content, res, **{k: v for k, v in a.items() if v is not DROP})
        if drop:
            for i, (num_, value, stream) in enumerate(b.objects):
                if num_ == ref.num:
                    b.objects[i] = (num_, {k: v for k, v in value.items() if k not in drop}, stream)
    if pages_attrs:
        return _with_pages_attrs(b, pages_attrs)
    return b.build()


class _Drop:
    """A page attribute left out (``/MediaBox`` that ``Builder.page`` sets)."""


DROP = _Drop()


def _with_pages_attrs(b: Builder, attrs: dict) -> bytes:
    from ._pdf import Doc, classic

    d = Doc()
    d.add(1, {"Type": Name("Catalog"), "Pages": Ref(2)})
    kids = [Ref(n) for n in b.pages]
    d.add(2, {"Type": Name("Pages"), "Kids": kids, "Count": len(kids), **attrs})
    for num_, value, stream in sorted(b.objects, key=lambda o: o[0]):
        d.add(num_, value, stream=stream)
    return classic(d, {"Root": Ref(1)})


def link_annot(b: Builder, rect: Any, **extra: Any) -> Ref:
    d = {"Type": Name("Annot"), "Subtype": Name("Link"), "Rect": rect, "Border": [0, 0, 0],
         "A": {"S": Name("URI"), "URI": b"https://example.com/a"}}
    d.update(extra)
    return b.obj({k: v for k, v in d.items() if v is not DROP})


# ---------------------------------------------------------------------------------- boxes


def cases_boxes() -> list[Case]:
    out: list[Case] = []
    good = {
        "origin": [10, 20, 210, 220],
        "inverted": [200, 200, 0, 0],
        "negative": [-100, -50, 100, 150],
        "fractional": [0.5, 0.25, 200.75, 199.5],
        "huge": [0, 0, 14400, 14400],
        "huge-int": [0, 0, Raw(b"1" + b"0" * 30), 200],
        "mixed": [0, 0.0, 200, 200.0],
        "bools": [False, False, 200, True],
        "refs": "refs",
        "inverted-x": [200, 0, 0, 200],
        "tall": [0, 0, 100, 400],
        "wide": [0, 0, 400, 100],
        "degenerate-x": [50, 0, 50, 200],
        "degenerate": [0, 0, 0, 0],
    }
    for name, box in good.items():
        b = Builder()
        font = b.obj(HELVETICA)
        if box == "refs":
            box = [b.obj(0), b.obj(0), b.obj(200), b.obj(200.5)]
        for _ in range(3):
            b.page(SCENE, {"Font": {"F1": font}}, MediaBox=box, Annots=[link_annot(b, [20, 20, 80, 60])])
        out.append((f"box-media-{name}", b.build(), None))
    # boxes Page.__init__ refuses (each page of the document raises: pdf.pages)
    bad = {
        "short": [0, 0, 200],
        "string": [0, 0, b"200", 200],
        "name": [0, 0, Name("A"), 200],
        "null-item": [0, 0, None, 200],
        "dict": {},
        "dict-items": {"A": 1},
        "bytes": b"\x00\x00\xc8\xc8",
        "number": 200,
        "nested": [[0, 0], [200, 200]],
        "long": [0, 0, 200, 200, 5],
    }
    for name, box in bad.items():
        out.append((f"box-media-bad-{name}", doc([(SCENE, {"MediaBox": box})] * 2), None))
    out.append(("box-media-missing", doc([(SCENE, {"MediaBox": DROP})]), None))
    out.append(("box-media-inherited", doc([(SCENE, {"MediaBox": DROP})] * 2,
                                           pages_attrs={"MediaBox": [5, 5, 205, 255]}), None))
    crops = {
        "larger": [-50, -50, 250, 250],
        "inverted": [180, 180, 20, 20],
        "partly": [100, 100, 300, 300],
        "inside": [20, 30, 150, 160],
        "outside": [300, 300, 400, 400],
        "degenerate": [10, 10, 10, 10],
        "bad": [0, 0, Name("x"), 1],
        "short": [0, 0, 1],
        "null": None,
    }
    for name, box in crops.items():
        out.append((f"box-crop-{name}", doc([(SCENE, {"CropBox": box})] * 3), None))
    out.append(("box-crop-inherited", doc([(SCENE, {})] * 2, pages_attrs={"CropBox": [10, 10, 190, 190]}), None))
    for which in ("TrimBox", "BleedBox", "ArtBox"):
        for name, box in (("set", [5, 6, 195, 194]), ("inverted", [195, 194, 5, 6]), ("bad", [0, Name("y"), 1, 1]),
                          ("short", [1, 2]), ("null", None)):
            out.append((f"box-{which.lower()}-{name}", doc([(SCENE, {which: box})] * 2), None))
    out.append(("box-all", doc([(SCENE, {"MediaBox": [0, 0, 300, 300], "CropBox": [10, 10, 290, 290],
                                         "TrimBox": [20, 20, 280, 280], "BleedBox": [15, 15, 285, 285],
                                         "ArtBox": [30, 30, 270, 270]})] * 4), None))
    rotations = [0, 90, 180, 270, -90, -180, 360, 450, 540, 45, 89, 90.0, 270.5, Raw(b"(90)"), Name("90"), None,
                 Raw(b"1" + b"0" * 25), True]
    for i, r in enumerate(rotations):
        b = Builder()
        font = b.obj(HELVETICA)
        for box in ([0, 0, 200, 300], [10, 20, 310, 220]):
            b.page(SCENE, {"Font": {"F1": font}}, MediaBox=box, Rotate=r,
                   Annots=[link_annot(b, [20, 20, 80, 60]), link_annot(b, [150, 250, 190, 290])])
        out.append((f"box-rotate-{i}", b.build(), None))
    out.append(("box-rotate-inherited", doc([(SCENE, {})] * 2, pages_attrs={"Rotate": 90}), None))
    return out


# ---------------------------------------------------------------------------------- geometry


def cases_geometry() -> list[Case]:
    out: list[Case] = []
    # objects on the boundaries of the derived pages (fractions of 200)
    content = b""
    for f in FRACTIONS:
        v = num(200 * f)
        content += b"%s 0 m %s 200 l S 0 %s m 200 %s l S\n" % (v, v, v, v)
        content += b"%s %s 10 10 re f\n" % (num(200 * f - 5), num(200 * f - 5))
        content += text(200 * f, 200 * (1 - f), b"x")
        content += text(200 * f - 3, 200 * (1 - f) + 2, b"yz")
    content += b"-20 -20 60 60 re S 170 170 60 60 re f 190 190 m 220 220 l S\n"
    content += text(-10, 100, b"left out") + text(195, 100, b"right out") + text(100, 205, b"above")
    out.append(("geom-boundaries", doc([(content, {})] * 6), None))
    out.append(("geom-boundaries-origin", doc([(content, {"MediaBox": [-20, -10, 180, 190]})] * 4), None))
    degenerate = (
        b"50 50 0 0 re S 60 60 0 10 re f 70 70 10 0 re B 80 80 -10 -10 re S 90 90 -5 10 re f\n"
        b"100 100 m 100 100 l S 110 110 m S 120 120 m h S\n"
        b"10 10 m 10 10 10 10 10 10 c S 20 20 m 30 30 40 40 v S 30 30 m 40 40 50 50 y S\n"
        b"10 150 m 50 150 l 50 190 l h f* 60 150 m 100 190 l 60 190 l b\n"
        b"q 0 0 0 0 0 0 cm 0 0 m 10 10 l S Q\n"
        b"q 2 0 0 0.5 10 10 cm 0 0 m 10 10 l S 0 0 20 20 re S Q\n"
        b"q 0 1 -1 0 150 50 cm 0 0 m 30 10 l S 0 0 20 10 re f Q\n"
    )
    out.append(("geom-degenerate", doc([(degenerate, {})] * 4), None))
    infinite = (
        b"(inf) 0 m (inf) 5 l S\n"
        b"0 (inf) m 5 (inf) l S\n"
        b"(inf) (inf) m (-inf) (-inf) l S\n"
        b"10 10 (inf) 5 re S\n"
        b"q " + INF + b" 0 0 " + INF + b" 0 0 cm 0 0 m 1 1 l S Q\n"
        b"q 1 0 0 1 " + INF + b" 0 cm 0 0 m 1 1 l S 0 0 5 5 re f Q\n"
        b"20 20 m 40 40 l S\n"
    )
    out.append(("geom-infinite", doc([(infinite + text(20, 100, b"after"), {})] * 3), None))
    # No char at an infinite position: its other coordinate is NaN (inf * 0
    # in the text matrix), and a NaN that reaches a set of
    # cluster_objects (words, lines, the text strategy's edges) takes the
    # place its address gives it in CPython (utils/clustering.rs).
    return out


# ---------------------------------------------------------------------------------- painting


def cases_paint() -> list[Case]:
    out: list[Case] = []
    dashes = [b"[] 0", b"[3] 0", b"[3 2] 1", b"[0 0] 0", b"[-1 2] 0", b"[1.5 (a)] 0", b"[/x] 0", b"[[1 2] 3] 0",
              b"[" * 40 + b"1" + b"]" * 40 + b" 0", b"3 0", b"[3 2] (p)", b"[true] 1"]
    content = b""
    for i, d in enumerate(dashes):
        content += d + b" d %d 10 m %d 190 l S %d 20 5 5 re S\n" % (10 + i * 15, 10 + i * 15, 12 + i * 15)
    out.append(("paint-dash", doc([(content + text(20, 100, b"dashed"), {})] * 4), None))
    colors = [b"0.5 G 0.25 g", b"1 0 0 RG 0 1 0 rg", b"0 0 0 1 K 0.1 0.2 0.3 0.4 k", b"/DeviceRGB CS 0.1 0.2 0.3 SC",
              b"/DeviceGray cs 0.7 sc", b"/P0 cs /Pat scn", b"/P1 cs 0.2 0.3 0.4 /Pat scn", b"/Sep cs 0.5 scn",
              b"/DN cs 0.1 0.2 scn", b"/Idx cs 3 scn", b"1 G 2 g", b"(a) G", b"0.5 G 0.5 0.5 0.5 rg"]

    def res(b: Builder) -> dict:
        pat = b.stream(b"0 0 5 5 re f", Type=Name("Pattern"), PatternType=1, PaintType=1, TilingType=1,
                       BBox=[0, 0, 5, 5], XStep=5, YStep=5, Resources={})
        upat = b.stream(b"0 0 5 5 re f", Type=Name("Pattern"), PatternType=1, PaintType=2, TilingType=1,
                        BBox=[0, 0, 5, 5], XStep=5, YStep=5, Resources={})
        fn = {"FunctionType": 2, "Domain": [0, 1], "C0": [0, 0, 0], "C1": [1, 1, 1], "N": 1}
        return {"Font": {"F1": b.obj(HELVETICA)}, "Pattern": {"Pat": pat, "UPat": upat},
                "ColorSpace": {"P0": Name("Pattern"), "P1": [Name("Pattern"), Name("DeviceRGB")],
                               "Sep": [Name("Separation"), Name("Spot"), Name("DeviceRGB"), fn],
                               "DN": [Name("DeviceN"), [Name("A"), Name("B")], Name("DeviceRGB"), fn],
                               "Idx": [Name("Indexed"), Name("DeviceRGB"), 3, b"\x00" * 12]}}

    pages = []
    for i, c in enumerate(colors):
        pages.append((c + b" 3 w 20 20 m 180 180 l S 30 30 50 50 re B " + c.replace(b" G", b" g").replace(b" SC", b" sc")
                      + b"\n" + text(20, 150, b"colored %d" % i), {}))
    out.append(("paint-colors", doc(pages, res), None))
    widths = b"0 w 0 0 m 10 10 l S 0.5 w 20 0 m 30 10 l S 100 w 40 0 m 50 10 l S (x) w 60 0 m 70 10 l S -2 w 80 0 m 90 10 l S\n"
    out.append(("paint-widths", doc([(widths + SCENE, {})] * 2), None))
    return out


# ---------------------------------------------------------------------------------- images


def cases_images() -> list[Case]:
    out: list[Case] = []

    def res(b: Builder) -> dict:
        icc = b.stream(b"\x00" * 16, N=3)
        cs_arr = b.obj([Name("ICCBased"), icc])
        xs = {
            "Im1": b.stream(b"\x00\x00\x00", Subtype=Name("Image"), Width=1, Height=1, ColorSpace=Name("DeviceRGB"),
                            BitsPerComponent=8),
            "Im2": b.stream(b"\x00", Subtype=Name("Image"), Width=b.obj(1), Height=1, ColorSpace=cs_arr,
                            BitsPerComponent=b.obj(8)),
            "Im3": b.stream(b"\x00", Subtype=Name("Image"), Width=1, Height=1, ImageMask=True),
            "Im4": b.stream(b"\x00", Subtype=Name("Image"), Width=1, Height=1),
            "Im5": b.stream(b"\x00", Subtype=Name("Image"), Width=1, Height=1,
                            ColorSpace=[Name("Indexed"), Name("DeviceRGB"), 1, b"\x00" * 6]),
            "Im6": b.stream(b"\x00", Subtype=Name("Image"), W=2, H=3, CS=Name("G"), BPC=1, IM=False),
            "Im#ff": b.stream(b"\x00", Subtype=Name("Image"), Width=1, Height=1, ColorSpace=Name("DeviceGray")),
            "Form": b.stream(b"BI /W 1 /H 1 /CS /G /BPC 8 ID x EI\n 0 0 m 5 5 l S BT /F1 5 Tf (in form) Tj ET",
                             Type=Name("XObject"), Subtype=Name("Form"), BBox=[0, 0, 10, 10],
                             Resources={"Font": {"F1": b.obj(HELVETICA)}}),
        }
        return {"Font": {"F1": b.obj(HELVETICA)}, "XObject": xs}

    draw = b""
    for i, n in enumerate((b"Im1", b"Im2", b"Im3", b"Im4", b"Im5", b"Im6", b"Im#ff", b"Form")):
        draw += b"q 20 0 0 20 %d %d cm /%s Do Q\n" % (10 + (i % 4) * 45, 10 + (i // 4) * 45, n)
    inline = (b"q 10 0 0 10 150 150 cm BI /W 1 /H 1 /CS /RGB /BPC 8 ID abc EI Q\n"
              b"q 10 0 0 10 170 150 cm BI /W 2 /H 1 /CS /G /BPC 8 ID xy EI Q\n"
              b"q 10 0 0 10 150 170 cm BI /W 1 /H 1 /IM true ID x EI Q\n")
    out.append(("image-kinds", doc([(draw + inline + text(20, 190, b"images"), {})] * 4, res), None))
    out.append(("image-inline-many", doc([(inline * 5 + SCENE, {})] * 2), None))
    return out


# ---------------------------------------------------------------------------------- marked content


def cases_marked() -> list[Case]:
    out: list[Case] = []
    mcids = [b"0", b"7", b"-1", b"2.5", b"/N", b"(s)", b"[1 2]", b"<< /a 1 >>", b"null", b"true",
             b"123456789012345678901234567890", b"[" * 30 + b"1" + b"]" * 30, b"<feff00e9>"]
    content = b""
    for i, m in enumerate(mcids):
        content += b"/P << /MCID %s >> BDC " % m + text(10, 190 - i * 14, b"mcid %d" % i) + \
            b"%d %d m %d %d l S EMC\n" % (120, 190 - i * 14, 150, 190 - i * 14)
    out.append(("mc-mcid-kinds", doc([(content, {})] * 3), None))

    def res(b: Builder) -> dict:
        return {"Font": {"F1": b.obj(HELVETICA)},
                "Properties": {"MC0": {"MCID": 3}, "MC1": b.obj({"MCID": b.obj(4)}), "MC2": b.obj([1])}}

    content = (b"/Span /MC0 BDC " + text(10, 180, b"by name") + b"EMC "
               b"/Span /MC1 BDC " + text(10, 160, b"by ref") + b"EMC "
               b"/Span /MC2 BDC " + text(10, 140, b"not a dict") + b"EMC "
               b"/Span /Nope BDC " + text(10, 120, b"unknown") + b"EMC "
               b"/Artifact BMC 10 10 m 50 50 l S EMC "
               b"/A << /MCID 1 >> BDC /B << /MCID 2 >> BDC " + text(10, 100, b"nested") + b"EMC " + text(10, 80, b"outer")
               + b"EMC EMC EMC " + text(10, 60, b"after extra EMC")
               + b"/#ff#fe << /MCID 9 >> BDC " + text(10, 40, b"bytes tag") + b"EMC "
               + b"/Q 5 BDC " + text(10, 20, b"odd props") + b"EMC "
               + b"/Unclosed << /MCID 11 >> BDC " + text(100, 20, b"unclosed") + b"\n")
    out.append(("mc-props", doc([(content, {})] * 3, res), None))
    # a figure tagged (paint_path adds nothing after it)
    def fres(b: Builder) -> dict:
        form = b.stream(b"0 0 m 5 5 l S", Type=Name("XObject"), Subtype=Name("Form"), BBox=[0, 0, 10, 10])
        return {"Font": {"F1": b.obj(HELVETICA)}, "XObject": {"X": form}}

    content = b"/P << /MCID 5 >> BDC q 10 0 0 10 50 50 cm /X Do Q 0 0 m S EMC " + text(20, 20, b"t")
    out.append(("mc-figure", doc([(content, {})] * 2, fres), None))
    # an MCID nested 10^4 levels (resolve_all raises RecursionError long
    # before the C units of a comparison could run out: page values are
    # nested at most a few hundred levels), in arrays and in dicts
    deep = (b"/P << /MCID " + b"[" * 10000 + b"1" + b"]" * 10000 + b" >> BDC " + text(10, 100, b"deep list") + b"EMC "
            + b"/P << /MCID " + b"<< /a " * 10000 + b"1" + b" >>" * 10000 + b" >> BDC " + text(10, 50, b"deep dict")
            + b"EMC")
    out.append(("mc-mcid-deep", doc([(deep, {}), (text(10, 10, b"next page"), {})]), None))
    return out


# ---------------------------------------------------------------------------------- fonts


CP936 = [b"\xcb\xce\xcc\xe5", b"\xba\xda\xcc\xe5", b"\xbf\xac\xcc\xe5_GB2312", b"\xb7\xc2\xcb\xce_GB2312",
         b"\xc1\xa5\xca\xe9"]


def named_font(b: Builder, fontname: Any) -> Ref:
    desc = b.obj({"Type": Name("FontDescriptor"), "FontName": fontname, "Flags": 32, "FontBBox": [0, -200, 1000, 800],
                  "Ascent": 800, "Descent": -200, "ItalicAngle": 0, "StemV": 80, "CapHeight": 700})
    return b.obj({"Type": Name("Font"), "Subtype": Name("TrueType"), "BaseFont": Name("Arial"), "FirstChar": 32,
                  "LastChar": 126, "Widths": [500] * 95, "FontDescriptor": desc})


def cases_fonts() -> list[Case]:
    out: list[Case] = []
    names = CP936 + [b"ABCDEF+" + n for n in CP936] + [b"plain", b"\xff\xfe", b"A+B+C", b"+", b"x'y\"z\\", b""]

    def res(b: Builder) -> dict:
        fonts = {f"N{i}": named_font(b, n) for i, n in enumerate(names)}
        fonts["T3"] = type3(b)
        fonts["V"] = type0(b, Name("Identity-V"), W2=[1, [-1000, 500, 880]])
        fonts["H"] = type0(b, Name("Identity-H"), W=[1, [500, 600]])
        fonts["P"] = type0(b, Name("UniJIS-UCS2-H"))
        fonts["L"] = b.obj({"Type": Name("Font"), "Subtype": Name("Type1"), "BaseFont": Name("Helvetica"),
                            "FontDescriptor": b.obj({"Type": Name("FontDescriptor"), "FontName": Name("Helvetica"),
                                                     "Flags": 32})})
        fonts["F1"] = b.obj(HELVETICA)
        return {"Font": fonts}

    pages = []
    content = b""
    for i in range(len(names)):
        content += text(10 + (i % 3) * 65, 190 - (i // 3) * 14, b"N%d" % i, font=b"N%d" % i, size=9)
    pages.append((content, {}))
    pages.append((b"BT /T3 12 Tf 20 150 Td (aab) Tj ET BT /V 10 Tf 100 180 Td <00010002> Tj ET "
                  b"BT /H 10 Tf 20 100 Td <00010002> Tj ET BT /P 10 Tf 20 80 Td <3042> Tj ET "
                  b"BT /L 10 Tf 20 60 Td (lone) Tj ET BT /Nope 10 Tf 20 40 Td (missing) Tj ET " + text(20, 20, b"after"), {}))
    out.append(("font-kinds", doc(pages * 2, res), None))
    return out


# ---------------------------------------------------------------------------------- text


TOUNICODE_LIG = (b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CMapName /L def "
                 b"1 begincodespacerange <00> <FF> endcodespacerange 9 beginbfchar "
                 b"<61> <FB00> <62> <FB01> <63> <FB02> <64> <FB03> <65> <FB04> <66> <FB05> <67> <FB06> "
                 b"<68> <D800> <69> <DC00> endbfchar "
                 b"1 beginbfrange <6a> <6b> [<DBFF> <0020>] endbfrange endcmap CMapName currentdict /CMap defineresource pop end end")


def cases_text() -> list[Case]:
    out: list[Case] = []
    # duplicates and overlaps for dedupe_chars
    dup = b""
    for i, off in enumerate((0, 0.5, 0.99, 1, 1.01, 1.5, 2, 2.5, 3, 4)):
        y = 190 - i * 15
        dup += text(10, y, b"dup") + text(10 + off, y + off, b"dup")
    dup += b"BT /F2 10 Tf 100 190 Td (font) Tj ET " + text(100, 190, b"font")
    dup += text(100, 170, b"size", size=11) + text(100, 170, b"size")
    dup += b"1 0 0 rg " + text(100, 150, b"color") + b"0 g " + text(100, 150, b"color")
    dup += text(100, 130, b"aaaa") + text(100, 130, b"aaaa") + text(100, 130, b"aaaa")

    def res2(b: Builder) -> dict:
        return {"Font": {"F1": b.obj(HELVETICA), "F2": b.obj({**HELVETICA, "BaseFont": Name("Courier")})}}

    out.append(("text-dedupe", doc([(dup, {})] * 6, res2), None))

    def lres(b: Builder) -> dict:
        return {"Font": {"F1": b.obj(HELVETICA), "L": b.obj({**HELVETICA, "ToUnicode": b.stream(TOUNICODE_LIG)})}}

    lig = b""
    for i, s in enumerate((b"abcdefg", b"xaybz", b"hijk", b"ahb ijk", b"a a", b"g g g")):
        lig += text(10, 190 - i * 20, s, font=b"L")
    lig += text(10, 60, b"plain fi ff")
    out.append(("text-ligatures", doc([(lig, {})] * 6, lres), None))
    # words and lines at the tolerances (x 3, y 3), punctuation, spaces
    words = b""
    for i, gap in enumerate((0, 1, 2.9, 3, 3.1, 5)):
        y = 190 - i * 12
        words += text(10, y, b"ab") + text(10 + 11.12 + gap, y, b"cd")
    for i, dy in enumerate((0, 1, 2.9, 3, 3.1, 4)):
        words += text(110 + i * 15, 100, b"q") + text(110 + i * 15 + 6, 100 + dy, b"r")
    words += text(10, 60, b"a,b;c.d!e?f-g(h)i") + text(10, 45, b"   spaced    out   ") + text(10, 30, b"")
    words += text(10, 15, b"tab\tin") + text(100, 15, b"\x00\x01\x02")
    out.append(("text-tolerances", doc([(words, {})] * 8), None))
    rotated = (b"BT /F1 10 Tf 0 1 -1 0 100 20 Tm (rotated 90) Tj ET "
               b"BT /F1 10 Tf -1 0 0 -1 150 150 Tm (rotated 180) Tj ET "
               b"BT /F1 10 Tf 0 -1 1 0 20 180 Tm (rotated 270) Tj ET "
               b"BT /F1 10 Tf 0.7 0.7 -0.7 0.7 50 50 Tm (diagonal) Tj ET "
               b"BT /F1 10 Tf 1 0 0 -1 20 100 Tm (mirrored) Tj ET "
               b"BT /F1 10 Tf 1 0 0.5 1 120 120 Tm (skewed) Tj ET ")

    def vres(b: Builder) -> dict:
        return {"Font": {"F1": b.obj(HELVETICA), "V": type0(b, Name("Identity-V"), W2=[1, [-1000, 500, 880]])}}

    vertical = b"BT /V 10 Tf 180 190 Td <0001000200010002> Tj ET BT /V 10 Tf 165 190 Td <00020001> Tj ET "
    out.append(("text-rotated", doc([(rotated + vertical, {})] * 6, vres), None))
    sizes = (text(10, 190, b"zero", size=0) + text(10, 170, b"negative", size=-10) + text(10, 150, b"huge", size=150)
             + text(10, 20, b"tiny", size=0.01) + b"BT /F1 10 Tf 0 Tz 10 120 Td (no width) Tj ET "
             + b"BT /F1 10 Tf 3 Tr 10 100 Td (invisible) Tj ET BT 10 80 Td (no font) Tj ET")
    out.append(("text-sizes", doc([(sizes, {})] * 4), None))
    many_lines = b"".join(text(10 + (i % 3) * 60, 195 - (i // 3) * 6, b"w%d x" % i, size=5) for i in range(90))
    out.append(("text-dense", doc([(many_lines, {})] * 3), None))
    return out


# ---------------------------------------------------------------------------------- tables


def grid(x0: float, y0: float, cols: list[float], rows: list[float], op: bytes = b"lines") -> bytes:
    out = b""
    xs = [x0]
    for c in cols:
        xs.append(xs[-1] + c)
    ys = [y0]
    for r in rows:
        ys.append(ys[-1] + r)
    if op == b"lines":
        for y in ys:
            out += b"%s %s m %s %s l S\n" % (num(xs[0]), num(y), num(xs[-1]), num(y))
        for x in xs:
            out += b"%s %s m %s %s l S\n" % (num(x), num(ys[0]), num(x), num(ys[-1]))
    elif op == b"rects":
        for i in range(len(xs) - 1):
            for j in range(len(ys) - 1):
                out += b"%s %s %s %s re S\n" % (num(xs[i]), num(ys[j]), num(xs[i + 1] - xs[i]), num(ys[j + 1] - ys[j]))
    else:  # curves
        for y in ys:
            out += b"%s %s m %s %s %s %s %s %s c S\n" % (num(xs[0]), num(y), num(xs[0] + 1), num(y),
                                                         num(xs[-1] - 1), num(y), num(xs[-1]), num(y))
        for x in xs:
            out += b"%s %s m %s %s %s %s %s %s c S\n" % (num(x), num(ys[0]), num(x), num(ys[0] + 1),
                                                         num(x), num(ys[-1] - 1), num(x), num(ys[-1]))
    for i in range(len(xs) - 1):
        for j in range(len(ys) - 1):
            out += text(xs[i] + 2, ys[j] + 2, b"c%d%d" % (i, j), size=6)
    return out


def cases_tables() -> list[Case]:
    out: list[Case] = []
    for kind in (b"lines", b"rects", b"curves"):
        out.append((f"table-grid-{kind.decode()}", doc([(grid(20, 20, [40, 50, 60], [20, 25, 30, 35], kind), {})] * 6), None))
    # edges off by the tolerances (snap 3, join 3, intersection 3, edge_min_length 3)
    near = b""
    for i, d in enumerate((0, 1, 2.99, 3, 3.01, 4, 6, 8, 8.01)):
        y = 20 + i * 19
        near += b"20 %s m 100 %s l S 20 %s m 100 %s l S\n" % (num(y), num(y), num(y + 10), num(y + 10 + d))
        near += b"%s %s m %s %s l S %s %s m %s %s l S\n" % (num(20 + d), num(y), num(20), num(y + 10),
                                                            num(100), num(y), num(100 + d), num(y + 10))
        near += b"110 %s m %s %s l S %s %s m 190 %s l S\n" % (num(y), num(140), num(y), num(140 + d), num(y), num(y))
        near += b"%s %s m %s %s l S\n" % (num(150), num(y - d / 2), num(150), num(y + 10))
        near += text(25, y + 2, b"t%d" % i, size=6) + text(115, y + 2, b"u%d" % i, size=6)
    out.append(("table-tolerances", doc([(near, {})] * 10), None))
    short = b"".join(b"%d 20 m %d %s l S 10 %d m %s %d l S\n" % (20 + 15 * i, 20 + 15 * i, num(20 + f), 30 + 15 * i,
                                                              num(10 + f), 30 + 15 * i)
                     for i, f in enumerate((0, 0.5, 1, 2.99, 3, 3.01, 10, 20, 21)))
    out.append(("table-short-edges", doc([(short + grid(100, 100, [30, 30], [20, 20]), {})] * 6), None))
    # the text strategy: aligned words
    cols = b""
    for r in range(8):
        for c, w in enumerate((b"name", b"12.5", b"x", b"long text")):
            cols += text(15 + c * 45, 180 - r * 12, w + (b"%d" % r if r % 2 else b""), size=7)
    out.append(("table-text-columns", doc([(cols, {})] * 8), None))
    nested = (grid(10, 10, [90, 90], [90, 90]) + grid(15, 15, [40, 40], [40, 40], b"rects"))
    out.append(("table-nested", doc([(nested, {})] * 4), None))

    def fres(b: Builder) -> dict:
        form = b.stream(grid(0, 0, [30, 30], [20, 20]), Type=Name("XObject"), Subtype=Name("Form"), BBox=[0, 0, 100, 100],
                        Resources={"Font": {"F1": b.obj(HELVETICA)}})
        return {"Font": {"F1": b.obj(HELVETICA)}, "XObject": {"G": form}}

    out.append(("table-in-figure", doc([(b"q 1 0 0 1 20 20 cm /G Do Q q 2 0 0 2 100 100 cm /G Do Q", {})] * 4, fres), None))
    tiny = b"".join(b"%s %s 0.5 0.5 re f\n" % (num(10 + (i % 70) * 2.5), num(10 + (i // 70) * 2.5)) for i in range(3000))
    tiny += b"".join(text(10 + (i % 40) * 4.5, 190 - (i // 40) * 4, b"%c" % (97 + i % 26), size=3) for i in range(800))
    out.append(("table-many-objects", doc([(tiny, {})] * 2), None))
    return out


# ---------------------------------------------------------------------------------- annotations


def cases_annots() -> list[Case]:
    out: list[Case] = []
    b = Builder()
    font = b.obj(HELVETICA)
    page_ref = b.reserve()
    variants = [
        {"Rect": DROP},
        {"Rect": [10, 10, 50]},
        {"Rect": [10, 10, Name("x"), 50]},
        {"Rect": [50, 50, 10, 10]},
        {"Rect": b.obj([10, 10, 50, 50])},
        {"A": [1, 2]},
        {"A": b.obj({"URI": b"https://ref.example"})},
        {"A": {"URI": Name("name-uri")}},
        {"A": {"URI": 7}},
        {"A": {"URI": b"\xff\xfe\x00"}},
        {"A": {"URI": b"\xfe\xff\x00h\x00i"}},
        {"A": {"URI": b"caf\xc3\xa9"}},
        {"A": {"URI": b.obj(b"https://indirect.example")}},
        {"A": {"S": Name("GoTo"), "D": [page_ref, Name("XYZ"), 0, 0, 1]}},
        {"A": DROP},
        {"T": b"title", "Contents": b"\xfe\xff\x00c\x00o"},
        {"T": b"\x80\x81", "Contents": b"\xff\xfe\xd8\x00"},
        {"P": page_ref},
        {"P": b.obj({"Type": Name("Page")})},
        {"Parent": b.obj({"Type": Name("Annot")})},
        {"Subtype": Name("Text"), "Contents": b"note"},
    ]
    annots = [link_annot(b, [10 + (i % 5) * 35, 10 + (i // 5) * 35, 40 + (i % 5) * 35, 40 + (i // 5) * 35], **v)
              for i, v in enumerate(variants)]
    b.put(page_ref, {"Type": Name("Page"), "Parent": Ref(2), "MediaBox": [0, 0, 200, 200],
                     "Contents": b.stream(SCENE), "Resources": {"Font": {"F1": font}}, "Annots": annots})
    b.pages.append(page_ref.num)
    b.page(SCENE, {"Font": {"F1": font}}, Annots=b.obj(annots[:3]), Rotate=90)
    b.page(SCENE, {"Font": {"F1": font}}, Annots={"A": 1})
    b.page(SCENE, {"Font": {"F1": font}}, Annots=Ref(9999))
    b.page(SCENE, {"Font": {"F1": font}}, Annots=[Ref(9998), None, 5, link_annot(b, [0, 0, 1, 1])])
    out.append(("annot-malformed", b.build(), None))
    return out


# ---------------------------------------------------------------------------------- layout


def cases_layout() -> list[Case]:
    out: list[Case] = []
    # boxes at equal distances from each other: the analysis' id() decides
    eq = b""
    for i in range(3):
        for j in range(3):
            eq += text(20 + i * 60, 180 - j * 60, b"box %d %d" % (i, j))
    out.append(("layout-equal-distances", doc([(eq, {})] * 4), None))
    sym = text(50, 100, b"left") + text(130, 100, b"right") + text(90, 140, b"top") + text(90, 60, b"bottom")
    out.append(("layout-symmetric", doc([(sym, {})] * 3), None))

    def fres(b: Builder) -> dict:
        inner = b.stream(text(0, 0, b"inner text") + b"0 0 m 10 10 l S", Type=Name("XObject"), Subtype=Name("Form"),
                         BBox=[0, 0, 100, 20], Resources={"Font": {"F1": b.obj(HELVETICA)}})
        outer = b.stream(b"q 1 0 0 1 0 20 cm /I Do Q " + text(0, 0, b"outer text"), Type=Name("XObject"),
                         Subtype=Name("Form"), BBox=[0, 0, 100, 50],
                         Resources={"Font": {"F1": b.obj(HELVETICA)}, "XObject": {"I": inner}})
        return {"Font": {"F1": b.obj(HELVETICA)}, "XObject": {"O": outer, "E": b.stream(
            b"", Type=Name("XObject"), Subtype=Name("Form"), BBox=[0, 0, 1, 1])}}

    content = b"q 1 0 0 1 20 100 cm /O Do Q q 1 0 0 1 100 20 cm /O Do Q /E Do " + text(20, 20, b"page text")
    out.append(("layout-figures", doc([(content, {})] * 4, fres), None))
    return out


# ---------------------------------------------------------------------------------- depth


def nested_array(n: int, leaf: bytes = b"1") -> bytes:
    return b"[" * n + leaf + b"]" * n


def ref_chain(b: Builder, n: int, leaf: Any) -> Ref:
    """``n`` objects, each an array holding a reference to the next."""
    refs = [b.reserve() for _ in range(n)]
    for i, r in enumerate(refs):
        b.put(r, [refs[i + 1]] if i + 1 < n else [leaf])
    return refs[0]


def depth_kinds() -> dict[str, tuple[Any, tuple[str, ...]]]:
    """The documents of ``cases_depth`` from the nesting ``n``, and the
    stages that check them."""
    objs = ("plumber_objects",)
    show = text(10, 10, b"deep")

    def colorspace_list(n: int) -> bytes:
        def res(b: Builder) -> dict:
            im = b.stream(b"\x00", Subtype=Name("Image"), Width=1, Height=1, ColorSpace=[Raw(nested_array(n, b"/G"))])
            return {"Font": {"F1": b.obj(HELVETICA)}, "XObject": {"Im": im}}
        return doc([(b"q 10 0 0 10 50 50 cm /Im Do Q " + show, {})], res)

    def colorspace_refs(n: int) -> bytes:
        def res(b: Builder) -> dict:
            im = b.stream(b"\x00", Subtype=Name("Image"), Width=1, Height=1, ColorSpace=ref_chain(b, n, Name("G")))
            return {"Font": {"F1": b.obj(HELVETICA)}, "XObject": {"Im": im}}
        return doc([(b"q 10 0 0 10 50 50 cm /Im Do Q " + show, {})], res)

    def mcid_list(n: int) -> bytes:
        return doc([(b"/P << /MCID " + nested_array(n) + b" >> BDC " + show + b"EMC", {})])

    def mcid_refs(n: int) -> bytes:
        def res(b: Builder) -> dict:
            return {"Font": {"F1": b.obj(HELVETICA)}, "Properties": {"M": {"MCID": ref_chain(b, n, 1)}}}
        return doc([(b"/P /M BDC " + show + b"EMC", {})], res)

    def dash_list(n: int) -> bytes:
        return doc([(nested_array(n) + b" 0 d 10 10 50 50 re S " + show, {})] * 20)

    return {
        "figures": (lambda n: form_chain(n, show, {"Font": {"F1": HELVETICA}}), ("plumber_objects", "plumber_text")),
        "figures-uncached": (lambda n: form_chain(n, b"0 0 m 5 5 l S", precache=0), objs),
        "colorspace-list": (colorspace_list, objs),
        "colorspace-refs": (colorspace_refs, objs),
        "mcid-list": (mcid_list, objs + ("plumber_text",)),
        "mcid-refs": (mcid_refs, objs),
        "dash-list": (dash_list, ("plumber_doc",)),
    }


def cases_depth() -> list[Case]:
    out: list[Case] = []
    for label, (make, stages) in depth_kinds().items():
        levels: set[int] = set()
        bounds = []
        for stage in stages:
            first = first_failing(stage, make, lo=2, hi=1200)
            bounds.append(f"{stage} {first}")
            levels.update(range(first - 2, first + 2))
        print(f"depth-{label}: first failing " + ", ".join(bounds), file=sys.stderr)
        for n in sorted(levels):
            out.append((f"depth-{label}-{n}", make(n), None))
    return out


def generate(depth: bool = True) -> None:
    cases = (cases_boxes() + cases_geometry() + cases_paint() + cases_images() + cases_marked() + cases_fonts()
             + cases_text() + cases_tables() + cases_annots() + cases_layout())
    if depth:
        cases += cases_depth()
    write_corpus("plumber", cases)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--no-depth", action="store_true",
                    help="skip cases_depth (minutes of bisection): a corpus for development, not the reference one")
    generate(depth=not ap.parse_args().no_depth)


if __name__ == "__main__":
    main()
