"""Adversarial PDFs for the interpreter: the ``interp``, ``interp_text``,
``fonts``, ``fonts_text`` and ``layout`` stages (``--set adv-interp``).

    PDFPLUMBER_ROOT=<main checkout> python -m oracle.adversarial.interp

writes ``<cache>/corpus/adv-interp/`` (same bytes on every run, but for the
depth windows, whose bounds are measured on the oracle: see
``cases_depth``). The cases (pdfminer/pdfinterp.py, pdfminer/pdfdevice.py
and what they call):

- ``op-<operator>``: every operator of ``PDFPageInterpreter`` with no
  operand, one too few, one too many, and each operand position given a
  value of every kind the content parser yields (ints, big ints, floats,
  infinite floats, bools, ``null``, names, strings, numeric strings such as
  ``(nan)`` and ``(1e400)`` that ``float()`` reads, arrays, dicts, keywords
  are not operands), one page per variant (an exception stops a page);
  keywords spelled like the methods (``T_a``, ``_q``, ``f_a``) and
  keywords that decode to an operator (``q\\x80``);
- ``text-*``: ``Tz``/``TL``/``Ts``/``Tc``/``Tw`` at 0, negative, huge and
  infinite values; ``TJ`` with numbers of every kind (bools, big ints,
  infinite floats), nested arrays, names, dicts, and ``TJ`` on a number, a
  string, a name, a dict, an inline image stream; word spacing on code 32
  with single-byte and multibyte fonts; ``'`` and ``"``; ``Td``/``TD``/
  ``T*``/``Tm`` with odd operands;
- ``font-*``: Type 3 fonts with odd ``/FontMatrix`` (ints, short, strings,
  arrays, missing), vertical ``Type0`` fonts (``Identity-V``, ``/W2``,
  ``/DW2``), a predefined CMap, an unknown CMap, missing and odd font
  resources (``Tf`` of an unknown name, a font that is not a dict, a
  reference to nothing, the same font by reference on several pages: the
  resource manager's cache);
- ``color-*``: every color space resource (``ICCBased`` with ``/N`` of
  every kind, ``DeviceN``, ``Indexed``, ``Separation``, ``Pattern``, an
  empty array, a name that is not a color space), ``CS``/``cs``/``SC``/
  ``sc``/``SCN``/``scn`` with 0 to 5 components, patterns colored and
  uncolored, ``G``/``g``/``RG``/``rg``/``K``/``k`` with odd operands;
- ``path-*``: ``m``/``l``/``c``/``v``/``y``/``h``/``re`` and every
  painting operator, with odd operands, several subpaths, no path; ``d``,
  ``w`` after degenerate ``cm`` (zeros, huge, infinite, NaN through
  strings); ``q``/``Q`` unbalanced;
- ``inline-*``: inline images at the content stream's block boundaries
  (4096 bytes), across two content streams, without ``/W``/``/H``, with an
  odd dictionary, an ASCII85 ``/F`` (``~>``), ``EI`` inside the data,
  ``ID`` without ``BI``;
- ``mc-*``: ``BMC``/``BDC``/``EMC``/``MP``/``DP`` unbalanced and with every
  kind of tag and property operand;
- ``xobj-*``: Form XObjects without ``/BBox``, with odd ``/BBox`` and
  ``/Matrix``, without ``/Resources`` (the parent's, copied), with
  resources that are not a dict, images with and without ``/Width``, an
  unknown name, a name that is not a stream, references that cycle (a form
  that draws itself, two forms that draw each other, a page's content
  stream drawn as a form), content arrays with broken members;
- ``depth-*`` (``cases_depth``): chains of nested Form XObjects around the
  first nesting depth at which Python raises ``RecursionError`` in each
  checked stage, the deepest point being each time another path.
"""

from __future__ import annotations

import argparse
import os
import sys
import tempfile
from typing import Any

from ._common import write_corpus
from ._pdf import Doc, Name, Raw, Ref, classic

Case = tuple[str, bytes, "str | None"]

HELVETICA = {"Type": Name("Font"), "Subtype": Name("Type1"), "BaseFont": Name("Helvetica")}


class Builder:
    """A document: object 1 the catalog, 2 the page tree, then the pages
    and the objects added, numbered in order."""

    def __init__(self) -> None:
        self.objects: list[tuple[int, Any, bytes | None]] = []
        self.pages: list[int] = []
        self.next = 3

    def obj(self, value: Any, stream: bytes | None = None) -> Ref:
        num = self.next
        self.next += 1
        self.objects.append((num, value, stream))
        return Ref(num)

    def stream(self, data: bytes, **attrs: Any) -> Ref:
        return self.obj(attrs, data)

    def reserve(self) -> Ref:
        """A number for an object added later with ``put``."""
        num = self.next
        self.next += 1
        return Ref(num)

    def put(self, ref: Ref, value: Any, stream: bytes | None = None) -> None:
        self.objects.append((ref.num, value, stream))

    def page(self, content: Any, resources: Any = None, **attrs: Any) -> Ref:
        """A page; ``content`` is the bytes of one content stream, or the
        ``/Contents`` value itself (a ``Ref``, a list, ``Raw``)."""
        if isinstance(content, (bytes, bytearray)) and not isinstance(content, Raw):
            content = self.stream(bytes(content))
        d: dict = {"Type": Name("Page"), "Parent": Ref(2), "MediaBox": [0, 0, 200, 200]}
        if content is not None:
            d["Contents"] = content
        if resources is not None:
            d["Resources"] = resources
        d.update(attrs)
        ref = self.obj(d)
        self.pages.append(ref.num)
        return ref

    def build(self) -> bytes:
        doc = Doc()
        doc.add(1, {"Type": Name("Catalog"), "Pages": Ref(2)})
        kids = [Ref(n) for n in self.pages]
        doc.add(2, {"Type": Name("Pages"), "Kids": kids, "Count": len(kids)})
        for num, value, stream in sorted(self.objects, key=lambda o: o[0]):
            doc.add(num, value, stream=stream)
        return classic(doc, {"Root": Ref(1)})


def one_page(content: bytes, resources: Any = None, **attrs: Any) -> bytes:
    b = Builder()
    b.page(content, resources if resources is not None else {"Font": {"F1": b.obj(HELVETICA)}}, **attrs)
    return b.build()


