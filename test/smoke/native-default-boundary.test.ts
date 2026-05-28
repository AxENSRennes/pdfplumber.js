import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, "../..");

function read(relativePath: string): string {
  return fs.readFileSync(path.join(repoRoot, relativePath), "utf8");
}

describe("default native extraction boundary", () => {
  it("keeps native pdfminer-style subsystems wired into open()", () => {
    const openSource = read("src/open.ts");
    const pageSource = read("src/page.ts");
    const pdfSource = read("src/pdf.ts");
    const publicSurfaceTest = read("test/smoke/stable-public-surface.test.ts");
    const docs = `${read("README.md")}\n${read("docs/public-api.md")}\n${read("docs/pdf-engine-comparison.md")}`;

    for (const importLine of [
      'from "./pdf/content.js"',
      'from "./pdf/document.js"',
      'from "./pdf/resources.js"',
      'from "./pdfminer-compat.js"',
      'from "./pdf.js"'
    ]) {
      expect(openSource, importLine).toContain(importLine);
    }

    const defaultPathCalls = [
      "parsePdfDocument(raw",
      "validateStreamsLikePdfminer(compatContext)",
      "parseInfoMetadata(raw, rawObjects)",
      "parsePageLabelsLikePdfminer(store, pageTotal)",
      "store.getPageModel(",
      "parsePageFontResourceMap(pageOwner, store)",
      "parsePageFontObjectNumbers(pageOwner, store)",
      "resolvePageBoxes(pdfPage, pageObjectText)",
      "extractPageContent(pageOwner, store)",
      "collectGraphicsHintsFromContent(pageContent",
      "parseColorSpaceResources(pageOwner, store)",
      "extractPageObjects(",
      "parseImageResources(pageOwner, store, pageContent)",
      "extractPredefinedCMapCharsFromContent(",
      "sortAnnotationsByRawPageOrder(annotationList, pageModel, store)",
      "annotationResolutionErrorLikePdfminer(pageModel, store)",
      "annotationStringDecodeErrorLikePdfminer(",
      "buildLayoutObjects(lowLevel.chars, lowLevel.images"
    ];
    for (const call of defaultPathCalls) {
      expect(openSource, call).toContain(call);
    }

    const pdfjsLoading = openSource.indexOf("(pdfjs as any).getDocument");
    for (const nativeCall of ["parsePdfDocument(raw", "validateStreamsLikePdfminer(compatContext)", "parseInfoMetadata(raw, rawObjects)"]) {
      expect(openSource.indexOf(nativeCall), nativeCall).toBeGreaterThanOrEqual(0);
      expect(openSource.indexOf(nativeCall), `${nativeCall} should run before PDF.js document loading`).toBeLessThan(pdfjsLoading);
    }

    for (const tableMethod of ["findTable(", "findTables(", "extractTable(", "extractTables("]) {
      expect(pageSource, tableMethod).toContain(tableMethod);
    }
    expect(pdfSource).toContain("path.operations.map");

    for (const publicSurface of [
      "metadata",
      "bbox",
      "cropbox",
      "chars.length",
      "extractWords",
      "search",
      "crop(",
      "withinBbox",
      "outsideBbox",
      "filter(",
      "rects",
      "lines",
      "curves",
      ".path",
      "non_stroking_color",
      "annots",
      "data",
      "images",
      "srcsize",
      "mcid",
      "findTables",
      "explicit_vertical_lines",
      "dedupeChars"
    ]) {
      expect(publicSurfaceTest, publicSurface).toContain(publicSurface);
    }

    expect(docs).toContain("native pdfminer-style parsing and extraction");
    expect(docs).toContain("PDF.js is not a public engine selector or oracle");
    expect(read("test/lowlevel/marked-content-compat.test.ts")).toContain("pdfplumber marked-content compatibility");
    expect(read("test/lowlevel/annotation-compat.test.ts")).toContain("parsed data dictionaries like Python pdfplumber");
  });
});
