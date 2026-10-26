"""Adversarial PDFs for the ``objects`` stage (and ``streams``): the
document layer of pdfminer (``pdfparser.py``, ``pdfdocument.py``).

    PDFPLUMBER_ROOT=<main checkout> python -m oracle.adversarial.objects

writes ``<cache>/corpus/adv-objects/`` (fixed seed, same bytes on every
run). The cases target what the real corpus rarely shows: broken or
missing xref tables and their fallback scan, wrong offsets, xref streams
with odd ``/W``, ``/Index``, predictors and field values, object streams
with bad ``/N``, ``/First``, offsets and nested references, ``/Prev`` and
``/XRefStm`` chains and loops, incremental updates, missing trailer or
``/Root``, garbage before ``%PDF`` and after ``%%EOF``, ``\\r``-only and
``\\r\\n`` line ends, stream ``/Length`` values (indirect, negative, huge,
wrong, in an unclosed array), object headers that do not match, values at
4096-byte block boundaries, and every standard encryption (RC4 40/128,
AES-128, identity and RC4 crypt filters, AES-256 revisions 5 and 6) with
the user, owner, a wrong and the empty password.
"""

from __future__ import annotations

import random
from collections.abc import Callable

from ._common import write_corpus
from ._pdf import (
    Doc,
    Encryption,
    Name,
    Raw,
    Ref,
    classic,
    flate,
    objstm,
    png_up,
    ser,
    standard_encryption,
    startxref,
    trailer,
    xref_stream_data,
    xref_table,
)

SEED = 20261015
CONTENT = b"BT /F1 12 Tf 10 10 Td (Hi) Tj ET"

Case = tuple[str, bytes, "str | None"]


def base_doc(eol: bytes = b"\n") -> Doc:
    doc = Doc(eol=eol)
    doc.add(1, {"Type": Name("Catalog"), "Pages": Ref(2)})
    doc.add(2, {"Type": Name("Pages"), "Kids": [Ref(3)], "Count": 1})
    doc.add(3, {
        "Type": Name("Page"), "Parent": Ref(2), "MediaBox": [0, 0, 200, 200], "Contents": Ref(4),
        "Resources": {"Font": {"F1": Ref(5)}},
    })
    doc.add(4, {}, stream=CONTENT)
    doc.add(5, {"Type": Name("Font"), "Subtype": Name("Type1"), "BaseFont": Name("Helvetica")})
    doc.add(6, {"Title": b"T", "Producer": b"adv"})
    return doc


TRAILER = {"Root": Ref(1), "Info": Ref(6)}


def simple(eol: bytes = b"\n", header: bytes | None = None) -> bytes:
    doc = base_doc(eol)
    if header is not None:
        doc.header = header
    return classic(doc, TRAILER)


def replace_once(data: bytes, old: bytes, new: bytes) -> bytes:
    assert data.count(old) >= 1, old
    return data.replace(old, new, 1)


def startxref_value(data: bytes) -> int:
    k = data.rindex(b"startxref")
    return int(data[k:].split()[1])


def with_startxref(data: bytes, value: bytes) -> bytes:
    k = data.rindex(b"startxref")
    return data[:k] + startxref(value)


# ---------------------------------------------------------------------------------- cases


def cases_basic() -> list[Case]:
    out: list[Case] = [("basic", simple(), None)]
    out.append(("crlf", simple(b"\r\n"), None))
    out.append(("cr-only", simple(b"\r"), None))
    data = simple()
    out.append(("garbage-before-header", b"garbage line\n" * 3 + data, None))
    out.append(("garbage-after-eof", data + b"trailing garbage\n%%EOF junk\n" + b"x" * 5000, None))
    out.append(("no-eol-at-eof", data.rstrip(b"\n"), None))
    out.append(("startxref-missing", data[: data.rindex(b"startxref")], None))
    for label, value in [("nondigit", b"abc"), ("negative", b"-5"), ("huge", b"2147483648"), ("beyond-eof", b"999999"),
                         ("zero", b"0"), ("empty", b""), ("spaces", b" 12 "), ("long", b"9" * 5000)]:
        out.append((f"startxref-{label}", with_startxref(data, value), None))
    out.append(("startxref-into-object", with_startxref(data, str(data.index(b"2 0 obj")).encode()), None))
    out.append(("no-eof-marker", data.replace(b"%%EOF", b""), None))
    # the xref table
    x = data.index(b"xref\n")
    out.append(("xref-keyword-missing", data[:x] + data[x + 5:], None))
    # read_xref_from reads the rest of the "xref" line: a subsection header on
    # it is lost
    out.append(("xref-header-same-line", replace_once(data, b"xref\n0 7\n", b"xref 0 7\n"), None))
    # the first token at startxref raises something else than PSEOF (an
    # octal escape above 255): not turned into PDFNoValidXRef
    k = data.rindex(b"startxref")
    out.append(("startxref-to-bad-octal", data[:k] + b"(\\400)\n" + startxref(k), None))
    out.append(("xref-bad-subsection", replace_once(data, b"0 7\n", b"0 7 9\n"), None))
    out.append(("xref-subsection-letters", replace_once(data, b"0 7\n", b"0 x\n"), None))
    out.append(("xref-count-short", replace_once(data, b"0 7\n", b"0 4\n"), None))
    out.append(("xref-count-long", replace_once(data, b"0 7\n", b"0 9\n"), None))
    out.append(("xref-start-1", replace_once(data, b"0 7\n", b"1 7\n"), None))
    out.append(("xref-underscore-start", replace_once(data, b"0 7\n", b"0_0 7\n"), None))
    entries = data[x:].split(b"\n")
    first_n = next(e for e in entries if e.endswith(b" n ") or e.endswith(b" n"))
    out.append(("xref-entry-bad-offset", replace_once(data, first_n, b"00000000x9 00000 n "), None))
    out.append(("xref-entry-negative-offset", replace_once(data, first_n, b"-000000009 00000 n "), None))
    out.append(("xref-entry-huge-offset", replace_once(data, first_n, b"9999999999 00000 n "), None))
    out.append(("xref-entry-two-fields", replace_once(data, first_n, b"0000000009 n       "), None))
    out.append(("xref-entry-free-letter", replace_once(data, first_n, first_n.replace(b" n", b" f")), None))
    out.append(("xref-entry-other-letter", replace_once(data, first_n, first_n.replace(b" n", b" q")), None))
    out.append(("xref-entry-wrong-offset", replace_once(data, first_n, b"%010d 00000 n " % (int(first_n[:10]) + 3)), None))
    off3 = data.index(b"3 0 obj")
    e3 = b"%010d 00000 n " % off3
    off2 = data.index(b"2 0 obj")
    out.append(("xref-entry-points-to-other-object", replace_once(data, e3, b"%010d 00000 n " % off2), None))
    out.append(("xref-entry-into-dict", replace_once(data, e3, b"%010d 00000 n " % (off3 + 9)), None))
    out.append(("xref-entry-genno-letters", replace_once(data, e3, b"%010d 0000x n " % off3), None))
    t = data.index(b"trailer")
    out.append(("trailer-missing", data[:t] + data[data.index(b"startxref"):], None))
    out.append(("trailer-keyword-glued", replace_once(data, b"trailer\n", b"trailerX\n"), None))
    out.append(("trailer-not-dict", replace_once(data, data[t:data.index(b"startxref")], b"trailer\n[1 0 R]\n"), None))
    out.append(("trailer-ref", replace_once(data, data[t:data.index(b"startxref")], b"trailer\n7 0 R\n").replace(
        b"xref\n", b"7 0 obj\n<</Root 1 0 R /Size 8>>\nendobj\nxref\n", 1), None))
    out.append(("trailer-unclosed", replace_once(data, b"/Info 6 0 R", b"/Info 6 0 R <<").rstrip(), None))
    out.append(("no-root", replace_once(data, b"/Root 1 0 R", b"/Rot 1 0 R"), None))
    out.append(("root-not-dict", replace_once(data, b"/Root 1 0 R", b"/Root 5"), None))
    out.append(("root-missing-object", replace_once(data, b"/Root 1 0 R", b"/Root 77 0 R"), None))
    out.append(("info-direct", replace_once(data, b"/Info 6 0 R", b"/Info <</Title (direct)>>"), None))
    out.append(("info-not-dict", replace_once(data, b"/Info 6 0 R", b"/Info 3.5"), None))
    out.append(("empty-file", b"", None))
    out.append(("header-only", b"%PDF-1.4\n", None))
    out.append(("not-a-pdf", b"hello world\n" * 100, None))
    return out


