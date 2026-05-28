import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";

import { parsePageLabelsLikePdfminer, parsePdfDocument } from "../../src/pdf/document.js";
import { open } from "../../src/index.js";

describe("low-level pdfminer page label compatibility", () => {
  it("generates page labels from catalog number-tree ranges like pdfminer", () => {
    const raw = [
      "1 0 obj << /Type /Catalog /Pages 2 0 R /PageLabels << /Nums [0 << /S /r /St 3 >> 2 << /S /D /St 1 >> 4 << /S /D /St 1 >>] >> >> endobj",
      "2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R 5 0 R 6 0 R 7 0 R] /Count 5 >> endobj",
      "3 0 obj << /Type /Page /Parent 2 0 R >> endobj",
      "4 0 obj << /Type /Page /Parent 2 0 R >> endobj",
      "5 0 obj << /Type /Page /Parent 2 0 R >> endobj",
      "6 0 obj << /Type /Page /Parent 2 0 R >> endobj",
      "7 0 obj << /Type /Page /Parent 2 0 R >> endobj"
    ].join("\n");

    expect(parsePageLabelsLikePdfminer(parsePdfDocument(raw), 5)).toEqual(["iii", "iv", "1", "2", "1"]);
  });

  it("matches pdfminer page labels for the upstream pagelabels fixture", async () => {
    const expected = ["iii", "iv", "1", "2", "1"];
    const raw = readFileSync("pdfminer-six/samples/contrib/pagelabels.pdf", "latin1");

    expect(parsePageLabelsLikePdfminer(parsePdfDocument(raw), 5)).toEqual(expected);

    const pdf = await open("pdfminer-six/samples/contrib/pagelabels.pdf");
    expect(pdf.pages.map((page) => page.label)).toEqual(expected);
    await pdf.close();
  });

  it("supports nested kids, prefixes, alpha styles, and missing initial range fallback like pdfminer", () => {
    const raw = [
      "1 0 obj << /Type /Catalog /Pages 2 0 R /PageLabels 8 0 R >> endobj",
      "2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R 5 0 R 6 0 R] /Count 4 >> endobj",
      "3 0 obj << /Type /Page /Parent 2 0 R >> endobj",
      "4 0 obj << /Type /Page /Parent 2 0 R >> endobj",
      "5 0 obj << /Type /Page /Parent 2 0 R >> endobj",
      "6 0 obj << /Type /Page /Parent 2 0 R >> endobj",
      "8 0 obj << /Kids [9 0 R] >> endobj",
      "9 0 obj << /Nums [2 << /P (App-) /S /A /St 2 >>] >> endobj"
    ].join("\n");

    expect(parsePageLabelsLikePdfminer(parsePdfDocument(raw), 4)).toEqual(["", "", "App-B", "App-C"]);
  });

  it("returns null when the catalog has no PageLabels like pdfminer.get_page_labels", () => {
    const raw = "1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Count 0 >> endobj";

    expect(parsePageLabelsLikePdfminer(parsePdfDocument(raw), 0)).toBeNull();
  });
});
