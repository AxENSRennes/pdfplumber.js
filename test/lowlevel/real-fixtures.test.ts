import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import {
  extractPageContent,
  parseFontRecords,
  parseImageResources,
  parsePathOps,
  parsePdfObjects,
  parseTextMatrixOps,
  parseTransformOps,
  resolvePageBoxes
} from "../../src/pdf.js";

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, "../..");

function rawPdf(relativePath: string): string {
  return readFileSync(path.join(repoRoot, relativePath), "latin1");
}

function firstPage(objects: Map<number, string>): string {
  const page = [...objects.values()].find((object) => /\/Type\s*\/Page\b/.test(object) && !/\/Type\s*\/Pages\b/.test(object));
  if (!page) throw new Error("fixture has no page object");
  return page;
}

describe("low-level parsing against imported PDF fixtures", () => {
  it("extracts content and font records from small PDF.js text fixtures", () => {
    const identityObjects = parsePdfObjects(rawPdf("pdfjs/test/pdfs/IdentityToUnicodeMap_charCodeOf.pdf"));
    const identityPageContent = extractPageContent(firstPage(identityObjects), identityObjects);

    expect(parseFontRecords(identityObjects)).toMatchObject([{ baseFont: "Times-Roman", subtype: "Type1", hasToUnicode: false }]);
    expect(identityPageContent).toContain("(ABCdef) Tj");

    const trueTypeObjects = parsePdfObjects(rawPdf("pdfjs/test/pdfs/TrueType_without_cmap.pdf"));
    const trueTypeContent = extractPageContent(firstPage(trueTypeObjects), trueTypeObjects);

    expect(parseFontRecords(trueTypeObjects)).toMatchObject([{ baseFont: "NRKWIM+Masis", subtype: "TrueType", hasToUnicode: false }]);
    expect(parseTextMatrixOps(trueTypeContent)).toEqual([[11.6758, 0, 3.608, 11.6758, 10, 20]]);
  });

  it("finds image resources and transforms in a compact bitmap fixture", () => {
    const objects = parsePdfObjects(rawPdf("pdfjs/test/pdfs/bitmap-mmr.pdf"));
    const page = firstPage(objects);
    const content = extractPageContent(page, objects);

    expect(parseTransformOps(content)).toEqual([[399, 0, 0, 400, 0, 0]]);
    expect(parseImageResources(page, objects, content).map((image) => [image.name, image.width, image.height, image.bits])).toEqual([["Im", 399, 400, 1]]);
  });

  it("keeps page-box normalization stable on an imported pdfplumber fixture", () => {
    const objects = parsePdfObjects(rawPdf("pdfplumber-python/tests/pdfs/page-boxes-example.pdf"));
    const page = firstPage(objects);
    const boxes = resolvePageBoxes({ view: [0, 0, 612, 792], rotate: 0 }, page);

    expect(boxes).toMatchObject({
      bbox: [0, 0, 623.62205, 870.23622],
      mediabox: [0, 0, 623.62205, 870.23622],
      cropbox: [14.17323, 42.51968, 581.10236, 856.06299],
      artbox: [42.51969, 70.86614, 552.75591, 827.71653],
      trimbox: [28.34646, 56.69291, 566.92913, 841.88976],
      width: 623.62205,
      height: 870.23622
    });
    expect(parsePathOps(extractPageContent(page, objects))).toEqual([]);
  });
});
