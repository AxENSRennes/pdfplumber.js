import { describe, expect, it } from "vitest";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { open } from "../../src/index.js";

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, "../..");

describe("public API", () => {
  it("opens a PDF and exposes pdfplumber-shaped pages", async () => {
    const document = await open(path.join(repoRoot, "pdfplumber-python/tests/pdfs/nics-background-checks-2015-11.pdf"));
    try {
      expect(document.pages).toHaveLength(1);
      expect(document.pages[0].page_number).toBe(1);
      expect(document.pages[0].bbox).toEqual([0, 0, 1008, 612]);
    } finally {
      await document.close();
    }
  });
});