def cases_fallback() -> list[Case]:
    """No usable xref: PDFXRefFallback scans the lines."""
    out: list[Case] = []
    doc = base_doc()
    body, _ = doc.write_objects(None, len(doc.header))
    plain = doc.header + body
    out.append(("fallback-no-xref", plain + trailer(TRAILER), None))
    out.append(("fallback-no-trailer", plain, None))
    out.append(("fallback-trailer-after-junk", plain + b"junk\n" + trailer(TRAILER) + b"more junk\n", None))
    out.append(("fallback-two-trailers", plain + trailer({"Root": Ref(9)}) + trailer(TRAILER), None))
    # duplicate object numbers: the last one wins
    dup = plain + b"5 0 obj\n<</Type /Font /Subtype /Type3>>\nendobj\n" + trailer(TRAILER)
    out.append(("fallback-duplicate-object", dup, None))
    # odd object headers for the regex ^(\d+)\s+(\d+)\s+obj\b
    heads = b"".join([
        b"7\t0\tobj\n(tab)\nendobj\n",
        b"8 0 objx\n(not a match)\nendobj\n",
        b"9 0 obj_\n(not a match)\nendobj\n",
        b"10 0 obj\xe9\n(latin-1 letter: not a match)\nendobj\n",
        b"11 0 obj\xa0\n(nbsp: a match)\nendobj\n",
        b"12\x0c0 obj (formfeed)\nendobj\n",
        b"13\x1c0\x85obj (unicode spaces)\nendobj\n",
        b" 14 0 obj (leading space)\nendobj\n",
        b"015 00 obj (leading zeros)\nendobj\n",
        b"16 0 obj<</A 1>>endobj\n",
        b"17 0 R obj\n(R)\nendobj\n",
    ])
    out.append(("fallback-object-headers", plain + heads + trailer(TRAILER), None))
    # object streams found by the scan, with odd ids
    for label, items, n in [
        ("ints", [(20, {"A": 1}), (21, [1, 2]), (22, Ref(3))], 3),
        ("float-ids", [(Raw(b"23.0"), {"F": 1}), (Raw(b"4.5"), 2)], 2),
        ("bool-and-null-ids", [(True, {"B": 1}), (None, 2), (False, 3)], 3),
        ("name-and-string-ids", [(Name("N"), 1), (b"s", 2), (Raw(b"kw"), 3)], 3),
        ("ref-id", [(Ref(3), 1)], 1),
        ("list-id", [([1], 1)], 1),
        ("dict-id", [({"A": 1}, 1)], 1),
        ("n-too-big", [(30, 1), (31, 2)], 9),
        ("n-zero", [(30, 1)], 0),
        ("n-negative", [(30, 1)], -1),
        ("n-float", [(30, 1), (31, 2)], 1.5),
        ("n-float-big", [(30, 1)], 5.5),
        ("n-name", [(30, 1)], Name("X")),
        ("n-ref", [(30, 1)], Ref(3)),
        ("n-bool", [(30, 1), (31, 2)], True),
        ("same-id-as-direct", [(5, {"Type": Name("Font"), "From": Name("ObjStm")})], 1),
    ]:
        data, first = objstm(items)
        d = {"Type": Name("ObjStm"), "First": first}
        if n is not None:
            d["N"] = n
        stm = b"40 0 obj\n" + ser(dict(d, Length=len(data))) + b"\nstream\n" + data + b"\nendstream\nendobj\n"
        out.append((f"fallback-objstm-{label}", plain + stm + trailer(TRAILER), None))
    data, first = objstm([(30, 1), (31, 2)])
    stm = b"40 0 obj\n" + ser({"Type": Name("ObjStm"), "First": first, "Length": len(data)}) + b"\nstream\n" + data + b"\nendstream\nendobj\n"
    out.append(("fallback-objstm-n-missing", plain + stm + trailer(TRAILER), None))
    z = flate(objstm([(30, {"Z": 1}), (31, 2)])[0])
    stm = (b"40 0 obj\n" + ser({"Type": Name("ObjStm"), "N": 2, "First": len(objstm([(30, {"Z": 1}), (31, 2)])[0]) - 13,
                                 "Filter": Name("FlateDecode"), "Length": len(z)}) + b"\nstream\n" + z + b"\nendstream\nendobj\n")
    out.append(("fallback-objstm-flate", plain + stm + trailer(TRAILER), None))
    # a later direct object with the number of an id the stream listed first
    data, first = objstm([(Raw(b"41.0"), 1)])
    stm = b"40 0 obj\n" + ser({"Type": Name("ObjStm"), "N": 1, "First": first, "Length": len(data)}) + b"\nstream\n" + data + b"\nendstream\nendobj\n"
    later = b"41 0 obj\n<</Later true>>\nendobj\n"
    out.append(("fallback-objstm-float-then-direct", plain + stm + later + trailer(TRAILER), None))
    # the scan reads streams with fallback=True: the data runs to endstream
    s = b"7 0 obj\n<</Length 3>>\nstream\nabcdefgh\nendstream\nendobj\n"
    out.append(("fallback-stream-length-ignored", plain + s + trailer(TRAILER), None))
    s = b"7 0 obj\n<</Length 3>>\nstream\nabc\n"
    out.append(("fallback-stream-unterminated", plain + trailer(TRAILER) + s, None))
    out.append(("fallback-cr-only", plain.replace(b"\n", b"\r") + trailer(TRAILER, b"\r"), None))
    out.append(("fallback-garbage-before", b"%junk\nxx 0 obj\n" + plain + trailer(TRAILER), None))
    out.append(("fallback-dict-odd", plain + b"7 0 obj\n<</A>>\nendobj\n" + trailer(TRAILER), None))
    return out