def pages_doc(contents: list[bytes], resources: Any = None) -> bytes:
    """One page per content stream (at most 20: the stages' default)."""
    assert len(contents) <= 20, len(contents)
    b = Builder()
    font = b.obj(HELVETICA)
    res = resources if resources is not None else {"Font": {"F1": font}}
    for c in contents:
        b.page(c, res)
    return b.build()


# ---------------------------------------------------------------------------------- operands

#: Operands of every kind the content parser yields (content stream syntax).
OPERANDS: dict[str, bytes] = {
    "int": b"3",
    "zero": b"0",
    "neg": b"-2",
    "bigint": b"123456789012345678901234567890",
    "hugeint": b"1" + b"0" * 330,
    "float": b"1.5",
    "negzero": b"-0.0",
    "inf": b"1" + b"0" * 400 + b".5",
    "true": b"true",
    "false": b"false",
    "null": b"null",
    "name": b"/F1",
    "badname": b"/#ff#fe",
    "string": b"(abc)",
    "empty": b"()",
    "numstr": b"( 2.5 )",
    "nanstr": b"(nan)",
    "infstr": b"(-inf)",
    "bigstr": b"(1e400)",
    "intstr": b"(12)",
    "hex": b"<414243>",
    "array": b"[1 2 3]",
    "emptyarray": b"[]",
    "nested": b"[[1] (x)]",
    "dict": b"<< /A 1 /MCID 3 >>",
    "emptydict": b"<< >>",
}

#: The operators and their number of operands (pdfminer/pdfinterp.py).
OPERATORS: dict[str, int] = {
    "q": 0, "Q": 0, "cm": 6, "w": 1, "J": 1, "j": 1, "M": 1, "d": 2, "ri": 1, "i": 1, "gs": 1,
    "m": 2, "l": 2, "c": 6, "v": 4, "y": 4, "h": 0, "re": 4, "S": 0, "s": 0, "f": 0, "F": 0,
    "f*": 0, "B": 0, "B*": 0, "b": 0, "b*": 0, "n": 0, "W": 0, "W*": 0, "CS": 1, "cs": 1,
    "G": 1, "g": 1, "RG": 3, "rg": 3, "K": 4, "k": 4, "SCN": 0, "scn": 0, "SC": 0, "sc": 0,
    "sh": 1, "BT": 0, "ET": 0, "BX": 0, "EX": 0, "MP": 1, "DP": 2, "BMC": 1, "BDC": 2, "EMC": 0,
    "Tc": 1, "Tw": 1, "Tz": 1, "TL": 1, "Tf": 2, "Tr": 1, "Ts": 1, "Td": 2, "TD": 2, "Tm": 6,
    "T*": 0, "TJ": 1, "Tj": 1, "'": 1, '"': 3, "EI": 1, "Do": 1,
}

#: What makes the effect of an operator visible in the traces: a path
#: painted, a string shown (with the text state and the colors).
SHOW = b" 0 0 m 10 10 l 5 5 m 7 7 l S BT /F1 10 Tf 20 30 Td (Ab c) Tj [(x) -250 (y)] TJ ET\n"

#: Valid operands of each operator, to vary one position at a time.
VALID: dict[str, list[bytes]] = {
    "cm": [b"2", b"0", b"0", b"2", b"5", b"5"], "w": [b"2"], "J": [b"1"], "j": [b"1"], "M": [b"4"],
    "d": [b"[3 1]", b"0"], "ri": [b"/Perceptual"], "i": [b"1"], "gs": [b"/GS1"], "m": [b"1", b"2"],
    "l": [b"3", b"4"], "c": [b"1", b"2", b"3", b"4", b"5", b"6"], "v": [b"1", b"2", b"3", b"4"],
    "y": [b"1", b"2", b"3", b"4"], "re": [b"1", b"2", b"30", b"40"], "CS": [b"/DeviceRGB"],
    "cs": [b"/DeviceRGB"], "G": [b"0.5"], "g": [b"0.5"], "RG": [b"1", b"0", b"0"], "rg": [b"1", b"0", b"0"],
    "K": [b"0", b"1", b"0", b"0"], "k": [b"0", b"1", b"0", b"0"], "sh": [b"/Sh1"], "MP": [b"/Tag"],
    "DP": [b"/Tag", b"<< /MCID 1 >>"], "BMC": [b"/Span"], "BDC": [b"/Span", b"<< /MCID 2 >>"],
    "Tc": [b"1"], "Tw": [b"2"], "Tz": [b"50"], "TL": [b"12"], "Tf": [b"/F1", b"9"], "Tr": [b"1"],
    "Ts": [b"3"], "Td": [b"5", b"6"], "TD": [b"5", b"6"], "Tm": [b"1", b"0", b"0", b"1", b"50", b"60"],
    "TJ": [b"[(ab) 100 (c)]"], "Tj": [b"(ab)"], "'": [b"(ab)"], '"': [b"1", b"2", b"(ab)"], "EI": [b"1"],
    "Do": [b"/Im1"],
}

#: Operators that need a text object around them to show something.
TEXT_OPS = {"TJ", "Tj", "'", '"', "Tc", "Tw", "Tz", "TL", "Tf", "Tr", "Ts", "Td", "TD", "Tm", "T*"}


def op_resources(b: Builder) -> dict:
    """Resources the operator cases may name."""
    image = b.stream(b"\x00\xff", Type=Name("XObject"), Subtype=Name("Image"), Width=1, Height=1,
                     BitsPerComponent=8, ColorSpace=Name("DeviceGray"))
    return {
        "Font": {"F1": b.obj(HELVETICA)},
        "XObject": {"Im1": image},
        "ExtGState": {"GS1": {"LW": 3}},
        "ColorSpace": {"CS0": [Name("ICCBased"), b.stream(b"", N=3)]},
    }


def op_line(op: str, operands: list[bytes]) -> bytes:
    body = b" ".join(operands + [op.encode("latin-1")])
    if op in TEXT_OPS:
        return b"BT /F1 10 Tf 10 20 Td " + body + b" (Z) Tj ET"
    return body


