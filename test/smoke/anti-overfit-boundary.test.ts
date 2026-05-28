import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, "../..");

function walk(relativePath: string): string[] {
  const absolutePath = path.join(repoRoot, relativePath);
  return fs.readdirSync(absolutePath, { withFileTypes: true }).flatMap((entry) => {
    const child = path.join(relativePath, entry.name);
    return entry.isDirectory() ? walk(child) : [child];
  });
}

describe("anti-overfit production boundary", () => {
  it("keeps production extraction code independent of fixture and corpus identities", () => {
    const sourceFiles = walk("src").filter((file) => file.endsWith(".ts")).sort();
    const forbiddenPatterns = [
      {
        label: "test/corpus path",
        pattern: /(?:pdfplumber-python|pdfminer-six|test\/fixtures|external-(?:holdout-)?pdfs|parity-cycles|upstream-contract-pdfs)\//i
      },
      {
        label: "PDF filename literal",
        pattern: /["'`][^"'`\n]*\.pdf(?:[#?][^"'`\n]*)?["'`]/i
      },
      {
        label: "real-cycle identity",
        pattern: /\b(?:sourceDocumentId|selectedPage|working-manifest|holdout-manifest|classified-holdout|golden)\b/i
      },
      {
        label: "filename-based branching helper",
        pattern: /\b(?:path\.)?basename\s*\(/i
      },
      {
        label: "PDF suffix branch",
        pattern: /\.endsWith\(\s*["'`][^"'`]*\.pdf/i
      }
    ];

    const violations = sourceFiles.flatMap((file) => {
      const text = fs.readFileSync(path.join(repoRoot, file), "utf8");
      return forbiddenPatterns
        .filter(({ pattern }) => pattern.test(text))
        .map(({ label }) => `${file}: ${label}`);
    });

    expect(violations).toEqual([]);
  });
});