def cases_length() -> list[Case]:
    out: list[Case] = []
    for label, length in [("negative-1", b"-1"), ("negative-5", b"-5"), ("too-long", b"500"), ("too-short", b"3"),
                          ("zero", b"0"), ("float", b"12.5"), ("bool", b"true"), ("name", b"/L"), ("string", b"(12)"),
                          ("huge", b"9223372036854775900"), ("hugeneg", b"-9223372036854775900"),
                          ("missing", None), ("null", b"null")]:
        doc = base_doc()
        doc.objects = [o for o in doc.objects if o.num != 4]
        d = b"<<" + (b"/Length " + length if length is not None else b"") + b">>"
        doc.add_raw(4, b"4 0 obj\n" + d + b"\nstream\n" + CONTENT + b"\nendstream\nendobj\n")
        out.append((f"length-{label}", classic(doc, TRAILER), None))
    # indirect /Length: a later object, an object of an object stream, a
    # missing object, a non-int, itself, a stream
    for label, target in [("later", b"%d" % len(CONTENT)), ("float", b"33.0"), ("name", b"/X"),
                          ("negative", b"-3"), ("ref-chain", b"8 0 R"), ("stream", None)]:
        doc = base_doc()
        doc.objects = [o for o in doc.objects if o.num != 4]
        doc.add_raw(4, b"4 0 obj\n<</Length 7 0 R>>\nstream\n" + CONTENT + b"\nendstream\nendobj\n")
        if target is None:
            doc.add_raw(7, b"7 0 obj\n<</Length 2>>\nstream\nab\nendstream\nendobj\n")
        else:
            doc.add_raw(7, b"7 0 obj\n" + target + b"\nendobj\n")
        doc.add(8, len(CONTENT))
        out.append((f"length-indirect-{label}", classic(doc, TRAILER), None))
    doc = base_doc()
    doc.objects = [o for o in doc.objects if o.num != 4]
    doc.add_raw(4, b"4 0 obj\n<</Length 77 0 R>>\nstream\n" + CONTENT + b"\nendstream\nendobj\n")
    out.append(("length-indirect-missing", classic(doc, TRAILER), None))
    # /Length is the stream itself: getobj recurses until RecursionError
    # (a reference whose object is that reference would loop forever in
    # resolve1, in Python too: not generated)
    doc = base_doc()
    doc.objects = [o for o in doc.objects if o.num != 4]
    doc.add_raw(4, b"4 0 obj\n<</Length 4 0 R>>\nstream\n" + CONTENT + b"\nendstream\nendobj\n")
    out.append(("length-indirect-itself", classic(doc, TRAILER), None))
    # the stream's dictionary inside an unclosed array, its /Length
    # indirect: the nested getobj resets the parser of the outer object
    doc = base_doc()
    doc.objects = [o for o in doc.objects if o.num != 4]
    doc.add_raw(4, b"4 0 obj\n[ 1 2 <</Length 7 0 R>>\nstream\n" + CONTENT + b"\nendstream\nendobj\n")
    doc.add(7, len(CONTENT))
    out.append(("length-indirect-in-array", classic(doc, TRAILER), None))
    doc = base_doc()
    doc.add_raw(7, b"7 0 obj\nstream\nabc\nendstream\nendobj\n")
    out.append(("stream-without-dict", classic(doc, TRAILER), None))
    doc = base_doc()
    doc.add_raw(7, b"7 0 obj\n5 0 R\nstream\nabc\nendstream\nendobj\n")
    out.append(("stream-dict-is-ref", classic(doc, TRAILER), None))
    for label, sep in [("crlf", b"\r\n"), ("cr", b"\r"), ("space", b" "), ("none", b"")]:
        doc = base_doc()
        doc.objects = [o for o in doc.objects if o.num != 4]
        doc.add_raw(4, b"4 0 obj\n<</Length %d>>\nstream%s" % (len(CONTENT), sep) + CONTENT + b"\nendstream\nendobj\n")
        out.append((f"stream-keyword-{label}", classic(doc, TRAILER), None))
    doc = base_doc()
    doc.objects = [o for o in doc.objects if o.num != 4]
    doc.add_raw(4, b"4 0 obj\n<</Length %d>>\nstream\n" % len(CONTENT) + CONTENT + b"\nendobj\n")
    out.append(("stream-no-endstream", classic(doc, TRAILER), None))
    # data and keywords across the 4096-byte blocks
    for shift in (0, 1, 2, 5, 9):
        doc = base_doc()
        doc.objects = [o for o in doc.objects if o.num != 4]
        pad = b"%" + b"p" * (4096 - 120 + shift) + b"\n"
        doc.add_raw(4, pad + b"4 0 obj\n<</Length %d>>\nstream\n" % len(CONTENT) + CONTENT + b"\nendstream\nendobj\n")
        data = classic(doc, TRAILER)
        out.append((f"block-boundary-{shift}", data, None))
    big = bytes(range(256)) * 40 + b"endstrea" + b"x" * 30
    doc = base_doc()
    doc.add(7, {}, stream=big)
    out.append(("stream-binary-big", classic(doc, TRAILER), None))
    return out