def cases_operators() -> list[Case]:
    out: list[Case] = []
    kinds = list(OPERANDS.values())
    for op, nargs in OPERATORS.items():
        valid = VALID.get(op, [])
        variants: list[bytes] = []
        variants.append(op_line(op, []))  # no operand (or the right count when none)
        if nargs:
            variants.append(op_line(op, valid[:-1]))  # one too few
            variants.append(op_line(op, [b"(extra)"] + valid))  # one too many: the last ones are taken
        else:
            variants.append(op_line(op, [b"1 /X (s)"]))  # operands left on the stack
        # each kind of operand in the first, then in the last position
        positions = sorted({0, nargs - 1}) if nargs else []
        for pos in positions:
            chunk = []
            for v in kinds:
                operands = list(valid)
                operands[pos] = v
                chunk.append(op_line(op, operands))
            # several operand kinds per page (an exception stops a page)
            for i in range(0, len(chunk), 3):
                variants.append(b"\n".join(chunk[i : i + 3]))
        pages: list[bytes] = [v + b"\n" + SHOW for v in variants]
        safe = op.replace("*", "_a").replace("'", "_q").replace('"', "_w")
        for i in range(0, len(pages), 20):
            b = Builder()
            res = op_resources(b)
            for c in pages[i : i + 20]:
                b.page(c, res)
            out.append((f"op-{safe}-{i // 20}", b.build(), None))
    # keywords spelled like the methods, and keywords that decode to an operator
    spelled = [
        b"q 2 0 0 2 0 0 cm _q", b"BT /F1 10 Tf 12 TL T_a (x) Tj ET", b"0 0 m 5 5 l f_a 0 0 m 5 5 l B_a",
        b"0 0 m 5 5 l b_a W_a n", b"BT /F1 10 Tf (x) _q 1 2 (y) _w ET", b"q\x80 2 0 0 2 0 0 cm Q\xff",
        b"BT /F1 10 Tf (a) T\xc3j ET", b"do_q q", b"BT /F1 10 Tf 0 -12 Td (b) Tj T\xff* (c) Tj ET",
        b"BI ID EI", b"1 2 3 R", b"BT /F1 10 Tf <<>> TJ ET", b"0 0 m 1 1 l S unknownop 3 4 l S",
    ]
    out.append(("op-spelled", pages_doc([s + b"\n" + SHOW for s in spelled]), None))
    return out


# ---------------------------------------------------------------------------------- text


def text_doc(contents: list[bytes], extra_fonts: dict | None = None) -> bytes:
    b = Builder()
    fonts: dict = {"F1": b.obj(HELVETICA)}
    for name, spec in (extra_fonts or {}).items():
        fonts[name] = spec(b) if callable(spec) else b.obj(spec)
    for c in contents:
        b.page(c, {"Font": fonts})
    return b.build()


def type0(b: Builder, encoding: Any = Name("Identity-H"), **desc: Any) -> Ref:
    d: dict = {"Type": Name("Font"), "Subtype": Name("CIDFontType2"), "BaseFont": Name("Test"),
               "CIDSystemInfo": {"Registry": b"Adobe", "Ordering": b"Identity", "Supplement": 0}}
    d.update(desc)
    return b.obj({"Type": Name("Font"), "Subtype": Name("Type0"), "BaseFont": Name("Test"),
                  "Encoding": encoding, "DescendantFonts": [b.obj(d)]})


def cases_text() -> list[Case]:
    out: list[Case] = []
    nums = [b"0", b"-50", b"100", b"1e3", b"1000000000000", b"(nan)", b"(inf)", b"(-inf)", b"0.0001", OPERANDS["inf"]]
    for op in ("Tz", "TL", "Ts", "Tc", "Tw"):
        contents = [b"BT /F1 10 Tf %s %s 10 20 Td (A B) Tj [(C) -300 ( D)] TJ T* (E) ' ET" % (v, op.encode())
                    for v in nums]
        contents.append(b"BT /F1 10 Tf 2 Tc 3 Tw 50 Tz (a b) Tj 0 Tz (c d) Tj ET")
        out.append((f"text-{op}", text_doc(contents), None))
    tj = [
        b"[(a) 1 (b) 2.5 (c) true (d) false (e)]", b"[(a) 123456789012345678901234567890 (b)]",
        b"[(a) %s (b)]" % OPERANDS["inf"], b"[(a) [(nested)] (b)]", b"[(a) /Name (b) << /A 1 >> (c) null]",
        b"[]", b"[[]]", b"5", b"2.5", b"(abc)", b"<00410042>", b"/Name", b"<< /A 1 /B 2 >>", b"true",
        b"null", b"[(a) (b) (c)]", b"[-1000 (a) -1000]", b"[(\x20\x20) 0 (\x20)]",
    ]
    contents = [b"BT /F1 10 Tf 1 Tc 2 Tw 10 20 Td %s TJ (z) Tj ET" % s for s in tj]
    contents.append(b"BT /F1 10 Tf 10 20 Td BI /W 1 /H 1 /F /A85 ID z~> TJ (after) Tj ET")
    for i in range(0, len(contents), 10):
        out.append((f"text-TJ-{i // 10}", text_doc(contents[i : i + 10]), None))
    # word spacing: single-byte and multibyte fonts; ' and "
    spacing = [
        b"BT /F1 10 Tf 5 Tw 1 Tc 10 20 Td (a b  c) Tj ET",
        b"BT /F2 10 Tf 5 Tw 1 Tc 10 20 Td <0020002100200041> Tj [<0020> -500 <0020>] TJ ET",
        b"BT /F2 10 Tf 5 Tw 10 20 Td <00> Tj <002000> Tj ET",
        b"BT /F1 10 Tf 12 TL (a) ' 4 1 (b c) \" -12 TL (d) ' ET",
        b"BT /F1 10 Tf 12 TL 1 2 TD (a) Tj 3 (x) TD (b) Tj (x) 4 Td (c) Tj T* (d) Tj ET",
        b"BT /F1 10 Tf 1 2 3 4 5 6 Tm (a) Tj (1) 2 3 4 5 6 Tm (b) Tj 0 0 0 0 0 0 Tm (c) Tj ET",
        b"BT /F1 10 Tf 3 Tr (a) Tj 1.7 Tr (b) Tj (x) Tr (c) Tj %s Tr (d) Tj ET" % OPERANDS["inf"],
        b"BT (no font) Tj [(no font)] TJ ET",
        b"BT /F1 10 Tf ET (outside BT) Tj BT BT (nested) Tj ET ET",
        b"BT /F1 (size) Tf (a) Tj /F1 %s Tf (b) Tj ET" % OPERANDS["inf"],
    ]
    out.append(("text-spacing", text_doc(spacing, {"F2": lambda b: type0(b)}), None))
    return out


# ---------------------------------------------------------------------------------- fonts


