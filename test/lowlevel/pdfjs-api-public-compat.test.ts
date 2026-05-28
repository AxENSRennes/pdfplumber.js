import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
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

class PdfBuilder {
  private objects: Uint8Array[] = [];

  add(body: string): number {
    this.objects.push(latin1Bytes(body));
    return this.objects.length;
  }

  stream(dictionary: string, data: string): number {
    const bytes = latin1Bytes(data);
    return this.add(`<< ${dictionary} /Length ${bytes.length} >>\nstream\n${data}\nendstream`);
  }

  write(rootObject: number): Uint8Array {
    const chunks: Uint8Array[] = [latin1Bytes("%PDF-1.7\n%\xe2\xe3\xcf\xd3\n")];
    const offsets = [0];
    let length = chunks[0].length;
    for (let index = 0; index < this.objects.length; index += 1) {
      offsets.push(length);
      const prefix = latin1Bytes(`${index + 1} 0 obj\n`);
      const suffix = latin1Bytes("\nendobj\n");
      chunks.push(prefix, this.objects[index], suffix);
      length += prefix.length + this.objects[index].length + suffix.length;
    }
    const xrefOffset = length;
    chunks.push(latin1Bytes(`xref\n0 ${this.objects.length + 1}\n0000000000 65535 f \n`));
    for (const offset of offsets.slice(1)) chunks.push(latin1Bytes(`${String(offset).padStart(10, "0")} 00000 n \n`));
    chunks.push(latin1Bytes(`trailer\n<< /Size ${this.objects.length + 1} /Root ${rootObject} 0 R >>\nstartxref\n${xrefOffset}\n%%EOF\n`));
    return concatBytes(...chunks);
  }
}

function latin1Bytes(value: string): Uint8Array {
  const bytes = new Uint8Array(value.length);
  for (let index = 0; index < value.length; index += 1) bytes[index] = value.charCodeAt(index) & 0xff;
  return bytes;
}

function concatBytes(...chunks: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(chunks.reduce((sum, chunk) => sum + chunk.length, 0));
  let offset = 0;
  for (const chunk of chunks) {
    out.set(chunk, offset);
    offset += chunk.length;
  }
  return out;
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

function pythonPublicSummaryForBytes(pdfBytes: Uint8Array): PythonPublicSummary {
  const dir = mkdtempSync(path.join(tmpdir(), "pdfplumber-js-annotations-"));
  const pdfPath = path.join(dir, "annotations.pdf");
  try {
    writeFileSync(pdfPath, pdfBytes);
    return pythonPublicSummary(pdfPath);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
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

function publicUriAnnotationsPdf(): Uint8Array {
  const pdf = new PdfBuilder();
  const contents = pdf.stream("", "");
  const httpLink = pdf.add("<< /Type /Annot /Subtype /Link /Rect [10 20 70 40] /Contents (Link note) /T (Link Title) /A << /S /URI /URI (http://www.ctan.org/tex-archive/info/lshort) >> >>");
  const protocolLessLink = pdf.add("<< /Type /Annot /Subtype /Link /Rect [80 20 150 40] /A << /S /URI /URI (www.hmrc.gov.uk) >> >>");
  const utf8Link = pdf.add("<< /Type /Annot /Subtype /Link /Rect [10 60 150 80] /A << /S /URI /URI (http://www.example.com/\\303\\274\\303\\266\\303\\244) >> >>");
  const page = pdf.add(`<< /Type /Page /MediaBox [0 0 200 200] /Resources << >> /Contents ${contents} 0 R /Annots [${httpLink} 0 R ${protocolLessLink} 0 R ${utf8Link} 0 R] >>`);
  const pages = pdf.add(`<< /Type /Pages /Kids [${page} 0 R] /Count 1 >>`);
  const catalog = pdf.add(`<< /Type /Catalog /Pages ${pages} 0 R >>`);
  return pdf.write(catalog);
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

  it("extracts public link annotation URI, contents, title, and geometry like Python pdfplumber", async () => {
    const pdfBytes = publicUriAnnotationsPdf();
    const expected = pythonPublicSummaryForBytes(pdfBytes);
    const document = await open(pdfBytes);
    try {
      const page = document.pages[0];
      expect(page.annots.map(annotSummary)).toEqual(expected.annots);
      expect(page.hyperlinks.map(annotSummary)).toEqual(expected.annots);
    } finally {
      await document.close();
    }
  });

  it("raises the pdfminer-compatible stable error for corrupt metadata streams", async () => {
    const relativePath = "pdfjs/test/pdfs/PDFBOX-3148-2-fuzzed.pdf";
    expect(pythonOpenError(relativePath).name).toBe("PdfminerException");
    await expect(open(fixture(relativePath))).rejects.toMatchObject({ name: "PdfminerException" });
  });
});
