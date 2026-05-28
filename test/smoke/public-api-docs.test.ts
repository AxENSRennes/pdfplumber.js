import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, "../..");

describe("public API documentation", () => {
  it("names the stable extraction surface and browser inputs", async () => {
    const readme = await readFile(path.join(repoRoot, "README.md"), "utf8");
    const publicApi = await readFile(path.join(repoRoot, "docs/public-api.md"), "utf8");
    const combinedDocs = `${readme}\n${publicApi}`;
    const requiredTerms = [
      "ArrayBuffer",
      "Uint8Array",
      "Blob",
      "URL",
      "metadata",
      "pages",
      "pageNumber",
      "label",
      "bbox",
      "mediabox",
      "cropbox",
      "artbox",
      "bleedbox",
      "trimbox",
      "chars",
      "extractText",
      "extractTextLines",
      "extractWords",
      "search",
      "filter",
      "crop",
      "withinBbox",
      "outsideBbox",
      "dedupeChars",
      "rects",
      "lines",
      "curves",
      "`pts`",
      "`path` command",
      "document-level vector edges",
      "colors",
      "annots",
      "mcid",
      "tag",
      "data",
      "images",
      "srcsize",
      "imagemask",
      "colorspace",
      "findTable",
      "findTables",
      "extractTable",
      "extractTables",
      "TableAxisGroup",
      "vertical_strategy",
      "horizontal_strategy"
    ];

    for (const term of requiredTerms) {
      expect(readme).toContain(term);
    }

    for (const typeName of [
      "PDFInput",
      "OpenOptions",
      "PDFPlumberDocument",
      "PDFPlumberPage",
      "PDFObject",
      "SearchResult",
      "ExtractTextOptions",
      "WordOptions",
      "SearchOptions",
      "TextLineOptions",
      "CropOptions",
      "DedupeOptions",
      "TableOptions",
      "TableStrategy",
      "ExplicitTableLine",
      "Table",
      "TableAxisGroup"
    ]) {
      expect(combinedDocs, typeName).toContain(typeName);
    }
  });
});