def type3(b: Builder, matrix: Any = None, **extra: Any) -> Ref:
    glyph = b.stream(b"1000 0 0 0 750 750 d1 0 0 750 750 re f")
    d: dict = {"Type": Name("Font"), "Subtype": Name("Type3"), "FontBBox": [0, 0, 750, 750],
               "CharProcs": {"a": glyph}, "Encoding": {"Type": Name("Encoding"), "Differences": [97, Name("a")]},
               "FirstChar": 97, "LastChar": 98, "Widths": [1000, 500]}
    if matrix is not None:
        d["FontMatrix"] = matrix
    d.update(extra)
    return b.obj(d)


def cases_fonts() -> list[Case]:
    out: list[Case] = []
    show = b"BT /T3 12 Tf 2 Tc 10 20 Td (aab) Tj [(a) -100 (b)] TJ ET\n"
    matrices = [
        None, [0.001, 0, 0, 0.001, 0, 0], [1, 0, 0, 1, 0, 0], [0.002, 0.001, 0, 0.002, 5, 5], [0.001, 0, 0],
        [Raw(b"(a)"), 0, 0, Raw(b"(b)"), 0, 0], [[1], 0, 0, [2], 0, 0], [0, 0, 0, 0, 0, 0], Name("Bad"),
        [Raw(b"1" + b"0" * 330), 0, 0, 1, 0, 0], [-0.001, 0, 0, -0.001, 0, 0], [True, 0, 0, False, 0, 0],
    ]
    for i, m in enumerate(matrices):
        b = Builder()
        b.page(show, {"Font": {"T3": type3(b, m), "F1": b.obj(HELVETICA)}})
        out.append((f"font-type3-matrix-{i}", b.build(), None))
    b = Builder()
    b.page(show, {"Font": {"T3": type3(b, [0.001, 0, 0, 0.001, 0, 0], Resources={"Font": {"F1": b.obj(HELVETICA)}})}})
    out.append(("font-type3-resources", b.build(), None))
    # vertical fonts
    vertical = [
        {},
        {"W2": [1, [-1000, 500, 880]], "DW2": [880, -1000]},
        {"W2": [1, 3, -500, 400, 800], "W": [1, [500, 600]]},
        {"DW2": [Raw(b"(x)"), Raw(b"(y)")]},
        {"W2": [1, [Raw(b"/N"), 2, 3]]},
    ]
    contents = b"BT /V 10 Tf 1 Tc 3 Tw 10 150 Td <000100020020> Tj [<0001> 300 <0003>] TJ ET\n"
    for i, desc in enumerate(vertical):
        b = Builder()
        b.page(contents, {"Font": {"V": type0(b, Name("Identity-V"), **desc)}})
        out.append((f"font-vertical-{i}", b.build(), None))
    # predefined and unknown CMaps, odd encodings
    for label, enc in [("unijis", Name("UniJIS-UCS2-H")), ("unijis-v", Name("UniJIS-UCS2-V")),
                       ("unknown", Name("NoSuchCMap-H")), ("dict", {"CMapName": Name("Identity-H")}),
                       ("number", 5), ("ref-missing", Ref(999))]:
        b = Builder()
        b.page(b"BT /C 10 Tf 10 100 Td <30423044> Tj <0041> Tj ET", {"Font": {"C": type0(b, enc)}})
        out.append((f"font-cmap-{label}", b.build(), None))
    # missing and odd font resources; the cache across pages
    b = Builder()
    font = b.obj(HELVETICA)
    tt = b.obj({"Type": Name("Font"), "Subtype": Name("TrueType"), "BaseFont": Name("Arial")})
    odd = {
        "F1": font, "F2": font, "TT": tt, "Missing": Ref(999), "Num": 5, "Str": b"x", "Arr": [1],
        "Direct": {"Type": Name("Font"), "Subtype": Name("Type1"), "BaseFont": Name("Courier")},
        "NoSub": {"Type": Name("Font"), "BaseFont": Name("Times-Roman")},
        "Odd": {"Type": Name("Font"), "Subtype": Name("Weird")},
        "Zero": Ref(0),
    }
    shows = [b"BT /%s 10 Tf 10 20 Td (ab) Tj ET" % k.encode() for k in odd]
    b.page(b"\n".join(shows), {"Font": odd})
    b.page(b"BT /F1 10 Tf (a) Tj /Nope 10 Tf (b) Tj /Nope 10 Tf (c) Tj 5 10 Tf (d) Tj ET", {"Font": {"F1": font}})
    b.page(b"BT /F1 10 Tf (a) Tj /TT 8 Tf (b) Tj ET", {"Font": {"F1": font, "TT": tt}})
    b.page(b"BT /F1 10 Tf (a) Tj ET", {"Font": Ref(999)})
    b.page(b"BT /F1 10 Tf (a) Tj ET", {"Font": [font]})
    out.append(("font-resources", b.build(), None))
    return out


# ---------------------------------------------------------------------------------- colors


