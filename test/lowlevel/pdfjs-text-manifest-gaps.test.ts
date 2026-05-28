import { execFileSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import { open } from "../../src/index.js";

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, "../..");
const python = path.join(repoRoot, "wsl_venv/bin/python");

interface GapCase {
  id: string;
  path: string;
  category: "cid-text-normalization" | "layout-ordering" | "glyph-decoding" | "malformed-content-recovery";
  expected: {
    pythonChars: number;
    jsChars: number;
    pythonWords: number;
    jsWords: number;
  };
}

interface TextStats {
  text: string;
  chars: number;
  words: number;
}

const gapCases: GapCase[] = [
  {
    id: "issue11403-text",
    path: "pdfjs/test/pdfs/issue11403_reduced.pdf",
    category: "cid-text-normalization",
    expected: { pythonChars: 25, jsChars: 25, pythonWords: 6, jsWords: 6 }
  },
  {
    id: "issue14415",
    path: "pdfjs/test/pdfs/issue14415.pdf",
    category: "layout-ordering",
    expected: { pythonChars: 395, jsChars: 395, pythonWords: 56, jsWords: 335 }
  },
  {
    id: "issue14497",
    path: "pdfjs/test/pdfs/issue14497.pdf",
    category: "layout-ordering",
    expected: { pythonChars: 224, jsChars: 224, pythonWords: 87, jsWords: 93 }
  },
  {
    id: "issue14999",
    path: "pdfjs/test/pdfs/issue14999_reduced.pdf",
    category: "layout-ordering",
    expected: { pythonChars: 55, jsChars: 55, pythonWords: 52, jsWords: 55 }
  },
  {
    id: "issue15352",
    path: "test/fixtures/upstream-contract-pdfs/pdfjs-issue15352.pdf",
    category: "layout-ordering",
    expected: { pythonChars: 4679, jsChars: 4679, pythonWords: 351, jsWords: 348 }
  },
  {
    id: "issue18059-text",
    path: "pdfjs/test/pdfs/issue18059.pdf",
    category: "cid-text-normalization",
    expected: { pythonChars: 52, jsChars: 52, pythonWords: 1, jsWords: 1 }
  },
  {
    id: "issue1936-text",
    path: "test/fixtures/upstream-contract-pdfs/pdfjs-issue1936-text.pdf",
    category: "glyph-decoding",
    expected: { pythonChars: 1983, jsChars: 1983, pythonWords: 96, jsWords: 96 }
  },
  {
    id: "issue18117-text",
    path: "pdfjs/test/pdfs/issue18117.pdf",
    category: "malformed-content-recovery",
    expected: { pythonChars: 0, jsChars: 94, pythonWords: 0, jsWords: 10 }
  },
  {
    id: "issue2017-text",
    path: "pdfjs/test/pdfs/issue2017r.pdf",
    category: "glyph-decoding",
    expected: { pythonChars: 84, jsChars: 84, pythonWords: 4, jsWords: 4 }
  },
  {
    id: "issue4550-text",
    path: "pdfjs/test/pdfs/issue4550.pdf",
    category: "glyph-decoding",
    expected: { pythonChars: 3, jsChars: 3, pythonWords: 1, jsWords: 1 }
  },
  {
    id: "issue6901-text",
    path: "pdfjs/test/pdfs/issue6901.pdf",
    category: "cid-text-normalization",
    expected: { pythonChars: 19, jsChars: 19, pythonWords: 1, jsWords: 6 }
  },
  {
    id: "issue7580-text",
    path: "pdfjs/test/pdfs/issue7580.pdf",
    category: "glyph-decoding",
    expected: { pythonChars: 23, jsChars: 23, pythonWords: 5, jsWords: 5 }
  },
  {
    id: "operator-in-TJ-array",
    path: "pdfjs/test/pdfs/operator-in-TJ-array.pdf",
    category: "malformed-content-recovery",
    expected: { pythonChars: 0, jsChars: 39, pythonWords: 0, jsWords: 1 }
  },
  {
    id: "reduced_planck_constant",
    path: "test/fixtures/upstream-contract-pdfs/pdfjs-reduced_planck_constant.pdf",
    category: "glyph-decoding",
    expected: { pythonChars: 1, jsChars: 1, pythonWords: 1, jsWords: 1 }
  },
  {
    id: "tracemonkey-extract_0_2_12",
    path: "pdfjs/test/pdfs/tracemonkey.pdf",
    category: "glyph-decoding",
    expected: { pythonChars: 4282, jsChars: 4282, pythonWords: 242, jsWords: 242 }
  },
  {
    id: "tracemonkey-text",
    path: "pdfjs/test/pdfs/tracemonkey.pdf",
    category: "glyph-decoding",
    expected: { pythonChars: 4282, jsChars: 4282, pythonWords: 242, jsWords: 242 }
  }
];

function fixture(relativePath: string): string {
  return path.join(repoRoot, relativePath);
}

function pythonTextStats(relativePath: string): TextStats {
  const code = `
import json
import pdfplumber

with pdfplumber.open(${JSON.stringify(relativePath)}) as pdf:
    page = pdf.pages[0]
    print(json.dumps({
        "text": page.extract_text() or "",
        "chars": len(page.chars),
        "words": len(page.extract_words()),
    }, ensure_ascii=False))
`;
  return JSON.parse(execFileSync(python, ["-c", code], { cwd: repoRoot, encoding: "utf8" })) as TextStats;
}

async function jsTextStats(relativePath: string): Promise<TextStats> {
  const document = await open(fixture(relativePath));
  try {
    const page = document.pages[0];
    return {
      text: await page.extractText(),
      chars: page.chars.length,
      words: (await page.extractWords()).length
    };
  } finally {
    await document.close();
  }
}

describe("classified PDF.js text manifest backend gaps", () => {
  it("keeps known pdfminer/pdf.js public text divergences explicit", async () => {
    for (const gapCase of gapCases) {
      const pythonStats = pythonTextStats(gapCase.path);
      const jsStats = await jsTextStats(gapCase.path);

      expect(
        {
          category: gapCase.category,
          pythonChars: pythonStats.chars,
          jsChars: jsStats.chars,
          pythonWords: pythonStats.words,
          jsWords: jsStats.words
        },
        gapCase.id
      ).toEqual({ category: gapCase.category, ...gapCase.expected });
      expect(jsStats.text, gapCase.id).not.toBe(pythonStats.text);
    }
  }, 30_000);
});