def cases_headers() -> list[Case]:
    """Object headers that do not match their xref entry (the "objid
    mismatch" hack of _getobj_parse)."""
    out: list[Case] = []
    for label, head in [("other-number", b"9 0 obj"), ("float-number", b"5.0 0 obj"), ("bool-number", b"true 0 obj"),
                        ("genno-other", b"5 7 obj"), ("no-obj", b"5 0 R"), ("tokens-before-obj", b"x 5 0 obj"),
                        ("one-token-before", b"5 obj"), ("name", b"/A 0 obj"), ("obj-glued", b"5 0obj"),
                        ("string-number", b"(5) 0 obj"),
                        # the hack recovers (x[-2] is the id), or takes the genno
                        ("hack-recovers", b"x y z 5 obj"), ("hack-takes-genno", b"a b c 5 0 obj")]:
        doc = base_doc()
        doc.objects = [o for o in doc.objects if o.num != 5]
        doc.add_raw(5, head + b"\n<</Type /Font /Subtype /Type1 /BaseFont /Times-Roman>>\nendobj\n")
        out.append((f"header-{label}", classic(doc, TRAILER), None))
    doc = base_doc()
    doc.add_raw(7, b"7 0 obj\n<</A 1 /B>>\nendobj\n")
    doc.add_raw(8, b"8 0 obj\n[1 2\nendobj\n")
    doc.add_raw(9, b"9 0 obj\n<</A [1 2>>\nendobj\n")
    doc.add_raw(10, b"10 0 obj\n1 2 3 4 5\nendobj\n")
    doc.add_raw(11, b"11 0 obj\nendobj\n")
    doc.add_raw(12, b"12 0 obj\n(unterminated\nendobj\n")
    doc.add_raw(13, b"13 0 obj\n<</A 1.5.5 /B --3 /C 1e5 /D .5 /E 5. /F +3>>\nendobj\n")
    doc.add_raw(14, b"14 0 obj\n<</N null /M 3 0 R /K 3.7 0 R /J (x) 0 R /I 99999999999999999999 0 R>>\nendobj\n")
    doc.add_raw(15, b"15 0 obj\n<</Type/Page/X[1/A(2)<3f>]>>%comment\nendobj\n")
    doc.add_raw(16, b"16 0 obj\n<</A xref /B startxref>>\nendobj\n")
    doc.add_raw(17, b"17 0 obj\n{ 1 2 } endobj\n")
    doc.add_raw(18, b"18 0 obj\n1 0 R 2 0 R R\nendobj\n")
    out.append(("odd-objects", classic(doc, TRAILER), None))
    return out


def incremental(base: bytes, objects: list[tuple[int, bytes]], prev: int | bytes | None, extra: dict | None = None,
                xref_stm: int | None = None) -> bytes:
    """Appends an update section to ``base``: objects, an xref table
    covering them and a trailer with /Prev."""
    out = base
    offsets = {}
    for num, body in objects:
        offsets[num] = (len(out), 0)
        out += body
    xpos = len(out)
    out += b"xref\n"
    for num in sorted(offsets):
        out += b"%d 1\n%010d 00000 n \n" % (num, offsets[num][0])
    d = dict(TRAILER, Size=20)
    if prev is not None:
        d["Prev"] = Raw(prev) if isinstance(prev, bytes) else prev
    if xref_stm is not None:
        d["XRefStm"] = xref_stm
    d.update(extra or {})
    return out + trailer(d) + startxref(xpos)


def cases_prev() -> list[Case]:
    out: list[Case] = []
    base = simple()
    prev = startxref_value(base)
    font = b"5 0 obj\n<</Type /Font /Subtype /Type1 /BaseFont /Courier>>\nendobj\n"
    info = b"6 0 obj\n<</Title (updated)>>\nendobj\n"
    out.append(("update-one", incremental(base, [(5, font)], prev), None))
    two = incremental(incremental(base, [(5, font)], prev), [(6, info)], startxref_value(incremental(base, [(5, font)], prev)))
    out.append(("update-two", two, None))
    out.append(("update-info-only", incremental(base, [(6, info)], prev, {"Info": Ref(6), "Root": Ref(1)}), None))
    out.append(("update-no-root", incremental(base, [(6, info)], prev, {"Root": Raw(b"null")}), None))
    hole = b"PREVPREVPR"  # a placeholder, replaced by a 10-digit offset
    for label, value in [("self", hole), ("negative", b"-10"), ("huge", b"9223372036854775808"),
                         ("very-huge", b"99999999999999999999999"), ("beyond-eof", b"99999"), ("float", b"12.5"),
                         ("bool", b"true"), ("name", b"/P"), ("ref", b"5 0 R"), ("into-object", None)]:
        if label == "into-object":
            data = incremental(base, [(5, font)], base.index(b"2 0 obj") + 2)
        else:
            data = incremental(base, [(5, font)], value)
        if label == "self":
            data = data.replace(hole, b"%010d" % startxref_value(data))
        out.append((f"prev-{label}", data, None))
    # a loop of two sections: the first one's /Prev is the second
    a = incremental(base, [(5, font)], hole)
    b = incremental(a, [(6, info)], startxref_value(a))
    out.append(("prev-loop", b.replace(hole, b"%010d" % startxref_value(b)), None))
    # an xref stream reached by /XRefStm (hybrid file)
    out.append(("hybrid-xrefstm", xref_stream_file(xref_stm_hybrid=True), None))
    for label, value in [("negative", b"-1"), ("garbage", b"/X"), ("loop", hole)]:
        data = incremental(base, [(5, font)], prev, {"XRefStm": Raw(value)})
        data = data.replace(hole, b"%010d" % startxref_value(data))
        out.append((f"xrefstm-{label}", data, None))
    return out


