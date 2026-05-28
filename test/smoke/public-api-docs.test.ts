import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, "../..");

describe("public API documentation", () => {
  it("names the stable extraction surface and browser inputs", async () => {
    const readme = await readFile(path.join(repoRoot, "README.md"), "utf8");
    const requiredTerms = [
      "ArrayBuffer",
      "Blob",
      "URL",
      "metadata",
      "pages",
      "bbox",
      "cropbox",
      "chars",
      "extractText",
      "extractWords",
      "search",
      "filter",
      "crop",
      "dedupeChars",
      "rects",
      "lines",
      "curves",
      "colors",
      "annots",
      "mcid",
      "tag",
      "images",
      "srcsize",
      "colorspace",
      "findTable",
      "extractTable",
      "vertical_strategy",
      "horizontal_strategy"
    ];

    for (const term of requiredTerms) {
      expect(readme).toContain(term);
    }
  });
});
