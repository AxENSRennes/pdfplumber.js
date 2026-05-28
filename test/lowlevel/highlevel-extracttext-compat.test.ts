import { execFileSync } from "node:child_process";

import { describe, expect, it } from "vitest";

import { open } from "../../src/index.js";
import { extractTextFromLayoutBoxesLikePdfminer } from "../../src/pdfminer-compat.js";

type PythonInputMode = "path" | "file";

interface ExtractTextCase {
  sample: string;
  laparams?: Record<string, unknown>;
}

function samplePath(sample: string): string {
  return `pdfminer-six/samples/${sample}`;
}

function pdfminerExtractText(path: string, laparams: Record<string, unknown> = {}, mode: PythonInputMode = "path"): string {
  const code = `
import json
from pdfminer.high_level import extract_text
from pdfminer.layout import LAParams

path = ${JSON.stringify(path)}
laparams = LAParams(**json.loads(${JSON.stringify(JSON.stringify(laparams))}))
if ${JSON.stringify(mode)} == "file":
    with open(path, "rb") as in_file:
        text = extract_text(in_file, laparams=laparams)
else:
    text = extract_text(path, laparams=laparams)
print(json.dumps(text))
`;
  return JSON.parse(execFileSync("wsl_venv/bin/python", ["-c", code], { encoding: "utf8" })) as string;
}

async function jsExtractTextLikePdfminer(path: string, laparams: Record<string, unknown> = {}): Promise<string> {
  const pdf = await open(path, { laparams });
  try {
    return extractTextFromLayoutBoxesLikePdfminer(pdf.pages);
  } finally {
    await pdf.close();
  }
}

describe("low-level pdfminer high-level extract_text compatibility", () => {
  it.each<ExtractTextCase>([
    { sample: "simple1.pdf" },
    { sample: "simple1.pdf", laparams: { boxes_flow: null } },
    { sample: "simple2.pdf" },
    { sample: "simple4.pdf" },
    { sample: "simple5.pdf" },
    { sample: "contrib/issue-886-xref-stream-widths.pdf" }
  ])("reconstructs pdfminer high-level text from layout boxes for $sample", async ({ sample, laparams = {} }) => {
    const path = samplePath(sample);
    const actual = await jsExtractTextLikePdfminer(path, laparams);

    expect(actual).toBe(pdfminerExtractText(path, laparams, "path"));
    expect(actual).toBe(pdfminerExtractText(path, laparams, "file"));
  });

  it("keeps the zlib-corrupted fixture prefix compatible with pdfminer", async () => {
    const path = samplePath("zen_of_python_corrupted.pdf");
    const actual = await jsExtractTextLikePdfminer(path);
    const expected = pdfminerExtractText(path).slice(0, "Mai 30, 18 13:27\n\nzen_of_python.txt".length);

    expect(actual.slice(0, expected.length)).toBe(expected);
  });

  it("preserves targeted CMap and xref-stream high-level text invariants", async () => {
    const issue495PdfObjRef = samplePath("contrib/issue_495_pdfobjref.pdf");
    expect((await jsExtractTextLikePdfminer(issue495PdfObjRef)).trim()).toBe(pdfminerExtractText(issue495PdfObjRef).trim());

    const issue566CMapBytes = samplePath("contrib/issue_566_test_1.pdf");
    expect((await jsExtractTextLikePdfminer(issue566CMapBytes)).trim()).toBe(pdfminerExtractText(issue566CMapBytes).trim());

    const issue566CidRange = samplePath("contrib/issue_566_test_2.pdf");
    expect((await jsExtractTextLikePdfminer(issue566CidRange)).trim()).toBe(pdfminerExtractText(issue566CidRange).trim());

    const issue625IdentityCmap = samplePath("contrib/issue-625-identity-cmap.pdf");
    expect((await jsExtractTextLikePdfminer(issue625IdentityCmap)).split(/\r?\n/)[6]).toBe(pdfminerExtractText(issue625IdentityCmap).split(/\r?\n/)[6]);

    const issue791NonUnicodeCMap = samplePath("contrib/issue-791-non-unicode-cmap.pdf");
    expect((await jsExtractTextLikePdfminer(issue791NonUnicodeCMap)).trim()).toBe(pdfminerExtractText(issue791NonUnicodeCMap).trim());

    const issue886XrefWidths = samplePath("contrib/issue-886-xref-stream-widths.pdf");
    expect((await jsExtractTextLikePdfminer(issue886XrefWidths)).trim()).toBe(pdfminerExtractText(issue886XrefWidths).trim());
  });
});