def xref_stream_file(*, w=(1, 2, 1), index=None, predictor: str | None = None, size: int | None = None,
                     extra: dict | None = None, mutate: Callable[[list], list] | None = None, objstm_nums=(),
                     objstm_dict: dict | None = None, objstm_items: Callable | None = None,
                     xref_stm_hybrid: bool = False, enc: Encryption | None = None, raw_w=None, raw_index=None,
                     data_cut: int = 0) -> bytes:
    """A file whose objects are indexed by an xref stream (object 19),
    some of them (``objstm_nums``) inside an object stream (object 18)."""
    doc = base_doc()
    in_stm = [o for o in doc.objects if o.num in objstm_nums]
    doc.objects = [o for o in doc.objects if o.num not in objstm_nums]
    stm_index: dict[int, int] = {}
    if in_stm:
        items = [(o.num, o.value) for o in in_stm]
        if objstm_items is not None:
            items = objstm_items(items)
        data, first = objstm(items)
        d = {"Type": Name("ObjStm"), "N": len(items), "First": first}
        d.update(objstm_dict or {})
        doc.add(18, d, stream=data)
        for i, (num, _) in enumerate(items):
            if isinstance(num, int):
                stm_index[num] = i
    head = doc.header
    body, offsets = doc.write_objects(enc, len(head))
    xpos = len(head) + len(body)
    n = max([19, *offsets, *stm_index]) + 1
    rows = []
    for num in range(n):
        if num in offsets:
            rows.append((1, offsets[num][0], 0))
        elif num in stm_index:
            rows.append((2, 18, stm_index[num]))
        elif num == 19:
            rows.append((1, xpos, 0))
        else:
            rows.append((0, 0, 0))
    if mutate is not None:
        rows = mutate(rows)
    w2 = list(w)
    raw_rows = xref_stream_data(rows, tuple(max(x, 0) for x in w2))
    d: dict = {"Type": Name("XRef"), "Size": size if size is not None else n, "W": Raw(raw_w) if raw_w else list(w)}
    if index is not None:
        d["Index"] = index
    if raw_index is not None:
        d["Index"] = Raw(raw_index)
    if predictor == "png":
        width = sum(max(x, 0) for x in w2)
        rows_b = [raw_rows[i:i + width] for i in range(0, len(raw_rows), width)]
        raw_rows = flate(png_up(rows_b))
        d["Filter"] = Name("FlateDecode")
        d["DecodeParms"] = {"Predictor": 12, "Columns": width}
    elif predictor == "flate":
        raw_rows = flate(raw_rows)
        d["Filter"] = Name("FlateDecode")
    if data_cut:
        raw_rows = raw_rows[:-data_cut]
    if xref_stm_hybrid:
        d.pop("Type")
        d["Type"] = Name("XRef")
    d.update(TRAILER)
    d.update(extra or {})
    d["Length"] = len(raw_rows)
    xs = b"19 0 obj\n" + ser(d) + b"\nstream\n" + raw_rows + b"\nendstream\nendobj\n"
    if xref_stm_hybrid:
        # a classic table listing the objects, whose trailer points at the
        # stream through /XRefStm
        table_pos = xpos + len(xs)
        return (head + body + xs + xref_table(offsets, n) +
                trailer(dict(TRAILER, Size=n, XRefStm=xpos)) + startxref(table_pos))
    return head + body + xs + startxref(xpos)