def cases_colors() -> list[Case]:
    out: list[Case] = []
    b = Builder()
    icc = lambda n: [Name("ICCBased"), b.stream(b"", N=n)]  # noqa: E731
    spaces: dict = {
        "I1": icc(1), "I3": icc(3), "I4": icc(4), "I0": icc(0), "INeg": icc(-2), "IBig": icc(2**70),
        "IFloat": icc(2.0), "IZeroF": icc(0.0), "ITrue": icc(True), "IFalse": icc(False), "IName": icc(Name("x")),
        "IRef": icc(Ref(999)), "IStr": icc(b"3"), "INone": [Name("ICCBased"), b.stream(b"")],
        "INotStream": [Name("ICCBased"), 5], "IShort": [Name("ICCBased")],
        "DN": [Name("DeviceN"), [Name("a"), Name("b")], Name("DeviceRGB"), {}],
        "DNBad": [Name("DeviceN"), 5], "Idx": [Name("Indexed"), Name("DeviceRGB"), 1, b"\x00\x00\x00\xff\xff\xff"],
        "Sep": [Name("Separation"), Name("Spot"), Name("DeviceCMYK"), {}], "Pat": [Name("Pattern"), Name("DeviceRGB")],
        "PatName": Name("Pattern"), "Lab": [Name("Lab"), {}], "Cal": [Name("CalRGB"), {}], "Unknown": Name("Nope"),
        "Empty": [], "NumFirst": [5, 6], "DeviceRGB": icc(1), "Ref": Ref(999), "Str": b"DeviceRGB",
    }
    res = {"ColorSpace": spaces, "Font": {"F1": b.obj(HELVETICA)}}
    comps = [b"", b"0.5", b"0.1 0.2", b"0.1 0.2 0.3", b"0.1 0.2 0.3 0.4", b"0.1 0.2 0.3 0.4 0.5",
             b"/P1", b"0.5 /P1", b"0.1 0.2 0.3 /P1", b"0.1 0.2 0.3 0.4 /P1", b"(x) /P1", b"/P1 0.5", b"(nan)",
             b"true false true", b"0.5 0.5 0.5 0.5 0.5 /P1"]
    pages = []
    for names in [["I1", "I3", "I4", "I0", "INeg"], ["IBig", "IFloat", "IZeroF", "ITrue", "IFalse"],
                  ["DN", "Idx", "Sep", "Pat", "PatName"], ["Lab", "Cal", "Unknown", "DeviceRGB", "Str"]]:
        for cs in names:
            body = b" ".join(b"/%s cs %s scn /%s CS %s SCN 0 0 m 5 5 l b" % (cs.encode(), c, cs.encode(), c) for c in comps)
            pages.append(body + b" BT /F1 10 Tf (x) Tj ET")
    for chunk in range(0, len(pages), 20):
        bb = Builder()
        bb.objects, bb.next = list(b.objects), b.next
        for p in pages[chunk : chunk + 20]:
            bb.page(p, res)
        out.append((f"color-spaces-{chunk // 20}", bb.build(), None))
    # the color spaces that raise in init_resources, one document each
    for cs in ["IName", "IRef", "IStr", "INone", "INotStream", "IShort", "DNBad", "Empty", "NumFirst", "Ref"]:
        bb = Builder()
        bb.objects, bb.next = list(b.objects), b.next
        bb.page(b"/%s cs 0.5 scn 0 0 5 5 re f" % cs.encode(), {"ColorSpace": {cs: spaces[cs]}})
        out.append((f"color-space-{cs}", bb.build(), None))
    plain = [
        b"0.5 G 0.2 g 1 0 0 RG 0 1 0 rg 0 0 0 1 K 1 0 0 0 k 0 0 m 5 5 l B",
        b"(0.5) G (x) g 1 (0) 0 RG 0 1 /N rg 0 0 0 (1) K 1 0 0 [0] k 0 0 m 5 5 l B",
        b"(nan) G (inf) g %s 0 0 RG 0 0 m 5 5 l B" % OPERANDS["inf"],
        b"/DeviceCMYK cs 0.1 0.2 sc 0.1 0.2 0.3 0.4 sc /Pattern cs /P1 sc /DeviceGray CS 0.3 SC 0 0 m 5 5 l B",
        b"/NoSuch cs 0.5 scn /F1 CS 0.5 SCN 5 cs (DeviceRGB) CS 0 0 m 5 5 l B",
        b"/DeviceRGB cs 1 0 0 scn 0.5 g 0 0 m 5 5 l f /Pattern cs /P1 scn 0 0 m 5 5 l f",
        b"0 0 10 10 re /Sh1 sh /Sh1 (x) sh /GS1 gs 5 gs 0 0 m 5 5 l S",
    ]
    out.append(("color-operators", pages_doc(plain), None))
    return out


# ---------------------------------------------------------------------------------- paths


def cases_paths() -> list[Case]:
    out: list[Case] = []
    paint = [b"S", b"s", b"f", b"F", b"f*", b"B", b"B*", b"b", b"b*", b"n", b"W n", b"W* n"]
    pages = [b"0 0 m 10 0 l 10 10 l 0 10 l h 20 20 m 30 30 l 5 5 40 40 re 1 2 3 4 5 6 c 1 2 3 4 v 1 2 3 4 y " + p
             for p in paint]
    pages.append(b"S f B b n h S")
    pages.append(b"10 l 1 2 3 c 0 0 m (x) 1 l 1 (y) m 1 2 3 (4) 5 6 c /a 1 2 3 v 1 2 3 [4] y 1 2 (w) 4 re S")
    pages.append(b"(nan) (nan) m (inf) (1e400) l %s 0 l 0 0 (nan) 5 re S" % OPERANDS["inf"])
    pages.append(b"0 0 m h h h S 1 1 m S")
    out.append(("path-painting", pages_doc(pages), None))
    ctm = [b"0 0 0 0 0 0 cm 2 w", b"1e300 0 0 1e300 0 0 cm 1e300 0 0 1e300 0 0 cm 2 w",
           b"%s 0 0 1 0 0 cm 2 w" % OPERANDS["inf"], b"(nan) 0 0 1 0 0 cm 2 w", b"(1) (2) (3) (4) (5) (6) cm 2 w",
           b"1 2 3 cm 2 w", b"123456789012345678901234567890 0 0 1 0 0 cm 2 w", b"-1 0 0 -1 0 0 cm (nan) w",
           b"1 0 0 1 0 0 cm (inf) w %s w" % OPERANDS["hugeint"], b"[1 2] 0 d [] 0 d (x) /y d 5 d 2 J 1 j 4 M /RI ri 0.5 i",
           b"q q q 2 0 0 2 0 0 cm Q 3 w Q Q Q Q 4 w", b"Q Q q 0.5 g q 1 g Q", b"q 0 0 m 5 5 l Q S",
           b"q 1 0 0 1 5 5 cm BT /F1 10 Tf (a) Tj Q (b) Tj ET"]
    out.append(("path-cm", pages_doc([c + b" 0 0 m 10 10 l S BT /F1 10 Tf (x) Tj ET" for c in ctm]), None))
    return out


# ---------------------------------------------------------------------------------- inline images


