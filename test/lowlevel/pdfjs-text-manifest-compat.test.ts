import { execFileSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import { open } from "../../src/index.js";

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, "../..");
const python = path.join(repoRoot, "wsl_venv/bin/python");

interface PdfjsTextManifestCase {
  id: string;
  path: string;
  firstPage?: number;
  lastPage?: number;
  pages?: number[];
}

interface PageTextSummary {
  page_number: number;
  width: number;
  height: number;
  text: string;
  chars: number;
  words: number;
}

interface TextSummary {
  pages: number;
  selected: PageTextSummary[];
}

const textManifestCases: PdfjsTextManifestCase[] = [
  { id: "arabiccidtruetype-text", path: "pdfjs/test/pdfs/ArabicCIDTrueType.pdf" },
  { id: "bug1130815-text", path: "test/fixtures/upstream-contract-pdfs/pdfjs-bug1130815.pdf", firstPage: 1, lastPage: 1 },
  { id: "bug931481", path: "test/fixtures/upstream-contract-pdfs/pdfjs-bug931481.pdf" },
  { id: "bug1245391-text", path: "pdfjs/test/pdfs/bug1245391_reduced.pdf" },
  { id: "bug1513120-text", path: "pdfjs/test/pdfs/bug1513120_reduced.pdf" },
  { id: "bug1627427", path: "pdfjs/test/pdfs/bug1627427_reduced.pdf" },
  { id: "bug1811668", path: "pdfjs/test/pdfs/bug1811668_reduced.pdf" },
  { id: "bug1947248-text", path: "pdfjs/test/pdfs/bug1947248_text.pdf" },
  { id: "bug864847-text", path: "pdfjs/test/pdfs/bug864847.pdf", firstPage: 1, lastPage: 1 },
  { id: "bug900822-encrypted-extract_0", path: "pdfjs/test/pdfs/bug900822.pdf" },
  { id: "bug946506-text", path: "pdfjs/test/pdfs/bug946506.pdf", firstPage: 1, lastPage: 1 },
  { id: "extgstate-text", path: "pdfjs/test/pdfs/extgstate.pdf" },
  { id: "IdentityToUnicodeMap_charCodeOf", path: "pdfjs/test/pdfs/IdentityToUnicodeMap_charCodeOf.pdf" },
  { id: "issue10301", path: "pdfjs/test/pdfs/issue10301.pdf" },
  { id: "issue1045", path: "pdfjs/test/pdfs/issue1045.pdf" },
  { id: "issue10529", path: "pdfjs/test/pdfs/issue10529.pdf" },
  { id: "issue11016", path: "pdfjs/test/pdfs/issue11016_reduced.pdf" },
  { id: "issue1127-text", path: "test/fixtures/upstream-contract-pdfs/pdfjs-issue1127.pdf", firstPage: 1, lastPage: 1 },
  { id: "issue11403-text", path: "pdfjs/test/pdfs/issue11403_reduced.pdf" },
  { id: "issue11651-text", path: "pdfjs/test/pdfs/issue11651.pdf" },
  { id: "issue11656", path: "pdfjs/test/pdfs/issue11656.pdf" },
  { id: "issue11713", path: "pdfjs/test/pdfs/issue11713.pdf" },
  { id: "issue12909", path: "test/fixtures/upstream-contract-pdfs/pdfjs-issue12909.pdf", firstPage: 1, lastPage: 1 },
  { id: "issue13845", path: "test/fixtures/upstream-contract-pdfs/pdfjs-issue13845.pdf", firstPage: 1, lastPage: 1 },
  { id: "issue14048", path: "pdfjs/test/pdfs/issue14048.pdf" },
  { id: "issue14415", path: "pdfjs/test/pdfs/issue14415.pdf" },
  { id: "issue14497", path: "pdfjs/test/pdfs/issue14497.pdf" },
  { id: "issue14627", path: "pdfjs/test/pdfs/issue14627.pdf" },
  { id: "issue14999", path: "pdfjs/test/pdfs/issue14999_reduced.pdf" },
  { id: "issue15352", path: "test/fixtures/upstream-contract-pdfs/pdfjs-issue15352.pdf", firstPage: 1, lastPage: 1 },
  { id: "issue15516", path: "pdfjs/test/pdfs/issue15516_reduced.pdf" },
  { id: "issue15629", path: "pdfjs/test/pdfs/issue15629.pdf" },
  { id: "issue15910", path: "pdfjs/test/pdfs/issue15910.pdf" },
  { id: "issue16221-text", path: "pdfjs/test/pdfs/issue16221.pdf" },
  { id: "issue16224-text", path: "pdfjs/test/pdfs/issue16224.pdf" },
  { id: "issue16843-text", path: "test/fixtures/upstream-contract-pdfs/pdfjs-issue16843-text.pdf", firstPage: 1, lastPage: 1 },
  { id: "issue18059-text", path: "pdfjs/test/pdfs/issue18059.pdf" },
  { id: "issue18117-text", path: "pdfjs/test/pdfs/issue18117.pdf", firstPage: 1, lastPage: 1 },
  { id: "issue1936-text", path: "test/fixtures/upstream-contract-pdfs/pdfjs-issue1936-text.pdf", firstPage: 1, lastPage: 1 },
  { id: "issue19624-text", path: "test/fixtures/upstream-contract-pdfs/pdfjs-issue19624-text.pdf", firstPage: 2, lastPage: 2 },
  { id: "issue19954", path: "test/fixtures/upstream-contract-pdfs/pdfjs-issue19954.pdf", firstPage: 1, lastPage: 1 },
  { id: "issue19800-text", path: "pdfjs/test/pdfs/issue19800.pdf" },
  { id: "issue19848-text", path: "pdfjs/test/pdfs/issue19848.pdf" },
  { id: "issue20930-text", path: "pdfjs/test/pdfs/issue20930.pdf" },
  { id: "operator-in-TJ-array", path: "pdfjs/test/pdfs/operator-in-TJ-array.pdf", firstPage: 1, lastPage: 1 },
  { id: "issue2017-text", path: "pdfjs/test/pdfs/issue2017r.pdf" },
  { id: "issue4665-text", path: "pdfjs/test/pdfs/issue4665.pdf" },
  { id: "issue4684-text", path: "pdfjs/test/pdfs/issue4684.pdf" },
  { id: "issue5421-text", path: "pdfjs/test/pdfs/issue5421.pdf" },
  { id: "issue5734-text", path: "pdfjs/test/pdfs/issue5734.pdf" },
  { id: "issue5808-text", path: "pdfjs/test/pdfs/issue5808.pdf" },
  { id: "issue5896-text", path: "pdfjs/test/pdfs/issue5896.pdf" },
  { id: "issue5972", path: "pdfjs/test/pdfs/issue5972.pdf" },
  { id: "issue6019-text", path: "pdfjs/test/pdfs/issue6019.pdf" },
  { id: "issue6342-text", path: "pdfjs/test/pdfs/issue6342.pdf" },
  { id: "issue6387-text", path: "pdfjs/test/pdfs/issue6387.pdf" },
  { id: "issue6605", path: "pdfjs/test/pdfs/issue6605.pdf" },
  { id: "issue6612-text", path: "pdfjs/test/pdfs/issue6612.pdf" },
  { id: "issue6901-text", path: "pdfjs/test/pdfs/issue6901.pdf" },
  { id: "issue6962", path: "pdfjs/test/pdfs/issue6962.pdf" },
  { id: "issue7180-text", path: "pdfjs/test/pdfs/issue7180.pdf" },
  { id: "issue7492-text", path: "pdfjs/test/pdfs/issue7492.pdf" },
  { id: "issue7580-text", path: "pdfjs/test/pdfs/issue7580.pdf" },
  { id: "issue7878", path: "pdfjs/test/pdfs/issue7878.pdf" },
  { id: "issue8229", path: "pdfjs/test/pdfs/issue8229.pdf" },
  { id: "issue8372-text", path: "pdfjs/test/pdfs/issue8372.pdf" },
  { id: "issue8702-text", path: "pdfjs/test/pdfs/issue8702.pdf", firstPage: 1, lastPage: 1 },
  { id: "issue2770-text", path: "test/fixtures/upstream-contract-pdfs/pdfjs-issue2770.pdf", firstPage: 1, lastPage: 1 },
  { id: "issue3064-text", path: "test/fixtures/upstream-contract-pdfs/pdfjs-issue3064.pdf", firstPage: 1, lastPage: 1 },
  { id: "issue3925", path: "test/fixtures/upstream-contract-pdfs/pdfjs-issue3925.pdf", firstPage: 1, lastPage: 1 },
  { id: "issue4550-text", path: "pdfjs/test/pdfs/issue4550.pdf" },
  { id: "issue9186", path: "test/fixtures/upstream-contract-pdfs/pdfjs-issue9186.pdf", firstPage: 1, lastPage: 1 },
  { id: "mao-text", path: "test/fixtures/upstream-contract-pdfs/pdfjs-mao.pdf", firstPage: 1, lastPage: 1 },
  { id: "issue9655-text", path: "pdfjs/test/pdfs/issue9655_reduced.pdf" },
  { id: "preistabelle-text", path: "test/fixtures/upstream-contract-pdfs/pdfjs-preistabelle.pdf", firstPage: 1, lastPage: 1 },
  { id: "reduced_planck_constant", path: "test/fixtures/upstream-contract-pdfs/pdfjs-reduced_planck_constant.pdf" },
  { id: "rotated-text", path: "pdfjs/test/pdfs/rotated.pdf" },
  { id: "simpletype3font-text", path: "pdfjs/test/pdfs/simpletype3font.pdf" },
  { id: "taro-text", path: "test/fixtures/upstream-contract-pdfs/pdfjs-TaroUTR50SortedList112.pdf", firstPage: 1, lastPage: 4 },
  { id: "tracemonkey-extract_0_2_12", path: "pdfjs/test/pdfs/tracemonkey.pdf", pages: [1, 3, 13] },
  { id: "tracemonkey-text", path: "pdfjs/test/pdfs/tracemonkey.pdf" },
  { id: "zero_descent", path: "pdfjs/test/pdfs/zero_descent.pdf", firstPage: 1, lastPage: 1 }
];

