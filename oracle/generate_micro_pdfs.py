#!/usr/bin/env python3
from __future__ import annotations

from pathlib import Path
import re
import subprocess


ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "test" / "fixtures" / "micro-pdfs"


class PDF:
    def __init__(self) -> None:
        self.objects: list[bytes] = []

    def add(self, body: str | bytes) -> int:
        if isinstance(body, str):
            body = body.encode("latin1")
        self.objects.append(body)
        return len(self.objects)

    def stream(self, dictionary: str, data: str | bytes) -> int:
        if isinstance(data, str):
            data = data.encode("latin1")
        body = f"<< {dictionary} /Length {len(data)} >>\nstream\n".encode("latin1") + data + b"\nendstream"
        return self.add(body)

    def write(self, path: Path, root_obj: int) -> None:
        out = bytearray(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n")
        offsets = [0]
        for index, body in enumerate(self.objects, start=1):
            offsets.append(len(out))
            out.extend(f"{index} 0 obj\n".encode("latin1"))
            out.extend(body)
            out.extend(b"\nendobj\n")
        xref = len(out)
        out.extend(f"xref\n0 {len(self.objects) + 1}\n".encode("latin1"))
        out.extend(b"0000000000 65535 f \n")
        for offset in offsets[1:]:
            out.extend(f"{offset:010d} 00000 n \n".encode("latin1"))
        out.extend(f"trailer\n<< /Size {len(self.objects) + 1} /Root {root_obj} 0 R >>\nstartxref\n{xref}\n%%EOF\n".encode("latin1"))
        path.write_bytes(bytes(out))


def page_tree(pdf: PDF, page_obj: int, extra_catalog: str = "") -> int:
    pages = pdf.add(f"<< /Type /Pages /Kids [{page_obj} 0 R] /Count 1 >>")
    catalog = pdf.add(f"<< /Type /Catalog /Pages {pages} 0 R {extra_catalog} >>")
    return catalog


def to_unicode_cmap(mapping: dict[int, str]) -> str:
    rows = []
    for code, text in mapping.items():
        src = f"<{code:04X}>"
        dst = "<" + "".join(f"{ord(char):04X}" for char in text) + ">"
        rows.append(f"{src} {dst}")
    return "\n".join(
        [
            "/CIDInit /ProcSet findresource begin",
            "12 dict begin",
            "begincmap",
            "/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def",
            "/CMapName /MicroToUnicode def",
            "/CMapType 2 def",
            "1 begincodespacerange",
            "<0000> <FFFF>",
            "endcodespacerange",
            f"{len(rows)} beginbfchar",
            *rows,
            "endbfchar",
            "endcmap",
            "CMapName currentdict /CMap defineresource pop",
            "end",
            "end",
        ]
    )


def type0_font(pdf: PDF, mapping: dict[int, str], wmode: int = 0, name: str = "MicroType0") -> int:
    cmap = pdf.stream("", to_unicode_cmap(mapping))
    descendant = pdf.add(
        f"<< /Type /Font /Subtype /CIDFontType2 /BaseFont /{name} /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> "
        f"/FontDescriptor << /Type /FontDescriptor /FontName /{name} /Flags 32 /Ascent 718 /Descent -207 /CapHeight 718 /ItalicAngle 0 /StemV 80 >> "
        f"/DW 600 >>"
    )
    encoding = "/Identity-V" if wmode else "/Identity-H"
    return pdf.add(f"<< /Type /Font /Subtype /Type0 /BaseFont /{name} /Encoding {encoding} /DescendantFonts [{descendant} 0 R] /ToUnicode {cmap} 0 R >>")


def text_operators() -> PDF:
    pdf = PDF()
    content = "\n".join(
        [
            "q 1 0 0 1 5 7 cm Q",
            "BT",
            "/F1 12 Tf",
            "80 Tz",
            "3 Ts",
            "1 0 0 1 20 180 Tm",
            "(A) Tj",
            "10 0 Td",
            "[(B) 20 (C)] TJ",
            "ET",
        ]
    )
    content_obj = pdf.stream("", content)
    font = pdf.add("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
    page = pdf.add(f"<< /Type /Page /MediaBox [0 0 240 220] /Resources << /Font << /F1 {font} 0 R >> >> /Contents {content_obj} 0 R >>")
    root = page_tree(pdf, page)
    pdf.root = root  # type: ignore[attr-defined]
    return pdf


def type3_simple() -> PDF:
    pdf = PDF()
    charproc = pdf.stream("", "500 0 0 0 500 500 d1\n50 50 400 400 re f")
    font = pdf.add(
        f"<< /Type /Font /Subtype /Type3 /Name /T3 /FontBBox [0 0 500 500] /FontMatrix [0.001 0 0 0.001 0 0] "
        f"/CharProcs << /A {charproc} 0 R >> /Encoding << /Type /Encoding /Differences [65 /A] >> /FirstChar 65 /LastChar 65 /Widths [500] "
        f"/Resources << >> >>"
    )
    content = "BT /F1 24 Tf 30 120 Td (A) Tj ET"
    content_obj = pdf.stream("", content)
    page = pdf.add(f"<< /Type /Page /MediaBox [0 0 180 160] /Resources << /Font << /F1 {font} 0 R >> >> /Contents {content_obj} 0 R >>")
    root = page_tree(pdf, page)
    pdf.root = root  # type: ignore[attr-defined]
    return pdf


def image_xobject() -> PDF:
    pdf = PDF()
    image = pdf.stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8", b"\xff\x00\x00")
    content_obj = pdf.stream("", "q 10 0 0 10 50 60 cm /Im1 Do Q")
    page = pdf.add(f"<< /Type /Page /MediaBox [0 0 160 160] /Resources << /XObject << /Im1 {image} 0 R >> >> /Contents {content_obj} 0 R >>")
    root = page_tree(pdf, page)
    pdf.root = root  # type: ignore[attr-defined]
    return pdf


def form_xobject_matrix() -> PDF:
    pdf = PDF()
    font = pdf.add("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
    inner_content = "0 0 30 20 re f\nBT /F1 10 Tf 2 8 Td (F) Tj ET"
    inner = pdf.stream(f"/Type /XObject /Subtype /Form /BBox [0 0 40 30] /Matrix [1 0 0 1 5 5] /Resources << /Font << /F1 {font} 0 R >> >>", inner_content)
    middle = pdf.stream(f"/Type /XObject /Subtype /Form /BBox [0 0 70 60] /Matrix [1 0 0 1 10 15] /Resources << /XObject << /Inner {inner} 0 R >> >>", "q /Inner Do Q")
    outer = pdf.stream(f"/Type /XObject /Subtype /Form /BBox [0 0 100 90] /Matrix [1 0 0 1 25 30] /Resources << /XObject << /Middle {middle} 0 R >> >>", "q /Middle Do Q")
    content_obj = pdf.stream("", "q /Fm1 Do Q")
    page = pdf.add(f"<< /Type /Page /MediaBox [0 0 160 160] /Resources << /XObject << /Fm1 {outer} 0 R >> >> /Contents {content_obj} 0 R >>")
    root = page_tree(pdf, page)
    pdf.root = root  # type: ignore[attr-defined]
    return pdf


def vector_objects() -> PDF:
    pdf = PDF()
    content = "\n".join(
        [
            "2 w",
            "10 10 m 100 10 l S",
            "20 20 40 30 re f",
            "10 80 m 30 120 70 120 90 80 c S",
        ]
    )
    content_obj = pdf.stream("", content)
    page = pdf.add(f"<< /Type /Page /MediaBox [0 0 140 140] /Resources << >> /Contents {content_obj} 0 R >>")
    root = page_tree(pdf, page)
    pdf.root = root  # type: ignore[attr-defined]
    return pdf


def annotations() -> PDF:
    pdf = PDF()
    content_obj = pdf.stream("", "BT /F1 12 Tf 20 160 Td (Annot target) Tj ET")
    font = pdf.add("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
    link = pdf.add("<< /Type /Annot /Subtype /Link /Rect [20 150 100 170] /Border [0 0 0] /A << /S /URI /URI (https://example.com) >> >>")
    highlight = pdf.add("<< /Type /Annot /Subtype /Highlight /Rect [20 145 100 170] /QuadPoints [20 170 100 170 20 145 100 145] /C [1 1 0] >>")
    widget = pdf.add("<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /V (value) /Rect [20 110 120 130] >>")
    page = pdf.add(
        f"<< /Type /Page /MediaBox [0 0 220 200] /Resources << /Font << /F1 {font} 0 R >> >> /Contents {content_obj} 0 R "
        f"/Annots [{link} 0 R {highlight} 0 R {widget} 0 R] >>"
    )
    acroform = f"/AcroForm << /Fields [{widget} 0 R] >>"
    root = page_tree(pdf, page, acroform)
    pdf.root = root  # type: ignore[attr-defined]
    return pdf


def page_boxes_rotate() -> PDF:
    pdf = PDF()
    content_obj = pdf.stream("", "0 0 50 20 re S")
    page = pdf.add(
        f"<< /Type /Page /MediaBox [0 0 200 100] /CropBox [10 20 190 90] /BleedBox [0 0 200 100] "
        f"/TrimBox [20 30 180 80] /ArtBox [30 40 170 70] /Rotate 90 /Resources << >> /Contents {content_obj} 0 R >>"
    )
    root = page_tree(pdf, page)
    pdf.root = root  # type: ignore[attr-defined]
    return pdf


def encodings_cmap() -> PDF:
    pdf = PDF()
    font = type0_font(pdf, {1: "A", 2: "\ufb01", 3: "?"}, name="MicroCMap")
    content = "BT /F1 18 Tf 20 120 Td <000100020003> Tj ET"
    content_obj = pdf.stream("", content)
    page = pdf.add(f"<< /Type /Page /MediaBox [0 0 180 160] /Resources << /Font << /F1 {font} 0 R >> >> /Contents {content_obj} 0 R >>")
    root = page_tree(pdf, page)
    pdf.root = root  # type: ignore[attr-defined]
    return pdf


def vertical_rtl_text() -> PDF:
    pdf = PDF()
    vertical_font = type0_font(pdf, {1: "縦", 2: "書"}, wmode=1, name="MicroVertical")
    rtl_font = type0_font(pdf, {1: "ا", 2: "ب", 3: "ج"}, name="MicroRTL")
    content = "\n".join(
        [
            "BT /Fv 18 Tf 40 130 Td <00010002> Tj ET",
            "BT /Fr 18 Tf 130 80 Td <000300020001> Tj ET",
        ]
    )
    content_obj = pdf.stream("", content)
    page = pdf.add(f"<< /Type /Page /MediaBox [0 0 180 160] /Resources << /Font << /Fv {vertical_font} 0 R /Fr {rtl_font} 0 R >> >> /Contents {content_obj} 0 R >>")
    root = page_tree(pdf, page)
    pdf.root = root  # type: ignore[attr-defined]
    return pdf


def inherited_resources() -> PDF:
    pdf = PDF()
    font = pdf.add("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
    content_obj = pdf.stream("", "BT /F1 12 Tf 20 70 Td (Inherited) Tj ET")
    page = pdf.add(f"<< /Type /Page /MediaBox [0 0 160 100] /Contents {content_obj} 0 R >>")
    pages = pdf.add(f"<< /Type /Pages /Kids [{page} 0 R] /Count 1 /Resources << /Font << /F1 {font} 0 R >> >> >>")
    root = pdf.add(f"<< /Type /Catalog /Pages {pages} 0 R >>")
    pdf.root = root  # type: ignore[attr-defined]
    return pdf


def contents_indirect_array() -> PDF:
    pdf = PDF()
    font = pdf.add("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
    c1 = pdf.stream("", "BT /F1 12 Tf 20 80 Td (Indirect) Tj ET")
    c2 = pdf.stream("", "BT /F1 12 Tf 20 60 Td (Array) Tj ET")
    array = pdf.add(f"[{c1} 0 R {c2} 0 R]")
    page = pdf.add(f"<< /Type /Page /MediaBox [0 0 160 110] /Resources << /Font << /F1 {font} 0 R >> >> /Contents {array} 0 R >>")
    root = page_tree(pdf, page)
    pdf.root = root  # type: ignore[attr-defined]
    return pdf


def graphics_state_colors() -> PDF:
    pdf = PDF()
    pattern_stream = "0 0 4 4 re f"
    pattern = pdf.stream("/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 4 4] /XStep 4 /YStep 4 /Resources << >>", pattern_stream)
    content = "\n".join(
        [
            "q W n",
            "[3 2] 1 d 2 w 0.5 G 10 10 m 110 10 l S",
            "0.25 g 20 20 30 20 re f",
            "0.1 0.2 0.3 rg 60 20 30 20 re B",
            "0 1 0 0 k 20 60 30 20 re f*",
            "/Pattern cs /P1 scn 60 60 30 20 re f",
            "Q",
        ]
    )
    content_obj = pdf.stream("", content)
    page = pdf.add(f"<< /Type /Page /MediaBox [0 0 140 110] /Resources << /Pattern << /P1 {pattern} 0 R >> >> /Contents {content_obj} 0 R >>")
    root = page_tree(pdf, page)
    pdf.root = root  # type: ignore[attr-defined]
    return pdf


def annotations_extended() -> PDF:
    pdf = PDF()
    content_obj = pdf.stream("", "BT /F1 12 Tf 20 220 Td (Extended annotations) Tj ET")
    font = pdf.add("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
    embedded = pdf.stream("/Type /EmbeddedFile /Subtype /text#2fplain", "attachment")
    filespec = pdf.add(f"<< /Type /Filespec /F (note.txt) /EF << /F {embedded} 0 R >> >>")
    square = pdf.add("<< /Type /Annot /Subtype /Square /Rect [20 170 60 210] /C [1 0 0] >>")
    circle = pdf.add("<< /Type /Annot /Subtype /Circle /Rect [70 170 110 210] /C [0 1 0] >>")
    freetext = pdf.add("<< /Type /Annot /Subtype /FreeText /Rect [20 130 120 160] /Contents (Free text) >>")
    fileattach = pdf.add(f"<< /Type /Annot /Subtype /FileAttachment /Rect [130 130 150 150] /FS {filespec} 0 R /Contents (Attached) >>")
    popup = pdf.add(f"<< /Type /Annot /Subtype /Popup /Rect [130 170 180 210] /Parent {freetext} 0 R >>")
    checkbox = pdf.add("<< /Type /Annot /Subtype /Widget /FT /Btn /T (check) /V /Yes /AS /Yes /Rect [20 90 35 105] >>")
    radio = pdf.add("<< /Type /Annot /Subtype /Widget /FT /Btn /Ff 32768 /T (radio) /V /Choice1 /AS /Choice1 /Rect [45 90 60 105] >>")
    choice = pdf.add("<< /Type /Annot /Subtype /Widget /FT /Ch /T (choice) /V (B) /Opt [(A) (B)] /Rect [70 85 130 110] >>")
    annots = [square, circle, freetext, fileattach, popup, checkbox, radio, choice]
    page = pdf.add(
        f"<< /Type /Page /MediaBox [0 0 220 250] /Resources << /Font << /F1 {font} 0 R >> >> /Contents {content_obj} 0 R "
        f"/Annots [{' '.join(f'{a} 0 R' for a in annots)}] >>"
    )
    root = page_tree(pdf, page, f"/AcroForm << /Fields [{checkbox} 0 R {radio} 0 R {choice} 0 R] >>")
    pdf.root = root  # type: ignore[attr-defined]
    return pdf


def images_advanced() -> PDF:
    pdf = PDF()
    mask = pdf.stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ImageMask true /BitsPerComponent 1", b"\x80")
    image = pdf.stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace 4 0 R /BitsPerComponent 8 /SMask 3 0 R", b"\x00\xff\x00")
    colorspace = pdf.add("[ /DeviceRGB ]")
    pdf.objects[image - 1] = f"<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace {colorspace} 0 R /BitsPerComponent 8 /SMask {mask} 0 R /Length 3 >>\nstream\n".encode("latin1") + b"\x00\xff\x00\nendstream"
    inline = "BI /W 1 /H 1 /BPC 8 /CS /RGB ID \xff\x00\x00 EI"
    content_obj = pdf.stream("", f"q 10 0 0 10 20 50 cm /Im1 Do Q\nq 8 0 0 8 60 50 cm {inline} Q")
    page = pdf.add(f"<< /Type /Page /MediaBox [0 0 120 100] /Resources << /XObject << /Im1 {image} 0 R >> >> /Contents {content_obj} 0 R >>")
    root = page_tree(pdf, page)
    pdf.root = root  # type: ignore[attr-defined]
    return pdf


def broken_recovery() -> PDF:
    pdf = PDF()
    content = pdf.stream("", "BT /F1 12 Tf 20 70 Td (Recovered) Tj ET")
    font = pdf.add("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
    page = pdf.add(f"<< /Type /Page /MediaBox [0 0 160 100] /Resources << /Font << /F1 {font} 0 R >> >> /Contents {content} 0 R >>")
    root = page_tree(pdf, page)
    pdf.root = root  # type: ignore[attr-defined]
    return pdf


FIXTURES = {
    "text-operators.pdf": text_operators,
    "type3-simple.pdf": type3_simple,
    "image-xobject.pdf": image_xobject,
    "form-xobject-matrix.pdf": form_xobject_matrix,
    "vector-objects.pdf": vector_objects,
    "annotations.pdf": annotations,
    "page-boxes-rotate.pdf": page_boxes_rotate,
    "encodings-cmap.pdf": encodings_cmap,
    "vertical-rtl-text.pdf": vertical_rtl_text,
    "inherited-resources.pdf": inherited_resources,
    "contents-indirect-array.pdf": contents_indirect_array,
    "graphics-state-colors.pdf": graphics_state_colors,
    "annotations-extended.pdf": annotations_extended,
    "images-advanced.pdf": images_advanced,
    "broken-recovery.pdf": broken_recovery,
}


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    for name, factory in FIXTURES.items():
        pdf = factory()
        pdf.write(OUT / name, pdf.root)  # type: ignore[attr-defined]
    broken = OUT / "broken-recovery.pdf"
    broken.write_bytes(re.sub(rb"startxref\n\d+", b"startxref\n0", broken.read_bytes()))
    plain = OUT / "text-operators.pdf"
    encrypted = OUT / "encrypted-password.pdf"
    subprocess.run(
        ["mutool", "clean", "-E", "rc4-40", "-U", "user", "-O", "owner", "-P", "none", str(plain), str(encrypted)],
        check=True,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


if __name__ == "__main__":
    main()