def cases_inline() -> list[Case]:
    out: list[Case] = []
    img = b"BI /W 2 /H 2 /BPC 8 /CS /G ID \x00\x01\x02\x03 EI"
    pages = [
        img,
        b"BI /W 1 /H 1 ID x EI " + img,
        b"BI /W 1 ID x EI BI /H 1 ID x EI BI ID x EI",
        b"BI /W 1 /H ID x EI",
        b"BI /Width 1 /Height 1 ID x EI",
        b"BI /W 1 /H 1 /F /A85 ID 87cURD]i,\"Ebo80~>",
        b"BI /W 1 /H 1 /F [/ASCII85Decode] ID 87cURD]i,\"Ebo80~> EI",
        b"BI /W 1 /H 1 /F /AHx ID 41EI42 EI",
        b"BI /W 1 /H 1 ID aEIbEI\nEI",
        b"ID x EI",
        b"BI /W 1 /H 1 ID x",
        b"BI /W 1 /H 1 /F 5 ID x EI",
        b"BI /W 1 /H 1 /F [] ID x EI",
        b"BI /W 1 /H 1 /DP << /K -1 >> ID x\r\nEI",
        b"q 2 0 0 2 10 10 cm " + img + b" Q " + img,
        b"BI /W 1 /H 1 ID x EI EI",
    ]
    out.append(("inline-images", pages_doc([p + b"\n0 0 m 5 5 l S" for p in pages]), None))
    # an inline image across the 4096-byte blocks of the content stream
    for shift in (-6, -4, -3, -2, -1, 0, 1, 2):
        pad = b"%" + b"x" * (4096 - 25 + shift) + b"\n"
        content = pad + b"BI /W 4 /H 1 ID abcd EI 0 0 m 5 5 l S"
        out.append((f"inline-boundary-{shift}", one_page(content), None))
    # the image's data split between two content streams
    b = Builder()
    b.page([b.stream(b"BI /W 1 /H 1 ID ab"), b.stream(b"cd EI 0 0 m 5 5 l S")])
    b.page([b.stream(b"BI /W 1 /H 1 ID abc E"), b.stream(b"I 0 0 m 5 5 l S")])
    b.page([b.stream(b"BI /W 1 /H"), b.stream(b"1 ID x EI")])
    out.append(("inline-split-streams", b.build(), None))
    return out


# ---------------------------------------------------------------------------------- marked content


def cases_marked() -> list[Case]:
    props = [b"<< /MCID 0 >>", b"<< /MCID 1 /Lang (en) >>", b"/P1", b"5", b"(str)", b"[1 2]", b"null",
             b"<< /MCID (x) >>", b"<< /MCID 2.5 >>", b"<< >>", b"true"]
    pages = []
    for p in props:
        pages.append(b"/Span %s BDC BT /F1 10 Tf (a) Tj ET 0 0 m 5 5 l S /Tag %s DP EMC" % (p, p))
    pages += [
        b"/P BMC /Q BMC BT /F1 10 Tf (a) Tj ET EMC EMC EMC EMC BT /F1 10 Tf (b) Tj ET",
        b"(s) BMC 5 BDC [1] << /MCID 1 >> BDC EMC /T MP 5 MP (x) /T DP /T << /MCID 3 >> DP",
        b"/Span << /MCID 4 >> BDC BI /W 1 /H 1 ID x EI /Im1 Do EMC",
        b"/A BMC /B << /MCID 7 >> BDC /C BMC BT /F1 10 Tf (abc) Tj ET EMC EMC (d) Tj EMC",
    ]
    b = Builder()
    image = b.stream(b"\x00", Type=Name("XObject"), Subtype=Name("Image"), Width=1, Height=1,
                     BitsPerComponent=8, ColorSpace=Name("DeviceGray"))
    res = {"Font": {"F1": b.obj(HELVETICA)}, "XObject": {"Im1": image},
           "Properties": {"P1": {"MCID": 9}}}
    for p in pages:
        b.page(p, res)
    return [("mc-tags", b.build(), None)]


# ---------------------------------------------------------------------------------- XObjects


def form(b: Builder, content: bytes, **attrs: Any) -> Ref:
    d: dict = {"Type": Name("XObject"), "Subtype": Name("Form"), "BBox": [0, 0, 100, 100]}
    d.update(attrs)
    d = {k: v for k, v in d.items() if v is not None}
    return b.stream(content, **d)