def cases_xref_stream() -> list[Case]:
    out: list[Case] = [("xrefstream-basic", xref_stream_file(), None)]
    out.append(("xrefstream-png", xref_stream_file(predictor="png"), None))
    out.append(("xrefstream-flate", xref_stream_file(predictor="flate"), None))
    out.append(("xrefstream-w-1-4-2", xref_stream_file(w=(1, 4, 2)), None))
    out.append(("xrefstream-w-0-2-1", xref_stream_file(w=(0, 2, 1), mutate=lambda r: [(1, b, c) if a == 1 else (a, b, c) for a, b, c in r]), None))
    out.append(("xrefstream-w-1-2-0", xref_stream_file(w=(1, 2, 0)), None))
    out.append(("xrefstream-w-1-8-1", xref_stream_file(w=(1, 8, 1)), None))
    for label, raw_w in [("float", b"[1 2.0 1]"), ("negative", b"[1 -2 1]"), ("bool", b"[true 2 1]"), ("two", b"[1 2]"),
                         ("four", b"[1 2 1 0]"), ("null", b"[1 null 1]"), ("string", b"(abc)"), ("dict", b"<</A 1 /B 2 /C 3>>"),
                         ("name", b"/W"), ("ref", b"5 0 R"), ("nested", b"[[1] [2] [1]]"), ("huge", b"[1 2 99999999999999999999]"),
                         ("strings", b"[(a) (b) (c)]")]:
        out.append((f"xrefstream-w-{label}", xref_stream_file(raw_w=raw_w), None))
    for label, raw_index in [("two-ranges", b"[0 3 3 17]"), ("odd", b"[0 20 5]"), ("float-start", b"[0.5 20]"),
                             ("float-count", b"[0 20.0]"), ("negative-count", b"[0 -3 0 20]"), ("offset-start", b"[5 20]"),
                             ("bytes", b"<0014>"), ("dict", b"<</A 1 /B 2>>"), ("name", b"/I"), ("bool", b"[false true 0 20]"),
                             ("empty", b"[]"), ("ref", b"[0 5 0 R]"), ("huge-start", b"[9223372036854775807 3 0 20]"),
                             ("string-start", b"[(a) 2 0 20]"), ("list-start", b"[[0] 20]")]:
        out.append((f"xrefstream-index-{label}", xref_stream_file(raw_index=raw_index), None))
    out.append(("xrefstream-size-missing", xref_stream_file(extra={"Size": Raw(b"null")}), None))
    out.append(("xrefstream-type-wrong", xref_stream_file(extra={"Type": Name("XRefs")}), None))
    out.append(("xrefstream-data-short", xref_stream_file(data_cut=9), None))
    out.append(("xrefstream-f1-unknown", xref_stream_file(mutate=lambda r: [(3, b, c) if i == 5 else (a, b, c) for i, (a, b, c) in enumerate(r)]), None))
    out.append(("xrefstream-f1-free", xref_stream_file(mutate=lambda r: [(0, b, c) if i == 5 else (a, b, c) for i, (a, b, c) in enumerate(r)]), None))
    out.append(("xrefstream-wrong-offset", xref_stream_file(mutate=lambda r: [(a, b + 2 if a == 1 else b, c) for a, b, c in r]), None))
    out.append(("xrefstream-not-a-stream", replace_once(xref_stream_file(), b"\nstream\n", b"\n"), None))
    # object streams
    out.append(("objstm-basic", xref_stream_file(objstm_nums=(5, 6)), None))
    out.append(("objstm-page-tree", xref_stream_file(objstm_nums=(1, 2, 3)), None))
    for label, d in [("n-big", {"N": 9}), ("n-small", {"N": 1}), ("n-float", {"N": 1.5}), ("n-negative", {"N": -1}),
                     ("n-missing", {"N": Raw(b"null")}), ("n-name", {"N": Name("N")}), ("n-list", {"N": [1]}),
                     ("n-string", {"N": b"2"}), ("n-bool", {"N": True}), ("n-ref", {"N": Ref(5)}),
                     ("first-wrong", {"First": 0}), ("type-wrong", {"Type": Name("XObject")})]:
        out.append((f"objstm-{label}", xref_stream_file(objstm_nums=(5, 6), objstm_dict=d), None))
    out.append(("objstm-index-big", xref_stream_file(objstm_nums=(5, 6), mutate=lambda r: [(2, 18, 7) if a == 2 else (a, b, c) for a, b, c in r]), None))
    out.append(("objstm-index-negative-row", xref_stream_file(objstm_nums=(5, 6), w=(1, 2, 2),
                                                             mutate=lambda r: [(2, 18, 65535) if a == 2 else (a, b, c) for a, b, c in r]), None))
    out.append(("objstm-strmid-missing", xref_stream_file(objstm_nums=(5, 6), mutate=lambda r: [(2, 77, c) if a == 2 else (a, b, c) for a, b, c in r]), None))
    out.append(("objstm-strmid-not-stream", xref_stream_file(objstm_nums=(5, 6), mutate=lambda r: [(2, 3, c) if a == 2 else (a, b, c) for a, b, c in r]), None))
    out.append(("objstm-strmid-self", xref_stream_file(objstm_nums=(5, 6), mutate=lambda r: [(2, i, c) if a == 2 else (a, b, c) for i, (a, b, c) in enumerate(r)]), None))
    out.append(("objstm-nested-refs", xref_stream_file(objstm_nums=(5, 6), objstm_items=lambda items: [(5, {"A": Ref(6), "B": [Ref(3), Ref(18)]}), (6, Ref(5))]), None))
    out.append(("objstm-filter-ref", xref_stream_file(objstm_nums=(5, 6), objstm_dict={"Filter": Ref(7)}), None))
    out.append(("objstm-odd-ids", xref_stream_file(objstm_nums=(5, 6), objstm_items=lambda items: [(Raw(b"5.0"), items[0][1]), (Name("six"), items[1][1])]), None))
    out.append(("objstm-stream-keyword", xref_stream_file(objstm_nums=(5, 6), objstm_items=lambda items: [(5, Raw(b"<<>> stream")), (6, Raw(b"1 R"))]), None))
    return out


