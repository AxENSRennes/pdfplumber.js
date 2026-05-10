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

  it("finds page-scoped font object numbers without collecting unrelated resources", () => {
    const pageObject = "1 0 obj\n<< /Type /Page /Resources << /Font << /F1 10 0 R /F2 20 0 R >> /XObject << /Im1 30 0 R >> >> >>\nendobj";

    expect(parsePageFontObjectNumbers(pageObject)).toEqual([10, 20]);
  });
});
