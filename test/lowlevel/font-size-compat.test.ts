import { execFileSync } from "node:child_process";

import { describe, expect, it } from "vitest";

import { open } from "../../src/index.js";
import type { SearchResult } from "../../src/types.js";

interface DigitLineSize {
  text: string;
  expectedSize: number;
}

function pdfminerDigitLineSizes(path: string): DigitLineSize[] {
  const code = `
import json
from pdfminer.high_level import extract_pages
from pdfminer.layout import LTChar, LTTextBox

out = []
for page in extract_pages(${JSON.stringify(path)}):
    for text_box in page:
        if isinstance(text_box, LTTextBox):
            for line in text_box:
                text = line.get_text().strip()
                if text.isdigit():
                    out.append({
                        "text": text,
                        "expectedSize": int(text),
                    })
print(json.dumps(out))
`;
  return JSON.parse(execFileSync("wsl_venv/bin/python", ["-c", code], { encoding: "utf8" })) as DigitLineSize[];
}

describe("low-level pdfminer font-size compatibility", () => {
  it("rounds digit line LTChar sizes like pdfminer on the upstream font-size fixture", async () => {
    const path = "pdfminer-six/samples/font-size-test.pdf";
    const expected = pdfminerDigitLineSizes(path);
    const pdf = await open(path);
    try {
      const actual: Array<DigitLineSize & { sizes: number[] }> = [];
      for (const page of pdf.pages) {
        const lines = await Promise.resolve(page.extract_text_lines({ return_chars: true })) as SearchResult[];
        actual.push(...lines
          .filter((line) => /^\d+$/.test(line.text.trim()))
          .map((line) => ({
            text: line.text.trim(),
            expectedSize: Number(line.text.trim()),
            sizes: (line.chars ?? []).map((char) => Math.round(Number(char.size)))
          })));
      }
      expect(actual.map(({ text, expectedSize }) => ({ text, expectedSize }))).toEqual(expected);
      for (const line of actual) {
        expect(line.sizes.length).toBeGreaterThan(0);
        expect(line.sizes.every((size) => size === line.expectedSize)).toBe(true);
      }
    } finally {
      await pdf.close();
    }
  });
});