def cases_encryption() -> list[Case]:
    rng = random.Random(SEED)
    out: list[Case] = []
    docid = b"\x01\x23\x45\x67\x89\xab\xcd\xef" * 2
    configs = [
        ("rc4-40", dict(r=2)), ("rc4-128", dict(r=3, length=128)), ("rc4-56", dict(r=3, length=56)),
        ("aes128", dict(r=4, cfm="AESV2")), ("v4-rc4", dict(r=4, cfm="V2")), ("v4-identity", dict(r=4, cfm=None)),
        ("aes128-no-metadata", dict(r=4, cfm="AESV2", encrypt_metadata=False)),
        ("aes256-r5", dict(r=5)), ("aes256-r6", dict(r=6)), ("aes256-r6-perms", dict(r=6, p=-3904)),
    ]
    for label, cfg in configs:
        for user, owner, passwords in [("user", "owner", ["user", "owner", "wrong", ""]), ("", "owner", ["", "owner"])]:
            enc = standard_encryption(rng, user=user, owner=owner, docid=docid, **cfg)
            for k, password in enumerate(passwords):
                doc = base_doc()
                doc.header += b"%variant " + str(k).encode() + b" " + label.encode() + b"\n"
                if label.endswith("no-metadata"):
                    doc.add(7, {"Type": Name("Metadata"), "Subtype": Name("XML")}, stream=b"<x:xmpmeta/>", encrypt=False)
                doc.add(8, {"Type": Name("Encrypt"), **enc.param}, encrypt=False)
                data = classic(doc, dict(TRAILER, Encrypt=Ref(8), ID=[docid, docid]), enc)
                name = f"encrypt-{label}-{'nouser' if not user else 'user'}-pw{k}"
                out.append((name, data, password))
    # no /ID, /Encrypt direct, unknown filter or version, encrypted
    # objects in an object stream, a broken xref
    enc = standard_encryption(rng, r=3, user="", owner="o", docid=b"")
    doc = base_doc()
    doc.add(8, {**enc.param}, encrypt=False)
    out.append(("encrypt-no-id", classic(doc, dict(TRAILER, Encrypt=Ref(8)), enc), None))
    doc = base_doc()
    out.append(("encrypt-direct", classic(doc, dict(TRAILER, Encrypt=dict(enc.param)), enc), None))
    doc = base_doc()
    out.append(("encrypt-unknown-filter", classic(doc, dict(TRAILER, Encrypt={**enc.param, "Filter": Name("Foo")}, ID=[b"", b""])), None))
    out.append(("encrypt-unknown-version", classic(doc, dict(TRAILER, Encrypt={**enc.param, "V": 3}, ID=[b"", b""])), None))
    out.append(("encrypt-unknown-revision", classic(doc, dict(TRAILER, Encrypt={**enc.param, "R": 7}, ID=[b"", b""])), None))
    out.append(("encrypt-not-dict", classic(doc, dict(TRAILER, Encrypt=5, ID=[b"", b""])), None))
    out.append(("encrypt-missing-object", classic(doc, dict(TRAILER, Encrypt=Ref(99), ID=[b"", b""])), None))
    enc = standard_encryption(rng, r=3, user="", owner="o", docid=docid)
    doc = base_doc()
    doc.add(8, {**enc.param}, encrypt=False)
    data = classic(doc, dict(TRAILER, Encrypt=Ref(8), ID=[docid, docid]), enc)
    out.append(("encrypt-broken-xref", data[: data.index(b"xref\n")] + trailer(dict(TRAILER, Encrypt=Ref(8), ID=[docid, docid])), None))
    enc = standard_encryption(rng, r=4, cfm="AESV2", user="", owner="o", docid=docid)
    out.append(("encrypt-objstm", xref_stream_file(objstm_nums=(5, 6), enc=enc,
                                                   extra={"Encrypt": dict(enc.param), "ID": [docid, docid]}), None))
    return out


def prev_chain(n: int, streams: str) -> bytes:
    """``n`` xref sections linked by ``/Prev`` (read_xref_from recurses):
    tables (``streams`` "none"), xref streams ("all"), or tables and an xref
    stream as the last one read ("bottom")."""
    out = bytearray(b"%PDF-1.5\n")
    off1 = len(out)
    out += b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n"
    off2 = len(out)
    out += b"2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n"
    prev = None
    num = 3
    for i in range(n):
        x = len(out)
        prevpart = (b" /Prev %d" % prev) if prev is not None else b""
        if streams == "all" or (streams == "bottom" and i == 0):
            ents = b"".join(b"\x01" + o.to_bytes(4, "big") + b"\x00" for o in (off1, off2))
            out += (b"%d 0 obj\n<< /Type /XRef /Size 3 /Index [1 2] /W [1 4 1] /Root 1 0 R%s /Length %d >>\nstream\n"
                    % (num, prevpart, len(ents)) + ents + b"\nendstream\nendobj\n")
            num += 1
        else:
            out += b"xref\n0 3\n0000000000 65535 f \n%010d 00000 n \n%010d 00000 n \n" % (off1, off2)
            out += b"trailer\n<< /Size 3 /Root 1 0 R" + prevpart + b" >>\n"
        prev = x
    return bytes(out) + b"startxref\n%d\n%%%%EOF\n" % prev


def length_chain(n: int) -> bytes:
    """Streams whose /Length is the next stream, n deep (do_keyword ->
    int_value -> getobj -> do_keyword...), then an int."""
    doc = base_doc()
    for i in range(n):
        doc.add_raw(10 + i, b"%d 0 obj\n<< /Length %d 0 R >>\nstream\nabc\nendstream\nendobj\n" % (10 + i, 11 + i))
    doc.add(10 + n, 3)
    return classic(doc, TRAILER)


def tail_object(body: bytes) -> bytes:
    """``simple()`` plus an object 7 written after ``%%EOF`` (the end of
    the file ends its parse), listed in the xref table."""
    data = simple()
    doc = base_doc()
    head = doc.header
    body0, offsets = doc.write_objects(None, len(head))
    x = len(head) + len(body0)
    end_len = len(trailer(dict(TRAILER, Size=8))) + len(startxref(x))
    pos = x + len(xref_table({**offsets, 7: (0, 0)}, 8)) + end_len
    data = head + body0 + xref_table({**offsets, 7: (pos, 0)}, 8) + trailer(dict(TRAILER, Size=8)) + startxref(x)
    assert len(data) == pos
    return data + b"7 0 obj\n" + body


def fallback_float_id(r: int, cfm: str | None, later: bytes, id_token: bytes = b"41.0", num: int = 41) -> bytes:
    """An encrypted document repaired by PDFXRefFallback whose object
    stream lists the id 41.0 before the object "41 0 obj": getobj(41.0)
    parses that object and deciphers it with the float id (struct.pack
    raises for RC4 and AES-128 once a string is deciphered; AES-256 and
    Identity ignore the id). With the id `true` before "1 0 obj" (the
    catalog then is 11), the key of the offsets is True, an int for
    struct.pack."""
    assert len(id_token) == 4
    docid = b"0123456789abcdef"
    enc = standard_encryption(random.Random(r * 10 + (cfm is not None)), r=r, cfm=cfm, user="", owner="o", docid=docid)
    out = bytearray(b"%PDF-1.5\n")
    root = 1 if num != 1 else 11
    out += b"%d 0 obj\n<< /Type /Catalog /Pages %d 0 R >>\nendobj\n" % (root, root + 1)
    out += b"%d 0 obj\n<< /Type /Pages /Kids [%d 0 R] /Count 1 >>\nendobj\n" % (root + 1, root + 2)
    out += b"%d 0 obj\n<< /Type /Page /Parent %d 0 R /MediaBox [0 0 50 50] >>\nendobj\n" % (root + 2, root + 1)
    inner = id_token + b" 0 1"
    out += b"40 0 obj\n<< /Type /ObjStm /N 1 /First 7 /Length %d >>\nstream\n" % len(inner) + inner + b"\nendstream\nendobj\n"
    out += b"%d 0 obj\n" % num + later + b"\nendobj\n"
    out += b"trailer\n" + ser({"Root": Ref(root), "Encrypt": dict(enc.param), "ID": [docid, docid]}) + b"\n%%EOF\n"
    return bytes(out)