def cases_xobjects() -> list[Case]:
    out: list[Case] = []
    b = Builder()
    font = b.obj(HELVETICA)
    inner = b"BT /F1 10 Tf 5 5 Td (in) Tj ET 0 0 m 10 10 l S"
    image = b.stream(b"\x00", Type=Name("XObject"), Subtype=Name("Image"), Width=1, Height=1,
                     BitsPerComponent=8, ColorSpace=Name("DeviceGray"))
    xobjs = {
        "Plain": form(b, inner),
        "NoBBox": form(b, inner, BBox=None),
        "BBoxRef": form(b, inner, BBox=b.obj([0, 0, 50, 50])),
        "BBoxShort": form(b, inner, BBox=[0, 0, 5]),
        "BBoxStr": form(b, inner, BBox=b"abc"),
        "BBoxName": form(b, inner, BBox=Name("x")),
        "Matrix": form(b, inner, Matrix=[2, 0, 0, 2, 10, 10]),
        "MatrixRef": form(b, inner, Matrix=b.obj([1, 0, 0, 1, 5, 5])),
        "MatrixShort": form(b, inner, Matrix=[1, 0, 0]),
        "MatrixLong": form(b, inner, Matrix=[1, 0, 0, 1, 0, 0, 7]),
        "MatrixStr": form(b, inner, Matrix=[Raw(b"(a)"), 0, 0, 1, 0, 0]),
        "MatrixArr": form(b, inner, Matrix=[[1], [2], [3], [4], 0, 0]),
        "MatrixDict": form(b, inner, Matrix=[{}, 0, 0, 1, 0, 0]),
        "MatrixName": form(b, inner, Matrix=Name("M")),
        "MatrixNull": form(b, inner, Matrix=None),
        "MatrixBool": form(b, inner, Matrix=[True, False, False, True, 0, 0]),
        "MatrixBig": form(b, inner, Matrix=[Raw(b"1" + b"0" * 330), 0, 0, 1, 0, 0]),
        "MatrixZero": form(b, inner, Matrix=[0, 0, 0, 0, 0, 0]),
        "Res": form(b, b"BT /G 10 Tf (own) Tj ET", Resources={"Font": {"G": font}}),
        "ResEmpty": form(b, inner, Resources={}),
        "ResRef": form(b, inner, Resources=b.obj({"Font": {"F1": font}})),
        "ResNum": form(b, inner, Resources=5),
        "ResList": form(b, inner, Resources=[1]),
        "NoSubtype": b.stream(inner, BBox=[0, 0, 1, 1]),
        "PS": b.stream(inner, Subtype=Name("PS")),
        "Image": image,
        "ImageNoW": b.stream(b"\x00", Subtype=Name("Image"), Height=1),
        "ImageNoH": b.stream(b"\x00", Subtype=Name("Image"), Width=1),
        "NotStream": b.obj({"Subtype": Name("Form"), "BBox": [0, 0, 1, 1]}),
        "Missing": Ref(999),
        "Num": 5,
    }
    res = {"Font": {"F1": font}, "XObject": xobjs}
    names = list(xobjs)
    for i in range(0, len(names), 3):
        content = b" ".join(b"q /%s Do Q" % n.encode() for n in names[i : i + 3])
        b.page(content + b" /Nope Do 5 Do (x) Do /Plain Do", res)
    out.append(("xobj-forms", b.build(), None))
    # resources of the page that are not a dict: a form without /Resources copies them
    for label, page_res in [("list", [Ref(999)]), ("num", 5), ("name", Name("R")), ("string", b"r"),
                            ("none", None), ("empty", {}), ("stream", "stream"), ("ref-missing", Ref(998))]:
        bb = Builder()
        f = form(bb, b"0 0 m 5 5 l S /Sub Do")
        if page_res == "stream":
            page_res = bb.stream(b"x")
        bb.page(b"/X Do 0 0 m 1 1 l S", page_res)
        bb.put(Ref(999), {"XObject": {"X": f}})
        if label in ("empty",):
            bb.page(b"/X Do", {"XObject": {"X": f}})
        out.append((f"xobj-page-resources-{label}", bb.build(), None))
    # cycles
    b = Builder()
    me = b.reserve()
    b.put(me, {"Type": Name("XObject"), "Subtype": Name("Form"), "BBox": [0, 0, 9, 9],
               "Resources": {"XObject": {"Me": me}}}, b"0 0 m 5 5 l S /Me Do")
    a, c = b.reserve(), b.reserve()
    b.put(a, {"Subtype": Name("Form"), "BBox": [0, 0, 9, 9], "Resources": {"XObject": {"C": c}}}, b"/C Do 1 1 m 2 2 l S")
    b.put(c, {"Subtype": Name("Form"), "BBox": [0, 0, 9, 9], "Resources": {"XObject": {"A": a}}}, b"/A Do 3 3 m 4 4 l S")
    b.page(b"/Me Do /A Do /C Do", {"XObject": {"Me": me, "A": a, "C": c}})
    content = b.reserve()
    b.put(content, {"Subtype": Name("Form"), "BBox": [0, 0, 9, 9]}, b"0 0 m 5 5 l S /Self Do")
    b.page(content, {"XObject": {"Self": content}})
    b.page([content, content], {"XObject": {"Self": content}})
    shared = form(b, b"0 0 m 1 1 l S")
    b.page(b"/S Do /S Do", {"XObject": {"S": shared}})
    out.append(("xobj-cycles", b.build(), None))
    # content arrays with broken members
    b = Builder()
    s1, s2 = b.stream(b"0 0 m 5 5 l S"), b.stream(b"BT /F1 10 Tf (a) Tj ET")
    res = {"Font": {"F1": b.obj(HELVETICA)}}
    for contents in ([s1, None, s2], [s1, Ref(999), s2], [], [s1, 5, s2], [s1, {"A": 1}, s2], [s1, s1, s2],
                     [b.obj([s1])], Ref(999), b.obj(5), [s1, b.stream(b"", Filter=Name("FlateDecode")), s2],
                     [s1, b.stream(b"garbage", Filter=Name("FlateDecode")), s2], [b.stream(b""), b.stream(b"")]):
        b.page(contents, res)
    out.append(("xobj-contents", b.build(), None))
    return out


# ---------------------------------------------------------------------------------- depth


def form_chain(n: int, deepest: bytes, deepest_res: dict | None = None, link: bytes = b"/X Do",
               cycle: bool = False, precache: int = 4) -> bytes:
    """A page drawing form 1, which draws form 2, ... down to form ``n``,
    whose content is ``deepest`` (with ``deepest_res``); each form draws
    the next with ``link``. ``cycle``: the deepest form also draws the
    first one (refused as circular).

    The page first draws form ``n - precache`` (and so the last forms) at
    its own depth: their objects are then in the document's cache and their
    data decoded, so that resolving and parsing them in the deep chain is
    shallow, and the deepest point of the chain is the content of the last
    form (otherwise it is the ``getobj`` of the last form's stream, under
    the ``do_Do`` of the form before it, whatever the last form does)."""
    b = Builder()
    font = b.obj(HELVETICA)
    refs = [b.reserve() for _ in range(n)]
    for i, ref in enumerate(refs):
        if i + 1 < n:
            content = b"0 0 m 1 1 l " + link
            res = {"XObject": {"X": refs[i + 1]}}
        else:
            content = deepest + (b" /Top Do" if cycle else b"")
            res = dict(deepest_res(b) if callable(deepest_res) else (deepest_res or {}))
            if cycle:
                res["XObject"] = {"Top": refs[0]}
        b.put(ref, {"Type": Name("XObject"), "Subtype": Name("Form"), "BBox": [0, 0, 10, 10], "Resources": res},
              content)
    xobjects = {"X": refs[0]}
    content = b"/X Do BT /F1 10 Tf (after) Tj ET"
    if 0 < precache < n:
        xobjects["Pre"] = refs[n - 1 - precache]
        content = b"/Pre Do " + content
    b.page(content, {"XObject": xobjects, "Font": {"F1": font}})
    return b.build()


TOUNICODE = (b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CMapName /T def "
             b"1 begincodespacerange <00> <FF> endcodespacerange 2 beginbfchar <61> <0041> <62> <0042> endbfchar "
             b"1 beginbfrange <63> <65> <0043> endbfrange endcmap CMapName currentdict /CMap defineresource pop end end")

TYPE1_PROGRAM = (b"%!PS-AdobeFont-1.0: Test 001\n/FontName /Test def\n/Encoding 256 array\n"
                 b"0 1 255 {1 index exch /.notdef put} for\ndup 97 /a put\ndup 98 /b put\nreadonly def\n"
                 b"currentfile eexec\n")


def tounicode_font(b: Builder) -> dict:
    return {"Font": {"F": b.obj({**HELVETICA, "ToUnicode": b.stream(TOUNICODE)})}}


def type1_program_font(b: Builder) -> dict:
    program = b.stream(TYPE1_PROGRAM, Length1=len(TYPE1_PROGRAM), Length2=0, Length3=0)
    desc = b.obj({"Type": Name("FontDescriptor"), "FontName": Name("Test"), "Flags": 4,
                  "FontBBox": [0, 0, 500, 700], "FontFile": program})
    return {"Font": {"F": b.obj({"Type": Name("Font"), "Subtype": Name("Type1"), "BaseFont": Name("Test"),
                                 "FirstChar": 97, "LastChar": 98, "Widths": [500, 600], "FontDescriptor": desc})}}