function fixture(relativePath: string): string {
  return path.join(repoRoot, relativePath);
}

function cleanNumber(value: number): number {
  return Number(value.toFixed(3));
}

function pythonTextSummaries(cases: PdfjsTextManifestCase[]): Record<string, TextSummary> {
  const code = `
import json
import sys
import pdfplumber

cases = json.load(sys.stdin)

def clean_number(value):
    return round(value, 3)

out = {}
for case in cases:
    with pdfplumber.open(case["path"]) as pdf:
        pages = case.get("pages")
        if pages:
            selected_pages = [pdf.pages[page_number - 1] for page_number in pages]
        else:
            first_page = case.get("firstPage")
            last_page = case.get("lastPage")
            start = first_page - 1 if first_page else 0
            end = last_page if last_page else len(pdf.pages)
            selected_pages = pdf.pages[start:end]
        selected = []
        for page in selected_pages:
            selected.append({
                "page_number": page.page_number,
                "width": clean_number(page.width),
                "height": clean_number(page.height),
                "text": page.extract_text() or "",
                "chars": len(page.chars),
                "words": len(page.extract_words()),
            })
        out[case["id"]] = {"pages": len(pdf.pages), "selected": selected}

print(json.dumps(out, ensure_ascii=False))
`;
  return JSON.parse(execFileSync(python, ["-c", code], { cwd: repoRoot, input: JSON.stringify(cases), encoding: "utf8" })) as Record<string, TextSummary>;
}

async function jsTextSummary(pdfCase: PdfjsTextManifestCase): Promise<TextSummary> {
  const document = await open(fixture(pdfCase.path));
  try {
    const start = pdfCase.firstPage ? pdfCase.firstPage - 1 : 0;
    const end = pdfCase.lastPage ?? document.pages.length;
    const pages = pdfCase.pages?.map((pageNumber) => document.pages[pageNumber - 1]) ?? document.pages.slice(start, end);
    const selected: PageTextSummary[] = [];
    for (const page of pages) {
      selected.push({
        page_number: page.pageNumber,
        width: cleanNumber(page.width),
        height: cleanNumber(page.height),
        text: await page.extractText(),
        chars: page.chars.length,
        words: (await page.extractWords()).length
      });
    }
    return { pages: document.pages.length, selected };
  } finally {
    await document.close();
  }
}

describe("PDF.js text manifest fixtures through the public extraction API", () => {
  it("matches Python pdfplumber on selected-page text and structured counts", async () => {
    const expected = pythonTextSummaries(textManifestCases);
    for (const pdfCase of textManifestCases) {
      await expect(jsTextSummary(pdfCase), pdfCase.id).resolves.toEqual(expected[pdfCase.id]);
    }
  }, 30_000);
});