#: A window of ``cases_depth``: its label, the PDF of a depth ``n``, the
#: first failing ``n`` and the levels before and after it.
Window = tuple[str, Callable[[int], bytes], int, int, int]


def depth_windows() -> list[Window]:
    """The windows of ``cases_depth`` (also the families of the API's own
    depth windows: ``oracle/unit/js_api.py``)."""
    return [
        # PDFDocument.__init__ fails from 985 /Prev sections on
        ("prev-tables", lambda n: prev_chain(n, "none"), 985, 5, 5),
        ("prev-streams", lambda n: prev_chain(n, "all"), 985, 2, 1),
        ("prev-stream-bottom", lambda n: prev_chain(n, "bottom"), 985, 2, 1),
        # getobj of the first stream fails from 142 on
        ("length-chain", length_chain, 142, 2, 1),
    ]


def cases_depth() -> list[Case]:
    """Recursions around the depth where Python runs out of frames (the
    first failing N in the comments): docs/architecture.md, "Profondeur de
    récursion"; and the boundaries the mutation pass of pdfparser.rs found
    untested."""
    out: list[Case] = []
    for label, make, first_failing, before, after in depth_windows():
        for n in range(first_failing - before, first_failing + after + 1):
            out.append((f"depth-{label}-{n}", make(n), None))
    # obj and endobj inside an object stream are skipped by PDFStreamParser
    out.append(("objstm-obj-endobj", xref_stream_file(objstm_nums=(5, 6), objstm_items=lambda items: [
        (Raw(b"5"), Raw(b"obj << /A 1 >> endobj")), *items]), None))
    # the stream keyword at the end of the file: nextline() raises PSEOF
    out.append(("stream-keyword-at-eof", tail_object(b"<< /Length 3 >>\nstream"), None))
    out.append(("stream-keyword-at-eof-no-eol", tail_object(b"<< /Length 3 >> stream"), None))
    # /Length around the largest bytes object (PY_SSIZE_T_MAX - 33):
    # BufferedReader.read allocates it (MemoryError), one more is an
    # OverflowError; a BytesIO (objects-bytesio) reads what there is
    for label, length in [("max-bytes", b"9223372036854775774"), ("max-bytes-plus-1", b"9223372036854775775"),
                          ("max-bytes-minus-1", b"9223372036854775773"), ("1e15", b"1000000000000000"),
                          ("2e62", b"4611686018427387904")]:
        doc = base_doc()
        doc.objects = [o for o in doc.objects if o.num != 4]
        doc.add_raw(4, b"4 0 obj\n<</Length " + length + b">>\nstream\n" + CONTENT + b"\nendstream\nendobj\n")
        out.append((f"length-{label}", classic(doc, TRAILER), None))
    # a field of an xref stream 100000 bytes wide (nunpack of a huge int)
    wide = 100000
    rows = b"\x01" + b"\xff" * wide + b"\x00" + b"\x01" + (9).to_bytes(wide, "big") + b"\x00"
    data = flate(rows)
    body = (b"%PDF-1.5\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n"
            b"2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n")
    x = len(body)
    body += (b"3 0 obj\n<< /Type /XRef /Size 3 /Index [1 2] /W [1 %d 1] /Root 1 0 R /Filter /FlateDecode /Length %d >>\nstream\n"
             % (wide, len(data)) + data + b"\nendstream\nendobj\n")
    out.append(("xrefstream-wide-field", body + startxref(x), None))
    # /W of sequences: get_pos repeats fl1 + fl2 + fl3 by the index
    # (entlen * index) before slicing with it; at the index where the size
    # overflows (OverflowError for bytes and str, MemoryError for a list) and
    # one below (an allocation no machine can make: MemoryError), and the
    # first entry (index 0: an empty repetition, then TypeError)
    for label, w in [("bytes", b"[(a) (b) (c)]"), ("str", b"<< /a 1 /b 2 /c 3 >>"), ("list", b"[[1] [2] [3]]")]:
        for objid in (0, 9223372036854775807 // 3, 9223372036854775807 // 3 + 1):
            body = (b"%PDF-1.5\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n"
                    b"2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n")
            x = len(body)
            body += (b"3 0 obj\n<< /Type /XRef /Size 3 /Index [0 9223372036854775807] /W " + w +
                     b" /Root %d 0 R /Length 6 >>\nstream\nabcdef\nendstream\nendobj\n" % objid)
            out.append((f"xrefstream-w-{label}-repeat-{objid}", body + startxref(x), None))
    # a float id deciphered (struct.error for RC4/AES-128 with a string)
    for label, r, cfm in [("rc4", 3, None), ("aes128", 4, "AESV2"), ("identity", 4, None), ("aes256", 6, None)]:
        out.append((f"fallback-float-id-{label}", fallback_float_id(r, cfm, b"<< /Later true >>"), None))
        out.append((f"fallback-float-id-{label}-string", fallback_float_id(r, cfm, b"<< /Later (abcdefghijklmnopqrstu) >>"), None))
        # a bool id: struct.pack takes True as 1
        out.append((f"fallback-bool-id-{label}-string",
                    fallback_float_id(r, cfm, b"<< /Later (abcdefghijklmnopqrstu) >>", b"true", 1), None))
    return out


def main() -> None:
    cases = cases_basic() + cases_fallback() + cases_length() + cases_headers() + cases_prev() + cases_xref_stream() + cases_encryption()
    cases += cases_depth()
    write_corpus("objects", cases)


if __name__ == "__main__":
    main()