def predefined_cmap_font(b: Builder) -> dict:
    return {"Font": {"F": type0(b, Name("UniJIS-UCS2-H"))}}


def vertical_font(b: Builder) -> dict:
    return {"Font": {"F": type0(b, Name("Identity-V"), W2=[1, [-1000, 500, 880]])}}



def depth_kinds() -> dict[str, Any]:
    """The chains of ``cases_depth``, by the path of their deepest point."""
    helv = {"Font": {"F": HELVETICA}}
    image = b"BI /W 1 /H 1 ID x EI"
    return {
        # the getobj of the last form, under the do_Do of the one before
        "uncached": lambda n: form_chain(n, b"0 0 m 5 5 l", precache=0),
        "plain": lambda n: form_chain(n, b"0 0 m 5 5 l"),
        "warning": lambda n: form_chain(n, b"(x) w"),
        "operand-warnings": lambda n: form_chain(n, b"(a) (b) m (c) G /x 1 2 rg 1 (y) 3 4 K (z) Tc"),
        "font": lambda n: form_chain(n, b"BT /F 10 Tf ET", helv),
        "unknown-font": lambda n: form_chain(n, b"BT /Nope 10 Tf (a) Tj ET"),
        "font-tounicode": lambda n: form_chain(n, b"BT /F 10 Tf (abc) Tj ET", tounicode_font),
        "font-type1-program": lambda n: form_chain(n, b"BT /F 10 Tf (ab) Tj ET", type1_program_font),
        "font-predefined-cmap": lambda n: form_chain(n, b"BT /F 10 Tf <3042> Tj ET", predefined_cmap_font),
        "char": lambda n: form_chain(n, b"BT /F 10 Tf (a) Tj ET", helv),
        "char-quote": lambda n: form_chain(n, b"BT /F 10 Tf 1 2 (a) \" (b) ' ET", helv),
        "char-vertical": lambda n: form_chain(n, b"BT /F 10 Tf <00010002> Tj ET", vertical_font),
        "text-seq": lambda n: form_chain(n, b"BT /F 10 Tf [(a) /x 5] TJ ET", helv),
        # PDFKeyError at the deepest form (not precached: the page would stop there)
        "text-seq-stream": lambda n: form_chain(n, b"BT /F 10 Tf BI /W 1 /H 1 /F /A85 ID z~> TJ ET", helv, precache=0),
        "inline-image": lambda n: form_chain(n, image),
        # the first inline image of the process: re.sub compiles its pattern
        "inline-image-first": lambda n: form_chain(n, image, precache=0),
        "image-xobject": lambda n: form_chain(n, b"/Im Do", lambda b: {"XObject": {"Im": b.stream(
            b"\x00", Subtype=Name("Image"), Width=1, Height=1)}}),
        "subpaths": lambda n: form_chain(n, b"0 0 m 5 5 l 6 6 m 7 7 l S"),
        "state": lambda n: form_chain(n, b"q 2 0 0 2 0 0 cm 1 w Q"),
        "color": lambda n: form_chain(n, b"/Pattern cs /P scn 0.1 0.2 0.3 /P scn"),
        "color-sc": lambda n: form_chain(n, b"/DeviceRGB cs 0.1 0.2 0.3 sc (x) 0.2 0.3 sc /DeviceCMYK CS 1 2 3 SC"),
        "colorspace-resource": lambda n: form_chain(n, b"/C cs 0.5 scn", lambda b: {"ColorSpace": {
            "C": [Name("ICCBased"), b.stream(b"", N=1)], "D": [Name("DeviceN"), [Name("a")]]}}),
        "tag": lambda n: form_chain(n, b"/Span << /MCID 1 >> BDC EMC /T MP"),
        "cycle": lambda n: form_chain(n, b"0 0 m 5 5 l S", cycle=True),
        "link-matrix": lambda n: form_chain(n, b"0 0 m 5 5 l S", link=b"q 1 0 0 1 1 1 cm /X Do Q"),
        "link-text": lambda n: form_chain(n, b"0 0 m 5 5 l S", link=b"BT /X Do ET"),
    }


#: The stages whose first failing depth bounds a window.
DEPTH_STAGES = ("interp_text", "interp", "fonts")


def _fails(stage: str, data: bytes) -> bool:
    """Whether the stage's trace of ``data`` has a ``RecursionError``
    (computed in a forked child: a fresh process state, as ``verify``)."""
    from ..unit.document import in_child
    from ..stages import trace_lines

    with tempfile.NamedTemporaryFile(suffix=".pdf", delete=False) as f:
        f.write(data)
        path = f.name
    try:
        lines = in_child(lambda: trace_lines(stage, path, None, 20))
    finally:
        os.unlink(path)
    return any("RecursionError" in line for line in lines)


def first_failing(stage: str, make: Any, lo: int = 2, hi: int = 600) -> int:
    """The smallest ``n`` whose chain fails in ``stage`` (bisection)."""
    assert not _fails(stage, make(lo)) and _fails(stage, make(hi)), (stage, lo, hi)
    while hi - lo > 1:
        mid = (lo + hi) // 2
        if _fails(stage, make(mid)):
            hi = mid
        else:
            lo = mid
    return hi


def cases_depth() -> list[Case]:
    out: list[Case] = []
    for label, make in depth_kinds().items():
        levels: set[int] = set()
        bounds = []
        for stage in DEPTH_STAGES:
            first = first_failing(stage, make)
            bounds.append(f"{stage} {first}")
            levels.update(range(first - 2, first + 2))
        print(f"depth-{label}: first failing " + ", ".join(bounds), file=sys.stderr)
        for n in sorted(levels):
            out.append((f"depth-{label}-{n}", make(n), None))
    return out


def generate(depth: bool = True) -> None:
    """Writes the corpus (``depth=False``: without ``cases_depth``)."""
    cases = (cases_operators() + cases_text() + cases_fonts() + cases_colors() + cases_paths() + cases_inline()
             + cases_marked() + cases_xobjects())
    if depth:
        cases += cases_depth()
    write_corpus("interp", cases)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--no-depth", action="store_true",
                    help="skip cases_depth (minutes of bisection): a corpus for development, not the reference one")
    generate(depth=not ap.parse_args().no_depth)


if __name__ == "__main__":
    main()
