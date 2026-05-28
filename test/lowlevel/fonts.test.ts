import { describe, expect, it } from "vitest";

import { parseFontRecords, parsePageFontObjectNumbers } from "../../src/pdf.js";

describe("low-level PDF font parsing", () => {
  it("extracts BaseFont direct names, Encoding Differences, widths, ascenders, descenders, and flags", () => {
    const objects = new Map<number, string>([
      [
        10,
        [
          "10 0 obj",
          "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica#2dBold /FirstChar 32 /LastChar 33",
          "/Widths [250 722] /Encoding << /Differences [32 /space 33 /A] >>",
          "/FontDescriptor 11 0 R >>",
          "endobj"
        ].join("\n")
      ],
      [11, "11 0 obj\n<< /Type /FontDescriptor /Ascent 718 /Descent -207 /Flags 4 /CharSet (/space/A) >>\nendobj"]
    ]);

    expect(parseFontRecords(objects)).toMatchObject([
      {
        objectNumber: 10,
        baseFont: "Helvetica-Bold",
        subtype: "Type1",
        symbolic: true,
        firstChar: 32,
        widths: [250, 722],
        encodingDifferences: { 32: "space", 33: "A" },
        ascent: 0.718,
        descent: -0.207
      }
    ]);
  });

  it("silently ignores invalid Encoding Differences entries like pdfminer", () => {
    const objects = new Map<number, string>([
      [
        10,
        [
          "10 0 obj",
          "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /FirstChar 32 /LastChar 33",
          "/Widths [250 722] /Encoding << /Differences [/ubuntu /1234 32 /space] >>",
          ">>",
          "endobj"
        ].join("\n")
      ]
    ]);

    expect(parseFontRecords(objects)).toMatchObject([{ encodingDifferences: { 32: "space" } }]);
  });

  it("extracts BaseFont indirect names and descendant font descriptors", () => {
    const objects = new Map<number, string>([
      [19, "19 0 obj /ABCDEE+NotoSans endobj"],
      [20, "20 0 obj << /Type /Font /Subtype /Type0 /BaseFont 19 0 R /DescendantFonts [21 0 R] /ToUnicode 30 0 R >> endobj"],
      [21, "21 0 obj << /Type /Font /Subtype /CIDFontType2 /FontDescriptor 22 0 R >> endobj"],
      [22, "22 0 obj << /Type /FontDescriptor /Ascent 880. /Descent -120. /Flags 32 >> endobj"]
    ]);

    expect(parseFontRecords(objects)).toMatchObject([
      {
        objectNumber: 20,
        baseFont: "ABCDEE+NotoSans",
        subtype: "Type0",
        hasToUnicode: true,
        ascent: 0.88,
        descent: -0.12
      }
    ]);
  });

  it("normalizes CID font CMap names and writing mode like pdfminer", () => {
    const objects = new Map<number, string>([
      [1, "1 0 obj << /CMapName /Identity-V >> stream\nendstream\nendobj"],
      [2, "2 0 obj << /CMapName (Identity-H) >> stream\nendstream\nendobj"],
      [3, "3 0 obj << /CMapName /DLIdent-V >> stream\nendstream\nendobj"],
      [4, "4 0 obj << /CMapName /DLIdent-H >> stream\nendstream\nendobj"],
      [5, "5 0 obj << /CMapName /OneByteIdentityV >> stream\nendstream\nendobj"],
      [6, "6 0 obj << /CMapName /OneByteIdentityH >> stream\nendstream\nendobj"],
      [7, "7 0 obj << /CMapName /V >> stream\nendstream\nendobj"],
      [8, "8 0 obj << /CMapName /H >> stream\nendstream\nendobj"],
      [21, "21 0 obj << /Type /Font /Subtype /Type0 /BaseFont /IdentityVDirect /Encoding /Identity-V >> endobj"],
      [22, "22 0 obj << /Type /Font /Subtype /Type0 /BaseFont /IdentityHDirect /Encoding /Identity-H >> endobj"],
      [23, "23 0 obj << /Type /Font /Subtype /Type0 /BaseFont /IdentityVStream /Encoding 1 0 R >> endobj"],
      [24, "24 0 obj << /Type /Font /Subtype /Type0 /BaseFont /IdentityHStream /Encoding 2 0 R >> endobj"],
      [25, "25 0 obj << /Type /Font /Subtype /Type0 /BaseFont /DLIdentVDirect /Encoding /DLIdent-V >> endobj"],
      [26, "26 0 obj << /Type /Font /Subtype /Type0 /BaseFont /DLIdentHDirect /Encoding /DLIdent-H >> endobj"],
      [27, "27 0 obj << /Type /Font /Subtype /Type0 /BaseFont /DLIdentVStream /Encoding 3 0 R >> endobj"],
      [28, "28 0 obj << /Type /Font /Subtype /Type0 /BaseFont /DLIdentHStream /Encoding 4 0 R >> endobj"],
      [29, "29 0 obj << /Type /Font /Subtype /Type0 /BaseFont /OneByteIdentityV /Encoding 5 0 R >> endobj"],
      [30, "30 0 obj << /Type /Font /Subtype /Type0 /BaseFont /OneByteIdentityH /Encoding 6 0 R >> endobj"],
      [31, "31 0 obj << /Type /Font /Subtype /Type0 /BaseFont /VStream /Encoding 7 0 R >> endobj"],
      [32, "32 0 obj << /Type /Font /Subtype /Type0 /BaseFont /HStream /Encoding 8 0 R >> endobj"],
      [33, "33 0 obj << /Type /Font /Subtype /Type0 /BaseFont /NoEncoding >> endobj"]
    ]);

    const verticalByBaseFont = Object.fromEntries(parseFontRecords(objects).map((record) => [record.baseFont, record.vertical]));

    expect(verticalByBaseFont).toMatchObject({
      IdentityVDirect: true,
      IdentityHDirect: false,
      IdentityVStream: true,
      IdentityHStream: false,
      DLIdentVDirect: true,
      DLIdentHDirect: false,
      DLIdentVStream: true,
      DLIdentHStream: false,
      OneByteIdentityV: true,
      OneByteIdentityH: false,
      VStream: true,
      HStream: false,
      NoEncoding: false
    });
  });

  it("finds page-scoped font object numbers without collecting unrelated resources", () => {
    const pageObject = "1 0 obj\n<< /Type /Page /Resources << /Font << /F1 10 0 R /F2 20 0 R >> /XObject << /Im1 30 0 R >> >> >>\nendobj";

    expect(parsePageFontObjectNumbers(pageObject)).toEqual([10, 20]);
  });
});
