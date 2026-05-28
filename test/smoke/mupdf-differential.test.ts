import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, "../..");
const allowedClassifications = new Set([
  "upstream-compatible-limitation",
  "backend-gap",
  "unsupported-feature",
  "intentional-difference",
  "duplicate-coverage",
  "excluded-behavior"
]);

interface MupdfClassification {
  pdf: string;
  page: number;
  subsystem: string;
  classification: string;
  summary: string;
  evidence: string;
  rationale: string;
}

function read(relativePath: string): string {
  return fs.readFileSync(path.join(repoRoot, relativePath), "utf8");
}

function defaultMupdfPdfs(): string[] {
  const script = read("scripts/compare-mupdf-backend.mjs");
  const match = /const DEFAULT_PDFS = \[(?<body>[\s\S]*?)\];/.exec(script);
  if (!match?.groups?.body) throw new Error("Could not find DEFAULT_PDFS in MuPDF audit script");
  return [...match.groups.body.matchAll(/"([^"]+)"/g)].map((item) => item[1]);
}

function classifications(): MupdfClassification[] {
  return JSON.parse(read("test/fixtures/mupdf-differential-classifications.json")) as MupdfClassification[];
}

describe("MuPDF differential audit", () => {
  it("keeps MuPDF as a dev-only differential tool outside the public package", () => {
    const pkg = JSON.parse(read("package.json")) as Record<string, any>;
    const script = read("scripts/compare-mupdf-backend.mjs");
    expect(pkg.dependencies?.mupdf).toBeUndefined();
    expect(pkg.devDependencies?.mupdf).toEqual(expect.any(String));
    expect(String(pkg.scripts["audit:mupdf"])).toContain("scripts/compare-mupdf-backend.mjs");
    expect(String(pkg.scripts["audit:mupdf"])).toContain("--pages 1");
    expect(String(pkg.scripts["audit:mupdf:check"])).toContain("--check-classifications test/fixtures/mupdf-differential-classifications.json");
    expect(String(pkg.scripts["build:browser"])).not.toContain("mupdf");
    expect(script).toContain('argValue("--pages", "1")');
    expect(script).toContain('argValue("--check-classifications", null)');
  });

  it("classifies every default MuPDF differential input", () => {
    const defaultPdfs = defaultMupdfPdfs();
    const records = classifications();
    const recordKeys = new Set(records.map((record) => `${record.pdf}#${record.page}`));

    expect(defaultPdfs).not.toHaveLength(0);
    expect(records).toHaveLength(defaultPdfs.length);
    expect(recordKeys.size).toBe(records.length);

    for (const pdf of defaultPdfs) {
      expect(records.some((record) => record.pdf === pdf && record.page === 1), pdf).toBe(true);
    }

    for (const record of records) {
      expect(defaultPdfs, record.pdf).toContain(record.pdf);
      expect(fs.existsSync(path.join(repoRoot, record.pdf)), record.pdf).toBe(true);
      expect(record.page, record.pdf).toBeGreaterThan(0);
      expect(record.subsystem, record.pdf).toMatch(/^(text|words|geometry|boxes|paths|annotations|marked-content|image-metadata|table)$/);
      expect(allowedClassifications.has(record.classification), record.pdf).toBe(true);
      expect(record.summary, record.pdf).toEqual(expect.any(String));
      expect(record.evidence, record.pdf).toContain("compare-mupdf-backend.mjs");
      expect(record.rationale, record.pdf).toMatch(/MuPDF|pdfplumber|pdfminer|Python/);
    }
  });

  it("documents that MuPDF is not the primary oracle", () => {
    const docs = read("docs/pdf-engine-comparison.md");
    expect(docs).toContain("MuPDF.js");
    expect(docs).toContain("optional/prototype engine");
    expect(docs).toContain("native pdfminer-style parsing/extraction");
    expect(docs).toContain("named, tested runtime/operator capabilities");
    expect(docs).toContain("not as the primary oracle or default extraction path");
    expect(docs).toContain("audit:mupdf:check");
  });
});
