import { describe, expect, it } from "vitest";

import { parseInfoMetadata, resolvePageBoxes } from "../../src/pdf.js";

describe("low-level PDF metadata and page boxes", () => {
  it("decodes literal, hex, indirect, UTF-16BE, PDFDocEncoding, and numeric Info entries", () => {
    const raw = "%PDF-1.7\ntrailer\n<< /Info 9 0 R >>";
    const objects = new Map<number, string>([
      [
        9,
        [
          "9 0 obj",
          "<< /Title (Hello\\053World) /Author <feff004100780065006c> /Subject 10 0 R /Keywords <41909394a0> /Copies 2 >>",
          "endobj"
        ].join("\n")
      ],
      [10, "10 0 obj\n(Indirect\\nSubject)\nendobj"]
    ]);

    expect(parseInfoMetadata(raw, objects)).toMatchObject({
      Title: "Hello+World",
      Author: "Axel",
      Subject: "Indirect\nSubject",
      Keywords: "A\u2019\uFB01\uFB02\u20AC",
      Copies: 2
    });
  });

  it("normalizes crop/media boxes and rotation into pdfplumber coordinates", () => {
    expect(
      resolvePageBoxes({ view: [0, 0, 200, 100], rotate: 0 }, "1 0 obj\n<< /MediaBox [0 0 200 100] /CropBox [10 20 190 90] >>\nendobj")
    ).toMatchObject({
      bbox: [0, 0, 200, 100],
      cropbox: [10, 10, 190, 80],
      width: 200,
      height: 100
    });

    expect(resolvePageBoxes({ view: [0, 0, 200, 100], rotate: 90 }, "1 0 obj\n<< /MediaBox [0 0 200 100] /CropBox [10 20 190 90] >>\nendobj")).toMatchObject({
      bbox: [0, 0, 100, 200],
      cropbox: [20, 10, 90, 190],
      width: 100,
      height: 200
    });
  });
});
