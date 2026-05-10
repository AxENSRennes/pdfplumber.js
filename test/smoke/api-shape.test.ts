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

  it("search accepts non-global regular expressions like Python compiled patterns", async () => {
    const document = await open(path.join(repoRoot, "pdfplumber-python/tests/pdfs/scotus-transcript-p1.pdf"));
    try {
      const page = document.pages[0];
      expect((await page.search(/Final/)).map((match) => match.text)).toEqual(["Final"]);
      expect((await page.search(/Final/g)).map((match) => match.text)).toEqual(["Final"]);
      expect(() => page.search(/Final/, { regex: false })).toThrow(/regex=false/);
      expect(() => page.search(/Final/, { case: false })).toThrow(/case=false/);
    } finally {
      await document.close();
    }
  });

  it("matches pdfplumber issue-297 page suppression narrowly", async () => {
    const document = await open(path.join(repoRoot, "pdfplumber-python/tests/pdfs/issue-297-example.pdf"));
    try {
      expect(document.metadata.Copies).toBe(0);
      expect(document.pages).toHaveLength(0);
    } finally {
      await document.close();
    }
  });

  it("does not suppress pages for unrelated PDFs containing %%Postscript OFF", async () => {
    for (const name of [
      "from-oss-fuzz/load/4927662560968704.pdf",
      "from-oss-fuzz/load/5317294594523136.pdf"
    ]) {
      const document = await open(path.join(repoRoot, "pdfplumber-python/tests/pdfs", name));
      try {
        expect(document.pages.length).toBeGreaterThan(0);
      } finally {
        await document.close();
      }
    }
  });
});
