import { execFileSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import { open, type PDFObject } from "../../src/index.js";

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, "../..");
const python = path.join(repoRoot, "wsl_venv/bin/python");

function fixture(relativePath: string): string {
  return path.join(repoRoot, relativePath);
}

function pythonJson<T>(code: string): T {
  return JSON.parse(execFileSync(python, ["-c", code], { cwd: repoRoot, encoding: "utf8" })) as T;
}

interface PythonPublicSummary {
  metadata: Record<string, unknown>;
  pages: number;
  annots: Array<Record<string, unknown>>;
  hyperlinks: number;
}

interface PythonOpenError {
  name: string;
}

function pythonPublicSummary(relativePath: string): PythonPublicSummary {
  const code = `
import json
import pdfplumber

def clean_number(value):
    return round(value, 5) if isinstance(value, (int, float)) else value

def annot_summary(annot):
    return {
        "x0": clean_number(annot.get("x0")),
        "y0": clean_number(annot.get("y0")),
        "x1": clean_number(annot.get("x1")),
        "y1": clean_number(annot.get("y1")),
        "top": clean_number(annot.get("top")),
        "bottom": clean_number(annot.get("bottom")),
        "uri": annot.get("uri"),
        "title": annot.get("title"),
        "contents": annot.get("contents"),
    }

with pdfplumber.open(${JSON.stringify(relativePath)}) as pdf:
    page = pdf.pages[0] if pdf.pages else None
    print(json.dumps({
        "metadata": pdf.metadata,
        "pages": len(pdf.pages),
        "annots": [annot_summary(annot) for annot in (page.annots if page else [])],
        "hyperlinks": len(page.hyperlinks) if page else 0,
    }, ensure_ascii=False))
`;
  return pythonJson<PythonPublicSummary>(code);
}

function pythonOpenError(relativePath: string): PythonOpenError {
  const code = `
import json
import pdfplumber

try:
    with pdfplumber.open(${JSON.stringify(relativePath)}):
        print(json.dumps({"name": None}))
except Exception as exc:
    print(json.dumps({"name": type(exc).__name__}))
`;
  return pythonJson<PythonOpenError>(code);
}

function cleanNumber(value: unknown): unknown {
  return typeof value === "number" ? Number(value.toFixed(5)) : value;
}

function annotSummary(annot: PDFObject): Record<string, unknown> {
  return {
    x0: cleanNumber(annot.x0),
    y0: cleanNumber(annot.y0),
    x1: cleanNumber(annot.x1),
    y1: cleanNumber(annot.y1),
    top: cleanNumber(annot.top),
    bottom: cleanNumber(annot.bottom),
    uri: annot.uri ?? null,
    title: annot.title ?? null,
    contents: annot.contents ?? null
  };
}

describe("PDF.js public capability fixtures through the pdfplumber API", () => {
  it("exposes Info metadata, including custom keys, like Python pdfplumber", async () => {
    for (const relativePath of ["pdfjs/test/pdfs/basicapi.pdf", "pdfjs/test/pdfs/tracemonkey.pdf", "pdfjs/test/pdfs/bug1606566.pdf"]) {
      const expected = pythonPublicSummary(relativePath);
      const document = await open(fixture(relativePath));
      try {
        expect(document.metadata).toEqual(expected.metadata);
        expect(document.pages).toHaveLength(expected.pages);
      } finally {
        await document.close();
      }
    }
  });

  it("keeps PDF.js GoToR, GoToE, and Launch unsafeUrl details out of public hyperlinks", async () => {
    for (const relativePath of ["pdfjs/test/pdfs/basicapi.pdf", "pdfjs/test/pdfs/bug766086.pdf", "pdfjs/test/pdfs/issue8844.pdf", "pdfjs/test/pdfs/issue17846.pdf"]) {
      const expected = pythonPublicSummary(relativePath);
      const document = await open(fixture(relativePath));
      try {
        const page = document.pages[0];
        expect(page.annots.map(annotSummary)).toEqual(expected.annots);
        expect(page.hyperlinks).toHaveLength(expected.hyperlinks);
      } finally {
        await document.close();
      }
    }
  });

  it("raises the pdfminer-compatible stable error for corrupt metadata streams", async () => {
    const relativePath = "pdfjs/test/pdfs/PDFBOX-3148-2-fuzzed.pdf";
    expect(pythonOpenError(relativePath).name).toBe("PdfminerException");
    await expect(open(fixture(relativePath))).rejects.toMatchObject({ name: "PdfminerException" });
  });
});
