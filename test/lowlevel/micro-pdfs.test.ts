import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { beforeAll, describe, expect, it } from "vitest";

import { open } from "../../src/index.js";

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, "../..");
const fixtureDir = path.join(repoRoot, "test/fixtures/micro-pdfs");
const python = path.join(repoRoot, "wsl_venv/bin/python");

const expectedFiles = [
  "annotations-extended.pdf",
  "annotations.pdf",
  "broken-recovery.pdf",
  "contents-indirect-array.pdf",
  "encodings-cmap.pdf",
  "encrypted-password.pdf",
  "form-xobject-matrix.pdf",
  "graphics-state-colors.pdf",
  "image-xobject.pdf",
  "images-advanced.pdf",
  "inherited-resources.pdf",
  "page-boxes-rotate.pdf",
  "text-operators.pdf",
  "type3-simple.pdf",
  "vector-objects.pdf",
  "vertical-rtl-text.pdf"
];

interface Snapshot {
  text: string | null;
  chars: number;
  lines: number;
  rects: number;
  curves: number;
  images: number;
  annots: number;
  hyperlinks: number;
  bbox: number[];
  cropbox: number[];
}

function normalizeNumbers(values: readonly number[]): number[] {
  return values.map((value) => Math.round(value * 1000) / 1000);
}

function pythonSnapshots(): Record<string, Snapshot> {
  const code = `
import json
from pathlib import Path
import pdfplumber
out = {}
for path in sorted(Path(${JSON.stringify(fixtureDir)}).glob("*.pdf")):
    kwargs = {"password": "user"} if path.name == "encrypted-password.pdf" else {}
    with pdfplumber.open(path, **kwargs) as pdf:
        page = pdf.pages[0]
        out[path.name] = {
            "text": page.extract_text(),
            "chars": len(page.chars),
            "lines": len(page.lines),
            "rects": len(page.rects),
            "curves": len(page.curves),
            "images": len(page.images),
            "annots": len(page.annots),
            "hyperlinks": len(page.hyperlinks),
            "bbox": list(page.bbox),
            "cropbox": list(page.cropbox),
        }
print(json.dumps(out))
`;
  return JSON.parse(execFileSync(python, ["-c", code], { cwd: repoRoot, encoding: "utf8" })) as Record<string, Snapshot>;
}

describe("generated micro-PDF fixtures", () => {
  beforeAll(() => {
    execFileSync(python, ["scripts/generate-micro-pdfs.py"], { cwd: repoRoot, stdio: "pipe" });
  });

  it("generates one small PDF per low-level feature", () => {
    for (const file of expectedFiles) {
      expect(existsSync(path.join(fixtureDir, file)), file).toBe(true);
    }
  });

  it("matches Python pdfplumber counts, text, and page boxes for every micro fixture", async () => {
    const expected = pythonSnapshots();
    expect(Object.keys(expected).sort()).toEqual(expectedFiles);

    for (const file of expectedFiles) {
      const document = await open(path.join(fixtureDir, file), file === "encrypted-password.pdf" ? { password: "user" } : {});
      try {
        const page = document.pages[0];
        const actual: Snapshot = {
          text: await Promise.resolve(page.extractText()),
          chars: page.chars.length,
          lines: page.lines.length,
          rects: page.rects.length,
          curves: page.curves.length,
          images: page.images.length,
          annots: page.annots.length,
          hyperlinks: page.hyperlinks.length,
          bbox: normalizeNumbers(page.bbox),
          cropbox: normalizeNumbers(page.cropbox)
        };
        const expectedSnapshot = {
          ...expected[file],
          bbox: normalizeNumbers(expected[file].bbox),
          cropbox: normalizeNumbers(expected[file].cropbox)
        };
        expect(actual, file).toEqual(expectedSnapshot);
      } finally {
        await document.close();
      }
    }
  });

  it("keeps focused feature expectations visible for the hard-case micro fixtures", async () => {
    const byFile: Record<string, Partial<Snapshot> & { contains?: string }> = {
      "encodings-cmap.pdf": { text: "Afi?", chars: 3 },
      "vertical-rtl-text.pdf": { text: "縦\n書\nجبا", chars: 5 },
      "inherited-resources.pdf": { text: "Inherited", chars: 9 },
      "contents-indirect-array.pdf": { text: "Indirect\nArray", chars: 13 },
      "graphics-state-colors.pdf": { lines: 1, rects: 4 },
      "annotations-extended.pdf": { annots: 8 },
      "images-advanced.pdf": { images: 2 },
      "encrypted-password.pdf": { text: "A BC", chars: 3 },
      "broken-recovery.pdf": { text: "Recovered", chars: 9 }
    };
    for (const [file, expected] of Object.entries(byFile)) {
      const document = await open(path.join(fixtureDir, file), file === "encrypted-password.pdf" ? { password: "user" } : {});
      try {
        const page = document.pages[0];
        if (expected.text !== undefined) expect(await Promise.resolve(page.extractText()), file).toBe(expected.text);
        if (expected.chars !== undefined) expect(page.chars.length, file).toBe(expected.chars);
        if (expected.lines !== undefined) expect(page.lines.length, file).toBe(expected.lines);
        if (expected.rects !== undefined) expect(page.rects.length, file).toBe(expected.rects);
        if (expected.images !== undefined) expect(page.images.length, file).toBe(expected.images);
        if (expected.annots !== undefined) expect(page.annots.length, file).toBe(expected.annots);
      } finally {
        await document.close();
      }
    }
  });
});
